//! A thread's request to let more people reach a port, and a person's decision on its card.
//!
//! `POST /threads/{id}/port-requests` (and the agent's `open_port`, which shares it) asks for an
//! audience of one port of the thread's environment. Who may be let in is a capability of the
//! environment (`port_place`, `port_link`, `port_host`), so the request is decided as every
//! capability is: where the environment holds the audience's capability, or the policy at its home
//! allows it, the opening is made at once and keeps what admitted it, with no card; where the policy
//! denies it, or the template leaves it out, it is refused; otherwise it waits on a card, an
//! approval of kind `port` that whoever holds Ports Manage on the environment now may decide. The
//! same request asked again answers its opening and makes no second card, and a refused one is
//! refused again for an hour. The card admits the opening once, or admits it and grants the
//! audience's capability to the environment (so that later requests of that kind open without a
//! card), or refuses it. A link made for the thread is sealed for the thread's own reading; the
//! deciding person may keep it to themself instead. These are boundary requests: no harness approval
//! mode, reviewer, thread rule or Stop decides one.

orch_api_base::api_family_callback! {
    bind __api_bind_386184d107, __api_install_386184d107, __api_callback_386184d107;
    slot __ApiCallback386184d107, __API_CALLBACK_386184D107, __api_set_386184d107;
    base orch_api_base::routes::port_requests;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_6669de2a5f, __api_install_6669de2a5f, __api_callback_6669de2a5f;
    slot __ApiCallback6669de2a5f, __API_CALLBACK_6669DE2A5F, __api_set_6669de2a5f;
    base orch_api_base::routes::port_requests;

    async fn request_for_thread_bounded<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: orch_db::Tx, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: orch_api_contract::ports::PortOpeningRequest, arg5: Option<&'__api orch_types::Json>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::PortOpenReply>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e7861bd2fa, __api_install_e7861bd2fa, __api_callback_e7861bd2fa;
    slot __ApiCallbacke7861bd2fa, __API_CALLBACK_E7861BD2FA, __api_set_e7861bd2fa;
    base orch_api_base::routes::port_requests;

    async fn request_ref<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: &'__api orch_db::queries::environments::routes::EnvironmentFull) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::PortRequestRef>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_79a50b72b8, __api_install_79a50b72b8, __api_callback_79a50b72b8;
    slot __ApiCallback79a50b72b8, __API_CALLBACK_79A50B72B8, __api_set_79a50b72b8;
    base orch_api_base::routes::port_requests;

    async fn decide<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: orch_db::Tx, arg2: orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_api_contract::approvals::ApprovalDecision) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::approvals::ApprovalDecisionReply>;
}
