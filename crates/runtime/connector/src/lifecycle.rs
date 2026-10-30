//! Lifecycle hooks shared by unit hosts and directory connectors.
//!
//! An operator names up to four executables in the machine policy's `hooks`: `pre_up` runs when a
//! unit's network exists and before the unit starts, `post_up` when it runs, `pre_down` before it stops and
//! `post_down` after. Each is given one JSON document on its standard input ([`HookDocument`]): the moment and why it
//! came (a start, a stop, a recreate, a move, a removal), the environment or the provider the unit serves, the unit,
//! its networks with their subnets and the host's and the unit's addresses there, its published ports and the host.
//! Hooks run with the hosting program's own rights. The machine's operator keeps them locally or hands this key
//! to its machine managers; managing one environment alone grants no authority over these programs.
//! they serve whatever the operator wants done around a unit: route its subnet through a VPN, open a firewall, tell a
//! monitor.
//!
//! **Up hooks fail closed.** A `pre_up` that exits other than 0, cannot be run or runs out of `timeout_secs` keeps
//! the unit from starting, and a `post_up` that does stops it again; the start fails with the first lines the hook
//! printed, for whoever manages the environment. **Down hooks are told and never hold a stop.** Every run is logged and
//! reported (`node.hook`), and the system writes it as a line of the history of what the unit serves: the moment,
//! why, the exit and how long it took, never what the hook printed. When the node starts and finds units running, it
//! runs `post_up` for each again, marked as a replay, so hooks must be idempotent; a unit that came up and ended by
//! itself is given its `post_down` when the node next looks.
//!
//! A hook is run directly, never through a shell, in a process group of its own, with an environment of its own that
//! holds the usual process settings and nothing else of the node's (none of its secrets reaches a hook). What it prints
//! is read up to a bound and the rest drained, so that a talkative hook never blocks; past its time its whole process
//! group is killed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use orch_api_contract::ports::PortTransport;
use orch_driver_contract::network::{EgressMode, UnitNetworkAddress};
use orch_driver_contract::runtime::{
    RuntimeKind, RuntimeSpec, SubjectEnvironment, SubjectProvider, UnitSubject,
};
use orch_link_contract::events::NodeHook;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub use orch_driver_contract::runtime::UnitReason;
pub use orch_link_contract::events::HookEvent;

/// The version of the document's shape, for a hook that wants to check what it reads.
pub const DOCUMENT_VERSION: u32 = 1;

/// How much is kept of each of a run's two output streams: enough for the first lines of a script's complaint, and
/// no store of logs. What comes after is read and dropped, so that the hook is never held up by a full pipe.
const OUTPUT_KEEP_BYTES: usize = 16 * 1024;

/// How many of the first lines a failing hook printed go with the failure it caused.
const FIRST_LINES: usize = 10;

/// How many characters of those lines at most: a failure message, not a log.
const FIRST_CHARS: usize = 1_000;

/// How long the output of a hook that ended (or was killed) is still read: what was in its pipes is there at once,
/// and a child that kept them open past the group's end is not waited for.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);

/// The variables a hook inherits from the node: the usual process settings, and nothing that could hold a secret.
const INHERITED: [&str; 7] = ["PATH", "HOME", "USER", "LOGNAME", "LANG", "TZ", "TMPDIR"];

/// The path a hook is given where the node has none: the usual places of programs.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// The four executables and the time each run may take, as the operator configured them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSettings {
    /// Run when the unit's network exists and before the unit starts.
    pub pre_up: Option<PathBuf>,
    /// Run when the unit runs.
    pub post_up: Option<PathBuf>,
    /// Run before the unit stops.
    pub pre_down: Option<PathBuf>,
    /// Run after the unit stopped.
    pub post_down: Option<PathBuf>,
    /// How long one run may take before its process group is killed and it counts as failed.
    pub timeout: Duration,
}

impl Default for HookSettings {
    fn default() -> Self {
        Self::from_policy(&orch_link_contract::policy::MachineHooks::default())
    }
}

