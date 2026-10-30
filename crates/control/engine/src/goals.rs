//! A thread's goal: set, held, edited and cleared by whoever drives the thread, kept going by Orch's own loop, and ended when its condition holds.
//!
//! A goal is an objective and a condition ("Done when"). Orch's loop is the same for every agent program: when a turn
//! ends and nothing else needs doing first, Orch queues the next input in the name of the person who set the goal, so
//! that it passes every check any input passes (caps, holds, approvals, recovery). A program's own goal feature stays
//! off, because it would start turns Orch did not check.
//!
//! **Who may.** Whoever may drive the thread (`thread.interact`) sets, pauses, edits and clears the goal; setting one
//! also takes their own use of the thread's model, since the continuations run on their access. Every act is named in
//! the thread's history and audited.
//!
//! **The loop** ([`drive`]) runs from the engine's look at a thread, after the end of a turn has been accounted. It
//! counts the turns that ended into the goal's figures, acts on what the agent reported with its `goal` tool, stops the
//! goal at a limit that is set, and otherwise queues one continuation. It waits while a question, an approval or a
//! plan asks a person, while the queue is held or holds inputs, while the thread is blocked and while call recovery
//! has work in hand.
//!
//! **"Met"** is the agent's report, checked by the place's helper model against the condition where there is one
//! ([`check`]); a "not yet" starts the next turn with its reason. Without a helper model the report decides, and the
//! goal's line says so.
//!
//! **Limits** (`thread.goal.max_turns`, `thread.goal.max_minutes` and `thread.goal.max_credits`) default to none. A
//! goal's own limit stands unless a place locked the setting; reaching one that is set stops the goal and says which.

mod check;
mod drive;
pub mod text;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::inputs::{self, InputError};
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::Refusal;
use orch_api_contract::goals::{
    GOAL_CONDITION_MAX_CHARS, GOAL_OBJECTIVE_MAX_CHARS, GoalAction, GoalBudgets, GoalCheckedBy,
    GoalLimits, GoalOutcome, GoalReason, GoalReport, GoalReportStatus, GoalStatus, GoalUsed,
    GoalWaiting, ThreadGoal, ThreadGoalCreateBody, ThreadGoalUpdateBody,
};
use orch_api_contract::users::UserRef;
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_db::queries::goals::{self as rows, GoalRow, ReportRow, status};
use orch_db::queries::threads::{self, ThreadRow, thread_state};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;

/// How many of the newest reports the agent's `get` gives back.
pub const REPORTS_SHOWN: i64 = 10;

/// The most characters of what an agent writes as evidence of a report: enough for the output of a test run.
pub const EVIDENCE_MAX_CHARS: usize = 4_000;
/// The most characters of what an agent writes as its next step.
pub const NEXT_MAX_CHARS: usize = 1_000;

/// Why a goal could not be set, changed or cleared.
#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    /// There is no such thread, or it has no goal to act on.
    #[error("there is no such thread or goal")]
    NotFound,
    /// The caller lacks something, and this is all of it.
    #[error("the caller lacks what a goal needs")]
    Denied(Box<Refusal>),
    /// The thread, or a place above it, is archived.
    #[error("the thread is archived")]
    Archived,
    /// The thread has a goal under way.
    #[error("a goal is under way here already")]
    Exists,
    /// A limit of the goal is reached, so that it cannot go on until it is raised.
    #[error("a limit of the goal is reached")]
    LimitReached,
    /// The request is not one that can be made.
    #[error("{0}")]
    Invalid(&'static str),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<InputError> for GoalError {
    fn from(error: InputError) -> Self {
        match error {
            InputError::NotFound => Self::NotFound,
            InputError::Denied(refusal) => Self::Denied(refusal),
            InputError::Archived => Self::Archived,
            InputError::Invalid(words) => Self::Invalid(words),
            InputError::Internal(error) => Self::Internal(error),
            _ => Self::Invalid("the goal's input could not be taken"),
        }
    }
}

