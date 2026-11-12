//! Permanent audit rows projected with current content and restoration authority.

orch_api_base::api_family_callback! {
    bind __api_bind_5411811a96, __api_install_5411811a96, __api_callback_5411811a96;
    slot __ApiCallback5411811a96, __API_CALLBACK_5411811A96, __api_set_5411811a96;
    base orch_api_base::routes::trace;

    fn actions(arg0: Option<orch_access_contract::audit::AuditKind>) -> Option<Vec<String>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_4d6aa3f24b, __api_install_4d6aa3f24b, __api_callback_4d6aa3f24b;
    slot __ApiCallback4d6aa3f24b, __API_CALLBACK_4D6AA3F24B, __api_set_4d6aa3f24b;
    base orch_api_base::routes::trace;

    async fn restore_allowed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api str, arg3: orch_types::Id) -> bool;
}

orch_api_base::api_family_callback! {
    bind __api_bind_7aeb1c6360, __api_install_7aeb1c6360, __api_callback_7aeb1c6360;
    slot __ApiCallback7aeb1c6360, __API_CALLBACK_7AEB1C6360, __api_set_7aeb1c6360;
    base orch_api_base::routes::trace;

    async fn entries<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api [orch_db::queries::trace::Row]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::nodes::AuditEntry>>;
}
