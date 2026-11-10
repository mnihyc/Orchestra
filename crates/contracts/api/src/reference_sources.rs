//! The native reference owner and the protected names its card actually projects.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Target,
    Orgs,
    Projects,
    Topics,
    Threads,
}
pub const NAMES: [(ObjectKind, Source); 4] = [
    (ObjectKind::Org, Source::Orgs),
    (ObjectKind::Project, Source::Projects),
    (ObjectKind::Topic, Source::Topics),
    (ObjectKind::Thread, Source::Threads),
];
pub const FACETS: [FacetSpec; 5] = [
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::Resolved("reference_target"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("reference_name_orgs"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("reference_name_projects"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("reference_name_topics"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("reference_name_threads"),
    ),
];
pub const PREVIEW: Declaration<Source> = Declaration {
    operation: "ReferencePreviewGet",
    bindings: &[
        Binding {
            source: Source::Target,
            target: TargetResolver::Resolved("reference_target"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection("reference_name_orgs"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection("reference_name_projects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection("reference_name_topics"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection("reference_name_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
