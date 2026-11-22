use super::ToolContext;
use crate::{models::Outcome, util};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(windows)]
use std::{path::PathBuf, sync::OnceLock};
use tokio::process::Command;

#[cfg(windows)]
pub(crate) fn git_bash() -> Option<PathBuf> {
    if std::env::var("MINICODE_WINDOWS_SHELL")
        .is_ok_and(|value| value.eq_ignore_ascii_case("powershell"))
    {
        return None;
    }
    let path = util::bundled_path().or_else(|| std::env::var_os("PATH"))?;
    for dir in std::env::split_paths(&path) {
        if !dir.join("git.exe").is_file() {
            continue;
        }
        let root = if dir.file_name().is_some_and(|name| {
            name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("bin")
        }) {
            dir.parent().unwrap_or(&dir)
        } else {
            &dir
        };
        for candidate in [root.join("bin/bash.exe"), root.join("usr/bin/bash.exe")] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn powershell_command(command: &str, program: &std::ffi::OsStr) -> Command {
    let mut cmd = Command::new(program);
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!(
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);$OutputEncoding=[Text.UTF8Encoding]::new($false);$global:LASTEXITCODE=$null;\n{command}\n$__minicode_ok=$?;if ($__minicode_ok) {{ exit 0 }} elseif ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }} else {{ exit 1 }}"
        ),
    ]);
    cmd
}

#[cfg(windows)]
fn pwsh_file_version(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // The returned version structure points into `data`, which stays alive
    // until the unaligned copy is complete.
    unsafe {
        let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
        if size == 0 || size > 1_000_000 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut value = std::ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(
            data.as_ptr().cast(),
            [b'\\' as u16, 0].as_ptr(),
            &mut value,
            &mut length,
        ) == 0
            || value.is_null()
            || (length as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let info = value.cast::<VS_FIXEDFILEINFO>().read_unaligned();
        if info.dwSignature != 0xfeef04bd {
            return None;
        }
        Some(format!(
            "{}.{}.{}",
            info.dwProductVersionMS >> 16,
            info.dwProductVersionMS & 0xffff,
            info.dwProductVersionLS >> 16
        ))
    }
}

#[cfg(windows)]
fn powershell_info() -> &'static (PathBuf, String) {
    use std::os::windows::{fs::MetadataExt, process::CommandExt};
    static INFO: OnceLock<(PathBuf, String)> = OnceLock::new();
    INFO.get_or_init(|| {
        let path = util::bundled_path().or_else(|| std::env::var_os("PATH"));
        let program = path
            .as_ref()
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join("pwsh.exe"))
            .find(|path| {
                let app_alias = path
                    .components()
                    .any(|component| component.as_os_str().eq_ignore_ascii_case("WindowsApps"))
                    && std::fs::symlink_metadata(path)
                        .is_ok_and(|metadata| metadata.file_attributes() & 0x400 != 0);
                path.is_file() && !app_alias
            })
            .unwrap_or_else(|| {
                std::env::var_os("SystemRoot")
                    .map(PathBuf::from)
                    .map(|root| root.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
                    .unwrap_or_else(|| PathBuf::from("powershell.exe"))
            });
        let version = if program
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("pwsh.exe"))
        {
            pwsh_file_version(&program)
        } else {
            None
        }
        .or_else(|| {
            (|| -> Option<String> {
                let mut output = tempfile::tempfile().ok()?;
                let mut child = std::process::Command::new(&program)
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        "[Console]::Out.Write($PSVersionTable.PSVersion.ToString())",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(output.try_clone().ok()?))
                    .stderr(Stdio::null())
                    .creation_flags(0x08000000)
                    .spawn()
                    .ok()?;
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) if status.success() => {
                            let mut version = String::new();
                            output.seek(SeekFrom::Start(0)).ok()?;
                            output.take(128).read_to_string(&mut version).ok()?;
                            let version = version.trim();
                            if version.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
                                && version.contains('.')
                            {
                                return Some(version.into());
                            }
                            return None;
                        }
                        Ok(Some(_)) => return None,
                        Ok(None) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return None;
                        }
                    }
                }
            })()
        })
        .unwrap_or_else(|| "版本未知".into());
        (program, version)
    })
}

