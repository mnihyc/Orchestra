//! Checked mount streams replace only their original owned root and cannot traverse a link.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use super::*;
use crate::blobs::MemoryBlobs;
use orch_api_contract::workspace::JobProgress;
use orch_driver_contract::volume_stream::{Entry, Kind, MAGIC};
use orch_link_contract::commands::{BlobRef, VolumeArchive};
use std::{
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::Path,
};
struct Quiet;
impl Progress for Quiet {
    fn report(&self, _: &JobProgress) {}
}
fn binding(path: &Path) -> DirectoryBinding {
    let stat = std::fs::metadata(path).unwrap();
    DirectoryBinding {
        path: path.to_str().unwrap().into(),
        device: Some(stat.dev()),
        inode: Some(stat.ino()),
    }
}
fn entry(path: &[u8], kind: Kind) -> Entry {
    Entry {
        path: path.to_vec(),
        kind,
        size: 0,
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        mode: 0o700,
        atime: 1,
        atime_ns: 0,
        mtime: 2,
        mtime_ns: 0,
        link: None,
        major: 0,
        minor: 0,
        xattrs: Vec::new(),
    }
}
fn stream(blobs: &MemoryBlobs, entries: &[Entry]) -> VolumeArchive {
    let mut bytes = MAGIC.to_vec();
    for entry in entries {
        let header = serde_json::to_vec(entry).unwrap();
        bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
        bytes.extend(header);
    }
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    let size = bytes.len() as i64;
    VolumeArchive {
        parts: vec![BlobRef {
            hash: blobs.insert(bytes),
            size,
        }],
        size,
    }
}

