//! The real home governing a new host's one-use enrollment credential.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Home,
}
pub const CREATE: Declaration<Source> = Declaration {
    operation: "HostEnrollmentTokenCreate",
    bindings: &[Binding {
        source: Source::Home,
        target: TargetResolver::Resolved("home"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
