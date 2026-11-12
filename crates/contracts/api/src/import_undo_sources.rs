//! The actual stored import and distinct native subjects of taking it back.

use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetSpec, FacetUsage, TargetResolver},
};
use orch_access_contract::{
    key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};

/// Native custody and the actual objects/parents represented by the undo producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The caller's genuine retained Import.
    Import,
    /// Actual distinct native subjects, not the package's source identifiers.
    Subjects,
    /// The actual account named as the job creator.
    Creator,
}

/// Manage authorizes a counts-only undo summary; it discloses no existing resource metadata.
pub const PREFLIGHT_FACETS: [FacetSpec; 2] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Identity(KeyIdentityKind::Import)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
        ],
        TargetResolver::ResolvedCollection("imported_objects"),
    ),
];

/// One factual row consumed by the existing native aggregate preview.
pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "ImportUndoPreflight",
    bindings: &[
        Binding {
            source: Source::Import,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Subjects,
            target: TargetResolver::ResolvedCollection("imported_objects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Queueing and continuing an undo also includes native home-resource cleanup.
pub const CREATE_FACETS: [FacetSpec; 4] = [
    PREFLIGHT_FACETS[0],
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Agent),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::McpServer),
            KeyTargetKind::Object(ObjectKind::Mount),
            KeyTargetKind::Object(ObjectKind::Skill),
        ],
        TargetResolver::ResolvedCollection("imported_objects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Import)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
        TargetResolver::ResolvedCollection("import_undo_creator"),
    ),
];

/// One row for the native accepted job, its execution and optional existing job metadata.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "ImportUndo",
    bindings: &[
        PREFLIGHT.bindings[0],
        PREFLIGHT.bindings[1],
        Binding {
            source: Source::Import,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Creator,
            target: TargetResolver::ResolvedCollection("import_undo_creator"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
