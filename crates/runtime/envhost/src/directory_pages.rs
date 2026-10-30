//! Immutable directory metadata, sorted with bounded memory into anonymous temporary files.
//!
//! There is one cache/build permit per host. The one-GiB ledger includes every intermediate
//! merge copy and retained file; pinned metadata is not evicted, and eviction invalidates extracted
//! page proofs. A chunk has at most 5,000 rows
//! and two MiB of encoded rows, plus those bounded rows' name/struct storage. Binary carry
//! merging retains at most logarithmically many anonymous file handles, not one per chunk.
//! Files close before their quota is released and the OS removes them when the run dies.

use crate::error::{HostError, Result};
use crate::files::{DirectoryLocation, DirectoryVersion, Files};
use crate::paths::RelPath;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_api_contract::workspace::{DirEntry, DirEntryKind};
use orch_link_contract::commands::FS_LIST_MAX_ENTRIES;
use orch_types::{Clock, Hash, Id, Time, new_id};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering as Compared;
use std::collections::{BinaryHeap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

mod captures;
pub(crate) use captures::Coverage;
mod operations;
pub(crate) use operations::{Attachment, Operation};

/// Half of the existing four-MiB frame leaves room for result/event envelopes.
pub(crate) const PAGE_BYTES: usize = 2 * 1024 * 1024;
// One existing client-frame allowance covers paths, cursors and result/status envelopes.
const ENTRY_BYTES: usize = PAGE_BYTES - orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES;
const IDLE: Duration = Duration::from_secs(5 * 60);
const SLOTS: usize = crate::environment::DIR_WATCHES_MAX;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

fn stale() -> HostError {
    HostError::Stale { current: None }
}

/// The actual attachment/path and the API's generation are part of the snapshot selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) environment: Id,
    pub(crate) attachment: Id,
    pub(crate) generation: Option<i64>,
    pub(crate) mount: Option<String>,
    pub(crate) path: String,
}

pub(crate) struct Page {
    pub(crate) entries: Vec<DirEntry>,
    pub(crate) revision: Hash,
    pub(crate) next_cursor: Option<String>,
    pub(crate) as_of: Time,
    pub(crate) observed_from: Time,
    pub(crate) expires_at: Time,
    observation: Arc<Observation>,
}

impl Page {
    pub(crate) fn validate(&self) -> Result<()> {
        self.observation.validate()
    }

    pub(crate) fn proof(&self) -> Arc<Observation> {
        Arc::clone(&self.observation)
    }
}

#[derive(Default)]
pub(crate) struct Observation {
    invalid: AtomicBool,
    published: AtomicBool,
    location: Option<DirectoryLocation>,
}

impl Observation {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.invalid.load(Ordering::Acquire) {
            Err(stale())
        } else {
            Ok(())
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    snapshot: Id,
    offset: u64,
}

struct Ledger {
    used: AtomicU64,
    maximum: u64,
}

/// Only metadata is written here: no content file, recording, journal or native session.
struct Stored {
    file: Option<File>,
    bytes: u64,
    ledger: Arc<Ledger>,
}

impl Stored {
    fn new(cache: &DirectoryPages) -> Result<Self> {
        let file = tempfile::tempfile_in(&cache.directory)
            .map_err(|error| HostError::io("making anonymous directory metadata", error))?;
        Ok(Self {
            file: Some(file),
            bytes: 0,
            ledger: Arc::clone(&cache.ledger),
        })
    }

    fn file(&mut self) -> Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| HostError::Internal("the directory metadata file was closed".to_owned()))
    }

    fn write(&mut self, cache: &DirectoryPages, bytes: &[u8]) -> Result<()> {
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        cache.reserve(size)?;
        // Charge before the write, including a failed partial write until this file closes.
        self.bytes = self.bytes.checked_add(size).ok_or_else(|| {
            HostError::Internal("directory metadata accounting overflowed".to_owned())
        })?;
        self.file()?
            .write_all(bytes)
            .map_err(|error| HostError::io("writing directory metadata", error))
    }
}

