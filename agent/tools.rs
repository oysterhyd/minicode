use crate::{
    attachments::Attachments,
    extensions::Catalog,
    mcp::McpRegistry,
    models::{Outcome, ToolSpec},
    store::Store,
    textfile::{self, Encoding},
    util,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime},
};
use tokio::{process::Command, sync::Mutex};
use tokio_util::sync::CancellationToken;

pub type Sink = Arc<dyn Fn(Value) + Send + Sync>;
#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub session: String,
    pub store: Store,
    pub attachments: Attachments,
    pub modalities: Vec<String>,
    pub catalog: Arc<Mutex<Catalog>>,
    pub mcp: Arc<McpRegistry>,
    pub cancel: CancellationToken,
    pub output: Option<Sink>,
    pub deadline: Option<Instant>,
    pub files: Files,
}
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
    /// The current state of `path`; reuses `previous` when size and mtime agree.
    fn of(path: &Path, previous: Option<&Seen>) -> Result<Self> {
        let metadata = path.metadata()?;
        if let Some(previous) = previous.filter(|p| p.matches(&metadata)) {
            return Ok(previous.clone());
        }
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        std::io::copy(&mut File::open(path)?, &mut hasher)?;
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            digest: format!("{:x}", sha2::Digest::finalize(hasher)),
        })
    }
}
fn remember(ctx: &ToolContext, path: &Path) {
    let previous = ctx.files.lock().unwrap().seen.get(path).cloned();
    if let Ok(state) = Seen::of(path, previous.as_ref()) {
        ctx.files.lock().unwrap().seen.insert(path.into(), state);
    }
}
/// Refuses to modify an existing file the session has not read, or one that
/// changed on disk since the session last saw it. New files are unrestricted.
fn ensure_current(ctx: &ToolContext, path: &Path, relative: &str) -> Result<Option<Seen>> {
    if !path.exists() {
        return Ok(None);
    }
    let Some(previous) = ctx.files.lock().unwrap().seen.get(path).cloned() else {
        bail!(
            "{relative} 已存在，但本会话还没有读取过它（会话恢复后也需要重新读取）；请先 read 当前内容再修改"
        )
    };
    let current = Seen::of(path, Some(&previous))?;
    if current.digest != previous.digest {
        bail!("{relative} 在上次读取后已被外部修改；请重新 read 后再修改")
    }
    Ok(Some(current))
}
pub fn builtins() -> &'static Vec<ToolSpec> {
    static TOOLS: OnceLock<Vec<ToolSpec>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let specs: Vec<ToolSpec> =
            serde_json::from_str(include_str!("assets/tools.json")).expect("built-in tool schemas");
        #[cfg(windows)]
        {
            let mut specs = specs;
            let bash = specs.iter_mut().find(|spec| spec.name == "bash").unwrap();
            let mut powershell = bash.clone();
            powershell.name = "powershell".into();
            powershell.description = bash.description.replace(
                "a shell command inside the workspace (PowerShell on Windows, bash elsewhere)",
                "a PowerShell command inside the workspace",
            );
            bash.description = bash.description.replace(
                "a shell command inside the workspace (PowerShell on Windows, bash elsewhere)",
                if git_bash().is_some() {
                    "a Git Bash command inside the workspace"
                } else {
                    "a PowerShell command inside the workspace (legacy bash alias)"
                },
            );
            specs.push(powershell);
            specs
        }
        #[cfg(not(windows))]
        specs
    })
}
pub fn read_only(name: &str) -> bool {
    [
        "read",
        "ls",
        "grep",
        "read_artifact",
        "skills_list",
        "skill_load",
        "skill_unload",
        "skill_resource",
        "delegate",
        "task_list",
        "memory_list",
    ]
    .contains(&name)
}
pub fn replay_safe(name: &str) -> bool {
    [
        "read",
        "ls",
        "grep",
        "read_artifact",
        "skills_list",
        "skill_resource",
        "task_list",
        "memory_list",
    ]
    .contains(&name)
}
pub fn parallel_read(name: &str) -> bool {
    replay_safe(name)
}
pub fn policy(name: &str, mode: &str) -> &'static str {
    if mode == "bypass"
        || read_only(name)
        || ["task_create", "task_claim", "task_complete"].contains(&name)
    {
        "allow"
    } else if ["write", "edit"].contains(&name) {
        if mode == "accept_edits" {
            "allow"
        } else {
            "ask"
        }
    } else if ["bash", "powershell"].contains(&name) || name.starts_with("mcp__") {
        "ask"
    } else {
        "deny"
    }
}
pub fn specs(
    catalog: &Catalog,
    mcp: &McpRegistry,
    agents: &[Value],
    allow_delegate: bool,
) -> Vec<ToolSpec> {
    let mut specs = builtins().clone();
    if !allow_delegate {
        specs.retain(|s| s.name != "delegate");
    }
    if let Some(spec) = specs.iter_mut().find(|s| s.name == "delegate") {
        spec.description="Delegate a bounded task to an assistant with its configured tools and the parent permission policy. Available kinds: ".into();
        let mut names = agents
            .iter()
            .filter(|a| a["enabled"] == true)
            .filter_map(|a| a["name"].as_str())
            .map(str::to_string)
            .collect::<Vec<_>>();
        names.extend(catalog.agents.keys().cloned());
        names.sort();
        names.dedup();
        spec.description.push_str(&names.join(", "));
    }
    let mut external = mcp
        .tools
        .values()
        .map(|(s, _, _)| s.clone())
        .collect::<Vec<_>>();
    external.sort_by(|a, b| a.name.cmp(&b.name));
    specs.extend(external);
    specs
}
pub fn validate(spec: &ToolSpec, input: &Value) -> Result<()> {
    static VALIDATORS: OnceLock<std::collections::HashMap<String, jsonschema::Validator>> =
        OnceLock::new();
    let cached = VALIDATORS.get_or_init(|| {
        builtins()
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    jsonschema::validator_for(&s.input_schema).expect("built-in schema"),
                )
            })
            .collect()
    });
    let compiled;
    let validator = if let Some(value) = cached.get(&spec.name) {
        value
    } else {
        compiled = jsonschema::validator_for(&spec.input_schema)?;
        &compiled
    };
    if !validator.is_valid(input) {
        let errors = validator
            .iter_errors(input)
            .take(4)
            .map(|e| format!("{}: {}", e.instance_path(), e))
            .collect::<Vec<_>>()
            .join("; ");
        bail!("invalid tool arguments: {errors}")
    };
    Ok(())
}
pub async fn execute(name: &str, args: &Value, ctx: &ToolContext) -> Outcome {
    match execute_inner(name, args, ctx).await {
        Ok(o) => o,
        Err(e) => Outcome::failure(e.to_string()),
    }
}
async fn execute_inner(name: &str, args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    if ctx.cancel.is_cancelled() {
        bail!("tool cancelled")
    }
    if let Some((_, original, connector)) = ctx.mcp.tools.get(name) {
        let mut connector = connector.lock().await;
        return tokio::select! {_ = ctx.cancel.cancelled()=>Err(anyhow::anyhow!("tool cancelled")),value=connector.call(original,args.clone(),&ctx.attachments)=>value};
    }
    match name {
        "bash" | "powershell" => {
            let command = util::string(args, "command")?;
            let cwd = util::in_workspace(&ctx.workspace, args["cwd"].as_str().unwrap_or("."))?;
            if !cwd.is_dir() {
                bail!("path is not a directory")
            }
            run_command_for(
                name,
                command,
                &cwd,
                args["timeout_s"]
                    .as_f64()
                    .unwrap_or(300.0)
                    .clamp(1.0, 3600.0),
                ctx,
            )
            .await
        }
        "skill_load" => {
            let name = util::string(args, "name")?;
            let s = ctx.catalog.lock().await.load(name)?;
            ctx.store.event(
                &ctx.session,
                "skill_activated",
                &json!({"name":name,"path":s.path,"sha256":s.digest}),
            )?;
            Ok(Outcome::ok(format!(
                "Activated skill {name} (sha256:{})\n{}",
                s.digest, s.content
            )))
        }
        "skill_unload" => {
            let name = util::string(args, "name")?;
            ctx.catalog.lock().await.unload(name)?;
            ctx.store
                .event(&ctx.session, "skill_deactivated", &json!({"name":name}))?;
            Ok(Outcome::ok(format!("Deactivated skill {name}")))
        }
        "skills_list" => Ok(Outcome::ok(ctx.catalog.lock().await.listing())),
        "skill_resource" => {
            let catalog = ctx.catalog.lock().await;
            let source =
                catalog.resource(util::string(args, "name")?, util::string(args, "path")?)?;
            Ok(bounded(source.content))
        }
        "task_create" => {
            ctx.store.task_create(&ctx.session, args)?;
            Ok(Outcome::ok(format!(
                "created task {}",
                util::string(args, "task_id")?
            )))
        }
        "task_claim" => {
            let task = util::string(args, "task_id")?;
            ctx.store.task_claim(&ctx.session, task, "agent")?;
            Ok(Outcome::ok(format!("claimed task {task}")))
        }
        "task_complete" => {
            let task = util::string(args, "task_id")?;
            let ok = args["ok"].as_bool().context("ok must be a boolean")?;
            ctx.store.task_complete(&ctx.session, task, ok)?;
            Ok(Outcome::ok(format!(
                "task {task}: {}",
                if ok { "done" } else { "failed" }
            )))
        }
        "task_list" => Ok(Outcome::ok(serde_json::to_string(
            &ctx.store.tasks(&ctx.session)?,
        )?)),
        "memory_list" => {
            let mut memories = ctx.store.memories(&ctx.workspace.to_string_lossy())?;
            if let Some(path) = args["path"].as_str() {
                let p = util::in_workspace(&ctx.workspace, path)?;
                let relative = p
                    .strip_prefix(&ctx.workspace)?
                    .to_string_lossy()
                    .replace('\\', "/");
                memories.retain(|m| {
                    let scope = m["scope"].as_str().unwrap_or(".");
                    scope == "." || scope == relative || relative.starts_with(&format!("{scope}/"))
                });
            }
            Ok(bounded(serde_json::to_string(&memories)?))
        }
        _ => {
            let name = name.to_string();
            let args = args.clone();
            let ctx = ctx.clone();
            tokio::task::spawn_blocking(move || execute_sync(&name, &args, &ctx)).await?
        }
    }
}
fn execute_sync(name: &str, args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    match name {
        "read" => {
            let outcome = read_file(args, ctx)?;
            remember(
                ctx,
                &util::in_workspace(&ctx.workspace, util::string(args, "path")?)?,
            );
            Ok(outcome)
        }
        "ls" => list(args, ctx),
        "grep" => grep(args, ctx),
        "write" | "edit" => edit(name, args, ctx),
        "read_artifact" => {
            let id = util::string(args, "artifact_id")?;
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let limit = args["limit"].as_u64().unwrap_or(20_000).min(20_000) as usize;
            let page = ctx.store.artifact_page(&ctx.session, id, offset, limit)?;
            let mut text = page["text"].as_str().unwrap_or_default().to_string();
            if page["hasMore"] == true {
                text.push_str(&format!(
                    "\n...[更多内容：next_offset={}]",
                    offset + text.chars().count()
                ));
            }
            Ok(Outcome::ok(text))
        }
        _ => bail!("unknown tool: {name}"),
    }
}
fn bounded(text: String) -> Outcome {
    if text.len() <= 50 * 1024 {
        Outcome::ok(text)
    } else {
        let mut o = Outcome::ok(format!(
            "{}\n...[output truncated: {} chars total]",
            util::prefix(&text, 50 * 1024),
            text.chars().count()
        ));
        o.full_output = Some(text);
        o
    }
}
fn read_file(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
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
    Ok(Outcome::ok(text))
}
fn list(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let path = args["path"].as_str().unwrap_or(".");
    let base = util::in_workspace(&ctx.workspace, path)?;
    if !base.is_dir() {
        bail!("path is not a directory: {path}")
    }
    let recursive = args["recursive"] == true;
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(500).min(500) as usize;
    let mut entries = vec![];
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
            let relative = entry
                .0
                .strip_prefix(&base)?
                .to_string_lossy()
                .replace('\\', "/");
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
    Ok(bounded(text))
}
fn grep(args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let pattern = util::string(args, "pattern")?;
    let pattern = if args.get("case_sensitive").is_some_and(|v| v == false) {
        format!("(?i){pattern}")
    } else {
        pattern.into()
    };
    let regex = fancy_regex::RegexBuilder::new(&pattern)
        .backtrack_limit(1_000_000)
        .build()
        .context("invalid regex")?;
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
    let mut skipped = 0;
    let start = Instant::now();
    let search_line = |line: &str,
                       number: &mut usize,
                       workspace_path: &str,
                       lines: &mut Vec<String>|
     -> Result<bool> {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        if start.elapsed().as_secs() > 30 {
            bail!("search timed out; narrow path or glob and retry")
        }
        *number += 1;
        if regex.is_match(line)? {
            let mut chars = line.chars();
            let snippet = chars.by_ref().take(500).collect::<String>();
            lines.push(format!(
                "{workspace_path}:{number}: {snippet}{}",
                if chars.next().is_some() {
                    "...[行已截断]"
                } else {
                    ""
                }
            ));
            if lines.len() >= max {
                lines.push("...[达到结果上限；请缩小 path 或 glob 继续搜索]".into());
                return Ok(true);
            }
        }
        Ok(false)
    };
    let files = util::walk(&base).filter(|e| e.file_type().is_file());
    'files: for entry in files {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        if start.elapsed().as_secs() > 30 {
            bail!("search timed out; narrow path or glob and retry")
        }
        let path = entry.path();
        let relative = if base.is_file() {
            base.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            path.strip_prefix(&base)?
                .to_string_lossy()
                .replace('\\', "/")
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
        let workspace_path = path
            .strip_prefix(&ctx.workspace)?
            .to_string_lossy()
            .replace('\\', "/");
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
                if search_line(line, &mut number, &workspace_path, &mut lines)? {
                    break 'files;
                }
            }
        }
        // Text that ends with a line break has one more, empty, line.
        if ended_with_lf && search_line("", &mut number, &workspace_path, &mut lines)? {
            break 'files;
        }
    }
    if lines.is_empty() {
        lines.push("(no matches)".into());
    }
    if skipped > 0 {
        lines.push(format!("...[跳过 {skipped} 个无法读取的文件]"));
    }
    Ok(bounded(lines.join("\n")))
}
fn replacement_texts<'a>(text: &str, args: &'a Value) -> Result<(Cow<'a, str>, Cow<'a, str>)> {
    let old = util::string(args, "old_text")?;
    let new = util::string(args, "new_text")?;
    if old.is_empty() {
        bail!("old_text must be nonempty")
    }
    // Reads expose logical lines. Keep a consistently CRLF file in its native
    // format when a model supplies the same text using LF line endings.
    let crlf = text.contains("\r\n")
        && !text
            .as_bytes()
            .iter()
            .enumerate()
            .any(|(i, b)| *b == b'\n' && (i == 0 || text.as_bytes()[i - 1] != b'\r'));
    let convert = |value: &'a str| {
        if crlf && !value.contains('\r') {
            Cow::Owned(value.replace('\n', "\r\n"))
        } else {
            Cow::Borrowed(value)
        }
    };
    Ok((convert(old), convert(new)))
}
fn edit(name: &str, args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    let raw = util::string(args, "path")?;
    let path = util::in_workspace(&ctx.workspace, raw)?;
    let relative = path
        .strip_prefix(&ctx.workspace)?
        .to_string_lossy()
        .replace('\\', "/");
    // Mutations of one file are serialized, so concurrent edits each apply to
    // the result of the previous one instead of racing on the same original.
    let lock = ctx
        .files
        .lock()
        .unwrap()
        .locks
        .entry(path.clone())
        .or_default()
        .clone();
    let _serialized = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if name == "edit" && !path.is_file() {
        bail!("path is not a file: {relative} (use write to create a new file)")
    }
    if path.exists() && !path.is_file() {
        bail!("path exists and is not a file: {relative}")
    }
    let seen = ensure_current(ctx, &path, &relative)?;
    let size = if path.exists() {
        path.metadata()?.len()
    } else {
        0
    };
    let encoding = if path.exists() {
        Encoding::probe(&path)?
    } else {
        Encoding::Utf8(false)
    };
    if name == "edit" && size > 256_000 && matches!(encoding, Encoding::Utf8(_)) {
        let outcome = edit_large(&path, &relative, args, ctx, seen)?;
        remember(ctx, &path);
        return Ok(outcome);
    }
    let before = if path.exists() && (size <= 256_000 || !matches!(encoding, Encoding::Utf8(_))) {
        encoding.decode(&textfile::read_bounded(&path)?)?
    } else {
        String::new()
    };
    let after = if name == "write" {
        util::string(args, "content")?.to_string()
    } else {
        let (old, new) = replacement_texts(&before, args)?;
        let count = before.matches(old.as_ref()).count();
        if count == 0 {
            bail!("old_text not found in {relative}: file may have changed; re-read the file first")
        }
        if count > 1 && args["replace_all"] != true {
            bail!(
                "old_text matches {count} locations in {relative}; include more surrounding context to make it unique, or pass replace_all=true"
            )
        }
        if args["replace_all"] == true {
            before.replace(old.as_ref(), new.as_ref())
        } else {
            before.replacen(old.as_ref(), new.as_ref(), 1)
        }
    };
    if ctx.cancel.is_cancelled() {
        bail!("tool cancelled")
    }
    let encoded = encoding.encode(&after)?;
    util::atomic_write(&path, &encoded)?;
    if let Ok(metadata) = path.metadata() {
        ctx.files.lock().unwrap().seen.insert(
            path.clone(),
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
    let input = File::open(path)?;
    let original = input.metadata()?;
    let seen = seen.context("large file is not in the read ledger")?;
    if !seen.matches(&original) {
        bail!("{relative} 在上次读取后已被外部修改；请重新 read 后再修改")
    }
    // The mapped source is borrowed; no second heap copy of a large file.
    let mapped = unsafe { memmap2::MmapOptions::new().map(&input)? };
    let text = std::str::from_utf8(&mapped).context("binary or non-UTF-8 file")?;
    let (old, new) = replacement_texts(text, args)?;
    let count = text.matches(old.as_ref()).count();
    if count == 0 {
        bail!("old_text not found in {relative}: file may have changed; re-read the file first")
    }
    if count > 1 && args["replace_all"] != true {
        bail!(
            "old_text matches {count} locations in {relative}; include more surrounding context to make it unique, or pass replace_all=true"
        )
    }
    let parent = path.parent().context("invalid file path")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    let mut position = 0;
    let mut replacements = 0;
    for (index, _) in text.match_indices(old.as_ref()) {
        if ctx.cancel.is_cancelled() {
            bail!("tool cancelled")
        }
        temp.write_all(&mapped[position..index])?;
        temp.write_all(new.as_bytes())?;
        position = index + old.len();
        replacements += 1;
        if args["replace_all"] != true {
            break;
        }
    }
    temp.write_all(&mapped[position..])?;
    temp.as_file().sync_all()?;
    temp.as_file().set_permissions(original.permissions())?;
    drop(mapped);
    drop(input);
    if !seen.matches(&path.metadata()?) {
        bail!("{relative} 在编辑过程中被外部修改；请重新 read 后再修改")
    }
    if ctx.cancel.is_cancelled() {
        bail!("tool cancelled")
    }
    temp.persist(path).map_err(|e| e.error)?;
    Ok(Outcome::ok(format!(
        "Applied change to large file {relative} ({replacements} replacement(s)); old={} bytes, new={} bytes.",
        original.len(),
        path.metadata()?.len()
    )))
}
#[cfg(windows)]
pub(crate) fn git_bash() -> Option<PathBuf> {
    if std::env::var("MINICODE_WINDOWS_SHELL")
        .is_ok_and(|value| value.eq_ignore_ascii_case("powershell"))
    {
        return None;
    }
    let path = util::bundled_path().or_else(|| std::env::var_os("PATH"))?;
    for dir in std::env::split_paths(&path) {
        if !dir.join("git.exe").is_file() {
            continue;
        }
        let root = if dir.file_name().is_some_and(|name| {
            name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("bin")
        }) {
            dir.parent().unwrap_or(&dir)
        } else {
            &dir
        };
        for candidate in [root.join("bin/bash.exe"), root.join("usr/bin/bash.exe")] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn powershell_command(command: &str, program: &std::ffi::OsStr) -> Command {
    let mut cmd = Command::new(program);
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!(
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);$OutputEncoding=[Text.UTF8Encoding]::new($false);$global:LASTEXITCODE=$null;\n{command}\n$__minicode_ok=$?;if ($__minicode_ok) {{ exit 0 }} elseif ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }} else {{ exit 1 }}"
        ),
    ]);
    cmd
}

