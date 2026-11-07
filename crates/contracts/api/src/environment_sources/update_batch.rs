//! Native reviewed update batches and their independently readable plans.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::{
    key_scope::{KeyAction, KeyTargetKind},
    permissions::ObjectKind,
};

/// Factual roles selected by the native update planner, never by response JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual members admitted for management, including a genuinely empty selection.
    Environments,
    /// Existing Environment content in plans and native skipped-member results.
    Content,
    /// Actual existing resources and protected names the native plan displays.
    Context,
}

/// The node selects the native batch; each member supplies its own authority and placement.
pub const NODE_PREFLIGHT: Declaration<Source> = Declaration {
    operation: "NodeEnvironmentUpdatePreflight",
    bindings: &[
        Binding {
            source: Source::Environments,
            target: TargetResolver::ResolvedCollection("affected_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Content,
            target: TargetResolver::ResolvedCollection("update_environments"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Context,
            target: TargetResolver::ResolvedCollection("update_context"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Management keeps its native skipped-member semantics; disclosure is optional after storage.
pub const NODE_PREFLIGHT_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("affected_environments"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("update_environments"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Object(ObjectKind::Template),
        ],
        TargetResolver::ResolvedCollection("update_context"),
    ),
];
