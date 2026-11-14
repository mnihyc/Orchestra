//! Whole-value schema refusals for resource forms, with field paths and no supplied values.

orch_api_base::api_family_callback! {
    bind __api_bind_d2a05682ae, __api_install_d2a05682ae, __api_callback_d2a05682ae;
    slot __ApiCallbackd2a05682ae, __API_CALLBACK_D2A05682AE, __api_set_d2a05682ae;
    base orch_api_base::routes::resources::validation;

    fn setting<'__api>(arg0: &'__api orch_engine::Engine, arg1: &'__api str, arg2: &'__api orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ef3cd14b55, __api_install_ef3cd14b55, __api_callback_ef3cd14b55;
    slot __ApiCallbackef3cd14b55, __API_CALLBACK_EF3CD14B55, __api_set_ef3cd14b55;
    base orch_api_base::routes::resources::validation;

    async fn named_things_exist<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api str, arg2: &'__api orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<()>;
}
