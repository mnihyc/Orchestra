//! Pools are ordinary managed resources; member identities remain permission-filtered.
use crate::access::{NewResourceAccess, ResourceAccess};
use crate::common::{Listed, ReceiptProjection};
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, OperationSpec, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, ResourceKind};
use orch_provider_contract::pools::{Definition, Kind, Member, MemberRule, Policy};
use orch_types::{Id, Json, Revision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The same native family and protected name roles as other grouped resources.
pub const READS: crate::resource_reads::Family = crate::resource_reads::Family {
    kind: ResourceKind::Pool,
    sources: &[],
    names: crate::resource_reads::NameRoles {
        groups: Some("pool_groups"),
        nodes: [
            "pool_names_orgs",
            "pool_names_projects",
            "pool_names_topics",
            "pool_names_threads",
        ],
        containers: None,
    },
};

/// Ordinary pool metadata; the written and expanded member lists have their own paged reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Pool {
    pub id: Id,
    pub name: String,
    pub description: String,
    pub address: String,
    pub kind: Kind,
    /// Selection intent only; native Use and member admission remain separate.
    pub target: orch_provider_contract::pools::Target,
    pub enabled: bool,
    pub policy: Option<Policy>,
    pub lend_members: bool,
    pub revision: Revision,
    pub owner: Option<crate::users::UserRef>,
    pub home: Option<crate::nodes::NodeRef>,
    pub access: ResourceAccess,
    pub grouping: crate::resource_groups::ResourceGrouping,
    pub settings: Vec<crate::settings::ResolvedSetting>,
}

/// Native inventory filtering; grouping does not confer permission.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// Initial creation is one ordinary resource act under this actual home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolCreateBody {
    pub home_id: Id,
    pub name: String,
    pub description: String,
    pub definition: Definition,
    pub access: NewResourceAccess,
}

/// Copy the current authored pool under actual source Read and destination creation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolDuplicateBody {
    pub expected_revision: Revision,
    pub home_id: Id,
    pub name: String,
    pub access: NewResourceAccess,
}

/// Metadata edits preserve every written member and the immutable family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolUpdateBody {
    pub expected_revision: Revision,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub policy: Option<Policy>,
    pub lend_members: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
}

/// One written member is edited without resending a hidden or unloaded member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolMemberPutBody {
    pub expected_revision: Revision,
    pub member: Member,
    /// Omitted for a new row appends; omitted on an existing row preserves its position.
    pub position: Option<i64>,
}

/// An accepted mutation receipt contains no private member or pool definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolReceipt {
    pub projection: ReceiptProjection,
    pub id: Id,
    pub revision: Revision,
}

/// A member's own identity and authored policy remain separate from its protected target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolMemberView {
    pub id: Id,
    pub position: i64,
    pub enabled: bool,
    pub priority: Option<u64>,
    pub weight: Option<String>,
    pub rule: Option<MemberRule>,
    pub label: Option<String>,
    pub reason: Option<String>,
}

/// Stable written-member pagination; the server takes its batch size from the registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolMembersQuery {
    pub after_position: Option<i64>,
    pub after_id: Option<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolMembersReply {
    pub items: Vec<PoolMemberView>,
    pub next: Option<PoolMembersQuery>,
}

