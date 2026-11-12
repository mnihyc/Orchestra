//! Health, setup and public metadata.
//!
//! Health is answered here; so is what a client may know before anybody has
//! signed in, and the making of the first administrator while the system has
//! no user.

orch_api_base::api_family_callback! {
    bind __api_bind_2af41ef4f9, __api_install_2af41ef4f9, __api_callback_2af41ef4f9;
    slot __ApiCallback2af41ef4f9, __API_CALLBACK_2AF41EF4F9, __api_set_2af41ef4f9;
    base orch_api_base::routes::meta;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_ec9f465d00, __api_install_ec9f465d00, __api_callback_ec9f465d00;
    slot __ApiCallbackec9f465d00, __API_CALLBACK_EC9F465D00, __api_set_ec9f465d00;
    base orch_api_base::routes::meta;

    fn health_router() -> axum::Router<orch_api_base::state::App>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f9d963c9b2, __api_install_f9d963c9b2, __api_callback_f9d963c9b2;
    slot __ApiCallbackf9d963c9b2, __API_CALLBACK_F9D963C9B2, __api_set_f9d963c9b2;
    base orch_api_base::routes::meta;

    async fn on_connection<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: Option<&'__api orch_api_base::middleware::authentication::Authed>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::auth::Meta>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f02731996c, __api_install_f02731996c, __api_callback_f02731996c;
    slot __ApiCallbackf02731996c, __API_CALLBACK_F02731996C, __api_set_f02731996c;
    base orch_api_base::routes::meta;

    fn setup_settled() -> orch_api_base::middleware::requests::ApiError;
}
