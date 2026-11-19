//! Documents, their blocks, accepted batches of changes, versions with their cause and what each changed, discussions and comments, files and per-person states.
//!
//! A document is rows with a sequence, as a thread is: its blocks, its
//! discussions and its comments are stamped from its own counter, `doc_seq`,
//! under the lock on its row, so that sequence order is commit order, and the
//! writer announces the last number on the `orch_doc` channel when the
//! transaction commits. "Every row stamped after S" then describes what
//! changed, which is how every open copy of a document stays the same
//! document.
//!
//! This module holds the rows of a document and of its blocks and what is done
//! to the document as a thing; [`batch`] applies a batch of changes and puts a
//! version back, [`history`] keeps and cuts the versions, [`discussions`]
//! the comments beside the text. The rules about what a batch may do, when a
//! version is cut and what a block may be live in `orch-documents`; here they
//! are carried out on rows, in the caller's transaction.

pub mod batch;
pub mod containers;
pub mod discussions;
pub mod history;
pub mod people;

use chrono::{DateTime, Utc};
pub mod copy;
use orch_documents::versions::{Span, Writer};
use orch_types::{Id, Json, Seq};
use std::collections::HashMap;

use crate::{Conn, DbError};

/// The channel on which a document's changes are announced; its payload is
/// `<document id>:<sequence>`. A person writing is told on it in another form,
/// `t:<document id>:<person>:<block or ->`, which is never stored.
pub const DOC_CHANNEL: &str = "orch_doc";

/// The columns of a document, as [`DocumentRow`] reads them.
const DOCUMENT_COLUMNS: &str = "d.id, d.node_id, d.org_id, d.owner_id, d.created_by, d.created_by_thread, d.created_by_agent,
    d.title, d.description, d.description_source, d.doc_seq, d.seq_updated, d.purged_seq, d.block_count, d.bytes, d.files_bytes, d.version, d.version_at,
    d.span, d.told, d.created_at, d.updated_at, d.updated_by, d.deleted_at, d.deleted_by";

/// A document, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DocumentRow {
    /// Its identifier.
    pub id: Id,
    /// The node it is written beside.
    pub node_id: Id,
    /// That node's root org.
    pub org_id: Id,
    /// Its first owner.
    pub owner_id: Id,
    /// The original human maker; ownership changes leave it alone.
    pub created_by: Option<Id>,
    /// The thread of the agent that made it, where one did.
    pub created_by_thread: Option<Id>,
    /// That agent's name then.
    pub created_by_agent: Option<String>,
    /// Its title.
    pub title: String,
    /// Its description, whose first line is shown where documents are listed; empty when nobody wrote one.
    pub description: String,
    /// Who maintains the description: `user` once a person or an agent wrote it, `auto` while automation may.
    pub description_source: String,
    /// Its sequence.
    pub doc_seq: i64,
    /// The stamp of this row.
    pub seq_updated: i64,
    /// Removed blocks stamped at or below this are gone for good.
    pub purged_seq: i64,
    /// Its live blocks.
    pub block_count: i32,
    /// The size of their props and content.
    pub bytes: i64,
    /// The size of the files it holds.
    pub files_bytes: i64,
    /// How many versions were cut so far.
    pub version: i32,
    /// When the last was cut.
    pub version_at: Option<DateTime<Utc>>,
    /// What was changed since, and by whom.
    pub span: Json,
    /// People who were told that they are named here.
    pub told: Vec<Id>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
    /// Who changed it last; none when an agent did.
    pub updated_by: Option<Id>,
    /// When it was deleted.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Who deleted it.
    pub deleted_by: Option<Id>,
}

impl DocumentRow {
    /// What was changed since the last version.
    #[must_use]
    pub fn span(&self) -> Span {
        Span::from_json(&self.span)
    }
}

/// The columns of a block, as [`BlockRow`] reads them.
const BLOCK_COLUMNS: &str = "b.id, b.document_id, b.parent_id, b.position::text AS position, b.type AS block_type,
    b.props, b.content, b.text, b.seq_created, b.seq_updated, b.created_by, b.updated_by, b.updated_by_thread,
    b.updated_by_agent, b.created_at, b.updated_at, b.removed_at";

/// A block, as stored; also as a version keeps it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct BlockRow {
    /// Its identifier.
    pub id: Id,
    /// The document.
    pub document_id: Id,
    /// The block it is under.
    pub parent_id: Option<Id>,
    /// Its place among its siblings, as a decimal.
    pub position: String,
    /// Its type.
    pub block_type: String,
    /// Its props.
    pub props: Json,
    /// Its content.
    pub content: Json,
    /// Its text, for search.
    pub text: String,
    /// The sequence at which it was made.
    pub seq_created: i64,
    /// The sequence at which it last changed.
    pub seq_updated: i64,
    /// Who made it.
    pub created_by: Option<Id>,
    /// The person who changed it last.
    pub updated_by: Option<Id>,
    /// The agent's thread that changed it last.
    pub updated_by_thread: Option<Id>,
    /// That agent's name then.
    pub updated_by_agent: Option<String>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
    /// When it was removed.
    pub removed_at: Option<DateTime<Utc>>,
}

impl BlockRow {
    /// Who changed the block last.
    #[must_use]
    pub fn last_writer(&self) -> Option<Writer> {
        match (
            self.updated_by,
            self.updated_by_thread,
            &self.updated_by_agent,
        ) {
            (Some(user), _, _) => Some(Writer::User(user)),
            (None, Some(thread), Some(agent)) => Some(Writer::Agent {
                thread,
                agent: agent.clone(),
            }),
            _ => None,
        }
    }
}

/// The columns of a person or an agent, to stamp a row with.
#[must_use]
pub fn stamped_by(writer: &Writer) -> (Option<Id>, Option<Id>, Option<&str>) {
    match writer {
        Writer::User(user) => (Some(*user), None, None),
        Writer::Agent { thread, agent } => (None, Some(*thread), Some(agent.as_str())),
    }
}

/// A new document.
#[derive(Debug, Clone, Copy)]
pub struct NewDocument<'a> {
    /// Its identifier.
    pub id: Id,
    /// The node it is written beside.
    pub node_id: Id,
    /// That node's root org.
    pub org_id: Id,
    /// Its first owner.
    pub owner_id: Id,
    /// The thread of the agent that makes it, where one does.
    pub created_by_thread: Option<Id>,
    /// That agent's name.
    pub created_by_agent: Option<&'a str>,
    /// Its title.
    pub title: &'a str,
    /// The moment it is made.
    pub now: DateTime<Utc>,
}

/// Makes a document with no blocks.
pub async fn insert(conn: &mut Conn, new: &NewDocument<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO documents (id, node_id, org_id, owner_id, created_by_thread, created_by_agent, title,
                                created_at, updated_at, updated_by, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9, $9)",
    )
    .bind(new.id)
    .bind(new.node_id)
    .bind(new.org_id)
    .bind(new.owner_id)
    .bind(new.created_by_thread)
    .bind(new.created_by_agent)
    .bind(new.title)
    .bind(new.now)
    .bind(new.created_by_thread.is_none().then_some(new.owner_id))
    .execute(&mut *conn)
    .await?;
    crate::queries::hierarchy::stamp(conn, new.node_id).await?;
    crate::queries::hierarchy::touch_activity(conn, new.node_id, new.now).await?;
    Ok(())
}

