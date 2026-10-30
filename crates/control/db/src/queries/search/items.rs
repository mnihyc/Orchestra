//! The newest matching items of the searched conversations, chosen without reading a history whole.
//!
//! Matching an item reads its whole text, and a long conversation holds far more text than any page shows.
//! The paged search therefore chooses its items before it ranks every source together, the way the word
//! windows choose theirs ([`super::candidates`]): the newest rows in recency order until enough match; when
//! they match rarely, the text index alone, if it finds no more than the probe allows; otherwise the recency
//! order again past the sample, until enough match. Each read stops at what it needs, and the result is
//! exactly the newest matches of each viewed history, with its continuations and retained spans.

use super::{Content, bind_content, item_filter, query_of};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// How many matches are wanted and how far each read may go.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// The newest matches wanted after the position, or the pool best matches are ranked in.
    pub wanted: i64,
    /// Rows read in recency order before the text index is asked (`search.word_recent_candidates`).
    pub recent: i64,
    /// Matches the text index may return before the words count as common (`search.word_probe_candidates`).
    pub probe: i64,
}

/// Every row a search can match, in the words of the item search indexes' own predicates.
const SEARCHABLE: &str =
    "i.search_text IS NOT NULL AND i.kind<>'marker' AND i.search_text NOT LIKE '%<orch>%'";
/// After the position: the cursor's moment and identity, or the oldest row already read.
const PAST: &str = "($38::timestamptz IS NULL OR (i.created_at,i.id)<($38,$39::uuid))";
/// The shortest word a trigram index can look up; shorter words leave it nothing to search by.
const TRIGRAM: usize = 3;

