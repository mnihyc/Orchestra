//! Compiling and reading inside the builder image.
//!
//! The server programs run on the distribution the Compose services use, and a
//! program built against a newer C library than that one does not start there.
//! So they are compiled in a container of that distribution, which is also
//! where the static programs are built (they are copied into machines of any
//! distribution, and need a C compiler for the musl target that a laptop may
//! not have). Nothing is installed on the machine that runs this.

use crate::release::{ReleaseError, capture, run};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The distribution the Compose services run, and the one the builder is based
/// on. This is the one place it is named: packaging refuses a Compose file that
/// runs the services on another.
pub const RUNTIME_IMAGE: &str = "debian:bookworm-slim";
/// The recipe of the builder image, below the root of the repository.
const BUILDER_RECIPE: &str = "images/orch-builder";
/// Where the source is mounted in the container, read-only.
const WORK_DIR: &str = "/work";
/// Where the build's target directory is mounted.
const TARGET_DIR: &str = "/target";
/// Where Cargo keeps what it downloads, so that a second build downloads nothing.
const CARGO_HOME_DIR: &str = "/cargo-home";
/// Where the build counters are mounted.
const COUNTERS_DIR: &str = "/counters";

/// The builder image and the directories it works in.
#[derive(Debug, Clone)]
pub struct Builder {
    /// The tag of the builder image.
    pub image: String,
    /// The toolchain the image holds.
    pub toolchain: String,
    root: PathBuf,
    state: PathBuf,
    counters: PathBuf,
    user: String,
    runtime: String,
}

impl Builder {
    /// A builder for the repository at `root`, keeping what it writes below
    /// `state`, and advancing the build counters in `counters`.
    pub fn new(root: &Path, state: &Path, counters: &Path) -> Result<Self, ReleaseError> {
        let toolchain = pinned_toolchain(root)?;
        let runtime = pinned_runtime(root)?;
        let tag = RUNTIME_IMAGE.rsplit(':').next().unwrap_or(RUNTIME_IMAGE);
        let user = format!(
            "{}:{}",
            capture(Command::new("id").arg("-u"))?,
            capture(Command::new("id").arg("-g"))?
        );
        Ok(Self {
            image: format!(
                "orch-builder:{tag}-rust{toolchain}-{}",
                runtime.rsplit(':').next().unwrap_or_default()
            ),
            toolchain,
            root: root.to_owned(),
            state: state.to_owned(),
            counters: counters.to_owned(),
            user,
            runtime,
        })
    }

    /// Where the build's target directory is, on this machine.
    #[must_use]
    pub fn target_dir(&self) -> PathBuf {
        self.state.join("target")
    }

    /// Where Cargo's downloads are, on this machine.
    #[must_use]
    pub fn cargo_home(&self) -> PathBuf {
        self.state.join("cargo-home")
    }

    /// Makes the builder image if it is not there.
    pub fn ensure_image(&self) -> Result<(), ReleaseError> {
        let present = Command::new("docker")
            .args(["image", "inspect", "--format", "{{.Id}}", &self.image])
            .output()
            .map_err(|error| ReleaseError::Command {
                program: "docker".to_owned(),
                detail: format!("cannot be run: {error}"),
            })?
            .status
            .success();
        if present {
            return Ok(());
        }
        let context = self.root.join(BUILDER_RECIPE);
        run(Command::new("docker")
            .args(["build", "--tag", &self.image])
            .args(["--build-arg", &format!("RUNTIME_IMAGE={}", self.runtime)])
            .args(["--build-arg", &format!("RUST_TOOLCHAIN={}", self.toolchain)])
            .arg(&context))
    }

    fn container(&self) -> Result<Command, ReleaseError> {
        // Each mounted directory is made here, as the person building: one Docker makes is root's, and the build
        // inside, which runs as that person, could not write its counters there.
        for dir in [self.target_dir(), self.cargo_home(), self.counters.clone()] {
            std::fs::create_dir_all(&dir)
                .map_err(|source| ReleaseError::Io { path: dir, source })?;
        }
        let mount = |from: &Path, to: &str, mode: &str| format!("{}:{to}{mode}", from.display());
        let mut command = Command::new("docker");
        command
            .args(["run", "--rm", "--user", &self.user])
            .args(["--volume", &mount(&self.root, WORK_DIR, ":ro")])
            .args(["--volume", &mount(&self.target_dir(), TARGET_DIR, "")])
            .args(["--volume", &mount(&self.cargo_home(), CARGO_HOME_DIR, "")])
            .args(["--volume", &mount(&self.counters, COUNTERS_DIR, "")])
            .args(["--workdir", WORK_DIR])
            // A build runs as the person who started it, who has no home here.
            .args(["--env", "HOME=/tmp"])
            .args(["--env", &format!("CARGO_HOME={CARGO_HOME_DIR}")])
            .args(["--env", &format!("CARGO_TARGET_DIR={TARGET_DIR}")])
            .args(["--env", &format!("ORCH_BUILD_NUMBER_DIR={COUNTERS_DIR}")])
            .args(["--env", "CARGO_INCREMENTAL=0"])
            // Keep the caller's build parallelism when compiling inside the distribution image.
            .args(["--env", "CARGO_BUILD_JOBS"])
            .args(["--env", "RUST_MIN_STACK"])
            .args(["--env", "ORCH_TEST_DATABASE_URL"])
            .args(["--env", "ORCH_TEST_DATABASE_CONNECTION_BUDGET"]);
        Ok(command)
    }

