//! Current thread viewers read an existing native session's effective commands with a fresh return guard.

orch_api_base::api_family_callback! {
    bind __api_bind_3e8b0c222d, __api_install_3e8b0c222d, __api_callback_3e8b0c222d;
    slot __ApiCallback3e8b0c222d, __API_CALLBACK_3E8B0C222D, __api_set_3e8b0c222d;
    base orch_api_base::routes::threads::native;

    async fn refresh_for_input<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<()>;
}
