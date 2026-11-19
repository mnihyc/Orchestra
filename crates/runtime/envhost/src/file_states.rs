//! Complete file-state baselines prepared from the index and refreshed only for watcher changes.
//!
//! Background index work owns the first complete baseline and changed-file reads. Recording a
//! turn publishes immutable metadata from that baseline; it never walks a directory or invokes
//! Git. Missing coverage, a live tree, lost notices, changed bytes and a byte-bound failure are
//! explained fallbacks. Immutable roots remain readable after a connector restart and project
//! garbage collection because every retained object belongs to the separate store.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Mutex, PoisonError};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use orch_link_contract::source::{FsRecord, FsRecordResult, RecordState};
use orch_types::{Hash, Time};

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::ignore::IGNORE_FILE_MAX_BYTES;
use crate::index::{Index, IndexEntry};
use crate::manifest::{self, EntryKind, ManifestEntry};
use crate::paths::{Locate, PathStyle, RelPath};
use crate::platform;
use crate::state_store::{ObjectStore, StateRoot};

/// The shipped maximum from the recording setting: one GiB of included code and fixtures.
pub const DEFAULT_RECORD_MAX_BYTES: i64 = 1024 * 1024 * 1024;
/// The shipped keeping period from the recording setting: a working week and its review.
pub const DEFAULT_RECORD_DAYS: i64 = 7;

/// Reads only bounded local Git metadata, with every resolved file kept inside the workspace.
/// No Git process, fetch, filter, helper or directory scan belongs on the recording path.
fn known_head(files: &Files, directory: &RelPath) -> Option<String> {
    let workspace = files.root().root_path();
    let read = |path: &std::path::Path, limit| -> Option<Vec<u8>> {
        let path = std::fs::canonicalize(path).ok()?;
        if !path.starts_with(workspace) {
            return None;
        }
        crate::state_store::read_bounded(&path, limit).ok()
    };
    let mut repository = directory.clone();
    loop {
        let at = repository.under(workspace);
        let entry = at.join(".git");
        let metadata = std::fs::symlink_metadata(&entry);
        let git_dir = match metadata {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                std::fs::canonicalize(entry).ok()?
            }
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                let bytes = read(&entry, crate::ignore::IGNORE_FILE_MAX_BYTES)?;
                let text = std::str::from_utf8(&bytes).ok()?.trim();
                let target = text.strip_prefix("gitdir: ")?;
                std::fs::canonicalize(at.join(target)).ok()?
            }
            Ok(_) => return None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                repository = repository.parent()?;
                continue;
            }
            Err(_) => return None,
        };
        if !git_dir.starts_with(workspace) {
            return None;
        }
        let original = read(&git_dir.join("HEAD"), crate::ignore::IGNORE_FILE_MAX_BYTES)?;
        let head = std::str::from_utf8(&original).ok()?.trim();
        let value = if let Some(reference) = head.strip_prefix("ref: ") {
            if !reference.starts_with("refs/")
                || reference
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
            {
                return None;
            }
            let common = match read(
                &git_dir.join("commondir"),
                crate::ignore::IGNORE_FILE_MAX_BYTES,
            ) {
                Some(bytes) => {
                    std::fs::canonicalize(git_dir.join(std::str::from_utf8(&bytes).ok()?.trim()))
                        .ok()?
                }
                None => git_dir.clone(),
            };
            if !common.starts_with(workspace) {
                return None;
            }
            if let Some(bytes) = read(
                &common.join(reference),
                crate::ignore::IGNORE_FILE_MAX_BYTES,
            ) {
                std::str::from_utf8(&bytes).ok()?.trim().to_owned()
            } else {
                // Packed refs share the protected Git reader's existing eight-MiB bound.
                let bytes = read(&common.join("packed-refs"), 8 * 1024 * 1024)?;
                std::str::from_utf8(&bytes).ok()?.lines().find_map(|line| {
                    let (oid, name) = line.split_once(' ')?;
                    (name == reference).then(|| oid.to_owned())
                })?
            }
        } else {
            head.to_owned()
        };
        if !matches!(value.len(), 40 | 64)
            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            || read(&git_dir.join("HEAD"), crate::ignore::IGNORE_FILE_MAX_BYTES)? != original
        {
            return None;
        }
        return Some(value);
    }
}

#[derive(Debug, Clone)]
struct CachedEntry {
    index: IndexEntry,
    manifest: ManifestEntry,
    object: Option<String>,
}

#[derive(Debug, Clone)]
struct Baseline {
    max_bytes: i64,
    ready: bool,
    reason: &'static str,
    entries: BTreeMap<String, CachedEntry>,
    ignore_fingerprint: Option<Hash>,
}

impl Baseline {
    fn missing(max_bytes: i64) -> Self {
        Self {
            max_bytes,
            ready: false,
            reason: "not_ready",
            entries: BTreeMap::new(),
            ignore_fingerprint: None,
        }
    }
}

pub(crate) struct GitRules {
    scoped: Vec<(RelPath, Gitignore)>,
    pub(crate) fingerprint: Hash,
}

