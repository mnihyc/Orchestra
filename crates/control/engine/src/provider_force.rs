//! A person forces the stop or the removal of a provider's unit: what is in the way is ended with its reason, the
//! record is settled, and the host is told until it has answered.
//!
//! A unit the host does not answer for, or that work holds, cannot be stopped or removed in the ordinary way. A
//! person who manages the provider or the host can force it, and what that does is told in the order it happens:
//!
//! - **What can refuse comes first.** A turn that runs, work of something else on the provider: nothing is ended
//!   for an act that is then refused.
//! - **Work of the provider's own that is in the way is ended.** An update is ended by its own undo, which takes
//!   the second unit away and puts the first back, so that nothing runs beside it on their one volume; the undo is
//!   given as long as a cancelled job is given before it is said to be still stopping (`jobs.stop_wait_secs`), and
//!   when the host does not answer in that time the update is ended where it stands and its second unit is
//!   retired, to be stopped and removed as soon as the host answers, with the shared volume kept. Making,
//!   starting and renewing a unit, and the removal of another unit, are ended where they stand: what they made
//!   is the stop's or the removal's to take away.
//! - **The record is settled.** A unit that is stopped by force reads as stopped at once, with the reason that a
//!   person forced it, and the provider has no machine.
//! - **The host is told until it has said that it stopped the unit.** The record keeps the reason of a stop that
//!   the host has not confirmed (`provider_forced_stop`); the schedule tells a host that is online again and
//!   again, with a new command each time, and when the host answers the reason is the plain one of a person's
//!   stop.

use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::provider_runtime::{Hold, HoldWhy, RuntimeError, StopWhy, hold_of};
use orch_db::DbError;
use orch_db::queries::environments::hosting::{self as env_rows, RuntimeFull, runtime_state};
use orch_db::queries::jobs::{JobRow, kind as job_kind, provider_jobs};
use orch_db::queries::resources::hosting as host_rows;
use orch_db::queries::resources::provider_runtime as rows;
use orch_link_contract::commands::{NodeRuntimeStop, SerialKey};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, new_id};
use std::time::Duration;

/// How many units one look at the schedule tells their hosts of.
const FORCED_BATCH: i64 = 100;
/// How often the end of an update is looked at while its undo runs.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// What a person's forced act is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Forcing {
    /// The stop of this unit.
    Stop {
        /// The unit.
        runtime: Id,
    },
    /// The removal of this unit, with the choice of volumes the manager made.
    Removal {
        /// The unit.
        runtime: Id,
        /// Whether its volumes go too.
        volumes: bool,
    },
}

/// The identifier a job's plan keeps under `member`.
fn planned(job: &JobRow, member: &str) -> Option<Id> {
    job.plan
        .get(member)
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
}

impl Engine {
    /// Stops a unit by force: what can refuse comes first, then the work that is in the way is ended, then the
    /// record says stopped and the host is told.
    pub(crate) async fn stop_by_force(
        &self,
        provider: Id,
        runtime: &RuntimeFull,
        interrupt: bool,
        why: StopWhy,
    ) -> Result<(), RuntimeError> {
        let StopWhy::Person(by) = why else {
            return Err(RuntimeError::Unavailable(
                "only a person stops a unit by force".to_owned(),
            ));
        };
        // A unit the record calls stopped has no turn to wait for; the person means the host.
        if !matches!(
            runtime.state.as_str(),
            runtime_state::STOPPED | runtime_state::FAILED
        ) {
            self.clear_provider_turns(provider, interrupt, why).await?;
        }
        let ended = self
            .end_provider_work(
                provider,
                by,
                Forcing::Stop {
                    runtime: runtime.id,
                },
            )
            .await?;
        if !ended.is_empty() {
            tracing::warn!(%provider, runtime = %runtime.id, ended_jobs = ?ended, "work in the way of a forced stop of a provider's unit was ended");
        }
        self.force_provider_stop(provider, runtime.id).await
    }

