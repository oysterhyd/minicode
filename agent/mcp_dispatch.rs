use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout},
    sync::oneshot,
};

type Reply = std::result::Result<Value, String>;
pub(crate) struct Pending {
    replies: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    live: AtomicBool,
}
impl Default for Pending {
    fn default() -> Self {
        Self {
            replies: Default::default(),
            live: AtomicBool::new(true),
        }
    }
}
pub(crate) struct Request {
    id: u64,
    pending: Arc<Pending>,
}
impl Drop for Request {
    fn drop(&mut self) {
        self.pending.replies.lock().unwrap().remove(&self.id);
    }
}
impl Pending {
    pub fn live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }
    pub fn register(self: &Arc<Self>, id: u64) -> Result<(Request, oneshot::Receiver<Reply>)> {
        let mut replies = self.replies.lock().unwrap();
        if !self.live() {
            bail!("MCP connection closed; retry after reconnect");
        }
        let (tx, rx) = oneshot::channel();
        replies.insert(id, tx);
        Ok((
            Request {
                id,
                pending: self.clone(),
            },
            rx,
        ))
    }
    pub fn dispatch(&self, response: Value) -> Option<Value> {
        if response["method"].is_string() {
            if response.get("id").is_some() {
                return Some(if response["method"] == "ping" {
                    json!({"jsonrpc":"2.0","id":response["id"],"result":{}})
                } else {
                    json!({"jsonrpc":"2.0","id":response["id"],"error":{"code":-32601,"message":"Client does not support this method"}})
                });
            }
        } else if (response.get("result").is_some() || response.get("error").is_some())
            && let Some(id) = response["id"].as_u64()
            && let Some(tx) = self.replies.lock().unwrap().remove(&id)
        {
            let _ = tx.send(Ok(response));
        }
        None
    }
    pub fn close(&self, error: &str) {
        let mut replies = self.replies.lock().unwrap();
        self.live.store(false, Ordering::Release);
        for (_, tx) in replies.drain() {
            let _ = tx.send(Err(error.into()));
        }
    }
}
struct Writing<'a> {
    pending: &'a Pending,
    complete: bool,
}
impl Drop for Writing<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.pending
                .close("MCP frame write interrupted; reconnect required");
        }
    }
}
pub(crate) async fn write(
    input: &tokio::sync::Mutex<ChildStdin>,
    value: &Value,
    pending: &Pending,
) -> Result<()> {
    let mut input = input.lock().await;
    let mut writing = Writing {
        pending,
        complete: false,
    };
    input.write_all(format!("{value}\n").as_bytes()).await?;
    input.flush().await?;
    writing.complete = true;
    Ok(())
}
pub(crate) async fn read_stdio(
    output: ChildStdout,
    input: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Arc<Pending>,
) {
    // This reader owns the frame buffer for the entire connection. Dropping a
    // caller never drops a partial frame or consumes another caller's reply.
    let result: Result<()> = async {
        let mut output = BufReader::new(output);
        loop {
            let mut line = Vec::new();
            loop {
                let part = output.fill_buf().await?;
                if part.is_empty() {
                    bail!("MCP server closed its output");
                }
                let count = part
                    .iter()
                    .position(|b| *b == b'\n')
                    .map(|n| n + 1)
                    .unwrap_or(part.len());
                if line.len() + count > 16_000_000 {
                    bail!("MCP response exceeds the size limit");
                }
                let complete = part[count - 1] == b'\n';
                line.extend_from_slice(&part[..count]);
                output.consume(count);
                if complete {
                    break;
                }
            }
            let response = serde_json::from_slice(&line).context("invalid MCP response frame")?;
            if let Some(reply) = pending.dispatch(response) {
                write(&input, &reply, &pending).await?;
            }
        }
    }
    .await;
    pending.close(&result.unwrap_err().to_string());
}
