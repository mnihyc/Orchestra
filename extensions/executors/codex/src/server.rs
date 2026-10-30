//! Start and stop the vendor's own exec server on loopback, of the build the harness needs.
//!
//! [`CodexExecServer`] is the executor an environment's exec node starts for a harness that runs in its
//! provider's runtime and reaches its tools through Codex's own exec server. Starting it means running the
//! vendor's program from the environment's tool cache as `codex exec-server`, listening on the loopback
//! address, on a port the system gives, and asking for a token on every connection. The node then serves the
//! harness's `exec` tunnels by connecting to the address as a WebSocket client with that token, and speaks
//! nothing of the vendor's protocol itself.
//!
//! The server is started with an environment of its own making and nothing else of the executor's: the
//! commands a harness runs through it inherit the server's environment, so what the executor itself holds
//! (the connector's settings, its tokens) must not be there. The environment's variables for its agents'
//! commands are, and the home of the program is a directory under the executor's own.
//!
//! The server is ended with `TERM` to its process group, and `KILL` after the grace. It is not tied to the
//! process that started it (the flag that would do that belongs to the vendor's remote mode only), so the
//! executor writes the server's process id and the digest of its token into its own directory; if the node
//! died without ending the server, the next start finds the file, makes sure by the command line that the
//! process is that server and no other, and ends it before it starts a new one.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::descriptor::SandboxMode;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::descriptor::{ExecutorAvailability, ExecutorDescriptor};
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_plugin_contract::Secret;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::descriptor::{self, Build, KIND, Missing};
use crate::sandbox::{self, Probe};

/// How long the server has to announce the address it listens on: the node's deadline for starting an
/// executor is two minutes, and the probe of the sandbox and the handshake come before and after.
pub const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a command of the sandbox probe, or the question for the program's version, may take: the
/// probe runs up to six, and with the start they have to fit in the node's two minutes.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// A local program's version is a quick observation, not a reason to delay the offer.
const LOCAL_VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// A version is one short line; a bounded read keeps a faulty program from filling memory.
const VERSION_OUTPUT_LIMIT: u64 = 4096;

/// How long the handshake with the started server may take.
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the server has between `TERM` and `KILL`, as a command has.
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(10);

/// The file in the executor's directory that names the server it started.
const PID_FILE: &str = "exec-server.pid";

/// How long a server left behind by a node that died is given to end on `TERM` before it is killed.
const LEFTOVER_GRACE: Duration = Duration::from_secs(3);

/// How many ports of a range are tried before the range is given up on.
const PORT_TRIES: usize = 16;

/// How many lines of the server's standard error are kept to say why it did not start.
const KEPT_ERROR_LINES: usize = 20;

/// The variables of the executor's own environment that the server, and so the commands it runs, receive.
const INHERITED_VARIABLES: [&str; 14] = [
    "PATH",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TZ",
    "TMPDIR",
    "SystemRoot",
    "PATHEXT",
    "COMSPEC",
    "TEMP",
    "TMP",
    "USERPROFILE",
];

/// What the executor is configured with.
#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// The ports the server may listen on. `None` lets the system give one, which is what an exec node
    /// does; a range is for a machine that allows a few ports only.
    pub ports: Option<RangeInclusive<u16>>,
    /// How long the server has to announce its address.
    pub start_timeout: Duration,
    /// How long one command of the sandbox probe may take.
    pub probe_timeout: Duration,
    /// How long the handshake may take.
    pub handshake_timeout: Duration,
    /// How long the server has between `TERM` and `KILL`.
    pub stop_grace: Duration,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            ports: None,
            start_timeout: DEFAULT_START_TIMEOUT,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            stop_grace: DEFAULT_STOP_GRACE,
        }
    }
}

/// What was found of a build, and kept until the next start.
#[derive(Debug, Clone)]
struct Found {
    program: PathBuf,
    version: Option<semver::Version>,
    modes: Vec<SandboxMode>,
}

/// A started server.
struct Running {
    endpoint: ExecutorEndpoint,
    child: Child,
    pid_file: PathBuf,
    tasks: Vec<JoinHandle<()>>,
}

