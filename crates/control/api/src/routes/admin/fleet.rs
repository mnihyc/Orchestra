//! What runs on the system's hosts and how it has fared: every host, the runtimes of both pools, the measurements the system stored about itself over a span, and the plugins loaded anywhere.
//!
//! These are read as the tables hold them. A host is shown as its own page shows it, to somebody
//! who is the Admin of its home; a runtime is shown whoever's it is. The measurements come from the
//! values of each minute while the span is inside the days they are kept, and from the values of each
//! hour for a longer or an older one, at a step that never gives more than 720 points for a series.
//! A plugin is listed from the configuration that names it, and from what each machine reports of
//! the plugins it loaded.

use super::{SECONDS_PER_DAY, admin_of, invalid};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::resources::LIST_LIMIT;
use crate::routes::resources::view::{
    Holding, accesses, addresses, environment_refs, machine_refs, parse, provider_refs,
};
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::decisions::Held;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::admin::{
    AdminHostList, AdminMetricList, AdminMetricListQuery, AdminPlugin, AdminPluginList,
    AdminRuntimeList, AdminRuntimeListQuery, METRIC_MAX_POINTS, METRIC_STEP_MINUTES,
    MetricResolution, MetricSeries, PluginState, Runtime,
};
use orch_api_contract::environment::{Amounts, HostStorage, TemplateRef, WhenFull};
use orch_api_contract::resources::{
    Host, HostDriver, HostRef, HostRunning, HostWhenFull, MachineRef, UnitKind,
};
use orch_authz::nodes::Page;
use orch_authz::{Policy, Principal, resources};
use orch_db::Conn;
use orch_db::queries::metrics::{self, Resolution, StepRow};
use orch_db::queries::resources::management;
use orch_db::queries::resources::routes::{self as queries, HostRow};
use orch_link_contract::session::MachineFactsPlugin;
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use std::collections::{BTreeMap, HashMap};

/// The thousandths of a processor that make one processor: a node reports whole processors, the API
/// counts in thousandths.
const MILLIS_PER_CPU: i64 = 1_000;
/// The finest step, in minutes, that a series read from the values of each hour may have: the
/// values stored for an hour cannot be told apart by the minute.
const FINEST_HOUR_STEP_MINUTES: u32 = 60;

/// The ways a plugin is told to be doing when only the configuration names it: the process has
/// not said that it started it.
const CONFIGURED_ONLY: &str =
    "it is named in the configuration, and this process has not reported starting it";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminHostList, hosts)
        .on(AdminRuntimeList, runtimes)
        .on(AdminMetricList, metric_series)
        .on(AdminPluginList, plugins);
}

// ---- hosts ----

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
        .or_else(|| whole(value, "cpus").map(|cpus| cpus.saturating_mul(MILLIS_PER_CPU)))
        .unwrap_or(0);
    Amounts {
        cpu_millis,
        memory_mb: whole(value, "memory_mb").unwrap_or(0),
        disk_gb: whole(value, "disk_gb").unwrap_or(0),
    }
}

/// The drivers a host reports, by name. What cannot be read as a driver is left out: the host is not
/// wrong about itself, this is only what the API shows.
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

/// Every host of the system, as the Admin of all of them reads them.
async fn hosts(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Host>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let ids = management::host_ids(&mut conn).await?;
    let rows = queries::hosts(&mut conn, &ids, principal.user()).await?;
    let standing = standings(&mut conn, &principal).await?;
    host_views(&ctx, &mut conn, &principal, rows, &standing).await
}

