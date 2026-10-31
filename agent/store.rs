use crate::{
    models::{Block, Event, Message, Usage},
    util,
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Store {
    pub path: PathBuf,
    connection: Arc<Mutex<Connection>>,
    writer: Arc<crate::store_writer::Writer>,
    pages: Arc<crate::paging::TextPages>,
}

pub struct SessionLock(File);

impl Drop for SessionLock {
    fn drop(&mut self) {
        // A forked child may still hold the same open file description until
        // exec. Unlock explicitly so closing our handle releases ownership now.
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
const COLUMNS: &str = "session_id,created_at,workspace,provider,model,status,exit_reason,rounds,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,usage_available";
fn summary(row: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"session_id":row.get::<_,String>(0)?,"created_at":row.get::<_,String>(1)?,
        "workspace":row.get::<_,String>(2)?,"provider":row.get::<_,String>(3)?,"model":row.get::<_,String>(4)?,
        "status":row.get::<_,String>(5)?,"exit_reason":row.get::<_,Option<String>>(6)?,"rounds":row.get::<_,i64>(7)?,
        "input_tokens":row.get::<_,i64>(8)?,"output_tokens":row.get::<_,i64>(9)?,"cache_read_tokens":row.get::<_,i64>(10)?,
        "cache_write_tokens":row.get::<_,i64>(11)?,"usage_available":row.get::<_,bool>(12)?}),
    )
}
impl Store {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if path != Path::new(":memory:") {
            fs::create_dir_all(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
        }
        let database = if path == Path::new(":memory:") {
            format!("file:minicode_{}?mode=memory&cache=shared", util::id())
        } else {
            path.to_string_lossy().into_owned()
        };
        let mut c = Connection::open(&database)?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 7 {
            bail!("minicode database was created by a newer version")
        }
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS sessions (session_id TEXT PRIMARY KEY,created_at TEXT NOT NULL,workspace TEXT NOT NULL,provider TEXT NOT NULL,model TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'running',exit_reason TEXT,rounds INTEGER NOT NULL DEFAULT 0,input_tokens INTEGER NOT NULL DEFAULT 0,output_tokens INTEGER NOT NULL DEFAULT 0,cache_read_tokens INTEGER NOT NULL DEFAULT 0,cache_write_tokens INTEGER NOT NULL DEFAULT 0,usage_available INTEGER NOT NULL DEFAULT 1);
            CREATE TABLE IF NOT EXISTS messages(session_id TEXT NOT NULL,seq INTEGER NOT NULL,role TEXT NOT NULL,content TEXT NOT NULL,PRIMARY KEY(session_id,seq));
            CREATE TABLE IF NOT EXISTS events(session_id TEXT NOT NULL,seq INTEGER NOT NULL,type TEXT NOT NULL,timestamp TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(session_id,seq));
            CREATE TABLE IF NOT EXISTS artifacts(session_id TEXT NOT NULL,artifact_id TEXT NOT NULL,kind TEXT NOT NULL,relpath TEXT NOT NULL,created_at TEXT NOT NULL,PRIMARY KEY(session_id,artifact_id));
            CREATE TABLE IF NOT EXISTS artifact_access(session_id TEXT NOT NULL,artifact_id TEXT NOT NULL,accessed_at TEXT NOT NULL,PRIMARY KEY(session_id,artifact_id));
            CREATE TABLE IF NOT EXISTS session_goals(session_id TEXT PRIMARY KEY,state TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS session_meta(session_id TEXT PRIMARY KEY,title TEXT,pinned INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS session_prompts(session_id TEXT PRIMARY KEY,system TEXT NOT NULL,sources TEXT NOT NULL,dynamic_hash TEXT);
            CREATE TABLE IF NOT EXISTS tool_execution(session_id TEXT NOT NULL,call_id TEXT NOT NULL,started INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(session_id,call_id));
            CREATE TABLE IF NOT EXISTS tool_results(session_id TEXT NOT NULL,call_id TEXT NOT NULL,result TEXT NOT NULL,PRIMARY KEY(session_id,call_id));
            CREATE TABLE IF NOT EXISTS tasks(session_id TEXT NOT NULL,task_id TEXT NOT NULL,title TEXT NOT NULL,depends_on TEXT NOT NULL,status TEXT NOT NULL,owner TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(session_id,task_id));
            CREATE TABLE IF NOT EXISTS project_memory(id TEXT PRIMARY KEY,workspace TEXT NOT NULL,scope TEXT NOT NULL,fact TEXT NOT NULL,source TEXT NOT NULL,updated_at TEXT NOT NULL);")?;
        let columns: Vec<String> = c
            .prepare("PRAGMA table_info(sessions)")?
            .query_map([], |r| r.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        for (name, default) in [
            ("cache_read_tokens", 0),
            ("cache_write_tokens", 0),
            ("usage_available", 0),
        ] {
            if !columns.iter().any(|x| x == name) {
                c.execute_batch(&format!(
                    "ALTER TABLE sessions ADD COLUMN {name} INTEGER NOT NULL DEFAULT {default}"
                ))?;
            }
        }
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !columns.iter().any(|name| name == "auto_title") {
            tx.execute_batch("ALTER TABLE sessions ADD COLUMN auto_title TEXT")?;
            let rows = tx.prepare("SELECT session_id,content FROM messages m WHERE role='user' AND seq=(SELECT MIN(seq) FROM messages WHERE session_id=m.session_id AND role='user')")?
                .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, content) in rows {
                let message = Message {
                    role: "user".into(),
                    content: serde_json::from_str(&content).unwrap_or_default(),
                };
                cache_title(&tx, &id, &message)?;
            }
        }
        tx.execute_batch("CREATE INDEX IF NOT EXISTS events_activity ON events(session_id,timestamp);
            CREATE INDEX IF NOT EXISTS messages_first_user ON messages(session_id,seq) WHERE role='user';
            CREATE INDEX IF NOT EXISTS sessions_created ON sessions(created_at);
            CREATE TABLE IF NOT EXISTS message_revisions(session_id TEXT PRIMARY KEY,version INTEGER NOT NULL);
            CREATE TRIGGER IF NOT EXISTS messages_revision_insert AFTER INSERT ON messages BEGIN
                INSERT INTO message_revisions VALUES(NEW.session_id,1) ON CONFLICT(session_id) DO UPDATE SET version=version+1; END;
            CREATE TRIGGER IF NOT EXISTS messages_revision_update AFTER UPDATE ON messages BEGIN
                INSERT INTO message_revisions VALUES(OLD.session_id,1) ON CONFLICT(session_id) DO UPDATE SET version=version+1;
                INSERT INTO message_revisions SELECT NEW.session_id,1 WHERE NEW.session_id<>OLD.session_id ON CONFLICT(session_id) DO UPDATE SET version=version+1; END;
            CREATE TRIGGER IF NOT EXISTS messages_revision_delete AFTER DELETE ON messages BEGIN
                INSERT INTO message_revisions VALUES(OLD.session_id,1) ON CONFLICT(session_id) DO UPDATE SET version=version+1; END;
            PRAGMA user_version=7;")?;
        tx.commit()?;
        let reader = Connection::open(&database)?;
        reader.busy_timeout(std::time::Duration::from_secs(5))?;
        let store = Self {
            path,
            connection: Arc::new(Mutex::new(reader)),
            writer: Arc::new(crate::store_writer::Writer::new(c)?),
            pages: Arc::new(crate::paging::TextPages::default()),
        };
        store.prune_output_artifacts(None)?;
        Ok(store)
    }
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.writer.flush()?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
        f(&connection)
    }
    pub fn transaction<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.writer.call(true, f)
    }
    pub fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.transaction(f)
    }
    pub fn create(&self, workspace: &str, provider: &str, model: &str) -> Result<String> {
        let id = util::id();
        let (record_id, workspace, provider, model) = (
            id.clone(),
            workspace.to_owned(),
            provider.to_owned(),
            model.to_owned(),
        );
        self.write(move |c|{c.execute("INSERT INTO sessions(session_id,created_at,workspace,provider,model,usage_available) VALUES(?,?,?,?,?,0)",params![record_id,util::now(),workspace,provider,model])?;Ok(())})?;
        Ok(id)
    }
    pub fn session(&self, id: &str) -> Result<Option<Value>> {
        self.with(|c| {
            Ok(c.query_row(
                &format!("SELECT {COLUMNS} FROM sessions WHERE session_id=?"),
                [id],
                summary,
            )
            .optional()?)
        })
    }
    pub fn require(&self, id: &str) -> Result<Value> {
        self.session(id)?.context("找不到会话")
    }
    pub fn frozen_prompt(&self, id: &str) -> Result<Option<(String, Value, Option<String>)>> {
        self.with(|c| {
            let row: Option<(String, String, Option<String>)> = c
                .query_row(
                    "SELECT system,sources,dynamic_hash FROM session_prompts WHERE session_id=?",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            row.map(|(system, sources, hash)| Ok((system, serde_json::from_str(&sources)?, hash)))
                .transpose()
        })
    }
    pub fn freeze_prompt(
        &self,
        id: &str,
        system: &str,
        sources: &Value,
    ) -> Result<(String, Value, Option<String>)> {
        let (id, system, sources) = (id.to_owned(), system.to_owned(), sources.clone());
        self.transaction(move |c| {
            let (id,system,sources) = (id.as_str(),system.as_str(),&sources);
            c.execute("INSERT INTO session_prompts(session_id,system,sources) VALUES(?,?,?) ON CONFLICT(session_id) DO NOTHING", params![id,system,serde_json::to_string(sources)?])?;
            let (system, sources, hash): (String, String, Option<String>) = c.query_row(
                "SELECT system,sources,dynamic_hash FROM session_prompts WHERE session_id=?", [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            Ok((system, serde_json::from_str(&sources)?, hash))
        })
    }
    pub fn append_dynamic_context(&self, id: &str, hash: &str, message: &Message) -> Result<()> {
        let (id, hash, message) = (id.to_owned(), hash.to_owned(), message.clone());
        self.transaction(move |c| {
            let (id, hash, message) = (id.as_str(), hash.as_str(), &message);
            if c.execute(
                "UPDATE session_prompts SET dynamic_hash=? WHERE session_id=?",
                params![hash, id],
            )? != 1
            {
                bail!("session prompt is not frozen")
            }
            append_message(c, id, message)
        })
    }
    pub fn resolve_session(&self, prefix: &str) -> Result<String> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            bail!("请提供非空会话 ID 或唯一前缀");
        }
        let sessions = self.sessions()?;
        if let Some(exact) = sessions
            .iter()
            .find(|session| session["session_id"] == prefix)
        {
            return Ok(util::string(exact, "session_id")?.into());
        }
        let matches = sessions
            .iter()
            .filter_map(|session| session["session_id"].as_str())
            .filter(|id| id.starts_with(prefix))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [id] => Ok((*id).into()),
            [] => bail!("会话不存在"),
            _ => bail!("会话 ID 前缀不唯一，请输入更多字符"),
        }
    }
    pub fn sessions(&self) -> Result<Vec<Value>> {
        self.sessions_page(0, None)
    }
    pub fn sessions_page(&self, offset: usize, limit: Option<usize>) -> Result<Vec<Value>> {
        self.with(|c| {
            let columns = COLUMNS.split(',').map(|name|format!("s.{name}")).collect::<Vec<_>>().join(",");
            let mut q = c.prepare(&format!("SELECT {columns},COALESCE(m.title,s.auto_title,'新建任务'),m.title IS NOT NULL,COALESCE(m.pinned,0),COALESCE((SELECT MAX(timestamp) FROM events e WHERE e.session_id=s.session_id),s.created_at) FROM sessions s LEFT JOIN session_meta m ON m.session_id=s.session_id ORDER BY s.created_at DESC,s.session_id LIMIT ? OFFSET ?"))?;
            Ok(q.query_map(params![limit.map(|n|n.min(i64::MAX as usize) as i64).unwrap_or(-1),offset.min(i64::MAX as usize) as i64], |row| {
                let mut value = summary(row)?;
                value["title"] = json!(row.get::<_,String>(13)?);
                value["custom_title"] = json!(row.get::<_,bool>(14)?);
                value["pinned"] = json!(row.get::<_,bool>(15)?);
                value["updated_at"] = json!(row.get::<_,String>(16)?);
                Ok(value)
            })?.collect::<rusqlite::Result<_>>()?)
        })
    }
    pub fn update(&self, id: &str, changes: &Value) -> Result<()> {
        let (id, changes) = (id.to_owned(), changes.clone());
        self.write(move |c| update_session(c, &id, &changes, false))
    }
    pub fn checkpoint(
        &self,
        id: &str,
        kind: &str,
        data: &Value,
        message: Option<&Message>,
        result: Option<&Block>,
        counters: Option<&Value>,
    ) -> Result<Event> {
        let durable =
            kind != "tool_output" || message.is_some() || result.is_some() || counters.is_some();
        let (id, kind, data, message, result, counters) = (
            id.to_owned(),
            kind.to_owned(),
            data.clone(),
            message.cloned(),
            result.cloned(),
            counters.cloned(),
        );
        self.writer.call(durable, move |c| {
            let (id,kind,data,message,result,counters) = (id.as_str(),kind.as_str(),&data,message.as_ref(),result.as_ref(),counters.as_ref());
            if let Some(m) = message {
                append_message(c, id, m)?;
                for block in &m.content {
                    if let Block::ToolUse { id: call_id, .. } = block {
                        c.execute("INSERT OR IGNORE INTO tool_execution VALUES(?,?,0)", params![id,call_id])?;
                    }
                }
            }
            if let Some(r @ Block::ToolResult { tool_use_id, .. }) = result {
                c.execute(
                    "INSERT OR REPLACE INTO tool_results VALUES(?,?,?)",
                    params![id, tool_use_id, serde_json::to_string(r)?],
                )?;
            }
            if kind == "tool_started" {
                c.execute("INSERT INTO tool_execution VALUES(?,?,1) ON CONFLICT(session_id,call_id) DO UPDATE SET started=1", params![id,data["call_id"].as_str().context("missing call_id")?])?;
            }
            if let Some(counters) = counters {
                update_session(c, id, counters, true)?;
            }
            let seq: i64 = c.query_row(
                "SELECT COALESCE(MAX(seq),-1)+1 FROM events WHERE session_id=?",
                [id],
                |r| r.get(0),
            )?;
            let event = Event {
                seq,
                r#type: kind.into(),
                timestamp: util::now(),
                data: data.clone(),
            };
            c.execute(
                "INSERT INTO events VALUES(?,?,?,?,?)",
                params![id, seq, kind, event.timestamp, serde_json::to_string(data)?],
            )?;
            Ok(event)
        })
    }
    pub fn event(&self, id: &str, kind: &str, data: &Value) -> Result<Event> {
        self.checkpoint(id, kind, data, None, None, None)
    }
    pub fn append(&self, id: &str, message: &Message) -> Result<()> {
        let (id, message) = (id.to_owned(), message.clone());
        self.transaction(move |c| append_message(c, &id, &message))
    }
    pub fn messages(&self, id: &str) -> Result<Vec<Message>> {
        self.with(|c| read_messages(c, id))
    }
    pub fn message_revision(&self, id: &str) -> Result<i64> {
        self.with(|c| message_revision(c, id))
    }
    pub fn versioned_messages(&self, id: &str) -> Result<(Vec<Message>, i64)> {
        self.with(|c| {
            // Messages and version must describe the same SQLite snapshot,
            // including when another bridge process writes this database.
            let tx = c.unchecked_transaction()?;
            let revision = message_revision(&tx, id)?;
            let messages = read_messages(&tx, id)?;
            tx.commit()?;
            Ok((messages, revision))
        })
    }
    /// Newest-first SQL reads, returned in chronological order. A byte budget
    /// prevents many large events from becoming one unbounded NDJSON reply.
    /// One oversized row is allowed so paging always makes lossless progress.
    pub fn history_page(&self, id: &str, cursor: Option<&Value>, limit: usize) -> Result<Value> {
        let before = |field: &str| -> Result<Option<i64>> {
            match cursor {
                None => Ok(Some(i64::MAX)),
                Some(c) if c.is_object() => match c.get(field) {
                    Some(Value::Null) => Ok(None),
                    Some(v) => Ok(Some(
                        v.as_i64()
                            .filter(|n| *n >= 0)
                            .context("invalid history cursor")?,
                    )),
                    None => bail!("invalid history cursor"),
                },
                _ => bail!("invalid history cursor"),
            }
        };
        let message_before = before("messages")?;
        let event_before = before("events")?;
        let summary = self.require(id)?;
        let limit = limit.clamp(1, 100);
        self.with(|c| {
            let mut messages = Vec::new();
            let mut events = Vec::new();
            let mut message_cursor = None;
            let mut event_cursor = None;
            let mut message_offset = 0i64;
            if let Some(before) = message_before {
                let mut q = c.prepare("SELECT seq,role,content FROM messages WHERE session_id=? AND seq<? ORDER BY seq DESC LIMIT ?")?;
                let mut rows = q.query(params![id, before, (limit + 1) as i64])?;
                let mut bytes = 0;
                while let Some(row) = rows.next()? {
                    let content: String = row.get(2)?;
                    if !messages.is_empty() && (messages.len() == limit || bytes + content.len() > 2_000_000) {
                        message_cursor = Some(message_offset);
                        break;
                    }
                    bytes += content.len();
                    message_offset = row.get(0)?;
                    messages.push(Message { role: row.get(1)?, content: serde_json::from_str(&content)? });
                }
                messages.reverse();
            }
            if let Some(before) = event_before {
                let mut q = c.prepare("SELECT seq,type,timestamp,data FROM events WHERE session_id=? AND seq<? ORDER BY seq DESC LIMIT ?")?;
                let mut rows = q.query(params![id, before, (limit + 1) as i64])?;
                let mut bytes = 0;
                let mut oldest = 0;
                while let Some(row) = rows.next()? {
                    let data = restore_result_detail(c,id,serde_json::from_str(&row.get::<_,String>(3)?)?)?;
                    let size = serde_json::to_vec(&data)?.len();
                    if !events.is_empty() && (events.len() == limit || bytes + size > 2_000_000) {
                        event_cursor = Some(oldest);
                        break;
                    }
                    bytes += size;
                    oldest = row.get(0)?;
                    events.push(Event { seq: oldest, r#type: row.get(1)?, timestamp: row.get(2)?, data });
                }
                events.reverse();
            }
            Ok(json!({"summary":summary,"messages":messages,"events":events,"messageOffset":message_offset,"cursor":{"messages":message_cursor,"events":event_cursor}}))
        })
    }
    pub fn active_skills(&self, id: &str) -> Result<std::collections::BTreeMap<String, String>> {
        self.with(|c| {
            let mut q = c.prepare("SELECT type,CASE WHEN type='session_start' THEN json_object('active_skills',json_extract(data,'$.active_skills')) ELSE data END FROM events WHERE session_id=? AND type IN ('session_start','skill_activated','skill_deactivated','project_instructions_changed') ORDER BY seq")?;
            let mut rows = q.query([id])?;
            let mut active = std::collections::BTreeMap::new();
            while let Some(row) = rows.next()? {
                let kind: String = row.get(0)?;
                let data: Value = serde_json::from_str(&row.get::<_, String>(1)?)?;
                match kind.as_str() {
                    "session_start" => {
                        if let Some(skills) = data["active_skills"].as_object() {
                            active = skills.iter().map(|(name, sha)| (name.clone(), sha.as_str().unwrap_or_default().into())).collect();
                        }
                    }
                    "skill_activated" => { active.insert(util::string(&data, "name")?.into(), util::string(&data, "sha256")?.into()); }
                    "skill_deactivated" => { active.remove(data["name"].as_str().unwrap_or_default()); }
                    _ => for change in data["changes"].as_array().into_iter().flatten().filter(|c| c["kind"] == "skill") {
                        let name = util::string(change, "name")?;
                        if change["status"] == "removed" { active.remove(name); }
                        else { active.insert(name.into(), util::string(change, "sha256")?.into()); }
                    }
                }
            }
            Ok(active)
        })
    }
    pub fn saved_budget(&self, id: &str) -> Result<Option<i64>> {
        self.with(|c| Ok(c.query_row("SELECT json_extract(data,'$.budget.max_total_tokens') FROM events WHERE session_id=? AND type IN ('budget_changed','session_start') ORDER BY seq DESC LIMIT 1", [id], |r| r.get(0)).optional()?.flatten()))
    }
    pub fn queued_prompts(&self, id: &str) -> Result<Vec<Value>> {
        self.with(|c| {
            let mut query = c.prepare("SELECT q.data FROM events q WHERE q.session_id=? AND q.type='prompt_queued' AND NOT EXISTS (SELECT 1 FROM events d WHERE d.session_id=q.session_id AND d.type IN ('queue_consumed','queue_deleted') AND json_extract(d.data,'$.id')=json_extract(q.data,'$.id')) ORDER BY q.seq")?;
            let rows = query.query_map([id], |r| r.get::<_, String>(0))?;
            rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
        })
    }
    pub fn enqueue_prompt(
        &self,
        session: &str,
        mode: &str,
        text: &str,
        attachments: &[Block],
    ) -> Result<Value> {
        self.require(session)?;
        if !["steer", "followUp"].contains(&mode) {
            bail!("mode must be steer or followUp")
        }
        if text.trim().is_empty() {
            bail!("请输入任务内容")
        }
        if self.queued_prompts(session)?.len() >= 100 {
            bail!("队列最多100条消息")
        }
        if attachments.len() > 10 {
            bail!("附件超过10个")
        }
        let item =
            json!({"id":util::id(),"mode":mode,"text":text.trim(),"attachments":attachments});
        self.event(session, "prompt_queued", &item)?;
        Ok(item)
    }
    pub fn replace_messages(&self, id: &str, messages: &[Message]) -> Result<()> {
        let (id, messages) = (id.to_owned(), messages.to_vec());
        self.transaction(move |c| {
            let (id, messages) = (id.as_str(), messages.as_slice());
            c.execute("DELETE FROM messages WHERE session_id=?", [id])?;
            let mut insert = c.prepare_cached("INSERT INTO messages VALUES(?,?,?,?)")?;
            for (seq, m) in messages.iter().enumerate() {
                insert.execute(params![
                    id,
                    seq as i64,
                    m.role,
                    serde_json::to_string(&m.content)?
                ])?;
                cache_title(c, id, m)?;
            }
            Ok(())
        })
    }
    /// Restore only control state and projected accounting/identity fields.
    /// Output streams, assistant bodies and tool arguments stay on disk.
    pub fn recovery_events(&self, id: &str) -> Result<Vec<Event>> {
        self.with(|c| {
            let mut q = c.prepare("SELECT seq,type,timestamp,CASE
                WHEN type IN ('assistant_message','provider_response_rejected') THEN json_object('usage',json_extract(data,'$.usage'),'tool_call_ids',json_extract(data,'$.tool_call_ids'),'provider_progress',json_extract(data,'$.provider_progress'))
                WHEN type='tool_call_start' THEN json_object('call_id',json_extract(data,'$.call_id'),'name',json_extract(data,'$.name'),'arguments',json_object('path',json_extract(data,'$.arguments.path')))
                WHEN type IN ('tool_call_result','tool_started') THEN json_object('call_id',json_extract(data,'$.call_id'),'name',json_extract(data,'$.name'),'observed_paths',json_extract(data,'$.observed_paths'),'success',CASE WHEN json_extract(data,'$.success') THEN json('true') ELSE json('false') END)
                WHEN type='subagent_result' THEN json_object('child_session_id',json_extract(data,'$.child_session_id'),'kind',json_extract(data,'$.kind'),'task',json_extract(data,'$.task'),'exit_reason',json_extract(data,'$.exit_reason'),'provider_progress',json_extract(data,'$.provider_progress'))
                WHEN type IN ('background_job_started','background_job_completed','background_job_lost') THEN json_object('job_id',json_extract(data,'$.job_id'),'command',json_extract(data,'$.command'))
                ELSE data END FROM events WHERE session_id=? AND type IN ('session_start','project_instructions','skill_activated','skill_deactivated','project_instructions_changed','subagent_start','subagent_result','subagent_isolation','assistant_message','provider_response_rejected','tool_call_start','tool_call_result','tool_started','background_job_started','background_job_completed','background_job_lost') ORDER BY seq")?;
            q.query_map([id], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?
                .map(|row| {let (seq,kind,timestamp,data)=row?; Ok(Event{seq,r#type:kind,timestamp,data:serde_json::from_str(&data)?})}).collect::<Result<_>>()
        })
    }
    pub fn events(&self, id: &str, after: i64) -> Result<Vec<Event>> {
        self.with(|c|{
        let mut q=c.prepare("SELECT seq,type,timestamp,data FROM events WHERE session_id=? AND seq>? ORDER BY seq")?;
        let mut rows=q.query(params![id,after])?;let mut events=Vec::new();while let Some(row)=rows.next()? {events.push(Event{seq:row.get(0)?,r#type:row.get(1)?,timestamp:row.get(2)?,data:restore_result_detail(c,id,serde_json::from_str(&row.get::<_,String>(3)?)?)?});}Ok(events)
    })
    }
    pub fn background_notifications(&self, id: &str) -> Result<Vec<Value>> {
        self.with(|c| {
            let mut q = c.prepare("SELECT e.data FROM events e WHERE e.session_id=? AND e.type IN ('background_job_completed','background_job_lost') AND NOT EXISTS (SELECT 1 FROM events n WHERE n.session_id=e.session_id AND n.type='background_job_notified' AND json_extract(n.data,'$.job_id')=json_extract(e.data,'$.job_id')) ORDER BY e.seq")?;
            q.query_map([id], |r| r.get::<_,String>(0))?.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
        })
    }
    pub fn submitted_result(&self, id: &str) -> Result<Option<Value>> {
        self.with(|c| {
            let data:Option<String>=c.query_row("SELECT json_extract(t.data,'$.arguments') FROM events s JOIN events t ON t.session_id=s.session_id AND t.type='tool_call_start' AND json_extract(t.data,'$.call_id')=json_extract(s.data,'$.call_id') WHERE s.session_id=? AND s.type='subagent_result_submitted' ORDER BY s.seq DESC,t.seq DESC LIMIT 1",[id],|r|r.get(0)).optional()?.flatten();
            data.map(|s|Ok(serde_json::from_str(&s)?)).transpose()
        })
    }
    pub fn statistics_events(&self, id: &str, after: i64) -> Result<(i64, Vec<Event>)> {
        self.with(|c| {
            // Capture an upper bound so concurrent writers cannot advance the
            // cursor past events that this poll has not processed.
            let until = c.prepare_cached("SELECT COALESCE(MAX(seq),-1) FROM events WHERE session_id=?")?.query_row([id], |r| r.get(0))?;
            if until <= after { return Ok((until, Vec::new())); }
            let mut q = c.prepare_cached("SELECT seq,type,timestamp,CASE WHEN type='assistant_message' THEN json_object('usage',json_extract(data,'$.usage'),'request_seconds',json_extract(data,'$.request_seconds'),'generation_seconds',json_extract(data,'$.generation_seconds'),'first_token_seconds',json_extract(data,'$.first_token_seconds')) ELSE '{}' END FROM events WHERE session_id=? AND seq>? AND seq<=? AND type IN ('assistant_message','tool_call_start') ORDER BY seq")?;
            let mut rows = q.query(params![id, after, until])?;
            let mut events = Vec::new();
            while let Some(row) = rows.next()? {
                events.push(Event { seq: row.get(0)?, r#type: row.get(1)?, timestamp: row.get(2)?, data: serde_json::from_str(&row.get::<_, String>(3)?)? });
            }
            Ok((until, events))
        })
    }
    pub fn tool_execution(&self, id: &str) -> Result<std::collections::HashMap<String, bool>> {
        self.with(|c| {
            let mut q =
                c.prepare("SELECT call_id,started FROM tool_execution WHERE session_id=?")?;
            Ok(q.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
        })
    }
    pub fn tool_results_for(
        &self,
        id: &str,
        calls: &[String],
    ) -> Result<std::collections::HashMap<String, Block>> {
        if calls.is_empty() {
            return Ok(Default::default());
        }
        self.with(|c| {
            let mut q = c.prepare("SELECT call_id,result FROM tool_results WHERE session_id=? AND call_id IN (SELECT value FROM json_each(?))")?;
            q.query_map(params![id,serde_json::to_string(calls)?], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                .map(|row| {let (id,result)=row?; Ok((id,serde_json::from_str(&result)?))}).collect()
        })
    }
    pub fn tool_results(&self, id: &str) -> Result<std::collections::HashMap<String, Block>> {
        self.with(|c| {
            let mut q = c.prepare("SELECT call_id,result FROM tool_results WHERE session_id=?")?;
            let mut rows = q.query([id])?;
            let mut results = std::collections::HashMap::new();
            while let Some(r) = rows.next()? {
                results.insert(r.get(0)?, serde_json::from_str(&r.get::<_, String>(1)?)?);
            }
            Ok(results)
        })
    }
    pub fn goal(&self, id: &str) -> Result<Option<Value>> {
        self.with(|c| {
            let raw: Option<String> = c
                .query_row(
                    "SELECT state FROM session_goals WHERE session_id=?",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
        })
    }
    pub fn save_goal(&self, id: &str, state: &Value) -> Result<()> {
        let (id, state) = (id.to_owned(), state.clone());
        self.write(move |c| {
            let (id, state) = (id.as_str(), &state);
            c.execute(
                "INSERT OR REPLACE INTO session_goals VALUES(?,?)",
                params![id, serde_json::to_string(state)?],
            )?;
            Ok(())
        })
    }
    pub fn rename(&self, id: &str, title: Option<&str>) -> Result<()> {
        self.require(id)?;
        let (id, title) = (id.to_owned(), title.map(str::to_owned));
        self.write(move |c|{c.execute("INSERT INTO session_meta(session_id,title) VALUES(?,?) ON CONFLICT(session_id) DO UPDATE SET title=excluded.title",params![id,title])?;Ok(())})
    }
    pub fn pin(&self, id: &str, pinned: bool) -> Result<()> {
        self.require(id)?;
        let id = id.to_owned();
        self.write(move |c|{c.execute("INSERT INTO session_meta(session_id,pinned) VALUES(?,?) ON CONFLICT(session_id) DO UPDATE SET pinned=excluded.pinned",params![id,pinned])?;Ok(())})
    }
    pub fn delete(&self, id: &str) -> Result<()> {
        let mut ownership = vec![self.activation(id)?];
        self.require(id)?;
        let ids: Vec<String> = self.with(|c| {
            let mut q = c.prepare("WITH RECURSIVE descendants(id) AS (SELECT ? UNION SELECT json_extract(e.data,'$.child_session_id') FROM events e JOIN descendants d ON e.session_id=d.id WHERE e.type='subagent_start' AND json_type(e.data,'$.child_session_id')='text') SELECT id FROM descendants ORDER BY id")?;
            Ok(q.query_map([id], |row|row.get(0))?.collect::<rusqlite::Result<_>>()?)
        })?;
        for child in &ids {
            if !util::valid_id(child) {
                bail!("invalid child session id");
            }
            if child != id && self.session(child)?.is_some() {
                ownership.push(self.activation(child)?);
            }
        }
        let root = self.root().join("artifacts");
        let mut directories = vec![];
        if root.exists() {
            if util::is_link(&root) {
                bail!("refusing to delete linked artifacts directory");
            }
            let root = util::canonical(&root)?;
            for session in &ids {
                let dir = root.join(session);
                if dir.exists() {
                    if util::is_link(&dir) {
                        bail!("refusing to delete linked session artifacts");
                    }
                    let resolved = util::canonical(&dir)?;
                    if !resolved.starts_with(&root) {
                        bail!("session artifacts escape storage root");
                    }
                    directories.push(resolved);
                }
            }
        }
        // An owner may have crashed while retaining a reviewable snapshot.
        // Clean it under the same activation locks before deleting its journal.
        let mut snapshots = std::collections::HashSet::new();
        for session in &ids {
            let Some(summary) = self.session(session)? else {
                continue;
            };
            for event in self
                .recovery_events(session)?
                .iter()
                .filter(|event| event.r#type == "subagent_isolation")
            {
                let snapshot = util::string(&event.data, "isolation_id")?;
                if snapshots.insert(snapshot.to_string())
                    && let Some(workspace) = crate::delegate_workspace::Workspace::load(
                        &self.root(),
                        session,
                        Path::new(util::string(&summary, "workspace")?),
                        snapshot,
                    )?
                {
                    workspace.discard()?;
                }
            }
        }
        let deleted_ids = ids.clone();
        self.transaction(move |c| {
            for id in deleted_ids {
                for table in [
                    "sessions",
                    "messages",
                    "events",
                    "artifacts",
                    "artifact_access",
                    "session_goals",
                    "session_meta",
                    "session_prompts",
                    "tool_results",
                    "tool_execution",
                    "tasks",
                    "message_revisions",
                ] {
                    c.execute(&format!("DELETE FROM {table} WHERE session_id=?"), [&id])?;
                }
            }
            Ok(())
        })?;
        for dir in directories {
            fs::remove_dir_all(dir)?;
        }
        // All rows are gone before releasing ownership. A stale activation
        // checks existence after locking and cannot start a deleted session.
        drop(ownership);
        for session in ids {
            match fs::remove_file(self.lock_path(&session)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
    pub fn root(&self) -> PathBuf {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }
    pub fn activation(&self, id: &str) -> Result<SessionLock> {
        if !util::valid_id(id) {
            bail!("invalid session id")
        }
        let path = self.lock_path(id);
        fs::create_dir_all(path.parent().unwrap())?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        fs2::FileExt::try_lock_exclusive(&f)
            .with_context(|| format!("session already running: {id}"))?;
        let lock = SessionLock(f);
        if self.session(id)?.is_none() {
            drop(lock);
            let _ = fs::remove_file(path);
            bail!("找不到会话");
        }
        if lock.0.metadata()?.len() == 0 {
            (&lock.0).write_all(&[0])?;
        }
        Ok(lock)
    }
    fn lock_path(&self, id: &str) -> PathBuf {
        self.root()
            .join(format!(
                ".{}.locks",
                self.path.file_name().unwrap_or_default().to_string_lossy()
            ))
            .join(format!("{id}.lock"))
    }
    pub fn spill(&self, id: &str, kind: &str, text: &str) -> Result<String> {
        if !util::valid_id(id) || !util::valid_id(kind) {
            bail!("invalid artifact name")
        }
        let artifact = format!("{kind}_{}", &util::id()[..12]);
        let rel = format!("{artifact}.txt");
        let path = self.root().join("artifacts").join(id).join(&rel);
        util::atomic_write(&path, text.as_bytes())?;
        let (record_session, record_artifact, record_kind, record_rel) = (
            id.to_owned(),
            artifact.clone(),
            kind.to_owned(),
            rel.clone(),
        );
        if let Err(e) = self.write(move |c| {
            let (id,artifact,kind,rel) = (&record_session,&record_artifact,&record_kind,&record_rel);
            c.execute(
                "INSERT INTO artifacts(session_id,artifact_id,kind,relpath,created_at) VALUES(?,?,?,?,?)",
                params![id, artifact, kind, rel, util::now()],
            )?;
            Ok(())
        }) {
            let _ = fs::remove_file(path);
            return Err(e);
        }
        self.prune_output_artifacts(Some((id, &artifact)))?;
        Ok(artifact)
    }
    /// Bound disposable output archives; compressed session history is retained.
    pub(crate) fn prune_output_artifacts(&self, keep: Option<(&str, &str)>) -> Result<()> {
        let setting = |name, default| {
            std::env::var(name)
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(default)
        };
        let max_bytes = setting("MINICODE_ARTIFACT_MAX_BYTES", 1024 * 1024 * 1024);
        let max_days = setting("MINICODE_ARTIFACT_MAX_AGE_DAYS", 30);
        if max_bytes == 0 && max_days == 0 {
            return Ok(());
        }
        let root = self.root().join("artifacts");
        if !root.exists() || util::is_link(&root) {
            return Ok(());
        }
        let keep = keep.map(|(id, artifact)| (id.to_owned(), artifact.to_owned()));
        self.transaction(move |c| {
            let mut query = c.prepare("SELECT a.session_id,a.artifact_id,a.relpath,a.created_at FROM artifacts a LEFT JOIN artifact_access x USING(session_id,artifact_id) WHERE a.kind IN ('command_output','tool_output') ORDER BY COALESCE(x.accessed_at,a.created_at),a.created_at,a.artifact_id")?;
            let rows = query.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let mut entries = vec![];
            let mut total = 0u64;
            for (session, artifact, rel, created) in rows {
                if !util::valid_id(&session) || !util::valid_id(&artifact) { continue; }
                let Ok(path) = util::in_workspace(&root, &format!("{session}/{rel}")) else { continue; };
                if util::is_link(&path) { continue; }
                let size = match path.metadata() {
                    Ok(m) if m.is_file() => m.len(),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                    _ => continue,
                };
                total = total.saturating_add(size);
                entries.push((session, artifact, path, created, size));
            }
            for (session, artifact, path, created, size) in entries {
                if keep.as_ref().map(|(id,artifact)|(id.as_str(),artifact.as_str())) == Some((session.as_str(),artifact.as_str())) { continue; }
                let expired = max_days > 0 && chrono::DateTime::parse_from_rfc3339(&created).is_ok_and(|created| chrono::Utc::now().signed_duration_since(created).num_seconds() > max_days.saturating_mul(86400).min(i64::MAX as u64) as i64);
                if !expired && (max_bytes == 0 || total <= max_bytes) { continue; }
                match fs::remove_file(&path) {
                    Ok(()) => {},
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                    Err(_) => continue, // Locked files are retried on the next sweep.
                }
                c.execute("DELETE FROM artifacts WHERE session_id=? AND artifact_id=?",params![session,artifact])?;
                c.execute("DELETE FROM artifact_access WHERE session_id=? AND artifact_id=?",params![session,artifact])?;
                total = total.saturating_sub(size);
            }
            Ok(())
        })
    }
    pub fn artifact_path(&self, id: &str, artifact: &str) -> Result<PathBuf> {
        if !util::valid_id(id) || !util::valid_id(artifact) {
            bail!("invalid artifact id")
        }
        let rel = self
            .with(|c| {
                Ok(c.query_row(
                    "SELECT relpath FROM artifacts WHERE session_id=? AND artifact_id=?",
                    params![id, artifact],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
            })?
            .context("归档内容不可用")?;
        let path = util::in_workspace(&self.root().join("artifacts").join(id), &rel)?;
        if path.is_file() {
            let (id, artifact) = (id.to_owned(), artifact.to_owned());
            self.write(move |c| {
                c.execute("INSERT INTO artifact_access VALUES(?,?,?) ON CONFLICT(session_id,artifact_id) DO UPDATE SET accessed_at=excluded.accessed_at",params![id,artifact,util::now()])?;
                Ok(())
            })?;
        }
        Ok(path)
    }
    pub fn artifact_page(
        &self,
        id: &str,
        artifact: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        let path = self.artifact_path(id, artifact)?;
        self.pages.read(&path, offset, limit)
    }
    pub fn tasks(&self, id: &str) -> Result<Vec<Value>> {
        self.with(|c|{
        let mut q=c.prepare("SELECT task_id,title,depends_on,status,owner,created_at,updated_at FROM tasks WHERE session_id=? ORDER BY created_at,task_id")?;
        let mut rows=q.query([id])?;let mut v=vec![];while let Some(r)=rows.next()? {v.push(json!({"task_id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"depends_on":serde_json::from_str::<Value>(&r.get::<_,String>(2)?)?,"status":r.get::<_,String>(3)?,"owner":r.get::<_,Option<String>>(4)?,"created_at":r.get::<_,String>(5)?,"updated_at":r.get::<_,String>(6)?}));}Ok(v)
    })
    }
    pub fn task_create(&self, id: &str, args: &Value) -> Result<()> {
        let task = util::string(args, "task_id")?;
        let title = util::string(args, "title")?;
        let deps = args.get("depends_on").cloned().unwrap_or(json!([]));
        let (id, task, title) = (id.to_owned(), task.to_owned(), title.to_owned());
        self.transaction(move |c| {
            let (id, task, title) = (id.as_str(), task.as_str(), title.as_str());
            if c.query_row(
                "SELECT 1 FROM tasks WHERE session_id=? AND task_id=?",
                params![id, task],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
            {
                bail!("task already exists: {task}")
            }
            let mut ready = true;
            let deps_list = deps.as_array().context("depends_on must be an array")?;
            if !deps_list.is_empty() {
                let mut query =
                    c.prepare("SELECT task_id,depends_on,status FROM tasks WHERE session_id=?")?;
                let rows = query.query_map([id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?;
                let mut graph = std::collections::HashMap::<String, Vec<String>>::new();
                let mut statuses = std::collections::HashMap::new();
                for row in rows {
                    let (key, raw, status) = row?;
                    graph.insert(key.clone(), serde_json::from_str(&raw)?);
                    statuses.insert(key, status);
                }
                let dependencies = deps_list
                    .iter()
                    .map(|v| v.as_str().map(str::to_string).context("invalid dependency"))
                    .collect::<Result<Vec<_>>>()?;
                for dep in &dependencies {
                    ready &= statuses
                        .get(dep)
                        .with_context(|| format!("unknown dependency: {dep}"))?
                        == "done";
                }
                graph.insert(task.to_string(), dependencies);
                // Iterative DFS visits each reachable edge once and detects corrupt legacy cycles.
                let mut colours = std::collections::HashMap::<&str, u8>::new();
                let mut stack = vec![(task, false)];
                while let Some((key, done)) = stack.pop() {
                    if done {
                        colours.insert(key, 2);
                        continue;
                    }
                    match colours.get(key) {
                        Some(1) => bail!("dependency cycle detected: {key}"),
                        Some(2) => continue,
                        _ => {}
                    }
                    colours.insert(key, 1);
                    stack.push((key, true));
                    if let Some(children) = graph.get(key) {
                        for child in children.iter().rev() {
                            stack.push((child, false));
                        }
                    }
                }
            }
            let time = util::now();
            c.execute(
                "INSERT INTO tasks VALUES(?,?,?,?,?,NULL,?,?)",
                params![
                    id,
                    task,
                    title,
                    deps.to_string(),
                    if ready { "ready" } else { "pending" },
                    time,
                    time
                ],
            )?;
            Ok(())
        })
    }
    pub fn task_claim(&self, id: &str, task: &str, owner: &str) -> Result<()> {
        let (id, task, owner) = (id.to_owned(), task.to_owned(), owner.to_owned());
        self.transaction(move |c|{
        if c.execute("UPDATE tasks SET status='running',owner=?,updated_at=? WHERE session_id=? AND task_id=? AND status='ready'",params![owner,util::now(),id,task])?!=1 {bail!("task is unknown or not ready: {task}")}Ok(())
    })
    }
    pub fn task_complete(&self, id: &str, task: &str, success: bool) -> Result<()> {
        let (id, task) = (id.to_owned(), task.to_owned());
        self.transaction(move |c|{
        if c.execute("UPDATE tasks SET status=?,updated_at=? WHERE session_id=? AND task_id=? AND status='running' AND owner='agent'",params![if success{"done"}else{"failed"},util::now(),id,task])?!=1 {bail!("task is unknown or not running: {task}")}
        // One set query updates all newly ready dependants, with indexed lookups.
        c.execute("UPDATE tasks SET status='ready',updated_at=? WHERE session_id=? AND status='pending' AND NOT EXISTS(SELECT 1 FROM json_each(tasks.depends_on) AS d LEFT JOIN tasks AS parent ON parent.session_id=tasks.session_id AND parent.task_id=d.value WHERE parent.status IS NULL OR parent.status!='done')",params![util::now(),id])?;Ok(())
    })
    }
    pub fn memories(&self, workspace: &str) -> Result<Vec<Value>> {
        self.with(|c|{
        let mut q=c.prepare("SELECT id,workspace,scope,fact,source,updated_at FROM project_memory WHERE workspace=? ORDER BY updated_at,id")?;
        Ok(q.query_map([workspace],|r|Ok(json!({"id":r.get::<_,String>(0)?,"workspace":r.get::<_,String>(1)?,"scope":r.get::<_,String>(2)?,"fact":r.get::<_,String>(3)?,"source":r.get::<_,String>(4)?,"updated_at":r.get::<_,String>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    }
}
fn restore_result_detail(c: &Connection, id: &str, mut data: Value) -> Result<Value> {
    if let Some(call_id) = data["result_ref"].as_str() {
        let raw: Option<String> = c
            .query_row(
                "SELECT result FROM tool_results WHERE session_id=? AND call_id=?",
                params![id, call_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = raw
            && let Block::ToolResult {
                content, images, ..
            } = serde_json::from_str::<Block>(&raw)?
        {
            let offset = data["output_offset"].as_u64().unwrap_or(0) as usize;
            data["output_detail"] = json!(content.get(offset..).unwrap_or(&content));
            data["images"] = json!(images);
        }
    }
    Ok(data)
}
fn sql_value(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
        Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap_or_default()),
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        _ => rusqlite::types::Value::Text(v.to_string()),
    }
}
fn update_session(c: &Connection, id: &str, changes: &Value, counters_only: bool) -> Result<()> {
    let changes = changes.as_object().context("invalid counters")?;
    if changes.is_empty() {
        return Ok(());
    }
    let mut sql = String::with_capacity(
        64 + changes.keys().map(String::len).sum::<usize>() + changes.len() * 3,
    );
    sql.push_str("UPDATE sessions SET ");
    let mut values = Vec::with_capacity(changes.len() + 1);
    for (key, value) in changes {
        if !matches!(
            key.as_str(),
            "status"
                | "exit_reason"
                | "rounds"
                | "input_tokens"
                | "output_tokens"
                | "cache_read_tokens"
                | "cache_write_tokens"
                | "usage_available"
        ) && (counters_only || !matches!(key.as_str(), "model" | "provider" | "workspace"))
        {
            bail!(if counters_only {
                "invalid counter"
            } else {
                "invalid session column"
            });
        }
        if !values.is_empty() {
            sql.push(',');
        }
        sql.push_str(key);
        sql.push_str("=?");
        values.push(sql_value(value));
    }
    values.push(rusqlite::types::Value::Text(id.into()));
    sql.push_str(" WHERE session_id=?");
    c.prepare_cached(&sql)?
        .execute(rusqlite::params_from_iter(values))?;
    Ok(())
}
fn append_message(c: &Connection, id: &str, m: &Message) -> Result<()> {
    let seq: i64 = c.query_row(
        "SELECT COALESCE(MAX(seq),-1)+1 FROM messages WHERE session_id=?",
        [id],
        |r| r.get(0),
    )?;
    c.execute(
        "INSERT INTO messages VALUES(?,?,?,?)",
        params![id, seq, m.role, serde_json::to_string(&m.content)?],
    )?;
    cache_title(c, id, m)
}
fn cache_title(c: &Connection, id: &str, message: &Message) -> Result<()> {
    if message.role == "user" {
        let text = message.text().chars().take(72).collect::<String>();
        let title = text.lines().next().unwrap_or("新建任务");
        c.prepare_cached(
            "UPDATE sessions SET auto_title=? WHERE session_id=? AND auto_title IS NULL",
        )?
        .execute(params![title, id])?;
    }
    Ok(())
}
pub fn counters(rounds: u64, usage: &Usage) -> Value {
    json!({"rounds":rounds,"input_tokens":usage.input_tokens,"output_tokens":usage.output_tokens,"cache_read_tokens":usage.cache_read_tokens,"cache_write_tokens":usage.cache_write_tokens,"usage_available":usage.available})
}

fn message_revision(c: &Connection, id: &str) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COALESCE((SELECT version FROM message_revisions WHERE session_id=?),0)",
        [id],
        |r| r.get(0),
    )?)
}
fn read_messages(c: &Connection, id: &str) -> Result<Vec<Message>> {
    let mut q = c.prepare("SELECT role,content FROM messages WHERE session_id=? ORDER BY seq")?;
    let mut rows = q.query([id])?;
    let mut messages = Vec::new();
    while let Some(row) = rows.next()? {
        let content: String = row.get(1)?;
        messages.push(Message {
            role: row.get(0)?,
            content: serde_json::from_str(&content)?,
        });
    }
    Ok(messages)
}
