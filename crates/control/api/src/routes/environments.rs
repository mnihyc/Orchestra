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
use crate::routes::access::page::same;
use crate::routes::nodes::{actor_name, invalid, placed, policy_at, view as node_view};
use crate::routes::resources::machines::{registrable_machine as own_machine, reported_policy};
use crate::routes::resources::sealing::master_key;
use crate::routes::resources::view::{Holding, accesses, addresses, machine_refs, parse};
use crate::routes::resources::{Home, LIST_LIMIT, clean_name, record, require};
use crate::sign_in::origin;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Held, ObjectRef};
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName, ResourceKind};
use orch_api_contract::binding::NewEnvironmentAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    self as api, Environment, EnvironmentExec, EnvironmentHolding, EnvironmentIndex,
    EnvironmentRef, EnvironmentTemplate, EnvironmentThread, ExecOffer, McpFound,
};
use orch_api_contract::nodes::{JobRef, JobState, NodeKind, NodeRef};
use orch_api_contract::resources::HostRef;
use orch_api_contract::settings as settings_api;
use orch_authz::engine::Graph;
use orch_authz::environments::{Creating, NewEnvironment, Place, check_creation};
use orch_authz::nodes::Page;
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
use orch_settings::{Place as SettingsPlace, resolve_all};
use orch_settings_contract::registry::{self, Invalid, Scope};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::HashMap;

/// The longest a root directory is written: a path, as long as the longest the platforms take.
const ROOT_MAX_CHARS: usize = 4_096;

mod agent_reach;
pub(crate) mod hosted;
mod incarnation;
pub(crate) mod jobs;
mod list_settings;
pub(crate) mod mcp;
pub(crate) mod moves;
pub(crate) mod network;
mod places;
mod reach;
pub(crate) mod reading;
mod recovery;
pub(crate) mod retained;
mod support;
pub(crate) mod sync;
pub(crate) mod updates;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
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
        .on(api::MachineEnvironmentCreate, create_on_machine)
        .on(api::EnvironmentList, list)
        .on(api::EnvironmentGet, get)
        .on(api::EnvironmentUpdate, update)
        .on(api::EnvironmentDisable, disable)
        .on(api::EnvironmentEnable, enable)
        .on(api::EnvironmentDelete, delete)
        .on(settings_api::EnvironmentSettingList, setting_list)
        .on(settings_api::EnvironmentSecretList, secret_list)
        .on(settings_api::EnvironmentSecretSet, secret_set)
        .on(settings_api::EnvironmentSecretDelete, secret_delete);
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
        self.owner
            || self.administers
            || self.held.iter().any(|held| {
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
    }
}