#[tokio::test]
async fn a_directory_import_restores_whole_mount_bytes_links_and_metadata_into_its_original_owned_root()
 {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(source.join("visible"), b"carried data").unwrap();
    std::fs::hard_link(source.join("visible"), source.join("alias")).unwrap();
    std::fs::set_permissions(
        source.join("visible"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    rustix::fs::setxattr(
        source.join("visible"),
        "user.mount-copy",
        b"kept",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    std::fs::write(source.join(".orch_ignore"), b"hidden").unwrap();
    std::fs::write(source.join("hidden"), b"whole data").unwrap();
    std::fs::create_dir(source.join("nested")).unwrap();
    std::fs::write(source.join("nested/child"), b"child").unwrap();
    symlink("visible", source.join("link")).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o750)).unwrap();
    let original_file = std::fs::metadata(source.join("visible")).unwrap();
    let original_root = std::fs::metadata(&source).unwrap();
    std::fs::write(target.join("older"), b"replaced only by a valid import").unwrap();
    let blobs = MemoryBlobs::new();
    let archive = super::export(binding(&source), 64, &blobs, Arc::new(Quiet))
        .await
        .unwrap();
    super::import(binding(&target), archive, &blobs, Arc::new(Quiet))
        .await
        .unwrap();
    for name in ["visible", "hidden", ".orch_ignore", "nested/child"] {
        assert_eq!(
            std::fs::read(target.join(name)).unwrap(),
            std::fs::read(source.join(name)).unwrap()
        );
    }
    assert!(!target.join("older").exists());
    let file = std::fs::metadata(target.join("visible")).unwrap();
    let alias = std::fs::metadata(target.join("alias")).unwrap();
    assert_eq!((file.dev(), file.ino()), (alias.dev(), alias.ino()));
    assert_eq!(
        (
            file.uid(),
            file.gid(),
            file.mode() & 0o7777,
            file.mtime(),
            file.mtime_nsec()
        ),
        (
            original_file.uid(),
            original_file.gid(),
            original_file.mode() & 0o7777,
            original_file.mtime(),
            original_file.mtime_nsec()
        )
    );
    let mut value = [0; 4];
    assert_eq!(
        rustix::fs::getxattr(target.join("visible"), "user.mount-copy", &mut value[..]).unwrap(),
        4
    );
    assert_eq!(&value, b"kept");
    let root = std::fs::metadata(&target).unwrap();
    assert_eq!(
        (root.mode() & 0o7777, root.mtime(), root.mtime_nsec()),
        (
            original_root.mode() & 0o7777,
            original_root.mtime(),
            original_root.mtime_nsec()
        )
    );
    assert_eq!(
        std::fs::read_link(target.join("link"))
            .unwrap()
            .as_os_str()
            .as_bytes(),
        b"visible"
    );
    assert_eq!(
        std::fs::read(source.join("visible")).unwrap(),
        b"carried data"
    );
}

#[tokio::test]
async fn a_directory_import_refuses_a_replaced_root_or_incomplete_archive_before_removing_existing_data()
 {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep"), b"original data").unwrap();
    let original = binding(&target);
    let blobs = MemoryBlobs::new();
    let valid = stream(&blobs, &[entry(b"", Kind::Directory)]);
    for archive in [
        VolumeArchive {
            parts: Vec::new(),
            size: 0,
        },
        VolumeArchive {
            size: valid.size + 1,
            ..valid.clone()
        },
        stream(&blobs, &[entry(b"wrong-root", Kind::Directory)]),
    ] {
        assert!(
            super::import(binding(&target), archive, &blobs, Arc::new(Quiet))
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(target.join("keep")).unwrap(),
            b"original data"
        );
    }
    std::fs::rename(&target, directory.path().join("original")).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep"), b"replacement data").unwrap();
    assert!(
        super::import(original, valid, &blobs, Arc::new(Quiet))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(target.join("keep")).unwrap(),
        b"replacement data"
    );
    assert_eq!(
        std::fs::read(directory.path().join("original/keep")).unwrap(),
        b"original data"
    );
}

#[tokio::test]
async fn a_directory_import_never_writes_through_parent_traversal_or_an_archive_symlink() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let outside = directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"outside data").unwrap();
    let blobs = MemoryBlobs::new();
    let mut link = entry(b"link", Kind::Link);
    link.link = Some(outside.as_os_str().as_bytes().to_vec());
    for entries in [
        vec![
            entry(b"", Kind::Directory),
            entry(b"../escaped", Kind::File),
        ],
        vec![
            entry(b"", Kind::Directory),
            link,
            entry(b"link/escaped", Kind::File),
        ],
    ] {
        assert!(
            super::import(
                binding(&target),
                stream(&blobs, &entries),
                &blobs,
                Arc::new(Quiet)
            )
            .await
            .is_err()
        );
        assert!(!directory.path().join("escaped").exists());
        assert!(!outside.join("escaped").exists());
        assert_eq!(
            std::fs::read(outside.join("keep")).unwrap(),
            b"outside data"
        );
    }
}

struct Refused;
#[async_trait::async_trait]
impl crate::blobs::BlobSource for Refused {
    async fn get(
        &self,
        _: orch_types::Hash,
        _: &mut dyn crate::blobs::BlobWriter,
    ) -> std::result::Result<i64, crate::blobs::BlobError> {
        Err(crate::blobs::BlobError::Refused(
            "the source is unavailable".into(),
        ))
    }
}
#[tokio::test]
async fn a_failed_directory_download_joins_its_native_reader_and_preserves_the_unstarted_target() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("keep"), b"keep").unwrap();
    let blobs = MemoryBlobs::new();
    let archive = stream(&blobs, &[entry(b"", Kind::Directory)]);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        super::import(
            binding(directory.path()),
            archive,
            &Refused,
            Arc::new(Quiet),
        ),
    )
    .await
    .expect("a failed download releases and joins its native reader");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("the source is unavailable")
    );
    assert_eq!(
        std::fs::read(directory.path().join("keep")).unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn a_directory_import_preserves_a_socket_entry_whose_filename_is_longer_than_a_socket_address()
 {
    use std::os::unix::fs::FileTypeExt;
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&target).unwrap();
    let socket = source.join("short");
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    let name = "s".repeat(200);
    std::fs::rename(socket, source.join(&name)).unwrap();
    let blobs = MemoryBlobs::new();
    let archive = super::export(binding(&source), 64, &blobs, Arc::new(Quiet))
        .await
        .unwrap();
    super::import(binding(&target), archive, &blobs, Arc::new(Quiet))
        .await
        .unwrap();
    assert!(
        std::fs::symlink_metadata(target.join(name))
            .unwrap()
            .file_type()
            .is_socket()
    );
}

