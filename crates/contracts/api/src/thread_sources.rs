//! Stored thread facts supplied by native control and response producers.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// A control's actual stored subject, before applying or projecting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The exact thread whose native control accepts the request.
    Thread,
    /// Actual selected saved-byte sources of an attachment reference.
    AttachmentChats,
    AttachmentThreads,
    AttachmentDocuments,
    AttachmentEnvironments,
    AttachmentUploads,
    /// Actual saved-byte sources of the admitted continuation content.
    ContinuationChats,
    ContinuationThreads,
    ContinuationDocuments,
    ContinuationEnvironments,
    ContinuationUploads,
    /// The actual newly created side, independently read after its accepted effect.
    CreatedThread,
    /// Native thread header and its selected foreign content sources.
    ThreadContent,
    /// Native profile, provider, environment and selected cap sources of an optional pane.
    OpeningControls,
    /// Current provenance labels selected by the opening's bounded native name producer.
    OpeningNames,
}

/// Setting a goal has independent effect and same-operation response roles.
pub const GOAL_CREATE: Declaration<Source> = Declaration {
    operation: "ThreadGoalCreate",
    bindings: &[
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// A screen opening reads its own conversation and only the selected auxiliary sources.
pub const OPENING: Declaration<Source> = Declaration {
    operation: "ThreadOpeningGet",
    bindings: &[
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OpeningControls,
            target: TargetResolver::ResolvedCollection("opening_controls"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OpeningNames,
            target: TargetResolver::ResolvedCollection("opening_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

const ATTACHMENT_THREAD: Binding<Source> = Binding {
    source: Source::Thread,
    target: TargetResolver::PermissionObject,
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
/// New supplied bytes require only the actual destination's Write.
pub const ATTACHMENT_DIRECT: Declaration<Source> = Declaration {
    operation: "ThreadAttachmentCreate.direct",
    bindings: &[ATTACHMENT_THREAD],
};
/// Referenced bytes select one real current native custody/saved-source witness.
pub const ATTACHMENT_REFERENCED: Declaration<Source> = Declaration {
    operation: "ThreadAttachmentCreate.from_upload",
    bindings: &[
        ATTACHMENT_THREAD,
        Binding {
            source: Source::AttachmentChats,
            target: TargetResolver::ResolvedCollection("message_source_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::AttachmentThreads,
            target: TargetResolver::ResolvedCollection("message_source_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::AttachmentDocuments,
            target: TargetResolver::ResolvedCollection("message_source_documents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::AttachmentEnvironments,
            target: TargetResolver::ResolvedCollection("message_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::AttachmentUploads,
            target: TargetResolver::ResolvedCollection("message_retained_uploads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

const CONTINUATION_BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::Thread,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::ContinuationChats,
        target: TargetResolver::ResolvedCollection("message_source_chats"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::ContinuationThreads,
        target: TargetResolver::ResolvedCollection("message_source_threads"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::ContinuationDocuments,
        target: TargetResolver::ResolvedCollection("message_source_documents"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::ContinuationEnvironments,
        target: TargetResolver::ResolvedCollection("message_source_environments"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::ContinuationUploads,
        target: TargetResolver::ResolvedCollection("message_retained_uploads"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];
/// The exact native source input/branch and supplied content of a prompt edit.
pub const PROMPT_CREATE: Declaration<Source> = Declaration {
    operation: "ThreadPromptVersionCreate",
    bindings: CONTINUATION_BINDINGS,
};
/// The real retained continuation selected on this thread, with its own queued content.
pub const CONTINUATION_ACTIVATE: Declaration<Source> = Declaration {
    operation: "ThreadContinuationActivate",
    bindings: CONTINUATION_BINDINGS,
};

/// The native aside uses its actual parent history and the same supplied-content sources.
pub const SIDE_CREATE: Declaration<Source> = Declaration {
    operation: "ThreadSideCreate",
    bindings: &[
        CONTINUATION_BINDINGS[0],
        CONTINUATION_BINDINGS[1],
        CONTINUATION_BINDINGS[2],
        CONTINUATION_BINDINGS[3],
        CONTINUATION_BINDINGS[4],
        CONTINUATION_BINDINGS[5],
        Binding {
            source: Source::CreatedThread,
            target: TargetResolver::Resolved("created_thread"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ThreadContent,
            target: TargetResolver::ResolvedCollection("side_thread_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
