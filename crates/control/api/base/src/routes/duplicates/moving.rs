//! Native consequence acknowledgement shared by atomic moving families; it grants no authority.

orch_api_base::api_family_callback! {
    bind __api_bind_5ca59db85d, __api_install_5ca59db85d, __api_callback_5ca59db85d;
    slot __ApiCallback5ca59db85d, __API_CALLBACK_5CA59DB85D, __api_set_5ca59db85d;
    base orch_api_base::routes::duplicates::moving;

    fn plan<'__api>(arg0: orch_api_contract::duplicates::CopySubject, arg1: orch_api_contract::duplicates::DuplicateDestination, arg2: i64, arg3: orch_types::Id, arg4: &'__api orch_api_contract::duplicates::DuplicateBody, arg5: serde_json::Value, arg6: Vec<orch_api_contract::duplicates::MoveOutDeleteEffect>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::duplicates::MoveOutDeletePlan>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_394be8c8e0, __api_install_394be8c8e0, __api_callback_394be8c8e0;
    slot __ApiCallback394be8c8e0, __API_CALLBACK_394BE8C8E0, __api_set_394be8c8e0;
    base orch_api_base::routes::duplicates::moving;

    fn confirm<'__api>(arg0: &'__api orch_api_contract::duplicates::MoveOutDeletePlan, arg1: &'__api orch_api_contract::duplicates::MoveOutConfirmation) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9dfaded361, __api_install_9dfaded361, __api_callback_9dfaded361;
    slot __ApiCallback9dfaded361, __API_CALLBACK_9DFADED361, __api_set_9dfaded361;
    base orch_api_base::routes::duplicates::moving;

    fn stale() -> orch_api_base::middleware::requests::ApiError;
}
