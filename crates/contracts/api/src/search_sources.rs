//! Native search selections associated with the canonical operation roles.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};
/// Factual search sources, supplied before hydration, ranking windows or serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual typed result governors.
    Results,
    /// Optional protected context and address components.
    Names,
    /// The source document of a related search.
    Document,
    /// Native document-folder ancestor labels.
    Folders,
}
const RESULTS: Binding<Source> = Binding {
    source: Source::Results,
    target: Target::ResolvedCollection("results"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const NAMES: Binding<Source> = Binding {
    source: Source::Names,
    target: Target::ResolvedCollection("search_names"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
/// Ranked heterogeneous names keep their actual stored identity and context facts.
pub const NAME_SEARCH: Declaration<Source> = Declaration {
    operation: "NameSearchList",
    bindings: &[RESULTS, NAMES],
};

/// Related lookup reads the source document, actual heterogeneous results and optional contexts.
pub const RELATED: Declaration<Source> = Declaration {
    operation: "DocumentRelatedList",
    bindings: &[
        Binding {
            source: Source::Document,
            target: Target::Resolved("document"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        RESULTS,
        NAMES,
        Binding {
            source: Source::Folders,
            target: Target::ResolvedCollection("search_folders"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Literal search binds the actual matched owner families before its native output window.
pub const CONTENT: Declaration<Source> = Declaration {
    operation: "SearchList",
    bindings: &[RESULTS, NAMES],
};
/// Native stream interest, actual lexical/meaning batches and queued query provenance
/// consume this same row; initial interest never admits an unproduced later result.
pub const STREAM: Declaration<Source> = Declaration {
    operation: "SearchStreamList",
    bindings: &[RESULTS, NAMES],
};
