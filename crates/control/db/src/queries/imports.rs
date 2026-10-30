//! Imports into a place: whose each is, what was asked, where it stands, the report with a row for each object, and for each thing an import made or took over the thing of its source that it corresponds to, so that a later import finds it and an import can be taken back.
//!
//! A row of `imports` is made with the job that does the import. Its report is written as the job goes, a row for
//! each object, so that a job that was cancelled or failed leaves the account of how far it came. A row of
//! `import_origins` is keyed by the place the person imported into and the identifier the thing had in its source;
//! it says what the thing is here, which import made it and which wrote it last. A row whose object was deleted
//! is no counterpart, and the next import that meets it writes it again.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// The words of `imports.state`.
pub mod state {
    /// A job is bringing it.
    pub const PENDING: &str = "pending";
    /// It was brought.
    pub const DONE: &str = "done";
    /// The job failed.
    pub const FAILED: &str = "failed";
    /// The job was cancelled.
    pub const CANCELLED: &str = "cancelled";
    /// A job is taking back what it made.
    pub const UNDOING: &str = "undoing";
    /// What it made was taken back.
    pub const UNDONE: &str = "undone";
}

/// The words of `import_origins.kind`.
pub mod kind {
    /// A node of any of the four kinds is named by its own word.
    pub const CHAT: &str = "chat";
    /// A document folder.
    pub const FOLDER: &str = "folder";
    /// A document.
    pub const DOCUMENT: &str = "document";
    /// A message of a chat.
    pub const MESSAGE: &str = "message";
    /// A turn of a thread.
    pub const TURN: &str = "turn";
    /// An item of a thread.
    pub const ITEM: &str = "item";
    /// A comment of a document.
    pub const COMMENT: &str = "comment";
    /// An earlier version of a document.
    pub const VERSION: &str = "version";
}

/// An import, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ImportRow {
    /// Its identifier.
    pub id: Id,
    /// Whose it is; none once the person is gone.
    pub user_id: Option<Id>,
    /// The place it went into; none once the place was purged.
    pub node_id: Option<Id>,
    /// The person's identity of the request.
    pub client_id: String,
    /// What was asked, as the request was written.
    pub request: Json,
    /// The job that brings it.
    pub job_id: Option<Id>,
    /// The job that takes it back.
    pub undo_job_id: Option<Id>,
    /// Where it stands.
    pub state: String,
    /// The names of the files it was made from.
    pub files: Vec<String>,
    /// The uploads it holds a pin on until it ends, as the hashes are written.
    pub source_blobs: Vec<String>,
    /// The header of the package it was made from.
    pub package: Option<Json>,
    /// What it made or changed, counted by kind.
    pub counts: Option<Json>,
    /// What it did with its objects, counted.
    pub actions: Option<Json>,
    /// What did not come back.
    pub losses: Json,
    /// Why it failed.
    pub failure: Option<Json>,
    /// When it was asked for.
    pub created_at: DateTime<Utc>,
    /// When it ended.
    pub finished_at: Option<DateTime<Utc>>,
    /// When what it made was taken back.
    pub undone_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str =
    "id, user_id, node_id, client_id, request, job_id, undo_job_id, state, files, source_blobs,
    package, counts, actions, losses, failure, created_at, finished_at, undone_at";

/// What a new import is made of.
#[derive(Debug, Clone)]
pub struct NewImport<'a> {
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user: Id,
    /// The place it goes into.
    pub node: Id,
    /// The person's identity of the request.
    pub client_id: &'a str,
    /// The request as it was written.
    pub request: &'a Json,
    /// The names of the files it is made from.
    pub files: &'a [String],
    /// The uploads it pins, as the hashes are written.
    pub source_blobs: &'a [String],
    /// The header of the package, if it is one.
    pub package: Option<&'a Json>,
    /// When it was asked for.
    pub at: DateTime<Utc>,
}

/// Makes the row of an import. The person's repeated request names the same row: `None` says that there was one
/// already.
pub async fn insert(conn: &mut Conn, new: &NewImport<'_>) -> Result<Option<ImportRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO imports (id, user_id, node_id, client_id, request, files, source_blobs, package, state, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9)
         ON CONFLICT (user_id, client_id) DO NOTHING
         RETURNING {COLUMNS}"
    )))
    .bind(new.id)
    .bind(new.user)
    .bind(new.node)
    .bind(new.client_id)
    .bind(new.request)
    .bind(new.files)
    .bind(new.source_blobs)
    .bind(new.package)
    .bind(new.at)
    .fetch_optional(conn)
    .await?)
}

/// The import a person asked for under that identity, if there is one.
pub async fn by_client(
    conn: &mut Conn,
    user: Id,
    client_id: &str,
) -> Result<Option<ImportRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM imports WHERE user_id = $1 AND client_id = $2"
    )))
    .bind(user)
    .bind(client_id)
    .fetch_optional(conn)
    .await?)
}

/// An import by its identifier.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<ImportRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM imports WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// An import by its identifier, locked for the rest of the transaction.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<ImportRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM imports WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Says which job brings an import.
pub async fn attach_job(conn: &mut Conn, id: Id, job: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE imports SET job_id = $2 WHERE id = $1")
        .bind(id)
        .bind(job)
        .execute(conn)
        .await?;
    Ok(())
}