impl HookSettings {
    /// The machine's own policy, shared by directory connectors and nodes.
    #[must_use]
    pub fn from_policy(hooks: &orch_link_contract::policy::MachineHooks) -> Self {
        Self {
            pre_up: hooks.pre_up.as_ref().map(PathBuf::from),
            post_up: hooks.post_up.as_ref().map(PathBuf::from),
            pre_down: hooks.pre_down.as_ref().map(PathBuf::from),
            post_down: hooks.post_down.as_ref().map(PathBuf::from),
            timeout: Duration::from_secs(hooks.timeout_secs.max(1)),
        }
    }
    /// The executable of `event`, if the operator named one.
    #[must_use]
    pub fn program(&self, event: HookEvent) -> Option<&Path> {
        match event {
            HookEvent::PreUp => self.pre_up.as_deref(),
            HookEvent::PostUp => self.post_up.as_deref(),
            HookEvent::PreDown => self.pre_down.as_deref(),
            HookEvent::PostDown => self.post_down.as_deref(),
        }
    }

    /// Whether the operator named any hook.
    #[must_use]
    pub fn any(&self) -> bool {
        [
            HookEvent::PreUp,
            HookEvent::PostUp,
            HookEvent::PreDown,
            HookEvent::PostDown,
        ]
        .into_iter()
        .any(|event| self.program(event).is_some())
    }
}

/// What the node keeps of a unit's request for its hooks: what it serves and why it was made, the network mode of
/// its template and the environment it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitAbout {
    /// What the system said the unit serves; none for a request made without it.
    #[serde(default)]
    pub subject: Option<UnitSubject>,
    /// The template's `network.mode` (`bridge` or `none`), as the request's spec says it.
    #[serde(default)]
    pub network_mode: Option<String>,
    /// The environment of the unit, from the label the system finds it by.
    #[serde(default)]
    pub environment_id: Option<Id>,
}

/// The label of a unit's request that names its environment.
const LABEL_ENVIRONMENT: &str = "orch.environment_id";

impl UnitAbout {
    /// What the node keeps of `spec` for the hooks.
    #[must_use]
    pub fn of<C>(spec: &RuntimeSpec<C>) -> Self {
        Self {
            subject: spec.subject.clone(),
            network_mode: spec
                .spec
                .get("network")
                .and_then(|network| network.get("mode"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            environment_id: spec
                .subject
                .as_ref()
                .and_then(|subject| subject.environment.as_ref())
                .map(|environment| environment.id)
                .or_else(|| {
                    spec.labels
                        .get(LABEL_ENVIRONMENT)
                        .and_then(|id| id.parse().ok())
                }),
        }
    }

    /// Why a start is: for a unit's first, why it was made; for any later one an ordinary start.
    #[must_use]
    pub fn up_reason(about: Option<&Self>, first_start: bool) -> UnitReason {
        match about.and_then(|about| about.subject.as_ref()) {
            Some(subject) if first_start => subject.made_for,
            _ => UnitReason::Start,
        }
    }
}

/// Where a unit stands as far as its hooks are concerned: it came up through a start the node carried out, or it went
/// down. A unit that came up and is found ended is given its `post_down`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    /// It came up.
    Up,
    /// It went down.
    Down,
}

/// One host opening the node holds for the unit's environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookPort {
    /// The opening.
    pub opening_id: Id,
    /// The number on the host.
    pub host_port: u16,
    /// The port inside the environment it reaches.
    pub port: u16,
    /// The transports bound.
    pub transports: Vec<PortTransport>,
}

/// What tells the hooks which host openings an environment has on this node.
pub trait PublishedPorts: Send + Sync {
    /// The openings this node holds for `environment`.
    fn ports_of(&self, environment: Id) -> Vec<HookPort>;
}

/// The host, as a hook is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookHost {
    /// The system the node belongs to.
    pub system: String,
    /// The host's name, as its operating system says it; empty where it says none.
    pub name: String,
}

/// The runtime, as a hook is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookRuntime {
    /// Its identifier.
    pub id: Id,
    /// Its pool.
    pub kind: RuntimeKind,
    /// The driver that made its unit.
    pub driver: String,
}

/// The unit, as a hook is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookUnit {
    /// Its name.
    pub name: String,
    /// The driver's own reference for it (a container's identifier).
    pub id: String,
}

/// The unit's network, as a hook is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookNetwork {
    /// The template's network mode: `bridge`, or `none` for Orch's transport alone.
    pub mode: String,
    /// How it reaches out: `open`, or `restricted` through its proxy.
    pub egress: EgressMode,
    /// The networks the unit and its proxy stand on, each with its subnet, gateway and their address.
    pub networks: Vec<UnitNetworkAddress>,
}

