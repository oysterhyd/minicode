use super::{
    Provider,
    error::stream_error,
    wire::{
        CallBuffer, media_part, parse_tool_arguments, parse_usage, push_order, tool_argument_value,
    },
};
use crate::{
    attachments::Attachments,
    models::{Block, Message, Response, ToolSpec, Usage},
    tools::Sink,
    util,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};

impl Provider {
    pub(super) fn payload_openai_responses(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        attachments: &Attachments,
    ) -> Result<Value> {
        if self.seed.is_some() {
            bail!("seed is not supported by the OpenAI Responses API");
        }
        let mut input = vec![];
        let flush_assistant_text = |input: &mut Vec<Value>, pending: &mut String| {
            if !pending.is_empty() {
                input.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": std::mem::take(pending)}],
                }));
            }
        };
        let source_id = self.thinking_source();
        for message in messages {
            let media = message
                .content
                .iter()
                .filter(|b| matches!(b, Block::Media { .. }))
                .cloned()
                .collect::<Vec<_>>();
            attachments.check(&media, &self.modalities, &self.style)?;
            if message.role == "assistant" {
                let mut pending_text = String::new();
                for block in &message.content {
                    match block {
                        Block::Text { text } => pending_text.push_str(text),
                        Block::Thinking {
                            thinking,
                            signature,
                            source,
                        } => {
                            if source == &source_id
                                && let Some(encrypted) =
                                    signature.as_deref().filter(|s| !s.is_empty())
                            {
                                flush_assistant_text(&mut input, &mut pending_text);
                                let summary = if thinking.is_empty() {
                                    vec![]
                                } else {
                                    vec![json!({"type": "summary_text", "text": thinking})]
                                };
                                input.push(json!({
                                    "type": "reasoning",
                                    "summary": summary,
                                    "encrypted_content": encrypted,
                                }));
                            } else if !thinking.is_empty() {
                                pending_text
                                    .push_str(&format!("[Previous reasoning]\n{thinking}\n"));
                            }
                        }
                        Block::RedactedThinking { data, source } => {
                            if source == &source_id && !data.is_empty() {
                                flush_assistant_text(&mut input, &mut pending_text);
                                input.push(json!({
                                    "type": "reasoning",
                                    "summary": [],
                                    "encrypted_content": data,
                                }));
                            } else {
                                pending_text
                                    .push_str("[Previous redacted reasoning is unavailable.]\n");
                            }
                        }
                        Block::ToolUse {
                            id,
                            name,
                            input: arguments,
                            input_error,
                        } => {
                            flush_assistant_text(&mut input, &mut pending_text);
                            input.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": if input_error.is_some() {
                                    "{}".to_string()
                                } else {
                                    arguments.to_string()
                                },
                            }));
                        }
                        Block::Media { .. } | Block::ToolResult { .. } => {}
                    }
                }
                flush_assistant_text(&mut input, &mut pending_text);
            } else {
                let text = message.text();
                let mut results = 0usize;
                let mut content = vec![];
                for block in &message.content {
                    if let Block::ToolResult {
                        tool_use_id,
                        content: result,
                        images,
                        ..
                    } = block
                    {
                        results += 1;
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id,
                            "output": result,
                        }));
                        if !images.is_empty() {
                            attachments.check(images, &self.modalities, &self.style)?;
                            content.push(json!({
                                "type": "input_text",
                                "text": format!("Images read by tool call {tool_use_id}: {result}"),
                            }));
                            for image in images {
                                content.push(media_part(image, attachments, &self.style)?);
                            }
                        }
                    }
                }
                for block in &media {
                    content.push(media_part(block, attachments, &self.style)?);
                }
                if !text.is_empty() || (results == 0 && content.is_empty()) {
                    content.push(json!({"type": "input_text", "text": text}));
                }
                if !content.is_empty() {
                    input.push(json!({"type": "message", "role": "user", "content": content}));
                }
            }
        }
        let available = self
            .window
            .saturating_sub(
                self.estimated_prompt_tokens(system, messages, tools)
                    .saturating_add(1024),
            )
            .max(1);
        let mut body = json!({
            "model": self.model,
            "input": input,
            "stream": true,
            "store": false,
            "max_output_tokens": self.max_output.min(available),
        });
        if !system.is_empty() {
            body["instructions"] = json!(system);
        }
        if let Some(effort) = self.wire_reasoning_effort() {
            let mut reasoning = json!({"effort": effort});
            match self.parameter_compat.get("reasoningSummary") {
                Some(Value::Bool(false)) => {}
                Some(Value::String(summary)) if !summary.is_empty() => {
                    reasoning["summary"] = json!(summary);
                }
                _ => {
                    reasoning["summary"] = json!("auto");
                }
            }
            body["reasoning"] = reasoning;
            if self.parameter_compat["encryptedReasoning"] != false {
                body["include"] = json!(["reasoning.encrypted_content"]);
            }
        }
        if !tools.is_empty() {
            body["tools"] = json!(
                tools
                    .iter()
                    .map(|spec| json!({
                        "type": "function",
                        "name": spec.name,
                        "description": spec.description,
                        "parameters": spec.input_schema,
                    }))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        Ok(body)
    }
}

