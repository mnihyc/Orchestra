//! A host is removed only after its reviewed independent cascades and unchanged retained-data promises finish.

use super::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone};
use crate::{EngineError, commands::Outgoing, ids};
use async_trait::async_trait;
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::workspace::{JobActivity, JobWaiting};
use orch_db::{
    DbError,
    queries::{
        environments::{changes, hosting as envs, retained},
        jobs,
        resources::{hosting as hosts, mounts},
    },
};
use orch_link_contract::commands::{NodeVolumeRemove, SerialKey};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

mod custody;

/// The durable parent is outside its children's batch, so even a parallel limit of one can progress.
pub const KIND: &str = "host_delete";
/// Recovery and retention dominate the bar; data deletion and registration closure follow them.
pub const PHASES: [Phase; 3] = [
    Phase {
        name: "cascades",
        weight: 80,
    },
    Phase {
        name: "mounts",
        weight: 15,
    },
    Phase {
        name: "delete",
        weight: 5,
    },
];

/// An explicitly confirmed owned mount volume, never a path on the host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Volume {
    /// The retained mount identity.
    pub mount: Id,
    /// Its owned native volume name.
    pub name: String,
    /// The driver's declared data service.
    pub driver: String,
    /// Its named storage, if any.
    pub storage: Option<String>,
}

/// An exact reviewed mount source whose files outlive ordinary environment cascades.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountData {
    /// Current or earlier kept source, selected only by the server.
    pub location: mounts::LocationRow,
    /// The earlier kept membership, or none for its current location.
    pub kept: Option<Id>,
    /// Its actual selected fate.
    pub fate: orch_api_contract::loss::HostDataFate,
    /// Safe reviewed identity and size used in the result.
    pub row: orch_api_contract::loss::HostDataRow,
    /// Owned volume identity in the reviewed node inventory, absent for a borrowed directory.
    pub native: Option<orch_driver_contract::resources::ResourceRef>,
}

/// Data and physical cleanup share this host's one accepted operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Custody {
    /// Erase owned non-data resources after the cascades.
    pub erase: bool,
    /// Every reviewed mount source, including Leave rows.
    pub mounts: Vec<MountData>,
    /// Existing copies to new locations, never replacement of destination data.
    pub copies: crate::mount_copies::MountDecisions,
    /// Unattributed owned volumes and layers still need their own explicit data choice.
    pub native_data: Vec<NativeData>,
    /// Exact standalone installation command; unknown or stack-wide commands are not invented.
    #[serde(default)]
    pub uninstall_command: Option<String>,
    /// The reviewed observation remains historical when the node cannot be reached.
    #[serde(default)]
    pub frozen_inventory: Option<orch_driver_contract::resources::ResourceInventory>,
}

