use crate::{
    attachments::Attachments,
    configuration::Configuration,
    extensions::Catalog,
    goals::Spec,
    mcp::McpRegistry,
    models::Budget,
    runtime::{Approvals, Hooks, Ledger, Runtime, Settings, Snapshot},
    store::Store,
    tools::Sink,
    util,
};
use anyhow::{Context, Result, bail};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock, atomic::AtomicU64},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::Semaphore,
};
use tokio_util::sync::CancellationToken;

/// Bridge protocol spoken by this agent. Clients compare it during
/// `initialize` and ignore event types they do not know.
pub const PROTOCOL_VERSION: u64 = 2;
/// The oldest client protocol this agent still serves correctly.
pub const MIN_CLIENT_PROTOCOL: u64 = 2;
/// Optional features a client may rely on when listed here.
pub const CAPABILITIES: &[&str] = &[
    "workspaceTrust",
    "projectInstructionsChanged",
    "sessionPaging",
    "diagnosticEvents",
    "runIdentity",
];
pub struct Conversation {
    pub settings: Arc<Mutex<Settings>>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub approvals: Approvals,
    pub sequence: Arc<AtomicU64>,
    pub runtime: Option<Arc<tokio::sync::Mutex<Runtime>>>,
    pub cancel: Arc<Mutex<CancellationToken>>,
    pub client_key: String,
    pub running: Option<tokio::task::JoinHandle<()>>,
    pub statistics: Statistics,
    pub catalog: Option<Arc<tokio::sync::Mutex<Catalog>>>,
    pub workspace: Option<PathBuf>,
}
impl Conversation {
    fn new(configuration: &Configuration) -> Result<Self> {
        Ok(Self {
            settings: Arc::new(Mutex::new(Settings::new(configuration)?)),
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            approvals: Arc::new(Mutex::new(HashMap::new())),
            sequence: Arc::new(AtomicU64::new(0)),
            runtime: None,
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            client_key: "default".into(),
            running: None,
            statistics: Statistics::default(),
            catalog: None,
            workspace: None,
        })
    }
    fn busy(&self) -> bool {
        self.snapshot.lock().unwrap().running
    }
    fn hooks(&self, sink: Sink) -> Hooks {
        Hooks {
            settings: self.settings.clone(),
            snapshot: self.snapshot.clone(),
            approvals: self.approvals.clone(),
            approval_sequence: self.sequence.clone(),
            sink: {
                let run_id = self.snapshot.lock().unwrap().run_id.clone();
                Arc::new(move |mut value| {
                    if let Some(id) = &run_id {
                        value["runId"] = json!(id);
                    }
                    sink(value);
                })
            },
            cancel: self.cancel.lock().unwrap().clone(),
            shared: Arc::new(Mutex::new(Ledger::default())),
        }
    }
    fn state(&mut self, store: &Store) -> Result<Value> {
        let settings = self.settings.lock().unwrap();
        let snapshot = self.snapshot.lock().unwrap();
        let stats = self.statistics.read(store, snapshot.session.as_deref())?;
        let mut always = settings.always_allow.iter().cloned().collect::<Vec<_>>();
        always.sort();
        Ok(
            json!({"protocolVersion":PROTOCOL_VERSION,"model":settings.model,"runId":snapshot.run_id,"phase":snapshot.phase,"running":snapshot.running,"activeModel":if snapshot.active_model.is_empty(){&settings.model}else{&snapshot.active_model},"pendingSettings":settings.pending_model||settings.registry_dirty,"effort":settings.effort,"permissionMode":settings.mode,"sessionId":snapshot.session,"taskPending":snapshot.pending,"rounds":snapshot.rounds,"contextTokens":snapshot.breakdown.as_object().map(|m|m.values().filter_map(Value::as_u64).sum::<u64>()).unwrap_or(0),"contextBreakdown":snapshot.breakdown,"contextWindow":snapshot.window,"usage":snapshot.usage,"statistics":stats,"budget":settings.budget,"acceptance":settings.acceptance.as_ref().map(|p|p.to_string_lossy().into_owned()).unwrap_or_default(),"alwaysAllow":always}),
        )
    }
}
// Controls never acquire the async conversation lock.
struct Owner {
    conversation: tokio::sync::Mutex<Conversation>,
    settings: Arc<Mutex<Settings>>,
    snapshot: Arc<Mutex<Snapshot>>,
    approvals: Approvals,
    cancel: Arc<Mutex<CancellationToken>>,
}
impl Owner {
    fn new(c: Conversation) -> Self {
        Self {
            settings: c.settings.clone(),
            snapshot: c.snapshot.clone(),
            approvals: c.approvals.clone(),
            cancel: c.cancel.clone(),
            conversation: tokio::sync::Mutex::new(c),
        }
    }
}
#[derive(Default)]
struct Routes {
    clients: HashMap<String, Arc<Owner>>,
    sessions: HashMap<String, Arc<Owner>>,
    recent: VecDeque<String>,
}
pub struct Router {
    pub store: Store,
    pub configuration: Configuration,
    routes: RwLock<Routes>,
    configuration_write: tokio::sync::Mutex<()>,
    pub sink: Sink,
}
impl Router {
    pub fn new(store: Store, sink: Sink) -> Self {
        Self {
            store,
            configuration: Configuration::default(),
            routes: RwLock::new(Routes::default()),
            configuration_write: tokio::sync::Mutex::new(()),
            sink,
        }
    }
    fn workspace(params: &Value) -> Result<PathBuf> {
        util::canonical(Path::new(
            params["workspace"]
                .as_str()
                .unwrap_or(&util::home().to_string_lossy()),
        ))
    }
    async fn ensure(
        &self,
        c: &mut Conversation,
        workspace: &Path,
        session: Option<&str>,
        model: Option<&str>,
    ) -> Result<()> {
        let active = c.snapshot.lock().unwrap().session.clone();
        if c.busy() {
            if c.workspace.as_deref() != Some(workspace) {
                bail!("当前任务仍在运行，不能替换会话或工作区")
            }
            if session.is_some() && active.as_deref() != session {
                bail!("当前任务仍在运行，不能替换会话或工作区")
            }
            return Ok(());
        }
        let mut reload_session = None;
        let different = if let Some(runtime) = &c.runtime {
            let r = runtime.lock().await;
            let stale = r.workspace == workspace
                && session.is_none_or(|s| s == r.session)
                && Some(self.store.message_revision(&r.session)?) != r.message_revision;
            if stale {
                reload_session = Some(r.session.clone());
            }
            r.workspace != workspace || session.is_some_and(|s| s != r.session) || stale
        } else {
            true
        };
        let session = session.or(reload_session.as_deref());
        if different {
            let settings = c.settings.lock().unwrap().clone();
            let summary = session.map(|id| self.store.require(id)).transpose()?;
            let model = model
                .map(str::to_string)
                .or_else(|| {
                    summary
                        .as_ref()
                        .and_then(|s| s["model"].as_str().map(str::to_string))
                })
                .unwrap_or(settings.model.clone());
            let r = Runtime::new(
                workspace,
                &model,
                session,
                self.store.clone(),
                self.configuration.clone(),
                &settings,
            )
            .await?;
            if session.is_none()
                && c.workspace.as_deref() == Some(workspace)
                && let Some(previous) = &c.catalog
            {
                let active = previous.lock().await.active.clone();
                let mut catalog = r.catalog.lock().await;
                for (name, digest) in active {
                    if catalog.load(&name)?.digest != digest {
                        bail!("activated skill changed before session start: {name}")
                    }
                }
            }
            if let Some(saved) = self
                .store
                .events(&r.session, -1)?
                .iter()
                .rev()
                .find(|e| ["budget_changed", "session_start"].contains(&e.r#type.as_str()))
                && settings.budget.max_total_tokens == 0
            {
                let cap = saved.data["budget"]["max_total_tokens"]
                    .as_i64()
                    .unwrap_or(0);
                if cap != 0 {
                    c.settings.lock().unwrap().budget.max_total_tokens = cap;
                }
            }
            c.settings.lock().unwrap().model = model;
            c.settings.lock().unwrap().always_allow.clear();
            let hooks = c.hooks(self.event_sink(&c.client_key));
            r.refresh_snapshot(&hooks).await?;
            c.catalog = Some(r.catalog.clone());
            c.workspace = Some(workspace.into());
            c.runtime = Some(Arc::new(tokio::sync::Mutex::new(r)));
            c.settings.lock().unwrap().registry_dirty = false;
        } else if c.settings.lock().unwrap().registry_dirty {
            if let Some(runtime) = &c.runtime {
                let mut r = runtime.lock().await;
                let catalog = r.catalog.lock().await.refresh()?;
                r.catalog = Arc::new(tokio::sync::Mutex::new(catalog));
                c.catalog = Some(r.catalog.clone());
            }
            c.settings.lock().unwrap().registry_dirty = false;
        }
        Ok(())
    }
    fn event_sink(&self, key: &str) -> Sink {
        let sink = self.sink.clone();
        let key = key.to_string();
        Arc::new(move |mut value| {
            value["clientKey"] = json!(key);
            sink(value);
        })
    }
    async fn start(
        &self,
        c: &mut Conversation,
        text: Option<String>,
        attachments: Vec<crate::models::Block>,
    ) -> Result<Value> {
        if c.busy() {
            bail!("当前任务仍在运行")
        }
        let runtime = c.runtime.clone().context("请先选择工作区")?;
        let session = runtime.lock().await.session.clone();
        *c.cancel.lock().unwrap() = CancellationToken::new();
        let sink = self.event_sink(&c.client_key);
        let returned_session = session.clone();
        let run_id = util::id();
        {
            let mut snapshot = c.snapshot.lock().unwrap();
            snapshot.running = true;
            snapshot.phase = "preparing".into();
            snapshot.run_id = Some(run_id.clone());
        }
        let hooks = c.hooks(sink);
        let sink = hooks.sink.clone();
        sink(json!({"event":"run_started","sessionId":session}));
        let task_run_id = run_id.clone();
        let driver = tokio::runtime::Handle::current();
        c.running = Some(tokio::task::spawn_blocking(move || {
            driver.block_on(async move {
                // Runtime::run catches panics in its loop; this guard covers a
                // panic anywhere else in the run so the session never stays
                // "running" and its jobs and MCP servers are stopped.
                let run = std::panic::AssertUnwindSafe(async {
                    let mut r = runtime.lock().await;
                    r.run(text, attachments, hooks.clone()).await
                })
                .catch_unwind()
                .await;
                let error = match run {
                    Ok(Ok(result)) => {
                        sink(json!({"event":"run_done","sessionId":session,"result":result}));
                        return;
                    }
                    Ok(Err(error)) => error.to_string(),
                    Err(panic) => {
                        hooks.cancel.cancel();
                        let r = runtime.lock().await;
                        r.mcp.close().await;
                        hooks
                            .approvals
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clear();
                        format!("internal panic: {}", util::panic_message(&*panic))
                    }
                };
                let mut s = hooks
                    .snapshot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if s.run_id.as_deref() == Some(&task_run_id) {
                    s.running = false;
                    s.pending = true;
                    s.phase = "paused".into();
                }
                drop(s);
                sink(json!({"event":"run_error","sessionId":session,"error":error}));
            })
        }));
        Ok(json!({"sessionId":returned_session,"runId":run_id}))
    }
    pub async fn handle(&self, method: &str, params: &Value) -> Result<Value> {
        if !params.is_object() {
            bail!("params must be an object")
        }
        for (m, field) in [
            ("resolveApproval", "granted"),
            ("setPluginEnabled", "enabled"),
            ("setAgentEnabled", "enabled"),
            ("setSkillActive", "active"),
            ("setLocalExtensionEnabled", "enabled"),
        ] {
            if method == m && !params[field].is_boolean() {
                bail!("{field} must be a boolean")
            }
        }
        let _configuration_write = if [
            "saveService",
            "deleteService",
            "saveAgent",
            "deleteAgent",
            "setAgentEnabled",
            "setDefaultModel",
            "setPluginEnabled",
            "lockPlugins",
            "setLocalExtensionEnabled",
            "trustWorkspace",
            "saveInstructions",
        ]
        .contains(&method)
        {
            Some(self.configuration_write.lock().await)
        } else {
            None
        };
        if ["renameSession", "pinSession", "deleteSession"].contains(&method) {
            let id = util::string(params, "sessionId")?;
            self.store.require(id)?;
            match method {
                "renameSession" => {
                    let title = params["title"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .chars()
                        .take(120)
                        .collect::<String>();
                    self.store
                        .rename(id, if title.is_empty() { None } else { Some(&title) })?;
                }
                "pinSession" => self.store.pin(id, params["pinned"] == true)?,
                _ => {
                    let owner = self.routes.read().unwrap().sessions.get(id).cloned();
                    let conversation = if let Some(owner) = &owner {
                        Some(owner.conversation.lock().await)
                    } else {
                        None
                    };
                    if conversation.as_ref().is_some_and(|c| c.busy()) {
                        bail!("任务运行中，无法删除会话")
                    }
                    self.store.delete(id)?;
                    let mut routes = self.routes.write().unwrap();
                    if let Some(owner) = routes.sessions.remove(id) {
                        routes.clients.retain(|_, c| !Arc::ptr_eq(c, &owner));
                    }
                    routes.recent.retain(|s| s != id);
                }
            }
            return Ok(json!(self.store.sessions()?));
        }
        let key = params["clientKey"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("default")
            .to_string();
        let session = params["sessionId"].as_str();
        let lookup = |routes: &Routes| {
            session
                .and_then(|s| routes.sessions.get(s))
                .cloned()
                .or_else(|| routes.clients.get(&key).cloned())
        };
        let owner = { lookup(&self.routes.read().unwrap()) };
        if ["cancelTurn", "resolveApproval"].contains(&method) {
            let Some(owner) = owner else {
                return Ok(json!(false));
            };
            if session.is_some() && owner.snapshot.lock().unwrap().session.as_deref() != session {
                return Ok(json!(false));
            }
            if method == "cancelTurn" {
                owner.cancel.lock().unwrap().cancel();
                return Ok(json!(true));
            }
            let id = util::string(params, "approvalId")?;
            let entry = owner.approvals.lock().unwrap().remove(id);
            if let Some((tool, tx)) = entry {
                let granted = params["granted"] == true;
                if granted && params["remember"] == "session" {
                    owner.settings.lock().unwrap().always_allow.insert(tool);
                }
                return Ok(json!(tx.send(granted).is_ok()));
            }
            return Ok(json!(false));
        }
        let candidate = if owner.is_none() {
            let c = Conversation::new(&self.configuration)?;
            let source = params["sourceClientKey"]
                .as_str()
                .and_then(|s| self.routes.read().unwrap().clients.get(s).cloned());
            if let Some(source) = source {
                let mut settings = source.settings.lock().unwrap().clone();
                settings.always_allow.clear();
                settings.pending_model = false;
                settings.registry_dirty = false;
                let default = self.configuration.default_model()?;
                if !default.is_empty() {
                    settings.model = default;
                }
                *c.settings.lock().unwrap() = settings;
            }
            Some(Arc::new(Owner::new(c)))
        } else {
            None
        };
        let owner = {
            let mut routes = self.routes.write().unwrap();
            let owner = lookup(&routes).or(owner).or(candidate).unwrap();
            routes.clients.insert(key.clone(), owner.clone());
            // Reserve before loading: concurrent clients share one runtime.
            if let Some(id) = session {
                routes.sessions.insert(id.into(), owner.clone());
            }
            owner
        };
        let mut c = owner.conversation.lock().await;
        if !c.busy() {
            c.client_key = key;
        }
        let result = self.dispatch(&mut c, method, params).await;
        let current = c.snapshot.lock().unwrap().session.clone();
        // Publish while still owning the conversation lock.
        {
            let mut routes = self.routes.write().unwrap();
            routes
                .sessions
                .retain(|id, v| !Arc::ptr_eq(v, &owner) || Some(id) == current.as_ref());
            if let Some(id) = current {
                routes.sessions.insert(id.clone(), owner.clone());
                routes.recent.retain(|s| s != &id);
                routes.recent.push_back(id);
            }
            let mut attempts = routes.recent.len();
            while routes.sessions.len() > 6 && attempts > 0 {
                attempts -= 1;
                let Some(id) = routes.recent.pop_front() else {
                    break;
                };
                let Some(candidate) = routes.sessions.get(&id).cloned() else {
                    continue;
                };
                let references = routes
                    .clients
                    .values()
                    .chain(routes.sessions.values())
                    .filter(|c| Arc::ptr_eq(c, &candidate))
                    .count();
                // Do not evict runs or owners held by in-flight requests.
                if Arc::strong_count(&candidate) == references + 1
                    && candidate.conversation.try_lock().is_ok_and(|c| !c.busy())
                {
                    routes.sessions.remove(&id);
                    routes.clients.retain(|_, c| !Arc::ptr_eq(c, &candidate));
                } else {
                    routes.recent.push_back(id);
                }
            }
        }
        drop(c);
        let result = result?;
        let clients = || {
            self.routes
                .read()
                .unwrap()
                .clients
                .values()
                .cloned()
                .collect::<Vec<_>>()
        };
        if [
            "setPluginEnabled",
            "lockPlugins",
            "setLocalExtensionEnabled",
            "trustWorkspace",
        ]
        .contains(&method)
        {
            for c in clients() {
                c.settings.lock().unwrap().registry_dirty = true;
            }
        }
        if ["saveService", "deleteService"].contains(&method) {
            let models = self.configuration.models()?;
            let default = self.configuration.default_model()?;
            for c in clients() {
                let mut settings = c.settings.lock().unwrap();
                if settings.model != "fake"
                    && !models
                        .iter()
                        .any(|m| m["id"] == settings.model && m["available"] == true)
                {
                    settings.model = default.clone();
                }
                if c.snapshot.lock().unwrap().session.is_some() {
                    settings.pending_model = true;
                } else {
                    settings.model = self.configuration.default_model()?;
                }
            }
        }
        Ok(result)
    }
    async fn dispatch(&self, c: &mut Conversation, method: &str, p: &Value) -> Result<Value> {
        let store = &self.store;
        match method {
            "getConfiguration" => self.configuration.public(),
            "listModels" => Ok(json!(self.configuration.models()?)),
            "saveService" => self.configuration.save_service(&p["service"]),
            "saveAgent" => Ok(json!(self.configuration.save_agent(&p["agent"])?)),
            "deleteService" => {
                let mut data = self.configuration.read()?;
                data["services"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|s| s["id"] != p["id"]);
                self.configuration.write(&data)?;
                if !self
                    .configuration
                    .models()?
                    .iter()
                    .any(|m| m["id"] == data["defaultModel"])
                {
                    data["defaultModel"] = json!("");
                    self.configuration.write(&data)?;
                }
                self.configuration.public()
            }
            "setDefaultModel" => {
                let model = util::string(p, "model")?;
                if !model.is_empty()
                    && !self
                        .configuration
                        .models()?
                        .iter()
                        .any(|m| m["id"] == model && m["available"] == true)
                {
                    bail!("请先配置并启用该模型的服务")
                };
                let mut data = self.configuration.read()?;
                data["defaultModel"] = json!(model);
                self.configuration.write(&data)?;
                self.configuration.public()
            }
            "setAgentEnabled" | "deleteAgent" => {
                let name = util::string(p, "name")?;
                let mut data = self.configuration.read()?;
                if method == "deleteAgent" {
                    data["agents"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|a| a["name"] != name);
                    if let Some(a) = data["disabledAgents"].as_array_mut() {
                        a.retain(|a| a != name);
                    }
                } else {
                    if !self
                        .configuration
                        .agents()?
                        .iter()
                        .any(|a| a["name"] == name)
                    {
                        bail!("找不到子助手")
                    };
                    if !data["disabledAgents"].is_array() {
                        data["disabledAgents"] = json!([]);
                    }
                    let names = data["disabledAgents"].as_array_mut().unwrap();
                    names.retain(|n| n != name);
                    if p["enabled"] != true {
                        names.push(json!(name));
                    }
                }
                self.configuration.write(&data)?;
                Ok(json!(self.configuration.agents()?))
            }
            "getInstructions" | "saveInstructions" => {
                let path = self.configuration.path.parent().unwrap().join("AGENTS.md");
                if util::is_link(&path) || util::is_link(path.parent().unwrap()) {
                    bail!("global instructions path is a link or junction")
                }
                if method == "saveInstructions" {
                    let content = util::string(p, "content")?;
                    if content.len() > 64_000 || content.contains('\0') {
                        bail!("全局指令不能超过 64 KB 或包含 NUL")
                    };
                    util::atomic_write(&path, content.as_bytes())?;
                }
                let content = if path.is_file() {
                    crate::extensions::Source::read(&path, 64_000)?.content
                } else {
                    String::new()
                };
                Ok(
                    json!({"path":path,"content":content.trim_start_matches('\u{feff}'),"exists":path.is_file()}),
                )
            }
            "initialize" => {
                let model = c.settings.lock().unwrap().model.clone();
                Ok(
                    json!({"protocolVersion":PROTOCOL_VERSION,"minClientProtocol":MIN_CLIENT_PROTOCOL,"capabilities":CAPABILITIES,"sessions":store.sessions()?,"models":self.configuration.models()?,"defaultModel":model,"commands":serde_json::from_str::<Value>(include_str!("assets/commands.json"))?,"state":c.state(store)?}),
                )
            }
            "listSessions" => Ok(json!(store.sessions()?)),
            "listTasks" => Ok(json!(if let Some(id) = p["sessionId"].as_str() {
                store.tasks(id)?
            } else {
                vec![]
            })),
            "getSession" | "getSessionPage" => {
                let cursor = if method == "getSessionPage" {
                    Some(p.get("cursor").context("history cursor required")?)
                } else {
                    None
                };
                let limit = p
                    .get("limit")
                    .map(|v| {
                        v.as_u64()
                            .filter(|n| *n > 0)
                            .context("limit must be a positive integer")
                    })
                    .transpose()?
                    .unwrap_or(100)
                    .min(100) as usize;
                store.history_page(util::string(p, "sessionId")?, cursor, limit)
            }
            "readArtifact" => store.artifact_page(
                util::string(p, "sessionId")?,
                util::string(p, "artifactId")?,
                p["offset"].as_u64().unwrap_or(0) as usize,
                20_000,
            ),
            "selectSession" if p["deferRuntime"] == true => {
                let id = util::string(p, "sessionId")?;
                if c.snapshot.lock().unwrap().session.as_deref() != Some(id) {
                    if c.busy() {
                        bail!("当前任务仍在运行，不能替换会话或工作区")
                    }
                    let summary = store.require(id)?;
                    let mut settings = c.settings.lock().unwrap();
                    settings.model = util::string(&summary, "model")?.into();
                    settings.always_allow.clear();
                    if settings.budget.max_total_tokens == 0 {
                        settings.budget.max_total_tokens = store.saved_budget(id)?.unwrap_or(0);
                    }
                    *c.snapshot.lock().unwrap() = Snapshot {
                        session: Some(id.into()),
                        pending: summary["status"] != "completed",
                        rounds: summary["rounds"].as_u64().unwrap_or(0),
                        usage: Some(serde_json::from_value({
                            let mut usage = summary.clone();
                            usage["available"] = summary["usage_available"].clone();
                            usage
                        })?),
                        ..Snapshot::default()
                    };
                    c.runtime = None;
                    c.catalog = None;
                    c.workspace = Some(PathBuf::from(util::string(&summary, "workspace")?));
                }
                c.state(store)
            }
            "selectSession" | "getState" => {
                if let Some(id) = p["sessionId"].as_str()
                    && c.snapshot.lock().unwrap().session.as_deref() != Some(id)
                {
                    let summary = store.require(id)?;
                    self.ensure(
                        c,
                        Path::new(util::string(&summary, "workspace")?),
                        Some(id),
                        Some(util::string(&summary, "model")?),
                    )
                    .await?;
                }
                c.state(store)
            }
            "resetSession" => {
                if c.busy() {
                    bail!("当前任务仍在运行")
                };
                c.runtime = None;
                c.catalog = None;
                c.workspace = None;
                *c.snapshot.lock().unwrap() = Snapshot::default();
                c.settings.lock().unwrap().always_allow.clear();
                c.state(store)
            }
            "setModel" | "setTuiProvider" => {
                let choice = if method == "setTuiProvider" {
                    p["provider"].as_str().unwrap_or("auto").to_lowercase()
                } else {
                    "auto".into()
                };
                if !["auto", "fake", "anthropic", "commandcode"].contains(&choice.as_str()) {
                    bail!("未知 provider：{choice}")
                }
                let mut model = p["model"].as_str().unwrap_or_default().to_string();
                if choice == "fake"
                    || (choice == "auto"
                        && (model == "fake"
                            || (method == "setTuiProvider"
                                && std::env::var("MINICODE_FAKE_SCRIPT")
                                    .ok()
                                    .is_some_and(|s| !s.trim().is_empty()))))
                {
                    model = "fake".into();
                } else if choice != "auto" {
                    if model.is_empty() {
                        model = if choice == "anthropic" {
                            "claude-sonnet-4-5"
                        } else {
                            "deepseek/deepseek-v4.1-flash"
                        }
                        .into();
                    }
                    let configured = self.configuration.find_model(&model)?;
                    let provider = configured
                        .map(|(s, _)| {
                            if s["apiStyle"] == "anthropic" {
                                "anthropic"
                            } else {
                                "commandcode"
                            }
                        })
                        .unwrap_or(if model.starts_with("claude") {
                            "anthropic"
                        } else {
                            "commandcode"
                        });
                    if provider != choice {
                        bail!("模型 {model} 不属于 provider {choice}")
                    }
                }
                if model.is_empty() {
                    model = c.settings.lock().unwrap().model.clone();
                }
                let provider = crate::provider::Provider::new(
                    &model,
                    &c.settings.lock().unwrap().effort,
                    &self.configuration,
                )?;
                let busy = c.busy();
                {
                    let mut settings = c.settings.lock().unwrap();
                    settings.model = model.clone();
                    settings.pending_model = busy;
                }
                if !busy && let Some(runtime) = &c.runtime {
                    let mut r = runtime.lock().await;
                    let mut provider = provider;
                    if provider.identity().is_some() && provider.identity() == r.provider.identity()
                    {
                        provider.index = r.provider.index;
                        provider.call_counter = r.provider.call_counter;
                    }
                    r.provider = provider;
                    store.update(
                        &r.session,
                        &json!({"model":model,"provider":r.provider.name}),
                    )?;
                    r.refresh_snapshot(&c.hooks(self.event_sink(&c.client_key)))
                        .await?;
                }
                c.state(store)
            }
            "setEffort" => {
                let effort = util::string(p, "effort")?.trim().to_lowercase();
                if !["off", "low", "medium", "high", "xhigh", "max"].contains(&effort.as_str()) {
                    bail!("无效 reasoning effort")
                }
                let model = c.settings.lock().unwrap().model.clone();
                let provider = if model.is_empty() {
                    None
                } else {
                    let provider =
                        crate::provider::Provider::new(&model, &effort, &self.configuration)?;
                    if effort != "off" && provider.effort != effort {
                        bail!("当前模型不支持 reasoning effort");
                    }
                    Some(provider)
                };
                if !c.busy()
                    && let Some(runtime) = &c.runtime
                {
                    let mut r = runtime.lock().await;
                    if let Some(provider) = provider {
                        r.provider.effort = provider.effort;
                    }
                }
                {
                    let busy = c.busy();
                    let mut s = c.settings.lock().unwrap();
                    s.effort = effort;
                    s.pending_model |= busy;
                }
                c.state(store)
            }
            "setPermissionMode" => {
                let mode = util::string(p, "mode")?
                    .trim()
                    .to_lowercase()
                    .replace('-', "_");
                if !["default", "accept_edits", "bypass"].contains(&mode.as_str()) {
                    bail!("无效权限模式")
                };
                c.settings.lock().unwrap().mode = mode;
                c.state(store)
            }
            "setBudget" => {
                let budget: Budget = serde_json::from_value(p.clone())?;
                if !budget.max_seconds.is_finite() || budget.max_seconds < 0.0 {
                    bail!("invalid max_seconds")
                };
                c.settings.lock().unwrap().budget = budget;
                c.state(store)
            }
            "setAcceptance" => {
                let path = p["path"].as_str().unwrap_or("").trim();
                let path = if path.is_empty() {
                    None
                } else {
                    let path = util::canonical(Path::new(path))?;
                    Spec::read(&path)?;
                    Some(path)
                };
                c.settings.lock().unwrap().acceptance = path;
                c.state(store)
            }
            "clearAlwaysAllow" => {
                c.settings.lock().unwrap().always_allow.clear();
                c.state(store)
            }
            "getCapabilities" => self.capabilities(&Self::workspace(p)?, c).await,
            "getLocalExtensions" | "setLocalExtensionEnabled" => {
                let workspace = Self::workspace(p)?;
                let catalog = Catalog::scan(&workspace, self.configuration.clone(), false)?;
                if method == "setLocalExtensionEnabled" {
                    catalog.set_local(util::string(p, "id")?, p["enabled"] == true)?;
                    let catalog = Catalog::scan(&workspace, self.configuration.clone(), false)?;
                    Ok(json!({"entries":catalog.local,"diagnostics":[]}))
                } else {
                    Ok(json!({"entries":catalog.local,"diagnostics":[]}))
                }
            }
            "getWorkspaceTrust" | "trustWorkspace" => {
                let workspace = Self::workspace(p)?;
                let mut catalog = Catalog::scan(&workspace, self.configuration.clone(), false)?;
                if method == "trustWorkspace" {
                    let trusted = p["trusted"] != false;
                    // Callers pass the fingerprints the user reviewed; without
                    // them, the plugins currently awaiting trust are meant.
                    let reviewed = match p["plugins"].as_array() {
                        Some(plugins) => plugins
                            .iter()
                            .map(|p| {
                                Ok((
                                    util::string(p, "name")?.to_string(),
                                    p["sha256"].as_str().unwrap_or_default().to_string(),
                                ))
                            })
                            .collect::<Result<Vec<_>>>()?,
                        None => catalog
                            .trust_required()
                            .iter()
                            .map(|p| {
                                (
                                    p["name"].as_str().unwrap_or_default().to_string(),
                                    p["sha256"].as_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect(),
                    };
                    catalog.set_trust(&reviewed, trusted)?;
                    catalog = Catalog::scan(&workspace, self.configuration.clone(), false)?;
                }
                Ok(
                    json!({"workspace":workspace,"plugins":catalog.plugins.iter().filter(|p|p["commands"].as_array().is_some_and(|c|!c.is_empty())).collect::<Vec<_>>()}),
                )
            }
            "setPluginEnabled" | "lockPlugins" => {
                let workspace = Self::workspace(p)?;
                let catalog = Catalog::scan(&workspace, self.configuration.clone(), false)?;
                if method == "lockPlugins" {
                    Ok(json!(catalog.write_lock()?))
                } else {
                    catalog.set_plugin(util::string(p, "name")?, p["enabled"] == true)?;
                    self.capabilities(&workspace, c).await
                }
            }
            "setSkillActive" => {
                let workspace = Self::workspace(p)?;
                if c.runtime.is_some() || p["sessionId"].is_string() {
                    self.ensure(c, &workspace, p["sessionId"].as_str(), None)
                        .await?;
                } else if c.catalog.is_none() || c.workspace.as_deref() != Some(workspace.as_path())
                {
                    c.catalog = Some(Arc::new(tokio::sync::Mutex::new(Catalog::scan(
                        &workspace,
                        self.configuration.clone(),
                        true,
                    )?)));
                    c.workspace = Some(workspace.clone());
                }
                let name = util::string(p, "name")?;
                let catalog = c
                    .catalog
                    .as_ref()
                    .context("skill catalog unavailable")?
                    .clone();
                let mut catalog = catalog.lock().await;
                let session = c.snapshot.lock().unwrap().session.clone();
                let data = if p["active"] == true {
                    let source = catalog.load(name)?;
                    json!({"name":name,"path":source.path,"sha256":source.digest})
                } else {
                    catalog.unload(name)?;
                    json!({"name":name})
                };
                let kind = if p["active"] == true {
                    "skill_activated"
                } else {
                    "skill_deactivated"
                };
                if let Some(session) = session {
                    let event = store.event(&session, kind, &data)?;
                    (self.event_sink(&c.client_key))(
                        json!({"event":"agent_event","sessionId":session,"item":event}),
                    );
                }
                Ok(json!(format!(
                    "{}技能 {name}",
                    if p["active"] == true {
                        "已激活"
                    } else {
                        "已停用"
                    }
                )))
            }
            "importAttachments" => {
                let paths = p["paths"]
                    .as_array()
                    .filter(|v| !v.is_empty() && v.len() <= 10)
                    .context("请选择 1 至 10 个附件")?;
                let attachments = Attachments {
                    root: store.root().join("attachments"),
                };
                let blocks = paths
                    .iter()
                    .map(|p| {
                        attachments
                            .import(Path::new(p.as_str().context("invalid attachment path")?))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if blocks
                    .iter()
                    .map(|b| {
                        if let crate::models::Block::Media { size, .. } = b {
                            *size
                        } else {
                            0
                        }
                    })
                    .sum::<u64>()
                    > 24_000_000
                {
                    bail!("附件总大小不能超过 24 MB")
                };
                Ok(json!(blocks))
            }
            "attachmentPath" => {
                let attachments = Attachments {
                    root: store.root().join("attachments"),
                };
                let block = attachments.get(util::string(p, "assetId")?)?;
                attachments.encoded(&block)?;
                Ok(json!({"path":attachments.path(&block)?}))
            }
            "sendPrompt" => {
                if c.busy() {
                    bail!("当前任务仍在运行")
                };
                let attachments = Attachments {
                    root: store.root().join("attachments"),
                };
                let mut blocks = vec![];
                if let Some(files) = p["attachments"].as_array() {
                    if files.len() > 10 {
                        bail!("附件列表无效或超过 10 个")
                    };
                    for f in files {
                        blocks.push(attachments.get(util::string(f, "asset_id")?)?);
                    }
                }
                let mut text = util::string(p, "text")?.trim().to_string();
                if text.is_empty() && !blocks.is_empty() {
                    text = "请分析这些附件".into();
                }
                if text.is_empty() {
                    bail!("请输入任务内容")
                };
                let workspace = Self::workspace(p)?;
                let model = p["model"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or(c.settings.lock().unwrap().model.clone());
                if model != "fake"
                    && !self
                        .configuration
                        .models()?
                        .iter()
                        .any(|m| m["id"] == model && m["available"] == true)
                {
                    if model.contains("::") {
                        bail!("请在设置中配置、启用并选择一个可用的 AI 模型");
                    }
                    crate::provider::Provider::new(
                        &model,
                        &c.settings.lock().unwrap().effort,
                        &self.configuration,
                    )?;
                }
                self.ensure(c, &workspace, p["sessionId"].as_str(), Some(&model))
                    .await?;
                {
                    let mut r = c.runtime.as_ref().unwrap().lock().await;
                    if r.provider.configuration_id != model {
                        r.provider = crate::provider::Provider::new(
                            &model,
                            &c.settings.lock().unwrap().effort,
                            &self.configuration,
                        )?;
                        store.update(
                            &r.session,
                            &json!({"model":model,"provider":r.provider.name}),
                        )?;
                    }
                    attachments.check(&blocks, &r.provider.modalities, &r.provider.style)?;
                    c.settings.lock().unwrap().model = model;
                }
                self.start(c, Some(text), blocks).await
            }
            "runSlash" => self.slash(c, p).await,
            _ => bail!("未知请求：{method}"),
        }
    }
    async fn capabilities(&self, workspace: &Path, c: &Conversation) -> Result<Value> {
        let catalog = Catalog::scan(workspace, self.configuration.clone(), false)?;
        let active = if let Some(catalog) = &c.catalog {
            catalog.lock().await.active.clone()
        } else if let Some(id) = c.snapshot.lock().unwrap().session.as_deref() {
            self.store.active_skills(id)?
        } else {
            BTreeActive::default()
        };
        let agents = self.configuration.agents()?;
        Ok(
            json!({"skills":catalog.skills.values().map(|s|json!({"name":s.name,"description":s.description,"origin":s.origin,"active":active.contains_key(&s.name)})).collect::<Vec<_>>(),"plugins":catalog.plugins,"mcp":catalog.servers.iter().map(|s|json!({"name":s.name,"plugin":s.plugin})).collect::<Vec<_>>(),"agents":agents.iter().filter(|a|a["enabled"]==true).map(|a|a["name"].clone()).chain(catalog.agents.keys().map(|s|json!(s))).collect::<Vec<_>>(),"agentDefinitions":agents}),
        )
    }
    async fn slash(&self, c: &mut Conversation, p: &Value) -> Result<Value> {
        let text = util::string(p, "text")?;
        let (verb, arg) = text.split_once(' ').unwrap_or((text, ""));
        let verb = verb.to_lowercase();
        let arg = arg.trim();
        if c.busy()
            && !["/help", "/?", "/model", "/effort", "/permissions", "/skill"]
                .contains(&verb.as_str())
        {
            bail!("当前回合结束后才能执行命令")
        }
        match verb.as_str() {
            "/help" | "/?" => {
                let commands: Vec<Value> =
                    serde_json::from_str(include_str!("assets/commands.json"))?;
                Ok(
                    json!({"message":format!("可用命令：\n{}",commands.iter().map(|c|format!("{}  {}",c["usage"].as_str().unwrap_or_default(),c["summary"].as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n"))}),
                )
            }
            "/exit" | "/quit" => Ok(json!({"action":"exit"})),
            "/clear" => Ok(json!({"action":"clear","message":"已清屏，会话上下文保留。"})),
            "/new" => {
                c.runtime = None;
                *c.snapshot.lock().unwrap() = Snapshot::default();
                c.settings.lock().unwrap().always_allow.clear();
                Ok(json!({"action":"new","message":"已开启新会话。"}))
            }
            "/sessions" => {
                let sessions = self.store.sessions()?;
                let text = sessions
                    .iter()
                    .take(20)
                    .map(|s| {
                        format!(
                            "{} · {} · {}",
                            &s["session_id"].as_str().unwrap_or_default()[..8],
                            s["title"].as_str().unwrap_or_default(),
                            s["status"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(json!({"message":if text.is_empty(){"暂无会话。".into()}else{text}}))
            }
            "/resume" => {
                if arg.is_empty() {
                    return Ok(json!({"action":"sessions"}));
                }
                let matches = self
                    .store
                    .sessions()?
                    .into_iter()
                    .filter(|s| s["session_id"].as_str().is_some_and(|s| s.starts_with(arg)))
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    bail!("会话 ID 不唯一或不存在")
                };
                Ok(json!({"action":"resume","sessionId":matches[0]["session_id"]}))
            }
            "/model" | "/effort" | "/permissions" => {
                let action = verb.trim_start_matches('/');
                if arg.is_empty() {
                    return Ok(json!({"action":action,"state":c.state(&self.store)?}));
                };
                let (method, field) = match action {
                    "model" => ("setModel", "model"),
                    "effort" => ("setEffort", "effort"),
                    _ => ("setPermissionMode", "mode"),
                };
                let params = json!({field:arg});
                let state = Box::pin(self.dispatch(c, method, &params)).await?;
                Ok(json!({"state":state,"message":format!("{action}: {arg}")}))
            }
            "/skill" => {
                if arg.is_empty() {
                    return Ok(json!({"action":"skills"}));
                }
                let active = !arg.starts_with("off ");
                let name = arg.strip_prefix("off ").unwrap_or(arg);
                let mut params = p.clone();
                params["name"] = json!(name);
                params["active"] = json!(active);
                let message = Box::pin(self.dispatch(c, "setSkillActive", &params)).await?;
                Ok(json!({"message":message}))
            }
            "/continue" | "/compact" => {
                let workspace = Self::workspace(p)?;
                self.ensure(c, &workspace, p["sessionId"].as_str(), None)
                    .await?;
                if verb == "/continue" {
                    if !c.snapshot.lock().unwrap().pending {
                        bail!("当前没有暂停的任务")
                    };
                    self.start(c, None, vec![]).await?;
                    Ok(json!({"action":"continue","state":c.state(&self.store)?}))
                } else {
                    let runtime = c.runtime.as_ref().unwrap().clone();
                    let changed = runtime
                        .lock()
                        .await
                        .compact(&c.hooks(self.event_sink(&c.client_key)))
                        .await?;
                    Ok(
                        json!({"message":if changed{"上下文已压缩。"}else{"当前没有可压缩的历史。"},"state":c.state(&self.store)?}),
                    )
                }
            }
            _ => bail!("未知命令：{verb}"),
        }
    }
    fn cancel_all(&self) {
        let routes = self.routes.read().unwrap();
        for c in routes.clients.values().chain(routes.sessions.values()) {
            c.cancel.lock().unwrap().cancel();
        }
    }
    pub async fn close(&self) {
        self.cancel_all();
        let mut seen = HashSet::new();
        let owners = {
            let routes = self.routes.read().unwrap();
            routes
                .clients
                .values()
                .chain(routes.sessions.values())
                .cloned()
                .collect::<Vec<_>>()
        };
        for c in &owners {
            c.cancel.lock().unwrap().cancel();
        }
        for c in &owners {
            let ptr = Arc::as_ptr(c) as usize;
            if !seen.insert(ptr) {
                continue;
            }
            let mut c = c.conversation.lock().await;
            if let Some(handle) = c.running.take() {
                let _ = handle.await;
            }
        }
    }
}
type BTreeActive = std::collections::BTreeMap<String, String>;
#[derive(Default)]
pub struct Statistics {
    session: Option<String>,
    after: i64,
    samples: VecDeque<Value>,
    calls: u64,
    requests: u64,
    seconds: f64,
    generation: f64,
    measured: u64,
    generated: u64,
}
impl Statistics {
    fn read(&mut self, store: &Store, session: Option<&str>) -> Result<Value> {
        if self.session.as_deref() != session {
            *self = Self {
                session: session.map(str::to_string),
                after: -1,
                ..Self::default()
            };
        }
        if let Some(id) = session {
            let (until, events) = store.statistics_events(id, self.after)?;
            for e in events {
                if e.r#type == "tool_call_start" {
                    self.calls += 1
                } else if e.r#type == "assistant_message" {
                    let usage = &e.data["usage"];
                    let output = usage["output_tokens"].as_u64().unwrap_or(0);
                    let available = usage["available"] != false;
                    let seconds = e.data["request_seconds"].as_f64();
                    let generation = e.data["generation_seconds"].as_f64();
                    self.requests += 1;
                    let request_tps = seconds
                        .filter(|s| *s > 0.0 && available)
                        .map(|s| output as f64 / s);
                    let tps = generation
                        .filter(|s| *s > 0.0 && available)
                        .map(|s| output as f64 / s);
                    if request_tps.is_some() {
                        self.seconds += seconds.unwrap();
                        self.measured += output
                    }
                    if tps.is_some() {
                        self.generation += generation.unwrap();
                        self.generated += output
                    }
                    self.samples.push_back(json!({"round":self.requests,"input":usage["input_tokens"],"output":output,"cached":usage["cache_read_tokens"],"available":available,"seconds":seconds,"generationSeconds":generation,"firstTokenSeconds":e.data["first_token_seconds"],"requestTps":request_tps,"tps":tps}));
                    if self.samples.len() > 32 {
                        self.samples.pop_front();
                    }
                }
            }
            self.after = until;
        }
        Ok(
            json!({"toolCalls":self.calls,"requests":self.requests,"modelSeconds":self.seconds,"tps":if self.generation>0.0{Some(self.generated as f64/self.generation)}else{None},"requestTps":if self.seconds>0.0{Some(self.measured as f64/self.seconds)}else{None},"lastTps":self.samples.back().map(|v|v["tps"].clone()).unwrap_or(Value::Null),"samples":self.samples}),
        )
    }
}
pub async fn network(method: &str, p: &Value, configuration: &Configuration) -> Result<Value> {
    if method == "refreshMcp" {
        let workspace = Router::workspace(p)?;
        let catalog = Catalog::scan(&workspace, configuration.clone(), true)?;
        let registry = McpRegistry::prepare(catalog.servers).await;
        let response =
            json!({"servers":registry.statuses,"tools":registry.tools.keys().collect::<Vec<_>>()});
        registry.close().await;
        return Ok(response);
    }
    let draft = &p["service"];
    let existing = configuration.read()?["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == draft["id"])
        .cloned()
        .unwrap_or(json!({}));
    let mut service = existing.clone();
    for (k, v) in draft.as_object().context("service must be an object")? {
        service[k] = v.clone();
    }
    if service["apiKey"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        service["apiKey"] = existing["apiKey"].clone();
    }
    let (url, key) = configuration.credentials(&service);
    if key.is_empty() {
        bail!("请填写 API 密钥")
    };
    let parsed = reqwest::Url::parse(&url)?;
    if !["http", "https"].contains(&parsed.scheme()) {
        bail!("接口地址必须是 HTTP(S) URL")
    }
    let endpoint =
        if service["apiStyle"] == "anthropic" && !url.trim_end_matches('/').ends_with("/v1") {
            format!("{}/v1/models", url.trim_end_matches('/'))
        } else {
            format!("{}/models", url.trim_end_matches('/'))
        };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let request = client.get(endpoint);
    let request = if service["apiStyle"] == "anthropic" {
        request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
    } else {
        request.bearer_auth(key)
    };
    let response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("连接失败，请检查地址、网络和接口格式"))?;
    if !response.status().is_success() {
        bail!("连接失败（HTTP {}）", response.status().as_u16())
    }
    let data: Value = response.json().await?;
    let models=data["data"].as_array().unwrap_or(&vec![]).iter().filter(|m|m["id"].is_string()).map(|m|json!({"modelId":m["id"],"name":m.get("display_name").unwrap_or(&m["id"]),"contextWindow":200000,"maxOutputTokens":8192,"supportsEffort":false})).collect::<Vec<_>>();
    Ok(json!({"message":format!("已连接，发现 {} 个模型",models.len()),"models":models}))
}
// The producer only enqueues values. Serialization, stdout writes and flushes
// run on a dedicated blocking task, preserving FIFO event/reply ordering.
fn output_writer(
    output: impl std::io::Write + Send + 'static,
) -> (Sink, tokio::task::JoinHandle<()>) {
    use std::io::Write;
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let writer = tokio::task::spawn_blocking(move || {
        let mut out = std::io::BufWriter::new(output);
        for value in rx {
            if serde_json::to_writer(&mut out, &value).is_err()
                || out.write_all(b"\n").is_err()
                || out.flush().is_err()
            {
                break;
            }
        }
    });
    (
        Arc::new(move |value| {
            let _ = tx.send(value);
        }),
        writer,
    )
}

pub async fn serve(store: Store) -> Result<()> {
    let (sink, writer) = output_writer(std::io::stdout());
    let router = Arc::new(Router::new(store, sink.clone()));
    let configuration = Configuration::default();
    let ids = Arc::new(Mutex::new(HashSet::<String>::new()));
    let semaphore = Arc::new(Semaphore::new(72));
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let mut line = Vec::new();
        let mut over = false;
        loop {
            let buf = reader.fill_buf().await?;
            if buf.is_empty() {
                break;
            }
            let n = buf
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(buf.len());
            if line.len() + n > 4_000_000 {
                over = true
            } else {
                line.extend_from_slice(&buf[..n]);
            }
            let complete = buf[n - 1] == b'\n';
            reader.consume(n);
            if complete {
                break;
            }
        }
        if line.is_empty() && !over {
            break;
        }
        let parsed = (|| -> Result<(Value, String, Value)> {
            if over {
                bail!("request exceeds the size limit")
            };
            let v: Value = serde_json::from_slice(&line)?;
            if !v.is_object() {
                bail!("request must be an object")
            };
            let id = v["id"].clone();
            if !id.is_string() && !id.is_i64() {
                bail!("request id must be a string or integer")
            };
            let method = util::string(&v, "method")?.to_string();
            let params = v.get("params").cloned().unwrap_or(json!({}));
            if !params.is_object() {
                bail!("method must be a string and params must be an object")
            };
            Ok((id, method, params))
        })();
        let (id, method, params) = match parsed {
            Ok(v) => v,
            Err(e) => {
                let id = serde_json::from_slice::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                    .filter(|v| v.is_string() || v.is_i64());
                sink(if let Some(id) = id {
                    json!({"id":id,"error":e.to_string()})
                } else {
                    json!({"event":"bridge_error","error":e.to_string()})
                });
                continue;
            }
        };
        let serialized = id.to_string();
        let count = ids.lock().unwrap().len();
        let reserved = if ["cancelTurn", "resolveApproval"].contains(&method.as_str()) {
            8
        } else {
            0
        };
        if ids.lock().unwrap().contains(&serialized) {
            sink(json!({"event":"bridge_error","error":"duplicate in-flight request id"}));
            continue;
        }
        if count >= 64 + reserved {
            sink(json!({"id":id,"error":"request queue is full"}));
            continue;
        }
        let permit = semaphore.clone().acquire_owned().await?;
        ids.lock().unwrap().insert(serialized.clone());
        let ids = ids.clone();
        let router = router.clone();
        let sink = sink.clone();
        let configuration = configuration.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let value = if ["cancelTurn", "resolveApproval"].contains(&method.as_str()) {
                router.handle(&method, &params).await
            } else {
                // SQLite, repository scanning and attachment import are synchronous.
                // Keep them off the workers that drive stdin and cancellations.
                let driver = tokio::runtime::Handle::current();
                tokio::task::spawn_blocking(move || {
                    driver.block_on(async {
                        if ["fetchServiceModels", "testServiceConnection", "refreshMcp"]
                            .contains(&method.as_str())
                        {
                            network(&method, &params, &configuration).await
                        } else {
                            router.handle(&method, &params).await
                        }
                    })
                })
                .await
                .unwrap_or_else(|error| Err(anyhow::anyhow!("bridge request task failed: {error}")))
            };
            let mut reply = json!({"id":id});
            match value {
                Ok(value) => reply["result"] = value,
                Err(error) => reply["error"] = json!(error.to_string()),
            }
            sink(reply);
            ids.lock().unwrap().remove(&serialized);
        });
        while tasks.try_join_next().is_some() {}
    }
    // Blocking requests cannot be aborted once started. Drain their handles
    // before closing owners and the writer so shutdown cannot orphan work.
    router.cancel_all();
    while tasks.join_next().await.is_some() {}
    router.close().await;
    drop(router);
    drop(sink);
    writer.await?;
    Ok(())
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;

    #[tokio::test]
    async fn ensure_checks_metadata_without_reading_message_bodies() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(":memory:").unwrap();
        let session = store
            .create(&root.path().to_string_lossy(), "fake", "fake")
            .unwrap();
        store
            .append(&session, &crate::models::Message::user("saved"))
            .unwrap();
        let mut router = Router::new(store.clone(), Arc::new(|_| {}));
        router.configuration = Configuration {
            path: root.path().join("config.json"),
        };
        let mut c = Conversation::new(&router.configuration).unwrap();
        router
            .ensure(&mut c, root.path(), Some(&session), Some("fake"))
            .await
            .unwrap();
        // The metadata is unchanged; selecting a message body now fails at
        // the SQL boundary. A freshness check must not touch that column.
        store.with(|db| { db.execute_batch("ALTER TABLE messages RENAME TO saved_messages; CREATE VIEW messages AS SELECT session_id,seq,role,json('invalid JSON') AS content FROM saved_messages;")?; Ok(()) }).unwrap();
        router
            .ensure(&mut c, root.path(), Some(&session), Some("fake"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn slow_stdout_is_queued_and_drained_in_order() {
        struct SlowOutput(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for SlowOutput {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                std::thread::sleep(Duration::from_millis(150));
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (sink, writer) = output_writer(SlowOutput(bytes.clone()));
        let start = std::time::Instant::now();
        sink(json!({"event":"log","text":"first"}));
        sink(json!({"id":1,"result":true}));
        assert!(start.elapsed() < Duration::from_millis(100));
        drop(sink);
        writer.await.unwrap();
        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let messages = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            vec![
                json!({"event":"log","text":"first"}),
                json!({"id":1,"result":true})
            ]
        );
    }

    #[tokio::test]
    async fn one_locked_conversation_does_not_block_another() {
        let router = Arc::new(Router::new(
            Store::open(":memory:").unwrap(),
            Arc::new(|_| {}),
        ));
        router
            .handle("getState", &json!({"clientKey":"slow"}))
            .await
            .unwrap();
        let owner = router.routes.read().unwrap().clients["slow"].clone();
        let _guard = owner.conversation.lock().await;
        let slow = {
            let router = router.clone();
            tokio::spawn(async move {
                router
                    .handle("getState", &json!({"clientKey":"slow"}))
                    .await
            })
        };
        tokio::task::yield_now().await;
        tokio::time::timeout(
            Duration::from_millis(100),
            router.handle("getState", &json!({"clientKey":"other"})),
        )
        .await
        .unwrap()
        .unwrap();
        slow.abort();
    }

    #[tokio::test]
    async fn controls_do_not_wait_for_the_conversation_lock() {
        let router = Router::new(Store::open(":memory:").unwrap(), Arc::new(|_| {}));
        router.handle("getState", &json!({})).await.unwrap();
        let owner = router
            .routes
            .read()
            .unwrap()
            .clients
            .get("default")
            .unwrap()
            .clone();
        let c = owner.conversation.lock().await;
        let token = c.cancel.lock().unwrap().clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        c.approvals
            .lock()
            .unwrap()
            .insert("approval".into(), ("write".into(), tx));
        for (method, params) in [
            ("cancelTurn", json!({})),
            (
                "resolveApproval",
                json!({"approvalId":"approval","granted":true}),
            ),
        ] {
            assert_eq!(
                tokio::time::timeout(Duration::from_millis(100), router.handle(method, &params))
                    .await
                    .expect("control blocked by conversation lock")
                    .unwrap(),
                true
            );
        }
        assert!(token.is_cancelled());
        assert!(rx.await.unwrap());
    }
}
