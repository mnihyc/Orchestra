//! Where content goes to and comes from: the two small traits the connector implements with the link's content transfer, and stand-ins for tests.
//!
//! The environment host never talks to the network. When a file has to reach the browser, or a
//! snapshot the system, it hands the bytes to a [`BlobSink`]; when a write or a move needs content
//! the system holds, it asks a [`BlobSource`] for it. Both move content in the shape the link's
//! resumable transfer uses, so the connector's side of each is a few lines: a [`BlobReader`]
//! answers positional reads for an upload, a [`BlobWriter`] holds positional writes for a
//! download and can say how much it already has.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use bytes::Bytes;
use orch_types::Hash;

use crate::platform;

/// How many bytes one positional read of a file asks for when the caller does not say.
///
/// A mebibyte keeps the number of system calls low for a file of any size while a cut transfer
/// loses very little: the link itself sends pieces of between 64 KiB and 4 MiB.
pub const READ_CHUNK_BYTES: usize = 1024 * 1024;

/// Why content could not be given or taken.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
    /// The store will not take or give it.
    #[error("the content store refused: {0}")]
    Refused(String),
    /// The content did not have the hash or the size it was announced with.
    #[error("the content did not have the announced hash and size")]
    HashMismatch,
    /// Nothing moved for as long as a transfer waits.
    #[error("the transfer stood still")]
    StoodStill,
    /// The local side could not read or write.
    #[error("the local side failed: {0}")]
    Local(String),
}

/// Where the bytes of an upload come from.
#[async_trait]
pub trait BlobReader: Send {
    /// Up to `len` bytes from `offset`; fewer only at the end of the content.
    async fn read(&mut self, offset: i64, len: i64) -> io::Result<Bytes>;
}

/// Where the bytes of a download go.
#[async_trait]
pub trait BlobWriter: Send {
    /// How many bytes from the first are held already, so that a download goes on where it stopped.
    async fn received(&mut self) -> io::Result<i64>;
    /// Holds bytes that begin at `offset`, which is what `received` said or what the last write ended at.
    async fn write(&mut self, offset: i64, bytes: Bytes) -> io::Result<()>;
    /// The hash of everything held.
    async fn hash(&mut self) -> io::Result<Hash>;
    /// Throws everything held away.
    async fn discard(&mut self) -> io::Result<()>;
}

/// The content store as the environment puts content into it.
#[async_trait]
pub trait BlobSink: Send + Sync {
    /// Which of `hashes` the store lacks.
    async fn missing(&self, hashes: &[Hash]) -> Result<Vec<Hash>, BlobError>;
    /// Stores the `size` bytes `source` gives, whose hash is `hash`. The store checks both.
    async fn put(
        &self,
        hash: Hash,
        size: i64,
        source: &mut dyn BlobReader,
    ) -> Result<(), BlobError>;
}

/// The content store as the environment takes content out of it.
#[async_trait]
pub trait BlobSource: Send + Sync {
    /// Gives the content with `hash` to `target`, from where `target` already stands, and says how
    /// many bytes the content has.
    async fn get(&self, hash: Hash, target: &mut dyn BlobWriter) -> Result<i64, BlobError>;
}

/// Reads an open file by position; the file is opened and checked by whoever hands it over.
#[derive(Debug, Clone)]
pub struct FileReader {
    file: Arc<File>,
    transfer: Option<Arc<crate::snapshot::TransferWatch>>,
}

impl FileReader {
    /// A reader of `file`.
    #[must_use]
    pub fn new(file: Arc<File>) -> Self {
        Self {
            file,
            transfer: crate::snapshot::TransferWatch::current(),
        }
    }
}

#[async_trait]
impl BlobReader for FileReader {
    async fn read(&mut self, offset: i64, len: i64) -> io::Result<Bytes> {
        let file = Arc::clone(&self.file);
        let offset = u64::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?;
        let len = usize::try_from(len).map_err(|_| io::ErrorKind::InvalidInput)?;
        let bytes =
            crate::error::blocking_io(move || read_exact_or_end(&file, offset, len)).await?;
        if let Some(transfer) = &self.transfer {
            transfer.advance(
                i64::try_from(offset)
                    .unwrap_or(i64::MAX)
                    .saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
            );
        }
        Ok(bytes)
    }
}

