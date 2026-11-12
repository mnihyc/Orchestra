//! Mount resources, locations on hosts and personal machines, and the places and environments that use them.
//!
//! Location sources are shown only to callers who may change that location. A private layer is
//! environment data; the underlying mount is never copied by changing where an environment runs.

pub use crate::routes::access::page::NameBounds;

pub use crate::routes::access::page::NameSources;

use crate::routes::resources::view::PreparedAccess;

pub use orch_api_contract::mounts as api;

pub use orch_api_contract::mounts::MountLocation;

pub use orch_authz::resources;

pub use orch_db::queries::resources::mounts::MountRow;

pub use orch_types::Id;

pub use std::collections::BTreeMap;
orch_api_base::api_family_callback! {
    bind __api_bind_dd4e30c19a, __api_install_dd4e30c19a, __api_callback_dd4e30c19a;
    slot __ApiCallbackdd4e30c19a, __API_CALLBACK_DD4E30C19A, __api_set_dd4e30c19a;
    base orch_api_base::routes::resources::mounts;

    async fn show<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::mounts::Mount>;
}

pub struct PreparedInventory {
    pub items: Vec<PreparedMount>,
    pub sources: NameSources,
    pub bounds: NameBounds,
    pub machines: Vec<Id>,
}

pub struct PreparedMount {
    pub row: MountRow,
    pub effective: resources::Effective,
    pub access: PreparedAccess,
    pub locations_total: usize,
    pub locations: Vec<MountLocation>,
    pub kept: Vec<api::MountKeptData>,
    pub lost_locations: Vec<api::MountLostLocation>,
    pub attached_environments: i64,
    pub sources: NameSources,
    pub publication: BTreeMap<NativeLocation, bool>,
}

/// Native location keys are captured before formatting; the boolean is the existing
/// native source-path visibility gate, not authority reconstructed from a reply.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeLocation {
    Active(Id, String, Option<String>, String),
    Kept(Id, Id),
}

impl PreparedInventory {
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        let items = fresh
            .items
            .iter()
            .map(|item| (item.row.id, item))
            .collect::<BTreeMap<_, _>>();
        self.items.iter().all(|old| {
            items.get(&old.row.id).is_some_and(|current| {
                old.publication.iter().all(|(source, disclosed)| {
                    current
                        .publication
                        .get(source)
                        .is_some_and(|now| !*disclosed || *now)
                })
            })
        })
    }
    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }
    pub fn machine_ids(&self) -> &[Id] {
        &self.machines
    }
    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_66bcfd9efe, __api_install_66bcfd9efe, __api_callback_66bcfd9efe;
    slot __ApiCallback66bcfd9efe, __API_CALLBACK_66BCFD9EFE, __api_set_66bcfd9efe;
    base orch_api_base::routes::resources::mounts;

    async fn prepare_inventory<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::mounts::PreparedInventory>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1cbe1308fe, __api_install_1cbe1308fe, __api_callback_1cbe1308fe;
    slot __ApiCallback1cbe1308fe, __API_CALLBACK_1CBE1308FE, __api_set_1cbe1308fe;
    base orch_api_base::routes::resources::mounts;

    async fn show_inventory_prepared<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::resources::mounts::PreparedInventory) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::mounts::Mount>>;
}
