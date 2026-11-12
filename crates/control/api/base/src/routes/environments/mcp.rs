//! The MCP servers an environment enables, imports copied on their machine and servers written for this environment alone.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use crate::routes::resources::Home;

pub use crate::routes::resources::mcp as resources;

pub use crate::routes::resources::record;

pub use orch_access_contract::audit::AuditAction;

pub use orch_authz::Principal;

pub use orch_db::queries::resources::mcp as rows;

pub use orch_db::queries::resources::mcp::McpRow;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use orch_types::Json;

pub use serde_json::json;
orch_api_base::api_family_callback! {
    bind __api_bind_25bd166465, __api_install_25bd166465, __api_callback_25bd166465;
    slot __ApiCallback25bd166465, __API_CALLBACK_25BD166465, __api_set_25bd166465;
    base orch_api_base::routes::environments::mcp;

    async fn may_import<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6abcd777fa, __api_install_6abcd777fa, __api_callback_6abcd777fa;
    slot __ApiCallback6abcd777fa, __API_CALLBACK_6ABCD777FA, __api_set_6abcd777fa;
    base orch_api_base::routes::environments::mcp;

    async fn import_in<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: orch_api_contract::environment::EnvironmentMcpImportCreateBody, arg5: orch_types::Id, arg6: bool) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::McpServer>;
}

#[path = "mcp/create_sources.rs"]
pub mod create_sources;
