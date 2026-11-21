use crate::util;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    digest: String,
    mode: u32,
}
#[derive(Clone, Serialize, Deserialize)]
struct Review {
    digest: String,
    round: u64,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    owner: String,
    parent: PathBuf,
    files: BTreeMap<String, Entry>,
    reviewed: Option<Review>,
    merged: bool,
}
struct State {
    bytes: Vec<u8>,
    mode: u32,
}
struct Change {
    path: String,
    before: Option<State>,
    after: Option<State>,
}

pub(crate) struct Workspace {
    pub id: String,
    root: PathBuf,
    manifest: Mutex<Manifest>,
    closed: AtomicBool,
}

impl Workspace {
    pub fn create(parent: &Path, storage: &Path, owner: &str, extra: &[PathBuf]) -> Result<Self> {
        let raw_parent = parent;
        let parent = util::canonical(parent)?;
        let container = storage.join("delegates");
        if util::canonical(&container)?.starts_with(&parent) {
            bail!("isolated child storage must be outside the parent workspace");
        }
        if util::is_link(&container) {
            bail!("refusing linked isolation storage");
        }
        fs::create_dir_all(&container)?;
        let temporary = tempfile::Builder::new()
            .prefix("creating-")
            .tempdir_in(&container)?;
        let id = util::id();
        let root = container.join(&id);
        fs::create_dir(temporary.path().join("base"))?;
        fs::create_dir(temporary.path().join("work"))?;
        let mut paths = source_files(&parent)?;
        for source in extra {
            if let Ok(relative) = source
                .strip_prefix(&parent)
                .or_else(|_| source.strip_prefix(raw_parent))
            {
                paths.push(
                    relative
                        .to_str()
                        .context("non-UTF-8 instruction path")?
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                );
            }
        }
        paths.sort();
        paths.dedup();
        let mut files = BTreeMap::new();
        let mut bytes = 0usize;
        for path in paths {
            let source = checked_target(&parent, &path)?;
            let Some(snapshot) = state(&source)? else {
                continue;
            };
            bytes = bytes
                .checked_add(snapshot.bytes.len())
                .context("snapshot size overflow")?;
            if files.len() >= 50_000 || bytes > 512 * 1024 * 1024 {
                bail!(
                    "isolated snapshot exceeds 50,000 files / 512 MiB; reduce ignored build outputs first"
                );
            }
            for tree in ["base", "work"] {
                let target = temporary.path().join(tree).join(&path);
                fs::create_dir_all(target.parent().unwrap())?;
                fs::write(&target, &snapshot.bytes)?;
                set_mode(&target, snapshot.mode)?;
            }
            let current = state_digest(&snapshot);
            if state(&source)?.as_ref().map(state_digest) != Some(current.clone()) {
                bail!("parent file changed during snapshot: {path}");
            }
            files.insert(
                path,
                Entry {
                    digest: current,
                    mode: snapshot.mode,
                },
            );
        }
        let manifest = Manifest {
            owner: owner.into(),
            parent,
            files,
            reviewed: None,
            merged: false,
        };
        util::atomic_write(
            &temporary.path().join("plan.json"),
            &serde_json::to_vec(&manifest)?,
        )?;
        fs::rename(temporary.path(), &root)?;
        // The temporary guard now refers to an absent path. The snapshot is
        // durable until its owner explicitly merges, discards or closes it.
        Ok(Self {
            id,
            root,
            manifest: Mutex::new(manifest),
            closed: AtomicBool::new(false),
        })
    }
    pub fn load(storage: &Path, owner: &str, parent: &Path, id: &str) -> Result<Option<Self>> {
        if !util::valid_id(id) {
            bail!("invalid isolation id");
        }
        let root = storage.join("delegates").join(id);
        if !root.try_exists()? {
            return Ok(None);
        }
        validate_root(&root)?;
        let manifest: Manifest = serde_json::from_slice(&fs::read(root.join("plan.json"))?)?;
        if manifest.owner != owner || manifest.parent != util::canonical(parent)? {
            bail!("isolation does not belong to this parent session");
        }
        Ok(Some(Self {
            id: id.into(),
            root,
            manifest: Mutex::new(manifest),
            closed: AtomicBool::new(false),
        }))
    }
    pub fn work(&self) -> PathBuf {
        self.root.join("work")
    }
    pub fn metadata(&self) -> serde_json::Value {
        let m = self.manifest.lock().unwrap();
        json!({"id":self.id,"owner":m.owner,"parent_workspace":m.parent})
    }
    pub fn check_child_path(&self, relative: &str) -> Result<()> {
        checked_target(&self.work(), relative).map(|_| ())
    }
    pub fn active(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }
    fn save(&self, manifest: &Manifest) -> Result<()> {
        util::atomic_write(&self.root.join("plan.json"), &serde_json::to_vec(manifest)?)
    }
    fn changes(&self, manifest: &Manifest) -> Result<Vec<Change>> {
        validate_root(&self.root)?;
        let mut paths = manifest.files.keys().cloned().collect::<BTreeSet<_>>();
        paths.extend(work_files(&self.work())?);
        let mut changes = vec![];
        for path in paths {
            let before = if let Some(expected) = manifest.files.get(&path) {
                let state = state(&checked_target(&self.root.join("base"), &path)?)?
                    .context("snapshot base is missing")?;
                if state_digest(&state) != expected.digest || state.mode != expected.mode {
                    bail!("snapshot base changed: {path}");
                }
                Some(state)
            } else {
                None
            };
            let after = state(&checked_target(&self.work(), &path)?)?;
            if !same(before.as_ref(), after.as_ref()) {
                changes.push(Change {
                    path,
                    before,
                    after,
                });
            }
        }
        Ok(changes)
    }
    pub fn inspect(&self, round: u64) -> Result<String> {
        let mut manifest = self.manifest.lock().unwrap();
        if manifest.merged {
            bail!("changes already merged; discard to retry cleanup");
        }
        let changes = self.changes(&manifest)?;
        let digest = changes_digest(&changes);
        let mut output = format!(
            "独立子任务改动：{} 个文件。主工作区尚未修改。\n",
            changes.len()
        );
        for change in &changes {
            let old = change
                .before
                .as_ref()
                .map(|s| s.bytes.as_slice())
                .unwrap_or_default();
            let new = change
                .after
                .as_ref()
                .map(|s| s.bytes.as_slice())
                .unwrap_or_default();
            match (std::str::from_utf8(old), std::str::from_utf8(new)) {
                (Ok(old), Ok(new)) => {
                    output.push_str(
                        &similar::TextDiff::configure()
                            .algorithm(similar::Algorithm::Patience)
                            .timeout(std::time::Duration::from_secs(1))
                            .diff_lines(old, new)
                            .unified_diff()
                            .header(&format!("a/{}", change.path), &format!("b/{}", change.path))
                            .to_string(),
                    );
                    if old == new {
                        output.push_str(&format!("{}: 文件权限发生变化\n", change.path));
                    }
                }
                _ => output.push_str(&format!("{}: 二进制文件发生变化\n", change.path)),
            }
        }
        let previous = manifest.reviewed.clone();
        manifest.reviewed = Some(Review { digest, round });
        if let Err(error) = self.save(&manifest) {
            manifest.reviewed = previous;
            return Err(error);
        }
        Ok(output)
    }
    pub fn ready_to_merge(&self, round: u64) -> Result<usize> {
        let manifest = self.manifest.lock().unwrap();
        let changes = self.changes(&manifest)?;
        check_review(&manifest, &changes, round)?;
        Ok(changes.len())
    }
    pub fn merge(&self, round: u64) -> Result<usize> {
        self.merge_with(round, |_| Ok(()))
    }
    fn merge_with(
        &self,
        round: u64,
        mut before_write: impl FnMut(usize) -> Result<()>,
    ) -> Result<usize> {
        let mut manifest = self.manifest.lock().unwrap();
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.parent().unwrap().join(format!(
                "merge-{}.lock",
                util::hash(manifest.parent.to_string_lossy().as_bytes())
            )))?;
        lock.lock_exclusive()?;
        let changes = self.changes(&manifest)?;
        check_review(&manifest, &changes, round)?;
        // Validate every path before the first write. A dirty parent is valid
        // when it still matches the snapshot; a newer external edit is not.
        for change in &changes {
            let target = checked_target(&manifest.parent, &change.path)?;
            if !same(state(&target)?.as_ref(), change.before.as_ref()) {
                bail!(
                    "merge conflict: parent changed since delegation: {}",
                    change.path
                );
            }
        }
        let mut created_dirs = vec![];
        let mut attempted = 0;
        let apply = (|| -> Result<()> {
            for (index, change) in changes.iter().enumerate() {
                attempted = index + 1;
                before_write(index)?;
                let target = checked_target(&manifest.parent, &change.path)?;
                if !same(state(&target)?.as_ref(), change.before.as_ref()) {
                    bail!("merge conflict during apply: {}", change.path);
                }
                let mut directory = target.parent().unwrap();
                while directory != manifest.parent && !directory.exists() {
                    created_dirs.push(directory.to_path_buf());
                    directory = directory.parent().unwrap();
                }
                replace(&target, change.after.as_ref())?;
            }
            manifest.merged = true;
            if let Err(error) = self.save(&manifest) {
                manifest.merged = false;
                return Err(error);
            }
            Ok(())
        })();
        if let Err(error) = apply {
            let mut failures = vec![];
            for change in changes[..attempted].iter().rev() {
                let restore = (|| -> Result<()> {
                    let target = checked_target(&manifest.parent, &change.path)?;
                    let current = state(&target)?;
                    if same(current.as_ref(), change.before.as_ref()) {
                        return Ok(());
                    }
                    if current.as_ref().map(state_digest) != change.after.as_ref().map(state_digest)
                    {
                        bail!("rollback conflict: {}", change.path)
                    }
                    replace(&target, change.before.as_ref())
                })();
                if let Err(error) = restore {
                    failures.push(error.to_string())
                }
            }
            created_dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
            created_dirs.dedup();
            for directory in created_dirs {
                if directory.is_dir() && fs::read_dir(&directory)?.next().is_none() {
                    fs::remove_dir(&directory)?
                }
            }
            if !failures.is_empty() {
                bail!(
                    "merge failed: {error}; rollback needs review: {}",
                    failures.join("; ")
                )
            }
            return Err(error);
        }
        Ok(changes.len())
    }
    pub fn discard(&self) -> Result<()> {
        if !self.active() {
            return Ok(());
        }
        if self.root.try_exists()? {
            validate_root(&self.root)?;
            #[cfg(windows)]
            for entry in walkdir::WalkDir::new(&self.root).follow_links(false) {
                let entry = entry?;
                if entry.file_type().is_file() && !util::is_link(entry.path()) {
                    clear_windows_readonly(entry.path())?;
                }
            }
            fs::remove_dir_all(&self.root)?;
        }
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn check_review(manifest: &Manifest, changes: &[Change], round: u64) -> Result<()> {
    if manifest.merged {
        bail!("changes already merged")
    }
    let review = manifest
        .reviewed
        .as_ref()
        .context("inspect the child diff before merging")?;
    if review.round >= round {
        bail!("review results must reach the parent before a later model response can merge")
    }
    if review.digest != changes_digest(changes) {
        bail!("child diff changed since review; inspect it again")
    }
    Ok(())
}
fn changes_digest(changes: &[Change]) -> String {
    util::hash(serde_json::to_vec(&changes.iter().map(|c|json!({"path":c.path,"before":c.before.as_ref().map(state_digest),"before_mode":c.before.as_ref().map(|s|s.mode),"after":c.after.as_ref().map(state_digest),"after_mode":c.after.as_ref().map(|s|s.mode)})).collect::<Vec<_>>()).unwrap().as_slice())
}
fn validate_root(root: &Path) -> Result<()> {
    let container = root.parent().context("isolation has no container")?;
    if util::is_link(container)
        || util::is_link(root)
        || util::is_link(&root.join("base"))
        || util::is_link(&root.join("work"))
    {
        bail!("refusing linked isolation directories")
    }
    if util::canonical(root)?.parent() != Some(util::canonical(container)?.as_path()) {
        bail!("isolation path escaped storage")
    }
    Ok(())
}
fn checked_target(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || path
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case(".git"))
    {
        bail!("invalid isolated file path: {relative}")
    }
    #[cfg(windows)]
    if relative
        .split(['/', '\\'])
        .any(|s| s.contains(':') || s.ends_with([' ', '.']))
    {
        bail!("invalid Windows isolated file path")
    }
    let mut current = root.to_path_buf();
    for component in path.components() {
        current.push(component);
        if util::is_link(&current) {
            bail!("refusing linked merge path: {relative}")
        }
    }
    let target = util::in_workspace(root, relative)?;
    let canonical_root = util::canonical(root)?;
    if target
        .strip_prefix(&canonical_root)?
        .components()
        .any(|component| component.as_os_str().eq_ignore_ascii_case(".git"))
    {
        bail!("refusing Git metadata through a path alias: {relative}");
    }
    Ok(target)
}
fn source_files(root: &Path) -> Result<Vec<String>> {
    let mut command = std::process::Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let result = command
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=",
            "-C",
        ])
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ".",
        ])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output();
    if let Ok(output) = result
        && output.status.success()
    {
        let mut paths = vec![];
        for raw in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
        {
            let path = std::str::from_utf8(raw)?.to_string();
            let target = root.join(&path);
            if !util::is_link(&target) && target.is_file() {
                paths.push(path)
            }
        }
        return Ok(paths);
    }
    let mut paths = vec![];
    for entry in ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
    {
        let entry = entry?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) && !util::is_link(entry.path()) {
            paths.push(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF-8 snapshot path")?
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            )
        }
    }
    Ok(paths)
}
fn work_files(root: &Path) -> Result<Vec<String>> {
    let mut paths = vec![];
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if util::is_link(entry.path()) {
            bail!("child created a link; review it manually")
        }
        if entry.file_type().is_file() {
            let path = entry
                .path()
                .strip_prefix(root)?
                .to_str()
                .context("non-UTF-8 child path")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            checked_target(root, &path)?;
            paths.push(path)
        }
    }
    Ok(paths)
}
fn state(path: &Path) -> Result<Option<State>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if util::is_link(path) || !metadata.is_file() {
        bail!("not a regular isolated file: {}", path.display())
    }
    if metadata.len() > 64 * 1024 * 1024 {
        bail!("isolated file exceeds 64 MiB: {}", path.display())
    }
    Ok(Some(State {
        bytes: fs::read(path)?,
        mode: mode(&metadata),
    }))
}
fn state_digest(state: &State) -> String {
    util::hash(&state.bytes)
}
fn same(a: Option<&State>, b: Option<&State>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.bytes == b.bytes && a.mode == b.mode,
        _ => false,
    }
}
fn replace(path: &Path, value: Option<&State>) -> Result<()> {
    if let Some(value) = value {
        util::atomic_write(path, &value.bytes)?;
        set_mode(path, value.mode)?
    } else if path.exists() {
        fs::remove_file(path)?
    }
    Ok(())
}
fn mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(windows)]
    {
        u32::from(metadata.permissions().readonly())
    }
}
#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)] // Windows file attribute; never applies Unix permission bits.
fn clear_windows_readonly(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(mode != 0);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("repo");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("old.txt"), "dirty original").unwrap();
        let work = Workspace::create(&parent, dir.path(), "owner", &[]).unwrap();
        (dir, work)
    }
    #[test]
    fn snapshot_is_independent_persistent_and_requires_a_current_review() {
        let (dir, work) = fixture();
        fs::write(work.work().join("old.txt"), "child").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("repo/old.txt")).unwrap(),
            "dirty original"
        );
        assert!(work.merge(2).is_err());
        assert!(work.inspect(2).unwrap().contains("dirty original"));
        assert!(work.merge(2).is_err());
        let restored = Workspace::load(dir.path(), "owner", &dir.path().join("repo"), &work.id)
            .unwrap()
            .unwrap();
        assert!(Workspace::load(dir.path(), "other", &dir.path().join("repo"), &work.id).is_err());
        fs::write(work.work().join("old.txt"), "changed after review").unwrap();
        assert!(restored.merge(3).is_err());
        restored.inspect(3).unwrap();
        restored.merge(4).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("repo/old.txt")).unwrap(),
            "changed after review"
        );
        restored.discard().unwrap();
    }
    #[test]
    fn merge_failure_rolls_back_prior_writes_and_new_directories() {
        let (dir, work) = fixture();
        fs::create_dir(work.work().join("a")).unwrap();
        fs::write(work.work().join("a/new.txt"), "new").unwrap();
        fs::write(work.work().join("old.txt"), "child").unwrap();
        work.inspect(1).unwrap();
        assert!(
            work.merge_with(2, |index| if index == 1 {
                bail!("simulated disk failure")
            } else {
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("repo/old.txt")).unwrap(),
            "dirty original"
        );
        assert!(!dir.path().join("repo/a").exists());
        work.discard().unwrap();
    }
    #[test]
    fn creation_failure_leaves_no_partial_snapshot_and_no_parent_changes() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("repo");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("a"), "same").unwrap();
        assert!(
            Workspace::create(&parent, dir.path(), "owner", &[parent.join("../outside")]).is_err()
        );
        assert_eq!(
            fs::read_dir(dir.path().join("delegates")).unwrap().count(),
            0
        );
        assert_eq!(fs::read_to_string(parent.join("a")).unwrap(), "same");
    }

    fn link_directory(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(link.to_string_lossy().replace('/', "\\"))
                .arg(target.to_string_lossy().replace('/', "\\"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "junction {} -> {}: {} {}",
                link.display(),
                target.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).unwrap();
        }
    }
    fn unlink_directory(link: &Path) {
        #[cfg(windows)]
        {
            fs::remove_dir(link).unwrap();
        }
        #[cfg(unix)]
        {
            fs::remove_file(link).unwrap();
        }
    }
    #[test]
    fn linked_paths_and_cleanup_redirection_never_modify_the_parent() {
        let (dir, work) = fixture();
        fs::create_dir(work.work().join("newdir")).unwrap();
        fs::write(work.work().join("newdir/file"), "child").unwrap();
        work.inspect(1).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let redirect = dir.path().join("repo/newdir");
        link_directory(&redirect, &outside);
        assert!(work.merge(2).is_err());
        assert!(!outside.join("file").exists());
        unlink_directory(&redirect);
        let backup = work.root.with_extension("backup");
        fs::rename(&work.root, &backup).unwrap();
        link_directory(&work.root, &dir.path().join("repo"));
        assert!(work.discard().is_err());
        assert!(work.active());
        assert_eq!(
            fs::read_to_string(dir.path().join("repo/old.txt")).unwrap(),
            "dirty original"
        );
        unlink_directory(&work.root);
        fs::rename(backup, &work.root).unwrap();
        work.discard().unwrap();
    }
    #[test]
    fn git_snapshot_includes_dirty_tracked_and_untracked_files_without_git_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("repo");
        fs::create_dir(&parent).unwrap();
        let git = |arguments: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&parent)
                    .args(arguments)
                    .output()
                    .unwrap()
                    .status
                    .success()
            )
        };
        git(&["init", "-q"]);
        fs::write(parent.join("tracked"), "index").unwrap();
        git(&["add", "tracked"]);
        fs::write(parent.join("tracked"), "uncommitted").unwrap();
        fs::write(parent.join("untracked"), "new").unwrap();
        fs::write(parent.join(".gitignore"), "ignored\n").unwrap();
        fs::write(parent.join("ignored"), "build output").unwrap();
        let work = Workspace::create(&parent, dir.path(), "owner", &[]).unwrap();
        assert_eq!(
            fs::read_to_string(work.work().join("tracked")).unwrap(),
            "uncommitted"
        );
        assert!(work.work().join("untracked").exists());
        assert!(!work.work().join("ignored").exists());
        assert!(!work.work().join(".git").exists());
        work.discard().unwrap();
        assert_eq!(
            fs::read_to_string(parent.join("tracked")).unwrap(),
            "uncommitted"
        );
    }
}
