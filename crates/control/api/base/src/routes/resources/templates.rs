//! Templates of both kinds: the ones the caller may use or manage, reading one, making one for an org or for the system, changing, switching off and deleting one that nothing was made from.
//!
//! A template is a recipe for units, environments or the runtimes of providers. Here it is data: it
//! is made, changed and kept, and no build is started. Who may make one where is the access engine's
//! to say; a driver the setting `templates.restricted_drivers` lists is saved by a system admin
//! alone, wherever the template is to be. The spec is kept as it is given: the driver of a host
//! reads it; the reported schema checks every save, and the driver also validates it before making a unit.

pub use crate::routes::access::page::NameBounds;

pub use crate::routes::access::page::NameSources;

use crate::routes::resources::view::PreparedAccess;

use orch_access_contract::decisions::Held;

pub use orch_access_contract::permissions::Permission;

pub use orch_db::queries::resources::routes::TemplateRow;

pub use orch_types::Id;

pub use std::collections::HashMap;
/// What the caller holds on a template, as the engine told it.
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
    pub rows: Vec<TemplateRow>,
    pub standing: HashMap<Id, Standing>,
    pub access: PreparedAccess,
    pub sources: NameSources,
    pub bounds: NameBounds,
}

impl PreparedListProjection {
    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }

    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_46453f6116, __api_install_46453f6116, __api_callback_46453f6116;
    slot __ApiCallback46453f6116, __API_CALLBACK_46453F6116, __api_set_46453f6116;
    base orch_api_base::routes::resources::templates;

    async fn inventory<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Template>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9924a0b621, __api_install_9924a0b621, __api_callback_9924a0b621;
    slot __ApiCallback9924a0b621, __API_CALLBACK_9924A0B621, __api_set_9924a0b621;
    base orch_api_base::routes::resources::templates;

    async fn prepare_list_projection<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::templates::PreparedListProjection>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9e6441beea, __api_install_9e6441beea, __api_callback_9e6441beea;
    slot __ApiCallback9e6441beea, __API_CALLBACK_9E6441BEEA, __api_set_9e6441beea;
    base orch_api_base::routes::resources::templates;

    async fn show_list_prepared<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::resources::templates::PreparedListProjection) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Template>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d83d9b6fff, __api_install_d83d9b6fff, __api_callback_d83d9b6fff;
    slot __ApiCallbackd83d9b6fff, __API_CALLBACK_D83D9B6FFF, __api_set_d83d9b6fff;
    base orch_api_base::routes::resources::templates;

    async fn show<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::Template>;
}
