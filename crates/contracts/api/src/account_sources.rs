//! Actual native account selection facts for account observation operations.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Native account observation sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual authenticated account, never an inferred organization ancestor.
    CurrentAccount,
    /// Ordinary native root organizations selected for the current account.
    Organizations,
    /// Actual protected sources of their full native organization payloads.
    OrganizationContext,
    /// The actual readable first-steps org, including its stored organization observations.
    FirstOrg,
    /// Actual native providers whose stored credential/model state was observed.
    FirstProviders,
    /// The actual first completed answer retained by the native selector.
    FirstAnswer,
    /// Actually readable membership domains supporting the joined observation.
    FirstMembers,
    /// The currently readable Start here document.
    FirstDocument,
    /// Protected parent and attribution names of that document reference.
    FirstDocumentNames,
    /// Every actual account selected by the ordinary native search.
    SearchAccounts,
    /// The actual account opened by the native profile audience check.
    ProfileAccount,
    /// Native daily work rows admitted before any calendar totals are made.
    ActivityThreads,
    /// Only the native recent topics that will actually be named.
    ActivityTopics,
    /// The native ordered pins actually retained by the profile reader.
    ProfilePins,
    /// Protected names and references of full native node pins.
    ProfileContext,
    /// The currently visible membership domains and personal org.
    ProfilePlaces,
    /// Actual protected parent and attribution names of pinned documents.
    ProfileDocumentNames,
    /// Actual direct shares selected under both people's existing native visibility.
    SharedResults,
    /// Protected native parent and attribution names of returned shared documents.
    SharedDocumentNames,
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
    /// Actual native readable chats carrying the account's personal chat state.
    Chats,
}

/// Canonical mixed profile context role, also consumed by the native node projection.
pub const PROFILE_CONTEXT_ROLE: &str = "profile_context";

/// Shared native organization context role of account shell observations.
pub const ORGANIZATION_CONTEXT_ROLE: &str = "account_organization_context";

/// The shell composes the same native personal suppliers under its own exact operation.
pub const BOOTSTRAP: Declaration<Source> = Declaration {
    operation: "MeBootstrapGet",
    bindings: &[
        Binding {
            source: Source::Account,
            target: TargetResolver::CurrentAccount,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Organizations,
            target: TargetResolver::ResolvedCollection("account_organizations"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OrganizationContext,
            target: TargetResolver::ResolvedCollection(ORGANIZATION_CONTEXT_ROLE),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Favorites,
            target: TargetResolver::ResolvedCollection("favorite_nodes"),
            usage: FacetUsage::ResponseContent,
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
        Binding {
            source: Source::ReadThreads,
            target: TargetResolver::ResolvedCollection("read_state_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Subscriptions,
            target: TargetResolver::ResolvedCollection("subscription_nodes"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Chats,
            target: TargetResolver::ResolvedCollection("personal_state_chats"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Stored first-steps observations, including source identities carried in action paths.
pub const FIRST_STEPS: Declaration<Source> = Declaration {
    operation: "MeFirstStepsGet",
    bindings: &[
        Binding {
            source: Source::CurrentAccount,
            target: TargetResolver::CurrentAccount,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::FirstOrg,
            target: TargetResolver::ResolvedCollection("first_org"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FirstProviders,
            target: TargetResolver::ResolvedCollection("first_providers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FirstAnswer,
            target: TargetResolver::ResolvedCollection("first_answer"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FirstMembers,
            target: TargetResolver::ResolvedCollection("first_members"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FirstDocument,
            target: TargetResolver::ResolvedCollection("first_document"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::FirstDocumentNames,
            target: TargetResolver::ResolvedCollection("first_document_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Current account identity and independently selected ordinary organizations.
pub const AUTHORITY: Declaration<Source> = Declaration {
    operation: "MeAuthorityGet",
    bindings: &[
        Binding {
            source: Source::CurrentAccount,
            target: TargetResolver::CurrentAccount,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Organizations,
            target: TargetResolver::ResolvedCollection("account_organizations"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::OrganizationContext,
            target: TargetResolver::ResolvedCollection(ORGANIZATION_CONTEXT_ROLE),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Direct-share results remain heterogeneous and genuinely empty when the native window is empty.
pub const USER_SHARES: Declaration<Source> = Declaration {
    operation: "UserSharesGet",
    bindings: &[
        Binding {
            source: Source::SharedResults,
            target: TargetResolver::ResolvedCollection("results"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::SharedDocumentNames,
            target: TargetResolver::ResolvedCollection("shared_document_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// A public profile does not lend its reader access to the places and pins beside it.
pub const PROFILE: Declaration<Source> = Declaration {
    operation: "ProfileGet",
    bindings: &[
        Binding {
            source: Source::ProfileAccount,
            target: TargetResolver::Resolved("account_by_username"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ProfilePins,
            target: TargetResolver::ResolvedCollection("profile_pins"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ProfileContext,
            target: TargetResolver::ResolvedCollection(PROFILE_CONTEXT_ROLE),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ProfilePlaces,
            target: TargetResolver::ResolvedCollection("profile_places"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ProfileDocumentNames,
            target: TargetResolver::ResolvedCollection("profile_document_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native account search is a collection, including a genuine empty result.
pub const USER_SEARCH: Declaration<Source> = Declaration {
    operation: "UserSearch",
    bindings: &[Binding {
        source: Source::SearchAccounts,
        target: TargetResolver::ResolvedCollection("results"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};

/// Calendar amounts belong to their real threads; recent-topic names are independently bounded.
pub const PROFILE_ACTIVITY: Declaration<Source> = Declaration {
    operation: "ProfileActivityGet",
    bindings: &[
        Binding {
            source: Source::ProfileAccount,
            target: TargetResolver::Resolved("account_by_username"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ActivityThreads,
            target: TargetResolver::ResolvedCollection("profile_activity_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ActivityTopics,
            target: TargetResolver::ResolvedCollection("profile_activity_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
