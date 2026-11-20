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
use orch_api_contract::threads::{
    RawCapture, RawCaptureLimits, RawCaptureOmission, RawCaptureOmissionCause, RawFrameRange,
};
use orch_types::Id;
use orch_types::Time;
use serde::{Deserialize, Serialize};

/// Safe facts supplied by the operation that actually captured or omitted this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureObservation {
    pub omitted: Option<RawCaptureOmissionCause>,
    pub oversized: bool,
    pub truncated: bool,
    pub limits: RawCaptureLimits,
}

#[derive(Debug, Serialize, Deserialize)]
struct CaptureState {
    first: i64,
    last: i64,
    observed: i64,
    kept: i64,
    off: i64,
    masking: i64,
    evicted: i64,
    expired: i64,
    oversized: i64,
    truncated: i64,
    earlier_unreported: bool,
    limits: RawCaptureLimits,
    captured_at: DateTime<Utc>,
}

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
    /// None for a body with no captured provenance; it must not acquire invented facts.
    pub capture: Option<CaptureObservation>,
}

/// A frame, as stored.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RawEventRow {
    /// Native capture or masking completion revision, independent of the capture number.
    pub change_id: i64,
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

// Capture writers share this lock; retention takes it exclusively before partition DDL.
// Retention only locks capture rows, so it never waits for a segment held by an ingesting unit.
async fn lock_capture(conn: &mut Conn) -> Result<(), DbError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock_shared(hashtextextended('orch.raw_capture.retention',0))",
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Holds capture/read publication until retention metadata and partition removal commit together.
pub async fn lock_retention(conn: &mut Conn) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('orch.raw_capture.retention',0))")
        .execute(conn)
        .await?;
    Ok(())
}

/// Protects the partition layout before the snapshot and authorization epoch are recorded.
/// A LOCK utility statement takes no data snapshot; a reader queued behind cleanup sees its committed counters and layout together.
pub async fn begin_reading(db: &crate::Db) -> Result<crate::Tx, DbError> {
    db.begin_isolated_locked(
        "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
        Some("LOCK TABLE ONLY raw_events IN ACCESS SHARE MODE"),
    )
    .await
}

