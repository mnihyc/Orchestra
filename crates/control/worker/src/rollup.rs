//! The fold of metric samples into hours, which are kept.
//!
//! The minute values of the measurements are kept for a few days only
//! (`retention.metrics_fine_days`); what is kept for good is their hour. Each
//! hour that has ended is folded into one row for each series: the mean of its
//! minute values, the largest, their sum (what a counter grew by in the hour)
//! and how many minutes had a value. The fold runs every hour under the lock of
//! its job, begins at the newest hour that has a row (which it makes again,
//! since a minute may have arrived after that hour was folded) and goes on to
//! the hour before the one that is running, so that a worker that was away for
//! a day folds the day when it is back. `metrics_retention` drops a minute only
//! once its hour is here.

use crate::schedule::{self, Job};
use orch_db::queries::notifications::upkeep;
use orch_db::{Db, DbError};
use orch_types::Clock;

/// What a run of `metrics_rollup` did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Folded {
    /// Hourly figures written, one for each series and hour.
    pub hours: u64,
}

/// Folds the hours of minute values that have ended into the hourly figures,
/// under the lock of the job. None where another run holds the lock.
pub async fn metric_hours(db: &Db, clock: &dyn Clock) -> Result<Option<Folded>, DbError> {
    let Some(mut tx) = schedule::take(db, Job::MetricsRollup).await? else {
        return Ok(None);
    };
    let since = upkeep::newest_folded_hour(&mut tx).await?;
    let hours = upkeep::fold_metric_hours(&mut tx, since, clock.now()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(Folded { hours }))
}
