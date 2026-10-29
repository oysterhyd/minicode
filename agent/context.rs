use crate::{
    models::{Block, Message, ToolSpec},
    store::Store,
    util,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub fn tokens(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        (text.chars().map(char_weight).sum::<u64>() / 3).max(1)
    }
}
fn char_weight(ch: char) -> u64 {
    // CJK and other non-ASCII text need a conservative one-token-per-char
    // fallback until the provider reports measured usage.
    if ch.is_ascii() { 1 } else { 3 }
}
pub fn block_tokens(b: &Block) -> u64 {
    match b {
        Block::Text { text } => tokens(text),
        Block::Thinking { thinking, .. } => tokens(thinking),
        Block::RedactedThinking { data, .. } => tokens(data),
        Block::Media { kind, size, .. } => {
            if kind == "image" {
                1536
            } else {
                1024 + size / 256
            }
        }
        Block::ToolUse { name, input, .. } => tokens(name) + json_tokens(input),
        Block::ToolResult {
            content, images, ..
        } => tokens(content) + images.iter().map(block_tokens).sum::<u64>(),
    }
}
pub fn estimate(system: &str, messages: &[Message], specs: &[ToolSpec]) -> u64 {
    tokens(system)
        + messages
            .iter()
            .flat_map(|m| &m.content)
            .map(block_tokens)
            .sum::<u64>()
        + specs
            .iter()
            .map(|s| {
                // Object delimiters, four field names and Python-style separators.
                let chars = 68
                    + json_string_chars(&s.name)
                    + json_string_chars(&s.description)
                    + json_chars(&s.input_schema)
                    + if s.requires_approval { 4 } else { 5 };
                (chars / 3).max(1)
            })
            .sum::<u64>()
}
fn json_string_chars(text: &str) -> u64 {
    2 + text
        .chars()
        .map(|ch| match ch {
            '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
            '\0'..='\u{1f}' => 6,
            _ => char_weight(ch),
        })
        .sum::<u64>()
}
fn json_chars(value: &Value) -> u64 {
    match value {
        Value::Array(a) => {
            2 + a.iter().map(json_chars).sum::<u64>() + a.len().saturating_sub(1) as u64 * 2
        }
        Value::Object(o) => {
            2 + o
                .iter()
                .map(|(k, v)| json_string_chars(k) + 2 + json_chars(v))
                .sum::<u64>()
                + o.len().saturating_sub(1) as u64 * 2
        }
        Value::String(s) => json_string_chars(s),
        Value::Null | Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(n) => n.to_string().len() as u64,
    }
}
fn json_tokens(value: &Value) -> u64 {
    (json_chars(value) / 3).max(1)
}
pub fn breakdown(system: &str, messages: &[Message], specs: &[ToolSpec]) -> Value {
    json!({"system":tokens(system),"tools":estimate("",&[],specs),"messages":estimate("",messages,&[])})
}
pub fn paired_prefix(messages: &[Message], boundaries: &[usize]) -> Option<usize> {
    // Global and prefix counts detect cross-boundary, duplicate and orphan ids.
    // Each block is visited once; candidate prefixes are checked in O(1).
    let mut totals = HashMap::<&str, (usize, usize)>::new();
    for b in messages.iter().flat_map(|m| &m.content) {
        match b {
            Block::ToolUse { id, .. } => totals.entry(id).or_default().0 += 1,
            Block::ToolResult { tool_use_id, .. } => totals.entry(tool_use_id).or_default().1 += 1,
            _ => {}
        }
    }
    let mut inside = HashMap::<&str, (usize, usize)>::new();
    let mut invalid = std::collections::HashSet::new();
    let mut last = None;
    let mut cursor = boundaries.iter().peekable();
    for (index, message) in messages.iter().enumerate() {
        for b in &message.content {
            let (id, is_use) = match b {
                Block::ToolUse { id, .. } => (id.as_str(), true),
                Block::ToolResult { tool_use_id, .. } => (tool_use_id.as_str(), false),
                _ => continue,
            };
            let counts = inside.entry(id).or_default();
            if is_use {
                counts.0 += 1
            } else {
                counts.1 += 1
            }
            let total = totals[id];
            if *counts == (1, 1) && total == (1, 1) {
                invalid.remove(id);
            } else {
                invalid.insert(id);
            }
        }
        if cursor.peek().is_some_and(|&&b| b == index + 1) {
            if invalid.is_empty() {
                last = Some(index + 1);
            }
            cursor.next();
        }
    }
    last
}