async fn insert(conn: &mut Conn, next: &NewRawEvent<'_>) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO raw_events (segment_id, n, at, direction, payload, capture_reported)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING n",
    )
    .bind(next.segment)
    .bind(next.n)
    .bind(next.at)
    .bind(next.direction)
    .bind(next.payload)
    .bind(next.capture.is_some())
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
    if next.n <= 0 || next.at_most <= 0 && next.capture.is_none() {
        return Ok(None);
    }
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    lock_capture(&mut tx).await?;
    // Lock the durable segment cursor, rather than a month's partition: it outlives all frame bytes.
    let last: i64 = sqlx::query_scalar("SELECT raw_last_n FROM segments WHERE id = $1 FOR UPDATE")
        .bind(next.segment)
        .fetch_one(&mut *tx)
        .await?;
    let n = if next.n <= last {
        None
    } else {
        let recorded: Option<sqlx::types::Json<CaptureState>> = sqlx::query_scalar(
            "SELECT capture FROM raw_capture_state WHERE segment_id=$1 FOR UPDATE",
        )
        .bind(next.segment)
        .fetch_optional(&mut *tx)
        .await?;
        let omitted = next
            .capture
            .as_ref()
            .and_then(|capture| capture.omitted)
            .or((next.at_most <= 0).then_some(RawCaptureOmissionCause::CaptureOff));
        let kept = if omitted.is_none() {
            append_new(&mut tx, next).await?
        } else {
            None
        };
        let mut state = recorded.map(|recorded| recorded.0);
        if let Some(observation) = &next.capture {
            let state = state.get_or_insert_with(|| CaptureState {
                first: next.n,
                last: next.n,
                observed: 0,
                kept: 0,
                off: 0,
                masking: 0,
                evicted: 0,
                expired: 0,
                oversized: 0,
                truncated: 0,
                earlier_unreported: last > 0,
                limits: observation.limits.clone(),
                captured_at: next.at,
            });
            // Positive, increasing native numbers also bound every lifetime counter by i64::MAX.
            state.last = next.n;
            state.observed += 1;
            state.captured_at = next.at;
            state.limits = observation.limits.clone();
            match omitted {
                Some(RawCaptureOmissionCause::CaptureOff) => state.off += 1,
                Some(RawCaptureOmissionCause::MaskingUnavailable) => state.masking += 1,
                Some(RawCaptureOmissionCause::CaptureAllowance) => {
                    state.kept += 1;
                    state.evicted += 1;
                }
                Some(RawCaptureOmissionCause::RetentionElapsed) => {
                    state.kept += 1;
                    state.expired += 1;
                }
                None => {
                    if kept.is_some() {
                        state.kept += 1;
                        if observation.oversized {
                            state.oversized += 1;
                        }
                        if observation.truncated {
                            state.truncated += 1;
                        }
                    }
                }
            }
            if kept.is_some() {
                sqlx::query("INSERT INTO raw_capture_months (segment_id, month_start, retained)
                    VALUES ($1, date_trunc('month', $2::timestamptz AT TIME ZONE 'UTC') AT TIME ZONE 'UTC', 1)
                    ON CONFLICT (segment_id, month_start) DO UPDATE SET retained=raw_capture_months.retained+1")
                    .bind(next.segment).bind(next.at).execute(&mut *tx).await?;
            }
        } else if let Some(state) = &mut state {
            state.earlier_unreported = true;
        }
        if kept.is_some() {
            let removed: Vec<(DateTime<Utc>, i64)> = sqlx::query_as(
                "WITH gone AS (DELETE FROM raw_events WHERE segment_id = $1 AND n < (
                SELECT n FROM raw_events WHERE segment_id = $1
                 ORDER BY n DESC OFFSET $2 LIMIT 1) RETURNING at, capture_reported)
             SELECT date_trunc('month', at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC', count(*)
              FROM gone WHERE capture_reported GROUP BY 1",
            )
            .bind(next.segment)
            .bind(next.at_most - 1)
            .fetch_all(&mut *tx)
            .await?;
            for (month, count) in removed {
                reduce_month(&mut tx, next.segment, month, count).await?;
                if let Some(state) = &mut state {
                    state.evicted += count;
                }
            }
        }
        sqlx::query("UPDATE segments SET raw_last_n=$2 WHERE id=$1")
            .bind(next.segment)
            .bind(next.n)
            .execute(&mut *tx)
            .await?;
        if let Some(state) = state {
            sqlx::query(
                "INSERT INTO raw_capture_state (segment_id,capture) VALUES($1,$2)
                ON CONFLICT(segment_id) DO UPDATE SET capture=EXCLUDED.capture",
            )
            .bind(next.segment)
            .bind(sqlx::types::Json(state))
            .execute(&mut *tx)
            .await?;
        }
        kept
    };
    tx.commit().await?;
    Ok(n)
}

async fn reduce_month(
    conn: &mut Conn,
    segment: Id,
    month: DateTime<Utc>,
    count: i64,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE raw_capture_months SET retained=retained-$3 WHERE segment_id=$1 AND month_start=$2",
    )
    .bind(segment)
    .bind(month)
    .bind(count)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM raw_capture_months WHERE segment_id=$1 AND month_start=$2 AND retained=0",
    )
    .bind(segment)
    .bind(month)
    .execute(conn)
    .await?;
    Ok(())
}

/// The source's actual reported bounds and counters, independent of viewed pages and filters.
/// The caller's read snapshot keeps these facts and its frame page at the same observation.
pub async fn capture(conn: &mut Conn, segment: Id) -> Result<Option<RawCapture>, DbError> {
    let state: Option<sqlx::types::Json<CaptureState>> =
        sqlx::query_scalar("SELECT capture FROM raw_capture_state WHERE segment_id=$1")
            .bind(segment)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(sqlx::types::Json(state)) = state else {
        return Ok(None);
    };
    let (from, through, unreported): (Option<i64>, Option<i64>, bool) = sqlx::query_as(
        "SELECT (SELECT n FROM raw_events WHERE segment_id=$1 ORDER BY n LIMIT 1),
                (SELECT n FROM raw_events WHERE segment_id=$1 ORDER BY n DESC LIMIT 1),
                EXISTS(SELECT 1 FROM raw_events WHERE segment_id=$1 AND NOT capture_reported)",
    )
    .bind(segment)
    .fetch_one(conn)
    .await?;
    let retained_ranges = match (from, through) {
        (Some(from), Some(through)) => vec![RawFrameRange {
            from,
            through,
            count: (!unreported).then_some(state.kept - state.evicted - state.expired),
        }],
        _ => Vec::new(),
    };
    Ok(Some(RawCapture {
        observed_range: RawFrameRange {
            from: state.first,
            through: state.last,
            count: Some(state.observed),
        },
        retained_ranges,
        omission_counts_by_cause: [
            (RawCaptureOmissionCause::CaptureOff, state.off),
            (RawCaptureOmissionCause::MaskingUnavailable, state.masking),
            (RawCaptureOmissionCause::CaptureAllowance, state.evicted),
            (RawCaptureOmissionCause::RetentionElapsed, state.expired),
        ]
        .into_iter()
        .map(|(cause, count)| RawCaptureOmission { cause, count })
        .collect(),
        oversize_notes: state.oversized,
        truncated_notes: state.truncated,
        earlier_unreported: state.earlier_unreported,
        limits: state.limits,
        captured_at: Time::new(state.captured_at),
    }))
}

