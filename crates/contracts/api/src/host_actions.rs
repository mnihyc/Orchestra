//! The actual Host controller for its physical program update.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Native factual source of the Host control.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The selected Host and its intrinsic physical program.
    Host,
}

/// Immediate request, later update grant and same-operation status disclosure.
pub const UPDATE_NOW: Declaration<Source> = Declaration {
    operation: "HostUpdateNow",
    bindings: &[
        Binding {
            source: Source::Host,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Host,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
