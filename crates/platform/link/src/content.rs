//! Resumable authenticated HTTPS content transfer.
//!
//! Content of any size does not travel in the session: a machine moves it
//! over HTTPS, to the same host, beside it. Every request belongs to a
//! command that is open for that machine, and carries its identifier. On a
//! pinned endpoint the machine is known by its client certificate; on a
//! public one it presents the token of its session.
//!
//! A small initial piece measures data rate separately from the natural metadata round trip.
//! Files then share a bounded bandwidth-delay window, with concurrent requests for disjoint ranges.
//! A lost answer is reconciled against durable server coverage; a lost middle piece is retried
//! independently while healthy pieces continue. Downloads retain their byte credits until the
//! contiguous native writes finish, then verify the complete hash.
//!
//! The HTTP itself is the business of an implementation of [`BlobEndpoint`]:
//! a client over the machine's TLS configuration in a connector, a double in
//! a test. What is here is the rest: how big each piece is, where to go on
//! from, what to do when a piece stalls or is refused, and the checks that
//! make the end of a transfer trustworthy.

use crate::framing::LinkMeter;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use orch_api_contract::threads::UploadRange;
use orch_link_contract::content::TransferPolicy;
use orch_link_contract::content::{
    BlobProgress, COMMAND_HEADER, PutQuery, STALL_SECS, TransferStatus, authorization,
};
use orch_types::{Hash, Id};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// Why a piece of an upload was not taken.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PutRefusal {
    /// A retry conflicts with acknowledged content; the server has this much.
    #[error("the piece conflicts with acknowledged content ({received} bytes)")]
    Offset {
        /// How many bytes the server has.
        received: i64,
    },
    /// No socket data moved for the transport silence allowance.
    #[error("the file request stopped moving")]
    Silent,
    /// The whole did not have the hash or size it was announced with, and
    /// what arrived is thrown away.
    #[error("the content did not have the announced hash and size")]
    HashMismatch,
    /// The server will not take it, whatever is tried.
    #[error("refused: {0}")]
    Refused(String),
    /// The request failed on the way; it may be tried again.
    #[error("the request failed: {0}")]
    Failed(String),
}

/// Why a transfer did not complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentError {
    /// The server will not take or give the content.
    #[error("refused: {0}")]
    Refused(String),
    /// The content did not have the hash or size it was announced with.
    #[error("the content did not have the announced hash and size")]
    HashMismatch,
    /// Nothing moved for as long as a transfer waits, as often as it was
    /// tried.
    #[error("the transfer stood still")]
    StoodStill,
    /// The local side could not read or write.
    #[error("the local side failed: {0}")]
    Local(String),
}

/// One request to a command-bound memory relay, separate from hash-addressed stored content.
#[derive(Clone)]
pub enum RelayRequest {
    /// The destination reports the prefix it has durably written.
    Stand { received: i64 },
    /// The destination asks for its next piece.
    Pull { offset: i64, len: i64 },
    /// The source offers a piece for a browser response.
    Push { offset: i64, bytes: Bytes },
}

/// The bounded answer to a memory relay request.
pub enum RelayReply {
    /// A prefix acknowledged by the destination.
    Progress(i64),
    /// A piece for the destination to write.
    Piece(Bytes),
    /// No piece was available during this poll.
    Idle,
    /// The source completed the transfer.
    End,
}

/// The requests of content transfer as a machine makes them. Every one names
/// the command it is made for.
#[async_trait]
pub trait BlobEndpoint: Send + Sync {
    /// Moves a piece through memory, without granting access to any stored blob.
    /// Endpoints used only as stored-content stand-ins need not implement a memory transport.
    async fn relay(
        &self,
        _command: Id,
        _relay: Id,
        _request: RelayRequest,
        _within: Duration,
    ) -> Result<RelayReply, PutRefusal> {
        Err(PutRefusal::Refused(
            "this endpoint does not carry memory relays".into(),
        ))
    }

    /// Accepted policy and durable ranges; a new upload has empty coverage. Metadata has a finite timeout.
    async fn status(&self, command: Id, hash: &Hash) -> Result<TransferStatus, PutRefusal>;

    /// `HEAD`: how much of the content the server has, or `None` if it knows
    /// nothing of it.
    async fn head(&self, command: Id, hash: &Hash) -> Result<Option<BlobProgress>, PutRefusal>;

