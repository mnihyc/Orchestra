//! The goal loop: what a turn that ended comes to for the goal, and the continuation that follows.
//!
//! One look at a thread ([`Engine::drive_goal`]) holds the thread's row and does, in this order: counts the turns that
//! ended into the goal's figures; pauses it where a person stopped the turn or a turn failed with nothing recovering
//! it; acts on the agent's latest report (blocked pauses, met is checked or taken); stops the goal at a limit that is
//! set; and otherwise queues the next input in the name of the person who set the goal. Every step is safe to repeat:
//! the figures move only past turns not yet counted, a report is decided once, and the continuation's identity is
//! derived from the goal and the turns it has used, so a look made twice queues one input.

use super::{Budgets, budgets, is_zero, met_in, pause_in, stop_in, text};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::inputs::{InputError, InputRequest, Made};
use crate::unit::Unit;
use orch_api_contract::goals::{GoalCheckedBy, GoalInputFacts, GoalReason};
use orch_api_contract::threads::{ContentPart, Delivery};
use orch_authz::Caller;
use orch_db::queries::goals::{self as rows, GoalRow, status};
use orch_db::queries::threads::{self, ThreadRow, thread_state};
use orch_provider_contract::calls::Purpose;
use orch_types::Id;

/// How many ended turns one look counts: a goal that was away for long catches up over several looks.
const COUNTED_AT_ONCE: i64 = 64;

/// What a look at a goal leaves for after its commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalWork {
    /// The helper model is to check this report of "met", under the claim the look took.
    Check {
        /// The goal.
        goal: Id,
        /// The report.
        report: Id,
    },
}

fn map_input(error: InputError) -> EngineError {
    match error {
        InputError::Internal(error) => error,
        other => EngineError::unreadable(other),
    }
}