#[cfg(windows)]
pub(crate) fn windows_shell_guidance() -> String {
    let version = &powershell_info().1;
    let major = version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok());
    let chains = if major.is_some_and(|major| major >= 7) {
        "PowerShell 7 支持 && 和 ||；仍应按 PowerShell 语法编写命令。"
    } else {
        "PowerShell 5.1 或版本未知时不支持 && 和 ||；请使用 if ($?) 或分步执行。"
    };
    let bash = if git_bash().is_some() {
        "bash 工具使用 Git Bash，按 bash 语法编写命令。"
    } else {
        "bash 工具是 PowerShell 的兼容别名，按 PowerShell 语法编写命令。"
    };
    format!("{bash} powershell 工具使用 PowerShell {version}。{chains}")
}

pub fn shell(command: &str, cwd: &Path) -> Command {
    shell_for("bash", command, cwd)
}

fn shell_for(name: &str, command: &str, cwd: &Path) -> Command {
    #[cfg(windows)]
    let mut cmd = {
        let bash = if name == "bash" { git_bash() } else { None };
        let mut c = if let Some(bash) = bash {
            let mut c = Command::new(bash);
            c.args(["-c", command]);
            c
        } else {
            powershell_command(command, powershell_info().0.as_os_str())
        };
        c.creation_flags(0x08000000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let _ = name;
        let mut c = Command::new("bash");
        c.args(["-c", command]);
        c.process_group(0);
        c
    };
    if let Some(path) = util::bundled_path() {
        cmd.env("PATH", path);
    }
    for (key, value) in [
        ("NO_COLOR", "1"),
        ("TERM", "dumb"),
        ("CARGO_TERM_COLOR", "never"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_EDITOR", "true"),
        ("GIT_SEQUENCE_EDITOR", "true"),
        ("PAGER", "cat"),
        ("GIT_PAGER", "cat"),
        ("CI", "1"),
    ] {
        if std::env::var_os(key).is_none_or(|value| value.is_empty()) {
            cmd.env(key, value);
        }
    }
    #[cfg(windows)]
    for (name, default) in [("PYTHONUTF8", "1"), ("PYTHONIOENCODING", "utf-8")] {
        if std::env::var_os(name).is_none_or(|value| value.is_empty()) {
            cmd.env(name, default);
        }
    }
    cmd.current_dir(cwd).stdin(Stdio::null()).kill_on_drop(true);
    cmd
}
pub async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("taskkill.exe");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x08000000);
            let _ = tokio::time::timeout(Duration::from_secs(5), cmd.status()).await;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}
pub async fn run_command(
    command: &str,
    cwd: &Path,
    timeout: f64,
    ctx: &ToolContext,
) -> Result<Outcome> {
    run_command_for("bash", command, cwd, timeout, ctx).await
}

struct OutputReaders(Vec<tokio::task::JoinHandle<Result<()>>>);
impl Drop for OutputReaders {
    fn drop(&mut self) {
        for reader in &self.0 {
            reader.abort();
        }
    }
}
struct PipeDoneGuard {
    remaining: Arc<std::sync::atomic::AtomicUsize>,
    done: Arc<tokio::sync::Notify>,
}
impl Drop for PipeDoneGuard {
    fn drop(&mut self) {
        if self
            .remaining
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            self.done.notify_one();
        }
    }
}
fn capture_pipe(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    file: Arc<std::sync::Mutex<File>>,
    remaining: Arc<std::sync::atomic::AtomicUsize>,
    done: Arc<tokio::sync::Notify>,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let _done_guard = PipeDoneGuard { remaining, done };
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let size = pipe.read(&mut buffer).await?;
            if size == 0 {
                return Ok(());
            }
            file.lock().unwrap().write_all(&buffer[..size])?;
        }
    })
}
pub async fn run_command_for(
    name: &str,
    command: &str,
    cwd: &Path,
    timeout: f64,
    ctx: &ToolContext,
) -> Result<Outcome> {
    run_command_captured(
        name,
        command,
        cwd,
        timeout,
        ctx,
        Arc::new(CommandCapture::new()?),
    )
    .await
}

