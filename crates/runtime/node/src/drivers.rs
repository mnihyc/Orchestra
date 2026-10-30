//! Configured built-in and external driver host.
//!
//! The node treats every driver through the one contract ([`EnvironmentDriver`]) and nothing else in Orch asks
//! which driver is in use: it reads the driver's descriptor. A node loads the drivers its configuration names.
//! A built-in driver is a registration ([`Registrations`]) the program composes (the library below the program does not
//! depend on any implementation); a name that is configured and has no implementation, such as a driver that
//! is not built yet, and an external driver that is not supported yet, are reported as not available and
//! are not pretended. A driver that is registered and could not be loaded (its daemon is not up yet) is tried
//! again by [`DriverHost::retry`], which the node calls on its reconcile interval, and joins the host when it
//! answers.
//!
//! **What the node needs beyond the contract** is [`DriverExtras`]: the contract has no operation for the
//! storages of a host with their sizes, for the progress of a pull or a build, for the digest a registry has
//! behind a tag, for holding a unit's processes still, or for asking whether the connector in a unit has
//! enrolled. A driver that has them gives them (the Docker glue of the program does); one that has not is
//! used through the contract alone and the node does without.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures::future::BoxFuture;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::sources::ImageEnsured;
use orch_driver_contract::units::UnitRef;
use orch_driver_contract::volumes::StorageInfo;
use orch_link_contract::commands::CommandError;
use orch_types::Json;

use crate::images::{BlobDesk, ImageProgress};
use crate::settings::NodeSettings;

/// The code of a command that asks for something that cannot be: a name that is none, a hash that is not one,
/// a request that is not what its kind defines. The catalogue of codes has no word for it, and the command is
/// not one to ask again.
pub const INVALID: &str = "invalid";

/// The code a command fails with when the host cannot make or start a unit for want of memory or disk. The
/// driver reports it in its own words; the system shows them with this code.
pub const NO_CAPACITY: &str = "no_capacity";

/// A driver's typed failure as a command failure, retaining its stable code and retry semantics.
#[must_use]
pub fn failure(error: &DriverError) -> CommandError {
    // The job already names its image phase; retain the registry diagnostic without a second prefix.
    let text = match error {
        DriverError::Image(message) => message.clone(),
        other => other.to_string(),
    };
    CommandError {
        code: error.code().to_owned(),
        detail: text,
        retryable: error.retryable(),
        details: None,
    }
}

/// A legacy future is dropped on cancel; its caller still joins the driver's external cleanup.
async fn legacy_volume<T>(
    work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    operation: impl std::future::Future<Output = Result<T, DriverError>>,
) -> Result<T, DriverError> {
    tokio::select! {
        biased;
        () = work.cancelled() => Err(DriverError::Unavailable("the filesystem work was cancelled".into())),
        result = operation => result,
    }
}

