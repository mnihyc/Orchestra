//! Hierarchy with orgs inside orgs, a project and a department moved inside their tree, archive, what was deleted and its restore, tags, subscriptions, a place's Activity, members with their domain, moving one and suspending one, and the end of a membership.
//!
//! The tree a person sees is made here: a node by itself, what lies directly
//! inside it, the orgs they belong to, the way down from a root org with
//! the names they may know, and the nodes of a whole tree for a view of it.
//! Making a node (an org inside an org, a project, a topic), renaming it,
//! archiving it and bringing it back, and the person's own subscription to
//! it are changes; every change writes the audit row its action has in the
//! same transaction, stamps the node into the feed of its tree, and is
//! decided by the access engine and nobody else. A thread is a node too,
//! but its view and its making belong to the threads routes: a list leaves
//! threads out, and the requests about threads are not bound here.

pub use orch_api_contract::nodes::NodeKind;

pub use orch_types::Id;
orch_api_base::api_family_callback! {
    bind __api_bind_6a67041e9d, __api_install_6a67041e9d, __api_callback_6a67041e9d;
    slot __ApiCallback6a67041e9d, __API_CALLBACK_6A67041E9D, __api_set_6a67041e9d;
    base orch_api_base::routes::nodes;

    async fn action<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::registry::Ctx, orch_db::Tx, orch_authz::Principal)>;
}

/// The name of a topic made without one, until automation or a person names it.
pub const NEW_TOPIC: &str = "New topic";

orch_api_base::api_family_callback! {
    bind __api_bind_323e7e7445, __api_install_323e7e7445, __api_callback_323e7e7445;
    slot __ApiCallback323e7e7445, __API_CALLBACK_323E7E7445, __api_set_323e7e7445;
    base orch_api_base::routes::nodes;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_5bb02f8208, __api_install_5bb02f8208, __api_callback_5bb02f8208;
    slot __ApiCallback5bb02f8208, __API_CALLBACK_5BB02F8208, __api_set_5bb02f8208;
    base orch_api_base::routes::nodes;

    fn invalid<'__api>(arg0: &'__api str, arg1: &'__api str) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8b7af4dd87, __api_install_8b7af4dd87, __api_callback_8b7af4dd87;
    slot __ApiCallback8b7af4dd87, __API_CALLBACK_8B7AF4DD87, __api_set_8b7af4dd87;
    base orch_api_base::routes::nodes;

    fn clean_name<'__api>(arg0: &'__api str, arg1: &'__api str) -> orch_api_base::middleware::requests::ApiResult<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ad86b076cb, __api_install_ad86b076cb, __api_callback_ad86b076cb;
    slot __ApiCallbackad86b076cb, __API_CALLBACK_AD86B076CB, __api_set_ad86b076cb;
    base orch_api_base::routes::nodes;

    fn clean_description<'__api>(arg0: &'__api str, arg1: &'__api str) -> orch_api_base::middleware::requests::ApiResult<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_4e4c0b11bd, __api_install_4e4c0b11bd, __api_callback_4e4c0b11bd;
    slot __ApiCallback4e4c0b11bd, __API_CALLBACK_4E4C0B11BD, __api_set_4e4c0b11bd;
    base orch_api_base::routes::nodes;

    async fn policy_at<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_authz::Policy>;
}

/// Where a node stands, as the audit row of an act on it needs it.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    /// The node.
    pub id: Id,
    /// Its root org.
    pub org: Id,
}

orch_api_base::api_family_callback! {
    bind __api_bind_ab7c5eef30, __api_install_ab7c5eef30, __api_callback_ab7c5eef30;
    slot __ApiCallbackab7c5eef30, __API_CALLBACK_AB7C5EEF30, __api_set_ab7c5eef30;
    base orch_api_base::routes::nodes;

    async fn record<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_access_contract::audit::AuditAction, arg3: orch_api_base::routes::nodes::Placed, arg4: &'__api str, arg5: orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_82375a5d88, __api_install_82375a5d88, __api_callback_82375a5d88;
    slot __ApiCallback82375a5d88, __API_CALLBACK_82375A5D88, __api_set_82375a5d88;
    base orch_api_base::routes::nodes;

    async fn record_about<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_access_contract::audit::AuditAction, arg3: orch_types::Id, arg4: orch_api_base::routes::nodes::Placed, arg5: &'__api str, arg6: orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d22b9e8a4f, __api_install_d22b9e8a4f, __api_callback_d22b9e8a4f;
    slot __ApiCallbackd22b9e8a4f, __API_CALLBACK_D22B9E8A4F, __api_set_d22b9e8a4f;
    base orch_api_base::routes::nodes;

    async fn placed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::routes::nodes::Placed, orch_api_contract::nodes::NodeKind)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d3e68dd0c4, __api_install_d3e68dd0c4, __api_callback_d3e68dd0c4;
    slot __ApiCallbackd3e68dd0c4, __API_CALLBACK_D3E68DD0C4, __api_set_d3e68dd0c4;
    base orch_api_base::routes::nodes;

    fn actor_name<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_fad93929f7, __api_install_fad93929f7, __api_callback_fad93929f7;
    slot __ApiCallbackfad93929f7, __API_CALLBACK_FAD93929F7, __api_set_fad93929f7;
    base orch_api_base::routes::nodes;

    fn node_id<'__api>(arg0: &'__api orch_api_contract::nodes::Node) -> orch_types::Id;
}

orch_api_base::api_family_callback! {
    bind __api_bind_868fc59953, __api_install_868fc59953, __api_callback_868fc59953;
    slot __ApiCallback868fc59953, __API_CALLBACK_868FC59953, __api_set_868fc59953;
    base orch_api_base::routes::nodes;

    async fn shown<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::nodes::Node>;
}

#[path = "nodes/content_sources.rs"]
pub mod content_sources;

#[path = "nodes/make.rs"]
pub mod make;

#[path = "nodes/moves.rs"]
pub mod moves;

#[path = "nodes/overview.rs"]
pub mod overview;

#[path = "nodes/projection.rs"]
pub mod projection;

#[path = "nodes/read_sources.rs"]
pub mod read_sources;

#[path = "nodes/tree.rs"]
pub mod tree;

#[path = "nodes/tree_work.rs"]
pub mod tree_work;

#[path = "nodes/view.rs"]
pub mod view;

#[path = "nodes/tree_docs.rs"]
pub mod tree_docs;