fn text_refs(text: &str) -> Vec<String> {
    static PATHS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let paths = PATHS.get_or_init(|| regex::Regex::new(r"(?:[A-Za-z]:[\\/]|[\\/])?(?:[.\p{L}\p{N}_@+-]+[\\/])*[.\p{L}\p{N}_@+-]+\.[A-Za-z][A-Za-z0-9_+-]{0,15}").expect("file reference regex"));
    paths
        .find_iter(text)
        .take(40)
        .filter(|m| m.as_str().len() <= 1024)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn input_refs(name: &str, input: &Value) -> Vec<String> {
    let mut refs = ["path", "file_path"]
        .iter()
        .filter_map(|key| input[*key].as_str())
        .filter(|path| !path.is_empty() && *path != "." && path.len() <= 1024)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if matches!(name, "bash" | "powershell" | "grep")
        && let Some(command) = input["command"].as_str()
    {
        refs.extend(text_refs(command));
    }
    refs.sort();
    refs.dedup();
    refs
}

struct CallInfo {
    name: String,
    refs: Vec<String>,
    position: usize,
}

fn call_info(messages: &[Message]) -> HashMap<String, CallInfo> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(index, m)| {
            m.content.iter().filter_map(move |b| match b {
                Block::ToolUse {
                    id, name, input, ..
                } => Some((
                    id.clone(),
                    CallInfo {
                        name: name.clone(),
                        refs: input_refs(name, input),
                        position: index,
                    },
                )),
                _ => None,
            })
        })
        .collect()
}

fn relevant_reads(messages: &[Message], calls: &HashMap<String, CallInfo>) -> HashSet<String> {
    let limit = std::env::var("MINICODE_CONTEXT_RELEVANT_READS")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(4)
        .min(16);
    let mut prompts = messages
        .iter()
        .filter(|m| {
            m.role == "user"
                && !m
                    .content
                    .iter()
                    .any(|b| matches!(b, Block::ToolResult { .. }))
        })
        .map(Message::text)
        .filter(|text| {
            !text.starts_with("此前历史已归档") && !text.starts_with("<system-reminder>")
        });
    let original = prompts.next().unwrap_or_default();
    let latest = prompts.next_back().unwrap_or_default();
    let prompt = format!("{original}\n{latest}").replace('\\', "/");
    let successful = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            Block::ToolResult {
                tool_use_id,
                is_error: false,
                ..
            } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut reads = calls
        .iter()
        .filter(|(id, call)| call.name == "read" && successful.contains(id.as_str()))
        .collect::<Vec<_>>();
    reads.sort_by(|(a_id, a), (b_id, b)| b.position.cmp(&a.position).then_with(|| a_id.cmp(b_id)));
    let mut seen = HashSet::new();
    reads
        .into_iter()
        .filter(|(_, call)| {
            call.refs
                .iter()
                .any(|path| seen.insert(path.clone()) && prompt.contains(&path.replace('\\', "/")))
        })
        .take(limit)
        .map(|(id, _)| id.clone())
        .collect()
}

fn history_refs(messages: &[Message]) -> Vec<String> {
    let calls = call_info(messages);
    let mut refs = calls
        .values()
        .flat_map(|call| call.refs.clone())
        .collect::<Vec<_>>();
    for message in messages {
        for block in &message.content {
            match block {
                Block::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if calls.get(tool_use_id).is_some_and(|call| {
                    matches!(call.name.as_str(), "bash" | "powershell" | "grep")
                }) =>
                {
                    refs.extend(text_refs(content))
                }
                Block::Text { text } if text.starts_with("此前历史已归档") => {
                    refs.extend(text_refs(text))
                }
                _ => {}
            }
        }
    }
    refs.sort();
    refs.dedup();
    refs.truncate(40);
    refs
}

