//! Archive, bringing back, soft-delete and restore orchestration; the deletion of an org at any level: where its members go, and what becomes of each of its resources.
//!
//! Settlement and its native controls are committed together. The command journal carries an
//! interrupt, lease return and session close through restart; terminal work is never revived.

use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::ids;
use crate::preflight::Blocked;
use crate::turns::{InputAfter, TurnEnd};
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_db::Conn;
use orch_db::queries::{approvals, hierarchy, lifecycle, link, threads};
use orch_link_contract::commands::{LeaseClose, SerialKey, TurnInterrupt};
use orch_types::{Id, Json, Time};
use serde_json::json;

/// Why an environment removal could not be scheduled atomically.
#[derive(Debug, thiserror::Error)]
pub enum RemovalScheduleError {
    /// Another durable job must settle before this environment can be removed.
    #[error("another job holds the environment")]
    Busy(Box<orch_db::queries::jobs::JobRow>),
    /// The environment is no longer present.
    #[error("the environment is gone")]
    Gone,
    /// A dependency failed while building the cleanup plan.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

/// Counts of work settled by one lifecycle action.
#[derive(Debug, Default, Clone)]
pub struct Settled {
    /// Threads included in the action.
    pub threads: usize,
    /// Active turns ended by the action.
    pub turns: usize,
    /// Inputs cancelled before their delivery completed.
    pub inputs: usize,
    /// Pending approvals cancelled.
    pub approvals: usize,
}

/// Every state a lifecycle action may settle while holding the thread row.
const THREAD_STATES: [&str; 5] = ["idle", "starting", "running", "waiting_approval", "blocked"];
/// The serial-key member used by the shared command journal.
const COMMAND_KEY: &str = "$key";
/// Seconds in the restoration window's complete UTC days.
const SECONDS_PER_DAY: u64 = 86_400;

impl Engine {
    /// Takes thread locks before an archive or deletion writes hierarchy flags.
    pub async fn lock_archive_in(&self, conn: &mut Conn, node: Id) -> Result<(), EngineError> {
        lifecycle::lock_threads_under(conn, node).await?;
        Ok(())
    }

    /// Locks the combined thread set before deleting a home with bindings in other places.
    pub async fn lock_home_in(&self, conn: &mut Conn, node: Id) -> Result<(), EngineError> {
        let mut ids = lifecycle::threads_under(conn, node).await?;
        for environment in lifecycle::environments_under(conn, node).await? {
            ids.extend(lifecycle::bindings(conn, environment).await?);
        }
        ids.sort_unstable();
        ids.dedup();
        threads::tools::lock_threads(conn, &ids).await?;
        Ok(())
    }

    /// Queues owned-unit cleanup or keeps an explicitly handed-over environment in the same transaction.
    pub async fn remove_home_in(
        &self,
        conn: &mut Conn,
        node: Id,
        by: Id,
        now: Time,
        keep_at: Option<Id>,
    ) -> Result<Vec<Id>, RemovalScheduleError> {
        self.remove_home_as_in(conn, node, Some(by), now, keep_at)
            .await
    }

    /// Preserves ordinary retained cleanup when a system retention policy deletes a home.
    pub(crate) async fn remove_home_as_in(
        &self,
        conn: &mut Conn,
        node: Id,
        by: Option<Id>,
        now: Time,
        keep_at: Option<Id>,
    ) -> Result<Vec<Id>, RemovalScheduleError> {
        let environments = lifecycle::environments_under(conn, node)
            .await
            .map_err(EngineError::from)?;
        let rows = hierarchy::with_ancestors(conn, &[node])
            .await
            .map_err(EngineError::from)?;
        let org = rows.iter().any(|row| row.id == node && row.kind == "org");
        for environment in &environments {
            if let Some(parent) = keep_at {
                lifecycle::rehome(conn, *environment, parent, now.get())
                    .await
                    .map_err(EngineError::from)?;
                continue;
            }
            self.remove_bindings_as_in(conn, *environment, by, now)
                .await?;
            let row = orch_db::queries::environments::routes::environments(conn, &[*environment])
                .await
                .map_err(EngineError::from)?
                .pop()
                .ok_or(RemovalScheduleError::Gone)?;
            if orch_db::queries::environment_facts::of(conn, *environment)
                .await
                .map_err(EngineError::from)?
                .is_some_and(|facts| facts.attaches())
            {
                orch_db::queries::environments::routes::remove_registration(
                    conn,
                    *environment,
                    now.get(),
                )
                .await
                .map_err(EngineError::from)?;
            } else if row.remove_at.is_none() {
                if org {
                    let user = by.ok_or_else(|| {
                        EngineError::unreadable("system retention does not delete orgs")
                    })?;
                    self.schedule_org_home_removal_in(conn, *environment, user, now)
                        .await?;
                } else {
                    self.check_removal_job(conn, *environment).await?;
                    self.schedule_environment_removal(conn, *environment, by, now.get(), now.get())
                        .await?;
                }
                orch_db::queries::environments::routes::schedule_removal(
                    conn,
                    *environment,
                    now.get(),
                    now.get(),
                )
                .await
                .map_err(EngineError::from)?;
            }
        }
        Ok(environments)
    }