    /// `PUT`: one piece of an upload.
    async fn put(
        &self,
        command: Id,
        hash: &Hash,
        query: PutQuery,
        body: Bytes,
    ) -> Result<BlobProgress, PutRefusal>;

    /// `GET` with a range: `len` bytes from `offset`.
    async fn get(
        &self,
        command: Id,
        hash: &Hash,
        offset: i64,
        len: i64,
    ) -> Result<Bytes, PutRefusal>;

    /// `POST /blobs/missing`: which of the hashes the server does not hold.
    async fn missing(&self, command: Id, hashes: &[Hash]) -> Result<Vec<Hash>, PutRefusal>;
}

/// Where the bytes of an upload come from.
#[async_trait]
pub trait BlobSource: Send {
    /// Distinct bytes confirmed by the receiver, including durable ranges of a resumed upload.
    fn accepted(&mut self, _received: i64) {}
    /// Up to `len` bytes from `offset`.
    async fn read(&mut self, offset: i64, len: i64) -> std::io::Result<Bytes>;
}

/// Where the bytes of a download go.
#[async_trait]
pub trait BlobSink: Send {
    /// How many bytes from the first are held already.
    async fn received(&mut self) -> std::io::Result<i64>;
    /// Holds bytes that begin at `offset`, which is what `received` said.
    async fn write(&mut self, offset: i64, bytes: Bytes) -> std::io::Result<()>;
    /// The hash of everything held.
    async fn hash(&mut self) -> std::io::Result<Hash>;
    /// Throws everything held away.
    async fn discard(&mut self) -> std::io::Result<()>;
}

/// The headers a machine sends with a request for content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The command the request is made for.
    pub command: Id,
    /// The session token, where the connection shows no client certificate.
    pub session_token: Option<String>,
}

impl Credentials {
    /// The headers, name and value.
    #[must_use]
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![(COMMAND_HEADER, self.command.to_string())];
        if let Some(token) = &self.session_token {
            headers.push(("authorization", authorization(token)));
        }
        headers
    }
}

/// How a transfer behaves.
#[derive(Debug, Clone, Copy)]
pub struct TransferOptions {
    /// How long a request may go without a byte moving.
    pub stall: Duration,
    /// How many times in a row a piece may stand still before the transfer
    /// is given up.
    pub stalls_allowed: u32,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            stall: Duration::from_secs(u64::from(STALL_SECS)),
            stalls_allowed: 3,
        }
    }
}

/// Moves content for a machine, within its limit of transfers at once.
#[derive(Clone)]
pub struct Transfers {
    endpoint: Arc<dyn BlobEndpoint>,
    meter: Arc<LinkMeter>,
    options: TransferOptions,
    window: Arc<Window>,
}

impl std::fmt::Debug for Transfers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transfers").finish_non_exhaustive()
    }
}

/// What a finished piece of work achieved, for a caller that wants to see
/// how the transfer went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcome {
    /// How many requests carried bytes.
    pub requests: u32,
    /// The size of the largest piece.
    pub largest_piece: i64,
    /// The size of the smallest piece.
    pub smallest_piece: i64,
    /// How many bytes were sent or fetched that had been sent or fetched
    /// before and were lost to a failure.
    pub lost_bytes: i64,
    /// How many times a piece failed or stood still and was tried again.
    pub retries: u32,
}

impl Transfers {
    /// Transfers over `endpoint`, whose rates are noted in `meter`.
    #[must_use]
    pub fn new(
        endpoint: Arc<dyn BlobEndpoint>,
        meter: Arc<LinkMeter>,
        options: TransferOptions,
    ) -> Self {
        Self {
            endpoint,
            meter,
            options,
            window: Arc::new(Window::default()),
        }
    }

    /// Which of `hashes` the server lacks, in the groups the server takes.
    pub async fn missing(&self, command: Id, hashes: &[Hash]) -> Result<Vec<Hash>, ContentError> {
        let mut lacking = Vec::new();
        for group in hashes.chunks(orch_link_contract::content::MAX_MISSING_HASHES) {
            let answer = self
                .endpoint
                .missing(command, group)
                .await
                .map_err(|e| ContentError::Refused(e.to_string()))?;
            lacking.extend(answer);
        }
        Ok(lacking)
    }