pub(crate) struct CommandCapture {
    pub file: tempfile::NamedTempFile,
    pub pid: std::sync::atomic::AtomicU32,
    pub background: bool,
}
impl CommandCapture {
    pub fn new() -> Result<Self> {
        Ok(Self {
            file: tempfile::NamedTempFile::new()?,
            pid: Default::default(),
            background: false,
        })
    }
}
pub(crate) async fn run_command_captured(
    name: &str,
    command: &str,
    cwd: &Path,
    timeout: f64,
    ctx: &ToolContext,
    capture: Arc<CommandCapture>,
) -> Result<Outcome> {
    // Drain bounded pipe chunks to disk; EOF tells us whether descendants still
    // hold output, independently of redirected processes in the command tree.
    let captured = &capture.file;
    let mut capture_file = captured.reopen()?;
    let mut cmd = shell_for(name, command, cwd);
    let mut sandbox = if ctx.sandbox.enabled || ctx.force_sandbox {
        match crate::sandbox::prepare(&cmd, cwd, &ctx.workspace, &ctx.sandbox) {
            Ok(prepared) => Some(prepared),
            Err(error) => {
                ctx.store.event(
                    &ctx.session,
                    "shell_sandbox",
                    &json!({"enforced":false,"command_executed":false,"error":error.to_string()}),
                )?;
                return Err(error);
            }
        }
    } else {
        None
    };
    if let Some(prepared) = &mut sandbox {
        std::mem::swap(&mut cmd, &mut prepared.command);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let (mut child, mut tree) = crate::command_process::ProcessTree::spawn(&mut cmd)
        .await
        .context("failed to start and own command process tree")?;
    capture.pid.store(
        child.id().unwrap_or(0),
        std::sync::atomic::Ordering::Release,
    );
    if let Some(prepared) = &sandbox {
        prepared.owned()?;
    }
    let file = Arc::new(std::sync::Mutex::new(captured.as_file().try_clone()?));
    let remaining_readers = Arc::new(std::sync::atomic::AtomicUsize::new(2));
    let readers_done = Arc::new(tokio::sync::Notify::new());
    let mut readers = OutputReaders(vec![
        capture_pipe(
            child.stdout.take().unwrap(),
            file.clone(),
            remaining_readers.clone(),
            readers_done.clone(),
        ),
        capture_pipe(
            child.stderr.take().unwrap(),
            file,
            remaining_readers.clone(),
            readers_done.clone(),
        ),
    ]);
    let started = Instant::now();
    let mut live_position = 0u64;
    let mut timed_out = false;
    let mut quota = false;
    let mut cancelled = false;
    let mut exited = None;
    let mut lingered = false;
    let mut last_emit = Instant::now();
    let mut sandbox_announced = false;
    let status = loop {
        if let Some(prepared) = &sandbox
            && !sandbox_announced
            && prepared.enforced()
        {
            ctx.store.event(&ctx.session,"shell_sandbox",&json!({"enforced":true,"backend":prepared.protection,"network":ctx.sandbox.network,"workspace":ctx.workspace}))?;
            sandbox_announced = true;
        }
        if exited.is_none()
            && let Some(status) = child.try_wait()?
        {
            exited = Some((Instant::now(), status));
        }
        let all_readers_done = remaining_readers.load(std::sync::atomic::Ordering::Acquire) == 0;
        if let Some((time, status)) = exited {
            if all_readers_done {
                break Some(status);
            }
            if time.elapsed() >= Duration::from_secs(2) {
                lingered = true;
                break Some(status);
            }
        }
        if ctx.cancel.is_cancelled() {
            cancelled = true;
            kill_tree(&mut child).await;
            break child.try_wait()?.or(exited.map(|(_, status)| status));
        }
        if (timeout > 0.0 && started.elapsed().as_secs_f64() >= timeout)
            || ctx.deadline.is_some_and(|d| Instant::now() >= d)
        {
            timed_out = true;
            kill_tree(&mut child).await;
            break child.try_wait()?.or(exited.map(|(_, status)| status));
        }
        let size = captured.as_file().metadata()?.len();
        if size > 100_000_000 {
            quota = true;
            kill_tree(&mut child).await;
            break child.try_wait()?.or(exited.map(|(_, status)| status));
        }
        if size > live_position && last_emit.elapsed() >= Duration::from_millis(250) {
            if let Some(emit) = &ctx.output {
                let mut f = File::open(captured.path())?;
                f.seek(SeekFrom::Start(size.saturating_sub(8000)))?;
                let mut bytes = vec![];
                f.take(8000).read_to_end(&mut bytes)?;
                let text = decode(&bytes);
                emit(json!({"text":util::suffix(&text,2000)}));
            }
            live_position = size;
            last_emit = Instant::now();
        }
        let heartbeat = if let Some((time, _)) = exited {
            Duration::from_secs(2)
                .saturating_sub(time.elapsed())
                .min(Duration::from_millis(50))
        } else {
            Duration::from_millis(50)
        };
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {}
            wait_result = child.wait(), if exited.is_none() => {
                exited = Some((Instant::now(), wait_result?));
            }
            _ = readers_done.notified(), if !all_readers_done => {}
            _ = tokio::time::sleep(heartbeat) => {}
        }
    };
    if sandbox.is_none() && !capture.background && !timed_out && !quota && !cancelled && !lingered {
        tree.release()?;
    } else {
        tree.shutdown().await;
    }
    drop(tree);
    if let Some(prepared) = &mut sandbox {
        prepared.finish()?;
    }
    if sandbox.is_some() && !sandbox_announced {
        ctx.store.event(
            &ctx.session,
            "shell_sandbox",
            &json!({"enforced":false,"command_executed":if cfg!(windows){Some(false)}else{None},"reason":"sandbox bootstrap could not be verified"}),
        )?;
        let mut bytes = vec![];
        captured.reopen()?.take(2000).read_to_end(&mut bytes)?;
        let detail = util::terminal_text(&String::from_utf8_lossy(&bytes));
        let reason = if cfg!(windows) {
            if detail.contains("shellSandbox.readOnlyPaths") {
                "OS shell sandbox could not start; command was not executed"
            } else {
                "OS shell sandbox could not start (if the shell or external toolchain lives outside default system directories, add its directory to shellSandbox.readOnlyPaths in settings.json); command was not executed"
            }
        } else {
            "OS shell sandbox startup could not be verified; command may have started"
        };
        bail!("{reason}: {detail}");
    }
    for reader in &mut readers.0 {
        match tokio::time::timeout(Duration::from_secs(1), &mut *reader).await {
            Ok(result) => result??,
            Err(_) => {
                reader.abort();
                let _ = reader.await;
            }
        }
    }
    let total = captured.as_file().metadata()?.len().min(100_000_000);
    #[cfg(windows)]
    let capture_utf8 = capture_is_utf8(&mut capture_file, total)?;
    #[cfg(not(windows))]
    let capture_utf8 = true;
    capture_file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![];
    (&mut capture_file).take(256_000).read_to_end(&mut bytes)?;
    let decoded = decode_capture_sample(&bytes, capture_utf8);
    let decoded = decoded.trim_end_matches(['\r', '\n']);
    const PREVIEW_BYTES: usize = 50 * 1024 - 512;
    let truncated =
        total > 256_000 || decoded.len() > PREVIEW_BYTES || decoded.lines().count() > 2000;
    let mut outcome = Outcome::ok(decoded);
    if truncated {
        let head = util::prefix(decoded, PREVIEW_BYTES / 2)
            .split_inclusive('\n')
            .take(999)
            .collect::<String>();
        let tail_source = if total > 256_000 {
            capture_file.seek(SeekFrom::Start(total.saturating_sub(128_000)))?;
            bytes.clear();
            (&mut capture_file).take(128_000).read_to_end(&mut bytes)?;
            decode_capture_sample(&bytes, capture_utf8)
        } else {
            decoded.to_string()
        };
        let tail = util::tail(&tail_source, 999, PREVIEW_BYTES / 2);
        let artifact = record_capture(captured, ctx, total)?;
        outcome.output = format!(
            "{}\n...[middle output omitted; output truncated; 完整输出见 [artifact:{artifact}]]\n{tail}",
            head.trim_end_matches(['\r', '\n'])
        );
        outcome.artifact_id = Some(artifact);
        outcome.truncated = true;
    }
    outcome.exit_code = status.and_then(|s| s.code());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        outcome.signal = status.and_then(|s| s.signal()).map(|signal| match signal {
            libc::SIGTERM => "SIGTERM".into(),
            libc::SIGKILL => "SIGKILL".into(),
            libc::SIGINT => "SIGINT".into(),
            libc::SIGHUP => "SIGHUP".into(),
            libc::SIGQUIT => "SIGQUIT".into(),
            libc::SIGABRT => "SIGABRT".into(),
            libc::SIGSEGV => "SIGSEGV".into(),
            libc::SIGPIPE => "SIGPIPE".into(),
            libc::SIGILL => "SIGILL".into(),
            libc::SIGBUS => "SIGBUS".into(),
            libc::SIGFPE => "SIGFPE".into(),
            libc::SIGUSR1 => "SIGUSR1".into(),
            libc::SIGUSR2 => "SIGUSR2".into(),
            libc::SIGALRM => "SIGALRM".into(),
            _ => format!("signal {signal}"),
        });
    }
    outcome.success =
        outcome.exit_code == Some(0) && !cancelled && !timed_out && !quota && !lingered;
    if cancelled {
        outcome.error = Some("tool cancelled; command tree was killed".into());
    } else if timed_out {
        outcome.error = Some(format!("command timed out after {timeout}s and was killed"));
    } else if lingered {
        outcome.error = Some("shell exited but a descendant kept output open; capture was stopped and the command tree was killed".into());
    } else if quota {
        outcome.error = Some(
            "command output exceeded capture quota and was killed; narrow or redirect output"
                .into(),
        );
    } else if !outcome.success {
        outcome.error = Some(match (&outcome.signal, outcome.exit_code) {
            (Some(signal), _) => format!("command terminated by {signal}"),
            (_, Some(code)) => format!("command exited with code {code}"),
            _ => "command terminated without an exit code".into(),
        });
    }
    if let (Some(signal), Some(error)) = (&outcome.signal, &mut outcome.error)
        && !error.contains(signal)
    {
        error.push_str(&format!(" ({signal})"));
    }
    Ok(outcome)
}
fn record_capture(
    captured: &tempfile::NamedTempFile,
    ctx: &ToolContext,
    total: u64,
) -> Result<String> {
    let artifact = format!("command_output_{}", &util::id()[..12]);
    let dir = ctx.store.root().join("artifacts").join(&ctx.session);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{artifact}.txt"));
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    let mut source = captured.as_file().try_clone()?;
    source.seek(SeekFrom::Start(0))?;
    #[cfg(windows)]
    let codepage = if capture_is_utf8(&mut source, total)? {
        None
    } else {
        use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};
        source.seek(SeekFrom::Start(0))?;
        let mut sample = vec![0; (total as usize).min(8192)];
        let n = source.read(&mut sample)?;
        sample.truncate(n);
        unsafe { legacy_codepage(&sample, GetOEMCP(), GetACP()) }
    };
    source.seek(SeekFrom::Start(0))?;
    let mut source = source.take(total);
    #[cfg(windows)]
    if let Some(codepage) = codepage {
        copy_codepage_capture(&mut source, &mut temp, codepage)?;
    } else {
        copy_utf8_capture(&mut source, &mut temp)?;
    }
    #[cfg(not(windows))]
    copy_utf8_capture(&mut source, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist(&path).map_err(|e| e.error)?;
    let (session, record_artifact) = (ctx.session.clone(), artifact.clone());
    ctx.store.write(move |c| {
        let artifact = &record_artifact;
        c.execute(
            "INSERT INTO artifacts(session_id,artifact_id,kind,relpath,created_at) VALUES(?,?,?,?,?)",
            rusqlite::params![
                session,
                artifact,
                "command_output",
                format!("{artifact}.txt"),
                util::now()
            ],
        )?;
        Ok(())
    })?;
    ctx.store
        .prune_output_artifacts(Some((&ctx.session, &artifact)))?;
    Ok(artifact)
}

