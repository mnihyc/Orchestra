//! Once-only native observation, followed by the common selected-source preview.

orch_api_base::api_family_callback! {
    bind __api_bind_1280176ede, __api_install_1280176ede, __api_callback_1280176ede;
    slot __ApiCallback1280176ede, __API_CALLBACK_1280176EDE, __api_set_1280176ede;
    base orch_api_base::routes::threads::adopt::operation;

    fn complete_staged<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: orch_types::Id, arg4: &'__api orch_api_contract::nodes::ThreadAdoptBody) -> orch_api_base::registry::OperationFuture<'__api>;
}
