//! Mount resources, locations on hosts and personal machines, and the places and environments that use them.
//!
//! Location sources are shown only to callers who may change that location. A private layer is
//! environment data; the underlying mount is never copied by changing where an environment runs.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, placed, policy_at, view as node_view};
use crate::routes::resources::providers::{home_policies, refused_making};
use crate::routes::resources::view::{
    Holding, accesses, addresses, environment_refs, machine_refs, parse,
};
use crate::routes::resources::{Home, LIST_LIMIT, begin_mutation, record, require};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::mounts::{
    self as api, Mount, MountLocation, MountLocationSetBody, MountRef,
};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::resources::HostRef;
use orch_authz::nodes::Page;
use orch_authz::ownership::{self, Making, NewAccess};
use orch_authz::resources::{Locating, LocationAsk, LocationKind, may_add_location};
use orch_authz::usable::Policies;
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::resources::mounts::{self as rows, LocationRow, MountRow};
use orch_db::{Conn, DbError};
use orch_driver_contract::mounts::check_target;
use orch_link_contract::commands::{NodeVolumeEnsure, NodeVolumeRemove, SerialKey};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Names fit the database's agent-facing path component: two through thirty-two characters.
const NAME_MIN: usize = 2;
const NAME_MAX: usize = 32;

/// Binds all mount resource requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::MachineMountList, on_machine)
        .on(api::MachineEnvironmentDataGet, environment_data)
        .on(api::MountList, list)
        .on(api::MountCreate, create)
        .on(api::MountGet, get)
        .on(api::MountUpdate, update)
        .on(api::MountDelete, delete)
        .on(api::MountLocationSet, location_set)
        .on(api::MountLocationUpdate, location_update)
        .on(api::MountLocationDelete, location_delete)
        .on(api::MountKeptPurge, kept_purge)
        .on(api::MountUsesGet, uses);
}

fn has(holding: Option<&resources::Effective>, permission: Permission) -> bool {
    holding.is_some_and(|holding| {
        holding
            .held
            .iter()
            .any(|held| held.permission == permission)
    })
}

async fn environment_data(ctx: Ctx, (): (), (): ()) -> ApiResult<api::MachineEnvironmentData> {
    let machine = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let snapshot = rows::environment_data(&mut conn, machine)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let ids: Vec<_> = snapshot
        .environments
        .0
        .iter()
        .map(|environment| environment.id)
        .chain(snapshot.host_id)
        .collect();
    let holdings: BTreeMap<Id, resources::Effective> =
        resources::effective_among(&mut conn, &principal, &ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|holding| (holding.object.id, holding))
            .collect();
    let owns = snapshot.owner_id == Some(principal.user()) && principal.caller().reaches(None);
    let host = snapshot.host_id.and_then(|id| holdings.get(&id));
    let manages = matches!(
        orch_authz::resources::may_change_policy_key(
            &mut conn,
            &principal,
            machine,
            orch_authz::resources::KeyAbout::TheMachine,
            true,
            Policy::default()
        )
        .await
        .map_err(ApiError::internal)?,
        orch_authz::resources::Keying::Allowed
    );
    if !owns && !manages && !has(host, Permission::HostView) && !has(host, Permission::HostUse) {
        return Err(ApiError::not_found());
    }
    let roots: Vec<String> = serde_json::from_value(snapshot.roots).map_err(ApiError::internal)?;
    let mut roots: Vec<_> = roots
        .into_iter()
        .map(|path| api::EnvironmentDirectoryRoot {
            path,
            uses: Vec::new(),
        })
        .collect();
    for environment in snapshot.environments.0 {
        if !has(holdings.get(&environment.id), Permission::EnvironmentView) {
            continue;
        }
        let reference = api::MountEnvironment {
            id: environment.id,
            name: environment.name,
            status: parse(&environment.status)?,
        };
        for (role, path) in [
            (api::EnvironmentDataRole::Workspace, environment.workspace),
            (api::EnvironmentDataRole::Home, environment.home),
        ] {
            let Some(path) = path else {
                continue;
            };
            for root in &mut roots {
                if std::path::Path::new(&path).starts_with(&root.path) {
                    root.uses.push(api::EnvironmentDirectoryUse {
                        environment: reference.clone(),
                        role,
                        path: path.clone(),
                    });
                }
            }
        }
    }
    let direct = serde_json::from_value(snapshot.direct).map_err(ApiError::internal)?;
    let drivers = snapshot
        .drivers
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, facts)| {
            let storage = facts
                .get("data_storage")
                .cloned()
                .map_or_else(|| Ok(Default::default()), serde_json::from_value)
                .map_err(ApiError::internal)?;
            Ok((name.clone(), storage))
        })
        .collect::<ApiResult<BTreeMap<_, _>>>()?;
    Ok(api::MachineEnvironmentData {
        machine_id: machine,
        roots,
        direct,
        drivers,
    })
}

