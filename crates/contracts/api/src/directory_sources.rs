//! Native directory subjects and selected rows shared by their operation adapters.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};

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
