//! Resource paragraphs through native Read, paid-refresh admission and metadata-only receipts.

pub use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_api_contract::summaries::*;

pub use orch_engine::summaries::sources;
orch_api_base::api_family_callback! {
    bind __api_bind_862439cb84, __api_install_862439cb84, __api_callback_862439cb84;
    slot __ApiCallback862439cb84, __API_CALLBACK_862439CB84, __api_set_862439cb84;
    base orch_api_base::routes::summaries;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_2f44c829e4, __api_install_2f44c829e4, __api_callback_2f44c829e4;
    slot __ApiCallback2f44c829e4, __API_CALLBACK_2F44C829E4, __api_set_2f44c829e4;
    base orch_api_base::routes::summaries;

    async fn note_chat_openings<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_access_contract::key_scope::KeyResolvedTarget]) -> orch_api_base::middleware::requests::ApiResult<()>;
}

pub struct View {
    pub identity: sources::Identity,
    pub inputs: Vec<KeyResolvedTarget>,
    pub providers: Vec<KeyResolvedTarget>,
    pub snapshot: SummarySnapshot,
    pub policy: orch_types::Hash,
}

#[path = "summaries/embedded.rs"]
pub mod embedded;