/// An owned native data object with no current logical owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeData {
    /// Exact node identity, never interpreted by the server.
    pub resource: orch_driver_contract::resources::ResourceRef,
    /// The safe name, bytes and accepted fate.
    pub row: orch_api_contract::loss::HostDataRow,
}
/// The reviewed physical source and accepted independent child jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// Host whose registration is removed last.
    pub host: Id,
    /// Physical identity revoked last.
    pub machine: Id,
    /// Person responsible for this operation.
    pub by: Id,
    /// Narrowed token, rechecked before each destructive phase.
    pub token: Option<Id>,
    /// Scheduling state to restore if the parent is cancelled before deletion.
    pub schedulable: bool,
    /// Jobs already accepted atomically with this parent.
    pub children: Vec<Id>,
    /// Only volumes whose deletion was explicitly selected and named.
    pub delete_volumes: Vec<Volume>,
    /// Whether this confirmation explicitly granted administrative permissions.
    pub admin_override: bool,
    /// New plans keep data exclusions beside their physical cleanup consent.
    #[serde(default)]
    pub custody: Option<Custody>,
}
/// The parent does not claim success while another retained source still needs the host.
pub struct Deleting {
    recipe: Recipe,
}
impl Deleting {
    /// Reads the immutable reviewed choices.
    pub fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            recipe: serde_json::from_value(row.plan["cascade"].clone())?,
        })
    }
    async fn principal(
        &self,
        job: &Job,
        conn: &mut orch_db::Conn,
    ) -> Result<orch_authz::Principal, Stop> {
        let row = job.row().await?;
        if row.plan.get("identity").is_some() {
            super::retained::requester(conn, &row, job.now()).await
        } else {
            super::source::load_principal(conn, job, self.recipe.by, self.recipe.token).await
        }
    }
    async fn authority(&self, job: &Job) -> Result<(), Stop> {
        let mut conn = job.engine().db().acquire().await?;
        let principal = self.principal(job, &mut conn).await?;
        if !matches!(
            crate::operation_access::review(
                &mut conn,
                &principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: self.recipe.host
                },
                Permission::HostManage
            )
            .await,
            Ok(None)
        ) {
            return Err(
                Failure::new("access.denied", "The caller no longer manages this host").into(),
            );
        }
        Ok(())
    }
    async fn waiting(&self, job: &Job, reason: Option<JobWaiting>) -> Result<(), Stop> {
        job.amend(|progress, _| {
            if progress
                .activity
                .as_ref()
                .and_then(|activity| activity.waiting_for.as_ref())
                == reason.as_ref()
            {
                return;
            }
            progress.activity = Some(JobActivity {
                waiting_for: reason,
                updated_at: Time::new(job.now()),
                stall_after_seconds: 0,
                directories_done: None,
                stopping: false,
            });
        })
        .await?;
        Ok(())
    }
    async fn wait(&self, job: &Job) -> Result<Done, Stop> {
        loop {
            job.check()?;
            self.authority(job).await?;
            let mut conn = job.engine().db().acquire().await?;
            let row = job.row().await?;
            let mut children = self.recipe.children.clone();
            children.extend(
                row.plan["cleanup_children"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Json::as_str)
                    .filter_map(|id| id.parse::<Id>().ok()),
            );
            let states: Vec<(Id, String)> =
                sqlx::query_as("SELECT id,state FROM jobs WHERE id=ANY($1) ORDER BY id")
                    .bind(&children)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            if states.len() != children.len() {
                return Err(
                    Failure::new("cascade_missing", "A cascade's durable job is missing").into(),
                );
            }
            if states
                .iter()
                .any(|(_, state)| matches!(state.as_str(), "failed" | "lost" | "cancelled"))
            {
                return Err(Failure::new(
                    "cascade_failed",
                    "A cascade did not finish; its result and the host are kept",
                )
                .into());
            }
            let done = states
                .iter()
                .filter(|(_, state)| state == "succeeded")
                .count();
            let percent = if children.is_empty() {
                0
            } else {
                (done * 80 / children.len()) as u32
            };
            let current = row
                .progress
                .get("percent")
                .and_then(Json::as_u64)
                .unwrap_or(0);
            if u64::from(percent) > current {
                job.amend(|progress, _| progress.percent = percent).await?;
            }
            if done < children.len() {
                self.waiting(
                    job,
                    Some(JobWaiting::Operations {
                        count: i64::try_from(children.len() - done).unwrap_or(i64::MAX),
                    }),
                )
                .await?;
                drop(conn);
                job.pause(Duration::from_secs(1)).await?;
                continue;
            }
            let runtimes:Vec<envs::RuntimeFull>=sqlx::query_as("SELECT * FROM runtimes WHERE host_id=$1 AND state<>'removed' AND lost_at IS NULL ORDER BY id").bind(self.recipe.host).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            let waiting = crate::resource_facts::host_waiting(
                &mut conn,
                self.recipe.host,
                Time::new(job.now()),
            )
            .await?
            .ok_or_else(|| Failure::new("not_found", "The host registration is gone"))?;
            if waiting.runtimes == 0 && waiting.kept == 0 && waiting.environments == 0 {
                self.waiting(job, None).await?;
                return Ok(Done::Finished);
            }
            self.waiting(
                job,
                Some(if waiting.environments > 0 {
                    JobWaiting::HostEnvironments {
                        count: waiting.environments,
                    }
                } else if let Some(until) = waiting.until {
                    JobWaiting::RetainedData { until }
                } else if !waiting.online {
                    JobWaiting::Machine { name: waiting.name }
                } else {
                    JobWaiting::Operations {
                        count: waiting.kept.saturating_add(waiting.runtimes),
                    }
                }),
            )
            .await?;
            // The reaper retains its exact existing deadline. Queue only already-due retired copies.
            let mut cleanup = Vec::new();
            for runtime in runtimes.iter().filter(|runtime| {
                runtime.state == "retired" && runtime.remove_at.is_none_or(|at| at <= job.now())
            }) {
                if let Some(environment) = runtime.environment_id {
                    match super::remove::queue(&mut conn, job.now(), environment, Some(runtime.id))
                        .await
                    {
                        Ok(child) => cleanup.push(child.id),
                        Err(super::runner::MakeError::Running(child)) => cleanup.push(child.id),
                        Err(error) => {
                            return Err(Failure::new("cascade_failed", error.to_string()).into());
                        }
                    }
                } else if let Some(provider) = runtime.provider_id {
                    match super::provider_provision::queue_removal(
                        &mut conn,
                        job.now(),
                        provider,
                        runtime.id,
                    )
                    .await?
                    {
                        jobs::provider_jobs::Queued::Made(child)
                        | jobs::provider_jobs::Queued::Running(child) => cleanup.push(child.id),
                        jobs::provider_jobs::Queued::NoProvider => {
                            return Err(Failure::new(
                                "cascade_failed",
                                "The provider's retained runtime is unavailable",
                            )
                            .into());
                        }
                    }
                }
            }
            if !cleanup.is_empty() {
                let mut all = row.plan["cleanup_children"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                all.extend(cleanup.iter().map(|id| json!(id)));
                all.sort_by_key(Json::to_string);
                all.dedup();
                let operation = row.plan["notification_operation"].clone();
                for child in &cleanup {
                    jobs::merge_plan(
                        &mut conn,
                        *child,
                        &json!({"notification_operation":operation}),
                        job.now(),
                    )
                    .await?;
                }
                drop(conn);
                job.remember(&json!({"cleanup_children":all})).await?;
                job.engine().kick();
            } else {
                drop(conn);
            }
            job.pause(Duration::from_secs(1)).await?;
        }
    }
}
#[async_trait]
impl Phased for Deleting {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        self.authority(job).await?;
        match phase {
            "cascades" => self.wait(job).await,
            "mounts" => {
                if self.recipe.custody.is_some() {
                    return self.clear_custody(job).await;
                }
                if !self.recipe.delete_volumes.is_empty() {
                    job.pass_commit().await?;
                }
                for volume in &self.recipe.delete_volumes {
                    let labels =
                        BTreeMap::from([("orch.mount_id".into(), volume.mount.to_string())]);
                    job.command(
                        Outgoing::new(
                            ids::derived(
                                "host.delete.mount",
                                &[job.id().as_bytes(), volume.mount.as_bytes()],
                            ),
                            self.recipe.machine,
                            SerialKey::Volume(volume.name.clone()),
                            &NodeVolumeRemove {
                                driver: volume.driver.clone(),
                                name: volume.name.clone(),
                                storage: volume.storage.clone(),
                                labels,
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                    let mut tx = job.engine().db().begin().await?;
                    if job.lock_owned(&mut tx).await?.is_none() {
                        return Err(Stop::Lost);
                    }
                    mounts::remove_location(&mut tx, volume.mount, self.recipe.machine).await?;
                    tx.commit().await.map_err(DbError::from)?;
                }
                Ok(Done::Finished)
            }
            "delete" => {
                job.pass_commit().await?;
                let mut tx = job.engine().db().begin().await?;
                if job.lock_owned(&mut tx).await?.is_none() {
                    return Err(Stop::Lost);
                }
                let host = hosts::host(&mut tx, self.recipe.host)
                    .await?
                    .ok_or_else(|| Failure::new("not_found", "The host registration is gone"))?;
                let (runtimes, _) =
                    changes::host_dependencies(&mut tx, host.id, host.machine_id).await?;
                let (kept, _, _, _) = retained::on_host(&mut tx, host.id).await?;
                if let Some(data) = crate::resource_facts::custody::host(&mut tx, host.id).await?
                    && !self.accepts_sources(&data)
                {
                    return Err(Failure::new(
                        "data_changed",
                        "The host acquired different data; review its deletion again",
                    )
                    .into());
                }
                if !runtimes.is_empty() || kept > 0 {
                    return Err(Failure::new(
                        "cascade_changed",
                        "The host acquired another dependency; review it again",
                    )
                    .into());
                }
                // The machine deletion trigger records lost volume sizes, forgets locations and detaches offerings.
                changes::forget_host(&mut tx, host.id, host.machine_id, job.now()).await?;
                jobs::merge_plan(
                    &mut tx,
                    job.id(),
                    &json!({"host_deleted_at":Time::new(job.now())}),
                    job.now(),
                )
                .await?;
                let mut entry = orch_db::audit::Entry::new(
                    orch_db::audit::Actor::User(self.recipe.by),
                    orch_db::audit::name!("host.deleted"),
                    "host",
                    Some(host.id),
                    "Removed a host after its reviewed cascades",
                );
                entry.history = true;
                entry.r#override = self.recipe.admin_override;
                entry.details = json!({"job":job.id(),"machine":host.machine_id,"children":self.recipe.children});
                orch_db::audit::append(
                    &mut tx,
                    &entry,
                    &orch_db::audit::Origin::default(),
                    job.now(),
                )
                .await?;
                tx.commit().await.map_err(DbError::from)?;
                Ok(Done::Finished)
            }
            _ => Err(Failure::new("invalid_plan", "The host deletion has no such phase").into()),
        }
    }
    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        if self.recipe.custody.is_some()
            && !job
                .plan()
                .await
                .ok()
                .is_some_and(|plan| plan["host_copies_committed"] == true)
        {
            let _left = crate::mount_copies::undo(job).await;
        }
        if let Ok(mut conn) = job.engine().db().acquire().await {
            let _=sqlx::query("UPDATE hosts SET schedulable=$2,revision=revision+1,updated_at=$3 WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM machines m WHERE m.id=hosts.machine_id AND (m.deleted_at IS NOT NULL OR m.lost_at IS NOT NULL))").bind(self.recipe.host).bind(self.recipe.schedulable).bind(job.now()).execute(&mut *conn).await;
        }
        Undone::Partly(
            "The host stays registered; accepted cascades continue with their own controls".into(),
        )
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        let plan = job.plan().await.ok()?;
        Some(
            json!({"host_id":self.recipe.host,"deleted_at":plan["host_deleted_at"],"children":self.recipe.children,"left":plan["host_left"],"left_data":plan["host_left_data"],"left_confirmed":plan["host_left_confirmed"],"moved":plan["host_copies_committed"],"unreachable":plan["host_objects_unreachable"],"uninstall_command":self.recipe.custody.as_ref().and_then(|custody|custody.uninstall_command.as_ref())}),
        )
    }
    async fn partial_result(&self, job: &Job) -> Option<Json> {
        self.result(job).await
    }
}
