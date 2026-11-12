//! Sharing a chat, a document, a thread or a whole place with agents, by area, as a copy or a live share to read or to write; listing, taking back, and an agent's reach as a tree.
//!
//! What sharing means is the engine's (`orch_engine::sharing`): which entries a live share is, what a copy holds,
//! who may do either. The handlers here read a request, ask the engine, and turn the answer into the types of the
//! API with the names of everything it mentions, and a refusal into the problem a client acts on.
//!
//! **An agent's reach as a tree** is the tree its own tools list, read for a person: every entry the agent reaches
//! is there, and one the person may not see themself comes without its name, and without the names of the places
//! above it in its path (it is then written by its identifier), so that nothing is learnt from the tree that the
//! person could not learn from the place.

orch_api_base::api_family_callback! {
    bind __api_bind_b57b73724e, __api_install_b57b73724e, __api_callback_b57b73724e;
    slot __ApiCallbackb57b73724e, __API_CALLBACK_B57B73724E, __api_set_b57b73724e;
    base orch_api_base::routes::share;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_eb48af3518, __api_install_eb48af3518, __api_callback_eb48af3518;
    slot __ApiCallbackeb48af3518, __API_CALLBACK_EB48AF3518, __api_set_eb48af3518;
    base orch_api_base::routes::share;

    async fn failed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_engine::sharing::ShareError) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2f2b473303, __api_install_2f2b473303, __api_callback_2f2b473303;
    slot __ApiCallback2f2b473303, __API_CALLBACK_2F2B473303, __api_set_2f2b473303;
    base orch_api_base::routes::share;

    async fn failed_projected<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_engine::sharing::ShareError, arg2: bool) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ffc7d657fe, __api_install_ffc7d657fe, __api_callback_ffc7d657fe;
    slot __ApiCallbackffc7d657fe, __API_CALLBACK_FFC7D657FE, __api_set_ffc7d657fe;
    base orch_api_base::routes::share;

    async fn viewer_entries_bounded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_engine::sharing::Record, arg3: &'__api orch_api_base::routes::access::page::NameBounds) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::access::Entry>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_da018a6af3, __api_install_da018a6af3, __api_callback_da018a6af3;
    slot __ApiCallbackda018a6af3, __API_CALLBACK_DA018A6AF3, __api_set_da018a6af3;
    base orch_api_base::routes::share;

    fn downloads<'__api>(arg0: &'__api orch_engine::sharing::Record) -> bool;
}

#[path = "share/operation.rs"]
pub mod operation;
