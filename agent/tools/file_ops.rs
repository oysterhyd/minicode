use super::{ToolContext, bounded};
use crate::{
    editing,
    models::Outcome,
    textfile::{self, Encoding},
    util,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

/// What the session last saw of each file: the state after its latest read or
/// after its own successful edit / write. Edits are checked against it so a
/// change made outside the session is never silently overwritten.
#[derive(Default)]
pub struct FileLedger {
    seen: HashMap<PathBuf, Seen>,
    locks: HashMap<PathBuf, Arc<std::sync::Mutex<()>>>,
}
pub type Files = Arc<std::sync::Mutex<FileLedger>>;
#[derive(Clone)]
struct Seen {
    size: u64,
    modified: Option<SystemTime>,
    digest: String,
}
impl Seen {
    fn matches(&self, metadata: &fs::Metadata) -> bool {
        self.size == metadata.len() && self.modified == metadata.modified().ok()
    }
    /// Always hash the bytes: coarse or restored mtimes cannot prove identity.
    fn of(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            digest: format!("{:x}", sha2::Digest::finalize(hasher)),
        })
    }
}
fn ledger_key(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}
pub(super) fn remember(ctx: &ToolContext, path: &Path) {
    if let Ok(state) = Seen::of(path) {
        ctx.files
            .lock()
            .unwrap()
            .seen
            .insert(ledger_key(path), state);
    }
}
/// Refuses to modify an existing file the session has not read, or one that
/// changed on disk since the session last saw it. New files are unrestricted.
fn ensure_current(ctx: &ToolContext, path: &Path, relative: &str) -> Result<Option<Seen>> {
    if !path.exists() {
        return Ok(None);
    }
    let Some(previous) = ctx
        .files
        .lock()
        .unwrap()
        .seen
        .get(&ledger_key(path))
        .cloned()
    else {
        bail!(
            "{relative} 已存在，但本会话还没有读取过它（会话恢复后也需要重新读取）；请先 read 当前内容再修改"
        )
    };
    let current = Seen::of(path)?;
    if current.digest != previous.digest {
        bail!("{relative} 在上次读取后已被外部修改；请重新 read 后再修改")
    }
    Ok(Some(current))
}
pub(super) fn observed_path(ctx: &ToolContext, path: &Path) -> Option<String> {
    let path = util::canonical(path).ok()?;
    let relative = portable_path(path.strip_prefix(&ctx.workspace).ok()?.to_str()?);
    Some(if relative.is_empty() {
        ".".into()
    } else {
        relative
    })
}
fn portable_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}
pub(super) fn read_file(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let raw = util::string(args, "path")?;
    let target = util::in_workspace(&ctx.workspace, raw)?;
    if !target.is_file() {
        bail!("path is not a file: {raw}")
    }
    if target.extension().is_some_and(|s| {
        ["png", "jpg", "jpeg", "gif", "webp"].contains(&s.to_string_lossy().to_lowercase().as_str())
    }) {
        if !ctx.modalities.iter().any(|s| s == "image") {
            bail!(
                "The current model has no enabled image input capability; enable it in model settings or switch models."
            )
        }
        if args["offset"].as_u64().unwrap_or(1) != 1
            || !args["limit"].is_null()
            || !args["cursor"].is_null()
        {
            bail!("Image reads do not accept text pagination arguments.")
        }
        let image = ctx.attachments.import(&target)?;
        let mut o = Outcome::ok(format!("Read image: {raw}"));
        o.images.push(image);
        return Ok(o);
    }
    let extension = target
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if extension == "pdf" || extension == "ipynb" {
        let bytes = textfile::read_bounded(&target)?;
        let text = if extension == "pdf" {
            let text =
                pdf_extract::extract_text_from_mem(&bytes).context("cannot extract PDF text")?;
            if text.trim().is_empty() {
                "(PDF has no extractable text layer; OCR is not supported)".into()
            } else {
                text.trim().to_string()
            }
        } else {
            let encoding = Encoding::probe(&target)?;
            textfile::notebook(encoding.decode(&bytes)?.as_bytes())?
        };
        return read_text_page(BufReader::new(std::io::Cursor::new(text)), args, ctx);
    }
    let encoding = Encoding::probe(&target)?;
    if !matches!(encoding, Encoding::Utf8(_)) {
        let text = encoding.decode(&textfile::read_bounded(&target)?)?;
        return read_text_page(BufReader::new(std::io::Cursor::new(text)), args, ctx);
    }
    let mut reader = BufReader::new(File::open(&target)?);
    if encoding == Encoding::Utf8(true) && args["cursor"].as_u64().unwrap_or(0) == 0 {
        reader.seek(SeekFrom::Start(3))?;
    }
    read_text_page(reader, args, ctx)
}

