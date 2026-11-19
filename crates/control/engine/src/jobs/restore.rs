//! Recorded-file restore jobs retain writer holds until native apply or rollback is complete.

use async_trait::async_trait;
use orch_api_contract::forks::ActiveTurn;
use orch_db::queries::{jobs, source, storage, threads};
use orch_link_contract::commands::{CommandBody, ProcList, ProcResume, ProcSuspend, SerialKey};
use orch_link_contract::source::{
    FsRecord, FsRecordResult, FsRestoreResult, RecordKind, RecordState,
};
use orch_types::Json;
use serde_json::json;
use std::time::Duration;

use super::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone};
use super::source::{Action, Plan};
use crate::{
    Engine, EngineError,
    commands::{Outcome, Outgoing},
    unit::Unit,
};

/// Explicit writer waiting precedes the only filesystem effect; the last phase releases writers.
pub const PHASES: [Phase; 4] = [
    Phase {
        name: "validate",
        weight: 5,
    },
    Phase {
        name: "lock",
        weight: 5,
    },
    Phase {
        name: "apply",
        weight: 85,
    },
    Phase {
        name: "commit",
        weight: 5,
    },
];
/// The common change runner looks for completion at a quarter-second cadence.
const LOOK_EVERY: Duration = Duration::from_millis(250);

pub struct Restoring {
    engine: Engine,
    plan: Plan,
}

impl Restoring {
    pub fn of(engine: Engine, row: &jobs::JobRow) -> Result<Self, EngineError> {
        let plan: Plan = serde_json::from_value(row.plan.clone())?;
        if !matches!(plan.action, Action::Restore { .. }) {
            return Err(EngineError::unreadable(
                "a restore job has no restore recipe",
            ));
        }
        Ok(Self { engine, plan })
    }

    /// A negative command answer gets a new envelope while keeping the native job/effect identity.
    /// A reconnect or a lost answer keeps the same in-flight envelope.
    async fn command<B: CommandBody>(
        &self,
        job: &Job,
        label: &str,
        body: &B,
        undo: bool,
    ) -> Result<Json, Stop> {
        let mut attempt = job
            .plan()
            .await?
            .get("file_attempts")
            .and_then(|values| values.get(label))
            .and_then(Json::as_u64)
            .unwrap_or(0);
        loop {
            let id = crate::ids::derived(label, &[job.id().as_bytes(), &attempt.to_le_bytes()]);
            let known = self
                .engine
                .inner
                .commands
                .status(id)
                .await
                .map_err(EngineError::from)?;
            if known.is_some_and(|info| {
                matches!(
                    info.outcome,
                    Some(Outcome::Failed(_) | Outcome::Expired | Outcome::Cancelled)
                )
            }) {
                attempt = attempt
                    .checked_add(1)
                    .ok_or_else(|| Failure::new("internal", "restore retry identity overflowed"))?;
                let mut attempts = job
                    .plan()
                    .await?
                    .get("file_attempts")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                attempts[label] = json!(attempt);
                job.remember(&json!({ "file_attempts": attempts })).await?;
                continue;
            }
            let command = Outgoing::new(
                id,
                self.plan.machine,
                SerialKey::Environment(self.plan.environment),
                body,
            )
            .map_err(EngineError::from)?;
            return if undo {
                job.command_to_undo(command).await
            } else {
                job.command(command).await
            };
        }
    }

