//! What a thread may be put on and putting it there: the providers with their models and the profiles that run on them, in every arrangement, with the reason one is not available or not permitted; the environments the caller manages and what a thread would get there; the defaults; and the binding and the environment of a thread that has no history yet.
//!
//! Which provider, which profile and which arrangement a thread may be bound to is the rule of the
//! engine ([`orch_engine::binding`]) applied to the facts of the database; who may use a provider,
//! a profile or an environment is the access engine's. This module puts the two side by side for
//! the dialog that binds a thread: every provider the caller can see, whether they may use it and
//! if not why, and for each profile that could run on it every arrangement with whose decision it
//! is when one is not allowed. A thread that has history is not bound again here: it is switched,
//! and the request says so.

orch_api_base::api_family_callback! {
    bind __api_bind_9aa49dd569, __api_install_9aa49dd569, __api_callback_9aa49dd569;
    slot __ApiCallback9aa49dd569, __API_CALLBACK_9AA49DD569, __api_set_9aa49dd569;
    base orch_api_base::routes::threads::binding;

    async fn ask_models<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::ask::AskModelOption>>;
}
