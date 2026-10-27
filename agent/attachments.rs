use crate::{models::Block, util};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, imageops::FilterType};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read},
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
        Ok(STANDARD.encode(self.verified_bytes(block)?))
    }
    pub fn encoded_image(&self, block: &Block) -> Result<(String, String)> {
        let Block::Media {
            kind, mime_type, ..
        } = block
        else {
            bail!("not an attachment")
        };
        if kind != "image" {
            bail!("not an image")
        }
        let data = self.verified_bytes(block)?;
        let edge = std::env::var("MINICODE_IMAGE_MAX_EDGE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| (32..=8000).contains(value))
            .unwrap_or(2000);
        let max_base64 = std::env::var("MINICODE_IMAGE_MAX_BASE64_BYTES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1024..=10_000_000).contains(value))
            .unwrap_or(4_900_000);
        let (mime, data) = prepare_image(&data, mime_type, edge, max_base64)?;
        Ok((mime, STANDARD.encode(data)))
    }
    fn verified_bytes(&self, block: &Block) -> Result<Vec<u8>> {
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
        Ok(data)
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

fn prepare_image(
    data: &[u8],
    mime: &str,
    max_edge: u32,
    max_base64: usize,
) -> Result<(String, Vec<u8>)> {
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => bail!("不支持的图片格式"),
    };
    let encoded_len = |size: usize| size.div_ceil(3) * 4;
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(32_768);
    limits.max_image_height = Some(32_768);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = match reader.into_decoder() {
        Ok(decoder) => decoder,
        Err(image::ImageError::Limits(error)) => bail!("图片尺寸超出解码限制：{error}"),
        // Existing attachments were checked by magic bytes only. Preserve the
        // old pass-through for small unrecognized files; never send oversized
        // data if it cannot be decoded and reduced.
        Err(_) if encoded_len(data.len()) <= max_base64 => return Ok((mime.into(), data.to_vec())),
        Err(error) => bail!("图片过大且无法解码缩放：{error}"),
    };
    let (width, height) = decoder.dimensions();
    if width <= max_edge && height <= max_edge && encoded_len(data.len()) <= max_base64 {
        return Ok((mime.into(), data.to_vec()));
    }
    if u64::from(width) * u64::from(height) > 64_000_000
        || decoder.total_bytes() > 256 * 1024 * 1024
    {
        bail!("图片像素数量超出解码限制，请先缩小图片")
    }
    let orientation = decoder.orientation().context("无法读取图片方向")?;
    let mut image = DynamicImage::from_decoder(decoder).context("无法解码图片")?;
    image.apply_orientation(orientation);
    if image.width() > max_edge || image.height() > max_edge {
        image = image.resize(max_edge, max_edge, FilterType::Triangle);
    }
    let output_mime = if format == ImageFormat::Jpeg {
        "image/jpeg"
    } else {
        "image/png"
    };
    loop {
        let mut output = Vec::new();
        if output_mime == "image/jpeg" {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 85)
                .encode_image(&image.to_rgb8())
                .context("无法压缩图片")?;
        } else {
            image
                .write_to(&mut Cursor::new(&mut output), ImageFormat::Png)
                .context("无法压缩图片")?;
        }
        if encoded_len(output.len()) <= max_base64 {
            return Ok((output_mime.into(), output));
        }
        if image.width() == 1 && image.height() == 1 {
            bail!("图片无法压缩到配置的体积上限")
        }
        let width = (image.width() * 4 / 5).max(1);
        let height = (image.height() * 4 / 5).max(1);
        image = image.resize_exact(width, height, FilterType::Triangle);
    }
}

fn asset_digest(mime: &str, data: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(mime.as_bytes());
    digest.update([0]);
    digest.update(data);
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgba};

    fn sample() -> DynamicImage {
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(160, 90, |x, y| {
            Rgba([
                (x * 3) as u8,
                (y * 5) as u8,
                (x * y) as u8,
                if x < 80 { 0 } else { 255 },
            ])
        }))
    }

    fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgb8(image.to_rgb8())
                .write_to(&mut output, format)
                .unwrap();
        } else {
            image.write_to(&mut output, format).unwrap();
        }
        output.into_inner()
    }

    #[test]
    fn supported_formats_resize_and_keep_small_images_unchanged() {
        for (format, mime) in [
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::WebP, "image/webp"),
        ] {
            let original = encode(&sample(), format);
            let (same_mime, same) = prepare_image(&original, mime, 2000, 4_900_000).unwrap();
            assert_eq!((same_mime.as_str(), same), (mime, original.clone()));
            let (output_mime, output) = prepare_image(&original, mime, 64, 100_000).unwrap();
            assert_eq!(
                output_mime,
                if format == ImageFormat::Jpeg {
                    "image/jpeg"
                } else {
                    "image/png"
                }
            );
            let resized = image::load_from_memory(&output).unwrap();
            assert_eq!(resized.dimensions(), (64, 36));
            if format != ImageFormat::Jpeg {
                assert_eq!(resized.get_pixel(4, 4).0[3], 0, "{mime}");
                assert_eq!(resized.get_pixel(60, 4).0[3], 255, "{mime}");
            }
        }
    }

    #[test]
    fn image_byte_limit_does_not_upscale_and_invalid_large_images_fail() {
        let mut state = 1u32;
        let noise = DynamicImage::ImageRgba8(ImageBuffer::from_fn(120, 80, |_, _| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Rgba([
                (state >> 24) as u8,
                (state >> 16) as u8,
                (state >> 8) as u8,
                255,
            ])
        }));
        let data = encode(&noise, ImageFormat::Png);
        let (_, output) = prepare_image(&data, "image/png", 2000, 4096).unwrap();
        assert!(output.len().div_ceil(3) * 4 <= 4096);
        let resized = image::load_from_memory(&output).unwrap();
        assert!(resized.width() <= 120 && resized.height() <= 80);
        let mut padded = data.clone();
        padded.resize(200_000, 0);
        let (_, recompressed) = prepare_image(&padded, "image/png", 2000, 100_000).unwrap();
        assert_eq!(
            image::load_from_memory(&recompressed).unwrap().dimensions(),
            (120, 80)
        );
        let invalid = [b"\x89PNG\r\n\x1a\n".as_slice(), &vec![0; 4096]].concat();
        assert!(prepare_image(&invalid, "image/png", 2000, 1024).is_err());
        assert_eq!(
            prepare_image(&invalid[..8], "image/png", 2000, 1024)
                .unwrap()
                .1,
            &invalid[..8]
        );
    }

    #[test]
    fn jpeg_orientation_is_applied_when_resizing() {
        let jpeg = encode(&sample(), ImageFormat::Jpeg);
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1];
        oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        oriented.extend_from_slice(exif);
        oriented.extend_from_slice(&jpeg[2..]);
        let (mime, output) = prepare_image(&oriented, "image/jpeg", 64, 100_000).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(
            image::load_from_memory(&output).unwrap().dimensions(),
            (36, 64)
        );
    }

    #[test]
    fn image_dimensions_are_checked_before_allocating_pixels() {
        let mut png = encode(&DynamicImage::new_rgba8(1, 1), ImageFormat::Png);
        png[16..20].copy_from_slice(&10_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&10_000u32.to_be_bytes());
        let mut crc = 0xffff_ffffu32;
        for byte in &png[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320u32 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        png[29..33].copy_from_slice(&(!crc).to_be_bytes());
        assert!(prepare_image(&png, "image/png", 2000, 4_900_000).is_err());
    }
}