/// What a driver can do beyond the contract, which the node uses where it is there. Every method has a
/// default that says the driver does not.
#[async_trait]
pub trait DriverExtras: Send + Sync + 'static {
    /// Fingerprints an owned volume read-only, leaving all content on its machine.
    async fn fingerprint_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<orch_types::Hash, DriverError> {
        let _ = volume;
        Err(DriverError::Unsupported(
            "fingerprinting mount content".into(),
        ))
    }

    /// Fingerprints a pinned borrowed directory read-only without following links outside it.
    async fn fingerprint_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
    ) -> Result<orch_types::Hash, DriverError> {
        let _ = directory;
        Err(DriverError::Unsupported(
            "fingerprinting directory content".into(),
        ))
    }

    /// Reads an owned volume with the command's live reports and cooperative cancellation.
    async fn fingerprint_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_types::Hash, DriverError> {
        let _ = work;
        self.fingerprint_volume(volume).await
    }

    /// Reads a pinned directory with the command's live reports and cooperative cancellation.
    async fn fingerprint_directory_reporting(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_types::Hash, DriverError> {
        let _ = work;
        self.fingerprint_directory(directory).await
    }

    /// Stops and joins a command's external helper; success proves it can no longer write.
    /// This also settles work dropped by a legacy volume method before serialization is released.
    async fn stop_volume_work(&self, _command: orch_types::Id) -> Result<(), DriverError> {
        Ok(())
    }
    /// Counts an owned volume with cooperative cancellation and live reports.
    async fn measure_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        legacy_volume(work, self.measure_volume(volume)).await
    }
    /// Counts the selected owned subtree with cooperative cancellation and live reports.
    async fn measure_selected_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        legacy_volume(work, self.measure_selected_volume(volume, selector)).await
    }
    /// Counts a checked borrowed directory with cooperative cancellation and live reports.
    async fn measure_directory_reporting(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        legacy_volume(work, self.measure_directory(directory, selector)).await
    }
    /// Exports a checked directory with cooperative cancellation and live reports.
    async fn export_directory_reporting(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        legacy_volume(work, self.export_directory(directory, selector, into)).await
    }
    /// Exports an owned volume with cooperative cancellation and live reports.
    async fn export_selected_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        legacy_volume(work, self.export_selected_volume(volume, selector, into)).await
    }
    /// Imports an unused owned volume with cooperative cancellation and live reports.
    async fn import_selected_volume_stream_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        legacy_volume(
            work,
            self.import_selected_volume_stream(volume, selector, input),
        )
        .await
    }

    /// Runs a cached harness in a fresh networkless unit, with no shared home or model credentials.
    async fn harness_conformance(
        &self,
        _request: &orch_link_contract::commands::NodeToolConformance,
        _program: &std::path::Path,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<orch_link_contract::commands::NodeToolConformanceResult, DriverError> {
        Err(DriverError::Unsupported(
            "isolated native harness checks".to_owned(),
        ))
    }
    /// Measure a borrowed root, without treating it as an owned volume or changing its metadata.
    async fn measure_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        let _ = (directory, selector);
        Err(DriverError::Unsupported(
            "measuring an existing directory for an explicit copy".into(),
        ))
    }
    /// Copy a checked borrowed home read-only; imports always go to newly owned target storage.
    async fn export_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        let _ = (directory, selector, into);
        Err(DriverError::Unsupported(
            "copying an existing directory".into(),
        ))
    }
    /// Measures the fixed store scope or an ordinary whole owned volume.
    async fn measure_selected_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
            return Err(DriverError::Unsupported(
                "measuring the owned file-state store".into(),
            ));
        }
        self.measure_volume(volume).await
    }

    /// Carries only the declared persistent store parts for a file_states selector.
    async fn export_selected_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
            return Err(DriverError::Unsupported(
                "carrying the owned file-state store".into(),
            ));
        }
        self.export_volume(volume, into).await
    }

    /// Streams into that fixed scope before a successor has ever used its home.
    async fn import_selected_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
            return Err(DriverError::Unsupported(
                "importing the owned file-state store".into(),
            ));
        }
        self.import_volume_stream(volume, input).await
    }

    /// Checks the whole-volume copy implementation before any destination volume or unit is made.
    async fn check_volume_carry(&self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(
            "carrying whole owned volumes".into(),
        ))
    }
    /// Measures an owned home or private-layer volume without opening file contents.
    async fn measure_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        let _ = volume;
        Err(DriverError::Unsupported(
            "measuring whole owned volumes".into(),
        ))
    }

    /// Streams a quiescent owned volume, including owners/modes/xattrs and overlay markers, to a file.
    /// Streams directly into a bounded pipe, so the node stages only one blob-sized part.
    async fn export_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        output: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<(), DriverError> {
        let _ = (volume, output);
        Err(DriverError::Unsupported(
            "streaming whole owned volumes".into(),
        ))
    }

    /// Reads ordered archive parts through a pipe before any unit mounts the destination.
    async fn import_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        let _ = (volume, input);
        Err(DriverError::Unsupported(
            "streaming whole owned volumes".into(),
        ))
    }

    /// Streams a quiescent owned volume, including owners/modes/xattrs and overlay markers, to a file.
    async fn export_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        let _ = (volume, into);
        Err(DriverError::Unsupported(
            "carrying whole owned volumes".into(),
        ))
    }

    /// Restores a whole archive into a fresh owned volume before a target unit uses it.
    async fn import_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        from: &std::path::Path,
    ) -> Result<(), DriverError> {
        let _ = (volume, from);
        Err(DriverError::Unsupported(
            "carrying whole owned volumes".into(),
        ))
    }
    /// Verifies an existing owned volume without creating it; false means confirmed absence.
    async fn volume_exists(
        &self,
        request: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<bool, DriverError> {
        let _ = request;
        Err(DriverError::Unsupported(
            "verifying kept volumes".to_owned(),
        ))
    }
    /// The named places for volumes on this host, with their sizes, as the driver sees them; `None` when the
    /// driver cannot say.
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        None
    }

    /// Makes the source of a spec available, telling `progress` what it does as it goes. `Unsupported` sends
    /// the node to the contract's `ensure_image`, which tells nothing.
    async fn ensure_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        let _ = (spec, progress);
        Err(DriverError::Unsupported("progress".to_owned()))
    }

    /// Builds the source of a spec again with its base images pulled anew, so that a build that has fallen
    /// behind its bases is brought up to date. `Unsupported` sends the node to `ensure_image`.
    async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        let _ = (spec, progress);
        Err(DriverError::Unsupported("rebuild".to_owned()))
    }

    /// The digest the registry has behind a reference now, without pulling the image: how a host notices that
    /// a tag has moved. `None` when the registry has none, or when the driver cannot look.
    async fn registry_digest(&self, reference: &str) -> Result<Option<String>, DriverError> {
        let _ = reference;
        Ok(None)
    }

    /// Holds a unit's processes still, so that its files do not change while its environment is moved.
    async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _ = unit;
        Err(DriverError::Unsupported("suspend".to_owned()))
    }

    /// Lets go of the processes that [`DriverExtras::suspend`] held.
    async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _ = unit;
        Err(DriverError::Unsupported("suspend".to_owned()))
    }

    /// Whether the connector in the unit has enrolled; `None` when the driver cannot tell.
    async fn enrolled(&self, unit: &UnitRef) -> Option<bool> {
        let _ = unit;
        None
    }

    /// Writes the files a stopped unit keeps below `path` of its state (its data directory), with their owners
    /// and modes, as a tar archive whose names begin with `path`, to the file `into`; `false` where the unit keeps
    /// nothing there. What a runtime moved to another host takes with it.
    async fn export_state(
        &self,
        unit: &UnitRef,
        path: &str,
        into: &std::path::Path,
    ) -> Result<bool, DriverError> {
        let _ = (unit, path, into);
        Err(DriverError::Unsupported(
            "moving a unit's files to another host".to_owned(),
        ))
    }

    /// Puts the files of a tar archive [`DriverExtras::export_state`] wrote into a unit's state, with their owners
    /// and modes, before the unit first starts.
    async fn import_state(
        &self,
        unit: &UnitRef,
        from: &std::path::Path,
    ) -> Result<(), DriverError> {
        let _ = (unit, from);
        Err(DriverError::Unsupported(
            "moving a unit's files to another host".to_owned(),
        ))
    }
}

