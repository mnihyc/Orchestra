//! Owned readers of immutable stored archives; request authority stays with each caller.

use super::*;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_link_contract::commands::WorkspaceLimits;
use orch_types::{Id, new_id};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
const WIRE_BYTES: usize = 2 * 1024 * 1024 - orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES;

/// One authorized archive selection and the window a caller asks for now.
#[derive(Clone)]
pub struct ArchiveRead {
    /// The immutable blob that this caller independently holds.
    pub meta: BlobMeta,
    /// The folder inside it; omitted selects the root.
    pub inside: Option<String>,
    /// A substring of child names, without regard to case.
    pub query: Option<String>,
    /// Continue exactly this selection.
    pub cursor: Option<String>,
    /// Positive rows in this window.
    pub limit: usize,
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    hash: Hash,
    size: i64,
    inside: String,
    query: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    read: Id,
    page: u64,
}
#[derive(Clone, Copy)]
struct Info {
    format: ArchiveFormat,
    compression: Option<ArchiveCompression>,
    encrypted: bool,
}

struct BudgetState {
    bytes: u64,
    maximum: u64,
    active: u64,
    slots: u64,
}
struct Budget(Mutex<BudgetState>);
struct Credit {
    budget: Arc<Budget>,
    bytes: u64,
}
impl Drop for Credit {
    fn drop(&mut self) {
        lock(&self.budget.0).bytes -= self.bytes;
    }
}
struct Owner(Arc<Budget>);
impl Drop for Owner {
    fn drop(&mut self) {
        lock(&self.0.0).active -= 1;
    }
}
impl Budget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Credit, ArchiveError> {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        let mut state = lock(&self.0);
        let next = state
            .bytes
            .checked_add(bytes)
            .filter(|next| *next <= state.maximum)
            .ok_or(ArchiveError::Busy)?;
        state.bytes = next;
        Ok(Credit {
            budget: Arc::clone(self),
            bytes,
        })
    }
    fn owner(self: &Arc<Self>) -> Result<Owner, ArchiveError> {
        let mut state = lock(&self.0);
        if state.active >= state.slots {
            return Err(ArchiveError::Busy);
        }
        state.active += 1;
        Ok(Owner(Arc::clone(self)))
    }
}
struct Row {
    entry: ArchiveEntry,
    _credit: Credit,
}
struct Reading {
    rows: mpsc::Receiver<Result<Row, ArchiveError>>,
    info: watch::Receiver<Option<Result<Info, ArchiveError>>>,
    next: u64,
    last: Option<(u64, ArchiveListing)>,
    last_credit: Option<Credit>,
    building: Vec<Row>,
    pending: Option<Row>,
    bytes: usize,
    complete: bool,
    failure: Option<ArchiveError>,
}
struct Read {
    id: Id,
    key: Key,
    cancel: CancellationToken,
    last_use: Mutex<Instant>,
    idle: Duration,
    active: AtomicUsize,
    reader: tokio::sync::Mutex<Reading>,
    driver: Mutex<Option<tokio::task::JoinHandle<()>>>,
    _credit: Credit,
}
impl Drop for Read {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Read {
    fn finished(&self) -> bool {
        lock(&self.driver)
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }
    async fn join(&self) {
        let driver = lock(&self.driver).take();
        if let Some(driver) = driver {
            let _ = driver.await;
        }
    }
    fn arm(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            loop {
                let remaining = {
                    let Some(read) = weak.upgrade() else {
                        return;
                    };
                    let elapsed = lock(&read.last_use).elapsed();
                    if read.active.load(Ordering::Acquire) > 0 {
                        read.idle
                    } else {
                        read.idle.saturating_sub(elapsed)
                    }
                };
                if remaining.is_zero() {
                    cancel.cancel();
                    return;
                }
                let Some(deadline) = Instant::now().checked_add(remaining) else {
                    cancel.cancelled().await;
                    return;
                };
                tokio::select! { biased; () = cancel.cancelled() => return, () = tokio::time::sleep_until(deadline.into()) => {} }
            }
        });
    }
}
struct Pool {
    reads: Mutex<HashMap<Id, Arc<Read>>>,
    budget: Arc<Budget>,
}

