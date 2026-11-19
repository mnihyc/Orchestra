//! The Docker driver as the node uses it: the settings it is given, and what the node adds to it.
//!
//! The Docker driver implements the driver contract and has a few operations beyond it that a node needs: the
//! storages of the host with their sizes, the progress of a pull or a build, the digest a registry has behind a
//! tag, holding a unit's processes still. [`DockerExtras`] gives them to the node through its own seam. It also
//! answers the one question the contract has no word for, whether the connector inside a unit has enrolled, by
//! looking for the record the connector keeps of its enrollment on the unit's state volume. And it is where
//! the blobs a build or an archive names are fetched from the system, for the driver that asks for them by hash.

use std::sync::Arc;

use async_trait::async_trait;
use bollard::Docker;
use bollard::query_parameters::DownloadFromContainerOptionsBuilder;
use futures::{StreamExt, TryStreamExt};
use orch_blob_contract::store::{BlobError, ByteStream};
use orch_connector_core::enrollment::RECORD_FILE;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::isolation::Boundary;
use orch_driver_contract::sources::ImageEnsured;
use orch_driver_contract::units::UnitRef;
use orch_driver_contract::volumes::StorageInfo;
use orch_driver_docker::config::{
    DockerConfig, EgressConfig, NetworkNames, PathMap, RegistryAuth, RuntimeSettings,
    StorageConfig, ToolCache,
};
use orch_driver_docker::progress::{Phase, ProgressSink};
use orch_driver_docker::spec::layout::STATE_DIR;
use orch_driver_docker::{BlobSource, DockerDriver};
use orch_node_core::drivers::{DriverExtras, Factory, HostDriver};
use orch_node_core::images::{BlobDesk, ImageProgress, ImageStep};
use orch_node_core::settings::NodeSettings;
use orch_types::{Hash, Json};
use tokio_util::io::ReaderStream;

/// The name the driver goes by in the configuration, in a template and in a capability's patch.
pub const NAME: &str = "docker";

/// The directory of the connector's identity, below its data directory.
const IDENTITY_DIR: &str = "identity";

/// How long a request to the daemon for the record of a unit's enrollment may take: it is a small file.
const PROBE_SECS: u64 = 10;

/// The driver's settings from the operator's `[node.docker]` and `[[node.storage]]`, and from where the node
/// keeps its tool cache. Everything else is the driver's own default.
pub fn config_of(settings: &NodeSettings) -> Result<DockerConfig, DriverError> {
    let docker = &settings.docker;
    let mut labels = docker.labels.clone();
    labels.insert(
        orch_driver_contract::runtime::LABEL_SYSTEM.into(),
        settings.system_name.clone(),
    );
    Ok(DockerConfig {
        socket: docker.socket.clone(),
        gateway_host: docker.gateway_host.clone(),
        live_helper_image: docker.live_helper_image.clone(),
        conformance_image: docker.conformance_image.clone(),
        live_helper_binary: docker
            .live_helper_binary
            .clone()
            .or_else(orch_connector_core::installation::program_path),
        builds: docker.builds,
        build_args: docker.build_args.clone(),
        offline_images: docker.offline_images.clone(),
        build_timeout_secs: docker.build_timeout_secs,
        build_parallel: docker.build_parallel,
        build_cache_gb: docker.build_cache_gb,
        registries: registry_credentials(settings)?,
        path_map: docker
            .path_map
            .iter()
            .map(|map| PathMap {
                node: map.node.clone().into(),
                host: map.host.clone().into(),
            })
            .collect(),
        check_host_paths: docker.check_host_paths,
        image_repository: docker.image_repository.clone(),
        labels,
        storages: settings
            .storages
            .iter()
            .map(|storage| StorageConfig {
                name: storage.name.clone(),
                path: storage.path.clone(),
                default: storage.default,
            })
            .collect(),
        tool_cache: ToolCache {
            dir: Some(settings.tools_dir()),
            connector_dir: Some(settings.connector_dir()),
        },
        // Each restricted unit's proxy keeps its orders, heartbeat and state below the node's data directory,
        // which the daemon sees at the same path.
        egress: EgressConfig {
            dir: Some(settings.data_dir.join(EGRESS_DIR)),
            proxy_image: docker.restricted_proxy_image.clone(),
        },
        // Every unit's own network, and a restricted unit's proxy's way out, are cut from the operator's pool.
        networks: NetworkNames {
            pool: docker.networks.pool.clone(),
            prefix: docker.networks.prefix,
            ..NetworkNames::default()
        },
        docker_inside_gb: docker.docker_inside_gb,
        resolvers: docker.resolvers.clone(),
        runtimes: docker
            .runtimes
            .iter()
            .map(|(name, declared)| {
                let boundary = Boundary::parse(&declared.boundary).ok_or_else(|| {
                    DriverError::InvalidSpec(format!(
                        "runtime {name}: boundary {:?} is none, shared_kernel, user_space_kernel or own_kernel",
                        declared.boundary
                    ))
                })?;
                Ok((
                    name.clone(),
                    RuntimeSettings {
                        boundary,
                        title: (!declared.title.is_empty()).then(|| declared.title.clone()),
                        keeps_software_on_stop: declared.keeps_software_on_stop,
                        memory_overhead_mb: declared.memory_overhead_mb,
                        enrol_timeout_secs: declared.enrol_timeout_secs,
                    },
                ))
            })
            .collect::<Result<_, DriverError>>()?,
        ..DockerConfig::default()
    })
}

