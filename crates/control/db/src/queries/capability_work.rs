//! Immutable work attribution from a native helper's actual initial input and source-owned journal.
//! These associations are billing facts, never permission or an executable source proof.
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// One immutable program attempt. Its producer retains the actual authority and prompt.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProgramWork {
    pub id: Id,
    /// Original accepted act, retained as a fact after its source is removed.
    pub request: Id,
    /// The actual live tool journal, absent after that journal is purged.
    pub source_answer: Option<Id>,
    pub source_summary_call: Option<Id>,
    pub source_ask_call: Option<Id>,
    pub attempt: Id,
    pub helper: Id,
    pub actor: Id,
    /// The consumer's original issuer; absence stays unattributed rather than naming the actor.
    pub person: Option<Id>,
    pub node: Id,
    pub purpose: String,
    pub backend: Json,
    /// Producer-owned journal locator and execution-source facts; never acting authority.
    pub record: Json,
    /// The actual initial input claims this attempt once and keeps its native identity.
    pub input: Option<Id>,
    /// The native subscription binding at the first real session proposal, without its credential.
    pub session_login_hash: Option<Vec<u8>>,
    pub deadline: DateTime<Utc>,
}

/// A factual live journal association; restoring its actual producer remains mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramOrigin {
    Tool { answer: Id },
    Summary { call: Id },
    Ask { call: Id },
}

/// The actual producer supplies its accepted source, frozen choice and original deadline.
pub struct NewProgramWork<'a> {
    pub id: Id,
    pub request: Id,
    pub source: ProgramOrigin,
    pub attempt: Id,
    pub helper: Id,
    pub actor: Id,
    pub person: Option<Id>,
    pub node: Id,
    pub purpose: &'a str,
    pub backend: &'a Json,
    pub record: &'a Json,
    pub at: DateTime<Utc>,
    pub deadline: DateTime<Utc>,
}

macro_rules! program_columns {
    () => {
        "id,request_id AS request,source_answer_id AS source_answer,source_summary_call_id AS source_summary_call,source_ask_call_id AS source_ask_call,attempt_id AS attempt,
         helper_id AS helper,actor_id AS actor,person_id AS person,request_node_id AS node,purpose,backend,record,
         initial_input_id AS input,session_login_hash,deadline_at AS deadline"
    };
}