fn read_text_page(
    mut reader: impl BufRead + Seek,
    args: &Value,
    ctx: &ToolContext,
) -> Result<Outcome> {
    let offset = args["offset"].as_u64().unwrap_or(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(2000).min(2000) as usize;
    if reader.fill_buf()?.is_empty() && offset == 1 && args["cursor"].as_u64().unwrap_or(0) == 0 {
        return Ok(Outcome::ok("(empty file)"));
    }
    let mut line_number = offset;
    let initial = reader.stream_position()?;
    if let Some(cursor) = args["cursor"].as_u64() {
        reader.seek(SeekFrom::Start(cursor.max(initial)))?;
    } else {
        let mut skipped = 0;
        while skipped < offset.saturating_sub(1) {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                bail!("offset {offset} beyond end of file ({skipped} lines)")
            }
            let mut cr = false;
            let size = if let Some(pos) = buf.iter().position(|b| *b == b'\n' || *b == b'\r') {
                cr = buf[pos] == b'\r';
                skipped += 1;
                pos + 1
            } else {
                buf.len()
            };
            reader.consume(size);
            if cr && reader.fill_buf()?.first() == Some(&b'\n') {
                reader.consume(1);
            }
            if ctx.cancel.is_cancelled() {
                bail!("tool cancelled")
            }
        }
    }
    let mut budget = 50 * 1024 - 200;
    let mut lines = vec![];
    let mut bytes = vec![];
    while lines.len() < limit && budget > 8 {
        let prefix = format!("{line_number}\t");
        let allowed = budget - prefix.len() - 1;
        let start = reader.stream_position()?;
        bytes.clear();
        let mut complete = false;
        while bytes.len() < allowed {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let n = buf
                .iter()
                .position(|b| *b == b'\n' || *b == b'\r')
                .map(|p| p + 1)
                .unwrap_or(buf.len())
                .min(allowed - bytes.len());
            let cr = buf[n - 1] == b'\r';
            complete = buf[n - 1] == b'\n' || cr;
            bytes.extend_from_slice(&buf[..n]);
            reader.consume(n);
            if cr && reader.fill_buf()?.first() == Some(&b'\n') {
                reader.consume(1);
            }
            if complete {
                break;
            }
        }
        if bytes.is_empty() {
            break;
        }
        let valid = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(e) if e.error_len().is_none() => {
                let fitted = &bytes[..e.valid_up_to()];
                reader.seek(SeekFrom::Start(start + fitted.len() as u64))?;
                std::str::from_utf8(fitted)?
            }
            Err(_) => bail!("binary or non-UTF-8 file"),
        };
        if valid.contains('\0') {
            bail!("binary or non-UTF-8 file")
        }
        let line = prefix + valid.trim_end_matches(['\r', '\n']);
        budget -= line.len() + 1;
        lines.push(line);
        if complete { line_number += 1 } else { break }
    }
    if lines.is_empty() {
        bail!(
            "offset {offset} beyond end of file ({})",
            offset.saturating_sub(1)
        )
    }
    let cursor = reader.stream_position()?;
    let more = !reader.fill_buf()?.is_empty();
    let mut text = lines.join("\n");
    if more {
        text.push_str(&format!(
            "\n...[更多内容：next_offset={line_number}, next_cursor={cursor}]"
        ));
    }
    let mut outcome = Outcome::ok(text);
    outcome.truncated = more;
    Ok(outcome)
}
pub(super) fn list(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let path = args["path"].as_str().unwrap_or(".");
    let base = util::in_workspace(&ctx.workspace, path)?;
    if !base.is_dir() {
        bail!("path is not a directory: {path}")
    }
    let recursive = args["recursive"] == true;
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(500).min(500) as usize;
    let mut entries = vec![];
    let mut observed = observed_path(ctx, &base).into_iter().collect::<Vec<_>>();
    let mut seen = 0;
    let mut used = 0;
    let mut more = false;
    let mut dirs = vec![base.clone()];
    'walk: while let Some(dir) = dirs.pop() {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        let mut children = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        children.sort_by_key(|e| e.file_name());
        let mut subdirs = vec![];
        let mut files = vec![];
        for e in children {
            if matches!(
                ctx.permissions
                    .path_decision(&ctx.workspace, "ls", &e.path().to_string_lossy())
                    .unwrap_or(Some(crate::permissions::Decision::Deny)),
                Some(crate::permissions::Decision::Deny)
            ) {
                continue;
            }
            let kind = e.file_type()?;
            if kind.is_dir() {
                if util::SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()) {
                    continue;
                }
                if !util::is_link(&e.path()) {
                    subdirs.push(e.path());
                }
            } else {
                files.push(e.path());
            }
        }
        for entry in subdirs
            .iter()
            .map(|p| (p, true))
            .chain(files.iter().map(|p| (p, false)))
        {
            let relative = portable_path(&entry.0.strip_prefix(&base)?.to_string_lossy());
            let text = if entry.1 {
                format!("{relative}/")
            } else {
                relative
            };
            if seen >= offset {
                if entries.len() >= limit || used + text.len() + 1 > 50 * 1024 - 100 {
                    more = true;
                    break 'walk;
                }
                used += text.len() + 1;
                entries.push(text);
                if let Some(path) = observed_path(ctx, entry.0) {
                    observed.push(path);
                }
            }
            seen += 1;
        }
        if recursive {
            dirs.extend(subdirs.into_iter().rev());
        } else {
            break;
        }
    }
    entries.sort();
    let mut text = entries.join("\n");
    if recursive && text.len() > 8192 {
        let mut rendered = vec![];
        let mut previous: Vec<&str> = vec![];
        for entry in &entries {
            let parts = entry.trim_end_matches('/').split('/').collect::<Vec<_>>();
            let n = if entry.ends_with('/') {
                parts.len()
            } else {
                parts.len() - 1
            };
            let common = previous
                .iter()
                .zip(parts[..n].iter())
                .take_while(|(a, b)| a == b)
                .count();
            for (depth, part) in parts.iter().enumerate().take(n).skip(common) {
                rendered.push(format!("{}{part}/", "  ".repeat(depth)));
            }
            if n < parts.len() {
                rendered.push(format!("{}{}", "  ".repeat(n), parts[n]));
            }
            previous = parts[..n].to_vec();
        }
        text = format!("目录树（缩进表示路径层级）：\n{}", rendered.join("\n"));
    }
    if more {
        text.push_str(&format!(
            "\n...[达到列表页上限；next_offset={}]",
            offset + entries.len()
        ));
    }
    let mut outcome = bounded(text);
    outcome.truncated |= more;
    outcome.observed_paths = observed;
    Ok(outcome)
}
pub(super) fn grep_files(
    base: &Path,
    include_ignored: bool,
) -> impl Iterator<Item = ignore::DirEntry> {
    ignore::WalkBuilder::new(base)
        .require_git(false)
        .standard_filters(!include_ignored)
        .hidden(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(move |e| {
            e.depth() == 0
                || !e.file_type().is_some_and(|t| t.is_dir())
                || !util::SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
        })
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
}
pub(super) fn grep(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let pattern = util::string(args, "pattern")?;
    let pattern = if args.get("case_sensitive").is_some_and(|v| v == false) {
        format!("(?i){pattern}")
    } else {
        pattern.into()
    };
    let regex = regex::RegexBuilder::new(&pattern)
        .build()
        .map_err(|error| anyhow::anyhow!("invalid regex: {error}"))?;
    let raw = args["path"].as_str().unwrap_or(".");
    let base = util::in_workspace(&ctx.workspace, raw)?;
    if !base.is_dir() && !base.is_file() {
        bail!("path is not a file or directory: {raw}")
    }
    let glob = args["glob"]
        .as_str()
        .map(globset::Glob::new)
        .transpose()?
        .map(|g| g.compile_matcher());
    let max = args["max_results"].as_i64().unwrap_or(100).clamp(1, 1000) as usize;
    let mut lines = vec![];
    let observed = std::cell::RefCell::new(std::collections::BTreeSet::new());
    let mut skipped = 0;
    let truncated = std::cell::Cell::new(false);
    let start = Instant::now();
    let search_line = |line: &str,
                       number: &mut usize,
                       workspace_path: &str,
                       actual_path: Option<&str>,
                       lines: &mut Vec<String>|
     -> Result<bool> {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        if start.elapsed().as_secs() > 30 {
            bail!("search timed out; narrow path or glob and retry")
        }
        *number += 1;
        if regex.is_match(line) {
            if let Some(path) = actual_path {
                observed.borrow_mut().insert(path.to_string());
            }
            let mut chars = line.chars();
            let snippet = chars.by_ref().take(500).collect::<String>();
            let shortened = chars.next().is_some();
            truncated.set(truncated.get() || shortened);
            lines.push(format!(
                "{workspace_path}:{number}: {snippet}{}",
                if shortened { "...[行已截断]" } else { "" }
            ));
            if lines.len() >= max {
                truncated.set(true);
                lines.push("...[达到结果上限；请缩小 path 或 glob 继续搜索]".into());
                return Ok(true);
            }
        }
        Ok(false)
    };
    let files = grep_files(&base, args["include_ignored"] == true);
    'files: for entry in files {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        if start.elapsed().as_secs() > 30 {
            bail!("search timed out; narrow path or glob and retry")
        }
        let path = entry.path();
        if matches!(
            ctx.permissions
                .path_decision(&ctx.workspace, "grep", &path.to_string_lossy())
                .unwrap_or(Some(crate::permissions::Decision::Deny)),
            Some(crate::permissions::Decision::Deny)
        ) {
            continue;
        }
        let relative = if base.is_file() {
            base.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            portable_path(&path.strip_prefix(&base)?.to_string_lossy())
        };
        if glob.as_ref().is_some_and(|g| {
            !g.is_match(&relative) && !g.is_match(path.file_name().unwrap_or_default())
        }) {
            continue;
        }
        if util::is_link(path) {
            continue;
        }
        // Buffered reads, never a mapping: a file truncated while mapped
        // faults the whole process (SIGBUS / in-page error).
        let mut reader = match File::open(path) {
            Ok(file) => BufReader::new(file),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let mut head = vec![];
        if reader.by_ref().take(8192).read_to_end(&mut head).is_err() {
            skipped += 1;
            continue;
        }
        if head.is_empty() || head.contains(&0) {
            continue;
        }
        let mut reader = BufReader::new(std::io::Cursor::new(head).chain(reader));
        let relative = path.strip_prefix(&ctx.workspace)?;
        let workspace_path = portable_path(&relative.to_string_lossy());
        // The walker never follows links. Record its actual path only for
        // matching files, and leave canonical comparison to the verifier.
        let actual_path = relative.to_str().map(portable_path);
        let mut number = 0;
        let mut chunk = vec![];
        let mut ended_with_lf = false;
        loop {
            chunk.clear();
            match reader.read_until(b'\n', &mut chunk) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => {
                    skipped += 1;
                    continue 'files;
                }
            }
            ended_with_lf = chunk.last() == Some(&b'\n');
            let text = String::from_utf8_lossy(&chunk);
            let body = text.strip_suffix('\n').unwrap_or(&text);
            let mut pieces = body
                .split([
                    '\r', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}', '\u{2029}',
                ])
                .collect::<Vec<_>>();
            // CRLF counts as one line, as in the original universal-newline reader.
            if ended_with_lf && body.ends_with('\r') {
                pieces.pop();
            }
            for line in pieces {
                if search_line(
                    line,
                    &mut number,
                    &workspace_path,
                    actual_path.as_deref(),
                    &mut lines,
                )? {
                    break 'files;
                }
            }
        }
        // Text that ends with a line break has one more, empty, line.
        if ended_with_lf
            && search_line(
                "",
                &mut number,
                &workspace_path,
                actual_path.as_deref(),
                &mut lines,
            )?
        {
            break 'files;
        }
    }
    if lines.is_empty() {
        lines.push("(no matches)".into());
    }
    if skipped > 0 {
        lines.push(format!("...[跳过 {skipped} 个无法读取的文件]"));
    }
    let mut outcome = bounded(lines.join("\n"));
    outcome.truncated |= truncated.get();
    outcome.observed_paths = observed.into_inner().into_iter().collect();
    Ok(outcome)
}
fn lexical_join(base: &Path, raw: &str) -> PathBuf {
    let combined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        base.join(raw)
    };
    let mut resolved = PathBuf::new();
    for component in combined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::Normal(part) => resolved.push(part),
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved
}

