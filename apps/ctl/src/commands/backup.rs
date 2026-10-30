//! The existing updater's database-only snapshot, completed manifest and metadata-only record repair.

use super::console::{finish, say};
use super::host::{self, Host};
use clap::{Args, Subcommand, ValueEnum};
use orch_api_contract::admin::BackupCause;
use orch_db::queries::installation;
use orch_pki::certificates::Certificate;
use orch_types::{Clock, SystemClock};
use orch_worker_core::backups::{self, Dump, Manifest};
use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tokio::process::Command;

/// Operator arguments; neither HTTP nor a browser can post a successful backup timestamp.
#[derive(Debug, Args)]
pub struct BackupArgs {
    #[command(flatten)]
    host: Host,
    /// The directory of the installation's public authority certificate.
    #[arg(
        long,
        env = "ORCH_SECRETS_DIR",
        default_value = "/run/secrets",
        global = true
    )]
    secrets_dir: PathBuf,
    #[command(subcommand)]
    command: Action,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Cause {
    Manual,
    PreUpdate,
}

impl From<Cause> for BackupCause {
    fn from(value: Cause) -> Self {
        match value {
            Cause::Manual => Self::Manual,
            Cause::PreUpdate => Self::PreUpdate,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Take the one dump from an exported snapshot, inventory it, validate it, then publish its record.
    Dump {
        /// New dump path; an existing pair is never overwritten.
        #[arg(long)]
        out: PathBuf,
        /// Protected updater metadata directory, mounted read-only into the API.
        #[arg(long)]
        records_dir: PathBuf,
        /// The installed release at this database point.
        #[arg(long)]
        release: String,
        /// Manual backup or pre-update save.
        #[arg(long, value_enum)]
        cause: Cause,
    },
    /// Re-publish metadata from an already verified pair, with no new pg_dump invocation.
    Record {
        /// Completed custom-format dump.
        #[arg(long)]
        dump: PathBuf,
        /// Its completed matching inventory manifest.
        #[arg(long)]
        manifest: PathBuf,
        /// Protected updater metadata directory.
        #[arg(long)]
        records_dir: PathBuf,
    },
}

fn identity(dir: &Path) -> Result<String, String> {
    let pem = fs::read_to_string(dir.join("orch_ca_cert.pem"))
        .map_err(|_| "the installation's public authority certificate is unreadable".to_owned())?;
    Certificate::from_pem(&pem)
        .map(|certificate| certificate.fingerprint())
        .map_err(|_| "the installation's public authority certificate does not read".to_owned())
}

fn manifest_path(dump: &Path) -> PathBuf {
    dump.with_extension("manifest.json")
}

fn decode(value: &str) -> String {
    let encoded = format!("v={}", value.replace('+', "%2B"));
    url::form_urlencoded::parse(encoded.as_bytes())
        .next()
        .map_or_else(String::new, |(_, value)| value.into_owned())
}

fn pg(database: &str, program: &str) -> Result<Command, String> {
    let url = url::Url::parse(database)
        .map_err(|_| "the configured database address does not read".to_owned())?;
    let mut command = Command::new(program);
    // Password travels in the child environment, never in argv, a manifest or a diagnostic.
    command
        .env("PGHOST", url.host_str().unwrap_or("localhost"))
        .env("PGPORT", url.port().unwrap_or(5432).to_string())
        .env("PGUSER", decode(url.username()))
        .env("PGDATABASE", decode(url.path().trim_start_matches('/')))
        .env("PGPASSWORD", decode(url.password().unwrap_or_default()))
        .kill_on_drop(true);
    Ok(command)
}

fn config_inventory(config: &Path, secrets: &Path) -> serde_json::Value {
    let paths = [
        ("config/orch.toml", config.to_owned()),
        ("config/secrets", secrets.to_owned()),
        (".env", PathBuf::from(".env")),
        ("runtime/node", PathBuf::from("runtime/node")),
    ];
    json!({"count":paths.len(), "included":false, "reason":"configuration, keys, CA and node identity require an encrypted host copy",
        "entries":paths.iter().map(|(location,path)| json!({"location":location,
            "location_status":if path.exists() {"known"} else {"unknown"}})).collect::<Vec<_>>()})
}

async fn dump(
    args: &BackupArgs,
    out: &Path,
    records: &Path,
    release: &str,
    cause: Cause,
) -> Result<(), String> {
    let deployment = args
        .host
        .connect()
        .await
        .map_err(|_| "the database is unavailable for its snapshot".to_owned())?;
    let system_id = identity(&args.secrets_dir)?;
    let manifest_file = manifest_path(out);
    let partial = out.with_extension("dump.partial");
    if out.exists() || manifest_file.exists() || partial.exists() {
        return Err(
            "that dump name is already in use; choose a new name without overwriting a pair"
                .to_owned(),
        );
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&partial)
        .map_err(|_| "the private dump staging file could not be made".to_owned())?;
    let mut tx = deployment
        .db
        .begin()
        .await
        .map_err(|_| "the database snapshot could not begin".to_owned())?;
    let snapshot = installation::export_snapshot(&mut tx)
        .await
        .map_err(|_| "the database snapshot inventory could not be read".to_owned())?;
    let result = pg(&deployment.config.database.url, "pg_dump")?
        .arg("--format=custom")
        .arg("--no-owner")
        .arg("--snapshot")
        .arg(&snapshot.id)
        .arg("--file")
        .arg(&partial)
        .output()
        .await
        .map_err(|_| "pg_dump could not be started".to_owned())?;
    if !result.status.success() {
        return Err("pg_dump failed; no completed pair was published".to_owned());
    }
    // Keep the exporting transaction alive until the consumer has finished its matching snapshot.
    tx.commit()
        .await
        .map_err(|_| "the exported snapshot could not be completed".to_owned())?;
    let checked = pg(&deployment.config.database.url, "pg_restore")?
        .arg("--list")
        .arg(&partial)
        .output()
        .await
        .map_err(|_| "pg_restore could not validate the dump".to_owned())?;
    if !checked.status.success() {
        return Err("the dump cannot be read back; no completed pair was published".to_owned());
    }
    let (bytes, hash) = backups::hash_file(&partial).map_err(|error| error.to_string())?;
    if bytes == 0 {
        return Err("the dump is empty; no completed pair was published".to_owned());
    }
    File::open(&partial)
        .and_then(|file| file.sync_all())
        .map_err(|_| "the dump could not be synced".to_owned())?;
    let name = out
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "the dump needs a relative file name".to_owned())?;
    let mut excluded = snapshot.excluded;
    excluded["configuration"] = config_inventory(&args.host.config, &args.secrets_dir);
    let manifest = Manifest {
        format_version: 1,
        system_id: system_id.clone(),
        release: release.to_owned(),
        schema: snapshot.schema.to_string(),
        database_at: snapshot.at.into(),
        completed_at: SystemClock.time(),
        cause: cause.into(),
        dump: Dump {
            name: name.to_owned(),
            bytes,
            hash,
        },
        included: json!({"database":{"count":1,"included":true,"location":name,"location_status":"known",
            "reason":"validated custom-format dump and inventory use one exported PostgreSQL snapshot"}}),
        excluded,
    };
    // A link creates the completed name only if absent. A cut before the manifest leaves no complete pair.
    fs::hard_link(&partial, out).map_err(|_| {
        "the completed dump name could not be published without replacing a file".to_owned()
    })?;
    backups::atomic(
        &manifest_file,
        &serde_json::to_vec_pretty(&manifest)
            .map_err(|_| "the manifest could not be encoded".to_owned())?,
    )
    .map_err(|error| error.to_string())?;
    fs::remove_file(&partial)
        .map_err(|_| "the completed dump's staging name could not be removed".to_owned())?;
    if let Err(error) = backups::publish_pair(records, out, &manifest_file, &system_id) {
        return Err(format!(
            "the database dump pair completed, but its Overview record did not: {error}; run ./update.sh backup --repair-record {name}"
        ));
    }
    say(format!(
        "Database backup complete: {name} ({bytes} bytes), with {}.",
        manifest_file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ))
    .map_err(|error| error.to_string())?;
    say("Blobs, configuration, credentials and execution volumes are not captured. Restore list: docs/operator/backup-and-restore.md")
        .map_err(|error| error.to_string())?;
    for (category, entry) in manifest.excluded.as_object().into_iter().flatten() {
        say(format!(
            "{category}: not included; count {}; {}",
            entry.get("count").unwrap_or(&serde_json::Value::Null),
            entry
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("inspect the recorded locations")
        ))
        .map_err(|error| error.to_string())?;
        for place in entry
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            say(format!("  {}", place)).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

async fn work(args: &BackupArgs) -> Result<(), String> {
    let (records, result) = match &args.command {
        Action::Dump {
            out,
            records_dir,
            release,
            cause,
        } => (
            records_dir,
            dump(args, out, records_dir, release, *cause).await,
        ),
        Action::Record {
            dump,
            manifest,
            records_dir,
        } => {
            let system_id = identity(&args.secrets_dir)?;
            let result = backups::publish_pair(records_dir, dump, manifest, &system_id)
                .map_err(|error| error.to_string())
                .and_then(|_| say("The Overview record was repaired from the completed pair. No new dump was taken.").map_err(|error| error.to_string()));
            (records_dir, result)
        }
    };
    if result.is_err() {
        let _ = backups::failed(
            records,
            SystemClock.time(),
            "the latest local backup or metadata publication failed; inspect the command's output",
        );
    }
    result
}

/// Runs only on the installation host, using the existing backup directory and PostgreSQL tools.
#[must_use]
pub fn run(args: &BackupArgs) -> ExitCode {
    finish(
        host::block_on(work(args))
            .map_err(|error| error.to_string())
            .and_then(|result| result),
    )
}
