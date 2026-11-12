//! Approval listing and decisions.
//!
//! What a harness asks before it acts waits here as an approval, and whoever
//! manages its thread answers it: the first answer to reach the database
//! wins, and the harness is told after the commit. A person's own list holds
//! what they can decide now, what they asked for, or what is attached to a
//! node they may view. Who may decide is the access engine's to say, asked of
//! it for the thread of each approval, the same way for one as for a page of
//! them. A request for a capability is decided on the machine's side, and a
//! request for access by whoever could change the page of what it asks for
//! (`routes/access/requests`); both are listed and read here with the rest.

pub use operation::Sources as CardSources;

pub use operation::bind_card_sources;

pub use operation::bind_prepared_card_sources;

pub use operation::card_read_caller;

pub use operation::card_sources;

pub use operation::governor as card_governor;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use orch_api_contract::approvals::Approval;

pub use orch_api_contract::approvals::ApprovalDecisionReply;

pub use orch_api_contract::approvals::ApprovalRequest;

pub use orch_api_contract::approvals::ApprovalRequester;

pub use orch_authz::nodes;

pub use orch_types::Id;
orch_api_base::api_family_callback! {
    bind __api_bind_4c6f191e70, __api_install_4c6f191e70, __api_callback_4c6f191e70;
    slot __ApiCallback4c6f191e70, __API_CALLBACK_4C6F191E70, __api_set_4c6f191e70;
    base orch_api_base::routes::approvals;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_73d0126e74, __api_install_73d0126e74, __api_callback_73d0126e74;
    slot __ApiCallback73d0126e74, __API_CALLBACK_73D0126E74, __api_set_73d0126e74;
    base orch_api_base::routes::approvals;

    async fn views<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_db::queries::approvals::ApprovalRow], arg3: chrono::DateTime<chrono::Utc>, arg4: &'__api orch_engine::Engine) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::approvals::Approval>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8e136e74a4, __api_install_8e136e74a4, __api_callback_8e136e74a4;
    slot __ApiCallback8e136e74a4, __API_CALLBACK_8E136E74A4, __api_set_8e136e74a4;
    base orch_api_base::routes::approvals;

    async fn waiting_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: chrono::DateTime<chrono::Utc>, arg3: &'__api orch_engine::Engine) -> orch_api_base::middleware::requests::ApiResult<i64>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8fa9886651, __api_install_8fa9886651, __api_callback_8fa9886651;
    slot __ApiCallback8fa9886651, __API_CALLBACK_8FA9886651, __api_set_8fa9886651;
    base orch_api_base::routes::approvals;

    async fn already_decided<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiError;
}

#[path = "approvals/operation.rs"]
pub mod operation;