/// A driver that has nothing beyond the contract.
fn transfer_error(error: impl std::fmt::Display) -> DriverError {
    DriverError::Internal(error.to_string())
}

struct ContractExtras {
    driver: Arc<dyn EnvironmentDriver>,
    staging: Arc<RwLock<Option<std::path::PathBuf>>>,
}

struct VolumeInputWork {
    inner: Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ready: tokio::sync::Notify,
}

#[async_trait]
impl orch_driver_contract::volumes::VolumeWork for VolumeInputWork {
    fn input_ready(&self) {
        self.inner.input_ready();
        // One importer owns one feeder; the permit also covers readiness before its first poll.
        self.ready.notify_one();
    }
    fn command_id(&self) -> Option<orch_types::Id> {
        self.inner.command_id()
    }
    fn report(&self, progress: orch_driver_contract::volumes::VolumeProgress) {
        self.inner.report(progress);
    }
    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    async fn cancelled(&self) {
        self.inner.cancelled().await;
    }
    fn timeout_secs(&self) -> u64 {
        self.inner.timeout_secs()
    }
}

impl ContractExtras {
    async fn archive_pipe(&self) -> Result<(tempfile::TempDir, std::path::PathBuf), DriverError> {
        let directory = self
            .staging
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                DriverError::Unavailable("the driver has no node transfer staging".into())
            })?;
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(transfer_error)?;
        let staging = tempfile::Builder::new()
            .prefix("volume-")
            .tempdir_in(directory)
            .map_err(transfer_error)?;
        let path = staging.path().join("archive.pipe");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &path,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .map_err(transfer_error)?;
        Ok((staging, path))
    }
}

