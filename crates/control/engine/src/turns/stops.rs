//! A person's accepted Stop is retained independently of its checked thread write and cancellation work.
//!
//! Recording acceptance takes no thread row and shares no insert key with another request. One worker per
//! thread applies it under the row; ordinary writers apply it before their own work too. The request observes
//! progress briefly, but never owns or cancels that worker's transaction. A slow writer cannot delay the answer.
//!
//! A preparation is bound to its input and execution history, including when that input becomes a native
//! running turn before application. Giving up a preparation holds its input and retains the obligation to
//! cancel its reads and commands. Whichever writer applies it, that obligation survives a restart and runs
//! outside the thread row before a resumed preparation may begin.

use super::*;
use chrono::{DateTime, Utc};

/// A bounded chance to include already-applied state; its worker keeps running after the answer.
const APPLICATION_WAIT: Duration = Duration::from_millis(300);

/// The pause before the engine looks again for the row of a thread whose Stop it could not carry out.
const RETRY_PAUSE: Duration = Duration::from_secs(1);

/// What a Stop was answered with.
#[derive(Debug, Clone)]
pub struct Stopped {
    /// The turn it names, as it stands, with the Stop on it; none for the Stop of a start that has no turn yet.
    pub turn: Option<Turn>,
    /// When the Stop was asked for.
    pub since: DateTime<Utc>,
    /// Whether the checked thread effects had committed when the answer was made. Acceptance stands either way.
    pub applied: bool,
    /// The independently versioned observation used by every reader of the acknowledgement.
    pub stopping: Option<orch_api_contract::threads::ThreadStopping>,
}

/// What carrying the open Stops of a thread out came to.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Applied {
    /// Whether any Stop was carried out.
    pub carried_out: bool,
}

impl Engine {
    /// Stops the turn `turn_id` of a thread for a person who may drive it, or, with none, the start of the
    /// thread's next turn. See [`Engine::stop_guarded`].
    pub async fn stop_thread(
        &self,
        caller: &Caller,
        thread_id: Id,
        turn_id: Option<Id>,
    ) -> Result<Stopped, InterruptError> {
        let mut tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        self.stop_guarded(tx, principal, thread_id, turn_id).await
    }

