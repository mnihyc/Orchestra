//! An accepted switch-off interrupts all attributed work immediately and durably finishes stopping its units.

use crate::jobs::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone, initial_progress};
use crate::{
    Engine, EngineError,
    commands::Outgoing,
    ids, ingest,
    resource_facts::activity,
    unit::{Finished, Unit},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{
    resource_facts::FactTarget,
    threads::{MarkerPayload, MarkerResourceResumed, MarkerResourceStopped},
};
use orch_db::{
    Conn, DbError,
    queries::{jobs, threads},
};
use orch_link_contract::commands::{AgentStop, NodeRuntimeStop, ProviderStop, SerialKey};
use orch_types::{Id, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// An admitted emergency stop is never cancelled after the resource becomes unavailable.
pub const KIND: &str = "resource_quiesce";
/// The turn interrupt must settle before units keeping its sign-in and files are stopped.
pub const PHASES: [Phase; 2] = [
    Phase {
        name: "stop",
        weight: 80,
    },
    Phase {
        name: "units",
        weight: 20,
    },
];
/// Reuses the hosting lifecycle's short observation cadence; its existing interrupt deadline still decides force.
const OBSERVE_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Recipe {
    target: FactTarget,
    turns: Vec<(Id, Id)>,
    requests: Vec<Id>,
    deleted: bool,
    #[serde(default)]
    harnesses: Vec<(Id, Id, Id)>,
    #[serde(default)]
    keepers: Vec<Id>,
    #[serde(default)]
    units: Vec<(Id, Id)>,
    #[serde(default)]
    logout: bool,
    #[serde(default = "pending_native")]
    pending_native: bool,
}

fn pending_native() -> bool {
    true
}

/// The reviewed resource, actor and history attribution of one immediate stop.
pub struct ResourceStop<'a> {
    /// The resource and optional model being switched off or deleted.
    pub target: FactTarget,
    /// The authorized display name captured for the history line.
    pub name: &'a str,
    /// The person accepting the stop.
    pub by: Id,
    /// The optional one-line reason shown to waiting people.
    pub note: Option<&'a str>,
    /// Whether native credentials must also be removed.
    pub deleted: bool,
}

/// The accepted availability and original-work choice of one resource resumption.
pub struct ResourceResume<'a> {
    /// The resource and optional model that is now available.
    pub target: &'a FactTarget,
    /// Its authorized captured name.
    pub name: &'a str,
    /// The person accepting the resource operation.
    pub by: Id,
    /// Whether eligible pending messages and exact resource-stopped work may resume.
    pub run_waiting: bool,
}

