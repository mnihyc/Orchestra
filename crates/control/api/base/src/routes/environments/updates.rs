//! Update and recreate plans over HTTP, confirmed once and checked again before a runtime is held.

orch_api_base::api_family_callback! {
    bind __api_bind_c3cec1834a, __api_install_c3cec1834a, __api_callback_c3cec1834a;
    slot __ApiCallbackc3cec1834a, __API_CALLBACK_C3CEC1834A, __api_set_c3cec1834a;
    base orch_api_base::routes::environments::updates;

    fn refused(arg0: orch_engine::updates::UpdateError) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e77c594372, __api_install_e77c594372, __api_callback_e77c594372;
    slot __ApiCallbacke77c594372, __API_CALLBACK_E77C594372, __api_set_e77c594372;
    base orch_api_base::routes::environments::updates;

    async fn ensure_unheld<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_bf2b3071af, __api_install_bf2b3071af, __api_callback_bf2b3071af;
    slot __ApiCallbackbf2b3071af, __API_CALLBACK_BF2B3071AF, __api_set_bf2b3071af;
    base orch_api_base::routes::environments::updates;

    async fn keep<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_engine::updates::Draft, arg3: orch_types::Id, arg4: &'__api str, arg5: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3149c67600, __api_install_3149c67600, __api_callback_3149c67600;
    slot __ApiCallback3149c67600, __API_CALLBACK_3149C67600, __api_set_3149c67600;
    base orch_api_base::routes::environments::updates;

    async fn provider_plan<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_engine::updates::Draft) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::forks::ProviderUpdatePlan>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c44e070c1c, __api_install_c44e070c1c, __api_callback_c44e070c1c;
    slot __ApiCallbackc44e070c1c, __API_CALLBACK_C44E070C1C, __api_set_c44e070c1c;
    base orch_api_base::routes::environments::updates;

    async fn confirm_notifying(arg0: orch_api_base::registry::Ctx, arg1: orch_engine::updates::Target, arg2: orch_api_contract::forks::ForkCreateBody, arg3: bool) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::workspace::Job>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8e42e7dfad, __api_install_8e42e7dfad, __api_callback_8e42e7dfad;
    slot __ApiCallback8e42e7dfad, __API_CALLBACK_8E42E7DFAD, __api_set_8e42e7dfad;
    base orch_api_base::routes::environments::updates;

    async fn audience_bounded<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id], arg4: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::forks::UpdateAudience>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6ac5ae2c78, __api_install_6ac5ae2c78, __api_callback_6ac5ae2c78;
    slot __ApiCallback6ac5ae2c78, __API_CALLBACK_6AC5AE2C78, __api_set_6ac5ae2c78;
    base orch_api_base::routes::environments::updates;

    async fn tell_update<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api [orch_types::Id], arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0daadb08b6, __api_install_0daadb08b6, __api_callback_0daadb08b6;
    slot __ApiCallback0daadb08b6, __API_CALLBACK_0DAADB08B6, __api_set_0daadb08b6;
    base orch_api_base::routes::environments::updates;

    async fn keep_provider_batch<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_types::Id, arg3: &'__api [orch_engine::updates::Recipe], arg4: &'__api orch_authz::CapturedOperation) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d91e721e63, __api_install_d91e721e63, __api_callback_d91e721e63;
    slot __ApiCallbackd91e721e63, __API_CALLBACK_D91E721E63, __api_set_d91e721e63;
    base orch_api_base::routes::environments::updates;

    async fn batch_drafts<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: Option<orch_types::Id>, arg4: bool, arg5: &'__api orch_api_contract::forks::ForkCreateBody) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_engine::updates::Draft>>;
}
