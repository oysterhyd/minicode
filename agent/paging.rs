use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};
pub const ARTIFACT_DEFAULT_LIMIT: usize = 4000;

pub fn artifact_line_mode(args: &Value) -> bool {
    ["start_line", "end_line", "pattern", "tail_lines"]
        .iter()
        .any(|key| !args[*key].is_null())
}

pub fn artifact_lines(path: &Path, args: &Value) -> Result<String> {
    let start = args["start_line"].as_u64().unwrap_or(1).max(1) as usize;
    let end = args["end_line"].as_u64().unwrap_or(u64::MAX) as usize;
    let tail = args["tail_lines"]
        .as_u64()
        .map(|n| n.clamp(1, 2000) as usize);
    if args["offset"].as_u64().unwrap_or(0) != 0 {
        bail!("line views cannot be combined with a character offset");
    }
    if end < start {
        bail!("end_line must be at least start_line");
    }
    if tail.is_some() && (args.get("start_line").is_some() || args.get("end_line").is_some()) {
        bail!("tail_lines cannot be combined with a line range");
    }
    let regex = args["pattern"]
        .as_str()
        .map(|s| {
            regex::RegexBuilder::new(s)
                .case_insensitive(args["case_sensitive"] == false)
                .build()
        })
        .transpose()
        .map_err(|error| anyhow::anyhow!("invalid regex: {error}"))?;
    let max_line = std::env::var("MINICODE_ARTIFACT_LINE_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| (1..=16 * 1024 * 1024).contains(n))
        .unwrap_or(1024 * 1024);
    let limit = args["limit"]
        .as_u64()
        .unwrap_or(ARTIFACT_DEFAULT_LIMIT as u64)
        .clamp(1, 20000) as usize;
    let mut reader = BufReader::new(File::open(path)?);
    let mut number = 0;
    let mut queue = VecDeque::new();
    let mut output = LineOutput {
        remaining: limit,
        text: String::new(),
        regex,
    };
    loop {
        let mut bytes = vec![];
        let capture = tail.is_some() || number + 1 >= start;
        let mut seen = false;
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let count = buf
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(buf.len());
            seen = true;
            if capture && bytes.len() + count > max_line {
                bail!(
                    "artifact line {} exceeds {max_line} bytes (MINICODE_ARTIFACT_LINE_MAX_BYTES); use character pagination",
                    number + 1
                );
            }
            let newline = buf[count - 1] == b'\n';
            if capture {
                bytes.extend_from_slice(&buf[..count]);
            }
            reader.consume(count);
            if newline {
                break;
            }
        }
        if !seen {
            break;
        }
        number += 1;
        if !capture {
            continue;
        }
        let line = String::from_utf8(bytes)?;
        if let Some(tail) = tail {
            let selected = output
                .regex
                .as_ref()
                .is_none_or(|r| r.is_match(line.trim_end_matches(['\r', '\n'])));
            queue.push_back((
                number,
                selected.then(|| line.chars().take(limit + 1).collect::<String>()),
            ));
            if queue.len() > tail {
                queue.pop_front();
            }
        } else if number >= start && !output.add(number, &line, false) {
            return Ok(output.text);
        }
        if tail.is_none() && number >= end {
            break;
        }
    }
    for (number, line) in queue {
        if let Some(line) = line
            && !output.add(number, &line, true)
        {
            break;
        }
    }
    if output.text.is_empty() {
        Ok("(no matching lines)".into())
    } else {
        Ok(output.text)
    }
}

struct LineOutput {
    remaining: usize,
    text: String,
    regex: Option<regex::Regex>,
}
impl LineOutput {
    fn add(&mut self, number: usize, line: &str, matched: bool) -> bool {
        let line = line.trim_end_matches(['\r', '\n']);
        if !matched && self.regex.as_ref().is_some_and(|r| !r.is_match(line)) {
            return true;
        }
        let rendered = format!("{number}\t{line}");
        let needed = rendered.chars().count() + usize::from(!self.text.is_empty());
        if needed > self.remaining {
            if self.text.is_empty() {
                self.text = rendered.chars().take(self.remaining).collect();
                self.text
                    .push_str("\n...[行已截断；请用字符分页读取完整行]");
            } else {
                self.text
                    .push_str(&format!("\n...[更多内容：next_start_line={number}]"));
            }
            return false;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(&rendered);
        self.remaining -= needed;
        true
    }
}
#[derive(Default)]
pub struct TextPages {
    index: Mutex<Vec<Index>>,
}
struct Index {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    positions: BTreeMap<usize, u64>,
}
impl TextPages {
    pub fn read(&self, path: &Path, offset: usize, limit: usize) -> Result<Value> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        let size = meta.len();
        let modified = meta.modified()?;
        let (mut skipped, byte) = self
            .index
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.path == path && i.size == size && i.modified == modified)
            .and_then(|i| i.positions.range(..=offset).next_back())
            .map(|(k, v)| (*k, *v))
            .unwrap_or((0, 0));
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(byte))?;
        let mut text = String::new();
        let mut count = 0;
        let mut extra = 0;
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let valid = match std::str::from_utf8(buf) {
                Ok(s) => s.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(e) => return Err(e.into()),
            };
            if valid == 0 {
                let first = buf[0];
                let length = if first & 0xe0 == 0xc0 {
                    2
                } else if first & 0xf0 == 0xe0 {
                    3
                } else if first & 0xf8 == 0xf0 {
                    4
                } else {
                    anyhow::bail!("invalid UTF-8")
                };
                let mut bytes = [0u8; 4];
                reader.read_exact(&mut bytes[..length])?;
                let ch = std::str::from_utf8(&bytes[..length])?
                    .chars()
                    .next()
                    .context("empty UTF-8 scalar")?;
                if skipped < offset {
                    skipped += 1
                } else {
                    text.push(ch);
                    count += 1;
                    if count > limit {
                        extra = ch.len_utf8();
                        break;
                    }
                }
                continue;
            }
            let part = std::str::from_utf8(&buf[..valid])?;
            let mut consumed = 0;
            for ch in part.chars() {
                consumed += ch.len_utf8();
                if skipped < offset {
                    skipped += 1
                } else {
                    text.push(ch);
                    count += 1;
                    if count > limit {
                        extra = ch.len_utf8();
                        break;
                    }
                }
            }
            reader.consume(consumed);
            if count > limit {
                break;
            }
        }
        let cursor = reader.stream_position()? - (extra as u64);
        let more = count > limit;
        if more {
            text.pop();
            count -= 1;
        }
        let mut index = self.index.lock().unwrap();
        let current = index.iter().position(|i| i.path == path);
        let mut item = current
            .map(|n| index.remove(n))
            .filter(|i| i.size == size && i.modified == modified)
            .unwrap_or(Index {
                path: path.into(),
                size,
                modified,
                positions: BTreeMap::from([(0, 0)]),
            });
        item.positions.insert(skipped + count, cursor);
        while item.positions.len() > 64 {
            let victim = *item.positions.keys().nth(1).unwrap();
            item.positions.remove(&victim);
        }
        index.push(item);
        if index.len() > 16 {
            index.remove(0);
        }
        Ok(json!({"text":text,"total":if more{None}else{Some(skipped+count)},"hasMore":more}))
    }
}
