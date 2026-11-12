//! An environment's or a runtime's network as people see it: what its host's node reported, the restriction retained on it, and what its template asks for where nothing was reported yet.
//!
//! The host's node is the only one that says what is in force for a unit; until it has said so for a restricted
//! one, the network is shown as asked and not ready, never as ready. A restriction retained on an old open unit (a
//! change to restricted that did not complete) is shown beside the mode the unit has, and the unit stays stopped.
//! A directory on a person's machine has the machine's network.

orch_api_base::api_family_callback! {
    bind __api_bind_4c76ea5c7c, __api_install_4c76ea5c7c, __api_callback_4c76ea5c7c;
    slot __ApiCallback4c76ea5c7c, __API_CALLBACK_4C76EA5C7C, __api_set_4c76ea5c7c;
    base orch_api_base::routes::environments::network;

    fn state_of<'__api>(arg0: Option<&'__api orch_db::queries::network::RuntimeNetworkRow>, arg1: bool) -> orch_api_contract::environment::NetworkState;
}

orch_api_base::api_family_callback! {
    bind __api_bind_54063f987e, __api_install_54063f987e, __api_callback_54063f987e;
    slot __ApiCallback54063f987e, __API_CALLBACK_54063F987E, __api_set_54063f987e;
    base orch_api_base::routes::environments::network;

    async fn of_runtimes<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::environment::NetworkState>>;
}
