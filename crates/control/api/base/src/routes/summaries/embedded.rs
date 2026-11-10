//! A native opening carries its summaries under that opening's own declared source roles.

pub use super::*;
pub struct Embedded {
    pub views: Vec<View>,
    pub range: SummaryChatRange,
    pub slots: [usize; 3],
}

impl Embedded {
    pub fn snapshots(&self) -> Vec<SummarySnapshot> {
        self.views
            .iter()
            .map(|view| view.snapshot.clone())
            .collect()
    }
    pub fn facts(&self) -> [Vec<KeyResolvedTarget>; 3] {
        [
            self.views
                .iter()
                .map(|view| view.identity.fact().clone())
                .collect(),
            self.views
                .iter()
                .flat_map(|view| view.inputs.iter().cloned())
                .collect(),
            self.views
                .iter()
                .flat_map(|view| view.providers.iter().cloned())
                .collect(),
        ]
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_e93ed15ce0, __api_install_e93ed15ce0, __api_callback_e93ed15ce0;
    slot __ApiCallbacke93ed15ce0, __API_CALLBACK_E93ED15CE0, __api_set_e93ed15ce0;
    base orch_api_base::routes::summaries::embedded;

    async fn prepare<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: &'__api orch_engine::operation_scope::Prepared, arg4: orch_types::Id, arg5: [usize; 3]) -> orch_api_base::middleware::requests::ApiResult<Option<std::sync::Arc<orch_api_base::routes::summaries::embedded::Embedded>>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_7e5709b692, __api_install_7e5709b692, __api_callback_7e5709b692;
    slot __ApiCallback7e5709b692, __API_CALLBACK_7E5709B692, __api_set_7e5709b692;
    base orch_api_base::routes::summaries::embedded;

    fn retain<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_authz::Principal, arg2: &'__api std::sync::Arc<orch_api_base::routes::summaries::embedded::Embedded>) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_face5ea742, __api_install_face5ea742, __api_callback_face5ea742;
    slot __ApiCallbackface5ea742, __API_CALLBACK_FACE5EA742, __api_set_face5ea742;
    base orch_api_base::routes::summaries::embedded;

    async fn note_read<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::summaries::embedded::Embedded) -> orch_api_base::middleware::requests::ApiResult<()>;
}
