//! Hosts: the machines that make environments from templates, listed by what the caller may use or manage, read, changed by their managers, and the token with which one enrolls.
//!
//! A host is a resource with a machine behind it: the machine is where its node runs, and the host
//! is what is owned, shared and used. What the node reports about itself (its drivers, its
//! capacity, its storages) is shown as it reports it; what an owner decides (the name, the labels,
//! whether it takes new environments, what it does when it is full) is changed here. Its stored
//! revision counter advances on configuration and ownership changes. Taking a host away, its metrics and its recycling
//! wait for the node and the engine's jobs. Its runtimes are listed, stopped and removed by its managers,
//! whoever's they are: that is their machine.

use crate::certificates::{TokenFor, TokenRequest};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{actor_name, invalid, placed, policy_at, view as node_view};
use crate::routes::resources::machines::{
    Installer, enrollment_commands, enrollment_commands_choices,
};
use crate::routes::resources::providers::{home_policies, refused_making};
use crate::routes::resources::view::{
    Holding, accesses, addresses, environment_refs, machine_refs, parse, provider_refs,
};
use crate::routes::resources::{Home, LIST_LIMIT, begin_mutation, clean_name, record, require};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Held, ObjectRef};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::admin::Runtime;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{Amounts, EnvironmentRef, HostStorage, TemplateRef, WhenFull};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::ports::HostPorts;
use orch_api_contract::resources::{
    self as api, EnrollmentToken, Host, HostDriver, HostRef, HostRunning, HostWhenFull,
    ProviderRef, UnitKind,
};
use orch_authz::nodes::Page;
use orch_authz::ownership::{self, Basis, Making};
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::environments::hosting::{self as environment_hosting, RuntimeFull};
use orch_db::queries::environments::routes as environment_queries;
use orch_db::queries::resources::routes::{self as queries, HostChange, HostRow};
use orch_db::{Conn, DbError};
use orch_engine::environments::{StopError, StopReason};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::HashMap;

/// CPU samples retain three decimal places in the integer resource projection.
const CPU_PERCENT_MILLI_SCALE: f64 = 1_000.0;

/// How many labels a host has at most.
const LABELS_MAX: usize = 32;
/// How long a label is at most, in characters.
const LABEL_MAX_CHARS: usize = 40;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::HostList, list)
        .on(api::HostEnrollmentTokenCreate, token_create)
        .on(api::HostGet, get)
        .on(api::HostUpdate, update)
        .on(api::HostUpdateNow, update_now)
        .on(api::HostRuntimeList, runtime_list)
        .on(api::HostRuntimeStop, runtime_stop)
        .on(api::HostRuntimeDelete, runtime_delete);
}

async fn update_now(ctx: Ctx, (): (), (): ()) -> ApiResult<api::ComponentUpdate> {
    let machine = {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let id = ctx.id("id")?;
        require(
            &mut conn,
            &principal,
            ResourceKind::Host,
            id,
            Permission::HostManage,
        )
        .await?;
        queries::hosts(&mut conn, &[id], principal.user())
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?
            .machine_id
    };
    super::machines::request_update_on(&ctx, machine).await
}

/// What the caller holds on a host, as the engine told it.
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

/// A number the node reported, if it is a whole one.
fn whole(value: &Json, key: &str) -> Option<i64> {
    value.get(key).and_then(Json::as_i64)
}

/// Amounts as a node reports them (`cpus`, `memory_mb`, `disk_gb`) or as the API names them
/// (`cpu_millis` in place of `cpus`); what is missing is nothing.
fn amounts(value: &Json) -> Amounts {
    let cpu_millis = whole(value, "cpu_millis")
        .or_else(|| whole(value, "cpus").map(|cpus| cpus.saturating_mul(1000)))
        .or_else(|| {
            value.get("cpus").and_then(Json::as_f64).map(|cpus| {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "a count of CPUs that a node reports is small"
                )]
                let millis = (cpus * 1000.0).round() as i64;
                millis
            })
        })
        .unwrap_or(0);
    Amounts {
        cpu_millis,
        memory_mb: whole(value, "memory_mb").unwrap_or(0),
        disk_gb: whole(value, "disk_gb").unwrap_or(0),
    }
}

