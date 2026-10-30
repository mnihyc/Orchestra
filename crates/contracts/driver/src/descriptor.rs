//! A schema for each pool it makes units for, confinement, capabilities, mount, private-view, storage and suspend support, and runtime classes.
//!
//! A driver turns a template into a running environment. Orch's core never asks which driver is in use: it
//! reads the driver's [`DriverDescriptor`] and acts on what that declares, so that a driver with no
//! confinement, no capabilities, no suspend, no sources and no imports passes the environment suite as well
//! as `docker` does.
//!
//! | Member | Meaning |
//! |--------|---------|
//! | `spec_schemas` | JSON Schema of the spec it accepts, one for each pool it can make units for; each validates the templates of its kind and renders their form |
//! | `confinement` | Whether the driver itself confines the agent; it sets the defaults proposed for a new template |
//! | `network_egress` | The egress modes this host can enforce; `restricted` only after its isolation and proxy probe passed |
//! | `capabilities` | The capability keys it can apply, each with its patch schema; empty means capabilities do not apply |
//! | `recreate` | `replace`: a new unit on the same volumes. `restart`: the same unit restarted |
//! | `volumes` | Whether workspace and home outlive a unit |
//! | `mounts` | How it attaches mounts ([`MountPlacement`]) |
//! | `private_views` | Whether it can put an environment's own changes on top of a read-only mount |
//! | `storages` | Whether it makes owned volumes or directories on a named machine storage |
//! | `suspend` | Whether it can hold a unit's processes still, so that files do not change while an environment is moved |
//! | `sources` | Where a unit's file system may come from; a host reports those it offers |
//! | `imports` | What a spec can be proposed from |
//! | `resource_limits` | Whether the limits are `enforced`, only `reported`, or `none` |
//! | `process_scope` | `all`: everything inside the unit is the environment's. `started`: only what the environment's connector started |
//! | `runtime_classes` | The values the host accepts for a `runtime` member of a spec |
//! | `runtimes` | Each of those as a class: its boundary and what holds differently under it ([`crate::isolation`]) |
//! | `default_runtime` | The class a unit is made under when its spec names none |
//!
//! A member that is absent means the driver does not claim it.

use std::collections::BTreeMap;

use orch_plugin_contract::Extensions;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::imports::ImportDef;
use crate::mounts::MountPlacement;
use crate::network::EgressMode;
use crate::runtime::RuntimeKind;
use crate::sources::SourceKind;

/// Whether the driver itself confines the agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Confinement {
    /// A container, a microVM: the driver holds the agent in, and the harness needs no sandbox of its own.
    Isolated,
    /// The driver confines nothing: the harness's own sandbox is all that holds the agent.
    #[default]
    None,
}

/// The isolation boundary declared by the hosting program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfinementBoundary {
    /// The physical or virtual machine is locally declared dedicated to Orch.
    Machine,
}

/// How an environment becomes available and is taken down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    /// A driver creates, starts and stops a unit.
    #[default]
    Unit,
    /// The connector attaches and detaches a directory without owning its other processes.
    Attach,
}

/// Whether a runtime can carry out an operation, with the actual constraint when it cannot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct OperationSupport {
    /// Whether the implementation has the required process boundary.
    pub supported: bool,
    /// Why it cannot run here, rather than a rule inferred from placement.
    pub reason: Option<String>,
}

impl OperationSupport {
    /// A declaration with the reason preserved for API and screen readers.
    #[must_use]
    pub fn available(supported: bool, reason: impl Into<String>) -> Self {
        Self {
            supported,
            reason: (!supported).then(|| reason.into()),
        }
    }

    /// Whether the driver made no claim at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.supported && self.reason.is_none()
    }

    /// Commands need a process group so a timeout stops every descendant.
    #[must_use]
    pub fn process_group_commands(supported: bool) -> Self {
        Self {
            supported,
            reason: (!supported)
                .then(|| "this platform cannot enforce a command's process group".into()),
        }
    }
}

