//! Archives of what a person may read in a place: whose each is, what was asked, where it stands and for how long it is kept, and the statements that count what an archive would hold.
//!
//! A row of `exports` is made with the job that writes the archive and says, once the job ends, where the
//! archive is in the store of content (a blob that is pinned for as long as the archive is kept), how large it is
//! and what is in it and what is not. Nothing here decides who may take what away: the counts are of what a
//! caller asked the statements about, and the access engine has said which of it is theirs. The sizes are
//! estimates read from what is stored (`pg_column_size` of the rows that make a conversation, a chat or a
//! document), cheap enough to ask again each time a screen's ticks change.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json};
use std::collections::HashMap;

/// The words of `exports.state`.
pub mod state {
    /// A job is writing it.
    pub const PENDING: &str = "pending";
    /// It can be fetched.
    pub const READY: &str = "ready";
    /// The job failed or was cancelled.
    pub const FAILED: &str = "failed";
    /// It was removed.
    pub const EXPIRED: &str = "expired";
}

/// How many days the row of an archive that was removed, or that failed, stays: long enough for its person, and
/// whoever audits, to see that it was made.
pub const ROW_KEPT_DAYS: i64 = 30;

/// How many bytes a message of a chat comes to beside what it says: the line that names its number, its author and
/// its time, and the line it ends with.
pub const MESSAGE_HEAD_BYTES: i64 = 64;

/// A span of moments: the turns and messages that were made inside it. Either end may be open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    /// The first moment inside it.
    pub from: Option<DateTime<Utc>>,
    /// The first moment after it.
    pub until: Option<DateTime<Utc>>,
}

/// An archive, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ExportRow {
    /// Its identifier.
    pub id: Id,
    /// Whose it is: nobody else fetches it. None once the person is gone: the next sweep removes the archive then.
    pub user_id: Option<Id>,
    /// The place it was made of. None once the place was purged; the archive stays its person's until its time.
    pub node_id: Option<Id>,
    /// What was asked, as the request was written.
    pub request: Json,
    /// The job that writes it.
    pub job_id: Option<Id>,
    /// Where it stands.
    pub state: String,
    /// The archive in the store of content, as the hash is written, once it is ready.
    pub blob: Option<String>,
    /// Its size once it is ready.
    pub bytes: Option<i64>,
    /// How many conversations, chats, documents and files it holds.
    pub counts: Json,
    /// What was asked for and is not in it, each with why.
    pub left_out: Json,
    /// When it was asked for.
    pub created_at: DateTime<Utc>,
    /// When it became ready.
    pub ready_at: Option<DateTime<Utc>>,
    /// When it is removed.
    pub expires_at: Option<DateTime<Utc>>,
    /// When it was first fetched.
    pub downloaded_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, user_id, node_id, request, job_id, state, blob, bytes, counts, left_out, created_at, ready_at,
    expires_at, downloaded_at";

/// Makes the row of an archive that is being asked for.
pub async fn insert(
    conn: &mut Conn,
    id: Id,
    user: Id,
    node: Id,
    request: &Json,
    at: DateTime<Utc>,
) -> Result<ExportRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO exports (id, user_id, node_id, request, state, created_at)
         VALUES ($1, $2, $3, $4, 'pending', $5)
         RETURNING {COLUMNS}"
    )))
    .bind(id)
    .bind(user)
    .bind(node)
    .bind(request)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Says which job writes an archive.
pub async fn attach_job(conn: &mut Conn, id: Id, job: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE exports SET job_id = $2 WHERE id = $1")
        .bind(id)
        .bind(job)
        .execute(conn)
        .await?;
    Ok(())
}

