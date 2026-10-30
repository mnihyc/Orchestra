//! Content that moves beside the session, for the commands that carry it: an archive or an image fetched for a command, a session exported and put up.
//!
//! Large content never travels in a command. A command that needs some (`session.open` with an archive
//! to import, `turn.start` with an image) names it by its hash, and the connector fetches it over HTTPS
//! into a scratch directory under the command's own name, hands the adapter a path, and removes the
//! directory when the command is over. A command that makes some (`session.export`) hands the file to
//! the same machinery, which hashes it and puts it up. The pieces, their sizes and the resuming after a
//! cut are the link's ([`orch_link::content`]); this module is the HTTPS requests under them, and the
//! files at either end.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use hyper::{Method, StatusCode};
use orch_link::content::{
    BlobEndpoint, BlobSink, BlobSource, ContentError, PutRefusal, TransferOptions, Transfers,
};
use orch_link::framing::LinkMeter;
use orch_link_contract::commands::{BlobRef, CommandError, ErrorCode};
use orch_link_contract::content::{
    BlobProgress, COMMAND_HEADER, COMPLETE_HEADER, MISSING_PATH, MissingReply, MissingRequest,
    PutQuery, RECEIVED_HEADER, STATUS_HASH_MISMATCH, STATUS_OFFSET_CONFLICT, authorization,
    blob_path,
};
use orch_link_contract::enrollment::Trust;
use orch_types::{Hash, Id};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use url::Url;

use crate::https::HttpsClient;

tokio::task_local! { static WORK: Arc<dyn orch_driver_contract::volumes::VolumeWork>; }

/// Gives a transfer the command's bounded progress and cancellation context for this task alone.
pub async fn with_work<F: std::future::Future>(
    work: Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    operation: F,
) -> F::Output {
    WORK.scope(work, operation).await
}

/// The work of a command whose transfers only the system's request to stop it can interrupt: the
/// command's own token ends the piece in flight and keeps the next one from starting. It keeps no
/// counts: how far an upload has come is told by the system, which holds what has arrived of it.
struct Stoppable {
    stop: tokio_util::sync::CancellationToken,
    timeout_secs: u64,
}

#[async_trait]
impl orch_driver_contract::volumes::VolumeWork for Stoppable {
    fn report(&self, _: orch_driver_contract::volumes::VolumeProgress) {}
    fn is_cancelled(&self) -> bool {
        self.stop.is_cancelled()
    }
    async fn cancelled(&self) {
        self.stop.cancelled().await;
    }
    fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }
}

/// Runs `operation`, whose transfers ([`Content::put`], [`Content::fetch`]) end at their next piece
/// when `stop` fires, which is when the system asks for the command not to be carried out. Without
/// this a transfer goes on to the end of its file after the system has given the command up.
/// `timeout_secs` is the allowance the command has from the dispatcher.
pub async fn stoppable<F: std::future::Future>(
    stop: &tokio_util::sync::CancellationToken,
    timeout_secs: u64,
    operation: F,
) -> F::Output {
    with_work(
        Arc::new(Stoppable {
            stop: stop.clone(),
            timeout_secs,
        }),
        operation,
    )
    .await
}

fn check_work(
    work: &Option<Arc<dyn orch_driver_contract::volumes::VolumeWork>>,
) -> std::io::Result<()> {
    if work.as_ref().is_some_and(|work| work.is_cancelled()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "the transfer was cancelled",
        ));
    }
    Ok(())
}

async fn interruptible<T>(
    request: impl std::future::Future<Output = Result<T, PutRefusal>>,
) -> Result<T, PutRefusal> {
    match WORK.try_with(Arc::clone).ok() {
        Some(work) => tokio::select! {
            biased;
            () = work.cancelled() => Err(PutRefusal::Refused("the transfer was cancelled".into())),
            response = request => response,
        },
        None => request.await,
    }
}