#[async_trait]
impl DriverExtras for ContractExtras {
    async fn stop_volume_work(&self, command: orch_types::Id) -> Result<(), DriverError> {
        self.driver.stop_volume_work(command).await
    }
    async fn check_volume_carry(&self) -> Result<(), DriverError> {
        self.driver.check_volume_carry().await
    }
    async fn measure_selected_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.driver.measure_volume(volume, selector, work).await
    }
    async fn measure_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.measure_selected_volume_reporting(
            volume,
            orch_driver_contract::volumes::OwnedVolumeSelector::Home,
            work,
        )
        .await
    }
    async fn export_selected_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        self.driver
            .export_volume(volume, selector, into, work)
            .await
    }
    async fn import_selected_volume_stream_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        use tokio::io::AsyncWriteExt;
        let (_staging, path) = self.archive_pipe().await?;
        // The private reader allows the writer to open without a polling loop. It consumes no bytes.
        let _ready = tokio::net::unix::pipe::OpenOptions::new()
            .open_receiver(&path)
            .map_err(transfer_error)?;
        let mut output = tokio::net::unix::pipe::OpenOptions::new()
            .open_sender(&path)
            .map_err(transfer_error)?;
        let ready = Arc::new(VolumeInputWork {
            inner: Arc::clone(work),
            ready: tokio::sync::Notify::new(),
        });
        let reporting: Arc<dyn orch_driver_contract::volumes::VolumeWork> = ready.clone();
        let feeding = async {
            ready.ready.notified().await;
            crate::work::copy(input, &mut output, work)
                .await
                .map_err(transfer_error)?;
            output.shutdown().await.map_err(transfer_error)?;
            drop(output);
            Ok::<_, DriverError>(())
        };
        let importing = self
            .driver
            .import_volume(volume, selector, &path, &reporting);
        let outcome = tokio::select! {
            result = tokio::time::timeout(std::time::Duration::from_secs(work.timeout_secs()), async { tokio::try_join!(feeding, importing).map(|_| ()) }) => match result {Ok(outcome)=>outcome,Err(_)=>Err(DriverError::Timeout)},
            () = work.cancelled() => Err(DriverError::Timeout),
        };
        if matches!(outcome, Err(DriverError::Timeout))
            && let Some(command) = work.command_id()
        {
            self.driver.stop_volume_work(command).await?;
        }
        outcome
    }
    async fn measure_selected_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.measure_selected_volume_reporting(
            volume,
            selector,
            &crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumesMeasure
                    .deadline_secs(),
            )),
        )
        .await
    }
    async fn measure_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.measure_selected_volume(
            volume,
            orch_driver_contract::volumes::OwnedVolumeSelector::Home,
        )
        .await
    }
    async fn export_selected_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.export_selected_volume_reporting(
            volume,
            selector,
            into,
            &crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumesExport.deadline_secs(),
            )),
        )
        .await
    }
    async fn export_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.export_selected_volume(
            volume,
            orch_driver_contract::volumes::OwnedVolumeSelector::Home,
            into,
        )
        .await
    }
    async fn import_selected_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.import_selected_volume_stream_reporting(
            volume,
            selector,
            input,
            &crate::work::silent(u64::from(
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumeImport.deadline_secs(),
            )),
        )
        .await
    }
    async fn import_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.import_selected_volume_stream(
            volume,
            orch_driver_contract::volumes::OwnedVolumeSelector::Home,
            input,
        )
        .await
    }
    async fn import_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        from: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.driver
            .import_volume(
                volume,
                orch_driver_contract::volumes::OwnedVolumeSelector::Home,
                from,
                &crate::work::silent(u64::from(
                    orch_link_contract::commands::CommandKind::NodeRuntimeVolumeImport
                        .deadline_secs(),
                )),
            )
            .await
    }
    async fn export_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        output: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<(), DriverError> {
        let (_staging, path) = self.archive_pipe().await?;
        let mut input = tokio::net::unix::pipe::OpenOptions::new()
            .open_receiver(&path)
            .map_err(transfer_error)?;
        // Keep a writer until the driver's writer has finished, so the initial read cannot see EOF.
        let ready = tokio::net::unix::pipe::OpenOptions::new()
            .open_sender(&path)
            .map_err(transfer_error)?;
        let work = crate::work::silent(u64::from(
            orch_link_contract::commands::CommandKind::NodeRuntimeVolumesExport.deadline_secs(),
        ));
        let exporting = async {
            let result = self
                .driver
                .export_volume(
                    volume,
                    orch_driver_contract::volumes::OwnedVolumeSelector::Home,
                    &path,
                    &work,
                )
                .await;
            drop(ready);
            result
        };
        let reading = async {
            crate::work::copy(&mut input, output, &work)
                .await
                .map_err(transfer_error)?;
            Ok::<_, DriverError>(())
        };
        tokio::try_join!(exporting, reading)?;
        Ok(())
    }

    fn storages(&self) -> Option<Vec<StorageInfo>> {
        self.driver.storages()
    }
    async fn volume_exists(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<bool, DriverError> {
        self.driver.volume_exists(volume).await
    }
    async fn ensure_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.driver.ensure_image_reporting(spec, progress).await
    }
    async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.driver.rebuild_image(spec, progress).await
    }
    async fn registry_digest(&self, reference: &str) -> Result<Option<String>, DriverError> {
        self.driver.registry_digest(reference).await
    }
    async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.driver.suspend(unit).await
    }
    async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.driver.resume(unit).await
    }
}