/// An archive.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<ExportRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM exports WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// An archive, locked until the transaction ends.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<ExportRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM exports WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The archives of a person, newest first: the page after `after` (the moment and the identifier of the last one
/// shown), `limit` of them.
pub async fn of_user(
    conn: &mut Conn,
    user: Id,
    after: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<Vec<ExportRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM exports
          WHERE user_id = $1
            AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
          ORDER BY created_at DESC, id DESC LIMIT $4"
    )))
    .bind(user)
    .bind(after.map(|each| each.0))
    .bind(after.map(|each| each.1))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Takes the lock that makes one person's archives counted and begun one request at a time, until the transaction
/// ends: two requests that come together do not both find room for the last place.
pub async fn lock_person(conn: &mut Conn, user: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('exports:' || $1::text, 0))")
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

/// How many archives of a person are being written now.
pub async fn writing(conn: &mut Conn, user: Id) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM exports WHERE user_id = $1 AND state = 'pending'")
            .bind(user)
            .fetch_one(conn)
            .await?,
    )
}

/// What an archive that is written holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Written<'a> {
    /// The archive in the store, as the hash is written.
    pub blob: &'a str,
    /// Its size.
    pub bytes: i64,
    /// How many of each kind it holds.
    pub counts: &'a Json,
    /// What is not in it.
    pub left_out: &'a Json,
    /// When it became ready.
    pub at: DateTime<Utc>,
    /// When it is removed.
    pub expires_at: DateTime<Utc>,
}

/// Makes an archive ready. Says whether it was: one that was removed, or failed, meanwhile is not.
pub async fn make_ready(conn: &mut Conn, id: Id, written: &Written<'_>) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE exports SET state = 'ready', blob = $2, bytes = $3, counts = $4, left_out = $5, ready_at = $6,
                            expires_at = $7
          WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(written.blob)
    .bind(written.bytes)
    .bind(written.counts)
    .bind(written.left_out)
    .bind(written.at)
    .bind(written.expires_at)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Says that the job that writes an archive failed or was cancelled: nothing was kept.
pub async fn make_failed(conn: &mut Conn, id: Id, left_out: &Json) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE exports SET state = 'failed', left_out = $2 WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(left_out)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Removes an archive: its state says so, and the blob it held is let go. Gives the hash of that blob where the
/// archive had one, for whoever pins and stores to let it go; nothing where it was removed already.
pub async fn expire(conn: &mut Conn, id: Id) -> Result<Option<Option<String>>, DbError> {
    let before: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT state, blob FROM exports WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((state, blob)) = before else {
        return Ok(None);
    };
    if state == "expired" {
        return Ok(None);
    }
    sqlx::query("UPDATE exports SET state = 'expired', blob = NULL, expires_at = COALESCE(expires_at, now()) WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(Some(blob))
}

/// Records that an archive was fetched, and says whether that was the first time.
pub async fn first_download(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    Ok(
        sqlx::query(
            "UPDATE exports SET downloaded_at = $2 WHERE id = $1 AND downloaded_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            > 0,
    )
}

/// Removes the archives that are to go, `limit` of them at most and the oldest first: those that are ready and whose
/// time has run out, those that are ready and whose person is gone, and any that failed and still hold a blob (none
/// should). Each is `expired` as [`expire`] makes it, its row lets go of the blob and the blob's pin is taken off with
/// its grace at an end, so that the sweep of the store takes its bytes out at its next run. An archive that another
/// transaction holds is left to it. Gives the archives that were removed. One whose place is gone and whose person is
/// not stays its person's until its time.
pub async fn expire_due(
    conn: &mut Conn,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    let gone: Vec<(Id, Option<String>)> = sqlx::query_as(
        "WITH due AS (
             SELECT id, blob FROM exports
              WHERE (state = 'ready' AND (user_id IS NULL OR (expires_at IS NOT NULL AND expires_at <= $1)))
                 OR (state = 'failed' AND blob IS NOT NULL)
              ORDER BY expires_at NULLS FIRST, id LIMIT $2
                FOR UPDATE SKIP LOCKED
         )
         UPDATE exports e SET state = 'expired', blob = NULL
           FROM due WHERE e.id = due.id
         RETURNING e.id, due.blob",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    for (_, blob) in &gone {
        let Some(blob) = blob else { continue };
        sqlx::query(
            "UPDATE blobs
                SET pin_count = pin_count - 1,
                    last_access_at = CASE WHEN pin_count = 1 THEN to_timestamp(0) ELSE last_access_at END
              WHERE hash = decode($1, 'hex') AND pin_count > 0",
        )
        .bind(blob)
        .execute(&mut *conn)
        .await?;
    }
    Ok(gone.into_iter().map(|(id, _)| id).collect())
}

/// Forgets the rows of archives that were removed, or that failed, before `before`: they stayed so that their
/// person and an auditor could see that they were made. Says how many went.
pub async fn forget_before(conn: &mut Conn, before: DateTime<Utc>) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM exports
          WHERE state IN ('expired', 'failed') AND COALESCE(expires_at, ready_at, created_at) < $1",
    )
    .bind(before)
    .execute(conn)
    .await?
    .rows_affected())
}