/// Says which job takes an import back, and that it is being taken back.
pub async fn begin_undo(conn: &mut Conn, id: Id, job: Id) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE imports SET undo_job_id = $2, state = 'undoing'
          WHERE id = $1 AND state IN ('done','failed','cancelled')",
    )
    .bind(id)
    .bind(job)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// The history statement for a first or subsequent page. A distinct cursor
/// shape lets cached generic plans seek instead of filtering preceding rows.
pub fn history_page_statement(after: bool) -> String {
    let cursor = if after {
        "(created_at, id) < ($2::timestamptz, $3::uuid)"
    } else {
        "$2::timestamptz IS NULL AND $3::uuid IS NULL"
    };
    format!(
        "SELECT id, node_id, state, job_id, undo_job_id, files[1] AS first_file,
                cardinality(files)::bigint AS file_count, actions, created_at, finished_at, undone_at
           FROM imports WHERE user_id = $1 AND {cursor}
          ORDER BY created_at DESC, id DESC LIMIT $4"
    )
}

/// An import as a list shows it: without what was asked, the names of its files after the first, or what did not
/// come back.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ImportLineRow {
    /// Its identifier.
    pub id: Id,
    /// The place it went into; none once the place was purged.
    pub node_id: Option<Id>,
    /// `pending`, `done`, `failed`, `undoing` or `undone`.
    pub state: String,
    /// The job that brings it.
    pub job_id: Option<Id>,
    /// The job that takes it back.
    pub undo_job_id: Option<Id>,
    /// The first of the files it was made from.
    pub first_file: Option<String>,
    /// How many files it was made from.
    pub file_count: i64,
    /// What it did with its objects.
    pub actions: Option<Json>,
    /// When it was asked for.
    pub created_at: DateTime<Utc>,
    /// When it ended.
    pub finished_at: Option<DateTime<Utc>>,
    /// When what it made was taken back.
    pub undone_at: Option<DateTime<Utc>>,
}

/// The imports of a person as a list shows them, newest first: a page after the one shown last.
pub async fn lines_of_user(
    conn: &mut Conn,
    user: Id,
    after: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<Vec<ImportLineRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(history_page_statement(after.is_some())))
            .bind(user)
            .bind(after.map(|each| each.0))
            .bind(after.map(|each| each.1))
            .bind(limit)
            .fetch_all(conn)
            .await?,
    )
}

/// The names of the files the import `id` of `user` was made from, with their places from 1, after the place
/// `after`: none where the import is not theirs.
pub async fn file_names(
    conn: &mut Conn,
    user: Id,
    id: Id,
    after: i64,
    limit: i64,
) -> Result<Option<Vec<(i64, String)>>, DbError> {
    let theirs: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM imports WHERE id = $1 AND user_id = $2)")
            .bind(id)
            .bind(user)
            .fetch_one(&mut *conn)
            .await?;
    if !theirs {
        return Ok(None);
    }
    Ok(Some(
        sqlx::query_as(
            "SELECT e.place, e.name FROM imports i
               CROSS JOIN LATERAL unnest(i.files) WITH ORDINALITY AS e(name, place)
              WHERE i.id = $1 AND e.place > $2 ORDER BY e.place LIMIT $3",
        )
        .bind(id)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?,
    ))
}

/// Takes the lock that makes one person's imports counted and begun one request at a time, until the transaction
/// ends.
pub async fn lock_person(conn: &mut Conn, user: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('imports:' || $1::text, 0))")
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

/// How many imports of a person are being brought or taken back now.
pub async fn running(conn: &mut Conn, user: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM imports WHERE user_id = $1 AND state IN ('pending','undoing')",
    )
    .bind(user)
    .fetch_one(conn)
    .await?)
}

/// What an import that ended says of itself.
#[derive(Debug, Clone)]
pub struct Ended<'a> {
    /// Where it ends: done, failed or cancelled.
    pub state: &'a str,
    /// What it made or changed, counted by kind.
    pub counts: &'a Json,
    /// What it did with its objects, counted.
    pub actions: &'a Json,
    /// What did not come back.
    pub losses: &'a Json,
    /// Why it failed.
    pub failure: Option<&'a Json>,
    /// The header of the package, once it was read.
    pub package: Option<&'a Json>,
    /// When it ended.
    pub at: DateTime<Utc>,
}

