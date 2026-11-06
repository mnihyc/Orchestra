//! Actual native account selection facts for account observation operations.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Native account observation sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Every actual account selected by the ordinary native search.
    SearchAccounts,
}

/// Native account search is a collection, including a genuine empty result.
pub const USER_SEARCH: Declaration<Source> = Declaration {
    operation: "UserSearch",
    bindings: &[Binding {
        source: Source::SearchAccounts,
        target: TargetResolver::ResolvedCollection("results"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};
