//! Selection, hashing, deduplicated upload and partial snapshots.
//!
//! A snapshot of a selection goes through the phases `scan`, `hash`, `upload` and `manifest`. The
//! scan lists what the selection names, with the hashes already known for the very version of each
//! file; the hash phase hashes the rest; the upload asks the store which of the hashes it lacks,
//! a thousand at a time, and uploads exactly those, each content once however many files hold it;
//! the manifest phase uploads the list itself. Every phase can be run again after a cut: what is
//! done is not done twice.
//!
//! A tree does not stand still while it is read. A file that changes while it is hashed is hashed
//! again once, with its new size and time written into the list so that the list is consistent;
//! a file that vanishes, cannot be read, keeps changing or is larger than the store takes is
//! listed without a hash and counted as missing, which makes the snapshot partial instead of
//! failing it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use orch_api_contract::workspace::{JobPhase, JobProgress, PhaseState};
use orch_link_contract::commands::{FsSnapshotResult, Selection};
use orch_link_contract::content::MAX_MISSING_HASHES;
use orch_types::{Clock, Hash};

use crate::blobs::{BlobSink, FileReader, MemoryReader};
use crate::error::{HostError, Result, blocking};
use crate::files::Files;
use crate::manifest::{self, EntryKind, ManifestEntry};
use crate::paths::RelPath;

/// How many files are hashed at once. Hashing reads a tree that people and agents are working in:
/// four at a time keeps the disk busy without taking it from them.
pub const HASH_PARALLEL: usize = 4;

/// How often progress is sent at most: the system keeps one report a second and a faster one is
/// only dropped.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);

/// The phases of a snapshot, each with its share of the bar. Hashing and uploading are where the
/// time goes; the others are short, and the last is the system's own.
const PHASES: [(&str, u32); 5] = [
    ("scan", 5),
    ("hash", 30),
    ("upload", 55),
    ("manifest", 5),
    ("finalize", 5),
];

/// Where the phases stand in [`PHASES`].
const PHASE_SCAN: usize = 0;
const PHASE_HASH: usize = 1;
const PHASE_UPLOAD: usize = 2;
const PHASE_MANIFEST: usize = 3;

/// Where progress goes.
pub trait Progress: Send + Sync {
    /// A new state of the job. Calls come at most once a second, and the last one always.
    fn report(&self, progress: &JobProgress);
    /// Bytes that a transfer moved within one large file, without waiting for that file to finish.
    fn transfer(&self, _bytes: i64) {}
    /// A streamed size count, before any file content is copied.
    fn counting(&self, _files: i64, _bytes: i64, _directories: i64) {}
}

tokio::task_local! { static TRANSFER_PROGRESS: std::sync::Arc<dyn Progress>; }
thread_local! {
    static BLOCKING_PROGRESS: std::cell::RefCell<Option<std::sync::Arc<dyn Progress>>> = const { std::cell::RefCell::new(None) };
}

/// The command's reporter, including the blocking filesystem workers that it owns.
pub(crate) fn current_progress() -> Option<std::sync::Arc<dyn Progress>> {
    TRANSFER_PROGRESS
        .try_with(std::sync::Arc::clone)
        .ok()
        .or_else(|| BLOCKING_PROGRESS.with(|progress| progress.borrow().clone()))
}

/// Restores a blocking thread's previous reporter before it handles unrelated work.
pub(crate) struct BlockingProgress(Option<std::sync::Arc<dyn Progress>>);
impl BlockingProgress {
    pub(crate) fn enter(progress: Option<std::sync::Arc<dyn Progress>>) -> Self {
        Self(BLOCKING_PROGRESS.with(|held| held.replace(progress)))
    }
}
impl Drop for BlockingProgress {
    fn drop(&mut self) {
        BLOCKING_PROGRESS.with(|held| held.replace(self.0.take()));
    }
}

/// A restore's local staging copy remains interruptible within one bounded filesystem chunk.
pub(crate) fn copy_counted(
    input: &mut impl std::io::Read,
    output: &mut impl std::io::Write,
) -> crate::error::Result<u64> {
    const COPY_BYTES: usize = 64 * 1024;
    let progress = current_progress();
    let mut buffer = [0u8; COPY_BYTES];
    let mut total = 0u64;
    loop {
        crate::error::check_cancelled()?;
        let read = input
            .read(&mut buffer)
            .map_err(|error| crate::error::HostError::io("reading restored bytes", error))?;
        if read == 0 {
            return Ok(total);
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| crate::error::HostError::io("staging restored bytes", error))?;
        total = total.saturating_add(read as u64);
        if let Some(progress) = &progress {
            progress.transfer(read as i64);
        }
    }
}