/// Bounded inventory rows keep the existing environment projection and its page's current standings.
pub(crate) async fn inventory(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<Environment>> {
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
    environment_views(ctx, conn, principal, rows, &standing).await
}

/// Environments as the API shows them, for any number of them in the same few statements. What
/// the caller does not manage is left out: the environment's own values.
async fn environment_views(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<EnvironmentFull>,
    standing: &HashMap<Id, Standing>,
) -> ApiResult<Vec<Environment>> {
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let activity = crate::routes::resources::view::environment_activity(conn, &ids).await?;
    let display_settings = list_settings::load(conn, &rows).await?;
    let lifetimes: HashMap<_, _> = orch_db::queries::environments::temporary::among(conn, &ids)
        .await?
        .into_iter()
        .map(|row| (row.environment_id, row))
        .collect();
    let exposed = crate::routes::ports::exposures(conn, &ids, Time::new(ctx.now)).await?;
    let addresses = addresses(conn, &ids).await?;
    let people: Vec<Id> = rows.iter().map(|row| row.owner_id).collect();
    let people = refusals::user_refs(conn, &people).await?;
    let machine_facts: HashMap<_, _> = orch_db::queries::environment_facts::of_many(conn, &ids)
        .await?
        .into_iter()
        .collect();
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
    let policies: HashMap<Id, Json> = queries::machine_policies(conn, &machine_ids)
        .await?
        .into_iter()
        .collect();
    let template_ids: Vec<Id> = rows.iter().filter_map(|row| row.template_id).collect();
    let templates = queries::template_names(conn, &template_ids).await?;
    // What a managed one has beyond a directory: its host, the job under way, why it is to be removed.
    let hosted = hosting::hosts_of(conn, &ids).await?;
    let loss_eligible = orch_db::queries::loss::eligible_environments(conn, &ids)
        .await?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let host_ids: Vec<Id> = hosted.iter().map(|(_, host, _)| *host).collect();
    let host_addresses = crate::routes::resources::view::addresses(conn, &host_ids).await?;
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
    let defaults = queries::default_for(conn, &ids).await?;
    // Each harness's own configuration in its workspace, and the answer given for it.
    let mut config_trust = crate::routes::trust::views_of(conn, &ids).await?;
    let threads = env_rows::threads_in(conn, &ids).await?;
    let thread_owners: Vec<_> = threads.iter().map(|thread| thread.owner_id).collect();
    let thread_owners = refusals::user_refs(conn, &thread_owners).await?;
    let reach = orch_authz::environments::reach_counts(conn, principal, &ids, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let mut nodes: Vec<Id> = rows.iter().map(|row| row.home_node_id).collect();
    nodes.extend(defaults.iter().map(|(_, topic)| *topic));
    for thread in &threads {
        nodes.extend([thread.thread_id, thread.topic_id]);
    }
    nodes.sort_unstable();
    nodes.dedup();
    let refs = node_view::refs(conn, principal, &nodes, Policy::default()).await?;
    let holdings: Vec<Holding<'_>> = rows
        .iter()
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
        .collect();
    let access = accesses(conn, principal, &holdings).await?;
    let runtime_rows = hosting::live_runtimes_of(conn, &ids, &[]).await?;
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
            mounts: if whole && display_settings.has_mounts(row.id) {
                orch_engine::mounts::views(conn, row.id, ctx.now)
                    .await
                    .map_err(ApiError::internal)?
            } else {
                Vec::new()
            },
            exec: exec_of(
                &row.config,
                machine_facts
                    .get(&row.id)
                    .and_then(|machine| machine.machine_id)
                    .and_then(|machine| policies.get(&machine)),
            ),
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

/// One environment as the caller sees it: all of it where they hold something on it, and what a
/// thread lets them see where they hold nothing. The caller was let read it by [`reading::decide`],
/// or is the one who just made or changed it.
pub(crate) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Environment> {
    let row = queries::environments(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let object = ObjectRef {
        kind: ResourceKind::Environment.into(),
        id,
    };
    let effective = resources::effective(conn, principal, object, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let standing = HashMap::from([(
        id,
        Standing {
            admin_access: effective.admin_access,
            admin_place: effective.admin_place,
            participating: effective.participating,
            admin_only: effective.admin_only,
            owner: effective.owner,
            administers: effective.administers,
            held: effective.held,
        },
    )]);
    let mut view = environment_views(ctx, conn, principal, vec![row], &standing)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("an environment was read and is not shown"))?;
    if reading::decide(conn, principal, id).await? == reading::Reading::Whole {
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

pub(crate) use places::{host_choices, places_for};

/// Reads an environment as the caller may: decided by [`reading::decide`], then shown as it says.
/// Whatever else shows the view of an environment that a request of its own reads is to ask this.
pub(crate) async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Environment> {
    reading::decide(conn, principal, id).await?;
    show(ctx, conn, principal, id).await
}

/// The environments among `ids` that the caller holds something on, as they see them, with what the
/// caller holds on each.
async fn viewed_among(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<(Vec<Environment>, HashMap<Id, Standing>)> {
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
    let views = environment_views(ctx, conn, principal, rows, &standing).await?;
    Ok((views, standing))
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

/// Registers a directory through Machine Use, first owned by its creator.
async fn create_on_machine(
    ctx: Ctx,
    (): (),
    body: api::MachineEnvironmentCreateBody,
) -> ApiResult<Environment> {
    let name = clean_name("name", &body.name)?;
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let machine = own_machine(&ctx, &mut tx, ctx.id("id")?).await?;
    if !machine.is_active() {
        return Err(ApiError::not_found());
    }
    let root = match body.data_sources.as_ref() {
        Some(sources) => {
            orch_engine::environments::check_machine_data_sources(&mut tx, &machine, sources)
                .await
                .map_err(|why| invalid("data_sources", &why))?;
            sources.workspace.directory().unwrap_or("").to_owned()
        }
        None => clean_root("root", &body.root, machine.platform_os == "windows")?,
    };
    if body.data_sources.is_none()
        && let Some(policy) = reported_policy(&mut tx, &machine).await?
        && !within_roots(&root, &policy.environment_roots)
    {
        return Err(invalid(
            "root",
            "the machine's local policy does not let an environment be there",
        ));
    }
    // The org it is registered in: the caller's domain or an org inside it.
    let (home_place, home_kind) = placed(&mut tx, body.home_id).await?;
    if home_kind != NodeKind::Org {
        return Err(invalid("home_id", "an environment is registered in an org"));
    }
    let graph = Graph::load(&mut tx, &[body.home_id])
        .await
        .map_err(ApiError::internal)?;
    let in_domain = graph.get(body.home_id).is_some_and(|node| {
        principal
            .memberships()
            .domain_in(node.root)
            .is_some_and(|domain| node.path.contains(&domain))
    });
    if !in_domain {
        return Err(invalid(
            "home_id",
            "an environment is registered in the caller's domain or an org inside it",
        ));
    }
    let home_node = body.home_node_id.unwrap_or(body.home_id);
    if home_node != body.home_id
        && orch_db::queries::resources::routes::homes_under(&mut tx, body.home_id, &[home_node])
            .await?
            .is_empty()
    {
        return Err(invalid(
            "home_node_id",
            "this is no project or topic of that org",
        ));
    }
    let policy = policy_at(&mut tx, home_node).await?;
    let policies = Policies::everywhere(ResourcePolicy {
        owners_name_outsiders: policy.owners_name_outsiders,
        shared_subscriptions: None,
    });
    let created = match check_creation(
        &mut tx,
        &principal,
        &NewEnvironment {
            home: home_node,
            place: Place::Machine {
                machine: machine.id,
            },
            mounts: Vec::new(),
        },
        &policies,
    )
    .await
    {
        Ok(Creating::Allowed(created)) => created,
        Ok(Creating::Refused(refusal)) => return Err(refusals::denied(&mut tx, *refusal).await?),
        Ok(Creating::NotYourMachine | Creating::Exceeds(_)) => return Err(ApiError::not_found()),
        Err(error) => return Err(refusals::problem(&mut tx, error).await),
    };
    if !orch_authz::environments::reserve_capacity(&mut tx, &created)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::not_found());
    }
    if let Some(existing) = queries::named(&mut tx, principal.user(), home_place.org, &name).await?
    {
        return Err(taken(Some(existing)));
    }
    let id = orch_types::new_id();
    let made = queries::insert_connected(
        &mut tx,
        &NewConnected {
            id,
            name: &name,
            owner_id: principal.user(),
            machine_id: machine.id,
            home_node_id: home_node,
            workspace_root: &root,
            remote_write: body.remote_write,
            terminal: body.terminal,
        },
        ctx.now,
    )
    .await;
    match made {
        Ok(true) => {}
        Ok(false) => return Err(ApiError::not_found()),
        Err(DbError::Unique { .. }) => return Err(taken(None)),
        Err(error) => return Err(error.into()),
    }
    orch_db::queries::environments::hosting::set_entries(
        &mut tx,
        id,
        created.host.as_ref().and_then(|(entry, _)| *entry),
        None,
    )
    .await?;
    if let Some(sources) = &body.data_sources {
        // Initial configuration is part of creation, rather than a second visible revision.
        sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
            .bind(id)
            .bind(json!({"env.data_sources":sources}))
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
    }
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        principal.user(),
        match body.access {
            None | Some(NewEnvironmentAccess::Defaults) => NewAccess::Defaults,
            Some(NewEnvironmentAccess::OnlyMe) => NewAccess::OnlyMe,
        },
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::EnvironmentCreated),
        id,
        Home {
            node: Some(home_node),
            org: Some(home_place.org),
        },
        &format!(
            "{who} registered {root} of the machine {} as the environment {name}",
            machine.name
        ),
        json!({ "environment": id, "name": name, "machine": machine.id, "root": root }),
    )
    .await?;
    let made = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(made)
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

/// The environments the caller owns or holds a permission on.
async fn list(
    ctx: Ctx,
    query: api::EnvironmentListQuery,
    (): (),
) -> ApiResult<Vec<orch_api_contract::common::Listed<Environment>>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let page = Page {
        after: None,
        limit: LIST_LIMIT,
    };
    let seen = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Environment,
        page,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut seen: Vec<_> = seen
        .items
        .into_iter()
        .filter(|each| match query.holding {
            Some(EnvironmentHolding::Manage) => each
                .held
                .iter()
                .any(|held| held.permission == Permission::EnvironmentManage),
            Some(EnvironmentHolding::View) => each
                .held
                .iter()
                .any(|held| held.permission == Permission::EnvironmentView),
            None => true,
        })
        .collect();
    if let Some(org) = query.org_id {
        let homes: Vec<Id> = seen.iter().filter_map(|each| each.resource.home).collect();
        let inside =
            orch_db::queries::resources::routes::homes_under(&mut conn, org, &homes).await?;
        seen.retain(|each| {
            each.resource
                .home
                .is_some_and(|home| inside.contains(&home))
        });
    }
    let private = crate::routes::resources::hidden_items(&mut conn, &principal, &mut seen).await?;
    let ids: Vec<Id> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::environments(&mut conn, &ids).await?;
    let standing: HashMap<Id, Standing> = seen
        .into_iter()
        .map(|each| {
            (
                each.resource.id,
                Standing {
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    admin_only: each.admin_only,
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held,
                },
            )
        })
        .collect();
    Ok(crate::routes::resources::listed(
        environment_views(&ctx, &mut conn, &principal, rows, &standing).await?,
        private,
    ))
}

/// Reads an environment: whoever holds any permission on it, one of its two levels or one of its
/// four doors, reads all of it; whoever may view a thread that runs in it reads its name, owner, place,
/// state and the job under way and nothing else ([`read`]).
async fn get(
    ctx: Ctx,
    query: orch_api_contract::nodes::DeletedInspectionQuery,
    (): (),
) -> ApiResult<Environment> {
    if query.inspect_deleted == Some(true) {
        let id = ctx.id("id")?;
        let mut conn = ctx.app.db.acquire().await?;
        if hosting::environment(&mut conn, id)
            .await?
            .is_some_and(|row| row.deleted_at.is_some())
        {
            drop(conn);
            return retained::inspect(ctx, id).await;
        }
    }
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    // A page is a stored projection. Resource inspection belongs to an explicit change/probe,
    // never this GET: an online but unresponsive node must not hold every tab open for 30 seconds.
    read(&ctx, &mut conn, &principal, id).await
}

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

/// The answer to a change that was made against a revision that is out of date.
async fn stale(ctx: &Ctx, conn: &mut Conn, principal: &Principal, id: Id) -> ApiError {
    let current = show(ctx, conn, principal, id).await.ok();
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the environment was changed since it was read",
    )
    .with("current", current)
}

/// Changes an environment: its name, its own values, whether files may be written from outside
/// and shells opened, and the node it takes its settings from. Who may was asked of the engine:
/// `environment.manage` on it, and `node.interact` on another home.
async fn update(ctx: Ctx, (): (), mut body: api::EnvironmentUpdateBody) -> ApiResult<Environment> {
    body.config = body.config.as_ref().map(registry::canonical_own_values);
    let id = ctx.id("id")?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    if let Some(config) = &body.config {
        clean_config("config", config)?;
    }
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let row = queries::environments(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Environment,
        id,
        Permission::EnvironmentManage,
    )
    .await?;
    // Runtime updates and moves hold configuration together with every affected thread.
    updates::ensure_unheld(&mut tx, id).await?;
    if let Some(sources) = &body.data_sources {
        let mut config = body.config.take().unwrap_or_else(|| row.config.clone());
        config
            .as_object_mut()
            .ok_or_else(|| invalid("config", "the environment's own values are an object"))?
            .insert(
                orch_settings_contract::registry::keys::ENV_DATA_SOURCES.into(),
                serde_json::to_value(sources).map_err(ApiError::internal)?,
            );
        body.config = Some(config);
    }
    if let Some(config) = &body.config {
        clean_config("config", config)?;
        let sources = orch_driver_contract::runtime::EnvironmentDataSources::from_config(config)
            .map_err(ApiError::internal)?;
        if sources.borrows_directories() {
            let source_machine = orch_db::queries::environment_facts::of(&mut tx, id)
                .await?
                .and_then(|facts| facts.machine_id)
                .ok_or_else(|| {
                    invalid(
                        "data_sources",
                        "choose a machine before choosing an existing directory there",
                    )
                })?;
            let machine = orch_db::queries::resources::machine(&mut tx, source_machine)
                .await?
                .ok_or_else(ApiError::not_found)?;
            orch_engine::environments::check_machine_data_sources(&mut tx, &machine, &sources)
                .await
                .map_err(|why| invalid("data_sources", &why))?;
        }
    }
    if let Some(new_home) = body.home_node_id.filter(|home| *home != row.home_node_id) {
        if !queries::same_tree(&mut tx, new_home, row.home_node_id).await? {
            return Err(invalid(
                "home_node_id",
                "this is no node of the same root org",
            ));
        }
        let policy = policy_at(&mut tx, new_home).await?;
        authorization::require(
            &mut tx,
            &principal,
            new_home,
            PermissionName::OfNode(NodePermission::Interact),
            Act::Use,
            policy,
        )
        .await?;
    }
    // What the machine's local policy offers bounds what the environment may offer.
    if let (Some(config), Some(machine)) = (&body.config, row.machine_id) {
        let policies = queries::machine_policies(&mut tx, &[machine]).await?;
        let machine_offers = policies
            .first()
            .and_then(|(_, policy)| texts(policy.get("exec.offers")));
        let asked = texts(config.get("exec.offers"));
        if let (Some(offered), Some(asked)) = (machine_offers, asked)
            && asked.iter().any(|each| {
                !orch_agent_programs::Programs::builtin().offers_include(&offered, each)
            })
        {
            return Err(ApiError::new(
                ErrorCode::PolicyExecNotOffered,
                "the machine's local policy does not let its exec node offer that",
            )
            .with("offers", offered));
        }
    }
    let change = Change {
        name: name.as_deref(),
        config: body.config.as_ref(),
        remote_write: body.remote_write,
        terminal: body.terminal,
        browser: body.browser,
        vnc: body.vnc,
        home_node_id: body.home_node_id.filter(|home| *home != row.home_node_id),
    };
    if change == Change::default() {
        if row.revision != body.expected_revision {
            return Err(stale(&ctx, &mut tx, &principal, id).await);
        }
        return show(&ctx, &mut tx, &principal, id).await;
    }
    if let Some(new_name) = &name
        && let Some(existing) = queries::named(&mut tx, row.owner_id, row.org_id, new_name).await?
        && existing != id
    {
        return Err(taken(Some(existing)));
    }
    match queries::change(&mut tx, id, body.expected_revision, &change, ctx.now).await {
        Ok(true) => {}
        Ok(false) => return Err(stale(&ctx, &mut tx, &principal, id).await),
        Err(DbError::Unique { .. }) => return Err(taken(None)),
        Err(error) => return Err(error.into()),
    }
    if body.config.is_some() || change.home_node_id.is_some() {
        orch_db::queries::resources::mounts::changed(&mut tx, &[id], ctx.now).await?;
    }
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::EnvironmentUpdated),
        id,
        home_of(&row),
        &format!("{who} changed the environment {}", row.name),
        json!({
            "environment": id,
            "name": name,
            "config": body.config.is_some(),
            "remote_write": body.remote_write,
            "terminal": body.terminal,
            "browser": body.browser,
            "vnc": body.vnc,
            "home_node_id": change.home_node_id,
        }),
    )
    .await?;
    let exec_changed = body.config.as_ref().is_some_and(|config| {
        config.get("exec.offers") != row.config.get("exec.offers")
            || config.get("exec.orch_tools") != row.config.get("exec.orch_tools")
    });
    if exec_changed {
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::EnvironmentExecChanged),
            id,
            home_of(&row),
            &format!(
                "{who} changed what the exec node of the environment {} offers",
                row.name
            ),
            json!({ "environment": id }),
        )
        .await?;
    }
    // Publish after mount invalidation so the announced cursor covers the whole mutation.
    // The notification commits with the row; metadata-only changes need it too.
    job_rows::stamp_environment(&mut tx, id, ctx.now).await?;
    let changed = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if body.config.is_some() || change.home_node_id.is_some() {
        if let Err(error) = ctx.app.engine.apply_environment_mounts(id).await {
            tracing::warn!(environment = %id, %error, "environment mount changes remain pending");
        }
        if let Err(error) = ctx.app.engine.apply_environment_resources(id).await {
            tracing::warn!(environment = %id, %error, "environment resource changes remain pending");
        }
    }
    Ok(changed)
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
    if row.status == "lost" {
        ctx.app
            .engine
            .remove_lost_environment_in(&mut tx, id, principal.user(), ctx.now)
            .await
            .map_err(ApiError::internal)?;
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
    if orch_db::queries::environment_facts::of(&mut tx, id)
        .await?
        .is_some_and(|facts| facts.attaches())
    {
        ctx.app
            .engine
            .schedule_environment_removal(&mut tx, id, Some(principal.user()), ctx.now, ctx.now)
            .await
            .map_err(|error| invalid("retention", &error.to_string()))?;
        let promise = orch_db::queries::environments::retained::removal(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        orch_db::queries::environments::retained::begin_remove(
            &mut tx,
            id,
            promise.removal_revision,
        )
        .await?;
        orch_db::queries::environments::retained::keep_removed(
            &mut tx,
            id,
            promise.removal_revision,
            ctx.now,
        )
        .await?;
        shown.base.status = api::EnvironmentStatus::Removed;
        shown.remove_at = Some(Time::new(ctx.now));
        let purge_at = orch_db::queries::environments::retained::removal(&mut tx, id)
            .await?
            .and_then(|p| p.volumes_purge_at);
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
            json!({ "environment": id, "name": row.name, "purge_at":purge_at }),
        )
        .await?;
    } else {
        // Repeating a prior acceptance joins its work without changing its frozen deadline.
        let at = row
            .remove_at
            .unwrap_or_else(|| query.at.map_or(ctx.now, Time::get));
        let accepted = ctx
            .app
            .engine
            .schedule_removal_in(&mut tx, id, at, principal.user(), principal.now())
            .await
            .map_err(|error| match error {
                orch_engine::archive::RemovalScheduleError::Internal(error) => {
                    invalid("retention", &error.to_string())
                }
                error => removal_schedule_error(error),
            })?;
        shown.remove_at = Some(Time::new(row.remove_at.unwrap_or(at)));
        if accepted {
            let saved = orch_db::queries::environments::retained::removal(&mut tx, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            record(&ctx,&mut tx,orch_db::audit::action!(AuditAction::EnvironmentRemovalScheduled),id,home_of(&row),&format!("{who} scheduled the removal of the environment {}",row.name),json!({"environment":id,"at":at,"removal_revision":saved.removal_revision,"keep_days":saved.removed_keep_days,"threads":settled.threads,"turns":settled.turns})).await?;
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
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    queries::environments(&mut conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    require(
        &mut conn,
        &principal,
        ResourceKind::Environment,
        id,
        Permission::EnvironmentView,
    )
    .await?;
    let manages = resources::check(
        &mut conn,
        &principal,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        Permission::EnvironmentManage,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed();
    let views = resolve_all(&mut conn, SettingsPlace::Environment(id))
        .await
        .map_err(ApiError::internal)?;
    let mut nodes: Vec<Id> = views.iter().flat_map(|view| view.nodes()).collect();
    nodes.sort_unstable();
    nodes.dedup();
    let refs = node_view::refs(&mut conn, &principal, &nodes, Policy::default()).await?;
    views
        .into_iter()
        .map(|view| {
            let writable = manages && view.def().set_at(Scope::Environment);
            let setting = view.into_setting(writable).map_node(|node| {
                refs.get(&node).cloned().unwrap_or(NodeRef {
                    thread_display: None,
                    id: node,
                    kind: NodeKind::Org,
                    name: None,
                    redacted: true,
                })
            });
            same(&setting).ok_or_else(|| ApiError::internal("a setting does not read as the API's"))
        })
        .collect()
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
