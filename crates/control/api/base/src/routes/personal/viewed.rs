//! A successful authorized page read carries its visit; drawing a page sends no extra mutation.

orch_api_base::api_family_callback! {
    bind __api_bind_d8907363a0, __api_install_d8907363a0, __api_callback_d8907363a0;
    slot __ApiCallbackd8907363a0, __API_CALLBACK_D8907363A0, __api_set_d8907363a0;
    base orch_api_base::routes::personal::viewed;

    async fn record<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<()>;
}