    async fn lock(&self, job: &Job) -> Result<Done, Stop> {
        let Action::Restore {
            writers,
            options,
            suspend,
            ..
        } = &self.plan.action
        else {
            unreachable!()
        };
        let began = job.row().await?.started_at.unwrap_or(job.now());
        for thread in writers {
            loop {
                job.check()?;
                let active = {
                    let mut conn = self.engine.db().acquire().await?;
                    threads::active_turn(&mut conn, *thread).await?
                };
                let Some(turn) = active else { break };
                if options.active_turn == ActiveTurn::Interrupt {
                    // The reviewed interruption permission is asked again for the current caller.
                    let mut conn = self.engine.db().acquire().await?;
                    let principal = super::source::principal(&mut conn, job, &self.plan).await?;
                    let env = orch_authz::resources::check(
                        &mut conn,
                        &principal,
                        orch_access_contract::decisions::ObjectRef {
                            kind: orch_access_contract::permissions::ObjectKind::Environment,
                            id: self.plan.environment,
                        },
                        orch_access_contract::permissions::Permission::EnvironmentManage,
                        orch_authz::Policy::default(),
                    )
                    .await
                    .map_err(EngineError::from)?;
                    if !matches!(
                        env,
                        orch_access_contract::decisions::Decision::Allowed { .. }
                    ) {
                        let allowed = orch_authz::nodes::check(
                            &mut conn,
                            &principal,
                            *thread,
                            orch_access_contract::permissions::Permission::ThreadInteract,
                            orch_authz::Act::Use,
                            orch_authz::Policy::default(),
                        )
                        .await
                        .map_err(EngineError::from)?;
                        if !matches!(
                            allowed,
                            orch_access_contract::decisions::Decision::Allowed { .. }
                        ) {
                            return Err(Failure::new(
                                "access.denied",
                                "current authority does not permit stopping a writer",
                            )
                            .into());
                        }
                    }
                    drop(conn);
                    self.engine
                        .interrupt_for_change(*thread, &turn, self.plan.caller)
                        .await?;
                }
                if job.now() - began >= chrono::Duration::minutes(options.wait_minutes) {
                    return Err(Failure::new(
                        "wait_timeout",
                        "writers did not finish within the reviewed wait; no restore was applied",
                    )
                    .into());
                }
                job.pause(LOOK_EVERY).await?;
            }
        }
        if *suspend {
            let mut conn = self.engine.db().acquire().await?;
            let principal = super::source::principal(&mut conn, job, &self.plan).await?;
            let allowed = orch_authz::resources::check(
                &mut conn,
                &principal,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Environment,
                    id: self.plan.environment,
                },
                orch_access_contract::permissions::Permission::TerminalManage,
                orch_authz::Policy::default(),
            )
            .await
            .map_err(EngineError::from)?;
            if !matches!(
                allowed,
                orch_access_contract::decisions::Decision::Allowed { .. }
            ) {
                return Err(Failure::new(
                    "access.denied",
                    "current authority does not permit suspending writers",
                )
                .into());
            }
            drop(conn);
            job.remember(&json!({ "suspension_requested": true }))
                .await?;
            self.command(
                job,
                "restore.suspend",
                &ProcSuspend {
                    environment_id: self.plan.environment,
                },
                false,
            )
            .await?;
        } else {
            loop {
                let value = self
                    .command(
                        job,
                        "restore.processes",
                        &ProcList {
                            environment_id: self.plan.environment,
                        },
                        false,
                    )
                    .await?;
                let observed: orch_link_contract::commands::ProcListResult =
                    serde_json::from_value(value).map_err(EngineError::from)?;
                if observed
                    .processes
                    .iter()
                    .all(|process| matches!(process.state.as_str(), "T" | "t" | "Z" | "X"))
                {
                    break;
                }
                if job.now() - began >= chrono::Duration::minutes(options.wait_minutes) {
                    return Err(Failure::new(
                        "wait_timeout",
                        "tracked writers still run; no restore was applied",
                    )
                    .into());
                }
                // A process listing is an observation. A new polling envelope must read again.
                let mut attempts = job
                    .plan()
                    .await?
                    .get("file_attempts")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let next = attempts["restore.processes"]
                    .as_u64()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| {
                        Failure::new("internal", "process observation identity overflowed")
                    })?;
                attempts["restore.processes"] = json!(next);
                job.remember(&json!({ "file_attempts": attempts })).await?;
                job.pause(LOOK_EVERY).await?;
            }
        }
        Ok(Done::Finished)
    }

    async fn apply(&self, job: &Job) -> Result<Done, Stop> {
        let Action::Restore { body, .. } = &self.plan.action else {
            unreachable!()
        };
        let recovering = job
            .plan()
            .await?
            .get("restore_recovery_required")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        if !recovering {
            super::source::check(job, &self.plan).await?;
        }
        job.pass_commit().await?;
        job.remember(&json!({ "restore_recovery_required": true }))
            .await?;
        let value = match self.command(job, "restore.apply", body, false).await {
            Ok(value) => value,
            Err(Stop::Failed(failure))
                if failure.code != "restore.recovery_required" && !recovering =>
            {
                // A completed native negative answer proves no partially applied journal. Link
                // loss is not a completed answer and keeps the conservative recovery hold.
                if !matches!(failure.code.as_str(), "expired" | "cancelled" | "internal") {
                    job.remember(&json!({ "restore_recovery_required": false }))
                        .await?;
                }
                return Err(Stop::Failed(failure));
            }
            Err(stop) => return Err(stop),
        };
        let result: FsRestoreResult =
            serde_json::from_value(value.clone()).map_err(EngineError::from)?;
        if result.undo_snapshot_id != body.undo_snapshot_id {
            return Err(
                Failure::new("internal", "the native restore returned another Undo state").into(),
            );
        }
        job.remember(&json!({ "restore_result": value, "restore_recovery_required": false }))
            .await?;
        Ok(Done::Finished)
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        let Action::Restore {
            body,
            directory,
            record_days,
            max_bytes,
            ..
        } = &self.plan.action
        else {
            unreachable!()
        };
        let result: FsRestoreResult = serde_json::from_value(
            job.plan()
                .await?
                .get("restore_result")
                .cloned()
                .ok_or_else(|| {
                    Failure::new("internal", "the completed restore result is missing")
                })?,
        )
        .map_err(EngineError::from)?;
        let value = self
            .command(
                job,
                "restore.undo.metadata",
                &FsRecord {
                    environment_id: self.plan.environment,
                    snapshot_id: body.undo_snapshot_id,
                    directory: directory.clone(),
                    kind: RecordKind::Restore,
                    record_days: *record_days,
                    max_bytes: *max_bytes,
                },
                false,
            )
            .await?;
        let undo: FsRecordResult = serde_json::from_value(value).map_err(EngineError::from)?;
        if undo.state != RecordState::Complete {
            return Err(
                Failure::new("internal", "the complete Undo metadata is unavailable").into(),
            );
        }
        let hash = undo
            .manifest_blob
            .ok_or_else(|| Failure::new("internal", "the Undo manifest is unavailable"))?;
        let manifest = self.engine.read_manifest_checked(hash).await?;
        if manifest.header().generation != i64::from(self.plan.generation)
            || manifest.entries().iter().any(|entry| {
                entry.kind == crate::manifest::EntryKind::Other
                    || entry.kind == crate::manifest::EntryKind::File && entry.hash.is_none()
                    || entry.kind == crate::manifest::EntryKind::Link && entry.link.is_none()
            })
        {
            return Err(Failure::new("internal", "the Undo manifest is incomplete").into());
        }
        let size = self
            .engine
            .blobs()
            .ok_or_else(|| Failure::new("internal", "the content store is unavailable"))?
            .size(&hash)
            .await
            .map_err(EngineError::unreadable)?
            .ok_or_else(|| Failure::new("internal", "the Undo manifest was lost"))?;
        let mut tx = self.engine.db().begin().await?;
        storage::register_blob(&mut tx, &hash, size, job.now()).await?;
        if source::finish_recorded(&mut tx, body.undo_snapshot_id, &undo, job.now()).await? {
            storage::pin_blobs(&mut tx, &[hash]).await?;
        }
        let saved = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| Failure::new("not_found", "the restore job disappeared"))?;
        if saved.plan.get("restore_published").and_then(Json::as_bool) != Some(true) {
            let mut unit = Unit::new(&mut tx, job.now());
            if let Some(thread) = self.plan.thread
                && let Some(row) = threads::lock(&mut *unit.conn, thread).await?
            {
                let seq = unit.stamp(thread).await?;
                source::stale_work(&mut *unit.conn, thread, job.now(), seq).await?;
                if !result.undone
                    && let Some(segment) = row.current_segment_id
                {
                    let by = crate::views::user_ref_json(&mut *unit.conn, self.plan.caller).await?;
                    crate::ingest::add_marker(&mut unit, thread, segment, None, &json!({ "marker": "files_restored",
                                "data": { "job_id": job.id(), "snapshot_id": body.snapshot_id, "undo_snapshot_id": body.undo_snapshot_id, "files": result.files, "by": by } }),
                                &format!("files-restored:{}", job.id())).await?;
                }
            }
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(self.plan.caller),
                orch_db::audit::name!("workspace.state_restored"),
                "environment",
                Some(self.plan.environment),
                if result.undone {
                    "Returned the files after an interrupted restore"
                } else {
                    "Restored the recorded files"
                },
            );
            entry.environment_id = Some(self.plan.environment);
            entry.node_id = self.plan.thread;
            entry.history = true;
            entry.details = json!({ "job_id": job.id(), "snapshot_id": body.snapshot_id, "undo_snapshot_id": body.undo_snapshot_id, "files": result.files, "undone": result.undone });
            orch_db::audit::append(
                &mut *unit.conn,
                &entry,
                &orch_db::audit::Origin::default(),
                job.now(),
            )
            .await?;
            jobs::merge_plan(
                &mut *unit.conn,
                job.id(),
                &json!({ "restore_published": true }),
                job.now(),
            )
            .await?;
            let finished = unit.finish().await?;
            self.engine.commit(tx, finished).await?;
        } else {
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        self.resume(job).await?;
        Ok(Done::Finished)
    }

    async fn resume(&self, job: &Job) -> Result<(), Stop> {
        if job
            .plan()
            .await?
            .get("suspension_requested")
            .and_then(Json::as_bool)
            == Some(true)
        {
            if let Err(stop) = self
                .command(
                    job,
                    "restore.resume",
                    &ProcResume {
                        environment_id: self.plan.environment,
                    },
                    true,
                )
                .await
            {
                if stop != Stop::Lost {
                    job.remember(&json!({ "restore_recovery_required": true }))
                        .await?;
                }
                return Err(stop);
            }
            job.remember(&json!({ "suspension_requested": false }))
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl Phased for Restoring {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "validate" => {
                super::source::check(job, &self.plan).await?;
                Ok(Done::Finished)
            }
            "lock" => self.lock(job).await,
            "apply" => self.apply(job).await,
            "commit" => self.commit(job).await,
            _ => Err(Failure::new("internal", "the restore job has an unknown phase").into()),
        }
    }
    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        if job.plan().await.ok().and_then(|plan| {
            plan.get("restore_recovery_required")
                .and_then(Json::as_bool)
        }) == Some(true)
        {
            return Undone::Partly(
                "the restore journal still owns files; the environment stays held for recovery"
                    .to_owned(),
            );
        }
        match self.resume(job).await {
            Ok(()) => Undone::Everything,
            Err(_) => Undone::Partly("tracked writers could not be resumed".to_owned()),
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan().await.ok()?.get("restore_result").cloned()
    }
}

