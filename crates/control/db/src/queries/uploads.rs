//! Completed upload ownership, saved sources of file bytes and serialized draft references.
//!
//! A content hash identifies bytes, not who may use them. Each uploader keeps
//! their own reference even when another person uploaded identical bytes.
//! Saved sources are candidates only: their current read authority is checked
//! by the caller before a copy is retained.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id};

/// Keeps an authorized personal reference in the upload or copy transaction.
pub async fn remember(
    conn: &mut Conn,
    user: Id,
    hash: &Hash,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO completed_uploads (user_id, hash, created_at) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, hash) DO NOTHING",
    )
    .bind(user)
    .bind(hash.as_bytes().as_slice())
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether this person retained these bytes through an upload or authorized copy.
pub async fn owns(conn: &mut Conn, user: Id, hash: &Hash) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM completed_uploads WHERE user_id = $1 AND hash = $2)",
    )
    .bind(user)
    .bind(hash.as_bytes().as_slice())
    .fetch_one(conn)
    .await?)
}

/// Readable source candidates, including this person's thread drafts.
/// Private chat drafts require a separately retained personal reference.
pub async fn sources(conn: &mut Conn, user: Id, hash: &Hash) -> Result<Vec<(String, Id)>, DbError> {
    let blob = hash.to_string();
    let direct = serde_json::json!([{ "blob": blob }]);
    let images = serde_json::json!([{ "images": [{ "blob": blob }] }]);
    let attachments = serde_json::json!([{ "attachments": [{ "blob": blob }] }]);
    Ok(sqlx::query_as(
        "SELECT 'chat' AS kind, chat_id AS id FROM chat_messages
          WHERE deleted_at IS NULL AND (content @> $1 OR content @> $2 OR content @> $3)
         UNION
         SELECT 'thread', thread_id FROM thread_inputs
          WHERE content @> $1 OR content @> $2 OR content @> $3
         UNION
         SELECT 'thread_draft', thread_id FROM drafts
          WHERE user_id = $4 AND (content @> $1 OR content @> $2 OR content @> $3)
         UNION
         SELECT 'document', document_id FROM document_files WHERE blob = $5
         UNION
         SELECT 'environment', environment_id FROM file_cache WHERE hash = $5",
    )
    .bind(direct)
    .bind(images)
    .bind(attachments)
    .bind(user)
    .bind(hash.as_bytes().as_slice())
    .fetch_all(conn)
    .await?)
}

/// Serializes replacing and discarding one person's draft, even before its first save.
pub async fn lock_chat_draft(conn: &mut Conn, user: Id, chat: Id) -> Result<(), DbError> {
    super::chats::lock_named(conn, &format!("chat-draft:{user}:{chat}")).await
}

/// Locks each referenced blob in hash order until commit, excluding concurrent collection.
pub async fn lock_blobs(conn: &mut Conn, hashes: &[Hash]) -> Result<(), DbError> {
    if hashes.is_empty() {
        return Ok(());
    }
    let raw: Vec<Vec<u8>> = hashes.iter().map(|hash| hash.as_bytes().to_vec()).collect();
    sqlx::query("SELECT hash FROM blobs WHERE hash = ANY($1) ORDER BY hash FOR UPDATE")
        .bind(raw)
        .fetch_all(conn)
        .await?;
    Ok(())
}