/// Retires only small month counters before whole protocol partitions are dropped in this same transaction.
/// Capture rows precede month rows; cleanup never takes the parent segment lock held by an ingesting unit.
pub async fn expire_partitions_before(
    conn: &mut Conn,
    cutoff: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    lock_retention(conn).await?;
    sqlx::query("WITH locked AS MATERIALIZED (
        SELECT s.segment_id FROM raw_capture_state s WHERE EXISTS (SELECT 1 FROM raw_capture_months m
          WHERE m.segment_id=s.segment_id AND m.retained>0 AND m.month_start<$1
            AND ((m.month_start AT TIME ZONE 'UTC') + interval '1 month') AT TIME ZONE 'UTC' <= $1)
        ORDER BY s.segment_id FOR UPDATE OF s), gone AS (
        DELETE FROM raw_capture_months m USING locked l WHERE m.segment_id=l.segment_id
          AND ((m.month_start AT TIME ZONE 'UTC') + interval '1 month') AT TIME ZONE 'UTC' <= $1
        RETURNING m.segment_id, m.retained), counted AS (
        SELECT segment_id, sum(retained)::bigint AS removed FROM gone GROUP BY segment_id)
        UPDATE raw_capture_state s SET capture=s.capture || jsonb_build_object(
          'expired', (s.capture->>'expired')::bigint+c.removed, 'captured_at', $2::timestamptz)
        FROM counted c WHERE s.segment_id=c.segment_id")
        .bind(cutoff).bind(at).execute(conn).await?;
    Ok(())
}

/// Removes one existing bounded partial-month page and records the actual reported rows removed.
pub async fn trim_before(
    conn: &mut Conn,
    cutoff: DateTime<Utc>,
    at: DateTime<Utc>,
    limit: i64,
) -> Result<u64, DbError> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    lock_retention(&mut tx).await?;
    let removed: Vec<(Id, DateTime<Utc>, bool, i64)> = sqlx::query_as("WITH picked AS MATERIALIZED (
        SELECT segment_id, n, at FROM raw_events WHERE at<$1 LIMIT $2),
        locked AS MATERIALIZED (SELECT s.segment_id FROM raw_capture_state s WHERE s.segment_id IN (SELECT segment_id FROM picked)
          ORDER BY s.segment_id FOR UPDATE OF s), lock_count AS MATERIALIZED (SELECT count(*) AS n FROM locked), gone AS (
        DELETE FROM raw_events r USING picked p, lock_count l WHERE r.segment_id=p.segment_id
          AND r.n=p.n AND r.at=p.at AND l.n>=0 RETURNING r.segment_id,r.at,r.capture_reported)
        SELECT segment_id, date_trunc('month',at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC',capture_reported,count(*)
          FROM gone GROUP BY 1,2,3")
        .bind(cutoff).bind(limit).fetch_all(&mut *tx).await?;
    let mut total = 0_u64;
    for (segment, month, reported, count) in removed {
        total += u64::try_from(count)
            .map_err(|error| DbError::Other(sqlx::Error::Decode(Box::new(error))))?;
        if reported {
            reduce_month(&mut tx, segment, month, count).await?;
            sqlx::query(
                "UPDATE raw_capture_state SET capture=capture || jsonb_build_object(
                'expired', (capture->>'expired')::bigint+$2, 'captured_at', $3::timestamptz)
                WHERE segment_id=$1",
            )
            .bind(segment)
            .bind(count)
            .bind(at)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(total)
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
        "SELECT n, at, direction, payload, change_id FROM raw_events
          WHERE segment_id = $1 AND ($2::bigint IS NULL OR n < $2)
          ORDER BY n DESC LIMIT $3",
    )
    .bind(segment)
    .bind(before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Current captured or masking-completed revision of this segment.
pub async fn last_change(conn: &mut Conn, segment: Id) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(max(change_id),0) FROM raw_events WHERE segment_id=$1")
            .bind(segment)
            .fetch_one(conn)
            .await?,
    )
}

