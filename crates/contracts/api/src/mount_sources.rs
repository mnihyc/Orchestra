//! Native Mount usage sources connected to their canonical operation facets.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Primary stored subject and actual native selected usage sources, including counted rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseSource {
    /// The actual live Mount that the native gate opens.
    Mount,
    /// Node settings that enable this Mount, with explicit native node kinds.
    Scopes,
    /// Enabled Environment sources supplying usage metadata or the native hidden count.
    Environments,
}

const PRIMARY: TargetResolver = TargetResolver::TypedPath {
    parameter: "id",
    kind: KeyTargetKind::Object(ObjectKind::Mount),
};

/// The native usage page reads each selected source under its own operation.
pub const USE_FACETS: [FacetSpec; 3] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        PRIMARY,
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::ResolvedCollection("mount_use_scopes"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("mount_use_environments"),
    ),
];

/// One selected-row declaration, shared by the native mounting adapter and producer.
pub const USES: Declaration<UseSource> = Declaration {
    operation: "MountUsesGet",
    bindings: &[
        Binding {
            source: UseSource::Mount,
            target: PRIMARY,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: UseSource::Scopes,
            target: TargetResolver::ResolvedCollection("mount_use_scopes"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UseSource::Environments,
            target: TargetResolver::ResolvedCollection("mount_use_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native physical-machine storage snapshot roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineSource {
    /// Actual personal Machine or Host permission subject; business family remains Machine.
    Machine,
    /// Mount rows actually selected as locations or available choices.
    Mounts,
    /// Environment references actually attached or using a configured directory root.
    Environments,
    /// Actual organization components of emitted native Mount addresses.
    AddressOrgs,
    /// Project components of resource homes.
    AddressProjects,
    /// Topic components of resource homes.
    AddressTopics,
    /// Thread components of resource homes.
    AddressThreads,
}
/// Actual selected Mount collection.
pub const MACHINE_MOUNTS: &str = "machine_selected_mounts";
/// Actual selected Environment collection.
pub const MACHINE_ENVIRONMENTS: &str = "machine_selected_environments";
/// Native protected Mount address components.
pub const MACHINE_ADDRESS_ORGS: &str = "machine_mount_address_orgs";
const MACHINE: TargetResolver = TargetResolver::Resolved("machine");
/// Physical machine mount data and independently readable selected references.
pub const MACHINE_MOUNT_FACETS: [FacetSpec; 7] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        MACHINE,
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        TargetResolver::ResolvedCollection(MACHINE_MOUNTS),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection(MACHINE_ENVIRONMENTS),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(MACHINE_ADDRESS_ORGS),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("machine_mount_address_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("machine_mount_address_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("machine_mount_address_threads"),
    ),
];
/// Physical machine directory facts and actual selected Environment uses.
pub const MACHINE_DATA_FACETS: [FacetSpec; 2] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        MACHINE,
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection(MACHINE_ENVIRONMENTS),
    ),
];
/// Native host-side mount snapshot declaration.
pub const MACHINE_MOUNT_READ: Declaration<MachineSource> = Declaration {
    operation: "MachineMountList",
    bindings: &[
        Binding {
            source: MachineSource::Machine,
            target: MACHINE,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: MachineSource::Mounts,
            target: TargetResolver::ResolvedCollection(MACHINE_MOUNTS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MachineSource::Environments,
            target: TargetResolver::ResolvedCollection(MACHINE_ENVIRONMENTS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MachineSource::AddressOrgs,
            target: TargetResolver::ResolvedCollection(MACHINE_ADDRESS_ORGS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MachineSource::AddressProjects,
            target: TargetResolver::ResolvedCollection("machine_mount_address_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MachineSource::AddressTopics,
            target: TargetResolver::ResolvedCollection("machine_mount_address_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MachineSource::AddressThreads,
            target: TargetResolver::ResolvedCollection("machine_mount_address_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
/// Native host-side environment directory snapshot declaration.
pub const MACHINE_DATA_READ: Declaration<MachineSource> = Declaration {
    operation: "MachineEnvironmentDataGet",
    bindings: &[
        Binding {
            source: MachineSource::Machine,
            target: MACHINE,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: MachineSource::Environments,
            target: TargetResolver::ResolvedCollection(MACHINE_ENVIRONMENTS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Actual native creation home and completed mount projection roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationSource {
    /// The actual existing organization or System namespace where creation is authorized.
    Home,
    /// The real stored mount produced by the completed native effect.
    Created,
    /// Native physical reference payloads.
    References,
    /// Group payloads in native permission metadata.
    Groups,
    /// Protected organization labels.
    Orgs,
    /// Protected project labels.
    Projects,
    /// Protected topic labels.
    Topics,
    /// Protected thread labels.
    Threads,
}

/// The native reference payload collection.
pub const REFERENCES: &str = "mount_created_sources";
/// The native permission-group payload collection.
pub const GROUPS: &str = "mount_created_groups";
/// Native protected node-label roles, in hierarchy kind order.
pub const NODES: [&str; 4] = [
    "mount_created_orgs",
    "mount_created_projects",
    "mount_created_topics",
    "mount_created_threads",
];

/// Creation keeps the existing management act; content remains same-operation Read.
pub const CREATE_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        TargetResolver::Resolved("home"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        TargetResolver::Resolved("created_mount"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection(REFERENCES),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection(GROUPS),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(NODES[0]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection(NODES[1]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection(NODES[2]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection(NODES[3]),
    ),
];

/// One native E declaration, also consumed by the stored completed-resource reader.
pub const CREATE: Declaration<CreationSource> = Declaration {
    operation: "MountCreate",
    bindings: &[
        Binding {
            source: CreationSource::Home,
            target: TargetResolver::Resolved("home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: CreationSource::Created,
            target: TargetResolver::Resolved("created_mount"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: CreationSource::References,
            target: TargetResolver::ResolvedCollection(REFERENCES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: CreationSource::Groups,
            target: TargetResolver::ResolvedCollection(GROUPS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: CreationSource::Orgs,
            target: TargetResolver::ResolvedCollection(NODES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: CreationSource::Projects,
            target: TargetResolver::ResolvedCollection(NODES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: CreationSource::Topics,
            target: TargetResolver::ResolvedCollection(NODES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: CreationSource::Threads,
            target: TargetResolver::ResolvedCollection(NODES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
