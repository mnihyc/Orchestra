//! Native resource library subjects and protected people-address sources.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// The native requested resource, owned parent and selected protected address domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual typed resource's native permission subject.
    Resource,
    /// The real environment whose own MCP server supplies the requested figures.
    OwnedEnvironments,
    /// Named organization addresses of selected public UserRefs.
    Orgs,
    /// Named project addresses of selected public UserRefs.
    Projects,
    /// Named topic addresses of selected public UserRefs.
    Topics,
    /// Named thread addresses of selected public UserRefs.
    Threads,
}

/// Existing business resource vocabulary of the native library service.
pub const RESOURCES: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Provider),
    KeyTargetKind::Object(ObjectKind::Agent),
    KeyTargetKind::Object(ObjectKind::Environment),
    KeyTargetKind::Object(ObjectKind::Host),
    KeyTargetKind::Object(ObjectKind::Machine),
    KeyTargetKind::Object(ObjectKind::Template),
    KeyTargetKind::Object(ObjectKind::McpServer),
    KeyTargetKind::Object(ObjectKind::Mount),
    KeyTargetKind::Object(ObjectKind::Skill),
];

/// Native address-domain roles in hierarchy kind order.
pub const NODES: [&str; 4] = [
    "library_people_orgs",
    "library_people_projects",
    "library_people_topics",
    "library_people_threads",
];

/// One resource licenses its native people figures; protected addresses keep independent Read.
pub const PEOPLE_FACETS: [FacetSpec; 6] = [
    FacetSpec::required(
        KeyAction::Read,
        RESOURCES,
        TargetResolver::Resolved("requested_resources"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("library_owned_environments"),
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

/// Exact native body-selected resource and selected people-address sources.
pub const PEOPLE: Declaration<Source> = Declaration {
    operation: "ResourceLibraryPeopleRead",
    bindings: &[
        Binding {
            source: Source::Resource,
            target: TargetResolver::Resolved("requested_resources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OwnedEnvironments,
            target: TargetResolver::ResolvedCollection("library_owned_environments"),
            usage: FacetUsage::Required,
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
