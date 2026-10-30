//! Immutable raw Git objects and recorded roots outside the workspace's repositories.
//!
//! This store writes the Git loose-object format directly. It runs no Git program, invokes no
//! filters and borrows no project object: each root owns every blob and tree it needs. The
//! common manifest adds empty directories, original link text and executable bits. Every read
//! verifies both the object identifier and the manifest's BLAKE3 content identifier before
//! giving bytes to a caller.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use orch_link_contract::source::{FsRecordResult, RecordKind, RecordState};
use orch_types::{Hash, Id, Time};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::blobs::READ_CHUNK_BYTES;
use crate::error::{HostError, Result};
use crate::manifest::{self, EntryKind, ManifestEntry};
use crate::paths::{PathStyle, RelPath};
use crate::platform;

/// Recording limits last frozen by the trusted control plane for one directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordPolicy {
    /// The configured keeping period, with zero disabling new states.
    pub record_days: i64,
    /// The configured complete included-byte bound.
    pub max_bytes: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedReply {
    request_hash: Hash,
    reply: FsRecordResult,
}

/// A recorded root protected while one bounded operation reads or applies it.
#[derive(Debug)]
pub struct StatePin {
    path: PathBuf,
    live: Arc<LiveOperations>,
    key: (Id, Id),
}

impl Drop for StatePin {
    fn drop(&mut self) {
        let mut live = self.live.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = live.pins.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                live.pins.remove(&self.key);
                // Reacquiring this same operation cannot publish a replacement pin between the
                // count reaching zero and removal of its durable file.
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

#[derive(Debug, Default)]
struct Operations {
    pins: BTreeMap<(Id, Id), usize>,
    destructive: bool,
}

#[derive(Debug, Default)]
struct LiveOperations(Mutex<Operations>);

fn store_busy() -> HostError {
    HostError::Busy("This environment is publishing, reading or replacing recorded files. Retry after the operation finishes.".to_owned())
}

/// Exclusive destructive admission, sharing the exact gate used to acquire a recorded-state pin.
/// It owns no mutex while asynchronous teardown or import is in progress.
#[derive(Debug)]
pub struct DestructiveLease {
    live: Arc<LiveOperations>,
    retired: Arc<AtomicBool>,
}

impl DestructiveLease {
    /// Older environment/store clones cannot admit a reader after this attachment is removed.
    pub(crate) fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }
}

impl Drop for DestructiveLease {
    fn drop(&mut self) {
        self.live
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .destructive = false;
    }
}

fn live_operations(root: &Path) -> Arc<LiveOperations> {
    static STORES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<LiveOperations>>>> = OnceLock::new();
    let mut stores = STORES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    stores.retain(|_, live| live.strong_count() > 0);
    if let Some(live) = stores.get(root).and_then(Weak::upgrade) {
        return live;
    }
    let live = Arc::new(LiveOperations::default());
    stores.insert(root.to_owned(), Arc::downgrade(&live));
    live
}

/// A bounded, open immutable manifest; a waiting publication allocates no manifest-sized buffer.
#[derive(Debug)]
pub struct ManifestInput {
    /// The hash verified by the receiving content store.
    pub hash: Hash,
    /// The complete compressed file length.
    pub size: i64,
    file: Arc<File>,
}

impl ManifestInput {
    /// Streams the pinned file through the existing hash-and-size-verifying content transport.
    pub async fn publish(self, sink: &dyn crate::blobs::BlobSink) -> Result<()> {
        let mut reader = crate::blobs::FileReader::new(self.file);
        sink.put(self.hash, self.size, &mut reader)
            .await
            .map_err(|error| HostError::Content(error.to_string()))
    }
}

/// A Git object identifier is twenty bytes, written as forty lowercase hexadecimal digits.
const OBJECT_ID_BYTES: usize = 20;
/// An object header has a type, a decimal 64-bit size and a terminator; this bound allows ample
/// room for those fields while refusing a corrupt file without reading it indefinitely.
const OBJECT_HEADER_MAX_BYTES: usize = 128;
/// Only the executable permission bits travel through a file-state restore.
pub const EXECUTABLE_BITS: u32 = 0o111;
/// Git's mode for an ordinary captured file.
const GIT_FILE_MODE: &str = "100644";
/// Git's mode for a captured executable file.
const GIT_EXECUTABLE_MODE: &str = "100755";
/// Git's mode for link text stored as a blob.
const GIT_LINK_MODE: &str = "120000";
/// Git's mode for a subtree, including an empty directory.
const GIT_TREE_MODE: &str = "40000";

/// The immutable metadata that roots one recorded state and its owned objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateRoot {
    /// The exact retry-stable snapshot identity.
    pub snapshot_id: Id,
    /// The frozen request identity, for recovering a published root after its reply was lost.
    #[serde(default)]
    pub request_hash: Option<Hash>,
    /// The workspace-relative directory this state covers.
    pub directory: String,
    /// The environment generation in which it was recorded.
    pub generation: i64,
    /// Turn recording or restore undo.
    pub kind: RecordKind,
    /// The Git tree identifier in this store alone.
    pub tree_ref: String,
    /// The BLAKE3 identifier of the common compressed manifest.
    pub manifest_blob: Hash,
    /// Git blob identifiers for files and links, keyed by directory-relative path.
    pub objects: BTreeMap<String, String>,
    /// How many regular files it holds.
    pub files: i64,
    /// Included file bytes and link-text bytes.
    pub bytes: i64,
    /// When it was recorded, from the injected clock.
    pub created_at: Time,
    /// The frozen keeping deadline.
    pub expires_at: Time,
    /// The project HEAD locally known at capture, if any.
    pub repository_head: Option<String>,
    /// The ignore-coverage fingerprint frozen at capture.
    pub ignore_fingerprint: Hash,
}

impl StateRoot {
    /// Safe command metadata for a verified complete state.
    #[must_use]
    pub fn result(&self) -> FsRecordResult {
        FsRecordResult {
            state: RecordState::Complete,
            tree_ref: Some(self.tree_ref.clone()),
            repository_head: self.repository_head.clone(),
            manifest_blob: Some(self.manifest_blob),
            files: self.files,
            bytes: self.bytes,
            expires_at: Some(self.expires_at),
            reason: None,
        }
    }
}

/// One environment's private loose-object repository, always outside its workspace.
#[derive(Debug, Clone)]
pub struct ObjectStore {
    root: PathBuf,
    live: Arc<LiveOperations>,
    // Clones belong to this attachment; an explicit open starts another handle, still sharing
    // the physical store's pin/exclusive gate. Retiring one attachment never revives its clones.
    retired: Arc<AtomicBool>,
    durability: DirectoryDurability,
}