    /// Uploads durable ranges concurrently. A failed request consults coverage and retries only itself.
    pub async fn upload(
        &self,
        command: Id,
        hash: Hash,
        size: i64,
        source: &mut dyn BlobSource,
    ) -> Result<Outcome, ContentError> {
        let status = self.status(command, hash).await?;
        let policy = status.policy;
        let turn = self.window.turn();
        let mut held = status.ranges;
        validate_ranges(&held, size)?;
        normalize(&mut held);
        if status.progress.complete {
            if status.progress.received != size {
                return Err(ContentError::Refused(
                    "stored content has a different size".into(),
                ));
            }
            source.accepted(size);
            return Ok(Outcome::default());
        }
        source.accepted(held.iter().map(|range| range.len).sum());
        let mut pending: FuturesUnordered<BoxFuture<'_, Result<Piece, ContentError>>> =
            FuturesUnordered::new();
        let mut reserved = Vec::new();
        let mut outcome = Outcome::default();
        let mut empty_sent = false;
        let mut quiet = std::collections::HashMap::new();
        loop {
            let changed = self.window.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            while size == 0 && !empty_sent
                || first_missing(&held, &reserved, size, policy.max_piece_bytes).is_some()
            {
                let Some(mut lease) = self.window.reserve(policy, true, turn.id) else {
                    break;
                };
                let range = if size == 0 && !empty_sent {
                    empty_sent = true;
                    Some(UploadRange { start: 0, len: 0 })
                } else {
                    first_missing(&held, &reserved, size, lease.bytes)
                };
                let Some(range) = range else {
                    break;
                };
                lease.resize(range.len);
                let body = source
                    .read(range.start, range.len)
                    .await
                    .map_err(|e| ContentError::Local(e.to_string()))?;
                if i64::try_from(body.len()).unwrap_or(i64::MAX) != range.len {
                    return Err(ContentError::Local(
                        "the source ended before its announced size".into(),
                    ));
                }
                reserved.push(range);
                let endpoint = Arc::clone(&self.endpoint);
                let stalls = self.options.stalls_allowed;
                let metadata_timeout = self.options.stall;
                pending.push(Box::pin(async move {
                    let mut outcome = Outcome::default();
                    loop {
                        outcome.record(range.len);
                        match endpoint
                            .put(
                                command,
                                &hash,
                                PutQuery {
                                    offset: range.start,
                                    size,
                                },
                                body.clone(),
                            )
                            .await
                        {
                            Ok(progress) => {
                                if progress.received < range.len || progress.received > size {
                                    return Err(ContentError::Refused(
                                        "invalid received count".into(),
                                    ));
                                }
                                if progress.complete && progress.received != size {
                                    return Err(ContentError::Refused(
                                        "invalid completed size".into(),
                                    ));
                                }
                                lease.observe(range.len);
                                return Ok(Piece {
                                    accepted: true,
                                    complete: progress.complete,
                                    range,
                                    bytes: Bytes::new(),
                                    _lease: lease,
                                    outcome,
                                });
                            }
                            Err(PutRefusal::HashMismatch) => {
                                return Err(ContentError::HashMismatch);
                            }
                            Err(PutRefusal::Refused(why)) => {
                                return Err(ContentError::Refused(why));
                            }
                            Err(PutRefusal::Offset { received }) => {
                                return Err(ContentError::Refused(format!(
                                    "the piece conflicts with {received} acknowledged bytes"
                                )));
                            }
                            Err(fault) => {
                                let silent = matches!(fault, PutRefusal::Silent);
                                outcome.retries += 1;
                                if outcome.retries > stalls {
                                    return Err(ContentError::StoodStill);
                                }
                                // Keep this piece's credit while healthy requests continue. A lost answer needs no resend.
                                match tokio::time::timeout(
                                    metadata_timeout,
                                    endpoint.status(command, &hash),
                                )
                                .await
                                {
                                    Ok(Ok(status)) => {
                                        validate_ranges(&status.ranges, size)?;
                                        if status.progress.complete
                                            && status.progress.received != size
                                        {
                                            return Err(ContentError::Refused(
                                                "invalid completed size".into(),
                                            ));
                                        }
                                        if status.progress.complete || covers(&status.ranges, range)
                                        {
                                            lease.observe(range.len);
                                            return Ok(Piece {
                                                accepted: true,
                                                complete: status.progress.complete,
                                                range,
                                                bytes: Bytes::new(),
                                                _lease: lease,
                                                outcome,
                                            });
                                        }
                                    }
                                    Ok(Err(PutRefusal::Refused(why))) => {
                                        return Err(ContentError::Refused(why));
                                    }
                                    Ok(Err(PutRefusal::HashMismatch)) => {
                                        return Err(ContentError::HashMismatch);
                                    }
                                    _ => {}
                                }
                                outcome.lost_bytes += range.len;
                                if silent {
                                    lease.restart();
                                    return Ok(Piece {
                                        accepted: false,
                                        complete: false,
                                        range,
                                        bytes: Bytes::new(),
                                        _lease: lease,
                                        outcome,
                                    });
                                }
                            }
                        }
                    }
                }));
            }
            if first_missing(&held, &reserved, size, size).is_none() {
                turn.unqueue();
            }
            if pending.is_empty() {
                if size == 0 && empty_sent || first_missing(&held, &[], size, size).is_none() {
                    return Ok(outcome);
                }
                changed.await;
                continue;
            }
            tokio::select! {
                piece = pending.next() => {
                    let piece = piece.ok_or(ContentError::StoodStill)??;
                    reserved.retain(|range| *range != piece.range);
                    if piece.complete {held=if size==0 {Vec::new()} else {vec![UploadRange {start:0,len:size}]};}
                    else if piece.accepted {held.push(piece.range);normalize(&mut held);}
                    else {
                        let count=quiet.entry((piece.range.start,piece.range.len)).or_insert(0u32);*count+=1;
                        if *count>self.options.stalls_allowed {return Err(ContentError::StoodStill);}
                    }
                    source.accepted(held.iter().map(|range|range.len).sum());
                    if piece.accepted {self.meter.observe_sent(usize::try_from(piece.range.len).unwrap_or(usize::MAX), Instant::now());}
                    outcome.join(piece.outcome);
                    let complete=piece.complete;
                    drop(piece);
                    if complete {return Ok(outcome);}
                }
                () = &mut changed => {}
            }
        }
    }

