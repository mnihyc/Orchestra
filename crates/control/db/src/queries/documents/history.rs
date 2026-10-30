//! The versions of a document: cut, listed, read, kept within a number and a size, and cut for a document that was left alone.
//!
//! A version is the live blocks of the document whole at a moment, with the
//! tally of what was changed between the one before and it, and by whom. The
//! document row keeps the open tally; cutting a version writes it into the new
//! row and starts a new one.
//!
//! A version is held whole, or as the change from an older version, its base:
//! the blocks that are new or different since, whole, and the identifiers of
//! the blocks that are gone. It is held as a change when that change and the
//! changes its base is rebuilt with come to less than its blocks whole, so that
//! reading any version reads at most about twice its size, and a history of
//! small edits to a large document holds each edit once. When a version goes,
//! a kept version that needs it is held whole again first.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use orch_api_contract::documents::{DocumentFileChange, VersionCause};
use orch_documents::position::Position;
use orch_documents::versions::{self, Writer};
use orch_types::{Id, Json, Time};

use super::{BlockRow, DocumentRow};
use crate::{Conn, DbError};

/// A version, without the blocks it holds.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct VersionRow {
    /// Its number.
    pub version: i32,
    /// The title then.
    pub title: String,
    /// The size of the text then.
    pub bytes: i64,
    /// Why it was cut.
    pub cause: String,
    /// Who changed the document between the version before and this one.
    pub authors: Json,
    /// How many blocks they inserted, updated, moved and removed.
    pub changed: Json,
    /// The first change of the span.
    pub from_at: DateTime<Utc>,
    /// The last change of the span.
    pub to_at: DateTime<Utc>,
    /// How many blocks it holds.
    pub block_count: i64,
}

const VERSION_COLUMNS: &str =
    "v.version, v.title, v.bytes, v.cause, v.authors, v.changed, v.from_at, v.to_at,
    v.block_count";

/// Records a named file change in the same version window as text. The caller holds the document
/// lock and commits the file, its pins and this history together; nothing outside SQL is awaited.
pub async fn record_file(
    conn: &mut Conn,
    document: Id,
    writer: &Writer,
    file: DocumentFileChange,
    rules: super::batch::Rules,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let Some(row) = super::get(conn, document).await? else {
        return Ok(());
    };
    let mut span = row.span();
    if let Some(cause) = versions::cut_before_a_batch(
        &span,
        row.version_at.map(Time::new),
        Time::new(row.created_at),
        Time::new(now),
        rules.window_secs,
        false,
    ) {
        cut(conn, &row, cause, now).await?;
        span = versions::Span::default();
    }
    span.record_file(writer, file, Time::new(now));
    super::keep_span(conn, document, &span).await?;
    prune(conn, document, rules.kept, rules.history_max_bytes).await?;
    Ok(())
}

/// The word that says why a version was cut.
#[must_use]
pub fn cause_word(cause: VersionCause) -> &'static str {
    match cause {
        VersionCause::Window => "window",
        VersionCause::Idle => "idle",
        VersionCause::Rewrite => "rewrite",
        VersionCause::Restore => "restore",
    }
}

/// Keeps the document as it stands as its next version, and starts a new tally
/// of what is changed from here. The document row is the one read under the
/// lock, before the change that is about to be made. Returns the number.
pub async fn cut(
    conn: &mut Conn,
    doc: &DocumentRow,
    cause: VersionCause,
    now: DateTime<Utc>,
) -> Result<i32, DbError> {
    let blocks = super::live_blocks(conn, doc.id).await?;
    let rows: Vec<Json> = blocks
        .iter()
        .filter_map(|block| serde_json::to_value(block).ok())
        .collect();
    let span = doc.span();
    let number = doc.version + 1;
    let held = held_form(conn, doc.id, number, rows).await?;
    let authors: Vec<Json> = span.authors.iter().map(Writer::to_json).collect();
    sqlx::query(
        "INSERT INTO document_versions (document_id, version, title, whole, base, change, stored_bytes, whole_bytes,
                                        block_count, top_blocks, bytes, cause, authors, changed, from_at, to_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(doc.id)
    .bind(number)
    .bind(&doc.title)
    .bind(&held.whole)
    .bind(held.base)
    .bind(&held.change)
    .bind(held.stored_bytes)
    .bind(held.whole_bytes)
    .bind(held.block_count)
    .bind(held.top_blocks)
    .bind(doc.bytes)
    .bind(cause_word(cause))
    .bind(Json::Array(authors))
    .bind(serde_json::json!({
        "inserted": span.tally.inserted,
        "updated": span.tally.updated,
        "moved": span.tally.moved,
        "removed": span.tally.removed,
        "files": span.files,
    }))
    .bind(span.from_at.map_or(now, Time::get))
    .bind(span.to_at.map_or(now, Time::get))
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE documents SET version = $2, version_at = $3, span = '{}'::jsonb WHERE id = $1",
    )
    .bind(doc.id)
    .bind(number)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(number)
}

