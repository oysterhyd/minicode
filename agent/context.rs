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

const SUMMARY_TOOL_USE_MAX_CHARS: usize = 1600;
const SUMMARY_TOOL_USE_HEAD_CHARS: usize = 1000;
const SUMMARY_TOOL_USE_TAIL_CHARS: usize = 400;
const SUMMARY_TOOL_RESULT_MAX_CHARS: usize = 2400;
const SUMMARY_TOOL_RESULT_HEAD_CHARS: usize = 1400;
const SUMMARY_TOOL_RESULT_TAIL_CHARS: usize = 800;
const SUMMARY_THINKING_MAX_CHARS: usize = 1000;
const SUMMARY_THINKING_HEAD_CHARS: usize = 500;
const SUMMARY_THINKING_TAIL_CHARS: usize = 300;

fn head_tail_truncate(
    text: &str,
    max_chars: usize,
    head_chars: usize,
    tail_chars: usize,
) -> String {
    let total = text.chars().count();
    if total <= max_chars || head_chars.saturating_add(tail_chars) >= total {
        return text.to_string();
    }
    let head_end = text
        .char_indices()
        .nth(head_chars)
        .map_or(text.len(), |(i, _)| i);
    let tail_start = if tail_chars == 0 {
        text.len()
    } else {
        text.char_indices()
            .nth(total - tail_chars)
            .map_or(text.len(), |(i, _)| i)
    };
    let omitted = total - head_chars - tail_chars;
    format!(
        "{}\n...[omitted {omitted} chars]...\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

/// Render messages into compact, LLM-readable transcript blocks (one block per message)
/// with bounded head+tail excerpts for verbose tool calls, tool outputs, and reasoning.
pub fn render_for_summary(messages: &[Message]) -> Vec<String> {
    let mut rendered = Vec::with_capacity(messages.len());
    for message in messages {
        let role_label = match message.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            other => other,
        };
        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                Block::Text { text } => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        parts.push(format!("[{role_label}] {trimmed}"));
                    }
                }
                Block::Thinking { thinking, .. } => {
                    let trimmed = thinking.trim();
                    if !trimmed.is_empty() {
                        let body = head_tail_truncate(
                            trimmed,
                            SUMMARY_THINKING_MAX_CHARS,
                            SUMMARY_THINKING_HEAD_CHARS,
                            SUMMARY_THINKING_TAIL_CHARS,
                        );
                        parts.push(format!("[Thinking] {body}"));
                    }
                }
                Block::RedactedThinking { .. } => {
                    parts.push("[RedactedThinking]".into());
                }
                Block::Media {
                    asset_id,
                    kind,
                    name,
                    mime_type,
                    size,
                } => {
                    parts.push(format!(
                        "[Media kind={kind} name={name} mime={mime_type} size={size} id={asset_id}]"
                    ));
                }
                Block::ToolUse {
                    id,
                    name,
                    input,
                    input_error,
                } => {
                    let serialized = serde_json::to_string(input).unwrap_or_default();
                    let truncated = serialized.chars().count() > SUMMARY_TOOL_USE_MAX_CHARS;
                    let mut body = head_tail_truncate(
                        &serialized,
                        SUMMARY_TOOL_USE_MAX_CHARS,
                        SUMMARY_TOOL_USE_HEAD_CHARS,
                        SUMMARY_TOOL_USE_TAIL_CHARS,
                    );
                    if truncated
                        && let Some(path) = ["path", "file_path"]
                            .iter()
                            .filter_map(|key| input[*key].as_str())
                            .find(|path| !path.is_empty() && !body.contains(path))
                    {
                        body = format!("path={path} {body}");
                    }
                    if let Some(error) = input_error {
                        parts.push(format!(
                            "[ToolUse name={name} id={id} input_error={error}] {body}"
                        ));
                    } else {
                        parts.push(format!("[ToolUse name={name} id={id}] {body}"));
                    }
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    images,
                } => {
                    let body = head_tail_truncate(
                        content.trim(),
                        SUMMARY_TOOL_RESULT_MAX_CHARS,
                        SUMMARY_TOOL_RESULT_HEAD_CHARS,
                        SUMMARY_TOOL_RESULT_TAIL_CHARS,
                    );
                    let image_note = if images.is_empty() {
                        String::new()
                    } else {
                        format!(" [{} image(s)]", images.len())
                    };
                    if body.is_empty() {
                        parts.push(format!(
                            "[ToolResult id={tool_use_id} error={is_error}]{image_note}"
                        ));
                    } else {
                        parts.push(format!(
                            "[ToolResult id={tool_use_id} error={is_error}]{image_note}\n{body}"
                        ));
                    }
                }
            }
        }
        if !parts.is_empty() {
            rendered.push(parts.join("\n"));
        }
    }
    rendered
}