async fn on_machine(ctx: Ctx, (): (), (): ()) -> ApiResult<api::MachineMounts> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let snapshot = rows::on_machine(&mut conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut ids: Vec<Id> = snapshot
        .mounts
        .0
        .iter()
        .map(|mount| mount.id)
        .chain(
            snapshot
                .environments
                .0
                .iter()
                .map(|environment| environment.id),
        )
        .collect();
    ids.extend(snapshot.host_id);
    let holdings: BTreeMap<Id, resources::Effective> =
        resources::effective_among(&mut conn, &principal, &ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|holding| (holding.object.id, holding))
            .collect();
    let host = snapshot.host_id.and_then(|id| holdings.get(&id));
    let owns_machine =
        snapshot.owner_id == Some(principal.user()) && principal.caller().reaches(None);
    let manages_machine = matches!(
        orch_authz::resources::may_change_policy_key(
            &mut conn,
            &principal,
            ctx.id("id")?,
            orch_authz::resources::KeyAbout::TheMachine,
            true,
            Policy::default()
        )
        .await
        .map_err(ApiError::internal)?,
        orch_authz::resources::Keying::Allowed
    );
    if !owns_machine && !manages_machine && !has(host, Permission::HostView) {
        return Err(ApiError::not_found());
    }
    let storages = snapshot.storages.as_array().cloned().unwrap_or_default();
    let default_storage = storages
        .iter()
        .find(|storage| storage.get("default").and_then(serde_json::Value::as_bool) == Some(true))
        .and_then(|storage| storage.get("name"))
        .and_then(serde_json::Value::as_str);
    let allows_storage = |storage: Option<&str>| {
        manages_machine
            && snapshot.makes_data
            && storage.is_none_or(|name| {
                storages.iter().any(|storage| {
                    storage.get("name").and_then(serde_json::Value::as_str) == Some(name)
                })
            })
    };
    let visible: Vec<Id> = snapshot
        .mounts
        .0
        .iter()
        .filter(|mount| {
            has(holdings.get(&mount.id), Permission::MountView)
                || has(holdings.get(&mount.id), Permission::MountManage)
        })
        .map(|mount| mount.id)
        .collect();
    let named = addresses(&mut conn, &visible).await?;
    let mount_ref = |mount: &rows::MachineMountData| MountRef {
        id: mount.id,
        name: mount.name.clone(),
        address: named.get(&mount.id).cloned().unwrap_or_default(),
    };
    let mut attached: BTreeMap<Id, Vec<api::MountEnvironment>> = BTreeMap::new();
    for environment in &snapshot.environments.0 {
        if !has(holdings.get(&environment.id), Permission::EnvironmentView) {
            continue;
        }
        let ids: BTreeSet<Id> = environment
            .mounts
            .as_array()
            .into_iter()
            .flatten()
            .filter(|fact| {
                fact.get("state").and_then(serde_json::Value::as_str) == Some("attached")
            })
            .filter_map(|fact| {
                fact.get("mount_id")
                    .and_then(serde_json::Value::as_str)?
                    .parse()
                    .ok()
            })
            .collect();
        let environment = api::MountEnvironment {
            id: environment.id,
            name: environment.name.clone(),
            status: parse(&environment.status)?,
        };
        for id in ids {
            attached.entry(id).or_default().push(environment.clone());
        }
    }
    let reports: Vec<orch_link_contract::events::NodeMountSize> =
        serde_json::from_value(snapshot.sizes.clone()).unwrap_or_default();
    let sizes: BTreeMap<_, _> = reports
        .into_iter()
        .map(|report| {
            let kind = match report.kind {
                orch_link_contract::events::NodeMountKind::Path => "path".to_owned(),
                orch_link_contract::events::NodeMountKind::Volume => "volume".to_owned(),
            };
            (
                (report.mount_id, kind, report.source, report.storage),
                report.bytes,
            )
        })
        .collect();
    let mut locations = Vec::new();
    let mut available = Vec::new();
    for mount in &snapshot.mounts.0 {
        let holding = holdings.get(&mount.id);
        let path = manages_machine && has(holding, Permission::MountUse);
        if let Some(kind) = &mount.kind {
            if !has(holding, Permission::MountView) {
                continue;
            }
            let kind: api::MountLocationKind = parse(kind)?;
            let can_remove = match kind {
                api::MountLocationKind::Path => path,
                api::MountLocationKind::Volume => path,
            };
            // A stale measurement of an earlier source must not become the size of its replacement.
            let size_bytes = snapshot.host_id.and_then(|_| {
                sizes
                    .get(&(
                        mount.id,
                        match kind {
                            api::MountLocationKind::Path => "path".into(),
                            api::MountLocationKind::Volume => "volume".into(),
                        },
                        mount.source.clone().unwrap_or_default(),
                        mount.storage.clone(),
                    ))
                    .copied()
            });
            locations.push(api::MachineMount {
                mount: mount_ref(mount),
                target: mount.target.clone(),
                kind,
                source: can_remove.then(|| mount.source.clone()).flatten(),
                storage: mount.storage.clone(),
                data_kind: mount.data_kind.as_deref().map(parse).transpose()?,
                size_bytes,
                environments: attached.remove(&mount.id).unwrap_or_default(),
                can_remove,
            });
        } else if path {
            let storage_names: Vec<String> = storages
                .iter()
                .filter_map(|storage| storage.get("name").and_then(serde_json::Value::as_str))
                .filter(|storage| allows_storage(Some(storage)))
                .map(str::to_owned)
                .collect();
            let default_storage = allows_storage(default_storage);
            if path || default_storage || !storage_names.is_empty() {
                available.push(api::MachineMountChoice {
                    mount: mount_ref(mount),
                    path,
                    default_storage,
                    storages: storage_names,
                });
            }
        }
    }
    Ok(api::MachineMounts {
        locations,
        available,
    })
}