impl Drop for Stored {
    fn drop(&mut self) {
        drop(self.file.take());
        self.ledger.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct Snapshot {
    id: Id,
    binding: Binding,
    version: DirectoryVersion,
    metadata: Mutex<Stored>,
    revision: Hash,
    observed_from: Time,
    as_of: Time,
    last_use: AtomicI64,
    observation: Arc<Observation>,
}

/// One host-wide pool, rather than a separate quota for every directory/watch/subscriber.
pub(crate) struct DirectoryPages {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
    ledger: Arc<Ledger>,
    snapshots: Mutex<HashMap<Id, Arc<Snapshot>>>,
    captures: Mutex<Vec<Arc<captures::Building>>>,
    build: Arc<Semaphore>,
    operations: Arc<operations::Owners>,
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) testing: Arc<observations_tests::Hooks>,
}

fn check(cancel: &CancellationToken, deadline: Instant) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(HostError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(HostError::Timeout(
            "the directory metadata read exceeded its command budget".to_owned(),
        ));
    }
    Ok(())
}

impl DirectoryPages {
    pub(crate) fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self {
            directory,
            clock,
            ledger: Arc::new(Ledger {
                used: AtomicU64::new(0),
                maximum: crate::manifest::MANIFEST_MAX_EXPANDED_BYTES,
            }),
            snapshots: Mutex::new(HashMap::new()),
            captures: Mutex::new(Vec::new()),
            build: Arc::new(Semaphore::new(1)),
            operations: operations::Owners::new(),
            #[cfg(all(test, target_os = "linux"))]
            testing: Arc::default(),
        }
    }

    fn expired(&self, snapshot: &Snapshot) -> bool {
        let idle = self
            .clock
            .now()
            .timestamp_millis()
            .saturating_sub(snapshot.last_use.load(Ordering::Acquire));
        idle >= i64::try_from(IDLE.as_millis()).unwrap_or(i64::MAX)
    }

    fn evict_one(&self) -> bool {
        let mut snapshots = lock(&self.snapshots);
        let candidate = snapshots
            .iter()
            .filter(|(_, value)| Arc::strong_count(value) == 1)
            .min_by_key(|(_, value)| {
                (
                    u8::from(!self.expired(value)),
                    value.last_use.load(Ordering::Acquire),
                )
            })
            .map(|(id, _)| *id);
        candidate.is_some_and(|id| {
            // Returned pages retain this proof after releasing their metadata pin. Invalidate
            // before making it unreachable to native admission, even without a known event yet.
            if let Some(snapshot) = snapshots.get(&id) {
                snapshot.observation.invalid.store(true, Ordering::Release);
            }
            snapshots.remove(&id).is_some()
        })
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn evict_observation_for_test(&self) -> bool {
        self.evict_one()
    }

    fn reserve(&self, bytes: u64) -> Result<()> {
        loop {
            if self
                .ledger
                .used
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    used.checked_add(bytes)
                        .filter(|next| *next <= self.ledger.maximum)
                })
                .is_ok()
            {
                return Ok(());
            }
            if !self.evict_one() {
                return Err(HostError::Busy(
                    "the directory metadata cache is full while metadata is in use".to_owned(),
                ));
            }
        }
    }

    pub(crate) fn forget(&self, environment: Id) {
        self.forget_selected(environment, None);
    }

    pub(crate) fn forget_attachment(&self, environment: Id, attachment: Id) {
        self.forget_selected(environment, Some(attachment));
    }

    fn forget_selected(&self, environment: Id, attachment: Option<Id>) {
        // Active page/build Arcs keep their anonymous files and quota until work really ends.
        let captures = lock(&self.captures);
        for capture in captures.iter().filter(|capture| {
            capture.binding.environment == environment
                && attachment.is_none_or(|attachment| capture.binding.attachment == attachment)
        }) {
            capture.cancel();
        }
        lock(&self.snapshots).retain(|_, snapshot| {
            if snapshot.binding.environment == environment
                && attachment.is_none_or(|attachment| snapshot.binding.attachment == attachment)
            {
                snapshot.observation.invalid.store(true, Ordering::Release);
                false
            } else {
                true
            }
        });
    }

    /// Invalidates known changes without enumerating siblings or waiting for batched presentation.
    pub(crate) fn invalidate(&self, environment: Id, attachment: Id, paths: &[RelPath], all: bool) {
        let captures = lock(&self.captures);
        for capture in captures.iter().filter(|capture| {
            capture.binding.environment == environment
                && capture.binding.attachment == attachment
                && affected(&capture.binding, &capture.location.path, paths, all)
        }) {
            capture.invalidate();
        }
        lock(&self.snapshots).retain(|_, snapshot| {
            if snapshot.binding.environment == environment
                && snapshot.binding.attachment == attachment
                && affected(
                    &snapshot.binding,
                    &snapshot.version.location.path,
                    paths,
                    all,
                )
            {
                snapshot.observation.invalid.store(true, Ordering::Release);
                false
            } else {
                true
            }
        });
        #[cfg(all(test, target_os = "linux"))]
        self.testing.invalidate();
    }

    #[cfg(all(test, target_os = "linux"))]
    async fn read(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        path: RelPath,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Page> {
        self.read_covered(binding, files, path, cursor, limit, None)
            .await
    }

    #[cfg(all(test, target_os = "linux"))]
    async fn read_covered(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        path: RelPath,
        cursor: Option<&str>,
        limit: u32,
        coverage: Option<Coverage>,
    ) -> Result<Page> {
        let attachment = Attachment::new();
        let mut operation = attachment.begin(self, None).await?;
        operation.cover(coverage);
        self.read_in(&operation, binding, files, path, cursor, limit)
            .await
    }

    pub(crate) async fn read_in(
        self: &Arc<Self>,
        operation: &Operation,
        binding: Binding,
        files: Files,
        path: RelPath,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Page> {
        operation.check()?;
        if !(1..=FS_LIST_MAX_ENTRIES).contains(&limit) {
            return Err(HostError::Unsupported(
                "a directory page needs between one and five thousand entries".to_owned(),
            ));
        }
        let cursor = cursor
            .map(|text| {
                if text.len() > crate::paths::PATH_MAX_BYTES {
                    return Err(stale());
                }
                let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| stale())?;
                serde_json::from_slice::<Cursor>(&bytes).map_err(|_| stale())
            })
            .transpose()?;
        let deadline = operation.deadline();
        let cancel = operation.cancellation();
        let coverage = operation.coverage();
        self.reap().await?;
        let mut reader = None;
        let snapshot = if let Some(cursor) = &cursor {
            let snapshot = lock(&self.snapshots)
                .get(&cursor.snapshot)
                .cloned()
                .ok_or_else(stale)?;
            if snapshot.binding != binding {
                return Err(stale());
            }
            if self.expired(&snapshot) {
                snapshot.observation.invalid.store(true, Ordering::Release);
                lock(&self.snapshots).remove(&cursor.snapshot);
                return Err(stale());
            }
            snapshot
        } else {
            let location = {
                let (source, directory) = (files.clone(), path.clone());
                #[cfg(all(test, target_os = "linux"))]
                let testing = Arc::clone(&self.testing);
                operation
                    .blocking(move || {
                        #[cfg(all(test, target_os = "linux"))]
                        let _location = testing.before_location()?;
                        source.directory_location(&directory)
                    })
                    .await?
            };
            let held = operation.admit(|| {
                let mut builds = lock(&self.captures);
                for build in builds.iter() {
                    if let Some(reader) = build.reader(&binding, &location, coverage.is_some())? {
                        return Ok(reader);
                    }
                }
                if builds.len() >= SLOTS {
                    return Err(HostError::Busy(
                        "the directory metadata cache has no unused capture owner".to_owned(),
                    ));
                }
                loop {
                    let pending = builds
                        .iter()
                        .filter(|build| !build.observation.published.load(Ordering::Acquire))
                        .count();
                    if lock(&self.snapshots).len() + pending < SLOTS {
                        break;
                    }
                    if !self.evict_one() {
                        return Err(HostError::Busy(
                            "the directory metadata cache has no unused observation slot"
                                .to_owned(),
                        ));
                    }
                }
                let (build, held) = captures::Building::new(
                    Arc::clone(self),
                    binding,
                    files.clone(),
                    path.clone(),
                    location,
                    coverage,
                    deadline,
                );
                builds.push(build);
                Ok(held)
            })?;
            #[cfg(all(test, target_os = "linux"))]
            self.testing.reader();
            let snapshot = tokio::select! {
                biased;
                () = cancel.cancelled() => { operation.check()?; return Err(HostError::Cancelled); }
                snapshot = held.snapshot(deadline) => snapshot?,
            };
            reader = Some(held);
            snapshot
        };
        let cache = Arc::clone(self);
        let continuing = cursor.is_some();
        let offset = cursor.map_or(0, |cursor| cursor.offset);
        let token = cancel.clone();
        let page = operation
            .blocking(move || {
                #[cfg(all(test, target_os = "linux"))]
                let _page = cache.testing.before_page()?;
                check(&token, deadline)?;
                let location = files.directory_location(&path).map_err(|error| {
                    if continuing
                        && matches!(&error, HostError::NotFound(_) | HostError::Ignored(_))
                    {
                        stale()
                    } else {
                        error
                    }
                })?;
                if location != snapshot.version.location {
                    return Err(stale());
                }
                cache.page(&snapshot, offset, limit, &token, deadline)
            })
            .await;
        drop(reader);
        self.reap().await?;
        page
    }

    fn capture(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        path: RelPath,
        cancel: &CancellationToken,
        deadline: Instant,
        state: &captures::State,
    ) -> Result<Arc<Snapshot>> {
        let observation = &state.observation;
        check(cancel, deadline)?;
        let observed_from = self.clock.time();
        #[cfg(all(test, target_os = "linux"))]
        self.testing.before_capture()?;
        check(cancel, deadline)?;
        std::fs::create_dir_all(&self.directory)
            .map_err(|error| HostError::io("preparing temporary directory metadata", error))?;
        let mut builder = Builder {
            cache: self,
            rows: Vec::new(),
            bytes: 0,
            levels: Vec::new(),
        };
        let version = files.visit_directory(&path, &mut |entry| {
            check(cancel, deadline)?;
            if observation.invalid.load(Ordering::Acquire) {
                return Err(HostError::Changed(
                    "the directory changed during observation".to_owned(),
                ));
            }
            let row = Record::new(entry)?;
            if !builder.rows.is_empty()
                && (builder.rows.len() >= FS_LIST_MAX_ENTRIES as usize
                    || builder.bytes.saturating_add(row.bytes.len() + 1) > PAGE_BYTES)
            {
                builder.flush_order(Order::Name, cancel, deadline)?;
            }
            builder.bytes += row.bytes.len() + 1;
            builder.rows.push(row);
            Ok(())
        })?;
        if observation
            .location
            .as_ref()
            .is_some_and(|expected| *expected != version.location)
        {
            return Err(HostError::Changed(
                "the directory identity changed before its observation began".to_owned(),
            ));
        }
        builder.flush_order(Order::Name, cancel, deadline)?;
        let names = builder.finish_order(Order::Name, cancel, deadline)?;
        // Name-only order makes duplicate/conflicting exact native names adjacent even if their
        // observed kinds differ. Verify that stream, then externally sort the same rows for display.
        // Both passes use the same bounded chunks and inclusive intermediate/retained byte ledger.
        let mut metadata = verified_display(self, names, cancel, deadline)?;
        let mut digest = Hash::hasher();
        metadata
            .file()?
            .seek(SeekFrom::Start(0))
            .map_err(|error| HostError::io("reading directory metadata revision", error))?;
        let mut buffer = [0; 8192];
        loop {
            check(cancel, deadline)?;
            let read = metadata
                .file()?
                .read(&mut buffer)
                .map_err(|error| HostError::io("reading directory metadata revision", error))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        let revision = Hash::finish(&digest);
        if files.directory_location(&path)? != version.location {
            return Err(HostError::Changed(
                "the directory changed while its metadata was sorted".to_owned(),
            ));
        }
        check(cancel, deadline)?;
        // Serialize publication with native admission, including changes queued before the slow
        // index/Git path. Directory metadata equality is not a native membership generation.
        #[cfg(all(test, target_os = "linux"))]
        self.testing.before_publication()?;
        check(cancel, deadline)?;
        #[cfg(all(test, target_os = "linux"))]
        self.testing.before_publication_lock()?;
        let _captures = lock(&self.captures);
        let readers = lock(&state.readers);
        // Reader reservation and the actual last-reader close use this same lock. A pre-lock
        // token check cannot prevent publication after the last owner left while we waited.
        check(cancel, deadline)?;
        if readers.closed || readers.count == 0 {
            return Err(HostError::Cancelled);
        }
        if observation.invalid.load(Ordering::Acquire) {
            return Err(HostError::Changed(
                "the directory changed before its observation was published".to_owned(),
            ));
        }
        let now = self.clock.now();
        if Time::new(now) < observed_from {
            return Err(HostError::Changed(
                "the clock moved backwards during the directory observation".to_owned(),
            ));
        }
        let snapshot = Arc::new(Snapshot {
            id: new_id(),
            binding,
            version,
            metadata: Mutex::new(metadata),
            revision,
            observed_from,
            as_of: Time::new(now),
            last_use: AtomicI64::new(now.timestamp_millis()),
            observation: Arc::clone(observation),
        });
        // Admission reserved one of the same256 slots; publication transfers that slot to metadata.
        lock(&self.snapshots).insert(snapshot.id, Arc::clone(&snapshot));
        observation.published.store(true, Ordering::Release);
        Ok(snapshot)
    }

    fn page(
        &self,
        snapshot: &Snapshot,
        offset: u64,
        limit: u32,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Page> {
        if snapshot.observation.invalid.load(Ordering::Acquire) {
            return Err(stale());
        }
        let mut metadata = lock(&snapshot.metadata);
        if offset > metadata.bytes {
            return Err(stale());
        }
        let file = metadata.file()?;
        file.seek(SeekFrom::Start(offset)).map_err(|_| stale())?;
        let mut reader = BufReader::new(file);
        let mut entries = Vec::new();
        let mut bytes = 2; // The enclosing JSON array; commas are charged per additional row.
        let mut next = None;
        loop {
            check(cancel, deadline)?;
            let position = reader.stream_position().map_err(|_| stale())?;
            let Some(row) = next_record(&mut reader)? else {
                break;
            };
            let cost = row.bytes.len() + usize::from(!entries.is_empty());
            if entries.len() >= limit as usize || bytes + cost > ENTRY_BYTES {
                if entries.is_empty() {
                    return Err(too_large(row.bytes.len()));
                }
                next = Some(position);
                break;
            }
            entries.push(serde_json::from_slice(&row.bytes)?);
            bytes += cost;
        }
        check(cancel, deadline)?;
        if snapshot.observation.invalid.load(Ordering::Acquire) {
            return Err(stale());
        }
        let now = self.clock.now();
        snapshot
            .last_use
            .store(now.timestamp_millis(), Ordering::Release);
        let next_cursor = next
            .map(|offset| {
                serde_json::to_vec(&Cursor {
                    snapshot: snapshot.id,
                    offset,
                })
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
            })
            .transpose()?;
        Ok(Page {
            entries,
            revision: snapshot.revision,
            next_cursor,
            as_of: snapshot.as_of,
            observed_from: snapshot.observed_from,
            expires_at: Time::new(now + chrono::Duration::minutes(5)),
            observation: Arc::clone(&snapshot.observation),
        })
    }
}

fn affected(binding: &Binding, directory: &RelPath, paths: &[RelPath], all: bool) -> bool {
    all || (binding.mount.is_none()
        && paths.iter().any(|path| {
            path == directory
                || path.parent().as_ref() == Some(directory)
                || directory
                    .as_str()
                    .strip_prefix(path.as_str())
                    .is_some_and(|tail| tail.starts_with('/'))
        }))
}

fn too_large(size: usize) -> HostError {
    HostError::TooLarge {
        what: "one directory metadata row".to_owned(),
        size: i64::try_from(size).unwrap_or(i64::MAX),
        limit: i64::try_from(ENTRY_BYTES - 2).unwrap_or(i64::MAX),
    }
}

struct Record {
    directory: bool,
    name: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Order {
    Display,
    Name,
}

impl Order {
    fn compare(self, a: &Record, b: &Record) -> Compared {
        match self {
            Self::Display => a.compare(b),
            Self::Name => a.name.cmp(&b.name),
        }
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.compare(other) == Compared::Equal
    }
}
impl Eq for Record {}
impl Ord for Record {
    fn cmp(&self, other: &Self) -> Compared {
        self.compare(other)
    }
}
impl PartialOrd for Record {
    fn partial_cmp(&self, other: &Self) -> Option<Compared> {
        Some(self.cmp(other))
    }
}

/// A small complete watch can be diffed without keeping all sibling names in memory.
/// A partial watch is invalidated instead; this prefix is never called a complete tree.
pub(crate) fn preview(files: &Files, path: &RelPath) -> Result<crate::files::Listing> {
    let mut rows = BinaryHeap::new();
    let mut bytes = 1; // One plus each row's bytes/comma equals the encoded nonempty array.
    let mut frontier: Option<Record> = None;
    let mut truncated = false;
    files.visit_directory(path, &mut |entry| {
        let row = Record::new(entry)?;
        if frontier.as_ref().is_some_and(|frontier| row >= *frontier) {
            truncated = true;
            return Ok(());
        }
        bytes += row.bytes.len() + 1;
        rows.push(row);
        while rows.len() > FS_LIST_MAX_ENTRIES as usize || bytes > ENTRY_BYTES {
            if let Some(removed) = rows.pop() {
                bytes -= removed.bytes.len() + 1;
                truncated = true;
                if frontier.as_ref().is_none_or(|frontier| removed < *frontier) {
                    frontier = Some(Record {
                        directory: removed.directory,
                        name: removed.name,
                        bytes: Vec::new(),
                    });
                }
            }
        }
        Ok(())
    })?;
    // A complete small preview must not hide a conflicting exact name in its map projection.
    // These are only the already bounded retained rows; wide watches publish invalidation instead.
    let mut rows = rows.into_vec();
    rows.sort_by(|a, b| Order::Name.compare(a, b));
    let mut previous = None;
    for row in &rows {
        reject_duplicate(Order::Name, &mut previous, row)?;
    }
    rows.sort_by(Record::compare);
    let entries = rows
        .into_iter()
        .map(|row| serde_json::from_slice(&row.bytes).map_err(HostError::from))
        .collect::<Result<Vec<_>>>()?;
    Ok(crate::files::Listing { entries, truncated })
}

impl Record {
    fn new(entry: DirEntry) -> Result<Self> {
        let bytes = serde_json::to_vec(&entry)?;
        if bytes.len() + 2 > ENTRY_BYTES {
            return Err(too_large(bytes.len()));
        }
        Ok(Self {
            directory: entry.kind == DirEntryKind::Dir,
            name: entry.name,
            bytes,
        })
    }

    fn compare(&self, other: &Self) -> Compared {
        (!self.directory)
            .cmp(&(!other.directory))
            .then_with(|| self.name.to_lowercase().cmp(&other.name.to_lowercase()))
            .then_with(|| self.name.cmp(&other.name))
    }

    fn write(&self, cache: &DirectoryPages, file: &mut Stored) -> Result<()> {
        file.write(cache, &self.bytes)?;
        file.write(cache, b"\n")
    }
}

fn next_record(reader: &mut impl BufRead) -> Result<Option<Record>> {
    let mut bytes = Vec::new();
    let read = reader
        .take(u64::try_from(PAGE_BYTES + 1).unwrap_or(u64::MAX))
        .read_until(b'\n', &mut bytes)
        .map_err(|error| HostError::io("reading directory metadata", error))?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.last() != Some(&b'\n') || bytes.len() > PAGE_BYTES {
        return Err(stale());
    }
    let _ = bytes.pop();
    let entry: DirEntry = serde_json::from_slice(&bytes).map_err(|_| stale())?;
    Ok(Some(Record {
        directory: entry.kind == DirEntryKind::Dir,
        name: entry.name,
        bytes,
    }))
}

struct Builder<'a> {
    cache: &'a DirectoryPages,
    rows: Vec<Record>,
    bytes: usize,
    levels: Vec<Option<Stored>>,
}

impl Builder<'_> {
    fn flush(&mut self, cancel: &CancellationToken, deadline: Instant) -> Result<()> {
        self.flush_order(Order::Display, cancel, deadline)
    }

    fn flush_order(
        &mut self,
        order: Order,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        check(cancel, deadline)?;
        self.rows.sort_by(|a, b| order.compare(a, b));
        let mut run = Stored::new(self.cache)?;
        let mut previous = None;
        for row in &self.rows {
            check(cancel, deadline)?;
            reject_duplicate(order, &mut previous, row)?;
            row.write(self.cache, &mut run)?;
        }
        self.rows.clear();
        self.bytes = 0;
        let mut level = 0;
        loop {
            if level == self.levels.len() {
                self.levels.push(None);
            }
            match self.levels[level].take() {
                None => {
                    self.levels[level] = Some(run);
                    return Ok(());
                }
                Some(previous) => {
                    run = merge(self.cache, previous, run, cancel, deadline, order)?;
                    level += 1;
                }
            }
        }
    }

    fn finish(self, cancel: &CancellationToken, deadline: Instant) -> Result<Stored> {
        self.finish_order(Order::Display, cancel, deadline)
    }

    fn finish_order(
        self,
        order: Order,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Stored> {
        let mut result = None;
        for run in self.levels.into_iter().flatten() {
            result = Some(match result {
                None => run,
                Some(previous) => merge(self.cache, previous, run, cancel, deadline, order)?,
            });
        }
        result.map_or_else(|| Stored::new(self.cache), Ok)
    }
}

fn merge(
    cache: &DirectoryPages,
    mut left: Stored,
    mut right: Stored,
    cancel: &CancellationToken,
    deadline: Instant,
    order: Order,
) -> Result<Stored> {
    let mut output = Stored::new(cache)?;
    left.file()?
        .seek(SeekFrom::Start(0))
        .map_err(|error| HostError::io("merging directory metadata", error))?;
    right
        .file()?
        .seek(SeekFrom::Start(0))
        .map_err(|error| HostError::io("merging directory metadata", error))?;
    {
        let mut a = BufReader::new(left.file()?);
        let mut b = BufReader::new(right.file()?);
        let mut first = next_record(&mut a)?;
        let mut second = next_record(&mut b)?;
        let mut previous = None;
        while first.is_some() || second.is_some() {
            check(cancel, deadline)?;
            let from_left = match (&first, &second) {
                (Some(a), Some(b)) => order.compare(a, b) != Compared::Greater,
                (Some(_), None) => true,
                _ => false,
            };
            if from_left {
                if let Some(row) = first.take() {
                    reject_duplicate(order, &mut previous, &row)?;
                    row.write(cache, &mut output)?;
                }
                first = next_record(&mut a)?;
            } else {
                if let Some(row) = second.take() {
                    reject_duplicate(order, &mut previous, &row)?;
                    row.write(cache, &mut output)?;
                }
                second = next_record(&mut b)?;
            }
        }
    }
    Ok(output)
}

fn reject_duplicate(order: Order, previous: &mut Option<String>, row: &Record) -> Result<()> {
    if matches!(order, Order::Name) {
        if previous.as_deref() == Some(row.name.as_str()) {
            return Err(HostError::Changed(
                "the directory returned a duplicate or conflicting name during observation"
                    .to_owned(),
            ));
        }
        *previous = Some(row.name.clone());
    }
    Ok(())
}

fn verified_display(
    cache: &DirectoryPages,
    mut names: Stored,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<Stored> {
    names
        .file()?
        .seek(SeekFrom::Start(0))
        .map_err(|error| HostError::io("verifying directory names", error))?;
    let mut builder = Builder {
        cache,
        rows: Vec::new(),
        bytes: 0,
        levels: Vec::new(),
    };
    {
        let mut stream = BufReader::new(names.file()?);
        let mut previous = None;
        while let Some(row) = next_record(&mut stream)? {
            check(cancel, deadline)?;
            reject_duplicate(Order::Name, &mut previous, &row)?;
            if !builder.rows.is_empty()
                && (builder.rows.len() >= FS_LIST_MAX_ENTRIES as usize
                    || builder.bytes.saturating_add(row.bytes.len() + 1) > PAGE_BYTES)
            {
                builder.flush(cancel, deadline)?;
            }
            builder.bytes += row.bytes.len() + 1;
            builder.rows.push(row);
        }
    }
    drop(names);
    builder.flush(cancel, deadline)?;
    builder.finish(cancel, deadline)
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod observations_tests;

#[cfg(all(test, target_os = "linux"))]
mod lifecycle_tests;