/// Actual native sources of pool mutations, independently of response content.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Home,
    Subject,
    Members,
}
const POOL: &[KeyTargetKind] = &[KeyTargetKind::Object(ObjectKind::Pool)];
pub const CREATE: Declaration<Source> = Declaration {
    operation: "PoolCreate",
    bindings: &[Binding {
        source: Source::Home,
        target: TargetResolver::Resolved("pool_home"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
pub const DUPLICATE: Declaration<Source> = Declaration {
    operation: "PoolDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Home,
            target: TargetResolver::Resolved("pool_home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
pub const UPDATE: Declaration<Source> = Declaration {
    operation: "PoolUpdate",
    bindings: &[Binding {
        source: Source::Subject,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
pub const MEMBERS: Declaration<Source> = Declaration {
    operation: "PoolMembersGet",
    bindings: MEMBER_READ_BINDINGS,
};
const MEMBER_READ_BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::Subject,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Members,
        target: TargetResolver::ResolvedCollection("member_targets"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
pub const EXPANDED: Declaration<Source> = Declaration {
    operation: "PoolExpandedGet",
    bindings: MEMBER_READ_BINDINGS,
};
pub const CHOICES: Declaration<Source> = Declaration {
    operation: "PoolChoicesGet",
    bindings: MEMBER_READ_BINDINGS,
};
pub const CHOICE_DETAILS: Declaration<Source> = Declaration {
    operation: "PoolChoiceDetailsGet",
    bindings: MEMBER_READ_BINDINGS,
};
pub const MEMBER_DELETE: Declaration<Source> = Declaration {
    operation: "PoolMemberDelete",
    bindings: UPDATE.bindings,
};
pub const DELETE: Declaration<Source> = Declaration {
    operation: "PoolDelete",
    bindings: UPDATE.bindings,
};
pub const RECOVER: Declaration<Source> = Declaration {
    operation: "PoolRecover",
    bindings: UPDATE.bindings,
};
pub const MEMBER_METADATA: Declaration<Source> = Declaration {
    operation: "PoolMemberMetadata",
    bindings: UPDATE.bindings,
};
pub const MEMBER_PUT: Declaration<Source> = Declaration {
    operation: "PoolMemberPut",
    bindings: UPDATE.bindings,
};

crate::endpoints! {
    /// Reads one ordinarily visible pool without disclosing hidden member identities.
    PoolGet: GET "/pools/{id}", crate::routes::Access::Permission("pool.view"), none, none, json Pool, 200,
        operation OperationSpec::facets(&READS.facets::<6>(TargetResolver::Path("id")));
    /// Lists the current caller's ordinary pool resources with native home and name filtering.
    PoolList: GET "/pools", crate::routes::Access::User, query PoolListQuery, none, json Vec<Listed<Pool>>, 200,
        operation OperationSpec::facets(&READS.facets::<6>(TargetResolver::ResolvedCollection("results")));
    /// Creates one configured resource pool under the actual native home authority.
    PoolCreate: POST "/pools", crate::routes::Access::User, none, json PoolCreateBody, json PoolReceipt, 201,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage,POOL,TargetResolver::Resolved("pool_home")));
    /// Copies every authored rule without copying permissions, consents, rotation or executions.
    PoolDuplicate: POST "/pools/{id}/duplicate", crate::routes::Access::User, none, json PoolDuplicateBody, json PoolReceipt, 201,
        operation OperationSpec::receipt(&OperationSpec::facets(&[
            FacetSpec::required(KeyAction::Read,POOL,TargetResolver::Path("id")),
            FacetSpec::required(KeyAction::Manage,POOL,TargetResolver::Resolved("pool_home")),
        ]));
    /// Edits the pool metadata while retaining its written members and current owner.
    PoolUpdate: PUT "/pools/{id}", crate::routes::Access::Permission("pool.manage"), none, json PoolUpdateBody, json PoolReceipt, 200,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage,POOL,TargetResolver::Path("id")));
    /// Removes the pool without deleting or disabling its shared member resources.
    PoolDelete: DELETE "/pools/{id}",crate::routes::Access::Permission("pool.manage"),none,json PoolDeleteBody,json PoolReceipt,200,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage,POOL,TargetResolver::Path("id")));
    /// Releases this pool's suppression without sending a probe or changing a shared failure fact.
    PoolRecover: POST "/pools/{id}/recover",crate::routes::Access::Permission("pool.manage"),none,json PoolRecoveryBody,json PoolReceipt,200,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage,POOL,TargetResolver::Path("id")));
    /// Edits one member under the native graph lock and pool revision.
    PoolMemberPut: PUT "/pools/{id}/members/{member_id}", crate::routes::Access::Permission("pool.manage"), none, json PoolMemberPutBody, json PoolReceipt, 200,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage,POOL,TargetResolver::Path("id")));
    /// Changes one row's options without disclosing or resending its target.
    PoolMemberMetadata: PATCH "/pools/{id}/members/{member_id}",crate::routes::Access::Permission("pool.manage"),none,json PoolMemberMetadataBody,json PoolReceipt,200,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage,POOL,TargetResolver::Path("id")));
    /// Removes one written rule while preserving hidden and unloaded rows.
    PoolMemberDelete: DELETE "/pools/{id}/members/{member_id}",crate::routes::Access::Permission("pool.manage"),query PoolMemberDeleteQuery,none,json PoolReceipt,200,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage,POOL,TargetResolver::Path("id")));
    /// Reads one written-member page with current target visibility.
    PoolMembersGet: GET "/pools/{id}/members", crate::routes::Access::Permission("pool.view"), query PoolMembersQuery, none, json PoolMembersReply, 200,
        operation OperationSpec::facets(&[FacetSpec::required(KeyAction::Read,POOL,TargetResolver::Path("id")),FacetSpec::response_content(LENDING_KINDS,TargetResolver::ResolvedCollection("member_targets"))]);
    /// Reads a bounded consent page through the actual member or defaults sharing source.
    PoolLendingList: POST "/pools/lending/list",crate::routes::Access::User,none,json PoolLendingListBody,json PoolLendingReply,200,
        operation OperationSpec::variants(&[
            crate::operations::VariantSpec{name:"member",selector:"scope.kind=member",facets:&[FacetSpec::required(KeyAction::Read,LENDING_KINDS,TargetResolver::Resolved("lending_subject"))]},
            crate::operations::VariantSpec{name:"defaults",selector:"scope.kind=defaults",facets:&[FacetSpec::required(KeyAction::Read,LENDING_HOMES,TargetResolver::Resolved("lending_home"))]},
        ]);
    /// Inspects one live member window through the same shared resolver as accepted choices.
    PoolExpandedGet: GET "/pools/{id}/expanded",crate::routes::Access::Permission("pool.view"),query PoolExpandedQuery,none,json PoolExpandedReply,200,
        operation OperationSpec::facets(&[FacetSpec::required(KeyAction::Read,POOL,TargetResolver::Path("id")),FacetSpec::response_content(LENDING_KINDS,TargetResolver::ResolvedCollection("member_targets"))]);
    /// Reads recorded root choices without advancing policy or native execution.
    PoolChoicesGet: GET "/pools/{id}/choices",crate::routes::Access::Permission("pool.view"),query PoolChoicesQuery,none,json PoolChoicesReply,200,
        operation OperationSpec::facets(&[FacetSpec::required(KeyAction::Read,POOL,TargetResolver::Path("id")),FacetSpec::response_content(LENDING_KINDS,TargetResolver::ResolvedCollection("member_targets"))]);
    /// Reads recorded candidate and skip details under current member disclosure authority.
    PoolChoiceDetailsGet: GET "/pools/{id}/choices/{choice_id}",crate::routes::Access::Permission("pool.view"),query PoolChoiceDetailsQuery,none,json PoolChoiceDetailsReply,200,
        operation OperationSpec::facets(&[FacetSpec::required(KeyAction::Read,POOL,TargetResolver::Path("id")),FacetSpec::response_content(LENDING_KINDS,TargetResolver::ResolvedCollection("member_targets"))]);
    /// Changes one member-side pool consent within its native sharing authority.
    PoolLendingPut: PUT "/pools/lending/{entry_id}",crate::routes::Access::User,none,json PoolLendingPutBody,json PoolReceipt,200,
        operation OperationSpec::receipt(&OperationSpec::variants(&[
            crate::operations::VariantSpec{name:"member",selector:"scope.kind=member",facets:&[FacetSpec::required(KeyAction::Manage,LENDING_KINDS,TargetResolver::Resolved("lending_subject"))]},
            crate::operations::VariantSpec{name:"defaults",selector:"scope.kind=defaults",facets:&[FacetSpec::required(KeyAction::Manage,LENDING_HOMES,TargetResolver::Resolved("lending_home"))]},
        ]));

}