    /// Ends the work of the provider's own that stands in the way of a forced act, and answers which jobs it ended.
    /// A job that has passed its commit is finishing and is not ended: the person is told.
    pub(crate) async fn end_provider_work(
        &self,
        provider: Id,
        by: Id,
        forcing: Forcing,
    ) -> Result<Vec<Id>, RuntimeError> {
        let open = {
            let mut conn = self.inner.db.acquire().await?;
            provider_jobs::open_of_provider(&mut conn, provider).await?
        };
        let mut ended = Vec::new();
        for job in open {
            let same_removal = match forcing {
                Forcing::Removal { runtime, volumes } => {
                    job.kind == provider_jobs::REMOVE
                        && planned(&job, "runtime") == Some(runtime)
                        && job
                            .plan
                            .get("volumes")
                            .and_then(Json::as_bool)
                            .unwrap_or(true)
                            == volumes
                }
                Forcing::Stop { .. } => false,
            };
            // The removal of a unit works on that unit; a unit that is stopped does not wait for it.
            if same_removal
                || (matches!(forcing, Forcing::Stop { .. }) && job.kind == provider_jobs::REMOVE)
            {
                continue;
            }
            if job.kind == job_kind::PROVIDER_RECREATE {
                self.end_update(provider, &job, by, forcing).await?;
            } else if !self
                .overrule_job(
                    job.id,
                    by,
                    "A person who manages the provider or its host ended this work by forcing a stop or a removal.",
                )
                .await?
            {
                return Err(RuntimeError::Held(past_commit(&job)));
            }
            ended.push(job.id);
        }
        Ok(ended)
    }

    /// Ends an update that works on a provider's unit: by its own undo, which takes the second unit away and puts
    /// the first back; and when the host does not let that finish in time, where it stands.
    async fn end_update(
        &self,
        provider: Id,
        job: &JobRow,
        by: Id,
        forcing: Forcing,
    ) -> Result<(), RuntimeError> {
        // The unit that is stopped, or taken away, stays so once the update is put back; the other comes back as it
        // was.
        let keeps_old_stopped = match forcing {
            Forcing::Stop { runtime } => planned(job, "runtime") != Some(runtime),
            Forcing::Removal { runtime, .. } => planned(job, "old") == Some(runtime),
        };
        {
            let mut conn = self.inner.db.acquire().await?;
            orch_db::queries::jobs::merge_plan(
                &mut conn,
                job.id,
                &serde_json::json!({ "keep_old_stopped": keeps_old_stopped }),
                self.inner.clock.now(),
            )
            .await?;
        }
        if !self.cancel_job(job.id, by).await? {
            // Over already, or past the point after which nothing is undone.
            let latest = {
                let mut conn = self.inner.db.acquire().await?;
                orch_db::queries::jobs::job(&mut conn, job.id).await?
            };
            return match latest {
                Some(latest) if latest.is_open() => Err(RuntimeError::Held(past_commit(&latest))),
                _ => Ok(()),
            };
        }
        let allowed = {
            let mut conn = self.inner.db.acquire().await?;
            let seconds =
                crate::jobs::measurement::setting(&mut conn, keys::JOB_STOP_WAIT_SECS).await?;
            Duration::from_secs(u64::try_from(seconds).unwrap_or_default())
        };
        let began = tokio::time::Instant::now();
        let latest = loop {
            let latest = {
                let mut conn = self.inner.db.acquire().await?;
                orch_db::queries::jobs::job(&mut conn, job.id).await?
            };
            let Some(latest) = latest else {
                return Ok(());
            };
            if !latest.is_open() || began.elapsed() >= allowed {
                break latest;
            }
            tokio::time::sleep(LOOK_EVERY).await;
        };
        if !latest.is_open() {
            tracing::info!(%provider, job = %job.id, state = %latest.state, "an update that a person's forced act stood in the way of was undone");
            return Ok(());
        }
        // The undo does not finish: the host does not answer. The update is ended where it stands, and the unit it
        // made is left to be stopped and removed when the host does.
        if !self
            .overrule_job(
                job.id,
                by,
                "A person who manages the provider or its host ended this update by forcing a stop or a removal, and it could not be undone in time.",
            )
            .await?
        {
            return Err(RuntimeError::Held(past_commit(&latest)));
        }
        self.abandon_successor(provider, &latest, forcing).await
    }

