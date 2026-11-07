//! Native administrative subjects; current system standing remains a separate ordinary gate.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};

/// Sources selected by the existing administrative producers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual selected subjects, never runtime or share surrogate identifiers.
    Results,
    /// Physical publishers and mandatory native runtime references.
    References,
    /// Protected optional reference names and node content.
    Names,
    /// Stored root organization changed by a native writer.
    Org,
    /// Same-operation optional response of a completed organization change.
    Content,
    /// The real destination of a requested exact build fetch.
    Destination,
    /// Existing global catalogue disclosed after a completed fetch.
    Catalogue,
}
const RESULTS: Binding<Source> = Binding {
    source: Source::Results,
    target: Target::ResolvedCollection("results"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const REFERENCES: Binding<Source> = Binding {
    source: Source::References,
    target: Target::ResolvedCollection("references"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const NAMES: Binding<Source> = Binding {
    source: Source::Names,
    target: Target::ResolvedCollection("names"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
/// Native program usage is aggregated only after selecting its actual Environment/Provider units.
pub const HARNESS_USAGE: Declaration<Source> = Declaration {
    operation: "AdminHarnessUsageList",
    bindings: &[RESULTS, NAMES],
};
/// The administrative fleet shape includes its actual physical publishers and retained names.
pub const HOSTS: Declaration<Source> = Declaration {
    operation: "AdminHostList",
    bindings: &[RESULTS, NAMES],
};
/// Live and retained organization pages retain their native ordering and inspection rules.
pub const ORGS: Declaration<Source> = Declaration {
    operation: "AdminOrgList",
    bindings: &[RESULTS, NAMES],
};
/// Runtime inspection is Manage on the actual unit and its native selected references.
pub const RUNTIMES: Declaration<Source> = Declaration {
    operation: "AdminRuntimeList",
    bindings: &[RESULTS, REFERENCES, NAMES],
};
const ORG: Binding<Source> = Binding {
    source: Source::Org,
    target: Target::Path("id"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const CONTENT: Binding<Source> = Binding {
    source: Source::Content,
    target: Target::Path("id"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};
/// Changes the stored root organization's limits; content remains optional.
pub const ORG_UPDATE: Declaration<Source> = Declaration {
    operation: "AdminOrgUpdate",
    bindings: &[ORG, CONTENT, NAMES],
};
/// Removes the real root organization's index and retains the native rebuild lifecycle.
pub const SEMANTIC_REBUILD: Declaration<Source> = Declaration {
    operation: "AdminSemanticSearchRebuild",
    bindings: &[ORG, CONTENT],
};
/// The retained generation belongs to this exact stored root organization.
pub const SEMANTIC_KEEP: Declaration<Source> = Declaration {
    operation: "AdminSemanticSearchKeep",
    bindings: &[ORG, CONTENT],
};
const CATALOGUE: Binding<Source> = Binding {
    source: Source::Catalogue,
    target: Target::System,
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};
/// The environment variant is selected from the native mutually exclusive destination.
pub const FETCH_ENVIRONMENT: Declaration<Source> = Declaration {
    operation: "HarnessVersionFetch.environment",
    bindings: &[
        Binding {
            source: Source::Destination,
            target: Target::Resolved("environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CATALOGUE,
    ],
};
/// Node settings variant; a path string is never an organization identity.
pub const FETCH_NODE: Declaration<Source> = Declaration {
    operation: "HarnessVersionFetch.node",
    bindings: &[
        Binding {
            source: Source::Destination,
            target: Target::Resolved("node"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CATALOGUE,
    ],
};
/// Native system variant still requires the maker's current system administration.
pub const FETCH_SYSTEM: Declaration<Source> = Declaration {
    operation: "HarnessVersionFetch.system",
    bindings: &[
        Binding {
            source: Source::Destination,
            target: Target::System,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CATALOGUE,
    ],
};
