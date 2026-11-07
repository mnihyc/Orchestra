//! Native topic selections and their actual resource/reference sources.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};
/// Native topic source roles, never transport IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual selected environments or heterogeneous places.
    Results,
    /// Protected native Template/Host/Machine/Mount portions of the returned shape.
    Resources,
    /// Optional native protected reference names.
    Names,
    /// The actual provided or inherited template used by native placement.
    Template,
    /// Actual topic/thread audience at a native delivery checkpoint.
    Destinations,
    /// Optional existing content after accepted delivery.
    Content,
}
const RESULTS: Binding<Source> = Binding {
    source: Source::Results,
    target: Target::ResolvedCollection("results"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const RESOURCES: Binding<Source> = Binding {
    source: Source::Resources,
    target: Target::ResolvedCollection("resources"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const NAMES: Binding<Source> = Binding {
    source: Source::Names,
    target: Target::ResolvedCollection("names"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
/// Native whole/through-thread environment list, including the stored default.
pub const ENVIRONMENTS: Declaration<Source> = Declaration {
    operation: "TopicEnvironmentList",
    bindings: &[RESULTS, RESOURCES, NAMES],
};
/// Hosts and existing environments keep their native shape and placement algorithm.
pub const PLACES: Declaration<Source> = Declaration {
    operation: "TopicPlaceList",
    bindings: &[
        RESULTS,
        RESOURCES,
        NAMES,
        Binding {
            source: Source::Template,
            target: Target::Resolved("template"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
/// Delivery owns its real selected audience; no topic root stands for every thread effect.
pub const ANNOUNCEMENT: Declaration<Source> = Declaration {
    operation: "NodeAnnouncementCreate",
    bindings: &[
        Binding {
            source: Source::Destinations,
            target: Target::ResolvedCollection("destinations"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Content,
            target: Target::ResolvedCollection("destinations"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