/// Writes how an import ended, and lets its uploads go.
pub async fn end(conn: &mut Conn, id: Id, ended: &Ended<'_>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE imports SET state = $2, counts = $3, actions = $4, losses = $5, failure = $6,
                package = COALESCE($7, package), finished_at = $8, source_blobs = '{}'
          WHERE id = $1",
    )
    .bind(id)
    .bind(ended.state)
    .bind(ended.counts)
    .bind(ended.actions)
    .bind(ended.losses)
    .bind(ended.failure)
    .bind(ended.package)
    .bind(ended.at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Writes that what an import made was taken back, or that taking it back stopped where it was.
pub async fn end_undo(
    conn: &mut Conn,
    id: Id,
    done: bool,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE imports SET state = CASE WHEN $2 THEN 'undone' ELSE 'done' END,
                undone_at = CASE WHEN $2 THEN $3 ELSE undone_at END
          WHERE id = $1",
    )
    .bind(id)
    .bind(done)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- the report

/// A row of the report, to be written.
#[derive(Debug, Clone)]
pub struct NewReportRow {
    /// Its place in the report.
    pub position: i64,
    /// The object, by its identifier in the source.
    pub source_id: Id,
    /// What it is, as its word is written.
    pub kind: String,
    /// What became of it, as its word is written.
    pub outcome: String,
    /// Whether it came back changed.
    pub warned: bool,
    /// The row as the API shows it.
    pub row: Json,
}

/// Writes rows of the report of an import.
pub async fn add_report_rows(
    conn: &mut Conn,
    import: Id,
    rows: &[NewReportRow],
) -> Result<(), DbError> {
    if rows.is_empty() {
        return Ok(());
    }
    let positions: Vec<i64> = rows.iter().map(|row| row.position).collect();
    let sources: Vec<Id> = rows.iter().map(|row| row.source_id).collect();
    let kinds: Vec<String> = rows.iter().map(|row| row.kind.clone()).collect();
    let outcomes: Vec<String> = rows.iter().map(|row| row.outcome.clone()).collect();
    let warned: Vec<bool> = rows.iter().map(|row| row.warned).collect();
    let bodies: Vec<Json> = rows.iter().map(|row| row.row.clone()).collect();
    sqlx::query(
        "INSERT INTO import_report_rows (import_id, position, source_id, kind, outcome, warned, row)
         SELECT $1, p, s, k, o, w, b
           FROM unnest($2::bigint[], $3::uuid[], $4::text[], $5::text[], $6::boolean[], $7::jsonb[])
                AS t(p, s, k, o, w, b)
         ON CONFLICT (import_id, position) DO UPDATE
            SET source_id = EXCLUDED.source_id, kind = EXCLUDED.kind, outcome = EXCLUDED.outcome,
                warned = EXCLUDED.warned, row = EXCLUDED.row",
    )
    .bind(import)
    .bind(&positions)
    .bind(&sources)
    .bind(&kinds)
    .bind(&outcomes)
    .bind(&warned)
    .bind(&bodies)
    .execute(conn)
    .await?;
    Ok(())
}

/// Which rows of a report.
#[derive(Debug, Clone, Default)]
pub struct ReportFilter<'a> {
    /// Only rows that came to this.
    pub outcome: Option<&'a str>,
    /// Only objects of this kind.
    pub kind: Option<&'a str>,
    /// Only rows with a warning.
    pub warned: bool,
}

/// The statement of [`report_rows`]: `$1` the import, `$2` the position to go on after (-1 for the first page), `$3` and `$4` the outcome and
/// the kind asked for (null for any), `$5` whether only rows with a warning are wanted, `$6` how many.
pub const REPORT_ROWS_SQL: &str = "SELECT position, row FROM import_report_rows
          WHERE import_id = $1 AND position > $2
            AND ($3::text IS NULL OR outcome = $3)
            AND ($4::text IS NULL OR kind = $4)
            AND (NOT $5 OR warned)
          ORDER BY position LIMIT $6";

/// The report statement with only its selected predicates. Each filter shape
/// has its own cached plan so rare matches seek their complete ordered index.
/// All values remain parameters; these fragments depend only on filter presence.
pub fn report_rows_statement(filter: &ReportFilter<'_>) -> String {
    REPORT_ROWS_SQL
        .replace(
            "($3::text IS NULL OR outcome = $3)",
            if filter.outcome.is_some() {
                "outcome = $3"
            } else {
                "$3::text IS NULL"
            },
        )
        .replace(
            "($4::text IS NULL OR kind = $4)",
            if filter.kind.is_some() {
                "kind = $4"
            } else {
                "$4::text IS NULL"
            },
        )
        .replace(
            "(NOT $5 OR warned)",
            if filter.warned {
                "($5::boolean AND warned)"
            } else {
                "NOT $5::boolean"
            },
        )
}

/// A page of the report, in the order it was written: those after position `after`.
pub async fn report_rows(
    conn: &mut Conn,
    import: Id,
    filter: &ReportFilter<'_>,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<(i64, Json)>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(report_rows_statement(filter)))
            .bind(import)
            .bind(after.unwrap_or(-1))
            .bind(filter.outcome)
            .bind(filter.kind)
            .bind(filter.warned)
            .bind(limit)
            .fetch_all(conn)
            .await?,
    )
}

/// How many rows of the report came to each outcome, and how many came with a warning.
pub async fn report_counts(
    conn: &mut Conn,
    import: Id,
) -> Result<(Vec<(String, i64)>, i64), DbError> {
    let by_outcome: Vec<(String, i64)> = sqlx::query_as(
        "SELECT outcome, count(*) FROM import_report_rows WHERE import_id = $1 GROUP BY outcome",
    )
    .bind(import)
    .fetch_all(&mut *conn)
    .await?;
    let warned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM import_report_rows WHERE import_id = $1 AND warned",
    )
    .bind(import)
    .fetch_one(conn)
    .await?;
    Ok((by_outcome, warned))
}

/// The most recent position written to the report of an import: where a job that was taken over goes on.
pub async fn last_position(conn: &mut Conn, import: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(max(position), -1) FROM import_report_rows WHERE import_id = $1",
    )
    .bind(import)
    .fetch_one(conn)
    .await?)
}

// ---------------------------------------------------------------- what things are here

