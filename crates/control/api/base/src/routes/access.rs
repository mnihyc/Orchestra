//! The permission catalogue, roles held in a place, permission pages with their owners and defaults pages with previews, the people behind a holder a page at a time, the one change that names owners and holders at a choice, asks who may name somebody from outside and marks an override, a thing's history, and access requests for everything missing at once.
//!
//! Every decision is the access engine's. A page is read and changed by its
//! functions, in the caller's transaction, so that the rows, the entries,
//! the audit rows, the line of history and the move of the authorization
//! epoch are made together or not at all. What is written here is the
//! translation to and from the wire (names for identifiers, the words of the
//! API for the words of the engine), the roles, which the engine only
//! reads, and the requests that ask about a person's access. The requests
//! that are the same for every kind of thing (`readers`, `previews`) are
//! answered for a node, a resource and a document alike, and a kind that has
//! no page is answered plainly as not there (`subject`); a request for access
//! is made and read in `requests`.

pub use crate::middleware::requests::ApiResult;

pub use orch_types::Id;
orch_api_base::api_family_callback! {
    bind __api_bind_8101ceef54, __api_install_8101ceef54, __api_callback_8101ceef54;
    slot __ApiCallback8101ceef54, __API_CALLBACK_8101CEEF54, __api_set_8101ceef54;
    base orch_api_base::routes::access;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_504712b347, __api_install_504712b347, __api_callback_504712b347;
    slot __ApiCallback504712b347, __API_CALLBACK_504712B347, __api_set_504712b347;
    base orch_api_base::routes::access;

    fn catalogue_reply() -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionCatalogueGetReply>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2c7309614f, __api_install_2c7309614f, __api_callback_2c7309614f;
    slot __ApiCallback2c7309614f, __API_CALLBACK_2C7309614F, __api_set_2c7309614f;
    base orch_api_base::routes::access;

    async fn roles_held<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_db::queries::access::RoleRow], arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::access::RoleHeld>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6c1917a46a, __api_install_6c1917a46a, __api_callback_6c1917a46a;
    slot __ApiCallback6c1917a46a, __API_CALLBACK_6C1917A46A, __api_set_6c1917a46a;
    base orch_api_base::routes::access;

    async fn admin_matters<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_da8675c2ca, __api_install_da8675c2ca, __api_callback_da8675c2ca;
    slot __ApiCallbackda8675c2ca, __API_CALLBACK_DA8675C2CA, __api_set_da8675c2ca;
    base orch_api_base::routes::access;

    async fn role_matters<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_api_contract::access::Role, arg4: &'__api orch_authz::giving::RoleStanding, arg5: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e93405a280, __api_install_e93405a280, __api_callback_e93405a280;
    slot __ApiCallbacke93405a280, __API_CALLBACK_E93405A280, __api_set_e93405a280;
    base orch_api_base::routes::access;

    async fn outsiders_matter<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_authz::Policy) -> orch_api_base::middleware::requests::ApiResult<()>;
}

#[path = "access/link_origin.rs"]
pub mod link_origin;

#[path = "access/operations.rs"]
pub mod operations;

#[path = "access/page.rs"]
pub mod page;

#[path = "access/page_operations.rs"]
pub mod page_operations;

#[path = "access/previews.rs"]
pub mod previews;

#[path = "access/requests.rs"]
pub mod requests;

#[path = "access/subject.rs"]
pub mod subject;
