use super::{Config, Prepared, clean_environment};
use crate::util;
use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    ptr,
};
use tokio::process::Command;
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Isolation::*, *},
    Storage::FileSystem::*,
    System::{Console::*, Threading::*},
};

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn code(result: u32) -> Result<()> {
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32).into());
    }
    Ok(())
}

pub(super) struct Profile {
    name: String,
    sid: PSID,
    grants: Vec<PathBuf>,
    _temporary: tempfile::TempDir,
    closed: bool,
}
// Profile owns its SID and ACL grants. Win32 profile/ACL APIs have no thread
// affinity; this object is moved between executor threads, never aliased.
unsafe impl Send for Profile {}

impl Profile {
    fn new(workspace: &Path, read_roots: &[PathBuf]) -> Result<Self> {
        let temporary = tempfile::tempdir()?;
        let name = format!("minicode.{}", util::id());
        let text = wide(&name);
        let mut sid = ptr::null_mut();
        let result = unsafe {
            CreateAppContainerProfile(
                text.as_ptr(),
                text.as_ptr(),
                text.as_ptr(),
                ptr::null(),
                0,
                &mut sid,
            )
        };
        if result < 0 {
            bail!(
                "Windows AppContainer profile unavailable (0x{:08x}); command was not executed",
                result as u32
            )
        }
        let mut profile = Self {
            name,
            sid,
            grants: vec![],
            _temporary: temporary,
            closed: false,
        };
        profile.grant(
            workspace,
            FILE_GENERIC_READ
                | FILE_GENERIC_WRITE
                | FILE_GENERIC_EXECUTE
                | DELETE
                | FILE_DELETE_CHILD,
        )?;
        let temp = profile._temporary.path().join("child");
        std::fs::create_dir(&temp)?;
        profile.grant(&temp, FILE_ALL_ACCESS)?;
        for root in read_roots {
            profile.grant(root, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)?;
        }
        Ok(profile)
    }
    fn grant(&mut self, path: &Path, access: u32) -> Result<()> {
        if util::is_link(path) {
            bail!("AppContainer grant root must not be a link")
        }
        acl(path, self.sid, GRANT_ACCESS, access)?;
        self.grants.push(path.into());
        Ok(())
    }
    pub(super) fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let mut failures = vec![];
        self.grants
            .retain(|path| match acl(path, self.sid, REVOKE_ACCESS, 0) {
                Ok(()) => false,
                Err(_) if !path.exists() => false,
                Err(_) => {
                    failures.push("could not remove temporary AppContainer file grant");
                    true
                }
            });
        if !failures.is_empty() {
            bail!("{}", failures.join("; "))
        }
        let result = unsafe { DeleteAppContainerProfile(wide(&self.name).as_ptr()) };
        if result < 0 {
            bail!(
                "could not delete temporary AppContainer profile (0x{:08x})",
                result as u32
            )
        }
        self.closed = true;
        Ok(())
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = self.close();
        unsafe {
            FreeSid(self.sid);
        }
    }
}

fn acl(path: &Path, sid: PSID, mode: ACCESS_MODE, access: u32) -> Result<()> {
    if util::is_link(path) {
        bail!("refusing linked AppContainer ACL target")
    }
    let mutex_name = wide(format!(
        "Local\\MiniCode.SandboxAcl.{}",
        util::hash(path.to_string_lossy().as_bytes())
    ));
    let mutex = unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    struct Guard(HANDLE);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0);
                CloseHandle(self.0);
            }
        }
    }
    let acquired = unsafe { WaitForSingleObject(mutex, 5000) };
    if acquired != WAIT_OBJECT_0 && acquired != WAIT_ABANDONED {
        unsafe {
            CloseHandle(mutex);
        }
        bail!("AppContainer ACL transaction lock unavailable")
    }
    let _guard = Guard(mutex);
    let name = wide(path);
    let mut old = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    code(unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut old,
            ptr::null_mut(),
            &mut descriptor,
        )
    })?;
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: mode,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.cast(),
        },
    };
    let mut next = ptr::null_mut();
    let built = unsafe { SetEntriesInAclW(1, &entry, old, &mut next) };
    let result = if built == 0 {
        unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                next,
                ptr::null(),
            )
        }
    } else {
        built
    };
    unsafe {
        if !next.is_null() {
            LocalFree(next.cast());
        }
        LocalFree(descriptor);
    }
    code(result).context("AppContainer file grant failed; command was not executed")
}

