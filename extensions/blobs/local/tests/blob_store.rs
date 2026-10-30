//! The local blob store against a real directory: what a write leaves, what a read gives, and what two writers do.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use futures::StreamExt;
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{
    BlobError, BlobStore, ByteStream, read_all, relative_path, stream_of,
};
use orch_blob_local::{LocalBlobStore, StoreOptions};
use orch_types::{Hash, ManualClock};

/// A store in a directory of its own, and the directory (removed when it is dropped).
async fn store() -> (tempfile::TempDir, LocalBlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalBlobStore::open(dir.path()).await.unwrap();
    (dir, store)
}

/// Content that is not periodic, so that a wrong offset shows.
fn content(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| u8::try_from((i * 7 + i / 251) % 256).unwrap())
        .collect()
}

/// The content in pieces of the given size.
fn pieces(content: &[u8], piece: usize) -> ByteStream {
    stream_of(content.chunks(piece).map(Bytes::copy_from_slice).collect())
}

/// Every file below a directory, however deep.
fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Blobs are read-only; the tests that play a damaged disk make the file writable first.
fn make_writable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}

async fn put(store: &LocalBlobStore, bytes: &[u8]) -> Hash {
    let hash = Hash::of(bytes);
    let stored = store
        .put(
            &hash,
            Some(i64::try_from(bytes.len()).unwrap()),
            pieces(bytes, 1000),
        )
        .await
        .unwrap();
    assert_eq!(stored.meta.hash, hash);
    hash
}

#[tokio::test]
async fn content_is_filed_by_its_hash_and_read_back_whole() {
    let (dir, store) = store().await;
    let bytes = content(5000);
    let hash = Hash::of(&bytes);
    let stored = store
        .put(&hash, Some(5000), pieces(&bytes, 777))
        .await
        .unwrap();
    assert!(stored.created);
    assert_eq!((stored.meta.hash, stored.meta.size), (hash, 5000));
    let at = dir.path().join(relative_path(&hash));
    assert!(at.is_file(), "{at:?}");
    assert_eq!(store.path_of(&hash), at);
    assert_eq!(std::fs::read(&at).unwrap(), bytes);
    let read = store.get(&hash, None).await.unwrap();
    assert!(read.range.is_whole(5000));
    assert_eq!(read.meta, stored.meta);
    assert_eq!(read_all(read.body).await.unwrap(), Bytes::from(bytes));
}

#[tokio::test]
async fn every_range_of_a_blob_is_what_a_read_of_the_whole_holds_at_that_place() {
    let (_dir, store) = store().await;
    let bytes = content(60);
    let hash = put(&store, &bytes).await;
    let size = 60_i64;
    let mut asked = vec![];
    for a in 0..=65 {
        asked.push(ByteRange::From { start: a });
        asked.push(ByteRange::Last { len: a });
        for b in a..=65 {
            asked.push(ByteRange::Between { start: a, end: b });
        }
    }
    for range in asked {
        match (range.resolve(size), store.get(&hash, Some(range)).await) {
            (Ok(expected), Ok(read)) => {
                assert_eq!(read.range, expected, "{range:?}");
                let from = usize::try_from(expected.start).unwrap();
                let to = usize::try_from(expected.end()).unwrap();
                assert_eq!(
                    read_all(read.body).await.unwrap(),
                    Bytes::copy_from_slice(&bytes[from..to]),
                    "{range:?}"
                );
            }
            (Err(_), Err(BlobError::RangeNotSatisfiable { size: told })) => {
                assert_eq!(told, size, "{range:?}");
            }
            (expected, got) => panic!("{range:?}: {expected:?} against {got:?}"),
        }
    }
}

#[tokio::test]
async fn a_range_across_many_pieces_arrives_in_pieces_no_larger_than_the_option() {
    let dir = tempfile::tempdir().unwrap();
    let options = StoreOptions {
        read_chunk_bytes: 100,
        ..StoreOptions::default()
    };
    let store = LocalBlobStore::open_with(dir.path(), options)
        .await
        .unwrap();
    let bytes = content(1050);
    let hash = put(&store, &bytes).await;
    let mut read = store
        .get(
            &hash,
            Some(ByteRange::Between {
                start: 5,
                end: 1004,
            }),
        )
        .await
        .unwrap();
    let mut total = 0;
    let mut count = 0;
    while let Some(piece) = read.body.next().await {
        let piece = piece.unwrap();
        assert!(piece.len() <= 100);
        total += piece.len();
        count += 1;
    }
    assert_eq!((total, count), (1000, 10));
}