// Cancel network waits without dropping a filesystem writer before its last bounded write is flushed.
struct CancellableEndpoint(Arc<dyn BlobEndpoint>);
#[async_trait]
impl BlobEndpoint for CancellableEndpoint {
    async fn head(&self, command: Id, hash: &Hash) -> Result<Option<BlobProgress>, PutRefusal> {
        interruptible(self.0.head(command, hash)).await
    }
    async fn put(
        &self,
        command: Id,
        hash: &Hash,
        query: PutQuery,
        body: Bytes,
    ) -> Result<BlobProgress, PutRefusal> {
        interruptible(self.0.put(command, hash, query, body)).await
    }
    async fn get(
        &self,
        command: Id,
        hash: &Hash,
        offset: i64,
        len: i64,
    ) -> Result<Bytes, PutRefusal> {
        interruptible(self.0.get(command, hash, offset, len)).await
    }
    async fn missing(&self, command: Id, hashes: &[Hash]) -> Result<Vec<Hash>, PutRefusal> {
        interruptible(self.0.missing(command, hashes)).await
    }
}

fn report_bytes(
    work: &Option<Arc<dyn orch_driver_contract::volumes::VolumeWork>>,
    done: i64,
    total: i64,
    message: &str,
) {
    if let Some(work) = work {
        work.report(orch_driver_contract::volumes::VolumeProgress {
            bytes_done: done,
            bytes_total: Some(total),
            message: Some(message.into()),
            ..Default::default()
        });
    }
}

/// How long one request for content may take from start to end. The link's pieces are sized to a few
/// seconds of the line's rate, and one that stands still is given up after the link's own limit.
const REQUEST_TIMEOUT: Duration =
    Duration::from_secs(orch_link_contract::content::STALL_SECS as u64);
/// How many bytes of a file are read at a time to hash it.
const HASH_READ_BYTES: usize = 64 * 1024;

/// Where the system's content endpoint is now: the address of the endpoint the session stands on and,
/// for an endpoint behind a public edge, the token of the session.
#[derive(Debug, Clone)]
pub struct EndpointNow {
    /// The scheme, host and port.
    pub base: Url,
    /// How the endpoint is trusted.
    pub trust: Trust,
    /// What a machine presents where the connection shows no client certificate.
    pub session_token: Option<String>,
}

/// Finds the endpoint that is in use.
pub type WhereIsIt = dyn Fn() -> Option<EndpointNow> + Send + Sync;

/// The requests for content, over HTTPS to the endpoint of the session.
pub struct HttpsBlobs {
    pinned: HttpsClient,
    public: HttpsClient,
    endpoint: Arc<WhereIsIt>,
}

impl std::fmt::Debug for HttpsBlobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpsBlobs").finish_non_exhaustive()
    }
}

impl HttpsBlobs {
    /// Requests made to the endpoint `endpoint` finds, with `pinned` to an endpoint of the system's own
    /// authority and `public` to one behind a public edge.
    #[must_use]
    pub fn new(pinned: HttpsClient, public: HttpsClient, endpoint: Arc<WhereIsIt>) -> Self {
        Self {
            pinned,
            public,
            endpoint,
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        command: Id,
        body: Bytes,
        extra: &[(&str, String)],
    ) -> Result<crate::https::Reply, PutRefusal> {
        let Some(now) = (self.endpoint)() else {
            return Err(PutRefusal::Failed("no session stands".to_owned()));
        };
        let mut headers = vec![(COMMAND_HEADER, command.to_string())];
        if let Some(token) = &now.session_token {
            headers.push(("authorization", authorization(token)));
        }
        headers.extend(extra.iter().map(|(n, v)| (*n, v.clone())));
        let client = match now.trust {
            Trust::Pinned => &self.pinned,
            Trust::Public => &self.public,
        };
        client
            .send(&now.base, method, path, &headers, body, REQUEST_TIMEOUT)
            .await
            .map_err(|e| PutRefusal::Failed(e.to_string()))
    }
}

fn refused(status: StatusCode) -> Option<PutRefusal> {
    match status.as_u16() {
        401 | 403 | 404 | 410 | 413 => {
            Some(PutRefusal::Refused(format!("the system answered {status}")))
        }
        _ => None,
    }
}

fn progress_of(reply: &crate::https::Reply) -> Option<BlobProgress> {
    let header = |name: &str| reply.headers.get(name)?.to_str().ok().map(str::to_owned);
    if let (Some(received), Some(complete)) = (header(RECEIVED_HEADER), header(COMPLETE_HEADER)) {
        return BlobProgress::from_headers(&received, &complete);
    }
    serde_json::from_slice(&reply.body).ok()
}

#[async_trait]
impl BlobEndpoint for HttpsBlobs {
    async fn head(&self, command: Id, hash: &Hash) -> Result<Option<BlobProgress>, PutRefusal> {
        let reply = self
            .call(Method::HEAD, &blob_path(hash), command, Bytes::new(), &[])
            .await?;
        match reply.status.as_u16() {
            200 => Ok(progress_of(&reply)),
            404 => Ok(None),
            _ => Err(refused(reply.status)
                .unwrap_or_else(|| PutRefusal::Failed(format!("HEAD answered {}", reply.status)))),
        }
    }

