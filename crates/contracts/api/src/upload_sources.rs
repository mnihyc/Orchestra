//! Native upload custody and selected saved-byte sources, independent of the content hash.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// The native owner namespace of a private upload release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The current account's actual upload-custody namespace.
    InventoryNamespace,
    /// Independently readable names of that same account.
    InventoryNames,
    /// The actual completed row held in the system's blob store.
    AdminBlob,
    /// The actual account whose private custody rows the operation can release.
    CustodyAccount,
    /// The actual unfinished upload session and its stored owner.
    Session,
    /// The exact selected saved chat byte witness.
    ReadChats,
    /// The exact selected saved thread byte witness.
    ReadThreads,
    /// The exact selected saved document byte witness.
    ReadDocuments,
    /// The exact selected environment byte witness.
    ReadEnvironments,
    /// The actual acting account's selected retained-upload custody.
    ReadUploads,
}

/// A hash selects bytes in this namespace; it never becomes an Upload UUID.
pub const RELEASE: Declaration<Source> = Declaration {
    operation: "UploadBlobDelete",
    bindings: &[Binding {
        source: Source::CustodyAccount,
        target: TargetResolver::Resolved("upload"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Cancelling a session acts on its stored identity, independently of any bytes it holds.
pub const SESSION_DELETE: Declaration<Source> = Declaration {
    operation: "UploadSessionDelete",
    bindings: &[Binding {
        source: Source::Session,
        target: TargetResolver::Path("upload_id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Appending newly supplied bytes acts on this exact unfinished session's identity.
pub const SESSION_PUT: Declaration<Source> = Declaration {
    operation: "UploadSessionPut",
    bindings: &[Binding {
        source: Source::Session,
        target: TargetResolver::Path("upload_id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Completion records the bytes of this exact unfinished session.
pub const SESSION_COMPLETE: Declaration<Source> = Declaration {
    operation: "UploadSessionComplete",
    bindings: &[Binding {
        source: Source::Session,
        target: TargetResolver::Path("upload_id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// A source-only byte read chooses one real native witness, without inventing an upload identity.
pub const BLOB_GET: Declaration<Source> = Declaration {
    operation: "UploadBlobGet",
    bindings: &[
        Binding {
            source: Source::ReadChats,
            target: TargetResolver::ResolvedCollection("message_source_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReadThreads,
            target: TargetResolver::ResolvedCollection("message_source_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReadDocuments,
            target: TargetResolver::ResolvedCollection("message_source_documents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReadEnvironments,
            target: TargetResolver::ResolvedCollection("message_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReadUploads,
            target: TargetResolver::ResolvedCollection("message_retained_uploads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Existing bytes retain the native reader's complete alternative source families.
#[must_use]
pub const fn blob_read_facets() -> [FacetSpec; 5] {
    use KeyTargetKind::{Identity, Object};
    [
        FacetSpec::required(
            KeyAction::Read,
            &[Identity(KeyIdentityKind::Chat)],
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
            &[Identity(KeyIdentityKind::Upload)],
            TargetResolver::ResolvedCollection("message_retained_uploads"),
        ),
    ]
}

/// Personal custody has one owner namespace; names require their own current Read.
pub const OWN_LIST_FACETS: [FacetSpec; 2] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Upload)],
        TargetResolver::Resolved("upload_namespace"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
        TargetResolver::ResolvedCollection("upload_owner_names"),
    ),
];
/// The personal list's existing native namespace and independently disclosed uploader name.
pub const OWN_LIST: Declaration<Source> = Declaration {
    operation: "MeUploadList",
    bindings: &[
        Binding {
            source: Source::InventoryNamespace,
            target: TargetResolver::Resolved("upload_namespace"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::InventoryNames,
            target: TargetResolver::ResolvedCollection("upload_owner_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
/// Administrative content opens the actual stored row under the system governor.
pub const ADMIN_OPEN: Declaration<Source> = Declaration {
    operation: "AdminUploadContentGet",
    bindings: &[Binding {
        source: Source::AdminBlob,
        target: TargetResolver::Resolved("upload_storage"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