/// Reports a command's large-file transfers through the same progress sink as its phases.
pub async fn with_transfer_progress<F: std::future::Future>(
    progress: std::sync::Arc<dyn Progress>,
    work: F,
) -> F::Output {
    TRANSFER_PROGRESS.scope(progress, work).await
}

/// One file's highest transferred offset, preventing retries from adding its bytes twice.
pub(crate) struct TransferWatch {
    progress: std::sync::Arc<dyn Progress>,
    offset: std::sync::atomic::AtomicI64,
}
impl std::fmt::Debug for TransferWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferWatch").finish_non_exhaustive()
    }
}
impl TransferWatch {
    /// Captures the reporting scope when a transfer's reader or writer is created.
    pub(crate) fn current() -> Option<std::sync::Arc<Self>> {
        current_progress().map(|progress| {
            std::sync::Arc::new(Self {
                progress,
                offset: std::sync::atomic::AtomicI64::new(0),
            })
        })
    }
    /// Counts each byte range once across retries and resumed pieces.
    pub(crate) fn advance(&self, offset: i64) {
        let previous = self
            .offset
            .fetch_max(offset, std::sync::atomic::Ordering::SeqCst);
        if offset > previous {
            self.progress.transfer(offset - previous);
        }
    }
}

/// Progress that is thrown away.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl Progress for NoProgress {
    fn report(&self, _progress: &JobProgress) {}
}

/// Progress that is kept, for tests and for callers that want the last state.
#[derive(Debug, Default)]
pub struct ProgressLog {
    reports: std::sync::Mutex<Vec<JobProgress>>,
}

impl ProgressLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every report so far, in order.
    #[must_use]
    pub fn reports(&self) -> Vec<JobProgress> {
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Progress for ProgressLog {
    fn report(&self, progress: &JobProgress) {
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(progress.clone());
    }
}

/// Keeps the numbers of a job that has phases and reports them without ever going back.
pub(crate) struct Tracker<'a> {
    out: &'a dyn Progress,
    names: Vec<(&'static str, u32)>,
    current: usize,
    files_done: i64,
    files_total: Option<i64>,
    bytes_done: i64,
    bytes_total: Option<i64>,
    percent: u32,
    started: Instant,
    last_sent: Option<Instant>,
}

impl<'a> Tracker<'a> {
    pub(crate) fn new(out: &'a dyn Progress, names: &[(&'static str, u32)]) -> Self {
        Self {
            out,
            names: names.to_vec(),
            current: 0,
            files_done: 0,
            files_total: None,
            bytes_done: 0,
            bytes_total: None,
            percent: 0,
            started: Instant::now(),
            last_sent: None,
        }
    }

    pub(crate) fn begin(
        &mut self,
        phase: usize,
        files_total: Option<i64>,
        bytes_total: Option<i64>,
    ) {
        self.current = phase;
        self.files_done = 0;
        self.files_total = files_total;
        self.bytes_done = 0;
        self.bytes_total = bytes_total;
        self.started = Instant::now();
        self.send(true);
    }

    pub(crate) fn advance(&mut self, files: i64, bytes: i64) {
        self.files_done += files;
        self.bytes_done += bytes;
        self.send(false);
    }

    pub(crate) fn finish(&mut self) {
        self.current = self.names.len();
        self.send(true);
    }

    fn send(&mut self, force: bool) {
        let now = Instant::now();
        let percent = self.compute_percent();
        self.percent = self.percent.max(percent);
        if !force
            && self
                .last_sent
                .is_some_and(|sent| now.duration_since(sent) < PROGRESS_INTERVAL)
        {
            return;
        }
        self.last_sent = Some(now);
        let elapsed = now.duration_since(self.started).as_secs_f64();
        let rate = (elapsed > 0.0 && self.bytes_done > 0)
            .then(|| (self.bytes_done as f64 / elapsed) as i64);
        let eta = match (rate, self.bytes_total) {
            (Some(rate), Some(total)) if rate > 0 => {
                Some(((total - self.bytes_done).max(0)) / rate)
            }
            _ => None,
        };
        let phases = self
            .names
            .iter()
            .enumerate()
            .map(|(i, (name, weight))| JobPhase {
                name: (*name).to_owned(),
                weight: *weight,
                state: match i.cmp(&self.current) {
                    std::cmp::Ordering::Less => PhaseState::Done,
                    std::cmp::Ordering::Equal => PhaseState::Running,
                    std::cmp::Ordering::Greater => PhaseState::Pending,
                },
            })
            .collect();
        let phase = self
            .names
            .get(self.current)
            .map_or_else(|| "done".to_owned(), |(name, _)| (*name).to_owned());
        self.out.report(&JobProgress {
            activity: None,
            phase,
            phases,
            percent: self.percent,
            files_done: self.files_done,
            files_total: self.files_total,
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
            rate_bps: rate,
            eta_seconds: eta,
            message: None,
        });
    }

    fn compute_percent(&self) -> u32 {
        let total: u32 = self.names.iter().map(|(_, weight)| *weight).sum();
        if total == 0 {
            return 0;
        }
        let done: u32 = self
            .names
            .iter()
            .take(self.current)
            .map(|(_, weight)| *weight)
            .sum();
        let within = self.names.get(self.current).map_or(0.0, |(_, weight)| {
            let fraction = match (self.bytes_total, self.files_total) {
                (Some(bytes), _) if bytes > 0 => self.bytes_done as f64 / bytes as f64,
                (_, Some(files)) if files > 0 => self.files_done as f64 / files as f64,
                _ => 0.0,
            };
            f64::from(*weight) * fraction.clamp(0.0, 1.0)
        });
        let percent = (f64::from(done) + within) * 100.0 / f64::from(total);
        percent.floor().clamp(0.0, 100.0) as u32
    }
}

