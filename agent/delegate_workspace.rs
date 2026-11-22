use crate::util;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SNAPSHOT_FILES: usize = 50_000;
const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const PACK_FILE_NAME: &str = "snapshot.pack";

#[cfg(test)]
thread_local! {
    static BASE_LOAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct PackSpan {
    offset: u64,
    len: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    digest: String,
    mode: u32,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    pack: Option<PackSpan>,
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
        let base_dir = temporary.path().join("base");
        let work_dir = temporary.path().join("work");
        fs::create_dir(&base_dir)?;
        fs::create_dir(&work_dir)?;
        let pack_path = base_dir.join(PACK_FILE_NAME);
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
        let mut snapshot_writer = SnapshotWriter::new(&pack_path, &work_dir)?;
        let mut files = BTreeMap::new();
        for path in paths {
            let source = checked_target(&parent, &path)?;
            let work_target = work_dir.join(&path);
            let Some(entry) =
                snapshot_writer.copy_entry(&source, &work_target, &path, files.len())?
            else {
                continue;
            };
            files.insert(path, entry);
        }
        snapshot_writer.finish(&pack_path)?;
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
        let max_pack_end = manifest
            .files
            .values()
            .filter_map(|entry| entry.pack.map(|span| span.offset.checked_add(span.len)))
            .collect::<Option<Vec<_>>>()
            .context("snapshot pack span overflow")?
            .into_iter()
            .max();
        if let Some(required_len) = max_pack_end {
            let pack_path = checked_target(&self.root.join("base"), PACK_FILE_NAME)?;
            let metadata = match fs::symlink_metadata(&pack_path) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    bail!("snapshot base is missing");
                }
                Err(e) => return Err(e.into()),
            };
            if util::is_link(&pack_path) || !metadata.is_file() {
                bail!("not a regular isolated file: {}", pack_path.display());
            }
            if metadata.len() < required_len {
                bail!("snapshot base changed: {PACK_FILE_NAME}");
            }
        }
        let work_root = self.work();
        let work_map = work_files(&work_root)?;
        let mut paths = manifest.files.keys().cloned().collect::<BTreeSet<_>>();
        paths.extend(work_map.keys().cloned());
        let mut hash_buf = vec![0u8; STREAM_BUFFER_BYTES];
        let mut pack_reader: Option<fs::File> = None;
        let mut changes = vec![];
        for path in paths {
            let work_path = match work_map.get(&path) {
                Some(existing) => existing.clone(),
                None => checked_target(&work_root, &path)?,
            };
            if let Some(expected) = manifest.files.get(&path) {
                match probe_work_file(&work_path, expected, &mut hash_buf)? {
                    WorkProbe::Unchanged => continue,
                    WorkProbe::Deleted => {
                        let before = self.load_base_state(&path, expected, &mut pack_reader)?;
                        changes.push(Change {
                            path,
                            before: Some(before),
                            after: None,
                        });
                    }
                    WorkProbe::Changed(after) => {
                        let before = self.load_base_state(&path, expected, &mut pack_reader)?;
                        if !same(Some(&before), Some(&after)) {
                            changes.push(Change {
                                path,
                                before: Some(before),
                                after: Some(after),
                            });
                        }
                    }
                }
            } else {
                let after = state(&work_path)?;
                if after.is_some() {
                    changes.push(Change {
                        path,
                        before: None,
                        after,
                    });
                }
            }
        }
        Ok(changes)
    }
    fn load_base_state(
        &self,
        path: &str,
        expected: &Entry,
        pack_reader: &mut Option<fs::File>,
    ) -> Result<State> {
        #[cfg(test)]
        BASE_LOAD_COUNT.with(|count| count.set(count.get() + 1));
        if let Some(span) = expected.pack {
            if span.len > MAX_FILE_BYTES {
                bail!("isolated file exceeds 64 MiB: {path}");
            }
            if pack_reader.is_none() {
                let pack_path = checked_target(&self.root.join("base"), PACK_FILE_NAME)?;
                let metadata = match fs::symlink_metadata(&pack_path) {
                    Ok(m) => m,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        bail!("snapshot base is missing");
                    }
                    Err(e) => return Err(e.into()),
                };
                if util::is_link(&pack_path) || !metadata.is_file() {
                    bail!("not a regular isolated file: {}", pack_path.display());
                }
                *pack_reader = Some(fs::File::open(&pack_path)?);
            }
            let file = pack_reader.as_mut().unwrap();
            let pack_len = file.metadata()?.len();
            let end = span
                .offset
                .checked_add(span.len)
                .context("snapshot pack span overflow")?;
            if end > pack_len {
                bail!("snapshot base changed: {path}");
            }
            file.seek(SeekFrom::Start(span.offset))?;
            let mut bytes = vec![0u8; span.len as usize];
            file.read_exact(&mut bytes)?;
            let loaded = State {
                bytes,
                mode: expected.mode,
            };
            if state_digest(&loaded) != expected.digest {
                bail!("snapshot base changed: {path}");
            }
            Ok(loaded)
        } else {
            let loaded = state(&checked_target(&self.root.join("base"), path)?)?
                .context("snapshot base is missing")?;
            if state_digest(&loaded) != expected.digest || loaded.mode != expected.mode {
                bail!("snapshot base changed: {path}");
            }
            Ok(loaded)
        }
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
struct SnapshotWriter {
    writer: BufWriter<fs::File>,
    pack_offset: u64,
    packed: HashMap<String, PackSpan>,
    created_work_dirs: HashSet<PathBuf>,
    buf: Vec<u8>,
    bytes: u64,
}

