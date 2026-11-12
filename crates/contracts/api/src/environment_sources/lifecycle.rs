//! Actual native operands of singleton moves and mixed recovery batches.
use crate::{
    environment::{EnvironmentMoveCreate, EnvironmentMovePreflight},
    loss::{EnvironmentRecoverCreate, EnvironmentRecoverPreflight},
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetSpec, FacetUsage, TargetResolver},
    routes::{Endpoint, Route},
};
use orch_access_contract::{
    key_scope::{KeyAction, KeyTargetKind},
    permissions::ObjectKind,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual selected source environments, including a native proven empty recovery set.
    Environments,
    /// Actual selected use/creation destinations, not every candidate offered by a picker.
    Destinations,
    /// Existing content and protected names carried by the ordinary full answer.
    Content,
}
const BINDINGS: [Binding<Source>; 3] = [
    Binding {
        source: Source::Environments,
        target: TargetResolver::ResolvedCollection("lifecycle_environments"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Destinations,
        target: TargetResolver::ResolvedCollection("lifecycle_destinations"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Content,
        target: TargetResolver::ResolvedCollection("lifecycle_content"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
const SOURCE_PHASE: &[Binding<Source>] = &[BINDINGS[0]];
const DESTINATION_PHASE: &[Binding<Source>] = &[BINDINGS[0], BINDINGS[1]];
/// One canonical declaration per operation; native preparation uses only an explicit prefix.
pub struct Row {
    pub route: Route,
    pub declaration: Declaration<Source>,
    pub source_phase: Declaration<Source>,
    pub destination_phase: Declaration<Source>,
}
const fn row(route: Route, operation: &'static str) -> Row {
    Row {
        route,
        declaration: Declaration {
            operation,
            bindings: &BINDINGS,
        },
        source_phase: Declaration {
            operation,
            bindings: SOURCE_PHASE,
        },
        destination_phase: Declaration {
            operation,
            bindings: DESTINATION_PHASE,
        },
    }
}
pub const MOVE_PREFLIGHT: Row = row(EnvironmentMovePreflight::ROUTE, "EnvironmentMovePreflight");
pub const MOVE_CREATE: Row = row(EnvironmentMoveCreate::ROUTE, "EnvironmentMoveCreate");
pub const RECOVER_PREFLIGHT: Row = row(
    EnvironmentRecoverPreflight::ROUTE,
    "EnvironmentRecoverPreflight",
);
pub const RECOVER_CREATE: Row = row(EnvironmentRecoverCreate::ROUTE, "EnvironmentRecoverCreate");
pub const FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("lifecycle_environments"),
    ),
    FacetSpec::required(
        KeyAction::Write,
        &[
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Template),
        ],
        TargetResolver::ResolvedCollection("lifecycle_destinations"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Agent),
            KeyTargetKind::Object(ObjectKind::Mount),
            KeyTargetKind::Object(ObjectKind::McpServer),
        ],
        TargetResolver::ResolvedCollection("lifecycle_content"),
    ),
];
