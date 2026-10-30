//! Host deletion gives each existing data source its own fate before physical erasure.

use super::{ApiError, ApiResult, Ctx, Principal, blocked};
use orch_api_contract::{loss as api, resources::HostRef};
use orch_db::{
    Conn,
    queries::{
        environments::retained,
        resources::{hosting as hosts, mounts},
    },
};
use orch_engine::{
    jobs::host_delete::{Custody, MountData, NativeData},
    resource_facts,
};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Private accepted data identities accompany their public rows, never caller paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Planned {
    /// The exact safe rows the caller reviewed.
    pub rows: Vec<api::HostDataRow>,
    /// Private exact sources and accepted physical cleanup intent.
    pub custody: Custody,
    /// Existing retained-file revisions reserved by early purge.
    pub purges: Vec<(Id, i64)>,
}

impl Planned {
    /// Compares source and destination identities without newly generated copy names.
    pub(super) fn unchanged(&self, fresh: &Self) -> bool {
        if self.purges != fresh.purges
            || self.rows.len() != fresh.rows.len()
            || self.custody.mounts.len() != fresh.custody.mounts.len()
        {
            return false;
        }
        self.custody
            .copies
            .copies
            .iter()
            .zip(&fresh.custody.copies.copies)
            .all(|(old, new)| {
                old.mount_id == new.mount_id
                    && old.target_machine == new.target_machine
                    && old.from == new.from
                    && old.kept.as_ref().map(|kept| &kept.previous)
                        == new.kept.as_ref().map(|kept| &kept.previous)
            })
            && self.custody.copies.copies.len() == fresh.custody.copies.copies.len()
            && self
                .custody
                .mounts
                .iter()
                .zip(&fresh.custody.mounts)
                .all(|(old, new)| {
                    old.kept == new.kept
                        && old.fate == new.fate
                        && old.location.mount_id == new.location.mount_id
                        && old.location.machine_id == new.location.machine_id
                        && old.location.kind == new.location.kind
                        && old.location.source == new.location.source
                        && old.location.storage == new.location.storage
                        && old.native == new.native
                        && old.row.host == new.row.host
                })
            && self.rows.iter().zip(&fresh.rows).all(|(old, new)| {
                old.id == new.id
                    && old.selected == new.selected
                    && old.until == new.until
                    && old.resource == new.resource
                    && old.label == new.label
                    && old.kind == new.kind
                    && old.host == new.host
            })
    }
}