impl SnapshotWriter {
    fn new(pack_path: &Path, work_root: &Path) -> Result<Self> {
        let mut created_work_dirs = HashSet::new();
        created_work_dirs.insert(work_root.to_path_buf());
        Ok(Self {
            writer: BufWriter::new(fs::File::create(pack_path)?),
            pack_offset: 0,
            packed: HashMap::new(),
            created_work_dirs,
            buf: vec![0u8; STREAM_BUFFER_BYTES],
            bytes: 0,
        })
    }

    fn copy_entry(
        &mut self,
        source: &Path,
        work_target: &Path,
        relative: &str,
        file_count: usize,
    ) -> Result<Option<Entry>> {
        let pre_meta = match fs::symlink_metadata(source) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if util::is_link(source) || !pre_meta.is_file() {
            bail!("not a regular isolated file: {}", source.display());
        }
        if pre_meta.len() > MAX_FILE_BYTES {
            bail!("isolated file exceeds 64 MiB: {}", source.display());
        }
        let expected_total = self
            .bytes
            .checked_add(pre_meta.len())
            .context("snapshot size overflow")?;
        if file_count >= MAX_SNAPSHOT_FILES || expected_total > MAX_SNAPSHOT_BYTES {
            bail!(
                "isolated snapshot exceeds 50,000 files / 512 MiB; reduce ignored build outputs first"
            );
        }
        let mut reader = match fs::File::open(source) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!("parent file changed during snapshot: {relative}");
            }
            Err(e) => return Err(e.into()),
        };
        let open_meta = reader.metadata()?;
        if !open_meta.is_file() {
            bail!("not a regular isolated file: {}", source.display());
        }
        if open_meta.len() > MAX_FILE_BYTES {
            bail!("isolated file exceeds 64 MiB: {}", source.display());
        }
        let file_mode = mode(&open_meta);
        if let Some(parent_dir) = work_target.parent()
            && self.created_work_dirs.insert(parent_dir.to_path_buf())
        {
            fs::create_dir_all(parent_dir)?;
        }
        let mut work_file = fs::File::create(work_target)?;

        let mut first_len = 0usize;
        let mut hit_eof = false;
        while first_len < self.buf.len() {
            let n = reader.read(&mut self.buf[first_len..])?;
            if n == 0 {
                hit_eof = true;
                break;
            }
            first_len += n;
        }

        let (digest, copied, span) = if hit_eof {
            let copied = first_len as u64;
            let next_bytes = self
                .bytes
                .checked_add(copied)
                .context("snapshot size overflow")?;
            if next_bytes > MAX_SNAPSHOT_BYTES {
                bail!(
                    "isolated snapshot exceeds 50,000 files / 512 MiB; reduce ignored build outputs first"
                );
            }
            let chunk = &self.buf[..first_len];
            let digest = format!("{:x}", Sha256::digest(chunk));
            if first_len > 0 {
                work_file.write_all(chunk)?;
            }
            let span = if let Some(&existing) = self.packed.get(&digest) {
                existing
            } else {
                if first_len > 0 {
                    self.writer.write_all(chunk)?;
                }
                let span = PackSpan {
                    offset: self.pack_offset,
                    len: copied,
                };
                self.pack_offset = self
                    .pack_offset
                    .checked_add(copied)
                    .context("snapshot pack offset overflow")?;
                self.packed.insert(digest.clone(), span);
                span
            };
            (digest, copied, span)
        } else {
            let start_offset = self.pack_offset;
            let mut hasher = Sha256::new();
            let first_chunk = &self.buf[..first_len];
            hasher.update(first_chunk);
            work_file.write_all(first_chunk)?;
            self.writer.write_all(first_chunk)?;
            let mut copied = first_len as u64;
            loop {
                let n = reader.read(&mut self.buf)?;
                if n == 0 {
                    break;
                }
                copied = copied
                    .checked_add(n as u64)
                    .context("snapshot size overflow")?;
                if copied > MAX_FILE_BYTES {
                    bail!("isolated file exceeds 64 MiB: {}", source.display());
                }
                let next_bytes = self
                    .bytes
                    .checked_add(copied)
                    .context("snapshot size overflow")?;
                if next_bytes > MAX_SNAPSHOT_BYTES {
                    bail!(
                        "isolated snapshot exceeds 50,000 files / 512 MiB; reduce ignored build outputs first"
                    );
                }
                let chunk = &self.buf[..n];
                hasher.update(chunk);
                work_file.write_all(chunk)?;
                self.writer.write_all(chunk)?;
            }
            let digest = format!("{:x}", hasher.finalize());
            let span = if let Some(&existing) = self.packed.get(&digest) {
                self.writer.flush()?;
                self.writer.get_ref().set_len(start_offset)?;
                self.writer.seek(SeekFrom::Start(start_offset))?;
                existing
            } else {
                let span = PackSpan {
                    offset: start_offset,
                    len: copied,
                };
                self.pack_offset = start_offset
                    .checked_add(copied)
                    .context("snapshot pack offset overflow")?;
                self.packed.insert(digest.clone(), span);
                span
            };
            (digest, copied, span)
        };

        drop(work_file);
        drop(reader);
        #[cfg(unix)]
        set_mode(work_target, file_mode)?;
        #[cfg(windows)]
        if file_mode != 0 {
            set_mode(work_target, file_mode)?;
        }

        let post_meta = match fs::symlink_metadata(source) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!("parent file changed during snapshot: {relative}");
            }
            Err(e) => return Err(e.into()),
        };
        if util::is_link(source)
            || !post_meta.is_file()
            || post_meta.len() != copied
            || mode(&post_meta) != file_mode
            || !same_file_identity(&open_meta, &post_meta)
        {
            bail!("parent file changed during snapshot: {relative}");
        }

        self.bytes = self
            .bytes
            .checked_add(copied)
            .context("snapshot size overflow")?;
        Ok(Some(Entry {
            digest,
            mode: file_mode,
            size: Some(copied),
            pack: Some(span),
        }))
    }

    fn finish(mut self, pack_path: &Path) -> Result<()> {
        self.writer.flush()?;
        drop(self.writer);
        lock_readonly(pack_path)
    }
}

