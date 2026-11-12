//! Native Node header sources selected before its protected references are formatted.

pub use super::view;

use crate::routes::environments::reading::TopicSummarySources;

pub use orch_access_contract::permissions::ObjectKind;

pub use orch_types::Id;

use std::collections::BTreeMap;

pub use std::collections::BTreeSet;
/// Factual source identities and the exact native associations of a successful header.
pub struct Prepared {
    pub objects: BTreeSet<(ObjectKind, Id)>,
    pub path: Vec<Id>,
    pub kind: String,
    pub readmes: Vec<(Id, Id, Option<Id>)>,
    pub names: BTreeSet<Id>,
    pub displays: BTreeSet<Id>,
    pub authority: view::ReferenceAuthority,
    pub thread: Option<orch_engine::views::ThreadMetadataSources>,
    pub topic_bindings: Vec<(Id, Id)>,
    pub topic_defaults: BTreeMap<Id, Option<Id>>,
    pub topic: Option<TopicSummarySources>,
}

impl Prepared {
    pub fn preserves(&self, fresh: &Self) -> bool {
        self.path == fresh.path
            && self.kind == fresh.kind
            && self.readmes == fresh.readmes
            && self.thread == fresh.thread
            && self.topic_bindings == fresh.topic_bindings
            && self.topic_defaults == fresh.topic_defaults
            && self.objects.is_subset(&fresh.objects)
            && self.names.is_subset(&fresh.names)
            && self.displays.is_subset(&fresh.displays)
            && self.authority.preserves(&fresh.authority)
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_8d4168e970, __api_install_8d4168e970, __api_callback_8d4168e970;
    slot __ApiCallback8d4168e970, __API_CALLBACK_8D4168E970, __api_set_8d4168e970;
    base orch_api_base::routes::nodes::content_sources;

    async fn prepare<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::nodes::content_sources::Prepared>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d562690967, __api_install_d562690967, __api_callback_d562690967;
    slot __ApiCallbackd562690967, __API_CALLBACK_D562690967, __api_set_d562690967;
    base orch_api_base::routes::nodes::content_sources;

    async fn render<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_api_base::routes::nodes::content_sources::Prepared) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::nodes::Node>;
}
