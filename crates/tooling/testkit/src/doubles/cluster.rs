//! A cluster-shaped driver with opaque units, storage classes and independently failing machines.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use orch_driver_contract::descriptor::{DriverDescriptor, OperationSupport};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::host::HostResources;
use orch_driver_contract::resources::{Resource, ResourceKind, ResourceRef};
use orch_driver_contract::runtime::{LABEL_SYSTEM, RuntimeKind, RuntimeSpec};
use orch_driver_contract::sources::{ImageEnsured, Source, SourceKind};
use orch_driver_contract::units::{UnitRef, UnitState, UnitStateKind};
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest, resolve_storage};
use orch_types::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::driver::MemoryDriver;
mod archive;
pub use archive::VolumeCodec;

/// One independently failing member of a host's scheduling pool.
#[derive(Debug, Clone)]
pub struct ClusterMachine {
    /// Opaque placement identity, private to the driver.
    pub id: String,
    /// Logical processors advertised by this member.
    pub cpus: i64,
    /// Schedulable memory in MiB advertised by this member.
    pub memory_mb: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    namespace: String,
}

#[derive(Default)]
struct Placement {
    machines: BTreeMap<String, (ClusterMachine, bool)>,
    units: BTreeMap<String, String>,
    unit_ids: BTreeMap<String, orch_types::Id>,
    volume_ids: BTreeMap<String, orch_types::Id>,
    files: BTreeMap<String, BTreeMap<String, ClusterFile>>,
}

/// One regular file and its metadata in the simulated storage service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterFile {
    /// Stored bytes.
    pub data: Vec<u8>,
    /// Numeric owner.
    pub uid: u64,
    /// Numeric group.
    pub gid: u64,
    /// Permission bits.
    pub mode: u32,
    /// Access time in Unix seconds.
    pub atime: i64,
    /// Access time fractional nanoseconds.
    pub atime_ns: u32,
    /// Modification time in Unix seconds.
    pub mtime: i64,
    /// Modification time fractional nanoseconds.
    pub mtime_ns: u32,
    /// Extended attributes, including binary values.
    pub xattrs: BTreeMap<String, Vec<u8>>,
}

/// A consumer-supplied canonical archive codec, avoiding another dependency in the model.
pub trait ClusterArchiveCodec: Send + Sync {
    /// Encodes one atomic file snapshot with its metadata.
    fn encode(&self, files: &BTreeMap<String, ClusterFile>) -> Result<Vec<u8>, DriverError>;
    /// Validates and restores the same metadata.
    fn decode(&self, archive: &[u8]) -> Result<BTreeMap<String, ClusterFile>, DriverError>;
}

/// The fixed persistent subtree used by real file-state exporters.
const STORE_PREFIX: &str = ".orch/file-states/";
/// Format parts, not a limit on the records each part may hold.
const STORE_PARTS: &[&str] = &[
    "git",
    "roots",
    "manifests",
    "journals",
    "plans",
    "results",
    "policies",
    "pins",
    "source-jobs",
    "git-jobs",
    "worktree-jobs",
    "materialize-jobs",
];
/// Ordinary regular witness files use owner write and public read permission.
const WITNESS_MODE: u32 = 0o644;