/// What a snapshot made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    /// What the command answers with.
    pub result: FsSnapshotResult,
    /// The paths listed without a hash, and why.
    pub missing: Vec<(String, String)>,
}

enum Hashed {
    Known {
        version: crate::platform::Meta,
        hash: Hash,
    },
    Gone(String),
}

impl Files {
    /// Hashes the file at `path` as it is now, once more if it changed while it was read.
    async fn hash_stable(&self, path: &RelPath) -> Hashed {
        let mut last_error = String::from("it keeps changing");
        for _ in 0..2 {
            let this = self.clone();
            let asked = path.clone();
            let opened = match blocking(move || this.open_regular(&asked)).await {
                Ok(opened) => opened,
                Err(error) => return Hashed::Gone(error.to_string()),
            };
            let size = i64::try_from(opened.meta.size).unwrap_or(i64::MAX);
            if size > self.limits().max_blob_bytes {
                return Hashed::Gone(format!(
                    "it has {size} bytes, more than {} that are stored",
                    self.limits().max_blob_bytes
                ));
            }
            match self.hash_opened(&opened).await {
                Ok(hash) => {
                    return Hashed::Known {
                        version: opened.meta,
                        hash,
                    };
                }
                Err(HostError::Changed(why)) => last_error = why,
                Err(error) => return Hashed::Gone(error.to_string()),
            }
        }
        Hashed::Gone(last_error)
    }

    /// Takes a snapshot of what `selection` names: lists it, hashes what is not hashed, uploads
    /// what the store lacks and uploads the list. Files that cannot be had are listed without a hash.
    pub async fn snapshot(
        &self,
        selection: &Selection,
        generation: i64,
        clock: &dyn Clock,
        sink: &dyn BlobSink,
        progress: &dyn Progress,
    ) -> Result<SnapshotReport> {
        self.snapshot_kind(selection, generation, clock, sink, progress, None)
            .await
            .map(|(report, _)| report)
    }

    /// A copied linked worktree has private HEAD/index and its own complete object/ref store.
    pub(crate) async fn snapshot_independent(
        &self,
        selection: &Selection,
        directory: &str,
        generation: i64,
        clock: &dyn Clock,
        sink: &dyn BlobSink,
        progress: &dyn Progress,
    ) -> Result<(SnapshotReport, bool)> {
        self.snapshot_kind(
            selection,
            generation,
            clock,
            sink,
            progress,
            Some(directory),
        )
        .await
    }

