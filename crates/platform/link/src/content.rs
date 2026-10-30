//! Resumable authenticated HTTPS content transfer.
//!
//! Content of any size does not travel in the session: a machine moves it
//! over HTTPS, to the same host, beside it. Every request belongs to a
//! command that is open for that machine, and carries its identifier. On a
//! pinned endpoint the machine is known by its client certificate; on a
//! public one it presents the token of its session.
//!
//! A transfer is sized to the link. It starts with a piece of a quarter of a
//! mebibyte, and sizes each next piece to about five seconds of what the
//! last ones achieved, between 64 KiB and 4 MiB; so a cut loses a few
//! seconds of work, not the transfer. After any failure the sender asks where
//! the other side stands and goes on from there. It is given up for standing
//! still, which is a request without a byte moving for a minute, and never
//! for being slow.
//!
//! The HTTP itself is the business of an implementation of [`BlobEndpoint`]:
//! a client over the machine's TLS configuration in a connector, a double in
//! a test. What is here is the rest: how big each piece is, where to go on
//! from, what to do when a piece stalls or is refused, and the checks that
//! make the end of a transfer trustworthy.

use crate::framing::LinkMeter;
use async_trait::async_trait;
use bytes::Bytes;
use orch_link_contract::content::{
    BlobProgress, COMMAND_HEADER, MAX_TRANSFERS_PER_MACHINE, PutQuery, STALL_SECS, authorization,
    chunk_bytes_after_stall, next_chunk_bytes,
};
use orch_types::{Hash, Id};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::Instant;

/// How much weight the newest piece has in the rate a transfer has seen: a
/// piece is a noisy measurement, and the last few say more than the last.
const RATE_WEIGHT: f64 = 0.5;

/// Why a piece of an upload was not taken.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PutRefusal {
    /// It does not begin where the last one ended; the server has this much.
    #[error("the piece does not begin where the server's copy ends ({received} bytes)")]
    Offset {
        /// How many bytes the server has.
        received: i64,
    },
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

/// The requests of content transfer as a machine makes them. Every one names
/// the command it is made for.
#[async_trait]
pub trait BlobEndpoint: Send + Sync {
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
    /// How many transfers a machine runs at once.
    pub concurrency: usize,
    /// How many times in a row a piece may stand still before the transfer
    /// is given up.
    pub stalls_allowed: u32,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            stall: Duration::from_secs(u64::from(STALL_SECS)),
            concurrency: MAX_TRANSFERS_PER_MACHINE as usize,
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
    slots: Arc<Semaphore>,
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
            slots: Arc::new(Semaphore::new(options.concurrency.max(1))),
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

