//! One process for each conversation under one home: the quiet and pinned start, idle exit and resume.
//!
//! The harness is one operating-system process for each conversation. This module starts such a process
//! from a [`LaunchSpec`] (a program, its arguments, its whole environment and its working directory), which
//! a [`Launcher`] turns into a command: in production a plain program inside the runtime, in a test a
//! command that runs the same program in a container. What leaves a process is read here, line by line,
//! and handed on as [`ProcessEvent`]s; what goes in is written by a task of its own so that a slow reader
//! on the other side never holds the adapter.
//!
//! The environment of a process is built, never inherited: [`base_environment`] keeps the few variables a
//! program needs to run and the switches that start the harness quiet and pinned ([`QUIET_SWITCHES`]), so
//! that nothing of the connector's own environment (tokens, enrolment secrets) reaches an agent that can
//! run `env`.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};

use futures::StreamExt;
use orch_agent_contract::flow;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Notify, watch};
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};

use crate::config::{FRAME_MAX_BYTES, STDERR_KEPT_LINES};

// A final diagnostic is drained, but an inherited pipe must not hide a process's exit.
const EXIT_DIAGNOSTIC_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Keep the latest 64 KiB even when a native program never terminates its diagnostic line.
const STDERR_TAIL_BYTES: usize = 64 * 1024;
/// Ordinary 8 KiB reads prevent a line buffer growing behind the process watcher.
const STDERR_READ_BYTES: usize = 8 * 1024;

fn keep_diagnostic(kept: &mut VecDeque<u8>, bytes: &[u8]) {
    kept.extend(bytes);
    let excess = kept.len().saturating_sub(STDERR_TAIL_BYTES);
    kept.drain(..excess);
}

fn diagnostic_lines(kept: &VecDeque<u8>) -> Vec<String> {
    let bytes: Vec<_> = kept.iter().copied().collect();
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<_> = text
        .lines()
        .rev()
        .take(STDERR_KEPT_LINES)
        .map(str::to_owned)
        .collect();
    lines.reverse();
    lines
}

/// The switches that start the harness without anything that is not the work leaving the machine, and
/// without it updating itself. The first covers most of the others by the vendor's own description; they
/// are set as well, so that a later narrowing of it opens nothing.
pub const QUIET_SWITCHES: [(&str, &str); 7] = [
    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ("DISABLE_TELEMETRY", "1"),
    ("DISABLE_ERROR_REPORTING", "1"),
    ("DISABLE_FEEDBACK_COMMAND", "1"),
    ("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY", "1"),
    ("DISABLE_UPDATES", "1"),
    ("CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL", "1"),
];

/// The variables of the parent that a harness process is allowed to see: where programs are, who is
/// running them, how text is written, where temporary files go, and how to reach the network.
const INHERITED: [&str; 19] = [
    "PATH",
    "HOME",
    "CLAUDE_CONFIG_DIR",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "TERM",
    "NO_COLOR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "XDG_RUNTIME_DIR",
];

/// The environment a harness process starts from: what it may inherit, then the quiet switches.
#[must_use]
pub fn base_environment(
    inherited: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    let mut environment: BTreeMap<String, String> = inherited
        .into_iter()
        .filter(|(name, _)| {
            INHERITED.contains(&name.as_str())
                || INHERITED.contains(&name.to_ascii_uppercase().as_str())
        })
        .collect();
    for (name, value) in QUIET_SWITCHES {
        environment.insert(name.to_owned(), value.to_owned());
    }
    environment
}

/// Everything needed to start one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// The program: a path, or a name looked up on the `PATH` of [`LaunchSpec::env`].
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Its whole environment; nothing else is inherited.
    pub env: BTreeMap<String, String>,
    /// Its working directory.
    pub cwd: PathBuf,
    /// A short name for the logs and, in a test, for the container.
    pub label: String,
}

/// Turns a launch description into a command. The adapter never builds a command itself, so that the same
/// adapter runs a program on the machine it is on, or the same program somewhere else.
pub trait Launcher: Send + Sync + std::fmt::Debug {
    /// The command that starts the program a launch description names. Its standard streams are set by the caller.
    fn command(&self, spec: &LaunchSpec) -> Command;
}

/// Starts the program on this machine, as it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectLauncher;

impl Launcher for DirectLauncher {
    fn command(&self, spec: &LaunchSpec) -> Command {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .envs(&spec.env)
            .current_dir(&spec.cwd);
        command
    }
}

