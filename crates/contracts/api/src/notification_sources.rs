//! Native notification subject declarations, shared by context and effect producers.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Subject kinds of the existing native notification subject switch.
/// A declared kind still passes the native producer's lifecycle and capability rules.
pub const SUBJECT_KINDS: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Org),
    KeyTargetKind::Object(ObjectKind::Project),
    KeyTargetKind::Object(ObjectKind::Topic),
    KeyTargetKind::Object(ObjectKind::Thread),
    KeyTargetKind::Object(ObjectKind::Document),
    KeyTargetKind::Object(ObjectKind::DocumentContainer),
    KeyTargetKind::Identity(KeyIdentityKind::Chat),
    KeyTargetKind::Object(ObjectKind::Provider),
    KeyTargetKind::Object(ObjectKind::Agent),
    KeyTargetKind::Object(ObjectKind::Environment),
    KeyTargetKind::Object(ObjectKind::Host),
    KeyTargetKind::Object(ObjectKind::Machine),
    KeyTargetKind::Object(ObjectKind::Template),
    KeyTargetKind::Object(ObjectKind::McpServer),
    KeyTargetKind::Object(ObjectKind::Mount),
    KeyTargetKind::Object(ObjectKind::Skill),
];

/// Native sources of a notification operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual node, document, chat or resource selected by the native producer.
    Subject,
    /// The actual document governing the selected owned notice jobs.
    NoticeDocument,
    /// Actual machine names reported by the native job preparation.
    NoticeMachines,
    /// Nodes carrying the selected manual notification exceptions.
    ExceptionNodes,
    /// Actual ancestor subscription rows supplying inherited levels.
    ExceptionDefaults,
    /// Optional org names in the native exception path.
    ExceptionOrgNames,
    /// Optional project names in that path.
    ExceptionProjectNames,
    /// Optional topic names in that path.
    ExceptionTopicNames,
    /// Optional thread names in that path.
    ExceptionThreadNames,
}

/// A context read is bound to the same actual subject as its native capability projection.
pub const PERSON_CONTEXT: Declaration<Source> = Declaration {
    operation: "PersonNotifyContextGet",
    bindings: &[Binding {
        source: Source::Subject,
        target: TargetResolver::Resolved("subject"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Document notice progress is bound to its document, with optional native waiting names.
pub const DOCUMENT_JOBS: Declaration<Source> = Declaration {
    operation: "DocumentNoticeJobList",
    bindings: &[
        Binding {
            source: Source::NoticeDocument,
            target: TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Object(ObjectKind::Document),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::NoticeMachines,
            target: TargetResolver::ResolvedCollection("notice_job_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The native exception row, inherited level source and optional path names.
pub const EXCEPTIONS: Declaration<Source> = Declaration {
    operation: "MeNotificationExceptionList",
    bindings: &[
        Binding {
            source: Source::ExceptionNodes,
            target: TargetResolver::ResolvedCollection("exception_nodes"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ExceptionDefaults,
            target: TargetResolver::ResolvedCollection("exception_defaults"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ExceptionOrgNames,
            target: TargetResolver::ResolvedCollection("exception_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ExceptionProjectNames,
            target: TargetResolver::ResolvedCollection("exception_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ExceptionTopicNames,
            target: TargetResolver::ResolvedCollection("exception_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ExceptionThreadNames,
            target: TargetResolver::ResolvedCollection("exception_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
