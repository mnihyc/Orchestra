//! One ordinary MCP projection for collections, individual reads and existing writers.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::nodes::view as node_view;
use crate::routes::resources::view::{Holding, addresses, environment_refs, machine_refs};
use crate::routes::resources::view::{PreparedAccess, accesses_prepared, prepare_accesses};
use orch_access_contract::decisions::Held;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_access_contract::roles::{AdminReach, PlaceRef};
use orch_api_contract::resources::{self as api, McpServer};
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::resources::mcp::{self as rows, McpRow};
use orch_db::queries::resources::routes as resource_rows;
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy)]
pub(crate) enum Style {
    /// The original collection/preset Get projection masks machine-backed preset definitions.
    Presets,
    /// The original `mcp::show` projection also serves an environment's own definition.
    Individual,
}
struct Access {
    id: Id,
    admin_only: Vec<Permission>,
    admin_access: Option<AdminReach>,
    admin_place: Option<PlaceRef>,
    participating: bool,
    owner: bool,
    administers: bool,
    held: Vec<Held>,
}
impl Access {
    fn holding(&self) -> Holding<'_> {
        Holding {
            id: self.id,
            admin_only: &self.admin_only,
            admin_access: self.admin_access,
            admin_place: self.admin_place,
            participating: self.participating,
            owner: self.owner,
            administers: self.administers,
            held: &self.held,
        }
    }
}
pub(crate) struct Prepared {
    rows: Vec<McpRow>,
    access: Vec<Access>,
    access_rows: PreparedAccess,
    subjects: HashMap<Id, Id>,
    offerings: Vec<rows::Offering>,
    style: Style,
    // Only the current ordinary preset selection supplies a proven-false Manage candidate.
    // Individual show and direct Get preserve the original unconditional native check.
    from_selection: bool,
    pub(crate) machines: Vec<Id>,
    pub(crate) environments: Vec<Id>,
    pub(crate) names: NameSources,
}
/// The existing native governor of a selected definition, before any API reference is made.
pub(crate) fn subject(row: &McpRow) -> ObjectRef {
    match row.environment_id {
        Some(id) => ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        None => ObjectRef {
            kind: ResourceKind::McpServer.into(),
            id: row.id,
        },
    }
}
fn show_offerings(style: Style, row: &McpRow) -> bool {
    matches!(style, Style::Individual) || row.imported
}

/// Read only selected ordinary rows and their raw source identities. Rich foreign references
/// are constructed later, after the calling scoped reader admits the actual sources.
pub(crate) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    seen: Option<&[resources::Seen]>,
    style: Style,
) -> ApiResult<Prepared> {
    let rows: Vec<_> = rows::servers(conn, ids)
        .await?
        .into_iter()
        .filter(|row| !matches!(style, Style::Presets) || row.environment_id.is_none())
        .collect();
    let subjects: HashMap<_, _> = rows.iter().map(|row| (row.id, subject(row).id)).collect();
    let access = if let Some(seen) = seen {
        seen.iter()
            .filter(|each| subjects.contains_key(&each.resource.id))
            .map(|each| Access {
                id: each.resource.id,
                admin_only: each.admin_only.clone(),
                admin_access: each.admin_access,
                admin_place: each.admin_place,
                participating: each.participating,
                owner: each.owner,
                administers: each.administers,
                held: each.held.clone(),
            })
            .collect()
    } else if matches!(style, Style::Individual) {
        // This is the original one-subject show path. Keep scalar effective's retained
        // and suspension semantics; the list never calls this branch per resource.
        match rows.as_slice() {
            [] => Vec::new(),
            [row] => {
                let each = resources::effective(conn, principal, subject(row), Policy::default())
                    .await
                    .map_err(ApiError::internal)?;
                vec![Access {
                    id: each.object.id,
                    admin_only: each.admin_only,
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held,
                }]
            }
            _ => {
                return Err(ApiError::internal(
                    "the individual MCP projection needs one subject",
                ));
            }
        }
    } else {
        let ids: Vec<_> = subjects.values().copied().collect();
        let mut found: HashMap<_, _> =
            resources::effective_among(conn, principal, &ids, Policy::default())
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .map(|each| (each.object, each))
                .collect();
        let mut access = Vec::new();
        for row in &rows {
            let actual = subject(row);
            let each = found.remove(&actual).ok_or_else(ApiError::not_found)?;
            access.push(Access {
                id: actual.id,
                admin_only: each.admin_only,
                admin_access: each.admin_access,
                admin_place: each.admin_place,
                participating: each.participating,
                owner: each.owner,
                administers: each.administers,
                held: each.held,
            });
        }
        access
    };
    let holdings: Vec<_> = access.iter().map(Access::holding).collect();
    let access_rows = prepare_accesses(conn, &holdings).await?;
    let mut names = access_rows.names.clone();
    names
        .nodes
        .extend(rows.iter().filter_map(|row| row.home_id));
    let offered: Vec<_> = rows
        .iter()
        .filter(|row| show_offerings(style, row))
        .map(|row| row.id)
        .collect();
    let offerings = rows::offerings(conn, &offered).await?;
    let machine_ids: Vec<_> = rows
        .iter()
        .filter_map(|row| row.machine_id)
        .chain(offerings.iter().map(|offering| offering.machine_id))
        .collect();
    let mut machines = resource_rows::live_machine_ids(conn, &machine_ids).await?;
    machines.sort_unstable();
    let mut environments: Vec<_> = rows.iter().filter_map(|row| row.environment_id).collect();
    environments.sort_unstable();
    environments.dedup();
    Ok(Prepared {
        rows,
        access,
        access_rows,
        subjects,
        offerings,
        style,
        from_selection: false,
        machines,
        environments,
        names,
    })
}

