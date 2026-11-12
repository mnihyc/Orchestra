//! Asking for access to things, reading where the request stands, and deciding it: one approval for each thing asked for, each sent to the nearest people who can give it.
//!
//! The rules are the access engine's and the transaction is the engine's: what is written here is
//! the translation. A request is refused whole when one of its things cannot be asked for, and
//! says which and why; it is made by anybody who is signed in, for a thing whose identifier they
//! know. Where an approval stands, and whether the caller may decide it, is told by [`views_of`],
//! which the list and the reading of approvals use for the approvals of this kind.

pub use orch_access_contract::decisions::ObjectRef;

pub use orch_api_contract::approvals::AccessRequest;

pub use orch_api_contract::approvals::ApprovalState;
/// How an approval of the kind `access` is shown to the one who reads it.
pub struct Shown {
    /// What it asks, with the names the reader may know.
    pub request: AccessRequest,
    /// The thing it is about, as the reader may know it.
    pub object: orch_api_contract::access::ObjectRef,
    /// Where it stands: a request that ran out reads as expired.
    pub state: ApprovalState,
    /// Whether the reader may decide it now.
    pub can_decide: bool,
    /// Whether it was sent to the reader: they are among the nearest who could give it.
    pub sent_to_them: bool,
}

orch_api_base::api_family_callback! {
    bind __api_bind_9eac981cd7, __api_install_9eac981cd7, __api_callback_9eac981cd7;
    slot __ApiCallback9eac981cd7, __API_CALLBACK_9EAC981CD7, __api_set_9eac981cd7;
    base orch_api_base::routes::access::requests;

    async fn views_of<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_engine::Engine, arg3: &'__api [orch_db::queries::approvals::ApprovalRow], arg4: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_base::routes::access::requests::Shown>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_eaa16a4cf5, __api_install_eaa16a4cf5, __api_callback_eaa16a4cf5;
    slot __ApiCallbackeaa16a4cf5, __API_CALLBACK_EAA16A4CF5, __api_set_eaa16a4cf5;
    base orch_api_base::routes::access::requests;

    async fn decide_row<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: orch_db::Tx, arg2: orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_api_contract::approvals::ApprovalDecision) -> orch_api_base::middleware::requests::ApiResult<orch_db::queries::approvals::ApprovalRow>;
}