/// How the blocks of a version are stored.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HeldForm {
    /// The blocks whole, when it is held whole.
    pub whole: Option<Json>,
    /// The version it is the change from, when it is held as one.
    pub base: Option<i32>,
    /// The change from its base: `{"put": [blocks], "gone": [identifiers]}`.
    pub change: Option<Json>,
    /// The size of what is stored, as JSON text.
    pub stored_bytes: i64,
    /// The size of its blocks whole, as JSON text.
    pub whole_bytes: i64,
    /// How many blocks it holds.
    pub block_count: i64,
    /// How many of them stand at the top of the page.
    pub top_blocks: i64,
}

fn json_size(value: &Json) -> i64 {
    i64::try_from(value.to_string().len()).unwrap_or(i64::MAX)
}

fn block_id(block: &Json) -> Option<&str> {
    block.get("id").and_then(Json::as_str)
}

/// The order `live_blocks` reads blocks in: by position as the number it is, then by identifier.
fn page_order(a: &Json, b: &Json) -> Ordering {
    let place = |block: &Json| {
        block
            .get("position")
            .and_then(Json::as_str)
            .and_then(Position::parse)
    };
    place(a)
        .cmp(&place(b))
        .then_with(|| block_id(a).cmp(&block_id(b)))
}

/// What changed from `before` to `after`: the blocks that are new or different, whole, and the blocks that are gone.
fn change_between(before: &[Json], after: &[Json]) -> Json {
    let earlier: HashMap<&str, &Json> = before
        .iter()
        .filter_map(|block| Some((block_id(block)?, block)))
        .collect();
    let put: Vec<Json> = after
        .iter()
        .filter(|block| {
            block_id(block)
                .and_then(|id| earlier.get(id))
                .is_none_or(|was| *was != *block)
        })
        .cloned()
        .collect();
    let now: HashSet<&str> = after.iter().filter_map(block_id).collect();
    let gone: Vec<Json> = before
        .iter()
        .filter_map(block_id)
        .filter(|id| !now.contains(id))
        .map(|id| Json::String(id.to_owned()))
        .collect();
    serde_json::json!({ "put": put, "gone": gone })
}

/// The blocks after `change`, in the page's order.
fn applied(before: Vec<Json>, change: &Json) -> Vec<Json> {
    let gone: HashSet<&str> = change["gone"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    let put: Vec<Json> = change["put"].as_array().cloned().unwrap_or_default();
    let replaced: HashSet<String> = put
        .iter()
        .filter_map(|block| block_id(block).map(str::to_owned))
        .collect();
    let mut blocks: Vec<Json> = before
        .into_iter()
        .filter(|block| {
            block_id(block).is_some_and(|id| !gone.contains(id) && !replaced.contains(id))
        })
        .collect();
    blocks.extend(put);
    blocks.sort_by(page_order);
    blocks
}

/// The blocks of a version, rebuilt from the whole version it goes back to and
/// the changes after it, with the size of those changes. None when the
/// document holds no such version.
async fn rebuilt(
    conn: &mut Conn,
    document: Id,
    version: i32,
) -> Result<Option<(Vec<Json>, i64)>, DbError> {
    let chain: Vec<(Option<Json>, Option<Json>, i64)> = sqlx::query_as(
        "WITH RECURSIVE chain AS (
            SELECT v.base, v.whole, v.change, v.stored_bytes, 0 AS depth
              FROM document_versions v WHERE v.document_id = $1 AND v.version = $2
            UNION ALL
            SELECT v.base, v.whole, v.change, v.stored_bytes, c.depth + 1
              FROM chain c JOIN document_versions v ON v.document_id = $1 AND v.version = c.base
         )
         SELECT whole, change, stored_bytes FROM chain ORDER BY depth DESC",
    )
    .bind(document)
    .bind(version)
    .fetch_all(conn)
    .await?;
    let mut rows = chain.into_iter();
    let Some((Some(Json::Array(mut blocks)), _, _)) = rows.next() else {
        return Ok(None);
    };
    let mut changes = 0_i64;
    for (_, change, stored) in rows {
        if let Some(change) = change {
            blocks = applied(blocks, &change);
            changes = changes.saturating_add(stored);
        }
    }
    Ok(Some((blocks, changes)))
}

