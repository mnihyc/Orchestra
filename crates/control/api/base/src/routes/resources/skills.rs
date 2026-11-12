//! Skills: made from an uploaded archive, an environment's directory or a draft begun in Orch; read with their versions and the files of each; switched on and off and deleted; given further versions by upload, by reading their directory again, or by publishing the draft their managers edit.
//!
//! A version is made in two steps so that no transaction waits on the store or on a machine: the
//! caller's right to make it and its source are checked, the files are read, checked and kept as an
//! archive outside any transaction, and the version is then recorded, with the caller's right asked
//! again, in the transaction that writes it.

pub use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::resources::view::PreparedAccess;

pub use orch_authz::resources;

pub use orch_db::queries::resources::skills::SkillRow;
orch_api_base::api_family_callback! {
    bind __api_bind_d3042354bd, __api_install_d3042354bd, __api_callback_d3042354bd;
    slot __ApiCallbackd3042354bd, __API_CALLBACK_D3042354BD, __api_set_d3042354bd;
    base orch_api_base::routes::resources::skills;

    async fn after_single_response<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_api_base::key_operations::native::resource_reply::ResponseRead) -> orch_api_base::middleware::requests::ApiResult<()>;
}

pub struct PreparedInventory {
    pub seen: Vec<resources::Seen>,
    pub skills: Vec<SkillRow>,
    pub access: PreparedAccess,
    pub sources: NameSources,
    pub bounds: NameBounds,
}

impl PreparedInventory {
    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }
    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_b0aaad3905, __api_install_b0aaad3905, __api_callback_b0aaad3905;
    slot __ApiCallbackb0aaad3905, __API_CALLBACK_B0AAAD3905, __api_set_b0aaad3905;
    base orch_api_base::routes::resources::skills;

    async fn prepare_inventory<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::skills::PreparedInventory>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a11b3ea78e, __api_install_a11b3ea78e, __api_callback_a11b3ea78e;
    slot __ApiCallbacka11b3ea78e, __API_CALLBACK_A11B3EA78E, __api_set_a11b3ea78e;
    base orch_api_base::routes::resources::skills;

    async fn show_inventory_prepared<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::resources::skills::PreparedInventory) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::skills::Skill>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c23fe61081, __api_install_c23fe61081, __api_callback_c23fe61081;
    slot __ApiCallbackc23fe61081, __API_CALLBACK_C23FE61081, __api_set_c23fe61081;
    base orch_api_base::routes::resources::skills;

    async fn inventory<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::skills::Skill>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_00c9851850, __api_install_00c9851850, __api_callback_00c9851850;
    slot __ApiCallback00c9851850, __API_CALLBACK_00C9851850, __api_set_00c9851850;
    base orch_api_base::routes::resources::skills;

    async fn show<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::skills::Skill>;
}
