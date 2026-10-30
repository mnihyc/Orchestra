//! The raw events of a segment: the frames that crossed between Orch and a harness, kept for diagnosis and dropped with their month.
//!
//! A frame keeps its machine-assigned number in its segment, in capture order, and is stored as the
//! zstd-compressed text of its JSON. Its number is assigned before capture is spooled, and the
//! segment cursor survives removal of the frame bytes, so a replay cannot consume another place.
//! A full segment keeps its newest captured frames. The table is cut by month on the moment a
//! frame was reported,
//! and a month's partition is dropped when all of it is older than `diagnostics.retention_days`
//! (the worker's job); a frame whose month has no partition yet makes one, since a row without a
//! partition is refused and a diagnostic is not worth a stalled stream.
//!
//! Nothing here decides what may be kept or who may read it: the engine masks what must not be
//! stored before it asks for a frame to be written, and the request that lists them is the thread
//! manager's.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// The word of `raw_events.direction` for a frame that went to the harness.
pub const TO_HARNESS: &str = "to_harness";
/// The word of `raw_events.direction` for a frame that came from it.
pub const FROM_HARNESS: &str = "from_harness";

/// A frame to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRawEvent<'a> {
    /// The segment it belongs to.
    pub segment: Id,
    /// The positive number the machine assigned to this frame before spooling it.
    pub n: i64,
    /// When the machine reported it.
    pub at: DateTime<Utc>,
    /// [`TO_HARNESS`] or [`FROM_HARNESS`].
    pub direction: &'a str,
    /// The compressed text of its JSON.
    pub payload: &'a [u8],
    /// The most frames the segment keeps, newest first.
    pub at_most: i64,
}

/// A frame, as stored.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RawEventRow {
    /// Its number in the segment.
    pub n: i64,
    /// When the machine reported it.
    pub at: DateTime<Utc>,
    /// [`TO_HARNESS`] or [`FROM_HARNESS`].
    pub direction: String,
    /// The compressed text of its JSON.
    pub payload: Vec<u8>,
}

/// The SQL state of a row that no partition takes.
const NO_PARTITION: &str = "23514";

async fn insert(conn: &mut Conn, next: &NewRawEvent<'_>) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO raw_events (segment_id, n, at, direction, payload)
         VALUES ($1, $2, $3, $4, $5) RETURNING n",
    )
    .bind(next.segment)
    .bind(next.n)
    .bind(next.at)
    .bind(next.direction)
    .bind(next.payload)
    .fetch_optional(conn)
    .await
}

/// The durable allocator floor of a segment, including accepted frames whose bytes were pruned.
pub async fn last_number(conn: &mut Conn, segment: Id) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT raw_last_n FROM segments WHERE id = $1")
            .bind(segment)
            .fetch_optional(conn)
            .await?
            .unwrap_or(0),
    )
}

/// Keeps a new original number once, atomically with its segment cursor. A duplicate, an older
/// number or disabled retention returns none. The window retains the newest frames, and missing
/// month partitions are made as needed.
pub async fn append(conn: &mut Conn, next: &NewRawEvent<'_>) -> Result<Option<i64>, DbError> {
    if next.at_most <= 0 || next.n <= 0 {
        return Ok(None);
    }
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    // Lock the durable segment cursor, rather than a month's partition: it outlives all frame bytes.
    let last: i64 = sqlx::query_scalar("SELECT raw_last_n FROM segments WHERE id = $1 FOR UPDATE")
        .bind(next.segment)
        .fetch_one(&mut *tx)
        .await?;
    let n = if next.n <= last {
        None
    } else {
        let kept = append_new(&mut tx, next).await?;
        sqlx::query("UPDATE segments SET raw_last_n = $2 WHERE id = $1")
            .bind(next.segment)
            .bind(next.n)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM raw_events WHERE segment_id = $1 AND n < (
                SELECT n FROM raw_events WHERE segment_id = $1
                 ORDER BY n DESC OFFSET $2 LIMIT 1)",
        )
        .bind(next.segment)
        .bind(next.at_most - 1)
        .execute(&mut *tx)
        .await?;
        kept
    };
    tx.commit().await?;
    Ok(n)
}

async fn append_new(conn: &mut Conn, next: &NewRawEvent<'_>) -> Result<Option<i64>, DbError> {
    // A refused row would end the transaction it is in: it is tried in a savepoint of its own.
    let first = {
        let mut attempt = sqlx::Connection::begin(&mut *conn).await?;
        match insert(&mut attempt, next).await {
            Ok(n) => {
                attempt.commit().await?;
                return Ok(n);
            }
            Err(error) => {
                attempt.rollback().await?;
                error
            }
        }
    };
    let missing = first
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == NO_PARTITION);
    if !missing {
        return Err(first.into());
    }
    crate::migrations::ensure_partitions(conn, next.at).await?;
    Ok(insert(conn, next).await?)
}