/// Changed frames in commit order, including an earlier frame whose private text just ended.
pub async fn changed(
    conn: &mut Conn,
    segment: Id,
    after: i64,
    through: i64,
    limit: i64,
) -> Result<Vec<RawEventRow>, DbError> {
    Ok(sqlx::query_as("SELECT n,at,direction,payload,change_id FROM raw_events WHERE segment_id=$1 AND change_id>$2 AND change_id<=$3 ORDER BY change_id LIMIT $4")
        .bind(segment).bind(after).bind(through).bind(limit).fetch_all(conn).await?)
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
            capture: None,
        }
    }

    #[tokio::test]
    async fn capture_provenance_counts_actual_omissions_once_and_survives_byte_retention() {
        use orch_api_contract::settings::{SettingScope, SettingSource};
        use orch_api_contract::threads::{RawCaptureLimits, RawCaptureOmissionCause};
        let test = TestDb::fresh().await;
        let segment = segment(&test).await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 1, 31, 23, 59, 59).unwrap();
        append(
            &mut conn,
            &frame(segment, 1, at, "uncaptured old record", 2),
        )
        .await
        .unwrap();
        assert!(capture(&mut conn, segment).await.unwrap().is_none());
        let source = SettingSource {
            scope: SettingScope::Default,
            node: None,
        };
        let limits = RawCaptureLimits {
            native_n: 3,
            captured_at: Time::new(at),
            frame_bytes: 64,
            head_bytes: 8,
            frames_per_segment: 2,
            frame_source: source.clone(),
            head_source: source.clone(),
            frames_source: source,
        };
        let observed = |n, when, oversized, omitted| NewRawEvent {
            capture: Some(CaptureObservation {
                limits: RawCaptureLimits {
                    native_n: n,
                    captured_at: Time::new(when),
                    ..limits.clone()
                },
                oversized,
                truncated: oversized,
                omitted,
            }),
            ..frame(segment, n, when, "masked bytes", 2)
        };
        for (n, when) in [
            (3, at),
            (9, at + chrono::Duration::seconds(1)),
            (10, at + chrono::Duration::days(1)),
        ] {
            append(&mut conn, &observed(n, when, n == 10, None))
                .await
                .unwrap();
        }
        let dropped = observed(
            12,
            at + chrono::Duration::days(2),
            false,
            Some(RawCaptureOmissionCause::MaskingUnavailable),
        );
        append(&mut conn, &dropped).await.unwrap();
        let before = capture(&mut conn, segment).await.unwrap().unwrap();
        assert_eq!(before.observed_range.from, 3);
        assert_eq!(before.observed_range.through, 12);
        assert_eq!(
            before.observed_range.count,
            Some(4),
            "native number gaps are not omission counts"
        );
        assert!(before.earlier_unreported);
        assert_eq!(before.retained_ranges[0].from, 9);
        assert_eq!(before.retained_ranges[0].through, 10);
        assert_eq!(before.retained_ranges[0].count, Some(2));
        let count = |receipt: &orch_api_contract::threads::RawCapture, cause| {
            receipt
                .omission_counts_by_cause
                .iter()
                .find(|entry| entry.cause == cause)
                .unwrap()
                .count
        };
        assert_eq!(
            count(&before, RawCaptureOmissionCause::MaskingUnavailable),
            1
        );
        assert_eq!(count(&before, RawCaptureOmissionCause::CaptureAllowance), 1);
        assert_eq!(before.oversize_notes, 1);
        assert_eq!(
            before.limits.frame_bytes, 64,
            "recorded bounds do not borrow the current default"
        );
        append(&mut conn, &dropped).await.unwrap();
        assert_eq!(capture(&mut conn, segment).await.unwrap().unwrap(), before);
        let cutoff = at + chrono::Duration::hours(12);
        assert_eq!(
            trim_before(&mut conn, cutoff, at + chrono::Duration::days(3), 100)
                .await
                .unwrap(),
            1
        );
        let partial = capture(&mut conn, segment).await.unwrap().unwrap();
        assert_eq!(
            count(&partial, RawCaptureOmissionCause::RetentionElapsed),
            1
        );
        assert_eq!(partial.retained_ranges[0].count, Some(1));
        let cutoff = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let mut tx = test.pool.begin().await.unwrap();
        expire_partitions_before(&mut tx, cutoff, cutoff)
            .await
            .unwrap();
        crate::migrations::drop_partitions_before(&mut tx, "raw_events", cutoff)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let expired = capture(&mut conn, segment).await.unwrap().unwrap();
        assert!(expired.retained_ranges.is_empty());
        assert_eq!(
            count(&expired, RawCaptureOmissionCause::RetentionElapsed),
            2
        );
        assert_eq!(expired.oversize_notes, 1);
        append(&mut conn, &dropped).await.unwrap();
        assert_eq!(capture(&mut conn, segment).await.unwrap().unwrap(), expired);
    }

    #[tokio::test]
    async fn capture_off_is_an_observed_omission_and_never_keeps_the_frame_body() {
        use orch_api_contract::settings::{SettingScope, SettingSource};
        use orch_api_contract::threads::{RawCaptureLimits, RawCaptureOmissionCause};
        let test = TestDb::fresh().await;
        let segment = segment(&test).await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 10, 12, 8, 0, 0).unwrap();
        let source = SettingSource {
            scope: SettingScope::System,
            node: None,
        };
        let next = NewRawEvent {
            capture: Some(CaptureObservation {
                oversized: false,
                truncated: false,
                omitted: Some(RawCaptureOmissionCause::CaptureOff),
                limits: RawCaptureLimits {
                    native_n: 4,
                    captured_at: Time::new(at),
                    frame_bytes: 64,
                    head_bytes: 8,
                    frames_per_segment: 0,
                    frame_source: source.clone(),
                    head_source: source.clone(),
                    frames_source: source,
                },
            }),
            ..frame(segment, 4, at, "must not be stored", 0)
        };
        assert_eq!(append(&mut conn, &next).await.unwrap(), None);
        assert_eq!(count(&mut conn, segment).await.unwrap(), 0);
        assert_eq!(last_number(&mut conn, segment).await.unwrap(), 4);
        let receipt = capture(&mut conn, segment).await.unwrap().unwrap();
        assert!(receipt.retained_ranges.is_empty());
        assert_eq!(
            receipt
                .omission_counts_by_cause
                .iter()
                .find(|entry| entry.cause == RawCaptureOmissionCause::CaptureOff)
                .unwrap()
                .count,
            1
        );
        append(&mut conn, &next).await.unwrap();
        assert_eq!(capture(&mut conn, segment).await.unwrap().unwrap(), receipt);
    }

    #[tokio::test]
    async fn capture_cleanup_does_not_wait_for_a_segment_already_held_by_an_ingesting_unit() {
        use orch_api_contract::settings::{SettingScope, SettingSource};
        let test = TestDb::fresh().await;
        let segment = segment(&test).await;
        let at = Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap();
        let source = SettingSource {
            scope: SettingScope::Default,
            node: None,
        };
        let mut initial = test.pool.acquire().await.unwrap();
        let limits = RawCaptureLimits {
            native_n: 1,
            captured_at: Time::new(at),
            frame_bytes: 64,
            head_bytes: 8,
            frames_per_segment: 2,
            frame_source: source.clone(),
            head_source: source.clone(),
            frames_source: source,
        };
        let first = NewRawEvent {
            capture: Some(CaptureObservation {
                limits: limits.clone(),
                omitted: None,
                oversized: false,
                truncated: false,
            }),
            ..frame(segment, 1, at, "masked first frame", 2)
        };
        append(&mut initial, &first).await.unwrap();
        drop(initial);
        let mut producer = test.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM segments WHERE id=$1 FOR UPDATE")
            .bind(segment)
            .execute(&mut *producer)
            .await
            .unwrap();
        let mut cleanup = test.pool.begin().await.unwrap();
        lock_retention(&mut cleanup).await.unwrap();
        let cutoff = at + chrono::Duration::days(60);
        let write = async {
            let later = NewRawEvent {
                capture: Some(CaptureObservation {
                    limits: RawCaptureLimits {
                        native_n: 2,
                        captured_at: Time::new(cutoff),
                        ..limits
                    },
                    omitted: None,
                    oversized: false,
                    truncated: false,
                }),
                ..frame(segment, 2, cutoff, "masked later frame", 2)
            };
            append(&mut producer, &later).await.unwrap();
            producer.commit().await.unwrap();
        };
        let remove = async {
            expire_partitions_before(&mut cleanup, cutoff, cutoff)
                .await
                .unwrap();
            crate::migrations::drop_partitions_before(&mut cleanup, "raw_events", cutoff)
                .await
                .unwrap();
            cleanup.commit().await.unwrap();
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(write, remove);
        })
        .await
        .expect("cleanup must finish without a segment/advisory lock cycle");
    }

    #[tokio::test]
    async fn a_raw_reader_queued_behind_partition_cleanup_takes_its_snapshot_after_the_cleanup() {
        use orch_api_contract::settings::{SettingScope, SettingSource};
        use orch_api_contract::threads::{RawCaptureLimits, RawCaptureOmissionCause};
        let test = TestDb::fresh().await;
        let segment = segment(&test).await;
        let at = Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap();
        let cutoff = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let source = SettingSource {
            scope: SettingScope::Default,
            node: None,
        };
        let mut conn = test.pool.acquire().await.unwrap();
        let observed = NewRawEvent {
            capture: Some(CaptureObservation {
                omitted: None,
                oversized: false,
                truncated: false,
                limits: RawCaptureLimits {
                    native_n: 1,
                    captured_at: Time::new(at),
                    frame_bytes: 64,
                    head_bytes: 8,
                    frames_per_segment: 2,
                    frame_source: source.clone(),
                    head_source: source.clone(),
                    frames_source: source,
                },
            }),
            ..frame(segment, 1, at, "masked bytes", 2)
        };
        append(&mut conn, &observed).await.unwrap();
        drop(conn);
        let mut blocker = test.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE ONLY raw_events IN ACCESS SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let mut cleanup = test.pool.begin().await.unwrap();
        let cleanup_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *cleanup)
            .await
            .unwrap();
        let cleaning = tokio::spawn(async move {
            expire_partitions_before(&mut cleanup, cutoff, cutoff)
                .await
                .unwrap();
            crate::migrations::drop_partitions_before(&mut cleanup, "raw_events", cutoff)
                .await
                .unwrap();
            cleanup.commit().await.unwrap();
        });
        struct AbortOnDrop(Vec<tokio::task::AbortHandle>);
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                for task in &self.0 {
                    task.abort();
                }
            }
        }
        let mut tasks = AbortOnDrop(vec![cleaning.abort_handle()]);
        let wait = |mode: &'static str, pid: Option<i32>, pool: sqlx::PgPool| async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks
                        WHERE database=(SELECT oid FROM pg_database WHERE datname=current_database())
                          AND relation='raw_events'::regclass AND mode=$1 AND NOT granted
                          AND ($2::int IS NULL OR pid=$2))")
                        .bind(mode).bind(pid).fetch_one(&pool).await.unwrap();
                    if waiting { break; }
                    tokio::task::yield_now().await;
                }
            }).await.expect("the barrier must reach the real relation lock queue");
        };
        wait("AccessExclusiveLock", Some(cleanup_pid), test.pool.clone()).await;
        let db = crate::Db::from_pool(test.pool.clone());
        let reading = tokio::spawn(async move {
            let mut read = begin_reading(&db).await.unwrap();
            let receipt = capture(&mut read, segment).await.unwrap().unwrap();
            let bytes = count(&mut read, segment).await.unwrap();
            read.commit().await.unwrap();
            (receipt, bytes)
        });
        tasks.0.push(reading.abort_handle());
        wait("AccessShareLock", None, test.pool.clone()).await;
        blocker.commit().await.unwrap();
        let (receipt, bytes) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            cleaning.await.unwrap();
            reading.await.unwrap()
        })
        .await
        .expect("the reader and cleanup must both finish");
        assert_eq!(bytes, 0);
        assert!(receipt.retained_ranges.is_empty());
        assert_eq!(
            receipt
                .omission_counts_by_cause
                .iter()
                .find(|entry| entry.cause == RawCaptureOmissionCause::RetentionElapsed)
                .unwrap()
                .count,
            1
        );
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