// ---------------------------------------------------------------- what an archive would hold

/// What is stored of the conversation of a thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConversationFacts {
    /// The size of the items of its main stream that are written, as they are stored.
    pub bytes: i64,
}

/// The size of the conversations of these threads, within a span.
pub async fn conversation_facts(
    conn: &mut Conn,
    threads: &[Id],
    span: Span,
) -> Result<HashMap<Id, ConversationFacts>, DbError> {
    let sql = format!(
        "{}{} SELECT i.thread_id,COALESCE(sum(pg_column_size(i.body->'payload')),0)::bigint,
                count(*) FILTER(WHERE i.body IS NULL)::bigint
           FROM visible_items i
          WHERE i.stream = 'main' AND i.kind <> 'reasoning'
            AND ($2::timestamptz IS NULL OR i.created_at >= $2)
            AND ($3::timestamptz IS NULL OR i.created_at < $3)
          GROUP BY i.thread_id",
        super::thread_continuations::reading::SELECTED_OWNERS,
        super::thread_history::SELECTED_ITEMS
    );
    let rows: Vec<(Id, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(threads)
        .bind(span.from)
        .bind(span.until)
        .fetch_all(conn)
        .await?;
    if rows.iter().any(|row| row.2 != 0) {
        return Err(super::thread_history::damaged());
    }
    Ok(rows
        .into_iter()
        .map(|(thread, bytes, _)| (thread, ConversationFacts { bytes }))
        .collect())
}

/// What is stored of the chat of a node.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ChatFacts {
    /// The chat.
    pub chat_id: Id,
    /// Its node.
    pub node_id: Id,
    /// How many messages it holds in the span, those deleted and the lines of the system left out.
    pub messages: i64,
    /// What they come to as they are stored, with the line each is given in its written form.
    pub bytes: i64,
}

/// The chats of these nodes with what they hold inside a span.
pub async fn chat_facts(
    conn: &mut Conn,
    nodes: &[Id],
    span: Span,
) -> Result<Vec<ChatFacts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id AS chat_id, c.node_id, count(m.id) AS messages,
                COALESCE(sum(pg_column_size(m.content) + $4), 0)::bigint AS bytes
           FROM chats c
           LEFT JOIN chat_messages m
             ON m.chat_id = c.id AND m.deleted_at IS NULL AND m.kind = 'message'
            AND ($2::timestamptz IS NULL OR m.created_at >= $2)
            AND ($3::timestamptz IS NULL OR m.created_at < $3)
          WHERE c.node_id = ANY($1)
          GROUP BY c.id, c.node_id",
    )
    .bind(nodes)
    .bind(span.from)
    .bind(span.until)
    .bind(MESSAGE_HEAD_BYTES)
    .fetch_all(conn)
    .await?)
}

