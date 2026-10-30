//! A thread's goal and the reports its agent made on it: the rows the goal loop reads and the person's acts write.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// The columns of a goal, with the amounts of credits as decimal text so that none is rounded on the way.
const GOAL_SELECT: &str = "SELECT id, thread_id, objective, condition, set_by, set_authority, set_at, status, reason, detail,
    plan_first, notify_on_end, max_turns, max_minutes, max_credits::text AS max_credits, turns_used, tokens_used,
    credits_used::text AS credits_used, elapsed_ms, active_since, after_turn, counted_turn, checking_until, ended_at,
    created_at, updated_at FROM thread_goals";

/// What a goal's status can be.
pub mod status {
    /// The loop keeps the thread working.
    pub const ACTIVE: &str = "active";
    /// A person, or the agent's report that it is blocked, holds the loop.
    pub const PAUSED: &str = "paused";
    /// The condition holds.
    pub const MET: &str = "met";
    /// The loop ended without the condition: a limit was reached, or nobody may go on.
    pub const STOPPED: &str = "stopped";
    /// A person took it away.
    pub const CLEARED: &str = "cleared";
}

/// One goal of a thread.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct GoalRow {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The work to do.
    pub objective: String,
    /// What is true when it is over ("Done when").
    pub condition: String,
    /// Whose inputs the continuations are; none once that person is gone.
    pub set_by: Option<Id>,
    /// That person's credential boundary as an input records it.
    pub set_authority: Option<Json>,
    /// When it was set.
    pub set_at: DateTime<Utc>,
    /// Active, paused, met, stopped or cleared.
    pub status: String,
    /// Why it is paused or stopped.
    pub reason: Option<String>,
    /// The words that go with the reason.
    pub detail: Option<String>,
    /// Whether its first turn is a planning turn.
    pub plan_first: bool,
    /// Whether the person asked to be told when it ends.
    pub notify_on_end: bool,
    /// Its own limit of turns; none follows the setting, zero is no limit.
    pub max_turns: Option<i64>,
    /// Its own limit of minutes.
    pub max_minutes: Option<i64>,
    /// Its own limit of credits, as decimal text.
    pub max_credits: Option<String>,
    /// How many turns it has used.
    pub turns_used: i64,
    /// How many tokens.
    pub tokens_used: i64,
    /// How many credits, as decimal text.
    pub credits_used: String,
    /// Milliseconds spent active, up to the last time it stopped running.
    pub elapsed_ms: i64,
    /// While it is active: where the elapsed time runs from.
    pub active_since: Option<DateTime<Utc>>,
    /// The thread's last turn when it was set.
    pub after_turn: i32,
    /// The last turn counted into the figures.
    pub counted_turn: i32,
    /// While the helper model checks a report: when the claim ends.
    pub checking_until: Option<DateTime<Utc>>,
    /// When it ended.
    pub ended_at: Option<DateTime<Utc>>,
    /// When the row was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

impl GoalRow {
    /// Whether it is under way: the loop may run it, or a person holds it.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.status == status::ACTIVE || self.status == status::PAUSED
    }

    /// The milliseconds it has been active in all at `now`.
    #[must_use]
    pub fn elapsed_at(&self, now: DateTime<Utc>) -> i64 {
        let running = self
            .active_since
            .map_or(0, |since| (now - since).num_milliseconds().max(0));
        self.elapsed_ms.saturating_add(running)
    }
}

/// A goal to make.
#[derive(Debug, Clone, PartialEq)]
pub struct NewGoal<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The work.
    pub objective: &'a str,
    /// Done when.
    pub condition: &'a str,
    /// Who sets it.
    pub set_by: Id,
    /// Their credential boundary.
    pub set_authority: &'a Json,
    /// Whether its first turn plans.
    pub plan_first: bool,
    /// Whether to tell the person when it ends.
    pub notify_on_end: bool,
    /// Its own limit of turns.
    pub max_turns: Option<i64>,
    /// Its own limit of minutes.
    pub max_minutes: Option<i64>,
    /// Its own limit of credits, as decimal text.
    pub max_credits: Option<&'a str>,
    /// The thread's last turn now.
    pub after_turn: i32,
    /// When.
    pub at: DateTime<Utc>,
}

