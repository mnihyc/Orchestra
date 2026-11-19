//! The waits of a thread's start, told to the thread: which command its step waits for, and when other work
//! on the same machine holds that command up, which.
//!
//! A command that shares its serial key with another waits in the machine's line until the one before it
//! ends, however long that takes (the line is the machine's, and a command's deadline counts only from the
//! moment it leaves it). A person who sees "Preparing" and nothing else cannot tell that from a fault, so a
//! wait that lasts is noted on the thread's row: the kind of command, and the kind of command it stands
//! behind, with the time the step began (the database keeps it). A quick command is never noted: nothing is
//! written before the first look, a few seconds in.

use std::future::Future;
use std::time::Duration;

use orch_db::queries::{link, threads};
use orch_link_contract::commands::CommandKind;
use orch_types::Id;

use crate::commands::{CommandsError, Outcome, Outgoing};
use crate::unit::Unit;
use crate::{Engine, EngineError};

/// How soon after a command was asked, and how often after, its wait is looked at.
pub(crate) const LOOK_EVERY: Duration = Duration::from_secs(2);

/// How a wait ended.
pub(crate) enum Waited<T> {
    /// The work ended and gave this.
    Done(T),
    /// The patience given ran out first: the work was not ended.
    Tired,
}

struct Observing(tokio::task::JoinHandle<()>);

