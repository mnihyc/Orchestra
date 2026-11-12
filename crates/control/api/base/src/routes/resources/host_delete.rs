//! Reviewed host deletion runs independent recovery or removal before detaching native resources and revoking its machine.

orch_api_base::api_family_callback! {
    bind __api_bind_9dd54ad790, __api_install_9dd54ad790, __api_callback_9dd54ad790;
    slot __ApiCallback9dd54ad790, __API_CALLBACK_9DD54AD790, __api_set_9dd54ad790;
    base orch_api_base::routes::resources::host_delete;

    fn recipient_subject<'__api>(arg0: &'__api orch_types::Json) -> orch_api_base::middleware::requests::ApiResult<orch_access_contract::decisions::ObjectRef>;
}
