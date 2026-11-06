//! Actual existing thread and environment operands of native placement.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Thread,
    PlacementEnvironments,
    ThreadContent,
}

pub const PLACEMENT: Declaration<Source> = Declaration {
    operation: "ThreadEnvironmentSet",
    bindings: &[
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::PlacementEnvironments,
            target: TargetResolver::ResolvedCollection("placement_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ThreadContent,
            target: TargetResolver::ResolvedCollection("placed_thread_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
