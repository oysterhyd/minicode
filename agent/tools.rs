mod file_ops;
mod shell;

#[cfg(test)]
use file_ops::grep_files;
pub use file_ops::{FileLedger, Files};
use file_ops::{edit, grep, list, observed_path, read_file, remember};
pub(crate) use shell::{CommandCapture, decode, run_command_captured};
#[cfg(windows)]
pub(crate) use shell::{git_bash, windows_shell_guidance};
pub use shell::{kill_tree, run_command, run_command_for, shell};

use crate::{
    attachments::Attachments,
    extensions::Catalog,
    mcp::McpRegistry,
    models::{Outcome, ToolSpec},
    store::Store,
    util,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::PathBuf, sync::Arc, sync::OnceLock, time::Instant};
use tokio::sync::Mutex;
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
    pub permissions: Arc<crate::permissions::Rules>,
    pub sandbox: crate::sandbox::Config,
    pub force_sandbox: bool,
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
        "bash_output",
        "submit_result",
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
        "submit_result",
    ]
    .contains(&name)
}
pub fn parallel_read(name: &str) -> bool {
    replay_safe(name) && name != "submit_result"
}
pub fn policy(name: &str, mode: &str) -> &'static str {
    if mode == "bypass" && !name.starts_with("mcp__")
        || read_only(name)
        || ["task_create", "task_claim", "task_complete", "kill_shell"].contains(&name)
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
    if allow_delegate {
        specs.retain(|s| s.name != "submit_result");
    }
    if !allow_delegate {
        specs.retain(|s| !["delegate", "review_delegate"].contains(&s.name.as_str()));
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
#[cfg(test)]
mod validator_cache_tests {
    use super::*;
    #[test]
    fn t6_cache_has_lru_and_schema_byte_bounds_and_shares_concurrent_calls() {
        let mut spec = ToolSpec {
            name: "mcp__bounded__0".into(),
            description: String::new(),
            input_schema: json!({"type":"integer","$comment":"x".repeat(65536)}),
            requires_approval: false,
            read_only_hint: false,
        };
        let mut cache = ValidatorCache::default();
        let first = cache.get(&spec).unwrap();
        for i in 1..150 {
            spec.name = format!("mcp__bounded__{i}");
            cache.get(&spec).unwrap();
        }
        assert!(cache.bytes <= 8 * 1024 * 1024);
        assert!(cache.entries.len() < 150);
        spec.name = "mcp__bounded__0".into();
        assert!(!Arc::ptr_eq(&first, &cache.get(&spec).unwrap()));
        let spec = ToolSpec {
            name: "mcp__concurrent_cache__tool".into(),
            description: String::new(),
            input_schema: json!({"type":"integer"}),
            requires_approval: false,
            read_only_hint: false,
        };
        let expected = external_validator(&spec).unwrap();
        let tasks = (0..8)
            .map(|_| {
                let spec = spec.clone();
                std::thread::spawn(move || external_validator(&spec).unwrap())
            })
            .collect::<Vec<_>>();
        for task in tasks {
            assert!(Arc::ptr_eq(&expected, &task.join().unwrap()));
        }
    }
    #[test]
    fn t6_external_validator_is_reused_and_changed_schemas_are_isolated() {
        let mut spec = ToolSpec {
            name: "mcp__cache_test__tool".into(),
            description: String::new(),
            input_schema: json!({"type":"string"}),
            requires_approval: false,
            read_only_hint: false,
        };
        let first = external_validator(&spec).unwrap();
        let again = external_validator(&spec).unwrap();
        assert!(Arc::ptr_eq(&first, &again), "MCP schema was recompiled");
        assert!(validate(&spec, &json!("valid")).is_ok());
        assert!(validate(&spec, &json!(7)).is_err());
        spec.input_schema = json!({"type":"integer"});
        let changed = external_validator(&spec).unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(validate(&spec, &json!(7)).is_ok());
        assert!(validate(&spec, &json!("invalid")).is_err());
        spec.name = "mcp__cache_test__other".into();
        assert!(!Arc::ptr_eq(&changed, &external_validator(&spec).unwrap()));
    }
}
fn external_validator(spec: &ToolSpec) -> Result<Arc<jsonschema::Validator>> {
    static CACHE: OnceLock<std::sync::Mutex<ValidatorCache>> = OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| anyhow::anyhow!("validator cache lock poisoned"))?
        .get(spec)
}

