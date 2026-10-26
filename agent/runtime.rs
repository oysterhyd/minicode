use crate::{
    attachments::Attachments,
    configuration::Configuration,
    context,
    extensions::Catalog,
    goals::{Goal, Spec},
    mcp::McpRegistry,
    models::{Block, Budget, Event, Message, Outcome, RunResult, Usage},
    provider::{Provider, validate_response},
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

#[derive(Clone)]
pub struct Settings {
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub budget: Budget,
    pub acceptance: Option<PathBuf>,
    pub always_allow: HashSet<String>,
    pub pending_model: bool,
    pub registry_dirty: bool,
}
impl Settings {
    pub fn new(configuration: &Configuration) -> Result<Self> {
        Ok(Self {
            model: configuration.default_model()?,
            effort: "off".into(),
            mode: "default".into(),
            budget: Budget::default(),
            acceptance: None,
            always_allow: HashSet::new(),
            pending_model: false,
            registry_dirty: false,
        })
    }
}
pub struct Snapshot {
    pub session: Option<String>,
    pub run_id: Option<String>,
    pub phase: String,
    pub running: bool,
    pub pending: bool,
    pub rounds: u64,
    pub usage: Option<Usage>,
    pub active_model: String,
    pub window: u64,
    pub breakdown: Value,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            session: None,
            run_id: None,
            phase: "idle".into(),
            running: false,
            pending: false,
            rounds: 0,
            usage: None,
            active_model: String::new(),
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
    pub allowed: Option<HashSet<String>>,
    pub child: bool,
    pub children: HashMap<(String, String), Box<Runtime>>,
    pub custom_prompt: Option<String>,
    pub compaction: bool,
    delegation_branch: bool,
    used_call_ids: HashSet<String>,
    /// Instruction and skill drift found on resume, reported by `prepare`.
    instruction_changes: Vec<Value>,
    /// Files this session has read or written; shared with its delegates.
    pub files: tools::Files,
}
impl Runtime {
    pub async fn new(
        workspace: &Path,
        model: &str,
        session: Option<&str>,
        store: Store,
        configuration: Configuration,
        settings: &Settings,
    ) -> Result<Self> {
        let workspace = util::canonical(workspace)?;
        if !workspace.is_dir() {
            bail!("workspace no longer exists")
        }
        let provider = Provider::new(model, &settings.effort, &configuration)?;
        let mut catalog = Catalog::scan(&workspace, configuration, true)?;
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
                    available: summary["usage_available"] == true,
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
        let events = store.events(&id, -1)?;
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
        let mut runtime = Self {
            session: id,
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
            allowed: None,
            child: false,
            children: HashMap::new(),
            custom_prompt: None,
            compaction: true,
            delegation_branch: false,
            used_call_ids,
            instruction_changes,
            files: Default::default(),
        };
        let child_ids = events
            .iter()
            .filter(|e| e.r#type == "subagent_start")
            .filter_map(|e| e.data["child_session_id"].as_str())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let resume_fake = runtime.provider.fake.is_some()
            && events
                .iter()
                .find(|e| e.r#type == "session_start")
                .and_then(|e| e.data["provider_resume_identity"].as_str())
                .is_some_and(|id| Some(id.to_string()) == runtime.provider.identity());
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
            let child_events = runtime.store.events(&child_id, -1)?;
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
                usage.available &= summary["usage_available"] == true;
            }
            recovered_usage.add(&usage);
            recovery_events.extend(child_events);
        }
        if recovered_usage.total() > runtime.usage.total() {
            runtime.usage = recovered_usage;
        }
        if resume_fake {
            runtime.restore_script_progress(&recovery_events);
        }
        Ok(runtime)
    }
    fn restore_script_progress(&mut self, events: &[Event]) {
        self.provider.index = events
            .iter()
            .filter(|e| e.r#type == "assistant_message")
            .count();
        self.provider.call_counter = self
            .used_call_ids
            .iter()
            .filter_map(|id| {
                id.strip_prefix("fake_tool_")
                    .and_then(|s| s.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0);
        for event in events {
            if let Some(n) = event.data["call_id"]
                .as_str()
                .and_then(|id| id.strip_prefix("fake_tool_"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                self.provider.call_counter = self.provider.call_counter.max(n);
            }
            if let Some(progress) = event.data.get("provider_progress") {
                self.provider.index = self
                    .provider
                    .index
                    .max(progress["index"].as_u64().unwrap_or(0) as usize);
                self.provider.call_counter = self
                    .provider
                    .call_counter
                    .max(progress["call_counter"].as_u64().unwrap_or(0) as usize);
            }
        }
    }
    pub fn set_script(&mut self, script: Value) -> Result<()> {
        self.provider.set_script(script)?;
        let mut events = self.store.events(&self.session, -1)?;
        if events
            .iter()
            .find(|e| e.r#type == "session_start")
            .and_then(|e| e.data["provider_resume_identity"].as_str())
            .is_some_and(|id| Some(id.to_string()) == self.provider.identity())
        {
            let children = events
                .iter()
                .filter(|e| e.r#type == "subagent_start")
                .filter_map(|e| e.data["child_session_id"].as_str().map(str::to_string))
                .collect::<HashSet<_>>();
            for id in children {
                events.extend(self.store.events(&id, -1)?);
            }
            self.restore_script_progress(&events);
        }
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
        let mut s = hooks.snapshot.lock().unwrap();
        s.session = Some(self.session.clone());
        s.pending = self.pending;
        s.rounds = self.rounds;
        s.usage = Some(self.usage.clone());
        s.active_model = self.provider.configuration_id.clone();
        s.window = self.provider.window;
        let mut breakdown = context::breakdown(&system, &self.messages, &specs);
        if let Some(values) = breakdown.as_object_mut() {
            for value in values.values_mut() {
                *value =
                    json!((value.as_u64().unwrap_or(0) as f64 * self.provider.prompt_scale) as u64)
            }
        }
        s.breakdown = breakdown;
        Ok(())
    }
    fn tool_specs(&self, catalog: &Catalog, agents: &[Value]) -> Vec<crate::models::ToolSpec> {
        let mut specs = tools::specs(catalog, &self.mcp, agents, !self.child);
        if let Some(allowed) = &self.allowed {
            specs.retain(|s| allowed.contains(&s.name));
        }
        specs
    }
    fn system(&self, catalog: &Catalog, specs: &[crate::models::ToolSpec]) -> Result<String> {
        let mut prompt =
            catalog.prompt(&specs.iter().map(|s| s.name.clone()).collect::<Vec<_>>())?;
        if let Some(custom) = &self.custom_prompt {
            prompt.push_str(custom)
        }
        Ok(prompt)
    }
    async fn connect_mcp(&mut self, hooks: &Hooks) -> Result<()> {
        self.trust_plugins(hooks).await?;
        let servers = self.catalog.lock().await.servers.clone();
        let deadline = hooks.shared.lock().unwrap().deadline;
        let prepare = McpRegistry::prepare(servers);
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
        let existing = self.store.events(&self.session, -1)?;
        if !existing.iter().any(|e| e.r#type == "session_start") {
            let catalog = self.catalog.lock().await;
            let global = catalog
                .configuration
                .path
                .parent()
                .unwrap()
                .join("AGENTS.md");
            self.event(hooks,"session_start",json!({"workspace":self.workspace,"provider":self.provider.name,"model":self.provider.configuration_id,"budget":hooks.settings.lock().unwrap().budget,"project_instructions":catalog.instructions.values().map(|s|s.metadata(&global)).collect::<Vec<_>>(),"active_skills":catalog.active,"provider_resume_identity":self.provider.identity()}))?;
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
        let journal = self.store.tool_results(&self.session)?;
        let mut results = vec![];
        for call in pending {
            let Block::ToolUse { id, name, .. } = &call else {
                continue;
            };
            let result = if let Some(result) = journal.get(id) {
                result.clone()
            } else if tools::replay_safe(name) {
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
        for (_, mut job) in jobs {
            job["status"] = json!("lost");
            self.event(hooks, "background_job_lost", job.clone())?;
            self.append(Message::user(format!(
                "后台任务 {} 在恢复会话时失联，结果未知，请核实。",
                job["job_id"]
            )))?;
        }
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
                "<system-reminder>\n自上次运行以来，项目指令或已激活的 Skill 发生了变化，系统提示已更新为当前版本：\n{lines}\n请以系统提示中的最新指令为准。\n</system-reminder>"
            )))?;
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
    pub async fn run(
        &mut self,
        text: Option<String>,
        attachments: Vec<Block>,
        hooks: Hooks,
    ) -> Result<RunResult> {
        let _ownership = self.store.activation(&self.session)?;
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
        let mut jobs: Vec<(
            String,
            String,
            CancellationToken,
            tokio::task::JoinHandle<Result<Outcome>>,
        )> = vec![];
        // A panic in the loop is caught so the teardown below still runs; it
        // is then reported as a run failure. Persisted history stays the
        // source of truth for the next run.
        let (result, panicked) = match std::panic::AssertUnwindSafe(self.run_inner(
            text,
            attachments,
            &hooks,
            &mut jobs,
        ))
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
        let running = jobs
            .iter()
            .map(|(_, _, _, handle)| !handle.is_finished())
            .collect::<Vec<_>>();
        for (_, _, cancel, _) in &jobs {
            cancel.cancel();
        }
        for ((job, command, _, handle), was_running) in jobs.into_iter().zip(running) {
            let outcome = handle.await.ok().and_then(Result::ok);
            if let Err(error) = self.event(&hooks,if was_running{"background_job_lost"}else{"background_job_completed"},json!({"job_id":job,"command":command,"status":if was_running{"lost"}else if outcome.as_ref().is_some_and(|o|o.success){"completed"}else{"failed"},"exit_code":outcome.as_ref().and_then(|o|o.exit_code),"output":outcome.map(|o|o.output).unwrap_or_default()})) {
                failures.push(format!("background job {job}: {error}"));
            }
        }
        self.mcp.close().await;
        self.mcp = Arc::new(McpRegistry::default());
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
            s.phase = if self.pending { "paused" } else { "idle" }.into();
        }
        if panicked || !failures.is_empty() {
            let mut report = error.into_iter().collect::<Vec<_>>();
            if !failures.is_empty() {
                report.push(format!("run teardown failed: {}", failures.join("; ")));
            }
            bail!(report.join("; "))
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
        }
    }
    async fn run_inner(
        &mut self,
        text: Option<String>,
        attachments: Vec<Block>,
        hooks: &Hooks,
        jobs: &mut Vec<(
            String,
            String,
            CancellationToken,
            tokio::task::JoinHandle<Result<Outcome>>,
        )>,
    ) -> Result<String> {
        self.prepare(hooks).await?;
        if let Some(text) = text {
            let mut content = attachments;
            content.push(Block::text(text));
            self.append(Message {
                role: "user".into(),
                content,
            })?;
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
            if settings.pending_model || settings.model != self.provider.configuration_id {
                let config = self.catalog.lock().await.configuration.clone();
                let mut next = Provider::new(&settings.model, &settings.effort, &config)?;
                if next.identity().is_some() && next.identity() == self.provider.identity() {
                    next.index = self.provider.index;
                    next.call_counter = self.provider.call_counter;
                }
                self.provider = next;
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
            let mut i = 0;
            while i < jobs.len() {
                if jobs[i].3.is_finished() {
                    let (job, command, _, handle) = jobs.remove(i);
                    let result = handle.await??;
                    self.event(hooks,"background_job_completed",json!({"job_id":job,"command":command,"status":if result.success{"completed"}else{"failed"},"exit_code":result.exit_code,"output":result.output}))?;
                    self.append(Message::user(format!(
                        "后台任务已完成\njob_id: {job}\n命令: {command}\n退出码: {:?}\n输出:\n{}",
                        result.exit_code, result.output
                    )))?;
                } else {
                    i += 1
                }
            }
            let (specs, system) = {
                let mut catalog = self.catalog.lock().await;
                catalog.pending_scopes.clear();
                for source in catalog.reload_instructions()? {
                    self.event(hooks, "project_instructions", source)?;
                }
                let agents = catalog.configuration.agents()?;
                let specs = self.tool_specs(&catalog, &agents);
                let system = self.system(&catalog, &specs)?;
                (specs, system)
            };
            if self.compaction {
                self.message_revision = None;
                let stats = context::compact(
                    &self.store,
                    &self.session,
                    &system,
                    &specs,
                    &mut self.messages,
                    (self.provider.window as f64 / self.provider.prompt_scale) as u64,
                    false,
                )?;
                self.message_revision = Some(self.store.message_revision(&self.session)?);
                if let Some(stats) = stats {
                    self.event(hooks, "context_compacted", stats)?;
                }
            }
            let mut context_tokens = (context::estimate(&system, &self.messages, &specs) as f64
                * self.provider.prompt_scale) as u64;
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
                    context_tokens = (context::estimate(&system, &self.messages, &specs) as f64
                        * self.provider.prompt_scale) as u64;
                }
                self.message_revision = Some(self.store.message_revision(&self.session)?);
            }
            if context_tokens.saturating_add(1024) >= self.provider.window {
                return Ok("context_limit".into());
            }
            {
                let mut ledger = hooks.shared.lock().unwrap();
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
            let (response, mut metrics) = loop {
                // Only the turn deadline bounds the whole request; silence on
                // the wire is the provider's idle timeout, so long streams that
                // keep producing bytes are never cut off.
                let deadline = hooks.shared.lock().unwrap().deadline;
                let request =
                    self.provider
                        .stream(&system, &self.messages, &specs, &attachments, &delta);
                let value = tokio::select! {_ = hooks.cancel.cancelled()=>bail!("turn cancelled"),r=async {
                    match deadline {
                        Some(d) => tokio::time::timeout(d.saturating_duration_since(Instant::now()), request).await.map_err(|_| anyhow::anyhow!("turn deadline exceeded")).and_then(|r| r),
                        None => request.await,
                    }
                }=>r};
                match value {
                    Ok(value) => break value,
                    Err(error) => {
                        self.usage.available = false;
                        let text = error.to_string();
                        let retryable = text.contains("connection")
                            || text.contains("HTTP 429")
                            || text.contains("HTTP 5")
                            || text.contains("idle timeout");
                        if !retryable || seen.load(Ordering::Relaxed) || attempt >= 2 {
                            return Err(if text == "turn deadline exceeded" {
                                error
                            } else {
                                anyhow::anyhow!("provider error: {error}")
                            });
                        }
                        let delay = 0.25 * 2f64.powi(attempt);
                        attempt += 1;
                        self.event(hooks,"provider_retry",json!({"attempt":attempt,"next_attempt":attempt+1,"delay_s":delay,"error":text}))?;
                        tokio::select! {_ = hooks.cancel.cancelled()=>bail!("turn cancelled"),_ = tokio::time::sleep(Duration::from_secs_f64(delay))=>{}}
                    }
                }
            };
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
                Some(&assistant),
                None,
                Some(&store::counters(self.rounds, &self.usage)),
            )?;
            self.messages.push(assistant);
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
                self.append(Message::user("上一条模型回复被输出长度截断。请简短续写未完成部分；不要假设被截断的工具调用已经执行。"))?;
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
                if name == "bash" && input["background"] == true {
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
                        let job = format!("bg_{}", util::id());
                        let cancel = hooks.cancel.child_token();
                        let mut ctx = self.context(hooks, None);
                        ctx.cancel = cancel.clone();
                        let timeout = input["timeout_s"]
                            .as_f64()
                            .unwrap_or(0.0)
                            .clamp(0.0, 3600.0);
                        let c = command.clone();
                        let job_cwd = cwd.clone();
                        let handle = tokio::spawn(async move {
                            tools::run_command(&c, &job_cwd, timeout, &ctx).await
                        });
                        self.event(
                            hooks,
                            "background_job_started",
                            json!({"job_id":job,"command":command,"cwd":cwd}),
                        )?;
                        jobs.push((job.clone(), command.clone(), cancel, handle));
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
                                    .filter(|m| m.role == "user")
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
                                allowed: self.allowed.clone(),
                                child: false,
                                children,
                                custom_prompt: self.custom_prompt.clone(),
                                compaction: self.compaction,
                                delegation_branch: true,
                                used_call_ids: self.used_call_ids.clone(),
                                instruction_changes: vec![],
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
            self.event(hooks, "round_end", json!({"round":self.rounds}))?;
            if repeated >= 3 {
                self.append(Message::user("连续多轮工具调用与结果完全相同，任务已暂停以避免无效循环。继续时请重新评估目标与可用证据。"))?;
                return Ok("stalled".into());
            }
            self.refresh_snapshot(hooks).await?;
        }
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
        let Block::ToolUse { id, name, input } = call else {
            bail!("invalid tool call")
        };
        Self::check(hooks)?;
        self.event(
            hooks,
            "tool_call_start",
            json!({"call_id":id,"name":name,"arguments":input,"recovered":recovered}),
        )?;
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
        if let Err(error) = tools::validate(spec, input) {
            return Ok(Some(Outcome::failure(format!(
                "invalid arguments for {name}: {error}"
            ))));
        }
        let mut catalog = self.catalog.lock().await;
        if ["read", "ls", "grep", "edit", "write", "bash"].contains(&name.as_str()) {
            let field = if name == "bash" { "cwd" } else { "path" };
            match catalog.discover_instructions(input[field].as_str().unwrap_or(".")) {
                Ok(sources) if !sources.is_empty() => {
                    for source in sources {
                        self.event(hooks, "project_instructions", source)?;
                    }
                    return Ok(Some(Outcome::failure(
                        "已加载该目录的 AGENTS.md 指令。请按新指令重新调用工具。",
                    )));
                }
                Err(error) => return Ok(Some(Outcome::failure(error.to_string()))),
                _ => {}
            }
        }
        Ok(None)
    }
    async fn authorize(&self, call: &Block, hooks: &Hooks) -> Result<bool> {
        let Block::ToolUse { id, name, input } = call else {
            bail!("invalid tool call")
        };
        Self::check(hooks)?;
        let settings = hooks.settings.lock().unwrap().clone();
        let decision = tools::policy(name, &settings.mode);
        if decision == "deny" {
            return Ok(false);
        }
        if decision == "allow" {
            return Ok(true);
        }
        let request = json!({"call_id":id,"tool_name":name,"arguments":input,"summary":format!("{name}: {}",util::prefix(&input.to_string(),1000))});
        self.event(
            hooks,
            "approval_request",
            json!({"call_id":id,"name":name,"arguments":input}),
        )?;
        let granted = if settings.always_allow.contains(name) {
            (hooks.sink)(
                json!({"event":"approval_auto","sessionId":self.session,"request":request}),
            );
            true
        } else {
            let granted = self.ask(hooks, name, request).await?;
            Self::phase(hooks, "tools");
            granted
        };
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
        let Block::ToolUse { name, input, id } = &call else {
            unreachable!()
        };
        let outcome = if !granted {
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
            let duration = deadline
                .map(|d| {
                    d.saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(300))
                })
                .unwrap_or(Duration::from_secs(300));
            let result = if name == "bash" {
                tools::execute(name, input, &ctx).await
            } else {
                tokio::time::timeout(duration, tools::execute(name, input, &ctx))
                    .await
                    .unwrap_or_else(|_| Outcome::failure("tool timed out"))
            };
            Self::check(hooks)?;
            result
        };
        self.record_result(
            &call,
            outcome,
            hooks,
            recovered,
            started.elapsed().as_secs_f64(),
        )
    }
    async fn invoke(&mut self, call: Block, hooks: &Hooks, recovered: bool) -> Result<Block> {
        if let Block::ToolUse { name, input, .. } = &call
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
                self.delegate(
                    util::string(input, "kind")?,
                    util::string(input, "task")?,
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
            images: if outcome.success {
                outcome.images.clone()
            } else {
                vec![]
            },
        };
        static ARTIFACT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let artifact = ARTIFACT
            .get_or_init(|| regex::Regex::new(r"\[artifact:([A-Za-z0-9_-]+)\]").unwrap())
            .captures(&outcome.output)
            .map(|c| c[1].to_string());
        let event=self.store.checkpoint(&self.session,"tool_call_result",&json!({"call_id":id,"name":name,"success":outcome.success,"exit_code":outcome.exit_code,"error":outcome.error,"output_preview":util::prefix(&outcome.output,1000),"output_detail":outcome.output,"artifact_id":artifact,"truncated":artifact.is_some()||outcome.output.contains("truncated"),"duration_s":duration,"images":outcome.images,"recovered":recovered}),None,Some(&result),None)?;
        (hooks.sink)(json!({"event":"agent_event","sessionId":self.session,"item":event}));
        Ok(result)
    }
    async fn readonly_delegate(&self, call: &Block) -> Result<bool> {
        let Block::ToolUse { name, input, .. } = call else {
            return Ok(false);
        };
        if name != "delegate" || self.child {
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
        kind: &'a str,
        task: &'a str,
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
            drop(catalog);
            let key = (kind.to_string(), task.to_string());
            let mut child = if let Some(child) = self.children.remove(&key) {
                child
            } else {
                let paused = self
                    .store
                    .events(&self.session, -1)?
                    .iter()
                    .rev()
                    .find(|e| {
                        ["subagent_start", "subagent_result"].contains(&e.r#type.as_str())
                            && e.data["kind"] == kind
                            && e.data["task"] == task
                    })
                    .and_then(|e| {
                        if e.r#type == "subagent_start" || e.data["exit_reason"] != "completed" {
                            e.data["child_session_id"].as_str().map(str::to_string)
                        } else {
                            None
                        }
                    });
                let mut settings = hooks.settings.lock().unwrap().clone();
                settings.acceptance = None;
                Box::new(
                    Self::new(
                        &self.workspace,
                        &self.provider.configuration_id,
                        paused.as_deref(),
                        self.store.clone(),
                        configuration,
                        &settings,
                    )
                    .await?,
                )
            };
            child.child = true;
            child.goal = None;
            child.files = self.files.clone();
            // A delegated task must see the same pinned project instructions
            // and activated skills as the request that created it.
            {
                let parent_catalog = self.catalog.lock().await;
                let mut child_catalog = child.catalog.lock().await;
                child_catalog.instructions = parent_catalog.instructions.clone();
                child_catalog.active = parent_catalog.active.clone();
            }
            child.allowed = allowed.map(|mut set| {
                set.remove("delegate");
                set.retain(|s| !s.starts_with("task_"));
                set
            });
            child.provider = self.provider.clone();
            let requirements = self
                .messages
                .iter()
                .rev()
                .filter(|m| m.role == "user")
                .take(12)
                .map(Message::text)
                .collect::<Vec<_>>()
                .join("\n");
            child.custom_prompt = Some(format!(
                "\n你是子助手。只使用配置的工具，遵守父会话权限。最终只输出 JSON 对象，键为 summary（字符串）、findings（字符串列表）、evidence_refs（你实际读取过的工作区相对路径列表）、unresolved（字符串列表）。不得编造证据。\n父任务要求：\n{requirements}\n子助手指令：\n{instructions}"
            ));
            let child_id = child.session.clone();
            self.event(
                hooks,
                "subagent_start",
                json!({"kind":kind,"task":task,"child_session_id":child_id,"source":source}),
            )?;
            let child_settings = hooks.settings.clone();
            let parent_sink = hooks.sink.clone();
            let parent_session = self.session.clone();
            let sink: Sink = Arc::new(move |mut event| {
                if matches!(event["event"].as_str(), Some("approval" | "approval_auto")) {
                    event["sessionId"] = json!(parent_session);
                    parent_sink(event);
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
            let result = child.run(text, vec![], child_hooks).await?;
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
            let parsed = serde_json::from_str::<Value>(&final_text).ok().filter(|v| {
                v["summary"].is_string()
                    && ["findings", "evidence_refs", "unresolved"].iter().all(|k| {
                        v[k].as_array()
                            .is_some_and(|items| items.iter().all(Value::is_string))
                    })
            });
            let structured = parsed.is_some();
            let mut payload=parsed.unwrap_or(json!({"summary":final_text,"findings":[],"evidence_refs":[],"unresolved":["子助手未返回有效的结构化结果。"]}));
            let events = self.store.events(&child_id, -1)?;
            let succeeded = events
                .iter()
                .filter(|e| e.r#type == "tool_call_result" && e.data["success"] == true)
                .filter_map(|e| e.data["call_id"].as_str())
                .collect::<HashSet<_>>();
            let observed = events
                .iter()
                .filter(|e| {
                    e.r#type == "tool_call_start"
                        && succeeded.contains(e.data["call_id"].as_str().unwrap_or_default())
                })
                .filter_map(|e| e.data["arguments"]["path"].as_str())
                .collect::<HashSet<_>>();
            let requested = payload["evidence_refs"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut verified = vec![];
            for r in requested {
                if let Some(reference) = r.as_str() {
                    let path = reference.split(':').next().unwrap_or(reference);
                    if observed.contains(path)
                        && util::in_workspace(&self.workspace, path).is_ok_and(|p| p.exists())
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
            payload["status"] = json!(if structured {
                result.exit_reason.as_str()
            } else {
                "invalid_result"
            });
            self.event(hooks,"subagent_result",json!({"kind":kind,"task":task,"child_session_id":child_id,"exit_reason":result.exit_reason,"structured":structured,"usage":usage,"usage_scope":"activation","provider_progress":{"index":self.provider.index,"call_counter":self.provider.call_counter},"evidence_refs":verified,"source":source}))?;
            let completed = structured && result.exit_reason == "completed";
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
        self.message_revision = None;
        let changed = context::compact(
            &self.store,
            &self.session,
            &system,
            &specs,
            &mut self.messages,
            (self.provider.window as f64 / self.provider.prompt_scale) as u64,
            true,
        )?;
        self.message_revision = Some(self.store.message_revision(&self.session)?);
        if let Some(stats) = changed {
            self.event(hooks, "context_compacted", stats)?;
            self.refresh_snapshot(hooks).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
