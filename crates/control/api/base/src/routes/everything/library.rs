//! Library views, privacy projections and recorded-use ordering share the inventory's single read.

orch_api_base::api_family_callback! {
    bind __api_bind_874c2d3b53, __api_install_874c2d3b53, __api_callback_874c2d3b53;
    slot __ApiCallback874c2d3b53, __API_CALLBACK_874C2D3B53, __api_set_874c2d3b53;
    base orch_api_base::routes::everything::library;

    fn select_relations<'__api>(arg0: &'__api mut [orch_engine::resource_facts::RawFacts]) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_3582a8c8ce, __api_install_3582a8c8ce, __api_callback_3582a8c8ce;
    slot __ApiCallback3582a8c8ce, __API_CALLBACK_3582A8C8CE, __api_set_3582a8c8ce;
    base orch_api_base::routes::everything::library;

    async fn threads<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id], arg2: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, i64>>;
}
