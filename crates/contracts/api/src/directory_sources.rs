//! Native directory subjects and selected rows shared by their operation adapters.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver as Target};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Sources supplied by the existing directory loaders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The current defining node of the stored tag.
    TagNode,
    /// The actual org in which the membership is changed.
    Organization,
    /// Current selected member account identities.
    Members,
    /// Org names and addresses actually emitted with the selected member rows.
    MemberDomains,
    /// Current selected service account identities with their real owning org.
    ServiceAccounts,
    /// The actual newly stored Group, independent of its creation parent.
    CreatedGroup,
    /// The actual newly stored service Account, with its native owning org.
    CreatedService,
    /// Optional current org labels selected by a created Group's native qualification.
    GroupOrgNames,
    /// Optional current project labels selected by that native qualification.
    GroupProjectNames,
    /// Optional current topic labels selected by that native qualification.
    GroupTopicNames,
    /// Optional current thread labels selected by that native qualification.
    GroupThreadNames,
    /// The actual stored removal review's keeping Org, after native custody validation.
    StoredPlanOrganization,
}

const TAG_REQUIRED: Binding<Source> = Binding {
    source: Source::TagNode,
    target: Target::Resolved("tag_node"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};

/// A tag update and its optional same-operation content share its actual keeper.
pub const TAG_UPDATE: Declaration<Source> = Declaration {
    operation: "TagUpdate",
    bindings: &[
        TAG_REQUIRED,
        Binding {
            usage: FacetUsage::ResponseContent,
            ..TAG_REQUIRED
        },
    ],
};

/// Deleting a tag needs no readback of stored content.
pub const TAG_DELETE: Declaration<Source> = Declaration {
    operation: "TagDelete",
    bindings: &[TAG_REQUIRED],
};

/// Suspension and optional suspension content use the current actual org.
pub const MEMBER_SUSPEND: Declaration<Source> = Declaration {
    operation: "OrgMemberSuspend",
    bindings: &[
        Binding {
            source: Source::Organization,
            target: Target::Resolved("membership_org"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Organization,
            target: Target::Resolved("membership_org"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Member selection binds actual accounts, not their membership as account ancestry.
pub const MEMBER_LIST: Declaration<Source> = Declaration {
    operation: "OrgMemberList",
    bindings: &[
        Binding {
            source: Source::Members,
            target: Target::ResolvedCollection("org_members"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::MemberDomains,
            target: Target::ResolvedCollection("member_domains"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Service selection retains the stored service-account ownership relation.
pub const SERVICE_LIST: Declaration<Source> = Declaration {
    operation: "OrgServiceAccountList",
    bindings: &[Binding {
        source: Source::ServiceAccounts,
        target: Target::ResolvedCollection("org_service_accounts"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};

/// A creation is governed by its real keeping Org with the created business family.
const CREATION_KEEPER: Binding<Source> = Binding {
    source: Source::Organization,
    target: Target::PermissionObject,
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};

const GROUP_READ: Binding<Source> = Binding {
    source: Source::CreatedGroup,
    target: Target::Resolved("created_resource"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};

/// Every protected Group qualification uses its consuming creation operation.
pub const GROUP_NAMES: [&str; 4] = [
    "created_group_orgs",
    "created_group_projects",
    "created_group_topics",
    "created_group_threads",
];

/// Ordinary native Group creation and optional native Group content.
pub const GROUP_CREATE: Declaration<Source> = Declaration {
    operation: "OrgGroupCreate",
    bindings: &[
        CREATION_KEEPER,
        GROUP_READ,
        Binding {
            source: Source::GroupOrgNames,
            target: Target::ResolvedCollection(GROUP_NAMES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::GroupProjectNames,
            target: Target::ResolvedCollection(GROUP_NAMES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::GroupTopicNames,
            target: Target::ResolvedCollection(GROUP_NAMES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::GroupThreadNames,
            target: Target::ResolvedCollection(GROUP_NAMES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Canonical actions remain declaration metadata, with explicit native role cardinalities.
pub const GROUP_CREATE_FACETS: [FacetSpec; 6] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        CREATION_KEEPER.target,
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        GROUP_READ.target,
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        Target::ResolvedCollection(GROUP_NAMES[0]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        Target::ResolvedCollection(GROUP_NAMES[1]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        Target::ResolvedCollection(GROUP_NAMES[2]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        Target::ResolvedCollection(GROUP_NAMES[3]),
    ),
];

/// The native service Account's intrinsic public user references add no private source.
pub const SERVICE_CREATE: Declaration<Source> = Declaration {
    operation: "OrgServiceAccountCreate",
    bindings: &[
        CREATION_KEEPER,
        Binding {
            source: Source::CreatedService,
            target: Target::Resolved("created_resource"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Creating an Account at an Org never turns a human's membership into account ancestry.
pub const SERVICE_CREATE_FACETS: [FacetSpec; 2] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
        CREATION_KEEPER.target,
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
        Target::Resolved("created_resource"),
    ),
];

/// Stored management-review content stays governed by its actual native keeping Org.
pub const OUTSIDER_PLAN: Declaration<Source> = Declaration {
    operation: "OrgOutsiderRemovePlanGet",
    bindings: &[Binding {
        source: Source::StoredPlanOrganization,
        target: Target::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
