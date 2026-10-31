use crate::util;
use anyhow::{Result, bail};
use fs2::FileExt;
use serde_json::Value;
use std::{fs, path::Path};

pub fn update(path: &Path, change: impl FnOnce(&mut Value) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("settings path has no parent"))?;
    fs::create_dir_all(parent)?;
    if util::is_link(path) || util::is_link(parent) {
        bail!("refusing linked settings path")
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("json.lock"))?;
    lock.lock_exclusive()?;
    let mut value = if path.try_exists()? {
        let bytes = fs::read(path)?;
        serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))?
    } else {
        Value::Null
    };
    change(&mut value)?;
    util::atomic_write(path, &serde_json::to_vec_pretty(&value)?)
}