fn extract_responses_reasoning_summary(item: &Value) -> String {
    let join_texts = |items: Option<&Vec<Value>>, expected_type: Option<&str>| {
        items
            .into_iter()
            .flatten()
            .filter(|entry| expected_type.is_none_or(|kind| entry["type"] == kind))
            .filter_map(|entry| entry["text"].as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let summary = join_texts(item["summary"].as_array(), None);
    if !summary.is_empty() {
        return summary;
    }
    join_texts(item["content"].as_array(), Some("reasoning_text"))
}

fn extract_responses_output_text(item: &Value) -> Result<String> {
    let mut text = String::new();
    for part in item["content"].as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("refusal") => {
                if part["refusal"]
                    .as_str()
                    .or_else(|| part["text"].as_str())
                    .is_some_and(|s| !s.is_empty())
                {
                    bail!("provider rejected the response");
                }
            }
            Some("output_text") => {
                if let Some(piece) = part["text"].as_str() {
                    text.push_str(piece);
                }
            }
            _ => {}
        }
    }
    Ok(text)
}

pub(super) fn parse_openai_responses(v: &Value, source: &str) -> Result<Response> {
    if !v["error"].is_null() || v["status"] == "failed" {
        return Err(stream_error(v).into());
    }
    if v["incomplete_details"]["reason"] == "content_filter" {
        bail!("provider rejected the response: content_filter");
    }
    let truncated =
        v["status"] == "incomplete" && v["incomplete_details"]["reason"] == "max_output_tokens";
    let mut blocks = vec![];
    for item in v["output"].as_array().context("missing provider output")? {
        match item["type"].as_str() {
            Some("reasoning") => {
                let thinking = extract_responses_reasoning_summary(item);
                let encrypted = item["encrypted_content"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if !thinking.is_empty() {
                    blocks.push(Block::Thinking {
                        thinking,
                        signature: encrypted,
                        source: source.into(),
                    });
                } else if let Some(data) = encrypted {
                    blocks.push(Block::RedactedThinking {
                        data,
                        source: source.into(),
                    });
                }
            }
            Some("message") => {
                let text = extract_responses_output_text(item)?;
                if !text.is_empty() {
                    blocks.push(Block::text(text));
                }
            }
            Some("function_call") if !truncated => {
                let arguments = &item["arguments"];
                let (input, input_error) = match arguments.as_str() {
                    Some(raw) => parse_tool_arguments(raw),
                    None if arguments.is_null() => (json!({}), None),
                    None => tool_argument_value(arguments.clone()),
                };
                blocks.push(Block::ToolUse {
                    id: item["call_id"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .or_else(|| item["id"].as_str())
                        .unwrap_or_default()
                        .into(),
                    name: util::string(item, "name")?.into(),
                    input,
                    input_error,
                });
            }
            _ => {}
        }
    }
    let stop_reason = if truncated {
        "max_tokens"
    } else if blocks
        .iter()
        .any(|block| matches!(block, Block::ToolUse { .. }))
    {
        "tool_use"
    } else {
        "end_turn"
    };
    Ok(Response {
        blocks,
        stop_reason: stop_reason.into(),
        usage: parse_usage(&v["usage"], "openai_responses"),
    })
}

pub(super) struct ResponsesSseState<'a> {
    pub(super) source: &'a str,
    pub(super) started: Instant,
    pub(super) first: &'a mut Option<f64>,
    pub(super) last: &'a mut Option<f64>,
    pub(super) response_bytes: &'a mut usize,
    pub(super) order: &'a mut Vec<usize>,
    pub(super) thoughts: &'a mut BTreeMap<usize, Block>,
    pub(super) texts: &'a mut BTreeMap<usize, String>,
    pub(super) calls: &'a mut BTreeMap<usize, CallBuffer>,
    pub(super) reasoning: &'a mut String,
    pub(super) usage: &'a mut Usage,
    pub(super) stop: &'a mut Option<String>,
    pub(super) ended: &'a mut bool,
    pub(super) emit: &'a Sink,
}

impl ResponsesSseState<'_> {
    fn touch(&mut self) {
        let now = self.started.elapsed().as_secs_f64();
        self.first.get_or_insert(now);
        *self.last = Some(now);
    }
}