/// How the blocks of a new version `number` are stored: as the change from the
/// newest version before it, when that change and the changes that version is
/// rebuilt with come to less than the blocks whole; otherwise whole.
pub(crate) async fn held_form(
    conn: &mut Conn,
    document: Id,
    number: i32,
    mut blocks: Vec<Json>,
) -> Result<HeldForm, DbError> {
    blocks.sort_by(page_order);
    let block_count = i64::try_from(blocks.len()).unwrap_or(i64::MAX);
    let top_blocks = i64::try_from(
        blocks
            .iter()
            .filter(|block| block.get("parent_id").is_none_or(Json::is_null))
            .count(),
    )
    .unwrap_or(i64::MAX);
    let base: Option<i32> = sqlx::query_scalar(
        "SELECT max(version) FROM document_versions WHERE document_id = $1 AND version < $2",
    )
    .bind(document)
    .bind(number)
    .fetch_one(&mut *conn)
    .await?;
    let whole = Json::Array(blocks);
    let whole_bytes = json_size(&whole);
    if let Some(base) = base
        && let Some((before, changes)) = rebuilt(conn, document, base).await?
    {
        let change = change_between(&before, whole.as_array().map_or(&[], Vec::as_slice));
        let change_bytes = json_size(&change);
        if changes.saturating_add(change_bytes) < whole_bytes {
            return Ok(HeldForm {
                whole: None,
                base: Some(base),
                change: Some(change),
                stored_bytes: change_bytes,
                whole_bytes,
                block_count,
                top_blocks,
            });
        }
    }
    Ok(HeldForm {
        whole: Some(whole),
        base: None,
        change: None,
        stored_bytes: whole_bytes,
        whole_bytes,
        block_count,
        top_blocks,
    })
}

/// Removes the oldest versions beyond the number that is kept and beyond the
/// size they may come to together, never the newest. A kept version held as
/// the change from one that goes is held whole first. Returns which went.
pub async fn prune(
    conn: &mut Conn,
    document: Id,
    kept: i64,
    max_bytes: i64,
) -> Result<Vec<i32>, DbError> {
    let held: Vec<(i32, i64, i64, Option<i32>)> = sqlx::query_as(
        "SELECT version, stored_bytes, whole_bytes, base FROM document_versions
          WHERE document_id = $1 ORDER BY version DESC",
    )
    .bind(document)
    .fetch_all(&mut *conn)
    .await?;
    let sizes: Vec<versions::Held> = held
        .iter()
        .map(|&(number, stored, whole, base)| versions::Held {
            number: i64::from(number),
            stored,
            whole,
            base: base.map(i64::from),
        })
        .collect();
    let dropped: Vec<i32> = versions::dropped_held(&sizes, kept, max_bytes)
        .into_iter()
        .filter_map(|version| i32::try_from(version).ok())
        .collect();
    if !dropped.is_empty() {
        let going: HashSet<i32> = dropped.iter().copied().collect();
        for &(version, _, _, base) in &held {
            if going.contains(&version) || !base.is_some_and(|base| going.contains(&base)) {
                continue;
            }
            if let Some((blocks, _)) = rebuilt(conn, document, version).await? {
                let whole = Json::Array(blocks);
                let size = json_size(&whole);
                sqlx::query(
                    "UPDATE document_versions SET whole = $3, base = NULL, change = NULL, stored_bytes = $4
                      WHERE document_id = $1 AND version = $2",
                )
                .bind(document)
                .bind(version)
                .bind(&whole)
                .bind(size)
                .execute(&mut *conn)
                .await?;
            }
        }
        sqlx::query("DELETE FROM document_versions WHERE document_id = $1 AND version = ANY($2)")
            .bind(document)
            .bind(&dropped)
            .execute(conn)
            .await?;
    }
    Ok(dropped)
}

