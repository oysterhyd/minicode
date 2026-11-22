use super::{Config, Prepared, clean_environment};
use crate::util;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
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

const ERROR_FILE_NOT_FOUND_HRESULT: i32 = 0x8007_0002_u32 as i32;

#[derive(Serialize, Deserialize)]
struct Lease {
    profile: String,
    grants: Vec<PathBuf>,
}

fn leases_dir() -> PathBuf {
    util::data_dir().join("sandbox-leases")
}

struct Sid(PSID);
impl Drop for Sid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                FreeSid(self.0);
            }
        }
    }
}

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
    lease_meta: PathBuf,
    lease_lock_path: PathBuf,
    lease_lock: Option<File>,
    _temporary: Option<tempfile::TempDir>,
    closed: bool,
}
// Profile owns its SID and ACL grants. Win32 profile/ACL APIs have no thread
// affinity; this object is moved between executor threads, never aliased.
unsafe impl Send for Profile {}

impl Profile {
    fn new(workspace: &Path, read_roots: &[PathBuf]) -> Result<Self> {
        Self::new_in(&leases_dir(), workspace, read_roots)
    }
    fn new_in(leases_dir: &Path, workspace: &Path, read_roots: &[PathBuf]) -> Result<Self> {
        if let Some(parent) = leases_dir.parent()
            && util::is_link(parent)
        {
            bail!("refusing linked sandbox lease parent directory")
        }
        fs::create_dir_all(leases_dir)?;
        if util::is_link(leases_dir) {
            bail!("refusing linked sandbox lease directory")
        }
        let temporary = tempfile::tempdir()?;
        let name = format!("minicode.{}", util::id());
        let lease_lock_path = leases_dir.join(format!("{name}.lock"));
        let lease_meta = leases_dir.join(format!("{name}.json"));
        if util::is_link(&lease_lock_path) || util::is_link(&lease_meta) {
            bail!("refusing linked sandbox lease path")
        }
        // Hold the OS lease lock before writing metadata or creating the profile
        // so concurrent orphan sweeps never reap an in-flight setup.
        let lease_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lease_lock_path)?;
        lease_file
            .try_lock_exclusive()
            .context("could not acquire sandbox lease lock")?;
        let mut recorded_grants = vec![workspace.to_path_buf()];
        recorded_grants.extend(read_roots.iter().cloned());
        if let Err(error) = util::atomic_write(
            &lease_meta,
            &serde_json::to_vec(&Lease {
                profile: name.clone(),
                grants: recorded_grants,
            })?,
        ) {
            let _ = lease_file.unlock();
            drop(lease_file);
            let _ = fs::remove_file(&lease_lock_path);
            return Err(error);
        }
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
            let _ = fs::remove_file(&lease_meta);
            let _ = lease_file.unlock();
            drop(lease_file);
            let _ = fs::remove_file(&lease_lock_path);
            bail!(
                "Windows AppContainer profile unavailable (0x{:08x}); command was not executed",
                result as u32
            )
        }
        let mut profile = Self {
            name,
            sid,
            grants: vec![],
            lease_meta,
            lease_lock_path,
            lease_lock: Some(lease_file),
            _temporary: Some(temporary),
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
        let temp = profile.temp_path().join("child");
        std::fs::create_dir(&temp)?;
        profile.grant(&temp, FILE_ALL_ACCESS)?;
        for root in read_roots {
            profile
                .grant(root, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)
                .with_context(|| {
                    format!(
                        "failed to grant Windows AppContainer read-only access to shellSandbox.readOnlyPaths entry '{}'; verify the directory exists, is not a symlink/junction, and permits ACL updates",
                        root.display()
                    )
                })?;
        }
        Ok(profile)
    }
    fn temp_path(&self) -> &Path {
        self._temporary
            .as_ref()
            .expect("sandbox profile temporary dir")
            .path()
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
        if result < 0 && result != ERROR_FILE_NOT_FOUND_HRESULT {
            bail!(
                "could not delete temporary AppContainer profile (0x{:08x})",
                result as u32
            )
        }
        if let Err(error) = fs::remove_file(&self.lease_meta)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            bail!("could not remove temporary sandbox lease metadata: {error}");
        }
        if let Some(lock) = self.lease_lock.take() {
            let _ = lock.unlock();
        }
        let _ = fs::remove_file(&self.lease_lock_path);
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

pub(super) fn cleanup_orphans() -> Result<usize> {
    cleanup_orphans_in(&leases_dir())
}