    /// Runs `cargo` with `arguments` in the container, with `environment` added.
    pub fn cargo(
        &self,
        arguments: &[&str],
        environment: &[(String, String)],
    ) -> Result<(), ReleaseError> {
        let mut command = self.container()?;
        for (name, value) in environment {
            command.args(["--env", &format!("{name}={value}")]);
        }
        run(command.arg(&self.image).arg("cargo").args(arguments))
    }

    /// The output of `cargo` with `arguments` in the container.
    pub fn cargo_output(&self, arguments: &[&str]) -> Result<String, ReleaseError> {
        let mut command = self.container()?;
        capture(command.arg(&self.image).arg("cargo").args(arguments))
    }

    /// The content of the file at `path` inside the image.
    pub fn read_in_image(&self, path: &str) -> Result<Vec<u8>, ReleaseError> {
        let output = Command::new("docker")
            .args(["run", "--rm", "--entrypoint", "cat", &self.image, path])
            .output()
            .map_err(|error| ReleaseError::Command {
                program: "docker".to_owned(),
                detail: format!("cannot be run: {error}"),
            })?;
        if !output.status.success() {
            return Err(ReleaseError::Command {
                program: "docker".to_owned(),
                detail: format!("cannot read {path} in {}", self.image),
            });
        }
        Ok(output.stdout)
    }

    /// The architecture the container runs on: `x86_64` or `aarch64`.
    pub fn architecture() -> Result<String, ReleaseError> {
        let reported =
            capture(Command::new("docker").args(["info", "--format", "{{.Architecture}}"]))?;
        match reported.as_str() {
            "x86_64" | "amd64" => Ok("x86_64".to_owned()),
            "aarch64" | "arm64" => Ok("aarch64".to_owned()),
            other => Err(ReleaseError::Invalid(format!(
                "Docker runs on {other:?}, and releases are made for x86_64 and aarch64"
            ))),
        }
    }
}

fn pinned_runtime(root: &Path) -> Result<String, ReleaseError> {
    let path = root.join("deploy/compose.yml");
    let text =
        std::fs::read_to_string(&path).map_err(|source| ReleaseError::Io { path, source })?;
    super::stock::compose_images(&text)?
        .into_iter()
        .find(|image| image.reference.split('@').next() == Some(RUNTIME_IMAGE))
        .map(|image| image.reference)
        .ok_or_else(|| {
            ReleaseError::Invalid(
                "the builder needs the immutable service runtime from Compose".into(),
            )
        })
}

/// The channel `rust-toolchain.toml` pins.
fn pinned_toolchain(root: &Path) -> Result<String, ReleaseError> {
    let path = root.join("rust-toolchain.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| ReleaseError::Io {
        path: path.clone(),
        source,
    })?;
    let table: toml::Table = text
        .parse()
        .map_err(|error| ReleaseError::Invalid(format!("{}: {error}", path.display())))?;
    table
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("channel"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ReleaseError::Invalid(format!("{} names no toolchain channel", path.display()))
        })
}

/// Checks that the Compose file runs its services on the distribution the
/// builder is based on: a server program is built for one and started on
/// another otherwise, and fails there.
pub fn check_compose_distribution(root: &Path) -> Result<(), ReleaseError> {
    let path = root.join("deploy/compose.yml");
    let text = std::fs::read_to_string(&path).map_err(|source| ReleaseError::Io {
        path: path.clone(),
        source,
    })?;
    let images: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("image:"))
        .map(|image| image.trim().trim_matches(['"', '\'']))
        .filter(|image| image.starts_with("debian:"))
        .collect();
    if images.is_empty()
        || images
            .iter()
            .any(|image| image.split('@').next() != Some(RUNTIME_IMAGE))
    {
        return Err(ReleaseError::Invalid(format!(
            "{} runs the services on {images:?}, and the programs are built for {RUNTIME_IMAGE}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toolchain_is_the_channel_the_repository_pins() {
        let dir = tempfile::tempdir().expect("a directory");
        std::fs::write(
            dir.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.97.0\"\ncomponents = [\"clippy\"]\n",
        )
        .expect("a file");
        assert_eq!(pinned_toolchain(dir.path()).expect("read"), "1.97.0");
    }

    #[test]
    fn services_on_another_distribution_than_the_builder_are_refused() {
        let dir = tempfile::tempdir().expect("a directory");
        std::fs::create_dir_all(dir.path().join("deploy")).expect("a directory");
        let compose = dir.path().join("deploy/compose.yml");
        std::fs::write(
            &compose,
            format!("services:\n  api:\n    image: {RUNTIME_IMAGE}\n"),
        )
        .expect("a file");
        check_compose_distribution(dir.path()).expect("the same distribution");
        std::fs::write(
            &compose,
            "services:\n  api:\n    image: debian:trixie-slim\n",
        )
        .expect("a file");
        assert!(check_compose_distribution(dir.path()).is_err());
    }

    #[test]
    fn the_builder_uses_the_exact_service_digest_instead_of_overriding_it_with_a_moving_tag() {
        let dir = tempfile::tempdir().expect("a directory");
        std::fs::create_dir(dir.path().join("deploy")).expect("a directory");
        let expected = format!("{RUNTIME_IMAGE}@sha256:{}", "a".repeat(64));
        std::fs::write(
            dir.path().join("deploy/compose.yml"),
            format!("services:\n  api:\n    image: {expected}\n"),
        )
        .expect("compose");
        assert_eq!(
            pinned_runtime(dir.path()).expect("immutable runtime"),
            expected
        );
        std::fs::write(
            dir.path().join("deploy/compose.yml"),
            format!("services:\n  api:\n    image: {RUNTIME_IMAGE}\n"),
        )
        .expect("compose");
        assert!(pinned_runtime(dir.path()).is_err());
    }
}
