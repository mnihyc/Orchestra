//! Authorized blob access and pinning.
//!
//! The store of content is a seam ([`BlobStore`]): the program that starts the API gives it one, and
//! everything here is what the API does around it. Content comes in three ways. A machine moves it
//! beside its session in pieces, which wait in files of the API until the last one has come and the
//! whole is checked against its hash ([`crate::content`]); a person's browser sends an image or a
//! file whole or in pieces ([`crate::uploads`]); and a request that carries a body is written to
//! the store by [`Blobs::store_stream`], which hashes it on the way in. A blob is stored once
//! however often it arrives, and a row of `blobs` says that the store has it and what points at it:
//! [`Blobs::pin`] and [`Blobs::unpin`] are how a snapshot, a list of a tree or an attachment keeps
//! content from going with its grace.

use crate::cache::{Manifest, ManifestError};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use orch_blob_contract::range::{BlobMeta, ByteRange, ResolvedRange};
use orch_blob_contract::store::{
    BlobError, BlobRead, BlobStore, ByteStream, Stored, Verifier, stream_of,
};
use orch_db::queries::storage;
use orch_db::{Conn, DbError};
use orch_link_contract::content::MAX_TRANSFERS_PER_MACHINE;
use orch_types::{Hash, Id, new_id};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

/// The directory of the partial files of uploads, below the storage root.
pub const UPLOADS_DIR: &str = "uploads";

/// How many bytes of a file are read at a time when it is hashed or put in the store: a quarter of
/// a mebibyte, the size the store itself reads and writes in.
const FILE_READ_BYTES: usize = 256 * 1024;

/// How many lists of a tree whose files a download may name are kept, hash by hash: a machine that
/// writes a tree down fetches its files one request at a time, and four transfers at once are
/// the most it runs, so a few lists are all that are asked for again and again.
const LISTED_MANIFESTS_KEPT: usize = 8;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why something could not be done with content.
#[derive(Debug, thiserror::Error)]
pub enum BlobsError {
    /// The store refused or failed.
    #[error("{0}")]
    Store(#[from] BlobError),
    /// A file of the API could not be read or written.
    #[error("a file of the uploads could not be used: {0}")]
    Io(#[from] std::io::Error),
    /// The content is larger than may be stored.
    #[error("the content is larger than the {limit} bytes that may be stored")]
    TooLarge {
        /// The most that may be stored.
        limit: i64,
    },
    /// The database failed.
    #[error("{0}")]
    Db(#[from] DbError),
    /// The body of a request broke off.
    #[error("the body of the request broke off: {0}")]
    Body(String),
    /// A list of a tree could not be read.
    #[error("{0}")]
    Manifest(#[from] ManifestError),
}

/// What [`Blobs`] shares between its clones.
struct Inner {
    store: Arc<dyn BlobStore>,
    uploads: PathBuf,
    /// Whether the directory of the uploads was made for this handle alone, and goes with it.
    owns_uploads: bool,
    transfers: Mutex<HashMap<Id, u32>>,
    listed: Mutex<VecDeque<(Hash, Arc<HashSet<Hash>>)>>,
    previews: Arc<tokio::sync::Mutex<crate::previews::Cache>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if self.owns_uploads {
            // A handle that was made for a test or for a program without a store of its own
            // cleans up after itself; failing to is not worth a message.
            let _ = std::fs::remove_dir_all(&self.uploads);
        }
    }
}

/// The store of content, with the places and the limits that go with it. Cheap to clone; every
/// clone is the same store.
#[derive(Clone)]
pub struct Blobs {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Blobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blobs")
            .field("uploads", &self.inner.uploads)
            .finish_non_exhaustive()
    }
}

/// A transfer a machine has under way, counted against the few it may run at once. Dropping it
/// ends the count.
pub struct Transfer {
    inner: Arc<Inner>,
    machine: Id,
}

impl Drop for Transfer {
    fn drop(&mut self) {
        let mut transfers = lock(&self.inner.transfers);
        if let Some(count) = transfers.get_mut(&self.machine) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                transfers.remove(&self.machine);
            }
        }
    }
}

/// The machine already runs as many transfers as it may.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("this machine runs as many transfers at once as it may")]
pub struct TooManyTransfers;