#[cfg(windows)]
fn pwsh_file_version(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // The returned version structure points into `data`, which stays alive
    // until the unaligned copy is complete.
    unsafe {
        let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
        if size == 0 || size > 1_000_000 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut value = std::ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(
            data.as_ptr().cast(),
            [b'\\' as u16, 0].as_ptr(),
            &mut value,
            &mut length,
        ) == 0
            || value.is_null()
            || (length as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let info = value.cast::<VS_FIXEDFILEINFO>().read_unaligned();
        if info.dwSignature != 0xfeef04bd {
            return None;
        }
        Some(format!(
            "{}.{}.{}",
            info.dwProductVersionMS >> 16,
            info.dwProductVersionMS & 0xffff,
            info.dwProductVersionLS >> 16
        ))
    }
}

#[cfg(windows)]
fn powershell_info() -> &'static (PathBuf, String) {
    use std::os::windows::{fs::MetadataExt, process::CommandExt};
    static INFO: OnceLock<(PathBuf, String)> = OnceLock::new();
    INFO.get_or_init(|| {
        let path = util::bundled_path().or_else(|| std::env::var_os("PATH"));
        let program = path
            .as_ref()
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join("pwsh.exe"))
            .find(|path| {
                let app_alias = path.parent().and_then(Path::file_name).is_some_and(|name| {
                    name.eq_ignore_ascii_case("WindowsApps")
                        && std::fs::symlink_metadata(path)
                            .is_ok_and(|metadata| metadata.file_attributes() & 0x400 != 0)
                });
                path.is_file() && !app_alias
            })
            .unwrap_or_else(|| {
                std::env::var_os("SystemRoot")
                    .map(PathBuf::from)
                    .map(|root| root.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
                    .unwrap_or_else(|| PathBuf::from("powershell.exe"))
            });
        let version = if program
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("pwsh.exe"))
        {
            pwsh_file_version(&program)
        } else {
            None
        }
        .or_else(|| {
            (|| -> Option<String> {
                let mut output = tempfile::tempfile().ok()?;
                let mut child = std::process::Command::new(&program)
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        "[Console]::Out.Write($PSVersionTable.PSVersion.ToString())",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(output.try_clone().ok()?))
                    .stderr(Stdio::null())
                    .creation_flags(0x08000000)
                    .spawn()
                    .ok()?;
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) if status.success() => {
                            let mut version = String::new();
                            output.seek(SeekFrom::Start(0)).ok()?;
                            output.take(128).read_to_string(&mut version).ok()?;
                            let version = version.trim();
                            if version.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
                                && version.contains('.')
                            {
                                return Some(version.into());
                            }
                            return None;
                        }
                        Ok(Some(_)) => return None,
                        Ok(None) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return None;
                        }
                    }
                }
            })()
        })
        .unwrap_or_else(|| "版本未知".into());
        (program, version)
    })
}

