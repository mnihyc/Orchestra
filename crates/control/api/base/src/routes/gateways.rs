//! Effective gateway metadata and observed carrier facts use the ordinary operation guards.

orch_api_base::api_family_callback! {
    bind __api_bind_c61b72f407, __api_install_c61b72f407, __api_callback_c61b72f407;
    slot __ApiCallbackc61b72f407, __API_CALLBACK_C61B72F407, __api_set_c61b72f407;
    base orch_api_base::routes::gateways;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_d8b3eca981, __api_install_d8b3eca981, __api_callback_d8b3eca981;
    slot __ApiCallbackd8b3eca981, __API_CALLBACK_D8B3ECA981, __api_set_d8b3eca981;
    base orch_api_base::routes::gateways;

    async fn thread_in<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::gateways::ThreadTunnelGatewayStatus>;
}
