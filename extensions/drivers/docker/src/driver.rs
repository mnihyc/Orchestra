//! The Docker driver itself: its construction and the operations of the driver contract.
//!
//! [`DockerDriver`] is what `orch-node` loads when its configuration names `docker`. It connects to the daemon once,
//! learns what that daemon can do, and from then on answers the contract's operations: it describes itself, makes,
//! starts, stops and removes units, reports their state and use, lists the units of a system, keeps volumes on the
//! host's storages, makes the source of a spec available as an image, and proposes specs from what a person has.
//! One driver is shared by every command the node runs, and commands for one unit never interleave.
//!
//! **What the node gives it.** The connector's directory and the tool cache (`DockerConfig::tool_cache`), the gateway
//! host (`DockerConfig::gateway_host`), and the enrollment inputs in each request: the enrollment token, the
//! authority and the gateway address. A source of blobs ([`DockerDriver::with_blob_source`]) is needed only for
//! templates that name a context archive or an image archive.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::imports::{ImportProposal, ImportRequest};
use orch_driver_contract::network::{NetworkPolicy, UnitNetwork};
use orch_driver_contract::runtime::{RuntimeKind, RuntimeSpec};
use orch_driver_contract::sources::ImageEnsured;
use orch_driver_contract::units::{ResourceSample, UnitRef, UnitState};
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use orch_types::{Clock, Json, SystemClock};

use crate::archive::BlobSource;
use crate::client::{Daemon, DaemonFacts, map_error};
use crate::config::DockerConfig;
use crate::descriptor::{IMPORT_ARCHIVE, IMPORT_DOCKERFILE, IMPORT_IMAGE, descriptor};
use crate::progress::{ProgressSink, Silent};
use crate::spec::DockerSpec;
use crate::stats::Memo;
use crate::units::KeyedLocks;
use crate::volumes;

/// The Docker driver.
#[derive(Clone)]
pub struct DockerDriver {
    pub(crate) daemon: Daemon,
    pub(crate) config: Arc<DockerConfig>,
    pub(crate) blobs: Option<Arc<dyn BlobSource>>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) locks: Arc<KeyedLocks>,
    pub(crate) builds: Arc<tokio::sync::Semaphore>,
    pub(crate) memos: Arc<Mutex<HashMap<String, Memo>>>,
    pub(crate) heartbeat: crate::restricted::Heartbeat,
    pub(crate) control_endpoints: Arc<Mutex<Vec<String>>>,
    pub(crate) live_helpers_kernel: bool,
    pub(crate) live_mounts: Arc<Mutex<HashMap<String, crate::live_mounts::Applied>>>,
    /// What trying found of restricted units under each runtime that is not a plain container.
    pub(crate) restricted_proved: Arc<crate::runtimes::RestrictedProved>,
}

impl std::fmt::Debug for DockerDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerDriver")
            .field("socket", &self.config.socket)
            .field("daemon", &self.daemon.facts().server_version)
            .finish_non_exhaustive()
    }
}

impl DockerDriver {
    /// Verifies the identity, ownership and storage of a kept volume without creating anything.
    pub async fn volume_exists(&self, request: &VolumeRequest) -> Result<bool, DriverError> {
        crate::volumes::exists(&self.daemon, &self.config, request).await
    }
    /// Connects to the daemon the configuration names and learns what it is.
    pub async fn connect(config: DockerConfig) -> Result<Self, DriverError> {
        Self::connect_mode(config, true).await
    }

    /// Connects for a local inventory without capability probes, helper creation or heartbeat writes.
    pub async fn connect_for_inventory(config: DockerConfig) -> Result<Self, DriverError> {
        Self::connect_mode(config, false).await
    }