fn name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if (NAME_MIN..=NAME_MAX).contains(&value.len())
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
    {
        Ok(value.to_owned())
    } else {
        Err(invalid(
            "name",
            "use 2 to 32 lower case letters, digits, - or _, beginning with a letter",
        ))
    }
}

fn home(row: &MountRow) -> Home {
    Home {
        node: row.home_id,
        org: row.org_id,
    }
}

async fn standing(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<orch_authz::resources::Effective> {
    resources::effective(
        conn,
        principal,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
            id,
        },
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)
}

fn ask(location: &LocationRow) -> LocationAsk {
    LocationAsk {
        mount: location.mount_id,
        machine: location.machine_id,
        kind: if location.kind == "volume" {
            LocationKind::Volume
        } else {
            LocationKind::Path
        },
        storage: location.storage.clone(),
    }
}

pub(crate) async fn locating(
    conn: &mut Conn,
    principal: &Principal,
    location: &LocationRow,
) -> ApiResult<()> {
    match may_add_location(conn, principal, &ask(location), &Policies::default())
        .await
        .map_err(ApiError::internal)?
    {
        Locating::Allowed => Ok(()),
        Locating::Refused(why) => Err(refusals::denied(conn, *why).await?),
        Locating::NoSuchMachine => Err(ApiError::not_found()),
        Locating::NotTheMachinesOwner => Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "changing a location requires management of its machine",
        )),
        Locating::NoVolumeThere => Err(invalid(
            "kind",
            "this machine has not declared that Orch can make data there",
        )),
        Locating::StorageNotAllowed { storage } => Err(invalid(
            "storage",
            &format!("the host entry does not allow storage {storage}"),
        )),
    }
}