impl From<orch_db::DbError> for GoalError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// The limits that apply to a goal, with the settings and their locks taken into account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budgets {
    /// The most turns; zero is no limit.
    pub max_turns: i64,
    /// The most minutes; zero is no limit.
    pub max_minutes: i64,
    /// The most credits, as decimal text; zero is no limit.
    pub max_credits: String,
    /// Whether a place locked one of the settings.
    pub locked: bool,
}

/// Whether `text` is a decimal number written as digits with at most one point.
#[must_use]
pub fn is_decimal(text: &str) -> bool {
    let mut parts = text.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
        && text.len() <= 32
}

/// Whether the decimal text is zero: no limit.
#[must_use]
pub fn is_zero(text: &str) -> bool {
    text.bytes().all(|byte| byte == b'0' || byte == b'.')
}

fn word_of<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn parse<T: serde::de::DeserializeOwned>(word: &str) -> Option<T> {
    serde_json::from_value(Json::String(word.to_owned())).ok()
}

/// What the settings say of the limits of a goal on `thread`, and whether a place locked any of them.
async fn setting_limits(
    conn: &mut Conn,
    thread: Id,
) -> Result<(i64, i64, String, [bool; 3]), EngineError> {
    let turns =
        orch_settings::resolve(conn, Place::Node(thread), keys::THREAD_GOAL_MAX_TURNS).await?;
    let minutes =
        orch_settings::resolve(conn, Place::Node(thread), keys::THREAD_GOAL_MAX_MINUTES).await?;
    let credits =
        orch_settings::resolve(conn, Place::Node(thread), keys::THREAD_GOAL_MAX_CREDITS).await?;
    let whole = |value: Option<&Json>| value.and_then(Json::as_i64).unwrap_or(0).max(0);
    let decimal = credits
        .value()
        .and_then(Json::as_str)
        .filter(|text| is_decimal(text))
        .unwrap_or("0")
        .to_owned();
    Ok((
        whole(turns.value()),
        whole(minutes.value()),
        decimal,
        [
            turns.resolved.locked_by.is_some(),
            minutes.resolved.locked_by.is_some(),
            credits.resolved.locked_by.is_some(),
        ],
    ))
}

/// The limits that apply to `goal`: its own where the place has not locked the setting, else the setting's.
pub(crate) async fn budgets(
    conn: &mut Conn,
    thread: Id,
    goal: &GoalRow,
) -> Result<Budgets, EngineError> {
    let (turns, minutes, credits, locked) = setting_limits(conn, thread).await?;
    Ok(Budgets {
        max_turns: if locked[0] {
            turns
        } else {
            goal.max_turns.unwrap_or(turns)
        },
        max_minutes: if locked[1] {
            minutes
        } else {
            goal.max_minutes.unwrap_or(minutes)
        },
        max_credits: if locked[2] {
            credits
        } else {
            goal.max_credits
                .clone()
                .filter(|text| is_decimal(text))
                .unwrap_or(credits)
        },
        locked: locked.iter().any(|held| *held),
    })
}

/// Whether the thread's agent is given the goal tool: its program takes Orch's tools over MCP.
async fn agent_reports(
    conn: &mut Conn,
    thread: &ThreadRow,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    let Some(agent) = thread.agent_id else {
        return Ok(false);
    };
    let Some(agent) = orch_db::queries::resources::thread_engine::agent(conn, agent).await? else {
        return Ok(false);
    };
    let takes = crate::harnesses::descriptor_of(&agent)?.is_some_and(|descriptor| {
        descriptor.capabilities.orch_tools == orch_agent_contract::descriptor::OrchToolsSupport::Mcp
    });
    if !takes {
        return Ok(false);
    }
    // The environment may switch Orch's tools off for the agents that work in it.
    Ok(
        crate::tools::offered_for_session(conn, thread, true, Time::new(now))
            .await?
            .contains(&orch_agent_contract::events::OrchTool::Goal),
    )
}

/// Whether the thread's agent program has a plan mode.
pub(crate) async fn agent_plans(conn: &mut Conn, thread: &ThreadRow) -> Result<bool, EngineError> {
    let Some(agent) = thread.agent_id else {
        return Ok(false);
    };
    let Some(agent) = orch_db::queries::resources::thread_engine::agent(conn, agent).await? else {
        return Ok(false);
    };
    Ok(crate::harnesses::descriptor_of(&agent)?
        .is_some_and(|descriptor| descriptor.capabilities.plan_mode))
}

