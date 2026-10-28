use crate::{
    attachments::Attachments,
    extensions::Server,
    models::{Outcome, ToolSpec},
    util,
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

const STDERR_TAIL_BYTES: usize = 60_000;
const STDERR_LOG_BYTES: u64 = 1_000_000;

#[derive(Default)]
struct Diagnostics {
    tail: VecDeque<u8>,
    log_path: Option<PathBuf>,
}
impl Diagnostics {
    fn append(&mut self, bytes: &[u8]) {
        self.tail.extend(bytes);
        let excess = self.tail.len().saturating_sub(STDERR_TAIL_BYTES);
        self.tail.drain(..excess);
    }
    fn tail(&self) -> String {
        let bytes = self.tail.iter().copied().collect::<Vec<_>>();
        util::tail(
            &util::terminal_text(&String::from_utf8_lossy(&bytes)),
            200,
            STDERR_TAIL_BYTES,
        )
    }
}

fn sensitive_environment(config: &Server) -> Vec<Vec<u8>> {
    let sensitive = |name: &str| {
        let name = name.to_ascii_uppercase();
        [
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "CREDENTIAL",
            "PRIVATE_KEY",
            "_KEY",
            "AUTH",
        ]
        .iter()
        .any(|part| name.contains(part))
    };
    let mut values = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .chain(config.env.iter().map(|(k, v)| (k.clone(), v.clone())))
        .filter(|(key, value)| sensitive(key) && value.len() >= 4)
        .map(|(_, value)| value.into_bytes())
        .collect::<Vec<_>>();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    values
}

fn diagnostic_log() -> (Option<PathBuf>, Option<tokio::fs::File>) {
    let dir = util::data_dir().join("logs").join("mcp");
    if std::fs::create_dir_all(&dir).is_err() {
        return (None, None);
    }
    let path = dir.join(format!("{}.log", util::id()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(file) => (Some(path), Some(tokio::fs::File::from_std(file))),
        Err(_) => (None, None),
    }
}

// Preserve possible partial matches across reads so secrets split at pipe
// boundaries cannot reach either the diagnostic file or the status event.
fn redact_stderr(pending: &mut Vec<u8>, secrets: &[Vec<u8>], eof: bool) -> Vec<u8> {
    let mut output = Vec::with_capacity(pending.len());
    let mut position = 0;
    while position < pending.len() {
        let remaining = &pending[position..];
        if !eof
            && secrets
                .iter()
                .any(|s| s.len() > remaining.len() && s.starts_with(remaining))
        {
            break;
        }
        if let Some(secret) = secrets.iter().find(|s| remaining.starts_with(s)) {
            output.extend_from_slice(b"[redacted]");
            position += secret.len();
        } else {
            output.push(remaining[0]);
            position += 1;
        }
    }
    pending.drain(..position);
    output
}

async fn capture_stderr(
    mut stderr: ChildStderr,
    diagnostics: Arc<StdMutex<Diagnostics>>,
    secrets: Vec<Vec<u8>>,
    mut log: Option<tokio::fs::File>,
) {
    let mut pending = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut written = 0u64;
    loop {
        let n = stderr.read(&mut buffer).await.unwrap_or(0);
        pending.extend_from_slice(&buffer[..n]);
        let output = redact_stderr(&mut pending, &secrets, n == 0);
        let tail = {
            let mut state = diagnostics.lock().unwrap();
            state.append(&output);
            state.tail.iter().copied().collect::<Vec<_>>()
        };
        if let Some(file) = log.as_mut() {
            if written + output.len() as u64 > STDERR_LOG_BYTES {
                if file.set_len(0).await.is_err()
                    || file.seek(std::io::SeekFrom::Start(0)).await.is_err()
                    || file.write_all(&tail).await.is_err()
                {
                    log = None;
                } else {
                    written = tail.len() as u64;
                }
            } else if file.write_all(&output).await.is_err() {
                log = None;
            } else {
                written += output.len() as u64;
            }
        }
        if n == 0 {
            break;
        }
    }
    if let Some(file) = log.as_mut() {
        let _ = file.flush().await;
    }
}

#[cfg(windows)]
fn windows_command(config: &Server) -> std::path::PathBuf {
    use std::path::{Path, PathBuf};

    let env = |name: &str| {
        config
            .env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| std::ffi::OsString::from(value))
    };
    let path = env("PATH")
        .or_else(util::bundled_path)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let extensions = env("PATHEXT")
        .or_else(|| std::env::var_os("PATHEXT"))
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let extensions = extensions.to_string_lossy();
    let command = Path::new(&config.command);
    let has_directory = command.components().count() > 1 || command.is_absolute();
    let directories: Vec<PathBuf> = if has_directory {
        vec![config.cwd.clone()]
    } else {
        std::env::split_paths(&path)
            .map(|dir| {
                if dir.is_absolute() {
                    dir
                } else {
                    config.cwd.join(dir)
                }
            })
            .collect()
    };
    for dir in directories {
        let base = dir.join(command);
        if base.extension().is_some_and(|ext| {
            ["exe", "com", "bat", "cmd"]
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        }) && base.is_file()
        {
            return base;
        }
        for extension in extensions.split(';').filter(|ext| {
            [".exe", ".com", ".bat", ".cmd"]
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        }) {
            let mut candidate = base.as_os_str().to_os_string();
            candidate.push(extension);
            let candidate = PathBuf::from(candidate);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    command.to_path_buf()
}

pub struct Connector {
    pub config: Server,
    pub protocol: Option<String>,
    pub server_version: Option<String>,
    child: Option<Child>,
    tree: Option<crate::command_process::ProcessTree>,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
    client: reqwest::Client,
    session: Option<String>,
    sequence: u64,
    legacy_receiver: Option<tokio::sync::mpsc::Receiver<Value>>,
    legacy_task: Option<tokio::task::JoinHandle<()>>,
    endpoint: Option<String>,
    diagnostics: Arc<StdMutex<Diagnostics>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Connector {
    fn drop(&mut self) {
        if let Some(task) = self.legacy_task.take() {
            task.abort();
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}
impl Connector {
    pub fn new(config: Server) -> Result<Self> {
        if !config.timeout.is_finite() || !(0.0 < config.timeout && config.timeout <= 3600.0) {
            bail!("invalid MCP timeout");
        }
        Ok(Self {
            config,
            protocol: None,
            server_version: None,
            child: None,
            tree: None,
            input: None,
            output: None,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            session: None,
            sequence: 0,
            legacy_receiver: None,
            legacy_task: None,
            endpoint: None,
            diagnostics: Arc::new(StdMutex::new(Diagnostics::default())),
            stderr_task: None,
        })
    }
    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .post(url)
            .header("Accept", "application/json, text/event-stream");
        for (k, v) in &self.config.headers {
            r = r.header(k, v);
        }
        if let Some(session) = &self.session {
            r = r.header("Mcp-Session-Id", session);
        }
        if let Some(protocol) = &self.protocol {
            r = r.header("MCP-Protocol-Version", protocol);
        }
        r
    }
    pub async fn discover(&mut self) -> Result<Vec<(ToolSpec, String)>> {
        self.config.verify()?;
        if self.config.transport == "stdio" {
            #[cfg(windows)]
            // Rust's Windows process launcher runs .cmd/.bat via cmd with
            // AutoRun disabled and escapes each argument for batch parsing.
            let mut cmd = Command::new(windows_command(&self.config));
            #[cfg(not(windows))]
            let mut cmd = Command::new(&self.config.command);
            if let Some(path) = util::bundled_path() {
                cmd.env("PATH", path);
            }
            cmd.args(&self.config.args)
                .current_dir(&self.config.cwd)
                .envs(&self.config.env)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let mut child = cmd.spawn().context("failed to start MCP server")?;
            self.tree = Some(crate::command_process::ProcessTree::attach(&child)?);
            if let Some(stderr) = child.stderr.take() {
                let (path, log) = diagnostic_log();
                self.diagnostics.lock().unwrap().log_path = path;
                self.stderr_task = Some(tokio::spawn(capture_stderr(
                    stderr,
                    self.diagnostics.clone(),
                    sensitive_environment(&self.config),
                    log,
                )));
            }
            self.input = child.stdin.take();
            self.output = child.stdout.take().map(BufReader::new);
            self.child = Some(child);
        } else if self.config.transport == "sse" {
            self.open_legacy().await?;
        }
        let response=self.rpc("initialize",json!({"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"minicode","version":crate::VERSION}})).await?;
        self.protocol = response["protocolVersion"].as_str().map(str::to_string);
        self.server_version = response["serverInfo"]["version"]
            .as_str()
            .map(str::to_string);
        self.notify("notifications/initialized", json!({})).await?;
        let mut tools = vec![];
        let mut cursor = None;
        let mut seen = std::collections::HashSet::new();
        let mut cursors = std::collections::HashSet::new();
        loop {
            let mut params = json!({});
            if let Some(c) = &cursor {
                params["cursor"] = json!(c);
            }
            let result = self.rpc("tools/list", params).await?;
            for tool in result["tools"]
                .as_array()
                .context("invalid MCP tools list")?
            {
                let original = util::string(tool, "name")?.to_string();
                if !seen.insert(original.clone()) {
                    bail!("duplicate MCP tool")
                }
                let name = format!("mcp__{}__{}", self.config.name, original);
                tools.push((
                    ToolSpec {
                        name,
                        description: tool["description"].as_str().unwrap_or("").into(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or(json!({"type":"object"})),
                        requires_approval: true,
                    },
                    original,
                ));
                if tools.len() > 2048 {
                    bail!("MCP tool list exceeds the size limit")
                }
            }
            let next = result["nextCursor"].as_str().map(str::to_string);
            if next.is_none() {
                break;
            }
            if !cursors.insert(next.clone()) || cursors.len() > 2048 {
                bail!("MCP repeated pagination cursor")
            }
            cursor = next;
        }
        Ok(tools)
    }
    async fn open_legacy(&mut self) -> Result<()> {
        let mut r = self
            .client
            .get(&self.config.url)
            .header("Accept", "text/event-stream");
        for (k, v) in &self.config.headers {
            r = r.header(k, v);
        }
        let response = r.send().await?.error_for_status()?;
        let mut stream = response.bytes_stream();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        self.legacy_task = Some(tokio::spawn(async move {
            let mut decoder = crate::sse::Decoder::default();
            while let Some(Ok(chunk)) = stream.next().await {
                let Ok(events) = decoder.push(&chunk) else {
                    return;
                };
                for event in events {
                    if event.kind == "endpoint" {
                        if tx.send(json!({"endpoint":event.data})).await.is_err() {
                            return;
                        }
                    } else if let Ok(v) = serde_json::from_str::<Value>(&event.data)
                        && tx.send(v).await.is_err()
                    {
                        return;
                    }
                }
            }
        }));
        let first = tokio::time::timeout(Duration::from_secs_f64(self.config.timeout), rx.recv())
            .await?
            .context("MCP SSE endpoint missing")?;
        let url = reqwest::Url::parse(&self.config.url)?.join(util::string(&first, "endpoint")?)?;
        let original = reqwest::Url::parse(&self.config.url)?;
        if url.origin() != original.origin() {
            bail!("MCP SSE endpoint changes origin")
        }
        self.endpoint = Some(url.into());
        self.legacy_receiver = Some(rx);
        Ok(())
    }
    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let value = json!({"jsonrpc":"2.0","method":method,"params":params});
        if let Some(input) = self.input.as_mut() {
            input.write_all(format!("{value}\n").as_bytes()).await?;
            input.flush().await?;
        } else {
            self.request(self.endpoint.as_deref().unwrap_or(&self.config.url))
                .json(&value)
                .send()
                .await?
                .error_for_status()?;
        }
        Ok(())
    }
    pub async fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        self.sequence += 1;
        let id = self.sequence;
        let value = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let timeout = Duration::from_secs_f64(self.config.timeout);
        tokio::time::timeout(timeout, async {
            if let Some(input) = self.input.as_mut() {
                input.write_all(format!("{value}\n").as_bytes()).await?;
                input.flush().await?;
                loop {
                    let output = self.output.as_mut().context("MCP output closed")?;
                    let mut line = Vec::new();
                    loop {
                        let part = output.fill_buf().await?;
                        if part.is_empty() {
                            break;
                        }
                        let end = part
                            .iter()
                            .position(|b| *b == b'\n')
                            .map(|n| n + 1)
                            .unwrap_or(part.len());
                        if line.len() + end > 16_000_000 {
                            bail!("MCP response exceeds the size limit")
                        }
                        let finished = part[end - 1] == b'\n';
                        line.extend_from_slice(&part[..end]);
                        output.consume(end);
                        if finished {
                            break;
                        }
                    }
                    let n = line.len();
                    if n == 0 {
                        bail!("MCP server closed its output")
                    }
                    if line.len() > 16_000_000 {
                        bail!("MCP response exceeds the size limit")
                    }
                    let response: Value = serde_json::from_slice(&line)?;
                    if response["id"] == id
                        && (response.get("result").is_some() || response.get("error").is_some())
                    {
                        return rpc_result(response);
                    }
                    if response["method"] == "ping" && response.get("id").is_some() {
                        let reply = json!({"jsonrpc":"2.0","id":response["id"],"result":{}});
                        let input = self.input.as_mut().unwrap();
                        input.write_all(format!("{reply}\n").as_bytes()).await?;
                        input.flush().await?;
                    } else if response["method"].is_string() && response.get("id").is_some() {
                        let reply = json!({"jsonrpc":"2.0","id":response["id"],"error":{"code":-32601,"message":"Client does not support this method"}});
                        let input = self.input.as_mut().unwrap();
                        input.write_all(format!("{reply}\n").as_bytes()).await?;
                        input.flush().await?;
                    }
                }
            }
            let response = self
                .request(self.endpoint.as_deref().unwrap_or(&self.config.url))
                .json(&value)
                .send()
                .await?
                .error_for_status()?;
            if let Some(s) = response.headers().get("Mcp-Session-Id") {
                self.session = Some(s.to_str()?.to_string());
            }
            if let Some(rx) = self.legacy_receiver.as_mut() {
                loop {
                    let v = rx.recv().await.context("MCP SSE stream closed")?;
                    if v["id"] == id {
                        return rpc_result(v);
                    }
                }
            }
            let sse = response
                .headers()
                .get("content-type")
                .is_some_and(|s| s.to_str().unwrap_or("").starts_with("text/event-stream"));
            let mut stream = response.bytes_stream();
            let mut buffer = Vec::new();
            let mut decoder = crate::sse::Decoder::default();
            while let Some(bytes) = stream.next().await {
                let bytes = bytes?;
                if sse {
                    for event in decoder.push(&bytes)? {
                        if let Ok(v) = serde_json::from_str::<Value>(&event.data)
                            && v["id"] == id
                        {
                            return rpc_result(v);
                        }
                    }
                } else {
                    buffer.extend_from_slice(&bytes);
                    if buffer.len() > 16_000_000 {
                        bail!("MCP response exceeds the size limit")
                    }
                }
            }
            if sse {
                bail!("MCP SSE response missing result")
            }
            let response: Value = serde_json::from_slice(&buffer)?;
            if response["id"] != id {
                bail!("MCP response id does not match request");
            }
            rpc_result(response)
        })
        .await
        .context("MCP request timed out")?
    }
    pub async fn call(
        &mut self,
        name: &str,
        args: Value,
        attachments: &Attachments,
    ) -> Result<Outcome> {
        self.config.verify()?;
        let result = self
            .rpc("tools/call", json!({"name":name,"arguments":args}))
            .await?;
        let mut text = vec![];
        let mut images = vec![];
        if let Some(content) = result["content"].as_array() {
            for block in content {
                match block["type"].as_str() {
                    Some("text") => {
                        text.push(block["text"].as_str().unwrap_or_default().to_string())
                    }
                    Some("image") => {
                        let data = STANDARD.decode(util::string(block, "data")?)?;
                        images.push(attachments.import_bytes(
                            &data,
                            "MCP image",
                            util::string(block, "mimeType")?,
                        )?);
                    }
                    Some("resource") => {
                        if let Some(s) = block["resource"]["text"].as_str() {
                            text.push(s.into())
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(v) = result.get("structuredContent") {
            text.push(serde_json::to_string(v)?);
        }
        let output = text.join("\n");
        let mut o = Outcome::ok(util::prefix(&output, 50 * 1024));
        if o.output != output {
            o.full_output = Some(output);
        }
        o.success = result["isError"] != true;
        o.images = images;
        Ok(o)
    }
    pub async fn close(&mut self) {
        if let Some(task) = self.legacy_task.take() {
            task.abort();
        }
        if let Some(tree) = &self.tree {
            tree.shutdown().await;
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Some(mut task) = self.stderr_task.take()
            && tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
        {
            task.abort();
        }
        self.tree = None;
        self.input = None;
        self.output = None;
    }

    fn diagnostics(&self) -> (String, Option<PathBuf>) {
        let state = self.diagnostics.lock().unwrap();
        (state.tail(), state.log_path.clone())
    }
}
fn rpc_result(v: Value) -> Result<Value> {
    if let Some(error) = v.get("error") {
        bail!(
            "MCP error: {}",
            error["message"].as_str().unwrap_or("request failed")
        )
    }
    v.get("result").cloned().context("invalid MCP response")
}
#[derive(Default)]
pub struct McpRegistry {
    pub tools: HashMap<String, (ToolSpec, String, Arc<Mutex<Connector>>)>,
    pub timeouts: HashMap<String, Duration>,
    pub statuses: Vec<Value>,
    connectors: Vec<Arc<Mutex<Connector>>>,
}
impl McpRegistry {
    pub async fn prepare(servers: Vec<Server>) -> Self {
        let mut r = Self::default();
        let mut seen = std::collections::HashSet::new();
        let servers = servers
            .into_iter()
            .filter(|server| seen.insert(server.name.clone()))
            .collect::<Vec<_>>();
        let mut prepared = futures_util::stream::iter(servers).map(|server| async move {
            let mut c = match Connector::new(server.clone()) {
                Ok(c) => c,
                Err(_) => {
                    return (json!({"server":server.name,"plugin":server.plugin,"protocol":null,"server_version":null,"error":"MCP client configuration error"}), None, vec![]);
                }
            };
            let discovery = tokio::time::timeout(Duration::from_secs_f64(server.timeout), c.discover()).await.context("MCP discovery timed out").and_then(|r| r);
            match discovery {
                Ok(tools) => {
                    let (stderr_tail, log_path) = c.diagnostics();
                    (json!({"server":server.name,"plugin":server.plugin,"protocol":c.protocol,"server_version":c.server_version,"error":null,"stderr_tail":stderr_tail,"log_path":log_path}), Some(c), tools)
                }
                Err(e) => {
                    c.close().await;
                    let (stderr_tail, log_path) = c.diagnostics();
                    let mut error = if !server.configuration_error.is_empty() {
                        server.configuration_error.clone()
                    } else if server.local {
                        "MCP connection failed".to_string()
                    } else {
                        e.to_string()
                    };
                    if !stderr_tail.is_empty() {
                        error.push_str("\nMCP stderr:\n");
                        error.push_str(&stderr_tail);
                    }
                    (json!({"server":server.name,"plugin":server.plugin,"protocol":null,"server_version":null,"error":error,"stderr_tail":stderr_tail,"log_path":log_path}), None, vec![])
                }
            }
        }).buffered(4);
        while let Some((status, connector, tools)) = prepared.next().await {
            r.statuses.push(status);
            if let Some(connector) = connector {
                let timeout = Duration::from_secs_f64(connector.config.timeout);
                let connector = Arc::new(Mutex::new(connector));
                for (spec, original) in tools {
                    r.timeouts.entry(spec.name.clone()).or_insert(timeout);
                    r.tools
                        .entry(spec.name.clone())
                        .or_insert((spec, original, connector.clone()));
                }
                r.connectors.push(connector);
            }
        }
        r
    }
    pub async fn close(&self) {
        for c in &self.connectors {
            c.lock().await.close().await;
        }
    }
    pub async fn statuses_with_diagnostics(&self) -> Vec<Value> {
        let mut statuses = self.statuses.clone();
        for connector in &self.connectors {
            let connector = connector.lock().await;
            let (stderr_tail, log_path) = connector.diagnostics();
            if let Some(status) = statuses
                .iter_mut()
                .find(|s| s["server"] == connector.config.name)
            {
                status["stderr_tail"] = json!(stderr_tail);
                status["log_path"] = json!(log_path);
            }
        }
        statuses
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::redact_stderr;

    #[test]
    fn secrets_are_redacted_across_stderr_read_boundaries() {
        let secrets = vec![b"token-long".to_vec(), b"token".to_vec()];
        let mut pending = b"before tok".to_vec();
        let mut output = redact_stderr(&mut pending, &secrets, false);
        pending.extend_from_slice(b"en-");
        output.extend(redact_stderr(&mut pending, &secrets, false));
        pending.extend_from_slice(b"long after token");
        output.extend(redact_stderr(&mut pending, &secrets, false));
        output.extend(redact_stderr(&mut pending, &secrets, true));
        assert_eq!(output, b"before [redacted] after [redacted]");
        assert!(pending.is_empty());
    }
}
