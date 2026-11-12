//! Listings of environments and provider runtimes, recycle, and batch moves and updates.
//!
//! A batch plans each visible unit independently and queues only plans without blockers. The jobs
//! already hold their units while queued; the runner limits how many execute at a time. Recycling
//! uses the same eligibility checks in the preview and immediately before an ordinary stop.

orch_api_base::api_family_callback! {
    bind __api_bind_005d526ddc, __api_install_005d526ddc, __api_callback_005d526ddc;
    slot __ApiCallback005d526ddc, __API_CALLBACK_005D526DDC, __api_set_005d526ddc;
    base orch_api_base::routes::fleet;

    async fn recycle_ids<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api [orch_types::Id], arg2: &'__api orch_api_contract::environment::RecycleBody, arg3: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::environment::RecycleReply>;
}