/// Reads up to `len` bytes at `offset`, fewer only where the file ends.
pub(crate) fn read_exact_or_end(file: &File, offset: u64, len: usize) -> io::Result<Bytes> {
    let mut buffer = vec![0u8; len];
    let mut filled = 0usize;
    while filled < len {
        let at = offset + filled as u64;
        let count = platform::read_at(file, &mut buffer[filled..], at)?;
        if count == 0 {
            break;
        }
        filled += count;
    }
    buffer.truncate(filled);
    Ok(Bytes::from(buffer))
}

/// Reads bytes that are in memory.
#[derive(Debug, Clone)]
pub struct MemoryReader {
    bytes: Bytes,
}

impl MemoryReader {
    /// A reader of `bytes`.
    #[must_use]
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }
}

#[async_trait]
impl BlobReader for MemoryReader {
    async fn read(&mut self, offset: i64, len: i64) -> io::Result<Bytes> {
        let total = self.bytes.len();
        let from = usize::try_from(offset)
            .map_err(|_| io::ErrorKind::InvalidInput)?
            .min(total);
        let len = usize::try_from(len).map_err(|_| io::ErrorKind::InvalidInput)?;
        let to = from.saturating_add(len).min(total);
        Ok(self.bytes.slice(from..to))
    }
}

/// Holds a download in a file that its owner made and will rename or remove.
#[derive(Debug)]
pub struct FileWriter {
    file: Arc<File>,
    transfer: Option<Arc<crate::snapshot::TransferWatch>>,
}

impl FileWriter {
    /// A writer into `file`, which has to be empty or hold the start of the content.
    #[must_use]
    pub fn new(file: Arc<File>) -> Self {
        Self {
            file,
            transfer: crate::snapshot::TransferWatch::current(),
        }
    }
}

#[async_trait]
impl BlobWriter for FileWriter {
    async fn received(&mut self) -> io::Result<i64> {
        let file = Arc::clone(&self.file);
        let length = crate::error::blocking_io(move || file.metadata().map(|m| m.len())).await?;
        i64::try_from(length).map_err(io::Error::other)
    }

    async fn write(&mut self, offset: i64, bytes: Bytes) -> io::Result<()> {
        let end = offset.saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX));
        let file = Arc::clone(&self.file);
        let offset = u64::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?;
        crate::error::blocking_io(move || {
            let mut written = 0usize;
            while written < bytes.len() {
                crate::error::check_cancelled().map_err(io::Error::other)?;
                let count = platform::write_at(&file, &bytes[written..], offset + written as u64)?;
                if count == 0 {
                    return Err(io::Error::from(io::ErrorKind::WriteZero));
                }
                written += count;
            }
            Ok(())
        })
        .await?;
        if let Some(transfer) = &self.transfer {
            transfer.advance(end);
        }
        Ok(())
    }

    async fn hash(&mut self) -> io::Result<Hash> {
        let file = Arc::clone(&self.file);
        crate::error::blocking_io(move || hash_file(&file)).await
    }

    async fn discard(&mut self) -> io::Result<()> {
        let file = Arc::clone(&self.file);
        crate::error::blocking_io(move || file.set_len(0)).await
    }
}

/// The hash of the whole content of `file`, read by position so that its cursor is not moved.
pub(crate) fn hash_file(file: &File) -> io::Result<Hash> {
    hash_file_prefix(file, u64::MAX)
}

/// Hashes at most the captured length of an opened file. The caller checks its
/// identity afterwards, so a growing file cannot make a bounded read run on.
pub(crate) fn hash_file_prefix(file: &File, length: u64) -> io::Result<Hash> {
    let mut hasher = Hash::hasher();
    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    let mut offset = 0u64;
    while offset < length {
        crate::error::check_cancelled().map_err(io::Error::other)?;
        let wanted = buffer
            .len()
            .min(usize::try_from(length - offset).unwrap_or(usize::MAX));
        let count = platform::read_at(file, &mut buffer[..wanted], offset)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        offset += count as u64;
    }
    Ok(Hash::finish(&hasher))
}