/// How many frames a segment holds.
pub async fn count(conn: &mut Conn, segment: Id) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM raw_events WHERE segment_id = $1")
            .bind(segment)
            .fetch_one(conn)
            .await?,
    )
}

/// The frames of a segment numbered below `before` (all of them where none is given), newest first,
/// at most `limit`.
pub async fn newest(
    conn: &mut Conn,
    segment: Id,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<RawEventRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n, at, direction, payload FROM raw_events
          WHERE segment_id = $1 AND ($2::bigint IS NULL OR n < $2)
          ORDER BY n DESC LIMIT $3",
    )
    .bind(segment)
    .bind(before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Whether the segment holds a frame numbered below `n`.
pub async fn older_than(conn: &mut Conn, segment: Id, n: i64) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM raw_events WHERE segment_id = $1 AND n < $2)",
    )
    .bind(segment)
    .bind(n)
    .fetch_one(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;

    async fn segment(test: &TestDb) -> Id {
        let segment = new_id();
        seed_segment(&test.pool, new_id(), segment).await;
        segment
    }

    fn frame(segment: Id, n: i64, at: DateTime<Utc>, text: &str, at_most: i64) -> NewRawEvent<'_> {
        NewRawEvent {
            segment,
            n,
            at,
            direction: FROM_HARNESS,
            payload: text.as_bytes(),
            at_most,
        }
    }

    #[tokio::test]
    async fn frames_keep_their_original_numbers_in_order_and_gaps_do_not_consume_the_frame_allowance()
     {
        let test = TestDb::fresh().await;
        let (segment, other) = (segment(&test).await, segment(&test).await);
        let mut conn = test.pool.acquire().await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 10, 12, 8, 0, 0).unwrap();
        for (n, text) in [(1, "a"), (20, "b"), (30000, "c")] {
            assert_eq!(
                append(&mut conn, &frame(segment, n, at, text, 3))
                    .await
                    .unwrap(),
                Some(n)
            );
        }
        assert_eq!(
            append(&mut conn, &frame(other, 1, at, "x", 3))
                .await
                .unwrap(),
            Some(1)
        );
        let page = newest(&mut conn, segment, None, 2).await.unwrap();
        assert_eq!(
            page.iter().map(|row| row.n).collect::<Vec<_>>(),
            [30000, 20]
        );
        assert_eq!(page[0].payload, b"c");
        assert!(older_than(&mut conn, segment, 20).await.unwrap());
        assert!(!older_than(&mut conn, segment, 1).await.unwrap());
        let before = newest(&mut conn, segment, Some(30000), 10).await.unwrap();
        assert_eq!(before.iter().map(|row| row.n).collect::<Vec<_>>(), [20, 1]);
        assert_eq!(count(&mut conn, segment).await.unwrap(), 3);
        assert_eq!(last_number(&mut conn, segment).await.unwrap(), 30000);
    }

    #[tokio::test]
    async fn a_segment_keeps_newest_frames_and_its_cursor_survives_eviction_and_all_byte_pruning() {
        let test = TestDb::fresh().await;
        let segment = segment(&test).await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 10, 12, 8, 0, 0).unwrap();
        for n in 1..=4 {
            assert_eq!(
                append(&mut conn, &frame(segment, n, at, "x", 3))
                    .await
                    .unwrap(),
                Some(n)
            );
        }
        assert_eq!(count(&mut conn, segment).await.unwrap(), 3);
        assert_eq!(
            newest(&mut conn, segment, None, 10)
                .await
                .unwrap()
                .iter()
                .map(|row| row.n)
                .collect::<Vec<_>>(),
            [4, 3, 2]
        );
        assert_eq!(
            append(&mut conn, &frame(segment, 1, at, "x", 3))
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            append(&mut conn, &frame(segment, 2, at, "x", 3))
                .await
                .unwrap(),
            None
        );
        sqlx::query("DELETE FROM raw_events WHERE segment_id = $1")
            .bind(segment)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(last_number(&mut conn, segment).await.unwrap(), 4);
        assert_eq!(
            append(
                &mut conn,
                &frame(segment, 4, at + chrono::Duration::days(32), "old", 3)
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(count(&mut conn, segment).await.unwrap(), 0);
        assert_eq!(
            append(&mut conn, &frame(segment, 5, at, "new", 3))
                .await
                .unwrap(),
            Some(5)
        );
    }

    #[tokio::test]
    async fn a_frame_of_a_month_that_has_no_partition_makes_it_inside_the_callers_transaction() {
        let test = TestDb::fresh().await;
        let (segment, other) = (segment(&test).await, segment(&test).await);
        let mut conn = test.pool.acquire().await.unwrap();
        let at = Utc.with_ymd_and_hms(2019, 3, 9, 8, 0, 0).unwrap();
        assert_eq!(
            append(&mut conn, &frame(segment, 1, at, "old", 10))
                .await
                .unwrap(),
            Some(1)
        );
        let mut tx = test.pool.begin().await.unwrap();
        let later = Utc.with_ymd_and_hms(2024, 7, 1, 0, 0, 0).unwrap();
        assert_eq!(
            append(&mut tx, &frame(other, 9, later, "later", 10))
                .await
                .unwrap(),
            Some(9)
        );
        assert_eq!(last_number(&mut tx, other).await.unwrap(), 9);
        tx.rollback().await.unwrap();
        assert_eq!(last_number(&mut conn, other).await.unwrap(), 0);
        assert_eq!(count(&mut conn, other).await.unwrap(), 0);
        assert_eq!(
            append(&mut conn, &frame(other, 9, later, "later", 10))
                .await
                .unwrap(),
            Some(9)
        );
    }

    #[tokio::test]
    async fn a_replayed_number_across_months_has_no_effect_and_equal_payloads_can_be_distinct_frames()
     {
        let test = TestDb::fresh().await;
        let segment = segment(&test).await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 9, 30, 23, 59, 59).unwrap();
        assert_eq!(
            append(&mut conn, &frame(segment, 41, at, "same", 2))
                .await
                .unwrap(),
            Some(41)
        );
        assert_eq!(
            append(
                &mut conn,
                &frame(
                    segment,
                    41,
                    at + chrono::Duration::days(1),
                    "changed replay",
                    2
                )
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            append(&mut conn, &frame(segment, 42, at, "same", 2))
                .await
                .unwrap(),
            Some(42)
        );
        let kept = newest(&mut conn, segment, None, 10).await.unwrap();
        assert_eq!(kept.iter().map(|row| row.n).collect::<Vec<_>>(), [42, 41]);
        assert!(
            kept.iter()
                .all(|row| row.payload == b"same" && row.at == at)
        );
    }

    #[tokio::test]
    async fn simultaneous_deliveries_of_one_frame_advance_one_segment_cursor_once() {
        let test = TestDb::fresh().await;
        let segment = segment(&test).await;
        let at = Utc.with_ymd_and_hms(2026, 10, 13, 8, 0, 0).unwrap();
        let append_once = || async {
            let mut conn = test.pool.acquire().await.unwrap();
            append(&mut conn, &frame(segment, 7, at, "once", 10))
                .await
                .unwrap()
        };
        let (first, second) = tokio::join!(append_once(), append_once());
        assert_eq!(
            [first, second].into_iter().filter(Option::is_some).count(),
            1
        );
        let mut conn = test.pool.acquire().await.unwrap();
        assert_eq!(count(&mut conn, segment).await.unwrap(), 1);
        assert_eq!(last_number(&mut conn, segment).await.unwrap(), 7);
    }

    async fn seed_segment(pool: &sqlx::PgPool, thread: Id, segment: Id) {
        let user = new_id();
        sqlx::query("INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, $2, 'Raw tester', 0)")
            .bind(user).bind(user.simple().to_string()).execute(pool).await.unwrap();
        let (org, project, topic) = (new_id(), new_id(), new_id());
        for (node, kind, parent, path) in [
            (org, "org", None, vec![org]),
            (project, "project", Some(org), vec![org, project]),
            (topic, "topic", Some(project), vec![org, project, topic]),
            (
                thread,
                "thread",
                Some(topic),
                vec![org, project, topic, thread],
            ),
        ] {
            sqlx::query(
                "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by)
                         VALUES ($1, $2, $3, $4, $5, 'Raw frames', $6, $6)",
            )
            .bind(node)
            .bind(kind)
            .bind(parent)
            .bind(org)
            .bind(path)
            .bind(user)
            .execute(pool)
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO threads (node_id, topic_id) VALUES ($1, $2)")
            .bind(thread)
            .bind(topic)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO segments (id, thread_id, ordinal, agent_id, provider_id, environment_id,
                        env_generation, arrangement, credential_path, seq_updated)
                    VALUES ($1, $2, 1, $3, $3, $3, 1, 'together', 'native', 1)",
        )
        .bind(segment)
        .bind(thread)
        .bind(new_id())
        .execute(pool)
        .await
        .unwrap();
    }
}
