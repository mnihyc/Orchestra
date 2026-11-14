//! Live traversal with bounded handoff, replayable pages and streaming directory handles.

use super::{Binding, DirectoryPages, ENTRY_BYTES, Ledger, captures, lock, stale};
use crate::error::{HostError, Result};
use crate::files::Files;
use crate::paths::{Locate, RelPath};
use crate::search::{self, SearchRequest};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileMatch, FileSearchIn};
use orch_link_contract::commands::{FsSearch, FsSearchResult};
use orch_types::{Id, Time, new_id};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    search: Id,
    page: u64,
}

enum Hit {
    Name(DirEntry),
    Line(FileMatch),
}
impl Hit {
    fn bytes(&self) -> Result<usize> {
        Ok(match self {
            Self::Name(entry) => serde_json::to_vec(entry)?,
            Self::Line(entry) => serde_json::to_vec(entry)?,
        }
        .len())
    }
}

// Page buffers are charged alongside anonymous traversal metadata, not once per environment.
struct Charge {
    ledger: Arc<Ledger>,
    bytes: u64,
}
impl Charge {
    fn resize(&mut self, cache: &DirectoryPages, bytes: u64) -> Result<()> {
        if bytes > self.bytes {
            cache.reserve(bytes - self.bytes)?;
        } else {
            self.ledger
                .used
                .fetch_sub(self.bytes - bytes, Ordering::AcqRel);
        }
        self.bytes = bytes;
        Ok(())
    }
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.ledger.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct Packet {
    hit: Hit,
    _charge: Charge,
}

struct Reader {
    receive: mpsc::Receiver<Result<Packet>>,
    next: u64,
    last: Option<(u64, FsSearchResult)>,
    building: Vec<Packet>,
    bytes: usize,
    pending: Option<Packet>,
    failure: Option<Arc<HostError>>,
    complete: bool,
    _charge: Charge,
}

pub(super) struct Session {
    id: Id,
    binding: Binding,
    request: FsSearch,
    observed_from: Time,
    last_use: AtomicI64,
    seen: Arc<AtomicU64>,
    cancel: CancellationToken,
    reader: tokio::sync::Mutex<Reader>,
    driver: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Session {
    pub(super) fn belongs(&self, environment: Id, attachment: Option<Id>) -> bool {
        self.binding.environment == environment
            && attachment.is_none_or(|id| self.binding.attachment == id)
    }
    pub(super) fn cancel(&self) {
        self.cancel.cancel();
    }
    fn finished(&self) -> bool {
        lock(&self.driver)
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }
    async fn join(&self) -> Result<()> {
        let driver = lock(&self.driver).take();
        if let Some(driver) = driver {
            driver.await.map_err(|error| {
                HostError::Internal(format!("the file search worker failed: {error}"))
            })?;
        }
        Ok(())
    }
}

impl DirectoryPages {
    /// No full index, recursive watch or full manifest is needed for live search.
    pub(crate) async fn search(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        mut request: FsSearch,
    ) -> Result<FsSearchResult> {
        if request.limit == 0 || request.query.is_empty() {
            return Err(HostError::Unsupported(
                "a search needs a query and a positive page size".to_owned(),
            ));
        }
        let cursor = request
            .cursor
            .take()
            .map(|text| {
                if text.len() > crate::paths::PATH_MAX_BYTES {
                    return Err(stale());
                }
                let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| stale())?;
                serde_json::from_slice::<Cursor>(&bytes).map_err(|_| stale())
            })
            .transpose()?;
        self.reap_searches().await?;
        let limit = request.limit;
        request.limit = 0; // Window size does not change the selected search.
        let (session, page) = if let Some(cursor) = cursor {
            let session = lock(&self.searches)
                .get(&cursor.search)
                .cloned()
                .ok_or_else(stale)?;
            if session.binding != binding
                || session.request != request
                || session.cancel.is_cancelled()
            {
                return Err(stale());
            }
            (session, cursor.page)
        } else {
            let mut sessions = lock(&self.searches);
            if sessions.len() >= self.operations.maximum() {
                return Err(HostError::Busy(
                    "the host's file search observations are in use".to_owned(),
                ));
            }
            let permit = self.operations.acquire()?;
            let bytes =
                u64::try_from(std::mem::size_of::<Session>() + serde_json::to_vec(&request)?.len())
                    .unwrap_or(u64::MAX);
            self.reserve(bytes)?;
            let charge = Charge {
                ledger: Arc::clone(&self.ledger),
                bytes,
            };
            let (send, receive) = mpsc::channel(1);
            let cancel = CancellationToken::new();
            let seen = Arc::new(AtomicU64::new(0));
            let session = Arc::new(Session {
                id: new_id(),
                binding,
                request: request.clone(),
                observed_from: self.clock.time(),
                last_use: AtomicI64::new(self.clock.now().timestamp_millis()),
                seen: Arc::clone(&seen),
                cancel: cancel.clone(),
                reader: tokio::sync::Mutex::new(Reader {
                    receive,
                    next: 0,
                    last: None,
                    building: Vec::new(),
                    bytes: 2,
                    pending: None,
                    failure: None,
                    complete: false,
                    _charge: charge,
                }),
                driver: Mutex::new(None),
            });
            let resources = Resources {
                cache: Arc::downgrade(self),
            };
            let driver = tokio::spawn(async move {
                let token = cancel.clone();
                let output = send.clone();
                let worker = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    traverse(&resources, &files, &request, &seen, &token, &output)
                });
                let outcome = worker.await.unwrap_or_else(|error| {
                    Err(HostError::Internal(format!(
                        "the file search worker failed: {error}"
                    )))
                });
                if let Err(error) = outcome {
                    tokio::select! { biased; () = cancel.cancelled() => {}, _ = send.send(Err(error)) => {} }
                }
            });
            *lock(&session.driver) = Some(driver);
            sessions.insert(session.id, Arc::clone(&session));
            (session, 0)
        };
        session
            .last_use
            .store(self.clock.now().timestamp_millis(), Ordering::Release);
        let incoming = crate::error::cancellation().unwrap_or_default();
        let mut reader = tokio::select! { biased;
            () = incoming.cancelled() => return Err(HostError::Cancelled),
            () = session.cancel.cancelled() => return Err(stale()),
            reader = session.reader.lock() => reader,
        };
        if let Some((last, result)) = &reader.last
            && *last == page
        {
            return Ok(result.clone());
        }
        if reader.next != page {
            return Err(stale());
        }
        let policy = self.policy(session.binding.environment);
        let window = usize::try_from(policy.directory_page_bytes)
            .unwrap_or(usize::MAX)
            .min(ENTRY_BYTES);
        let timer = tokio::time::sleep(Duration::from_secs(policy.search_slice_secs));
        tokio::pin!(timer);
        loop {
            if let Some(error) = &reader.failure {
                return Err(captures::copy_error(error));
            }
            if reader.building.len() >= limit as usize || reader.complete {
                break;
            }
            let hit = match reader.pending.take() {
                Some(hit) => Some(Ok(hit)),
                None => tokio::select! { biased;
                    () = incoming.cancelled() => return Err(HostError::Cancelled),
                    () = session.cancel.cancelled() => return Err(stale()),
                    () = &mut timer => break,
                    hit = reader.receive.recv() => hit,
                },
            };
            match hit {
                None => reader.complete = true,
                Some(Err(error)) => reader.failure = Some(Arc::new(error)),
                Some(Ok(hit)) => {
                    let bytes = hit.hit.bytes()?.saturating_add(1);
                    if bytes + 2 > ENTRY_BYTES {
                        return Err(super::too_large(bytes));
                    }
                    if !reader.building.is_empty() && reader.bytes.saturating_add(bytes) > window {
                        reader.pending = Some(hit);
                        break;
                    }
                    reader.bytes += bytes;
                    reader.building.push(hit);
                }
            }
        }
        let mut cached_bytes =
            std::mem::size_of::<Session>() + serde_json::to_vec(&session.request)?.len();
        for packet in &reader.building {
            cached_bytes = cached_bytes.saturating_add(
                packet
                    .hit
                    .bytes()?
                    .saturating_add(std::mem::size_of::<Hit>()),
            );
        }
        reader
            ._charge
            .resize(self, u64::try_from(cached_bytes).unwrap_or(u64::MAX))?;
        let mut names = Vec::new();
        let mut matches = Vec::new();
        for packet in std::mem::take(&mut reader.building) {
            match packet.hit {
                Hit::Name(entry) => names.push(entry),
                Hit::Line(entry) => matches.push(entry),
            }
        }
        reader.bytes = 2;
        let next = page.checked_add(1).ok_or_else(stale)?;
        let next_cursor = if reader.complete {
            None
        } else {
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
                search: session.id,
                page: next,
            })?))
        };
        let result = FsSearchResult {
            names: (session.request.scope == FileSearchIn::Names).then_some(names),
            matches,
            truncated: next_cursor.is_some(),
            next_cursor,
            observed_from: Some(session.observed_from),
            as_of: Some(self.clock.time()),
            searched: i64::try_from(session.seen.load(Ordering::Acquire)).unwrap_or(i64::MAX),
        };
        reader.next = next;
        reader.last = Some((page, result.clone()));
        Ok(result)
    }

    async fn reap_searches(&self) -> Result<()> {
        let sessions: Vec<_> = lock(&self.searches).values().cloned().collect();
        for session in sessions {
            let idle = self
                .clock
                .now()
                .timestamp_millis()
                .saturating_sub(session.last_use.load(Ordering::Acquire));
            let maximum = self
                .policy(session.binding.environment)
                .directory_idle_secs
                .saturating_mul(1000);
            if idle >= i64::try_from(maximum).unwrap_or(i64::MAX) {
                session.cancel();
            }
            if session.cancel.is_cancelled() && session.finished() {
                session.join().await?;
                lock(&self.searches).remove(&session.id);
            }
        }
        Ok(())
    }

    pub(super) async fn join_searches(&self, environment: Id, attachment: Id) -> Result<()> {
        let sessions: Vec<_> = lock(&self.searches)
            .values()
            .filter(|search| search.belongs(environment, Some(attachment)))
            .cloned()
            .collect();
        for session in sessions {
            session.cancel();
            session.join().await?;
            lock(&self.searches).remove(&session.id);
        }
        Ok(())
    }
}