/// The drivers a host reports, by name: what each is called and what it makes units for. What a
/// host reports that cannot be read as a driver is left out; the host is not wrong about itself,
/// this is only what the API shows.
fn drivers_of(value: &Json) -> Vec<HostDriver> {
    let Some(reported) = value.as_object() else {
        return Vec::new();
    };
    reported
        .iter()
        .map(|(name, descriptor)| HostDriver {
            availability: serde_json::from_value::<
                orch_driver_contract::descriptor::DriverDescriptor,
            >(descriptor.clone())
            .map(|driver| driver.availability())
            .unwrap_or_default(),
            name: descriptor
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or(name)
                .to_owned(),
            title: descriptor
                .get("title")
                .and_then(Json::as_str)
                .unwrap_or(name)
                .to_owned(),
            creates: descriptor
                .get("spec_schemas")
                .and_then(Json::as_object)
                .map(|schemas| {
                    schemas
                        .keys()
                        .filter_map(|pool| parse::<UnitKind>(pool).ok())
                        .collect()
                })
                .unwrap_or_default(),
            runtimes: descriptor
                .get("runtimes")
                .and_then(|classes| serde_json::from_value(classes.clone()).ok())
                .unwrap_or_default(),
            default_runtime: descriptor
                .get("default_runtime")
                .and_then(Json::as_str)
                .map(str::to_owned),
            runtime_classes: descriptor
                .get("runtime_classes")
                .and_then(Json::as_array)
                .map(|classes| {
                    classes
                        .iter()
                        .filter_map(|class| class.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The storages a host reports; one that cannot be read is left out.
fn storages_of(value: &Json) -> Vec<HostStorage> {
    value
        .as_array()
        .map(|reported| {
            reported
                .iter()
                .filter_map(|storage| serde_json::from_value(storage.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn when_full_word(value: WhenFull) -> &'static str {
    match value {
        WhenFull::Warn => "warn",
        WhenFull::Block => "block",
    }
}

/// The labels of a change: each trimmed, none empty, none twice, the longest and the most bounded.
fn clean_labels(path: &str, labels: &[String]) -> ApiResult<Vec<String>> {
    let mut clean: Vec<String> = Vec::with_capacity(labels.len());
    for label in labels {
        let label = label.trim();
        if label.is_empty()
            || label.chars().count() > LABEL_MAX_CHARS
            || label.chars().any(char::is_control)
        {
            return Err(invalid(
                path,
                "a label is one to forty characters, none of them a control character",
            ));
        }
        if !clean.iter().any(|each| each == label) {
            clean.push(label.to_owned());
        }
    }
    if clean.len() > LABELS_MAX {
        return Err(invalid(path, "a host has at most thirty-two labels"));
    }
    Ok(clean)
}

/// Host and fleet readers share the inventory, including environments whose units were archived.
pub(crate) async fn kept_layers_of(
    conn: &mut Conn,
    ids: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<HashMap<Id, Vec<api::HostKeptLayers>>> {
    let inventories: Vec<(Id, Json)> =
        sqlx::query_as("SELECT id,kept_images FROM hosts WHERE id=ANY($1)")
            .bind(ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let images: HashMap<Id, Vec<orch_driver_contract::kept::KeptImage>> = inventories
        .into_iter()
        .map(|(host, value)| {
            (
                host,
                serde_json::from_value(value.get("images").cloned().unwrap_or_else(|| json!([])))
                    .unwrap_or_default(),
            )
        })
        .collect();
    let environment_ids: Vec<Id> = images
        .values()
        .flatten()
        .map(|image| image.environment)
        .collect();
    let environments: HashMap<Id, EnvironmentRef> =
        environment_refs(conn, &environment_ids, &[], orch_types::Time::new(now))
            .await?
            .into_iter()
            .map(|(_, environment)| (environment.id, environment))
            .collect();
    Ok(images
        .into_iter()
        .map(|(host, images)| {
            (
                host,
                images
                    .into_iter()
                    .map(|image| api::HostKeptLayers {
                        environment_id: image.environment,
                        environment: environments.get(&image.environment).cloned(),
                        generation: image.generation,
                        bytes: image.bytes,
                    })
                    .collect(),
            )
        })
        .collect())
}

/// Host and fleet managers share retained-file rows with the same environment and home redactions.
pub(crate) async fn kept_data_of(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    hosts: &[Id],
) -> ApiResult<HashMap<Id, Vec<api::HostKeptData>>> {
    let rows = orch_db::queries::environments::retained::on_hosts(conn, hosts).await?;
    let homes: Vec<Id> = rows.iter().map(|row| row.home_id).collect();
    let homes = node_view::refs(conn, principal, &homes, Policy::default()).await?;
    let mut grouped: HashMap<Id, Vec<api::HostKeptData>> = HashMap::new();
    for row in rows {
        let Some(removal) = crate::routes::environments::retained::present(
            ctx,
            conn,
            principal,
            row.environment_id,
        )
        .await?
        else {
            continue;
        };
        if !matches!(
            removal.phase,
            orch_api_contract::environment::EnvironmentRemovalPhase::Kept
                | orch_api_contract::environment::EnvironmentRemovalPhase::Restoring
                | orch_api_contract::environment::EnvironmentRemovalPhase::Purging
        ) {
            continue;
        }
        grouped
            .entry(row.host_id)
            .or_default()
            .push(api::HostKeptData {
                environment_id: row.environment_id,
                name: removal.can_inspect.then_some(row.name),
                home: homes.get(&row.home_id).cloned(),
                storages: row.storages,
                phase: removal.phase,
                bytes: removal.bytes,
                purge_at: removal.purge_at,
                job: removal.job,
            });
    }
    Ok(grouped)
}

async fn host_views(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<HostRow>,
    standing: &HashMap<Id, Standing>,
) -> ApiResult<Vec<Host>> {
    let full_percent = orch_engine::places::system_full_percent(conn)
        .await
        .map_err(ApiError::internal)?;
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let managed: Vec<Id> = ids
        .iter()
        .copied()
        .filter(|id| {
            standing.get(id).is_some_and(|standing| {
                standing
                    .held
                    .iter()
                    .any(|held| held.permission == Permission::HostManage)
            })
        })
        .collect();
    let kept_data = kept_data_of(ctx, conn, principal, &managed).await?;
    let kept_layers = kept_layers_of(conn, &ids, ctx.now).await?;
    let loads = orch_db::queries::resources::hosting::loads(conn, &ids).await?;
    let mut by_host: HashMap<Id, Vec<_>> = HashMap::new();
    for load in loads {
        by_host.entry(load.host_id).or_default().push(load);
    }
    let declared: HashMap<Id, _> = by_host
        .into_iter()
        .map(|(id, held)| (id, orch_engine::places::declared_of(&held)))
        .collect();
    let addresses = addresses(conn, &ids).await?;
    let people: Vec<Id> = rows.iter().filter_map(|row| row.owner_id).collect();
    let people = refusals::user_refs(conn, &people).await?;
    let homes: Vec<Id> = rows.iter().filter_map(|row| row.home_id).collect();
    let homes = node_view::refs(conn, principal, &homes, Policy::default()).await?;
    let machines: Vec<Id> = rows.iter().map(|row| row.machine_id).collect();
    let software_updates = crate::component_updates::views(&ctx.app.link, conn, &machines).await?;
    let machines = machine_refs(&ctx.app, conn, &machines).await?;
    let system: Option<String> = setting(conn, Place::System, keys::HOSTS_WHEN_FULL)
        .await
        .map_err(ApiError::internal)?;
    let system: WhenFull = parse(system.as_deref().unwrap_or("warn"))?;
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
    rows.into_iter()
        .map(|row| {
            let own = row
                .when_full
                .as_deref()
                .map(parse::<WhenFull>)
                .transpose()?;
            let capacity = amounts(&row.capacity);
            let host_declared = declared
                .get(&row.id)
                .cloned()
                .unwrap_or_else(|| amounts(&row.declared));
            Ok(Host {
                lost_at: row.lost_at.map(orch_types::Time::new),
                full: Some(orch_engine::places::is_full(
                    &capacity,
                    &host_declared,
                    full_percent,
                )),
                usage: row.load_at.map(|at| api::HostUsage {
                    // Thousandths retain the node's fractional percentage without a floating wire count.
                    cpu_percent_milli: row
                        .load
                        .get("cpu_percent")
                        .and_then(Json::as_f64)
                        .filter(|value| value.is_finite())
                        .map(|value| (value * CPU_PERCENT_MILLI_SCALE).round() as i64),
                    memory_used_mb: row.load.get("memory_used_mb").and_then(Json::as_i64),
                    observed_at: orch_types::Time::new(at),
                }),
                stopped_environments: Some(row.stopped_environments),
                offline_since: if machines
                    .get(&row.machine_id)
                    .is_some_and(|machine| machine.online)
                {
                    None
                } else {
                    row.offline_since.map(orch_types::Time::new)
                },
                software_update: software_updates.get(&row.machine_id).cloned(),
                kept_data: kept_data.get(&row.id).cloned().unwrap_or_default(),
                kept_layers: if standing.get(&row.id).is_some_and(|standing| {
                    standing.held.iter().any(|held| {
                        matches!(
                            held.permission,
                            Permission::HostView | Permission::HostManage
                        )
                    })
                }) {
                    kept_layers.get(&row.id).cloned().unwrap_or_default()
                } else {
                    Vec::new()
                },
                base: HostRef {
                    id: row.id,
                    name: row.name.clone(),
                    address: addresses.get(&row.id).cloned().unwrap_or_default(),
                },
                owner: row.owner_id.and_then(|owner| people.get(&owner).cloned()),
                org_id: row.org_id,
                home: row.home_id.and_then(|home| homes.get(&home).cloned()),
                machine: machines
                    .get(&row.machine_id)
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the machine of a host is not there"))?,
                labels: row.labels.clone(),
                drivers: drivers_of(&row.drivers),
                ports: row
                    .ports
                    .as_ref()
                    .and_then(|saved| serde_json::from_value(saved.clone()).ok()),
                ports_revision: row.ports_revision,
                ports_proposal: crate::host_ports::proposal_of(&ctx.app, row.id),
                capacity,
                declared: host_declared,
                schedulable: row.schedulable,
                when_full: HostWhenFull {
                    value: own.unwrap_or(system),
                    own: own.is_some(),
                },
                storages: storages_of(&row.storages),
                running: HostRunning {
                    provider: row.provider_units,
                    environment: row.environment_units,
                },
                mine: row.mine,
                revision: row.revision,
                access: access.get(&row.id).cloned().ok_or_else(|| {
                    ApiError::internal("what the caller holds on a host was not read")
                })?,
            })
        })
        .collect()
}

/// Bounded inventory rows use the normal host view without another visibility scan.
pub(crate) async fn inventory(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<Host>> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::hosts(conn, &ids, principal.user()).await?;
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
    host_views(ctx, conn, principal, rows, &standing).await
}

/// Memory load for ordering uses the scheduler's same declarations and reported capacity.
pub(crate) async fn inventory_loads(conn: &mut Conn, ids: &[Id]) -> ApiResult<orch_types::Json> {
    let loads = orch_db::queries::resources::hosting::loads(conn, ids).await?;
    let mut by_host: HashMap<Id, Vec<_>> = HashMap::new();
    for load in loads {
        by_host.entry(load.host_id).or_default().push(load);
    }
    let capacities: Vec<(Id, Json)> =
        sqlx::query_as("SELECT id,capacity FROM hosts WHERE id=ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await
            .map_err(DbError::from)?;
    Ok(Json::Object(
        capacities
            .into_iter()
            .map(|(id, capacity)| {
                let declared =
                    orch_engine::places::declared_of(by_host.get(&id).map_or(&[], Vec::as_slice));
                let capacity = amounts(&capacity).memory_mb;
                let load = if capacity > 0 {
                    json!(declared.memory_mb as f64 / capacity as f64)
                } else {
                    Json::Null
                };
                (id.to_string(), load)
            })
            .collect(),
    ))
}

pub(crate) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Host> {
    let row = queries::hosts(conn, &[id], principal.user())
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let object = ObjectRef {
        kind: ResourceKind::Host.into(),
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
    host_views(ctx, conn, principal, vec![row], &standing)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("a host was read and is not shown"))
}

/// The hosts the caller may use or manage.
async fn list(
    ctx: Ctx,
    query: api::HostListQuery,
    (): (),
) -> ApiResult<Vec<orch_api_contract::common::Listed<Host>>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let page = Page {
        after: None,
        limit: LIST_LIMIT,
    };
    let seen = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Host,
        page,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut seen: Vec<_> = seen
        .items
        .into_iter()
        .filter(|each| {
            each.administers
                || each.held.iter().any(|held| {
                    matches!(
                        held.permission,
                        Permission::HostView
                            | Permission::HostUse
                            | Permission::HostManage
                            | Permission::AccessManage
                    )
                })
        })
        .collect();
    if let Some(org) = query.org_id {
        let homes: Vec<Id> = seen.iter().filter_map(|each| each.resource.home).collect();
        let inside = queries::homes_under(&mut conn, org, &homes).await?;
        seen.retain(|each| each.resource.home.is_none_or(|home| inside.contains(&home)));
    }
    let private = super::hidden_items(&mut conn, &principal, &mut seen).await?;
    let ids: Vec<Id> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::hosts(&mut conn, &ids, principal.user()).await?;
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
    Ok(super::listed(
        host_views(&ctx, &mut conn, &principal, rows, &standing).await?,
        private,
    ))
}

/// Reads a host: whoever holds any permission on it.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Host> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    queries::hosts(&mut conn, &[id], principal.user())
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    require(
        &mut conn,
        &principal,
        ResourceKind::Host,
        id,
        Permission::HostView,
    )
    .await?;
    let host = show(&ctx, &mut conn, &principal, id).await?;
    // A manager's page is filled with the node's proposal for its ports, asked for in the background
    // where this process has none yet; the next read shows it.
    if host.ports_proposal.is_none()
        && host.machine.online
        && host
            .access
            .permissions
            .iter()
            .any(|held| held == "host.manage")
    {
        crate::host_ports::ask_in_background(&ctx.app, id, host.machine.id);
    }
    Ok(host)
}

/// Makes the token with which a host enrolls: single use, and the host that uses it is the
/// caller's and has the home asked for. Who may use it is the page of the host to decide once it has
/// enrolled; its creation choice is bound to the token's one-use server ledger.
async fn token_create(
    ctx: Ctx,
    (): (),
    body: api::HostEnrollmentTokenCreateBody,
) -> ApiResult<EnrollmentToken> {
    super::machines::require_node_installable(&ctx.app.config)?;
    let name_hint = body
        .name_hint
        .as_deref()
        .map(|hint| clean_name("name_hint", hint))
        .transpose()?;
    if body.home_id.is_none()
        && body.access == Some(orch_api_contract::access::NewResourceAccess::Home)
    {
        return Err(invalid(
            "access",
            "home access needs an org as the host's home",
        ));
    }
    let (ctx, mut conn, principal) = begin_mutation(&ctx).await?;
    if let Some(home) = body.home_id {
        let (_, kind) = placed(&mut conn, home).await?;
        if kind != NodeKind::Org {
            return Err(invalid("home_id", "a host is at home in an org"));
        }
    }
    let members_make = match body.home_id {
        Some(home) => home_policies(&mut conn, home).await?.members_make,
        None => Vec::new(),
    };
    let policy = match body.home_id {
        Some(home) => policy_at(&mut conn, home).await?,
        None => Policy::default(),
    };
    match ownership::may_make(
        &mut conn,
        &principal,
        ResourceKind::Host,
        body.home_id,
        &members_make,
        policy,
    )
    .await
    {
        Ok(Making::Allowed(Basis::Member | Basis::Rules(_) | Basis::SystemAdmin)) => {}
        Ok(Making::Refused(why)) => {
            return Err(match body.home_id {
                Some(home) => refused_making(&mut conn, home, why).await,
                None => {
                    refusals::system_admin_only("only a system admin adds a host of the system")
                }
            });
        }
        Err(error) => return Err(refusals::problem(&mut conn, error).await),
    }
    let choices = body.delegate_policy.map(|handover| {
        orch_link_contract::enrollment_choices::EnrollmentChoices {
            roles: vec![orch_link_contract::session::Role::Node],
            delegate: if handover {
                vec!["*".into()]
            } else {
                Vec::new()
            },
            dedicated: false,
        }
    });
    let request = TokenRequest {
        created_by: principal.user(),
        purpose: TokenFor::Host { home: body.home_id },
        name_hint,
    };
    let certificates = ctx.app.link.certificates();
    let made = match &choices {
        Some(choices) => {
            certificates
                .issue_host_token_choices(&mut conn, &request, body.access, Some(choices))
                .await
        }
        None => {
            certificates
                .issue_host_token(&mut conn, &request, body.access)
                .await
        }
    }
    .map_err(|fault| ApiError::internal(format!("{}: {}", fault.code, fault.detail)))?;
    if !crate::certificates::bind_issuer(&mut conn, &made.receipt, ctx.user()?).await? {
        return Err(ApiError::internal(
            "the enrollment issuer could not be recorded",
        ));
    }
    conn.commit().await.map_err(DbError::from)?;
    Ok(EnrollmentToken {
        receipt: Some(made.receipt),
        commands: choices.as_ref().map_or_else(
            || enrollment_commands(&ctx.app.config, &made.token, Installer::Node),
            |choices| {
                enrollment_commands_choices(&ctx.app.config, &made.token, Installer::Node, choices)
            },
        ),
        token: made.token,
        expires_at: made.expires_at,
    })
}

async fn stale(ctx: &Ctx, conn: &mut Conn, principal: &Principal, id: Id) -> ApiError {
    let current = show(ctx, conn, principal, id).await.ok();
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the host was changed since it was read",
    )
    .with("current", current)
}

/// Changes a host: its name, its labels, whether it takes new environments, and what it does when
/// it is full.
/// The declaration of host ports a host has saved.
fn saved_ports(row: &HostRow) -> Option<HostPorts> {
    row.ports
        .as_ref()
        .and_then(|saved| serde_json::from_value(saved.clone()).ok())
}

/// Has the host's node check a new declaration of host ports under its own ceiling, before anything is
/// written and outside any transaction (the node may take seconds): answers the revision it accepted it at.
async fn accepted_ports(ctx: &Ctx, id: Id, declaration: &HostPorts) -> ApiResult<i64> {
    let row = {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(ctx, &mut conn).await?;
        queries::hosts(&mut conn, &[id], principal.user())
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?
    };
    if saved_ports(&row).as_ref() == Some(declaration) {
        sync_ports_policy(ctx, row.machine_id, Some(declaration.clone())).await?;
        return Ok(row.ports_revision);
    }
    let connected = {
        let mut conn = ctx.app.db.acquire().await?;
        orch_db::queries::resources::machine(&mut conn, row.machine_id)
            .await?
            .is_some_and(|machine| {
                crate::routes::resources::view::is_online(&ctx.app, machine.id, machine.online)
            })
    };
    if !connected {
        return Err(ApiError::new(
            ErrorCode::SourceOffline,
            "the host's node is not connected: a declaration of host ports is saved once its node accepted it",
        ));
    }
    let revision = row.ports_revision + 1;
    let answer = crate::host_ports::ask(
        &ctx.app,
        id,
        row.machine_id,
        Some(declaration.clone()),
        revision,
    )
    .await
    .map_err(|_| {
        ApiError::new(
            ErrorCode::SourceOffline,
            "the host's node did not answer: a declaration of host ports is saved once its node accepted it",
        )
    })?;
    if !answer.accepted || answer.revision != revision {
        return Err(ApiError::new(
            ErrorCode::PortsNotAllowed,
            answer
                .reason
                .unwrap_or_else(|| "the host's node did not accept the declaration".to_owned()),
        ));
    }
    sync_ports_policy(ctx, row.machine_id, Some(declaration.clone())).await?;
    Ok(revision)
}

async fn sync_ports_policy(ctx: &Ctx, machine: Id, publish: Option<HostPorts>) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    let facts = orch_db::queries::environment_facts::machine(&mut conn, machine, None).await?;
    let mut policy = facts
        .and_then(|facts| facts.policy)
        .and_then(|policy| {
            serde_json::from_value::<orch_link_contract::policy::LocalPolicy>(policy).ok()
        })
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::SourceOffline,
                "the machine has not reported its ports policy",
            )
        })?;
    if policy.ports.publish == publish {
        return Ok(());
    }
    policy.ports.publish = publish;
    drop(conn);
    super::machines::policy_update_on(
        ctx.clone(),
        machine,
        std::collections::BTreeMap::from([("ports".into(), json!(policy.ports))]),
    )
    .await?;
    Ok(())
}

async fn update(ctx: Ctx, (): (), body: api::HostUpdateBody) -> ApiResult<Host> {
    let id = ctx.id("id")?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let labels = body
        .labels
        .as_deref()
        .map(|labels| clean_labels("labels", labels))
        .transpose()?;
    // A declaration of host ports is checked by the host's node before the change is written.
    let accepted = match &body.ports {
        Some(Some(declaration)) => Some(accepted_ports(&ctx, id, declaration).await?),
        Some(None) => {
            let mut conn = ctx.app.db.acquire().await?;
            let principal = authorization::principal(&ctx, &mut conn).await?;
            let host = queries::hosts(&mut conn, &[id], principal.user())
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            drop(conn);
            // Clearing what is not there changes nothing, and needs no node.
            if saved_ports(&host).is_some() {
                sync_ports_policy(&ctx, host.machine_id, None).await?;
            }
            None
        }
        _ => None,
    };
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = queries::hosts(&mut tx, &[id], principal.user())
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if body.schedulable == Some(true)
        && orch_db::queries::loss::machine_lost(&mut tx, row.machine_id).await?
    {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "A host declared lost cannot take new work",
        ));
    }
    // Only what differs is a change: asking for what is there changes nothing, and moves nothing.
    let when_full = body.when_full.map(|value| value.map(when_full_word));
    let change = HostChange {
        name: name.as_deref().filter(|name| *name != row.name),
        labels: labels
            .as_deref()
            .filter(|labels| *labels != row.labels.as_slice()),
        schedulable: body.schedulable.filter(|each| *each != row.schedulable),
        when_full: when_full.filter(|each| *each != row.when_full.as_deref()),
    };
    let ports = body
        .ports
        .clone()
        .filter(|wanted| *wanted != saved_ports(&row));
    if change == HostChange::default() && ports.is_none() {
        if row.revision != body.expected_revision {
            return Err(stale(&ctx, &mut tx, &principal, id).await);
        }
        return show(&ctx, &mut tx, &principal, id).await;
    }
    if change != HostChange::default()
        && !queries::change_host(&mut tx, id, body.expected_revision, &change, ctx.now).await?
    {
        return Err(stale(&ctx, &mut tx, &principal, id).await);
    }
    if let Some(ports) = &ports {
        let revision = match ports {
            Some(_) => accepted.ok_or_else(|| {
                ApiError::internal("a declaration of host ports was not checked by its node")
            })?,
            None => row.ports_revision + 1,
        };
        let written = ports
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(ApiError::internal)?;
        // The revision was checked by the change just written, where there was one.
        let expected = (change == HostChange::default()).then_some(body.expected_revision);
        if !queries::set_host_ports(&mut tx, id, expected, written.as_ref(), revision, ctx.now)
            .await?
        {
            return Err(stale(&ctx, &mut tx, &principal, id).await);
        }
        let who = actor_name(&ctx)?;
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::HostPortsChanged),
            id,
            Home {
                node: row.home_id,
                org: row.org_id,
            },
            &format!("{who} changed the host ports of {}", row.name),
            json!({
                "host": id,
                "incoming_address": ports.as_ref().map(|each| each.incoming_address.clone()),
                "listen": ports.as_ref().map(|each| each.listen.clone()),
                "ranges": ports.as_ref().map(|each| each.ranges.clone()),
                "revision": revision,
            }),
        )
        .await?;
        if change == HostChange::default() {
            let changed = show(&ctx, &mut tx, &principal, id).await?;
            tx.commit().await.map_err(DbError::from)?;
            ports_changed(&ctx, id, ports.is_none()).await;
            return Ok(changed);
        }
    }
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::HostUpdated),
        id,
        Home {
            node: row.home_id,
            org: row.org_id,
        },
        &format!("{who} changed the host {}", row.name),
        json!({
            "host": id,
            "name": change.name,
            "labels": change.labels,
            "schedulable": change.schedulable,
            "when_full": change.when_full,
        }),
    )
    .await?;
    let changed = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Some(ports) = &ports {
        ports_changed(&ctx, id, ports.is_none()).await;
    }
    Ok(changed)
}