/// What a recreate does to the unit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Recreate {
    /// A new unit on the same volumes.
    Replace,
    /// The same unit, restarted.
    #[default]
    Restart,
}

/// How far the unit's limits are enforced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLimits {
    /// The unit cannot use more.
    Enforced,
    /// Use is measured and reported, and nothing stops it.
    Reported,
    /// Neither.
    #[default]
    None,
}

/// Support for each resource separately; a process scope does not imply a disk quota.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ResourceLimitSupport {
    /// Why a resource is only declared or unavailable, keyed by the resource's ordinary member name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reasons: BTreeMap<String, String>,
    /// Processor time.
    pub cpu: ResourceLimits,
    /// Resident memory.
    pub memory: ResourceLimits,
    /// Number of processes.
    pub pids: ResourceLimits,
    /// Persistent disk usage; reported unless a real quota is available.
    pub disk: ResourceLimits,
}

impl ResourceLimitSupport {
    /// The process boundary's support, with disk support declared independently.
    #[must_use]
    pub const fn processes(processes: ResourceLimits, disk: ResourceLimits) -> Self {
        Self {
            reasons: BTreeMap::new(),
            cpu: processes,
            memory: processes,
            pids: processes,
            disk,
        }
    }
}

/// Which processes of a unit are the environment's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessScope {
    /// Everything inside the unit except the connector and its own children that are executors or MCP hosts.
    All,
    /// Only the descendants of processes started by an executor, a harness or a shell for the environment,
    /// tracked by process group or job object.
    #[default]
    Started,
}

/// What this driver makes when data is made by Orch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MadeDataKind {
    /// A persistent volume.
    Volume,
    /// A new directory owned by Orch.
    Directory,
}

/// Supported workspace/home storage choices, based on this driver's actual ability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EnvironmentDataStorage {
    /// None means that the driver has not offered making storage.
    pub made_by_orch: Option<MadeDataKind>,
    /// Whether existing directories can be bound or used with the unit's identity checked.
    pub existing_directory: bool,
    /// Why an existing directory cannot be selected here.
    pub reason: Option<String>,
}

impl EnvironmentDataStorage {
    /// Older drivers make no promise about either choice.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.made_by_orch.is_none() && !self.existing_directory && self.reason.is_none()
    }
}

/// Everything a driver declares about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LiveChanges {
    /// Gateway-given capabilities apply immediately without a driver patch or a process interruption.
    pub gateway_given: Vec<String>,
    /// CPU, memory and process limits can change without stopping the unit; fixed disk quotas still replace it.
    pub resources: bool,
    /// Placed mounts can change safely without stopping the unit and are replayed after start.
    pub mounts: bool,
    /// Capability keys this host can give without replacement; withdrawal may restart the same unit.
    pub capabilities: Vec<String>,
}

