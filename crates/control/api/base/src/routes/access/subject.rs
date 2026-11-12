//! What a request that is the same for every kind of thing names by `{object}` and `{id}` of its path, and the checks that are the same for all of them.
//!
//! Nodes, documents and every resource kind have permission pages decided by
//! the same access engine, each by the machinery of its own side. A chat has no page of its
//! own (its people are read as the people of a chat, and what was shared
//! with agents is listed where agents are shared with), so a request for the
//! page of a chat, or of anything the path invents, is answered plainly as not
//! there and never as a fault of the server.

orch_api_base::api_family_callback! {
    bind __api_bind_18e0c5f57f, __api_install_18e0c5f57f, __api_callback_18e0c5f57f;
    slot __ApiCallback18e0c5f57f, __API_CALLBACK_18E0C5F57F, __api_set_18e0c5f57f;
    base orch_api_base::routes::access::subject;

    fn kind_of_word<'__api>(arg0: &'__api str) -> Option<orch_access_contract::permissions::ObjectKind>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6ea4d24b43, __api_install_6ea4d24b43, __api_callback_6ea4d24b43;
    slot __ApiCallback6ea4d24b43, __API_CALLBACK_6EA4D24B43, __api_set_6ea4d24b43;
    base orch_api_base::routes::access::subject;

    async fn policy_at_place<'__api>(arg0: &'__api mut orch_db::Conn, arg1: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<orch_authz::Policy>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8dfafb0b48, __api_install_8dfafb0b48, __api_callback_8dfafb0b48;
    slot __ApiCallback8dfafb0b48, __API_CALLBACK_8DFAFB0B48, __api_set_8dfafb0b48;
    base orch_api_base::routes::access::subject;

    async fn resource_policy<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_authz::things::Thing, arg2: &'__api orch_authz::engine::Site) -> orch_api_base::middleware::requests::ApiResult<orch_authz::usable::ResourcePolicy>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_873b95e8e2, __api_install_873b95e8e2, __api_callback_873b95e8e2;
    slot __ApiCallback873b95e8e2, __API_CALLBACK_873B95E8E2, __api_set_873b95e8e2;
    base orch_api_base::routes::access::subject;

    async fn require_view<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_authz::things::Thing, arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a0c3c93238, __api_install_a0c3c93238, __api_callback_a0c3c93238;
    slot __ApiCallbacka0c3c93238, __API_CALLBACK_A0C3C93238, __api_set_a0c3c93238;
    base orch_api_base::routes::access::subject;

    async fn require_page<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_authz::things::Thing, arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<()>;
}
