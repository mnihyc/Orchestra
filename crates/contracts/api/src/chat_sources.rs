//! Actual chat and selected byte sources for the canonical chat operations.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Native facts before formatting any chat or file response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The real stored chat containing the requested message or private draft.
    Chat,
    /// Selected saved chat witnesses for private draft bytes.
    ByteChats,
    /// Selected saved thread witnesses for private draft bytes.
    ByteThreads,
    /// Selected saved document witnesses for private draft bytes.
    ByteDocuments,
    /// Selected environment-file witnesses for private draft bytes.
    ByteEnvironments,
    /// The acting account's selected retained-upload custody.
    ByteUploads,
    /// The actual root org at which the chat is opened.
    Parent,
    /// The native newly created or deduplicated existing chat.
    OpenedChat,
    /// Group identities named by the native inspection audience.
    InspectionGroups,
    /// Current org-name sources of a native conversation projection.
    NameOrgs,
    /// Current project-name sources of a native conversation projection.
    NameProjects,
    /// Current topic-name sources of a native conversation projection.
    NameTopics,
    /// Current thread-name sources of a native conversation projection.
    NameThreads,
    /// Current optional group names selected by the native message producer.
    NameGroups,
    /// Actual native rows in a member or inspection list page.
    ListResults,
    /// The exact readable and followed native collection selected for a personal read-position act.
    AllReadChats,
}

/// Saved messages need their chat; private drafts also retain their actual byte witness.
pub const BLOB_GET: Declaration<Source> = Declaration {
    operation: "ChatBlobGet",
    bindings: &[
        Binding {
            source: Source::Chat,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ByteChats,
            target: TargetResolver::ResolvedCollection("message_source_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ByteThreads,
            target: TargetResolver::ResolvedCollection("message_source_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ByteDocuments,
            target: TargetResolver::ResolvedCollection("message_source_documents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ByteEnvironments,
            target: TargetResolver::ResolvedCollection("message_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ByteUploads,
            target: TargetResolver::ResolvedCollection("message_retained_uploads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The canonical actions remain here; the source row only connects their actual roles.
#[must_use]
pub const fn blob_read_facets() -> [FacetSpec; 6] {
    use KeyIdentityKind as I;
    use KeyTargetKind::{Identity, Object};
    [
        FacetSpec::required(
            KeyAction::Read,
            &[Identity(I::Chat)],
            TargetResolver::Path("id"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[Identity(I::Chat)],
            TargetResolver::ResolvedCollection("message_source_chats"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection("message_source_threads"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[Object(ObjectKind::Document)],
            TargetResolver::ResolvedCollection("message_source_documents"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[Object(ObjectKind::Environment)],
            TargetResolver::ResolvedCollection("message_source_environments"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[Identity(I::Upload)],
            TargetResolver::ResolvedCollection("message_retained_uploads"),
        ),
    ]
}

/// One opening row; effect and existing-content response are independent.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "OrgChatCreate",
    bindings: &[
        Binding {
            source: Source::Parent,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OpenedChat,
            target: TargetResolver::Resolved("created_or_existing_chat"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::InspectionGroups,
            target: TargetResolver::ResolvedCollection("chat_inspection_groups"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameOrgs,
            target: TargetResolver::ResolvedCollection("message_name_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameProjects,
            target: TargetResolver::ResolvedCollection("message_name_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameTopics,
            target: TargetResolver::ResolvedCollection("message_name_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameThreads,
            target: TargetResolver::ResolvedCollection("message_name_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameGroups,
            target: TargetResolver::ResolvedCollection("message_name_groups"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native opening at its parent, with the existing conversation's actual response sources.
#[must_use]
pub const fn creation_facets() -> [FacetSpec; 8] {
    use KeyIdentityKind as I;
    use KeyTargetKind::{Identity, Object};
    [
        FacetSpec::required(
            KeyAction::Write,
            &[Identity(I::Chat)],
            TargetResolver::Path("id"),
        ),
        FacetSpec::response_content(
            &[Identity(I::Chat)],
            TargetResolver::Resolved("created_or_existing_chat"),
        ),
        FacetSpec::response_content(
            &[Identity(I::Group)],
            TargetResolver::ResolvedCollection("chat_inspection_groups"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Org)],
            TargetResolver::ResolvedCollection("message_name_orgs"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Project)],
            TargetResolver::ResolvedCollection("message_name_projects"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Topic)],
            TargetResolver::ResolvedCollection("message_name_topics"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection("message_name_threads"),
        ),
        FacetSpec::response_content(
            &[Identity(I::Group)],
            TargetResolver::ResolvedCollection("message_name_groups"),
        ),
    ]
}

const LIST_BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::ListResults,
        target: TargetResolver::ResolvedCollection("results"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::NameOrgs,
        target: TargetResolver::ResolvedCollection("message_name_orgs"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::NameProjects,
        target: TargetResolver::ResolvedCollection("message_name_projects"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::NameTopics,
        target: TargetResolver::ResolvedCollection("message_name_topics"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::NameThreads,
        target: TargetResolver::ResolvedCollection("message_name_threads"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];

/// Ordinary member/guest visibility, selected by the native query intent.
pub const LIST_MEMBER: Declaration<Source> = Declaration {
    operation: "OrgChatList.member",
    bindings: LIST_BINDINGS,
};
/// Privileged inspection retains its existing independent Manage action.
pub const LIST_INSPECT: Declaration<Source> = Declaration {
    operation: "OrgChatList.inspect",
    bindings: LIST_BINDINGS,
};

/// The selected native page and optional live place/author names.
#[must_use]
pub const fn list_facets(action: KeyAction) -> [FacetSpec; 5] {
    use KeyTargetKind::{Identity, Object};
    [
        FacetSpec::required(
            action,
            &[Identity(KeyIdentityKind::Chat)],
            TargetResolver::ResolvedCollection("results"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Org)],
            TargetResolver::ResolvedCollection("message_name_orgs"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Project)],
            TargetResolver::ResolvedCollection("message_name_projects"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Topic)],
            TargetResolver::ResolvedCollection("message_name_topics"),
        ),
        FacetSpec::response_content(
            &[Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection("message_name_threads"),
        ),
    ]
}

/// The native collection is fixed before the rollbackable personal-state row prefix.
pub const READ_ALL: Declaration<Source> = Declaration {
    operation: "OrgChatsRead",
    bindings: &[Binding {
        source: Source::AllReadChats,
        target: TargetResolver::ResolvedCollection("org_chats"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};

/// Actual chat export and optional current node-label sources, before any bytes are produced.
pub const EXPORT_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Chat)],
        TargetResolver::Resolved("chat"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("chat_export_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("chat_export_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("chat_export_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("chat_export_threads"),
    ),
];
pub const EXPORT: Declaration<Source> = Declaration {
    operation: "ChatExportGet",
    bindings: &[
        Binding {
            source: Source::Chat,
            target: TargetResolver::Resolved("chat"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::NameOrgs,
            target: TargetResolver::ResolvedCollection("chat_export_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameProjects,
            target: TargetResolver::ResolvedCollection("chat_export_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameTopics,
            target: TargetResolver::ResolvedCollection("chat_export_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameThreads,
            target: TargetResolver::ResolvedCollection("chat_export_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
