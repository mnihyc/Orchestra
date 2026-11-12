//! The caller's Inbox: the list newest first, the badge's counters, what is marked read, what they chose about being notified, the browsers that take push, and the kinds of notifier this system has.
//!
//! The counters are two: how many notifications wait unread, and how many
//! approvals wait in the caller's own list of approvals. The second is counted
//! as that list is made (`routes/approvals`); it is the number of the Approvals
//! badge, and the same one the caller's feed carries.

use orch_types::Seq;
/// The counters of a person's Inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Notifications not read yet.
    pub unread: i64,
    /// Approvals that wait in the person's own list of approvals.
    pub approvals_pending: i64,
    /// The highest number their Inbox has given out.
    pub seq: Seq,
}

orch_api_base::api_family_callback! {
    bind __api_bind_66aa9c259f, __api_install_66aa9c259f, __api_callback_66aa9c259f;
    slot __ApiCallback66aa9c259f, __API_CALLBACK_66AA9C259F, __api_set_66aa9c259f;
    base orch_api_base::routes::notifications::inbox;

    async fn summary<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_engine::Engine, arg3: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::notifications::inbox::Summary>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b96d6186ae, __api_install_b96d6186ae, __api_callback_b96d6186ae;
    slot __ApiCallbackb96d6186ae, __API_CALLBACK_B96D6186AE, __api_set_b96d6186ae;
    base orch_api_base::routes::notifications::inbox;

    async fn summary_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_engine::Engine, arg3: Option<orch_types::Id>, arg4: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::notifications::NotificationSummaryGetReply>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_5ec55184a8, __api_install_5ec55184a8, __api_callback_5ec55184a8;
    slot __ApiCallback5ec55184a8, __API_CALLBACK_5EC55184A8, __api_set_5ec55184a8;
    base orch_api_base::routes::notifications::inbox;

    async fn outside_status<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_types::Id>, arg3: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::notifications::OutsideStatus>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6023449a2d, __api_install_6023449a2d, __api_callback_6023449a2d;
    slot __ApiCallback6023449a2d, __API_CALLBACK_6023449A2D, __api_set_6023449a2d;
    base orch_api_base::routes::notifications::inbox;

    async fn prefs_in<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::notifications::NotificationPrefs>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a4b40e5758, __api_install_a4b40e5758, __api_callback_a4b40e5758;
    slot __ApiCallbacka4b40e5758, __API_CALLBACK_A4B40E5758, __api_set_a4b40e5758;
    base orch_api_base::routes::notifications::inbox;

    fn notifier_catalogue<'__api>(arg0: &'__api str) -> Vec<orch_api_contract::notifications::Notifier>;
}
