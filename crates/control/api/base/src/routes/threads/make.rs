//! Making a thread in a topic: its binding to a provider with model and effort, an agent profile and an environment, checked for the caller by the access engine with everything missing in the refusal; the page it starts with; and what is said to it at once.
//!
//! What the request leaves out is taken from the settings of the topic: the
//! provider and the agent profile the place defaults to, the model and the
//! effort, and the environment the topic's threads share. A default the
//! caller may not use is not used; a thread made without any binding at all
//! begins blocked as unbound, and waits for one. Putting the thread on what
//! it runs on is asked of the access engine for the caller and not again: the
//! engine's own function makes the thread, with its first owner, its chat and
//! the page it starts with, in one transaction.

/// The name a thread has until it is renamed, and its title is kept by
/// automation.
pub const NEW_THREAD: &str = "New thread";

orch_api_base::api_family_callback! {
    bind __api_bind_5ad940dfb4, __api_install_5ad940dfb4, __api_callback_5ad940dfb4;
    slot __ApiCallback5ad940dfb4, __API_CALLBACK_5AD940DFB4, __api_set_5ad940dfb4;
    base orch_api_base::routes::threads::make;

    fn page_of<'__api>(arg0: Option<&'__api orch_api_contract::access::ThreadAccess>) -> orch_api_base::middleware::requests::ApiResult<Option<orch_access_contract::pages::PageChange>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8a3eb8d954, __api_install_8a3eb8d954, __api_callback_8a3eb8d954;
    slot __ApiCallback8a3eb8d954, __API_CALLBACK_8A3EB8D954, __api_set_8a3eb8d954;
    base orch_api_base::routes::threads::make;

    async fn scheduled_choices<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: &'__api mut orch_db::Conn, arg2: orch_types::Id, arg3: orch_types::Id, arg4: &'__api orch_api_contract::nodes::CreateThread, arg5: &'__api str) -> orch_api_base::middleware::requests::ApiResult<(orch_engine::binding::NewThread, orch_engine::binding::Birth)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_db6ac243da, __api_install_db6ac243da, __api_callback_db6ac243da;
    slot __ApiCallbackdb6ac243da, __API_CALLBACK_DB6AC243DA, __api_set_db6ac243da;
    base orch_api_base::routes::threads::make;

    async fn create_in<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_api_contract::nodes::CreateThread) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::threads::Thread>;
}
