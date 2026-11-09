//! Environments as resources: creation, lifecycle, configuration changed while one runs, their own secrets, what the exec node offers, the MCP servers found there, their import and one of an environment's own, who reaches into one, its history, update plans, removal, moves, and their jobs.

//!
//! An environment is a resource like the others: it belongs to a person, is listed to everyone who
//! holds something on it, and is shared, changed and removed by the same means. This module serves
//! directories registered through their machine's Use permission, and what
//! every environment has beside: its page's readers, its own settings and its secrets, which are
//! sealed and never read back. Every decision is the access engine's. An environment made from a
//! template on a host needs a host's node and the engine's jobs, which are not served here: reading
//! and listing them works, creating one and its lifecycle do not.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::nodes::{actor_name, invalid, placed, policy_at, view as node_view};
use crate::routes::resources::machines::reported_policy;
use crate::routes::resources::sealing::master_key;
use crate::routes::resources::view::{
    Holding, PreparedAccess, accesses_prepared, machine_refs, parse, prepare_accesses,
};
use crate::routes::resources::{Home, clean_name, record, require};
use crate::sign_in::origin;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Held, ObjectRef};
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName, ResourceKind};
use orch_api_contract::binding::NewEnvironmentAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    self as api, Environment, EnvironmentExec, EnvironmentIndex, EnvironmentRef,
    EnvironmentTemplate, EnvironmentThread, ExecOffer, McpFound,
};
use orch_api_contract::nodes::{JobRef, JobState, NodeKind, NodeRef};
use orch_api_contract::resources::HostRef;
use orch_api_contract::settings as settings_api;
use orch_authz::engine::Graph;
use orch_authz::environments::{Creating, NewEnvironment, Place, check_creation};
use orch_authz::ownership::{self, NewAccess};
use orch_authz::usable::{Policies, ResourcePolicy};
use orch_authz::{Act, Policy, Principal, resources};
use orch_db::audit::Actor;
use orch_db::queries::environments as env_rows;
use orch_db::queries::environments::hosting;
use orch_db::queries::environments::routes::{
    self as queries, Change, EnvironmentFull, NewConnected,
};
use orch_db::queries::jobs as job_rows;
use orch_db::{Conn, DbError};
use orch_settings::secrets::{self, Owner, SecretError};
use orch_settings_contract::registry::{self, Invalid, Scope};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

/// The longest a root directory is written: a path, as long as the longest the platforms take.
const ROOT_MAX_CHARS: usize = 4_096;

mod agent_reach;
mod exec_version;
mod settings_read;
pub(crate) use exec_version::confirmed as confirmed_machine_programs;
pub(crate) mod hosted;
mod incarnation;
pub(crate) mod jobs;
mod lifecycle;
mod list_settings;
mod machine_create;
pub(crate) mod mcp;
pub(crate) mod moves;
pub(crate) mod network;
pub(crate) mod places;
mod reach;
mod reach_operation;
mod read_sources;
pub(crate) mod reading;
pub(crate) mod recovery;
pub(crate) mod retained;
mod support;
pub(crate) mod sync;
mod update_sources;
pub(crate) mod updates;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    crate::routes::resources::environment_read::mount(registry);
    hosted::mount(registry);
    retained::mount(registry);
    jobs::mount(registry);
    places::mount(registry);
    reach::mount(registry);
    agent_reach::mount(registry);
    sync::mount(registry);
    updates::mount(registry);
    moves::mount(registry);
    recovery::mount(registry);
    mcp::mount(registry);
    crate::routes::fleet::mount(registry);
    registry
        .on_with_operation(
            api::MachineEnvironmentCreate,
            crate::key_operations::native::effects::prepare::<machine_create::Create>,
            machine_create::create,
        )
        .creation(api::MachineEnvironmentCreate, machine_create::binding())
        .on_snapshot_read_with_optional_operation(
            api::EnvironmentGet,
            crate::key_operations::native::selected::<read_sources::EnvironmentRead>,
            crate::key_operations::native::selected_read::<read_sources::EnvironmentRead>,
        )
        .on_with_operation(
            api::EnvironmentUpdate,
            crate::key_operations::native::effects::prepare::<update_sources::Update>,
            update,
        )
        .on(api::EnvironmentDisable, disable)
        .on(api::EnvironmentEnable, enable)
        .on(api::EnvironmentDelete, delete)
        .on_snapshot_read_with_optional_operation(
            settings_api::EnvironmentSettingList,
            crate::key_operations::native::selected::<settings_read::Environment>,
            setting_list,
        )
        .on(settings_api::EnvironmentSecretList, secret_list)
        .on(settings_api::EnvironmentSecretSet, secret_set)
        .on_with_operation(
            settings_api::EnvironmentSecretDelete,
            crate::key_operations::direct::declared,
            secret_delete,
        );
}

// ---- the view ----

/// What the caller holds on an environment, as the engine told it.
#[derive(Debug, Clone)]
struct Standing {
    admin_access: Option<orch_access_contract::roles::AdminReach>,
    admin_place: Option<orch_access_contract::roles::PlaceRef>,
    participating: bool,
    admin_only: Vec<Permission>,
    owner: bool,
    administers: bool,
    held: Vec<Held>,
}

impl Standing {
    fn manages(&self) -> bool {
        self.held
            .iter()
            .any(|held| held.permission == Permission::EnvironmentManage)
    }

    /// Whether the caller holds anything on the environment: owning it, administering its place, or a
    /// row of its page. Somebody who does not sees it only through a thread (see [`reading::decide`]).
    fn holds_something(&self) -> bool {
        self.held.iter().any(|held| {
            !matches!(
                held.permission,
                Permission::AccessManage | Permission::AccessExternal
            )
        })
    }
}

/// The names in a list of text that a JSON value is.
fn texts(value: Option<&Json>) -> Option<Vec<String>> {
    value.and_then(Json::as_array).map(|list| {
        list.iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()
    })
}

/// What an environment's exec node offers: the offers it is configured with (or else what its
/// machine's local policy lets it offer), each with whether the machine offers it.
fn exec_of(config: &Json, machine_policy: Option<&Json>) -> EnvironmentExec {
    let machine_offers = machine_policy.and_then(|policy| texts(policy.get("exec.offers")));
    let configured = texts(config.get("exec.offers"));
    let names = configured
        .or_else(|| machine_offers.clone())
        .unwrap_or_default();
    let offers = names
        .into_iter()
        .map(|name| {
            // The same offer however it is written: a tool set alone, as offers were before they named their
            // executor, or with it.
            let available = machine_offers.as_ref().is_none_or(|offered| {
                orch_agent_programs::Programs::builtin().offers_include(offered, &name)
            });
            ExecOffer {
                reason: (!available)
                    .then(|| "the machine's local policy does not let it offer this".to_owned()),
                available,
                name,
            }
        })
        .collect();
    EnvironmentExec {
        offers,
        orch_tools: config
            .get("exec.orch_tools")
            .and_then(Json::as_bool)
            .unwrap_or(true),
        machine_offers,
        machine_programs: Vec::new(),
    }
}

