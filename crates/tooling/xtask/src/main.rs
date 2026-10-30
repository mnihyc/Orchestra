//! Development commands shared by contributors and CI.

#![forbid(unsafe_code)]

mod contracts;
mod names;
mod release;

use clap::{Parser, Subcommand};
use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};
use thiserror::Error;

#[derive(Parser)]
#[command(about = "Run the implemented development checks for Orch")]
struct Args {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Run formatting, compilation, Clippy, documentation and dependency checks.
    Verify {
        /// Check these packages; dependency policy still covers the whole graph.
        #[arg(short = 'p', long = "package")]
        packages: Vec<String>,
        /// Use cached dependencies and the cached advisory database.
        #[arg(long)]
        offline: bool,
    },
    /// Fail when shared code names an agent program or a vendor of one: a program is a manifest, and the code
    /// reads it.
    Names,
    /// Write the OpenAPI document and the browser's types from the contract crates.
    Contracts {
        /// Generate beside the committed files and fail when they differ.
        #[arg(long)]
        check: bool,
    },
    /// Build the files of a release for this platform into a directory, with the
    /// manifest that lists them; or check such a directory.
    Package(release::package::PackageArgs),
    /// Build the default environment image from its recipe, alone, and check that
    /// its labels say the versions the recipe pins.
    Image(release::image::ImageArgs),
    /// Print the identity the next build of a component Cargo does not build
    /// is given: the browser application's.
    Identity {
        /// The component: `frontend`.
        component: String,
    },
}

#[derive(Debug, Error)]
enum Error {
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Check(String),
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn verify_arguments(packages: &[String], offline: bool) -> Vec<Vec<String>> {
    let scopes: Vec<Option<&str>> = if packages.is_empty() {
        vec![None]
    } else {
        packages
            .iter()
            .map(|package| Some(package.as_str()))
            .collect()
    };
    let mut checks = Vec::new();
    for (operation, suffix) in [
        ("fmt", vec!["--", "--check"]),
        ("clippy", vec!["--all-targets", "--", "-D", "warnings"]),
        ("check", vec!["--all-targets"]),
        ("doc", vec!["--no-deps"]),
    ] {
        for scope in scopes.iter().copied() {
            let mut arguments = vec![operation.to_owned()];
            match scope {
                Some(package) => arguments.extend(["--package".to_owned(), package.to_owned()]),
                None => arguments.push(
                    if operation == "fmt" {
                        "--all"
                    } else {
                        "--workspace"
                    }
                    .to_owned(),
                ),
            }
            if operation != "fmt" {
                arguments.push("--locked".to_owned());
                if offline {
                    arguments.push("--offline".to_owned());
                }
            }
            arguments.extend(suffix.iter().map(|argument| (*argument).to_owned()));
            checks.push(arguments);
        }
    }
    let mut dependency_check = vec!["deny".to_owned()];
    if offline {
        dependency_check.push("--offline".to_owned());
    }
    dependency_check.extend(["--locked", "check"].map(str::to_owned));
    checks.push(dependency_check);
    checks
}

fn run() -> Result<(), Error> {
    let args = Args::parse();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .ok_or_else(|| Error::Check("xtask must live under crates/tooling".into()))?;
    if let Task::Names = args.command {
        return names::run(root).map_err(Error::Check);
    }
    if let Task::Contracts { check } = args.command {
        return contracts::run(root, check).map_err(Error::Check);
    }
    if let Task::Package(package) = &args.command {
        return release::package::run(root, package)
            .map_err(|error| Error::Check(error.to_string()));
    }
    if let Task::Image(image) = &args.command {
        return release::image::command(root, image)
            .map_err(|error| Error::Check(error.to_string()));
    }
    if let Task::Identity { component } = &args.command {
        let asked = release::identity::identity(component)
            .map_err(|error| Error::Check(error.to_string()))?;
        println!("{}", asked.json());
        return Ok(());
    }
    if let Task::Verify { packages, offline } = &args.command {
        names::run(root).map_err(Error::Check)?;
        for arguments in verify_arguments(packages, *offline) {
            eprintln!("Running cargo {}", arguments.join(" "));
            let mut command = cargo();
            command.args(&arguments).current_dir(root);
            if arguments[0] == "doc" {
                let mut flags = std::env::var("RUSTDOCFLAGS").unwrap_or_default();
                flags.push_str(" -D warnings");
                command.env("RUSTDOCFLAGS", flags);
            }
            let status = command.status().map_err(|source| Error::Io {
                path: root.to_owned(),
                source,
            })?;
            if !status.success() {
                return Err(Error::Check(format!(
                    "cargo {} failed: {status}",
                    arguments.join(" ")
                )));
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
