//! Exact share operands, bounded by the shared operation gate and consumed by the ordinary writer.

orch_api_base::api_family_callback! {
    bind __api_bind_d61a4f1c27, __api_install_d61a4f1c27, __api_callback_d61a4f1c27;
    slot __ApiCallbackd61a4f1c27, __API_CALLBACK_D61A4F1C27, __api_set_d61a4f1c27;
    base orch_api_base::routes::share::operation;

    async fn prepare_agents_trial<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: &'__api orch_api_contract::share::ShareWithAgents, arg4: orch_api_base::key_operations::admission::Prepared) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::key_operations::admission::Prepared, Result<orch_engine::sharing::PreparedAgentShare, orch_api_base::middleware::requests::ApiError>)>;
}
