//! Joined live readers with replayable byte-bounded pages and one-record handoff.

use super::{Binding, DirectoryPages, ENTRY_BYTES, Ledger, captures, lock, stale};
use crate::archive::ArchiveInfo;
use crate::error::{HostError, Result};
use crate::files::Files;
use crate::git::GitTool;
use crate::index::Index;
use crate::paths::{Locate, RelPath};
use crate::search::{self, SearchRequest};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_api_contract::workspace::{
    ArchiveEntry, ArchiveListing, DirEntry, DirEntryKind, FileMatch, FileSearchIn,
};
use orch_link_contract::commands::{
    FsArchiveList, FsSearch, FsSearchResult, GitDiff, GitDiffResult,
};
use orch_types::{Id, Time, new_id};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    search: Id,
    page: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
enum Query {
    Files(FsSearch),
    Archive(FsArchiveList),
    Difference(GitDiff),
}
impl Query {
    fn normalize(&mut self) -> Result<(Option<String>, u32, Option<u64>)> {
        match self {
            Self::Files(request) => {
                if request.query.is_empty() || request.limit == 0 {
                    return Err(HostError::Unsupported(
                        "a search needs a query and a positive page size".to_owned(),
                    ));
                }
                let rows = std::mem::take(&mut request.limit);
                Ok((request.cursor.take(), rows, None))
            }
            Self::Archive(request) => {
                if request.limit == 0 {
                    return Err(HostError::Unsupported(
                        "an archive page needs a positive row count".to_owned(),
                    ));
                }
                let rows = std::mem::take(&mut request.limit);
                Ok((request.cursor.take(), rows, None))
            }
            Self::Difference(request) => {
                if request.max_bytes == Some(0) {
                    return Err(HostError::Unsupported(
                        "a difference page needs a positive byte count".to_owned(),
                    ));
                }
                Ok((request.cursor.take(), u32::MAX, request.max_bytes.take()))
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum Page {
    Files(FsSearchResult),
    Archive(ArchiveListing),
    Difference(GitDiffResult),
}

enum Hit {
    Name(DirEntry),
    Line(FileMatch),
    Archive(ArchiveEntry),
    Difference(String),
}
impl Hit {
    fn bytes(&self) -> Result<usize> {
        Ok(match self {
            Self::Name(entry) => serde_json::to_vec(entry)?,
            Self::Line(entry) => serde_json::to_vec(entry)?,
            Self::Archive(entry) => serde_json::to_vec(entry)?,
            Self::Difference(text) => serde_json::to_vec(text)?,
        }
        .len())
    }
}

// Query records, native iterator metadata, retained pages and in-flight rows share the host ledger.
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

type ArchiveMetadata = std::result::Result<ArchiveInfo, Arc<HostError>>;
#[derive(Default)]
struct Metadata {
    value: Mutex<Option<ArchiveMetadata>>,
    changed: Notify,
}
impl Metadata {
    fn publish(&self, value: ArchiveInfo) {
        *lock(&self.value) = Some(Ok(value));
        self.changed.notify_waiters();
    }
    fn failed(&self, error: Arc<HostError>) {
        let mut value = lock(&self.value);
        if value.is_none() {
            *value = Some(Err(error));
        }
        drop(value);
        self.changed.notify_waiters();
    }
    async fn archive(
        &self,
        incoming: &CancellationToken,
        cancel: &CancellationToken,
    ) -> Result<ArchiveInfo> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let value = lock(&self.value).clone();
            if let Some(value) = value {
                return value.map_err(|error| captures::copy_error(&error));
            }
            tokio::select! { biased;
                () = incoming.cancelled() => return Err(HostError::Cancelled),
                () = cancel.cancelled() => return Err(stale()),
                () = changed => {},
            }
        }
    }
}

struct Reader {
    receive: mpsc::Receiver<Result<Packet>>,
    next: u64,
    last: Option<(u64, Page)>,
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
    request: Query,
    observed_from: Time,
    last_use: AtomicI64,
    active: AtomicUsize,
    seen: Arc<AtomicU64>,
    cancel: CancellationToken,
    metadata: Arc<Metadata>,
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
                HostError::Internal(format!("the live reader worker failed: {error}"))
            })?;
        }
        Ok(())
    }
    fn arm_idle(self: &Arc<Self>, cache: &Arc<DirectoryPages>) {
        let session = Arc::downgrade(self);
        let cache = Arc::downgrade(cache);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            loop {
                let remaining = {
                    let (Some(session), Some(cache)) = (session.upgrade(), cache.upgrade()) else {
                        return;
                    };
                    let idle = cache
                        .clock
                        .now()
                        .timestamp_millis()
                        .saturating_sub(session.last_use.load(Ordering::Acquire));
                    let maximum = cache
                        .policy(session.binding.environment)
                        .directory_idle_secs
                        .saturating_mul(1000);
                    if session.active.load(Ordering::Acquire) > 0 {
                        maximum
                    } else {
                        maximum.saturating_sub(u64::try_from(idle).unwrap_or(0))
                    }
                };
                if remaining == 0 {
                    cancel.cancel();
                    return;
                }
                let Some(deadline) =
                    std::time::Instant::now().checked_add(Duration::from_millis(remaining))
                else {
                    cancel.cancelled().await;
                    return;
                };
                tokio::select! { biased; () = cancel.cancelled() => return, () = tokio::time::sleep_until(deadline.into()) => {} }
            }
        });
    }
}