/// A host spanning several machines, with no paths shared with its node agent.
pub struct ClusterDriver {
    memory: MemoryDriver,
    lifecycle: tokio::sync::Mutex<()>,
    descriptor: DriverDescriptor,
    placement: Mutex<Placement>,
    storages: Vec<StorageInfo>,
    archive: Option<Arc<dyn ClusterArchiveCodec>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ClusterDriver {
    /// A pool with caller-chosen capacities and shared storage classes, avoiding local-machine facts.
    #[must_use]
    pub fn new(machines: Vec<ClusterMachine>, storages: Vec<StorageInfo>) -> Self {
        let memory = MemoryDriver::new("cluster-test")
            .replacing()
            .with_sources(vec![SourceKind::Image])
            .with_storages(storages.clone())
            .enforcing_exclusive_volumes();
        let mut descriptor = memory.describe();
        descriptor.title = "Cluster test pool".into();
        descriptor.configuration_schema = Some(
            json!({"type":"object","properties":{"namespace":{"type":"string","minLength":1}},"required":["namespace"],"additionalProperties":false}),
        );
        descriptor.host_scope = Some(orch_driver_contract::host::HostScope::Driver);
        descriptor.tool_delivery = Some(orch_driver_contract::host::ToolDelivery::Driver);
        descriptor.gateway_access = Some(orch_driver_contract::host::GatewayAccess::DriverNetwork);
        descriptor.host_ports =
            OperationSupport::available(false, "this pool has no node-local host listeners");
        descriptor.kept_file_transfer =
            OperationSupport::available(false, "this pool uses immutable image references");
        descriptor.recreate_preserves_files = false;
        descriptor.data_storage.made_by_orch =
            Some(orch_driver_contract::descriptor::MadeDataKind::Volume);
        descriptor.data_storage.existing_directory = false;
        descriptor.data_storage.reason = Some("node paths are not shared with this pool".into());
        descriptor.mounts = orch_driver_contract::mounts::MountPlacement::Placed;
        descriptor.network_egress = vec![orch_driver_contract::network::EgressMode::Open];
        descriptor.unavailable = BTreeMap::from([
            (
                "host_paths".into(),
                "node paths are not shared with this pool".into(),
            ),
            (
                "build".into(),
                "this pool accepts image references and has no builder".into(),
            ),
            (
                "archive".into(),
                "this pool does not load image archives".into(),
            ),
            (
                "kept_files".into(),
                "this pool uses immutable image references".into(),
            ),
            (
                "private_views".into(),
                "storage classes do not offer private views".into(),
            ),
            (
                "suspend".into(),
                "this pool cannot suspend every process in a unit".into(),
            ),
            (
                "live_mounts".into(),
                "mount changes require replacing the unit".into(),
            ),
        ]);
        descriptor.spec_schemas = [RuntimeKind::Environment, RuntimeKind::Provider].into_iter()
            .map(|kind| (kind, json!({"type":"object","properties":{"image":{"type":"string","minLength":1}},"required":["image"],"additionalProperties":true}))).collect();
        let placement = Placement {
            machines: machines
                .into_iter()
                .map(|machine| (machine.id.clone(), (machine, true)))
                .collect(),
            ..Placement::default()
        };
        Self {
            memory,
            lifecycle: tokio::sync::Mutex::new(()),
            descriptor,
            placement: Mutex::new(placement),
            storages,
            archive: None,
        }
    }

    /// Enables metadata-preserving carry and atomic snapshots using the consumer's existing archive codec.
    #[must_use]
    pub fn with_archive_codec(mut self, codec: Arc<dyn ClusterArchiveCodec>) -> Self {
        self.archive = Some(codec);
        self.descriptor.volume_export_consistency =
            Some(orch_driver_contract::host::VolumeExportConsistency::Driver);
        self
    }

    /// Simulates a definitively failed machine whose processes have ended, rather than a partition.
    pub fn fail_machine(&self, machine: &str) {
        if let Some((_, healthy)) = lock(&self.placement).machines.get_mut(machine) {
            *healthy = false;
        }
        for unit in self.memory.units() {
            if self.machine_of(&unit.unit).as_deref() == Some(machine) {
                self.memory
                    .mark_failed(&unit.unit.name, "the unit's machine has failed");
            }
        }
    }

    /// The driver's placement decision, exposed only for a test's observation.
    #[must_use]
    pub fn machine_of(&self, unit: &UnitRef) -> Option<String> {
        lock(&self.placement).units.get(&unit.name).cloned()
    }

    /// Owned persistent volumes, independent of the units and their machines.
    #[must_use]
    pub fn volumes(&self) -> Vec<super::driver::VolumeRecord> {
        self.memory.volumes()
    }

    /// The launch request retained by the simulated unit, for observing a transfer's destination.
    #[must_use]
    pub fn unit_spec(&self, runtime: orch_types::Id) -> Option<RuntimeSpec> {
        self.memory
            .units()
            .into_iter()
            .find(|unit| unit.unit.runtime_id == runtime)
            .map(|unit| unit.spec)
    }

    /// Writes a test witness through storage, with no node filesystem location.
    pub fn write_volume(
        &self,
        volume: &str,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<(), DriverError> {
        let mut file = self
            .stored_files(volume)
            .remove(path)
            .unwrap_or(ClusterFile {
                data: Vec::new(),
                uid: 0,
                gid: 0,
                mode: WITNESS_MODE,
                atime: 0,
                atime_ns: 0,
                mtime: 0,
                mtime_ns: 0,
                xattrs: BTreeMap::new(),
            });
        file.data = bytes;
        self.write_volume_file(volume, path, file)
    }

    /// Writes explicit metadata for a transfer or recovery witness.
    pub fn write_volume_file(
        &self,
        volume: &str,
        path: &str,
        file: ClusterFile,
    ) -> Result<(), DriverError> {
        if !self
            .memory
            .volumes()
            .iter()
            .any(|known| known.name == volume)
        {
            return Err(DriverError::NotFound(
                "the owned volume does not exist".into(),
            ));
        }
        lock(&self.placement)
            .files
            .entry(volume.into())
            .or_default()
            .insert(path.into(), file);
        Ok(())
    }

    /// Reads witness contents from the simulated storage service.
    #[must_use]
    pub fn volume_files(&self, volume: &str) -> BTreeMap<String, Vec<u8>> {
        self.stored_files(volume)
            .into_iter()
            .map(|(path, file)| (path, file.data))
            .collect()
    }

    /// Every stored regular file, with metadata rather than just its contents.
    #[must_use]
    pub fn stored_files(&self, volume: &str) -> BTreeMap<String, ClusterFile> {
        lock(&self.placement)
            .files
            .get(volume)
            .cloned()
            .unwrap_or_default()
    }

    fn selected(path: &str, selector: orch_driver_contract::volumes::OwnedVolumeSelector) -> bool {
        match selector {
            orch_driver_contract::volumes::OwnedVolumeSelector::Home => {
                !path.strip_prefix(STORE_PREFIX).is_some_and(|part| {
                    let name = part.split('/').next().unwrap_or("");
                    name.starts_with("source-credential-") || name.starts_with(".tmp")
                })
            }
            orch_driver_contract::volumes::OwnedVolumeSelector::FileStates => path
                .strip_prefix(STORE_PREFIX)
                .is_some_and(|part| STORE_PARTS.contains(&part.split('/').next().unwrap_or(""))),
            orch_driver_contract::volumes::OwnedVolumeSelector::Private(_) => false,
        }
    }

    /// Captures an atomic file snapshot and encodes the selected owned scope.
    pub fn volume_archive(
        &self,
        volume: &str,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<Vec<u8>, DriverError> {
        self.selected_volume(volume, selector)?;
        let codec = self
            .archive
            .as_ref()
            .ok_or_else(|| DriverError::Unsupported("the model has no archive codec".into()))?;
        let files = self
            .stored_files(volume)
            .into_iter()
            .filter(|(path, _)| Self::selected(path, selector))
            .map(|(path, file)| {
                (
                    if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
                        path.strip_prefix(STORE_PREFIX).unwrap_or(&path).to_owned()
                    } else {
                        path
                    },
                    file,
                )
            })
            .collect();
        codec.encode(&files)
    }

    /// Restores exactly the requested scope while keeping other home data separate.
    pub fn apply_volume_archive(
        &self,
        volume: &str,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        archive: &[u8],
    ) -> Result<(), DriverError> {
        self.selected_volume(volume, selector)?;
        let codec = self
            .archive
            .as_ref()
            .ok_or_else(|| DriverError::Unsupported("the model has no archive codec".into()))?;
        let files = codec
            .decode(archive)?
            .into_iter()
            .map(|(path, file)| {
                (
                    if selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates {
                        format!("{STORE_PREFIX}{path}")
                    } else {
                        path
                    },
                    file,
                )
            })
            .collect::<BTreeMap<_, _>>();
        if files.keys().any(|path| !Self::selected(path, selector)) {
            return Err(DriverError::InvalidSpec(
                "the archive names files outside the selected owned scope".into(),
            ));
        }
        let mut placement = lock(&self.placement);
        let target = placement.files.entry(volume.into()).or_default();
        target.retain(|path, _| match selector {
            orch_driver_contract::volumes::OwnedVolumeSelector::Home => false,
            orch_driver_contract::volumes::OwnedVolumeSelector::FileStates => {
                !path.starts_with(STORE_PREFIX)
            }
            orch_driver_contract::volumes::OwnedVolumeSelector::Private(_) => false,
        });
        target.extend(files);
        Ok(())
    }

    fn selected_volume(
        &self,
        volume: &str,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
    ) -> Result<(), DriverError> {
        if matches!(
            selector,
            orch_driver_contract::volumes::OwnedVolumeSelector::Private(_)
        ) {
            return Err(DriverError::Unsupported(
                "storage classes have no private views".into(),
            ));
        }
        if self.memory.volume(volume).is_none() {
            return Err(DriverError::NotFound(
                "the selected owned volume is absent".into(),
            ));
        }
        Ok(())
    }

    async fn ensure_volume_inner(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        let storage = resolve_storage(volume.storage.as_deref(), &self.storages)
            .map_err(|error| DriverError::InvalidSpec(error.to_string()))?;
        if let Some(known) = self
            .memory
            .volumes()
            .iter()
            .find(|known| known.name == volume.name)
            && known.storage.as_deref() != Some(&storage.name)
        {
            return Err(DriverError::InvalidSpec(
                "an existing volume cannot change storage class".into(),
            ));
        }
        let mut request = volume.clone();
        request.storage = Some(storage.name.clone());
        self.memory.ensure_volume(&request).await?;
        lock(&self.placement)
            .volume_ids
            .entry(volume.name.clone())
            .or_insert_with(orch_types::new_id);
        Ok(())
    }

    async fn remove_unit_inner(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.memory.remove(unit).await?;
        let mut placement = lock(&self.placement);
        placement.units.remove(&unit.name);
        placement.unit_ids.remove(&unit.name);
        Ok(())
    }

    async fn remove_volume_inner(&self, name: &str) -> Result<(), DriverError> {
        if self
            .memory
            .units()
            .iter()
            .any(|unit| unit.spec.volumes.values().any(|volume| volume.name == name))
        {
            return Err(DriverError::Busy("a unit still owns this volume".into()));
        }
        self.memory.remove_volume(name).await?;
        let mut placement = lock(&self.placement);
        placement.files.remove(name);
        placement.volume_ids.remove(name);
        Ok(())
    }

    fn resource_ref(kind: ResourceKind, id: orch_types::Id) -> ResourceRef {
        ResourceRef {
            kind,
            id: format!("pool-object:{id}"),
        }
    }

    fn inventory(&self, system: &str) -> Result<Vec<Resource>, DriverError> {
        if system.trim().is_empty() {
            return Err(DriverError::InvalidSpec(
                "an installation label is required".into(),
            ));
        }
        let units = self.memory.units();
        let volumes = self.memory.volumes();
        let placement = lock(&self.placement);
        let mut resources = Vec::new();
        for unit in units.iter().filter(|unit| {
            unit.spec
                .labels
                .get(LABEL_SYSTEM)
                .is_some_and(|owner| owner == system)
        }) {
            let identity = placement
                .unit_ids
                .get(&unit.unit.name)
                .copied()
                .ok_or_else(|| {
                    DriverError::Unavailable("the unit's physical identity is not observed".into())
                })?;
            let mut depends_on = Vec::new();
            for name in unit.volumes() {
                let identity = placement.volume_ids.get(&name).copied().ok_or_else(|| {
                    DriverError::Unavailable("a unit's storage identity is not observed".into())
                })?;
                depends_on.push(Self::resource_ref(ResourceKind::Volume, identity));
            }
            depends_on.extend(unit.spec.tools.iter().map(|tool| ResourceRef {
                kind: ResourceKind::ToolVersion,
                id: format!("{}/{}", tool.name, tool.version),
            }));
            depends_on.sort();
            depends_on.dedup();
            resources.push(Resource {
                resource: Self::resource_ref(ResourceKind::Unit, identity),
                name: unit.unit.name.clone(),
                size_bytes: None,
                labels: unit.spec.labels.clone(),
                depends_on,
                aliases: Vec::new(),
                last_used_at: None,
                busy: unit.state.state == UnitStateKind::Running,
            });
        }
        for volume in volumes.iter().filter(|volume| {
            volume
                .labels
                .get(LABEL_SYSTEM)
                .is_some_and(|owner| owner == system)
        }) {
            let identity = placement
                .volume_ids
                .get(&volume.name)
                .copied()
                .ok_or_else(|| {
                    DriverError::Unavailable(
                        "the volume's physical identity is not observed".into(),
                    )
                })?;
            resources.push(Resource {
                resource: Self::resource_ref(ResourceKind::Volume, identity),
                name: volume.name.clone(),
                size_bytes: None,
                labels: volume.labels.clone(),
                depends_on: Vec::new(),
                aliases: Vec::new(),
                last_used_at: None,
                busy: units
                    .iter()
                    .any(|unit| unit.volumes().contains(&volume.name)),
            });
        }
        resources.sort_by(|one, two| one.resource.cmp(&two.resource));
        Ok(resources)
    }

    fn opaque(mut unit: UnitRef) -> UnitRef {
        unit.reference = format!("unit:{}", unit.runtime_id);
        unit
    }

    fn healthy(&self, unit: &UnitRef) -> bool {
        let placement = lock(&self.placement);
        placement
            .units
            .get(&unit.name)
            .and_then(|id| placement.machines.get(id))
            .is_some_and(|(_, healthy)| *healthy)
    }
}

#[async_trait]
impl EnvironmentDriver for ClusterDriver {
    async fn resources(&self, system: &str) -> Result<Vec<Resource>, DriverError> {
        let _held = self.lifecycle.lock().await;
        self.inventory(system)
    }

    async fn stop_resource(
        &self,
        system: &str,
        resource: &ResourceRef,
        grace: Duration,
    ) -> Result<(), DriverError> {
        if resource.kind != ResourceKind::Unit {
            return Err(DriverError::InvalidSpec(
                "only a unit can be stopped".into(),
            ));
        }
        let _held = self.lifecycle.lock().await;
        let resources = self.inventory(system)?;
        let selected = resources
            .iter()
            .find(|entry| &entry.resource == resource)
            .ok_or_else(|| DriverError::NotFound("the exact owned unit is absent".into()))?;
        let unit = self
            .memory
            .unit(&selected.name)
            .ok_or_else(|| DriverError::NotFound("the exact owned unit is absent".into()))?;
        self.memory.stop(&Self::opaque(unit.unit), grace).await
    }

    async fn remove_resource(
        &self,
        system: &str,
        resource: &ResourceRef,
    ) -> Result<(), DriverError> {
        let _held = self.lifecycle.lock().await;
        let resources = self.inventory(system)?;
        let selected = resources
            .iter()
            .find(|entry| &entry.resource == resource)
            .ok_or_else(|| DriverError::NotFound("the exact owned resource is absent".into()))?;
        if selected.busy
            || resources
                .iter()
                .any(|entry| entry.depends_on.contains(resource))
        {
            return Err(DriverError::Busy(
                "an owned dependency still keeps this resource".into(),
            ));
        }
        match resource.kind {
            ResourceKind::Unit => {
                let unit = self.memory.unit(&selected.name).ok_or_else(|| {
                    DriverError::NotFound("the exact owned unit is absent".into())
                })?;
                self.remove_unit_inner(&Self::opaque(unit.unit)).await
            }
            ResourceKind::Volume => self.remove_volume_inner(&selected.name).await,
            _ => Err(DriverError::NotFound(
                "this pool has no such owned object".into(),
            )),
        }
    }

    async fn check_volume_carry(&self) -> Result<(), DriverError> {
        if self.archive.is_some() {
            Ok(())
        } else {
            Err(DriverError::Unsupported(
                "the model has no archive codec".into(),
            ))
        }
    }

    async fn measure_volume(
        &self,
        volume: &VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.selected_volume(&volume.name, selector)?;
        if work.is_cancelled() {
            return Err(DriverError::Timeout);
        }
        let files: BTreeMap<_, _> = self
            .stored_files(&volume.name)
            .into_iter()
            .filter(|(path, _)| Self::selected(path, selector))
            .collect();
        let file_count =
            i64::try_from(files.len()).map_err(|error| DriverError::Internal(error.to_string()))?;
        let total_bytes = files
            .values()
            .try_fold(0_i64, |sum, file| {
                i64::try_from(file.data.len()).map(|size| sum.saturating_add(size))
            })
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        let file_states = (selector
            == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates)
            .then(|| orch_driver_contract::volumes::FileStatesMeasure {
                roots: i64::try_from(
                    files
                        .keys()
                        .filter(|path| path.starts_with(".orch/file-states/roots/"))
                        .count(),
                )
                .unwrap_or(i64::MAX),
                objects: i64::try_from(
                    files
                        .keys()
                        .filter(|path| path.starts_with(".orch/file-states/git/objects/"))
                        .count(),
                )
                .unwrap_or(i64::MAX),
                journals: i64::try_from(
                    files
                        .keys()
                        .filter(|path| path.contains("/journals/") || path.contains("-jobs/"))
                        .count(),
                )
                .unwrap_or(i64::MAX),
                total_bytes,
                complete: true,
            });
        work.report(orch_driver_contract::volumes::VolumeProgress {
            files_done: file_count,
            bytes_done: total_bytes,
            files_total: Some(file_count),
            bytes_total: Some(total_bytes),
            ..Default::default()
        });
        Ok(orch_driver_contract::volumes::VolumeMeasure {
            file_count,
            total_bytes,
            file_states,
        })
    }

    async fn export_volume(
        &self,
        volume: &VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        if work.is_cancelled() {
            return Err(DriverError::Timeout);
        }
        let archive = self.volume_archive(&volume.name, selector)?;
        #[cfg(unix)]
        let fifo = {
            use std::os::unix::fs::FileTypeExt;
            std::fs::symlink_metadata(into).is_ok_and(|metadata| metadata.file_type().is_fifo())
        };
        #[cfg(not(unix))]
        let fifo = false;
        if fifo {
            #[cfg(unix)]
            {
                use tokio::io::AsyncWriteExt;
                let mut output = tokio::net::unix::pipe::OpenOptions::new()
                    .open_sender(into)
                    .map_err(|error| DriverError::Internal(error.to_string()))?;
                tokio::select! {
                    result=output.write_all(&archive)=>result.map_err(|error|DriverError::Internal(error.to_string()))?,
                    ()=work.cancelled()=>return Err(DriverError::Timeout),
                }
            }
        } else {
            tokio::select! {
                result = tokio::fs::write(into, &archive) => result.map_err(|error| DriverError::Internal(error.to_string()))?,
                () = work.cancelled() => return Err(DriverError::Timeout),
            }
        }
        let bytes = i64::try_from(archive.len())
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        work.report(orch_driver_contract::volumes::VolumeProgress {
            bytes_done: bytes,
            bytes_total: Some(bytes),
            message: Some("Owned volume exported".into()),
            ..Default::default()
        });
        Ok(())
    }

    async fn import_volume(
        &self,
        volume: &VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        from: &std::path::Path,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        let _held = self.lifecycle.lock().await;
        if work.is_cancelled() {
            return Err(DriverError::Timeout);
        }
        self.selected_volume(&volume.name, selector)?;
        if !self.volume_exists(volume).await? {
            return Err(DriverError::NotFound(
                "the prepared owned volume is absent".into(),
            ));
        }
        if self.memory.units().iter().any(|unit| {
            unit.spec
                .volumes
                .values()
                .any(|known| known.name == volume.name)
        }) {
            return Err(DriverError::Busy(
                "a unit still owns the import destination".into(),
            ));
        }
        use tokio::io::AsyncReadExt;
        #[cfg(unix)]
        let fifo = {
            use std::os::unix::fs::FileTypeExt;
            std::fs::symlink_metadata(from).is_ok_and(|metadata| metadata.file_type().is_fifo())
        };
        #[cfg(not(unix))]
        let fifo = false;
        let mut input: Box<dyn tokio::io::AsyncRead + Unpin + Send> = if fifo {
            #[cfg(unix)]
            {
                Box::new(
                    tokio::net::unix::pipe::OpenOptions::new()
                        .open_receiver(from)
                        .map_err(|error| DriverError::Internal(error.to_string()))?,
                )
            }
            #[cfg(not(unix))]
            {
                return Err(DriverError::Unsupported(
                    "FIFO import requires a Unix node".into(),
                ));
            }
        } else {
            Box::new(tokio::select! {
                result = tokio::fs::File::open(from) => result.map_err(|error| DriverError::Internal(error.to_string()))?,
                () = work.cancelled() => return Err(DriverError::Timeout),
            })
        };
        work.input_ready();
        let mut archive = Vec::new();
        tokio::select! {
            result = input.read_to_end(&mut archive) => {result.map_err(|error| DriverError::Internal(error.to_string()))?;},
            () = work.cancelled() => return Err(DriverError::Timeout),
        }
        self.apply_volume_archive(&volume.name, selector, &archive)?;
        let bytes = i64::try_from(archive.len())
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        work.report(orch_driver_contract::volumes::VolumeProgress {
            bytes_done: bytes,
            bytes_total: Some(bytes),
            message: Some("Owned volume imported".into()),
            ..Default::default()
        });
        Ok(())
    }

    fn describe(&self) -> DriverDescriptor {
        self.descriptor.clone()
    }

    fn validate_configuration(&self, value: &Json) -> Result<(), DriverError> {
        let config: Configuration = serde_json::from_value(value.clone()).map_err(|_| {
            DriverError::InvalidSpec("configuration needs only a namespace string".into())
        })?;
        if config.namespace.is_empty() {
            return Err(DriverError::InvalidSpec(
                "namespace must not be empty".into(),
            ));
        }
        Ok(())
    }

    fn host_resources(&self) -> Result<HostResources, DriverError> {
        let placement = lock(&self.placement);
        let (cpus, memory_mb) = placement
            .machines
            .values()
            .filter(|(_, healthy)| *healthy)
            .fold((0_i64, 0_i64), |(cpus, memory), (machine, _)| {
                (
                    cpus.saturating_add(machine.cpus),
                    memory.saturating_add(machine.memory_mb),
                )
            });
        let (disk_gb, disk_free_gb) =
            self.storages
                .iter()
                .fold((0_i64, 0_i64), |(capacity, free), storage| {
                    (
                        capacity.saturating_add(storage.capacity_gb),
                        free.saturating_add(storage.free_gb),
                    )
                });
        Ok(HostResources {
            cpus,
            cpu_millis: None,
            memory_mb,
            disk_gb,
            cpu_percent: Some(0.0),
            memory_used_mb: Some(0),
            disk_used_gb: Some(disk_gb.saturating_sub(disk_free_gb)),
            disk_free_gb: Some(disk_free_gb),
        })
    }

    fn storages(&self) -> Option<Vec<StorageInfo>> {
        Some(self.storages.clone())
    }

    async fn validate(&self, _kind: RuntimeKind, spec: &Json) -> Result<(), DriverError> {
        match Source::from_spec(spec)
            .map_err(|error| DriverError::InvalidSpec(error.to_string()))?
        {
            Source::Image { .. } => Ok(()),
            _ => Err(DriverError::Unsupported(
                "this pool accepts image references only".into(),
            )),
        }
    }

    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        let _held = self.lifecycle.lock().await;
        self.validate(spec.kind, &spec.spec).await?;
        if spec
            .volumes
            .values()
            .any(|volume| volume.directory.is_some())
            || spec
                .mounts
                .iter()
                .any(|mount| mount.kind == orch_driver_contract::mounts::MountKind::Path)
        {
            return Err(DriverError::Unsupported(
                "node paths are not shared with this pool".into(),
            ));
        }
        for volume in spec.volumes.values() {
            self.ensure_volume_inner(&VolumeRequest {
                name: volume.name.clone(),
                storage: volume.storage.clone(),
                labels: spec.labels.clone(),
            })
            .await?;
        }
        {
            let mut placement = lock(&self.placement);
            if !placement.units.contains_key(&spec.name) {
                let machine = placement
                    .machines
                    .iter()
                    .filter(|(_, (_, healthy))| *healthy)
                    .min_by_key(|(id, _)| {
                        placement
                            .units
                            .values()
                            .filter(|placed| *placed == *id)
                            .count()
                    })
                    .map(|(id, _)| id.clone())
                    .ok_or_else(|| {
                        DriverError::NoCapacity("this pool has no healthy machine".into())
                    })?;
                placement.units.insert(spec.name.clone(), machine);
                placement
                    .unit_ids
                    .entry(spec.name.clone())
                    .or_insert_with(orch_types::new_id);
            }
        }
        self.memory.create(spec).await.map(Self::opaque)
    }

    async fn start(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _held = self.lifecycle.lock().await;
        if !self.healthy(unit) {
            return Err(DriverError::Unavailable(
                "the unit's machine has failed".into(),
            ));
        }
        self.memory.start(unit).await
    }

    async fn stop(&self, unit: &UnitRef, grace: Duration) -> Result<(), DriverError> {
        let _held = self.lifecycle.lock().await;
        self.memory.stop(unit, grace).await
    }

    async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _held = self.lifecycle.lock().await;
        self.remove_unit_inner(unit).await
    }