    async fn put(
        &self,
        command: Id,
        hash: &Hash,
        query: PutQuery,
        body: Bytes,
    ) -> Result<BlobProgress, PutRefusal> {
        let path = format!(
            "{}?offset={}&size={}",
            blob_path(hash),
            query.offset,
            query.size
        );
        let reply = self.call(Method::PUT, &path, command, body, &[]).await?;
        match reply.status.as_u16() {
            200 => progress_of(&reply)
                .ok_or_else(|| PutRefusal::Failed("the answer states no progress".to_owned())),
            STATUS_OFFSET_CONFLICT => {
                let received = progress_of(&reply).map_or(0, |progress| progress.received);
                Err(PutRefusal::Offset { received })
            }
            STATUS_HASH_MISMATCH => Err(PutRefusal::HashMismatch),
            _ => Err(refused(reply.status)
                .unwrap_or_else(|| PutRefusal::Failed(format!("PUT answered {}", reply.status)))),
        }
    }

    async fn get(
        &self,
        command: Id,
        hash: &Hash,
        offset: i64,
        len: i64,
    ) -> Result<Bytes, PutRefusal> {
        let range = format!("bytes={offset}-{}", offset + len - 1);
        let reply = self
            .call(
                Method::GET,
                &blob_path(hash),
                command,
                Bytes::new(),
                &[("range", range)],
            )
            .await?;
        match reply.status.as_u16() {
            200 | 206 => Ok(reply.body),
            _ => Err(refused(reply.status)
                .unwrap_or_else(|| PutRefusal::Failed(format!("GET answered {}", reply.status)))),
        }
    }

    async fn missing(&self, command: Id, hashes: &[Hash]) -> Result<Vec<Hash>, PutRefusal> {
        let body = serde_json::to_vec(&MissingRequest {
            hashes: hashes.to_vec(),
        })
        .map_err(|e| PutRefusal::Failed(e.to_string()))?;
        let reply = self
            .call(
                Method::POST,
                MISSING_PATH,
                command,
                Bytes::from(body),
                &[("content-type", "application/json".to_owned())],
            )
            .await?;
        if reply.status.as_u16() != 200 {
            return Err(refused(reply.status).unwrap_or_else(|| {
                PutRefusal::Failed(format!("the question answered {}", reply.status))
            }));
        }
        serde_json::from_slice::<MissingReply>(&reply.body)
            .map(|reply| reply.missing)
            .map_err(|e| PutRefusal::Failed(e.to_string()))
    }
}

// ---------------------------------------------------------------------------------------------------------
// Files at either end

struct FileSink {
    path: PathBuf,
    work: Option<Arc<dyn orch_driver_contract::volumes::VolumeWork>>,
    total: i64,
}

#[async_trait]
impl BlobSink for FileSink {
    async fn received(&mut self) -> std::io::Result<i64> {
        match tokio::fs::metadata(&self.path).await {
            Ok(meta) => Ok(i64::try_from(meta.len()).unwrap_or(i64::MAX)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e),
        }
    }

