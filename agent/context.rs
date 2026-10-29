use crate::{
    models::{Block, Message, ToolSpec},
    store::Store,
    util,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;

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
        let mut refs = vec![];
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
                    Block::ToolUse { input, .. } => {
                        if let Some(path) = input["path"].as_str() {
                            refs.push(path.to_string());
                        }
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
        refs.sort();
        refs.dedup();
        refs.truncate(40);
        let summary = |artifact: &str| {
            if let Some(text) = generated {
                return Message::user(format!(
                    "此前历史已归档，完整消息见 [artifact:{artifact}]。\n<structured-summary>\n{text}\n</structured-summary>"
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
    if estimate(system, messages, specs) > trigger || (force && archived == 0) {
        let total = messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, Block::ToolResult { .. }))
            .count();
        let mut index = 0;
        for m in messages.iter_mut() {
            for b in &mut m.content {
                if let Block::ToolResult { content, .. } = b {
                    index += 1;
                    if index + 2 <= total
                        && content.chars().count() > 240
                        && !content.contains("[已压缩")
                    {
                        let artifact = store.spill(id, "tool_output", content)?;
                        *content = format!(
                            "{}\n...[已压缩；完整内容见执行报告 [artifact:{artifact}]]",
                            content.chars().take(200).collect::<String>()
                        );
                        shrunk += 1;
                    }
                }
            }
        }
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
