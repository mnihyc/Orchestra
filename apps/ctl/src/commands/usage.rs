//! Ledger-backed usage rollup rebuild; the rate catalogue loaded from a file, or put back as the release came with it.
//!
//! `usage catalogue <file>` loads a rate catalogue the way an admin's upload
//! does, by calling the very function the API's request calls: the file is
//! checked whole (its format, its unit, its date and every family with its
//! pattern and its rate) before anything is written, and a file with one bad
//! entry is refused, naming the entry, and changes nothing. A catalogue that
//! passes is in force at once, whatever the one it replaces was and whatever
//! its date, and the audit trail has its line as `rate_catalogue.loaded`,
//! written by the actor `system`.
//!
//! `usage catalogue --shipped` puts back the catalogue the running release
//! came with, one an admin gave or a newer one replaced included. The release
//! keeps it as `share/model-rates.json` beside its `migrations/`; the file
//! is looked for there, then beside the program, then where a checkout of the
//! repository has it, and `--model-rates` names it where it is none of these.
//!
//! `usage rebuild [--from <date>]` makes the rollups again from the ledger
//! alone: every daily row from that day on (all of them where no day is given)
//! and every monthly row from the month it is in. It calls the very function
//! that keeps the rollups as rows arrive, so that what it writes is what the
//! live tables would hold, and it runs in one transaction in which the writers
//! of the rollups wait, so that a reader never sees half a rebuild. Nothing in
//! the ledger changes; it is what to run after the ledger was repaired by hand
//! or a restore brought rows back.

use super::console::{finish, say};
use super::host::{self, Host, HostError};
use chrono::NaiveDate;
use clap::{Args, Subcommand};
use orch_api_contract::usage::USAGE_CATALOGUE_MAX_BYTES;
use orch_config::ServerConfig;
use orch_db::DbError;
use orch_db::audit::Actor;
use orch_db::seeds::{self, Loaded, SeedError, Taken};
use orch_db::usage::rollup;
use orch_types::{Clock, SystemClock};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Where a release keeps the catalogue it comes with, beside its programs and
/// its `migrations/`.
const SHIPPED_FILE: &str = "share/model-rates.json";

/// Where a checkout of the repository has it.
const CHECKOUT_FILE: &str = "deploy/model-rates.json";

/// What is done about the use of the system from the host.
#[derive(Debug, Args)]
pub struct UsageArgs {
    #[command(flatten)]
    host: Host,
    #[command(subcommand)]
    command: UsageCommand,
}

#[derive(Debug, Subcommand)]
enum UsageCommand {
    /// Load a rate catalogue from a file, checked whole as an upload is, or
    /// put back the one the release came with. It is in force at once.
    Catalogue(CatalogueArgs),
    /// Make the rollups again from the ledger alone, from a day on or all of
    /// them: the same rule that keeps them as rows arrive.
    Rebuild(RebuildArgs),
}

#[derive(Debug, Args)]
struct RebuildArgs {
    /// The first day to make again, written YYYY-MM-DD (UTC); every day of the
    /// ledger where none is given.
    #[arg(long, value_name = "DATE")]
    from: Option<String>,
}

#[derive(Debug, Args)]
struct CatalogueArgs {
    /// The catalogue, as JSON.
    #[arg(value_name = "FILE", required_unless_present = "shipped")]
    file: Option<PathBuf>,
    /// Put back the catalogue the release came with instead.
    #[arg(long, conflicts_with = "file")]
    shipped: bool,
    /// Where the release's own catalogue is, `model-rates.json`, where it is
    /// not where a release keeps it.
    #[arg(long, value_name = "FILE", requires = "shipped")]
    model_rates: Option<PathBuf>,
}