#[derive(Debug, Clone, Default)]
struct DirectoryDurability {
    #[cfg(test)]
    control: Option<Arc<DirectoryControl>>,
}

fn directory_parent(path: &Path) -> Option<&Path> {
    path.parent()
        .map(|parent| {
            if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            }
        })
        .filter(|parent| *parent != path)
}

impl DirectoryDurability {
    fn sync(&self, path: &Path) -> Result<()> {
        #[cfg(test)]
        if let Some(control) = &self.control {
            control.before_sync(path)?;
        }
        platform::Dir::open_path(path)
            .and_then(|dir| dir.sync())
            .map_err(|error| HostError::io("syncing a file-state directory", error))?;
        #[cfg(test)]
        if let Some(control) = &self.control {
            control.record(DirectoryEvent::Synced(path.to_owned()));
        }
        Ok(())
    }

    fn ensure(&self, path: &Path) -> Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = directory_parent(path) {
                    self.ensure(parent)?;
                }
                match std::fs::create_dir(path) {
                    Ok(()) =>
                    {
                        #[cfg(test)]
                        if let Some(control) = &self.control {
                            control.record(DirectoryEvent::Created(path.to_owned()));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(HostError::io("creating the file-state store", error));
                    }
                }
            }
            Err(error) => return Err(HostError::io("checking the file-state store", error)),
        }
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| HostError::io("checking the file-state store", error))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(HostError::Unsupported(
                "the file-state store contains a linked directory".to_owned(),
            ));
        }
        // A failed parent barrier may leave the directory visible. Retrying an existing name
        // must flush that edge too, before creating a child or publishing a file inside it.
        if let Some(parent) = directory_parent(path) {
            self.sync(parent)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum DirectoryEvent {
    Created(PathBuf),
    Sync(PathBuf),
    Synced(PathBuf),
    Published(PathBuf),
}

#[cfg(test)]
#[derive(Debug, Default)]
struct DirectoryControl {
    events: Mutex<Vec<DirectoryEvent>>,
    refuse: Mutex<Option<PathBuf>>,
}

#[cfg(test)]
impl DirectoryControl {
    fn record(&self, event: DirectoryEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
    fn before_sync(&self, path: &Path) -> Result<()> {
        self.record(DirectoryEvent::Sync(path.to_owned()));
        let mut refuse = self.refuse.lock().unwrap_or_else(PoisonError::into_inner);
        if refuse.as_deref() == Some(path) {
            *refuse = None;
            return Err(HostError::io(
                "syncing a file-state directory",
                std::io::Error::other("injected directory sync refusal"),
            ));
        }
        Ok(())
    }
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| HostError::io("opening recorded metadata", error))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| HostError::io("reading recorded metadata", error))?;
    if bytes.len() as u64 > limit {
        return Err(HostError::Unsupported(
            "recorded metadata is too large".to_owned(),
        ));
    }
    Ok(bytes)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with(path, bytes, &DirectoryDurability::default())
}

fn atomic_write_with(path: &Path, bytes: &[u8], durability: &DirectoryDurability) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| HostError::Internal("recorded metadata has no directory".to_owned()))?;
    durability.ensure(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| HostError::io("staging recorded metadata", error))?;
    staged
        .write_all(bytes)
        .map_err(|error| HostError::io("writing recorded metadata", error))?;
    staged
        .as_file()
        .sync_all()
        .map_err(|error| HostError::io("syncing recorded metadata", error))?;
    staged
        .persist(path)
        .map_err(|error| HostError::io("publishing recorded metadata", error.error))?;
    #[cfg(test)]
    if let Some(control) = &durability.control {
        control.record(DirectoryEvent::Published(path.to_owned()));
    }
    durability.sync(parent)
}

impl ObjectStore {
    /// Opens a separate store, refusing a path that lies inside the project workspace.
    pub fn open(root: &Path, workspace: &Path) -> Result<Self> {
        Self::open_with(root, workspace, DirectoryDurability::default())
    }

