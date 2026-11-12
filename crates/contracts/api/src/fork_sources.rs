//! Actual job and name sources of native fork plans and batches.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    PendingBatchJobs,
    BatchJobs,
    WaitingMachines,
    NamesOrgs,
    NamesProjects,
    NamesTopics,
    NamesThreads,
}

pub const BATCH_GET: Declaration<Source> = Declaration {
    operation: "ForkBatchGet",
    bindings: &[
        Binding {
            source: Source::BatchJobs,
            target: TargetResolver::ResolvedCollection("batch_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::WaitingMachines,
            target: TargetResolver::ResolvedCollection("batch_waiting_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesOrgs,
            target: TargetResolver::ResolvedCollection("batch_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesProjects,
            target: TargetResolver::ResolvedCollection("batch_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesTopics,
            target: TargetResolver::ResolvedCollection("batch_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesThreads,
            target: TargetResolver::ResolvedCollection("batch_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const BATCH_CANCEL: Declaration<Source> = Declaration {
    operation: "ForkBatchCancel",
    bindings: &[
        Binding {
            source: Source::PendingBatchJobs,
            target: TargetResolver::ResolvedCollection("remaining_batch_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::BatchJobs,
            target: TargetResolver::ResolvedCollection("batch_subjects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::WaitingMachines,
            target: TargetResolver::ResolvedCollection("batch_waiting_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesOrgs,
            target: TargetResolver::ResolvedCollection("batch_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesProjects,
            target: TargetResolver::ResolvedCollection("batch_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesTopics,
            target: TargetResolver::ResolvedCollection("batch_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesThreads,
            target: TargetResolver::ResolvedCollection("batch_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native act sources. Carried conversation/files remain under the one managed source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanSource {
    /// Actual new topic where the moving conversation is created.
    MoveDestination,
    /// The same native resource cascade selected by original Thread Delete.
    MoveResources,
    Thread,
    Sources,
    CopiedPages,
    Subjects,
    Content,
}
const SINGLE_BINDINGS: &[Binding<PlanSource>] = &[
    Binding {
        source: PlanSource::Thread,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: PlanSource::CopiedPages,
        target: TargetResolver::ResolvedCollection("fork_copied_pages"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: PlanSource::Subjects,
        target: TargetResolver::ResolvedCollection("fork_subjects"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: PlanSource::Content,
        target: TargetResolver::ResolvedCollection("fork_content"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
const BATCH_BINDINGS: &[Binding<PlanSource>] = &[
    Binding {
        source: PlanSource::Sources,
        target: TargetResolver::ResolvedCollection("fork_sources"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    SINGLE_BINDINGS[1],
    SINGLE_BINDINGS[2],
    SINGLE_BINDINGS[3],
];
pub const PREFLIGHT_FORK: Declaration<PlanSource> = Declaration {
    operation: "ThreadForkPreflight.fork",
    bindings: SINGLE_BINDINGS,
};
pub const PREFLIGHT_SWITCH: Declaration<PlanSource> = Declaration {
    operation: "ThreadForkPreflight.switch",
    bindings: SINGLE_BINDINGS,
};
pub const CONFIRM_FORK: Declaration<PlanSource> = Declaration {
    operation: "ThreadForkCreate.fork",
    bindings: SINGLE_BINDINGS,
};
pub const CONFIRM_SWITCH: Declaration<PlanSource> = Declaration {
    operation: "ThreadForkCreate.switch",
    bindings: SINGLE_BINDINGS,
};
pub const BATCH_PREFLIGHT: Declaration<PlanSource> = Declaration {
    operation: "ForkBatchPreflight.switch",
    bindings: BATCH_BINDINGS,
};
pub const BATCH_CONFIRM: Declaration<PlanSource> = Declaration {
    operation: "ForkBatchCreate.switch",
    bindings: BATCH_BINDINGS,
};
pub const NOW: Declaration<PlanSource> = Declaration {
    operation: "ThreadForkNow",
    bindings: SINGLE_BINDINGS,
};
use crate::operations::{FacetSpec as Facet, VariantSpec as Variant};
use orch_access_contract::{
    key_scope::{KeyAction::Manage, KeyTargetKind::Object},
    permissions::ObjectKind as K,
};
const COPIED_PAGE: Facet = Facet::required(
    Manage,
    &[Object(K::Thread)],
    TargetResolver::ResolvedCollection("fork_copied_pages"),
);
const SUBJECTS: Facet = Facet::response_content(
    &[Object(K::Thread), Object(K::Environment)],
    TargetResolver::ResolvedCollection("fork_subjects"),
);
const CONTENT: Facet = Facet::response_content(
    &[
        Object(K::Org),
        Object(K::Project),
        Object(K::Topic),
        Object(K::Thread),
        Object(K::Document),
        Object(K::Agent),
        Object(K::Provider),
        Object(K::Environment),
        Object(K::Machine),
        Object(K::Host),
        Object(K::Template),
        Object(K::Mount),
    ],
    TargetResolver::ResolvedCollection("fork_content"),
);
const SINGLE_SOURCE: Facet =
    Facet::required(Manage, &[Object(K::Thread)], TargetResolver::Path("id"));
const BATCH_SOURCES: Facet = Facet::required(
    Manage,
    &[Object(K::Thread), Object(K::Environment)],
    TargetResolver::ResolvedCollection("fork_sources"),
);
pub const SINGLE_VARIANTS: [Variant; 2] = [
    Variant {
        name: "fork",
        selector: "the native result creates a thread",
        facets: &[SINGLE_SOURCE, COPIED_PAGE, SUBJECTS, CONTENT],
    },
    Variant {
        name: "switch",
        selector: "the native result replaces execution",
        facets: &[SINGLE_SOURCE, COPIED_PAGE, SUBJECTS, CONTENT],
    },
];
pub const BATCH_VARIANTS: [Variant; 1] = [Variant {
    name: "switch",
    selector: "the actual selected native batch",
    facets: &[BATCH_SOURCES, COPIED_PAGE, SUBJECTS, CONTENT],
}];
pub const NOW_FACETS: &[Facet] = &[SINGLE_SOURCE, COPIED_PAGE, SUBJECTS, CONTENT];
const SINGLE_PREPARATION: &[Binding<PlanSource>] = &[SINGLE_BINDINGS[0], SINGLE_BINDINGS[1]];
const BATCH_PREPARATION: &[Binding<PlanSource>] = &[BATCH_BINDINGS[0], BATCH_BINDINGS[1]];
const fn phase(row: &Declaration<PlanSource>, batch: bool) -> Declaration<PlanSource> {
    Declaration {
        operation: row.operation,
        bindings: if batch {
            BATCH_PREPARATION
        } else {
            SINGLE_PREPARATION
        },
    }
}
const PREFLIGHT_FORK_PHASE: Declaration<PlanSource> = phase(&PREFLIGHT_FORK, false);
const PREFLIGHT_SWITCH_PHASE: Declaration<PlanSource> = phase(&PREFLIGHT_SWITCH, false);
const CONFIRM_FORK_PHASE: Declaration<PlanSource> = phase(&CONFIRM_FORK, false);
const CONFIRM_SWITCH_PHASE: Declaration<PlanSource> = phase(&CONFIRM_SWITCH, false);
const BATCH_PREFLIGHT_PHASE: Declaration<PlanSource> = phase(&BATCH_PREFLIGHT, true);
const BATCH_CONFIRM_PHASE: Declaration<PlanSource> = phase(&BATCH_CONFIRM, true);
const NOW_PHASE: Declaration<PlanSource> = phase(&NOW, false);
/// The destination side uses the native creation/binding prerequisites, not component writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDestination {
    CreationAndBinding,
    Binding,
}

/// One canonical registration row, including its real native variants and preparation phase.
pub struct PlanRow {
    pub destination: NativeDestination,
    pub route: crate::routes::Route,
    pub default: &'static Declaration<PlanSource>,
    pub phase_default: &'static Declaration<PlanSource>,
    pub fork: Option<&'static Declaration<PlanSource>>,
    pub switch: Option<&'static Declaration<PlanSource>>,
    pub fork_phase: Option<&'static Declaration<PlanSource>>,
    pub switch_phase: Option<&'static Declaration<PlanSource>>,
}
use crate::routes::Endpoint;
pub const PREFLIGHT: PlanRow = PlanRow {
    destination: NativeDestination::CreationAndBinding,
    route: crate::forks::ThreadForkPreflight::ROUTE,
    default: &PREFLIGHT_FORK,
    phase_default: &PREFLIGHT_FORK_PHASE,
    fork: Some(&PREFLIGHT_FORK),
    switch: Some(&PREFLIGHT_SWITCH),
    fork_phase: Some(&PREFLIGHT_FORK_PHASE),
    switch_phase: Some(&PREFLIGHT_SWITCH_PHASE),
};
pub const CONFIRM: PlanRow = PlanRow {
    destination: NativeDestination::CreationAndBinding,
    route: crate::forks::ThreadForkCreate::ROUTE,
    default: &CONFIRM_FORK,
    phase_default: &CONFIRM_FORK_PHASE,
    fork: Some(&CONFIRM_FORK),
    switch: Some(&CONFIRM_SWITCH),
    fork_phase: Some(&CONFIRM_FORK_PHASE),
    switch_phase: Some(&CONFIRM_SWITCH_PHASE),
};
pub const BATCH_PREFLIGHT_ROW: PlanRow = PlanRow {
    destination: NativeDestination::Binding,
    route: crate::forks::ForkBatchPreflight::ROUTE,
    default: &BATCH_PREFLIGHT,
    phase_default: &BATCH_PREFLIGHT_PHASE,
    fork: None,
    switch: Some(&BATCH_PREFLIGHT),
    fork_phase: None,
    switch_phase: Some(&BATCH_PREFLIGHT_PHASE),
};
pub const BATCH_CONFIRM_ROW: PlanRow = PlanRow {
    destination: NativeDestination::Binding,
    route: crate::forks::ForkBatchCreate::ROUTE,
    default: &BATCH_CONFIRM,
    phase_default: &BATCH_CONFIRM_PHASE,
    fork: None,
    switch: Some(&BATCH_CONFIRM),
    fork_phase: None,
    switch_phase: Some(&BATCH_CONFIRM_PHASE),
};
pub const NOW_ROW: PlanRow = PlanRow {
    destination: NativeDestination::CreationAndBinding,
    route: crate::forks::ThreadForkNow::ROUTE,
    default: &NOW,
    phase_default: &NOW_PHASE,
    fork: Some(&NOW),
    switch: None,
    fork_phase: Some(&NOW_PHASE),
    switch_phase: None,
};

const MOVE_DESTINATION: Binding<PlanSource> = Binding {
    source: PlanSource::MoveDestination,
    target: TargetResolver::Resolved("move_destination"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const MOVE_RESOURCES: Binding<PlanSource> = Binding {
    source: PlanSource::MoveResources,
    target: TargetResolver::ResolvedCollection("move_deleted_resources"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const MOVE_BINDINGS: &[Binding<PlanSource>] = &[
    SINGLE_BINDINGS[0],
    SINGLE_BINDINGS[1],
    MOVE_DESTINATION,
    MOVE_RESOURCES,
    SINGLE_BINDINGS[2],
    SINGLE_BINDINGS[3],
];
const MOVE_PHASE_BINDINGS: &[Binding<PlanSource>] = &[
    SINGLE_BINDINGS[0],
    SINGLE_BINDINGS[1],
    MOVE_DESTINATION,
    MOVE_RESOURCES,
];
pub const MOVE_PREVIEW: Declaration<PlanSource> = Declaration {
    operation: "ThreadMoveOutPreview",
    bindings: MOVE_BINDINGS,
};
pub const MOVE_CONFIRM: Declaration<PlanSource> = Declaration {
    operation: "ThreadMoveOut",
    bindings: MOVE_BINDINGS,
};
const MOVE_PREVIEW_PHASE: Declaration<PlanSource> = Declaration {
    operation: MOVE_PREVIEW.operation,
    bindings: MOVE_PHASE_BINDINGS,
};
const MOVE_CONFIRM_PHASE: Declaration<PlanSource> = Declaration {
    operation: MOVE_CONFIRM.operation,
    bindings: MOVE_PHASE_BINDINGS,
};
pub const MOVE_FACETS: &[Facet] = &[
    SINGLE_SOURCE,
    COPIED_PAGE,
    Facet::required(
        orch_access_contract::key_scope::KeyAction::Write,
        &[Object(K::Thread)],
        TargetResolver::Resolved("move_destination"),
    ),
    Facet::required(
        Manage,
        &[
            Object(K::Provider),
            Object(K::Agent),
            Object(K::Environment),
            Object(K::Host),
            Object(K::Machine),
            Object(K::Template),
            Object(K::McpServer),
            Object(K::Mount),
            Object(K::Skill),
        ],
        TargetResolver::ResolvedCollection("move_deleted_resources"),
    ),
    SUBJECTS,
    CONTENT,
];
pub const MOVE_PREFLIGHT_ROW: PlanRow = PlanRow {
    destination: NativeDestination::CreationAndBinding,
    route: crate::duplicates::ThreadMoveOutPreview::ROUTE,
    default: &MOVE_PREVIEW,
    phase_default: &MOVE_PREVIEW_PHASE,
    fork: Some(&MOVE_PREVIEW),
    switch: None,
    fork_phase: Some(&MOVE_PREVIEW_PHASE),
    switch_phase: None,
};
pub const MOVE_CONFIRM_ROW: PlanRow = PlanRow {
    destination: NativeDestination::CreationAndBinding,
    route: crate::duplicates::ThreadMoveOut::ROUTE,
    default: &MOVE_CONFIRM,
    phase_default: &MOVE_CONFIRM_PHASE,
    fork: Some(&MOVE_CONFIRM),
    switch: None,
    fork_phase: Some(&MOVE_CONFIRM_PHASE),
    switch_phase: None,
};
