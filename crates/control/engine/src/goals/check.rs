//! Checking a report of "met": the helper model is asked outside any transaction, and its answer is applied under the
//! thread's lock to the goal as it stands then.
//!
//! A goal is held by one claim while its report is checked ([`orch_db::queries::goals::claim_check`]), so that two
//! processes do not ask twice and the loop waits. A claim that is not let go expires after the call's deadline, and
//! the next look asks again. A helper that cannot be asked is tried three times, a minute apart; after that the
//! agent's own report decides, and the line of the goal says that the check could not run.

use super::{GoalError, met_in};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::summaries::goal_check::{self, Checked};
use crate::unit::Unit;
use chrono::Duration;
use orch_api_contract::goals::GoalCheckedBy;
use orch_db::queries::goals::{self as rows, status};
use orch_db::queries::threads;
use orch_types::Id;

/// Seconds beyond the helper call's own deadline that its claim is kept, for the commits around the call.
const CLAIM_GRACE_SECS: i64 = 30;
/// How many times a helper that cannot be asked is tried before the agent's report decides.
const ATTEMPTS: i32 = 3;
/// Seconds between those tries, per try made.
const RETRY_SECS: i64 = 60;

/// How long a look's claim on a goal holds for a check: the helper's deadline and a little more.
pub(super) fn claim_for() -> Duration {
    Duration::seconds(
        i64::from(orch_link_contract::commands::CommandKind::ProviderComplete.deadline_secs())
            + CLAIM_GRACE_SECS,
    )
}

impl Engine {
    /// Starts the check of a report in a task of its own, after the commit of the look that claimed the goal.
    pub(super) fn spawn_goal_check(&self, thread: Id, goal: Id, report: Id) {
        let engine = self.clone();
        tokio::spawn(async move {
            if engine.is_stopped() {
                return;
            }
            if let Err(error) = engine.run_goal_check(thread, goal, report).await {
                tracing::warn!(%error, %thread, %goal, "a goal's report could not be checked; the claim expires and the next look asks again");
            }
        });
    }

    async fn run_goal_check(
        &self,
        thread: Id,
        goal_id: Id,
        report_id: Id,
    ) -> Result<(), EngineError> {
        let (goal, report) = {
            let mut conn = self.db().acquire().await?;
            let goal = rows::goal(&mut conn, goal_id).await?;
            let report = rows::report(&mut conn, report_id).await?;
            match (goal, report) {
                (Some(goal), Some(report))
                    if goal.status == status::ACTIVE && report.outcome.is_none() =>
                {
                    (goal, report)
                }
                _ => return Ok(()),
            }
        };
        let checked = goal_check::ask(self, thread, &goal, &report).await?;
        self.apply_goal_check(thread, &goal.condition, goal_id, report_id, checked)
            .await
            .map_err(|error| match error {
                GoalError::Internal(error) => error,
                other => EngineError::unreadable(other),
            })
    }

    /// Applies what the helper said to the goal as it stands now: not at all where the goal moved on meanwhile.
    async fn apply_goal_check(
        &self,
        thread_id: Id,
        condition: &str,
        goal_id: Id,
        report_id: Id,
        checked: Checked,
    ) -> Result<(), GoalError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, now);
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            let (Some(goal), Some(report)) = (
                rows::goal(&mut *unit.conn, goal_id).await?,
                rows::report(&mut *unit.conn, report_id).await?,
            ) else {
                return Ok(());
            };
            // The goal ended, was held or edited, or the report was settled: this answer is about something else.
            if goal.status != status::ACTIVE
                || goal.condition != condition
                || report.outcome.is_some()
            {
                return Ok(());
            }
            match checked {
                Checked::Met => {
                    met_in(
                        &mut unit,
                        &thread,
                        &goal,
                        &report,
                        GoalCheckedBy::Helper,
                        None,
                    )
                    .await?;
                }
                Checked::NotYet(reason) => {
                    rows::decide_report(
                        &mut *unit.conn,
                        report.id,
                        "not_yet",
                        None,
                        Some(&reason),
                        now,
                    )
                    .await?;
                    rows::release_check(&mut *unit.conn, goal.id, now).await?;
                    super::publish(&mut unit, thread_id).await?;
                    unit.wake(thread_id);
                }
                Checked::Unavailable(why) => {
                    let tried = rows::note_check_attempt(&mut *unit.conn, report.id).await?;
                    if tried >= ATTEMPTS {
                        let reason = format!("the check could not run: {why}");
                        met_in(
                            &mut unit,
                            &thread,
                            &goal,
                            &report,
                            GoalCheckedBy::Agent,
                            Some(&reason),
                        )
                        .await?;
                    } else {
                        rows::delay_check(
                            &mut *unit.conn,
                            goal.id,
                            now + Duration::seconds(RETRY_SECS * i64::from(tried)),
                            now,
                        )
                        .await?;
                    }
                }
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }
}
