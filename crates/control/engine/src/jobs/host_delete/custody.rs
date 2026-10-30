//! Host erasure preserves every Leave row and recovery promise while removing exact owned objects.

use super::{Deleting, Done, EngineError, Failure, Job, Outgoing, Stop, mounts};
use crate::{ids, mount_copies};
use orch_api_contract::loss::HostDataFate;
use orch_driver_contract::resources::{ResourceInventory, ResourceKind, ResourceRef};
use orch_link_contract::commands::{NodeResourcesList, NodeResourcesRemove, SerialKey};
use orch_types::{Id, new_id};
use serde_json::json;
use std::collections::BTreeSet;

async fn inventory(job: &Job, machine: Id) -> Result<ResourceInventory, Stop> {
    let answer = job
        .command(
            Outgoing::new(new_id(), machine, SerialKey::Machine, &NodeResourcesList {})
                .map_err(EngineError::from)?,
        )
        .await?;
    let answer: ResourceInventory = serde_json::from_value(answer).map_err(EngineError::from)?;
    if !answer.unavailable.is_empty() {
        return Err(Failure::new(
            "inventory_incomplete",
            "The node cannot report every object; the host stays registered",
        )
        .into());
    }
    Ok(answer)
}

async fn remove(job: &Job, machine: Id, resource: &ResourceRef) -> Result<(), Stop> {
    job.command_irreversible(
        Outgoing::new(
            ids::derived(
                "host.delete.object",
                &[job.id().as_bytes(), resource.id.as_bytes()],
            ),
            machine,
            SerialKey::Machine,
            &NodeResourcesRemove {
                kind: resource.kind,
                id: resource.id.clone(),
                stop_first: resource.kind == ResourceKind::Unit,
                expected_last_used_at: None,
            },
        )
        .map_err(EngineError::from)?,
    )
    .await?;
    Ok(())
}

