//! Providers on a key: the ones the caller owns, may use and manages, reading one without its key, making one with its endpoints and its key sealed, changing, switching off and deleting it, and writing the key and the secret headers again.
//!
//! A provider is what a thread's model calls go through. An API provider is one upstream that one key
//! opens; the key is sealed before it is stored and no request returns it, nor any secret header's value:
//! a provider says only that it has a key and which secret headers have a value. Who sees a provider,
//! who may use it, who manages it, and who may make one where, is the access engine's to say; the
//! policies that govern a provider's home are settings, resolved there.

use orch_access_contract::decisions::Held;

pub use orch_access_contract::permissions::Permission;

pub use orch_access_contract::permissions::ResourceKind;

pub use orch_db::queries::resources::routes::ProviderRow;

pub use orch_types::Id;

pub use std::collections::HashMap;
// ---- policies at a provider's home ----

/// The settings that govern a provider at its home.
pub struct HomePolicies {
    pub members_make: Vec<ResourceKind>,
    pub http_allowed: bool,
    pub subscription_harnesses: Option<Vec<String>>,
    pub subscription_sharing: Option<Vec<String>>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_0f1e4ccdb2, __api_install_0f1e4ccdb2, __api_callback_0f1e4ccdb2;
    slot __ApiCallback0f1e4ccdb2, __API_CALLBACK_0F1E4CCDB2, __api_set_0f1e4ccdb2;
    base orch_api_base::routes::resources::providers;

    async fn home_policies<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::providers::HomePolicies>;
}

// ---- the standing of a provider ----

/// What a provider is, to the people it is shown to: whether the caller manages it decides how
/// much of its inside they see.
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

impl Standing {
    pub fn views(&self) -> bool {
        self.held
            .iter()
            .any(|held| held.permission == Permission::ProviderView)
    }

    pub fn manages(&self) -> bool {
        self.held
            .iter()
            .any(|held| held.permission == Permission::ProviderManage)
    }
}

// ---- the view ----

#[derive(Clone)]
pub struct PreparedProviderViews {
    pub access: super::view::PreparedAccess,
    pub caps: crate::routes::usage::caps::PreparedProviderChoices,
    pub sources: crate::routes::access::page::NameSources,
    pub bounds: crate::routes::access::page::NameBounds,
}

#[derive(Clone)]
pub struct PreparedProjection {
    pub row: ProviderRow,
    pub standing: Standing,
    pub projection: PreparedProviderViews,
}

impl PreparedProjection {
    /// Actual protected sources retained by the existing ordinary Provider reader.
    pub fn name_sources(&self) -> crate::routes::access::page::NameSources {
        self.projection.sources.clone()
    }

    /// The consuming operation supplies its own current same-operation name bound.
    pub fn bound_names(&mut self, bounds: crate::routes::access::page::NameBounds) {
        self.projection.bounds = bounds;
    }
}

/// Shared source-role contract used by readable Provider projections in a compound read.
pub fn readable_name_roles() -> &'static crate::routes::access::operations::NameRoles {
    &super::provider_projection::NAMES
}

orch_api_base::api_family_callback! {
    bind __api_bind_1b8aab182e, __api_install_1b8aab182e, __api_callback_1b8aab182e;
    slot __ApiCallback1b8aab182e, __API_CALLBACK_1B8AAB182E, __api_set_1b8aab182e;
    base orch_api_base::routes::resources::providers;

    async fn prepare_readable_projection<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::providers::PreparedProjection>;
}

pub struct PreparedListProjection {
    pub rows: Vec<ProviderRow>,
    pub standing: HashMap<Id, Standing>,
    pub projection: PreparedProviderViews,
}

impl PreparedListProjection {
    pub fn name_sources(&self) -> &crate::routes::access::page::NameSources {
        &self.projection.sources
    }

    pub fn bound_names(&mut self, bounds: crate::routes::access::page::NameBounds) {
        self.projection.bounds = bounds;
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_23f24917ee, __api_install_23f24917ee, __api_callback_23f24917ee;
    slot __ApiCallback23f24917ee, __API_CALLBACK_23F24917EE, __api_set_23f24917ee;
    base orch_api_base::routes::resources::providers;

    async fn prepare_list_projection<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::providers::PreparedListProjection>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9be6fd1d28, __api_install_9be6fd1d28, __api_callback_9be6fd1d28;
    slot __ApiCallback9be6fd1d28, __API_CALLBACK_9BE6FD1D28, __api_set_9be6fd1d28;
    base orch_api_base::routes::resources::providers;

    async fn show_list_prepared<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::resources::providers::PreparedListProjection) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Provider>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_4494adecd2, __api_install_4494adecd2, __api_callback_4494adecd2;
    slot __ApiCallback4494adecd2, __API_CALLBACK_4494ADECD2, __api_set_4494adecd2;
    base orch_api_base::routes::resources::providers;

    async fn show_prepared<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_api_base::routes::resources::providers::PreparedProjection) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::Provider>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_420c6a586c, __api_install_420c6a586c, __api_callback_420c6a586c;
    slot __ApiCallback420c6a586c, __API_CALLBACK_420C6A586C, __api_set_420c6a586c;
    base orch_api_base::routes::resources::providers;

    async fn inventory<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Provider>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e9b9616032, __api_install_e9b9616032, __api_callback_e9b9616032;
    slot __ApiCallbacke9b9616032, __API_CALLBACK_E9B9616032, __api_set_e9b9616032;
    base orch_api_base::routes::resources::providers;

    async fn show<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::Provider>;
}
