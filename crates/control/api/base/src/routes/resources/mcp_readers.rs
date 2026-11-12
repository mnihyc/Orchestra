//! MCP presets listed with private permission-page destinations and read only with MCP View.

orch_api_base::api_family_callback! {
    bind __api_bind_92ce626e66, __api_install_92ce626e66, __api_callback_92ce626e66;
    slot __ApiCallback92ce626e66, __API_CALLBACK_92CE626E66, __api_set_92ce626e66;
    base orch_api_base::routes::resources::mcp_readers;

    async fn views<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::McpServer>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_712f4c3049, __api_install_712f4c3049, __api_callback_712f4c3049;
    slot __ApiCallback712f4c3049, __API_CALLBACK_712F4C3049, __api_set_712f4c3049;
    base orch_api_base::routes::resources::mcp_readers;

    async fn read_for<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::McpServer>;
}
