//! Native loss preview custody and independently readable recorded content.

use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetSpec, FacetUsage, TargetResolver},
};
use orch_access_contract::{
    key_scope::{KeyAction, KeyTargetKind},
    permissions::ObjectKind,
};

/// Typed native sources selected before the preview's formatter.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual requested native inspector and its independently readable context.
    Target,
    /// Recorded Environment, Provider and Mount identities in the native inventory.
    Related,
    /// Only actual recorded Thread names emitted by the bounded native projector.
    ThreadNames,
    /// The complete actual native cascade footprint, before any loss mutation.
    Affected,
}

/// A preview saves a new review at its actual target; it performs no cascade.
pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "LossPreflight",
    bindings: &[
        Binding {
            source: Source::Target,
            target: TargetResolver::Resolved("loss_target"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Target,
            target: TargetResolver::Resolved("loss_target"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Related,
            target: TargetResolver::ResolvedCollection("loss_related"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ThreadNames,
            target: TargetResolver::ResolvedCollection("loss_thread_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Management can save the native review without borrowing any response Read.
pub const PREFLIGHT_FACETS: [FacetSpec; 4] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::Resolved("loss_target"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::Resolved("loss_target"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Mount),
        ],
        TargetResolver::ResolvedCollection("loss_related"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("loss_thread_names"),
    ),
];

/// A confirmation owns its current cascade separately from its original preview.
pub const DECLARE: Declaration<Source> = Declaration {
    operation: "LossDeclare",
    bindings: &[
        PREFLIGHT.bindings[0],
        Binding {
            source: Source::Affected,
            target: TargetResolver::ResolvedCollection("loss_affected"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        PREFLIGHT.bindings[1],
        PREFLIGHT.bindings[2],
        PREFLIGHT.bindings[3],
    ],
};

/// Every actual native effect is bounded without adding another ordinary permission gate.
pub const DECLARE_FACETS: [FacetSpec; 5] = [
    PREFLIGHT_FACETS[0],
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Mount),
            KeyTargetKind::Object(ObjectKind::McpServer),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::ResolvedCollection("loss_affected"),
    ),
    PREFLIGHT_FACETS[1],
    PREFLIGHT_FACETS[2],
    PREFLIGHT_FACETS[3],
];