/// The document a hook reads on its standard input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookDocument {
    /// The version of this shape: [`DOCUMENT_VERSION`].
    pub version: u32,
    /// The moment.
    pub event: HookEvent,
    /// Why it came.
    pub reason: UnitReason,
    /// Whether this is a run again for a unit the node found running when it started.
    pub replay: bool,
    /// When.
    pub at: Time,
    /// The host.
    pub host: HookHost,
    /// The runtime.
    pub runtime: HookRuntime,
    /// The environment the unit serves; `null` for a provider's runtime or a request that did not say.
    pub environment: Option<SubjectEnvironment>,
    /// The provider a provider's runtime serves; `null` otherwise.
    pub provider: Option<SubjectProvider>,
    /// The unit.
    pub unit: HookUnit,
    /// Its network.
    pub network: HookNetwork,
    /// The host openings the node holds for its environment.
    pub ports: Vec<HookPort>,
}

/// How one run of a hook ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRun {
    /// The moment it ran at.
    pub event: HookEvent,
    /// Why.
    pub reason: UnitReason,
    /// Whether it was a run again at the node's start.
    pub replay: bool,
    /// When it began.
    pub at: Time,
    /// How long it took.
    pub duration: Duration,
    /// Its exit status; none when it was killed or could not be run.
    pub exit: Option<i32>,
    /// Whether it ran out of its time and was killed.
    pub timed_out: bool,
    /// Whether it counts as failed.
    pub failed: bool,
    /// The first lines it printed (its error output when it wrote any, else its standard output), or why it could
    /// not be run; bounded, with control characters replaced.
    pub first_lines: String,
}

impl HookRun {
    /// What the node reports of the run: no word of what it printed.
    #[must_use]
    pub fn report(&self, runtime_id: Id) -> NodeHook {
        NodeHook {
            runtime_id,
            event: self.event,
            reason: self.reason,
            replay: self.replay,
            at: self.at,
            duration_ms: i64::try_from(self.duration.as_millis()).unwrap_or(i64::MAX),
            exit: self.exit,
            timed_out: self.timed_out,
            failed: self.failed,
        }
    }

    /// Why a start failed because of this run, in a sentence for whoever manages the environment, with the first
    /// lines the hook printed.
    #[must_use]
    pub fn why(&self) -> String {
        let what = if self.timed_out {
            format!(
                "ran out of its {} seconds and was stopped",
                self.duration.as_secs().max(1)
            )
        } else {
            match self.exit {
                Some(code) => format!("exited with {code}"),
                None => "could not be run".to_owned(),
            }
        };
        let said = if self.first_lines.is_empty() {
            String::new()
        } else {
            format!(": {}", self.first_lines)
        };
        format!(
            "the host's {} hook {what}, so the unit does not run{said}",
            self.event.as_str()
        )
    }
}

/// Reads a stream to its end, keeping the first [`OUTPUT_KEEP_BYTES`] and dropping the rest.
async fn read_bounded(mut stream: impl AsyncRead + Unpin) -> Vec<u8> {
    const PIECE: usize = 4096;
    let mut kept = Vec::new();
    let mut piece = [0u8; PIECE];
    loop {
        match stream.read(&mut piece).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = OUTPUT_KEEP_BYTES.saturating_sub(kept.len());
                kept.extend_from_slice(&piece[..read.min(room)]);
            }
        }
    }
    kept
}

/// The first lines of what was printed, shortened to the display's line and character limits,
/// control characters replaced, joined by ` / `.
#[must_use]
pub fn first_lines(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            line.chars()
                .map(|c| if c.is_control() { '?' } else { c })
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .filter(|line| !line.is_empty())
        .take(FIRST_LINES)
        .collect();
    let joined = lines.join(" / ");
    if joined.chars().count() > FIRST_CHARS {
        let mut cut: String = joined.chars().take(FIRST_CHARS).collect();
        cut.push('…');
        cut
    } else {
        joined
    }
}

/// Runs the operator's hooks.
#[derive(Debug, Clone)]
pub struct Hooks {
    settings: Arc<HookSettings>,
    policy: Arc<std::sync::Mutex<Option<Arc<crate::policy::PolicyStore>>>>,
}

impl Hooks {
    /// Hooks as `settings` name them.
    #[must_use]
    pub fn new(settings: HookSettings) -> Self {
        Self {
            settings: Arc::new(settings),
            policy: Arc::default(),
        }
    }

