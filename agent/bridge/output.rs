use crate::tools::Sink;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::Write,
    sync::{Arc, Condvar, Mutex},
};

pub const DEFAULT_MAX_DELTAS: usize = 512;
pub const DEFAULT_MAX_CRITICAL: usize = 4096;
pub const MAX_COALESCED_TEXT_BYTES: usize = 65_536;

pub(super) fn is_transient_stream_event(value: &Value) -> bool {
    value.get("id").is_none()
        && matches!(
            value["event"].as_str(),
            Some("text_delta" | "reasoning_delta" | "usage_delta" | "log")
        )
}

#[derive(Default)]
struct QueueState {
    queue: VecDeque<Value>,
    transient_count: usize,
    critical_count: usize,
    dropped_transient: u64,
    closed: bool,
}

impl QueueState {
    fn same_stream_target(back: &Value, incoming: &Value) -> bool {
        back.get("id").is_none()
            && back["event"] == incoming["event"]
            && back.get("clientKey") == incoming.get("clientKey")
            && back.get("sessionId") == incoming.get("sessionId")
            && back.get("runId") == incoming.get("runId")
    }

    fn try_coalesce(&mut self, incoming: &Value) -> bool {
        let Some(back) = self.queue.back_mut() else {
            return false;
        };
        if !Self::same_stream_target(back, incoming) {
            return false;
        }
        match incoming["event"].as_str() {
            Some("text_delta" | "reasoning_delta") => {
                let (Some(existing), Some(addition)) =
                    (back["text"].as_str(), incoming["text"].as_str())
                else {
                    return false;
                };
                if existing.len() + addition.len() > MAX_COALESCED_TEXT_BYTES {
                    return false;
                }
                let mut merged = String::with_capacity(existing.len() + addition.len());
                merged.push_str(existing);
                merged.push_str(addition);
                back["text"] = Value::String(merged);
                true
            }
            Some("usage_delta") => {
                back["usage"] = incoming["usage"].clone();
                true
            }
            _ => false,
        }
    }

    fn shed_oldest_transient(&mut self) -> bool {
        let target = self
            .queue
            .iter()
            .position(|v| {
                v.get("id").is_none() && matches!(v["event"].as_str(), Some("log" | "usage_delta"))
            })
            .or_else(|| self.queue.iter().position(is_transient_stream_event));
        if let Some(pos) = target {
            self.queue.remove(pos);
            self.transient_count -= 1;
            self.dropped_transient += 1;
            true
        } else {
            false
        }
    }

    fn take_warning_if_ready(
        &mut self,
        flushing_critical: bool,
        max_deltas: usize,
    ) -> Option<Value> {
        if self.dropped_transient == 0 {
            return None;
        }
        if flushing_critical || self.transient_count <= max_deltas / 2 || self.queue.is_empty() {
            let dropped = std::mem::take(&mut self.dropped_transient);
            return Some(json!({
                "event": "warning",
                "text": format!("bridge stdout backpressure: dropped {dropped} transient stream frames"),
            }));
        }
        None
    }
}

struct QueueSender {
    shared: Arc<(Mutex<QueueState>, Condvar)>,
    max_deltas: usize,
    max_critical: usize,
}

impl QueueSender {
    fn send(&self, value: Value) {
        let (lock, cvar) = &*self.shared;
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.closed {
            return;
        }
        if is_transient_stream_event(&value) {
            if guard.try_coalesce(&value) {
                return;
            }
            if self.max_deltas == 0 {
                guard.dropped_transient += 1;
                return;
            }
            while guard.transient_count >= self.max_deltas {
                if !guard.shed_oldest_transient() {
                    break;
                }
            }
            guard.transient_count += 1;
            guard.queue.push_back(value);
            cvar.notify_one();
            return;
        }
        while !guard.closed && self.max_critical > 0 && guard.critical_count >= self.max_critical {
            guard = cvar
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if guard.closed {
            return;
        }
        guard.critical_count += 1;
        guard.queue.push_back(value);
        cvar.notify_one();
    }
}

impl Drop for QueueSender {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.shared;
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.closed = true;
        cvar.notify_all();
    }
}

fn write_frame(out: &mut std::io::BufWriter<impl Write>, value: &Value) -> bool {
    serde_json::to_writer(&mut *out, value).is_ok()
        && out.write_all(b"\n").is_ok()
        && out.flush().is_ok()
}

// The producer enqueues non-blocking deltas with coalescing and bounded shedding,
// while critical replies/events are never dropped and drain in strict FIFO order.
pub(super) fn output_writer(
    output: impl Write + Send + 'static,
) -> (Sink, tokio::task::JoinHandle<()>) {
    output_writer_with_limits(output, DEFAULT_MAX_DELTAS, DEFAULT_MAX_CRITICAL)
}

