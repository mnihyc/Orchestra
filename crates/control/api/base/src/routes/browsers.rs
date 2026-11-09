//! Browser requests and the whole current list shared with the unretained browser feed.

orch_api_base::api_family_callback! {
    bind __api_bind_ea12023e1e, __api_install_ea12023e1e, __api_callback_ea12023e1e;
    slot __ApiCallbackea12023e1e, __API_CALLBACK_EA12023E1E, __api_set_ea12023e1e;
    base orch_api_base::routes::browsers;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_0ebb32af85, __api_install_0ebb32af85, __api_callback_0ebb32af85;
    slot __ApiCallback0ebb32af85, __API_CALLBACK_0EBB32AF85, __api_set_0ebb32af85;
    base orch_api_base::routes::browsers;

    async fn holds<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_access_contract::permissions::Permission) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3f4b14d8fe, __api_install_3f4b14d8fe, __api_callback_3f4b14d8fe;
    slot __ApiCallback3f4b14d8fe, __API_CALLBACK_3F4B14D8FE, __api_set_3f4b14d8fe;
    base orch_api_base::routes::browsers;

    async fn current<'__api>(arg0: &'__api orch_api_base::state::App, arg1: &'__api mut orch_db::Conn, arg2: Option<&'__api orch_authz::Principal>, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::browsers::EnvironmentBrowsers>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_66bd6d39aa, __api_install_66bd6d39aa, __api_callback_66bd6d39aa;
    slot __ApiCallback66bd6d39aa, __API_CALLBACK_66BD6D39AA, __api_set_66bd6d39aa;
    base orch_api_base::routes::browsers;

    async fn filter_list<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<orch_types::Json>;
}
