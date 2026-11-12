//! Same native ticket bindings at initial admission and current delivery, from stored Environment.

orch_api_base::api_family_callback! {
    bind __api_bind_caa4e5bd2d, __api_install_caa4e5bd2d, __api_callback_caa4e5bd2d;
    slot __ApiCallbackcaa4e5bd2d, __API_CALLBACK_CAA4E5BD2D, __api_set_caa4e5bd2d;
    base orch_api_base::routes::interactive_tickets;

    async fn core<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api str, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_engine::operation_scope::Prepared>;
}
