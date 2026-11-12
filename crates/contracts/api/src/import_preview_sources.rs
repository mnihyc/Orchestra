//! Native destination viewing and the actual readable byte witnesses of an import preview.

use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetSpec, FacetUsage, TargetResolver},
    upload_sources,
};
use orch_access_contract::{
    key_scope::{KeyAction, KeyTargetKind},
    permissions::ObjectKind,
};

/// Only actual native preview inputs govern its read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The real requested destination, under the existing node-view gate.
    Destination,
    /// One actual selected native saved-byte family, independent of the content hash.
    Bytes(upload_sources::Source),
    /// Actual live rows consulted while matching the source to the destination.
    Existing,
    /// Actual accounts matched by the native people producer.
    Accounts,
}

const fn byte_binding(index: usize) -> Binding<Source> {
    let original = upload_sources::BLOB_GET.bindings[index];
    Binding {
        source: Source::Bytes(original.source),
        target: original.target,
        usage: original.usage,
        cardinality: original.cardinality,
    }
}

/// One row supplies the existing shared native stored-byte producer.
pub const PREVIEW: Declaration<Source> = Declaration {
    operation: "NodeImportPreview",
    bindings: &[
        Binding {
            source: Source::Destination,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        byte_binding(0),
        byte_binding(1),
        byte_binding(2),
        byte_binding(3),
        byte_binding(4),
    ],
};

/// A source-object UUID inside the uploaded package is never a live destination identity.
pub const PREVIEW_FACETS: [FacetSpec; 6] = {
    let bytes = upload_sources::blob_read_facets();
    [
        FacetSpec::required(
            KeyAction::Read,
            &[
                KeyTargetKind::Object(ObjectKind::Org),
                KeyTargetKind::Object(ObjectKind::Project),
                KeyTargetKind::Object(ObjectKind::Topic),
                KeyTargetKind::Object(ObjectKind::Thread),
            ],
            TargetResolver::Path("id"),
        ),
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
    ]
};

/// Planning reads the uploaded source and its complete native conflict context.
pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "NodeImportPreflight",
    bindings: &[
        PREVIEW.bindings[0],
        byte_binding(0),
        byte_binding(1),
        byte_binding(2),
        byte_binding(3),
        byte_binding(4),
        Binding {
            source: Source::Existing,
            target: TargetResolver::ResolvedCollection("import_existing_targets"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Accounts,
            target: TargetResolver::ResolvedCollection("import_matched_accounts"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Uploaded package identifiers supply no live destination or account authority.
pub const PREFLIGHT_FACETS: [FacetSpec; 8] = [
    PREVIEW_FACETS[0],
    PREVIEW_FACETS[1],
    PREVIEW_FACETS[2],
    PREVIEW_FACETS[3],
    PREVIEW_FACETS[4],
    PREVIEW_FACETS[5],
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
            KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Chat),
        ],
        TargetResolver::ResolvedCollection("import_existing_targets"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(
            orch_access_contract::key_scope::KeyIdentityKind::Account,
        )],
        TargetResolver::ResolvedCollection("import_matched_accounts"),
    ),
];