/// The document `id`, deleted or not.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<DocumentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DOCUMENT_COLUMNS} FROM documents d WHERE d.id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The documents `ids`, deleted or not.
pub async fn get_many(conn: &mut Conn, ids: &[Id]) -> Result<Vec<DocumentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DOCUMENT_COLUMNS} FROM documents d WHERE d.id = ANY($1)"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The document `id` with its row locked until the transaction ends, so
/// that the numbers of its sequence are given out in the order the
/// transactions commit. Deleted documents are returned as they are.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<DocumentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DOCUMENT_COLUMNS} FROM documents d WHERE d.id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Where a list of documents resumes: the moment of the last change of the last
/// one shown, and its identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    /// The moment.
    pub at: DateTime<Utc>,
    /// The identifier.
    pub id: Id,
}

/// What a list of the documents beside a node is asked for.
#[derive(Debug, Clone, Copy)]
pub struct ListWhere {
    /// The node.
    pub node: Id,
    /// Also the documents of the nodes below it.
    pub under: bool,
    /// Only those deleted and not yet purged; otherwise only those that are not.
    pub deleted: bool,
    /// Rows to resume after.
    pub after: Option<ListCursor>,
    /// A bounded scan continuation after that visible cursor, without exposing a skipped row's activity.
    pub offset: i64,
    /// How many at most.
    pub limit: i64,
}

/// The documents beside a node, the most recently changed first. A document
/// whose node, or a node above it, is deleted is not here: it goes and comes
/// back with its node.
pub async fn list_beside(conn: &mut Conn, which: &ListWhere) -> Result<Vec<DocumentRow>, DbError> {
    // Keep a direct place read indexable even after PostgreSQL selects a generic plan.
    // The subtree form retains its native path reach and the same bound/cursor.
    let reach = if which.under {
        "$3 AND $1 = ANY(n.path)"
    } else {
        "NOT $3 AND d.node_id = $1"
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DOCUMENT_COLUMNS}
           FROM documents d
           JOIN nodes n ON n.id = d.node_id
          WHERE ({reach})
            AND (d.deleted_at IS NOT NULL) = $2
            AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
            AND ($4::uuid IS NULL OR (d.updated_at, d.id) < ($5, $4))
          ORDER BY d.updated_at DESC, d.id DESC
          LIMIT $6 OFFSET $7"
    )))
    .bind(which.node)
    .bind(which.deleted)
    .bind(which.under)
    .bind(which.after.map(|cursor| cursor.id))
    .bind(which.after.map(|cursor| cursor.at))
    .bind(which.limit)
    .bind(which.offset)
    .fetch_all(conn)
    .await?)
}

/// A document's administrative identity, without activity or content facts.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DocumentNameRow {
    /// Its identifier.
    pub id: Id,
    /// Its structural parent.
    pub node_id: Id,
    /// Its first owner.
    pub owner_id: Id,
    /// Its title.
    pub title: String,
}

/// Candidates for the private group, ordered independently of document activity.
pub async fn names_beside(
    conn: &mut Conn,
    node: Id,
    under: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<DocumentNameRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT d.id, d.node_id, d.owner_id, d.title
           FROM documents d JOIN nodes n ON n.id = d.node_id
          WHERE ($2 AND $1 = ANY(n.path) OR NOT $2 AND d.node_id = $1)
            AND d.deleted_at IS NULL
            AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
          ORDER BY d.title, d.id OFFSET $3 LIMIT $4",
    )
    .bind(node)
    .bind(under)
    .bind(offset)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// How many people each document's Owners row names now, groups opened.
pub async fn owner_counts(
    conn: &mut Conn,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<HashMap<Id, i64>, DbError> {
    let rows: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT d.id,
                (SELECT count(DISTINCT x.person) FROM (
                    SELECT e.holder_id AS person FROM permission_entries e
                     WHERE e.object_kind = 'document' AND e.object_id = d.id AND e.page = 'live'
                       AND e.permission = 'access.own' AND e.removed_at IS NULL
                       AND (e.expires_at IS NULL OR e.expires_at > $2) AND e.holder_kind = 'user'
                    UNION ALL
                    SELECT gm.user_id FROM permission_entries e
                      JOIN group_members gm ON gm.group_id = e.holder_id
                     WHERE e.object_kind = 'document' AND e.object_id = d.id AND e.page = 'live'
                       AND e.permission = 'access.own' AND e.removed_at IS NULL
                       AND (e.expires_at IS NULL OR e.expires_at > $2) AND e.holder_kind = 'group') x)
           FROM documents d WHERE d.id = ANY($1)",
    )
    .bind(ids)
    .bind(now)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().collect())
}

/// How many discussions beside each document nobody resolved.
pub async fn open_discussion_counts(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<HashMap<Id, i64>, DbError> {
    let rows: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT document_id, count(*) FROM document_discussions
          WHERE document_id = ANY($1) AND resolved_at IS NULL GROUP BY document_id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The sequence each of `ids` was last had on screen up to by `user`, for those
/// they ever had.
pub async fn seen_seqs(
    conn: &mut Conn,
    user: Id,
    ids: &[Id],
) -> Result<HashMap<Id, (i64, DateTime<Utc>)>, DbError> {
    let rows: Vec<(Id, i64, DateTime<Utc>)> = sqlx::query_as(
        "SELECT document_id, seen_seq, updated_at FROM document_states
          WHERE user_id = $1 AND document_id = ANY($2)",
    )
    .bind(user)
    .bind(ids)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, seq, at)| (id, (seq, at)))
        .collect())
}

/// Says that `user` has had the document on screen up to `seq`. Never moves
/// back: the largest number said stands.
pub async fn mark_seen(
    conn: &mut Conn,
    user: Id,
    document: Id,
    seq: Seq,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO document_states (user_id, document_id, seen_seq, updated_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, document_id) DO UPDATE
            SET seen_seq = GREATEST(document_states.seen_seq, EXCLUDED.seen_seq),
                updated_at = CASE WHEN EXCLUDED.seen_seq > document_states.seen_seq
                                  THEN EXCLUDED.updated_at ELSE document_states.updated_at END",
    )
    .bind(user)
    .bind(document)
    .bind(seq)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// A file a document holds, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct FileRow {
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Its type.
    pub media_type: String,
    /// Its size.
    pub size: i64,
    /// Whether its bytes are text.
    pub is_text: bool,
    /// The person who put it there.
    pub added_by: Option<Id>,
    /// The agent's thread that did.
    pub added_by_thread: Option<Id>,
    /// When.
    pub added_at: DateTime<Utc>,
}

/// The files a document holds, by name.
pub async fn files(conn: &mut Conn, document: Id) -> Result<Vec<FileRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, name, media_type, size, is_text, added_by, added_by_thread, added_at
           FROM document_files WHERE document_id = $1 ORDER BY name",
    )
    .bind(document)
    .fetch_all(conn)
    .await?)
}

/// Which of `ids` are files of the document.
pub async fn files_among(conn: &mut Conn, document: Id, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM document_files WHERE document_id = $1 AND id = ANY($2)")
            .bind(document)
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Which of the files `ids` of the document a live block names.
pub async fn files_in_use(conn: &mut Conn, document: Id, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    let named: Vec<String> = ids.iter().map(ToString::to_string).collect();
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT (b.props->>'file_id')::uuid FROM document_blocks b
          WHERE b.document_id = $1 AND b.removed_at IS NULL AND b.props->>'file_id' = ANY($2)",
    )
    .bind(document)
    .bind(&named)
    .fetch_all(conn)
    .await?)
}

/// Takes a file out of the document and counts again the size of those it holds.
/// False where the document has no such file.
pub async fn remove_file(conn: &mut Conn, document: Id, file: Id) -> Result<bool, DbError> {
    let gone = sqlx::query("DELETE FROM document_files WHERE document_id = $1 AND id = $2")
        .bind(document)
        .bind(file)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if gone == 0 {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE documents
            SET files_bytes = COALESCE((SELECT sum(size) FROM document_files WHERE document_id = $1), 0)::bigint
          WHERE id = $1",
    )
    .bind(document)
    .execute(conn)
    .await?;
    Ok(true)
}

