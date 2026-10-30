//! Frozen future inputs and distinct civil occurrences, claimed and stamped in their owning transaction.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// The authored definition and its current future-work state.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduleRow {
    /// Schedule identity.
    pub id: Id,
    /// Owning topic.
    pub topic_id: Id,
    /// Person whose current authority is used.
    pub author_id: Id,
    /// Safe name.
    pub name: String,
    /// Existing thread or new-thread choices.
    pub target: Json,
    /// Private frozen authored parts.
    pub content: Json,
    /// Optional private native selection.
    pub native_action: Option<Json>,
    /// Explicit civil timing.
    pub timing: Json,
    /// Active, paused, finished or deleted.
    pub state: String,
    /// Next due instant, where active.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Editor compare-and-set revision.
    pub revision: i64,
    /// Current topic feed stamp.
    pub topic_seq: Seq,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last edit/occurrence.
    pub updated_at: DateTime<Utc>,
    /// Removal time, without removing queued work.
    pub deleted_at: Option<DateTime<Utc>>,
}

/// One idempotent occurrence and its ordinary input's outcome.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunRow {
    /// Run identity.
    pub id: Id,
    /// Schedule identity.
    pub schedule_id: Id,
    /// Distinct instant or civil date/time.
    pub occurrence: String,
    /// None for a nonexistent local time.
    pub scheduled_for: Option<DateTime<Utc>>,
    /// Queued, blocked, started, failed or skipped.
    pub state: String,
    /// Destination, where created or still present.
    pub thread_id: Option<Id>,
    /// Accepted ordinary input, where any.
    pub input_id: Option<Id>,
    /// Its actual ordinary turn.
    pub turn_id: Option<Id>,
    /// Safe reason, with no draft content.
    pub reason: Option<String>,
    /// Current topic stamp.
    pub topic_seq: Seq,
    /// Occurrence creation.
    pub created_at: DateTime<Utc>,
    /// Actual ordinary turn start.
    pub started_at: Option<DateTime<Utc>>,
}

/// Reads a schedule, taking its row lock for a mutation or due claim when requested.
pub async fn schedule(conn: &mut Conn, id: Id, lock: bool) -> Result<Option<ScheduleRow>, DbError> {
    let query = if lock {
        "SELECT * FROM input_schedules WHERE id=$1 FOR UPDATE"
    } else {
        "SELECT * FROM input_schedules WHERE id=$1"
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(query))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Topic metadata in next-due order; unreadable rows are filtered before API paging.
pub async fn topic(
    conn: &mut Conn,
    id: Id,
    since: Option<Seq>,
) -> Result<Vec<ScheduleRow>, DbError> {
    Ok(sqlx::query_as("SELECT * FROM input_schedules WHERE topic_id=$1 AND ($2::bigint IS NULL OR topic_seq>$2) ORDER BY next_run_at NULLS LAST,id")
        .bind(id).bind(since).fetch_all(conn).await?)
}

/// Inserts a frozen authored definition; caller already checked and pinned its content.
pub async fn insert(conn: &mut Conn, row: &ScheduleRow) -> Result<(), DbError> {
    sqlx::query("INSERT INTO input_schedules(id,topic_id,author_id,name,target,content,native_action,timing,state,next_run_at,revision,topic_seq,created_at,updated_at,deleted_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(row.id).bind(row.topic_id).bind(row.author_id).bind(&row.name).bind(&row.target).bind(&row.content)
        .bind(&row.native_action).bind(&row.timing).bind(&row.state).bind(row.next_run_at).bind(row.revision)
        .bind(row.topic_seq).bind(row.created_at).bind(row.updated_at).bind(row.deleted_at).execute(conn).await?;
    Ok(())
}

/// Updates the definition and future state while the caller holds its row lock.
pub async fn update(conn: &mut Conn, row: &ScheduleRow) -> Result<(), DbError> {
    sqlx::query("UPDATE input_schedules SET name=$2,target=$3,content=$4,native_action=$5,timing=$6,state=$7,next_run_at=$8,revision=$9,topic_seq=$10,updated_at=$11,deleted_at=$12 WHERE id=$1")
        .bind(row.id).bind(&row.name).bind(&row.target).bind(&row.content).bind(&row.native_action).bind(&row.timing)
        .bind(&row.state).bind(row.next_run_at).bind(row.revision).bind(row.topic_seq).bind(row.updated_at).bind(row.deleted_at)
        .execute(conn).await?;
    Ok(())
}

/// An indexed page of due identities; the actual per-row claim rechecks time and state.
pub async fn due(conn: &mut Conn, now: DateTime<Utc>, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT id FROM input_schedules WHERE state='active' AND next_run_at<=$1 ORDER BY next_run_at,id LIMIT $2")
        .bind(now).bind(limit).fetch_all(conn).await?)
}

/// Occurrences newest first, including accepted work that remains after future work is stopped.
pub async fn runs(conn: &mut Conn, id: Id) -> Result<Vec<RunRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT * FROM input_schedule_runs WHERE schedule_id=$1 ORDER BY created_at DESC,id DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// The unique occurrence protects both thread creation and input acceptance from replay.
pub async fn insert_run(conn: &mut Conn, row: &RunRow) -> Result<bool, DbError> {
    Ok(sqlx::query("INSERT INTO input_schedule_runs(id,schedule_id,occurrence,scheduled_for,state,thread_id,input_id,turn_id,reason,topic_seq,created_at,started_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(schedule_id,occurrence) DO NOTHING")
        .bind(row.id).bind(row.schedule_id).bind(&row.occurrence).bind(row.scheduled_for).bind(&row.state)
        .bind(row.thread_id).bind(row.input_id).bind(row.turn_id).bind(&row.reason).bind(row.topic_seq)
        .bind(row.created_at).bind(row.started_at).execute(conn).await?.rows_affected()==1)
}

/// Publishes the accepted work or safe failure in the same occurrence transaction.
pub async fn update_run(conn: &mut Conn, row: &RunRow) -> Result<(), DbError> {
    sqlx::query("UPDATE input_schedule_runs SET state=$2,thread_id=$3,input_id=$4,turn_id=$5,reason=$6,topic_seq=$7,started_at=$8 WHERE id=$1")
        .bind(row.id).bind(&row.state).bind(row.thread_id).bind(row.input_id).bind(row.turn_id).bind(&row.reason)
        .bind(row.topic_seq).bind(row.started_at).execute(conn).await?;
    Ok(())
}

/// A still-waiting ordinary input prevents another pending occurrence of this schedule.
pub async fn pending(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM input_schedule_runs r JOIN thread_inputs i ON i.id=r.input_id WHERE r.schedule_id=$1 AND i.state IN ('pending','delivering'))")
        .bind(id).fetch_one(conn).await?)
}