/// Everything a driver declares about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DriverDescriptor {
    /// Its name, which a template names and which a capability's patch is keyed by.
    pub name: String,
    /// Its version.
    pub version: String,
    /// What it is called.
    pub title: String,
    /// Schema for this driver's entry in the machine policy configuration map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_schema: Option<Json>,
    /// Resource observations belong to this scope; absence makes no remote-pool claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_scope: Option<crate::host::HostScope>,
    /// How builds reach a unit; absence does not claim a delivery mechanism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_delivery: Option<crate::host::ToolDelivery>,
    /// How units reach the gateway; absence does not claim a network route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_access: Option<crate::host::GatewayAccess>,
    /// A declared driver-owned consistent export; absence requires the unit to be stopped or suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_export_consistency: Option<crate::host::VolumeExportConsistency>,
    /// Whether this host can publish node listeners for this driver's units.
    #[serde(default, skip_serializing_if = "OperationSupport::is_empty")]
    pub host_ports: OperationSupport,
    /// Whether kept installed files can travel between hosts as an owned artifact.
    #[serde(default, skip_serializing_if = "OperationSupport::is_empty")]
    pub kept_file_transfer: OperationSupport,
    /// Whether templates launch programs as local machine accounts, subject to process ceilings.
    #[serde(default, skip_serializing_if = "is_false")]
    pub local_accounts: bool,
    /// Actual reasons for unavailable optional features, keyed by their ordinary descriptor member.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
    /// The schema of the spec it accepts, for each pool it makes units for. It makes units only for the
    /// pools it has a schema for.
    #[serde(default)]
    pub spec_schemas: BTreeMap<RuntimeKind, Json>,
    /// Whether it confines the agent.
    #[serde(default)]
    pub confinement: Confinement,
    /// An explicit machine boundary; absent leaves the driver's own confinement declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confinement_by: Option<ConfinementBoundary>,
    /// Whether availability means starting a unit or attaching a directory.
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// Whether after-start commands can stop their entire process group on failure or timeout.
    #[serde(default)]
    pub commands_after_start: OperationSupport,
    /// The egress modes this host can enforce: `restricted` only where the host's own isolation and proxy
    /// probe passed. Absent: the driver claims none, not even `open`.
    #[serde(default)]
    pub network_egress: Vec<EgressMode>,
    /// The capability keys it can apply, each with the schema of this driver's patch for the key.
    #[serde(default)]
    pub capabilities: BTreeMap<String, Json>,
    /// What a recreate does.
    #[serde(default)]
    pub recreate: Recreate,
    /// The changes this host actually applies to its running units.
    #[serde(default)]
    pub live_changes: LiveChanges,
    /// Whether a same-image replacement keeps the unit's own files outside persistent volumes.
    #[serde(default)]
    pub recreate_preserves_files: bool,
    /// Whether workspace and home outlive a unit.
    #[serde(default)]
    pub volumes: bool,
    /// Workspace/home choices; placement does not imply support for either choice.
    #[serde(default, skip_serializing_if = "EnvironmentDataStorage::is_empty")]
    pub data_storage: EnvironmentDataStorage,
    /// How it attaches mounts.
    #[serde(default = "no_mounts")]
    pub mounts: MountPlacement,
    /// Whether it can put an environment's own changes on top of a read-only mount; whether this host can
    /// is reported per storage.
    #[serde(default)]
    pub private_views: bool,
    /// Whether it makes owned volumes or directories on named machine storages.
    #[serde(default)]
    pub storages: bool,
    /// Whether it can hold a unit's processes still and let them go again.
    #[serde(default)]
    pub suspend: bool,
    /// Where a unit's file system may come from; empty means the driver has no such thing.
    #[serde(default)]
    pub sources: Vec<SourceKind>,
    /// What a spec can be proposed from.
    #[serde(default)]
    pub imports: Vec<ImportDef>,
    /// Whether the limits are enforced.
    #[serde(default)]
    pub resource_limits: ResourceLimitSupport,
    /// Which processes of a unit are the environment's.
    #[serde(default)]
    pub process_scope: ProcessScope,
    /// The values the host accepts for a `runtime` member of a spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_classes: Option<Vec<String>>,
    /// Each runtime as a class: what holds a unit in, and every member of this declaration that holds differently
    /// under it. Empty for a driver whose units all run alike.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtimes: Vec<crate::isolation::RuntimeClass>,
    /// The class a unit is made under when its spec names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_runtime: Option<String>,
    /// Members that belong to the implementation, kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

fn no_mounts() -> MountPlacement {
    MountPlacement::None
}

fn is_false(value: &bool) -> bool {
    !value
}

