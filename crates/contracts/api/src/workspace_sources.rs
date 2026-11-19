//! Native Snapshot subjects connected to the canonical workspace operation facets.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Factual identities supplied by the existing Snapshot custody reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Files authority follows the stored Snapshot's native current governor.
    SnapshotGovernor,
    /// The stored origin Environment supplies the metadata reference in a full reply.
    SnapshotOrigin,
}

/// Relabeling uses the current native Files governor; full content also reads its origin.
pub const SNAPSHOT_UPDATE: Declaration<Source> = Declaration {
    operation: "SnapshotUpdate",
    bindings: &[
        Binding {
            source: Source::SnapshotGovernor,
            target: TargetResolver::Resolved("snapshot_environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::SnapshotGovernor,
            target: TargetResolver::Resolved("snapshot_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::SnapshotOrigin,
            target: TargetResolver::Resolved("snapshot_origin"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Source roles of a native thread Snapshot window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotListSource {
    /// Current Files governors of the selected rows, including historical recorded stores.
    Governors,
    /// Stored origin environments supplying the selected metadata references.
    Origins,
}

/// Selected source collections are complete even when the actual native window is empty.
pub const THREAD_SNAPSHOT_LIST: Declaration<SnapshotListSource> = Declaration {
    operation: "ThreadSnapshotList",
    bindings: &[
        Binding {
            source: SnapshotListSource::Governors,
            target: TargetResolver::ResolvedCollection("snapshot_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: SnapshotListSource::Origins,
            target: TargetResolver::ResolvedCollection("snapshot_origins"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