// Secrets have no Debug implementation and malformed input never reaches an error message.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryCredentials {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    identity_token: Option<String>,
}

/// Registry credential files are small JSON objects, bounded to the existing log-tail budget.
const REGISTRY_CREDENTIAL_BYTES: u64 = orch_driver_docker::progress::LOG_TAIL_BYTES as u64;

fn registry_credentials(settings: &NodeSettings) -> Result<Vec<RegistryAuth>, DriverError> {
    use std::io::Read;
    let mut servers = std::collections::BTreeSet::new();
    settings.docker.registries.iter().map(|registry| {
        let authority = registry.server.as_str();
        if authority.is_empty() || authority.contains(['/', '@', '?', '#'])
            || authority.chars().any(char::is_whitespace) || !servers.insert(authority) {
            return Err(DriverError::InvalidSpec("each registry needs a distinct host name and optional port".into()));
        }
        let mut path = std::path::PathBuf::from(&registry.credentials_file);
        if path.as_os_str().is_empty() {
            return Err(DriverError::InvalidSpec("a registry needs a protected credentials_file".into()));
        }
        if path.is_relative() {
            let base = settings.config_file.as_deref().and_then(std::path::Path::parent)
                .ok_or_else(|| DriverError::InvalidSpec("a relative registry credentials_file needs a configuration-file directory".into()))?;
            path = base.join(path);
        }
        let unavailable = || DriverError::Unavailable(format!("registry {authority}: its protected credentials file cannot be read"));
        let mut file = std::fs::File::open(path).map_err(|_| unavailable())?;
        let metadata = file.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file() || metadata.len() > REGISTRY_CREDENTIAL_BYTES {
            return Err(DriverError::InvalidSpec(format!("registry {authority}: credentials must be a small regular file")));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(DriverError::InvalidSpec(format!("registry {authority}: credentials must be readable only by their owner (0400 or 0600)")));
            }
        }
        let mut bytes = Vec::new();
        (&mut file).take(REGISTRY_CREDENTIAL_BYTES + 1).read_to_end(&mut bytes).map_err(|_| unavailable())?;
        if bytes.len() as u64 > REGISTRY_CREDENTIAL_BYTES {
            return Err(DriverError::InvalidSpec(format!("registry {authority}: credentials file is too large")));
        }
        let credentials: RegistryCredentials = serde_json::from_slice(&bytes).map_err(|_| {
            DriverError::InvalidSpec(format!("registry {authority}: credentials must contain username/password or identity_token"))
        })?;
        let token = credentials.identity_token.as_ref().is_some_and(|value| !value.is_empty());
        let password = credentials.password.as_ref().is_some_and(|value| !value.is_empty());
        let username = credentials.username.as_ref().is_some_and(|value| !value.is_empty());
        if !(token || (username && password)) {
            return Err(DriverError::InvalidSpec(format!("registry {authority}: credentials are incomplete")));
        }
        Ok(RegistryAuth { server: registry.server.clone(), username: credentials.username,
            password: credentials.password, identity_token: credentials.identity_token })
    }).collect()
}

