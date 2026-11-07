//! Native source declarations for the canonical operation facets.
//!
//! Actions and business families remain in `operations`. These rows connect a
//! producer's factual sources to those facets without adding permission rules.

use crate::operations::{FacetUsage, TargetResolver};

/// The actual cardinality reported by a native source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// Exactly one authoritative subject.
    One,
    /// Every selected subject, including a producer-proven empty selection.
    Collection,
}

/// One source-to-facet connection in an operation declaration.
#[derive(Debug, Clone, Copy)]
pub struct Binding<S> {
    /// The domain's native source, never a response field.
    pub source: S,
    /// The canonical target role to fill.
    pub target: TargetResolver,
    /// Required effect or optional response content in the canonical contract.
    pub usage: FacetUsage,
    /// The native source's declared cardinality.
    pub cardinality: Cardinality,
}

/// One exact operation/variant and its complete native source connections.
#[derive(Debug, Clone, Copy)]
pub struct Declaration<S: 'static> {
    /// Exact canonical selection, including a variant suffix where applicable.
    pub operation: &'static str,
    /// Each canonical facet is bound once by its authoritative native source.
    /// A target role with several canonical actions uses the same native facts for all of them.
    pub bindings: &'static [Binding<S>],
}
