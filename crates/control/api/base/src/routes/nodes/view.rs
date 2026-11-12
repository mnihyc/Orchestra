//! A node as the API shows it to one caller: what is stored of it, its way down from the root org with the names the caller may know, and what the caller holds on it, for any number of nodes in the same few statements.
//!
//! Every list of nodes is made here. The nodes are read together with the
//! nodes above them, the access engine is asked once about all of them
//! (the caller's role, the permissions held, whether they own or administer
//! it, whether its page is still the defaults, whether it is private), and
//! what a view adds from other tables is counted for all of them at once. So
//! a list costs the same number of statements for one node as for a
//! thousand. A node the caller may not view is built all the same and marked
//! as one they may not: whoever lists decides what to do with it.

pub use crate::middleware::requests::ApiResult;

pub use orch_access_contract::permissions::Permission;

pub use orch_access_contract::roles;

pub use orch_access_contract::roles::PlaceRef;

pub use orch_api_contract::access::Role;

pub use orch_api_contract::nodes::Node;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use std::collections::BTreeSet;

pub use std::collections::HashMap;
/// A node built for one caller.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// The node.
    pub node: Node,
    /// Whether the caller may view it: holds its View permission, is not
    /// suspended where it lies, and has a token that reaches it.
    pub viewable: bool,
}

/// The native reference producer's name and thread-display authority before formatting.
pub struct ReferenceDisclosure {
    pub names: BTreeSet<Id>,
    pub displays: BTreeSet<Id>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_d95d290b5b, __api_install_d95d290b5b, __api_callback_d95d290b5b;
    slot __ApiCallbackd95d290b5b, __API_CALLBACK_D95D290B5B, __api_set_d95d290b5b;
    base orch_api_base::routes::nodes::view;

    async fn reference_disclosure<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::nodes::view::ReferenceDisclosure>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d13249f940, __api_install_d13249f940, __api_callback_d13249f940;
    slot __ApiCallbackd13249f940, __API_CALLBACK_D13249F940, __api_set_d13249f940;
    base orch_api_base::routes::nodes::view;

    async fn reference_visibility<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<std::collections::BTreeSet<orch_types::Id>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_bc5d22e369, __api_install_bc5d22e369, __api_callback_bc5d22e369;
    slot __ApiCallbackbc5d22e369, __API_CALLBACK_BC5D22E369, __api_set_bc5d22e369;
    base orch_api_base::routes::nodes::view;

    async fn reference_thread_visibility<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<std::collections::BTreeSet<orch_types::Id>>;
}

/// The ordinary native header's authority shape, without constructing a Node reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceAuthority {
    pub permissions: BTreeSet<Permission>,
    pub owns: bool,
    pub administers: bool,
    pub admin: Option<(PlaceRef, bool)>,
    pub role: Option<(roles::Role, PlaceRef)>,
}

impl ReferenceAuthority {
    pub fn preserves(&self, fresh: &Self) -> bool {
        self.permissions.is_subset(&fresh.permissions)
            && (!self.owns || fresh.owns)
            && (!self.administers || fresh.administers)
            && self.admin == fresh.admin
            && self.role == fresh.role
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_0e20fd9794, __api_install_0e20fd9794, __api_callback_0e20fd9794;
    slot __ApiCallback0e20fd9794, __API_CALLBACK_0E20FD9794, __api_set_0e20fd9794;
    base orch_api_base::routes::nodes::view;

    fn admin_access<'__api>(arg0: orch_access_contract::roles::AdminReach, arg1: &'__api std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::NodeRef>) -> Option<orch_api_contract::access::AdminAccess>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c92383aa47, __api_install_c92383aa47, __api_callback_c92383aa47;
    slot __ApiCallbackc92383aa47, __API_CALLBACK_C92383AA47, __api_set_c92383aa47;
    base orch_api_base::routes::nodes::view;

    fn admin_place<'__api>(arg0: orch_access_contract::roles::PlaceRef, arg1: &'__api std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::NodeRef>) -> Option<orch_api_contract::nodes::NodeOrSystem>;
}

/// Names provenance from any batched frame in one qualified-place lookup.
pub async fn named_admin_access<K: Clone + Eq + std::hash::Hash>(
    conn: &mut Conn,
    principal: &Principal,
    places: &HashMap<K, orch_access_contract::roles::AdminReach>,
) -> ApiResult<HashMap<K, orch_api_contract::access::AdminAccess>> {
    let ids: Vec<Id> = places
        .values()
        .filter_map(|reach| reach.place.node())
        .collect();
    let refs = qualified_refs(conn, principal, &ids, Policy::default()).await?;
    Ok(places
        .iter()
        .filter_map(|(id, place)| admin_access(*place, &refs).map(|marker| (id.clone(), marker)))
        .collect())
}

