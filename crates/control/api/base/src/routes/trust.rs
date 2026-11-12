//! A workspace's own harness configuration on its environment's page: what each harness found and the answer given
//! for it, in the environment's view, and the answer given or changed by whoever manages the environment.
//!
//! A person answers, never an agent; the sessions opened after an answer carry it, the threads that waited go on,
//! and the question is asked again when the files change.

orch_api_base::api_family_callback! {
    bind __api_bind_edc9d609ea, __api_install_edc9d609ea, __api_callback_edc9d609ea;
    slot __ApiCallbackedc9d609ea, __API_CALLBACK_EDC9D609EA, __api_set_edc9d609ea;
    base orch_api_base::routes::trust;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_ec0f96e79d, __api_install_ec0f96e79d, __api_callback_ec0f96e79d;
    slot __ApiCallbackec0f96e79d, __API_CALLBACK_EC0F96E79D, __api_set_ec0f96e79d;
    base orch_api_base::routes::trust;

    async fn views_of<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::trust::ConfigTrust>>>;
}
