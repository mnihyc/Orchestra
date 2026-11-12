//! Native personal row selection, before any personal response DTO is rendered.

orch_api_base::api_family_callback! {
    bind __api_bind_a2018a5d59, __api_install_a2018a5d59, __api_callback_a2018a5d59;
    slot __ApiCallbacka2018a5d59, __API_CALLBACK_A2018A5D59, __api_set_a2018a5d59;
    base orch_api_base::routes::personal::native;

    fn source_usage<'__api>(arg0: &'__api orch_api_contract::operation_sources::Declaration<orch_api_contract::personal_sources::Source>, arg1: orch_api_contract::personal_sources::Source) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::operations::FacetUsage>;
}