fn copy_utf8_capture(source: &mut impl Read, output: &mut impl Write) -> Result<()> {
    let mut buffer = vec![];
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut chunk)?;
        buffer.extend_from_slice(&chunk[..n]);
        let mut end = 0;
        while end < buffer.len() {
            match std::str::from_utf8(&buffer[end..]) {
                Ok(_) => {
                    output.write_all(&buffer[end..])?;
                    end = buffer.len();
                }
                Err(error) => {
                    let valid = end + error.valid_up_to();
                    output.write_all(&buffer[end..valid])?;
                    end = valid;
                    if let Some(length) = error.error_len() {
                        output.write_all("�".as_bytes())?;
                        end += length;
                    } else if n == 0 {
                        output.write_all("�".as_bytes())?;
                        end = buffer.len();
                    } else {
                        break;
                    }
                }
            }
        }
        buffer.drain(..end);
        if n == 0 {
            break;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn capture_is_utf8(source: &mut File, total: u64) -> Result<bool> {
    let mut source = source.take(total);
    let mut pending = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut chunk)?;
        pending.extend_from_slice(&chunk[..n]);
        match std::str::from_utf8(&pending) {
            Ok(_) => pending.clear(),
            Err(error) if error.error_len().is_some() => return Ok(false),
            Err(error) => {
                pending.drain(..error.valid_up_to());
            }
        }
        if n == 0 {
            return Ok(pending.is_empty());
        }
    }
}