/// What a saved change of a host's declaration does to the host openings it holds: a cleared declaration
/// ends them on the node and leaves them unavailable until one is saved again; a changed one publishes each
/// again under it (a changed listen address rebinds).
async fn ports_changed(ctx: &Ctx, host: Id, cleared: bool) {
    let held = match ctx.app.db.acquire().await {
        Ok(mut conn) => orch_db::queries::ports::host_openings_on(&mut conn, host)
            .await
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if cleared {
        if let Ok(mut conn) = ctx.app.db.acquire().await {
            for (opening, revision) in &held {
                let _ = orch_db::queries::ports::host_published(
                    &mut conn,
                    *opening,
                    *revision,
                    false,
                    Some("the host's declaration of host ports was cleared"),
                )
                .await;
            }
        }
        let ids: Vec<Id> = held.iter().map(|(opening, _)| *opening).collect();
        crate::host_ports::unpublish_in_background(&ctx.app, &ids);
    } else {
        for (opening, _) in held {
            crate::host_ports::publish_in_background(&ctx.app, opening);
        }
    }
}

// ---------------------------------------------------------------- what runs on a host

/// The word the API writes a unit's kind in, and the one it is stored in.
fn unit_kind_word(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Environment => "environment",
        UnitKind::Provider => "provider",
    }
}

