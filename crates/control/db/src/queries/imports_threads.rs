//! What bringing a thread as history needs beyond the ordinary queries of threads: the rows of its one imported segment, its items and turns under identifiers an import derived, and the taking back of what an import added.
//!
//! The ordinary queries of threads write an item as an agent or a person makes it, at the moment of making, under the segment a
//! harness is running. An import writes history that was made elsewhere and long ago, so what is here adds to them: a range of the
//! thread's sequence taken at once (taking it locks the thread's row, as everywhere else), the one segment that stands for the
//! history (it ran on nothing here, so its agent, provider and environment are NULL and its report says what it ran on
//! there), the items of a page in one statement with their original times and the mark that they come from before Orch, and the
//! rows of `import_origins` written together for what was added to a thread that was there.

use crate::queries::imports::OriginRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json, Seq};
use std::collections::HashSet;

/// How many hashes one lookup of the store asks about: a page, so that a thread with very many files is asked about in
/// statements of a size the server parses without strain.
const BLOBS_PER_LOOKUP: usize = 1000;
/// How many identifiers one lookup of a thread's items asks about, for the same reason.
const ITEMS_PER_LOOKUP: usize = 5000;

/// What a thread is to somebody who means to put history into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// Whether its node was deleted.
    pub deleted: bool,
    /// Whether it is an aside of another thread, which takes no history of its own.
    pub side: bool,
}

/// The thread `thread` as somebody who puts history into it finds it, with its row locked until the end of the transaction so that
/// two imports into it go one after the other; none where there is no such thread.
pub async fn held(conn: &mut Conn, thread: Id) -> Result<Option<Held>, DbError> {
    let row: Option<(bool, bool)> = sqlx::query_as(
        "SELECT n.deleted_at IS NOT NULL, t.side_of IS NOT NULL
           FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.node_id = $1
            FOR NO KEY UPDATE OF t",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(deleted, side)| Held { deleted, side }))
}

/// Takes `count` numbers of the sequence of `thread` at once and answers the first of them; none where there is no such thread.
/// The thread's row is locked until the end of the transaction, which taking the numbers does as well.
pub async fn take_seqs(conn: &mut Conn, thread: Id, count: i64) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE threads SET next_seq = next_seq + $2 WHERE node_id = $1 RETURNING next_seq - $2",
    )
    .bind(thread)
    .bind(count)
    .fetch_optional(conn)
    .await?)
}

/// What a node says of itself that an import carries over: its description, which then is the person's and not automation's to
/// keep, and the moment it last changed there.
pub async fn describe_node(
    conn: &mut Conn,
    node: Id,
    description: Option<&str>,
    updated_at: Option<DateTime<Utc>>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE nodes
            SET description = COALESCE($2::text, description),
                description_source = CASE WHEN $2::text IS NULL THEN description_source ELSE 'user' END,
                updated_at = COALESCE($3::timestamptz, updated_at)
          WHERE id = $1",
    )
    .bind(node)
    .bind(description)
    .bind(updated_at)
    .execute(conn)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- the segment

/// What a segment that stands for imported history is made of.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSegment<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread: Id,
    /// Who imported it.
    pub started_by: Id,
    /// When what it holds began.
    pub started_at: DateTime<Utc>,
    /// When what it holds ended.
    pub ended_at: DateTime<Utc>,
    /// The arrangement type it ran under there, in the word the column has.
    pub arrangement: &'a str,
    /// The credential path it had there, in the word the column has.
    pub credential_path: &'a str,
    /// What was kept, converted and dropped, and what it ran on there.
    pub report: &'a Json,
    /// The number it is stamped with.
    pub seq: Seq,
}