    async fn write(&mut self, offset: i64, bytes: Bytes) -> std::io::Result<()> {
        check_work(&self.work)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&self.path)
            .await?;
        file.seek(std::io::SeekFrom::Start(u64::try_from(offset).unwrap_or(0)))
            .await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        report_bytes(
            &self.work,
            offset.saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
            self.total,
            "Downloading files",
        );
        Ok(())
    }

    async fn hash(&mut self) -> std::io::Result<Hash> {
        hash_file(&self.path).await
    }

    async fn discard(&mut self) -> std::io::Result<()> {
        match tokio::fs::remove_file(&self.path).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

struct FileSource {
    file: tokio::fs::File,
    work: Option<Arc<dyn orch_driver_contract::volumes::VolumeWork>>,
    total: i64,
}

#[async_trait]
impl BlobSource for FileSource {
    async fn read(&mut self, offset: i64, len: i64) -> std::io::Result<Bytes> {
        check_work(&self.work)?;
        self.file
            .seek(std::io::SeekFrom::Start(u64::try_from(offset).unwrap_or(0)))
            .await?;
        let mut buffer = vec![0u8; usize::try_from(len).unwrap_or(0)];
        let mut filled = 0;
        while filled < buffer.len() {
            let read = self.file.read(&mut buffer[filled..]).await?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        buffer.truncate(filled);
        report_bytes(
            &self.work,
            offset.saturating_add(i64::try_from(filled).unwrap_or(i64::MAX)),
            self.total,
            "Uploading files",
        );
        Ok(Bytes::from(buffer))
    }
}

/// The hash of a file, read in pieces.
pub async fn hash_file(path: &Path) -> std::io::Result<Hash> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Hash::hasher();
    let work = WORK.try_with(Arc::clone).ok();
    let total = i64::try_from(file.metadata().await?.len()).unwrap_or(i64::MAX);
    let mut done = 0i64;
    let mut buffer = vec![0u8; HASH_READ_BYTES];
    loop {
        check_work(&work)?;
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok(Hash::finish(&hasher));
        }
        hasher.update(&buffer[..read]);
        done = done.saturating_add(i64::try_from(read).unwrap_or(i64::MAX));
        report_bytes(&work, done, total, "Checking files");
    }
}

fn failure(error: &ContentError) -> CommandError {
    match error {
        ContentError::Refused(why) => CommandError::new(
            ErrorCode::Internal,
            format!("the system would not move the content: {why}"),
        ),
        ContentError::HashMismatch => CommandError::new(
            ErrorCode::Internal,
            "the content did not have the hash it was announced with",
        )
        .retryable(),
        ContentError::StoodStill => CommandError::new(
            ErrorCode::Timeout,
            "the transfer stood still for as long as a transfer waits",
        )
        .retryable(),
        ContentError::Local(why) => CommandError::new(
            ErrorCode::Internal,
            format!("the content could not be read or written here: {why}"),
        ),
    }
}

/// Moves the content of commands, through the link's pieces, to and from files.
pub struct Content {
    transfers: Transfers,
    endpoint: Arc<dyn BlobEndpoint>,
    scratch: PathBuf,
}

impl std::fmt::Debug for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Content")
            .field("scratch", &self.scratch)
            .finish_non_exhaustive()
    }
}

impl Content {
    /// Content moved over `endpoint`, whose rates are noted in `meter`, with files under `scratch`.
    #[must_use]
    pub fn new(endpoint: Arc<dyn BlobEndpoint>, meter: Arc<LinkMeter>, scratch: PathBuf) -> Self {
        let endpoint: Arc<dyn BlobEndpoint> = Arc::new(CancellableEndpoint(endpoint));
        Self {
            transfers: Transfers::new(Arc::clone(&endpoint), meter, TransferOptions::default()),
            endpoint,
            scratch,
        }
    }

