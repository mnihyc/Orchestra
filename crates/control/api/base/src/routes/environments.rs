//! Environments as resources: creation, lifecycle, configuration changed while one runs, their own secrets, what the exec node offers, the MCP servers found there, their import and one of an environment's own, who reaches into one, its history, update plans, removal, moves, and their jobs.
//!
//! An environment is a resource like the others: it belongs to a person, is listed to everyone who
//! holds something on it, and is shared, changed and removed by the same means. This module serves
//! directories registered through their machine's Use permission, and what
//! every environment has beside: its page's readers, its own settings and its secrets, which are
//! sealed and never read back. Every decision is the access engine's. An environment made from a
//! template on a host needs a host's node and the engine's jobs, which are not served here: reading
//! and listing them works, creating one and its lifecycle do not.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

pub use crate::routes::nodes::view as node_view;

use crate::routes::resources::view::PreparedAccess;

use orch_access_contract::decisions::Held;

pub use orch_access_contract::decisions::ObjectRef;

pub use orch_access_contract::permissions::Permission;

pub use orch_api_contract::environment::Environment;

pub use orch_authz::Principal;

pub use orch_authz::resources;

pub use orch_db::queries::environments as env_rows;

pub use orch_db::queries::environments::hosting;

pub use orch_db::queries::environments::routes as queries;

pub use orch_db::queries::environments::routes::EnvironmentFull;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use std::collections::BTreeSet;

pub use std::collections::HashMap;

pub use exec_version::confirmed as confirmed_machine_programs;

