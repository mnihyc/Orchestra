//! Actual native ownership observation subjects, including retained identities.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Sources selected by ownership readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual native recoverable root org rows selected for their owner.
    DeletedOrgs,
}

/// Deletion-list inspection uses current retained identities, never a live Get surrogate.
pub const DELETED_ORGS: Declaration<Source> = Declaration {
    operation: "OrgDeletedList",
    bindings: &[Binding {
        source: Source::DeletedOrgs,
        target: TargetResolver::ResolvedCollection("results"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};
