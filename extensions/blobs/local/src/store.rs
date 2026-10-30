//! Immutable hash-addressed files.
//!
//! A blob is a file that is written once and then only read or removed: its name is the hash of its content, so
//! the same name can never be given other bytes, and two writers of the same content produce the same file. The
//! files are made read-only once they are in place, so that nothing changes one by accident.
//!
//! ```text
//! <root>/blobs/ab/cd/abcd…      the blobs, fanned out over directories by the first two pairs of digits
//! <root>/tmp/blob-<id>.part     writes that are not finished; nothing else of the store is ever below tmp
//! ```
//!
//! The store is cheap to clone and every clone works on the same files. Any number of processes may share a
//! root: all coordination is through the file system (a new name is made with a hard link, which either makes
//! the file or says that it is there already).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{
    BlobError, BlobRead, BlobStore, ByteStream, Stored, relative_path,
};
use orch_types::{Clock, Hash, SystemClock};

use crate::{ranges, writes};

/// The directory below the root where unfinished writes live. A storage root has a `tmp/` for scratch files of
/// several kinds, so only files with the prefix [`INCOMING_PREFIX`] belong to this store.
pub const INCOMING_DIR: &str = "tmp";

/// How a file of an unfinished write is named: `blob-<id>.part`.
pub const INCOMING_PREFIX: &str = "blob-";

/// How a file of an unfinished write ends.
pub const INCOMING_SUFFIX: &str = ".part";

/// How long an unfinished write may have been silent before it is taken for dead and cleared away: a day. No upload
/// the API accepts goes on that long without a piece, and a dead write is one file of garbage, so the exact figure
/// matters little; a day keeps the sweep from ever touching a live writer.
pub const STALE_WRITE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The size of the pieces a read is cut into: 256 KiB, large enough that a read costs a syscall per quarter
/// megabyte and small enough that many concurrent readers hold little memory.
pub const READ_CHUNK_BYTES: usize = 256 * 1024;

/// How much a write collects before it goes to the disk: 256 KiB, the same as a read piece, so that a client
/// that sends small pieces does not cost a syscall for each.
pub const WRITE_BUFFER_BYTES: usize = 256 * 1024;

/// How a store behaves; the defaults are those of a server.
#[derive(Debug, Clone)]
pub struct StoreOptions {
    /// Whether a blob is on the disk for good before its write is reported: the file is flushed and so is its
    /// directory. A crash then never loses a blob that was reported stored.
    pub durable_writes: bool,
    /// Whether the read of a whole blob checks the bytes against the blob's name as they pass, and ends the
    /// stream with [`BlobError::Corrupt`] when they are not that content.
    pub verify_whole_reads: bool,
    /// How long an unfinished write may have been silent before [`LocalBlobStore::clear_stale_writes`] removes it.
    pub stale_write_age: Duration,
    /// The size of the pieces a read gives.
    pub read_chunk_bytes: usize,
    /// Where the time comes from, to tell how old an unfinished write is.
    pub clock: Arc<dyn Clock>,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            durable_writes: true,
            verify_whole_reads: true,
            stale_write_age: STALE_WRITE_AGE,
            read_chunk_bytes: READ_CHUNK_BYTES,
            clock: Arc::new(SystemClock),
        }
    }
}

/// What the clones of one store share.
#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) root: PathBuf,
    pub(crate) options: StoreOptions,
}

impl Inner {
    /// The file of a blob.
    pub(crate) fn path_of(&self, hash: &Hash) -> PathBuf {
        self.root.join(relative_path(hash))
    }

    /// The directory of unfinished writes.
    pub(crate) fn incoming_dir(&self) -> PathBuf {
        self.root.join(INCOMING_DIR)
    }
}

/// The blob store of a local directory.
#[derive(Debug, Clone)]
pub struct LocalBlobStore {
    inner: Arc<Inner>,
}

/// An I/O failure, as the contract states it. The detail is for the log: the error itself says what failed and
/// how, and nothing of the directory layout.
pub(crate) fn io_error(what: &str, error: &std::io::Error) -> BlobError {
    tracing::warn!(%error, what, "the blob store could not do it");
    BlobError::Io(format!("{what}: {}", error.kind()))
}

impl LocalBlobStore {
    /// Opens the store below `root` with the default options, making its directories when they are not there and
    /// clearing away the unfinished writes that a crash left.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, BlobError> {
        Self::open_with(root, StoreOptions::default()).await
    }

    /// Opens the store below `root` with the given options.
    pub async fn open_with(
        root: impl Into<PathBuf>,
        options: StoreOptions,
    ) -> Result<Self, BlobError> {
        let store = Self {
            inner: Arc::new(Inner {
                root: root.into(),
                options,
            }),
        };
        tokio::fs::create_dir_all(store.inner.root.join("blobs"))
            .await
            .map_err(|error| io_error("making the blob directory", &error))?;
        tokio::fs::create_dir_all(store.inner.incoming_dir())
            .await
            .map_err(|error| io_error("making the directory of unfinished writes", &error))?;
        let cleared = store.clear_stale_writes().await?;
        if cleared > 0 {
            tracing::info!(cleared, "cleared the unfinished writes of an earlier run");
        }
        Ok(store)
    }

    /// The directory the store keeps its files below.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Where the file of a blob is (or would be).
    #[must_use]
    pub fn path_of(&self, hash: &Hash) -> PathBuf {
        self.inner.path_of(hash)
    }

    /// Removes the unfinished writes that have been silent for longer than `stale_write_age`, and answers how
    /// many. A write in progress touches its file with every piece, so it is never among them. The API runs this
    /// from time to time, and the store runs it when it is opened.
    pub async fn clear_stale_writes(&self) -> Result<u64, BlobError> {
        writes::clear_stale(&self.inner).await
    }
}

#[async_trait]
impl BlobStore for LocalBlobStore {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        content: ByteStream,
    ) -> Result<Stored, BlobError> {
        writes::put(&self.inner, hash, size, content).await
    }

    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        ranges::get(&self.inner, hash, range).await
    }

    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        Ok(self.size(hash).await?.is_some())
    }

    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        match tokio::fs::metadata(self.inner.path_of(hash)).await {
            Ok(meta) if meta.is_file() => Ok(Some(i64::try_from(meta.len()).unwrap_or(i64::MAX))),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("looking at a blob", &error)),
        }
    }

    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        writes::delete(&self.inner, hash).await
    }
}
