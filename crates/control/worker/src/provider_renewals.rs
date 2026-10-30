//! Starting a stopped provider's runtime so that its harness renews the sign-in it holds, once it has been stopped for as long as the setting says.
//!
//! A subscription's login is a token that its harness refreshes while it runs. A unit that stays
//! stopped for weeks lets the refresh lapse, and the provider then has to be signed in by hand. So the
//! worker looks, on the interval of the job `provider_renew`, at native sign-ins with running
//! or stopped units or an available directory runtime. It queues renewal when fewer than `runtime.renew_days` remain, or after that
//! interval since the last successful renewal when expiry is unknown. A stopped runtime may start;
//! a running runtime is reused and renewal never stops or replaces it. The worker talks to no machine.

use crate::schedule::{self, Job};
use chrono::{DateTime, Duration, Utc};
use orch_db::queries::jobs::provider_jobs;
use orch_db::queries::resources::provider_runtime as rows;
use orch_db::{Conn, Db, DbError};
use orch_settings::Place;
use orch_types::Clock;

/// How many stopped runtimes one run takes up.
const BATCH: i64 = 200;
/// The setting that bounds remaining life or the interval between unknown-expiry renewals.
const RENEW_DAYS: &str = "runtime.renew_days";

/// Why a run could not look at the runtimes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A setting could not be read.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
}

/// What a run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Renewed {
    /// Stopped runtimes that were looked at.
    pub looked_at: u32,
    /// Jobs that were queued to start one.
    pub queued: u32,
}

/// One run of the job: nothing when another worker holds its lock, else what was done.
pub async fn scheduled(db: &Db, clock: &dyn Clock) -> Result<Option<Renewed>, Error> {
    let Some(mut tx) = schedule::take(db, Job::ProviderRenew).await? else {
        return Ok(None);
    };
    let done = renew_due(&mut tx, clock.now()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(done))
}

/// Queues every sign-in due by its expiry or its last successful renewal, in complete bounded pages.
/// A runtime that has a job under way is left to it.
pub async fn renew_due(conn: &mut Conn, now: DateTime<Utc>) -> Result<Renewed, Error> {
    let mut done = Renewed::default();
    let mut after = None;
    loop {
        let page = rows::renewal_candidates(conn, after, BATCH).await?;
        let more = i64::try_from(page.len()).is_ok_and(|count| count == BATCH);
        after = page.last().map(|candidate| candidate.provider_id);
        for candidate in page {
            done.looked_at += 1;
            let days: i64 =
                orch_settings::get(conn, Place::Provider(candidate.provider_id), RENEW_DAYS)
                    .await?
                    .unwrap_or(0);
            if days <= 0 {
                continue;
            }
            let interval = Duration::try_days(days).unwrap_or(Duration::MAX);
            let due = candidate.expires_at.map_or_else(
                || {
                    candidate
                        .since
                        .checked_add_signed(interval)
                        .is_some_and(|due| due <= now)
                },
                |expires| expires - now < interval,
            );
            if !due {
                continue;
            }
            if !orch_settings::provider_sign_in(conn, candidate.provider_id)
                .await?
                .allowed
            {
                continue;
            }
            if let provider_jobs::Queued::Made(job) =
                provider_jobs::queue_renewal(conn, candidate.provider_id, now).await?
            {
                tracing::info!(provider = %candidate.provider_id, job = %job.id, days, "a native sign-in is due for renewal");
                done.queued += 1;
            }
        }
        if !more {
            break;
        }
    }
    Ok(done)
}