/// The chats `ids` of nodes, with what they hold inside a span.
pub async fn chat_facts_of(
    conn: &mut Conn,
    ids: &[Id],
    span: Span,
) -> Result<Vec<ChatFacts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id AS chat_id, c.node_id, count(m.id) AS messages,
                COALESCE(sum(pg_column_size(m.content) + $4), 0)::bigint AS bytes
           FROM chats c
           LEFT JOIN chat_messages m
             ON m.chat_id = c.id AND m.deleted_at IS NULL AND m.kind = 'message'
            AND ($2::timestamptz IS NULL OR m.created_at >= $2)
            AND ($3::timestamptz IS NULL OR m.created_at < $3)
          WHERE c.id = ANY($1) AND c.node_id IS NOT NULL
          GROUP BY c.id, c.node_id",
    )
    .bind(ids)
    .bind(span.from)
    .bind(span.until)
    .bind(MESSAGE_HEAD_BYTES)
    .fetch_all(conn)
    .await?)
}

/// What is stored of a document.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DocumentFacts {
    /// The document.
    pub id: Id,
    /// The node it is written beside.
    pub node_id: Id,
    /// Its title.
    pub title: String,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// The size of its text.
    pub bytes: i64,
    /// How many earlier versions it has.
    pub versions: i64,
    /// What they come to together.
    pub version_bytes: i64,
    /// What its comments come to.
    pub comment_bytes: i64,
}

/// The documents `ids` that were not deleted, with what is stored of each.
pub async fn document_facts(conn: &mut Conn, ids: &[Id]) -> Result<Vec<DocumentFacts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT d.id, d.node_id, d.title, d.created_at, d.bytes,
                (SELECT count(*) FROM document_versions v WHERE v.document_id = d.id) AS versions,
                (SELECT COALESCE(sum(v.bytes), 0) FROM document_versions v WHERE v.document_id = d.id)::bigint
                    AS version_bytes,
                (SELECT COALESCE(sum(pg_column_size(c.content)), 0) FROM document_comments c
                  WHERE c.document_id = d.id AND c.deleted_at IS NULL)::bigint AS comment_bytes
           FROM documents d
          WHERE d.id = ANY($1) AND d.deleted_at IS NULL
          ORDER BY d.created_at, d.id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The files something carries: how many, how large together, and how many are past the largest an archive takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct FileFacts {
    /// How many files an archive takes in.
    pub files: i64,
    /// What they come to together.
    pub bytes: i64,
    /// How many are larger than the most one file may be, and are left out.
    pub too_large: i64,
}

/// The files that the messages of these chats carry, and those of the messages forwarded in them, by chat. `most` is the largest file an archive takes.
pub async fn chat_files(
    conn: &mut Conn,
    chats: &[Id],
    most: i64,
) -> Result<HashMap<Id, FileFacts>, DbError> {
    let rows: Vec<(Id, i64, i64, i64)> = sqlx::query_as(
        "SELECT m.chat_id,
                count(*) FILTER (WHERE b.size <= $2),
                COALESCE(sum(b.size) FILTER (WHERE b.size <= $2), 0)::bigint,
                count(*) FILTER (WHERE b.size > $2)
           FROM chat_messages m
          CROSS JOIN LATERAL jsonb_array_elements(
                CASE WHEN jsonb_typeof(m.content) = 'array' THEN m.content ELSE '[]'::jsonb END) AS part
          CROSS JOIN LATERAL (
                SELECT part ->> 'blob' AS blob WHERE part ->> 'type' IN ('image', 'attachment')
                UNION ALL
                SELECT copy ->> 'blob' FROM jsonb_array_elements(
                    CASE WHEN part ->> 'type' = 'forwarded' AND jsonb_typeof(part -> 'images') = 'array'
                         THEN part -> 'images' ELSE '[]'::jsonb END) AS copy
                UNION ALL
                SELECT copy ->> 'blob' FROM jsonb_array_elements(
                    CASE WHEN part ->> 'type' = 'forwarded' AND jsonb_typeof(part -> 'attachments') = 'array'
                         THEN part -> 'attachments' ELSE '[]'::jsonb END) AS copy
          ) AS file
           JOIN blobs b ON b.hash = decode(file.blob, 'hex')
          WHERE m.chat_id = ANY($1) AND m.deleted_at IS NULL
          GROUP BY m.chat_id",
    )
    .bind(chats)
    .bind(most)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(chat, files, bytes, too_large)| {
            (
                chat,
                FileFacts {
                    files,
                    bytes,
                    too_large,
                },
            )
        })
        .collect())
}

