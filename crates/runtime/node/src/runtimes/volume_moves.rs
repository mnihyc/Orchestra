//! Whole owned-volume measurements and copies, with source pause recovery and import before any target unit exists.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use orch_driver_contract::runtime::RuntimeKind;
use orch_driver_contract::units::UnitStateKind;
use orch_driver_contract::volumes::{OwnedVolumeSelector, VolumeRequest};
use orch_link_contract::commands::{
    CommandError, ErrorCode, ExportedVolume, MeasuredVolume, NodeRuntimeVolumeImport,
    NodeRuntimeVolumesExport, NodeRuntimeVolumesExportResult, NodeRuntimeVolumesMeasure,
    NodeRuntimeVolumesMeasureResult, SelectedVolume,
};
use orch_types::Id;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::Runtimes;
use crate::drivers::failure;
use crate::ledger::Record;

fn internal(error: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorCode::Internal, error.to_string())
}

fn selected_name(selected: &SelectedVolume) -> Result<(), CommandError> {
    let name = &selected.volume.name;
    let valid = crate::volumes::is_volume_name(name)
        && match selected.selector {
            OwnedVolumeSelector::Home | OwnedVolumeSelector::FileStates => {
                name.starts_with("orch-home-")
            }
            OwnedVolumeSelector::Private(mount) => {
                name.starts_with("orch-over-") && name.contains(&mount.to_string())
            }
        };
    if valid {
        Ok(())
    } else {
        Err(CommandError::new(
            ErrorCode::Conflict,
            "the selector does not name an owned home or private-layer volume",
        ))
    }
}

fn source_volumes(
    record: &Record,
    volumes: &[SelectedVolume],
) -> Result<Vec<VolumeRequest>, CommandError> {
    if record.kind == RuntimeKind::Provider
        && volumes
            .iter()
            .any(|volume| volume.selector != OwnedVolumeSelector::Home)
    {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "a provider carries its harness home, never a workspace, private view or connector state",
        ));
    }
    let mut selectors = BTreeSet::new();
    let mut names = BTreeMap::<String, OwnedVolumeSelector>::new();
    volumes
        .iter()
        .map(|selected| {
            selected_name(selected)?;
            let previous = names.insert(selected.volume.name.clone(), selected.selector);
            let repeated = previous.is_some_and(|previous| {
                !matches!(
                    (previous, selected.selector),
                    (OwnedVolumeSelector::Home, OwnedVolumeSelector::FileStates)
                        | (OwnedVolumeSelector::FileStates, OwnedVolumeSelector::Home)
                )
            });
            if !selectors.insert(selected.selector)
                || repeated
                || !record.volumes.contains(&selected.volume.name)
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the requested volume is repeated or does not belong to this runtime",
                ));
            }
            Ok(VolumeRequest {
                name: selected.volume.name.clone(),
                storage: selected.volume.storage.clone(),
                labels: BTreeMap::new(),
            })
        })
        .collect()
}

