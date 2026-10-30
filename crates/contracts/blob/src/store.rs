//! Content-addressed put, get, exists, delete and size operations.
//!
//! The blob store holds files by their content. A blob is named by the BLAKE3-256 hash of its bytes, so a
//! name never changes what it names, the same content is stored once, and an object store needs no change to
//! the schema. The local directory is the one implementation in `v0.1.0`; the trait is the seam.
//!
//! | Method | Does |
//! |--------|------|
//! | `put` | Stores content under the hash it claims, after checking that it is that content; idempotent |
//! | `get` | Reads a blob whole or by a range, as a stream |
//! | `exists`, `size` | Whether a blob is there, and how large |
//! | `delete` | Removes a blob; removing one that is not there is no error |
//!
//! A write that is wrong leaves nothing: a hash that is not the content's, or a size that is not the
//! stated one, is refused ([`Verifier`]). Two writers of the same content both succeed, with one file.
//! Blob files live at `<root>/blobs/<hex[0:2]>/<hex[2:4]>/<hex>` ([`relative_path`]).

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use orch_types::Hash;

use crate::range::{BlobMeta, ByteRange, ResolvedRange};

/// Content as a stream of pieces; the error of a piece ends the stream.
pub type ByteStream = BoxStream<'static, Result<Bytes, BlobError>>;

/// Why an operation of the store did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
    /// There is no blob of that hash.
    #[error("there is no blob {0}")]
    NotFound(Hash),
    /// The content is not the content of the hash it was offered under; nothing was stored.
    #[error("the content hashes to {actual}, not to {claimed}")]
    HashMismatch {
        /// The hash it was offered under.
        claimed: Hash,
        /// The hash it has.
        actual: Hash,
    },
    /// The content is not the size that was stated; nothing was stored.
    #[error("the content is {actual} bytes, not the {expected} that were stated")]
    SizeMismatch {
        /// The size stated.
        expected: i64,
        /// The size it has.
        actual: i64,
    },
    /// The range holds no byte of the blob.
    #[error("the range holds no byte of a blob of {size} bytes")]
    RangeNotSatisfiable {
        /// The blob's size.
        size: i64,
    },
    /// The stored bytes are not what their name says: the store has been damaged.
    #[error("the stored content of {0} is damaged")]
    Corrupt(Hash),
    /// The store could not do it.
    #[error("the store failed: {0}")]
    Io(String),
}

/// What storing content did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stored {
    /// The blob.
    pub meta: BlobMeta,
    /// Whether this write made it; `false` when it was there already (or another writer made it first).
    pub created: bool,
}

/// What reading a blob gives: the blob, which of its bytes, and those bytes.
pub struct BlobRead {
    /// The blob that was read.
    pub meta: BlobMeta,
    /// The bytes that follow: the whole, or the range asked for.
    pub range: ResolvedRange,
    /// Exactly `range.len` bytes, in pieces.
    pub body: ByteStream,
}

impl std::fmt::Debug for BlobRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobRead")
            .field("meta", &self.meta)
            .field("range", &self.range)
            .finish_non_exhaustive()
    }
}

/// One implementation, used by `orch-api`, which owns the store.
#[async_trait]
pub trait BlobStore: Send + Sync + 'static {
    /// Stores `content` under `hash`, after checking that it hashes to it and, where `size` is stated, that
    /// it has that many bytes. A wrong one is refused and leaves nothing. A blob that is there already is
    /// not an error: the store keeps one, and `created` is `false`.
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        content: ByteStream,
    ) -> Result<Stored, BlobError>;

    /// Reads a blob, whole or the range asked for. A range that holds no byte of it is
    /// [`BlobError::RangeNotSatisfiable`].
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError>;

    /// Whether the blob is there.
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError>;

    /// How large the blob is; `None` when it is not there.
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError>;

    /// Removes the blob. `true` if it was there; removing one that is not is no error.
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError>;
}

/// The path of a blob below the store's root: `blobs/<hex[0:2]>/<hex[2:4]>/<hex>`. The first two pairs of
/// digits fan the files out over directories, so that none grows large.
#[must_use]
pub fn relative_path(hash: &Hash) -> String {
    let hex = hash.to_string();
    format!("blobs/{}/{}/{hex}", &hex[0..2], &hex[2..4])
}

/// Checks content against the hash it claims as it passes, piece by piece, without holding it: a store feeds
/// it every piece it writes and asks at the end.
pub struct Verifier {
    claimed: Hash,
    expected_size: Option<i64>,
    hasher: blake3::Hasher,
    seen: i64,
}