    async fn snapshot_kind(
        &self,
        selection: &Selection,
        generation: i64,
        clock: &dyn Clock,
        sink: &dyn BlobSink,
        progress: &dyn Progress,
        directory: Option<&str>,
    ) -> Result<(SnapshotReport, bool)> {
        let mut tracker = Tracker::new(progress, &PHASES);

        // scan
        tracker.begin(PHASE_SCAN, None, None);
        let this = self.clone();
        let chosen = selection.clone();
        let directory = directory.map(str::to_owned);
        let (resolved, entries, mapping) = blocking(move || {
            let resolved = manifest::resolve(this.root(), &chosen)?;
            let entries = manifest::collect(&this, &resolved)?;
            let mapping = directory
                .as_deref()
                .map(|directory| crate::git_copy::independent(&this, &chosen, directory))
                .transpose()?
                .flatten();
            Ok((
                resolved,
                entries,
                mapping.map(|mapping| (mapping, directory.unwrap_or_default())),
            ))
        })
        .await?;
        let mut entries: BTreeMap<String, ManifestEntry> = entries
            .into_iter()
            .map(|entry| (entry.p.clone(), entry))
            .collect();
        let independent = mapping.is_some();
        let (sources, inline) = match mapping {
            Some((mapping, directory)) => {
                crate::git_copy::map(mapping, self, &resolved, &directory, &mut entries)?
            }
            None => (BTreeMap::new(), BTreeMap::new()),
        };
        let files: Vec<String> = entries
            .values()
            .filter(|entry| entry.t == EntryKind::File)
            .map(|entry| entry.p.clone())
            .collect();

        // hash
        let unhashed: Vec<String> = files
            .iter()
            .filter(|path| entries[*path].h.is_none())
            .cloned()
            .collect();
        let to_hash: i64 = unhashed.iter().map(|path| entries[path].s).sum();
        tracker.begin(
            PHASE_HASH,
            i64::try_from(unhashed.len()).ok(),
            Some(to_hash),
        );
        let mut missing: BTreeMap<String, String> = BTreeMap::new();
        let mut results = futures::stream::iter(unhashed.iter().cloned())
            .map(|path| {
                let this = self.clone();
                let from_root = sources
                    .get(&path)
                    .cloned()
                    .or_else(|| join_below(&resolved.root, &path));
                async move {
                    let outcome = match from_root {
                        Some(real) => this.hash_stable(&real).await,
                        None => Hashed::Gone("its path cannot be rebuilt".to_owned()),
                    };
                    (path, outcome)
                }
            })
            .buffer_unordered(HASH_PARALLEL);
        while let Some((path, outcome)) = results.next().await {
            crate::error::check_cancelled()?;
            let size_before = entries[&path].s;
            match outcome {
                Hashed::Known { version, hash } => {
                    if let Some(entry) = entries.get_mut(&path) {
                        // The list says what was hashed, not what was seen before it.
                        entry.h = Some(hash);
                        entry.s = i64::try_from(version.size).unwrap_or(i64::MAX);
                        entry.m = version.mtime_ns;
                        entry.x = version.mode;
                    }
                }
                Hashed::Gone(why) => {
                    missing.insert(path, why);
                }
            }
            tracker.advance(1, size_before);
        }
        drop(results);

        // upload
        let mut by_hash: BTreeMap<Hash, Vec<String>> = BTreeMap::new();
        for path in &files {
            if let Some(hash) = entries[path].h {
                by_hash.entry(hash).or_default().push(path.clone());
            }
        }
        let wanted: Vec<Hash> = by_hash.keys().copied().collect();
        let mut lacking: Vec<Hash> = Vec::new();
        for group in wanted.chunks(MAX_MISSING_HASHES) {
            lacking.extend(
                sink.missing(group)
                    .await
                    .map_err(|error| HostError::Content(error.to_string()))?,
            );
        }
        let upload_bytes: i64 = lacking
            .iter()
            .map(|hash| entries[&by_hash[hash][0]].s)
            .sum();
        tracker.begin(
            PHASE_UPLOAD,
            i64::try_from(lacking.len()).ok(),
            Some(upload_bytes),
        );
        let mut uploaded_bytes = 0i64;
        for hash in lacking {
            if let Some(bytes) = inline.get(&hash) {
                let size = i64::try_from(bytes.len()).map_err(|_| {
                    HostError::Unsupported("the independent configuration is too large".into())
                })?;
                sink.put(hash, size, &mut MemoryReader::new(bytes.clone()))
                    .await
                    .map_err(|error| HostError::Content(error.to_string()))?;
                uploaded_bytes += size;
                tracker.advance(1, size);
                continue;
            }
            let paths = by_hash.get(&hash).cloned().unwrap_or_default();
            let mut sent = false;
            let mut settled: std::collections::BTreeSet<&String> =
                std::collections::BTreeSet::new();
            for path in &paths {
                let Some(real) = sources
                    .get(path)
                    .cloned()
                    .or_else(|| join_below(&resolved.root, path))
                else {
                    continue;
                };
                match self.upload_file(&real, hash, sink).await {
                    Ok(size) => {
                        uploaded_bytes += size;
                        tracker.advance(1, size);
                        sent = true;
                        break;
                    }
                    Err(HostError::Changed(_)) => {
                        // The file is another by now: its new content has a hash of its own, and
                        // that content is what the list will say it is.
                        settled.insert(path);
                        match self.rehash_and_send(&real, sink).await? {
                            Resent::Sent {
                                hash: new_hash,
                                version,
                                bytes,
                            } => {
                                uploaded_bytes += bytes;
                                if let Some(entry) = entries.get_mut(path) {
                                    entry.h = Some(new_hash);
                                    entry.s = i64::try_from(version.size).unwrap_or(i64::MAX);
                                    entry.m = version.mtime_ns;
                                }
                            }
                            Resent::Gone(why) => {
                                missing.insert(path.clone(), why);
                            }
                        }
                    }
                    Err(HostError::NotFound(why)) => {
                        settled.insert(path);
                        missing.insert(path.clone(), why);
                    }
                    Err(error) => return Err(error),
                }
            }
            if !sent {
                // Not one file that held this content could be read as it was: none of the rest is whole.
                for path in paths.iter().filter(|path| !settled.contains(path)) {
                    missing
                        .entry(path.clone())
                        .or_insert_with(|| "its content could not be sent".to_owned());
                }
            }
        }

        // manifest
        tracker.begin(PHASE_MANIFEST, None, None);
        for path in missing.keys() {
            if let Some(entry) = entries.get_mut(path) {
                entry.h = None;
            }
        }
        let header = manifest::header_for(self.root(), &resolved.root, generation, clock.now());
        let written = manifest::build(&header, entries.into_values().collect())?;
        let size = i64::try_from(written.bytes.len()).unwrap_or(i64::MAX);
        if !sink
            .missing(&[written.hash])
            .await
            .map_err(|e| HostError::Content(e.to_string()))?
            .is_empty()
        {
            let mut reader = MemoryReader::new(written.bytes.clone());
            sink.put(written.hash, size, &mut reader)
                .await
                .map_err(|e| HostError::Content(e.to_string()))?;
            uploaded_bytes += size;
        }
        tracker.finish();
        let missing_count = i64::try_from(missing.len()).unwrap_or(i64::MAX);
        Ok((
            SnapshotReport {
                result: FsSnapshotResult {
                    manifest_blob: written.hash,
                    file_count: written.file_count,
                    total_bytes: written.total_bytes,
                    uploaded_bytes,
                    missing: missing_count,
                },
                missing: missing.into_iter().collect(),
            },
            independent,
        ))
    }