/// The goal `id`.
pub async fn goal(conn: &mut Conn, id: Id) -> Result<Option<GoalRow>, DbError> {
    let sql = format!("{GOAL_SELECT} WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The goal of a thread that is under way (active or paused), if it has one.
pub async fn live(conn: &mut Conn, thread: Id) -> Result<Option<GoalRow>, DbError> {
    let sql = format!("{GOAL_SELECT} WHERE thread_id = $1 AND status IN ('active','paused')");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_optional(conn)
        .await?)
}

/// The goal a thread shows: the one under way, else the latest ended goal unless it was cleared.
pub async fn current(conn: &mut Conn, thread: Id) -> Result<Option<GoalRow>, DbError> {
    if let Some(goal) = live(conn, thread).await? {
        return Ok(Some(goal));
    }
    let sql = format!(
        "{GOAL_SELECT} WHERE thread_id = $1 AND status IN ('met','stopped','cleared')
          ORDER BY set_at DESC, id DESC LIMIT 1"
    );
    let goal: Option<GoalRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_optional(conn)
        .await?;
    Ok(goal.filter(|goal| goal.status != status::CLEARED))
}

/// The goals of the threads `threads`, as each thread shows them, in one read. Each requested thread probes its live
/// index and, only where no live goal exists, the newest ended row; no thread's accumulated history is sorted.
pub async fn current_of(conn: &mut Conn, threads: &[Id]) -> Result<Vec<GoalRow>, DbError> {
    let sql = current_of_sql();
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(threads)
        .fetch_all(conn)
        .await?)
}

// Deduplicate requested threads before probing, and materialize selected IDs before grouping their goal rows.
fn current_of_sql() -> String {
    format!(
        "WITH wanted AS MATERIALIZED (
           SELECT DISTINCT thread_id FROM unnest($1::uuid[]) requested(thread_id)),
         picked AS MATERIALIZED (
           SELECT COALESCE(
             (SELECT g.id FROM thread_goals g WHERE g.thread_id=wanted.thread_id AND g.status IN ('active','paused')),
             (SELECT g.id FROM thread_goals g WHERE g.thread_id=wanted.thread_id AND g.status IN ('met','stopped','cleared')
               ORDER BY g.set_at DESC, g.id DESC LIMIT 1)) AS id
           FROM wanted)
         {GOAL_SELECT} WHERE status <> 'cleared' AND id IN (SELECT id FROM picked)"
    )
}

/// Makes a goal, active from `at`. The thread's row must be locked, and it has no other goal under way.
pub async fn insert(conn: &mut Conn, new: &NewGoal<'_>) -> Result<GoalRow, DbError> {
    sqlx::query(
        "INSERT INTO thread_goals (id, thread_id, objective, condition, set_by, set_authority, set_at, status,
            plan_first, notify_on_end, max_turns, max_minutes, max_credits, active_since, after_turn, counted_turn,
            created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'active', $8, $9, $10, $11, $12::text::numeric, $7, $13, $13, $7, $7)",
    )
    .bind(new.id)
    .bind(new.thread_id)
    .bind(new.objective)
    .bind(new.condition)
    .bind(new.set_by)
    .bind(new.set_authority)
    .bind(new.at)
    .bind(new.plan_first)
    .bind(new.notify_on_end)
    .bind(new.max_turns)
    .bind(new.max_minutes)
    .bind(new.max_credits)
    .bind(new.after_turn)
    .execute(&mut *conn)
    .await?;
    goal(conn, new.id).await?.ok_or_else(|| DbError::Rule {
        constraint: "thread_goal_made_is_not_there".into(),
    })
}

/// What a person may change of a goal.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalEdit<'a> {
    /// The new work, where it changes.
    pub objective: Option<&'a str>,
    /// The new condition, where it changes.
    pub condition: Option<&'a str>,
    /// New limits, where they change: the inner value is the goal's own, none following the setting.
    pub max_turns: Option<Option<i64>>,
    /// See `max_turns`.
    pub max_minutes: Option<Option<i64>>,
    /// See `max_turns`; decimal text.
    pub max_credits: Option<Option<&'a str>>,
    /// Whether to tell the person when it ends, where that changes.
    pub notify_on_end: Option<bool>,
}