fn cleanup_orphans_in(leases_dir: &Path) -> Result<usize> {
    if !leases_dir.exists() {
        return Ok(0);
    }
    if util::is_link(leases_dir) {
        bail!("refusing linked sandbox lease directory")
    }
    let mut cleaned = 0usize;
    for entry in fs::read_dir(leases_dir)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if util::is_link(&path) || !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !stem.strip_prefix("minicode.").is_some_and(util::valid_id) {
            continue;
        }
        if ext == "lock" {
            let meta = leases_dir.join(format!("{stem}.json"));
            if !meta.exists()
                && let Ok(lock) = OpenOptions::new().read(true).write(true).open(&path)
                && lock.try_lock_exclusive().is_ok()
            {
                let _ = lock.unlock();
                drop(lock);
                let _ = fs::remove_file(&path);
            }
            continue;
        }
        if ext != "json" {
            continue;
        }
        let lock_path = leases_dir.join(format!("{stem}.lock"));
        if util::is_link(&lock_path) {
            continue;
        }
        let Ok(lock) = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        else {
            continue;
        };
        if lock.try_lock_exclusive().is_err() {
            continue;
        }
        if util::is_link(&path) || !path.is_file() {
            let _ = lock.unlock();
            drop(lock);
            let _ = fs::remove_file(&lock_path);
            continue;
        }
        let mut bytes = Vec::new();
        let Ok(file) = File::open(&path) else {
            let _ = lock.unlock();
            drop(lock);
            continue;
        };
        if file.take(1024 * 1024).read_to_end(&mut bytes).is_err() {
            let _ = lock.unlock();
            drop(lock);
            continue;
        }
        let Ok(lease) = serde_json::from_slice::<Lease>(&bytes) else {
            let _ = lock.unlock();
            drop(lock);
            continue;
        };
        if lease.profile != stem {
            let _ = lock.unlock();
            drop(lock);
            continue;
        }
        let mut raw_sid = ptr::null_mut();
        let derived = unsafe {
            DeriveAppContainerSidFromAppContainerName(wide(&lease.profile).as_ptr(), &mut raw_sid)
        };
        let mut revoke_ok = true;
        if derived >= 0 && !raw_sid.is_null() {
            let sid = Sid(raw_sid);
            for grant in &lease.grants {
                if grant.exists()
                    && !util::is_link(grant)
                    && acl(grant, sid.0, REVOKE_ACCESS, 0).is_err()
                {
                    revoke_ok = false;
                }
            }
        }
        if !revoke_ok {
            let _ = lock.unlock();
            drop(lock);
            continue;
        }
        let deleted = unsafe { DeleteAppContainerProfile(wide(&lease.profile).as_ptr()) };
        if deleted < 0 && deleted != ERROR_FILE_NOT_FOUND_HRESULT {
            let _ = lock.unlock();
            drop(lock);
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            cleaned += 1;
        }
        let _ = lock.unlock();
        drop(lock);
        let _ = fs::remove_file(&lock_path);
    }
    Ok(cleaned)
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
            "Windows AppContainer sandbox cannot support Git Bash/MSYS global named objects; use the powershell tool, and add any external toolchain directories to shellSandbox.readOnlyPaths in settings.json. Command was not executed."
        )
    }
    if config.network {
        bail!(
            "Windows shell sandbox currently supports network=false only; command was not executed"
        )
    }
    static CLEANUP: std::sync::Once = std::sync::Once::new();
    CLEANUP.call_once(|| {
        let _ = cleanup_orphans();
    });
    let program_path = Path::new(original.as_std().get_program());
    let sandbox_program = if program_path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("pwsh.exe"))
        && program_path
            .components()
            .any(|component| component.as_os_str().eq_ignore_ascii_case("WindowsApps"))
    {
        std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .map(|root| root.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
            .unwrap_or_else(|| PathBuf::from("powershell.exe"))
    } else {
        program_path.to_path_buf()
    };
    let guard = Profile::new(workspace, &config.read_only_paths)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("sandbox-worker")
        .arg("--profile")
        .arg(&guard.name)
        .arg("--cwd")
        .arg(cwd)
        .arg("--program")
        .arg(&sandbox_program)
        .arg("--status")
        .arg(guard.temp_path().join("started"))
        .arg("--")
        .args(original.as_std().get_args());
    command
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .creation_flags(0x08000000);
    clean_environment(&mut command, &guard.temp_path().join("child"));
    Ok(Prepared {
        status: guard.temp_path().join("started"),
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
        let err = std::io::Error::last_os_error();
        let hint = match program.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(dir) => format!(
                "AppContainer process could not start ({}); if this executable or its toolchain lives outside Windows default readable directories, authorize its directory (e.g. \"{}\") via shellSandbox.readOnlyPaths in settings.json",
                program.display(),
                dir.display()
            ),
            None => format!(
                "AppContainer process could not start ({}); authorize external toolchain directories via shellSandbox.readOnlyPaths in settings.json",
                program.display()
            ),
        };
        return Err(anyhow::Error::from(err).context(hint));
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
    fn dacl_contains_sid(path: &Path, sid: PSID) -> bool {
        unsafe {
            let mut dacl = ptr::null_mut();
            let mut descriptor = ptr::null_mut();
            let res = GetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            );
            assert_eq!(res, 0);
            let mut count = 0u32;
            let mut entries = ptr::null_mut();
            let got = GetExplicitEntriesFromAclW(dacl, &mut count, &mut entries);
            assert_eq!(got, 0);
            let slice = if count == 0 || entries.is_null() {
                &[]
            } else {
                std::slice::from_raw_parts(entries, count as usize)
            };
            let found = slice.iter().any(|entry| {
                entry.Trustee.TrusteeForm == TRUSTEE_IS_SID
                    && !entry.Trustee.ptstrName.is_null()
                    && EqualSid(entry.Trustee.ptstrName.cast(), sid) != 0
            });
            if !entries.is_null() {
                LocalFree(entries.cast());
            }
            if !descriptor.is_null() {
                LocalFree(descriptor);
            }
            found
        }
    }
    #[test]
    fn failed_process_creation_never_marks_the_sandbox_started() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let leases = root.path().join("leases");
        std::fs::create_dir(&workspace).unwrap();
        let mut profile = Profile::new_in(&leases, &workspace, &[]).unwrap();
        let marker = profile.temp_path().join("status");
        std::fs::write(marker.with_extension("owned"), "owned").unwrap();
        let err = worker(
            &profile.name,
            &workspace,
            &root.path().join("nonexistent.exe"),
            &marker,
            &[],
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("shellSandbox.readOnlyPaths"), "{message}");
        assert!(message.contains("nonexistent.exe"), "{message}");
        assert!(!marker.exists());
        profile.close().unwrap();
    }
    #[test]
    fn orphan_cleanup_revokes_workspace_acl_and_deletes_crashed_profile() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let leases = root.path().join("leases");
        std::fs::create_dir(&workspace).unwrap();
        let mut profile = Profile::new_in(&leases, &workspace, &[]).unwrap();
        let name = profile.name.clone();
        let sid = profile.sid;
        assert!(dacl_contains_sid(&workspace, sid));
        assert_eq!(cleanup_orphans_in(&leases).unwrap(), 0);

        // Simulate host hard-kill: OS closes handles without running Profile::close().
        let temp = profile._temporary.take();
        let lock = profile.lease_lock.take();
        drop(lock);
        drop(temp);
        std::mem::forget(profile);

        assert_eq!(cleanup_orphans_in(&leases).unwrap(), 1);
        assert!(!dacl_contains_sid(&workspace, sid));
        unsafe {
            FreeSid(sid);
            // Recreating the same profile name succeeds only because cleanup deleted it.
            let mut probe_sid = ptr::null_mut();
            let text = wide(&name);
            let recreated = CreateAppContainerProfile(
                text.as_ptr(),
                text.as_ptr(),
                text.as_ptr(),
                ptr::null(),
                0,
                &mut probe_sid,
            );
            assert!(
                recreated >= 0,
                "crashed profile was not deleted: 0x{:08x}",
                recreated as u32
            );
            FreeSid(probe_sid);
            assert!(DeleteAppContainerProfile(text.as_ptr()) >= 0);
        }
        assert_eq!(fs::read_dir(&leases).unwrap().count(), 0);
    }
    #[test]
    fn orphan_cleanup_skips_active_profile() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let leases = root.path().join("leases");
        std::fs::create_dir(&workspace).unwrap();
        let mut profile = Profile::new_in(&leases, &workspace, &[]).unwrap();
        assert_eq!(cleanup_orphans_in(&leases).unwrap(), 0);
        assert!(dacl_contains_sid(&workspace, profile.sid));
        profile.close().unwrap();
        assert!(!dacl_contains_sid(&workspace, profile.sid));
        assert_eq!(fs::read_dir(&leases).unwrap().count(), 0);
    }
    #[test]
    fn command_line_quoting_preserves_quotes_spaces_and_trailing_backslashes() {
        assert_eq!(quote("a b"), "\"a b\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("a\\"), "\"a\\\\\"");
    }
    #[test]
    fn read_only_paths_acl_is_granted_revoked_and_errors_reference_setting() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let leases = root.path().join("leases");
        let readonly = root.path().join("readonly");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&readonly).unwrap();
        let mut profile =
            Profile::new_in(&leases, &workspace, std::slice::from_ref(&readonly)).unwrap();
        assert!(dacl_contains_sid(&readonly, profile.sid));
        profile.close().unwrap();
        assert!(!dacl_contains_sid(&readonly, profile.sid));

        let err =
            match Profile::new_in(&leases, &workspace, &[root.path().join("missing-readonly")]) {
                Ok(_) => panic!("missing read-only root must fail"),
                Err(err) => err,
            };
        assert!(
            err.to_string().contains("shellSandbox.readOnlyPaths"),
            "{err}"
        );
    }
}
