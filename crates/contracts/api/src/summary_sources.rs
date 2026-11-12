//! Actual native document settings sources for summary options; permission stays with the document.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};

/// Native source, independent of the document's containing node's settings family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual live document and its ordinary folder/node ancestry.
    Document,
}
const TARGET: TargetResolver = TargetResolver::TypedPath {
    parameter: "id",
    kind: KeyTargetKind::Object(ObjectKind::Document),
};
const BINDINGS: &[Binding<Source>] = &[Binding {
    source: Source::Document,
    target: TARGET,
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
}];
/// Ordinary read of the effective document summary options.
pub const READ: Declaration<Source> = Declaration {
    operation: "DocumentSummaryOptionsGet",
    bindings: BINDINGS,
};
/// Ordinary document edit with a content-free receipt.
pub const WRITE: Declaration<Source> = Declaration {
    operation: "DocumentSummaryOptionsSet",
    bindings: BINDINGS,
};

/// Native sources of a summary paragraph and its actual paying provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceSource {
    /// The actual thread, topic, document or chat.
    Target,
    /// Every additional source contributing words to this paragraph.
    Inputs,
    /// The chosen provider, with Use distinct from optional metadata Read.
    Provider,
}
const RESOURCE_READS: &[Binding<ResourceSource>] = &[
    Binding {
        source: ResourceSource::Target,
        target: TargetResolver::Resolved("summary_target"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: ResourceSource::Inputs,
        target: TargetResolver::ResolvedCollection("summary_inputs"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: ResourceSource::Provider,
        target: TargetResolver::ResolvedCollection("summary_providers"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
/// Paragraph reads never start a provider request.
pub const RESOURCE_READ: Declaration<ResourceSource> = Declaration {
    operation: "SummaryGet",
    bindings: RESOURCE_READS,
};
/// Stored search projections share native source producers without borrowing SummaryGet's namespace.
pub const RESOURCE_BATCH: Declaration<ResourceSource> = Declaration {
    operation: "SummaryBatchGet",
    bindings: &[
        Binding {
            source: ResourceSource::Target,
            target: TargetResolver::ResolvedCollection("summary_targets"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        RESOURCE_READS[1],
        RESOURCE_READS[2],
    ],
};
/// The same declaration is restored at acceptance, dispatch and publication of paid work.
pub const RESOURCE_REFRESH: Declaration<ResourceSource> = Declaration {
    operation: "SummaryRefresh",
    bindings: &[RESOURCE_READS[0], RESOURCE_READS[1]],
};
