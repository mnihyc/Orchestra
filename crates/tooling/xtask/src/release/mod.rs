//! Planned release support; no publishing commands are implemented.
//!
//! What is here packages a release and checks one: `cargo xtask package` builds
//! the assets of the host's platform into a directory and writes the manifest
//! that lists them, and `cargo xtask identity` answers a build that Cargo does
//! not run. Nothing here publishes anything: a release workflow does that, from
//! a directory this command made.

pub mod archive;
pub mod connectors;
pub mod container;
mod font_names;
pub mod identity;
pub mod image;
pub mod licences;
pub mod manifest;
pub mod package;
mod stock;

use std::path::PathBuf;
use std::process::Command;

/// Why a release could not be packaged or checked.
#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    /// A file could not be read or written.
    #[error("cannot access {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: std::io::Error,
    },
    /// A JSON file could not be read or written.
    #[error("{path}: {source}")]
    Json {
        /// The file.
        path: PathBuf,
        /// What was wrong with it.
        #[source]
        source: serde_json::Error,
    },
    /// The identity of the build could not be established.
    #[error("{0}")]
    Identity(#[from] orch_build_support::Error),
    /// A program that was run failed.
    #[error("{program} failed: {detail}")]
    Command {
        /// The program.
        program: String,
        /// How it failed.
        detail: String,
    },
    /// Something is not as packaging needs it, said as a sentence.
    #[error("{0}")]
    Invalid(String),
    /// The directory is not a whole release.
    #[error("{dir} is not a whole release:\n{problems}")]
    NotWhole {
        /// The directory.
        dir: PathBuf,
        /// One fault per line.
        problems: String,
    },
}

/// Runs `command`, which inherits the terminal, and fails unless it succeeds.
pub(crate) fn run(command: &mut Command) -> Result<(), ReleaseError> {
    let program = command.get_program().to_string_lossy().into_owned();
    eprintln!("+ {}", describe(command));
    let status = command.status().map_err(|error| ReleaseError::Command {
        program: program.clone(),
        detail: format!("cannot be run: {error}"),
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(ReleaseError::Command {
            program,
            detail: status.to_string(),
        })
    }
}

/// Runs `command` and answers what it printed, with the line break trimmed.
pub(crate) fn capture(command: &mut Command) -> Result<String, ReleaseError> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().map_err(|error| ReleaseError::Command {
        program: program.clone(),
        detail: format!("cannot be run: {error}"),
    })?;
    if !output.status.success() {
        return Err(ReleaseError::Command {
            program,
            detail: format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The command as a person would type it, for the log.
fn describe(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| {
            let part = part.to_string_lossy();
            if part.contains(' ') {
                format!("'{part}'")
            } else {
                part.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
