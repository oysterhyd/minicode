use crate::{
    models::{Block, Outcome, Response},
    provider::ProviderError,
    tools::Sink,
    util,
};
use anyhow::{Context as _, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Context {
        Context {
            session_id: "session-fixture".into(),
            run_id: "run-fixture".into(),
            workspace: "fixture-workspace".into(),
            ..Default::default()
        }
    }
    #[test]
    fn g5_error_replay_preserves_retry_and_overflow_types() {
        let errors = vec![
            ProviderError::Http {
                status: 429,
                body: "busy".into(),
                retry_after: Some(Duration::from_secs(2)),
            },
            ProviderError::ContextOverflow {
                status: Some(400),
                body: "context_length_exceeded".into(),
            },
            ProviderError::Stream {
                kind: "overloaded_error".into(),
                message: "busy".into(),
            },
            ProviderError::RecordedNetwork,
            ProviderError::Timeout {
                idle: Duration::from_millis(125),
            },
            ProviderError::Deadline,
            ProviderError::Cancelled,
            ProviderError::Interrupted,
            ProviderError::Protocol("invalid SSE JSON".into()),
        ];
        for error in errors {
            let retry = error.is_retryable();
            let delay = error.retry_after();
            let display = error.to_string();
            let restored = Failure::from_error(&error.into()).error();
            assert_eq!(restored.is_retryable(), retry);
            assert_eq!(restored.retry_after(), delay);
            assert_eq!(restored.to_string(), display);
        }
    }
    #[test]
    fn g5_truncated_tail_recovers_and_corrupt_committed_lines_fail() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("provider.jsonl");
        let trace = Options::recording(path.clone(), root.path().join("events.jsonl"))
            .open()
            .unwrap();
        let context = context();
        let call = trace
            .begin(
                &context,
                json!({"configuration_id":"fake"}),
                json!({"prompt":"same"}),
            )
            .unwrap();
        let start = call.entry.to_string();
        drop(call);
        fs::write(&path, format!("{start}\n{{\"partial\":")).unwrap();
        let replay = Options::replaying(path.clone(), root.path().join("replay.jsonl"))
            .open()
            .unwrap();
        let mut call = replay
            .begin(
                &context,
                json!({"configuration_id":"fake"}),
                json!({"prompt":"same"}),
            )
            .unwrap();
        let sink: Sink = Arc::new(|_| {});
        assert!(matches!(
            call.replay(&sink)
                .unwrap_err()
                .downcast_ref::<ProviderError>(),
            Some(ProviderError::Interrupted)
        ));
        fs::write(&path, "{broken}\n").unwrap();
        assert!(
            Options::replaying(path.clone(), root.path().join("replay.jsonl"))
                .open()
                .is_err()
        );
        fs::write(&path, "{\"schema_version\":999}\n").unwrap();
        assert!(
            Options::replaying(path, root.path().join("replay.jsonl"))
                .open()
                .is_err()
        );
    }
    #[test]
    fn g5_cancelled_and_deadline_futures_leave_replayable_records() {
        for cancelled in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("provider.jsonl");
            let trace = Options::recording(path.clone(), root.path().join("events.jsonl"))
                .open()
                .unwrap();
            let mut context = context();
            context.cancel = Some(CancellationToken::new());
            if cancelled {
                context.cancel.as_ref().unwrap().cancel();
            } else {
                context.deadline = Some(Instant::now());
            }
            drop(
                trace
                    .begin(
                        &context,
                        json!({"configuration_id":"fake"}),
                        json!({"prompt":"same"}),
                    )
                    .unwrap(),
            );
            let replay = Options::replaying(path, root.path().join("replay.jsonl"))
                .open()
                .unwrap();
            let mut call = replay
                .begin(
                    &context,
                    json!({"configuration_id":"fake"}),
                    json!({"prompt":"same"}),
                )
                .unwrap();
            let sink: Sink = Arc::new(|_| {});
            let error = call.replay(&sink).unwrap_err();
            assert!(if cancelled {
                matches!(
                    error.downcast_ref::<ProviderError>(),
                    Some(ProviderError::Cancelled)
                )
            } else {
                matches!(
                    error.downcast_ref::<ProviderError>(),
                    Some(ProviderError::Deadline)
                )
            });
        }
    }
    #[test]
    fn g5_concurrent_appends_keep_each_record_complete() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("provider.jsonl");
        let trace = Options::recording(path.clone(), root.path().join("events.jsonl"))
            .open()
            .unwrap();
        let workers = (0..8)
            .map(|i| {
                let trace = trace.clone();
                std::thread::spawn(move || {
                    let call = trace
                        .begin(
                            &context(),
                            json!({"configuration_id":"fake"}),
                            json!({"i":i}),
                        )
                        .unwrap();
                    call.finish(
                        &Ok((
                            Response {
                                blocks: vec![Block::text("done")],
                                stop_reason: "end_turn".into(),
                                usage: Default::default(),
                            },
                            json!({}),
                        )),
                        json!({"index":i}),
                    )
                    .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        let entries = fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str::<Value>(s).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 16);
        let calls = entries
            .iter()
            .filter(|e| e["kind"] == "provider_call")
            .map(|e| e["call_id"].as_str().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(calls.len(), 8);
    }
    #[test]
    fn g5_redaction_covers_values_nested_fields_and_split_deltas() {
        let trace = Trace::disabled();
        trace.add_secret("fixture-secret-value");
        let value = json!({"api_key":"private","password":"spaced private value","text":"password=\"quoted private value\" Bearer fixture-bearer-value fixture-secret-value"});
        let clean = trace.clean(&value, &context()).to_string();
        for secret in [
            "private value",
            "fixture-bearer-value",
            "fixture-secret-value",
        ] {
            assert!(!clean.contains(secret));
        }
        let mut partial = Partial::default();
        partial.capture(&json!({"event":"text_delta","text":"fixture-secret-"}));
        partial.capture(&json!({"event":"text_delta","text":"value"}));
        assert!(
            !trace
                .clean(&json!(partial.emitted()), &context())
                .to_string()
                .contains("fixture-secret-value")
        );
        assert!(
            trace
                .clean(&json!("日期：2026-10-31 fixture-workspace"), &context())
                .as_str()
                .unwrap()
                .contains("$DATE $WORKSPACE")
        );
    }
}

const LIMIT: u64 = 128 * 1024 * 1024;
const PARTIAL_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct Options {
    pub record: Option<PathBuf>,
    pub replay: Option<PathBuf>,
    pub log: Option<PathBuf>,
    cached: Arc<Mutex<Option<Arc<Trace>>>>,
}
impl Options {
    pub fn from_env() -> Self {
        let path = |name| {
            std::env::var_os(name)
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
        };
        Self {
            record: path("MINICODE_PROVIDER_RECORD"),
            replay: path("MINICODE_PROVIDER_REPLAY"),
            log: path("MINICODE_LOG_FILE"),
            ..Default::default()
        }
    }
    pub fn recording(record: PathBuf, log: PathBuf) -> Self {
        Self {
            record: Some(record),
            log: Some(log),
            ..Default::default()
        }
    }
    pub fn replaying(replay: PathBuf, log: PathBuf) -> Self {
        Self {
            replay: Some(replay),
            log: Some(log),
            ..Default::default()
        }
    }
    pub fn open(&self) -> Result<Arc<Trace>> {
        let mut cached = self.cached.lock().unwrap();
        if let Some(trace) = &*cached {
            return Ok(trace.clone());
        }
        if self.record.is_some() && self.replay.is_some() {
            bail!("provider recording and replay are mutually exclusive");
        }
        if self
            .log
            .as_ref()
            .is_some_and(|p| Some(p) == self.record.as_ref() || Some(p) == self.replay.as_ref())
        {
            bail!("structured log and provider recording must use different files");
        }
        let mut entries = vec![];
        if let Some(path) = &self.replay {
            if fs::metadata(path)?.len() > LIMIT {
                bail!("provider recording exceeds replay limit");
            }
            let mut source = String::new();
            fs::File::open(path)?
                .take(LIMIT + 1)
                .read_to_string(&mut source)?;
            if source.len() as u64 > LIMIT {
                bail!("provider recording exceeds replay limit");
            }
            for line in source
                .split_inclusive('\n')
                .filter(|l| !l.trim().is_empty())
            {
                let entry: Value = match serde_json::from_str(line) {
                    Ok(entry) => entry,
                    Err(_) if !line.ends_with('\n') => break,
                    Err(error) => return Err(error).context("invalid provider recording"),
                };
                if entry["schema_version"] != 1 {
                    bail!("unsupported provider recording version");
                }
                entries.push(entry);
            }
            // An abrupt process exit may leave a durable start with no finish.
            let finished = entries
                .iter()
                .filter(|e| e["kind"] == "provider_call")
                .filter_map(|e| e["call_id"].as_str().map(str::to_string))
                .collect::<HashSet<_>>();
            let interrupted = entries
                .iter()
                .filter(|e| {
                    e["kind"] == "provider_start"
                        && !e["call_id"]
                            .as_str()
                            .is_some_and(|id| finished.contains(id))
                })
                .cloned()
                .map(|mut e| {
                    e["kind"] = json!("provider_call");
                    e["failure"] = json!(Failure::from_error(&ProviderError::Interrupted.into()));
                    e["emitted"] = json!([]);
                    e
                })
                .collect::<Vec<_>>();
            entries.extend(interrupted);
        }
        let secrets = std::env::vars()
            .filter(|(name, value)| {
                let name = name.to_ascii_uppercase();
                value.len() >= 4
                    && ["API_KEY", "TOKEN", "PASSWORD", "SECRET", "CREDENTIAL"]
                        .iter()
                        .any(|key| name.contains(key))
                    && ![
                        "MINICODE_PROVIDER_RECORD",
                        "MINICODE_PROVIDER_REPLAY",
                        "MINICODE_LOG_FILE",
                    ]
                    .contains(&name.as_str())
            })
            .map(|(_, value)| value)
            .collect();
        let trace = Arc::new(Trace {
            record: self.record.clone(),
            log: self.log.clone(),
            replay: self.replay.is_some(),
            state: Mutex::new(Replay {
                entries,
                used: HashSet::new(),
                sessions: HashMap::new(),
                selected_run: None,
            }),
            secrets: Mutex::new(secrets),
        });
        *cached = Some(trace.clone());
        Ok(trace)
    }
}

#[derive(Clone)]
pub struct Context {
    pub session_id: String,
    pub run_id: String,
    pub workspace: String,
    pub purpose: String,
    pub cancel: Option<CancellationToken>,
    pub deadline: Option<Instant>,
}
impl Default for Context {
    fn default() -> Self {
        Self {
            session_id: "standalone".into(),
            run_id: util::id(),
            workspace: String::new(),
            purpose: "task".into(),
            cancel: None,
            deadline: None,
        }
    }
}
struct Replay {
    entries: Vec<Value>,
    used: HashSet<usize>,
    sessions: HashMap<String, String>,
    selected_run: Option<String>,
}
pub struct Trace {
    record: Option<PathBuf>,
    log: Option<PathBuf>,
    replay: bool,
    state: Mutex<Replay>,
    secrets: Mutex<Vec<String>>,
}
impl Trace {
    pub fn active(&self) -> bool {
        self.record.is_some() || self.log.is_some() || self.replay
    }
    pub fn disabled() -> Arc<Self> {
        Arc::new(Self {
            record: None,
            log: None,
            replay: false,
            state: Mutex::new(Replay {
                entries: vec![],
                used: HashSet::new(),
                sessions: HashMap::new(),
                selected_run: None,
            }),
            secrets: Mutex::new(vec![]),
        })
    }
    pub fn is_replay(&self) -> bool {
        self.replay
    }
    pub fn add_secret(&self, secret: &str) {
        if !secret.is_empty() {
            let mut secrets = self.secrets.lock().unwrap();
            if !secrets.iter().any(|s| s == secret) {
                secrets.push(secret.into());
            }
        }
    }
    pub fn metadata(&self, model: &str) -> Result<Option<Value>> {
        if !self.replay {
            return Ok(None);
        }
        let state = self.state.lock().unwrap();
        state
            .entries
            .iter()
            .find(|e| {
                e["kind"] == "provider_call"
                    && e["provider"]["configuration_id"] == model
                    && state
                        .selected_run
                        .as_ref()
                        .is_none_or(|id| e["run_id"] == id.as_str())
            })
            .map(|e| Some(e["provider"].clone()))
            .context("provider replay has no matching model")
    }
    pub fn select_run(&self, run_id: Option<&str>) -> Result<(Value, Value)> {
        let mut state = self.state.lock().unwrap();
        let start = state
            .entries
            .iter()
            .rev()
            .find(|e| {
                e["kind"] == "run_start"
                    && e["data"]["child"] != true
                    && run_id.is_none_or(|id| e["run_id"] == id)
            })
            .cloned()
            .context("recording has no matching root run")?;
        let request = state
            .entries
            .iter()
            .find(|e| {
                e["kind"] == "provider_call"
                    && e["session_id"] == start["session_id"]
                    && e["run_id"] == start["run_id"]
                    && e["purpose"] == "task"
            })
            .cloned()
            .context("recording has no provider request for this run")?;
        let excluded = state
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e["run_id"] != start["run_id"])
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        state.used.extend(excluded);
        state.selected_run = start["run_id"].as_str().map(str::to_string);
        Ok((start, request))
    }
    pub fn boundary(&self, context: &Context, kind: &str, data: Value) -> Result<()> {
        if let Some(path) = &self.record {
            self.append(path,&json!({"schema_version":1,"kind":kind,"timestamp":util::now(),"session_id":context.session_id,"run_id":context.run_id,"data":data}),context,true)?;
        }
        self.log(context, kind, None, data);
        Ok(())
    }
    pub fn tool_specs(&self, model: &str, session: &str) -> Option<Vec<crate::models::ToolSpec>> {
        if !self.replay {
            return None;
        }
        let state = self.state.lock().unwrap();
        state
            .entries
            .iter()
            .find(|e| {
                e["kind"] == "provider_call"
                    && e["purpose"] == "task"
                    && e["provider"]["configuration_id"] == model
                    && state
                        .selected_run
                        .as_ref()
                        .is_none_or(|id| e["run_id"] == id.as_str())
                    && state
                        .sessions
                        .get(session)
                        .is_none_or(|id| e["session_id"] == id.as_str())
            })
            .and_then(|e| serde_json::from_value(e["request"]["tools"].clone()).ok())
    }
    pub fn goal(&self, context: &Context, report: Option<Value>) -> Result<Value> {
        if self.replay {
            let mut state = self.state.lock().unwrap();
            let session = state
                .sessions
                .get(&context.session_id)
                .context("provider replay has no session binding")?;
            let index = state
                .entries
                .iter()
                .enumerate()
                .find(|(i, e)| {
                    !state.used.contains(i)
                        && e["kind"] == "goal_check"
                        && e["session_id"] == session.as_str()
                })
                .map(|(i, _)| i)
                .context("provider replay goal check does not match recording")?;
            let report = state.entries[index]["report"].clone();
            state.used.insert(index);
            return Ok(report);
        }
        let report = report.context("missing goal check report")?;
        if let Some(path) = &self.record {
            self.append(path,&json!({"schema_version":1,"kind":"goal_check","session_id":context.session_id,"run_id":context.run_id,"report":report}),context,true)?;
        }
        Ok(report)
    }
    fn clean(&self, value: &Value, context: &Context) -> Value {
        static ASSIGNMENT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        static BEARER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        static DATE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let assignment=ASSIGNMENT.get_or_init(||regex::Regex::new(r#"(?i)(\b(?:api[_-]?key|password|secret|(?:access[_-]?|refresh[_-]?)?token|authorization)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,}\r\n]+)"#).unwrap());
        let bearer =
            BEARER.get_or_init(|| regex::Regex::new(r#"(?i)\bBearer\s+[^\s"']+"#).unwrap());
        let date = DATE.get_or_init(|| regex::Regex::new(r"日期：\d{4}-\d{2}-\d{2}").unwrap());
        fn visit(
            trace: &Trace,
            v: &Value,
            c: &Context,
            a: &regex::Regex,
            b: &regex::Regex,
            d: &regex::Regex,
        ) -> Value {
            match v {
                Value::Object(map) => Value::Object(
                    map.iter()
                        .map(|(key, value)| {
                            let key_lower = key.to_ascii_lowercase().replace(['-', '_'], "");
                            let value = if [
                                "apikey",
                                "authorization",
                                "password",
                                "secret",
                                "accesstoken",
                                "refreshtoken",
                            ]
                            .contains(&key_lower.as_str())
                            {
                                json!("[REDACTED]")
                            } else {
                                visit(trace, value, c, a, b, d)
                            };
                            (key.clone(), value)
                        })
                        .collect(),
                ),
                Value::Array(values) => json!(
                    values
                        .iter()
                        .map(|v| visit(trace, v, c, a, b, d))
                        .collect::<Vec<_>>()
                ),
                Value::String(text) => {
                    let mut text = text.clone();
                    let mut secrets = trace.secrets.lock().unwrap().clone();
                    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
                    for secret in secrets {
                        text = text.replace(&secret, "[REDACTED]");
                    }
                    text = a.replace_all(&text, "$1[REDACTED]").into_owned();
                    text = b.replace_all(&text, "Bearer [REDACTED]").into_owned();
                    if !c.workspace.is_empty() {
                        text = text.replace(&c.workspace, "$WORKSPACE");
                    }
                    text = d.replace_all(&text, "日期：$$DATE").into_owned();
                    json!(text)
                }
                _ => v.clone(),
            }
        }
        visit(self, value, context, assignment, bearer, date)
    }
    fn append(&self, path: &Path, entry: &Value, context: &Context, sync: bool) -> Result<()> {
        let entry = self.clean(entry, context);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        if util::is_link(path) {
            bail!("trace output cannot be a link");
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.lock_exclusive()?;
        let bytes = serde_json::to_vec(&entry)?;
        if file
            .metadata()?
            .len()
            .saturating_add(bytes.len() as u64 + 1)
            > LIMIT
        {
            bail!("trace file exceeds 128 MiB limit");
        }
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        if sync {
            file.sync_data()?;
        }
        Ok(())
    }
    pub fn log(&self, context: &Context, kind: &str, call_id: Option<&str>, data: Value) {
        if let Some(path) = &self.log {
            let entry = json!({"schema_version":1,"timestamp":util::now(),"level":if kind=="provider_error" || kind=="run_end" && data["exit_reason"]!="completed" {"error"} else {"info"},"kind":kind,"session_id":context.session_id,"run_id":context.run_id,"call_id":call_id,"data":data});
            if self.append(path, &entry, context, false).is_err() {
                eprintln!("structured log write failed");
            }
        }
    }
    pub fn begin(
        self: &Arc<Self>,
        context: &Context,
        provider: Value,
        request: Value,
    ) -> Result<Call> {
        let request = self.clean(&request, context);
        let fingerprint = util::hash(&serde_json::to_vec(&request)?);
        let entry = json!({"schema_version":1,"kind":"provider_start","timestamp":util::now(),"session_id":context.session_id,"run_id":context.run_id,"call_id":util::id(),"purpose":context.purpose,"provider":provider,"request":request,"request_sha256":fingerprint});
        if let Some(path) = &self.record {
            self.append(path, &entry, context, true)?;
        }
        self.log(
            context,
            "provider_start",
            entry["call_id"].as_str(),
            json!({"model":entry["provider"]["configuration_id"],"request_sha256":fingerprint}),
        );
        Ok(Call {
            trace: self.clone(),
            context: context.clone(),
            entry,
            partial: Default::default(),
            done: false,
        })
    }
    pub fn replay_tool(&self, context: &Context, call: &Block) -> Result<Option<(Outcome, Value)>> {
        if !self.replay {
            return Ok(None);
        }
        let call = self.clean(&serde_json::to_value(call)?, context);
        let mut state = self.state.lock().unwrap();
        let session = state
            .sessions
            .get(&context.session_id)
            .cloned()
            .context("provider replay has no session binding")?;
        let index = state
            .entries
            .iter()
            .enumerate()
            .find(|(i, e)| {
                !state.used.contains(i)
                    && e["kind"] == "tool_result"
                    && e["session_id"] == session
                    && e["tool_call"] == call
            })
            .map(|(i, _)| i)
            .context("provider replay tool request does not match recording")?;
        let outcome = serde_json::from_value(state.entries[index]["outcome"].clone())?;
        let ledger = state.entries[index]["ledger"].clone();
        state.used.insert(index);
        Ok(Some((outcome, ledger)))
    }
    pub fn record_tool(
        &self,
        context: &Context,
        call: &Block,
        outcome: &Outcome,
        ledger: Value,
    ) -> Result<()> {
        if let Some(path) = &self.record {
            self.append(path,&json!({"schema_version":1,"kind":"tool_result","timestamp":util::now(),"session_id":context.session_id,"run_id":context.run_id,"call_id":match call {Block::ToolUse{id,..}=>id.as_str(),_=>""},"tool_call":call,"outcome":outcome,"ledger":ledger}),context,true)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct Partial {
    text: String,
    reasoning: String,
    usage: Option<Value>,
    exceeded: bool,
}
impl Partial {
    fn capture(&mut self, value: &Value) {
        let target = match value["event"].as_str() {
            Some("text_delta") => Some(&mut self.text),
            Some("reasoning_delta") => Some(&mut self.reasoning),
            Some("usage_delta") => {
                self.usage = Some(value.clone());
                None
            }
            _ => None,
        };
        if let Some(target) = target {
            let text = value["text"].as_str().unwrap_or_default();
            if target.len().saturating_add(text.len()) <= PARTIAL_LIMIT {
                target.push_str(text);
            } else {
                self.exceeded = true;
            }
        }
    }
    fn emitted(&self) -> Vec<Value> {
        let mut values = vec![];
        if !self.reasoning.is_empty() {
            values.push(json!({"event":"reasoning_delta","text":self.reasoning}));
        }
        if !self.text.is_empty() {
            values.push(json!({"event":"text_delta","text":self.text}));
        }
        if let Some(usage) = &self.usage {
            values.push(usage.clone());
        }
        values
    }
}

pub struct Call {
    trace: Arc<Trace>,
    context: Context,
    entry: Value,
    partial: Arc<Mutex<Partial>>,
    done: bool,
}
impl Call {
    pub fn sink(&self, emit: &Sink) -> Sink {
        let partial = self.partial.clone();
        let emit = emit.clone();
        Arc::new(move |value| {
            partial.lock().unwrap().capture(&value);
            emit(value);
        })
    }
    pub fn progress(&self) -> &Value {
        &self.entry["progress"]
    }
    pub fn replay(&mut self, emit: &Sink) -> Result<Option<(Response, Value)>> {
        if !self.trace.replay {
            return Ok(None);
        }
        self.done = true;
        let mut state = self.trace.state.lock().unwrap();
        let bound = state.sessions.get(&self.context.session_id);
        let index = state
            .entries
            .iter()
            .enumerate()
            .find(|(i, e)| {
                !state.used.contains(i)
                    && e["kind"] == "provider_call"
                    && e["purpose"] == self.entry["purpose"]
                    && e["provider"]["configuration_id"]
                        == self.entry["provider"]["configuration_id"]
                    && e["request_sha256"] == self.entry["request_sha256"]
                    && bound.is_none_or(|session| e["session_id"] == session.as_str())
            })
            .map(|(i, _)| i)
            .context("provider replay request does not match recording")?;
        let entry = state.entries[index].clone();
        self.entry["progress"] = entry["progress"].clone();
        state.used.insert(index);
        state.sessions.insert(
            self.context.session_id.clone(),
            entry["session_id"]
                .as_str()
                .context("recording missing session")?
                .into(),
        );
        drop(state);
        for value in entry["emitted"]
            .as_array()
            .context("recording missing emitted events")?
        {
            emit(value.clone());
        }
        if !entry["failure"].is_null() {
            let failure: Failure = serde_json::from_value(entry["failure"].clone())?;
            if failure.kind == "cancelled"
                && let Some(cancel) = &self.context.cancel
            {
                cancel.cancel();
            }
            return Err(failure.error().into());
        }
        Ok(Some((
            serde_json::from_value(entry["response"].clone())?,
            entry["metrics"].clone(),
        )))
    }
    pub fn finish(mut self, result: &Result<(Response, Value)>, progress: Value) -> Result<()> {
        self.entry["kind"] = json!("provider_call");
        self.entry["emitted"] = json!(self.partial.lock().unwrap().emitted());
        self.entry["progress"] = progress;
        match result {
            Ok((response, metrics)) => {
                self.entry["response"] = serde_json::to_value(response)?;
                self.entry["metrics"] = metrics.clone();
            }
            Err(error) => self.entry["failure"] = json!(Failure::from_error(error)),
        }
        if self.partial.lock().unwrap().exceeded {
            bail!("provider recording exceeds partial output limit");
        }
        self.write()?;
        self.done = true;
        Ok(())
    }
    pub fn finish_panic(mut self, message: &str, progress: Value) -> Result<()> {
        self.entry["kind"] = json!("provider_call");
        self.entry["failure"] = json!(Failure {
            kind: "panic".into(),
            message: message.into(),
            status: None,
            retry_after_ms: None,
            idle_ms: None
        });
        self.entry["progress"] = progress;
        self.entry["emitted"] = json!(
            self.partial
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .emitted()
        );
        self.write()?;
        self.done = true;
        Ok(())
    }
    fn write(&self) -> Result<()> {
        if let Some(path) = &self.trace.record {
            self.trace.append(path, &self.entry, &self.context, true)?;
        }
        self.trace.log(
            &self.context,
            if self.entry["failure"].is_null() {
                "provider_end"
            } else {
                "provider_error"
            },
            self.entry["call_id"].as_str(),
            json!({"failure":self.entry["failure"],"usage":self.entry["response"]["usage"]}),
        );
        Ok(())
    }
}
impl Drop for Call {
    fn drop(&mut self) {
        if self.done || self.trace.replay {
            return;
        }
        let error = if self
            .context
            .cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            ProviderError::Cancelled
        } else if self.context.deadline.is_some_and(|d| Instant::now() >= d) {
            ProviderError::Deadline
        } else {
            ProviderError::Interrupted
        };
        self.entry["kind"] = json!("provider_call");
        self.entry["failure"] = json!(Failure::from_error(&error.into()));
        self.entry["emitted"] = json!(
            self.partial
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .emitted()
        );
        if self.write().is_err() {
            eprintln!("provider recording write failed");
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Failure {
    kind: String,
    message: String,
    status: Option<u16>,
    retry_after_ms: Option<u64>,
    idle_ms: Option<u64>,
}
impl Failure {
    fn from_error(error: &anyhow::Error) -> Self {
        let mut failure = Self {
            kind: "protocol".into(),
            message: error.to_string(),
            status: None,
            retry_after_ms: None,
            idle_ms: None,
        };
        match error.downcast_ref::<ProviderError>() {
            Some(ProviderError::Http {
                status,
                body,
                retry_after,
            }) => {
                failure.kind = "http".into();
                failure.status = Some(*status);
                failure.message = body.clone();
                failure.retry_after_ms =
                    retry_after.map(|d| d.as_millis().min(u64::MAX as u128) as u64);
            }
            Some(ProviderError::ContextOverflow { status, body }) => {
                failure.kind = "overflow".into();
                failure.status = *status;
                failure.message = body.clone();
            }
            Some(ProviderError::Stream { kind, message }) => {
                failure.kind = format!("stream:{kind}");
                failure.message = message.clone();
            }
            Some(ProviderError::Network(_) | ProviderError::RecordedNetwork) => {
                failure.kind = "network".into()
            }
            Some(ProviderError::Timeout { idle }) => {
                failure.kind = "timeout".into();
                failure.idle_ms = Some(idle.as_millis().min(u64::MAX as u128) as u64);
            }
            Some(ProviderError::Deadline) => failure.kind = "deadline".into(),
            Some(ProviderError::Cancelled) => failure.kind = "cancelled".into(),
            Some(ProviderError::Interrupted) => failure.kind = "interrupted".into(),
            _ => {}
        }
        failure
    }
    fn error(self) -> ProviderError {
        match self.kind.as_str() {
            "http" => ProviderError::Http {
                status: self.status.unwrap_or(500),
                body: self.message,
                retry_after: self.retry_after_ms.map(Duration::from_millis),
            },
            "overflow" => ProviderError::ContextOverflow {
                status: self.status,
                body: self.message,
            },
            "network" => ProviderError::RecordedNetwork,
            "timeout" => ProviderError::Timeout {
                idle: Duration::from_millis(self.idle_ms.unwrap_or(0)),
            },
            "deadline" => ProviderError::Deadline,
            "cancelled" => ProviderError::Cancelled,
            "interrupted" => ProviderError::Interrupted,
            "panic" => std::panic::panic_any(self.message),
            kind if kind.starts_with("stream:") => ProviderError::Stream {
                kind: kind[7..].into(),
                message: self.message,
            },
            _ => ProviderError::Protocol(self.message),
        }
    }
}
