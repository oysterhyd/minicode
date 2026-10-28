use crate::util;
use anyhow::{Result, bail};
use serde_json::Value;
use std::{borrow::Cow, collections::HashSet, sync::Arc};
use unicode_normalization::UnicodeNormalization;

pub(crate) struct Change {
    pub start: usize,
    pub end: usize,
    pub replacement: Arc<str>,
}

pub(crate) fn plan(text: &str, args: &Value, relative: &str) -> Result<Vec<Change>> {
    let operations = if let Some(edits) = args.get("edits") {
        let edits = edits
            .as_array()
            .filter(|e| !e.is_empty() && e.len() <= 100)
            .ok_or_else(|| anyhow::anyhow!("edits must contain 1–100 operations"))?;
        if args.get("old_text").is_some() || args.get("new_text").is_some() {
            bail!("use either edits or old_text/new_text");
        }
        edits.as_slice()
    } else {
        std::slice::from_ref(args)
    };
    let exact_only = args["match_mode"] == "exact";
    let max = std::env::var("MINICODE_EDIT_NORMALIZE_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| (1..=20 * 1024 * 1024).contains(v))
        .unwrap_or(2 * 1024 * 1024);
    let mut changes = vec![];
    for operation in operations {
        let (old, new) = replacement_texts(text, operation)?;
        let replacement: Arc<str> = Arc::from(new.as_ref());
        let limit = if operation["replace_all"] == true {
            usize::MAX
        } else {
            2
        };
        let mut matches = text
            .match_indices(old.as_ref())
            .take(limit)
            .map(|(start, _)| (start, start + old.len(), replacement.clone()))
            .collect::<Vec<_>>();
        if matches.is_empty() && !exact_only && text.len() <= max {
            let source = normalize(text);
            let needle = normalize(&old).text;
            if !needle.is_empty() {
                for (start, _) in source.text.match_indices(&needle) {
                    let end = start + needle.len();
                    if start > 0 && source.spans[start - 1] == source.spans[start]
                        || end < source.spans.len() && source.spans[end - 1] == source.spans[end]
                    {
                        continue;
                    }
                    matches.push((
                        source.spans[start].0,
                        source.spans[end - 1].1,
                        replacement.clone(),
                    ));
                    if matches.len() >= limit {
                        break;
                    }
                }
            }
            if matches.is_empty() {
                matches = indent_matches(text, &old, &new, limit)?;
            }
            matches.sort_by_key(|m| (m.0, m.1));
            matches.dedup_by_key(|m| (m.0, m.1));
        }
        if matches.is_empty() {
            let limit = if !exact_only && text.len() > max {
                format!(
                    "; normalized matching is limited to {max} bytes (MINICODE_EDIT_NORMALIZE_MAX_BYTES), use exact context"
                )
            } else {
                String::new()
            };
            bail!(
                "old_text not found in {relative}: re-read the file first{limit}\n{}",
                nearest(text, &old)
            );
        }
        if matches.len() > 1 && operation["replace_all"] != true {
            bail!(
                "old_text matches at least {} locations in {relative}; include more surrounding context to make it unique, or pass replace_all=true",
                matches.len()
            );
        }
        if operation["replace_all"] != true {
            matches.truncate(1);
        }
        changes.extend(matches.into_iter().map(|(start, end, replacement)| Change {
            start,
            end,
            replacement,
        }));
    }
    changes.sort_by_key(|c| c.start);
    if changes.windows(2).any(|c| c[0].end > c[1].start) {
        bail!("edits overlap in {relative}; no changes applied");
    }
    Ok(changes)
}

pub(crate) fn apply(text: &str, changes: &[Change]) -> String {
    let mut output = String::with_capacity(text.len());
    let mut position = 0;
    for change in changes {
        output.push_str(&text[position..change.start]);
        output.push_str(&change.replacement);
        position = change.end;
    }
    output.push_str(&text[position..]);
    output
}

struct Normalized {
    text: String,
    spans: Vec<(usize, usize)>,
}
fn normalize(text: &str) -> Normalized {
    let mut out = Normalized {
        text: String::new(),
        spans: vec![],
    };
    let mut position = 0;
    for line in text.split_inclusive('\n') {
        let body = line
            .trim_end_matches(['\r', '\n'])
            .trim_end_matches([' ', '\t']);
        for (offset, ch) in body.char_indices() {
            for ch in ch.to_string().nfkd() {
                let ch = match ch {
                    '‘' | '’' | '‚' | '‛' => '\'',
                    '“' | '”' | '„' | '‟' => '"',
                    _ => ch,
                };
                out.text.push(ch);
                out.spans.extend(std::iter::repeat_n(
                    (
                        position + offset,
                        position + offset + body[offset..].chars().next().unwrap().len_utf8(),
                    ),
                    ch.len_utf8(),
                ));
            }
        }
        if line.ends_with('\n') {
            out.text.push('\n');
            out.spans
                .push((position + body.len(), position + line.len()));
        }
        position += line.len();
    }
    out
}

fn indent(line: &str) -> (usize, usize) {
    let mut width = 0;
    let mut bytes = 0;
    for ch in line.chars() {
        match ch {
            ' ' => width += 1,
            '\t' => width += 4,
            _ => break,
        };
        bytes += ch.len_utf8();
    }
    (width, bytes)
}
fn dedent(text: &str) -> (String, usize, String) {
    let lines = text.lines().collect::<Vec<_>>();
    let base = lines
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| indent(s).0)
        .min()
        .unwrap_or(0);
    let prefix = lines
        .iter()
        .find(|s| !s.trim().is_empty() && indent(s).0 == base)
        .map(|s| s[..indent(s).1].to_string())
        .unwrap_or_default();
    let normalized = lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                let (columns, bytes) = indent(line);
                format!("{}:{}", columns - base, normalize(&line[bytes..]).text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (normalized, base, prefix)
}
fn indent_matches(
    text: &str,
    old: &str,
    new: &str,
    limit: usize,
) -> Result<Vec<(usize, usize, Arc<str>)>> {
    let mut offset = 0;
    let lines = text
        .split_inclusive('\n')
        .map(|line| {
            let span = (offset, offset + line.len());
            offset += line.len();
            span
        })
        .collect::<Vec<_>>();
    let count = old.lines().count();
    let (wanted, base, _) = dedent(old);
    let mut found: Vec<(usize, usize, Arc<str>)> = vec![];
    if count == 0 || wanted.is_empty() {
        return Ok(found);
    }
    let (anchor, line) = old
        .lines()
        .enumerate()
        .max_by_key(|(_, line)| line.trim().len())
        .unwrap();
    let needle = normalize(line.trim()).text;
    for window in lines.windows(count) {
        let candidate_line = &text[window[anchor].0..window[anchor].1];
        if normalize(candidate_line.trim()).text != needle {
            continue;
        }
        let start = window[0].0;
        let end = window[count - 1].1;
        let candidate = &text[start..end];
        let (normalized, _, prefix) = dedent(candidate);
        if normalized != wanted {
            continue;
        }
        if limit == usize::MAX && found.last().is_some_and(|last| last.1 > start) {
            bail!("normalized edits overlap; use exact context");
        }
        let mut replacement = String::new();
        for line in new.split_inclusive('\n') {
            if line.trim().is_empty() {
                replacement.push_str(line);
                continue;
            }
            let mut remove = 0;
            let mut bytes = 0;
            for ch in line.chars() {
                if remove >= base {
                    break;
                }
                match ch {
                    ' ' => remove += 1,
                    '\t' => remove += 4,
                    _ => break,
                };
                bytes += ch.len_utf8();
            }
            if remove != base {
                bail!("replacement indentation cannot be aligned safely; use exact matching");
            }
            replacement.push_str(&prefix);
            replacement.push_str(&line[bytes..]);
        }
        let end = if old.ends_with('\n') {
            end
        } else {
            end - candidate.len() + candidate.trim_end_matches(['\r', '\n']).len()
        };
        found.push((start, end, Arc::from(replacement)));
        if found.len() >= limit {
            break;
        }
    }
    Ok(found)
}

fn nearest(text: &str, old: &str) -> String {
    let anchor = old
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(old);
    let grams = |s: &str| {
        let chars = normalize(&s.chars().take(500).collect::<String>())
            .text
            .chars()
            .collect::<Vec<_>>();
        chars
            .windows(2)
            .map(|c| (c[0], c[1]))
            .collect::<HashSet<_>>()
    };
    let wanted = grams(anchor);
    let mut best = vec![];
    for (i, line) in text.lines().take(20000).enumerate() {
        let candidate = grams(line);
        let score = wanted.intersection(&candidate).count() * 2000
            / (wanted.len() + candidate.len()).max(1);
        best.push((score, i + 1, line));
        best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        best.truncate(3);
    }
    let snippets = best
        .into_iter()
        .map(|(_, line, text)| format!("{line}\t{}", text.chars().take(240).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n");
    format!("Nearest candidate lines (first 20000 lines inspected; not applied):\n{snippets}")
}

fn replacement_texts<'a>(text: &str, args: &'a Value) -> Result<(Cow<'a, str>, Cow<'a, str>)> {
    let old = util::string(args, "old_text")?;
    let new = util::string(args, "new_text")?;
    if old.is_empty() {
        bail!("old_text must be nonempty");
    }
    let crlf = text.contains("\r\n")
        && !text
            .as_bytes()
            .iter()
            .enumerate()
            .any(|(i, b)| *b == b'\n' && (i == 0 || text.as_bytes()[i - 1] != b'\r'));
    let convert = |value: &'a str| {
        if crlf && !value.contains('\r') {
            Cow::Owned(value.replace('\n', "\r\n"))
        } else {
            Cow::Borrowed(value)
        }
    };
    Ok((convert(old), convert(new)))
}