    async fn connect_mode(config: DockerConfig, probes: bool) -> Result<Self, DriverError> {
        if let Err(faults) = config.check() {
            return Err(DriverError::Internal(format!(
                "the settings of the docker driver are wrong: {}",
                faults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        let daemon = Daemon::connect(&config).await?;
        // Whether this daemon makes a restricted unit's network as it must be is found by trying, once.
        let restricted = probes && crate::restricted::probe(&daemon, &config).await;
        let daemon = daemon.with_restricted_egress(restricted);
        let live_helpers_kernel = probes && crate::live_mounts::kernel_supported(&daemon).await;
        // A runtime whose units have a kernel of their own is offered restricted units only where one was tried.
        let mut restricted_proved = crate::runtimes::RestrictedProved::new();
        if restricted {
            for name in &daemon.facts().runtimes {
                if crate::runtimes::boundary_of(&config, name).has_kernel_of_its_own() {
                    restricted_proved.insert(
                        name.clone(),
                        crate::restricted::probe_runtime(&daemon, &config, name).await,
                    );
                }
            }
        }
        let permits = usize::try_from(config.build_parallel.max(1)).unwrap_or(1);
        // The proxies' heartbeat follows the machine's own clock, which their program reads too.
        let heartbeat = crate::restricted::Heartbeat::default();
        if let Some(dir) = config.egress.dir.clone().filter(|_| probes) {
            heartbeat.spawn(dir, Arc::new(SystemClock));
        }
        Ok(Self {
            daemon,
            config: Arc::new(config),
            blobs: None,
            clock: Arc::new(SystemClock),
            locks: Arc::new(KeyedLocks::default()),
            builds: Arc::new(tokio::sync::Semaphore::new(permits)),
            memos: Arc::new(Mutex::new(HashMap::new())),
            heartbeat,
            control_endpoints: Arc::new(Mutex::new(Vec::new())),
            live_helpers_kernel,
            live_mounts: Arc::new(Mutex::new(HashMap::new())),
            restricted_proved: Arc::new(restricted_proved),
        })
    }

    /// The heartbeat of this host's restricted units' proxies, which the node stops while its link to the system is
    /// down: a proxy without it closes its unit's network.
    #[must_use]
    pub fn heartbeat(&self) -> &crate::restricted::Heartbeat {
        &self.heartbeat
    }

    /// Gives the driver the means to fetch a blob by its hash: the context archive of a build, an image archive.
    #[must_use]
    pub fn with_blob_source(mut self, source: Arc<dyn BlobSource>) -> Self {
        self.blobs = Some(source);
        self
    }

    /// Gives the driver another clock than the machine's.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The settings the driver runs with.
    #[must_use]
    pub fn config(&self) -> &DockerConfig {
        &self.config
    }

    /// What the driver learned of the daemon.
    #[must_use]
    pub fn facts(&self) -> &DaemonFacts {
        self.daemon.facts()
    }

    /// The named places for volumes on this host, with their sizes and whether private views can be made on
    /// them: what the node reports of the host's storages.
    #[must_use]
    pub fn storages(&self) -> Vec<StorageInfo> {
        volumes::storages(&self.config, self.daemon.facts())
    }

    /// Holds a unit's processes still, so that its files do not change while the environment is moved.
    pub async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let container = self.container_of(unit).await?.ok_or_else(|| {
            DriverError::NotFound(format!("there is no unit {} of this driver", unit.name))
        })?;
        self.daemon
            .docker()
            .pause_container(container.id.as_deref().unwrap_or_default())
            .await
            .map_err(map_error)
    }

    /// Lets go of the processes of a unit that [`DockerDriver::suspend`] held.
    pub async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let container = self.container_of(unit).await?.ok_or_else(|| {
            DriverError::NotFound(format!("there is no unit {} of this driver", unit.name))
        })?;
        self.daemon
            .docker()
            .unpause_container(container.id.as_deref().unwrap_or_default())
            .await
            .map_err(map_error)
    }

    /// Makes the source of a spec available like `EnvironmentDriver::ensure_image`, and reports the progress of
    /// what is long (a pull, a build, a load) to `progress` as it goes.
    pub async fn ensure_image_reporting(
        &self,
        spec: &Json,
        progress: &dyn ProgressSink,
    ) -> Result<ImageEnsured, DriverError> {
        self.ensure_source(spec, progress).await
    }

    /// The digest the registry has now for a reference, without pulling the image: how a host notices that a tag
    /// has moved.
    pub async fn registry_digest(&self, reference: &str) -> Result<Option<String>, DriverError> {
        if reference.starts_with("orch-offline:") {
            return Ok(self
                .daemon
                .image(reference)
                .await?
                .and_then(|image| image.id));
        }
        if let Some(content) = self.config.offline_images.get(reference) {
            return Ok(self.daemon.image(content).await?.and_then(|image| image.id));
        }
        let name = crate::images::parse_reference(reference);
        let credentials = self
            .config
            .registries
            .iter()
            .find(|auth| auth.server == crate::images::registry_of(&name.name))
            .map(|auth| bollard::auth::DockerCredentials {
                username: auth.username.clone(),
                password: auth.password.clone(),
                identitytoken: auth.identity_token.clone(),
                serveraddress: Some(auth.server.clone()),
                ..bollard::auth::DockerCredentials::default()
            });
        match self
            .daemon
            .docker()
            .inspect_registry_image(reference, credentials)
            .await
        {
            Ok(found) => Ok(found.descriptor.digest),
            Err(error) if crate::client::status_of(&error) == Some(404) => Ok(None),
            Err(error) => Err(map_error(error)),
        }
    }

    /// Removes one selected image name without pruning parents; a unit still using it prevents removal.
    pub async fn remove_image(&self, tag: &str) -> Result<crate::images::Removal, DriverError> {
        let repository = format!("{}:", self.config.image_repository);
        if !tag.starts_with(&repository) {
            return Err(DriverError::InvalidSpec(format!(
                "{tag} is not a name the driver keeps images under"
            )));
        }
        self.remove_image_tag(tag).await
    }
}

#[async_trait]
impl EnvironmentDriver for DockerDriver {
    async fn resources(
        &self,
        system: &str,
    ) -> Result<Vec<orch_driver_contract::resources::Resource>, DriverError> {
        self.inventory(system).await
    }
    async fn remove_resource(
        &self,
        system: &str,
        resource: &orch_driver_contract::resources::ResourceRef,
    ) -> Result<(), DriverError> {
        self.remove_inventoried(system, resource).await
    }

    async fn stop_resource(
        &self,
        system: &str,
        resource: &orch_driver_contract::resources::ResourceRef,
        grace: Duration,
    ) -> Result<(), DriverError> {
        self.stop_inventoried(system, resource, grace).await
    }

    async fn estimate_keep(
        &self,
        unit: &UnitRef,
    ) -> Result<orch_driver_contract::kept::KeepEstimate, DriverError> {
        self.estimate_kept_files(unit).await
    }
    async fn keep_files(
        &self,
        unit: &UnitRef,
        environment: orch_types::Id,
        generation: i64,
        capture: orch_types::Id,
    ) -> Result<orch_driver_contract::kept::KeptImage, DriverError> {
        self.record_kept_files(unit, environment, generation, capture)
            .await
    }
    async fn kept_images(
        &self,
        system: &str,
    ) -> Result<Vec<orch_driver_contract::kept::KeptImage>, DriverError> {
        self.list_kept_images(system).await
    }
    async fn sweep_kept_metadata(
        &self,
        system: &str,
        owners: &[orch_driver_contract::kept::KeptOwner],
    ) -> Result<usize, DriverError> {
        DockerDriver::sweep_kept_metadata(self, system, owners).await
    }
    async fn remove_kept_image(
        &self,
        image: &orch_driver_contract::kept::KeptImage,
    ) -> Result<(), DriverError> {
        self.drop_kept_image(image).await
    }
    async fn export_kept_image(
        &self,
        image: &orch_driver_contract::kept::KeptImage,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.save_kept_image(image, into).await
    }
    async fn import_kept_image(
        &self,
        image: &orch_driver_contract::kept::KeptImage,
        from: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.load_kept_image(image, from).await
    }
    async fn apply_resources(
        &self,
        unit: &UnitRef,
        resources: &orch_driver_contract::runtime::Resources,
    ) -> Result<(), DriverError> {
        self.update_resources(unit, resources).await
    }
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        Some(DockerDriver::storages(self))
    }
    async fn volume_exists(&self, volume: &VolumeRequest) -> Result<bool, DriverError> {
        DockerDriver::volume_exists(self, volume).await
    }
    async fn volume_identity(
        &self,
        volume: &VolumeRequest,
    ) -> Result<Option<orch_driver_contract::volumes::VolumeIdentity>, DriverError> {
        crate::volumes::identity(&self.daemon, &self.config, volume).await
    }
    async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        DockerDriver::suspend(self, unit).await
    }
    async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        DockerDriver::resume(self, unit).await
    }
    async fn registry_digest(&self, reference: &str) -> Result<Option<String>, DriverError> {
        DockerDriver::registry_digest(self, reference).await
    }
    async fn ensure_image_reporting(
        &self,
        spec: &Json,
        progress: &dyn orch_driver_contract::sources::ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        DockerDriver::ensure_image_reporting(self, spec, &ContractProgress(progress)).await
    }
    async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn orch_driver_contract::sources::ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        DockerDriver::rebuild_image(self, spec, &ContractProgress(progress)).await
    }
    fn describe(&self) -> DriverDescriptor {
        let mut described = descriptor(self.daemon.facts(), &self.config);
        described.live_changes.mounts = self.live_helpers_supported();
        described.data_storage.existing_directory = self.live_helpers_supported();
        described.data_storage.reason = (!self.live_helpers_supported()).then(|| "existing directories need a verified Linux daemon and the node's trusted static launch helper".into());
        if self.live_helpers_supported() {
            described.live_changes.capabilities = vec!["kvm".into(), "docker_socket".into()];
        }
        // Each runtime of the daemon as a class, laid over what is declared above for a unit that shares the kernel.
        let restricted = described
            .network_egress
            .contains(&orch_driver_contract::network::EgressMode::Restricted);
        described.runtimes = crate::runtimes::classes(
            self.daemon.facts(),
            &self.config,
            &described.live_changes,
            restricted,
            &self.restricted_proved,
        );
        described.default_runtime =
            crate::runtimes::default_class(self.daemon.facts(), &described.runtimes);
        described
    }

    async fn validate(&self, kind: RuntimeKind, spec: &Json) -> Result<(), DriverError> {
        let docker = DockerSpec::parse(kind, spec)?;
        if docker.source.kind() == orch_driver_contract::sources::SourceKind::Build
            && !self.config.builds
        {
            return Err(DriverError::InvalidSpec(
                "this host does not build images".to_owned(),
            ));
        }
        Ok(())
    }

    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        self.create_unit(spec).await
    }

    async fn start(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.start_recorded_capabilities(unit).await
    }

    async fn start_with_capabilities(
        &self,
        unit: &UnitRef,
        state: &orch_driver_contract::capabilities::CapabilityState,
    ) -> Result<(), DriverError> {
        self.start_live_capabilities(unit, state).await
    }

    async fn apply_capabilities(
        &self,
        unit: &UnitRef,
        before: &orch_driver_contract::capabilities::CapabilityState,
        after: &orch_driver_contract::capabilities::CapabilityState,
    ) -> Result<bool, DriverError> {
        self.apply_live_capabilities(unit, before, after).await
    }

    async fn apply_mounts(
        &self,
        unit: &UnitRef,
        change: &orch_driver_contract::mounts::MountChange,
    ) -> Result<Vec<orch_driver_contract::mounts::AttachedMount>, DriverError> {
        self.change_live_mounts(unit, change).await
    }

    async fn stop(&self, unit: &UnitRef, grace: Duration) -> Result<(), DriverError> {
        self.stop_unit(unit, grace).await
    }

    async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.remove_unit(unit).await
    }

    async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
        self.inspect_unit(unit).await
    }

    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        self.list_units(system).await
    }

    async fn stats(&self, unit: &UnitRef) -> Result<ResourceSample, DriverError> {
        self.stats_unit(unit).await
    }

    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        volumes::ensure(&self.daemon, &self.config, volume).await
    }

    async fn remove_volume(&self, name: &str) -> Result<(), DriverError> {
        volumes::remove(&self.daemon, &self.config, name).await
    }

    async fn ensure_image(&self, spec: &Json) -> Result<ImageEnsured, DriverError> {
        self.ensure_source(spec, &Silent).await
    }

    async fn import(&self, request: &ImportRequest) -> Result<ImportProposal, DriverError> {
        self.propose(request).await
    }

    async fn apply_network(
        &self,
        unit: &UnitRef,
        policy: &NetworkPolicy,
    ) -> Result<UnitNetwork, DriverError> {
        self.apply_unit_network(unit, policy).await
    }

    fn control_endpoints(&self, addresses: Vec<String>) {
        *self
            .control_endpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = addresses;
    }

    fn control_link(&self, connected: bool) {
        self.heartbeat.set_link(connected);
    }

    async fn inspect_network(&self, unit: &UnitRef) -> Result<UnitNetwork, DriverError> {
        self.observe_unit_network(unit).await
    }
}

