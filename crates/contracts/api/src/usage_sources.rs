//! Native usage subjects associated with the canonical operation facets.
//!
//! Usage aggregates and ledger pages belong to the native requested usage scope:
//! an organization, project, topic/thread, provider, or the caller's own Account.
//! Its Read admits that subject's native aggregate rows; individual ledger threads
//! are not additional key targets. The native reader's current depth, row visibility,
//! redacted references and withheld turn links still decide disclosure.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};

/// Factual native usage sources, independent of action/permission semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The native aggregate/ledger subject, before filtering, aggregation or paging.
    UsageScope,
    /// The exact stored provider selected by the typed health query.
    Provider,
    /// The native Home of the stored cap, never the cap identifier as a node.
    CapHome,
    /// The native selected cap homes, before standings or names are computed.
    Caps,
    /// Complete Group subject payloads emitted by selected caps.
    Groups,
    /// Optional protected names, chosen before their ordinary projection.
    Names,
    /// The exact node selected by the limits query.
    Node,
    /// Actual usable Provider rows in a node's rate table.
    RateProviders,
    /// The system form's actual native settings scope.
    System,
    /// Native View prerequisites for naming a provider cap's node or a new subject's keeper.
    Prerequisites,
}

/// Health samples belong to their actual provider.
pub const HEALTH: Declaration<Source> = Declaration {
    operation: "UsageHealthGet",
    bindings: &[Binding {
        source: Source::Provider,
        target: Target::Resolved("provider"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// A cap removal binds the defining home before its native writer.
pub const CAP_DELETE: Declaration<Source> = Declaration {
    operation: "UsageCapDelete",
    bindings: &[Binding {
        source: Source::CapHome,
        target: Target::Resolved("cap_subjects"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

const CAP_SCOPE: Binding<Source> = Binding {
    source: Source::CapHome,
    target: Target::Resolved("cap_subjects"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const CAPS: Binding<Source> = Binding {
    source: Source::Caps,
    target: Target::ResolvedCollection("caps"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const GROUPS: Binding<Source> = Binding {
    source: Source::Groups,
    target: Target::ResolvedCollection("cap_groups"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const NAMES: Binding<Source> = Binding {
    source: Source::Names,
    target: Target::ResolvedCollection("usage_names"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};

/// Native defined/covering cap selection and its emitted names.
pub const CAP_LIST: Declaration<Source> = Declaration {
    operation: "UsageCapList",
    bindings: &[CAP_SCOPE, CAPS, GROUPS, NAMES],
};
/// The exact named node and the native applicable cap selection.
pub const LIMITS: Declaration<Source> = Declaration {
    operation: "UsageLimitsGet",
    bindings: &[
        Binding {
            source: Source::Node,
            target: Target::Resolved("node"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CAPS,
        GROUPS,
        NAMES,
    ],
};
/// A stored cap's native queue with independently bounded protected thread labels.
pub const WAITING: Declaration<Source> = Declaration {
    operation: "UsageCapWaitingList",
    bindings: &[CAP_SCOPE, NAMES],
};
/// The native node rate-table intent, including actual usable provider sources.
pub const RATES_NODE: Declaration<Source> = Declaration {
    operation: "UsageRateTableGet.node",
    bindings: &[
        Binding {
            source: Source::Node,
            target: Target::Resolved("node"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::RateProviders,
            target: Target::ResolvedCollection("rate_providers"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
/// The mutually exclusive system-settings rate-table intent.
pub const RATES_SYSTEM: Declaration<Source> = Declaration {
    operation: "UsageRateTableGet.system",
    bindings: &[Binding {
        source: Source::System,
        target: Target::System,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

const CAP_CONTENT: Binding<Source> = Binding {
    usage: FacetUsage::ResponseContent,
    ..CAP_SCOPE
};
const PREREQUISITES: Binding<Source> = Binding {
    source: Source::Prerequisites,
    target: Target::ResolvedCollection("named_nodes"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::Collection,
};
const CAP_CONTENT_GROUPS: Binding<Source> = Binding {
    usage: FacetUsage::ResponseContent,
    ..GROUPS
};
const CAP_CONTENT_NAMES: Binding<Source> = Binding {
    usage: FacetUsage::ResponseContent,
    ..NAMES
};
/// Creation binds actual Home and only the native View prerequisites that are present.
pub const CAP_CREATE: Declaration<Source> = Declaration {
    operation: "UsageCapCreate",
    bindings: &[
        CAP_SCOPE,
        PREREQUISITES,
        CAP_CONTENT,
        CAP_CONTENT_GROUPS,
        CAP_CONTENT_NAMES,
    ],
};
/// Update uses the immutable stored Home and independently prepared optional content.
pub const CAP_UPDATE: Declaration<Source> = Declaration {
    operation: "UsageCapUpdate",
    bindings: &[
        CAP_SCOPE,
        CAP_CONTENT,
        CAP_CONTENT_GROUPS,
        CAP_CONTENT_NAMES,
    ],
};

const USAGE_SCOPE: Binding<Source> = Binding {
    source: Source::UsageScope,
    target: Target::Resolved("scope"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
/// The native usage subject governs all its aggregate rows at the reader's current depth.
/// Its original SQL filter and reader-specific projection decide which rows contribute.
pub const ANALYTICS: Declaration<Source> = Declaration {
    operation: "UsageGet",
    bindings: &[USAGE_SCOPE],
};
/// Ledger rows use that same native subject and current reader-specific privacy cut.
/// A hidden thread is not a separate key prerequisite and never gains a name or turn link.
pub const EVENTS: Declaration<Source> = Declaration {
    operation: "UsageEventList",
    bindings: &[USAGE_SCOPE],
};
