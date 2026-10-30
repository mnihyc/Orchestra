//! Command execution with thread and tool-call context.
//!
//! A command is run with the environment's shell, in its own process group, with a clean environment of its
//! own: a few variables every program needs, the variables the environment gives to its agents' commands, and
//! the two that say which thread and which tool call it belongs to (`ORCH_THREAD_ID`, `ORCH_ITEM_ID`), set
//! last so that nothing overrides them. The environment attributes a process to a thread by reading them.
//!
//! A command runs in the foreground, where the call waits for it, or in the background, where it gets an id
//! and runs until it ends or is stopped. Both are one mechanism: a supervising task owns the process, reads its
//! output into a bounded view and a file, and ends it when its time is up, when it is stopped, when the client
//! that asked for it has gone away (a foreground one) or when the executor stops. Ending a command is `TERM`
//! to its whole process group, and `KILL` ten seconds later to whatever is still there.
//!
//! Commands are the property of the thread that started them: a thread cannot see, read or stop another
//! thread's command.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use orch_agent_contract::facts::ShellInfo;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::descriptor::{ITEM_VARIABLE, THREAD_VARIABLE};
use crate::output::{Capture, HeadTail, OutputLimits, Rendered};
use crate::tools::ToolFault;

/// How long a foreground command may run when the call does not say: two minutes covers an ordinary build
/// or test run.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a command has between `TERM` and `KILL`.
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(10);

/// How long output is waited for after the command itself has ended: what the command wrote is in the pipe,
/// and reading it takes moments; a program it left running that still holds the pipe must not hold the call.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// How often the end of a process group is looked for while its members are given time to stop.
const GROUP_POLL: Duration = Duration::from_millis(50);

/// How much is read from a pipe at a time.
const PUMP_CHUNK: usize = 32 * 1024;

/// How many commands one thread may have running in the background.
pub const MAX_BACKGROUND_PER_THREAD: usize = 32;

/// How many commands that have ended one thread keeps the id, the answer and the output file of: the
/// oldest are forgotten first.
pub const KEPT_ENDED_PER_THREAD: usize = 32;

/// The longest a call to read a background command's output waits for more: a harness may give a tool call as
/// little as a minute, and the call must answer within it.
pub const MAX_OUTPUT_WAIT: Duration = Duration::from_secs(30);

/// The variables of the executor's own environment that a command receives. Everything else the executor
/// has, such as its token or the connector's settings, stays out of the agent's reach.
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

/// What a program that writes to a terminal does for a terminal that is not one.
const TERMINAL_KIND: &str = "dumb";

/// The shells whose `-c` takes a script and whose `trap` runs at exit, which is how the directory a command
/// leaves its shell in is learnt.
const TRAP_SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "ash"];

/// What the executor knows of the environment its commands run in.
#[derive(Debug, Clone)]
pub struct Host {
    /// The environment's shell.
    pub shell: ShellInfo,
    /// The user's home.
    pub home: Option<String>,
    /// The user the commands run as.
    pub user: String,
    /// The variables the environment gives its agents' commands, its secrets among them.
    pub variables: BTreeMap<String, String>,
    /// The environment's declared resource-group launcher, before every shell command.
    pub process_launcher: Vec<String>,
    /// Where the output of commands is kept: one directory for each thread below it.
    pub outputs_dir: PathBuf,
    /// The bounds of a command's output.
    pub limits: OutputLimits,
    /// How long a command has between `TERM` and `KILL`.
    pub stop_grace: Duration,
}

/// A command to run.
#[derive(Debug, Clone)]
pub struct Launch<'a> {
    /// The thread it belongs to.
    pub thread_id: &'a str,
    /// The tool call it comes from.
    pub call_id: &'a str,
    /// The text for the shell.
    pub command: &'a str,
    /// Where it runs.
    pub cwd: &'a Path,
    /// How long it may run; a foreground command without one gets [`DEFAULT_TIMEOUT`], a background one
    /// runs until it ends or is stopped.
    pub timeout: Option<Duration>,
    /// Whether to learn the directory the command leaves its shell in.
    pub track_directory: bool,
}

/// Why a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// By itself.
    Exited,
    /// Its time was up.
    TimedOut,
    /// It was stopped by a call.
    Stopped,
    /// The client that asked for it went away.
    Cancelled,
    /// The executor stopped.
    Shutdown,
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    /// The exit code; for a command a signal ended, 128 plus the signal, as a shell reports it.
    pub code: i32,
    /// The signal that ended it, if one did.
    pub signal: Option<i32>,
    /// Why it ended.
    pub reason: Reason,
}

/// What a foreground command left.
#[derive(Debug)]
pub struct Finished {
    /// How it ended.
    pub exit: ExitInfo,
    /// What it wrote to standard output, cut to the inline bound.
    pub stdout: Rendered,
    /// What it wrote to standard error, cut to the inline bound.
    pub stderr: Rendered,
    /// Both, in the order they arrived, cut to the inline bound.
    pub combined: Rendered,
    /// The file that holds the whole output, when the output was cut.
    pub output_file: Option<PathBuf>,
    /// The directory it left its shell in, when that was asked for and could be learnt.
    pub final_directory: Option<PathBuf>,
}

/// A command that runs in the background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// Its id, for the calls that read and stop it.
    pub process_id: String,
    /// The file its output is written to.
    pub output_file: PathBuf,
}

