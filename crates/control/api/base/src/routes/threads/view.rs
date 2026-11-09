//! A thread as the API shows it to one caller: what the engine knows of it, what it runs on and what that comes to, whether the caller may give it input, and what they may do with it.
//!
//! The thread is a node, so what every node has (its place, its owner, what
//! the caller holds on it) is made with the other nodes, in
//! [`crate::routes::nodes::view`], and handed here as the base. What is added
//! is the thread's own: the engine's facts, the binding with what it comes
//! to, where the agent's notes are kept, and three answers that are the access
//! engine's and nobody else's: whether the caller may give the thread input
//! now, with everything they lack when not, what they hold on the environment
//! it runs in (which the views beside the conversation ask, since a thread's
//! own page holds none of it), and whether they manage that environment.

orch_api_base::api_family_callback! {
    bind __api_bind_a3c1635f6e, __api_install_a3c1635f6e, __api_callback_a3c1635f6e;
    slot __ApiCallbacka3c1635f6e, __API_CALLBACK_A3C1635F6E, __api_set_a3c1635f6e;
    base orch_api_base::routes::threads::view;

    async fn thread_with_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_api_contract::nodes::NodeBase<orch_api_contract::threads::ThreadKind>, arg3: chrono::DateTime<chrono::Utc>, arg4: Option<&'__api std::collections::BTreeSet<orch_types::Id>>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::threads::Thread>;
}