/// Holds a download in memory.
#[derive(Debug, Default)]
pub struct MemoryWriter {
    bytes: Vec<u8>,
    limit: Option<usize>,
}

impl MemoryWriter {
    /// An empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty writer that refuses to hold more than `limit` bytes: for content whose size
    /// nobody has vouched for, such as a list of files that arrives from outside.
    #[must_use]
    pub fn with_limit(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit: Some(limit),
        }
    }

    /// What was written.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[async_trait]
impl BlobWriter for MemoryWriter {
    async fn received(&mut self) -> io::Result<i64> {
        i64::try_from(self.bytes.len()).map_err(io::Error::other)
    }

    async fn write(&mut self, offset: i64, bytes: Bytes) -> io::Result<()> {
        let offset = usize::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?;
        if offset != self.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a write must begin where the last one ended",
            ));
        }
        if self
            .limit
            .is_some_and(|limit| self.bytes.len().saturating_add(bytes.len()) > limit)
        {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "the content is larger than this download may be",
            ));
        }
        self.bytes.extend_from_slice(&bytes);
        Ok(())
    }

    async fn hash(&mut self) -> io::Result<Hash> {
        Ok(Hash::of(&self.bytes))
    }

    async fn discard(&mut self) -> io::Result<()> {
        self.bytes.clear();
        Ok(())
    }
}

/// A content store in memory, which checks what it is given as the system's store does. It stands
/// in for the link's transfer in tests of this package and of the connector.
#[derive(Debug, Clone, Default)]
pub struct MemoryBlobs {
    state: Arc<Mutex<MemoryState>>,
}

#[derive(Debug, Default)]
struct MemoryState {
    blobs: HashMap<Hash, Bytes>,
    puts: Vec<Hash>,
}

impl MemoryBlobs {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Puts content in without a transfer, as if the system held it already.
    pub fn insert(&self, content: impl Into<Bytes>) -> Hash {
        let content = content.into();
        let hash = Hash::of(&content);
        self.state().blobs.insert(hash, content);
        hash
    }

    /// The content with `hash`, if the store holds it.
    #[must_use]
    pub fn get_bytes(&self, hash: &Hash) -> Option<Bytes> {
        self.state().blobs.get(hash).cloned()
    }

    /// Whether the store holds the content with `hash`.
    #[must_use]
    pub fn holds(&self, hash: &Hash) -> bool {
        self.state().blobs.contains_key(hash)
    }

    /// Every upload that was taken, in order. Content the store held already is not uploaded again,
    /// so this is what a test asserts "only what the sink lacks" with.
    #[must_use]
    pub fn uploads(&self) -> Vec<Hash> {
        self.state().puts.clone()
    }

    /// How many blobs the store holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state().blobs.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl BlobSink for MemoryBlobs {
    async fn missing(&self, hashes: &[Hash]) -> Result<Vec<Hash>, BlobError> {
        let state = self.state();
        Ok(hashes
            .iter()
            .filter(|hash| !state.blobs.contains_key(hash))
            .copied()
            .collect())
    }

    async fn put(
        &self,
        hash: Hash,
        size: i64,
        source: &mut dyn BlobReader,
    ) -> Result<(), BlobError> {
        let mut content = Vec::new();
        let mut offset = 0i64;
        let piece = i64::try_from(READ_CHUNK_BYTES).unwrap_or(i64::MAX);
        while offset < size {
            let bytes = source
                .read(offset, piece.min(size - offset))
                .await
                .map_err(|error| BlobError::Local(error.to_string()))?;
            if bytes.is_empty() {
                return Err(BlobError::HashMismatch);
            }
            offset += i64::try_from(bytes.len()).unwrap_or(i64::MAX);
            content.extend_from_slice(&bytes);
        }
        if Hash::of(&content) != hash {
            return Err(BlobError::HashMismatch);
        }
        let mut state = self.state();
        state.puts.push(hash);
        state.blobs.insert(hash, Bytes::from(content));
        Ok(())
    }
}