#[cfg(windows)]
pub(crate) fn windows_shell_guidance() -> String {
    let version = &powershell_info().1;
    let major = version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok());
    let chains = if major.is_some_and(|major| major >= 7) {
        "PowerShell 7 支持 && 和 ||；仍应按 PowerShell 语法编写命令。"
    } else {
        "PowerShell 5.1 或版本未知时不支持 && 和 ||；请使用 if ($?) 或分步执行。"
    };
    let bash = if git_bash().is_some() {
        "bash 工具使用 Git Bash，按 bash 语法编写命令。"
    } else {
        "bash 工具是 PowerShell 的兼容别名，按 PowerShell 语法编写命令。"
    };
    format!("{bash} powershell 工具使用 PowerShell {version}。{chains}")
}

pub fn shell(command: &str, cwd: &Path) -> Command {
    shell_for("bash", command, cwd)
}

fn shell_for(name: &str, command: &str, cwd: &Path) -> Command {
    #[cfg(windows)]
    let mut cmd = {
        let bash = if name == "bash" { git_bash() } else { None };
        let mut c = if let Some(bash) = bash {
            let mut c = Command::new(bash);
            c.args(["-c", command]);
            c
        } else {
            powershell_command(command, powershell_info().0.as_os_str())
        };
        c.creation_flags(0x08000000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let _ = name;
        let mut c = Command::new("bash");
        c.args(["-c", command]);
        c.process_group(0);
        c
    };
    if let Some(path) = util::bundled_path() {
        cmd.env("PATH", path);
    }
    for (key, value) in [
        ("NO_COLOR", "1"),
        ("TERM", "dumb"),
        ("CARGO_TERM_COLOR", "never"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_EDITOR", "true"),
        ("GIT_SEQUENCE_EDITOR", "true"),
        ("PAGER", "cat"),
        ("GIT_PAGER", "cat"),
        ("CI", "1"),
    ] {
        if std::env::var_os(key).is_none_or(|value| value.is_empty()) {
            cmd.env(key, value);
        }
    }
    #[cfg(windows)]
    for (name, default) in [("PYTHONUTF8", "1"), ("PYTHONIOENCODING", "utf-8")] {
        if std::env::var_os(name).is_none_or(|value| value.is_empty()) {
            cmd.env(name, default);
        }
    }
    cmd.current_dir(cwd).stdin(Stdio::null()).kill_on_drop(true);
    cmd
}
pub async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("taskkill.exe");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x08000000);
            let _ = tokio::time::timeout(Duration::from_secs(5), cmd.status()).await;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}