/// Changes what a person edited; the thread's row must be locked. A change of the condition or the work starts
/// the helper's claim over, so that an answer about the old one is not applied to the new.
pub async fn edit(
    conn: &mut Conn,
    id: Id,
    edit: &GoalEdit<'_>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE thread_goals SET
            objective = COALESCE($2, objective),
            condition = COALESCE($3, condition),
            max_turns = CASE WHEN $4 THEN $5 ELSE max_turns END,
            max_minutes = CASE WHEN $6 THEN $7 ELSE max_minutes END,
            max_credits = CASE WHEN $8 THEN $9::text::numeric ELSE max_credits END,
            notify_on_end = COALESCE($10, notify_on_end),
            checking_until = CASE WHEN $2 IS NOT NULL OR $3 IS NOT NULL THEN NULL ELSE checking_until END,
            updated_at = $11
          WHERE id = $1",
    )
    .bind(id)
    .bind(edit.objective)
    .bind(edit.condition)
    .bind(edit.max_turns.is_some())
    .bind(edit.max_turns.flatten())
    .bind(edit.max_minutes.is_some())
    .bind(edit.max_minutes.flatten())
    .bind(edit.max_credits.is_some())
    .bind(edit.max_credits.flatten())
    .bind(edit.notify_on_end)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Holds an active goal, ending the time it has been running; a goal that is not active is left as it is. Says
/// whether it changed.
pub async fn pause(
    conn: &mut Conn,
    id: Id,
    reason: &str,
    detail: Option<&str>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE thread_goals SET status = 'paused', reason = $2, detail = $3,
            elapsed_ms = elapsed_ms + CASE WHEN active_since IS NULL THEN 0
                                           ELSE GREATEST(0, (EXTRACT(EPOCH FROM ($4 - active_since)) * 1000)::bigint) END,
            active_since = NULL, checking_until = NULL, updated_at = $4
          WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .bind(reason)
    .bind(detail)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Sets a paused, or a stopped, goal going again from `at`. Says whether it changed.
pub async fn resume(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE thread_goals SET status = 'active', reason = NULL, detail = NULL, active_since = $2,
            ended_at = NULL, checking_until = NULL, updated_at = $2
          WHERE id = $1 AND status IN ('paused','stopped')",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Ends a goal as `met`, `stopped` or `cleared`, ending the time it has been running. Says whether it changed.
pub async fn end(
    conn: &mut Conn,
    id: Id,
    to: &str,
    reason: Option<&str>,
    detail: Option<&str>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE thread_goals SET status = $2, reason = COALESCE($3, reason), detail = COALESCE($4, detail),
            elapsed_ms = elapsed_ms + CASE WHEN active_since IS NULL THEN 0
                                           ELSE GREATEST(0, (EXTRACT(EPOCH FROM ($5 - active_since)) * 1000)::bigint) END,
            active_since = NULL, checking_until = NULL, ended_at = $5, updated_at = $5
          WHERE id = $1 AND status IN ('active','paused','stopped','met')",
    )
    .bind(id)
    .bind(to)
    .bind(reason)
    .bind(detail)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Takes away the latest ended goal, so that it does not linger above new work. A cleared latest row keeps older
/// goals hidden, so no historical row needs updating; returns the one goal cleared, if there was one.
pub async fn clear_ended(
    conn: &mut Conn,
    thread: Id,
    at: DateTime<Utc>,
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE thread_goals SET status = 'cleared', updated_at = $2
          WHERE id = (SELECT id FROM thread_goals WHERE thread_id = $1 AND status IN ('met','stopped','cleared')
                       ORDER BY set_at DESC, id DESC LIMIT 1)
            AND status IN ('met','stopped') RETURNING id",
    )
    .bind(thread)
    .bind(at)
    .fetch_optional(conn)
    .await?)
}