#[async_trait]
impl BlobSource for MemoryBlobs {
    async fn get(&self, hash: Hash, target: &mut dyn BlobWriter) -> Result<i64, BlobError> {
        let content = self
            .get_bytes(&hash)
            .ok_or_else(|| BlobError::Refused(format!("{hash} is not held")))?;
        let size = i64::try_from(content.len()).unwrap_or(i64::MAX);
        let mut offset = target
            .received()
            .await
            .map_err(|error| BlobError::Local(error.to_string()))?;
        while offset < size {
            let end = (offset + i64::try_from(READ_CHUNK_BYTES).unwrap_or(i64::MAX)).min(size);
            let piece = content
                .slice(usize::try_from(offset).unwrap_or(0)..usize::try_from(end).unwrap_or(0));
            target
                .write(offset, piece)
                .await
                .map_err(|error| BlobError::Local(error.to_string()))?;
            offset = end;
        }
        Ok(size)
    }
}

/// Lets content that several requests want in the store at the same moment go up once: the others
/// wait, then find it there.
#[derive(Debug, Default)]
pub struct Uploads {
    gates: Mutex<HashMap<Hash, Arc<tokio::sync::Mutex<()>>>>,
}

impl Uploads {
    /// A gate with nobody waiting at it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes sure the store holds `hash`, uploading it from `source` only if no request before this
    /// one has; says whether this request did the upload.
    pub async fn ensure(
        &self,
        sink: &dyn BlobSink,
        hash: Hash,
        size: i64,
        source: &mut dyn BlobReader,
    ) -> Result<bool, BlobError> {
        let gate = {
            let mut gates = self.gates.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(gates.entry(hash).or_default())
        };
        let held = gate.lock().await;
        let result = async {
            if sink.missing(&[hash]).await?.is_empty() {
                return Ok(false);
            }
            sink.put(hash, size, source).await?;
            Ok(true)
        }
        .await;
        drop(held);
        {
            let mut gates = self.gates.lock().unwrap_or_else(PoisonError::into_inner);
            // The map holds one handle and this request one; any more is somebody waiting.
            if Arc::strong_count(&gate) <= 2 {
                gates.remove(&hash);
            }
        }
        result
    }
}

/// Downloads `hash` into `target` and checks the hash of what arrived; on a mismatch what arrived
/// is thrown away.
pub(crate) async fn download_checked(
    source: &dyn BlobSource,
    hash: Hash,
    target: &mut dyn BlobWriter,
) -> Result<i64, BlobError> {
    let size = source.get(hash, target).await?;
    let held = target
        .hash()
        .await
        .map_err(|error| BlobError::Local(error.to_string()))?;
    if held != hash {
        target
            .discard()
            .await
            .map_err(|error| BlobError::Local(error.to_string()))?;
        return Err(BlobError::HashMismatch);
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_store_takes_only_what_it_lacks_and_checks_what_it_is_given() {
        let store = MemoryBlobs::new();
        let held = store.insert(&b"already here"[..]);
        let new = Hash::of(b"new content");
        assert_eq!(store.missing(&[held, new]).await.unwrap(), vec![new]);

        let mut reader = MemoryReader::new(&b"new content"[..]);
        store.put(new, 11, &mut reader).await.unwrap();
        assert!(store.holds(&new));
        assert_eq!(store.uploads(), vec![new]);

        let mut wrong = MemoryReader::new(&b"other content"[..]);
        assert_eq!(
            store.put(Hash::of(b"claimed"), 13, &mut wrong).await,
            Err(BlobError::HashMismatch)
        );
    }

    #[tokio::test]
    async fn a_download_goes_on_where_the_writer_stands_and_is_checked() {
        let store = MemoryBlobs::new();
        let hash = store.insert(vec![7u8; 3 * READ_CHUNK_BYTES]);
        let mut writer = MemoryWriter::new();
        let size = download_checked(&store, hash, &mut writer).await.unwrap();
        assert_eq!(size, 3 * 1024 * 1024);
        assert_eq!(writer.into_bytes().len(), 3 * READ_CHUNK_BYTES);
    }
}