impl Engine {
    /// Enable either holds eligible inputs or wakes their threads after commit, preserving authored Stop holds.
    pub async fn enable_resource_waiting_in(
        &self,
        conn: &mut Conn,
        resume: ResourceResume<'_>,
        now: DateTime<Utc>,
    ) -> Result<Finished, EngineError> {
        let ResourceResume {
            target,
            name,
            by,
            run_waiting,
        } = resume;
        if !run_waiting {
            return self.hold_resource_waiting_in(conn, target, by, now).await;
        }
        let work = activity::read(conn, target).await?;
        let mut done = Finished::default();
        for row in work.iter().filter(|row| row.kind == "waiting") {
            done.wakes.insert(row.thread);
        }
        let operation = new_id();
        let actor: orch_api_contract::users::UserRef =
            serde_json::from_value(crate::views::user_ref_json(conn, by).await?)?;
        let model: Option<String> = match target.model_id {
            Some(id) => sqlx::query_scalar(
                "SELECT model_name FROM provider_models WHERE id=$1 AND provider_id=$2",
            )
            .bind(id)
            .bind(target.object.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?,
            None => None,
        };
        for row in work.iter().filter(|row| row.kind == "stopped") {
            let Some(thread) = threads::lock(conn, row.thread).await? else {
                continue;
            };
            if thread.current_turn_id.is_some() || thread.queue_held {
                continue;
            }
            let bound = match target.object.kind {
                ObjectKind::Provider => thread.provider_id,
                ObjectKind::Agent => thread.agent_id,
                ObjectKind::Environment => thread.environment_id,
                _ => None,
            };
            if bound != Some(target.object.id) {
                continue;
            }
            let Some(turn) = threads::latest_active_turn(conn, row.thread)
                .await?
                .filter(|turn| turn.id == row.id && turn.state == "interrupted")
            else {
                continue;
            };
            let resumed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items WHERE turn_id=$1 AND kind='marker' AND payload->>'marker'='resource_resumed' AND payload#>>'{data,input_id}' IS NOT NULL)")
                .bind(turn.id).fetch_one(&mut *conn).await.map_err(DbError::from)?;
            if resumed {
                continue;
            }
            let origin = threads::input(conn, turn.input_id).await?;
            let reference = ids::derived(
                "resource.continue",
                &[operation.as_bytes(), turn.id.as_bytes()],
            );
            let mut unit = Unit::new(conn, now);
            let (input_id, reason) = match origin {
                // An input whose turn never began was returned to the queue.
                // Enabling with Run waiting admits that original input rather
                // than inventing a continuation for work that did not start.
                Some(origin) if turn.started_at.is_none() && origin.state == "pending" => {
                    (Some(origin.id), None)
                }
                Some(origin) => match self
                    .continue_original_input_in(&mut unit, &thread, &origin, reference, None)
                    .await
                {
                    Ok(input) => (input, None),
                    Err(EngineError::Unreadable(_)) => {
                        (None, Some("original_author_unavailable".to_owned()))
                    }
                    Err(error) => return Err(error),
                },
                None => (None, Some("original_input_unavailable".to_owned())),
            };
            let marker =
                serde_json::to_value(MarkerPayload::ResourceResumed(MarkerResourceResumed {
                    object: target.object,
                    name: name.into(),
                    model: model.clone(),
                    by: actor.clone(),
                    turn_id: turn.id,
                    reference,
                    input_id,
                    reason,
                }))?;
            ingest::add_marker(
                &mut unit,
                row.thread,
                turn.segment_id,
                Some(turn.id),
                &marker,
                &format!("resource-resumed:{reference}"),
            )
            .await?;
            let finished = unit.finish().await?;
            done.wakes.extend(finished.wakes);
            done.touched.extend(finished.touched);
        }
        Ok(done)
    }

    /// Enable's off-default choice uses the existing held queue and its visible Resume.
    pub async fn hold_resource_waiting_in(
        &self,
        conn: &mut Conn,
        target: &FactTarget,
        by: Id,
        now: DateTime<Utc>,
    ) -> Result<Finished, EngineError> {
        let mut work = activity::read(conn, target).await?;
        work.retain(|row| row.kind == "waiting");
        work.sort_by_key(|row| row.thread);
        work.dedup_by_key(|row| row.thread);
        let mut unit = Unit::new(conn, now);
        for row in work.iter().filter(|row| row.kind == "waiting") {
            if let Some(thread) = threads::lock(unit.conn, row.thread).await? {
                self.hold_queue(&mut unit, &thread, by).await?;
            }
        }
        unit.finish().await
    }
    /// Persists interrupts and their history in the same guarded transaction that switches a resource off.
    pub async fn stop_resource_in(
        &self,
        conn: &mut Conn,
        stop: ResourceStop<'_>,
        now: DateTime<Utc>,
    ) -> Result<(jobs::JobRow, Finished), EngineError> {
        let ResourceStop {
            target,
            name,
            by,
            note,
            deleted,
        } = stop;
        let mut work = activity::read(conn, &target).await?;
        work.sort_by_key(|row| (row.thread, row.id));
        let actor = serde_json::from_value(crate::views::user_ref_json(conn, by).await?)?;
        let model = match target.model_id {
            Some(id) => sqlx::query_scalar(
                "SELECT model_name FROM provider_models WHERE id=$1 AND provider_id=$2",
            )
            .bind(id)
            .bind(target.object.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?,
            None => None,
        };
        let marker = serde_json::to_value(MarkerPayload::ResourceStopped(MarkerResourceStopped {
            object: target.object,
            name: name.into(),
            model,
            by: actor,
            note: note.map(str::to_owned),
            deleted,
        }))?;
        let mut unit = Unit::new(conn, now);
        let mut turns = Vec::new();
        for row in work.iter().filter(|row| row.kind == "running") {
            let Some(thread) = threads::lock(unit.conn, row.thread).await? else {
                continue;
            };
            let Some(turn) = threads::turn(unit.conn, row.id)
                .await?
                .filter(|turn| turn.is_active())
            else {
                continue;
            };
            let seq = unit.stamp(row.thread).await?;
            let caused = threads::mark_interrupted_by(unit.conn, turn.id, by, seq).await?;
            self.withdraw_interrupted_harness(&mut unit, row.thread, turn.id)
                .await?;
            if caused {
                threads::set_interrupt_cause(unit.conn, turn.id, "resource_disabled").await?;
            }
            ingest::add_marker(
                &mut unit,
                row.thread,
                turn.segment_id,
                Some(turn.id),
                &marker,
                &format!(
                    "resource-stopped:{}:{:?}:{}",
                    target.object.id, target.model_id, turn.id
                ),
            )
            .await?;
            unit.wake(thread.node_id);
            turns.push((row.thread, turn.id));
        }
        let requests = activity::requests(unit.conn, &target).await?;
        let label = if target.model_id.is_some() {
            "Model"
        } else {
            match target.object.kind {
                ObjectKind::Provider => "Provider",
                ObjectKind::Agent => "Agent profile",
                ObjectKind::Environment => "Environment",
                _ => "Resource",
            }
        };
        activity::stop_requests(
            unit.conn,
            &requests,
            &format!(
                "{label} {}",
                if deleted { "removed" } else { "switched off" }
            ),
        )
        .await?;
        let harnesses = if target.model_id.is_none()
            && matches!(target.object.kind, ObjectKind::Provider | ObjectKind::Agent)
        {
            sqlx::query_as("SELECT provider_id,agent_id,machine_id FROM harnesses WHERE (($2='provider' AND provider_id=$1) OR ($2='agent' AND agent_id=$1)) AND (($3 AND $2='provider') OR status='online' OR start_command_id IS NOT NULL)")
                .bind(target.object.id).bind(target.object.kind.as_str()).bind(deleted)
                .fetch_all(&mut *unit.conn).await.map_err(DbError::from)?
        } else {
            Vec::new()
        };
        let (keepers, logout) = if target.object.kind == ObjectKind::Provider
            && target.model_id.is_none()
        {
            let provider: Option<(Option<Id>, Option<Id>, String)> =
                sqlx::query_as("SELECT machine_id,key_machine_id,kind FROM providers WHERE id=$1")
                    .bind(target.object.id)
                    .fetch_optional(&mut *unit.conn)
                    .await
                    .map_err(DbError::from)?;
            provider.map_or((Vec::new(), false), |(runtime, keeper, kind)| {
                let mut machines: Vec<_> = runtime.into_iter().chain(keeper).collect();
                machines.sort_unstable();
                machines.dedup();
                (machines, deleted && kind == "subscription")
            })
        } else {
            (Vec::new(), false)
        };
        let keepers: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM machines WHERE id=ANY($1) AND 'provider'=ANY(roles) ORDER BY id",
        )
        .bind(&keepers)
        .fetch_all(&mut *unit.conn)
        .await
        .map_err(DbError::from)?;
        let units = if target.model_id.is_none()
            && matches!(
                target.object.kind,
                ObjectKind::Provider | ObjectKind::Environment
            ) {
            sqlx::query_as("SELECT r.id,h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE (($2='provider' AND r.provider_id=$1) OR ($2='environment' AND r.environment_id=$1)) AND r.state IN ('running','starting','stopped') ORDER BY r.id")
                .bind(target.object.id).bind(target.object.kind.as_str()).fetch_all(&mut *unit.conn).await.map_err(DbError::from)?
        } else {
            Vec::new()
        };
        let pending_native = !turns.is_empty()
            || !requests.is_empty()
            || !harnesses.is_empty()
            || !keepers.is_empty()
            || !units.is_empty();
        let plan = json!({"quiesce":Recipe{target,turns,requests,deleted,harnesses,keepers,units,logout,pending_native},"committed":true});
        let row = jobs::insert(
            unit.conn,
            &jobs::NewJob {
                id: new_id(),
                kind: KIND,
                node_id: None,
                environment_id: None,
                provider_id: None,
                plan: &plan,
                progress: &serde_json::to_value(initial_progress(&PHASES))?,
                created_by: Some(by),
            },
            now,
        )
        .await?;
        Ok((row, unit.finish().await?))
    }

    /// Sends the accepted stops immediately after commit; the durable runner repeats them after a crash.
    pub async fn stop_resource_now(&self, id: Id) -> Result<(), EngineError> {
        let row = jobs::job(&mut *self.db().acquire().await?, id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the accepted stop is gone"))?;
        let recipe: Recipe = serde_json::from_value(row.plan["quiesce"].clone())?;
        let mut failure = None;
        for request in recipe.requests {
            if let Err(error) = self.inner.commands.cancel(request).await {
                failure = Some(EngineError::from(error));
            }
        }
        for (thread, _) in recipe.turns {
            if let Err(error) = self.drive_interrupt(thread).await {
                failure = Some(error);
            }
            self.wake(thread);
        }
        self.kick();
        failure.map_or(Ok(()), Err)
    }
}

/// Carries an already-accepted immediate stop to native acknowledgements without changing the paused state.
pub struct Quiescing {
    engine: Engine,
    recipe: Recipe,
}
impl Quiescing {
    /// Restores the exact accepted work identities, never a new authority to stop unrelated work.
    pub fn of(engine: Engine, row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            engine,
            recipe: serde_json::from_value(row.plan["quiesce"].clone())?,
        })
    }
}
#[async_trait]
impl Phased for Quiescing {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "stop" => {
                self.engine.stop_resource_now(job.id()).await?;
                loop {
                    let mut active = false;
                    for (thread, turn) in &self.recipe.turns {
                        let row = threads::turn(
                            &mut *self
                                .engine
                                .db()
                                .acquire()
                                .await
                                .map_err(EngineError::from)?,
                            *turn,
                        )
                        .await?;
                        if row.is_some_and(|turn| turn.is_active()) {
                            active = true;
                            self.engine.drive_interrupt(*thread).await?;
                        }
                    }
                    if !active {
                        break;
                    }
                    job.progress(0.0, Some("Waiting for the interrupted turns to stop"))
                        .await?;
                    job.pause(OBSERVE_EVERY).await?;
                }
            }
            "units" => {
                let target = &self.recipe.target;
                if target.model_id.is_some() {
                    return Ok(Done::Finished);
                }
                let removing_provider =
                    self.recipe.deleted && target.object.kind == ObjectKind::Provider;
                for (provider, agent, machine) in &self.recipe.harnesses {
                    let command = Outgoing::new(
                        ids::derived(
                            "resource.pause.harness",
                            &[
                                job.id().as_bytes(),
                                provider.as_bytes(),
                                agent.as_bytes(),
                                machine.as_bytes(),
                            ],
                        ),
                        *machine,
                        SerialKey::Harness {
                            provider_id: *provider,
                            agent_id: *agent,
                        },
                        &AgentStop {
                            provider_id: *provider,
                            agent_id: *agent,
                            logout: self.recipe.logout.then_some(true),
                        },
                    )
                    .map_err(EngineError::from)?;
                    if removing_provider {
                        // Deletion cannot wait for a missing machine. Its exact native cleanup remains
                        // a durable command, and its acknowledgement closes delivered memberships.
                        self.engine
                            .inner
                            .commands
                            .issue(command)
                            .await
                            .map_err(EngineError::from)?;
                    } else {
                        job.progress(0.0, Some("Waiting for the resource's harnesses to stop"))
                            .await?;
                        job.command_to_undo(command).await?;
                        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
                        orch_db::queries::resources::thread_engine::set_harness_status(
                            &mut tx,
                            *provider,
                            *agent,
                            *machine,
                            &orch_db::queries::resources::thread_engine::HarnessReport {
                                status: crate::harnesses::OFFLINE,
                                detail: None,
                                version: None,
                                sessions_open: 0,
                            },
                            job.now(),
                        )
                        .await?;
                        orch_db::queries::session_dependencies::retire_harness(
                            &mut tx,
                            *provider,
                            *agent,
                            *machine,
                            job.now(),
                        )
                        .await?;
                        tx.commit().await.map_err(DbError::from)?;
                        crate::engine::lock(&self.engine.inner.sessions).restarted(
                            crate::harnesses::HarnessKey {
                                provider: *provider,
                                agent: *agent,
                                machine: *machine,
                            },
                        );
                    }
                }
                if removing_provider {
                    let mut conn = self
                        .engine
                        .db()
                        .acquire()
                        .await
                        .map_err(EngineError::from)?;
                    crate::provider_runtime::retire_runtime(&mut conn, target.object.id, job.now())
                        .await?;
                }
                for machine in &self.recipe.keepers {
                    let command = Outgoing::new(
                        ids::derived(
                            "resource.pause.provider",
                            &[job.id().as_bytes(), machine.as_bytes()],
                        ),
                        *machine,
                        SerialKey::Provider(target.object.id),
                        &ProviderStop {
                            provider_id: target.object.id,
                            purge: removing_provider.then_some(true),
                        },
                    )
                    .map_err(EngineError::from)?;
                    if removing_provider {
                        self.engine
                            .inner
                            .commands
                            .issue(command)
                            .await
                            .map_err(EngineError::from)?;
                    } else {
                        job.progress(0.0, Some("Waiting for the provider's runtime to stop"))
                            .await?;
                        job.command_to_undo(command).await?;
                    }
                }
                if removing_provider {
                    return Ok(Done::Finished);
                }
                for (runtime, machine) in &self.recipe.units {
                    job.command_to_undo(
                        Outgoing::new(
                            ids::derived(
                                "resource.pause.unit",
                                &[job.id().as_bytes(), runtime.as_bytes()],
                            ),
                            *machine,
                            SerialKey::Runtime(*runtime),
                            &NodeRuntimeStop {
                                runtime_id: *runtime,
                                reason: None,
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                    sqlx::query("UPDATE runtimes SET state='stopped',updated_at=$2 WHERE id=$1 AND state IN ('running','starting')").bind(runtime).bind(job.now()).execute(&mut *self.engine.db().acquire().await.map_err(EngineError::from)?).await.map_err(DbError::from)?;
                }
            }
            _ => return Err(Failure::new("phase", "the stop phase is unknown").into()),
        }
        Ok(Done::Finished)
    }
    async fn undo(&self, _job: &Job, _why: &Stop) -> Undone {
        Undone::Partly(
            "The resource stays switched off; its remaining native stop is still required".into(),
        )
    }
}
