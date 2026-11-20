use anyhow::{Context, Result, bail};
use tokio::process::{Child, Command};

// OS ownership also reaps descendants when a command future is dropped.
#[cfg(windows)]
pub struct ProcessTree(std::os::windows::io::OwnedHandle);
#[cfg(windows)]
impl ProcessTree {
    pub async fn spawn(cmd: &mut Command) -> Result<(Child, Self)> {
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        // Create suspended, bind the Job before any user-mode instruction runs,
        // and only then resume so fast-spawning descendants cannot escape.
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let mut child = cmd.spawn()?;
        let tree = match Self::attach(&child) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = child.kill().await;
                return Err(error);
            }
        };
        let pid = match child.id() {
            Some(pid) => pid,
            None => {
                tree.shutdown().await;
                let _ = child.kill().await;
                bail!("command process id missing");
            }
        };
        if let Err(error) = resume_process(pid) {
            tree.shutdown().await;
            let _ = child.kill().await;
            return Err(error);
        }
        Ok((child, tree))
    }
    pub fn attach(child: &Child) -> Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let job = Self(std::os::windows::io::OwnedHandle::from_raw_handle(handle));
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(
                    job.0.as_raw_handle(),
                    child
                        .raw_handle()
                        .context("command process handle missing")?,
                ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(job)
        }
    }
    pub fn active(&self) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            QueryInformationJobObject(
                self.0.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
                std::ptr::null_mut(),
            ) != 0
                && info.ActiveProcesses > 0
        }
    }
    pub fn terminate(&self) {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.as_raw_handle(), 1);
        }
    }
    pub fn release(&mut self) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            if SetInformationJobObject(
                self.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
}
#[cfg(windows)]
fn resume_process(pid: u32) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);
        impl Drop for HandleGuard {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let _snap = HandleGuard(snapshot);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        if Thread32First(snapshot, &mut entry) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut resumed = 0usize;
        loop {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                let _thread_guard = HandleGuard(thread);
                if ResumeThread(thread) == u32::MAX {
                    return Err(std::io::Error::last_os_error().into());
                }
                resumed += 1;
            }
            if Thread32Next(snapshot, &mut entry) == 0 {
                break;
            }
        }
        if resumed == 0 {
            bail!("no thread found to resume for spawned process {pid}");
        }
    }
    Ok(())
}
#[cfg(unix)]
pub struct ProcessTree(i32);
#[cfg(unix)]
impl ProcessTree {
    pub async fn spawn(cmd: &mut Command) -> Result<(Child, Self)> {
        let mut child = cmd.spawn()?;
        match Self::attach(&child) {
            Ok(tree) => Ok((child, tree)),
            Err(error) => {
                let _ = child.kill().await;
                Err(error)
            }
        }
    }
    pub fn attach(child: &Child) -> Result<Self> {
        Ok(Self(
            child.id().context("command process id missing")? as i32
        ))
    }
    pub fn terminate(&self) {
        if self.0 != 0 {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
    pub fn release(&mut self) -> Result<()> {
        self.0 = 0;
        Ok(())
    }
}
impl ProcessTree {
    pub async fn shutdown(&self) {
        self.terminate();
        #[cfg(windows)]
        {
            let started = std::time::Instant::now();
            while self.active() && started.elapsed() < std::time::Duration::from_secs(2) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }
}
#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::process::Stdio;

    #[tokio::test]
    async fn windows_spawn_binds_job_before_descendants_escape() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let ps = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .map(|r| r.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
            .unwrap_or_else(|| std::path::PathBuf::from("powershell.exe"));
        let script = format!(
            "$p = Start-Process -FilePath '{}' -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60' -PassThru -WindowStyle Hidden; [IO.File]::WriteAllText('{}', [string]$p.Id); Start-Sleep -Seconds 60",
            ps.display(),
            pid_file.display()
        );
        let mut cmd = Command::new(&ps);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let (mut child, tree) = ProcessTree::spawn(&mut cmd).await.unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let gc_pid: u32 = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild did not write pid file"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        assert!(tree.active());
        tree.shutdown().await;
        let _ = child.wait().await;
        assert!(!tree.active());
        unsafe {
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::Threading::{
                    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                },
            };
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, gc_pid);
            if !handle.is_null() {
                let mut code = 0u32;
                let ok = GetExitCodeProcess(handle, &mut code);
                CloseHandle(handle);
                assert!(ok == 0 || code != 259, "grandchild {gc_pid} still alive");
            }
        }
    }
}