    /// Freezes the canonical retained-file promise before confirmed removal commits.
    pub async fn schedule_removal_in(
        &self,
        conn: &mut Conn,
        environment: Id,
        at: DateTime<Utc>,
        by: Id,
        now: Time,
    ) -> Result<bool, RemovalScheduleError> {
        self.check_removal_job(conn, environment).await?;
        let accepted = self
            .schedule_environment_removal(conn, environment, Some(by), at, now.get())
            .await?;
        // An accepted immediate delete owns durable work before its transaction commits. A
        // future schedule remains withdrawable until its deadline; neither waits on a machine here.
        let due = orch_db::queries::environments::hosting::environment(conn, environment)
            .await
            .map_err(EngineError::from)?
            .and_then(|row| row.remove_at)
            .is_some_and(|deadline| deadline <= now.get());
        if due {
            use crate::jobs::runner::MakeError;
            match crate::jobs::remove::queue(conn, now.get(), environment, None).await {
                Ok(job) if accepted => {
                    // Its scheduled audit entry is appended later in this same transaction.
                    // Only the first acceptance can supply the request's safe origin.
                    orch_db::queries::jobs::merge_plan(
                        conn,
                        job.id,
                        &json!({"origin":orch_db::audit::current_origin()}),
                        now.get(),
                    )
                    .await
                    .map_err(EngineError::from)?;
                }
                Ok(_) => {}
                Err(MakeError::Running(job)) => {
                    let promise =
                        orch_db::queries::environments::retained::removal(conn, environment)
                            .await
                            .map_err(EngineError::from)?
                            .ok_or(RemovalScheduleError::Gone)?;
                    if job.kind != orch_db::queries::jobs::kind::ENVIRONMENT_REMOVE
                        || job.plan.get("runtime") != Some(&serde_json::Value::Null)
                        || job
                            .plan
                            .get("removal_revision")
                            .and_then(serde_json::Value::as_i64)
                            != Some(promise.removal_revision)
                    {
                        return Err(RemovalScheduleError::Busy(job));
                    }
                }
                Err(MakeError::NoEnvironment) => return Err(RemovalScheduleError::Gone),
                Err(MakeError::Db(error)) => return Err(EngineError::from(error).into()),
            }
        }
        Ok(accepted)
    }

    /// Preserves an org's recoverable data for its complete restoration window.
    pub async fn schedule_org_home_removal_in(
        &self,
        conn: &mut Conn,
        environment: Id,
        by: Id,
        now: Time,
    ) -> Result<bool, RemovalScheduleError> {
        self.check_removal_job(conn, environment).await?;
        let days = i64::try_from(self.inner.config.soft_delete.as_secs() / SECONDS_PER_DAY)
            .unwrap_or(i64::MAX);
        Ok(self
            .schedule_environment_removal_with_days(
                conn,
                environment,
                Some(by),
                (now.get(), now.get()),
                Some(days),
            )
            .await?)
    }

    async fn check_removal_job(
        &self,
        conn: &mut Conn,
        environment: Id,
    ) -> Result<(), RemovalScheduleError> {
        if let Some(job) = orch_db::queries::jobs::retained::open_of_environment(conn, environment)
            .await
            .map_err(EngineError::from)?
            .into_iter()
            .find(|job| job.kind != orch_db::queries::jobs::kind::ENVIRONMENT_REMOVE)
        {
            return Err(RemovalScheduleError::Busy(Box::new(job)));
        }
        Ok(())
    }