// ------------------------------------------------------------------ the bytes of files (put in and served)

/// A file of a document together with the blob of the store that has its bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldFile {
    /// The file as it is listed.
    pub file: FileRow,
    /// The blob that holds its bytes.
    pub blob: orch_types::Hash,
}

#[derive(sqlx::FromRow)]
struct RawHeld {
    #[sqlx(flatten)]
    file: FileRow,
    blob: Vec<u8>,
}

impl TryFrom<RawHeld> for HeldFile {
    type Error = DbError;

    fn try_from(raw: RawHeld) -> Result<Self, DbError> {
        Ok(Self {
            file: raw.file,
            blob: orch_types::Hash::from_slice(&raw.blob).map_err(|_| {
                DbError::Other(sqlx::Error::Decode("a hash that is not 32 bytes".into()))
            })?,
        })
    }
}

const HELD_COLUMNS: &str =
    "id, name, media_type, size, is_text, added_by, added_by_thread, added_at, blob";

/// The file `id` of the document, if the document holds it.
pub async fn file(conn: &mut Conn, document: Id, id: Id) -> Result<Option<HeldFile>, DbError> {
    let raw: Option<RawHeld> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {HELD_COLUMNS} FROM document_files WHERE document_id = $1 AND id = $2"
    )))
    .bind(document)
    .bind(id)
    .fetch_optional(conn)
    .await?;
    raw.map(HeldFile::try_from).transpose()
}

/// The file of the document that has this name, if it holds one.
pub async fn file_named(
    conn: &mut Conn,
    document: Id,
    name: &str,
) -> Result<Option<HeldFile>, DbError> {
    let raw: Option<RawHeld> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {HELD_COLUMNS} FROM document_files WHERE document_id = $1 AND name = $2"
    )))
    .bind(document)
    .bind(name)
    .fetch_optional(conn)
    .await?;
    raw.map(HeldFile::try_from).transpose()
}

/// A file about to be held by a document.
#[derive(Debug, Clone)]
pub struct NewFile<'a> {
    /// Its identifier: what an `image` or a `file` block names. A file that replaces another of
    /// the same name keeps the identifier it had.
    pub id: Id,
    /// The document.
    pub document: Id,
    /// The blob that holds its bytes.
    pub blob: orch_types::Hash,
    /// Its name, unique among the files of the document.
    pub name: &'a str,
    /// Its type.
    pub media_type: &'a str,
    /// Its size in bytes.
    pub size: i64,
    /// Whether its bytes are text.
    pub is_text: bool,
    /// The person who put it there.
    pub added_by: Option<Id>,
    /// The agent's thread that did.
    pub added_by_thread: Option<Id>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Counts again the size of the files the document holds.
async fn recount_files(conn: &mut Conn, document: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE documents
            SET files_bytes = COALESCE((SELECT sum(size) FROM document_files WHERE document_id = $1), 0)::bigint
          WHERE id = $1",
    )
    .bind(document)
    .execute(conn)
    .await?;
    Ok(())
}

/// Makes the document hold a file, and counts again the size of those it holds. The name is the
/// caller's to have checked: a second file of the same name is a fault of the database.
pub async fn add_file(conn: &mut Conn, new: &NewFile<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO document_files
             (id, document_id, blob, name, media_type, size, is_text, added_by, added_by_thread, added_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(new.id)
    .bind(new.document)
    .bind(new.blob.as_bytes().as_slice())
    .bind(new.name)
    .bind(new.media_type)
    .bind(new.size)
    .bind(new.is_text)
    .bind(new.added_by)
    .bind(new.added_by_thread)
    .bind(new.at)
    .execute(&mut *conn)
    .await?;
    recount_files(conn, new.document).await
}

/// Puts other bytes in the place of those of a file, which keeps its identifier and its name, and
/// counts again the size of those the document holds. False where the document has no such file.
pub async fn replace_file(conn: &mut Conn, new: &NewFile<'_>) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE document_files
            SET blob = $3, media_type = $4, size = $5, is_text = $6,
                added_by = $7, added_by_thread = $8, added_at = $9
          WHERE document_id = $1 AND id = $2",
    )
    .bind(new.document)
    .bind(new.id)
    .bind(new.blob.as_bytes().as_slice())
    .bind(new.media_type)
    .bind(new.size)
    .bind(new.is_text)
    .bind(new.added_by)
    .bind(new.added_by_thread)
    .bind(new.at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if done == 0 {
        return Ok(false);
    }
    recount_files(conn, new.document).await?;
    Ok(true)
}

// ------------------------------------------------------------------ reading blocks

/// Every live block of a document, in the order of the page's siblings.
pub async fn live_blocks(conn: &mut Conn, document: Id) -> Result<Vec<BlockRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {BLOCK_COLUMNS} FROM document_blocks b
          WHERE b.document_id = $1 AND b.removed_at IS NULL ORDER BY b.position, b.id"
    )))
    .bind(document)
    .fetch_all(conn)
    .await?)
}

/// The blocks `ids` of a document, removed ones among them.
pub async fn blocks_by_ids(
    conn: &mut Conn,
    document: Id,
    ids: &[Id],
) -> Result<Vec<BlockRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {BLOCK_COLUMNS} FROM document_blocks b
          WHERE b.document_id = $1 AND b.id = ANY($2) ORDER BY b.position, b.id"
    )))
    .bind(document)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The blocks, removed ones among them, stamped above `since` and up to
/// `upto`, at most `limit` of them.
pub async fn blocks_changed(
    conn: &mut Conn,
    document: Id,
    since: Seq,
    upto: Seq,
    limit: i64,
) -> Result<Vec<BlockRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {BLOCK_COLUMNS} FROM document_blocks b
          WHERE b.document_id = $1 AND b.seq_updated > $2 AND b.seq_updated <= $3
          ORDER BY b.seq_updated, b.position, b.id LIMIT $4"
    )))
    .bind(document)
    .bind(since)
    .bind(upto)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// How many blocks, removed ones among them, are stamped above `since`.
pub async fn count_blocks_changed(
    conn: &mut Conn,
    document: Id,
    since: Seq,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM document_blocks WHERE document_id = $1 AND seq_updated > $2",
    )
    .bind(document)
    .bind(since)
    .fetch_one(conn)
    .await?)
}

/// Which of `ids` are the identifier of any block of any document: an
/// identifier in use stays in use for as long as the row does.
pub async fn ids_in_use(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM document_blocks WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// The sequences of the documents `ids`, for the sweep of the realtime server.
pub async fn sequences(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Seq)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, doc_seq FROM documents WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Those of the documents `ids` that are gone: deleted, or beside a node that
/// is or has a deleted node above it, or that never were.
pub async fn gone(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT x.id FROM UNNEST($1::uuid[]) AS x(id)
          WHERE NOT EXISTS (
                SELECT 1 FROM documents d JOIN nodes n ON n.id = d.node_id
                 WHERE d.id = x.id AND d.deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL))",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The agent that last changed each of the documents `ids`, for those that an
/// agent changed last: the thread and the name it had then.
pub async fn last_agents(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<HashMap<Id, (Id, String)>, DbError> {
    let rows: Vec<(Id, Id, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (b.document_id) b.document_id, b.updated_by_thread, b.updated_by_agent
           FROM document_blocks b
          WHERE b.document_id = ANY($1) AND b.updated_by IS NULL
            AND b.updated_by_thread IS NOT NULL AND b.updated_by_agent IS NOT NULL
          ORDER BY b.document_id, b.seq_updated DESC",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(document, thread, agent)| (document, (thread, agent)))
        .collect())
}

/// The people whose usernames are `names`, by username, for a text that
/// names them with `@`.
pub async fn users_by_username(
    conn: &mut Conn,
    names: &[String],
) -> Result<Vec<(String, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT username, id FROM users WHERE username = ANY($1) AND status = 'active'",
    )
    .bind(names)
    .fetch_all(conn)
    .await?)
}

