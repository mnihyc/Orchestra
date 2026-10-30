//! Recreate and update, of an environment and of a provider's runtime: a successor on the same volumes, the old unit kept until the checks pass and brought back otherwise.
//!
//! The old unit and its identity remain the current ones while a successor is made and checked.
//! A failure removes only that candidate and brings the old unit back. Every affected thread is
//! held by the open job and is released when the runner finishes it.

use crate::commands::Outgoing;
use crate::ids;
use crate::jobs::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone, initial_progress};
use crate::unit::Unit;
use crate::updates::{Draft, Recipe, Target, UpdateError};
use crate::{Engine, EngineError};
use async_trait::async_trait;
use orch_api_contract::environment::UpdatePreflightReason;
use orch_api_contract::forks::ActiveTurn;
use orch_db::queries::{
    environments::{changes as env_changes, hosting as envs},
    jobs::{self, changes as rows},
    resources::{hosting as hosts, provider_runtime as providers},
    threads,
};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{
    AgentStart, NodeRuntimeRemove, NodeRuntimeStart, NodeRuntimeStop, ProviderRefresh, SerialKey,
    TurnSteer,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::time::Duration;

/// Replacement weights reflect unit creation dominating the work, followed by its health checks.
pub const PHASES: [Phase; 6] = [
    Phase {
        name: "notify",
        weight: 0,
    },
    Phase {
        name: "lock",
        weight: 5,
    },
    Phase {
        name: "replace",
        weight: 75,
    },
    Phase {
        name: "after_up",
        weight: 5,
    },
    Phase {
        name: "verify",
        weight: 15,
    },
    Phase {
        name: "commit",
        weight: 5,
    },
];
/// A quarter-second poll matches the runner's cancellation watch.
const LOOK_EVERY: Duration = Duration::from_millis(250);
/// A UUID's last twelve digits vary even when two jobs begin in the same millisecond.
const NAME_DIGITS: usize = 12;

fn fail(code: &str, message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new(code, message))
}

