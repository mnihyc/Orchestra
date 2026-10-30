//! Streaming range reads.
//!
//! A read opens the blob's file once, goes to the first byte asked for and gives the bytes as a stream of pieces,
//! each at most `read_chunk_bytes` long; nothing but one piece is held in memory. A read that is cut short by the
//! consumer costs nothing more. A blob that is deleted while it is being read is read to the end, as any open file
//! is.
//!
//! The read of a whole blob can check the bytes against the blob's name as they pass ([`crate::StoreOptions`]): the
//! stream then ends with [`BlobError::Corrupt`] when the file was damaged on the disk, so that no consumer takes
//! bad bytes for good ones without being told. A part of a blob is not checked: its hash is not known.

use std::io::{ErrorKind, SeekFrom};

use bytes::BytesMut;
use futures::StreamExt;
use orch_blob_contract::range::{BlobMeta, ByteRange, ResolvedRange};
use orch_blob_contract::store::{BlobError, BlobRead, ByteStream};
use orch_types::Hash;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::store::{Inner, io_error};

/// What a stream of a read carries from one piece to the next.
struct Reading {
    file: tokio::fs::File,
    /// How many bytes of the range are still to come.
    remaining: u64,
    chunk: usize,
    hash: Hash,
    /// The hash of the bytes so far, when the read is of the whole blob and is checked.
    check: Option<blake3::Hasher>,
}

/// Reads a blob, whole or by a range; see `BlobStore::get`.
pub(crate) async fn get(
    inner: &Inner,
    hash: &Hash,
    range: Option<ByteRange>,
) -> Result<BlobRead, BlobError> {
    let mut file = match tokio::fs::File::open(inner.path_of(hash)).await {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(BlobError::NotFound(*hash));
        }
        Err(error) => return Err(io_error("opening a blob", &error)),
    };
    let meta = file
        .metadata()
        .await
        .map_err(|error| io_error("looking at a blob", &error))?;
    if !meta.is_file() {
        return Err(BlobError::NotFound(*hash));
    }
    let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    let resolved = match range {
        None => ResolvedRange::whole(size),
        Some(range) => range
            .resolve(size)
            .map_err(|_| BlobError::RangeNotSatisfiable { size })?,
    };
    // `resolve` gives non-negative positions within the blob, so these conversions hold; a negative one
    // would be a fault of the contract, and reads nothing.
    let start = u64::try_from(resolved.start).unwrap_or(0);
    let remaining = u64::try_from(resolved.len).unwrap_or(0);
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|error| io_error("reading a blob", &error))?;
    }
    let check = (inner.options.verify_whole_reads && resolved.is_whole(size)).then(Hash::hasher);
    let reading = Reading {
        file,
        remaining,
        chunk: inner.options.read_chunk_bytes.max(1),
        hash: *hash,
        check,
    };
    Ok(BlobRead {
        meta: BlobMeta { hash: *hash, size },
        range: resolved,
        body: pieces(reading),
    })
}

/// The pieces of a read, one after the other; an error ends the stream.
fn pieces(reading: Reading) -> ByteStream {
    futures::stream::unfold(Some(reading), |state| async move {
        let mut reading = state?;
        if reading.remaining == 0 {
            if let Some(hasher) = reading.check.take()
                && Hash::finish(&hasher) != reading.hash
            {
                tracing::error!(hash = %reading.hash, "a stored blob is not the content of its name");
                return Some((Err(BlobError::Corrupt(reading.hash)), None));
            }
            return None;
        }
        let want = usize::try_from(reading.remaining).map_or(reading.chunk, |left| {
            left.min(reading.chunk)
        });
        let mut piece = BytesMut::with_capacity(want);
        let read = (&mut reading.file)
            .take(want as u64)
            .read_buf(&mut piece)
            .await;
        match read {
            Ok(0) => {
                // The file ended before the size it had when it was opened: it was cut short on the disk.
                tracing::error!(hash = %reading.hash, "a stored blob is shorter than it was");
                Some((Err(BlobError::Corrupt(reading.hash)), None))
            }
            Ok(read) => {
                reading.remaining = reading.remaining.saturating_sub(read as u64);
                if let Some(hasher) = reading.check.as_mut() {
                    hasher.update(&piece);
                }
                Some((Ok(piece.freeze()), Some(reading)))
            }
            Err(error) => Some((Err(io_error("reading a blob", &error)), None)),
        }
    })
    .boxed()
}
