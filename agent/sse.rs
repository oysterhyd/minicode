use anyhow::{Context, Result, bail};

const LIMIT: usize = 16_000_000;

pub(crate) struct Event {
    pub kind: String,
    pub data: String,
}

/// Incremental UTF-8 SSE framing shared by model and MCP transports.
#[derive(Default)]
pub(crate) struct Decoder {
    line: Vec<u8>,
    data: String,
    kind: String,
    skip_lf: bool,
    started: bool,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Event>> {
        let mut events = vec![];
        for part in bytes.split_inclusive(|b| *b == b'\r' || *b == b'\n') {
            if self.skip_lf && part == b"\n" {
                self.skip_lf = false;
                continue;
            }
            let delimiter = part.last().copied().filter(|b| *b == b'\r' || *b == b'\n');
            self.skip_lf = delimiter == Some(b'\r');
            let body = &part[..part.len() - usize::from(delimiter.is_some())];
            if self.line.len() + body.len() + self.data.len() + self.kind.len() > LIMIT {
                bail!("SSE event exceeds the capture limit");
            }
            self.line.extend_from_slice(body);
            if delimiter.is_some() {
                let raw = std::str::from_utf8(&self.line).context("invalid SSE UTF-8")?;
                let line = if self.started {
                    raw
                } else {
                    raw.trim_start_matches('\u{feff}')
                };
                self.started = true;
                if line.is_empty() {
                    if !self.data.is_empty() {
                        self.data.pop(); // SSE appends one newline for every data field.
                        events.push(Event {
                            kind: if self.kind.is_empty() {
                                "message".into()
                            } else {
                                std::mem::take(&mut self.kind)
                            },
                            data: std::mem::take(&mut self.data),
                        });
                    }
                    self.kind.clear();
                } else if !line.starts_with(':') {
                    let (field, value) = line.split_once(':').unwrap_or((line, ""));
                    let value = value.strip_prefix(' ').unwrap_or(value);
                    match field {
                        "data" => {
                            self.data.push_str(value);
                            self.data.push('\n');
                        }
                        "event" => self.kind = value.into(),
                        _ => {}
                    }
                }
                self.line.clear();
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_survives_every_chunk_boundary_and_all_line_endings() {
        let input = "\u{feff}: comment\r\nevent: endpoint\r\ndata:  first\r\ndata: 中文😀\r\n\r\ndata: second\r\rdata:\n\n";
        for split in 0..=input.len() {
            let mut decoder = Decoder::default();
            let mut events = decoder.push(&input.as_bytes()[..split]).unwrap();
            events.extend(decoder.push(&input.as_bytes()[split..]).unwrap());
            assert_eq!(events.len(), 3, "split {split}");
            assert_eq!(events[0].kind, "endpoint");
            assert_eq!(events[0].data, " first\n中文😀");
            assert_eq!(events[1].kind, "message");
            assert_eq!(events[1].data, "second");
            assert_eq!(events[2].data, "");
        }
    }

    #[test]
    fn incomplete_events_are_not_dispatched() {
        let mut decoder = Decoder::default();
        assert!(decoder.push(b"data: partial\n").unwrap().is_empty());
        let events = decoder.push(b"data: rest\n\n").unwrap();
        assert_eq!(events[0].data, "partial\nrest");
    }
}
