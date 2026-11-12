//! Actual requested resource figures and their native named dependency sources.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Source roles of the shared native figures/impact producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual requested resource subjects, with explicit native cardinality.
    Requested,
    /// Real environment parents for requested environment-owned MCP servers.
    OwnedEnvironments,
    /// Actual native objects licensing named dependencies, work and thread status.
    Names,
}

/// The native relationship vocabulary: existing resource pages and hierarchy nodes.
pub const NAMED_KINDS: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Provider),
    KeyTargetKind::Object(ObjectKind::Agent),
    KeyTargetKind::Object(ObjectKind::Environment),
    KeyTargetKind::Object(ObjectKind::Host),
    KeyTargetKind::Object(ObjectKind::Machine),
    KeyTargetKind::Object(ObjectKind::Template),
    KeyTargetKind::Object(ObjectKind::McpServer),
    KeyTargetKind::Object(ObjectKind::Mount),
    KeyTargetKind::Object(ObjectKind::Skill),
    KeyTargetKind::Object(ObjectKind::Org),
    KeyTargetKind::Object(ObjectKind::Project),
    KeyTargetKind::Object(ObjectKind::Topic),
    KeyTargetKind::Object(ObjectKind::Thread),
];
/// The native dependency-name role, shared independently by each consuming operation.
pub const NAMES: &str = "resource_fact_names";
/// The native optional owned-environment role.
pub const PARENTS: &str = "library_owned_environments";

/// Native batched library roots, actual owned parents and independently readable names.
pub const LIBRARY_FACETS: [FacetSpec; 3] = [
    FacetSpec::required(
        KeyAction::Read,
        crate::library_sources::RESOURCES,
        TargetResolver::ResolvedCollection("requested_resources"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection(PARENTS),
    ),
    FacetSpec::response_content(NAMED_KINDS, TargetResolver::ResolvedCollection(NAMES)),
];
/// The exact single impact target supported by the native reader and its named consequences.
pub const IMPACT_FACETS: [FacetSpec; 2] = [
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Agent),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::Resolved("requested_resources"),
    ),
    FacetSpec::response_content(NAMED_KINDS, TargetResolver::ResolvedCollection(NAMES)),
];

/// One factual native row for batched resource figures, including actual empty requests.
pub const LIBRARY: Declaration<Source> = Declaration {
    operation: "ResourceLibraryRead",
    bindings: &[
        Binding {
            source: Source::Requested,
            target: TargetResolver::ResolvedCollection("requested_resources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OwnedEnvironments,
            target: TargetResolver::ResolvedCollection(PARENTS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Names,
            target: TargetResolver::ResolvedCollection(NAMES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
/// One factual native row for exact current impact and activity.
pub const IMPACT: Declaration<Source> = Declaration {
    operation: "ResourceImpactRead",
    bindings: &[
        Binding {
            source: Source::Requested,
            target: TargetResolver::Resolved("requested_resources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Names,
            target: TargetResolver::ResolvedCollection(NAMES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