/// A driver as the node holds it: the contract, and what it has beyond it.
#[derive(Clone)]
pub struct HostDriver {
    driver: Arc<dyn EnvironmentDriver>,
    extras: Arc<dyn DriverExtras>,
    configuration_schema: Option<Json>,
    configuration_check: Option<ConfigurationCheck>,
    transfer_staging: Arc<RwLock<Option<std::path::PathBuf>>>,
}

/// A composition-supplied validator for an implementation's connection settings.
pub type ConfigurationCheck = Arc<dyn Fn(&Json) -> Result<(), DriverError> + Send + Sync>;

impl std::fmt::Debug for HostDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDriver")
            .field("name", &self.driver.describe().name)
            .finish_non_exhaustive()
    }
}

impl HostDriver {
    /// A driver that is used through the contract alone.
    #[must_use]
    pub fn new(driver: Arc<dyn EnvironmentDriver>) -> Self {
        let transfer_staging = Arc::new(RwLock::new(None));
        Self {
            extras: Arc::new(ContractExtras {
                driver: Arc::clone(&driver),
                staging: Arc::clone(&transfer_staging),
            }),
            transfer_staging,
            driver,
            configuration_schema: None,
            configuration_check: None,
        }
    }

    /// The same driver with what it has beyond the contract.
    #[must_use]
    pub fn with_extras(mut self, extras: Arc<dyn DriverExtras>) -> Self {
        self.extras = extras;
        self
    }

    /// Supplies a built-in implementation's schema and its matching validator at composition.
    #[must_use]
    pub fn with_configuration(mut self, schema: Json, check: ConfigurationCheck) -> Self {
        self.configuration_schema = Some(schema);
        self.configuration_check = Some(check);
        self
    }

    /// Checks a driver-owned configuration before activating it.
    pub fn validate_configuration(&self, configuration: &Json) -> Result<(), DriverError> {
        if let Some(check) = &self.configuration_check {
            check(configuration)
        } else {
            self.driver.validate_configuration(configuration)
        }
    }

    /// What it declares.
    #[must_use]
    pub fn describe(&self) -> DriverDescriptor {
        let mut described = self.driver.describe();
        if self.configuration_schema.is_some() {
            described
                .configuration_schema
                .clone_from(&self.configuration_schema);
        }
        described.live_changes.gateway_given = orch_driver_contract::capabilities::GATEWAY_GIVEN
            .iter()
            .filter(|key| **key != "port_host" || described.host_ports.supported)
            .map(|key| (*key).into())
            .collect();
        described
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> String {
        self.describe().name
    }

    /// The driver through the contract.
    #[must_use]
    pub fn contract(&self) -> &dyn EnvironmentDriver {
        self.driver.as_ref()
    }

    /// What it has beyond the contract.
    #[must_use]
    pub fn extras(&self) -> &dyn DriverExtras {
        self.extras.as_ref()
    }

    /// Makes the source of a spec available, with progress where the driver can tell it.
    pub async fn ensure_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        match self.extras.ensure_image(spec, progress).await {
            Err(DriverError::Unsupported(_)) => self.driver.ensure_image(spec).await,
            other => other,
        }
    }

    /// Builds a source again, or makes it available when the driver has no such thing as building again.
    pub async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        match self.extras.rebuild_image(spec, progress).await {
            Err(DriverError::Unsupported(_)) => self.ensure_image(spec, progress).await,
            other => other,
        }
    }
}