/// The files that the inputs of these threads carried (an image, or a file with its content kept), by thread.
pub async fn input_files(
    conn: &mut Conn,
    threads: &[Id],
    most: i64,
    span: Span,
) -> Result<HashMap<Id, FileFacts>, DbError> {
    let sql = format!(
        "{}{} SELECT i.thread_id,
                count(*) FILTER (WHERE b.size <= $2),
                COALESCE(sum(b.size) FILTER (WHERE b.size <= $2), 0)::bigint,
                count(*) FILTER (WHERE b.size > $2),count(*) FILTER(WHERE i.body IS NULL)::bigint
           FROM visible_items i
          LEFT JOIN LATERAL jsonb_array_elements(
                CASE WHEN jsonb_typeof(i.body #> '{{payload,parts}}') = 'array'
                  THEN i.body #> '{{payload,parts}}' ELSE '[]'::jsonb END) AS part
            ON part ->> 'type' IN ('image','file')
          LEFT JOIN blobs b ON b.hash = decode(part ->> 'blob', 'hex')
          WHERE i.kind = 'user_message' AND i.stream = 'main'
            AND ($3::timestamptz IS NULL OR i.created_at >= $3)
            AND ($4::timestamptz IS NULL OR i.created_at < $4)
          GROUP BY i.thread_id",
        super::thread_continuations::reading::SELECTED_OWNERS,
        super::thread_history::SELECTED_ITEMS
    );
    let rows: Vec<(Id, i64, i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(threads)
        .bind(most)
        .bind(span.from)
        .bind(span.until)
        .fetch_all(conn)
        .await?;
    if rows.iter().any(|row| row.4 != 0) {
        return Err(super::thread_history::damaged());
    }
    Ok(rows
        .into_iter()
        .map(|(thread, files, bytes, too_large, _)| {
            (
                thread,
                FileFacts {
                    files,
                    bytes,
                    too_large,
                },
            )
        })
        .collect())
}

/// A file a document holds, with where its content is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentFile {
    /// The file's own identifier: a block of the document names it by this.
    pub id: Id,
    /// The document.
    pub document: Id,
    /// Its name among the files of the document.
    pub name: String,
    /// Its size.
    pub size: i64,
    /// Its content in the store.
    pub blob: Hash,
}

/// The files of these documents with their content, by document and name.
pub async fn document_files(
    conn: &mut Conn,
    documents: &[Id],
) -> Result<Vec<DocumentFile>, DbError> {
    let rows: Vec<(Id, Id, String, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT id, document_id, name, size, blob FROM document_files
          WHERE document_id = ANY($1) ORDER BY document_id, name",
    )
    .bind(documents)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|(id, document, name, size, raw)| {
            Ok(DocumentFile {
                id,
                document,
                name,
                size,
                blob: Hash::from_slice(&raw).map_err(|_| {
                    DbError::Other(sqlx::Error::Decode("a hash that is not 32 bytes".into()))
                })?,
            })
        })
        .collect()
}

/// The files of documents as facts: how many, how large, how many are past `most`.
pub async fn document_file_facts(
    conn: &mut Conn,
    documents: &[Id],
    most: i64,
) -> Result<HashMap<Id, FileFacts>, DbError> {
    let rows: Vec<(Id, i64, i64, i64)> = sqlx::query_as(
        "SELECT document_id,
                count(*) FILTER (WHERE size <= $2),
                COALESCE(sum(size) FILTER (WHERE size <= $2), 0)::bigint,
                count(*) FILTER (WHERE size > $2)
           FROM document_files WHERE document_id = ANY($1) GROUP BY document_id",
    )
    .bind(documents)
    .bind(most)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(document, files, bytes, too_large)| {
            (
                document,
                FileFacts {
                    files,
                    bytes,
                    too_large,
                },
            )
        })
        .collect())
}