fn shrink_results(
    store: &Store,
    id: &str,
    messages: &mut [Message],
    mut current: u64,
    target: u64,
) -> Result<usize> {
    let calls = call_info(messages);
    let relevant = relevant_reads(messages, &calls);
    let total = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, Block::ToolResult { .. }))
        .count();
    let mut sequence = 0;
    let mut candidates = vec![];
    for (mi, message) in messages.iter().enumerate() {
        for (bi, block) in message.content.iter().enumerate() {
            if let Block::ToolResult {
                tool_use_id,
                is_error,
                ..
            } = block
            {
                sequence += 1;
                if sequence + 2 <= total {
                    let priority = if relevant.contains(tool_use_id) {
                        3
                    } else if *is_error {
                        2
                    } else if calls
                        .get(tool_use_id)
                        .is_some_and(|call| call.name == "read")
                    {
                        1
                    } else {
                        0
                    };
                    candidates.push((priority, sequence, mi, bi));
                }
            }
        }
    }
    candidates.sort_unstable();
    let mut changed = HashSet::new();
    for keep in [2000, 200] {
        for &(_, _, mi, bi) in &candidates {
            if current <= target {
                return Ok(changed.len());
            }
            let Block::ToolResult {
                tool_use_id,
                content,
                ..
            } = &mut messages[mi].content[bi]
            else {
                unreachable!()
            };
            let old = tokens(content);
            let mut refs = calls
                .get(tool_use_id)
                .map(|call| call.refs.clone())
                .unwrap_or_default();
            if calls
                .get(tool_use_id)
                .is_some_and(|call| matches!(call.name.as_str(), "bash" | "powershell" | "grep"))
            {
                refs.extend(text_refs(content));
            }
            refs.sort();
            refs.dedup();
            refs.truncate(8);
            let body = content.split("\n...[已压缩").next().unwrap_or(content);
            let preview = body.chars().take(keep).collect::<String>();
            let render = |artifact: &str| {
                format!(
                    "{preview}\n...[已压缩；文件引用：{}；完整内容见 [artifact:{artifact}]]",
                    refs.join(", ")
                )
            };
            if tokens(&render("tool_output_000000000000")) >= old {
                continue;
            }
            // Only our own truncation markers identify an existing full archive.
            let existing = if content.contains("[已压缩") || content.contains("output truncated")
            {
                content
                    .rsplit_once("[artifact:")
                    .and_then(|(_, rest)| rest.split_once(']'))
                    .map(|(artifact, _)| artifact)
                    .filter(|artifact| util::valid_id(artifact))
                    .map(str::to_string)
            } else {
                None
            };
            let artifact = match existing {
                Some(artifact) => artifact,
                None => store.spill(id, "tool_output", content)?,
            };
            *content = render(&artifact);
            current = current.saturating_sub(old).saturating_add(tokens(content));
            changed.insert((mi, bi));
        }
    }
    Ok(changed.len())
}
pub fn compact(
    store: &Store,
    id: &str,
    system: &str,
    specs: &[ToolSpec],
    messages: &mut Vec<Message>,
    window: u64,
    force: bool,
) -> Result<Option<Value>> {
    compact_summarized(store, id, system, specs, messages, window, (force, None))
}

pub fn archive_range(
    messages: &[Message],
    force: bool,
    window: u64,
) -> Option<std::ops::Range<usize>> {
    let units = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.role == "user"
                && !m
                    .content
                    .iter()
                    .any(|b| matches!(b, Block::ToolResult { .. }))
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let head = units.first()? + 1;
    let keep = 20_000.min(window / 3).max(1);
    let keep = if force {
        keep.min((estimate("", messages, &[]) / 2).max(1))
    } else {
        keep
    };
    let mut tail = 0;
    let mut keep_start = messages.len();
    for (i, message) in messages.iter().enumerate().rev() {
        tail += estimate("", std::slice::from_ref(message), &[]);
        keep_start = i;
        if tail >= keep {
            break;
        }
    }
    // Manual compaction keeps the latest request verbatim whenever possible.
    if force && let Some(&latest) = units.last().filter(|&&i| i > head) {
        keep_start = latest;
    }
    let calls = call_info(messages);
    for id in relevant_reads(messages, &calls) {
        if let Some(call) = calls.get(&id) {
            keep_start = keep_start.min(call.position);
        }
    }
    let candidates = (head + 1..=keep_start).collect::<Vec<_>>();
    let end = paired_prefix(messages, &candidates)?;
    Some(head..end)
}

