//! The `goal` tool: the agent reads the goal of its thread and reports where it stands.
//!
//! A report is what the agent says, kept with the turn it was said in. What it comes to is decided when that turn has
//! ended and been counted (see `goals`): a report of work going on changes nothing, one of "blocked" pauses the goal,
//! and one of "met" is checked by the place's helper model, or taken as it stands where the place has none. The tool
//! is given only to a thread that has a goal under way.

use super::definitions::{Code, Refusal};
use super::{Answer, CallContext};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::goals::{EVIDENCE_MAX_CHARS, NEXT_MAX_CHARS, REPORTS_SHOWN};
use crate::unit::Unit;
use orch_db::queries::goals::{self as rows, GoalRow, ReportRow, status};
use orch_db::queries::threads::{self, ThreadRow};
use orch_types::{Json, new_id};
use serde_json::json;

fn minutes(goal: &GoalRow, now: chrono::DateTime<chrono::Utc>) -> i64 {
    goal.elapsed_at(now) / 60_000
}

fn report_json(report: &ReportRow) -> Json {
    json!({
        "turn": report.turn_ordinal,
        "status": report.status,
        "evidence": report.evidence,
        "next": report.next,
        "checked": report.outcome,
        "reason": report.reason,
        "at": report.created_at,
    })
}

impl Engine {
    /// Answers a call of the `goal` tool.
    pub(super) async fn goal_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        match call.arguments.get("command").and_then(Json::as_str) {
            Some("get") => self.goal_get(thread).await,
            Some("update") => self.goal_update(call, thread).await,
            _ => Ok(Refusal::invalid("command is get or update.").into()),
        }
    }

    async fn goal_get(&self, thread: &ThreadRow) -> Result<Answer, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(goal) = rows::live(&mut conn, thread.node_id).await? else {
            return Ok(Refusal::new(Code::NotFound, "This thread has no goal under way.").into());
        };
        let budgets = crate::goals::budgets(&mut conn, thread.node_id, &goal).await?;
        let reports = rows::reports(&mut conn, goal.id, REPORTS_SHOWN).await?;
        let now = self.inner.clock.now();
        Ok(Answer::Done(json!({
            "objective": goal.objective,
            "condition": goal.condition,
            "status": goal.status,
            "limits": {
                "max_turns": budgets.max_turns,
                "max_minutes": budgets.max_minutes,
                "max_credits": budgets.max_credits,
                "note": "0 is no limit",
            },
            "used": {
                "turns": goal.turns_used,
                "minutes": minutes(&goal, now),
                "tokens": goal.tokens_used,
                "credits": goal.credits_used,
            },
            "reports": reports.iter().map(report_json).collect::<Vec<_>>(),
        })))
    }

    async fn goal_update(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let arguments = &call.arguments;
        let Some(said) = arguments.get("status").and_then(Json::as_str) else {
            return Ok(Refusal::invalid("update takes a status: working, met or blocked.").into());
        };
        if !matches!(said, "working" | "met" | "blocked") {
            return Ok(Refusal::invalid("status is working, met or blocked.").into());
        }
        let evidence = arguments
            .get("evidence")
            .and_then(Json::as_str)
            .unwrap_or("")
            .trim();
        let next = arguments
            .get("next")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|words| !words.is_empty());
        if said != "working" && evidence.is_empty() {
            return Ok(Refusal::invalid(
                "met and blocked take evidence: what you ran or read and what it showed, or what stops you.",
            )
            .into());
        }
        if evidence.chars().count() > EVIDENCE_MAX_CHARS
            || next.is_some_and(|words| words.chars().count() > NEXT_MAX_CHARS)
        {
            return Ok(Refusal::invalid(format!(
                "evidence is at most {EVIDENCE_MAX_CHARS} characters and next at most {NEXT_MAX_CHARS}: say the part that shows it."
            ))
            .into());
        }
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let (finished, helper) = {
            let mut unit = Unit::new(&mut tx, now);
            let Some(locked) = threads::lock(&mut *unit.conn, thread.node_id).await? else {
                return Ok(Refusal::new(Code::NotFound, "This thread is gone.").into());
            };
            let Some(goal) = rows::live(&mut *unit.conn, locked.node_id).await? else {
                return Ok(
                    Refusal::new(Code::NotFound, "This thread has no goal under way.").into(),
                );
            };
            if goal.status != status::ACTIVE {
                return Ok(Refusal::invalid(
                    "The goal is paused: a person sets it going again. Say in your answer where you stand.",
                )
                .into());
            }
            let report = new_id();
            rows::insert_report(
                &mut *unit.conn,
                &rows::NewReport {
                    id: report,
                    goal_id: goal.id,
                    thread_id: locked.node_id,
                    turn_id: Some(call.turn),
                    status: said,
                    evidence,
                    next,
                    at: now,
                },
            )
            .await?;
            if said != "working" {
                rows::note_older_reports(&mut *unit.conn, goal.id, report, now).await?;
            }
            let helper =
                crate::summaries::goal_helper_available(&mut *unit.conn, locked.node_id).await?;
            let seq = unit.stamp(locked.node_id).await?;
            threads::set_seq_state(&mut *unit.conn, locked.node_id, seq).await?;
            (unit.finish().await?, helper)
        };
        self.commit(tx, finished).await?;
        let note = match (said, helper) {
            ("met", true) => {
                "Recorded. When this turn ends Orch checks the evidence against the condition; if it does not show it, you are told why at the start of your next turn."
            }
            ("met", false) => "Recorded. When this turn ends the goal is met on your word.",
            ("blocked", _) => "Recorded. When this turn ends the goal waits for a person.",
            _ => "Recorded. Go on.",
        };
        Ok(Answer::Done(
            json!({ "recorded": true, "status": said, "note": note }),
        ))
    }
}
