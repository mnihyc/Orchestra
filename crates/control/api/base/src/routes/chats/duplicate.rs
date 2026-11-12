//! A source-qualified group copy uses the native opening, naming, retained bytes and forwarding writers.

orch_api_base::api_family_callback! {
    bind __api_bind_fce0a68441, __api_install_fce0a68441, __api_callback_fce0a68441;
    slot __ApiCallbackfce0a68441, __API_CALLBACK_FCE0A68441, __api_set_fce0a68441;
    base orch_api_base::routes::chats::duplicate;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_b088dc274d, __api_install_b088dc274d, __api_callback_b088dc274d;
    slot __ApiCallbackb088dc274d, __API_CALLBACK_B088DC274D, __api_set_b088dc274d;
    base orch_api_base::routes::chats::duplicate;

    async fn copy_destination<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: orch_types::Id, arg5: orch_types::Id, arg6: &'__api str) -> orch_api_base::middleware::requests::ApiResult<orch_chat::chats::PreparedOpen>;
}