impl Drop for Observing {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Optional reporting owns a separate future. Neither its database reads nor its row writes
/// can delay the reply or the monotonic deadline that the caller actually waits on.
async fn wait_while_observing<T>(
    work: impl Future<Output = T>,
    observing: impl Future<Output = ()> + Send + 'static,
    mut blocked: tokio::sync::watch::Receiver<bool>,
    mut remaining: Option<Duration>,
) -> Waited<T> {
    let _observing = Observing(tokio::spawn(observing));
    tokio::pin!(work);
    let mut paused = *blocked.borrow();
    let mut watching = true;
    loop {
        let began = tokio::time::Instant::now();
        let deadline = async move {
            match remaining {
                Some(left) if !paused => tokio::time::sleep(left).await,
                _ => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            biased;
            done = &mut work => return Waited::Done(done),
            () = deadline => return Waited::Tired,
            changed = blocked.changed(), if watching => {
                if !paused { remaining = remaining.map(|left| left.saturating_sub(began.elapsed())); }
                watching = changed.is_ok();
                paused = watching && *blocked.borrow();
            }
        }
    }
}

impl Engine {
    /// Waits for `work`, which is the end of the command `command` of kind `kind` that a start of `thread`
    /// asked for, and tells the thread what it waits for once that has lasted [`LOOK_EVERY`]: the command,
    /// and the other work it stands behind, if any. A step change clears its old note atomically.
    ///
    /// With a `patience` the wait is given up when that much time has gone by with the command not held up
    /// by other work: the time spent in the line of the machine is not the command's own and does not count.
    pub(crate) async fn waiting_noted<T>(
        &self,
        thread: Id,
        command: Id,
        kind: CommandKind,
        patience: Option<Duration>,
        work: impl Future<Output = T>,
    ) -> Waited<T> {
        let engine = self.clone();
        let (held, blocked) = tokio::sync::watch::channel(false);
        let observing = async move {
            // One latest note is enough. Its writer can wait for a row lock without stopping
            // ownership observations or retaining a growing queue of obsolete progress.
            let (notes, mut pending) = tokio::sync::watch::channel::<Option<Option<String>>>(None);
            let writer = engine.clone();
            let _writing = Observing(tokio::spawn(async move {
                while pending.changed().await.is_ok() {
                    loop {
                        let behind = pending.borrow_and_update().clone();
                        let Some(behind) = behind else { break };
                        match writer
                            .note_start_work(thread, Some(kind.as_str()), behind.as_deref())
                            .await
                        {
                            Ok(()) => break,
                            Err(error) => {
                                tracing::debug!(%error, %thread, "the wait of a start could not be noted")
                            }
                        }
                        tokio::time::sleep(LOOK_EVERY).await;
                    }
                }
            }));
            let mut look =
                tokio::time::interval_at(tokio::time::Instant::now() + LOOK_EVERY, LOOK_EVERY);
            look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                look.tick().await;
                let owns_key = engine.key_is_held(command).await;
                if held.send(owns_key).is_err() {
                    return;
                }
                let behind = engine.held_up_by(command).await;
                notes.send_if_modified(|last| {
                    if last.as_ref() == Some(&behind) {
                        false
                    } else {
                        *last = Some(behind);
                        true
                    }
                });
            }
        };
        // Step changes clear their old note in the same preparation transaction. Finishing the
        // wait never acquires the thread again just to erase optional progress.
        wait_while_observing(work, observing, blocked, patience).await
    }

    /// Issues `command` for a start of `thread` and waits for its end, noting the wait as
    /// [`Engine::waiting_noted`] does.
    pub(crate) async fn run_noted(
        &self,
        thread: Id,
        command: Outgoing,
    ) -> Result<Outcome, CommandsError> {
        let (id, kind) = (command.id, command.kind);
        let work = crate::commands::run(self.inner.commands.as_ref(), command);
        match self.waiting_noted(thread, id, kind, None, work).await {
            Waited::Done(outcome) => outcome,
            Waited::Tired => Err(CommandsError::Other(
                "a wait that has no patience ended".to_owned(),
            )),
        }
    }

    /// The kind of command that holds `command` up in its machine's line, if one does.
    async fn held_up_by(&self, command: Id) -> Option<String> {
        let mut conn = self.inner.db.acquire().await.ok()?;
        link::held_up_by(&mut conn, command)
            .await
            .ok()
            .flatten()
            .map(|(kind, _)| kind)
    }

    async fn key_is_held(&self, command: Id) -> bool {
        let Ok(mut conn) = self.inner.db.acquire().await else {
            return false;
        };
        link::key_holder(&mut conn, command)
            .await
            .ok()
            .flatten()
            .is_some()
    }

    /// Writes which command the step of a starting thread waits for and which work holds it up, when that
    /// is not what the row says; a number is spent only on a change.
    async fn note_start_work(
        &self,
        thread: Id,
        command: Option<&str>,
        behind: Option<&str>,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if let Some(row) = threads::lock(&mut *unit.conn, thread).await?
                && row.state == threads::thread_state::STARTING
                && row.starting_step.is_some()
                && (row.starting_command.as_deref() != command
                    || row.starting_behind.as_deref() != behind)
            {
                let seq = unit.stamp(thread).await?;
                threads::set_starting_work(&mut *unit.conn, thread, command, behind, seq).await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod tests {
    use super::*;
    use tokio::sync::watch;
    use tokio::time::Instant;

    #[tokio::test]
    async fn releasing_the_real_key_resumes_patience_while_its_progress_write_is_still_locked() {
        use crate::test_common::{World, eventually};
        use orch_db::queries::link::{NewCommand, insert_command, mark_accepted_phase};
        let world = World::new().await;
        let thread = world.thread("A held progress note").await;
        let now = world.clock.now();
        let first = orch_types::new_id();
        let next = orch_types::new_id();
        let body = serde_json::json!({});
        let mut conn = world.pool.acquire().await.unwrap();
        for (id, offset, phase) in [(first, 0, 1), (next, 1, 0)] {
            insert_command(
                &mut conn,
                &NewCommand {
                    native_target: None,
                    id,
                    machine_id: world.machine_id,
                    kind: "fs.record",
                    body: &body,
                    serial_key: Some("environment"),
                    deadline_at: now + chrono::Duration::minutes(1),
                },
                now + chrono::Duration::seconds(offset),
            )
            .await
            .unwrap();
            mark_accepted_phase(&mut conn, id, phase, now)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE threads SET state='starting', starting_step='files' WHERE node_id=$1")
            .bind(thread)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let mut holder = world.pool.begin().await.unwrap();
        threads::lock(&mut holder, thread).await.unwrap();
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        let engine = world.engine();
        let waiting = tokio::spawn(async move {
            engine
                .waiting_noted(
                    thread,
                    next,
                    CommandKind::FsRecord,
                    Some(LOOK_EVERY * 3),
                    std::future::pending::<()>(),
                )
                .await
        });
        eventually(
            "the progress note is waiting for the held thread row",
            || async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a
                WHERE a.datname=current_database() AND $1=ANY(pg_blocking_pids(a.pid)))",
                )
                .bind(holder_pid)
                .fetch_one(&mut *world.pool.acquire().await.unwrap())
                .await
                .unwrap()
            },
        )
        .await;
        let mut conn = world.pool.acquire().await.unwrap();
        mark_accepted_phase(&mut conn, first, 2, now).await.unwrap();
        drop(conn);
        let result = tokio::time::timeout(Duration::from_secs(15), waiting)
            .await
            .expect("actual key release resumes the timer before the optional note can be written")
            .unwrap();
        assert!(matches!(result, Waited::Tired));
        holder.rollback().await.unwrap();
        world.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_completed_reply_does_not_wait_for_the_progress_database() {
        let (_held, blocked) = watch::channel(false);
        let began = Instant::now();
        let result = wait_while_observing(
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                42
            },
            std::future::pending(),
            blocked,
            Some(Duration::from_secs(5)),
        )
        .await;
        assert!(matches!(result, Waited::Done(42)));
        assert_eq!(began.elapsed(), Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_recordings_patience_is_elapsed_time_even_when_progress_queries_stall() {
        let (_held, blocked) = watch::channel(false);
        let began = Instant::now();
        let result = wait_while_observing(
            std::future::pending::<()>(),
            async {
                tokio::time::sleep(Duration::from_secs(15)).await;
                std::future::pending::<()>().await
            },
            blocked,
            Some(Duration::from_secs(5)),
        )
        .await;
        assert!(matches!(result, Waited::Tired));
        assert_eq!(began.elapsed(), Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn only_time_with_an_actual_key_holder_is_excluded_from_patience() {
        let (held, blocked) = watch::channel(true);
        let began = Instant::now();
        let result = wait_while_observing(
            std::future::pending::<()>(),
            async move {
                tokio::time::sleep(Duration::from_secs(20)).await;
                let _ = held.send(false);
                std::future::pending::<()>().await;
            },
            blocked,
            Some(Duration::from_secs(5)),
        )
        .await;
        assert!(matches!(result, Waited::Tired));
        assert_eq!(began.elapsed(), Duration::from_secs(25));
    }
}