    /// Uses the live machine policy, including changes made while a unit is running.
    pub fn set_policy(&self, policy: Arc<crate::policy::PolicyStore>) {
        *self
            .policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(policy);
    }

    /// Lifecycle programs of a directory-hosting connector.
    #[must_use]
    pub fn for_machine(policy: Arc<crate::policy::PolicyStore>) -> Self {
        let hooks = Self::new(HookSettings::default());
        hooks.set_policy(policy);
        hooks
    }

    fn current(&self) -> HookSettings {
        self.policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|policy| policy.policy())
            .map_or_else(
                || (*self.settings).clone(),
                |policy| HookSettings::from_policy(&policy.hooks),
            )
    }

    /// Whether the operator named a hook for `event`.
    #[must_use]
    pub fn wants(&self, event: HookEvent) -> bool {
        self.current().program(event).is_some()
    }

    /// Whether the operator named any hook.
    #[must_use]
    pub fn any(&self) -> bool {
        self.current().any()
    }

    /// Runs the hook of `document.event` with the document on its standard input and waits for it, at most its
    /// time; answers how it ended, or none when the operator named no hook for that moment.
    pub async fn run(&self, document: &HookDocument, clock: &dyn Clock) -> Option<HookRun> {
        let settings = self.current();
        let program = settings.program(document.event)?.to_owned();
        let at = clock.time();
        let began = tokio::time::Instant::now();
        let ended = |exit: Option<i32>, timed_out: bool, failed: bool, said: String| HookRun {
            event: document.event,
            reason: document.reason,
            replay: document.replay,
            at,
            duration: began.elapsed(),
            exit,
            timed_out,
            failed,
            first_lines: said,
        };
        let input = match serde_json::to_vec(document) {
            Ok(input) => input,
            Err(error) => {
                return Some(ended(
                    None,
                    false,
                    true,
                    format!("its document could not be written: {error}"),
                ));
            }
        };
        let mut command = tokio::process::Command::new(&program);
        command.env_clear();
        for (name, value) in std::env::vars() {
            if INHERITED.contains(&name.as_str()) || name.starts_with("LC_") {
                command.env(name, value);
            }
        }
        if std::env::var_os("PATH").is_none() {
            command.env("PATH", DEFAULT_PATH);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let run = ended(
                    None,
                    false,
                    true,
                    format!("it could not be run ({})", error.kind()),
                );
                log(&run, &program);
                return Some(run);
            }
        };
        let stdin = child.stdin.take();
        let feeding = tokio::spawn(async move {
            if let Some(mut stdin) = stdin {
                // A hook that reads nothing closes its end first; that is its business.
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            }
        });
        let out = child
            .stdout
            .take()
            .map(|stdout| tokio::spawn(read_bounded(stdout)));
        let err = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(read_bounded(stderr)));
        let waited = tokio::time::timeout(settings.timeout, child.wait()).await;
        let (exit, timed_out) = match waited {
            Ok(Ok(status)) => (status.code(), false),
            Ok(Err(_)) => (None, false),
            Err(_) => {
                // A hook past its time goes with whatever it started; one that ended in time may have left a
                // helper running on purpose, and that is the operator's.
                kill_group(&child);
                let _ = child.kill().await;
                let _ = child.wait().await;
                (None, true)
            }
        };
        feeding.abort();
        let collect = |task: Option<tokio::task::JoinHandle<Vec<u8>>>| async move {
            match task {
                Some(task) => {
                    let aborter = task.abort_handle();
                    match tokio::time::timeout(OUTPUT_GRACE, task).await {
                        Ok(Ok(bytes)) => bytes,
                        _ => {
                            aborter.abort();
                            Vec::new()
                        }
                    }
                }
                None => Vec::new(),
            }
        };
        let (stdout, stderr) = tokio::join!(collect(out), collect(err));
        let said = if stderr.iter().any(|byte| !byte.is_ascii_whitespace()) {
            first_lines(&stderr)
        } else {
            first_lines(&stdout)
        };
        let failed = timed_out || exit != Some(0);
        let run = ended(exit, timed_out, failed, said);
        log(&run, &program);
        Some(run)
    }
}

/// Kills the whole process group a hook was started in; a group that is gone already is gone.
fn kill_group(child: &tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    #[cfg(not(unix))]
    let _ = child;
}

