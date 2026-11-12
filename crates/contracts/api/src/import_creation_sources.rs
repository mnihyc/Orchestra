//! Native byte witnesses, actual write governors and response content for a queued import.
use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetSpec, FacetUsage, TargetResolver as Target, VariantSpec},
    upload_sources,
};
use orch_access_contract::{
    key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};
/// Sources selected by the native opened plan, before any queue or import write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// One real saved-byte alternative for every input hash.
    Bytes(upload_sources::Source),
    /// Actual existing destination governors for the native writes.
    Destinations,
    /// The native pages whose access or authorship is actually restored.
    Delegation,
    /// Private creator metadata of the returned native job.
    Creator,
}
const WRITABLE: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Org),
    KeyTargetKind::Object(ObjectKind::Project),
    KeyTargetKind::Object(ObjectKind::Topic),
    KeyTargetKind::Object(ObjectKind::Thread),
    KeyTargetKind::Object(ObjectKind::Document),
    KeyTargetKind::Object(ObjectKind::DocumentContainer),
    KeyTargetKind::Identity(KeyIdentityKind::Chat),
];
const fn byte(index: usize) -> Binding<Source> {
    let original = upload_sources::BLOB_GET.bindings[index];
    Binding {
        source: Source::Bytes(original.source),
        target: original.target,
        usage: original.usage,
        cardinality: original.cardinality,
    }
}
const BYTES: [Binding<Source>; 5] = [byte(0), byte(1), byte(2), byte(3), byte(4)];
const DESTINATION: Binding<Source> = Binding {
    source: Source::Destinations,
    target: Target::ResolvedCollection("import_destinations"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const DELEGATION: Binding<Source> = Binding {
    source: Source::Delegation,
    target: Target::ResolvedCollection("import_delegation_targets"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const CREATOR: Binding<Source> = Binding {
    source: Source::Creator,
    target: Target::Resolved("import_creator"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};
/// Both native variants share only the truthful, selected byte Read prefix.
pub const CANDIDATES: [Declaration<Source>; 2] = [
    Declaration {
        operation: "NodeImportCreate.content",
        bindings: &[
            BYTES[0],
            BYTES[1],
            BYTES[2],
            BYTES[3],
            BYTES[4],
            DESTINATION,
            CREATOR,
        ],
    },
    Declaration {
        operation: "NodeImportCreate.restore_access",
        bindings: &[
            BYTES[0],
            BYTES[1],
            BYTES[2],
            BYTES[3],
            BYTES[4],
            DESTINATION,
            DELEGATION,
            CREATOR,
        ],
    },
];
const fn content() -> [FacetSpec; 7] {
    let bytes = upload_sources::blob_read_facets();
    [
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        FacetSpec::required(KeyAction::Write, WRITABLE, DESTINATION.target),
        FacetSpec::response_content(
            &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
            CREATOR.target,
        ),
    ]
}
const CONTENT: [FacetSpec; 7] = content();
const RESTORE: [FacetSpec; 8] = [
    CONTENT[0],
    CONTENT[1],
    CONTENT[2],
    CONTENT[3],
    CONTENT[4],
    CONTENT[5],
    FacetSpec::required(KeyAction::Manage, WRITABLE, DELEGATION.target),
    CONTENT[6],
];
/// Actual archive contents and native defaults select the slot after the common prefix, never request flags.
pub const VARIANTS: [VariantSpec; 2] = [
    VariantSpec {
        name: "content",
        selector: "resolved plan access is false and resolved authors is labels",
        facets: &CONTENT,
    },
    VariantSpec {
        name: "restore_access",
        selector: "resolved plan access is true or resolved authors is accounts, including administrator defaults",
        facets: &RESTORE,
    },
];