pub async fn run_command(
    command: &str,
    cwd: &Path,
    timeout: f64,
    ctx: &ToolContext,
) -> Result<Outcome> {
    run_command_for("bash", command, cwd, timeout, ctx).await
}

struct OutputReaders(Vec<tokio::task::JoinHandle<Result<()>>>);
impl Drop for OutputReaders {
    fn drop(&mut self) {
        for reader in &self.0 {
            reader.abort();
        }
    }
}
fn capture_pipe(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    file: Arc<std::sync::Mutex<File>>,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let size = pipe.read(&mut buffer).await?;
            if size == 0 {
                return Ok(());
            }
            file.lock().unwrap().write_all(&buffer[..size])?;
        }
    })
}
pub async fn run_command_for(
    name: &str,
    command: &str,
    cwd: &Path,
    timeout: f64,
    ctx: &ToolContext,
) -> Result<Outcome> {
    // Drain bounded pipe chunks to disk; EOF tells us whether descendants still
    // hold output, independently of redirected processes in the command tree.
    let mut captured = tempfile::NamedTempFile::new()?;
    let mut cmd = shell_for(name, command, cwd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("failed to start command")?;
    let mut tree = match crate::command_process::ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            kill_tree(&mut child).await;
            return Err(error.context("failed to own command process tree"));
        }
    };
    let file = Arc::new(std::sync::Mutex::new(captured.as_file().try_clone()?));
    let mut readers = OutputReaders(vec![
        capture_pipe(child.stdout.take().unwrap(), file.clone()),
        capture_pipe(child.stderr.take().unwrap(), file),
    ]);
    let started = Instant::now();
    let mut live_position = 0u64;
    let mut timed_out = false;
    let mut quota = false;
    let mut cancelled = false;
    let mut exited = None;
    let mut lingered = false;
    let mut last_emit = Instant::now();
    let status = loop {
        if exited.is_none()
            && let Some(status) = child.try_wait()?
        {
            exited = Some((Instant::now(), status));
        }
        if let Some((time, status)) = exited {
            if readers.0.iter().all(|reader| reader.is_finished()) {
                break Some(status);
            }
            if time.elapsed() >= Duration::from_secs(2) {
                lingered = true;
                break Some(status);
            }
        }
        if ctx.cancel.is_cancelled() {
            cancelled = true;
            kill_tree(&mut child).await;
            break child.try_wait()?;
        }
        if (timeout > 0.0 && started.elapsed().as_secs_f64() >= timeout)
            || ctx.deadline.is_some_and(|d| Instant::now() >= d)
        {
            timed_out = true;
            kill_tree(&mut child).await;
            break child.try_wait()?;
        }
        let size = captured.as_file().metadata()?.len();
        if size > 100_000_000 {
            quota = true;
            kill_tree(&mut child).await;
            break child.try_wait()?;
        }
        if size > live_position && last_emit.elapsed() >= Duration::from_millis(250) {
            if let Some(emit) = &ctx.output {
                let mut f = File::open(captured.path())?;
                f.seek(SeekFrom::Start(size.saturating_sub(8000)))?;
                let mut bytes = vec![];
                f.take(8000).read_to_end(&mut bytes)?;
                let text = decode(&bytes);
                emit(json!({"text":util::suffix(&text,2000)}));
            }
            live_position = size;
            last_emit = Instant::now();
        }
        tokio::select! {_ = tokio::time::sleep(Duration::from_millis(50))=>{},_ = ctx.cancel.cancelled()=>{}}
    };
    if !timed_out && !quota && !cancelled && !lingered {
        tree.release()?;
    } else {
        tree.shutdown().await;
    }
    drop(tree);
    for reader in &mut readers.0 {
        match tokio::time::timeout(Duration::from_secs(1), &mut *reader).await {
            Ok(result) => result??,
            Err(_) => {
                reader.abort();
                let _ = reader.await;
            }
        }
    }
    let total = captured.as_file().metadata()?.len().min(100_000_000);
    #[cfg(windows)]
    let capture_utf8 = capture_is_utf8(captured.as_file_mut(), total)?;
    #[cfg(not(windows))]
    let capture_utf8 = true;
    captured.as_file_mut().seek(SeekFrom::Start(0))?;
    let mut bytes = vec![];
    captured
        .as_file_mut()
        .take(256_000)
        .read_to_end(&mut bytes)?;
    let decoded = decode_capture_sample(&bytes, capture_utf8);
    let decoded = decoded.trim_end_matches(['\r', '\n']);
    const PREVIEW_BYTES: usize = 50 * 1024 - 512;
    let truncated =
        total > 256_000 || decoded.len() > PREVIEW_BYTES || decoded.lines().count() > 2000;
    let mut outcome = Outcome::ok(decoded);
    if truncated {
        let head = util::prefix(decoded, PREVIEW_BYTES / 2)
            .split_inclusive('\n')
            .take(999)
            .collect::<String>();
        let tail_source = if total > 256_000 {
            captured
                .as_file_mut()
                .seek(SeekFrom::Start(total.saturating_sub(128_000)))?;
            bytes.clear();
            captured
                .as_file_mut()
                .take(128_000)
                .read_to_end(&mut bytes)?;
            decode_capture_sample(&bytes, capture_utf8)
        } else {
            decoded.to_string()
        };
        let tail = util::tail(&tail_source, 999, PREVIEW_BYTES / 2);
        let artifact = record_capture(&captured, ctx, total)?;
        outcome.output = format!(
            "{}\n...[middle output omitted; output truncated; 完整输出见 [artifact:{artifact}]]\n{tail}",
            head.trim_end_matches(['\r', '\n'])
        );
    }
    outcome.exit_code = status.and_then(|s| s.code());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        outcome.signal = status.and_then(|s| s.signal()).map(|signal| match signal {
            libc::SIGTERM => "SIGTERM".into(),
            libc::SIGKILL => "SIGKILL".into(),
            libc::SIGINT => "SIGINT".into(),
            libc::SIGHUP => "SIGHUP".into(),
            libc::SIGQUIT => "SIGQUIT".into(),
            libc::SIGABRT => "SIGABRT".into(),
            libc::SIGSEGV => "SIGSEGV".into(),
            libc::SIGPIPE => "SIGPIPE".into(),
            libc::SIGILL => "SIGILL".into(),
            libc::SIGBUS => "SIGBUS".into(),
            libc::SIGFPE => "SIGFPE".into(),
            libc::SIGUSR1 => "SIGUSR1".into(),
            libc::SIGUSR2 => "SIGUSR2".into(),
            libc::SIGALRM => "SIGALRM".into(),
            _ => format!("signal {signal}"),
        });
    }
    outcome.success =
        outcome.exit_code == Some(0) && !cancelled && !timed_out && !quota && !lingered;
    if cancelled {
        outcome.error = Some("tool cancelled; command tree was killed".into());
    } else if timed_out {
        outcome.error = Some(format!("command timed out after {timeout}s and was killed"));
    } else if lingered {
        outcome.error = Some("shell exited but a descendant kept output open; capture was stopped and the command tree was killed".into());
    } else if quota {
        outcome.error = Some(
            "command output exceeded capture quota and was killed; narrow or redirect output"
                .into(),
        );
    } else if !outcome.success {
        outcome.error = Some(match (&outcome.signal, outcome.exit_code) {
            (Some(signal), _) => format!("command terminated by {signal}"),
            (_, Some(code)) => format!("command exited with code {code}"),
            _ => "command terminated without an exit code".into(),
        });
    }
    if let (Some(signal), Some(error)) = (&outcome.signal, &mut outcome.error)
        && !error.contains(signal)
    {
        error.push_str(&format!(" ({signal})"));
    }
    Ok(outcome)
}
fn record_capture(
    captured: &tempfile::NamedTempFile,
    ctx: &ToolContext,
    total: u64,
) -> Result<String> {
    let artifact = format!("command_output_{}", &util::id()[..12]);
    let dir = ctx.store.root().join("artifacts").join(&ctx.session);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{artifact}.txt"));
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    let mut source = captured.as_file().try_clone()?;
    source.seek(SeekFrom::Start(0))?;
    #[cfg(windows)]
    let codepage = if capture_is_utf8(&mut source, total)? {
        None
    } else {
        use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};
        source.seek(SeekFrom::Start(0))?;
        let mut sample = vec![0; (total as usize).min(8192)];
        let n = source.read(&mut sample)?;
        sample.truncate(n);
        unsafe { legacy_codepage(&sample, GetOEMCP(), GetACP()) }
    };
    source.seek(SeekFrom::Start(0))?;
    let mut source = source.take(total);
    #[cfg(windows)]
    if let Some(codepage) = codepage {
        copy_codepage_capture(&mut source, &mut temp, codepage)?;
    } else {
        copy_utf8_capture(&mut source, &mut temp)?;
    }
    #[cfg(not(windows))]
    copy_utf8_capture(&mut source, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist(&path).map_err(|e| e.error)?;
    ctx.store.with(|c| {
        c.execute(
            "INSERT INTO artifacts VALUES(?,?,?,?,?)",
            rusqlite::params![
                ctx.session,
                artifact,
                "command_output",
                format!("{artifact}.txt"),
                util::now()
            ],
        )?;
        Ok(())
    })?;
    ctx.store
        .prune_output_artifacts(Some((&ctx.session, &artifact)))?;
    Ok(artifact)
}

