use crate::{
    attachments::Attachments,
    models::{Block, Message, Response, Usage},
    util,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
pub(super) struct CallBuffer {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) arguments: String,
    pub(super) arguments_seen: bool,
    pub(super) argument_value: Option<Value>,
    pub(super) argument_conflict: bool,
    pub(super) initial: Value,
}

impl CallBuffer {
    pub(super) fn parsed_arguments(&self) -> (Value, Option<String>) {
        if self.argument_conflict {
            return (
                json!([self.argument_value, self.arguments]),
                Some("工具参数包含相互冲突的内容或分片".into()),
            );
        }
        if let Some(value) = &self.argument_value {
            return tool_argument_value(value.clone());
        }
        if self.arguments_seen {
            parse_tool_arguments(&self.arguments)
        } else if self.initial.is_null() {
            (json!({}), None)
        } else {
            tool_argument_value(self.initial.clone())
        }
    }
}

pub(super) fn truncated_tool_name(name: Option<&str>) -> String {
    match name {
        Some(name)
            if !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') =>
        {
            name.into()
        }
        _ => "未知工具".into(),
    }
}

pub(super) fn json_truncated_tools(value: &Value, style: &str) -> Vec<String> {
    let calls = match style {
        "anthropic" => value["content"].as_array(),
        "openai_responses" => value["output"].as_array(),
        _ => value["choices"][0]["message"]["tool_calls"].as_array(),
    };
    calls
        .into_iter()
        .flatten()
        .filter(|call| match style {
            "anthropic" => call["type"] == "tool_use",
            "openai_responses" => call["type"] == "function_call",
            _ => true,
        })
        .take(128)
        .map(|call| {
            let name = match style {
                "anthropic" | "openai_responses" => call["name"].as_str(),
                _ => call["function"]["name"].as_str(),
            };
            truncated_tool_name(name)
        })
        .collect()
}

pub(super) fn push_order(order: &mut Vec<usize>, index: usize) {
    if !order.contains(&index) {
        order.push(index);
    }
}

