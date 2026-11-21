mod output;
mod protocol;
mod statistics;

pub use protocol::{CAPABILITIES, MIN_CLIENT_PROTOCOL, PROTOCOL_VERSION};
pub use statistics::{Statistics, network};

use crate::{
    attachments::Attachments,
    configuration::Configuration,
    extensions::Catalog,
    goals::Spec,
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
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::Semaphore,
};
use tokio_util::sync::CancellationToken;

pub struct Conversation {
    pub settings: Arc<Mutex<Settings>>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub approvals: Approvals,
    pub sequence: Arc<AtomicU64>,
    pub runtime: Option<Arc<tokio::sync::Mutex<Runtime>>>,
    pub cancel: Arc<Mutex<CancellationToken>>,
    background: Arc<Mutex<std::sync::Weak<crate::background::Jobs>>>,
    pub client_key: String,
    listeners: Arc<RwLock<HashSet<String>>>,
    pub running: Option<tokio::task::JoinHandle<()>>,
    pub statistics: Arc<Mutex<Statistics>>,
    preparing: bool,
    preparing_new: bool,
    settings_session: Option<String>,
    pub catalog: Option<Arc<tokio::sync::Mutex<Catalog>>>,
    pub workspace: Option<PathBuf>,
    configuration: Configuration,
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
            background: Default::default(),
            client_key: "default".into(),
            listeners: Arc::new(RwLock::new(HashSet::new())),
            running: None,
            statistics: Arc::new(Mutex::new(Statistics::default())),
            preparing: false,
            preparing_new: false,
            settings_session: None,
            catalog: None,
            workspace: None,
            configuration: configuration.clone(),
        })
    }
    fn busy(&self) -> bool {
        self.snapshot.lock().unwrap().running && !self.preparing
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
    fn state(&self, store: &Store) -> Result<Value> {
        snapshot_state(
            &self.settings,
            &self.snapshot,
            &self.statistics,
            &self.configuration,
            store,
        )
    }
}
fn snapshot_state(
    settings: &Mutex<Settings>,
    snapshot: &Mutex<Snapshot>,
    statistics: &Mutex<Statistics>,
    configuration: &Configuration,
    store: &Store,
) -> Result<Value> {
    let settings = settings.lock().unwrap();
    let snapshot = snapshot.lock().unwrap();
    let active_model = if snapshot.active_model.is_empty() {
        settings.model.as_str()
    } else {
        snapshot.active_model.as_str()
    };
    let stats = statistics.lock().unwrap().read(
        store,
        configuration,
        active_model,
        snapshot.session.as_deref(),
    )?;
    let mut always = settings.always_allow.iter().cloned().collect::<Vec<_>>();
    always.sort();
    Ok(
        json!({"protocolVersion":PROTOCOL_VERSION,"model":settings.model,"runId":snapshot.run_id,"phase":snapshot.phase,"running":snapshot.running,"activeModel":if snapshot.active_model.is_empty(){&settings.model}else{&snapshot.active_model},"pendingSettings":settings.pending_model||settings.registry_dirty,"effort":settings.effort,"permissionMode":settings.mode,"sessionId":snapshot.session,"taskPending":snapshot.pending,"rounds":snapshot.rounds,"contextTokens":snapshot.breakdown.as_object().map(|m|m.values().filter_map(Value::as_u64).sum::<u64>()).unwrap_or(0),"contextBreakdown":snapshot.breakdown,"contextWindow":snapshot.window,"branch":snapshot.branch,"usage":snapshot.usage,"statistics":stats,"budget":settings.budget,"acceptance":settings.acceptance.as_ref().map(|p|p.to_string_lossy().into_owned()).unwrap_or_default(),"alwaysAllow":always}),
    )
}
// Controls never acquire the async conversation lock.
struct Owner {
    listeners: Arc<RwLock<HashSet<String>>>,
    preparation: Mutex<Option<tokio::task::JoinHandle<()>>>,
    statistics: Arc<Mutex<Statistics>>,
    conversation: tokio::sync::Mutex<Conversation>,
    settings: Arc<Mutex<Settings>>,
    snapshot: Arc<Mutex<Snapshot>>,
    approvals: Approvals,
    cancel: Arc<Mutex<CancellationToken>>,
    background: Arc<Mutex<std::sync::Weak<crate::background::Jobs>>>,
}
impl Owner {
    fn new(c: Conversation) -> Self {
        Self {
            listeners: c.listeners.clone(),
            preparation: Mutex::new(None),
            statistics: c.statistics.clone(),
            settings: c.settings.clone(),
            snapshot: c.snapshot.clone(),
            approvals: c.approvals.clone(),
            cancel: c.cancel.clone(),
            background: c.background.clone(),
            conversation: tokio::sync::Mutex::new(c),
        }
    }
}
#[derive(Default)]
struct Routes {
    clients: HashMap<String, Arc<Owner>>,
    sessions: HashMap<String, Arc<Owner>>,
    recent: VecDeque<String>,
    saved_settings: HashMap<String, Settings>,
}
impl Routes {
    fn bind(&mut self, owner: &Arc<Owner>, key: &str, listen: bool) -> bool {
        if let Some(previous) = self.clients.insert(key.into(), owner.clone())
            && !Arc::ptr_eq(&previous, owner)
        {
            previous.listeners.write().unwrap().remove(key);
        }
        listen && owner.listeners.write().unwrap().insert(key.into())
    }
    fn trim(&mut self) {
        let mut attempts = self.recent.len();
        while self.sessions.len() > 6 && attempts > 0 {
            attempts -= 1;
            let Some(id) = self.recent.pop_front() else {
                break;
            };
            let Some(candidate) = self.sessions.get(&id).cloned() else {
                continue;
            };
            let references = self
                .clients
                .values()
                .chain(self.sessions.values())
                .filter(|c| Arc::ptr_eq(c, &candidate))
                .count();
            // Do not evict runs or owners held by in-flight requests.
            if Arc::strong_count(&candidate) == references + 1
                && candidate.conversation.try_lock().is_ok_and(|c| {
                    if c.busy() {
                        return false;
                    }
                    // Eviction closes the Runtime. Session-owned processes,
                    // connections and retained job captures outlive turns.
                    if c.runtime.as_ref().is_some_and(|r| match r.try_lock() {
                        Ok(r) => r.holds_resources(),
                        Err(_) => true,
                    }) {
                        return false;
                    }
                    self.saved_settings
                        .insert(id.clone(), c.settings.lock().unwrap().clone());
                    true
                })
            {
                self.sessions.remove(&id);
                self.clients.retain(|_, c| !Arc::ptr_eq(c, &candidate));
            } else {
                self.recent.push_back(id);
            }
        }
    }
}
#[derive(Clone)]
pub struct Router {
    pub store: Store,
    pub configuration: Configuration,
    routes: Arc<RwLock<Routes>>,
    configuration_write: Arc<tokio::sync::Mutex<()>>,
    pub sink: Sink,
}
impl Router {
    pub fn new(store: Store, sink: Sink) -> Self {
        Self {
            store,
            configuration: Configuration::default(),
            routes: Arc::new(RwLock::new(Routes::default())),
            configuration_write: Arc::new(tokio::sync::Mutex::new(())),
            sink,
        }
    }
    pub(super) fn workspace(params: &Value) -> Result<PathBuf> {
        util::canonical(Path::new(
            params["workspace"]
                .as_str()
                .unwrap_or(&util::home().to_string_lossy()),
        ))
    }
    fn session_settings(&self, c: &Conversation, session: Option<&str>) -> Settings {
        if session.is_some() && c.settings_session.as_deref() == session {
            return c.settings.lock().unwrap().clone();
        }
        if let Some(saved) =
            session.and_then(|id| self.routes.read().unwrap().saved_settings.get(id).cloned())
        {
            return saved;
        }
        let mut settings = c.settings.lock().unwrap().clone();
        settings.always_allow.clear();
        settings
    }
    fn commit_settings(&self, c: &mut Conversation, session: &str, settings: Settings) {
        if let Some(previous) = &c.settings_session
            && previous != session
        {
            self.routes
                .write()
                .unwrap()
                .saved_settings
                .insert(previous.clone(), c.settings.lock().unwrap().clone());
        }
        *c.settings.lock().unwrap() = settings;
        c.settings_session = Some(session.into());
    }
    fn reset_settings(&self, c: &mut Conversation) {
        c.settings.lock().unwrap().always_allow.clear();
        if let Some(previous) = c.settings_session.take() {
            self.routes
                .write()
                .unwrap()
                .saved_settings
                .insert(previous, c.settings.lock().unwrap().clone());
        }
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
            let mut settings = self.session_settings(c, session);
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
            if (session.is_none() || c.preparing_new)
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
            if let Some(cap) = self.store.saved_budget(&r.session)?
                && settings.budget.max_total_tokens == 0
            {
                settings.budget.max_total_tokens = cap;
            }
            settings.model = model;
            settings.pending_model = false;
            if !c.preparing && c.snapshot.lock().unwrap().running {
                bail!("当前任务仍在运行")
            }
            let hooks = c.hooks(self.event_sink(&c.listeners));
            if let Some(previous) = &c.runtime {
                previous.lock().await.close_resources(&hooks).await?;
            }
            r.refresh_snapshot(&hooks).await?;
            if !c.preparing
                && summary.is_some()
                && !self.store.queued_prompts(&r.session)?.is_empty()
            {
                c.snapshot.lock().unwrap().queue_paused = true;
            }
            self.commit_settings(c, &r.session, settings);
            c.catalog = Some(r.catalog.clone());
            c.workspace = Some(workspace.into());
            *c.background.lock().unwrap() = Arc::downgrade(&r.background);
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
    fn event_sink(&self, listeners: &Arc<RwLock<HashSet<String>>>) -> Sink {
        let sink = self.sink.clone();
        let listeners = listeners.clone();
        Arc::new(move |value| {
            let keys = listeners
                .read()
                .unwrap()
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            for key in keys {
                let mut event = value.clone();
                event["clientKey"] = json!(key);
                sink(event);
            }
        })
    }
    fn observed_state(&self, owner: &Owner, key: &str, joined: bool) -> Result<Value> {
        let state = snapshot_state(
            &owner.settings,
            &owner.snapshot,
            &owner.statistics,
            &self.configuration,
            &self.store,
        )?;
        if joined && state["running"] == true {
            (self.sink)(
                json!({"event":"run_started","clientKey":key,"sessionId":state["sessionId"],"runId":state["runId"],"state":state}),
            );
        }
        Ok(state)
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
        if !c.preparing {
            *c.cancel.lock().unwrap() = CancellationToken::new();
        }
        let sink = self.event_sink(&c.listeners);
        let returned_session = session.clone();
        let run_id = if c.preparing {
            c.snapshot
                .lock()
                .unwrap()
                .run_id
                .clone()
                .context("missing accepted run")?
        } else {
            util::id()
        };
        {
            let mut snapshot = c.snapshot.lock().unwrap();
            snapshot.running = true;
            snapshot.phase = "preparing".into();
            snapshot.run_id = Some(run_id.clone());
        }
        let hooks = c.hooks(sink);
        let sink = hooks.sink.clone();
        if !c.preparing {
            sink(json!({"event":"run_started","sessionId":session}));
        }
        let task_run_id = run_id.clone();
        let driver = tokio::runtime::Handle::current();
        let store = self.store.clone();
        let base_sink = self.event_sink(&c.listeners);
        c.running = Some(tokio::task::spawn_blocking(move || {
            driver.block_on(async move {
                let mut hooks = hooks;
                let mut current_run = task_run_id;
                let mut text = text;
                let mut attachments = attachments;
                loop {
                    let event_sink = base_sink.clone();
                    let event_run = current_run.clone();
                    hooks.sink = Arc::new(move |mut event| {
                        event["runId"] = json!(event_run);
                        event_sink(event);
                    });
                    let sink = hooks.sink.clone();
                    let run = std::panic::AssertUnwindSafe(async {
                        let mut runtime = runtime.lock().await;
                        runtime
                            .run(text.take(), std::mem::take(&mut attachments), hooks.clone())
                            .await
                    })
                    .catch_unwind()
                    .await;
                    let result = match run {
                        Ok(Ok(result)) => result,
                        failure => {
                            hooks.cancel.cancel();
                            let runtime = runtime.lock().await;
                            let _ = runtime.close_resources(&hooks).await;
                            runtime.mcp.close().await;
                            hooks
                                .approvals
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .clear();
                            let error = match failure {
                                Ok(Err(error)) => error.to_string(),
                                Err(panic) => {
                                    format!("internal panic: {}", util::panic_message(&*panic))
                                }
                                _ => unreachable!(),
                            };
                            let mut state = hooks
                                .snapshot
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if state.run_id.as_deref() == Some(&current_run) {
                                state.running = false;
                                state.pending = true;
                                state.queue_paused = true;
                                state.queued_id = None;
                                state.phase = "paused".into();
                            }
                            drop(state);
                            sink(json!({"event":"run_error","sessionId":session,"error":error}));
                            return;
                        }
                    };
                    let next = if result.exit_reason == "completed"
                        && !hooks.cancel.is_cancelled()
                        && !hooks.snapshot.lock().unwrap().queue_paused
                    {
                        store
                            .queued_prompts(&session)
                            .ok()
                            .and_then(|items| items.into_iter().next())
                    } else {
                        None
                    };
                    if let Some(next) = next {
                        let mut state = hooks.snapshot.lock().unwrap();
                        if state.run_id.as_deref() != Some(&current_run) {
                            drop(state);
                            sink(json!({"event":"run_done","sessionId":session,"result":result}));
                            return;
                        }
                        current_run = util::id();
                        state.run_id = Some(current_run.clone());
                        state.queued_id = next["id"].as_str().map(str::to_string);
                        state.running = true;
                        state.phase = "preparing".into();
                        drop(state);
                        sink(json!({"event":"run_done","sessionId":session,"result":result}));
                        base_sink(
                            json!({"event":"run_started","sessionId":session,"runId":current_run}),
                        );
                        text = next["text"].as_str().map(str::to_string);
                        attachments =
                            serde_json::from_value(next["attachments"].clone()).unwrap_or_default();
                    } else {
                        sink(json!({"event":"run_done","sessionId":session,"result":result}));
                        return;
                    }
                }
            })
        }));
        Ok(json!({"sessionId":returned_session,"runId":run_id}))
    }
    fn accept_prompt(&self, owner: Arc<Owner>, key: &str, params: &Value) -> Result<Value> {
        let mut preparation = owner.preparation.lock().unwrap();
        if owner.snapshot.lock().unwrap().running {
            bail!("当前任务仍在运行")
        }
        let workspace = Self::workspace(params)?;
        if !workspace.is_dir() {
            bail!("workspace no longer exists")
        }
        let text = util::string(params, "text")?;
        if text.trim().is_empty() && params["attachments"].as_array().is_none_or(Vec::is_empty) {
            bail!("请输入任务内容")
        }
        let settings = owner.settings.lock().unwrap().clone();
        let model = params["model"]
            .as_str()
            .filter(|m| !m.is_empty())
            .unwrap_or(&settings.model)
            .to_string();
        let provider = crate::provider::Provider::for_settings(
            &Settings {
                model: model.clone(),
                ..settings.clone()
            },
            &self.configuration,
            None,
        )?;
        let selected = owner.snapshot.lock().unwrap().session.clone();
        let requested = params["sessionId"].as_str().or(selected.as_deref());
        let mut new_session = false;
        let session = if let Some(id) = requested {
            let summary = self.store.require(id)?;
            if util::canonical(Path::new(util::string(&summary, "workspace")?))? == workspace {
                id.to_string()
            } else if params["sessionId"].is_string() {
                bail!("会话不属于当前工作区")
            } else {
                new_session = true;
                self.store
                    .create(&workspace.to_string_lossy(), &provider.name, &model)?
            }
        } else {
            new_session = true;
            self.store
                .create(&workspace.to_string_lossy(), &provider.name, &model)?
        };
        let run_id = util::id();
        {
            let mut snapshot = owner.snapshot.lock().unwrap();
            if snapshot.running {
                bail!("当前任务仍在运行")
            }
            *owner.cancel.lock().unwrap() = CancellationToken::new();
            snapshot.running = true;
            snapshot.phase = "preparing".into();
            snapshot.run_id = Some(run_id.clone());
            snapshot.session = Some(session.clone());
        }
        {
            let mut routes = self.routes.write().unwrap();
            routes.sessions.insert(session.clone(), owner.clone());
            routes.recent.retain(|id| id != &session);
            routes.recent.push_back(session.clone());
            routes.trim();
        }
        let sink = self.event_sink(&owner.listeners);
        sink(json!({"event":"run_started","sessionId":session,"runId":run_id,"phase":"preparing"}));
        let mut params = params.clone();
        params["sessionId"] = json!(session);
        params["model"] = json!(model);
        params["workspace"] = json!(workspace);
        let router = self.clone();
        let task_owner = owner.clone();
        let task_run = run_id.clone();
        let task_session = session.clone();
        let key = key.to_string();
        let driver = tokio::runtime::Handle::current();
        *preparation = Some(tokio::task::spawn_blocking(move || {
            driver.block_on(async move {
            let mut c = task_owner.conversation.lock().await;
            c.preparing = true;
            c.preparing_new = new_session;
            c.client_key = key;
            let prepared = std::panic::AssertUnwindSafe(router.dispatch(&mut c,"sendPrompt",&params)).catch_unwind().await;
            c.preparing = false;
            c.preparing_new = false;
            let error = match prepared {
                Ok(Ok(_)) => return,
                Ok(Err(e)) => e.to_string(),
                Err(panic) => format!("internal panic: {}",util::panic_message(&*panic)),
            };
            let mut snapshot = c.snapshot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if snapshot.run_id.as_deref() == Some(&task_run) {
                snapshot.running = false;
                snapshot.phase = "paused".into();
            }
            drop(snapshot);
            sink(json!({"event":"run_error","sessionId":task_session,"runId":task_run,"error":error}));
        })
        }));
        Ok(json!({"sessionId":session,"runId":run_id}))
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
                    if let Some(c) = &conversation
                        && let Some(runtime) = &c.runtime
                    {
                        runtime
                            .lock()
                            .await
                            .close_resources(&c.hooks(self.event_sink(&c.listeners)))
                            .await?;
                    }
                    self.store.delete(id)?;
                    let mut routes = self.routes.write().unwrap();
                    if let Some(owner) = routes.sessions.remove(id) {
                        routes.clients.retain(|_, c| !Arc::ptr_eq(c, &owner));
                    }
                    routes.recent.retain(|s| s != id);
                    routes.saved_settings.remove(id);
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
        let mut owner = { lookup(&self.routes.read().unwrap()) };
        if ["enqueuePrompt", "listQueue", "deleteQueuedPrompt"].contains(&method) {
            let owner = owner.as_ref().context("请先开始或恢复会话")?;
            let mut snapshot = owner.snapshot.lock().unwrap();
            let id = snapshot.session.clone().context("请先开始或恢复会话")?;
            if session.is_some_and(|requested| requested != id) {
                bail!("会话不匹配");
            }
            let queue = self.store.queued_prompts(&id)?;
            if method == "listQueue" {
                return Ok(json!(queue));
            }
            let result = if method == "deleteQueuedPrompt" {
                let queued_id = util::string(params, "id")?;
                if snapshot.queued_id.as_deref() == Some(queued_id) {
                    return Ok(json!(false));
                }
                if !queue.iter().any(|item| item["id"] == queued_id) {
                    return Ok(json!(false));
                }
                self.store
                    .event(&id, "queue_deleted", &json!({"id":queued_id}))?;
                json!(true)
            } else {
                let mode = params["mode"].as_str().unwrap_or("followUp");
                if !["steer", "followUp"].contains(&mode) {
                    bail!("mode must be steer or followUp");
                }
                let text = util::string(params, "text")?.trim();
                if text.is_empty() {
                    bail!("请输入任务内容");
                }
                if queue.len() >= 100 {
                    bail!("队列最多100条消息");
                }
                let assets = Attachments {
                    root: self.store.root().join("attachments"),
                };
                let blocks = params["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|file| assets.get(util::string(file, "asset_id")?))
                    .collect::<Result<Vec<_>>>()?;
                if blocks.len() > 10 {
                    bail!("附件超过10个");
                }
                let item = self.store.enqueue_prompt(&id, mode, text, &blocks)?;
                if !snapshot.running {
                    snapshot.queue_paused = true;
                }
                item
            };
            let queued = self.store.queued_prompts(&id)?;
            (self.event_sink(&owner.listeners))(
                json!({"event":"queue_changed","sessionId":id,"queue":queued,"paused":!snapshot.running}),
            );
            return Ok(result);
        }
        if ["cancelTurn", "resolveApproval"].contains(&method) {
            let Some(owner) = owner else {
                return Ok(json!(false));
            };
            if session.is_some() && owner.snapshot.lock().unwrap().session.as_deref() != session {
                return Ok(json!(false));
            }
            if method == "cancelTurn" {
                owner.cancel.lock().unwrap().cancel();
                let background = owner.background.lock().unwrap().upgrade();
                if let Some(jobs) = background {
                    jobs.cancel();
                    if !owner.snapshot.lock().unwrap().running {
                        jobs.close().await?;
                        let id = owner.snapshot.lock().unwrap().session.clone();
                        if let Some(id) = id {
                            jobs.flush(&self.store, &id, &self.event_sink(&owner.listeners))?;
                        }
                    }
                }
                return Ok(json!(true));
            }
            let id = util::string(params, "approvalId")?;
            let entry = owner.approvals.lock().unwrap().remove(id);
            if let Some((scope, tx)) = entry {
                let granted = params["granted"] == true;
                if granted
                    && matches!(params["remember"].as_str(), Some("session" | "project"))
                    && let Ok(scope) = serde_json::from_str::<Value>(&scope)
                {
                    let rule = util::string(&scope, "rule")?;
                    if params["remember"] == "project" {
                        if let Err(error) = crate::permissions::persist_project(
                            Path::new(util::string(&scope, "workspace")?),
                            rule,
                        ) {
                            owner
                                .approvals
                                .lock()
                                .unwrap()
                                .insert(id.to_string(), (scope.to_string(), tx));
                            return Err(error);
                        }
                    } else {
                        owner
                            .settings
                            .lock()
                            .unwrap()
                            .always_allow
                            .insert(rule.into());
                    }
                }
                return Ok(json!(tx.send(granted).is_ok()));
            }
            return Ok(json!(false));
        }
        if method == "getState"
            && let Some(owner) = &owner
        {
            let snapshot = owner.snapshot.lock().unwrap();
            let running = snapshot.running;
            if running && session.is_some() && snapshot.session.as_deref() != session {
                bail!("当前任务仍在运行，不能替换会话或工作区")
            }
            drop(snapshot);
            if running {
                let joined = self.routes.write().unwrap().bind(owner, &key, true);
                return self.observed_state(owner, &key, joined);
            }
        }
        // /new detaches this view; other subscribers keep their old session.
        let detach = method == "resetSession"
            || (method == "runSlash"
                && params["text"]
                    .as_str()
                    .filter(|text| text.split_whitespace().count() == 1)
                    .and_then(|s| s.split_whitespace().next())
                    .is_some_and(|s| {
                        s.eq_ignore_ascii_case("/new") || s.eq_ignore_ascii_case("/clear")
                    }));
        if detach && let Some(previous) = &owner {
            let mut previous = previous.conversation.lock().await;
            if previous.busy() {
                bail!("当前任务仍在运行")
            }
            let mut next = Conversation::new(&self.configuration)?;
            next.snapshot.lock().unwrap().branch = previous.snapshot.lock().unwrap().branch.clone();
            let mut settings = previous.settings.lock().unwrap().clone();
            settings.always_allow.clear();
            settings.pending_model = false;
            *next.settings.lock().unwrap() = settings;
            if method == "runSlash" {
                next.catalog = previous.catalog.clone();
                next.workspace = previous.workspace.clone();
            } else {
                // resetSession also invalidates the old runtime so a later
                // explicit resume re-pins changed instructions (R1).
                if let Some(runtime) = &previous.runtime {
                    runtime
                        .lock()
                        .await
                        .close_resources(&previous.hooks(self.event_sink(&previous.listeners)))
                        .await?;
                }
                previous.runtime = None;
                previous.catalog = None;
                previous.workspace = None;
            }
            drop(previous);
            owner = Some(Arc::new(Owner::new(next)));
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
        let (owner, joined) = {
            let mut routes = self.routes.write().unwrap();
            let owner = if detach {
                owner.or(candidate).unwrap()
            } else {
                lookup(&routes).or(owner).or(candidate).unwrap()
            };
            // Raw history reads do not subscribe an unrelated default view.
            let listen = ![
                "getSession",
                "getSessionPage",
                "readArtifact",
                "listSessions",
                "listTasks",
                "attachmentPath",
            ]
            .contains(&method);
            let joined = routes.bind(&owner, &key, listen);
            // Reserve before loading: concurrent clients share one runtime.
            if !detach && let Some(id) = session {
                routes.sessions.insert(id.into(), owner.clone());
            }
            (owner, joined)
        };
        if joined && owner.snapshot.lock().unwrap().running {
            self.observed_state(&owner, &key, true)?;
        }
        if method == "sendPrompt" && params["asyncStart"] == true {
            return self.accept_prompt(owner, &key, params);
        }
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
            routes.trim();
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
            "getCredentialImportInfo" => {
                if p["source"] != "commandcode" {
                    bail!("unsupported credential import source")
                }
                Ok(crate::configuration::commandcode_import_info())
            }
            "importCommandCode" => self
                .configuration
                .import_commandcode(p["confirmed"] == true),
            "listModels" => Ok(json!(self.configuration.models()?)),
            "saveService" => self.configuration.save_service(&p["service"]),
            "saveAgent" => Ok(json!(self.configuration.save_agent(&p["agent"])?)),
            "deleteService" => {
                self.configuration.update(|data| {
                    if p.get("revision").is_some()
                        && let Some(existing) = data["services"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|s| s["id"] == p["id"])
                    {
                        crate::configuration::check_revision(existing, p)?;
                    }
                    data["services"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|s| s["id"] != p["id"]);
                    if !self
                        .configuration
                        .models_from(data)?
                        .iter()
                        .any(|m| m["id"] == data["defaultModel"])
                    {
                        data["defaultModel"] = json!("");
                    }
                    Ok(())
                })?;
                self.configuration.public()
            }
            "setDefaultModel" => {
                let model = util::string(p, "model")?;
                self.configuration.update(|data| {
                    if !model.is_empty()
                        && !self
                            .configuration
                            .models_from(data)?
                            .iter()
                            .any(|m| m["id"] == model && m["available"] == true)
                    {
                        bail!("请先配置并启用该模型的服务")
                    };
                    data["defaultModel"] = json!(model);
                    Ok(())
                })?;
                self.configuration.public()
            }
            "setAgentEnabled" | "deleteAgent" => {
                let name = util::string(p, "name")?;
                self.configuration.update(|data| {
                    if p.get("revision").is_some()
                        && let Some(existing) = data["agents"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|a| a["name"] == name)
                    {
                        crate::configuration::check_revision(existing, p)?;
                    }
                    if method == "deleteAgent" {
                        data["agents"]
                            .as_array_mut()
                            .unwrap()
                            .retain(|a| a["name"] != name);
                        if let Some(a) = data["disabledAgents"].as_array_mut() {
                            a.retain(|a| a != name);
                        }
                    } else {
                        if !Configuration::agents_from(data)?
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
                        if let Some(agent) = data["agents"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .find(|a| a["name"] == name)
                        {
                            agent["revision"] = json!(
                                agent["revision"]
                                    .as_u64()
                                    .unwrap_or(0)
                                    .checked_add(1)
                                    .context("agent revision exhausted")?
                            );
                        }
                    }
                    Ok(())
                })?;
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
                c.snapshot.lock().unwrap().branch =
                    crate::environment::git_branch(&Self::workspace(p)?);
                let model = c.settings.lock().unwrap().model.clone();
                Ok(
                    json!({"protocolVersion":PROTOCOL_VERSION,"minClientProtocol":MIN_CLIENT_PROTOCOL,"capabilities":CAPABILITIES,"sessions":store.sessions()?,"models":self.configuration.models()?,"defaultModel":model,"commands":serde_json::from_str::<Value>(include_str!("assets/commands.json"))?,"state":c.state(store)?}),
                )
            }
            "listSessions" => {
                let offset = p
                    .get("offset")
                    .map(|v| {
                        v.as_u64()
                            .filter(|n| *n <= i64::MAX as u64)
                            .context("offset must be a non-negative integer")
                    })
                    .transpose()?
                    .unwrap_or(0) as usize;
                let limit = p
                    .get("limit")
                    .map(|v| {
                        v.as_u64()
                            .filter(|n| *n > 0)
                            .map(|n| n.min(1000) as usize)
                            .context("limit must be a positive integer")
                    })
                    .transpose()?;
                Ok(json!(store.sessions_page(offset, limit)?))
            }
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
            "listFiles" => {
                let workspace = Self::workspace(p)?;
                let query = p["query"].as_str().unwrap_or("").to_lowercase();
                let mut files = ignore::WalkBuilder::new(&workspace)
                    .hidden(false)
                    .require_git(false)
                    .filter_entry(|entry| entry.file_name() != ".git")
                    .build()
                    .take(50_000)
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
                    .filter_map(|entry| {
                        entry
                            .path()
                            .strip_prefix(&workspace)
                            .ok()
                            .map(|path| path.to_string_lossy().replace('\\', "/"))
                    })
                    .filter(|path| path.to_lowercase().contains(&query))
                    .collect::<Vec<_>>();
                files.sort();
                files.truncate(200);
                Ok(json!(files))
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
                    let mut settings = self.session_settings(c, Some(id));
                    settings.model = util::string(&summary, "model")?.into();
                    if settings.budget.max_total_tokens == 0 {
                        settings.budget.max_total_tokens = store.saved_budget(id)?.unwrap_or(0);
                    }
                    *c.snapshot.lock().unwrap() = Snapshot {
                        branch: crate::environment::git_branch(Path::new(util::string(
                            &summary,
                            "workspace",
                        )?)),
                        queue_paused: !store.queued_prompts(id)?.is_empty(),
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
                    self.commit_settings(c, id, settings);
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
                let mut snapshot = c.snapshot.lock().unwrap();
                let branch = snapshot.branch.clone();
                *snapshot = Snapshot {
                    branch,
                    ..Snapshot::default()
                };
                drop(snapshot);
                self.reset_settings(c);
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
                crate::provider::Provider::for_settings(
                    &Settings {
                        model: model.clone(),
                        ..c.settings.lock().unwrap().clone()
                    },
                    &self.configuration,
                    None,
                )?;
                let busy = c.busy();
                {
                    let mut settings = c.settings.lock().unwrap();
                    settings.model = model.clone();
                    settings.pending_model = busy;
                }
                if !busy && let Some(runtime) = &c.runtime {
                    let mut r = runtime.lock().await;
                    r.provider = crate::provider::Provider::for_settings(
                        &Settings {
                            model: model.clone(),
                            ..c.settings.lock().unwrap().clone()
                        },
                        &self.configuration,
                        Some(&r.provider),
                    )?;
                    store.update(
                        &r.session,
                        &json!({"model":model,"provider":r.provider.name}),
                    )?;
                    r.refresh_snapshot(&c.hooks(self.event_sink(&c.listeners)))
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
                    let provider = crate::provider::Provider::for_settings(
                        &Settings {
                            model: model.clone(),
                            effort: effort.clone(),
                            ..c.settings.lock().unwrap().clone()
                        },
                        &self.configuration,
                        None,
                    )?;
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
                    (self.event_sink(&c.listeners))(
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
                    && c.settings.lock().unwrap().observability.replay.is_none()
                    && !self
                        .configuration
                        .models()?
                        .iter()
                        .any(|m| m["id"] == model && m["available"] == true)
                {
                    if model.contains("::") {
                        bail!("请在设置中配置、启用并选择一个可用的 AI 模型");
                    }
                    crate::provider::Provider::for_settings(
                        &Settings {
                            model: model.clone(),
                            ..c.settings.lock().unwrap().clone()
                        },
                        &self.configuration,
                        None,
                    )?;
                }
                self.ensure(c, &workspace, p["sessionId"].as_str(), Some(&model))
                    .await?;
                {
                    let mut r = c.runtime.as_ref().unwrap().lock().await;
                    if r.provider.configuration_id != model {
                        r.provider = crate::provider::Provider::for_settings(
                            &Settings {
                                model: model.clone(),
                                ..c.settings.lock().unwrap().clone()
                            },
                            &self.configuration,
                            Some(&r.provider),
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
            "resumeQueue" => {
                if c.busy() {
                    bail!("当前任务仍在运行");
                }
                let workspace = Self::workspace(p)?;
                self.ensure(c, &workspace, p["sessionId"].as_str(), None)
                    .await?;
                let id = c
                    .snapshot
                    .lock()
                    .unwrap()
                    .session
                    .clone()
                    .context("没有会话")?;
                let next = self
                    .store
                    .queued_prompts(&id)?
                    .into_iter()
                    .next()
                    .context("队列为空")?;
                {
                    let mut state = c.snapshot.lock().unwrap();
                    state.queue_paused = false;
                    state.queued_id = next["id"].as_str().map(str::to_string);
                }
                self.start(
                    c,
                    next["text"].as_str().map(str::to_string),
                    serde_json::from_value(next["attachments"].clone())?,
                )
                .await
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
        let (verb, arg) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let verb = verb.to_lowercase();
        let arg = arg.trim();
        if [
            "/clear", "/new", "/cls", "/cost", "/context", "/doctor", "/init", "/agents", "/mcp",
        ]
        .contains(&verb.as_str())
            && !arg.is_empty()
        {
            bail!("{verb} 不接受参数");
        }
        let session = c.snapshot.lock().unwrap().session.clone();
        let state = c.state(&self.store)?;
        if let Some(message) = crate::commands::information(
            &verb,
            arg,
            &Self::workspace(p)?,
            session.as_deref(),
            &state,
            &self.store,
        )? {
            return Ok(json!({"message":message}));
        }
        if c.busy()
            && ![
                "/help",
                "/?",
                "/model",
                "/effort",
                "/permissions",
                "/skill",
                "/steer",
                "/follow",
                "/queue",
                "/cost",
                "/context",
                "/tasks",
            ]
            .contains(&verb.as_str())
        {
            bail!("当前回合结束后才能执行命令")
        }
        match verb.as_str() {
            "/steer" | "/follow" => {
                let mut params = p.clone();
                params["mode"] = json!(if verb == "/steer" {
                    "steer"
                } else {
                    "followUp"
                });
                params["text"] = json!(arg);
                let item = Box::pin(self.handle("enqueuePrompt", &params)).await?;
                Ok(json!({"message":format!("已排队 {}",item["id"])}))
            }
            "/queue" => {
                let words = arg.split_whitespace().collect::<Vec<_>>();
                match words.as_slice() {
                    [] => {
                        let queued = Box::pin(self.handle("listQueue", p)).await?;
                        Ok(json!({"message":serde_json::to_string_pretty(&queued)?}))
                    }
                    ["cancel", id] => {
                        let mut params = p.clone();
                        params["id"] = json!(id);
                        let deleted = Box::pin(self.handle("deleteQueuedPrompt", &params)).await?;
                        Ok(json!({"message":format!("queue_deleted: {deleted}")}))
                    }
                    ["resume"] => Box::pin(self.dispatch(c, "resumeQueue", p)).await,
                    _ => bail!("用法：/queue [resume | cancel ID]"),
                }
            }
            "/help" | "/?" => {
                let commands: Vec<Value> =
                    serde_json::from_str(include_str!("assets/commands.json"))?;
                Ok(
                    json!({"message":format!("可用命令：\n{}",commands.iter().map(|c|format!("{}  {}",c["usage"].as_str().unwrap_or_default(),c["summary"].as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n"))}),
                )
            }
            "/exit" | "/quit" => Ok(json!({"action":"exit"})),
            "/cls" => Ok(json!({"action":"clear","message":"已清屏，会话上下文保留。"})),
            "/new" | "/clear" => {
                c.runtime = None;
                let mut snapshot = c.snapshot.lock().unwrap();
                let branch = snapshot.branch.clone();
                *snapshot = Snapshot {
                    branch,
                    ..Snapshot::default()
                };
                drop(snapshot);
                self.reset_settings(c);
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
                            util::short_id(s["session_id"].as_str().unwrap_or_default()),
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
                    let listing = self
                        .store
                        .sessions()?
                        .iter()
                        .take(20)
                        .map(|row| {
                            format!(
                                "{} · {}",
                                row["session_id"].as_str().unwrap_or_default(),
                                row["title"].as_str().unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(
                        json!({"action":"sessions","message":if listing.is_empty(){"暂无会话。".into()}else{listing}}),
                    );
                }
                Ok(json!({"action":"resume","sessionId":self.store.resolve_session(arg)?}))
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
                let (active, name) = crate::commands::skill_argument(arg)?
                    .context("用法：/skill [名称 | off 名称]")?;
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
                        .compact(&c.hooks(self.event_sink(&c.listeners)))
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
            let preparation = c.preparation.lock().unwrap().take();
            if let Some(handle) = preparation {
                let _ = handle.await;
            }
            let mut c = c.conversation.lock().await;
            if let Some(handle) = c.running.take() {
                let _ = handle.await;
            }
            if let Some(runtime) = &c.runtime {
                let hooks = c.hooks(self.event_sink(&c.listeners));
                let _ = runtime.lock().await.close_resources(&hooks).await;
            }
        }
    }
}
type BTreeActive = std::collections::BTreeMap<String, String>;

pub async fn serve(store: Store) -> Result<()> {
    let (sink, writer) = output::output_writer(std::io::stdout());
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
            let retained = n.min(4_000_000 - line.len());
            if retained < n {
                over = true;
            }
            line.extend_from_slice(&buf[..retained]);
            let complete = buf[n - 1] == b'\n';
            reader.consume(n);
            if complete {
                break;
            }
        }
        if line.is_empty() && !over {
            break;
        }
        let req = match protocol::parse_request_line(&line, over) {
            Ok(req) => req,
            Err(e) => {
                let id = if over {
                    protocol::request_id_prefix(&line)
                } else {
                    protocol::extract_valid_id(&line)
                };
                sink(if let Some(id) = id {
                    json!({"id":id,"error":e.to_string()})
                } else {
                    json!({"event":"bridge_error","error":e.to_string()})
                });
                continue;
            }
        };
        let serialized = req.id.key_string();
        let kind = protocol::classify_method(&req.method);
        let reserved = if kind == Some(protocol::MethodKind::Control) {
            8
        } else {
            0
        };
        let count = ids.lock().unwrap().len();
        if ids.lock().unwrap().contains(&serialized) {
            sink(json!({"event":"bridge_error","error":"duplicate in-flight request id"}));
            continue;
        }
        if count >= 64 + reserved {
            sink(protocol::RpcResponse::err(&req.id, "request queue is full"));
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
            let value = if kind == Some(protocol::MethodKind::Control) {
                router.handle(&req.method, &req.params).await
            } else {
                // SQLite, repository scanning and attachment import are synchronous.
                // Keep them off the workers that drive stdin and cancellations.
                let driver = tokio::runtime::Handle::current();
                tokio::task::spawn_blocking(move || {
                    driver.block_on(async {
                        if kind == Some(protocol::MethodKind::Network) {
                            network(&req.method, &req.params, &configuration).await
                        } else {
                            router.handle(&req.method, &req.params).await
                        }
                    })
                })
                .await
                .unwrap_or_else(|error| Err(anyhow::anyhow!("bridge request task failed: {error}")))
            };
            let reply = match value {
                Ok(value) => protocol::RpcResponse::ok(&req.id, value),
                Err(error) => protocol::RpcResponse::err(&req.id, &error.to_string()),
            };
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
    use std::time::Duration;

    #[tokio::test]
    async fn accepted_prompt_and_state_do_not_wait_for_preparation_lock() {
        let root = tempfile::tempdir().unwrap();
        let mut router = Router::new(Store::open(":memory:").unwrap(), Arc::new(|_| {}));
        router.configuration = Configuration {
            path: root.path().join("config.json"),
        };
        router.handle("getState", &json!({})).await.unwrap();
        let owner = router.routes.read().unwrap().clients["default"].clone();
        let lock = owner.conversation.lock().await;
        let request =
            json!({"asyncStart":true,"workspace":root.path(),"model":"fake","text":"accepted"});
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            router.handle("sendPrompt", &request),
        )
        .await
        .expect("acceptance waited for preparation")
        .unwrap();
        let state = tokio::time::timeout(
            Duration::from_millis(100),
            router.handle("getState", &json!({})),
        )
        .await
        .expect("state waited for preparation")
        .unwrap();
        assert_eq!(state["running"], true);
        assert_eq!(state["runId"], result["runId"]);
        assert_eq!(state["sessionId"], result["sessionId"]);
        drop(lock);
        router.close().await;
    }

    #[tokio::test]
    async fn ensure_checks_metadata_without_reading_message_bodies() {
        let root = tempfile::tempdir().unwrap();
        // CI's Windows temp path may contain RUNNER~1; dispatch passes a
        // canonical workspace, so exercise the same freshness-check contract.
        let workspace = util::canonical(root.path()).unwrap();
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
            .ensure(&mut c, &workspace, Some(&session), Some("fake"))
            .await
            .unwrap();
        // The metadata is unchanged; selecting a message body now fails at
        // the SQL boundary. A freshness check must not touch that column.
        store.with(|db| { db.execute_batch("ALTER TABLE messages RENAME TO saved_messages; CREATE VIEW messages AS SELECT session_id,seq,role,json('invalid JSON') AS content FROM saved_messages;")?; Ok(()) }).unwrap();
        router
            .ensure(&mut c, &workspace, Some(&session), Some("fake"))
            .await
            .unwrap();
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