/// What the caller holds on each host there is, a page of the engine's list at a time.
async fn standings(conn: &mut Conn, principal: &Principal) -> ApiResult<HashMap<Id, Standing>> {
    let mut found = HashMap::new();
    let mut after = None;
    loop {
        let page = resources::visible(
            conn,
            principal,
            ResourceKind::Host,
            Page {
                after,
                limit: LIST_LIMIT,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        for each in page.items {
            found.insert(
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
            );
        }
        match page.next {
            Some(next) => after = Some(next),
            None => return Ok(found),
        }
    }
}

async fn host_views(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<HostRow>,
    standing: &HashMap<Id, Standing>,
) -> ApiResult<Vec<Host>> {
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
    let kept_data =
        crate::routes::resources::hosts::kept_data_of(ctx, conn, principal, &managed).await?;
    let kept_layers = crate::routes::resources::hosts::kept_layers_of(conn, &ids, ctx.now).await?;
    let addresses = addresses(conn, &ids).await?;
    let people: Vec<Id> = rows.iter().filter_map(|row| row.owner_id).collect();
    let people = crate::middleware::authorization::refusals::user_refs(conn, &people).await?;
    let homes: Vec<Id> = rows.iter().filter_map(|row| row.home_id).collect();
    let homes =
        crate::routes::nodes::view::refs(conn, principal, &homes, Policy::default()).await?;
    let machines: Vec<Id> = rows.iter().map(|row| row.machine_id).collect();
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
            Ok(Host {
                lost_at: row.lost_at.map(orch_types::Time::new),
                full: None,
                usage: None,
                stopped_environments: None,
                // The fleet's overview does not follow a host's transitions or its program's update: its own page does.
                offline_since: None,
                software_update: None,
                kept_data: kept_data.get(&row.id).cloned().unwrap_or_default(),
                kept_layers: kept_layers.get(&row.id).cloned().unwrap_or_default(),
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
                capacity: amounts(&row.capacity),
                declared: amounts(&row.declared),
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
                // A host nothing was read on is the Admin's all the same.
                access: access.get(&row.id).cloned().unwrap_or_else(|| {
                    orch_api_contract::access::ResourceAccess {
                        admin_place: None,
                        admin_only: Vec::new(),
                        admin_access: None,
                        participating: false,
                        permissions: Vec::new(),
                        administers: true,
                        through: Vec::new(),
                    }
                }),
            })
        })
        .collect()
}

// ---- runtimes ----

/// The runtimes of both pools, whoever's they are.
async fn runtimes(ctx: Ctx, query: AdminRuntimeListQuery, (): ()) -> ApiResult<Vec<Runtime>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let kind = query.kind.map(|kind| match kind {
        UnitKind::Environment => "environment",
        UnitKind::Provider => "provider",
    });
    let rows = management::runtimes(&mut conn, query.host_id, kind).await?;
    let machine_ids: Vec<Id> = rows.iter().filter_map(|row| row.machine_id).collect();
    let environment_ids: Vec<Id> = rows.iter().filter_map(|row| row.environment_id).collect();
    let provider_ids: Vec<Id> = rows.iter().filter_map(|row| row.provider_id).collect();
    let machines = machine_refs(&ctx.app, &mut conn, &machine_ids).await?;
    let environments = environment_refs(
        &mut conn,
        &environment_ids,
        &[],
        orch_types::Time::new(ctx.now),
    )
    .await?;
    let providers = provider_refs(&mut conn, &provider_ids, &[]).await?;
    let runtime_ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let networks =
        crate::routes::environments::network::of_runtimes(&mut conn, &runtime_ids).await?;
    let updates = ctx
        .app
        .engine
        .pending_runtime_updates(&mut conn, &runtime_ids)
        .await
        .map_err(ApiError::internal)?;
    rows.into_iter()
        .map(|row| {
            Ok(Runtime {
                network: networks
                    .get(&row.id)
                    .cloned()
                    .unwrap_or_else(|| crate::routes::environments::network::state_of(None, false)),
                id: row.id,
                kind: parse(&row.kind)?,
                host_id: row.host_id,
                driver: row.driver,
                unit_name: row.unit_name,
                state: row.state,
                state_detail: row.state_detail,
                generation: i64::from(row.generation),
                machine: row
                    .machine_id
                    .and_then(|machine| machines.get(&machine).cloned()),
                template: TemplateRef {
                    id: row.template_id,
                    name: row.template_name,
                },
                updates: updates.get(&row.id).map_or(0, |updates| {
                    i64::try_from(updates.len()).unwrap_or(i64::MAX)
                }),
                successor_of: row.successor_of,
                remove_at: row.remove_at.map(Time::new),
                environment: row.environment_id.and_then(|id| {
                    environments
                        .iter()
                        .find(|(_, found)| found.id == id)
                        .map(|(_, found)| found.clone())
                }),
                provider: row.provider_id.and_then(|id| {
                    providers
                        .iter()
                        .find(|(_, found)| found.id == id)
                        .map(|(_, found)| found.clone())
                }),
                last_active_at: row.last_active_at.map(Time::new),
            })
        })
        .collect()
}

// ---- stored metrics ----

