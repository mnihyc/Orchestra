//! What bringing the messages of a chat needs beyond the ordinary queries of chats: which messages of a source are here already, the messages an import adds in one statement with the original time and author, and the taking back of them.
//!
//! The ordinary queries of chats write a message as somebody sends it, at the moment of sending. An import writes a message that
//! was sent long ago, so what is here adds to them: a range of the chat's sequence taken at once (the chat's row is locked by
//! whoever takes it, as everywhere else), the marks a message carries when it arrives (reactions, a pin, the moment it was edited),
//! the chat's newest message moved forward and never back, and the rows of `import_origins` written together.

use crate::queries::imports::OriginRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json, Seq};
use std::collections::HashSet;

/// Which of the messages `ids` of the chat `chat` are there, each with whether it was deleted. A message that is not there is
/// not among them.
pub async fn present_messages(
    conn: &mut Conn,
    chat: Id,
    ids: &[Id],
) -> Result<Vec<(Id, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, deleted_at IS NOT NULL FROM chat_messages WHERE chat_id = $1 AND id = ANY($2)",
    )
    .bind(chat)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Which requested file hashes a live message of this chat already carries.
/// A package may reuse those files when adding another message to the same chat;
/// global blob existence alone does not make a file this chat's to share.
pub async fn held_blobs(
    conn: &mut Conn,
    chat: Id,
    hashes: &HashSet<Hash>,
) -> Result<HashSet<Hash>, DbError> {
    if hashes.is_empty() {
        return Ok(HashSet::new());
    }
    let wanted: Vec<String> = hashes.iter().map(ToString::to_string).collect();
    // One pass over the destination chat per imported page, with at most one result
    // per requested hash. Only the file-bearing part shapes count; deleted messages
    // have given up their files.
    let found: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT candidate.hash
           FROM chat_messages m
           CROSS JOIN LATERAL jsonb_array_elements(
             CASE WHEN jsonb_typeof(m.content) = 'array' THEN m.content ELSE '[]'::jsonb END
           ) AS part(value)
           CROSS JOIN LATERAL (
             SELECT part.value->>'blob' AS hash
               WHERE part.value->>'type' IN ('image', 'attachment', 'pasted_text')
             UNION ALL
             SELECT image.value->>'blob' AS hash
               FROM jsonb_array_elements(
                 CASE WHEN part.value->>'type' = 'forwarded'
                            AND jsonb_typeof(part.value->'images') = 'array'
                      THEN part.value->'images' ELSE '[]'::jsonb END
               ) AS image(value)
             UNION ALL
             SELECT attachment.value->>'blob' AS hash
               FROM jsonb_array_elements(
                 CASE WHEN part.value->>'type' = 'forwarded'
                            AND jsonb_typeof(part.value->'attachments') = 'array'
                      THEN part.value->'attachments' ELSE '[]'::jsonb END
               ) AS attachment(value)
           ) AS candidate
          WHERE m.chat_id = $1 AND m.kind = 'message' AND m.deleted_at IS NULL
            AND candidate.hash = ANY($2)",
    )
    .bind(chat)
    .bind(&wanted)
    .fetch_all(conn)
    .await?;
    Ok(found
        .into_iter()
        .filter_map(|word| word.parse().ok())
        .collect())
}

/// Takes `count` numbers of the sequence of `chat` at once and answers the first of them; none where there is no such chat.
/// The chat's row has to be locked until the end of the transaction, which taking the numbers does as well.
pub async fn take_seqs(conn: &mut Conn, chat: Id, count: i64) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + $2 WHERE id = $1 RETURNING next_seq - $2",
    )
    .bind(chat)
    .bind(count)
    .fetch_optional(conn)
    .await?)
}