pub(super) fn apply_responses_sse_event(
    value: &Value,
    state: &mut ResponsesSseState<'_>,
) -> Result<()> {
    match value["type"].as_str() {
        Some("response.output_item.added") => {
            let index = value["output_index"]
                .as_u64()
                .context("invalid output index")? as usize;
            let item = &value["item"];
            match item["type"].as_str() {
                Some("message") => push_order(state.order, index),
                Some("reasoning") => {
                    push_order(state.order, index);
                    let encrypted = item["encrypted_content"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    if let Some(enc) = &encrypted {
                        *state.response_bytes += enc.len();
                    }
                    state
                        .thoughts
                        .entry(index)
                        .or_insert_with(|| Block::Thinking {
                            thinking: String::new(),
                            signature: encrypted,
                            source: state.source.into(),
                        });
                }
                Some("function_call") => {
                    push_order(state.order, index);
                    let id = item["call_id"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .or_else(|| item["id"].as_str().filter(|s| !s.is_empty()))
                        .unwrap_or_default()
                        .to_string();
                    let name = item["name"].as_str().unwrap_or_default().to_string();
                    let initial = item["arguments"].as_str().unwrap_or_default().to_string();
                    let seen = !initial.is_empty();
                    *state.response_bytes += id.len() + name.len() + initial.len();
                    state.calls.insert(
                        index,
                        CallBuffer {
                            id,
                            name,
                            arguments: initial,
                            arguments_seen: seen,
                            ..CallBuffer::default()
                        },
                    );
                }
                _ => {}
            }
        }
        Some("response.output_text.delta") => {
            let index = value["output_index"].as_u64().unwrap_or(0) as usize;
            push_order(state.order, index);
            if let Some(piece) = value["delta"].as_str().filter(|s| !s.is_empty()) {
                state.touch();
                *state.response_bytes += piece.len();
                state.texts.entry(index).or_default().push_str(piece);
                (state.emit)(json!({"event":"text_delta","text":piece}));
            }
        }
        Some("response.refusal.delta" | "response.refusal.done") => {
            if value["delta"]
                .as_str()
                .or_else(|| value["refusal"].as_str())
                .is_some_and(|s| !s.is_empty())
            {
                bail!("provider rejected the response");
            }
        }
        Some("response.reasoning_summary_part.added") => {
            let index = value["output_index"].as_u64().unwrap_or(0) as usize;
            if value["summary_index"].as_u64().unwrap_or(0) > 0
                && let Some(Block::Thinking { thinking, .. }) = state.thoughts.get_mut(&index)
                && !thinking.is_empty()
            {
                thinking.push('\n');
                state.reasoning.push('\n');
                *state.response_bytes += 1;
                (state.emit)(json!({"event":"reasoning_delta","text":"\n"}));
            }
        }
        Some("response.reasoning_summary_text.delta" | "response.reasoning_text.delta") => {
            let index = value["output_index"].as_u64().unwrap_or(0) as usize;
            push_order(state.order, index);
            if let Some(piece) = value["delta"].as_str().filter(|s| !s.is_empty()) {
                state.touch();
                *state.response_bytes += piece.len();
                state.reasoning.push_str(piece);
                let entry = state
                    .thoughts
                    .entry(index)
                    .or_insert_with(|| Block::Thinking {
                        thinking: String::new(),
                        signature: None,
                        source: state.source.into(),
                    });
                if let Block::Thinking { thinking, .. } = entry {
                    thinking.push_str(piece);
                }
                (state.emit)(json!({"event":"reasoning_delta","text":piece}));
            } else {
                state
                    .thoughts
                    .entry(index)
                    .or_insert_with(|| Block::Thinking {
                        thinking: String::new(),
                        signature: None,
                        source: state.source.into(),
                    });
            }
        }
        Some("response.function_call_arguments.delta") => {
            let index = value["output_index"]
                .as_u64()
                .context("invalid tool output index")? as usize;
            push_order(state.order, index);
            let piece = value["delta"].as_str().unwrap_or_default();
            state.touch();
            *state.response_bytes += piece.len();
            let call = state.calls.entry(index).or_default();
            call.arguments_seen = true;
            call.arguments.push_str(piece);
        }
        Some("response.function_call_arguments.done") => {
            let index = value["output_index"]
                .as_u64()
                .context("invalid tool output index")? as usize;
            push_order(state.order, index);
            let call = state.calls.entry(index).or_default();
            if call.name.is_empty()
                && let Some(name) = value["name"].as_str().filter(|s| !s.is_empty())
            {
                *state.response_bytes += name.len();
                call.name = name.into();
            }
            if !call.arguments_seen
                && let Some(raw) = value["arguments"].as_str()
            {
                call.arguments_seen = true;
                *state.response_bytes += raw.len();
                call.arguments = raw.into();
            }
        }
        Some("response.output_item.done") => {
            let index = value["output_index"]
                .as_u64()
                .context("invalid output index")? as usize;
            let item = &value["item"];
            match item["type"].as_str() {
                Some("reasoning") => {
                    push_order(state.order, index);
                    let summary = extract_responses_reasoning_summary(item);
                    let encrypted = item["encrypted_content"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    let entry = state
                        .thoughts
                        .entry(index)
                        .or_insert_with(|| Block::Thinking {
                            thinking: String::new(),
                            signature: None,
                            source: state.source.into(),
                        });
                    if let Block::Thinking {
                        thinking,
                        signature,
                        ..
                    } = entry
                    {
                        if thinking.is_empty() && !summary.is_empty() {
                            *state.response_bytes += summary.len();
                            *thinking = summary.clone();
                            state.reasoning.push_str(&summary);
                            (state.emit)(json!({"event":"reasoning_delta","text":summary}));
                        }
                        if signature.is_none()
                            && let Some(enc) = encrypted
                        {
                            *state.response_bytes += enc.len();
                            *signature = Some(enc);
                        }
                        if thinking.is_empty() {
                            if let Some(data) = signature.take() {
                                *entry = Block::RedactedThinking {
                                    data,
                                    source: state.source.into(),
                                };
                            } else {
                                state.thoughts.remove(&index);
                            }
                        }
                    }
                }
                Some("function_call") => {
                    push_order(state.order, index);
                    let call = state.calls.entry(index).or_default();
                    if let Some(cid) = item["call_id"].as_str().filter(|s| !s.is_empty()) {
                        if call.id != cid {
                            *state.response_bytes += cid.len();
                            call.id = cid.into();
                        }
                    } else if call.id.is_empty()
                        && let Some(id) = item["id"].as_str().filter(|s| !s.is_empty())
                    {
                        *state.response_bytes += id.len();
                        call.id = id.into();
                    }
                    if call.name.is_empty()
                        && let Some(name) = item["name"].as_str().filter(|s| !s.is_empty())
                    {
                        *state.response_bytes += name.len();
                        call.name = name.into();
                    }
                    if !call.arguments_seen
                        && let Some(arguments) = item.get("arguments").filter(|a| !a.is_null())
                    {
                        if let Some(raw) = arguments.as_str() {
                            call.arguments_seen = true;
                            *state.response_bytes += raw.len();
                            call.arguments = raw.into();
                        } else {
                            *state.response_bytes += arguments.to_string().len();
                            call.argument_value = Some(arguments.clone());
                        }
                    }
                }
                Some("message") => {
                    push_order(state.order, index);
                    let text = extract_responses_output_text(item)?;
                    if !text.is_empty() && state.texts.get(&index).is_none_or(String::is_empty) {
                        *state.response_bytes += text.len();
                        state.texts.insert(index, text.clone());
                        (state.emit)(json!({"event":"text_delta","text":text}));
                    }
                }
                _ => {}
            }
        }
        Some("response.completed" | "response.incomplete") => {
            *state.ended = true;
            let resp = &value["response"];
            if !resp["error"].is_null() || resp["status"] == "failed" {
                return Err(stream_error(resp).into());
            }
            if resp["usage"].is_object() {
                *state.usage = parse_usage(&resp["usage"], "openai_responses");
                (state.emit)(json!({"event":"usage_delta","usage":*state.usage}));
            }
            if value["type"] == "response.incomplete" || resp["status"] == "incomplete" {
                let reason = resp["incomplete_details"]["reason"]
                    .as_str()
                    .unwrap_or_default();
                if reason == "content_filter" {
                    bail!("provider rejected the response: content_filter");
                }
                *state.stop = Some(
                    if reason == "max_output_tokens" {
                        "max_tokens"
                    } else {
                        "end_turn"
                    }
                    .into(),
                );
            } else {
                if state.order.is_empty()
                    && resp["output"].as_array().is_some_and(|a| !a.is_empty())
                {
                    let parsed = parse_openai_responses(resp, state.source)?;
                    for (index, block) in parsed.blocks.into_iter().enumerate() {
                        push_order(state.order, index);
                        match block {
                            Block::Text { text } => {
                                *state.response_bytes += text.len();
                                (state.emit)(json!({"event":"text_delta","text":text}));
                                state.texts.insert(index, text);
                            }
                            Block::Thinking { ref thinking, .. } => {
                                *state.response_bytes += thinking.len();
                                if !thinking.is_empty() {
                                    state.reasoning.push_str(thinking);
                                    (state.emit)(
                                        json!({"event":"reasoning_delta","text":thinking}),
                                    );
                                }
                                state.thoughts.insert(index, block);
                            }
                            Block::RedactedThinking { ref data, .. } => {
                                *state.response_bytes += data.len();
                                state.thoughts.insert(index, block);
                            }
                            Block::ToolUse {
                                id,
                                name,
                                input,
                                input_error,
                            } => {
                                *state.response_bytes += id.len() + name.len();
                                let mut buffer = CallBuffer {
                                    id,
                                    name,
                                    ..CallBuffer::default()
                                };
                                if input_error.is_some()
                                    && let Some(raw) = input.as_str()
                                {
                                    buffer.arguments_seen = true;
                                    buffer.arguments = raw.to_string();
                                } else {
                                    buffer.argument_value = Some(input);
                                }
                                state.calls.insert(index, buffer);
                            }
                            _ => {}
                        }
                    }
                }
                *state.stop = Some(
                    if !state.calls.is_empty() {
                        "tool_use"
                    } else {
                        "end_turn"
                    }
                    .into(),
                );
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::Configuration;
    #[test]
    fn openai_responses_payload_supports_reasoning_with_tools_and_encrypted_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = Configuration {
            path: dir.path().join("config.json"),
        };
        config
            .write(&json!({
                "services": [{
                    "id": "openai-resp",
                    "name": "OpenAI Responses",
                    "enabled": true,
                    "apiStyle": "openai_responses",
                    "baseUrl": "https://example.test/v1",
                    "apiKey": "test-key",
                    "models": [{
                        "modelId": "gpt-5.4-pro",
                        "contextWindow": 200000,
                        "maxOutputTokens": 16000,
                        "supportsEffort": true
                    }]
                }],
                "agents": []
            }))
            .unwrap();
        let provider = Provider::new("openai-resp::gpt-5.4-pro", "high", &config).unwrap();
        let attachments = Attachments {
            root: dir.path().join("attachments"),
        };
        let specs = [ToolSpec {
            name: "read".into(),
            description: "Read a file".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            requires_approval: false,
            read_only_hint: true,
        }];
        let messages = [
            Message::user("inspect foo.txt"),
            Message {
                role: "assistant".into(),
                content: vec![
                    Block::Thinking {
                        thinking: "check foo.txt".into(),
                        signature: Some("enc_blob_1".into()),
                        source: provider.thinking_source(),
                    },
                    Block::RedactedThinking {
                        data: "enc_blob_2".into(),
                        source: provider.thinking_source(),
                    },
                    Block::ToolUse {
                        id: "call_1".into(),
                        name: "read".into(),
                        input: json!({"path": "foo.txt"}),
                        input_error: None,
                    },
                ],
            },
            Message {
                role: "user".into(),
                content: vec![Block::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "ok".into(),
                    is_error: false,
                    images: vec![],
                }],
            },
        ];
        let payload = provider
            .payload("system prompt", &messages, &specs, &attachments)
            .unwrap();
        assert_eq!(payload["model"], "gpt-5.4-pro");
        assert_eq!(payload["instructions"], "system prompt");
        assert_eq!(payload["store"], false);
        assert_eq!(payload["reasoning"]["effort"], "high");
        assert_eq!(payload["reasoning"]["summary"], "auto");
        assert_eq!(payload["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(
            payload["tools"],
            json!([{
                "type": "function",
                "name": "read",
                "description": "Read a file",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
            }])
        );
        let input = payload["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["encrypted_content"], "enc_blob_1");
        assert_eq!(input[1]["summary"][0]["text"], "check foo.txt");
        assert_eq!(input[2]["type"], "reasoning");
        assert_eq!(input[2]["encrypted_content"], "enc_blob_2");
        assert_eq!(input[2]["summary"], json!([]));
        assert_eq!(input[3]["type"], "function_call");
        assert_eq!(input[3]["call_id"], "call_1");
        assert_eq!(input[4]["type"], "function_call_output");
        assert_eq!(input[4]["call_id"], "call_1");
        assert_eq!(input[4]["output"], "ok");
    }

    #[test]
    fn openai_responses_cross_provider_thinking_degrades_safely() {
        let dir = tempfile::tempdir().unwrap();
        let config = Configuration {
            path: dir.path().join("config.json"),
        };
        config
            .write(&json!({
                "services": [{
                    "id": "openai-resp",
                    "name": "OpenAI Responses",
                    "enabled": true,
                    "apiStyle": "openai_responses",
                    "baseUrl": "https://example.test/v1",
                    "apiKey": "test-key",
                    "models": [{
                        "modelId": "gpt-5.4",
                        "contextWindow": 200000,
                        "maxOutputTokens": 16000,
                        "supportsEffort": true
                    }]
                }],
                "agents": []
            }))
            .unwrap();
        let provider = Provider::new("openai-resp::gpt-5.4", "low", &config).unwrap();
        let attachments = Attachments {
            root: dir.path().join("attachments"),
        };
        let messages = [Message {
            role: "assistant".into(),
            content: vec![
                Block::Thinking {
                    thinking: "claude thought".into(),
                    signature: Some("anthropic_sig".into()),
                    source: "anthropic:claude-sonnet-4-5".into(),
                },
                Block::RedactedThinking {
                    data: "anthropic_redacted".into(),
                    source: "anthropic:claude-sonnet-4-5".into(),
                },
                Block::text("answer"),
            ],
        }];
        let payload = provider.payload("", &messages, &[], &attachments).unwrap();
        let input = payload["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "assistant");
        let text = input[0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("[Previous reasoning]\nclaude thought"));
        assert!(text.contains("[Previous redacted reasoning is unavailable.]"));
        assert!(text.contains("answer"));
        assert!(!payload.to_string().contains("anthropic_sig"));
    }

    #[test]
    fn openai_responses_parse_json_and_usage() {
        let raw = json!({
            "status": "completed",
            "output": [
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "thinking step"}],
                    "encrypted_content": "enc_123"
                },
                {
                    "type": "reasoning",
                    "summary": [],
                    "encrypted_content": "enc_redacted"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "done"}]
                },
                {
                    "type": "function_call",
                    "id": "fc_item_1",
                    "call_id": "call_1",
                    "name": "read",
                    "arguments": "{\"path\":\"a.txt\"}"
                }
            ],
            "usage": {
                "input_tokens": 120,
                "output_tokens": 45,
                "input_tokens_details": {"cached_tokens": 30}
            }
        });
        let parsed = parse_openai_responses(&raw, "openai_responses:s::m").unwrap();
        assert_eq!(parsed.stop_reason, "tool_use");
        assert_eq!(parsed.usage.input_tokens, 120);
        assert_eq!(parsed.usage.output_tokens, 45);
        assert_eq!(parsed.usage.cache_read_tokens, 30);
        assert!(parsed.usage.available);
        assert_eq!(parsed.blocks.len(), 4);
        assert!(matches!(
            &parsed.blocks[0],
            Block::Thinking { thinking, signature: Some(sig), .. } if thinking == "thinking step" && sig == "enc_123"
        ));
        assert!(matches!(
            &parsed.blocks[1],
            Block::RedactedThinking { data, .. } if data == "enc_redacted"
        ));
        assert!(matches!(&parsed.blocks[2], Block::Text { text } if text == "done"));
        assert!(matches!(
            &parsed.blocks[3],
            Block::ToolUse { id, name, input, input_error: None } if id == "call_1" && name == "read" && input["path"] == "a.txt"
        ));
    }

    #[test]
    fn openai_responses_sse_streaming_with_reasoning_tools_and_incomplete() {
        let emit: Sink = std::sync::Arc::new(|_| {});
        let events = [
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "reasoning", "summary": []}}),
            json!({"type": "response.reasoning_summary_text.delta", "output_index": 0, "delta": "part 1"}),
            json!({"type": "response.reasoning_summary_part.added", "output_index": 0, "summary_index": 1}),
            json!({"type": "response.reasoning_summary_text.delta", "output_index": 0, "delta": "part 2"}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "reasoning", "summary": [{"type": "summary_text", "text": "part 1"}, {"type": "summary_text", "text": "part 2"}], "encrypted_content": "enc_sse"}}),
            json!({"type": "response.output_item.added", "output_index": 1, "item": {"type": "message", "role": "assistant"}}),
            json!({"type": "response.output_text.delta", "output_index": 1, "delta": "hi "}),
            json!({"type": "response.output_text.delta", "output_index": 1, "delta": "there"}),
            json!({"type": "response.output_item.added", "output_index": 2, "item": {"type": "function_call", "id": "fc_2", "call_id": "call_2", "name": "ls"}}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "{\"path\":"}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "\".\"}"}),
            json!({"type": "response.output_item.done", "output_index": 2, "item": {"type": "function_call", "call_id": "call_2", "name": "ls", "arguments": "{\"path\":\".\"}"}}),
            json!({"type": "response.completed", "response": {"status": "completed", "usage": {"input_tokens": 200, "output_tokens": 50, "input_tokens_details": {"cached_tokens": 80}}}}),
        ];
        let mut order = vec![];
        let mut thoughts = BTreeMap::new();
        let mut texts = BTreeMap::new();
        let mut calls = BTreeMap::new();
        let mut reasoning = String::new();
        let mut usage = Usage::default();
        let mut stop = None;
        let mut ended = false;
        let mut response_bytes = 0usize;
        let mut first = None;
        let mut last = None;
        for event in &events {
            let mut state = ResponsesSseState {
                source: "openai_responses:s::m",
                started: Instant::now(),
                first: &mut first,
                last: &mut last,
                response_bytes: &mut response_bytes,
                order: &mut order,
                thoughts: &mut thoughts,
                texts: &mut texts,
                calls: &mut calls,
                reasoning: &mut reasoning,
                usage: &mut usage,
                stop: &mut stop,
                ended: &mut ended,
                emit: &emit,
            };
            apply_responses_sse_event(event, &mut state).unwrap();
        }
        assert!(ended);
        assert_eq!(stop.as_deref(), Some("tool_use"));
        assert_eq!(reasoning, "part 1\npart 2");
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.cache_read_tokens, 80);
        assert!(matches!(
            thoughts.get(&0),
            Some(Block::Thinking { thinking, signature: Some(sig), .. }) if thinking == "part 1\npart 2" && sig == "enc_sse"
        ));
        assert_eq!(texts.get(&1).map(String::as_str), Some("hi there"));
        let (args, err) = calls.get(&2).unwrap().parsed_arguments();
        assert_eq!(args, json!({"path": "."}));
        assert!(err.is_none());

        // Incomplete max_output_tokens case
        let mut inc_stop = None;
        let mut inc_ended = false;
        let mut inc_state = ResponsesSseState {
            source: "openai_responses:s::m",
            started: Instant::now(),
            first: &mut None,
            last: &mut None,
            response_bytes: &mut 0,
            order: &mut vec![],
            thoughts: &mut BTreeMap::new(),
            texts: &mut BTreeMap::new(),
            calls: &mut BTreeMap::new(),
            reasoning: &mut String::new(),
            usage: &mut Usage::default(),
            stop: &mut inc_stop,
            ended: &mut inc_ended,
            emit: &emit,
        };
        apply_responses_sse_event(
            &json!({"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}}),
            &mut inc_state,
        )
        .unwrap();
        assert!(inc_ended);
        assert_eq!(inc_stop.as_deref(), Some("max_tokens"));
    }
}
