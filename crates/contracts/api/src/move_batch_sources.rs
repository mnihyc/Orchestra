//! Native mixed-lifecycle node move batches and their observed content.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::{
    key_scope::{KeyAction, KeyTargetKind},
    permissions::ObjectKind,
};

/// Typed native selection roles, never response UUID extraction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual managed members of the native batch.
    Environments,
    /// Actual selected Host/Template or attached-machine use.
    Destinations,
    /// Native existing content and protected context references.
    Content,
}
const BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::Environments,
        target: TargetResolver::ResolvedCollection("affected_environments"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Destinations,
        target: TargetResolver::ResolvedCollection("destinations"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Content,
        target: TargetResolver::ResolvedCollection("move_batch_content"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
/// The native preflight is a request-time observation, including pending measurements.
pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "NodeEnvironmentMovePreflight",
    bindings: BINDINGS,
};
/// The same native stored-batch subjects under the confirmation operation.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "NodeEnvironmentMoveCreate",
    bindings: BINDINGS,
};
/// Each lifecycle supplies its real destination kind, including honest empty batches.
pub const FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("affected_environments"),
    ),
    FacetSpec::required(
        KeyAction::Write,
        &[
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Template),
        ],
        TargetResolver::ResolvedCollection("destinations"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::Mount),
            KeyTargetKind::Object(ObjectKind::McpServer),
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::ResolvedCollection("move_batch_content"),
    ),
];