enum WorkProbe {
    Unchanged,
    Deleted,
    Changed(State),
}

fn probe_work_file(work_path: &Path, expected: &Entry, buf: &mut [u8]) -> Result<WorkProbe> {
    let metadata = match fs::symlink_metadata(work_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(WorkProbe::Deleted),
        Err(e) => return Err(e.into()),
    };
    if util::is_link(work_path) || !metadata.is_file() {
        bail!("not a regular isolated file: {}", work_path.display());
    }
    if metadata.len() > MAX_FILE_BYTES {
        bail!("isolated file exceeds 64 MiB: {}", work_path.display());
    }
    let work_mode = mode(&metadata);
    if work_mode != expected.mode || expected.size.is_some_and(|size| size != metadata.len()) {
        return match state(work_path)? {
            Some(after) => Ok(WorkProbe::Changed(after)),
            None => Ok(WorkProbe::Deleted),
        };
    }
    let mut reader = match fs::File::open(work_path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(WorkProbe::Deleted),
        Err(e) => return Err(e.into()),
    };
    let open_meta = reader.metadata()?;
    if !open_meta.is_file() || open_meta.len() > MAX_FILE_BYTES {
        bail!("not a regular isolated file: {}", work_path.display());
    }
    let mut first_len = 0usize;
    let mut hit_eof = false;
    while first_len < buf.len() {
        let n = reader.read(&mut buf[first_len..])?;
        if n == 0 {
            hit_eof = true;
            break;
        }
        first_len += n;
    }
    if hit_eof {
        let digest = format!("{:x}", Sha256::digest(&buf[..first_len]));
        if digest == expected.digest {
            return Ok(WorkProbe::Unchanged);
        }
        return Ok(WorkProbe::Changed(State {
            bytes: buf[..first_len].to_vec(),
            mode: work_mode,
        }));
    }
    let mut hasher = Sha256::new();
    hasher.update(&buf[..first_len]);
    let mut total = first_len as u64;
    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .context("isolated file size overflow")?;
        if total > MAX_FILE_BYTES {
            bail!("isolated file exceeds 64 MiB: {}", work_path.display());
        }
        hasher.update(&buf[..n]);
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest == expected.digest {
        Ok(WorkProbe::Unchanged)
    } else {
        drop(reader);
        match state(work_path)? {
            Some(after) => Ok(WorkProbe::Changed(after)),
            None => Ok(WorkProbe::Deleted),
        }
    }
}