    /// Settles every live thread below an archived node inside its caller's authority transaction.
    pub async fn archive_in(
        &self,
        conn: &mut Conn,
        node: Id,
        by: Id,
        now: Time,
    ) -> Result<Settled, EngineError> {
        self.archive_as_in(conn, node, Some(by), now).await
    }

    /// Runs the same settlement for the system's configured retention policy.
    pub(crate) async fn archive_as_in(
        &self,
        conn: &mut Conn,
        node: Id,
        by: Option<Id>,
        now: Time,
    ) -> Result<Settled, EngineError> {
        let ids = lifecycle::threads_under(conn, node).await?;
        let mut unit = Unit::new(conn, now.get());
        let mut result = self
            .settle_in(&mut unit, &ids, by, "archived", false)
            .await?;
        let rows = hierarchy::with_ancestors(&mut *unit.conn, &[node]).await?;
        let root = rows
            .iter()
            .find(|row| row.id == node)
            .ok_or_else(|| EngineError::unreadable("the archived node is gone"))?;
        let actor = match by {
            Some(user) => crate::views::user_ref_json(&mut *unit.conn, user).await?,
            None => serde_json::Value::Null,
        };
        let payload = json!({"marker":"archived", "data": {
            "node": {"id":root.id,"kind":root.kind,"name":root.name,"redacted":false}, "by":actor,
        }});
        // Parents are stamped by a lane's close, so close lanes before ending their parent's segment.
        for id in &ids {
            if threads::forking::side_parent(&mut *unit.conn, *id)
                .await?
                .is_some()
            {
                threads::forking::close_with_marker(
                    &mut *unit.conn,
                    *id,
                    "closed",
                    by,
                    None,
                    now.get(),
                )
                .await?;
            }
        }
        for id in &ids {
            let thread = threads::thread(&mut *unit.conn, *id)
                .await?
                .ok_or_else(|| EngineError::unreadable("an archived thread is gone"))?;
            if let Some(segment) = thread.current_segment_id {
                let seq = unit.stamp(*id).await?;
                threads::end_segment(&mut *unit.conn, segment, "archived", now.get(), seq).await?;
                crate::ingest::add_marker(
                    &mut unit,
                    *id,
                    segment,
                    None,
                    &payload,
                    &format!("archived:{node}:{seq}"),
                )
                .await?;
            }
        }
        result.threads = ids.len();
        unit.finish().await?;
        Ok(result)
    }

    /// Settles and detaches current bindings before confirmed environment removal.
    pub async fn remove_bindings_in(
        &self,
        conn: &mut Conn,
        environment: Id,
        by: Id,
        now: Time,
    ) -> Result<Settled, EngineError> {
        self.remove_bindings_as_in(conn, environment, Some(by), now)
            .await
    }

    async fn remove_bindings_as_in(
        &self,
        conn: &mut Conn,
        environment: Id,
        by: Option<Id>,
        now: Time,
    ) -> Result<Settled, EngineError> {
        let ids = lifecycle::bindings(conn, environment).await?;
        let mut unit = Unit::new(conn, now.get());
        let result = self
            .settle_in(&mut unit, &ids, by, "environment_removed", true)
            .await?;
        for id in ids {
            let thread = threads::thread(&mut *unit.conn, id)
                .await?
                .ok_or_else(|| EngineError::unreadable("a bound thread is gone"))?;
            let seq = unit.stamp(id).await?;
            if let Some(segment) = thread.current_segment_id {
                threads::end_segment(&mut *unit.conn, segment, "error", now.get(), seq).await?;
            }
            lifecycle::detach(&mut *unit.conn, id, environment).await?;
            threads::set_state(
                &mut *unit.conn,
                id,
                &THREAD_STATES,
                &threads::StateChange::blocked("unbound", Some("Its environment was removed.")),
                seq,
            )
            .await?;
        }
        unit.finish().await?;
        Ok(result)
    }

    pub(crate) async fn settle_side_in(
        &self,
        conn: &mut Conn,
        side: Id,
        by: Option<Id>,
        now: Time,
    ) -> Result<(), EngineError> {
        let Some(thread) = threads::thread(conn, side).await? else {
            return Ok(());
        };
        let mut unit = Unit::new(conn, now.get());
        self.settle_in(&mut unit, &[side], by, "closed", false)
            .await?;
        if let Some(segment) = thread.current_segment_id {
            let seq = unit.stamp(side).await?;
            threads::end_segment(&mut *unit.conn, segment, "archived", now.get(), seq).await?;
        }
        unit.finish().await?;
        Ok(())
    }

