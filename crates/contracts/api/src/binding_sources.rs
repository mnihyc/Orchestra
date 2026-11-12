//! Actual existing thread and environment operands of native placement.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Thread,
    PlacementEnvironments,
    ThreadContent,
    OptionsNode,
    OptionsContext,
    OptionsChoices,
    OptionsOrgs,
    OptionsProjects,
    OptionsTopics,
    OptionsThreads,
}

/// Source management and the destination's existing native profile/provider use requirements.
pub enum NativeDestination {
    Binding,
}
pub struct BindingRow {
    pub sources: Declaration<Source>,
    pub phase: Declaration<Source>,
    pub destination: NativeDestination,
}
pub const REBIND: BindingRow = BindingRow {
    destination: NativeDestination::Binding,
    sources: Declaration {
        operation: "ThreadBindingSet",
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
            Binding {
                source: Source::ThreadContent,
                target: TargetResolver::ResolvedCollection("bound_thread_content"),
                usage: FacetUsage::ResponseContent,
                cardinality: Cardinality::Collection,
            },
        ],
    },
    phase: Declaration {
        operation: "ThreadBindingSet",
        bindings: &[Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        }],
    },
};

pub const PLACEMENT: Declaration<Source> = Declaration {
    operation: "ThreadEnvironmentSet",
    bindings: &[
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::PlacementEnvironments,
            target: TargetResolver::ResolvedCollection("placement_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Thread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ThreadContent,
            target: TargetResolver::ResolvedCollection("placed_thread_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const OPTIONS: Declaration<Source> = Declaration {
    operation: "NodeBindingOptionsGet",
    bindings: &[
        Binding {
            source: Source::OptionsNode,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OptionsContext,
            target: TargetResolver::ResolvedCollection("binding_context"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OptionsChoices,
            target: TargetResolver::ResolvedCollection("binding_choices"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OptionsOrgs,
            target: TargetResolver::ResolvedCollection("binding_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OptionsProjects,
            target: TargetResolver::ResolvedCollection("binding_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OptionsTopics,
            target: TargetResolver::ResolvedCollection("binding_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OptionsThreads,
            target: TargetResolver::ResolvedCollection("binding_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