    /// Uploads content of `size` bytes with `hash`, for the command `command`.
    /// A transfer that was begun is gone on with from where the server stands.
    pub async fn upload(
        &self,
        command: Id,
        hash: Hash,
        size: i64,
        source: &mut dyn BlobSource,
    ) -> Result<Outcome, ContentError> {
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|e| ContentError::Local(e.to_string()))?;
        let mut outcome = Outcome::default();
        let mut rate: Option<i64> = None;
        let mut stalls = 0u32;
        let mut shrink_to: Option<i64> = None;
        let mut offset = self.resume_point(command, &hash).await?;
        let mut last_offset = offset;
        // An absent empty blob still needs one PUT to create and verify it.
        // Its resume point is zero, just like a completed empty blob's.
        if size != 0 && offset >= size {
            return Ok(outcome);
        }
        loop {
            let mut piece = next_chunk_bytes(rate);
            if let Some(smaller) = shrink_to {
                piece = piece.min(smaller);
            }
            let piece = piece.min(size - offset);
            let body = source
                .read(offset, piece)
                .await
                .map_err(|e| ContentError::Local(e.to_string()))?;
            if size != 0 && body.is_empty() {
                return Err(ContentError::Local("the source ended early".to_owned()));
            }
            let sent = i64::try_from(body.len()).unwrap_or(i64::MAX);
            let started = Instant::now();
            let attempt = tokio::time::timeout(
                self.options.stall,
                self.endpoint
                    .put(command, &hash, PutQuery { offset, size }, body),
            )
            .await;
            match attempt {
                Ok(Ok(progress)) => {
                    stalls = 0;
                    self.note(&mut rate, sent, started, true);
                    outcome.requests += 1;
                    outcome.largest_piece = outcome.largest_piece.max(sent);
                    outcome.smallest_piece = if outcome.smallest_piece == 0 {
                        sent
                    } else {
                        outcome.smallest_piece.min(sent)
                    };
                    offset = progress.received;
                    last_offset = offset;
                    if progress.complete || offset >= size {
                        return Ok(outcome);
                    }
                }
                Ok(Err(PutRefusal::Offset { received })) => {
                    outcome.retries += 1;
                    outcome.lost_bytes += (offset - received).max(0);
                    offset = received;
                }
                Ok(Err(PutRefusal::HashMismatch)) => return Err(ContentError::HashMismatch),
                Ok(Err(PutRefusal::Refused(why))) => return Err(ContentError::Refused(why)),
                Ok(Err(PutRefusal::Failed(_))) | Err(_) => {
                    outcome.retries += 1;
                    stalls += 1;
                    if stalls > self.options.stalls_allowed {
                        return Err(ContentError::StoodStill);
                    }
                    shrink_to = Some(chunk_bytes_after_stall(sent));
                    // Where the server stands tells what the failure cost.
                    let before = offset;
                    offset = self
                        .resume_point(command, &hash)
                        .await
                        .unwrap_or(last_offset);
                    outcome.lost_bytes += (before + sent - offset).max(0).min(sent);
                }
            }
        }
    }

    async fn resume_point(&self, command: Id, hash: &Hash) -> Result<i64, ContentError> {
        match self.endpoint.head(command, hash).await {
            Ok(Some(progress)) => Ok(progress.received),
            Ok(None) => Ok(0),
            Err(PutRefusal::Refused(why)) => Err(ContentError::Refused(why)),
            Err(_) => Ok(0),
        }
    }

    fn note(&self, rate: &mut Option<i64>, bytes: i64, started: Instant, up: bool) {
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
            Some(before) => {
                (before as f64 + RATE_WEIGHT * (measured as f64 - before as f64)) as i64
            }
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

    /// Downloads content of `size` bytes with `hash`, for the command
    /// `command`, into `sink`, from where the sink already stands.
    pub async fn download(
        &self,
        command: Id,
        hash: Hash,
        size: i64,
        sink: &mut dyn BlobSink,
    ) -> Result<Outcome, ContentError> {
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|e| ContentError::Local(e.to_string()))?;
        let mut outcome = Outcome::default();
        let mut rate: Option<i64> = None;
        let mut stalls = 0u32;
        let mut shrink_to: Option<i64> = None;
        let local = |e: std::io::Error| ContentError::Local(e.to_string());
        let mut offset = sink.received().await.map_err(local)?;
        while offset < size {
            let mut piece = next_chunk_bytes(rate);
            if let Some(smaller) = shrink_to {
                piece = piece.min(smaller);
            }
            let piece = piece.min(size - offset);
            let started = Instant::now();
            let attempt = tokio::time::timeout(
                self.options.stall,
                self.endpoint.get(command, &hash, offset, piece),
            )
            .await;
            match attempt {
                Ok(Ok(bytes)) if !bytes.is_empty() => {
                    stalls = 0;
                    let got = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
                    sink.write(offset, bytes).await.map_err(local)?;
                    self.note(&mut rate, got, started, false);
                    outcome.requests += 1;
                    outcome.largest_piece = outcome.largest_piece.max(got);
                    outcome.smallest_piece = if outcome.smallest_piece == 0 {
                        got
                    } else {
                        outcome.smallest_piece.min(got)
                    };
                    offset += got;
                }
                Ok(Err(PutRefusal::Refused(why))) => return Err(ContentError::Refused(why)),
                Ok(Ok(_) | Err(_)) | Err(_) => {
                    outcome.retries += 1;
                    stalls += 1;
                    if stalls > self.options.stalls_allowed {
                        return Err(ContentError::StoodStill);
                    }
                    shrink_to = Some(chunk_bytes_after_stall(piece));
                    offset = sink.received().await.map_err(local)?;
                }
            }
        }
        if sink.hash().await.map_err(local)? != hash {
            sink.discard().await.map_err(local)?;
            return Err(ContentError::HashMismatch);
        }
        Ok(outcome)
    }
}
