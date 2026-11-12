//! Account creation replay is proved through the native username/password path.

orch_api_base::api_family_callback! {
    bind __api_bind_2e1ba4f348, __api_install_2e1ba4f348, __api_callback_2e1ba4f348;
    slot __ApiCallback2e1ba4f348, __API_CALLBACK_2E1BA4F348, __api_set_2e1ba4f348;
    base orch_api_base::routes::auth::creation;

    fn setup_binding() -> orch_api_base::middleware::idempotency::ProofCreation<(), orch_api_contract::auth::SetupCreateBody, orch_api_contract::auth::SetupCreateReply>;
}
