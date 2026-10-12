use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};
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