/// The runtimes of a host as the API shows them, for any number of them in the same few statements.
async fn runtime_views(
    ctx: &Ctx,
    conn: &mut Conn,
    runtimes: Vec<RuntimeFull>,
) -> ApiResult<Vec<Runtime>> {
    let environment_ids: Vec<Id> = runtimes
        .iter()
        .filter_map(|row| row.environment_id)
        .collect();
    let provider_ids: Vec<Id> = runtimes.iter().filter_map(|row| row.provider_id).collect();
    let machine_ids: Vec<Id> = runtimes.iter().filter_map(|row| row.machine_id).collect();
    let template_ids: Vec<Id> = runtimes.iter().map(|row| row.template_id).collect();
    let environments: HashMap<Id, EnvironmentRef> =
        environment_refs(conn, &environment_ids, &[], orch_types::Time::new(ctx.now))
            .await?
            .into_iter()
            .map(|(_, each)| (each.id, each))
            .collect();
    let providers: HashMap<Id, ProviderRef> = provider_refs(conn, &provider_ids, &[])
        .await?
        .into_iter()
        .map(|(_, each)| (each.id, each))
        .collect();
    let machines = machine_refs(&ctx.app, conn, &machine_ids).await?;
    let templates = environment_queries::template_names(conn, &template_ids).await?;
    let runtime_ids: Vec<Id> = runtimes.iter().map(|row| row.id).collect();
    let networks = crate::routes::environments::network::of_runtimes(conn, &runtime_ids).await?;
    let updates = ctx
        .app
        .engine
        .pending_runtime_updates(conn, &runtime_ids)
        .await
        .map_err(ApiError::internal)?;
    runtimes
        .into_iter()
        .map(|row| {
            let name = templates
                .iter()
                .find(|(id, _, _)| *id == row.template_id)
                .map(|(_, name, _)| name.clone())
                .unwrap_or_default();
            Ok(Runtime {
                network: networks
                    .get(&row.id)
                    .cloned()
                    .unwrap_or_else(|| crate::routes::environments::network::state_of(None, false)),
                id: row.id,
                kind: parse(&row.kind)?,
                host_id: row.host_id,
                driver: row.driver.clone(),
                unit_name: row.unit_name.clone(),
                state: row.state.clone(),
                state_detail: row.state_detail.clone(),
                generation: i64::from(row.generation),
                machine: row.machine_id.and_then(|id| machines.get(&id).cloned()),
                template: TemplateRef {
                    id: row.template_id,
                    name,
                },
                updates: updates.get(&row.id).map_or(0, |updates| {
                    i64::try_from(updates.len()).unwrap_or(i64::MAX)
                }),
                successor_of: row.successor_of,
                remove_at: row.remove_at.map(orch_types::Time::new),
                environment: row
                    .environment_id
                    .and_then(|id| environments.get(&id).cloned()),
                provider: row.provider_id.and_then(|id| providers.get(&id).cloned()),
                last_active_at: row.last_active_at.map(orch_types::Time::new),
            })
        })
        .collect()
}