/// The newest matching items after `after` (moment and identity), newest first, at most `bounds.wanted`.
pub async fn newest(
    conn: &mut Conn,
    ask: &Content<'_>,
    after: Option<(DateTime<Utc>, Id)>,
    bounds: Bounds,
) -> Result<Vec<Id>, DbError> {
    if ask.item_kinds.is_empty() || bounds.wanted < 1 {
        return Ok(Vec::new());
    }
    let query = query_of(ask);
    let history = format!(
        "WITH {}, {}, history_owners AS MATERIALIZED (SELECT * FROM conversation_owners) ",
        include_str!("../search_candidate_visibility.sql"),
        include_str!("../search_history.sql"),
    );
    let threads: Vec<Id> = bind_content(
        sqlx::query_as::<_, (Id,)>(sqlx::AssertSqlSafe(format!(
            "{history}SELECT DISTINCT thread_id FROM history_owners"
        )))
        .persistent(false),
        ask,
        &query,
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(thread,)| thread)
    .collect();
    if threads.is_empty() {
        return Ok(Vec::new());
    }
    // One conversation reads its own recency index; several read the installation's in one order.
    let within = if threads.len() == 1 {
        "i.thread_id=($37::uuid[])[1]"
    } else {
        "i.thread_id=ANY($37::uuid[])"
    };
    // A correlated test, kept out of the join order, so that each read keeps its index order and stops.
    let matches = format!(
        "EXISTS(SELECT 1 FROM history_owners o WHERE o.thread_id=i.thread_id AND o.owner=i.continuation_id AND {} OFFSET 0)",
        item_filter(ask)
    );
    let recent = bounds.recent.max(bounds.wanted);
    let read = |sql: String, position: Option<(DateTime<Utc>, Id)>, limit: i64| {
        bind_content(
            sqlx::query_as::<_, (Id, DateTime<Utc>)>(sqlx::AssertSqlSafe(sql)).persistent(false),
            ask,
            &query,
        )
        .bind(threads.as_slice())
        .bind(position.map(|(at, _)| at))
        .bind(position.map(|(_, id)| id))
        .bind(recent)
        .bind(limit)
    };
    let settings: (String, String, String) = sqlx::query_as(
        "SELECT current_setting('enable_seqscan'),current_setting('enable_indexscan'),current_setting('enable_bitmapscan')",
    )
    .fetch_one(&mut *conn)
    .await?;
    let plan = |index: bool| {
        sqlx::query(
            "SELECT set_config('enable_seqscan','off',true),set_config('enable_indexscan',$1,true),set_config('enable_bitmapscan',$2,true)",
        )
        .bind(if index { "on" } else { "off" })
        .bind(if index { "off" } else { "on" })
    };

    // The newest rows in recency order, until enough of them match.
    plan(true).execute(&mut *conn).await?;
    let mut found = read(
        format!(
            "{history}SELECT i.id,i.created_at FROM (SELECT i.* FROM items i WHERE {within} AND {SEARCHABLE} AND {PAST}
               ORDER BY i.created_at DESC,i.id DESC LIMIT $40) i
             WHERE {matches} ORDER BY i.created_at DESC,i.id DESC LIMIT $41"
        ),
        after,
        bounds.wanted,
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut past = None;
    if i64::try_from(found.len()).unwrap_or(i64::MAX) < bounds.wanted {
        // Where the sample ended, and whether the histories hold anything older.
        let edge: Option<(DateTime<Utc>, Id, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT i.created_at,i.id,count(*) OVER () FROM (SELECT i.created_at,i.id FROM items i
               WHERE {} AND {SEARCHABLE} AND {}
               ORDER BY i.created_at DESC,i.id DESC LIMIT $4) i ORDER BY i.created_at,i.id LIMIT 1",
            within.replace("$37", "$1"),
            PAST.replace("$38", "$2").replace("$39", "$3"),
        )))
        .persistent(false)
        .bind(threads.as_slice())
        .bind(after.map(|(at, _)| at))
        .bind(after.map(|(_, id)| id))
        .bind(recent)
        .fetch_optional(&mut *conn)
        .await?;
        past = edge
            .filter(|(_, _, rows)| *rows >= recent)
            .map(|(at, id, _)| (at, id));
    }
    if let Some(past) = past {
        let missing = bounds.wanted - i64::try_from(found.len()).unwrap_or(i64::MAX);
        // Words of three characters or more have trigrams; the index alone then finds rare ones anywhere.
        let indexed = query
            .terms()
            .iter()
            .any(|term| term.chars().count() >= TRIGRAM);
        let mut probed = None;
        if indexed {
            // The text index pays for each match it returns and the recency order for each row it passes:
            // a probe of √(wanted × rows past the sample) bounds either read at about that many rows, so
            // neither the rare nor the common words of a long history read it whole. The setting caps it.
            let wanted = f64::from(i32::try_from(bounds.wanted).unwrap_or(i32::MAX));
            let ceiling = f64::from(i32::try_from(bounds.probe).unwrap_or(i32::MAX));
            let rows_past = if threads.len() == 1 {
                plan(true).execute(&mut *conn).await?;
                let counted: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT count(*) FROM (SELECT 1 FROM items i WHERE {} AND {SEARCHABLE} AND {} LIMIT $4) i",
                    within.replace("$37", "$1"),
                    PAST.replace("$38", "$2").replace("$39", "$3"),
                )))
                .persistent(false)
                .bind(threads.as_slice())
                .bind(past.0)
                .bind(past.1)
                .bind(((ceiling * ceiling / wanted).ceil() as i64).max(1))
                .fetch_one(&mut *conn)
                .await?;
                counted as f64
            } else {
                // Several conversations are probed by text alone: the whole table is what it may return.
                f64::from(
                    sqlx::query_scalar::<_, f32>(
                        "SELECT GREATEST(reltuples,0) FROM pg_class WHERE oid='items'::regclass",
                    )
                    .fetch_one(&mut *conn)
                    .await?,
                )
            };
            let probe = ((wanted * rows_past).sqrt().ceil() as i64)
                .max(bounds.wanted)
                .min(bounds.probe);
            // One conversation's text is looked up together with its identity in the same index; several
            // are looked up by text alone, and the history test keeps theirs.
            let probe_within = if threads.len() == 1 { within } else { "true" };
            plan(false).execute(&mut *conn).await?;
            let rows = read(
                format!(
                    "{history}SELECT i.id,i.created_at FROM items i WHERE {probe_within} AND {SEARCHABLE} AND {PAST}
                       AND i.search_text ILIKE ($1::text[])[1] AND {matches} LIMIT $41"
                ),
                Some(past),
                probe.saturating_add(1),
            )
            .fetch_all(&mut *conn)
            .await?;
            if i64::try_from(rows.len()).unwrap_or(i64::MAX) <= probe {
                probed = Some(rows);
            }
        }
        let mut older = match probed {
            Some(rows) => rows,
            None => {
                // Common past the sample: recency order again, stopping at the matches still missing.
                plan(true).execute(&mut *conn).await?;
                read(
                    format!(
                        "{history}SELECT i.id,i.created_at FROM items i WHERE {within} AND {SEARCHABLE} AND {PAST}
                           AND {matches} ORDER BY i.created_at DESC,i.id DESC LIMIT $41"
                    ),
                    Some(past),
                    missing,
                )
                .fetch_all(&mut *conn)
                .await?
            }
        };
        older.sort_by_key(|row| std::cmp::Reverse((row.1, row.0)));
        older.truncate(usize::try_from(missing).unwrap_or(0));
        found.extend(older);
    }
    sqlx::query("SELECT set_config('enable_seqscan',$1,true),set_config('enable_indexscan',$2,true),set_config('enable_bitmapscan',$3,true)")
        .bind(settings.0)
        .bind(settings.1)
        .bind(settings.2)
        .execute(&mut *conn)
        .await?;
    Ok(found.into_iter().map(|(id, _)| id).collect())
}