/// Shows a mount through its resource page and filters its locations by machine visibility.
pub(crate) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Mount> {
    let row = rows::mounts(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let effective = standing(conn, principal, id).await?;
    let access = accesses(
        conn,
        principal,
        &[Holding {
            admin_only: &effective.admin_only,
            admin_access: effective.admin_access,
            admin_place: effective.admin_place,
            participating: effective.participating,
            id,
            owner: effective.owner,
            administers: effective.administers,
            held: &effective.held,
        }],
    )
    .await?;
    let addresses = addresses(conn, &[id]).await?;
    let homes = node_view::refs(
        conn,
        principal,
        &row.home_id.into_iter().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let known_locations = rows::locations(conn, &[id]).await?;
    let locations_total = known_locations.len();
    let mut locations = Vec::new();
    for location in known_locations {
        let Some((machine, host)) = seen(ctx, conn, principal, location.machine_id).await? else {
            continue;
        };
        let changeable = matches!(
            may_add_location(conn, principal, &ask(&location), &Policies::default())
                .await
                .map_err(ApiError::internal)?,
            Locating::Allowed
        );
        let by = refusals::user_refs(conn, &[location.created_by]).await?;
        let sizes: Json = sqlx::query_scalar(
            "SELECT COALESCE(facts->'mount_sizes','[]'::jsonb) FROM machines WHERE id=$1",
        )
        .bind(location.machine_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let reports: Vec<orch_link_contract::events::NodeMountSize> =
            serde_json::from_value(sizes).unwrap_or_default();
        let measured_bytes = reports
            .iter()
            .find(|report| {
                let kind = match report.kind {
                    orch_link_contract::events::NodeMountKind::Path => "path",
                    orch_link_contract::events::NodeMountKind::Volume => "volume",
                };
                report.mount_id == id
                    && kind == location.kind
                    && report.source == location.source
                    && report.storage == location.storage
            })
            .and_then(|report| i64::try_from(report.bytes).ok());
        locations.push(MountLocation {
            measured_bytes,
            machine,
            host,
            kind: parse(&location.kind)?,
            storage: location.storage,
            source: changeable.then_some(location.source),
            data_kind: location.data_kind.as_deref().map(parse).transpose()?,
            can_remove: changeable,
            created_by: by
                .get(&location.created_by)
                .cloned()
                .ok_or_else(ApiError::not_found)?,
            created_at: Time::new(location.created_at),
        });
    }
    let mut kept = Vec::new();
    for data in rows::kept(conn, &[id]).await? {
        let Some((machine, host)) = seen(ctx, conn, principal, data.machine_id).await? else {
            continue;
        };
        let can_purge = matches!(
            may_add_location(conn, principal, &kept_ask(&data), &Policies::default())
                .await
                .map_err(ApiError::internal)?,
            Locating::Allowed
        );
        let by = refusals::user_refs(conn, &[data.kept_by]).await?;
        kept.push(api::MountKeptData {
            id: data.id,
            machine,
            host,
            kind: parse(&data.kind)?,
            storage: data.storage,
            source: can_purge.then_some(data.source),
            data_kind: data.data_kind.as_deref().map(parse).transpose()?,
            file_count: data.file_count,
            total_bytes: data.total_bytes,
            kept_by: by
                .get(&data.kept_by)
                .cloned()
                .ok_or_else(ApiError::not_found)?,
            kept_at: Time::new(data.kept_at),
            can_purge,
        });
    }
    let people = refusals::user_refs(conn, &row.owner_id.into_iter().collect::<Vec<_>>()).await?;
    let lost_locations = if effective
        .held
        .iter()
        .any(|held| held.permission == Permission::MountManage)
    {
        rows::lost_locations(conn, id)
            .await?
            .into_iter()
            .map(|lost| api::MountLostLocation {
                id: lost.id,
                machine_name: lost.machine_name,
                host_name: lost.host_name,
                storage: lost.storage,
                total_bytes: lost.total_bytes,
                size_at: lost.size_at.map(Time::new),
                lost_at: Time::new(lost.lost_at),
            })
            .collect()
    } else {
        Vec::new()
    };
    let attached_environments: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT e.id) FROM environments e CROSS JOIN LATERAL
          jsonb_array_elements(CASE WHEN jsonb_typeof(e.facts->'mounts')='array' THEN e.facts->'mounts' ELSE '[]'::jsonb END) attachment
         WHERE e.deleted_at IS NULL AND e.status <> 'removed' AND attachment->>'mount_id'=$1::uuid::text AND attachment->>'state'='attached'",
    ).bind(id).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    Ok(Mount {
        lost_locations,
        attached_environments: Some(attached_environments),
        base: MountRef {
            id,
            name: row.name,
            address: addresses.get(&id).cloned().unwrap_or_default(),
        },
        description: row.description,
        org_id: row.org_id,
        home: row.home_id.and_then(|id| homes.get(&id).cloned()),
        owner: row.owner_id.and_then(|id| people.get(&id).cloned()),
        target: row.target,
        read_only: row.read_only,
        enabled: row.enabled,
        default_fate: parse(&row.default_fate)?,
        locations_hidden: Some(locations_total - locations.len()),
        locations,
        kept,
        revision: row.revision,
        access: access
            .get(&id)
            .cloned()
            .ok_or_else(|| ApiError::internal("the mount's access was not read"))?,
    })
}

/// A machine as the caller may see it beside a mount, with its host; `None` when it is not theirs to see.
async fn seen(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    machine_id: Id,
) -> ApiResult<Option<(orch_api_contract::resources::MachineRef, Option<HostRef>)>> {
    let Some(machine) = orch_db::queries::environments::location_machine(conn, machine_id).await?
    else {
        return Ok(None);
    };
    let object = match machine.host_id {
        Some(host) => ObjectRef {
            kind: ResourceKind::Host.into(),
            id: host,
        },
        None => ObjectRef {
            kind: ResourceKind::Machine.into(),
            id: machine_id,
        },
    };
    let holding = resources::effective(conn, principal, object, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let visible = machine.host_id.is_none()
        && machine.owner_id == Some(principal.user())
        && principal.caller().reaches(None)
        || holding.held.iter().any(|held| {
            matches!(
                held.permission,
                Permission::HostView
                    | Permission::HostUse
                    | Permission::HostManage
                    | Permission::MachineView
                    | Permission::MachineUse
                    | Permission::MachineManage
            )
        });
    if !visible {
        return Ok(None);
    }
    let machines = machine_refs(&ctx.app, conn, &[machine_id]).await?;
    let reference = machines
        .get(&machine_id)
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    let host = match machine.host_id {
        Some(host) => {
            let name: String = sqlx::query_scalar("SELECT name FROM hosts WHERE id=$1")
                .bind(host)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            Some(HostRef {
                id: host,
                name,
                address: addresses_for_host(conn, host).await?,
            })
        }
        None => None,
    };
    Ok(Some((reference, host)))
}

fn kept_ask(data: &rows::KeptRow) -> LocationAsk {
    LocationAsk {
        mount: data.mount_id,
        machine: data.machine_id,
        kind: if data.kind == "volume" {
            LocationKind::Volume
        } else {
            LocationKind::Path
        },
        storage: data.storage.clone(),
    }
}

async fn addresses_for_host(conn: &mut Conn, id: Id) -> ApiResult<String> {
    Ok(addresses(conn, &[id])
        .await?
        .remove(&id)
        .unwrap_or_default())
}

async fn list(
    ctx: Ctx,
    query: api::MountListQuery,
    (): (),
) -> ApiResult<Vec<orch_api_contract::common::Listed<Mount>>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut after = None;
    let mut result = Vec::new();
    let mut private = Vec::new();
    loop {
        let visible = resources::visible(
            &mut conn,
            &principal,
            ResourceKind::Mount,
            Page {
                after,
                limit: LIST_LIMIT,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        let homes: Vec<Id> = visible
            .items
            .iter()
            .filter_map(|seen| seen.resource.home)
            .collect();
        let inside = match query.org_id {
            Some(org) => {
                orch_db::queries::resources::routes::homes_under(&mut conn, org, &homes).await?
            }
            None => homes,
        };
        let mut seen: Vec<_> = visible
            .items
            .into_iter()
            .filter(|seen| seen.resource.home.is_none_or(|home| inside.contains(&home)))
            .collect();
        private.extend(super::hidden_items(&mut conn, &principal, &mut seen).await?);
        for seen in seen {
            result.push(show(&ctx, &mut conn, &principal, seen.resource.id).await?);
        }
        after = visible.next;
        if after.is_none() {
            break;
        }
    }
    private.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(super::listed(result, private))
}

async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Mount> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(
        &mut conn,
        &principal,
        ResourceKind::Mount,
        id,
        Permission::MountView,
    )
    .await?;
    show(&ctx, &mut conn, &principal, id).await
}

async fn create(ctx: Ctx, (): (), body: api::MountCreateBody) -> ApiResult<Mount> {
    let name = name(&body.name)?;
    check_target(&body.target).map_err(|error| invalid("target", &error.to_string()))?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let (home_org, members, policy) = match body.home_id {
        Some(home) => {
            let (place, kind) = placed(&mut tx, home).await?;
            if kind != NodeKind::Org {
                return Err(invalid("home_id", "a mount is at home in an org"));
            }
            (
                Some(place.org),
                home_policies(&mut tx, home).await?.members_make,
                policy_at(&mut tx, home).await?,
            )
        }
        None => (None, Vec::new(), Policy::default()),
    };
    match ownership::may_make(
        &mut tx,
        &principal,
        ResourceKind::Mount,
        body.home_id,
        &members,
        policy,
    )
    .await
    {
        Ok(Making::Allowed(_)) => {}
        Ok(Making::Refused(why)) => {
            return Err(match body.home_id {
                Some(home) => refused_making(&mut tx, home, why).await,
                None => {
                    refusals::system_admin_only("only a system admin makes a mount of the system")
                }
            });
        }
        Err(error) => return Err(refusals::problem(&mut tx, error).await),
    }
    let row = MountRow {
        id: orch_types::new_id(),
        name,
        description: body.description.unwrap_or_default().trim().to_owned(),
        org_id: home_org,
        home_id: body.home_id,
        owner_id: Some(principal.user()),
        target: body.target,
        read_only: body.read_only.unwrap_or(true),
        enabled: true,
        default_fate: fate_word(body.default_fate.unwrap_or_default()).to_owned(),
        revision: 1,
    };
    rows::insert(&mut tx, &row, ctx.now).await?;
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
            id: row.id,
        },
        principal.user(),
        match body.access {
            None | Some(NewResourceAccess::Defaults) => NewAccess::Defaults,
            Some(NewResourceAccess::OnlyMe) => NewAccess::OnlyMe,
            Some(NewResourceAccess::Home) => NewAccess::Home,
        },
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MountCreated),
        row.id,
        home(&row),
        &format!("Added the mount {}", row.name),
        json!({"mount": row.id, "name": row.name}),
    )
    .await?;
    let result = show(&ctx, &mut tx, &principal, row.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn update(ctx: Ctx, (): (), body: api::MountUpdateBody) -> ApiResult<Mount> {
    let id = ctx.id("id")?;
    let cleaned = body.name.as_deref().map(name).transpose()?;
    if let Some(target) = &body.target {
        check_target(target).map_err(|error| invalid("target", &error.to_string()))?;
    }
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Mount,
        id,
        Permission::MountManage,
    )
    .await?;
    let mut row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.revision != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the mount changed since it was read",
        )
        .with("current", show(&ctx, &mut tx, &principal, id).await?));
    }
    let old = (
        row.name.clone(),
        row.description.clone(),
        row.target.clone(),
        row.read_only,
        row.enabled,
        row.default_fate.clone(),
    );
    if let Some(name) = cleaned {
        row.name = name;
    }
    if let Some(description) = body.description {
        row.description = description.trim().to_owned();
    }
    if let Some(target) = body.target {
        row.target = target;
    }
    if let Some(read_only) = body.read_only {
        row.read_only = read_only;
    }
    if let Some(enabled) = body.enabled {
        row.enabled = enabled;
    }
    if let Some(fate) = body.default_fate {
        fate_word(fate).clone_into(&mut row.default_fate);
    }
    if old
        != (
            row.name.clone(),
            row.description.clone(),
            row.target.clone(),
            row.read_only,
            row.enabled,
            row.default_fate.clone(),
        )
    {
        rows::update(&mut tx, &row, ctx.now).await?;
        if old.2 != row.target || old.3 != row.read_only || old.4 != row.enabled {
            let candidates = rows::candidates(&mut tx, id).await?;
            rows::changed(&mut tx, &candidates, ctx.now).await?;
        }
        record(&ctx, &mut tx, orch_db::audit::action!(AuditAction::MountUpdated), id, home(&row), &format!("Changed the mount {}", row.name), json!({"mount": id, "name": row.name, "target": row.target, "read_only": row.read_only, "enabled": row.enabled, "default_fate": row.default_fate})).await?;
    }
    let result = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Err(error) = ctx.app.engine.mount_resource_changed(id).await {
        tracing::warn!(mount = %id, %error, "mount changes remain pending");
    }
    Ok(result)
}