#[cfg(windows)]
fn copy_codepage_capture(
    source: &mut impl Read,
    output: &mut impl Write,
    codepage: u32,
) -> Result<()> {
    use windows_sys::Win32::Globalization::IsDBCSLeadByteEx;
    let lead: [bool; 256] =
        std::array::from_fn(|byte| unsafe { IsDBCSLeadByteEx(codepage, byte as u8) != 0 });
    let mut pending = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut chunk)?;
        pending.extend_from_slice(&chunk[..n]);
        let mut end = 0;
        while end < pending.len() {
            if lead[pending[end] as usize] {
                if end + 1 == pending.len() && n != 0 {
                    break;
                }
                end += if end + 1 < pending.len() { 2 } else { 1 };
            } else {
                end += 1;
            }
        }
        if end > 0 {
            let text = decode_codepage(&pending[..end], codepage, false)
                .unwrap_or_else(|| String::from_utf8_lossy(&pending[..end]).into_owned());
            output.write_all(text.as_bytes())?;
            pending.drain(..end);
        }
        if n == 0 {
            break;
        }
    }
    Ok(())
}
#[cfg(windows)]
fn decode_codepage(bytes: &[u8], codepage: u32, strict: bool) -> Option<String> {
    use windows_sys::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};
    if bytes.is_empty() {
        return Some(String::new());
    }
    let len = i32::try_from(bytes.len()).ok()?;
    let flags = if strict { MB_ERR_INVALID_CHARS } else { 0 };
    // MultiByteToWideChar first computes the UTF-16 length, then fills exactly
    // that many elements in the allocated buffer.
    unsafe {
        let needed = MultiByteToWideChar(
            codepage,
            flags,
            bytes.as_ptr(),
            len,
            std::ptr::null_mut(),
            0,
        );
        if needed <= 0 {
            return None;
        }
        let mut wide = vec![0u16; needed as usize];
        let written = MultiByteToWideChar(
            codepage,
            flags,
            bytes.as_ptr(),
            len,
            wide.as_mut_ptr(),
            needed,
        );
        (written == needed).then(|| String::from_utf16_lossy(&wide))
    }
}