/// A shared bounded pool, not a separate cache for each file, directory or user.
#[derive(Clone)]
pub struct ArchiveReads(Arc<Pool>);
impl Default for ArchiveReads {
    fn default() -> Self {
        let policy = WorkspaceLimits::default();
        Self(Arc::new(Pool {
            reads: Mutex::new(HashMap::new()),
            budget: Arc::new(Budget(Mutex::new(BudgetState {
                bytes: 0,
                maximum: policy.directory_cache_bytes,
                active: 0,
                slots: policy.directory_slots,
            }))),
        }))
    }
}
struct FirstPage {
    read: Arc<Read>,
    keep: bool,
}
impl Drop for FirstPage {
    fn drop(&mut self) {
        *lock(&self.read.last_use) = Instant::now();
        self.read.active.fetch_sub(1, Ordering::AcqRel);
        if !self.keep {
            self.read.cancel.cancel();
        }
    }
}

impl ArchiveReads {
    /// Every page must be preceded by the route's current source/permission checks.
    pub async fn list(
        &self,
        blobs: &Blobs,
        request: ArchiveRead,
        policy: &WorkspaceLimits,
    ) -> Result<ArchiveListing, ArchiveError> {
        if request.limit == 0 {
            return Err(unreadable("an archive page needs a positive row count"));
        }
        {
            let mut state = lock(&self.0.budget.0);
            state.maximum = policy.directory_cache_bytes;
            state.slots = policy.directory_slots;
        }
        self.reap().await;
        let key = Key {
            hash: request.meta.hash,
            size: request.meta.size,
            inside: request
                .inside
                .as_deref()
                .map(clean)
                .unwrap_or("")
                .to_owned(),
            query: request.query.as_deref().unwrap_or("").to_lowercase(),
        };
        let cursor = request
            .cursor
            .as_deref()
            .map(|text| {
                let bytes = URL_SAFE_NO_PAD
                    .decode(text)
                    .map_err(|_| ArchiveError::Stale)?;
                serde_json::from_slice::<Cursor>(&bytes).map_err(|_| ArchiveError::Stale)
            })
            .transpose()?;
        let (read, page) = if let Some(cursor) = cursor {
            let read = lock(&self.0.reads)
                .get(&cursor.read)
                .cloned()
                .ok_or(ArchiveError::Stale)?;
            if read.key != key || read.cancel.is_cancelled() {
                return Err(ArchiveError::Stale);
            }
            (read, cursor.page)
        } else {
            let mut reads = lock(&self.0.reads);
            if reads.len() as u64 >= policy.directory_slots {
                return Err(ArchiveError::Busy);
            }
            let owner = self.0.budget.owner()?;
            let credit = self
                .0
                .budget
                .reserve(std::mem::size_of::<Read>() + key.inside.len() + key.query.len())?;
            let source_credit = self.0.budget.reserve(WINDOW_BYTES as usize)?;
            let (delivery, rows) = mpsc::channel(1);
            let (metadata, info) = watch::channel(None);
            let cancel = CancellationToken::new();
            let read = Arc::new(Read {
                id: new_id(),
                key: key.clone(),
                cancel: cancel.clone(),
                last_use: Mutex::new(Instant::now()),
                idle: Duration::from_secs(policy.directory_idle_secs),
                active: AtomicUsize::new(0),
                reader: tokio::sync::Mutex::new(Reading {
                    rows,
                    info,
                    next: 0,
                    last: None,
                    last_credit: None,
                    building: Vec::new(),
                    pending: None,
                    bytes: 2,
                    complete: false,
                    failure: None,
                }),
                driver: Mutex::new(None),
                _credit: credit,
            });
            let store = blobs.store();
            let runtime = tokio::runtime::Handle::current();
            let budget = Arc::clone(&self.0.budget);
            let meta = request.meta;
            let token = cancel.clone();
            let output = delivery.clone();
            let published = metadata.clone();
            let driver = tokio::spawn(async move {
                let worker = tokio::task::spawn_blocking(move || {
                    let (_owner, _source_credit) = (owner, source_credit);
                    stream(
                        &store,
                        meta,
                        &runtime,
                        &key,
                        &token,
                        &mut |info| {
                            published.send_replace(Some(Ok(info)));
                            Ok(())
                        },
                        &mut |entry| {
                            let credit = budget.reserve(
                                serde_json::to_vec(&entry).map_err(unreadable)?.len()
                                    + std::mem::size_of::<Row>(),
                            )?;
                            runtime.block_on(async { tokio::select! { biased; () = token.cancelled() => Err(ArchiveError::Stale), result = output.send(Ok(Row { entry, _credit: credit })) => result.map_err(|_| ArchiveError::Stale) } })
                        },
                    )
                });
                let result = worker
                    .await
                    .unwrap_or_else(|error| Err(ArchiveError::Failed(error.to_string())));
                if let Err(error) = result {
                    if metadata.borrow().is_none() {
                        metadata.send_replace(Some(Err(error.clone())));
                    }
                    tokio::select! { biased; () = cancel.cancelled() => {}, _ = delivery.send(Err(error)) => {} }
                }
            });
            *lock(&read.driver) = Some(driver);
            read.arm();
            reads.insert(read.id, Arc::clone(&read));
            (read, 0)
        };
        *lock(&read.last_use) = Instant::now();
        read.active.fetch_add(1, Ordering::AcqRel);
        let mut first = FirstPage {
            read: Arc::clone(&read),
            keep: page != 0,
        };
        let mut reader = read.reader.lock().await;
        if let Some((last, result)) = &reader.last
            && *last == page
        {
            first.keep = true;
            return Ok(result.clone());
        }
        if reader.next != page {
            return Err(ArchiveError::Stale);
        }
        let info = loop {
            let info = reader.info.borrow().clone();
            if let Some(info) = info {
                break info?;
            }
            tokio::select! { biased; () = read.cancel.cancelled() => return Err(ArchiveError::Stale), result = reader.info.changed() => result.map_err(|_| ArchiveError::Failed("the archive reader stopped before its format was known".to_owned()))? }
        };
        let window = usize::try_from(policy.directory_page_bytes)
            .unwrap_or(usize::MAX)
            .min(WIRE_BYTES);
        let timer = tokio::time::sleep(Duration::from_secs(policy.search_slice_secs));
        tokio::pin!(timer);
        loop {
            if let Some(error) = &reader.failure {
                return Err(error.clone());
            }
            if reader.building.len() >= request.limit || reader.complete {
                break;
            }
            let row = match reader.pending.take() {
                Some(row) => Some(Ok(row)),
                None => {
                    tokio::select! { biased; () = read.cancel.cancelled() => return Err(ArchiveError::Stale), () = &mut timer => break, row = reader.rows.recv() => row }
                }
            };
            match row {
                None => reader.complete = true,
                Some(Err(error)) => reader.failure = Some(error),
                Some(Ok(row)) => {
                    let bytes = serde_json::to_vec(&row.entry).map_err(unreadable)?.len() + 1;
                    if bytes + 2 > WIRE_BYTES {
                        return Err(unreadable("one archive name exceeds the wire page size"));
                    }
                    if !reader.building.is_empty() && reader.bytes.saturating_add(bytes) > window {
                        reader.pending = Some(row);
                        break;
                    }
                    reader.bytes += bytes;
                    reader.building.push(row);
                }
            }
        }
        let bytes = reader.building.iter().try_fold(0_usize, |sum, row| {
            serde_json::to_vec(&row.entry)
                .map(|bytes| sum.saturating_add(bytes.len() + std::mem::size_of::<ArchiveEntry>()))
                .map_err(unreadable)
        })?;
        let credit = self.0.budget.reserve(bytes)?;
        let next = page.checked_add(1).ok_or(ArchiveError::Stale)?;
        let next_cursor = if reader.complete {
            None
        } else {
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&Cursor {
                        read: read.id,
                        page: next,
                    })
                    .map_err(unreadable)?,
                ),
            )
        };
        let result = ArchiveListing {
            format: info.format,
            compression: info.compression,
            encrypted: info.encrypted,
            entries: std::mem::take(&mut reader.building)
                .into_iter()
                .map(|row| row.entry)
                .collect(),
            truncated: next_cursor.is_some(),
            next_cursor,
        };
        reader.bytes = 2;
        reader.next = next;
        reader.last = Some((page, result.clone()));
        reader.last_credit = Some(credit);
        first.keep = true;
        Ok(result)
    }
    async fn reap(&self) {
        let reads: Vec<_> = lock(&self.0.reads).values().cloned().collect();
        for read in reads {
            if read.active.load(Ordering::Acquire) == 0
                && lock(&read.last_use).elapsed() >= read.idle
            {
                read.cancel.cancel();
            }
            if read.cancel.is_cancelled() && read.finished() {
                read.join().await;
                lock(&self.0.reads).remove(&read.id);
            }
        }
    }
}

