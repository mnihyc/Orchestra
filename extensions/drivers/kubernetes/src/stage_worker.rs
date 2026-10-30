//! Private staging and publication for the trusted init container.
//!
//! These operations need a trusted, read-only helper image and an authenticated
//! Kubernetes exec channel. They are not by themselves a working Pod lifecycle.

#![cfg(target_os = "linux")]

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use orch_driver_contract::driver::DriverError;
use rustix::fs::{CWD, Mode, OFlags, RenameFlags, openat, renameat_with};
use tokio::io::AsyncRead;
use tokio::time::Instant;

use crate::stage_protocol::{self, StageHeader};
use crate::staging;

const WORK: &str = ".orch-stage";
const CLAIM: &str = "claimed";
const ARCHIVE: &str = "archive";
const TREE: &str = "tree";
const READY: &str = ".orch-stage-ready";
const READY_TEMP: &str = ".orch-stage-ready.tmp";
const SUPERVISOR: &str = "orch-kube-supervisor";
// A protocol header is bounded independently of archive size.
const MAX_READY_BYTES: u64 = 256 * 1024;
// The helper is one executable, not another unbounded tool archive.
const MAX_SUPERVISOR_BYTES: u64 = 128 * 1024 * 1024;

fn unavailable() -> DriverError {
    DriverError::Unavailable("the trusted staging transfer could not be published".into())
}

fn deadline_live(deadline: Instant) -> Result<(), DriverError> {
    if Instant::now() >= deadline {
        Err(DriverError::Timeout)
    } else {
        Ok(())
    }
}

fn owned_directory(path: &Path, mode: u32) -> Result<(), DriverError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != mode
    {
        return Err(unavailable());
    }
    Ok(())
}

/// Set up an empty mounted directory once. An old or partial tree is never adopted.
pub(crate) fn prepare(root: &Path) -> Result<(), DriverError> {
    if !root.is_absolute() {
        return Err(unavailable());
    }
    let metadata = fs::symlink_metadata(root).map_err(|_| unavailable())?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || fs::read_dir(root)
            .map_err(|_| unavailable())?
            .next()
            .is_some()
    {
        return Err(unavailable());
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|_| unavailable())?;
    fs::create_dir(root.join(WORK)).map_err(|_| unavailable())?;
    fs::set_permissions(root.join(WORK), fs::Permissions::from_mode(0o700))
        .map_err(|_| unavailable())?;
    Ok(())
}

fn new_private_file(path: &Path) -> Result<File, DriverError> {
    OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| unavailable())
}

fn install_supervisor(
    source: &Path,
    destination: &Path,
    deadline: Instant,
) -> Result<(), DriverError> {
    deadline_live(deadline)?;
    if !source.is_absolute() {
        return Err(unavailable());
    }
    // Pin the image file and refuse a symlink at its final component.
    let fd = openat(
        CWD,
        source,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| unavailable())?;
    let mut input = File::from(fd);
    let metadata = input.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_SUPERVISOR_BYTES
        || metadata.mode() & 0o022 != 0
        || metadata.mode() & 0o111 == 0
        || !(metadata.uid() == 0 || metadata.uid() == rustix::process::geteuid().as_raw())
    {
        return Err(unavailable());
    }
    let mut output = new_private_file(destination)?;
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        deadline_live(deadline)?;
        let count = input.read(&mut buffer).map_err(|_| unavailable())?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(u64::try_from(count).map_err(|_| unavailable())?)
            .filter(|size| *size <= MAX_SUPERVISOR_BYTES)
            .ok_or_else(unavailable)?;
        output
            .write_all(&buffer[..count])
            .map_err(|_| unavailable())?;
    }
    if copied != metadata.len() {
        return Err(unavailable());
    }
    output
        .set_permissions(fs::Permissions::from_mode(0o755))
        .map_err(|_| unavailable())?;
    output.sync_all().map_err(|_| unavailable())?;
    deadline_live(deadline)
}