    /// Retires the unit an update made beside the provider's, which the update did not take away: its host stops
    /// and removes it when it can be told, and the volume it shares with the provider's unit stays.
    async fn abandon_successor(
        &self,
        provider: Id,
        job: &JobRow,
        forcing: Forcing,
    ) -> Result<(), RuntimeError> {
        let Some(next) = planned(job, "runtime") else {
            return Ok(());
        };
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        rows::lock(&mut tx, provider).await?;
        let retired = match env_rows::runtime(&mut tx, next).await? {
            Some(row)
                if row.successor_of.is_some()
                    && !matches!(
                        row.state.as_str(),
                        runtime_state::RETIRED | runtime_state::REMOVED
                    ) =>
            {
                env_rows::retire_runtime(&mut tx, next, Some(rows::UPDATE_ENDED), now).await?
            }
            _ => false,
        };
        // A stop has nothing else to queue, so the removal of the second unit is queued at once. A removal of the first
        // unit is queued by the person's act, and the schedule takes the second after it.
        if retired && matches!(forcing, Forcing::Stop { .. }) {
            crate::jobs::provider_provision::queue_removal(&mut tx, now, provider, next).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        if retired {
            tracing::warn!(%provider, runtime = %next, job = %job.id, "the unit an update had made is left to be removed: its host did not answer while the update was undone");
            self.kick();
        }
        Ok(())
    }

    /// Settles the stop of a unit without its host's word: it reads as stopped at once, with the reason that a person
    /// forced it, and the host is told to stop it until it says that it did.
    pub(crate) async fn force_provider_stop(
        &self,
        provider: Id,
        runtime: Id,
    ) -> Result<(), RuntimeError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        rows::lock(&mut tx, provider).await?;
        let Some(row) = env_rows::runtime(&mut tx, runtime).await? else {
            return Err(RuntimeError::NotFound);
        };
        // A unit that is being made, runs, or is being stopped is stopped in the record. One the record already
        // calls stopped or failed says that its host has not said that it stopped it, which is what a person who
        // forces its stop doubts.
        let moved = env_rows::set_runtime_state(
            &mut tx,
            runtime,
            &[
                runtime_state::RUNNING,
                runtime_state::STOPPING,
                runtime_state::CREATING,
                runtime_state::STOPPED,
                runtime_state::FAILED,
            ],
            runtime_state::STOPPED,
            Some(rows::STOPPED_FORCED),
            now,
        )
        .await?;
        if moved
            && rows::live_runtime(&mut tx, provider)
                .await?
                .is_some_and(|current| current.id == runtime)
        {
            self.unit_is_gone(&mut tx, provider, runtime, row.machine_id, now)
                .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        tracing::warn!(%provider, %runtime, host = %row.host_id, was = %row.state, settled = moved, "a person stopped the unit of a provider's runtime by force: it reads as stopped, and its host is told until it says that it stopped it");
        if moved {
            let engine = self.clone();
            tokio::spawn(async move {
                engine.tell_host_of_forced_stop(provider, runtime).await;
            });
        }
        Ok(())
    }

    /// Recovers the forced stops left by an earlier process. Each unit follows its host independently
    /// once it is taken up, so unrelated upkeep cannot hold up another attempt.
    pub(crate) async fn tell_hosts_of_forced_stops(&self) -> Result<(), EngineError> {
        let followed: Vec<Id> = lock(&self.inner.forced_stops).iter().copied().collect();
        let forced = {
            let mut conn = self.inner.db.acquire().await?;
            rows::forced_stops(&mut conn, &followed, FORCED_BATCH).await?
        };
        for runtime in forced {
            let Some(provider) = runtime.provider_id else {
                continue;
            };
            let engine = self.clone();
            tokio::spawn(async move {
                engine.tell_host_of_forced_stop(provider, runtime.id).await;
            });
        }
        Ok(())
    }

    /// Follows one forced stop until its host confirms it or the unit leaves that stop. Refusals and
    /// timeouts are paced by the existing reaper interval, independently of the shared upkeep sweep.
    pub(crate) async fn tell_host_of_forced_stop(&self, provider: Id, runtime: Id) {
        if !lock(&self.inner.forced_stops).insert(runtime) {
            return;
        }
        let mut pending = None;
        while !self.is_stopped() {
            match self
                .ask_host_to_stop_forced(provider, runtime, &mut pending)
                .await
            {
                Ok(false) => break,
                Ok(true) => {}
                Err(error) => {
                    tracing::warn!(%error, %provider, %runtime, "the host could not be told to stop a unit that was stopped by force; it is told again");
                }
            }
            tokio::time::sleep(self.inner.config.hosting.reaper_every).await;
        }
        lock(&self.inner.forced_stops).remove(&runtime);
    }

    /// Answers whether the host still owes confirmation. A local wait that runs out keeps following
    /// its command; only a terminal answer makes room for another command with a fresh identifier.
    async fn ask_host_to_stop_forced(
        &self,
        provider: Id,
        runtime: Id,
        pending: &mut Option<Id>,
    ) -> Result<bool, RuntimeError> {
        let (row, host) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(row) = env_rows::runtime(&mut conn, runtime).await? else {
                return Ok(false);
            };
            let host = host_rows::host(&mut conn, row.host_id).await?;
            (row, host)
        };
        // What was forced is settled meanwhile (the host said so, the unit was started again, taken away).
        if row.state != runtime_state::STOPPED
            || row.state_detail.as_deref() != Some(rows::STOPPED_FORCED)
        {
            return Ok(false);
        }
        let Some(host) = host else {
            return Err(RuntimeError::Host("the host is gone".to_owned()));
        };
        if !host.online {
            return Ok(true);
        }
        if let Some(id) = *pending
            && self
                .inner
                .commands
                .status(id)
                .await
                .map_err(EngineError::from)?
                .is_none()
        {
            // An issue that failed before retaining its command has nothing left to follow.
            *pending = None;
        }
        let id = if let Some(id) = *pending {
            id
        } else {
            let command = Outgoing::new(
                new_id(),
                host.machine_id,
                SerialKey::Runtime(runtime),
                &NodeRuntimeStop {
                    runtime_id: runtime,
                    reason: Some(orch_driver_contract::runtime::UnitReason::Stop),
                },
            )
            .map_err(EngineError::from)?;
            let id = command.id;
            *pending = Some(id);
            tracing::info!(%provider, %runtime, host = %row.host_id, command = %id, "the host is told to stop a unit that a person stopped by force");
            self.inner
                .commands
                .issue(command)
                .await
                .map_err(EngineError::from)?;
            id
        };
        let waited = tokio::time::timeout(
            self.inner.config.hosting.stop_wait,
            self.inner.commands.wait(id),
        )
        .await;
        if matches!(&waited, Ok(Ok(_))) {
            *pending = None;
        }
        // A host that has no such unit has nothing left to stop.
        let missing = matches!(
            &waited,
            Ok(Ok(Outcome::Failed(error)))
                if error.code == orch_link_contract::commands::ErrorCode::NotFound.as_str()
        );
        if !(missing || matches!(&waited, Ok(Ok(Outcome::Succeeded(_))))) {
            let reason = match &waited {
                Ok(Ok(Outcome::Failed(error))) => format!("{}: {}", error.code, error.detail),
                _ => "it did not answer in time".to_owned(),
            };
            tracing::warn!(%provider, %runtime, host = %row.host_id, %reason, "the host did not stop a unit that a person stopped by force; it is told again");
            return Ok(true);
        }
        let mut tx = self.inner.db.begin().await?;
        rows::lock(&mut tx, provider).await?;
        let Some(current) = env_rows::runtime(&mut tx, runtime).await? else {
            return Ok(false);
        };
        if current.state != runtime_state::STOPPED
            || current.state_detail.as_deref() != Some(rows::STOPPED_FORCED)
        {
            return Ok(false);
        }
        let moved = env_rows::set_runtime_state(
            &mut tx,
            runtime,
            &[runtime_state::STOPPED],
            runtime_state::STOPPED,
            Some(rows::STOPPED_PERSON),
            self.inner.clock.now(),
        )
        .await?;
        if moved && missing {
            env_rows::set_unit_missing(&mut tx, runtime, true).await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        tracing::info!(%provider, %runtime, host = %row.host_id, missing, "the host stopped the unit that a person stopped by force");
        Ok(false)
    }
}

/// The work is at its last step: the reason a forced act is refused while it finishes.
fn past_commit(job: &JobRow) -> Hold {
    match hold_of(job) {
        Hold {
            work: Some(work),
            step: Some(step),
            ..
        } => Hold {
            why: HoldWhy::PastCommit,
            work: Some(work),
            step: Some(step),
        },
        _ => Hold::of(HoldWhy::PastCommit),
    }
}