// ---------------------------------------------------------------- spans of days

/// The numbers of the first and the last turn of a thread that began inside a span.
pub async fn turn_numbers_in(
    conn: &mut Conn,
    thread: Id,
    span: Span,
) -> Result<Option<(i64, i64)>, DbError> {
    let row: (Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT min(ordinal), max(ordinal) FROM turns
          WHERE thread_id = $1
            AND ($2::timestamptz IS NULL OR created_at >= $2)
            AND ($3::timestamptz IS NULL OR created_at < $3)",
    )
    .bind(thread)
    .bind(span.from)
    .bind(span.until)
    .fetch_one(conn)
    .await?;
    Ok(match row {
        (Some(first), Some(last)) => Some((i64::from(first), i64::from(last))),
        _ => None,
    })
}

/// The numbers of the first and the last message of a chat (as its written form counts them) made inside a span.
pub async fn message_numbers_in(
    conn: &mut Conn,
    chat: Id,
    span: Span,
) -> Result<Option<(i64, i64)>, DbError> {
    let row: (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT min(number), max(number)
           FROM (SELECT row_number() OVER (ORDER BY m.seq_created) AS number, m.created_at
                   FROM chat_messages m WHERE m.chat_id = $1) numbered
          WHERE ($2::timestamptz IS NULL OR created_at >= $2)
            AND ($3::timestamptz IS NULL OR created_at < $3)",
    )
    .bind(chat)
    .bind(span.from)
    .bind(span.until)
    .fetch_one(conn)
    .await?;
    Ok(match row {
        (Some(first), Some(last)) => Some((first, last)),
        _ => None,
    })
}

// ---------------------------------------------------------------- the people and places an archive names