fn copy_utf8_capture(source: &mut impl Read, output: &mut impl Write) -> Result<()> {
    let mut buffer = vec![];
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut chunk)?;
        buffer.extend_from_slice(&chunk[..n]);
        let mut end = 0;
        while end < buffer.len() {
            match std::str::from_utf8(&buffer[end..]) {
                Ok(_) => {
                    output.write_all(&buffer[end..])?;
                    end = buffer.len();
                }
                Err(error) => {
                    let valid = end + error.valid_up_to();
                    output.write_all(&buffer[end..valid])?;
                    end = valid;
                    if let Some(length) = error.error_len() {
                        output.write_all("�".as_bytes())?;
                        end += length;
                    } else if n == 0 {
                        output.write_all("�".as_bytes())?;
                        end = buffer.len();
                    } else {
                        break;
                    }
                }
            }
        }
        buffer.drain(..end);
        if n == 0 {
            break;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn capture_is_utf8(source: &mut File, total: u64) -> Result<bool> {
    let mut source = source.take(total);
    let mut pending = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut chunk)?;
        pending.extend_from_slice(&chunk[..n]);
        match std::str::from_utf8(&pending) {
            Ok(_) => pending.clear(),
            Err(error) if error.error_len().is_some() => return Ok(false),
            Err(error) => {
                pending.drain(..error.valid_up_to());
            }
        }
        if n == 0 {
            return Ok(pending.is_empty());
        }
    }
}

