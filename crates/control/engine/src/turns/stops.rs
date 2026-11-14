//! A person's Stop: recorded the moment it is asked, carried out under the thread's row by whoever takes the row
//! next, and the work the start has in flight cancelled instead of waited for.
//!
//! Everybody who changes a thread holds its row while they do, and some hold it for a while (a long write, a
//! summary that reads its whole source). A Stop that took the row first would wait behind all of them, and past
//! the fifteen seconds that a statement may wait it would fail. So the door of a Stop only writes one row of its
//! own, which refers to nothing and so waits for nobody ([`threads::record_stop`]), and then looks for the thread's
//! row for a moment ([`ROW_WAIT`]). Where it gets the row, the Stop is carried out in the answer, as it always was;
//! where it does not, the answer says the thread is stopping, and the engine carries the Stop out as soon as the
//! row is free (a task of its own that keeps its place in the line for the row, and the sweep, which finds a Stop
//! that a restart left). Every other change that takes the row carries the Stops that are open out first
//! ([`Engine::apply_open_stops_in`]), so that a message sent after a Stop never overtakes it, and no turn is
//! started behind its back.
//!
//! What the Stop of a turn does is what it always did: holds what waits, marks the turn, and has the harness asked
//! to stop it, or ends a start the harness has not taken. What the Stop of a start with no turn yet does (the thread
//! is recording the directory or writing the message's files) is to give that up: the message stays in the queue,
//! held with the rest, the thread is idle, and what the start had in flight is cancelled so that nothing waits for
//! it.

use super::*;
use chrono::{DateTime, Utc};

/// How long a Stop looks for the thread's row before it answers that the thread is stopping: an answer at once,
/// and long enough that a thread nobody else holds carries the Stop out in the answer.
const ROW_WAIT: Duration = Duration::from_millis(250);

/// The pause between two looks at a row that is held.
const ROW_LOOK: Duration = Duration::from_millis(20);

/// How long the answer waits for what follows the Stop (the harness asked to stop, a start ended) before it goes
/// on without it: that work goes on in a task, and is safe to do again.
const WORK_WAIT: Duration = Duration::from_millis(300);

/// The pause before the engine looks again for the row of a thread whose Stop it could not carry out.
const RETRY_PAUSE: Duration = Duration::from_secs(1);

/// What a Stop was answered with.
#[derive(Debug, Clone)]
pub struct Stopped {
    /// The turn it names, as it stands, with the Stop on it; none for the Stop of a start that has no turn yet.
    pub turn: Option<Turn>,
    /// When the Stop was asked for.
    pub since: DateTime<Utc>,
    /// Whether the thread had carried the Stop out when the answer was made; otherwise the thread's row is held
    /// by another change and the Stop is carried out as soon as it is free.
    pub applied: bool,
}

