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
    /// Optional names in legacy token restrictions.
    TokenOrgNames,
    /// Project labels in token restrictions.
    TokenProjectNames,
    /// Topic labels in token restrictions.
    TokenTopicNames,
    /// Thread labels in token restrictions.
    TokenThreadNames,
    /// The actual stored token, independent of its restriction node.
    Token,
    /// Optional error disclosure of the stored token revocation.
    TokenContent,
    /// The actual selected browser session.
    Session,
    /// Every actual unrevoked session of the selected account.
    Sessions,
    /// The actual account governing bulk credential management.
    Account,
    /// Optional same-operation revocation error disclosure.
    SessionContent,
}
/// Current system authority and same-operation content for a created sign-in method.
pub const IDENTITY_PROVIDER_CREATE: Declaration<Source> = Declaration {
    operation: "AdminIdentityProviderCreate",
    bindings: &[
        Binding {
            source: Source::Results,
            target: Target::System,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Content,
            target: Target::System,
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
/// Current system account administration governs a created person's private metadata.
pub const USER_CREATE: Declaration<Source> = Declaration {
    operation: "AdminUserCreate",
    bindings: &[
        Binding {
            source: Source::Results,
            target: Target::System,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Content,
            target: Target::System,
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
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

/// Credential metadata is selected by actual Key identity, not its legacy node restriction.
pub const TOKENS: Declaration<Source> = Declaration {
    operation: "AdminTokenList",
    bindings: &[
        RESULTS,
        Binding {
            source: Source::TokenOrgNames,
            target: Target::ResolvedCollection("token_org_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::TokenProjectNames,
            target: Target::ResolvedCollection("token_project_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::TokenTopicNames,
            target: Target::ResolvedCollection("token_topic_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::TokenThreadNames,
            target: Target::ResolvedCollection("token_thread_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
/// Revocation binds the stored Key and actual owning Account association.
pub const TOKEN_REVOKE: Declaration<Source> = Declaration {
    operation: "AdminTokenDelete",
    bindings: &[
        Binding {
            source: Source::Token,
            target: Target::Resolved("token"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::TokenContent,
            target: Target::Resolved("token"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Per-person inventory uses the same native token metadata roles.
pub const USER_TOKENS: Declaration<Source> = Declaration {
    operation: "AdminUserTokenList",
    bindings: TOKENS.bindings,
};
const fn credential(
    source: Source,
    parameter: &'static str,
    kind: orch_access_contract::key_scope::KeyIdentityKind,
    usage: FacetUsage,
) -> Binding<Source> {
    Binding {
        source,
        target: match kind {
            orch_access_contract::key_scope::KeyIdentityKind::Key => Target::Resolved("token"),
            orch_access_contract::key_scope::KeyIdentityKind::Session => {
                Target::Resolved("session")
            }
            _ => Target::TypedPath {
                parameter,
                kind: orch_access_contract::key_scope::KeyTargetKind::Identity(kind),
            },
        },
        usage,
        cardinality: Cardinality::One,
    }
}
/// Actual token of the person named by the request.
pub const USER_TOKEN_REVOKE: Declaration<Source> = Declaration {
    operation: "AdminUserTokenDelete",
    bindings: &[
        credential(
            Source::Token,
            "token_id",
            orch_access_contract::key_scope::KeyIdentityKind::Key,
            FacetUsage::Required,
        ),
        credential(
            Source::TokenContent,
            "token_id",
            orch_access_contract::key_scope::KeyIdentityKind::Key,
            FacetUsage::ResponseContent,
        ),
    ],
};
/// Retained Key history has its own current Read, never authentication authority.
pub const USER_TOKEN_ACTIVITY: Declaration<Source> = Declaration {
    operation: "AdminUserTokenActivityList",
    bindings: &[credential(
        Source::Token,
        "token_id",
        orch_access_contract::key_scope::KeyIdentityKind::Key,
        FacetUsage::Required,
    )],
};
/// Each emitted session keeps its actual owning Account witness.
pub const USER_SESSIONS: Declaration<Source> = Declaration {
    operation: "AdminUserSessionList",
    bindings: &[RESULTS],
};
/// Single-session revocation never borrows a session from another person.
pub const USER_SESSION_REVOKE: Declaration<Source> = Declaration {
    operation: "AdminUserSessionDelete",
    bindings: &[
        credential(
            Source::Session,
            "session_id",
            orch_access_contract::key_scope::KeyIdentityKind::Session,
            FacetUsage::Required,
        ),
        credential(
            Source::SessionContent,
            "session_id",
            orch_access_contract::key_scope::KeyIdentityKind::Session,
            FacetUsage::ResponseContent,
        ),
    ],
};
/// All means the complete guarded set, never the subset a credential happens to permit.
pub const USER_SESSIONS_REVOKE: Declaration<Source> = Declaration {
    operation: "AdminUserSessionsDelete",
    bindings: &[
        credential(
            Source::Account,
            "id",
            orch_access_contract::key_scope::KeyIdentityKind::Account,
            FacetUsage::Required,
        ),
        Binding {
            source: Source::Sessions,
            target: Target::ResolvedCollection("session_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        credential(
            Source::SessionContent,
            "id",
            orch_access_contract::key_scope::KeyIdentityKind::Account,
            FacetUsage::ResponseContent,
        ),
    ],
};