/// What one turn that ended costs a goal.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TurnFigures {
    /// The turn.
    pub id: Id,
    /// Its number in the thread.
    pub ordinal: i32,
    /// How it ended: `completed`, `interrupted` or `failed`.
    pub state: String,
    /// Why it was interrupted, if it was.
    pub interrupt_cause: Option<String>,
    /// The person who stopped it, for a person's Stop.
    pub interrupted_by: Option<Id>,
    /// Whether the harness began it: a turn that never started used nothing and is not counted.
    pub started: bool,
    /// Whether its accounting (usage, notifications) is still to be done: what it used is not whole yet.
    pub end_pending: bool,
    /// Its tokens, as its harness reported them.
    pub tokens: i64,
    /// What the ledger counted for it, as decimal text.
    pub credits: String,
}

/// The turns of `thread` after `after` that ended, oldest first.
const ENDED_TURNS_SQL: &str = "WITH chosen AS MATERIALIZED (
    SELECT id, ordinal, state, interrupt_cause, interrupted_by, started_at, end_work, usage
    FROM turns WHERE thread_id = $1 AND ordinal > $2 AND state IN ('completed','interrupted','failed')
    ORDER BY ordinal LIMIT $3
), credits AS (
    SELECT e.turn_id, sum(e.credits) AS credits FROM usage_events e
    WHERE e.counted AND e.turn_id = ANY(ARRAY(SELECT id FROM chosen)) GROUP BY e.turn_id
)
SELECT t.id, t.ordinal, t.state, t.interrupt_cause, t.interrupted_by, (t.started_at IS NOT NULL) AS started,
    (t.end_work IS NOT NULL) AS end_pending, COALESCE((t.usage->>'total_tokens')::bigint, 0) AS tokens,
    COALESCE(credits.credits, 0)::text AS credits FROM chosen t LEFT JOIN credits ON credits.turn_id = t.id
ORDER BY t.ordinal";

/// Reads one bounded page of ended turns and their credit totals in one grouped ledger statement.
pub async fn ended_turns_after(
    conn: &mut Conn,
    thread: Id,
    after: i32,
    limit: i64,
) -> Result<Vec<TurnFigures>, DbError> {
    Ok(sqlx::query_as(ENDED_TURNS_SQL)
        .bind(thread)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Accounts the completed prefix once; an end whose follow-up is pending still stops the prefix.
pub async fn add_usage_batch(
    conn: &mut Conn,
    id: Id,
    turns: &[TurnFigures],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let figures: Vec<Json> = turns
        .iter()
        .take_while(|turn| !turn.end_pending)
        .map(|turn| {
            serde_json::json!({
                "ordinal": turn.ordinal, "started": turn.started,
                "tokens": turn.tokens, "credits": turn.credits,
            })
        })
        .collect();
    if figures.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "WITH figures AS (
            SELECT count(*) FILTER (WHERE started)::bigint AS turns,
                COALESCE(sum(tokens) FILTER (WHERE started), 0)::bigint AS tokens,
                COALESCE(sum(credits::numeric) FILTER (WHERE started), 0) AS credits,
                max(ordinal) AS counted_turn
            FROM jsonb_to_recordset($2::jsonb) f(ordinal integer, started boolean, tokens bigint, credits text)
        ) UPDATE thread_goals g SET turns_used = g.turns_used + f.turns,
            tokens_used = g.tokens_used + f.tokens, credits_used = g.credits_used + f.credits,
            counted_turn = GREATEST(g.counted_turn, f.counted_turn), updated_at = $3
        FROM figures f WHERE g.id = $1",
    )
    .bind(id)
    .bind(Json::Array(figures))
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Adds what the counted turns used and moves the mark of the last turn counted.
pub async fn add_usage(
    conn: &mut Conn,
    id: Id,
    turns: i64,
    tokens: i64,
    credits: &str,
    counted_turn: i32,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE thread_goals SET turns_used = turns_used + $2, tokens_used = tokens_used + $3,
            credits_used = credits_used + $4::text::numeric, counted_turn = GREATEST(counted_turn, $5), updated_at = $6
          WHERE id = $1",
    )
    .bind(id)
    .bind(turns)
    .bind(tokens)
    .bind(credits)
    .bind(counted_turn)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether what a goal has used of credits reaches `limit`, which is written as decimal text.
pub async fn credits_reached(conn: &mut Conn, id: Id, limit: &str) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT credits_used >= $2::text::numeric FROM thread_goals WHERE id = $1",
    )
    .bind(id)
    .bind(limit)
    .fetch_optional(conn)
    .await?
    .unwrap_or(false))
}

