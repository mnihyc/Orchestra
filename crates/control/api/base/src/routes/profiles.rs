//! Personal pages, visible pins, work by day and direct shares between people, with recorded inspection of otherwise closed pages.

orch_api_base::api_family_callback! {
    bind __api_bind_82605ffc6d, __api_install_82605ffc6d, __api_callback_82605ffc6d;
    slot __ApiCallback82605ffc6d, __API_CALLBACK_82605FFC6D, __api_set_82605ffc6d;
    base orch_api_base::routes::profiles;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_6e5f32e3ad, __api_install_6e5f32e3ad, __api_callback_6e5f32e3ad;
    slot __ApiCallback6e5f32e3ad, __API_CALLBACK_6E5F32E3AD, __api_set_6e5f32e3ad;
    base orch_api_base::routes::profiles;

    async fn admin_place<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<Option<orch_access_contract::roles::PlaceRef>>;
}
