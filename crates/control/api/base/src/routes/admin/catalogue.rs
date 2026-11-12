//! The data a system admin keeps for the whole system: the definitions of capabilities, the builds of harnesses that hosts may fetch, and the agent profiles that have no home.
//!
//! A capability definition is a row like any other: the ones that ship are edited, more are added,
//! and a custom one is removed once no environment holds it. The builds of a harness come from two
//! places that are told apart by `source`: the list the release wrote, which is read from the
//! manifest in its payload when the release has one, and the builds an admin added, which win where
//! both name the same build. A profile of the system belongs to no org; its page starts with nobody
//! on it, and system admins hold every permission on it.

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use crate::routes::resources::Home;

use crate::routes::resources::record as record_resource;

pub use orch_access_contract::audit::AuditAction;

pub use orch_db::queries::resources::management;

pub use orch_db::Conn;

pub use orch_types::Json;

pub use serde_json::json;
orch_api_base::api_family_callback! {
    bind __api_bind_3f8389ed5c, __api_install_3f8389ed5c, __api_callback_3f8389ed5c;
    slot __ApiCallback3f8389ed5c, __API_CALLBACK_3F8389ED5C, __api_set_3f8389ed5c;
    base orch_api_base::routes::admin::catalogue;

    async fn builds_listed<'__api>(arg0: &'__api std::path::Path, arg1: &'__api mut orch_db::Conn) -> Result<Vec<orch_api_contract::admin::HarnessBuild>, orch_db::DbError>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9718450093, __api_install_9718450093, __api_callback_9718450093;
    slot __ApiCallback9718450093, __API_CALLBACK_9718450093, __api_set_9718450093;
    base orch_api_base::routes::admin::catalogue;

    async fn target_programs<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api [orch_engine::hosting::ToolBuild]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::admin::RunningHarnessBuild>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_891ba98c5e, __api_install_891ba98c5e, __api_callback_891ba98c5e;
    slot __ApiCallback891ba98c5e, __API_CALLBACK_891BA98C5E, __api_set_891ba98c5e;
    base orch_api_base::routes::admin::catalogue;

    async fn reported_program<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<Option<orch_api_contract::admin::RunningHarnessBuild>>;
}

/// Validated native system-profile operands; callers supply their own actual operation authority.
pub struct GlobalProfile {
    pub id: orch_types::Id,
    pub name: String,
    pub harness: String,
    pub profile: Json,
}

impl GlobalProfile {
    pub fn id(&self) -> orch_types::Id {
        self.id
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_2402bcfdbf, __api_install_2402bcfdbf, __api_callback_2402bcfdbf;
    slot __ApiCallback2402bcfdbf, __API_CALLBACK_2402BCFDBF, __api_set_2402bcfdbf;
    base orch_api_base::routes::admin::catalogue;

    fn prepare_global_profile(arg0: orch_api_contract::resources::OrgAgentCreateBody) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::admin::catalogue::GlobalProfile>;
}

/// The original native system insert and audit, shared by creation and duplication.
pub async fn insert_global_profile<F>(
    ctx: &Ctx,
    conn: &mut Conn,
    value: &GlobalProfile,
    before: F,
) -> ApiResult<orch_types::Id>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    before()?;
    management::insert_system_agent(
        conn,
        value.id,
        &value.name,
        &value.harness,
        &value.profile,
        ctx.now,
    )
    .await?;
    let who = &ctx.user()?.user.display_name;
    record_resource(
        ctx,
        conn,
        AuditAction::AgentCreated,
        value.id,
        Home::default(),
        &format!("{who} added the agent profile {} to the system", value.name),
        json!({"agent":value.id,"name":value.name,"harness_kind":value.harness,"system":true}),
    )
    .await?;
    Ok(value.id)
}
