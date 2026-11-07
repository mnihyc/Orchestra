//! Actual sources of an adopting creator, preview and its native preparation phase.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Factual roles, independent of the currently checking transport or credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The real destination Topic, with Thread creation as its business family.
    Destination,
    /// The actual Provider whose native session is read.
    Session,
    /// The actual Provider, Agent and selected Environment used by the native phase.
    Binding,
    /// Existing targets whose content the preview actually emits.
    PreviewContent,
    /// Optional existing address components emitted by the native environment reference.
    NameOrgs,
}

const PREPARATION: &[Binding<Source>] = &[
    Binding {
        source: Source::Destination,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Session,
        target: TargetResolver::Resolved("native_session"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Binding,
        target: TargetResolver::ResolvedCollection("chosen_binding"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];

/// The actual known-source preparation of an adopting creator. This cannot admit its later gifts.
pub const ADOPT_PREPARATION: Declaration<Source> = Declaration {
    operation: "ThreadAdopt",
    bindings: PREPARATION,
};
/// The same native preparation, under the preview's independent operation.
pub const PREVIEW_PREPARATION: Declaration<Source> = Declaration {
    operation: "ThreadAdoptPreview",
    bindings: PREPARATION,
};

/// A preview has no newly created Thread. Its selected native content supplies its own Read.
pub const PREVIEW: Declaration<Source> = Declaration {
    operation: "ThreadAdoptPreview",
    bindings: &[
        PREPARATION[0],
        PREPARATION[1],
        PREPARATION[2],
        Binding {
            source: Source::PreviewContent,
            target: TargetResolver::ResolvedCollection("preview_content"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameOrgs,
            target: TargetResolver::ResolvedCollection("preview_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