/// Bounded inventory rows keep the existing environment projection and its page's current standings.
pub(crate) async fn inventory(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<Environment>> {
    let prepared = prepare_inventory(ctx, conn, principal, seen).await?;
    show_inventory_prepared(ctx, conn, principal, &prepared).await
}

pub(crate) async fn prepare_inventory(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<PreparedInventory> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::environments(conn, &ids).await?;
    let standing = seen
        .iter()
        .map(|each| {
            (
                each.resource.id,
                Standing {
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    admin_only: each.admin_only.clone(),
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held.clone(),
                },
            )
        })
        .collect();
    prepare_environment_views(conn, rows, standing, principal.now()).await
}

/// Opaque existing Environment view facts; no ordinary permission or operation is selected here.
pub(crate) struct PreparedInventory {
    rows: Vec<EnvironmentFull>,
    standing: HashMap<Id, Standing>,
    access: PreparedAccess,
    sources: NameSources,
    bounds: NameBounds,
    machine_facts: HashMap<Id, orch_db::queries::environment_facts::EnvironmentMachine>,
    hosted: Vec<(Id, Id, String)>,
    defaults: Vec<(Id, Id)>,
    threads: Vec<env_rows::ThreadInEnvironment>,
    runtime_rows: Vec<hosting::RuntimeFull>,
    templates: Vec<Id>,
    template_names: Vec<(Id, String, String)>,
    hosts: Vec<Id>,
    physical_machines: Vec<Id>,
    display_settings: list_settings::DisplaySettings,
    mounts: HashMap<Id, orch_engine::mounts::PreparedViews>,
    mount_sources: Vec<Id>,
}
impl PreparedInventory {
    /// Native FK/reference associations used by this exact prepared Environment body.
    pub(crate) fn preserves_sources(&self, fresh: &Self) -> bool {
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

    pub(crate) fn selected_ids(&self) -> Vec<Id> {
        self.rows.iter().map(|row| row.id).collect()
    }
    /// The native through-thread style has no Host/Template/Machine/Mount disclosure.
    pub(crate) async fn content_sources(
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
    pub(crate) async fn place_names(&self, conn: &mut Conn) -> ApiResult<NameSources> {
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
    pub(crate) fn name_sources(&self) -> &NameSources {
        &self.sources
    }
    pub(crate) fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
    pub(crate) fn template_ids(&self) -> &[Id] {
        &self.templates
    }
    pub(crate) fn host_ids(&self) -> &[Id] {
        &self.hosts
    }
    /// Native selected attachments, prepared before any Environment response is rendered.
    pub(crate) fn mount_ids(&self) -> &[Id] {
        &self.mount_sources
    }
    /// Physical policy/capacity source from the ordinary environment_facts supplier.
    pub(crate) fn physical_machine_ids(&self) -> &[Id] {
        &self.physical_machines
    }
}
fn inventory_holdings<'a>(
    rows: &'a [EnvironmentFull],
    standing: &'a HashMap<Id, Standing>,
) -> Vec<Holding<'a>> {
    rows.iter()
        .filter_map(|row| {
            standing.get(&row.id).map(|each| Holding {
                admin_only: &each.admin_only,
                admin_access: each.admin_access,
                admin_place: each.admin_place,
                participating: each.participating,
                id: row.id,
                owner: each.owner,
                administers: each.administers,
                held: &each.held,
            })
        })
        .collect()
}
async fn prepare_environment_views(
    conn: &mut Conn,
    rows: Vec<EnvironmentFull>,
    standing: HashMap<Id, Standing>,
    now: Time,
) -> ApiResult<PreparedInventory> {
    let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
    let machine_facts: HashMap<_, _> = orch_db::queries::environment_facts::of_many(conn, &ids)
        .await?
        .into_iter()
        .collect();
    let hosted = hosting::hosts_of(conn, &ids).await?;
    let defaults = queries::default_for(conn, &ids).await?;
    let threads = env_rows::threads_in(conn, &ids).await?;
    let runtime_rows = hosting::live_runtimes_of(conn, &ids, &[]).await?;
    let access = prepare_accesses(conn, &inventory_holdings(&rows, &standing)).await?;
    let mut sources = access.names.clone();
    sources
        .nodes
        .extend(rows.iter().map(|row| row.home_node_id));
    sources.nodes.extend(defaults.iter().map(|(_, node)| *node));
    sources.nodes.extend(
        threads
            .iter()
            .flat_map(|thread| [thread.thread_id, thread.topic_id]),
    );
    let address_ids = ids
        .iter()
        .copied()
        .chain(hosted.iter().map(|(_, id, _)| *id))
        .collect::<Vec<_>>();
    sources.nodes.extend(
        orch_authz::resources::address_name_sources(conn, &address_ids)
            .await
            .map_err(ApiError::internal)?,
    );

    let distinct = |ids: Vec<Id>| {
        ids.into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let template_ids: Vec<_> = rows.iter().filter_map(|row| row.template_id).collect();
    let template_names = queries::template_names(conn, &template_ids).await?;
    let templates = distinct(template_names.iter().map(|(id, _, _)| *id).collect());
    let hosts = distinct(hosted.iter().map(|(_, host, _)| *host).collect());
    let physical_machines = distinct(
        machine_facts
            .values()
            .filter_map(|facts| facts.machine_id)
            .collect(),
    );
    let display_settings = list_settings::load(conn, &rows).await?;
    let mut mounts = HashMap::new();
    for row in &rows {
        if standing.get(&row.id).is_some_and(Standing::holds_something)
            && display_settings.has_mounts(row.id)
        {
            mounts.insert(
                row.id,
                orch_engine::mounts::prepare_views(conn, row.id, now.get())
                    .await
                    .map_err(ApiError::engine)?,
            );
        }
    }
    let mount_sources = distinct(
        mounts
            .values()
            .flat_map(|mounts| mounts.mount_ids())
            .collect(),
    );
    Ok(PreparedInventory {
        rows,
        standing,
        access,
        sources,
        bounds: NameBounds::default(),
        machine_facts,
        hosted,
        defaults,
        threads,
        runtime_rows,
        templates,
        template_names,
        hosts,
        physical_machines,
        display_settings,
        mounts,
        mount_sources,
    })
}

/// Render the native prepared rows, preserving whole and through-thread cuts.
pub(crate) async fn show_inventory_prepared(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedInventory,
) -> ApiResult<Vec<Environment>> {
    let rows = prepared.rows.clone();
    let standing = &prepared.standing;
    let machine_facts = &prepared.machine_facts;
    let hosted = &prepared.hosted;
    let defaults = &prepared.defaults;
    let threads = &prepared.threads;
    let runtime_rows = &prepared.runtime_rows;
    let bounds = prepared.bounds.limited.then_some(&prepared.bounds);
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let activity = crate::routes::resources::view::environment_activity(conn, &ids).await?;
    let display_settings = &prepared.display_settings;
    let lifetimes: HashMap<_, _> = orch_db::queries::environments::temporary::among(conn, &ids)
        .await?
        .into_iter()
        .map(|row| (row.environment_id, row))
        .collect();
    let exposed = crate::routes::ports::exposures(conn, &ids, Time::new(ctx.now)).await?;
    let addresses =
        crate::routes::resources::view::addresses_with_names(conn, &ids, bounds).await?;
    let people: Vec<Id> = rows
        .iter()
        .map(|row| row.owner_id)
        .chain(rows.iter().flat_map(|row| {
            exec_version::stored(&row.config)
                .into_values()
                .map(|entry| entry.by)
        }))
        .collect();
    let people = refusals::user_refs(conn, &people).await?;
    let machine_ids: Vec<Id> = machine_facts
        .values()
        .filter_map(|machine| machine.machine_id)
        .chain(rows.iter().filter_map(|row| row.machine_id))
        .collect();
    let machines = machine_refs(&ctx.app, conn, &machine_ids).await?;
    let connector_updates =
        crate::component_updates::views(&ctx.app.link, conn, &machine_ids).await?;
    // Runtime updates compare the reported connector with the release's accepted minimum.
    let connector_versions: HashMap<Id, String> =
        env_rows::changes::connector_versions(conn, &machine_ids)
            .await?
            .into_iter()
            .collect();
    let program_facts: HashMap<Id, Json> = queries::machine_program_facts(conn, &machine_ids)
        .await?
        .into_iter()
        .collect();
    let templates = &prepared.template_names;
    // What a managed one has beyond a directory: its host, the job under way, why it is to be removed.
    let loss_eligible = orch_db::queries::loss::eligible_environments(conn, &ids)
        .await?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let host_ids: Vec<Id> = hosted.iter().map(|(_, host, _)| *host).collect();
    let host_addresses =
        crate::routes::resources::view::addresses_with_names(conn, &host_ids, bounds).await?;
    let open_jobs = job_rows::open_for(conn, &ids, &job_rows::kind::ONE_AT_A_TIME).await?;
    let reasons = hosting::removal_reasons(conn, &ids).await?;
    // Each one's network, as its host's node last reported it.
    let connected: Vec<Id> = rows
        .iter()
        .filter(|row| {
            machine_facts
                .get(&row.id)
                .is_some_and(|facts| facts.attaches())
        })
        .map(|row| row.id)
        .collect();
    let networks = network::of_environments(conn, principal, &ids, &connected).await?;
    // Each harness's own configuration in its workspace, and the answer given for it.
    let mut config_trust = crate::routes::trust::views_of(conn, &ids).await?;
    let thread_owners: Vec<_> = threads.iter().map(|thread| thread.owner_id).collect();
    let thread_owners = refusals::user_refs(conn, &thread_owners).await?;
    let reach = orch_authz::environments::reach_counts(conn, principal, &ids, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let mut nodes: Vec<Id> = rows.iter().map(|row| row.home_node_id).collect();
    nodes.extend(defaults.iter().map(|(_, topic)| *topic));
    for thread in threads {
        nodes.extend([thread.thread_id, thread.topic_id]);
    }
    nodes.sort_unstable();
    nodes.dedup();
    let refs = match bounds {
        Some(bounds) => bounds.refs(conn, principal, &nodes).await?,
        None => node_view::refs(conn, principal, &nodes, Policy::default()).await?,
    };
    let known: BTreeSet<Id> = refs
        .iter()
        .filter(|(_, node)| !node.redacted)
        .map(|(id, _)| *id)
        .collect();
    let holdings = inventory_holdings(&rows, standing);
    let access = accesses_prepared(conn, principal, &holdings, &prepared.access, bounds).await?;
    let mut runtime_updates = ctx
        .app
        .engine
        .pending_runtime_updates(conn, &runtime_rows.iter().map(|r| r.id).collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let mut views = Vec::with_capacity(rows.len());
    for mut row in rows {
        if let Some(descriptor) = machine_facts
            .get(&row.id)
            .and_then(|machine| machine.descriptor.as_ref())
        {
            if !row.facts.is_object() {
                row.facts = json!({});
            }
            row.facts["environment_descriptor"] = descriptor.clone();
        }
        if let Some(policy) = machine_facts
            .get(&row.id)
            .and_then(|machine| machine.policy.as_ref())
        {
            if !row.facts.is_object() {
                row.facts = json!({});
            }
            row.facts["machine_policy"] = json!({"environment.after_up":policy.get("environment.after_up").and_then(Json::as_bool)});
        }
        let manages = standing.get(&row.id).is_some_and(Standing::manages);
        let owner = people
            .get(&row.owner_id)
            .cloned()
            .ok_or_else(|| ApiError::internal("the owner of an environment is not there"))?;
        let seen = |id: Id| refs.get(&id).filter(|each| !each.redacted).cloned();
        let mut shown_threads: Vec<EnvironmentThread> = Vec::new();
        let mut hidden = 0_i64;
        for thread in threads
            .iter()
            .filter(|thread| thread.environment_id == row.id)
        {
            if !bound_thread_visible(thread, &known) {
                hidden += 1;
                continue;
            }
            match (seen(thread.thread_id), seen(thread.topic_id)) {
                (Some(node), Some(topic)) => {
                    shown_threads.push(EnvironmentThread {
                        thread: node,
                        topic,
                        directory: thread.workspace_dir.clone(),
                        owner: thread_owners
                            .get(&thread.owner_id)
                            .cloned()
                            .ok_or_else(|| {
                                ApiError::internal("the owner of a thread is not there")
                            })?,
                    });
                }
                _ => hidden += 1,
            }
        }
        let template = row.template_id.and_then(|template| {
            templates
                .iter()
                .find(|(id, _, _)| *id == template)
                .map(|(id, name, driver)| EnvironmentTemplate {
                    id: *id,
                    name: name.clone(),
                    driver: driver.clone(),
                })
        });
        let mcp_found: Vec<McpFound> = row
            .facts
            .get("mcp_found")
            .and_then(Json::as_array)
            .map(|found| found.iter().filter_map(mcp::found_view).collect())
            .unwrap_or_default();
        let whole = standing.get(&row.id).is_some_and(Standing::holds_something);
        let ceiling = machine_facts
            .get(&row.id)
            .and_then(|m| m.policy.clone())
            .and_then(|p| serde_json::from_value::<orch_link_contract::policy::LocalPolicy>(p).ok())
            .unwrap_or_default();
        let switches = ceiling.environment_switches(
            Some(row.remote_write),
            Some(row.terminal),
            Some(row.browser),
            Some(row.vnc),
        );
        let view = Environment {
            provenance: orch_api_contract::resource_groups::ResourceProvenance {
                origin_machine: machine_facts
                    .get(&row.id)
                    .and_then(|machine| machine.machine_id)
                    .and_then(|id| machines.get(&id).cloned()),
                availability: Vec::new(),
            },
            sandbox_readiness: None,
            can_declare_lost: manages && loss_eligible.contains(&row.id),
            temporary: lifetimes
                .get(&row.id)
                .map(|lifetime| api::EnvironmentTemporary {
                    expires_at: if lifetime.active {
                        None
                    } else {
                        Some(Time::new(lifetime.expires_at))
                    },
                    idle_hours: lifetime.idle_hours,
                    on_archive: orch_db::queries::environments::temporary::mode(&row.config)
                        == Some(true),
                }),
            runtime_changes: None,
            isolation: None,
            base: EnvironmentRef {
                activity: activity.get(&row.id).cloned(),
                ports_exposed: exposed.contains(&row.id),
                address: addresses.get(&row.id).cloned().unwrap_or_default(),
                id: row.id,
                name: row.name.clone(),
                owner,
                placement: parse(&row.placement)?,
                status: parse(&row.status)?,
            },
            org_id: row.org_id,
            machine: machine_facts
                .get(&row.id)
                .and_then(|machine| machine.machine_id)
                .and_then(|machine| machines.get(&machine).cloned()),
            network: networks.get(&row.id).cloned().unwrap_or_else(|| {
                network::state_of(
                    None,
                    machine_facts.get(&row.id).is_some_and(|m| m.attaches()),
                )
            }),
            home: refs.get(&row.home_node_id).cloned().unwrap_or(NodeRef {
                thread_display: None,
                id: row.home_node_id,
                kind: NodeKind::Org,
                name: None,
                redacted: true,
            }),
            config_trust: config_trust.remove(&row.id).unwrap_or_default(),
            default_for: defaults
                .iter()
                .filter(|(environment, _)| *environment == row.id)
                .filter_map(|(_, topic)| seen(*topic))
                .collect(),
            threads: shown_threads,
            threads_hidden: hidden,
            reach: reach.get(&row.id).copied().unwrap_or_default(),
            workspace_incarnation: Some(incarnation::of(
                crate::routes::resources::sealing::installed_master_key()?,
                &row,
            )?),
            data_sources: row
                .config
                .get(orch_settings_contract::registry::keys::ENV_DATA_SOURCES)
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(ApiError::internal)?,
            workspace_root: row.workspace_root.clone(),
            workspace_storage: None,
            config: if manages {
                row.config.clone()
            } else {
                json!({})
            },
            host: hosted
                .iter()
                .find(|(environment, _, _)| *environment == row.id)
                .map(|(_, host, name)| HostRef {
                    id: *host,
                    name: name.clone(),
                    address: host_addresses.get(host).cloned().unwrap_or_default(),
                }),
            mounts: if whole {
                prepared
                    .mounts
                    .get(&row.id)
                    .map(|mounts| mounts.render())
                    .transpose()
                    .map_err(ApiError::engine)?
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            exec: {
                let reported = machine_facts
                    .get(&row.id)
                    .and_then(|machine| machine.machine_id)
                    .or(row.machine_id)
                    .and_then(|machine| program_facts.get(&machine));
                let mut exec = exec_of(
                    &row.config,
                    reported.and_then(orch_db::queries::environment_facts::policy_in),
                );
                if row.placement == "connected"
                    && row.template_id.is_none()
                    && let Some(reported) = reported
                {
                    exec.machine_programs = exec_version::view(&row.config, reported, &people);
                }
                exec
            },
            mcp_found,
            index: row
                .facts
                .get("index")
                .map(|index| serde_json::from_value(index.clone()))
                .transpose()
                .map_err(ApiError::internal)?
                .unwrap_or(EnvironmentIndex::Full),
            index_reason: row
                .facts
                .get("index_reason")
                .map(|reason| serde_json::from_value(reason.clone()))
                .transpose()
                .map_err(ApiError::internal)?
                .flatten(),
            template,
            generation: i64::from(row.generation),
            // Runtime updates are noticed; they never replace a unit during a read.
            updates: runtime_rows
                .iter()
                .find(|runtime| runtime.environment_id == Some(row.id))
                .and_then(|runtime| runtime_updates.remove(&runtime.id))
                .unwrap_or_default(),
            connector_update: row
                .machine_id
                .and_then(|machine| connector_updates.get(&machine).cloned()),
            update_required: orch_engine::updates::update_required(
                row.machine_id
                    .and_then(|machine| connector_versions.get(&machine))
                    .map(String::as_str),
                orch_engine::updates::CONNECTOR_MINIMUM,
            ),
            remote_write: switches.remote_write,
            terminal: switches.terminal,
            browser: switches.browser,
            vnc: switches.vnc,
            support: support::of(&row.facts),
            granted_capabilities: row.granted_capabilities.clone(),
            status_detail: row.status_detail.clone(),
            facts: row.facts.clone(),
            manifest_at: row.manifest_at.map(Time::new),
            last_activity_at: row.last_activity_at.map(Time::new),
            idle_stop_at: display_settings
                .idle_stop(
                    &row,
                    machine_facts
                        .get(&row.id)
                        .is_some_and(|machine| machine.makes_units()),
                )?
                .map(Time::new),
            unused_since: row.unused_since.map(Time::new),
            removal: if row.remove_at.is_some() || row.status == "removed" {
                retained::present(ctx, conn, principal, row.id).await?
            } else {
                // A live unscheduled environment has no deletion promise to read.
                None
            },
            inspection: None,
            remove_at: row.remove_at.map(Time::new),
            remove_reason: row.remove_at.map(|_| {
                match reasons
                    .iter()
                    .find(|(environment, _)| *environment == row.id)
                    .map(|(_, word)| word.as_str())
                {
                    Some("unused") => api::EnvironmentRemoveReason::Unused,
                    Some("switch") => api::EnvironmentRemoveReason::Switch,
                    _ => api::EnvironmentRemoveReason::Request,
                }
            }),
            job: open_jobs
                .iter()
                .find(|job| job.environment_id == Some(row.id))
                .map(job_ref),
            seq: row.env_seq,
            revision: row.revision,
            access: access.get(&row.id).cloned().ok_or_else(|| {
                ApiError::internal("what the caller holds on an environment was not read")
            })?,
        };
        // Somebody who holds nothing on it and sees it through a thread is shown as much as that is.
        views.push(if whole {
            view
        } else {
            reading::through_a_thread(view)
        });
    }
    Ok(views)
}

/// The existing Environment view admits a bound Thread line only when both its
/// Thread and Topic references are known to this ordinary account/credential.
pub(super) fn bound_thread_visible(
    thread: &env_rows::ThreadInEnvironment,
    known: &BTreeSet<Id>,
) -> bool {
    known.contains(&thread.thread_id) && known.contains(&thread.topic_id)
}

/// One environment as the caller sees it: all of it where they hold something on it, and what a
/// thread lets them see where they hold nothing. The caller was let read it by [`reading::decide`],
/// or is the one who just made or changed it.
pub(crate) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Environment> {
    let prepared = prepare_shown(conn, principal, id).await?;
    let view = show_inventory_prepared(ctx, conn, principal, &prepared)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("an environment was read and is not shown"))?;
    finish_shown(ctx, conn, principal, id, view).await
}

/// Stored individual-view operands before the native formatter or runtime additions run.
async fn prepare_shown(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<PreparedInventory> {
    let row = queries::environments(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    prepare_shown_rows(conn, principal, vec![row]).await
}

/// The same native formatter inputs for actual stored snapshots of completed fleet decisions.
/// Their source owner separately verifies current custody and the ordinary Read door.
pub(crate) async fn prepare_shown_rows(
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<EnvironmentFull>,
) -> ApiResult<PreparedInventory> {
    let mut standing = HashMap::new();
    for row in &rows {
        let object = ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: row.id,
        };
        let effective = resources::effective(conn, principal, object, Policy::default())
            .await
            .map_err(ApiError::internal)?;
        standing.insert(
            row.id,
            Standing {
                admin_access: effective.admin_access,
                admin_place: effective.admin_place,
                participating: effective.participating,
                admin_only: effective.admin_only,
                owner: effective.owner,
                administers: effective.administers,
                held: effective.held,
            },
        );
    }
    prepare_environment_views(conn, rows, standing, principal.now()).await
}

/// The native individual Environment projection's ordinary whole-view additions.
/// Inventory preparation does not lend a Get operation or add Environment View.
pub(crate) async fn finish_shown(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    mut view: Environment,
) -> ApiResult<Environment> {
    if reading::decide(conn, principal, id).await? == reading::Reading::Whole {
        view.sandbox_readiness = Some(
            crate::sandbox_readiness::environment(
                conn,
                principal,
                view.base.clone(),
                None,
                None,
                None,
            )
            .await?,
        );
        view.runtime_changes = ctx
            .app
            .engine
            .runtime_changes_from(conn, orch_engine::updates::Target::Environment(id))
            .await
            .map_err(ApiError::internal)?;
        // What it runs on and under, from its host's declaration: environments made before runtimes were declared
        // show it too, with no stored record of it.
        view.isolation = ctx
            .app
            .engine
            .environment_capabilities(conn, id)
            .await
            .map_err(ApiError::internal)?
            .and_then(|capabilities| capabilities.isolation);
    }
    Ok(view)
}

pub(crate) use places::host_choices;

/// The environments among `ids` that the caller holds something on, as they see them, with what the
/// caller holds on each.
async fn viewed_among(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<(Vec<Environment>, HashMap<Id, Standing>)> {
    let (prepared, standing) = prepare_viewed_among(conn, principal, ids).await?;
    let views = show_inventory_prepared(ctx, conn, principal, &prepared).await?;
    Ok((views, standing))
}

/// The same native whole/through-thread list cut, before rendering.
pub(crate) async fn prepare_viewed(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<PreparedInventory> {
    Ok(prepare_viewed_among(conn, principal, ids).await?.0)
}

/// The same native whole/through-thread selection, before public references are formatted.
async fn prepare_viewed_among(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<(PreparedInventory, HashMap<Id, Standing>)> {
    let seen = reading::among(conn, principal, ids).await?;
    let standing: HashMap<Id, Standing> = seen
        .into_iter()
        .map(|(id, each)| {
            (
                id,
                Standing {
                    admin_access: if each.reading == reading::Reading::Whole {
                        each.effective.admin_access
                    } else {
                        None
                    },
                    admin_place: if each.reading == reading::Reading::Whole {
                        each.effective.admin_place
                    } else {
                        None
                    },
                    participating: each.reading == reading::Reading::Whole
                        && each.effective.participating,
                    admin_only: if each.reading == reading::Reading::Whole {
                        each.effective.admin_only
                    } else {
                        Vec::new()
                    },
                    owner: each.reading == reading::Reading::Whole && each.effective.owner,
                    administers: each.reading == reading::Reading::Whole
                        && each.effective.administers,
                    held: each.effective.held,
                },
            )
        })
        .collect();
    let wanted: Vec<Id> = standing.keys().copied().collect();
    let rows = queries::environments(conn, &wanted).await?;
    let prepared = prepare_environment_views(conn, rows, standing.clone(), principal.now()).await?;
    Ok((prepared, standing))
}

/// The environments among `ids` that the caller holds something on, as they see them.
pub(crate) async fn environments_of(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<Vec<Environment>> {
    Ok(viewed_among(ctx, conn, principal, ids).await?.0)
}

/// A job in the few words an environment carries it in.
fn job_ref(job: &job_rows::JobRow) -> JobRef {
    let (progress, _) = orch_engine::jobs::runner::read_progress(job);
    JobRef {
        id: job.id,
        kind: job.kind.clone(),
        state: match job.state.as_str() {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "succeeded" => JobState::Succeeded,
            "cancelled" => JobState::Cancelled,
            "lost" => JobState::Lost,
            _ => JobState::Failed,
        },
        phase: progress.phase,
        percent: progress.percent,
    }
}

/// The home node and its root org, for a resource operation.
fn home_of(row: &EnvironmentFull) -> Home {
    Home {
        node: Some(row.home_node_id),
        org: Some(row.org_id),
    }
}

// ---- registering a directory of a machine ----

/// A root directory as the machine's platform writes an absolute path.
fn clean_root(path: &str, root: &str, windows: bool) -> ApiResult<String> {
    let root = root.trim();
    let bytes = root.as_bytes();
    let absolute = if windows {
        root.starts_with("\\\\")
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/'))
    } else {
        root.starts_with('/')
    };
    if !absolute || root.chars().count() > ROOT_MAX_CHARS || root.chars().any(char::is_control) {
        return Err(invalid(path, "this is the absolute path of a directory"));
    }
    if root.split(['/', '\\']).any(|part| part == "..") {
        return Err(invalid(path, "a path does not climb with `..`"));
    }
    Ok(root.to_owned())
}

/// Whether `root` lies inside one of the directories the machine's local policy lets an
/// environment be in. A machine that has not reported a policy, or that lists none, is asked: it
/// enforces its roots whatever the system believes.
fn within_roots(root: &str, roots: &[String]) -> bool {
    roots.is_empty()
        || roots.iter().any(|allowed| {
            let allowed = allowed.trim_end_matches(['/', '\\']);
            root == allowed
                || root
                    .strip_prefix(allowed)
                    .is_some_and(|rest| rest.starts_with(['/', '\\']))
        })
}

/// The answer to a name that one of the caller's environments of that org has already.
fn taken(existing: Option<Id>) -> ApiError {
    let error = ApiError::new(
        ErrorCode::ConflictExists,
        "you have an environment by that name already",
    );
    match existing {
        Some(existing) => error.with("existing_id", existing),
        None => error,
    }
}

// ---- listing and reading ----

// ---- changing one ----

/// The values an environment has of its own: only keys that are set on an environment, each with
/// a value that fits it.
fn clean_config(path: &str, config: &Json) -> ApiResult<()> {
    let sources = orch_driver_contract::runtime::EnvironmentDataSources::from_config(config)
        .map_err(|error| invalid(path, &error.to_string()))?;
    for directory in [sources.workspace.directory(), sources.home.directory()]
        .into_iter()
        .flatten()
    {
        if directory.trim().is_empty() || !std::path::Path::new(directory).is_absolute() {
            return Err(invalid(
                "data_sources",
                "an existing directory needs its absolute path on the selected machine",
            ));
        }
    }
    let Some(values) = config.as_object() else {
        return Err(invalid(path, "the environment's own values are an object"));
    };
    for (key, value) in values {
        match registry::validate_at(key, Scope::Environment, value) {
            Ok(_) => {}
            Err(Invalid::UnknownKey(_)) => {
                return Err(invalid(&format!("{path}.{key}"), "this is no setting"));
            }
            Err(Invalid::WrongScope { .. }) => {
                return Err(invalid(
                    &format!("{path}.{key}"),
                    "this is not set on an environment",
                ));
            }
            Err(Invalid::Value { faults, .. }) => {
                let message = faults.first().map_or_else(
                    || "this value does not fit".to_owned(),
                    |fault| fault.message.clone(),
                );
                return Err(invalid(&format!("{path}.{key}"), &message));
            }
        }
    }
    Ok(())
}

/// PATCH accepts an unstamped decision, then prepares it under the environment's Manage guard.
/// Every other value is checked here, and the complete stored config is checked after preparation.
fn clean_patch_config(path: &str, config: &Json) -> ApiResult<()> {
    let mut ordinary = config.clone();
    if let Some(values) = ordinary.as_object_mut() {
        values.remove(registry::keys::EXEC_VERSION_OVERRIDES);
    }
    clean_config(path, &ordinary)
}

/// A native stale revision keeps current content only when this operation can read it.
fn stale_current(current: Option<Environment>) -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the environment was changed since it was read",
    )
    .with("current", current)
}

/// Changes an environment: its name, its own values, whether files may be written from outside
/// and shells opened, and the node it takes its settings from. Who may was asked of the engine:
/// `environment.manage` on it, and `node.interact` on another home.
async fn update(
    ctx: Ctx,
    (): (),
    _body: api::EnvironmentUpdateBody,
) -> ApiResult<api::EnvironmentUpdateResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done =
        crate::key_operations::native::effects::apply::<update_sources::Update>(&ctx, &mut tx)
            .await?;
    tx.commit().await.map_err(DbError::from)?;
    let conflicted = done.output.stale;
    let receipt = api::EnvironmentUpdateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: done.output.environment,
        revision: done.output.revision,
        changed: done.output.changed,
    };
    if !conflicted {
        crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    }
    let id = done.output.environment;
    // Reconciliation consumes the committed native settings; it is not another submitted act.
    if done.output.version_consent_changed
        && let Err(error) = done
            .ctx
            .app
            .engine
            .reevaluate_bindings(None, Some(id))
            .await
    {
        tracing::warn!(environment=%id,%error,"machine program consent is saved; blocked bindings await their next recheck");
    }
    if done.output.reconcile {
        if let Err(error) = done.ctx.app.engine.apply_environment_mounts(id).await {
            tracing::warn!(environment=%id,%error,"environment mount changes remain pending");
        }
        if let Err(error) = done.ctx.app.engine.apply_environment_resources(id).await {
            tracing::warn!(environment=%id,%error,"environment resource changes remain pending");
        }
    }
    let shown =
        crate::key_operations::native::resource_reply::with_input::<update_sources::Update>(
            &done.ctx,
            &done.accepted,
            done.output,
        )
        .await?;
    let current = match shown {
        Some(orch_api_contract::resources::Resource::Environment(value)) => Some(value),
        Some(_) => {
            return Err(ApiError::internal(
                "the native Environment update projection changed kind",
            ));
        }
        None => None,
    };
    if conflicted {
        return Err(stale_current(current));
    }
    Ok(match current {
        Some(value) => api::EnvironmentUpdateResponse::Full(Box::new(value)),
        None => api::EnvironmentUpdateResponse::Receipt(receipt),
    })
}

/// Switches an environment off.
async fn disable(
    ctx: Ctx,
    query: orch_api_contract::resource_facts::ResourcePauseQuery,
    (): (),
) -> ApiResult<Environment> {
    let note = crate::routes::resources::impact::note(query.note.as_deref())?;
    switch(&ctx, true, note, false).await
}

/// Switches an environment on again.
async fn enable(
    ctx: Ctx,
    query: orch_api_contract::resource_facts::ResourceResumeQuery,
    (): (),
) -> ApiResult<Environment> {
    switch(&ctx, false, None, query.run_waiting.unwrap_or(false)).await
}

async fn switch(
    ctx: &Ctx,
    off: bool,
    note: Option<&str>,
    run_waiting: bool,
) -> ApiResult<Environment> {
    let id = ctx.id("id")?;
    let (current, mut tx, principal) = crate::routes::nodes::action(ctx).await?;
    let ctx = &current;
    let row = queries::environments(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let lost: bool = sqlx::query_scalar("SELECT lost_at IS NOT NULL FROM environments WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::from)?;
    if lost {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "A lost environment needs a reviewed recovery plan",
        ));
    }
    let mut stopping = None;
    let mut enable_target = None;
    let mut resumed = Vec::new();
    if off != (row.status == "disabled") {
        let target = orch_api_contract::resource_facts::FactTarget {
            object: orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id,
            },
            model_id: None,
        };
        if off {
            stopping = Some(
                ctx.app
                    .engine
                    .stop_resource_in(
                        &mut tx,
                        orch_engine::resource_pause::ResourceStop {
                            target,
                            name: &row.name,
                            by: principal.user(),
                            note,
                            deleted: false,
                        },
                        ctx.now,
                    )
                    .await
                    .map_err(ApiError::internal)?,
            );
        } else {
            if orch_engine::resource_facts::activity::stopping(&mut tx, &target)
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::PlanBlocked,
                    "Waiting for the accepted stop to finish",
                ));
            }
            enable_target = Some(target);
        }
        queries::set_disabled(&mut tx, id, off, ctx.now).await?;
        sqlx::query("UPDATE environments SET status_detail=$2 WHERE id=$1")
            .bind(id)
            .bind(if off { note } else { None })
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        if let Some(target) = enable_target {
            let finished = ctx
                .app
                .engine
                .enable_resource_waiting_in(
                    &mut tx,
                    orch_engine::resource_pause::ResourceResume {
                        target: &target,
                        name: &row.name,
                        by: principal.user(),
                        run_waiting,
                    },
                    ctx.now,
                )
                .await
                .map_err(ApiError::internal)?;
            resumed.extend(finished.wakes);
        }
        let who = actor_name(ctx)?;
        let (action, word) = if off {
            (
                orch_db::audit::action!(AuditAction::EnvironmentDisabled),
                "switched off",
            )
        } else {
            (
                orch_db::audit::action!(AuditAction::EnvironmentEnabled),
                "switched on",
            )
        };
        record(
            ctx,
            &mut tx,
            action,
            id,
            home_of(&row),
            &format!("{who} {word} the environment {}", row.name),
            json!({ "environment": id }),
        )
        .await?;
    }
    let shown = show(ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    for thread in resumed {
        ctx.app.engine.wake(thread);
    }
    if let Some((job, finished)) = stopping {
        for thread in finished.wakes {
            ctx.app.engine.wake(thread);
        }
        ctx.app
            .engine
            .stop_resource_now(job.id)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(shown)
}

/// Settles current bindings before confirmed removal. Of a directory on a person's machine only
/// the registration goes, at once; one made from a template has its removal dated for the reaper.
async fn delete(ctx: Ctx, query: api::EnvironmentDeleteQuery, (): ()) -> ApiResult<Environment> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = queries::environments(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if query.confirm != row.name {
        return Err(invalid(
            "confirm",
            "the name of the environment, to be sure",
        ));
    }
    let settled = ctx
        .app
        .engine
        .remove_bindings_in(&mut tx, id, principal.user(), principal.now())
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    // The environment as the reply shows it is read before the registration goes.
    let mut shown = show(&ctx, &mut tx, &principal, id).await?;
    let ended = orch_engine::environments::ending::apply(
        &ctx.app.engine,
        &mut tx,
        &row,
        principal.user(),
        query.at,
        principal.now(),
    )
    .await
    .map_err(ending_error)?;
    match ended {
        orch_engine::environments::ending::Disposition::Lost => {
            shown.base.status = api::EnvironmentStatus::Removed;
            shown.remove_at = Some(Time::new(ctx.now));
            record(
                &ctx,
                &mut tx,
                AuditAction::EnvironmentDeleted,
                id,
                home_of(&row),
                "Removed a lost environment registration",
                json!({"environment":id,"threads":settled.threads,"turns":settled.turns}),
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
            ctx.app.engine.kick();
            return Ok(shown);
        }
        orch_engine::environments::ending::Disposition::Connected { purge_at } => {
            shown.base.status = api::EnvironmentStatus::Removed;
            shown.remove_at = Some(Time::new(ctx.now));
            record(
                &ctx,
                &mut tx,
                AuditAction::EnvironmentDeleted,
                id,
                home_of(&row),
                &format!(
                    "{who} removed the registration of the environment {}",
                    row.name
                ),
                json!({"environment":id,"name":row.name,"purge_at":purge_at}),
            )
            .await?;
        }
        orch_engine::environments::ending::Disposition::Scheduled {
            at,
            accepted,
            removal: saved,
        } => {
            shown.remove_at = Some(Time::new(at));
            if accepted {
                record(&ctx,&mut tx,orch_db::audit::action!(AuditAction::EnvironmentRemovalScheduled),id,home_of(&row),&format!("{who} scheduled the removal of the environment {}",row.name),json!({"environment":id,"at":at,"removal_revision":saved.removal_revision,"keep_days":saved.removed_keep_days,"threads":settled.threads,"turns":settled.turns})).await?;
            }
        }
    }
    shown.removal = retained::present(&ctx, &mut tx, &principal, id).await?;
    if let Some(job) = shown
        .removal
        .as_ref()
        .and_then(|removal| removal.job.clone())
    {
        shown.job = Some(job);
    }
    shown.remove_reason = Some(api::EnvironmentRemoveReason::Request);
    orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}

pub(crate) fn ending_error(error: orch_engine::environments::ending::Error) -> ApiError {
    use orch_engine::environments::ending::Error;
    match error {
        Error::Native(error) => ApiError::internal(error),
        Error::Retention(error) => invalid("retention", &error.to_string()),
        Error::Schedule(orch_engine::archive::RemovalScheduleError::Internal(error)) => {
            invalid("retention", &error.to_string())
        }
        Error::Schedule(error) => removal_schedule_error(error),
        Error::Gone => ApiError::not_found(),
    }
}

/// Turns a cleanup-plan conflict into the job a caller must wait for.
pub(crate) fn removal_schedule_error(
    error: orch_engine::archive::RemovalScheduleError,
) -> ApiError {
    match error {
        orch_engine::archive::RemovalScheduleError::Busy(job) => ApiError::new(
            ErrorCode::PlanBlocked,
            "another job must finish before the environment can be removed",
        )
        .with("job", orch_engine::locks::job_ref(&job)),
        orch_engine::archive::RemovalScheduleError::Gone => ApiError::not_found(),
        orch_engine::archive::RemovalScheduleError::Internal(error) => ApiError::engine(error),
    }
}

// ---- its own settings and secrets ----

/// Every key valid on the environment, as it holds there: what its home resolves, then its own.
async fn setting_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<settings_api::ResolvedSetting>> {
    crate::key_operations::native::selected_read::<settings_read::Environment>(ctx, (), ()).await
}

fn secret_failed(error: SecretError) -> ApiError {
    match error {
        SecretError::Db(error) => error.into(),
        SecretError::NotFound | SecretError::NoSuchSecret => ApiError::not_found(),
        SecretError::BadName => invalid(
            "name",
            "is capitals, digits and underscores, beginning with no digit, at most 64 characters",
        ),
        SecretError::Empty => invalid("value", "must not be empty"),
        SecretError::TooLarge { max } => {
            ApiError::new(ErrorCode::RequestTooLarge, "the value is too large").with("limit", max)
        }
        other => ApiError::internal(other),
    }
}

fn secret_info(info: secrets::SecretInfo) -> settings_api::SecretInfo {
    settings_api::SecretInfo {
        name: info.name,
        node: None,
        rotated_at: Some(Time::new(info.rotated_at.unwrap_or(info.created_at))),
    }
}

/// The names of the secrets an environment has, its own and those it is given from above: never
/// their values.
async fn secret_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<settings_api::SecretInfo>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut shown: Vec<settings_api::SecretInfo> = Vec::new();
    let list = secrets::list(&mut conn, Owner::Environment(id))
        .await
        .map_err(secret_failed)?;
    let nodes: Vec<Id> = list.iter().filter_map(|info| info.node).collect();
    let refs = node_view::refs(&mut conn, &principal, &nodes, Policy::default()).await?;
    for info in list {
        let node = info.node.and_then(|node| refs.get(&node).cloned());
        let mut each = secret_info(info);
        each.node = node;
        shown.push(each);
    }
    Ok(shown)
}

/// Writes a secret of the environment, sealed; it is not returned.
async fn secret_set(
    ctx: Ctx,
    (): (),
    body: settings_api::NodeSecretSetBody,
) -> ApiResult<settings_api::SecretInfo> {
    let id = ctx.id("id")?;
    let name = ctx.param("name")?.to_owned();
    let master = master_key()?;
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let put = secrets::put(
        &mut tx,
        master,
        &secrets::Put {
            actor: Actor::User(principal.user()),
            owner: Owner::Environment(id),
            name: &name,
            value: &body.value,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await
    .map_err(secret_failed)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(secret_info(put))
}

/// Deletes a secret of the environment.
async fn secret_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let name = ctx.param("name")?.to_owned();
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    secrets::delete(
        &mut tx,
        &secrets::Delete {
            actor: Actor::User(principal.user()),
            owner: Owner::Environment(id),
            name: &name,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await
    .map_err(secret_failed)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_config_previews_validate_full_consent_and_patch_deferral_is_narrow() {
        let config = json!({"exec.version_overrides":{"codex":{
            "path":"/codex","version":"unknown","by":orch_types::new_id(),
            "at":"2026-10-31T00:00:00.000Z"
        }}});
        assert!(clean_config("config", &config).is_ok());
        let mut pending = config.clone();
        pending[registry::keys::EXEC_VERSION_OVERRIDES]["codex"]["at"] = json!("untrusted");
        assert!(clean_config("config", &pending).is_err());
        assert!(clean_patch_config("config", &pending).is_ok());
        pending["unknown.setting"] = json!(true);
        assert!(clean_patch_config("config", &pending).is_err());
    }

    #[test]
    fn a_root_is_an_absolute_path_of_the_machines_own_kind_that_does_not_climb() {
        assert_eq!(
            clean_root("root", " /srv/ada/work ", false).unwrap(),
            "/srv/ada/work"
        );
        assert!(clean_root("root", "work", false).is_err());
        assert!(clean_root("root", "/srv/ada/../root", false).is_err());
        assert!(clean_root("root", "C:\\Users\\ada\\work", false).is_err());
        assert!(clean_root("root", "C:\\Users\\ada\\work", true).is_ok());
        assert!(clean_root("root", "/srv/ada", true).is_err());
        assert!(clean_root("root", "/srv/a\nb", false).is_err());
    }

    #[test]
    fn a_root_must_lie_inside_a_root_the_machine_lists_and_a_machine_that_lists_none_is_asked() {
        let roots = vec!["/srv/ada/".to_owned(), "/data".to_owned()];
        assert!(within_roots("/srv/ada/work", &roots));
        assert!(within_roots("/srv/ada", &roots));
        assert!(within_roots("/data/ada", &roots));
        assert!(
            !within_roots("/srv/adam/work", &roots),
            "a directory beside, not inside"
        );
        assert!(
            !within_roots("/srv", &roots),
            "the parent of a root is not inside it"
        );
        assert!(!within_roots("/etc", &roots));
        assert!(within_roots("/srv/adam/work", &["/srv".to_owned()]));
        assert!(within_roots("/anywhere", &[]));
    }

    #[test]
    fn what_the_exec_node_offers_is_what_is_configured_within_what_the_machine_offers() {
        let policy = json!({ "exec.offers": ["plain", "custom"] });
        let configured = exec_of(
            &json!({ "exec.offers": ["plain", "docker"], "exec.orch_tools": false }),
            Some(&policy),
        );
        assert_eq!(configured.offers.len(), 2);
        assert!(configured.offers[0].available && !configured.offers[1].available);
        assert!(configured.offers[1].reason.is_some());
        assert!(!configured.orch_tools);
        let unconfigured = exec_of(&json!({}), Some(&policy));
        assert_eq!(
            unconfigured
                .offers
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["plain", "custom"]
        );
        assert!(unconfigured.orch_tools);
        let managed = exec_of(&json!({ "exec.offers": ["docker"] }), None);
        assert!(managed.offers[0].available && managed.machine_offers.is_none());
    }
}
