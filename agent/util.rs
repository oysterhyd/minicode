use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}
pub fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
pub fn data_dir() -> PathBuf {
    home().join(".minicode")
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn is_link(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 && fs::read_link(path).is_ok()
    }
    #[cfg(not(windows))]
    false
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
#[cfg(test)]
thread_local! {
    static CANONICAL_CALLS: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))};
}
fn path_metadata(path: &Path) -> std::io::Result<fs::Metadata> {
    #[cfg(test)]
    CANONICAL_CALLS.with(|c| {
        let (m, r) = c.get();
        c.set((m + 1, r));
    });
    fs::symlink_metadata(path)
}
fn path_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(test)]
    CANONICAL_CALLS.with(|c| {
        let (m, r) = c.get();
        c.set((m, r + 1));
    });
    fs::canonicalize(path)
}
pub fn canonical(path: &Path) -> Result<PathBuf> {
    // Resolve a whole existing prefix, but preserve link/.. semantics rather
    // than letting Win32 normalize parent components before following links.
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    let mut dirty = false;
    for component in full.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if dirty {
                    resolved = resolve_prefix(&resolved)?;
                    dirty = false;
                }
                if !resolved.pop() {
                    bail!("path has no parent")
                }
            }
            std::path::Component::Normal(part) => {
                resolved.push(part);
                dirty = true;
            }
            _ => resolved.push(component.as_os_str()),
        }
    }
    if dirty {
        resolved = resolve_prefix(&resolved)?;
    }
    #[cfg(windows)]
    if let Some(raw) = resolved.to_str() {
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{unc}")));
        }
        if let Some(dos) = raw.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(dos));
        }
    }
    Ok(resolved)
}

