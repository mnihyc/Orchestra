//! One confirmed cleanup keeps its current authority and each irreversible result across cancellation and restart.

use async_trait::async_trait;
use orch_access_contract::{audit::AuditAction, decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::storage::{CleanupChoice, CleanupOutcome, CleanupPlan, CleanupRequest};
use orch_authz::Principal;
use orch_db::{
    Conn, DbError,
    audit::{self, Actor, Entry, Origin},
    queries::{
        environments::{hosting, retained},
        jobs,
    },
};
use orch_driver_contract::resources::{ResourceInventory, ResourceRef};
use orch_link_contract::commands::{
    NodeResourcesList, NodeResourcesRemove, NodeRuntimeRemove, SerialKey,
};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone};
use crate::{Engine, EngineError, commands::Outgoing, ids, resource_facts};

/// Cleanup shares the ordinary durable job ledger.
pub const KIND: &str = "storage_cleanup";
/// Each item is acknowledged before the next item is admitted.
pub const PHASES: [Phase; 1] = [Phase {
    name: "cleanup",
    weight: 100,
}];

/// An exact existing source, resolved by the server while building the displayed plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// An opaque node object, whose ownership and dependencies are verified afresh.
    Native {
        /// The source host, never a caller-supplied machine selector.
        host: Id,
        /// Its driver-neutral identity.
        resource: ResourceRef,
        /// The preview's occupancy fence.
        last_used_at: Option<Time>,
    },
    /// Frozen retained files of an environment.
    Environment {
        /// Their original permission page.
        environment: Id,
        /// Their exact removal promise.
        revision: i64,
    },
    /// A skill archive version whose historical references must be checked again.
    SkillVersion {
        /// Its original skill page.
        skill: Id,
        /// The exact archive membership.
        version: Id,
    },
    /// A displaced location retains its machine's existing data-management authority.
    MountData {
        /// Its original mount permission page.
        mount: Id,
        /// The exact immutable kept-location membership.
        kept: Id,
    },
}

/// One item keeps its choice and exact source beside the public plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Work {
    /// Its displayed record identity.
    pub id: String,
    /// The accepted real decision.
    pub choice: CleanupChoice,
    /// No target for a kept or blocked item.
    pub target: Option<Target>,
}

/// The persisted recipe contains no path or credential supplied by a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// The authorized entry point supplies the real scope of its single history line.
    #[serde(default)]
    pub request: Option<CleanupRequest>,
    /// The plan the person read.
    pub plan: CleanupPlan,
    /// Every item, including kept and blocked rows.
    pub work: Vec<Work>,
}

fn failed(code: &str, words: impl Into<String>) -> Stop {
    Failure::new(code, words).into()
}

/// Runs the same plan from Storage or any resource's Clean up act.
pub struct Cleaning {
    engine: Engine,
    recipe: Recipe,
}

