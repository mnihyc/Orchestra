//! Shells that are their opener's alone: sessions, the ticket that takes one to another screen, the choice of which to close at a person's limit, and closing by whoever manages the environment.
//!
//! A shell is opened in an environment by somebody who holds `terminal.manage` on the environment, which
//! whoever manages it holds without being named, when no gate that no permission lifts is shut and the
//! person is below their limit; a thread the request names says only beside which one the shell is opened
//! and gives nothing. At the limit the request names the shells of theirs they give up, and the
//! answer to one that does not is the list of their own to choose from. A shell is its opener's alone: a
//! ticket to attach to it is given to its opener and to nobody else, whatever anybody else holds, and
//! whoever manages the environment sees that it exists and may close it. What a closed shell last printed
//! is read by its opener, by whoever manages the environment and by whoever holds `audit.view` on the
//! thread it was opened from; the read of anybody but the opener is a line of the environment's history.
//! The agent's own terminal is watched with a ticket for `terminal.view` on the thread's environment, and
//! typed into with one for `terminal.manage` there, where the agent declares one. The sockets themselves
//! are in the realtime module.

orch_api_base::api_family_callback! {
    bind __api_bind_26765e8efb, __api_install_26765e8efb, __api_callback_26765e8efb;
    slot __ApiCallback26765e8efb, __API_CALLBACK_26765E8EFB, __api_set_26765e8efb;
    base orch_api_base::routes::terminals;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_09bdb1d5b2, __api_install_09bdb1d5b2, __api_callback_09bdb1d5b2;
    slot __ApiCallback09bdb1d5b2, __API_CALLBACK_09BDB1D5B2, __api_set_09bdb1d5b2;
    base orch_api_base::routes::terminals;

    async fn list_for<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: orch_api_contract::terminals::EnvironmentTerminalListQuery, arg5: Option<&'__api std::collections::HashMap<orch_types::Id, bool>>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::common::Page<orch_api_contract::terminals::TerminalSession>>;
}
