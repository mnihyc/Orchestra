//! Native document source rows for content windows and metadata projections.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::FacetSpec;
use crate::operations::{FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Source roles produced by the native history content window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistorySource {
    /// Stored content and its existing live/retained governing identity.
    Document,
    /// Emitted organization labels.
    Orgs,
    /// Emitted project labels.
    Projects,
    /// Emitted topic labels.
    Topics,
    /// Emitted thread and live agent-attribution labels.
    Threads,
    /// Live titles supplied by referenced documents.
    ReferencedDocuments,
    /// Live titles supplied by referenced chats.
    ReferencedChats,
}

const HISTORY: &[Binding<HistorySource>] = &[
    Binding {
        source: HistorySource::Document,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: HistorySource::Orgs,
        target: TargetResolver::ResolvedCollection("document_names_orgs"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: HistorySource::Projects,
        target: TargetResolver::ResolvedCollection("document_names_projects"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: HistorySource::Topics,
        target: TargetResolver::ResolvedCollection("document_names_topics"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: HistorySource::Threads,
        target: TargetResolver::ResolvedCollection("document_names_threads"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: HistorySource::ReferencedDocuments,
        target: TargetResolver::ResolvedCollection("document_reference_documents"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: HistorySource::ReferencedChats,
        target: TargetResolver::ResolvedCollection("document_reference_chats"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];

/// The native inspection selector chooses one exact canonical variant.
pub struct History {
    /// The ordinary native read intent.
    pub read: Declaration<HistorySource>,
    /// The explicit retained-content inspection intent.
    pub inspect: Declaration<HistorySource>,
}

const fn history(read: &'static str, inspect: &'static str) -> History {
    History {
        read: Declaration {
            operation: read,
            bindings: HISTORY,
        },
        inspect: Declaration {
            operation: inspect,
            bindings: HISTORY,
        },
    }
}

/// Native version page sources.
pub const VERSIONS: History = history(
    "DocumentVersionList.read",
    "DocumentVersionList.inspect_deleted",
);
/// Native historical snapshot sources.
pub const VERSION: History = history(
    "DocumentVersionGet.read",
    "DocumentVersionGet.inspect_deleted",
);
/// Native before/after comparison sources, including both snapshots and removers.
pub const CHANGES: History = history(
    "DocumentChangesGet.read",
    "DocumentChangesGet.inspect_deleted",
);

/// Source roles supplied by a native document/folder entry projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrySource {
    /// The existing native directory chosen for a creation effect.
    Destination,
    /// The exact stored text or folder identity, never its hierarchy anchor.
    Entry,
    /// Organization labels in the native place/provenance projection.
    Orgs,
    /// Project labels in the native place/provenance projection.
    Projects,
    /// Topic labels in the native place/provenance projection.
    Topics,
    /// Thread and live attribution labels in the native projection.
    Threads,
    /// Names of actual ancestor folders, separate from the selected entry's own title.
    Containers,
}

/// Stable native role names shared by facets and the source adapter.
pub const ENTRY_NODE_ROLES: [&str; 4] = [
    "entry_names_orgs",
    "entry_names_projects",
    "entry_names_topics",
    "entry_names_threads",
];
/// The native ancestor-folder label collection.
pub const ENTRY_CONTAINERS: &str = "entry_names_containers";

/// Compose the native entry's own action with its independently protected labels.
pub const fn entry_facets(
    action: KeyAction,
    kinds: &'static [KeyTargetKind],
    primary: TargetResolver,
) -> [FacetSpec; 6] {
    [
        FacetSpec::required(action, kinds, primary),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Org)],
            TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[0]),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Project)],
            TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[1]),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Topic)],
            TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[2]),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[3]),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
            TargetResolver::ResolvedCollection(ENTRY_CONTAINERS),
        ),
    ]
}

const fn entry_bindings(primary: TargetResolver) -> [Binding<EntrySource>; 6] {
    [
        Binding {
            source: EntrySource::Entry,
            target: primary,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: EntrySource::Orgs,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[0]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Projects,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[1]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Topics,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[2]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Threads,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[3]),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Containers,
            target: TargetResolver::ResolvedCollection(ENTRY_CONTAINERS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ]
}

const CONTAINER_BINDINGS: [Binding<EntrySource>; 6] = entry_bindings(TargetResolver::TypedPath {
    parameter: "id",
    kind: KeyTargetKind::Object(ObjectKind::DocumentContainer),
});
const ENTRY_BINDINGS: [Binding<EntrySource>; 6] =
    entry_bindings(TargetResolver::Resolved("typed_entry"));

/// Exact live/retained native intents for an entry reader.
pub struct EntryRead {
    /// Ordinary metadata viewing.
    pub read: Declaration<EntrySource>,
    /// Explicit retained-content viewing under current restoration authority.
    pub inspect: Declaration<EntrySource>,
}

/// The folder-only entry row.
pub const CONTAINER_GET: EntryRead = EntryRead {
    read: Declaration {
        operation: "DocumentContainerGet.read",
        bindings: &CONTAINER_BINDINGS,
    },
    inspect: Declaration {
        operation: "DocumentContainerGet.inspect_deleted",
        bindings: &CONTAINER_BINDINGS,
    },
};
/// The typed document-or-folder entry row.
pub const ENTRY_GET: EntryRead = EntryRead {
    read: Declaration {
        operation: "DocumentTreeEntryGet.read",
        bindings: &ENTRY_BINDINGS,
    },
    inspect: Declaration {
        operation: "DocumentTreeEntryGet.inspect_deleted",
        bindings: &ENTRY_BINDINGS,
    },
};

/// Source roles of the native readable/private document-list window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListSource {
    /// Both readable documents and the native administrator's minimal private rows.
    Documents,
    /// Organization labels actually named by the selected references.
    Orgs,
    /// Project labels actually named by the selected references.
    Projects,
    /// Topic labels actually named by the selected references.
    Topics,
    /// Thread and live attribution labels actually named by the selected references.
    Threads,
}

