//! Compose the stateless gateway listeners, backplane and tunnel relay.
//!
//! The program reads the deployment's configuration and the gateway's own
//! certificate, listens on the two ports machines connect to, dials the
//! system, and serves until it is told to stop. It keeps nothing on disk.

#![forbid(unsafe_code)]

mod composition;

use anyhow::Context;
use clap::Parser;
use orch_build_info::BuildInfo;
use orch_config::{RuntimeComponent, ServerConfig};
use std::path::PathBuf;

const BUILD: BuildInfo = BuildInfo::new(
    "gateway",
    env!("ORCH_RELEASE_VERSION"),
    env!("ORCH_RELEASE_TAG"),
    env!("ORCH_SERVER_BUILD_NUMBER"),
);

/// The Orch gateway: where machines connect.
#[derive(Parser)]
#[command(name = "orch-gateway", version = env!("ORCH_RELEASE_VERSION"), about)]
struct Args {
    /// The deployment's configuration file.
    #[arg(long, env = "ORCH_CONFIG", default_value = "/etc/orch/orch.toml")]
    config: PathBuf,
    /// The directory of the deployment's secrets: the authority's certificate
    /// and the gateway's own key and certificate, as `orchctl compose-secrets`
    /// makes them.
    #[arg(long, env = "ORCH_SECRETS_DIR", default_value = orch_config::DEFAULT_SECRETS_DIR)]
    secrets_dir: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    #[cfg(feature = "test-observers")]
    let observer =
        orch_gateway_core::test_observers::process::ProcessRecording::from_env("gateway", &BUILD)?;
    let config: ServerConfig = orch_config::load(&args.config)
        .with_context(|| format!("reading {}", args.config.display()))?;
    let worker_stack_bytes = config.worker_stack_bytes(RuntimeComponent::Gateway)?;
    let result = tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(worker_stack_bytes)
        .enable_all()
        .build()?
        .block_on(Box::pin(composition::run(config, &args.secrets_dir, BUILD)));
    #[cfg(feature = "test-observers")]
    if result.is_ok()
        && let Some(observer) = observer
    {
        observer.finish()?;
    }
    result
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
