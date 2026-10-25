use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}
pub fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
pub fn data_dir() -> PathBuf {
    home().join(".minicode")
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn is_link(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 && fs::read_link(path).is_ok()
    }
    #[cfg(not(windows))]
    false
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn canonical(path: &Path) -> Result<PathBuf> {
    // Resolve links before walking a parent component. Extended Windows paths
    // do not normalize '..' automatically, so none may reach the file APIs.
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in full.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !resolved.pop() {
                    bail!("path has no parent")
                }
            }
            std::path::Component::Normal(part) => {
                resolved.push(part);
                if fs::symlink_metadata(&resolved).is_ok() {
                    resolved = fs::canonicalize(&resolved)?;
                }
            }
            _ => resolved.push(component.as_os_str()),
        }
    }
    #[cfg(windows)]
    if let Some(raw) = resolved.to_str() {
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{unc}")));
        }
        if let Some(dos) = raw.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(dos));
        }
    }
    Ok(resolved)
}
pub fn in_workspace(workspace: &Path, raw: &str) -> Result<PathBuf> {
    if raw.contains('\0') {
        bail!("invalid path")
    }
    #[cfg(windows)]
    if raw.split(['/', '\\']).any(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        part.contains(':') && part.len() != 2
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
    }) {
        bail!("unsafe Windows path")
    }
    let root = canonical(workspace)?;
    let path = Path::new(raw);
    let target = canonical(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })?;
    if !target.starts_with(&root) {
        bail!("path escapes workspace: {raw}")
    }
    Ok(target)
}
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(content)?;
    file.as_file().sync_all()?;
    if let Ok(metadata) = path.metadata() {
        file.as_file().set_permissions(metadata.permissions())?;
    }
    // Windows briefly denies replacing a file that was itself just replaced
    // (scanners and the pending delete of the previous version hold it).
    let mut attempt = 0;
    loop {
        match file.persist(path) {
            Ok(_) => return Ok(()),
            Err(e)
                if cfg!(windows)
                    && attempt < 20
                    && e.error.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                attempt += 1;
                file = e.file;
                std::thread::sleep(std::time::Duration::from_millis(5 * attempt));
            }
            Err(e) => return Err(e.error.into()),
        }
    }
}
pub fn prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
pub fn terminal_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}
pub fn suffix(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}
pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let text = suffix(text, max_bytes);
    let lines: Vec<_> = text.split_inclusive('\n').rev().take(max_lines).collect();
    if text.is_empty() {
        String::new()
    } else {
        lines
            .into_iter()
            .rev()
            .collect::<String>()
            .trim_end_matches(['\r', '\n'])
            .to_string()
    }
}
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    "node_modules",
    ".minicode",
    ".pytest_cache",
    ".ruff_cache",
];
pub fn walk(root: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
        })
        .filter_map(Result::ok)
}
pub fn bundled_path() -> Option<std::ffi::OsString> {
    let runtime = std::env::var_os("MINICODE_RESOURCES")
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(p).join("runtime"))
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        })?;
    if !runtime.join("node").is_dir() {
        return None;
    }
    let mut dirs = vec![runtime.join("node"), runtime.join("git/cmd")];
    if let Some(existing) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(dirs).ok()
}
pub fn string<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .with_context(|| format!("{key} must be a string"))
}
/// Readable text of a caught panic payload.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}