const LIST_BINDINGS: &[Binding<ListSource>] = &[
    Binding {
        source: ListSource::Documents,
        target: TargetResolver::ResolvedCollection("listed_documents"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: ListSource::Orgs,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[0]),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: ListSource::Projects,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[1]),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: ListSource::Topics,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[2]),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: ListSource::Threads,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[3]),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];

/// Compose the list's selected-content action and its separately protected live labels.
pub const fn list_facets(action: KeyAction) -> [FacetSpec; 5] {
    let entry = entry_facets(
        action,
        &[KeyTargetKind::Object(ObjectKind::Document)],
        TargetResolver::ResolvedCollection("listed_documents"),
    );
    [entry[0], entry[1], entry[2], entry[3], entry[4]]
}

/// Exact native live/deleted selectors for the document list.
pub const DOCUMENT_LIST: EntryReadList = EntryReadList {
    read: Declaration {
        operation: "NodeDocumentList.read",
        bindings: LIST_BINDINGS,
    },
    deleted: Declaration {
        operation: "NodeDocumentList.deleted",
        bindings: LIST_BINDINGS,
    },
};

/// Transport aggregation consumes the same native document window under its own operation.
/// Names are optional content so a genuinely empty document selection needs no name-source grant.
pub const DOCUMENT_PAGES: Declaration<ListSource> = Declaration {
    operation: "NodeDocumentPagesGet",
    bindings: &[
        LIST_BINDINGS[0],
        Binding {
            usage: FacetUsage::ResponseContent,
            ..LIST_BINDINGS[1]
        },
        Binding {
            usage: FacetUsage::ResponseContent,
            ..LIST_BINDINGS[2]
        },
        Binding {
            usage: FacetUsage::ResponseContent,
            ..LIST_BINDINGS[3]
        },
        Binding {
            usage: FacetUsage::ResponseContent,
            ..LIST_BINDINGS[4]
        },
    ],
};

/// The aggregate's real document collection and its separately admitted native labels.
pub const fn pages_facets() -> [FacetSpec; 5] {
    let row = list_facets(KeyAction::Read);
    [
        row[0],
        FacetSpec::response_content(row[1].resources, row[1].target),
        FacetSpec::response_content(row[2].resources, row[2].target),
        FacetSpec::response_content(row[3].resources, row[3].target),
        FacetSpec::response_content(row[4].resources, row[4].target),
    ]
}