/// An unpublished, private staged tree. Only a matching release can publish it.
pub(crate) struct PreparedStage {
    root: PathBuf,
    header: StageHeader,
}

impl PreparedStage {
    pub(crate) fn header(&self) -> &StageHeader {
        &self.header
    }

    /// Release is consumed once, after the node has checked the prepared receipt
    /// and the original Kubernetes identities. A mismatch quarantines the tree.
    pub(crate) fn release(
        self,
        release: &StageHeader,
        deadline: Instant,
    ) -> Result<StageHeader, DriverError> {
        deadline_live(deadline)?;
        if release != &self.header {
            return Err(unavailable());
        }
        owned_directory(&self.root, 0o700)?;
        let tree = self.root.join(WORK).join(TREE);
        // Do not replace anything already present, even if another actor creates it.
        for name in ["bin", "tools"] {
            renameat_with(
                CWD,
                tree.join(name),
                CWD,
                self.root.join(name),
                RenameFlags::NOREPLACE,
            )
            .map_err(|_| unavailable())?;
        }
        fs::remove_dir(&tree).map_err(|_| unavailable())?;
        let mut ready = new_private_file(&self.root.join(READY_TEMP))?;
        serde_json::to_writer(&mut ready, &self.header).map_err(|_| unavailable())?;
        ready
            .set_permissions(fs::Permissions::from_mode(0o444))
            .map_err(|_| unavailable())?;
        ready.sync_all().map_err(|_| unavailable())?;
        drop(ready);
        deadline_live(deadline)?;
        // A non-root workload needs to traverse the read-only mount after init exits.
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o755))
            .map_err(|_| unavailable())?;
        renameat_with(
            CWD,
            self.root.join(READY_TEMP),
            CWD,
            self.root.join(READY),
            RenameFlags::NOREPLACE,
        )
        .map_err(|_| unavailable())?;
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| unavailable())?;
        deadline_live(deadline)?;
        Ok(self.header)
    }
}

/// Receive and verify one transfer without publishing it. A claimed or partial
/// attempt is quarantined; the caller must replace the Pod to try again.
pub(crate) async fn prepare_transfer<R: AsyncRead + Unpin>(
    root: &Path,
    supervisor: &Path,
    pod_uid: &str,
    input: &mut R,
    stall_timeout: Duration,
    deadline: Instant,
) -> Result<PreparedStage, DriverError> {
    deadline_live(deadline)?;
    owned_directory(root, 0o700)?;
    let work = root.join(WORK);
    owned_directory(&work, 0o700)?;
    // This file is deliberately retained even on failure or cancellation.
    let _claim = new_private_file(&work.join(CLAIM))?;
    let header = stage_protocol::read_header(input, pod_uid, deadline).await?;
    let spool = new_private_file(&work.join(ARCHIVE))?;
    let mut spool = tokio::fs::File::from_std(spool);
    stage_protocol::transfer_archive(input, &mut spool, &header, pod_uid, stall_timeout, deadline)
        .await?;
    // The spool is private and ephemeral; transfer_archive already flushed it.
    // If conversion is cancelled, its claimed directory remains quarantined.
    let mut spool = tokio::time::timeout_at(deadline, spool.into_std())
        .await
        .map_err(|_| DriverError::Timeout)?;
    deadline_live(deadline)?;
    let tree = work.join(TREE);
    fs::create_dir(&tree).map_err(|_| unavailable())?;
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o700)).map_err(|_| unavailable())?;
    staging::extract(
        &mut spool,
        &tree,
        &header.tools,
        &header.receipt,
        deadline.into_std(),
    )?;
    install_supervisor(supervisor, &tree.join("bin").join(SUPERVISOR), deadline)?;
    drop(spool);
    fs::remove_file(work.join(ARCHIVE)).map_err(|_| unavailable())?;
    deadline_live(deadline)?;
    Ok(PreparedStage {
        root: root.to_path_buf(),
        header,
    })
}

