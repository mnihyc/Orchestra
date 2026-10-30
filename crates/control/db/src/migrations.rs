//! Apply canonical SQL migrations.
//!
//! The schema is the SQL files of the `migrations` directory, applied in
//! order and recorded, so that a start applies only what is new. Three
//! tables are partitioned by time; their partitions are made here, ahead
//! of the time they will hold, because a row for which no partition
//! exists is refused.

use crate::pool::{Conn, Db, DbError};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use std::path::Path;

mod progress;
pub use progress::MigrationProgress;

/// Applies every migration of `dir` that the database does not have yet.
pub async fn apply(db: &Db, dir: &Path) -> Result<(), MigrateError> {
    apply_reporting(
        db,
        dir,
        &orch_types::SystemClock,
        std::time::Duration::from_secs(1),
        |_| {},
    )
    .await
}

/// Applies the canonical migrations with step observations, preserving SQLx's complete migration procedure.
pub async fn apply_reporting(
    db: &Db,
    dir: &Path,
    clock: &dyn orch_types::Clock,
    interval: std::time::Duration,
    mut report: impl FnMut(&MigrationProgress) + Send,
) -> Result<(), MigrateError> {
    let started = std::time::Instant::now();
    let interval = interval.max(std::time::Duration::from_secs(1));
    let migrator = sqlx::migrate::Migrator::new(dir).await?;
    let mut conn = db.acquire().await?;
    // Schema changes may exceed the API pool's per-request statement timeout. Disable that bound on
    // this connection during migration, then restore it before the pool hands the connection out again.
    sqlx::query("SET statement_timeout = 0")
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let ran = progress::run(db, &mut conn, &migrator, clock, interval, &mut report).await;
    let reset = sqlx::query("RESET statement_timeout")
        .execute(&mut *conn)
        .await;
    ran?;
    reset.map_err(DbError::from)?;
    let repaired = repair_machine_homes(&mut conn).await;
    report(&progress::finished(
        clock,
        started.elapsed().as_secs(),
        interval,
        i64::try_from(migrator.iter().count()).unwrap_or(i64::MAX),
        repaired.as_ref().ok().copied(),
    ));
    repaired?;
    Ok(())
}

/// Repair pre-resource physical machines at upgrade; existing homes and internal connectors stay put.
/// This data-only step is idempotent and also runs when no schema migration is pending.
pub async fn repair_machine_homes(conn: &mut Conn) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE machines m SET home_id=n.id,org_id=n.org_id,revision=m.revision+1
         FROM users u,nodes n
         WHERE m.owner_id=u.id AND m.runtime_id IS NULL AND m.host_id IS NULL
           AND n.id=COALESCE(m.home_id,u.personal_org_id,u.service_org_id)
           AND (m.home_id IS NULL OR m.org_id IS DISTINCT FROM n.org_id)",
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// Why the schema could not be brought up to date.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// A canonical schema migration failed.
    #[error("the schema could not be brought up to date: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// An idempotent upgrade data repair failed.
    #[error("upgrade data could not be brought up to date: {0}")]
    Data(#[from] DbError),
}

/// How a partitioned table is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    Day,
    Month,
    Year,
}

/// The partitioned tables, with how each is cut and how many spans ahead
/// of the present are kept ready. Two ahead: a job that runs once a day
/// and misses a run still finds tomorrow's partition there.
const PARTITIONED: &[(&str, Span)] = &[
    ("raw_events", Span::Month),
    ("metric_samples", Span::Day),
    ("metric_hours", Span::Year),
];
const SPANS_AHEAD: u32 = 2;

fn start_of(span: Span, day: NaiveDate) -> NaiveDate {
    match span {
        Span::Day => day,
        Span::Month => day.with_day(1).unwrap_or(day),
        Span::Year => NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(day),
    }
}

fn next(span: Span, start: NaiveDate) -> NaiveDate {
    match span {
        Span::Day => start + Duration::days(1),
        // A day past the longest month, or the longest year, lands in the next one.
        Span::Month => start_of(Span::Month, start + Duration::days(32)),
        Span::Year => start_of(Span::Year, start + Duration::days(367)),
    }
}

fn suffix(span: Span, start: NaiveDate) -> String {
    match span {
        Span::Day => start.format("%Y%m%d").to_string(),
        Span::Month => start.format("%Y%m").to_string(),
        Span::Year => start.format("%Y").to_string(),
    }
}