/// The vendor's exec server, run for an environment.
#[derive(Default)]
pub struct CodexExecServer {
    config: CodexConfig,
    found: tokio::sync::Mutex<Option<Found>>,
    running: Option<Running>,
}

impl std::fmt::Debug for CodexExecServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexExecServer")
            .field("running", &self.running.is_some())
            .finish_non_exhaustive()
    }
}

impl CodexExecServer {
    /// An executor with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An executor with the configuration given.
    #[must_use]
    pub fn with_config(config: CodexConfig) -> Self {
        Self {
            config,
            found: tokio::sync::Mutex::new(None),
            running: None,
        }
    }

    /// Where the started server is reached, while it runs.
    #[must_use]
    pub fn endpoint(&self) -> Option<&ExecutorEndpoint> {
        self.running.as_ref().map(|running| &running.endpoint)
    }

    /// The build of the environment, the version its program reports, and the sandbox modes that work,
    /// found anew.
    async fn examine(&self, env: &EnvironmentContext) -> Result<Found, String> {
        let build = descriptor::locate(env).map_err(|missing| missing.to_string())?;
        let version = match self.version_of(&build, env).await {
            Ok(version) => Some(version),
            Err(reason) if env.tools_dir.is_none() => {
                tracing::info!(%reason, "the local program's version is unknown");
                None
            }
            Err(reason) => return Err(reason),
        };
        if version.is_none() {
            return Ok(Found {
                program: build.program,
                version,
                modes: vec![orch_agent_contract::descriptor::SandboxMode::Open],
            });
        }
        let vars = environment(env, &codex_home(env), env.tools_dir.is_some());
        let scratch = sandbox::scratch_dir(Path::new(&env.data_dir));
        let enforceable = sandbox::probe(&Probe {
            program: &build.program,
            shell: &env.shell.path,
            scratch: &scratch,
            environment: &vars,
            timeout: self.config.probe_timeout,
        })
        .await;
        for (mode, why) in &enforceable.refused {
            tracing::info!(?mode, %why, "the sandbox mode cannot be enforced here");
        }
        Ok(Found {
            program: build.program,
            version,
            modes: enforceable.modes,
        })
    }