// ------------------------------------------------------------------ stamping

/// Takes the next number of the document's sequence and stamps the document's
/// row with it. The row is locked until the transaction ends. None where there
/// is no such document, or it is deleted.
pub async fn next_seq(conn: &mut Conn, document: Id) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE documents SET doc_seq = doc_seq + 1, seq_updated = doc_seq + 1
          WHERE id = $1 AND deleted_at IS NULL RETURNING doc_seq",
    )
    .bind(document)
    .fetch_optional(conn)
    .await?)
}

/// Queues the announcement that `document` has changes up to `seq`. It is
/// delivered when the transaction commits, and not at all if it does not.
pub async fn notify(conn: &mut Conn, document: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(DOC_CHANNEL)
        .bind(format!("{document}:{seq}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// Announces `text` on the document channel at once, outside any transaction of
/// the document's: for what is told and not stored, as a person writing is.
pub async fn announce(conn: &mut Conn, text: &str) -> Result<(), DbError> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(DOC_CHANNEL)
        .bind(text)
        .execute(conn)
        .await?;
    Ok(())
}

/// Recounts a document's live blocks and the size of their props and content
/// and keeps both on its row. Returns them.
pub async fn recount(conn: &mut Conn, document: Id) -> Result<(i64, i64), DbError> {
    let counted: (i64, i64) = sqlx::query_as(
        "UPDATE documents d SET block_count = c.n::int, bytes = c.bytes
           FROM (SELECT count(*) AS n,
                        COALESCE(sum(octet_length(b.props::text) + octet_length(b.content::text)), 0)::bigint AS bytes
                   FROM document_blocks b WHERE b.document_id = $1 AND b.removed_at IS NULL) c
          WHERE d.id = $1 RETURNING d.block_count::bigint, d.bytes",
    )
    .bind(document)
    .fetch_one(conn)
    .await?;
    Ok(counted)
}

/// Keeps the tally of what was changed since the last version, as the
/// document row holds it.
pub async fn keep_span(conn: &mut Conn, document: Id, span: &Span) -> Result<(), DbError> {
    sqlx::query("UPDATE documents SET span = $2 WHERE id = $1")
        .bind(document)
        .bind(span.to_json())
        .execute(conn)
        .await?;
    Ok(())
}

/// Says that the people `named` were told that they are named in the document.
pub async fn record_told(conn: &mut Conn, document: Id, named: &[Id]) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE documents SET told = ARRAY(SELECT DISTINCT x FROM unnest(told || $2::uuid[]) AS x)
          WHERE id = $1",
    )
    .bind(document)
    .bind(named)
    .execute(conn)
    .await?;
    Ok(())
}

/// Writes a row to the Inbox of a person; when they have one with the same key
/// already, renews it instead: new words, the moment it is renewed, unread again
/// and at the end of their feed. One row for a discussion that is answered a
/// hundred times, always the last answer.
pub async fn tell(
    conn: &mut Conn,
    new: &crate::queries::notifications::people::NewNotification<'_>,
) -> Result<Option<crate::queries::notifications::people::Announced>, DbError> {
    if let Some(announced) = crate::queries::notifications::people::announce(conn, new).await? {
        return Ok(Some(announced));
    }
    let seq: Option<i64> = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1 WHERE id = $1 RETURNING inbox_seq",
    )
    .bind(new.user_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(seq) = seq else {
        return Ok(None);
    };
    let id: Option<Id> = sqlx::query_scalar(
        "UPDATE notifications
            SET title = $3, body = $4, data = $5, created_at = $6, read_at = NULL, inbox_seq = $7
          WHERE user_id = $1 AND dedupe_key = $2 RETURNING id",
    )
    .bind(new.user_id)
    .bind(new.dedupe_key)
    .bind(new.title)
    .bind(new.body)
    .bind(new.data)
    .bind(new.at)
    .bind(seq)
    .fetch_optional(&mut *conn)
    .await?;
    crate::sequences::notify_user(conn, new.user_id, seq).await?;
    Ok(id.map(|id| crate::queries::notifications::people::Announced { id, inbox_seq: seq }))
}

// ------------------------------------------------------------------ the document as a thing

/// Gives the document a new title, stamped as a change by `writer`. None where
/// the document is gone.
pub async fn rename(
    conn: &mut Conn,
    document: Id,
    title: &str,
    writer: &Writer,
    now: DateTime<Utc>,
) -> Result<Option<Seq>, DbError> {
    containers::lock_document_scope(conn, document).await?;
    let Some(seq) = next_seq(conn, document).await? else {
        return Ok(None);
    };
    let (user, _, _) = stamped_by(writer);
    sqlx::query("UPDATE documents SET title = $2, updated_at = $3, updated_by = $4 WHERE id = $1")
        .bind(document)
        .bind(title)
        .bind(now)
        .bind(user)
        .execute(&mut *conn)
        .await?;
    notify(conn, document, seq).await?;
    // The tree lists the documents beside a place by their titles: the place is stamped, so that every tree that
    // shows it reads them again.
    tree::stamp(conn, document).await?;
    Ok(Some(seq))
}

/// Gives the document a description written by `writer`, a person or an agent: from now on it is theirs to keep
/// (`user`), and automation leaves it alone. Stamped as a change, so that every open copy and every list that
/// shows its first line hears of it. None where the document is gone.
pub async fn describe(
    conn: &mut Conn,
    document: Id,
    description: &str,
    writer: &Writer,
    now: DateTime<Utc>,
) -> Result<Option<Seq>, DbError> {
    let Some(seq) = next_seq(conn, document).await? else {
        return Ok(None);
    };
    let (user, _, _) = stamped_by(writer);
    sqlx::query(
        "UPDATE documents SET description = $2, description_source = 'user', updated_at = $3, updated_by = $4
          WHERE id = $1",
    )
    .bind(document)
    .bind(description)
    .bind(now)
    .bind(user)
    .execute(&mut *conn)
    .await?;
    notify(conn, document, seq).await?;
    tree::stamp(conn, document).await?;
    Ok(Some(seq))
}

/// Gives a copy the description of the document it copies, kept by whoever kept that one; the copy is new, so
/// nothing is stamped or announced that its creation does not announce already.
pub async fn copy_description(conn: &mut Conn, from: Id, to: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE documents d SET description = s.description, description_source = s.description_source
           FROM documents s WHERE d.id = $2 AND s.id = $1",
    )
    .bind(from)
    .bind(to)
    .execute(conn)
    .await?;
    Ok(())
}