fn report_view(row: &ReportRow) -> Option<GoalReport> {
    Some(GoalReport {
        id: row.id,
        turn: row.turn_ordinal.map(i64::from),
        status: parse::<GoalReportStatus>(&row.status)?,
        evidence: row.evidence.clone(),
        next: row.next.clone(),
        outcome: row.outcome.as_deref().and_then(parse::<GoalOutcome>),
        checked_by: row.checked_by.as_deref().and_then(parse::<GoalCheckedBy>),
        reason: row.reason.clone(),
        at: Time::new(row.created_at),
    })
}

/// What an active goal waits for while it does not work.
async fn waiting_of(
    conn: &mut Conn,
    thread: &ThreadRow,
) -> Result<Option<GoalWaiting>, EngineError> {
    if rows::asks_a_person(conn, thread.node_id).await? {
        let plan: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM approvals WHERE thread_id = $1 AND state = 'pending' AND kind = 'plan')",
        )
        .bind(thread.node_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        return Ok(Some(if plan {
            GoalWaiting::Plan
        } else {
            GoalWaiting::Approval
        }));
    }
    if thread.state == thread_state::BLOCKED {
        return Ok(Some(GoalWaiting::Blocked));
    }
    if thread.queue_held {
        return Ok(Some(GoalWaiting::Held));
    }
    Ok(None)
}

/// A person as the wire names them, where they are still there.
async fn person(conn: &mut Conn, user: Option<Id>) -> Result<Option<UserRef>, EngineError> {
    let Some(user) = user else {
        return Ok(None);
    };
    let reference = crate::views::user_ref_json(conn, user).await?;
    Ok(serde_json::from_value(reference).ok())
}

/// The goal a thread shows, as the API has it: the one under way, else the last that ended and was not cleared.
pub async fn view(
    conn: &mut Conn,
    thread: Id,
    now: DateTime<Utc>,
) -> Result<Option<ThreadGoal>, EngineError> {
    let Some(row) = rows::current(conn, thread).await? else {
        return Ok(None);
    };
    let Some(thread) = threads::thread(conn, thread).await? else {
        return Ok(None);
    };
    Ok(Some(view_of(conn, &thread, &row, now).await?))
}

/// A goal row as the API has it.
pub(crate) async fn view_of(
    conn: &mut Conn,
    thread: &ThreadRow,
    row: &GoalRow,
    now: DateTime<Utc>,
) -> Result<ThreadGoal, EngineError> {
    let budgets = budgets(conn, thread.node_id, row).await?;
    let live = row.status == status::ACTIVE;
    let checking = live && row.checking_until.is_some_and(|until| until > now);
    let waiting = if live && !checking {
        waiting_of(conn, thread).await?
    } else {
        None
    };
    let latest = rows::latest_report(conn, row.id).await?;
    Ok(ThreadGoal {
        id: row.id,
        objective: row.objective.clone(),
        condition: row.condition.clone(),
        set_by: person(conn, row.set_by).await?,
        set_at: Time::new(row.set_at),
        status: parse::<GoalStatus>(&row.status).unwrap_or(GoalStatus::Stopped),
        reason: row.reason.as_deref().and_then(parse::<GoalReason>),
        detail: row.detail.clone(),
        waiting,
        checking,
        plan_first: row.plan_first,
        notify_on_end: row.notify_on_end,
        limits: GoalLimits {
            max_turns: row.max_turns,
            max_minutes: row.max_minutes,
            max_credits: row.max_credits.clone(),
        },
        budgets: GoalBudgets {
            max_turns: budgets.max_turns,
            max_minutes: budgets.max_minutes,
            max_credits: budgets.max_credits,
            locked: budgets.locked,
        },
        used: GoalUsed {
            turns: row.turns_used,
            elapsed_secs: row.elapsed_ms / 1_000,
            running_since: row.active_since.map(Time::new),
            tokens: row.tokens_used,
            credits: row.credits_used.clone(),
        },
        helper: crate::summaries::goal_helper_available(conn, thread.node_id).await?,
        agent_reports: agent_reports(conn, thread, now).await?,
        last_report: latest.as_ref().and_then(report_view),
        ended_at: row.ended_at.map(Time::new),
    })
}

