//! Workspace move plans and confirmations, by data ownership and declared machine lifecycle.

orch_api_base::api_family_callback! {
    bind __api_bind_4d665163fa, __api_install_4d665163fa, __api_callback_4d665163fa;
    slot __ApiCallback4d665163fa, __API_CALLBACK_4D665163FA, __api_set_4d665163fa;
    base orch_api_base::routes::environments::moves;

    async fn shown_machine<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_engine::jobs::machine_move::Draft) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::forks::EnvironmentMovePlan>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8e01278771, __api_install_8e01278771, __api_callback_8e01278771;
    slot __ApiCallback8e01278771, __API_CALLBACK_8E01278771, __api_set_8e01278771;
    base orch_api_base::routes::environments::moves;

    async fn shown<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_engine::jobs::environment_move::MoveDraft) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::forks::EnvironmentMovePlan>;
}