fn resolve_prefix(path: &Path) -> Result<PathBuf> {
    let mut prefix = path;
    let mut missing = vec![];
    loop {
        match path_metadata(prefix) {
            Ok(_) => {
                let mut resolved = path_canonicalize(prefix)?;
                for part in missing.into_iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                missing.push(
                    prefix
                        .file_name()
                        .context("path has no existing prefix")?
                        .to_os_string(),
                );
                prefix = prefix.parent().context("path has no existing parent")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod canonical_tests {
    use super::*;
    #[test]
    fn u5_existing_prefix_is_resolved_once_and_missing_work_is_linear() {
        let dir = tempfile::tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        for i in 0..32 {
            deep.push(format!("d{i}"));
        }
        fs::create_dir_all(&deep).unwrap();
        CANONICAL_CALLS.with(|c| c.set((0, 0)));
        let resolved = canonical(&deep).unwrap();
        let (metadata, resolves) = CANONICAL_CALLS.with(|c| c.get());
        assert_eq!(resolves, 1, "repeatedly resolved existing path");
        assert!(metadata <= 1);
        let missing = deep.join("a/b/c/d/e/f");
        CANONICAL_CALLS.with(|c| c.set((0, 0)));
        assert_eq!(canonical(&missing).unwrap(), resolved.join("a/b/c/d/e/f"));
        let (metadata, resolves) = CANONICAL_CALLS.with(|c| c.get());
        assert_eq!(resolves, 1);
        assert!(metadata <= 7);
    }

    fn link_directory(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let out = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(link.to_string_lossy().replace('/', "\\"))
                .arg(target.to_string_lossy().replace('/', "\\"))
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        }
    }

    #[test]
    fn u5_preserves_links_before_parent_components_and_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let target = dir.path().join("outside/目标");
        fs::create_dir(&repo).unwrap();
        fs::create_dir_all(&target).unwrap();
        let link = repo.join("alias");
        link_directory(&link, &target);
        assert_eq!(
            canonical(&link.join("..")).unwrap(),
            canonical(target.parent().unwrap()).unwrap()
        );
        assert_eq!(
            canonical(&link.join("missing/../new.txt")).unwrap(),
            canonical(&target).unwrap().join("new.txt")
        );
        assert!(in_workspace(&repo, "alias/new.txt").is_err());
        assert!(in_workspace(&repo, "alias/../private.txt").is_err());
        assert_eq!(
            canonical(&repo.join("missing/../inside")).unwrap(),
            canonical(&repo).unwrap().join("inside")
        );
        assert_eq!(
            canonical(&repo.join("./nested/../../repo/inside")).unwrap(),
            canonical(&repo).unwrap().join("inside")
        );
        let broken_target = dir.path().join("temporary-target");
        fs::create_dir(&broken_target).unwrap();
        let broken = repo.join("broken");
        link_directory(&broken, &broken_target);
        fs::remove_dir(&broken_target).unwrap();
        assert!(canonical(&broken.join("missing")).is_err());
    }

    fn slow_reference(path: &Path) -> Result<PathBuf> {
        let mut resolved = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    resolved.pop();
                }
                std::path::Component::Normal(part) => {
                    resolved.push(part);
                    if path_metadata(&resolved).is_ok() {
                        resolved = path_canonicalize(&resolved)?;
                    }
                }
                _ => resolved.push(component.as_os_str()),
            }
        }
        Ok(resolved)
    }
    #[test]
    #[ignore = "manual path-depth and syscall benchmark"]
    fn u5_path_depth_benchmark() {
        let dir = tempfile::tempdir().unwrap();
        let mut path = dir.path().to_path_buf();
        let depth = std::env::var("MINICODE_CANONICAL_BENCH_DEPTH")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(32)
            .clamp(1, 100);
        let iterations = std::env::var("MINICODE_CANONICAL_BENCH_ITERATIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(100)
            .clamp(1, 1000);
        let baseline =
            std::env::var("MINICODE_CANONICAL_BENCH_MODE").is_ok_and(|s| s == "baseline");
        for i in 0..depth {
            path.push(format!("d{i}"));
            fs::create_dir(&path).unwrap();
        }
        CANONICAL_CALLS.with(|c| c.set((0, 0)));
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(
                if baseline {
                    slow_reference(&path)
                } else {
                    canonical(&path)
                }
                .unwrap(),
            );
        }
        println!(
            "U5 baseline={baseline} depth={depth} iterations={iterations} calls={:?} time={:?}",
            CANONICAL_CALLS.with(|c| c.get()),
            start.elapsed()
        );
    }
}
pub fn in_workspace(workspace: &Path, raw: &str) -> Result<PathBuf> {
    if raw.contains('\0') {
        bail!("invalid path")
    }
    #[cfg(windows)]
    if raw.split(['/', '\\']).any(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        part.contains(':') && part.len() != 2
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
    }) {
        bail!("unsafe Windows path")
    }
    let root = canonical(workspace)?;
    let path = Path::new(raw);
    let target = canonical(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })?;
    if !target.starts_with(&root) {
        bail!("path escapes workspace: {raw}")
    }
    Ok(target)
}
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(content)?;
    file.as_file().sync_all()?;
    #[cfg(not(windows))]
    if let Ok(metadata) = path.metadata() {
        file.as_file().set_permissions(metadata.permissions())?;
    }
    #[cfg(windows)]
    {
        // ReplaceFile opens the replacement exclusively; close our file handle
        // but retain a TempPath guard until the replacement is complete.
        let temp = file.into_temp_path();
        let path =
            fs::canonicalize(parent)?.join(path.file_name().context("path has no filename")?);
        for attempt in 0..=20 {
            match replace_windows(&temp, &path) {
                Ok(()) => return Ok(()),
                Err(error)
                    if attempt < 20
                        && matches!(error.raw_os_error(), Some(5 | 32 | 33 | 80 | 183)) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5 * (attempt + 1)));
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!()
    }
    #[cfg(not(windows))]
    {
        file.persist(path).map_err(|e| e.error)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[cfg(windows)]
fn replace_windows(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::*;
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<u16>>()
    };
    let source_wide = wide(source);
    let target_wide = wide(target);
    let target_file = match fs::File::open(target) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if unsafe {
                MoveFileExW(
                    source_wide.as_ptr(),
                    target_wide.as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            } != 0
            {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            };
        }
        Err(error) => return Err(error),
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(target_file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if info.nNumberOfLinks > 1 {
        return Err(std::io::Error::other(
            "atomic replacement would break hard links; write refused",
        ));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_READONLY != 0 {
        return Err(std::io::Error::from_raw_os_error(5));
    }
    let attributes = info.dwFileAttributes
        & (FILE_ATTRIBUTE_READONLY
            | FILE_ATTRIBUTE_HIDDEN
            | FILE_ATTRIBUTE_SYSTEM
            | FILE_ATTRIBUTE_ARCHIVE
            | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_TEMPORARY);
    if unsafe {
        SetFileAttributesW(
            source_wide.as_ptr(),
            if attributes == 0 {
                FILE_ATTRIBUTE_NORMAL
            } else {
                attributes
            },
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // A backup protects the original even for ReplaceFile's partial failures.
    let backup = tempfile::NamedTempFile::new_in(target.parent().unwrap())?.into_temp_path();
    fs::remove_file(&backup)?;
    let backup_wide = wide(&backup);
    if unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            source_wide.as_ptr(),
            backup_wide.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    } != 0
    {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if backup.exists() && (target.exists() || fs::hard_link(&backup, target).is_err()) {
        let retained = backup.keep().map_err(|e| e.error)?;
        return Err(std::io::Error::other(format!(
            "{error}; original retained at {}",
            retained.display()
        )));
    }
    Err(error)
}
pub fn prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
pub fn terminal_text(text: &str) -> String {
    command_text(text)
}
/// Plain command transcript: discard terminal controls and replace progress lines.
pub fn command_text(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut output = String::with_capacity(text.len());
    let mut line = 0;
    let mut carriage = false;
    while let Some(ch) = chars.next() {
        let control = if ch == '\u{1b}' {
            chars.next()
        } else {
            Some(ch)
        };
        match control {
            Some('[') if ch == '\u{1b}' => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
                continue;
            }
            Some('\u{9b}') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
                continue;
            }
            Some(']' | 'P' | 'X' | '^' | '_') if ch == '\u{1b}' => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || c == '\u{9c}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
                continue;
            }
            Some('\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || c == '\u{9c}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
                continue;
            }
            _ if ch == '\u{1b}' => {
                if control.is_some_and(|c| (' '..='/').contains(&c)) {
                    for c in chars.by_ref() {
                        if !(' '..='/').contains(&c) {
                            break;
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        match ch {
            '\r' => carriage = true,
            '\n' => {
                output.push('\n');
                line = output.len();
                carriage = false;
            }
            '\u{8}' => {
                if output.len() > line {
                    output.pop();
                }
            }
            c if !c.is_control() || c == '\t' => {
                if carriage {
                    output.truncate(line);
                    carriage = false;
                }
                output.push(c);
            }
            _ => {}
        }
    }
    output
}
pub fn suffix(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}
pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let text = suffix(text, max_bytes);
    let lines: Vec<_> = text.split_inclusive('\n').rev().take(max_lines).collect();
    if text.is_empty() {
        String::new()
    } else {
        lines
            .into_iter()
            .rev()
            .collect::<String>()
            .trim_end_matches(['\r', '\n'])
            .to_string()
    }
}
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    "node_modules",
    ".minicode",
    ".pytest_cache",
    ".ruff_cache",
];
pub fn walk(root: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
        })
        .filter_map(Result::ok)
}
pub fn bundled_path() -> Option<std::ffi::OsString> {
    let runtime = std::env::var_os("MINICODE_RESOURCES")
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(p).join("runtime"))
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        })?;
    if !runtime.join("node").is_dir() {
        return None;
    }
    let mut dirs = vec![runtime.join("node"), runtime.join("git/cmd")];
    if let Some(existing) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(dirs).ok()
}
pub fn string<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .with_context(|| format!("{key} must be a string"))
}
/// Readable text of a caught panic payload.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