pub(super) fn output_writer_with_limits(
    output: impl Write + Send + 'static,
    max_deltas: usize,
    max_critical: usize,
) -> (Sink, tokio::task::JoinHandle<()>) {
    let shared = Arc::new((Mutex::new(QueueState::default()), Condvar::new()));
    let writer_shared = shared.clone();
    let writer = tokio::task::spawn_blocking(move || {
        let mut out = std::io::BufWriter::new(output);
        let (lock, cvar) = &*writer_shared;
        loop {
            let (frame, warning) = {
                let mut guard = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while guard.queue.is_empty() && !guard.closed {
                    guard = cvar
                        .wait(guard)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                if let Some(frame) = guard.queue.pop_front() {
                    let transient = is_transient_stream_event(&frame);
                    if transient {
                        guard.transient_count -= 1;
                    } else {
                        guard.critical_count -= 1;
                        cvar.notify_all();
                    }
                    let warning = guard.take_warning_if_ready(!transient, max_deltas);
                    (Some(frame), warning)
                } else {
                    let warning = guard.take_warning_if_ready(true, max_deltas);
                    (None, warning)
                }
            };
            if let Some(warning) = warning
                && !write_frame(&mut out, &warning)
            {
                break;
            }
            match frame {
                Some(frame) => {
                    if !write_frame(&mut out, &frame) {
                        break;
                    }
                }
                None => break,
            }
        }
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.closed = true;
        cvar.notify_all();
    });
    let sender = QueueSender {
        shared,
        max_deltas,
        max_critical,
    };
    (
        Arc::new(move |value| {
            sender.send(value);
        }),
        writer,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct SlowOutput {
        delay: Duration,
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for SlowOutput {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.delay);
            self.bytes.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn slow_stdout_is_queued_and_drained_in_order() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (sink, writer) = output_writer(SlowOutput {
            delay: Duration::from_millis(150),
            bytes: bytes.clone(),
        });
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
    async fn slow_stdout_coalesces_adjacent_deltas_and_preserves_rpc_replies() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (sink, writer) = output_writer_with_limits(
            SlowOutput {
                delay: Duration::from_millis(40),
                bytes: bytes.clone(),
            },
            16,
            64,
        );
        // Kick off writer delay on the first critical event, then enqueue adjacent deltas while it writes.
        sink(json!({"event":"run_started","sessionId":"s1","runId":"r1"}));
        for chunk in ["hel", "lo ", "world"] {
            sink(json!({"event":"text_delta","sessionId":"s1","runId":"r1","text":chunk}));
        }
        for i in 1..=4 {
            sink(
                json!({"event":"usage_delta","sessionId":"s1","runId":"r1","usage":{"input_tokens":i}}),
            );
        }
        sink(json!({"id":7,"result":{"ok":true}}));
        drop(sink);
        writer.await.unwrap();

        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let frames = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let text: String = frames
            .iter()
            .filter(|f| f["event"] == "text_delta")
            .filter_map(|f| f["text"].as_str())
            .collect();
        assert_eq!(text, "hello world");
        let last_usage = frames
            .iter()
            .rfind(|f| f["event"] == "usage_delta")
            .unwrap();
        assert_eq!(last_usage["usage"]["input_tokens"], 4);
        assert_eq!(
            frames.last().unwrap(),
            &json!({"id":7,"result":{"ok":true}})
        );
    }

    #[tokio::test]
    async fn slow_stdout_bounds_backlog_under_flood_and_emits_warning() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (sink, writer) = output_writer_with_limits(
            SlowOutput {
                delay: Duration::from_millis(25),
                bytes: bytes.clone(),
            },
            8,
            64,
        );
        sink(json!({"event":"run_started","sessionId":"s1","runId":"r1"}));
        for i in 0..120 {
            sink(json!({"event":"log","text":format!("log-{i}")}));
            sink(
                json!({"event":"text_delta","sessionId":"s1","runId":"r1","text":format!("chunk-{i} ")}),
            );
            if i == 50 {
                sink(json!({"event":"approval","sessionId":"s1","runId":"r1","approvalId":"a1"}));
            }
        }
        sink(
            json!({"event":"run_done","sessionId":"s1","runId":"r1","result":{"exit_reason":"completed"}}),
        );
        sink(json!({"id":99,"result":true}));
        drop(sink);
        writer.await.unwrap();

        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let frames = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(
            frames.len() < 40,
            "expected bounded output, got {}",
            frames.len()
        );
        assert!(
            frames
                .iter()
                .any(|f| f["event"] == "approval" && f["approvalId"] == "a1")
        );
        assert!(frames.iter().any(|f| f["event"] == "run_done"));
        assert!(frames.iter().any(|f| f == &json!({"id":99,"result":true})));
        assert!(
            frames.iter().any(|f| {
                f["event"] == "warning"
                    && f["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("bridge stdout backpressure"))
            }),
            "expected backpressure warning in {frames:?}"
        );
    }
}