pub(super) fn prepare(
    original: &Command,
    cwd: &Path,
    workspace: &Path,
    config: &Config,
) -> Result<Prepared> {
    if Path::new(original.as_std().get_program())
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("bash.exe"))
    {
        bail!(
            "Windows AppContainer sandbox cannot support Git Bash/MSYS global named objects; use the powershell tool. Command was not executed."
        )
    }
    if config.network {
        bail!(
            "Windows shell sandbox currently supports network=false only; command was not executed"
        )
    }
    let guard = Profile::new(workspace, &config.read_only_paths)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("sandbox-worker")
        .arg("--profile")
        .arg(&guard.name)
        .arg("--cwd")
        .arg(cwd)
        .arg("--program")
        .arg(original.as_std().get_program())
        .arg("--status")
        .arg(guard._temporary.path().join("started"))
        .arg("--")
        .args(original.as_std().get_args());
    command
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .creation_flags(0x08000000);
    clean_environment(&mut command, &guard._temporary.path().join("child"));
    Ok(Prepared {
        status: guard._temporary.path().join("started"),
        command,
        protection: "windows-appcontainer",
        guard,
    })
}

fn quote(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            result.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
            result.push('"')
        } else {
            result.extend(std::iter::repeat_n('\\', slashes));
            result.push(ch)
        }
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

pub(super) fn worker(
    profile: &str,
    cwd: &Path,
    program: &Path,
    status: &Path,
    args: &[String],
) -> Result<i32> {
    if !profile
        .strip_prefix("minicode.")
        .is_some_and(util::valid_id)
    {
        bail!("invalid private AppContainer identity")
    }
    let owned = status.with_extension("owned");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !owned.is_file() {
        if std::time::Instant::now() >= deadline {
            bail!(
                "sandbox supervisor did not confirm process-tree ownership; command was not executed"
            )
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut sid = ptr::null_mut();
    let result =
        unsafe { DeriveAppContainerSidFromAppContainerName(wide(profile).as_ptr(), &mut sid) };
    if result < 0 {
        bail!("AppContainer identity could not be derived")
    }
    struct Sid(PSID);
    impl Drop for Sid {
        fn drop(&mut self) {
            unsafe {
                FreeSid(self.0);
            }
        }
    }
    let sid = Sid(sid);
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.0,
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let mut size = 0;
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
    }
    let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
    let list = storage.as_mut_ptr().cast();
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for Attributes {
        fn drop(&mut self) {
            unsafe {
                DeleteProcThreadAttributeList(self.0);
            }
        }
    }
    let list = Attributes(list);
    if unsafe {
        UpdateProcThreadAttribute(
            list.0,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            (&capabilities as *const SECURITY_CAPABILITIES).cast(),
            std::mem::size_of::<SECURITY_CAPABILITIES>(),
            ptr::null_mut(),
            ptr::null(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    unsafe {
        startup.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
        startup.StartupInfo.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
        startup.StartupInfo.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    }
    startup.lpAttributeList = list.0;
    let program_text = program.to_str().context("non-UTF-8 sandbox executable")?;
    let line = std::iter::once(program_text)
        .chain(args.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ");
    let mut line = wide(line);
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            wide(program).as_ptr(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_SUSPENDED,
            ptr::null(),
            wide(cwd).as_ptr(),
            &startup.StartupInfo,
            &mut process,
        )
    } == 0
    {
        return Err(anyhow::Error::from(std::io::Error::last_os_error())
            .context("AppContainer process could not start"));
    }
    unsafe {
        if let Err(error) = std::fs::write(status, "started") {
            TerminateProcess(process.hProcess, 1);
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
            return Err(error.into());
        }
        if ResumeThread(process.hThread) == u32::MAX {
            TerminateProcess(process.hProcess, 1);
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
            return Err(std::io::Error::last_os_error().into());
        }
        CloseHandle(process.hThread);
        WaitForSingleObject(process.hProcess, INFINITE);
        let mut code = 1;
        let ok = GetExitCodeProcess(process.hProcess, &mut code);
        CloseHandle(process.hProcess);
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(code as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_process_creation_never_marks_the_sandbox_started() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        std::fs::create_dir(&workspace).unwrap();
        let mut profile = Profile::new(&workspace, &[]).unwrap();
        let marker = profile._temporary.path().join("status");
        std::fs::write(marker.with_extension("owned"), "owned").unwrap();
        assert!(
            worker(
                &profile.name,
                &workspace,
                &root.path().join("nonexistent.exe"),
                &marker,
                &[]
            )
            .is_err()
        );
        assert!(!marker.exists());
        profile.close().unwrap();
    }
    #[test]
    fn command_line_quoting_preserves_quotes_spaces_and_trailing_backslashes() {
        assert_eq!(quote("a b"), "\"a b\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("a\\"), "\"a\\\\\"");
    }
}