    fn open_with(root: &Path, workspace: &Path, durability: DirectoryDurability) -> Result<Self> {
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|error| HostError::io("locating the workspace", error))?;
        if root.starts_with(&workspace) {
            return Err(HostError::Unsupported(
                "the file-state store must be outside the workspace".to_owned(),
            ));
        }
        durability.ensure(root)?;
        let root = std::fs::canonicalize(root)
            .map_err(|error| HostError::io("locating the file-state store", error))?;
        if root.starts_with(&workspace) {
            return Err(HostError::Unsupported(
                "the file-state store must be outside the workspace".to_owned(),
            ));
        }
        for relative in [
            "git",
            "git/objects",
            "git/refs",
            "git/refs/snapshots",
            "roots",
            "manifests",
            "journals",
            "results",
            "policies",
            "pins",
        ] {
            durability.ensure(&root.join(relative))?;
        }
        let live = live_operations(&root);
        let store = Self {
            root,
            live,
            retired: Arc::new(AtomicBool::new(false)),
            durability,
        };
        if !store.root.join("git/HEAD").exists() {
            store.write_metadata(
                &store.root.join("git/HEAD"),
                b"ref: refs/heads/file-states\n",
            )?;
        }
        if !store.root.join("git/config").exists() {
            store.write_metadata(
                &store.root.join("git/config"),
                b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n",
            )?;
        }
        Ok(store)
    }

    fn write_metadata(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        atomic_write_with(path, bytes, &self.durability)
    }

    /// The owned store's location, for move planning and journal storage.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Verifies every still-retained root and every journal dependency without changing a project.
    pub fn verify_kept(
        &self,
        now: Time,
    ) -> Result<orch_link_contract::source::FsRecordVerifyResult> {
        let mut required = BTreeSet::new();
        let mut journals = 0i64;
        for part in [
            "journals",
            "source-jobs",
            "git-jobs",
            "worktree-jobs",
            "materialize-jobs",
            "plans",
        ] {
            let path = self.root.join(part);
            if !path.exists() {
                continue;
            }
            for entry in std::fs::read_dir(path)
                .map_err(|error| HostError::io("reading carried recovery metadata", error))?
            {
                let entry = entry
                    .map_err(|error| HostError::io("reading carried recovery metadata", error))?;
                if entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "json")
                {
                    continue;
                }
                if !entry
                    .file_type()
                    .map_err(|error| HostError::io("checking carried recovery metadata", error))?
                    .is_file()
                {
                    return Err(HostError::Unsupported(
                        "carried recovery metadata has a link or non-file entry".into(),
                    ));
                }
                let value: serde_json::Value = serde_json::from_slice(&read_bounded(
                    &entry.path(),
                    manifest::MANIFEST_MAX_EXPANDED_BYTES,
                )?)
                .map_err(|_| {
                    HostError::Unsupported(
                        "carried recovery metadata is not a complete JSON value".into(),
                    )
                })?;
                if !value.is_object() {
                    return Err(HostError::Unsupported(
                        "carried recovery metadata is not an object".into(),
                    ));
                }
                if part != "plans" {
                    journals += 1;
                }
                let finished = matches!(
                    value.get("phase").and_then(serde_json::Value::as_str),
                    Some("complete" | "rolled_back")
                ) || value.get("result").is_some_and(|value| !value.is_null())
                    || value.get("removed").and_then(serde_json::Value::as_bool) == Some(true);
                if !finished && part != "plans" {
                    for field in ["snapshot_id", "undo_snapshot_id"] {
                        if let Some(id) = value.get(field).and_then(serde_json::Value::as_str) {
                            required.insert(id.parse::<Id>().map_err(|_| {
                                HostError::Unsupported(
                                    "a carried journal names an invalid root".into(),
                                )
                            })?);
                        }
                    }
                }
            }
        }
        let mut roots = BTreeMap::new();
        for entry in std::fs::read_dir(self.root.join("roots"))
            .map_err(|error| HostError::io("reading carried roots", error))?
        {
            let entry = entry.map_err(|error| HostError::io("reading carried roots", error))?;
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|name| name.parse::<Id>().ok())
            else {
                continue;
            };
            if !entry
                .file_type()
                .map_err(|error| HostError::io("checking a carried root", error))?
                .is_file()
            {
                return Err(HostError::Unsupported(
                    "a carried root is a link or non-file".into(),
                ));
            }
            let root: StateRoot = serde_json::from_slice(&read_bounded(
                &entry.path(),
                manifest::MANIFEST_MAX_EXPANDED_BYTES,
            )?)
            .map_err(|_| HostError::Unsupported("carried root metadata is invalid".into()))?;
            if root.snapshot_id != id {
                return Err(HostError::Unsupported(
                    "a carried root changed its immutable identity".into(),
                ));
            }
            if root.expires_at > now || self.pinned(id, now)? {
                required.insert(id);
            }
            roots.insert(id, root);
        }
        // References also retain a root whose keeping deadline passed before collection ran.
        for entry in std::fs::read_dir(self.root.join("git/refs/snapshots"))
            .map_err(|error| HostError::io("reading carried references", error))?
        {
            let entry =
                entry.map_err(|error| HostError::io("reading a carried reference", error))?;
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<Id>().ok())
            else {
                continue;
            };
            required.insert(id);
            let root = roots
                .get(&id)
                .ok_or_else(|| HostError::Unsupported("a carried reference has no root".into()))?;
            if read_bounded(&entry.path(), OBJECT_HEADER_MAX_BYTES as u64)?.as_slice()
                != format!("{}\n", root.tree_ref).as_bytes()
            {
                return Err(HostError::Unsupported(
                    "a carried reference changed its recorded tree".into(),
                ));
            }
        }
        let mut verified = BTreeSet::new();
        for id in required {
            if !roots.contains_key(&id) {
                return Err(HostError::Unsupported(
                    "a carried recovery journal lost its root".into(),
                ));
            }
            let (root, parsed) = self.load_for_recovery(id)?;
            for entry in parsed
                .entries
                .iter()
                .filter(|entry| entry.t != EntryKind::Dir)
            {
                let oid = root.objects.get(&entry.p).ok_or_else(|| {
                    HostError::Unsupported("a carried entry has no owned object".into())
                })?;
                let identity = (oid.clone(), entry.h, entry.s, entry.l.clone());
                if verified.insert(identity) {
                    self.read_entry(&root, entry)?;
                }
            }
        }
        Ok(orch_link_contract::source::FsRecordVerifyResult {
            roots: i64::try_from(roots.len())
                .map_err(|_| HostError::Unsupported("too many carried roots".into()))?,
            objects: i64::try_from(verified.len())
                .map_err(|_| HostError::Unsupported("too many carried objects".into()))?,
            journals,
        })
    }

    /// Whether this identity has already published root metadata, even if its reply was lost.
    #[must_use]
    pub fn has_root(&self, snapshot: Id) -> bool {
        self.root
            .join("roots")
            .join(format!("{snapshot}.json"))
            .exists()
    }

    /// The first durable recording reply for an identity, including a reasoned fallback.
    pub fn recorded_reply(
        &self,
        request: &orch_link_contract::source::FsRecord,
    ) -> Result<Option<FsRecordResult>> {
        let snapshot = request.snapshot_id;
        let path = self.root.join("results").join(format!("{snapshot}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let bytes = read_bounded(&path, manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
        let saved: RecordedReply = serde_json::from_slice(&bytes).map_err(|error| {
            HostError::Unsupported(format!("recorded reply is invalid: {error}"))
        })?;
        let asked =
            serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?;
        if saved.request_hash != Hash::of(&asked) {
            return Err(HostError::Changed(
                "the recording identity was used with another frozen choice".to_owned(),
            ));
        }
        // The name may be visible after a previous rename whose directory flush failed.
        // File data was synced before that rename; commit its directory before replaying it.
        self.durability.sync(&self.root.join("results"))?;
        Ok(Some(saved.reply))
    }

    /// Keeps the first reply so a retry cannot record a newer version under the same identity.
    pub fn keep_reply(
        &self,
        request: &orch_link_contract::source::FsRecord,
        reply: &FsRecordResult,
    ) -> Result<()> {
        let snapshot = request.snapshot_id;
        let asked =
            serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?;
        let bytes = serde_json::to_vec(&RecordedReply {
            request_hash: Hash::of(&asked),
            reply: reply.clone(),
        })
        .map_err(|error| HostError::Internal(error.to_string()))?;
        self.write_metadata(
            &self.root.join("results").join(format!("{snapshot}.json")),
            &bytes,
        )
    }

    /// Claim a first capture durably. An interrupted identity without a complete root is never recaptured.
    pub(crate) fn begin_record(
        &self,
        request: &orch_link_contract::source::FsRecord,
    ) -> Result<()> {
        let marker = self
            .root
            .join("results")
            .join(format!("{}.started", request.snapshot_id));
        if marker.exists() {
            return Err(HostError::Changed("the interrupted recording has no complete captured state; use a new state identity".to_owned()));
        }
        let bytes =
            serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?;
        self.write_metadata(&marker, Hash::of(&bytes).to_string().as_bytes())
    }

    /// Acquire both crash protection and an immutable file before allowing the environment key to go.
    pub(crate) fn publication(
        &self,
        request: &orch_link_contract::source::FsRecord,
        reply: &FsRecordResult,
        generation: i64,
        now: Time,
        operation: Id,
        until: Time,
    ) -> Result<(ManifestInput, StatePin)> {
        let pin = self.pin(request.snapshot_id, operation, until)?;
        // Preserve the existing root/manifest/tree verification. Its bounded parse is local to
        // capture and dropped here; waiting publications retain only the open file below.
        let (root, _) = self.load(request.snapshot_id, now)?;
        let asked =
            serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?;
        if root.request_hash != Some(Hash::of(&asked))
            || root.generation != generation
            || root.result() != *reply
        {
            return Err(HostError::Changed(
                "the captured state does not match this recording's frozen identity".to_owned(),
            ));
        }
        let directory = platform::Dir::open_path(&self.root)
            .and_then(|directory| directory.open_child(std::ffi::OsStr::new("manifests")))
            .map_err(|error| HostError::io("opening recorded manifests", error))?;
        let file = directory
            .open_read(std::ffi::OsStr::new(&root.manifest_blob.to_string()))
            .map_err(|error| HostError::io("opening the captured manifest", error))?;
        let size = file
            .metadata()
            .map_err(|error| HostError::io("reading captured manifest size", error))?
            .len();
        if size > manifest::MANIFEST_MAX_EXPANDED_BYTES {
            return Err(HostError::Unsupported(
                "the captured manifest exceeds the recording limit".to_owned(),
            ));
        }
        let size = i64::try_from(size)
            .map_err(|_| HostError::Unsupported("the captured manifest is too large".to_owned()))?;
        Ok((
            ManifestInput {
                hash: root.manifest_blob,
                size,
                file: Arc::new(file),
            },
            pin,
        ))
    }

    /// Saves only the nonsecret recording settings, for a subsequent explicit restore plan.
    pub fn keep_policy(&self, directory: &RelPath, policy: RecordPolicy) -> Result<()> {
        let key = Hash::of(directory.as_str().as_bytes()).to_string();
        let bytes =
            serde_json::to_vec(&policy).map_err(|error| HostError::Internal(error.to_string()))?;
        self.write_metadata(&self.root.join("policies").join(key), &bytes)
    }

    /// Reads the directory's last trusted recording settings, without guessing an enabled policy.
    pub fn policy(&self, directory: &RelPath) -> Result<Option<RecordPolicy>> {
        let path = self
            .root
            .join("policies")
            .join(Hash::of(directory.as_str().as_bytes()).to_string());
        if !path.exists() {
            return Ok(None);
        }
        let bytes = read_bounded(&path, manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            HostError::Unsupported(format!("recording policy is invalid: {error}"))
        })
    }

    /// Protects a root until this command's existing operation budget, with a durable operation id.
    pub fn pin(&self, snapshot: Id, operation: Id, until: Time) -> Result<StatePin> {
        let path = self
            .root
            .join("pins")
            .join(snapshot.to_string())
            .join(operation.to_string());
        let bytes =
            serde_json::to_vec(&until).map_err(|error| HostError::Internal(error.to_string()))?;
        let mut live = self.live.0.lock().unwrap_or_else(PoisonError::into_inner);
        if self.retired.load(Ordering::SeqCst) {
            return Err(HostError::NotFound(
                "this environment attachment has ended".to_owned(),
            ));
        }
        if live.destructive {
            return Err(store_busy());
        }
        self.write_metadata(&path, &bytes)?;
        let key = (snapshot, operation);
        *live.pins.entry(key).or_default() += 1;
        Ok(StatePin {
            path,
            live: Arc::clone(&self.live),
            key,
        })
    }

    /// Reserves destructive work against both existing pins and all subsequent pin admission.
    pub fn reserve_destructive(&self) -> Result<DestructiveLease> {
        let mut live = self.live.0.lock().unwrap_or_else(PoisonError::into_inner);
        if self.retired.load(Ordering::SeqCst) {
            return Err(HostError::NotFound(
                "this environment attachment has ended".to_owned(),
            ));
        }
        if live.destructive || !live.pins.is_empty() {
            return Err(store_busy());
        }
        live.destructive = true;
        Ok(DestructiveLease {
            live: Arc::clone(&self.live),
            retired: Arc::clone(&self.retired),
        })
    }

    /// Replacing an attachment fences its old handles without invalidating pins already admitted.
    pub(crate) fn retire_attachment(&self) {
        let _admission = self.live.0.lock().unwrap_or_else(PoisonError::into_inner);
        self.retired.store(true, Ordering::SeqCst);
    }

    /// Observes current availability; destructive callers must retain `reserve_destructive` instead.
    pub fn require_idle(&self) -> Result<()> {
        let live = self.live.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !live.destructive && live.pins.is_empty() {
            return Ok(());
        }
        Err(store_busy())
    }

    /// Whether a bounded reading or applying operation still protects this root after a restart.
    pub fn pinned(&self, snapshot: Id, now: Time) -> Result<bool> {
        if self
            .live
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pins
            .keys()
            .any(|(id, _)| *id == snapshot)
        {
            return Ok(true);
        }
        let directory = self.root.join("pins").join(snapshot.to_string());
        if !directory.exists() {
            return Ok(false);
        }
        for entry in std::fs::read_dir(directory)
            .map_err(|error| HostError::io("reading file-state pins", error))?
        {
            let entry = entry.map_err(|error| HostError::io("reading a file-state pin", error))?;
            let bytes = read_bounded(&entry.path(), manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
            let until: Time = serde_json::from_slice(&bytes)
                .map_err(|_| HostError::Unsupported("a file-state pin is invalid".to_owned()))?;
            if until > now {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn protect_tree(&self, root: &str, protected: &mut BTreeSet<String>) -> Result<()> {
        let mut pending = vec![root.to_owned()];
        while let Some(oid) = pending.pop() {
            if !protected.insert(oid.clone()) {
                continue;
            }
            let verified = self.verify_object(&oid, "tree", None, None)?;
            let bytes = read_bounded(verified.path(), manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
            let mut position = 0usize;
            while position < bytes.len() {
                let end = bytes[position..]
                    .iter()
                    .position(|byte| *byte == 0)
                    .map(|end| position + end)
                    .ok_or_else(|| {
                        HostError::Unsupported("a kept tree entry is invalid".to_owned())
                    })?;
                let entry = std::str::from_utf8(&bytes[position..end]).map_err(|_| {
                    HostError::Unsupported("a kept tree entry is invalid".to_owned())
                })?;
                let (mode, _) = entry.split_once(' ').ok_or_else(|| {
                    HostError::Unsupported("a kept tree entry is invalid".to_owned())
                })?;
                let start = end + 1;
                let end = start
                    .checked_add(OBJECT_ID_BYTES)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| {
                        HostError::Unsupported(
                            "a kept tree object identifier is incomplete".to_owned(),
                        )
                    })?;
                let child = hex::encode(&bytes[start..end]);
                if mode == GIT_TREE_MODE {
                    pending.push(child);
                } else {
                    protected.insert(child);
                }
                position = end;
            }
        }
        Ok(())
    }

    /// Removes only expired unpinned roots, then objects unreachable from roots, journals or cache.
    /// Root metadata remains as an explanation of expiry; collection never touches a project Git.
    pub fn prune(
        &self,
        _expired: &[Id],
        now: Time,
        mut protected: BTreeSet<String>,
    ) -> Result<i64> {
        let mut journal_roots = BTreeSet::new();
        for entry in std::fs::read_dir(self.root.join("journals"))
            .map_err(|error| HostError::io("reading restore journals", error))?
        {
            let entry = entry.map_err(|error| HostError::io("reading a restore journal", error))?;
            if !entry
                .file_type()
                .map_err(|error| HostError::io("checking a restore journal", error))?
                .is_file()
            {
                continue;
            }
            let bytes = read_bounded(&entry.path(), manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
            let journal: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
                HostError::Unsupported(
                    "a restore journal is invalid; collection waits for recovery".to_owned(),
                )
            })?;
            if matches!(
                journal.get("phase").and_then(serde_json::Value::as_str),
                Some("complete" | "rolled_back")
            ) {
                continue;
            }
            for key in ["snapshot_id", "undo_snapshot_id"] {
                if let Some(id) = journal.get(key).and_then(serde_json::Value::as_str) {
                    journal_roots.insert(id.parse::<Id>().map_err(|_| {
                        HostError::Unsupported("a journal root is invalid".to_owned())
                    })?);
                }
            }
        }
        let worktrees = self.root.join("worktree-jobs");
        if worktrees.exists() {
            for entry in std::fs::read_dir(&worktrees)
                .map_err(|error| HostError::io("reading worktree recovery journals", error))?
            {
                let entry = entry
                    .map_err(|error| HostError::io("reading a worktree recovery journal", error))?;
                if !entry
                    .file_type()
                    .map_err(|error| HostError::io("checking a worktree journal", error))?
                    .is_file()
                {
                    continue;
                }
                let bytes = read_bounded(&entry.path(), manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
                let journal: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
                    HostError::Unsupported(
                        "an incomplete worktree journal is invalid; collection waits for recovery"
                            .into(),
                    )
                })?;
                if journal
                    .get("result")
                    .is_some_and(|result| !result.is_null())
                    || journal.get("removed").and_then(serde_json::Value::as_bool) == Some(true)
                {
                    continue;
                }
                if let Some(snapshot) = journal
                    .get("snapshot_id")
                    .and_then(serde_json::Value::as_str)
                {
                    journal_roots.insert(snapshot.parse::<Id>().map_err(|_| {
                        HostError::Unsupported("a worktree recovery root is invalid".into())
                    })?);
                }
            }
        }
        let mut releasing = Vec::new();
        for entry in std::fs::read_dir(self.root.join("roots"))
            .map_err(|error| HostError::io("reading recorded roots", error))?
        {
            let entry = entry.map_err(|error| HostError::io("reading a recorded root", error))?;
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|name| name.parse::<Id>().ok())
            else {
                continue;
            };
            let bytes = read_bounded(&entry.path(), manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
            let root: StateRoot = serde_json::from_slice(&bytes).map_err(|_| {
                HostError::Unsupported(
                    "a recorded root is invalid; collection waits for recovery".to_owned(),
                )
            })?;
            if root.snapshot_id != id {
                return Err(HostError::Unsupported(
                    "a recorded root identity is invalid".to_owned(),
                ));
            }
            let keep =
                root.expires_at > now || self.pinned(id, now)? || journal_roots.contains(&id);
            if keep {
                self.protect_tree(&root.tree_ref, &mut protected)?;
            } else {
                releasing.push(id);
            }
        }
        // All root and journal checks precede deletion. A malformed live root cannot cause its
        // objects to be mistaken for garbage.
        let mut removed = 0i64;
        for id in releasing {
            let reference = self.root.join("git/refs/snapshots").join(id.to_string());
            match std::fs::remove_file(&reference) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(HostError::io("releasing an expired recorded root", error));
                }
            }
            let reply_path = self.root.join("results").join(format!("{id}.json"));
            if reply_path.exists() {
                let bytes = read_bounded(&reply_path, manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
                let mut saved: RecordedReply = serde_json::from_slice(&bytes).map_err(|_| {
                    HostError::Unsupported("an expired recording reply is invalid".to_owned())
                })?;
                saved.reply.state = RecordState::Unavailable;
                saved.reply.tree_ref = None;
                saved.reply.manifest_blob = None;
                saved.reply.reason = Some("expired".to_owned());
                let bytes = serde_json::to_vec(&saved)
                    .map_err(|error| HostError::Internal(error.to_string()))?;
                self.write_metadata(&reply_path, &bytes)?;
            }
        }
        for directory in std::fs::read_dir(self.root.join("git/objects"))
            .map_err(|error| HostError::io("reading owned object directories", error))?
        {
            let directory = directory
                .map_err(|error| HostError::io("reading an owned object directory", error))?;
            let name = directory.file_name();
            let Some(prefix) = name.to_str().filter(|name| {
                name.len() == 2
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            }) else {
                continue;
            };
            if !directory
                .file_type()
                .map_err(|error| HostError::io("checking an object directory", error))?
                .is_dir()
            {
                continue;
            }
            for entry in std::fs::read_dir(directory.path())
                .map_err(|error| HostError::io("reading owned objects", error))?
            {
                let entry =
                    entry.map_err(|error| HostError::io("reading an owned object", error))?;
                let oid = format!("{prefix}{}", entry.file_name().to_string_lossy());
                if self.object_path(&oid).is_ok() && !protected.contains(&oid) {
                    std::fs::remove_file(entry.path()).map_err(|error| {
                        HostError::io("collecting an unreferenced owned object", error)
                    })?;
                }
            }
        }
        Ok(removed)
    }

    fn object_path(&self, oid: &str) -> Result<PathBuf> {
        if oid.len() != OBJECT_ID_BYTES * 2
            || !oid
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(HostError::Unsupported(
                "a recorded Git object identifier is invalid".to_owned(),
            ));
        }
        Ok(self
            .root
            .join("git/objects")
            .join(&oid[..2])
            .join(&oid[2..]))
    }

    fn publish_object(&self, staged: tempfile::NamedTempFile, oid: &str) -> Result<()> {
        let path = self.object_path(oid)?;
        let parent = path
            .parent()
            .ok_or_else(|| HostError::Internal("a Git object has no directory".to_owned()))?;
        self.durability.ensure(parent)?;
        staged
            .as_file()
            .sync_all()
            .map_err(|error| HostError::io("syncing a recorded object", error))?;
        // Replacing this name publishes the same verified object content, and also repairs an
        // existing corrupt copy instead of declaring a newly recorded blob complete over it.
        staged
            .persist(&path)
            .map_err(|error| HostError::io("publishing a recorded object", error.error))?;
        #[cfg(test)]
        if let Some(control) = &self.durability.control {
            control.record(DirectoryEvent::Published(path.clone()));
        }
        self.durability.sync(parent)
    }

    /// Stores raw file bytes as an owned Git blob and returns its Git and BLAKE3 identifiers.
    pub fn write_blob(&self, file: &mut File, size: u64) -> Result<(String, Hash)> {
        let before = platform::file_meta(file)
            .map_err(|error| HostError::io("checking a recorded file", error))?;
        if !before.is_file() || before.size != size {
            return Err(HostError::Changed(
                "a recorded file changed before it was read".to_owned(),
            ));
        }
        let staged = tempfile::NamedTempFile::new_in(self.root.join("git/objects"))
            .map_err(|error| HostError::io("staging a recorded object", error))?;
        let mut encoder = ZlibEncoder::new(staged, Compression::default());
        let header = format!("blob {size}\0");
        let mut oid = Sha1::new();
        oid.update(header.as_bytes());
        encoder
            .write_all(header.as_bytes())
            .map_err(|error| HostError::io("writing a blob header", error))?;
        let mut hash = Hash::hasher();
        let mut buffer = vec![0u8; READ_CHUNK_BYTES];
        let mut read = 0u64;
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|error| HostError::io("reading recorded file bytes", error))?;
            if count == 0 {
                break;
            }
            read = read.checked_add(count as u64).ok_or_else(|| {
                HostError::Unsupported("a recorded file size overflowed".to_owned())
            })?;
            if read > size {
                return Err(HostError::Changed(
                    "a recorded file grew while it was read".to_owned(),
                ));
            }
            oid.update(&buffer[..count]);
            hash.update(&buffer[..count]);
            encoder
                .write_all(&buffer[..count])
                .map_err(|error| HostError::io("storing recorded file bytes", error))?;
        }
        let after = platform::file_meta(file)
            .map_err(|error| HostError::io("rechecking a recorded file", error))?;
        if read != size || after.identity() != before.identity() || after.mode != before.mode {
            return Err(HostError::Changed(
                "a recorded file changed while it was read".to_owned(),
            ));
        }
        let oid = hex::encode(oid.finalize());
        let staged = encoder
            .finish()
            .map_err(|error| HostError::io("finishing a recorded blob", error))?;
        self.publish_object(staged, &oid)?;
        Ok((oid, Hash::finish(&hash)))
    }

    /// Stores an owned Git blob or tree without any project attribute or filter processing.
    pub fn write_object(&self, kind: &str, bytes: &[u8]) -> Result<String> {
        if !matches!(kind, "blob" | "tree") {
            return Err(HostError::Unsupported(
                "only recorded blobs and trees are stored".to_owned(),
            ));
        }
        let staged = tempfile::NamedTempFile::new_in(self.root.join("git/objects"))
            .map_err(|error| HostError::io("staging a recorded object", error))?;
        let mut encoder = ZlibEncoder::new(staged, Compression::default());
        let header = format!("{kind} {}\0", bytes.len());
        let mut hash = Sha1::new();
        hash.update(header.as_bytes());
        hash.update(bytes);
        encoder
            .write_all(header.as_bytes())
            .and_then(|()| encoder.write_all(bytes))
            .map_err(|error| HostError::io("writing a recorded object", error))?;
        let oid = hex::encode(hash.finalize());
        self.publish_object(
            encoder
                .finish()
                .map_err(|error| HostError::io("finishing a recorded object", error))?,
            &oid,
        )?;
        Ok(oid)
    }

    /// Builds and owns every subtree, retaining empty directories and link text as Git entries.
    pub fn write_tree(
        &self,
        entries: &[ManifestEntry],
        objects: &BTreeMap<String, String>,
    ) -> Result<String> {
        self.tree(entries, objects, false)
    }

    fn tree(
        &self,
        entries: &[ManifestEntry],
        objects: &BTreeMap<String, String>,
        verify: bool,
    ) -> Result<String> {
        let mut directories: BTreeSet<String> = BTreeSet::from([String::new()]);
        let mut children: BTreeMap<String, Vec<(String, &str, String)>> = BTreeMap::new();
        for entry in entries {
            let path = RelPath::parse(&entry.p, PathStyle::Posix)?;
            if path.is_root() || path.has_component(".git") {
                return Err(HostError::Unsupported(
                    "recorded trees contain no project metadata or root entry".to_owned(),
                ));
            }
            if entry.t == EntryKind::Dir {
                directories.insert(entry.p.clone());
            }
            let mut parent = path.parent();
            while let Some(path) = parent {
                directories.insert(path.to_string());
                parent = path.parent();
            }
            if entry.t != EntryKind::Dir {
                let mode = match entry.t {
                    EntryKind::File if entry.x & EXECUTABLE_BITS != 0 => GIT_EXECUTABLE_MODE,
                    EntryKind::File => GIT_FILE_MODE,
                    EntryKind::Link => GIT_LINK_MODE,
                    _ => {
                        return Err(HostError::Unsupported(
                            "a recorded tree contains an unsupported file type".to_owned(),
                        ));
                    }
                };
                let oid = objects.get(&entry.p).ok_or_else(|| {
                    HostError::Unsupported("a recorded file has no owned blob".to_owned())
                })?;
                children
                    .entry(path.parent().unwrap_or_else(RelPath::root).to_string())
                    .or_default()
                    .push((
                        path.file_name().unwrap_or_default().to_owned(),
                        mode,
                        oid.clone(),
                    ));
            }
        }
        let mut directory_order: Vec<String> = directories.iter().cloned().collect();
        directory_order.sort_by_key(|path| {
            std::cmp::Reverse(if path.is_empty() {
                0
            } else {
                path.split('/').count()
            })
        });
        let mut trees: BTreeMap<String, String> = BTreeMap::new();
        for directory in directory_order {
            let mut entries = children.remove(&directory).unwrap_or_default();
            entries.sort_by(|left, right| {
                let key = |child: &(String, &str, String)| {
                    if child.1 == GIT_TREE_MODE {
                        format!("{}/", child.0)
                    } else {
                        child.0.clone()
                    }
                };
                key(left).as_bytes().cmp(key(right).as_bytes())
            });
            let mut body = Vec::new();
            for (name, mode, oid) in entries {
                body.extend_from_slice(format!("{mode} {name}\0").as_bytes());
                let raw = hex::decode(&oid).map_err(|_| {
                    HostError::Unsupported("a recorded object id is invalid".to_owned())
                })?;
                if raw.len() != OBJECT_ID_BYTES {
                    return Err(HostError::Unsupported(
                        "a recorded object id has the wrong size".to_owned(),
                    ));
                }
                body.extend(raw);
            }
            let oid = if verify {
                let mut hash = Sha1::new();
                hash.update(format!("tree {}\0", body.len()).as_bytes());
                hash.update(&body);
                let oid = hex::encode(hash.finalize());
                self.verify_object(&oid, "tree", Some(body.len() as i64), Some(Hash::of(&body)))?;
                oid
            } else {
                self.write_object("tree", &body)?
            };
            let path = RelPath::parse(&directory, PathStyle::Posix)?;
            if let Some(parent) = path.parent() {
                children.entry(parent.to_string()).or_default().push((
                    path.file_name().unwrap_or_default().to_owned(),
                    GIT_TREE_MODE,
                    oid.clone(),
                ));
            }
            trees.insert(directory, oid);
        }
        trees
            .remove("")
            .ok_or_else(|| HostError::Internal("the recorded root tree was not built".to_owned()))
    }

    /// Publishes a complete root only after its manifest and every owned object are durable.
    pub fn keep(&self, state: &StateRoot, manifest: &[u8]) -> Result<()> {
        RelPath::parse(&state.directory, PathStyle::Posix)?;
        if Hash::of(manifest) != state.manifest_blob {
            return Err(HostError::Unsupported(
                "a recorded manifest has the wrong hash".to_owned(),
            ));
        }
        self.verify_object(&state.tree_ref, "tree", None, None)?;
        self.write_metadata(
            &self
                .root
                .join("manifests")
                .join(state.manifest_blob.to_string()),
            manifest,
        )?;
        self.keep_root_metadata(state)
    }

    /// Reestablishes the same root/reference after a visible rename whose durability failed.
    pub(crate) fn keep_root_metadata(&self, state: &StateRoot) -> Result<()> {
        let bytes =
            serde_json::to_vec(state).map_err(|error| HostError::Internal(error.to_string()))?;
        self.write_metadata(
            &self
                .root
                .join("roots")
                .join(format!("{}.json", state.snapshot_id)),
            &bytes,
        )?;
        self.write_metadata(
            &self
                .root
                .join("git/refs/snapshots")
                .join(state.snapshot_id.to_string()),
            format!("{}\n", state.tree_ref).as_bytes(),
        )
    }

    /// Loads the exact root and verifies its manifest and expiry; it never substitutes live files.
    pub fn load(&self, snapshot: Id, now: Time) -> Result<(StateRoot, manifest::Parsed)> {
        self.load_checked(snapshot, Some(now))
    }

    /// Verifies a root held by a persisted restore journal even if ordinary retention expired.
    pub(crate) fn load_for_recovery(&self, snapshot: Id) -> Result<(StateRoot, manifest::Parsed)> {
        self.load_checked(snapshot, None)
    }

    fn load_checked(
        &self,
        snapshot: Id,
        now: Option<Time>,
    ) -> Result<(StateRoot, manifest::Parsed)> {
        let bytes = read_bounded(
            &self.root.join("roots").join(format!("{snapshot}.json")),
            manifest::MANIFEST_MAX_EXPANDED_BYTES,
        )?;
        let state: StateRoot = serde_json::from_slice(&bytes).map_err(|error| {
            HostError::Unsupported(format!("recorded root metadata is invalid: {error}"))
        })?;
        if state.snapshot_id != snapshot {
            return Err(HostError::Unsupported(
                "recorded root identity does not match".to_owned(),
            ));
        }
        if now.is_some_and(|now| state.expires_at <= now) {
            return Err(HostError::Unsupported("expired".to_owned()));
        }
        RelPath::parse(&state.directory, PathStyle::Posix)?;
        self.verify_object(&state.tree_ref, "tree", None, None)?;
        let bytes = read_bounded(
            &self
                .root
                .join("manifests")
                .join(state.manifest_blob.to_string()),
            manifest::MANIFEST_MAX_EXPANDED_BYTES,
        )?;
        if Hash::of(&bytes) != state.manifest_blob {
            return Err(HostError::Unsupported(
                "recorded manifest hash does not match".to_owned(),
            ));
        }
        let parsed = manifest::parse(&bytes)?;
        if self.tree(&parsed.entries, &state.objects, true)? != state.tree_ref {
            return Err(HostError::Unsupported(
                "recorded manifest and tree do not match".to_owned(),
            ));
        }
        Ok((state, parsed))
    }

    fn verify_object(
        &self,
        oid: &str,
        wanted_kind: &str,
        wanted_size: Option<i64>,
        wanted_hash: Option<Hash>,
    ) -> Result<tempfile::NamedTempFile> {
        let file = File::open(self.object_path(oid)?)
            .map_err(|error| HostError::io("opening a recorded object", error))?;
        let mut source = BufReader::new(ZlibDecoder::new(file));
        let mut header = Vec::new();
        (&mut source)
            .take(OBJECT_HEADER_MAX_BYTES as u64)
            .read_until(0, &mut header)
            .map_err(|error| HostError::io("reading a recorded object header", error))?;
        if header.last() != Some(&0) {
            return Err(HostError::Unsupported(
                "a recorded object header is invalid".to_owned(),
            ));
        }
        let text = std::str::from_utf8(&header[..header.len() - 1]).map_err(|_| {
            HostError::Unsupported("a recorded object header is invalid".to_owned())
        })?;
        let (kind, size) = text.split_once(' ').ok_or_else(|| {
            HostError::Unsupported("a recorded object header is invalid".to_owned())
        })?;
        let size: i64 = size
            .parse()
            .map_err(|_| HostError::Unsupported("a recorded object size is invalid".to_owned()))?;
        let bound = wanted_size
            .filter(|size| *size >= 0)
            .map_or(manifest::MANIFEST_MAX_EXPANDED_BYTES, |size| size as u64);
        if kind != wanted_kind
            || size < 0
            || size as u64 > bound
            || wanted_size.is_some_and(|wanted| wanted != size)
        {
            return Err(HostError::Unsupported(
                "a recorded object has the wrong type or size".to_owned(),
            ));
        }
        let mut out = tempfile::NamedTempFile::new_in(&self.root)
            .map_err(|error| HostError::io("staging verified recorded bytes", error))?;
        let mut object_hash = Sha1::new();
        object_hash.update(&header);
        let mut content_hash = Hash::hasher();
        let mut buffer = vec![0u8; READ_CHUNK_BYTES];
        let mut total = 0i64;
        loop {
            let count = source
                .read(&mut buffer)
                .map_err(|error| HostError::io("reading a recorded object", error))?;
            if count == 0 {
                break;
            }
            total = total.checked_add(count as i64).ok_or_else(|| {
                HostError::Unsupported("a recorded object is too large".to_owned())
            })?;
            if total > size {
                return Err(HostError::Unsupported(
                    "a recorded object expanded beyond its declared size".to_owned(),
                ));
            }
            object_hash.update(&buffer[..count]);
            content_hash.update(&buffer[..count]);
            out.write_all(&buffer[..count])
                .map_err(|error| HostError::io("staging verified recorded bytes", error))?;
        }
        if total != size
            || hex::encode(object_hash.finalize()) != oid
            || wanted_hash.is_some_and(|expected| expected != Hash::finish(&content_hash))
        {
            return Err(HostError::Unsupported(
                "a recorded object failed hash verification".to_owned(),
            ));
        }
        Ok(out)
    }

    /// Reads one verified file or raw link, including its BLAKE3 and Git object checks.
    pub fn read_entry(
        &self,
        state: &StateRoot,
        entry: &ManifestEntry,
    ) -> Result<tempfile::NamedTempFile> {
        let oid = state.objects.get(&entry.p).ok_or_else(|| {
            HostError::Unsupported("the recorded entry has no owned object".to_owned())
        })?;
        let (size, hash) = match entry.t {
            EntryKind::File => (entry.s, entry.h),
            EntryKind::Link => {
                let link = entry.l.as_ref().ok_or_else(|| {
                    HostError::Unsupported("recorded link text is unavailable".to_owned())
                })?;
                (
                    i64::try_from(link.len()).map_err(|_| {
                        HostError::Unsupported("recorded link text is too large".to_owned())
                    })?,
                    Some(Hash::of(link.as_bytes())),
                )
            }
            _ => {
                return Err(HostError::Unsupported(
                    "only recorded files and links have raw bytes".to_owned(),
                ));
            }
        };
        self.verify_object(oid, "blob", Some(size), hash)
    }
}