pub(super) fn append_openai_tool_delta(
    delta: &Value,
    calls: &mut BTreeMap<usize, CallBuffer>,
    order: &mut Vec<usize>,
    last: &mut Option<usize>,
    response_bytes: &mut usize,
) {
    let id = delta["id"].as_str().filter(|id| !id.is_empty());
    let free_index = || {
        (0..=calls.len())
            .find(|index| !calls.contains_key(index))
            .unwrap()
    };
    let index = delta["index"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .unwrap_or_else(|| {
            if let Some(id) = id {
                if let Some(index) = calls
                    .iter()
                    .find_map(|(index, call)| (call.id == id).then_some(*index))
                {
                    return index;
                }
                if let Some(index) = *last
                    && let Some(call) = calls.get(&index)
                    && call.id.is_empty()
                    && (call.name.is_empty()
                        || delta["function"]["name"].as_str().is_none_or(str::is_empty))
                {
                    return index;
                }
                free_index()
            } else {
                last.unwrap_or_else(free_index)
            }
        });
    calls.entry(index).or_insert_with(|| {
        order.push(index);
        CallBuffer::default()
    });
    let call = calls.get_mut(&index).unwrap();
    if let Some(id) = id {
        *response_bytes += id.len();
        call.id = id.into();
    }
    if let Some(name) = delta["function"]["name"]
        .as_str()
        .filter(|name| !name.is_empty())
    {
        *response_bytes += name.len();
        call.name = name.into();
    }
    if let Some(argument) = delta["function"].get("arguments") {
        if let Some(piece) = argument.as_str() {
            call.arguments_seen = true;
            *response_bytes += piece.len();
            call.argument_conflict |= !piece.is_empty() && call.argument_value.is_some();
            call.arguments.push_str(piece);
        } else {
            *response_bytes += argument.to_string().len();
            call.argument_conflict |= !call.arguments.is_empty()
                || call
                    .argument_value
                    .as_ref()
                    .is_some_and(|old| old != argument);
            if call.argument_value.is_none() {
                call.argument_value = Some(argument.clone());
            }
        }
    }
    *last = Some(index);
}

pub(super) fn valid_summary(text: &str) -> bool {
    text.chars().count() <= 12_000
        && [
            "## Goal",
            "## Constraints & Preferences",
            "## Progress",
            "### Done",
            "### In Progress",
            "### Blocked",
            "## Key Decisions",
            "## Next Steps",
            "## Critical Context",
            "### Read Files",
            "### Modified Files",
        ]
        .iter()
        .all(|heading| text.lines().any(|line| line.trim() == *heading))
}

// false: low/medium/high; true: low/medium/high/xhigh.
pub(crate) fn openai_reasoning_family(model: &str) -> Option<bool> {
    let model = model.strip_prefix("openai/").unwrap_or(model);
    if let Some(rest) = model.strip_prefix("gpt-5") {
        if !rest.is_empty() && !rest.starts_with(['-', '.']) {
            return None;
        }
        let version = rest.strip_prefix('.').and_then(|version| {
            version
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse::<u32>()
                .ok()
        });
        return Some(version.is_some_and(|version| version >= 2));
    }
    let suffix = model.strip_prefix('o')?;
    let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || (suffix.len() > digits && !suffix[digits..].starts_with('-')) {
        return None;
    }
    Some(false)
}

pub(super) fn openai_chat_tools_require_none(model: &str) -> bool {
    let model = model.strip_prefix("openai/").unwrap_or(model);
    model
        .strip_prefix("gpt-5.")
        .and_then(|version| {
            version
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse::<u32>()
                .ok()
        })
        .is_some_and(|version| version >= 4)
}

pub(crate) fn chat_supported(model: &str, compat: &Value) -> bool {
    compat["chatSupported"].as_bool().unwrap_or_else(|| {
        let model = model.strip_prefix("openai/").unwrap_or(model);
        ![
            "gpt-5-pro",
            "gpt-5.2-pro",
            "gpt-5.4-pro",
            "gpt-5.5-pro",
            "o1-pro",
            "o3-pro",
        ]
        .iter()
        .any(|name| {
            model == *name
                || model
                    .strip_prefix(name)
                    .is_some_and(|suffix| suffix.starts_with('-'))
        })
    })
}

pub(super) fn anthropic_tool_ids(messages: &[Message]) -> HashMap<String, String> {
    let ids = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            Block::ToolUse { id, .. } => Some(id),
            Block::ToolResult { tool_use_id, .. } => Some(tool_use_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let valid = |id: &str| {
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    };
    let mut used = ids
        .iter()
        .filter(|id| valid(id))
        .map(|id| (*id).clone())
        .collect::<HashSet<_>>();
    let mut mapped = HashMap::new();
    for id in ids {
        if valid(id) || mapped.contains_key(id) {
            continue;
        }
        let hash = util::hash(id.as_bytes());
        let mut counter = 0u64;
        let replacement = loop {
            let suffix = if counter == 0 {
                String::new()
            } else {
                format!("_{counter}")
            };
            let candidate = format!("mc_{}{}", &hash[..61 - suffix.len()], suffix);
            if used.insert(candidate.clone()) {
                break candidate;
            }
            counter += 1;
        };
        mapped.insert(id.clone(), replacement);
    }
    mapped
}
pub(super) fn anthropic_prompt_cache_enabled() -> bool {
    !std::env::var("MINICODE_ANTHROPIC_PROMPT_CACHE")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
}
/// Strict Anthropic endpoints (Bedrock, some gateways) reject adjacent
/// messages with the same role, while the runtime appends user notices right
/// after user tool results. Merge them on the wire only, so persisted history
/// stays untouched and sessions written before this fix still resume.
pub(super) fn normalize_anthropic(chat: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(chat.len());
    for mut message in chat {
        if let Some(previous) = out.last_mut().filter(|p| p["role"] == message["role"]) {
            if let (Some(into), Value::Array(from)) = (
                previous["content"].as_array_mut(),
                message["content"].take(),
            ) {
                into.extend(from);
            }
            continue;
        }
        out.push(message);
    }
    for message in out.iter_mut().filter(|m| m["role"] == "user") {
        if let Some(content) = message["content"].as_array_mut() {
            // Stable: tool results first, other blocks keep their order.
            content.sort_by_key(|block| block["type"] != "tool_result");
        }
    }
    out
}
pub(super) fn media_part(b: &Block, store: &Attachments, style: &str) -> Result<Value> {
    let Block::Media {
        kind,
        mime_type,
        name,
        ..
    } = b
    else {
        bail!("not media")
    };
    let (mime_type, data) = if kind == "image" {
        store.encoded_image(b)?
    } else {
        (mime_type.clone(), store.encoded(b)?)
    };
    if style == "anthropic" {
        return Ok(
            json!({"type":if kind=="image"{"image"}else{"document"},"source":{"type":"base64","media_type":mime_type,"data":data}}),
        );
    }
    let uri = format!("data:{mime_type};base64,{data}");
    if style == "openai_responses" {
        return Ok(match kind.as_str() {
            "image" => json!({"type":"input_image","image_url":uri}),
            "pdf" => json!({"type":"input_file","filename":name,"file_data":uri}),
            "audio" => {
                json!({"type":"input_audio","input_audio":{"data":data,"format":if mime_type=="audio/wav"{"wav"}else{"mp3"}}})
            }
            _ => bail!("OpenAI Responses 接口不支持原生视频附件"),
        });
    }
    Ok(match kind.as_str() {
        "image" => json!({"type":"image_url","image_url":{"url":uri}}),
        "pdf" => json!({"type":"file","file":{"filename":name,"file_data":uri}}),
        "audio" => {
            json!({"type":"input_audio","input_audio":{"data":data,"format":if mime_type=="audio/wav"{"wav"}else{"mp3"}}})
        }
        _ => json!({"type":"video_url","video_url":{"url":uri}}),
    })
}
pub(super) fn parse_usage(v: &Value, style: &str) -> Usage {
    if style == "openai_responses" {
        let has_data = ["input_tokens", "output_tokens"]
            .iter()
            .any(|key| v[*key].as_u64().is_some())
            || ["cached_tokens", "cache_creation_tokens"]
                .iter()
                .any(|key| v["input_tokens_details"][*key].as_u64().is_some());
        if !v.is_object() || !has_data {
            return Usage {
                available: false,
                ..Usage::default()
            };
        }
        return Usage {
            input_tokens: v["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["output_tokens"].as_u64().unwrap_or(0),
            cache_read_tokens: v["input_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_write_tokens: v["input_tokens_details"]["cache_creation_tokens"]
                .as_u64()
                .unwrap_or(0),
            available: true,
        };
    }
    let fields: &[&str] = if style == "anthropic" {
        &[
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
    } else {
        &["prompt_tokens", "completion_tokens"]
    };
    let has_data = fields.iter().any(|key| v[*key].as_u64().is_some())
        || (style != "anthropic"
            && ["cached_tokens", "cache_creation_tokens"]
                .iter()
                .any(|key| v["prompt_tokens_details"][*key].as_u64().is_some()));
    if !v.is_object() || !has_data {
        return Usage {
            available: false,
            ..Usage::default()
        };
    }
    if style == "anthropic" {
        let read = v["cache_read_input_tokens"].as_u64().unwrap_or(0);
        let write = v["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        Usage {
            input_tokens: v["input_tokens"].as_u64().unwrap_or(0) + read + write,
            output_tokens: v["output_tokens"].as_u64().unwrap_or(0),
            cache_read_tokens: read,
            cache_write_tokens: write,
            available: true,
        }
    } else {
        Usage {
            input_tokens: v["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["completion_tokens"].as_u64().unwrap_or(0),
            cache_read_tokens: v["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_write_tokens: v["prompt_tokens_details"]["cache_creation_tokens"]
                .as_u64()
                .unwrap_or(0),
            available: true,
        }
    }
}
pub(super) fn tool_argument_value(value: Value) -> (Value, Option<String>) {
    if value.is_object() {
        (value, None)
    } else {
        (value, Some("工具参数必须是 JSON 对象".into()))
    }
}
pub(super) fn parse_tool_arguments(raw: &str) -> (Value, Option<String>) {
    match serde_json::from_str(raw) {
        Ok(value) => tool_argument_value(value),
        Err(error) => (json!(raw), Some(format!("工具参数不是合法 JSON：{error}"))),
    }
}
pub(super) fn parse_openai(v: &Value, source: &str) -> Result<Response> {
    let choice = v["choices"]
        .as_array()
        .and_then(|v| v.first())
        .context("missing provider choice")?;
    let message = &choice["message"];
    if choice["finish_reason"] == "content_filter"
        || message["refusal"].as_str().is_some_and(|s| !s.is_empty())
    {
        bail!("provider rejected the response")
    }
    let mut blocks = vec![];
    if let Some(thinking) = message["reasoning_content"]
        .as_str()
        .or_else(|| message["reasoning"].as_str())
        .filter(|s| !s.is_empty())
    {
        blocks.push(Block::Thinking {
            thinking: thinking.into(),
            signature: None,
            source: source.into(),
        });
    }
    if let Some(text) = message["content"].as_str().filter(|s| !s.is_empty()) {
        blocks.push(Block::text(text))
    }
    let truncated = choice["finish_reason"] == "length";
    if let Some(calls) = message["tool_calls"].as_array().filter(|_| !truncated) {
        for call in calls {
            let arguments = &call["function"]["arguments"];
            let (input, input_error) = match arguments.as_str() {
                Some(raw) => parse_tool_arguments(raw),
                None => tool_argument_value(arguments.clone()),
            };
            blocks.push(Block::ToolUse {
                id: call["id"].as_str().unwrap_or_default().into(),
                name: util::string(&call["function"], "name")?.into(),
                input,
                input_error,
            });
        }
    }
    Ok(Response {
        blocks,
        stop_reason: match choice["finish_reason"].as_str() {
            Some("tool_calls") => "tool_use",
            Some("length") => "max_tokens",
            _ => "end_turn",
        }
        .into(),
        usage: parse_usage(&v["usage"], "openai"),
    })
}
pub(super) fn parse_anthropic(v: &Value, source: &str) -> Result<Response> {
    let mut blocks = vec![];
    let truncated = v["stop_reason"] == "max_tokens";
    for b in v["content"]
        .as_array()
        .context("missing provider content")?
    {
        match b["type"].as_str() {
            Some("text") => {
                let text = util::string(b, "text")?;
                if !text.is_empty() {
                    blocks.push(Block::text(text));
                }
            }
            Some("thinking") => blocks.push(Block::Thinking {
                thinking: util::string(b, "thinking")?.into(),
                signature: b["signature"].as_str().map(str::to_string),
                source: source.into(),
            }),
            Some("redacted_thinking") => blocks.push(Block::RedactedThinking {
                data: util::string(b, "data")?.into(),
                source: source.into(),
            }),
            Some("tool_use") if !truncated => blocks.push(Block::ToolUse {
                id: b["id"].as_str().unwrap_or_default().into(),
                name: util::string(b, "name")?.into(),
                input: b["input"].clone(),
                input_error: (!b["input"].is_object()).then(|| "工具参数必须是 JSON 对象".into()),
            }),
            _ => {}
        }
    }
    Ok(Response {
        blocks,
        stop_reason: v["stop_reason"].as_str().unwrap_or("end_turn").into(),
        usage: parse_usage(&v["usage"], "anthropic"),
    })
}

pub(super) fn validate_fake(v: &Value) -> Result<()> {
    fn fields(v: &Value, keys: &[&str]) -> Result<()> {
        for key in v
            .as_object()
            .context("fake script entry must be an object")?
            .keys()
        {
            if !keys.contains(&key.as_str()) {
                bail!("脚本包含未知字段: {key}")
            }
        }
        Ok(())
    }
    fields(
        v,
        &[
            "turns",
            "exhausted_text",
            "default_input_tokens",
            "default_output_tokens",
            "summary",
            "summary_error",
            "context_window",
        ],
    )?;
    for key in ["summary", "summary_error"] {
        if !v[key].is_null() && !v[key].is_string() {
            bail!("{key} must be a string");
        }
    }
    if v.get("context_window").is_some() && v["context_window"].as_u64().is_none_or(|n| n == 0) {
        bail!("context_window must be a positive integer");
    }
    let empty = vec![];
    let turns = match v.get("turns") {
        Some(turns) => turns.as_array().context("turns must be an array")?,
        None => &empty,
    };
    for turn in turns {
        fields(
            turn,
            &[
                "text",
                "tool_calls",
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
                "stop_reason",
                "error",
                "panic",
            ],
        )?;
        for key in ["error", "panic"] {
            if !turn[key].is_null() && !turn[key].is_string() {
                bail!("{key} must be a string")
            }
        }
        if let Some(calls) = turn["tool_calls"].as_array() {
            for c in calls {
                fields(c, &["name", "arguments", "id"])?;
                util::string(c, "name")?;
                if !c["arguments"].is_null() && !c["arguments"].is_object() {
                    bail!("tool arguments must be an object")
                }
            }
        }
    }
    Ok(())
}
pub fn normalize_tool_ids(response: &mut Response, previous: &HashSet<String>) -> usize {
    let mut reserved = previous.clone();
    reserved.extend(response.blocks.iter().filter_map(|block| match block {
        Block::ToolUse { id, .. } if !id.trim().is_empty() && id.len() <= 256 => Some(id.clone()),
        _ => None,
    }));
    let mut seen = previous.clone();
    let mut count = 0;
    for block in &mut response.blocks {
        if let Block::ToolUse { id, .. } = block
            && (id.trim().is_empty() || id.len() > 256 || !seen.insert(id.clone()))
        {
            loop {
                let candidate = format!("tool_{}", util::id());
                if reserved.insert(candidate.clone()) && seen.insert(candidate.clone()) {
                    *id = candidate;
                    break;
                }
            }
            count += 1;
        }
    }
    count
}

pub fn validate_response(response: &Response, messages: &[Message]) -> Result<()> {
    if response.blocks.is_empty()
        && !matches!(
            response.stop_reason.as_str(),
            "end_turn" | "stop_sequence" | "max_tokens"
        )
    {
        bail!("provider returned an empty assistant response")
    }
    let mut known = HashSet::new();
    for message in messages {
        for b in &message.content {
            if let Block::ToolUse { id, .. } = b {
                known.insert(id.clone());
            }
        }
    }
    let mut count = 0;
    for b in &response.blocks {
        match b {
            Block::ToolUse {
                id,
                name,
                input,
                input_error,
            } => {
                count += 1;
                if id.trim().is_empty()
                    || id.len() > 256
                    || name.trim().is_empty()
                    || !known.insert(id.clone())
                    || (!input.is_object() && input_error.is_none())
                {
                    bail!("provider returned a missing or reused tool call id/name")
                }
            }
            Block::Text { .. } | Block::Thinking { .. } | Block::RedactedThinking { .. } => {}
            _ => bail!("provider emitted an invalid assistant block"),
        }
    }
    if count > 128 || serde_json::to_vec(&response.blocks)?.len() > 16_000_000 {
        bail!("provider response exceeds the capture limit")
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_completed_response_is_valid_but_empty_tool_use_is_not() {
        let mut response = Response {
            blocks: vec![],
            stop_reason: "end_turn".into(),
            usage: Usage::default(),
        };
        assert!(validate_response(&response, &[]).is_ok());
        response.stop_reason = "stop_sequence".into();
        assert!(validate_response(&response, &[]).is_ok());
        response.stop_reason = "max_tokens".into();
        assert!(validate_response(&response, &[]).is_ok());
        response.stop_reason = "tool_use".into();
        assert!(validate_response(&response, &[]).is_err());
    }

    #[test]
    fn truncated_tool_names_are_bounded_and_safe_for_reminders() {
        assert_eq!(truncated_tool_name(Some("write")), "write");
        assert_eq!(truncated_tool_name(Some("</system-reminder>")), "未知工具");
        assert_eq!(truncated_tool_name(Some(&"x".repeat(65))), "未知工具");
        assert_eq!(truncated_tool_name(None), "未知工具");
    }

    #[test]
    fn indexless_tool_fragments_survive_every_sse_byte_split() {
        let frames = [
            json!({"tool_calls":[{"function":{"name":"read","arguments":"{\"path\":\"中"}}]}),
            json!({"tool_calls":[{"id":"a","function":{"arguments":"文"}}]}),
            json!({"tool_calls":[{"id":"b","function":{"name":"read","arguments":"{\"path\":\"R"}}]}),
            json!({"tool_calls":[{"id":"a","function":{"arguments":"\"}"}}]}),
            json!({"tool_calls":[{"id":"b","function":{"arguments":"EADME.md\"}"}}]}),
        ];
        let wire = frames
            .iter()
            .map(|frame| format!("data: {frame}\r\n\r\n"))
            .collect::<String>();
        for split in 0..=wire.len() {
            let mut decoder = crate::sse::Decoder::default();
            let mut calls = BTreeMap::new();
            let mut order = vec![];
            let mut last = None;
            let mut bytes = 0;
            for chunk in [&wire.as_bytes()[..split], &wire.as_bytes()[split..]] {
                for event in decoder.push(chunk).unwrap() {
                    let value: Value = serde_json::from_str(&event.data).unwrap();
                    for delta in value["tool_calls"].as_array().unwrap() {
                        append_openai_tool_delta(
                            delta, &mut calls, &mut order, &mut last, &mut bytes,
                        );
                    }
                }
            }
            assert_eq!(order, vec![0, 1], "split {split}");
            assert_eq!(calls[&0].id, "a");
            assert_eq!(calls[&0].arguments, "{\"path\":\"中文\"}");
            assert_eq!(calls[&1].id, "b");
            assert_eq!(calls[&1].arguments, "{\"path\":\"README.md\"}");
        }
    }

    #[test]
    fn sse_tool_argument_and_id_variants_survive_every_byte_split() {
        let frame = json!({"tool_calls":[
            {"index":0,"id":"","function":{"name":"read","arguments":{"path":"中文"}}},
            {"index":1,"id":"duplicate","function":{"name":"read","arguments":"{\"path\":"}},
            {"index":2,"id":"duplicate","function":{"name":"read","arguments":"{\"path\":\"README.md\"}"}},
            {"id":"distinct","function":{"name":"read","arguments":{"path":"README.md"}}}
        ]});
        let wire = format!("data: {frame}\r\n\r\n");
        for split in 0..=wire.len() {
            let mut decoder = crate::sse::Decoder::default();
            let mut calls = BTreeMap::new();
            let mut order = vec![];
            let mut last = None;
            let mut bytes = 0;
            for chunk in [&wire.as_bytes()[..split], &wire.as_bytes()[split..]] {
                for event in decoder.push(chunk).unwrap() {
                    let value: Value = serde_json::from_str(&event.data).unwrap();
                    for delta in value["tool_calls"].as_array().unwrap() {
                        append_openai_tool_delta(
                            delta, &mut calls, &mut order, &mut last, &mut bytes,
                        );
                    }
                }
            }
            assert_eq!(order, vec![0, 1, 2, 3], "split {split}");
            let mut blocks = vec![];
            for index in order {
                let call = calls.remove(&index).unwrap();
                let (input, input_error) = call.parsed_arguments();
                blocks.push(Block::ToolUse {
                    id: call.id,
                    name: call.name,
                    input,
                    input_error,
                });
            }
            let mut response = Response {
                blocks,
                stop_reason: "tool_use".into(),
                usage: Usage::default(),
            };
            assert_eq!(normalize_tool_ids(&mut response, &HashSet::new()), 2);
            assert!(validate_response(&response, &[]).is_ok());
            let inputs = response
                .blocks
                .iter()
                .map(|block| match block {
                    Block::ToolUse {
                        id,
                        input,
                        input_error,
                        ..
                    } => (id, input, input_error),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            assert_eq!(
                inputs
                    .iter()
                    .map(|item| item.0)
                    .collect::<HashSet<_>>()
                    .len(),
                4
            );
            assert_eq!(inputs[0].1["path"], "中文");
            assert!(inputs[1].2.is_some());
            assert!(inputs[2].2.is_none());
            assert_eq!(inputs[3].1["path"], "README.md");
        }
    }
}