struct ContractProgress<'a>(&'a dyn orch_driver_contract::sources::ImageProgress);
impl ProgressSink for ContractProgress<'_> {
    fn line(&self, phase: crate::progress::Phase, text: &str) {
        use orch_driver_contract::sources::ImageStep;
        self.0.line(
            match phase {
                crate::progress::Phase::Pull => ImageStep::Pull,
                crate::progress::Phase::Build => ImageStep::Build,
                crate::progress::Phase::Load => ImageStep::Load,
            },
            text,
        );
    }
}

impl DockerDriver {
    /// Proposes a spec from what a person has; see `EnvironmentDriver::import`.
    async fn propose(&self, request: &ImportRequest) -> Result<ImportProposal, DriverError> {
        let descriptor = self.describe();
        let Some(def) = descriptor.import(&request.format) else {
            return Err(DriverError::Unsupported(format!(
                "import of {}",
                request.format
            )));
        };
        def.check_values(&request.values).map_err(|faults| {
            DriverError::InvalidSpec(
                faults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        let text = |name: &str| {
            request
                .values
                .get(name)
                .and_then(Json::as_str)
                .filter(|v| !v.is_empty())
        };
        match request.format.as_str() {
            IMPORT_DOCKERFILE => {
                let context = if let Some(blob) = text("context_blob") {
                    Some(serde_json::json!({ "blob": blob }))
                } else {
                    text("git").map(|git| {
                        serde_json::json!({ "git": git, "ref": text("git_ref").unwrap_or("main"), "path": text("git_path").unwrap_or(".") })
                    })
                };
                crate::imports::propose_dockerfile(
                    request.kind,
                    text("dockerfile").unwrap_or_default(),
                    context,
                )
            }
            IMPORT_IMAGE => {
                self.propose_image_import(request.kind, text("reference").unwrap_or_default())
                    .await
            }
            IMPORT_ARCHIVE => {
                self.propose_archive_import(
                    request.kind,
                    text("blob").unwrap_or_default(),
                    text("image"),
                )
                .await
            }
            other => Err(DriverError::Unsupported(format!("import of {other}"))),
        }
    }
}
