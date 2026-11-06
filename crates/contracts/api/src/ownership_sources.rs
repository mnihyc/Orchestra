//! Actual native ownership observation subjects, including retained identities.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind as K;

/// Sources selected by ownership readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual native recoverable root org rows selected for their owner.
    DeletedOrgs,
    /// The membership's stored domain, independent of the requested org address.
    MemberDomain,
    /// Actual child resource references selected by native membership preparation.
    HoldingsResources,
    /// Actual physical machine references of the selected registrations.
    HoldingsMachines,
    /// The actual destination selected by native membership-move validation.
    MemberDestination,
    /// The moved member's actual Account identity, independent of human memberships.
    MemberAccount,
    /// Native org path/qualification sources in a full member response.
    MemberNames,
    /// Actual live deletion-review subject selected by the native lifecycle planner.
    DeleteSubject,
    /// Existing node content of a completed review, separate from its Manage admission.
    DeleteReviewNode,
    /// Actual restoration authority references in the native review.
    DeleteReviewPlaces,
    /// Native ownership and personal-registration subjects of an ending.
    MemberEffects,
    /// Actual successor orgs selected by the native ownership/retained-node plan.
    MemberDestinations,
    /// The member selected for a pure move preview, without granting profile disclosure.
    PreviewAccount,
    /// Actual qualification and role-content places of the native move preview.
    PreviewPlaces,
    /// Actual retained root or restoration parent selected by native recovery.
    RestorationAuthority,
    /// Native saved resources whose flags the restoration changes.
    RestorationResources,
    /// Native Undo members whose archive state is independently reopened.
    RestorationUnarchive,
    /// The real restored Node, separate from its completed Manage admission.
    RestoredNode,
    /// Actual factual Node header reference sources selected before formatting.
    RestoredContent,
}

pub const RESTORE_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
        ],
        TargetResolver::Resolved("restoration_parent"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(K::Provider),
            KeyTargetKind::Object(K::Agent),
            KeyTargetKind::Object(K::Environment),
            KeyTargetKind::Object(K::Host),
            KeyTargetKind::Object(K::Machine),
            KeyTargetKind::Object(K::Template),
            KeyTargetKind::Object(K::McpServer),
            KeyTargetKind::Object(K::Mount),
            KeyTargetKind::Object(K::Skill),
        ],
        TargetResolver::ResolvedCollection("restoration_resources"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
        ],
        TargetResolver::ResolvedCollection("restoration_unarchive"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
        ],
        TargetResolver::Resolved("restored_node"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
            KeyTargetKind::Object(K::Document),
            KeyTargetKind::Object(K::DocumentContainer),
            KeyTargetKind::Object(K::Provider),
            KeyTargetKind::Object(K::Agent),
            KeyTargetKind::Object(K::Environment),
            KeyTargetKind::Object(K::Host),
            KeyTargetKind::Object(K::Machine),
            KeyTargetKind::Object(K::Template),
            KeyTargetKind::Object(K::McpServer),
            KeyTargetKind::Object(K::Mount),
            KeyTargetKind::Object(K::Skill),
        ],
        TargetResolver::ResolvedCollection("restored_content"),
    ),
];

