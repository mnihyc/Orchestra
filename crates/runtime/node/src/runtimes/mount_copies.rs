//! A mount's data copied between hosts: measured and archived where it is, unpacked into a volume made for that mount.
//!
//! The data is read where it lies, as a volume Orch made for the mount or as a directory inside the machine's
//! `environment.roots`, never through a unit: a unit holds a read-only mount read-only and writes a private view
//! into its own layer. It is unpacked only into a volume whose name is the mount's, and only while no unit has that
//! volume attached, before the unit that is to use it is made.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use orch_driver_contract::runtime::DirectoryBinding;
use orch_driver_contract::volumes::{OwnedVolumeSelector, VolumeMeasure, VolumeRequest};
use orch_link_contract::commands::{
    CommandError, ErrorCode, MountData, NodeMountExport, NodeMountFingerprint,
    NodeMountFingerprintResult, NodeMountImport, NodeMountMeasure, VOLUME_ARCHIVE_PART_MAX_BYTES,
    VolumeArchive, is_mount_volume,
};
use orch_types::Id;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::Runtimes;
use crate::drivers::failure;

/// A mount's data is read whole, as a borrowed home is: every file with its owner, mode and extended attributes.
const WHOLE_TREE: OwnedVolumeSelector = OwnedVolumeSelector::Home;

/// A pipe of this size carries an archive from the downloaded part into the driver; the whole volume is never
/// staged on the target.
const IMPORT_PIPE_BYTES: usize = 64 * 1024;

fn internal(error: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorCode::Internal, error.to_string())
}

/// Where a mount's data is read, checked against what this machine allows.
enum Source {
    Volume(VolumeRequest),
    Directory(DirectoryBinding),
}

impl Source {
    fn lock_name(&self) -> String {
        match self {
            Self::Volume(volume) => volume.name.clone(),
            // A colon is not a volume-name character, so a checked directory cannot alias a name.
            Self::Directory(directory) => format!("directory:{}", directory.path),
        }
    }
}

fn mount_volume(
    mount: Id,
    name: &str,
    storage: Option<&String>,
) -> Result<VolumeRequest, CommandError> {
    if !crate::volumes::is_volume_name(name) || !is_mount_volume(mount, name) {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "the volume is not one Orch made for this mount",
        ));
    }
    Ok(VolumeRequest {
        name: name.to_owned(),
        storage: storage.cloned(),
        labels: BTreeMap::new(),
    })
}