/// The runtime `runtime` of the host `host`: it is not found under another host.
async fn runtime_of_host(conn: &mut Conn, host: Id, runtime: Id) -> ApiResult<RuntimeFull> {
    environment_hosting::runtime(conn, runtime)
        .await?
        .filter(|row| row.host_id == host)
        .ok_or_else(ApiError::not_found)
}

/// Everything that runs on a host, whoever's it is.
async fn runtime_list(
    ctx: Ctx,
    query: api::HostRuntimeListQuery,
    (): (),
) -> ApiResult<Vec<Runtime>> {
    let host = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    orch_db::queries::resources::hosting::host(&mut conn, host)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let found =
        environment_hosting::runtimes_on(&mut conn, host, query.kind.map(unit_kind_word)).await?;
    runtime_views(&ctx, &mut conn, found).await
}

/// Stops the selected unit on the host, returning credentials and respecting active work.
async fn runtime_stop(ctx: Ctx, (): (), body: api::HostRuntimeStopBody) -> ApiResult<Runtime> {
    let host = ctx.id("id")?;
    let runtime = ctx.id("runtime_id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = runtime_of_host(&mut tx, host, runtime).await?;
    if row.kind == "provider" {
        ctx.app
            .engine
            .stop_host_provider_unit_in(
                tx,
                &principal,
                &crate::sign_in::origin(&ctx),
                host,
                runtime,
                body.interrupt,
            )
            .await
            .map_err(super::provider_runtime::refused)?;
        let mut conn = ctx.app.db.acquire().await?;
        let row = runtime_of_host(&mut conn, host, runtime).await?;
        return runtime_views(&ctx, &mut conn, vec![row])
            .await?
            .pop()
            .ok_or_else(|| ApiError::internal("a runtime was read and is not shown"));
    }
    let Some(environment) = row.environment_id.filter(|_| row.kind == "environment") else {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "this unit has no environment or provider",
        ));
    };
    let user = ctx.user()?.id();
    ctx.app
        .engine
        .stop_environment_in(
            tx,
            &principal,
            &crate::sign_in::origin(&ctx),
            environment,
            body.interrupt,
            StopReason::Person(user),
        )
        .await
        .map_err(|error| match error {
            StopError::NotFound => ApiError::not_found(),
            StopError::NotManaged => ApiError::new(ErrorCode::PlanBlocked, error.to_string()),
            StopError::Busy(turns) => ApiError::new(
                ErrorCode::ResourceInUse,
                "turns run in the environment; stop it with interrupt to end them first",
            )
            .with("turns", turns),
            StopError::Unavailable(status) => ApiError::new(
                ErrorCode::PlanBlocked,
                format!("the environment is {status}"),
            ),
            StopError::CredentialReturnFailed(why) => {
                ApiError::new(ErrorCode::CredentialReturnFailed, why)
            }
            StopError::Host(why) => ApiError::new(ErrorCode::EnvironmentUnavailable, why),
            StopError::Internal(error) => ApiError::engine(error),
        })?;
    let mut conn = ctx.app.db.acquire().await?;
    let row = runtime_of_host(&mut conn, host, runtime).await?;
    runtime_views(&ctx, &mut conn, vec![row])
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("a runtime was read and is not shown"))
}

