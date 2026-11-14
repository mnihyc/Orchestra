//! Complete file-state baselines prepared from the index and refreshed only for watcher changes.
//!
//! Background index work owns the first complete baseline and changed-file reads. Recording a
//! turn publishes immutable metadata from that baseline; it never walks a directory or invokes
//! Git. Missing coverage, a live tree, lost notices, changed bytes and a byte-bound failure are
//! explained fallbacks. Immutable roots remain readable after a connector restart and project
//! garbage collection because every retained object belongs to the separate store.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

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
use crate::state_store::{ObjectStore, PreparedState, StateRoot};

/// The shipped maximum from the recording setting: one GiB of included code and fixtures.
pub const DEFAULT_RECORD_MAX_BYTES: i64 = 1024 * 1024 * 1024;
/// The shipped keeping period from the recording setting: a working week and its review.
pub const DEFAULT_RECORD_DAYS: i64 = 7;

/// How many changed paths wait for the baselines one by one. Past it nothing is lost: everything is looked
/// at again as a whole, which is cheaper than a list of that length.
const PENDING_LIMIT: usize = 1_000_000;
/// How long a path that could not be read waits before it is tried again, so that a file that is being
/// written is not read at every change.
const RETRY_AFTER: Duration = Duration::from_secs(1);
/// How long a pass over a whole tree that could not read something waits before it is tried again.
const SEED_RETRY_AFTER: Duration = Duration::from_secs(30);
/// How often a directory over the byte bound is measured again while it changes: it may have shrunk.
const TOO_LARGE_RECHECK: Duration = Duration::from_secs(60);
/// How many objects written without a flush of their own wait before the worker flushes them: the
/// recording that follows finds little left to flush.
const FLUSH_EVERY_OBJECTS: u64 = 1_000;

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
    prepared: Option<PreparedState>,
    ready: bool,
    reason: &'static str,
    entries: BTreeMap<String, CachedEntry>,
    ignore_fingerprint: Option<Hash>,
    /// What this scope leaves out, as the pass that made it read it; every later change is judged by
    /// these rules until an ignore file changes.
    rules: Option<std::sync::Arc<GitRules>>,
    /// The included file bytes and link text, kept up to date change by change.
    total: i64,
    /// Whether a pass over the whole scope is owed: a new scope, lost notices, changed ignore
    /// rules, a larger bound.
    stale: bool,
    /// Not before this moment is the pass that is owed tried again.
    retry_at: Option<Instant>,
    /// Whether a directory over the byte bound changed since it was measured.
    changed_since: bool,
}

impl Baseline {
    fn missing(max_bytes: i64) -> Self {
        Self {
            max_bytes,
            prepared: None,
            ready: false,
            reason: "not_ready",
            entries: BTreeMap::new(),
            ignore_fingerprint: None,
            rules: None,
            total: 0,
            stale: true,
            retry_at: None,
            changed_since: false,
        }
    }

    /// A directory whose included files are more than the bound: it keeps nothing, and is measured
    /// again after a minute if it changes.
    fn exceeded(max_bytes: i64) -> Self {
        Self {
            max_bytes,
            prepared: None,
            ready: false,
            reason: "too_large",
            entries: BTreeMap::new(),
            ignore_fingerprint: None,
            rules: None,
            total: 0,
            stale: false,
            retry_at: Some(Instant::now() + TOO_LARGE_RECHECK),
            changed_since: false,
        }
    }
}

/// Takes `relative` and everything below it out of `entries` into `taken`.
fn take_subtree(
    entries: &mut BTreeMap<String, CachedEntry>,
    relative: &str,
    taken: &mut HashMap<String, CachedEntry>,
) {
    if let Some(entry) = entries.remove(relative) {
        taken.insert(relative.to_owned(), entry);
    }
    // Everything below "a/b" sorts between "a/b/" and "a/b0" ('0' follows '/').
    let from = format!("{relative}/");
    let to = format!("{relative}0");
    let keys: Vec<String> = entries
        .range(from..to)
        .map(|(key, _)| key.clone())
        .collect();
    for key in keys {
        if let Some(entry) = entries.remove(&key) {
            taken.insert(key, entry);
        }
    }
}

/// What an entry adds to the included bytes: a file's size, a link's text.
fn weight(entry: &CachedEntry) -> i64 {
    entry.manifest.s.saturating_add(
        entry
            .manifest
            .l
            .as_ref()
            .map_or(0, |link| link.len() as i64),
    )
}

