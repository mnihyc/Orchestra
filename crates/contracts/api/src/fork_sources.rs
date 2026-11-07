//! Actual job and name sources of native fork plans and batches.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    PendingBatchJobs,
    BatchJobs,
    WaitingMachines,
    NamesOrgs,
    NamesProjects,
    NamesTopics,
    NamesThreads,
}

pub const BATCH_GET: Declaration<Source> = Declaration {
    operation: "ForkBatchGet",
    bindings: &[
        Binding {
            source: Source::BatchJobs,
            target: TargetResolver::ResolvedCollection("batch_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::WaitingMachines,
            target: TargetResolver::ResolvedCollection("batch_waiting_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesOrgs,
            target: TargetResolver::ResolvedCollection("batch_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesProjects,
            target: TargetResolver::ResolvedCollection("batch_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesTopics,
            target: TargetResolver::ResolvedCollection("batch_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesThreads,
            target: TargetResolver::ResolvedCollection("batch_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const BATCH_CANCEL: Declaration<Source> = Declaration {
    operation: "ForkBatchCancel",
    bindings: &[
        Binding {
            source: Source::PendingBatchJobs,
            target: TargetResolver::ResolvedCollection("remaining_batch_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::BatchJobs,
            target: TargetResolver::ResolvedCollection("batch_subjects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::WaitingMachines,
            target: TargetResolver::ResolvedCollection("batch_waiting_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesOrgs,
            target: TargetResolver::ResolvedCollection("batch_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesProjects,
            target: TargetResolver::ResolvedCollection("batch_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesTopics,
            target: TargetResolver::ResolvedCollection("batch_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesThreads,
            target: TargetResolver::ResolvedCollection("batch_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
