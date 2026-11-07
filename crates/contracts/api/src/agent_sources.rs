//! Native organization profile creation and its optional individual response sources.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Actual native creation home and completed profile projection roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual physical machine or its native Host facade supplying the registration.
    Machine,
    /// The actual existing organization where creation is authorized.
    Home,
    /// The real stored profile produced by the completed native effect.
    Created,
    /// Native provider and physical reference payloads.
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
pub const REFERENCES: &str = "agent_created_sources";
/// The native permission-group payload collection.
pub const GROUPS: &str = "agent_created_groups";
/// Native protected node-label roles, in hierarchy kind order.
pub const NODES: [&str; 4] = [
    "agent_created_orgs",
    "agent_created_projects",
    "agent_created_topics",
    "agent_created_threads",
];

/// Creation keeps the existing management act; content remains same-operation Read.
pub const CREATE_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Agent)],
        TargetResolver::PermissionObject,
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Agent)],
        TargetResolver::Resolved("created_resource"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Machine),
        ],
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
pub const CREATE: Declaration<Source> = Declaration {
    operation: "OrgAgentCreate",
    bindings: &[
        Binding {
            source: Source::Home,
            target: TargetResolver::PermissionObject,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Created,
            target: TargetResolver::Resolved("created_resource"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::References,
            target: TargetResolver::ResolvedCollection(REFERENCES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Groups,
            target: TargetResolver::ResolvedCollection(GROUPS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection(NODES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection(NODES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection(NODES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection(NODES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Stored-machine registration keeps its own Write and its actual Agent creation home.
pub const MACHINE_CREATE_FACETS: [FacetSpec; 9] = [
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::Resolved("registration_machine"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Agent)],
        TargetResolver::Resolved("registration_home"),
    ),
    CREATE_FACETS[1],
    CREATE_FACETS[2],
    CREATE_FACETS[3],
    CREATE_FACETS[4],
    CREATE_FACETS[5],
    CREATE_FACETS[6],
    CREATE_FACETS[7],
];
/// The existing stored facts and ordinary registration producer, without a remote refresh.
pub const MACHINE_CREATE: Declaration<Source> = Declaration {
    operation: "MachineAgentCreate",
    bindings: &[
        Binding {
            source: Source::Machine,
            target: TargetResolver::Resolved("registration_machine"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Home,
            target: TargetResolver::Resolved("registration_home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Created,
            target: TargetResolver::Resolved("created_resource"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::References,
            target: TargetResolver::ResolvedCollection(REFERENCES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Groups,
            target: TargetResolver::ResolvedCollection(GROUPS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection(NODES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection(NODES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection(NODES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection(NODES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