/// Whether the thread has a goal under way, so that its agent is given the goal tool.
pub(crate) async fn offers_tool(conn: &mut Conn, thread: Id) -> Result<bool, EngineError> {
    Ok(rows::live(conn, thread).await?.is_some())
}

// ---------------------------------------------------------------- the words of a request

fn trimmed(
    words: &str,
    max: u32,
    empty: &'static str,
    long: &'static str,
) -> Result<String, GoalError> {
    let words = words.trim();
    if words.is_empty() {
        return Err(GoalError::Invalid(empty));
    }
    if words.chars().count() > usize::try_from(max).unwrap_or(usize::MAX) {
        return Err(GoalError::Invalid(long));
    }
    Ok(words.to_owned())
}

fn checked_limits(limits: &GoalLimits) -> Result<(), GoalError> {
    if limits.max_turns.is_some_and(|turns| turns < 0)
        || limits.max_minutes.is_some_and(|minutes| minutes < 0)
    {
        return Err(GoalError::Invalid(
            "a limit is zero, which is no limit, or more",
        ));
    }
    if limits
        .max_credits
        .as_deref()
        .is_some_and(|credits| !is_decimal(credits))
    {
        return Err(GoalError::Invalid(
            "the credits are written as a decimal number",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- what a goal leaves behind

/// A line in the thread's history, once: where the thread has a segment to write it in.
async fn marker(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    kind: &str,
    data: Json,
) -> Result<(), EngineError> {
    let Some(segment) = thread.current_segment_id else {
        return Ok(());
    };
    let key = format!("goal:{kind}:{}:{}", new_id(), unit.now().timestamp_millis());
    crate::ingest::add_marker(
        unit,
        thread.node_id,
        segment,
        None,
        &json!({ "marker": kind, "data": data }),
        &key,
    )
    .await
}

/// Publishes the thread's row to its readers: the goal is part of it.
async fn publish(unit: &mut Unit<'_>, thread: Id) -> Result<(), EngineError> {
    let seq = unit.stamp(thread).await?;
    threads::set_seq_state(unit.conn, thread, seq).await?;
    Ok(())
}

async fn by(conn: &mut Conn, user: Id) -> Result<Json, EngineError> {
    crate::views::user_ref_json(conn, user).await
}

async fn audit(
    unit: &mut Unit<'_>,
    thread: Id,
    user: Option<Id>,
    action: &str,
    details: Json,
) -> Result<(), EngineError> {
    let now = unit.now();
    match user {
        Some(user) => {
            crate::audit_facts::person(unit.conn, user, thread, action, details, now).await
        }
        None => crate::audit_facts::system(unit.conn, thread, action, details, now).await,
    }
}

/// Tells the person who set the goal that it ended, where they asked to be.
async fn tell_end(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    goal: &GoalRow,
    how: &str,
    reason: Option<&str>,
) -> Result<(), EngineError> {
    let Some(person) = goal.set_by.filter(|_| goal.notify_on_end) else {
        return Ok(());
    };
    let title = if how == "met" {
        format!("Goal met on {}", thread.name)
    } else {
        format!("Goal stopped on {}", thread.name)
    };
    let event = crate::notifications::Event {
        kind: crate::notifications::Kind::GoalEnded,
        node: Some(thread.node_id),
        thread: Some(thread.node_id),
        approval: None,
        title,
        body: goal.condition.clone(),
        data: json!({ "how": how, "reason": reason, "turns": goal.turns_used, "goal_id": goal.id }),
        source: format!("goal-ended:{}", goal.id),
        actor: None,
        at: unit.now(),
    };
    crate::notifications::tell(unit.conn, &event, &[person], Some(thread.org_id)).await?;
    Ok(())
}

/// Holds an active goal.
pub(crate) async fn pause_in(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    goal: &GoalRow,
    reason: GoalReason,
    detail: Option<&str>,
    by_user: Option<Id>,
) -> Result<bool, EngineError> {
    let now = unit.now();
    if !rows::pause(unit.conn, goal.id, &word_of(&reason), detail, now).await? {
        return Ok(false);
    }
    let who = match by_user {
        Some(user) => Some(by(unit.conn, user).await?),
        None => None,
    };
    marker(
        unit,
        thread,
        "goal_paused",
        json!({ "goal_id": goal.id, "by": who, "reason": reason, "detail": detail }),
    )
    .await?;
    audit(
        unit,
        thread.node_id,
        by_user,
        orch_db::audit::name!("thread.goal_changed"),
        json!({ "goal_id": goal.id, "action": "pause", "reason": reason }),
    )
    .await?;
    publish(unit, thread.node_id).await?;
    Ok(true)
}

/// Ends a goal that was met, with the evidence that showed it.
pub(crate) async fn met_in(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    goal: &GoalRow,
    report: &ReportRow,
    checked_by: GoalCheckedBy,
    reason: Option<&str>,
) -> Result<bool, EngineError> {
    let now = unit.now();
    if !rows::end(unit.conn, goal.id, status::MET, None, None, now).await? {
        return Ok(false);
    }
    rows::decide_report(
        unit.conn,
        report.id,
        "met",
        Some(&word_of(&checked_by)),
        reason,
        now,
    )
    .await?;
    marker(
        unit,
        thread,
        "goal_met",
        json!({
            "goal_id": goal.id, "condition": goal.condition, "evidence": report.evidence,
            "checked_by": checked_by, "reason": reason, "turns": goal.turns_used,
        }),
    )
    .await?;
    audit(
        unit,
        thread.node_id,
        None,
        orch_db::audit::name!("thread.goal_ended"),
        json!({ "goal_id": goal.id, "how": "met", "checked_by": checked_by }),
    )
    .await?;
    tell_end(unit, thread, goal, "met", None).await?;
    publish(unit, thread.node_id).await?;
    Ok(true)
}

/// Ends a goal without its condition: a limit was reached, or nobody may go on.
pub(crate) async fn stop_in(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    goal: &GoalRow,
    reason: GoalReason,
    limit: Option<String>,
) -> Result<bool, EngineError> {
    let now = unit.now();
    let word = word_of(&reason);
    if !rows::end(unit.conn, goal.id, status::STOPPED, Some(&word), None, now).await? {
        return Ok(false);
    }
    marker(
        unit,
        thread,
        "goal_stopped",
        json!({ "goal_id": goal.id, "reason": reason, "turns": goal.turns_used, "limit": limit }),
    )
    .await?;
    audit(
        unit,
        thread.node_id,
        None,
        orch_db::audit::name!("thread.goal_ended"),
        json!({ "goal_id": goal.id, "how": "stopped", "reason": reason }),
    )
    .await?;
    tell_end(unit, thread, goal, "stopped", Some(&word)).await?;
    publish(unit, thread.node_id).await?;
    Ok(true)
}

// ---------------------------------------------------------------- what a person does

impl Engine {
    /// Sets a goal on a thread in the caller's name, and lets Orch's loop begin on it. The caller drives the thread
    /// and may use its model themself, since each continuation runs on their access.
    pub async fn set_goal_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        body: &ThreadGoalCreateBody,
    ) -> Result<ThreadGoal, GoalError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        let objective = trimmed(
            &body.objective,
            GOAL_OBJECTIVE_MAX_CHARS,
            "a goal says what the work is",
            "the objective is longer than a goal may hold",
        )?;
        let condition = trimmed(
            &body.condition,
            GOAL_CONDITION_MAX_CHARS,
            "a goal says when it is done",
            "the condition is longer than a goal may hold",
        )?;
        let limits = body.limits.clone().unwrap_or_default();
        checked_limits(&limits)?;
        let (finished, reply) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(GoalError::NotFound)?;
            if thread.archived_at.is_some() {
                return Err(GoalError::Archived);
            }
            self.check_sender(&mut unit, &caller, &thread, true).await?;
            if rows::live(&mut *unit.conn, thread_id).await?.is_some() {
                return Err(GoalError::Exists);
            }
            let after_turn: i32 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(ordinal), 0) FROM turns WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
            let authority = inputs::input_authority(&caller)?;
            let plan_first = body.plan && agent_plans(&mut *unit.conn, &thread).await?;
            let goal = rows::insert(
                &mut *unit.conn,
                &rows::NewGoal {
                    id: new_id(),
                    thread_id,
                    objective: &objective,
                    condition: &condition,
                    set_by: caller.user(),
                    set_authority: &authority,
                    plan_first,
                    notify_on_end: body.notify_on_end,
                    max_turns: limits.max_turns,
                    max_minutes: limits.max_minutes,
                    max_credits: limits.max_credits.as_deref(),
                    after_turn,
                    at: now,
                },
            )
            .await?;
            let who = by(&mut *unit.conn, caller.user()).await?;
            marker(
                &mut unit,
                &thread,
                "goal_set",
                json!({ "goal_id": goal.id, "by": who, "objective": objective, "condition": condition }),
            )
            .await?;
            audit(
                &mut unit,
                thread_id,
                Some(caller.user()),
                orch_db::audit::name!("thread.goal_set"),
                json!({ "goal_id": goal.id, "limits": limits, "plan": plan_first }),
            )
            .await?;
            // The first turn begins at once where nothing else is in the way; otherwise the loop finds its time.
            self.continue_goal_in(&mut unit, &thread, &goal, Some(&caller))
                .await?;
            publish(&mut unit, thread_id).await?;
            unit.wake(thread_id);
            let thread = threads::thread(&mut *unit.conn, thread_id)
                .await?
                .ok_or(GoalError::NotFound)?;
            let row = rows::goal(&mut *unit.conn, goal.id)
                .await?
                .ok_or(GoalError::NotFound)?;
            let reply = view_of(&mut *unit.conn, &thread, &row, now).await?;
            (unit.finish().await?, reply)
        };
        self.commit(tx, finished).await?;
        Ok(reply)
    }

    /// Edits a goal, pauses it, or sets it going again, for whoever drives the thread.
    pub async fn update_goal_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        body: &ThreadGoalUpdateBody,
    ) -> Result<ThreadGoal, GoalError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        let objective = body
            .objective
            .as_deref()
            .map(|words| {
                trimmed(
                    words,
                    GOAL_OBJECTIVE_MAX_CHARS,
                    "a goal says what the work is",
                    "the objective is longer than a goal may hold",
                )
            })
            .transpose()?;
        let condition = body
            .condition
            .as_deref()
            .map(|words| {
                trimmed(
                    words,
                    GOAL_CONDITION_MAX_CHARS,
                    "a goal says when it is done",
                    "the condition is longer than a goal may hold",
                )
            })
            .transpose()?;
        if let Some(limits) = &body.limits {
            checked_limits(limits)?;
        }
        let (finished, reply) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(GoalError::NotFound)?;
            if thread.archived_at.is_some() {
                return Err(GoalError::Archived);
            }
            self.check_sender(&mut unit, &caller, &thread, false)
                .await?;
            let goal = rows::current(&mut *unit.conn, thread_id)
                .await?
                .ok_or(GoalError::NotFound)?;
            if goal.status == status::MET {
                return Err(GoalError::Invalid("a goal that is met is not changed"));
            }
            // Words that are what they were are no edit, and leave no line in the history.
            let objective = objective.filter(|words| *words != goal.objective);
            let condition = condition.filter(|words| *words != goal.condition);
            let edits = objective.is_some()
                || condition.is_some()
                || body.limits.is_some()
                || body.notify_on_end.is_some();
            if edits {
                let limits = body.limits.as_ref();
                rows::edit(
                    &mut *unit.conn,
                    goal.id,
                    &rows::GoalEdit {
                        objective: objective.as_deref(),
                        condition: condition.as_deref(),
                        max_turns: limits.map(|limits| limits.max_turns),
                        max_minutes: limits.map(|limits| limits.max_minutes),
                        max_credits: limits.map(|limits| limits.max_credits.as_deref()),
                        notify_on_end: body.notify_on_end,
                    },
                    now,
                )
                .await?;
                if objective.is_some() || condition.is_some() {
                    let who = by(&mut *unit.conn, caller.user()).await?;
                    marker(
                        &mut unit,
                        &thread,
                        "goal_changed",
                        json!({
                            "goal_id": goal.id, "by": who,
                            "condition": condition.as_deref().unwrap_or(&goal.condition),
                        }),
                    )
                    .await?;
                }
                audit(
                    &mut unit,
                    thread_id,
                    Some(caller.user()),
                    orch_db::audit::name!("thread.goal_changed"),
                    json!({ "goal_id": goal.id, "action": "edit" }),
                )
                .await?;
            }
            let goal = rows::goal(&mut *unit.conn, goal.id)
                .await?
                .ok_or(GoalError::NotFound)?;
            match body.action {
                Some(GoalAction::Pause) if goal.status == status::ACTIVE => {
                    pause_in(
                        &mut unit,
                        &thread,
                        &goal,
                        GoalReason::Person,
                        None,
                        Some(caller.user()),
                    )
                    .await?;
                }
                Some(GoalAction::Pause) => {}
                Some(GoalAction::Resume)
                    if goal.status == status::PAUSED || goal.status == status::STOPPED =>
                {
                    if self
                        .limit_reached(&mut unit, &thread, &goal)
                        .await?
                        .is_some()
                    {
                        return Err(GoalError::LimitReached);
                    }
                    if rows::resume(&mut *unit.conn, goal.id, now).await? {
                        let who = by(&mut *unit.conn, caller.user()).await?;
                        marker(
                            &mut unit,
                            &thread,
                            "goal_resumed",
                            json!({ "goal_id": goal.id, "by": who }),
                        )
                        .await?;
                        audit(
                            &mut unit,
                            thread_id,
                            Some(caller.user()),
                            orch_db::audit::name!("thread.goal_changed"),
                            json!({ "goal_id": goal.id, "action": "resume" }),
                        )
                        .await?;
                        // The loop's next look counts the turns that ended while the goal was held, and only then
                        // queues the next turn, with the right number and its own identity.
                    }
                }
                Some(GoalAction::Resume) | None => {}
            }
            publish(&mut unit, thread_id).await?;
            unit.wake(thread_id);
            let thread = threads::thread(&mut *unit.conn, thread_id)
                .await?
                .ok_or(GoalError::NotFound)?;
            let row = rows::goal(&mut *unit.conn, goal.id)
                .await?
                .ok_or(GoalError::NotFound)?;
            let reply = view_of(&mut *unit.conn, &thread, &row, now).await?;
            (unit.finish().await?, reply)
        };
        self.commit(tx, finished).await?;
        Ok(reply)
    }

    /// Takes a goal away: the loop ends, and what it did stays in the conversation.
    pub async fn clear_goal_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
    ) -> Result<ThreadGoal, GoalError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        let (finished, reply) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(GoalError::NotFound)?;
            self.check_sender(&mut unit, &caller, &thread, false)
                .await?;
            let goal = rows::current(&mut *unit.conn, thread_id)
                .await?
                .ok_or(GoalError::NotFound)?;
            let was_live = goal.is_live();
            rows::end(&mut *unit.conn, goal.id, status::CLEARED, None, None, now).await?;
            if was_live {
                let who = by(&mut *unit.conn, caller.user()).await?;
                marker(
                    &mut unit,
                    &thread,
                    "goal_cleared",
                    json!({ "goal_id": goal.id, "by": who }),
                )
                .await?;
            }
            audit(
                &mut unit,
                thread_id,
                Some(caller.user()),
                orch_db::audit::name!("thread.goal_cleared"),
                json!({ "goal_id": goal.id, "was": goal.status }),
            )
            .await?;
            publish(&mut unit, thread_id).await?;
            unit.wake(thread_id);
            let thread = threads::thread(&mut *unit.conn, thread_id)
                .await?
                .ok_or(GoalError::NotFound)?;
            let row = rows::goal(&mut *unit.conn, goal.id)
                .await?
                .ok_or(GoalError::NotFound)?;
            let reply = view_of(&mut *unit.conn, &thread, &row, now).await?;
            (unit.finish().await?, reply)
        };
        self.commit(tx, finished).await?;
        Ok(reply)
    }

    /// A transaction and the caller as the guarded acts take them, for a caller that has no action guard of its own.
    async fn goal_transaction(
        &self,
        caller: &Caller,
    ) -> Result<(orch_db::Tx, Principal), GoalError> {
        let mut tx = self.inner.db.begin().await?;
        let principal = Principal::load(&mut tx, caller.clone(), self.inner.clock.time())
            .await
            .map_err(EngineError::from)?;
        Ok((tx, principal))
    }

    /// Sets a goal for `caller`, as [`Engine::set_goal_guarded`] does inside the action guard's transaction.
    pub async fn set_goal(
        &self,
        caller: &Caller,
        thread_id: Id,
        body: &ThreadGoalCreateBody,
    ) -> Result<ThreadGoal, GoalError> {
        let (tx, principal) = self.goal_transaction(caller).await?;
        self.set_goal_guarded(tx, principal, thread_id, body).await
    }

    /// Edits, pauses or resumes a goal for `caller`, as [`Engine::update_goal_guarded`] does.
    pub async fn update_goal(
        &self,
        caller: &Caller,
        thread_id: Id,
        body: &ThreadGoalUpdateBody,
    ) -> Result<ThreadGoal, GoalError> {
        let (tx, principal) = self.goal_transaction(caller).await?;
        self.update_goal_guarded(tx, principal, thread_id, body)
            .await
    }

    /// Clears a goal for `caller`, as [`Engine::clear_goal_guarded`] does.
    pub async fn clear_goal(
        &self,
        caller: &Caller,
        thread_id: Id,
    ) -> Result<ThreadGoal, GoalError> {
        let (tx, principal) = self.goal_transaction(caller).await?;
        self.clear_goal_guarded(tx, principal, thread_id).await
    }

    /// The goal a thread shows, for a caller who may view it.
    pub async fn goal_of(&self, thread_id: Id) -> Result<Option<ThreadGoal>, EngineError> {
        let mut conn = self.db().acquire().await?;
        view(&mut conn, thread_id, self.inner.clock.now()).await
    }

    /// The caller who set a goal, as the checks of an input know them: the person with the credential boundary they
    /// had then. None where they cannot be reconstructed.
    pub(crate) async fn goal_caller(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        goal: &GoalRow,
    ) -> Result<Option<Caller>, EngineError> {
        let Some(person) = goal.set_by else {
            return Ok(None);
        };
        let Some(authority) = inputs::saved_authority(goal.set_authority.as_ref())? else {
            return Ok(None);
        };
        let now = Time::new(unit.now());
        inputs::authority_caller(unit.conn, person, thread.node_id, now, &authority).await
    }

    /// Takes a person's cancellation of a goal's queued continuation as a pause: the loop would only queue it again.
    pub(crate) async fn goal_input_cancelled(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &threads::InputRow,
        by_user: Id,
    ) -> Result<(), EngineError> {
        if input.source != "goal" {
            return Ok(());
        }
        let Some(id) = rows::input_goal(unit.conn, input.id).await? else {
            return Ok(());
        };
        let Some(goal) = rows::goal(unit.conn, id).await? else {
            return Ok(());
        };
        if goal.status == status::ACTIVE {
            pause_in(unit, thread, &goal, GoalReason::Person, None, Some(by_user)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_are_written_as_a_decimal_and_zero_is_no_limit() {
        assert!(is_decimal("0"));
        assert!(is_decimal("12.50"));
        assert!(!is_decimal(""));
        assert!(!is_decimal("1."));
        assert!(!is_decimal(".5"));
        assert!(!is_decimal("1e3"));
        assert!(!is_decimal("-1"));
        assert!(is_zero("0"));
        assert!(is_zero("0.00"));
        assert!(!is_zero("0.01"));
    }

    #[test]
    fn a_goal_is_set_with_words_that_say_something() {
        assert!(trimmed("  ", 10, "empty", "long").is_err());
        assert!(trimmed("12345678901", 10, "empty", "long").is_err());
        assert_eq!(
            trimmed("  tests pass ", 20, "empty", "long").unwrap(),
            "tests pass"
        );
    }
}