/// Why a process could not be started.
#[derive(Debug, thiserror::Error)]
#[error("{program} could not be started: {source}")]
pub struct SpawnError {
    /// The program.
    pub program: String,
    /// Why not.
    pub source: std::io::Error,
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    /// Its exit status, when it exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, when it was killed.
    pub signal: Option<i32>,
    /// The last lines it wrote to its standard error, which usually say why.
    pub stderr_tail: Vec<String>,
}

impl ExitInfo {
    /// A sentence for a log or an error.
    #[must_use]
    pub fn describe(&self) -> String {
        let how = match (self.code, self.signal) {
            (Some(code), _) => format!("exited with status {code}"),
            (None, Some(signal)) => format!("was ended by signal {signal}"),
            (None, None) => "ended".to_owned(),
        };
        let said = self.stderr_tail.join(" | ");
        if said.is_empty() {
            how
        } else {
            format!("{how}: {said}")
        }
    }
}

/// What a running process reports to whoever started it.
#[derive(Debug)]
pub enum ProcessEvent {
    /// A line of its standard output.
    Line(String),
    /// A line of its standard output that was longer than a frame may be, and was dropped.
    TooLong,
    /// A line of its standard output that could not be read as text.
    Unreadable(String),
    /// It ended; no line follows.
    Exited(ExitInfo),
}

/// A running process as the adapter holds it. Dropping it closes its input, which ends the harness.
#[derive(Debug)]
pub struct Running {
    generation: u64,
    input: flow::Sender<String>,
    close: watch::Sender<bool>,
    exited: watch::Receiver<bool>,
    kill: Option<Arc<Notify>>,
}

impl Running {
    /// Which process of its session this is: events carry the same number, so that a late line of a
    /// process that was replaced is told from a line of the new one.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Writes a line to its input; `false` when the process is gone.
    pub async fn write(&self, line: String) -> bool {
        !self.has_exited() && self.input.send(line).await.is_ok()
    }

    /// Actual child exit is observable even while its last output waits for a consumer.
    #[must_use]
    pub fn has_exited(&self) -> bool {
        *self.exited.borrow()
    }

    /// Priority cancellation does not wait behind backpressured input or output.
    pub(crate) fn kill_handle(&self) -> Option<Arc<Notify>> {
        self.kill.clone()
    }

    /// Closes its input: a harness with nothing left to do ends by itself.
    pub fn close_input(&self) {
        self.close.send_replace(true);
    }

    /// Ends it now.
    pub fn kill(&mut self) {
        if let Some(kill) = self.kill.take() {
            kill.notify_one();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let closing = *self.close.borrow();
        if !closing {
            self.kill();
        }
    }
}

/// The bounded producer and consumer of generation-tagged native process output.
pub type ProcessEventChannel = (
    flow::Sender<(u64, ProcessEvent)>,
    flow::Receiver<(u64, ProcessEvent)>,
);

/// A bounded process-event lane, measured by retained native strings and diagnostic data.
#[must_use]
pub fn event_channel() -> ProcessEventChannel {
    flow::channel(|(_, event)| {
        std::mem::size_of::<ProcessEvent>().saturating_add(match event {
            ProcessEvent::Line(line) | ProcessEvent::Unreadable(line) => line.capacity(),
            ProcessEvent::TooLong => 0,
            ProcessEvent::Exited(info) => info
                .stderr_tail
                .capacity()
                .saturating_mul(std::mem::size_of::<String>())
                .saturating_add(info.stderr_tail.iter().map(String::capacity).sum::<usize>()),
        })
    })
}

/// Starts a process and reports it to `events`, every event tagged with `generation`.
pub fn spawn(
    launcher: &dyn Launcher,
    spec: &LaunchSpec,
    generation: u64,
    events: flow::Sender<(u64, ProcessEvent)>,
) -> Result<Running, SpawnError> {
    spawn_with_exit_grace(
        launcher,
        spec,
        generation,
        events,
        crate::config::EXIT_GRACE,
    )
}

// Sessions keep their shutdown grace; metadata probes use their own run limit after stdin closes.
fn spawn_with_exit_grace(
    launcher: &dyn Launcher,
    spec: &LaunchSpec,
    generation: u64,
    events: flow::Sender<(u64, ProcessEvent)>,
    exit_grace: std::time::Duration,
) -> Result<Running, SpawnError> {
    let mut command = launcher.command(spec);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|source| SpawnError {
        program: spec.program.clone(),
        source,
    })?;
    let (input_tx, mut input_rx) = flow::channel(|line: &String| line.capacity());
    let (close_tx, mut close_rx) = watch::channel(false);
    let kill = Arc::new(Notify::new());
    let (exit_tx, exit_rx) = watch::channel(false);
    let local_reason = Arc::new(Mutex::new(None::<String>));

