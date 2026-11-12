//! Agent profiles: the ones the caller may use or manage, reading one, making one in an org, changing, switching off and deleting one that no thread is bound to.
//!
//! A profile says which harness to run and how; it has no runtime of its own. Whoever writes one
//! is trusted by every provider it may be bound to, so a profile is made by whoever makes the
//! rules of its org, and the access engine decides that with every other question here: who sees
//! a profile, who may use it, who manages it, and what a refusal says is missing.

pub use crate::routes::access::page::NameBounds;

pub use crate::routes::access::page::NameSources;

use crate::routes::resources::view::PreparedAccess;

use orch_access_contract::decisions::Held;

pub use orch_access_contract::permissions::Permission;

pub use orch_db::queries::resources::routes::AgentRow;

pub use orch_types::Id;

pub use std::collections::HashMap;
/// What the caller holds on a profile, as the engine told it.
#[derive(Debug, Clone)]
pub struct Standing {
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    pub admin_place: Option<orch_access_contract::roles::PlaceRef>,
    pub participating: bool,
    pub admin_only: Vec<Permission>,
    pub owner: bool,
    pub administers: bool,
    pub held: Vec<Held>,
}

pub struct PreparedListProjection {
    pub rows: Vec<AgentRow>,
    pub standing: HashMap<Id, Standing>,
    pub access: PreparedAccess,
    pub sources: NameSources,
    pub bounds: NameBounds,
    pub associations: Vec<(Id, Id)>,
    pub providers: Vec<Id>,
    pub machines: Vec<Id>,
}

impl PreparedListProjection {
    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }

    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }

    pub fn provider_ids(&self) -> &[Id] {
        &self.providers
    }

    pub fn machine_ids(&self) -> &[Id] {
        &self.machines
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_005a5bf42e, __api_install_005a5bf42e, __api_callback_005a5bf42e;
    slot __ApiCallback005a5bf42e, __API_CALLBACK_005A5BF42E, __api_set_005a5bf42e;
    base orch_api_base::routes::resources::agents;

    async fn prepare_list_projection<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::agents::PreparedListProjection>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2a26470ecc, __api_install_2a26470ecc, __api_callback_2a26470ecc;
    slot __ApiCallback2a26470ecc, __API_CALLBACK_2A26470ECC, __api_set_2a26470ecc;
    base orch_api_base::routes::resources::agents;

    async fn show_list_prepared<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::resources::agents::PreparedListProjection) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Agent>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_51d577fc2d, __api_install_51d577fc2d, __api_callback_51d577fc2d;
    slot __ApiCallback51d577fc2d, __API_CALLBACK_51D577FC2D, __api_set_51d577fc2d;
    base orch_api_base::routes::resources::agents;

    async fn inventory<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Agent>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e6e77546c5, __api_install_e6e77546c5, __api_callback_e6e77546c5;
    slot __ApiCallbacke6e77546c5, __API_CALLBACK_E6E77546C5, __api_set_e6e77546c5;
    base orch_api_base::routes::resources::agents;

    async fn show<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::Agent>;
}
