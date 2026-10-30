use crate::{
    models::Outcome,
    store::Store,
    tools::{self, CommandCapture, Sink, ToolContext},
    util,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Completion {
    outcome: Option<Outcome>,
    published: bool,
}
struct Job {
    command: String,
    cancel: CancellationToken,
    capture: Arc<CommandCapture>,
    completion: Arc<Mutex<Completion>>,
    cursor: u64,
    handle: Option<tokio::task::JoinHandle<()>>,
    finished: CancellationToken,
}
/// Live process ownership belongs to a session, independently of any turn.
#[derive(Default)]
pub struct Jobs(Mutex<HashMap<String, Job>>);

fn publish(
    id: &str,
    command: &str,
    completion: &Mutex<Completion>,
    store: &Store,
    session: &str,
    sink: &Sink,
) -> Result<()> {
    let mut state = completion.lock().unwrap();
    if state.published {
        return Ok(());
    }
    let Some(outcome) = &mut state.outcome else {
        return Ok(());
    };
    if outcome.output.len() > 4000 && outcome.artifact_id.is_none() {
        outcome.artifact_id = Some(store.spill(session, "background_output", &outcome.output)?);
    }
    let event = store.event(session, "background_job_completed", &json!({
        "job_id":id, "command":command, "status":if outcome.success {"completed"} else {"failed"},
        "exit_code":outcome.exit_code, "output":util::prefix(&outcome.output,4000),
        "error":outcome.error, "artifact_id":outcome.artifact_id, "truncated":outcome.truncated
    }))?;
    state.published = true;
    drop(state);
    sink(json!({"event":"agent_event","sessionId":session,"item":event}));
    Ok(())
}
impl Jobs {
    pub fn has_jobs(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }
    pub fn contains(&self, id: &str) -> bool {
        self.0.lock().unwrap().contains_key(id)
    }

    pub fn start(
        &self,
        name: String,
        command: String,
        cwd: PathBuf,
        timeout: f64,
        mut ctx: ToolContext,
        sink: Sink,
    ) -> Result<String> {
        let mut jobs = self.0.lock().unwrap();
        if ctx.cancel.is_cancelled() {
            bail!("turn cancelled");
        }
        // Bound retained captures and process count for long-lived sessions.
        if jobs.len() >= 128 {
            bail!("本会话后台任务已达 128 个，请关闭会话后重试");
        }
        let id = format!("bg_{}", util::id());
        let mut capture = CommandCapture::new()?;
        capture.background = true;
        let capture = Arc::new(capture);
        let completion = Arc::new(Mutex::new(Completion::default()));
        let cancel = CancellationToken::new();
        let finished = CancellationToken::new();
        let finished_guard = finished.clone().drop_guard();
        ctx.cancel = cancel.clone();
        ctx.deadline = None;
        ctx.output = None;
        let event = ctx.store.event(
            &ctx.session,
            "background_job_started",
            &json!({"job_id":id,"command":command,"cwd":cwd}),
        )?;
        sink(json!({"event":"agent_event","sessionId":ctx.session,"item":event}));
        let (task_id, task_command, task_capture, task_completion) = (
            id.clone(),
            command.clone(),
            capture.clone(),
            completion.clone(),
        );
        let handle = tokio::spawn(async move {
            let _finished = finished_guard;
            use futures_util::FutureExt;
            let outcome = match std::panic::AssertUnwindSafe(tools::run_command_captured(
                &name,
                &task_command,
                &cwd,
                timeout,
                &ctx,
                task_capture,
            ))
            .catch_unwind()
            .await
            {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(error)) => Outcome::failure(error.to_string()),
                Err(_) => Outcome::failure("background command panicked"),
            };
            task_completion.lock().unwrap().outcome = Some(outcome);
            // A failed write is retried by flush before the next turn/close.
            let _ = publish(
                &task_id,
                &task_command,
                &task_completion,
                &ctx.store,
                &ctx.session,
                &sink,
            );
        });
        jobs.insert(
            id.clone(),
            Job {
                command,
                cancel,
                capture,
                completion,
                cursor: 0,
                handle: Some(handle),
                finished,
            },
        );
        Ok(id)
    }
    pub fn flush(&self, store: &Store, session: &str, sink: &Sink) -> Result<()> {
        for (id, job) in self.0.lock().unwrap().iter() {
            publish(id, &job.command, &job.completion, store, session, sink)?;
        }
        Ok(())
    }
    pub fn output(&self, args: &Value) -> Result<Outcome> {
        let mut jobs = self.0.lock().unwrap();
        let id = util::string(args, "job_id")?;
        let job = jobs.get_mut(id).context("后台任务不存在或属于其他会话")?;
        let state = job.completion.lock().unwrap();
        let mut file = job.capture.file.reopen()?;
        let total = file.metadata()?.len();
        let cursor = args["cursor"].as_u64().unwrap_or(job.cursor);
        if cursor > total {
            bail!("后台输出游标超出范围");
        }
        let limit = args["limit"].as_u64().unwrap_or(16000).clamp(4, 65536);
        file.seek(SeekFrom::Start(cursor))?;
        let mut bytes = vec![];
        file.take(limit).read_to_end(&mut bytes)?;
        // Leave an incomplete trailing UTF-8 scalar for the next page/read.
        if let Err(error) = std::str::from_utf8(&bytes)
            && error.error_len().is_none()
            && (state.outcome.is_none() || cursor + (bytes.len() as u64) < total)
        {
            bytes.truncate(error.valid_up_to());
        }
        let next = cursor + bytes.len() as u64;
        job.cursor = next;
        let outcome = state.outcome.as_ref();
        Ok(Outcome::ok(json!({"job_id":id,"pid":job.capture.pid.load(Ordering::Acquire),
            "status":outcome.map(|o| if o.success {"completed"} else {"failed"}).unwrap_or("running"),
            "cursor":cursor,"next_cursor":next,"total_bytes":total,"truncated":next<total,
            "output":tools::decode(&bytes),"exit_code":outcome.and_then(|o|o.exit_code),
            "error":outcome.and_then(|o|o.error.as_ref()),"artifact_id":outcome.and_then(|o|o.artifact_id.as_ref())
        }).to_string()))
    }
    pub async fn kill(&self, id: &str) -> Result<Outcome> {
        let (handle, finished) = {
            let mut jobs = self.0.lock().unwrap();
            let job = jobs.get_mut(id).context("后台任务不存在或属于其他会话")?;
            job.cancel.cancel();
            (job.handle.take(), job.finished.clone())
        };
        if let Some(handle) = handle {
            handle.await?;
        }
        finished.cancelled().await;
        Ok(Outcome::ok(format!(
            "后台任务 {id} 已停止（重复终止无副作用）"
        )))
    }
    pub fn cancel(&self) {
        for job in self.0.lock().unwrap().values() {
            job.cancel.cancel();
        }
    }
    pub async fn close(&self) -> Result<()> {
        self.cancel();
        let handles = self
            .0
            .lock()
            .unwrap()
            .values_mut()
            .map(|j| (j.handle.take(), j.finished.clone()))
            .collect::<Vec<_>>();
        let mut failures = vec![];
        for (handle, finished) in handles {
            if let Some(handle) = handle
                && let Err(error) = handle.await
            {
                failures.push(error.to_string());
            }
            finished.cancelled().await;
        }
        if !failures.is_empty() {
            bail!("{}", failures.join("; "));
        }
        Ok(())
    }
}
impl Drop for Jobs {
    fn drop(&mut self) {
        self.cancel();
    }
}