#[derive(Default)]
struct ValidatorCache {
    entries: VecDeque<CachedValidator>,
    bytes: usize,
}
struct CachedValidator {
    key: (String, String),
    schema_bytes: usize,
    validator: Arc<jsonschema::Validator>,
}
impl ValidatorCache {
    fn get(&mut self, spec: &ToolSpec) -> Result<Arc<jsonschema::Validator>> {
        const MAX_ENTRIES: usize = 256;
        const MAX_BYTES: usize = 8 * 1024 * 1024;
        let bytes = serde_json::to_vec(&spec.input_schema)?;
        let key = (spec.name.clone(), util::hash(&bytes));
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key) {
            let entry = self.entries.remove(position).unwrap();
            let validator = entry.validator.clone();
            self.entries.push_back(entry);
            return Ok(validator);
        }
        let validator = Arc::new(jsonschema::validator_for(&spec.input_schema)?);
        if bytes.len() <= MAX_BYTES {
            while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes.len() > MAX_BYTES {
                if let Some(victim) = self.entries.pop_front() {
                    self.bytes -= victim.schema_bytes;
                }
            }
            self.bytes += bytes.len();
            self.entries.push_back(CachedValidator {
                key,
                schema_bytes: bytes.len(),
                validator: validator.clone(),
            });
        }
        Ok(validator)
    }
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
        compiled = external_validator(spec)?;
        compiled.as_ref()
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
    let mut outcome = match execute_inner(name, args, ctx).await {
        Ok(o) => o,
        Err(e) => Outcome::failure(e.to_string()),
    };
    if outcome.success
        && outcome.observed_paths.is_empty()
        && ["read", "write", "edit"].contains(&name)
        && let Some(raw) = args["path"].as_str()
        && let Ok(path) = util::in_workspace(&ctx.workspace, raw)
        && let Some(path) = observed_path(ctx, &path)
    {
        outcome.observed_paths.push(path);
    }
    outcome
}
async fn execute_inner(name: &str, args: &Value, ctx: &ToolContext) -> Result<Outcome> {
    if ctx.cancel.is_cancelled() {
        bail!("tool cancelled")
    }
    if let Some((_, original, connector)) = ctx.mcp.tools.get(name) {
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
            if crate::paging::artifact_line_mode(args) {
                let path = ctx.store.artifact_path(&ctx.session, id)?;
                return Ok(Outcome::ok(crate::paging::artifact_lines(&path, args)?));
            }
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let limit = args["limit"]
                .as_u64()
                .unwrap_or(crate::paging::ARTIFACT_DEFAULT_LIMIT as u64)
                .min(20_000) as usize;
            let page = ctx.store.artifact_page(&ctx.session, id, offset, limit)?;
            let mut text = page["text"].as_str().unwrap_or_default().to_string();
            if page["hasMore"] == true {
                text.push_str(&format!(
                    "\n...[更多内容：next_offset={}]",
                    offset + text.chars().count()
                ));
            }
            let mut outcome = Outcome::ok(text);
            outcome.truncated = page["hasMore"] == true;
            Ok(outcome)
        }
        _ => bail!("unknown tool: {name}"),
    }
}
pub(crate) fn bounded(text: String) -> Outcome {
    if text.len() <= 50 * 1024 {
        Outcome::ok(text)
    } else {
        let mut o = Outcome::ok(format!(
            "{}\n...[output truncated: {} chars total]",
            util::prefix(&text, 50 * 1024),
            text.chars().count()
        ));
        o.full_output = Some(text);
        o.truncated = true;
        o
    }
}
#[cfg(test)]
mod output_tests {
    #[test]
    #[ignore = "manual ignored-tree traversal benchmark"]
    fn t2_ignored_tree_benchmark() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".gitignore"),
            "target/\ndist/\nbuild/\n.next/\n",
        )
        .unwrap();
        for folder in ["target", "dist", "build", ".next"] {
            let path = dir.path().join(folder);
            std::fs::create_dir(&path).unwrap();
            for i in 0..500 {
                std::fs::write(path.join(format!("{i}.txt")), "noise").unwrap();
            }
        }
        std::fs::write(dir.path().join("source.txt"), "needle").unwrap();
        let start = std::time::Instant::now();
        let baseline = crate::util::walk(dir.path())
            .filter(|e| e.file_type().is_file())
            .inspect(|e| {
                std::fs::read(e.path()).unwrap();
            })
            .count();
        let baseline_time = start.elapsed();
        let start = std::time::Instant::now();
        let filtered = super::grep_files(dir.path(), false)
            .inspect(|e| {
                std::fs::read(e.path()).unwrap();
            })
            .count();
        let filtered_time = start.elapsed();
        assert_eq!(baseline, 2002);
        assert_eq!(filtered, 2);
        println!(
            "T2 traversal + file reads: {baseline} files {baseline_time:?}; filtered {filtered} files {filtered_time:?}"
        );
    }
    #[test]
    fn sh3_strips_terminal_sequences_and_overwritten_progress() {
        assert_eq!(super::decode(b"\x1b[31merror\x1b[0m\x1b[K\n\x1b[2J\x1b[?25l\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x07\n10%\r20%\r100%\r\n"), "error\nlink\n100%\n");
        assert_eq!(super::decode(b"ok\x1b[31"), "ok");
    }
}