fn initial_lock_key(workspace: &Path, raw: &str) -> Result<PathBuf> {
    if raw.contains('\0') {
        bail!("invalid path")
    }
    let root = util::canonical(workspace)?;
    let lex = lexical_join(&root, raw);
    // Canonicalize only the parent directory so concurrent ReplaceFileW calls
    // on the leaf file cannot race with path canonicalization before locking.
    let key = if let (Some(parent), Some(name)) = (lex.parent(), lex.file_name()) {
        if let Ok(canon_parent) = util::canonical(parent) {
            canon_parent.join(name)
        } else {
            lex
        }
    } else {
        lex
    };
    Ok(ledger_key(&key))
}

fn lock_for(ctx: &ToolContext, key: &Path) -> Arc<std::sync::Mutex<()>> {
    ctx.files
        .lock()
        .unwrap()
        .locks
        .entry(key.to_path_buf())
        .or_default()
        .clone()
}

fn large_edit_max_bytes() -> u64 {
    std::env::var("MINICODE_LARGE_EDIT_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|v| (256_000..=256 * 1024 * 1024).contains(v))
        .unwrap_or(64 * 1024 * 1024)
}

pub(super) fn edit(name: &str, args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let raw = util::string(args, "path")?;
    // Mutations of one file are serialized before resolving the leaf file, so
    // concurrent edits neither race on the same original nor hit transient
    // replacement paths on Windows.
    let initial_key = initial_lock_key(&ctx.workspace, raw)?;
    let first_lock = lock_for(ctx, &initial_key);
    let first_guard = first_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut path = util::in_workspace(&ctx.workspace, raw)?;
    let resolved_key = ledger_key(&path);
    let (low_lock, high_lock);
    let (_primary_guard, _secondary_guard) = if resolved_key != initial_key {
        drop(first_guard);
        let (low, high) = if initial_key <= resolved_key {
            (&initial_key, &resolved_key)
        } else {
            (&resolved_key, &initial_key)
        };
        low_lock = lock_for(ctx, low);
        high_lock = lock_for(ctx, high);
        let low_guard = low_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let high_guard = high_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        path = util::in_workspace(&ctx.workspace, raw)?;
        (low_guard, Some(high_guard))
    } else {
        (first_guard, None)
    };
    let relative = path
        .strip_prefix(&ctx.workspace)?
        .to_string_lossy()
        .replace('\\', "/");
    let mut outcome = edit_locked(name, args, ctx, &path, &relative)?;
    if outcome.success
        && let Some(observed) = observed_path(ctx, &path)
    {
        outcome.observed_paths.push(observed);
    }
    Ok(outcome)
}

fn edit_locked(
    name: &str,
    args: &Value,
    ctx: &ToolContext,
    path: &Path,
    relative: &str,
) -> Result<Outcome> {
    if name == "edit" && !path.is_file() {
        bail!("path is not a file: {relative} (use write to create a new file)")
    }
    if path.exists() && !path.is_file() {
        bail!("path exists and is not a file: {relative}")
    }
    let seen = ensure_current(ctx, path, relative)?;
    let size = if path.exists() {
        path.metadata()?.len()
    } else {
        0
    };
    let encoding = if path.exists() {
        Encoding::probe(path)?
    } else {
        Encoding::Utf8(false)
    };
    if name == "edit" && size > 256_000 && matches!(encoding, Encoding::Utf8(_)) {
        let outcome = edit_large(path, relative, args, ctx, seen)?;
        remember(ctx, path);
        return Ok(outcome);
    }
    let before = if path.exists() && (size <= 256_000 || !matches!(encoding, Encoding::Utf8(_))) {
        encoding.decode(&textfile::read_bounded(path)?)?
    } else {
        String::new()
    };
    let after = if name == "write" {
        util::string(args, "content")?.to_string()
    } else {
        editing::apply(&before, &editing::plan(&before, args, relative)?)
    };
    if ctx.cancel.is_cancelled() {
        bail!("tool cancelled")
    }
    let encoded = encoding.encode(&after)?;
    util::atomic_write(path, &encoded)?;
    if let Ok(metadata) = path.metadata() {
        ctx.files.lock().unwrap().seen.insert(
            ledger_key(path),
            Seen {
                size: metadata.len(),
                modified: metadata.modified().ok(),
                digest: util::hash(&encoded),
            },
        );
    }
    if size > 256_000 || after.len() > 256_000 {
        return Ok(Outcome::ok(format!(
            "Applied write to {relative}; old={size} bytes, new={} bytes. Large diff omitted; use read pages to inspect.",
            after.len()
        )));
    }
    let lines = |text: &str| {
        let text = text.replace("\r\n", "\n");
        let rows = text
            .split_terminator([
                '\n', '\r', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}',
                '\u{2029}',
            ])
            .collect::<Vec<_>>();
        if rows.is_empty() {
            String::new()
        } else {
            rows.join("\n") + "\n"
        }
    };
    let before_lines = lines(&before);
    let after_lines = lines(&after);
    let diff = similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Patience)
        .timeout(Duration::from_secs(1))
        .diff_lines(&before_lines, &after_lines);
    let adds = diff
        .iter_all_changes()
        .filter(|c| c.tag() == similar::ChangeTag::Insert)
        .count();
    let dels = diff
        .iter_all_changes()
        .filter(|c| c.tag() == similar::ChangeTag::Delete)
        .count();
    let body = diff
        .unified_diff()
        .header(&format!("a/{relative}"), &format!("b/{relative}"))
        .missing_newline_hint(false)
        .to_string();
    Ok(bounded(format!(
        "Applied change to {relative} (+{adds} -{dels})\n{body}"
    )))
}
fn edit_large(
    path: &Path,
    relative: &str,
    args: &Value,
    ctx: &ToolContext,
    seen: Option<Seen>,
) -> Result<Outcome> {
    let max_bytes = large_edit_max_bytes();
    let input = File::open(path)?;
    let original = input.metadata()?;
    if original.len() > max_bytes {
        bail!(
            "{relative} exceeds the large-edit limit ({} > {max_bytes} bytes; MINICODE_LARGE_EDIT_MAX_BYTES)",
            original.len()
        )
    }
    let seen = seen.context("large file is not in the read ledger")?;
    if !seen.matches(&original) {
        bail!("{relative} 在上次读取后已被外部修改；请重新 read 后再修改")
    }
    let mut bytes = Vec::with_capacity(original.len().min(max_bytes) as usize);
    input.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        bail!(
            "{relative} exceeds the large-edit limit ({} > {max_bytes} bytes; MINICODE_LARGE_EDIT_MAX_BYTES)",
            bytes.len()
        )
    }
    if util::hash(&bytes) != seen.digest {
        bail!("{relative} 在上次读取后已被外部修改；请重新 read 后再修改")
    }
    let bom = bytes.starts_with(b"\xef\xbb\xbf");
    let body = if bom { &bytes[3..] } else { &bytes[..] };
    let text = std::str::from_utf8(body).context("binary or non-UTF-8 file")?;
    if text.contains('\0') {
        bail!("binary file contains NUL characters");
    }
    let changes = editing::plan(text, args, relative)?;
    let parent = path.parent().context("invalid file path")?;
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = std::io::BufWriter::new(temp);
    if bom {
        writer.write_all(b"\xef\xbb\xbf")?;
    }
    let mut position = 0;
    let replacements = changes.len();
    for change in changes {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        writer.write_all(&body[position..change.start])?;
        writer.write_all(change.replacement.as_bytes())?;
        position = change.end;
    }
    writer.write_all(&body[position..])?;
    writer.flush()?;
    let temp = writer.into_inner().map_err(|e| e.into_error())?;
    if Seen::of(path)?.digest != seen.digest {
        bail!("{relative} 在编辑过程中被外部修改；请重新 read 后再修改")
    }
    if ctx.cancel.is_cancelled() {
        bail!("tool cancelled")
    }
    util::atomic_persist(temp, path)?;
    Ok(Outcome::ok(format!(
        "Applied change to large file {relative} ({replacements} replacement(s)); old={} bytes, new={} bytes.",
        original.len(),
        path.metadata()?.len()
    )))
}