#[cfg(all(test, unix))]
mod review_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_pins_survive_their_crash_deadline_but_an_abandoned_durable_pin_expires()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let root = tempfile::tempdir()?;
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace)?;
        let store = ObjectStore::open(&root.path().join("kept"), &workspace)?;
        let now: Time = "2026-10-16T00:00:00Z".parse()?;
        let until = Time::new(
            now.get()
                + chrono::Duration::seconds(i64::from(
                    orch_link_contract::commands::CommandKind::FsRecord.deadline_secs(),
                )),
        );
        let snapshot = orch_types::new_id();
        let operation = orch_types::new_id();
        let pin = store.pin(snapshot, operation, until)?;
        let reopened = ObjectStore::open(store.root(), &workspace)?;
        let later = Time::new(until.get() + chrono::Duration::seconds(1));
        assert!(reopened.pinned(snapshot, later)?);
        assert!(reopened.require_idle().is_err());
        let duplicate = reopened.pin(snapshot, operation, until)?;
        drop(pin);
        assert!(reopened.pinned(snapshot, later)?);
        assert!(reopened.require_idle().is_err());
        assert!(
            store
                .root()
                .join("pins")
                .join(snapshot.to_string())
                .join(operation.to_string())
                .is_file()
        );
        drop(duplicate);
        assert!(!reopened.pinned(snapshot, later)?);
        // A process crash leaves the file, but no live in-memory owner. Recreate that exact
        // durable format without leaking a live guard in this process.
        atomic_write(
            &store
                .root()
                .join("pins")
                .join(snapshot.to_string())
                .join(operation.to_string()),
            &serde_json::to_vec(&until)?,
        )?;
        assert!(reopened.require_idle().is_ok());
        assert!(reopened.pinned(snapshot, now)?);
        assert!(!reopened.pinned(snapshot, later)?);
        Ok(())
    }

    #[test]
    fn raw_blobs_are_git_objects_without_running_filters_or_borrowing_project_objects() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let store = ObjectStore::open(&root.path().join("kept"), &workspace).unwrap();
        let oid = store.write_object("blob", b"hello\n").unwrap();
        assert_eq!(oid, "ce013625030ba8dba906f756967f9e9ca394464a");
        let verified = store
            .verify_object(&oid, "blob", Some(6), Some(Hash::of(b"hello\n")))
            .unwrap();
        assert_eq!(std::fs::read(verified.path()).unwrap(), b"hello\n");
        std::fs::write(store.object_path(&oid).unwrap(), b"corrupt").unwrap();
        assert!(
            store
                .verify_object(&oid, "blob", Some(6), Some(Hash::of(b"hello\n")))
                .is_err()
        );
        assert!(!workspace.join(".git").exists());
    }

    #[test]
    fn a_store_cannot_be_created_in_a_project_and_object_names_cannot_traverse() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        assert!(ObjectStore::open(&workspace.join("states"), &workspace).is_err());
        assert!(!workspace.join("states").exists());
        let store = ObjectStore::open(&root.path().join("kept"), &workspace).unwrap();
        assert!(store.object_path("../../outside").is_err());
    }
}
