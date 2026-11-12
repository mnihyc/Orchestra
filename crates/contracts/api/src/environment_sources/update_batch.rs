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
    /// Newly selected templates whose native use is part of confirmation/execution.
    Templates,
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

/// Confirmation reuses the same actual member and response roles under its own operation.
pub const NODE_CREATE: Declaration<Source> = Declaration {
    operation: "NodeEnvironmentUpdateCreate",
    bindings: &[
        Binding {
            source: Source::Environments,
            target: TargetResolver::ResolvedCollection("affected_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Templates,
            target: TargetResolver::ResolvedCollection("update_templates"),
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
            KeyTargetKind::Object(ObjectKind::Machine),
        ],
        TargetResolver::ResolvedCollection("update_context"),
    ),
];

/// Confirmation uses each newly selected Template, while retaining an old one needs no new Use.
pub const NODE_CREATE_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("affected_environments"),
    ),
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Template)],
        TargetResolver::ResolvedCollection("update_templates"),
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
            KeyTargetKind::Object(ObjectKind::Machine),
        ],
        TargetResolver::ResolvedCollection("update_context"),
    ),
];

/// The same native update act selected globally, without a parent-node prerequisite.
pub const GLOBAL_PREFLIGHT: Declaration<Source> = Declaration {
    operation: "EnvironmentUpdateBatchPreflight",
    bindings: NODE_PREFLIGHT.bindings,
};
/// Confirmation retains its own operation while using the same native source factory.
pub const GLOBAL_CREATE: Declaration<Source> = Declaration {
    operation: "EnvironmentUpdateBatchCreate",
    bindings: NODE_CREATE.bindings,
};
/// Native update semantics and optional disclosure are shared by both selectors.
pub const BATCH_FACETS: &[FacetSpec] = NODE_PREFLIGHT_FACETS;

/// Confirmation uses the same selected-template native act for either selector.
pub const BATCH_CREATE_FACETS: &[FacetSpec] = NODE_CREATE_FACETS;
