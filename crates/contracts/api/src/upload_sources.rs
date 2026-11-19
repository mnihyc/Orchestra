//! Native personal upload custody, independent of the content hash or saved copies.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// The native owner namespace of a private upload release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual account whose private custody rows the operation can release.
    CustodyAccount,
}

/// A hash selects bytes in this namespace; it never becomes an Upload UUID.
pub const RELEASE: Declaration<Source> = Declaration {
    operation: "UploadBlobDelete",
    bindings: &[Binding {
        source: Source::CustodyAccount,
        target: TargetResolver::Resolved("upload"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