#[cfg(all(test, unix))]
mod unix_atomic_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn u4_replacement_preserves_mode_and_persists_directory_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read(&path).unwrap(), b"new");
        // The accompanying syscall trace verifies file + parent fsync.
    }
}

#[cfg(all(test, windows))]
mod atomic_tests {
    use super::*;
    use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};

    #[test]
    fn u4_preserves_protected_dacl() {
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, GetFileSecurityW};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acl.txt");
        fs::write(&path, b"old").unwrap();
        let user = std::process::Command::new("whoami.exe").output().unwrap();
        assert!(user.status.success());
        let grant = format!("{}:(F)", String::from_utf8(user.stdout).unwrap().trim());
        let result = std::process::Command::new("icacls.exe")
            .arg(&path)
            .args(["/inheritance:r", "/grant:r", &grant])
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let dacl = || unsafe {
            let mut needed = 0;
            GetFileSecurityW(
                wide.as_ptr(),
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
            assert!(needed > 0);
            let mut data = vec![0u32; (needed as usize).div_ceil(4)];
            assert_ne!(
                GetFileSecurityW(
                    wide.as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    data.as_mut_ptr().cast(),
                    needed,
                    &mut needed
                ),
                0
            );
            data
        };
        let before = dacl();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(dacl(), before);
    }

    #[test]
    fn u4_readonly_failure_preserves_original_and_cleans_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly.txt");
        fs::write(&path, b"old").unwrap();
        let mut permissions = path.metadata().unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        assert!(atomic_write(&path, b"new").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(
                wide.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
            );
        }
    }

    #[test]
    fn u4_preserves_named_stream_creation_time_and_attributes() {
        use windows_sys::Win32::Storage::FileSystem::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("中文.txt");
        fs::write(&path, b"old").unwrap();
        let stream = path.with_extension("txt:notes");
        fs::write(&stream, b"retained stream").unwrap();
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        assert_ne!(
            unsafe {
                SetFileAttributesW(
                    wide.as_ptr(),
                    FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_ARCHIVE,
                )
            },
            0
        );
        let created = path.metadata().unwrap().creation_time();
        std::thread::sleep(std::time::Duration::from_millis(20));
        atomic_write(&path, b"new").unwrap();
        assert_eq!(fs::read(&stream).unwrap(), b"retained stream");
        assert_eq!(path.metadata().unwrap().creation_time(), created);
        assert_ne!(
            path.metadata().unwrap().file_attributes() & FILE_ATTRIBUTE_HIDDEN,
            0
        );
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn u4_refuses_to_silently_break_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        let link = dir.path().join("link.txt");
        fs::write(&path, b"original").unwrap();
        fs::hard_link(&path, &link).unwrap();
        assert!(atomic_write(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read(&link).unwrap(), b"original");
    }

    #[test]
    fn u4_retries_sharing_violation_until_reader_releases_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.txt");
        fs::write(&path, b"old").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            drop(lock);
        });
        atomic_write(&path, b"new").unwrap();
        release.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }
}