async fn uses_of(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<api::MountUsesGetReply> {
    let scopes = rows::scopes(conn, id).await?;
    let refs = node_view::refs(
        conn,
        principal,
        &scopes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let mut result = api::MountUsesGetReply {
        scopes: Vec::new(),
        environments: Vec::new(),
        hidden: 0,
    };
    for (id, value) in scopes {
        match refs.get(&id).filter(|node| !node.redacted) {
            Some(node) => result.scopes.push(api::MountScope {
                node: node.clone(),
                value: serde_json::from_value(value).map_err(ApiError::internal)?,
            }),
            None => result.hidden += 1,
        }
    }
    for environment in rows::candidates(conn, id).await? {
        let enabled: Option<orch_types::Json> = orch_settings::get(
            conn,
            orch_settings::Place::Environment(environment),
            orch_settings_contract::registry::keys::ENV_MOUNTS,
        )
        .await
        .map_err(ApiError::internal)?;
        if !enabled
            .as_ref()
            .and_then(|mounts| mounts.get(id.to_string()))
            .is_some_and(|value| *value != json!(false))
        {
            continue;
        }
        match crate::routes::environments::reading::decide(conn, principal, environment).await {
            Ok(_) => {
                let refs = environment_refs(conn, &[environment], &[], Time::new(ctx.now)).await?;
                if let Some((_, environment)) = refs.into_iter().next() {
                    let facts: Option<orch_types::Json> =
                        sqlx::query_scalar("SELECT facts FROM environments WHERE id=$1")
                            .bind(environment.id)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(DbError::from)?;
                    let state = facts
                        .as_ref()
                        .and_then(|facts| facts.get("mounts"))
                        .and_then(orch_types::Json::as_array)
                        .and_then(|mounts| {
                            mounts
                                .iter()
                                .find(|mount| mount.get("mount_id") == Some(&json!(id)))
                        })
                        .and_then(|mount| mount.get("state"))
                        .and_then(orch_types::Json::as_str)
                        .unwrap_or("pending")
                        .to_owned();
                    result.environments.push(api::MountUse {
                        environment,
                        state,
                        workspace: false,
                    });
                }
            }
            Err(_) => result.hidden += 1,
        }
    }
    Ok(result)
}

async fn uses(ctx: Ctx, (): (), (): ()) -> ApiResult<api::MountUsesGetReply> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(
        &mut conn,
        &principal,
        ResourceKind::Mount,
        id,
        Permission::MountView,
    )
    .await?;
    uses_of(&ctx, &mut conn, &principal, id).await
}

async fn volume(ctx: &Ctx, conn: &mut Conn, location: &LocationRow, remove: bool) -> ApiResult<()> {
    let machine = location.machine_id;
    let (driver, storages) = rows::data_driver(conn, machine)
        .await?
        .ok_or_else(|| invalid("kind", "this machine has not offered making data"))?;
    if let Some(storage) = &location.storage
        && !storages.as_array().is_some_and(|storages| {
            storages
                .iter()
                .any(|row| row.get("name").and_then(orch_types::Json::as_str) == Some(storage))
        })
    {
        return Err(invalid("storage", "this host has no storage of that name"));
    }
    let labels = BTreeMap::from([("orch.mount_id".to_owned(), location.mount_id.to_string())]);
    let key = SerialKey::Volume(location.source.clone());
    if remove {
        ctx.app
            .link
            .dispatcher()
            .run(
                machine,
                &key,
                &NodeVolumeRemove {
                    driver,
                    name: location.source.clone(),
                    storage: location.storage.clone(),
                    labels,
                },
            )
            .await
            .map_err(ApiError::internal)?;
    } else {
        ctx.app
            .link
            .dispatcher()
            .run(
                machine,
                &key,
                &NodeVolumeEnsure {
                    driver,
                    name: location.source.clone(),
                    storage: location.storage.clone(),
                    labels,
                },
            )
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(())
}

async fn running_uses(conn: &mut Conn, mount: Id, machine: Id) -> ApiResult<Vec<Id>> {
    Ok(sqlx::query_scalar("SELECT DISTINCT e.id FROM environments e LEFT JOIN runtimes r ON r.environment_id=e.id AND r.state<>'removed'
        LEFT JOIN hosts h ON h.id=r.host_id WHERE (r.id IS NOT NULL OR (e.deleted_at IS NULL AND e.status IN ('running','starting','offline','failed')))
        AND (e.machine_id=$2 OR h.machine_id=$2) AND (EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(e.facts->'mounts','[]')) f
        WHERE f->>'mount_id'=$1::text AND f->>'state'='attached') OR e.config->'env.mounts' ? $1::text)")
        .bind(mount).bind(machine).fetch_all(conn).await.map_err(DbError::from)?)
}

async fn location_set(ctx: Ctx, (): (), body: MountLocationSetBody) -> ApiResult<Mount> {
    let id = ctx.id("id")?;
    let machine = ctx.id("machine_id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let located = orch_db::queries::environments::location_machine(&mut tx, machine)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (kind, source, storage) = match body {
        MountLocationSetBody::Path { source } => {
            check_target(&source).map_err(|error| invalid("source", &error.to_string()))?;
            ("path".to_owned(), source, None)
        }
        MountLocationSetBody::Volume { storage } => {
            ("volume".to_owned(), format!("orch-mount-{id}"), storage)
        }
    };
    let location = LocationRow {
        mount_id: id,
        machine_id: machine,
        host_id: located.host_id,
        host_name: None,
        kind,
        source,
        storage,
        data_kind: None,
        created_by: principal.user(),
        created_at: ctx.now,
    };
    locating(&mut tx, &principal, &location).await?;
    if location.kind == "path" {
        let machine = orch_db::queries::resources::machine(&mut tx, location.machine_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let policy = super::machines::reported_policy(&mut tx, &machine)
            .await?
            .ok_or_else(|| {
                invalid(
                    "source",
                    "this machine has not offered any environment.roots",
                )
            })?;
        policy
            .check_root(std::path::Path::new(&location.source))
            .map_err(|_| {
                invalid(
                    "source",
                    "the mount path is outside this machine's environment.roots",
                )
            })?;
    }
    if let Some(previous) = rows::locations(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|location| location.machine_id == machine)
    {
        if previous.kind == location.kind
            && previous.source == location.source
            && previous.storage == location.storage
        {
            return show(&ctx, &mut tx, &principal, id).await;
        }
        locating(&mut tx, &principal, &previous).await?;
        if previous.kind == "volume" {
            return Err(invalid(
                "kind",
                "remove the old volume location with its confirmation first",
            ));
        }
    }
    if location.kind == "volume" {
        volume(&ctx, &mut tx, &location, false).await?;
    }
    rows::set_location(&mut tx, &location).await?;
    let candidates = rows::candidates(&mut tx, id).await?;
    rows::changed(&mut tx, &candidates, ctx.now).await?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MountLocationAdded),
        id,
        home(&row),
        "Added a mount location",
        json!({"mount": id,"machine": machine,"kind": location.kind}),
    )
    .await?;
    let result = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Err(error) = ctx.app.engine.mount_resource_changed(id).await {
        tracing::warn!(mount = %id, %error, "mount location changes remain pending");
    }
    Ok(result)
}