struct IndexingPass<'a>(&'a std::sync::atomic::AtomicU64);
impl Drop for IndexingPass<'_> {
    fn drop(&mut self) {
        self.0.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// What the background worker of baselines does next.
pub(crate) enum Work {
    /// Nothing is owed.
    Nothing,
    /// Build the immutable turn metadata requested by a recording.
    RecordMetadata,
    /// A pass over everything: a new directory, lost notices, changed ignore rules, a larger bound.
    Everything,
    /// Only these paths changed.
    Paths(Vec<RelPath>),
}

/// Changed paths that have been taken into the index and wait to be taken into the baselines.
#[derive(Debug, Default)]
struct Pending {
    paths: BTreeSet<RelPath>,
    /// Paths that could not be read just now, tried again at `retry_at`.
    retry: BTreeSet<RelPath>,
    retry_at: Option<Instant>,
    /// More paths arrived than are kept one by one: everything is looked at again.
    overflow: bool,
}

#[derive(Debug)]
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
        let max_rule_bytes = files
            .ignore()
            .map_or(IGNORE_FILE_MAX_BYTES, |rules| rules.max_file_bytes());
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
                if !meta.is_file() || meta.size > max_rule_bytes {
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
                    .take(max_rule_bytes.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| HostError::io("reading recording ignore rules", error))?;
                let after = platform::file_meta(&file)
                    .map_err(|error| HostError::io("checking recording ignore rules", error))?;
                if bytes.len() as u64 > max_rule_bytes || before.identity() != after.identity() {
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
    /// How many passes that bring baselines up to date are running or about to run.
    working: std::sync::atomic::AtomicUsize,
    /// The changed paths waiting for the baselines.
    pending: Mutex<Pending>,
    /// Whether a pass over everything was asked for, whatever the scopes think they owe.
    full_wanted: std::sync::atomic::AtomicBool,
    capture_wanted: std::sync::atomic::AtomicBool,
    /// How many files the pass over a whole tree has done, and how many it has to do.
    pass_done: std::sync::atomic::AtomicU64,
    pass_total: std::sync::atomic::AtomicU64,
    /// Whether the background worker of the baselines is running.
    worker: std::sync::atomic::AtomicBool,
    /// Whether a timer is set to start the worker again when work that waits for its time is due.
    timer: std::sync::atomic::AtomicBool,
    /// Whether a recording has already waited out the work in flight: the ones that follow answer
    /// at once instead of each waiting as long, until the work has moved on.
    impatient: std::sync::atomic::AtomicBool,
}

/// Background work that brings baselines up to date, for as long as it is held: a recording that finds a
/// baseline not ready waits only while there is such work.
#[derive(Debug)]
pub(crate) struct Working(std::sync::Arc<FileStates>);

impl Drop for Working {
    fn drop(&mut self) {
        self.0
            .working
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
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
            working: std::sync::atomic::AtomicUsize::new(0),
            pending: Mutex::new(Pending::default()),
            full_wanted: std::sync::atomic::AtomicBool::new(false),
            capture_wanted: std::sync::atomic::AtomicBool::new(false),
            pass_done: std::sync::atomic::AtomicU64::new(0),
            pass_total: std::sync::atomic::AtomicU64::new(0),
            worker: std::sync::atomic::AtomicBool::new(false),
            timer: std::sync::atomic::AtomicBool::new(false),
            impatient: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Notes that a pass that brings baselines up to date is about to run, until the answer is dropped.
    pub(crate) fn begin_work(self: &std::sync::Arc<Self>) -> Working {
        self.working
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Working(std::sync::Arc::clone(self))
    }

    /// Whether a pass that brings baselines up to date is running or about to run.
    pub(crate) fn work_in_flight(&self) -> bool {
        self.working.load(std::sync::atomic::Ordering::SeqCst) > 0
    }

    /// Takes the right to run the background worker; false when it is running already.
    pub(crate) fn claim_worker(&self) -> bool {
        !self.worker.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// Gives the right to run the background worker back.
    pub(crate) fn release_worker(&self) {
        self.worker
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Sets the one timer that starts the worker again; false when one is set already.
    pub(crate) fn arm_timer(&self) -> bool {
        !self.timer.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// The timer went off.
    pub(crate) fn disarm_timer(&self) {
        self.timer.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether work is owed now that the worker has not taken yet.
    pub(crate) fn has_work(&self) -> bool {
        let now = Instant::now();
        if self.full_wanted.load(std::sync::atomic::Ordering::SeqCst)
            || self
                .capture_wanted
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return true;
        }
        {
            let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.overflow
                || !pending.paths.is_empty()
                || (!pending.retry.is_empty() && pending.retry_at.is_none_or(|at| at <= now))
            {
                return true;
            }
        }
        self.baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|baseline| Self::owes_a_pass(baseline, now))
    }

    /// Notes that a recording waited out the work in flight without its baseline becoming ready.
    pub(crate) fn note_impatient(&self) {
        self.impatient
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether a recording has waited out the work in flight already.
    pub(crate) fn is_impatient(&self) -> bool {
        self.impatient.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The worker finished one piece of work: the next recording waits for the next piece afresh.
    pub(crate) fn calm(&self) {
        self.impatient
            .store(false, std::sync::atomic::Ordering::SeqCst);
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
        let mut protected = self.cached_objects();
        let prepared: Vec<_> = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter_map(|baseline| baseline.prepared.clone())
            .collect();
        let mut maps = BTreeSet::new();
        for prepared in prepared {
            self.store
                .protect_tree(&prepared.metadata.tree_ref, &mut protected)?;
            maps.insert(prepared.objects_blob);
        }
        self.store.prune_with_maps(expired, now, protected, maps)
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
        let seed = match baselines.get_mut(directory) {
            None => {
                baselines.insert(directory.clone(), Baseline::missing(max_bytes));
                true
            }
            Some(baseline) if max_bytes < baseline.max_bytes && baseline.ready => {
                if baseline.total > max_bytes {
                    *baseline = Baseline::exceeded(max_bytes);
                } else {
                    baseline.max_bytes = max_bytes;
                }
                false
            }
            Some(baseline) if baseline.reason == "off" || max_bytes != baseline.max_bytes => {
                baseline.max_bytes = max_bytes;
                baseline.ready = false;
                baseline.prepared = None;
                baseline.reason = "not_ready";
                baseline.stale = true;
                baseline.retry_at = None;
                true
            }
            _ => false,
        };
        if seed {
            self.full_wanted
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.capture_wanted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        seed
    }

    /// Current coverage without copying a scope's entries or doing any filesystem work.
    pub(crate) fn scope_recording(
        &self,
        directory: &RelPath,
    ) -> Option<orch_link_contract::events::RecordingCoverage> {
        self.baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(directory)
            .map(|baseline| orch_link_contract::events::RecordingCoverage {
                state: if baseline.ready && baseline.prepared.is_some() {
                    RecordState::Complete
                } else {
                    RecordState::Unavailable
                },
                reason: (!baseline.ready).then(|| baseline.reason.to_owned()),
            })
    }

    /// Whether the index worker still owes this scope its first/current baseline.
    /// Definitive live-tree and byte-limit fallbacks do not start extra preparation.
    pub(crate) fn needs_preparing(&self, directory: &RelPath) -> bool {
        self.baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(directory)
            .is_none_or(|baseline| {
                (!baseline.ready || baseline.prepared.is_none())
                    && matches!(baseline.reason, "not_ready" | "indexing" | "off")
            })
    }

    /// Makes coverage unknown immediately when watching loses events or ignore rules change: every
    /// scope is looked at again, in the background, by [`FileStates::take_work`].
    pub fn invalidate(&self, reason: &'static str) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for baseline in self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values_mut()
        {
            baseline.ready = false;
            baseline.prepared = None;
            baseline.reason = reason;
            baseline.stale = reason != "live_tree";
        }
    }

    /// Makes affected scopes unknown the moment a watch batch arrives, before the index has taken it
    /// in. The paths are handed to the background worker by [`FileStates::queue`] once it has.
    pub fn changed(&self, paths: &[RelPath]) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for (directory, baseline) in self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter_mut()
        {
            if !paths.iter().any(|path| {
                (path.starts_with(directory) || directory.starts_with(path))
                    && (matches!(
                        path.file_name(),
                        Some(".gitignore" | crate::ignore::IGNORE_FILE)
                    ) || !baseline.rules.as_ref().is_some_and(|rules| {
                        rules.ignored(
                            path,
                            path.strip_prefix(directory)
                                .and_then(|relative| baseline.entries.get(relative.as_str()))
                                .is_some_and(|entry| entry.manifest.t == EntryKind::Dir),
                        )
                    }))
            }) {
                continue;
            }
            if baseline.reason == "too_large" {
                // A directory over the bound stays so, and is measured again at most once a minute.
                baseline.changed_since = true;
            } else if !matches!(baseline.reason, "live_tree" | "off") {
                baseline.ready = false;
                baseline.prepared = None;
                baseline.reason = "not_ready";
            }
        }
    }

    /// Hands paths that the index has taken in to the background worker: they are brought into every
    /// baseline one by one, by what changed and not by the size of the tree.
    pub fn queue(&self, paths: &[RelPath]) {
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.overflow || pending.paths.len().saturating_add(paths.len()) > PENDING_LIMIT {
                // Nothing is lost: more changes than are kept one by one are looked at as a whole.
                pending.overflow = true;
                pending.paths.clear();
            } else {
                pending.paths.extend(paths.iter().cloned());
            }
        }
        self.changed(paths);
    }

    /// Paths that could not be read just now are tried again shortly; the baseline is not ready until
    /// they were.
    fn queue_retry(&self, paths: Vec<RelPath>) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.retry.extend(paths);
        pending.retry_at = Some(Instant::now() + RETRY_AFTER);
    }

    /// Whether a scope owes a pass over everything that is due now.
    fn owes_a_pass(baseline: &Baseline, now: Instant) -> bool {
        let owed = match baseline.reason {
            "live_tree" | "off" => false,
            "too_large" => baseline.changed_since,
            _ => baseline.stale,
        };
        owed && baseline.retry_at.is_none_or(|at| at <= now)
    }

    /// What the background worker does next: a pass over everything (a new directory, lost notices,
    /// changed ignore rules, a larger bound), the changed paths one by one, or nothing.
    pub(crate) fn take_work(&self) -> Work {
        let now = Instant::now();
        let baselines = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let everything = self
            .full_wanted
            .swap(false, std::sync::atomic::Ordering::SeqCst)
            || baselines
                .values()
                .any(|baseline| Self::owes_a_pass(baseline, now));
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if everything || pending.overflow {
            pending.overflow = false;
            pending.paths.clear();
            // A new baseline replaces its descriptor even if an earlier metadata pass finished.
            self.capture_wanted
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Work::Everything;
        }
        let mut paths = std::mem::take(&mut pending.paths);
        if pending.retry_at.is_some_and(|at| at <= now) {
            paths.append(&mut pending.retry);
            pending.retry_at = None;
        }
        if paths.is_empty() {
            if self
                .capture_wanted
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                Work::RecordMetadata
            } else {
                Work::Nothing
            }
        } else {
            Work::Paths(paths.into_iter().collect())
        }
    }

    /// Asks for a pass over everything, whatever the scopes think they owe.
    pub(crate) fn want_everything(&self) {
        self.full_wanted
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// How long until work that is waiting for its time is due: paths to try again, a directory to
    /// measure again.
    pub(crate) fn next_due(&self) -> Option<std::time::Duration> {
        let now = Instant::now();
        let mut soonest = {
            let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            pending.retry_at.filter(|_| !pending.retry.is_empty())
        };
        for baseline in self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            let waiting = match baseline.reason {
                "live_tree" | "off" => false,
                "too_large" => baseline.changed_since,
                _ => baseline.stale,
            };
            if waiting && let Some(at) = baseline.retry_at {
                soonest = Some(soonest.map_or(at, |soonest| soonest.min(at)));
            }
        }
        soonest.map(|at| at.saturating_duration_since(now))
    }

    /// How far the pass that is looking at a whole tree has come: files done and files in all.
    #[must_use]
    pub fn indexing(&self) -> Option<(u64, u64)> {
        let total = self.pass_total.load(std::sync::atomic::Ordering::SeqCst);
        (total > 0).then(|| {
            (
                self.pass_done
                    .load(std::sync::atomic::Ordering::SeqCst)
                    .min(total),
                total,
            )
        })
    }

    /// Brings baselines up to date: with `changed` only those paths, which costs what they cost and
    /// not what the tree costs; with `None` everything, reading every included file again.
    /// This runs with background index work, never in the turn's capture or observation path.
    pub fn refresh(
        &self,
        files: &Files,
        index: &Index,
        changed: Option<&[RelPath]>,
        complete: bool,
    ) -> Result<()> {
        self.refresh_with(files, index, changed, complete, changed.is_none())
    }

    /// Looks at everything again but reads only the files whose size, time or mode differ from what
    /// was kept: the pass after lost notices or a changed ignore rule.
    pub fn reconcile(&self, files: &Files, index: &Index, complete: bool) -> Result<()> {
        self.refresh_with(files, index, None, complete, false)
    }

    fn refresh_with(
        &self,
        files: &Files,
        index: &Index,
        changed: Option<&[RelPath]>,
        complete: bool,
        forced: bool,
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
        let epoch = self.epoch.load(std::sync::atomic::Ordering::SeqCst);
        match changed {
            None => self.seed(files, index, forced)?,
            Some(paths) => self.apply(files, index, paths)?,
        }
        self.settle(epoch);
        Ok(())
    }

    /// Marks the scopes that are whole and have nothing waiting as ready. Whatever arrived while a
    /// pass ran leaves them not ready: its paths are in the queue and the next pass takes them.
    fn settle(&self, epoch: u64) {
        let mut baselines = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if self.epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch
            || !pending.paths.is_empty()
            || !pending.retry.is_empty()
            || pending.overflow
        {
            return;
        }
        for baseline in baselines.values_mut() {
            if !baseline.stale && baseline.reason == "not_ready" && baseline.rules.is_some() {
                baseline.ready = true;
            }
        }
    }

    /// A pass over everything, for every scope.
    fn seed(&self, files: &Files, index: &Index, forced: bool) -> Result<()> {
        let indexed = index.entries();
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
                baseline.stale = false;
                self.baselines
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(directory, baseline);
                continue;
            }
            let result = self.prepare(files, &indexed, &directory, &previous, forced, true);
            let mut baselines = self
                .baselines
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match result {
                Ok(prepared) => {
                    baselines.insert(directory, prepared);
                }
                Err(HostError::TooLarge { .. }) => {
                    baselines.insert(directory, Baseline::exceeded(previous.max_bytes));
                }
                Err(error) => {
                    // What cannot be read is looked at again later, not at every change.
                    let baseline = baselines.entry(directory).or_insert(previous);
                    baseline.ready = false;
                    baseline.stale = true;
                    baseline.retry_at = Some(Instant::now() + SEED_RETRY_AFTER);
                    baseline.reason = match error {
                        HostError::Changed(_) => "changed_during_recording",
                        _ => "unreadable",
                    };
                }
            }
        }
        Ok(())
    }

    /// Brings the changed paths, and what lies below them, into every scope that holds them.
    fn apply(&self, files: &Files, index: &Index, changed: &[RelPath]) -> Result<()> {
        let scopes: Vec<RelPath> = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        for directory in scopes {
            let mut old: HashMap<String, CachedEntry> = HashMap::new();
            let (rules, max_bytes) = {
                let mut baselines = self
                    .baselines
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let Some(baseline) = baselines.get_mut(&directory) else {
                    continue;
                };
                if matches!(baseline.reason, "live_tree" | "off") {
                    continue;
                }
                if !changed
                    .iter()
                    .any(|path| path.starts_with(&directory) || directory.starts_with(path))
                {
                    continue;
                }
                if baseline.reason == "too_large" {
                    baseline.changed_since = true;
                    continue;
                }
                // A pass over everything is owed, the directory itself moved, or an ignore file that
                // applies here changed: what is left out may be different for any path.
                let rules_changed = changed.iter().any(|path| {
                    directory.starts_with(path)
                        || matches!(
                            path.file_name(),
                            Some(".gitignore" | crate::ignore::IGNORE_FILE)
                        ) && {
                            let base = path.parent().unwrap_or_else(RelPath::root);
                            base.starts_with(&directory) || directory.starts_with(&base)
                        }
                });
                let Some(rules) = baseline.rules.clone().filter(|_| !baseline.stale) else {
                    baseline.ready = false;
                    continue;
                };
                if rules_changed {
                    baseline.stale = true;
                    baseline.ready = false;
                    continue;
                }
                if matches!(baseline.reason, "changed_during_recording" | "unreadable") {
                    baseline.reason = "not_ready";
                }
                for path in changed {
                    let Some(relative) = path
                        .strip_prefix(&directory)
                        .filter(|relative| !relative.is_root())
                    else {
                        continue;
                    };
                    take_subtree(&mut baseline.entries, relative.as_str(), &mut old);
                }
                let removed: i64 = old.values().map(weight).sum();
                baseline.total = baseline.total.saturating_sub(removed);
                baseline.ready = false;
                (rules, baseline.max_bytes)
            };
            // What is there now, by the index: each changed path read again, what lies below it as the
            // index has it.
            let mut targets: BTreeMap<String, (RelPath, IndexEntry, bool)> = BTreeMap::new();
            for path in changed {
                if path
                    .strip_prefix(&directory)
                    .filter(|relative| !relative.is_root())
                    .is_none()
                {
                    continue;
                }
                if let Some(entry) = index.get(path) {
                    targets.insert(path.to_string(), (path.clone(), entry, true));
                }
                for (below, entry) in index.below(path) {
                    targets
                        .entry(below.to_string())
                        .or_insert_with(|| (below, entry, false));
                }
            }
            let mut fresh: Vec<(String, CachedEntry)> = Vec::new();
            let mut failure: Option<&'static str> = None;
            let mut unsettled: Vec<RelPath> = Vec::new();
            let mut added = 0i64;
            for (path, entry, forced) in targets.into_values() {
                let Some(relative) = path
                    .strip_prefix(&directory)
                    .filter(|relative| !relative.is_root())
                else {
                    continue;
                };
                if rules.ignored(&path, entry.kind == platform::Kind::Dir) {
                    continue;
                }
                let known = if forced {
                    None
                } else {
                    old.remove(relative.as_str()).filter(|known| {
                        known.index == entry
                            && (entry.kind != platform::Kind::File || known.object.is_some())
                    })
                };
                let cached = match known {
                    Some(known) => Ok(known),
                    None => self.cached_entry(files, &path, &relative, &entry),
                };
                match cached {
                    Ok(cached) => {
                        added = added.saturating_add(weight(&cached));
                        fresh.push((relative.to_string(), cached));
                    }
                    Err(HostError::Changed(_)) => {
                        failure = Some("changed_during_recording");
                        unsettled.push(path);
                    }
                    Err(_) => {
                        failure = Some("unreadable");
                        unsettled.push(path);
                    }
                }
            }
            let mut baselines = self
                .baselines
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(baseline) = baselines.get_mut(&directory) else {
                continue;
            };
            if baseline.stale {
                // A pass over everything was asked for meanwhile and sees all of this.
                continue;
            }
            baseline.total = baseline.total.saturating_add(added);
            baseline.entries.extend(fresh);
            if baseline.total > max_bytes {
                *baseline = Baseline::exceeded(max_bytes);
            } else if let Some(reason) = failure {
                baseline.ready = false;
                baseline.reason = reason;
                self.queue_retry(unsettled);
            }
        }
        if self.store.unsynced_objects() >= FLUSH_EVERY_OBJECTS {
            self.store.make_durable()?;
        }
        Ok(())
    }

    /// One included entry as it is now: a file's bytes are read and kept, without a flush of their
    /// own (see [`ObjectStore::make_durable`]).
    fn cached_entry(
        &self,
        files: &Files,
        path: &RelPath,
        relative: &RelPath,
        entry: &IndexEntry,
    ) -> Result<CachedEntry> {
        let located = files.root().locate(path, Locate::ENTRY)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::Changed("an included recorded path vanished".to_owned()))?;
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
                let mut file = located
                    .parent
                    .open_read(located.name()?)
                    .map_err(|error| HostError::io("opening included recording bytes", error))?;
                let (oid, hash) = self.store.write_blob_deferred(&mut file, entry.size)?;
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
                    Some(self.store.write_object_deferred("blob", link.as_bytes())?),
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
        Ok(CachedEntry {
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
        })
    }

    /// A pass over every included entry of `directory`. With `forced` every file is read again;
    /// without, an entry whose size, time and mode are what was kept is kept as it is. With
    /// `tolerant` a file that changes while it is read is tried again shortly instead of ending the
    /// pass.
    fn prepare(
        &self,
        files: &Files,
        indexed: &[(RelPath, IndexEntry)],
        directory: &RelPath,
        previous: &Baseline,
        forced: bool,
        tolerant: bool,
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
        let counted = u64::try_from(included.len()).unwrap_or(u64::MAX);
        self.pass_done.store(0, std::sync::atomic::Ordering::SeqCst);
        self.pass_total
            .store(counted.max(1), std::sync::atomic::Ordering::SeqCst);
        let ended = || {
            self.pass_total
                .store(0, std::sync::atomic::Ordering::SeqCst)
        };
        let mut entries = BTreeMap::new();
        let mut unsettled: Vec<RelPath> = Vec::new();
        for (path, relative, entry) in included {
            let reusable = if forced {
                None
            } else {
                previous.entries.get(relative.as_str()).filter(|known| {
                    known.index == *entry
                        && (entry.kind != platform::Kind::File || known.object.is_some())
                })
            };
            let cached = match reusable {
                Some(known) => Ok(known.clone()),
                None => self.cached_entry(files, path, &relative, entry),
            };
            match cached {
                Ok(cached) => {
                    entries.insert(relative.to_string(), cached);
                }
                Err(HostError::Changed(_)) if tolerant => unsettled.push(path.clone()),
                Err(error) => {
                    ended();
                    return Err(error);
                }
            }
            self.pass_done
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        ended();
        self.store.make_durable()?;
        let reason = if unsettled.is_empty() {
            "not_ready"
        } else {
            self.queue_retry(unsettled);
            "changed_during_recording"
        };
        let fingerprint = rules.fingerprint;
        Ok(Baseline {
            max_bytes: previous.max_bytes,
            prepared: None,
            ready: false,
            reason,
            entries,
            ignore_fingerprint: Some(fingerprint),
            rules: Some(std::sync::Arc::new(rules)),
            total,
            stale: false,
            retry_at: None,
            changed_since: false,
        })
    }

    /// Prepares immutable manifests, object maps and trees off the turn's serial key.
    /// Native changes can invalidate coverage while this works; the result is installed only if the
    /// affected scope remained ready. Unrelated scopes do not throw its work away.
    pub fn prepare_recordings(&self, files: &Files, generation: i64, now: Time) -> Result<()> {
        let _refreshing = self
            .refreshing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let scopes: Vec<_> = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, baseline)| baseline.ready && baseline.prepared.is_none())
            .map(|(directory, baseline)| (directory.clone(), baseline.clone()))
            .collect();
        for (directory, baseline) in scopes {
            self.pass_total.store(
                baseline.entries.len() as u64,
                std::sync::atomic::Ordering::SeqCst,
            );
            self.pass_done.store(0, std::sync::atomic::Ordering::SeqCst);
            let _indexing = IndexingPass(&self.pass_total);
            let entries: Vec<_> = baseline
                .entries
                .values()
                .map(|entry| {
                    self.pass_done
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    entry.manifest.clone()
                })
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
            manifest::parse(&manifest.bytes)?;
            let root = StateRoot {
                selected_paths: None,
                snapshot_id: orch_types::Id::nil(),
                request_hash: None,
                directory: directory.to_string(),
                generation,
                kind: orch_link_contract::source::RecordKind::Turn,
                tree_ref: self.store.write_tree(&entries, &objects)?,
                manifest_blob: manifest.hash,
                objects,
                files: manifest.file_count,
                bytes: baseline.total,
                created_at: now,
                expires_at: now,
                repository_head: known_head(files, &directory),
                ignore_fingerprint: baseline.ignore_fingerprint.ok_or_else(|| {
                    HostError::Unsupported("recording ignore coverage is not ready".to_owned())
                })?,
            };
            let prepared = self.store.prepare(&root, &manifest.bytes)?;
            if let Some(current) = self
                .baselines
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(&directory)
                && current.ready
            {
                current.prepared = Some(prepared);
            }
        }
        Ok(())
    }

    /// Unit fixtures explicitly prepare their background metadata before synchronous recording.
    #[cfg(test)]
    pub fn record(
        &self,
        files: &Files,
        request: &FsRecord,
        generation: i64,
        now: Time,
    ) -> Result<FsRecordResult> {
        if request.record_days > 0 {
            self.prepare_recordings(files, generation, now)?;
        }
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
        if operation.cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        let _recording = loop {
            if operation.cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }
            match self.recording.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) if request.wait_for_indexing => {
                    std::thread::sleep(RETRY_AFTER)
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err(HostError::Busy(
                        "the saved files are being read or collected".to_owned(),
                    ));
                }
            }
        };
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
            let result = self.store.recover_reply(request, now)?;
            self.store.keep_reply(request, &result)?;
            return Ok(result);
        }
        if request.kind == orch_link_contract::source::RecordKind::Restore {
            return Err(HostError::NotFound(
                "the original restore undo state is unavailable".to_owned(),
            ));
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
            baseline.prepared = None;
            baseline.entries.clear();
            baseline.total = 0;
        }
        let baseline = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&directory)
            .map(|baseline| {
                (
                    baseline.ready,
                    baseline.reason,
                    baseline.total,
                    baseline.prepared.clone(),
                )
            });
        let mut result = if request.record_days == 0 {
            unavailable("off")
        } else {
            match baseline {
                Some((true, _, bytes, _)) if bytes > request.max_bytes => unavailable("too_large"),
                Some((true, _, _, Some(prepared))) => self
                    .store
                    .keep_turn(&prepared, request, &directory, generation, now)?,
                Some((_, "off", _, _)) | None => unavailable("not_ready"),
                Some((_, reason, _, _)) => unavailable(reason),
            }
        };
        if matches!(result.reason.as_deref(), Some("not_ready" | "indexing"))
            && let Some((done, total)) = self.indexing()
        {
            result.reason = Some("indexing".to_owned());
            result.indexing = Some(orch_link_contract::events::IndexingProgress { done, total });
        }
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
            true,
            false,
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
            selected_paths: None,
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
        let entries: Vec<ManifestEntry> = {
            let baselines = self
                .baselines
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(baseline) = baselines.get(directory).filter(|baseline| baseline.ready) else {
                return Ok(None);
            };
            baseline
                .entries
                .values()
                .map(|entry| entry.manifest.clone())
                .collect()
        };
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
        indexing: None,

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
        let index = Index::new(
            IndexMode::Full,
            orch_link_contract::commands::WorkspaceLimits::default().index_entries,
        );
        let states =
            FileStates::new(ObjectStore::open(&dir.path().join("kept"), &workspace).unwrap());
        (dir, files, index, states)
    }

    fn request() -> FsRecord {
        FsRecord {
            environment_id: new_id(),
            snapshot_id: new_id(),
            directory: String::new(),
            wait_for_indexing: true,
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
        states.prepare_recordings(&files, 1, at())?;
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
        states.prepare_recordings(&files, 1, at())?;
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
        states.prepare_recordings(&files, 1, at())?;
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
        states.prepare_recordings(&files, 1, at())?;
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
    fn a_recording_never_waits_behind_an_explicit_reader_of_its_old_files() {
        let (_dir, files, index, states) = setup();
        std::fs::write(files.root().root_path().join("code"), "before").unwrap();
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        let _reader = states.recording.lock().unwrap();
        let operation = RecordOperation {
            id: orch_types::new_id(),
            until: at(),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let request = FsRecord {
            wait_for_indexing: false,
            ..request()
        };
        assert!(matches!(
            states.capture(&files, &request, 1, at(), &operation),
            Err(HostError::Busy(_))
        ));
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
        let live = Index::new(
            IndexMode::Live,
            orch_link_contract::commands::WorkspaceLimits::default().index_entries,
        );
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
    fn a_recording_waits_for_a_baseline_only_while_background_work_is_bringing_it_up_to_date() {
        let (_dir, _files, _index, states) = setup();
        let states = Arc::new(states);
        assert!(
            states.needs_preparing(&RelPath::root()),
            "a baseline nobody prepared yet is owed one"
        );
        assert!(!states.work_in_flight(), "and nothing is making it");
        let first = states.begin_work();
        let second = states.begin_work();
        assert!(states.work_in_flight());
        drop(first);
        assert!(states.work_in_flight(), "one pass is still going");
        drop(second);
        assert!(!states.work_in_flight(), "nothing is left to wait for");
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

    fn recorded_paths(
        states: &FileStates,
        files: &Files,
    ) -> std::result::Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let request = request();
        let recorded = states.record(files, &request, 1, at())?;
        assert_eq!(recorded.state, RecordState::Complete);
        let (_, tree) = states.store.load(request.snapshot_id, at())?;
        Ok(tree.entries.iter().map(|entry| entry.p.clone()).collect())
    }

    #[test]
    fn a_change_costs_what_it_changes_and_not_what_the_tree_holds()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        let root = files.root().root_path().to_owned();
        for number in 0..200 {
            std::fs::write(
                root.join(format!("file-{number}.txt")),
                format!("bytes of {number}"),
            )?;
        }
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        let read_before = states.store().deferred_written();
        assert_eq!(read_before, 200);
        std::fs::write(root.join("file-7.txt"), "different bytes entirely")?;
        let changed = [RelPath::parse("file-7.txt", PathStyle::Posix)?];
        index.refresh(
            files.root(),
            files.ignore().ok_or("missing ignore rules")?,
            &changed,
        )?;
        states.refresh(&files, &index, Some(changed.as_slice()), true)?;
        assert_eq!(
            states.store().deferred_written() - read_before,
            1,
            "one file was read, not the two hundred of the tree"
        );
        let request = request();
        let recorded = states.record(&files, &request, 1, at())?;
        assert_eq!(recorded.state, RecordState::Complete);
        assert_eq!(recorded.files, 200);
        let (saved, tree) = states.store.load(request.snapshot_id, at())?;
        let entry = tree
            .entries
            .iter()
            .find(|entry| entry.p == "file-7.txt")
            .ok_or("the changed file is not in the state")?;
        assert_eq!(
            std::fs::read(states.store.read_entry(&saved, entry)?.path())?,
            b"different bytes entirely"
        );
        Ok(())
    }

    #[test]
    fn a_new_directory_comes_in_with_what_is_in_it_and_a_removed_one_goes_with_it()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        let root = files.root().root_path().to_owned();
        std::fs::create_dir_all(root.join("keep"))?;
        std::fs::create_dir_all(root.join("gone"))?;
        std::fs::write(root.join("keep/a.txt"), "a")?;
        std::fs::write(root.join("gone/b.txt"), "b")?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        std::fs::create_dir_all(root.join("fresh/deep"))?;
        std::fs::write(root.join("fresh/x.txt"), "x")?;
        std::fs::write(root.join("fresh/deep/y.txt"), "y")?;
        std::fs::remove_dir_all(root.join("gone"))?;
        let changed = [
            RelPath::parse("fresh", PathStyle::Posix)?,
            RelPath::parse("gone", PathStyle::Posix)?,
        ];
        index.refresh(
            files.root(),
            files.ignore().ok_or("missing ignore rules")?,
            &changed,
        )?;
        states.refresh(&files, &index, Some(changed.as_slice()), true)?;
        let mut paths = recorded_paths(&states, &files)?;
        paths.sort();
        assert_eq!(
            paths,
            [
                "fresh",
                "fresh/deep",
                "fresh/deep/y.txt",
                "fresh/x.txt",
                "keep",
                "keep/a.txt"
            ]
        );
        Ok(())
    }

    #[test]
    fn paths_that_arrived_wait_for_the_worker_and_the_baseline_is_ready_when_they_are_in()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        let root = files.root().root_path().to_owned();
        std::fs::write(root.join("first.txt"), "first")?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        assert!(matches!(states.take_work(), Work::Nothing));
        std::fs::write(root.join("second.txt"), "second")?;
        let changed = [RelPath::parse("second.txt", PathStyle::Posix)?];
        index.refresh(
            files.root(),
            files.ignore().ok_or("missing ignore rules")?,
            &changed,
        )?;
        states.changed(&changed);
        states.queue(&changed);
        assert_eq!(
            states
                .record(&files, &request(), 1, at())?
                .reason
                .as_deref(),
            Some("not_ready"),
            "the new file is not in the baseline yet, so nothing partial is offered"
        );
        let Work::Paths(paths) = states.take_work() else {
            return Err("the arrived path was not handed to the worker".into());
        };
        assert_eq!(paths, changed);
        states.refresh(&files, &index, Some(paths.as_slice()), true)?;
        assert!(matches!(states.take_work(), Work::Nothing));
        let mut recorded = recorded_paths(&states, &files)?;
        recorded.sort();
        assert_eq!(recorded, ["first.txt", "second.txt"]);
        Ok(())
    }

    #[test]
    fn a_change_to_an_ignore_file_makes_the_whole_directory_owed_a_pass_and_the_pass_reads_nothing_it_kept()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (_dir, files, index, states) = setup();
        let root = files.root().root_path().to_owned();
        std::fs::write(root.join("code.txt"), "code")?;
        std::fs::write(root.join("secret.txt"), "secret")?;
        index.scan(files.root(), files.ignore().ok_or("missing ignore rules")?)?;
        states.refresh(&files, &index, None, true)?;
        let read_before = states.store().deferred_written();
        std::fs::write(root.join(".gitignore"), "secret.txt\n")?;
        let changed = [RelPath::parse(".gitignore", PathStyle::Posix)?];
        index.refresh(
            files.root(),
            files.ignore().ok_or("missing ignore rules")?,
            &changed,
        )?;
        states.refresh(&files, &index, Some(changed.as_slice()), true)?;
        assert!(
            matches!(states.take_work(), Work::Everything),
            "what is left out may be different for any path"
        );
        states.reconcile(&files, &index, true)?;
        assert_eq!(
            states.store().deferred_written() - read_before,
            1,
            "only the new ignore file was read; code.txt kept its copy"
        );
        let mut paths = recorded_paths(&states, &files)?;
        paths.sort();
        assert_eq!(paths, [".gitignore", "code.txt"]);
        Ok(())
    }
}