pub use orch_access_contract::pools::LendingScope;
/// An explicit member-side consent. Ordinary current sharing and typed limits bound every write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolLendingPutBody {
    pub scope: LendingScope,
    pub expected_revision: Option<Revision>,
    pub matcher: orch_access_contract::pools::LendingMatcher,
    pub options: Json,
    pub show_names: bool,
    pub enabled: bool,
    pub expires_at: Option<orch_types::Time>,
}
/// The written current consents of one native member/defaults policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolLendingListBody {
    pub scope: LendingScope,
    pub after_id: Option<Id>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolLendingReply {
    pub items: Vec<PoolLendingView>,
    pub next: Option<Id>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolLendingView {
    pub id: Id,
    pub revision: Revision,
    pub matcher: orch_access_contract::pools::LendingMatcher,
    pub options: Json,
    pub show_names: bool,
    pub expires_at: Option<orch_types::Time>,
    pub giver: Option<crate::users::UserRef>,
    pub can_revoke: bool,
}
const LENDING_KINDS: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Provider),
    KeyTargetKind::Object(ObjectKind::Pool),
    KeyTargetKind::Object(ObjectKind::Environment),
    KeyTargetKind::Object(ObjectKind::Host),
    KeyTargetKind::Object(ObjectKind::Template),
];
const LENDING_HOMES: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Org),
    KeyTargetKind::Object(ObjectKind::Project),
    KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::System),
];
pub const LENDING_MEMBER: Declaration<Source> = Declaration {
    operation: "PoolLendingPut.member",
    bindings: &[Binding {
        source: Source::Subject,
        target: TargetResolver::Resolved("lending_subject"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
pub const LENDING_DEFAULTS: Declaration<Source> = Declaration {
    operation: "PoolLendingPut.defaults",
    bindings: &[Binding {
        source: Source::Home,
        target: TargetResolver::Resolved("lending_home"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

pub const LENDING_LIST_MEMBER: Declaration<Source> = Declaration {
    operation: "PoolLendingList.member",
    bindings: LENDING_MEMBER.bindings,
};
pub const LENDING_LIST_DEFAULTS: Declaration<Source> = Declaration {
    operation: "PoolLendingList.defaults",
    bindings: LENDING_DEFAULTS.bindings,
};

/// A sealed continuation retains one bounded supplier window at each nested node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolExpandedQuery {
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolExpandedMember {
    pub resource: Option<orch_provider_contract::pools::Leaf>,
    pub label: Option<String>,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolExpandedReply {
    pub items: Vec<PoolExpandedMember>,
    pub next: Option<String>,
}

/// Newest-first root-choice pagination; both cursor fields are supplied together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolChoicesQuery {
    pub before_created_at: Option<orch_types::Time>,
    pub before_id: Option<Id>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PoolChoiceState {
    Building,
    Selected,
    Exhausted,
}
/// Saved selection facts; source authority, actor and hidden member data are never returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolChoiceView {
    pub id: Id,
    pub created_at: orch_types::Time,
    pub state: PoolChoiceState,
    pub resource: Option<orch_provider_contract::pools::Leaf>,
    pub label: Option<String>,
    pub policy: Option<Policy>,
    pub reason: Option<String>,
    pub details_expired_at: Option<orch_types::Time>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolChoicesReply {
    pub items: Vec<PoolChoiceView>,
    pub next: Option<PoolChoicesQuery>,
}
/// Detail rows use their immutable walk ordinal, not a supplier's current membership.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolChoiceDetailsQuery {
    pub after_ordinal: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PoolChoiceDetail {
    pub ordinal: i64,
    pub resource: Option<orch_provider_contract::pools::Leaf>,
    pub label: Option<String>,
    pub reason: Option<String>,
    /// The recorded request estimate in credits; absent means unknown, never zero.
    pub cost: Option<String>,
    /// How the saved estimate was calculated for this readable member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_basis: Option<PoolCostBasis>,
    /// Observations saved for the load measures used by this member's pool path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observations: Vec<PoolLoadObservation>,
}

/// A native observation at selection time. Counts stay counts and utilization is in percent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PoolLoadObservation {
    pub measure: orch_provider_contract::pools::LoadMeasure,
    pub score: f64,
    pub at: orch_types::Time,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolCostBasis {
    /// Where the rate used for this estimate came from.
    pub source: crate::usage::RateSource,
    /// Whether the estimate explicitly priced all input at the uncached rate.
    pub uncached_input: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PoolChoiceDetailsReply {
    pub id: Id,
    pub expired_at: Option<orch_types::Time>,
    pub items: Vec<PoolChoiceDetail>,
    pub next: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolMemberDeleteQuery {
    pub expected_revision: Revision,
}

/// A manager can change an authored row's options while its target remains unreadable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolMemberMetadataBody {
    pub expected_revision: Revision,
    pub enabled: bool,
    pub priority: Option<u64>,
    pub weight: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolDeleteBody {
    pub expected_revision: Revision,
}

/// The pool whose local suppression the manager reviewed; shared facts are retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolRecoveryBody {
    pub expected_revision: Revision,
}
