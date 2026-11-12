//! Credential bounds around the existing permission-page and ownership writers.
//! The native page engine remains the only grant, ownership and sharing evaluator.

orch_api_base::api_family_callback! {
    bind __api_bind_82ef4ec4cc, __api_install_82ef4ec4cc, __api_callback_82ef4ec4cc;
    slot __ApiCallback82ef4ec4cc, __API_CALLBACK_82EF4EC4CC, __api_set_82ef4ec4cc;
    base orch_api_base::routes::access::page_operations;

    async fn action<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::registry::Ctx, orch_db::Tx, orch_api_base::middleware::transaction_guard::Guard)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_fe020d0482, __api_install_fe020d0482, __api_callback_fe020d0482;
    slot __ApiCallbackfe020d0482, __API_CALLBACK_FE020D0482, __api_set_fe020d0482;
    base orch_api_base::routes::access::page_operations;

    fn acknowledged<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: i64) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPageWriteReceipt>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_cde21c68a9, __api_install_cde21c68a9, __api_callback_cde21c68a9;
    slot __ApiCallbackcde21c68a9, __API_CALLBACK_CDE21C68A9, __api_set_cde21c68a9;
    base orch_api_base::routes::access::page_operations;

    async fn reply<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_authz::Principal, arg2: orch_api_contract::access::PermissionPageWriteReceipt) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPageWriteResponse>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_fd33eacf87, __api_install_fd33eacf87, __api_callback_fd33eacf87;
    slot __ApiCallbackfd33eacf87, __API_CALLBACK_FD33EACF87, __api_set_fd33eacf87;
    base orch_api_base::routes::access::page_operations;

    async fn current<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal) -> orch_api_base::middleware::requests::ApiResult<Option<orch_api_contract::access::PermissionPage>>;
}