/// The widths of a point, in minutes, that a series read from the values of each hour may have.
fn hour_steps() -> impl Iterator<Item = u32> {
    METRIC_STEP_MINUTES
        .into_iter()
        .filter(|step| *step >= FINEST_HOUR_STEP_MINUTES)
}

/// How many points a series between `from` and `to` has at most at a step of `step` minutes: the
/// steps are aligned to the epoch, so this is how many of them the span touches.
fn points_between(from: DateTime<Utc>, to: DateTime<Utc>, step: u32) -> i64 {
    const SECONDS_PER_MINUTE: i64 = 60;
    let step = i64::from(step.max(1)) * SECONDS_PER_MINUTE;
    to.timestamp().div_euclid(step) - from.timestamp().div_euclid(step) + 1
}

/// The step of a point: the one asked for if it fits, else the finest of the steps that fit that
/// gives at most [`METRIC_MAX_POINTS`] points.
fn step_for(
    asked: Option<u32>,
    resolution: Resolution,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> ApiResult<u32> {
    let allowed: Vec<u32> = match resolution {
        Resolution::Minutes => METRIC_STEP_MINUTES.to_vec(),
        Resolution::Hours => hour_steps().collect(),
    };
    let fits = |step: u32| points_between(from, to, step) <= i64::from(METRIC_MAX_POINTS);
    match asked {
        Some(step) if !METRIC_STEP_MINUTES.contains(&step) => Err(invalid(
            "step",
            "is a number of minutes: 1, 5, 60 or 1440",
        )),
        Some(step) if !allowed.contains(&step) => Err(invalid(
            "step",
            "a span of that age is kept by the hour: ask for a step of 60 or 1440",
        )),
        Some(step) if !fits(step) => Err(invalid(
            "step",
            &format!("gives more than {METRIC_MAX_POINTS} points over this span"),
        )),
        Some(step) => Ok(step),
        None => allowed.into_iter().find(|step| fits(*step)).ok_or_else(|| {
            invalid(
                "from",
                &format!(
                    "this span has more than {METRIC_MAX_POINTS} points even at a step of a day: ask for a shorter one"
                ),
            )
        }),
    }
}

/// The labels of a series as text, whatever the figures that stored them were.
fn labels_of(labels: &Json) -> BTreeMap<String, String> {
    labels
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The series of the points that were read, one for each measurement with its labels, in order.
fn series_of(rows: Vec<StepRow>, resolution: Resolution) -> Vec<MetricSeries> {
    let mut series: Vec<MetricSeries> = Vec::new();
    let mut last: Option<(String, Json)> = None;
    for row in rows {
        let key = (row.name.clone(), row.labels.clone());
        if last.as_ref() != Some(&key) {
            series.push(MetricSeries {
                name: row.name.clone(),
                labels: labels_of(&row.labels),
                resolution: match resolution {
                    Resolution::Minutes => MetricResolution::Minute,
                    Resolution::Hours => MetricResolution::Hour,
                },
                points: Vec::new(),
                peaks: matches!(resolution, Resolution::Hours).then(Vec::new),
            });
            last = Some(key);
        }
        if let Some(current) = series.last_mut() {
            current.points.push((Time::new(row.at), row.value));
            if let (Some(peaks), Some(peak)) = (current.peaks.as_mut(), row.peak) {
                peaks.push((Time::new(row.at), peak));
            }
        }
    }
    series
}

/// What the system stored about itself, over a span.
async fn metric_series(
    ctx: Ctx,
    query: AdminMetricListQuery,
    (): (),
) -> ApiResult<Vec<MetricSeries>> {
    admin_of(&ctx)?;
    let (from, to) = (query.from.get(), query.to.get());
    if to < from {
        return Err(invalid("to", "must not be before from"));
    }
    let fine_days = ctx.app.config.retention.metrics_fine_days;
    let fine_since =
        ctx.now - Duration::days(i64::try_from(fine_days).unwrap_or(i64::MAX / SECONDS_PER_DAY));
    // Minutes are kept for `metrics_fine_days`, or for good where that is none.
    let resolution = if fine_days == 0 || from >= fine_since {
        Resolution::Minutes
    } else {
        Resolution::Hours
    };
    let step = step_for(query.step, resolution, from, to)?;
    let mut conn = ctx.app.db.acquire().await?;
    let names = query.name.unwrap_or_default();
    if names.is_empty() {
        // Without a name the answer is which names there are.
        return Ok(metrics::measured_names(&mut conn, resolution, from, to)
            .await?
            .into_iter()
            .map(|name| MetricSeries {
                name,
                labels: BTreeMap::new(),
                resolution: match resolution {
                    Resolution::Minutes => MetricResolution::Minute,
                    Resolution::Hours => MetricResolution::Hour,
                },
                points: Vec::new(),
                peaks: None,
            })
            .collect());
    }
    let rows = metrics::steps(
        &mut conn,
        resolution,
        &names,
        from,
        to,
        i32::try_from(step).unwrap_or(i32::MAX),
    )
    .await?;
    Ok(series_of(rows, resolution))
}

// ---- plugins ----

fn plugin_state(word: &str) -> PluginState {
    match word {
        "running" => PluginState::Running,
        "restarting" => PluginState::Restarting,
        _ => PluginState::Failed,
    }
}

/// The plugins named in the configuration of this process, and the ones every machine reports.
async fn plugins(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<AdminPlugin>> {
    admin_of(&ctx)?;
    let mut found: Vec<AdminPlugin> = ctx
        .app
        .config
        .plugin
        .iter()
        .map(|plugin| AdminPlugin {
            component: "api".to_owned(),
            machine: None,
            kind: plugin.kind.clone(),
            name: plugin.name.clone(),
            version: String::new(),
            state: PluginState::Restarting,
            detail: CONFIGURED_ONLY.to_owned(),
        })
        .collect();
    let mut conn = ctx.app.db.acquire().await?;
    let reporting = management::machines_reporting_plugins(&mut conn).await?;
    let ids: Vec<Id> = reporting.iter().map(|row| row.id).collect();
    let machines: HashMap<Id, MachineRef> = machine_refs(&ctx.app, &mut conn, &ids).await?;
    for row in reporting {
        let reported: Vec<MachineFactsPlugin> =
            serde_json::from_value(row.plugins).unwrap_or_default();
        for plugin in reported {
            found.push(AdminPlugin {
                component: if row.is_host { "node" } else { "connector" }.to_owned(),
                machine: machines.get(&row.id).cloned(),
                kind: plugin.kind,
                name: plugin.name,
                version: plugin.version,
                state: plugin_state(&plugin.state),
                detail: match plugin.state.as_str() {
                    "running" | "restarting" | "failed" => String::new(),
                    other => format!("it reports {other:?}"),
                },
            });
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    #[test]
    fn a_span_gets_the_finest_step_that_gives_at_most_720_points() {
        let from = at("2026-10-01T00:00:00Z");
        assert_eq!(
            step_for(None, Resolution::Minutes, from, at("2026-10-01T06:00:00Z")).unwrap(),
            1,
            "six hours are 361 minutes"
        );
        assert_eq!(
            step_for(None, Resolution::Minutes, from, at("2026-10-01T12:00:00Z")).unwrap(),
            5,
            "twelve hours are 721 minutes: one too many"
        );
        assert_eq!(
            step_for(None, Resolution::Minutes, from, at("2026-10-30T00:00:00Z")).unwrap(),
            60,
            "twenty-nine days are 697 hours"
        );
        assert_eq!(
            step_for(None, Resolution::Minutes, from, at("2026-10-31T00:00:00Z")).unwrap(),
            1440,
            "thirty days are 721 hours: one too many"
        );
        assert_eq!(
            step_for(None, Resolution::Hours, from, at("2026-10-02T00:00:00Z")).unwrap(),
            60,
            "an old span is answered by the hour at the finest"
        );
        assert!(step_for(None, Resolution::Hours, from, at("2029-10-01T00:00:00Z")).is_err());
    }

    #[test]
    fn a_step_that_is_asked_for_must_be_one_of_the_four_and_fit() {
        let (from, to) = (at("2026-10-01T00:00:00Z"), at("2026-10-01T06:00:00Z"));
        assert_eq!(step_for(Some(5), Resolution::Minutes, from, to).unwrap(), 5);
        assert!(step_for(Some(7), Resolution::Minutes, from, to).is_err());
        assert!(
            step_for(Some(1), Resolution::Hours, from, to).is_err(),
            "the hours cannot give a finer step than an hour"
        );
        assert!(
            step_for(
                Some(1),
                Resolution::Minutes,
                from,
                at("2026-10-02T00:00:00Z")
            )
            .is_err(),
            "a day of minutes is more than 720 points"
        );
    }
}