/// What carrying the open Stops of a thread out came to.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Applied {
    /// Whether any Stop was carried out.
    pub carried_out: bool,
    /// Whether a start that had no turn yet was given up.
    pub preparation: bool,
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
    /// What follows waits for the thread's row only for a moment (see the module's words).
    pub async fn stop_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        turn_id: Option<Id>,
    ) -> Result<Stopped, InterruptError> {
        let caller = principal.caller().clone();
        let at = principal.now().get();
        let thread = threads::thread(&mut tx, thread_id)
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
        let target = match turn_id {
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
                Some(turn_id)
            }
            None => {
                // The start of the next turn: a turn that exists and has not begun is named, and a thread that
                // is preparing one has none to name.
                let active = threads::active_turn(&mut tx, thread_id).await?;
                match active {
                    Some(turn) if turn.state == turn_state::STARTING => Some(turn.id),
                    Some(turn) => {
                        return Err(InterruptError::NotRunning {
                            current_turn_id: Some(turn.id),
                        });
                    }
                    None if thread.state == thread_state::STARTING => None,
                    None => {
                        return Err(InterruptError::NotRunning {
                            current_turn_id: None,
                        });
                    }
                }
            }
        };
        let stop = threads::record_stop(
            &mut tx,
            orch_types::new_id(),
            thread_id,
            target,
            caller.user(),
            at,
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let since = stop.as_ref().map_or(at, |stop| stop.requested_at);
        let applied = self.stop_now(thread_id).await;
        let turn = match target {
            Some(turn_id) => Some(self.stopped_turn(turn_id, stop.as_ref()).await?),
            None => None,
        };
        Ok(Stopped {
            turn,
            since,
            applied,
        })
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

    /// Carries the open Stops of a thread out, looking for its row for a moment, and does what follows; where
    /// the row stays held, hands the Stops to the engine's own task. Says whether they were carried out.
    async fn stop_now(&self, thread_id: Id) -> bool {
        match self.apply_stops(thread_id, Some(ROW_WAIT)).await {
            Ok(Some(applied)) => {
                let cancelled = if applied.preparation {
                    self.cancel_preparation_work(thread_id).await
                } else {
                    Ok(())
                };
                if let Err(error) = cancelled {
                    tracing::warn!(%error, %thread_id, "the work of a start that was stopped could not be cancelled");
                }
                let _ = tokio::time::timeout(WORK_WAIT, self.spawn_stop_work(thread_id)).await;
                true
            }
            Ok(None) => {
                self.keep_stop_of(thread_id);
                false
            }
            Err(error) => {
                tracing::warn!(%error, %thread_id, "a Stop could not be carried out at once; the engine tries again");
                self.keep_stop_of(thread_id);
                false
            }
        }
    }

    /// Carries the Stops that are open for a thread out, in their own transaction. With a time to wait, looks
    /// for the row that long and answers `None` where it is still held; without one, waits for it in its place
    /// in the line (a statement of the database's own bound still applies, and ends in an error that the
    /// caller looks again after).
    pub(crate) async fn apply_stops(
        &self,
        thread_id: Id,
        wait: Option<Duration>,
    ) -> Result<Option<Applied>, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        if let Some(wait) = wait {
            let started = tokio::time::Instant::now();
            while !threads::lock_if_free(&mut tx, thread_id).await? {
                if started.elapsed() >= wait {
                    return Ok(None);
                }
                tokio::time::sleep(ROW_LOOK).await;
            }
        }
        let (finished, applied) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let applied = if threads::lock(&mut *unit.conn, thread_id).await?.is_some() {
                self.apply_open_stops_in(&mut unit, thread_id).await?
            } else {
                // The thread is gone, and its Stops with it.
                let open = threads::open_stops(&mut *unit.conn, thread_id).await?;
                let ids: Vec<Id> = open.iter().map(|stop| stop.id).collect();
                threads::close_stops(&mut *unit.conn, &ids).await?;
                Applied::default()
            };
            (unit.finish().await?, applied)
        };
        self.commit(tx, finished).await?;
        Ok(Some(applied))
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
            let one = self.apply_stop_in(unit, thread_id, stop).await?;
            applied.carried_out = true;
            applied.preparation |= one;
        }
        if applied.carried_out {
            let ids: Vec<Id> = open.iter().map(|stop| stop.id).collect();
            threads::close_stops(&mut *unit.conn, &ids).await?;
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
            // The start has a turn by now: it is that turn's start that is stopped.
            (None, _) => threads::active_turn(&mut *unit.conn, thread_id)
                .await?
                .filter(|turn| turn.state == turn_state::STARTING),
        };
        let Some(turn) = turn else {
            return self.give_up_preparation(unit, &thread, by).await;
        };
        if !turn.is_active() {
            // The turn that has just ended, with nothing newer begun: what waits is still held.
            let current = threads::active_turn(&mut *unit.conn, thread_id).await?;
            let latest = threads::latest_turn(&mut *unit.conn, thread_id).await?;
            if current.is_none() && latest.as_ref().map(|latest| latest.id) == Some(turn.id) {
                self.hold_queue(unit, &thread, by).await?;
                self.hold_background_wakes(unit, &thread, turn.id).await?;
            }
            return Ok(false);
        }
        // Stop holds what waits before the harness is asked to stop.
        self.hold_queue(unit, &thread, by).await?;
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
        by: Id,
    ) -> Result<bool, EngineError> {
        if thread.state != thread_state::STARTING || thread.current_turn_id.is_some() {
            return Ok(false);
        }
        let thread_id = thread.node_id;
        self.hold_queue(unit, thread, by).await?;
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

    /// Cancels what the thread's recording of its directory has in flight, so that nothing waits for it.
    async fn cancel_preparation_work(&self, thread_id: Id) -> Result<(), EngineError> {
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
        for attempt in 0..SESSION_ATTEMPTS {
            self.inner
                .commands
                .cancel(ids::session_open(turn.segment_id, attempt))
                .await?;
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

    fn spawn_stop_work(&self, thread_id: Id) -> tokio::task::JoinHandle<()> {
        let engine = self.clone();
        tokio::spawn(async move {
            if let Err(error) = engine.drive_stop_work(thread_id).await {
                tracing::warn!(%error, %thread_id, "the work that follows a Stop could not be done; it is looked at again");
            }
        })
    }

    /// Leaves a thread's Stops, and the work that follows them, to a task of the engine's own that does not
    /// stop until they are carried out. One thread has one such task at a time.
    pub(crate) fn keep_stop_of(&self, thread_id: Id) {
        if !lock(&self.inner.stops).insert(thread_id) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                if engine.is_stopped() {
                    break;
                }
                match engine.carry_out_stop_of(thread_id).await {
                    Ok(()) => break,
                    Err(error) => {
                        tracing::warn!(%error, %thread_id, "a Stop could not be carried out; the engine tries again");
                        tokio::time::sleep(RETRY_PAUSE).await;
                    }
                }
            }
            lock(&engine.inner.stops).remove(&thread_id);
            // A Stop that was asked while this task finished is not left to the sweep.
            let more = match engine.inner.db.acquire().await {
                Ok(mut conn) => threads::open_stop_since(&mut conn, thread_id)
                    .await
                    .ok()
                    .flatten()
                    .is_some(),
                Err(_) => false,
            };
            if more && !engine.is_stopped() {
                engine.keep_stop_of(thread_id);
            }
        });
    }

    async fn carry_out_stop_of(&self, thread_id: Id) -> Result<(), EngineError> {
        let open = {
            let mut conn = self.inner.db.acquire().await?;
            threads::open_stop_since(&mut conn, thread_id)
                .await?
                .is_some()
        };
        let applied = if open {
            self.apply_stops(thread_id, None).await?
        } else {
            None
        };
        if applied.is_some_and(|applied| applied.preparation) {
            self.cancel_preparation_work(thread_id).await?;
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
