//! Administrative entry point. Commands are added as their components land.

#![forbid(unsafe_code)]

mod commands;

use clap::{Parser, Subcommand};
use commands::bootstrap::{self, Options, Outcome};
use commands::{checksum, doctor};
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_types::SystemClock;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const BUILD: BuildInfo = BuildInfo::new(
    "cli",
    env!("ORCH_RELEASE_VERSION"),
    env!("ORCH_RELEASE_TAG"),
    env!("ORCH_CLI_BUILD_NUMBER"),
);

#[derive(Parser)]
#[command(name = "orchctl", version = env!("ORCH_RELEASE_VERSION"), about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the identity compiled into this executable.
    Version {
        /// Print a JSON object for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Make the secrets of a deployment, once: the certificate authority, the
    /// master key, the first certificate of each service and the token of the
    /// bundled node. A directory that holds all of them is left as it is, and
    /// one that holds only a part is refused.
    ComposeSecrets {
        /// The directory the secrets go in (`config/secrets` of a deployment).
        #[arg(long, value_name = "DIR")]
        secrets_dir: PathBuf,
        /// The deployment's `orch.toml`, for the system's name and the names
        /// the service certificates are good for. Without it, the defaults
        /// that ship.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
    /// Make one request to the API with a token (`ORCH_URL`, `ORCH_TOKEN`) and
    /// print the answer as it came back: the way without a browser.
    ///
    /// The status is 0 for an answer in the 200s and otherwise the first digit
    /// of the status the API answered (3, 4 or 5); it is 1 when no answer came.
    Api(commands::api::ApiArgs),
    /// Import a provider's credential from standard input with ORCH_URL and ORCH_TOKEN.
    Provider(commands::provider::ProviderArgs),
    /// Break-glass administration on the host: a one-use address at which a
    /// person sets a new password, and making and unmaking a system admin or a
    /// suspension, for when nobody can sign in.
    Admin(commands::admin::AdminArgs),
    /// The rate catalogue, from a file or as the release came with it. It is
    /// in force at once.
    Usage(commands::usage::UsageArgs),
    /// A channel of the system, such as its mail server or its bot, made from
    /// a file of its fields, and one sample sent through it.
    Channel(commands::channel::ChannelArgs),
    /// Check an external implementation of an extension point against the
    /// suite of its kind before it is registered: one line for each rule, and
    /// the status 1 when one failed.
    Plugin(commands::plugin::PluginArgs),
    /// Check the stack and, with `--url`, the edge in front of it: each line
    /// says what was tried and what came back, and the exit status is 1 when a
    /// check failed.
    Doctor(doctor::Options),
    /// Complete or repair the existing database-only dump/manifest record on the installation host.
    Backup(commands::backup::BackupArgs),
    /// Print the updater's configured release source, without contacting it.
    ReleaseSource {
        /// The same operational configuration the API and worker read.
        #[arg(long, env = "ORCH_CONFIG", default_value = "/etc/orch/orch.toml")]
        config: PathBuf,
    },
    /// Maintain the updater's protected local storage records under its deployment lock.
    #[command(hide = true)]
    OperatorStorage(commands::operator_storage::Options),
    /// Print the configured system name for a local destructive operation's confirmation.
    SystemName {
        /// The same operational configuration the services read.
        #[arg(long, env = "ORCH_CONFIG", default_value = "/etc/orch/orch.toml")]
        config: PathBuf,
    },
    /// Print the BLAKE3 hash of files as `b3sum` does, for a host that has no
    /// `b3sum`: the updater checks a release against the hashes it lists.
    Checksum {
        /// The files to hash.
        #[arg(required = true, value_name = "FILE")]
        files: Vec<PathBuf>,
    },
}

fn compose_secrets(secrets_dir: PathBuf, config: Option<PathBuf>) -> Result<(), String> {
    let config: ServerConfig = match config {
        Some(path) => orch_config::load(&path).map_err(|error| error.to_string())?,
        None => ServerConfig::default(),
    };
    let options = Options::from_config(secrets_dir, &config);
    let outcome = bootstrap::compose_secrets(&options, &SystemClock).map_err(|e| e.to_string())?;
    let mut output = io::stdout().lock();
    let written = match outcome {
        Outcome::AlreadyComposed => writeln!(
            output,
            "the secrets in {} are all there already; nothing was written",
            options.secrets_dir.display()
        ),
        Outcome::Made {
            ca_fingerprint,
            ca_pin,
            files,
        } => writeln!(
            output,
            "made the secrets of \"{}\" in {}",
            options.system_name,
            options.secrets_dir.display()
        )
        .and_then(|()| {
            writeln!(
                output,
                "  authority fingerprint (SHA-256): {ca_fingerprint}"
            )
        })
        .and_then(|()| writeln!(output, "  authority pin, as tokens carry it: {ca_pin}"))
        .and_then(|()| writeln!(output, "  files: {}", files.join(", "))),
    };
    written.map_err(|error| error.to_string())
}

fn main() -> ExitCode {
    let args = Cli::parse();
    let result = match args.command {
        Command::Version { json } => {
            let mut output = io::stdout().lock();
            let written = if json {
                serde_json::to_writer(&mut output, &BUILD)
                    .map_err(io::Error::from)
                    .and_then(|()| writeln!(output))
            } else {
                writeln!(output, "{BUILD}")
            };
            written.map_err(|error| error.to_string())
        }
        Command::ComposeSecrets {
            secrets_dir,
            config,
        } => compose_secrets(secrets_dir, config),
        Command::Api(args) => return commands::api::run(&args),
        Command::Provider(args) => return commands::provider::run(&args),
        Command::Admin(args) => return commands::admin::run(&args),
        Command::Usage(args) => return commands::usage::run(&args),
        Command::Channel(args) => return commands::channel::run(&args),
        Command::Plugin(args) => return commands::plugin::run(&args),
        Command::Doctor(options) => doctor::run(&options),
        Command::Backup(args) => return commands::backup::run(&args),
        Command::OperatorStorage(options) => commands::operator_storage::run(&options),
        Command::ReleaseSource { config } => orch_config::load::<ServerConfig>(&config)
            .map_err(|error| error.to_string())
            .and_then(|config| {
                writeln!(io::stdout().lock(), "{}", config.release.source())
                    .map_err(|error| error.to_string())
            }),
        Command::SystemName { config } => orch_config::load::<ServerConfig>(&config)
            .map_err(|error| error.to_string())
            .and_then(|config| {
                writeln!(io::stdout().lock(), "{}", config.system.name)
                    .map_err(|error| error.to_string())
            }),
        Command::Checksum { files } => checksum::run(&files),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(io::stderr(), "orchctl: {message}");
            ExitCode::FAILURE
        }
    }
}
