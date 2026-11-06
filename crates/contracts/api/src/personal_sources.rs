//! Native account-owned personal row sources for their ordinary read operations.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Sources selected by the existing personal state producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The authenticated account whose private state is read.
    Account,
    /// Actual thread identities carrying the selected read positions.
    ReadThreads,
    /// Actual node identities carrying the selected subscriptions.
    Subscriptions,
    /// Native selected favorite node identities.
    Favorites,
    /// Protected org names emitted by the native favorite path projection.
    FavoriteOrgNames,
    /// Protected project names emitted by that projection.
    FavoriteProjectNames,
    /// Protected topic names emitted by that projection.
    FavoriteTopicNames,
    /// Protected thread names emitted by that projection.
    FavoriteThreadNames,
}

const ACCOUNT: Binding<Source> = Binding {
    source: Source::Account,
    target: TargetResolver::CurrentAccount,
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};

/// Private read positions and every actual selected thread are bound separately.
pub const READ_STATES: Declaration<Source> = Declaration {
    operation: "MeReadStateList",
    bindings: &[
        ACCOUNT,
        Binding {
            source: Source::ReadThreads,
            target: TargetResolver::ResolvedCollection("read_state_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Private subscription ownership does not create ancestry for the subscribed node.
pub const SUBSCRIPTIONS: Declaration<Source> = Declaration {
    operation: "MeSubscriptionList",
    bindings: &[
        ACCOUNT,
        Binding {
            source: Source::Subscriptions,
            target: TargetResolver::ResolvedCollection("subscription_nodes"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Favorites and optional path names use their own exact operation's Read.
pub const FAVORITES: Declaration<Source> = Declaration {
    operation: "MeFavoriteList",
    bindings: &[
        ACCOUNT,
        Binding {
            source: Source::Favorites,
            target: TargetResolver::ResolvedCollection("favorite_nodes"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FavoriteOrgNames,
            target: TargetResolver::ResolvedCollection("favorite_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FavoriteProjectNames,
            target: TargetResolver::ResolvedCollection("favorite_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FavoriteTopicNames,
            target: TargetResolver::ResolvedCollection("favorite_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FavoriteThreadNames,
            target: TargetResolver::ResolvedCollection("favorite_names_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