/// The directory of the node's data directory that holds restricted units' proxies' files.
const EGRESS_DIR: &str = "egress";

/// Connects to the daemon `config` names and makes the node's driver of it, which fetches blobs through
/// `desk`.
pub async fn connect(config: DockerConfig, desk: Arc<BlobDesk>) -> Result<HostDriver, DriverError> {
    let socket = config.socket.clone();
    let request_timeout = config.request_timeout_secs;
    let driver = DockerDriver::connect(config)
        .await?
        .with_blob_source(Arc::new(DeskSource(desk)));
    let docker =
        Docker::connect_with_socket(&socket, request_timeout, bollard::API_DEFAULT_VERSION)
            .map_err(|error| DriverError::Unavailable(error.to_string()))?;
    let driver = Arc::new(driver);
    let contract: Arc<dyn EnvironmentDriver> = driver.clone();
    Ok(HostDriver::new(contract).with_extras(Arc::new(DockerExtras { driver, docker })))
}

/// The registration of the driver for a program: it reads the settings `adjust` is given the chance to change
/// (a test gives its names to what the driver makes on a daemon it shares).
#[must_use]
pub fn registration(adjust: impl Fn(&mut DockerConfig) + Send + Sync + 'static) -> Factory {
    let adjust = Arc::new(adjust);
    Arc::new(move |context| {
        let adjust = Arc::clone(&adjust);
        Box::pin(async move {
            let mut settings = context.settings.clone();
            if let Some(value) = &context.configuration {
                settings.docker = serde_json::from_value(value.clone())
                    .map_err(|error| DriverError::InvalidSpec(error.to_string()))?;
            }
            // Permission to build is checked live; the driver reports its actual implementation.
            settings.docker.builds = true;
            let mut config = config_of(&settings)?;
            adjust(&mut config);
            let schema = crate::composition::configuration_schema(
                &orch_node_core::machine_policy::docker_options(
                    &orch_config::server::NodeDocker::default(),
                ),
            );
            let checked_schema = schema.clone();
            Ok(connect(config, context.blobs).await?.with_configuration(
                schema,
                Arc::new(move |value| {
                    orch_plugin::schema::validate(&checked_schema, value)
                        .map_err(|error| DriverError::InvalidSpec(error.to_string()))?;
                    serde_json::from_value::<orch_config::server::NodeDocker>(value.clone())
                        .map_err(|error| DriverError::InvalidSpec(error.to_string()))?;
                    Ok(())
                }),
            ))
        })
    })
}

/// What the Docker driver has beyond the contract, as the node asks for it.
pub struct DockerExtras {
    driver: Arc<DockerDriver>,
    docker: Docker,
}

/// Tells the node's progress what the driver tells.
struct Reporting<'a>(&'a dyn ImageProgress);

impl ProgressSink for Reporting<'_> {
    fn line(&self, phase: Phase, text: &str) {
        self.0.line(
            match phase {
                Phase::Pull => ImageStep::Pull,
                Phase::Build => ImageStep::Build,
                Phase::Load => ImageStep::Load,
            },
            text,
        );
    }
}