    pub(crate) async fn settle_in(
        &self,
        unit: &mut Unit<'_>,
        ids: &[Id],
        by: Option<Id>,
        reason: &str,
        removing: bool,
    ) -> Result<Settled, EngineError> {
        threads::tools::lock_threads(&mut *unit.conn, ids).await?;
        let mut counts = Settled {
            threads: ids.len(),
            ..Settled::default()
        };
        for id in ids {
            let Some(thread) = threads::thread(&mut *unit.conn, *id).await? else {
                continue;
            };
            let now = unit.now();
            if let Some(hold) = crate::locks::lock_of(&mut *unit.conn, *id).await?
                && matches!(
                    hold.job.kind.as_str(),
                    "thread_fork" | "thread_continuation"
                )
            {
                if hold.job.kind == "thread_continuation" {
                    crate::jobs::continuation::supersede_in(
                        &mut *unit.conn,
                        hold.job.id,
                        &thread,
                        reason,
                        by,
                        now,
                    )
                    .await?;
                } else {
                    orch_db::queries::jobs::request_cancel_by(
                        &mut *unit.conn,
                        hold.job.id,
                        by,
                        now,
                    )
                    .await?;
                }
            }
            let machine = self.harness_machine_of(&mut *unit.conn, *id).await?;
            lifecycle::cancel_delivery(&mut *unit.conn, *id, now).await?;
            counts.approvals += approvals::pending_of_thread(&mut *unit.conn, *id)
                .await?
                .len();
            let lease = crate::leases::close_for_turn(
                unit,
                *id,
                if reason == "disabled" {
                    "disabled"
                } else {
                    "archived"
                },
            )
            .await?;
            if let Some(turn) = threads::active_turn(&mut *unit.conn, *id).await? {
                if let Some(machine) = machine {
                    persist(
                        &mut *unit.conn,
                        now,
                        Outgoing::new(
                            ids::turn_interrupt(turn.id),
                            machine,
                            SerialKey::Thread(*id),
                            &TurnInterrupt {
                                thread_id: *id,
                                turn_id: turn.id,
                            },
                        )?,
                    )
                    .await?;
                }
                self.end_turn(
                    unit,
                    &thread,
                    &turn,
                    TurnEnd {
                        state: "interrupted",
                        error: Some(crate::turns::error_json(reason, reason, false)),
                        input: InputAfter::Keep,
                        then_block: Some(Blocked {
                            reason: "error",
                            detail: Some(reason.to_owned()),
                        }),
                    },
                )
                .await?;
                counts.turns += 1;
            }
            // The ending owns the selected execution queue. Retained inactive
            // versions keep their words, status and Stop state unchanged. The
            // thread lock above keeps this selected owner fixed across pages.
            let mut after = None;
            loop {
                let inputs = orch_db::queries::thread_continuations::reading::open_inputs(
                    &mut *unit.conn,
                    *id,
                    thread.active_continuation_id,
                    after,
                    500,
                )
                .await?;
                after = inputs.last().map(|input| input.id);
                let last_page = inputs.len() < 500;
                for input in inputs {
                    let seq = unit.stamp(*id).await?;
                    if threads::change_input(
                        &mut *unit.conn,
                        input.id,
                        &["pending", "delivering"],
                        &threads::InputChange {
                            state: "cancelled",
                            detail: Some(reason),
                            cancelled_by: by,
                            ..threads::InputChange::default()
                        },
                        seq,
                    )
                    .await?
                    {
                        counts.inputs += 1;
                    }
                }
                if last_page {
                    break;
                }
            }
            for approval in approvals::pending_of_thread(&mut *unit.conn, *id).await? {
                if approvals::cancel_pending(&mut *unit.conn, approval.id, now).await? {
                    crate::notifications::approval_settled(unit, approval.id).await?;
                    if let Some(item) = approval.item_id {
                        let seq = unit.stamp(*id).await?;
                        threads::set_item_status(
                            &mut *unit.conn,
                            item,
                            &["pending"],
                            "cancelled",
                            now,
                            seq,
                        )
                        .await?;
                    }
                }
            }
            if let Some(lease) = lease {
                queue_lease_close(&mut *unit.conn, now, &lease).await?;
            }
            if let Some(machine) = machine {
                let open_segment = match thread.current_segment_id {
                    Some(segment) => threads::segment(&mut *unit.conn, segment)
                        .await?
                        .is_some_and(|segment| segment.ended_at.is_none()),
                    None => false,
                };
                if (open_segment || thread.current_turn_id.is_some())
                    && !lifecycle::closing(&mut *unit.conn, *id).await?
                {
                    let seq = unit.stamp(*id).await?;
                    let command = ids::derived(
                        "lifecycle.session.close",
                        &[id.as_bytes(), &seq.to_be_bytes()],
                    );
                    let outgoing = crate::commands::session_close(
                        &mut *unit.conn,
                        command,
                        machine,
                        *id,
                        thread.current_segment_id,
                    )
                    .await?;
                    persist(&mut *unit.conn, now, outgoing).await?;
                }
            }
            let seq = unit.stamp(*id).await?;
            let state = if removing
                || thread.environment_id.is_none()
                || thread.provider_id.is_none()
                || thread.agent_id.is_none()
            {
                threads::StateChange::blocked("unbound", None)
            } else {
                threads::StateChange::plain("idle")
            };
            threads::set_state(&mut *unit.conn, *id, &THREAD_STATES, &state, seq).await?;
        }
        Ok(counts)
    }

