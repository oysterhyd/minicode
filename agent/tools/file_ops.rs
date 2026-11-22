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
    collections::{BTreeSet, HashMap, VecDeque},
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    ops::ControlFlow,
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
fn path_denied(ctx: &ToolContext, tool: &str, path: &Path) -> bool {
    matches!(
        ctx.permissions
            .path_decision(&ctx.workspace, tool, &path.to_string_lossy())
            .unwrap_or(Some(crate::permissions::Decision::Deny)),
        Some(crate::permissions::Decision::Deny)
    )
}

pub(super) fn list(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let path = args["path"].as_str().unwrap_or(".");
    let base = util::in_workspace(&ctx.workspace, path)?;
    if !base.is_dir() {
        bail!("path is not a directory: {path}")
    }
    let recursive = args["recursive"] == true;
    let include_ignored = args["include_ignored"] == true;
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(500).min(500) as usize;
    let mut entries = vec![];
    let mut observed = observed_path(ctx, &base).into_iter().collect::<Vec<_>>();
    let mut seen = 0;
    let mut used = 0;
    let mut more = false;
    if recursive {
        let permissions = ctx.permissions.clone();
        let workspace = ctx.workspace.clone();
        let walker = ignore::WalkBuilder::new(&base)
            .require_git(false)
            .standard_filters(!include_ignored)
            .hidden(false)
            .sort_by_file_name(|a, b| a.cmp(b))
            .filter_entry(move |e| {
                if e.depth() == 0 {
                    return true;
                }
                if e.file_type().is_some_and(|t| t.is_dir())
                    && (util::SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
                        || util::is_link(e.path()))
                {
                    return false;
                }
                !matches!(
                    permissions
                        .path_decision(&workspace, "ls", &e.path().to_string_lossy())
                        .unwrap_or(Some(crate::permissions::Decision::Deny)),
                    Some(crate::permissions::Decision::Deny)
                )
            })
            .build();
        for entry in walker.filter_map(Result::ok) {
            if ctx.cancel.is_cancelled() {
                bail!("tool cancelled")
            }
            if entry.depth() == 0 {
                continue;
            }
            let entry_path = entry.path();
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            let relative = portable_path(&entry_path.strip_prefix(&base)?.to_string_lossy());
            let text = if is_dir {
                format!("{relative}/")
            } else {
                relative
            };
            if seen >= offset {
                if entries.len() >= limit || used + text.len() + 1 > 50 * 1024 - 100 {
                    more = true;
                    break;
                }
                used += text.len() + 1;
                entries.push(text);
                if let Some(path) = observed_path(ctx, entry_path) {
                    observed.push(path);
                }
            }
            seen += 1;
        }
    } else {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        let mut children = fs::read_dir(&base)?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        children.sort_by_key(|e| e.file_name());
        let mut subdirs = vec![];
        let mut files = vec![];
        for e in children {
            if path_denied(ctx, "ls", &e.path()) {
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
                    break;
                }
                used += text.len() + 1;
                entries.push(text);
                if let Some(path) = observed_path(ctx, entry.0) {
                    observed.push(path);
                }
            }
            seen += 1;
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
                || (!util::SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
                    && !util::is_link(e.path()))
        })
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
}
pub(super) fn glob(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let raw_pattern = util::string(args, "pattern")?;
    let normalized = raw_pattern.replace('\\', "/");
    let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
    if normalized.is_empty() {
        bail!("pattern must not be empty")
    }
    let has_sep = normalized.contains('/');
    let matcher = globset::GlobBuilder::new(normalized)
        .literal_separator(true)
        .build()
        .map_err(|error| anyhow::anyhow!("invalid glob pattern: {error}"))?
        .compile_matcher();
    let raw_path = args["path"].as_str().unwrap_or(".");
    let base = util::in_workspace(&ctx.workspace, raw_path)?;
    if !base.is_dir() {
        bail!("path is not a directory: {raw_path}")
    }
    if path_denied(ctx, "glob", &base) || path_denied(ctx, "ls", &base) {
        bail!("path denied by permission rules: {raw_path}")
    }
    let limit = args["limit"].as_i64().unwrap_or(200).clamp(1, 1000) as usize;
    let include_ignored = args["include_ignored"] == true;
    let start = Instant::now();
    let mut matches: Vec<(Option<SystemTime>, String, Option<String>)> = vec![];
    for entry in grep_files(&base, include_ignored) {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        if start.elapsed().as_secs() > 30 {
            bail!("glob timed out; narrow path or pattern and retry")
        }
        let path = entry.path();
        if util::is_link(path) {
            continue;
        }
        if path_denied(ctx, "glob", path) || path_denied(ctx, "ls", path) {
            continue;
        }
        let match_rel = portable_path(&path.strip_prefix(&base)?.to_string_lossy());
        let matched = matcher.is_match(&match_rel)
            || (!has_sep && path.file_name().is_some_and(|name| matcher.is_match(name)));
        if !matched {
            continue;
        }
        let relative = path.strip_prefix(&ctx.workspace)?;
        let workspace_path = portable_path(&relative.to_string_lossy());
        let actual_path = relative.to_str().map(portable_path);
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
        matches.push((modified, workspace_path, actual_path));
    }
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = matches.len();
    let truncated = total > limit;
    let mut selected = Vec::with_capacity(total.min(limit));
    let mut observed = Vec::with_capacity(total.min(limit));
    for (_, workspace_path, actual_path) in matches.into_iter().take(limit) {
        selected.push(workspace_path);
        if let Some(path) = actual_path {
            observed.push(path);
        }
    }
    let text = if selected.is_empty() {
        "(no matches)".to_string()
    } else {
        let mut text = selected.join("\n");
        if truncated {
            text.push_str(&format!(
                "\n...[达到结果上限（共 {total} 个匹配，仅显示最近修改的 {limit} 个）；请缩小 path 或 pattern 继续搜索]"
            ));
        }
        text
    };
    let mut outcome = bounded(text);
    outcome.truncated |= truncated;
    outcome.observed_paths = observed;
    Ok(outcome)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GrepOutputMode {
    Content,
    FilesWithMatches,
    Count,
}

fn format_grep_line(
    workspace_path: &str,
    number: usize,
    sep: char,
    line: &str,
    truncated: &mut bool,
) -> String {
    let mut chars = line.chars();
    let snippet = chars.by_ref().take(500).collect::<String>();
    let shortened = chars.next().is_some();
    *truncated |= shortened;
    format!(
        "{workspace_path}{sep}{number}{sep} {snippet}{}",
        if shortened { "...[行已截断]" } else { "" }
    )
}

fn for_each_file_line(
    mut reader: impl BufRead,
    ctx: &ToolContext,
    start: Instant,
    mut on_line: impl FnMut(usize, &str, bool) -> Result<ControlFlow<()>>,
) -> Result<bool> {
    let mut number = 0;
    let mut chunk = vec![];
    let mut ended_with_lf = false;
    loop {
        chunk.clear();
        match reader.read_until(b'\n', &mut chunk) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return Ok(false),
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
            if ctx.cancel.is_cancelled() {
                bail!("tool cancelled")
            }
            if start.elapsed().as_secs() > 30 {
                bail!("search timed out; narrow path or glob and retry")
            }
            number += 1;
            if matches!(on_line(number, line, false)?, ControlFlow::Break(())) {
                return Ok(true);
            }
        }
    }
    // Text that ends with a line break has one more, empty, line.
    if ended_with_lf {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        if start.elapsed().as_secs() > 30 {
            bail!("search timed out; narrow path or glob and retry")
        }
        number += 1;
        let _ = on_line(number, "", true)?;
    }
    Ok(true)
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
    let output_mode = match args.get("output_mode") {
        None | Some(Value::Null) => GrepOutputMode::Content,
        Some(Value::String(s)) => match s.as_str() {
            "content" => GrepOutputMode::Content,
            "files_with_matches" => GrepOutputMode::FilesWithMatches,
            "count" => GrepOutputMode::Count,
            other => bail!("invalid output_mode: {other}"),
        },
        Some(_) => bail!("output_mode must be a string"),
    };
    let context_lines = args["context_lines"].as_u64().unwrap_or(0).min(10) as usize;
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
    let mut observed = BTreeSet::new();
    let mut skipped = 0;
    let mut truncated = false;
    let mut matches_found = 0usize;
    let start = Instant::now();
    let files = grep_files(&base, args["include_ignored"] == true);
    'files: for entry in files {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        if start.elapsed().as_secs() > 30 {
            bail!("search timed out; narrow path or glob and retry")
        }
        let path = entry.path();
        if path_denied(ctx, "grep", path) {
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
        let reader = BufReader::new(std::io::Cursor::new(head).chain(reader));
        let relative = path.strip_prefix(&ctx.workspace)?;
        let workspace_path = portable_path(&relative.to_string_lossy());
        // The walker never follows links. Record its actual path only for
        // matching files, and leave canonical comparison to the verifier.
        let actual_path = relative.to_str().map(portable_path);
        match output_mode {
            GrepOutputMode::FilesWithMatches => {
                let mut stop_all = false;
                let read_ok = for_each_file_line(reader, ctx, start, |_, line, _| {
                    if regex.is_match(line) {
                        if let Some(path) = actual_path.as_deref() {
                            observed.insert(path.to_string());
                        }
                        lines.push(workspace_path.clone());
                        if lines.len() >= max {
                            truncated = true;
                            lines.push("...[达到结果上限；请缩小 path 或 glob 继续搜索]".into());
                            stop_all = true;
                        }
                        return Ok(ControlFlow::Break(()));
                    }
                    Ok(ControlFlow::Continue(()))
                })?;
                if !read_ok {
                    skipped += 1;
                    continue 'files;
                }
                if stop_all {
                    break 'files;
                }
            }
            GrepOutputMode::Count => {
                let mut file_matches = 0usize;
                let read_ok = for_each_file_line(reader, ctx, start, |_, line, _| {
                    if regex.is_match(line) {
                        file_matches += 1;
                    }
                    Ok(ControlFlow::Continue(()))
                })?;
                if !read_ok {
                    skipped += 1;
                    continue 'files;
                }
                if file_matches > 0 {
                    if let Some(path) = actual_path.as_deref() {
                        observed.insert(path.to_string());
                    }
                    lines.push(format!("{workspace_path}: {file_matches}"));
                    if lines.len() >= max {
                        truncated = true;
                        lines.push("...[达到结果上限；请缩小 path 或 glob 继续搜索]".into());
                        break 'files;
                    }
                }
            }
            GrepOutputMode::Content => {
                let mut before: VecDeque<(usize, String)> = VecDeque::with_capacity(context_lines);
                let mut after_remaining = 0usize;
                let mut last_emitted_line: Option<usize> = None;
                let mut limit_hit_in_file = false;
                let mut stop_all = false;
                let read_ok =
                    for_each_file_line(reader, ctx, start, |number, line, is_trailing_empty| {
                        let matched = regex.is_match(line);
                        if is_trailing_empty && !matched {
                            return Ok(ControlFlow::Continue(()));
                        }
                        if limit_hit_in_file {
                            if !matched && after_remaining > 0 {
                                lines.push(format_grep_line(
                                    &workspace_path,
                                    number,
                                    '-',
                                    line,
                                    &mut truncated,
                                ));
                                after_remaining -= 1;
                                if after_remaining == 0 {
                                    return Ok(ControlFlow::Break(()));
                                }
                                return Ok(ControlFlow::Continue(()));
                            }
                            return Ok(ControlFlow::Break(()));
                        }
                        if matched {
                            if let Some(path) = actual_path.as_deref() {
                                observed.insert(path.to_string());
                            }
                            if context_lines > 0 {
                                let first_line = before.front().map(|(n, _)| *n).unwrap_or(number);
                                if last_emitted_line.is_some_and(|prev| first_line > prev + 1) {
                                    lines.push("--".into());
                                }
                                for (bn, bl) in before.drain(..) {
                                    lines.push(format_grep_line(
                                        &workspace_path,
                                        bn,
                                        '-',
                                        &bl,
                                        &mut truncated,
                                    ));
                                }
                            }
                            lines.push(format_grep_line(
                                &workspace_path,
                                number,
                                ':',
                                line,
                                &mut truncated,
                            ));
                            last_emitted_line = Some(number);
                            after_remaining = context_lines;
                            matches_found += 1;
                            if matches_found >= max {
                                truncated = true;
                                stop_all = true;
                                if context_lines == 0 {
                                    return Ok(ControlFlow::Break(()));
                                }
                                limit_hit_in_file = true;
                            }
                        } else if context_lines > 0 {
                            if after_remaining > 0 {
                                lines.push(format_grep_line(
                                    &workspace_path,
                                    number,
                                    '-',
                                    line,
                                    &mut truncated,
                                ));
                                last_emitted_line = Some(number);
                                after_remaining -= 1;
                            } else {
                                if before.len() == context_lines {
                                    before.pop_front();
                                }
                                before.push_back((number, line.to_string()));
                            }
                        }
                        Ok(ControlFlow::Continue(()))
                    })?;
                if !read_ok {
                    skipped += 1;
                    continue 'files;
                }
                if stop_all {
                    lines.push("...[达到结果上限；请缩小 path 或 glob 继续搜索]".into());
                    break 'files;
                }
            }
        }
    }
    if lines.is_empty() {
        lines.push("(no matches)".into());
    }
    if skipped > 0 {
        lines.push(format!("...[跳过 {skipped} 个无法读取的文件]"));
    }
    let mut outcome = bounded(lines.join("\n"));
    outcome.truncated |= truncated;
    outcome.observed_paths = observed.into_iter().collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        attachments::Attachments, configuration::Configuration, extensions::Catalog,
        mcp::McpRegistry, permissions::Rules, store::Store, tools,
    };
    use serde_json::json;
    use std::collections::{BTreeMap, HashSet};
    use tokio_util::sync::CancellationToken;

    fn test_ctx(rules: Rules) -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = util::canonical(dir.path()).unwrap();
        let store = Store::open(workspace.join("test.db")).unwrap();
        let session = store
            .create(&workspace.to_string_lossy(), "fake", "fake")
            .unwrap();
        let catalog = Catalog {
            workspace: workspace.clone(),
            skills: BTreeMap::new(),
            plugins: vec![],
            servers: vec![],
            agents: BTreeMap::new(),
            local: vec![],
            instructions: BTreeMap::new(),
            active: BTreeMap::new(),
            pending_scopes: HashSet::new(),
            configuration: Configuration {
                path: workspace.join("config.json"),
            },
        };
        let ctx = ToolContext {
            permissions: Arc::new(rules),
            sandbox: Default::default(),
            force_sandbox: false,
            workspace: workspace.clone(),
            session,
            store,
            attachments: Attachments {
                root: workspace.join("attachments"),
            },
            modalities: vec!["image".into()],
            catalog: Arc::new(tokio::sync::Mutex::new(catalog)),
            mcp: Arc::new(McpRegistry::default()),
            cancel: CancellationToken::new(),
            output: None,
            deadline: None,
            files: Default::default(),
        };
        (dir, ctx)
    }

    fn write_with_mtime(path: &Path, content: &str, mtime: SystemTime) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
    }

    #[tokio::test]
    async fn glob_matches_patterns_sorts_by_mtime_and_tracks_observed_paths() {
        let (_dir, ctx) = test_ctx(Rules::default());
        let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        write_with_mtime(&ctx.workspace.join("root.rs"), "", base_time);
        write_with_mtime(
            &ctx.workspace.join("src/b.rs"),
            "",
            base_time + Duration::from_secs(10),
        );
        write_with_mtime(
            &ctx.workspace.join("src/a.rs"),
            "",
            base_time + Duration::from_secs(10),
        );
        write_with_mtime(
            &ctx.workspace.join("src/sub/deep.rs"),
            "",
            base_time + Duration::from_secs(20),
        );
        write_with_mtime(
            &ctx.workspace.join("src/user_auth.ts"),
            "",
            base_time + Duration::from_secs(30),
        );
        write_with_mtime(
            &ctx.workspace.join("auth/other.ts"),
            "",
            base_time + Duration::from_secs(40),
        );

        let all_rs = tools::execute("glob", &json!({"pattern": "**/*.rs"}), &ctx).await;
        assert!(all_rs.success, "{:?}", all_rs.error);
        assert!(!all_rs.truncated);
        assert_eq!(
            all_rs.output,
            "src/sub/deep.rs\nsrc/a.rs\nsrc/b.rs\nroot.rs"
        );
        assert_eq!(
            all_rs.observed_paths,
            vec!["src/sub/deep.rs", "src/a.rs", "src/b.rs", "root.rs"]
        );

        let auth_ts = tools::execute("glob", &json!({"pattern": "*auth*.ts"}), &ctx).await;
        assert!(auth_ts.success, "{:?}", auth_ts.error);
        assert_eq!(auth_ts.output, "src/user_auth.ts");
        assert_eq!(auth_ts.observed_paths, vec!["src/user_auth.ts"]);

        let shallow_rs = tools::execute("glob", &json!({"pattern": "src/*.rs"}), &ctx).await;
        assert!(shallow_rs.success, "{:?}", shallow_rs.error);
        assert_eq!(shallow_rs.output, "src/a.rs\nsrc/b.rs");

        let limited =
            tools::execute("glob", &json!({"pattern": "**/*.rs", "limit": 2}), &ctx).await;
        assert!(limited.success, "{:?}", limited.error);
        assert!(limited.truncated);
        assert!(
            limited
                .output
                .starts_with("src/sub/deep.rs\nsrc/a.rs\n...[达到结果上限")
        );
        assert_eq!(limited.observed_paths, vec!["src/sub/deep.rs", "src/a.rs"]);

        let none = tools::execute("glob", &json!({"pattern": "**/*.go"}), &ctx).await;
        assert!(none.success, "{:?}", none.error);
        assert_eq!(none.output, "(no matches)");
        assert!(none.observed_paths.is_empty());
    }

    #[tokio::test]
    async fn glob_respects_ignore_files_skip_dirs_and_permissions() {
        let mut rules = Rules::default();
        rules
            .extend(&json!({"deny": ["read(.env)", "glob(secret/**)"]}))
            .unwrap();
        let (_dir, ctx) = test_ctx(rules);

        fs::write(
            ctx.workspace.join(".gitignore"),
            "target/\n*.log\n!keep.log\n",
        )
        .unwrap();
        fs::write(ctx.workspace.join(".ignore"), "ignored.txt\n").unwrap();
        for rel in [
            "main.txt",
            "skip.log",
            "keep.log",
            "ignored.txt",
            "target/build.txt",
            "node_modules/pkg.txt",
            ".env",
            "secret/key.txt",
        ] {
            let p = ctx.workspace.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, "x").unwrap();
        }

        let filtered = tools::execute("glob", &json!({"pattern": "**/*"}), &ctx).await;
        assert!(filtered.success, "{:?}", filtered.error);
        let lines: Vec<&str> = filtered.output.lines().collect();
        assert!(lines.contains(&"main.txt"));
        assert!(lines.contains(&"keep.log"));
        assert!(!lines.contains(&"skip.log"));
        assert!(!lines.contains(&"ignored.txt"));
        assert!(!lines.contains(&"target/build.txt"));
        assert!(!lines.contains(&"node_modules/pkg.txt"));
        assert!(!lines.contains(&".env"));
        assert!(!lines.contains(&"secret/key.txt"));

        let override_ignored = tools::execute(
            "glob",
            &json!({"pattern": "**/*", "include_ignored": true}),
            &ctx,
        )
        .await;
        assert!(override_ignored.success, "{:?}", override_ignored.error);
        let lines: Vec<&str> = override_ignored.output.lines().collect();
        assert!(lines.contains(&"skip.log"));
        assert!(lines.contains(&"ignored.txt"));
        assert!(lines.contains(&"target/build.txt"));
        assert!(!lines.contains(&"node_modules/pkg.txt"));
        assert!(!lines.contains(&".env"));
        assert!(!lines.contains(&"secret/key.txt"));
    }

    #[tokio::test]
    async fn grep_supports_files_with_matches_count_and_context_lines() {
        let (_dir, ctx) = test_ctx(Rules::default());
        fs::write(
            ctx.workspace.join("a.txt"),
            "zero\none\nhit1\ntwo\nhit2\nthree\nfour\nfive\nhit3\nsix\n",
        )
        .unwrap();
        fs::write(ctx.workspace.join("b.txt"), "hit only\n").unwrap();
        fs::write(ctx.workspace.join("c.txt"), "nothing here\n").unwrap();
        fs::write(ctx.workspace.join("bin.dat"), b"hit\0binary\n").unwrap();

        let files_mode = tools::execute(
            "grep",
            &json!({"pattern": "hit", "output_mode": "files_with_matches"}),
            &ctx,
        )
        .await;
        assert!(files_mode.success, "{:?}", files_mode.error);
        assert_eq!(files_mode.output, "a.txt\nb.txt");
        assert_eq!(files_mode.observed_paths, vec!["a.txt", "b.txt"]);

        let files_limited = tools::execute(
            "grep",
            &json!({"pattern": "hit", "output_mode": "files_with_matches", "max_results": 1}),
            &ctx,
        )
        .await;
        assert!(files_limited.success, "{:?}", files_limited.error);
        assert!(files_limited.truncated);
        assert!(files_limited.output.starts_with("a.txt\n...[达到结果上限"));
        assert_eq!(files_limited.observed_paths, vec!["a.txt"]);

        let count_mode = tools::execute(
            "grep",
            &json!({"pattern": "hit", "output_mode": "count"}),
            &ctx,
        )
        .await;
        assert!(count_mode.success, "{:?}", count_mode.error);
        assert_eq!(count_mode.output, "a.txt: 3\nb.txt: 1");
        assert_eq!(count_mode.observed_paths, vec!["a.txt", "b.txt"]);

        let context_mode = tools::execute(
            "grep",
            &json!({"pattern": "hit", "output_mode": "content", "context_lines": 1}),
            &ctx,
        )
        .await;
        assert!(context_mode.success, "{:?}", context_mode.error);
        assert_eq!(
            context_mode.output,
            [
                "a.txt-2- one",
                "a.txt:3: hit1",
                "a.txt-4- two",
                "a.txt:5: hit2",
                "a.txt-6- three",
                "--",
                "a.txt-8- five",
                "a.txt:9: hit3",
                "a.txt-10- six",
                "b.txt:1: hit only",
            ]
            .join("\n")
        );
        assert_eq!(context_mode.observed_paths, vec!["a.txt", "b.txt"]);
    }

    #[tokio::test]
    async fn ls_recursive_respects_ignore_files_and_flat_ls_keeps_direct_entries() {
        let (_dir, ctx) = test_ctx(Rules::default());
        fs::write(ctx.workspace.join(".gitignore"), "dist_cache/\n*.tmp\n").unwrap();
        fs::write(ctx.workspace.join(".ignore"), "secret.txt\n").unwrap();
        fs::create_dir_all(ctx.workspace.join("dist_cache")).unwrap();
        fs::create_dir_all(ctx.workspace.join("node_modules")).unwrap();
        fs::create_dir_all(ctx.workspace.join("src")).unwrap();
        fs::write(ctx.workspace.join("dist_cache/artifact.bin"), "x").unwrap();
        fs::write(ctx.workspace.join("node_modules/pkg.txt"), "x").unwrap();
        fs::write(ctx.workspace.join("junk.tmp"), "x").unwrap();
        fs::write(ctx.workspace.join("secret.txt"), "x").unwrap();
        fs::write(ctx.workspace.join("src/main.rs"), "fn main() {}").unwrap();

        let flat = tools::execute("ls", &json!({"path": "."}), &ctx).await;
        assert!(flat.success, "{:?}", flat.error);
        assert!(flat.output.contains("dist_cache/"));
        assert!(flat.output.contains("junk.tmp"));
        assert!(flat.output.contains("secret.txt"));
        assert!(!flat.output.contains("node_modules"));

        let recursive = tools::execute("ls", &json!({"path": ".", "recursive": true}), &ctx).await;
        assert!(recursive.success, "{:?}", recursive.error);
        assert!(recursive.output.contains("src/"));
        assert!(recursive.output.contains("src/main.rs"));
        assert!(!recursive.output.contains("dist_cache"));
        assert!(!recursive.output.contains("artifact.bin"));
        assert!(!recursive.output.contains("junk.tmp"));
        assert!(!recursive.output.contains("secret.txt"));
        assert!(!recursive.output.contains("node_modules"));

        let recursive_all = tools::execute(
            "ls",
            &json!({"path": ".", "recursive": true, "include_ignored": true}),
            &ctx,
        )
        .await;
        assert!(recursive_all.success, "{:?}", recursive_all.error);
        assert!(recursive_all.output.contains("dist_cache/"));
        assert!(recursive_all.output.contains("dist_cache/artifact.bin"));
        assert!(recursive_all.output.contains("junk.tmp"));
        assert!(recursive_all.output.contains("secret.txt"));
        assert!(!recursive_all.output.contains("node_modules"));
    }
}
