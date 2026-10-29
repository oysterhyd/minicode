use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

type Completion = Box<dyn FnOnce(std::result::Result<(), String>) + Send>;
type Operation = Box<dyn FnOnce(&Connection, Option<&str>) -> Completion + Send>;
enum Command {
    Write { operation: Operation, durable: bool },
    Flush(mpsc::Sender<Result<()>>),
}
pub(crate) struct Writer {
    sender: Option<mpsc::Sender<Command>>,
    worker: Option<JoinHandle<()>>,
    pending: Arc<AtomicUsize>,
}
impl Writer {
    pub fn new(connection: Connection) -> Result<Self> {
        let millis = std::env::var("MINICODE_STORE_BATCH_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n <= 1000)
            .unwrap_or(10);
        Self::with_batch(connection, Duration::from_millis(millis))
    }
    fn with_batch(connection: Connection, batch: Duration) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let count = pending.clone();
        let worker = thread::Builder::new()
            .name("minicode-store".into())
            .spawn(move || {
                let mut state = State {
                    connection,
                    pending: count,
                    open: false,
                    writes: 0,
                    previews: 0,
                    failure: None,
                    completions: vec![],
                };
                loop {
                    let command = if state.open {
                        receiver.recv_timeout(batch)
                    } else {
                        receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    match command {
                        Ok(command) => {
                            if state.apply(command) || batch.is_zero() {
                                while state.open && state.failure.is_none() && state.writes < 64 {
                                    match receiver.try_recv() {
                                        Ok(command) => {
                                            state.apply(command);
                                        }
                                        Err(_) => break,
                                    }
                                }
                                state.commit();
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => state.commit(),
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            state.commit();
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            pending,
        })
    }
    pub fn call<T: Send + 'static>(
        &self,
        durable: bool,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = mpsc::channel();
        let operation: Operation = Box::new(move |connection, failure| {
            let result = (|| {
                if let Some(failure) = failure {
                    bail!("database writer failed: {failure}");
                }
                connection.execute_batch("SAVEPOINT minicode_write")?;
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(connection)))
                        .unwrap_or_else(|panic| {
                            Err(anyhow::anyhow!(
                                "database operation panicked: {}",
                                crate::util::panic_message(&*panic)
                            ))
                        });
                if result.is_err() {
                    connection.execute_batch("ROLLBACK TO minicode_write")?;
                }
                connection.execute_batch("RELEASE minicode_write")?;
                result
            })();
            Box::new(move |commit| {
                let _ = sender.send(commit.map_err(anyhow::Error::msg).and(result));
            })
        });
        if !durable {
            self.pending.fetch_add(1, Ordering::SeqCst);
        }
        if self
            .sender
            .as_ref()
            .context("database writer closed")?
            .send(Command::Write { operation, durable })
            .is_err()
        {
            if !durable {
                self.pending.fetch_sub(1, Ordering::SeqCst);
            }
            bail!("database writer stopped");
        }
        wait(receiver)
    }
    pub fn flush(&self) -> Result<()> {
        if self.pending.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel();
        self.sender
            .as_ref()
            .context("database writer closed")?
            .send(Command::Flush(sender))
            .map_err(|_| anyhow::anyhow!("database writer stopped"))?;
        wait(receiver)
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take()
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
    }
}
fn wait<T>(receiver: mpsc::Receiver<Result<T>>) -> Result<T> {
    let receive = || receiver.recv().context("database writer stopped")?;
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(receive)
    } else {
        receive()
    }
}
struct State {
    connection: Connection,
    pending: Arc<AtomicUsize>,
    open: bool,
    writes: usize,
    previews: usize,
    failure: Option<String>,
    completions: Vec<Completion>,
}
impl State {
    fn apply(&mut self, command: Command) -> bool {
        match command {
            Command::Write { operation, durable } => {
                let mut begin_error = None;
                if !self.open && self.failure.is_none() {
                    match self.connection.execute_batch("BEGIN IMMEDIATE") {
                        Ok(()) => self.open = true,
                        Err(error) => begin_error = Some(error.to_string()),
                    }
                }
                let completion = operation(
                    &self.connection,
                    self.failure.as_deref().or(begin_error.as_deref()),
                );
                self.writes += 1;
                if durable {
                    self.completions.push(completion);
                } else {
                    self.previews += 1;
                    completion(Ok(()));
                }
                durable || self.writes >= 64 || begin_error.is_some() || self.failure.is_some()
            }
            Command::Flush(sender) => {
                self.completions.push(Box::new(move |result| {
                    let _ = sender.send(result.map_err(anyhow::Error::msg));
                }));
                true
            }
        }
    }
    fn commit(&mut self) {
        if self.open {
            if let Err(error) = self.connection.execute_batch("COMMIT") {
                self.failure = Some(error.to_string());
                let _ = self.connection.execute_batch("ROLLBACK");
            }
            self.open = false;
        }
        self.pending.fetch_sub(self.previews, Ordering::SeqCst);
        self.previews = 0;
        self.writes = 0;
        for completion in self.completions.drain(..) {
            completion(self.failure.clone().map_or(Ok(()), Err));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Writer, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE entries(value INTEGER)").unwrap();
        let reader = Connection::open(path).unwrap();
        let writer = Writer::with_batch(connection, Duration::from_secs(60)).unwrap();
        (dir, writer, reader)
    }
    #[test]
    fn s2_progress_is_batched_and_checkpoint_waits_for_commit() {
        let (_dir, writer, reader) = fixture();
        for value in 0..10 {
            writer
                .call(false, move |c| {
                    c.execute("INSERT INTO entries VALUES(?)", [value])?;
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(
            reader
                .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let mode = writer
            .call(true, |c| {
                assert_eq!(thread::current().name(), Some("minicode-store"));
                c.execute("INSERT INTO entries VALUES(10)", [])?;
                Ok(c.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))?)
            })
            .unwrap();
        assert_eq!(mode, 2, "critical checkpoints keep FULL durability");
        assert_eq!(
            reader
                .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            11
        );
    }
    #[test]
    fn s2_failed_job_rolls_back_without_losing_other_progress() {
        let (_dir, writer, reader) = fixture();
        writer
            .call(false, |c| {
                c.execute("INSERT INTO entries VALUES(1)", [])?;
                Ok(())
            })
            .unwrap();
        let failure: Result<()> = writer.call(true, |c| {
            c.execute("INSERT INTO entries VALUES(2)", [])?;
            bail!("expected rollback")
        });
        assert!(failure.is_err());
        writer
            .call(true, |c| {
                c.execute("INSERT INTO entries VALUES(3)", [])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            reader
                .query_row("SELECT group_concat(value) FROM entries", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "1,3"
        );
    }
    #[test]
    fn s2_drop_flushes_progress_and_begin_errors_allow_retry() {
        let (_dir, writer, reader) = fixture();
        writer
            .call(true, |c| {
                c.busy_timeout(Duration::from_millis(1))?;
                Ok(())
            })
            .unwrap();
        reader.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(
            writer
                .call(
                    true,
                    |c| Ok(c.execute("INSERT INTO entries VALUES(1)", [])?)
                )
                .is_err()
        );
        reader.execute_batch("COMMIT").unwrap();
        writer
            .call(false, |c| {
                c.execute("INSERT INTO entries VALUES(2)", [])?;
                Ok(())
            })
            .unwrap();
        drop(writer);
        assert_eq!(
            reader
                .query_row("SELECT value FROM entries", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