#[tokio::test]
async fn an_empty_blob_is_stored_and_read_and_no_range_of_it_can_be_satisfied() {
    let (_dir, store) = store().await;
    let hash = put(&store, b"").await;
    assert_eq!(store.size(&hash).await.unwrap(), Some(0));
    let whole = store.get(&hash, None).await.unwrap();
    assert!(whole.range.is_whole(0));
    assert!(read_all(whole.body).await.unwrap().is_empty());
    assert_eq!(
        store
            .get(&hash, Some(ByteRange::From { start: 0 }))
            .await
            .unwrap_err(),
        BlobError::RangeNotSatisfiable { size: 0 }
    );
}

#[tokio::test]
async fn a_wrong_hash_is_refused_and_leaves_nothing() {
    let (dir, store) = store().await;
    let claimed = Hash::of(b"what was promised");
    let error = store
        .put(&claimed, None, pieces(b"something else", 5))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        BlobError::HashMismatch {
            claimed,
            actual: Hash::of(b"something else")
        }
    );
    assert!(!store.exists(&claimed).await.unwrap());
    assert_eq!(files_below(dir.path()), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn a_wrong_size_is_refused_and_leaves_nothing() {
    let (dir, store) = store().await;
    let bytes = content(300);
    let hash = Hash::of(&bytes);
    assert_eq!(
        store
            .put(&hash, Some(301), pieces(&bytes, 100))
            .await
            .unwrap_err(),
        BlobError::SizeMismatch {
            expected: 301,
            actual: 300
        }
    );
    // More than was stated stops at the piece that goes over.
    let over = store
        .put(&hash, Some(250), pieces(&bytes, 100))
        .await
        .unwrap_err();
    assert_eq!(
        over,
        BlobError::SizeMismatch {
            expected: 250,
            actual: 300
        }
    );
    assert!(!store.exists(&hash).await.unwrap());
    assert_eq!(files_below(dir.path()), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn a_hash_that_exists_does_not_excuse_content_that_is_not_it() {
    let (_dir, store) = store().await;
    let hash = put(&store, b"the real content").await;
    let error = store
        .put(&hash, None, pieces(b"not the real content", 4))
        .await
        .unwrap_err();
    assert!(matches!(error, BlobError::HashMismatch { .. }));
    let kept = store.get(&hash, None).await.unwrap();
    assert_eq!(read_all(kept.body).await.unwrap(), "the real content");
}

#[tokio::test]
async fn a_write_whose_content_fails_in_the_middle_leaves_nothing() {
    let (dir, store) = store().await;
    let hash = Hash::of(b"hello, world");
    let failing: ByteStream = futures::stream::iter(vec![
        Ok(Bytes::from_static(b"hello, ")),
        Err(BlobError::Io("the connection was reset".to_owned())),
    ])
    .boxed();
    assert_eq!(
        store.put(&hash, None, failing).await.unwrap_err(),
        BlobError::Io("the connection was reset".to_owned())
    );
    assert!(!store.exists(&hash).await.unwrap());
    assert_eq!(files_below(dir.path()), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn a_write_that_is_abandoned_before_it_ends_leaves_nothing() {
    let (dir, store) = store().await;
    let hash = Hash::of(b"never finished");
    let stalling: ByteStream = futures::stream::iter(vec![Ok(Bytes::from_static(b"never "))])
        .chain(futures::stream::pending())
        .boxed();
    let outcome =
        tokio::time::timeout(Duration::from_millis(200), store.put(&hash, None, stalling)).await;
    assert!(outcome.is_err(), "the write was cut off by the timeout");
    assert!(!store.exists(&hash).await.unwrap());
    assert_eq!(files_below(dir.path()), Vec::<PathBuf>::new());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_of_the_same_content_both_succeed_and_one_file_is_left() {
    let (dir, store) = store().await;
    let bytes = Arc::new(content(2_000_000));
    let hash = Hash::of(&bytes);
    let mut writers = Vec::new();
    for round in 0..16_usize {
        let store = store.clone();
        let bytes = Arc::clone(&bytes);
        writers.push(tokio::spawn(async move {
            // Pieces of different sizes, so that the writers are never in step.
            let piece = 1000 + round * 313;
            store.put(&hash, None, pieces(&bytes, piece)).await
        }));
    }
    let mut created = 0;
    for writer in writers {
        let stored = writer.await.unwrap().unwrap();
        assert_eq!(stored.meta.size, 2_000_000);
        created += usize::from(stored.created);
    }
    assert_eq!(created, 1, "exactly one write made the blob");
    let files = files_below(dir.path());
    assert_eq!(files, vec![dir.path().join(relative_path(&hash))]);
    assert_eq!(std::fs::read(&files[0]).unwrap(), *bytes);
}

#[tokio::test]
async fn the_same_content_written_twice_is_stored_once_and_the_second_says_so() {
    let (dir, store) = store().await;
    let hash = Hash::of(b"abc");
    let first = store.put(&hash, None, pieces(b"abc", 1)).await.unwrap();
    let second = store.put(&hash, None, pieces(b"abc", 2)).await.unwrap();
    assert!(first.created && !second.created);
    assert_eq!(first.meta, second.meta);
    assert_eq!(files_below(dir.path()).len(), 1);
}

#[tokio::test]
async fn exists_size_and_delete_follow_the_files() {
    let (_dir, store) = store().await;
    let hash = Hash::of(b"here today");
    assert!(!store.exists(&hash).await.unwrap());
    assert_eq!(store.size(&hash).await.unwrap(), None);
    assert!(
        !store.delete(&hash).await.unwrap(),
        "removing what is not there is no error"
    );
    assert_eq!(
        store.get(&hash, None).await.unwrap_err(),
        BlobError::NotFound(hash)
    );
    put(&store, b"here today").await;
    assert!(store.exists(&hash).await.unwrap());
    assert_eq!(store.size(&hash).await.unwrap(), Some(10));
    assert!(store.delete(&hash).await.unwrap());
    assert!(!store.exists(&hash).await.unwrap());
    assert!(!store.delete(&hash).await.unwrap());
    // And the same content can be put again afterwards.
    let again = store
        .put(&hash, None, pieces(b"here today", 3))
        .await
        .unwrap();
    assert!(again.created);
}

#[tokio::test]
async fn a_reader_goes_on_when_the_blob_is_deleted_under_it() {
    let dir = tempfile::tempdir().unwrap();
    let options = StoreOptions {
        read_chunk_bytes: 64,
        ..StoreOptions::default()
    };
    let store = LocalBlobStore::open_with(dir.path(), options)
        .await
        .unwrap();
    let bytes = content(1000);
    let hash = put(&store, &bytes).await;
    let mut read = store.get(&hash, None).await.unwrap();
    let first = read.body.next().await.unwrap().unwrap();
    assert!(store.delete(&hash).await.unwrap());
    let mut got = first.to_vec();
    while let Some(piece) = read.body.next().await {
        got.extend_from_slice(&piece.unwrap());
    }
    assert_eq!(got, bytes);
}

#[tokio::test]
async fn a_blob_damaged_on_the_disk_is_reported_by_a_read_of_the_whole_and_not_taken_for_good() {
    let (_dir, store) = store().await;
    let bytes = content(4000);
    let hash = put(&store, &bytes).await;
    let path = store.path_of(&hash);
    let mut damaged = bytes.clone();
    damaged[2500] ^= 0xff;
    make_writable(&path);
    std::fs::write(&path, &damaged).unwrap();
    let whole = store.get(&hash, None).await.unwrap();
    assert_eq!(
        read_all(whole.body).await.unwrap_err(),
        BlobError::Corrupt(hash)
    );
    // A part is not checked: its hash is not known.
    let part = store
        .get(&hash, Some(ByteRange::Between { start: 0, end: 99 }))
        .await
        .unwrap();
    assert_eq!(
        read_all(part.body).await.unwrap(),
        Bytes::copy_from_slice(&bytes[..100])
    );
}

#[tokio::test]
async fn a_blob_cut_short_on_the_disk_is_reported_when_the_read_comes_to_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let options = StoreOptions {
        read_chunk_bytes: 100,
        ..StoreOptions::default()
    };
    let store = LocalBlobStore::open_with(dir.path(), options)
        .await
        .unwrap();
    let bytes = content(1000);
    let hash = put(&store, &bytes).await;
    let mut read = store.get(&hash, None).await.unwrap();
    let path = store.path_of(&hash);
    make_writable(&path);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(300)
        .unwrap();
    let mut failed = None;
    while let Some(piece) = read.body.next().await {
        if let Err(error) = piece {
            failed = Some(error);
            break;
        }
    }
    assert_eq!(failed, Some(BlobError::Corrupt(hash)));
}

#[cfg(unix)]
#[tokio::test]
async fn a_blob_in_place_cannot_be_opened_for_writing() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, store) = store().await;
    let hash = put(&store, b"immutable").await;
    let mode = std::fs::metadata(store.path_of(&hash))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o222,
        0,
        "no write permission, whoever asks: {mode:o}"
    );
}

#[tokio::test]
async fn unfinished_writes_of_a_crashed_run_are_cleared_when_the_store_is_opened() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    let day = Duration::from_secs(24 * 60 * 60);
    let old = dir.path().join("tmp/blob-0001.part");
    let young = dir.path().join("tmp/blob-0002.part");
    let foreign = dir.path().join("tmp/upload-0003.part");
    for path in [&old, &young, &foreign] {
        std::fs::write(path, b"partial").unwrap();
    }
    for path in [&old, &foreign] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - 3 * day)
            .unwrap();
    }
    let store = LocalBlobStore::open(dir.path()).await.unwrap();
    assert!(!old.exists(), "a write silent for three days is dead");
    assert!(
        young.exists(),
        "a write that just touched its file may be alive"
    );
    assert!(
        foreign.exists(),
        "only files of this store's own are ever removed"
    );
    assert_eq!(store.clear_stale_writes().await.unwrap(), 0);
}