/// What a background command has written since it was last asked.
#[derive(Debug)]
pub struct Progress {
    /// The new output, cut to the inline bound.
    pub output: Rendered,
    /// How it ended, once it has.
    pub exit: Option<ExitInfo>,
    /// The file that holds the whole output.
    pub output_file: PathBuf,
}

/// Which of its pipes a chunk of output came from.
#[derive(Debug, Clone, Copy)]
enum Pipe {
    Out,
    Err,
}

/// The mutable part of a command.
struct Inner {
    capture: Capture,
    combined: HeadTail,
    stdout: HeadTail,
    stderr: HeadTail,
    unread: HeadTail,
    exit: Option<ExitInfo>,
    final_directory: Option<PathBuf>,
}

/// One command, running or ended.
pub struct Process {
    id: String,
    thread_id: String,
    background: bool,
    output_path: PathBuf,
    directory_path: Option<PathBuf>,
    stop: CancellationToken,
    changed: Notify,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Process")
            .field("id", &self.id)
            .field("thread_id", &self.thread_id)
            .finish_non_exhaustive()
    }
}

impl Process {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // The state holds plain data and is never left half-changed by a panic.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn take_in(&self, pipe: Pipe, bytes: &[u8]) {
        {
            let mut inner = self.lock();
            inner.capture.push(bytes);
            inner.combined.push(bytes);
            inner.unread.push(bytes);
            match pipe {
                Pipe::Out => inner.stdout.push(bytes),
                Pipe::Err => inner.stderr.push(bytes),
            }
        }
        self.changed.notify_waiters();
    }

    /// Whether the command is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.lock().exit.is_none()
    }

    /// How the command ended, once it has.
    #[must_use]
    pub fn exit(&self) -> Option<ExitInfo> {
        self.lock().exit
    }

    /// Where the whole output is.
    #[must_use]
    pub fn output_file(&self) -> &Path {
        &self.output_path
    }

    /// Waits until the command has ended and its output is complete.
    pub async fn wait_done(&self) -> ExitInfo {
        loop {
            // The wake-up is registered before the state is looked at, so that none is lost between them.
            let notified = self.changed.notified();
            if let Some(exit) = self.exit() {
                return exit;
            }
            notified.await;
        }
    }
}

/// The processes of one process group, by the number of its leader.
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct Group(rustix::process::Pid);

#[cfg(unix)]
impl Group {
    /// The group a child leads, when it has been started in a group of its own.
    fn of(child: &Child) -> Option<Self> {
        let raw = i32::try_from(child.id()?).ok()?;
        rustix::process::Pid::from_raw(raw).map(Self)
    }

    /// Sends `signal` to every process of the group. A group that is gone is not an error.
    fn signal(self, signal: rustix::process::Signal) {
        let _ = rustix::process::kill_process_group(self.0, signal);
    }

    fn term(self) {
        self.signal(rustix::process::Signal::TERM);
    }

    fn kill(self) {
        self.signal(rustix::process::Signal::KILL);
    }

    /// Whether any process of the group is still there.
    fn exists(self) -> bool {
        rustix::process::test_kill_process_group(self.0).is_ok()
    }
}

/// Without process groups the child alone can be ended.
#[cfg(not(unix))]
#[derive(Debug, Clone, Copy)]
struct Group;

#[cfg(not(unix))]
impl Group {
    fn of(_child: &Child) -> Option<Self> {
        None
    }
    fn term(self) {}
    fn kill(self) {}
    fn exists(self) -> bool {
        false
    }
}

/// What a thread has in this executor.
#[derive(Default)]
struct ThreadState {
    /// The working directory the session was last told to start in.
    start_directory: Option<String>,
    /// The directory the session's shell is in now.
    directory: Option<PathBuf>,
    /// Its commands, oldest first.
    processes: Vec<Arc<Process>>,
}

/// The commands of every thread, and where they run.
#[derive(Clone)]
pub struct Commands {
    host: Arc<Host>,
    baseline: Arc<Vec<(OsString, OsString)>>,
    threads: Arc<Mutex<HashMap<String, ThreadState>>>,
    root: CancellationToken,
}

impl std::fmt::Debug for Commands {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Commands").finish_non_exhaustive()
    }
}

impl Commands {
    /// The commands of an executor that runs for `host`; they all end when `root` is cancelled.
    #[must_use]
    pub fn new(host: Host, root: CancellationToken) -> Self {
        let mut baseline: Vec<(OsString, OsString)> = INHERITED_VARIABLES
            .iter()
            .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
            .collect();
        let home = host
            .home
            .as_ref()
            .map(OsString::from)
            .or_else(|| std::env::var_os("HOME"));
        if let Some(home) = home {
            baseline.push((OsString::from("HOME"), home));
        }
        for name in ["USER", "LOGNAME"] {
            baseline.push((OsString::from(name), OsString::from(&host.user)));
        }
        baseline.push((OsString::from("SHELL"), OsString::from(&host.shell.path)));
        baseline.push((OsString::from("TERM"), OsString::from(TERMINAL_KIND)));
        Self {
            host: Arc::new(host),
            baseline: Arc::new(baseline),
            threads: Arc::new(Mutex::new(HashMap::new())),
            root,
        }
    }

    fn threads(&self) -> MutexGuard<'_, HashMap<String, ThreadState>> {
        self.threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The directory where the output of `thread`'s commands is kept.
    #[must_use]
    pub fn output_dir(&self, thread: &str) -> PathBuf {
        self.host.outputs_dir.join(thread)
    }