impl Blobs {
    /// The content in `store`, with the files of uploads that are not whole below `storage_root`.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, storage_root: &Path) -> Self {
        Self::with(store, storage_root.join(UPLOADS_DIR), false)
    }

    /// Content kept in memory, with the files of uploads in a directory of its own that goes when
    /// the last clone does: for tests, and for a program that has no store.
    #[must_use]
    pub fn in_memory() -> Self {
        let directory = std::env::temp_dir().join(format!("orch-uploads-{}", new_id().simple()));
        Self::with(Arc::new(MemoryBlobs::default()), directory, true)
    }

    fn with(store: Arc<dyn BlobStore>, uploads: PathBuf, owns_uploads: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                uploads,
                owns_uploads,
                transfers: Mutex::new(HashMap::new()),
                listed: Mutex::new(VecDeque::new()),
                previews: Arc::new(tokio::sync::Mutex::new(crate::previews::Cache::default())),
            }),
        }
    }

    /// The store itself.
    #[must_use]
    pub fn store(&self) -> Arc<dyn BlobStore> {
        Arc::clone(&self.inner.store)
    }

    /// The bounded derived-preview cache belonging to this store.
    pub(crate) fn previews(&self) -> Arc<tokio::sync::Mutex<crate::previews::Cache>> {
        Arc::clone(&self.inner.previews)
    }

    /// The directory where the pieces of uploads wait.
    #[must_use]
    pub fn uploads_dir(&self) -> &Path {
        &self.inner.uploads
    }

    /// The file where the pieces of a machine's upload of `hash` wait.
    #[must_use]
    pub fn part_of(&self, hash: &Hash) -> PathBuf {
        self.inner.uploads.join(format!("{hash}.part"))
    }

    /// The file where the pieces of a person's upload session wait.
    #[must_use]
    pub fn part_of_session(&self, session: Id) -> PathBuf {
        self.inner
            .uploads
            .join(format!("session-{}.part", session.simple()))
    }

    /// Makes the directory of the uploads if it is not there.
    pub async fn make_uploads_dir(&self) -> Result<(), BlobsError> {
        tokio::fs::create_dir_all(&self.inner.uploads).await?;
        Ok(())
    }

    /// Counts a transfer for `machine`, unless it runs as many as it may.
    pub fn begin_transfer(&self, machine: Id) -> Result<Transfer, TooManyTransfers> {
        let mut transfers = lock(&self.inner.transfers);
        let count = transfers.entry(machine).or_insert(0);
        if *count >= MAX_TRANSFERS_PER_MACHINE {
            return Err(TooManyTransfers);
        }
        *count += 1;
        Ok(Transfer {
            inner: Arc::clone(&self.inner),
            machine,
        })
    }

    /// Writes what `body` brings to the store, hashing it as it comes: the pieces go to a file of
    /// the uploads first, because the store wants the hash before the content. Refused as soon as
    /// more than `limit` bytes have come. The blob is not recorded in the database; the caller does
    /// that in its own transaction ([`Blobs::register`]).
    pub async fn store_stream<S, E>(&self, body: S, limit: i64) -> Result<BlobMeta, BlobsError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        self.make_uploads_dir().await?;
        let path = self
            .inner
            .uploads
            .join(format!("incoming-{}.part", new_id().simple()));
        let outcome = self.write_hashing(body, limit, &path).await;
        let stored = match outcome {
            Ok((hash, size)) => self
                .put_file(&path, hash, size)
                .await
                .map(|_| BlobMeta { hash, size }),
            Err(error) => Err(error),
        };
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "a file of an upload that ended could not be removed");
        }
        stored
    }

    async fn write_hashing<S, E>(
        &self,
        mut body: S,
        limit: i64,
        path: &Path,
    ) -> Result<(Hash, i64), BlobsError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let mut file = tokio::fs::File::create(path).await?;
        let mut hasher = Hash::hasher();
        let mut size: i64 = 0;
        while let Some(piece) = body.next().await {
            let piece = piece.map_err(|error| BlobsError::Body(error.to_string()))?;
            size = size.saturating_add(i64::try_from(piece.len()).unwrap_or(i64::MAX));
            if size > limit {
                return Err(BlobsError::TooLarge { limit });
            }
            hasher.update(&piece);
            file.write_all(&piece).await?;
        }
        file.flush().await?;
        Ok((Hash::finish(&hasher), size))
    }

    /// Reads a whole file and says its hash and size.
    pub async fn hash_file(path: &Path) -> Result<(Hash, i64), BlobsError> {
        let file = tokio::fs::File::open(path).await?;
        let mut stream = ReaderStream::with_capacity(file, FILE_READ_BYTES);
        let mut hasher = Hash::hasher();
        let mut size: i64 = 0;
        while let Some(piece) = stream.next().await {
            let piece = piece?;
            size = size.saturating_add(i64::try_from(piece.len()).unwrap_or(i64::MAX));
            hasher.update(&piece);
        }
        Ok((Hash::finish(&hasher), size))
    }

    /// Puts a file in the store as the content `hash` of `size` bytes, which the store checks as it
    /// reads: content that is not what the hash says leaves nothing.
    pub async fn put_file(&self, path: &Path, hash: Hash, size: i64) -> Result<Stored, BlobsError> {
        let file = tokio::fs::File::open(path).await?;
        let stream: ByteStream = ReaderStream::with_capacity(file, FILE_READ_BYTES)
            .map(|piece| piece.map_err(|error| BlobError::Io(error.to_string())))
            .boxed();
        Ok(self.inner.store.put(&hash, Some(size), stream).await?)
    }

    /// Puts bytes that are in memory in the store.
    pub async fn put_bytes(&self, bytes: Bytes) -> Result<BlobMeta, BlobsError> {
        let hash = Hash::of(&bytes);
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        self.inner
            .store
            .put(&hash, Some(size), stream_of(vec![bytes]))
            .await?;
        Ok(BlobMeta { hash, size })
    }

    /// Reads a blob whole into memory, if it is no larger than `limit`: for lists and other content
    /// the API itself looks into.
    pub async fn read_whole(&self, hash: &Hash, limit: i64) -> Result<Bytes, BlobsError> {
        let size = self
            .inner
            .store
            .size(hash)
            .await?
            .ok_or(BlobError::NotFound(*hash))?;
        if size > limit {
            return Err(BlobsError::TooLarge { limit });
        }
        let read = self.inner.store.get(hash, None).await?;
        Ok(orch_blob_contract::store::read_all(read.body).await?)
    }

    /// Reads a part of a blob.
    pub async fn read(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        self.inner.store.get(hash, range).await
    }

    /// Records in the database that the store holds a blob, or that it was asked for now.
    pub async fn register(
        &self,
        conn: &mut Conn,
        meta: BlobMeta,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        storage::register_blob(conn, &meta.hash, meta.size, at).await
    }

    /// Makes sure the database has a row for each of `hashes` that the store holds, and says which
    /// the store lacks: a machine that is asked which content is missing is answered from the
    /// store, which is the truth about it.
    pub async fn missing(
        &self,
        conn: &mut Conn,
        hashes: &[Hash],
        at: DateTime<Utc>,
    ) -> Result<Vec<Hash>, BlobsError> {
        let mut absent = Vec::new();
        let mut held = Vec::new();
        for hash in hashes {
            match self.inner.store.size(hash).await? {
                Some(size) => held.push((*hash, size)),
                None => absent.push(*hash),
            }
        }
        let known: HashSet<Hash> =
            storage::blobs_among(conn, &held.iter().map(|each| each.0).collect::<Vec<_>>())
                .await?
                .into_iter()
                .collect();
        for (hash, size) in held {
            if !known.contains(&hash) {
                storage::register_blob(conn, &hash, size, at).await?;
            }
        }
        Ok(absent)
    }

    /// Adds a pin to each of `hashes`: the content stays while anything is pinned to it. Says
    /// how many had no row to pin, which is a defect of whoever pins, since content is recorded
    /// when it arrives.
    pub async fn pin(&self, conn: &mut Conn, hashes: &[Hash]) -> Result<u64, DbError> {
        let distinct: Vec<Hash> = hashes
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let pinned = storage::pin_blobs(conn, &distinct).await?;
        Ok((distinct.len() as u64).saturating_sub(pinned))
    }

    /// Takes a pin off each of `hashes`; content nothing points at is cache again, and goes after
    /// its grace.
    pub async fn unpin(
        &self,
        conn: &mut Conn,
        hashes: &[Hash],
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let distinct: Vec<Hash> = hashes
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        storage::unpin_blobs(conn, &distinct, at).await?;
        Ok(())
    }

    /// The hashes of the files a list of a tree names: what a download for a command that writes
    /// that tree down is served.
    pub async fn files_of_manifest(
        &self,
        manifest: &Hash,
        limit: i64,
    ) -> Result<Arc<HashSet<Hash>>, BlobsError> {
        if let Some((_, found)) = lock(&self.inner.listed)
            .iter()
            .find(|(hash, _)| hash == manifest)
        {
            return Ok(Arc::clone(found));
        }
        let bytes = self.read_whole(manifest, limit).await?;
        let parsed = tokio::task::spawn_blocking(move || Manifest::parse(&bytes))
            .await
            .map_err(|error| BlobsError::Body(error.to_string()))??;
        let found: Arc<HashSet<Hash>> = Arc::new(parsed.hashes().collect());
        let mut listed = lock(&self.inner.listed);
        listed.push_front((*manifest, Arc::clone(&found)));
        listed.truncate(LISTED_MANIFESTS_KEPT);
        Ok(found)
    }
}