    /// How large the content with `hash` is, as the system holds it: what it has received of it, when it
    /// holds it whole.
    pub async fn size_of(&self, command: Id, hash: Hash) -> Result<i64, CommandError> {
        match self.endpoint.head(command, &hash).await {
            Ok(Some(progress)) if progress.complete => Ok(progress.received),
            Ok(_) => Err(CommandError::new(
                ErrorCode::NotFound,
                "the system does not hold that content whole",
            )),
            Err(PutRefusal::Refused(why)) => Err(CommandError::new(
                ErrorCode::Internal,
                format!("the system would not say how large the content is: {why}"),
            )),
            Err(error) => {
                Err(CommandError::new(ErrorCode::Internal, error.to_string()).retryable())
            }
        }
    }

    /// The directory of a command's files.
    #[must_use]
    pub fn directory_of(&self, command: Id) -> PathBuf {
        self.scratch.join(command.to_string())
    }

    /// Fetches content of `size` bytes with `hash` for `command` into a file called `name` in the
    /// command's directory, and says where it is.
    pub async fn fetch(
        &self,
        command: Id,
        hash: Hash,
        size: i64,
        name: &str,
    ) -> Result<PathBuf, CommandError> {
        let directory = self.directory_of(command);
        tokio::fs::create_dir_all(&directory).await.map_err(|e| {
            CommandError::new(
                ErrorCode::Internal,
                format!("no directory for the content: {e}"),
            )
        })?;
        let path = directory.join(file_name(name));
        let mut sink = FileSink {
            path: path.clone(),
            work: WORK.try_with(Arc::clone).ok(),
            total: size,
        };
        self.transfers
            .download(command, hash, size, &mut sink)
            .await
            .map_err(|e| failure(&e))?;
        Ok(path)
    }

    /// Hashes the file at `path` and puts it up for `command`.
    pub async fn put(&self, command: Id, path: &Path) -> Result<BlobRef, CommandError> {
        let local = |e: std::io::Error| {
            CommandError::new(ErrorCode::Internal, format!("a file cannot be read: {e}"))
        };
        let size = i64::try_from(tokio::fs::metadata(path).await.map_err(local)?.len())
            .unwrap_or(i64::MAX);
        let hash = hash_file(path).await.map_err(local)?;
        let mut source = FileSource {
            file: tokio::fs::File::open(path).await.map_err(local)?,
            work: WORK.try_with(Arc::clone).ok(),
            total: size,
        };
        self.transfers
            .upload(command, hash, size, &mut source)
            .await
            .map_err(|e| failure(&e))?;
        Ok(BlobRef { hash, size })
    }

    /// Removes the files of a command that is over.
    pub async fn finish(&self, command: Id) {
        let _ = tokio::fs::remove_dir_all(self.directory_of(command)).await;
    }
}

/// A name that is safe to make a file of: the last part of what was given, and nothing that climbs.
fn file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = last
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "content".to_owned()
    } else {
        cleaned
    }
}

// ---------------------------------------------------------------------------------------------------------
// Content for the environment host

pub use for_the_host::{CommandBlobs, STANDING_COMMAND};

/// What the environment host puts into the system's content store and takes out of it, moved through the
/// link's transfers: the two small traits of the host, made from the transfers that belong to one command.
mod for_the_host {
    use super::Content;
    use std::io;
    use std::sync::Arc;

    use async_trait::async_trait;
    use bytes::Bytes;
    use orch_envhost::blobs::{BlobError, BlobReader, BlobSink, BlobSource, BlobWriter};
    use orch_link::content::{BlobSink as LinkSink, BlobSource as LinkSource, ContentError};
    use orch_types::{Hash, Id};

    /// The command identifier that content names when the environment host sends it on its own account:
    /// the list of a tree it publishes when a turn ended, what a shell printed last when the shell ended
    /// by itself. No command is open for those, and the nil identifier is what stands for "none"; the
    /// commands that upload carry their own identifier.
    pub const STANDING_COMMAND: Id = Id::nil();

