//! The local database path for reading the effective address and recovering the trusted file seed.

use super::console::{finish, say};
use super::host::{self, Host, HostError, Sealing};
use clap::{Args, Subcommand};
use orch_db::{
    DbError,
    audit::{self, Actor, Entry},
    queries::public_addresses,
};
use orch_types::{Clock, SystemClock};
use std::process::ExitCode;

#[derive(Debug, Args)]
pub struct Options {
    #[command(flatten)]
    host: Host,
    #[command(flatten)]
    sealing: Sealing,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print only the primary address currently in force, for local readiness scripts.
    Get,
    /// Clear the stored override and republish the validated file seed through the local database.
    Reset,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(transparent)]
    Database(#[from] DbError),
    #[error(transparent)]
    Certificate(#[from] orch_pki::PkiError),
    #[error("the new file seed could not be prepared: {0}")]
    Preparation(String),
    #[error("system.{field} {message}")]
    InvalidSeed {
        field: &'static str,
        message: String,
    },
    #[error("cannot write the result: {0}")]
    Output(#[from] std::io::Error),
}

async fn work(options: &Options) -> Result<(), Error> {
    let deployment = options.host.connect().await?;
    let seed = deployment.config.system.address_seed();
    let now = SystemClock.now();
    match options.command {
        Command::Get => {
            let mut conn = deployment.db.acquire().await?;
            let current = public_addresses::read(&mut conn, &seed, now).await?;
            say(current.addresses.public_url)?;
        }
        Command::Reset => {
            let seed = seed.validated().map_err(|error| Error::InvalidSeed {
                field: error.field,
                message: error.to_string(),
            })?;
            let (known, current) = {
                let mut conn = deployment.db.acquire().await?;
                let previous_seed = public_addresses::seed(&mut conn).await?;
                let known_seed = previous_seed
                    .as_ref()
                    .is_none_or(|previous| previous == &seed);
                let current = if known_seed {
                    None
                } else {
                    Some(public_addresses::read(&mut conn, &seed, now).await?)
                };
                (
                    known_seed
                        || current
                            .as_ref()
                            .is_some_and(|current| current.addresses == seed),
                    current,
                )
            };
            let mut prepared = if known {
                None
            } else {
                use orch_api_core::{
                    certificates::{CertificateService, CertificateSettings},
                    public_addresses::{PrepareOptions, Preparer},
                };
                let authority = std::sync::Arc::new(
                    orch_pki::ca::CaFiles::in_dir(&options.sealing.secrets_dir).load()?,
                );
                let clock: std::sync::Arc<dyn Clock> = std::sync::Arc::new(SystemClock);
                let service = CertificateService::new(
                    deployment.db.clone(),
                    std::sync::Arc::clone(&clock),
                    authority,
                    CertificateSettings::from_config(&deployment.config),
                );
                let managed = std::env::var_os("ORCH_FRONTEND_HTTPS_BIND")
                    .is_some_and(|value| !value.is_empty());
                let options = PrepareOptions::read(&deployment.db, managed)
                    .await
                    .map_err(|error| Error::Preparation(error.problem.detail))?;
                let revision = current
                    .as_ref()
                    .map(|current| current.revision)
                    .ok_or_else(|| {
                        Error::Preparation("the existing revision is missing".to_owned())
                    })?;
                Some(
                    Preparer {
                        db: &deployment.db,
                        config: &deployment.config,
                        clock: clock.as_ref(),
                        certificates: &service,
                    }
                    .prepare(seed.clone(), revision, options)
                    .await
                    .map_err(|error| Error::Preparation(error.problem.detail))?,
                )
            };
            let now = SystemClock.now();
            let mut tx = deployment.db.begin().await?;
            let current = match prepared.as_mut() {
                Some(prepared) => prepared
                    .reset(&mut tx, now)
                    .await
                    .map_err(|error| Error::Preparation(error.problem.detail))?,
                None => public_addresses::reset(&mut tx, &seed, now).await?,
            };
            let mut entry = Entry::new(
                Actor::System,
                orch_db::audit::name!("system.settings_updated"),
                "system",
                None,
                "Public addresses restored from the file seed",
            );
            entry.details = serde_json::json!({"key": public_addresses::KEY, "revision": current.revision, "public_url": current.addresses.public_url, "alternate_urls": current.addresses.alternate_urls, "from_file_seed": true});
            audit::append(&mut tx, &entry, &host::origin(), now).await?;
            tx.commit().await.map_err(DbError::from)?;
            if let Some(prepared) = prepared {
                if prepared.release().await.is_err() {
                    super::console::note(
                        "the public address was committed; its preparation connection closed",
                    );
                }
            }
            say(format!(
                "Public address reset to {} (revision {})",
                current.addresses.public_url, current.revision
            ))?;
        }
    }
    Ok(())
}

pub fn run(options: &Options) -> ExitCode {
    finish(
        host::block_on(work(options))
            .map_err(Error::from)
            .and_then(|done| done),
    )
}
