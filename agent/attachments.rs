use crate::{models::Block, util};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
#[derive(Clone)]
pub struct Attachments {
    pub root: PathBuf,
}
const FORMATS: &[(&str, &str, &str)] = &[
    (".png", "image", "image/png"),
    (".jpeg", "image", "image/jpeg"),
    (".gif", "image", "image/gif"),
    (".webp", "image", "image/webp"),
    (".pdf", "pdf", "application/pdf"),
    (".wav", "audio", "audio/wav"),
    (".mp3", "audio", "audio/mpeg"),
    (".mp4", "video", "video/mp4"),
    (".mov", "video", "video/quicktime"),
    (".webm", "video", "video/webm"),
    (".jpg", "image", "image/jpeg"),
];
impl Attachments {
    pub fn import(&self, path: &Path) -> Result<Block> {
        let ext = format!(
            ".{}",
            path.extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
        );
        let (_, _, mime) = FORMATS
            .iter()
            .find(|(s, _, _)| *s == ext)
            .context("请选择图片、PDF、WAV / MP3 音频或 MP4 / MOV / WebM 视频")?;
        let mut data = vec![];
        fs::File::open(path)?
            .take(20_000_001)
            .read_to_end(&mut data)?;
        self.import_bytes(
            &data,
            &path.file_name().unwrap_or_default().to_string_lossy(),
            mime,
        )
    }
    pub fn import_bytes(&self, data: &[u8], name: &str, mime: &str) -> Result<Block> {
        let (_, kind, _) = FORMATS
            .iter()
            .find(|(_, _, m)| *m == mime)
            .context("Unsupported attachment MIME type")?;
        if data.is_empty() || data.len() > 20_000_000 {
            bail!("附件不能为空，单个文件不能超过 20 MB")
        }
        let valid = match mime {
            "image/png" => data.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => data.starts_with(b"\xff\xd8\xff"),
            "image/gif" => data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a"),
            "image/webp" => data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP"),
            "application/pdf" => data[..data.len().min(1024)]
                .windows(5)
                .any(|x| x == b"%PDF-"),
            "audio/wav" => data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WAVE"),
            "audio/mpeg" => {
                data.starts_with(b"ID3")
                    || (data.len() > 1 && data[0] == 255 && data[1] & 224 == 224)
            }
            "video/mp4" | "video/quicktime" => data.get(4..8) == Some(b"ftyp"),
            "video/webm" => data.starts_with(b"\x1aE\xdf\xa3"),
            _ => false,
        };
        if !valid {
            bail!("文件内容与格式不符：{name}")
        }
        if name.is_empty() || name.chars().count() > 255 {
            bail!("invalid attachment name")
        }
        let asset = asset_digest(mime, data);
        let block = Block::Media {
            asset_id: asset.clone(),
            kind: (*kind).into(),
            name: name.into(),
            mime_type: mime.into(),
            size: data.len() as u64,
        };
        util::atomic_write(&self.path(&block)?, data)?;
        util::atomic_write(
            &self.root.join(format!("{asset}.json")),
            &serde_json::to_vec(&block)?,
        )?;
        Ok(block)
    }
    pub fn get(&self, id: &str) -> Result<Block> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("无效附件 ID")
        }
        let block: Block = serde_json::from_slice(
            &fs::read(self.root.join(format!("{id}.json")))
                .context("附件不存在或已损坏，请重新添加")?,
        )?;
        match &block {
            Block::Media {
                asset_id,
                kind,
                mime_type,
                size,
                ..
            } if asset_id == id
                && *size > 0
                && *size <= 20_000_000
                && FORMATS
                    .iter()
                    .any(|(_, k, m)| *k == kind && *m == mime_type) => {}
            _ => bail!("附件元数据已损坏，请重新添加"),
        };
        Ok(block)
    }
    pub fn path(&self, block: &Block) -> Result<PathBuf> {
        if let Block::Media {
            asset_id,
            mime_type,
            ..
        } = block
        {
            let (suffix, _, _) = FORMATS
                .iter()
                .find(|(_, _, m)| *m == mime_type)
                .context("invalid MIME")?;
            Ok(self.root.join(format!("{asset_id}{suffix}")))
        } else {
            bail!("not a media block")
        }
    }
    pub fn encoded(&self, block: &Block) -> Result<String> {
        let Block::Media {
            asset_id,
            mime_type,
            size,
            ..
        } = block
        else {
            bail!("not an attachment")
        };
        let saved = self.get(asset_id)?;
        // Names belong to individual message references. Identical bytes can
        // be imported again under a different filename without changing them.
        if !matches!(&saved, Block::Media { asset_id: saved_id, kind: saved_kind, mime_type: saved_mime, size: saved_size, .. }
            if saved_id == asset_id && saved_mime == mime_type && saved_size == size
                && matches!(block, Block::Media { kind, .. } if kind == saved_kind))
        {
            bail!("附件元数据不一致，请重新添加")
        }
        let mut data = vec![];
        fs::File::open(self.path(block)?)?
            .take(20_000_001)
            .read_to_end(&mut data)?;
        if data.len() as u64 != *size || asset_digest(mime_type, &data) != *asset_id {
            bail!("附件已损坏，请重新添加")
        }
        Ok(STANDARD.encode(data))
    }
    pub fn check(&self, blocks: &[Block], modalities: &[String], style: &str) -> Result<()> {
        if blocks.len() > 10
            || blocks
                .iter()
                .map(|b| {
                    if let Block::Media { size, .. } = b {
                        *size
                    } else {
                        0
                    }
                })
                .sum::<u64>()
                > 24_000_000
        {
            bail!("最多 10 个附件，附件总大小不能超过 24 MB")
        }
        for b in blocks {
            if let Block::Media { kind, name, .. } = b {
                if !modalities.contains(kind) {
                    bail!(
                        "当前模型未启用 {kind} 输入能力：{name}，请在模型设置中确认能力或切换模型"
                    )
                }
                if style == "anthropic" && ["audio", "video"].contains(&kind.as_str()) {
                    bail!(
                        "Anthropic Messages 接口不支持原生音频或视频附件，请选择支持这些输入的兼容接口"
                    )
                }
            } else {
                bail!("invalid attachment")
            }
        }
        Ok(())
    }
}

fn asset_digest(mime: &str, data: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(mime.as_bytes());
    digest.update([0]);
    digest.update(data);
    format!("{:x}", digest.finalize())
}