/// Only the typed API list uses its current selection on the actual repeatable snapshot.
/// Its initial autocommit preparation remains unoptimized. Generic inventory/realtime views
/// keep the original native check even when their earlier selection supplied access rows.
pub(crate) async fn prepare_for_snapshot(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    seen: &[resources::Seen],
) -> ApiResult<Prepared> {
    let mut prepared = prepare(conn, principal, ids, Some(seen), Style::Presets).await?;
    prepared.from_selection = orch_db::queries::transaction_guard::in_transaction(conn);
    Ok(prepared)
}

fn stale(offerings: &[&rows::Offering], found: &HashMap<Id, Vec<Json>>) -> bool {
    offerings
        .iter()
        .filter(|offering| offering.state == "ready")
        .any(|offering| {
            found
                .get(&offering.machine_id)
                .into_iter()
                .flatten()
                .any(|found| {
                    let origin = &offering.source;
                    found["source"] == origin["source"]
                        && (found["scope"] == origin["scope"]
                            || (found["scope"] == "directory" && origin["scope"] == "project"))
                        && found["name"] == origin["name"]
                        && found["directory"] == origin["directory"]
                        && found["digest"] != origin["digest"]
                })
        })
}

pub(crate) async fn render(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &Prepared,
    bounds: Option<&NameBounds>,
) -> ApiResult<Vec<McpServer>> {
    let ids: Vec<_> = prepared.rows.iter().map(|row| row.id).collect();
    let people: Vec<Id> = prepared
        .rows
        .iter()
        .flat_map(|row| {
            row.owner_id.into_iter().chain(
                show_offerings(prepared.style, row)
                    .then(|| {
                        row.spec
                            .pointer("/imported/by")
                            .and_then(Json::as_str)
                            .and_then(|id| id.parse().ok())
                    })
                    .flatten(),
            )
        })
        .collect();
    let people = refusals::user_refs(conn, &people).await?;
    let home_ids: Vec<_> = prepared.rows.iter().filter_map(|row| row.home_id).collect();
    let homes = match bounds {
        Some(bounds) if bounds.limited => {
            node_view::refs_with_name_sources(
                conn,
                principal,
                &home_ids,
                Policy::default(),
                &bounds.nodes,
            )
            .await?
        }
        _ => node_view::refs(conn, principal, &home_ids, Policy::default()).await?,
    };
    let preset_ids: Vec<_> = prepared
        .rows
        .iter()
        .filter(|row| row.environment_id.is_none())
        .map(|row| row.id)
        .collect();
    let addresses = addresses(conn, &preset_ids).await?;
    let environments: HashMap<_, _> =
        environment_refs(conn, &prepared.environments, &[], Time::new(ctx.now))
            .await?
            .into_iter()
            .map(|(_, reference)| (reference.id, reference))
            .collect();
    let machines = machine_refs(&ctx.app, conn, &prepared.machines).await?;
    let holdings: Vec<_> = prepared.access.iter().map(Access::holding).collect();
    let access =
        accesses_prepared(conn, principal, &holdings, &prepared.access_rows, bounds).await?;
    let enabled: HashMap<Id, i64> = sqlx::query_as::<_, (Id, i64)>(
        "SELECT wanted.id,
          (SELECT count(*) FROM settings s JOIN nodes n ON n.id=s.node_id AND n.deleted_at IS NULL WHERE s.key='mcp.enabled' AND s.value->>wanted.id::text='true') +
          (SELECT count(*) FROM environments e WHERE e.deleted_at IS NULL AND e.config->'mcp.enabled'->>wanted.id::text='true')
         FROM unnest($1::uuid[]) wanted(id)",
    ).bind(&ids).fetch_all(&mut *conn).await.map_err(DbError::from)?.into_iter().collect();
    // Group once: this endpoint drains its whole collection, so scans per server must
    // visit that server's offerings rather than the complete batch again.
    let mut by_server: HashMap<Id, Vec<&rows::Offering>> = HashMap::new();
    for offering in &prepared.offerings {
        by_server
            .entry(offering.server_id)
            .or_default()
            .push(offering);
    }
    let held_manage: HashSet<_> = prepared
        .access
        .iter()
        .filter(|each| {
            each.held
                .iter()
                .any(|held| held.permission == Permission::McpManage)
        })
        .map(|each| each.id)
        .collect();
    let mut can_manage = HashSet::new();
    for row in &prepared.rows {
        // A current View-only preset selection cannot pass Manage. Individual show/direct
        // Get still execute the original check, including its refusal and failure behavior.
        // Every actual management candidate uses that check; a held flag never grants it.
        if row.environment_id.is_none()
            && (!prepared.from_selection || held_manage.contains(&row.id))
            && by_server.contains_key(&row.id)
            && resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ResourceKind::McpServer.into(),
                    id: row.id,
                },
                Permission::McpManage,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?
            .is_allowed()
        {
            can_manage.insert(row.id);
        }
    }
    // Only the actual API selection snapshot may reuse an Environment decision. Other
    // callers keep the original per-offering check, including nontransactional realtime.
    let mut import_allowed = HashMap::new();
    let mut offering_allowed = HashMap::new();
    for offering in &prepared.offerings {
        if can_manage.contains(&offering.server_id)
            && (offering.state == "found" || offering.proposal.is_some())
        {
            if let Some(environment) = offering
                .proposal
                .as_ref()
                .and_then(|proposal| proposal["environment"].as_str())
                .and_then(|id| id.parse::<Id>().ok())
                .or(offering.environment_id)
            {
                let allowed = if prepared.from_selection {
                    match import_allowed.entry(environment) {
                        std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            let allowed = super::super::super::environments::mcp::may_import(
                                conn,
                                principal,
                                environment,
                            )
                            .await?;
                            entry.insert(allowed);
                            allowed
                        }
                    }
                } else {
                    super::super::super::environments::mcp::may_import(conn, principal, environment)
                        .await?
                };
                offering_allowed.insert((offering.server_id, offering.machine_id), allowed);
            }
        }
    }
    let imported: HashSet<_> = prepared
        .rows
        .iter()
        .filter(|row| row.spec.get("imported").is_some())
        .map(|row| row.id)
        .collect();
    let ready: Vec<_> = prepared
        .offerings
        .iter()
        .filter(|offering| offering.state == "ready" && imported.contains(&offering.server_id))
        .map(|offering| offering.machine_id)
        .collect();
    let mut found: HashMap<Id, Vec<Json>> = HashMap::new();
    for (machine, discovery) in rows::found_on_machines(conn, &ready).await? {
        found.entry(machine).or_default().push(discovery);
    }
    prepared
        .rows
        .iter()
        .map(|row| {
            let current = by_server.get(&row.id).into_iter().flatten().copied();
            let offerings = current
                .filter_map(|offering| {
                    machines.get(&offering.machine_id).cloned().map(|machine| {
                        api::McpServerOffering {
                            different_definition: offering.proposal.is_some()
                                || offering.reason.as_deref() == Some("different_definition"),
                            definition_fingerprint: offering
                                .proposal
                                .as_ref()
                                .map(|proposal| &proposal["source"])
                                .unwrap_or(&offering.source)
                                .get("definition_fingerprint")
                                .cloned()
                                .and_then(|value| serde_json::from_value(value).ok()),
                            can_use_definition: offering_allowed
                                .get(&(row.id, offering.machine_id))
                                .copied()
                                .unwrap_or(false),
                            machine,
                            state: match offering.state.as_str() {
                                "found" => api::McpOfferingState::Found,
                                "ready" => api::McpOfferingState::Ready,
                                "pending" => api::McpOfferingState::Pending,
                                _ => api::McpOfferingState::Failed,
                            },
                            offered_at: offering.offered_at.map(Time::new),
                            seen_at: Time::new(offering.seen_at),
                            reason: offering
                                .proposal
                                .as_ref()
                                .and_then(|proposal| proposal["reason"].as_str())
                                .map(str::to_owned)
                                .or_else(|| offering.reason.clone()),
                        }
                    })
                })
                .collect();
            let imported = if show_offerings(prepared.style, row) {
                if let Some(imported) = row.spec.get("imported") {
                    let by: Id = serde_json::from_value(imported["by"].clone())
                        .map_err(ApiError::internal)?;
                    Some(api::McpServerImported {
                        source: imported["source"].as_str().unwrap_or_default().to_owned(),
                        scope: serde_json::from_value(imported["scope"].clone())
                            .map_err(ApiError::internal)?,
                        directory: imported["directory"].as_str().map(str::to_owned),
                        name: imported["name"].as_str().unwrap_or_default().to_owned(),
                        at: serde_json::from_value(imported["at"].clone())
                            .map_err(ApiError::internal)?,
                        by: people.get(&by).cloned().ok_or_else(ApiError::not_found)?,
                        stale: stale(
                            by_server.get(&row.id).map_or(&[][..], Vec::as_slice),
                            &found,
                        ),
                    })
                } else {
                    None
                }
            } else {
                None
            };
            let environment = row
                .environment_id
                .and_then(|id| environments.get(&id).cloned());
            let address = addresses
                .get(&row.id)
                .cloned()
                .or_else(|| {
                    environment
                        .as_ref()
                        .map(|environment| format!("{}.{}", environment.address, row.name))
                })
                .unwrap_or_default();
            let masked = row.imported
                || (matches!(prepared.style, Style::Presets) && row.machine_id.is_some());
            Ok(McpServer {
                offerings,
                enabled_places: Some(if !row.enabled {
                    0
                } else if row.environment_id.is_some() {
                    1
                } else {
                    enabled.get(&row.id).copied().unwrap_or_default()
                }),
                id: row.id,
                name: row.name.clone(),
                description: row.description.clone(),
                org_id: row.org_id,
                owner: row.owner_id.and_then(|id| people.get(&id).cloned()),
                home: row.home_id.and_then(|id| homes.get(&id).cloned()),
                address,
                machine: row.machine_id.and_then(|id| machines.get(&id).cloned()),
                environment,
                imported,
                spec: if masked {
                    json!({"transport":row.spec["transport"]})
                } else {
                    row.spec.clone()
                },
                enabled: row.enabled,
                revision: row.revision,
                access: prepared
                    .subjects
                    .get(&row.id)
                    .and_then(|id| access.get(id))
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the MCP server's access was not read"))?,
            })
        })
        .collect()
}