/// For each of `documents` that has one, the text of its first paragraph: the first live top-level paragraph block
/// whose text is not blank, in the order of the page, cut to `chars` characters. What a reader is given in place of
/// a description that nobody wrote; computed from the blocks as they stand, with no model. Saved reference labels
/// are the author's words in this paragraph; target names and identities are never looked up for this preview.
pub async fn first_paragraphs(
    conn: &mut Conn,
    documents: &[Id],
    chars: i32,
) -> Result<HashMap<Id, String>, DbError> {
    if documents.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Id, String)> = sqlx::query_as(
        r#"SELECT owner.id, left(opening.text, $2) AS text FROM unnest($1::uuid[]) AS owner(id)
         CROSS JOIN LATERAL (SELECT (
             SELECT coalesce(string_agg(CASE piece.value->>'type'
               WHEN 'text' THEN piece.value->>'text'
               WHEN 'math' THEN piece.value->>'source'
               WHEN 'ref' THEN piece.value->>'label'
               WHEN 'break' THEN chr(10)
               ELSE '' END, '' ORDER BY piece.ordinality), '')
             FROM jsonb_array_elements(b.content) WITH ORDINALITY AS piece(value, ordinality)
           ) AS text FROM document_blocks b
           WHERE b.document_id = owner.id AND b.parent_id IS NULL AND b.removed_at IS NULL
             AND b.type = 'paragraph' AND (btrim(b.text) <> '' OR jsonb_path_exists(
               b.content, '$[*] ? (@.type == "ref" && @.label.type() == "string" && @.label like_regex "[^ ]")'))
           ORDER BY b.position, b.id LIMIT 1) opening"#,
    )
    .bind(documents)
    .bind(chars)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The documents that the nodes `nodes` name as their READMEs, where each is still one of its node's own
/// documents and is not deleted: its `node_id` is the node that names it.
pub async fn readmes_of(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<DocumentRow>, DbError> {
    if nodes.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DOCUMENT_COLUMNS} FROM nodes n
           JOIN documents d ON d.id = n.readme_document_id AND d.node_id = n.id AND d.deleted_at IS NULL
          WHERE n.id = ANY($1)"
    )))
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// Names one of a node's own documents as its README, or none.
pub async fn set_readme(conn: &mut Conn, node: Id, document: Option<Id>) -> Result<(), DbError> {
    sqlx::query("UPDATE nodes SET readme_document_id = $2 WHERE id = $1")
        .bind(node)
        .bind(document)
        .execute(conn)
        .await?;
    Ok(())
}

/// Puts the document beside another node, in the tree of `org`. Stamped, so
/// that whoever has it open is told.
pub async fn move_to(
    conn: &mut Conn,
    document: Id,
    node: Id,
    org: Id,
    now: DateTime<Utc>,
) -> Result<Option<Seq>, DbError> {
    let from: Option<Id> = sqlx::query_scalar("SELECT node_id FROM documents WHERE id=$1")
        .bind(document)
        .fetch_optional(&mut *conn)
        .await?;
    let mut scopes = vec![node];
    scopes.extend(from);
    containers::lock_scopes(conn, &scopes).await?;
    let current: Option<Id> = sqlx::query_scalar("SELECT node_id FROM documents WHERE id=$1")
        .bind(document)
        .fetch_optional(&mut *conn)
        .await?;
    if current != from {
        return Err(DbError::Retry);
    }
    let before = tree::lock_node_of(conn, document).await?;
    let Some(seq) = next_seq(conn, document).await? else {
        return Ok(None);
    };
    sqlx::query("UPDATE documents SET node_id = $2, org_id = $3, parent_container_id=NULL, updated_at = $4 WHERE id = $1")
        .bind(document)
        .bind(node)
        .bind(org)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    // A README is one of its place's own documents: the place it left names it no more.
    sqlx::query(
        "UPDATE nodes SET readme_document_id = NULL WHERE readme_document_id = $1 AND id <> $2",
    )
    .bind(document)
    .bind(node)
    .execute(&mut *conn)
    .await?;
    notify(conn, document, seq).await?;
    if let Some(before) = before {
        crate::queries::hierarchy::stamp(conn, before).await?;
        crate::queries::hierarchy::touch_activity(conn, before, now).await?;
        if before != node {
            crate::queries::hierarchy::stamp(conn, node).await?;
            crate::queries::hierarchy::touch_activity(conn, node, now).await?;
        }
    }
    Ok(Some(seq))
}

/// Stamps the document without changing anything of it, for a change of its
/// page, which the document row reports (`follows`, `custom`, `restricted`).
pub async fn touch(conn: &mut Conn, document: Id) -> Result<Option<Seq>, DbError> {
    let Some(seq) = next_seq(conn, document).await? else {
        return Ok(None);
    };
    notify(conn, document, seq).await?;
    tree::stamp(conn, document).await?;
    Ok(Some(seq))
}

/// Sets the first owner of the document.
pub async fn set_owner(conn: &mut Conn, document: Id, owner: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE documents SET owner_id = $2 WHERE id = $1")
        .bind(document)
        .bind(owner)
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes the document, softly: it can be restored until it is purged.
/// False where it is gone already.
pub async fn soft_delete(
    conn: &mut Conn,
    document: Id,
    by: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    containers::lock_document_scope(conn, document).await?;
    let done = sqlx::query(
        "UPDATE documents SET deleted_at = $2, deleted_by = $3, deleted_in=NULL WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(document)
    .bind(now)
    .bind(by)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if done > 0 {
        tree::stamp(conn, document).await?;
    }
    Ok(done > 0)
}

/// Brings a deleted document back, stamped. False where it was not deleted.
pub async fn restore_deleted(conn: &mut Conn, document: Id) -> Result<Option<Seq>, DbError> {
    containers::lock_document_scope(conn, document).await?;
    let done = sqlx::query(
        "UPDATE documents SET deleted_at = NULL, deleted_by = NULL, deleted_in=NULL WHERE id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(document)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if done == 0 {
        return Ok(None);
    }
    touch(conn, document).await
}

// ------------------------------------------------------------------ keeping up

/// Removes the rows of blocks that were removed before `before`, and says in
/// each document up to which sequence that was done, so that a reader who would
/// continue from below it loads the document again. Returns how many blocks
/// went.
pub async fn purge_removed_blocks(conn: &mut Conn, before: DateTime<Utc>) -> Result<i64, DbError> {
    purge_removed_blocks_selected(conn, before, None).await
}

/// Purges removed blocks only in a page whose document rows the caller has already locked.
pub async fn purge_removed_blocks_among(
    conn: &mut Conn,
    before: DateTime<Utc>,
    documents: &[Id],
) -> Result<i64, DbError> {
    purge_removed_blocks_selected(conn, before, Some(documents)).await
}

async fn purge_removed_blocks_selected(
    conn: &mut Conn,
    before: DateTime<Utc>,
    documents: Option<&[Id]>,
) -> Result<i64, DbError> {
    // Separate the two shapes so a generic prepared plan can seek by document
    // and age without a nullable-parameter OR hiding the index condition.
    let statement = if documents.is_some() {
        "WITH gone AS (
            DELETE FROM document_blocks WHERE document_id=ANY($2)
              AND removed_at IS NOT NULL AND removed_at < $1
            RETURNING document_id, seq_updated)
         SELECT document_id, max(seq_updated) FROM gone GROUP BY document_id"
    } else {
        "WITH gone AS (
            DELETE FROM document_blocks WHERE removed_at IS NOT NULL AND removed_at < $1
            RETURNING document_id, seq_updated)
         SELECT document_id, max(seq_updated) FROM gone GROUP BY document_id"
    };
    let mut query = sqlx::query_as(statement).bind(before);
    if let Some(documents) = documents {
        query = query.bind(documents);
    }
    let gone: Vec<(Id, i64)> = query.fetch_all(&mut *conn).await?;
    let mut count = 0_i64;
    for (document, seq) in &gone {
        sqlx::query("UPDATE documents SET purged_seq = GREATEST(purged_seq, $2) WHERE id = $1")
            .bind(document)
            .bind(seq)
            .execute(&mut *conn)
            .await?;
        count += 1;
    }
    Ok(count)
}