/// The earlier versions of documents, newest first for each: their number, when the last change in them was made,
/// and the document.
pub async fn version_lines(
    conn: &mut Conn,
    documents: &[Id],
) -> Result<Vec<(Id, i32, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT document_id, version, to_at FROM document_versions
          WHERE document_id = ANY($1) ORDER BY document_id, version DESC",
    )
    .bind(documents)
    .fetch_all(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;

    #[tokio::test]
    async fn an_archive_is_made_ready_once_and_removed_once() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (user, node) = (new_id(), new_id());
        crate::queries::identity::testing::insert_user(&mut conn, user, "ada")
            .await
            .unwrap();
        crate::queries::hierarchy::testing::insert_node(&mut conn, node, "org", None, "Acme", user)
            .await
            .unwrap();
        let at = Utc::now();
        let id = new_id();
        let row = insert(
            &mut conn,
            id,
            user,
            node,
            &serde_json::json!({ "what": "all" }),
            at,
        )
        .await
        .unwrap();
        assert_eq!(row.state, state::PENDING);
        assert_eq!(writing(&mut conn, user).await.unwrap(), 1);
        let counts = serde_json::json!({ "places": 1 });
        let left_out = serde_json::json!([]);
        let written = Written {
            blob: "ab",
            bytes: 12,
            counts: &counts,
            left_out: &left_out,
            at,
            expires_at: at,
        };
        assert!(make_ready(&mut conn, id, &written).await.unwrap());
        assert!(
            !make_ready(&mut conn, id, &written).await.unwrap(),
            "ready once"
        );
        assert_eq!(writing(&mut conn, user).await.unwrap(), 0);
        assert!(first_download(&mut conn, id, at).await.unwrap());
        assert!(!first_download(&mut conn, id, at).await.unwrap());
        assert_eq!(
            expire(&mut conn, id).await.unwrap(),
            Some(Some("ab".to_owned()))
        );
        assert_eq!(expire(&mut conn, id).await.unwrap(), None, "removed once");
        let row = get(&mut conn, id).await.unwrap().unwrap();
        assert_eq!((row.state.as_str(), row.blob), (state::EXPIRED, None));
    }

    #[tokio::test]
    async fn the_archives_of_a_person_are_listed_newest_first_a_page_at_a_time_and_only_theirs() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bea, node) = (new_id(), new_id(), new_id());
        for (user, name) in [(ada, "ada"), (bea, "bea")] {
            crate::queries::identity::testing::insert_user(&mut conn, user, name)
                .await
                .unwrap();
        }
        crate::queries::hierarchy::testing::insert_node(&mut conn, node, "org", None, "Acme", ada)
            .await
            .unwrap();
        let start = Utc::now();
        let mut mine = Vec::new();
        for minute in 0..5 {
            let id = new_id();
            insert(
                &mut conn,
                id,
                ada,
                node,
                &serde_json::json!({}),
                start + chrono::Duration::minutes(minute),
            )
            .await
            .unwrap();
            mine.push(id);
        }
        insert(
            &mut conn,
            new_id(),
            bea,
            node,
            &serde_json::json!({}),
            start,
        )
        .await
        .unwrap();
        mine.reverse();
        let first = of_user(&mut conn, ada, None, 2).await.unwrap();
        assert_eq!(
            first.iter().map(|row| row.id).collect::<Vec<_>>(),
            mine[..2],
            "newest first"
        );
        let last = first.last().unwrap();
        let second = of_user(&mut conn, ada, Some((last.created_at, last.id)), 2)
            .await
            .unwrap();
        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<_>>(),
            mine[2..4]
        );
        let last = second.last().unwrap();
        let third = of_user(&mut conn, ada, Some((last.created_at, last.id)), 2)
            .await
            .unwrap();
        assert_eq!(
            third.iter().map(|row| row.id).collect::<Vec<_>>(),
            mine[4..]
        );
        assert_eq!(of_user(&mut conn, bea, None, 10).await.unwrap().len(), 1);
        assert_eq!(writing(&mut conn, ada).await.unwrap(), 5);
        assert_eq!(writing(&mut conn, bea).await.unwrap(), 1);
        // One person's archive is not another's to read by its identifier alone: the row says whose it is.
        assert_eq!(
            get(&mut conn, mine[0]).await.unwrap().unwrap().user_id,
            Some(ada)
        );
    }

    #[tokio::test]
    async fn an_archive_that_failed_keeps_what_was_left_out_and_is_not_made_ready_afterwards() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (user, node) = (new_id(), new_id());
        crate::queries::identity::testing::insert_user(&mut conn, user, "ada")
            .await
            .unwrap();
        crate::queries::hierarchy::testing::insert_node(&mut conn, node, "org", None, "Acme", user)
            .await
            .unwrap();
        let id = new_id();
        insert(
            &mut conn,
            id,
            user,
            node,
            &serde_json::json!({}),
            Utc::now(),
        )
        .await
        .unwrap();
        let left_out = serde_json::json!([{ "what": "not_exportable", "count": 2, "where": null }]);
        assert!(make_failed(&mut conn, id, &left_out).await.unwrap());
        assert!(
            !make_failed(&mut conn, id, &left_out).await.unwrap(),
            "failed once"
        );
        let counts = serde_json::json!({});
        let written = Written {
            blob: "ab",
            bytes: 1,
            counts: &counts,
            left_out: &left_out,
            at: Utc::now(),
            expires_at: Utc::now(),
        };
        assert!(
            !make_ready(&mut conn, id, &written).await.unwrap(),
            "a job that was stopped cannot make it ready afterwards"
        );
        let row = get(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.left_out),
            (state::FAILED, left_out)
        );
        assert_eq!(writing(&mut conn, user).await.unwrap(), 0);
    }
}
