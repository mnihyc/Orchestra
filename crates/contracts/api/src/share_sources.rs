//! Native stored sharing contributions, source selection and optional named projections.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};
/// Actual native share source roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The requested actual source or recipient identity.
    Root,
    /// Actual stored subjects of selected native records.
    Results,
    /// Existing linked input payloads, governed by their actual stored Thread.
    Inputs,
    /// The stored subject changed or removed, never its share UUID.
    Subject,
    /// Optional same-operation content of the completed subject change.
    Content,
    /// A real recipient control effect, only for an effective first-reading change.
    Control,
    /// Native protected organization names.
    Orgs,
    /// Native protected project names.
    Projects,
    /// Native protected topic names.
    Topics,
    /// Native protected thread names and attribution.
    Threads,
}
const ORGS: Binding<Source> = Binding {
    source: Source::Orgs,
    target: Target::ResolvedCollection("share_name_orgs"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
const PROJECTS: Binding<Source> = Binding {
    source: Source::Projects,
    target: Target::ResolvedCollection("share_name_projects"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
const TOPICS: Binding<Source> = Binding {
    source: Source::Topics,
    target: Target::ResolvedCollection("share_name_topics"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
const THREADS: Binding<Source> = Binding {
    source: Source::Threads,
    target: Target::ResolvedCollection("share_name_threads"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
/// Source and recipient-side selectors retain the native current giver/holder projection.
pub const LIST: Declaration<Source> = Declaration {
    operation: "AgentShareList",
    bindings: &[
        Binding {
            source: Source::Root,
            target: Target::Resolved("source_or_recipient"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Results,
            target: Target::ResolvedCollection("shared_sources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Inputs,
            target: Target::ResolvedCollection("share_inputs"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        ORGS,
        PROJECTS,
        TOPICS,
        THREADS,
    ],
};
const SUBJECT: Binding<Source> = Binding {
    source: Source::Subject,
    target: Target::Resolved("shared_source"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const CONTENT: Binding<Source> = Binding {
    source: Source::Content,
    target: Target::Resolved("shared_source"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};
/// Native pure grant updates do not control or wake the recipient.
pub const UPDATE_GRANT: Declaration<Source> = Declaration {
    operation: "ShareAgentUpdate.grant",
    bindings: &[SUBJECT, CONTENT, ORGS, PROJECTS, TOPICS, THREADS],
};
/// The native stored effective first-reading change additionally controls its actual recipient.
pub const UPDATE_READ_FIRST: Declaration<Source> = Declaration {
    operation: "ShareAgentUpdate.read_first",
    bindings: &[
        SUBJECT,
        CONTENT,
        Binding {
            source: Source::Control,
            target: Target::Resolved("recipients"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        ORGS,
        PROJECTS,
        TOPICS,
        THREADS,
    ],
};
/// Native removal is Manage at its actual stored subject, never recipient Interact.
pub const DELETE: Declaration<Source> = Declaration {
    operation: "ShareAgentDelete",
    bindings: &[SUBJECT],
};
