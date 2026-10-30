//! Running the real harnesses the way a connector hosts them, from inside the image of the development harnesses: the program of the harness is started with `docker run`, in a container named `orch-test-conn-…` that shares the network of the host (so it reaches the connector's listener on the loopback) and mounts the directories the adapter and the harness both use at their own paths (so every path the adapter computes is true on both sides). The key a model server wants is never given to a container: what a container is given is recorded, and a test looks in it.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use orch_adapter_claude_code::process::{LaunchSpec, Launcher as ClaudeLauncher};
use orch_adapter_codex::process::{
    HarnessProcess, LaunchError, LaunchRequest, Launcher as CodexLauncher, ProcessControl,
    ProcessExit,
};
use orch_types::new_id;
use tokio::process::Command;

/// The image the harnesses run in.
pub const IMAGE: &str = "orch-dev-harness";

/// Whether Docker answers and the image of the harnesses is on this machine; a line says so when it is not.
pub fn available(test: &str) -> bool {
    let present = std::process::Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !present {
        eprintln!(
            "SKIPPED {test}: Docker or the image {IMAGE} is not on this machine (docker image inspect failed); \
             this test runs the real harness in a container"
        );
    }
    present
}

/// Variables of a launch that name a place on the host and mean nothing in the container.
const HOST_ONLY: [&str; 9] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "TERM",
    "NO_COLOR",
];

/// What one container was started with.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// The arguments of `docker`.
    pub argv: Vec<String>,
    /// The environment of `docker`'s own process that was passed to the container by name.
    pub env: BTreeMap<String, String>,
}

/// The containers a test started, by name; they are removed by name when it ends, whether it passed or not.
#[derive(Debug)]
pub struct Containers {
    prefix: String,
    counter: AtomicU32,
    names: Mutex<Vec<String>>,
    recorded: Mutex<Vec<Recorded>>,
}

impl Containers {
    /// Containers named `orch-test-conn-<label>-<process>-<random>-<n>`.
    pub fn new(label: &str) -> Arc<Self> {
        Arc::new(Self {
            prefix: format!(
                "orch-test-conn-{label}-{}-{}",
                std::process::id(),
                &new_id().simple().to_string()[..8]
            ),
            counter: AtomicU32::new(0),
            names: Mutex::new(Vec::new()),
            recorded: Mutex::new(Vec::new()),
        })
    }

    fn next(&self) -> String {
        let name = format!(
            "{}-{}",
            self.prefix,
            self.counter.fetch_add(1, Ordering::SeqCst)
        );
        self.names.lock().unwrap().push(name.clone());
        name
    }

    /// What every container was started with so far.
    pub fn recorded(&self) -> Vec<Recorded> {
        self.recorded.lock().unwrap().clone()
    }

    /// The names of the containers started so far.
    pub fn names(&self) -> Vec<String> {
        self.names.lock().unwrap().clone()
    }

    /// What `docker inspect` says of a container that runs: its environment and its command, as text.
    pub fn inspect(&self, name: &str) -> Option<String> {
        let output = std::process::Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{json .Config.Env}} {{json .Config.Cmd}} {{json .Args}}",
                name,
            ])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Removes every container started, by its exact name, whatever state it is in.
    pub fn remove_all(&self) {
        let names = self.names.lock().unwrap().clone();
        for name in names {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", &name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    /// The command that runs `program` in a container.
    fn command(
        &self,
        mounts: &[PathBuf],
        cwd: Option<&Path>,
        env: &BTreeMap<String, String>,
        program: &str,
        args: &[String],
    ) -> (Command, String) {
        let name = self.next();
        let mut command = Command::new("docker");
        command.args(["run", "-i", "--rm", "--name", &name, "--network", "host"]);
        for mount in mounts {
            command.arg("-v").arg(format!("{0}:{0}", mount.display()));
        }
        if let Some(cwd) = cwd {
            command.arg("-w").arg(cwd);
        }
        let mut passed = BTreeMap::new();
        for (key, value) in env {
            if !HOST_ONLY.contains(&key.as_str()) {
                // By name: the value is in the environment of `docker`, not in its arguments.
                command.arg("-e").arg(key);
                command.env(key, value);
                passed.insert(key.clone(), value.clone());
            }
        }
        command.arg(IMAGE).arg(program).args(args);
        self.recorded.lock().unwrap().push(Recorded {
            argv: command
                .as_std()
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            env: passed,
        });
        (command, name)
    }
}

impl Drop for Containers {
    fn drop(&mut self) {
        self.remove_all();
    }
}

/// Starts the programs of the Codex adapter in containers.
#[derive(Debug)]
pub struct CodexDocker {
    containers: Arc<Containers>,
    mounts: Vec<PathBuf>,
}

impl CodexDocker {
    /// A launcher that mounts these directories.
    pub fn new(containers: Arc<Containers>, mounts: Vec<PathBuf>) -> Self {
        Self { containers, mounts }
    }
}

struct DockerControl {
    child: tokio::process::Child,
    name: String,
}

#[async_trait]
impl ProcessControl for DockerControl {
    async fn wait(&mut self) -> ProcessExit {
        match self.child.wait().await {
            Ok(status) => ProcessExit {
                code: status.code(),
                detail: String::new(),
            },
            Err(error) => ProcessExit {
                code: None,
                detail: error.to_string(),
            },
        }
    }

    fn kill(&mut self) {
        // Ending the client of `docker run` does not end the container: the container is removed.
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.child.start_kill();
    }
}

#[async_trait]
impl CodexLauncher for CodexDocker {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        let (mut command, name) = self.containers.command(
            &self.mounts,
            request.cwd.as_deref(),
            &request.env,
            &request.program,
            &request.args,
        );
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|source| LaunchError::Spawn {
            program: "docker".to_owned(),
            source,
        })?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        Ok(HarnessProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: Some(Box::new(stderr)),
            control: Box::new(DockerControl { child, name }),
        })
    }
}

/// Starts the programs of the Claude Code adapter in containers.
#[derive(Debug)]
pub struct ClaudeDocker {
    containers: Arc<Containers>,
    mounts: Vec<PathBuf>,
}

impl ClaudeDocker {
    /// A launcher that mounts these directories.
    pub fn new(containers: Arc<Containers>, mounts: Vec<PathBuf>) -> Self {
        Self { containers, mounts }
    }
}

impl ClaudeLauncher for ClaudeDocker {
    fn command(&self, spec: &LaunchSpec) -> Command {
        self.containers
            .command(
                &self.mounts,
                Some(&spec.cwd),
                &spec.env,
                &spec.program,
                &spec.args,
            )
            .0
    }
}