/// What a thing of a source is here.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct OriginRow {
    /// The place the person imported into.
    pub scope_id: Id,
    /// The identifier the thing had in its source.
    pub source_id: Id,
    /// What it is, as its word is written.
    pub kind: String,
    /// What it is here.
    pub object_id: Id,
    /// The thing here that it lies in, for a row that is no object of its own.
    pub parent_id: Option<Id>,
    /// The import that made it; none where it took over what was there.
    pub created_by_import: Option<Id>,
    /// The import that wrote it last.
    pub last_import: Id,
    /// Its name there.
    pub source_name: Option<String>,
    /// Where it lay in the source.
    pub source_path: Option<String>,
    /// The hash of its record there.
    pub source_sha256: Option<String>,
    /// When it last changed there.
    pub source_updated_at: Option<DateTime<Utc>>,
    /// Who made it there: `{ name, username }`.
    pub source_creator: Option<Json>,
    /// Who imported it.
    pub imported_by: Option<Id>,
    /// When it was imported.
    pub imported_at: DateTime<Utc>,
}

const ORIGIN_COLUMNS: &str = "scope_id, source_id, kind, object_id, parent_id, created_by_import, last_import,
    source_name, source_path, source_sha256, source_updated_at, source_creator, imported_by, imported_at";

/// What the things `sources` of a source are here, in the place `scope`.
pub async fn origins_of(
    conn: &mut Conn,
    scope: Id,
    sources: &[Id],
) -> Result<Vec<OriginRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ORIGIN_COLUMNS} FROM import_origins WHERE scope_id = $1 AND source_id = ANY($2)"
    )))
    .bind(scope)
    .bind(sources)
    .fetch_all(conn)
    .await?)
}

/// What a thing here was made from, wherever it was imported into.
pub async fn origins_of_objects(
    conn: &mut Conn,
    objects: &[Id],
) -> Result<Vec<OriginRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ORIGIN_COLUMNS} FROM import_origins WHERE object_id = ANY($1)"
    )))
    .bind(objects)
    .fetch_all(conn)
    .await?)
}

/// Writes what a thing of a source is here. A row that is there is written over: the thing that an earlier import
/// made keeps its maker unless that thing was deleted, in which case the new one is the maker.
pub async fn put_origin(
    conn: &mut Conn,
    row: &OriginRow,
    object_alive: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO import_origins (scope_id, source_id, kind, object_id, parent_id, created_by_import, last_import,
                source_name, source_path, source_sha256, source_updated_at, source_creator, imported_by, imported_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT (scope_id, source_id) DO UPDATE SET
            kind = EXCLUDED.kind,
            object_id = EXCLUDED.object_id,
            parent_id = EXCLUDED.parent_id,
            created_by_import = CASE
                WHEN $15 THEN COALESCE(import_origins.created_by_import, EXCLUDED.created_by_import)
                ELSE EXCLUDED.created_by_import END,
            last_import = EXCLUDED.last_import,
            source_name = EXCLUDED.source_name,
            source_path = EXCLUDED.source_path,
            source_sha256 = EXCLUDED.source_sha256,
            source_updated_at = EXCLUDED.source_updated_at,
            source_creator = EXCLUDED.source_creator,
            imported_by = EXCLUDED.imported_by,
            imported_at = EXCLUDED.imported_at",
    )
    .bind(row.scope_id)
    .bind(row.source_id)
    .bind(&row.kind)
    .bind(row.object_id)
    .bind(row.parent_id)
    .bind(row.created_by_import)
    .bind(row.last_import)
    .bind(&row.source_name)
    .bind(&row.source_path)
    .bind(&row.source_sha256)
    .bind(row.source_updated_at)
    .bind(&row.source_creator)
    .bind(row.imported_by)
    .bind(row.imported_at)
    .bind(object_alive)
    .execute(conn)
    .await?;
    Ok(())
}

/// Everything an import made, in the order it made it.
pub async fn made_by(conn: &mut Conn, import: Id) -> Result<Vec<OriginRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ORIGIN_COLUMNS} FROM import_origins WHERE created_by_import = $1 ORDER BY imported_at, source_id"
    )))
    .bind(import)
    .fetch_all(conn)
    .await?)
}

/// The rows an import made that are no object of their own (messages, turns, items, comments, versions) and lie in
/// `parent`.
pub async fn made_inside(
    conn: &mut Conn,
    import: Id,
    parent: Id,
    kind: &str,
) -> Result<Vec<OriginRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ORIGIN_COLUMNS} FROM import_origins
          WHERE created_by_import = $1 AND parent_id = $2 AND kind = $3 ORDER BY imported_at, source_id"
    )))
    .bind(import)
    .bind(parent)
    .bind(kind)
    .fetch_all(conn)
    .await?)
}

/// Forgets what the things `objects` were made from: once they are taken back, a later import makes them anew.
pub async fn forget_objects(conn: &mut Conn, import: Id, objects: &[Id]) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM import_origins WHERE created_by_import = $1 AND object_id = ANY($2)",
    )
    .bind(import)
    .bind(objects)
    .execute(conn)
    .await?
    .rows_affected())
}

// ---------------------------------------------------------------- what is here already

/// Which of the people `ids` have an account here.
pub async fn existing_users(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM users WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// The people whose confirmed address is one of `emails` (written in lower case): each address with the one who holds it.
pub async fn users_by_confirmed_email(
    conn: &mut Conn,
    emails: &[String],
) -> Result<Vec<(String, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT lower(email), id FROM users
          WHERE kind = 'human' AND email_verified_at IS NOT NULL AND lower(email) = ANY($1)",
    )
    .bind(emails)
    .fetch_all(conn)
    .await?)
}