/// Removes the records of accepted batches that are older than `before`.
pub async fn purge_batch_records(conn: &mut Conn, before: DateTime<Utc>) -> Result<u64, DbError> {
    purge_batch_records_selected(conn, before, None).await
}

/// Expires accepted batch ids only in the locked document page.
pub async fn purge_batch_records_among(
    conn: &mut Conn,
    before: DateTime<Utc>,
    documents: &[Id],
) -> Result<u64, DbError> {
    purge_batch_records_selected(conn, before, Some(documents)).await
}

async fn purge_batch_records_selected(
    conn: &mut Conn,
    before: DateTime<Utc>,
    documents: Option<&[Id]>,
) -> Result<u64, DbError> {
    let statement = if documents.is_some() {
        "DELETE FROM document_ops WHERE document_id=ANY($2) AND created_at < $1"
    } else {
        "DELETE FROM document_ops WHERE created_at < $1"
    };
    let mut query = sqlx::query(statement).bind(before);
    if let Some(documents) = documents {
        query = query.bind(documents);
    }
    Ok(query.execute(conn).await?.rows_affected())
}

/// One stable page for daily maintenance, locking documents before their blocks or versions.
pub async fn maintenance_page(
    conn: &mut Conn,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as("SELECT id,node_id FROM documents WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT $2 FOR UPDATE")
        .bind(after).bind(limit).fetch_all(conn).await?)
}

/// The files nothing names any more, each with its document: no block of the document names it,
/// live or removed and not yet purged (a removed block can come back), and no version the document
/// keeps holds its id (a version can be put back), and it was put in before `added_before`, so that a
/// file uploaded a moment before the block that names it is never taken. At most `limit`, the oldest
/// first; `only` asks about that one file alone.
async fn unnamed(
    conn: &mut Conn,
    added_before: DateTime<Utc>,
    limit: i64,
    only: Option<Id>,
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT f.document_id, f.id FROM document_files f
          WHERE f.added_at < $1 AND ($3::uuid IS NULL OR f.id = $3)
            AND NOT EXISTS (SELECT 1 FROM document_blocks b
                             WHERE b.document_id = f.document_id AND b.props->>'file_id' = f.id::text)
            AND NOT EXISTS (SELECT 1 FROM document_versions v
                             WHERE v.document_id = f.document_id
                               AND strpos(COALESCE(v.whole, v.change)::text, f.id::text) > 0)
          ORDER BY f.added_at, f.id LIMIT $2",
    )
    .bind(added_before)
    .bind(limit)
    .bind(only)
    .fetch_all(conn)
    .await?)
}

/// The files nothing names any more and that were put in before `added_before`, each with its
/// document, at most `limit` of them, the oldest first: what the purge of documents takes out.
pub async fn unnamed_files(
    conn: &mut Conn,
    added_before: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(Id, Id)>, DbError> {
    unnamed(conn, added_before, limit, None).await
}

/// Takes out the file `file` of `document` where nothing names it still, under the lock of the
/// document, lets go of the pin it held on its bytes and stamps the document, as taking a file out by
/// hand does. False where something named it meanwhile, or it is gone.
pub async fn release_unnamed_file(
    conn: &mut Conn,
    document: Id,
    file: Id,
    added_before: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    if lock(conn, document).await?.is_none()
        || unnamed(conn, added_before, 1, Some(file)).await?.is_empty()
    {
        return Ok(false);
    }
    let Some(held) = self::file(conn, document, file).await? else {
        return Ok(false);
    };
    if !remove_file(conn, document, file).await? {
        return Ok(false);
    }
    crate::queries::storage::unpin_blobs(conn, &[held.blob], at).await?;
    touch(conn, document).await?;
    Ok(true)
}

/// The documents that lie at the nodes `nodes` or in the trees of the root orgs among them: the rows
/// that go with those nodes when they are purged.
pub async fn of_nodes(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM documents WHERE node_id = ANY($1) OR org_id = ANY($1) ORDER BY id",
    )
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// Lets go of the pins that the files of `documents` hold on their bytes, one for each file, as
/// taking each out would: what purges the documents does this first, since the rows of their files
/// go with them and nothing would let the pins go after. Returns how many blobs lost a pin.
pub async fn release_file_pins(
    conn: &mut Conn,
    documents: &[Id],
    at: DateTime<Utc>,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE blobs b
            SET pin_count = GREATEST(b.pin_count - f.files, 0),
                last_access_at = CASE WHEN b.pin_count <= f.files THEN GREATEST(b.last_access_at, $2)
                                      ELSE b.last_access_at END
           FROM (SELECT blob, count(*)::int AS files FROM document_files
                  WHERE document_id = ANY($1) GROUP BY blob) f
          WHERE b.hash = f.blob AND b.pin_count > 0",
    )
    .bind(documents)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected())
}

#[cfg(test)]
pub(crate) mod testing {
    //! A document and its place, for the tests of this module and the ones that follow.

    use super::*;
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::insert_user;
    use orch_types::new_id;

    use super::batch::{self, Limits, Outcome, Rules};
    use orch_api_contract::documents::{BlockContent, BlockType, DocumentOp, Inline, NewBlock};

    /// What a document may hold, as shipped.
    pub const LIMITS: Limits = Limits {
        max_bytes: 2 * 1024 * 1024,
        max_blocks: 5_000,
    };
    /// When versions are cut and how many are kept, as shipped.
    pub const RULES: Rules = Rules {
        window_secs: 600,
        kept: 200,
        history_max_bytes: 16 * 1024 * 1024,
    };

    pub fn paragraph(id: Id, words: &str) -> NewBlock {
        NewBlock {
            id,
            block_type: BlockType::Paragraph,
            props: None,
            content: Some(BlockContent::Inline(vec![Inline::Text {
                text: words.to_owned(),
                marks: None,
                href: None,
                color: None,
                background: None,
            }])),
            children: None,
        }
    }

    pub fn put(id: Id, words: &str) -> DocumentOp {
        DocumentOp::Insert {
            block: paragraph(id, words),
            parent_id: None,
            after_id: None,
        }
    }

    pub fn edit(id: Id, base_seq: i64, words: &str) -> DocumentOp {
        DocumentOp::Update {
            id,
            base_seq,
            block_type: None,
            props: None,
            content: paragraph(id, words).content,
        }
    }

    /// A batch by a person, with a client identifier of its own.
    pub async fn write(
        conn: &mut Conn,
        document: Id,
        by: Id,
        ops: &[DocumentOp],
        at: DateTime<Utc>,
    ) -> Outcome {
        batch::apply(
            conn,
            document,
            &Writer::User(by),
            Some(orch_types::new_id()),
            ops,
            LIMITS,
            RULES,
            at,
        )
        .await
        .unwrap()
    }

    /// A person, a root org, a project and a topic, and a document beside the topic.
    pub struct Place {
        pub ada: Id,
        pub bob: Id,
        pub org: Id,
        pub project: Id,
        pub topic: Id,
        pub document: Id,
    }