impl GitRules {
    pub(crate) fn read(
        files: &Files,
        indexed: &[(RelPath, IndexEntry)],
        directory: &RelPath,
    ) -> Result<Self> {
        let mut directories = std::collections::BTreeSet::from([RelPath::root()]);
        let mut ancestor = Some(directory.clone());
        while let Some(path) = ancestor {
            directories.insert(path.clone());
            ancestor = path.parent();
        }
        for (path, entry) in indexed {
            if entry.kind == platform::Kind::Dir && !path.has_component(".git") {
                directories.insert(path.clone());
            }
        }
        let mut fingerprints = BTreeMap::new();
        let mut scoped = Vec::new();
        for base in directories {
            if !base.starts_with(directory) && !directory.starts_with(&base) {
                continue;
            }
            for name in [".gitignore", crate::ignore::IGNORE_FILE] {
                let path = base.join(name)?;
                let located = match files.root().locate(&path, Locate::ENTRY) {
                    Ok(located) => located,
                    Err(HostError::NotFound(_)) => continue,
                    Err(error) => return Err(error),
                };
                let Some(meta) = located.meta else { continue };
                if !meta.is_file() || meta.size > IGNORE_FILE_MAX_BYTES {
                    return Err(HostError::Unsupported(
                        "recording ignore rules are unavailable".to_owned(),
                    ));
                }
                let file = located
                    .parent
                    .open_read(located.name()?)
                    .map_err(|error| HostError::io("reading recording ignore rules", error))?;
                let before = platform::file_meta(&file)
                    .map_err(|error| HostError::io("checking recording ignore rules", error))?;
                let mut bytes = Vec::new();
                (&file)
                    .take(IGNORE_FILE_MAX_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| HostError::io("reading recording ignore rules", error))?;
                let after = platform::file_meta(&file)
                    .map_err(|error| HostError::io("checking recording ignore rules", error))?;
                if bytes.len() as u64 > IGNORE_FILE_MAX_BYTES
                    || before.identity() != after.identity()
                {
                    return Err(HostError::Changed(
                        "recording ignore rules changed while read".to_owned(),
                    ));
                }
                fingerprints.insert(path.to_string(), Hash::of(&bytes));
                if name == ".gitignore" {
                    let text = std::str::from_utf8(&bytes).map_err(|_| {
                        HostError::Unsupported("recording ignore rules are not UTF-8".to_owned())
                    })?;
                    let mut builder = GitignoreBuilder::new(".");
                    builder
                        .case_insensitive(!files.root().case_sensitive())
                        .map_err(|error| HostError::Unsupported(error.to_string()))?;
                    for line in text.lines() {
                        builder
                            .add_line(None, line)
                            .map_err(|error| HostError::Unsupported(error.to_string()))?;
                    }
                    scoped.push((
                        base.clone(),
                        builder
                            .build()
                            .map_err(|error| HostError::Unsupported(error.to_string()))?,
                    ));
                }
            }
        }
        scoped.sort_by_key(|(path, _)| path.depth());
        let configured = files
            .ignore()
            .map(|rules| rules.configured_patterns())
            .unwrap_or(&[]);
        let bytes = serde_json::to_vec(
            &serde_json::json!({ "configured": configured, "files": fingerprints,
            "case_sensitive": files.root().case_sensitive(), "directory": directory.as_str() }),
        )
        .map_err(|error| HostError::Internal(error.to_string()))?;
        Ok(Self {
            scoped,
            fingerprint: Hash::of(&bytes),
        })
    }

    pub(crate) fn ignored(&self, path: &RelPath, is_dir: bool) -> bool {
        if path.has_component(".git") {
            return true;
        }
        let names: Vec<&str> = path.components().collect();
        for end in 1..=names.len() {
            let candidate = names[..end].join("/");
            let Ok(candidate) = RelPath::parse(&candidate, PathStyle::Posix) else {
                return true;
            };
            let mut ignored = false;
            for (base, rules) in &self.scoped {
                let Some(relative) = candidate
                    .strip_prefix(base)
                    .filter(|relative| !relative.is_root())
                else {
                    continue;
                };
                match rules.matched(relative.as_str(), end < names.len() || is_dir) {
                    ignore::Match::Ignore(_) => ignored = true,
                    ignore::Match::Whitelist(_) => ignored = false,
                    ignore::Match::None => {}
                }
            }
            if ignored {
                return true;
            }
        }
        false
    }
}

/// The indexed baselines and immutable roots owned by one attached environment.
#[derive(Debug)]
pub struct FileStates {
    store: ObjectStore,
    baselines: Mutex<BTreeMap<RelPath, Baseline>>,
    recording: Mutex<()>,
    refreshing: Mutex<()>,
    actions: tokio::sync::Mutex<()>,
    epoch: std::sync::atomic::AtomicU64,
}

/// One recorded read, with the exact immutable root pinned while verified bytes are transferred.
#[derive(Debug)]
pub struct RecordedRead {
    /// The immutable recorded root.
    pub root: StateRoot,
    /// The recorded entries, without substituting live files.
    pub entries: Vec<ManifestEntry>,
    /// The common manifest verified against its original local hash.
    pub manifest: Vec<u8>,
    /// The bounded operation's durable protection from pruning.
    pub pin: crate::state_store::StatePin,
}

/// The command identity, existing budget and cancellation signal of one local capture.
pub struct RecordOperation {
    /// The link command, distinct from its immutable snapshot identity.
    pub id: orch_types::Id,
    /// Crash protection ends at this command's existing operation budget.
    pub until: Time,
    /// Cancellation is observed between local steps; the caller must join those steps.
    pub cancel: tokio_util::sync::CancellationToken,
}

/// A durable reply, immutable file input and a lease independent of the publication future.
#[derive(Debug)]
pub struct CapturedState {
    /// The exact locally persisted outcome.
    pub result: FsRecordResult,
    /// Only the compressed manifest travels to the system.
    pub input: Option<crate::state_store::ManifestInput>,
    /// Kept through command finalization, including journal-write retries.
    pub hold: Option<crate::state_store::StatePin>,
}

impl CapturedState {
    /// Publishes both phases for a direct caller without a command journal.
    pub async fn publish(self, sink: &dyn crate::blobs::BlobSink) -> Result<FsRecordResult> {
        let Self {
            result,
            input,
            hold,
        } = self;
        let outcome = match input {
            Some(input) => input.publish(sink).await,
            None => Ok(()),
        };
        drop(hold);
        outcome.map(|()| result)
    }
}