/// The people whose username is one of `names` (written in lower case): each name with the one who holds it.
pub async fn users_by_username(
    conn: &mut Conn,
    names: &[String],
) -> Result<Vec<(String, Id)>, DbError> {
    Ok(
        sqlx::query_as(
            "SELECT username, id FROM users WHERE kind = 'human' AND username = ANY($1)",
        )
        .bind(names)
        .fetch_all(conn)
        .await?,
    )
}

/// Which of the things `ids` of a kind are here and were not deleted. A node of any kind is `nodes`; the other kinds are
/// their own tables.
pub async fn alive(conn: &mut Conn, table: Table, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    let sql = match table {
        Table::Nodes => "SELECT id FROM nodes WHERE id = ANY($1) AND deleted_at IS NULL",
        Table::Documents => "SELECT id FROM documents WHERE id = ANY($1) AND deleted_at IS NULL",
        Table::Containers => {
            "SELECT id FROM document_containers WHERE id = ANY($1) AND deleted_at IS NULL"
        }
        Table::Chats => {
            "SELECT c.id FROM chats c JOIN nodes n ON n.id = c.node_id
              WHERE c.id = ANY($1) AND n.deleted_at IS NULL"
        }
        Table::Messages => "SELECT id FROM chat_messages WHERE id = ANY($1) AND deleted_at IS NULL",
        Table::Rows => {
            "SELECT id FROM (SELECT id FROM items WHERE id = ANY($1) UNION ALL SELECT id FROM turns WHERE id = ANY($1)) r"
        }
    };
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// The tables whose rows an import makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// Places and threads.
    Nodes,
    /// Documents.
    Documents,
    /// Document folders.
    Containers,
    /// Chats of nodes.
    Chats,
    /// Messages of chats.
    Messages,
    /// Items and turns of threads.
    Rows,
}

/// A thing here that has the name of a thing of a source: its identifier, its name, and the name it was asked for by.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Sibling {
    /// Its identifier.
    pub id: Id,
    /// Its name here.
    pub name: String,
    /// The name of the thing of the source that it meets.
    pub asked: String,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

/// The nodes of `kind` that lie in `parent` and have one of `names`.
pub async fn sibling_nodes(
    conn: &mut Conn,
    parent: Id,
    kind: &str,
    names: &[String],
) -> Result<Vec<Sibling>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.name, k.name AS asked, n.updated_at
           FROM unnest($3::text[]) AS k(name)
           JOIN nodes n ON lower(n.name) = lower(k.name)
          WHERE n.parent_id = $1 AND n.kind = $2 AND n.deleted_at IS NULL",
    )
    .bind(parent)
    .bind(kind)
    .bind(names)
    .fetch_all(conn)
    .await?)
}

