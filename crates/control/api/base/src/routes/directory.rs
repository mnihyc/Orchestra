//! Org groups, roles, tags and service identities, each kept by one org at any level.
//!
//! An org keeps its own people: the members whose domain is it or an org
//! inside it, the groups it makes, and the service identities it creates for
//! automation. A member can be suspended, which closes the org and
//! everything inside it to them without removing anything of theirs. Tags
//! are the labels that topics carry, defined by an org or a project. Every
//! change here is decided by the access engine on the org that keeps the
//! thing, and writes its audit row in the same transaction.

orch_api_base::api_family_callback! {
    bind __api_bind_0502b165d2, __api_install_0502b165d2, __api_callback_0502b165d2;
    slot __ApiCallback0502b165d2, __API_CALLBACK_0502B165D2, __api_set_0502b165d2;
    base orch_api_base::routes::directory;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_bb09bd2387, __api_install_bb09bd2387, __api_callback_bb09bd2387;
    slot __ApiCallbackbb09bd2387, __API_CALLBACK_BB09BD2387, __api_set_bb09bd2387;
    base orch_api_base::routes::directory;

    async fn groups_named<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Vec<orch_db::queries::access::GroupFull>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::directory::Group>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_43f6a1890c, __api_install_43f6a1890c, __api_callback_43f6a1890c;
    slot __ApiCallback43f6a1890c, __API_CALLBACK_43F6A1890C, __api_set_43f6a1890c;
    base orch_api_base::routes::directory;

    async fn groups_named_bounded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Vec<orch_db::queries::access::GroupFull>, arg3: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::directory::Group>>;
}