#[cfg(windows)]
fn legacy_codepage(bytes: &[u8], oem: u32, ansi: u32) -> Option<u32> {
    let candidates = [oem, ansi];
    candidates
        .iter()
        .copied()
        .filter(|codepage| *codepage != 65001)
        .find(|codepage| decode_codepage(bytes, *codepage, true).is_some())
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .filter(|codepage| *codepage != 65001)
                .find(|codepage| decode_codepage(bytes, *codepage, false).is_some())
        })
}

#[cfg(windows)]
fn decode_windows(bytes: &[u8], oem: u32, ansi: u32) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    legacy_codepage(bytes, oem, ansi)
        .and_then(|codepage| decode_codepage(bytes, codepage, false))
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

pub(crate) fn decode(bytes: &[u8]) -> String {
    #[cfg(windows)]
    let text = unsafe {
        use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};
        decode_windows(bytes, GetOEMCP(), GetACP())
    };
    #[cfg(not(windows))]
    let text = String::from_utf8_lossy(bytes).into_owned();
    util::command_text(&text)
}

fn decode_capture_sample(bytes: &[u8], utf8: bool) -> String {
    if !utf8 {
        return decode(bytes);
    }
    // A window may start/end inside a UTF-8 scalar. Its boundaries must not
    // cause Windows to reinterpret an otherwise UTF-8 capture as a code page.
    let start = bytes
        .iter()
        .take(3)
        .take_while(|b| **b & 0xc0 == 0x80)
        .count();
    let bytes = &bytes[start..];
    let end = match std::str::from_utf8(bytes) {
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        _ => bytes.len(),
    };
    util::command_text(&String::from_utf8_lossy(&bytes[..end]))
}
#[cfg(all(test, windows))]
mod windows_shell_tests {
    use super::powershell_command;
    use std::{ffi::OsStr, path::PathBuf};