/// The statements of [`sibling_folders`], for the top of a place's documents and for a folder: `$1` the place, `$2` (for a folder) the folder,
/// then the names. The names are folded as the unique index of live names folds them (the letters of the alphabet only), so that the index
/// serves each name asked, and a place of a hundred thousand folders is not read to find a few.
/// The unique live-name key permits at most one match. Keeping that lookup
/// lateral prevents a rare input name from turning into a whole-directory join.
pub const SIBLING_FOLDERS_TOP_SQL: &str = "SELECT c.id, c.name, k.name AS asked, c.updated_at
           FROM unnest($2::text[]) AS k(name)
           JOIN LATERAL (
             SELECT c.* FROM document_containers c
              WHERE c.node_id = $1 AND c.parent_container_id IS NULL AND c.deleted_at IS NULL
                AND translate(c.name, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
                    = translate(k.name, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
              LIMIT 1
           ) c ON true";
/// As [`SIBLING_FOLDERS_TOP_SQL`], for the folders in a folder.
pub const SIBLING_FOLDERS_IN_SQL: &str = "SELECT c.id, c.name, k.name AS asked, c.updated_at
           FROM unnest($3::text[]) AS k(name)
           JOIN LATERAL (
             SELECT c.* FROM document_containers c
              WHERE c.node_id = $1 AND c.parent_container_id = $2 AND c.deleted_at IS NULL
                AND translate(c.name, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
                    = translate(k.name, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
              LIMIT 1
           ) c ON true";

/// The document folders of `node` that lie in `container` (the top of the node's documents where none) and have one of
/// `names`.
pub async fn sibling_folders(
    conn: &mut Conn,
    node: Id,
    container: Option<Id>,
    names: &[String],
) -> Result<Vec<Sibling>, DbError> {
    Ok(match container {
        None => {
            sqlx::query_as(SIBLING_FOLDERS_TOP_SQL)
                .bind(node)
                .bind(names)
                .fetch_all(conn)
                .await?
        }
        Some(container) => {
            sqlx::query_as(SIBLING_FOLDERS_IN_SQL)
                .bind(node)
                .bind(container)
                .bind(names)
                .fetch_all(conn)
                .await?
        }
    })
}

/// The statements of [`sibling_documents`], folded as the unique index of live names folds them (see [`SIBLING_FOLDERS_TOP_SQL`]).
pub const SIBLING_DOCUMENTS_TOP_SQL: &str = "SELECT d.id, d.title AS name, k.name AS asked, d.updated_at
           FROM unnest($2::text[]) AS k(name)
           JOIN LATERAL (
             SELECT d.* FROM documents d
              WHERE d.node_id = $1 AND d.parent_container_id IS NULL AND d.deleted_at IS NULL
                AND translate(d.basename, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
                    = translate(document_basename(k.name), 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
              LIMIT 1
           ) d ON true";
/// As [`SIBLING_DOCUMENTS_TOP_SQL`], for the documents in a folder.
pub const SIBLING_DOCUMENTS_IN_SQL: &str = "SELECT d.id, d.title AS name, k.name AS asked, d.updated_at
           FROM unnest($3::text[]) AS k(name)
           JOIN LATERAL (
             SELECT d.* FROM documents d
              WHERE d.node_id = $1 AND d.parent_container_id = $2 AND d.deleted_at IS NULL
                AND translate(d.basename, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
                    = translate(document_basename(k.name), 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')
              LIMIT 1
           ) d ON true";

/// The documents of `node` that lie in `container` (the top of the node's documents where none) and whose file name is
/// the one that a title of `titles` makes.
pub async fn sibling_documents(
    conn: &mut Conn,
    node: Id,
    container: Option<Id>,
    titles: &[String],
) -> Result<Vec<Sibling>, DbError> {
    Ok(match container {
        None => {
            sqlx::query_as(SIBLING_DOCUMENTS_TOP_SQL)
                .bind(node)
                .bind(titles)
                .fetch_all(conn)
                .await?
        }
        Some(container) => {
            sqlx::query_as(SIBLING_DOCUMENTS_IN_SQL)
                .bind(node)
                .bind(container)
                .bind(titles)
                .fetch_all(conn)
                .await?
        }
    })
}

/// An earlier version of a document, as a package kept it.
#[derive(Debug, Clone)]
pub struct NewVersion<'a> {
    /// The document it is a version of.
    pub document: Id,
    /// Its number there.
    pub version: i32,
    /// The title then.
    pub title: &'a str,
    /// The blocks then, as a version keeps them (`BlockRow`s).
    pub snapshot: &'a Json,
    /// The size of the text then.
    pub bytes: i64,
    /// Who changed the document before it was cut, as writers.
    pub authors: &'a Json,
    /// When it was cut.
    pub at: DateTime<Utc>,
}

/// Writes an earlier version of a document, cut for the reason `rewrite` (what a version says when somebody made it
/// by hand), held as the versions a document cuts are. A number the document already holds is left as it is. Returns
/// whether it was written.
pub async fn put_version(conn: &mut Conn, new: &NewVersion<'_>) -> Result<bool, DbError> {
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM document_versions WHERE document_id = $1 AND version = $2)",
    )
    .bind(new.document)
    .bind(new.version)
    .fetch_one(&mut *conn)
    .await?;
    if held {
        return Ok(false);
    }
    let blocks = new.snapshot.as_array().cloned().unwrap_or_default();
    let form =
        super::documents::history::held_form(conn, new.document, new.version, blocks).await?;
    let written = sqlx::query(
        "INSERT INTO document_versions (document_id, version, title, whole, base, change, stored_bytes, whole_bytes,
                                        block_count, top_blocks, bytes, cause, authors, changed, from_at, to_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'rewrite', $12, '{}'::jsonb, $13, $13)
         ON CONFLICT (document_id, version) DO NOTHING",
    )
    .bind(new.document)
    .bind(new.version)
    .bind(new.title)
    .bind(&form.whole)
    .bind(form.base)
    .bind(&form.change)
    .bind(form.stored_bytes)
    .bind(form.whole_bytes)
    .bind(form.block_count)
    .bind(form.top_blocks)
    .bind(new.bytes)
    .bind(new.authors)
    .bind(new.at)
    .execute(conn)
    .await?;
    Ok(written.rows_affected() == 1)
}

/// Lets the next version a document cuts follow the ones that were put in it: the counter never goes down.
pub async fn raise_version_counter(
    conn: &mut Conn,
    document: Id,
    version: i32,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE documents SET version = $2, version_at = COALESCE(version_at, $3) WHERE id = $1 AND version < $2",
    )
    .bind(document)
    .bind(version)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- what an import made, counted and paged

/// What the import made that is still there, counted by kind in a few statements and without loading a row: places and threads by their
/// kind, `document`, `folder`, and what it added to things that were there (`message`, `turn`, `item`, `comment`, `version`). What it wrote
/// in a place or a document it made itself is not counted again: it goes with what holds it.
pub async fn counts_made(conn: &mut Conn, import: Id) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.kind, count(*) FROM import_origins o JOIN nodes n ON n.id = o.object_id
          WHERE o.created_by_import = $1 AND o.kind IN ('org','project','topic','thread') AND n.deleted_at IS NULL
          GROUP BY n.kind
         UNION ALL
         SELECT 'document', count(*) FROM import_origins o JOIN documents d ON d.id = o.object_id
          WHERE o.created_by_import = $1 AND o.kind = 'document' AND d.deleted_at IS NULL
         HAVING count(*) > 0
         UNION ALL
         SELECT 'folder', count(*) FROM import_origins o JOIN document_containers c ON c.id = o.object_id
          WHERE o.created_by_import = $1 AND o.kind = 'folder' AND c.deleted_at IS NULL
         HAVING count(*) > 0
         UNION ALL
         SELECT 'message', count(*) FROM import_origins o JOIN chats c ON c.id = o.parent_id
          WHERE o.created_by_import = $1 AND o.kind = 'message'
            AND NOT EXISTS (SELECT 1 FROM import_origins p WHERE p.created_by_import = $1 AND p.object_id = c.node_id)
         HAVING count(*) > 0
         UNION ALL
         SELECT o.kind, count(*) FROM import_origins o
          WHERE o.created_by_import = $1 AND o.kind IN ('turn','item','comment','version')
            AND NOT EXISTS (SELECT 1 FROM import_origins p WHERE p.created_by_import = $1 AND p.object_id = o.parent_id)
          GROUP BY o.kind",
    )
    .bind(import)
    .fetch_all(conn)
    .await?)
}

/// A page of the chats (`kind` is `message`) or threads (`item`) that the import added things to, by identifier after `after`: where it
/// wrote messages or items that its taking back has to look for.
pub async fn parents_with_added(
    conn: &mut Conn,
    import: Id,
    kind: &str,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT o.parent_id FROM import_origins o
          WHERE o.created_by_import = $1 AND o.kind = $2 AND o.parent_id IS NOT NULL
            AND ($3::uuid IS NULL OR o.parent_id > $3)
          ORDER BY o.parent_id LIMIT $4",
    )
    .bind(import)
    .bind(kind)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// How many things lie in the places the import made that it did not make: what people added since, counted without loading: the places and
/// threads below them, the documents beside them, and the places whose chat or history holds anything written after `since` (the moment the
/// import began) that the import did not write.
pub async fn added_since(
    conn: &mut Conn,
    import: Id,
    since: DateTime<Utc>,
) -> Result<i64, DbError> {
    let (nodes, documents, written): (i64, i64, i64) = sqlx::query_as(
        "WITH tops AS (
             SELECT o.object_id AS id FROM import_origins o JOIN nodes n ON n.id = o.object_id
              WHERE o.created_by_import = $1 AND o.kind IN ('org','project','topic','thread') AND n.deleted_at IS NULL
                AND NOT EXISTS (SELECT 1 FROM import_origins p WHERE p.created_by_import = $1 AND p.object_id = n.parent_id))
         SELECT
           (SELECT count(DISTINCT n.id) FROM tops t JOIN nodes n ON n.path @> ARRAY[t.id] AND n.id <> t.id
             WHERE n.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM import_origins m WHERE m.created_by_import = $1 AND m.object_id = n.id)),
           (SELECT count(*) FROM import_origins t JOIN documents d ON d.node_id = t.object_id
             WHERE t.created_by_import = $1 AND t.kind IN ('org','project','topic','thread') AND d.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM import_origins m WHERE m.created_by_import = $1 AND m.object_id = d.id)),
           (SELECT count(*) FROM import_origins t JOIN nodes n ON n.id = t.object_id
             WHERE t.created_by_import = $1 AND t.kind IN ('org','project','topic','thread') AND n.deleted_at IS NULL
               AND (EXISTS (SELECT 1 FROM chat_messages m JOIN chats c ON c.id = m.chat_id
                             WHERE c.node_id = t.object_id AND m.deleted_at IS NULL AND m.created_at > $2
                               AND (m.client_id IS NULL OR m.client_id NOT LIKE 'import:%'))
                    OR EXISTS (SELECT 1 FROM items i WHERE i.thread_id = t.object_id AND i.created_at > $2
                                AND (i.native_ref IS NULL OR i.native_ref NOT LIKE 'import:%'))))",
    )
    .bind(import)
    .bind(since)
    .fetch_one(conn)
    .await?;
    Ok(nodes + documents + written)
}

