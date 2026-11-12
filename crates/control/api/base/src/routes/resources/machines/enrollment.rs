//! Issuer-scoped enrollment progress, reusing the native resource projection and its current rights.

orch_api_base::api_family_callback! {
    bind __api_bind_8fafa279ab, __api_install_8fafa279ab, __api_callback_8fafa279ab;
    slot __ApiCallback8fafa279ab, __API_CALLBACK_8FAFA279AB, __api_set_8fafa279ab;
    base orch_api_base::routes::resources::machines::enrollment;

    async fn show<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api str) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::EnrollmentProgress>;
}