    /// The directory the session's shell is in: `start` when the session has not been seen, or has been
    /// told to start elsewhere than before, else the one its last command left it in.
    #[must_use]
    pub fn directory(&self, thread: &str, start: &str) -> PathBuf {
        let mut threads = self.threads();
        let state = threads.entry(thread.to_owned()).or_default();
        if state.start_directory.as_deref() != Some(start) {
            state.start_directory = Some(start.to_owned());
            state.directory = Some(PathBuf::from(start));
        }
        state
            .directory
            .clone()
            .unwrap_or_else(|| PathBuf::from(start))
    }

    /// Notes the directory the session's shell is in.
    pub fn set_directory(&self, thread: &str, directory: PathBuf) {
        self.threads()
            .entry(thread.to_owned())
            .or_default()
            .directory = Some(directory);
    }

    /// Runs a command and waits for it. When `cancel` fires (the client has gone away) the command is ended.
    pub async fn run(
        &self,
        launch: &Launch<'_>,
        cancel: &CancellationToken,
    ) -> Result<Finished, ToolFault> {
        let process = self.start(launch, false, Some(cancel.clone()))?;
        let exit = process.wait_done().await;
        let finished = {
            let inner = process.lock();
            Finished {
                exit,
                stdout: inner.stdout.render(),
                stderr: inner.stderr.render(),
                combined: inner.combined.render(),
                output_file: (inner.capture.total() > self.host.limits.inline_bytes as u64)
                    .then(|| process.output_path.clone()),
                final_directory: inner.final_directory.clone(),
            }
        };
        if finished.output_file.is_none() {
            self.forget(&process);
        }
        Ok(finished)
    }

    /// Starts a command in the background.
    pub fn start_background(&self, launch: &Launch<'_>) -> Result<Started, ToolFault> {
        let process = self.start(launch, true, None)?;
        Ok(Started {
            process_id: process.id.clone(),
            output_file: process.output_path.clone(),
        })
    }

    /// What `process_id` of `thread` has written since it was last asked, waiting up to `wait` for something
    /// to have come or for the command to end.
    pub async fn output(
        &self,
        thread: &str,
        process_id: &str,
        wait: Duration,
    ) -> Result<Progress, ToolFault> {
        let process = self.find(thread, process_id)?;
        let wait = wait.min(MAX_OUTPUT_WAIT);
        let deadline = Instant::now() + wait;
        loop {
            let notified = process.changed.notified();
            {
                let mut inner = process.lock();
                let ready = !inner.unread.is_empty() || inner.exit.is_some();
                if ready || Instant::now() >= deadline {
                    let output = inner.unread.render();
                    inner.unread.clear();
                    return Ok(Progress {
                        output,
                        exit: inner.exit,
                        output_file: process.output_path.clone(),
                    });
                }
            }
            // Either something came or the time is up; the next turn of the loop answers in both cases.
            let _ = tokio::time::timeout_at(deadline, notified).await;
        }
    }

    /// Ends `process_id` of `thread`: `TERM`, then `KILL` after the grace. How it ended.
    pub async fn stop(&self, thread: &str, process_id: &str) -> Result<ExitInfo, ToolFault> {
        let process = self.find(thread, process_id)?;
        process.stop.cancel();
        Ok(process.wait_done().await)
    }

    /// Ends every command of every thread and removes what they kept.
    pub async fn shutdown(&self) {
        self.root.cancel();
        let all: Vec<Arc<Process>> = self
            .threads()
            .values()
            .flat_map(|state| state.processes.iter().cloned())
            .collect();
        let waiting = async {
            for process in &all {
                process.wait_done().await;
            }
        };
        // The commands have the grace to end in, and a moment more for their output to be gathered.
        let _ = tokio::time::timeout(self.host.stop_grace + DRAIN_GRACE * 4, waiting).await;
        self.threads().clear();
        let _ = std::fs::remove_dir_all(&self.host.outputs_dir);
    }

    fn find(&self, thread: &str, process_id: &str) -> Result<Arc<Process>, ToolFault> {
        self.threads()
            .get(thread)
            .and_then(|state| state.processes.iter().find(|p| p.id == process_id))
            .cloned()
            .ok_or_else(|| {
                ToolFault::NoSuchProcess(format!(
                    "this thread has no command with the id {process_id}"
                ))
            })
    }

    /// Drops a command that has ended from the thread's table, and what it kept.
    fn forget(&self, process: &Arc<Process>) {
        if let Some(state) = self.threads().get_mut(&process.thread_id) {
            state.processes.retain(|p| p.id != process.id);
        }
        remove_files(process);
    }

