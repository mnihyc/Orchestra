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