fn child(
    key: &Key,
    raw: &str,
    kind: ArchiveEntryKind,
    size: Option<i64>,
    modified_at: Option<Time>,
) -> Option<ArchiveEntry> {
    let raw = clean(raw);
    let below = if key.inside.is_empty() {
        raw
    } else {
        raw.strip_prefix(&key.inside)?.strip_prefix('/')?
    };
    let (name, deeper) = below
        .split_once('/')
        .map_or((below, false), |(name, _)| (name, true));
    if name.is_empty() || !name.to_lowercase().contains(&key.query) {
        return None;
    }
    let kind = if deeper { ArchiveEntryKind::Dir } else { kind };
    Some(ArchiveEntry {
        path: if key.inside.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", key.inside)
        },
        kind,
        size: if kind == ArchiveEntryKind::Dir {
            None
        } else {
            size
        },
        modified_at: if deeper { None } else { modified_at },
    })
}
fn stream(
    store: &Arc<dyn BlobStore>,
    meta: BlobMeta,
    runtime: &tokio::runtime::Handle,
    key: &Key,
    cancel: &CancellationToken,
    info: &mut dyn FnMut(Info) -> Result<(), ArchiveError>,
    emit: &mut dyn FnMut(ArchiveEntry) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let open = || {
        let mut source = Source::open(store, &meta, runtime);
        source.cancel = Some(cancel.clone());
        source
    };
    match shape_of(&open)? {
        Shape::Zip => {
            let mut source = open();
            check_zip_entries(&mut source)?;
            let mut archive = zip::ZipArchive::new(source).map_err(unreadable)?;
            let mut encrypted = false;
            for index in 0..archive.len() {
                if cancel.is_cancelled() {
                    return Err(ArchiveError::Stale);
                }
                encrypted |= archive.by_index_raw(index).map_err(unreadable)?.encrypted();
            }
            info(Info {
                format: ArchiveFormat::Zip,
                compression: None,
                encrypted,
            })?;
            for index in 0..archive.len() {
                if cancel.is_cancelled() {
                    return Err(ArchiveError::Stale);
                }
                let entry = archive.by_index_raw(index).map_err(unreadable)?;
                let Ok(name) = entry.name() else {
                    continue;
                };
                let kind = if entry.is_dir() {
                    ArchiveEntryKind::Dir
                } else if entry.is_symlink() {
                    ArchiveEntryKind::Link
                } else {
                    ArchiveEntryKind::File
                };
                if let Some(entry) = child(
                    key,
                    &name,
                    kind,
                    i64::try_from(entry.size()).ok(),
                    entry.last_modified().and_then(time_of_dos),
                ) {
                    emit(entry)?;
                }
            }
        }
        Shape::Tar(compression) => {
            info(Info {
                format: ArchiveFormat::Tar,
                compression,
                encrypted: false,
            })?;
            let mut archive = tar::Archive::new(decompress(open(), compression)?);
            for entry in archive.entries().map_err(unreadable)? {
                if cancel.is_cancelled() {
                    return Err(ArchiveError::Stale);
                }
                let entry = entry.map_err(unreadable)?;
                let header = entry.header();
                let kind = match header.entry_type() {
                    tar::EntryType::Directory => ArchiveEntryKind::Dir,
                    tar::EntryType::Symlink | tar::EntryType::Link => ArchiveEntryKind::Link,
                    tar::EntryType::Regular
                    | tar::EntryType::Continuous
                    | tar::EntryType::GNUSparse => ArchiveEntryKind::File,
                    _ => continue,
                };
                let path = entry
                    .path()
                    .map_err(unreadable)?
                    .to_string_lossy()
                    .into_owned();
                if let Some(entry) = child(
                    key,
                    &path,
                    kind,
                    header.size().ok().and_then(|size| i64::try_from(size).ok()),
                    header.mtime().ok().and_then(time_of_unix),
                ) {
                    emit(entry)?;
                }
            }
        }
    }
    Ok(())
}