impl FileStates {
    /// Begins without a complete state until the background index work prepares it.
    #[must_use]
    pub fn new(store: ObjectStore) -> Self {
        Self {
            store,
            baselines: Mutex::new(BTreeMap::from([(
                RelPath::root(),
                Baseline::missing(DEFAULT_RECORD_MAX_BYTES),
            )])),
            recording: Mutex::new(()),
            refreshing: Mutex::new(()),
            actions: tokio::sync::Mutex::new(()),
            epoch: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The environment-owned immutable store, for reads, moves and restore journals.
    #[must_use]
    pub fn store(&self) -> &ObjectStore {
        &self.store
    }

    /// Serializes restart-safe source and Git mutations without blocking watcher hashing.
    pub(crate) async fn action_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.actions.lock().await
    }

    pub(crate) fn change_gate(
        &self,
    ) -> (std::sync::MutexGuard<'_, ()>, std::sync::MutexGuard<'_, ()>) {
        (
            self.recording
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            self.refreshing
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Opens the exact recorded state, checking current ignore rules before returning any bytes.
    pub fn read_recorded(
        &self,
        files: &Files,
        index: &Index,
        request: &orch_link_contract::source::FsRecordRead,
        operation: orch_types::Id,
        now: Time,
    ) -> Result<RecordedRead> {
        self.read_recorded_inner(files, index, request, operation, now, false)
    }

    /// Reads a state already owned by a verified persisted worktree journal during recovery.
    pub(crate) fn read_recorded_recovery(
        &self,
        files: &Files,
        index: &Index,
        request: &orch_link_contract::source::FsRecordRead,
        operation: orch_types::Id,
        now: Time,
    ) -> Result<RecordedRead> {
        self.read_recorded_inner(files, index, request, operation, now, true)
    }

    fn read_recorded_inner(
        &self,
        files: &Files,
        index: &Index,
        request: &orch_link_contract::source::FsRecordRead,
        operation: orch_types::Id,
        now: Time,
        recovery: bool,
    ) -> Result<RecordedRead> {
        let _reading = self
            .recording
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if request.export && request.path.is_some() {
            return Err(HostError::Unsupported(
                "an export names the whole state, not one path".to_owned(),
            ));
        }
        let until = now
            .get()
            .checked_add_signed(chrono::Duration::seconds(i64::from(
                orch_link_contract::commands::CommandKind::FsRecordRead.deadline_secs(),
            )))
            .ok_or_else(|| {
                HostError::Unsupported("the recorded read deadline is invalid".to_owned())
            })?;
        let pin = self
            .store
            .pin(request.snapshot_id, operation, Time::new(until))?;
        let (root, parsed) = if recovery {
            self.store.load_for_recovery(request.snapshot_id)?
        } else {
            self.store.load(request.snapshot_id, now)?
        };
        let directory = files.parse(&root.directory)?;
        let mut indexed = index.entries();
        for entry in &parsed.entries {
            if entry.t != EntryKind::Dir {
                continue;
            }
            let full = join(&directory, &files.parse(&entry.p)?, files)?;
            indexed.push((
                full,
                IndexEntry {
                    kind: platform::Kind::Dir,
                    size: 0,
                    mtime_ns: entry.m,
                    mode: entry.x,
                    link_target: None,
                },
            ));
        }
        let mut ancestor = Some(directory.clone());
        while let Some(path) = ancestor {
            indexed.push((
                path.clone(),
                IndexEntry {
                    kind: platform::Kind::Dir,
                    size: 0,
                    mtime_ns: 0,
                    mode: 0,
                    link_target: None,
                },
            ));
            ancestor = path.parent();
        }
        let rules = GitRules::read(files, &indexed, &directory)?;
        let requested = request
            .path
            .as_deref()
            .map(|path| files.parse(path))
            .transpose()?;
        let mut entries = Vec::new();
        for entry in parsed.entries {
            if requested
                .as_ref()
                .is_some_and(|path| path.as_str() != entry.p)
            {
                continue;
            }
            let full = join(&directory, &files.parse(&entry.p)?, files)?;
            let is_dir = entry.t == EntryKind::Dir;
            if rules.ignored(&full, is_dir)
                || files
                    .ignore()
                    .is_some_and(|ignore| ignore.is_ignored(files.root(), &full, is_dir))
            {
                return Err(HostError::Ignored(
                    "current ignore rules hide required recorded files".to_owned(),
                ));
            }
            entries.push(entry);
        }
        if requested.is_some() && entries.len() != 1 {
            return Err(HostError::NotFound(
                "the recorded path is not in this state".to_owned(),
            ));
        }
        let manifest = crate::state_store::read_bounded(
            &self
                .store
                .root()
                .join("manifests")
                .join(root.manifest_blob.to_string()),
            manifest::MANIFEST_MAX_EXPANDED_BYTES,
        )?;
        if Hash::of(&manifest) != root.manifest_blob {
            return Err(HostError::Unsupported(
                "recorded manifest verification failed".to_owned(),
            ));
        }
        Ok(RecordedRead {
            root,
            entries,
            manifest,
            pin,
        })
    }

    /// The currently prepared objects that pruning must keep even before their next turn root.
    pub(crate) fn cached_objects(&self) -> std::collections::BTreeSet<String> {
        self.baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .flat_map(|state| {
                state
                    .entries
                    .values()
                    .filter_map(|entry| entry.object.clone())
            })
            .collect()
    }

    /// Releases expired unpinned roots and unreferenced objects under the capture/refresh gates.
    pub fn prune(&self, expired: &[orch_types::Id], now: Time) -> Result<i64> {
        let _recording = self
            .recording
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _refreshing = self
            .refreshing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.store.prune(expired, now, self.cached_objects())
    }

    /// Verifies carried roots while baseline refresh and collection cannot change their objects.
    pub fn verify_kept(
        &self,
        now: Time,
    ) -> Result<orch_link_contract::source::FsRecordVerifyResult> {
        let _recording = self
            .recording
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _refreshing = self
            .refreshing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.store.verify_kept(now)
    }

    /// Requests an off-turn baseline for a new directory or larger explicit recording bound.
    pub fn request(&self, directory: &RelPath, max_bytes: i64) -> bool {
        let mut baselines = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match baselines.get_mut(directory) {
            None => {
                let inherited = baselines
                    .iter()
                    .filter(|(base, state)| directory.starts_with(base) && state.ready)
                    .max_by_key(|(base, _)| base.depth())
                    .and_then(|(base, state)| {
                        let relative = directory.strip_prefix(base)?;
                        if !state
                            .entries
                            .get(relative.as_str())
                            .is_some_and(|entry| entry.manifest.t == EntryKind::Dir)
                        {
                            return None;
                        }
                        let mut subset = Baseline::missing(max_bytes);
                        for (path, entry) in &state.entries {
                            let path = RelPath::parse(path, PathStyle::Posix).ok()?;
                            if let Some(child) =
                                path.strip_prefix(&relative).filter(|path| !path.is_root())
                            {
                                let mut entry = entry.clone();
                                entry.manifest.p = child.to_string();
                                subset.entries.insert(child.to_string(), entry);
                            }
                        }
                        // Raw objects can be inherited, but ignore fingerprints are scoped to
                        // their directory. The first off-turn seed verifies the new scope.
                        subset.ready = false;
                        subset.ignore_fingerprint = None;
                        Some(subset)
                    });
                let needs_seed = inherited.as_ref().is_none_or(|state| !state.ready);
                baselines.insert(
                    directory.clone(),
                    inherited.unwrap_or_else(|| Baseline::missing(max_bytes)),
                );
                needs_seed
            }
            Some(baseline) if max_bytes > baseline.max_bytes => {
                baseline.max_bytes = max_bytes;
                baseline.ready = false;
                baseline.reason = "not_ready";
                true
            }
            _ => false,
        }
    }

    /// Whether the index worker still owes this scope its first/current baseline.
    /// Definitive live-tree and byte-limit fallbacks do not start extra preparation.
    pub(crate) fn needs_preparing(&self, directory: &RelPath) -> bool {
        self.baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(directory)
            .is_none_or(|baseline| {
                !baseline.ready && matches!(baseline.reason, "not_ready" | "off")
            })
    }

    /// Makes coverage unknown immediately when watching loses events or ignore rules change.
    pub fn invalidate(&self, reason: &'static str) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for baseline in self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values_mut()
        {
            baseline.ready = false;
            baseline.reason = reason;
        }
    }

    /// Makes affected scopes unknown before background work refreshes their changed files.
    pub fn changed(&self, paths: &[RelPath]) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for (directory, baseline) in self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter_mut()
        {
            if paths
                .iter()
                .any(|path| path.starts_with(directory) || directory.starts_with(path))
            {
                baseline.ready = false;
                baseline.reason = "not_ready";
            }
        }
    }

    /// Refreshes requested scopes from the complete in-memory index, hashing only changed files.
    /// This runs with background index work, never in the turn's capture or observation path.
    pub fn refresh(
        &self,
        files: &Files,
        index: &Index,
        changed: Option<&[RelPath]>,
        complete: bool,
    ) -> Result<()> {
        let _refreshing = self
            .refreshing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !index.state().is_full() {
            self.invalidate("live_tree");
            return Ok(());
        }
        if !complete || index.changes_since(index.revision()).is_none() {
            self.invalidate("not_ready");
            return Ok(());
        }
        let indexed = index.entries();
        let epoch = self.epoch.load(std::sync::atomic::Ordering::SeqCst);
        let scopes: Vec<(RelPath, Baseline)> = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(path, state)| (path.clone(), state.clone()))
            .collect();
        for (directory, previous) in scopes {
            if self
                .store
                .policy(&directory)?
                .is_some_and(|policy| policy.record_days == 0)
            {
                let mut baseline = Baseline::missing(previous.max_bytes);
                baseline.reason = "off";
                self.baselines
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(directory, baseline);
                continue;
            }
            let result = self.prepare(files, &indexed, &directory, &previous, changed);
            let mut baselines = self
                .baselines
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if epoch != self.epoch.load(std::sync::atomic::Ordering::SeqCst) {
                continue;
            }
            match result {
                Ok(prepared) => {
                    baselines.insert(directory, prepared);
                }
                Err(error) => {
                    let baseline = baselines.entry(directory).or_insert(previous);
                    baseline.ready = false;
                    baseline.reason = match error {
                        HostError::TooLarge { .. } => "too_large",
                        HostError::Changed(_) => "changed_during_recording",
                        _ => "unreadable",
                    };
                }
            }
        }
        Ok(())
    }

    fn prepare(
        &self,
        files: &Files,
        indexed: &[(RelPath, IndexEntry)],
        directory: &RelPath,
        previous: &Baseline,
        changed: Option<&[RelPath]>,
    ) -> Result<Baseline> {
        let located = files.root().locate(directory, Locate::THROUGH)?;
        if !located.meta.is_some_and(|meta| meta.is_dir()) || located.real != *directory {
            return Err(HostError::NotFound(
                "a recorded directory is absent or follows a link".to_owned(),
            ));
        }
        files.check_visible(directory, &located, true)?;
        let rules = GitRules::read(files, indexed, directory)?;
        let mut included = Vec::new();
        let mut total = 0i64;
        for (path, entry) in indexed {
            let Some(relative) = path.strip_prefix(directory).filter(|path| !path.is_root()) else {
                continue;
            };
            if rules.ignored(path, entry.kind == platform::Kind::Dir) {
                continue;
            }
            let size = match entry.kind {
                platform::Kind::File => i64::try_from(entry.size).map_err(|_| {
                    HostError::Unsupported("a recorded file is too large".to_owned())
                })?,
                platform::Kind::Symlink => entry
                    .link_target
                    .as_ref()
                    .map_or(0, |link| link.len() as i64),
                platform::Kind::Dir => 0,
                platform::Kind::Other => {
                    return Err(HostError::Unsupported(
                        "the directory holds an unsupported file type".to_owned(),
                    ));
                }
            };
            total = total.checked_add(size).ok_or_else(|| {
                HostError::Unsupported("recorded byte sizes overflowed".to_owned())
            })?;
            if total > previous.max_bytes {
                return Err(HostError::TooLarge {
                    what: "the included directory".to_owned(),
                    size: total,
                    limit: previous.max_bytes,
                });
            }
            included.push((path, relative, entry));
        }
        let mut entries = BTreeMap::new();
        for (path, relative, entry) in included {
            let forced = changed.is_none()
                || changed
                    .is_some_and(|paths| paths.iter().any(|changed| path.starts_with(changed)));
            if !forced
                && let Some(known) = previous
                    .entries
                    .get(relative.as_str())
                    .filter(|known| known.index == *entry)
            {
                entries.insert(relative.to_string(), known.clone());
                continue;
            }
            let located = files.root().locate(path, Locate::ENTRY)?;
            let meta = located.meta.ok_or_else(|| {
                HostError::Changed("an included recorded path vanished".to_owned())
            })?;
            files.check_visible(path, &located, meta.is_dir())?;
            if meta.kind != entry.kind
                || meta.mode != entry.mode
                || meta.is_file() && (meta.size != entry.size || meta.mtime_ns != entry.mtime_ns)
            {
                return Err(HostError::Changed(
                    "an included recorded path changed after indexing".to_owned(),
                ));
            }
            let (object, hash, link) = match entry.kind {
                platform::Kind::File => {
                    let mut file = located.parent.open_read(located.name()?).map_err(|error| {
                        HostError::io("opening included recording bytes", error)
                    })?;
                    let (oid, hash) = self.store.write_blob(&mut file, entry.size)?;
                    (Some(oid), Some(hash), None)
                }
                platform::Kind::Symlink => {
                    let link = located
                        .parent
                        .read_link(located.name()?)
                        .map_err(|error| HostError::io("reading recorded link text", error))?
                        .into_string()
                        .map_err(|_| {
                            HostError::Unsupported("recorded link text is not UTF-8".to_owned())
                        })?;
                    if entry.link_target.as_deref() != Some(link.as_str()) {
                        return Err(HostError::Changed(
                            "a recorded link changed after indexing".to_owned(),
                        ));
                    }
                    (
                        Some(self.store.write_object("blob", link.as_bytes())?),
                        None,
                        Some(link),
                    )
                }
                platform::Kind::Dir => (None, None, None),
                platform::Kind::Other => {
                    return Err(HostError::Unsupported(
                        "unsupported recorded file type".to_owned(),
                    ));
                }
            };
            entries.insert(
                relative.to_string(),
                CachedEntry {
                    index: entry.clone(),
                    object,
                    manifest: ManifestEntry {
                        p: relative.to_string(),
                        t: entry.kind.into(),
                        s: if meta.is_file() { entry.size as i64 } else { 0 },
                        m: entry.mtime_ns,
                        x: entry.mode,
                        h: hash,
                        l: link,
                    },
                },
            );
        }
        Ok(Baseline {
            max_bytes: previous.max_bytes,
            ready: true,
            reason: "not_ready",
            entries,
            ignore_fingerprint: Some(rules.fingerprint),
        })
    }

    /// Records only prepared immutable metadata; every fallback is durable under this identity.
    pub fn record(
        &self,
        files: &Files,
        request: &FsRecord,
        generation: i64,
        now: Time,
    ) -> Result<FsRecordResult> {
        let _recording = self
            .recording
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.record_locked(files, request, generation, now)
    }

    /// Capture, open and pin are one critical section with respect to pruning.
    pub fn capture(
        &self,
        files: &Files,
        request: &FsRecord,
        generation: i64,
        now: Time,
        operation: &RecordOperation,
    ) -> Result<CapturedState> {
        let _recording = self
            .recording
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if operation.cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        let result = self.record_locked(files, request, generation, now)?;
        if operation.cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        if result.reason.as_deref() == Some("expired") {
            return Err(HostError::NotFound(
                "the captured state has expired".to_owned(),
            ));
        }
        let (input, hold) = if result.state == RecordState::Complete {
            let (input, pin) = self.store.publication(
                request,
                &result,
                generation,
                now,
                operation.id,
                operation.until,
            )?;
            (Some(input), Some(pin))
        } else {
            (None, None)
        };
        Ok(CapturedState {
            result,
            input,
            hold,
        })
    }

    fn record_locked(
        &self,
        files: &Files,
        request: &FsRecord,
        generation: i64,
        now: Time,
    ) -> Result<FsRecordResult> {
        if let Some(reply) = self.store.recorded_reply(request)? {
            return Ok(reply);
        }
        if self.store.has_root(request.snapshot_id) {
            let (root, _) = self.store.load(request.snapshot_id, now)?;
            let bytes = serde_json::to_vec(request)
                .map_err(|error| HostError::Internal(error.to_string()))?;
            if root.request_hash != Some(Hash::of(&bytes)) {
                return Err(HostError::Changed(
                    "the published recording belongs to another frozen choice".to_owned(),
                ));
            }
            self.store.keep_root_metadata(&root)?;
            let result = root.result();
            self.store.keep_reply(request, &result)?;
            return Ok(result);
        }
        if request.record_days < 0 || request.max_bytes < 1 {
            return Err(HostError::Unsupported(
                "recording limits are invalid".to_owned(),
            ));
        }
        self.store.begin_record(request)?;
        let directory = files.parse(&request.directory)?;
        self.store.keep_policy(
            &directory,
            crate::state_store::RecordPolicy {
                record_days: request.record_days,
                max_bytes: request.max_bytes,
            },
        )?;
        if request.record_days == 0
            && let Some(baseline) = self
                .baselines
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(&directory)
        {
            baseline.ready = false;
            baseline.reason = "off";
            baseline.entries.clear();
        }
        let baseline = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&directory)
            .cloned();
        let result = if request.record_days == 0 {
            unavailable("off")
        } else {
            match baseline {
                None => unavailable("not_ready"),
                Some(baseline) if !baseline.ready => unavailable(if baseline.reason == "off" {
                    "not_ready"
                } else {
                    baseline.reason
                }),
                Some(baseline) => {
                    let bytes = baseline
                        .entries
                        .values()
                        .try_fold(0i64, |total, entry| {
                            total.checked_add(entry.manifest.s).and_then(|total| {
                                total.checked_add(
                                    entry
                                        .manifest
                                        .l
                                        .as_ref()
                                        .map_or(0, |link| link.len() as i64),
                                )
                            })
                        })
                        .ok_or_else(|| {
                            HostError::Unsupported("recorded bytes overflowed".to_owned())
                        })?;
                    if bytes > request.max_bytes {
                        unavailable("too_large")
                    } else {
                        let days =
                            chrono::Duration::try_days(request.record_days).ok_or_else(|| {
                                HostError::Unsupported("recording days are too large".to_owned())
                            })?;
                        let expiry = now.get().checked_add_signed(days).ok_or_else(|| {
                            HostError::Unsupported("recording expiry is too large".to_owned())
                        })?;
                        let entries: Vec<ManifestEntry> = baseline
                            .entries
                            .values()
                            .map(|entry| entry.manifest.clone())
                            .collect();
                        let objects = baseline
                            .entries
                            .iter()
                            .filter_map(|(path, entry)| {
                                entry.object.as_ref().map(|oid| (path.clone(), oid.clone()))
                            })
                            .collect();
                        let manifest = manifest::build(
                            &manifest::header_for(files.root(), &directory, generation, now.get()),
                            entries.clone(),
                        )?;
                        let root = StateRoot {
                            snapshot_id: request.snapshot_id,
                            request_hash: Some(Hash::of(
                                &serde_json::to_vec(request)
                                    .map_err(|error| HostError::Internal(error.to_string()))?,
                            )),
                            directory: directory.to_string(),
                            generation,
                            kind: request.kind,
                            tree_ref: self.store.write_tree(&entries, &objects)?,
                            manifest_blob: manifest.hash,
                            objects,
                            files: manifest.file_count,
                            bytes,
                            created_at: now,
                            expires_at: Time::new(expiry),
                            repository_head: known_head(files, &directory),
                            ignore_fingerprint: baseline.ignore_fingerprint.ok_or_else(|| {
                                HostError::Unsupported(
                                    "recording ignore coverage is not ready".to_owned(),
                                )
                            })?,
                        };
                        self.store.keep(&root, &manifest.bytes)?;
                        root.result()
                    }
                }
            }
        };
        self.store.keep_reply(request, &result)?;
        Ok(result)
    }

