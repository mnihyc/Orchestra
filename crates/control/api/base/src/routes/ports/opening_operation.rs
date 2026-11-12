//! Actual stored-opening governor, current returned resource sources and optional protected names.

orch_api_base::api_family_callback! {
    bind __api_bind_40d114e821, __api_install_40d114e821, __api_callback_40d114e821;
    slot __ApiCallback40d114e821, __API_CALLBACK_40D114E821, __api_set_40d114e821;
    base orch_api_base::routes::ports::opening_operation;

    async fn foreign_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::queries::ports::Opening) -> orch_api_base::middleware::requests::ApiResult<(Vec<orch_access_contract::key_scope::KeyResolvedTarget>, Vec<orch_access_contract::key_scope::KeyResolvedTarget>)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f02d1b6741, __api_install_f02d1b6741, __api_callback_f02d1b6741;
    slot __ApiCallbackf02d1b6741, __API_CALLBACK_F02D1B6741, __api_set_f02d1b6741;
    base orch_api_base::routes::ports::opening_operation;

    fn name_sources<'__api>(arg0: &'__api orch_db::queries::ports::Opening) -> orch_api_base::routes::access::page::NameSources;
}
