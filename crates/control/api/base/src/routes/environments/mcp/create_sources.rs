//! Native owned-server creation and shared readback of written and imported servers.

pub use super::*;

use std::collections::BTreeMap;
pub struct Creation {
    pub environment: Id,
    pub org: Id,
    pub name: String,
    pub description: String,
    pub spec: Json,
    pub secrets: BTreeMap<String, String>,
    pub enable: bool,
}

orch_api_base::api_family_callback! {
    bind __api_bind_a0c768943c, __api_install_a0c768943c, __api_callback_a0c768943c;
    slot __ApiCallbacka0c768943c, __API_CALLBACK_A0C768943C, __api_set_a0c768943c;
    base orch_api_base::routes::environments::mcp::create_sources;

    async fn require_create<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_db::queries::environments::hosting::EnvironmentState>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_be38dce02e, __api_install_be38dce02e, __api_callback_be38dce02e;
    slot __ApiCallbackbe38dce02e, __API_CALLBACK_BE38DCE02E, __api_set_be38dce02e;
    base orch_api_base::routes::environments::mcp::create_sources;

    async fn prepare_native<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_api_contract::environment::EnvironmentMcpServerCreateBody) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::mcp::create_sources::Creation>;
}

impl Creation {
    pub async fn require(&self, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
        let env = require_create(conn, principal, self.environment).await?;
        if env.org_id != self.org {
            return Err(ApiError::not_found());
        }
        Ok(())
    }
}

/// The original environment-owned writer, including its real enablement and native audit.
pub async fn insert_native<F>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Creation,
    before: F,
) -> ApiResult<McpRow>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    before()?;
    let row = McpRow {
        imported: false,
        id: orch_types::new_id(),
        name: value.name.clone(),
        description: value.description.clone(),
        org_id: Some(value.org),
        home_id: Some(value.org),
        owner_id: Some(principal.user()),
        machine_id: None,
        environment_id: Some(value.environment),
        spec: value.spec.clone(),
        enabled: true,
        revision: 1,
    };
    rows::insert(conn, &row, ctx.now).await?;
    for (name, value) in &value.secrets {
        resources::put_secret(ctx, conn, row.id, name, value).await?;
    }
    rows::enable(conn, value.environment, row.id, value.enable, ctx.now).await?;
    record(
        ctx,
        conn,
        AuditAction::McpCreated,
        row.id,
        Home {
            node: row.home_id,
            org: row.org_id,
        },
        "Added an environment MCP server",
        json!({"server":row.id,"environment":value.environment,"name":row.name}),
    )
    .await?;
    Ok(row)
}
