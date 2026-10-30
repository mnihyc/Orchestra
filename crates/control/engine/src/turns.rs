//! Turn state transitions, the phase a turn is in as its thread's status line says it, interrupt and timeouts.
//!
//! A turn is made by pre-flight in `starting`; this module carries it from
//! there. The start sequence (`Engine::drive_start`) asks of the machines,
//! in order, what the turn needs: where the turn goes through the
//! forwarder, the provider on the machine that keeps its key
//! (`provider.start`); the harness process (`agent.start`); the session
//! (`session.open`); the lease at the provider (`lease.open`), after the
//! session because a session outlasts the turn and a lease does not; then
//! the turn itself (`turn.start`). Each is a command whose identifier is
//! derived from what it is for, so that a process that starts again repeats
//! none of them, and a provider that runs already is not started again by
//! the next turn. The thread's `starting_step` says which one it waits for.
//! The harness then says `turn.started`, and the events that follow are
//! applied by `ingest`.
//!
//! **Ends.** A turn ends in one of three ways, and one function (`Engine::end_turn`)
//! makes the end whatever its cause: the harness says it completed, was
//! interrupted or failed; the engine gives up on a start that was not
//! accepted in time or could not go on; the engine ends a turn that did not
//! stop when asked. Making the end settles everything that hung on the turn
//! (its unfinished items, its approvals, its lease, the inputs steered into
//! it) and clears its current pointer in one transaction. Frozen terminal
//! accounting and notifications are retained for a thread-locked follow-up;
//! the next input waits until that transaction has committed too.
//!
//! **Interrupts.** A person's interrupt marks the turn and sends
//! `turn.interrupt`; a turn that was not yet accepted by the harness is
//! ended on the spot. The harness has `interrupt_force_secs` to answer with
//! `turn.completed` (`interrupted`); after that the engine ends the turn
//! itself with the code `forced` and closes the session, which the next turn
//! opens again by resuming it.

use crate::commands::{Info, Outcome, Outgoing, Stage};
use crate::engine::{Engine, lock};
use crate::environments::Awaited;
use crate::error::EngineError;
use crate::harnesses::Started as HarnessStarted;
use crate::ids;
use crate::ingest;
use crate::instructions::{self, InputRules};
use crate::model_inputs::{Delivery, ModelBlock, Sent};
use crate::preflight::{self, Blocked};
use crate::providers::{Keeping, classify_keeper, classify_plan};
use crate::sessions::{self, StartLoad, StartSnapshot};
use crate::unit::{Finished, Unit};
use orch_access_contract::decisions::{Decision, Refusal};
use orch_access_contract::permissions::Permission;
use orch_agent_contract::credentials::CredentialPath;
use orch_api_contract::threads::{ContentPart, Turn};
use orch_authz::{Act, Caller, Principal, nodes};
use orch_db::queries::approvals as approval_queries;
use orch_db::queries::environments::thread_engine as environments;
use orch_db::queries::threads::{
    self, InputChange, InputRow, ThreadRow, TurnRow, input_state, thread_state, turn_state,
};
use orch_link_contract::commands::{
    ErrorCode, FsMkdir, SerialKey, SessionMode, SessionOpenResult, TurnInterrupt, TurnStart,
    TurnSteer,
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::time::Duration;

mod end_work;

/// This exact cause belongs to retained terminal follow-up, not to a person's Stop
/// or another failure. Successful follow-up may clear only this cause.
pub(crate) const END_FOLLOWUP_FAILED: &str = "The last turn ended, but its usage and notifications could not be saved. Check the agent profile or storage.";

/// How many turns an input is given to start: it goes back to waiting once
/// after a start that failed for a reason that may pass, and stays failed
/// after a second, which points at something a person must look at.
pub const START_ATTEMPTS: i64 = 2;
/// How many times the session of one turn is opened again because the
/// harness said that it holds none: once, since a harness that loses a
/// session twice in a row is not going to hold the third.
const SESSION_REPAIRS: u32 = 1;
/// The last attempt at opening a lease: once, and again after the provider was started anew.
const LEASE_OPEN_ATTEMPTS: u32 = 1;
/// How many times the session of one turn is opened at most: once, and again
/// after each repair. The identifiers of the commands that open it are told
/// apart by turn and by attempt, so each turn reserves this many.
const SESSION_ATTEMPTS: u32 = SESSION_REPAIRS + 1;

/// What becomes of the input of a turn that ended before the harness took it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAfter {
    /// It was taken (or was never in doubt): it stays as it is.
    Keep,
    /// It was interrupted before delivery and returns without spending a failed-start attempt.
    Return,
    /// It waits again where it stood, if the failure may be retried and it has
    /// not failed twice; otherwise it stays failed.
    Retry {
        /// Whether trying again may help.
        retryable: bool,
    },
}

/// How a turn ends.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnEnd {
    /// `completed`, `interrupted` or `failed`.
    pub state: &'static str,
    /// What ended it, for a turn that did not complete.
    pub error: Option<Json>,
    /// What becomes of its input if the harness did not take it.
    pub input: InputAfter,
    /// A reason to block the thread with, instead of taking up the next
    /// input.
    pub then_block: Option<Blocked>,
}

/// What ending a turn did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ended {
    /// Whether this call ended it: a turn that is over is not ended again.
    pub changed: bool,
    /// The turn that took its place, where one was started.
    pub started: Option<preflight::Started>,
}

/// What a steer is made from, before the files of its input are written and its text is made.
struct SteerFacts {
    /// The machine of the harness, which the steer is sent to.
    machine: Id,
    /// The environment the thread runs in, where the files go.
    environment: environments::EnvironmentRow,
    /// What the input carries.
    content: Vec<ContentPart>,
    /// The name of its sender.
    name: Option<String>,
    /// Whether the sender is named in the text.
    attribute: bool,
    /// Whether the texts Orch composes may name a person: not under `collab.attribution = never`.
    names_people: bool,
    /// Where these are the first words to name their sender after words that named nobody: the notice of who sent
    /// those, said just before them.
    names_begin: Option<String>,
    /// The thread's directory, absolute.
    directory: String,
}

/// How the sending of a record ended, from the command's outcome.
fn sent_of(outcome: &Outcome) -> Sent {
    match outcome {
        Outcome::Succeeded(_) => Sent::Taken,
        Outcome::Failed(_) => Sent::Refused,
        Outcome::Expired | Outcome::Cancelled => Sent::Unknown,
    }
}

/// An error as a turn and the timeline carry it.
#[must_use]
pub fn error_json(code: &str, message: &str, retryable: bool) -> Json {
    json!({ "code": code, "message": message, "retryable": retryable })
}

/// The error a harness ended a turn with, as the turn carries it: its code and sentence, and what the harness or
/// its vendor said, the moment a limit of the plan ends, and whether the answer may have been cut short.
#[must_use]
pub fn turn_error_json(error: &orch_link_contract::events::TurnError) -> Json {
    let mut stored = json!({
        "code": error.code, "message": error.message, "retryable": error.retryable,
        "reason": error.reason, "status": error.status, "retry_after_secs": error.retry_after_secs,
        "said": error.said, "resets_at": error.resets_at, "incomplete": error.incomplete, "window": error.window,
    });
    // The limit's structure, where the adapter read one: what a screen says and offers, not the vendor's words.
    if let Some(limit) = &error.limit {
        stored["limit"] = limit.clone();
    }
    stored
}

/// The `close_reason` of a lease whose turn ended in `state`.
fn lease_reason(state: &str) -> &'static str {
    match state {
        turn_state::COMPLETED => crate::leases::TURN_ENDED,
        turn_state::INTERRUPTED => crate::leases::INTERRUPTED,
        _ => crate::leases::ERROR,
    }
}

/// Whether the machine said that it holds no such provider: `lease.open` before `provider.start`.
fn provider_was_lost(outcome: &Outcome) -> bool {
    matches!(outcome, Outcome::Failed(error) if error.known() == Some(ErrorCode::NotFound))
}

/// The status the unfinished items of a turn are settled to.
fn item_status(state: &str) -> &'static str {
    match state {
        turn_state::COMPLETED => "completed",
        turn_state::INTERRUPTED => "cancelled",
        _ => "failed",
    }
}

impl Engine {
    /// Commits a transaction and takes up the follow-up work it asked for.
    pub(crate) async fn commit(
        &self,
        tx: orch_db::Tx,
        finished: Finished,
    ) -> Result<Finished, EngineError> {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if let Err(error) = self.invalidate_work().await {
            tracing::warn!(%error, "committed file changes await their durable work invalidation");
        }
        for thread in &finished.wakes {
            self.wake(*thread);
        }
        Ok(finished)
    }