impl Engine {
    /// Looks at the goal of a thread, if it has one that is active, and does what is due.
    pub(crate) async fn drive_goal(&self, thread_id: Id) -> Result<(), EngineError> {
        if self.is_stopped() {
            return Ok(());
        }
        {
            let mut conn = self.inner.db.acquire().await?;
            if rows::live(&mut conn, thread_id)
                .await?
                .is_none_or(|goal| goal.status != status::ACTIVE)
            {
                return Ok(());
            }
        }
        let mut tx = self.inner.db.begin().await?;
        let (finished, work) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            let work = self.goal_step(&mut unit, &thread).await?;
            (unit.finish().await?, work)
        };
        self.commit(tx, finished).await?;
        if let Some(GoalWork::Check { goal, report }) = work {
            self.spawn_goal_check(thread_id, goal, report);
        }
        Ok(())
    }

    async fn goal_step(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<Option<GoalWork>, EngineError> {
        let Some(goal) = rows::live(unit.conn, thread.node_id).await? else {
            return Ok(None);
        };
        if goal.status != status::ACTIVE || thread.deleted_at.is_some() {
            return Ok(None);
        }
        if thread.archived_at.is_some() {
            stop_in(unit, thread, &goal, GoalReason::Archived, None).await?;
            return Ok(None);
        }
        let now = unit.now();
        // 1. What the turns that ended used.
        let ended = rows::ended_turns_after(
            unit.conn,
            thread.node_id,
            goal.counted_turn,
            COUNTED_AT_ONCE,
        )
        .await?;
        let counted = ended.iter().take_while(|turn| !turn.end_pending).count();
        rows::add_usage_batch(unit.conn, goal.id, &ended[..counted], now).await?;
        let last = ended.into_iter().take(counted).next_back();
        let goal = rows::goal(unit.conn, goal.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the goal is gone"))?;
        // 2. A turn a person stopped holds the goal: they said stop. A turn that a person's message cut (it has the same
        // cause) is no stop: that message runs next, and the goal goes on after it.
        if let Some(turn) = &last
            && turn.state == threads::turn_state::INTERRUPTED
            && turn.interrupt_cause.as_deref() == Some("person")
            && (thread.queue_held
                || (thread.current_turn_id.is_none()
                    && threads::count_pending_inputs(unit.conn, thread.node_id).await? == 0))
        {
            pause_in(
                unit,
                thread,
                &goal,
                GoalReason::Interrupted,
                None,
                turn.interrupted_by,
            )
            .await?;
            return Ok(None);
        }
        // 3. A helper that is checking a report has the goal until it answers.
        if goal.checking_until.is_some_and(|until| until > now) {
            return Ok(None);
        }
        // 4. What the agent said, once its turn has ended and been counted.
        if let Some(report) = rows::latest_report(unit.conn, goal.id).await?
            && report.outcome.is_none()
            && report
                .turn_ordinal
                .is_none_or(|ordinal| ordinal <= goal.counted_turn)
        {
            match report.status.as_str() {
                "met" => {
                    if crate::summaries::goal_helper_available(unit.conn, thread.node_id).await? {
                        let until = now + super::check::claim_for();
                        if rows::claim_check(unit.conn, goal.id, until, now).await? {
                            return Ok(Some(GoalWork::Check {
                                goal: goal.id,
                                report: report.id,
                            }));
                        }
                        return Ok(None);
                    }
                    met_in(unit, thread, &goal, &report, GoalCheckedBy::Agent, None).await?;
                    return Ok(None);
                }
                "blocked" => {
                    rows::decide_report(unit.conn, report.id, "blocked", None, None, now).await?;
                    let detail = [
                        report.evidence.trim(),
                        report.next.as_deref().unwrap_or("").trim(),
                    ]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                    pause_in(
                        unit,
                        thread,
                        &goal,
                        GoalReason::Blocked,
                        (!detail.is_empty()).then_some(detail.as_str()),
                        None,
                    )
                    .await?;
                    return Ok(None);
                }
                _ => {}
            }
        }
        // 5. A turn that failed, and that nothing recovers, is for a person to look at.
        if let Some(turn) = &last
            && turn.state == threads::turn_state::FAILED
            && self.recovery_in_hand(unit, thread).await?.is_none()
        {
            pause_in(unit, thread, &goal, GoalReason::Failed, None, None).await?;
            return Ok(None);
        }
        // 6. A limit that is set ends the goal.
        if let Some((reason, shown)) = self.limit_reached(unit, thread, &goal).await? {
            stop_in(unit, thread, &goal, reason, Some(shown)).await?;
            return Ok(None);
        }
        // 7. The next turn.
        self.continue_goal_in(unit, thread, &goal, None).await?;
        Ok(None)
    }

    /// Whether the thread's own recovery of a failed call has work in hand or a stop that needs a person: its state.
    async fn recovery_in_hand(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<Option<String>, EngineError> {
        Ok(
            crate::calls::read(unit.conn, thread.node_id, Purpose::Agent)
                .await?
                .map(|recovery| recovery.state)
                .filter(|state| {
                    matches!(
                        state.as_str(),
                        "scheduled" | "switching" | "running" | "stopped" | "waiting"
                    )
                }),
        )
    }

    /// The limit of the goal that is reached, with how it is shown: the number it counts to.
    pub(super) async fn limit_reached(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        goal: &GoalRow,
    ) -> Result<Option<(GoalReason, String)>, EngineError> {
        let Budgets {
            max_turns,
            max_minutes,
            max_credits,
            ..
        } = budgets(unit.conn, thread.node_id, goal).await?;
        if max_turns > 0 && goal.turns_used >= max_turns {
            return Ok(Some((GoalReason::Turns, max_turns.to_string())));
        }
        if max_minutes > 0 && goal.elapsed_at(unit.now()) >= max_minutes.saturating_mul(60_000) {
            return Ok(Some((GoalReason::Minutes, max_minutes.to_string())));
        }
        if !is_zero(&max_credits) && rows::credits_reached(unit.conn, goal.id, &max_credits).await?
        {
            return Ok(Some((GoalReason::Credits, max_credits)));
        }
        Ok(None)
    }

    /// Queues the next turn of a goal where nothing else is in the way, in the name of the person who set it, with
    /// every check an input passes. Says whether an input was queued. `caller` is the person at the moment of
    /// setting; the loop's own looks reconstruct the person from what their credential allowed then.
    pub(super) async fn continue_goal_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        goal: &GoalRow,
        caller: Option<&Caller>,
    ) -> Result<bool, EngineError> {
        if thread.current_turn_id.is_some()
            || thread.state != thread_state::IDLE
            || thread.queue_held
            || crate::locks::held_by(unit.conn, thread.node_id)
                .await?
                .is_some()
            || threads::count_pending_inputs(unit.conn, thread.node_id).await? > 0
            || rows::asks_a_person(unit.conn, thread.node_id).await?
            || threads::turn_end_work_pending(unit.conn, thread.node_id).await?
            || self.recovery_in_hand(unit, thread).await?.is_some()
        {
            return Ok(false);
        }
        let caller = match caller {
            Some(caller) => caller.clone(),
            None => match self.goal_caller(unit, thread, goal).await? {
                Some(caller) => caller,
                None => {
                    stop_in(unit, thread, goal, GoalReason::Access, None).await?;
                    return Ok(false);
                }
            },
        };
        // The person who set it must still drive the thread and use its model: their access answers for the turn.
        match self.check_sender(unit, &caller, thread, true).await {
            Ok(()) => {}
            Err(InputError::Denied(_)) => {
                stop_in(unit, thread, goal, GoalReason::Access, None).await?;
                return Ok(false);
            }
            Err(other) => return Err(map_input(other)),
        }
        let limits = budgets(unit.conn, thread.node_id, goal).await?;
        let not_yet = rows::latest_report(unit.conn, goal.id)
            .await?
            .filter(|report| {
                report.outcome.as_deref() == Some("not_yet")
                    && report
                        .turn_ordinal
                        .is_some_and(|ordinal| ordinal == goal.counted_turn)
            })
            .and_then(|report| report.reason);
        let plan = goal.plan_first
            && goal.turns_used == 0
            && super::agent_plans(unit.conn, thread).await?;
        let reports = super::agent_reports(unit.conn, thread, unit.now()).await?;
        let facts = GoalInputFacts {
            goal_id: goal.id,
            condition: goal.condition.clone(),
            turn: goal.turns_used + 1,
            max_turns: limits.max_turns,
            tokens: goal.tokens_used,
            not_yet,
            plan,
        };
        let attempts = rows::lost_inputs(unit.conn, goal.id).await?;
        let request = InputRequest {
            content: vec![ContentPart::Text {
                text: text::input_text(&facts, &goal.objective, reports),
            }],
            delivery: Delivery::Queue,
            client_id: format!("goal:{}:{}:{attempts}", goal.id, goal.turns_used),
            bypass: None,
            plan,
        };
        let stored = serde_json::to_value(&facts)?;
        let made = Made {
            source: "goal",
            goal: Some((goal.id, &stored)),
        };
        match self
            .submit_made_in(unit, &caller, thread.node_id, &request, made)
            .await
        {
            Ok(taken) => Ok(taken.submitted.created),
            Err(InputError::Archived | InputError::NotFound) => {
                stop_in(unit, thread, goal, GoalReason::Archived, None).await?;
                Ok(false)
            }
            Err(InputError::Denied(_)) => {
                stop_in(unit, thread, goal, GoalReason::Access, None).await?;
                Ok(false)
            }
            Err(other) => Err(map_input(other)),
        }
    }
}