/// Takes this exact environment or provider unit away from its host. Provider removal uses its
/// durable lifecycle job, retaining volumes unless requested. Active work prevents acceptance.
async fn runtime_delete(ctx: Ctx, query: api::HostRuntimeDeleteQuery, (): ()) -> ApiResult<()> {
    let host = ctx.id("id")?;
    let runtime = ctx.id("runtime_id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = runtime_of_host(&mut tx, host, runtime).await?;
    if query.confirm != row.unit_name {
        return Err(invalid("confirm", "the name of the unit, to be sure"));
    }
    if row.kind == "provider" {
        return ctx
            .app
            .engine
            .remove_host_provider_unit_in(
                tx,
                &principal,
                &crate::sign_in::origin(&ctx),
                host,
                runtime,
                query.volumes == Some(true),
            )
            .await
            .map_err(super::provider_runtime::refused);
    }
    let Some(environment) = row.environment_id.filter(|_| row.kind == "environment") else {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "this unit has no environment or provider",
        ));
    };
    if !orch_db::queries::jobs::open_of_environment(&mut tx, environment)
        .await?
        .is_empty()
    {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "a job works on the environment",
        ));
    }
    let turns = environment_hosting::active_turns(&mut tx, environment).await?;
    if turns > 0 {
        return Err(
            ApiError::new(ErrorCode::ResourceInUse, "turns run in the environment")
                .with("turns", turns),
        );
    }
    if query.volumes == Some(true) {
        let bound = environment_queries::threads_bound(&mut tx, environment).await?;
        if bound > 0 {
            return Err(ApiError::new(
                ErrorCode::ResourceInUse,
                "threads are bound to the environment, whose volumes would go with its unit",
            )
            .with("threads", bound));
        }
        if ctx
            .app
            .engine
            .schedule_environment_removal(
                &mut tx,
                environment,
                Some(principal.user()),
                ctx.now,
                ctx.now,
            )
            .await
            .map_err(ApiError::internal)?
        {
            let env = environment_hosting::environment(&mut tx, environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let promise = orch_db::queries::environments::retained::removal(&mut tx, environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(principal.user()),
                "environment.removal_scheduled",
                "environment",
                Some(environment),
                "Removal of the environment was scheduled by its host manager",
            );
            entry.history = true;
            entry.node_id = Some(env.home_node_id);
            entry.org_id = Some(env.org_id);
            entry.environment_id = Some(environment);
            entry.details = json!({"reason":"host_manager","runtime":runtime,"at":ctx.now,"removal_revision":promise.removal_revision,"keep_days":promise.removed_keep_days});
            orch_db::audit::append(&mut tx, &entry, &crate::sign_in::origin(&ctx), ctx.now).await?;
        }
    } else {
        // The unit goes, the volumes stay: the environment is stopped, and its next start makes a new unit on them.
        environment_hosting::retire_runtime(
            &mut tx,
            runtime,
            Some("removed by the host's manager"),
            ctx.now,
        )
        .await?;
        environment_hosting::set_status(
            &mut tx,
            environment,
            &["running", "offline", "stopped", "failed"],
            "stopped",
            None,
            ctx.now,
        )
        .await?;
    }
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        AuditAction::HostUpdated,
        host,
        Home::default(),
        &format!("{who} removed the unit {} from the host", row.unit_name),
        json!({ "host": host, "runtime": runtime, "environment": environment, "volumes": query.volumes == Some(true) }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_read_as_a_node_reports_them_or_as_the_api_names_them() {
        assert_eq!(
            amounts(&json!({ "cpus": 4, "memory_mb": 8192, "disk_gb": 100 })),
            Amounts {
                cpu_millis: 4000,
                memory_mb: 8192,
                disk_gb: 100
            }
        );
        assert_eq!(amounts(&json!({ "cpus": 0.5 })).cpu_millis, 500);
        assert_eq!(amounts(&json!({ "cpu_millis": 1500 })).cpu_millis, 1500);
        assert_eq!(
            amounts(&json!({})),
            Amounts {
                cpu_millis: 0,
                memory_mb: 0,
                disk_gb: 0
            }
        );
    }

    #[test]
    fn what_a_node_reports_of_its_drivers_and_storages_is_shown_and_what_is_not_readable_is_left_out()
     {
        let drivers = drivers_of(&json!({
            "docker": {
                "name": "docker",
                "title": "Docker",
                "spec_schemas": { "environment": {}, "provider": {}, "elsewhere": {} },
                "runtime_classes": ["runc", "gvisor"]
            },
            "host": { "title": "Plain processes" }
        }));
        assert_eq!(drivers.len(), 2);
        assert_eq!(drivers[0].name, "docker");
        assert_eq!(
            drivers[0].creates,
            [UnitKind::Environment, UnitKind::Provider]
        );
        assert_eq!(drivers[0].runtime_classes, ["runc", "gvisor"]);
        assert_eq!(drivers[1].name, "host");
        assert!(drivers[1].creates.is_empty() && drivers[1].runtime_classes.is_empty());
        assert!(drivers_of(&json!([1, 2])).is_empty());
        let storages = storages_of(&json!([
            { "name": "fast", "default": true, "capacity_gb": 500, "free_gb": 200, "private_views": false },
            { "name": "broken" }
        ]));
        assert_eq!(storages.len(), 1);
        assert_eq!(storages[0].name, "fast");
    }

    #[test]
    fn labels_are_trimmed_kept_once_and_bounded() {
        let labels = |each: &[&str]| {
            each.iter()
                .map(|label| (*label).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            clean_labels("labels", &labels(&[" gpu ", "fast", "gpu"])).unwrap(),
            ["gpu", "fast"]
        );
        assert!(clean_labels("labels", &labels(&["  "])).is_err());
        assert!(clean_labels("labels", &labels(&[&"l".repeat(41)])).is_err());
        assert!(clean_labels("labels", &labels(&["a\nb"])).is_err());
        let many: Vec<String> = (0..33).map(|index| format!("label{index}")).collect();
        assert!(clean_labels("labels", &many).is_err());
        assert!(clean_labels("labels", &many[..32]).is_ok());
    }
}