/// Reuses current facts, ordinary location authority and the existing copy planner.
pub(super) async fn plan(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    host: Id,
    body: &api::HostDeletePreflightBody,
    facts: &resource_facts::custody::HostCustody,
) -> ApiResult<Planned> {
    use api::HostDataFate as Fate;
    let mut sources: Vec<_> = facts
        .locations
        .iter()
        .cloned()
        .map(|row| (row, None, None))
        .collect();
    sources.extend(facts.mount_kept.iter().map(|row| {
        (
            mounts::LocationRow {
                mount_id: row.mount_id,
                machine_id: row.machine_id,
                host_id: Some(host),
                host_name: None,
                kind: row.kind.clone(),
                source: row.source.clone(),
                storage: row.storage.clone(),
                data_kind: row.data_kind.clone(),
                created_by: row.kept_by,
                created_at: row.kept_at,
            },
            Some(row.id),
            row.total_bytes,
        )
    }));
    let ids: Vec<_> = sources
        .iter()
        .map(|(row, _, _)| row.mount_id)
        .chain(facts.kept.iter().map(|row| row.id))
        .collect();
    let rights = resource_facts::authorities(conn, principal, &ids)
        .await
        .map_err(ApiError::internal)?;
    let mounts: BTreeMap<_, _> = mounts::mounts(conn, &ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let mut rows = Vec::new();
    let mut selected = Vec::new();
    let mut copies = orch_engine::mount_copies::MountDecisions::default();
    let mut purges = Vec::new();
    for (source, kept, bytes) in sources {
        let id = kept.map_or_else(
            || format!("mount:{}", source.mount_id),
            |id| format!("mount-kept:{id}"),
        );
        let choice = body.data.iter().find(|row| row.id == id);
        let fate = choice.map_or(
            if body.mounts == api::HostMountCascade::DeleteData {
                Fate::Delete
            } else {
                Fate::Leave
            },
            |row| row.fate,
        );
        let name = rights
            .iter()
            .find(|right| right.resource.id == source.mount_id && right.readable)
            .and_then(|_| mounts.get(&source.mount_id))
            .map(|row| row.name.clone());
        let mut row = api::HostDataRow {
            id,
            resource: api::LossResource {
                id: source.mount_id,
                name,
            },
            label: None,
            kind: "mount".into(),
            bytes,
            until: None,
            selected: fate,
            choices: vec![Fate::Leave, Fate::Move, Fate::Delete],
            host: None,
            copy: None,
            blockers: vec![],
        };
        if !row.choices.contains(&fate) {
            row.blockers.push(blocked(
                "data_fate_unavailable",
                "Choose Leave, Move or Delete for mount data",
            ));
        }
        if fate != Fate::Leave && !facts.inventory.online {
            row.blockers.push(blocked(
                "source_offline",
                "The source must answer before its data can be moved or deleted",
            ));
        }
        if matches!(fate, Fate::Move | Fate::Delete)
            && super::super::mounts::locating(conn, principal, &source)
                .await
                .is_err()
        {
            row.blockers.push(blocked(
                "access.denied",
                "This data needs its mount's and machine's location authority",
            ));
        }
        if fate == Fate::Delete && source.kind != "volume" {
            row.blockers.push(blocked(
                "borrowed_directory",
                "Orch does not erase an existing directory it was given",
            ));
        }
        if fate == Fate::Delete && row.resource.name.is_none() {
            row.blockers.push(blocked(
                "access.denied",
                "Read this mount's name before confirming deletion of its data",
            ));
        }
        if fate == Fate::Move {
            let target = choice.and_then(|row| row.host_id).or(body.host_id);
            if let Some(target) = target.filter(|target| *target != host) {
                if let Some(destination) = hosts::host(conn, target).await? {
                    let allowed = orch_authz::resources::check(
                        conn,
                        principal,
                        orch_access_contract::decisions::ObjectRef {
                            kind: orch_access_contract::permissions::ObjectKind::Host,
                            id: target,
                        },
                        orch_access_contract::permissions::Permission::HostUse,
                        Default::default(),
                    )
                    .await
                    .map_err(ApiError::internal)?
                    .is_allowed();
                    let available:bool=sqlx::query_scalar("SELECT online AND status='active' AND deleted_at IS NULL AND lost_at IS NULL FROM machines WHERE id=$1").bind(destination.machine_id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
                    if !allowed {
                        row.blockers.push(blocked(
                            "access.denied",
                            "Use of the destination host is required",
                        ));
                    } else if !available || !destination.schedulable {
                        row.blockers.push(blocked(
                            "target_unavailable",
                            "Choose an online host that accepts work",
                        ));
                    } else {
                        row.host = Some(HostRef {
                            id: target,
                            name: destination.name,
                            address: String::new(),
                        });
                        if row.blockers.is_empty() {
                            let decided = orch_engine::mount_copies::decide_location_copy(
                                &ctx.app.engine,
                                conn,
                                principal,
                                &source,
                                destination.machine_id,
                                ctx.now,
                            )
                            .await
                            .map_err(ApiError::internal)?;
                            row.copy = decided
                                .fated
                                .into_iter()
                                .find(|row| row.mount_id == source.mount_id)
                                .and_then(|row| row.copy);
                            if !row.copy.as_ref().is_some_and(|copy| copy.measured) {
                                row.blockers.push(blocked(
                                    "measurement_pending",
                                    "Waiting for this data's copy count",
                                ));
                            }
                            row.blockers.extend(decided.blockers);
                            copies.copies.extend(decided.decisions.copies);
                        }
                    }
                } else {
                    row.blockers.push(blocked(
                        "target_unavailable",
                        "The destination host is unavailable",
                    ));
                }
            } else {
                row.blockers.push(blocked(
                    "destination_required",
                    "Choose another host for this data",
                ));
            }
        }
        let native = facts
            .native
            .as_ref()
            .and_then(|inventory| {
                inventory.resources.iter().find(|object| {
                    object.resource.kind == orch_driver_contract::resources::ResourceKind::Volume
                        && object.name == source.source
                        && object
                            .labels
                            .get("orch.mount_id")
                            .is_some_and(|id| id == &source.mount_id.to_string())
                })
            })
            .map(|object| object.resource.clone());
        if matches!(fate, Fate::Move | Fate::Delete) && source.kind == "volume" && native.is_none()
        {
            row.blockers.push(blocked(
                "inventory_unrecorded",
                "The node has not reported an owned identity for this volume",
            ));
        }
        selected.push(MountData {
            location: source,
            kept,
            fate,
            row: row.clone(),
            native,
        });
        rows.push(row);
    }
    for promise in &facts.kept {
        let id = format!("environment:{}", promise.id);
        let choice = body.data.iter().find(|row| row.id == id);
        let fate = choice.map_or(Fate::Wait, |row| row.fate);
        let right = rights.iter().find(|right| right.resource.id == promise.id);
        let mut row = api::HostDataRow {
            id,
            resource: api::LossResource {
                id: promise.id,
                name: right
                    .filter(|right| right.readable || right.manageable)
                    .map(|right| right.resource.name.clone()),
            },
            label: None,
            kind: "environment".into(),
            bytes: promise.retained_bytes,
            until: promise.volumes_purge_at.map(Time::new),
            selected: fate,
            choices: vec![Fate::Wait, Fate::Purge],
            host: None,
            copy: None,
            blockers: vec![],
        };
        if !row.choices.contains(&fate) {
            row.blockers.push(blocked(
                "data_fate_unavailable",
                "Choose Wait or Purge now for retained environment data",
            ));
        }
        if fate == Fate::Purge {
            let (manager, admin) =
                orch_engine::jobs::retained::authority(conn, principal, promise.id)
                    .await
                    .map_err(ApiError::internal)?;
            if !admin && !(manager && promise.volumes_purge_at.is_some_and(|at| at <= ctx.now)) {
                row.blockers.push(blocked(
                    "access.denied",
                    "Only its current home administrator may shorten this recovery window",
                ));
            }
            if !facts.inventory.online {
                row.blockers.push(blocked(
                    "source_offline",
                    "Its host must answer before the retained data can be purged",
                ));
            }
            if promise.removal_phase.as_deref() != Some("kept") {
                row.blockers.push(blocked(
                    "in_use",
                    "Another restore or purge owns these files",
                ));
            }
            if row.blockers.is_empty() {
                purges.push((promise.id, promise.removal_revision));
            }
        }
        rows.push(row);
    }
    let mut native_data = Vec::new();
    if let Some(native) = &facts.native {
        use orch_driver_contract::resources::ResourceKind;
        let retained: Vec<retained::Volume> = facts
            .kept
            .iter()
            .map(|row| {
                serde_json::from_value::<Vec<retained::Volume>>(row.retained_volumes.clone())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApiError::internal)?
            .into_iter()
            .flatten()
            .collect();
        for object in native.resources.iter().filter(|object| {
            matches!(
                object.resource.kind,
                ResourceKind::Volume | ResourceKind::KeptLayer
            )
        }) {
            if selected.iter().any(|mount| {
                object.name == mount.location.source
                    && object
                        .labels
                        .get("orch.mount_id")
                        .is_some_and(|id| id == &mount.location.mount_id.to_string())
            }) || retained.iter().any(|volume| {
                volume.name == object.name
                    && object
                        .labels
                        .get("orch.runtime_id")
                        .is_some_and(|id| id == &volume.runtime_id.to_string())
            }) {
                continue;
            }
            // Current owners have their own recovery rows. Their data is never an orphan merely
            // because it has not yet become a retained-file promise.
            if object
                .labels
                .get("orch.runtime_id")
                .and_then(|id| id.parse::<Id>().ok())
                .is_some_and(|id| facts.inventory.runtimes.iter().any(|row| row.0 == id))
            {
                continue;
            }
            let id = format!("native:{}", object.resource.id);
            let fate = body
                .data
                .iter()
                .find(|row| row.id == id)
                .map_or(Fate::Leave, |row| row.fate);
            let mut row = api::HostDataRow {
                id,
                resource: api::LossResource {
                    id: host,
                    name: Some(facts.inventory.name.clone()),
                },
                label: Some(object.name.clone()),
                kind: if object.resource.kind == ResourceKind::Volume {
                    "volume"
                } else {
                    "kept_layer"
                }
                .into(),
                bytes: object.size_bytes,
                until: None,
                selected: fate,
                choices: vec![Fate::Leave, Fate::Delete],
                host: None,
                copy: None,
                blockers: vec![],
            };
            if !row.choices.contains(&fate) {
                row.blockers.push(blocked(
                    "data_fate_unavailable",
                    "This data has no logical owner whose copy plan could move it",
                ));
            }
            if fate == Fate::Delete && !facts.inventory.online {
                row.blockers.push(blocked(
                    "source_offline",
                    "Its host must answer before this data can be deleted",
                ));
            }
            native_data.push(NativeData {
                resource: object.resource.clone(),
                row: row.clone(),
            });
            rows.push(row);
        }
    }
    let moved: Vec<_> = selected
        .iter()
        .filter(|mount| mount.fate == Fate::Move)
        .map(|mount| mount.location.mount_id)
        .collect();
    for row in rows
        .iter_mut()
        .filter(|row| row.kind == "mount" && row.selected == Fate::Move)
    {
        if moved.iter().filter(|id| **id == row.resource.id).count() > 1 {
            row.blockers.push(blocked(
                "copy_conflict",
                "Choose one source of this mount to move; keep its other copies",
            ));
        }
    }
    let kept_refs: BTreeSet<_> = selected
        .iter()
        .filter(|mount| mount.fate == Fate::Leave)
        .filter_map(|mount| mount.native.as_ref())
        .collect();
    for mount in selected
        .iter()
        .filter(|mount| matches!(mount.fate, Fate::Move | Fate::Delete))
    {
        if mount
            .native
            .as_ref()
            .is_some_and(|resource| kept_refs.contains(resource))
            && let Some(row) = rows.iter_mut().find(|row| row.id == mount.row.id)
        {
            row.blockers.push(blocked(
                "data_shared",
                "Another data row keeps this same physical object",
            ));
        }
    }
    let ids: BTreeSet<_> = rows.iter().map(|row| row.id.as_str()).collect();
    if body.data.iter().any(|row| !ids.contains(row.id.as_str()))
        || body
            .data
            .iter()
            .map(|row| &row.id)
            .collect::<BTreeSet<_>>()
            .len()
            != body.data.len()
    {
        return Err(ApiError::invalid(
            "data",
            "choose each data item from this host's current plan once",
        ));
    }
    Ok(Planned {
        rows,
        custody: Custody {
            erase: body.erase.unwrap_or(true) && !facts.inventory.lost,
            mounts: selected,
            copies,
            native_data,
            uninstall_command: facts.uninstall_command.clone(),
            frozen_inventory: facts.native.clone(),
        },
        purges,
    })
}

/// Early purge reuses the same reservation, actor and recovery path as a scoped Clean up.
pub(super) async fn queue(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    plan: &Planned,
) -> ApiResult<Vec<Id>> {
    let identity = crate::routes::environments::retained::identity(ctx)?;
    let mut children = Vec::new();
    for (id, revision) in &plan.purges {
        let promise = retained::removal(conn, *id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if promise.removal_revision != *revision {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                "The retained data changed",
            ));
        }
        let row = orch_engine::jobs::retained::queue(
            conn,
            *id,
            *revision,
            true,
            Some(principal.user()),
            identity.clone(),
            ctx.now,
        )
        .await
        .map_err(ApiError::internal)?;
        children.push(row.id);
    }
    Ok(children)
}
