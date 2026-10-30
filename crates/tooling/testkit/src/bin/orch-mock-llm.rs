//! Scripted model endpoint placeholder for behavioral integration exercises.
//!
//! The scripted model server as a program, for the tests that run outside a process: the end-to-end stack,
//! and the contract tests of the harnesses. It speaks the Responses format of OpenAI and the Messages format of
//! Anthropic on one port and answers from a script chosen by a directive in the message of the person, or
//! set ahead through its control endpoint below `/_mock/`. The manual of both is the documentation of
//! `orch_testkit::doubles::model`.
//!
//! ```text
//! orch-mock-llm [--listen <address>] [--accept <ways>] [--model <id>]... [--default-script <file>]
//!               [--created-at <seconds>] [--credential-file <file>]
//! ```
//!
//! | Option | Meaning |
//! |--------|---------|
//! | `--listen` | The address to listen on; `127.0.0.1:0`, a free port of the loopback, unless given |
//! | `--accept` | The ways in which the credential is accepted, separated by commas: `bearer`, `x-api-key`, `header:<name>`; the first two unless given |
//! | `--model` | A model to list; repeat for more; `mock-small` and `mock-large` unless given |
//! | `--default-script` | A file holding the script that answers a request nothing else answers |
//! | `--created-at` | The moment to report, in seconds since the epoch |
//! | `--credential-file` | A file holding the credential to require; the first line is it |
//!
//! The credential can also be given in the variable `ORCH_MOCK_LLM_CREDENTIAL`; it is never an argument, which
//! other users of the machine could read. Without one the server lets every request in.
//!
//! The program prints `listening on http://<address>` as its first line, once it listens, and runs until it
//! receives an interrupt or a termination signal.

#![forbid(unsafe_code)]

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use orch_testkit::doubles::model::{MockConfig, MockLlm, MockModel, Presentation};

/// The variable that holds the credential to require.
const CREDENTIAL_VARIABLE: &str = "ORCH_MOCK_LLM_CREDENTIAL";

/// The scripted model server.
#[derive(Debug, Parser)]
#[command(name = "orch-mock-llm", version, about)]
struct Args {
    /// The address to listen on.
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: SocketAddr,
    /// The ways in which the credential is accepted: bearer, x-api-key, header:NAME.
    #[arg(long, value_delimiter = ',')]
    accept: Vec<Presentation>,
    /// A model to list; repeat for more.
    #[arg(long = "model")]
    models: Vec<String>,
    /// A file holding the script that answers a request nothing else answers.
    #[arg(long)]
    default_script: Option<PathBuf>,
    /// The moment to report, in seconds since the epoch.
    #[arg(long)]
    created_at: Option<i64>,
    /// A file holding the credential to require.
    #[arg(long)]
    credential_file: Option<PathBuf>,
}

/// The set-up the arguments and the environment ask for.
fn config(args: &Args) -> Result<MockConfig, String> {
    let mut config = MockConfig::default();
    let from_file = match &args.credential_file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            text.lines().next().map(str::trim).map(str::to_owned)
        }
        None => None,
    };
    config.credential = from_file
        .or_else(|| std::env::var(CREDENTIAL_VARIABLE).ok())
        .filter(|credential| !credential.is_empty());
    if !args.accept.is_empty() {
        config.accept.clone_from(&args.accept);
    }
    if !args.models.is_empty() {
        config.models = args.models.iter().map(|id| MockModel::named(id)).collect();
        if let Some(first) = args.models.first() {
            config.default_model.clone_from(first);
        }
    }
    if let Some(created_at) = args.created_at {
        config.created_at = created_at;
    }
    if let Some(path) = &args.default_script {
        config.default_script = Some(
            std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
        );
    }
    Ok(config)
}

/// The signals that stop the program. They are taken before the program says where it listens, so that a
/// signal sent by whoever read that line is never met by the default action, which would end the program
/// as if it had failed.
struct Stop {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
}

impl Stop {
    /// Starts listening for an interrupt and a termination signal.
    fn listen() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Self {
                interrupt: signal(SignalKind::interrupt()).ok(),
                terminate: signal(SignalKind::terminate()).ok(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }

    /// Waits for either of them.
    async fn wait(self) {
        #[cfg(unix)]
        {
            let Self {
                interrupt,
                terminate,
            } = self;
            if let (Some(mut interrupt), Some(mut terminate)) = (interrupt, terminate) {
                tokio::select! {
                    _ = interrupt.recv() => {}
                    _ = terminate.recv() => {}
                }
                return;
            }
        }
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let stop = Stop::listen();
    let config = match config(&args) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("orch-mock-llm: {message}");
            return ExitCode::FAILURE;
        }
    };
    let server = match MockLlm::start_on(args.listen, config).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("orch-mock-llm: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("listening on {}", server.url());
    let _ = std::io::stdout().flush();
    stop.wait().await;
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        Args::parse_from(std::iter::once("orch-mock-llm").chain(list.iter().copied()))
    }

    #[test]
    fn without_options_the_server_is_open_to_everyone_and_lists_two_models() {
        let config = config(&args(&[])).unwrap();
        assert!(config.credential.is_none() || std::env::var(CREDENTIAL_VARIABLE).is_ok());
        assert_eq!(config.models.len(), 2);
        assert_eq!(config.accept, [Presentation::Bearer, Presentation::XApiKey]);
    }

    #[test]
    fn the_ways_and_the_models_are_what_the_options_say() {
        let config = config(&args(&[
            "--accept",
            "header:api-key,bearer",
            "--model",
            "gpt-6-sol",
            "--model",
            "claude-opus-5",
            "--created-at",
            "42",
        ]))
        .unwrap();
        assert_eq!(
            config.accept,
            [Presentation::Header("api-key".into()), Presentation::Bearer]
        );
        let ids: Vec<_> = config.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6-sol", "claude-opus-5"]);
        assert_eq!(config.default_model, "gpt-6-sol");
        assert_eq!(config.created_at, 42);
    }

    #[test]
    fn a_file_that_is_not_there_is_named_in_the_message() {
        let error = config(&args(&["--default-script", "/nonexistent/script.txt"])).unwrap_err();
        assert!(error.contains("/nonexistent/script.txt"));
    }
}