/// What a registration is given when it makes its driver.
#[derive(Clone)]
pub struct LoadContext {
    /// This registration's driver-owned connection object, when the machine policy names one.
    pub configuration: Option<Json>,
    /// How the node is configured.
    pub settings: NodeSettings,
    /// How blobs are fetched for the commands that name them, which a driver that builds or loads images
    /// from archives needs.
    pub blobs: Arc<BlobDesk>,
}

/// Makes a driver.
pub type Factory =
    Arc<dyn Fn(LoadContext) -> BoxFuture<'static, Result<HostDriver, DriverError>> + Send + Sync>;

#[derive(Clone)]
enum Registration {
    Built(Factory),
    NotBuilt(String),
}

/// The built-in drivers a program composes, by the name the configuration gives them.
#[derive(Clone, Default)]
pub struct Registrations {
    entries: BTreeMap<String, Registration>,
    schemas: BTreeMap<String, orch_types::Json>,
}

impl std::fmt::Debug for Registrations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.entries.keys()).finish()
    }
}

impl Registrations {
    /// No drivers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A driver that is built, under its name.
    #[must_use]
    pub fn register(mut self, name: &str, factory: Factory) -> Self {
        self.schemas.remove(name);
        self.entries
            .insert(name.to_owned(), Registration::Built(factory));
        self
    }

    /// A built-in driver's setup schema, available even before its factory succeeds.
    #[must_use]
    pub fn register_with_schema(
        self,
        name: &str,
        factory: Factory,
        schema: orch_types::Json,
    ) -> Self {
        let mut registrations = self.register(name, factory);
        registrations.schemas.insert(name.to_owned(), schema);
        registrations
    }

    /// A name that is known and has no implementation yet, with the sentence that says so.
    #[must_use]
    pub fn not_built(mut self, name: &str, why: &str) -> Self {
        self.schemas.remove(name);
        self.entries
            .insert(name.to_owned(), Registration::NotBuilt(why.to_owned()));
        self
    }
}

/// The drivers a node has loaded, and the ones it was told to load and could not.
pub struct DriverHost {
    registrations: Registrations,
    context: LoadContext,
    wanted: Vec<String>,
    loaded: RwLock<BTreeMap<String, Arc<HostDriver>>>,
    failures: RwLock<BTreeMap<String, String>>,
}

impl std::fmt::Debug for DriverHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverHost")
            .field("loaded", &self.names())
            .field("failures", &self.failures())
            .finish_non_exhaustive()
    }
}

impl DriverHost {
    /// Loads the drivers the settings name, once each. A name with no registration, a registration that is
    /// not built, a driver that cannot be made and a declaration that does not hold together are each
    /// recorded as a failure with its reason; none of them stops the node. The external drivers the operator
    /// registered are recorded as not available.
    pub async fn load(
        settings: &NodeSettings,
        registrations: Registrations,
        blobs: Arc<BlobDesk>,
    ) -> Arc<Self> {
        let host = Self {
            context: LoadContext {
                configuration: None,
                settings: settings.clone(),
                blobs,
            },
            registrations,
            wanted: settings
                .drivers
                .iter()
                .chain(&settings.external_drivers)
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            loaded: RwLock::new(BTreeMap::new()),
            failures: RwLock::new(BTreeMap::new()),
        };
        host.try_all(true).await;
        Arc::new(host)
    }

    /// A host over drivers that are already made, which load nothing: for a program that has made its driver
    /// itself, and for tests.
    #[must_use]
    pub fn of(
        settings: &NodeSettings,
        blobs: Arc<BlobDesk>,
        drivers: Vec<HostDriver>,
    ) -> Arc<Self> {
        let host = Self {
            context: LoadContext {
                configuration: None,
                settings: settings.clone(),
                blobs,
            },
            registrations: Registrations::new(),
            wanted: Vec::new(),
            loaded: RwLock::new(BTreeMap::new()),
            failures: RwLock::new(BTreeMap::new()),
        };
        for driver in drivers {
            host.admit(driver);
        }
        Arc::new(host)
    }

