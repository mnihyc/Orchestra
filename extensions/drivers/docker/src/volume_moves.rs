//! Whole owned home and private-layer volumes, measured and streamed through the trusted static helper before first use.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bollard::container::LogOutput;
use bollard::models::{ContainerCreateBody, HostConfig, HostConfigLogConfig};
use bollard::query_parameters::{
    AttachContainerOptionsBuilder, CreateContainerOptionsBuilder, ListContainersOptionsBuilder,
    StartContainerOptions, WaitContainerOptions,
};
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::volumes::{VolumeMeasure, VolumeProgress, VolumeRequest, VolumeWork};
use tokio::io::AsyncWriteExt;

use crate::client::map_error;
use crate::driver::DockerDriver;
use crate::units::{LABEL_PREPARE, driver_labels};

const HELPER_BINARY: &str = "/orch-runtime-helper";
/// A measurement is two integers; this bounds an invalid or incompatible helper reply.
const MEASUREMENT_MAX_BYTES: usize = 64 * 1024;

tokio::task_local! {
    static WORK: Arc<dyn VolumeWork>;
}

/// Runs a volume operation with its caller's reporting and cancellation context, scoped to this task alone.
pub async fn with_work<F: std::future::Future>(
    work: Arc<dyn VolumeWork>,
    operation: F,
) -> F::Output {
    WORK.scope(work, operation).await
}

