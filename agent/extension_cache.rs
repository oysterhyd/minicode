use crate::{configuration::Configuration, extensions::Catalog, util};
use anyhow::Result;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Metadata {
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    kind: u8,
    readonly: bool,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
    #[cfg(windows)]
    attributes: u32,
}
impl Metadata {
    fn of(meta: fs::Metadata) -> Self {
        Self {
            size: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            kind: if meta.is_file() {
                1
            } else if meta.is_dir() {
                2
            } else if meta.file_type().is_symlink() {
                3
            } else {
                4
            },
            readonly: meta.permissions().readonly(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
            },
            #[cfg(windows)]
            attributes: {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes()
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    link: Option<Metadata>,
    target: Option<Metadata>,
    unreadable: bool,
}
impl Stamp {
    fn read(path: &Path) -> Self {
        let read = |result: std::io::Result<fs::Metadata>| match result {
            Ok(meta) => (Some(Metadata::of(meta)), false),
            Err(error) => (None, error.kind() != std::io::ErrorKind::NotFound),
        };
        let (link, a) = read(fs::symlink_metadata(path));
        let (target, b) = read(fs::metadata(path));
        Self {
            link,
            target,
            unreadable: a || b,
        }
    }
}
type Dependencies = BTreeMap<PathBuf, Stamp>;
thread_local! { static TRACKING:RefCell<Option<Dependencies>>=const{RefCell::new(None)}; }
/// Record every file/directory whose state influenced discovery, including
/// missing candidates. Cache hits only stat these paths; they never rescan.
pub(crate) fn watch(path: &Path) {
    TRACKING.with(|tracking| {
        if let Some(deps) = tracking.borrow_mut().as_mut() {
            deps.entry(path.into()).or_insert_with(|| Stamp::read(path));
        }
    });
}
struct Tracking {
    previous: Option<Dependencies>,
    active: bool,
}
impl Tracking {
    fn begin() -> Self {
        Self {
            previous: TRACKING.with(|tracking| tracking.replace(Some(BTreeMap::new()))),
            active: true,
        }
    }
    fn finish(mut self) -> Dependencies {
        self.active = false;
        TRACKING
            .with(|tracking| tracking.replace(self.previous.take()))
            .unwrap()
    }
}
impl Drop for Tracking {
    fn drop(&mut self) {
        if self.active {
            TRACKING.with(|tracking| tracking.replace(self.previous.take()));
        }
    }
}
fn unchanged(deps: &Dependencies) -> bool {
    deps.iter()
        .all(|(path, stamp)| !stamp.unreadable && Stamp::read(path) == *stamp)
}
fn inherit(deps: &Dependencies) {
    TRACKING.with(|tracking| {
        if let Some(parent) = tracking.borrow_mut().as_mut() {
            for (path, stamp) in deps {
                parent.entry(path.clone()).or_insert_with(|| stamp.clone());
            }
        }
    });
}
struct Digest {
    value: String,
    dependencies: Dependencies,
}
type DigestSlot = Arc<Mutex<Option<Digest>>>;
type DigestCache = VecDeque<((PathBuf, bool), DigestSlot)>;
/// The plugin and imported-file caches use the same metadata signatures and
/// dependency propagation as Catalog discovery, including nested cache hits.
pub(crate) fn fingerprint(
    path: &Path,
    tree: bool,
    build: impl FnOnce() -> Result<String>,
) -> Result<String> {
    static CACHE: OnceLock<Mutex<DigestCache>> = OnceLock::new();
    let key = (util::canonical(path)?, tree);
    let slot = {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = cache.iter().position(|(k, _)| k == &key) {
            let entry = cache.remove(index).unwrap();
            let slot = entry.1.clone();
            cache.push_back(entry);
            slot
        } else {
            if cache.len() >= 128 {
                cache.pop_front();
            }
            let slot = Arc::new(Mutex::new(None));
            cache.push_back((key, slot.clone()));
            slot
        }
    };
    let mut entry = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = entry.as_ref()
        && unchanged(&entry.dependencies)
    {
        inherit(&entry.dependencies);
        return Ok(entry.value.clone());
    }
    *entry = None;
    let tracking = Tracking::begin();
    watch(path);
    let result = build();
    let dependencies = tracking.finish();
    inherit(&dependencies);
    let value = result?;
    if unchanged(&dependencies) {
        *entry = Some(Digest {
            value: value.clone(),
            dependencies,
        });
    }
    Ok(value)
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    workspace: PathBuf,
    configuration: PathBuf,
    check_lock: bool,
    environment: String,
}
struct Entry {
    catalog: Catalog,
    dependencies: Dependencies,
}
type Slot = Arc<Mutex<Option<Entry>>>;
pub(crate) fn catalog(
    workspace: &Path,
    configuration: Configuration,
    check_lock: bool,
    build: impl FnOnce() -> Result<Catalog>,
) -> Result<Catalog> {
    static CACHE: OnceLock<Mutex<VecDeque<(Key, Slot)>>> = OnceLock::new();
    // Expansion of environment variables and HOME-dependent discovery must
    // never reuse a snapshot from another environment. Only retain the hash.
    let mut environment = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect::<Vec<_>>();
    environment.sort();
    let key = Key {
        workspace: util::canonical(workspace)?,
        configuration: util::canonical(&configuration.path)?,
        check_lock,
        environment: util::hash(&serde_json::to_vec(&environment)?),
    };
    let slot = {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = cache.iter().position(|(k, _)| k == &key) {
            let entry = cache.remove(index).unwrap();
            let slot = entry.1.clone();
            cache.push_back(entry);
            slot
        } else {
            if cache.len() >= 32 {
                cache.pop_front();
            }
            let slot = Arc::new(Mutex::new(None));
            cache.push_back((key, slot.clone()));
            slot
        }
    };
    // A per-key lock coalesces simultaneous Runtime/child constructors without
    // serializing discovery for unrelated workspaces.
    let mut entry = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = entry.as_ref()
        && unchanged(&entry.dependencies)
    {
        return Ok(entry.catalog.clone());
    }
    *entry = None;
    let tracking = Tracking::begin();
    watch(&configuration.path);
    let result = build();
    let dependencies = tracking.finish();
    let catalog = result?;
    if unchanged(&dependencies) {
        *entry = Some(Entry {
            catalog: catalog.clone(),
            dependencies,
        });
    }
    Ok(catalog)
}