/// Why a catalogue was not loaded.
#[derive(Debug, thiserror::Error)]
enum UsageError {
    #[error(transparent)]
    Host(#[from] HostError),
    #[error("cannot read {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("{path} is larger than the {megabytes} MiB a catalogue may be")]
    TooLarge { path: String, megabytes: i64 },
    #[error("the release's catalogue was not found ({places}): give its file with --model-rates")]
    NoShipped { places: String },
    #[error("{source_name} is refused, and nothing was changed: {reason}")]
    Refused { source_name: String, reason: String },
    #[error("{0:?} is not a day: write --from as YYYY-MM-DD")]
    NotADay(String),
    #[error("the database failed: {0}")]
    Database(String),
    #[error("cannot write the answer: {0}")]
    Output(#[from] std::io::Error),
}

impl From<DbError> for UsageError {
    fn from(error: DbError) -> Self {
        Self::Database(error.to_string())
    }
}

/// The text of a catalogue file, which may be no larger than an upload.
fn read_file(path: &Path) -> Result<String, UsageError> {
    let unreadable = |error: std::io::Error| UsageError::Read {
        path: path.display().to_string(),
        reason: error.to_string(),
    };
    let length = std::fs::metadata(path).map_err(unreadable)?.len();
    if i64::try_from(length).unwrap_or(i64::MAX) > USAGE_CATALOGUE_MAX_BYTES {
        return Err(UsageError::TooLarge {
            path: path.display().to_string(),
            megabytes: USAGE_CATALOGUE_MAX_BYTES / (1024 * 1024),
        });
    }
    std::fs::read_to_string(path).map_err(unreadable)
}

/// The places a release's catalogue may be, in the order they are looked in.
fn shipped_places(config: &ServerConfig) -> Vec<PathBuf> {
    let mut places = Vec::new();
    // The release is laid out as `bin/`, `migrations/` and `share/`.
    if let Some(release) = Path::new(&config.database.migrations_dir).parent() {
        places.push(release.join(SHIPPED_FILE));
    }
    if let Some(bin) = std::env::current_exe()
        .ok()
        .and_then(|program| program.parent().map(Path::to_owned))
    {
        places.push(bin.join("..").join(SHIPPED_FILE));
        places.push(bin.join("..").join("..").join(CHECKOUT_FILE));
    }
    places.push(PathBuf::from(CHECKOUT_FILE));
    places
}

/// The file of the release's catalogue.
fn shipped_file(config: &ServerConfig, given: Option<&Path>) -> Result<PathBuf, UsageError> {
    if let Some(path) = given {
        return Ok(path.to_owned());
    }
    let places = shipped_places(config);
    places
        .iter()
        .find(|place| place.is_file())
        .cloned()
        .ok_or_else(|| UsageError::NoShipped {
            places: places
                .iter()
                .map(|place| place.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        })
}

/// What a refusal of the library comes to in the words of a command.
fn refused(source_name: &Path, error: SeedError) -> UsageError {
    match error {
        SeedError::Db(error) => error.into(),
        // The catalogue's member at fault is named before what is wrong with it, as the library says it.
        SeedError::Catalogue { path, reason } => UsageError::Refused {
            source_name: source_name.display().to_string(),
            reason: if path.is_empty() {
                reason
            } else {
                format!("{path}: {reason}")
            },
        },
        other => UsageError::Database(other.to_string()),
    }
}

fn plural(count: usize) -> String {
    if count == 1 {
        "1 family".to_owned()
    } else {
        format!("{count} families")
    }
}

/// What a catalogue that was loaded is told as.
fn told(loaded: &Loaded, what: &str) -> String {
    let replaced = loaded
        .replaced
        .map_or_else(String::new, |day| format!(", replacing the one of {day}"));
    format!(
        "{what} is in force now: figures of {}, {}{replaced}",
        loaded.as_of,
        plural(loaded.families)
    )
}

async fn catalogue(host: &Host, args: &CatalogueArgs) -> Result<(), UsageError> {
    let config = host.load_config()?;
    let (path, text) = if args.shipped {
        let path = shipped_file(&config, args.model_rates.as_deref())?;
        let text = read_file(&path)?;
        (path, text)
    } else {
        let path = args.file.clone().unwrap_or_default();
        let text = read_file(&path)?;
        (path, text)
    };
    let deployment = host.connect_to(config).await?;
    let origin = host::origin();
    let now = SystemClock.now();
    let mut tx = deployment.db.begin().await?;
    let loaded = if args.shipped {
        seeds::put_back_shipped_catalogue(&mut tx, &text, Actor::System, &origin, now).await
    } else {
        seeds::load_catalogue(&mut tx, &text, Taken::Uploaded, Actor::System, &origin, now).await
    }
    .map_err(|error| refused(&path, error))?;
    tx.commit().await.map_err(DbError::from)?;
    let what = if args.shipped {
        "the rate catalogue of the release".to_owned()
    } else {
        format!("the rate catalogue in {}", path.display())
    };
    say(told(&loaded, &what))?;
    Ok(())
}

async fn rebuild(host: &Host, args: &RebuildArgs) -> Result<(), UsageError> {
    let from = args
        .from
        .as_deref()
        .map(|text| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map_err(|_| UsageError::NotADay(text.to_owned()))
        })
        .transpose()?;
    let config = host.load_config()?;
    let deployment = host.connect_to(config).await?;
    let mut tx = deployment.db.begin().await?;
    let rebuilt = rollup::rebuild(&mut tx, from).await?;
    tx.commit().await.map_err(DbError::from)?;
    let from = from.map_or_else(String::new, |day| format!(" from {day}"));
    say(format!(
        "the rollups are made again from the ledger{from}: {} daily rows and {} monthly rows",
        rebuilt.daily, rebuilt.monthly
    ))?;
    Ok(())
}

async fn work(args: &UsageArgs) -> Result<(), UsageError> {
    match &args.command {
        UsageCommand::Catalogue(each) => catalogue(&args.host, each).await,
        UsageCommand::Rebuild(each) => rebuild(&args.host, each).await,
    }
}

/// Does what the command asks and says what came of it.
#[must_use]
pub fn run(args: &UsageArgs) -> ExitCode {
    finish(
        host::block_on(work(args))
            .map_err(UsageError::from)
            .and_then(|done| done),
    )
}
