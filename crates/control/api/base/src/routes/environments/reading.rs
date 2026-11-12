//! Who may read an environment and how much of it: whoever holds a permission on it reads all of it, and whoever holds none and may view a thread that runs there reads its name, owner, place, state and the job under way, and nothing of what is behind it.
//!
//! This is the one place that says it, for the request that reads an environment and for whatever
//! else shows the same view of one. `decide` decides, by the access engine's own answers: a
//! permission on the environment itself (its owner, an admin of its place, either level, any of its
//! four doors) is the whole of it, and a thread bound to it that the caller may view is the cut-down
//! view, which `through_a_thread` makes of a whole one. Somebody who has neither is refused with what
//! they lack to look at it and who can give it.

pub use orch_types::Id;

pub use std::collections::HashMap;
/// The existing topic-summary producer's actual bindings after its ordinary visibility check.
/// This returns identities only, before any environment name or current status is projected.
pub struct TopicSummarySources {
    pub bindings: Vec<(Id, Id)>,
    pub defaults: HashMap<Id, Option<Id>>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_50baf5598a, __api_install_50baf5598a, __api_callback_50baf5598a;
    slot __ApiCallback50baf5598a, __API_CALLBACK_50BAF5598A, __api_set_50baf5598a;
    base orch_api_base::routes::environments::reading;

    async fn topic_summary_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::reading::TopicSummarySources>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0794630518, __api_install_0794630518, __api_callback_0794630518;
    slot __ApiCallback0794630518, __API_CALLBACK_0794630518, __api_set_0794630518;
    base orch_api_base::routes::environments::reading;

    async fn topic_summaries<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::environment::EnvironmentRef>>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f45b52f7df, __api_install_f45b52f7df, __api_callback_f45b52f7df;
    slot __ApiCallbackf45b52f7df, __API_CALLBACK_F45B52F7DF, __api_set_f45b52f7df;
    base orch_api_base::routes::environments::reading;

    async fn topic_summaries_from_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::environments::reading::TopicSummarySources, arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::environment::EnvironmentRef>>>;
}

/// How much of an environment a caller may read, which is how they reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// They hold a permission on the environment: all of it, and its configuration and the names of
    /// its secrets with `environment.manage`.
    Whole,
    /// They hold nothing there and may view a thread that runs in it: what `through_a_thread` leaves.
    ThroughAThread,
}

orch_api_base::api_family_callback! {
    bind __api_bind_cfc8a895c0, __api_install_cfc8a895c0, __api_callback_cfc8a895c0;
    slot __ApiCallbackcfc8a895c0, __API_CALLBACK_CFC8A895C0, __api_set_cfc8a895c0;
    base orch_api_base::routes::environments::reading;

    async fn decide<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::reading::Reading>;
}