    async fn status(&self, command: Id, hash: Hash) -> Result<TransferStatus, ContentError> {
        let started = Instant::now();
        let status = tokio::time::timeout(self.options.stall, self.endpoint.status(command, &hash))
            .await
            .map_err(|_| ContentError::StoodStill)?
            .map_err(content_error)?;
        if !status.policy.valid() {
            return Err(ContentError::Refused("invalid transfer policy".into()));
        }
        self.window.round_trip(started.elapsed().as_secs_f64());
        Ok(status)
    }

    /// Records a completed piece for adaptive sizing and the shared link measurements.
    pub fn note(&self, rate: &mut Option<i64>, bytes: i64, started: Instant, up: bool) {
        let seconds = started.elapsed().as_secs_f64().max(0.001);
        // A piece's rate is bytes over the time it took; an average of the
        // last few steers the next piece.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            reason = "a rate in bytes a second is far below the range of either type"
        )]
        let measured = (bytes as f64 / seconds) as i64;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            reason = "as above"
        )]
        let blended = match *rate {
            Some(before) => (before as f64 + 0.5 * (measured as f64 - before as f64)) as i64,
            None => measured,
        };
        *rate = Some(blended);
        let now = Instant::now();
        let usize_bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
        if up {
            self.meter.observe_sent(usize_bytes, now);
        } else {
            self.meter.observe_received(usize_bytes, now);
        }
    }

    /// Fetches a window of ranges; completed suffixes stay charged until contiguous writes finish.
    pub async fn download(
        &self,
        command: Id,
        hash: Hash,
        size: i64,
        sink: &mut dyn BlobSink,
    ) -> Result<Outcome, ContentError> {
        let policy = self.status(command, hash).await?.policy;
        let turn = self.window.turn();
        let local = |e: std::io::Error| ContentError::Local(e.to_string());
        let mut offset = sink.received().await.map_err(local)?;
        if offset < 0 || offset > size {
            return Err(ContentError::Local(
                "the download prefix has an invalid size".into(),
            ));
        }
        let mut pending: FuturesUnordered<BoxFuture<'_, Result<Piece, ContentError>>> =
            FuturesUnordered::new();
        let mut reserved = Vec::new();
        let mut ready = std::collections::BTreeMap::new();
        let mut quiet = std::collections::HashMap::new();
        let mut outcome = Outcome::default();
        if size == 0 {
            let empty = self
                .endpoint
                .get(command, &hash, 0, 0)
                .await
                .map_err(content_error)?;
            if !empty.is_empty() {
                return Err(ContentError::Refused(
                    "empty content returned file bytes".into(),
                ));
            }
            sink.write(0, Bytes::new()).await.map_err(local)?;
            outcome.record(0);
        }
        while offset < size {
            let changed = self.window.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            while first_missing(
                &[UploadRange {
                    start: 0,
                    len: offset,
                }],
                &reserved,
                size,
                policy.max_piece_bytes,
            )
            .is_some()
            {
                let Some(mut lease) = self.window.reserve(policy, false, turn.id) else {
                    break;
                };
                let Some(range) = first_missing(
                    &[UploadRange {
                        start: 0,
                        len: offset,
                    }],
                    &reserved,
                    size,
                    lease.bytes,
                ) else {
                    break;
                };
                lease.resize(range.len);
                reserved.push(range);
                let endpoint = Arc::clone(&self.endpoint);
                let stalls = self.options.stalls_allowed;
                pending.push(Box::pin(async move {
                    let mut outcome = Outcome::default();
                    loop {
                        outcome.record(range.len);
                        match endpoint.get(command, &hash, range.start, range.len).await {
                            Ok(bytes)
                                if i64::try_from(bytes.len()).unwrap_or(i64::MAX) == range.len =>
                            {
                                lease.observe(range.len);
                                lease.buffered();
                                return Ok(Piece {
                                    accepted: true,
                                    complete: false,
                                    range,
                                    bytes,
                                    _lease: lease,
                                    outcome,
                                });
                            }
                            Err(PutRefusal::Refused(why)) => {
                                return Err(ContentError::Refused(why));
                            }
                            Err(PutRefusal::HashMismatch) => {
                                return Err(ContentError::HashMismatch);
                            }
                            Err(PutRefusal::Silent) => {
                                outcome.retries += 1;
                                outcome.lost_bytes += range.len;
                                lease.restart();
                                return Ok(Piece {
                                    accepted: false,
                                    complete: false,
                                    range,
                                    bytes: Bytes::new(),
                                    _lease: lease,
                                    outcome,
                                });
                            }
                            _ => {
                                outcome.retries += 1;
                                outcome.lost_bytes += range.len;
                                if outcome.retries > stalls {
                                    return Err(ContentError::StoodStill);
                                }
                            }
                        }
                    }
                }));
            }
            if first_missing(
                &[UploadRange {
                    start: 0,
                    len: offset,
                }],
                &reserved,
                size,
                size,
            )
            .is_none()
            {
                turn.unqueue();
            }
            if pending.is_empty() {
                changed.await;
                continue;
            }
            tokio::select! {
                piece=pending.next() => {
                    let piece=piece.ok_or(ContentError::StoodStill)??;
                    if !piece.accepted {
                        reserved.retain(|range|*range!=piece.range);
                        let count=quiet.entry((piece.range.start,piece.range.len)).or_insert(0u32);*count+=1;
                        if *count>self.options.stalls_allowed {return Err(ContentError::StoodStill);}
                        outcome.join(piece.outcome);drop(piece);continue;
                    }
                    ready.insert(piece.range.start,piece);
                    while let Some(piece)=ready.remove(&offset) {
                        sink.write(offset,piece.bytes.clone()).await.map_err(local)?;
                        offset+=piece.range.len;
                        reserved.retain(|range|*range!=piece.range);
                        self.meter.observe_received(usize::try_from(piece.range.len).unwrap_or(usize::MAX),Instant::now());
                        outcome.join(piece.outcome);
                        drop(piece); // Data stays charged through its final native write.
                    }
                }
                ()=&mut changed=>{}
            }
        }
        if sink.hash().await.map_err(local)? != hash {
            sink.discard().await.map_err(local)?;
            return Err(ContentError::HashMismatch);
        }
        Ok(outcome)
    }
}