impl DockerDriver {
    /// Reads an owned mount's data through a read-only helper binding.
    pub async fn fingerprint_volume(
        &self,
        volume: &VolumeRequest,
    ) -> Result<orch_types::Hash, DriverError> {
        self.carried_volume(volume).await?;
        let bytes = self
            .volume_helper("volume-fingerprint", volume, None, None)
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| {
            DriverError::Internal("the helper returned no valid content fingerprint".into())
        })
    }

    /// Reads a directory after the helper verifies its pinned identity at the read-only binding.
    pub async fn fingerprint_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
    ) -> Result<orch_types::Hash, DriverError> {
        self.check_volume_carry().await?;
        let bytes = self
            .bound_volume_helper(
                "volume-fingerprint",
                crate::mounts::bind(&directory.path, "/volume", true),
                Some(directory),
                None,
                None,
            )
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| {
            DriverError::Internal("the helper returned no valid content fingerprint".into())
        })
    }

    /// Removes a helper retained by an interrupted command, including after the node restarts.
    pub async fn stop_volume_work(&self, command: orch_types::Id) -> Result<(), DriverError> {
        remove_until_stopped(
            &self.daemon,
            &format!("orch-volume-{}", command.simple()),
            Duration::from_millis(self.config.helper_cleanup_retry_ms),
        )
        .await;
        Ok(())
    }
    /// Measures only the persistent file-state store on the owned home volume.
    pub async fn measure_file_states(
        &self,
        volume: &VolumeRequest,
    ) -> Result<VolumeMeasure, DriverError> {
        self.carried_volume(volume).await?;
        let bytes = self
            .volume_helper("file-states-measure", volume, None, None)
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| {
            DriverError::Internal("the file-state helper returned no complete measurement".into())
        })
    }

    /// Exports the closed file-state format, excluding temporary command credentials.
    pub async fn export_file_states(
        &self,
        volume: &VolumeRequest,
        into: &Path,
    ) -> Result<(), DriverError> {
        self.carried_volume(volume).await?;
        let mut file = tokio::fs::File::create(into)
            .await
            .map_err(|_| DriverError::Internal("no room for the file-state archive".into()))?;
        self.volume_helper("file-states-export", volume, None, Some(&mut file))
            .await?;
        file.sync_all().await.map_err(|_| {
            DriverError::Internal("the file-state archive could not be flushed".into())
        })?;
        Ok(())
    }

    /// Imports the original roots, objects and journals before a successor uses this home.
    pub async fn import_file_states_stream(
        &self,
        volume: &VolumeRequest,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.carried_volume(volume).await?;
        let users = self
            .daemon
            .docker()
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&HashMap::from([(
                        "volume".to_owned(),
                        vec![volume.name.clone()],
                    )]))
                    .build(),
            ))
            .await
            .map_err(map_error)?;
        if !users.is_empty() {
            return Err(DriverError::Unavailable(
                "the file-state store is imported before any target unit exists".into(),
            ));
        }
        self.volume_helper("file-states-import", volume, Some(input), None)
            .await?;
        Ok(())
    }

    async fn carried_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        if !self.volume_exists(volume).await? {
            return Err(DriverError::NotFound(
                "the owned volume to carry is absent".into(),
            ));
        }
        self.check_volume_carry().await
    }

    /// A read-only target preflight checks the helper before any destination is made.
    pub async fn check_volume_carry(&self) -> Result<(), DriverError> {
        if self.daemon.facts().os_type != "linux" || self.daemon.facts().rootless {
            return Err(DriverError::Unsupported(
                "whole volume copies require Linux owners, xattrs and device nodes".into(),
            ));
        }
        let binary = self.config.live_helper_binary.as_deref().ok_or_else(|| {
            DriverError::Unsupported(
                "whole volume copies need the configured static node helper".into(),
            )
        })?;
        if self.config.live_helper_image.is_none() || !crate::live_mounts::static_binary(binary) {
            return Err(DriverError::Unsupported(
                "whole volume copies need a trusted image and a static node helper".into(),
            ));
        }
        self.daemon
            .docker()
            .inspect_image(self.config.live_helper_image.as_deref().unwrap_or_default())
            .await
            .map_err(|_| {
                DriverError::Unsupported(
                    "the trusted volume helper image must be preloaded on this host".into(),
                )
            })?;
        Ok(())
    }

    /// Measurements happen on the source host before a job holds or creates anything.
    pub async fn measure_volume(
        &self,
        volume: &VolumeRequest,
    ) -> Result<VolumeMeasure, DriverError> {
        self.carried_volume(volume).await?;
        let bytes = self
            .volume_helper("volume-measure", volume, None, None)
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| {
            DriverError::Internal("the volume helper returned no valid measurement".into())
        })
    }

    /// The caller has quiesced the source; stdout is streamed to disk, never gathered in memory.
    pub async fn export_volume(
        &self,
        volume: &VolumeRequest,
        into: &Path,
    ) -> Result<(), DriverError> {
        let mut file = tokio::fs::File::create(into)
            .await
            .map_err(|_| DriverError::Internal("no room for the volume archive".into()))?;
        self.export_volume_stream(volume, &mut file).await?;
        file.sync_all()
            .await
            .map_err(|_| DriverError::Internal("the volume archive could not be flushed".into()))?;
        Ok(())
    }

    /// Streams stdout into the node's bounded transfer pipe or a local quiescent staging file.
    pub async fn export_volume_stream(
        &self,
        volume: &VolumeRequest,
        output: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.carried_volume(volume).await?;
        self.volume_helper("volume-export", volume, None, Some(output))
            .await?;
        Ok(())
    }

    /// Imports before any container uses the target, so overlayfs never sees a changed upper under a mounted view.
    pub async fn import_volume(
        &self,
        volume: &VolumeRequest,
        from: &Path,
    ) -> Result<(), DriverError> {
        let mut file = tokio::fs::File::open(from).await.map_err(|_| {
            DriverError::Internal("the downloaded volume archive could not be opened".into())
        })?;
        self.import_volume_stream(volume, &mut file).await
    }

    /// Imports the ordered blob parts without gathering or staging the whole volume.
    pub async fn import_volume_stream(
        &self,
        volume: &VolumeRequest,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.carried_volume(volume).await?;
        let holders = self
            .daemon
            .docker()
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&HashMap::from([(
                        "volume".to_owned(),
                        vec![volume.name.clone()],
                    )]))
                    .build(),
            ))
            .await
            .map_err(map_error)?;
        if !holders.is_empty() {
            return Err(DriverError::Unavailable("the target volume is already used by a container; import must precede unit creation".into()));
        }
        self.volume_helper("volume-import", volume, Some(input), None)
            .await?;
        Ok(())
    }

    /// An explicit directory copy reads the borrowed source through the same metadata-preserving archive.
    pub async fn measure_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<VolumeMeasure, DriverError> {
        self.check_volume_carry().await?;
        let operation =
            if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
                "file-states-measure"
            } else {
                "volume-measure"
            };
        let bytes = self
            .bound_volume_helper(
                operation,
                crate::mounts::bind(&directory.path, "/volume", true),
                Some(directory),
                None,
                None,
            )
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| {
            DriverError::Internal("the existing directory could not be measured".into())
        })
    }

    /// The source is always read-only; import remains restricted to a fresh owned volume.
    pub async fn export_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &Path,
    ) -> Result<(), DriverError> {
        self.check_volume_carry().await?;
        let operation =
            if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
                "file-states-export"
            } else {
                "volume-export"
            };
        let mut output = tokio::fs::File::create(into)
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        self.bound_volume_helper(
            operation,
            crate::mounts::bind(&directory.path, "/volume", true),
            Some(directory),
            None,
            Some(&mut output),
        )
        .await?;
        output
            .sync_all()
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))
    }

    async fn volume_helper(
        &self,
        operation: &str,
        volume: &VolumeRequest,
        from: Option<&mut (dyn tokio::io::AsyncRead + Unpin + Send)>,
        into: Option<&mut (dyn tokio::io::AsyncWrite + Unpin + Send)>,
    ) -> Result<Vec<u8>, DriverError> {
        self.bound_volume_helper(
            operation,
            crate::mounts::volume(&volume.name, "/volume", from.is_none()),
            None,
            from,
            into,
        )
        .await
    }

    async fn bound_volume_helper(
        &self,
        operation: &str,
        source: bollard::models::Mount,
        expected: Option<&orch_driver_contract::runtime::DirectoryBinding>,
        from: Option<&mut (dyn tokio::io::AsyncRead + Unpin + Send)>,
        into: Option<&mut (dyn tokio::io::AsyncWrite + Unpin + Send)>,
    ) -> Result<Vec<u8>, DriverError> {
        let work = WORK.try_with(Arc::clone).ok();
        let command = work.as_ref().and_then(|work| work.command_id());
        let helper = format!(
            "orch-volume-{}",
            command.unwrap_or_else(orch_types::new_id).simple()
        );
        if command.is_some() {
            remove_until_stopped(
                &self.daemon,
                &helper,
                Duration::from_millis(self.config.helper_cleanup_retry_ms),
            )
            .await;
        }
        if work.as_ref().is_some_and(|work| work.is_cancelled()) {
            return Err(DriverError::Unavailable(
                "the filesystem work was cancelled".into(),
            ));
        }
        let binary = self.config.live_helper_binary.as_deref().ok_or_else(|| {
            DriverError::Unsupported("the static volume helper is unavailable".into())
        })?;
        let image = self.config.live_helper_image.clone().ok_or_else(|| {
            DriverError::Unsupported("the trusted helper image is unavailable".into())
        })?;
        let importing = from.is_some();
        if importing && expected.is_some() {
            return Err(DriverError::InvalidSpec(
                "an existing directory is never an import target".into(),
            ));
        }
        let mut command = vec![
            "__runtime-helper".into(),
            operation.into(),
            "--target".into(),
            "/volume".into(),
        ];
        if let Some(expected) = expected {
            let mut expected = expected.clone();
            expected.path = "/volume".into();
            command.extend([
                "--reserved".into(),
                serde_json::to_string(&expected)
                    .map_err(|error| DriverError::Internal(error.to_string()))?,
            ]);
        }
        let mut labels: HashMap<_, _> = driver_labels(&self.config).into_iter().collect();
        labels.insert(LABEL_PREPARE.into(), helper.clone());
        let body = ContainerCreateBody {
            image: Some(image),
            entrypoint: Some(vec![HELPER_BINARY.into()]),
            cmd: Some(command),
            user: Some("0:0".into()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            attach_stdin: Some(importing),
            open_stdin: Some(importing),
            stdin_once: Some(importing),
            labels: Some(labels),
            host_config: Some(HostConfig {
                network_mode: Some("none".into()),
                readonly_rootfs: Some(true),
                // Archive bytes travel only over the attached stream, never into Docker's log files.
                log_config: Some(HostConfigLogConfig {
                    typ: Some("none".into()),
                    config: None,
                }),
                // trusted.* overlay markers require SYS_ADMIN; whiteouts are character 0:0 nodes.
                cap_add: Some(vec![
                    "SYS_ADMIN".into(),
                    "MKNOD".into(),
                    "DAC_READ_SEARCH".into(),
                ]),
                security_opt: Some(vec!["apparmor=unconfined".into()]),
                device_cgroup_rules: Some(vec!["c *:* m".into(), "b *:* m".into()]),
                mounts: Some(vec![
                    crate::mounts::bind(
                        &self.config.host_path(binary).to_string_lossy(),
                        HELPER_BINARY,
                        true,
                    ),
                    source,
                ]),
                ..HostConfig::default()
            }),
            ..ContainerCreateBody::default()
        };
        self.daemon
            .docker()
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&helper)
                        .build(),
                ),
                body,
            )
            .await
            .map_err(map_error)?;
        let mut cleanup_guard = Cleanup {
            daemon: self.daemon.clone(),
            helper: Some(helper.clone()),
            retry: Duration::from_millis(self.config.helper_cleanup_retry_ms),
        };
        let timeout = Duration::from_secs(work.as_ref().map_or_else(
            || {
                if matches!(operation, "volume-measure" | "volume-fingerprint") {
                    3600
                } else {
                    21600
                }
            },
            |work| work.timeout_secs(),
        ));
        let mut destination = into;
        let operation = async {
            let attached = self
                .daemon
                .docker()
                .attach_container(
                    &helper,
                    Some(
                        AttachContainerOptionsBuilder::default()
                            .stdin(importing)
                            .stdout(true)
                            .stderr(true)
                            .stream(true)
                            .logs(false)
                            .build(),
                    ),
                )
                .await
                .map_err(map_error)?;
            let mut input = attached.input;
            let mut output = attached.output;
            self.daemon
                .docker()
                .start_container(&helper, None::<StartContainerOptions>)
                .await
                .map_err(map_error)?;
            let sending = async {
                if let Some(mut from) = from {
                    tokio::io::copy(&mut from, &mut input).await.map_err(|_| {
                        DriverError::Unavailable("the volume archive stream was interrupted".into())
                    })?;
                    input.shutdown().await.map_err(|_| {
                        DriverError::Unavailable(
                            "the volume archive stream could not finish".into(),
                        )
                    })?;
                }
                Ok::<_, DriverError>(())
            };
            let receiving = async {
                let file = &mut destination;
                let mut measured = Vec::new();
                let mut status = Vec::new();
                while let Some(piece) = output.next().await {
                    match piece.map_err(map_error)? {
                        LogOutput::StdOut { message } | LogOutput::Console { message } => {
                            if let Some(file) = file.as_mut() {
                                file.write_all(&message).await.map_err(|_| {
                                    DriverError::Internal(
                                        "the volume archive could not be written".into(),
                                    )
                                })?;
                            } else if measured.len().saturating_add(message.len())
                                <= MEASUREMENT_MAX_BYTES
                            {
                                measured.extend_from_slice(&message);
                            } else {
                                return Err(DriverError::Internal(
                                    "the volume helper's reply exceeds its bound".into(),
                                ));
                            }
                        }
                        LogOutput::StdErr { message } => {
                            if status.len().saturating_add(message.len()) > MEASUREMENT_MAX_BYTES {
                                status.clear();
                            }
                            if message.len() <= MEASUREMENT_MAX_BYTES {
                                status.extend_from_slice(&message);
                            }
                            while let Some(end) = status.iter().position(|byte| *byte == b'\n') {
                                if let Some(line) = status[..end].strip_prefix(b"orch-progress ")
                                    && let Ok(progress) =
                                        serde_json::from_slice::<VolumeProgress>(line)
                                    && let Some(work) = &work
                                {
                                    work.report(progress);
                                }
                                status.drain(..=end);
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(file) = file.as_mut() {
                    file.flush().await.map_err(|_| {
                        DriverError::Internal("the volume archive could not be flushed".into())
                    })?;
                }
                Ok::<_, DriverError>(measured)
            };
            let (_, measured) = futures::try_join!(sending, receiving)?;
            let mut waiting = self
                .daemon
                .docker()
                .wait_container(&helper, None::<WaitContainerOptions>);
            match waiting.next().await {
                Some(Ok(ended)) if ended.status_code == 0 => Ok(measured),
                _ => Err(DriverError::Unavailable(
                    "the whole volume could not be preserved; no carry is accepted".into(),
                )),
            }
        };
        let timed = tokio::time::timeout(timeout, operation);
        let cancelled = async {
            match &work {
                Some(work) => work.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let outcome = tokio::select! {
            biased;
            () = cancelled => Err(DriverError::Unavailable("the filesystem work was cancelled".into())),
            result = timed => result.unwrap_or(Err(DriverError::Timeout)),
        };
        let flushed = if let Some(output) = &mut destination {
            // A cancelled Tokio file write can still own a bounded blocking write. Join it before
            // reporting stop, so no archive grows after the command's handler has returned.
            output
                .flush()
                .await
                .map_err(|error| DriverError::Internal(error.to_string()))
        } else {
            Ok(())
        };
        remove_until_stopped(&self.daemon, &helper, cleanup_guard.retry).await;
        cleanup_guard.helper = None;
        flushed?;
        outcome
    }
}

struct Cleanup {
    daemon: crate::client::Daemon,
    helper: Option<String>,
    retry: Duration,
}

async fn remove_until_stopped(daemon: &crate::client::Daemon, helper: &str, retry: Duration) {
    loop {
        match crate::volumes::remove_helper(daemon, helper).await {
            Ok(()) => return,
            Err(error) => {
                tracing::warn!(%error, "still stopping a volume helper; retrying its removal");
                tokio::time::sleep(retry).await;
            }
        }
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(helper) = self.helper.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let daemon = self.daemon.clone();
            let retry = self.retry;
            runtime.spawn(async move {
                remove_until_stopped(&daemon, &helper, retry).await;
            });
        }
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicI64, Ordering};
    use tokio_util::sync::CancellationToken;

    struct Work {
        cancel: CancellationToken,
        counted: AtomicI64,
        bytes: AtomicI64,
        changed: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl VolumeWork for Work {
        fn report(&self, progress: VolumeProgress) {
            self.counted.store(progress.files_done, Ordering::SeqCst);
            self.bytes.store(progress.bytes_done, Ordering::SeqCst);
            self.changed.notify_one();
        }
        fn is_cancelled(&self) -> bool {
            self.cancel.is_cancelled()
        }
        async fn cancelled(&self) {
            self.cancel.cancelled().await;
        }
        fn timeout_secs(&self) -> u64 {
            30
        }
    }

    async fn driver(binary: std::path::PathBuf) -> DockerDriver {
        DockerDriver::connect(crate::config::DockerConfig {
            labels: BTreeMap::from([("orch.test".into(), orch_types::new_id().to_string())]),
            live_helper_binary: Some(binary),
            live_helper_image: Some(
                std::env::var("ORCH_TEST_VOLUME_HELPER_IMAGE")
                    .unwrap_or_else(|_| "orch-dev-harness".into()),
            ),
            ..Default::default()
        })
        .await
        .unwrap()
    }
    async fn helpers(driver: &DockerDriver) -> Vec<String> {
        let labels = driver
            .config
            .labels
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>();
        driver
            .daemon
            .docker()
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&HashMap::from([("label".to_owned(), labels)]))
                    .build(),
            ))
            .await
            .unwrap()
            .into_iter()
            .filter(|container| {
                container.labels.as_ref().is_some_and(|labels| {
                    labels
                        .get(LABEL_PREPARE)
                        .is_some_and(|name| name.starts_with("orch-volume-"))
                })
            })
            .filter_map(|container| container.id)
            .collect()
    }

    #[tokio::test]
    async fn cancelling_a_content_check_stops_reading_and_removes_its_actual_helper() {
        let Some(binary) = std::env::var_os("ORCH_TEST_NODE_BINARY") else {
            eprintln!("not run: the native node binary must be supplied");
            return;
        };
        let driver = driver(binary.into()).await;
        let root = tempfile::tempdir().unwrap();
        // A sparse file allocates no data blocks and keeps hashing active beyond its one-second report cadence.
        const HASH_BYTES: u64 = 8 * 1024 * 1024 * 1024;
        std::fs::File::create(root.path().join("large-file"))
            .unwrap()
            .set_len(HASH_BYTES)
            .unwrap();
        let work = Arc::new(Work {
            cancel: CancellationToken::new(),
            counted: AtomicI64::new(0),
            bytes: AtomicI64::new(0),
            changed: tokio::sync::Notify::new(),
        });
        let scope: Arc<dyn VolumeWork> = work.clone();
        let checking = with_work(
            scope,
            driver.bound_volume_helper(
                "volume-fingerprint",
                crate::mounts::bind(&root.path().to_string_lossy(), "/volume", true),
                None,
                None,
                None,
            ),
        );
        tokio::pin!(checking);
        let reported = async {
            loop {
                let changed = work.changed.notified();
                if work.bytes.load(Ordering::SeqCst) > 0 {
                    break;
                }
                changed.await;
            }
        };
        tokio::select! {
            () = reported => {},
            result = &mut checking => panic!("the large check ended before a partial byte report: {result:?}"),
            () = tokio::time::sleep(Duration::from_secs(20)) => panic!("the content check emitted no byte progress"),
        }
        assert!(work.bytes.load(Ordering::SeqCst) < i64::try_from(HASH_BYTES).unwrap());
        assert_eq!(helpers(&driver).await.len(), 1);
        work.cancel.cancel();
        assert!(checking.await.is_err());
        assert!(
            helpers(&driver).await.is_empty(),
            "a stopped content check left its I/O process alive"
        );
    }

    #[tokio::test]
    async fn cancelling_a_streaming_count_joins_and_removes_its_actual_helper_process() {
        let (Some(binary), Some(tree)) = (
            std::env::var_os("ORCH_TEST_NODE_BINARY"),
            std::env::var_os("ORCH_MEASUREMENT_FIXTURE"),
        ) else {
            eprintln!(
                "not run: the native node binary and large measurement fixture must be supplied"
            );
            return;
        };
        let driver = driver(binary.into()).await;
        let work = Arc::new(Work {
            cancel: CancellationToken::new(),
            counted: AtomicI64::new(0),
            bytes: AtomicI64::new(0),
            changed: tokio::sync::Notify::new(),
        });
        let scope: Arc<dyn VolumeWork> = work.clone();
        // The native helper runs in the development image for this proof. Release static-binary
        // admission has separate coverage; the actual helper, stream and awaited cleanup are identical.
        let measuring = with_work(
            scope,
            driver.bound_volume_helper(
                "volume-measure",
                crate::mounts::bind(
                    &std::path::PathBuf::from(tree).to_string_lossy(),
                    "/volume",
                    true,
                ),
                None,
                None,
                None,
            ),
        );
        tokio::pin!(measuring);
        tokio::select! {
            _ = work.changed.notified() => {},
            result = &mut measuring => panic!("the large count ended before a live report: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(20)) => panic!("the count emitted no progress"),
        }
        assert!(work.counted.load(Ordering::SeqCst) > 0);
        assert_eq!(helpers(&driver).await.len(), 1);
        work.cancel.cancel();
        assert!(measuring.await.is_err());
        assert!(
            helpers(&driver).await.is_empty(),
            "a cancelled count left its process container alive"
        );
    }

    #[tokio::test]
    async fn cancelling_a_large_archive_stops_output_growth_and_removes_its_actual_helper_process()
    {
        let Some(binary) = std::env::var_os("ORCH_TEST_NODE_BINARY") else {
            eprintln!("not run: the native node binary must be supplied");
            return;
        };
        let driver = driver(binary.into()).await;
        let root = tempfile::tempdir().unwrap();
        // Sparse input costs no data allocation; it remains much larger than a sampling interval's copy.
        let source = std::fs::File::create(root.path().join("large-file")).unwrap();
        source.set_len(8 * 1024 * 1024 * 1024).unwrap();
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("archive");
        let mut output = tokio::fs::File::create(&output_path).await.unwrap();
        let work = Arc::new(Work {
            cancel: CancellationToken::new(),
            counted: AtomicI64::new(0),
            bytes: AtomicI64::new(0),
            changed: tokio::sync::Notify::new(),
        });
        let scope: Arc<dyn VolumeWork> = work.clone();
        let copying = with_work(
            scope,
            driver.bound_volume_helper(
                "volume-export",
                crate::mounts::bind(&root.path().to_string_lossy(), "/volume", true),
                None,
                None,
                Some(&mut output),
            ),
        );
        tokio::pin!(copying);
        let until = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            tokio::select! {
                result=&mut copying=>panic!("the large archive ended before cancellation: {result:?}"),
                _=tokio::time::sleep(Duration::from_millis(10))=>{
                    if tokio::fs::metadata(&output_path).await.unwrap().len()>1024*1024 {break;}
                    assert!(tokio::time::Instant::now()<until,"the archive made no progress");
                }
            }
        }
        assert_eq!(helpers(&driver).await.len(), 1);
        work.cancel.cancel();
        assert!(copying.await.is_err());
        assert!(
            helpers(&driver).await.is_empty(),
            "a cancelled archive left its process container alive"
        );
        let stopped = tokio::fs::metadata(&output_path).await.unwrap().len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            tokio::fs::metadata(&output_path).await.unwrap().len(),
            stopped,
            "output kept growing after cancellation completed"
        );
    }
}
