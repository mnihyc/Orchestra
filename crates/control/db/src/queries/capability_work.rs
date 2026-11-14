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
    pub deadline: DateTime<Utc>,
}

/// The actual producer supplies its accepted source, frozen choice and original deadline.
pub struct NewProgramWork<'a> {
    pub id: Id,
    pub source_answer: Id,
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
        "id,request_id AS request,source_answer_id AS source_answer,attempt_id AS attempt,
         helper_id AS helper,actor_id AS actor,person_id AS person,request_node_id AS node,purpose,backend,record,
         initial_input_id AS input,deadline_at AS deadline"
    };
}

/// Replaying a request reads its original journal and never replaces its backend or source.
pub async fn retain_program(
    conn: &mut Conn,
    work: &NewProgramWork<'_>,
) -> Result<Option<ProgramWork>, DbError> {
    sqlx::query("INSERT INTO delegated_program_work(id,request_id,source_answer_id,attempt_id,helper_id,actor_id,request_node_id,purpose,backend,record,created_at,deadline_at,person_id)
        SELECT $1,s.answer_id,s.answer_id,$3,$4,$5,s.thread_id,$7,$8,$9,$10,$11,$12
        FROM thread_tool_calls s WHERE s.answer_id=$2 AND s.thread_id=$6
          AND NOT(s.completed OR s.retired) AND $11::timestamptz>$10::timestamptz ON CONFLICT DO NOTHING")
        .bind(work.id).bind(work.source_answer).bind(work.attempt).bind(work.helper).bind(work.actor)
        .bind(work.node).bind(work.purpose).bind(work.backend).bind(work.record).bind(work.at).bind(work.deadline).bind(work.person)
        .execute(&mut *conn).await?;
    Ok(program(conn, work.id).await?.filter(|saved| {
        saved.request == work.source_answer
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
}

const SOURCE: &str = "SELECT node_id AS node,purpose,person_id AS person FROM usage_work_scope($1)";

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