/// Folder creation uses its real existing directory and optional native entry projection.
pub const FOLDER_CREATE: Declaration<EntrySource> = Declaration {
    operation: "DocumentContainerCreate",
    bindings: &[
        Binding {
            source: EntrySource::Destination,
            target: TargetResolver::Resolved("destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: EntrySource::Entry,
            target: TargetResolver::Resolved("created_entry"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: EntrySource::Orgs,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Projects,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Topics,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Threads,
            target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: EntrySource::Containers,
            target: TargetResolver::ResolvedCollection(ENTRY_CONTAINERS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Compose the original creation act with its actual optional native source roles.
pub const fn folder_create_facets() -> [FacetSpec; 7] {
    let entry = entry_facets(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
        TargetResolver::Resolved("created_entry"),
    );
    [
        FacetSpec::required(
            KeyAction::Write,
            &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
            TargetResolver::Resolved("destination"),
        ),
        FacetSpec::response_content(entry[0].resources, entry[0].target),
        FacetSpec::response_content(entry[1].resources, entry[1].target),
        FacetSpec::response_content(entry[2].resources, entry[2].target),
        FacetSpec::response_content(entry[3].resources, entry[3].target),
        FacetSpec::response_content(entry[4].resources, entry[4].target),
        FacetSpec::response_content(entry[5].resources, entry[5].target),
    ]
}

/// A deleted list uses its native management gate; labels keep their own Read action.
pub struct EntryReadList {
    /// The ordinary list of live documents.
    pub read: Declaration<ListSource>,
    /// The native list of restorable deleted documents.
    pub deleted: Declaration<ListSource>,
}

/// Native text-creation destinations, copied labels and optional completed metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextCreationSource {
    /// The actual selected Chat custody of imported bytes.
    UploadChats,
    /// The actual selected Thread custody of imported bytes.
    UploadThreads,
    /// The actual selected Document custody of imported bytes.
    UploadDocuments,
    /// The actual selected Environment-files custody of imported bytes.
    UploadEnvironments,
    /// The acting Account retaining the actual imported upload bytes.
    Uploads,
    /// Existing native original documents actually read for a new copy; completed replay is empty.
    Originals,
    /// The actual existing directory selected by this native creator.
    Destination,
    /// Organization labels copied into newly stored content.
    WrittenOrgs,
    /// Project labels copied into newly stored content.
    WrittenProjects,
    /// Topic labels copied into newly stored content.
    WrittenTopics,
    /// Thread attribution labels copied into newly stored content.
    WrittenThreads,
    /// Document labels copied into newly stored content.
    WrittenDocuments,
    /// Chat labels copied into newly stored content.
    WrittenChats,
    /// The real completed text document.
    Created,
    /// Organization labels in completed native document metadata.
    Orgs,
    /// Project labels in completed native document metadata.
    Projects,
    /// Topic labels in completed native document metadata.
    Topics,
    /// Thread attribution labels in completed native document metadata.
    Threads,
    /// Ancestor-folder names in completed native document metadata.
    Containers,
}

const TEXT_CREATION: &[Binding<TextCreationSource>] = &[
    Binding {
        source: TextCreationSource::Destination,
        target: TargetResolver::Resolved("destination"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: TextCreationSource::WrittenOrgs,
        target: TargetResolver::ResolvedCollection("reference_orgs"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::WrittenProjects,
        target: TargetResolver::ResolvedCollection("reference_projects"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::WrittenTopics,
        target: TargetResolver::ResolvedCollection("reference_topics"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::WrittenThreads,
        target: TargetResolver::ResolvedCollection("reference_threads"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::WrittenDocuments,
        target: TargetResolver::ResolvedCollection("reference_documents"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::WrittenChats,
        target: TargetResolver::ResolvedCollection("reference_chats"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::Created,
        target: TargetResolver::Resolved("created_document"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
    Binding {
        source: TextCreationSource::Orgs,
        target: TargetResolver::ResolvedCollection("document_names_orgs"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::Projects,
        target: TargetResolver::ResolvedCollection("document_names_projects"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::Topics,
        target: TargetResolver::ResolvedCollection("document_names_topics"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::Threads,
        target: TargetResolver::ResolvedCollection("document_names_threads"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::Containers,
        target: TargetResolver::ResolvedCollection("document_names_containers"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];

/// Text creation beside a hierarchy node.
pub const TEXT_NODE_CREATE: Declaration<TextCreationSource> = Declaration {
    operation: "NodeDocumentCreate",
    bindings: TEXT_CREATION,
};
/// Text creation inside its actual stored document-only folder.
pub const TEXT_CONTAINER_CREATE: Declaration<TextCreationSource> = Declaration {
    operation: "DocumentContainerDocumentCreate",
    bindings: TEXT_CREATION,
};

/// New copy and completed replay share the native creator and completed-document sources.
pub const TEXT_COPY: Declaration<TextCreationSource> = Declaration {
    operation: "NodeDocumentCopy",
    bindings: &[
        Binding {
            source: TextCreationSource::Originals,
            target: TargetResolver::ResolvedCollection("copy_originals"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        TEXT_CREATION[0],
        TEXT_CREATION[1],
        TEXT_CREATION[2],
        TEXT_CREATION[3],
        TEXT_CREATION[4],
        TEXT_CREATION[5],
        TEXT_CREATION[6],
        TEXT_CREATION[7],
        TEXT_CREATION[8],
        TEXT_CREATION[9],
        TEXT_CREATION[10],
        TEXT_CREATION[11],
        TEXT_CREATION[12],
    ],
};

/// Original Read is an explicit native collection, including the true empty read set of replay.
pub const fn text_copy_facets() -> [FacetSpec; 14] {
    let created = text_create_facets();
    [
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Document)],
            TargetResolver::ResolvedCollection("copy_originals"),
        ),
        created[0],
        created[1],
        created[2],
        created[3],
        created[4],
        created[5],
        created[6],
        created[7],
        created[8],
        created[9],
        created[10],
        created[11],
        created[12],
    ]
}

/// The actual text act, safely selected copied labels and optional completed native metadata.
pub const fn text_create_facets() -> [FacetSpec; 13] {
    [
        FacetSpec::required(
            KeyAction::Write,
            &[KeyTargetKind::Object(ObjectKind::Document)],
            TargetResolver::Resolved("destination"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Org)],
            TargetResolver::ResolvedCollection("reference_orgs"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Project)],
            TargetResolver::ResolvedCollection("reference_projects"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Topic)],
            TargetResolver::ResolvedCollection("reference_topics"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection("reference_threads"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Document)],
            TargetResolver::ResolvedCollection("reference_documents"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Chat)],
            TargetResolver::ResolvedCollection("reference_chats"),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Document)],
            TargetResolver::Resolved("created_document"),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Org)],
            TargetResolver::ResolvedCollection("document_names_orgs"),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Project)],
            TargetResolver::ResolvedCollection("document_names_projects"),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Topic)],
            TargetResolver::ResolvedCollection("document_names_topics"),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection("document_names_threads"),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
            TargetResolver::ResolvedCollection("document_names_containers"),
        ),
    ]
}

/// Native source and actual hierarchy destination of a document move review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveSource {
    /// The actual stored source document, shared by its act and optional disclosure.
    Document,
    /// The actual existing node selected for creation after moving.
    Destination,
    /// The same real document selected from the completed movement, under its new governor.
    Result,

    /// Actual native full-document content origins (currently checked empty).
    References,
    /// Protected organization labels in the completed native document.
    Orgs,
    /// Protected project labels in the completed native document.
    Projects,
    /// Protected topic labels in the completed native document.
    Topics,
    /// Protected live thread/author labels in the completed native document.
    Threads,
    /// Protected ancestor-folder names in the completed native document.
    Containers,
}

/// The original compound review act with independent response Read.
pub const MOVE_PREVIEW_FACETS: [FacetSpec; 3] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Document)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Document)],
        TargetResolver::Resolved("destination"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Document)],
        TargetResolver::Path("id"),
    ),
];
/// One native declaration for the review's real compound operands.
pub const MOVE_PREVIEW: Declaration<MoveSource> = Declaration {
    operation: "DocumentMovePreview",
    bindings: &[
        Binding {
            source: MoveSource::Document,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: MoveSource::Document,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: MoveSource::Destination,
            target: TargetResolver::Resolved("destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};

/// Native completed movement singleton source role.
pub const MOVE_REFERENCES: &str = "moved_document_sources";
/// Native completed movement node-name roles in hierarchy kind order.
pub const MOVE_NODES: [&str; 4] = [
    "moved_document_orgs",
    "moved_document_projects",
    "moved_document_topics",
    "moved_document_threads",
];
/// Native completed movement ancestor-folder labels.
pub const MOVE_CONTAINERS: &str = "moved_document_containers";
/// The original movement act plus only its actual optional full-document sources.
pub const MOVE_FACETS: [FacetSpec; 9] = [
    MOVE_PREVIEW_FACETS[0],
    MOVE_PREVIEW_FACETS[1],
    MOVE_PREVIEW_FACETS[2],
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Document)],
        TargetResolver::ResolvedCollection(MOVE_REFERENCES),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(MOVE_NODES[0]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection(MOVE_NODES[1]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection(MOVE_NODES[2]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection(MOVE_NODES[3]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
        TargetResolver::ResolvedCollection(MOVE_CONTAINERS),
    ),
];
/// One native movement row; the same real source supplies effect and completed Read facts.
pub const MOVE: Declaration<MoveSource> = Declaration {
    operation: "DocumentMove",
    bindings: &[
        MOVE_PREVIEW.bindings[0],
        Binding {
            source: MoveSource::Result,
            ..MOVE_PREVIEW.bindings[1]
        },
        MOVE_PREVIEW.bindings[2],
        Binding {
            source: MoveSource::References,
            target: TargetResolver::ResolvedCollection(MOVE_REFERENCES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MoveSource::Orgs,
            target: TargetResolver::ResolvedCollection(MOVE_NODES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MoveSource::Projects,
            target: TargetResolver::ResolvedCollection(MOVE_NODES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MoveSource::Topics,
            target: TargetResolver::ResolvedCollection(MOVE_NODES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MoveSource::Threads,
            target: TargetResolver::ResolvedCollection(MOVE_NODES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: MoveSource::Containers,
            target: TargetResolver::ResolvedCollection(MOVE_CONTAINERS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Retained document mutation uses the same factual role shapes as movement.
pub type RestoreSource = MoveSource;
/// Native restored-document content-origin collection.
pub const RESTORE_REFERENCES: &str = "restored_document_sources";
/// Native restored-document node-name roles in hierarchy kind order.
pub const RESTORE_NODES: [&str; 4] = [
    "restored_document_orgs",
    "restored_document_projects",
    "restored_document_topics",
    "restored_document_threads",
];
/// Native restored-document ancestor-folder labels.
pub const RESTORE_CONTAINERS: &str = "restored_document_containers";
/// Restore keeps actual retained Manage and actual recorded-directory Write, then completed Read.
pub const RESTORE_FACETS: [FacetSpec; 9] = [
    MOVE_FACETS[0],
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Document)],
        TargetResolver::Resolved("restoration_destination"),
    ),
    MOVE_FACETS[2],
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Document)],
        TargetResolver::ResolvedCollection(RESTORE_REFERENCES),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(RESTORE_NODES[0]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection(RESTORE_NODES[1]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection(RESTORE_NODES[2]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection(RESTORE_NODES[3]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
        TargetResolver::ResolvedCollection(RESTORE_CONTAINERS),
    ),
];
/// One exact retained document preparation and completed projection declaration.
pub const RESTORE: Declaration<RestoreSource> = Declaration {
    operation: "DocumentRestore",
    bindings: &[
        MOVE.bindings[0],
        MOVE.bindings[1],
        Binding {
            source: RestoreSource::Destination,
            target: TargetResolver::Resolved("restoration_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: RestoreSource::References,
            target: TargetResolver::ResolvedCollection(RESTORE_REFERENCES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RestoreSource::Orgs,
            target: TargetResolver::ResolvedCollection(RESTORE_NODES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RestoreSource::Projects,
            target: TargetResolver::ResolvedCollection(RESTORE_NODES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RestoreSource::Topics,
            target: TargetResolver::ResolvedCollection(RESTORE_NODES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RestoreSource::Threads,
            target: TargetResolver::ResolvedCollection(RESTORE_NODES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RestoreSource::Containers,
            target: TargetResolver::ResolvedCollection(RESTORE_CONTAINERS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native directory, mixed selected rows and protected labels of a tree page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeSource {
    /// Actual existing directory; NodeDocs uses its real node permission object.
    Directory,
    /// Actual selected readable or retained-managed documents and folders.
    Entries,
    /// Named organization references.
    Orgs,
    /// Named project references.
    Projects,
    /// Named topic references.
    Topics,
    /// Named thread and attribution references.
    Threads,
    /// Named structural ancestor folders, excluding intrinsic primary items.
    Containers,
}
const TREE_BINDINGS: &[Binding<TreeSource>] = &[
    Binding {
        source: TreeSource::Directory,
        target: TargetResolver::Resolved("directory"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: TreeSource::Entries,
        target: TargetResolver::ResolvedCollection("tree_entries"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TreeSource::Orgs,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[0]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TreeSource::Projects,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[1]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TreeSource::Topics,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[2]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TreeSource::Threads,
        target: TargetResolver::ResolvedCollection(ENTRY_NODE_ROLES[3]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TreeSource::Containers,
        target: TargetResolver::ResolvedCollection(ENTRY_CONTAINERS),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
/// Native live and retained directory windows keep the same canonical selectors.
pub struct TreeList {
    /// Ordinary live entries under current viewing authority.
    pub read: Declaration<TreeSource>,
    /// Retained entries under current restoration authority.
    pub inspect: Declaration<TreeSource>,
}
/// The mixed native tree-page source declaration.
pub const TREE_LIST: TreeList = TreeList {
    read: Declaration {
        operation: "DocumentTreeList.view",
        bindings: TREE_BINDINGS,
    },
    inspect: Declaration {
        operation: "DocumentTreeList.retained",
        bindings: TREE_BINDINGS,
    },
};
/// The existing directory and its real mixed row collection have distinct cardinality.
pub const fn tree_facets(action: KeyAction) -> [FacetSpec; 7] {
    let mut entry = entry_facets(
        action,
        &[
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
        ],
        TargetResolver::ResolvedCollection("tree_entries"),
    );
    entry[1].usage = FacetUsage::ResponseContent;
    entry[2].usage = FacetUsage::ResponseContent;
    entry[3].usage = FacetUsage::ResponseContent;
    entry[4].usage = FacetUsage::ResponseContent;
    entry[5].usage = FacetUsage::ResponseContent;
    [
        FacetSpec::required(
            action,
            &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
            TargetResolver::Resolved("directory"),
        ),
        entry[0],
        entry[1],
        entry[2],
        entry[3],
        entry[4],
        entry[5],
    ]
}

/// Complete native membership of a document-tree management preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeManagementSource {
    /// Actual selected roots, admitted before native recursive preparation.
    Selected,
    /// All real affected documents and containers, without an omitted descendant tail.
    Affected,
}

/// Management and descriptive disclosure are independent on the same complete membership.
pub const TREE_DELETE_PREVIEW_FACETS: [FacetSpec; 3] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
        ],
        TargetResolver::ResolvedCollection("selected_entries"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
        ],
        TargetResolver::ResolvedCollection("affected_entries"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
        ],
        TargetResolver::ResolvedCollection("affected_entries"),
    ),
];
/// The one actual native closure supplies both roles with explicit mixed cardinality.
pub const TREE_DELETE_PREVIEW: Declaration<TreeManagementSource> = Declaration {
    operation: "DocumentTreeDeletePreview",
    bindings: &[
        Binding {
            source: TreeManagementSource::Selected,
            target: TargetResolver::ResolvedCollection("selected_entries"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: TreeManagementSource::Affected,
            target: TargetResolver::ResolvedCollection("affected_entries"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: TreeManagementSource::Affected,
            target: TargetResolver::ResolvedCollection("affected_entries"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Actual native compound-tree inputs and independently readable completed result metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationSource {
    /// Actual selected roots, or the same stored roots of a genuine native replay.
    Selected,
    /// Complete actual native affected closure, never a top-root substitute.
    Affected,
    /// Actual existing creation/movement or recorded restoration directories.
    Destinations,
    /// Actual committed native entries, independent of earlier input authority.
    Results,
    /// Existing native audience/copy inputs supplying completed outcome metadata.
    Contexts,
    /// Protected organization labels.
    Orgs,
    /// Protected project labels.
    Projects,
    /// Protected topic labels.
    Topics,
    /// Protected thread labels.
    Threads,
    /// Actual named ancestor document folders.
    Containers,
}
pub const OPERATION_SELECTED: &str = "operation_selected";
pub const OPERATION_AFFECTED: &str = "operation_affected";
pub const OPERATION_DESTINATIONS: &str = "operation_destinations";
pub const OPERATION_RESULTS: &str = "operation_results";
pub const OPERATION_CONTEXTS: &str = "operation_contexts";
pub const OPERATION_NODES: [&str; 4] = [
    "operation_orgs",
    "operation_projects",
    "operation_topics",
    "operation_threads",
];
pub const OPERATION_CONTAINERS: &str = "operation_containers";

const OPERATION_ENTRY_KINDS: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Document),
    KeyTargetKind::Object(ObjectKind::DocumentContainer),
];
const OPERATION_TEXT_KIND: &[KeyTargetKind] = &[KeyTargetKind::Object(ObjectKind::Document)];
const OPERATION_FOLDER_KIND: &[KeyTargetKind] =
    &[KeyTargetKind::Object(ObjectKind::DocumentContainer)];

const fn operation_response_facets(kinds: &'static [KeyTargetKind]) -> [FacetSpec; 7] {
    [
        FacetSpec::response_content(kinds, TargetResolver::ResolvedCollection(OPERATION_RESULTS)),
        FacetSpec::response_content(
            kinds,
            TargetResolver::ResolvedCollection(OPERATION_CONTEXTS),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Org)],
            TargetResolver::ResolvedCollection(OPERATION_NODES[0]),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Project)],
            TargetResolver::ResolvedCollection(OPERATION_NODES[1]),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Topic)],
            TargetResolver::ResolvedCollection(OPERATION_NODES[2]),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection(OPERATION_NODES[3]),
        ),
        FacetSpec::response_content(
            OPERATION_FOLDER_KIND,
            TargetResolver::ResolvedCollection(OPERATION_CONTAINERS),
        ),
    ]
}
const fn operation_sources_facets(
    action: KeyAction,
    kinds: &'static [KeyTargetKind],
) -> [FacetSpec; 2] {
    [
        FacetSpec::required(
            action,
            kinds,
            TargetResolver::ResolvedCollection(OPERATION_SELECTED),
        ),
        FacetSpec::required(
            action,
            kinds,
            TargetResolver::ResolvedCollection(OPERATION_AFFECTED),
        ),
    ]
}
const fn operation_with_destination_facets(action: KeyAction) -> [FacetSpec; 10] {
    let source = operation_sources_facets(action, OPERATION_ENTRY_KINDS);
    let response = operation_response_facets(OPERATION_ENTRY_KINDS);
    [
        source[0],
        source[1],
        FacetSpec::required(
            KeyAction::Write,
            OPERATION_ENTRY_KINDS,
            TargetResolver::ResolvedCollection(OPERATION_DESTINATIONS),
        ),
        response[0],
        response[1],
        response[2],
        response[3],
        response[4],
        response[5],
        response[6],
    ]
}
const fn operation_local_facets(
    action: KeyAction,
    kinds: &'static [KeyTargetKind],
) -> [FacetSpec; 9] {
    let source = operation_sources_facets(action, kinds);
    let response = operation_response_facets(kinds);
    [
        source[0],
        source[1],
        response[0],
        response[1],
        response[2],
        response[3],
        response[4],
        response[5],
        response[6],
    ]
}
const fn operation_folder_facets() -> [FacetSpec; 8] {
    let response = operation_response_facets(OPERATION_FOLDER_KIND);
    [
        FacetSpec::required(
            KeyAction::Write,
            OPERATION_FOLDER_KIND,
            TargetResolver::ResolvedCollection(OPERATION_DESTINATIONS),
        ),
        response[0],
        response[1],
        response[2],
        response[3],
        response[4],
        response[5],
        response[6],
    ]
}
const fn operation_binding(
    source: OperationSource,
    role: &'static str,
    usage: FacetUsage,
) -> Binding<OperationSource> {
    Binding {
        source,
        target: TargetResolver::ResolvedCollection(role),
        usage,
        cardinality: Cardinality::Collection,
    }
}
const OPERATION_RESPONSE_BINDINGS: [Binding<OperationSource>; 7] = [
    operation_binding(
        OperationSource::Results,
        OPERATION_RESULTS,
        FacetUsage::ResponseContent,
    ),
    operation_binding(
        OperationSource::Contexts,
        OPERATION_CONTEXTS,
        FacetUsage::ResponseContent,
    ),
    operation_binding(
        OperationSource::Orgs,
        OPERATION_NODES[0],
        FacetUsage::ResponseContent,
    ),
    operation_binding(
        OperationSource::Projects,
        OPERATION_NODES[1],
        FacetUsage::ResponseContent,
    ),
    operation_binding(
        OperationSource::Topics,
        OPERATION_NODES[2],
        FacetUsage::ResponseContent,
    ),
    operation_binding(
        OperationSource::Threads,
        OPERATION_NODES[3],
        FacetUsage::ResponseContent,
    ),
    operation_binding(
        OperationSource::Containers,
        OPERATION_CONTAINERS,
        FacetUsage::ResponseContent,
    ),
];
const OPERATION_SELECTED_BINDING: Binding<OperationSource> = operation_binding(
    OperationSource::Selected,
    OPERATION_SELECTED,
    FacetUsage::Required,
);
const OPERATION_AFFECTED_BINDING: Binding<OperationSource> = operation_binding(
    OperationSource::Affected,
    OPERATION_AFFECTED,
    FacetUsage::Required,
);
const OPERATION_DESTINATION_BINDING: Binding<OperationSource> = operation_binding(
    OperationSource::Destinations,
    OPERATION_DESTINATIONS,
    FacetUsage::Required,
);
const OPERATION_WITH_DESTINATION_BINDINGS: [Binding<OperationSource>; 10] = [
    OPERATION_SELECTED_BINDING,
    OPERATION_AFFECTED_BINDING,
    OPERATION_DESTINATION_BINDING,
    OPERATION_RESPONSE_BINDINGS[0],
    OPERATION_RESPONSE_BINDINGS[1],
    OPERATION_RESPONSE_BINDINGS[2],
    OPERATION_RESPONSE_BINDINGS[3],
    OPERATION_RESPONSE_BINDINGS[4],
    OPERATION_RESPONSE_BINDINGS[5],
    OPERATION_RESPONSE_BINDINGS[6],
];
const OPERATION_LOCAL_BINDINGS: [Binding<OperationSource>; 9] = [
    OPERATION_SELECTED_BINDING,
    OPERATION_AFFECTED_BINDING,
    OPERATION_RESPONSE_BINDINGS[0],
    OPERATION_RESPONSE_BINDINGS[1],
    OPERATION_RESPONSE_BINDINGS[2],
    OPERATION_RESPONSE_BINDINGS[3],
    OPERATION_RESPONSE_BINDINGS[4],
    OPERATION_RESPONSE_BINDINGS[5],
    OPERATION_RESPONSE_BINDINGS[6],
];
const OPERATION_FOLDER_BINDINGS: [Binding<OperationSource>; 8] = [
    OPERATION_DESTINATION_BINDING,
    OPERATION_RESPONSE_BINDINGS[0],
    OPERATION_RESPONSE_BINDINGS[1],
    OPERATION_RESPONSE_BINDINGS[2],
    OPERATION_RESPONSE_BINDINGS[3],
    OPERATION_RESPONSE_BINDINGS[4],
    OPERATION_RESPONSE_BINDINGS[5],
    OPERATION_RESPONSE_BINDINGS[6],
];

/// Canonical native create folder intent and actual completed disclosure.
pub const OPERATION_CREATE_FOLDER_FACETS: [FacetSpec; 8] = operation_folder_facets();
/// One factual native create folder declaration, consumed by the shared adapter.
pub const OPERATION_CREATE_FOLDER: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.create_folder",
    bindings: &OPERATION_FOLDER_BINDINGS,
};

/// Canonical native copy intent and actual completed disclosure.
pub const OPERATION_COPY_FACETS: [FacetSpec; 10] =
    operation_with_destination_facets(KeyAction::Read);
/// One factual native copy declaration, consumed by the shared adapter.
pub const OPERATION_COPY: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.copy",
    bindings: &OPERATION_WITH_DESTINATION_BINDINGS,
};

/// Canonical native move intent and actual completed disclosure.
pub const OPERATION_MOVE_FACETS: [FacetSpec; 10] =
    operation_with_destination_facets(KeyAction::Manage);
/// One factual native move declaration, consumed by the shared adapter.
pub const OPERATION_MOVE: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.move",
    bindings: &OPERATION_WITH_DESTINATION_BINDINGS,
};

/// Owner-only cross-root copy and complete native source deletion in one intent.
pub const OPERATION_MOVE_OUT_FACETS: [FacetSpec; 10] =
    operation_with_destination_facets(KeyAction::Manage);
pub const OPERATION_MOVE_OUT: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.move_out",
    bindings: &OPERATION_WITH_DESTINATION_BINDINGS,
};

/// Canonical native rename intent and actual completed disclosure.
pub const OPERATION_RENAME_FACETS: [FacetSpec; 9] =
    operation_local_facets(KeyAction::Write, OPERATION_TEXT_KIND);
/// One factual native rename declaration, consumed by the shared adapter.
pub const OPERATION_RENAME: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.rename",
    bindings: &OPERATION_LOCAL_BINDINGS,
};

/// Canonical native delete intent and actual completed disclosure.
pub const OPERATION_DELETE_FACETS: [FacetSpec; 9] =
    operation_local_facets(KeyAction::Manage, OPERATION_ENTRY_KINDS);
/// One factual native delete declaration, consumed by the shared adapter.
pub const OPERATION_DELETE: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.delete",
    bindings: &OPERATION_LOCAL_BINDINGS,
};

/// Canonical native restore intent and actual completed disclosure.
pub const OPERATION_RESTORE_FACETS: [FacetSpec; 10] =
    operation_with_destination_facets(KeyAction::Manage);
/// One factual native restore declaration, consumed by the shared adapter.
pub const OPERATION_RESTORE: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.restore",
    bindings: &OPERATION_WITH_DESTINATION_BINDINGS,
};

/// Canonical native restore version intent and actual completed disclosure.
pub const OPERATION_RESTORE_VERSION_FACETS: [FacetSpec; 9] =
    operation_local_facets(KeyAction::Manage, OPERATION_TEXT_KIND);
/// One factual native restore version declaration, consumed by the shared adapter.
pub const OPERATION_RESTORE_VERSION: Declaration<OperationSource> = Declaration {
    operation: "DocumentTreeOperate.restore_version",
    bindings: &OPERATION_LOCAL_BINDINGS,
};

/// Actual transient conversion actor/document and protected diagnostic roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionSource {
    /// The signed-in Account for supplied-content conversion.
    Account,
    /// The actual contextual Document and completed directory-derived result.
    Document,
    /// The actual node setting an applicable operational file limit.
    LimitNodes,
}
pub const CONVERSION_DOCUMENT: TargetResolver = TargetResolver::Resolved("conversion_document");
pub const CONVERSION_LIMIT_NODES: &str = "conversion_limit_nodes";
/// Supplied conversion reads no existing document or attachment context.
pub const CONVERSION_SUPPLIED_FACETS: [FacetSpec; 1] = [FacetSpec::required(
    KeyAction::Write,
    &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
    TargetResolver::CurrentAccount,
)];
/// Contextual conversion necessarily resolves the existing native attachment directory.
pub const CONVERSION_DOCUMENT_FACETS: [FacetSpec; 4] = [
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Document)],
        CONVERSION_DOCUMENT,
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Document)],
        CONVERSION_DOCUMENT,
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Document)],
        CONVERSION_DOCUMENT,
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::ResolvedCollection(CONVERSION_LIMIT_NODES),
    ),
];
/// One actual Account binding, with no fabricated document target.
pub const CONVERSION_SUPPLIED: Declaration<ConversionSource> = Declaration {
    operation: "DocumentConvert.supplied",
    bindings: &[Binding {
        source: ConversionSource::Account,
        target: TargetResolver::CurrentAccount,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
/// One actual contextual Document binds both native source actions independently of completed Read.
pub const CONVERSION_DOCUMENT_ROW: Declaration<ConversionSource> = Declaration {
    operation: "DocumentConvert.document",
    bindings: &[
        Binding {
            source: ConversionSource::Document,
            target: CONVERSION_DOCUMENT,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: ConversionSource::Document,
            target: CONVERSION_DOCUMENT,
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: ConversionSource::LimitNodes,
            target: TargetResolver::ResolvedCollection(CONVERSION_LIMIT_NODES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
/// A named limit is one actual refusal disclosure, never a fabricated successful conversion.
pub const CONVERSION_LIMIT: Declaration<ConversionSource> = Declaration {
    operation: "DocumentConvert.document",
    bindings: &[Binding {
        source: ConversionSource::LimitNodes,
        target: TargetResolver::ResolvedCollection(CONVERSION_LIMIT_NODES),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    }],
};

/// Import source custody uses the same native byte-source roles as the other creators.
const IMPORT_BYTES: [Binding<TextCreationSource>; 5] = [
    Binding {
        source: TextCreationSource::UploadChats,
        target: TargetResolver::ResolvedCollection("message_source_chats"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::UploadThreads,
        target: TargetResolver::ResolvedCollection("message_source_threads"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::UploadDocuments,
        target: TargetResolver::ResolvedCollection("message_source_documents"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::UploadEnvironments,
        target: TargetResolver::ResolvedCollection("message_source_environments"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: TextCreationSource::Uploads,
        target: TargetResolver::ResolvedCollection("message_retained_uploads"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];
/// The complete known footprint before untrusted import source IO.
pub const TEXT_IMPORT_INPUTS: Declaration<TextCreationSource> = Declaration {
    operation: "NodeDocumentImport",
    bindings: &[
        TEXT_CREATION[0],
        IMPORT_BYTES[0],
        IMPORT_BYTES[1],
        IMPORT_BYTES[2],
        IMPORT_BYTES[3],
        IMPORT_BYTES[4],
    ],
};
/// Native partial imports retain one explicit completed Document collection.
pub const TEXT_IMPORT: Declaration<TextCreationSource> = Declaration {
    operation: "NodeDocumentImport",
    bindings: &[
        TEXT_CREATION[0],
        TEXT_CREATION[1],
        TEXT_CREATION[2],
        TEXT_CREATION[3],
        TEXT_CREATION[4],
        TEXT_CREATION[5],
        TEXT_CREATION[6],
        IMPORT_BYTES[0],
        IMPORT_BYTES[1],
        IMPORT_BYTES[2],
        IMPORT_BYTES[3],
        IMPORT_BYTES[4],
        Binding {
            source: TextCreationSource::Created,
            target: TargetResolver::ResolvedCollection("created_documents"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        TEXT_CREATION[8],
        TEXT_CREATION[9],
        TEXT_CREATION[10],
        TEXT_CREATION[11],
        TEXT_CREATION[12],
    ],
};
/// Every native byte source and copied label stays independently typed.
pub const fn text_import_facets() -> [FacetSpec; 18] {
    let text = text_create_facets();
    [
        text[0],
        text[1],
        text[2],
        text[3],
        text[4],
        text[5],
        text[6],
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Chat)],
            TargetResolver::ResolvedCollection("message_source_chats"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Thread)],
            TargetResolver::ResolvedCollection("message_source_threads"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Document)],
            TargetResolver::ResolvedCollection("message_source_documents"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Environment)],
            TargetResolver::ResolvedCollection("message_source_environments"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Upload)],
            TargetResolver::ResolvedCollection("message_retained_uploads"),
        ),
        FacetSpec::response_content(
            &[KeyTargetKind::Object(ObjectKind::Document)],
            TargetResolver::ResolvedCollection("created_documents"),
        ),
        text[8],
        text[9],
        text[10],
        text[11],
        text[12],
    ]
}