impl DriverDescriptor {
    /// Optional operations with the declared reason for each unavailable choice.
    #[must_use]
    pub fn availability(&self) -> BTreeMap<String, OperationSupport> {
        let features = [
            ("host_paths", self.data_storage.existing_directory),
            ("build", self.has_source(SourceKind::Build)),
            ("archive", self.has_source(SourceKind::Archive)),
            ("kept_files", self.keeps_files_on_recreate()),
            ("private_views", self.private_views),
            ("suspend", self.suspend),
            ("live_mounts", self.live_changes.mounts),
            ("tools", self.tool_delivery.is_some()),
            ("gateway", self.gateway_access.is_some()),
        ];
        let mut answer: BTreeMap<_, _> = features
            .into_iter()
            .map(|(feature, supported)| {
                let reason = self
                    .unavailable
                    .get(feature)
                    .cloned()
                    .unwrap_or_else(|| format!("this driver does not offer {feature}"));
                (
                    feature.into(),
                    OperationSupport::available(supported, reason),
                )
            })
            .collect();
        answer.insert(
            "host_ports".into(),
            if self.host_ports.is_empty() {
                OperationSupport::available(false, "this driver does not offer host listeners")
            } else {
                self.host_ports.clone()
            },
        );
        answer.insert(
            "kept_file_transfer".into(),
            if self.kept_file_transfer.is_empty() {
                OperationSupport::available(
                    false,
                    "this driver does not transfer kept installed files",
                )
            } else {
                self.kept_file_transfer.clone()
            },
        );
        answer
    }

    /// The actual services of a connector that attaches registered directories.
    #[must_use]
    pub fn registered_directories(version: &str, storage: EnvironmentDataStorage) -> Self {
        let mut extensions = Extensions::default();
        let _ = extensions.insert(
            "x_storage_roles",
            serde_json::json!(crate::volumes::DIRECTORY_STORAGE_ROLES),
        );
        let storages = storage.made_by_orch.is_some();
        Self {
            name: "directory".into(),
            version: version.into(),
            title: "Registered directories".into(),
            configuration_schema: None,
            host_scope: Some(crate::host::HostScope::Node),
            tool_delivery: Some(crate::host::ToolDelivery::HostDirectory),
            gateway_access: Some(crate::host::GatewayAccess::NodeAddress),
            volume_export_consistency: None,
            host_ports: OperationSupport::available(true, ""),
            kept_file_transfer: OperationSupport::default(),
            local_accounts: false,
            unavailable: BTreeMap::new(),
            spec_schemas: BTreeMap::new(),
            confinement: Confinement::None,
            confinement_by: None,
            lifecycle: Lifecycle::Attach,
            commands_after_start: OperationSupport::process_group_commands(cfg!(unix)),
            network_egress: vec![EgressMode::Open],
            capabilities: BTreeMap::new(),
            recreate: Recreate::Restart,
            live_changes: LiveChanges {
                gateway_given: crate::capabilities::GATEWAY_GIVEN
                    .iter()
                    .map(|key| (*key).into())
                    .collect(),
                ..LiveChanges::default()
            },
            recreate_preserves_files: true,
            volumes: false,
            data_storage: storage,
            mounts: MountPlacement::InPlace,
            private_views: false,
            storages,
            suspend: false,
            sources: Vec::new(),
            imports: Vec::new(),
            resource_limits: ResourceLimitSupport::processes(
                ResourceLimits::Reported,
                ResourceLimits::Reported,
            ),
            process_scope: ProcessScope::Started,
            runtime_classes: None,
            runtimes: Vec::new(),
            default_runtime: None,
            extensions,
        }
    }

    /// Only local machine configuration may supply this boundary; delegation does not change it.
    #[must_use]
    pub fn dedicated_machine(mut self, dedicated: bool) -> Self {
        if self.lifecycle == Lifecycle::Attach {
            self.confinement = if dedicated {
                Confinement::Isolated
            } else {
                Confinement::None
            };
            self.confinement_by = dedicated.then_some(ConfinementBoundary::Machine);
        }
        self
    }

    /// Whether this driver's recreate keeps all disk contents when the source image stays the same.
    #[must_use]
    pub fn keeps_files_on_recreate(&self) -> bool {
        self.recreate == Recreate::Restart || self.recreate_preserves_files
    }
    /// Whether the driver makes units for the pool `kind`: it has a schema for it.
    #[must_use]
    pub fn makes_units_for(&self, kind: RuntimeKind) -> bool {
        self.spec_schemas.contains_key(&kind)
    }