impl Runtimes {
    /// Reads only the permitted location and returns its fingerprint, never its files.
    pub async fn fingerprint_mount(
        &self,
        body: &NodeMountFingerprint,
    ) -> Result<NodeMountFingerprintResult, CommandError> {
        self.fingerprint_mount_reporting(
            body,
            &crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeMountFingerprint.deadline_secs(),
            )),
        )
        .await
    }

    /// Joins a permitted content read and its helper cleanup before acknowledging cancellation.
    pub async fn fingerprint_mount_reporting(
        &self,
        body: &NodeMountFingerprint,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<NodeMountFingerprintResult, CommandError> {
        let driver = self.driver(&body.driver)?;
        let fingerprint = match self.mount_source(body.mount_id, &body.data)? {
            Source::Volume(request) => {
                driver
                    .extras()
                    .fingerprint_volume_reporting(&request, work)
                    .await
            }
            Source::Directory(directory) => {
                driver
                    .extras()
                    .fingerprint_directory_reporting(&directory, work)
                    .await
            }
        }
        .map_err(|error| failure(&error))?;
        Ok(NodeMountFingerprintResult { fingerprint })
    }

    fn mount_source(&self, mount: Id, data: &MountData) -> Result<Source, CommandError> {
        match data {
            MountData::Volume { name, storage } => {
                Ok(Source::Volume(mount_volume(mount, name, storage.as_ref())?))
            }
            MountData::Path { path } => {
                let policy = super::lock(&self.inner.policy)
                    .as_ref()
                    .and_then(|store| store.policy())
                    .ok_or_else(|| {
                        CommandError::new(
                            ErrorCode::PolicyRootNotAllowed,
                            "this machine has no environment.roots policy",
                        )
                    })?;
                Ok(Source::Directory(
                    orch_connector_core::policy::inspect_environment_directory(
                        &policy,
                        Path::new(path),
                    )?,
                ))
            }
        }
    }

    /// Counts the files and bytes of a mount's data where it is, without changing or uploading anything.
    /// Performs the same operation without a job progress subscriber.
    pub async fn measure_mount(
        &self,
        body: &NodeMountMeasure,
    ) -> Result<VolumeMeasure, CommandError> {
        self.measure_mount_reporting(
            body,
            &crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeMountMeasure.deadline_secs(),
            )),
        )
        .await
    }

    /// Performs the operation with the caller's live reporting and cooperative cancellation.
    pub async fn measure_mount_reporting(
        &self,
        body: &NodeMountMeasure,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<VolumeMeasure, CommandError> {
        let node = self.clone();
        let body = body.clone();
        crate::work::owned(
            &self.inner.copy_tasks,
            &self.inner.stop,
            work.command_id().unwrap_or_else(orch_types::new_id),
            Arc::clone(work),
            move |work| async move { node.measure_mount_held(&body, &work).await },
        )
        .await
    }

    async fn measure_mount_held(
        &self,
        body: &NodeMountMeasure,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<VolumeMeasure, CommandError> {
        let driver = self.driver(&body.driver)?;
        let source = self.mount_source(body.mount_id, &body.data)?;
        let _held = self
            .hold_volumes(&body.driver, [source.lock_name()], work)
            .await?;
        self.volume_driver(&driver, work, async {
            match source {
                Source::Volume(request) => {
                    driver
                        .extras()
                        .measure_volume_reporting(&request, work)
                        .await
                }
                Source::Directory(directory) => {
                    driver
                        .extras()
                        .measure_directory_reporting(&directory, WHOLE_TREE, work)
                        .await
                }
            }
        })
        .await
    }

    /// A disconnected waiter stops its work and retains ownership until staging cleanup ends.
    /// Performs the same operation without a job progress subscriber.
    pub async fn export_mount(
        &self,
        command: Id,
        body: NodeMountExport,
    ) -> Result<VolumeArchive, CommandError> {
        self.export_mount_reporting(
            command,
            body,
            crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeMountExport.deadline_secs(),
            )),
        )
        .await
    }

    /// Performs the operation with the caller's live reporting and cooperative cancellation.
    pub async fn export_mount_reporting(
        &self,
        command: Id,
        body: NodeMountExport,
        work: Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<VolumeArchive, CommandError> {
        let node = self.clone();
        crate::work::owned(
            &self.inner.copy_tasks,
            &self.inner.stop,
            command,
            work,
            move |work| async move { node.export_mount_held(command, &body, &work).await },
        )
        .await
    }

    async fn export_mount_held(
        &self,
        command: Id,
        body: &NodeMountExport,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<VolumeArchive, CommandError> {
        if body.part_bytes <= 0 || body.part_bytes > VOLUME_ARCHIVE_PART_MAX_BYTES {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the archive part size exceeds the node's staging bound",
            ));
        }
        let driver = self.driver(&body.driver)?;
        let source = self.mount_source(body.mount_id, &body.data)?;
        let _held = self
            .hold_volumes(&body.driver, [source.lock_name()], work)
            .await?;
        let content = self.content()?.clone();
        let directory = content.directory_of(command);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(internal)?;
        let outcome = async {
            let archive = directory.join("mount.archive");
            self.volume_driver(&driver, work, async {
                match &source {
                    Source::Volume(request) => {
                        driver
                            .extras()
                            .export_selected_volume_reporting(request, WHOLE_TREE, &archive, work)
                            .await
                    }
                    Source::Directory(binding) => {
                        driver
                            .extras()
                            .export_directory_reporting(binding, WHOLE_TREE, &archive, work)
                            .await
                    }
                }
            })
            .await?;
            let uploaded = upload(
                &content,
                command,
                &directory,
                &archive,
                body.part_bytes,
                work,
            )
            .await;
            tokio::fs::remove_file(&archive).await.map_err(internal)?;
            uploaded
        }
        .await;
        content.finish(command).await;
        outcome
    }

    /// Unpacks only into a volume made for this mount that no unit has attached; what it held is replaced.
    /// Performs the same operation without a job progress subscriber.
    pub async fn import_mount(
        &self,
        command: Id,
        body: NodeMountImport,
    ) -> Result<(), CommandError> {
        self.import_mount_reporting(
            command,
            body,
            crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeMountImport.deadline_secs(),
            )),
        )
        .await
    }

    /// Performs the operation with the caller's live reporting and cooperative cancellation.
    pub async fn import_mount_reporting(
        &self,
        command: Id,
        body: NodeMountImport,
        work: Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), CommandError> {
        let node = self.clone();
        crate::work::owned(
            &self.inner.copy_tasks,
            &self.inner.stop,
            command,
            work,
            move |work| async move { node.import_mount_held(command, &body, &work).await },
        )
        .await
    }

    async fn import_mount_held(
        &self,
        command: Id,
        body: &NodeMountImport,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), CommandError> {
        let size = body.archive.parts.iter().try_fold(0i64, |sum, part| {
            (part.size > 0 && part.size <= VOLUME_ARCHIVE_PART_MAX_BYTES)
                .then_some(())
                .and_then(|()| sum.checked_add(part.size))
        });
        if body.archive.parts.is_empty() || size != Some(body.archive.size) {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the archive does not contain the complete ordered stream",
            ));
        }
        let request = mount_volume(body.mount_id, &body.name, body.storage.as_ref())?;
        let _held = self
            .hold_volumes(&body.driver, [request.name.clone()], work)
            .await?;
        if !self.inner.ledger.using_volume(&request.name).is_empty() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "a unit on this host has the volume attached; a copy lands before any unit uses it",
            ));
        }
        let driver = self.driver(&body.driver)?;
        if !driver
            .extras()
            .volume_exists(&request)
            .await
            .map_err(|error| failure(&error))?
        {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "the volume made for the copy is absent",
            ));
        }
        let content = self.content()?.clone();
        let outcome = async {
            let (mut output, mut input) = tokio::io::duplex(IMPORT_PIPE_BYTES);
            let source_content = &content;
            let feeding = async move {
                let mut transferred = 0i64;
                for part in &body.archive.parts {
                    let path = orch_connector_core::content::with_work(
                        crate::work::offset(Arc::clone(work), transferred, body.archive.size),
                        source_content.fetch(command, part.hash, part.size, "download.part"),
                    )
                    .await?;
                    let mut file = tokio::fs::File::open(&path).await.map_err(internal)?;
                    crate::work::copy(&mut file, &mut output, work)
                        .await
                        .map_err(internal)?;
                    drop(file);
                    tokio::fs::remove_file(path).await.map_err(internal)?;
                    transferred = transferred.saturating_add(part.size);
                }
                output.shutdown().await.map_err(internal)?;
                Ok::<_, CommandError>(())
            };
            let importing = async move {
                self.volume_driver(
                    &driver,
                    work,
                    driver.extras().import_selected_volume_stream_reporting(
                        &request, WHOLE_TREE, &mut input, work,
                    ),
                )
                .await
            };
            let (fed, imported) = tokio::join!(feeding, importing);
            fed?;
            imported?;
            Ok(())
        }
        .await;
        content.finish(command).await;
        outcome
    }
}