orch_api_base::api_family_callback! {
    bind __api_bind_d00ac38ead, __api_install_d00ac38ead, __api_callback_d00ac38ead;
    slot __ApiCallbackd00ac38ead, __API_CALLBACK_D00AC38EAD, __api_set_d00ac38ead;
    base orch_api_base::routes::nodes::view;

    async fn build<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: chrono::DateTime<chrono::Utc>, arg4: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_base::routes::nodes::view::Built>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9f5afc18da, __api_install_9f5afc18da, __api_callback_9f5afc18da;
    slot __ApiCallback9f5afc18da, __API_CALLBACK_9F5AFC18DA, __api_set_9f5afc18da;
    base orch_api_base::routes::nodes::view;

    async fn visible<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: chrono::DateTime<chrono::Utc>, arg4: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::nodes::Node>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_04c4dded17, __api_install_04c4dded17, __api_callback_04c4dded17;
    slot __ApiCallback04c4dded17, __API_CALLBACK_04C4DDED17, __api_set_04c4dded17;
    base orch_api_base::routes::nodes::view;

    async fn visible_with_read_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: chrono::DateTime<chrono::Utc>, arg4: &'__api std::collections::BTreeSet<orch_types::Id>, arg5: &'__api std::collections::BTreeSet<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::nodes::Node>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_fd6dc0f5ec, __api_install_fd6dc0f5ec, __api_callback_fd6dc0f5ec;
    slot __ApiCallbackfd6dc0f5ec, __API_CALLBACK_FD6DC0F5EC, __api_set_fd6dc0f5ec;
    base orch_api_base::routes::nodes::view;

    async fn build_with_read_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: chrono::DateTime<chrono::Utc>, arg4: orch_authz::Policy, arg5: &'__api std::collections::BTreeSet<orch_types::Id>, arg6: &'__api std::collections::BTreeSet<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_base::routes::nodes::view::Built>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d2452234dd, __api_install_d2452234dd, __api_callback_d2452234dd;
    slot __ApiCallbackd2452234dd, __API_CALLBACK_D2452234DD, __api_set_d2452234dd;
    base orch_api_base::routes::nodes::view;

    async fn one<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: chrono::DateTime<chrono::Utc>, arg4: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<Option<orch_api_contract::nodes::Node>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2d073aa738, __api_install_2d073aa738, __api_callback_2d073aa738;
    slot __ApiCallback2d073aa738, __API_CALLBACK_2D073AA738, __api_set_2d073aa738;
    base orch_api_base::routes::nodes::view;

    async fn one_with_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: chrono::DateTime<chrono::Utc>, arg4: orch_authz::Policy, arg5: &'__api std::collections::BTreeSet<orch_types::Id>, arg6: Option<&'__api orch_api_base::routes::environments::reading::TopicSummarySources>) -> orch_api_base::middleware::requests::ApiResult<Option<orch_api_contract::nodes::Node>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ccc8c65fdf, __api_install_ccc8c65fdf, __api_callback_ccc8c65fdf;
    slot __ApiCallbackccc8c65fdf, __API_CALLBACK_CCC8C65FDF, __api_set_ccc8c65fdf;
    base orch_api_base::routes::nodes::view;

    async fn ways<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::nodes::NodeRef>>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_800650d18e, __api_install_800650d18e, __api_callback_800650d18e;
    slot __ApiCallback800650d18e, __API_CALLBACK_800650D18E, __api_set_800650d18e;
    base orch_api_base::routes::nodes::view;

    async fn ways_with_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy, arg4: &'__api std::collections::BTreeSet<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::nodes::NodeRef>>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c08cec6978, __api_install_c08cec6978, __api_callback_c08cec6978;
    slot __ApiCallbackc08cec6978, __API_CALLBACK_C08CEC6978, __api_set_c08cec6978;
    base orch_api_base::routes::nodes::view;

    async fn refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::NodeRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b615bcd360, __api_install_b615bcd360, __api_callback_b615bcd360;
    slot __ApiCallbackb615bcd360, __API_CALLBACK_B615BCD360, __api_set_b615bcd360;
    base orch_api_base::routes::nodes::view;

    async fn refs_with_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy, arg4: &'__api std::collections::BTreeSet<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::NodeRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_88b5309e9a, __api_install_88b5309e9a, __api_callback_88b5309e9a;
    slot __ApiCallback88b5309e9a, __API_CALLBACK_88B5309E9A, __api_set_88b5309e9a;
    base orch_api_base::routes::nodes::view;

    async fn qualified_refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::NodeRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6cc7a07912, __api_install_6cc7a07912, __api_callback_6cc7a07912;
    slot __ApiCallback6cc7a07912, __API_CALLBACK_6CC7A07912, __api_set_6cc7a07912;
    base orch_api_base::routes::nodes::view;

    fn qualified_from<'__api>(arg0: &'__api orch_authz::engine::Graph, arg1: &'__api std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::NodeRef>, arg2: orch_types::Id) -> Option<orch_api_contract::nodes::NodeRef>;
}