/// Makes sure that the partitions for `now` and the spans after it exist.
/// Safe to call at every start and from a daily job.
pub async fn ensure_partitions(conn: &mut Conn, now: DateTime<Utc>) -> Result<(), DbError> {
    for (table, span) in PARTITIONED {
        let mut start = start_of(*span, now.date_naive());
        for _ in 0..=SPANS_AHEAD {
            let end = next(*span, start);
            if *table == "metric_samples" {
                ensure_metric_partition(conn, start).await?;
                start = end;
                continue;
            }
            // Names and bounds are built from this module's own constants and
            // from dates: nothing here comes from outside.
            let statement = format!(
                "CREATE TABLE IF NOT EXISTS {table}_p{} PARTITION OF {table} FOR VALUES FROM ('{start}') TO ('{end}')",
                suffix(*span, start)
            );
            use sqlx::Connection;
            let mut tx = conn.begin().await?;
            // Serialize this parent's name check and creation without introducing
            // one startup transaction across independent producer locks.
            crate::queries::chats::lock_named(&mut tx, &format!("time_partitions:{table}")).await?;
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            start = end;
        }
    }
    Ok(())
}

/// Ensures one UTC day of fine measurements exists before an independent producer appends to it.
pub async fn ensure_metric_partition(conn: &mut Conn, day: NaiveDate) -> Result<(), DbError> {
    let name = format!("metric_samples_p{}", day.format("%Y%m%d"));
    let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(&name)
        .fetch_one(&mut *conn)
        .await?;
    if exists {
        return Ok(());
    }
    use sqlx::Connection;
    let mut tx = conn.begin().await?;
    // Producers and daily upkeep share this lock, so concurrent first observations cannot race in the catalogues.
    crate::queries::chats::lock_named(&mut tx, &format!("metric_partition:{name}")).await?;
    let statement = format!(
        "CREATE TABLE IF NOT EXISTS {name} PARTITION OF metric_samples FOR VALUES FROM ('{day}') TO ('{}')",
        next(Span::Day, day)
    );
    sqlx::query(sqlx::AssertSqlSafe(statement))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Drops the partitions of `table` that end at or before `before`, and
/// says how many went. A table that is not partitioned here is left alone.
pub async fn drop_partitions_before(
    conn: &mut Conn,
    table: &str,
    before: DateTime<Utc>,
) -> Result<u32, DbError> {
    let Some((table, span)) = PARTITIONED.iter().find(|(name, _)| *name == table) else {
        return Ok(0);
    };
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT child.relname::text FROM pg_inherits \
         JOIN pg_class parent ON parent.oid = inhparent JOIN pg_class child ON child.oid = inhrelid \
         WHERE parent.relname = $1 ORDER BY child.relname",
    )
    .bind(*table)
    .fetch_all(&mut *conn)
    .await?;
    let mut dropped = 0;
    for name in names {
        let Some(text) = name.strip_prefix(&format!("{table}_p")) else {
            continue;
        };
        let start = match span {
            Span::Day => NaiveDate::parse_from_str(text, "%Y%m%d").ok(),
            Span::Month => NaiveDate::parse_from_str(&format!("{text}01"), "%Y%m%d").ok(),
            Span::Year => NaiveDate::parse_from_str(&format!("{text}0101"), "%Y%m%d").ok(),
        };
        let Some(start) = start else { continue };
        if next(*span, start) <= before.date_naive() {
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE IF EXISTS {name}")))
                .execute(&mut *conn)
                .await?;
            dropped += 1;
        }
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_testkit::database::TestDb;

    #[tokio::test]
    async fn a_measurement_is_stored_once_its_partition_exists_and_old_days_can_be_dropped() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 0).unwrap();
        let insert =
            "INSERT INTO metric_samples (name, labels, at, value) VALUES ('m', '{}', $1, 1)";
        assert!(
            sqlx::query(insert)
                .bind(now)
                .execute(&mut *conn)
                .await
                .is_err()
        );

        ensure_partitions(&mut conn, now).await.unwrap();
        ensure_partitions(&mut conn, now).await.unwrap();
        sqlx::query(insert)
            .bind(now)
            .execute(&mut *conn)
            .await
            .unwrap();
        // The next day is in the next year: both tables are ready for it.
        let tomorrow = now + Duration::minutes(2);
        sqlx::query(insert)
            .bind(tomorrow)
            .execute(&mut *conn)
            .await
            .unwrap();

        let dropped = drop_partitions_before(&mut conn, "metric_samples", tomorrow)
            .await
            .unwrap();
        assert_eq!(dropped, 1);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM metric_samples")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(left, 1);
    }
}