/// Takes the helper's claim on a goal until `until`, where nobody holds one. Says whether it was taken.
pub async fn claim_check(
    conn: &mut Conn,
    id: Id,
    until: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE thread_goals SET checking_until = $2, updated_at = $3
          WHERE id = $1 AND status = 'active' AND (checking_until IS NULL OR checking_until <= $3)",
    )
    .bind(id)
    .bind(until)
    .bind(now)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Lets go of the helper's claim.
pub async fn release_check(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_goals SET checking_until = NULL, updated_at = $2 WHERE id = $1 AND checking_until IS NOT NULL")
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// The threads with an active goal that may need the loop's attention: idle or blocked, no turn under way, and no
/// helper check in hand. The cursor advances over a page of active goals before checking their threads, so waiting
/// goals neither starve later ones nor make a sweep read every active goal. A missing next cursor starts a new pass.
pub async fn threads_with_goal_work(
    conn: &mut Conn,
    now: DateTime<Utc>,
    after: Option<Id>,
    limit: i64,
) -> Result<(Vec<Id>, Option<Id>), DbError> {
    let comparison = if after.is_some() { ">" } else { ">=" };
    let sql = format!(
        "WITH page AS MATERIALIZED (
           SELECT thread_id, checking_until FROM thread_goals
            WHERE status = 'active' AND thread_id {comparison} $2
            ORDER BY thread_id LIMIT $3
         )
         SELECT p.thread_id, t.current_turn_id IS NULL AND t.state IN ('idle','blocked')
           AND (p.checking_until IS NULL OR p.checking_until <= $1) AS ready
         FROM page p JOIN threads t ON t.node_id = p.thread_id ORDER BY p.thread_id"
    );
    let page = sqlx::query_as::<_, (Id, bool)>(sqlx::AssertSqlSafe(sql))
        .bind(now)
        .bind(after.unwrap_or_else(Id::nil))
        .bind(limit)
        .fetch_all(conn)
        .await?;
    let next = if i64::try_from(page.len()).unwrap_or(i64::MAX) == limit {
        page.last().map(|(id, _)| *id)
    } else {
        None
    };
    Ok((
        page.into_iter()
            .filter_map(|(id, ready)| ready.then_some(id))
            .collect(),
        next,
    ))
}

/// Whether anything of the thread asks a person for an answer: an approval, a question, a plan, a capability.
pub async fn asks_a_person(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM approvals WHERE thread_id = $1 AND state = 'pending')",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// Ties an input of the goal's loop to its goal, with what the goal said of itself in it.
pub async fn tie_input(conn: &mut Conn, input: Id, goal: Id, facts: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_inputs SET goal_id = $2, goal_facts = $3 WHERE id = $1")
        .bind(input)
        .bind(goal)
        .bind(facts)
        .execute(conn)
        .await?;
    Ok(())
}

/// What a goal's input said of its goal, for the block the timeline shows in its place.
pub async fn input_facts(conn: &mut Conn, input: Id) -> Result<Option<Json>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT goal_facts FROM thread_inputs WHERE id = $1")
            .bind(input)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// The goal an input of its loop belongs to.
pub async fn input_goal(conn: &mut Conn, input: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT goal_id FROM thread_inputs WHERE id = $1")
            .bind(input)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

const RETRY_ATTEMPTS_SQL: &str = "SELECT retry_attempts FROM thread_goals WHERE id = $1";

/// The retained retry generation; cleaning up failed inputs never reuses an earlier continuation identity.
pub async fn lost_inputs(conn: &mut Conn, goal: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(RETRY_ATTEMPTS_SQL)
        .bind(goal)
        .fetch_optional(conn)
        .await?
        .unwrap_or(0))
}

