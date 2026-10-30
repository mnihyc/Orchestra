//! What a command that works on the host is given: the configuration the API reads, the directory of its secrets, and the database.
//!
//! These commands are run where the stack runs (`docker compose exec api
//! orchctl …`), with the database address and the secrets the API has: the
//! configuration is the same file, whose `database.url` may be given by
//! `ORCH_DATABASE_URL` as everywhere else, and the secrets are the directory
//! the API was given. They never start or stop a service, and they do what a
//! request of the API would do by calling the same functions of the same
//! libraries; they only have no person signed in, so what they write is done
//! by the actor `system`.

use super::bootstrap::MASTER_KEY_FILE;
use clap::Args;
use orch_config::{ConfigError, SecretError, ServerConfig};
use orch_db::audit::Origin;
use orch_db::queries::notifications::upkeep;
use orch_db::secrets::MasterKey;
use orch_db::{Db, DbError};
use std::future::Future;
use std::path::PathBuf;

/// Where a deployment keeps the file the API reads, in the container that
/// runs it.
pub const DEFAULT_CONFIG: &str = "/etc/orch/orch.toml";

/// Where a container finds the secrets it was given: the directory every program of the deployment looks in.
pub const DEFAULT_SECRETS_DIR: &str = orch_config::DEFAULT_SECRETS_DIR;

/// How many connections a command opens: it does one thing at a time inside
/// one transaction, and the second is for the rare statement outside it.
const CONNECTIONS: u32 = 2;

/// What every command that works on the host is told where to find.
#[derive(Debug, Clone, Args)]
pub struct Host {
    /// The deployment's configuration: the file the API reads. The address of
    /// the database is in it, or in `ORCH_DATABASE_URL`.
    #[arg(
        long,
        global = true,
        env = "ORCH_CONFIG",
        value_name = "FILE",
        default_value = DEFAULT_CONFIG
    )]
    pub config: PathBuf,
}

/// Where a command that seals or opens a secret finds the key to do it with.
#[derive(Debug, Clone, Args)]
pub struct Sealing {
    /// The directory of the deployment's secrets: the master key that seals
    /// what the database stores is read from it.
    #[arg(
        long,
        global = true,
        env = "ORCH_SECRETS_DIR",
        value_name = "DIR",
        default_value = DEFAULT_SECRETS_DIR
    )]
    pub secrets_dir: PathBuf,
}

/// Why a command could not even begin.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The configuration cannot be read or does not hold together.
    #[error(
        "{0}. The configuration is the file the API reads: give it with --config or ORCH_CONFIG"
    )]
    Config(#[from] ConfigError),
    /// No address of a database.
    #[error(
        "the address of the database is not set: give database.url in the configuration, or ORCH_DATABASE_URL"
    )]
    NoDatabase,
    /// The database does not answer.
    #[error(
        "cannot connect to the database: {0}. Is it up, and is database.url (or ORCH_DATABASE_URL) right?"
    )]
    Connect(DbError),
    /// The database is there and holds no schema.
    #[error("the database has no schema yet: start the API once, which applies it")]
    NoSchema,
    /// The master key cannot be read.
    #[error(
        "{0}. The master key seals what the database stores: give the directory that holds orch_master_key with --secrets-dir or ORCH_SECRETS_DIR"
    )]
    MasterKey(#[from] SecretError),
    /// The database failed while a command was working.
    #[error("the database failed: {0}")]
    Database(#[from] DbError),
    /// The program could not make what it runs on.
    #[error("cannot start: {0}")]
    Runtime(std::io::Error),
}

/// The deployment a command works on.
#[derive(Debug)]
pub struct Deployment {
    /// What the API is configured with.
    pub config: ServerConfig,
    /// The database, whose schema is there.
    pub db: Db,
}

impl Host {
    /// Reads the configuration: the file, with what the environment replaces
    /// in it.
    pub fn load_config(&self) -> Result<ServerConfig, HostError> {
        Ok(orch_config::load(&self.config)?)
    }

    /// Reads the configuration and connects to the database it names.
    pub async fn connect(&self) -> Result<Deployment, HostError> {
        let config = self.load_config()?;
        self.connect_to(config).await
    }

    /// Connects to the database a configuration that was read names.
    pub async fn connect_to(&self, config: ServerConfig) -> Result<Deployment, HostError> {
        if config.database.url.is_empty() {
            return Err(HostError::NoDatabase);
        }
        let db = Db::connect(&config.database.url, CONNECTIONS)
            .await
            .map_err(HostError::Connect)?;
        let mut conn = db.acquire().await.map_err(HostError::Connect)?;
        if upkeep::applied_schema_version(&mut conn).await?.is_none() {
            return Err(HostError::NoSchema);
        }
        drop(conn);
        Ok(Deployment { config, db })
    }
}

impl Sealing {
    /// The key that seals what the database stores, from the secrets
    /// directory.
    pub fn master_key(&self) -> Result<MasterKey, HostError> {
        let bytes = orch_config::read_secret_hex::<{ super::bootstrap::MASTER_KEY_BYTES }>(
            &self.secrets_dir.join(MASTER_KEY_FILE),
        )?;
        Ok(MasterKey::new(bytes))
    }
}

/// Where an audit row written by a command comes from: this program, which
/// has no network address.
#[must_use]
pub fn origin() -> Origin {
    Origin {
        as_admin: None,
        ip: None,
        user_agent: Some(format!("orchctl {}", env!("ORCH_RELEASE_VERSION"))),
        request_id: None,
    }
}

/// Runs the future of a command to its end on a runtime of its own: a command
/// does one thing, so one thread is all it needs. A program whose `main` is
/// already inside a runtime may not start another on that thread, so there the
/// command runs on a thread of its own.
pub fn block_on<T: Send>(future: impl Future<Output = T> + Send) -> Result<T, HostError> {
    let run = move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(HostError::Runtime)?;
        Ok(runtime.block_on(future))
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return run();
    }
    std::thread::scope(|scope| scope.spawn(run).join()).unwrap_or_else(|_| {
        Err(HostError::Runtime(std::io::Error::other(
            "the command stopped unexpectedly",
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_runs_where_no_runtime_is_running() {
        assert_eq!(block_on(async { 7 }).unwrap(), 7);
    }

    #[tokio::test]
    async fn a_command_runs_also_when_the_program_is_inside_a_runtime_already() {
        assert_eq!(block_on(async { 7 }).unwrap(), 7);
    }

    /// What the argument `secrets_dir` of a set of arguments is when nobody says.
    fn secrets_dir_default<A: Args>() -> Vec<String> {
        let command = A::augment_args(clap::Command::new("probe"));
        let found = command
            .get_arguments()
            .find(|argument| argument.get_id() == "secrets_dir");
        found
            .map(|argument| {
                argument
                    .get_default_values()
                    .iter()
                    .filter_map(|value| value.to_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_commands_look_for_the_secrets_where_the_stack_binds_them_unless_told_otherwise() {
        assert_eq!(
            secrets_dir_default::<Sealing>(),
            [orch_config::DEFAULT_SECRETS_DIR]
        );
        assert_eq!(
            secrets_dir_default::<crate::commands::doctor::Options>(),
            [orch_config::DEFAULT_SECRETS_DIR]
        );
    }
}