    /// Notes that a driver is not available, and says so in the log when that is news.
    fn fail(&self, name: &str, why: &str) {
        let before = self
            .failures
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned(), why.to_owned());
        if before.as_deref() != Some(why) {
            tracing::warn!(driver = name, "the driver is not available: {why}");
        }
    }

    fn admit(&self, driver: HostDriver) -> Option<String> {
        let descriptor = driver.describe();
        *driver
            .transfer_staging
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(self.context.settings.data_dir.join("driver-transfers"));
        let scope = descriptor
            .host_scope
            .unwrap_or(orch_driver_contract::host::HostScope::Node);
        if self.all().iter().any(|(_, loaded)| {
            let loaded_scope = loaded
                .describe()
                .host_scope
                .unwrap_or(orch_driver_contract::host::HostScope::Node);
            loaded_scope != scope || scope == orch_driver_contract::host::HostScope::Driver
        }) {
            self.fail(&descriptor.name, "one host must have one resource scope; register a separate host for this driver's destination");
            return None;
        }
        if let Some(configuration) = self
            .context
            .settings
            .driver_configurations
            .get(&descriptor.name)
            && let Err(error) = driver.validate_configuration(configuration)
        {
            self.fail(
                &descriptor.name,
                &format!("its configuration is invalid: {error}"),
            );
            return None;
        }
        if let Err(faults) = descriptor.check() {
            let why = faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            self.fail(
                &descriptor.name,
                &format!("its declaration does not hold together: {why}"),
            );
            return None;
        }
        self.failures
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&descriptor.name);
        self.loaded
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(descriptor.name.clone(), Arc::new(driver));
        tracing::info!(driver = %descriptor.name, "the driver is loaded");
        Some(descriptor.name)
    }

    /// Loads the drivers that are wanted and not loaded. A name with no implementation is noted at the first
    /// try and not again: trying cannot change it.
    async fn try_all(&self, first: bool) -> Vec<String> {
        let mut made = Vec::new();
        for name in &self.wanted {
            if self.get(name).is_some() {
                continue;
            }
            match self.registrations.entries.get(name) {
                None if first => self.fail(name, "this node has no such driver"),
                Some(Registration::NotBuilt(why)) if first => self.fail(name, why),
                None | Some(Registration::NotBuilt(_)) => {}
                Some(Registration::Built(factory)) => match factory(LoadContext {
                    configuration: self
                        .context
                        .settings
                        .driver_configurations
                        .get(name)
                        .cloned(),
                    ..self.context.clone()
                })
                .await
                {
                    Ok(driver) => {
                        if driver.name() != *name {
                            self.fail(name, "the registered driver describes a different name");
                            continue;
                        }
                        if let Some(name) = self.admit(driver) {
                            made.push(name);
                        }
                    }
                    Err(error) => self.fail(name, &error.to_string()),
                },
            }
        }
        made
    }

    /// Tries again the registered drivers that could not be loaded. Answers the names that were loaded now.
    pub async fn retry(&self) -> Vec<String> {
        if self.failures().is_empty() {
            return Vec::new();
        }
        self.try_all(false).await
    }

    /// The driver of that name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<HostDriver>> {
        self.loaded
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// New work uses the connection that was actually opened, including when an entry is removed.
    pub fn check_configuration(
        &self,
        name: &str,
        configured: Option<&Json>,
    ) -> Result<(), CommandError> {
        if self.context.settings.driver_configurations.get(name) != configured {
            return Err(CommandError::new(
                orch_link_contract::commands::ErrorCode::Unsupported,
                "drivers (node role): connection settings changed; restart orch-node before making data or a unit",
            ));
        }
        if let Some(configuration) = configured
            && let Some(driver) = self.get(name)
        {
            driver
                .validate_configuration(configuration)
                .map_err(|error| failure(&error))?;
        }
        Ok(())
    }

    /// The names of the drivers loaded.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.loaded
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// Every driver loaded, with its name.
    #[must_use]
    pub fn all(&self) -> Vec<(String, Arc<HostDriver>)> {
        self.loaded
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(name, driver)| (name.clone(), Arc::clone(driver)))
            .collect()
    }

    /// What each loaded driver declares, by name: what the system learns of the host's drivers.
    #[must_use]
    pub fn descriptors(&self) -> BTreeMap<String, DriverDescriptor> {
        self.all()
            .into_iter()
            .map(|(name, driver)| (name, driver.describe()))
            .collect()
    }

    /// Setup schemas declared by built-in registrations, independently of loaded drivers.
    #[must_use]
    pub fn configuration_schemas(&self) -> BTreeMap<String, orch_types::Json> {
        self.registrations.schemas.clone()
    }

    /// The drivers that were wanted and are not loaded, with why.
    #[must_use]
    pub fn failures(&self) -> BTreeMap<String, String> {
        self.failures
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The settings the host was made with.
    #[must_use]
    pub fn settings(&self) -> &NodeSettings {
        &self.context.settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::doubles::driver::MemoryDriver;

    fn blobs() -> Arc<BlobDesk> {
        Arc::new(BlobDesk::new())
    }

    fn made(name: &'static str) -> Factory {
        Arc::new(move |_| Box::pin(async move { Ok(HostDriver::new(MemoryDriver::shared(name))) }))
    }

    #[tokio::test]
    async fn the_drivers_the_configuration_names_are_loaded_and_the_rest_are_reported_as_not_available()
     {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = NodeSettings::in_dir(dir.path());
        settings.drivers = vec!["docker".into(), "host".into(), "invented".into()];
        settings.external_drivers = vec!["firecracker".into()];
        let registrations = Registrations::new()
            .register("docker", made("docker"))
            .not_built("host", "the host driver is not built yet");
        let host = DriverHost::load(&settings, registrations, blobs()).await;
        assert_eq!(host.names(), ["docker"]);
        let failures = host.failures();
        assert!(failures["host"].contains("not built"), "{failures:?}");
        assert!(failures["invented"].contains("no such driver"));
        assert!(failures["firecracker"].contains("no such driver"));
        assert!(host.descriptors().contains_key("docker"));
    }

    #[tokio::test]
    async fn a_driver_that_could_not_be_made_joins_the_host_when_a_later_try_works() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = NodeSettings::in_dir(dir.path());
        settings.drivers = vec!["docker".into()];
        let up = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let factory: Factory = {
            let up = Arc::clone(&up);
            Arc::new(move |_| {
                let up = Arc::clone(&up);
                Box::pin(async move {
                    if up.load(std::sync::atomic::Ordering::SeqCst) {
                        Ok(HostDriver::new(MemoryDriver::shared("docker")))
                    } else {
                        Err(DriverError::Unavailable("no daemon at the socket".into()))
                    }
                })
            })
        };
        let host = DriverHost::load(
            &settings,
            Registrations::new().register("docker", factory),
            blobs(),
        )
        .await;
        assert!(host.names().is_empty());
        assert!(host.failures()["docker"].contains("no daemon"));
        assert!(host.retry().await.is_empty(), "still down");
        up.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(host.retry().await, ["docker"]);
        assert!(host.failures().is_empty());
    }

    #[tokio::test]
    async fn a_failed_driver_reports_its_setup_schema_without_becoming_schedulable() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = NodeSettings::in_dir(dir.path());
        settings.drivers = vec!["cluster".into()];
        let factory: Factory = Arc::new(|_| {
            Box::pin(async { Err(DriverError::Unavailable("cluster is offline".into())) })
        });
        let schema = serde_json::json!({"type": "object", "required": ["namespace"]});
        let host = DriverHost::load(
            &settings,
            Registrations::new().register_with_schema("cluster", factory, schema.clone()),
            blobs(),
        )
        .await;
        assert!(host.descriptors().is_empty());
        assert!(host.failures().contains_key("cluster"));
        assert_eq!(host.configuration_schemas().get("cluster"), Some(&schema));
    }

    #[test]
    fn typed_capacity_and_busy_errors_reach_the_caller_without_guessing_from_words() {
        let full = failure(&DriverError::NoCapacity(
            "container create: no space left on device".into(),
        ));
        assert_eq!(full.code, NO_CAPACITY);
        assert!(full.detail.contains("no space left"));
        let busy = failure(&DriverError::Busy("a transfer owns this unit".into()));
        assert_eq!((busy.code.as_str(), busy.retryable), ("busy", true));
        let unrelated = failure(&DriverError::Internal(
            "test text: cannot allocate memory".into(),
        ));
        assert_eq!(
            unrelated.code, "internal",
            "driver words never invent a capacity failure"
        );
        let other = failure(&DriverError::Image("pull access denied".into()));
        assert_eq!(other.code, "image_failed");
        assert_eq!(other.detail, "pull access denied");
        assert!(!other.retryable);
    }
}
