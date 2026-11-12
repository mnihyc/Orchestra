//! Native reviewed Host cascade sources, shared by preview and confirmation.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::{
    key_scope::{KeyAction, KeyTargetKind},
    permissions::ObjectKind,
};

/// Actual native cascade roles, independent of the formatted plan.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The intrinsic Host controller, including its physical node cleanup.
    Host,
    /// Independent logical resources on which the native cascade accepts management acts.
    Managed,
    /// Actual selected placement and mount-location use.
    Used,
    /// Existing plan, job and context content, including protected names.
    Content,
}
const BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::Host,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Managed,
        target: TargetResolver::ResolvedCollection("host_delete_managed"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Used,
        target: TargetResolver::ResolvedCollection("host_delete_used"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Content,
        target: TargetResolver::ResolvedCollection("host_delete_content"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
/// One native preflight declaration; unknown later placement roles stay unknown during initial preparation.
pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "HostDeletePreflight",
    bindings: BINDINGS,
};
/// One native confirmation declaration over its freshly reviewed cascade.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "HostDeleteCreate",
    bindings: BINDINGS,
};
/// Host control, genuine logical cascade acts, selected use, and independently admitted disclosure.
pub const FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Host)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Provider),
        ],
        TargetResolver::ResolvedCollection("host_delete_managed"),
    ),
    FacetSpec::required(
        KeyAction::Write,
        &[
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Mount),
        ],
        TargetResolver::ResolvedCollection("host_delete_used"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Mount),
            KeyTargetKind::Object(ObjectKind::McpServer),
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::ResolvedCollection("host_delete_content"),
    ),
];