/// Read a completed receipt; the node must independently compare it with its own
/// expected digest and the pinned Pod and container identities.
pub(crate) fn ready(root: &Path, pod_uid: &str) -> Result<Option<StageHeader>, DriverError> {
    if !root.is_absolute() {
        return Err(unavailable());
    }
    let metadata = fs::symlink_metadata(root).map_err(|_| unavailable())?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || ![0o700, 0o755].contains(&(metadata.mode() & 0o777))
    {
        return Err(unavailable());
    }
    let fd = match openat(
        CWD,
        root.join(READY),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(unavailable()),
    };
    let file = File::from(fd);
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o222 != 0
        || metadata.len() > MAX_READY_BYTES
    {
        return Err(unavailable());
    }
    let header: StageHeader =
        serde_json::from_reader(file.take(MAX_READY_BYTES + 1)).map_err(|_| unavailable())?;
    header.check(pod_uid)?;
    Ok(Some(header))
}

/// The init process waits for the verified receiver's publication under the same
/// absolute deadline; it never interprets an incomplete private tree as success.
pub(crate) async fn wait_ready(
    root: &Path,
    pod_uid: &str,
    deadline: Instant,
) -> Result<StageHeader, DriverError> {
    const POLL: Duration = Duration::from_millis(200);
    loop {
        deadline_live(deadline)?;
        if let Some(header) = ready(root, pod_uid)? {
            deadline_live(deadline)?;
            return Ok(header);
        }
        tokio::time::sleep_until((Instant::now() + POLL).min(deadline)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::staging::archive;
    use std::time::Duration;

    fn fixture() -> (tempfile::TempDir, Vec<u8>, StageHeader) {
        let parent = tempfile::tempdir().unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let cache = parent.path().join("cache");
        let connector = cache.join("orch-connector").join("fixture");
        fs::create_dir_all(&connector).unwrap();
        for path in [
            cache.clone(),
            cache.join("orch-connector"),
            connector.clone(),
        ] {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(connector.join(".complete"), format!("{}\n", "a".repeat(64))).unwrap();
        fs::set_permissions(
            connector.join(".complete"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::write(connector.join("orch-connector"), b"synthetic connector").unwrap();
        fs::set_permissions(
            connector.join("orch-connector"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut archive_bytes = Vec::new();
        let receipt = archive(
            &cache,
            &connector,
            &[],
            &mut archive_bytes,
            std::time::Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        let header = StageHeader {
            pod_uid: "fixture-pod".into(),
            receipt,
            tools: Vec::new(),
        };
        let encoded = serde_json::to_vec(&header).unwrap();
        let mut stream = u32::try_from(encoded.len()).unwrap().to_be_bytes().to_vec();
        stream.extend(encoded);
        stream.extend(archive_bytes);
        (parent, stream, header)
    }

    #[tokio::test]
    async fn verified_tools_and_the_image_supervisor_are_published_once() {
        let (parent, bytes, header) = fixture();
        let root = parent.path().join("stage");
        fs::create_dir(&root).unwrap();
        let supervisor = parent.path().join("supervisor");
        fs::write(&supervisor, b"synthetic supervisor").unwrap();
        fs::set_permissions(&supervisor, fs::Permissions::from_mode(0o755)).unwrap();
        prepare(&root).unwrap();
        assert!(ready(&root, "fixture-pod").unwrap().is_none());
        let prepared = prepare_transfer(
            &root,
            &supervisor,
            "fixture-pod",
            &mut bytes.as_slice(),
            Duration::from_secs(3),
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(prepared.header(), &header);
        assert!(ready(&root, "fixture-pod").unwrap().is_none());
        assert!(!root.join("bin").exists());
        let completed = prepared
            .release(&header, Instant::now() + Duration::from_secs(10))
            .unwrap();
        assert_eq!(completed, header);
        assert_eq!(ready(&root, "fixture-pod").unwrap(), Some(header));
        assert!(
            wait_ready(
                &root,
                "fixture-pod",
                Instant::now() + Duration::from_secs(1)
            )
            .await
            .is_ok()
        );
        assert!(ready(&root, "different-pod").is_err());
        assert_eq!(
            fs::read(root.join("bin/orch-connector")).unwrap(),
            b"synthetic connector"
        );
        assert_eq!(
            fs::read(root.join("bin/orch-kube-supervisor")).unwrap(),
            b"synthetic supervisor"
        );
        assert_eq!(fs::metadata(&root).unwrap().mode() & 0o777, 0o755);
        assert!(!root.join(WORK).join(ARCHIVE).exists());
        assert!(prepare(&root).is_err());
        assert!(
            prepare_transfer(
                &root,
                &supervisor,
                "fixture-pod",
                &mut bytes.as_slice(),
                Duration::from_secs(3),
                Instant::now() + Duration::from_secs(10)
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn a_bad_transfer_is_quarantined_without_a_ready_tree() {
        let (parent, mut bytes, _) = fixture();
        let root = parent.path().join("stage");
        fs::create_dir(&root).unwrap();
        let supervisor = parent.path().join("supervisor");
        fs::write(&supervisor, b"synthetic supervisor").unwrap();
        fs::set_permissions(&supervisor, fs::Permissions::from_mode(0o755)).unwrap();
        prepare(&root).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(
            prepare_transfer(
                &root,
                &supervisor,
                "fixture-pod",
                &mut bytes.as_slice(),
                Duration::from_secs(3),
                Instant::now() + Duration::from_secs(10)
            )
            .await
            .is_err()
        );
        assert!(ready(&root, "fixture-pod").unwrap().is_none());
        assert!(matches!(
            wait_ready(&root, "fixture-pod", Instant::now()).await,
            Err(DriverError::Timeout)
        ));
        assert!(!root.join("bin").exists());
        assert!(
            prepare_transfer(
                &root,
                &supervisor,
                "fixture-pod",
                &mut bytes.as_slice(),
                Duration::from_secs(3),
                Instant::now() + Duration::from_secs(10)
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn a_supervisor_symlink_cannot_publish_a_stage() {
        use std::os::unix::fs::symlink;
        let (parent, bytes, _) = fixture();
        let root = parent.path().join("stage");
        fs::create_dir(&root).unwrap();
        let executable = parent.path().join("image-supervisor");
        fs::write(&executable, b"synthetic supervisor").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let link = parent.path().join("supervisor-link");
        symlink(&executable, &link).unwrap();
        prepare(&root).unwrap();
        assert!(
            prepare_transfer(
                &root,
                &link,
                "fixture-pod",
                &mut bytes.as_slice(),
                Duration::from_secs(3),
                Instant::now() + Duration::from_secs(10),
            )
            .await
            .is_err()
        );
        assert!(ready(&root, "fixture-pod").unwrap().is_none());
        assert!(!root.join("bin").exists());
    }

    #[tokio::test]
    async fn an_altered_release_leaves_the_prepared_tree_unpublished() {
        let (parent, bytes, mut header) = fixture();
        let root = parent.path().join("stage");
        fs::create_dir(&root).unwrap();
        let supervisor = parent.path().join("supervisor");
        fs::write(&supervisor, b"synthetic supervisor").unwrap();
        fs::set_permissions(&supervisor, fs::Permissions::from_mode(0o755)).unwrap();
        prepare(&root).unwrap();
        let prepared = prepare_transfer(
            &root,
            &supervisor,
            "fixture-pod",
            &mut bytes.as_slice(),
            Duration::from_secs(3),
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();
        header.receipt.sha256 = "b".repeat(64);
        assert!(
            prepared
                .release(&header, Instant::now() + Duration::from_secs(10))
                .is_err()
        );
        assert!(ready(&root, "fixture-pod").unwrap().is_none());
        assert!(!root.join("bin").exists());
        assert!(
            prepare_transfer(
                &root,
                &supervisor,
                "fixture-pod",
                &mut bytes.as_slice(),
                Duration::from_secs(3),
                Instant::now() + Duration::from_secs(10),
            )
            .await
            .is_err()
        );
    }
}