#[cfg(windows)]
fn copy_codepage_capture(
    source: &mut impl Read,
    output: &mut impl Write,
    codepage: u32,
) -> Result<()> {
    use windows_sys::Win32::Globalization::IsDBCSLeadByteEx;
    let lead: [bool; 256] =
        std::array::from_fn(|byte| unsafe { IsDBCSLeadByteEx(codepage, byte as u8) != 0 });
    let mut pending = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut chunk)?;
        pending.extend_from_slice(&chunk[..n]);
        let mut end = 0;
        while end < pending.len() {
            if lead[pending[end] as usize] {
                if end + 1 == pending.len() && n != 0 {
                    break;
                }
                end += if end + 1 < pending.len() { 2 } else { 1 };
            } else {
                end += 1;
            }
        }
        if end > 0 {
            let text = decode_codepage(&pending[..end], codepage, false)
                .unwrap_or_else(|| String::from_utf8_lossy(&pending[..end]).into_owned());
            output.write_all(text.as_bytes())?;
            pending.drain(..end);
        }
        if n == 0 {
            break;
        }
    }
    Ok(())
}
#[cfg(windows)]
fn decode_codepage(bytes: &[u8], codepage: u32, strict: bool) -> Option<String> {
    use windows_sys::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};
    if bytes.is_empty() {
        return Some(String::new());
    }
    let len = i32::try_from(bytes.len()).ok()?;
    let flags = if strict { MB_ERR_INVALID_CHARS } else { 0 };
    // MultiByteToWideChar first computes the UTF-16 length, then fills exactly
    // that many elements in the allocated buffer.
    unsafe {
        let needed = MultiByteToWideChar(
            codepage,
            flags,
            bytes.as_ptr(),
            len,
            std::ptr::null_mut(),
            0,
        );
        if needed <= 0 {
            return None;
        }
        let mut wide = vec![0u16; needed as usize];
        let written = MultiByteToWideChar(
            codepage,
            flags,
            bytes.as_ptr(),
            len,
            wide.as_mut_ptr(),
            needed,
        );
        (written == needed).then(|| String::from_utf16_lossy(&wide))
    }
}

