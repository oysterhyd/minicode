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
        let mut c = Connection::open(&path)?;
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
            CREATE TABLE IF NOT EXISTS session_goals(session_id TEXT PRIMARY KEY,state TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS session_meta(session_id TEXT PRIMARY KEY,title TEXT,pinned INTEGER NOT NULL DEFAULT 0);
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
        tx.execute_batch("CREATE INDEX IF NOT EXISTS events_activity ON events(session_id,timestamp);
            CREATE INDEX IF NOT EXISTS messages_first_user ON messages(session_id,seq) WHERE role='user';
            CREATE INDEX IF NOT EXISTS sessions_created ON sessions(created_at); PRAGMA user_version=7;")?;
        tx.commit()?;
        Ok(Self {
            path,
            connection: Arc::new(Mutex::new(c)),
            pages: Arc::new(crate::paging::TextPages::default()),
        })
    }
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
        f(&connection)
    }
    pub fn transaction<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut c = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn create(&self, workspace: &str, provider: &str, model: &str) -> Result<String> {
        let id = util::id();
        self.with(|c|{c.execute("INSERT INTO sessions(session_id,created_at,workspace,provider,model) VALUES(?,?,?,?,?)",params![id,util::now(),workspace,provider,model])?;Ok(())})?;
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
    pub fn sessions(&self) -> Result<Vec<Value>> {
        self.with(|c|{
        let mut q=c.prepare(&format!("SELECT {COLUMNS} FROM sessions ORDER BY created_at DESC,session_id"))?;
        let rows=q.query_map([],summary)?;let mut result=Vec::new();
        let mut title_query=c.prepare("SELECT content FROM messages WHERE session_id=? AND role='user' ORDER BY seq LIMIT 1")?;
        let mut meta_query=c.prepare("SELECT title,pinned FROM session_meta WHERE session_id=?")?;
        let mut time_query=c.prepare("SELECT MAX(timestamp) FROM events WHERE session_id=?")?;
        for row in rows {let mut v=row?;let id=v["session_id"].as_str().unwrap();
            let (custom,pinned)=meta_query.query_row([id],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,bool>(1)?))).optional()?.unwrap_or((None,false));
            let content:Option<String>=title_query.query_row([id],|r|r.get(0)).optional()?;
            let title=custom.clone().unwrap_or_else(||content.and_then(|s|serde_json::from_str::<Vec<Block>>(&s).ok()).map(|blocks|blocks.iter().filter_map(|b|if let Block::Text{text}=b {Some(text.as_str())} else {None}).collect::<String>()).map(|s|s.chars().take(72).collect::<String>().lines().next().unwrap_or("新建任务").to_string()).unwrap_or("新建任务".into()));
            let activity:Option<String>=time_query.query_row([id],|r|r.get(0))?;
            v["title"]=json!(title);v["custom_title"]=json!(custom.is_some());v["pinned"]=json!(pinned);v["updated_at"]=activity.map(Value::String).unwrap_or(v["created_at"].clone());result.push(v);
        }Ok(result)
    })
    }
    pub fn update(&self, id: &str, changes: &Value) -> Result<()> {
        self.with(|c| update_session(c, id, changes, false))
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
        self.transaction(|c| {
            if let Some(m) = message {
                append_message(c, id, m)?;
            }
            if let Some(r @ Block::ToolResult { tool_use_id, .. }) = result {
                c.execute(
                    "INSERT OR REPLACE INTO tool_results VALUES(?,?,?)",
                    params![id, tool_use_id, serde_json::to_string(r)?],
                )?;
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
        self.transaction(|c| append_message(c, id, message))
    }
    pub fn messages(&self, id: &str) -> Result<Vec<Message>> {
        self.with(|c| {
            let mut q =
                c.prepare("SELECT role,content FROM messages WHERE session_id=? ORDER BY seq")?;
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
        })
    }
    pub fn replace_messages(&self, id: &str, messages: &[Message]) -> Result<()> {
        self.transaction(|c| {
            c.execute("DELETE FROM messages WHERE session_id=?", [id])?;
            let mut insert = c.prepare_cached("INSERT INTO messages VALUES(?,?,?,?)")?;
            for (seq, m) in messages.iter().enumerate() {
                insert.execute(params![
                    id,
                    seq as i64,
                    m.role,
                    serde_json::to_string(&m.content)?
                ])?;
            }
            Ok(())
        })
    }
    pub fn events(&self, id: &str, after: i64) -> Result<Vec<Event>> {
        self.with(|c|{
        let mut q=c.prepare("SELECT seq,type,timestamp,data FROM events WHERE session_id=? AND seq>? ORDER BY seq")?;
        let mut rows=q.query(params![id,after])?;let mut events=Vec::new();while let Some(row)=rows.next()? {events.push(Event{seq:row.get(0)?,r#type:row.get(1)?,timestamp:row.get(2)?,data:serde_json::from_str(&row.get::<_,String>(3)?)?});}Ok(events)
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
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO session_goals VALUES(?,?)",
                params![id, serde_json::to_string(state)?],
            )?;
            Ok(())
        })
    }
    pub fn rename(&self, id: &str, title: Option<&str>) -> Result<()> {
        self.require(id)?;
        self.with(|c|{c.execute("INSERT INTO session_meta(session_id,title) VALUES(?,?) ON CONFLICT(session_id) DO UPDATE SET title=excluded.title",params![id,title])?;Ok(())})
    }
    pub fn pin(&self, id: &str, pinned: bool) -> Result<()> {
        self.require(id)?;
        self.with(|c|{c.execute("INSERT INTO session_meta(session_id,pinned) VALUES(?,?) ON CONFLICT(session_id) DO UPDATE SET pinned=excluded.pinned",params![id,pinned])?;Ok(())})
    }
    pub fn delete(&self, id: &str) -> Result<()> {
        let _ownership = self.activation(id)?;
        self.require(id)?;
        self.transaction(|c| {
            for table in [
                "sessions",
                "messages",
                "events",
                "artifacts",
                "session_goals",
                "session_meta",
                "tool_results",
                "tasks",
            ] {
                c.execute(&format!("DELETE FROM {table} WHERE session_id=?"), [id])?;
            }
            Ok(())
        })?;
        if util::valid_id(id) {
            let dir = self.root().join("artifacts").join(id);
            if dir.exists() {
                fs::remove_dir_all(dir)?;
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
        let dir = self.root().join(format!(
            ".{}.locks",
            self.path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::create_dir_all(&dir)?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(format!("{id}.lock")))?;
        fs2::FileExt::try_lock_exclusive(&f)
            .with_context(|| format!("session already running: {id}"))?;
        let lock = SessionLock(f);
        if lock.0.metadata()?.len() == 0 {
            (&lock.0).write_all(&[0])?;
        }
        Ok(lock)
    }
    pub fn spill(&self, id: &str, kind: &str, text: &str) -> Result<String> {
        if !util::valid_id(id) || !util::valid_id(kind) {
            bail!("invalid artifact name")
        }
        let artifact = format!("{kind}_{}", &util::id()[..12]);
        let rel = format!("{artifact}.txt");
        let path = self.root().join("artifacts").join(id).join(&rel);
        util::atomic_write(&path, text.as_bytes())?;
        if let Err(e) = self.with(|c| {
            c.execute(
                "INSERT INTO artifacts VALUES(?,?,?,?,?)",
                params![id, artifact, kind, rel, util::now()],
            )?;
            Ok(())
        }) {
            let _ = fs::remove_file(path);
            return Err(e);
        }
        Ok(artifact)
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
        util::in_workspace(&self.root().join("artifacts").join(id), &rel)
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
        self.transaction(|c| {
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
        self.transaction(|c|{
        if c.execute("UPDATE tasks SET status='running',owner=?,updated_at=? WHERE session_id=? AND task_id=? AND status='ready'",params![owner,util::now(),id,task])?!=1 {bail!("task is unknown or not ready: {task}")}Ok(())
    })
    }
    pub fn task_complete(&self, id: &str, task: &str, success: bool) -> Result<()> {
        self.transaction(|c|{
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
    Ok(())
}
pub fn counters(rounds: u64, usage: &Usage) -> Value {
    json!({"rounds":rounds,"input_tokens":usage.input_tokens,"output_tokens":usage.output_tokens,"cache_read_tokens":usage.cache_read_tokens,"cache_write_tokens":usage.cache_write_tokens,"usage_available":usage.available})
}