    /// Ends a turn and retains its follow-up before the next input may start.
    /// The thread's row must be locked by the caller (it
    /// is, once any number has been stamped for it).
    pub(crate) fn end_turn<'a, 'unit>(
        &'a self,
        unit: &'a mut Unit<'unit>,
        thread: &'a ThreadRow,
        turn: &'a TurnRow,
        end: TurnEnd,
    ) -> impl std::future::Future<Output = Result<Ended, EngineError>> + Send + 'a {
        // Start, recovery and interrupt paths all await this state. Allocate it once here so
        // their nested futures do not embed terminalization into every branch's poll frame.
        Box::pin(self.end_turn_inner(unit, thread, turn, end))
    }

    async fn end_turn_inner(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        turn: &TurnRow,
        end: TurnEnd,
    ) -> Result<Ended, EngineError> {
        let thread_id = thread.node_id;
        // The items of the turn end with all the text they were told.
        self.write_held_of(unit, thread_id).await?;
        let seq = unit.stamp(thread_id).await?;
        let now = unit.now();
        let changed = threads::finish_turn(
            &mut *unit.conn,
            turn.id,
            end.state,
            end.error.as_ref(),
            now,
            seq,
        )
        .await?;
        crate::resource_facts::library::close_turn(unit.conn, turn.id).await?;
        let owns_current = thread.current_turn_id == Some(turn.id);
        if !changed && !owns_current {
            return Ok(Ended::default());
        }
        if !changed {
            // Republish a legacy partial end even to readers which already held its
            // old terminal sequence but never received a consistent thread row.
            sqlx::query("UPDATE turns SET seq_updated=$2 WHERE id=$1")
                .bind(turn.id)
                .bind(seq)
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
        }
        // Nothing fallible may commit a terminal turn while retaining its Running pointer.
        // The same transaction keeps the immutable accounting/notification work for replay.
        end_work::keep(unit, thread, turn, &end, self.inner.live.as_ref()).await?;
        // The caps of the next turn are looked at afresh; the thread's last look leaves the engine's memory.
        self.forget_cap_check(thread_id);
        if changed
            && let Some(segment) = threads::segment(&mut *unit.conn, turn.segment_id).await?
            && let Some(agent) =
                orch_db::queries::resources::thread_engine::agent(&mut *unit.conn, segment.agent_id)
                    .await?
        {
            metrics::counter!("orch_turns_total","status"=>end.state,"harness"=>agent.harness_kind.clone()).increment(1);
            let elapsed = (now - turn.started_at.unwrap_or(turn.created_at))
                .num_milliseconds()
                .max(0) as f64
                / 1000.0;
            metrics::histogram!("orch_turn_seconds","status"=>end.state,"harness"=>agent.harness_kind).record(elapsed);
        }
        self.observe_turn_end(unit, thread, turn, seq).await?;
        threads::settle_items_of_turn(&mut *unit.conn, turn.id, item_status(end.state), now, seq)
            .await?;
        crate::approvals::cancel_pending_harness_for_turn(unit, thread_id, turn.id).await?;
        if owns_current {
            crate::leases::close_for_turn(unit, thread_id, lease_reason(end.state)).await?;
        }
        // A turn that ends may leave room in a cap that held others back: the threads that wait for it are
        // looked at, the one whose oldest input is oldest first.
        self.room_may_have_been_made();
        // The input the turn answers, if the harness never took it.
        if let Some(input) = threads::input(&mut *unit.conn, turn.input_id).await?
            && input.state == input_state::DELIVERING
        {
            let keep_trying = match end.input {
                InputAfter::Keep => None,
                InputAfter::Return => Some(true),
                InputAfter::Retry { retryable } => Some(
                    retryable
                        && threads::count_failed_turns_of_input(&mut *unit.conn, input.id, turn.id)
                            .await?
                            < START_ATTEMPTS - 1,
                ),
            };
            match keep_trying {
                Some(true) => {
                    let returned = threads::return_input_in_place(
                        &mut *unit.conn,
                        input.id,
                        &[input_state::DELIVERING],
                        seq,
                    )
                    .await?;
                    if returned
                        && input.queue_held
                        && !thread.queue_held
                        && let Some(by) = turn.interrupted_by
                    {
                        // This was the only input at Stop; create its hold when
                        // it actually returns to the queue.
                        sqlx::query("UPDATE thread_inputs SET queue_held=false WHERE id=$1")
                            .bind(input.id)
                            .execute(&mut *unit.conn)
                            .await
                            .map_err(orch_db::DbError::from)?;
                        self.hold_queue(unit, thread, by).await?;
                    }
                }
                Some(false) => {
                    threads::change_input(
                        &mut *unit.conn,
                        input.id,
                        &[input_state::DELIVERING],
                        &InputChange {
                            state: input_state::FAILED,
                            detail: Some("start_failed"),
                            ..InputChange::default()
                        },
                        seq,
                    )
                    .await?;
                    // Words whose place a steer kept are settled with the input that failed.
                    ingest::settle_user_message(unit, thread_id, input.id, "failed").await?;
                }
                None => {}
            }
        }
        // What was being steered into a turn that is over waits again, first.
        for input in threads::delivering_inputs(&mut *unit.conn, thread_id).await? {
            if input.turn_id == Some(turn.id)
                && input.id != turn.input_id
                && input.state_detail.as_deref() != Some("steering")
            {
                threads::return_input_to_front(
                    &mut *unit.conn,
                    input.id,
                    &[input_state::DELIVERING],
                    seq,
                )
                .await?;
            }
        }
        // A late end for an older turn must not clear a newer turn's pointer or lease.
        if !owns_current {
            unit.wake(thread_id);
            return Ok(Ended {
                changed,
                started: None,
            });
        }
        // Earlier cleanup can stamp queue markers and approvals. Publish the
        // thread row after those changes so readers see the whole end.
        let seq = unit.stamp(thread_id).await?;
        threads::set_current(
            &mut *unit.conn,
            thread_id,
            thread.current_segment_id,
            None,
            seq,
        )
        .await?;
        threads::mark_activity(&mut *unit.conn, thread_id, seq, now).await?;
        if let Some(environment) = thread.environment_id {
            environments::touch_activity(&mut *unit.conn, environment, now).await?;
        }
        unit.wake(thread_id);
        match end.then_block {
            Some(blocked) => {
                crate::notifications::set_state(
                    unit,
                    thread_id,
                    &[
                        thread_state::STARTING,
                        thread_state::RUNNING,
                        thread_state::WAITING_APPROVAL,
                        thread_state::BLOCKED,
                        thread_state::IDLE,
                    ],
                    &threads::StateChange::blocked(blocked.reason, blocked.detail.as_deref()),
                    seq,
                )
                .await?;
                Ok(Ended {
                    changed: true,
                    started: None,
                })
            }
            None => {
                threads::set_state(
                    &mut *unit.conn,
                    thread_id,
                    &[
                        thread_state::STARTING,
                        thread_state::RUNNING,
                        thread_state::WAITING_APPROVAL,
                        thread_state::BLOCKED,
                    ],
                    &threads::StateChange::plain(thread_state::IDLE),
                    seq,
                )
                .await?;
                // Follow-up and next-input admission run after this core commit. A bad
                // profile cannot roll back or partially commit the completed state.
                Ok(Ended {
                    changed: true,
                    started: None,
                })
            }
        }
    }

    // ------------------------------------------------------------ the start sequence

    /// Says what a starting thread waits for.
    pub(crate) async fn set_step(&self, thread: Id, step: &str) -> Result<(), EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let row = threads::thread(&mut conn, thread).await?;
        if row.as_ref().is_none_or(|row| {
            row.state != thread_state::STARTING || row.starting_step.as_deref() == Some(step)
        }) {
            return Ok(());
        }
        drop(conn);
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            // The thread is looked at again under its lock: a number is spent
            // only on a change that is made.
            if let Some(locked) = threads::lock(&mut *unit.conn, thread).await?
                && locked.state == thread_state::STARTING
                && locked.starting_step.as_deref() != Some(step)
            {
                let seq = unit.stamp(thread).await?;
                threads::set_starting_step(&mut *unit.conn, thread, Some(step), seq).await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Carries a starting turn through what it needs: the lease, the harness,
    /// the session, then `turn.start`. Every step looks at the database
    /// again first, so it is safe to call at any time and from any process;
    /// it returns when the turn has been handed to the harness, or has failed.
    pub(crate) async fn drive_start(&self, thread_id: Id) -> Result<(), EngineError> {
        let mut repairs: u32 = 0;
        'driving: loop {
            if self.is_stopped() {
                return Ok(());
            }
            // The environment first, and in parallel with the rest: one that is stopped or still being
            // made is started and waited for, the thread saying that it waits for the environment.
            match self.await_environment(thread_id).await? {
                Awaited::Ready => {}
                Awaited::Waiting | Awaited::Failed => return Ok(()),
            }
            // The provider's runtime, where it is a managed one that is not up: started and waited for,
            // the thread saying that it waits for the provider.
            match self.await_provider_runtime(thread_id).await? {
                Awaited::Ready => {}
                Awaited::Waiting | Awaited::Failed => return Ok(()),
            }
            let loaded = {
                let mut conn = self.inner.db.acquire().await?;
                // Snapshot preparation walks shared material deeply; keep that future off the worker stack.
                Box::pin(self.load_start_snapshot(&mut conn, thread_id)).await?
            };
            let mut snap = match loaded {
                StartLoad::NoTurn => return Ok(()),
                StartLoad::Unstartable(turn) => {
                    return self.fail_unstartable(thread_id, turn).await;
                }
                StartLoad::Ready(snap) => *snap,
            };
            self.scope_agent_history_notices(&mut snap).await?;
            let own_side_recovery = self.agent_history_side_recipient(&snap).await?;
            if snap.segment.native_session_ref.is_some()
                && self.agent_history_was_removed(snap.segment.id).await?
            {
                let closed = self.close_missing_agent_history(&snap).await?;
                if !matches!(closed, Outcome::Succeeded(_)) {
                    return self.fail_start(&snap, &closed).await;
                }
                self.lose_segment(&snap).await?;
                continue;
            }
            // A turn whose start the harness accepted waits for what the harness
            // says: there is nothing left to drive, and looking again does not
            // disturb the thread. A harness that took the turn and never says
            // that it began gets its own wait, counted from the latest moment the
            // command could have been answered (so that a restart counts the same),
            // and the turn is then given up as one that was not taken up in time.
            for attempt in repairs..=SESSION_REPAIRS {
                let Some(mut info) = self
                    .inner
                    .commands
                    .status(ids::turn_start(snap.turn.id, attempt))
                    .await?
                else {
                    continue;
                };
                if snap.turn.interrupted_by.is_some() {
                    return Ok(());
                }
                // Admission and link adoption are separate commits. A process
                // can stop between them while the machine link stays up.
                if info.stage == Stage::Queued {
                    let due = info.issued_at
                        + chrono::Duration::from_std(self.inner.config.start_timeout)
                            .unwrap_or_else(|_| chrono::Duration::zero());
                    if self.inner.clock.now() >= due {
                        self.inner
                            .commands
                            .cancel(ids::turn_start(snap.turn.id, attempt))
                            .await?;
                        info.outcome = Some(Outcome::Expired);
                    } else {
                        if !self
                            .reissue_admitted_start(&snap, ids::turn_start(snap.turn.id, attempt))
                            .await?
                        {
                            return Ok(());
                        }
                        if let Some(current) = self
                            .inner
                            .commands
                            .status(ids::turn_start(snap.turn.id, attempt))
                            .await?
                        {
                            info = current;
                        }
                    }
                }
                let outcome = match info.outcome {
                    Some(outcome) => outcome,
                    None => {
                        let due = info.issued_at
                            + chrono::Duration::from_std(self.inner.config.start_timeout)
                                .unwrap_or_else(|_| chrono::Duration::zero());
                        let left = (due - self.inner.clock.now()).to_std().unwrap_or_default();
                        match tokio::time::timeout(
                            left,
                            self.inner
                                .commands
                                .wait(ids::turn_start(snap.turn.id, attempt)),
                        )
                        .await
                        {
                            Ok(outcome) => outcome?,
                            Err(_) => {
                                self.inner
                                    .commands
                                    .cancel(ids::turn_start(snap.turn.id, attempt))
                                    .await?;
                                Outcome::Expired
                            }
                        }
                    }
                };
                let record: Option<Id> = sqlx::query_scalar(
                    "SELECT id FROM model_inputs WHERE node_id=$1 AND purpose='input' AND reference=$2 AND state IN ('prepared','uncertain')",
                )
                .bind(thread_id)
                .bind(snap.turn.id.to_string())
                .fetch_optional(&mut *self.inner.db.acquire().await?)
                .await
                .map_err(orch_db::DbError::from)?;
                if let Some(record) = record {
                    self.settle_model_input(thread_id, record, sent_of(&outcome))
                        .await;
                }
                if matches!(&outcome, Outcome::Succeeded(_)) {
                    self.forget_notices_said(&snap).await;
                    self.forget_cut_said(&snap).await;
                    let now = self.inner.clock.now();
                    if now >= self.started_due(info.issued_at)
                        && !self.still_hearing_start(
                            snap.key.machine,
                            thread_id,
                            snap.segment.id,
                            info.issued_at,
                            now,
                        )
                    {
                        return self.fail_start_as(&snap, never_began()).await;
                    }
                    return Ok(());
                }
                if matches!(&outcome, Outcome::Failed(_)) && snap.input.native_action.is_some() {
                    self.revalidate_native_start(&snap, true).await?;
                    return Ok(());
                }
                if matches!(&outcome, Outcome::Failed(failure)
                    if failure.known() == Some(ErrorCode::SessionNotFound))
                    && attempt < SESSION_REPAIRS
                {
                    repairs = attempt + 1;
                    lock(&self.inner.sessions).closed(snap.segment.id);
                    continue 'driving;
                }
                return self.fail_start(&snap, &outcome).await;
            }
            // The provider, where the turn goes through the forwarder: started on the machine
            // that keeps its key, unless the link's rows say that it was.
            if snap.path == CredentialPath::Forwarded {
                match self
                    .ensure_provider_started(thread_id, snap.provider.id)
                    .await?
                {
                    Keeping::Up => {}
                    Keeping::Failed(outcome) => {
                        return self
                            .fail_start_as(&snap, classify_keeper("provider.start", &outcome))
                            .await;
                    }
                    Keeping::Cannot(error) => {
                        return self.fail_start_as(&snap, classify_plan(&error)).await;
                    }
                }
            }
            // The directory the thread works in, where it is not made yet.
            self.make_thread_directory(&snap).await?;
            // The harness process.
            if !self.harness_is_online(snap.key).await? {
                self.set_step(thread_id, "harness").await?;
            }
            let started = self
                .ensure_harness(snap.key, &snap.agent, ids::agent_start(snap.turn.id))
                .await?;
            if let HarnessStarted::Failed(outcome) = started {
                let mut failure = classify(&outcome);
                // A first refused receipt must keep the binding reason of later preflights,
                // so correcting the profile pin can wake the original queued input.
                if let Some(pin) = snap
                    .agent
                    .profile
                    .get("harness_version")
                    .and_then(Json::as_str)
                    && let Some(report) = orch_db::queries::resources::thread_engine::harness(
                        &mut *self.inner.db.acquire().await?,
                        snap.key.provider,
                        snap.key.agent,
                        snap.key.machine,
                    )
                    .await?
                    && report
                        .harness_version
                        .as_deref()
                        .is_some_and(|version| version != pin)
                {
                    failure.blocked.reason = "binding_invalid";
                }
                return self.fail_start_as(&snap, failure).await;
            }
            // A credential that leaves the provider is given to the harness before its session opens
            // (the part that keeps providers).
            if matches!(snap.path, CredentialPath::Shared | CredentialPath::Handover) {
                self.set_step(thread_id, "credential").await?;
                if let crate::credentials::Given::Failed(failure) =
                    self.give_credential(&snap).await?
                {
                    return self.fail_start_as(&snap, *failure).await;
                }
            }
            // The session.
            // Adapters without live reload resume at the next turn with the changed offered set.
            let mcp_revision = self.mcp_revision(snap.environment.id).await?;
            let orch_tools_revision = snap.orch_tools_revision();
            let configuration = snap.session_configuration();
            // The skills the thread is given now. A program takes its skills when its session opens, so a change
            // reaches the thread by opening its session anew before this turn.
            let mut skills = self.skills_for_thread(thread_id).await?;
            if snap.descriptor.capabilities.skills
                == orch_agent_contract::descriptor::SkillsSupport::None
            {
                // A program that takes no skills is given none, and its screens say so.
                skills.skills.clear();
            }
            let skills_fingerprint = crate::skills::fingerprint(&skills);
            let side_unverified = !own_side_recovery
                && crate::side_threads::history::needs_verification(self, &snap).await?;
            let open = {
                let sessions = lock(&self.inner.sessions);
                snap.segment.native_session_ref.is_none()
                    || side_unverified
                    || !sessions.is_open(snap.segment.id, snap.key)
                    || sessions.instructions_changed(snap.segment.id, &snap.instructions().text)
                    || sessions.subagent_limits_changed(snap.segment.id, snap.settings.subagent_limits)
                    || sessions.configuration_changed(snap.segment.id,&configuration)
                    || sessions.mcp_changed(snap.segment.id, mcp_revision)
                    || sessions.orch_tools_changed(snap.segment.id, orch_tools_revision)
                    || sessions.skills_changed(snap.segment.id, &skills_fingerprint)
                    // A program that keeps a directory's files while its session is open takes up changes only
                    // when the session is opened again.
                    || (snap.descriptor.capabilities.session_reload
                        == orch_agent_contract::descriptor::SessionReload::Turn
                        && snap.thread.executor.is_none())
            };
            if open {
                self.set_step(thread_id, "session").await?;
                // A session that exists already (a person began it by hand) is opened as one that was adopted
                // the first time Orch opens it; one that Orch began is resumed.
                let ordinary = if snap.segment.native_session_ref.is_none() {
                    SessionMode::New
                } else if orch_db::queries::adoption::is_pending(
                    &mut *self.inner.db.acquire().await?,
                    snap.segment.id,
                )
                .await?
                {
                    SessionMode::Adopt
                } else {
                    SessionMode::Resume
                };
                // A side thread (the part that forks) begins its session from its parent's history.
                let recovering = match Box::pin(self.recover_agent_history(&mut snap)).await {
                    Ok(recovering) => recovering,
                    Err(EngineError::Unreadable(detail)) if snap.thread.side_of.is_some() => {
                        return self.hold_side_history(&snap, &detail).await;
                    }
                    Err(error) => return Err(error),
                };
                let recovering_history = recovering.is_some() || own_side_recovery;
                let (mode, import) = match if recovering_history {
                    Ok(recovering)
                } else {
                    self.side_session(&snap).await
                } {
                    Ok(Some(found)) => found,
                    Ok(None) => (ordinary, sessions::SessionImport::default()),
                    Err(EngineError::Unreadable(detail)) if snap.thread.side_of.is_some() => {
                        return self.hold_side_history(&snap, &detail).await;
                    }
                    Err(error) => return Err(error),
                };
                let mut body = sessions::session_open_body_importing(&snap, mode, &import)?;
                body.mcp = self.mcp_for_session(snap.environment.id, thread_id).await?;
                body.skills = crate::skills::session_open_skills(&skills);
                body.raw_after = orch_db::queries::raw_events::last_number(
                    &mut *self.inner.db.acquire().await?,
                    snap.segment.id,
                )
                .await?;
                if snap.descriptor.capabilities.instruction_root_marker
                    && body.exec.executor.is_none()
                {
                    let marker = Outgoing::new(
                        ids::instruction_root(snap.turn.id, repairs),
                        snap.environment.machine_id.ok_or_else(|| {
                            EngineError::unreadable("the environment has no instruction-root host")
                        })?,
                        SerialKey::Environment(snap.environment.id),
                        &FsMkdir {
                            environment_id: snap.environment.id,
                            path: orch_agent_contract::session::INSTRUCTION_ROOT_MARKER.to_owned(),
                        },
                    )?;
                    match crate::commands::run(self.inner.commands.as_ref(), marker).await? {
                        Outcome::Succeeded(_) => body.instruction_root_marker = true,
                        Outcome::Failed(ref failure)
                            if failure.code == "policy.remote_write_disabled" => {}
                        other => return self.fail_start(&snap, &other).await,
                    }
                }
                // Commit the source before sending an open: the connector can report immediately.
                if !orch_db::queries::machine_streams::bind_segment(
                    &mut *self.inner.db.acquire().await?,
                    thread_id,
                    snap.segment.id,
                    snap.key.machine,
                )
                .await?
                {
                    return Err(EngineError::unreadable(
                        "the segment belongs to another harness machine",
                    ));
                }
                let attempt = u32::try_from(snap.turn.ordinal)
                    .unwrap_or(0)
                    .saturating_mul(SESSION_ATTEMPTS)
                    + repairs;
                let opening = if recovering_history {
                    ids::session_open(snap.segment.id, attempt)
                } else {
                    crate::side_threads::history::opening_id(
                        &snap,
                        mode,
                        ids::session_open(snap.segment.id, attempt),
                    )
                };
                // What the session is opened with is kept before it is sent; nothing is sent where it cannot be.
                let record = self
                    .record_model_input(
                        &Delivery {
                            thread: thread_id,
                            segment: snap.segment.id,
                            turn: Some(snap.turn.id),
                            input: None,
                            purpose: "session",
                            reference: &opening.to_string(),
                        },
                        &snap.instruction_blocks(),
                    )
                    .await?;
                let tool_record = self
                    .record_model_input(
                        &crate::model_inputs::Delivery {
                            thread: thread_id,
                            segment: snap.segment.id,
                            turn: None,
                            input: None,
                            purpose: "tool",
                            reference: &format!("{opening}/tools"),
                        },
                        &snap.tool_blocks(),
                    )
                    .await?;
                let command = {
                    let mut conn = self.inner.db.acquire().await?;
                    crate::commands::session_open(
                        &mut conn,
                        opening,
                        snap.key.machine,
                        &body,
                        snap.segment.native_session_ref.as_ref().map(|native| {
                            orch_agent_contract::history::NativePrefixSource {
                                thread_id,
                                segment_id: snap.segment.id,
                                native_session_ref: native.clone(),
                            }
                        }),
                    )
                    .await?
                };
                let command = match if recovering_history {
                    if own_side_recovery {
                        self.admit_agent_history_side(&snap, command).await
                    } else {
                        Ok(command)
                    }
                } else {
                    crate::side_threads::history::saved(self, &snap, command).await
                } {
                    Ok(command) => command,
                    Err(EngineError::Unreadable(detail)) if snap.thread.side_of.is_some() => {
                        return self.hold_side_history(&snap, &detail).await;
                    }
                    Err(error) => return Err(error),
                };
                let accepted_configuration = crate::harnesses::SessionConfiguration::from(&body);
                let outcome =
                    crate::commands::run(self.inner.commands.as_ref(), command.clone()).await?;
                self.settle_model_input(thread_id, record, sent_of(&outcome))
                    .await;
                self.settle_model_input(thread_id, tool_record, sent_of(&outcome))
                    .await;
                match if recovering_history {
                    Ok(false)
                } else {
                    crate::side_threads::history::finished(self, &snap, &command, &outcome).await
                } {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(EngineError::Unreadable(detail)) if snap.thread.side_of.is_some() => {
                        return self.hold_side_history(&snap, &detail).await;
                    }
                    Err(error) => return Err(error),
                }
                match outcome {
                    Outcome::Succeeded(result) => {
                        let result: SessionOpenResult = serde_json::from_value(result)?;
                        // What an adopted session held before was read from its harness when it was adopted and is
                        // on the thread already, from the session before Orch: nothing is asked of it again here.
                        self.record_session_open(
                            &snap,
                            &result.native_session_ref,
                            &result.instruction_sources,
                            result.approval_reviewer.as_ref(),
                        )
                        .await?;
                        let mut sessions = lock(&self.inner.sessions);
                        sessions.mcp_requested(snap.segment.id, mcp_revision);
                        sessions.orch_tools_requested(snap.segment.id, orch_tools_revision);
                        sessions.configuration_accepted(snap.segment.id, accepted_configuration);
                        sessions.skills_requested(snap.segment.id, skills_fingerprint);
                    }
                    Outcome::Failed(ref failure)
                        if failure.known() == Some(ErrorCode::SessionNotFound)
                            && matches!(mode, SessionMode::Resume | SessionMode::Adopt)
                            && repairs < SESSION_REPAIRS =>
                    {
                        repairs += 1;
                        let closed = self.close_missing_agent_history(&snap).await?;
                        if !matches!(closed, Outcome::Succeeded(_)) {
                            return self.fail_start(&snap, &closed).await;
                        }
                        self.lose_segment(&snap).await?;
                        continue;
                    }
                    other => return self.fail_start(&snap, &other).await,
                }
            }
            // The lease, on the machine that keeps the key: after the session, before the turn.
            if let Some(lease) = &snap.lease
                && lease.mode == "forwarded"
                && lease.closed_at.is_none()
            {
                self.set_step(thread_id, "credential").await?;
                let protocol = snap.arrangement.protocol.clone();
                let mut outcome = self
                    .open_lease_at_provider(lease, &snap.provider, protocol.clone(), 0)
                    .await?;
                if provider_was_lost(&outcome) {
                    // The machine holds no such provider any more (its store was wiped, or the
                    // provider was stopped and switched on again): it is started once more, for
                    // this lease, and the lease is asked for again.
                    match self.restart_provider(snap.provider.id, lease.id).await? {
                        Keeping::Up => {
                            outcome = self
                                .open_lease_at_provider(lease, &snap.provider, protocol, 1)
                                .await?;
                        }
                        Keeping::Failed(failed) => {
                            return self
                                .fail_start_as(&snap, classify_keeper("provider.start", &failed))
                                .await;
                        }
                        Keeping::Cannot(error) => {
                            return self.fail_start_as(&snap, classify_plan(&error)).await;
                        }
                    }
                }
                if !matches!(outcome, Outcome::Succeeded(_)) {
                    return self
                        .fail_start_as(&snap, classify_keeper("lease.open", &outcome))
                        .await;
                }
            }
            if !self.revalidate_native_start(&snap, false).await? {
                return Ok(());
            }
            // The files of the input, written into the thread's directory before the turn begins.
            let Some(placement) = self.deliver_input_files(&snap).await? else {
                return Ok(());
            };
            // Copies have now been verified in this environment; their first-reading index can
            // accompany this very first input rather than waiting until the following turn.
            snap.material = {
                let mut conn = self.inner.db.acquire().await?;
                self.topic_material(&mut conn, thread_id).await?
            };
            // The turn. A plan is never worked as ordinary work: a program that lost plan mode since the
            // message was taken fails the turn instead, and the message can be sent again without it.
            if snap.turn.plan && !snap.descriptor.capabilities.plan_mode {
                return self.fail_start_as(&snap, no_plan_mode()).await;
            }
            self.set_step(thread_id, "turn").await?;
            // The turn records the skill versions it was given.
            self.record_turn_skills(snap.turn.id, &skills).await?;
            self.scope_agent_history_notices(&mut snap).await?;
            threads::notices::pin_delivery(
                &mut *self.db().acquire().await?,
                snap.thread.node_id,
                snap.turn.id,
                &snap.pending_notice_positions,
            )
            .await?;
            // A concurrent retry uses the first prepared page, and exact lane checks still apply.
            self.scope_agent_history_notices(&mut snap).await?;
            let (text, blocks) = self.turn_text(&snap, &placement).await?;
            let body = TurnStart {
                thread_id,
                turn_id: snap.turn.id,
                text: text.text,
                images: text.images,
                model: snap.turn.model.clone(),
                effort: snap.turn.effort.clone(),
                params: match snap.adapter_params()? {
                    Json::Object(members) => members,
                    _ => serde_json::Map::new(),
                },
                approval_mode: sessions::convert(&snap.approval)?,
                sandbox_mode: sessions::convert(&snap.sandbox)?,
                native_action: snap
                    .input
                    .native_action
                    .clone()
                    .map(serde_json::from_value)
                    .transpose()?,
                plan: snap.turn.plan,
            };
            let id = ids::turn_start(snap.turn.id, repairs);
            let fenced = snap.descriptor.capabilities.native_input_prefix.is_some() || {
                let mut conn = self.inner.db.acquire().await?;
                crate::commands::requires_fence(&mut conn, thread_id).await?
            };
            let command = if fenced {
                Outgoing::new(
                    id,
                    snap.key.machine,
                    SerialKey::Thread(thread_id),
                    &orch_link_contract::continuations::TurnStartFenced {
                        thread_id,
                        segment_id: snap.segment.id,
                        turn: Box::new(body),
                    },
                )?
            } else {
                Outgoing::new(id, snap.key.machine, SerialKey::Thread(thread_id), &body)?
            };
            // Whether the words name their author is kept before they are sent: once they are delivered, the next
            // words know whether names began.
            self.record_named(snap.input.id, snap.names_its_author())
                .await?;
            // What of where the agent is goes with it, given once the harness says the turn began.
            let reference = snap.turn.id.to_string();
            self.context_sending(snap.segment.id, &reference, &snap.context_lines())
                .await?;
            // What the model is given is kept before it is sent; nothing is sent where it cannot be.
            let record = self
                .record_model_input(
                    &Delivery {
                        thread: thread_id,
                        segment: snap.segment.id,
                        turn: Some(snap.turn.id),
                        input: Some(snap.input.id),
                        purpose: "input",
                        reference: &reference,
                    },
                    &blocks,
                )
                .await?;
            let command = match if own_side_recovery {
                self.admit_agent_history_side(&snap, command).await
            } else {
                crate::side_threads::history::save_first_turn(self, &snap, command).await
            } {
                Ok(command) => command,
                Err(EngineError::Unreadable(detail)) if snap.thread.side_of.is_some() => {
                    return self.hold_side_history(&snap, &detail).await;
                }
                Err(error) => return Err(error),
            };
            let Some(command) = self.admit_turn_start(&snap, command).await? else {
                self.settle_model_input(thread_id, record, Sent::Unknown)
                    .await;
                return Ok(());
            };
            self.inner.commands.issue(command).await?;
            let waited = tokio::time::timeout(
                self.inner.config.start_timeout,
                self.inner.commands.wait(id),
            )
            .await;
            let sent = match &waited {
                Ok(Ok(outcome)) => sent_of(outcome),
                Ok(Err(_)) | Err(_) => Sent::Unknown,
            };
            self.settle_model_input(thread_id, record, sent).await;
            match waited {
                Err(_elapsed) => {
                    self.inner.commands.cancel(id).await?;
                    return self.fail_start(&snap, &Outcome::Expired).await;
                }
                Ok(outcome) => match outcome? {
                    Outcome::Succeeded(_) => {
                        self.forget_notices_said(&snap).await;
                        self.forget_cut_said(&snap).await;
                        return Ok(());
                    }
                    Outcome::Failed(_) if snap.input.native_action.is_some() => {
                        self.revalidate_native_start(&snap, true).await?;
                        return Ok(());
                    }
                    Outcome::Failed(ref failure)
                        if failure.known() == Some(ErrorCode::SessionNotFound)
                            && repairs < SESSION_REPAIRS =>
                    {
                        // The harness holds no such session any longer: the next
                        // round opens it again.
                        repairs += 1;
                        lock(&self.inner.sessions).closed(snap.segment.id);
                    }
                    other => return self.fail_start(&snap, &other).await,
                },
            }
        }
    }

    /// Re-adopts a queued start from its original durable body and deadline.
    async fn reissue_admitted_start(
        &self,
        snap: &StartSnapshot,
        id: Id,
    ) -> Result<bool, EngineError> {
        let mut tx = self.db().begin().await?;
        let Some(thread) = threads::lock(&mut tx, snap.thread.node_id).await? else {
            return Ok(false);
        };
        let turn = threads::turn(&mut tx, snap.turn.id).await?;
        let input = threads::input(&mut tx, snap.input.id).await?;
        if thread.current_turn_id != Some(snap.turn.id)
            || thread.current_segment_id != Some(snap.segment.id)
            || thread.active_continuation_id != snap.input.continuation_id
            || thread.execution_revision != snap.thread.execution_revision
            || thread.deleted_at.is_some()
            || thread.archived_at.is_some()
            || turn.as_ref().is_none_or(|turn| {
                turn.thread_id != thread.node_id
                    || turn.segment_id != snap.segment.id
                    || turn.input_id != snap.input.id
                    || turn.state != turn_state::STARTING
                    || turn.interrupted_by.is_some()
            })
            || input.as_ref().is_none_or(|input| {
                input.thread_id != thread.node_id
                    || input.continuation_id != thread.active_continuation_id
                    || input.state != input_state::DELIVERING
                    || input.turn_id != Some(snap.turn.id)
            })
        {
            return Ok(false);
        }
        let Some(row) = orch_db::queries::link::command(&mut tx, id).await? else {
            return Ok(false);
        };
        if row.state != "queued" {
            return Ok(false);
        }
        let kind = match row.kind.as_str() {
            "turn.start" => {
                let body: TurnStart = serde_json::from_value(row.body.clone())?;
                if body.thread_id != thread.node_id
                    || body.turn_id != snap.turn.id
                    || body.model != snap.turn.model
                    || body.effort != snap.turn.effort
                    || body.plan != snap.turn.plan
                {
                    return Err(EngineError::unreadable(
                        "the retained turn start names another turn",
                    ));
                }
                orch_link_contract::commands::CommandKind::TurnStart
            }
            "turn.start.fenced" => {
                let body: orch_link_contract::continuations::TurnStartFenced =
                    serde_json::from_value(row.body.clone())?;
                if body.thread_id != thread.node_id
                    || body.segment_id != snap.segment.id
                    || body.turn.thread_id != thread.node_id
                    || body.turn.turn_id != snap.turn.id
                    || body.turn.model != snap.turn.model
                    || body.turn.effort != snap.turn.effort
                    || body.turn.plan != snap.turn.plan
                {
                    return Err(EngineError::unreadable(
                        "the retained turn start names another lane",
                    ));
                }
                orch_link_contract::commands::CommandKind::TurnStartFenced
            }
            _ => {
                return Err(EngineError::unreadable(
                    "the retained turn start has another kind",
                ));
            }
        };
        let key = SerialKey::Thread(thread.node_id);
        if row.machine_id != snap.key.machine || row.serial_key() != key.wire().as_deref() {
            return Err(EngineError::unreadable(
                "the retained turn start has another recipient",
            ));
        }
        let due = row.created_at
            + chrono::Duration::from_std(self.inner.config.start_timeout)
                .unwrap_or_else(|_| chrono::Duration::zero());
        let command = Outgoing {
            id,
            machine: row.machine_id,
            key,
            kind,
            body: row.body,
            deadline: Some(Time::new(row.deadline_at)),
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // Taking the lock may have consumed the remaining start time. The
        // next supervisor pass cancels the row without issuing it.
        if self.inner.clock.now() >= due {
            return Ok(false);
        }
        self.inner.commands.issue(command).await?;
        Ok(true)
    }

    /// Admits a start under the same thread lock as Stop, before it can be sent.
    async fn admit_turn_start(
        &self,
        snap: &StartSnapshot,
        mut command: Outgoing,
    ) -> Result<Option<Outgoing>, EngineError> {
        let mut tx = self.db().begin().await?;
        let Some(thread) = threads::lock(&mut tx, snap.thread.node_id).await? else {
            return Ok(None);
        };
        let turn = threads::turn(&mut tx, snap.turn.id).await?;
        let input = threads::input(&mut tx, snap.input.id).await?;
        if thread.current_turn_id != Some(snap.turn.id)
            || thread.current_segment_id != Some(snap.segment.id)
            || thread.active_continuation_id != snap.input.continuation_id
            || thread.deleted_at.is_some()
            || thread.archived_at.is_some()
            || turn.as_ref().is_none_or(|turn| {
                turn.state != turn_state::STARTING || turn.interrupted_by.is_some()
            })
            || input.as_ref().is_none_or(|input| {
                input.state != input_state::DELIVERING || input.turn_id != Some(snap.turn.id)
            })
        {
            return Ok(None);
        }
        let old = orch_db::queries::link::command(&mut tx, command.id).await?;
        let deadline = old.as_ref().map_or_else(
            || {
                Time::new(
                    self.inner.clock.now()
                        + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
                )
            },
            |row| Time::new(row.deadline_at),
        );
        command.deadline = Some(deadline);
        let key = command.key.wire();
        orch_db::queries::link::insert_command(
            &mut tx,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: deadline.get(),
            },
            self.inner.clock.now(),
        )
        .await?;
        let retained = orch_db::queries::link::command(&mut tx, command.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the admitted turn start disappeared"))?;
        if retained.machine_id != command.machine
            || retained.kind != command.kind.as_str()
            || retained.serial_key() != key.as_deref()
            || retained.body != command.body
        {
            return Err(EngineError::unreadable(
                "the admitted turn start changed identity",
            ));
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Some(command))
    }

    /// Rechecks the loaded selector before file delivery; a changed mapping preserves the waiting input.
    async fn hold_side_history(
        &self,
        snap: &StartSnapshot,
        detail: &str,
    ) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let Some(thread) = threads::lock(unit.conn, snap.thread.node_id).await? else {
            return Ok(());
        };
        let Some(turn) = threads::turn(unit.conn, snap.turn.id).await? else {
            return Ok(());
        };
        if thread.current_turn_id != Some(turn.id)
            || thread.current_segment_id != Some(snap.segment.id)
            || turn.thread_id != thread.node_id
            || turn.segment_id != snap.segment.id
            || turn.input_id != snap.input.id
            || thread.active_continuation_id != snap.input.continuation_id
            || thread.execution_revision != snap.thread.execution_revision
            || turn.state != turn_state::STARTING
            || turn.interrupted_by.is_some()
        {
            return Ok(());
        }
        self.end_turn(
            &mut unit,
            &thread,
            &turn,
            TurnEnd {
                state: turn_state::FAILED,
                error: Some(error_json("history_unavailable", detail, false)),
                input: InputAfter::Keep,
                then_block: Some(Blocked::new("error", detail)),
            },
        )
        .await?;
        let seq = unit.stamp(thread.node_id).await?;
        threads::return_input_in_place(unit.conn, snap.input.id, &[input_state::DELIVERING], seq)
            .await?;
        threads::hold_input(unit.conn, snap.input.id, Some("history_unavailable"), seq).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Rechecks the loaded selector before file delivery; a changed mapping preserves the waiting input.
    async fn revalidate_native_start(
        &self,
        snap: &StartSnapshot,
        rejected: bool,
    ) -> Result<bool, EngineError> {
        if snap.input.native_action.is_none() {
            return Ok(true);
        }
        if !rejected {
            self.native_actions(snap.thread.node_id, true).await?;
        }
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let Some(thread) = threads::lock(&mut *unit.conn, snap.thread.node_id).await? else {
            return Ok(false);
        };
        let Some(turn) = threads::turn(&mut *unit.conn, snap.turn.id).await? else {
            return Ok(false);
        };
        if turn.state != turn_state::STARTING {
            return Ok(false);
        }
        let Some(input) = threads::input(&mut *unit.conn, snap.input.id).await? else {
            return Ok(false);
        };
        if !rejected
            && !crate::native_actions::hold_invalid(&mut unit, thread.node_id, &input).await?
        {
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(true);
        }
        self.end_turn(&mut unit,&thread,&turn,TurnEnd{state:turn_state::FAILED,
            error:Some(error_json("native_action.unavailable","The selected command changed or is unavailable; refresh or remove that selection",false)),
            input:InputAfter::Keep,then_block:Some(Blocked { reason: "error", detail: Some("The selected command is waiting for its author".to_owned()) })}).await?;
        let seq = unit.stamp(thread.node_id).await?;
        threads::return_input_in_place(&mut *unit.conn, input.id, &[input_state::DELIVERING], seq)
            .await?;
        threads::hold_input(
            &mut *unit.conn,
            input.id,
            Some("native_action.unavailable"),
            seq,
        )
        .await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(false)
    }

    /// Makes the directory the thread works in, with those above it, in its environment: the harness is
    /// started in it (a process cannot start in a place that is not there), and the tools of a harness that
    /// runs elsewhere run in it. A directory that is there is taken for done. A machine that does not let
    /// Orch write in its workspace says so, and the turn goes on: the directory may be there already, and
    /// where it is not the harness says so itself.
    async fn make_thread_directory(&self, snap: &StartSnapshot) -> Result<(), EngineError> {
        let Some(machine) = snap.environment.machine_id else {
            return Ok(());
        };
        let path = sessions::workspace_path(&snap.thread.workspace_dir, &snap.settings.cwd);
        if path.is_empty() {
            return Ok(());
        }
        let command = Outgoing::new(
            ids::thread_directory(snap.turn.id),
            machine,
            SerialKey::Environment(snap.environment.id),
            &FsMkdir {
                environment_id: snap.environment.id,
                path,
            },
        )?;
        let outcome = crate::commands::run(self.inner.commands.as_ref(), command).await?;
        if !matches!(outcome, Outcome::Succeeded(_)) {
            tracing::info!(thread = %snap.thread.node_id, ?outcome, "the directory of the thread could not be made; the turn goes on");
        }
        Ok(())
    }

    /// The text a turn is started with, block by block in the order they go: where the agent is, what a side
    /// conversation is, why the turn before was cut, what waited, who sent the earlier words where names begin, and
    /// the input's own words. The text is the blocks joined; the record of what the model was given keeps them apart.
    async fn turn_text(
        &self,
        snap: &StartSnapshot,
        placement: &crate::delivery::Placement,
    ) -> Result<(instructions::InputText, Vec<ModelBlock>), EngineError> {
        let content: Vec<ContentPart> = serde_json::from_value(snap.input.content.clone())?;
        let (content, native_image_bytes) = {
            let mut conn = self.inner.db.acquire().await?;
            let content = self
                .copied_content_for_agent(&mut conn, snap.thread.node_id, &content)
                .await?;
            let bytes = orch_settings::get::<i64>(
                &mut conn,
                orch_settings::Place::Node(snap.thread.node_id),
                orch_settings_contract::registry::keys::FILES_NATIVE_IMAGE_BYTES,
            )
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("the native image size has no resolved value")
            })?;
            (content, bytes)
        };
        let parameters = snap.model_parameters()?;
        let images_pass = snap.descriptor.capabilities.input_images && parameters.takes_images();
        let attribute = snap.names_its_author();
        // Under `collab.attribution = never` the texts Orch composes name nobody.
        let names_people = snap.settings.attribution.names_people();
        let directory = snap.directory();
        let name = snap.author_name.clone();
        let mut text = instructions::input_text(
            &content,
            name.as_deref(),
            &InputRules {
                rendered_announcement: matches!(
                    snap.input.source.as_str(),
                    "announcement" | "goal"
                ),
                attribute_to: if attribute { name.as_deref() } else { None },
                names_people,
                images_pass,
                native_image_bytes: Some(native_image_bytes),
                directory: &directory,
                placement,
            },
        );
        let mut blocks: Vec<ModelBlock> = Vec::new();
        // Where the agent is goes first of all (T3): whole with a session's first input and after a compaction, and
        // otherwise what changed of it since it was last told. It is a message, not a new session.
        if let Some(block) =
            instructions::context_changes(&snap.context_lines(), &snap.context_sent)
        {
            blocks.push(ModelBlock::new(
                instructions::WHERE_YOU_ARE_KEY,
                "orch",
                block,
            ));
        }
        // A side conversation's first input is headed by what it is and that it only reads (T6).
        if let Some(reading) = snap.thread.side_reading.as_deref()
            && snap.turn.ordinal == 1
        {
            let block =
                instructions::side_reading_block(reading == "no_tools", &snap.settings.messages);
            blocks.push(ModelBlock::new(
                instructions::SIDE_READING_KEY,
                "orch",
                block,
            ));
        }
        // Why the turn before was cut, in the one line of its recorded cause (T8), once.
        if let Some((_, cause, by)) = &snap.cut
            && let Some(line) = instructions::cut_block(
                cause,
                by.as_deref().filter(|_| names_people),
                &snap.settings.messages,
            )
        {
            let key = instructions::cut_key(cause).unwrap_or(instructions::TURN_STOPPED_KEY);
            blocks.push(ModelBlock::new(key, "orch", line));
        }
        // What other agents announced, and what was shared, while the thread was not at work.
        let notices = snap
            .thread
            .pending_notices
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        if let Some(block) =
            instructions::pending_notices_block(notices, &snap.settings.messages, names_people)
        {
            blocks.push(ModelBlock::new(
                instructions::PENDING_NOTICES_KEY,
                "notices",
                block,
            ));
        }
        // Words that are the first to name their sender after words that named nobody are told, just before them,
        // who sent those.
        if attribute
            && let Some(block) = instructions::attribution_started_block(
                &snap.unnamed_before,
                &snap.settings.messages,
            )
        {
            blocks.push(ModelBlock::new(
                instructions::ATTRIBUTION_STARTED_KEY,
                "orch",
                block,
            ));
        }
        blocks.push(ModelBlock::new("input", "input", text.text.clone()));
        text.text = crate::model_inputs::joined(&blocks);
        Ok((text, blocks))
    }

    /// Notes, before a delivery is sent, where the agent is as it goes with it: it counts as given once the harness
    /// takes the delivery (a turn that began, a steer it took), and a delivery that fails leaves what was given before.
    async fn context_sending(
        &self,
        segment: Id,
        reference: &str,
        lines: &[(&'static str, String)],
    ) -> Result<(), EngineError> {
        let record = instructions::context_record(lines);
        let mut conn = self.inner.db.acquire().await?;
        threads::context_sending(&mut conn, segment, reference, &record["lines"]).await?;
        Ok(())
    }

    /// Keeps whether an input's words name their author, before they are sent: it counts once they are delivered.
    async fn record_named(&self, input: Id, named: bool) -> Result<(), EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        threads::set_input_named(&mut conn, input, named).await?;
        Ok(())
    }

    /// The turn was handed to the harness with the line of the cut before it: that cut is not told again. Where this
    /// is not reached the line is said with the next input, as the notices are.
    async fn forget_cut_said(&self, snap: &StartSnapshot) {
        let Some((turn, _, _)) = &snap.cut else {
            return;
        };
        let told = async {
            let mut conn = self.inner.db.acquire().await?;
            threads::cut_told(&mut conn, *turn).await?;
            Ok::<_, EngineError>(())
        };
        if let Err(error) = told.await {
            tracing::warn!(%error, thread = %snap.thread.node_id, "the line of a cut turn could not be marked as said");
        }
    }

    /// The turn was handed to the harness with the notices that waited for it said first: they are cleared, and
    /// those that came after the snapshot was read stay. Where this is not reached (the process stopped) the
    /// notices are said again at the next turn, which is better than not at all.
    async fn forget_notices_said(&self, snap: &StartSnapshot) {
        if let Err(error) = self.complete_agent_history_delivery(snap).await {
            tracing::warn!(%error, thread=%snap.thread.node_id, "delivered recovery will be recorded before its notice is cleared");
            return;
        }
        if snap.pending_notice_positions.is_empty() {
            return;
        }
        if let Err(error) = self
            .forget_pending_notice_positions(
                snap.thread.node_id,
                snap.turn.id,
                &snap.pending_notice_positions,
            )
            .await
        {
            tracing::warn!(%error, thread = %snap.thread.node_id, "notices that were said could not be cleared");
        }
    }

    /// Records that the session of a segment is open.
    async fn record_session_open(
        &self,
        snap: &StartSnapshot,
        native_ref: &str,
        reported: &[orch_agent_contract::session::InstructionFile],
        reviewer: Option<&orch_agent_contract::review::ApprovalReviewer>,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, snap.thread.node_id)
                .await?
                .is_some()
            {
                let seq = unit.stamp(snap.thread.node_id).await?;
                threads::set_native_session_ref(&mut *unit.conn, snap.segment.id, native_ref, seq)
                    .await?;
                // What the session was opened with, source by source: what the agent reads, for its readers.
                let current: Json =
                    sqlx::query_scalar("SELECT instruction_sources FROM segments WHERE id=$1")
                        .bind(snap.segment.id)
                        .fetch_one(&mut *unit.conn)
                        .await
                        .map_err(orch_db::DbError::from)?;
                let early_reports = current
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|source| source["evidence"] == "reported")
                    .cloned()
                    .collect::<Vec<_>>();
                let sources = crate::instruction_sources::merge(
                    &snap.instruction_sources(reported),
                    &early_reports,
                );
                threads::set_instruction_sources(&mut *unit.conn, snap.segment.id, &sources, seq)
                    .await?;
                // A session that was waiting to be adopted is adopted now.
                sqlx::query("UPDATE segments SET approval_reviewer=$2 WHERE id=$1")
                    .bind(snap.segment.id)
                    .bind(reviewer.map(serde_json::to_value).transpose()?)
                    .execute(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
                orch_db::queries::adoption::mark_opened(&mut *unit.conn, snap.segment.id).await?;
                self.complete_agent_history_recovery(&mut unit, snap, snap.segment.id, false)
                    .await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        let mut opened = lock(&self.inner.sessions);
        opened.opened(snap.segment.id, snap.thread.node_id, snap.key);
        opened.instructions_requested(snap.segment.id, &snap.instructions().text);
        opened.subagent_limits_requested(snap.segment.id, snap.settings.subagent_limits);
        Ok(())
    }

    /// The harness no longer holds the conversation of a segment: the
    /// segment ends and its successor reconstructs the conversation by the place's switch rule.
    async fn lose_segment(&self, snap: &StartSnapshot) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let thread_id = snap.thread.node_id;
            if let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? {
                let seq = unit.stamp(thread_id).await?;
                let now = unit.now();
                if threads::end_segment(&mut *unit.conn, snap.segment.id, "error", now, seq).await?
                {
                    let next = sessions::ensure_segment(
                        &mut unit,
                        &sessions::SegmentPlace {
                            thread: &thread,
                            agent: &snap.agent,
                            provider: &snap.provider,
                            environment: &snap.environment,
                            path: snap.path,
                            started_by: snap.input.author_id,
                        },
                        seq,
                    )
                    .await?;
                    let history = orch_settings::get::<String>(
                        unit.conn,
                        orch_settings::Place::Node(thread_id),
                        orch_settings_contract::registry::keys::THREAD_SWITCH_HISTORY,
                    )
                    .await?
                    .unwrap_or_else(|| "carry".into());
                    if history != "none" {
                        orch_db::queries::thread_history_cuts::capture(
                            unit.conn,
                            &orch_db::queries::thread_history_cuts::Capture {
                                id: next.id,
                                thread: thread_id,
                                view: thread.active_continuation_id,
                                job: None,
                                owner_thread: Some(thread_id),
                                segment: Some(snap.segment.id),
                                execution_revision: thread.execution_revision,
                                range: Some((1, snap.turn.ordinal.saturating_sub(1))),
                                at: now,
                            },
                        )
                        .await?;
                    }
                    orch_db::queries::agent_history::recovery(
                        unit.conn,
                        next.id,
                        &json!({"cut":(history!="none").then_some(next.id),"history":history,"source_segment":snap.segment.id}),
                    )
                    .await?;
                    // The turn belongs to the segment it was made in: it moves.
                    threads::move_turn_to_segment(&mut *unit.conn, snap.turn.id, next.id, seq)
                        .await?;
                }
            }
            lock(&self.inner.sessions).closed(snap.segment.id);
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// A starting turn that cannot be set up from what the database holds
    /// now (a profile gone, a sandbox that cannot be enforced any more) ends,
    /// and what the checks say of the thread now is its state. It is that
    /// turn that ends and no other: the thread may have gone on to another
    /// since it was looked at.
    async fn fail_unstartable(&self, thread_id: Id, turn_id: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(&mut *unit.conn, turn_id).await? else {
                return Ok(());
            };
            if turn.state != turn_state::STARTING || turn.interrupted_by.is_some() {
                return Ok(());
            }
            self.end_turn(
                &mut unit,
                &thread,
                &turn,
                TurnEnd {
                    state: turn_state::FAILED,
                    error: Some(error_json(
                        "harness_error",
                        "the turn can no longer be set up from what its thread is bound to",
                        true,
                    )),
                    input: InputAfter::Retry { retryable: true },
                    then_block: None,
                },
            )
            .await?;
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// A start that failed: the turn ends as failed, its input waits again if
    /// the failure may be retried, and the thread is blocked for the reason
    /// the failure says.
    async fn fail_start(&self, snap: &StartSnapshot, outcome: &Outcome) -> Result<(), EngineError> {
        self.fail_start_as(snap, classify(outcome)).await
    }

    /// As [`Engine::fail_start`], for a failure that was told in the words of a step.
    async fn fail_start_as(
        &self,
        snap: &StartSnapshot,
        failure: StartFailure,
    ) -> Result<(), EngineError> {
        tracing::warn!(thread = %snap.thread.node_id, turn = %snap.turn.id, code = failure.code, "a turn could not be started");
        let thread_id = snap.thread.node_id;
        let mut tx = self.inner.db.begin().await?;
        let (finished, ended) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(&mut *unit.conn, snap.turn.id).await? else {
                return Ok(());
            };
            // Only a turn that the harness has not confirmed is a turn whose
            // start failed: one that has begun is running, whatever became of
            // the command that asked for it.
            if turn.state != turn_state::STARTING || turn.interrupted_by.is_some() {
                return Ok(());
            }
            // A workspace's own harness configuration is a question, not an error of the thread's: asked once of
            // the people who manage the environment, while the thread waits for it; where the environment was
            // answered for meanwhile, the input is simply tried again.
            let asks = failure.code == crate::trust::REFUSAL;
            let then_block = if !asks {
                ingest::add_error_item(
                    &mut unit,
                    thread_id,
                    turn.segment_id,
                    Some(turn.id),
                    failure.code,
                    &failure.message,
                    failure.retryable,
                )
                .await?;
                Some(failure.blocked.clone())
            } else if crate::trust::ask(&mut unit, snap, &failure.message).await? {
                crate::trust::waiting(&mut *unit.conn, &snap.environment, &snap.agent.harness_kind)
                    .await?
                    .or_else(|| Some(failure.blocked.clone()))
            } else {
                unit.wake(thread_id);
                None
            };
            let ended = self
                .end_turn(
                    &mut unit,
                    &thread,
                    &turn,
                    TurnEnd {
                        state: turn_state::FAILED,
                        error: Some(error_json(
                            failure.code,
                            &failure.message,
                            failure.retryable,
                        )),
                        input: InputAfter::Retry {
                            retryable: failure.retryable,
                        },
                        then_block,
                    },
                )
                .await?;
            (unit.finish().await?, ended)
        };
        self.commit(tx, finished).await?;
        if ended.changed && failure.code == "start_timeout" {
            // The machine may yet start what it was asked to: ask it to stop.
            self.send_interrupt_best_effort(snap.key.machine, thread_id, snap.turn.id)
                .await;
        }
        Ok(())
    }

    /// Asks a harness to stop a turn the engine has given up on, without
    /// waiting for the answer.
    pub(crate) async fn send_interrupt_best_effort(&self, machine: Id, thread: Id, turn: Id) {
        let body = TurnInterrupt {
            thread_id: thread,
            turn_id: turn,
        };
        let command = Outgoing::new(
            ids::turn_interrupt(turn),
            machine,
            SerialKey::Thread(thread),
            &body,
        );
        match command {
            Ok(command) => {
                if let Err(error) = self.inner.commands.issue(command).await {
                    tracing::warn!(%error, %thread, "an interrupt could not be sent");
                }
            }
            Err(error) => tracing::warn!(%error, %thread, "an interrupt could not be made"),
        }
    }

    // ------------------------------------------------------------ steering

    /// Hands to the running turn the inputs that were steered into it, and
    /// settles them by how that went.
    pub(crate) async fn drive_steers(&self, thread_id: Id) -> Result<(), EngineError> {
        let (delivering, turn) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                threads::delivering_inputs(&mut conn, thread_id).await?,
                threads::active_turn(&mut conn, thread_id).await?,
            )
        };
        for input in delivering
            .into_iter()
            .filter(|input| input.delivery == "steer" && input.turn_id.is_some())
        {
            let live = turn
                .as_ref()
                .filter(|turn| Some(turn.id) == input.turn_id && turn.state == turn_state::RUNNING);
            match live {
                Some(turn) => self.steer_one(thread_id, turn, &input).await?,
                None => {
                    // The turn it was meant for is over, or never began running.
                    let still_starting = turn.as_ref().is_some_and(|turn| {
                        Some(turn.id) == input.turn_id && turn.state == turn_state::STARTING
                    });
                    if !still_starting {
                        self.finish_ended_steer(thread_id, &input).await?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Settles an ended turn's dispatched steer from its retained command after a restart.
    async fn finish_ended_steer(&self, thread_id: Id, input: &InputRow) -> Result<(), EngineError> {
        if input.state_detail.as_deref() != Some("steering") {
            return self.return_steer(thread_id, input.id).await;
        }
        let id = ids::turn_steer(input.id);
        let Some(info) = self.inner.commands.status(id).await? else {
            // The process stopped after reserving the words but before it
            // issued the command, so the harness could not have received it.
            return self.return_steer(thread_id, input.id).await;
        };
        let outcome = match info.outcome {
            Some(outcome) => outcome,
            None => self.inner.commands.wait(id).await?,
        };
        let record: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM model_inputs WHERE node_id=$1 AND purpose='input' AND reference=$2",
        )
        .bind(thread_id)
        .bind(input.id.to_string())
        .fetch_optional(&mut *self.inner.db.acquire().await?)
        .await
        .map_err(orch_db::DbError::from)?;
        if let Some(record) = record {
            self.settle_model_input(thread_id, record, sent_of(&outcome))
                .await;
        }
        match (outcome, input.turn_id) {
            (Outcome::Succeeded(_), Some(turn)) => {
                self.steer_delivered(thread_id, turn, input.id).await
            }
            _ => self.return_steer(thread_id, input.id).await,
        }
    }

    async fn steer_one(
        &self,
        thread_id: Id,
        turn: &TurnRow,
        input: &InputRow,
    ) -> Result<(), EngineError> {
        if input.native_action.is_some() {
            self.native_actions(thread_id, true).await?;
            let mut tx = self.db().begin().await?;
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            threads::lock(&mut *unit.conn, thread_id).await?;
            if crate::native_actions::hold_invalid(&mut unit, thread_id, input).await? {
                let seq = unit.stamp(thread_id).await?;
                threads::return_input_in_place(
                    &mut *unit.conn,
                    input.id,
                    &[input_state::DELIVERING],
                    seq,
                )
                .await?;
                threads::hold_input(
                    &mut *unit.conn,
                    input.id,
                    Some("native_action.unavailable"),
                    seq,
                )
                .await?;
                let finished = unit.finish().await?;
                self.commit(tx, finished).await?;
                return Ok(());
            }
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
        }
        let snapshot = {
            let mut conn = self.inner.db.acquire().await?;
            let thread = threads::thread(&mut conn, thread_id).await?;
            let segment = threads::segment(&mut conn, turn.segment_id).await?;
            match (thread, segment) {
                (Some(thread), Some(segment)) => {
                    self.steer_facts(&mut conn, &thread, &segment, input)
                        .await?
                }
                _ => None,
            }
        };
        let Some(facts) = snapshot else {
            return self.return_steer(thread_id, input.id).await;
        };
        // The files of the input are written into the thread's directory before the steer is sent.
        let Some(placement) = self
            .deliver_files(&crate::delivery::Delivering {
                thread: thread_id,
                input: input.id,
                content: &facts.content,
                environment: &facts.environment,
                directory: &facts.directory,
                sender: input.author_id,
            })
            .await?
        else {
            return Ok(());
        };
        // A file that could not be written holds the input before anything is sent into the turn: it waits for
        // its author with each file named, as an input before its turn does.
        if self
            .hold_steer_for_files(thread_id, input, &facts.content, &placement)
            .await?
        {
            return Ok(());
        }
        let machine = facts.machine;
        let (content, native_image_bytes) = {
            let mut conn = self.inner.db.acquire().await?;
            let content = self
                .copied_content_for_agent(&mut conn, thread_id, &facts.content)
                .await?;
            let bytes = orch_settings::get::<i64>(
                &mut conn,
                orch_settings::Place::Node(thread_id),
                orch_settings_contract::registry::keys::FILES_NATIVE_IMAGE_BYTES,
            )
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("the native image size has no resolved value")
            })?;
            (content, bytes)
        };
        // What changed of where the agent is since it was last told goes with the steer too, first of all.
        let place = {
            let mut conn = self.inner.db.acquire().await?;
            match self
                .load_turn_snapshot(&mut conn, thread_id, threads::turn_state::RUNNING)
                .await?
            {
                StartLoad::Ready(snap) => Some(*snap),
                StartLoad::NoTurn | StartLoad::Unstartable(_) => None,
            }
        };
        let images_pass = match &place {
            Some(snap) => {
                snap.descriptor.capabilities.input_images && snap.model_parameters()?.takes_images()
            }
            None => false,
        };
        let text = instructions::input_text(
            &content,
            facts.name.as_deref(),
            &InputRules {
                rendered_announcement: matches!(input.source.as_str(), "announcement" | "goal"),
                attribute_to: if facts.attribute {
                    facts.name.as_deref()
                } else {
                    None
                },
                names_people: facts.names_people,
                images_pass,
                native_image_bytes: Some(native_image_bytes),
                directory: &facts.directory,
                placement: &placement,
            },
        );
        let named = facts.attribute && facts.name.is_some();
        let changed = place.as_ref().and_then(|snap| {
            instructions::context_changes(&snap.context_lines(), &snap.context_sent)
        });
        let mut blocks: Vec<ModelBlock> = Vec::new();
        if let Some(block) = &changed {
            blocks.push(ModelBlock::new(
                instructions::WHERE_YOU_ARE_KEY,
                "orch",
                block.clone(),
            ));
        }
        if let Some(block) = &facts.names_begin {
            blocks.push(ModelBlock::new(
                instructions::ATTRIBUTION_STARTED_KEY,
                "orch",
                block.clone(),
            ));
        }
        blocks.push(ModelBlock::new("input", "input", text.text));
        let body = TurnSteer {
            thread_id,
            turn_id: turn.id,
            text: crate::model_inputs::joined(&blocks),
            images: text.images,
            native_action: input
                .native_action
                .clone()
                .map(serde_json::from_value)
                .transpose()?,
        };
        // The words take their place in the timeline before they are sent, so that an answer that comes before
        // the acknowledgement never stands above them; an input that is no longer to be steered is not sent.
        if !self.reserve_steer(thread_id, turn, input.id).await? {
            return Ok(());
        }
        self.record_named(input.id, named).await?;
        if let (Some(snap), Some(_)) = (&place, &changed) {
            self.context_sending(
                snap.segment.id,
                &input.id.to_string(),
                &snap.context_lines(),
            )
            .await?;
        }
        // What the model is given is kept before it is sent; nothing is sent where it cannot be.
        let record = self
            .record_model_input(
                &Delivery {
                    thread: thread_id,
                    segment: turn.segment_id,
                    turn: Some(turn.id),
                    input: Some(input.id),
                    purpose: "input",
                    reference: &input.id.to_string(),
                },
                &blocks,
            )
            .await?;
        let id = ids::turn_steer(input.id);
        self.inner
            .commands
            .issue(Outgoing::new(
                id,
                machine,
                SerialKey::Thread(thread_id),
                &body,
            )?)
            .await?;
        let outcome = self.inner.commands.wait(id).await?;
        self.settle_model_input(thread_id, record, sent_of(&outcome))
            .await;
        match outcome {
            Outcome::Succeeded(_) => self.steer_delivered(thread_id, turn.id, input.id).await,
            _ => self.return_steer(thread_id, input.id).await,
        }
    }

    /// Where a steer goes and what it says.
    async fn steer_facts(
        &self,
        conn: &mut orch_db::Conn,
        thread: &ThreadRow,
        segment: &threads::SegmentRow,
        input: &InputRow,
    ) -> Result<Option<SteerFacts>, EngineError> {
        let (Some(provider_id), Some(environment_id)) = (thread.provider_id, thread.environment_id)
        else {
            return Ok(None);
        };
        let Some(provider) =
            orch_db::queries::resources::thread_engine::provider(conn, provider_id).await?
        else {
            return Ok(None);
        };
        let Some(environment) = environments::environment(conn, environment_id).await? else {
            return Ok(None);
        };
        let Some(machine) = crate::harnesses::machine_of(
            Some(segment.arrangement.as_str()),
            &provider,
            environment.machine_id,
        ) else {
            return Ok(None);
        };
        let settings = crate::resolved::load(conn, thread.node_id).await?;
        let content: Vec<ContentPart> = serde_json::from_value(input.content.clone())?;
        let content = self.prepare_pastes(conn, thread, &content).await?;
        let stored = serde_json::to_value(&content)?;
        if stored != input.content {
            sqlx::query("UPDATE thread_inputs SET content=$2 WHERE id=$1")
                .bind(input.id)
                .bind(stored)
                .execute(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        }
        let author = match input.author_id {
            Some(author) => threads::people(conn, &[author]).await?.into_iter().next(),
            None => None,
        };
        let authors = threads::count_delivering_authors(conn, thread.node_id).await?;
        let attribute = settings.attribution.applies(authors);
        let name = author
            .map(|person| orch_chat::written::person_name(&person.display_name, &person.username));
        // Words that are the first to name their sender after words that named nobody are told who sent those.
        let names_begin = if attribute && name.is_some() {
            let earlier = threads::unnamed_authors_before(conn, thread.node_id, input.id).await?;
            instructions::attribution_started_block(&earlier, &settings.messages)
        } else {
            None
        };
        let directory =
            sessions::absolute_directory(&environment.workspace_root, &thread.workspace_dir);
        Ok(Some(SteerFacts {
            machine,
            environment,
            content,
            name,
            attribute,
            names_people: settings.attribution.names_people(),
            names_begin,
            directory,
        }))
    }

    /// A steer reached the harness: the input is delivered, and its words
    /// are in the timeline.
    async fn steer_delivered(
        &self,
        thread_id: Id,
        turn: Id,
        input_id: Id,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, thread_id).await?.is_none() {
                return Ok(());
            }
            let seq = unit.stamp(thread_id).await?;
            let now = unit.now();
            let moved = threads::change_input(
                &mut *unit.conn,
                input_id,
                &[input_state::DELIVERING],
                &InputChange {
                    state: input_state::DELIVERED,
                    turn_id: Some(turn),
                    delivered_at: Some(now),
                    ..InputChange::default()
                },
                seq,
            )
            .await?;
            if moved
                && let (Some(input), Some(turn_row)) = (
                    threads::input(&mut *unit.conn, input_id).await?,
                    threads::turn(&mut *unit.conn, turn).await?,
                )
            {
                ingest::add_user_message(
                    &mut unit,
                    thread_id,
                    turn_row.segment_id,
                    Some(turn),
                    &input,
                )
                .await?;
                // What of where the agent is went with the steer is what it was given.
                crate::native_actions::invoked(&mut unit, &turn_row, input_id).await?;
                threads::context_given(&mut *unit.conn, turn_row.segment_id, &input_id.to_string())
                    .await?;
            }
            if moved {
                unit.wake(thread_id);
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Rechecks the author's authority for the running turn before native delivery.
    async fn steer_authorized(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        turn: &TurnRow,
        input: &InputRow,
    ) -> Result<bool, EngineError> {
        match self.check_input_author_driving(unit, thread, input).await {
            Ok(()) => {}
            Err(crate::inputs::InputError::Internal(error)) => return Err(error),
            Err(_) => return Ok(false),
        }
        if let Some(author) = input.author_id
            && crate::preflight::second_step_missing(unit.conn, author, thread.node_id).await?
        {
            return Ok(false);
        }
        let Some(caller) = crate::inputs::input_caller(unit, thread, input).await? else {
            return Ok(false);
        };
        match self
            .check_sender_for_turn(unit, &caller, thread, true, Some(turn))
            .await
        {
            Ok(()) => Ok(true),
            Err(crate::inputs::InputError::Internal(error)) => Err(error),
            Err(_) => Ok(false),
        }
    }

    /// Keeps a steer's words in the timeline before sending, under the thread's lock.
    async fn reserve_steer(
        &self,
        thread_id: Id,
        turn: &TurnRow,
        input_id: Id,
    ) -> Result<bool, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let (finished, steering) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(false);
            };
            let input = threads::input(&mut *unit.conn, input_id)
                .await?
                .filter(|input| {
                    input.state == input_state::DELIVERING && input.turn_id == Some(turn.id)
                });
            let mut steering = false;
            if let Some(input) = input {
                let seq = unit.stamp(thread_id).await?;
                // Once this fence was crossed, a replay must keep the original
                // decision: the command may already have reached the harness.
                let allowed = input.state_detail.as_deref() == Some("steering")
                    || Box::pin(self.steer_authorized(&mut unit, &thread, turn, &input)).await?;
                if allowed {
                    sqlx::query(
                        "UPDATE thread_inputs SET state_detail='steering',seq_updated=$2 WHERE id=$1",
                    )
                    .bind(input.id)
                    .bind(seq)
                    .execute(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
                    ingest::reserve_user_message(
                        &mut unit,
                        thread_id,
                        turn.segment_id,
                        Some(turn.id),
                        &input,
                    )
                    .await?;
                    steering = true;
                } else {
                    threads::return_input_in_place(
                        &mut *unit.conn,
                        input.id,
                        &[input_state::DELIVERING],
                        seq,
                    )
                    .await?;
                    unit.wake(thread_id);
                }
            }
            (unit.finish().await?, steering)
        };
        self.commit(tx, finished).await?;
        Ok(steering)
    }

    /// A steer could not be handed to its turn: the input waits again, first.
    async fn return_steer(&self, thread_id: Id, input_id: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, thread_id).await?.is_some() {
                let seq = unit.stamp(thread_id).await?;
                if threads::return_input_to_front(
                    &mut *unit.conn,
                    input_id,
                    &[input_state::DELIVERING],
                    seq,
                )
                .await?
                {
                    unit.wake(thread_id);
                }
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    // ------------------------------------------------------------ interrupting

    /// Stops the turn `turn_id` of a thread for a person who may drive it.
    /// A turn the harness has not yet taken is ended on the spot; one it
    /// runs is asked to stop, and ended by the engine if it does not within
    /// the wait that is configured.
    pub async fn interrupt(
        &self,
        caller: &Caller,
        thread_id: Id,
        turn_id: Id,
    ) -> Result<Turn, InterruptError> {
        let mut tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        self.interrupt_guarded(tx, principal, thread_id, turn_id)
            .await
    }

    /// As [`Engine::interrupt`], in the transaction of the API's action guard: `principal` was loaded in `tx`
    /// by that check once the request had come, before any thread was locked, and its caller and its time
    /// are the ones that stop the turn and hold what waits.
    pub async fn interrupt_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        turn_id: Id,
    ) -> Result<Turn, InterruptError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        let (finished, turn) = {
            let mut unit = Unit::new(&mut tx, now);
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Err(InterruptError::NotFound);
            };
            match nodes::check(
                &mut *unit.conn,
                &principal,
                thread_id,
                Permission::ThreadInteract,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            {
                Decision::Allowed { .. } => {}
                Decision::Refused(refusal) => {
                    return Err(InterruptError::Denied(Box::new(refusal)));
                }
            }
            let turn = threads::turn(&mut *unit.conn, turn_id)
                .await?
                .filter(|turn| turn.thread_id == thread_id)
                .ok_or(InterruptError::NotFound)?;
            let owner = threads::segment(unit.conn, turn.segment_id)
                .await?
                .ok_or(InterruptError::NotFound)?
                .continuation_id;
            if owner != thread.active_continuation_id {
                return Err(InterruptError::NotRunning {
                    current_turn_id: thread.current_turn_id,
                });
            }
            if !turn.is_active() {
                // A Stop of the turn that has just ended, with nothing newer begun, still holds what waits
                // and answers that turn as it ended; one of an older turn, or while another runs, changes
                // nothing and names the turn that stands.
                let current = threads::active_turn(&mut *unit.conn, thread_id).await?;
                let latest = threads::latest_turn(&mut *unit.conn, thread_id).await?;
                if current.is_some() || latest.as_ref().map(|latest| latest.id) != Some(turn.id) {
                    return Err(InterruptError::NotRunning {
                        current_turn_id: current.map(|current| current.id),
                    });
                }
                self.hold_queue(&mut unit, &thread, caller.user()).await?;
                let finished = unit.finish().await?;
                self.commit(tx, finished).await?;
                let mut conn = self.inner.db.acquire().await?;
                let people = crate::views::People::load(&mut conn, turn.interrupted_by).await?;
                return Ok(crate::views::turn_view(&turn, "", &people)?);
            }
            // Stop holds what waits before the harness is asked to stop, in this transaction.
            self.hold_queue(&mut unit, &thread, caller.user()).await?;
            let seq = unit.stamp(thread_id).await?;
            if turn.state == turn_state::STARTING {
                // If this turn never reaches the harness, its input returns to
                // the queue and must remain stopped as well.
                let held = sqlx::query("UPDATE thread_inputs SET queue_held=true,seq_updated=$3 WHERE id=$1 AND turn_id=$2 AND state='delivering' AND NOT queue_held")
                    .bind(turn.input_id).bind(turn.id).bind(seq)
                    .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                if held.rows_affected() > 0 {
                    threads::touch_queue(unit.conn, thread_id, seq).await?;
                }
            }
            // What cut the turn, for the line its next input is told (T8): a person's Stop.
            threads::set_interrupt_cause(&mut *unit.conn, turn.id, "person").await?;
            if threads::mark_interrupted_by(&mut *unit.conn, turn.id, caller.user(), seq).await? {
                crate::audit_facts::person(
                    &mut *unit.conn,
                    caller.user(),
                    thread_id,
                    orch_db::audit::name!("turn.interrupted"),
                    serde_json::json!({"turn":turn.id}),
                    now,
                )
                .await?;
            }
            self.withdraw_interrupted_harness(&mut unit, thread_id, turn.id)
                .await?;
            unit.wake(thread_id);
            let _ = thread;
            (unit.finish().await?, turn)
        };
        self.commit(tx, finished).await?;
        self.drive_interrupt_turn(thread_id, &turn).await?;
        let mut conn = self.inner.db.acquire().await?;
        let row = threads::turn(&mut conn, turn_id)
            .await?
            .ok_or(InterruptError::NotFound)?;
        let people = crate::views::People::load(&mut conn, row.interrupted_by).await?;
        let display = String::new();
        Ok(crate::views::turn_view(&row, &display, &people)?)
    }

    /// Sends the interrupt of a turn that was asked to stop, or ends it where
    /// the harness never took it; ends it by force once the wait is over.
    pub(crate) async fn drive_interrupt(&self, thread_id: Id) -> Result<(), EngineError> {
        let turn = {
            let mut conn = self.inner.db.acquire().await?;
            threads::active_turn(&mut conn, thread_id).await?
        };
        match turn {
            Some(turn) if turn.interrupted_by.is_some() => {
                self.drive_interrupt_turn(thread_id, &turn).await
            }
            _ => Ok(()),
        }
    }

    async fn drive_interrupt_turn(&self, thread_id: Id, turn: &TurnRow) -> Result<(), EngineError> {
        let machine = {
            let mut conn = self.inner.db.acquire().await?;
            self.harness_machine_of(&mut conn, thread_id).await?
        };
        let start = if turn.state == turn_state::STARTING {
            self.start_progress(turn).await?
        } else {
            None
        };
        // A turn the harness has not taken is ended here and now.
        if turn.state == turn_state::STARTING {
            let taken = start
                .as_ref()
                .is_some_and(|info| matches!(info.stage, Stage::Accepted | Stage::Over));
            if !taken {
                for attempt in 0..=SESSION_REPAIRS {
                    self.inner
                        .commands
                        .cancel(ids::turn_start(turn.id, attempt))
                        .await?;
                }
                return self.end_interrupted(thread_id, turn.id, None).await;
            }
        }
        let Some(machine) = machine else {
            return self
                .end_interrupted(thread_id, turn.id, Some("forced"))
                .await;
        };
        let id = ids::turn_interrupt(turn.id);
        let known = self.inner.commands.status(id).await?;
        let issued_at = match known {
            Some(info) => info.issued_at,
            None => {
                self.send_interrupt_best_effort(machine, thread_id, turn.id)
                    .await;
                return Ok(());
            }
        };
        let due = issued_at
            + chrono::Duration::from_std(self.inner.config.interrupt_force)
                .unwrap_or_else(|_| chrono::Duration::zero());
        if self.inner.clock.now() >= due {
            self.end_interrupted(thread_id, turn.id, Some("forced"))
                .await?;
            // Closing this native session has no preservation proof for an earlier turn's retained
            // commands. Keep it open; pre-flight's native-busy guard awaits its real completion.
        }
        Ok(())
    }

    /// The progress of the command that starts a turn, the latest attempt.
    async fn start_progress(&self, turn: &TurnRow) -> Result<Option<Info>, EngineError> {
        let mut latest = None;
        for attempt in 0..=SESSION_REPAIRS {
            if let Some(info) = self
                .inner
                .commands
                .status(ids::turn_start(turn.id, attempt))
                .await?
            {
                latest = Some(info);
            }
        }
        Ok(latest)
    }

    /// The machine the harness of a thread runs on.
    pub(crate) async fn harness_machine_of(
        &self,
        conn: &mut orch_db::Conn,
        thread_id: Id,
    ) -> Result<Option<Id>, EngineError> {
        let Some(thread) = threads::thread(conn, thread_id).await? else {
            return Ok(None);
        };
        let (Some(provider_id), Some(environment_id)) = (thread.provider_id, thread.environment_id)
        else {
            return Ok(None);
        };
        let provider =
            orch_db::queries::resources::thread_engine::provider(conn, provider_id).await?;
        let environment = environments::environment(conn, environment_id).await?;
        Ok(match (provider, environment) {
            (Some(provider), Some(environment)) => crate::harnesses::machine_of(
                thread.arrangement.as_deref(),
                &provider,
                environment.machine_id,
            ),
            _ => None,
        })
    }

    /// Ends a turn that was asked to stop. With a code the engine did it
    /// itself, because the harness did not answer; without, the turn never
    /// reached the harness.
    async fn end_interrupted(
        &self,
        thread_id: Id,
        turn_id: Id,
        forced: Option<&'static str>,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(&mut *unit.conn, turn_id).await? else {
                return Ok(());
            };
            if !turn.is_active() {
                return Ok(());
            }
            let error = forced.map(|code| {
                error_json(
                    code,
                    "the harness did not stop when asked, so the turn was ended without it",
                    false,
                )
            });
            if let Some(code) = forced {
                ingest::add_error_item(
                    &mut unit,
                    thread_id,
                    turn.segment_id,
                    Some(turn.id),
                    code,
                    "The harness did not stop when asked; the turn was ended without it.",
                    false,
                )
                .await?;
            }
            let resource_pause: bool = sqlx::query_scalar(
                "SELECT COALESCE(interrupt_cause='resource_disabled',false) FROM turns WHERE id=$1",
            )
            .bind(turn.id)
            .fetch_one(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
            self.end_turn(
                &mut unit,
                &thread,
                &turn,
                TurnEnd {
                    state: turn_state::INTERRUPTED,
                    error,
                    input: if turn.state == turn_state::STARTING {
                        InputAfter::Return
                    } else if resource_pause {
                        InputAfter::Keep
                    } else {
                        InputAfter::Retry { retryable: true }
                    },
                    // A controller deadline is not proof that native work stopped.
                    then_block: forced.map(|_| Blocked {
                        reason: "error",
                        detail: Some(crate::background::NATIVE_BUSY.to_owned()),
                    }),
                },
            )
            .await?;
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    // ------------------------------------------------------------ presence

    /// A machine lost its session: the threads whose turn runs on it are
    /// blocked as `agent_offline`, and the turn stays as it is until events
    /// resume or somebody interrupts. The directories of people's machines
    /// that are on it stand `offline` from the next sweep.
    pub async fn machine_offline(&self, machine: Id) -> Result<(), EngineError> {
        lock(&self.inner.sessions).machine_lost(machine);
        // The directories on it are looked at now: they stand `offline`.
        self.kick();
        let affected = {
            let mut conn = self.inner.db.acquire().await?;
            threads::active_threads_of_machine(&mut conn, machine).await?
        };
        for thread_id in affected.iter().copied() {
            self.block_as_offline(thread_id).await?;
        }
        self.tell_machine_offline(machine, &affected).await
    }

    /// Blocks one thread as offline, where its turn is running or waits for an
    /// approval; the turn stays as it is.
    pub(crate) async fn block_as_offline(&self, thread_id: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, thread_id).await?.is_some() {
                let seq = unit.stamp(thread_id).await?;
                crate::notifications::set_state(
                    &mut unit,
                    thread_id,
                    &[thread_state::RUNNING, thread_state::WAITING_APPROVAL],
                    &threads::StateChange::blocked(
                        "agent_offline",
                        Some("the machine of the harness is offline"),
                    ),
                    seq,
                )
                .await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// A machine has a session again: what was blocked for want of it is
    /// looked at, and a turn that was under way goes on as it was.
    pub async fn machine_online(&self, machine: Id) -> Result<(), EngineError> {
        // The directories on it are looked at now: they are attached in its new session.
        self.kick();
        let blocked = {
            let mut conn = self.inner.db.acquire().await?;
            threads::blocked_threads_of_machine(&mut conn, machine).await?
        };
        for thread_id in blocked {
            self.restore_after_offline(thread_id).await?;
            self.wake(thread_id);
        }
        Ok(())
    }

    /// Re-evaluates all bindings held by a physical connector/node update, including hosted units.
    pub async fn component_update_finished(&self, machine: Id) -> Result<(), EngineError> {
        let threads = {
            let mut conn = self.inner.db.acquire().await?;
            orch_db::queries::component_updates::waiting(&mut conn, machine).await?
        };
        for thread in threads {
            self.wake(thread);
        }
        self.kick();
        Ok(())
    }

    /// Puts a thread whose turn was under way back in the state the turn is
    /// in.
    pub(crate) async fn restore_after_offline(&self, thread_id: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? {
                self.restore_in(&mut unit, &thread).await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// The same, inside a transaction.
    pub(crate) async fn restore_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<(), EngineError> {
        if thread.state != thread_state::BLOCKED
            || thread.blocked_reason.as_deref() != Some("agent_offline")
        {
            return Ok(());
        }
        let Some(turn) = threads::active_turn(&mut *unit.conn, thread.node_id).await? else {
            return Ok(());
        };
        let waiting =
            approval_queries::count_pending_of_thread(&mut *unit.conn, thread.node_id).await? > 0;
        let (to, step) = match (turn.state.as_str(), waiting) {
            (turn_state::STARTING, _) => (thread_state::STARTING, Some("turn")),
            (_, true) => (thread_state::WAITING_APPROVAL, None),
            _ => (thread_state::RUNNING, None),
        };
        let seq = unit.stamp(thread.node_id).await?;
        threads::set_state(
            &mut *unit.conn,
            thread.node_id,
            &[thread_state::BLOCKED],
            &threads::StateChange {
                state: to,
                reason: None,
                detail: None,
                step,
            },
            seq,
        )
        .await?;
        Ok(())
    }

    // ------------------------------------------------------------ supervision

    /// Looks at a thread and does what is outstanding for it.
    pub(crate) async fn reconcile(&self, thread_id: Id) -> Result<(), EngineError> {
        if let Err(error) = self.drive_turn_end_work(thread_id).await {
            // Accounting/notification trouble must not prevent a previously closed
            // lease from being retired on its native keeper. Both remain durable.
            self.drive_lease_closes(thread_id).await?;
            return Err(error);
        }
        // The end of the last turn first: the next one waits for it.
        self.drive_end_observation(thread_id).await?;
        // The goal of the thread, if it has one, counts what the turn used and queues the next turn.
        self.drive_goal(thread_id).await?;
        self.recheck_waiting(thread_id).await?;
        self.drive_preparation(thread_id).await?;
        // A start includes the whole session setup; reconciliation should not hold it inline on the worker stack.
        Box::pin(self.drive_start(thread_id)).await?;
        self.drive_steers(thread_id).await?;
        self.drive_interrupt(thread_id).await?;
        self.drive_approvals(thread_id).await?;
        self.drive_lease_closes(thread_id).await?;
        Ok(())
    }

    /// Runs pre-flight for a thread that has inputs waiting and is idle, or
    /// blocked for a reason that can clear by itself. A thread blocked by an
    /// error waits for a person to retry or to send another input.
    async fn recheck_waiting(&self, thread_id: Id) -> Result<(), EngineError> {
        let worth = {
            let mut conn = self.inner.db.acquire().await?;
            match threads::thread(&mut conn, thread_id).await? {
                Some(row) => match row.state.as_str() {
                    thread_state::IDLE => {
                        threads::count_pending_inputs(&mut conn, thread_id).await? > 0
                    }
                    // A thread blocked for a reason that can clear is looked at even
                    // when nothing waits any more: it is then idle, unless the cause
                    // is the thread's own (its binding, its place) and still there.
                    thread_state::BLOCKED => {
                        let recovered_source = row.blocked_reason.as_deref() == Some("error")
                            && row.blocked_detail.as_deref()
                                == Some(crate::preparation::SOURCE_FAILED)
                            && crate::source::ready(&row)?;
                        (row.blocked_reason.as_deref() != Some("error") || recovered_source)
                            && row.current_turn_id.is_none()
                    }
                    _ => false,
                },
                None => false,
            }
        };
        if worth {
            self.preflight(thread_id).await?;
        }
        Ok(())
    }

    /// Whether the machine that keeps the key is still to be told that a lease it was asked to open is
    /// closed: it was asked to open it, and has not been told yet. A lease that was never opened there
    /// (its turn failed before) is nothing to the machine.
    pub(crate) async fn lease_needs_close(
        &self,
        lease: &threads::LeaseRow,
    ) -> Result<bool, EngineError> {
        // A stored closing answer may outlive the process waiting for it. Replay its totals
        // until the ledger carries the reconciliation marker.
        if let Some(info) = self
            .inner
            .commands
            .status(ids::lease_close(lease.id))
            .await?
        {
            if let Some(crate::commands::Outcome::Succeeded(answer)) = info.outcome
                && answer.get("usage").is_some_and(|usage| !usage.is_null())
            {
                let mut conn = self.inner.db.acquire().await?;
                return Ok(!orch_db::usage::ledger::event_exists(
                    &mut conn,
                    ids::usage_close(lease.id),
                )
                .await?);
            }
            return Ok(false);
        }
        for attempt in 0..=LEASE_OPEN_ATTEMPTS {
            if self
                .inner
                .commands
                .status(ids::lease_open(lease.id, attempt))
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Tells the provider's side that the leases of the thread which were closed are, a page at a time through the
    /// window.
    pub(crate) async fn drive_lease_closes(&self, thread_id: Id) -> Result<(), EngineError> {
        let mut after = (
            crate::config::decisions_since(self.inner.clock.now()),
            Id::nil(),
        );
        loop {
            let page = {
                let mut conn = self.inner.db.acquire().await?;
                threads::leases_closed_after(
                    &mut conn,
                    Some(thread_id),
                    after,
                    self.inner.config.sweep_batch,
                )
                .await?
            };
            let full =
                i64::try_from(page.len()).unwrap_or(i64::MAX) >= self.inner.config.sweep_batch;
            if let Some(last) = page.last() {
                after = (last.closed_at.unwrap_or(after.0), last.id);
            }
            self.close_leases_told(page).await?;
            if !full {
                return Ok(());
            }
        }
    }

    /// Tells the provider's side of each of these closed leases that needs it.
    async fn close_leases_told(&self, closed: Vec<threads::LeaseRow>) -> Result<(), EngineError> {
        // A lease that let a credential leave is closed at the provider's side as well; one that was never
        // opened there (a thread in an environment that holds the login already) is not told of.
        for lease in closed {
            if self.lease_needs_close(&lease).await? {
                let provider = {
                    let mut conn = self.inner.db.acquire().await?;
                    orch_db::queries::resources::thread_engine::provider(
                        &mut conn,
                        lease.provider_id,
                    )
                    .await?
                };
                if let Some(provider) = provider {
                    self.send_lease_close(&lease, &provider).await?;
                }
            }
        }
        Ok(())
    }
}

/// Why a turn could not be interrupted.
#[derive(Debug, thiserror::Error)]
pub enum InterruptError {
    /// There is no such thread, or no such turn in it.
    #[error("there is no such thread or turn")]
    NotFound,
    /// The turn is over and is not the one that just ended, or another turn runs now.
    #[error("the turn is not running")]
    NotRunning {
        /// The turn that runs now, if one does: a stale Stop never stops it.
        current_turn_id: Option<Id>,
    },
    /// The caller may not drive the thread.
    #[error("the caller may not drive the thread")]
    Denied(Box<Refusal>),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for InterruptError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for InterruptError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// Why a start failed, in the words a turn, an item and a thread carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartFailure {
    /// The code of the turn's error.
    pub code: &'static str,
    /// A sentence for the timeline.
    pub message: String,
    /// Whether the same input may succeed if it is sent again.
    pub retryable: bool,
    /// What the thread is blocked as.
    pub blocked: Blocked,
}

/// What a command that did not succeed means for the turn it was for.
#[must_use]
pub fn classify(outcome: &Outcome) -> StartFailure {
    let failure =
        |code: &'static str, message: String, retryable: bool, reason: &'static str| StartFailure {
            code,
            blocked: Blocked {
                reason,
                detail: Some(message.clone()),
            },
            message,
            retryable,
        };
    match outcome {
        Outcome::Succeeded(_) => failure(
            "harness_error",
            "the command succeeded".to_owned(),
            false,
            "error",
        ),
        Outcome::Expired => failure(
            "start_timeout",
            "the turn was not taken up in time".to_owned(),
            true,
            "error",
        ),
        Outcome::Cancelled => failure(
            "environment_unreachable",
            "the machine could not be asked: it was revoked or the request was withdrawn"
                .to_owned(),
            false,
            "agent_offline",
        ),
        Outcome::Failed(error) => {
            let message = format!("{}: {}", error.code, error.detail);
            if error.code == "environment_unreachable" {
                return failure("environment_unreachable", message, error.retryable, "error");
            }
            match error.known() {
                Some(ErrorCode::Timeout) => failure("start_timeout", message, true, "error"),
                Some(ErrorCode::OfflinePeer) => {
                    failure("environment_unreachable", message, true, "agent_offline")
                }
                Some(ErrorCode::HarnessUnavailable) => {
                    failure("harness_error", message, false, "binding_invalid")
                }
                // The workspace holds a configuration of the harness's own that nobody has answered for: no error of
                // the thread's, but a question for the people who manage the environment, while the input waits.
                Some(ErrorCode::WorkspaceConfiguration) => failure(
                    crate::trust::REFUSAL,
                    error.detail.clone(),
                    true,
                    crate::trust::WAITING,
                ),
                Some(ErrorCode::LoginRequired | ErrorCode::CredentialUnusable) => failure(
                    "provider_unauthorized",
                    message,
                    false,
                    "provider_unavailable",
                ),
                Some(
                    ErrorCode::PolicyRootNotAllowed
                    | ErrorCode::PolicyAutoApprovalNotAllowed
                    | ErrorCode::PolicyUpstreamNotAllowed
                    | ErrorCode::PolicyArrangementNotAllowed
                    | ErrorCode::PolicyExecNotOffered,
                ) => failure("harness_error", message, false, "binding_invalid"),
                _ => failure("harness_error", message, error.retryable, "error"),
            }
        }
    }
}

/// How long a start is waited for.
#[must_use]
pub fn start_wait(config: &crate::config::EngineConfig) -> Duration {
    config.start_timeout
}

/// A planning turn on a program that no longer declares plan mode: not retried, since it would fail again.
#[must_use]
pub fn no_plan_mode() -> StartFailure {
    let message = "the agent program has no plan mode now, so the plan was not started".to_owned();
    StartFailure {
        code: "harness_error",
        blocked: Blocked {
            reason: "error",
            detail: Some(message.clone()),
        },
        message,
        retryable: false,
    }
}

/// A turn its harness took and never began: given up as a start that was not taken up in time, which may pass,
/// so its input waits again and the thread says what happened.
#[must_use]
pub fn never_began() -> StartFailure {
    let message =
        "the harness took the turn but did not begin it in time, so it was given up".to_owned();
    StartFailure {
        code: "start_timeout",
        blocked: Blocked {
            reason: "error",
            detail: Some(message.clone()),
        },
        message,
        retryable: true,
    }
}

impl Engine {
    /// When a turn whose `turn.start` was issued at `issued_at` and answered must have begun: the latest moment
    /// the command could have been answered, and the wait for `turn.started` after it.
    fn started_due(
        &self,
        issued_at: chrono::DateTime<chrono::Utc>,
    ) -> chrono::DateTime<chrono::Utc> {
        let config = &self.inner.config;
        let wait = config.start_timeout.saturating_add(config.started_wait);
        issued_at + chrono::Duration::from_std(wait).unwrap_or_else(|_| chrono::Duration::zero())
    }

    /// Whether a turn whose start is due is waited for still, because the system has not taken in all that
    /// the machine of its harness said on its session (C-turns): its `turn.started` may be among it. Under
    /// load a hundred sessions' events were taken in a minute and more after they came, and turns their
    /// harness had begun at once were given up as never begun, their threads blocked. The wait ends all the
    /// same ten waits for `turn.started` past the due time: a session that far behind is stuck, not busy.
    fn still_hearing_start(
        &self,
        machine: Id,
        thread_id: Id,
        segment_id: Id,
        issued_at: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        let longest = chrono::Duration::from_std(self.inner.config.started_wait.saturating_mul(10))
            .unwrap_or_else(|_| chrono::Duration::zero());
        now < self.started_due(issued_at) + longest
            && self.inner.commands.hears_behind(
                machine,
                &orch_link_contract::streams::Stream::Session {
                    thread_id,
                    segment_id,
                },
            )
    }
}

// ---------------------------------------------------------------- Orch's own use of a model

/// A model call Orch made for itself, as the ledger counts it: a summary of a topic or of one of its threads.
#[derive(Debug, Clone)]
pub struct OwnUse<'a> {
    /// What the call was for.
    pub purpose: OwnPurpose,
    /// Boundaries observed by Orch's model client.
    pub timing: Option<&'a orch_api_contract::usage::RequestTiming>,
    /// The row's identifier, chosen from what the call is: a call counted twice is one row.
    pub id: Id,
    /// When it happened.
    pub at: chrono::DateTime<chrono::Utc>,
    /// The topic the summary is of, or whose thread it is of; for embeddings, the root org whose index they are.
    pub topic: Id,
    /// The thread, where the summary is of one.
    pub thread: Option<Id>,
    /// The provider that paid.
    pub provider: Id,
    /// The entry through which the person uses the provider.
    pub entry: Option<Id>,
    /// Whoever last wrote the summarizer's setting.
    pub person: Option<Id>,
    /// The model, by the name that was sent.
    pub model: &'a str,
    /// What it used.
    pub tokens: orch_db::usage::ledger::Tokens,
    /// The upstream's own name for the request.
    pub request_ref: Option<&'a str>,
}

/// What a model call Orch made for itself was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnPurpose {
    /// A title, a description, tags or a handoff: put down to its topic and thread.
    Summary,
    /// Vectors for search by meaning: put down to the root org whose index they are, which stands for its
    /// project and topic too.
    Embeddings,
}

impl Engine {
    /// Counts a model call Orch made for itself: one `request` row of the origin `orch` and its purpose that
    /// always counts, against the provider and the caps that are shared and never against a person's own, at
    /// the rate the model is counted at as a turn's would be. The answer says whether this call wrote the row.
    pub async fn count_own_use(
        &self,
        conn: &mut orch_db::Conn,
        own: &OwnUse<'_>,
    ) -> Result<bool, EngineError> {
        use orch_db::usage::ledger::{
            NewSummary, SummaryAttribution, purpose, record_own_measured, summary_place,
        };
        let (org, project, word) = match own.purpose {
            OwnPurpose::Summary => {
                let Some((org, project)) = summary_place(conn, own.topic).await? else {
                    return Ok(false);
                };
                (org, project, purpose::SUMMARY)
            }
            OwnPurpose::Embeddings => (own.topic, own.topic, purpose::EMBEDDINGS),
        };
        let rate = crate::caps::own_rate(conn, own.provider, own.model).await?;
        let book = crate::rates::RateBook::load(conn, own.thread.unwrap_or(own.topic)).await?;
        let priced = book
            .price(
                conn,
                rate.as_ref(),
                own.model,
                &own.tokens,
                Some(own.tokens.input),
                None,
            )
            .await?;
        let rate_json = priced
            .found
            .as_ref()
            .and_then(|found| serde_json::to_value(&found.rate).ok());
        let written = record_own_measured(
            conn,
            word,
            &NewSummary {
                id: own.id,
                at: own.at,
                attribution: &SummaryAttribution {
                    org_id: org,
                    project_id: project,
                    topic_id: own.topic,
                    thread_id: own.thread,
                    provider_id: own.provider,
                    entry_id: own.entry,
                    user_id: own.person,
                },
                model: own.model,
                tier: None,
                tokens: own.tokens,
                context_tokens: Some(own.tokens.input),
                request_ref: own.request_ref,
                rate: rate_json.as_ref(),
                rate_source: priced.found.as_ref().map(|found| found.source.word()),
                credits: priced.credits.as_deref(),
            },
            own.timing,
        )
        .await?;
        if written && let Some(timing) = own.timing {
            crate::providers::record_model_metrics(own.model, None, timing);
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::commands::CommandError;

    #[test]
    fn a_start_that_was_not_taken_up_in_time_is_retryable_and_blocks_as_an_error() {
        let failure = classify(&Outcome::Expired);
        assert_eq!(failure.code, "start_timeout");
        assert!(failure.retryable);
        assert_eq!(failure.blocked.reason, "error");
    }

    #[test]
    fn what_a_machine_refuses_by_policy_is_a_binding_that_does_not_hold() {
        let refused = Outcome::Failed(CommandError::new(
            ErrorCode::PolicyAutoApprovalNotAllowed,
            "no",
        ));
        let failure = classify(&refused);
        assert_eq!(failure.blocked.reason, "binding_invalid");
        assert!(!failure.retryable);
        let login = Outcome::Failed(CommandError::new(ErrorCode::LoginRequired, "sign in"));
        assert_eq!(classify(&login).code, "provider_unauthorized");
        assert_eq!(classify(&login).blocked.reason, "provider_unavailable");
    }

    #[test]
    fn a_machine_that_cannot_be_reached_blocks_the_thread_as_offline() {
        let gone = Outcome::Failed(CommandError::new(ErrorCode::OfflinePeer, "away"));
        assert_eq!(classify(&gone).blocked.reason, "agent_offline");
        assert_eq!(
            classify(&Outcome::Cancelled).blocked.reason,
            "agent_offline"
        );
    }

    #[test]
    fn an_unreachable_tool_environment_keeps_its_reason_and_retryability() {
        let mut refusal =
            CommandError::new(ErrorCode::Internal, "the tool connection failed").retryable();
        refusal.code = "environment_unreachable".into();
        let failure = classify(&Outcome::Failed(refusal));
        assert_eq!(failure.code, "environment_unreachable");
        assert!(failure.message.contains("the tool connection failed"));
        assert!(failure.retryable);
        assert_eq!(failure.blocked.reason, "error");
    }
}