/// Replaying a request reads its original journal and never replaces its backend or source.
pub async fn retain_program(
    conn: &mut Conn,
    work: &NewProgramWork<'_>,
) -> Result<Option<ProgramWork>, DbError> {
    let (answer, summary, ask) = match work.source {
        ProgramOrigin::Tool { answer } => (Some(answer), None, None),
        ProgramOrigin::Summary { call } => (None, Some(call), None),
        ProgramOrigin::Ask { call } => (None, None, Some(call)),
    };
    sqlx::query("WITH live AS (
        SELECT s.answer_id AS request,s.answer_id AS answer,NULL::uuid AS summary_call,NULL::uuid AS ask_call,s.thread_id AS node
        FROM thread_tool_calls s WHERE s.answer_id=$13 AND s.answer_id=$2 AND s.thread_id=$6 AND NOT(s.completed OR s.retired)
        UNION ALL
        SELECT $2::uuid,NULL::uuid,s.id,NULL::uuid,s.node_id FROM summary_calls s
        WHERE s.id=$14 AND s.node_id=$6 AND s.purpose='summary' AND $7='summary'
          AND s.person_id=$5 AND s.issued_by IS NOT DISTINCT FROM $12::uuid AND s.cancel_reason IS NULL
          AND s.delegated_program IS NOT NULL AND s.program_backend=$8
          AND s.provider_id=($8#>>'{offering,provider}')::uuid AND s.model_name=$8#>>'{offering,offering}'
        UNION ALL
        SELECT a.id,NULL::uuid,NULL::uuid,a.id,a.root_org_id FROM ask_calls a
        JOIN threads t ON t.node_id=a.thread_id AND t.current_turn_id=a.turn_id AND t.ask_state='running'
        WHERE a.id=$15 AND a.id=$2 AND a.id=$3 AND a.root_org_id=$6 AND $7='ask' AND a.person_id=$5 AND a.person_id=$12
          AND NOT a.settled AND a.machine_id IS NULL AND a.delegated_program IS NOT NULL AND a.request->'backend'=$8
          AND a.provider_id=($8#>>'{offering,provider}')::uuid
      ) INSERT INTO delegated_program_work(id,request_id,source_answer_id,source_summary_call_id,source_ask_call_id,attempt_id,helper_id,actor_id,request_node_id,purpose,backend,record,created_at,deadline_at,person_id)
        SELECT $1,live.request,live.answer,live.summary_call,live.ask_call,$3,$4,$5,live.node,$7,$8,$9,$10,$11,$12
        FROM live WHERE $11::timestamptz>$10::timestamptz ON CONFLICT DO NOTHING")
        .bind(work.id).bind(work.request).bind(work.attempt).bind(work.helper).bind(work.actor)
        .bind(work.node).bind(work.purpose).bind(work.backend).bind(work.record).bind(work.at).bind(work.deadline).bind(work.person)
        .bind(answer).bind(summary).bind(ask).execute(&mut *conn).await?;
    Ok(program(conn, work.id).await?.filter(|saved| {
        saved.request == work.request
            && saved.source_answer == answer
            && saved.source_summary_call == summary
            && saved.source_ask_call == ask
            && saved.attempt == work.attempt
            && saved.helper == work.helper
            && saved.actor == work.actor
            && saved.person == work.person
            && saved.node == work.node
            && saved.purpose == work.purpose
    }))
}

/// This is a factual read; restoring the producer's current authority remains its own boundary.
pub async fn program(conn: &mut Conn, id: Id) -> Result<Option<ProgramWork>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        program_columns!(),
        " FROM delegated_program_work WHERE id=$1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Existing attempts of one actual tool journal, newest first. These facts grant no execution or Read.
pub async fn programs_for_tool(conn: &mut Conn, answer: Id) -> Result<Vec<ProgramWork>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ", program_columns!(),
        " FROM delegated_program_work WHERE source_answer_id=$1 AND initial_input_id IS NOT NULL ORDER BY created_at DESC,id DESC"
    )).bind(answer).fetch_all(conn).await?)
}

/// Only the actual initial helper input can claim an attempt; a later input cannot replace it.
pub async fn bind_program_input(conn: &mut Conn, work: Id, input: Id) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE delegated_program_work w SET initial_input_id=i.id FROM thread_inputs i
        WHERE w.id=$1 AND i.id=$2 AND i.thread_id=w.helper_id AND i.author_id=w.actor_id
          AND i.client_id='capability-helper:' || w.attempt_id::text
          AND i.submission_authority#>>'{capability_parent,work}'=w.id::text
          AND i.submission_authority#>>'{capability_parent,helper}'=w.helper_id::text
          AND (w.initial_input_id IS NULL OR w.initial_input_id=i.id)",
    )
    .bind(work)
    .bind(input)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Work {
    /// The requesting place, independently of the helper's own thread/turn membership.
    pub node: Id,
    /// The requested common capability.
    pub purpose: String,
    pub person: Option<Id>,
    /// This native model step uses the concurrency reservation of its accepted Ask turn.
    pub ask_turn: Option<Id>,
}

const SOURCE: &str =
    "SELECT node_id AS node,purpose,person_id AS person,ask_turn FROM usage_work_scope($1)";

/// One retained input; later independent inputs in the same ordinary helper have no such association.
pub async fn for_input(conn: &mut Conn, input: Id) -> Result<Option<Work>, DbError> {
    Ok(sqlx::query_as(SOURCE)
        .bind(input)
        .fetch_optional(conn)
        .await?)
}

/// Preserve actual native turn identity while supplying its separate requesting work scope.
pub async fn for_turn(conn: &mut Conn, turn: Id) -> Result<Option<Work>, DbError> {
    let input: Option<Id> = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(turn)
        .fetch_optional(&mut *conn)
        .await?;
    match input {
        Some(input) => for_input(conn, input).await,
        None => Ok(None),
    }
}

/// An idle native conversation still owned by the delegated attempt, not later independent work.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IdleProgram {
    pub work: Id,
    pub helper: Id,
    pub segment: Id,
    pub machine: Id,
    pub provider: Id,
    pub completed: DateTime<Utc>,
    pub previous_close: Option<Id>,
}

