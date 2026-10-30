//! Raw-event partition retention, the minutes of stored metric samples once their hours are folded, and recents trimming.
//!
//! Three jobs, each daily. `raw_events` drops the partitions of raw protocol
//! frames that ended before `diagnostics.retention_days` ago, trims the rows
//! older than that from the one partition that is only partly old, and makes
//! sure that the partitions for the days and months to come exist.
//! `metrics_retention` drops the day-partitions of the minute values older
//! than `[retention] metrics_fine_days`, but only once the hours of those days
//! are folded into the figures that are kept (a day with a minute whose hour is
//! not folded stays, and so does everything after it), and removes the hourly
//! figures older than `[retention] metrics_days`, where that is not 0.
//! `recents_trim` keeps each person's `[limits] recents_per_user` newest
//! recent visits, and removes the records of authority changes that every
//! snapshot sees, a day after they were written (`access_changes`).

use crate::schedule::{self, Job};
use crate::spans;
use chrono::{DateTime, Utc};
use orch_config::ServerConfig;
use orch_db::queries::notifications::upkeep;
use orch_db::{Db, DbError, migrations};
use orch_settings::Place;
use orch_types::Clock;

/// How many days raw events are kept when the setting has no value: the
/// setting's own default.
const DEFAULT_RAW_EVENT_DAYS: i64 = 14;

/// The fewest days raw events are kept, whatever a row of the database says: a
/// value that is not a number of days, or is none, never makes the cutoff now.
const LEAST_RAW_EVENT_DAYS: i64 = 1;

/// Why a retention job failed.
#[derive(Debug, thiserror::Error)]
pub enum RetentionError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A setting could not be read.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
}

/// What a run of `raw_events` did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawEvents {
    /// Partitions dropped.
    pub partitions_dropped: u32,
    /// Rows removed from the partition that was only partly old.
    pub rows_trimmed: u64,
}

/// Makes the partitions to come, drops the raw events that are past their days
/// and trims the rest, under the lock of the job. None where another run holds
/// the lock.
pub async fn raw_events(db: &Db, clock: &dyn Clock) -> Result<Option<RawEvents>, RetentionError> {
    let Some(mut tx) = schedule::take(db, Job::RawEvents).await? else {
        return Ok(None);
    };
    let now = clock.now();
    let days = orch_settings::get::<i64>(&mut tx, Place::System, "diagnostics.retention_days")
        .await?
        .unwrap_or(DEFAULT_RAW_EVENT_DAYS);
    let cutoff = now - spans::days(days.max(LEAST_RAW_EVENT_DAYS).unsigned_abs());
    migrations::ensure_partitions(&mut tx, now).await?;
    let partitions_dropped =
        migrations::drop_partitions_before(&mut tx, "raw_events", cutoff).await?;
    let rows_trimmed = upkeep::trim_raw_events(&mut tx, cutoff).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(RawEvents {
        partitions_dropped,
        rows_trimmed,
    }))
}

/// What a run of `metrics_retention` did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricMinutes {
    /// Day-partitions of minute values dropped.
    pub partitions_dropped: u32,
    /// Hourly figures removed.
    pub hours_removed: u64,
}

/// The moment before which the minutes of metric samples may be dropped: the
/// end of the days that are old enough, and no later than the start of the
/// first day that has a minute whose hour is not folded.
#[must_use]
pub fn fine_cutoff(
    now: DateTime<Utc>,
    fine_days: u64,
    first_unfolded_day: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    if fine_days == 0 {
        return None;
    }
    let old_enough = now - spans::days(fine_days);
    Some(first_unfolded_day.map_or(old_enough, |day| day.min(old_enough)))
}

/// Drops the minutes that are old and folded, and the hours that are past their
/// days, under the lock of the job. None where another run holds the lock.
pub async fn metric_minutes(
    db: &Db,
    clock: &dyn Clock,
    config: &ServerConfig,
) -> Result<Option<MetricMinutes>, RetentionError> {
    let Some(mut tx) = schedule::take(db, Job::MetricsRetention).await? else {
        return Ok(None);
    };
    let now = clock.now();
    let mut done = MetricMinutes::default();
    if config.retention.metrics_fine_days > 0 {
        let old_enough = now - spans::days(config.retention.metrics_fine_days);
        let unfolded = upkeep::first_unfolded_day(&mut tx, old_enough).await?;
        if let Some(before) = fine_cutoff(now, config.retention.metrics_fine_days, unfolded) {
            done.partitions_dropped =
                migrations::drop_partitions_before(&mut tx, "metric_samples", before).await?;
        }
    }
    if config.retention.metrics_days > 0 {
        let before = now - spans::days(config.retention.metrics_days);
        done.hours_removed = upkeep::drop_metric_hours_before(&mut tx, before).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(done))
}

/// Keeps each person's newest recent visits to their share, under the lock of
/// the job. None where another run holds the lock.
pub async fn recents(db: &Db, config: &ServerConfig) -> Result<Option<u64>, RetentionError> {
    let Some(mut tx) = schedule::take(db, Job::RecentsTrim).await? else {
        return Ok(None);
    };
    let removed = upkeep::trim_recents(&mut tx, i64::from(config.limits.recents_per_user)).await?;
    // The records of authority changes that every snapshot sees, past a day: kept holder sets no longer need them.
    orch_db::queries::access::kept::prune(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn minutes_are_dropped_when_old_enough_and_never_past_a_day_whose_hours_are_not_folded() {
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let thirty_days_ago = Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
        assert_eq!(fine_cutoff(now, 30, None), Some(thirty_days_ago));
        let behind = Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap();
        assert_eq!(
            fine_cutoff(now, 30, Some(behind)),
            Some(behind),
            "the rollup is behind: keep from there"
        );
        let recent = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
        assert_eq!(
            fine_cutoff(now, 30, Some(recent)),
            Some(thirty_days_ago),
            "a gap newer than the cutoff does not matter"
        );
        assert_eq!(
            fine_cutoff(now, 0, None),
            None,
            "0 keeps every minute for good"
        );
    }
}
