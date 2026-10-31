use crate::{
    attachments::Attachments,
    configuration::Configuration,
    context,
    extensions::Catalog,
    goals::{Goal, Spec},
    mcp::McpRegistry,
    models::{Block, Budget, Event, Message, Outcome, RunResult, ToolSpec, Usage},
    provider::{Provider, ProviderError, normalize_tool_ids, validate_response},
    store::{self, Store},
    tools::{self, Sink, ToolContext},
    util,
};
use anyhow::{Context, Result, bail};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Mechanisms {
    pub compression: bool,
    pub cache: bool,
    pub subagents: bool,
    pub prompt: bool,
}
impl Default for Mechanisms {
    fn default() -> Self {
        Self {
            compression: true,
            cache: true,
            subagents: true,
            prompt: true,
        }
    }
}
#[derive(Clone)]
pub struct Settings {
    pub mechanisms: Mechanisms,
    pub temperature: Option<f64>,
    pub seed: Option<u64>,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub budget: Budget,
    pub token_budget_override: Option<i64>,
    pub acceptance: Option<PathBuf>,
    pub always_allow: HashSet<String>,
    pub pending_model: bool,
    pub registry_dirty: bool,
    pub permission_rules: Value,
    pub shell_sandbox: Option<bool>,
}
impl Settings {
    pub fn new(configuration: &Configuration) -> Result<Self> {
        Ok(Self {
            mechanisms: Mechanisms::default(),
            temperature: None,
            seed: None,
            model: configuration.default_model()?,
            effort: "off".into(),
            mode: "default".into(),
            budget: Budget::default(),
            token_budget_override: None,
            acceptance: None,
            always_allow: HashSet::new(),
            pending_model: false,
            registry_dirty: false,
            permission_rules: Value::Null,
            shell_sandbox: None,
        })
    }
}

pub struct Snapshot {
    pub queued_id: Option<String>,
    pub queue_paused: bool,
    pub session: Option<String>,
    pub run_id: Option<String>,
    pub phase: String,
    pub running: bool,
    pub pending: bool,
    pub rounds: u64,
    pub usage: Option<Usage>,
    pub active_model: String,
    pub branch: Option<String>,
    pub window: u64,
    pub breakdown: Value,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            queued_id: None,
            queue_paused: false,
            session: None,
            run_id: None,
            phase: "idle".into(),
            running: false,
            pending: false,
            rounds: 0,
            usage: None,
            active_model: String::new(),
            branch: None,
            window: 200_000,
            breakdown: json!({"system":0,"tools":0,"messages":0}),
        }
    }
}
pub type Approvals = Arc<Mutex<HashMap<String, (String, oneshot::Sender<bool>)>>>;
#[derive(Clone)]
pub struct Hooks {
    pub settings: Arc<Mutex<Settings>>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub approvals: Approvals,
    pub approval_sequence: Arc<AtomicU64>,
    pub sink: Sink,
    pub cancel: CancellationToken,
    pub shared: Arc<Mutex<Ledger>>,
}
#[derive(Default)]
pub struct Ledger {
    pub usage: Usage,
    pub rounds: u64,
    pub started: Option<Instant>,
    pub deadline: Option<Instant>,
}

struct RetryPolicy {
    max_retries: u32,
    base_s: f64,
    max_s: f64,
    retry_after_max_s: f64,
}
impl RetryPolicy {
    fn from_env() -> Self {
        fn seconds(name: &str, default: f64) -> f64 {
            std::env::var(name)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(|s| s.min(60.0))
                .unwrap_or(default)
        }
        Self {
            max_retries: std::env::var("MINICODE_PROVIDER_MAX_RETRIES")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .map(|n| n.min(8))
                .unwrap_or(6),
            base_s: seconds("MINICODE_PROVIDER_RETRY_BASE_S", 1.0),
            max_s: seconds("MINICODE_PROVIDER_RETRY_MAX_S", 30.0),
            retry_after_max_s: seconds("MINICODE_PROVIDER_RETRY_AFTER_MAX_S", 60.0),
        }
    }
    fn delay(&self, attempt: u32, retry_after: Option<Duration>, random: f64) -> Duration {
        let seconds = if let Some(retry_after) = retry_after {
            retry_after.as_secs_f64().min(self.retry_after_max_s)
        } else {
            (self.base_s * 2f64.powi(attempt.min(16) as i32)).min(self.max_s) * random
        };
        Duration::from_secs_f64(seconds)
    }
}
fn retry_random() -> f64 {
    let id = uuid::Uuid::new_v4();
    let mut bytes = [0u8; 8];
    bytes[..6].copy_from_slice(&id.as_bytes()[..6]);
    u64::from_le_bytes(bytes) as f64 / ((1u64 << 48) - 1) as f64
}
fn genuine_user_text(message: &Message) -> Option<String> {
    if message.role != "user"
        || message
            .content
            .iter()
            .any(|block| !matches!(block, Block::Text { .. } | Block::Media { .. }))
    {
        return None;
    }
    let text = message.text();
    let trimmed = text.trim_start();
    (!trimmed.is_empty()
        && !trimmed.starts_with("<system-reminder>")
        && !trimmed.starts_with("此前历史已归档"))
    .then_some(text)
}

fn delegate_requirements(messages: &[Message]) -> String {
    const LIMIT: usize = 16_384;
    const TRUNCATED: &str = "\n[…已截断…]\n";
    let mut remaining = LIMIT;
    let mut selected = vec![];
    for mut text in messages.iter().rev().filter_map(genuine_user_text).take(12) {
        let separator = usize::from(!selected.is_empty());
        let available = remaining.saturating_sub(separator);
        if available <= TRUNCATED.len() {
            break;
        }
        if text.len() > available {
            // Prefer recent requests; keep both constraints at the beginning
            // and the final ask when one request alone exceeds the byte cap.
            let half = (available - TRUNCATED.len()) / 2;
            let head = util::prefix(&text, half);
            let mut tail = text.len() - (available - TRUNCATED.len() - head.len());
            while !text.is_char_boundary(tail) {
                tail += 1;
            }
            text = format!("{head}{TRUNCATED}{}", &text[tail..]);
        }
        remaining -= text.len() + separator;
        selected.push(text);
    }
    selected.reverse();
    selected.join("\n")
}

fn prompt_sources(sources: &Value) -> BTreeMap<PathBuf, String> {
    sources
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some((
                PathBuf::from(value["path"].as_str()?),
                value["sha256"].as_str()?.to_string(),
            ))
        })
        .collect()
}