/// Writes ended imported history with no local agent, provider or environment, and fidelity `structured`. The answer says whether this
/// call wrote it; it is there already where another did.
pub async fn put_segment(conn: &mut Conn, new: &NewSegment<'_>) -> Result<bool, DbError> {
    let written: Option<Id> = sqlx::query_scalar(
        "INSERT INTO segments (id, thread_id, ordinal, agent_id, provider_id, environment_id, env_generation,
                               arrangement, credential_path, started_by, started_at, ended_at,
                               import_fidelity, import_report, seq_updated)
         SELECT $1::uuid, $2::uuid, (SELECT COALESCE(MAX(ordinal), 0) + 1 FROM segments WHERE thread_id = $2::uuid),
                NULL, NULL, NULL, 0, $3::text, $4::text, $5::uuid, $6::timestamptz, $7::timestamptz,
                'structured', $8::jsonb, $9::bigint
         ON CONFLICT (id) DO NOTHING
         RETURNING id",
    )
    .bind(new.id)
    .bind(new.thread)
    .bind(new.arrangement)
    .bind(new.credential_path)
    .bind(new.started_by)
    .bind(new.started_at)
    .bind(new.ended_at)
    .bind(new.report)
    .bind(new.seq)
    .fetch_optional(conn)
    .await?;
    Ok(written.is_some())
}

/// A segment of imported history that is there.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// What it says of what it holds.
    pub report: Option<Json>,
}

/// The segment `segment` of `thread`, where it is there.
pub async fn placed_segment(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
) -> Result<Option<Placed>, DbError> {
    let row: Option<(Option<Json>,)> =
        sqlx::query_as("SELECT import_report FROM segments WHERE id = $1 AND thread_id = $2")
            .bind(segment)
            .bind(thread)
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(report,)| Placed { report }))
}

/// Replaces what a segment of imported history says of what it holds, stamping it `seq`.
pub async fn set_segment_report(
    conn: &mut Conn,
    segment: Id,
    report: &Json,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE segments SET import_report = $2, seq_updated = $3 WHERE id = $1")
        .bind(segment)
        .bind(report)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- the items

/// Which of the items `ids` are in `thread` already.
pub async fn present_items(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
) -> Result<HashSet<Id>, DbError> {
    let mut found = HashSet::new();
    for page in ids.chunks(ITEMS_PER_LOOKUP) {
        let held: Vec<Id> =
            sqlx::query_scalar("SELECT id FROM items WHERE thread_id = $1 AND id = ANY($2)")
                .bind(thread)
                .bind(page)
                .fetch_all(&mut *conn)
                .await?;
        found.extend(held);
    }
    Ok(found)
}

/// An item to write as history from before Orch.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedItem<'a> {
    /// Its identifier here.
    pub id: Id,
    /// The item it is made under, which is written before it.
    pub parent: Option<Id>,
    /// The line of the conversation, `main` or `sub:` and the sub-agent.
    pub stream: &'a str,
    /// The kind, in the word the column has.
    pub kind: &'a str,
    /// Its status, in the word the column has.
    pub status: &'a str,
    /// Who it is by, where that is a person with an account here.
    pub author: Option<Id>,
    /// What it holds.
    pub payload: &'a Json,
    /// The name the import gives it in its segment, which no other item of the segment has.
    pub native_ref: &'a str,
    /// Its projection for search.
    pub search_text: Option<&'a str>,
    /// The number it is stamped with.
    pub seq: Seq,
    /// When it was made there.
    pub created_at: DateTime<Utc>,
    /// When it last changed there.
    pub updated_at: DateTime<Utc>,
}

