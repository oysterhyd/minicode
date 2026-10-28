use anyhow::{Context, Result, bail};
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Encoding {
    Utf8(bool),
    Utf16 { le: bool, bom: bool },
    Gbk,
    Gb18030,
}

impl Encoding {
    pub fn probe(path: &Path) -> Result<Self> {
        let mut sample = vec![];
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        file.take(decode_limit()).read_to_end(&mut sample)?;
        Ok(Self::detect(&sample, (sample.len() as u64) < size))
    }

    fn detect(bytes: &[u8], partial: bool) -> Self {
        if bytes.starts_with(b"\xef\xbb\xbf") {
            return Self::Utf8(true);
        }
        for (bom, le) in [(b"\xff\xfe", true), (b"\xfe\xff", false)] {
            if bytes.starts_with(bom) {
                return Self::Utf16 { le, bom: true };
            }
        }
        if bytes.len() >= 4 {
            let even = bytes.iter().step_by(2).filter(|b| **b == 0).count();
            let odd = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
            if odd > bytes.len() / 6 && even < bytes.len() / 16 + 1 {
                return Self::Utf16 {
                    le: true,
                    bom: false,
                };
            }
            if even > bytes.len() / 6 && odd < bytes.len() / 16 + 1 {
                return Self::Utf16 {
                    le: false,
                    bom: false,
                };
            }
        }
        if std::str::from_utf8(bytes).is_ok_and(|s| !s.contains('\0'))
            || partial && std::str::from_utf8(bytes).is_err_and(|e| e.error_len().is_none())
        {
            return Self::Utf8(false);
        }
        if bytes.windows(4).any(|b| {
            (0x81..=0xfe).contains(&b[0])
                && b[1].is_ascii_digit()
                && (0x81..=0xfe).contains(&b[2])
                && b[3].is_ascii_digit()
        }) {
            Self::Gb18030
        } else {
            Self::Gbk
        }
    }

    pub fn decode(self, bytes: &[u8]) -> Result<String> {
        let text = match self {
            Self::Utf8(bom) => std::str::from_utf8(if bom {
                bytes
                    .strip_prefix(b"\xef\xbb\xbf")
                    .context("UTF-8 BOM changed")?
            } else {
                bytes
            })?
            .to_string(),
            Self::Utf16 { le, bom } => {
                let bytes = if bom {
                    bytes
                        .strip_prefix(if le { b"\xff\xfe" } else { b"\xfe\xff" })
                        .context("UTF-16 BOM changed")?
                } else {
                    bytes
                };
                if bytes.len() % 2 != 0 {
                    bail!("incomplete UTF-16 code unit");
                }
                let units = bytes.as_chunks::<2>().0.iter().map(|b| {
                    if le {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                });
                std::char::decode_utf16(units).collect::<std::result::Result<String, _>>()?
            }
            Self::Gbk | Self::Gb18030 => encoding_rs::GB18030
                .decode_without_bom_handling_and_without_replacement(bytes)
                .context("binary or invalid GB18030 file")?
                .into_owned(),
        };
        if text.contains('\0') {
            bail!("binary file contains NUL characters");
        }
        Ok(text)
    }

    pub fn encode(self, text: &str) -> Result<Vec<u8>> {
        match self {
            Self::Utf8(bom) => Ok([
                if bom { b"\xef\xbb\xbf".as_slice() } else { b"" },
                text.as_bytes(),
            ]
            .concat()),
            Self::Utf16 { le, bom } => {
                let mut bytes = if bom {
                    if le { vec![255, 254] } else { vec![254, 255] }
                } else {
                    vec![]
                };
                for unit in text.encode_utf16() {
                    bytes.extend_from_slice(&if le {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    });
                }
                Ok(bytes)
            }
            Self::Gbk | Self::Gb18030 => {
                let encoding = if self == Self::Gbk {
                    encoding_rs::GBK
                } else {
                    encoding_rs::GB18030
                };
                let (bytes, _, errors) = encoding.encode(text);
                if errors {
                    bail!(
                        "replacement cannot be represented in original {} encoding; file was not changed",
                        encoding.name()
                    );
                }
                Ok(bytes.into_owned())
            }
        }
    }
}

fn decode_limit() -> u64 {
    std::env::var("MINICODE_TEXT_DECODE_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| (1..=100_000_000).contains(v))
        .unwrap_or(20_000_000)
}

pub(crate) fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let max = decode_limit();
    let mut bytes = vec![];
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        bail!(
            "decoded document exceeds {max} bytes; increase MINICODE_TEXT_DECODE_MAX_BYTES or read a smaller document"
        );
    }
    Ok(bytes)
}

pub(crate) fn notebook(bytes: &[u8]) -> Result<String> {
    let book: serde_json::Value = serde_json::from_slice(bytes)?;
    let cells = book["cells"]
        .as_array()
        .context("notebook must contain a cells array (nbformat 4)")?;
    fn source(value: &serde_json::Value) -> Result<String> {
        if value.is_null() {
            return Ok(String::new());
        }
        if let Some(s) = value.as_str() {
            return Ok(s.to_string());
        }
        value
            .as_array()
            .context("notebook text must be a string or string array")?
            .iter()
            .map(|s| s.as_str().context("invalid notebook text"))
            .collect::<Result<String>>()
    }
    let mut text = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let kind = cell["cell_type"]
            .as_str()
            .context("notebook cell has no type")?;
        text.push_str(&format!(
            "Cell {} [{kind}]\n{}\n",
            i + 1,
            source(&cell["source"])?
        ));
        if let Some(outputs) = cell["outputs"].as_array() {
            for output in outputs {
                let value = if !output["text"].is_null() {
                    &output["text"]
                } else if !output["data"]["text/plain"].is_null() {
                    &output["data"]["text/plain"]
                } else {
                    &output["traceback"]
                };
                let rendered = source(value)?;
                if !rendered.is_empty() {
                    text.push_str(&format!("Output:\n{rendered}\n"));
                }
            }
        }
    }
    Ok(text)
}
