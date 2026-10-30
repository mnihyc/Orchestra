//! Verified atomic content insertion and deletion.
//!
//! A write never touches the name it is for until the content is known to be right. The pieces go to a file of
//! their own below `tmp/` and through a [`Verifier`], which checks them against the hash and the size claimed; only
//! when both are right is the file linked to the blob's name. A hard link either makes the name or says it is there
//! already, so two writers of the same content race harmlessly: one of them made the blob, and both succeed.
//!
//! Whatever ends a write early (a failed piece, a refused hash, a dropped future, a cancelled task) removes its file;
//! what a crash of the whole process leaves is cleared by the next start
//! ([`crate::LocalBlobStore::clear_stale_writes`]). The blob appears with all of its content or not at all.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use futures::StreamExt;
use orch_blob_contract::store::{BlobError, ByteStream, Stored, Verifier};
use orch_types::{Hash, new_id};
use tokio::io::{AsyncWriteExt, BufWriter};

use crate::store::{INCOMING_PREFIX, INCOMING_SUFFIX, Inner, WRITE_BUFFER_BYTES, io_error};

/// The permissions of a blob once it is in place: readable by everyone who can read the directory and by no one
/// writable, so that a program that opens a blob for writing by mistake is refused.
#[cfg(unix)]
const BLOB_FILE_MODE: u32 = 0o444;

/// The file of a write that is not finished. Dropping it removes the file, which is all a write that fails, is
/// refused or is abandoned has to do; after a success it removes the temporary name of the blob that now has its
/// own.
struct Unfinished {
    path: PathBuf,
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(%error, "an unfinished blob write could not be removed");
        }
    }
}

/// Makes a file for a new write, and the guard that removes it.
async fn create_incoming(inner: &Inner) -> Result<(tokio::fs::File, Unfinished), BlobError> {
    let dir = inner.incoming_dir();
    let name = format!("{INCOMING_PREFIX}{}{INCOMING_SUFFIX}", new_id().simple());
    let path = dir.join(name);
    let open = |path: &Path| {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let path = path.to_owned();
        async move { options.open(path).await }
    };
    let file = match open(&path).await {
        Ok(file) => file,
        // The directory is made when the store is opened; somebody may have removed it since.
        Err(error) if error.kind() == ErrorKind::NotFound => {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|error| io_error("making the directory of unfinished writes", &error))?;
            open(&path)
                .await
                .map_err(|error| io_error("starting a blob write", &error))?
        }
        Err(error) => return Err(io_error("starting a blob write", &error)),
    };
    Ok((file, Unfinished { path }))
}

/// Stores `content` under `hash`; see `BlobStore::put`.
pub(crate) async fn put(
    inner: &Inner,
    hash: &Hash,
    size: Option<i64>,
    mut content: ByteStream,
) -> Result<Stored, BlobError> {
    let (file, unfinished) = create_incoming(inner).await?;
    let mut writer = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
    let mut verifier = Verifier::new(*hash, size);
    while let Some(piece) = content.next().await {
        let piece = piece?;
        verifier.update(&piece);
        if let Some(expected) = size
            && verifier.seen() > expected
        {
            // More has come than was stated. Nothing more is written or waited for; `actual` is how many
            // bytes had come when it was seen.
            return Err(BlobError::SizeMismatch {
                expected,
                actual: verifier.seen(),
            });
        }
        writer
            .write_all(&piece)
            .await
            .map_err(|error| io_error("writing a blob", &error))?;
    }
    writer
        .flush()
        .await
        .map_err(|error| io_error("writing a blob", &error))?;
    let meta = verifier.finish()?;
    let file = writer.into_inner();
    let durable = inner.options.durable_writes;
    if durable {
        file.sync_all()
            .await
            .map_err(|error| io_error("flushing a blob", &error))?;
    }
    drop(file);
    make_read_only(&unfinished.path).await;
    let target = inner.path_of(hash);
    let created = install(&unfinished.path, &target, durable)
        .await
        .map_err(|error| io_error("putting a blob in place", &error))?;
    Ok(Stored { meta, created })
}

/// Removes the file of a blob; see `BlobStore::delete`.
pub(crate) async fn delete(inner: &Inner, hash: &Hash) -> Result<bool, BlobError> {
    match tokio::fs::remove_file(inner.path_of(hash)).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("removing a blob", &error)),
    }
}

