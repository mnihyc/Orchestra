//! Stored thread facts supplied by native control and response producers.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// A control's actual stored subject, before applying or projecting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The exact thread whose native control accepts the request.
    Thread,
    /// Native profile, provider, environment and selected cap sources of an optional pane.
    OpeningControls,
    /// Current provenance labels selected by the opening's bounded native name producer.
    OpeningNames,
}

/// Setting a goal has independent effect and same-operation response roles.
pub const GOAL_CREATE: Declaration<Source> = Declaration {
    operation: "ThreadGoalCreate",
    bindings: &[
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// A screen opening reads its own conversation and only the selected auxiliary sources.
pub const OPENING: Declaration<Source> = Declaration {
    operation: "ThreadOpeningGet",
    bindings: &[
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OpeningControls,
            target: TargetResolver::ResolvedCollection("opening_controls"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OpeningNames,
            target: TargetResolver::ResolvedCollection("opening_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
