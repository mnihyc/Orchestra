//! Resource pages read the same batched library figures as inventory rows, after ordinary View checks.

orch_api_base::api_family_callback! {
    bind __api_bind_eead74b020, __api_install_eead74b020, __api_callback_eead74b020;
    slot __ApiCallbackeead74b020, __API_CALLBACK_EEAD74B020, __api_set_eead74b020;
    base orch_api_base::routes::resources::library;

    async fn readable<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_access_contract::decisions::ObjectRef]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashSet<orch_access_contract::decisions::ObjectRef>>;
}
