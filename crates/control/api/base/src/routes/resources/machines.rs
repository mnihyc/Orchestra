//! A person's machines: the token a machine enrolls with, listing and reading with whether each is online and what was registered from it, renaming, revoking, asking a machine for its facts again, and its local policy as it reported it with the keys its owner handed over.
//!
//! A machine's page names its owners and other holders; a stranger is told it is not there.
//! An enabled administrator can review its page and explicitly name themself before managing it.
//! Its local policy has the last word. Keys are handed to Orch one by one; the access engine
//! checks who manages what each key limits before changing it. The machine decides what stays local.

pub use crate::middleware::requests::ApiResult;

pub use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::resources::view::PreparedAccess;

pub use orch_access_contract::decisions::ObjectRef;

pub use orch_access_contract::permissions::ObjectKind;

pub use orch_db::queries::resources::MachineRow;

pub use orch_db::queries::resources::routes as queries;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use std::collections::BTreeMap;
orch_api_base::api_family_callback! {
    bind __api_bind_c5ec983f9e, __api_install_c5ec983f9e, __api_callback_c5ec983f9e;
    slot __ApiCallbackc5ec983f9e, __API_CALLBACK_C5EC983F9E, __api_set_c5ec983f9e;
    base orch_api_base::routes::resources::machines;

    async fn own_machine<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_db::queries::resources::MachineRow>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_55c8b9c49d, __api_install_55c8b9c49d, __api_callback_55c8b9c49d;
    slot __ApiCallback55c8b9c49d, __API_CALLBACK_55C8B9C49D, __api_set_55c8b9c49d;
    base orch_api_base::routes::resources::machines;

    async fn require_machine<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_access_contract::permissions::Permission) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a909d67302, __api_install_a909d67302, __api_callback_a909d67302;
    slot __ApiCallbacka909d67302, __API_CALLBACK_A909D67302, __api_set_a909d67302;
    base orch_api_base::routes::resources::machines;

    async fn registrable_machine_on<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_db::queries::resources::MachineRow>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b444ed81ee, __api_install_b444ed81ee, __api_callback_b444ed81ee;
    slot __ApiCallbackb444ed81ee, __API_CALLBACK_B444ED81EE, __api_set_b444ed81ee;
    base orch_api_base::routes::resources::machines;

    async fn machine_views<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: Vec<orch_db::queries::resources::MachineRow>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Machine>>;
}

/// Native physical-machine/facade facts; minimal registered refs remain this inventory's content.
pub struct PreparedInventory {
    pub grouping: super::grouping::Prepared,
    pub found: Vec<MachineRow>,
    pub hosts: Vec<(Id, Id, String)>,
    pub effective: Vec<orch_authz::resources::Effective>,
    pub access: PreparedAccess,
    pub sources: NameSources,
    pub bounds: NameBounds,
    pub registered: Registered,
}

pub struct Registered {
    pub providers: Vec<queries::ProviderRefRow>,
    pub agents: Vec<queries::AgentRefRow>,
    pub environments: Vec<queries::EnvironmentRefRow>,
    pub sources: Vec<ObjectRef>,
}

impl Registered {
    pub fn associations(&self) -> BTreeMap<(ObjectKind, Id), Option<Id>> {
        self.providers
            .iter()
            .map(|row| ((ObjectKind::Provider, row.id), row.machine_id))
            .chain(
                self.agents
                    .iter()
                    .map(|row| ((ObjectKind::Agent, row.id), row.machine_id)),
            )
            .chain(
                self.environments
                    .iter()
                    .map(|row| ((ObjectKind::Environment, row.id), row.machine_id)),
            )
            .collect()
    }

    pub async fn load(conn: &mut Conn, machines: &[Id]) -> ApiResult<Self> {
        if machines.is_empty() {
            return Ok(Self {
                providers: Vec::new(),
                agents: Vec::new(),
                environments: Vec::new(),
                sources: Vec::new(),
            });
        }
        let providers = queries::providers_among(conn, &[], machines).await?;
        let agents = queries::agents_among(conn, &[], machines).await?;
        let environments = queries::environments_among(conn, &[], machines).await?;
        let mut sources: Vec<_> = providers
            .iter()
            .map(|row| ObjectRef {
                kind: ObjectKind::Provider,
                id: row.id,
            })
            .chain(agents.iter().map(|row| ObjectRef {
                kind: ObjectKind::Agent,
                id: row.id,
            }))
            .chain(environments.iter().map(|row| ObjectRef {
                kind: ObjectKind::Environment,
                id: row.id,
            }))
            .collect();
        sources.sort_unstable_by_key(|source| (source.kind, source.id));
        sources.dedup();
        Ok(Self {
            providers,
            agents,
            environments,
            sources,
        })
    }
}

impl PreparedInventory {
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        let registrations = fresh.registered.associations();
        self.grouping.preserves_sources(&fresh.grouping)
            && self.access.preserves_authority(&fresh.access)
            && self
                .found
                .iter()
                .all(|row| fresh.found.iter().any(|current| current.id == row.id))
            && self.hosts.iter().all(|(machine, host, _)| {
                fresh
                    .hosts
                    .iter()
                    .any(|(current_machine, current_host, _)| {
                        current_machine == machine && current_host == host
                    })
            })
            && self
                .registered
                .associations()
                .iter()
                .all(|(source, machine)| registrations.get(source) == Some(machine))
    }

    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }
    pub fn reference_sources(&self) -> &[ObjectRef] {
        &self.registered.sources
    }
    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_63d7556e7e, __api_install_63d7556e7e, __api_callback_63d7556e7e;
    slot __ApiCallback63d7556e7e, __API_CALLBACK_63D7556E7E, __api_set_63d7556e7e;
    base orch_api_base::routes::resources::machines;

    async fn prepare_inventory<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Vec<orch_db::queries::resources::MachineRow>) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::machines::PreparedInventory>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c297297595, __api_install_c297297595, __api_callback_c297297595;
    slot __ApiCallbackc297297595, __API_CALLBACK_C297297595, __api_set_c297297595;
    base orch_api_base::routes::resources::machines;

    async fn show_inventory_prepared<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::resources::machines::PreparedInventory) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Machine>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d2ace19f11, __api_install_d2ace19f11, __api_callback_d2ace19f11;
    slot __ApiCallbackd2ace19f11, __API_CALLBACK_D2ACE19F11, __api_set_d2ace19f11;
    base orch_api_base::routes::resources::machines;

    async fn machine_with_readiness<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_db::queries::resources::MachineRow) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::Machine>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_cb869fa787, __api_install_cb869fa787, __api_callback_cb869fa787;
    slot __ApiCallbackcb869fa787, __API_CALLBACK_CB869FA787, __api_set_cb869fa787;
    base orch_api_base::routes::resources::machines;

    async fn reported_policy<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::queries::resources::MachineRow) -> orch_api_base::middleware::requests::ApiResult<Option<orch_link_contract::policy::LocalPolicy>>;
}

#[path = "machines/enrollment.rs"]
pub mod enrollment;
