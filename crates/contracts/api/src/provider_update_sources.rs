//! Actual accepted runtime update jobs and their current native batch response sources.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Native source roles from the batch writer and its current response projector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Providers selected by the original native writer.
    Providers,
    /// Actual accepted member provider references.
    Content,
    /// Host labels in the native response.
    Hosts,
    /// Template labels in the native response.
    Templates,
    /// Actual waiting-machine labels in the current jobs.
    Machines,
    /// Protected org address components.
    Orgs,
    /// Protected project address components.
    Projects,
    /// Protected topic address components.
    Topics,
    /// Protected thread address components.
    Threads,
}

/// One declaration for the existing provider runtime batch confirmation.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "ProviderRuntimeUpdateBatchCreate",
    bindings: &[
        Binding {
            source: Source::Providers,
            target: TargetResolver::ResolvedCollection("affected_providers"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Content,
            target: TargetResolver::ResolvedCollection("batch_providers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Hosts,
            target: TargetResolver::ResolvedCollection("batch_hosts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Templates,
            target: TargetResolver::ResolvedCollection("batch_templates"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Machines,
            target: TargetResolver::ResolvedCollection("batch_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection("batch_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection("batch_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection("batch_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection("batch_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