    /// The version the program reports for `--version`.
    async fn version_of(
        &self,
        build: &Build,
        env: &EnvironmentContext,
    ) -> Result<semver::Version, String> {
        let vars = environment(env, &codex_home(env), env.tools_dir.is_some());
        let argv = env.launch(build.program.clone());
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
            .arg("--version")
            .env_clear()
            .envs(vars.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        std::fs::create_dir_all(codex_home(env))
            .map_err(|error| format!("the program's home could not be made: {error}"))?;
        let mut child = command
            .spawn()
            .map_err(|error| format!("{} could not be run: {error}", build.program.display()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or("the version probe has no output stream")?;
        let timeout = if env.tools_dir.is_none() {
            self.config.probe_timeout.min(LOCAL_VERSION_TIMEOUT)
        } else {
            self.config.probe_timeout
        };
        let output = tokio::time::timeout(timeout, async {
            let mut bytes = Vec::new();
            stdout
                .take(VERSION_OUTPUT_LIMIT + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| error.to_string())?;
            let status = child.wait().await.map_err(|error| error.to_string())?;
            if !status.success() || bytes.len() > VERSION_OUTPUT_LIMIT as usize {
                return Err("the version probe failed or wrote too much output".to_owned());
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| {
            format!(
                "{} did not answer --version in time",
                build.program.display()
            )
        })??;
        descriptor::version_in(&String::from_utf8_lossy(&output)).ok_or_else(|| {
            format!(
                "{} did not say which version it is (it printed {:?})",
                build.program.display(),
                String::from_utf8_lossy(&output).trim()
            )
        })
    }
}

/// The program's home: a directory under the executor's own, so that nothing of a person's own Codex home is
/// read or written.
fn codex_home(env: &EnvironmentContext) -> PathBuf {
    Path::new(&env.data_dir).join("codex-home")
}

/// Minimal operating-system variables and a private home, plus work variables only for an actual server.
fn environment(
    env: &EnvironmentContext,
    home: &Path,
    with_variables: bool,
) -> Vec<(OsString, OsString)> {
    let mut vars: Vec<(OsString, OsString)> = INHERITED_VARIABLES
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect();
    let user_home = env
        .home
        .as_ref()
        .map(OsString::from)
        .or_else(|| std::env::var_os("HOME"));
    if let Some(user_home) = user_home {
        vars.push((OsString::from("HOME"), user_home));
    }
    for name in ["USER", "LOGNAME"] {
        vars.push((OsString::from(name), OsString::from(&env.user)));
    }
    vars.push((OsString::from("SHELL"), OsString::from(&env.shell.path)));
    if with_variables {
        vars.extend(
            env.variables
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        );
    }
    vars.push((OsString::from("CODEX_HOME"), home.as_os_str().to_owned()));
    vars
}

#[async_trait]
impl Executor for CodexExecServer {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn describe(&self) -> ExecutorDescriptor {
        descriptor::descriptor()
    }

    async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
        if env.os != orch_agent_contract::facts::Os::Linux {
            return ExecutorAvailability::unavailable(
                "this executor is declared for Linux; this host's native execution has not been verified",
            );
        }
        let mut found = self.found.lock().await;
        let usable = match (&*found, descriptor::locate(env)) {
            (Some(known), Ok(build))
                if known.program == build.program && env.tools_dir.is_some() =>
            {
                Ok(known.clone())
            }
            (Some(known), Ok(build)) if known.program == build.program => {
                let version = self.version_of(&build, env).await.ok();
                if version == known.version {
                    Ok(known.clone())
                } else {
                    self.examine(env).await
                }
            }
            (_, Err(missing)) => Err(missing.to_string()),
            _ => self.examine(env).await,
        };
        match usable {
            Ok(now) => {
                let report = now.version.as_ref().map_or_else(
                    || ExecutorAvailability::available("unknown", now.modes.clone()),
                    |version| descriptor::availability(version, now.modes.clone()),
                );
                *found = Some(now);
                report
            }
            Err(reason) => ExecutorAvailability::unavailable(&reason),
        }
    }

    async fn start(&mut self, env: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError> {
        if env.os != orch_agent_contract::facts::Os::Linux {
            return Err(AdapterError::HarnessUnavailable("this executor is declared for Linux; this host's native execution has not been verified".into()));
        }
        if let Some(running) = &self.running {
            let endpoint = running.endpoint.clone();
            if env.tools_dir.is_some() {
                return Ok(endpoint);
            }
            let build = match descriptor::locate(env) {
                Ok(build) => build,
                Err(missing) => {
                    self.stop().await?;
                    return Err(AdapterError::HarnessUnavailable(missing.to_string()));
                }
            };
            let version = self.version_of(&build, env).await.ok();
            if version
                .as_ref()
                .is_some_and(|version| version.to_string() == endpoint.version)
            {
                return Ok(endpoint);
            }
            self.stop().await?;
        }
        let build = descriptor::locate(env)
            .map_err(|missing: Missing| AdapterError::HarnessUnavailable(missing.to_string()))?;
        // The sandbox is probed at every start: the place may have changed since it was last asked.
        let found = self
            .examine(env)
            .await
            .map_err(AdapterError::HarnessUnavailable)?;
        *self.found.lock().await = Some(found.clone());

        reap_leftover(Path::new(&env.data_dir)).await;
        let home = codex_home(env);
        std::fs::create_dir_all(&home).map_err(|error| {
            AdapterError::Internal(format!("the program's home could not be made: {error}"))
        })?;
        let mut raw = [0u8; 32];
        rand::fill(&mut raw[..]);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let vars = environment(env, &home, true);

        let mut last = String::new();
        for attempt in 0..PORT_TRIES {
            let port = match &self.config.ports {
                None if attempt > 0 => break,
                None => 0,
                Some(range) => pick(range),
            };
            match self
                .spawn(
                    &build,
                    &vars,
                    &env.data_dir,
                    &token,
                    port,
                    &env.process_launcher,
                )
                .await
            {
                Ok(spawned) => {
                    let handshake =
                        handshake(spawned.port, &token, self.config.handshake_timeout).await;
                    let version = match handshake {
                        Ok(version) => version,
                        Err(why) => {
                            let mut running = spawned.into_running(placeholder_endpoint());
                            terminate(&mut running, &self.config).await;
                            return Err(AdapterError::HarnessError(format!(
                                "the exec server started but did not answer the handshake: {why}"
                            )));
                        }
                    };
                    if found
                        .version
                        .as_ref()
                        .is_some_and(|reported| version != *reported)
                    {
                        tracing::warn!(%version, listed = ?found.version, "the exec server is not the build its program reported");
                    }
                    let endpoint = ExecutorEndpoint {
                        address: format!("ws://127.0.0.1:{}", spawned.port),
                        token: Some(Secret::new(token)),
                        version: version.to_string(),
                        tool_sets: Vec::new(),
                        sandbox_modes: found.modes,
                    };
                    tracing::info!(port = spawned.port, %version, "the exec server listens on the loopback address");
                    self.running = Some(spawned.into_running(endpoint.clone()));
                    return Ok(endpoint);
                }
                Err(why) => last = why,
            }
        }
        Err(AdapterError::HarnessError(format!(
            "the exec server did not start: {last}"
        )))
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        if let Some(mut running) = self.running.take() {
            terminate(&mut running, &self.config).await;
        }
        Ok(())
    }
}

/// A port of the range, at random.
fn pick(range: &RangeInclusive<u16>) -> u16 {
    let width = u32::from(*range.end()).saturating_sub(u32::from(*range.start())) + 1;
    let offset = rand::random_range(0..width);
    u16::try_from(u32::from(*range.start()) + offset).unwrap_or(*range.start())
}

/// A server that has announced its address.
struct Spawned {
    child: Child,
    port: u16,
    pid_file: PathBuf,
    tasks: Vec<JoinHandle<()>>,
}

impl Spawned {
    fn into_running(self, endpoint: ExecutorEndpoint) -> Running {
        Running {
            endpoint,
            child: self.child,
            pid_file: self.pid_file,
            tasks: self.tasks,
        }
    }
}

/// The endpoint of a server that is being ended before it was ever handed out.
fn placeholder_endpoint() -> ExecutorEndpoint {
    ExecutorEndpoint {
        address: String::new(),
        token: None,
        version: String::new(),
        tool_sets: Vec::new(),
        sandbox_modes: Vec::new(),
    }
}

impl CodexExecServer {
    /// Starts the program and waits for it to say where it listens; why not, if it did not.
    async fn spawn(
        &self,
        build: &Build,
        vars: &[(OsString, OsString)],
        data_dir: &str,
        token: &str,
        port: u16,
        launcher: &[String],
    ) -> Result<Spawned, String> {
        let digest = hex::encode(Sha256::digest(token.as_bytes()));
        let mut argv: Vec<OsString> = launcher.iter().map(OsString::from).collect();
        argv.push(build.program.as_os_str().to_owned());
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
            .arg("exec-server")
            .args(["--listen", &format!("ws://127.0.0.1:{port}")])
            .args([
                "--ws-auth",
                "capability-token",
                "--ws-token-sha256",
                &digest,
            ])
            .env_clear()
            .envs(vars.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| format!("{} could not be run: {error}", build.program.display()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or("the server has no standard output")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("the server has no standard error")?;

        let kept: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let errors = {
            let kept = kept.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "orch_executor_codex::server", "exec server: {line}");
                    let mut kept = kept
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if kept.len() == KEPT_ERROR_LINES {
                        kept.pop_front();
                    }
                    kept.push_back(line);
                }
            })
        };
        let mut lines = BufReader::new(stdout).lines();
        let announced = tokio::time::timeout(self.config.start_timeout, lines.next_line()).await;
        let why_not = |kept: &Arc<Mutex<VecDeque<String>>>, what: String| {
            let kept = kept
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let said = kept.iter().cloned().collect::<Vec<_>>().join("; ");
            if said.is_empty() {
                what
            } else {
                format!("{what}: {said}")
            }
        };
        let line = match announced {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                // The server has gone; its standard error may still be arriving.
                let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                return Err(why_not(
                    &kept,
                    "the exec server ended without announcing an address".to_owned(),
                ));
            }
            Ok(Err(error)) => {
                kill_child(&mut child);
                return Err(format!(
                    "the exec server's output could not be read: {error}"
                ));
            }
            Err(_) => {
                kill_child(&mut child);
                return Err(why_not(
                    &kept,
                    format!(
                        "the exec server announced no address within {} seconds",
                        self.config.start_timeout.as_secs()
                    ),
                ));
            }
        };
        let announced_port = line
            .trim()
            .strip_prefix("ws://127.0.0.1:")
            .and_then(|rest| rest.trim_end_matches('/').parse::<u16>().ok())
            .filter(|found| *found != 0);
        let Some(announced_port) = announced_port else {
            kill_child(&mut child);
            return Err(format!(
                "the exec server announced {:?}, which is not an address on the loopback interface",
                line.trim()
            ));
        };
        let output = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "orch_executor_codex::server", "exec server: {line}");
            }
        });
        // What names the server, for the node that comes after this one if this one dies without ending it.
        let pid_file = Path::new(&data_dir).join(PID_FILE);
        if let Some(pid) = child.id() {
            let _ = std::fs::write(&pid_file, format!("{pid}\n{digest}\n"));
        }
        Ok(Spawned {
            child,
            port: announced_port,
            pid_file,
            tasks: vec![errors, output],
        })
    }
}

