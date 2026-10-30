//! A durable same-unit environment restart that keeps every file and lets queued inputs continue.

use super::runner::{Done, Failure, Job, MakeError, Making, Phase, Phased, Stop, Undone, make};
use crate::{Engine, EngineError, commands::Outgoing, ids, unit::Unit};
use async_trait::async_trait;
use orch_access_contract::{
    audit::AuditAction,
    decisions::{Decision, ObjectRef, Refusal},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    environment::EnvironmentRestartMode,
    threads::{EnvironmentRunReason, MarkerEnvironmentRun, MarkerPayload},
};
use orch_authz::{Policy, Principal};
use orch_db::{
    Conn,
    queries::{
        environments::{hosting as envs, thread_engine as env_view},
        jobs,
        resources::hosting as hosts,
        threads, transaction_guard,
    },
};
use orch_link_contract::commands::{EnvDetach, NodeRuntimeStop, SerialKey};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// One ordinary observation pass is quick against the 120-second shipped grace and bounded stop budget.
const LOOK_EVERY: Duration = Duration::from_millis(100);
const PHASES: [Phase; 7] = [
    Phase {
        name: "waiting",
        weight: 5,
    },
    Phase {
        name: "stopping",
        weight: 20,
    },
    Phase {
        name: "starting",
        weight: 30,
    },
    Phase {
        name: "enroll",
        weight: 20,
    },
    Phase {
        name: "attach",
        weight: 20,
    },
    Phase {
        name: "after_up",
        weight: 10,
    },
    Phase {
        name: "commit",
        weight: 5,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Plan {
    environment: Id,
    runtime: Id,
    host: Id,
    caller: Id,
    token: Option<Id>,
    threads: Vec<Id>,
    generation: i32,
    restart_mode: EnvironmentRestartMode,
    grace_secs: i64,
    definition: Option<Json>,
    mode: String,
    origin: orch_db::audit::Origin,
}

/// Why the request queued no restart.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The environment or its current unit is gone.
    #[error("the environment has no current managed unit")]
    NotFound,
    /// A directory on a person's machine has no unit Orch may stop and start.
    #[error("only an Orch-managed environment unit can be restarted")]
    NotManaged,
    /// Another lifecycle action owns the unit.
    #[error("another job holds this environment")]
    Busy(Box<jobs::JobRow>),
    /// The current requester cannot manage it.
    #[error("the requester may not restart this environment")]
    Denied(Box<Refusal>),
    /// A local, state or restriction gate prevents a same-unit start.
    #[error("{0}")]
    Unavailable(String),
    /// A control-plane dependency failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}
impl From<orch_db::DbError> for Error {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}
impl From<orch_authz::Error> for Error {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

fn failed(message: impl Into<String>) -> Stop {
    Failure::new("environment_restart_failed", message).into()
}

impl Engine {
    /// Queues one restart in the caller's action transaction, without stopping or replacing a unit.
    pub async fn queue_restart(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        environment: Id,
        mode: EnvironmentRestartMode,
    ) -> Result<jobs::JobRow, Error> {
        transaction_guard::lock_action(conn).await?;
        let affected = jobs::changes::threads(conn, Some(environment), None, true).await?;
        let env = envs::lock_environment(conn, environment)
            .await?
            .filter(|env| env.deleted_at.is_none())
            .ok_or(Error::NotFound)?;
        let at = self.inner.clock.time();
        let principal = Principal::load(conn, principal.caller().clone(), at).await?;
        if let Decision::Refused(why) = orch_authz::resources::check(
            conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
            Permission::EnvironmentManage,
            Policy::default(),
        )
        .await?
        {
            return Err(Error::Denied(Box::new(why)));
        }
        if !orch_db::queries::environment_facts::has_unit(conn, environment).await? {
            return Err(Error::NotManaged);
        }
        if !matches!(
            env.status.as_str(),
            "running" | "stopped" | "offline" | "failed"
        ) {
            return Err(Error::Unavailable(
                "the environment cannot restart in its current state".into(),
            ));
        }
        if let Some(job) = jobs::changes::environment_hold(conn, environment).await? {
            return Err(Error::Busy(Box::new(job)));
        }
        let runtime = envs::live_runtime(conn, environment)
            .await?
            .ok_or(Error::NotFound)?;
        let host = hosts::host(conn, runtime.host_id)
            .await?
            .ok_or(Error::NotFound)?;
        if host.disabled {
            return Err(Error::Unavailable("the host is disabled".into()));
        }
        if let Some(blocked) = crate::network::blocked(conn, runtime.id).await? {
            return Err(Error::Unavailable(
                blocked.detail.unwrap_or_else(|| blocked.reason.into()),
            ));
        }
        let definition = jobs::changes::definitions(conn, &[runtime.id])
            .await?
            .into_iter()
            .find_map(|(_, definition)| definition);
        let plan = Plan {
            environment,
            runtime: runtime.id,
            host: host.id,
            caller: principal.user(),
            token: principal.caller().grant().and_then(|grant| grant.token()),
            threads: affected.clone(),
            generation: env.generation,
            restart_mode: mode,
            grace_secs: self.recreate_grace_secs(conn, environment).await?.max(0),
            definition,
            mode: "start".into(),
            origin: orch_db::audit::current_origin(),
        };
        let mut value = json!(plan);
        if plan.definition.is_none()
            && let Some(fields) = value.as_object_mut()
        {
            fields.remove("definition");
        }
        let row = make(
            conn,
            &Making {
                kind: jobs::kind::ENVIRONMENT_RESTART,
                environment,
                plan: value,
                phases: &PHASES,
                created_by: Some(plan.caller),
            },
            at.get(),
        )
        .await
        .map_err(|error| match error {
            MakeError::Running(row) => Error::Busy(row),
            MakeError::NoEnvironment => Error::NotFound,
            MakeError::Db(error) => Error::Internal(error.into()),
        })?;
        let mut unit = Unit::new(conn, at.get());
        for thread in affected {
            let Some(row) = threads::thread(&mut *unit.conn, thread).await? else {
                continue;
            };
            if threads::active_turn(&mut *unit.conn, thread)
                .await?
                .is_none()
                && matches!(row.state.as_str(), "idle" | "blocked")
            {
                let seq = unit.stamp(thread).await?;
                crate::notifications::set_state(&mut unit,thread,&["idle","blocked"],
                    &threads::StateChange::blocked("switching",Some("Its environment is restarting; queued inputs continue when it is ready.")),seq).await?;
            }
        }
        unit.finish().await?;
        Ok(row)
    }

    /// Releases the same holds after success, failed verification, cancellation or process recovery.
    pub(crate) async fn after_restart(&self, row: &jobs::JobRow) {
        let mut affected = serde_json::from_value::<Plan>(row.plan.clone())
            .map(|plan| plan.threads)
            .unwrap_or_default();
        if let Some(environment) = row.environment_id
            && let Ok(mut conn) = self.db().acquire().await
        {
            affected.extend(
                jobs::changes::threads(&mut conn, Some(environment), None, false)
                    .await
                    .unwrap_or_default(),
            );
        }
        affected.sort_unstable();
        affected.dedup();
        for thread in affected {
            if let Err(error) = self.release_lock(thread).await {
                tracing::warn!(%thread,%error,"a restart's queued work will be reconsidered at the next sweep");
            }
        }
    }
}

/// The immutable selected unit; no phase creates a runtime, image or volume.
pub struct Restarting {
    plan: Plan,
}
impl Restarting {
    /// Loads a retained request for the ordinary job runner.
    pub fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            plan: serde_json::from_value(row.plan.clone())?,
        })
    }

    async fn current(&self, job: &Job) -> Result<envs::RuntimeFull, Stop> {
        let mut tx = transaction_guard::scope(false, job.engine().db().begin()).await?;
        transaction_guard::lock_action(&mut tx).await?;
        let principal =
            super::source::load_principal(&mut tx, job, self.plan.caller, self.plan.token).await?;
        if !matches!(
            orch_authz::resources::check(
                &mut tx,
                &principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: self.plan.environment
                },
                Permission::EnvironmentManage,
                Policy::default()
            )
            .await
            .map_err(EngineError::from)?,
            Decision::Allowed { .. }
        ) {
            return Err(failed(
                "the restart requester's current environment permission was withdrawn",
            ));
        }
        let env = envs::environment(&mut tx, self.plan.environment)
            .await?
            .filter(|env| env.deleted_at.is_none() && env.status != "disabled")
            .ok_or_else(|| failed("the environment was removed or disabled"))?;
        let runtime = envs::live_runtime(&mut tx, self.plan.environment)
            .await?
            .filter(|runtime| runtime.id == self.plan.runtime && runtime.host_id == self.plan.host)
            .ok_or_else(|| failed("the environment's current unit changed before restart"))?;
        if env.generation != self.plan.generation {
            return Err(failed(
                "the environment's generation changed before restart",
            ));
        }
        let host = hosts::host(&mut tx, runtime.host_id)
            .await?
            .filter(|host| !host.disabled)
            .ok_or_else(|| failed("the host is gone or disabled"))?;
        let _ = host;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(runtime)
    }

    async fn waiting(&self, job: &Job) -> Result<(), Stop> {
        let plan = job.plan().await?;
        let since = plan
            .get("waiting_since")
            .cloned()
            .and_then(|value| serde_json::from_value::<chrono::DateTime<chrono::Utc>>(value).ok())
            .unwrap_or(job.now());
        if plan.get("waiting_since").is_none() {
            job.remember(&json!({"waiting_since":since})).await?;
        }
        job.progress(0.0, Some("waiting for running turns before restart"))
            .await?;
        loop {
            job.check()?;
            self.current(job).await?;
            let mut conn = job.engine().db().acquire().await?;
            let affected =
                jobs::changes::threads(&mut conn, Some(self.plan.environment), None, false).await?;
            let mut active = Vec::new();
            for thread in affected {
                if let Some(turn) = threads::active_turn(&mut conn, thread).await? {
                    active.push((thread, turn));
                }
            }
            drop(conn);
            if active.is_empty() {
                return Ok(());
            }
            if self.plan.restart_mode == EnvironmentRestartMode::Interrupt
                || job.now().signed_duration_since(since).num_seconds() >= self.plan.grace_secs
            {
                for (thread, turn) in active {
                    job.engine().cancel_pending_approvals(thread).await?;
                    job.engine()
                        .interrupt_for_change(thread, &turn, self.plan.caller)
                        .await?;
                }
            }
            job.pause(LOOK_EVERY).await?;
        }
    }

    async fn stopping(&self, job: &Job) -> Result<(), Stop> {
        let runtime = self.current(job).await?;
        let plan = job.plan().await?;
        if plan.get("restart_stopped").and_then(Json::as_bool) == Some(true) {
            return Ok(());
        }
        job.engine()
            .return_credentials_of(self.plan.environment)
            .await
            .map_err(|error| {
                failed(format!(
                    "credentials could not be returned before restart: {error}"
                ))
            })?;
        if runtime.state != "stopped" {
            job.remember(&json!({"restart_stop_requested":true}))
                .await?;
            // Detach tombstones desired child services before their connector disappears; recovery
            // must not reopen the old shells, browser/display handles or outbound port connections.
            if let Some(machine) = runtime.machine_id {
                job.command(
                    Outgoing::new(
                        ids::derived("restart.env.detach", &[job.id().as_bytes()]),
                        machine,
                        SerialKey::Environment(self.plan.environment),
                        &EnvDetach {
                            environment_id: self.plan.environment,
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            }
            let mut tx = job.engine().db().begin().await?;
            envs::set_runtime_state(
                &mut tx,
                runtime.id,
                &envs::runtime_state::LIVE,
                envs::runtime_state::STOPPING,
                None,
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            job.command(
                Outgoing::new(
                    ids::derived("restart.unit.stop", &[job.id().as_bytes()]),
                    {
                        let mut conn = job.engine().db().acquire().await?;
                        hosts::host(&mut conn, self.plan.host)
                            .await?
                            .ok_or_else(|| failed("the restart host disappeared"))?
                            .machine_id
                    },
                    SerialKey::Runtime(runtime.id),
                    &NodeRuntimeStop {
                        runtime_id: runtime.id,
                        reason: Some(orch_driver_contract::runtime::UnitReason::Stop),
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        }
        let mut tx = job.engine().db().begin().await?;
        envs::set_runtime_state(
            &mut tx,
            runtime.id,
            &envs::runtime_state::LIVE,
            envs::runtime_state::STOPPED,
            None,
            job.now(),
        )
        .await?;
        envs::set_status(
            &mut tx,
            self.plan.environment,
            &["running", "offline", "stopped", "failed"],
            "provisioning",
            None,
            job.now(),
        )
        .await?;
        envs::close_restart_sessions(&mut tx, self.plan.environment, job.now()).await?;
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"restart_stopped":true}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    async fn commit(&self, job: &Job) -> Result<(), Stop> {
        if job
            .plan()
            .await?
            .get("restart_committed")
            .and_then(Json::as_bool)
            == Some(true)
        {
            return Ok(());
        }
        self.current(job).await?;
        // The normal native health gate verifies the same connector before ready is published.
        super::provision::Provision::of(job.engine().clone(), &job.row().await?)
            .run(job, "verify")
            .await?;
        job.pass_commit().await?;
        let mut tx = transaction_guard::scope(true, job.engine().db().begin()).await?;
        let env = envs::lock_environment(&mut tx, self.plan.environment)
            .await?
            .ok_or_else(|| failed("the environment disappeared before restart commit"))?;
        let runtime = envs::live_runtime(&mut tx, self.plan.environment)
            .await?
            .filter(|runtime| runtime.id == self.plan.runtime)
            .ok_or_else(|| failed("the environment's current unit changed"))?;
        let (machine, _) = hosts::machine_of_runtime(&mut tx, runtime.id)
            .await?
            .ok_or_else(|| failed("the restarted unit's connector is not known"))?;
        if !envs::make_running(
            &mut tx,
            env.id,
            &["provisioning"],
            &envs::Up {
                machine,
                workspace_root: None,
                facts: None,
                new_unit: false,
            },
            job.now(),
        )
        .await?
        {
            return Err(failed(
                "the environment changed before restart could finish",
            ));
        }
        envs::set_runtime_state(
            &mut tx,
            runtime.id,
            &envs::runtime_state::LIVE,
            envs::runtime_state::RUNNING,
            None,
            job.now(),
        )
        .await?;
        let visible = env_view::environment(&mut tx, env.id)
            .await?
            .ok_or_else(|| failed("the restarted environment disappeared"))?;
        let reference = serde_json::from_value(
            crate::views::environment_ref_json(&mut tx, &visible, Time::new(job.now())).await?,
        )
        .map_err(EngineError::from)?;
        let by =
            serde_json::from_value(crate::views::user_ref_json(&mut tx, self.plan.caller).await?)
                .map_err(EngineError::from)?;
        let payload = MarkerPayload::EnvironmentRestarted(MarkerEnvironmentRun {
            environment: reference,
            reason: EnvironmentRunReason::Person,
            by: Some(by),
        });
        let affected = jobs::changes::threads(&mut tx, Some(env.id), None, true).await?;
        let mut unit = Unit::new(&mut tx, job.now());
        for thread in affected {
            super::recreate::renew_segment(
                &mut unit,
                thread,
                Some(runtime.generation),
                "environment_restarted",
                self.plan.caller,
                false,
            )
            .await?;
            if let Some(segment) = threads::thread(&mut *unit.conn, thread)
                .await?
                .and_then(|thread| thread.current_segment_id)
            {
                crate::ingest::add_marker(
                    &mut unit,
                    thread,
                    segment,
                    None,
                    &serde_json::to_value(&payload).map_err(EngineError::from)?,
                    &format!("environment-restarted:{}:{thread}", job.id()),
                )
                .await?;
            }
        }
        unit.finish().await?;
        let summary = format!("{} was restarted with every file kept", env.name);
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(self.plan.caller),
            orch_db::audit::action!(AuditAction::EnvironmentRestarted).as_str(),
            "environment",
            Some(env.id),
            &summary,
        );
        entry.history = true;
        entry.node_id = Some(env.home_node_id);
        entry.org_id = Some(env.org_id);
        entry.environment_id = Some(env.id);
        entry.details = json!({"job":job.id(),"runtime":runtime.id,"generation":runtime.generation,"mode":self.plan.restart_mode,"files_kept":true});
        orch_db::audit::append(&mut tx, &entry, &self.plan.origin, job.now()).await?;
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"restart_committed":true}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}

#[async_trait]
impl Phased for Restarting {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "waiting" => self.waiting(job).await?,
            "stopping" => self.stopping(job).await?,
            "starting" | "enroll" | "attach" | "after_up" => {
                self.current(job).await?;
                if phase == "starting" {
                    job.remember(&json!({"restart_start_requested":true}))
                        .await?;
                }
                super::provision::Provision::of(job.engine().clone(), &job.row().await?)
                    .run(job, if phase == "starting" { "start" } else { phase })
                    .await?;
            }
            "commit" => self.commit(job).await?,
            _ => return Err(failed("the restart has an unknown phase")),
        }
        Ok(Done::Finished)
    }

    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        if super::after_up::record_interrupted(job).await.is_err() {
            tracing::warn!("an interrupted restart command outcome could not be recorded");
        }
        let Ok(plan) = job.plan().await else {
            return Undone::Partly(
                "the restart state could not be read; every file remains on its same unit".into(),
            );
        };
        if plan.get("restart_stop_requested").and_then(Json::as_bool) != Some(true)
            && plan.get("restart_start_requested").and_then(Json::as_bool) != Some(true)
        {
            return Undone::Everything;
        }
        let detail = match why {
            Stop::Failed(failure) => format!(
                "Restart failed: {}. Its same unit and files were kept.",
                failure.message
            ),
            _ => "Restart did not finish; its same unit and files were kept.".to_owned(),
        };
        // An interrupted restart leaves that exact unit stopped. It never recreates, removes or
        // resumes it behind a later disable, ownership change or replacement.
        let stop = async {
            let mut conn = job.engine().db().acquire().await?;
            let current = envs::live_runtime(&mut conn, self.plan.environment).await?;
            if current.is_none_or(|runtime| runtime.id != self.plan.runtime) {
                return Ok::<bool, EngineError>(false);
            }
            let host = hosts::host(&mut conn, self.plan.host)
                .await?
                .ok_or_else(|| EngineError::unreadable("the restart host is unavailable"))?;
            drop(conn);
            job.command_to_undo(Outgoing::new(
                ids::derived("restart.undo.stop", &[job.id().as_bytes()]),
                host.machine_id,
                SerialKey::Runtime(self.plan.runtime),
                &NodeRuntimeStop {
                    runtime_id: self.plan.runtime,
                    reason: Some(orch_driver_contract::runtime::UnitReason::Stop),
                },
            )?)
            .await
            .map_err(|error| EngineError::unreadable(format!("{error:?}")))?;
            let mut tx = job.engine().db().begin().await?;
            envs::set_runtime_state(
                &mut tx,
                self.plan.runtime,
                &envs::runtime_state::LIVE,
                "stopped",
                None,
                job.now(),
            )
            .await?;
            envs::set_status(
                &mut tx,
                self.plan.environment,
                &["running", "provisioning", "stopped", "offline", "failed"],
                "stopped",
                Some(&detail),
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok(true)
        }
        .await;
        match stop {
            Ok(true) => Undone::Partly(
                "the same unit is stopped with every file kept; interrupted work was not revived"
                    .into(),
            ),
            Ok(false) => Undone::Partly(
                "a later unit owns the environment; the earlier restart changed no successor"
                    .into(),
            ),
            Err(_) => {
                if let Ok(mut tx) = job.engine().db().begin().await
                    && envs::live_runtime(&mut tx, self.plan.environment)
                        .await
                        .ok()
                        .flatten()
                        .is_some_and(|runtime| runtime.id == self.plan.runtime)
                {
                    let _=envs::set_status(&mut tx,self.plan.environment,&["running","provisioning","stopped","offline","failed"],"failed",Some("Restart failed and the unit's stop could not be confirmed; its files were kept."),job.now()).await;
                    let _ = tx.commit().await;
                }
                Undone::Partly(
                    "the unit's stop could not be confirmed; no file, image or volume was removed"
                        .into(),
                )
            }
        }
    }
}