impl Cleaning {
    /// Reconstructs the immutable accepted selection after a runner restart.
    pub fn of(engine: Engine, row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            engine,
            recipe: serde_json::from_value(row.plan["recipe"].clone())?,
        })
    }

    async fn principal(&self, conn: &mut Conn, job: &Job) -> Result<Principal, Stop> {
        super::retained::requester(conn, &job.row().await?, job.now()).await
    }

    async fn require(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        object: ObjectRef,
    ) -> Result<(), Stop> {
        if !resource_facts::authorities(conn, principal, &[object.id])
            .await?
            .iter()
            .any(|row| row.resource.object() == object && row.manageable)
        {
            return Err(failed(
                "denied",
                "the requester no longer manages this item",
            ));
        }
        Ok(())
    }

    async fn native(
        &self,
        job: &Job,
        work: &Work,
        host: Id,
        resource: &ResourceRef,
        last_used_at: Option<Time>,
    ) -> Result<(), Stop> {
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let principal = self.principal(&mut tx, job).await?;
        self.require(
            &mut tx,
            &principal,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
        )
        .await?;
        let host_row = orch_db::queries::resources::hosting::host(&mut tx, host)
            .await?
            .ok_or_else(|| failed("gone", "the host is gone"))?;
        tx.commit().await.map_err(DbError::from)?;
        let inventory: ResourceInventory = serde_json::from_value(
            job.command(
                Outgoing::new(
                    ids::derived(
                        "storage.cleanup.inventory",
                        &[job.id().as_bytes(), work.id.as_bytes()],
                    ),
                    host_row.machine_id,
                    SerialKey::Machine,
                    &NodeResourcesList {},
                )
                .map_err(EngineError::from)?,
            )
            .await?,
        )
        .map_err(EngineError::from)?;
        if !inventory.unavailable.is_empty() {
            return Err(failed(
                "inventory_incomplete",
                "the node cannot report every dependency",
            ));
        }
        let Some(current) = inventory
            .resources
            .iter()
            .find(|item| item.resource == *resource)
        else {
            return Ok(());
        };
        let fence = work.choice != CleanupChoice::StopAndRemove;
        if fence && current.last_used_at != last_used_at {
            return Err(failed(
                "changed",
                "this item was used after the plan was read",
            ));
        }
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let principal = self.principal(&mut tx, job).await?;
        self.require(
            &mut tx,
            &principal,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
        )
        .await?;
        sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
            .bind(host)
            .bind(json!(inventory))
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        let facts = resource_facts::read(
            &mut tx,
            &[orch_api_contract::resource_facts::FactSubject::Artifact {
                host_id: host,
                artifact_kind: serde_json::to_value(resource.kind)
                    .map_err(EngineError::from)?
                    .as_str()
                    .unwrap_or_default()
                    .into(),
                id: resource.id.clone(),
            }],
            Time::new(job.now()),
        )
        .await?;
        if facts.iter().any(|row| {
            !row.dependents_complete
                || row.dependents.iter().any(|edge| {
                    matches!(
                        edge.relation,
                        orch_api_contract::resource_facts::DependentRelation::Restore
                            | orch_api_contract::resource_facts::DependentRelation::Attachment
                            | orch_api_contract::resource_facts::DependentRelation::NativeDependency
                            | orch_api_contract::resource_facts::DependentRelation::Operation
                    )
                })
        }) {
            return Err(failed(
                "in_use",
                "a restore promise or an unknown dependency still needs this item",
            ));
        }
        tx.commit().await.map_err(DbError::from)?;
        job.command_irreversible(
            Outgoing::new(
                ids::derived(
                    "storage.cleanup.remove",
                    &[job.id().as_bytes(), work.id.as_bytes()],
                ),
                host_row.machine_id,
                SerialKey::Machine,
                &NodeResourcesRemove {
                    kind: resource.kind,
                    id: resource.id.clone(),
                    stop_first: work.choice == CleanupChoice::StopAndRemove,
                    expected_last_used_at: if fence { last_used_at } else { None },
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
        Ok(())
    }

    async fn environment(
        &self,
        job: &Job,
        work: &Work,
        environment: Id,
        revision: i64,
    ) -> Result<(), Stop> {
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        hosting::lock_environment(&mut tx, environment)
            .await?
            .ok_or_else(|| failed("gone", "the environment is gone"))?;
        let principal = self.principal(&mut tx, job).await?;
        let (manager, admin) = super::retained::authority(&mut tx, &principal, environment).await?;
        let promise = retained::removal(&mut tx, environment)
            .await?
            .ok_or_else(|| failed("gone", "the kept files are gone"))?;
        if promise.purged_at.is_some() {
            return Ok(());
        }
        if promise.removal_revision != revision {
            return Err(failed("changed", "the kept-file promise changed"));
        }
        if !admin && !(manager && promise.volumes_purge_at.is_some_and(|at| at <= job.now())) {
            return Err(failed(
                "restore_window",
                "only a current home admin may shorten the recovery window",
            ));
        }
        let reservation = format!("cleanup_reserved_{}", work.id);
        let plan = job.plan().await?;
        if plan.get(&reservation).and_then(Json::as_bool) != Some(true) {
            if !jobs::retained::open_of_environment_except(&mut tx, environment, Some(job.id()))
                .await?
                .is_empty()
                || !retained::reserve(&mut tx, environment, revision, "purging", job.now()).await?
            {
                return Err(failed(
                    "in_use",
                    "another lifecycle operation owns the kept files",
                ));
            }
            jobs::merge_plan(&mut tx, job.id(), &json!({(reservation):true}), job.now()).await?;
        } else if promise.removal_phase.as_deref() != Some("purging") {
            return Err(failed(
                "changed",
                "another lifecycle operation owns the kept files",
            ));
        }
        let refs: Vec<retained::Volume> =
            serde_json::from_value(promise.retained_volumes.clone()).map_err(EngineError::from)?;
        for volume in &refs {
            if hosting::volume_in_use(&mut tx, volume.host_id, &volume.name, volume.runtime_id)
                .await?
            {
                return Err(failed("in_use", "a live successor still needs these files"));
            }
        }
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if owned.cancel_requested() {
            return Err(Stop::Cancelled);
        }
        let mut begun = plan
            .get("cleanup_purges")
            .and_then(Json::as_object)
            .cloned()
            .unwrap_or_default();
        begun.insert(
            environment.to_string(),
            json!({"revision":revision,"started":true}),
        );
        // Once admitted, even a lost reply can mean removed files. Keep the original requester and
        // purge-only state for the existing reaper; cancellation stops before the next item.
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"cleanup_purges":begun}),
            job.now(),
        )
        .await?;
        let host = match refs.first() {
            Some(volume) => Some(
                orch_db::queries::resources::hosting::host(&mut tx, volume.host_id)
                    .await?
                    .ok_or_else(|| failed("node_away", "the host keeping these files is gone"))?,
            ),
            None => None,
        };
        tx.commit().await.map_err(DbError::from)?;
        if let (Some(host), Some(volume)) = (host, refs.first()) {
            job.command_to_undo(
                Outgoing::new(
                    ids::derived(
                        "storage.cleanup.environment",
                        &[job.id().as_bytes(), work.id.as_bytes()],
                    ),
                    host.machine_id,
                    SerialKey::Runtime(volume.runtime_id),
                    &NodeRuntimeRemove {
                        runtime_id: volume.runtime_id,
                        keep_volumes: false,
                        volume_refs: serde_json::from_value(promise.retained_volumes)
                            .map_err(EngineError::from)?,
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        }
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        if !retained::purged(&mut tx, environment, revision, job.now()).await? {
            return Err(failed(
                "changed",
                "the kept-file promise changed after removal",
            ));
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    async fn settle_environment(&self, job: &Job, work: &Work, why: &str) -> Result<(), Stop> {
        let Some(Target::Environment {
            environment,
            revision,
        }) = &work.target
        else {
            return Ok(());
        };
        let plan = job.plan().await?;
        if plan
            .get(format!("cleanup_reserved_{}", work.id))
            .and_then(Json::as_bool)
            != Some(true)
        {
            return Ok(());
        }
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        hosting::lock_environment(&mut tx, *environment).await?;
        if plan
            .get("cleanup_purges")
            .and_then(|map| map.get(environment.to_string()))
            .and_then(|row| row.get("started"))
            .and_then(Json::as_bool)
            == Some(true)
        {
            let failures = hosting::failed_removals(&mut tx, *environment, None).await? + 1;
            let pause = super::remove::pause_after(
                failures,
                self.engine.inner.config.hosting.removal_first_pause,
                self.engine.inner.config.hosting.reaper_retry_max,
            );
            retained::retry_purge(
                &mut tx,
                *environment,
                *revision,
                why,
                job.now() + chrono::Duration::from_std(pause).unwrap_or_default(),
                job.now(),
            )
            .await?;
        } else {
            retained::release(&mut tx, *environment, *revision, "purging", why, job.now()).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    async fn skill(&self, job: &Job, skill: Id, version: Id) -> Result<(), Stop> {
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let principal = self.principal(&mut tx, job).await?;
        self.require(
            &mut tx,
            &principal,
            ObjectRef {
                kind: ObjectKind::Skill,
                id: skill,
            },
        )
        .await?;
        sqlx::query("SELECT id FROM skills WHERE id=$1 FOR UPDATE")
            .bind(skill)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?;
        let held: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns WHERE skill_versions @> ARRAY[$1::uuid]) OR EXISTS(SELECT 1 FROM skill_drafts WHERE based_on=$1) OR EXISTS(SELECT 1 FROM skills WHERE id=$2 AND latest_version_id=$1 AND deleted_at IS NULL) OR EXISTS(SELECT 1 FROM settings WHERE key='agents.skills' AND value @> jsonb_build_object($2::text,$1::text))")
            .bind(version).bind(skill).fetch_one(&mut *tx).await.map_err(DbError::from)?;
        if held {
            return Err(failed(
                "in_use",
                "a turn or an explicit giving still keeps this version",
            ));
        }
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if owned.cancel_requested() {
            return Err(Stop::Cancelled);
        }
        sqlx::query("UPDATE skills SET latest_version_id=NULL WHERE id=$1 AND deleted_at IS NOT NULL AND latest_version_id=$2").bind(skill).bind(version).execute(&mut *tx).await.map_err(DbError::from)?;
        // The existing version trigger releases exactly its own archive pin.
        sqlx::query("DELETE FROM skill_versions WHERE id=$1 AND skill_id=$2")
            .bind(version)
            .bind(skill)
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    async fn mount(&self, job: &Job, mount: Id, kept: Id) -> Result<(), Stop> {
        use orch_authz::resources::{Locating, LocationAsk, LocationKind};
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let principal = self.principal(&mut tx, job).await?;
        self.require(
            &mut tx,
            &principal,
            ObjectRef {
                kind: ObjectKind::Mount,
                id: mount,
            },
        )
        .await?;
        let Some(data) = orch_db::queries::resources::mounts::kept(&mut tx, &[mount])
            .await?
            .into_iter()
            .find(|row| row.id == kept)
        else {
            return Ok(());
        };
        let ask = LocationAsk {
            mount,
            machine: data.machine_id,
            kind: if data.kind == "volume" {
                LocationKind::Volume
            } else {
                LocationKind::Path
            },
            storage: data.storage.clone(),
        };
        if !matches!(
            orch_authz::resources::may_add_location(&mut tx, &principal, &ask, &Default::default())
                .await
                .map_err(EngineError::from)?,
            Locating::Allowed
        ) {
            return Err(failed(
                "denied",
                "changing kept data requires management of its machine",
            ));
        }
        let driver = if data.kind == "volume" {
            Some(
                orch_db::queries::resources::mounts::data_driver(&mut tx, data.machine_id)
                    .await?
                    .ok_or_else(|| failed("node_away", "the machine no longer offers its data"))?
                    .0,
            )
        } else {
            None
        };
        tx.commit().await.map_err(DbError::from)?;
        if let Some(driver) = driver {
            job.command_irreversible(
                Outgoing::new(
                    ids::derived(
                        "storage.cleanup.mount",
                        &[job.id().as_bytes(), kept.as_bytes()],
                    ),
                    data.machine_id,
                    SerialKey::Volume(data.source.clone()),
                    &orch_link_contract::commands::NodeVolumeRemove {
                        driver,
                        name: data.source,
                        storage: data.storage,
                        labels: std::collections::BTreeMap::from([(
                            "orch.mount_id".into(),
                            mount.to_string(),
                        )]),
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        }
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        orch_db::queries::resources::mounts::forget_kept(&mut tx, kept).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    async fn finish_history(&self, job: &Job) -> Result<(), Stop> {
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE action='storage.cleaned' AND details->>'job'=$1)")
            .bind(job.id().to_string()).fetch_one(&mut *tx).await.map_err(DbError::from)?;
        if !exists {
            let row = job.row().await?;
            let object = self.recipe.request.as_ref().and_then(|request| {
                request.resource.or_else(|| {
                    request
                        .scope
                        .strip_prefix("host:")
                        .and_then(|id| id.parse().ok())
                        .map(|id| ObjectRef {
                            kind: ObjectKind::Host,
                            id,
                        })
                })
            });
            let kind = object.map_or("storage", |object| object.kind.as_str());
            let summary = format!("Cleanup finished in {}", self.recipe.plan.confirmation_name);
            let mut entry = Entry::new(
                row.created_by.map_or(Actor::System, Actor::User),
                orch_db::audit::action!(AuditAction::StorageCleaned).as_str(),
                kind,
                Some(object.map_or(job.id(), |object| object.id)),
                &summary,
            );
            entry.history = true;
            entry.node_id = object.and_then(|object| {
                self.recipe
                    .plan
                    .items
                    .iter()
                    .filter(|item| item.record.resource == Some(object))
                    .find_map(|item| item.record.place.as_ref().map(|place| place.id))
            });
            entry.environment_id = object
                .filter(|object| object.kind == ObjectKind::Environment)
                .map(|object| object.id);
            // A host's history can be read by people who do not see every logical owner cleaned
            // there. Detailed names and native errors stay in the private job until projected.
            let outcomes: Vec<Json> = row.plan["cleanup_results"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|outcome| json!({"id":outcome["id"],"outcome":outcome["outcome"]}))
                .collect();
            entry.details = json!({"version":1,"job":job.id(),"scope":self.recipe.plan.confirmation_name,"items":self.recipe.plan.items.iter().map(|item|json!({"id":item.record.id,"kind":item.record.kind,"resource":item.record.resource,"bytes":item.record.bytes,"until":item.record.until})).collect::<Vec<_>>(),"outcomes":outcomes});
            let origin: Origin =
                serde_json::from_value(row.plan["origin"].clone()).unwrap_or_default();
            audit::append(&mut tx, &entry, &origin, job.now()).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
}

#[async_trait]
impl Phased for Cleaning {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if phase != "cleanup" {
            return Err(failed("phase", "the cleanup phase is unknown"));
        }
        let mut outcomes: Vec<CleanupOutcome> =
            serde_json::from_value(job.plan().await?["cleanup_results"].clone())
                .unwrap_or_default();
        for (index, work) in self.recipe.work.iter().enumerate() {
            if outcomes.iter().any(|row| row.id == work.id) {
                continue;
            }
            job.check()?;
            let result = match work
                .target
                .as_ref()
                .filter(|_| work.choice != CleanupChoice::Keep)
            {
                None => None,
                Some(Target::Native {
                    host,
                    resource,
                    last_used_at,
                }) => Some(self.native(job, work, *host, resource, *last_used_at).await),
                Some(Target::Environment {
                    environment,
                    revision,
                }) => Some(self.environment(job, work, *environment, *revision).await),
                Some(Target::SkillVersion { skill, version }) => {
                    Some(self.skill(job, *skill, *version).await)
                }
                Some(Target::MountData { mount, kept }) => {
                    Some(self.mount(job, *mount, *kept).await)
                }
            };
            let (outcome, reason) = match result {
                None => {
                    if let Some(reason) = self
                        .recipe
                        .plan
                        .items
                        .iter()
                        .find(|item| item.record.id == work.id)
                        .and_then(|item| item.record.blocker.clone())
                    {
                        ("blocked", Some(reason))
                    } else {
                        ("kept", None)
                    }
                }
                Some(Ok(())) => ("removed", None),
                Some(Err(Stop::Failed(failure))) => {
                    self.settle_environment(job, work, &failure.message).await?;
                    ("failed", Some(failure.message))
                }
                Some(Err(other)) => return Err(other),
            };
            outcomes.push(CleanupOutcome {
                id: work.id.clone(),
                outcome: outcome.into(),
                reason,
            });
            job.remember(&json!({"cleanup_results":outcomes})).await?;
            job.progress(
                (index + 1) as f64 / self.recipe.work.len().max(1) as f64,
                None,
            )
            .await?;
        }
        self.finish_history(job).await?;
        Ok(Done::Finished)
    }
    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        let reason = match why {
            Stop::Failed(failure) => failure.message.as_str(),
            Stop::Cancelled => "Cleanup stopped before the remaining items",
            Stop::Lost => "The controller stopped during cleanup",
        };
        for work in &self.recipe.work {
            if let Err(error) = self.settle_environment(job, work, reason).await {
                tracing::warn!(?error, "a cleanup reservation could not be settled");
                return Undone::Deferred(
                    "Waiting to settle the kept files' cleanup reservation".into(),
                );
            }
        }
        if let Err(error) = self.finish_history(job).await {
            tracing::warn!(?error, "cleanup history could not be finalized");
            return Undone::Deferred("Waiting to save the cleanup results".into());
        }
        if job.plan().await.ok().is_some_and(|plan| {
            plan["cleanup_results"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["outcome"] == "removed"))
                || plan["cleanup_purges"]
                    .as_object()
                    .is_some_and(|rows| rows.values().any(|row| row["started"] == true))
        }) {
            Undone::Partly(
                "Removed objects stay removed; admitted file purges remain purge-only".into(),
            )
        } else {
            Undone::Everything
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        Some(json!({"cleanup":job.plan().await.ok()?["cleanup_results"]}))
    }
    async fn partial_result(&self, job: &Job) -> Option<Json> {
        self.result(job).await
    }
}