#[cfg(windows)]
fn legacy_codepage(bytes: &[u8], oem: u32, ansi: u32) -> Option<u32> {
    let candidates = [oem, ansi];
    candidates
        .iter()
        .copied()
        .filter(|codepage| *codepage != 65001)
        .find(|codepage| decode_codepage(bytes, *codepage, true).is_some())
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .filter(|codepage| *codepage != 65001)
                .find(|codepage| decode_codepage(bytes, *codepage, false).is_some())
        })
}

#[cfg(windows)]
fn decode_windows(bytes: &[u8], oem: u32, ansi: u32) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    legacy_codepage(bytes, oem, ansi)
        .and_then(|codepage| decode_codepage(bytes, codepage, false))
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

fn decode(bytes: &[u8]) -> String {
    #[cfg(windows)]
    let text = unsafe {
        use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};
        decode_windows(bytes, GetOEMCP(), GetACP())
    };
    #[cfg(not(windows))]
    let text = String::from_utf8_lossy(bytes).into_owned();
    util::command_text(&text)
}

fn decode_capture_sample(bytes: &[u8], utf8: bool) -> String {
    if !utf8 {
        return decode(bytes);
    }
    // A window may start/end inside a UTF-8 scalar. Its boundaries must not
    // cause Windows to reinterpret an otherwise UTF-8 capture as a code page.
    let start = bytes
        .iter()
        .take(3)
        .take_while(|b| **b & 0xc0 == 0x80)
        .count();
    let bytes = &bytes[start..];
    let end = match std::str::from_utf8(bytes) {
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        _ => bytes.len(),
    };
    util::command_text(&String::from_utf8_lossy(&bytes[..end]))
}

