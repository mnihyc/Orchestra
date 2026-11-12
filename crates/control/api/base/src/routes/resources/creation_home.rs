//! Existing native resource-creation homes and ordinary membership/settings/system authority.

orch_api_base::api_family_callback! {
    bind __api_bind_4796f73651, __api_install_4796f73651, __api_callback_4796f73651;
    slot __ApiCallback4796f73651, __API_CALLBACK_4796F73651, __api_set_4796f73651;
    base orch_api_base::routes::resources::creation_home;

    async fn lock<'__api>(arg0: &'__api mut orch_db::Conn, arg1: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<()>;
}