    #[tokio::test]
    async fn powershell_versions_preserve_native_and_cmdlet_status() {
        let legacy = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let modern = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("pwsh.exe"))
                .find(|path| path.is_file())
        });
        for program in std::iter::once(legacy).chain(modern) {
            for (command, expected) in [
                ("node -e \"process.exit(7)\"", 7),
                ("node -e \"process.exit(9)\"; Write-Output 'recovered'", 0),
                ("Write-Error 'failure'", 1),
                ("exit 23", 23),
            ] {
                let status = powershell_command(command, OsStr::new(&program))
                    .output()
                    .await
                    .unwrap()
                    .status;
                assert_eq!(
                    status.code(),
                    Some(expected),
                    "{}: {command}",
                    program.display()
                );
            }
        }
    }
}

#[cfg(all(test, windows))]
mod windows_encoding_tests {
    use super::{capture_is_utf8, copy_codepage_capture, decode_windows};
    use std::io::{Seek, SeekFrom, Write};

    // This machine may use a non-Chinese system locale; the production
    // decoder will be exercised with CP936 explicitly after the fix.
    fn cp936(bytes: &[u8]) -> String {
        decode_windows(bytes, 936, 936)
    }

    #[test]
    fn gbk_cp936_and_utf8_are_decoded_without_mojibake() {
        assert_eq!(cp936(&[0xd6, 0xd0, 0xce, 0xc4]), "中文");
        assert_eq!(
            decode_windows(&[0xd6, 0xd0, 0xce, 0xc4], 65001, 936),
            "中文"
        );
        assert_eq!(cp936("中文".as_bytes()), "中文");
        if unsafe { windows_sys::Win32::Globalization::GetOEMCP() } == 936 {
            assert_eq!(super::decode(&[0xd6, 0xd0, 0xce, 0xc4]), "中文");
        }
    }

    #[test]
    fn gbk_capture_keeps_characters_at_chunk_boundaries() {
        for padding in [65_534, 65_535] {
            let mut bytes = vec![b'a'; padding];
            bytes.extend_from_slice(&[0xd6, 0xd0, 0xce, 0xc4]);
            let mut decoded = Vec::new();
            copy_codepage_capture(&mut bytes.as_slice(), &mut decoded, 936).unwrap();
            assert_eq!(decoded.len(), padding + "中文".len());
            assert!(decoded.ends_with("中文".as_bytes()));
        }
    }

