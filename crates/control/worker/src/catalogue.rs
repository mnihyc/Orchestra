//! The rate catalogue fetched from where a system admin said, on its schedule, and checked whole before it replaces the one in force.
//!
//! Nothing is fetched unless a system admin set `usage.catalogue_url`, and
//! nothing is fetched from anywhere else: the one address, no redirect to
//! another place, an answer of at most 2 MiB. On its schedule (once a day) the
//! job looks only when `usage.catalogue_refresh_days` have passed since the
//! catalogue in force was loaded, and loads what it finds only when its
//! figures are newer; an admin who asks for a fetch gets what the address
//! gives, whatever its date. Either way the text is checked whole by the one
//! function that an upload and the command line of the host call
//! ([`orch_db::seeds::load_catalogue`]), and a catalogue with one wrong entry
//! is refused, naming the entry, and changes nothing.

use crate::schedule::{self, Job};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use orch_db::audit::{Actor, Origin};
use orch_db::seeds::{self, Loaded, SeedError, Taken};
use orch_db::usage::ledger;
use orch_db::{Db, DbError};
use orch_settings::Place;
use orch_types::Clock;
use std::time::Duration as Wait;

/// The setting that says where to fetch from.
const URL_KEY: &str = "usage.catalogue_url";
/// The setting that says how many days pass between two fetches; 0 is never.
const REFRESH_DAYS_KEY: &str = "usage.catalogue_refresh_days";
/// How large an answer may be: as large as an upload may be.
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// How long a fetch may take altogether: a catalogue is one small file.
const FETCH_TIMEOUT: Wait = Wait::from_secs(30);
/// How the worker introduces itself to the address.
const USER_AGENT: &str = "orch-worker";

/// Why a catalogue was not taken.
#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    /// Nobody set the address.
    #[error("usage.catalogue_url is not set")]
    NoAddress,
    /// The address did not give a catalogue.
    #[error("the catalogue could not be fetched: {0}")]
    Fetch(String),
    /// What the address gave is not a catalogue, or has an entry that is wrong; nothing was changed.
    #[error("{0}")]
    Refused(String),
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A setting could not be read.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
}

/// When a fetch is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    /// Whenever it is asked for, and whatever the date of what is found: an admin's request.
    Always,
    /// When the days of the setting have passed and what is found is newer: the schedule.
    WhenDue,
}

/// What a fetch came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not yet: the days between two fetches have not passed, or none is wanted.
    NotDue,
    /// What the address gives is not newer than what is in force; nothing was changed.
    NotNewer {
        /// The date of the figures that were fetched.
        fetched: NaiveDate,
        /// The date of the ones in force.
        in_force: NaiveDate,
    },
    /// The catalogue is in force.
    Loaded(Loaded),
}

/// Fetches the text at `url`: the one address, no redirect, at most 2 MiB.
pub async fn fetch_text(url: &str) -> Result<String, CatalogueError> {
    let fail = |what: String| CatalogueError::Fetch(what);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(FETCH_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| fail(error.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| fail(error.to_string()))?;
    if !response.status().is_success() {
        return Err(fail(format!("the address answered {}", response.status())));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| fail(error.to_string()))?
    {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err(fail(
                "the answer is larger than a catalogue may be".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| fail("the answer is not text".to_owned()))
}

/// The date of the figures that a catalogue text says it has, where it says one.
fn as_of_of(text: &str) -> Option<NaiveDate> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()?
        .get("as_of")?
        .as_str()
        .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
}

/// Whether a fetch is due: the days between two have passed since the catalogue in force was loaded.
fn is_due(now: DateTime<Utc>, days: i64, loaded: Option<DateTime<Utc>>) -> bool {
    days > 0 && loaded.is_none_or(|at| now - at >= Duration::days(days))
}

/// Fetches the catalogue from the address that was set and puts it in force as `by`, as `wanted` says.
pub async fn refresh(
    db: &Db,
    clock: &dyn Clock,
    by: Actor,
    origin: &Origin,
    wanted: Wanted,
) -> Result<Outcome, CatalogueError> {
    let now = clock.now();
    let (url, days, in_force) = {
        let mut conn = db.acquire().await?;
        let url = orch_settings::get::<String>(&mut conn, Place::System, URL_KEY).await?;
        let days = orch_settings::get::<i64>(&mut conn, Place::System, REFRESH_DAYS_KEY)
            .await?
            .unwrap_or(0);
        (url, days, ledger::catalogue_in_force(&mut conn).await?)
    };
    let url = url
        .filter(|url| !url.trim().is_empty())
        .ok_or(CatalogueError::NoAddress)?;
    if wanted == Wanted::WhenDue && !is_due(now, days, in_force.as_ref().map(|row| row.loaded_at)) {
        return Ok(Outcome::NotDue);
    }
    let text = fetch_text(&url).await?;
    if wanted == Wanted::WhenDue
        && let (Some(fetched), Some(held)) = (as_of_of(&text), in_force.as_ref())
        && fetched <= held.as_of
    {
        return Ok(Outcome::NotNewer {
            fetched,
            in_force: held.as_of,
        });
    }
    let mut tx = db.begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let taken = Taken::Fetched { from: &url };
    let loaded = seeds::load_catalogue(&mut tx, &text, taken, by, origin, now)
        .await
        .map_err(|error| match error {
            SeedError::Db(error) => CatalogueError::Db(error),
            other => CatalogueError::Refused(other.to_string()),
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Outcome::Loaded(loaded))
}

/// The scheduled look, under the lock of the job: nothing where another run
/// holds the lock, where no address was set, or where a fetch is not due yet. An
/// address that gives something that is not a catalogue is an error, which the
/// job's log tells, since nobody asked for it and nothing was changed.
pub async fn scheduled(db: &Db, clock: &dyn Clock) -> Result<Option<Outcome>, CatalogueError> {
    // The job's lease is held while the fetch is made, so that a second worker finds it taken and
    // does not fetch the same file. A lease and not a lock in a transaction: no transaction stays
    // open while the fetch waits on the network.
    let Some(lease) = schedule::take_lease(db, Job::RateCatalogue, clock).await? else {
        return Ok(None);
    };
    let outcome = refresh(
        db,
        clock,
        Actor::System,
        &Origin::default(),
        Wanted::WhenDue,
    )
    .await;
    lease.release(db).await?;
    match outcome {
        Ok(outcome) => Ok(Some(outcome)),
        Err(CatalogueError::NoAddress) => Ok(None),
        Err(error) => Err(error),
    }
}
