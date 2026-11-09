//! Real main/side records and the native destination of one reviewed conversation move.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Main,
    Sides,
    Destination,
    ReviewContent,
    ThreadContent,
}

const REQUIRED: &[Binding<Source>] = &[
    Binding {
        source: Source::Main,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Sides,
        target: TargetResolver::ResolvedCollection("move_side_lanes"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Destination,
        target: TargetResolver::ResolvedCollection("move_destinations"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Main,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Sides,
        target: TargetResolver::ResolvedCollection("move_side_lanes"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];

pub const PREVIEW: Declaration<Source> = Declaration {
    operation: "ThreadMovePreview",
    bindings: &[
        REQUIRED[0],
        REQUIRED[1],
        REQUIRED[2],
        REQUIRED[3],
        REQUIRED[4],
        Binding {
            source: Source::ReviewContent,
            target: TargetResolver::ResolvedCollection("move_review_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
pub const MOVE: Declaration<Source> = Declaration {
    operation: "ThreadMove",
    bindings: &[
        REQUIRED[0],
        REQUIRED[1],
        REQUIRED[2],
        REQUIRED[3],
        REQUIRED[4],
        Binding {
            source: Source::ThreadContent,
            target: TargetResolver::ResolvedCollection("move_thread_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