pub use places::host_choices;
orch_api_base::api_family_callback! {
    bind __api_bind_a7561acc31, __api_install_a7561acc31, __api_callback_a7561acc31;
    slot __ApiCallbacka7561acc31, __API_CALLBACK_A7561ACC31, __api_set_a7561acc31;
    base orch_api_base::routes::environments;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

// ---- the view ----

/// What the caller holds on an environment, as the engine told it.
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
    pub fn manages(&self) -> bool {
        self.held
            .iter()
            .any(|held| held.permission == Permission::EnvironmentManage)
    }

    /// Whether the caller holds anything on the environment: owning it, administering its place, or a
    /// row of its page. Somebody who does not sees it only through a thread (see [`reading::decide`]).
    pub fn holds_something(&self) -> bool {
        self.held.iter().any(|held| {
            !matches!(
                held.permission,
                Permission::AccessManage | Permission::AccessExternal
            )
        })
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_5e3042a2f8, __api_install_5e3042a2f8, __api_callback_5e3042a2f8;
    slot __ApiCallback5e3042a2f8, __API_CALLBACK_5E3042A2F8, __api_set_5e3042a2f8;
    base orch_api_base::routes::environments;

    async fn inventory<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::environment::Environment>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_4c32840a49, __api_install_4c32840a49, __api_callback_4c32840a49;
    slot __ApiCallback4c32840a49, __API_CALLBACK_4C32840A49, __api_set_4c32840a49;
    base orch_api_base::routes::environments;

    async fn prepare_inventory<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::PreparedInventory>;
}

/// Opaque existing Environment view facts; no ordinary permission or operation is selected here.
pub struct PreparedInventory {
    pub rows: Vec<EnvironmentFull>,
    pub standing: HashMap<Id, Standing>,
    pub access: PreparedAccess,
    pub sources: NameSources,
    pub bounds: NameBounds,
    pub machine_facts: HashMap<Id, orch_db::queries::environment_facts::EnvironmentMachine>,
    pub hosted: Vec<(Id, Id, String)>,
    pub defaults: Vec<(Id, Id)>,
    pub threads: Vec<env_rows::ThreadInEnvironment>,
    pub runtime_rows: Vec<hosting::RuntimeFull>,
    pub templates: Vec<Id>,
    pub template_names: Vec<(Id, String, String)>,
    pub hosts: Vec<Id>,
    pub physical_machines: Vec<Id>,
    pub display_settings: list_settings::DisplaySettings,
    pub mounts: HashMap<Id, orch_engine::mounts::PreparedViews>,
    pub mount_sources: Vec<Id>,
}

impl PreparedInventory {
    /// Native FK/reference associations used by this exact prepared Environment body.
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        let rows = fresh
            .rows
            .iter()
            .map(|row| (row.id, row))
            .collect::<HashMap<_, _>>();
        let hosted = fresh
            .hosted
            .iter()
            .map(|(first, second, _)| (*first, *second))
            .collect::<BTreeSet<_>>();
        self.access.preserves_authority(&fresh.access)
            && self.rows.iter().all(|old| {
                rows.get(&old.id).is_some_and(|current| {
                    old.placement == current.placement
                        && old.template_id == current.template_id
                        && old.machine_id == current.machine_id
                })
            })
            && self
                .hosted
                .iter()
                .all(|(first, second, _)| hosted.contains(&(*first, *second)))
            && self.machine_facts.iter().all(|(id, old)| {
                old.machine_id.is_none_or(|machine| {
                    fresh
                        .machine_facts
                        .get(id)
                        .is_some_and(|current| current.machine_id == Some(machine))
                })
            })
            && self.mounts.iter().all(|(environment, old)| {
                fresh.mounts.get(environment).is_some_and(|current| {
                    let selected: BTreeSet<_> = current.mount_ids().collect();
                    old.mount_ids().all(|mount| selected.contains(&mount))
                })
            })
    }

    pub fn selected_ids(&self) -> Vec<Id> {
        self.rows.iter().map(|row| row.id).collect()
    }
    /// The native through-thread style has no Host/Template/Machine/Mount disclosure.
    pub async fn content_sources(
        &self,
        conn: &mut Conn,
    ) -> ApiResult<HashMap<Id, Vec<orch_access_contract::decisions::ObjectRef>>> {
        use orch_access_contract::permissions::ObjectKind;
        let live = orch_db::queries::resources::routes::live_machine_ids(
            conn,
            self.physical_machine_ids(),
        )
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
        let mut sources = HashMap::new();
        for row in &self.rows {
            let mut selected = Vec::new();
            if self
                .standing
                .get(&row.id)
                .is_some_and(Standing::holds_something)
            {
                if let Some(id) = row.template_id
                    && self.template_ids().contains(&id)
                {
                    selected.push(orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::Template,
                        id,
                    });
                }
                selected.extend(
                    self.hosted
                        .iter()
                        .filter(|(env, id, _)| *env == row.id && self.host_ids().contains(id))
                        .map(|(_, id, _)| orch_access_contract::decisions::ObjectRef {
                            kind: ObjectKind::Host,
                            id: *id,
                        }),
                );
                if let Some(id) = self
                    .machine_facts
                    .get(&row.id)
                    .and_then(|facts| facts.machine_id)
                    .filter(|id| live.contains(id))
                {
                    selected.push(orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::Machine,
                        id,
                    });
                }
                if let Some(mounts) = self.mounts.get(&row.id) {
                    selected.extend(
                        mounts
                            .mount_ids()
                            .filter(|id| self.mount_ids().contains(id))
                            .map(|id| orch_access_contract::decisions::ObjectRef {
                                kind: ObjectKind::Mount,
                                id,
                            }),
                    );
                }
            }
            selected.sort_unstable_by_key(|object| (object.kind, object.id));
            selected.dedup();
            sources.insert(row.id, selected);
        }
        Ok(sources)
    }
    /// Source identities for the native PlaceEnvironment subset, before its DTO is constructed.
    pub async fn place_names(&self, conn: &mut Conn) -> ApiResult<NameSources> {
        let mut wanted = NameSources::default();
        let ids = self
            .rows
            .iter()
            .map(|row| row.id)
            .chain(self.hosted.iter().map(|(_, id, _)| *id))
            .collect::<Vec<_>>();
        wanted.nodes.extend(
            orch_authz::resources::address_name_sources(conn, &ids)
                .await
                .map_err(ApiError::internal)?,
        );
        wanted.nodes.extend(
            self.threads
                .iter()
                .flat_map(|thread| [thread.thread_id, thread.topic_id]),
        );
        Ok(wanted)
    }
    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }
    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
    pub fn template_ids(&self) -> &[Id] {
        &self.templates
    }
    pub fn host_ids(&self) -> &[Id] {
        &self.hosts
    }
    /// Native selected attachments, prepared before any Environment response is rendered.
    pub fn mount_ids(&self) -> &[Id] {
        &self.mount_sources
    }
    /// Physical policy/capacity source from the ordinary environment_facts supplier.
    pub fn physical_machine_ids(&self) -> &[Id] {
        &self.physical_machines
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_a51865252c, __api_install_a51865252c, __api_callback_a51865252c;
    slot __ApiCallbacka51865252c, __API_CALLBACK_A51865252C, __api_set_a51865252c;
    base orch_api_base::routes::environments;

    async fn show_inventory_prepared<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::environments::PreparedInventory) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::environment::Environment>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1ae31b360e, __api_install_1ae31b360e, __api_callback_1ae31b360e;
    slot __ApiCallback1ae31b360e, __API_CALLBACK_1AE31B360E, __api_set_1ae31b360e;
    base orch_api_base::routes::environments;

    async fn show<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::environment::Environment>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_82ef5af6e4, __api_install_82ef5af6e4, __api_callback_82ef5af6e4;
    slot __ApiCallback82ef5af6e4, __API_CALLBACK_82EF5AF6E4, __api_set_82ef5af6e4;
    base orch_api_base::routes::environments;

    async fn finish_shown<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: orch_api_contract::environment::Environment) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::environment::Environment>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1feedd6762, __api_install_1feedd6762, __api_callback_1feedd6762;
    slot __ApiCallback1feedd6762, __API_CALLBACK_1FEEDD6762, __api_set_1feedd6762;
    base orch_api_base::routes::environments;

    async fn prepare_viewed<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::PreparedInventory>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_58797c68d7, __api_install_58797c68d7, __api_callback_58797c68d7;
    slot __ApiCallback58797c68d7, __API_CALLBACK_58797C68D7, __api_set_58797c68d7;
    base orch_api_base::routes::environments;

    async fn environments_of<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::environment::Environment>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_da2cee7ceb, __api_install_da2cee7ceb, __api_callback_da2cee7ceb;
    slot __ApiCallbackda2cee7ceb, __API_CALLBACK_DA2CEE7CEB, __api_set_da2cee7ceb;
    base orch_api_base::routes::environments;

    fn job_ref<'__api>(arg0: &'__api orch_db::queries::jobs::JobRow) -> orch_api_contract::nodes::JobRef;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b143dd075a, __api_install_b143dd075a, __api_callback_b143dd075a;
    slot __ApiCallbackb143dd075a, __API_CALLBACK_B143DD075A, __api_set_b143dd075a;
    base orch_api_base::routes::environments;

    fn ending_error(arg0: orch_engine::environments::ending::Error) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e075801632, __api_install_e075801632, __api_callback_e075801632;
    slot __ApiCallbacke075801632, __API_CALLBACK_E075801632, __api_set_e075801632;
    base orch_api_base::routes::environments;

    fn removal_schedule_error(arg0: orch_engine::archive::RemovalScheduleError) -> orch_api_base::middleware::requests::ApiError;
}

#[path = "environments/exec_version.rs"]
pub mod exec_version;

#[path = "environments/hosted.rs"]
pub mod hosted;

#[path = "environments/jobs.rs"]
pub mod jobs;

#[path = "environments/list_settings.rs"]
pub mod list_settings;

#[path = "environments/mcp.rs"]
pub mod mcp;

#[path = "environments/moves.rs"]
pub mod moves;

#[path = "environments/network.rs"]
pub mod network;

#[path = "environments/places.rs"]
pub mod places;

#[path = "environments/read_sources.rs"]
pub mod read_sources;

#[path = "environments/reading.rs"]
pub mod reading;

#[path = "environments/retained.rs"]
pub mod retained;

#[path = "environments/sync.rs"]
pub mod sync;

#[path = "environments/updates.rs"]
pub mod updates;
