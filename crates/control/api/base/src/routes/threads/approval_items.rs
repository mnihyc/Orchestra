//! Personalize harness request cards in a timeline batch, without a per-card HTTP read.

orch_api_base::api_family_callback! {
    bind __api_bind_f30f174188, __api_install_f30f174188, __api_callback_f30f174188;
    slot __ApiCallbackf30f174188, __API_CALLBACK_F30F174188, __api_set_f30f174188;
    base orch_api_base::routes::threads::approval_items;

    async fn project<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: &'__api mut [orch_api_contract::threads::Item], arg4: chrono::DateTime<chrono::Utc>, arg5: &'__api orch_engine::Engine) -> orch_api_base::middleware::requests::ApiResult<()>;
}