    /// Uploads the file at `path`, which is to have the content `hash`; says how many bytes went.
    async fn upload_file(&self, path: &RelPath, hash: Hash, sink: &dyn BlobSink) -> Result<i64> {
        let this = self.clone();
        let asked = path.clone();
        let opened = blocking(move || this.open_regular(&asked)).await?;
        let size = i64::try_from(opened.meta.size).unwrap_or(i64::MAX);
        let mut reader = FileReader::new(Arc::clone(&opened.file));
        match sink.put(hash, size, &mut reader).await {
            Ok(()) => Ok(size),
            Err(crate::blobs::BlobError::HashMismatch) => Err(HostError::Changed(format!(
                "{path} changed while it was sent"
            ))),
            Err(error) => Err(HostError::Content(error.to_string())),
        }
    }
}

/// What came of hashing a file again after it changed, and sending what it became.
enum Resent {
    Sent {
        hash: Hash,
        version: crate::platform::Meta,
        bytes: i64,
    },
    Gone(String),
}

impl Files {
    /// Hashes the file at `path` as it is now and makes sure the store holds that content.
    async fn rehash_and_send(&self, path: &RelPath, sink: &dyn BlobSink) -> Result<Resent> {
        match self.hash_stable(path).await {
            Hashed::Gone(why) => Ok(Resent::Gone(why)),
            Hashed::Known { version, hash } => {
                let lacking = sink
                    .missing(&[hash])
                    .await
                    .map_err(|error| HostError::Content(error.to_string()))?;
                if lacking.is_empty() {
                    return Ok(Resent::Sent {
                        hash,
                        version,
                        bytes: 0,
                    });
                }
                match self.upload_file(path, hash, sink).await {
                    Ok(bytes) => Ok(Resent::Sent {
                        hash,
                        version,
                        bytes,
                    }),
                    Err(HostError::Changed(why) | HostError::NotFound(why)) => {
                        Ok(Resent::Gone(why))
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
}

/// The path from the workspace root of an entry `below` the manifest's root.
fn join_below(root: &RelPath, below: &str) -> Option<RelPath> {
    let mut path = root.clone();
    for name in below.split('/') {
        path = path.join(name).ok()?;
    }
    Some(path)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::blobs::MemoryBlobs;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::{PathStyle, Workspace};
    use orch_types::ManualClock;

    fn files_with(root: &std::path::Path, patterns: &[&str], limits: Limits) -> Files {
        let ws = Workspace::open(root, &[], PathStyle::Posix, true).unwrap();
        let rules = Arc::new(IgnoreRules::new(
            &patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            true,
        ));
        Files::workspace(
            ws,
            rules,
            Arc::new(HashCache::new()),
            limits,
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    fn clock() -> ManualClock {
        ManualClock::new(chrono::DateTime::from_timestamp(1_791_484_919, 0).unwrap())
    }

    fn selection(include: &[&str]) -> Selection {
        Selection {
            include: include.iter().map(|s| (*s).to_owned()).collect(),
            exclude: vec![],
        }
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn tree(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("proj/src")).unwrap();
        std::fs::write(root.join("proj/src/a.rs"), "fn a() {}").unwrap();
        std::fs::write(root.join("proj/src/b.rs"), "fn b() {}").unwrap();
        std::fs::write(root.join("proj/copy-of-a.rs"), "fn a() {}").unwrap();
        std::fs::write(root.join("proj/debug.log"), "noise").unwrap();
        std::fs::write(root.join("proj/empty"), "").unwrap();
    }

    #[test]
    fn snapshot_patterns_match_nested_paths_without_capturing_siblings_or_ignored_bytes() {
        let root = tempfile::tempdir().unwrap();
        tree(root.path());
        std::fs::create_dir_all(root.path().join("proj/src/deep")).unwrap();
        std::fs::write(root.path().join("proj/src/deep/c.rs"), "nested").unwrap();
        std::fs::write(root.path().join("proj/src/deep/c.txt"), "unmatched").unwrap();
        std::fs::write(root.path().join("proj/src/secret.rs"), "hidden").unwrap();
        let files = files_with(root.path(), &["secret.rs", "*.log"], Limits::default());
        let store = MemoryBlobs::new();
        let chosen = Selection {
            include: vec!["proj/src/**/*.rs".to_owned()],
            exclude: vec!["**/b.rs".to_owned()],
        };
        let report = block_on(files.snapshot(&chosen, 1, &clock(), &store, &NoProgress)).unwrap();
        let parsed =
            manifest::parse(&store.get_bytes(&report.result.manifest_blob).unwrap()).unwrap();
        let paths: Vec<&str> = parsed
            .entries
            .iter()
            .filter(|entry| entry.t == manifest::EntryKind::File)
            .map(|entry| entry.p.as_str())
            .collect();
        assert_eq!(paths, ["proj/src/a.rs", "proj/src/deep/c.rs"]);
        assert_eq!(report.result.file_count, 2);
        assert!(!store.holds(&Hash::of(b"unmatched")) && !store.holds(&Hash::of(b"hidden")));
        for pattern in ["../proj/**", "proj/**/../src", "/proj/*", "proj/a**b"] {
            let refused =
                block_on(files.snapshot(&selection(&[pattern]), 1, &clock(), &store, &NoProgress));
            assert!(refused.is_err(), "{pattern}");
        }
    }

    #[test]
    fn a_snapshot_uploads_only_what_the_store_lacks_each_content_once() {
        let root = tempfile::tempdir().unwrap();
        tree(root.path());
        let files = files_with(root.path(), &["*.log"], Limits::default());
        let store = MemoryBlobs::new();
        let held = store.insert(&b"fn b() {}"[..]);
        let log = ProgressLog::new();
        let report =
            block_on(files.snapshot(&selection(&["proj"]), 2, &clock(), &store, &log)).unwrap();
        assert_eq!(
            report.result.file_count, 4,
            "a.rs, b.rs, copy-of-a.rs and the empty file; the log is left out"
        );
        assert_eq!(report.result.missing, 0);
        // The content b.rs has was there; a.rs and its copy are one content; the empty file is one more.
        let uploads = store.uploads();
        assert_eq!(
            uploads.len(),
            3,
            "two contents and the manifest: {uploads:?}"
        );
        assert!(
            !uploads.contains(&held),
            "what the store held is not sent again"
        );
        assert!(
            uploads.contains(&Hash::of(b"fn a() {}"))
                && uploads.contains(&report.result.manifest_blob)
        );
        assert_eq!(
            report.result.uploaded_bytes,
            9 + i64::try_from(store.get_bytes(&report.result.manifest_blob).unwrap().len())
                .unwrap()
        );

        // The manifest lists the files by path below the directory, with their hashes.
        let parsed =
            manifest::parse(&store.get_bytes(&report.result.manifest_blob).unwrap()).unwrap();
        let by_path: BTreeMap<&str, &ManifestEntry> =
            parsed.entries.iter().map(|e| (e.p.as_str(), e)).collect();
        assert_eq!(by_path["src/a.rs"].h, Some(Hash::of(b"fn a() {}")));
        assert_eq!(by_path["copy-of-a.rs"].h, by_path["src/a.rs"].h);
        assert!(!by_path.contains_key("debug.log"));
        assert_eq!(parsed.header.generation, 2);

        // The same tree again sends nothing but, at most, the manifest, which is the same.
        let again =
            block_on(files.snapshot(&selection(&["proj"]), 2, &clock(), &store, &NoProgress))
                .unwrap();
        assert_eq!(store.uploads().len(), 3, "nothing is uploaded twice");
        assert_eq!(again.result.uploaded_bytes, 0);
        assert_eq!(again.result.manifest_blob, report.result.manifest_blob);

        // Progress ended at a hundred, and never went down.
        let reports = log.reports();
        assert_eq!(reports.last().unwrap().percent, 100);
        assert!(
            reports
                .windows(2)
                .all(|pair| pair[0].percent <= pair[1].percent)
        );
        assert_eq!(
            reports[0]
                .phases
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["scan", "hash", "upload", "manifest", "finalize"]
        );
    }

    #[test]
    fn a_file_that_cannot_be_had_is_listed_without_a_hash_and_makes_the_snapshot_partial() {
        let root = tempfile::tempdir().unwrap();
        tree(root.path());
        let limits = Limits {
            max_blob_bytes: 7,
            ..Limits::default()
        };
        let files = files_with(root.path(), &["*.log"], limits);
        let store = MemoryBlobs::new();
        let report =
            block_on(files.snapshot(&selection(&["proj"]), 1, &clock(), &store, &NoProgress))
                .unwrap();
        // Four files hold 9 bytes each except the empty one: the three larger than the limit are missing.
        assert_eq!(report.result.missing, 3);
        assert_eq!(report.missing.len(), 3);
        assert!(
            report
                .missing
                .iter()
                .all(|(_, why)| why.contains("more than 7")),
            "{:?}",
            report.missing
        );
        let parsed =
            manifest::parse(&store.get_bytes(&report.result.manifest_blob).unwrap()).unwrap();
        let hashed: Vec<&str> = parsed
            .entries
            .iter()
            .filter(|e| e.h.is_some())
            .map(|e| e.p.as_str())
            .collect();
        assert_eq!(
            hashed,
            ["empty"],
            "what could not be had is listed, with no hash"
        );
        assert_eq!(
            report.result.file_count, 4,
            "and still counted among the files the list names"
        );
    }

    /// A store that lets the tree change once, after the files were hashed and before any was sent.
    struct ChangesTheTree {
        inner: MemoryBlobs,
        root: std::path::PathBuf,
        done: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl BlobSink for ChangesTheTree {
        async fn missing(
            &self,
            hashes: &[Hash],
        ) -> std::result::Result<Vec<Hash>, crate::blobs::BlobError> {
            if !self.done.swap(true, std::sync::atomic::Ordering::SeqCst) {
                std::fs::write(self.root.join("proj/src/b.rs"), "fn b() { changed }").unwrap();
                std::fs::remove_file(self.root.join("proj/src/a.rs")).unwrap();
            }
            self.inner.missing(hashes).await
        }
        async fn put(
            &self,
            hash: Hash,
            size: i64,
            source: &mut dyn crate::blobs::BlobReader,
        ) -> std::result::Result<(), crate::blobs::BlobError> {
            self.inner.put(hash, size, source).await
        }
    }

    #[test]
    fn files_that_change_or_vanish_while_the_snapshot_is_taken_are_listed_as_they_became_or_as_missing()
     {
        let root = tempfile::tempdir().unwrap();
        tree(root.path());
        std::fs::write(root.path().join("proj/copy-of-a.rs"), "fn copy() {}").unwrap();
        let files = files_with(root.path(), &["*.log"], Limits::default());
        let store = ChangesTheTree {
            inner: MemoryBlobs::new(),
            root: root.path().to_path_buf(),
            done: false.into(),
        };
        let report =
            block_on(files.snapshot(&selection(&["proj"]), 1, &clock(), &store, &NoProgress))
                .unwrap();
        let parsed =
            manifest::parse(&store.inner.get_bytes(&report.result.manifest_blob).unwrap()).unwrap();
        let by_path: BTreeMap<&str, &ManifestEntry> =
            parsed.entries.iter().map(|e| (e.p.as_str(), e)).collect();
        // The file that was rewritten is listed with the content it has now, which was sent.
        let b = by_path["src/b.rs"];
        assert_eq!(b.h, Some(Hash::of(b"fn b() { changed }")));
        assert_eq!(b.s, 18, "the list says what was hashed and sent");
        assert!(store.inner.holds(&Hash::of(b"fn b() { changed }")));
        // The one that vanished is listed without a hash, and the snapshot is partial; the rest are whole.
        assert_eq!(by_path["src/a.rs"].h, None);
        assert_eq!(by_path["copy-of-a.rs"].h, Some(Hash::of(b"fn copy() {}")));
        assert_eq!(report.result.missing, 1);
        assert_eq!(report.missing[0].0, "src/a.rs");
    }

    #[test]
    fn what_the_ignore_rules_leave_out_is_in_no_snapshot_and_no_upload() {
        let root = tempfile::tempdir().unwrap();
        tree(root.path());
        std::fs::write(root.path().join("proj/.env"), "SECRET=1").unwrap();
        let files = files_with(root.path(), &[".env", "*.log"], Limits::default());
        let store = MemoryBlobs::new();
        let report =
            block_on(files.snapshot(&selection(&[]), 1, &clock(), &store, &NoProgress)).unwrap();
        let parsed =
            manifest::parse(&store.get_bytes(&report.result.manifest_blob).unwrap()).unwrap();
        assert!(
            parsed
                .entries
                .iter()
                .all(|e| !e.p.ends_with(".env") && !e.p.ends_with(".log"))
        );
        assert!(!store.holds(&Hash::of(b"SECRET=1")) && !store.holds(&Hash::of(b"noise")));
    }

    #[test]
    fn a_store_that_asks_for_a_thousand_hashes_at_a_time_gets_them_in_groups() {
        let root = tempfile::tempdir().unwrap();
        for n in 0..2500 {
            std::fs::write(root.path().join(format!("f{n:04}")), format!("content {n}")).unwrap();
        }
        let files = files_with(root.path(), &[], Limits::default());
        struct Counting {
            inner: MemoryBlobs,
            asked: std::sync::Mutex<Vec<usize>>,
        }
        #[async_trait::async_trait]
        impl BlobSink for Counting {
            async fn missing(
                &self,
                hashes: &[Hash],
            ) -> std::result::Result<Vec<Hash>, crate::blobs::BlobError> {
                self.asked.lock().unwrap().push(hashes.len());
                self.inner.missing(hashes).await
            }
            async fn put(
                &self,
                hash: Hash,
                size: i64,
                source: &mut dyn crate::blobs::BlobReader,
            ) -> std::result::Result<(), crate::blobs::BlobError> {
                self.inner.put(hash, size, source).await
            }
        }
        let store = Counting {
            inner: MemoryBlobs::new(),
            asked: std::sync::Mutex::new(Vec::new()),
        };
        let report =
            block_on(files.snapshot(&selection(&[]), 1, &clock(), &store, &NoProgress)).unwrap();
        assert_eq!(report.result.file_count, 2500);
        let asked = store.asked.lock().unwrap().clone();
        assert_eq!(&asked[..3], &[1000, 1000, 500], "{asked:?}");
        assert_eq!(store.inner.uploads().len(), 2501);
    }
}

#[cfg(test)]
mod cancellation_progress_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct CancelAfterFirstChunk {
        cancel: tokio_util::sync::CancellationToken,
        copied: AtomicI64,
    }
    impl Progress for CancelAfterFirstChunk {
        fn report(&self, _: &JobProgress) {}
        fn transfer(&self, bytes: i64) {
            self.copied.fetch_add(bytes, Ordering::SeqCst);
            self.cancel.cancel();
        }
    }

    #[tokio::test]
    async fn a_blocking_restore_copy_reports_within_one_file_and_stops_on_cancellation() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let progress = Arc::new(CancelAfterFirstChunk {
            cancel: cancel.clone(),
            copied: AtomicI64::new(0),
        });
        let reporting: Arc<dyn Progress> = progress.clone();
        let (result, bytes) = crate::error::with_cancellation(
            cancel,
            with_transfer_progress(
                reporting,
                crate::error::blocking(move || {
                    let source = vec![1u8; 1024 * 1024];
                    let mut output = Vec::new();
                    let result = copy_counted(&mut source.as_slice(), &mut output);
                    Ok((result, output.len()))
                }),
            ),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(crate::error::HostError::Cancelled)));
        assert_eq!(bytes, 64 * 1024);
        assert_eq!(progress.copied.load(Ordering::SeqCst), bytes as i64);
    }
}