fn same_file_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.size() == b.size()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        a.creation_time() == b.creation_time()
            && a.last_write_time() == b.last_write_time()
            && a.file_size() == b.file_size()
            && a.file_attributes() == b.file_attributes()
    }
}

fn lock_readonly(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o444))?;
    }
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn work_files(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut paths = BTreeMap::new();
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
            let target = checked_target(root, &path)?;
            paths.insert(path, target);
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
    if metadata.len() > MAX_FILE_BYTES {
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
    #[test]
    fn inspect_only_loads_changed_base_files_and_detects_base_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("repo");
        fs::create_dir_all(parent.join("src")).unwrap();
        for index in 0..200 {
            let content = if index % 4 == 0 {
                "shared unmodified module contents\n".to_string()
            } else {
                format!("unmodified module {index:03}\n")
            };
            fs::write(parent.join(format!("src/mod_{index:03}.txt")), content).unwrap();
        }
        fs::write(parent.join("empty.txt"), b"").unwrap();
        fs::write(parent.join("big_unmodified.bin"), vec![b'Z'; 96 * 1024]).unwrap();
        fs::write(parent.join("modified.txt"), "original modified text").unwrap();
        fs::write(parent.join("same_size.txt"), "abcd1234").unwrap();
        fs::write(parent.join("deleted.txt"), "to be deleted").unwrap();

        let work = Workspace::create(&parent, dir.path(), "owner", &[]).unwrap();
        let base_dir = work.root.join("base");
        let pack_path = base_dir.join(PACK_FILE_NAME);
        assert!(pack_path.is_file());
        assert_eq!(fs::read_dir(&base_dir).unwrap().count(), 1);

        BASE_LOAD_COUNT.with(|count| count.set(0));
        let clean = work.inspect(1).unwrap();
        assert!(clean.contains("0 个文件"));
        assert_eq!(BASE_LOAD_COUNT.with(|count| count.get()), 0);

        fs::write(
            work.work().join("modified.txt"),
            "updated modified text with different length",
        )
        .unwrap();
        fs::write(work.work().join("same_size.txt"), "wxyz5678").unwrap();
        fs::remove_file(work.work().join("deleted.txt")).unwrap();
        fs::write(work.work().join("added.txt"), "brand new").unwrap();

        BASE_LOAD_COUNT.with(|count| count.set(0));
        let diff = work.inspect(2).unwrap();
        assert_eq!(BASE_LOAD_COUNT.with(|count| count.get()), 3);
        assert!(diff.contains("modified.txt"));
        assert!(diff.contains("same_size.txt"));
        assert!(diff.contains("deleted.txt"));
        assert!(diff.contains("added.txt"));
        assert!(!diff.contains("mod_000.txt"));
        assert!(!diff.contains("big_unmodified.bin"));

        let offset = work
            .manifest
            .lock()
            .unwrap()
            .files
            .get("modified.txt")
            .unwrap()
            .pack
            .unwrap()
            .offset as usize;
        #[cfg(windows)]
        clear_windows_readonly(&pack_path).unwrap();
        #[cfg(unix)]
        set_mode(&pack_path, 0o644).unwrap();
        let mut pack_bytes = fs::read(&pack_path).unwrap();
        pack_bytes[offset] ^= 0xff;
        fs::write(&pack_path, pack_bytes).unwrap();
        lock_readonly(&pack_path).unwrap();

        let err = work.inspect(3).unwrap_err();
        assert!(err.to_string().contains("snapshot base changed"));
        assert!(work.merge(3).is_err());

        #[cfg(windows)]
        clear_windows_readonly(&pack_path).unwrap();
        #[cfg(unix)]
        set_mode(&pack_path, 0o644).unwrap();
        fs::remove_file(&pack_path).unwrap();
        let err = work.inspect(3).unwrap_err();
        assert!(err.to_string().contains("snapshot base is missing"));

        work.discard().unwrap();
    }
    #[test]
    fn legacy_per_file_base_snapshot_loads_and_detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("repo");
        fs::create_dir(&parent).unwrap();
        let original = b"legacy original";
        fs::write(parent.join("old.txt"), original).unwrap();
        let canonical_parent = util::canonical(&parent).unwrap();
        let file_mode = mode(&fs::symlink_metadata(parent.join("old.txt")).unwrap());

        let id = util::id();
        let root = dir.path().join("delegates").join(&id);
        fs::create_dir_all(root.join("base")).unwrap();
        fs::create_dir_all(root.join("work")).unwrap();
        fs::write(root.join("base/old.txt"), original).unwrap();
        fs::write(root.join("work/old.txt"), b"legacy child").unwrap();
        let legacy_plan = json!({
            "owner": "owner",
            "parent": canonical_parent,
            "files": {
                "old.txt": {
                    "digest": util::hash(original),
                    "mode": file_mode,
                }
            },
            "reviewed": null,
            "merged": false,
        });
        fs::write(
            root.join("plan.json"),
            serde_json::to_vec(&legacy_plan).unwrap(),
        )
        .unwrap();

        let work = Workspace::load(dir.path(), "owner", &parent, &id)
            .unwrap()
            .unwrap();
        let diff = work.inspect(1).unwrap();
        assert!(diff.contains("legacy original"));
        assert!(diff.contains("legacy child"));

        fs::write(work.root.join("base/old.txt"), b"tampered").unwrap();
        let err = work.inspect(2).unwrap_err();
        assert!(err.to_string().contains("snapshot base changed"));

        work.discard().unwrap();
    }
}