struct ReplaceRoot {
    path: std::path::PathBuf,
    moved: std::path::PathBuf,
    changed: std::sync::atomic::AtomicBool,
}
impl Progress for ReplaceRoot {
    fn report(&self, _: &JobProgress) {}
    fn counting(&self, _: i64, _: i64, _: i64) {
        if !self.changed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            std::fs::rename(&self.path, &self.moved).unwrap();
            std::fs::create_dir(&self.path).unwrap();
            std::fs::write(self.path.join("replacement"), b"leave this directory alone").unwrap();
        }
    }
}
#[tokio::test]
async fn a_directory_import_never_reports_a_replaced_destination_as_the_current_completed_copy() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let blobs = MemoryBlobs::new();
    let archive = stream(
        &blobs,
        &[entry(b"", Kind::Directory), entry(b"copied", Kind::File)],
    );
    let progress = Arc::new(ReplaceRoot {
        path: target.clone(),
        moved: directory.path().join("original"),
        changed: std::sync::atomic::AtomicBool::new(false),
    });
    assert!(
        super::import(binding(&target), archive, &blobs, progress.clone())
            .await
            .is_err()
    );
    assert!(progress.changed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        std::fs::read(target.join("replacement")).unwrap(),
        b"leave this directory alone"
    );
    assert!(!target.join("copied").exists());
}

#[tokio::test]
async fn a_directory_import_restores_interleaved_directory_metadata_after_all_of_its_children() {
    let directory = tempfile::tempdir().unwrap();
    let blobs = MemoryBlobs::new();
    let mut root = entry(b"", Kind::Directory);
    root.mode = 0o750;
    root.mtime = 21;
    let mut a = entry(b"a", Kind::Directory);
    a.mode = 0o510;
    a.mtime = 11;
    let mut b = entry(b"b", Kind::Directory);
    b.mode = 0o510;
    b.mtime = 12;
    let archive = stream(
        &blobs,
        &[
            root,
            a,
            b,
            entry(b"a/one", Kind::File),
            entry(b"b/two", Kind::File),
            entry(b"a/three", Kind::File),
        ],
    );
    super::import(binding(directory.path()), archive, &blobs, Arc::new(Quiet))
        .await
        .unwrap();
    for name in ["a/one", "b/two", "a/three"] {
        assert!(directory.path().join(name).is_file());
    }
    for (name, mode, mtime) in [("", 0o750, 21), ("a", 0o510, 11), ("b", 0o510, 12)] {
        let metadata = std::fs::metadata(directory.path().join(name)).unwrap();
        assert_eq!((metadata.mode() & 0o7777, metadata.mtime()), (mode, mtime));
    }
}

#[tokio::test]
async fn removing_owned_mount_data_keeps_other_directories_and_refuses_a_replaced_root() {
    let directory = tempfile::tempdir().unwrap();
    let owned = directory.path().join("owned");
    std::fs::create_dir(&owned).unwrap();
    let other = directory.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("keep"), b"other data").unwrap();
    let mut deep = owned.clone();
    for _ in 0..256 {
        deep.push("d");
        std::fs::create_dir(&deep).unwrap();
    }
    std::fs::write(deep.join("data"), b"owned").unwrap();
    let original = binding(&owned);
    super::remove(original.clone(), Arc::new(Quiet))
        .await
        .unwrap();
    assert!(!owned.exists());
    assert_eq!(std::fs::read(other.join("keep")).unwrap(), b"other data");
    std::fs::create_dir(&owned).unwrap();
    let original = binding(&owned);
    std::fs::rename(&owned, directory.path().join("moved")).unwrap();
    std::fs::create_dir(&owned).unwrap();
    std::fs::write(owned.join("keep"), b"replacement").unwrap();
    assert!(super::remove(original, Arc::new(Quiet)).await.is_err());
    assert_eq!(std::fs::read(owned.join("keep")).unwrap(), b"replacement");
}
