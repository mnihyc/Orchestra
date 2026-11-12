//! Native facts associated with port operations; canonical actions remain in ports.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual environment whose native capability is being changed.
    CapabilityEnvironment,
    /// The actual environment of one owned, bidirectional port admission.
    AdmissionEnvironment,
    ReviewEnvironment,
    NamedPlace,
    ReviewHosts,
    ReviewPublishers,
    ReviewOrgs,
    ReviewProjects,
    ReviewTopics,
    ReviewThreads,
    OpeningEnvironment,
    /// The actual Thread making the existing native capability request.
    RequestThread,
    OpeningHosts,
    OpeningPublishers,
    OutsideNode,
    OutsideEnvironments,
    OutsideHosts,
    OutsidePublishers,
    NamesOrgs,
    NamesProjects,
    NamesTopics,
    NamesThreads,
}

pub const CAPABILITY_GRANT: Declaration<Source> = Declaration {
    operation: "EnvironmentPortCapabilityGrant",
    bindings: &[
        Binding {
            source: Source::CapabilityEnvironment,
            target: TargetResolver::Resolved("capability_environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::CapabilityEnvironment,
            target: TargetResolver::Resolved("capability_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

pub const PORT_USE: Declaration<Source> = Declaration {
    operation: "EnvironmentPortUseCreate",
    bindings: &[Binding {
        source: Source::AdmissionEnvironment,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

pub const PORT_ENTER: Declaration<Source> = Declaration {
    operation: "EnvironmentPortEnter",
    bindings: &[Binding {
        source: Source::AdmissionEnvironment,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

pub const OUTSIDE: Declaration<Source> = Declaration {
    operation: "NodePortOpeningList",
    bindings: &[
        Binding {
            source: Source::OutsideNode,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OutsideEnvironments,
            target: TargetResolver::ResolvedCollection("outside_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OutsideHosts,
            target: TargetResolver::ResolvedCollection("opening_hosts"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OutsidePublishers,
            target: TargetResolver::ResolvedCollection("opening_publishers"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesOrgs,
            target: TargetResolver::ResolvedCollection("opening_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesProjects,
            target: TargetResolver::ResolvedCollection("opening_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesTopics,
            target: TargetResolver::ResolvedCollection("opening_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesThreads,
            target: TargetResolver::ResolvedCollection("opening_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const OPENING_REVIEW: Declaration<Source> = Declaration {
    operation: "EnvironmentPortOpeningPreflight",
    bindings: &[
        Binding {
            source: Source::ReviewEnvironment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::NamedPlace,
            target: TargetResolver::ResolvedCollection("named_place"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReviewEnvironment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ReviewHosts,
            target: TargetResolver::ResolvedCollection("review_hosts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReviewPublishers,
            target: TargetResolver::ResolvedCollection("review_publishers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReviewOrgs,
            target: TargetResolver::ResolvedCollection("review_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReviewProjects,
            target: TargetResolver::ResolvedCollection("review_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReviewTopics,
            target: TargetResolver::ResolvedCollection("review_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ReviewThreads,
            target: TargetResolver::ResolvedCollection("review_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const OPENING_CREATE: Declaration<Source> = Declaration {
    operation: "EnvironmentPortOpeningCreate",
    bindings: &[
        Binding {
            source: Source::OpeningEnvironment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::NamedPlace,
            target: TargetResolver::ResolvedCollection("named_place"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OpeningEnvironment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OpeningHosts,
            target: TargetResolver::ResolvedCollection("opening_hosts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OpeningPublishers,
            target: TargetResolver::ResolvedCollection("opening_publishers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesOrgs,
            target: TargetResolver::ResolvedCollection("opening_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesProjects,
            target: TargetResolver::ResolvedCollection("opening_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesTopics,
            target: TargetResolver::ResolvedCollection("opening_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesThreads,
            target: TargetResolver::ResolvedCollection("opening_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The request remains one Thread Interact act; current opening metadata is independent Read.
pub const THREAD_REQUEST: Declaration<Source> = Declaration {
    operation: "ThreadPortRequestCreate",
    bindings: &[
        Binding {
            source: Source::RequestThread,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OpeningEnvironment,
            target: TargetResolver::Resolved("request_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OpeningHosts,
            target: TargetResolver::ResolvedCollection("opening_hosts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OpeningPublishers,
            target: TargetResolver::ResolvedCollection("opening_publishers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesOrgs,
            target: TargetResolver::ResolvedCollection("opening_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesProjects,
            target: TargetResolver::ResolvedCollection("opening_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesTopics,
            target: TargetResolver::ResolvedCollection("opening_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NamesThreads,
            target: TargetResolver::ResolvedCollection("opening_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
