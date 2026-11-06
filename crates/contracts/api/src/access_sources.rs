//! Native permission-related history rows and their independently protected live names.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind as K;

/// Factual sources reported by the native history selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual stored page subject whose native history is selected.
    Page,
    /// Actual viewed threads a history line is about.
    Threads,
    /// Optional current org labels.
    OrgNames,
    /// Optional current project labels.
    ProjectNames,
    /// Optional current topic labels.
    TopicNames,
    /// Optional current thread labels.
    ThreadNames,
}

/// Canonical history actions and kinds; the native descriptor supplies only their sources.
pub const HISTORY_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
            KeyTargetKind::Object(K::Document),
            KeyTargetKind::Object(K::DocumentContainer),
            KeyTargetKind::Object(K::Provider),
            KeyTargetKind::Object(K::Agent),
            KeyTargetKind::Object(K::Environment),
            KeyTargetKind::Object(K::Host),
            KeyTargetKind::Object(K::Machine),
            KeyTargetKind::Object(K::Template),
            KeyTargetKind::Object(K::McpServer),
            KeyTargetKind::Object(K::Mount),
            KeyTargetKind::Object(K::Skill),
        ],
        TargetResolver::Resolved("page"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(K::Thread)],
        TargetResolver::ResolvedCollection("history_threads"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::ResolvedCollection("history_org_names"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(K::Project)],
        TargetResolver::ResolvedCollection("history_project_names"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(K::Topic)],
        TargetResolver::ResolvedCollection("history_topic_names"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(K::Thread)],
        TargetResolver::ResolvedCollection("history_thread_names"),
    ),
];

/// One row binds the native history factory to its canonical operation.
pub const HISTORY: Declaration<Source> = Declaration {
    operation: "HistoryList",
    bindings: &[
        Binding {
            source: Source::Page,
            target: TargetResolver::Resolved("page"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection("history_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OrgNames,
            target: TargetResolver::ResolvedCollection("history_org_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ProjectNames,
            target: TargetResolver::ResolvedCollection("history_project_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::TopicNames,
            target: TargetResolver::ResolvedCollection("history_topic_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ThreadNames,
            target: TargetResolver::ResolvedCollection("history_thread_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