    if let Some(mut stdin) = child.stdin.take() {
        let kill = Arc::clone(&kill);
        let local_reason = Arc::clone(&local_reason);
        tokio::spawn(async move {
            let mut closing = false;
            loop {
                let line = tokio::select! {
                    _ = close_rx.changed(), if !closing => {
                        closing = true;
                        input_rx.close();
                        continue;
                    }
                    line = input_rx.recv() => line,
                };
                let Some(line) = line else { break };
                let writing = async {
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.flush().await
                };
                match tokio::time::timeout(crate::config::READY_TIMEOUT, writing).await {
                    Ok(Ok(())) => {}
                    ended => {
                        let reason = match ended {
                            Ok(Err(error)) => {
                                format!("the harness no longer reads its input: {error}")
                            }
                            _ => "the harness did not read its input before the request deadline"
                                .into(),
                        };
                        *local_reason.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
                        kill.notify_one();
                        break;
                    }
                }
            }
        });
    }

    let tail: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_task = child.stderr.take().map(|mut stderr| {
        let tail = Arc::clone(&tail);
        let label = spec.label.clone();
        tokio::spawn(async move {
            let mut buffer = [0; STDERR_READ_BYTES];
            loop {
                let read = match stderr.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                tracing::debug!(target: "claude_code_stderr", process = %label, "{}", String::from_utf8_lossy(&buffer[..read]));
                let mut kept = tail.lock().unwrap_or_else(PoisonError::into_inner);
                keep_diagnostic(&mut kept, &buffer[..read]);
            }
        })
    });

    // Process control stays live while a full event queue stops the stdout reader.
    let mut native_exit = exit_rx.clone();
    let stdout = child.stdout.take();
    let forward = events.clone();
    let reader_kill = Arc::clone(&kill);
    let reader_reason = Arc::clone(&local_reason);
    let stdout_task = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut lines =
                FramedRead::new(stdout, LinesCodec::new_with_max_length(FRAME_MAX_BYTES));
            let mut native_deadline = None;
            loop {
                if native_deadline.is_none() && *native_exit.borrow() {
                    native_deadline = Some(tokio::time::Instant::now() + EXIT_DIAGNOSTIC_GRACE);
                }
                let line = tokio::select! {
                    _ = native_exit.changed(), if native_deadline.is_none() => {
                        native_deadline = Some(tokio::time::Instant::now() + EXIT_DIAGNOSTIC_GRACE);
                        continue;
                    }
                    _ = async { if let Some(at) = native_deadline { tokio::time::sleep_until(at).await; } }, if native_deadline.is_some() => break,
                    line = lines.next() => line,
                };
                let event = match line {
                    Some(Ok(line)) => ProcessEvent::Line(line),
                    Some(Err(LinesCodecError::MaxLineLengthExceeded)) => ProcessEvent::TooLong,
                    Some(Err(LinesCodecError::Io(error))) => {
                        ProcessEvent::Unreadable(error.to_string())
                    }
                    None => break,
                };
                // Native EOF waiting keeps its original budget. Delivery of an already read
                // frame waits for host capacity, preserving it without keeping a live child.
                let delivering = tokio::time::Instant::now();
                if forward.send((generation, event)).await.is_err() {
                    *reader_reason.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some("the adapter stopped consuming native output".into());
                    reader_kill.notify_one();
                    break;
                }
                if let Some(deadline) = &mut native_deadline {
                    *deadline += delivering.elapsed();
                }
            }
        }
    });
    let controller_kill = Arc::clone(&kill);
    let mut stop_rx = close_tx.subscribe();
    tokio::spawn(async move {
        let mut graceful_deadline = None;
        let status = loop {
            tokio::select! {
                status = child.wait() => break status.ok(),
                _ = controller_kill.notified() => {
                    let _ = child.start_kill();
                    break child.wait().await.ok();
                }
                _ = stop_rx.changed(), if graceful_deadline.is_none() => {
                    graceful_deadline = Some(tokio::time::Instant::now() + exit_grace);
                }
                () = async { if let Some(at) = graceful_deadline { tokio::time::sleep_until(at).await; } },
                    if graceful_deadline.is_some() => {
                    let _ = child.start_kill();
                    break child.wait().await.ok();
                }
            }
        };
        exit_tx.send_replace(true);
        let deadline = tokio::time::Instant::now() + EXIT_DIAGNOSTIC_GRACE;
        if let Some(mut task) = stderr_task
            && tokio::time::timeout_at(deadline, &mut task).await.is_err()
        {
            task.abort();
        }
        // The reader enforces its native EOF budget independently. A full host queue can
        // delay delivery, while the actual-exit watch and process control are already settled.
        let _ = stdout_task.await;
        let mut stderr_tail =
            diagnostic_lines(&tail.lock().unwrap_or_else(PoisonError::into_inner));
        if let Some(reason) = local_reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            stderr_tail.push(reason);
        }
        let (code, signal) =
            status.map_or((None, None), |status| (status.code(), signal_of(&status)));
        let _ = events
            .send((
                generation,
                ProcessEvent::Exited(ExitInfo {
                    code,
                    signal,
                    stderr_tail,
                }),
            ))
            .await;
    });

    Ok(Running {
        generation,
        input: input_tx,
        close: close_tx,
        exited: exit_rx,
        kill: Some(kill),
    })
}