#[async_trait]
impl DriverExtras for DockerExtras {
    async fn stop_volume_work(&self, command: orch_types::Id) -> Result<(), DriverError> {
        self.driver.stop_volume_work(command).await
    }
    /// Counts an owned volume with cooperative cancellation and live reports.
    async fn measure_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        orch_driver_docker::volume_moves::with_work(
            std::sync::Arc::clone(work),
            self.measure_volume(volume),
        )
        .await
    }
    /// Counts the selected owned subtree with cooperative cancellation and live reports.
    async fn measure_selected_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        orch_driver_docker::volume_moves::with_work(
            std::sync::Arc::clone(work),
            self.measure_selected_volume(volume, selector),
        )
        .await
    }
    /// Counts a checked borrowed directory with cooperative cancellation and live reports.
    async fn measure_directory_reporting(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        orch_driver_docker::volume_moves::with_work(
            std::sync::Arc::clone(work),
            self.measure_directory(directory, selector),
        )
        .await
    }
    /// Exports a checked directory with cooperative cancellation and live reports.
    async fn export_directory_reporting(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        orch_driver_docker::volume_moves::with_work(
            std::sync::Arc::clone(work),
            self.export_directory(directory, selector, into),
        )
        .await
    }
    /// Exports an owned volume with cooperative cancellation and live reports.
    async fn export_selected_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        orch_driver_docker::volume_moves::with_work(
            std::sync::Arc::clone(work),
            self.export_selected_volume(volume, selector, into),
        )
        .await
    }
    /// Imports an unused owned volume with cooperative cancellation and live reports.
    async fn import_selected_volume_stream_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
        work: &std::sync::Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        orch_driver_docker::volume_moves::with_work(
            std::sync::Arc::clone(work),
            self.import_selected_volume_stream(volume, selector, input),
        )
        .await
    }

    async fn harness_conformance(
        &self,
        request: &orch_link_contract::commands::NodeToolConformance,
        program: &std::path::Path,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<orch_link_contract::commands::NodeToolConformanceResult, DriverError> {
        crate::conformance_unit::run(&self.docker, self.driver.config(), request, program, cancel)
            .await
    }
    async fn volume_exists(
        &self,
        request: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<bool, orch_driver_contract::driver::DriverError> {
        self.driver.volume_exists(request).await
    }
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        Some(self.driver.storages())
    }

    async fn ensure_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.driver
            .ensure_image_reporting(spec, &Reporting(progress))
            .await
    }

    async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.driver.rebuild_image(spec, &Reporting(progress)).await
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

    async fn export_state(
        &self,
        unit: &UnitRef,
        path: &str,
        into: &std::path::Path,
    ) -> Result<bool, DriverError> {
        self.driver.export_state(unit, path, into).await
    }

    async fn measure_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.driver.measure_volume(volume).await
    }
    async fn fingerprint_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
    ) -> Result<orch_types::Hash, DriverError> {
        self.driver.fingerprint_volume(volume).await
    }
    async fn fingerprint_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
    ) -> Result<orch_types::Hash, DriverError> {
        self.driver.fingerprint_directory(directory).await
    }
    async fn fingerprint_volume_reporting(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_types::Hash, DriverError> {
        orch_driver_docker::volume_moves::with_work(
            Arc::clone(work),
            self.driver.fingerprint_volume(volume),
        )
        .await
    }
    async fn fingerprint_directory_reporting(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_types::Hash, DriverError> {
        orch_driver_docker::volume_moves::with_work(
            Arc::clone(work),
            self.driver.fingerprint_directory(directory),
        )
        .await
    }
    async fn measure_selected_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
            self.driver.measure_file_states(volume).await
        } else {
            self.driver.measure_volume(volume).await
        }
    }
    async fn measure_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.driver.measure_directory(directory, selector).await
    }
    async fn export_directory(
        &self,
        directory: &orch_driver_contract::runtime::DirectoryBinding,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.driver
            .export_directory(directory, selector, into)
            .await
    }
    async fn export_selected_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
            self.driver.export_file_states(volume, into).await
        } else {
            self.driver.export_volume(volume, into).await
        }
    }
    async fn import_selected_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
            self.driver.import_file_states_stream(volume, input).await
        } else {
            self.driver.import_volume_stream(volume, input).await
        }
    }

    async fn check_volume_carry(&self) -> Result<(), DriverError> {
        self.driver.check_volume_carry().await
    }

    async fn export_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        output: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.driver.export_volume_stream(volume, output).await
    }

    async fn import_volume_stream(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        input: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.driver.import_volume_stream(volume, input).await
    }

    async fn export_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.driver.export_volume(volume, into).await
    }

    async fn import_volume(
        &self,
        volume: &orch_driver_contract::volumes::VolumeRequest,
        from: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.driver.import_volume(volume, from).await
    }

    async fn import_state(
        &self,
        unit: &UnitRef,
        from: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.driver.import_state(unit, from).await
    }

    async fn enrolled(&self, unit: &UnitRef) -> Option<bool> {
        let record = format!("{STATE_DIR}/{IDENTITY_DIR}/{RECORD_FILE}");
        let options = DownloadFromContainerOptionsBuilder::default()
            .path(&record)
            .build();
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(PROBE_SECS),
            self.docker
                .download_from_container(&unit.reference, Some(options))
                .try_collect::<Vec<_>>(),
        )
        .await
        .ok()?;
        match reply {
            Ok(pieces) => Some(pieces.iter().any(|piece| !piece.is_empty())),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Some(false),
            Err(_) => None,
        }
    }
}