impl Engine {
    pub(crate) async fn after_file_restore(&self, row: &jobs::JobRow) {
        if row
            .plan
            .get("restore_recovery_required")
            .and_then(Json::as_bool)
            == Some(true)
        {
            return;
        }
        if let Ok(plan) = serde_json::from_value::<Plan>(row.plan.clone())
            && let Action::Restore { writers, body, .. } = plan.action
        {
            if row
                .plan
                .get("restore_plan_released")
                .and_then(Json::as_bool)
                != Some(true)
            {
                let mut tx = match self.db().begin().await {
                    Ok(tx) => tx,
                    Err(_) => return,
                };
                if let Ok(Some(current)) = jobs::lock(&mut tx, row.id).await
                    && current
                        .plan
                        .get("restore_plan_released")
                        .and_then(Json::as_bool)
                        != Some(true)
                {
                    if storage::unpin_blobs(&mut tx, &[body.plan_blob], self.inner.clock.now())
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if jobs::merge_plan(
                        &mut tx,
                        row.id,
                        &json!({ "restore_plan_released": true }),
                        self.inner.clock.now(),
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                    if tx.commit().await.is_err() {
                        return;
                    }
                }
            }
            let mut releasing = writers;
            if let Ok(mut conn) = self.db().acquire().await
                && let Ok(current) = sqlx::query_scalar::<_, orch_types::Id>(
                    "SELECT node_id FROM threads WHERE environment_id = $1 ORDER BY node_id",
                )
                .bind(plan.environment)
                .fetch_all(&mut *conn)
                .await
            {
                releasing.extend(current);
            }
            releasing.sort();
            releasing.dedup();
            for thread in releasing {
                if let Err(error) = self.release_lock(thread).await {
                    tracing::warn!(%error, %thread, "a completed restore's writer could not be released");
                }
            }
        }
    }
}
