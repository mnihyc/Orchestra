//! Native notification subject declarations, shared by context and effect producers.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
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
    /// Optional node and ancestor names actually copied into a native outgoing notice.
    NoticePlaceNames,
    /// The actual Chat copied only when a person grant requests a notice.
    NoticeChats,
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
    /// The stored channel's real Account, root Org or System namespace.
    ChannelOwner,
    /// Real owner namespaces of the selected available channel rows.
    AvailableChannelOwners,
    /// The caller's actual private route/recipient state, only when selected for output.
    ChannelCallerRoutes,
}

const NOTICE_NAMES: Binding<Source> = Binding {
    source: Source::NoticePlaceNames,
    target: TargetResolver::ResolvedCollection("notice_place_names"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};

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

/// An explicit notice copies an existing subject and performs its own notification act.
pub const PERSON_CREATE: Declaration<Source> = Declaration {
    operation: "PersonNotifyCreate",
    bindings: &[PERSON_CONTEXT.bindings[0], NOTICE_NAMES],
};

const NOTICE_NAME_FACET: FacetSpec = FacetSpec::response_content(
    &[
        KeyTargetKind::Object(ObjectKind::Org),
        KeyTargetKind::Object(ObjectKind::Project),
        KeyTargetKind::Object(ObjectKind::Topic),
        KeyTargetKind::Object(ObjectKind::Thread),
    ],
    TargetResolver::ResolvedCollection("notice_place_names"),
);

/// The notice copies its own subject; optional native place labels have separate sources.
pub const PERSON_CREATE_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        orch_access_contract::key_scope::KeyAction::Read,
        SUBJECT_KINDS,
        TargetResolver::Resolved("subject"),
    ),
    FacetSpec::required(
        orch_access_contract::key_scope::KeyAction::Write,
        SUBJECT_KINDS,
        TargetResolver::Resolved("subject"),
    ),
    NOTICE_NAME_FACET,
];

/// A person grant manages its Chat; a requested notice copies that same existing Chat.
pub const CHAT_GRANT: Declaration<Source> = Declaration {
    operation: "ChatPersonGrant",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::NoticeChats,
            target: TargetResolver::ResolvedCollection("notice_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        NOTICE_NAMES,
    ],
};

/// No notice means a genuinely empty copy source, never Read implied by Manage.
pub const CHAT_GRANT_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        orch_access_contract::key_scope::KeyAction::Manage,
        &[KeyTargetKind::Identity(KeyIdentityKind::Chat)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        orch_access_contract::key_scope::KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Chat)],
        TargetResolver::ResolvedCollection("notice_chats"),
    ),
    NOTICE_NAME_FACET,
];

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

/// Removing a stored channel returns no existing content or secret metadata.
pub const CHANNEL_DELETE: Declaration<Source> = Declaration {
    operation: "NotificationChannelDelete",
    bindings: &[Binding {
        source: Source::ChannelOwner,
        target: TargetResolver::Resolved("channel_owner"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Native sample use keeps the channel's actual owner and its editor-or-own-route gate.
pub const CHANNEL_TEST: Declaration<Source> = Declaration {
    operation: "NotificationChannelTest",
    bindings: CHANNEL_DELETE.bindings,
};

/// Available rows and their independent current-account route projection.
pub const CHANNEL_LIST_AVAILABLE: Declaration<Source> = Declaration {
    operation: "NotificationChannelList.available",
    bindings: &[
        Binding {
            source: Source::AvailableChannelOwners,
            target: TargetResolver::ResolvedCollection("available_channels"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ChannelCallerRoutes,
            target: TargetResolver::ResolvedCollection("channel_caller_routes"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// One actual Org/System management namespace, plus optional own-route content.
pub const CHANNEL_LIST_MANAGE: Declaration<Source> = Declaration {
    operation: "NotificationChannelList.manage",
    bindings: &[
        Binding {
            source: Source::ChannelOwner,
            target: TargetResolver::Resolved("channel_owner"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ChannelCallerRoutes,
            target: TargetResolver::ResolvedCollection("channel_caller_routes"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native update owner, independent stored-channel Read and optional own-route content.
pub const CHANNEL_UPDATE: Declaration<Source> = Declaration {
    operation: "NotificationChannelUpdate",
    bindings: &[
        Binding {
            source: Source::ChannelOwner,
            target: TargetResolver::Resolved("channel_owner"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ChannelOwner,
            target: TargetResolver::Resolved("channel_owner"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ChannelCallerRoutes,
            target: TargetResolver::ResolvedCollection("channel_caller_routes"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