pub struct Runtime {
    pub session: String,
    pub workspace: PathBuf,
    pub store: Store,
    pub catalog: Arc<tokio::sync::Mutex<Catalog>>,
    pub provider: Provider,
    pub messages: Vec<Message>,
    pub message_revision: Option<i64>,
    pub usage: Usage,
    pub rounds: u64,
    pub pending: bool,
    pub goal: Option<Goal>,
    pub mcp: Arc<McpRegistry>,
    pub background: Arc<crate::background::Jobs>,
    ownership: Mutex<Option<store::SessionLock>>,
    submitted_result: Option<Value>,
    isolation: Option<Arc<crate::delegate_workspace::Workspace>>,
    isolations: HashMap<String, Arc<crate::delegate_workspace::Workspace>>,
    permissions: Arc<crate::permissions::Rules>,
    permission_workspace: PathBuf,
    permission_settings: PathBuf,
    sandbox: crate::sandbox::Config,
    pub allowed: Option<HashSet<String>>,
    pub child: bool,
    pub children: HashMap<(String, String), Box<Runtime>>,
    pub custom_prompt: Option<String>,
    pub compaction: bool,
    pub mechanisms: Mechanisms,
    delegation_branch: bool,
    used_call_ids: HashSet<String>,
    /// Instruction and skill drift found on resume, reported by `prepare`.
    instruction_changes: Vec<Value>,
    /// The session's stable prompt and the instruction versions included in it.
    frozen_system: Option<String>,
    frozen_sources: BTreeMap<PathBuf, String>,
    dynamic_hash: Option<String>,
    /// Files this session has read or written; shared with its delegates.
    pub files: tools::Files,
}
impl Runtime {
    pub fn holds_resources(&self) -> bool {
        self.background.has_jobs()
            || self.mcp.has_live_connections()
            || self.isolations.values().any(|workspace| workspace.active())
            || self.children.values().any(|c| c.holds_resources())
    }
    pub async fn new(
        workspace: &Path,
        model: &str,
        session: Option<&str>,
        store: Store,
        configuration: Configuration,
        settings: &Settings,
    ) -> Result<Self> {
        let workspace = util::canonical(workspace)?;
        let catalog = Catalog::scan(&workspace, configuration, true)?;
        Self::from_catalog(workspace, model, session, store, catalog, settings).await
    }
    async fn from_catalog(
        workspace: PathBuf,
        model: &str,
        session: Option<&str>,
        store: Store,
        mut catalog: Catalog,
        settings: &Settings,
    ) -> Result<Self> {
        if !workspace.is_dir() {
            bail!("workspace no longer exists")
        }
        let provider = Provider::for_settings(
            &Settings {
                model: model.into(),
                ..settings.clone()
            },
            &catalog.configuration,
            None,
        )?;
        let permission_settings = catalog
            .configuration
            .path
            .parent()
            .context("configuration has no parent")?
            .to_path_buf();
        let permissions = Arc::new(crate::permissions::Rules::load(
            &workspace,
            &permission_settings,
            &settings.permission_rules,
            &settings.always_allow,
        )?);
        let sandbox =
            crate::sandbox::Config::load(&workspace, &permission_settings, settings.shell_sandbox)?;
        let (id, messages, message_revision, usage, rounds, pending) = if let Some(id) = session {
            let summary = store.require(id)?;
            if util::canonical(Path::new(
                summary["workspace"].as_str().context("invalid workspace")?,
            ))? != workspace
            {
                bail!("会话不属于当前工作区")
            }
            let (messages, revision) = store.versioned_messages(id)?;
            (
                id.to_string(),
                messages,
                revision,
                Usage {
                    input_tokens: summary["input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: summary["output_tokens"].as_u64().unwrap_or(0),
                    cache_read_tokens: summary["cache_read_tokens"].as_u64().unwrap_or(0),
                    cache_write_tokens: summary["cache_write_tokens"].as_u64().unwrap_or(0),
                    available: summary["usage_available"] == true
                        && (summary["rounds"].as_u64().unwrap_or(0) > 0
                            || [
                                "input_tokens",
                                "output_tokens",
                                "cache_read_tokens",
                                "cache_write_tokens",
                            ]
                            .iter()
                            .any(|key| summary[*key].as_u64().unwrap_or(0) > 0)),
                },
                summary["rounds"].as_u64().unwrap_or(0),
                summary["status"] != "completed",
            )
        } else {
            (
                store.create(&workspace.to_string_lossy(), &provider.name, model)?,
                vec![],
                0,
                Usage::default(),
                0,
                false,
            )
        };
        let goal = if let Some(saved) = store.goal(&id)? {
            Some(Goal::restore(&saved, &workspace)?)
        } else if let Some(path) = &settings.acceptance {
            Some(Goal::new(Spec::read(path)?, &workspace)?)
        } else {
            None
        };
        let events = store.recovery_events(&id)?;
        let mut instruction_changes = vec![];
        if session.is_some() && events.iter().any(|e| e.r#type == "session_start") {
            let global = catalog
                .configuration
                .path
                .parent()
                .unwrap()
                .join("AGENTS.md");
            let mut expected = BTreeMap::<PathBuf, String>::new();
            let mut active = BTreeMap::new();
            for e in &events {
                if e.r#type == "session_start" {
                    if let Some(sources) = e.data["project_instructions"].as_array() {
                        for source in sources {
                            if source["scope"] != "global" {
                                expected.insert(
                                    PathBuf::from(util::string(source, "path")?),
                                    util::string(source, "sha256")?.into(),
                                );
                            }
                        }
                    }
                    if let Some(a) = e.data["active_skills"].as_object() {
                        for (name, v) in a {
                            active.insert(name.clone(), v.as_str().unwrap_or_default().to_string());
                        }
                    }
                } else if e.r#type == "project_instructions" && e.data["scope"] != "global" {
                    expected.insert(
                        PathBuf::from(util::string(&e.data, "path")?),
                        util::string(&e.data, "sha256")?.into(),
                    );
                } else if e.r#type == "skill_activated" {
                    active.insert(
                        util::string(&e.data, "name")?.into(),
                        util::string(&e.data, "sha256")?.into(),
                    );
                } else if e.r#type == "skill_deactivated" {
                    active.remove(e.data["name"].as_str().unwrap_or_default());
                } else if e.r#type == "project_instructions_changed" {
                    for change in e.data["changes"].as_array().into_iter().flatten() {
                        let sha = change["sha256"].as_str().unwrap_or_default().to_string();
                        match (change["kind"].as_str(), change["status"].as_str()) {
                            (Some("instructions"), Some("deleted")) => {
                                expected.remove(&PathBuf::from(util::string(change, "path")?));
                            }
                            (Some("instructions"), _) => {
                                expected.insert(PathBuf::from(util::string(change, "path")?), sha);
                            }
                            (Some("skill"), Some("removed")) => {
                                active.remove(change["name"].as_str().unwrap_or_default());
                            }
                            (Some("skill"), _) => {
                                active.insert(util::string(change, "name")?.into(), sha);
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Drift is not fatal: the current version is re-pinned and the
            // model is told about it before the next request (see prepare).
            for path in catalog.instructions.keys() {
                if *path != global && !expected.contains_key(path) {
                    instruction_changes.push(json!({"kind":"instructions","path":path,"sha256":catalog.instructions[path].digest,"status":"added"}));
                }
            }
            for (path, sha) in expected {
                if path == global {
                    continue;
                }
                if !path.is_file() {
                    catalog.instructions.remove(&path);
                    instruction_changes.push(json!({"kind":"instructions","path":path,"previous_sha256":sha,"status":"deleted"}));
                    continue;
                }
                if !util::canonical(&path)?.starts_with(&workspace) {
                    bail!("project instructions outside workspace")
                }
                let source = crate::extensions::Source::read(&path, 64_000)?;
                if source.digest != sha {
                    instruction_changes.push(json!({"kind":"instructions","path":path,"previous_sha256":sha,"sha256":source.digest,"status":"changed"}));
                }
                catalog.instructions.insert(path, source);
            }
            for (name, sha) in active {
                if !catalog.skills.contains_key(&name) {
                    instruction_changes.push(json!({"kind":"skill","name":name,"previous_sha256":sha,"status":"removed"}));
                    continue;
                }
                let digest = catalog.load(&name)?.digest;
                if digest != sha {
                    instruction_changes.push(json!({"kind":"skill","name":name,"previous_sha256":sha,"sha256":digest,"status":"changed"}));
                }
            }
        }
        let used_call_ids = messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| {
                if let Block::ToolUse { id, .. } = b {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .chain(events.iter().flat_map(|e| {
                e.data["tool_call_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str().map(str::to_string))
            }))
            .chain(
                events
                    .iter()
                    .filter_map(|e| e.data["call_id"].as_str().map(str::to_string)),
            )
            .collect();
        let frozen = store.frozen_prompt(&id)?;
        let frozen_system = frozen.as_ref().map(|(system, _, _)| system.clone());
        let frozen_sources = frozen
            .as_ref()
            .map(|(_, sources, _)| prompt_sources(sources))
            .unwrap_or_default();
        let dynamic_hash = frozen.and_then(|(_, _, hash)| hash);
        let submitted_result = store.submitted_result(&id)?;
        if let Some(value) = &submitted_result {
            crate::subagent_result::validate(value)?;
        }
        let mut runtime = Self {
            session: id,
            permission_workspace: workspace.clone(),
            workspace,
            store,
            catalog: Arc::new(tokio::sync::Mutex::new(catalog)),
            provider,
            messages,
            message_revision: Some(message_revision),
            usage,
            rounds,
            pending,
            goal,
            mcp: Arc::new(McpRegistry::default()),
            background: Default::default(),
            ownership: Default::default(),
            submitted_result,
            isolation: None,
            isolations: HashMap::new(),
            permissions,
            permission_settings,
            sandbox,
            allowed: None,
            child: false,
            children: HashMap::new(),
            custom_prompt: None,
            compaction: settings.mechanisms.compression,
            mechanisms: settings.mechanisms.clone(),
            delegation_branch: false,
            used_call_ids,
            instruction_changes,
            frozen_system,
            frozen_sources,
            dynamic_hash,
            files: Default::default(),
        };
        for event in events
            .iter()
            .filter(|event| event.r#type == "subagent_isolation")
        {
            let id = util::string(&event.data, "isolation_id")?;
            let call = util::string(&event.data, "call_id")?;
            let existing = runtime
                .isolations
                .values()
                .find(|workspace| workspace.id == id)
                .cloned();
            if let Some(workspace) = existing {
                runtime.isolations.insert(call.into(), workspace);
            } else if let Some(workspace) = crate::delegate_workspace::Workspace::load(
                &runtime.store.root(),
                &runtime.session,
                &runtime.workspace,
                id,
            )? {
                runtime.isolations.insert(call.into(), Arc::new(workspace));
            }
        }
        if let Some(start) = events
            .iter()
            .find(|event| event.r#type == "session_start" && event.data["child"] == true)
        {
            runtime.child = true;
            runtime.allowed = Some(
                serde_json::from_value(start.data["allowed_tools"].clone())
                    .context("invalid saved child whitelist")?,
            );
            if let Some(isolation) = start.data["isolation"].as_object() {
                let id = isolation
                    .get("id")
                    .and_then(Value::as_str)
                    .context("invalid saved child isolation")?;
                let owner = isolation
                    .get("owner")
                    .and_then(Value::as_str)
                    .context("invalid saved child owner")?;
                let parent = isolation
                    .get("parent_workspace")
                    .and_then(Value::as_str)
                    .context("invalid saved child workspace")?;
                let workspace = crate::delegate_workspace::Workspace::load(
                    &runtime.store.root(),
                    owner,
                    Path::new(parent),
                    id,
                )?
                .context("isolated child workspace is closed")?;
                if util::canonical(&workspace.work())? != runtime.workspace {
                    bail!("saved child isolation mismatch")
                }
                runtime.isolation = Some(Arc::new(workspace));
                runtime.permission_workspace = PathBuf::from(parent);
            }
        }
        let child_ids = events
            .iter()
            .filter(|e| e.r#type == "subagent_start")
            .filter_map(|e| e.data["child_session_id"].as_str())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let mut recovered_usage = Usage::default();
        for event in events.iter().filter(|e| {
            matches!(
                e.r#type.as_str(),
                "assistant_message" | "provider_response_rejected"
            )
        }) {
            if let Ok(usage) = serde_json::from_value::<Usage>(event.data["usage"].clone()) {
                recovered_usage.add(&usage);
            }
        }
        let mut recovery_events = events;
        for child_id in child_ids {
            let child_events = runtime.store.recovery_events(&child_id)?;
            let mut usage = Usage::default();
            for event in child_events.iter().filter(|e| {
                matches!(
                    e.r#type.as_str(),
                    "assistant_message" | "provider_response_rejected"
                )
            }) {
                if let Ok(response_usage) =
                    serde_json::from_value::<Usage>(event.data["usage"].clone())
                {
                    usage.add(&response_usage);
                }
            }
            if let Some(summary) = runtime.store.session(&child_id)? {
                usage.input_tokens = usage
                    .input_tokens
                    .max(summary["input_tokens"].as_u64().unwrap_or(0));
                usage.output_tokens = usage
                    .output_tokens
                    .max(summary["output_tokens"].as_u64().unwrap_or(0));
                usage.cache_read_tokens = usage
                    .cache_read_tokens
                    .max(summary["cache_read_tokens"].as_u64().unwrap_or(0));
                usage.cache_write_tokens = usage
                    .cache_write_tokens
                    .max(summary["cache_write_tokens"].as_u64().unwrap_or(0));
                if summary["rounds"].as_u64().unwrap_or(0) > 0 {
                    usage.available |= summary["usage_available"] == true;
                }
            }
            recovered_usage.add(&usage);
            recovery_events.extend(child_events);
        }
        recovered_usage.available |= runtime.usage.available;
        if recovered_usage.total() > runtime.usage.total() {
            runtime.usage = recovered_usage;
        } else {
            runtime.usage.available |= recovered_usage.available;
        }
        if session.is_some()
            && (runtime.store.require(&runtime.session)?["usage_available"] == true)
                != runtime.usage.available
        {
            runtime.store.update(
                &runtime.session,
                &json!({"usage_available":runtime.usage.available}),
            )?;
        }
        runtime
            .provider
            .restore_script_progress(&recovery_events, &runtime.used_call_ids);
        Ok(runtime)
    }
    pub fn set_script(&mut self, script: Value) -> Result<()> {
        self.provider.set_script(script)?;
        let mut events = self.store.recovery_events(&self.session)?;
        let children = events
            .iter()
            .filter(|e| e.r#type == "subagent_start")
            .filter_map(|e| e.data["child_session_id"].as_str().map(str::to_string))
            .collect::<HashSet<_>>();
        for id in children {
            events.extend(self.store.recovery_events(&id)?);
        }
        self.provider
            .restore_script_progress(&events, &self.used_call_ids);
        Ok(())
    }
    pub fn event(&self, hooks: &Hooks, kind: &str, data: Value) -> Result<Event> {
        let event = self.store.event(&self.session, kind, &data)?;
        (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
        Ok(event)
    }
    pub fn phase(hooks: &Hooks, phase: &str) {
        hooks.snapshot.lock().unwrap().phase = phase.into();
    }
    pub async fn refresh_snapshot(&self, hooks: &Hooks) -> Result<()> {
        let catalog = self.catalog.lock().await;
        let agents = catalog.configuration.agents()?;
        let specs = self.tool_specs(&catalog, &agents);
        let system = self.system(&catalog, &specs)?;
        let branch = crate::environment::git_branch(&self.workspace);
        let mut s = hooks.snapshot.lock().unwrap();
        s.branch = branch;
        s.session = Some(self.session.clone());
        s.pending = self.pending;
        s.rounds = self.rounds;
        s.usage = Some(self.usage.clone());
        s.active_model = self.provider.configuration_id.clone();
        s.window = self.provider.window;
        let mut breakdown = context::breakdown(&system, &self.messages, &specs);
        if let Some(values) = breakdown.as_object_mut() {
            let total = self
                .provider
                .estimated_prompt_tokens(&system, &self.messages, &specs);
            let raw = values
                .values()
                .filter_map(Value::as_u64)
                .sum::<u64>()
                .max(1);
            for value in values.values_mut() {
                *value = json!(
                    (value.as_u64().unwrap_or(0) as u128 * total as u128 / raw as u128) as u64
                );
            }
            let assigned = values.values().filter_map(Value::as_u64).sum::<u64>();
            let messages = values.get("messages").and_then(Value::as_u64).unwrap_or(0);
            values.insert(
                "messages".into(),
                json!(messages.saturating_add(total.saturating_sub(assigned))),
            );
        }
        s.breakdown = breakdown;
        Ok(())
    }
    fn tool_specs(&self, catalog: &Catalog, agents: &[Value]) -> Vec<crate::models::ToolSpec> {
        let mut specs = tools::specs(catalog, &self.mcp, agents, !self.child);
        if !self.mechanisms.subagents {
            specs.retain(|s| !["delegate", "review_delegate"].contains(&s.name.as_str()));
        }
        if let Some(allowed) = &self.allowed {
            specs.retain(|s| allowed.contains(&s.name));
        }
        if self.child && self.isolation.is_none() {
            specs.retain(|spec| {
                tools::read_only(&spec.name)
                    || spec.name.starts_with("mcp__") && spec.read_only_hint
            });
        }
        specs
    }
    fn system(&self, catalog: &Catalog, specs: &[crate::models::ToolSpec]) -> Result<String> {
        if let Some(system) = &self.frozen_system {
            return Ok(system.clone());
        }
        let mut prompt = catalog.prompt_with_template(
            &specs.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            self.mechanisms.prompt,
        )?;
        if let Some(custom) = &self.custom_prompt {
            prompt.push_str(custom)
        }
        Ok(prompt)
    }
    async fn sync_dynamic_context(&mut self) -> Result<()> {
        let (entries, active) = {
            let catalog = self.catalog.lock().await;
            let global = catalog
                .configuration
                .path
                .parent()
                .unwrap_or(Path::new("."))
                .join("AGENTS.md");
            let root = catalog.workspace.join("AGENTS.md");
            let mut roots = vec![global, root];
            roots.dedup();
            let mut entries = vec![];
            for path in &roots {
                match (
                    self.frozen_sources.get(path),
                    catalog.instructions.get(path),
                ) {
                    (Some(old), Some(current)) if old == &current.digest => {}
                    (None, None) => {}
                    (_, Some(current)) => entries.push(format!(
                        "{} 的当前指令（取代冻结版本，sha256:{}）：\n{}",
                        path.display(),
                        current.digest,
                        current.content
                    )),
                    (Some(_), None) => entries.push(format!(
                        "{} 已删除，其中冻结的旧指令不再生效。",
                        path.display()
                    )),
                }
            }
            for source in catalog
                .instructions
                .values()
                .filter(|source| !roots.contains(&source.path))
            {
                entries.push(format!(
                    "作用域 AGENTS.md：{} (sha256:{})，仅适用于其所在目录及子目录\n{}",
                    source.path.display(),
                    source.digest,
                    source.content
                ));
            }
            let mut active = vec![];
            for (name, digest) in &catalog.active {
                if let Some(skill) = catalog.skills.get(name) {
                    let source = crate::extensions::Source::read(&skill.path, 64_000)?;
                    if source.digest != *digest {
                        bail!("activated skill changed since session start: {name}")
                    }
                    active.push(name.clone());
                    entries.push(format!(
                        "Skill {name} (sha256:{digest})：\n{}",
                        source.content
                    ));
                }
            }
            (entries, active)
        };
        #[cfg(windows)]
        let entries = {
            let mut entries = entries;
            if let Some(frozen) = &self.frozen_system {
                let shell = tools::windows_shell_guidance();
                if !frozen.contains(&shell) {
                    entries.push(format!("当前命令环境（取代冻结提示中的旧说明）：{shell}"));
                }
            }
            entries
        };
        if entries.is_empty() && active.is_empty() && self.dynamic_hash.is_none() {
            return Ok(());
        }
        let body = format!(
            "当前有效的动态 AGENTS.md 指令与 Skills 如下；同一来源以本快照为准，已删除或停用的内容不再生效。\n当前启用的 Skills：{}\n{}",
            if active.is_empty() {
                "无".into()
            } else {
                active.join("、")
            },
            if entries.is_empty() {
                "当前没有额外动态指令。".into()
            } else {
                entries.join("\n\n")
            }
        );
        let hash = util::hash(body.as_bytes());
        let reminder = format!("<system-reminder>\n{body}\n</system-reminder>");
        if self.dynamic_hash.as_deref() == Some(hash.as_str())
            && self.messages.iter().any(|m| {
                m.role == "user"
                    && matches!(m.content.as_slice(), [Block::Text { text }] if text == &reminder)
            })
        {
            return Ok(());
        }
        let message = Message::user(reminder);
        self.message_revision = None;
        self.store
            .append_dynamic_context(&self.session, &hash, &message)?;
        self.messages.push(message);
        self.message_revision = Some(self.store.message_revision(&self.session)?);
        self.dynamic_hash = Some(hash);
        Ok(())
    }
    async fn connect_mcp(&mut self, hooks: &Hooks) -> Result<()> {
        if self.child
            && self
                .allowed
                .as_ref()
                .is_some_and(|set| !set.iter().any(|name| name.starts_with("mcp__")))
        {
            self.mcp.close().await;
            self.mcp = Arc::new(McpRegistry::default());
            return Ok(());
        }
        {
            let mut catalog = self.catalog.lock().await;
            *catalog = catalog.refresh()?;
        }
        let current_servers = self.catalog.lock().await.servers.clone();
        self.mcp.retire_changed(&current_servers).await;
        self.trust_plugins(hooks).await?;
        let mut servers = self.catalog.lock().await.servers.clone();
        if let Some(allowed) = &self.allowed {
            servers.retain(|server| {
                allowed
                    .iter()
                    .any(|name| name.starts_with(&format!("mcp__{}__", server.name)))
            });
        }
        let deadline = hooks.shared.lock().unwrap().deadline;
        let prepare = McpRegistry::reconcile(&self.mcp, servers);
        let registry = tokio::select! {
            _=hooks.cancel.cancelled()=>bail!("turn cancelled"),
            result=async {
                if let Some(deadline)=deadline {
                    tokio::time::timeout(deadline.saturating_duration_since(Instant::now()),prepare).await.map_err(|_|anyhow::anyhow!("turn deadline exceeded"))
                } else { Ok(prepare.await) }
            }=>result?
        };
        self.mcp = Arc::new(registry);
        for status in &self.mcp.statuses {
            self.event(hooks, "mcp_discovery", status.clone())?;
        }
        Ok(())
    }
    pub async fn prepare(&mut self, hooks: &Hooks) -> Result<()> {
        let existing = self.store.recovery_events(&self.session)?;
        if !existing.iter().any(|e| e.r#type == "session_start") {
            let catalog = self.catalog.lock().await;
            let global = catalog
                .configuration
                .path
                .parent()
                .unwrap()
                .join("AGENTS.md");
            self.event(hooks,"session_start",json!({"workspace":self.workspace,"provider":self.provider.name,"model":self.provider.configuration_id,"budget":hooks.settings.lock().unwrap().budget,"project_instructions":catalog.instructions.values().map(|s|s.metadata(&global)).collect::<Vec<_>>(),"active_skills":catalog.active,"provider_resume_identity":self.provider.identity(),"child":self.child,"allowed_tools":self.allowed,"isolation":self.isolation.as_ref().map(|workspace|workspace.metadata())}))?;
        }
        if let Some(goal) = &self.goal {
            goal.save(&self.store, &self.session)?;
        }
        let mut answered = HashSet::new();
        let mut pending = vec![];
        for b in self.messages.iter().flat_map(|m| &m.content) {
            if let Block::ToolResult { tool_use_id, .. } = b {
                answered.insert(tool_use_id.clone());
            }
        }
        for b in self.messages.iter().flat_map(|m| &m.content) {
            if let Block::ToolUse { id, .. } = b
                && !answered.contains(id)
            {
                pending.push(b.clone());
            }
        }
        let ids = pending
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let journal = self.store.tool_results_for(&self.session, &ids)?;
        let execution = self.store.tool_execution(&self.session)?;
        let mut results = vec![];
        for call in pending {
            let Block::ToolUse { id, name, .. } = &call else {
                continue;
            };
            let result = if let Some(result) = journal.get(id) {
                result.clone()
            } else if execution.get(id) == Some(&false) {
                self.record_result(
                    &call,
                    Outcome::failure("上次运行已中断，未执行"),
                    hooks,
                    true,
                    0.0,
                )?
            } else if matches!(
                &call,
                Block::ToolUse {
                    input_error: Some(_),
                    ..
                }
            ) || tools::replay_safe(name)
            {
                self.invoke(call.clone(), hooks, true).await?
            } else {
                self.event(
                    hooks,
                    "side_effect_unknown",
                    json!({"call_id":id,"name":name}),
                )?;
                Block::ToolResult {
                    tool_use_id: id.clone(),
                    content: format!(
                        "副作用状态未知：进程在「{name}」执行后、结果落库前被中断。不要假设它已执行或未执行；请先用只读工具核实当前状态再继续。"
                    ),
                    is_error: false,
                    images: vec![],
                }
            };
            results.push(result);
        }
        if !results.is_empty() {
            self.append(Message {
                role: "user".into(),
                content: results,
            })?;
        }
        let mut jobs = HashMap::new();
        for event in existing {
            match event.r#type.as_str() {
                "background_job_started" => {
                    jobs.insert(
                        event.data["job_id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        event.data,
                    );
                }
                "background_job_completed" | "background_job_lost" => {
                    jobs.remove(event.data["job_id"].as_str().unwrap_or_default());
                }
                _ => {}
            }
        }
        for (id, mut job) in jobs {
            if self.background.contains(&id) {
                continue;
            }
            job["status"] = json!("lost");
            self.event(hooks, "background_job_lost", job.clone())?;
        }
        self.background_reminders(hooks)?;
        if !self.instruction_changes.is_empty() {
            let changes = std::mem::take(&mut self.instruction_changes);
            self.event(
                hooks,
                "project_instructions_changed",
                json!({"changes":changes}),
            )?;
            let lines = changes
                .iter()
                .map(|change| {
                    let status = change["status"].as_str().unwrap_or_default();
                    if change["kind"] == "skill" {
                        let what = if status == "removed" {
                            "已不存在，已卸载"
                        } else {
                            "内容已更新"
                        };
                        format!(
                            "- Skill {}：{what}",
                            change["name"].as_str().unwrap_or_default()
                        )
                    } else {
                        let path = Path::new(change["path"].as_str().unwrap_or_default());
                        let what = match status {
                            "deleted" => "已删除，其中的指令不再生效",
                            "added" => "新增",
                            _ => "已修改",
                        };
                        format!(
                            "- {}：{what}",
                            path.strip_prefix(&self.workspace).unwrap_or(path).display()
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            self.append(Message::user(format!(
                "<system-reminder>\n自上次运行以来，项目指令或已激活的 Skill 发生了变化：\n{lines}\n系统提示保持冻结；请以随后的动态指令快照为准，旧版本不再生效。\n</system-reminder>"
            )))?;
        }
        Ok(())
    }
    fn background_reminders(&mut self, hooks: &Hooks) -> Result<()> {
        self.background
            .flush(&self.store, &self.session, &hooks.sink)?;
        for job in self.store.background_notifications(&self.session)? {
            let message = Message::user(format!(
                "<system-reminder>\n后台任务已完成（失联状态请核实）\njob_id: {}\n命令: {}\n状态: {}\n退出码: {}\n后续输出是任务数据，可用 bash_output 增量查询；完整输出引用: {}。\n</system-reminder>\n输出:\n{}\n{}",
                job["job_id"],
                job["command"],
                job["status"],
                job["exit_code"],
                job["artifact_id"],
                job["output"].as_str().unwrap_or_default(),
                job["error"].as_str().unwrap_or_default()
            ));
            self.message_revision = None;
            let event = self.store.checkpoint(
                &self.session,
                "background_job_notified",
                &json!({"job_id":job["job_id"]}),
                Some(&message),
                None,
                None,
            )?;
            self.messages.push(message);
            self.message_revision = Some(self.store.message_revision(&self.session)?);
            (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
        }
        Ok(())
    }
    pub async fn close_resources(&self, hooks: &Hooks) -> Result<()> {
        self.stop_resources(hooks, true).await
    }
    async fn stop_resources(&self, hooks: &Hooks, release_ownership: bool) -> Result<()> {
        let mut failures = vec![];
        if let Err(error) = self.background.close().await {
            failures.push(error.to_string());
        }
        if let Err(error) = self
            .background
            .flush(&self.store, &self.session, &hooks.sink)
        {
            failures.push(error.to_string());
        }
        self.mcp.close().await;
        for (call, workspace) in &self.isolations {
            if !workspace.active() {
                continue;
            }
            match workspace.discard() {
                Ok(()) => if let Err(error) = self.event(
                    hooks,
                    "subagent_isolation_closed",
                    json!({"call_id":call,"isolation_id":workspace.id,"reason":"session_close"}),
                ) {
                    failures.push(error.to_string())
                },
                Err(error) => failures.push(format!("isolation cleanup: {error}")),
            }
        }
        for child in self.children.values() {
            if let Err(error) = Box::pin(child.close_resources(hooks)).await {
                failures.push(error.to_string());
            }
        }
        if release_ownership && !self.isolations.values().any(|workspace| workspace.active()) {
            self.ownership.lock().unwrap().take();
        }
        if !failures.is_empty() {
            bail!("{}", failures.join("; "));
        }
        Ok(())
    }
    fn append(&mut self, message: Message) -> Result<()> {
        self.message_revision = None;
        self.store.append(&self.session, &message)?;
        self.messages.push(message);
        self.message_revision = Some(self.store.message_revision(&self.session)?);
        Ok(())
    }
    fn consume_queued(&mut self, item: &Value, hooks: &Hooks) -> Result<()> {
        let state = hooks.snapshot.lock().unwrap();
        if state.queue_paused
            || !self
                .store
                .queued_prompts(&self.session)?
                .iter()
                .any(|queued| queued["id"] == item["id"])
        {
            return Ok(());
        }
        let mut content: Vec<Block> =
            serde_json::from_value(item["attachments"].clone()).unwrap_or_default();
        content.push(Block::text(util::string(item, "text")?));
        let message = Message {
            role: "user".into(),
            content,
        };
        self.message_revision = None;
        let event = self.store.checkpoint(
            &self.session,
            "queue_consumed",
            &json!({"id":item["id"],"mode":item["mode"],"text":item["text"]}),
            Some(&message),
            None,
            None,
        )?;
        self.messages.push(message);
        self.message_revision = Some(self.store.message_revision(&self.session)?);
        drop(state);
        (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
        Ok(())
    }
    pub async fn run(
        &mut self,
        text: Option<String>,
        attachments: Vec<Block>,
        hooks: Hooks,
    ) -> Result<RunResult> {
        if text.is_none() && !self.pending {
            bail!("当前没有暂停的任务");
        }
        let ownership = if self.ownership.lock().unwrap().is_some() {
            None
        } else {
            Some(self.store.activation(&self.session)?)
        };
        self.store.require(&self.session)?;
        if Some(self.store.message_revision(&self.session)?) != self.message_revision {
            bail!("会话已被另一个进程更新，请重新加载会话后继续；本次没有执行任何操作")
        }
        if text.is_none()
            && self.store.require(&self.session)?["exit_reason"] == "goal_not_met"
            && let Some(goal) = &mut self.goal
        {
            goal.attempts = 0;
        }
        let started = Instant::now();
        let budget = hooks.settings.lock().unwrap().budget.clone();
        {
            let mut snapshot = hooks.snapshot.lock().unwrap();
            if !snapshot.running || snapshot.run_id.is_none() {
                snapshot.run_id = Some(util::id());
            }
            snapshot.running = true;
            snapshot.phase = "preparing".into();
        }
        if !self.child {
            let mut ledger = hooks.shared.lock().unwrap();
            ledger.usage = self.usage.clone();
            ledger.rounds = 0;
            ledger.started = Some(started);
            ledger.deadline = if budget.max_seconds > 0.0 {
                Duration::try_from_secs_f64(budget.max_seconds)
                    .ok()
                    .and_then(|duration| started.checked_add(duration))
            } else {
                None
            };
        }
        // A panic in the loop is caught so the teardown below still runs; it
        // is then reported as a run failure. Persisted history stays the
        // source of truth for the next run.
        let (result, panicked) =
            match std::panic::AssertUnwindSafe(self.run_inner(text, attachments, &hooks))
                .catch_unwind()
                .await
            {
                Ok(result) => (result, false),
                Err(panic) => (
                    Err(anyhow::anyhow!(
                        "internal panic: {}",
                        util::panic_message(&*panic)
                    )),
                    true,
                ),
            };
        // Teardown is best-effort: every step runs even when an earlier one
        // fails, and the failures are reported together at the end.
        let mut failures = vec![];
        if hooks.cancel.is_cancelled()
            && let Err(error) = self.backfill_cancelled(&hooks)
        {
            failures.push(format!("cancelled calls: {error}"));
        }
        if (hooks.cancel.is_cancelled() || result.is_err())
            && let Err(error) = self.stop_resources(&hooks, false).await
        {
            failures.push(format!("background jobs: {error}"));
        }
        if let Err(error) = self
            .background
            .flush(&self.store, &self.session, &hooks.sink)
        {
            failures.push(format!("background events: {error}"));
        }
        if hooks.cancel.is_cancelled() || result.is_err() {
            self.mcp.close().await;
            self.mcp = Arc::new(McpRegistry::default());
        }
        {
            let mut approvals = hooks
                .approvals
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.child {
                let prefix = format!("{}:", self.session);
                approvals.retain(|id, _| !id.starts_with(&prefix));
            } else {
                approvals.clear();
            }
        }
        let (reason, error) = match result {
            Ok(reason) => (reason, None),
            Err(error) => {
                let reason = if hooks.cancel.is_cancelled() {
                    "cancelled"
                } else if error.to_string() == "turn deadline exceeded" {
                    "time_budget"
                } else if error.to_string().starts_with("provider ")
                    || error.to_string().contains("SSE")
                {
                    "provider_error"
                } else {
                    "internal_error"
                };
                (reason.into(), Some(error.to_string()))
            }
        };
        self.pending = reason != "completed";
        let mut counters = store::counters(self.rounds, &self.usage);
        counters["status"] = json!(if self.pending { "paused" } else { "completed" });
        counters["exit_reason"] = json!(reason);
        let run_id = hooks
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .run_id
            .clone();
        let mut data = json!({"exit_reason":reason,"status":counters["status"],"rounds":self.rounds,"total_usage":self.usage,"run_id":run_id});
        data["total_usage"]["total_tokens"] = json!(self.usage.total());
        if let Some(error) = &error {
            data["error"] = json!(error)
        }
        match self.store.checkpoint(
            &self.session,
            "session_end",
            &data,
            None,
            None,
            Some(&counters),
        ) {
            Ok(event) => {
                (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}))
            }
            Err(e) => failures.push(format!("session_end: {e}")),
        }
        if let Err(e) = self.refresh_snapshot(&hooks).await {
            failures.push(format!("snapshot: {e}"));
        }
        {
            let mut s = hooks
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            s.running = false;
            if self.pending {
                s.queue_paused = true;
            }
            s.phase = if self.pending { "paused" } else { "idle" }.into();
        }
        if panicked || !failures.is_empty() {
            if let Err(error) = self.stop_resources(&hooks, false).await {
                failures.push(format!("background jobs: {error}"));
            }
            self.mcp = Arc::new(McpRegistry::default());
            let mut report = error.into_iter().collect::<Vec<_>>();
            if !failures.is_empty() {
                report.push(format!("run teardown failed: {}", failures.join("; ")));
            }
            if self.isolations.values().any(|workspace| workspace.active()) {
                if let Some(ownership) = ownership {
                    *self.ownership.lock().unwrap() = Some(ownership);
                }
            } else {
                self.ownership.lock().unwrap().take();
            }
            bail!(report.join("; "))
        }
        if !hooks.cancel.is_cancelled() && error.is_none() && self.holds_resources() {
            if let Some(ownership) = ownership {
                *self.ownership.lock().unwrap() = Some(ownership);
            }
        } else {
            self.ownership.lock().unwrap().take();
        }
        Ok(RunResult {
            session_id: self.session.clone(),
            exit_reason: reason,
            rounds: self.rounds,
            total_usage: self.usage.clone(),
            duration_s: started.elapsed().as_secs_f64(),
            error,
        })
    }
    fn check(hooks: &Hooks) -> Result<()> {
        if hooks.cancel.is_cancelled() {
            bail!("turn cancelled")
        };
        if hooks
            .shared
            .lock()
            .unwrap()
            .deadline
            .is_some_and(|d| Instant::now() >= d)
        {
            bail!("turn deadline exceeded")
        };
        Ok(())
    }
    pub fn context(&self, hooks: &Hooks, output: Option<Sink>) -> ToolContext {
        ToolContext {
            workspace: self.workspace.clone(),
            session: self.session.clone(),
            store: self.store.clone(),
            attachments: Attachments {
                root: self.store.root().join("attachments"),
            },
            modalities: self.provider.modalities.clone(),
            catalog: self.catalog.clone(),
            mcp: self.mcp.clone(),
            cancel: hooks.cancel.clone(),
            output,
            deadline: hooks.shared.lock().unwrap().deadline,
            files: self.files.clone(),
            permissions: self.permissions.clone(),
            sandbox: self.sandbox.clone(),
            force_sandbox: self.isolation.is_some(),
        }
    }
    async fn run_inner(
        &mut self,
        text: Option<String>,
        attachments: Vec<Block>,
        hooks: &Hooks,
    ) -> Result<String> {
        self.prepare(hooks).await?;
        Self::check(hooks)?;
        if self.child && self.submitted_result.is_some() {
            return Ok("completed".into());
        }
        if let Some(text) = text {
            let mut content = attachments;
            content.push(Block::text(text));
            let message = Message {
                role: "user".into(),
                content,
            };
            let queued_id = hooks.snapshot.lock().unwrap().queued_id.clone();
            if let Some(id) = queued_id {
                self.message_revision = None;
                let event = self.store.checkpoint(
                    &self.session,
                    "queue_consumed",
                    &json!({"id":id,"text":message.text()}),
                    Some(&message),
                    None,
                    None,
                )?;
                self.messages.push(message);
                self.message_revision = Some(self.store.message_revision(&self.session)?);
                hooks.snapshot.lock().unwrap().queued_id = None;
                (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
            } else {
                self.append(message)?;
            }
            self.pending = true;
            if let Some(goal) = &mut self.goal {
                goal.attempts = 0;
            }
        } else if !self.pending {
            bail!("当前没有暂停的任务")
        }
        self.store.update(
            &self.session,
            &json!({"status":"running","exit_reason":null}),
        )?;
        let budget = hooks.settings.lock().unwrap().budget.clone();
        self.event(hooks, "budget_changed", json!({"budget":budget}))?;
        self.connect_mcp(hooks).await?;
        let mut truncations = 0;
        let mut repeated = 0;
        let mut previous = None;
        loop {
            Self::check(hooks)?;
            let settings = hooks.settings.lock().unwrap().clone();
            self.permissions = Arc::new(crate::permissions::Rules::load(
                &self.permission_workspace,
                &self.permission_settings,
                &settings.permission_rules,
                &settings.always_allow,
            )?);
            self.sandbox = crate::sandbox::Config::load(
                &self.permission_workspace,
                &self.permission_settings,
                settings.shell_sandbox,
            )?;
            if settings.pending_model || settings.model != self.provider.configuration_id {
                let config = self.catalog.lock().await.configuration.clone();
                self.provider = Provider::for_settings(&settings, &config, Some(&self.provider))?;
                hooks.settings.lock().unwrap().pending_model = false;
                self.store.update(
                    &self.session,
                    &json!({"model":settings.model,"provider":self.provider.name}),
                )?;
            }
            {
                let ledger = hooks.shared.lock().unwrap();
                if budget.max_total_tokens > 0
                    && ledger.usage.total() >= budget.max_total_tokens as u64
                {
                    return Ok("token_budget".into());
                }
                if budget.max_rounds > 0 && ledger.rounds >= budget.max_rounds {
                    return Ok("max_rounds".into());
                }
            }
            self.background_reminders(hooks)?;
            if self.mcp.needs_reconnect() {
                self.connect_mcp(hooks).await?;
            }
            let (specs, mut system, sources) = {
                let mut catalog = self.catalog.lock().await;
                catalog.pending_scopes.clear();
                for source in catalog.reload_instructions()? {
                    self.event(hooks, "project_instructions", source)?;
                }
                let agents = catalog.configuration.agents()?;
                let specs = self.tool_specs(&catalog, &agents);
                let system = self.system(&catalog, &specs)?;
                let global = catalog
                    .configuration
                    .path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("AGENTS.md");
                let root = catalog.workspace.join("AGENTS.md");
                let sources = catalog
                    .instructions
                    .iter()
                    .filter(|(path, _)| **path == global || **path == root)
                    .map(|(path, source)| (path.clone(), source.digest.clone()))
                    .collect::<BTreeMap<_, _>>();
                (specs, system, sources)
            };
            if self.frozen_system.is_none() {
                let data = json!(
                    sources
                        .iter()
                        .map(|(path, sha)| json!({"path":path,"sha256":sha}))
                        .collect::<Vec<_>>()
                );
                let (saved, saved_sources, hash) =
                    self.store.freeze_prompt(&self.session, &system, &data)?;
                system = saved;
                self.frozen_system = Some(system.clone());
                self.frozen_sources = prompt_sources(&saved_sources);
                self.dynamic_hash = hash;
            }
            self.sync_dynamic_context().await?;
            if self.compaction && self.compact_context(hooks, &system, &specs, false).await? {
                self.sync_dynamic_context().await?;
            }
            let mut context_tokens =
                self.provider
                    .estimated_prompt_tokens(&system, &self.messages, &specs);
            // Offloading is a last resort after normal compaction. A large tool
            // result alone must not truncate a fresh, otherwise small context.
            if self.compaction && context_tokens.saturating_add(1024) >= self.provider.window {
                self.message_revision = None;
                let offloaded =
                    context::offload_large(&self.store, &self.session, &mut self.messages)?;
                if offloaded > 0 {
                    self.store.replace_messages(&self.session, &self.messages)?;
                    self.event(
                        hooks,
                        "context_compacted",
                        json!({"offloaded_blocks":offloaded}),
                    )?;
                    context_tokens =
                        self.provider
                            .estimated_prompt_tokens(&system, &self.messages, &specs);
                }
                self.message_revision = Some(self.store.message_revision(&self.session)?);
            }
            if context_tokens.saturating_add(1024) >= self.provider.window {
                return Ok("context_limit".into());
            }
            {
                let mut ledger = hooks.shared.lock().unwrap();
                if budget.max_total_tokens > 0
                    && ledger.usage.total() >= budget.max_total_tokens as u64
                {
                    return Ok("token_budget".into());
                }
                if budget.max_rounds > 0 && ledger.rounds >= budget.max_rounds {
                    return Ok("max_rounds".into());
                }
                ledger.rounds += 1;
            }
            self.rounds += 1;
            self.event(hooks, "round_start", json!({"round":self.rounds}))?;
            self.store
                .update(&self.session, &json!({"rounds":self.rounds}))?;
            Self::phase(hooks, "model");
            self.refresh_snapshot(hooks).await?;
            let sink = hooks.sink.clone();
            let session = self.session.clone();
            let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let saw = seen.clone();
            let delta: Sink = Arc::new(move |mut v| {
                saw.store(true, Ordering::Relaxed);
                v["sessionId"] = json!(session);
                sink(v);
            });
            let attachments = Attachments {
                root: self.store.root().join("attachments"),
            };
            let mut attempt = 0;
            let mut overflow_recovered = false;
            let retry_policy = RetryPolicy::from_env();
            let (mut response, mut metrics) = loop {
                if budget.max_total_tokens > 0
                    && hooks.shared.lock().unwrap().usage.total() >= budget.max_total_tokens as u64
                {
                    return Ok("token_budget".into());
                }
                // Only the turn deadline bounds the whole request; silence on
                // the wire is the provider's idle timeout, so long streams that
                // keep producing bytes are never cut off.
                let deadline = hooks.shared.lock().unwrap().deadline;
                let request =
                    self.provider
                        .stream(&system, &self.messages, &specs, &attachments, &delta);
                let value = tokio::select! {_ = hooks.cancel.cancelled()=>return Err(ProviderError::Cancelled.into()),r=async {
                    match deadline {
                        Some(d) => tokio::time::timeout(d.saturating_duration_since(Instant::now()), request).await.map_err(|_| anyhow::Error::from(ProviderError::Deadline)).and_then(|r| r),
                        None => request.await,
                    }
                }=>r};
                match value {
                    Ok(value) => break value,
                    Err(error) => {
                        let text = format!("{error:#}");
                        let retryable = error
                            .downcast_ref::<ProviderError>()
                            .is_some_and(ProviderError::is_retryable);
                        let reset_output = seen.swap(false, Ordering::Relaxed);
                        let before_compaction = context::estimate(&system, &self.messages, &specs);
                        if matches!(
                            error.downcast_ref::<ProviderError>(),
                            Some(ProviderError::ContextOverflow { .. })
                        ) && !overflow_recovered
                            && self.compaction
                            && self.compact_context(hooks, &system, &specs, true).await?
                            && context::estimate(&system, &self.messages, &specs)
                                < before_compaction
                        {
                            overflow_recovered = true;
                            self.sync_dynamic_context().await?;
                            self.refresh_snapshot(hooks).await?;
                            self.event(hooks, "provider_retry", json!({"attempt":attempt+1,"next_attempt":attempt+2,"delay_s":0,"reason":"context_overflow","error":text,"reset_output":reset_output}))?;
                            continue;
                        }
                        if !retryable || attempt >= retry_policy.max_retries {
                            return Err(
                                if matches!(
                                    error.downcast_ref::<ProviderError>(),
                                    Some(ProviderError::Deadline)
                                ) {
                                    error
                                } else {
                                    let detail = format!("provider error: {error:#}");
                                    error.context(detail)
                                },
                            );
                        }
                        let retry_after = error
                            .downcast_ref::<ProviderError>()
                            .and_then(ProviderError::retry_after);
                        let delay = retry_policy.delay(attempt, retry_after, retry_random());
                        attempt += 1;
                        let mut retry = json!({"attempt":attempt,"next_attempt":attempt+1,"delay_s":delay.as_secs_f64(),"error":text});
                        if reset_output {
                            retry["reset_output"] = json!(true);
                        }
                        if let Some(retry_after) = retry_after {
                            retry["retry_after_s"] = json!(retry_after.as_secs_f64());
                        }
                        self.event(hooks, "provider_retry", retry)?;
                        if let Some(deadline) = deadline {
                            tokio::select! {
                                biased;
                                _ = hooks.cancel.cancelled() => return Err(ProviderError::Cancelled.into()),
                                _ = tokio::time::sleep(deadline.saturating_duration_since(Instant::now())) => return Err(ProviderError::Deadline.into()),
                                _ = tokio::time::sleep(delay) => {}
                            }
                        } else {
                            tokio::select! {
                                _ = hooks.cancel.cancelled() => return Err(ProviderError::Cancelled.into()),
                                _ = tokio::time::sleep(delay) => {}
                            }
                        }
                    }
                }
            };
            let normalized = normalize_tool_ids(&mut response, &self.used_call_ids);
            if normalized > 0 {
                metrics["tool_id_normalizations"] = json!(normalized);
            }
            self.usage.add(&response.usage);
            hooks.shared.lock().unwrap().usage.add(&response.usage);
            let validation = validate_response(&response, &self.messages).and_then(|_| {
                if response.blocks.iter().any(
                    |b| matches!(b, Block::ToolUse { id, .. } if self.used_call_ids.contains(id)),
                ) {
                    bail!("reused archived tool call id");
                }
                Ok(())
            });
            if let Err(error) = validation {
                let event = self.store.checkpoint(&self.session, "provider_response_rejected", &json!({"error":error.to_string(),"usage":response.usage,"provider_progress":{"index":self.provider.index,"call_counter":self.provider.call_counter}}), None, None, Some(&store::counters(self.rounds, &self.usage)))?;
                (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
                bail!("provider protocol error: {error}");
            }
            let assistant = Message {
                role: "assistant".into(),
                content: response.blocks.clone(),
            };
            let calls = response
                .blocks
                .iter()
                .filter(|b| matches!(b, Block::ToolUse { .. }))
                .cloned()
                .collect::<Vec<_>>();
            metrics["text"] = json!(assistant.text());
            metrics["usage"] = json!(response.usage);
            metrics["usage"]["total_tokens"] = json!(response.usage.total());
            metrics["stop_reason"] = json!(response.stop_reason);
            metrics["tool_calls"] = json!(
                calls
                    .iter()
                    .filter_map(|b| if let Block::ToolUse { name, .. } = b {
                        Some(name)
                    } else {
                        None
                    })
                    .collect::<Vec<_>>()
            );
            metrics["tool_call_ids"] = json!(
                calls
                    .iter()
                    .filter_map(|b| if let Block::ToolUse { id, .. } = b {
                        Some(id)
                    } else {
                        None
                    })
                    .collect::<Vec<_>>()
            );
            metrics["provider_progress"] =
                json!({"index":self.provider.index,"call_counter":self.provider.call_counter});
            self.message_revision = None;
            let event = self.store.checkpoint(
                &self.session,
                "assistant_message",
                &metrics,
                (!assistant.content.is_empty()).then_some(&assistant),
                None,
                Some(&store::counters(self.rounds, &self.usage)),
            )?;
            if !assistant.content.is_empty() {
                self.messages.push(assistant);
            }
            self.message_revision = Some(self.store.message_revision(&self.session)?);
            self.used_call_ids.extend(calls.iter().filter_map(|b| {
                if let Block::ToolUse { id, .. } = b {
                    Some(id.clone())
                } else {
                    None
                }
            }));
            (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
            Self::check(hooks)?;
            if budget.max_total_tokens > 0
                && hooks.shared.lock().unwrap().usage.total() >= budget.max_total_tokens as u64
            {
                self.backfill(&calls, "未执行：显式 Token 费用上限已达到。")?;
                return Ok("token_budget".into());
            }
            if response.stop_reason == "max_tokens" {
                self.backfill(&calls, "未执行：模型响应被截断，请重新生成完整调用。")?;
                let names = metrics["truncated_tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|name| name.as_str())
                    .collect::<Vec<_>>();
                let reminder = if names.is_empty() {
                    "上一条模型回复因输出长度截断。请重新组织并简洁完成任务。".to_string()
                } else {
                    format!(
                        "工具调用 {} 因输出长度截断未执行，请拆成更小的 write / edit 操作后重新发出完整调用。",
                        names.join("、")
                    )
                };
                self.append(Message::user(format!(
                    "<system-reminder>\n{reminder}\n</system-reminder>"
                )))?;
                truncations += 1;
                if truncations >= 3 {
                    return Ok("max_tokens".into());
                }
                continue;
            }
            truncations = 0;
            if calls.is_empty() {
                Self::phase(hooks, "finishing");
                let ctx = self.context(hooks, None);
                if let Some(goal) = &mut self.goal {
                    let report = goal.check(&ctx).await?;
                    let passed = report["passed"] == true;
                    let attempts_exceeded = goal.attempts >= goal.spec.max_fix_attempts;
                    self.event(hooks, "goal_check", report.clone())?;
                    if passed {
                        return Ok("completed".into());
                    }
                    self.append(Message::user(Goal::failure(&report)))?;
                    if attempts_exceeded {
                        return Ok("goal_not_met".into());
                    }
                } else {
                    return Ok("completed".into());
                }
                continue;
            }
            Self::phase(hooks, "tools");
            let mut results = vec![];
            let mut index = 0;
            while index < calls.len() {
                Self::check(hooks)?;
                let call = &calls[index];
                let Block::ToolUse { name, input, .. } = call else {
                    unreachable!()
                };
                if self.child && self.submitted_result.is_some() {
                    results.push(self.record_result(
                        call,
                        Outcome::failure("子助手已提交最终结果，后续调用未执行"),
                        hooks,
                        false,
                        0.0,
                    )?);
                    index += 1;
                    continue;
                }
                if ["bash", "powershell"].contains(&name.as_str()) && input["background"] == true {
                    if let Some(outcome) = self.preflight(call, hooks, false).await? {
                        results.push(self.record_result(call, outcome, hooks, false, 0.0)?);
                        index += 1;
                        continue;
                    }
                    let granted = self.authorize(call, hooks).await?;
                    let result = if granted {
                        let command = util::string(input, "command")?.to_string();
                        let cwd = util::in_workspace(
                            &self.workspace,
                            input["cwd"].as_str().unwrap_or("."),
                        )?;
                        self.mark_tool_started(call, hooks)?;
                        let timeout = input["timeout_s"]
                            .as_f64()
                            .unwrap_or(0.0)
                            .clamp(0.0, 3600.0);
                        let job = self.background.start(
                            name.clone(),
                            command.clone(),
                            cwd,
                            timeout,
                            self.context(hooks, None),
                            hooks.sink.clone(),
                        )?;
                        let mut outcome = Outcome::ok(format!(
                            "已在后台启动命令。\njob_id: {job}\n命令: {command}\n结果将在后台完成后由系统通知。"
                        ));
                        outcome.job_id = Some(job);
                        self.record_result(call, outcome, hooks, false, 0.0)?
                    } else {
                        self.record_result(
                            call,
                            Outcome::failure("用户拒绝了工具执行"),
                            hooks,
                            false,
                            0.0,
                        )?
                    };
                    results.push(result);
                    index += 1;
                } else if name == "delegate"
                    && self.provider.fake.is_none()
                    && self.readonly_delegate(call).await?
                {
                    let mut end = index + 1;
                    if end < calls.len()
                        && self.readonly_delegate(&calls[end]).await?
                        && matches!((call, &calls[end]), (Block::ToolUse{input:a,..}, Block::ToolUse{input:b,..}) if a["kind"]!=b["kind"] || a["task"]!=b["task"])
                    {
                        end += 1;
                    }
                    let mut branches = vec![];
                    for call in &calls[index..end] {
                        let Block::ToolUse { input, .. } = call else {
                            unreachable!()
                        };
                        let key = (
                            input["kind"].as_str().unwrap_or_default().to_string(),
                            input["task"].as_str().unwrap_or_default().to_string(),
                        );
                        let mut children = HashMap::new();
                        if let Some(child) = self.children.remove(&key) {
                            children.insert(key, child);
                        }
                        branches.push((
                            call.clone(),
                            Runtime {
                                session: self.session.clone(),
                                workspace: self.workspace.clone(),
                                store: self.store.clone(),
                                catalog: self.catalog.clone(),
                                provider: self.provider.clone(),
                                messages: self
                                    .messages
                                    .iter()
                                    .rev()
                                    .filter(|m| genuine_user_text(m).is_some())
                                    .take(12)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect(),
                                message_revision: None,
                                usage: Usage::default(),
                                rounds: self.rounds,
                                pending: self.pending,
                                goal: None,
                                mcp: self.mcp.clone(),
                                background: self.background.clone(),
                                ownership: Default::default(),
                                submitted_result: None,
                                isolation: None,
                                isolations: HashMap::new(),
                                permissions: self.permissions.clone(),
                                permission_workspace: self.permission_workspace.clone(),
                                permission_settings: self.permission_settings.clone(),
                                sandbox: self.sandbox.clone(),
                                allowed: self.allowed.clone(),
                                child: false,
                                children,
                                custom_prompt: self.custom_prompt.clone(),
                                compaction: self.compaction,
                                mechanisms: self.mechanisms.clone(),
                                delegation_branch: true,
                                used_call_ids: self.used_call_ids.clone(),
                                instruction_changes: vec![],
                                frozen_system: self.frozen_system.clone(),
                                frozen_sources: self.frozen_sources.clone(),
                                dynamic_hash: self.dynamic_hash.clone(),
                                files: self.files.clone(),
                            },
                        ));
                    }
                    let outputs = futures_util::future::join_all(branches.into_iter().map(
                        |(call, mut branch)| async move {
                            let result = branch.invoke(call, hooks, false).await;
                            (result, branch.children)
                        },
                    ))
                    .await;
                    self.usage = hooks.shared.lock().unwrap().usage.clone();
                    self.store
                        .update(&self.session, &store::counters(self.rounds, &self.usage))?;
                    for (result, children) in outputs {
                        self.children.extend(children);
                        results.push(result?);
                    }
                    index = end;
                } else if tools::parallel_read(name) {
                    let mut end = index + 1;
                    while end < calls.len()
                        && end - index < 4
                        && matches!(&calls[end],Block::ToolUse{name,..} if tools::parallel_read(name))
                    {
                        end += 1
                    }
                    let futures = calls[index..end]
                        .iter()
                        .map(|c| self.invoke_read(c.clone(), hooks, false));
                    let outputs = futures_util::future::join_all(futures).await;
                    for result in outputs {
                        match result {
                            Ok(result) => results.push(result),
                            Err(e) => {
                                if !results.is_empty() {
                                    self.append(Message {
                                        role: "user".into(),
                                        content: results,
                                    })?;
                                }
                                return Err(e);
                            }
                        }
                    }
                    index = end;
                } else {
                    match self.invoke(call.clone(), hooks, false).await {
                        Ok(result) => results.push(result),
                        Err(e) => {
                            if !results.is_empty() {
                                self.append(Message {
                                    role: "user".into(),
                                    content: results,
                                })?;
                            }
                            return Err(e);
                        }
                    }
                    index += 1;
                }
            }
            let signature = util::hash(
                serde_json::to_string(&(
                    calls
                        .iter()
                        .map(|c| {
                            if let Block::ToolUse { name, input, .. } = c {
                                json!({"name":name,"input":input})
                            } else {
                                json!(null)
                            }
                        })
                        .collect::<Vec<_>>(),
                    results
                        .iter()
                        .map(|b| {
                            if let Block::ToolResult {
                                content,
                                is_error,
                                images,
                                ..
                            } = b
                            {
                                json!({"content":content,"is_error":is_error,"images":images})
                            } else {
                                json!(null)
                            }
                        })
                        .collect::<Vec<_>>(),
                ))?
                .as_bytes(),
            );
            if previous.as_ref() == Some(&signature) {
                repeated += 1
            } else {
                repeated = 0;
                previous = Some(signature)
            }
            self.append(Message {
                role: "user".into(),
                content: results,
            })?;
            Self::check(hooks)?;
            for queued in self
                .store
                .queued_prompts(&self.session)?
                .into_iter()
                .filter(|item| item["mode"] == "steer")
            {
                Self::check(hooks)?;
                self.consume_queued(&queued, hooks)?;
            }
            self.sync_dynamic_context().await?;
            self.event(hooks, "round_end", json!({"round":self.rounds}))?;
            if self.child && self.submitted_result.is_some() {
                return Ok("completed".into());
            }
            if repeated >= 3 {
                self.append(Message::user("<system-reminder>\n连续多轮工具调用与结果完全相同，任务已暂停以避免无效循环。继续时请重新评估目标与可用证据。\n</system-reminder>"))?;
                return Ok("stalled".into());
            }
            self.refresh_snapshot(hooks).await?;
        }
    }
    fn backfill_cancelled(&mut self, hooks: &Hooks) -> Result<()> {
        let answered = self
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                Block::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let ids = self
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                Block::ToolUse { id, .. } if !answered.contains(id) => Some(id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let journal = self.store.tool_results_for(&self.session, &ids)?;
        let execution = self.store.tool_execution(&self.session)?;
        let mut results = vec![];
        for call in self.messages.iter().flat_map(|m| &m.content) {
            if let Block::ToolUse { id, .. } = call
                && !answered.contains(id)
            {
                if let Some(result) = journal.get(id) {
                    results.push(result.clone());
                } else if execution.get(id) == Some(&false) {
                    results.push(self.record_result(
                        call,
                        Outcome::failure("用户已中断，未执行"),
                        hooks,
                        false,
                        0.0,
                    )?);
                }
            }
        }
        if !results.is_empty() {
            self.append(Message {
                role: "user".into(),
                content: results,
            })?;
        }
        Ok(())
    }
    fn mark_tool_started(&self, call: &Block, hooks: &Hooks) -> Result<()> {
        Self::check(hooks)?;
        if let Block::ToolUse { id, name, .. } = call {
            self.event(hooks, "tool_started", json!({"call_id":id,"name":name}))?;
        }
        Ok(())
    }
    fn backfill(&mut self, calls: &[Block], reason: &str) -> Result<()> {
        let content = calls
            .iter()
            .filter_map(|b| {
                if let Block::ToolUse { id, .. } = b {
                    Some(Block::ToolResult {
                        tool_use_id: id.clone(),
                        content: reason.into(),
                        is_error: true,
                        images: vec![],
                    })
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if !content.is_empty() {
            self.append(Message {
                role: "user".into(),
                content,
            })?;
        }
        Ok(())
    }
    async fn preflight(
        &self,
        call: &Block,
        hooks: &Hooks,
        recovered: bool,
    ) -> Result<Option<Outcome>> {
        let Block::ToolUse {
            id,
            name,
            input,
            input_error,
        } = call
        else {
            bail!("invalid tool call")
        };
        Self::check(hooks)?;
        self.event(
            hooks,
            "tool_call_start",
            json!({"call_id":id,"name":name,"arguments":input,"recovered":recovered}),
        )?;
        if let Some(error) = input_error {
            return Ok(Some(Outcome::failure(error.clone())));
        }
        if !self.mechanisms.subagents && ["delegate", "review_delegate"].contains(&name.as_str()) {
            return Ok(Some(Outcome::failure(
                "subagents are disabled for this evaluation",
            )));
        }
        if self
            .allowed
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(name))
        {
            return Ok(Some(Outcome::failure(format!("unknown tool: {name}"))));
        }
        let spec = tools::builtins()
            .iter()
            .find(|s| s.name == *name)
            .or_else(|| self.mcp.tools.get(name).map(|(s, _, _)| s));
        let Some(spec) = spec else {
            return Ok(Some(Outcome::failure(format!("unknown tool: {name}"))));
        };
        if self.child
            && self.isolation.is_none()
            && !tools::read_only(name)
            && !(name.starts_with("mcp__") && spec.read_only_hint)
        {
            return Ok(Some(Outcome::failure(format!(
                "read-only child cannot use {name}"
            ))));
        }
        if let Err(error) = tools::validate(spec, input) {
            return Ok(Some(Outcome::failure(format!(
                "invalid arguments for {name}: {error}"
            ))));
        }
        match self.permissions.decision(&self.workspace, name, input) {
            Ok(Some(crate::permissions::Decision::Deny)) => {
                return Ok(Some(Outcome::failure(
                    "permission denied by an explicit rule or unsupported restricted shell syntax",
                )));
            }
            Err(error) => return Ok(Some(Outcome::failure(error.to_string()))),
            _ => {}
        }
        if ["bash", "powershell"].contains(&name.as_str()) && self.permissions.has_file_denies() {
            return Ok(Some(Outcome::failure(
                "configured file deny rules cannot be guaranteed for arbitrary shell programs; command was not executed. Use scoped native file tools for this workspace.",
            )));
        }
        if let Some(isolation) = &self.isolation
            && ["edit", "write"].contains(&name.as_str())
            && let Err(error) = isolation.check_child_path(input["path"].as_str().unwrap_or(""))
        {
            return Ok(Some(Outcome::failure(error.to_string())));
        }
        let mut catalog = self.catalog.lock().await;
        if ["read", "ls", "grep", "edit", "write", "bash", "powershell"].contains(&name.as_str()) {
            let field = if ["bash", "powershell"].contains(&name.as_str()) {
                "cwd"
            } else {
                "path"
            };
            match catalog.discover_instructions_for(
                input[field].as_str().unwrap_or("."),
                tools::read_only(name),
            ) {
                Ok(sources) if !sources.is_empty() => {
                    for source in sources {
                        self.event(hooks, "project_instructions", source)?;
                    }
                    if !tools::read_only(name) {
                        return Ok(Some(Outcome::failure(
                            "已加载该目录的 AGENTS.md 指令。请按新指令重新调用工具。",
                        )));
                    }
                }
                Err(error) => return Ok(Some(Outcome::failure(error.to_string()))),
                _ => {}
            }
        }
        Ok(None)
    }
    async fn authorize(&self, call: &Block, hooks: &Hooks) -> Result<bool> {
        if !self.authorize_policy(call, hooks).await? {
            return Ok(false);
        }
        if let Block::ToolUse {
            id, name, input, ..
        } = call
            && ["bash", "powershell"].contains(&name.as_str())
            && let Some(reason) =
                crate::sandbox::dangerous(input["command"].as_str().unwrap_or_default())
        {
            self.event(
                hooks,
                "approval_request",
                json!({"call_id":id,"name":"dangerous_command","reason":reason}),
            )?;
            let request = json!({"call_id":id,"tool_name":"dangerous_command","arguments":input,"summary":format!("高危命令独立确认（{reason}）：{}",util::prefix(input["command"].as_str().unwrap_or_default(),1000))});
            let granted = self.ask(hooks, "", request).await?;
            Self::phase(hooks, "tools");
            self.event(
                hooks,
                "approval_decision",
                json!({"call_id":id,"name":"dangerous_command","granted":granted}),
            )?;
            return Ok(granted);
        }
        Ok(true)
    }
    async fn authorize_policy(&self, call: &Block, hooks: &Hooks) -> Result<bool> {
        let Block::ToolUse {
            id, name, input, ..
        } = call
        else {
            bail!("invalid tool call")
        };
        Self::check(hooks)?;
        let settings = hooks.settings.lock().unwrap().clone();
        let rules = crate::permissions::Rules::load(
            &self.permission_workspace,
            &self.permission_settings,
            &settings.permission_rules,
            &settings.always_allow,
        )?;
        let explicit = rules.decision(&self.workspace, name, input)?;
        let merge = name == "review_delegate" && input["action"] == "merge";
        let decision = if name == "review_delegate" {
            if merge {
                if settings.mode == "plan" {
                    "deny"
                } else {
                    "ask"
                }
            } else {
                "allow"
            }
        } else {
            match explicit {
                Some(crate::permissions::Decision::Deny) => "deny",
                Some(crate::permissions::Decision::Ask) => "ask",
                Some(crate::permissions::Decision::Allow) => "allow",
                None => tools::policy(name, &settings.mode),
            }
        };
        let decision = if explicit == Some(crate::permissions::Decision::Deny) {
            "deny"
        } else {
            decision
        };
        if decision == "deny" {
            return Ok(false);
        }
        if decision == "allow" {
            return Ok(true);
        }
        let summary = if merge {
            format!(
                "将子任务 {} 的已审阅改动合并到主工作区？拒绝将丢弃这些子任务改动。",
                input["delegate_call_id"].as_str().unwrap_or_default()
            )
        } else {
            format!("{name}: {}", util::prefix(&input.to_string(), 1000))
        };
        let grant = if merge {
            None
        } else {
            crate::permissions::Rules::grant_rule(&self.workspace, name, input)?
        };
        let request = json!({"call_id":id,"tool_name":name,"arguments":input,"summary":summary,"grant_rule":grant});
        self.event(
            hooks,
            "approval_request",
            json!({"call_id":id,"name":name,"arguments":input}),
        )?;
        let remember = grant
            .map(|rule| json!({"rule":rule,"workspace":self.permission_workspace}).to_string())
            .unwrap_or_default();
        let granted = self.ask(hooks, &remember, request).await?;
        Self::phase(hooks, "tools");
        self.event(
            hooks,
            "approval_decision",
            json!({"call_id":id,"name":name,"granted":granted}),
        )?;
        Ok(granted)
    }
    /// Shows `request` to the user and waits for the answer.
    async fn ask(&self, hooks: &Hooks, name: &str, request: Value) -> Result<bool> {
        Self::phase(hooks, "approval");
        let approval = format!(
            "{}:{}",
            self.session,
            hooks.approval_sequence.fetch_add(1, Ordering::Relaxed) + 1
        );
        let (tx, rx) = oneshot::channel();
        hooks
            .approvals
            .lock()
            .unwrap()
            .insert(approval.clone(), (name.to_string(), tx));
        (hooks.sink)(
            json!({"event":"approval","sessionId":self.session,"approvalId":approval,"request":request}),
        );
        let deadline = hooks.shared.lock().unwrap().deadline;
        let duration = deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(86400 * 365));
        let granted = tokio::select! {_ = hooks.cancel.cancelled()=>{hooks.approvals.lock().unwrap().remove(&approval);bail!("turn cancelled")},_ = tokio::time::sleep(duration)=>{hooks.approvals.lock().unwrap().remove(&approval);bail!("turn deadline exceeded")},value=rx=>value.unwrap_or(false)};
        hooks.approvals.lock().unwrap().remove(&approval);
        Ok(granted)
    }
    /// Workspace plugins that declare MCP servers run only after the user
    /// explicitly trusts them. Permission mode and "always allow" never count
    /// as that answer; a refusal just leaves the servers stopped.
    async fn trust_plugins(&self, hooks: &Hooks) -> Result<()> {
        let pending = self.catalog.lock().await.trust_required();
        if self.child || pending.is_empty() {
            return Ok(());
        }
        let call_id = format!("workspace_trust_{}", util::id());
        let arguments = json!({"workspace":self.workspace,"plugins":pending.iter().map(|p|json!({"name":p["name"],"version":p["version"],"sha256":p["sha256"],"servers":p["commands"]})).collect::<Vec<_>>()});
        let summary = pending
            .iter()
            .map(|p| {
                let commands = p["commands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        format!(
                            "{} {}",
                            c["command"].as_str().unwrap_or_default(),
                            c["args"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("；");
                format!(
                    "插件 {}@{} 要启动 MCP 服务：{commands}",
                    p["name"].as_str().unwrap_or_default(),
                    p["version"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.event(
            hooks,
            "approval_request",
            json!({"call_id":call_id,"name":"workspace_trust","arguments":arguments}),
        )?;
        let request = json!({"call_id":call_id,"tool_name":"workspace_trust","arguments":arguments,"summary":format!("信任此工作区的插件？\n{summary}")});
        let granted = self.ask(hooks, "workspace_trust", request).await?;
        Self::phase(hooks, "preparing");
        self.event(
            hooks,
            "approval_decision",
            json!({"call_id":call_id,"name":"workspace_trust","granted":granted}),
        )?;
        let mut catalog = self.catalog.lock().await;
        if granted {
            let reviewed = pending
                .iter()
                .map(|p| {
                    (
                        p["name"].as_str().unwrap_or_default().to_string(),
                        p["sha256"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect::<Vec<_>>();
            catalog.set_trust(&reviewed, true)?;
            let next = catalog.refresh()?;
            *catalog = next;
        } else {
            drop(catalog);
            self.event(
                hooks,
                "workspace_trust_required",
                json!({"workspace":self.workspace,"plugins":arguments["plugins"],"hint":"工作区插件未获信任，其 MCP 服务没有启动。审阅后可在界面中信任，或运行 `minicode plugins trust --workspace <路径>`。"}),
            )?;
        }
        Ok(())
    }
    async fn invoke_read(&self, call: Block, hooks: &Hooks, recovered: bool) -> Result<Block> {
        let started = Instant::now();
        if let Some(outcome) = self.preflight(&call, hooks, recovered).await? {
            return self.record_result(
                &call,
                outcome,
                hooks,
                recovered,
                started.elapsed().as_secs_f64(),
            );
        }
        let granted = self.authorize(&call, hooks).await?;
        let Block::ToolUse {
            name, input, id, ..
        } = &call
        else {
            unreachable!()
        };
        let mut outcome = if !granted {
            Outcome::failure("用户拒绝了工具执行")
        } else {
            let store = self.store.clone();
            let session = self.session.clone();
            let call_id = id.clone();
            let sink = hooks.sink.clone();
            let output: Sink = Arc::new(move |value| {
                if let Ok(event) = store.event(
                    &session,
                    "tool_output",
                    &json!({"call_id":call_id,"output_preview":value["text"]}),
                ) {
                    sink(json!({"event":"agent_event","sessionId":session,"item":event}));
                }
            });
            let ctx = self.context(hooks, Some(output));
            let deadline = hooks.shared.lock().unwrap().deadline;
            let configured = self.catalog.lock().await.configuration.tool_timeout(name)?;
            let limit = configured
                .or_else(|| self.mcp.timeouts.get(name).copied())
                .unwrap_or(Duration::from_secs(300));
            let duration = deadline
                .map(|d| d.saturating_duration_since(Instant::now()).min(limit))
                .unwrap_or(limit);
            self.mark_tool_started(&call, hooks)?;
            if name == "bash_output" {
                self.background
                    .output(input)
                    .unwrap_or_else(|e| Outcome::failure(e.to_string()))
            } else if name == "kill_shell" {
                self.background
                    .kill(util::string(input, "job_id")?)
                    .await
                    .unwrap_or_else(|e| Outcome::failure(e.to_string()))
            } else if ["bash", "powershell"].contains(&name.as_str()) {
                tools::execute(name, input, &ctx).await
            } else {
                tokio::time::timeout(duration, tools::execute(name, input, &ctx))
                    .await
                    .unwrap_or_else(|_| Outcome::failure("tool timed out"))
            }
        };
        if ["read", "ls", "grep"].contains(&name.as_str()) {
            let target =
                util::in_workspace(&self.workspace, input["path"].as_str().unwrap_or("."))?;
            let catalog = self.catalog.lock().await;
            let context = catalog
                .instructions
                .iter()
                .filter(|(path, _)| {
                    path.parent().is_some_and(|scope| {
                        target.starts_with(scope) && catalog.pending_scopes.contains(scope)
                    })
                })
                .map(|(path, source)| {
                    format!(
                        "<system-reminder>\n目录指令 {}：\n{}\n</system-reminder>",
                        path.display(),
                        source.content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !context.is_empty() {
                outcome.output = format!("{context}\n{}", outcome.output);
                if let Some(full) = &mut outcome.full_output {
                    full.insert_str(0, &format!("{context}\n"));
                }
            }
        }
        let result = self.record_result(
            &call,
            outcome,
            hooks,
            recovered,
            started.elapsed().as_secs_f64(),
        )?;
        Self::check(hooks)?;
        Ok(result)
    }
    async fn invoke(&mut self, call: Block, hooks: &Hooks, recovered: bool) -> Result<Block> {
        if let Block::ToolUse { name, input, .. } = &call
            && name == "review_delegate"
            && !self.child
        {
            let started = Instant::now();
            let result = if let Some(outcome) = self.preflight(&call, hooks, recovered).await? {
                outcome
            } else {
                self.mark_tool_started(&call, hooks)?;
                self.review_delegate(&call, input, hooks)
                    .await
                    .unwrap_or_else(|error| Outcome::failure(error.to_string()))
            };
            return self.record_result(
                &call,
                result,
                hooks,
                recovered,
                started.elapsed().as_secs_f64(),
            );
        }
        if self.child
            && let Block::ToolUse {
                name, input, id, ..
            } = &call
            && name == "submit_result"
        {
            if let Some(outcome) = self.preflight(&call, hooks, recovered).await? {
                return self.record_result(&call, outcome, hooks, recovered, 0.0);
            }
            if let Err(error) = crate::subagent_result::validate(input) {
                return self.record_result(
                    &call,
                    Outcome::failure(error.to_string()),
                    hooks,
                    recovered,
                    0.0,
                );
            }
            if !self.authorize(&call, hooks).await? {
                return self.record_result(
                    &call,
                    Outcome::failure("用户拒绝了工具执行"),
                    hooks,
                    recovered,
                    0.0,
                );
            }
            self.mark_tool_started(&call, hooks)?;
            if self.submitted_result.as_ref() != Some(input) {
                self.event(hooks, "subagent_result_submitted", json!({"call_id":id}))?;
            }
            self.submitted_result = Some(input.clone());
            return self.record_result(
                &call,
                Outcome::ok("Structured result submitted to parent."),
                hooks,
                recovered,
                0.0,
            );
        }
        if let Block::ToolUse {
            id, name, input, ..
        } = &call
            && name == "delegate"
            && !self.child
        {
            let started = Instant::now();
            if let Some(outcome) = self.preflight(&call, hooks, recovered).await? {
                return self.record_result(
                    &call,
                    outcome,
                    hooks,
                    recovered,
                    started.elapsed().as_secs_f64(),
                );
            }
            let granted = self.authorize(&call, hooks).await?;
            let result = if granted {
                self.mark_tool_started(&call, hooks)?;
                self.delegate(
                    id,
                    util::string(input, "kind")?,
                    util::string(input, "task")?,
                    input["writable"] == true,
                    hooks,
                )
                .await?
            } else {
                Outcome::failure("用户拒绝了工具执行")
            };
            return self.record_result(
                &call,
                result,
                hooks,
                recovered,
                started.elapsed().as_secs_f64(),
            );
        }
        self.invoke_read(call, hooks, recovered).await
    }
    fn record_result(
        &self,
        call: &Block,
        mut outcome: Outcome,
        hooks: &Hooks,
        recovered: bool,
        duration: f64,
    ) -> Result<Block> {
        let Block::ToolUse { id, name, .. } = call else {
            bail!("invalid tool call")
        };
        if let Some(full) = outcome.full_output.take() {
            let artifact = self.store.spill(&self.session, "tool_output", &full)?;
            outcome.output = format!(
                "{}\n...[完整输出已归档为 [artifact:{artifact}]；用 read_artifact 分页续读]",
                util::prefix(&outcome.output, 4000)
            );
            outcome.artifact_id = Some(artifact);
            outcome.truncated = true;
        }
        let content = if let Some(error) = &outcome.error {
            if outcome.output.is_empty() {
                error.clone()
            } else {
                format!("{error}\n{}", outcome.output)
            }
        } else {
            outcome.output.clone()
        };
        let result = Block::ToolResult {
            tool_use_id: id.clone(),
            content,
            is_error: !outcome.success,
            images: outcome.images.clone(),
        };
        let output_offset = outcome.error.as_ref().map_or(0, |e| {
            if outcome.output.is_empty() {
                e.len()
            } else {
                e.len() + 1
            }
        });
        let mut event=self.store.checkpoint(&self.session,"tool_call_result",&json!({"call_id":id,"name":name,"success":outcome.success,"exit_code":outcome.exit_code,"signal":outcome.signal,"error":outcome.error,"output_preview":util::prefix(&outcome.output,1000),"result_ref":id,"output_offset":output_offset,"artifact_id":outcome.artifact_id,"truncated":outcome.truncated,"duration_s":duration,"recovered":recovered,"observed_paths":outcome.observed_paths}),None,Some(&result),None)?;
        // Keep the live protocol compatible; only the persisted event is compact.
        event.data["output_detail"] = json!(outcome.output);
        event.data["images"] = json!(outcome.images);
        (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
        Ok(result)
    }
    async fn review_delegate(
        &mut self,
        call: &Block,
        input: &Value,
        hooks: &Hooks,
    ) -> Result<Outcome> {
        let key = util::string(input, "delegate_call_id")?;
        let workspace = self
            .isolations
            .get(key)
            .filter(|workspace| workspace.active())
            .cloned()
            .context("no pending isolated changes for this delegate call")?;
        match util::string(input, "action")? {
            "inspect" => {
                let diff = workspace.inspect(self.rounds)?;
                self.event(
                    hooks,
                    "subagent_changes_reviewed",
                    json!({"call_id":key,"isolation_id":workspace.id,"round":self.rounds}),
                )?;
                Ok(tools::bounded(diff))
            }
            "discard" => {
                workspace.discard()?;
                self.event(
                    hooks,
                    "subagent_isolation_closed",
                    json!({"call_id":key,"isolation_id":workspace.id,"reason":"discard"}),
                )?;
                Ok(Outcome::ok("已丢弃子任务改动；主工作区未修改。"))
            }
            "merge" => {
                workspace.ready_to_merge(self.rounds)?;
                if !self.authorize(call, hooks).await? {
                    workspace.discard()?;
                    self.event(hooks,"subagent_isolation_closed",json!({"call_id":key,"isolation_id":workspace.id,"reason":"merge_rejected"}))?;
                    return Ok(Outcome::failure(
                        "用户拒绝合并，已丢弃子任务改动；主工作区未修改。",
                    ));
                }
                Self::check(hooks)?;
                let count = workspace.merge(self.rounds)?;
                self.event(
                    hooks,
                    "subagent_changes_merged",
                    json!({"call_id":key,"isolation_id":workspace.id,"files":count}),
                )?;
                let cleanup = workspace
                    .discard()
                    .err()
                    .map(|error| format!("\n改动已合并；隔离目录清理失败，请重试 discard：{error}"))
                    .unwrap_or_default();
                Ok(Outcome::ok(format!("已合并 {count} 个文件。{cleanup}")))
            }
            _ => bail!("invalid review action"),
        }
    }
    async fn readonly_delegate(&self, call: &Block) -> Result<bool> {
        let Block::ToolUse { name, input, .. } = call else {
            return Ok(false);
        };
        if name != "delegate" || self.child || input["writable"] == true {
            return Ok(false);
        }
        let catalog = self.catalog.lock().await;
        let kind = input["kind"].as_str().unwrap_or_default();
        if let Some(def) = catalog
            .configuration
            .agents()?
            .iter()
            .find(|a| a["name"] == kind)
        {
            return Ok(def["enabled"] == true
                && def["inheritTools"] != true
                && def["tools"].as_array().is_some_and(|tools| {
                    tools
                        .iter()
                        .all(|t| t.as_str().is_some_and(tools::read_only))
                }));
        }
        Ok(catalog.agents.contains_key(kind))
    }
    fn delegate<'a>(
        &'a mut self,
        call_id: &'a str,
        kind: &'a str,
        task: &'a str,
        writable: bool,
        hooks: &'a Hooks,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Outcome>> + Send + 'a>> {
        Box::pin(async move {
            let catalog = self.catalog.lock().await;
            let configuration = catalog.configuration.clone();
            let definition = configuration
                .agents()?
                .into_iter()
                .find(|a| a["name"] == kind);
            let (instructions, source, allowed) = if let Some(def) = definition {
                if def["enabled"] != true {
                    return Ok(Outcome::failure(format!("subagent is disabled: {kind}")));
                }
                let allowed = if def["inheritTools"] == true {
                    None
                } else {
                    Some(serde_json::from_value::<HashSet<String>>(
                        def["tools"].clone(),
                    )?)
                };
                (
                    util::string(&def, "instructions")?.to_string(),
                    json!({"agent":kind,"origin":if def["builtin"]==true{"builtin"}else{"user"}}),
                    allowed,
                )
            } else if let Some((source, metadata)) = catalog.agents.get(kind) {
                (
                    source.content.clone(),
                    metadata.clone(),
                    Some(
                        ["read", "ls", "grep", "read_artifact"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    ),
                )
            } else {
                return Ok(Outcome::failure(format!(
                    "unknown read-only subagent kind: {kind}"
                )));
            };
            let mut inherited_catalog = catalog.clone();
            drop(catalog);
            let key = (kind.to_string(), task.to_string());
            let mut cached = self.children.remove(&key);
            let recovery = self.store.recovery_events(&self.session)?;
            let mut paused = cached
                .as_ref()
                .map(|child| child.session.clone())
                .or_else(|| {
                    recovery
                        .iter()
                        .rev()
                        .find(|e| {
                            ["subagent_start", "subagent_result"].contains(&e.r#type.as_str())
                                && e.data["kind"] == kind
                                && e.data["task"] == task
                        })
                        .and_then(|e| {
                            if e.r#type == "subagent_start" || e.data["exit_reason"] != "completed"
                            {
                                e.data["child_session_id"].as_str().map(str::to_string)
                            } else {
                                None
                            }
                        })
                });
            let isolation = if writable {
                let previous_workspace = if let Some(child) = &cached {
                    Some(child.workspace.clone())
                } else if let Some(id) = &paused {
                    self.store
                        .session(id)?
                        .and_then(|summary| summary["workspace"].as_str().map(PathBuf::from))
                } else {
                    None
                };
                let existing = self
                    .isolations
                    .values()
                    .find(|workspace| {
                        workspace.active()
                            && previous_workspace.as_ref().is_some_and(|previous| {
                                util::canonical(&workspace.work())
                                    .is_ok_and(|work| work == *previous)
                            })
                    })
                    .cloned();
                let workspace = if let Some(workspace) = existing {
                    workspace
                } else {
                    let extra = inherited_catalog
                        .instructions
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>();
                    Arc::new(crate::delegate_workspace::Workspace::create(
                        &self.workspace,
                        &self.store.root(),
                        &self.session,
                        &extra,
                    )?)
                };
                self.isolations.insert(call_id.into(), workspace.clone());
                self.event(
                    hooks,
                    "subagent_isolation",
                    json!({"call_id":call_id,"isolation_id":workspace.id,"kind":kind,"task":task}),
                )?;
                Some(workspace)
            } else {
                None
            };
            let child_workspace = util::canonical(
                &isolation
                    .as_ref()
                    .map_or_else(|| self.workspace.clone(), |workspace| workspace.work()),
            )?;
            let saved_workspace = paused
                .as_ref()
                .map(|id| {
                    self.store.require(id).and_then(|summary| {
                        summary["workspace"]
                            .as_str()
                            .map(PathBuf::from)
                            .context("invalid child workspace")
                    })
                })
                .transpose()?;
            if saved_workspace
                .as_ref()
                .is_some_and(|saved| *saved != child_workspace)
            {
                paused = None;
                if let Some(child) = cached.take() {
                    child.close_resources(hooks).await?;
                }
            }
            if isolation.is_some() {
                inherited_catalog.workspace = child_workspace.clone();
                inherited_catalog.instructions = inherited_catalog
                    .instructions
                    .into_iter()
                    .map(|(path, mut source)| {
                        let path = path.strip_prefix(&self.workspace).map_or_else(
                            |_| path.clone(),
                            |relative| child_workspace.join(relative),
                        );
                        source.path = path.clone();
                        (path, source)
                    })
                    .collect();
            }
            let mut child = if let Some(child) = cached {
                child
            } else {
                let mut settings = hooks.settings.lock().unwrap().clone();
                settings.acceptance = None;
                Box::new(
                    Self::from_catalog(
                        child_workspace,
                        &self.provider.configuration_id,
                        paused.as_deref(),
                        self.store.clone(),
                        inherited_catalog,
                        &settings,
                    )
                    .await?,
                )
            };
            child.child = true;
            child.permission_workspace = self.permission_workspace.clone();
            child.permissions = self.permissions.clone();
            child.isolation = isolation;
            child.compaction = self.compaction;
            child.mechanisms = self.mechanisms.clone();
            child.goal = None;
            child.files = self.files.clone();
            // A delegated task must see the same pinned project instructions
            // and activated skills as the request that created it.
            if !writable {
                let parent_catalog = self.catalog.lock().await;
                let mut child_catalog = child.catalog.lock().await;
                child_catalog.instructions = parent_catalog.instructions.clone();
                child_catalog.active = parent_catalog.active.clone();
            }
            let mut set = allowed.unwrap_or_else(|| {
                tools::builtins()
                    .iter()
                    .map(|spec| spec.name.clone())
                    .chain(self.mcp.tools.keys().cloned())
                    .collect()
            });
            {
                set.remove("delegate");
                set.remove("review_delegate");
                set.retain(|s| !s.starts_with("task_"));
                if writable {
                    set.retain(|name| !name.starts_with("mcp__"));
                } else {
                    set.retain(|name| {
                        tools::read_only(name)
                            || self
                                .mcp
                                .tools
                                .get(name)
                                .is_some_and(|(spec, _, _)| spec.read_only_hint)
                    });
                }
                set.insert("submit_result".into());
            }
            child.allowed = Some(set);
            // Close only child-owned recovery connections before borrowing the
            // current parent view. Borrowed handles never close parent services.
            child.mcp.close().await;
            child.mcp = Arc::new(self.mcp.restricted(child.allowed.as_ref()));
            child.provider = self.provider.clone();
            let requirements = delegate_requirements(&self.messages);
            child.custom_prompt = Some(format!(
                "\n你是子助手。只使用配置的工具，遵守父会话权限。完成时优先调用 submit_result 提交结构化结果；也可最终输出 JSON 对象，键为 summary（字符串）、findings（字符串列表）、evidence_refs（你实际读取过的工作区相对路径列表）、unresolved（字符串列表）。不得编造证据。\n父任务要求：\n{requirements}\n子助手指令：\n{instructions}"
            ));
            let child_id = child.session.clone();
            self.event(
                hooks,
                "subagent_start",
                json!({"call_id":call_id,"kind":kind,"task":task,"child_session_id":child_id,"source":source}),
            )?;
            let child_settings = hooks.settings.clone();
            let parent_sink = hooks.sink.clone();
            let parent_session = self.session.clone();
            let parent_store = self.store.clone();
            let parent_call = call_id.to_string();
            let progress_child = child_id.clone();
            let sink: Sink = Arc::new(move |mut event| {
                if matches!(event["event"].as_str(), Some("approval" | "approval_auto")) {
                    event["sessionId"] = json!(parent_session);
                    parent_sink(event);
                } else if event["event"] == "agent_event" {
                    let item = &event["item"];
                    let (phase, status) = match item["type"].as_str() {
                        Some("round_start") => ("model", "running"),
                        Some("tool_call_start") => ("tool", "running"),
                        Some("tool_call_result") => (
                            "tool",
                            if item["data"]["success"] == true {
                                "completed"
                            } else {
                                "failed"
                            },
                        ),
                        _ => return,
                    };
                    // Whitelist summary fields; never copy arguments, text, output or errors.
                    let mut data = json!({"call_id":parent_call,"child_session_id":progress_child,"child_event_seq":item["seq"],"phase":phase,"status":status});
                    if phase == "tool" {
                        data["child_call_id"] = item["data"]["call_id"].clone();
                        data["name"] = item["data"]["name"].clone();
                    }
                    if let Ok(progress) =
                        parent_store.event(&parent_session, "subagent_progress", &data)
                    {
                        parent_sink(
                            json!({"event":"agent_event","sessionId":parent_session,"item":progress}),
                        );
                    }
                }
            });
            let child_hooks = Hooks {
                settings: child_settings,
                snapshot: Arc::new(Mutex::new(Snapshot::default())),
                approvals: hooks.approvals.clone(),
                approval_sequence: hooks.approval_sequence.clone(),
                sink,
                cancel: hooks.cancel.clone(),
                shared: hooks.shared.clone(),
            };
            let old = child.usage.clone();
            let text = if child.pending {
                None
            } else {
                Some(format!("{kind}：{task}"))
            };
            let result = match child.run(text, vec![], child_hooks.clone()).await {
                Ok(result) => result,
                Err(error) => {
                    self.event(hooks, "subagent_result", json!({"call_id":call_id,"kind":kind,"task":task,"child_session_id":child_id,"exit_reason":"error"}))?;
                    return Err(error);
                }
            };
            self.provider.index = child.provider.index;
            self.provider.call_counter = child.provider.call_counter;
            let usage = Usage {
                input_tokens: child.usage.input_tokens - old.input_tokens,
                output_tokens: child.usage.output_tokens - old.output_tokens,
                cache_read_tokens: child.usage.cache_read_tokens - old.cache_read_tokens,
                cache_write_tokens: child.usage.cache_write_tokens - old.cache_write_tokens,
                available: child.usage.available,
            };
            self.usage.add(&usage);
            if !self.delegation_branch {
                self.store
                    .update(&self.session, &store::counters(self.rounds, &self.usage))?;
            }
            let final_text = child
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant")
                .map(Message::text)
                .unwrap_or_default();
            let parsed = child
                .submitted_result
                .clone()
                .or_else(|| crate::subagent_result::extract(&final_text));
            let structured = parsed.is_some();
            let mut payload=parsed.unwrap_or(json!({"summary":final_text,"findings":[],"evidence_refs":[],"unresolved":["子助手未返回有效的结构化结果。"]}));
            let events = self.store.recovery_events(&child_id)?;
            let mut observed = HashSet::new();
            for result in events
                .iter()
                .filter(|e| e.r#type == "tool_call_result" && e.data["success"] == true)
            {
                let legacy_call = if result.data["name"].as_str().is_none()
                    || result.data["observed_paths"].is_null()
                {
                    events.iter().rev().find(|e| {
                        e.r#type == "tool_call_start" && e.data["call_id"] == result.data["call_id"]
                    })
                } else {
                    None
                };
                let name = result.data["name"]
                    .as_str()
                    .or_else(|| legacy_call.and_then(|call| call.data["name"].as_str()))
                    .unwrap_or_default();
                if !["read", "write", "edit", "grep", "ls"].contains(&name) {
                    continue;
                }
                if let Some(paths) = result.data["observed_paths"].as_array() {
                    for path in paths.iter().filter_map(Value::as_str) {
                        if let Ok(path) = util::in_workspace(&child.workspace, path) {
                            observed.insert(path);
                        }
                    }
                } else if let Some(call) = legacy_call
                    && ["read", "write", "edit", "ls"]
                        .contains(&call.data["name"].as_str().unwrap_or_default())
                    && let Some(path) = call.data["arguments"]["path"].as_str()
                    && let Ok(path) = util::in_workspace(&child.workspace, path)
                {
                    observed.insert(path);
                }
            }
            let requested = payload["evidence_refs"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut verified = vec![];
            for r in requested {
                if let Some(reference) = r.as_str() {
                    let path = crate::subagent_result::reference_path(reference);
                    if util::in_workspace(&child.workspace, path)
                        .is_ok_and(|path| path.exists() && observed.contains(&path))
                    {
                        verified.push(r)
                    } else {
                        payload["unresolved"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!(format!("未验证的证据引用：{reference}")));
                    }
                }
            }
            payload["evidence_refs"] = json!(verified);
            payload["child_session_id"] = json!(child_id);
            if let Some(isolation) = &child.isolation {
                payload["isolation_id"] = json!(isolation.id);
                payload["requires_review"] = json!(true);
                payload["review_instructions"] = json!(format!(
                    "主工作区尚未修改。父会话须调用 review_delegate(delegate_call_id={call_id}, action=inspect)，查看 diff 后在后续响应中选择 merge 或 discard。"
                ));
            }
            payload["status"] = json!(if structured {
                result.exit_reason.as_str()
            } else {
                "invalid_result"
            });
            self.event(hooks,"subagent_result",json!({"call_id":call_id,"kind":kind,"task":task,"child_session_id":child_id,"exit_reason":result.exit_reason,"structured":structured,"usage":usage,"usage_scope":"activation","provider_progress":{"index":self.provider.index,"call_counter":self.provider.call_counter},"evidence_refs":verified,"source":source}))?;
            let completed = structured && result.exit_reason == "completed";
            if completed {
                child.close_resources(&child_hooks).await?;
            }
            if !completed {
                self.children.insert(key, child);
                let status = payload["status"].clone();
                payload["unresolved"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(format!("子任务状态：{status}")));
            }
            let mut outcome = Outcome::ok(serde_json::to_string(&payload)?);
            outcome.success = completed;
            if !completed {
                outcome.error = Some(format!("子任务未完成：{}", payload["status"]));
            }
            Ok(outcome)
        })
    }
    pub async fn compact(&mut self, hooks: &Hooks) -> Result<bool> {
        let catalog = self.catalog.lock().await;
        let specs = self.tool_specs(&catalog, &catalog.configuration.agents()?);
        let system = self.system(&catalog, &specs)?;
        drop(catalog);
        let changed = self.compact_context(hooks, &system, &specs, true).await?;
        if changed {
            if self.frozen_system.is_some() {
                self.sync_dynamic_context().await?;
            }
            self.refresh_snapshot(hooks).await?;
        }
        Ok(changed)
    }

    async fn compact_context(
        &mut self,
        hooks: &Hooks,
        system: &str,
        specs: &[ToolSpec],
        force: bool,
    ) -> Result<bool> {
        let raw = context::estimate(system, &self.messages, specs);
        let measured = self
            .provider
            .estimated_prompt_tokens(system, &self.messages, specs);
        let window = ((raw as u128 * self.provider.window as u128) / measured.max(1) as u128)
            .min(u64::MAX as u128) as u64;
        let trigger = window.saturating_sub((window / 10).min(16_384));
        if !force && context::estimate(system, &self.messages, specs) <= trigger {
            return Ok(false);
        }
        let mut generated = None;
        if let Some(range) = context::archive_range(&self.messages, force, window) {
            let config = self.catalog.lock().await.configuration.clone();
            let model = config.read()?["compactionModel"]
                .as_str()
                .map(str::to_string);
            let provider = match model {
                Some(model) => Provider::for_settings(
                    &Settings {
                        model,
                        effort: "off".into(),
                        ..hooks.settings.lock().unwrap().clone()
                    },
                    &config,
                    None,
                ),
                None => Ok(self.provider.clone()),
            };
            if let Ok(mut provider) = provider {
                let attachments = Attachments {
                    root: self.store.root().join("attachments"),
                };
                let deadline = hooks.shared.lock().unwrap().deadline;
                let timeout = deadline.map_or(Duration::from_secs(30), |d| {
                    d.saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(30))
                });
                let mut usage = Usage::default();
                let limit = hooks.settings.lock().unwrap().budget.max_total_tokens;
                let remaining = if limit > 0 {
                    (limit as u64)
                        .saturating_sub(hooks.shared.lock().unwrap().usage.total())
                        .max(1)
                } else {
                    0
                };
                let result = tokio::select! {
                    _ = hooks.cancel.cancelled() => Err(anyhow::Error::from(ProviderError::Cancelled)),
                    result = tokio::time::timeout(timeout, provider.summarize(&self.messages[..range.end], &attachments, &mut usage, remaining)) => result.map_err(anyhow::Error::from).and_then(|r| r),
                };
                if usage.total() > 0 {
                    self.usage.add(&usage);
                    hooks.shared.lock().unwrap().usage.add(&usage);
                    self.store.checkpoint(
                        &self.session,
                        "context_summary",
                        &json!({"usage":usage,"model":provider.configuration_id}),
                        None,
                        None,
                        Some(&store::counters(self.rounds, &self.usage)),
                    )?;
                }
                if hooks.cancel.is_cancelled() {
                    return Err(ProviderError::Cancelled.into());
                }
                generated = result.ok();
            }
        }
        self.message_revision = None;
        let changed = context::compact_summarized(
            &self.store,
            &self.session,
            system,
            specs,
            &mut self.messages,
            window,
            (force, generated.as_deref()),
        )?;
        self.message_revision = Some(self.store.message_revision(&self.session)?);
        if let Some(stats) = changed {
            self.event(hooks, "context_compacted", stats)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn full_jitter_and_retry_after_are_bounded() {
        let policy = RetryPolicy {
            max_retries: 6,
            base_s: 1.0,
            max_s: 30.0,
            retry_after_max_s: 60.0,
        };
        assert_eq!(policy.delay(0, None, 0.0), Duration::ZERO);
        assert_eq!(policy.delay(0, None, 1.0), Duration::from_secs(1));
        assert_eq!(policy.delay(3, None, 0.5), Duration::from_secs(4));
        assert_eq!(policy.delay(7, None, 1.0), Duration::from_secs(30));
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(5)), 0.0),
            Duration::from_secs(5)
        );
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(90)), 0.0),
            Duration::from_secs(60)
        );
        for _ in 0..32 {
            assert!((0.0..=1.0).contains(&retry_random()));
        }
    }
}
