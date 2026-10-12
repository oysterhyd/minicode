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
        (text.chars().count() as u64 / 3).max(1)
    }
}
pub fn block_tokens(b: &Block) -> u64 {
    match b {
        Block::Text { text } => tokens(text),
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
            _ => 1,
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
    let before = estimate(system, messages, specs);
    let trigger = window.saturating_sub((window / 10).min(16_384));
    if !force && before <= trigger {
        return Ok(None);
    }
    let mut archived = 0;
    let mut shrunk = 0;
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
    let mut keep_start = messages.len();
    let mut kept_tokens = 0;
    let mut kept_units = 0;
    for &start in units.iter().rev() {
        kept_tokens += estimate("", &messages[start..keep_start], &[]);
        kept_units += 1;
        keep_start = start;
        if kept_tokens >= 20_000 && kept_units >= 1 {
            break;
        }
    }
    let candidates = units
        .iter()
        .copied()
        .skip(2)
        .filter(|b| *b <= keep_start)
        .collect::<Vec<_>>();
    if let Some(end) = paired_prefix(messages, &candidates) {
        let head = units.first().copied().unwrap_or(0);
        let archive = &messages[head..end];
        let serialized = serde_json::to_string(archive)?;
        let artifact = store.spill(id, "history", &serialized)?;
        let mut goals = vec![];
        let mut refs = vec![];
        let mut findings = vec![];
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
        let summary = Message::user(format!(
            "此前历史已归档，完整消息见 [artifact:{artifact}]。\n目标：\n{}\n关键文件：{}\n失败与待解决问题：\n{}",
            goals.join("\n"),
            refs.join(", "),
            findings.join("\n")
        ));
        archived = units.iter().filter(|&&b| b >= head && b < end).count();
        messages.splice(head..end, [summary]);
    }
    if estimate(system, messages, specs) > trigger {
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
    if archived == 0 && shrunk == 0 {
        return Ok(None);
    }
    let after = estimate(system, messages, specs);
    store.replace_messages(id, messages)?;
    Ok(Some(
        json!({"tokens_before":before,"tokens_after":after,"archived_units":archived,"shrunk_results":shrunk,"summarized_units":0}),
    ))
}
pub fn offload_large(store: &Store, id: &str, messages: &mut [Message]) -> Result<usize> {
    let mut count = 0;
    for message in messages {
        for block in &mut message.content {
            match block {
                Block::ToolResult { content, .. }
                    if content.len() > 16_000 && !content.contains("[artifact:") =>
                {
                    let artifact = store.spill(id, "tool_output", content)?;
                    *content = format!(
                        "{}\n...[output truncated; 完整输出见 [artifact:{artifact}]]",
                        util::prefix(content, 4000)
                    );
                    count += 1;
                }
                _ => {}
            }
        }
    }
    Ok(count)
}
