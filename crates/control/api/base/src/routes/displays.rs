//! Display requests share the same safe current rows with the unretained display feed.

pub use super::browsers::holds;
orch_api_base::api_family_callback! {
    bind __api_bind_0102d264ad, __api_install_0102d264ad, __api_callback_0102d264ad;
    slot __ApiCallback0102d264ad, __API_CALLBACK_0102D264AD, __api_set_0102d264ad;
    base orch_api_base::routes::displays;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_c12234bdba, __api_install_c12234bdba, __api_callback_c12234bdba;
    slot __ApiCallbackc12234bdba, __API_CALLBACK_C12234BDBA, __api_set_c12234bdba;
    base orch_api_base::routes::displays;

    async fn current<'__api>(arg0: &'__api orch_api_base::state::App, arg1: &'__api mut orch_db::Conn, arg2: Option<&'__api orch_authz::Principal>, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::displays::EnvironmentDisplays>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3e10c98a8c, __api_install_3e10c98a8c, __api_callback_3e10c98a8c;
    slot __ApiCallback3e10c98a8c, __API_CALLBACK_3E10C98A8C, __api_set_3e10c98a8c;
    base orch_api_base::routes::displays;

    async fn filter_list<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<orch_types::Json>;
}