    async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
        let state = self.memory.inspect(unit).await?;
        if state.state != UnitStateKind::Absent && !self.healthy(unit) {
            let mut failed = UnitState::of(UnitStateKind::Failed);
            failed.detail = Some("the unit's machine has failed".into());
            return Ok(failed);
        }
        Ok(state)
    }

    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        self.memory
            .list(system)
            .await
            .map(|units| units.into_iter().map(Self::opaque).collect())
    }

    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        let _held = self.lifecycle.lock().await;
        self.ensure_volume_inner(volume).await
    }

    async fn volume_exists(&self, volume: &VolumeRequest) -> Result<bool, DriverError> {
        let Some(known) = self.memory.volume(&volume.name) else {
            return Ok(false);
        };
        // A missing legacy owner can use its recorded system, while a conflicting present owner cannot.
        let owners = ["orch.environment_id", "orch.provider_id", "orch.system"];
        let owned = owners.iter().any(|key| {
            volume
                .labels
                .get(*key)
                .is_some_and(|value| known.labels.get(*key) == Some(value))
        });
        if owners.iter().any(|key| volume.labels.contains_key(*key)) && !owned
            || volume.labels.iter().any(|(key, value)| {
                known.labels.get(key) != Some(value)
                    && !(owned && owners.contains(&key.as_str()) && !known.labels.contains_key(key))
            })
        {
            return Err(DriverError::InvalidSpec(
                "the volume's ownership does not match its frozen reference".into(),
            ));
        }
        Ok(volume
            .storage
            .as_ref()
            .is_none_or(|storage| known.storage.as_ref() == Some(storage)))
    }

    async fn remove_volume(&self, name: &str) -> Result<(), DriverError> {
        let _held = self.lifecycle.lock().await;
        self.remove_volume_inner(name).await
    }

    async fn ensure_image(&self, spec: &Json) -> Result<ImageEnsured, DriverError> {
        self.validate(RuntimeKind::Environment, spec).await?;
        self.memory.ensure_image(spec).await
    }
}