/// Writes items of `segment` in `thread` as history from before Orch: in no turn, read-only, never sent to an agent again. Those
/// that are there already are left as they are. The answer names the items this call wrote.
pub async fn put_items(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
    items: &[ImportedItem<'_>],
) -> Result<Vec<Id>, DbError> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Id> = items.iter().map(|item| item.id).collect();
    let parents: Vec<Option<Id>> = items.iter().map(|item| item.parent).collect();
    let streams: Vec<&str> = items.iter().map(|item| item.stream).collect();
    let kinds: Vec<&str> = items.iter().map(|item| item.kind).collect();
    let statuses: Vec<&str> = items.iter().map(|item| item.status).collect();
    let authors: Vec<Option<Id>> = items.iter().map(|item| item.author).collect();
    let payloads: Vec<&Json> = items.iter().map(|item| item.payload).collect();
    let refs: Vec<&str> = items.iter().map(|item| item.native_ref).collect();
    let texts: Vec<Option<&str>> = items.iter().map(|item| item.search_text).collect();
    let seqs: Vec<Seq> = items.iter().map(|item| item.seq).collect();
    let created: Vec<DateTime<Utc>> = items.iter().map(|item| item.created_at).collect();
    let updated: Vec<DateTime<Utc>> = items.iter().map(|item| item.updated_at).collect();
    Ok(sqlx::query_scalar(
        "INSERT INTO items (id, thread_id, segment_id, turn_id, stream, parent_item_id, kind, status, author_id,
                            payload, native_ref, before_orch, search_text, seq_created, seq_updated, created_at, updated_at)
         SELECT x.id, $1::uuid, $2::uuid, NULL::uuid, x.stream, x.parent, x.kind, x.status, x.author,
                x.payload, x.native_ref, true, x.search_text, x.seq, x.seq, x.created_at, x.updated_at
           FROM UNNEST($3::uuid[], $4::uuid[], $5::text[], $6::text[], $7::text[], $8::uuid[], $9::jsonb[],
                       $10::text[], $11::text[], $12::bigint[], $13::timestamptz[], $14::timestamptz[])
                AS x(id, parent, stream, kind, status, author, payload, native_ref, search_text, seq, created_at, updated_at)
         ON CONFLICT DO NOTHING
         RETURNING id",
    )
    .bind(thread)
    .bind(segment)
    .bind(&ids)
    .bind(&parents)
    .bind(&streams)
    .bind(&kinds)
    .bind(&statuses)
    .bind(&authors)
    .bind(&payloads)
    .bind(&refs)
    .bind(&texts)
    .bind(&seqs)
    .bind(&created)
    .bind(&updated)
    .fetch_all(conn)
    .await?)
}

/// Which of the files `hashes` the store holds, whoever named them.
pub async fn present_blobs(conn: &mut Conn, hashes: &[Hash]) -> Result<HashSet<Hash>, DbError> {
    let mut found = HashSet::with_capacity(hashes.len());
    for page in hashes.chunks(BLOBS_PER_LOOKUP) {
        let raw: Vec<Vec<u8>> = page.iter().map(|hash| hash.as_bytes().to_vec()).collect();
        let held: Vec<Vec<u8>> = sqlx::query_scalar("SELECT hash FROM blobs WHERE hash = ANY($1)")
            .bind(&raw)
            .fetch_all(&mut *conn)
            .await?;
        found.extend(held.iter().filter_map(|bytes| Hash::from_slice(bytes).ok()));
    }
    Ok(found)
}

/// Which of the files `hashes` are named by items of `thread` that are there: the thread holds them, and may name them again.
pub async fn held_blobs(
    conn: &mut Conn,
    thread: Id,
    hashes: &[Hash],
) -> Result<HashSet<Hash>, DbError> {
    let mut found = HashSet::new();
    for page in hashes.chunks(BLOBS_PER_LOOKUP) {
        let raw: Vec<Vec<u8>> = page.iter().map(|hash| hash.as_bytes().to_vec()).collect();
        let held: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT DISTINCT r.blob FROM thread_item_blob_refs r JOIN items i ON i.id = r.item_id
              WHERE i.thread_id = $1 AND r.blob = ANY($2)",
        )
        .bind(thread)
        .bind(&raw)
        .fetch_all(&mut *conn)
        .await?;
        found.extend(held.iter().filter_map(|bytes| Hash::from_slice(bytes).ok()));
    }
    Ok(found)
}

// ---------------------------------------------------------------- what an import added, and its taking back