    pub async fn place(conn: &mut Conn, now: DateTime<Utc>) -> Place {
        let (ada, bob, org, project, topic, document) =
            (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
        insert_user(conn, ada, "ada").await.unwrap();
        insert_user(conn, bob, "bob").await.unwrap();
        insert_node(conn, org, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(conn, project, "project", Some(org), "Web", ada)
            .await
            .unwrap();
        insert_node(conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        insert(
            conn,
            &NewDocument {
                id: document,
                node_id: topic,
                org_id: org,
                owner_id: ada,
                created_by_thread: None,
                created_by_agent: None,
                title: "Handbook",
                now,
            },
        )
        .await
        .unwrap();
        Place {
            ada,
            bob,
            org,
            project,
            topic,
            document,
        }
    }
}

// ---------------------------------------------------------------- what a place reaches (giving as a file or a folder is given)

/// The documents that are written beside `place` or beside anything inside it, at any depth, that
/// were not deleted; those beside a side thread are left out, as that thread is no thing of its own.
pub async fn beside_or_inside(conn: &mut Conn, place: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT d.id
           FROM documents d
           JOIN nodes n ON n.id = d.node_id
           LEFT JOIN threads t ON t.node_id = n.id
          WHERE n.path @> ARRAY[$1]::uuid[] AND n.deleted_at IS NULL AND d.deleted_at IS NULL
            AND t.side_of IS NULL
          ORDER BY d.id",
    )
    .bind(place)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- managed by an agent

/// Deletes the document, softly, as an agent does: it can be restored until it is purged, and nobody is named as
/// having deleted it, as nobody is named as having changed what an agent changed last (the audit row says which
/// agent). False where it is gone already.
pub async fn soft_delete_by_agent(
    conn: &mut Conn,
    document: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    containers::lock_document_scope(conn, document).await?;
    let done = sqlx::query(
        "UPDATE documents SET deleted_at = $2, deleted_by = NULL, deleted_in=NULL WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(document)
    .bind(now)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if done > 0 {
        tree::stamp(conn, document).await?;
    }
    Ok(done > 0)
}

#[cfg(test)]
mod tests {
    use super::testing::place;
    use super::*;
    use orch_documents::versions::Writer;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    #[tokio::test]
    async fn a_document_is_listed_beside_its_node_and_with_those_below_when_asked() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let list = |node, under, deleted| ListWhere {
            node,
            under,
            deleted,
            after: None,
            offset: 0,
            limit: 50,
        };
        let ids = |rows: Vec<DocumentRow>| rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
        assert_eq!(
            ids(list_beside(&mut conn, &list(here.topic, false, false))
                .await
                .unwrap()),
            [here.document]
        );
        assert!(
            list_beside(&mut conn, &list(here.project, false, false))
                .await
                .unwrap()
                .is_empty(),
            "beside the project there is none"
        );
        assert_eq!(
            ids(list_beside(&mut conn, &list(here.project, true, false))
                .await
                .unwrap()),
            [here.document],
            "and with what is below it, there is one"
        );
        assert!(
            list_beside(&mut conn, &list(here.topic, false, true))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_most_recently_changed_come_first_and_a_list_resumes_after_the_last_it_showed() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let here = place(&mut conn, start).await;
        let mut made = vec![here.document];
        for minute in 1..=3 {
            let id = new_id();
            let title = format!("Later {minute}");
            insert(
                &mut conn,
                &NewDocument {
                    id,
                    node_id: here.topic,
                    org_id: here.org,
                    owner_id: here.ada,
                    created_by_thread: None,
                    created_by_agent: None,
                    title: &title,
                    now: start + chrono::Duration::minutes(minute),
                },
            )
            .await
            .unwrap();
            made.push(id);
        }
        let page = |after| ListWhere {
            node: here.topic,
            under: false,
            deleted: false,
            after,
            offset: 0,
            limit: 2,
        };
        let first = list_beside(&mut conn, &page(None)).await.unwrap();
        assert_eq!(
            first.iter().map(|d| d.id).collect::<Vec<_>>(),
            [made[3], made[2]]
        );
        let last = first.last().unwrap();
        let second = list_beside(
            &mut conn,
            &page(Some(ListCursor {
                at: last.updated_at,
                id: last.id,
            })),
        )
        .await
        .unwrap();
        assert_eq!(
            second.iter().map(|d| d.id).collect::<Vec<_>>(),
            [made[1], made[0]]
        );
    }

    #[tokio::test]
    async fn a_document_beside_a_deleted_node_is_not_listed_and_comes_back_with_it() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
            .bind(here.project)
            .bind(now)
            .execute(&mut *conn)
            .await
            .unwrap();
        let list = ListWhere {
            node: here.topic,
            under: false,
            deleted: false,
            after: None,
            offset: 0,
            limit: 50,
        };
        assert!(list_beside(&mut conn, &list).await.unwrap().is_empty());
        assert_eq!(
            gone(&mut conn, &[here.document]).await.unwrap(),
            [here.document]
        );
        sqlx::query("UPDATE nodes SET deleted_at = NULL WHERE id = $1")
            .bind(here.project)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(list_beside(&mut conn, &list).await.unwrap().len(), 1);
        assert!(gone(&mut conn, &[here.document]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_deleted_document_is_listed_as_deleted_and_comes_back_until_it_is_purged() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        assert!(
            soft_delete(&mut conn, here.document, here.ada, now)
                .await
                .unwrap()
        );
        assert!(
            !soft_delete(&mut conn, here.document, here.ada, now)
                .await
                .unwrap(),
            "once"
        );
        let deleted = ListWhere {
            node: here.topic,
            under: false,
            deleted: true,
            after: None,
            offset: 0,
            limit: 50,
        };
        assert_eq!(list_beside(&mut conn, &deleted).await.unwrap().len(), 1);
        assert!(
            next_seq(&mut conn, here.document).await.unwrap().is_none(),
            "nothing is written in it"
        );
        let seq = restore_deleted(&mut conn, here.document).await.unwrap();
        assert_eq!(
            seq,
            Some(1),
            "and its coming back is a change the feed tells"
        );
        assert!(list_beside(&mut conn, &deleted).await.unwrap().is_empty());
        assert!(
            restore_deleted(&mut conn, here.document)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_sequence_of_a_document_counts_one_after_the_other_and_a_rename_is_a_change() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        assert_eq!(next_seq(&mut conn, here.document).await.unwrap(), Some(1));
        let renamed = rename(
            &mut conn,
            here.document,
            "Manual",
            &Writer::User(here.bob),
            now,
        )
        .await
        .unwrap();
        assert_eq!(renamed, Some(2));
        let row = get(&mut conn, here.document).await.unwrap().unwrap();
        assert_eq!(
            (row.title.as_str(), row.doc_seq, row.seq_updated),
            ("Manual", 2, 2)
        );
        assert_eq!(row.updated_by, Some(here.bob));
        assert_eq!(
            sequences(&mut conn, &[here.document]).await.unwrap(),
            [(here.document, 2)]
        );
    }

    #[tokio::test]
    async fn a_description_is_kept_by_whoever_wrote_it_and_is_a_change_like_any_other() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let made = get(&mut conn, here.document).await.unwrap().unwrap();
        assert_eq!(
            (made.description.as_str(), made.description_source.as_str()),
            ("", "auto"),
            "a new document has none, and automation may write one"
        );
        let later = now + chrono::Duration::minutes(1);
        let seq = describe(
            &mut conn,
            here.document,
            "How we ship\nand roll back",
            &Writer::User(here.bob),
            later,
        )
        .await
        .unwrap();
        assert_eq!(seq, Some(1));
        let row = get(&mut conn, here.document).await.unwrap().unwrap();
        assert_eq!(
            (
                row.description.as_str(),
                row.description_source.as_str(),
                row.doc_seq,
                row.seq_updated,
                row.updated_by,
                row.updated_at
            ),
            (
                "How we ship\nand roll back",
                "user",
                1,
                1,
                Some(here.bob),
                later
            )
        );
        soft_delete(&mut conn, here.document, here.ada, later)
            .await
            .unwrap();
        assert_eq!(
            describe(
                &mut conn,
                here.document,
                "Gone",
                &Writer::User(here.bob),
                later
            )
            .await
            .unwrap(),
            None,
            "a deleted document is described no more"
        );
    }

    #[tokio::test]
    async fn a_document_without_a_description_is_given_its_first_paragraph_in_page_order() {
        use super::testing::{paragraph, write};
        use orch_api_contract::documents::{BlockType, DocumentOp, NewBlock};
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        assert!(
            first_paragraphs(&mut conn, &[here.document], 200)
                .await
                .unwrap()
                .is_empty(),
            "an empty page has no first paragraph"
        );
        let (heading, blank, first, nested, second) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let mut title = paragraph(heading, "Rollout plan");
        title.block_type = BlockType::Heading;
        title.props = serde_json::from_value(serde_json::json!({ "level": 1 })).ok();
        let mut outer = paragraph(first, "Ship the redirect fix behind the flag.");
        outer.children = Some(vec![paragraph(nested, "A nested line.")]);
        let at_end = |block: NewBlock, after: Id| DocumentOp::Insert {
            block,
            parent_id: None,
            after_id: Some(after),
        };
        let ops = vec![
            DocumentOp::Insert {
                block: title,
                parent_id: None,
                after_id: None,
            },
            at_end(paragraph(blank, "   "), heading),
            at_end(outer, blank),
            at_end(paragraph(second, "Then announce it."), first),
        ];
        write(&mut conn, here.document, here.ada, &ops, now).await;
        let found = first_paragraphs(&mut conn, &[here.document], 200)
            .await
            .unwrap();
        assert_eq!(
            found.get(&here.document).map(String::as_str),
            Some("Ship the redirect fix behind the flag."),
            "a heading and a blank paragraph are passed over"
        );
        assert_eq!(
            first_paragraphs(&mut conn, &[here.document], 4)
                .await
                .unwrap()
                .get(&here.document)
                .map(String::as_str),
            Some("Ship"),
            "the text is cut to the characters asked for"
        );
        write(
            &mut conn,
            here.document,
            here.ada,
            &[DocumentOp::Remove { id: first }],
            now,
        )
        .await;
        assert_eq!(
            first_paragraphs(&mut conn, &[here.document], 200)
                .await
                .unwrap()
                .get(&here.document)
                .map(String::as_str),
            Some("Then announce it."),
            "a removed paragraph, and what was nested in it, are passed over"
        );
    }

    #[tokio::test]
    async fn automatic_descriptions_keep_written_reference_labels_in_existing_paragraphs() {
        use super::testing::{paragraph, write};
        use orch_api_contract::documents::DocumentOp;
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let first = new_id();
        let mut opening = paragraph(first, "");
        opening.content = Some(
            serde_json::from_value(serde_json::json!([
                {"type": "text", "text": "The design depends on the "},
                {"type": "ref", "kind": "document", "id": here.document, "label": "Rollout plan"},
                {"type": "text", "text": "."}
            ]))
            .unwrap(),
        );
        write(
            &mut conn,
            here.document,
            here.ada,
            &[
                DocumentOp::Insert {
                    block: opening,
                    parent_id: None,
                    after_id: None,
                },
                DocumentOp::Insert {
                    block: paragraph(new_id(), "Then announce it."),
                    parent_id: None,
                    after_id: Some(first),
                },
            ],
            now,
        )
        .await;
        let search_text: String =
            sqlx::query_scalar("SELECT text FROM document_blocks WHERE id = $1")
                .bind(first)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(
            search_text, "The design depends on the .",
            "the existing stored search text has no reference label"
        );
        assert_eq!(
            first_paragraphs(&mut conn, &[here.document], 200)
                .await
                .unwrap()[&here.document],
            "The design depends on the Rollout plan."
        );
        assert_eq!(
            first_paragraphs(&mut conn, &[here.document], 29)
                .await
                .unwrap()[&here.document],
            "The design depends on the Rol",
            "the whole preview is cut after inserting labels"
        );

        // Old reference-only paragraphs have empty search text. Only their saved words belong in the preview.
        sqlx::query("UPDATE document_blocks SET text = '', content = $2 WHERE id = $1")
            .bind(first).bind(serde_json::json!([
                {"type": "ref", "kind": "document", "id": new_id(), "label": "Only for me: plan", "name": "Private live name"}
            ])).execute(&mut *conn).await.unwrap();
        assert_eq!(
            first_paragraphs(&mut conn, &[here.document], 200)
                .await
                .unwrap()[&here.document],
            "Only for me: plan",
            "a saved label is retained without resolving or trusting a hydrated name"
        );
        sqlx::query("UPDATE document_blocks SET content = $2 WHERE id = $1")
            .bind(first)
            .bind(serde_json::json!([
                {"type": "ref", "kind": "document", "id": new_id(), "name": "Private live name"}
            ]))
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            first_paragraphs(&mut conn, &[here.document], 200)
                .await
                .unwrap()[&here.document],
            "Then announce it.",
            "an older unlabelled reference supplies no target identity"
        );
    }

    #[tokio::test]
    async fn how_far_a_person_has_had_a_document_on_screen_only_moves_forward() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        assert!(
            seen_seqs(&mut conn, here.bob, &[here.document])
                .await
                .unwrap()
                .is_empty()
        );
        mark_seen(&mut conn, here.bob, here.document, 5, now)
            .await
            .unwrap();
        let later = now + chrono::Duration::minutes(1);
        mark_seen(&mut conn, here.bob, here.document, 3, later)
            .await
            .unwrap();
        let seen = seen_seqs(&mut conn, here.bob, &[here.document])
            .await
            .unwrap();
        assert_eq!(seen[&here.document].0, 5);
        assert_eq!(
            seen[&here.document].1, now,
            "an older number moves nothing, not even the moment"
        );
        mark_seen(&mut conn, here.bob, here.document, 9, later)
            .await
            .unwrap();
        let seen = seen_seqs(&mut conn, here.bob, &[here.document])
            .await
            .unwrap();
        assert_eq!((seen[&here.document].0, seen[&here.document].1), (9, later));
    }

    #[tokio::test]
    async fn the_announcement_reaches_a_listener_after_the_commit() {
        let test = TestDb::fresh().await;
        let now = clock::start();
        let here = {
            let mut conn = test.pool.acquire().await.unwrap();
            place(&mut conn, now).await
        };
        let mut listener = sqlx::postgres::PgListener::connect(&test.url)
            .await
            .unwrap();
        listener.listen(DOC_CHANNEL).await.unwrap();
        let mut tx = test.pool.begin().await.unwrap();
        let seq = next_seq(&mut tx, here.document).await.unwrap().unwrap();
        notify(&mut tx, here.document, seq).await.unwrap();
        tx.commit().await.unwrap();
        let heard = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(heard.payload(), format!("{}:{seq}", here.document));
    }
}

// Announcing document placement and visibility to open trees.
pub mod tree;

/// Writes an unchanged automatic description, stamped and notified without cutting a content version.
pub async fn describe_automatically(
    conn: &mut Conn,
    document: Id,
    description: &str,
    expected: Seq,
    now: DateTime<Utc>,
) -> Result<Option<Seq>, DbError> {
    let seq: Option<Seq> = sqlx::query_scalar("UPDATE documents SET description=$2,doc_seq=doc_seq+1,seq_updated=doc_seq+1,updated_at=$4,updated_by=NULL WHERE id=$1 AND doc_seq=$3 AND description_source='auto' AND deleted_at IS NULL AND description<>$2 RETURNING doc_seq")
        .bind(document).bind(description).bind(expected).bind(now).fetch_optional(&mut *conn).await?;
    if let Some(seq) = seq {
        notify(conn, document, seq).await?;
        tree::stamp(conn, document).await?;
    }
    Ok(seq)
}