/// How many documents the import made have been changed since by somebody other than `user`.
pub async fn edited_since(conn: &mut Conn, import: Id, user: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM import_origins o JOIN documents d ON d.id = o.object_id
          WHERE o.created_by_import = $1 AND o.kind = 'document' AND d.deleted_at IS NULL
            AND d.updated_at > o.imported_at
            AND (d.updated_by IS DISTINCT FROM $2
                 OR EXISTS (SELECT 1 FROM document_blocks b WHERE b.document_id = d.id AND b.updated_at > o.imported_at
                             AND (b.updated_by IS DISTINCT FROM $2 OR b.updated_by_thread IS NOT NULL)))",
    )
    .bind(import)
    .bind(user)
    .fetch_one(conn)
    .await?)
}

/// The first-page statement of [`nodes_page`]: `$1` the import, `$2` and `$3`
/// both null, `$4` how many. Subsequent pages use [`NODES_PAGE_AFTER_SQL`].
pub const NODES_PAGE_SQL: &str = "SELECT object_id, source_name, node_depth FROM import_origins
          WHERE created_by_import = $1 AND node_depth IS NOT NULL
            AND $2::int IS NULL AND $3::uuid IS NULL
          ORDER BY node_depth DESC, object_id DESC LIMIT $4";

/// As [`NODES_PAGE_SQL`], seeking directly after a preceding page even in a
/// cached generic plan. The depth follows current hierarchy changes in storage.
pub const NODES_PAGE_AFTER_SQL: &str =
    "SELECT object_id, source_name, node_depth FROM import_origins
          WHERE created_by_import = $1 AND node_depth IS NOT NULL
            AND (node_depth, object_id) < ($2::int, $3::uuid)
          ORDER BY node_depth DESC, object_id DESC LIMIT $4";