/// What a short-lived run of the program printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    /// The lines of its standard output.
    pub lines: Vec<String>,
    /// How it ended.
    pub exit: ExitInfo,
}

/// Why a short-lived run gave no answer.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The program could not be started.
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    /// It did not end in time.
    #[error("{0} did not end in time")]
    TimedOut(String),
    /// A metadata probe printed more than one bounded native response can hold.
    #[error("{0} printed more than the bounded metadata response can hold")]
    OutputTooLarge(String),
}

/// Runs the program to its end and returns what it printed: for the questions the harness answers on its
/// own, such as its version and the state of its sign-in. Its input is closed at once.
pub async fn run_to_end(
    launcher: &dyn Launcher,
    spec: &LaunchSpec,
    limit: std::time::Duration,
) -> Result<Finished, RunError> {
    let (events_tx, mut events) = event_channel();
    let mut running = spawn_with_exit_grace(launcher, spec, 0, events_tx, limit)?;
    running.close_input();
    let mut lines = Vec::new();
    let mut retained = 0usize;
    let collected = tokio::time::timeout(limit, async {
        while let Some((_, event)) = events.recv().await {
            match event {
                ProcessEvent::Line(line) => {
                    retained = retained
                        .saturating_add(line.capacity())
                        .saturating_add(std::mem::size_of::<String>());
                    // These are version/login/capability questions, never a whole native history.
                    if retained > FRAME_MAX_BYTES {
                        return Err(RunError::OutputTooLarge(spec.program.clone()));
                    }
                    lines.push(line);
                }
                ProcessEvent::Exited(exit) => return Ok(Some(exit)),
                ProcessEvent::TooLong => {
                    return Err(RunError::OutputTooLarge(spec.program.clone()));
                }
                ProcessEvent::Unreadable(_) => {}
            }
        }
        Ok(None)
    })
    .await;
    match collected {
        Ok(Ok(Some(exit))) => Ok(Finished { lines, exit }),
        Ok(Err(error)) => {
            running.kill();
            Err(error)
        }
        _ => {
            running.kill();
            Err(RunError::TimedOut(spec.program.clone()))
        }
    }
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn signal_of(_: &std::process::ExitStatus) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn spec(program: &str, args: &[&str]) -> LaunchSpec {
        LaunchSpec {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: base_environment(std::env::vars()),
            cwd: std::env::temp_dir(),
            label: "test".into(),
        }
    }

    async fn collect(mut events: flow::Receiver<(u64, ProcessEvent)>) -> Vec<ProcessEvent> {
        let mut seen = Vec::new();
        while let Ok(Some((_, event))) =
            tokio::time::timeout(Duration::from_secs(10), events.recv()).await
        {
            let done = matches!(event, ProcessEvent::Exited(_));
            seen.push(event);
            if done {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_harness_sees_the_variables_it_needs_and_the_quiet_switches_and_nothing_of_the_connector() {
        let environment = base_environment([
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("ORCH_ENROLLMENT_TOKEN".to_owned(), "secret".to_owned()),
            ("AWS_SECRET_ACCESS_KEY".to_owned(), "secret".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ]);
        assert_eq!(
            environment.get("PATH").map(String::as_str),
            Some("/usr/bin")
        );
        assert_eq!(environment.get("LANG").map(String::as_str), Some("C.UTF-8"));
        assert!(!environment.contains_key("ORCH_ENROLLMENT_TOKEN"));
        assert!(!environment.contains_key("AWS_SECRET_ACCESS_KEY"));
        for (switch, value) in QUIET_SWITCHES {
            assert_eq!(environment.get(switch).map(String::as_str), Some(value));
        }
    }

    #[tokio::test]
    async fn the_lines_a_process_writes_arrive_in_order_and_its_end_after_them() {
        let (tx, rx) = event_channel();
        let running = spawn(
            &DirectLauncher,
            &spec("sh", &["-c", "echo one; echo two; echo oops 1>&2; exit 3"]),
            7,
            tx,
        )
        .unwrap();
        assert_eq!(running.generation(), 7);
        let seen = collect(rx).await;
        assert!(matches!(&seen[0], ProcessEvent::Line(line) if line == "one"));
        assert!(matches!(&seen[1], ProcessEvent::Line(line) if line == "two"));
        let Some(ProcessEvent::Exited(info)) = seen.last() else {
            panic!("it ended");
        };
        assert_eq!(info.code, Some(3));
        assert_eq!(info.stderr_tail, vec!["oops".to_owned()]);
        assert!(info.describe().contains("status 3") && info.describe().contains("oops"));
    }

    #[tokio::test]
    async fn an_exited_process_is_reported_when_a_descendant_keeps_its_pipes_open() {
        let (tx, rx) = event_channel();
        let _running = spawn(
            &DirectLauncher,
            &spec("sh", &["-c", "sleep 3 & echo fatal-diagnostic >&2; exit 7"]),
            1,
            tx,
        )
        .unwrap();
        let began = std::time::Instant::now();
        let seen = collect(rx).await;
        assert!(began.elapsed() < std::time::Duration::from_secs(2));
        let Some(ProcessEvent::Exited(info)) = seen.last() else {
            panic!("the process exit is visible");
        };
        assert_eq!(info.code, Some(7));
        assert!(info.describe().contains("fatal-diagnostic"));
    }

    #[tokio::test]
    async fn what_is_written_reaches_the_process_and_closing_the_input_ends_it() {
        let (tx, rx) = event_channel();
        let running = spawn(&DirectLauncher, &spec("cat", &[]), 1, tx).unwrap();
        assert!(running.write("hello\n".to_owned()).await);
        running.close_input();
        let seen = collect(rx).await;
        assert!(matches!(&seen[0], ProcessEvent::Line(line) if line == "hello"));
        assert!(matches!(seen.last(), Some(ProcessEvent::Exited(info)) if info.code == Some(0)));
    }

    #[tokio::test]
    async fn a_process_that_is_killed_says_which_signal_ended_it() {
        let (tx, rx) = event_channel();
        let mut running = spawn(&DirectLauncher, &spec("sleep", &["30"]), 1, tx).unwrap();
        running.kill();
        let seen = collect(rx).await;
        let Some(ProcessEvent::Exited(info)) = seen.last() else {
            panic!("it ended");
        };
        assert_eq!(info.code, None);
        assert_eq!(info.signal, Some(9));
    }

    #[tokio::test]
    async fn a_short_run_gives_what_it_printed_and_how_it_ended() {
        let finished = run_to_end(
            &DirectLauncher,
            &spec("sh", &["-c", "echo 2.1.296 '(Claude Code)'; exit 0"]),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(finished.lines, vec!["2.1.296 (Claude Code)".to_owned()]);
        assert_eq!(finished.exit.code, Some(0));
        let slow = run_to_end(
            &DirectLauncher,
            &spec("sleep", &["30"]),
            Duration::from_millis(200),
        )
        .await;
        assert!(matches!(slow, Err(RunError::TimedOut(_))));
    }

    #[tokio::test]
    async fn a_metadata_probe_can_finish_after_the_session_exit_grace() {
        let finished = run_to_end(
            &DirectLauncher,
            &spec("sh", &["-c", "sleep 10; echo probe-ok"]),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert_eq!(finished.lines, vec!["probe-ok".to_owned()]);
        assert_eq!(finished.exit.code, Some(0));
        assert_eq!(finished.exit.signal, None);
    }

    #[tokio::test]
    async fn closing_a_session_input_still_enforces_the_exit_grace() {
        let (tx, rx) = event_channel();
        let running = spawn(&DirectLauncher, &spec("sleep", &["30"]), 1, tx).unwrap();
        let began = tokio::time::Instant::now();
        running.close_input();
        let events = tokio::time::timeout(
            crate::config::EXIT_GRACE + EXIT_DIAGNOSTIC_GRACE,
            collect(rx),
        )
        .await
        .unwrap();
        assert!(began.elapsed() >= crate::config::EXIT_GRACE);
        assert!(
            matches!(events.last(), Some(ProcessEvent::Exited(info)) if info.signal == Some(9))
        );
    }

    #[tokio::test]
    async fn a_program_that_is_not_there_is_an_error_of_the_start() {
        let (tx, _rx) = event_channel();
        let error = spawn(&DirectLauncher, &spec("/nonexistent/claude", &[]), 1, tx).unwrap_err();
        assert!(error.to_string().contains("could not be started"));
    }
    #[test]
    fn a_million_unterminated_diagnostic_bytes_keep_the_final_reason_in_bounded_memory() {
        let mut kept = VecDeque::new();
        let block = [b'x'; STDERR_READ_BYTES];
        for _ in 0..128 {
            keep_diagnostic(&mut kept, &block);
        }
        keep_diagnostic(&mut kept, b" final-native-failure");
        assert!(kept.len() <= STDERR_TAIL_BYTES);
        let lines = diagnostic_lines(&kept);
        assert!(lines.last().unwrap().ends_with("final-native-failure"));
        for _ in 0..100_000 {
            keep_diagnostic(&mut kept, b"another diagnostic line\n");
        }
        keep_diagnostic(&mut kept, b"the actual final cause");
        let lines = diagnostic_lines(&kept);
        assert_eq!(lines.len(), STDERR_KEPT_LINES);
        assert_eq!(lines.last().unwrap(), "the actual final cause");
    }
    #[tokio::test]
    async fn a_full_stdout_queue_bounds_retained_frames_and_does_not_block_process_cancellation() {
        let (tx, rx) = event_channel();
        let observed = tx.clone();
        let mut running = spawn(
            &DirectLauncher,
            &spec(
                "sh",
                &[
                    "-c",
                    "while :; do printf 'a bounded native output line\\n'; done",
                ],
            ),
            1,
            tx,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while observed.usage().items < flow::QUEUE_ITEMS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(observed.usage().items, flow::QUEUE_ITEMS);
        assert!(observed.usage().bytes <= flow::QUEUE_BYTES);
        let mut exited = running.exited.clone();
        running.kill();
        tokio::time::timeout(Duration::from_secs(2), exited.wait_for(|done| *done))
            .await
            .unwrap()
            .unwrap();
        assert!(
            running.has_exited(),
            "the process controller stays live while output is blocked"
        );
        drop(rx);
    }
    #[tokio::test]
    async fn buffered_native_output_survives_exit_while_the_host_consumer_is_stalled() {
        const LINES: usize = flow::QUEUE_ITEMS * 2;
        let script = format!(
            "i=0; while [ \"$i\" -lt {LINES} ]; do printf 'line-%s\\n' \"$i\"; i=$((i+1)); done"
        );
        let (tx, rx) = event_channel();
        let running = spawn(&DirectLauncher, &spec("sh", &["-c", &script]), 1, tx).unwrap();
        let mut exited = running.exited.clone();
        drop(
            tokio::time::timeout(Duration::from_secs(2), exited.wait_for(|done| *done))
                .await
                .unwrap()
                .unwrap(),
        );
        tokio::time::sleep(EXIT_DIAGNOSTIC_GRACE * 2).await;
        let events = collect(rx).await;
        let lines: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ProcessEvent::Line(line) => Some(line.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            lines.len(),
            LINES,
            "accepted native output is never discarded for a full host queue"
        );
        for (index, line) in lines.into_iter().enumerate() {
            assert_eq!(line, format!("line-{index}"));
        }
        assert!(matches!(events.last(), Some(ProcessEvent::Exited(info)) if info.code == Some(0)));
    }
}