/// One report of the agent.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ReportRow {
    /// Its identifier.
    pub id: Id,
    /// The goal.
    pub goal_id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The turn it was made in.
    pub turn_id: Option<Id>,
    /// The number of that turn, where it still exists.
    pub turn_ordinal: Option<i32>,
    /// `working`, `met` or `blocked`: what the agent said.
    pub status: String,
    /// What shows it.
    pub evidence: String,
    /// What the agent does next, or needs.
    pub next: Option<String>,
    /// What Orch made of it; none while it waits.
    pub outcome: Option<String>,
    /// Who found it met: `helper` or `agent`.
    pub checked_by: Option<String>,
    /// The helper's reason, or why it could not check.
    pub reason: Option<String>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When Orch decided what it came to.
    pub decided_at: Option<DateTime<Utc>>,
}

const REPORT_SELECT: &str = "SELECT r.id, r.goal_id, r.thread_id, r.turn_id, t.ordinal AS turn_ordinal, r.status, r.evidence,
    r.next, r.outcome, r.checked_by, r.reason, r.created_at, r.decided_at
    FROM thread_goal_reports r LEFT JOIN turns t ON t.id = r.turn_id";

/// A report to keep.
#[derive(Debug, Clone, PartialEq)]
pub struct NewReport<'a> {
    /// Its identifier.
    pub id: Id,
    /// The goal.
    pub goal_id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The turn.
    pub turn_id: Option<Id>,
    /// `working`, `met` or `blocked`.
    pub status: &'a str,
    /// What shows it.
    pub evidence: &'a str,
    /// What comes next.
    pub next: Option<&'a str>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Keeps a report. A report of work going on is settled at once as `noted`.
pub async fn insert_report(conn: &mut Conn, new: &NewReport<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO thread_goal_reports (id, goal_id, thread_id, turn_id, status, evidence, next, outcome, decided_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, CASE WHEN $5 = 'working' THEN 'noted' END,
                 CASE WHEN $5 = 'working' THEN $8 END, $8)",
    )
    .bind(new.id)
    .bind(new.goal_id)
    .bind(new.thread_id)
    .bind(new.turn_id)
    .bind(new.status)
    .bind(new.evidence)
    .bind(new.next)
    .bind(new.at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The newest `limit` reports of a goal, newest first.
pub async fn reports(conn: &mut Conn, goal: Id, limit: i64) -> Result<Vec<ReportRow>, DbError> {
    let sql = format!(
        "{REPORT_SELECT} WHERE r.goal_id = $1 ORDER BY r.created_at DESC, r.id DESC LIMIT $2"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(goal)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The newest report of a goal.
pub async fn latest_report(conn: &mut Conn, goal: Id) -> Result<Option<ReportRow>, DbError> {
    Ok(reports(conn, goal, 1).await?.into_iter().next())
}

/// The report `id`.
pub async fn report(conn: &mut Conn, id: Id) -> Result<Option<ReportRow>, DbError> {
    let sql = format!("{REPORT_SELECT} WHERE r.id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Settles what a report came to. Says whether it was still waiting.
pub async fn decide_report(
    conn: &mut Conn,
    id: Id,
    outcome: &str,
    checked_by: Option<&str>,
    reason: Option<&str>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE thread_goal_reports SET outcome = $2, checked_by = $3, reason = $4, decided_at = $5
          WHERE id = $1 AND outcome IS NULL",
    )
    .bind(id)
    .bind(outcome)
    .bind(checked_by)
    .bind(reason)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Notes that the helper could not be asked about a report, and says how many times that has happened.
pub async fn note_check_attempt(conn: &mut Conn, id: Id) -> Result<i32, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE thread_goal_reports SET attempts = attempts + 1 WHERE id = $1 RETURNING attempts",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// Holds the helper's claim on a goal until `until`, so that the check is tried again then.
pub async fn delay_check(
    conn: &mut Conn,
    id: Id,
    until: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_goals SET checking_until = $2, updated_at = $3 WHERE id = $1 AND status = 'active'")
        .bind(id)
        .bind(until)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Marks the reports of a goal that wait and are older than `id` as superseded: the agent said more since.
pub async fn note_older_reports(
    conn: &mut Conn,
    goal: Id,
    newest: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE thread_goal_reports SET outcome = 'noted', decided_at = $3
          WHERE goal_id = $1 AND id <> $2 AND outcome IS NULL",
    )
    .bind(goal)
    .bind(newest)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