/// What an entry of the history is narrowed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByAuthor {
    /// The entries a person wrote in.
    User(Id),
    /// The entries the agent of a thread wrote in.
    Thread(Id),
}

/// The entries of a document's history, newest first, below the version
/// `before` where that is given, at most `limit`.
pub async fn list(
    conn: &mut Conn,
    document: Id,
    author: Option<ByAuthor>,
    before: Option<i32>,
    limit: i64,
) -> Result<Vec<VersionRow>, DbError> {
    let contains = author.map(|author| match author {
        ByAuthor::User(user) => serde_json::json!([{ "user": user }]),
        ByAuthor::Thread(thread) => serde_json::json!([{ "thread": thread }]),
    });
    // Keep a supplied cursor in the index condition even in a generic prepared plan.
    let cursor_filter = if before.is_some() {
        "v.version < $3"
    } else {
        "$3::int IS NULL"
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VERSION_COLUMNS} FROM document_versions v
          WHERE v.document_id = $1 AND ($2::jsonb IS NULL OR v.authors @> $2::jsonb)
            AND {cursor_filter}
          ORDER BY v.version DESC LIMIT $4"
    )))
    .bind(document)
    .bind(contains)
    .bind(before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// One version of a document.
pub async fn get(
    conn: &mut Conn,
    document: Id,
    version: i32,
) -> Result<Option<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VERSION_COLUMNS} FROM document_versions v WHERE v.document_id = $1 AND v.version = $2"
    )))
    .bind(document)
    .bind(version)
    .fetch_optional(conn)
    .await?)
}

/// The blocks a version holds, in the order of the page's siblings.
pub async fn snapshot(
    conn: &mut Conn,
    document: Id,
    version: i32,
) -> Result<Option<Vec<BlockRow>>, DbError> {
    Ok(rebuilt(conn, document, version).await?.map(|(blocks, _)| {
        blocks
            .into_iter()
            .filter_map(|block| serde_json::from_value(block).ok())
            .collect()
    }))
}

/// The last version whose changes ended before `seen`: the one cut before a
/// reader last had the document open. None for a reader who never did, or
/// when no version ends that early.
pub async fn last_before(
    conn: &mut Conn,
    document: Id,
    seen: DateTime<Utc>,
) -> Result<Option<i32>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT max(version) FROM document_versions WHERE document_id = $1 AND to_at <= $2",
    )
    .bind(document)
    .bind(seen)
    .fetch_one(conn)
    .await?)
}

/// Who changed the document between the version `from` and the version `to`
/// (the document as it stands when none), in the order they began: the
/// authors of the entries after the first, up to the second, and of the open
/// span when the second is the present.
pub async fn authors_between(
    conn: &mut Conn,
    document: Id,
    from: i32,
    to: Option<i32>,
) -> Result<Vec<Writer>, DbError> {
    let entries: Vec<Json> = sqlx::query_scalar(
        "SELECT authors FROM document_versions
          WHERE document_id = $1 AND version > $2 AND ($3::int IS NULL OR version <= $3)
          ORDER BY version",
    )
    .bind(document)
    .bind(from)
    .bind(to)
    .fetch_all(&mut *conn)
    .await?;
    let mut out: Vec<Writer> = Vec::new();
    for entry in entries {
        for author in entry
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Writer::from_json)
        {
            if !out.contains(&author) {
                out.push(author);
            }
        }
    }
    if to.is_none()
        && let Some(doc) = super::get(conn, document).await?
    {
        for author in doc.span().authors {
            if !out.contains(&author) {
                out.push(author);
            }
        }
    }
    Ok(out)
}