    #[test]
    fn utf8_capture_detection_handles_split_characters() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![b'a'; 65_535]).unwrap();
        file.write_all("中文".as_bytes()).unwrap();
        let total = file.metadata().unwrap().len();
        file.seek(SeekFrom::Start(0)).unwrap();
        assert!(capture_is_utf8(&mut file, total).unwrap());
        file.seek(SeekFrom::Start(total)).unwrap();
        file.write_all(&[0xd6, 0xd0]).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        assert!(!capture_is_utf8(&mut file, total + 2).unwrap());
    }
}

#[cfg(test)]
mod capture_wakeup_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt;
    use tokio::sync::Notify;

    fn test_tool_context() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = util::canonical(dir.path()).unwrap();
        let store = crate::store::Store::open(workspace.join("test.db")).unwrap();
        let session = store
            .create(&workspace.to_string_lossy(), "fake", "fake")
            .unwrap();
        let catalog = crate::extensions::Catalog {
            workspace: workspace.clone(),
            skills: Default::default(),
            plugins: vec![],
            servers: vec![],
            agents: Default::default(),
            local: vec![],
            instructions: Default::default(),
            active: Default::default(),
            pending_scopes: Default::default(),
            configuration: crate::configuration::Configuration {
                path: workspace.join("config.json"),
            },
        };
        let ctx = ToolContext {
            workspace: workspace.clone(),
            session,
            store,
            attachments: crate::attachments::Attachments {
                root: workspace.join("attachments"),
            },
            modalities: vec![],
            catalog: Arc::new(tokio::sync::Mutex::new(catalog)),
            mcp: Arc::new(crate::mcp::McpRegistry::default()),
            cancel: tokio_util::sync::CancellationToken::new(),
            output: None,
            deadline: None,
            files: Default::default(),
            permissions: Default::default(),
            sandbox: Default::default(),
            force_sandbox: false,
        };
        (dir, ctx)
    }

    #[tokio::test]
    async fn capture_pipe_notifies_immediately_when_last_pipe_closes() {
        let captured = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(std::sync::Mutex::new(
            captured.as_file().try_clone().unwrap(),
        ));
        let remaining = Arc::new(AtomicUsize::new(2));
        let done = Arc::new(Notify::new());
        let (r1, mut w1) = tokio::io::duplex(64);
        let (r2, mut w2) = tokio::io::duplex(64);
        let h1 = capture_pipe(r1, file.clone(), remaining.clone(), done.clone());
        let h2 = capture_pipe(r2, file, remaining.clone(), done.clone());

        w1.write_all(b"hello ").await.unwrap();
        drop(w1);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(remaining.load(Ordering::Acquire), 1);

        let notified = done.notified();
        tokio::pin!(notified);
        w2.write_all(b"world").await.unwrap();
        drop(w2);
        tokio::time::timeout(Duration::from_millis(25), &mut notified)
            .await
            .expect("Notify should wake as soon as both pipes hit EOF");
        assert_eq!(remaining.load(Ordering::Acquire), 0);
        h1.await.unwrap().unwrap();
        h2.await.unwrap().unwrap();
        assert_eq!(fs::read_to_string(captured.path()).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn fast_command_completes_and_lingering_descendant_is_detected() {
        let (_dir, ctx) = test_tool_context();
        let fast = run_command("echo fast-done", &ctx.workspace, 10.0, &ctx)
            .await
            .unwrap();
        assert!(fast.success, "{fast:?}");
        assert!(fast.output.contains("fast-done"));

        #[cfg(windows)]
        let linger_cmd = (
            "powershell",
            "Start-Process -NoNewWindow -FilePath (Get-Process -Id $PID).Path -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 5'; Write-Output 'parent-exited'",
        );
        #[cfg(not(windows))]
        let linger_cmd = ("bash", "sleep 5 & echo parent-exited");

        let linger = run_command_for(linger_cmd.0, linger_cmd.1, &ctx.workspace, 10.0, &ctx)
            .await
            .unwrap();
        assert!(!linger.success, "{linger:?}");
        assert!(
            linger
                .error
                .as_deref()
                .is_some_and(|err| err.contains("descendant kept output open")),
            "{linger:?}"
        );
        assert!(linger.output.contains("parent-exited"), "{linger:?}");
    }
}