impl Deleting {
    pub(super) fn accepts_sources(
        &self,
        facts: &crate::resource_facts::custody::HostCustody,
    ) -> bool {
        let Some(custody) = &self.recipe.custody else {
            return true;
        };
        let known = custody
            .mounts
            .iter()
            .map(|mount| {
                (
                    mount.kept,
                    mount.location.mount_id,
                    mount.location.machine_id,
                    mount.location.kind.as_str(),
                    mount.location.source.as_str(),
                    mount.location.storage.as_deref(),
                )
            })
            .collect::<BTreeSet<_>>();
        facts.locations.iter().all(|row| {
            known.contains(&(
                None,
                row.mount_id,
                row.machine_id,
                row.kind.as_str(),
                row.source.as_str(),
                row.storage.as_deref(),
            ))
        }) && facts.mount_kept.iter().all(|row| {
            known.contains(&(
                Some(row.id),
                row.mount_id,
                row.machine_id,
                row.kind.as_str(),
                row.source.as_str(),
                row.storage.as_deref(),
            ))
        })
    }
    pub(super) async fn clear_custody(&self, job: &Job) -> Result<Done, Stop> {
        let Some(custody) = &self.recipe.custody else {
            return Ok(Done::Finished);
        };
        let mut conn = job.engine().db().acquire().await?;
        let host = orch_db::queries::resources::hosting::host(&mut conn, self.recipe.host)
            .await?
            .ok_or_else(|| Failure::new("not_found", "The host is gone"))?;
        let lost: bool = sqlx::query_scalar("SELECT lost_at IS NOT NULL FROM machines WHERE id=$1")
            .bind(host.machine_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if lost {
            let reference = orch_api_contract::resources::HostRef {
                id: self.recipe.host,
                name: host.name,
                address: String::new(),
            };
            let observed = custody
                .frozen_inventory
                .as_ref()
                .map(|inventory| crate::storage::node_records(&reference, inventory, 0, false));
            job.remember(&json!({"host_left":observed,"host_left_confirmed":false,"host_left_data":custody.mounts.iter().map(|mount|&mount.row).collect::<Vec<_>>(),"host_objects_unreachable":true})).await?;
            return Ok(Done::Finished);
        }
        if job.plan().await?["host_data_released"] == true {
            let left = inventory(job, self.recipe.machine).await?;
            let reference = orch_api_contract::resources::HostRef {
                id: self.recipe.host,
                name: host.name,
                address: String::new(),
            };
            job.remember(&json!({"host_left":crate::storage::node_records(&reference,&left,0,true),"host_left_confirmed":true})).await?;
            return Ok(Done::Finished);
        }
        // Recheck each source's ordinary location authority at execution, including token scope.
        let principal = self.principal(job, &mut conn).await?;
        let released = job.plan().await?["host_data_released"]
            .as_bool()
            .unwrap_or(false);
        for mount in custody
            .mounts
            .iter()
            .filter(|mount| matches!(mount.fate, HostDataFate::Move | HostDataFate::Delete))
        {
            if !released {
                let current = if let Some(kept) = mount.kept {
                    mounts::kept(&mut conn, &[mount.location.mount_id])
                        .await?
                        .into_iter()
                        .find(|row| row.id == kept)
                        .map(|row| (row.kind, row.source, row.storage))
                } else {
                    mounts::locations(&mut conn, &[mount.location.mount_id])
                        .await?
                        .into_iter()
                        .find(|row| row.machine_id == mount.location.machine_id)
                        .map(|row| (row.kind, row.source, row.storage))
                };
                if current.as_ref().map(|row| (&row.0, &row.1, &row.2))
                    != Some((
                        &mount.location.kind,
                        &mount.location.source,
                        &mount.location.storage,
                    ))
                {
                    return Err(Failure::new(
                        "data_changed",
                        "The source location changed; its data stays intact",
                    )
                    .into());
                }
            }
            let access = orch_authz::resources::may_add_location(
                &mut conn,
                &principal,
                &orch_authz::resources::LocationAsk {
                    mount: mount.location.mount_id,
                    machine: mount.location.machine_id,
                    kind: if mount.location.kind == "volume" {
                        orch_authz::resources::LocationKind::Volume
                    } else {
                        orch_authz::resources::LocationKind::Path
                    },
                    storage: mount.location.storage.clone(),
                },
                &Default::default(),
            )
            .await
            .map_err(EngineError::from)?;
            if !matches!(access, orch_authz::resources::Locating::Allowed) {
                return Err(Failure::new(
                    "access.denied",
                    "The caller no longer manages this mount location",
                )
                .into());
            }
        }
        for copy in &custody.copies.copies {
            let target:Option<Id>=sqlx::query_scalar("SELECT h.id FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.machine_id=$1 AND h.schedulable AND m.online AND m.deleted_at IS NULL AND m.lost_at IS NULL").bind(copy.target_machine).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
            let target = target.ok_or_else(|| {
                Failure::new("target_unavailable", "The copy destination is unavailable")
            })?;
            if !orch_authz::resources::check(
                &mut conn,
                &principal,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Host,
                    id: target,
                },
                orch_access_contract::permissions::Permission::HostUse,
                Default::default(),
            )
            .await
            .map_err(EngineError::from)?
            .is_allowed()
            {
                return Err(
                    Failure::new("access.denied", "Use of the copy destination ended").into(),
                );
            }
        }
        drop(conn);
        if !job.plan().await?["host_copies_committed"]
            .as_bool()
            .unwrap_or(false)
            && !custody.copies.copies.is_empty()
        {
            let mut copies = mount_copies::of_plan(&job.plan().await?)?;
            let child_plans: Vec<orch_types::Json> =
                sqlx::query_scalar("SELECT plan FROM jobs WHERE id=ANY($1) AND state='succeeded'")
                    .bind(&self.recipe.children)
                    .fetch_all(job.engine().db().pool())
                    .await
                    .map_err(orch_db::DbError::from)?;
            let child_copies = child_plans
                .iter()
                .map(mount_copies::of_plan)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flat_map(|decisions| decisions.copies)
                .collect::<Vec<_>>();
            let mut conn = job.engine().db().acquire().await?;
            for copy in &mut copies.copies {
                let current = mounts::locations(&mut conn, &[copy.mount_id])
                    .await?
                    .into_iter()
                    .find(|row| row.machine_id == copy.target_machine);
                let expected = copy.kept.as_ref().map(|kept| &kept.previous);
                if current
                    .as_ref()
                    .map(|row| (&row.kind, &row.source, &row.storage))
                    != expected.map(|data| (&data.kind, &data.source, &data.storage))
                {
                    let carried = current.as_ref().filter(|current| {
                        child_copies.iter().any(|child| {
                            child.made
                                && child.mount_id == copy.mount_id
                                && child.source_machine == copy.source_machine
                                && child.from == copy.from
                                && child.target_machine == copy.target_machine
                                && child.to.kind == current.kind
                                && child.to.source == current.source
                                && child.to.storage == current.storage
                        })
                    });
                    let current=carried.ok_or_else(||Failure::new("data_changed","A destination changed outside the accepted cascades; the source stays intact"))?;
                    copy.kept = Some(mount_copies::KeptDecision {
                        previous: mount_copies::DataRef {
                            kind: current.kind.clone(),
                            source: current.source.clone(),
                            storage: current.storage.clone(),
                        },
                        copied_into: None,
                        file_count: None,
                        total_bytes: None,
                        data_kind: current.data_kind.clone(),
                    });
                }
            }
            drop(conn);
            job.remember(&json!({"mount_decisions":copies})).await?;
            let targets: BTreeSet<_> = copies
                .copies
                .iter()
                .map(|copy| copy.target_machine)
                .collect();
            for machine in targets {
                mount_copies::apply(job, machine).await?;
            }
            self.authority(job).await?;
            let mut tx = job.engine().db().begin().await?;
            let accepted = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
            if accepted.cancel_requested() {
                return Err(Stop::Cancelled);
            }
            for copy in &copies.copies {
                let current = mounts::locations(&mut tx, &[copy.mount_id])
                    .await?
                    .into_iter()
                    .find(|row| row.machine_id == copy.target_machine);
                let expected = copy.kept.as_ref().map(|kept| &kept.previous);
                let same = current
                    .as_ref()
                    .map(|row| (&row.kind, &row.source, &row.storage))
                    == expected.map(|data| (&data.kind, &data.source, &data.storage));
                if !same {
                    return Err(Failure::new(
                        "data_changed",
                        "A destination location changed while copying; the source stays intact",
                    )
                    .into());
                }
            }
            if !orch_db::queries::jobs::pass_commit(&mut tx, job.id(), job.now()).await? {
                return Err(Stop::Cancelled);
            }
            mount_copies::commit_locations(
                &mut tx,
                &copies,
                None,
                job.id(),
                self.recipe.by,
                job.now(),
            )
            .await?;
            orch_db::queries::jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"host_copies_committed":true}),
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        let removes_data = custody.mounts.iter().any(|mount| {
            matches!(mount.fate, HostDataFate::Delete | HostDataFate::Move)
                && mount.location.kind == "volume"
        }) || custody
            .native_data
            .iter()
            .any(|data| data.row.selected == HostDataFate::Delete);
        let before = match inventory(job, self.recipe.machine).await {
            Ok(inventory) => inventory,
            Err(Stop::Failed(_)) if !custody.erase && !removes_data => {
                job.remember(&json!({"host_left_data":custody.mounts.iter().map(|mount|&mount.row).collect::<Vec<_>>(),"host_objects_unreachable":true,"host_left_confirmed":false})).await?;
                return Ok(Done::Finished);
            }
            Err(stop) => return Err(stop),
        };
        let reference = orch_api_contract::resources::HostRef {
            id: self.recipe.host,
            name: host.name.clone(),
            address: String::new(),
        };
        job.remember(&json!({"host_left":crate::storage::node_records(&reference,&before,0,true),"host_left_confirmed":false})).await?;
        let mut selected = BTreeSet::new();
        for mount in custody.mounts.iter().filter(|mount| {
            matches!(mount.fate, HostDataFate::Delete | HostDataFate::Move)
                && mount.location.kind == "volume"
        }) {
            if let Some(resource) = &mount.native {
                selected.insert(resource.clone());
            } else {
                return Err(Failure::new(
                    "inventory_unrecorded",
                    "The node has no owned identity for selected mount data",
                )
                .into());
            }
        }
        selected.extend(
            custody
                .native_data
                .iter()
                .filter(|data| data.row.selected == HostDataFate::Delete)
                .map(|data| data.resource.clone()),
        );
        if custody.erase {
            selected.extend(
                before
                    .resources
                    .iter()
                    .filter(|object| {
                        !matches!(
                            object.resource.kind,
                            ResourceKind::Volume | ResourceKind::KeptLayer
                        )
                    })
                    .map(|object| object.resource.clone()),
            );
        }
        // The node repeats ownership and native-dependency checks for every exact identity.
        let mut pending = selected;
        while !pending.is_empty() {
            job.check()?;
            self.authority(job).await?;
            let current = inventory(job, self.recipe.machine).await?;
            let mut conn = job.engine().db().acquire().await?;
            let waiting = crate::resource_facts::host_waiting(
                &mut conn,
                self.recipe.host,
                orch_types::Time::new(job.now()),
            )
            .await?
            .ok_or_else(|| Failure::new("not_found", "The host is gone"))?;
            let data = crate::resource_facts::custody::host(&mut conn, self.recipe.host)
                .await?
                .ok_or_else(|| Failure::new("not_found", "The host is gone"))?;
            if !self.accepts_sources(&data) {
                return Err(Failure::new(
                    "data_changed",
                    "The host acquired different mount data; review it before erasure",
                )
                .into());
            }
            if waiting.environments > 0 || waiting.kept > 0 || waiting.runtimes > 0 {
                return Err(Failure::new(
                    "cascade_changed",
                    "New work still needs the host; nothing else is erased",
                )
                .into());
            }
            drop(conn);
            job.remember(&json!({"host_left":crate::storage::node_records(&reference,&current,0,true),"host_left_confirmed":false})).await?;
            let next = pending
                .iter()
                .find(|resource| {
                    !current
                        .resources
                        .iter()
                        .any(|object| object.depends_on.contains(resource))
                })
                .cloned();
            let Some(next) = next else {
                return Err(Failure::new("in_use","A kept object still depends on selected cleanup data; the host stays registered").into());
            };
            if current
                .resources
                .iter()
                .any(|object| object.resource == next)
            {
                remove(job, self.recipe.machine, &next).await?;
            }
            pending.remove(&next);
        }
        let mut tx = job.engine().db().begin().await?;
        if job.lock_owned(&mut tx).await?.is_none() {
            return Err(Stop::Lost);
        }
        for mount in custody.mounts.iter().filter(|mount| {
            matches!(mount.fate, HostDataFate::Move | HostDataFate::Delete)
                && mount.location.kind == "volume"
        }) {
            if let Some(kept) = mount.kept {
                mounts::forget_kept(&mut tx, kept).await?;
            } else {
                mounts::remove_location(
                    &mut tx,
                    mount.location.mount_id,
                    mount.location.machine_id,
                )
                .await?;
            }
        }
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"host_data_released":true}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let left = inventory(job, self.recipe.machine).await?;
        let records = crate::storage::node_records(&reference, &left, 0, true);
        job.remember(&json!({"host_left":records,"host_left_confirmed":true,"host_left_data":custody.mounts.iter().filter(|mount|mount.location.kind=="path").map(|mount|&mount.row).collect::<Vec<_>>()})).await?;
        Ok(Done::Finished)
    }
}
