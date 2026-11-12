//! Making an org inside an org, a project and a topic, and a team's root org, each with the Owners row of its maker and the audit row of its making.
//!
//! A node is made in one transaction: the node with its side row and its
//! chat, the Owners row that names its maker as its first owner, the page it
//! starts with (the defaults, or cut loose when its maker asks for a private
//! one), the audit row, and the stamp that tells whoever has the place open.
//! Who may make what is asked again inside the action transaction after
//! the body arrives; the handler also checks what only the request knows: the
//! tree's two limits, how deep orgs may lie in one another, and that the
//! names and short names are fit.

orch_api_base::api_family_callback! {
    bind __api_bind_80201844f3, __api_install_80201844f3, __api_callback_80201844f3;
    slot __ApiCallback80201844f3, __API_CALLBACK_80201844F3, __api_set_80201844f3;
    base orch_api_base::routes::nodes::make;

    fn may_make_org<'__api>(arg0: orch_api_contract::auth::OrgCreation, arg1: &'__api orch_api_base::middleware::authentication::Authed) -> bool;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b69b8a0c63, __api_install_b69b8a0c63, __api_callback_b69b8a0c63;
    slot __ApiCallbackb69b8a0c63, __API_CALLBACK_B69B8A0C63, __api_set_b69b8a0c63;
    base orch_api_base::routes::nodes::make;

    async fn topic_prefix<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_contract::nodes::TopicCreateBody) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::nodes::Placed>;
}
