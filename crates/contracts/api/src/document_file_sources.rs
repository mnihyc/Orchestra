//! Actual document-file writes, selected native saved-byte witnesses and protected refusals.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// The native producer owns these roles; no content hash is an invented Upload identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual current live document and its optional completed-file Read.
    Document,
    /// Selected readable chat byte witnesses.
    Chats,
    /// Selected readable thread byte witnesses.
    Threads,
    /// Selected readable document byte witnesses.
    Documents,
    /// Selected readable environment-files byte witnesses.
    Environments,
    /// The acting account's native retained-upload custody.
    Uploads,
    /// Actual node supplying a protected rich limit refusal, absent on success.
    LimitNodes,
}

const DOCUMENT: TargetResolver = TargetResolver::PermissionObject;
/// The native setting node; numeric applicable limits remain safe operational information.
pub const LIMIT_NODES: &str = "file_limit_nodes";
/// Direct supplied bytes need no stored-source Read.
pub const DIRECT_FACETS: [FacetSpec; 3] = [
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Document)],
        DOCUMENT,
    ),
    FacetSpec::response_content(&[KeyTargetKind::Object(ObjectKind::Document)], DOCUMENT),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::ResolvedCollection(LIMIT_NODES),
    ),
];
/// Existing bytes keep exactly the native source families that can actually supply them.
pub const REFERENCE_FACETS: [FacetSpec; 8] = [
    DIRECT_FACETS[0],
    DIRECT_FACETS[1],
    DIRECT_FACETS[2],
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
];

const BASE: [Binding<Source>; 3] = [
    Binding {
        source: Source::Document,
        target: DOCUMENT,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Document,
        target: DOCUMENT,
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::LimitNodes,
        target: TargetResolver::ResolvedCollection(LIMIT_NODES),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
/// A raw supplied body is chosen only by its native filename-header branch.
pub const DIRECT: Declaration<Source> = Declaration {
    operation: "DocumentFileCreate.direct",
    bindings: &BASE,
};
/// A parsed native saved reference retains its real selected witness families.
pub const REFERENCE: Declaration<Source> = Declaration {
    operation: "DocumentFileCreate.from_upload",
    bindings: &[
        BASE[0],
        BASE[1],
        BASE[2],
        Binding {
            source: Source::Chats,
            target: TargetResolver::ResolvedCollection("message_source_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection("message_source_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Documents,
            target: TargetResolver::ResolvedCollection("message_source_documents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Environments,
            target: TargetResolver::ResolvedCollection("message_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Uploads,
            target: TargetResolver::ResolvedCollection("message_retained_uploads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// A completed file retry reads only its current native metadata, not spent input-byte sources.
pub const METADATA_DIRECT: Declaration<Source> = Declaration {
    operation: DIRECT.operation,
    bindings: &[BASE[1], BASE[2]],
};
/// The same existing response roles on the canonical saved-source variant.
pub const METADATA_REFERENCE: Declaration<Source> = Declaration {
    operation: REFERENCE.operation,
    bindings: METADATA_DIRECT.bindings,
};

/// A same-name refusal discloses only the actual existing document file identity.
pub const CONFLICT_DIRECT: Declaration<Source> = Declaration {
    operation: DIRECT.operation,
    bindings: &[BASE[1]],
};
/// The same refusal in the saved-source variant, with no invented success operands.
pub const CONFLICT_REFERENCE: Declaration<Source> = Declaration {
    operation: REFERENCE.operation,
    bindings: CONFLICT_DIRECT.bindings,
};
/// A named limit reads its actual setting node, independently of document metadata.
pub const LIMIT_DIRECT: Declaration<Source> = Declaration {
    operation: DIRECT.operation,
    bindings: &[BASE[2]],
};
/// The actual setting node of a saved-source request's limit refusal.
pub const LIMIT_REFERENCE: Declaration<Source> = Declaration {
    operation: REFERENCE.operation,
    bindings: LIMIT_DIRECT.bindings,
};
