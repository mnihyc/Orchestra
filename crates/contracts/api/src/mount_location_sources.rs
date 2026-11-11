//! Native location effects and independently readable completed mount inventory.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::{
    key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Mount,
    Machine,
    References,
    Groups,
    Orgs,
    Projects,
    Topics,
    Threads,
}
pub const REFERENCES: &str = "mount_location_sources";
pub const GROUPS: &str = "mount_location_groups";
pub const NODES: [&str; 4] = [
    "mount_location_orgs",
    "mount_location_projects",
    "mount_location_topics",
    "mount_location_threads",
];
pub const SET_FACETS: [FacetSpec; 9] = [
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::Path("machine_id"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        TargetResolver::Path("id"),
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
pub const SET: Declaration<Source> = Declaration {
    operation: "MountLocationSet",
    bindings: &[
        Binding {
            source: Source::Mount,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Machine,
            target: TargetResolver::Path("machine_id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Mount,
            target: TargetResolver::Path("id"),
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

/// The stored active/kept location supplies its real machine.
pub const REMOVE_BINDINGS: [Binding<Source>; 2] = [
    Binding {
        source: Source::Mount,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Machine,
        target: TargetResolver::Resolved("location_machine"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
];
pub const DELETE: Declaration<Source> = Declaration {
    operation: "MountLocationDelete",
    bindings: &REMOVE_BINDINGS,
};
pub const PURGE: Declaration<Source> = Declaration {
    operation: "MountKeptPurge",
    bindings: &REMOVE_BINDINGS,
};