/// Writes what many items of a source are here, in one statement. A row that is there is written over whole: the item that an
/// earlier import added and an undo took back has another identifier here now.
pub async fn put_item_origins(conn: &mut Conn, rows: &[OriginRow]) -> Result<(), DbError> {
    if rows.is_empty() {
        return Ok(());
    }
    let scopes: Vec<Id> = rows.iter().map(|row| row.scope_id).collect();
    let sources: Vec<Id> = rows.iter().map(|row| row.source_id).collect();
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind.as_str()).collect();
    let objects: Vec<Id> = rows.iter().map(|row| row.object_id).collect();
    let parents: Vec<Option<Id>> = rows.iter().map(|row| row.parent_id).collect();
    let makers: Vec<Option<Id>> = rows.iter().map(|row| row.created_by_import).collect();
    let lasts: Vec<Id> = rows.iter().map(|row| row.last_import).collect();
    let changed: Vec<Option<DateTime<Utc>>> =
        rows.iter().map(|row| row.source_updated_at).collect();
    let creators: Vec<Option<&Json>> = rows.iter().map(|row| row.source_creator.as_ref()).collect();
    let by: Vec<Option<Id>> = rows.iter().map(|row| row.imported_by).collect();
    let at: Vec<DateTime<Utc>> = rows.iter().map(|row| row.imported_at).collect();
    sqlx::query(
        "INSERT INTO import_origins (scope_id, source_id, kind, object_id, parent_id, created_by_import, last_import,
                source_updated_at, source_creator, imported_by, imported_at)
         SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::text[], $4::uuid[], $5::uuid[], $6::uuid[], $7::uuid[],
                $8::timestamptz[], $9::jsonb[], $10::uuid[], $11::timestamptz[])
         ON CONFLICT (scope_id, source_id) DO UPDATE SET
            kind = EXCLUDED.kind,
            object_id = EXCLUDED.object_id,
            parent_id = EXCLUDED.parent_id,
            created_by_import = EXCLUDED.created_by_import,
            last_import = EXCLUDED.last_import,
            source_name = NULL,
            source_path = NULL,
            source_sha256 = NULL,
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
    .bind(&changed)
    .bind(&creators)
    .bind(&by)
    .bind(&at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Takes back at most `limit` of the items the import `import` added to the thread `thread`, and forgets that it did: how many it looked
/// at, which is none when there are no more. An item that something else of the thread is made under (a sub-agent's work under its call)
/// waits for what hangs on it, so that no batch leaves an item without the one it was made under; what hung on one (the rows of a chat that
/// quoted it) goes with it.
pub async fn take_back_items(
    conn: &mut Conn,
    import: Id,
    thread: Id,
    limit: i64,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "WITH mine AS (
             SELECT o.scope_id, o.source_id, o.object_id FROM import_origins o
              WHERE o.created_by_import = $1 AND o.kind = 'item' AND o.parent_id = $2
                AND NOT EXISTS (SELECT 1 FROM items c WHERE c.parent_item_id = o.object_id)
              ORDER BY o.object_id LIMIT $3),
         gone AS (
             DELETE FROM items i USING mine m WHERE i.id = m.object_id AND i.thread_id = $2 RETURNING i.id),
         forgotten AS (
             DELETE FROM import_origins o USING mine m WHERE o.scope_id = m.scope_id AND o.source_id = m.source_id RETURNING 1)
         SELECT count(*) FROM mine",
    )
    .bind(import)
    .bind(thread)
    .bind(limit)
    .fetch_one(conn)
    .await?)
}

/// Moves the end of the segment `segment` of imported history to `at`, where that is later than the end it has.
pub async fn extend_segment(
    conn: &mut Conn,
    segment: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE segments SET ended_at = GREATEST(ended_at, $2) WHERE id = $1")
        .bind(segment)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Takes back the segments of imported history that the import `import` wrote and that hold nothing now: how many. A segment that
/// still holds items (those of a thread the import made, which goes and comes back as a whole) is left to its thread.
pub async fn take_back_segments(conn: &mut Conn, import: Id) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM segments s
          WHERE s.import_fidelity IS NOT NULL
            AND s.import_report #>> '{imported,import}' = $1::text
            AND NOT EXISTS (SELECT 1 FROM items i WHERE i.segment_id = s.id)
            AND NOT EXISTS (SELECT 1 FROM turns t WHERE t.segment_id = s.id)",
    )
    .bind(import.to_string())
    .execute(conn)
    .await?
    .rows_affected())
}
