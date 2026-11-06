//! Native workspace subjects connected to the canonical operation facets.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Factual identities supplied by the existing Snapshot custody reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Files authority follows the stored Snapshot's native current governor.
    SnapshotGovernor,
}

/// Deleting retained state needs its actual Files governor, with no existing-content reply.
pub const SNAPSHOT_DELETE: Declaration<Source> = Declaration {
    operation: "SnapshotDelete",
    bindings: &[Binding {
        source: Source::SnapshotGovernor,
        target: TargetResolver::Resolved("snapshot_environment"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