/// Puts an archive up in parts of at most `part_bytes`, one part staged on disk at a time.
async fn upload(
    content: &Arc<orch_connector_core::content::Content>,
    command: Id,
    directory: &Path,
    archive: &Path,
    part_bytes: i64,
    work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
) -> Result<VolumeArchive, CommandError> {
    let transfer_total = i64::try_from(tokio::fs::metadata(archive).await.map_err(internal)?.len())
        .map_err(internal)?;
    let limit = u64::try_from(part_bytes).map_err(internal)?;
    let mut input = tokio::fs::File::open(archive).await.map_err(internal)?;
    let mut parts = Vec::new();
    let mut size = 0i64;
    loop {
        let part: PathBuf = directory.join("upload.part");
        let mut output = tokio::fs::File::create(&part).await.map_err(internal)?;
        let copied = crate::work::copy(&mut (&mut input).take(limit), &mut output, work)
            .await
            .map_err(internal)?;
        output.sync_all().await.map_err(internal)?;
        drop(output);
        if copied == 0 {
            tokio::fs::remove_file(&part).await.map_err(internal)?;
            break;
        }
        let archived = orch_connector_core::content::with_work(
            crate::work::offset(Arc::clone(work), size, transfer_total),
            content.put(command, &part),
        )
        .await?;
        size = size.checked_add(archived.size).ok_or_else(|| {
            CommandError::new(
                ErrorCode::TooLarge,
                "the archive's total size exceeds its counter",
            )
        })?;
        parts.push(archived);
        tokio::fs::remove_file(&part).await.map_err(internal)?;
    }
    Ok(VolumeArchive { parts, size })
}
