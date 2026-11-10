//! One person-projected metadata cut for HTTP, operation answers and ephemeral views.

pub use crate::middleware::inspection;

pub use orch_types::Hash;

pub use orch_types::Id;

pub use orch_types::Seq;

pub use orch_types::Time;
#[derive(Debug, Clone)]
pub struct Cut {
    pub node: Id,
    pub root: Id,
    pub hierarchy: Hash,
    pub membership: Seq,
    pub epoch: i64,
    pub until: Option<Time>,
    pub as_admin: bool,
}

pub struct Projection<T> {
    pub value: T,
    pub cut: Cut,
    pub inspections: Vec<inspection::Info>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_1e70f78bc0, __api_install_1e70f78bc0, __api_callback_1e70f78bc0;
    slot __ApiCallback1e70f78bc0, __API_CALLBACK_1E70F78BC0, __api_set_1e70f78bc0;
    base orch_api_base::routes::document_tree::metadata;

    async fn cut<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::document_tree::metadata::Cut>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_bae5735b23, __api_install_bae5735b23, __api_callback_bae5735b23;
    slot __ApiCallbackbae5735b23, __API_CALLBACK_BAE5735B23, __api_set_bae5735b23;
    base orch_api_base::routes::document_tree::metadata;

    async fn unchanged<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::document_tree::metadata::Cut, arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_838bfdc53f, __api_install_838bfdc53f, __api_callback_838bfdc53f;
    slot __ApiCallback838bfdc53f, __API_CALLBACK_838BFDC53F, __api_set_838bfdc53f;
    base orch_api_base::routes::document_tree::metadata;

    async fn page<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: Option<orch_types::Id>, arg4: orch_api_contract::document_tree::DocumentDirectory, arg5: &'__api orch_api_contract::document_tree::DocumentTreeListQuery) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::document_tree::metadata::Projection<orch_api_contract::document_tree::DocumentTreePage>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d13bbf4a7d, __api_install_d13bbf4a7d, __api_callback_d13bbf4a7d;
    slot __ApiCallbackd13bbf4a7d, __API_CALLBACK_D13BBF4A7D, __api_set_d13bbf4a7d;
    base orch_api_base::routes::document_tree::metadata;

    async fn entry<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_api_contract::document_tree::DocumentTreeItem, arg4: bool) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::document_tree::metadata::Projection<orch_api_contract::document_tree::DocumentTreeRead>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_917e2cb467, __api_install_917e2cb467, __api_callback_917e2cb467;
    slot __ApiCallback917e2cb467, __API_CALLBACK_917E2CB467, __api_set_917e2cb467;
    base orch_api_base::routes::document_tree::metadata;

    async fn location<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::documents::DocumentRow, arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::document_tree::DocumentLocation>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a77a8d0bbe, __api_install_a77a8d0bbe, __api_callback_a77a8d0bbe;
    slot __ApiCallbacka77a8d0bbe, __API_CALLBACK_A77A8D0BBE, __api_set_a77a8d0bbe;
    base orch_api_base::routes::document_tree::metadata;

    async fn location_with_name_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::documents::DocumentRow, arg3: orch_types::Time, arg4: &'__api std::collections::BTreeSet<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::document_tree::DocumentLocation>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e2a51ca39a, __api_install_e2a51ca39a, __api_callback_e2a51ca39a;
    slot __ApiCallbacke2a51ca39a, __API_CALLBACK_E2A51CA39A, __api_set_e2a51ca39a;
    base orch_api_base::routes::document_tree::metadata;

    async fn named_document_paths<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashSet<orch_types::Id>>;
}