/// The documents that changed since their last version and that nobody has
/// written in for `idle_secs`: the candidates for a version cut because they
/// were left alone.
pub async fn left_alone(
    conn: &mut Conn,
    now: DateTime<Utc>,
    idle_secs: i64,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM documents
          WHERE deleted_at IS NULL AND span ? 'to_at'
            AND (span->>'to_at')::timestamptz <= $1 - ($2::bigint * interval '1 second')
          ORDER BY (span->>'to_at')::timestamptz LIMIT $3",
    )
    .bind(now)
    .bind(idle_secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Cuts a version of the document if it still was changed and left alone for
/// `idle_secs`, which is looked at again under its lock. Returns the number.
pub async fn cut_if_left_alone(
    conn: &mut Conn,
    document: Id,
    now: DateTime<Utc>,
    idle_secs: i64,
) -> Result<Option<i32>, DbError> {
    let Some(doc) = super::lock(conn, document)
        .await?
        .filter(|doc| doc.deleted_at.is_none())
    else {
        return Ok(None);
    };
    if !versions::cut_when_left_alone(&doc.span(), Time::new(now), idle_secs) {
        return Ok(None);
    }
    Ok(Some(cut(conn, &doc, VersionCause::Idle, now).await?))
}

#[cfg(test)]
mod tests {
    use super::super::batch::{self, Outcome};
    use super::super::testing::{LIMITS, RULES, edit, place, put, write};
    use super::*;
    use chrono::Duration;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    #[tokio::test]
    async fn a_version_is_cut_before_a_batch_that_comes_after_the_window_and_not_before_it() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let here = place(&mut conn, start).await;
        let first = new_id();
        let Outcome::Applied(done) = write(
            &mut conn,
            here.document,
            here.ada,
            &[put(first, "one")],
            start,
        )
        .await
        else {
            panic!("applied");
        };
        assert_eq!(
            done.cut, None,
            "a document that was just made is not yet worth a version"
        );
        let soon = start + Duration::seconds(600);
        let Outcome::Applied(done) = write(
            &mut conn,
            here.document,
            here.bob,
            &[edit(first, done.seq, "two")],
            soon,
        )
        .await
        else {
            panic!("applied");
        };
        assert_eq!(
            done.cut, None,
            "ten minutes exactly is not longer than the window"
        );
        assert_eq!(
            super::super::get(&mut conn, here.document)
                .await
                .unwrap()
                .unwrap()
                .version,
            0
        );
        let later = start + Duration::seconds(601);
        let Outcome::Applied(done) = write(
            &mut conn,
            here.document,
            here.ada,
            &[edit(first, done.seq, "three")],
            later,
        )
        .await
        else {
            panic!("applied");
        };
        assert_eq!(done.cut, Some(VersionCause::Window));
        let doc = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(doc.version, 1);
        let kept = get(&mut conn, here.document, 1).await.unwrap().unwrap();
        assert_eq!(kept.cause, "window");
        assert_eq!(kept.block_count, 1);
        assert_eq!(kept.changed["inserted"], 1);
        assert_eq!(
            kept.changed["updated"], 1,
            "what was written between: the first update"
        );
        let snapshot = snapshot(&mut conn, here.document, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(
            snapshot[0].content.to_string().contains("two"),
            "the state before the batch that cut it"
        );
        // The open span now holds only the batch that followed.
        let span = doc.span();
        assert_eq!(span.tally.updated, 1);
        assert_eq!(span.tally.inserted, 0);
        let authors = kept.authors.as_array().unwrap();
        assert_eq!(
            authors.len(),
            2,
            "both people who wrote before the cut are in its entry"
        );
    }

    #[tokio::test]
    async fn a_document_left_alone_is_cut_after_the_idle_time_and_one_that_is_still_written_is_not()
    {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let here = place(&mut conn, start).await;
        let id = new_id();
        write(&mut conn, here.document, here.ada, &[put(id, "x")], start).await;
        assert!(
            left_alone(&mut conn, start + Duration::seconds(119), 120, 10)
                .await
                .unwrap()
                .is_empty()
        );
        let found = left_alone(&mut conn, start + Duration::seconds(120), 120, 10)
            .await
            .unwrap();
        assert_eq!(found, [here.document]);
        let number = cut_if_left_alone(
            &mut conn,
            here.document,
            start + Duration::seconds(120),
            120,
        )
        .await
        .unwrap();
        assert_eq!(number, Some(1));
        let kept = get(&mut conn, here.document, 1).await.unwrap().unwrap();
        assert_eq!(kept.cause, "idle");
        assert!(
            left_alone(&mut conn, start + Duration::hours(1), 120, 10)
                .await
                .unwrap()
                .is_empty(),
            "nothing changed since"
        );
        assert_eq!(
            cut_if_left_alone(&mut conn, here.document, start + Duration::hours(1), 120)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn the_oldest_versions_go_first_and_the_newest_stays() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let here = place(&mut conn, start).await;
        let id = new_id();
        let mut seq = 0;
        let mut at = start;
        let Outcome::Applied(done) =
            write(&mut conn, here.document, here.ada, &[put(id, "v0")], at).await
        else {
            panic!();
        };
        seq = seq.max(done.seq);
        for round in 1..=5 {
            at += Duration::seconds(700);
            let Outcome::Applied(done) = write(
                &mut conn,
                here.document,
                here.ada,
                &[edit(id, seq, &format!("v{round}"))],
                at,
            )
            .await
            else {
                panic!();
            };
            seq = done.seq;
        }
        let doc = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(doc.version, 5);
        let dropped = prune(&mut conn, here.document, 3, i64::MAX).await.unwrap();
        assert_eq!(dropped, [2, 1]);
        let left: Vec<i32> = list(&mut conn, here.document, None, None, 50)
            .await
            .unwrap()
            .iter()
            .map(|v| v.version)
            .collect();
        assert_eq!(left, [5, 4, 3]);
        let dropped = prune(&mut conn, here.document, 3, 1).await.unwrap();
        assert_eq!(
            dropped,
            [4, 3],
            "the newest is never removed, however little room there is"
        );
    }

    #[tokio::test]
    async fn the_history_is_kept_to_the_entries_one_author_wrote_in() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let here = place(&mut conn, start).await;
        let id = new_id();
        let Outcome::Applied(done) =
            write(&mut conn, here.document, here.ada, &[put(id, "a")], start).await
        else {
            panic!()
        };
        let at = start + Duration::seconds(700);
        let Outcome::Applied(done) = write(
            &mut conn,
            here.document,
            here.bob,
            &[edit(id, done.seq, "b")],
            at,
        )
        .await
        else {
            panic!()
        };
        let thread = new_id();
        crate::queries::hierarchy::testing::insert_node(
            &mut conn,
            thread,
            "thread",
            Some(here.topic),
            "Fix",
            here.ada,
        )
        .await
        .unwrap();
        let agent = Writer::Agent {
            thread,
            agent: "codex".into(),
        };
        let at = at + Duration::seconds(700);
        let Outcome::Applied(done) = batch::apply(
            &mut conn,
            here.document,
            &agent,
            None,
            &[edit(id, done.seq, "c")],
            LIMITS,
            RULES,
            at,
        )
        .await
        .unwrap() else {
            panic!()
        };
        let at = at + Duration::seconds(700);
        write(
            &mut conn,
            here.document,
            here.ada,
            &[edit(id, done.seq, "d")],
            at,
        )
        .await;
        // Each entry names the people who wrote before it was cut; the open span names the last.
        for (author, expected) in [
            (ByAuthor::User(here.ada), vec![1]),
            (ByAuthor::User(here.bob), vec![2]),
            (ByAuthor::Thread(thread), vec![3]),
        ] {
            let found: Vec<i32> = list(&mut conn, here.document, Some(author), None, 50)
                .await
                .unwrap()
                .into_iter()
                .map(|v| v.version)
                .collect();
            assert_eq!(found, expected, "{author:?}");
        }
    }
}