    /// Captures the explicitly scanned current directory as complete undo before a restore.
    /// This path belongs only to a person's guarded restore, never to turn preparation.
    pub(crate) fn capture_explicit(
        &self,
        files: &Files,
        indexed: &[(RelPath, IndexEntry)],
        request: &FsRecord,
        generation: i64,
        now: Time,
    ) -> Result<StateRoot> {
        if request.record_days <= 0 || request.max_bytes < 1 {
            return Err(HostError::Unsupported(
                "complete undo recording is disabled".to_owned(),
            ));
        }
        let directory = files.parse(&request.directory)?;
        let prepared = self.prepare(
            files,
            indexed,
            &directory,
            &Baseline::missing(request.max_bytes),
            None,
        )?;
        let entries: Vec<ManifestEntry> = prepared
            .entries
            .values()
            .map(|entry| entry.manifest.clone())
            .collect();
        let objects = prepared
            .entries
            .iter()
            .filter_map(|(path, entry)| {
                entry.object.as_ref().map(|oid| (path.clone(), oid.clone()))
            })
            .collect();
        let built = manifest::build(
            &manifest::header_for(files.root(), &directory, generation, now.get()),
            entries.clone(),
        )?;
        // A complete state must be readable under the same manifest bounds before it is rooted.
        manifest::parse(&built.bytes)?;
        let days = chrono::Duration::try_days(request.record_days)
            .ok_or_else(|| HostError::Unsupported("undo keeping period is too large".to_owned()))?;
        let expires = now
            .get()
            .checked_add_signed(days)
            .ok_or_else(|| HostError::Unsupported("undo expiry is too large".to_owned()))?;
        let root = StateRoot {
            snapshot_id: request.snapshot_id,
            request_hash: Some(Hash::of(
                &serde_json::to_vec(request)
                    .map_err(|error| HostError::Internal(error.to_string()))?,
            )),
            directory: directory.to_string(),
            generation,
            kind: request.kind,
            tree_ref: self.store.write_tree(&entries, &objects)?,
            manifest_blob: built.hash,
            objects,
            files: built.file_count,
            bytes: built.total_bytes
                + entries
                    .iter()
                    .filter_map(|entry| entry.l.as_ref())
                    .map(|link| link.len() as i64)
                    .sum::<i64>(),
            created_at: now,
            expires_at: Time::new(expires),
            repository_head: known_head(files, &directory),
            ignore_fingerprint: prepared.ignore_fingerprint.ok_or_else(|| {
                HostError::Unsupported("undo ignore coverage is unavailable".to_owned())
            })?,
        };
        self.store.keep(&root, &built.bytes)?;
        self.store.keep_reply(request, &root.result())?;
        Ok(root)
    }