    /// Starts a command and returns it running.
    fn start(
        &self,
        launch: &Launch<'_>,
        background: bool,
        cancel: Option<CancellationToken>,
    ) -> Result<Arc<Process>, ToolFault> {
        if !launch.cwd.is_dir() {
            return Err(ToolFault::NotFound(format!(
                "the working directory {} does not exist",
                launch.cwd.display()
            )));
        }
        let id = self.new_id();
        let dir = self.output_dir(launch.thread_id);
        std::fs::create_dir_all(&dir)
            .map_err(|error| ToolFault::io("make the output directory", &dir, error))?;
        {
            let mut threads = self.threads();
            let state = threads.entry(launch.thread_id.to_owned()).or_default();
            if background {
                let running = state
                    .processes
                    .iter()
                    .filter(|p| p.background && p.is_running())
                    .count();
                if running >= MAX_BACKGROUND_PER_THREAD {
                    return Err(ToolFault::TooManyProcesses(format!(
                        "this thread already has {running} commands running in the background, the most it may have; stop one first"
                    )));
                }
            }
            drop_oldest_ended(state);
        }
        let output_path = dir.join(format!("{id}.log"));
        let directory_path = launch
            .track_directory
            .then(|| dir.join(format!("{id}.cwd")));
        let capture = Capture::create(output_path.clone(), &self.host.limits)
            .map_err(|error| ToolFault::io("make the output file", &output_path, error))?;
        let inline = self.host.limits.inline_bytes;

        let mut command = self.command(launch, directory_path.as_deref());
        let mut child = command.spawn().map_err(|error| {
            ToolFault::Unavailable(format!(
                "the shell {} could not be started: {error}",
                self.host.shell.path
            ))
        })?;
        let group = Group::of(&child);
        let process = Arc::new(Process {
            id,
            thread_id: launch.thread_id.to_owned(),
            background,
            output_path,
            directory_path,
            stop: CancellationToken::new(),
            changed: Notify::new(),
            inner: Mutex::new(Inner {
                capture,
                combined: HeadTail::new(inline),
                stdout: HeadTail::new(inline),
                stderr: HeadTail::new(inline),
                unread: HeadTail::new(inline),
                exit: None,
                final_directory: None,
            }),
        });
        self.threads()
            .entry(launch.thread_id.to_owned())
            .or_default()
            .processes
            .push(process.clone());

        let mut readers: Vec<JoinHandle<()>> = Vec::new();
        if let Some(out) = child.stdout.take() {
            readers.push(tokio::spawn(pump(out, Pipe::Out, process.clone())));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(tokio::spawn(pump(err, Pipe::Err, process.clone())));
        }
        let supervision = Supervision {
            process: process.clone(),
            group,
            readers,
            // A foreground command is held to a time; a background one only when the call says so.
            deadline: launch
                .timeout
                .or_else(|| (!background).then_some(DEFAULT_TIMEOUT)),
            cancel: cancel.unwrap_or_default(),
            root: self.root.clone(),
            grace: self.host.stop_grace,
        };
        tokio::spawn(supervise(supervision, child));
        Ok(process)
    }

    /// An id that no command of the executor has.
    fn new_id(&self) -> String {
        loop {
            let mut bytes = [0u8; 6];
            rand::fill(&mut bytes[..]);
            let id = format!(
                "cmd_{}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
            let taken = self
                .threads()
                .values()
                .any(|state| state.processes.iter().any(|p| p.id == id));
            if !taken {
                return id;
            }
        }
    }

    /// The program to run: the shell, with the script, in a process group of its own, with its environment
    /// made here and nothing else.
    fn command(&self, launch: &Launch<'_>, directory_file: Option<&Path>) -> Command {
        let shell = &self.host.shell;
        let mut argv: Vec<OsString> = self
            .host
            .process_launcher
            .iter()
            .map(OsString::from)
            .collect();
        argv.push(shell.path.clone().into());
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command.args(shell_arguments(shell, launch.command, directory_file));
        command
            .current_dir(launch.cwd)
            .env_clear()
            .envs(self.baseline.iter().map(|(k, v)| (k, v)))
            .envs(&self.host.variables)
            .env(THREAD_VARIABLE, launch.thread_id)
            .env(ITEM_VARIABLE, launch.call_id)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        #[cfg(unix)]
        command.process_group(0);
        command
    }
}

/// Removes the oldest ended commands of a thread beyond the number kept, with their files.
fn drop_oldest_ended(state: &mut ThreadState) {
    let ended = state.processes.iter().filter(|p| !p.is_running()).count();
    let mut surplus = ended.saturating_sub(KEPT_ENDED_PER_THREAD);
    state.processes.retain(|process| {
        if surplus > 0 && !process.is_running() {
            surplus -= 1;
            remove_files(process);
            false
        } else {
            true
        }
    });
}

/// Removes the files of a command that is not wanted any more.
fn remove_files(process: &Process) {
    let _ = std::fs::remove_file(&process.output_path);
    if let Some(path) = &process.directory_path {
        let _ = std::fs::remove_file(path);
    }
}

/// The arguments that make `shell` run `script`. For a shell that can run a `trap` at exit, a first line
/// writes the directory the script leaves it in to `directory_file`.
fn shell_arguments(shell: &ShellInfo, script: &str, directory_file: Option<&Path>) -> Vec<String> {
    let name = shell_name(shell);
    match name.as_str() {
        "pwsh" | "powershell" => vec![
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-Command".to_owned(),
            script.to_owned(),
        ],
        "cmd" => vec![
            "/D".to_owned(),
            "/S".to_owned(),
            "/C".to_owned(),
            script.to_owned(),
        ],
        name if TRAP_SHELLS.contains(&name) => {
            let text = match directory_file {
                Some(file) => format!(
                    "trap 'pwd -P >| {} 2>/dev/null' EXIT\n{script}",
                    quote_for_posix_shell(&file.display().to_string()).replace('\'', "'\\''")
                ),
                None => script.to_owned(),
            };
            vec!["-c".to_owned(), text]
        }
        _ => vec!["-c".to_owned(), script.to_owned()],
    }
}

/// The shell's name without its directory and extension, in lower case.
fn shell_name(shell: &ShellInfo) -> String {
    Path::new(&shell.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map_or_else(|| shell.name.to_lowercase(), str::to_lowercase)
}

/// `text` as one word for a POSIX shell.
fn quote_for_posix_shell(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Reads one of a command's pipes into its output.
async fn pump<R: AsyncRead + Unpin>(mut reader: R, pipe: Pipe, process: Arc<Process>) {
    let mut buffer = vec![0u8; PUMP_CHUNK];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => process.take_in(pipe, &buffer[..read]),
        }
    }
}

/// Everything the task that watches over a command needs.
struct Supervision {
    process: Arc<Process>,
    group: Option<Group>,
    readers: Vec<JoinHandle<()>>,
    deadline: Option<Duration>,
    cancel: CancellationToken,
    root: CancellationToken,
    grace: Duration,
}

/// Watches over a command until it is over, ends it when it has to be ended, and records how it ended.
async fn supervise(mut work: Supervision, mut child: Child) {
    let timeout = async {
        match work.deadline {
            Some(limit) => tokio::time::sleep(limit).await,
            None => std::future::pending().await,
        }
    };
    let (status, reason) = tokio::select! {
        status = child.wait() => (status, Reason::Exited),
        () = timeout => (end(&mut child, work.group, work.grace).await, Reason::TimedOut),
        () = work.process.stop.cancelled() => (end(&mut child, work.group, work.grace).await, Reason::Stopped),
        () = work.cancel.cancelled() => (end(&mut child, work.group, work.grace).await, Reason::Cancelled),
        () = work.root.cancelled() => (end(&mut child, work.group, work.grace).await, Reason::Shutdown),
    };
    // What the command wrote before it ended is in the pipes; a program it left running that holds them open
    // is not waited for.
    let drain = async {
        for reader in &mut work.readers {
            let _ = reader.await;
        }
    };
    if tokio::time::timeout(DRAIN_GRACE, drain).await.is_err() {
        for reader in &work.readers {
            reader.abort();
        }
    }
    let exit = exit_info(status.as_ref().ok(), reason);
    let directory = work
        .process
        .directory_path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| PathBuf::from(text.trim_end_matches('\n')))
        .filter(|path| path.is_absolute());
    {
        let mut inner = work.process.lock();
        inner.capture.finish();
        inner.final_directory = directory;
        inner.exit = Some(exit);
    }
    work.process.changed.notify_waiters();
}