impl Verifier {
    /// A check of content that is to hash to `claimed` and, where stated, to have `expected_size` bytes.
    #[must_use]
    pub fn new(claimed: Hash, expected_size: Option<i64>) -> Self {
        Self {
            claimed,
            expected_size,
            hasher: Hash::hasher(),
            seen: 0,
        }
    }

    /// Takes the next piece of the content.
    pub fn update(&mut self, piece: &[u8]) {
        self.hasher.update(piece);
        self.seen += i64::try_from(piece.len()).unwrap_or(i64::MAX);
    }

    /// How many bytes have been seen.
    #[must_use]
    pub const fn seen(&self) -> i64 {
        self.seen
    }

    /// Ends the check: the content is what was claimed, or the error says how it is not.
    pub fn finish(self) -> Result<BlobMeta, BlobError> {
        if let Some(expected) = self.expected_size
            && expected != self.seen
        {
            return Err(BlobError::SizeMismatch {
                expected,
                actual: self.seen,
            });
        }
        let actual = Hash::finish(&self.hasher);
        if actual == self.claimed {
            Ok(BlobMeta {
                hash: actual,
                size: self.seen,
            })
        } else {
            Err(BlobError::HashMismatch {
                claimed: self.claimed,
                actual,
            })
        }
    }
}

/// A stream of the given pieces, for tests and for a store that already holds the bytes.
#[must_use]
pub fn stream_of(pieces: Vec<Bytes>) -> ByteStream {
    futures::stream::iter(pieces.into_iter().map(Ok)).boxed()
}