#[cfg(test)]
mod output_tests {
    #[test]
    fn sh3_strips_terminal_sequences_and_overwritten_progress() {
        assert_eq!(super::decode(b"\x1b[31merror\x1b[0m\x1b[K\n\x1b[2J\x1b[?25l\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x07\n10%\r20%\r100%\r\n"), "error\nlink\n100%\n");
        assert_eq!(super::decode(b"ok\x1b[31"), "ok");
    }
}

#[cfg(all(test, windows))]
mod windows_shell_tests {
    use super::powershell_command;
    use std::{ffi::OsStr, path::PathBuf};

    #[tokio::test]
    async fn powershell_versions_preserve_native_and_cmdlet_status() {
        let legacy = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let modern = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("pwsh.exe"))
                .find(|path| path.is_file())
        });
        for program in std::iter::once(legacy).chain(modern) {
            for (command, expected) in [
                ("node -e \"process.exit(7)\"", 7),
                ("node -e \"process.exit(9)\"; Write-Output 'recovered'", 0),
                ("Write-Error 'failure'", 1),
                ("exit 23", 23),
            ] {
                let status = powershell_command(command, OsStr::new(&program))
                    .output()
                    .await
                    .unwrap()
                    .status;
                assert_eq!(
                    status.code(),
                    Some(expected),
                    "{}: {command}",
                    program.display()
                );
            }
        }
    }
}

#[cfg(all(test, windows))]
mod windows_encoding_tests {
    use super::{capture_is_utf8, copy_codepage_capture, decode_windows};
    use std::io::{Seek, SeekFrom, Write};

    // This machine may use a non-Chinese system locale; the production
    // decoder will be exercised with CP936 explicitly after the fix.
    fn cp936(bytes: &[u8]) -> String {
        decode_windows(bytes, 936, 936)
    }

    #[test]
    fn gbk_cp936_and_utf8_are_decoded_without_mojibake() {
        assert_eq!(cp936(&[0xd6, 0xd0, 0xce, 0xc4]), "中文");
        assert_eq!(
            decode_windows(&[0xd6, 0xd0, 0xce, 0xc4], 65001, 936),
            "中文"
        );
        assert_eq!(cp936("中文".as_bytes()), "中文");
        if unsafe { windows_sys::Win32::Globalization::GetOEMCP() } == 936 {
            assert_eq!(super::decode(&[0xd6, 0xd0, 0xce, 0xc4]), "中文");
        }
    }

    #[test]
    fn gbk_capture_keeps_characters_at_chunk_boundaries() {
        for padding in [65_534, 65_535] {
            let mut bytes = vec![b'a'; padding];
            bytes.extend_from_slice(&[0xd6, 0xd0, 0xce, 0xc4]);
            let mut decoded = Vec::new();
            copy_codepage_capture(&mut bytes.as_slice(), &mut decoded, 936).unwrap();
            assert_eq!(decoded.len(), padding + "中文".len());
            assert!(decoded.ends_with("中文".as_bytes()));
        }
    }

    #[test]
    fn utf8_capture_detection_handles_split_characters() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![b'a'; 65_535]).unwrap();
        file.write_all("中文".as_bytes()).unwrap();
        let total = file.metadata().unwrap().len();
        file.seek(SeekFrom::Start(0)).unwrap();
        assert!(capture_is_utf8(&mut file, total).unwrap());
        file.seek(SeekFrom::Start(total)).unwrap();
        file.write_all(&[0xd6, 0xd0]).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        assert!(!capture_is_utf8(&mut file, total + 2).unwrap());
    }
}