/// `TERM` to the whole group, then `KILL` to what is left once the grace is over. The leader's status.
async fn end(
    child: &mut Child,
    group: Option<Group>,
    grace: Duration,
) -> std::io::Result<ExitStatus> {
    let until = Instant::now() + grace;
    match group {
        Some(group) => group.term(),
        None => {
            let _ = child.start_kill();
        }
    }
    let status = match tokio::time::timeout_at(until, child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            if let Some(group) = group {
                group.kill();
            }
            let _ = child.start_kill();
            return child.wait().await;
        }
    };
    // The leader is gone; the rest of its group has what is left of the grace to be gone too.
    if let Some(group) = group {
        while group.exists() && Instant::now() < until {
            tokio::time::sleep(GROUP_POLL).await;
        }
        if group.exists() {
            group.kill();
        }
    }
    status
}

/// How a command ended, from its status.
fn exit_info(status: Option<&ExitStatus>, reason: Reason) -> ExitInfo {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.and_then(ExitStatusExt::signal)
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let code = match (status.and_then(ExitStatus::code), signal) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => -1,
    };
    ExitInfo {
        code,
        signal,
        reason,
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    /// A rig with the shell `sh`, in a directory of its own.
    struct Rig {
        dir: tempfile::TempDir,
        commands: Commands,
        root: CancellationToken,
    }

    fn rig_with(limits: OutputLimits, grace: Duration, variables: &[(&str, &str)]) -> Rig {
        let dir = tempfile::tempdir().expect("a directory");
        let root = CancellationToken::new();
        let host = Host {
            shell: ShellInfo {
                name: "sh".to_owned(),
                path: "/bin/sh".to_owned(),
            },
            home: None,
            user: "agent".to_owned(),
            process_launcher: Vec::new(),
            variables: variables
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            outputs_dir: dir.path().join("outputs"),
            limits,
            stop_grace: grace,
        };
        Rig {
            commands: Commands::new(host, root.clone()),
            dir,
            root,
        }
    }

    fn rig() -> Rig {
        rig_with(OutputLimits::default(), Duration::from_millis(400), &[])
    }

    fn launch<'a>(rig: &'a Rig, thread: &'a str, command: &'a str) -> Launch<'a> {
        Launch {
            thread_id: thread,
            call_id: "call-1",
            command,
            cwd: rig.dir.path(),
            timeout: None,
            track_directory: false,
        }
    }

    async fn run(rig: &Rig, thread: &str, command: &str) -> Finished {
        rig.commands
            .run(&launch(rig, thread, command), &CancellationToken::new())
            .await
            .expect("the command runs")
    }

    #[tokio::test]
    async fn a_foreground_command_answers_its_output_and_its_exit_code() {
        let rig = rig();
        let finished = run(&rig, "t1", "echo out; echo err >&2; exit 3").await;
        assert_eq!(finished.exit.code, 3);
        assert_eq!(finished.exit.reason, Reason::Exited);
        assert_eq!(finished.stdout.text, "out\n");
        assert_eq!(finished.stderr.text, "err\n");
        assert!(finished.output_file.is_none(), "short output keeps no file");
    }

    #[tokio::test]
    async fn a_command_runs_inside_the_environments_declared_process_launcher() {
        let mut rig = rig();
        // This stand-in records entry to the resource boundary before running the actual shell.
        Arc::make_mut(&mut rig.commands.host).process_launcher = vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'resource group\\n'; exec \"$@\"".into(),
            "launcher".into(),
        ];
        let finished = run(&rig, "t1", "printf 'agent output\\n'").await;
        assert_eq!(finished.exit.code, 0);
        assert_eq!(finished.stdout.text, "resource group\nagent output\n");
    }

    #[tokio::test]
    async fn every_command_carries_the_thread_and_the_tool_call_it_belongs_to() {
        let rig = rig();
        let mut job = launch(&rig, "thread-77", "echo $ORCH_THREAD_ID/$ORCH_ITEM_ID");
        job.call_id = "toolu_abc";
        let finished = rig
            .commands
            .run(&job, &CancellationToken::new())
            .await
            .expect("runs");
        assert_eq!(finished.stdout.text, "thread-77/toolu_abc\n");
    }

    #[tokio::test]
    async fn the_attribution_variables_cannot_be_overridden_by_the_environments_variables() {
        let rig = rig_with(
            OutputLimits::default(),
            Duration::from_millis(400),
            &[("ORCH_THREAD_ID", "someone else's"), ("PROJECT", "web")],
        );
        let finished = run(&rig, "mine", "echo $ORCH_THREAD_ID $PROJECT").await;
        assert_eq!(finished.stdout.text, "mine web\n");
    }

    #[tokio::test]
    async fn a_command_does_not_see_the_environment_the_executor_runs_in() {
        let rig = rig();
        let finished = run(&rig, "t1", "env | sort").await;
        let names: Vec<&str> = finished
            .stdout
            .text
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        for name in &names {
            assert!(
                [
                    "PATH",
                    "HOME",
                    "USER",
                    "LOGNAME",
                    "SHELL",
                    "TERM",
                    "PWD",
                    "ORCH_THREAD_ID",
                    "ORCH_ITEM_ID",
                    "LANG",
                    "LANGUAGE",
                    "LC_ALL",
                    "LC_CTYPE",
                    "LC_MESSAGES",
                    "TZ",
                    "TMPDIR",
                    "OLDPWD",
                    "_",
                    "SHLVL"
                ]
                .contains(name),
                "{name} is in the command's environment"
            );
        }
        assert!(names.contains(&"ORCH_ITEM_ID") && names.contains(&"TERM"));
    }

    #[tokio::test]
    async fn a_foreground_command_that_outlives_its_time_is_ended_with_term() {
        let rig = rig();
        let mut job = launch(&rig, "t1", "echo started; sleep 30");
        job.timeout = Some(Duration::from_millis(300));
        let began = Instant::now();
        let finished = rig
            .commands
            .run(&job, &CancellationToken::new())
            .await
            .expect("runs");
        assert!(began.elapsed() < Duration::from_secs(5));
        assert_eq!(finished.exit.reason, Reason::TimedOut);
        assert_eq!(finished.exit.signal, Some(15));
        assert_eq!(finished.exit.code, 143);
        assert_eq!(finished.stdout.text, "started\n");
    }

    #[tokio::test]
    async fn a_command_that_ignores_term_is_killed_after_the_grace() {
        let rig = rig_with(OutputLimits::default(), Duration::from_millis(500), &[]);
        let mut job = launch(&rig, "t1", "trap '' TERM; while true; do sleep 0.1; done");
        job.timeout = Some(Duration::from_millis(200));
        let began = Instant::now();
        let finished = rig
            .commands
            .run(&job, &CancellationToken::new())
            .await
            .expect("runs");
        assert_eq!(finished.exit.signal, Some(9), "{:?}", finished.exit);
        assert!(began.elapsed() >= Duration::from_millis(600));
        assert!(began.elapsed() < Duration::from_secs(6));
    }

    #[tokio::test]
    async fn ending_a_command_ends_what_it_started() {
        let rig = rig();
        let marker = rig.dir.path().join("child.pid");
        let script = format!("sleep 60 & echo $! > {}; wait", marker.display());
        let started = rig
            .commands
            .start_background(&launch(&rig, "t1", &script))
            .expect("starts");
        for _ in 0..100 {
            if marker.exists()
                && !std::fs::read_to_string(&marker)
                    .unwrap_or_default()
                    .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let child: i32 = std::fs::read_to_string(&marker)
            .expect("the pid")
            .trim()
            .parse()
            .expect("a number");
        let pid = rustix::process::Pid::from_raw(child).expect("a pid");
        assert!(
            rustix::process::test_kill_process(pid).is_ok(),
            "the child runs"
        );
        let exit = rig
            .commands
            .stop("t1", &started.process_id)
            .await
            .expect("stops");
        assert_eq!(exit.reason, Reason::Stopped);
        for _ in 0..100 {
            if rustix::process::test_kill_process(pid).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the child of the command is still running");
    }

    #[tokio::test]
    async fn a_background_command_gives_its_output_in_pieces_and_its_exit_code_at_the_end() {
        let rig = rig();
        let started = rig
            .commands
            .start_background(&launch(&rig, "t1", "echo one; sleep 0.4; echo two; exit 4"))
            .expect("starts");
        let mut seen = String::new();
        let mut pieces = 0;
        let exit = loop {
            let progress = rig
                .commands
                .output("t1", &started.process_id, Duration::from_secs(5))
                .await
                .expect("output");
            if !progress.output.text.is_empty() {
                pieces += 1;
                seen.push_str(&progress.output.text);
            }
            if let Some(exit) = progress.exit {
                break exit;
            }
        };
        assert_eq!(seen, "one\ntwo\n", "everything comes once, in order");
        assert!(pieces >= 1);
        assert_eq!((exit.code, exit.reason), (4, Reason::Exited));
        let nothing_new = rig
            .commands
            .output("t1", &started.process_id, Duration::ZERO)
            .await
            .expect("output");
        assert!(
            nothing_new.output.text.is_empty(),
            "what was given is not given twice"
        );
        assert_eq!(nothing_new.exit.map(|e| e.code), Some(4));
        let on_disk = std::fs::read_to_string(&started.output_file).expect("the file");
        assert_eq!(on_disk, "one\ntwo\n");
    }

    #[tokio::test]
    async fn a_call_for_output_waits_for_something_to_have_come_and_not_longer() {
        let rig = rig();
        let started = rig
            .commands
            .start_background(&launch(&rig, "t1", "sleep 30"))
            .expect("starts");
        let began = Instant::now();
        let progress = rig
            .commands
            .output("t1", &started.process_id, Duration::from_millis(300))
            .await
            .expect("output");
        assert!(progress.output.text.is_empty() && progress.exit.is_none());
        assert!(began.elapsed() >= Duration::from_millis(280));
        assert!(began.elapsed() < Duration::from_secs(5));
        rig.commands.shutdown().await;
    }

    #[tokio::test]
    async fn a_thread_cannot_see_read_or_stop_the_commands_of_another() {
        let rig = rig();
        let started = rig
            .commands
            .start_background(&launch(&rig, "first-thread", "sleep 30"))
            .expect("starts");
        for thread in ["intruder", "first-threa"] {
            let read = rig
                .commands
                .output(thread, &started.process_id, Duration::ZERO)
                .await
                .expect_err("not theirs");
            assert_eq!(read.code(), "no_such_process");
            let stop = rig
                .commands
                .stop(thread, &started.process_id)
                .await
                .expect_err("not theirs");
            assert_eq!(stop.code(), "no_such_process");
        }
        assert!(
            rig.commands
                .find("first-thread", &started.process_id)
                .expect("theirs")
                .is_running()
        );
        rig.commands.shutdown().await;
    }

    #[tokio::test]
    async fn a_thread_may_have_only_so_many_commands_running_in_the_background() {
        let rig = rig();
        for _ in 0..MAX_BACKGROUND_PER_THREAD {
            rig.commands
                .start_background(&launch(&rig, "t1", "sleep 30"))
                .expect("starts");
        }
        let refused = rig
            .commands
            .start_background(&launch(&rig, "t1", "sleep 30"))
            .expect_err("one too many");
        assert_eq!(refused.code(), "too_many_processes");
        rig.commands
            .start_background(&launch(&rig, "t2", "sleep 30"))
            .expect("another thread has its own");
        rig.commands.shutdown().await;
    }

    #[tokio::test]
    async fn output_beyond_the_inline_bound_is_cut_in_the_middle_and_kept_whole_in_a_file() {
        let limits = OutputLimits {
            inline_bytes: 64,
            kept_bytes: 1024 * 1024,
            tail_bytes: 256 * 1024,
        };
        let rig = rig_with(limits, Duration::from_millis(400), &[]);
        let finished = run(
            &rig,
            "t1",
            "i=0; while [ $i -lt 400 ]; do echo line-$i; i=$((i+1)); done",
        )
        .await;
        assert!(finished.stdout.omitted > 0);
        assert!(finished.stdout.text.starts_with("line-0\n"));
        assert!(finished.stdout.text.trim_end().ends_with("line-399"));
        assert!(finished.stdout.text.contains("bytes omitted"));
        let file = finished.output_file.expect("the output is kept");
        let kept = std::fs::read_to_string(file).expect("the file");
        assert!(kept.starts_with("line-0\n") && kept.ends_with("line-399\n"));
        assert_eq!(
            kept.lines().count(),
            400,
            "nothing is left out of a file within its bound"
        );
    }

    #[tokio::test]
    async fn output_beyond_what_is_kept_loses_its_middle_in_the_file_too() {
        let limits = OutputLimits {
            inline_bytes: 64,
            kept_bytes: 2000,
            tail_bytes: 500,
        };
        let rig = rig_with(limits, Duration::from_millis(400), &[]);
        let finished = run(
            &rig,
            "t1",
            "i=0; while [ $i -lt 2000 ]; do echo line-$i; i=$((i+1)); done",
        )
        .await;
        let kept = std::fs::read_to_string(finished.output_file.expect("kept")).expect("the file");
        assert!(kept.starts_with("line-0\n"), "{}", &kept[..30]);
        assert!(kept.trim_end().ends_with("line-1999"));
        assert!(kept.contains("bytes omitted"));
        assert!(kept.len() < 2000 + 100);
    }

    #[tokio::test]
    async fn the_directory_a_command_leaves_its_shell_in_is_learnt() {
        let rig = rig();
        std::fs::create_dir_all(rig.dir.path().join("sub/inner")).expect("dirs");
        let mut job = launch(&rig, "t1", "cd sub/inner; echo here");
        job.track_directory = true;
        let finished = rig
            .commands
            .run(&job, &CancellationToken::new())
            .await
            .expect("runs");
        let learnt = finished.final_directory.expect("the directory");
        assert_eq!(
            learnt,
            std::fs::canonicalize(rig.dir.path().join("sub/inner")).expect("path")
        );
        assert_eq!(finished.stdout.text, "here\n");
    }

    #[tokio::test]
    async fn the_directory_is_learnt_from_bash_as_well() {
        if !Path::new("/bin/bash").is_file() {
            return;
        }
        let mut rig = rig();
        rig.commands = Commands::new(
            Host {
                shell: ShellInfo {
                    name: "bash".to_owned(),
                    path: "/bin/bash".to_owned(),
                },
                home: None,
                user: "agent".to_owned(),
                process_launcher: Vec::new(),
                variables: BTreeMap::new(),
                outputs_dir: rig.dir.path().join("outputs"),
                limits: OutputLimits::default(),
                stop_grace: Duration::from_millis(400),
            },
            rig.root.clone(),
        );
        std::fs::create_dir_all(rig.dir.path().join("a b/c'd")).expect("dirs");
        let mut job = launch(&rig, "t1", "cd 'a b'/c\\'d; exit 5");
        job.track_directory = true;
        let finished = rig
            .commands
            .run(&job, &CancellationToken::new())
            .await
            .expect("runs");
        assert_eq!(finished.exit.code, 5);
        assert_eq!(
            finished.final_directory,
            Some(std::fs::canonicalize(rig.dir.path().join("a b/c'd")).expect("path")),
            "a directory with a space and a quote in its name is learnt too"
        );
    }

    #[tokio::test]
    async fn the_exit_code_survives_the_trap_that_learns_the_directory() {
        let rig = rig();
        let mut job = launch(&rig, "t1", "exit 7");
        job.track_directory = true;
        let finished = rig
            .commands
            .run(&job, &CancellationToken::new())
            .await
            .expect("runs");
        assert_eq!(finished.exit.code, 7);
        assert!(finished.final_directory.is_some());
    }

    #[tokio::test]
    async fn a_client_that_goes_away_takes_its_foreground_command_with_it() {
        let rig = rig();
        let cancel = CancellationToken::new();
        let waiting = {
            let commands = rig.commands.clone();
            let cancel = cancel.clone();
            let cwd = rig.dir.path().to_path_buf();
            tokio::spawn(async move {
                let job = Launch {
                    thread_id: "t1",
                    call_id: "call-1",
                    command: "echo up; sleep 30",
                    cwd: &cwd,
                    timeout: None,
                    track_directory: false,
                };
                commands.run(&job, &cancel).await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
        let finished = waiting.await.expect("joins").expect("answers");
        assert_eq!(finished.exit.reason, Reason::Cancelled);
        assert_eq!(finished.exit.signal, Some(15));
    }

    #[tokio::test]
    async fn stopping_the_executor_ends_every_command_and_removes_what_they_kept() {
        let rig = rig();
        let started = rig
            .commands
            .start_background(&launch(&rig, "t1", "echo hi; sleep 60"))
            .expect("starts");
        let process = rig.commands.find("t1", &started.process_id).expect("found");
        tokio::time::sleep(Duration::from_millis(200)).await;
        rig.commands.shutdown().await;
        assert_eq!(process.exit().map(|e| e.reason), Some(Reason::Shutdown));
        assert!(!rig.dir.path().join("outputs").exists());
        assert!(rig.root.is_cancelled());
    }

    #[tokio::test]
    async fn a_working_directory_that_is_not_there_is_said_so_and_nothing_runs() {
        let rig = rig();
        let mut job = launch(&rig, "t1", "echo no");
        let missing = rig.dir.path().join("nowhere");
        job.cwd = &missing;
        let refused = rig
            .commands
            .run(&job, &CancellationToken::new())
            .await
            .expect_err("no such directory");
        assert_eq!(refused.code(), "not_found");
    }

    #[tokio::test]
    async fn the_oldest_ended_commands_are_forgotten_beyond_the_number_kept() {
        let rig = rig();
        let mut first = None;
        for n in 0..(KEPT_ENDED_PER_THREAD + 3) {
            let started = rig
                .commands
                .start_background(&launch(&rig, "t1", "echo done"))
                .expect("starts");
            rig.commands
                .find("t1", &started.process_id)
                .expect("found")
                .wait_done()
                .await;
            if n == 0 {
                first = Some(started);
            }
        }
        let first = first.expect("the first");
        assert!(rig.commands.find("t1", &first.process_id).is_err());
        assert!(!first.output_file.exists(), "its file went with it");
    }

    #[test]
    fn a_session_starts_where_it_is_told_and_goes_on_from_where_its_shell_was_left() {
        let rig = rig();
        let commands = &rig.commands;
        assert_eq!(commands.directory("t1", "/w"), PathBuf::from("/w"));
        commands.set_directory("t1", PathBuf::from("/w/api"));
        assert_eq!(commands.directory("t1", "/w"), PathBuf::from("/w/api"));
        assert_eq!(
            commands.directory("t1", "/other"),
            PathBuf::from("/other"),
            "a session told to start elsewhere starts there"
        );
        assert_eq!(commands.directory("t2", "/w"), PathBuf::from("/w"));
    }
}