/// Removes the unfinished writes that have been silent for longer than the options allow.
pub(crate) async fn clear_stale(inner: &Inner) -> Result<u64, BlobError> {
    let mut entries = match tokio::fs::read_dir(inner.incoming_dir()).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_error("looking for unfinished writes", &error)),
    };
    let now: SystemTime = inner.options.clock.now().into();
    let mut cleared = 0;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("looking for unfinished writes", &error))?
    {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with(INCOMING_PREFIX) && name.ends_with(INCOMING_SUFFIX)) {
            continue;
        }
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        let silent_for = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok());
        // A file from the future (the clock was set back) or without a time is young, never stale.
        if silent_for.is_some_and(|age| age > inner.options.stale_write_age)
            && tokio::fs::remove_file(entry.path()).await.is_ok()
        {
            cleared += 1;
        }
    }
    Ok(cleared)
}

/// Takes the write permission from a file that is about to become a blob.
#[cfg(unix)]
async fn make_read_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) =
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(BLOB_FILE_MODE)).await
    {
        // Only a safeguard: a blob that stays writable is still the right content.
        tracing::debug!(%error, "a blob could not be made read-only");
    }
}

/// Where files cannot be made read-only the store leaves them as they are.
#[cfg(not(unix))]
async fn make_read_only(_: &Path) {}

/// Gives the finished file `from` the name `to`, atomically, and says whether this call made that name.
///
/// A hard link is the one step that cannot overwrite: it makes the name, or it fails because the name is there. When
/// the two names are on different file systems, or the file system has no links, the content is copied beside the
/// target first, so that the final step is still a single atomic one in the target's own directory.
async fn install(from: &Path, to: &Path, durable: bool) -> std::io::Result<bool> {
    let Some(parent) = to.parent() else {
        return Err(ErrorKind::InvalidInput.into());
    };
    tokio::fs::create_dir_all(parent).await?;
    let created = match tokio::fs::hard_link(from, to).await {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
        Err(error) => {
            tracing::debug!(%error, "no hard link to the blob's name; copying into place");
            install_by_copy(from, to, durable).await?
        }
    };
    if durable && created {
        sync_directory(parent).await?;
    }
    Ok(created)
}

/// The slower way of [`install`]: a copy beside the target, then the same single step.
async fn install_by_copy(from: &Path, to: &Path, durable: bool) -> std::io::Result<bool> {
    let sibling = to.with_file_name(format!(".{}{INCOMING_SUFFIX}", new_id().simple()));
    tokio::fs::copy(from, &sibling).await?;
    let outcome = async {
        if durable {
            tokio::fs::File::open(&sibling).await?.sync_all().await?;
        }
        match tokio::fs::hard_link(&sibling, to).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
            // No links at all: a rename is still one atomic step, and what it puts in place is the same
            // content whoever gets there first.
            Err(_) if !tokio::fs::try_exists(to).await? => {
                tokio::fs::rename(&sibling, to).await.map(|()| true)
            }
            Err(_) => Ok(false),
        }
    }
    .await;
    // After a rename the sibling is gone; after a link it is the second name of the blob.
    if let Err(error) = tokio::fs::remove_file(&sibling).await
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!(%error, "a copy beside a blob could not be removed");
    }
    outcome
}

/// Flushes a directory, so that the names made in it survive a crash.
#[cfg(unix)]
async fn sync_directory(dir: &Path) -> std::io::Result<()> {
    tokio::fs::File::open(dir).await?.sync_all().await
}

/// Directories cannot be flushed on this platform.
#[cfg(not(unix))]
async fn sync_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slower way of putting a blob in place, which the tests of the store cannot reach on one file system:
    /// the first copy makes the name, a second says that it is there, and nothing is left beside it.
    #[tokio::test]
    async fn a_blob_copied_into_place_is_made_once_and_leaves_no_copy_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let target_dir = dir.path().join("blobs/ab/cd");
        tokio::fs::create_dir_all(&target_dir).await.unwrap();
        let target = target_dir.join("abcdef");
        tokio::fs::write(&from, b"content").await.unwrap();
        assert!(install_by_copy(&from, &target, true).await.unwrap());
        assert!(!install_by_copy(&from, &target, true).await.unwrap());
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"content");
        let mut names = vec![];
        let mut entries = tokio::fs::read_dir(&target_dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name());
        }
        assert_eq!(names, vec![std::ffi::OsString::from("abcdef")]);
    }
}