    /// Delivers lifecycle controls from their committed journal, including after process restart.
    pub(crate) async fn deliver_lifecycle_controls(&self) -> Result<(), EngineError> {
        self.retry_withdrawn_registration_detaches().await?;
        self.retry_provider_purges().await?;
        self.reconcile_disabled_units().await?;
        let rows = {
            let mut conn = self.inner.db.acquire().await?;
            lifecycle::controls(&mut conn, self.inner.config.sweep_batch).await?
        };
        for mut row in rows {
            if row.kind == "node.runtime.stop" {
                if !self.prepare_disabled_unit_stop(&row).await? {
                    continue;
                }
                // A node that was away may return long after the accepted ending. Start the
                // command's execution budget when it can actually be delivered.
                if row.state == "queued" {
                    row.deadline_at = self.inner.clock.now()
                        + chrono::Duration::seconds(i64::from(
                            orch_link_contract::commands::CommandKind::NodeRuntimeStop
                                .deadline_secs(),
                        ));
                    let mut conn = self.inner.db.acquire().await?;
                    sqlx::query(
                        "UPDATE commands SET deadline_at=$2 WHERE id=$1 AND state='queued'",
                    )
                    .bind(row.id)
                    .bind(row.deadline_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
                }
            }
            let mut body = row.body.clone();
            let key = if row.serial_key() == Some("machine") {
                SerialKey::Machine
            } else {
                row.serial_key()
                    .and_then(|key| key.parse::<Id>().ok())
                    .map_or(SerialKey::None, |id| match row.kind.as_str() {
                        "lease.close" | "provider.stop" => SerialKey::Provider(id),
                        "node.runtime.stop" => SerialKey::Runtime(id),
                        "env.detach" => SerialKey::Environment(id),
                        _ => SerialKey::Thread(id),
                    })
            };
            if let Some(object) = body.as_object_mut() {
                object.remove(COMMAND_KEY);
            }
            let kind = row.kind.parse().map_err(EngineError::unreadable)?;
            if matches!(
                row.kind.as_str(),
                "session.close.fenced" | "session.continuation.close"
            ) {
                let segment = body
                    .get("segment_id")
                    .or_else(|| {
                        body.get("source")
                            .and_then(|source| source.get("segment_id"))
                    })
                    .and_then(Json::as_str)
                    .and_then(|segment| segment.parse().ok());
                if let Some(segment) = segment {
                    lock(&self.inner.sessions).closed(segment);
                }
            } else if row.kind == "session.close"
                && let Some(thread) = body["thread_id"]
                    .as_str()
                    .and_then(|thread| thread.parse().ok())
            {
                self.forget_open_sessions(thread);
            }
            self.inner
                .commands
                .issue(Outgoing {
                    id: row.id,
                    machine: row.machine_id,
                    key,
                    kind,
                    body,
                    deadline: Some(Time::new(row.deadline_at)),
                })
                .await?;
            if let Some(info) = self.inner.commands.status(row.id).await?
                && let Some(outcome) = info.outcome
            {
                let error;
                let ending = match outcome {
                    Outcome::Succeeded(ref result) => link::Ending::Succeeded(result),
                    Outcome::Failed(ref failure) => {
                        error = serde_json::to_value(failure)?;
                        link::Ending::Failed(&error)
                    }
                    Outcome::Expired => link::Ending::Expired,
                    Outcome::Cancelled => link::Ending::Cancelled,
                };
                let mut conn = self.inner.db.acquire().await?;
                link::finish_command(&mut conn, row.id, &ending, self.inner.clock.now()).await?;
                self.finish_disabled_unit_stop(&mut conn, &row, &outcome)
                    .await?;
            }
        }
        Ok(())
    }

    /// Retries deleted-provider key cleanup once its original keeper is online again.
    async fn retry_provider_purges(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        // One normal execution window between retries keeps a persistent failure from flooding the journal.
        let window = chrono::Duration::seconds(i64::from(
            orch_link_contract::commands::CommandKind::ProviderStop.deadline_secs(),
        ));
        let pending = {
            let mut conn = self.inner.db.acquire().await?;
            lifecycle::provider_purges_due(&mut conn, now - window, self.inner.config.sweep_batch)
                .await?
        };
        for previous in pending {
            let provider: Id = previous
                .body
                .get("provider_id")
                .and_then(Json::as_str)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| {
                    EngineError::unreadable("the provider cleanup has no valid provider")
                })?;
            let serial_key = provider.to_string();
            if previous
                .serial_key()
                .is_some_and(|key| key != serial_key.as_str())
            {
                return Err(EngineError::unreadable(
                    "the provider cleanup has another serial identity",
                ));
            }
            let id = ids::derived("provider.purge.retry", &[previous.id.as_bytes()]);
            let mut tx = self.inner.db.begin().await?;
            if lifecycle::claim_provider_purge_retry(&mut tx, previous.id, id).await? {
                let inserted = link::insert_command(
                    &mut tx,
                    &link::NewCommand {
                        id,
                        machine_id: previous.machine_id,
                        kind: &previous.kind,
                        body: &previous.body,
                        serial_key: Some(&serial_key),
                        deadline_at: now + window,
                    },
                    now,
                )
                .await?;
                if !inserted {
                    return Err(EngineError::unreadable(
                        "the provider cleanup retry identity is already in use",
                    ));
                }
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(())
    }

    async fn retry_withdrawn_registration_detaches(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let pause = chrono::Duration::from_std(crate::commands::BUSY_RETRY_PAUSE)
            .map_err(EngineError::unreadable)?;
        let pending = {
            let mut conn = self.inner.db.acquire().await?;
            lifecycle::busy_registration_detaches(
                &mut conn,
                now - pause,
                now,
                self.inner.config.sweep_batch,
            )
            .await?
        };
        for previous in pending {
            let id = crate::commands::busy_retry_id(previous.id);
            let mut tx = self.inner.db.begin().await?;
            if lifecycle::claim_busy_registration_detach(&mut tx, previous.id, id, now).await? {
                link::insert_command(
                    &mut tx,
                    &link::NewCommand {
                        id,
                        machine_id: previous.machine_id,
                        kind: &previous.kind,
                        body: &previous.body,
                        serial_key: previous.serial_key.as_deref(),
                        deadline_at: previous.deadline_at,
                    },
                    now,
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(())
    }
}

pub(crate) async fn persist(
    conn: &mut Conn,
    now: DateTime<Utc>,
    command: Outgoing,
) -> Result<(), EngineError> {
    let body = command.body;
    let serial_key = command.key.wire();
    let deadline = now + chrono::Duration::seconds(i64::from(command.kind.deadline_secs()));
    link::insert_command(
        conn,
        &link::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &body,
            serial_key: serial_key.as_deref(),
            deadline_at: deadline,
        },
        now,
    )
    .await?;
    Ok(())
}

async fn queue_lease_close(
    conn: &mut Conn,
    now: DateTime<Utc>,
    lease: &threads::LeaseRow,
) -> Result<(), EngineError> {
    if let Some(provider) =
        orch_db::queries::resources::thread_engine::provider(conn, lease.provider_id).await?
        && let Some(machine) = crate::leases::machine_of(&provider)
    {
        persist(
            conn,
            now,
            Outgoing::new(
                ids::lease_close(lease.id),
                machine,
                SerialKey::Provider(lease.provider_id),
                &LeaseClose { lease_id: lease.id },
            )?,
        )
        .await?;
    }
    Ok(())
}