struct InUse {
    session: Arc<Session>,
    clock: Arc<dyn orch_types::Clock>,
}
impl Drop for InUse {
    fn drop(&mut self) {
        self.session
            .last_use
            .store(self.clock.now().timestamp_millis(), Ordering::Release);
        self.session.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn completed_candidate(reads: &std::collections::HashMap<Id, Arc<Session>>) -> Option<Id> {
    reads
        .values()
        .filter(|read| {
            read.active.load(Ordering::Acquire) == 0
                && Arc::strong_count(read) == 1
                && read.reader.try_lock().is_ok_and(|reader| reader.complete)
        })
        .min_by_key(|read| read.last_use.load(Ordering::Acquire))
        .map(|read| read.id)
}

impl DirectoryPages {
    pub(crate) async fn search(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        request: FsSearch,
        index: Arc<Index>,
    ) -> Result<FsSearchResult> {
        match self
            .read_query(binding, files, Query::Files(request), None, Some(index))
            .await?
        {
            Page::Files(result) => Ok(result),
            _ => Err(HostError::Internal(
                "the live reader returned another query kind".to_owned(),
            )),
        }
    }
    pub(crate) async fn archive(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        request: FsArchiveList,
    ) -> Result<ArchiveListing> {
        match self
            .read_query(binding, files, Query::Archive(request), None, None)
            .await?
        {
            Page::Archive(result) => Ok(result),
            _ => Err(HostError::Internal(
                "the live reader returned another query kind".to_owned(),
            )),
        }
    }
    pub(crate) async fn difference(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        tool: GitTool,
        request: GitDiff,
    ) -> Result<GitDiffResult> {
        match self
            .read_query(binding, files, Query::Difference(request), Some(tool), None)
            .await?
        {
            Page::Difference(result) => Ok(result),
            _ => Err(HostError::Internal(
                "the live reader returned another query kind".to_owned(),
            )),
        }
    }

    async fn read_query(
        self: &Arc<Self>,
        binding: Binding,
        files: Files,
        mut request: Query,
        tool: Option<GitTool>,
        index: Option<Arc<Index>>,
    ) -> Result<Page> {
        let (cursor, limit, preferred_bytes) = request.normalize()?;
        let cursor = cursor
            .map(|text| {
                if text.len() > crate::paths::PATH_MAX_BYTES {
                    return Err(stale());
                }
                let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| stale())?;
                serde_json::from_slice::<Cursor>(&bytes).map_err(|_| stale())
            })
            .transpose()?;
        self.reap_searches().await?;
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
            let permit = self.operations.acquire()?;
            let charge = charged(
                self,
                std::mem::size_of::<Session>() + serde_json::to_vec(&request)?.len(),
            )?;
            let mut sessions = lock(&self.searches);
            while sessions.len() >= self.operations.maximum() {
                let id = completed_candidate(&sessions).ok_or_else(|| {
                    HostError::Busy("the host's live reader observations are in use".to_owned())
                })?;
                if let Some(read) = sessions.remove(&id) {
                    read.cancel();
                }
            }
            let (delivery, receive) = mpsc::channel(1);
            let cancel = CancellationToken::new();
            let seen = Arc::new(AtomicU64::new(0));
            let metadata = Arc::new(Metadata::default());
            let session = Arc::new(Session {
                id: new_id(),
                binding,
                request: request.clone(),
                observed_from: self.clock.time(),
                last_use: AtomicI64::new(self.clock.now().timestamp_millis()),
                active: AtomicUsize::new(0),
                seen: Arc::clone(&seen),
                cancel: cancel.clone(),
                metadata: Arc::clone(&metadata),
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
                let output = delivery.clone();
                let info = Arc::clone(&metadata);
                let outcome = match request {
                    Query::Difference(body) => {
                        let _permit = permit;
                        match tool {
                            Some(tool) => {
                                let path = body
                                    .path
                                    .as_deref()
                                    .map(|path| files.parse(path))
                                    .transpose();
                                match path {
                                    Ok(path) => {
                                        files
                                            .stream_git_diff(
                                                &tool,
                                                path.as_ref(),
                                                body.staged.unwrap_or(false),
                                                &token,
                                                |text| {
                                                    send_async(
                                                        &resources,
                                                        &output,
                                                        Hit::Difference(text),
                                                        &token,
                                                    )
                                                },
                                            )
                                            .await
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                            None => Err(HostError::Internal(
                                "a difference reader has no Git program".to_owned(),
                            )),
                        }
                    }
                    request => {
                        let worker = tokio::task::spawn_blocking(move || {
                            let _permit = permit;
                            match request {
                                Query::Files(request) => traverse(
                                    &resources,
                                    &files,
                                    &request,
                                    &seen,
                                    &token,
                                    &output,
                                    index.as_ref().map(|index| (index.as_ref(), limit)),
                                ),
                                Query::Archive(request) => files.stream_archive(
                                    &files.parse(&request.path)?,
                                    request.inside.as_deref(),
                                    &token,
                                    &mut |metadata| {
                                        info.publish(metadata);
                                        Ok(())
                                    },
                                    &mut |entry| {
                                        seen.fetch_add(1, Ordering::AcqRel);
                                        if request.query.as_ref().is_some_and(|query| {
                                            !entry
                                                .path
                                                .rsplit('/')
                                                .next()
                                                .unwrap_or("")
                                                .to_lowercase()
                                                .contains(&query.to_lowercase())
                                        }) {
                                            return Ok(());
                                        }
                                        send(&resources, &output, Hit::Archive(entry), &token)
                                    },
                                ),
                                Query::Difference(_) => Err(HostError::Internal(
                                    "a difference entered the blocking reader".to_owned(),
                                )),
                            }
                        });
                        worker.await.unwrap_or_else(|error| {
                            Err(HostError::Internal(format!(
                                "the live reader worker failed: {error}"
                            )))
                        })
                    }
                };
                if let Err(error) = outcome {
                    let error = Arc::new(error);
                    metadata.failed(Arc::clone(&error));
                    let error = captures::copy_error(&error);
                    tokio::select! { biased; () = cancel.cancelled() => {}, _ = delivery.send(Err(error)) => {} }
                }
            });
            *lock(&session.driver) = Some(driver);
            session.arm_idle(self);
            sessions.insert(session.id, Arc::clone(&session));
            (session, 0)
        };
        session
            .last_use
            .store(self.clock.now().timestamp_millis(), Ordering::Release);
        session.active.fetch_add(1, Ordering::AcqRel);
        let _active = InUse {
            session: Arc::clone(&session),
            clock: Arc::clone(&self.clock),
        };
        let incoming = crate::error::cancellation().unwrap_or_default();
        let mut reader = tokio::select! { biased;
            () = incoming.cancelled() => { if page == 0 { session.cancel(); } return Err(HostError::Cancelled); },
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
        let archive = if matches!(session.request, Query::Archive(_)) {
            match session.metadata.archive(&incoming, &session.cancel).await {
                Ok(info) => Some(info),
                Err(error) => {
                    if page == 0 {
                        session.cancel();
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let policy = self.policy(session.binding.environment);
        let byte_default = if matches!(session.request, Query::Difference(_)) {
            policy.git_diff_bytes
        } else {
            policy.directory_page_bytes
        };
        let window = usize::try_from(preferred_bytes.unwrap_or(byte_default))
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
                    () = incoming.cancelled() => { if page == 0 { session.cancel(); } return Err(HostError::Cancelled); },
                    () = session.cancel.cancelled() => return Err(stale()),
                    () = &mut timer => break,
                    hit = reader.receive.recv() => hit,
                },
            };
            match hit {
                None => reader.complete = true,
                Some(Err(error)) => reader.failure = Some(Arc::new(error)),
                Some(Ok(mut hit)) => {
                    let available = window.saturating_sub(reader.bytes);
                    if !reader.building.is_empty() && hit.hit.bytes()? + 1 > available {
                        reader.pending = Some(hit);
                        break;
                    }
                    if let Hit::Difference(text) = &mut hit.hit {
                        let at = text_boundary(text, available.max(2));
                        if at < text.len() {
                            let rest = text.split_off(at);
                            match packet(self, Hit::Difference(rest)) {
                                Ok(rest) => reader.pending = Some(rest),
                                Err((error, rest)) => {
                                    if let Hit::Difference(rest) = *rest {
                                        text.push_str(&rest);
                                    }
                                    reader.pending = Some(hit);
                                    return Err(error);
                                }
                            }
                        }
                    }
                    let bytes = hit.hit.bytes()?.saturating_add(1);
                    if bytes + 2 > ENTRY_BYTES {
                        return Err(super::too_large(bytes));
                    }
                    reader.bytes += bytes;
                    reader.building.push(hit);
                    if reader.pending.is_some() {
                        break;
                    }
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
        let mut entries = Vec::new();
        let mut text = String::new();
        for packet in std::mem::take(&mut reader.building) {
            match packet.hit {
                Hit::Name(entry) => names.push(entry),
                Hit::Line(entry) => matches.push(entry),
                Hit::Archive(entry) => entries.push(entry),
                Hit::Difference(chunk) => text.push_str(&chunk),
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
        let result = match &session.request {
            Query::Files(request) => Page::Files(FsSearchResult {
                names: (request.scope == FileSearchIn::Names).then_some(names),
                matches,
                truncated: next_cursor.is_some(),
                next_cursor,
                observed_from: Some(session.observed_from),
                as_of: Some(self.clock.time()),
                searched: i64::try_from(session.seen.load(Ordering::Acquire)).unwrap_or(i64::MAX),
            }),
            Query::Archive(_) => {
                let info = archive.ok_or_else(|| {
                    HostError::Internal("the archive reader has no format".to_owned())
                })?;
                Page::Archive(ArchiveListing {
                    format: info.format,
                    compression: info.compression,
                    encrypted: info.encrypted,
                    entries,
                    truncated: next_cursor.is_some(),
                    next_cursor,
                })
            }
            Query::Difference(_) => Page::Difference(GitDiffResult {
                text,
                truncated: next_cursor.is_some(),
                next_cursor,
            }),
        };
        reader.next = next;
        reader.last = Some((page, result.clone()));
        Ok(result)
    }

    pub(super) fn evict_completed_reader(&self) -> bool {
        let removed = {
            let mut reads = lock(&self.searches);
            completed_candidate(&reads).and_then(|id| reads.remove(&id))
        };
        if let Some(read) = removed {
            read.cancel();
            true
        } else {
            false
        }
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
            if session.active.load(Ordering::Acquire) == 0
                && idle >= i64::try_from(maximum).unwrap_or(i64::MAX)
            {
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

fn text_boundary(text: &str, maximum: usize) -> usize {
    let mut low = 0;
    let mut high = text.len().min(maximum);
    while high > 0 && !text.is_char_boundary(high) {
        high -= 1;
    }
    while low < high {
        let mut at = (low + high).div_ceil(2);
        while at < high && !text.is_char_boundary(at) {
            at += 1;
        }
        if serde_json::to_vec(&text[..at]).is_ok_and(|encoded| encoded.len() <= maximum) {
            low = at;
        } else {
            high = at.saturating_sub(1);
            while high > low && !text.is_char_boundary(high) {
                high -= 1;
            }
        }
    }
    if low == 0 {
        text.chars().next().map_or(0, char::len_utf8)
    } else {
        low
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
fn charged(cache: &DirectoryPages, bytes: usize) -> Result<Charge> {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    cache.reserve(bytes)?;
    Ok(Charge {
        ledger: Arc::clone(&cache.ledger),
        bytes,
    })
}
fn packet(cache: &DirectoryPages, hit: Hit) -> std::result::Result<Packet, (HostError, Box<Hit>)> {
    let bytes = match hit.bytes() {
        Ok(bytes) => bytes.saturating_add(std::mem::size_of::<Packet>()),
        Err(error) => return Err((error, Box::new(hit))),
    };
    match charged(cache, bytes) {
        Ok(charge) => Ok(Packet {
            hit,
            _charge: charge,
        }),
        Err(error) => Err((error, Box::new(hit))),
    }
}
async fn send_async(
    resources: &Resources,
    output: &mpsc::Sender<Result<Packet>>,
    hit: Hit,
    cancel: &CancellationToken,
) -> Result<()> {
    let packet = {
        let cache = resources.cache()?;
        packet(&cache, hit).map_err(|(error, _)| error)?
    };
    tokio::select! { biased; () = cancel.cancelled() => Err(HostError::Cancelled), result = output.send(Ok(packet)) => result.map_err(|_| HostError::Cancelled) }
}
fn send(
    resources: &Resources,
    output: &mpsc::Sender<Result<Packet>>,
    hit: Hit,
    cancel: &CancellationToken,
) -> Result<()> {
    tokio::runtime::Handle::current().block_on(send_async(resources, output, hit, cancel))
}

fn traverse(
    resources: &Resources,
    files: &Files,
    request: &FsSearch,
    seen: &AtomicU64,
    cancel: &CancellationToken,
    output: &mpsc::Sender<Result<Packet>>,
    hints: Option<(&Index, u32)>,
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
    let mut hinted = std::collections::HashSet::new();
    let mut hint_credit = Vec::new();
    if !content && let Some((index, rows)) = hints {
        for path in index.matching_paths(&start, &request.query, request.case, rows) {
            if cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }
            let Ok(current) = files.stat(&path) else {
                continue;
            };
            let credit = {
                let cache = resources.cache()?;
                charged(
                    &cache,
                    path.as_str().len()
                        + std::mem::size_of::<RelPath>()
                        + 2 * std::mem::size_of::<usize>(),
                )?
            };
            hint_credit.push(credit);
            hinted.insert(path);
            seen.fetch_add(1, Ordering::AcqRel);
            send(resources, output, Hit::Name(current.entry), cancel)?;
        }
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
            if !hinted.contains(&path) && matched(&entry.path) {
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