/// Writes what a message that is brought carries beyond what it says: how people reacted (`{emoji: [user ids]}`), who pinned it
/// and when, and when it was edited. The message was written a moment before, so the number it changed at is the one it was
/// written at.
pub async fn set_marks(
    conn: &mut Conn,
    id: Id,
    reactions: Option<&Json>,
    pinned: Option<(Id, DateTime<Utc>)>,
    edited_at: Option<DateTime<Utc>>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE chat_messages SET reactions = COALESCE($2, reactions), pinned_by = $3, pinned_at = $4, edited_at = $5
          WHERE id = $1",
    )
    .bind(id)
    .bind(reactions)
    .bind(pinned.map(|(by, _)| by))
    .bind(pinned.map(|(_, at)| at))
    .bind(edited_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Moves the newest message of `chat` to the one written at `seq` and `at`, unless a newer one is there already.
pub async fn touch_last(
    conn: &mut Conn,
    chat: Id,
    seq: Seq,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE chats SET last_message_seq = GREATEST(last_message_seq, $2),
                last_message_at = GREATEST(last_message_at, $3)
          WHERE id = $1",
    )
    .bind(chat)
    .bind(seq)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Writes what many things of a source are here, in one statement. A row that is there is written over by the new one whole:
/// the message that an earlier import added and an undo took back has another identifier here now.
pub async fn put_origins(conn: &mut Conn, rows: &[OriginRow]) -> Result<(), DbError> {
    if rows.is_empty() {
        return Ok(());
    }
    let scopes: Vec<Id> = rows.iter().map(|row| row.scope_id).collect();
    let sources: Vec<Id> = rows.iter().map(|row| row.source_id).collect();
    let kinds: Vec<String> = rows.iter().map(|row| row.kind.clone()).collect();
    let objects: Vec<Id> = rows.iter().map(|row| row.object_id).collect();
    let parents: Vec<Option<Id>> = rows.iter().map(|row| row.parent_id).collect();
    let makers: Vec<Option<Id>> = rows.iter().map(|row| row.created_by_import).collect();
    let lasts: Vec<Id> = rows.iter().map(|row| row.last_import).collect();
    let names: Vec<Option<String>> = rows.iter().map(|row| row.source_name.clone()).collect();
    let paths: Vec<Option<String>> = rows.iter().map(|row| row.source_path.clone()).collect();
    let hashes: Vec<Option<String>> = rows.iter().map(|row| row.source_sha256.clone()).collect();
    let changed: Vec<Option<DateTime<Utc>>> =
        rows.iter().map(|row| row.source_updated_at).collect();
    let creators: Vec<Option<Json>> = rows.iter().map(|row| row.source_creator.clone()).collect();
    let by: Vec<Option<Id>> = rows.iter().map(|row| row.imported_by).collect();
    let at: Vec<DateTime<Utc>> = rows.iter().map(|row| row.imported_at).collect();
    sqlx::query(
        "INSERT INTO import_origins (scope_id, source_id, kind, object_id, parent_id, created_by_import, last_import,
                source_name, source_path, source_sha256, source_updated_at, source_creator, imported_by, imported_at)
         SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::text[], $4::uuid[], $5::uuid[], $6::uuid[], $7::uuid[],
                $8::text[], $9::text[], $10::text[], $11::timestamptz[], $12::jsonb[], $13::uuid[], $14::timestamptz[])
         ON CONFLICT (scope_id, source_id) DO UPDATE SET
            kind = EXCLUDED.kind,
            object_id = EXCLUDED.object_id,
            parent_id = EXCLUDED.parent_id,
            created_by_import = EXCLUDED.created_by_import,
            last_import = EXCLUDED.last_import,
            source_name = EXCLUDED.source_name,
            source_path = EXCLUDED.source_path,
            source_sha256 = EXCLUDED.source_sha256,
            source_updated_at = EXCLUDED.source_updated_at,
            source_creator = EXCLUDED.source_creator,
            imported_by = EXCLUDED.imported_by,
            imported_at = EXCLUDED.imported_at",
    )
    .bind(&scopes)
    .bind(&sources)
    .bind(&kinds)
    .bind(&objects)
    .bind(&parents)
    .bind(&makers)
    .bind(&lasts)
    .bind(&names)
    .bind(&paths)
    .bind(&hashes)
    .bind(&changed)
    .bind(&creators)
    .bind(&by)
    .bind(&at)
    .execute(conn)
    .await?;
    Ok(())
}

/// A page of the messages that the import `import` added to the chat `chat`: at most `limit` of the rows that say so, by identifier, each
/// with whether its message is still there (written, and not deleted since).
pub async fn added_to_chat(
    conn: &mut Conn,
    import: Id,
    chat: Id,
    limit: i64,
) -> Result<Vec<(Id, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.object_id, (m.id IS NOT NULL AND m.deleted_at IS NULL)
           FROM import_origins o
           LEFT JOIN chat_messages m ON m.id = o.object_id AND m.chat_id = o.parent_id
          WHERE o.created_by_import = $1 AND o.kind = 'message' AND o.parent_id = $2
          ORDER BY o.object_id LIMIT $3",
    )
    .bind(import)
    .bind(chat)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Whether the place a chat belongs to is still there: what lies in a place that was deleted goes and comes back with it.
pub async fn place_is_live(conn: &mut Conn, node: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM nodes WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(node)
    .fetch_one(conn)
    .await?)
}

/// Deletes the messages `ids` of the chat `chat` the way a chat deletes a message, in one statement: the row stays, with what it said
/// emptied, at the number of the chat's sequence that stands at its place in `seqs`. A message that was deleted already is left as it is.
/// The answer is what each message deleted by this call said, so that the files it carried can be let go.
pub async fn delete_messages(
    conn: &mut Conn,
    chat: Id,
    ids: &[Id],
    seqs: &[Seq],
    by: Id,
    at: DateTime<Utc>,
) -> Result<Vec<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "WITH gone AS (
             SELECT m.id, m.content, x.seq
               FROM chat_messages m JOIN UNNEST($1::uuid[], $2::bigint[]) AS x(id, seq) ON x.id = m.id
              WHERE m.chat_id = $3 AND m.deleted_at IS NULL
                FOR UPDATE OF m),
         done AS (
             UPDATE chat_messages m
                SET content = '[]'::jsonb, mentions = '{}', mentions_all = false, mention_targets = '[]'::jsonb,
                    asked_agents = '[]'::jsonb, reactions = '{}'::jsonb, search_text = NULL, pinned_at = NULL, pinned_by = NULL,
                    deleted_at = $4, deleted_by = $5, seq_updated = gone.seq
               FROM gone WHERE m.id = gone.id
             RETURNING m.id)
         SELECT gone.content FROM gone JOIN done ON done.id = gone.id",
    )
    .bind(ids)
    .bind(seqs)
    .bind(chat)
    .bind(at)
    .bind(by)
    .fetch_all(conn)
    .await?)
}

/// Lets go of files that messages carried: a file is pinned once for each message that carries it, so one that several of the hashes
/// name (a hash written once for each message) is let go that many times.
pub async fn unpin_blobs(conn: &mut Conn, hashes: &[Vec<u8>]) -> Result<(), DbError> {
    if hashes.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE blobs b SET pin_count = GREATEST(b.pin_count - c.n, 0)
           FROM (SELECT t.h, count(*)::int AS n FROM UNNEST($1::bytea[]) AS t(h) GROUP BY t.h) c
          WHERE b.hash = c.h",
    )
    .bind(hashes)
    .execute(conn)
    .await?;
    Ok(())
}