/// Reads a stream to the end.
pub async fn read_all(mut stream: ByteStream) -> Result<Bytes, BlobError> {
    let mut all = Vec::new();
    while let Some(piece) = stream.next().await {
        all.extend_from_slice(&piece?);
    }
    Ok(Bytes::from(all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A store in memory: the least that keeps the contract.
    #[derive(Default)]
    struct Memory {
        blobs: Mutex<BTreeMap<Hash, Bytes>>,
    }

    #[async_trait]
    impl BlobStore for Memory {
        async fn put(
            &self,
            hash: &Hash,
            size: Option<i64>,
            content: ByteStream,
        ) -> Result<Stored, BlobError> {
            let mut verifier = Verifier::new(*hash, size);
            let mut all = Vec::new();
            let mut content = content;
            while let Some(piece) = content.next().await {
                let piece = piece?;
                verifier.update(&piece);
                all.extend_from_slice(&piece);
            }
            let meta = verifier.finish()?;
            let created = self
                .blobs
                .lock()
                .unwrap()
                .insert(*hash, Bytes::from(all))
                .is_none();
            Ok(Stored { meta, created })
        }
        async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
            let blobs = self.blobs.lock().unwrap();
            let content = blobs.get(hash).ok_or(BlobError::NotFound(*hash))?;
            let size = i64::try_from(content.len()).unwrap();
            let range = match range {
                None => ResolvedRange::whole(size),
                Some(range) => range
                    .resolve(size)
                    .map_err(|_| BlobError::RangeNotSatisfiable { size })?,
            };
            let from = usize::try_from(range.start).unwrap();
            let to = usize::try_from(range.end()).unwrap();
            Ok(BlobRead {
                meta: BlobMeta { hash: *hash, size },
                range,
                body: stream_of(vec![content.slice(from..to)]),
            })
        }
        async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
            Ok(self.blobs.lock().unwrap().contains_key(hash))
        }
        async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
            Ok(self
                .blobs
                .lock()
                .unwrap()
                .get(hash)
                .map(|content| i64::try_from(content.len()).unwrap()))
        }
        async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
            Ok(self.blobs.lock().unwrap().remove(hash).is_some())
        }
    }

    fn pieces(parts: &[&'static str]) -> ByteStream {
        stream_of(
            parts
                .iter()
                .map(|part| Bytes::from_static(part.as_bytes()))
                .collect(),
        )
    }

    #[test]
    fn content_is_stored_under_its_hash_read_whole_or_by_range_and_removed() {
        let store = Memory::default();
        let hash = Hash::of(b"hello, world");
        let stored = block_on(store.put(&hash, Some(12), pieces(&["hello, ", "world"]))).unwrap();
        assert!(stored.created);
        assert_eq!(stored.meta, BlobMeta { hash, size: 12 });
        assert!(block_on(store.exists(&hash)).unwrap());
        assert_eq!(block_on(store.size(&hash)).unwrap(), Some(12));
        let whole = block_on(store.get(&hash, None)).unwrap();
        assert!(whole.range.is_whole(12));
        assert_eq!(block_on(read_all(whole.body)).unwrap(), "hello, world");
        let part = block_on(store.get(&hash, Some(ByteRange::parse("bytes=7-").unwrap()))).unwrap();
        assert_eq!((part.range.start, part.range.len), (7, 5));
        assert_eq!(block_on(read_all(part.body)).unwrap(), "world");
        let last = block_on(store.get(&hash, Some(ByteRange::Last { len: 3 }))).unwrap();
        assert_eq!(block_on(read_all(last.body)).unwrap(), "rld");
        assert_eq!(
            block_on(store.get(&hash, Some(ByteRange::From { start: 12 }))).unwrap_err(),
            BlobError::RangeNotSatisfiable { size: 12 }
        );
        assert_eq!(block_on(store.delete(&hash)), Ok(true));
        assert_eq!(
            block_on(store.delete(&hash)),
            Ok(false),
            "removing what is not there is no error"
        );
        assert_eq!(block_on(store.size(&hash)).unwrap(), None);
        assert_eq!(
            block_on(store.get(&hash, None)).unwrap_err(),
            BlobError::NotFound(hash)
        );
    }

    #[test]
    fn a_range_of_a_blob_is_what_a_read_of_the_whole_holds_at_that_place() {
        let store = Memory::default();
        let data = "0123456789".repeat(10);
        let hash = Hash::of(data.as_bytes());
        block_on(store.put(&hash, None, stream_of(vec![Bytes::from(data.clone())]))).unwrap();
        let whole = block_on(read_all(block_on(store.get(&hash, None)).unwrap().body)).unwrap();
        for header in [
            "bytes=0-9",
            "bytes=95-",
            "bytes=-7",
            "bytes=33-66",
            "bytes=99-99",
        ] {
            let range = ByteRange::parse(header).unwrap();
            let read = block_on(store.get(&hash, Some(range))).unwrap();
            let resolved = read.range;
            let body = block_on(read_all(read.body)).unwrap();
            let from = usize::try_from(resolved.start).unwrap();
            assert_eq!(body, whole.slice(from..from + body.len()), "{header}");
            assert_eq!(i64::try_from(body.len()).unwrap(), resolved.len, "{header}");
        }
    }

    #[test]
    fn a_wrong_hash_or_a_wrong_size_is_refused_and_leaves_nothing() {
        let store = Memory::default();
        let claimed = Hash::of(b"what was promised");
        let error = block_on(store.put(&claimed, None, pieces(&["something else"]))).unwrap_err();
        assert_eq!(
            error,
            BlobError::HashMismatch {
                claimed,
                actual: Hash::of(b"something else")
            }
        );
        assert!(!block_on(store.exists(&claimed)).unwrap());
        let hash = Hash::of(b"abc");
        assert_eq!(
            block_on(store.put(&hash, Some(4), pieces(&["abc"]))).unwrap_err(),
            BlobError::SizeMismatch {
                expected: 4,
                actual: 3
            }
        );
        assert!(!block_on(store.exists(&hash)).unwrap());
        assert!(store.blobs.lock().unwrap().is_empty());
    }

    #[test]
    fn the_same_content_written_twice_is_stored_once_and_both_writes_succeed() {
        let store = Memory::default();
        let hash = Hash::of(b"abc");
        let first = block_on(store.put(&hash, None, pieces(&["abc"]))).unwrap();
        let second = block_on(store.put(&hash, None, pieces(&["a", "bc"]))).unwrap();
        assert!(first.created && !second.created);
        assert_eq!(store.blobs.lock().unwrap().len(), 1);
        let empty = Hash::of(b"");
        assert_eq!(
            block_on(store.put(&empty, Some(0), stream_of(vec![])))
                .unwrap()
                .meta
                .size,
            0
        );
    }

    #[test]
    fn a_verifier_checks_pieces_as_they_pass_without_holding_them() {
        let mut verifier = Verifier::new(Hash::of(b"abcdef"), Some(6));
        verifier.update(b"abc");
        assert_eq!(verifier.seen(), 3);
        verifier.update(b"def");
        assert_eq!(
            verifier.finish(),
            Ok(BlobMeta {
                hash: Hash::of(b"abcdef"),
                size: 6
            })
        );
        let mut short = Verifier::new(Hash::of(b"abcdef"), None);
        short.update(b"abc");
        assert!(matches!(
            short.finish(),
            Err(BlobError::HashMismatch { .. })
        ));
    }

    #[test]
    fn a_blob_is_filed_under_the_first_two_pairs_of_digits_of_its_hash() {
        let hash: Hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .parse()
            .unwrap();
        assert_eq!(
            relative_path(&hash),
            "blobs/01/23/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }
}