/// The stored word of a fate, as the database's check names it.
fn fate_word(fate: api::MountFate) -> &'static str {
    match fate {
        api::MountFate::AsIs => "as_is",
        api::MountFate::Detach => "detach",
        api::MountFate::Copy => "copy",
    }
}

/// The stored word of a data kind, as the database's check names it.
fn data_kind_word(kind: api::MountDataKind) -> &'static str {
    match kind {
        api::MountDataKind::Own => "own",
        api::MountDataKind::Shared => "shared",
        api::MountDataKind::Synced => "synced",
    }
}

/// Only the hint changes, so a manager of the mount sets it without managing the machine; the location's data and
/// every environment that attaches it stay as they are.
async fn location_update(ctx: Ctx, (): (), body: api::MountLocationUpdateBody) -> ApiResult<Mount> {
    let id = ctx.id("id")?;
    let machine = ctx.id("machine_id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Mount,
        id,
        Permission::MountManage,
    )
    .await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let previous = rows::locations(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|location| location.machine_id == machine)
        .ok_or_else(ApiError::not_found)?;
    let word = body.data_kind.map(data_kind_word);
    if previous.data_kind.as_deref() != word {
        if !rows::set_data_kind(&mut tx, id, machine, word).await? {
            return Err(ApiError::not_found());
        }
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::MountUpdated),
            id,
            home(&row),
            "Described a mount location's data",
            json!({"mount": id, "machine": machine, "data_kind": word}),
        )
        .await?;
    }
    let result = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn remove_location(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    location: &LocationRow,
    confirm: Option<&str>,
    row: &MountRow,
) -> ApiResult<()> {
    locating(conn, principal, location).await?;
    if location.kind == "volume" {
        if confirm != Some(row.name.as_str()) {
            return Err(invalid(
                "confirm",
                "type the mount's name before deleting its volume and data",
            ));
        }
        let running = running_uses(conn, row.id, location.machine_id).await?;
        if !running.is_empty() {
            return Err(ApiError::new(
                ErrorCode::ResourceInUse,
                "environments still hold this data on the machine; detach it and remove any retained unit before purging it",
            )
            .with("environments", running.len()));
        }
        volume(ctx, conn, location, true).await?;
    }
    rows::remove_location(conn, row.id, location.machine_id).await?;
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::MountLocationRemoved),
        row.id,
        home(row),
        "Removed a mount location",
        json!({"mount": row.id,"machine": location.machine_id,"kind": location.kind}),
    )
    .await
}

