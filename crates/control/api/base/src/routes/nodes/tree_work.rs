//! Compact waiting and blocked explanations for the same authorized tree over HTTP and the live feed.

orch_api_base::api_family_callback! {
    bind __api_bind_7100f6658b, __api_install_7100f6658b, __api_callback_7100f6658b;
    slot __ApiCallback7100f6658b, __API_CALLBACK_7100F6658B, __api_set_7100f6658b;
    base orch_api_base::routes::nodes::tree_work;

    async fn waiting<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: chrono::DateTime<chrono::Utc>, arg4: &'__api orch_engine::Engine) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::TreeWaiting>>;
}