fn content_error(error: PutRefusal) -> ContentError {
    match error {
        PutRefusal::HashMismatch => ContentError::HashMismatch,
        PutRefusal::Refused(why) => ContentError::Refused(why),
        _ => ContentError::StoodStill,
    }
}
fn validate_ranges(ranges: &[UploadRange], size: i64) -> Result<(), ContentError> {
    if size < 0
        || ranges.iter().any(|r| {
            r.start < 0 || r.len < 0 || r.start.checked_add(r.len).is_none_or(|end| end > size)
        })
    {
        return Err(ContentError::Refused("invalid durable ranges".into()));
    }
    Ok(())
}
fn normalize(ranges: &mut Vec<UploadRange>) {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut normalized: Vec<UploadRange> = Vec::new();
    for range in ranges.iter().copied() {
        if let Some(last) = normalized.last_mut()
            && last.start + last.len >= range.start
        {
            last.len = last.len.max(range.start + range.len - last.start);
        } else {
            normalized.push(range);
        }
    }
    *ranges = normalized;
}
fn covers(ranges: &[UploadRange], needed: UploadRange) -> bool {
    ranges
        .iter()
        .any(|r| r.start <= needed.start && r.start + r.len >= needed.start + needed.len)
}
fn first_missing(
    held: &[UploadRange],
    reserved: &[UploadRange],
    size: i64,
    maximum: i64,
) -> Option<UploadRange> {
    let mut covered = held.to_vec();
    covered.extend_from_slice(reserved);
    normalize(&mut covered);
    let mut start = 0;
    for range in covered {
        if start < range.start {
            return Some(UploadRange {
                start,
                len: maximum.min(range.start - start),
            });
        }
        start = start.max(range.start + range.len);
    }
    (start < size).then(|| UploadRange {
        start,
        len: maximum.min(size - start),
    })
}
impl Outcome {
    fn record(&mut self, bytes: i64) {
        self.requests += 1;
        self.largest_piece = self.largest_piece.max(bytes);
        self.smallest_piece = if self.requests == 1 {
            bytes
        } else {
            self.smallest_piece.min(bytes)
        };
    }
    fn join(&mut self, other: Self) {
        if self.requests == 0 {
            self.smallest_piece = other.smallest_piece;
        } else {
            self.smallest_piece = self.smallest_piece.min(other.smallest_piece);
        }
        self.requests += other.requests;
        self.largest_piece = self.largest_piece.max(other.largest_piece);
        self.retries += other.retries;
        self.lost_bytes += other.lost_bytes;
    }
}
struct Piece {
    accepted: bool,
    complete: bool,
    range: UploadRange,
    bytes: Bytes,
    _lease: Lease,
    outcome: Outcome,
}