fn fallback_summary(archive: &[Message], refs: &[String], artifact: &str) -> Message {
    let mut goals = vec![];
    let mut findings = vec![];
    let mut assistant_notes = vec![];
    let mut modified_files = vec![];
    let mut seen_modified = HashSet::new();

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

    let successful_ids = archive
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            Block::ToolResult {
                tool_use_id,
                is_error: false,
                ..
            } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    let mut push_modified = |path: &str| {
        let path = path.trim();
        if !path.is_empty()
            && path != "."
            && path != "无"
            && path.len() <= 1024
            && modified_files.len() < 40
            && seen_modified.insert(path.to_string())
        {
            modified_files.push(path.to_string());
        }
    };

    for m in archive {
        if m.role == "assistant" {
            let text = m.text();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                assistant_notes.push(head_tail_truncate(trimmed, 240, 140, 80));
            }
        }
        for b in &m.content {
            match b {
                Block::Text { text } => {
                    if text.starts_with("此前历史已归档") {
                        let mut in_goals = false;
                        for line in text.lines() {
                            if line == "目标：" {
                                in_goals = true;
                                continue;
                            }
                            if matches!(line, "关键进展与结论：" | "失败与待解决问题：")
                                || line.starts_with("已修改文件：")
                                || line.starts_with("关键文件：")
                                || line.starts_with("<structured-summary>")
                            {
                                in_goals = false;
                            } else if in_goals && !line.trim().is_empty() && goals.len() < 12 {
                                goals.push(line.chars().take(240).collect::<String>());
                            }
                            if let Some(rest) = line.strip_prefix("已修改文件：") {
                                for entry in rest.split(',') {
                                    push_modified(entry);
                                }
                            }
                        }
                    } else if m.role == "user"
                        && !text.starts_with("<system-reminder>")
                        && goals.len() < 12
                    {
                        goals.push(text.chars().take(240).collect::<String>());
                    }
                }
                Block::ToolUse {
                    id,
                    name,
                    input,
                    input_error,
                } if matches!(name.as_str(), "edit" | "write")
                    && input_error.is_none()
                    && successful_ids.contains(id.as_str()) =>
                {
                    for key in ["path", "file_path"] {
                        if let Some(path) = input[key].as_str() {
                            push_modified(path);
                        }
                    }
                }
                Block::ToolResult {
                    content,
                    is_error: true,
                    ..
                } if findings.len() < 12 => {
                    findings.push(content.chars().take(240).collect::<String>());
                }
                _ => {}
            }
        }
    }

    if assistant_notes.len() > 5 {
        assistant_notes.drain(..assistant_notes.len() - 5);
    }

    let mut sections = vec![format!(
        "此前历史已归档，完整消息见 [artifact:{artifact}]。"
    )];
    if let Some(prev) = previous {
        sections.push(prev);
    }
    sections.push(format!("目标：\n{}", goals.join("\n")));
    if !assistant_notes.is_empty() {
        sections.push(format!("关键进展与结论：\n{}", assistant_notes.join("\n")));
    }
    if !modified_files.is_empty() {
        sections.push(format!("已修改文件：{}", modified_files.join(", ")));
    }
    sections.push(format!("关键文件：{}", refs.join(", ")));
    sections.push(format!("失败与待解决问题：\n{}", findings.join("\n")));
    Message::user(sections.join("\n"))
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
        let refs = history_refs(archive);
        let summary = |artifact: &str| {
            if let Some(text) = generated {
                return Message::user(format!(
                    "此前历史已归档，完整消息见 [artifact:{artifact}]。\n<structured-summary>\n{text}\n</structured-summary>\n文件引用：{}",
                    refs.join(", ")
                ));
            }
            fallback_summary(archive, &refs, artifact)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_for_summary_formats_all_blocks_and_truncates_large_payloads() {
        let huge_output = format!(
            "HEAD_START {}\n{}\nTAIL_END 中文🦀",
            "中".repeat(2000),
            "LOG_LINE\n".repeat(6000),
        );
        let huge_write_content = format!("{}\n{}", "中文内容".repeat(600), "TAIL_CODE");
        let messages = vec![
            Message::user("  请修改核心逻辑  "),
            Message {
                role: "assistant".into(),
                content: vec![
                    Block::Thinking {
                        thinking: format!("先分析{}后总结", "推理".repeat(700)),
                        signature: Some("secret_signature_blob".into()),
                        source: "anthropic:test".into(),
                    },
                    Block::RedactedThinking {
                        data: "opaque_encrypted_blob".into(),
                        source: "anthropic:test".into(),
                    },
                    Block::Text {
                        text: "我将读取文件并写入补丁。".into(),
                    },
                    Block::Media {
                        asset_id: "asset_123".into(),
                        kind: "image".into(),
                        name: "screen.png".into(),
                        mime_type: "image/png".into(),
                        size: 2048,
                    },
                    Block::ToolUse {
                        id: "call_write".into(),
                        name: "write".into(),
                        input: json!({
                            "content": huge_write_content,
                            "path": "src/target.rs"
                        }),
                        input_error: None,
                    },
                    Block::ToolUse {
                        id: "call_middle".into(),
                        name: "write".into(),
                        input: json!({
                            "head": "H".repeat(1200),
                            "path": "src/middle.rs",
                            "tail": "T".repeat(600)
                        }),
                        input_error: None,
                    },
                    Block::ToolUse {
                        id: "call_bad".into(),
                        name: "edit".into(),
                        input: json!({"raw": "broken"}),
                        input_error: Some("invalid json".into()),
                    },
                ],
            },
            Message {
                role: "user".into(),
                content: vec![
                    Block::ToolResult {
                        tool_use_id: "call_write".into(),
                        content: huge_output,
                        is_error: false,
                        images: vec![Block::Media {
                            asset_id: "img_1".into(),
                            kind: "image".into(),
                            name: "diff.png".into(),
                            mime_type: "image/png".into(),
                            size: 100,
                        }],
                    },
                    Block::ToolResult {
                        tool_use_id: "call_bad".into(),
                        content: "   ".into(),
                        is_error: true,
                        images: vec![],
                    },
                ],
            },
        ];

        let rendered = render_for_summary(&messages);
        assert_eq!(rendered.len(), 3);
        assert_eq!(rendered[0], "[User] 请修改核心逻辑");

        let assistant = &rendered[1];
        assert!(assistant.contains("[Thinking] 先分析"));
        assert!(assistant.contains("...[omitted "));
        assert!(assistant.contains("后总结"));
        assert!(!assistant.contains("secret_signature_blob"));
        assert!(assistant.contains("[RedactedThinking]"));
        assert!(!assistant.contains("opaque_encrypted_blob"));
        assert!(assistant.contains("[Assistant] 我将读取文件并写入补丁。"));
        assert!(
            assistant.contains(
                "[Media kind=image name=screen.png mime=image/png size=2048 id=asset_123]"
            )
        );
        assert!(assistant.contains("[ToolUse name=write id=call_write]"));
        assert!(assistant.contains("src/target.rs"));
        assert!(assistant.contains("[ToolUse name=write id=call_middle] path=src/middle.rs "));
        assert!(assistant.contains("[ToolUse name=edit id=call_bad input_error=invalid json]"));

        let tool_results = &rendered[2];
        assert!(
            tool_results
                .contains("[ToolResult id=call_write error=false] [1 image(s)]\nHEAD_START ")
        );
        assert!(tool_results.contains("...[omitted "));
        assert!(tool_results.contains("TAIL_END 中文🦀"));
        assert!(tool_results.len() < 12_000);
        assert!(tool_results.contains("[ToolResult id=call_bad error=true]"));
    }

    #[test]
    fn fallback_summary_retains_recent_assistant_notes_and_successful_modified_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("test.db")).unwrap();
        let session = store
            .create(&dir.path().to_string_lossy(), "fake", "fake")
            .unwrap();

        let mut messages = vec![Message::user("原始任务要求")];
        for i in 1..=7 {
            messages.push(Message {
                role: "assistant".into(),
                content: vec![
                    Block::text(format!("进展第{i}步结论")),
                    Block::ToolUse {
                        id: format!("edit_{i}"),
                        name: if i % 2 == 0 { "edit" } else { "write" }.into(),
                        input: json!({"path": format!("src/file_{i}.rs")}),
                        input_error: None,
                    },
                ],
            });
            messages.push(Message {
                role: "user".into(),
                content: vec![Block::ToolResult {
                    tool_use_id: format!("edit_{i}"),
                    content: if i == 2 {
                        "permission denied".into()
                    } else {
                        "ok".into()
                    },
                    is_error: i == 2,
                    images: vec![],
                }],
            });
        }
        // Failed input_error edit and read-only read must NOT appear in 已修改文件.
        messages.push(Message {
            role: "assistant".into(),
            content: vec![
                Block::ToolUse {
                    id: "bad_edit".into(),
                    name: "edit".into(),
                    input: json!({"path": "src/bad_input.rs"}),
                    input_error: Some("bad args".into()),
                },
                Block::ToolUse {
                    id: "read_only".into(),
                    name: "read".into(),
                    input: json!({"path": "src/read_only.rs"}),
                    input_error: None,
                },
            ],
        });
        messages.push(Message {
            role: "user".into(),
            content: vec![
                Block::ToolResult {
                    tool_use_id: "bad_edit".into(),
                    content: "ok".into(),
                    is_error: false,
                    images: vec![],
                },
                Block::ToolResult {
                    tool_use_id: "read_only".into(),
                    content: "contents ".repeat(300),
                    is_error: false,
                    images: vec![],
                },
            ],
        });
        messages.push(Message::user("继续下一步"));

        let stats = compact_summarized(
            &store,
            &session,
            "",
            &[],
            &mut messages,
            200_000,
            (true, None),
        )
        .unwrap()
        .unwrap();
        assert!(stats["archived_units"].as_u64().unwrap() > 0);
        let summary_msg = messages
            .iter()
            .find(|m| m.text().contains("此前历史已归档"))
            .unwrap()
            .text();
        assert!(summary_msg.contains("关键进展与结论："));
        assert!(!summary_msg.contains("进展第1步结论"));
        assert!(!summary_msg.contains("进展第2步结论"));
        for i in 3..=7 {
            assert!(summary_msg.contains(&format!("进展第{i}步结论")));
        }
        let modified_line = summary_msg
            .lines()
            .find(|line| line.starts_with("已修改文件："))
            .unwrap();
        assert!(modified_line.contains("src/file_1.rs"));
        assert!(modified_line.contains("src/file_7.rs"));
        assert!(!modified_line.contains("src/file_2.rs"));
        assert!(!modified_line.contains("src/bad_input.rs"));
        assert!(!modified_line.contains("src/read_only.rs"));

        // Chained fallback compaction carries forward previously recorded 已修改文件.
        messages.insert(
            2,
            Message {
                role: "assistant".into(),
                content: vec![
                    Block::text("后续新结论"),
                    Block::ToolUse {
                        id: "write_8".into(),
                        name: "write".into(),
                        input: json!({"file_path": "src/file_8.rs"}),
                        input_error: None,
                    },
                ],
            },
        );
        messages.insert(
            3,
            Message {
                role: "user".into(),
                content: vec![Block::ToolResult {
                    tool_use_id: "write_8".into(),
                    content: "ok ".repeat(300),
                    is_error: false,
                    images: vec![],
                }],
            },
        );
        messages.push(Message::user("最新请求"));
        compact_summarized(
            &store,
            &session,
            "",
            &[],
            &mut messages,
            200_000,
            (true, None),
        )
        .unwrap()
        .unwrap();
        let chained = messages
            .iter()
            .find(|m| m.text().contains("此前历史已归档"))
            .unwrap()
            .text();
        let chained_modified = chained
            .lines()
            .find(|line| line.starts_with("已修改文件："))
            .unwrap();
        assert!(
            chained_modified.contains("src/file_1.rs"),
            "chained:\n{chained}"
        );
        assert!(
            chained_modified.contains("src/file_8.rs"),
            "stats={stats:?}\nchained:\n{chained}"
        );
        assert!(chained.contains("后续新结论"), "chained:\n{chained}");
    }
}