/// Purges kept data under the same authority and confirmation as removing that machine's location: a volume goes
/// with its data once the mount's name is typed, a directory is only forgotten.
async fn kept_purge(ctx: Ctx, query: api::MountLocationDeleteQuery, (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let kept_id = ctx.id("kept_id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let data = rows::kept(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|data| data.id == kept_id)
        .ok_or_else(ApiError::not_found)?;
    let as_location = LocationRow {
        mount_id: id,
        machine_id: data.machine_id,
        host_id: None,
        host_name: None,
        kind: data.kind.clone(),
        source: data.source.clone(),
        storage: data.storage.clone(),
        data_kind: data.data_kind.clone(),
        created_by: data.kept_by,
        created_at: data.kept_at,
    };
    locating(&mut tx, &principal, &as_location).await?;
    if data.kind == "volume" {
        if query.confirm.as_deref() != Some(row.name.as_str()) {
            return Err(invalid(
                "confirm",
                "type the mount's name before deleting its kept volume and data",
            ));
        }
        volume(&ctx, &mut tx, &as_location, true).await?;
    }
    rows::forget_kept(&mut tx, kept_id).await?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MountLocationRemoved),
        id,
        home(&row),
        "Purged a mount's kept data",
        json!({"mount": id, "machine": data.machine_id, "kind": data.kind, "kept": kept_id}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

async fn location_delete(ctx: Ctx, query: api::MountLocationDeleteQuery, (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let machine = ctx.id("machine_id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let location = rows::locations(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|location| location.machine_id == machine)
        .ok_or_else(ApiError::not_found)?;
    remove_location(
        &ctx,
        &mut tx,
        &principal,
        &location,
        query.confirm.as_deref(),
        &row,
    )
    .await?;
    let candidates = rows::candidates(&mut tx, id).await?;
    rows::changed(&mut tx, &candidates, ctx.now).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Err(error) = ctx.app.engine.mount_resource_changed(id).await {
        tracing::warn!(mount = %id, %error, "mount removal remains pending");
    }
    Ok(())
}

async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Mount,
        id,
        Permission::MountManage,
    )
    .await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let used = uses_of(&ctx, &mut tx, &principal, id).await?;
    if !used.scopes.is_empty() || !used.environments.is_empty() || used.hidden > 0 {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "places or environments still enable this mount",
        )
        .with("uses", used));
    }
    let locations = rows::locations(&mut tx, &[id]).await?;
    // Volume data requires a separately confirmed location removal before deleting the resource.
    if locations.iter().any(|location| location.kind == "volume")
        || rows::kept(&mut tx, &[id])
            .await?
            .iter()
            .any(|data| data.kind == "volume")
    {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "remove the mount's volume locations with their confirmations first",
        ));
    }
    for location in locations {
        remove_location(&ctx, &mut tx, &principal, &location, None, &row).await?;
    }
    rows::delete(&mut tx, id, ctx.now).await?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MountDeleted),
        id,
        home(&row),
        "Deleted a mount",
        json!({"mount": id,"name": row.name}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
