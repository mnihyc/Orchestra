//! Native upload custody and selected saved-byte sources, independent of the content hash.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// The native owner namespace of a private upload release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
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