    /// The end of the system's content transfer that belongs to one command, as the environment host takes
    /// it.
    pub struct CommandBlobs {
        content: Arc<Content>,
        command: Id,
    }

    impl std::fmt::Debug for CommandBlobs {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("CommandBlobs")
                .field("command", &self.command)
                .finish_non_exhaustive()
        }
    }

    impl Content {
        /// The transfers of `command`, for the environment host.
        #[must_use]
        pub fn blobs_for(self: &Arc<Self>, command: Id) -> CommandBlobs {
            CommandBlobs {
                content: Arc::clone(self),
                command,
            }
        }

        /// The transfers the host makes on its own account.
        #[must_use]
        pub fn standing_blobs(self: &Arc<Self>) -> CommandBlobs {
            self.blobs_for(STANDING_COMMAND)
        }
    }

    fn blob_error(error: ContentError) -> BlobError {
        match error {
            ContentError::Refused(why) => BlobError::Refused(why),
            ContentError::HashMismatch => BlobError::HashMismatch,
            ContentError::StoodStill => BlobError::StoodStill,
            ContentError::Local(why) => BlobError::Local(why),
        }
    }

    /// Where the bytes of an upload come from, as the link's transfer asks for them.
    struct Reading<'a>(&'a mut dyn BlobReader);

    #[async_trait]
    impl LinkSource for Reading<'_> {
        async fn read(&mut self, offset: i64, len: i64) -> io::Result<Bytes> {
            self.0.read(offset, len).await
        }
    }

    /// Where the bytes of a download go, as the link's transfer holds them.
    struct Writing<'a>(&'a mut dyn BlobWriter);

    #[async_trait]
    impl LinkSink for Writing<'_> {
        async fn received(&mut self) -> io::Result<i64> {
            self.0.received().await
        }

        async fn write(&mut self, offset: i64, bytes: Bytes) -> io::Result<()> {
            self.0.write(offset, bytes).await
        }

        async fn hash(&mut self) -> io::Result<Hash> {
            self.0.hash().await
        }

        async fn discard(&mut self) -> io::Result<()> {
            self.0.discard().await
        }
    }

    async fn interruptible<T>(
        work: impl std::future::Future<Output = Result<T, BlobError>>,
    ) -> Result<T, BlobError> {
        if let Some(cancel) = orch_envhost::error::cancellation() {
            tokio::select! { result = work => result, () = cancel.cancelled() => Err(BlobError::Refused("the transfer was cancelled".into())) }
        } else {
            work.await
        }
    }

    #[async_trait]
    impl BlobSink for CommandBlobs {
        async fn missing(&self, hashes: &[Hash]) -> Result<Vec<Hash>, BlobError> {
            interruptible(async {
                self.content
                    .transfers
                    .missing(self.command, hashes)
                    .await
                    .map_err(blob_error)
            })
            .await
        }

        async fn put(
            &self,
            hash: Hash,
            size: i64,
            source: &mut dyn BlobReader,
        ) -> Result<(), BlobError> {
            interruptible(async {
                self.content
                    .transfers
                    .upload(self.command, hash, size, &mut Reading(source))
                    .await
                    .map(|_| ())
                    .map_err(blob_error)
            })
            .await
        }
    }

    #[async_trait]
    impl BlobSource for CommandBlobs {
        async fn get(&self, hash: Hash, target: &mut dyn BlobWriter) -> Result<i64, BlobError> {
            interruptible(async {
                let size = self
                    .content
                    .size_of(self.command, hash)
                    .await
                    .map_err(|error| BlobError::Refused(failure_text(&error)))?;
                self.content
                    .transfers
                    .download(self.command, hash, size, &mut Writing(target))
                    .await
                    .map(|_| size)
                    .map_err(blob_error)
            })
            .await
        }
    }

    fn failure_text(error: &orch_link_contract::commands::CommandError) -> String {
        format!("{}: {}", error.code, error.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A system that holds what it was given, and gives it back in the pieces it is asked for.
    #[derive(Default)]
    struct Store {
        blobs: Mutex<HashMap<Hash, Vec<u8>>>,
    }

    #[async_trait]
    impl BlobEndpoint for Store {
        async fn head(&self, _: Id, hash: &Hash) -> Result<Option<BlobProgress>, PutRefusal> {
            Ok(self
                .blobs
                .lock()
                .unwrap()
                .get(hash)
                .map(|bytes| BlobProgress {
                    received: i64::try_from(bytes.len()).unwrap(),
                    complete: false,
                }))
        }

        async fn put(
            &self,
            _: Id,
            hash: &Hash,
            query: PutQuery,
            body: Bytes,
        ) -> Result<BlobProgress, PutRefusal> {
            let mut blobs = self.blobs.lock().unwrap();
            let held = blobs.entry(*hash).or_default();
            if i64::try_from(held.len()).unwrap() != query.offset {
                return Err(PutRefusal::Offset {
                    received: i64::try_from(held.len()).unwrap(),
                });
            }
            held.extend_from_slice(&body);
            let received = i64::try_from(held.len()).unwrap();
            let complete = received >= query.size;
            if complete && Hash::of(held) != *hash {
                blobs.remove(hash);
                return Err(PutRefusal::HashMismatch);
            }
            Ok(BlobProgress { received, complete })
        }

        async fn get(
            &self,
            _: Id,
            hash: &Hash,
            offset: i64,
            len: i64,
        ) -> Result<Bytes, PutRefusal> {
            let blobs = self.blobs.lock().unwrap();
            let held = blobs
                .get(hash)
                .ok_or_else(|| PutRefusal::Refused("unknown".into()))?;
            let from = usize::try_from(offset).unwrap();
            let to = (from + usize::try_from(len).unwrap()).min(held.len());
            Ok(Bytes::copy_from_slice(&held[from..to]))
        }

        async fn missing(&self, _: Id, hashes: &[Hash]) -> Result<Vec<Hash>, PutRefusal> {
            let blobs = self.blobs.lock().unwrap();
            Ok(hashes
                .iter()
                .filter(|h| !blobs.contains_key(h))
                .copied()
                .collect())
        }
    }

    fn content(store: Arc<Store>, scratch: &Path) -> Content {
        Content::new(store, LinkMeter::new(), scratch.to_owned())
    }

    #[tokio::test]
    async fn a_file_that_is_put_up_is_fetched_back_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::default());
        let content = content(Arc::clone(&store), dir.path());
        let original: Vec<u8> = (0..700_000u32).map(|n| (n % 251) as u8).collect();
        let source = dir.path().join("session.json");
        std::fs::write(&source, &original).unwrap();
        let command = orch_types::new_id();
        let blob = content.put(command, &source).await.unwrap();
        assert_eq!(blob.size, 700_000);
        assert_eq!(blob.hash, Hash::of(&original));
        let fetched = content
            .fetch(orch_types::new_id(), blob.hash, blob.size, "again.json")
            .await
            .unwrap();
        assert_eq!(std::fs::read(&fetched).unwrap(), original);
    }

    #[tokio::test]
    async fn content_that_does_not_have_its_hash_is_thrown_away_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::default());
        let announced = Hash::of(b"what was announced");
        store
            .blobs
            .lock()
            .unwrap()
            .insert(announced, b"something else entirely".to_vec());
        let content = content(store, dir.path());
        let command = orch_types::new_id();
        let error = content
            .fetch(command, announced, 23, "x.bin")
            .await
            .unwrap_err();
        assert_eq!(error.code, "internal");
        assert!(error.retryable);
        assert!(!content.directory_of(command).join("x.bin").exists());
    }

    #[tokio::test]
    async fn the_files_of_a_command_are_removed_when_it_is_over() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::default());
        let content = content(Arc::clone(&store), dir.path());
        let bytes = b"history".to_vec();
        let hash = Hash::of(&bytes);
        store.blobs.lock().unwrap().insert(hash, bytes);
        let command = orch_types::new_id();
        let path = content
            .fetch(command, hash, 7, "history.jsonl")
            .await
            .unwrap();
        assert!(path.exists());
        content.finish(command).await;
        assert!(!content.directory_of(command).exists());
    }

    #[test]
    fn a_name_cannot_climb_out_of_the_directory_of_a_command() {
        assert_eq!(file_name("../../etc/passwd"), "passwd");
        assert_eq!(file_name(".."), "content");
        assert_eq!(file_name(""), "content");
        assert_eq!(file_name("a\\b\\c.png"), "c.png");
        assert_eq!(file_name("plain.txt"), "plain.txt");
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use orch_driver_contract::volumes::{VolumeProgress, VolumeWork};
    use std::sync::atomic::{AtomicI64, Ordering};
    use tokio_util::sync::CancellationToken;

    struct Work {
        cancel: CancellationToken,
    }
    #[async_trait]
    impl VolumeWork for Work {
        fn report(&self, _: VolumeProgress) {}
        fn is_cancelled(&self) -> bool {
            self.cancel.is_cancelled()
        }
        async fn cancelled(&self) {
            self.cancel.cancelled().await;
        }
        fn timeout_secs(&self) -> u64 {
            30
        }
    }
    struct StopBeforeNextPiece {
        bytes: Bytes,
        work: Arc<Work>,
        stopped_at: AtomicI64,
    }
    #[async_trait]
    impl BlobEndpoint for StopBeforeNextPiece {
        async fn head(&self, _: Id, _: &Hash) -> Result<Option<BlobProgress>, PutRefusal> {
            Ok(Some(BlobProgress {
                received: self.bytes.len() as i64,
                complete: true,
            }))
        }
        async fn put(
            &self,
            _: Id,
            _: &Hash,
            _: PutQuery,
            _: Bytes,
        ) -> Result<BlobProgress, PutRefusal> {
            Err(PutRefusal::Refused("not an upload".into()))
        }
        async fn get(&self, _: Id, _: &Hash, offset: i64, len: i64) -> Result<Bytes, PutRefusal> {
            if offset > 0 {
                self.stopped_at.store(offset, Ordering::SeqCst);
                self.work.cancel.cancel();
            }
            Ok(self.bytes.slice(offset as usize..(offset + len) as usize))
        }
        async fn missing(&self, _: Id, _: &[Hash]) -> Result<Vec<Hash>, PutRefusal> {
            Ok(Vec::new())
        }
    }
    #[tokio::test]
    async fn a_cancelled_download_flushes_its_last_write_and_never_extends_the_file_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let command = orch_types::new_id();
        let work = Arc::new(Work {
            cancel: CancellationToken::new(),
        });
        let bytes = Bytes::from(vec![7u8; 3 * 1024 * 1024]);
        let hash = Hash::of(&bytes);
        let size = bytes.len() as i64;
        let endpoint = Arc::new(StopBeforeNextPiece {
            bytes,
            work: work.clone(),
            stopped_at: AtomicI64::new(0),
        });
        let content = Content::new(
            endpoint.clone(),
            LinkMeter::new(),
            directory.path().to_owned(),
        );
        let reporting: Arc<dyn VolumeWork> = work;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            with_work(
                reporting,
                content.fetch(command, hash, size, "download.part"),
            ),
        )
        .await?;
        assert!(result.is_err());
        let path = content.directory_of(command).join("download.part");
        let held = tokio::fs::metadata(&path).await?.len();
        assert!(
            held > 0,
            "the cancellation must happen during a real file transfer"
        );
        assert_eq!(
            held,
            endpoint.stopped_at.load(Ordering::SeqCst) as u64,
            "a piece was written after the stop request"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            tokio::fs::metadata(&path).await?.len(),
            held,
            "the file grew after the transfer returned"
        );
        content.finish(command).await;
        assert!(!path.exists());
        Ok(())
    }
}
