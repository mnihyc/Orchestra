//! Native document source rows for content windows and metadata projections.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::FacetSpec;
use crate::operations::{FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
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
