//! Personal discussion choices and current discussion-start writers for documents.
//!
//! Discussion choices share the document's existing per-person row. Choosing
//! a bell never advances its seen position or changes the document itself.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use orch_types::Id;

use crate::{Conn, DbError};

/// The caller's stored discussion choices, with absent rows left to the default.
pub async fn levels(conn: &mut Conn, user: Id, ids: &[Id]) -> Result<HashMap<Id, String>, DbError> {
    let rows: Vec<(Id, String)> = sqlx::query_as(
        "SELECT document_id, COALESCE(to_jsonb(s)->>'discussion_level', 'involved')
           FROM document_states s WHERE user_id = $1 AND document_id = ANY($2)",
    )
    .bind(user)
    .bind(ids)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Changes only the caller's discussion bell on this document.
pub async fn set_level(
    conn: &mut Conn,
    user: Id,
    document: Id,
    level: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO document_states (user_id, document_id, discussion_level, updated_at)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, document_id) DO UPDATE
         SET discussion_level = EXCLUDED.discussion_level",
    )
    .bind(user)
    .bind(document)
    .bind(level)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// People who chose all discussions; their current reading is checked by the producer.
pub async fn all_subscribers(conn: &mut Conn, document: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM document_states s
          WHERE document_id = $1 AND to_jsonb(s)->>'discussion_level' = 'all'",
    )
    .bind(document)
    .fetch_all(conn)
    .await?)
}

/// The last writer of a block, with an agent represented by its thread's current first owner.
pub async fn block_writer(conn: &mut Conn, document: Id, block: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(b.updated_by, n.owner_id)
           FROM document_blocks b LEFT JOIN nodes n ON n.id = b.updated_by_thread AND n.deleted_at IS NULL
          WHERE b.document_id = $1 AND b.id = $2 AND b.removed_at IS NULL",
    )
    .bind(document)
    .bind(block)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// Reserves an immutable boundary for a discussion notice's subscriber audience.
pub async fn subscription_cutoff(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT nextval('document_discussion_subscription_seq')")
            .fetch_one(conn)
            .await?,
    )
}

/// Participants and subscribers are indexed scalar identities, with one keyset page held at a time.
pub const DISCUSSION_AUDIENCE_SQL: &str = "SELECT user_id FROM (
    (SELECT user_id FROM document_states WHERE document_id=$1 AND discussion_level='all'
      AND COALESCE(discussion_subscription_seq,0)<=$5 AND ($3::uuid IS NULL OR user_id>$3) ORDER BY user_id LIMIT $7)
    UNION (SELECT user_id FROM document_discussion_participants WHERE discussion_id=$2 AND NOT $6
      AND first_seq<=$4 AND ($3::uuid IS NULL OR user_id>$3) ORDER BY user_id LIMIT $7)
  ) audience ORDER BY user_id LIMIT $7";

/// One bounded audience page from the event's first-involvement and subscription cutoffs.
#[expect(
    clippy::too_many_arguments,
    reason = "one bounded audience page carries its target, event snapshot and continuation"
)]
pub async fn discussion_audience_page(
    conn: &mut Conn,
    document: Id,
    discussion: Id,
    after: Option<Id>,
    sequence: i64,
    subscriptions: i64,
    started: bool,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(DISCUSSION_AUDIENCE_SQL)
        .bind(document)
        .bind(discussion)
        .bind(after)
        .bind(sequence)
        .bind(subscriptions)
        .bind(started)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}
