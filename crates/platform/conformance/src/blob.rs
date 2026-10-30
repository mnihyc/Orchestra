//! The suite every blob store passes: put, ranged get, existence, delete and size by content address.
//!
//! Run this only against a store reserved for the check. Each run uses content with a fresh identifier;
//! it removes only those addresses afterwards. Failed writes must leave no readable content.

use std::time::Duration;

use bytes::Bytes;
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobStore, read_all, stream_of};
use orch_types::{Hash, new_id};

use crate::report::{Checked, Report};

/// A small local or remote object should complete each operation within a minute.
const WITHIN: Duration = Duration::from_secs(60);

/// Checks content addressing, idempotence, exact range bytes, failed writes and removal.
pub async fn run(store: &dyn BlobStore, report: &mut Report) {
    let content = Bytes::from(format!("conformance:{}:abcdefgh", new_id()));
    let hash = Hash::of(&content);
    report
        .run(
            "blob.put",
            "Content is stored under its hash once",
            WITHIN,
            async {
                let first = store
                    .put(&hash, None, stream_of(vec![content.clone()]))
                    .await
                    .map_err(|error| error.to_string())?;
                let second = store
                    .put(&hash, None, stream_of(vec![content.clone()]))
                    .await
                    .map_err(|error| error.to_string())?;
                if !first.created
                    || second.created
                    || first.meta.hash != hash
                    || first.meta.size != i64::try_from(content.len()).unwrap_or(i64::MAX)
                {
                    return Err("the first write did not create exactly one content address".into());
                }
                Ok(())
            },
        )
        .await;
    report
        .run(
            "blob.read",
            "Whole and suffix reads return the exact stored bytes",
            WITHIN,
            async {
                let whole = store
                    .get(&hash, None)
                    .await
                    .map_err(|error| error.to_string())?;
                let all = read_all(whole.body)
                    .await
                    .map_err(|error| error.to_string())?;
                let suffix = store
                    .get(&hash, Some(ByteRange::Last { len: 8 }))
                    .await
                    .map_err(|error| error.to_string())?;
                let range = suffix.range;
                let last = read_all(suffix.body)
                    .await
                    .map_err(|error| error.to_string())?;
                if all != content || last.as_ref() != b"abcdefgh" || range.len != 8 {
                    return Err("a read returned bytes other than the requested content".into());
                }
                if !store
                    .exists(&hash)
                    .await
                    .map_err(|error| error.to_string())?
                    || store.size(&hash).await.map_err(|error| error.to_string())?
                        != Some(i64::try_from(content.len()).unwrap_or(i64::MAX))
                {
                    return Err("existence or size disagrees with the readable blob".into());
                }
                Ok(())
            },
        )
        .await;
    let wrong = Hash::of(format!("not-the-content:{}", new_id()).as_bytes());
    report
        .run(
            "blob.hash",
            "A wrong hash is refused and stores nothing",
            WITHIN,
            async {
                if !matches!(
                    store
                        .put(&wrong, None, stream_of(vec![content.clone()]))
                        .await,
                    Err(BlobError::HashMismatch { .. })
                ) {
                    return Err(
                        "content offered under the wrong hash was not refused as hash_mismatch"
                            .into(),
                    );
                }
                absent(store, &wrong).await
            },
        )
        .await;
    let short = Bytes::from(format!("wrong-size:{}", new_id()));
    let short_hash = Hash::of(&short);
    report
        .run(
            "blob.size",
            "A wrong declared size is refused and stores nothing",
            WITHIN,
            async {
                let wrong_size = i64::try_from(short.len())
                    .unwrap_or(i64::MAX)
                    .saturating_add(1);
                if !matches!(
                    store
                        .put(&short_hash, Some(wrong_size), stream_of(vec![short]))
                        .await,
                    Err(BlobError::SizeMismatch { .. })
                ) {
                    return Err(
                        "content of a different size was not refused as size_mismatch".into(),
                    );
                }
                absent(store, &short_hash).await
            },
        )
        .await;
    report
        .run(
            "blob.range",
            "A range past the content is refused with its size",
            WITHIN,
            async {
                let size = i64::try_from(content.len()).unwrap_or(i64::MAX);
                match store
                    .get(&hash, Some(ByteRange::From { start: size }))
                    .await
                {
                    Err(BlobError::RangeNotSatisfiable { size: said }) if said == size => Ok(()),
                    _ => Err("an unsatisfiable range did not report the blob's actual size".into()),
                }
            },
        )
        .await;
    report
        .run(
            "blob.delete",
            "Removal is idempotent and leaves no readable blob",
            WITHIN,
            async {
                if !store
                    .delete(&hash)
                    .await
                    .map_err(|error| error.to_string())?
                    || store
                        .delete(&hash)
                        .await
                        .map_err(|error| error.to_string())?
                {
                    return Err("the first removal did not remove it exactly once".into());
                }
                absent(store, &hash).await
            },
        )
        .await;
    // Even a store that broke a rule must not retain the suite's content.
    for address in [hash, wrong, short_hash] {
        let _ = tokio::time::timeout(WITHIN, store.delete(&address)).await;
    }
}

async fn absent(store: &dyn BlobStore, hash: &Hash) -> Checked {
    if store
        .exists(hash)
        .await
        .map_err(|error| error.to_string())?
        || store
            .size(hash)
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        || !matches!(store.get(hash, None).await, Err(BlobError::NotFound(_)))
    {
        return Err("content that must be absent can still be found or read".into());
    }
    Ok(())
}
