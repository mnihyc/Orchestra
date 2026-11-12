//! One native composing-context check for all existing grant owners.

orch_api_base::api_family_callback! {
    bind __api_bind_559437e4dc, __api_install_559437e4dc, __api_callback_559437e4dc;
    slot __ApiCallback559437e4dc, __API_CALLBACK_559437E4DC, __api_set_559437e4dc;
    base orch_api_base::routes::access::link_origin;

    async fn check<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_access_contract::audit::LinkOrigin>) -> orch_api_base::middleware::requests::ApiResult<()>;
}