impl Runtimes {
    fn source_directory(
        &self,
        runtime: Id,
        selected: &SelectedVolume,
    ) -> Result<Option<orch_driver_contract::runtime::DirectoryBinding>, CommandError> {
        let retained = self
            .inner
            .ledger
            .directories(runtime)
            .remove(&selected.volume.name);
        match (&selected.volume.directory, retained) {
            (None, None) => Ok(None),
            (Some(asked), Some(retained)) => {
                let policy = super::lock(&self.inner.policy)
                    .as_ref()
                    .and_then(|store| store.policy())
                    .ok_or_else(|| {
                        CommandError::new(
                            ErrorCode::PolicyRootNotAllowed,
                            "this machine has no environment.roots policy",
                        )
                    })?;
                let current = orch_connector_core::policy::inspect_environment_directory(
                    &policy,
                    std::path::Path::new(&asked.path),
                )?;
                if current != retained {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the borrowed source directory changed since this unit was made",
                    ));
                }
                Ok(Some(retained))
            }
            _ => Err(CommandError::new(
                ErrorCode::Conflict,
                "the selected data source differs from this node's retained binding",
            )),
        }
    }
    /// A target's unsupported driver/helper is said in the plan before any volume is created.
    pub async fn check_volume_carry(
        &self,
        body: &orch_link_contract::commands::NodeVolumeCarryCheck,
    ) -> orch_link_contract::commands::NodeVolumeCarryCheckResult {
        let outcome = async {
            self.driver(&body.driver)?
                .extras()
                .check_volume_carry()
                .await
                .map_err(|error| failure(&error))
        }
        .await;
        orch_link_contract::commands::NodeVolumeCarryCheckResult {
            available: outcome.is_ok(),
            reason: outcome.err().map(|error| error.detail),
        }
    }
    /// Measures on the old node before any source hold or target creation; no data is uploaded.
    /// Performs the same operation without a job progress subscriber.
    pub async fn measure_volumes(
        &self,
        body: &NodeRuntimeVolumesMeasure,
    ) -> Result<NodeRuntimeVolumesMeasureResult, CommandError> {
        self.measure_volumes_reporting(
            body,
            &crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumesMeasure
                    .deadline_secs(),
            )),
        )
        .await
    }

    /// Performs the operation with the caller's live reporting and cooperative cancellation.
    pub async fn measure_volumes_reporting(
        &self,
        body: &NodeRuntimeVolumesMeasure,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<NodeRuntimeVolumesMeasureResult, CommandError> {
        let node = self.clone();
        let body = body.clone();
        crate::work::owned(
            &self.inner.copy_tasks,
            &self.inner.stop,
            work.command_id().unwrap_or_else(orch_types::new_id),
            Arc::clone(work),
            move |work| async move { node.measure_volumes_held(&body, &work).await },
        )
        .await
    }

    async fn measure_volumes_held(
        &self,
        body: &NodeRuntimeVolumesMeasure,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<NodeRuntimeVolumesMeasureResult, CommandError> {
        let _busy = self.inner.busy.enter(body.runtime_id);
        let serial = self.lock_of(body.runtime_id);
        let _held = serial.lock().await;
        let (driver, record) = self.require(body.runtime_id).await?;
        let requests = source_volumes(&record, &body.volumes)?;
        let _volumes = self
            .hold_volumes(
                &record.driver,
                requests.iter().map(|request| request.name.clone()),
                work,
            )
            .await?;
        let mut volumes = Vec::new();
        for (selected, request) in body.volumes.iter().zip(requests) {
            let directory = self.source_directory(body.runtime_id, selected)?;
            let measure = self
                .volume_driver(&driver, work, async {
                    match directory {
                        Some(directory) => {
                            driver
                                .extras()
                                .measure_directory_reporting(&directory, selected.selector, work)
                                .await
                        }
                        None => {
                            driver
                                .extras()
                                .measure_selected_volume_reporting(
                                    &request,
                                    selected.selector,
                                    work,
                                )
                                .await
                        }
                    }
                })
                .await?;
            volumes.push(MeasuredVolume {
                selector: selected.selector,
                measure,
            });
        }
        Ok(NodeRuntimeVolumesMeasureResult { volumes })
    }

    /// A disconnected waiter stops its copy and retains ownership through cleanup and source resume.
    /// Performs the same operation without a job progress subscriber.
    pub async fn export_volumes(
        &self,
        command: Id,
        body: NodeRuntimeVolumesExport,
    ) -> Result<NodeRuntimeVolumesExportResult, CommandError> {
        self.export_volumes_reporting(
            command,
            body,
            crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumesExport.deadline_secs(),
            )),
        )
        .await
    }

    /// Performs the operation with the caller's live reporting and cooperative cancellation.
    pub async fn export_volumes_reporting(
        &self,
        command: Id,
        body: NodeRuntimeVolumesExport,
        work: Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<NodeRuntimeVolumesExportResult, CommandError> {
        let node = self.clone();
        crate::work::owned(
            &self.inner.copy_tasks,
            &self.inner.stop,
            command,
            work,
            move |work| async move { node.export_volumes_held(command, &body, &work).await },
        )
        .await
    }

    async fn export_volumes_held(
        &self,
        command: Id,
        body: &NodeRuntimeVolumesExport,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<NodeRuntimeVolumesExportResult, CommandError> {
        if body.part_bytes <= 0
            || body.part_bytes > orch_link_contract::commands::VOLUME_ARCHIVE_PART_MAX_BYTES
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the archive part size exceeds the node's staging bound",
            ));
        }
        let _busy = self.inner.busy.enter(body.runtime_id);
        let serial = self.lock_of(body.runtime_id);
        let _held = serial.lock().await;
        let (driver, record) = self.require(body.runtime_id).await?;
        let requests = source_volumes(&record, &body.volumes)?;
        let _volumes = self
            .hold_volumes(
                &record.driver,
                requests.iter().map(|request| request.name.clone()),
                work,
            )
            .await?;
        let content = self.content()?;
        let directory = content.directory_of(command);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(internal)?;
        let state = driver
            .contract()
            .inspect(&record.unit)
            .await
            .map_err(|error| failure(&error))?;
        let paused_here = state.state == UnitStateKind::Running
            && state.detail.as_deref() != Some("paused")
            && driver.describe().volume_export_consistency
                != Some(orch_driver_contract::host::VolumeExportConsistency::Driver);
        if paused_here && !driver.describe().suspend {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "this driver cannot make a consistent owned-volume copy while its unit runs; stop it before copying",
            ));
        }
        let outcome = async {
            if paused_here {
                self.inner
                    .ledger
                    .set_volume_copy_hold(body.runtime_id, true)
                    .map_err(internal)?;
                driver
                    .extras()
                    .suspend(&record.unit)
                    .await
                    .map_err(|error| failure(&error))?;
            }
            let copied = async {
                let mut files = Vec::new();
                for (index, request) in requests.iter().enumerate() {
                    let path = directory.join(format!("volume-{index}.archive"));
                    let selected = &body.volumes[index];
                    let source = self.source_directory(body.runtime_id, selected)?;
                    self.volume_driver(&driver, work, async {
                        match source {
                            Some(directory) => {
                                driver
                                    .extras()
                                    .export_directory_reporting(
                                        &directory,
                                        selected.selector,
                                        &path,
                                        work,
                                    )
                                    .await
                            }
                            None => {
                                driver
                                    .extras()
                                    .export_selected_volume_reporting(
                                        request,
                                        selected.selector,
                                        &path,
                                        work,
                                    )
                                    .await
                            }
                        }
                    })
                    .await?;
                    files.push(path);
                }
                Ok::<_, CommandError>(files)
            }
            .await;
            if paused_here {
                driver
                    .extras()
                    .resume(&record.unit)
                    .await
                    .map_err(|error| failure(&error))?;
                self.inner
                    .ledger
                    .set_volume_copy_hold(body.runtime_id, false)
                    .map_err(internal)?;
            }
            let files = copied?;
            let mut volumes = Vec::new();
            for (selected, path) in body.volumes.iter().zip(files) {
                // Resume before the network transfer; a slow link never extends the source hold.
                let transfer_total =
                    i64::try_from(tokio::fs::metadata(&path).await.map_err(internal)?.len())
                        .map_err(internal)?;
                let mut input = tokio::fs::File::open(&path).await.map_err(internal)?;
                let mut parts = Vec::new();
                let mut size = 0i64;
                loop {
                    let part = directory.join("upload.part");
                    let mut output = tokio::fs::File::create(&part).await.map_err(internal)?;
                    let copied = crate::work::copy(
                        &mut (&mut input).take(u64::try_from(body.part_bytes).map_err(internal)?),
                        &mut output,
                        work,
                    )
                    .await
                    .map_err(internal)?;
                    output.sync_all().await.map_err(internal)?;
                    drop(output);
                    if copied == 0 {
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
                drop(input);
                tokio::fs::remove_file(&path).await.map_err(internal)?;
                volumes.push(ExportedVolume {
                    selector: selected.selector,
                    archive: orch_link_contract::commands::VolumeArchive { parts, size },
                });
            }
            Ok(NodeRuntimeVolumesExportResult { volumes })
        }
        .await;
        content.finish(command).await;
        outcome
    }

    /// Imports only into an owned unused successor volume; the node never unpacks into a running namespace.
    /// Performs the same operation without a job progress subscriber.
    pub async fn import_volume(
        &self,
        command: Id,
        body: NodeRuntimeVolumeImport,
    ) -> Result<(), CommandError> {
        self.import_volume_reporting(
            command,
            body,
            crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumeImport.deadline_secs(),
            )),
        )
        .await
    }

    /// Performs the operation with the caller's live reporting and cooperative cancellation.
    pub async fn import_volume_reporting(
        &self,
        command: Id,
        body: NodeRuntimeVolumeImport,
        work: Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), CommandError> {
        let node = self.clone();
        crate::work::owned(
            &self.inner.copy_tasks,
            &self.inner.stop,
            command,
            work,
            move |work| async move { node.import_volume_held(command, &body, &work).await },
        )
        .await
    }

    async fn import_volume_held(
        &self,
        command: Id,
        body: &NodeRuntimeVolumeImport,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), CommandError> {
        let size = body.archive.parts.iter().try_fold(0i64, |sum, part| {
            (part.size > 0
                && part.size <= orch_link_contract::commands::VOLUME_ARCHIVE_PART_MAX_BYTES)
                .then_some(())
                .and_then(|()| sum.checked_add(part.size))
        });
        if body.archive.parts.is_empty() || size != Some(body.archive.size) {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the volume archive does not contain the complete ordered stream",
            ));
        }
        selected_name(&body.selected)?;
        if body.selected.volume.directory.is_some() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "a copy is imported only into new storage made by Orch, never into an existing directory",
            ));
        }
        let _busy = self.inner.busy.enter(body.runtime_id);
        let serial = self.lock_of(body.runtime_id);
        let _held = serial.lock().await;
        let _volumes = self
            .hold_volumes(&body.driver, [body.selected.volume.name.clone()], work)
            .await?;
        if self.inner.ledger.get(body.runtime_id).is_some()
            || !self
                .inner
                .ledger
                .using_volume(&body.selected.volume.name)
                .is_empty()
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "a whole volume is imported before any target unit is created",
            ));
        }
        let driver = self.driver(&body.driver)?;
        let request = VolumeRequest {
            name: body.selected.volume.name.clone(),
            storage: body.selected.volume.storage.clone(),
            labels: BTreeMap::from([
                (
                    if body.provider_id.is_some() {
                        "orch.provider_id".into()
                    } else {
                        "orch.environment_id".into()
                    },
                    body.provider_id.unwrap_or(body.environment_id).to_string(),
                ),
                ("orch.runtime_id".into(), body.runtime_id.to_string()),
                ("orch.carried".into(), "true".into()),
            ]),
        };
        if !driver
            .extras()
            .volume_exists(&request)
            .await
            .map_err(|error| failure(&error))?
        {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "the prepared owned destination volume is absent",
            ));
        }
        let content = self.content()?;
        let outcome = async {
            // One verified part on disk and a 64 KiB pipe; the target never stages the whole volume.
            let (mut output, mut input) = tokio::io::duplex(64 * 1024);
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
                        &request,
                        body.selected.selector,
                        &mut input,
                        work,
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

    /// A node crash during an export cannot leave the old unit paused indefinitely.
    pub async fn recover_volume_copy_holds(&self) {
        let busy = self.inner.busy.ids();
        for runtime in self.inner.ledger.volume_copy_holds() {
            if busy.contains(&runtime) {
                continue;
            }
            let serial = self.lock_of(runtime);
            let Ok(_held) = serial.try_lock() else {
                continue;
            };
            let recovered = async {
                if let Some(record) = self.inner.ledger.get(runtime) {
                    let driver = self.driver(&record.driver)?;
                    let state = driver
                        .contract()
                        .inspect(&record.unit)
                        .await
                        .map_err(|error| failure(&error))?;
                    if state.detail.as_deref() == Some("paused") {
                        driver
                            .extras()
                            .resume(&record.unit)
                            .await
                            .map_err(|error| failure(&error))?;
                    }
                }
                self.inner
                    .ledger
                    .set_volume_copy_hold(runtime, false)
                    .map_err(internal)
            }
            .await;
            if recovered.is_err() {
                tracing::warn!(%runtime, "the source unit of an interrupted volume copy still awaits resume");
            }
        }
    }
}