/// Logs a run for the host's operator: the moment, why, how it ended and how long it took, and what a failed one
/// printed first.
fn log(run: &HookRun, program: &Path) {
    let millis = run.duration.as_millis();
    if run.failed {
        tracing::warn!(
            hook = run.event.as_str(), reason = run.reason.as_str(), replay = run.replay,
            program = %program.display(), exit = ?run.exit, timed_out = run.timed_out, millis,
            first_lines = %run.first_lines, "an operator's hook failed"
        );
    } else {
        tracing::info!(
            hook = run.event.as_str(), reason = run.reason.as_str(), replay = run.replay,
            program = %program.display(), millis, "an operator's hook ran"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use orch_driver_contract::network::UnitNetworkRole;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, name: &str, body: &str) -> std::io::Result<PathBuf> {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        Ok(path)
    }

    fn document(event: HookEvent) -> Result<HookDocument, Box<dyn std::error::Error>> {
        Ok(HookDocument {
            version: DOCUMENT_VERSION,
            event,
            reason: UnitReason::Recreate,
            replay: false,
            at: "2026-10-14T15:00:00.000Z".parse()?,
            host: HookHost {
                system: "Orch".to_owned(),
                name: "host-1".to_owned(),
            },
            runtime: HookRuntime {
                id: "0199c1a0-0000-7000-8000-00000000000a".parse()?,
                kind: RuntimeKind::Environment,
                driver: "docker".to_owned(),
            },
            environment: Some(SubjectEnvironment {
                id: "0199c1a0-0000-7000-8000-00000000000b".parse()?,
                name: "payments".to_owned(),
                path: "Acme / Platform".to_owned(),
                template_id: None,
                template: "Default".to_owned(),
                generation: 3,
            }),
            provider: None,
            unit: HookUnit {
                name: "orch-environment-00000000000a".to_owned(),
                id: "c0ffee".to_owned(),
            },
            network: HookNetwork {
                mode: "bridge".to_owned(),
                egress: EgressMode::Open,
                networks: vec![UnitNetworkAddress {
                    name: "orch-environment-00000000000a-net".to_owned(),
                    role: UnitNetworkRole::Unit,
                    subnet: "10.199.0.32/28".to_owned(),
                    gateway: Some("10.199.0.33".to_owned()),
                    address: Some("10.199.0.34".to_owned()),
                }],
            },
            ports: Vec::new(),
        })
    }

    fn hooks(event: HookEvent, program: PathBuf, timeout: Duration) -> Hooks {
        let mut settings = HookSettings {
            timeout,
            ..HookSettings::default()
        };
        match event {
            HookEvent::PreUp => settings.pre_up = Some(program),
            HookEvent::PostUp => settings.post_up = Some(program),
            HookEvent::PreDown => settings.pre_down = Some(program),
            HookEvent::PostDown => settings.post_down = Some(program),
        }
        Hooks::new(settings)
    }

    #[tokio::test]
    async fn a_hook_reads_what_the_unit_is_on_its_standard_input_and_a_zero_exit_passes()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir().unwrap();
        let told = dir.path().join("told.json");
        let program = script(
            dir.path(),
            "post-up",
            &format!("cat > '{}'", told.display()),
        )?;
        let run = hooks(HookEvent::PostUp, program, Duration::from_secs(10))
            .run(&document(HookEvent::PostUp)?, &orch_types::SystemClock)
            .await
            .expect("a hook is named for the moment");
        assert!(!run.failed, "{run:?}");
        assert_eq!(run.exit, Some(0));
        let read: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&told).unwrap()).unwrap();
        assert_eq!(read["event"], "post_up");
        assert_eq!(read["reason"], "recreate");
        assert_eq!(read["environment"]["path"], "Acme / Platform");
        assert_eq!(read["network"]["networks"][0]["subnet"], "10.199.0.32/28");
        assert_eq!(read["network"]["networks"][0]["address"], "10.199.0.34");
        assert_eq!(read["network"]["egress"], "open");
        Ok(())
    }

    #[tokio::test]
    async fn a_failing_hook_says_its_first_lines_and_its_exit_and_the_report_carries_none_of_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "pre-up",
            "echo 'the tunnel wg0 is down' >&2; echo 'second line' >&2; exit 3",
        )?;
        let run = hooks(HookEvent::PreUp, program, Duration::from_secs(10))
            .run(&document(HookEvent::PreUp)?, &orch_types::SystemClock)
            .await
            .unwrap();
        assert!(run.failed);
        assert_eq!(run.exit, Some(3));
        assert_eq!(run.first_lines, "the tunnel wg0 is down / second line");
        assert!(
            run.why().contains("pre_up hook exited with 3"),
            "{}",
            run.why()
        );
        assert!(run.why().contains("the tunnel wg0 is down"));
        let report =
            serde_json::to_string(&run.report(document(HookEvent::PreUp)?.runtime.id)).unwrap();
        assert!(
            !report.contains("tunnel"),
            "nothing the hook printed is reported: {report}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_hook_past_its_time_is_killed_with_what_it_started_and_counts_as_failed()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("still-here");
        let program = script(
            dir.path(),
            "post-down",
            &format!("(sleep 3; touch '{}') & sleep 30", marker.display()),
        )?;
        let began = std::time::Instant::now();
        let run = hooks(HookEvent::PostDown, program, Duration::from_secs(1))
            .run(&document(HookEvent::PostDown)?, &orch_types::SystemClock)
            .await
            .unwrap();
        assert!(run.timed_out && run.failed && run.exit.is_none(), "{run:?}");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the run did not wait for the sleep"
        );
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(
            !marker.exists(),
            "a child the hook left behind was killed with its group"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_hook_that_cannot_be_run_fails_and_a_moment_without_one_runs_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let named = hooks(HookEvent::PreUp, missing, Duration::from_secs(5));
        let run = named
            .run(&document(HookEvent::PreUp)?, &orch_types::SystemClock)
            .await
            .unwrap();
        assert!(run.failed && run.exit.is_none() && !run.timed_out);
        assert!(run.why().contains("could not be run"), "{}", run.why());
        assert!(
            named
                .run(&document(HookEvent::PostDown)?, &orch_types::SystemClock)
                .await
                .is_none()
        );
        assert!(!Hooks::new(HookSettings::default()).any());
        Ok(())
    }

    #[tokio::test]
    async fn a_hook_is_given_no_variable_of_the_node_but_the_usual_settings()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("env.txt");
        let program = script(
            dir.path(),
            "pre-down",
            &format!("env > '{}'", seen.display()),
        )?;
        // A variable the test process has (cargo gives it several) that is none of the usual settings.
        let secret_name = std::env::vars()
            .map(|(name, _)| name)
            .find(|name| !INHERITED.contains(&name.as_str()) && !name.starts_with("LC_"));
        let run = hooks(HookEvent::PreDown, program, Duration::from_secs(10))
            .run(&document(HookEvent::PreDown)?, &orch_types::SystemClock)
            .await
            .unwrap();
        assert!(!run.failed, "{run:?}");
        let text = std::fs::read_to_string(&seen).unwrap();
        assert!(text.lines().any(|line| line.starts_with("PATH=")));
        if let Some(name) = secret_name {
            assert!(
                !text
                    .lines()
                    .any(|line| line.starts_with(&format!("{name}="))),
                "{name} reached the hook"
            );
        }
        Ok(())
    }

    #[test]
    fn what_a_hook_printed_is_shown_bounded_and_without_control_characters() {
        let long = "x".repeat(5_000);
        assert_eq!(
            first_lines(format!("{long}\n").as_bytes()).chars().count(),
            FIRST_CHARS + 1
        );
        assert_eq!(
            first_lines(b"a\x1b[31mred\x07\n\n  b  \n"),
            "a?[31mred? / b"
        );
        let many: String = (0..30).map(|n| format!("line {n}\n")).collect();
        assert_eq!(
            first_lines(many.as_bytes()).matches("line").count(),
            FIRST_LINES
        );
    }

    #[test]
    fn a_units_first_start_is_told_why_it_was_made_and_a_later_one_is_an_ordinary_start() {
        let about = UnitAbout {
            subject: Some(UnitSubject {
                made_for: UnitReason::Move,
                environment: None,
                provider: None,
            }),
            network_mode: Some("bridge".to_owned()),
            environment_id: None,
        };
        assert_eq!(UnitAbout::up_reason(Some(&about), true), UnitReason::Move);
        assert_eq!(UnitAbout::up_reason(Some(&about), false), UnitReason::Start);
        assert_eq!(UnitAbout::up_reason(None, true), UnitReason::Start);
    }
}