// ---------------------------------------------------------------- the files of pieces

/// Writes a piece at `offset` of the file where the pieces of an upload wait: whatever lies beyond
/// the bytes that are known to have arrived (a piece the system did not get to record) is cut off
/// first, and the piece is on the disk before it is answered.
pub async fn write_piece(path: &Path, offset: i64, piece: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncSeekExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .await?;
    let offset = u64::try_from(offset).unwrap_or(0);
    file.set_len(offset).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    file.write_all(piece).await?;
    file.flush().await?;
    file.sync_data().await
}

/// Removes the file of an upload that ended; a file that is not there is not a fault.
pub async fn discard(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, "the file of an upload that ended could not be removed");
    }
}

// ---------------------------------------------------------------- content in memory

/// A store that keeps content in memory: the least that keeps the contract of the seam. For tests
/// and for a program that has no directory to keep content in.
#[derive(Debug, Default)]
pub struct MemoryBlobs {
    blobs: Mutex<BTreeMap<Hash, Bytes>>,
}

#[async_trait]
impl BlobStore for MemoryBlobs {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        mut content: ByteStream,
    ) -> Result<Stored, BlobError> {
        let mut verifier = Verifier::new(*hash, size);
        let mut all = Vec::new();
        while let Some(piece) = content.next().await {
            let piece = piece?;
            verifier.update(&piece);
            all.extend_from_slice(&piece);
        }
        let meta = verifier.finish()?;
        let created = lock(&self.blobs).insert(*hash, Bytes::from(all)).is_none();
        Ok(Stored { meta, created })
    }

    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        let content = lock(&self.blobs)
            .get(hash)
            .cloned()
            .ok_or(BlobError::NotFound(*hash))?;
        let size = i64::try_from(content.len()).unwrap_or(i64::MAX);
        let range = match range {
            None => ResolvedRange::whole(size),
            Some(range) => range
                .resolve(size)
                .map_err(|_| BlobError::RangeNotSatisfiable { size })?,
        };
        let from = usize::try_from(range.start).unwrap_or(0);
        let to = usize::try_from(range.end()).unwrap_or(content.len());
        Ok(BlobRead {
            meta: BlobMeta { hash: *hash, size },
            range,
            body: stream_of(vec![content.slice(from..to)]),
        })
    }

    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        Ok(lock(&self.blobs).contains_key(hash))
    }

    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        Ok(lock(&self.blobs)
            .get(hash)
            .map(|content| i64::try_from(content.len()).unwrap_or(i64::MAX)))
    }

    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        Ok(lock(&self.blobs).remove(hash).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    #[tokio::test]
    async fn a_machine_runs_four_transfers_at_once_and_a_fifth_waits_for_one_to_end() {
        let blobs = Blobs::in_memory();
        let machine = new_id();
        let held: Vec<Transfer> = (0..MAX_TRANSFERS_PER_MACHINE)
            .map(|_| blobs.begin_transfer(machine).unwrap())
            .collect();
        assert!(blobs.begin_transfer(machine).is_err());
        assert!(
            blobs.begin_transfer(new_id()).is_ok(),
            "another machine has transfers of its own"
        );
        drop(held);
        assert!(blobs.begin_transfer(machine).is_ok());
    }

    #[tokio::test]
    async fn a_body_is_stored_under_the_hash_it_has_and_one_that_is_too_large_leaves_nothing() {
        let blobs = Blobs::in_memory();
        let pieces: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(b"hello, ")),
            Ok(Bytes::from_static(b"world")),
        ];
        let meta = blobs.store_stream(stream::iter(pieces), 100).await.unwrap();
        assert_eq!(meta.hash, Hash::of(b"hello, world"));
        assert_eq!(meta.size, 12);
        assert!(blobs.store().exists(&meta.hash).await.unwrap());
        let too_long: Vec<Result<Bytes, std::io::Error>> =
            vec![Ok(Bytes::from_static(b"0123456789"))];
        let refused = blobs.store_stream(stream::iter(too_long), 4).await;
        assert!(matches!(refused, Err(BlobsError::TooLarge { limit: 4 })));
        assert!(
            !blobs
                .store()
                .exists(&Hash::of(b"0123456789"))
                .await
                .unwrap()
        );
    }
}