pub const RESTORE: Declaration<Source> = Declaration {
    operation: "NodeRestore",
    bindings: &[
        Binding {
            source: Source::RestorationAuthority,
            target: TargetResolver::Resolved("restoration_parent"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::RestorationResources,
            target: TargetResolver::ResolvedCollection("restoration_resources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::RestorationUnarchive,
            target: TargetResolver::ResolvedCollection("restoration_unarchive"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::RestoredNode,
            target: TargetResolver::Resolved("restored_node"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::RestoredContent,
            target: TargetResolver::ResolvedCollection("restored_content"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const DELETE_PREVIEW_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
        ],
        TargetResolver::Resolved("delete_subject"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
        ],
        TargetResolver::Resolved("delete_review_node"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
        ],
        TargetResolver::ResolvedCollection("delete_review_places"),
    ),
];

pub const DELETE_PREVIEW: Declaration<Source> = Declaration {
    operation: "NodeDeletePreview",
    bindings: &[
        Binding {
            source: Source::DeleteSubject,
            target: TargetResolver::Resolved("delete_subject"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::DeleteReviewNode,
            target: TargetResolver::Resolved("delete_review_node"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::DeleteReviewPlaces,
            target: TargetResolver::ResolvedCollection("delete_review_places"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const MEMBER_END_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::Resolved("member_domain"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
            KeyTargetKind::Object(K::Document),
            KeyTargetKind::Object(K::DocumentContainer),
            KeyTargetKind::Object(K::Provider),
            KeyTargetKind::Object(K::Agent),
            KeyTargetKind::Object(K::Environment),
            KeyTargetKind::Object(K::Host),
            KeyTargetKind::Object(K::Machine),
            KeyTargetKind::Object(K::Template),
            KeyTargetKind::Object(K::McpServer),
            KeyTargetKind::Object(K::Mount),
            KeyTargetKind::Object(K::Skill),
        ],
        TargetResolver::ResolvedCollection("member_effects"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::ResolvedCollection("member_destinations"),
    ),
];
const MEMBER_END_BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::MemberDomain,
        target: TargetResolver::Resolved("member_domain"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::MemberEffects,
        target: TargetResolver::ResolvedCollection("member_effects"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::MemberDestinations,
        target: TargetResolver::ResolvedCollection("member_destinations"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];
pub const MEMBER_LEAVE: Declaration<Source> = Declaration {
    operation: "OrgMemberRemove.leave",
    bindings: MEMBER_END_BINDINGS,
};
pub const MEMBER_REMOVE: Declaration<Source> = Declaration {
    operation: "OrgMemberRemove.remove",
    bindings: MEMBER_END_BINDINGS,
};

pub const MEMBER_MOVE_PREVIEW_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::Resolved("member_domain"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::Resolved("member_destination"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(
            orch_access_contract::key_scope::KeyIdentityKind::Account,
        )],
        TargetResolver::ResolvedCollection("preview_accounts"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(K::Org),
            KeyTargetKind::Object(K::Project),
            KeyTargetKind::Object(K::Topic),
            KeyTargetKind::Object(K::Thread),
        ],
        TargetResolver::ResolvedCollection("preview_places"),
    ),
];

pub const MEMBER_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "OrgMemberMovePreview",
    bindings: &[
        MEMBER_MOVE.bindings[0],
        MEMBER_MOVE.bindings[1],
        Binding {
            source: Source::PreviewAccount,
            target: TargetResolver::ResolvedCollection("preview_accounts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::PreviewPlaces,
            target: TargetResolver::ResolvedCollection("preview_places"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const MEMBER_MOVE_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::Resolved("member_domain"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::Resolved("member_destination"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(
            orch_access_contract::key_scope::KeyIdentityKind::Account,
        )],
        TargetResolver::Resolved("member_account"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::ResolvedCollection("member_names"),
    ),
];

pub const MEMBER_MOVE: Declaration<Source> = Declaration {
    operation: "OrgMemberMove",
    bindings: &[
        Binding {
            source: Source::MemberDomain,
            target: TargetResolver::Resolved("member_domain"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::MemberDestination,
            target: TargetResolver::Resolved("member_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::MemberAccount,
            target: TargetResolver::Resolved("member_account"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::MemberNames,
            target: TargetResolver::ResolvedCollection("member_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const MEMBER_HOLDINGS_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(K::Org)],
        TargetResolver::Resolved("member_domain"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(K::Provider),
            KeyTargetKind::Object(K::Agent),
            KeyTargetKind::Object(K::Environment),
            KeyTargetKind::Object(K::Host),
            KeyTargetKind::Object(K::Machine),
            KeyTargetKind::Object(K::Template),
            KeyTargetKind::Object(K::McpServer),
            KeyTargetKind::Object(K::Mount),
            KeyTargetKind::Object(K::Skill),
        ],
        TargetResolver::ResolvedCollection("holdings_resources"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(K::Machine)],
        TargetResolver::ResolvedCollection("holdings_machines"),
    ),
];

/// One native membership preview supplies the aggregate or its full protected references.
pub const MEMBER_HOLDINGS: Declaration<Source> = Declaration {
    operation: "OrgMemberHoldings",
    bindings: &[
        Binding {
            source: Source::MemberDomain,
            target: TargetResolver::Resolved("member_domain"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::HoldingsResources,
            target: TargetResolver::ResolvedCollection("holdings_resources"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::HoldingsMachines,
            target: TargetResolver::ResolvedCollection("holdings_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Deletion-list inspection uses current retained identities, never a live Get surrogate.
pub const DELETED_ORGS: Declaration<Source> = Declaration {
    operation: "OrgDeletedList",
    bindings: &[Binding {
        source: Source::DeletedOrgs,
        target: TargetResolver::ResolvedCollection("results"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};