/// Fetches the blobs the commands name, for the driver, through the node's desk.
struct DeskSource(Arc<BlobDesk>);

#[async_trait]
impl BlobSource for DeskSource {
    async fn open(&self, hash: &Hash) -> Result<ByteStream, BlobError> {
        let path = self
            .0
            .fetch(hash)
            .await
            .map_err(|error| BlobError::Io(error.to_string()))?;
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|error| BlobError::Io(format!("{}: {error}", path.display())))?;
        Ok(ReaderStream::new(file)
            .map(|piece| piece.map_err(|error| BlobError::Io(error.to_string())))
            .boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_operators_pool_is_what_every_units_network_is_cut_from() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = NodeSettings::in_dir(dir.path());
        settings.docker.networks.pool = "10.77.0.0/20".into();
        settings.docker.networks.prefix = 27;
        let config = config_of(&settings).unwrap();
        assert_eq!(config.networks.pool().unwrap().to_string(), "10.77.0.0/20");
        assert_eq!(config.networks.prefix, 27);
        assert_eq!(
            config.networks.env, "orch-env",
            "the old common bridges keep their names, to be moved off"
        );
        assert_eq!(
            config_of(&NodeSettings::in_dir(dir.path()))
                .unwrap()
                .networks
                .pool,
            "10.199.0.0/16"
        );
    }

    #[test]
    fn a_private_registry_reads_its_login_from_a_protected_file_and_does_not_print_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        std::fs::write(
            &path,
            r#"{"username":"tester","password":"registry-test-password"}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut settings = NodeSettings::in_dir(dir.path());
        settings.config_file = Some(dir.path().join("orch.toml"));
        settings
            .docker
            .registries
            .push(orch_config::server::NodeDockerRegistry {
                server: "127.0.0.1:5000".into(),
                credentials_file: "registry.json".into(),
            });
        let config = config_of(&settings).unwrap();
        assert_eq!(
            config.registries[0].password.as_deref(),
            Some("registry-test-password")
        );
        assert!(!format!("{config:?} {settings:?}").contains("registry-test-password"));
        assert_eq!(
            config.redact_registry_text("denied registry-test-password"),
            "denied <hidden>"
        );
        std::fs::write(
            &path,
            r#"{"password":"registry-test-password","unknown":"registry-test-password"}"#,
        )
        .unwrap();
        let error = config_of(&settings).unwrap_err();
        assert!(!error.to_string().contains("registry-test-password"));
        assert!(!format!("{error:?}").contains("registry-test-password"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_or_public_registry_credentials_refuse_loading_the_driver() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let mut settings = NodeSettings::in_dir(dir.path());
        settings
            .docker
            .registries
            .push(orch_config::server::NodeDockerRegistry {
                server: "registry.test".into(),
                credentials_file: path.to_string_lossy().into_owned(),
            });
        assert!(matches!(
            config_of(&settings),
            Err(DriverError::Unavailable(_))
        ));
        std::fs::write(
            &path,
            r#"{"username":"tester","password":"registry-test-password"}"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            config_of(&settings),
            Err(DriverError::InvalidSpec(_))
        ));
    }
}