const IDLE_PROGRAM: &str = "SELECT w.id AS work,w.helper_id AS helper,s.id AS segment,s.machine_id AS machine,
    s.provider_id AS provider,u.completed_at AS completed,w.idle_close_command_id AS previous_close
    FROM delegated_program_work w JOIN threads t ON t.node_id=w.helper_id
    JOIN segments s ON s.id=t.current_segment_id
    JOIN turns u ON u.thread_id=t.node_id AND u.input_id=w.initial_input_id AND u.segment_id=s.id
    LEFT JOIN commands c ON c.id=w.idle_close_command_id
    WHERE t.current_turn_id IS NULL AND s.machine_id IS NOT NULL AND s.provider_id IS NOT NULL AND s.credential_path='native'
      AND s.ended_at IS NULL AND u.completed_at IS NOT NULL AND u.state NOT IN ('starting','running')
      AND NOT EXISTS(SELECT 1 FROM turns newer WHERE newer.thread_id=t.node_id
          AND (newer.ordinal>u.ordinal OR newer.state IN ('starting','running')))
      AND NOT EXISTS(SELECT 1 FROM thread_inputs i WHERE i.thread_id=t.node_id AND i.state IN ('pending','delivering'))
      AND NOT EXISTS(SELECT 1 FROM thread_inputs extra WHERE extra.turn_id=u.id AND extra.id<>w.initial_input_id)
      AND NOT EXISTS(SELECT 1 FROM approvals a WHERE a.thread_id=t.node_id AND a.state='pending')
      AND NOT EXISTS(SELECT 1 FROM leases l WHERE l.thread_id=t.node_id AND l.closed_at IS NULL)
      AND NOT EXISTS(SELECT 1 FROM commands pending WHERE pending.body->>'thread_id'=t.node_id::text
          AND pending.state IN ('queued','sent','accepted'))
      AND w.context_closed_at IS NULL AND (c.id IS NULL OR (c.state IN ('failed','expired','cancelled') AND c.updated_at<=$1))";

/// Keyset pages visit every open owned context even when an earlier one has a longer idle setting.
pub async fn idle_programs(
    conn: &mut Conn,
    retry_before: DateTime<Utc>,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<IdleProgram>, DbError> {
    let sql = format!("{IDLE_PROGRAM} AND ($2::uuid IS NULL OR w.id>$2) ORDER BY w.id LIMIT $3");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(retry_before)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Recheck under the ordinary helper row lock before committing its reducing close.
pub async fn idle_program(
    conn: &mut Conn,
    retry_before: DateTime<Utc>,
    work: Id,
) -> Result<Option<IdleProgram>, DbError> {
    let sql = format!("{IDLE_PROGRAM} AND w.id=$2");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(retry_before)
        .bind(work)
        .fetch_optional(conn)
        .await?)
}

/// The same transaction owns the exact close command and the next recovery identity.
pub async fn retain_idle_close(conn: &mut Conn, work: Id, command: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE delegated_program_work SET idle_close_command_id=$2 WHERE id=$1")
        .bind(work)
        .bind(command)
        .execute(conn)
        .await?;
    Ok(())
}

/// Excludes only real generated initial work from automatic producers; later input stays ordinary.
pub async fn execution_only(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT delegated_execution_only($1)")
        .bind(thread)
        .fetch_one(conn)
        .await?)
}

/// Accepted source copies require the ordinary informed sharing transition even before
/// their first native answer. Protected prompt markers keep that disclosure after job/input cleanup.
pub async fn copied_source_count(conn: &mut Conn, helper: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT
        (SELECT count(*) FROM delegated_program_work w WHERE w.helper_id=$1 AND w.initial_input_id IS NOT NULL) +
        (SELECT count(*) FROM model_inputs m
         WHERE m.node_id=$1 AND m.purpose='input'
           AND (EXISTS(SELECT 1 FROM resource_summary_model_inputs g WHERE g.model_input_id=m.id)
                OR EXISTS(SELECT 1 FROM delegated_program_model_inputs g WHERE g.model_input_id=m.id))
           AND NOT EXISTS(SELECT 1 FROM delegated_program_work w WHERE w.helper_id=$1 AND w.initial_input_id=m.input_id))")
        .bind(helper).fetch_one(conn).await?)
}