struct Resources {
    cache: Weak<DirectoryPages>,
}
impl Resources {
    fn cache(&self) -> Result<Arc<DirectoryPages>> {
        self.cache.upgrade().ok_or(HostError::Cancelled)
    }
}

fn send(
    resources: &Resources,
    send: &mpsc::Sender<Result<Packet>>,
    hit: Hit,
    cancel: &CancellationToken,
) -> Result<()> {
    let bytes = u64::try_from(hit.bytes()?.saturating_add(std::mem::size_of::<Packet>()))
        .unwrap_or(u64::MAX);
    let charge = {
        let cache = resources.cache()?;
        cache.reserve(bytes)?;
        Charge {
            ledger: Arc::clone(&cache.ledger),
            bytes,
        }
    };
    let packet = Packet {
        hit,
        _charge: charge,
    };
    tokio::runtime::Handle::current().block_on(async {
        tokio::select! { biased;
            () = cancel.cancelled() => Err(HostError::Cancelled),
            result = send.send(Ok(packet)) => result.map_err(|_| HostError::Cancelled),
        }
    })
}

fn traverse(
    resources: &Resources,
    files: &Files,
    request: &FsSearch,
    seen: &AtomicU64,
    cancel: &CancellationToken,
    output: &mpsc::Sender<Result<Packet>>,
) -> Result<()> {
    let start = files.parse(&request.path)?;
    let content = request.scope == FileSearchIn::Content;
    let search = SearchRequest {
        query: request.query.clone(),
        regex: request.regex,
        case_sensitive: request.case,
        glob: request.glob.clone(),
        limit: usize::MAX,
    };
    let pattern = content.then(|| search::compile(&search)).transpose()?;
    let glob = request
        .glob
        .as_deref()
        .map(search::compile_glob)
        .transpose()?;
    let query = if request.case {
        request.query.clone()
    } else {
        request.query.to_lowercase()
    };
    let matched = |path: &str| {
        if request.case {
            path.contains(&query)
        } else {
            path.to_lowercase().contains(&query)
        }
    };
    let selected = files.stat(&start)?;
    if selected.entry.kind == DirEntryKind::File {
        seen.fetch_add(1, Ordering::AcqRel);
        if let Some(pattern) = &pattern {
            search::stream_file(
                files,
                &start,
                &start,
                pattern,
                glob.as_ref(),
                cancel,
                &mut |hit| send(resources, output, Hit::Line(hit), cancel),
            )?;
        } else if matched(&selected.entry.path) {
            send(resources, output, Hit::Name(selected.entry), cancel)?;
        }
        return Ok(());
    }
    let mut stack = Vec::new();
    let frame = |path: &RelPath| -> Result<_> {
        let stream = files.live_directory_stream(path)?;
        let bytes = u64::try_from(stream.footprint()).unwrap_or(u64::MAX);
        let charge = {
            let cache = resources.cache()?;
            cache.reserve(bytes)?;
            Charge {
                ledger: Arc::clone(&cache.ledger),
                bytes,
            }
        };
        Ok((stream, search::GitIgnores::new(files.root()), charge))
    };
    stack.push(frame(&start)?);
    while let Some((stream, git, _)) = stack.last_mut() {
        if cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        let Some(entry) = stream.next() else {
            stack.pop();
            continue;
        };
        let entry = entry?;
        let path = files.parse(&entry.path)?;
        if content {
            if entry.name == ".git" {
                continue;
            }
            let located = files.root().locate(&path, Locate::ENTRY)?;
            if git.leaves_out(&path, entry.kind == DirEntryKind::Dir, &located.parent)
                && (entry.kind == DirEntryKind::Dir || request.glob.is_none())
            {
                continue;
            }
        } else {
            seen.fetch_add(1, Ordering::AcqRel);
            if matched(&entry.path) {
                send(resources, output, Hit::Name(entry.clone()), cancel)?;
            }
        }
        if entry.kind == DirEntryKind::Dir {
            stack.push(frame(&path)?);
        } else if let Some(pattern) = &pattern
            && entry.kind == DirEntryKind::File
        {
            seen.fetch_add(1, Ordering::AcqRel);
            search::stream_file(
                files,
                &path,
                &start,
                pattern,
                glob.as_ref(),
                cancel,
                &mut |hit| send(resources, output, Hit::Line(hit), cancel),
            )?;
        }
    }
    Ok(())
}