/// A page of the places and threads the import made that are still there, the deepest first and by identifier within a depth: what lies in
/// a place is looked at before the place is, so that one that still holds what people added since can be left where it is. Each comes
/// with the name it had there and its depth.
pub async fn nodes_page(
    conn: &mut Conn,
    import: Id,
    after: Option<(i32, Id)>,
    limit: i64,
) -> Result<Vec<(Id, Option<String>, i32)>, DbError> {
    Ok(sqlx::query_as(if after.is_some() {
        NODES_PAGE_AFTER_SQL
    } else {
        NODES_PAGE_SQL
    })
    .bind(import)
    .bind(after.map(|(depth, _)| depth))
    .bind(after.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The statement of [`node_is_clear`]: `$1` the place, `$2` the moment the import began. A place that is below a place is below it through
/// its parent, and a deleted place takes what lies below it down with it, so whether anything below is still there is whether a place whose
/// parent it is is: asked through the index of parents. (Asked of every place whose path holds this one it read the whole table of places.)
pub const NODE_IS_CLEAR_SQL: &str = "SELECT NOT EXISTS (SELECT 1 FROM nodes d WHERE d.parent_id = $1 AND d.deleted_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM documents x WHERE x.node_id = $1 AND x.deleted_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM document_containers f WHERE f.node_id = $1 AND f.deleted_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM chat_messages m JOIN chats c ON c.id = m.chat_id
                             WHERE c.node_id = $1 AND m.deleted_at IS NULL AND m.created_at > $2
                               AND (m.client_id IS NULL OR m.client_id NOT LIKE 'import:%'))
            AND NOT EXISTS (SELECT 1 FROM items i WHERE i.thread_id = $1 AND i.created_at > $2
                               AND (i.native_ref IS NULL OR i.native_ref NOT LIKE 'import:%'))";

/// Whether nothing is left in the place `node` that its deletion would take with it and that is not the import's to take back: no place
/// or thread below it that is still there, no document or folder beside it, no message of its chat written after `since` (the moment the
/// import began, since what it brought keeps the times it had there) and no item of its history written after it.
pub async fn node_is_clear(
    conn: &mut Conn,
    node: Id,
    since: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(NODE_IS_CLEAR_SQL)
        .bind(node)
        .bind(since)
        .fetch_one(conn)
        .await?)
}

/// Which of the documents `ids`, made by the import, have been changed since by somebody other than `user`: their work is in them.
pub async fn edited_by_others(
    conn: &mut Conn,
    import: Id,
    ids: &[Id],
    user: Id,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT d.id FROM documents d JOIN import_origins o ON o.object_id = d.id AND o.created_by_import = $1
          WHERE d.id = ANY($2) AND d.updated_at > o.imported_at
            AND (d.updated_by IS DISTINCT FROM $3
                 OR EXISTS (SELECT 1 FROM document_blocks b WHERE b.document_id = d.id AND b.updated_at > o.imported_at
                             AND (b.updated_by IS DISTINCT FROM $3 OR b.updated_by_thread IS NOT NULL)))",
    )
    .bind(import)
    .bind(ids)
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// The statement of [`empty_folders`]: `$1` the folders. What lies in a folder lies in the same place, so the lookups name it and are read through the
/// indexes that begin with the place.
pub const EMPTY_FOLDERS_SQL: &str = "SELECT c.id FROM document_containers c
          WHERE c.id = ANY($1)
            AND NOT EXISTS (SELECT 1 FROM documents d WHERE d.node_id = c.node_id AND d.parent_container_id = c.id AND d.deleted_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM document_containers s WHERE s.node_id = c.node_id AND s.parent_container_id = c.id AND s.deleted_at IS NULL)";

/// Which of the folders `ids` hold nothing now: no document and no folder that is still there.
pub async fn empty_folders(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(EMPTY_FOLDERS_SQL)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// The statement of [`made_page`] for documents: `$1` the import, `$2` where to go on from (or null), `$3` how many.
pub const DOCUMENTS_PAGE_SQL: &str =
    "SELECT o.object_id FROM import_origins o JOIN documents d ON d.id = o.object_id
          WHERE o.created_by_import = $1 AND o.kind = 'document' AND d.deleted_at IS NULL
            AND ($2::uuid IS NULL OR o.object_id > $2)
          ORDER BY o.object_id LIMIT $3";

/// The statement of [`made_page`] for document folders.
pub const FOLDERS_PAGE_SQL: &str =
    "SELECT o.object_id FROM import_origins o JOIN document_containers c ON c.id = o.object_id
          WHERE o.created_by_import = $1 AND o.kind = 'folder' AND c.deleted_at IS NULL
            AND ($2::uuid IS NULL OR o.object_id > $2)
          ORDER BY o.object_id LIMIT $3";

/// A page of the documents (`document`) or document folders (`folder`) the import made that are still there, by identifier after `after`.
pub async fn made_page(
    conn: &mut Conn,
    import: Id,
    kind: &str,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    let sql = if kind == "folder" {
        FOLDERS_PAGE_SQL
    } else {
        DOCUMENTS_PAGE_SQL
    };
    Ok(sqlx::query_scalar(sql)
        .bind(import)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}
