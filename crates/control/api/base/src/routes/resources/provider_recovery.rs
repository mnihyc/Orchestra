//! A provider's existing re-create control reviews lost runtime provisioning without borrowing its owner's destination authority.

orch_api_base::api_family_callback! {
    bind __api_bind_da1c5574f6, __api_install_da1c5574f6, __api_callback_da1c5574f6;
    slot __ApiCallbackda1c5574f6, __API_CALLBACK_DA1C5574F6, __api_set_da1c5574f6;
    base orch_api_base::routes::resources::provider_recovery;

    fn recipient_subject<'__api>(arg0: &'__api orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<orch_access_contract::decisions::ObjectRef>;
}