    /// Returns an end manifest from the prepared baseline, without reading live files or Git.
    pub fn observe(
        &self,
        files: &Files,
        directory: &RelPath,
        generation: i64,
        now: Time,
    ) -> Result<Option<manifest::Manifest>> {
        let baseline = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(directory)
            .filter(|baseline| baseline.ready)
            .cloned();
        let Some(baseline) = baseline else {
            return Ok(None);
        };
        let entries = baseline
            .entries
            .values()
            .map(|entry| entry.manifest.clone())
            .collect();
        manifest::build(
            &manifest::header_for(files.root(), directory, generation, now.get()),
            entries,
        )
        .map(Some)
    }
}

pub(crate) fn join(directory: &RelPath, relative: &RelPath, files: &Files) -> Result<RelPath> {
    if directory.is_root() {
        return Ok(relative.clone());
    }
    if relative.is_root() {
        return Ok(directory.clone());
    }
    files.parse(&format!("{directory}/{relative}"))
}

/// A capture fallback never supplies a partial state as a whole directory.
#[must_use]
pub fn unavailable(reason: &str) -> FsRecordResult {
    FsRecordResult {
        state: RecordState::Unavailable,
        tree_ref: None,
        repository_head: None,
        manifest_blob: None,
        files: 0,
        bytes: 0,
        expires_at: None,
        reason: Some(reason.to_owned()),
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::blobs::{BlobError, BlobReader, BlobSink};
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::Workspace;
    use orch_link_contract::commands::IndexMode;
    use orch_link_contract::source::RecordKind;
    use orch_types::new_id;
    use std::sync::Arc;

    struct RefuseManifest;
    #[async_trait::async_trait]
    impl BlobSink for RefuseManifest {
        async fn missing(&self, hashes: &[Hash]) -> std::result::Result<Vec<Hash>, BlobError> {
            Ok(hashes.to_vec())
        }
        async fn put(
            &self,
            _: Hash,
            _: i64,
            _: &mut dyn BlobReader,
        ) -> std::result::Result<(), BlobError> {
            Err(BlobError::Refused("synthetic content refusal".to_owned()))
        }
    }

    fn at() -> Time {
        "2026-10-14T12:00:00.000Z".parse().unwrap()
    }

    fn setup() -> (tempfile::TempDir, Files, Index, FileStates) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let rules = Arc::new(IgnoreRules::new(&[], true));
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true).unwrap(),
            rules,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let index = Index::new(IndexMode::Full, Limits::default().index_max_entries);
        let states =
            FileStates::new(ObjectStore::open(&dir.path().join("kept"), &workspace).unwrap());
        (dir, files, index, states)
    }

    fn request() -> FsRecord {
        FsRecord {
            environment_id: new_id(),
            snapshot_id: new_id(),
            directory: String::new(),
            kind: RecordKind::Turn,
            record_days: DEFAULT_RECORD_DAYS,
            max_bytes: DEFAULT_RECORD_MAX_BYTES,
        }
    }

    #[tokio::test]
    async fn publication_keeps_original_binary_input_and_live_gc_protection_through_finalization()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        let original = [0, 255, 1, 2, 0, 3];
        std::fs::write(files.root().root_path().join("code.bin"), original)?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        let request = request();
        let until = Time::new(
            at().get()
                + chrono::Duration::seconds(i64::from(
                    orch_link_contract::commands::CommandKind::FsRecord.deadline_secs(),
                )),
        );
        let operation = RecordOperation {
            id: new_id(),
            until,
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let CapturedState {
            result,
            input,
            hold,
        } = states.capture(&files, &request, 1, at(), &operation)?;
        assert_eq!(result.state, RecordState::Complete);
        assert_eq!((result.files, result.bytes), (1, 6));
        let original_reply = result.clone();
        std::fs::write(
            files.root().root_path().join("code.bin"),
            b"new workspace bytes",
        )?;
        let blobs = crate::blobs::MemoryBlobs::new();
        input
            .ok_or("missing immutable manifest input")?
            .publish(&blobs)
            .await?;
        let hash = result.manifest_blob.ok_or("missing captured hash")?;
        let manifest = manifest::parse(&blobs.get_bytes(&hash).ok_or("manifest not stored")?)?;
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.p == "code.bin")
            .ok_or("missing captured file")?;
        assert_eq!(entry.h, Some(Hash::of(&original)));
        let (root, _) = states.store.load(request.snapshot_id, at())?;
        let saved = states.store.read_entry(&root, entry)?;
        assert_eq!(std::fs::read(saved.path())?, original);
        let expired = Time::new(at().get() + chrono::Duration::days(DEFAULT_RECORD_DAYS + 1));
        let reopened = ObjectStore::open(states.store.root(), files.root().root_path())?;
        assert!(reopened.pinned(request.snapshot_id, expired)?);
        let Err(busy) = reopened.require_idle() else {
            panic!("publication/finalization must retain the store");
        };
        assert_eq!(busy.command_error().code, "busy");
        assert!(busy.command_error().retryable);
        assert_eq!(states.prune(&[request.snapshot_id], expired)?, 0);
        assert_eq!(states.store.recorded_reply(&request)?, Some(original_reply));
        drop(hold);
        assert!(reopened.require_idle().is_ok());
        assert_eq!(states.prune(&[request.snapshot_id], expired)?, 1);
        let retry = RecordOperation {
            id: new_id(),
            until: Time::new(expired.get() + chrono::Duration::hours(6)),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        assert!(
            states
                .capture(&files, &request, 1, expired, &retry)
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_publication_retries_the_original_manifest_after_the_live_baseline_changes()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        let path = files.root().root_path().join("code.bin");
        std::fs::write(&path, b"original captured bytes")?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        let request = request();
        let operation = RecordOperation {
            id: new_id(),
            until: Time::new(
                at().get()
                    + chrono::Duration::seconds(i64::from(
                        orch_link_contract::commands::CommandKind::FsRecord.deadline_secs(),
                    )),
            ),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let captured = states.capture(&files, &request, 1, at(), &operation)?;
        let frozen = captured.result.clone();
        let Err(HostError::Content(cause)) = captured.publish(&RefuseManifest).await else {
            return Err("the content refusal was not reported".into());
        };
        assert!(cause.contains("synthetic content refusal"));
        assert!(states.store.require_idle().is_ok());
        std::fs::write(&path, b"different live bytes after a failed upload")?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        let retry = RecordOperation {
            id: new_id(),
            ..operation
        };
        let captured = states.capture(&files, &request, 1, at(), &retry)?;
        assert_eq!(captured.result, frozen);
        let blobs = crate::blobs::MemoryBlobs::new();
        assert_eq!(captured.publish(&blobs).await?, frozen);
        let hash = frozen.manifest_blob.ok_or("captured manifest absent")?;
        let parsed = manifest::parse(&blobs.get_bytes(&hash).ok_or("published manifest absent")?)?;
        assert_eq!(
            parsed
                .entries
                .iter()
                .find(|entry| entry.p == "code.bin")
                .and_then(|entry| entry.h),
            Some(Hash::of(b"original captured bytes"))
        );
        assert!(states.store.require_idle().is_ok());
        Ok(())
    }

    #[test]
    fn corrupt_saved_reply_or_manifest_is_refused_before_immutable_publication_is_handed_off()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        std::fs::write(files.root().root_path().join("code.txt"), b"captured")?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        let request = request();
        let operation = RecordOperation {
            id: new_id(),
            until: Time::new(
                at().get()
                    + chrono::Duration::seconds(i64::from(
                        orch_link_contract::commands::CommandKind::FsRecord.deadline_secs(),
                    )),
            ),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let captured = states.capture(&files, &request, 1, at(), &operation)?;
        let frozen = captured.result.clone();
        drop(captured);
        let mut altered = frozen.clone();
        altered.expires_at = frozen
            .expires_at
            .map(|expires| Time::new(expires.get() + chrono::Duration::hours(1)));
        states.store.keep_reply(&request, &altered)?;
        assert!(matches!(
            states.capture(&files, &request, 1, at(), &operation),
            Err(HostError::Changed(_))
        ));
        assert!(states.store.require_idle().is_ok());
        states.store.keep_reply(&request, &frozen)?;
        let hash = frozen.manifest_blob.ok_or("captured hash absent")?;
        std::fs::write(
            states.store.root().join("manifests").join(hash.to_string()),
            b"corrupt manifest",
        )?;
        assert!(matches!(
            states.capture(&files, &request, 1, at(), &operation),
            Err(HostError::Unsupported(_))
        ));
        assert_eq!(states.store.recorded_reply(&request)?, Some(frozen));
        assert!(states.store.require_idle().is_ok());
        Ok(())
    }

    #[test]
    fn interrupted_or_missing_original_states_never_capture_new_workspace_bytes_under_the_old_identity()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        std::fs::write(files.root().root_path().join("code.txt"), b"before")?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        let interrupted = request();
        states.store.begin_record(&interrupted)?;
        let operation = RecordOperation {
            id: new_id(),
            until: Time::new(at().get() + chrono::Duration::hours(6)),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        std::fs::write(files.root().root_path().join("code.txt"), b"after")?;
        assert!(matches!(
            states.capture(&files, &interrupted, 1, at(), &operation),
            Err(HostError::Changed(_))
        ));
        assert!(!states.store.has_root(interrupted.snapshot_id));
        let complete = request();
        let captured = states.capture(&files, &complete, 1, at(), &operation)?;
        let frozen = captured.result.clone();
        drop(captured);
        assert!(matches!(
            states.capture(&files, &complete, 2, at(), &operation),
            Err(HostError::Changed(_))
        ));
        std::fs::remove_file(
            states
                .store
                .root()
                .join("roots")
                .join(format!("{}.json", complete.snapshot_id)),
        )?;
        assert!(
            states
                .capture(&files, &complete, 1, at(), &operation)
                .is_err()
        );
        assert_eq!(states.store.recorded_reply(&complete)?, Some(frozen));
        assert!(!states.store.has_root(complete.snapshot_id));
        assert!(
            states.store.require_idle().is_ok(),
            "failed preparation releases its pin"
        );
        Ok(())
    }

    #[test]
    fn a_capture_cancelled_before_local_work_creates_no_root_or_capture_receipt()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, _index, states) = setup();
        let request = request();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let operation = RecordOperation {
            id: new_id(),
            until: Time::new(at().get() + chrono::Duration::hours(6)),
            cancel,
        };
        assert!(matches!(
            states.capture(&files, &request, 1, at(), &operation),
            Err(HostError::Cancelled)
        ));
        assert!(!states.store.has_root(request.snapshot_id));
        assert!(
            !states
                .store
                .root()
                .join("results")
                .join(format!("{}.started", request.snapshot_id))
                .exists()
        );
        assert!(states.store.recorded_reply(&request)?.is_none());
        Ok(())
    }

    #[test]
    fn a_complete_state_owns_raw_bytes_links_empty_directories_and_ignores_without_a_project_repository()
     {
        let (_dir, files, index, states) = setup();
        let root = files.root().root_path();
        std::fs::write(root.join("code.bin"), [0, 255, 1, 2]).unwrap();
        std::fs::write(root.join("secret.txt"), "secret").unwrap();
        std::fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
        std::fs::create_dir(root.join("empty")).unwrap();
        std::os::unix::fs::symlink("../../outside", root.join("link")).unwrap();
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        let request = request();
        let result = states.record(&files, &request, 1, at()).unwrap();
        assert_eq!(result.state, RecordState::Complete);
        let (saved, tree) = states.store.load(request.snapshot_id, at()).unwrap();
        let entries: BTreeMap<&str, &ManifestEntry> = tree
            .entries
            .iter()
            .map(|entry| (entry.p.as_str(), entry))
            .collect();
        assert!(entries.contains_key("empty") && !entries.contains_key("secret.txt"));
        assert_eq!(entries["link"].l.as_deref(), Some("../../outside"));
        let raw = states
            .store
            .read_entry(&saved, entries["code.bin"])
            .unwrap();
        assert_eq!(std::fs::read(raw.path()).unwrap(), [0, 255, 1, 2]);
        std::fs::write(root.join("code.bin"), "newer").unwrap();
        let repeated = states.record(&files, &request, 1, at()).unwrap();
        assert_eq!(
            repeated, result,
            "the same state identity never captures later live files"
        );
        assert!(!root.join(".git").exists());
    }

    #[test]
    fn recorded_reads_use_original_bytes_and_current_ignore_rules_and_pins_survive_expiry() {
        let (_dir, files, index, states) = setup();
        std::fs::write(files.root().root_path().join("code"), "original").unwrap();
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        let request = request();
        let recorded = states.record(&files, &request, 1, at()).unwrap();
        std::fs::write(files.root().root_path().join("code"), "newer live bytes").unwrap();
        let before_expiry = Time::new(
            at().get() + chrono::Duration::days(DEFAULT_RECORD_DAYS) - chrono::Duration::minutes(1),
        );
        let read_request = orch_link_contract::source::FsRecordRead {
            environment_id: request.environment_id,
            snapshot_id: request.snapshot_id,
            path: Some("code".to_owned()),
            export: false,
        };
        let read = states
            .read_recorded(&files, &index, &read_request, new_id(), before_expiry)
            .unwrap();
        let raw = states
            .store
            .read_entry(&read.root, &read.entries[0])
            .unwrap();
        assert_eq!(std::fs::read(raw.path()).unwrap(), b"original");
        let expired = Time::new(before_expiry.get() + chrono::Duration::minutes(2));
        assert_eq!(
            states.prune(&[request.snapshot_id], expired).unwrap(),
            0,
            "a read that began before expiry protects the root"
        );
        assert_eq!(read.root.manifest_blob, recorded.manifest_blob.unwrap());
        drop(read);
        assert_eq!(states.prune(&[request.snapshot_id], expired).unwrap(), 1);
        assert_eq!(
            states.prune(&[request.snapshot_id], expired).unwrap(),
            0,
            "expiry collection is idempotent"
        );
        assert!(states.store.load(request.snapshot_id, expired).is_err());

        let second = FsRecord {
            snapshot_id: new_id(),
            ..request.clone()
        };
        states.record(&files, &second, 1, at()).unwrap();
        std::fs::write(files.root().root_path().join(".gitignore"), "code\n").unwrap();
        let hidden = orch_link_contract::source::FsRecordRead {
            snapshot_id: second.snapshot_id,
            ..read_request
        };
        assert!(matches!(
            states.read_recorded(&files, &index, &hidden, new_id(), at()),
            Err(HostError::Ignored(_))
        ));
    }

    #[test]
    fn not_ready_live_lost_and_too_large_coverage_never_become_partial_restore_targets() {
        let (_dir, files, index, states) = setup();
        let first = states.record(&files, &request(), 1, at()).unwrap();
        assert_eq!(first.reason.as_deref(), Some("not_ready"));
        let live = Index::new(IndexMode::Live, Limits::default().index_max_entries);
        states.refresh(&files, &live, None, true).unwrap();
        assert_eq!(
            states
                .record(&files, &request(), 1, at())
                .unwrap()
                .reason
                .as_deref(),
            Some("live_tree")
        );
        std::fs::write(files.root().root_path().join("code"), "large").unwrap();
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        let mut small = request();
        small.max_bytes = 1;
        let refused = states.record(&files, &small, 1, at()).unwrap();
        assert_eq!(refused.reason.as_deref(), Some("too_large"));
        assert!(refused.tree_ref.is_none() && refused.manifest_blob.is_none());
        states.invalidate("not_ready");
        assert_eq!(
            states
                .record(&files, &request(), 1, at())
                .unwrap()
                .reason
                .as_deref(),
            Some("not_ready")
        );
    }

    #[test]
    fn disabled_recording_does_not_repopulate_raw_cached_states_and_can_be_enabled_again() {
        let (_dir, files, index, states) = setup();
        std::fs::write(files.root().root_path().join("code"), "before").unwrap();
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        assert!(!states.cached_objects().is_empty());
        let disabled = FsRecord {
            record_days: 0,
            ..request()
        };
        assert_eq!(
            states
                .record(&files, &disabled, 1, at())
                .unwrap()
                .reason
                .as_deref(),
            Some("off")
        );
        std::fs::write(files.root().root_path().join("code"), "later").unwrap();
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        assert!(
            states.cached_objects().is_empty(),
            "background changes do not defeat recording off"
        );
        assert!(
            states
                .observe(&files, &RelPath::root(), 1, at())
                .unwrap()
                .is_none()
        );
        let enabled = request();
        assert_eq!(
            states
                .record(&files, &enabled, 1, at())
                .unwrap()
                .reason
                .as_deref(),
            Some("not_ready")
        );
        states.refresh(&files, &index, None, true).unwrap();
        let saved = states.record(&files, &request(), 1, at()).unwrap();
        assert_eq!(saved.state, RecordState::Complete);
        assert!(saved.manifest_blob.is_some());
    }
}