#[derive(Default)]
struct Window {
    state: Mutex<WindowState>,
    changed: Notify,
}
struct Credit {
    id: u64,
    bytes: i64,
    policy: TransferPolicy,
    up: bool,
    wire: bool,
}
#[derive(Default)]
struct WindowState {
    active: Vec<Credit>,
    next: u64,
    waiting: std::collections::VecDeque<u64>,
    directions: [Direction; 2],
    rtt: f64,
}
#[derive(Default)]
struct Direction {
    rate: f64,
    epoch: Option<Instant>,
    last_ack: Option<Instant>,
    burst_base: Option<Instant>,
    burst_bytes: i64,
}
impl Window {
    fn lock(&self) -> MutexGuard<'_, WindowState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn round_trip(&self, seconds: f64) {
        let mut state = self.lock();
        state.rtt = seconds;
    }
    fn turn(self: &Arc<Self>) -> Turn {
        let mut state = self.lock();
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        Turn {
            window: Arc::clone(self),
            id,
        }
    }
    fn reserve(self: &Arc<Self>, policy: TransferPolicy, up: bool, turn: u64) -> Option<Lease> {
        let mut state = self.lock();
        let direction = usize::from(!up);
        let rate = state.directions[direction].rate;
        // One small measurement per direction, while the other direction may still make progress.
        if rate <= 0.0
            && state
                .active
                .iter()
                .filter(|credit| credit.up == up && credit.wire)
                .count()
                >= usize::try_from(policy.initial_requests.min(policy.requests))
                    .unwrap_or(usize::MAX)
        {
            let before = state.waiting.len();
            state.waiting.retain(|id| *id != turn);
            let removed = before != state.waiting.len();
            drop(state);
            if removed {
                self.changed.notify_waiters();
            }
            return None;
        }
        if !state.waiting.contains(&turn) {
            state.waiting.push_back(turn);
        }
        if state.waiting.front().copied() != Some(turn) {
            return None;
        }
        let requests = state
            .active
            .iter()
            .fold(policy.requests, |n, credit| n.min(credit.policy.requests));
        let ceiling = state
            .active
            .iter()
            .fold(policy.max_window_bytes, |n, credit| {
                n.min(credit.policy.max_window_bytes)
            });
        let window = policy
            .window_bytes(rate, state.rtt)
            .min(ceiling)
            .max(policy.min_piece_bytes);
        let bytes: i64 = state.active.iter().map(|credit| credit.bytes).sum();
        let direction_bytes: i64 = state
            .active
            .iter()
            .filter(|credit| credit.up == up && credit.wire)
            .map(|credit| credit.bytes)
            .sum();
        let available = (window - direction_bytes).min(ceiling - bytes);
        if i64::try_from(state.active.iter().filter(|credit| credit.wire).count())
            .unwrap_or(i64::MAX)
            >= requests
            || available < policy.min_piece_bytes
        {
            return None;
        }
        let piece = policy.piece_bytes(rate, state.rtt).min(available);
        if !state
            .active
            .iter()
            .any(|credit| credit.up == up && credit.wire)
        {
            state.directions[direction].epoch = Some(Instant::now());
            state.directions[direction].last_ack = None;
            state.directions[direction].burst_base = None;
            state.directions[direction].burst_bytes = 0;
        }
        state.waiting.pop_front();
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        state.active.push(Credit {
            id,
            bytes: piece,
            policy,
            up,
            wire: true,
        });
        drop(state);
        self.changed.notify_waiters();
        Some(Lease {
            window: Arc::clone(self),
            id,
            bytes: piece,
            up,
        })
    }
}
struct Turn {
    window: Arc<Window>,
    id: u64,
}
impl Turn {
    fn unqueue(&self) {
        let changed = {
            let mut state = self.window.lock();
            let before = state.waiting.len();
            state.waiting.retain(|id| *id != self.id);
            before != state.waiting.len()
        };
        if changed {
            self.window.changed.notify_waiters();
        }
    }
}
impl Drop for Turn {
    fn drop(&mut self) {
        self.unqueue();
    }
}
struct Lease {
    window: Arc<Window>,
    id: u64,
    bytes: i64,
    up: bool,
}
impl Lease {
    fn resize(&mut self, bytes: i64) {
        let mut state = self.window.lock();
        if let Some(credit) = state.active.iter_mut().find(|credit| credit.id == self.id) {
            credit.bytes = bytes;
        }
        self.bytes = bytes;
    }
    fn restart(&self) {
        self.window.lock().directions[usize::from(!self.up)].rate = 0.0;
    }
    fn buffered(&self) {
        {
            let mut state = self.window.lock();
            if let Some(credit) = state.active.iter_mut().find(|credit| credit.id == self.id) {
                credit.wire = false;
            }
        }
        self.window.changed.notify_waiters();
    }
    fn observe(&self, bytes: i64) {
        let mut state = self.window.lock();
        let direction = &mut state.directions[usize::from(!self.up)];
        let now = Instant::now();
        // Group acknowledgements inside the same millisecond already used as measurement resolution.
        if direction
            .last_ack
            .is_some_and(|last| now - last < Duration::from_millis(1))
        {
            direction.burst_bytes += bytes;
        } else {
            direction.burst_base = direction.last_ack.or(direction.epoch);
            direction.burst_bytes = bytes;
            direction.last_ack = Some(now);
        }
        if let Some(before) = direction.burst_base {
            direction.rate = direction.burst_bytes as f64 / (now - before).as_secs_f64().max(0.001);
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.window
            .lock()
            .active
            .retain(|credit| credit.id != self.id);
        self.window.changed.notify_waiters();
    }
}
