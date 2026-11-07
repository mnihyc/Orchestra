//! Native place-export source roles shared by metadata and typed preparation.
use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetSpec, FacetUsage, TargetResolver as Target},
};
use orch_access_contract::{
    key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};
/// Actual native subjects selected before any plan aggregation or archive write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual requested place and its configuration.
    Place,
    /// Native readable content, including readable entries that lack Export.
    Contents,
    /// Independently disclosed current org labels.
    Orgs,
    /// Independently disclosed current project labels.
    Projects,
    /// Independently disclosed current topic labels.
    Topics,
    /// Independently disclosed current thread labels.
    Threads,
}
const NODES: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Org),
    KeyTargetKind::Object(ObjectKind::Project),
    KeyTargetKind::Object(ObjectKind::Topic),
    KeyTargetKind::Object(ObjectKind::Thread),
];
const CONTENTS: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Org),
    KeyTargetKind::Object(ObjectKind::Project),
    KeyTargetKind::Object(ObjectKind::Topic),
    KeyTargetKind::Object(ObjectKind::Thread),
    KeyTargetKind::Object(ObjectKind::Document),
    KeyTargetKind::Object(ObjectKind::DocumentContainer),
    KeyTargetKind::Identity(KeyIdentityKind::Chat),
];
/// Root Read and actual native contributing content are distinct; extra live labels are optional.
pub const PREFLIGHT_FACETS: [FacetSpec; 6] = [
    FacetSpec::required(KeyAction::Read, NODES, Target::Path("id")),
    FacetSpec::required(
        KeyAction::Read,
        CONTENTS,
        Target::ResolvedCollection("export_contents"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        Target::ResolvedCollection("export_name_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        Target::ResolvedCollection("export_name_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        Target::ResolvedCollection("export_name_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        Target::ResolvedCollection("export_name_threads"),
    ),
];
/// One source row consumed by the effect-free native plan's selected-body adapter.
pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "NodeExportPreflight",
    bindings: &[
        Binding {
            source: Source::Place,
            target: Target::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Contents,
            target: Target::ResolvedCollection("export_contents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: Target::ResolvedCollection("export_name_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: Target::ResolvedCollection("export_name_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: Target::ResolvedCollection("export_name_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: Target::ResolvedCollection("export_name_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
