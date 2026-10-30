//! Compose the API, engine, database, PKI, blob store, summarizers and identity providers.
//!
//! The program reads the deployment's configuration, brings the schema up
//! to date, and serves until it is told to stop.

#![forbid(unsafe_code)]

mod composition;

use clap::Parser;
use orch_build_info::BuildInfo;
use std::path::PathBuf;

const BUILD: BuildInfo = BuildInfo::new(
    "api",
    env!("ORCH_RELEASE_VERSION"),
    env!("ORCH_RELEASE_TAG"),
    env!("ORCH_SERVER_BUILD_NUMBER"),
);

// Debug request and service futures can exhaust Tokio's default 2 MiB stack;
// keep 8 MiB for runtime workers and blocking tasks as a server-wide guard.
const WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// The Orch API service.
#[derive(Parser)]
#[command(name = "orch-api", version = env!("ORCH_RELEASE_VERSION"), about)]
struct Args {
    /// The deployment's configuration file.
    #[arg(long, env = "ORCH_CONFIG", default_value = "/etc/orch/orch.toml")]
    config: PathBuf,
    /// The directory of the deployment's secrets: the authority, the master
    /// key and the API's own certificate, as `orchctl compose-secrets` makes
    /// them.
    #[arg(long, env = "ORCH_SECRETS_DIR", default_value = orch_config::DEFAULT_SECRETS_DIR)]
    secrets_dir: PathBuf,
    /// Serve the browser API without TLS, for development on one machine. A
    /// deployment never uses this: its API is reached only under the
    /// system's own certificate authority. Without the secrets of a
    /// deployment the API then has no machines.
    #[arg(long)]
    plain_http: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(WORKER_STACK_BYTES)
        .enable_all()
        .build()?
        .block_on(Box::pin(composition::run(
            &composition::Launch {
                config: &args.config,
                secrets_dir: &args.secrets_dir,
                plain_http: args.plain_http,
            },
            BUILD,
        )))
}

#[cfg(test)]
mod tests {
    use super::Args;
    use clap::CommandFactory;

    #[test]
    fn the_secrets_are_looked_for_where_the_stack_binds_them_unless_told_otherwise() {
        let command = Args::command();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "secrets_dir")
            .unwrap();
        let defaults: Vec<&str> = argument
            .get_default_values()
            .iter()
            .filter_map(|value| value.to_str())
            .collect();
        assert_eq!(defaults, [orch_config::DEFAULT_SECRETS_DIR]);
    }
}
