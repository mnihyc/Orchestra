//! Indexed deferred notices keep append, retry and next-turn reads independent of queue history.

use crate::{Conn, DbError};
use orch_types::{Id, Json};

/// A turn reads at most this many notices; later values remain for subsequent turns.
/// The same prompt budget caps the compatibility preview and every deletion batch.
pub const PAGE_SIZE: usize = 64;

/// A queued value and its immutable keyset cursor.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NoticeRow {
    /// The position within the thread's queue.
    pub position: i64,
    /// The complete original payload.
    pub notice: Json,
}

/// Adds a value once by its authored identity, preserving existing legacy duplicates.
pub async fn append(conn: &mut Conn, thread: Id, notice: &Json) -> Result<bool, DbError> {
    Ok(sqlx::query(include_str!("notices_append.sql"))
        .bind(thread)
        .bind(notice)
        .execute(conn)
        .await?
        .rows_affected()
        != 0)
}

/// Whether an identity is already queued, including beyond the first delivery page.
pub async fn contains(conn: &mut Conn, thread: Id, id: &str) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_pending_notices WHERE thread_id=$1 AND notice_id=$2)",
    )
    .bind(thread)
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// Reads one page strictly after the previous page's cursor.
pub async fn page(
    conn: &mut Conn,
    thread: Id,
    after: Option<i64>,
) -> Result<Vec<NoticeRow>, DbError> {
    Ok(sqlx::query_as(include_str!("notices_page.sql"))
        .bind(thread)
        .bind(after.unwrap_or(0))
        .fetch_all(conn)
        .await?)
}

/// Reads a native recovery handoff before ordinary news, from two indexed bounded lanes.
pub async fn for_segment(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
) -> Result<Vec<NoticeRow>, DbError> {
    Ok(sqlx::query_as(include_str!("notices_segment.sql"))
        .bind(thread)
        .bind(segment)
        .fetch_all(conn)
        .await?)
}

/// Removes only delivered identities in bounded batches; new arrivals stay queued.
pub async fn forget(conn: &mut Conn, thread: Id, said: &[String]) -> Result<bool, DbError> {
    let mut changed = false;
    for batch in said.chunks(PAGE_SIZE) {
        changed |= sqlx::query(include_str!("notices_forget.sql"))
            .bind(thread)
            .bind(batch)
            .execute(&mut *conn)
            .await?
            .rows_affected()
            != 0;
    }
    Ok(changed)
}

/// Returns a turn's immutable delivery page, if its input has been prepared.
pub async fn delivery_positions(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
) -> Result<Option<Vec<i64>>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT pending_notice_positions FROM turns WHERE id=$1 AND thread_id=$2",
    )
    .bind(turn)
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// Pins the first prepared input so an accepted-command retry cannot acknowledge a later page.
pub async fn pin_delivery(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
    positions: &[i64],
) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET pending_notice_positions=COALESCE(pending_notice_positions,$3::bigint[]) WHERE id=$1 AND thread_id=$2")
        .bind(turn).bind(thread).bind(positions).execute(conn).await?;
    Ok(())
}

/// Reloads the pinned page in delivery order, excluding rows already acknowledged.
pub async fn delivery_page(
    conn: &mut Conn,
    thread: Id,
    positions: &[i64],
) -> Result<Vec<NoticeRow>, DbError> {
    Ok(sqlx::query_as(include_str!("notices_delivery.sql"))
        .bind(thread)
        .bind(positions)
        .fetch_all(conn)
        .await?)
}

/// Acknowledges exact queue positions, including values without authored identifiers.
pub async fn forget_positions(
    conn: &mut Conn,
    thread: Id,
    positions: &[i64],
) -> Result<bool, DbError> {
    let mut changed = false;
    for batch in positions.chunks(PAGE_SIZE) {
        changed |= sqlx::query(include_str!("notices_forget_positions.sql"))
            .bind(thread)
            .bind(batch)
            .execute(&mut *conn)
            .await?
            .rows_affected()
            != 0;
    }
    Ok(changed)
}
