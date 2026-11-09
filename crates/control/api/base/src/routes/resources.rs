//! Providers, listed by what the caller owns, may use and manages, with their setup, sign-in by each declared method, allowed arrangement types and runtime (start, stop, update plans), models, profiles, hosts with their stored metrics, machines with their policy, templates of both kinds with their import, drivers, MCP presets and imported servers, and mounts; the owners of a resource.
//!
//! Every decision is the access engine's: who may see a resource, use it, manage it, make one of a
//! kind in a place, and what a refusal says is missing. What is written here is the translation to
//! and from the wire, the rows each request reads and writes, and the audit row of every change,
//! made in the transaction of the change. A key, a secret header and every other secret that a
//! request brings is sealed before it is stored and is returned by no request.
//!
//! The requests are bound by kind of resource, each in a module of its own below.

pub use orch_types::Id;
/// How many resources a list shows. The lists of the contract have no cursor, so a list holds the
/// first this many that the caller may see, in the order of their identifiers; a place with more
/// resources of one kind than this narrows its lists with `org_id`.
pub const LIST_LIMIT: usize = 500;

orch_api_base::api_family_callback! {
    bind __api_bind_b76d4af877, __api_install_b76d4af877, __api_callback_b76d4af877;
    slot __ApiCallbackb76d4af877, __API_CALLBACK_B76D4AF877, __api_set_b76d4af877;
    base orch_api_base::routes::resources;

    async fn begin_mutation<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::registry::Ctx, orch_db::Tx, orch_authz::Principal)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8cb26d9e4c, __api_install_8cb26d9e4c, __api_callback_8cb26d9e4c;
    slot __ApiCallback8cb26d9e4c, __API_CALLBACK_8CB26D9E4C, __api_set_8cb26d9e4c;
    base orch_api_base::routes::resources;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_08053fcda3, __api_install_08053fcda3, __api_callback_08053fcda3;
    slot __ApiCallback08053fcda3, __API_CALLBACK_08053FCDA3, __api_set_08053fcda3;
    base orch_api_base::routes::resources;

    fn clean_name<'__api>(arg0: &'__api str, arg1: &'__api str) -> orch_api_base::middleware::requests::ApiResult<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6286b1a27a, __api_install_6286b1a27a, __api_callback_6286b1a27a;
    slot __ApiCallback6286b1a27a, __API_CALLBACK_6286B1A27A, __api_set_6286b1a27a;
    base orch_api_base::routes::resources;

    async fn require<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_access_contract::permissions::ResourceKind, arg3: orch_types::Id, arg4: orch_access_contract::permissions::Permission) -> orch_api_base::middleware::requests::ApiResult<()>;
}

/// Where a resource lies, as the audit row of an act on it needs it: the org or node it is at home
/// in and the root org of that.
#[derive(Debug, Clone, Copy, Default)]
pub struct Home {
    /// The place its page follows; none for a resource of the system.
    pub node: Option<Id>,
    /// The root org of that place.
    pub org: Option<Id>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_824140da0a, __api_install_824140da0a, __api_callback_824140da0a;
    slot __ApiCallback824140da0a, __API_CALLBACK_824140DA0A, __api_set_824140da0a;
    base orch_api_base::routes::resources;

    async fn record<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_access_contract::audit::AuditAction, arg3: orch_types::Id, arg4: orch_api_base::routes::resources::Home, arg5: &'__api str, arg6: orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<()>;
}

#[path = "resources/agents.rs"]
pub mod agents;

#[path = "resources/creation_home.rs"]
pub mod creation_home;

#[path = "resources/environment_fleet.rs"]
pub mod environment_fleet;

#[path = "resources/environment_places.rs"]
pub mod environment_places;

#[path = "resources/environment_read.rs"]
pub mod environment_read;

#[path = "resources/grouping.rs"]
pub mod grouping;

#[path = "resources/host_delete.rs"]
pub mod host_delete;

#[path = "resources/hosts.rs"]
pub mod hosts;

#[path = "resources/impact.rs"]
pub mod impact;

#[path = "resources/library.rs"]
pub mod library;

#[path = "resources/machines.rs"]
pub mod machines;

#[path = "resources/mcp.rs"]
pub mod mcp;

#[path = "resources/mcp_readers.rs"]
pub mod mcp_readers;

#[path = "resources/mounts.rs"]
pub mod mounts;

#[path = "resources/page.rs"]
pub mod page;

#[path = "resources/projection.rs"]
pub mod projection;

#[path = "resources/provider_projection.rs"]
pub mod provider_projection;

#[path = "resources/provider_recovery.rs"]
pub mod provider_recovery;

#[path = "resources/providers.rs"]
pub mod providers;

#[path = "resources/sealing.rs"]
pub mod sealing;

#[path = "resources/skills.rs"]
pub mod skills;

#[path = "resources/templates.rs"]
pub mod templates;

#[path = "resources/validation.rs"]
pub mod validation;

#[path = "resources/view.rs"]
pub mod view;
