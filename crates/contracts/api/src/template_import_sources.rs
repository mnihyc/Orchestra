//! A proposal's real creation home, executing Host and retained archive sources.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Actual native operands, including explicitly empty unselected content families.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Home,
    Administration,
    Host,
    Chats,
    Threads,
    Documents,
    Environments,
    Uploads,
}

/// The same source roles consumed by the common stored-byte witness binder.
pub const CONTENT: [(Source, &str, KeyTargetKind); 5] = [
    (
        Source::Chats,
        "message_source_chats",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
    ),
    (
        Source::Threads,
        "message_source_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
    ),
    (
        Source::Documents,
        "message_source_documents",
        KeyTargetKind::Object(ObjectKind::Document),
    ),
    (
        Source::Environments,
        "message_source_environments",
        KeyTargetKind::Object(ObjectKind::Environment),
    ),
    (
        Source::Uploads,
        "message_retained_uploads",
        KeyTargetKind::Identity(KeyIdentityKind::Upload),
    ),
];

/// Existing creation authority, ordinary Host Use and actual copied archive Read.
pub const FACETS: [FacetSpec; 8] = [
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
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Host)],
        TargetResolver::Resolved("execution_host"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[CONTENT[0].2],
        TargetResolver::ResolvedCollection(CONTENT[0].1),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[CONTENT[1].2],
        TargetResolver::ResolvedCollection(CONTENT[1].1),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[CONTENT[2].2],
        TargetResolver::ResolvedCollection(CONTENT[2].1),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[CONTENT[3].2],
        TargetResolver::ResolvedCollection(CONTENT[3].1),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[CONTENT[4].2],
        TargetResolver::ResolvedCollection(CONTENT[4].1),
    ),
];

/// One proposal operation; the native driver saves no Template.
pub const IMPORT: Declaration<Source> = Declaration {
    operation: "TemplateImport",
    bindings: &[
        Binding {
            source: Source::Home,
            target: TargetResolver::Resolved("home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Administration,
            target: TargetResolver::ResolvedCollection("driver_administration"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Host,
            target: TargetResolver::Resolved("execution_host"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Chats,
            target: TargetResolver::ResolvedCollection(CONTENT[0].1),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection(CONTENT[1].1),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Documents,
            target: TargetResolver::ResolvedCollection(CONTENT[2].1),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Environments,
            target: TargetResolver::ResolvedCollection(CONTENT[3].1),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Uploads,
            target: TargetResolver::ResolvedCollection(CONTENT[4].1),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
