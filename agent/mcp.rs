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
use std::{collections::HashMap, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

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
}
impl Connector {
    pub fn new(config: Server) -> Result<Self> {
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
            let mut cmd = Command::new(&self.config.command);
            if let Some(path) = util::bundled_path() {
                cmd.env("PATH", path);
            }
            cmd.args(&self.config.args)
                .current_dir(&self.config.cwd)
                .envs(&self.config.env)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let mut child = cmd.spawn().context("failed to start MCP server")?;
            self.tree = Some(crate::command_process::ProcessTree::attach(&child)?);
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
            if cursor == next {
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
            let mut pending = Vec::new();
            let mut data = String::new();
            let mut event = String::new();
            while let Some(Ok(chunk)) = stream.next().await {
                pending.extend_from_slice(&chunk);
                if pending.len() > 16_000_000 {
                    break;
                }
                while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                    let Ok(line) = String::from_utf8(pending.drain(..=end).collect()) else {
                        return;
                    };
                    let line = line.trim_end_matches(['\r', '\n']);
                    if line.is_empty() {
                        if event == "endpoint" {
                            if tx.send(json!({"endpoint":data})).await.is_err() {
                                return;
                            }
                        } else if let Ok(v) = serde_json::from_str::<Value>(&data)
                            && tx.send(v).await.is_err()
                        {
                            return;
                        }
                        data.clear();
                        event.clear();
                    } else if let Some(s) = line.strip_prefix("data:") {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(s.trim_start());
                        if data.len() > 16_000_000 {
                            return;
                        }
                    } else if let Some(s) = line.strip_prefix("event:") {
                        event = s.trim().into();
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
            let mut event_data = String::new();
            while let Some(bytes) = stream.next().await {
                buffer.extend_from_slice(&bytes?);
                if buffer.len() > 16_000_000 {
                    bail!("MCP response exceeds the size limit")
                }
                if sse {
                    while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                        let line = String::from_utf8(buffer.drain(..=end).collect())?;
                        let line = line.trim_end_matches(['\r', '\n']);
                        if line.is_empty() {
                            if let Ok(v) = serde_json::from_str::<Value>(&event_data)
                                && v["id"] == id
                            {
                                return rpc_result(v);
                            }
                            event_data.clear();
                        } else if let Some(s) = line.strip_prefix("data:") {
                            if !event_data.is_empty() {
                                event_data.push('\n');
                            }
                            event_data.push_str(s.trim_start());
                            if event_data.len() > 16_000_000 {
                                bail!("MCP response exceeds the size limit")
                            }
                        }
                    }
                }
            }
            if sse {
                bail!("MCP SSE response missing result")
            }
            rpc_result(serde_json::from_slice(&buffer)?)
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
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.tree = None;
        self.input = None;
        self.output = None;
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
    pub statuses: Vec<Value>,
}
impl McpRegistry {
    pub async fn prepare(servers: Vec<Server>) -> Self {
        let mut r = Self::default();
        for server in servers {
            let mut c = match Connector::new(server.clone()) {
                Ok(c) => c,
                Err(_) => {
                    r.statuses.push(json!({"server":server.name,"plugin":server.plugin,"protocol":null,"server_version":null,"error":"MCP client configuration error"}));
                    continue;
                }
            };
            match c.discover().await {
                Ok(tools) => {
                    r.statuses.push(json!({"server":server.name,"plugin":server.plugin,"protocol":c.protocol,"server_version":c.server_version,"error":null}));
                    let c = Arc::new(Mutex::new(c));
                    for (spec, original) in tools {
                        if !r.tools.contains_key(&spec.name) {
                            r.tools
                                .insert(spec.name.clone(), (spec, original, c.clone()));
                        }
                    }
                }
                Err(e) => {
                    c.close().await;
                    r.statuses.push(json!({"server":server.name,"plugin":server.plugin,"protocol":null,"server_version":null,"error":if server.local{"MCP connection failed".to_string()}else{e.to_string()}}));
                }
            }
        }
        r
    }
    pub async fn close(&self) {
        for (_, _, c) in self.tools.values() {
            c.lock().await.close().await;
        }
    }
}
