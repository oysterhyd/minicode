use anyhow::{Context, Result, bail};
use serde::Deserialize;
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

fn normalize_id(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.len() > 128 {
        return None;
    }
    if raw.starts_with('"') {
        let text = serde_json::from_str::<String>(raw).ok()?;
        let id = text.parse::<u64>().ok()?;
        return (id.to_string() == text).then_some(id);
    }
    let (mantissa, exponent) = if let Some(index) = raw.find(['e', 'E']) {
        (&raw[..index], raw[index + 1..].parse::<i64>().ok()?)
    } else {
        (raw, 0)
    };
    let (whole, fraction) = if let Some((whole, fraction)) = mantissa.split_once('.') {
        if fraction.is_empty() {
            return None;
        }
        (whole, fraction)
    } else {
        (mantissa, "")
    };
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    let scale = exponent.checked_sub(fraction.len() as i64)?;
    let (digits, zeros) = if scale < 0 {
        let places = usize::try_from(scale.unsigned_abs()).ok()?;
        if places > digits.len() {
            return None;
        }
        let split = digits.len() - places;
        if !digits[split..].bytes().all(|b| b == b'0') {
            return None;
        }
        (&digits[..split], 0)
    } else {
        (digits, usize::try_from(scale).ok()?)
    };
    if digits.len().checked_add(zeros)? > 20 {
        return None;
    }
    let mut id = digits.parse::<u64>().ok()?;
    for _ in 0..zeros {
        id = id.checked_mul(10)?;
    }
    Some(id)
}
#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    id: Option<&'a serde_json::value::RawValue>,
}
/// Normalize only response IDs, using their original decimal representation.
/// Parsing via f64 first could round a fractional or adjacent ID into ours.
pub(crate) fn decode(bytes: &[u8]) -> Result<Value> {
    let envelope: Envelope<'_> = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    if !value["method"].is_string()
        && let Some(raw) = envelope.id
    {
        value["id"] = normalize_id(raw.get())
            .map(Value::from)
            .unwrap_or(Value::Null);
    }
    Ok(value)
}
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
            let response = decode(&line).context("invalid MCP response frame")?;
            if let Some(reply) = pending.dispatch(response) {
                write(&input, &reply, &pending).await?;
            }
        }
    }
    .await;
    pending.close(&result.unwrap_err().to_string());
}

#[cfg(test)]
mod id_tests {
    use super::*;
    #[test]
    fn mc5_ids_normalize_exact_integer_values_without_float_rounding() {
        for (raw, expected) in [
            ("1", Some(1)),
            ("1.0", Some(1)),
            ("1.0000000000000000000000000", Some(1)),
            ("1e0", Some(1)),
            ("10e-1", Some(1)),
            ("0.01e2", Some(1)),
            (r#""1""#, Some(1)),
            (r#""\u0031""#, Some(1)),
            ("0", Some(0)),
            ("9007199254740993.0", Some(9007199254740993)),
            ("18446744073709551615.0", Some(u64::MAX)),
            (r#""18446744073709551615""#, Some(u64::MAX)),
            ("1.0000000000000001", None),
            ("0.99999999999999999", None),
            ("9007199254740992.1", None),
            ("18446744073709551615.1", None),
            ("18446744073709551616", None),
            (r#""01""#, None),
            (r#""+1""#, None),
            (r#""1.0""#, None),
            (r#""1e0""#, None),
            (r#"" 1 ""#, None),
            ("-1", None),
            ("1.5", None),
            ("true", None),
            ("null", None),
            ("[]", None),
            ("{}", None),
            ("", None),
            ("1e9223372036854775807", None),
            ("1.0e-9223372036854775808", None),
        ] {
            assert_eq!(normalize_id(raw), expected, "{raw}");
        }
    }
    #[test]
    fn mc5_decode_preserves_raw_decimal_identity_and_server_request_ids() {
        for raw in [
            "1.0000000000000001",
            "0.99999999999999999",
            "9007199254740992.1",
        ] {
            let frame = format!(r#"{{"jsonrpc":"2.0","id":{raw},"result":{{}}}}"#);
            assert!(decode(frame.as_bytes()).unwrap()["id"].is_null());
        }
        let frame = br#"{"jsonrpc":"2.0","id":18446744073709551615.0,"result":{}}"#;
        assert_eq!(decode(frame).unwrap()["id"].as_u64(), Some(u64::MAX));
        assert_eq!(
            decode(br#"{"id":"1.0","method":"ping"}"#).unwrap()["id"],
            "1.0"
        );
        assert!(decode(br#"{"id":1,"id":2,"result":{}}"#).is_err());
    }
}
