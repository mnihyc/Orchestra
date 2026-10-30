//! What taking in a session that already exists reads and writes: the connected environments of a machine, and whether the session of a segment is one that Orch has yet to open.
//!
//! A person who began a session in a harness by hand has it listed on their machine's page. To take
//! it in, Orch looks for an environment of theirs on that machine whose folder holds the directory
//! the session ran in, and attaches a thread to the session; the first time that thread's turn
//! opens the session, it is opened as one that was adopted, which is not the same as one that Orch
//! began and resumes; the segment that holds the session says so until then.

use crate::{Conn, DbError};
use orch_types::Id;

/// Takes a lock, until the end of the transaction, that is the same for every request to take in
/// the session named: two people who take in one session at once come one after the other, and the
/// second finds that a thread has it.
pub async fn lock_session(conn: &mut Conn, reference: &str) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("adopt:{reference}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// A directory of a machine that is registered as an environment.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Registered {
    /// The environment.
    pub id: Id,
    /// The absolute path of its folder, written as the machine writes paths.
    pub workspace_root: String,
}

/// The environments that are directories of the machine `machine`: not deleted, not due for
/// removal and not switched off.
pub async fn registered_on(conn: &mut Conn, machine: Id) -> Result<Vec<Registered>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, workspace_root FROM environments
          WHERE machine_id = $1 AND placement = 'connected' AND deleted_at IS NULL
            AND remove_at IS NULL AND status <> 'disabled'
          ORDER BY created_at, id",
    )
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// What a segment says of a session that was begun outside Orch, written in its report of what a
/// history kept until what was taken in of the harness's record replaces it ([`set_adopted_report`]).
const REPORT_WARNING: &str = "This session was begun outside Orch. What was said in it before is on the thread as the harness recorded it, from the session before Orch.";

/// Says of a new segment that it holds a session that was begun outside Orch and that Orch has not
/// opened yet. The member `adopted` of the report is the reading of this: `pending` until the
/// first time the session is opened, `opened` after. A reader of the segment is shown the report
/// (a history that kept everything, where nothing travelled) and ignores that member.
pub async fn mark_adopted(conn: &mut Conn, segment: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE segments
            SET import_fidelity = 'lossless',
                import_report = jsonb_build_object(
                    'fidelity', 'lossless', 'items_total', 0, 'by_kind', '{}'::jsonb, 'dropped', '[]'::jsonb,
                    'warnings', jsonb_build_array($2::text), 'adopted', 'pending')
          WHERE id = $1",
    )
    .bind(segment)
    .bind(REPORT_WARNING)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether the session of a segment was begun outside Orch and has not been opened by it yet: the
/// first open of such a session is an adoption, and every later one a resume.
pub async fn is_pending(conn: &mut Conn, segment: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT import_report ->> 'adopted' = 'pending' FROM segments WHERE id = $1), false)",
    )
    .bind(segment)
    .fetch_one(conn)
    .await?)
}

/// Records that Orch opened the session of a segment: one that was waiting to be adopted is adopted.
pub async fn mark_opened(conn: &mut Conn, segment: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE segments SET import_report = jsonb_set(import_report, '{adopted}', '\"opened\"')
          WHERE id = $1 AND import_report ->> 'adopted' = 'pending'",
    )
    .bind(segment)
    .execute(conn)
    .await?;
    Ok(())
}

/// Who began the sessions of a machine by hand: its owner, for a person's machine; nobody for a machine Orch
/// manages, where no person is the harness's user.
pub async fn machine_owner(conn: &mut Conn, machine: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT owner_id FROM machines WHERE id = $1")
            .bind(machine)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// Marks an item as one from the session before Orch, said or done at `at` where the harness recorded when:
/// read-only, in no turn, and never sent to the agent again.
pub async fn mark_before_orch(
    conn: &mut Conn,
    item: Id,
    at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE items SET before_orch = true,
                          created_at = COALESCE($2, created_at), updated_at = COALESCE($2, updated_at)
          WHERE id = $1",
    )
    .bind(item)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Replaces what the segment of an adopted session says it holds of the session's history with `report`, what
/// was taken in of the harness's record, keeping whether Orch has opened the session yet.
pub async fn set_adopted_report(
    conn: &mut Conn,
    segment: Id,
    report: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE segments
            SET import_fidelity = COALESCE($2::jsonb ->> 'fidelity', import_fidelity),
                import_report = jsonb_set($2::jsonb, '{adopted}',
                                          COALESCE(import_report -> 'adopted', '\"pending\"'::jsonb))
          WHERE id = $1",
    )
    .bind(segment)
    .bind(report)
    .execute(conn)
    .await?;
    Ok(())
}
