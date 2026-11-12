//! What a reader of usage is shown, by the scope they ask about: their own use, a place and everything below it, or what ran on a provider; how deep their access goes there, and which dimensions and filters that depth gives.
//!
//! The access engine decides the depth ([`orch_authz::caps::depth_at`]); this
//! module only turns the decision into what a request may ask. A dimension or a
//! filter the depth does not give is refused with `403 authz.denied`, naming
//! `usage.view`: a thread at the depth of a summary, and a person beyond oneself
//! at the depth of one's own use.

pub use orch_api_contract::usage::UsageDepth;

pub use orch_api_contract::usage::UsageEnvironmentKind;

pub use orch_db::usage::rollup::Filter;

pub use orch_types::Id;
/// What a request asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every counted use, under the existing actual-system administration rule.
    System,
    /// What the caller used themself, anywhere.
    Me,
    /// A place and everything below it.
    Node(Id),
    /// What ran on a provider, of whatever org.
    Provider(Id),
}

orch_api_base::api_family_callback! {
    bind __api_bind_3c5645224e, __api_install_3c5645224e, __api_callback_3c5645224e;
    slot __ApiCallback3c5645224e, __API_CALLBACK_3C5645224E, __api_set_3c5645224e;
    base orch_api_base::routes::usage::scope;

    fn parse<'__api>(arg0: &'__api str) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::usage::scope::Scope>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_5f74f1b8b7, __api_install_5f74f1b8b7, __api_callback_5f74f1b8b7;
    slot __ApiCallback5f74f1b8b7, __API_CALLBACK_5F74F1B8B7, __api_set_5f74f1b8b7;
    base orch_api_base::routes::usage::scope;

    fn parse_day<'__api>(arg0: &'__api str, arg1: &'__api str) -> orch_api_base::middleware::requests::ApiResult<chrono::NaiveDate>;
}

/// A scope the caller was let into, and how deep.
#[derive(Debug, Clone)]
pub struct Opened {
    /// What was asked about, as the answer names it.
    pub scope: String,
    /// How deep the caller is shown.
    pub depth: UsageDepth,
    /// What the scope itself limits the rows to.
    pub base: Filter,
    /// Who asks.
    pub caller: Id,
    /// The place asked about, where it is one.
    pub node: Option<Id>,
    /// Whether the answer carries the caller's own part (for a place or a provider).
    pub has_own_part: bool,
}

/// What a request limits the rows to beyond the scope.
#[derive(Debug, Clone, Default)]
pub struct Wanted {
    /// Only the use of this person.
    pub user_id: Option<Id>,
    /// Only this model.
    pub model: Option<String>,
    /// Only what ran on this provider.
    pub provider_id: Option<Id>,
    /// Only this harness kind.
    pub harness_kind: Option<String>,
    /// Only this kind of environment.
    pub environment_kind: Option<UsageEnvironmentKind>,
    /// Only this project.
    pub project_id: Option<Id>,
    /// Only this topic.
    pub topic_id: Option<Id>,
    /// Only this thread.
    pub thread_id: Option<Id>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_2ce58f8e2a, __api_install_2ce58f8e2a, __api_callback_2ce58f8e2a;
    slot __ApiCallback2ce58f8e2a, __API_CALLBACK_2CE58F8E2A, __api_set_2ce58f8e2a;
    base orch_api_base::routes::usage::scope;

    async fn open<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_api_base::routes::usage::scope::Scope) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::usage::scope::Opened>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d76bd083b9, __api_install_d76bd083b9, __api_callback_d76bd083b9;
    slot __ApiCallbackd76bd083b9, __API_CALLBACK_D76BD083B9, __api_set_d76bd083b9;
    base orch_api_base::routes::usage::scope;

    async fn require_dimension<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::usage::scope::Opened, arg3: orch_api_contract::usage::UsageDimension, arg4: &'__api str) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0339046321, __api_install_0339046321, __api_callback_0339046321;
    slot __ApiCallback0339046321, __API_CALLBACK_0339046321, __api_set_0339046321;
    base orch_api_base::routes::usage::scope;

    async fn filter<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::usage::scope::Opened, arg3: &'__api orch_api_base::routes::usage::scope::Wanted, arg4: bool) -> orch_api_base::middleware::requests::ApiResult<orch_db::usage::rollup::Filter>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_86c6ad7e57, __api_install_86c6ad7e57, __api_callback_86c6ad7e57;
    slot __ApiCallback86c6ad7e57, __API_CALLBACK_86C6AD7E57, __api_set_86c6ad7e57;
    base orch_api_base::routes::usage::scope;

    async fn all_record_places<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::usage::scope::Opened) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ddbe793671, __api_install_ddbe793671, __api_callback_ddbe793671;
    slot __ApiCallbackddbe793671, __API_CALLBACK_DDBE793671, __api_set_ddbe793671;
    base orch_api_base::routes::usage::scope;

    async fn visible_records<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: bool, arg3: Vec<orch_db::usage::ledger::EventRow>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_db::usage::ledger::EventRow>>;
}
