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

impl Engine {
    /// Waits for `work`, which is the end of the command `command` of kind `kind` that a start of `thread`
    /// asked for, and tells the thread what it waits for once that has lasted [`LOOK_EVERY`]: the command,
    /// and the other work it stands behind, if any. The note goes when the wait ends.
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
        tokio::pin!(work);
        let mut look =
            tokio::time::interval_at(tokio::time::Instant::now() + LOOK_EVERY, LOOK_EVERY);
        look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut noted: Option<Option<String>> = None;
        let mut own = Duration::ZERO;
        let ended = loop {
            tokio::select! {
                done = &mut work => break Waited::Done(done),
                _ = look.tick() => {
                    let behind = self.held_up_by(command).await;
                    if noted.as_ref() != Some(&behind) {
                        if let Err(error) = self
                            .note_start_work(thread, Some(kind.as_str()), behind.as_deref())
                            .await
                        {
                            tracing::debug!(%error, %thread, "the wait of a start could not be noted");
                        } else {
                            noted = Some(behind.clone());
                        }
                    }
                    if behind.is_none() {
                        own += LOOK_EVERY;
                    }
                    if patience.is_some_and(|patience| own >= patience) {
                        break Waited::Tired;
                    }
                }
            }
        };
        if noted.is_some()
            && let Err(error) = self.note_start_work(thread, None, None).await
        {
            tracing::debug!(%error, %thread, "the note of a start's wait could not be cleared");
        }
        ended
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