#[tokio::test]
async fn how_old_an_unfinished_write_is_comes_from_the_clock_given() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    let path = dir.path().join("tmp/blob-0004.part");
    std::fs::write(&path, b"partial").unwrap();
    let clock = ManualClock::new(chrono_now());
    let options = StoreOptions {
        clock: Arc::new(clock.clone()),
        stale_write_age: Duration::from_secs(600),
        ..StoreOptions::default()
    };
    let store = LocalBlobStore::open_with(dir.path(), options)
        .await
        .unwrap();
    assert!(path.exists(), "no time has passed");
    clock.advance(chrono_duration(Duration::from_secs(601)));
    assert_eq!(store.clear_stale_writes().await.unwrap(), 1);
    assert!(!path.exists());
}

/// The present moment as the clock type wants it.
fn chrono_now() -> chrono::DateTime<chrono::Utc> {
    SystemTime::now().into()
}

fn chrono_duration(by: Duration) -> chrono::Duration {
    chrono::Duration::from_std(by).unwrap()
}

#[tokio::test]
async fn a_store_whose_scratch_directory_was_removed_makes_it_again() {
    let (dir, store) = store().await;
    std::fs::remove_dir_all(dir.path().join("tmp")).unwrap();
    let hash = put(&store, b"still works").await;
    assert!(store.exists(&hash).await.unwrap());
}

#[tokio::test]
async fn two_stores_on_one_directory_see_each_others_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let one = LocalBlobStore::open(dir.path()).await.unwrap();
    let other = LocalBlobStore::open(dir.path()).await.unwrap();
    let hash = put(&one, b"shared").await;
    assert!(other.exists(&hash).await.unwrap());
    let again = other.put(&hash, None, pieces(b"shared", 2)).await.unwrap();
    assert!(!again.created);
}

#[tokio::test]
async fn a_blob_larger_than_a_read_piece_comes_back_exactly_in_many_pieces() {
    let (_dir, store) = store().await;
    let bytes = content(3 * 1024 * 1024 + 17);
    let hash = put(&store, &bytes).await;
    let mut read = store.get(&hash, None).await.unwrap();
    let mut got = Vec::with_capacity(bytes.len());
    let mut count = 0;
    while let Some(piece) = read.body.next().await {
        got.extend_from_slice(&piece.unwrap());
        count += 1;
    }
    assert!(count > 1);
    assert_eq!(Hash::of(&got), hash);
}