pub fn compact_summarized(
    store: &Store,
    id: &str,
    system: &str,
    specs: &[ToolSpec],
    messages: &mut Vec<Message>,
    window: u64,
    mode: (bool, Option<&str>),
) -> Result<Option<Value>> {
    let (force, mut generated) = mode;
    let before = estimate(system, messages, specs);
    let trigger = window.saturating_sub((window / 10).min(16_384));
    if !force && before <= trigger {
        return Ok(None);
    }
    let mut archived = 0;
    let mut shrunk = 0;
    let mut split_turn = false;
    if let Some(range) = archive_range(messages, force, window) {
        let head = range.start;
        let end = range.end;
        let archive = &messages[head..end];
        if generated
            .is_some_and(|text| tokens(text).saturating_add(100) >= estimate("", archive, &[]))
        {
            generated = None;
        }
        let mut goals = vec![];
        let refs = history_refs(archive);
        let mut findings = vec![];
        let previous = archive.iter().find_map(|m| {
            let text = m.text();
            text.split_once("<structured-summary>\n")
                .and_then(|(_, summary)| {
                    summary
                        .split_once("\n</structured-summary>")
                        .map(|(summary, _)| {
                            format!("<structured-summary>\n{summary}\n</structured-summary>")
                        })
                })
        });
        for m in archive {
            for b in &m.content {
                match b {
                    Block::Text { text } if m.role == "user" && goals.len() < 12 => {
                        goals.push(text.chars().take(240).collect::<String>())
                    }
                    Block::ToolResult {
                        content,
                        is_error: true,
                        ..
                    } if findings.len() < 12 => {
                        findings.push(content.chars().take(240).collect::<String>())
                    }
                    _ => {}
                }
            }
        }
        let summary = |artifact: &str| {
            if let Some(text) = generated {
                return Message::user(format!(
                    "此前历史已归档，完整消息见 [artifact:{artifact}]。\n<structured-summary>\n{text}\n</structured-summary>\n文件引用：{}",
                    refs.join(", ")
                ));
            }
            Message::user(format!(
                "此前历史已归档，完整消息见 [artifact:{artifact}]。\n{}\n目标：\n{}\n关键文件：{}\n失败与待解决问题：\n{}",
                previous.as_deref().unwrap_or_default(),
                goals.join("\n"),
                refs.join(", "),
                findings.join("\n")
            ))
        };
        if estimate(
            "",
            std::slice::from_ref(&summary("history_000000000000")),
            &[],
        ) < estimate("", archive, &[])
        {
            let serialized = serde_json::to_string(archive)?;
            let artifact = store.spill(id, "history", &serialized)?;
            archived = archive
                .iter()
                .filter(|m| {
                    m.role == "user"
                        && !m
                            .content
                            .iter()
                            .any(|b| matches!(b, Block::ToolResult { .. }))
                })
                .count()
                .max(1);
            split_turn = messages.get(end).is_some_and(|m| {
                m.role != "user"
                    || m.content
                        .iter()
                        .any(|b| matches!(b, Block::ToolResult { .. }))
            });
            messages.splice(head..end, [summary(&artifact)]);
        }
    }
    let current = estimate(system, messages, specs);
    if current > trigger || (force && archived == 0) {
        let target = if force {
            trigger.min(current.saturating_mul(3) / 4)
        } else {
            trigger
        };
        shrunk = shrink_results(store, id, messages, current, target)?;
    }
    let offloaded = if force && archived == 0 && shrunk == 0 {
        offload_large_inner(store, id, messages, true)?
    } else {
        0
    };
    if archived == 0 && shrunk == 0 && offloaded == 0 {
        return Ok(None);
    }
    let after = estimate(system, messages, specs);
    store.replace_messages(id, messages)?;
    Ok(Some(
        json!({"tokens_before":before,"tokens_after":after,"archived_units":archived,"shrunk_results":shrunk,"summarized_units":if generated.is_some(){archived}else{0},"split_turn":split_turn,"offloaded_blocks":offloaded}),
    ))
}
pub fn offload_large(store: &Store, id: &str, messages: &mut [Message]) -> Result<usize> {
    offload_large_inner(store, id, messages, false)
}
fn offload_large_inner(
    store: &Store,
    id: &str,
    messages: &mut [Message],
    force: bool,
) -> Result<usize> {
    let mut count = 0;
    let threshold = if force { 1024 } else { 16_000 };
    let preview = if force { 400 } else { 4000 };
    let completed = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            Block::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    let last = messages.len().saturating_sub(1);
    for (index, message) in messages.iter_mut().enumerate() {
        for block in &mut message.content {
            match block {
                Block::ToolUse {
                    id: call_id, input, ..
                } if completed.contains(call_id)
                    && input.get("_artifact").is_none()
                    && json_chars(input) > 16_000 =>
                {
                    let serialized = serde_json::to_string(input)?;
                    let artifact = store.spill(id, "tool_input", &serialized)?;
                    let mut replay = json!({"_artifact":artifact, "_preview":format!("{} ...[artifact:{artifact}]", util::prefix(&serialized, 1000))});
                    for key in ["path", "file_path"] {
                        if let Some(value) = input.get(key) {
                            replay[key] = value.clone();
                        }
                    }
                    *input = replay;
                    count += 1;
                }
                Block::Text { text }
                    if message.role == "assistant"
                        && (index < last || force)
                        && text.len() > threshold
                        && !text.contains("[artifact:") =>
                {
                    let artifact = store.spill(id, "assistant_text", text)?;
                    *text = format!(
                        "{}\n...[已压缩；完整内容见 [artifact:{artifact}]]",
                        util::prefix(text, preview)
                    );
                    count += 1;
                }
                Block::ToolResult { content, .. }
                    if content.len() > threshold && !content.contains("[artifact:") =>
                {
                    let artifact = store.spill(id, "tool_output", content)?;
                    *content = format!(
                        "{}\n...[output truncated; 完整输出见 [artifact:{artifact}]]",
                        util::prefix(content, preview)
                    );
                    count += 1;
                }
                _ => {}
            }
        }
    }
    Ok(count)
}