    /// Stops the turn `turn_id` of a thread, or with none the start that has no turn yet, in the transaction of
    /// the API's action guard: `principal` was loaded in `tx` by that check once the request had come. The
    /// transaction takes no lock on the thread: it checks what it can read, records the Stop, and commits.
    /// The request never waits for the worker's transaction beyond its brief observation window.
    pub async fn stop_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        turn_id: Option<Id>,
    ) -> Result<Stopped, InterruptError> {
        let caller = principal.caller().clone();
        let at = principal.now().get();
        let thread = threads::stop_target(&mut tx, thread_id)
            .await?
            .ok_or(InterruptError::NotFound)?;
        match nodes::check(
            &mut tx,
            &principal,
            thread_id,
            Permission::ThreadInteract,
            Act::Use,
            orch_authz::Policy::default(),
        )
        .await?
        {
            Decision::Allowed { .. } => {}
            Decision::Refused(refusal) => return Err(InterruptError::Denied(Box::new(refusal))),
        }
        // What is asked for is read here without the row and read again under it when it is carried out: a
        // Stop that is out of date by then changes nothing.
        let (target, input_id) = match turn_id {
            Some(turn_id) => {
                let turn = threads::turn(&mut tx, turn_id)
                    .await?
                    .filter(|turn| turn.thread_id == thread_id)
                    .ok_or(InterruptError::NotFound)?;
                let owner = threads::segment(&mut tx, turn.segment_id)
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
                    let current = threads::active_turn(&mut tx, thread_id).await?;
                    let latest = threads::latest_turn(&mut tx, thread_id).await?;
                    if current.is_some() || latest.as_ref().map(|latest| latest.id) != Some(turn.id)
                    {
                        return Err(InterruptError::NotRunning {
                            current_turn_id: current.map(|current| current.id),
                        });
                    }
                }
                (Some(turn_id), turn.input_id)
            }
            None => {
                match (
                    thread.current_turn_id,
                    thread.turn_state.as_deref(),
                    thread.input_id,
                ) {
                    (Some(turn), Some(turn_state::STARTING), Some(input)) => (Some(turn), input),
                    (None, _, Some(input)) if thread.state == thread_state::STARTING => {
                        (None, input)
                    }
                    _ => {
                        return Err(InterruptError::NotRunning {
                            current_turn_id: thread.current_turn_id,
                        });
                    }
                }
            }
        };
        let stop = threads::record_stop(
            &mut tx,
            &threads::NewStop {
                id: orch_types::new_id(),
                thread_id,
                turn_id: target,
                input_id,
                continuation_id: thread.active_continuation_id,
                requested_by: caller.user(),
                requested_at: at,
            },
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // Cancelling this exact read never waits for the row or consumes the accepted fact.
        self.cancel_preparation_read(thread_id, input_id);
        let receipt = stop.id;
        let stop = {
            let mut conn = self.inner.db.acquire().await?;
            threads::first_stop_for_target(&mut conn, thread_id, input_id, target)
                .await?
                .unwrap_or(stop)
        };
        let since = stop.requested_at;
        let applied = self.stop_now(thread_id, receipt).await;
        let turn = match target {
            Some(turn_id) => Some(self.stopped_turn(turn_id, Some(&stop)).await?),
            None => None,
        };
        let stopping = self.thread_stopping(thread_id).await?;
        Ok(Stopped {
            turn,
            since,
            applied,
            stopping,
        })
    }

    /// A small read independent of thread writes and history; the event feed uses the identical observation.
    pub async fn thread_stopping(
        &self,
        thread: Id,
    ) -> Result<Option<orch_api_contract::threads::ThreadStopping>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(threads::stopping_status(&mut conn, thread)
            .await?
            .map(crate::views::stopping_view))
    }

    /// The turn as the answer to its Stop shows it: with the Stop on it even where the thread has not carried
    /// the Stop out yet.
    async fn stopped_turn(
        &self,
        turn_id: Id,
        stop: Option<&threads::StopRow>,
    ) -> Result<Turn, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let mut row = threads::turn(&mut conn, turn_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the turn that was stopped is gone"))?;
        if row.interrupted_by.is_none()
            && row.is_active()
            && let Some(stop) = stop
        {
            row.interrupted_by = Some(stop.requested_by);
            row.interrupted_at = Some(stop.requested_at);
        }
        let people = crate::views::People::load(&mut conn, row.interrupted_by).await?;
        crate::views::turn_view(&row, "", &people)
    }

    /// The acknowledgement never owns the thread transaction; even a free row can lead to a slow writer.
    async fn stop_now(&self, thread_id: Id, receipt: Id) -> bool {
        let progress = self.keep_stop_of(thread_id);
        let _ = tokio::time::timeout(APPLICATION_WAIT, progress.notified()).await;
        match self.inner.db.acquire().await {
            Ok(mut conn) => threads::stop_applied(&mut conn, receipt)
                .await
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Apply acceptance in the thread's one independent worker, never in the request's future.
    pub(crate) async fn apply_stops(&self, thread_id: Id) -> Result<Applied, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let (finished, applied) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let applied = if threads::lock(&mut *unit.conn, thread_id).await?.is_some() {
                self.apply_open_stops_in(&mut unit, thread_id).await?
            } else {
                // Acceptance remains, but a removed thread has no work left to interrupt.
                let open = threads::open_stops(&mut *unit.conn, thread_id).await?;
                let at = unit.now();
                for stop in &open {
                    threads::mark_stop_applied(unit.conn, stop.id, false, at).await?;
                }
                let ids: Vec<Id> = open.iter().map(|stop| stop.id).collect();
                threads::finish_stop_work(unit.conn, &ids, at).await?;
                Applied::default()
            };
            (unit.finish().await?, applied)
        };
        self.commit(tx, finished).await?;
        if let Some(progress) = lock(&self.inner.stops).get(&thread_id) {
            progress.notify_waiters();
        }
        Ok(applied)
    }

    /// Carries out the Stops of a thread that are open, in the transaction that holds its row, before whatever
    /// that transaction does: a message sent after a Stop is held with what the Stop holds and steers into
    /// nothing, and no turn begins behind the Stop's back. A caller that reads the thread before this call
    /// reads it again after one that carried a Stop out.
    pub(crate) async fn apply_open_stops_in(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
    ) -> Result<Applied, EngineError> {
        let open = threads::open_stops(&mut *unit.conn, thread_id).await?;
        let mut applied = Applied::default();
        for stop in &open {
            let preparation = self.apply_stop_in(unit, thread_id, stop).await?;
            let at = unit.now();
            threads::mark_stop_applied(unit.conn, stop.id, preparation, at).await?;
            applied.carried_out = true;
        }
        if applied.carried_out {
            unit.wake(thread_id);
        }
        Ok(applied)
    }

    /// One Stop, under the thread's row. Says whether it gave up a start that had no turn yet.
    async fn apply_stop_in(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        stop: &threads::StopRow,
    ) -> Result<bool, EngineError> {
        let thread = threads::thread(&mut *unit.conn, thread_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread that was stopped is gone"))?;
        let by = stop.requested_by;
        // Accepted Stop holds later arrivals even if an execution selection changed before application.
        self.hold_queue(unit, &thread, by).await?;
        let named = match stop.turn_id {
            Some(turn_id) => threads::turn(&mut *unit.conn, turn_id)
                .await?
                .filter(|turn| turn.thread_id == thread_id),
            None => None,
        };
        let turn = match (stop.turn_id, named) {
            (Some(_), Some(turn)) => Some(turn),
            // The turn is gone.
            (Some(_), None) => return Ok(false),
            // A preparation may already be running natively; only its exact input may be substituted.
            (None, _) => threads::active_turn(&mut *unit.conn, thread_id)
                .await?
                .filter(|turn| turn.input_id == stop.input_id),
        };
        let Some(turn) = turn else {
            self.hold_queue(unit, &thread, by).await?;
            return self
                .give_up_preparation(unit, &thread, stop.input_id, by)
                .await;
        };
        if !turn.is_active() {
            // The turn that has just ended, with nothing newer begun: what waits is still held.
            let current = threads::active_turn(&mut *unit.conn, thread_id).await?;
            let latest = threads::latest_turn(&mut *unit.conn, thread_id).await?;
            if current.is_none() && latest.as_ref().map(|latest| latest.id) == Some(turn.id) {
                self.hold_background_wakes(unit, &thread, turn.id).await?;
            }
            return Ok(false);
        }
        // Stop holds what waits, and protects an empty queue until the program has stopped too.
        self.hold_background_wakes(unit, &thread, turn.id).await?;
        let seq = unit.stamp(thread_id).await?;
        // What cut the turn, for the line its next input is told: a person's Stop.
        threads::set_interrupt_cause(&mut *unit.conn, turn.id, "person").await?;
        if threads::mark_interrupted_by(&mut *unit.conn, turn.id, by, seq).await? {
            threads::set_interrupted_at(&mut *unit.conn, turn.id, stop.requested_at).await?;
            let now = unit.now();
            crate::audit_facts::person(
                &mut *unit.conn,
                by,
                thread_id,
                orch_db::audit::name!("turn.interrupted"),
                serde_json::json!({"turn":turn.id}),
                now,
            )
            .await?;
        }
        // Background wake cleanup can retire an empty ordinary hold. Establish Stop's active
        // hold after that cleanup and its interrupt mark, so later arrivals keep waiting.
        let now = unit.now();
        threads::hold_stop_queue(unit.conn, thread_id, by, now, seq).await?;
        unit.wake(thread_id);
        Ok(false)
    }

    /// The Stop of a start that has no turn: the thread is preparing the next turn's input (waiting for its
    /// environment, recording the directory, writing the files), and gives that up. The messages stay, held
    /// with the rest, and the thread is idle; it begins again when somebody resumes the queue.
    async fn give_up_preparation(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input_id: Id,
        by: Id,
    ) -> Result<bool, EngineError> {
        if thread.state != thread_state::STARTING || thread.current_turn_id.is_some() {
            return Ok(false);
        }
        let thread_id = thread.node_id;
        if !threads::pending_inputs(unit.conn, thread_id)
            .await?
            .iter()
            .any(|input| {
                input.id == input_id
                    && input.state_detail.as_deref() == Some(crate::preparation::PREPARING)
            })
        {
            return Ok(false);
        }
        let seq = unit.stamp(thread_id).await?;
        for input in threads::pending_inputs(&mut *unit.conn, thread_id).await? {
            if input.state_detail.as_deref() == Some(crate::preparation::PREPARING) {
                threads::hold_input(&mut *unit.conn, input.id, None, seq).await?;
            }
        }
        threads::set_state(
            &mut *unit.conn,
            thread_id,
            &[thread_state::STARTING],
            &threads::StateChange::plain(thread_state::IDLE),
            seq,
        )
        .await?;
        let now = unit.now();
        crate::audit_facts::person(
            &mut *unit.conn,
            by,
            thread_id,
            orch_db::audit::name!("turn.interrupted"),
            serde_json::json!({"start": true}),
            now,
        )
        .await?;
        Ok(true)
    }

    /// Cancel only the read belonging to the accepted preparation, never a newer input of the thread.
    fn cancel_preparation_read(&self, thread_id: Id, input_id: Id) {
        if let Some((input, reading)) = lock(&self.inner.preparation_reads).get(&thread_id)
            && *input == input_id
        {
            reading.cancel();
        }
    }

    /// The persisted application keeps this obligation until its commands were cancelled outside the row.
    async fn cancel_preparation_work(
        &self,
        thread_id: Id,
        input_id: Id,
    ) -> Result<(), EngineError> {
        self.cancel_preparation_read(thread_id, input_id);
        let snapshots: Vec<Id> = {
            let mut conn = self.inner.db.acquire().await?;
            sqlx::query_scalar(
                "SELECT id FROM snapshots WHERE thread_id = $1 AND kind = 'turn' AND state = 'running'",
            )
            .bind(thread_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
        };
        for snapshot in snapshots {
            self.inner
                .commands
                .cancel(ids::derived(
                    "fs.record.before-turn",
                    &[snapshot.as_bytes()],
                ))
                .await?;
        }
        Ok(())
    }

    /// Cancels the work a turn's start has in flight (the commands that make its directory, open its session and
    /// its lease, and ask the harness to start it), so that the start's task does not wait for them: each answer
    /// is then heard as cancelled, and the start, whose turn has ended, does nothing. The start of the harness
    /// process is the one thing left alone, because other threads wait for the same process.
    async fn cancel_start_work(&self, turn: &TurnRow) -> Result<(), EngineError> {
        if let Some(waiting) = lock(&self.inner.start_waits).get(&turn.id) {
            waiting.cancel();
        }
        // The model input keeps the actual opening ID, including ordinal and side-history variants.
        let openings: Vec<String> = sqlx::query_scalar("SELECT reference FROM model_inputs WHERE node_id=$1 AND turn_id=$2 AND purpose='session'")
            .bind(turn.thread_id).bind(turn.id).fetch_all(&mut *self.inner.db.acquire().await?).await.map_err(orch_db::DbError::from)?;
        for opening in openings
            .into_iter()
            .filter_map(|reference| reference.parse::<Id>().ok())
        {
            self.inner.commands.cancel(opening).await?;
        }
        for attempt in 0..SESSION_ATTEMPTS {
            self.inner
                .commands
                .cancel(ids::instruction_root(turn.id, attempt))
                .await?;
        }
        if let Some(lease) = turn.lease_id {
            for attempt in 0..=LEASE_OPEN_ATTEMPTS {
                self.inner
                    .commands
                    .cancel(ids::lease_open(lease, attempt))
                    .await?;
            }
        }
        self.inner
            .commands
            .cancel(ids::thread_directory(turn.id))
            .await?;
        Ok(())
    }

    /// What follows the carrying out of a Stop, outside any transaction and safe to do again: the work a start
    /// has in flight is cancelled, then the harness is asked to stop the turn, or the start the harness has not
    /// taken is ended, or, after the wait that is configured, the turn is ended without the harness. It is not
    /// part of the thread's phases, which may be waiting for something else.
    pub(crate) async fn drive_stop_work(&self, thread_id: Id) -> Result<(), EngineError> {
        let work = {
            let mut conn = self.inner.db.acquire().await?;
            threads::pending_stop_work(&mut conn, thread_id).await?
        };
        for stop in &work {
            if stop.cancel_preparation {
                self.cancel_preparation_work(thread_id, stop.input_id)
                    .await?;
            }
        }
        if !work.is_empty() {
            let ids: Vec<_> = work.iter().map(|stop| stop.id).collect();
            // Publish completion only after the application transaction and external cancellation are over.
            let mut tx = self.inner.db.begin().await?;
            threads::finish_stop_work(&mut tx, &ids, self.inner.clock.now()).await?;
            threads::announce_stop(&mut tx, thread_id, None).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            self.wake(thread_id);
        }
        let turn = {
            let mut conn = self.inner.db.acquire().await?;
            threads::active_turn(&mut conn, thread_id).await?
        };
        let Some(turn) = turn.filter(|turn| turn.interrupted_by.is_some()) else {
            return Ok(());
        };
        if turn.state == turn_state::STARTING {
            self.cancel_start_work(&turn).await?;
        }
        self.drive_interrupt_turn(thread_id, &turn).await
    }

    /// Leaves a thread's Stops, and the work that follows them, to a task of the engine's own that does not
    /// stop until they are carried out. One thread has one such task at a time.
    pub(crate) fn keep_stop_of(&self, thread_id: Id) -> std::sync::Arc<tokio::sync::Notify> {
        let progress = {
            let mut stops = lock(&self.inner.stops);
            if let Some(progress) = stops.get(&thread_id) {
                return std::sync::Arc::clone(progress);
            }
            let progress = std::sync::Arc::new(tokio::sync::Notify::new());
            stops.insert(thread_id, std::sync::Arc::clone(&progress));
            progress
        };
        let ended = std::sync::Arc::clone(&progress);
        let engine = self.clone();
        tokio::spawn(async move {
            // The explicit operation gets its own attempt even when only the supervisor is paused.
            let mut completed = false;
            loop {
                match engine.carry_out_stop_of(thread_id).await {
                    Ok(()) => {
                        completed = true;
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(%error, %thread_id, "a Stop could not be carried out; the engine tries again");
                        if engine.is_stopped() {
                            break;
                        }
                        tokio::time::sleep(RETRY_PAUSE).await;
                    }
                }
            }
            lock(&engine.inner.stops).remove(&thread_id);
            ended.notify_waiters();
            // A Stop that was asked while this task finished is not left to the sweep.
            let more = match engine.inner.db.acquire().await {
                Ok(mut conn) => threads::open_stop_since(&mut conn, thread_id)
                    .await
                    .ok()
                    .flatten()
                    .is_some(),
                Err(_) => false,
            };
            if more && (completed || !engine.is_stopped()) {
                engine.keep_stop_of(thread_id);
            }
        });
        progress
    }

    async fn carry_out_stop_of(&self, thread_id: Id) -> Result<(), EngineError> {
        // Another act may already have applied a Stop; its cancellation never waits for a later row holder.
        self.drive_stop_work(thread_id).await?;
        let open = {
            let mut conn = self.inner.db.acquire().await?;
            !threads::open_stops(&mut conn, thread_id).await?.is_empty()
        };
        if open {
            self.apply_stops(thread_id).await?;
        }
        self.drive_stop_work(thread_id).await
    }

    /// The sweep's look at Stops: those the engine has not carried out (a process that stopped between asking
    /// and carrying out, or a thread whose row stayed held) and the turns that were asked to stop and are not
    /// over, whose harness has to be asked or whose wait is out. Each goes to the task of its thread, which the
    /// sweep never waits for.
    pub(crate) async fn keep_stops(&self) -> Result<(), EngineError> {
        let batch = self.inner.config.sweep_batch;
        let (open, driven) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                threads::threads_with_open_stops(&mut conn, batch).await?,
                threads::interrupted_active_turns(&mut conn, batch).await?,
            )
        };
        for thread in open {
            self.keep_stop_of(thread);
        }
        for turn in driven {
            self.keep_stop_of(turn.thread_id);
        }
        Ok(())
    }
}
