//! One thread-locked, transactional follow-up for an immutable terminal turn.
//!
//! Core state/pointer cleanup commits before this work. Its original usage/time survive
//! failures and process restarts; ledger, notifications and removal commit together.

use super::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EndWork {
    at: DateTime<Utc>,
    usage: Option<Json>,
    usage_detail: Option<Json>,
    was_starting: bool,
    thread_name: String,
    org: Id,
    native_completed: bool,
    watching_author: Option<Id>,
}

pub(super) async fn keep(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    turn: &TurnRow,
    end: &TurnEnd,
    live: &dyn crate::live::LiveSink,
) -> Result<(), EngineError> {
    let native_completed: bool =
        sqlx::query_scalar("SELECT native_end_confirmed FROM turns WHERE id=$1")
            .bind(turn.id)
            .fetch_one(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
    let watching_author = if end.state == turn_state::COMPLETED {
        orch_db::queries::notifications::events::input_author(&mut *unit.conn, turn.input_id)
            .await?
            .filter(|author| live.thread_is_visible(thread.node_id, *author))
    } else {
        None
    };
    let work = EndWork {
        at: turn.completed_at.unwrap_or(unit.now()),
        usage: turn.usage.clone(),
        usage_detail: turn.usage_detail.clone(),
        was_starting: turn.state == turn_state::STARTING || turn.started_at.is_none(),
        thread_name: thread.name.clone(),
        org: thread.org_id,
        native_completed,
        watching_author,
    };
    threads::keep_turn_end_work(&mut *unit.conn, turn.id, &json!({"snapshot":work})).await?;
    Ok(())
}

fn terminal_end(turn: &TurnRow) -> Result<TurnEnd, EngineError> {
    let state = match turn.state.as_str() {
        turn_state::COMPLETED => turn_state::COMPLETED,
        turn_state::FAILED => turn_state::FAILED,
        turn_state::INTERRUPTED => turn_state::INTERRUPTED,
        _ => {
            return Err(EngineError::unreadable(
                "a pending terminal follow-up belongs to an active turn",
            ));
        }
    };
    Ok(TurnEnd {
        state,
        error: turn.error.clone(),
        input: InputAfter::Keep,
        then_block: None,
    })
}

/// Stable diagnostic categories; never format a database error, stored value or command body.
fn failure_class(error: &EngineError) -> (&'static str, Option<String>) {
    use orch_db::DbError;
    match error {
        EngineError::Db(DbError::Unique { .. }) => ("db_unique", Some("23505".into())),
        EngineError::Db(DbError::Reference { .. }) => ("db_reference", Some("23503".into())),
        EngineError::Db(DbError::Check { .. }) => ("db_check", Some("23514".into())),
        EngineError::Db(DbError::Rule { .. }) => ("db_rule", None),
        EngineError::Db(DbError::Retry) => ("db_retry", None),
        EngineError::Db(DbError::Other(error)) => {
            if let Some(database) = error.as_database_error() {
                let code = database
                    .code()
                    .filter(|code| {
                        code.len() == 5 && code.bytes().all(|byte| byte.is_ascii_alphanumeric())
                    })
                    .map(|code| code.into_owned());
                ("db_database", code)
            } else {
                let class = match error {
                    sqlx::Error::PoolTimedOut => "pool_timeout",
                    sqlx::Error::RowNotFound => "row_missing",
                    sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "db_decode",
                    _ => "db_other",
                };
                (class, None)
            }
        }
        EngineError::Authz(_) => ("authz", None),
        EngineError::Access(_) => ("access", None),
        EngineError::Settings(_) => ("settings", None),
        EngineError::Commands(_) => ("commands", None),
        EngineError::MountFingerprint(_) => ("mount_fingerprint", None),
        EngineError::Unreadable(_) => ("unreadable", None),
        EngineError::StaleToolSource => ("stale_tool_source", None),
        EngineError::ToolCallIdentity => ("tool_call_identity", None),
    }
}

impl Engine {
    fn end_work_phase(
        &self,
        thread: Id,
        phase: &'static str,
        outcome: &'static str,
        error: Option<&EngineError>,
    ) {
        let (error_class, sqlstate) = error.map_or(("none", None), failure_class);
        tracing::debug!(target:"orch_engine::terminal_work",%thread,phase,outcome,error_class,
            sqlstate=sqlstate.as_deref().unwrap_or(""),stopped=self.is_stopped());
    }

    /// Repairs an older partial end in its own transaction: a failed follow-up cannot roll
    /// this repair back. Only this exact thread's current, already-terminal turn qualifies.
    async fn repair_terminal_current(&self, thread_id: Id) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let Some(thread) = threads::lock(&mut tx, thread_id).await? else {
            return Ok(());
        };
        let Some(id) = thread.current_turn_id else {
            return Ok(());
        };
        let Some(turn) = threads::turn(&mut tx, id)
            .await?
            .filter(|turn| turn.thread_id == thread_id && !turn.is_active())
        else {
            return Ok(());
        };
        let end = terminal_end(&turn)?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        self.end_turn(&mut unit, &thread, &turn, end).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Owns at most one pending turn through its thread row lock. All waits are metadata
    /// reads/writes on this connection; no lease, second connection or native/store wait.
    pub(crate) async fn drive_turn_end_work(&self, thread_id: Id) -> Result<(), EngineError> {
        self.end_work_phase(thread_id, "repair_current", "enter", None);
        if let Err(error) = self.repair_terminal_current(thread_id).await {
            self.end_work_phase(thread_id, "repair_current", "error", Some(&error));
            return Err(error);
        }
        let mut phase = "begin";
        match self.apply_turn_end_work(thread_id, &mut phase).await {
            Ok(()) => {
                self.end_work_phase(thread_id, phase, "ok", None);
                Ok(())
            }
            Err(error) => {
                self.end_work_phase(thread_id, phase, "error", Some(&error));
                // The failed work transaction has ended. Retain the immutable snapshot,
                // rotate the next bounded sweep, and publish a cause for the waiting input.
                // Another block and a person's queue hold remain their own decisions.
                self.end_work_phase(thread_id, "record_failure", "enter", None);
                let recorded = async {
                    let mut tx = self.db().begin().await?;
                    if let Some(thread) = threads::lock(&mut tx, thread_id).await?
                        && threads::turn_end_work_pending(&mut tx, thread_id).await?
                    {
                        threads::turn_end_work_attempted(
                            &mut tx,
                            thread_id,
                            self.inner.clock.now(),
                        )
                        .await?;
                        if thread.current_turn_id.is_none() && thread.state == thread_state::IDLE {
                            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
                            let seq = unit.stamp(thread_id).await?;
                            threads::set_state(
                                &mut *unit.conn,
                                thread_id,
                                &[thread_state::IDLE],
                                &threads::StateChange::blocked("error", Some(END_FOLLOWUP_FAILED)),
                                seq,
                            )
                            .await?;
                            unit.finish().await?;
                        }
                    }
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    Ok::<(), EngineError>(())
                }
                .await;
                if let Err(failure) = recorded {
                    self.end_work_phase(thread_id, "record_failure", "error", Some(&failure));
                    return Err(failure);
                }
                self.end_work_phase(thread_id, "record_failure", "ok", None);
                Err(error)
            }
        }
    }

    async fn apply_turn_end_work(
        &self,
        thread_id: Id,
        phase: &mut &'static str,
    ) -> Result<(), EngineError> {
        macro_rules! entering {
            ($name:literal) => {{
                *phase = $name;
                self.end_work_phase(thread_id, $name, "enter", None);
            }};
        }
        entering!("begin");
        let mut tx = self.db().begin().await?;
        entering!("lock_thread");
        let Some(mut thread) = threads::lock(&mut tx, thread_id).await? else {
            return Ok(());
        };
        entering!("load_work");
        let Some((id, saved)) = threads::turn_end_work(&mut tx, thread_id).await? else {
            return Ok(());
        };
        entering!("decode_work");
        let work: EndWork = serde_json::from_value(saved["snapshot"].clone())?;
        entering!("load_turn");
        let mut turn = threads::turn(&mut tx, id)
            .await?
            .filter(|turn| turn.thread_id == thread_id)
            .ok_or_else(|| EngineError::unreadable("the terminal follow-up's turn disappeared"))?;
        let end = terminal_end(&turn)?;
        turn.usage = work.usage;
        turn.usage_detail = work.usage_detail;
        turn.completed_at = Some(work.at);
        thread.name = work.thread_name;
        thread.org_id = work.org;
        // Recipient authority is evaluated now; only the accounting event is dated
        // to the frozen terminal time. Backdating the unit would revive expired roles.
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        entering!("settle_requests");
        self.close_running_requests(&mut *unit.conn, turn.id, end.state, end.error.as_ref())
            .await?;
        entering!("account_usage");
        self.count_harness_usage(&mut unit, &turn).await?;
        // Notification eligibility is based on the turn before terminalization. Its event
        // identity stays the turn ID; current recipient permissions are checked on replay.
        let mut before = turn.clone();
        before.state = if work.was_starting {
            turn_state::STARTING
        } else {
            turn_state::RUNNING
        }
        .to_owned();
        entering!("notifications");
        crate::notifications::turn_ended(&mut unit, &thread, &before, &end, work.watching_author)
            .await?;
        if work.native_completed {
            entering!("context");
            self.estimate_context_at_end(&mut unit, thread_id, turn.id)
                .await?;
            if orch_db::queries::thread_history::selected_turn(unit.conn, thread_id, turn.id)
                .await?
            {
                entering!("summary_due");
                crate::summaries::due(&mut *unit.conn, thread_id, "turn_completed", work.at)
                    .await?;
            }
        }
        entering!("retire_work");
        if !threads::finish_turn_end_work(&mut *unit.conn, id, &saved).await? {
            return Err(EngineError::unreadable(
                "the terminal follow-up changed while its thread was locked",
            ));
        }
        if thread.current_turn_id.is_none()
            && thread.state == thread_state::BLOCKED
            && thread.blocked_reason.as_deref() == Some("error")
            && thread.blocked_detail.as_deref() == Some(END_FOLLOWUP_FAILED)
        {
            entering!("clear_reason");
            let seq = unit.stamp(thread_id).await?;
            threads::set_state(
                &mut *unit.conn,
                thread_id,
                &[thread_state::BLOCKED],
                &threads::StateChange::plain(thread_state::IDLE),
                seq,
            )
            .await?;
        }
        unit.wake(thread_id);
        entering!("unit_finish");
        let finished = unit.finish().await?;
        entering!("commit");
        self.commit(tx, finished).await?;
        self.room_may_have_been_made();
        Ok(())
    }
}

#[cfg(test)]
mod diagnostics_tests {
    use super::*;

    #[test]
    fn failure_metadata_excludes_error_values_and_constraint_text() {
        let unreadable = EngineError::Unreadable("synthetic private stored value".into());
        assert_eq!(failure_class(&unreadable), ("unreadable", None));
        let check = EngineError::Db(orch_db::DbError::Check {
            table: "synthetic private table value".into(),
            constraint: "synthetic private constraint value".into(),
        });
        assert_eq!(failure_class(&check), ("db_check", Some("23514".into())));
        assert_eq!(
            failure_class(&EngineError::Db(orch_db::DbError::Retry)),
            ("db_retry", None)
        );
    }
}
