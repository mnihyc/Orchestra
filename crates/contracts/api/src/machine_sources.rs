//! Native physical Machine subjects and their optional individual projection sources.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Stored native subject and independently protected response roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual issuer Account of a private enrollment recovery receipt.
    Issuer,
    /// The current physical page or its Host facade, with Machine as the business kind.
    Governor,
    /// Actual registered Provider, Agent and Environment references.
    Registered,
    /// Physical machines actually supplying an individual Environment readiness result.
    ReadinessMachines,
    /// Permission group labels selected by the native access renderer.
    Groups,
    /// Native organization context labels.
    Orgs,
    /// Native project context labels.
    Projects,
    /// Native topic context labels.
    Topics,
    /// Native thread context labels.
    Threads,
}

pub const SUBJECT: TargetResolver = TargetResolver::Resolved("machine_subject");
pub const REGISTERED: &str = "machine_reply_sources";
/// Actual physical readiness sources, separate from the requested machine.
pub const READINESS_MACHINES: &str = "machine_readiness_machines";
pub const GROUPS: &str = "machine_reply_groups";
pub const NODES: [&str; 4] = [
    "machine_reply_orgs",
    "machine_reply_projects",
    "machine_reply_topics",
    "machine_reply_threads",
];

/// The stored subject is required; existing content remains optional same-operation Read.
pub const UPDATE_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        SUBJECT,
    ),
    FacetSpec::response_content(&[KeyTargetKind::Object(ObjectKind::Machine)], SUBJECT),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Agent),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::ResolvedCollection(REGISTERED),
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

/// One native stored-source row; optional sources are completed only by their real renderer.
pub const UPDATE: Declaration<Source> = Declaration {
    operation: "MachineUpdate",
    bindings: &[
        Binding {
            source: Source::Governor,
            target: SUBJECT,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Governor,
            target: SUBJECT,
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Registered,
            target: TargetResolver::ResolvedCollection(REGISTERED),
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

/// Withdrawal uses the same actual physical/facade subject and native response roles.
pub const REVOKE_FACETS: [FacetSpec; 8] = UPDATE_FACETS;

/// One stored subject row; withdrawal retains its own operation rather than borrowing rename.
pub const REVOKE: Declaration<Source> = Declaration {
    operation: "MachineRevoke",
    bindings: UPDATE.bindings,
};

/// A native Machine list supplies a real possibly empty selected result collection.
pub const LIST_FACETS: [FacetSpec; 7] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection("results"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Agent),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::ResolvedCollection(REGISTERED),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection(GROUPS),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(NODES[0]),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection(NODES[1]),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection(NODES[2]),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection(NODES[3]),
    ),
];

/// One native list row, independent of individual readiness work.
pub const LIST: Declaration<Source> = Declaration {
    operation: "MachineList",
    bindings: &[
        Binding {
            source: Source::Governor,
            target: TargetResolver::ResolvedCollection("results"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Registered,
            target: TargetResolver::ResolvedCollection(REGISTERED),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Groups,
            target: TargetResolver::ResolvedCollection(GROUPS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection(NODES[0]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection(NODES[1]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection(NODES[2]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection(NODES[3]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Individual native metadata keeps the readiness that is absent from the inventory.
pub const GET_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        SUBJECT,
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Agent),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::ResolvedCollection(REGISTERED),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection(READINESS_MACHINES),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection(GROUPS),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(NODES[0]),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection(NODES[1]),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection(NODES[2]),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection(NODES[3]),
    ),
];

/// One real stored Machine plus its independently licensed native projection sources.
pub const GET: Declaration<Source> = Declaration {
    operation: "MachineGet",
    bindings: &[
        Binding {
            source: Source::Governor,
            target: SUBJECT,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Registered,
            target: TargetResolver::ResolvedCollection(REGISTERED),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReadinessMachines,
            target: TargetResolver::ResolvedCollection(READINESS_MACHINES),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Groups,
            target: TargetResolver::ResolvedCollection(GROUPS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection(NODES[0]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection(NODES[1]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection(NODES[2]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection(NODES[3]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Only resources actually selected by this native progress projection.
pub const ENROLLMENT_RESOURCES: &str = "enrollment_selected_resources";
/// Issuer recovery remains Account data before an actual Machine or Host exists.
pub const ENROLLMENT_FACETS: [FacetSpec; 9] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
        TargetResolver::CurrentAccount,
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Host),
        ],
        TargetResolver::ResolvedCollection(ENROLLMENT_RESOURCES),
    ),
    GET_FACETS[1],
    GET_FACETS[2],
    GET_FACETS[3],
    GET_FACETS[4],
    GET_FACETS[5],
    GET_FACETS[6],
    GET_FACETS[7],
];
/// Native pending and completed progress share one issuer and explicit selected cardinality.
pub const ENROLLMENT: Declaration<Source> = Declaration {
    operation: "MachineEnrollmentStatusGet",
    bindings: &[
        Binding {
            source: Source::Issuer,
            target: TargetResolver::CurrentAccount,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Governor,
            target: TargetResolver::ResolvedCollection(ENROLLMENT_RESOURCES),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        GET.bindings[1],
        GET.bindings[2],
        GET.bindings[3],
        GET.bindings[4],
        GET.bindings[5],
        GET.bindings[6],
        GET.bindings[7],
    ],
};
