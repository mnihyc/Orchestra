//! Fork and switch plans with their options, and execution, single and batch; side threads started and listed.
//!
//! A fork or a switch is planned first and carried out by naming the plan: the engine works the plan out
//! ([`orch_engine::fork_plan`]), and what only this layer knows is added before it is kept: the names inside
//! what the caller lacks, and the page the result would have. Carrying a plan out makes a job and, for a
//! switch, holds the thread in the same transaction; a batch makes one job for each plan without blockers.
//! A side thread is the fork with nothing to choose and is made by one request; every request on it, by
//! its own identifier, is asked of its parent.

orch_api_base::api_family_callback! {
    bind __api_bind_e570164848, __api_install_e570164848, __api_callback_e570164848;
    slot __ApiCallbacke570164848, __API_CALLBACK_E570164848, __api_set_e570164848;
    base orch_api_base::routes::forks;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_020ffd9110, __api_install_020ffd9110, __api_callback_020ffd9110;
    slot __ApiCallback020ffd9110, __API_CALLBACK_020FFD9110, __api_set_020ffd9110;
    base orch_api_base::routes::forks;

    async fn read_batch_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::forks::ForkBatch>;
}