/// Holds the threads and inserts their replacement in the caller's transaction.
pub async fn queue(
    conn: &mut Conn,
    mut draft: Draft,
    batch: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<jobs::JobRow, UpdateError> {
    if !draft.blockers.is_empty() {
        return Err(UpdateError::Blocked(draft.blockers));
    }
    let target = draft.recipe.target;
    match target {
        Target::Environment(id) => {
            envs::lock_environment(conn, id)
                .await?
                .ok_or(UpdateError::NotFound)?;
        }
        Target::Provider(id) => {
            providers::lock(conn, id)
                .await?
                .ok_or(UpdateError::NotFound)?;
        }
    }
    if let Some(job) = match target {
        Target::Environment(id) => rows::environment_hold(conn, id).await?,
        Target::Provider(id) => rows::provider_hold(conn, id).await?,
    } {
        return Err(UpdateError::Held(job.id));
    }
    draft.recipe.threads = rows::threads(
        conn,
        match target {
            Target::Environment(id) => Some(id),
            _ => None,
        },
        match target {
            Target::Provider(id) => Some(id),
            _ => None,
        },
        true,
    )
    .await?;
    // A fork that only copies from a thread holds its environment without holding that thread.
    // Check again after acquiring the thread rows, so its concurrent confirmation cannot slip through.
    if let Some(job) = match target {
        Target::Environment(id) => rows::environment_hold(conn, id).await?,
        Target::Provider(id) => rows::provider_hold(conn, id).await?,
    } {
        return Err(UpdateError::Held(job.id));
    }
    for thread in &draft.recipe.threads {
        if let Some(job) = rows::thread_hold(conn, *thread).await? {
            return Err(UpdateError::Held(job.id));
        }
    }
    if draft.recipe.network_policy.restricted() {
        orch_db::queries::network::require(conn, draft.recipe.old, Some("restricted")).await?;
    }
    let plan = serde_json::to_value(&draft.recipe).map_err(EngineError::from)?;
    let progress = serde_json::to_value(initial_progress(&PHASES)).map_err(EngineError::from)?;
    let job = rows::insert(
        conn,
        &jobs::NewJob {
            id: new_id(),
            kind: target.job_kind(),
            node_id: None,
            environment_id: match target {
                Target::Environment(id) => Some(id),
                _ => None,
            },
            provider_id: match target {
                Target::Provider(id) => Some(id),
                _ => None,
            },
            plan: &plan,
            progress: &progress,
            created_by: Some(draft.recipe.by),
        },
        batch,
        now,
    )
    .await?;
    if let Target::Environment(id) = target
        && !envs::set_status(
            conn,
            id,
            &["running", "stopped", "failed", "offline"],
            "recreating",
            None,
            now,
        )
        .await?
    {
        return Err(UpdateError::Blocked(vec![crate::updates::blocker(
            "environment_unavailable",
            "the environment cannot be recreated in this state",
        )]));
    }
    let mut unit = Unit::new(conn, now);
    for thread in &draft.recipe.threads {
        let row = threads::thread(&mut *unit.conn, *thread)
            .await?
            .ok_or(UpdateError::NotFound)?;
        if threads::active_turn(&mut *unit.conn, *thread)
            .await?
            .is_none()
            && matches!(row.state.as_str(), "idle" | "blocked")
        {
            let seq = unit.stamp(*thread).await?;
            crate::notifications::set_state(
                &mut unit,
                *thread,
                &["idle", "blocked"],
                &threads::StateChange::blocked(
                    "switching",
                    Some("Its runtime is being replaced; messages wait until that is done."),
                ),
                seq,
            )
            .await?;
        }
    }
    unit.finish().await?;
    Ok(job)
}

/// Replacement of an environment or provider runtime, on the same persistent volumes.
pub struct Recreating {
    recipe: Recipe,
}

impl Recreating {
    /// Reads a durable recipe from a queued job.
    pub fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            recipe: serde_json::from_value(row.plan.clone())?,
        })
    }

    async fn notice(&self, job: &Job, message: &str) -> Result<(), Stop> {
        let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
        let mut unit = Unit::new(&mut tx, job.now());
        let mut steers = Vec::new();
        for thread in &self.recipe.threads {
            let Some(row) = threads::lock(&mut *unit.conn, *thread).await? else {
                continue;
            };
            if let Some(turn) = threads::active_turn(&mut *unit.conn, *thread).await?
                && turn.state == "running"
                && job.engine().agent_takes_a_steer(&mut unit, &row).await?
                && let (Some(provider), Some(environment)) = (row.provider_id, row.environment_id)
                && let Some(provider) =
                    orch_db::queries::resources::thread_engine::provider(&mut *unit.conn, provider)
                        .await?
                && let Some(environment) = envs::environment(&mut *unit.conn, environment).await?
                && let Some(machine) = crate::harnesses::machine_of(
                    row.arrangement.as_deref(),
                    &provider,
                    environment.machine_id,
                )
            {
                steers.push((*thread, turn.id, machine));
            }
            env_changes::notice(&mut *unit.conn, *thread, message, job.now()).await?;
            let seq = unit.stamp(*thread).await?;
            threads::set_seq_state(&mut *unit.conn, *thread, seq).await?;
        }
        let finished = unit.finish().await?;
        job.engine().commit(tx, finished).await?;
        for (thread, turn, machine) in steers {
            let command = Outgoing::new(
                ids::derived(
                    "turn.steer",
                    &[job.id().as_bytes(), thread.as_bytes(), message.as_bytes()],
                ),
                machine,
                SerialKey::Thread(thread),
                &TurnSteer {
                    thread_id: thread,
                    turn_id: turn,
                    text: message.to_owned(),
                    images: vec![],
                    native_action: None,
                },
            )
            .map_err(EngineError::from)?;
            let id = command.id;
            match tokio::time::timeout(
                job.engine().inner.config.hosting.stop_wait,
                job.command(command),
            )
            .await
            {
                Ok(Ok(_)) => {
                    job.engine()
                        .forget_pending_notices(thread, &[env_changes::notice_id(message)])
                        .await?;
                }
                Ok(Err(error @ (Stop::Cancelled | Stop::Lost))) => return Err(error),
                Ok(Err(_)) => {}
                Err(_) => {
                    job.engine()
                        .inner
                        .commands
                        .cancel(id)
                        .await
                        .map_err(EngineError::from)?;
                }
            }
        }
        Ok(())
    }

    async fn lock(&self, job: &Job) -> Result<Done, Stop> {
        let plan = job.plan().await?;
        let since = plan
            .get("waiting_since")
            .cloned()
            .and_then(|v| serde_json::from_value::<chrono::DateTime<chrono::Utc>>(v).ok())
            .unwrap_or(job.now());
        if plan.get("waiting_since").is_none() {
            job.remember(&json!({"waiting_since":since})).await?;
        }
        for thread in &self.recipe.threads {
            job.engine().cancel_pending_approvals(*thread).await?;
        }
        loop {
            job.check()?;
            let mut active = Vec::new();
            {
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                for thread in &self.recipe.threads {
                    if let Some(turn) = threads::active_turn(&mut conn, *thread).await?
                        && !(self.recipe.auto_start && turn.state == "starting")
                    {
                        active.push((*thread, turn));
                    }
                }
            }
            if active.is_empty() {
                break;
            }
            if self.recipe.options.active_turn == ActiveTurn::Wait
                && job.now() - since
                    >= self.recipe.capability_grace_secs.map_or_else(
                        || chrono::Duration::minutes(self.recipe.options.wait_minutes),
                        chrono::Duration::seconds,
                    )
            {
                return Err(fail(
                    "thread.not_idle",
                    "a turn did not finish before the wait ended; no unit was changed",
                ));
            }
            if self.recipe.options.active_turn == ActiveTurn::Interrupt {
                for (thread, turn) in active {
                    job.engine()
                        .interrupt_for_change(thread, &turn, self.recipe.by)
                        .await?;
                }
            }
            job.pause(LOOK_EVERY).await?;
        }
        Ok(Done::Finished)
    }

    async fn old(&self, job: &Job) -> Result<envs::RuntimeFull, Stop> {
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        envs::runtime(&mut conn, self.recipe.old)
            .await?
            .ok_or_else(|| fail("runtime_gone", "the old runtime is gone"))
    }

    async fn replace(&self, job: &Job) -> Result<Done, Stop> {
        let old = self.old(job).await?;
        let plan = job.plan().await?;
        if plan.get("runtime").is_none() {
            let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
            let next = new_id();
            let digits = next.simple().to_string();
            let name = format!(
                "orch-{}-{}",
                old.kind,
                &digits[digits.len() - NAME_DIGITS..]
            );
            // A provider's runtime whose host is changed gets its new unit on that host, with a state volume of its
            // own there; the files that go with it are carried before its first start.
            let moving = self.recipe.host.filter(|host| {
                *host != old.host_id && matches!(self.recipe.target, Target::Provider(_))
            });
            let mut placed = old.clone();
            if let Some(sources) = &self.recipe.data_sources {
                for (role, prefix, source) in [
                    ("workspace", "orch-ws", &sources.workspace),
                    ("home", "orch-home", &sources.home),
                ] {
                    if matches!(self.recipe.target, Target::Provider(_))
                        && (role == "workspace"
                            || source.directory().is_none() && placed.volumes.get(role).is_none())
                    {
                        continue;
                    }
                    if placed.volumes.get(role).is_none() {
                        placed.volumes[role] =
                            json!({"name":format!("{prefix}-{next}"),"storage":null});
                    }
                    match source.directory() {
                        Some(path) => {
                            placed.volumes[role]["directory"] = json!({"path": path});
                            placed.volumes[role]["storage"] = serde_json::Value::Null;
                        }
                        None => {
                            let volume = placed.volumes[role].as_object_mut().ok_or_else(|| {
                                fail("data_root_missing", "the old data root binding is missing")
                            })?;
                            if volume.remove("directory").is_some() {
                                volume.insert("name".into(), json!(format!("{prefix}-{next}")));
                                if role == "home" {
                                    volume.insert(
                                        "storage".into(),
                                        json!(self.recipe.provider_home_storage),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            if let Some(host) = moving {
                placed.host_id = host;
                placed.machine_id = None;
                placed.volumes = json!({"state":{"name":format!("{name}-state"),"storage":null}});
                if self.recipe.volume_carry.is_some() {
                    placed.volumes["home"] = json!({"name":format!("orch-home-{next}"),"storage":self.recipe.provider_home_storage});
                }
            }
            env_changes::successor(
                &mut tx,
                &placed,
                next,
                &name,
                self.recipe.template,
                &self.recipe.hash,
                job.now(),
            )
            .await?;
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"runtime":next,"host":placed.host_id,"carry":moving.is_some(),"fresh":false,"mode":"make"}),
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
        }
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let host = hosts::host(&mut conn, old.host_id)
            .await?
            .ok_or_else(|| fail("no_host", "the old host is gone"))?;
        drop(conn);
        // Remember before sending: a crash after the stop still takes the rollback path.
        if self.recipe.network_policy.restricted() {
            // The node persists the same guard before the old open unit can be stopped.
            job.command(
                Outgoing::new(
                    ids::derived("node.network.apply", &[job.id().as_bytes(), b"require"]),
                    host.machine_id,
                    SerialKey::Runtime(old.id),
                    &crate::network::apply(old.id, &self.recipe.network_policy),
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        } else {
            // This accepted plan explicitly chooses open; generic Start never clears the guard.
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            if orch_db::queries::network::required(&mut conn, old.id)
                .await?
                .is_some()
            {
                drop(conn);
                job.command(
                    Outgoing::new(
                        ids::derived(
                            "node.network.apply",
                            &[job.id().as_bytes(), b"explicit-open"],
                        ),
                        host.machine_id,
                        SerialKey::Runtime(old.id),
                        &crate::network::apply(old.id, &self.recipe.network_policy),
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                orch_db::queries::network::require(&mut conn, old.id, None).await?;
            }
        }
        job.remember(&json!({"old_stop_requested":true})).await?;
        {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            envs::set_runtime_state(
                &mut conn,
                old.id,
                &envs::runtime_state::LIVE,
                "stopping",
                None,
                job.now(),
            )
            .await?;
        }
        job.command(
            Outgoing::new(
                ids::derived("node.runtime.stop", &[job.id().as_bytes(), b"replace"]),
                host.machine_id,
                SerialKey::Runtime(old.id),
                &NodeRuntimeStop {
                    runtime_id: old.id,
                    reason: Some(orch_driver_contract::runtime::UnitReason::Recreate),
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
        {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            envs::set_runtime_state(
                &mut conn,
                old.id,
                &envs::runtime_state::LIVE,
                "stopped",
                None,
                job.now(),
            )
            .await?;
        }
        if let Target::Environment(id) = self.recipe.target {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            envs::close_shells(&mut conn, id, job.now()).await?;
        }
        if self.recipe.preserve_files
            && let Target::Environment(environment) = self.recipe.target
        {
            let restart = host
                .drivers
                .get(&old.driver)
                .and_then(|driver| driver.get("recreate"))
                .and_then(Json::as_str)
                == Some("restart");
            if !restart && job.plan().await?.get("kept_image").is_none() {
                let generation = i64::from(old.generation) + 1;
                job.remember(
                    &json!({"keep_intent":{"environment":environment,"generation":generation}}),
                )
                .await?;
                let image = job
                    .command(
                        Outgoing::new(
                            ids::derived("node.keep.files", &[job.id().as_bytes()]),
                            host.machine_id,
                            SerialKey::Runtime(old.id),
                            &orch_link_contract::commands::NodeKeepFiles {
                                runtime_id: old.id,
                                environment_id: environment,
                                generation,
                                capture: job.id(),
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                let image: orch_driver_contract::kept::KeptImage =
                    serde_json::from_value(image).map_err(EngineError::from)?;
                let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
                let next = job
                    .plan()
                    .await?
                    .get("runtime")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
                    .ok_or_else(|| fail("runtime_gone", "the successor is missing"))?;
                sqlx::query("UPDATE runtimes SET kept_image=$2,updated_at=$3 WHERE id=$1")
                    .bind(next)
                    .bind(json!(image))
                    .bind(job.now())
                    .execute(&mut *tx)
                    .await
                    .map_err(DbError::from)?;
                jobs::merge_plan(&mut tx, job.id(), &json!({"kept_image":image}), job.now())
                    .await?;
                tx.commit().await.map_err(DbError::from)?;
            }
        }
        let row = job.row().await?;
        let carry = job.plan().await?.get("carry").and_then(Json::as_bool) == Some(true);
        for phase in ["image", "tools", "create", "enroll"] {
            if phase == "create"
                && let Target::Provider(provider) = self.recipe.target
                && self.recipe.volume_carry.is_some()
            {
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                let planned = job.plan().await?;
                let next: Id = serde_json::from_value(planned["runtime"].clone())
                    .map_err(EngineError::from)?;
                let runtime = envs::runtime(&mut conn, next)
                    .await?
                    .ok_or_else(|| fail("runtime_gone", "the destination runtime is gone"))?;
                let host = hosts::host(&mut conn, runtime.host_id)
                    .await?
                    .ok_or_else(|| fail("no_host", "the destination host is gone"))?;
                drop(conn);
                for (role, volume) in runtime.volumes.as_object().into_iter().flatten() {
                    if volume
                        .get("directory")
                        .is_some_and(|directory| !directory.is_null())
                    {
                        continue;
                    }
                    let name = volume["name"].as_str().ok_or_else(|| {
                        fail("data_root_missing", "the destination volume has no name")
                    })?;
                    job.command(
                        Outgoing::new(
                            ids::derived(
                                "node.volume.ensure",
                                &[job.id().as_bytes(), role.as_bytes()],
                            ),
                            host.machine_id,
                            SerialKey::Volume(name.into()),
                            &orch_link_contract::commands::NodeVolumeEnsure {
                                driver: runtime.driver.clone(),
                                name: name.into(),
                                storage: volume["storage"].as_str().map(str::to_owned),
                                labels: std::collections::BTreeMap::from([
                                    ("orch.provider_id".into(), provider.to_string()),
                                    ("orch.runtime_id".into(), runtime.id.to_string()),
                                    ("orch.carried".into(), (role == "home").to_string()),
                                ]),
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                }
                crate::volume_carry::apply(job, &runtime, host.machine_id, provider).await?;
            }
            if phase == "enroll" && carry {
                self.carry(job, &old).await?;
            }
            if let Target::Environment(_) = self.recipe.target {
                crate::jobs::provision::Provision::of(job.engine().clone(), &row)
                    .run(job, phase)
                    .await?;
            } else {
                crate::jobs::provider_provision::ProviderProvision::of(job.engine().clone(), &row)
                    .run(job, phase)
                    .await?;
            }
        }
        if let Target::Environment(_) = self.recipe.target {
            crate::jobs::provision::Provision::of(job.engine().clone(), &row)
                .run(job, "attach")
                .await?;
        }
        Ok(Done::Finished)
    }

    /// Gives a provider's new unit on another host the sign-ins and harness homes its old unit kept, before it first
    /// starts: the old host's node puts up the files below `harness/<provider>` of the stopped old unit's state as one
    /// archive, and the new host's node puts them below the same directory of the new unit's state, with their owners
    /// and modes, through the blob store as an environment's files go when it moves. Nothing is carried where the old
    /// unit kept none; the archive is kept until the job ends.
    async fn carry(&self, job: &Job, old: &envs::RuntimeFull) -> Result<(), Stop> {
        let Target::Provider(provider) = self.recipe.target else {
            return Ok(());
        };
        let plan = job.plan().await?;
        let next: Id =
            serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
        let target: Id = serde_json::from_value(plan["host"].clone()).map_err(EngineError::from)?;
        let path = format!("harness/{provider}");
        let archive = if let Some(kept) = plan.get("carried") {
            serde_json::from_value::<Option<orch_link_contract::commands::BlobRef>>(kept.clone())
                .map_err(EngineError::from)?
        } else {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            let from = hosts::host(&mut conn, old.host_id)
                .await?
                .ok_or_else(|| fail("no_host", "the old host is gone"))?;
            drop(conn);
            let value = job
                .command(
                    Outgoing::new(
                        ids::derived("node.runtime.export", &[job.id().as_bytes()]),
                        from.machine_id,
                        SerialKey::Runtime(old.id),
                        &orch_link_contract::commands::NodeRuntimeExport {
                            runtime_id: old.id,
                            path: path.clone(),
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            let exported: orch_link_contract::commands::NodeRuntimeExportResult =
                serde_json::from_value(value).map_err(EngineError::from)?;
            let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
            if let Some(archive) = exported.archive {
                orch_db::queries::storage::pin_blobs(&mut tx, &[archive.hash]).await?;
            }
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({
                    "carried": exported.archive,
                    "carry_pins": exported.archive.map(|archive| vec![archive.hash]).unwrap_or_default(),
                }),
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
            exported.archive
        };
        let Some(archive) = archive else {
            return Ok(());
        };
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let to = hosts::host(&mut conn, target)
            .await?
            .ok_or_else(|| fail("no_host", "the selected host is gone"))?;
        drop(conn);
        job.command(
            Outgoing::new(
                ids::derived("node.runtime.import", &[job.id().as_bytes()]),
                to.machine_id,
                SerialKey::Runtime(next),
                &orch_link_contract::commands::NodeRuntimeImport {
                    runtime_id: next,
                    path,
                    archive,
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
        Ok(())
    }

    async fn verify_inner(&self, job: &Job) -> Result<Done, Stop> {
        let plan = job.plan().await?;
        let next: Id =
            serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let candidate = envs::runtime(&mut conn, next)
            .await?
            .ok_or_else(|| fail("runtime_gone", "the candidate is gone"))?;
        if !crate::network::verified(&mut conn, next, &self.recipe.network_policy).await? {
            return Err(fail(
                "network.restricted_unavailable",
                "the candidate's host has not verified the requested restriction",
            ));
        }
        let (machine, online) = hosts::machine_of_runtime(&mut conn, next)
            .await?
            .ok_or_else(|| fail("unhealthy", "the candidate has no connector"))?;
        if !online || candidate.state == "failed" {
            return Err(fail(
                "unhealthy",
                "the candidate connector is not online or its unit failed",
            ));
        }
        match self.recipe.target {
            Target::Environment(_) => {
                let facts = plan
                    .get("candidate_facts")
                    .ok_or_else(|| fail("unhealthy", "the candidate was not attached"))?;
                let wrong = crate::jobs::health::findings(&crate::jobs::health::Side {
                    online,
                    facts,
                    runtime_state: &candidate.state,
                });
                if !wrong.is_empty() {
                    return Err(fail("unhealthy", wrong.join("; ")));
                }
            }
            Target::Provider(provider) => {
                let state = providers::state(&mut conn, provider)
                    .await?
                    .ok_or_else(|| fail("provider_gone", "the provider is gone"))?;
                // A separate native keeper was not replaced and its sign-in is not in this home.
                // Verify the candidate's services; a later binding may request a permitted copy.
                let keeps_native = state.key_machine_id.is_none()
                    || providers::keeper_has_unit(&mut conn, provider).await?;
                if let Some(agent) = state.agent_id
                    && keeps_native
                {
                    let agent = orch_db::queries::resources::thread_engine::agent(&mut conn, agent)
                        .await?
                        .ok_or_else(|| fail("unhealthy", "the provider profile is gone"))?;
                    let provider_config =
                        crate::harnesses::provider_config_of(&mut conn, provider).await?;
                    let mut runtime =
                        crate::provider_runtime::harness_values(&mut conn, provider).await?;
                    runtime.home=Some(match candidate.volumes.pointer("/home/directory/path").and_then(Json::as_str) {
                        Some(path)=>orch_driver_contract::runtime::EnvironmentDataSource::ExistingDirectory {path:path.into()},
                        None=>orch_driver_contract::runtime::EnvironmentDataSource::MadeByOrch,
                    });
                    let runtime = Some(runtime);
                    drop(conn);
                    job.command(
                        Outgoing::new(
                            ids::derived("agent.start", &[job.id().as_bytes()]),
                            machine,
                            SerialKey::Harness {
                                provider_id: provider,
                                agent_id: agent.id,
                            },
                            &AgentStart {
                                provider_id: provider,
                                agent_id: agent.id,
                                harness_kind: agent.harness_kind,
                                profile: serde_json::from_value(agent.profile)
                                    .map_err(EngineError::from)?,
                                home: None,
                                provider_config,
                                runtime,
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                    let facts_source = {
                        let mut conn = job
                            .engine()
                            .db()
                            .acquire()
                            .await
                            .map_err(EngineError::from)?;
                        let row = orch_db::queries::resources::thread_engine::provider(
                            &mut conn, provider,
                        )
                        .await?
                        .ok_or_else(|| fail("provider_gone", "the provider is gone"))?;
                        crate::provider_sources::spec(
                            &mut conn,
                            provider,
                            &row.sources,
                            orch_provider_contract::SourceElement::Facts,
                        )
                        .await?
                    };
                    let refreshed = job
                        .command(
                            Outgoing::new(
                                ids::derived("provider.refresh", &[job.id().as_bytes()]),
                                machine,
                                SerialKey::Provider(provider),
                                &ProviderRefresh {
                                    provider_id: provider,
                                    retry: None,
                                    renew: None,
                                    facts: facts_source,
                                },
                            )
                            .map_err(EngineError::from)?,
                        )
                        .await?;
                    if self.recipe.old_facts["provider_status"] == "ready"
                        && refreshed.get("status").and_then(Json::as_str) != Some("ready")
                    {
                        return Err(fail(
                            "sign_in_lost",
                            "the candidate did not report its previous sign-in",
                        ));
                    }
                }
            }
        }
        Ok(Done::Finished)
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        let plan = job.plan().await?;
        let next: Id =
            serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
        let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
        let locked = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| fail("job_gone", "the job is gone"))?;
        if locked.cancel_requested() {
            return Err(Stop::Cancelled);
        }
        if !env_changes::commit(
            &mut tx,
            self.recipe.old,
            next,
            plan.get("candidate_facts"),
            job.now(),
        )
        .await?
        {
            if locked.plan.get("committed").and_then(Json::as_bool) == Some(true) {
                return Ok(Done::Finished);
            }
            return Err(fail(
                "conflict",
                "the successor no longer belongs to this replacement",
            ));
        }
        let runtime = envs::runtime(&mut tx, next)
            .await?
            .ok_or_else(|| fail("runtime_gone", "the runtime is gone"))?;
        if let Target::Provider(provider) = self.recipe.target
            && self.recipe.volume_carry.is_some()
            && self
                .recipe
                .data_sources
                .as_ref()
                .is_some_and(|sources| sources.home.directory().is_none())
        {
            sqlx::query("UPDATE providers SET runtime_config=jsonb_set(runtime_config,'{runtime.home}','{\"kind\":\"made_by_orch\"}'::jsonb),revision=revision+1,updated_at=$2 WHERE id=$1")
                .bind(provider).bind(job.now()).execute(&mut *tx).await.map_err(DbError::from)?;
        }
        let template = hosts::template(&mut tx, runtime.template_id)
            .await?
            .ok_or_else(|| fail("template_unavailable", "the template is gone"))?;
        let mut unit = Unit::new(&mut tx, job.now());
        for thread in &self.recipe.threads {
            let Some(row) = threads::lock(&mut *unit.conn, *thread).await? else {
                continue;
            };
            renew_segment(
                &mut unit,
                *thread,
                if matches!(self.recipe.target, Target::Environment(_)) {
                    Some(runtime.generation)
                } else {
                    None
                },
                "environment_recreated",
                self.recipe.by,
                self.recipe.auto_start,
            )
            .await?;
            if let (Target::Environment(_), Some(segment)) =
                (self.recipe.target, row.current_segment_id)
            {
                let marker = orch_api_contract::threads::MarkerPayload::EnvironmentRecreated(
                    orch_api_contract::threads::MarkerEnvironmentRecreated {
                        generation: i64::from(runtime.generation),
                        reason: if self.recipe.reason == UpdatePreflightReason::Update {
                            orch_api_contract::threads::RecreatedReason::Update
                        } else {
                            orch_api_contract::threads::RecreatedReason::Recreate
                        },
                        template: orch_api_contract::threads::MarkerTemplate {
                            id: template.id,
                            name: template.name.clone(),
                        },
                    },
                );
                crate::ingest::add_marker(
                    &mut unit,
                    *thread,
                    segment,
                    None,
                    &serde_json::to_value(marker).map_err(EngineError::from)?,
                    &format!("recreated:{}", job.id()),
                )
                .await?;
            }
            env_changes::notice(
                &mut *unit.conn,
                *thread,
                &replaced_notice(&self.recipe, runtime.generation),
                job.now(),
            )
            .await?;
        }
        let finished = unit.finish().await?;
        audit_change(&mut tx, &self.recipe, job.id(), false, job.now()).await?;
        let (resource, provider) = match self.recipe.target {
            Target::Environment(id) => (id, false),
            Target::Provider(id) => (id, true),
        };
        crate::resource_facts::library::created(
            &mut tx,
            runtime.template_id,
            (!provider).then_some(runtime.host_id),
            resource,
            provider,
            Some(self.recipe.by),
            orch_types::Time::new(job.now()),
        )
        .await?;
        jobs::merge_plan(&mut tx, job.id(), &json!({"committed":true}), job.now()).await?;
        hosts::delete_runtime_secret(&mut tx, next, "enroll_token").await?;
        job.engine().commit(tx, finished).await?;
        Ok(Done::Finished)
    }

    async fn rollback(&self, job: &Job) -> Result<Option<String>, Stop> {
        let plan = job.plan().await?;
        if plan.get("committed").and_then(Json::as_bool) == Some(true) {
            return Ok(None);
        }
        let mut cleanup_detail = None;
        let old = self.old(job).await?;
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let host = hosts::host(&mut conn, old.host_id)
            .await?
            .ok_or_else(|| fail("no_host", "the old host is gone"))?;
        drop(conn);
        // A successor made on another host is removed there.
        let successor_machine = match plan
            .get("host")
            .and_then(|v| serde_json::from_value::<Id>(v.clone()).ok())
            .filter(|other| *other != old.host_id)
        {
            Some(other) => {
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                hosts::host(&mut conn, other)
                    .await?
                    .map_or(host.machine_id, |other| other.machine_id)
            }
            None => host.machine_id,
        };
        if let Some(next) = plan
            .get("runtime")
            .cloned()
            .and_then(|v| serde_json::from_value::<Id>(v).ok())
        {
            let command = Outgoing::new(
                ids::derived("node.runtime.remove", &[job.id().as_bytes(), b"undo"]),
                successor_machine,
                SerialKey::Runtime(next),
                &NodeRuntimeRemove {
                    runtime_id: next,
                    keep_volumes: true,
                    volume_refs: Vec::new(),
                },
            )
            .map_err(EngineError::from)?;
            let cleanup = match job.command_to_undo(command).await {
                Ok(_) => None,
                Err(Stop::Failed(f)) if f.code == "not_found" => None,
                Err(error) => Some(error),
            };
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            if let Some(error) = cleanup {
                // A failed cleanup must not keep the old unit stopped. The reaper can remove
                // this candidate later; its volumes are still protected by the current unit.
                let detail = format!("candidate cleanup is pending after rollback: {error:?}");
                envs::retire_runtime(&mut conn, next, Some(&detail), job.now()).await?;
                drop(conn);
                job.remember(&json!({"cleanup_pending":next,"cleanup_detail":detail}))
                    .await?;
                cleanup_detail = Some(detail);
            } else {
                env_changes::discard(&mut conn, next, job.now()).await?;
                hosts::delete_runtime_secret(&mut conn, next, "enroll_token").await?;
            }
        }
        let mut guard_conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let guarded = crate::network::blocked(&mut guard_conn, old.id)
            .await?
            .is_some();
        let disabled = hosts::host(&mut guard_conn, old.host_id)
            .await?
            .is_some_and(|host| host.disabled)
            || match self.recipe.target {
                Target::Environment(id) => envs::environment(&mut guard_conn, id)
                    .await?
                    .is_some_and(|env| env.status == "disabled"),
                Target::Provider(id) => providers::state(&mut guard_conn, id)
                    .await?
                    .is_some_and(|provider| provider.status == "disabled"),
            };
        drop(guard_conn);
        let resume = !guarded && !disabled && (self.recipe.auto_start || !self.recipe.was_stopped);
        if resume
            && (plan.get("old_stop_requested").and_then(Json::as_bool) == Some(true)
                || self.recipe.auto_start
                || old.state == "failed")
        {
            job.command_to_undo(
                Outgoing::new(
                    ids::derived("node.runtime.start", &[job.id().as_bytes(), b"undo"]),
                    host.machine_id,
                    SerialKey::Runtime(old.id),
                    &NodeRuntimeStart { runtime_id: old.id },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
            super::after_up::restored(job, old.id).await?;
        }
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        envs::set_runtime_state(
            &mut conn,
            old.id,
            &envs::runtime_state::LIVE,
            if resume { "running" } else { "stopped" },
            None,
            job.now(),
        )
        .await?;
        if let Target::Environment(id) = self.recipe.target {
            envs::set_status(
                &mut conn,
                id,
                &["recreating"],
                if resume { "running" } else { "stopped" },
                None,
                job.now(),
            )
            .await?;
            envs::record_facts(
                &mut conn,
                id,
                &self.recipe.old_facts,
                self.recipe
                    .old_facts
                    .get("workspace_root")
                    .and_then(Json::as_str),
                job.now(),
            )
            .await?;
        }
        if let Target::Provider(id) = self.recipe.target {
            providers::set_machine(
                &mut conn,
                id,
                if resume { old.machine_id } else { None },
                job.now(),
            )
            .await?;
        }
        audit_change(&mut conn, &self.recipe, job.id(), true, job.now()).await?;
        Ok(cleanup_detail)
    }
}

/// Opens the next segment without discarding a session that stays on persistent state or with its provider.
pub(crate) async fn renew_segment(
    unit: &mut Unit<'_>,
    thread: Id,
    generation: Option<i32>,
    reason: &str,
    by: Id,
    keep_starting: bool,
) -> Result<(), Stop> {
    let Some(row) = threads::thread(&mut *unit.conn, thread).await? else {
        return Ok(());
    };
    let Some(current) = row.current_segment_id else {
        return Ok(());
    };
    let Some(segment) = threads::segment(&mut *unit.conn, current).await? else {
        return Ok(());
    };
    let seq = unit.stamp(thread).await?;
    let now = unit.now();
    threads::end_segment(&mut *unit.conn, current, reason, now, seq).await?;
    let next = new_id();
    let environment = row.environment_id.unwrap_or(segment.environment_id);
    threads::insert_segment(
        &mut *unit.conn,
        &threads::NewSegment {
            id: next,
            thread_id: thread,
            agent_id: segment.agent_id,
            provider_id: segment.provider_id,
            environment_id: environment,
            env_generation: generation.unwrap_or(segment.env_generation),
            arrangement: &segment.arrangement,
            executor: segment.executor.as_deref(),
            tool_set: segment.tool_set.as_deref(),
            credential_path: &segment.credential_path,
            native_session_ref: segment.native_session_ref.as_deref(),
            started_by: Some(by),
            started_at: now,
        },
        seq,
    )
    .await?;
    let active = if keep_starting {
        threads::active_turn(&mut *unit.conn, thread)
            .await?
            .filter(|t| t.state == "starting")
    } else {
        None
    };
    if let Some(turn) = &active {
        env_changes::starting_segment(&mut *unit.conn, turn.id, next, seq).await?;
    }
    threads::set_current(
        &mut *unit.conn,
        thread,
        Some(next),
        active.map(|t| t.id),
        seq,
    )
    .await?;
    Ok(())
}

/// Writes the environment or provider's short history line in the transaction of its change.
pub(crate) async fn audit_change(
    conn: &mut Conn,
    recipe: &Recipe,
    job: Id,
    rolled_back: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), Stop> {
    use orch_db::audit::{self, Actor, Entry, Origin};
    if rolled_back {
        for thread in &recipe.threads {
            env_changes::notice(
                conn,
                *thread,
                "The runtime change did not finish. The previous unit is usable again; files and conversations are unchanged.",
                now,
            ).await?;
        }
        if recipe.auto_start {
            let (node, people, kind, path) = match recipe.target {
                Target::Environment(id) => (
                    envs::environment(conn, id).await?.map(|e| e.home_node_id),
                    envs::managers_of(conn, id).await?,
                    crate::notifications::Kind::EnvironmentUpdate,
                    format!("/environments/{id}"),
                ),
                Target::Provider(id) => (
                    providers::state(conn, id).await?.map(|p| p.home_id),
                    providers::managers_of(conn, id).await?,
                    crate::notifications::Kind::ProviderAttention,
                    format!("/providers/{id}"),
                ),
            };
            let org = match node {
                Some(node) => orch_db::queries::notifications::subscribers::path_of(conn, node)
                    .await?
                    .first()
                    .copied(),
                None => None,
            };
            let event = crate::notifications::Event {
                kind,
                node,
                thread: None,
                approval: None,
                title: "Update on start was rolled back".to_owned(),
                body:
                    "The previous runtime started instead. Its files and conversations are intact."
                        .to_owned(),
                data: json!({"job_id":job,"path":path}),
                source: job.to_string(),
                actor: None,
                at: now,
            };
            let mut managers = Vec::new();
            for person in people {
                let principal = orch_authz::Principal::load(
                    conn,
                    orch_authz::Caller::session(person),
                    orch_types::Time::new(now),
                )
                .await
                .map_err(EngineError::from)?;
                if crate::updates::may_manage(conn, &principal, recipe.target).await? {
                    managers.push(person);
                }
            }
            crate::notifications::tell(conn, &event, &managers, org).await?;
        }
        // A failed replacement is reported by its job, never as an update that was applied.
        return Ok(());
    }
    let environment = match recipe.target {
        Target::Environment(id) => Some(id),
        _ => None,
    };
    let action = if matches!(recipe.target, Target::Environment(_)) {
        if recipe.reason == UpdatePreflightReason::Update {
            orch_db::audit::name!("environment.update_applied")
        } else {
            orch_db::audit::name!("environment.recreated")
        }
    } else if recipe.reason == UpdatePreflightReason::Update {
        orch_db::audit::name!("provider.runtime_updated")
    } else {
        orch_db::audit::name!("provider.runtime_recreated")
    };
    let mut entry = Entry::new(
        if recipe.auto_start {
            Actor::System
        } else {
            Actor::User(recipe.by)
        },
        action,
        if environment.is_some() {
            "environment"
        } else {
            "provider"
        },
        Some(recipe.target.id()),
        if rolled_back {
            "The runtime change was rolled back"
        } else {
            "The runtime was replaced, keeping its persistent data"
        },
    );
    entry.environment_id = environment;
    entry.history = true;
    entry.details = json!({"job":job,"reason":recipe.reason,"rolled_back":rolled_back});
    audit::append(conn, &entry, &Origin::default(), now).await?;
    if let Some(environment) = environment {
        let old = envs::runtime(conn, recipe.old).await?;
        let current = envs::live_runtime(conn, environment).await?;
        if let (Some(old), Some(current)) = (old, current)
            && old.template_id != current.template_id
        {
            let mut changed = Entry::new(
                entry.actor,
                orch_db::audit::name!("environment.template_changed"),
                "environment",
                Some(environment),
                "Changed the environment's template",
            );
            changed.history = true;
            changed.environment_id = Some(environment);
            changed.node_id = envs::environment(conn, environment)
                .await?
                .map(|row| row.home_node_id);
            changed.details = json!({"job":job,"from_template":old.template_id,"to_template":current.template_id});
            audit::append(conn, &changed, &Origin::default(), now).await?;
        }
    }
    Ok(())
}

/// What an agent is told once its environment was made again: why, and whether what it installed outside the volumes is
/// kept. Where it is gone the line says what to do instead.
fn replaced_notice(recipe: &Recipe, generation: i32) -> String {
    let reason = if recipe.capability.is_some() {
        "a capability changed"
    } else if recipe.reason == UpdatePreflightReason::Update {
        "an update was applied"
    } else {
        "it was recreated"
    };
    if recipe.preserve_files {
        format!(
            "The runtime was recreated (generation {generation}). Earlier processes are gone; everything on disk and conversations are intact."
        )
    } else {
        format!(
            "{} Earlier processes are gone; the volumes and conversations are intact.",
            crate::isolation::replaced_line(reason, false).trim_start_matches("- ")
        )
    }
}

#[async_trait]
impl Phased for Recreating {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "notify" => {
                let mut message = if self.recipe.preserve_files {
                    "The runtime will be recreated shortly. Running processes stop; everything on disk, installed programs and conversations are kept.".to_owned()
                } else {
                    "The runtime will be recreated shortly. Running processes stop; persistent volumes and conversations are kept. The successor starts from its image, so installed programs and changes outside the volumes do not carry over.".to_owned()
                };
                if let Some(estimate) = self.recipe.keep_estimate {
                    message.push(' ');
                    message.push_str(&crate::kept_images::recording_note(Some(estimate)));
                }
                self.notice(job, &message).await?;
                Ok(Done::Finished)
            }
            "lock" => self.lock(job).await,
            "replace" => self.replace(job).await,
            "after_up" => super::after_up::run(job).await,
            "verify" => {
                super::after_up::run(job).await?;
                tokio::time::timeout(
                    Duration::from_secs(self.recipe.options.verify_timeout_secs as u64),
                    self.verify_inner(job),
                )
                .await
                .map_err(|_| {
                    fail(
                        "verify_timeout",
                        "the candidate did not pass its checks in time",
                    )
                })?
            }
            "commit" => self.commit(job).await,
            _ => Err(fail(
                "internal",
                format!("a replacement has no phase {phase}"),
            )),
        }
    }
    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        if super::after_up::record_interrupted(job).await.is_err() {
            tracing::warn!("an interrupted replacement command outcome could not be recorded");
        }
        match self.rollback(job).await {
            Ok(None) => Undone::Everything,
            Ok(Some(detail)) => Undone::Partly(detail),
            Err(error) => {
                let detail = format!("the old unit could not be restored: {error:?}");
                if let Target::Environment(id) = self.recipe.target
                    && let Ok(mut conn) = job.engine().db().acquire().await
                {
                    let _ = envs::set_status(
                        &mut conn,
                        id,
                        &["recreating"],
                        "failed",
                        Some(&detail),
                        job.now(),
                    )
                    .await;
                }
                Undone::Partly(detail)
            }
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        Some(json!({"runtime":job.plan().await.ok()?.get("runtime"),"reason":self.recipe.reason}))
    }
}

impl Engine {
    /// Lets queued inputs run after a replacement ended, whether successful, failed or cancelled.
    pub(crate) async fn after_recreate(&self, row: &jobs::JobRow) {
        self.after_volume_carry(row).await;
        if row.plan.get("committed").and_then(Json::as_bool) != Some(true)
            && row.plan.get("capability_reason").and_then(Json::as_str) == Some("capability")
            && let (Some(environment), Some(key)) = (
                row.environment_id,
                row.plan.get("capability").and_then(Json::as_str),
            )
            && let Ok(mut conn) = self.db().acquire().await
            && let Err(error) = orch_db::queries::environments::capabilities::revoke(
                &mut conn,
                environment,
                key,
                self.inner.clock.now(),
            )
            .await
        {
            tracing::warn!(%environment,%key,%error,"the failed capability grant could not be rolled back");
        }
        if row.plan.get("recovery").is_none()
            && let Err(error) = self.cleanup_recreated_images(row).await
        {
            tracing::warn!(job=%row.id,%error,"kept-image cleanup waits for a later sweep");
        }
        // A provider's carried files were kept in the blob store only for the move.
        if let Ok(pins) =
            serde_json::from_value::<Vec<orch_types::Hash>>(row.plan["carry_pins"].clone())
            && let Ok(mut conn) = self.db().acquire().await
            && let Err(error) =
                orch_db::queries::storage::unpin_blobs(&mut conn, &pins, self.inner.clock.now())
                    .await
        {
            tracing::warn!(%error, "a move's carried files could not be let go");
        }
        let threads = match serde_json::from_value::<Recipe>(row.plan.clone()) {
            Ok(recipe) => recipe.threads,
            Err(_) => match self.db().acquire().await {
                Ok(mut conn) => {
                    rows::threads(&mut conn, row.environment_id, row.provider_id, false)
                        .await
                        .unwrap_or_default()
                }
                Err(_) => Vec::new(),
            },
        };
        for thread in threads {
            if let Err(error) = self.release_lock(thread).await {
                tracing::warn!(%error,%thread,"a runtime change could not release a thread; the next sweep looks again");
            }
        }
    }
}
