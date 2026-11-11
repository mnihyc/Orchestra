//! Native Template creation and its optional individual response sources.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Actual native creation home and completed template projection roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual existing organization or System namespace where creation is authorized.
    Home,
    /// The real stored template produced by the completed native effect.
    Created,
    /// Actual System driver administration, checked empty for an unrestricted driver.
    DriverAdministration,
    /// The native singleton template content-source collection.
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
pub const REFERENCES: &str = "template_created_sources";
/// The native permission-group payload collection.
pub const GROUPS: &str = "template_created_groups";
/// Native protected node-label roles, in hierarchy kind order.
pub const NODES: [&str; 4] = [
    "template_created_orgs",
    "template_created_projects",
    "template_created_topics",
    "template_created_threads",
];

/// Creation keeps the existing management act; content remains same-operation Read.
pub const CREATE_FACETS: [FacetSpec; 9] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Template)],
        TargetResolver::Resolved("home"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Template)],
        TargetResolver::ResolvedCollection("driver_administration"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Template)],
        TargetResolver::Resolved("created_template"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Template)],
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
    operation: "TemplateCreate",
    bindings: &[
        Binding {
            source: Source::Home,
            target: TargetResolver::Resolved("home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::DriverAdministration,
            target: TargetResolver::ResolvedCollection("driver_administration"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Created,
            target: TargetResolver::Resolved("created_template"),
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
