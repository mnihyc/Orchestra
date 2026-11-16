//! Native mount exports keep their complete data and metadata within checked stream parts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use super::*;
use crate::blobs::MemoryBlobs;
use orch_api_contract::workspace::JobProgress;
use std::{
    io::{Cursor, Read},
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

#[tokio::test]
async fn a_directory_export_keeps_ignored_files_hard_links_and_native_metadata_without_following_a_link()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("mount");
    std::fs::create_dir(&root).unwrap();
    let outside = directory.path().join("outside");
    std::fs::write(&outside, b"outside bytes never enter the archive").unwrap();
    rustix::fs::setxattr(
        &outside,
        "user.not-from-this-mount",
        b"outside attribute",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    std::fs::write(root.join("visible"), b"original file bytes").unwrap();
    std::fs::hard_link(root.join("visible"), root.join("alias")).unwrap();
    std::fs::set_permissions(root.join("visible"), std::fs::Permissions::from_mode(0o640)).unwrap();
    rustix::fs::setxattr(
        root.join("visible"),
        "user.mount-copy",
        b"kept attribute",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    std::fs::write(root.join(".orch_ignore"), b"hidden\n").unwrap();
    std::fs::write(root.join("hidden"), b"kept despite ignore").unwrap();
    symlink(&outside, root.join("link")).unwrap();
    let source = std::fs::metadata(root.join("visible")).unwrap();
    let blobs = MemoryBlobs::new();
    let archive = super::export(binding(&root), 64, &blobs, Arc::new(Quiet))
        .await
        .unwrap();
    assert!(archive.parts.len() > 1);
    assert!(
        archive
            .parts
            .iter()
            .all(|part| part.size > 0 && part.size <= 64)
    );
    let mut stream = Vec::new();
    for part in &archive.parts {
        let bytes = blobs.get_bytes(&part.hash).unwrap();
        assert_eq!(bytes.len() as i64, part.size);
        stream.extend_from_slice(&bytes);
    }
    assert_eq!(stream.len() as i64, archive.size);
    assert!(stream.starts_with(b"ORCH-VOLUME-1\n"));
    let mut input = Cursor::new(&stream[b"ORCH-VOLUME-1\n".len()..]);
    let mut entries = std::collections::BTreeMap::new();
    loop {
        let mut size = [0; 4];
        input.read_exact(&mut size).unwrap();
        let size = u32::from_le_bytes(size) as usize;
        if size == 0 {
            break;
        }
        let mut header = vec![0; size];
        input.read_exact(&mut header).unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header).unwrap();
        let name = header["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|byte| byte.as_u64().unwrap() as u8)
            .collect::<Vec<_>>();
        let mut data = vec![0; header["size"].as_u64().unwrap() as usize];
        input.read_exact(&mut data).unwrap();
        entries.insert(name, (header, data));
    }
    assert_eq!(entries[b"hidden".as_slice()].1, b"kept despite ignore");
    assert_eq!(entries[b".orch_ignore".as_slice()].1, b"hidden\n");
    let files = [b"visible".as_slice(), b"alias".as_slice()].map(|name| &entries[name]);
    assert_eq!(
        files
            .iter()
            .filter(|(header, _)| header["kind"] == "file")
            .count(),
        1
    );
    assert_eq!(
        files
            .iter()
            .filter(|(header, _)| header["kind"] == "hard_link")
            .count(),
        1
    );
    for (header, data) in files {
        assert_eq!(header["uid"], source.uid());
        assert_eq!(header["gid"], source.gid());
        assert_eq!(header["mode"], 0o640);
        assert_eq!(header["mtime"], source.mtime());
        assert_eq!(header["mtime_ns"], source.mtime_nsec());
        assert!(
            header["xattrs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|attribute| attribute
                    == &serde_json::json!([b"user.mount-copy", b"kept attribute"]))
        );
        if header["kind"] == "file" {
            assert_eq!(data, b"original file bytes");
        }
    }
    let link = &entries[b"link".as_slice()];
    assert_eq!(link.0["kind"], "link");
    assert_eq!(
        link.0["link"],
        serde_json::json!(outside.as_os_str().as_bytes())
    );
    assert!(link.1.is_empty());
    assert!(
        !link.0["xattrs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|attribute| attribute[0] == serde_json::json!(b"user.not-from-this-mount"))
    );
    assert!(
        !stream
            .windows(b"outside bytes never enter the archive".len())
            .any(|window| window == b"outside bytes never enter the archive")
    );
    assert_eq!(
        std::fs::read(root.join("visible")).unwrap(),
        b"original file bytes"
    );
}

#[tokio::test]
async fn a_directory_export_refuses_a_replaced_root_before_uploading_any_data() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("mount");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("original"), b"keep").unwrap();
    let source = binding(&root);
    std::fs::rename(&root, directory.path().join("original")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("replacement"), b"private replacement").unwrap();
    let blobs = MemoryBlobs::new();
    assert!(
        super::export(source, 64, &blobs, Arc::new(Quiet))
            .await
            .is_err()
    );
    assert!(blobs.uploads().is_empty());
    assert_eq!(
        std::fs::read(directory.path().join("original/original")).unwrap(),
        b"keep"
    );
}

struct Refused;
#[async_trait::async_trait]
impl crate::blobs::BlobSink for Refused {
    async fn missing(
        &self,
        _: &[orch_types::Hash],
    ) -> std::result::Result<Vec<orch_types::Hash>, crate::blobs::BlobError> {
        Err(crate::blobs::BlobError::Refused(
            "the store is unavailable".into(),
        ))
    }
    async fn put(
        &self,
        _: orch_types::Hash,
        _: i64,
        _: &mut dyn crate::blobs::BlobReader,
    ) -> std::result::Result<(), crate::blobs::BlobError> {
        unreachable!("a refused upload must not begin")
    }
}

#[tokio::test]
async fn a_refused_directory_upload_joins_its_encoder_and_keeps_its_source() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("data"), vec![7; 4096]).unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        super::export(binding(directory.path()), 64, &Refused, Arc::new(Quiet)),
    )
    .await
    .expect("a refused sink must release its blocked native encoder");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("the store is unavailable")
    );
    assert_eq!(
        std::fs::read(directory.path().join("data")).unwrap(),
        vec![7; 4096]
    );
}

struct Cancels(tokio_util::sync::CancellationToken);
#[async_trait::async_trait]
impl crate::blobs::BlobSink for Cancels {
    async fn missing(
        &self,
        _: &[orch_types::Hash],
    ) -> std::result::Result<Vec<orch_types::Hash>, crate::blobs::BlobError> {
        self.0.cancel();
        std::future::pending().await
    }
    async fn put(
        &self,
        _: orch_types::Hash,
        _: i64,
        _: &mut dyn crate::blobs::BlobReader,
    ) -> std::result::Result<(), crate::blobs::BlobError> {
        unreachable!("the cancelled transfer must not begin")
    }
}

#[tokio::test]
async fn cancelling_a_directory_export_joins_its_native_reader_even_when_the_store_has_stopped_replying()
 {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("data"), vec![7; 4096]).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let source = binding(directory.path());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::error::with_cancellation(cancel.clone(), async move {
            super::export(source, 64, &Cancels(cancel), Arc::new(Quiet)).await
        }),
    )
    .await
    .expect("cancellation must join the native reader even when its upload was waiting");
    assert!(matches!(result, Err(HostError::Cancelled)));
    assert_eq!(
        std::fs::read(directory.path().join("data")).unwrap(),
        vec![7; 4096]
    );
}
