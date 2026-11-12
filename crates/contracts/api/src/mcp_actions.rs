//! Native MCP definition actions and their actual stored governors and copied artifacts.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Facts selected by the existing MCP definition lifecycle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The preset page or the Environment that owns this server.
    ManagedServer,
    /// The actual online physical suppliers whose private copies are forgotten.
    ForgottenCopies,
}

/// Deletion binds the stored governor and every native copy selected for cleanup.
pub const DELETE: Declaration<Source> = Declaration {
    operation: "McpServerDelete",
    bindings: &[
        Binding {
            source: Source::ManagedServer,
            target: TargetResolver::Resolved("server_subject"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ForgottenCopies,
            target: TargetResolver::ResolvedCollection("forgotten_copies"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
