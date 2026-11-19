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