    /// The import of `format`, if the driver declares it.
    #[must_use]
    pub fn import(&self, format: &str) -> Option<&ImportDef> {
        self.imports.iter().find(|import| import.format == format)
    }

    /// Whether the driver can name `source` in a spec.
    #[must_use]
    pub fn has_source(&self, source: SourceKind) -> bool {
        self.sources.contains(&source)
    }

    /// Whether a template whose spec has the `runtime` member `runtime` can be scheduled on a host that
    /// reports this descriptor: when the spec has such a member and the descriptor lists classes, the value
    /// is listed. A spec without the member, or a driver without classes, is not restricted.
    #[must_use]
    pub fn accepts_runtime_class(&self, runtime: Option<&str>) -> bool {
        match (runtime, &self.runtime_classes) {
            (Some(wanted), Some(classes)) => classes.iter().any(|class| class == wanted),
            _ => true,
        }
    }

    /// What is wrong with the declaration, if anything: a driver that claims to attach mounts in place or
    /// where it places them but has nowhere to keep them; private views without mounts, suspension without
    /// volumes, storages without owned-data support, or a missing name or version. Every fault is returned.
    pub fn check(&self) -> Result<(), Vec<DescriptorFault>> {
        let mut faults = Vec::new();
        if self.name.is_empty() {
            faults.push(DescriptorFault::NoName);
        }
        if self.version.is_empty() {
            faults.push(DescriptorFault::NoVersion);
        }
        if self.lifecycle == Lifecycle::Unit && self.spec_schemas.is_empty() {
            faults.push(DescriptorFault::NoSchema);
        }
        for capability in &self.live_changes.capabilities {
            if !self.capabilities.contains_key(capability) {
                faults.push(DescriptorFault::UndeclaredLiveCapability(
                    capability.clone(),
                ));
            }
        }
        if self.private_views && self.mounts != MountPlacement::Placed {
            faults.push(DescriptorFault::PrivateViewsWithoutPlacedMounts);
        }
        if self.storages && !self.volumes && self.data_storage.made_by_orch.is_none() {
            faults.push(DescriptorFault::StoragesWithoutVolumes);
        }
        if self.recreate == Recreate::Replace && !self.volumes {
            faults.push(DescriptorFault::ReplaceWithoutVolumes);
        }
        if let Some(classes) = &self.runtime_classes
            && classes.is_empty()
        {
            faults.push(DescriptorFault::EmptyRuntimeClasses);
        }
        for (position, class) in self.runtimes.iter().enumerate() {
            if class.name.is_empty()
                || self.runtimes[..position]
                    .iter()
                    .any(|before| before.name == class.name)
            {
                faults.push(DescriptorFault::DuplicateRuntime(class.name.clone()));
            }
        }
        if let Some(default) = &self.default_runtime
            && !self.runtimes.iter().any(|class| &class.name == default)
        {
            faults.push(DescriptorFault::UnknownDefaultRuntime(default.clone()));
        }
        for (position, import) in self.imports.iter().enumerate() {
            if self.imports[..position]
                .iter()
                .any(|before| before.format == import.format)
            {
                faults.push(DescriptorFault::DuplicateImport(import.format.clone()));
            }
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

/// What is wrong with a driver's declaration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorFault {
    /// A live capability must also be one whose patch this driver declares.
    #[error("live capability {0} is not declared by this driver")]
    UndeclaredLiveCapability(String),
    /// It has no name.
    #[error("a driver has a name")]
    NoName,
    /// It has no version.
    #[error("a driver has a version")]
    NoVersion,
    /// It makes units for no pool.
    #[error("a driver has a spec schema for at least one pool")]
    NoSchema,
    /// A private view is laid over a mount the driver places; it cannot be made over one used in place.
    #[error("private views are claimed and mounts are not placed")]
    PrivateViewsWithoutPlacedMounts,
    /// Named storages need either volume support or declared owned-directory data.
    #[error("storages are claimed without support for owned data")]
    StoragesWithoutVolumes,
    /// A new unit on the same volumes needs volumes.
    #[error("recreate is replace and volumes are not claimed")]
    ReplaceWithoutVolumes,
    /// A list of runtime classes names at least one, or is left out.
    #[error("runtime_classes lists at least one class, or is left out")]
    EmptyRuntimeClasses,
    /// Two runtime classes have one name, or one has none.
    #[error("the runtime class {0:?} is declared twice or has no name")]
    DuplicateRuntime(String),
    /// The default runtime is not one of the classes.
    #[error("the default runtime {0:?} is not a declared class")]
    UnknownDefaultRuntime(String),
    /// Two imports have one format.
    #[error("the import format {0:?} is declared twice")]
    DuplicateImport(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_dedicated_machine_supplies_the_directory_isolation_boundary_as_a_fact() {
        let directory =
            DriverDescriptor::registered_directories("1", EnvironmentDataStorage::default());
        let dedicated = directory.clone().dedicated_machine(true);
        let fact = serde_json::to_value(&dedicated).unwrap();
        assert_eq!(fact["confinement"], json!("isolated"));
        assert_eq!(fact["confinement_by"], json!("machine"));
        assert_eq!(dedicated.clone().dedicated_machine(false), directory);
        assert_eq!(dedicated.check(), Ok(()));
    }

    #[test]
    fn a_registered_directory_declares_its_real_boundaries_without_a_unit_recipe() {
        let descriptor = DriverDescriptor::registered_directories(
            "1",
            EnvironmentDataStorage {
                made_by_orch: Some(MadeDataKind::Directory),
                existing_directory: true,
                reason: None,
            },
        );
        assert_eq!(descriptor.check(), Ok(()));
        assert_eq!(descriptor.confinement, Confinement::None);
        assert_eq!(descriptor.lifecycle, Lifecycle::Attach);
        assert!(descriptor.storages);
        assert_eq!(
            descriptor.extensions.get("x_storage_roles"),
            Some(&serde_json::json!(crate::volumes::DIRECTORY_STORAGE_ROLES))
        );
        assert_eq!(descriptor.mounts, MountPlacement::InPlace);
        assert_eq!(descriptor.process_scope, ProcessScope::Started);
        assert!(descriptor.capabilities.is_empty() && !descriptor.private_views);
        assert_eq!(descriptor.network_egress, vec![EgressMode::Open]);
        assert!(!descriptor.makes_units_for(RuntimeKind::Environment));
    }

    /// A descriptor of the first built-in driver, as it is published.
    fn docker() -> Json {
        json!({
          "name": "docker", "version": "1", "title": "Docker containers",
          "spec_schemas": { "environment": { "type": "object" }, "provider": { "type": "object" } },
          "confinement": "isolated",
          "lifecycle": "unit",
          "commands_after_start": {"supported":true,"reason":null},
          "network_egress": ["open", "restricted"],
          "capabilities": { "kvm": { "type": "object" }, "docker_socket": { "type": "object" } },
          "recreate": "replace",
          "recreate_preserves_files": true,
          "live_changes": {"resources":true,"mounts":true,"capabilities":["kvm","docker_socket"],"gateway_given":["port_place","port_host","port_link"]},
          "volumes": true,
          "mounts": "placed",
          "private_views": true,
          "storages": true,
          "suspend": true,
          "sources": ["image", "build", "archive"],
          "imports": [ { "format": "dockerfile", "title": "A Dockerfile", "fields": [] } ],
          "resource_limits": {"cpu":"enforced","memory":"enforced","pids":"enforced","disk":"reported"},
          "process_scope": "all",
          "runtime_classes": ["runc", "runsc"]
        })
    }

    #[test]
    fn the_descriptor_of_the_first_driver_reads_and_writes_as_the_spec_shows_it() {
        let read: DriverDescriptor = serde_json::from_value(docker()).unwrap();
        assert_eq!(read.confinement, Confinement::Isolated);
        assert_eq!(read.mounts, MountPlacement::Placed);
        assert!(
            read.recreate_preserves_files
                && read.live_changes.resources
                && read.live_changes.mounts
        );
        assert!(
            read.live_changes
                .capabilities
                .iter()
                .any(|key| key == "kvm")
        );
        assert!(read.makes_units_for(RuntimeKind::Provider));
        assert!(read.has_source(SourceKind::Build));
        assert_eq!(read.check(), Ok(()));
        assert_eq!(serde_json::to_value(&read).unwrap(), docker());
    }

    #[test]
    fn a_driver_that_claims_the_least_is_a_driver_that_confines_nothing_and_offers_nothing() {
        let read: DriverDescriptor = serde_json::from_value(json!({
            "name": "host", "version": "1", "title": "Processes of this host",
            "spec_schemas": { "environment": {} }, "confinement": "none", "recreate": "restart", "process_scope": "started"
        }))
        .unwrap();
        assert_eq!(read.confinement, Confinement::None);
        assert!(read.capabilities.is_empty() && read.sources.is_empty() && read.imports.is_empty());
        assert!(!read.suspend && !read.private_views && !read.storages && !read.volumes);
        assert_eq!(read.mounts, MountPlacement::None);
        assert_eq!(read.process_scope, ProcessScope::Started);
        assert_eq!(read.resource_limits, ResourceLimitSupport::default());
        assert!(
            !read.makes_units_for(RuntimeKind::Provider),
            "it makes units only for the pools it has a schema for"
        );
        assert_eq!(read.check(), Ok(()));
    }

    #[test]
    fn a_template_is_scheduled_on_a_host_whose_driver_lists_the_runtime_it_names() {
        let read: DriverDescriptor = serde_json::from_value(docker()).unwrap();
        assert!(read.accepts_runtime_class(Some("runsc")));
        assert!(!read.accepts_runtime_class(Some("kata")));
        assert!(
            read.accepts_runtime_class(None),
            "a spec without a runtime member is not restricted"
        );
        let mut unrestricted = read;
        unrestricted.runtime_classes = None;
        assert!(unrestricted.accepts_runtime_class(Some("kata")));
    }

    #[test]
    fn members_of_the_implementations_own_are_kept_and_unknown_ones_are_ignored() {
        let mut with = docker();
        with["x_socket"] = json!("/var/run/docker.sock");
        with["future_member"] = json!(1);
        let read: DriverDescriptor = serde_json::from_value(with).unwrap();
        assert_eq!(
            read.extensions.get("x_socket"),
            Some(&json!("/var/run/docker.sock"))
        );
        assert!(
            serde_json::to_value(&read)
                .unwrap()
                .get("future_member")
                .is_none()
        );
        assert!(read.import("dockerfile").is_some() && read.import("archive").is_none());
    }

    #[test]
    fn a_declaration_that_cannot_hold_together_is_refused() {
        let mut broken: DriverDescriptor = serde_json::from_value(docker()).unwrap();
        broken.name.clear();
        broken.mounts = MountPlacement::InPlace;
        broken.volumes = false;
        broken.runtime_classes = Some(vec![]);
        broken.imports.push(broken.imports[0].clone());
        broken.spec_schemas.clear();
        let faults = broken.check().unwrap_err();
        for expected in [
            DescriptorFault::NoName,
            DescriptorFault::NoSchema,
            DescriptorFault::PrivateViewsWithoutPlacedMounts,
            DescriptorFault::StoragesWithoutVolumes,
            DescriptorFault::ReplaceWithoutVolumes,
            DescriptorFault::EmptyRuntimeClasses,
            DescriptorFault::DuplicateImport("dockerfile".into()),
        ] {
            assert!(faults.contains(&expected), "{expected:?} in {faults:?}");
        }
    }
}
