//! MCP preset resources, environment-owned servers and imports whose definitions stay on their machine.

orch_api_base::api_family_callback! {
    bind __api_bind_2d568dc9e3, __api_install_2d568dc9e3, __api_callback_2d568dc9e3;
    slot __ApiCallback2d568dc9e3, __API_CALLBACK_2D568DC9E3, __api_set_2d568dc9e3;
    base orch_api_base::routes::resources::mcp;

    async fn forget_from_machine<'__api>(arg0: &'__api orch_api_base::state::App, arg1: orch_types::Id, arg2: orch_types::Id, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b46eced664, __api_install_b46eced664, __api_callback_b46eced664;
    slot __ApiCallbackb46eced664, __API_CALLBACK_B46ECED664, __api_set_b46eced664;
    base orch_api_base::routes::resources::mcp;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_576be5d12f, __api_install_576be5d12f, __api_callback_576be5d12f;
    slot __ApiCallback576be5d12f, __API_CALLBACK_576BE5D12F, __api_set_576be5d12f;
    base orch_api_base::routes::resources::mcp;

    fn name<'__api>(arg0: &'__api str) -> orch_api_base::middleware::requests::ApiResult<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_87ebeaa552, __api_install_87ebeaa552, __api_callback_87ebeaa552;
    slot __ApiCallback87ebeaa552, __API_CALLBACK_87EBEAA552, __api_set_87ebeaa552;
    base orch_api_base::routes::resources::mcp;

    fn spec<'__api>(arg0: &'__api orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<orch_types::Json>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0269d3f504, __api_install_0269d3f504, __api_callback_0269d3f504;
    slot __ApiCallback0269d3f504, __API_CALLBACK_0269D3F504, __api_set_0269d3f504;
    base orch_api_base::routes::resources::mcp;

    async fn require_subject<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: Option<orch_types::Id>, arg4: bool) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_35059fd8e4, __api_install_35059fd8e4, __api_callback_35059fd8e4;
    slot __ApiCallback35059fd8e4, __API_CALLBACK_35059FD8E4, __api_set_35059fd8e4;
    base orch_api_base::routes::resources::mcp;

    async fn show<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::McpServer>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_690315d601, __api_install_690315d601, __api_callback_690315d601;
    slot __ApiCallback690315d601, __API_CALLBACK_690315D601, __api_set_690315d601;
    base orch_api_base::routes::resources::mcp;

    async fn put_secret<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_types::Id, arg3: &'__api str, arg4: &'__api str) -> orch_api_base::middleware::requests::ApiResult<()>;
}

#[path = "mcp/projection.rs"]
pub mod projection;
