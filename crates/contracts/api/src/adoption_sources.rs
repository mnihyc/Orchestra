//! Actual sources of an adopting creator, preview and its native preparation phase.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};

/// Factual roles, independent of the currently checking transport or credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The real destination Topic, with Thread creation as its business family.
    Destination,
    /// The actual Provider whose native session is read.
    Session,
    /// The actual Provider, Agent and selected Environment used by the native phase.
    Binding,
    /// Existing targets whose content the preview actually emits.
    PreviewContent,
    /// Optional existing address components emitted by the native environment reference.
    NameOrgs,
    /// The actual retained adopted Thread; its original marker supplies the record source.
    RetainedThread,
    /// The existing native Node header's actual binding, README and visible-name sources.
    Header,
    /// Actual created Thread content.
    Created,
    /// Native no-new-environment decision.
    NewEnvironmentHomes,
    /// Native initial-copy source Threads, if selected.
    InitialThreads,
    /// Native initial-copy source Environments, if selected.
    InitialEnvironments,
    /// Actual native file preparation effects.
    FileEnvironments,
    /// Actual native file copy sources.
    FileSources,
    /// Project gift source selected by the actual creator.
    AgentProjects,
    /// Topic gift source selected by the actual creator.
    AgentTopics,
    /// Actual born Thread gift source, never its parent path.
    AgentThreads,
    /// Native message copy Chat sources.
    MessageChats,
    /// Native message copy Thread sources.
    MessageThreads,
    /// Native message copy Document sources.
    MessageDocuments,
    /// Native message copy Environment sources.
    MessageEnvironments,
    /// Actual retained upload sources of the native first input.
    MessageUploads,
}

const PREPARATION: &[Binding<Source>] = &[
    Binding {
        source: Source::Destination,
        target: TargetResolver::TypedPath {
            parameter: "id",
            kind: KeyTargetKind::Object(ObjectKind::Topic),
        },
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Session,
        target: TargetResolver::Resolved("native_session"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Binding,
        target: TargetResolver::ResolvedCollection("chosen_binding"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];

/// The known retained Thread and original Provider used by missing-history retry preparation.
pub const RETRY_PREPARATION: Declaration<Source> = Declaration {
    operation: "ThreadAdoptRetryHistory",
    bindings: &[
        Binding {
            source: Source::RetainedThread,
            target: TargetResolver::PermissionObject,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Session,
            target: TargetResolver::Resolved("adopted_history_source"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};

/// The actual known-source preparation of an adopting creator. This cannot admit its later gifts.
pub const ADOPT_PREPARATION: Declaration<Source> = Declaration {
    operation: "ThreadAdopt",
    bindings: PREPARATION,
};
/// The same native preparation, under the preview's independent operation.
pub const PREVIEW_PREPARATION: Declaration<Source> = Declaration {
    operation: "ThreadAdoptPreview",
    bindings: PREPARATION,
};

/// A preview has no newly created Thread. Its selected native content supplies its own Read.
pub const PREVIEW: Declaration<Source> = Declaration {
    operation: "ThreadAdoptPreview",
    bindings: &[
        PREPARATION[0],
        PREPARATION[1],
        PREPARATION[2],
        Binding {
            source: Source::PreviewContent,
            target: TargetResolver::ResolvedCollection("preview_content"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameOrgs,
            target: TargetResolver::ResolvedCollection("preview_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Complete actual adopting creator/outcome source connections, including dependent native effects.
pub const ADOPT: Declaration<Source> = Declaration {
    operation: "ThreadAdopt",
    bindings: &[
        PREPARATION[0],
        PREPARATION[1],
        PREPARATION[2],
        Binding {
            source: Source::NewEnvironmentHomes,
            target: TargetResolver::ResolvedCollection("creator_new_environment_homes"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::InitialThreads,
            target: TargetResolver::ResolvedCollection("creator_initial_file_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::InitialEnvironments,
            target: TargetResolver::ResolvedCollection("creator_initial_file_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FileEnvironments,
            target: TargetResolver::ResolvedCollection("creator_file_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FileSources,
            target: TargetResolver::ResolvedCollection("creator_file_sources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::AgentProjects,
            target: TargetResolver::ResolvedCollection("creator_agent_projects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::AgentTopics,
            target: TargetResolver::ResolvedCollection("creator_agent_topics"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::AgentThreads,
            target: TargetResolver::ResolvedCollection("creator_agent_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::MessageChats,
            target: TargetResolver::ResolvedCollection("message_source_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::MessageThreads,
            target: TargetResolver::ResolvedCollection("message_source_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::MessageDocuments,
            target: TargetResolver::ResolvedCollection("message_source_documents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::MessageEnvironments,
            target: TargetResolver::ResolvedCollection("message_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::MessageUploads,
            target: TargetResolver::ResolvedCollection("message_retained_uploads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Created,
            target: TargetResolver::Resolved("created_thread"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Header,
            target: TargetResolver::ResolvedCollection("adopted_thread_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Retry admission and independent optional native header publication.
pub const RETRY: Declaration<Source> = Declaration {
    operation: "ThreadAdoptRetryHistory",
    bindings: &[
        RETRY_PREPARATION.bindings[0],
        RETRY_PREPARATION.bindings[1],
        Binding {
            source: Source::RetainedThread,
            target: TargetResolver::PermissionObject,
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Header,
            target: TargetResolver::ResolvedCollection("retried_thread_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