/// Ends a server: `TERM` to its process group, then `KILL` once the grace is over. What names it is removed.
async fn terminate(running: &mut Running, config: &CodexConfig) {
    signal_group(&running.child, false);
    if tokio::time::timeout(config.stop_grace, running.child.wait())
        .await
        .is_err()
    {
        signal_group(&running.child, true);
        let _ = running.child.start_kill();
        let _ = running.child.wait().await;
    }
    let _ = std::fs::remove_file(&running.pid_file);
    for task in &running.tasks {
        task.abort();
    }
}

/// Ends a server that an earlier node left behind, if the file in the executor's directory names a process
/// that is that server. Nothing is signalled that the command line does not prove to be the server.
async fn reap_leftover(data_dir: &Path) {
    let file = data_dir.join(PID_FILE);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let _ = std::fs::remove_file(&file);
    let mut words = text.split_whitespace();
    // A process id of a process that can be a server: not 0 (a group), not 1 (init), not negative.
    let pid = words
        .next()
        .and_then(|p| p.parse::<i32>().ok())
        .filter(|pid| *pid > 1);
    let (Some(pid), Some(digest)) = (pid, words.next()) else {
        return;
    };
    if !is_our_server(pid, digest) {
        return;
    }
    tracing::warn!(
        pid,
        "an exec server left behind by an earlier node is ended"
    );
    #[cfg(unix)]
    if let Some(pid) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
        let until = tokio::time::Instant::now() + LEFTOVER_GRACE;
        while rustix::process::test_kill_process(pid).is_ok() && tokio::time::Instant::now() < until
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// Whether process `pid` is an exec server that was started with the token whose digest is `digest`, by its
/// command line. Where the command line cannot be read, nothing is proven.
fn is_our_server(pid: i32, digest: &str) -> bool {
    let Ok(command_line) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let words: Vec<&[u8]> = command_line.split(|byte| *byte == 0).collect();
    words.iter().any(|word| *word == b"exec-server")
        && words
            .windows(2)
            .any(|pair| pair[0] == b"--ws-token-sha256" && pair[1] == digest.as_bytes())
}

/// Ends a server that did not start well, and what it started.
fn kill_child(child: &mut Child) {
    signal_group(child, true);
    let _ = child.start_kill();
}

/// Sends `TERM`, or `KILL`, to the server's process group.
#[cfg(unix)]
fn signal_group(child: &Child, kill: bool) {
    let signal = if kill {
        rustix::process::Signal::KILL
    } else {
        rustix::process::Signal::TERM
    };
    if let Some(pid) = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

/// Without process groups the server alone can be ended, and `terminate` does it.
#[cfg(not(unix))]
fn signal_group(_child: &Child, _kill: bool) {}

/// What a started server says of itself.
async fn handshake(port: u16, token: &str, within: Duration) -> Result<semver::Version, String> {
    let work = async {
        let mut request = format!("ws://127.0.0.1:{port}")
            .into_client_request()
            .map_err(|error| error.to_string())?;
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|error| error.to_string())?,
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|error| format!("the connection was refused: {error}"))?;
        let initialize = serde_json::json!({
            "id": 1, "method": "initialize", "params": { "clientName": "orch-exec-node" }
        });
        socket
            .send(Message::text(initialize.to_string()))
            .await
            .map_err(|error| error.to_string())?;
        let answer = loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let value: serde_json::Value =
                        serde_json::from_str(text.as_str()).map_err(|error| error.to_string())?;
                    if value.get("id") == Some(&serde_json::json!(1)) {
                        break value;
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Err("the server closed the connection".to_owned());
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.to_string()),
            }
        };
        let _ = socket.close(None).await;
        if let Some(error) = answer.get("error") {
            return Err(format!("the server refused to initialize: {error}"));
        }
        let version = answer
            .pointer("/result/environmentInfo/executorVersion")
            .and_then(serde_json::Value::as_str)
            .ok_or("the server did not say which version it is")?;
        semver::Version::parse(version)
            .map_err(|error| format!("{version:?} is no version: {error}"))
    };
    tokio::time::timeout(within, work)
        .await
        .map_err(|_| format!("no answer within {} seconds", within.as_secs()))?
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn local_environment(root: &Path, source: &str) -> (EnvironmentContext, PathBuf) {
        let program = root.join("codex");
        std::fs::write(&program, source).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let context = serde_json::from_value(serde_json::json!({
            "environment_id":"0199abcd-1234-7000-8000-0000000000e1","generation":1,
            "data_dir":root.join("data"),"workspace_root":root,"home":null,"tools_dir":null,"tools":[],
            "local_programs":{"codex":program.clone()},"mounts":[],"user":"test",
            "shell":{"name":"sh","path":"/bin/sh"},"os":"linux","arch":"x86_64",
            "path_style":"posix","limits":{},"variables":{},"mcp_servers_enabled":false
        })).unwrap();
        (context, program)
    }

    #[tokio::test]
    async fn a_confirmed_program_that_does_not_report_a_version_is_offered_with_open_only() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = local_environment(root.path(), "#!/bin/sh\nexec /bin/sleep 5\n");
        let exec = CodexExecServer::with_config(CodexConfig {
            probe_timeout: Duration::from_millis(100),
            ..CodexConfig::default()
        });
        let report = exec.available(&env).await;
        assert!(report.available);
        assert_eq!(report.version.as_deref(), Some("unknown"));
        assert_eq!(
            report.sandbox_modes,
            [orch_agent_contract::descriptor::SandboxMode::Open]
        );
    }

    #[tokio::test]
    async fn a_program_replaced_at_the_same_path_reports_its_new_version() {
        let root = tempfile::tempdir().unwrap();
        let (env, program) = local_environment(
            root.path(),
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'codex-cli 1.0.0'; exit 0; fi\nexit 1\n",
        );
        let exec = CodexExecServer::new();
        assert_eq!(exec.available(&env).await.version.as_deref(), Some("1.0.0"));
        std::fs::write(program, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'codex-cli 1.1.0'; exit 0; fi\nexit 1\n").unwrap();
        assert_eq!(exec.available(&env).await.version.as_deref(), Some("1.1.0"));
    }

    #[tokio::test]
    async fn version_and_sandbox_probes_do_not_receive_environment_secrets() {
        let root = tempfile::tempdir().unwrap();
        let source = "#!/bin/sh\nprintf '%s' \"${ORCH_EXEC_TEST_SECRET-unset}\" >> \"$0.env\"\nif [ \"$1\" = --version ]; then echo 'codex-cli 1.0.0'; exit 0; fi\nexit 1\n";
        let (mut env, program) = local_environment(root.path(), source);
        env.variables
            .insert("ORCH_EXEC_TEST_SECRET".into(), "synthetic-secret".into());
        let report = CodexExecServer::new().available(&env).await;
        assert!(report.available);
        let observed = std::fs::read_to_string(format!("{}.env", program.display())).unwrap();
        assert!(!observed.contains("synthetic-secret"));
        assert!(observed.contains("unset"));
    }
}
