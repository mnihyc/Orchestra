//! Interactive shell PTYs that outlive their tunnels: the last of their output kept for whoever takes one up again and for the environment's history when one closes, one attachment at a time, resize, activity, and the report when one ends unattended.
//!
//! A shell is a pseudo-terminal the host owns. It does not depend on anybody looking at it: a
//! thread of the host reads whatever the shell writes whether or not a tunnel is open, so that
//! nothing in the shell ever blocks on output, and keeps the last 256 KiB of it. A person who
//! comes back, from the same browser or another, [`Terminals::attach`]es and is given what was
//! kept to send first, then what comes.
//!
//! **One screen at a time.** Attaching while another attachment holds the shell takes the shell
//! from it: the first is told ([`Event::Taken`]) and can no longer type. The shell itself is not
//! touched. When an attachment is dropped the shell runs on, and it is noted since when nobody has
//! been attached.
//!
//! **Flow control.** What an attachment has not yet taken is counted; past a mebibyte the reading
//! of the shell waits (the shell then waits to write, as it would at a terminal that cannot keep
//! up) until the attachment has taken it down to a quarter of that, or lets go.
//!
//! **Ending.** A shell ends when it exits, or when it is closed, which ends it and everything it
//! started. A shell that exits while nobody is attached is reported, with what it printed last if
//! that is kept; one that exits attached tells its attachment. The same pseudo-terminal serves the
//! agent's own terminal: the host starts the harness in it, and an attachment may be one that can
//! only watch.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use orch_link_contract::commands::BlobRef;
use orch_link_contract::events::PtyExited;
use orch_types::{Clock, Hash, Id};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::blobs::{BlobSink, MemoryReader};
use crate::environment::{Report, Reports};
use crate::error::{HostError, Result};
use crate::processes::{Processes, RootKind};

/// How many bytes one read of a terminal asks for. Sixteen kibibytes is a screenful many times
/// over, so that a program that prints a lot costs few wake-ups, and small enough that the first
/// of a burst reaches a person at once.
pub const READ_CHUNK_BYTES: usize = 16 * 1024;

/// How far behind an attachment may fall before the reading of the shell waits, as the realtime
/// socket holds it: a mebibyte.
pub const HIGH_WATER_BYTES: usize = 1024 * 1024;

/// How far an attachment has to catch up before the reading goes on: a quarter of the high water.
pub const LOW_WATER_BYTES: usize = 256 * 1024;

/// How long the end of a shell waits for the last of its output to be read before it is announced.
const EXIT_DRAIN: Duration = Duration::from_millis(500);

/// How long a closed shell is given to end by itself before what it started is killed.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// How long a closed shell is given when the environment has no table of processes to find what it
/// started in: only its own group can be reached, and it is not worth waiting for.
const CLOSE_GRACE_GROUP_ONLY: Duration = Duration::from_millis(300);

/// How long a close waits between looks at whether the shell has ended.
const ENDED_POLL: Duration = Duration::from_millis(25);

/// How long the reading thread sleeps between looks when an attachment is behind.
const BACKPRESSURE_POLL: Duration = Duration::from_millis(50);

/// The widest or tallest a terminal may be made; larger is a mistake or an attack on memory.
const TERMINAL_SIZE_MAX: u16 = 1000;

/// What a terminal is told it is, where nothing else says.
const DEFAULT_TERM: &str = "xterm-256color";

/// The name of the file in which the shells that are open are written down, so that a connector
/// that was restarted can report the ones it no longer has.
const REGISTRY_FILE: &str = "shells.json";

/// The variables of the connector itself that a shell does not inherit: where its identity and its
/// enrollment are kept is not for the people and programs a shell runs.
pub(crate) const CONNECTOR_PRIVATE_VARIABLES: [&str; 5] = [
    "ORCH_ENROLL_FILE",
    "ORCH_CA_FILE",
    "ORCH_GATEWAY_URL",
    "ORCH_DATA_DIR",
    "ORCH_ROLES",
];

/// What a pseudo-terminal runs for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A person's shell.
    Shell,
    /// The harness of an agent that can only be driven through its terminal.
    Agent,
}

/// What to start in a new pseudo-terminal.
#[derive(Debug, Clone)]
pub struct OpenSpec {
    /// The identifier it is known by from now on.
    pub id: Id,
    /// The environment it belongs to.
    pub environment_id: Id,
    /// What it is for.
    pub purpose: Purpose,
    /// The program: a shell, or a harness.
    pub program: PathBuf,
    /// Its arguments, after the ones a login shell gets.
    pub args: Vec<String>,
    /// Whether the program is started as a login shell.
    pub login: bool,
    /// The directory it starts in, which has to exist.
    pub cwd: PathBuf,
    /// The variables it is given besides those of the connector's own environment.
    pub env: BTreeMap<String, String>,
    /// The width of its screen.
    pub cols: u16,
    /// The height of its screen.
    pub rows: u16,
    /// Whether the last of what it prints is kept when it ends.
    pub record: bool,
}

/// What came of opening one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The program that runs.
    pub shell: String,
    /// The number of the process.
    pub pid: Option<u32>,
}

/// What happens at an attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Bytes the shell wrote.
    Data(Bytes),
    /// The shell ended, with the code it exited with where it has one.
    Exit {
        /// The exit code; `None` for a shell that was ended by a signal.
        code: Option<i64>,
    },
    /// Another attachment took the shell. This one has ended.
    Taken,
}

/// What a shell was when it was closed or looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    /// What it exited with; `None` for a shell ended by a signal.
    pub exit_code: Option<i64>,
    /// How many bytes it printed in all.
    pub bytes_out: i64,
    /// What was kept of its output, where that was asked for.
    pub kept: Option<BlobRef>,
}

/// A shell as the host lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalInfo {
    /// Its identifier.
    pub id: Id,
    /// The environment it belongs to.
    pub environment_id: Id,
    /// What it is for.
    pub purpose: Purpose,
    /// The program that runs.
    pub shell: String,
    /// Its width.
    pub cols: u16,
    /// Its height.
    pub rows: u16,
    /// Whether an attachment holds it now.
    pub attached: bool,
    /// Since when nobody has been attached; `None` while somebody is.
    pub detached_since: Option<DateTime<Utc>>,
    /// Whether a command is running in it, and not just a prompt waiting.
    pub busy: bool,
    /// When something last went in or came out.
    pub last_io_at: DateTime<Utc>,
    /// When it was opened.
    pub opened_at: DateTime<Utc>,
    /// Whether it has ended.
    pub exited: bool,
    /// How many bytes it printed in all.
    pub bytes_out: i64,
}

/// What the host's shells add up to, for deciding whether an environment may be stopped for idleness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    /// How many shells are open.
    pub open: usize,
    /// How many of them have a command running.
    pub busy: usize,
    /// When anything last went in or out of any.
    pub last_io_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct Slot {
    id: u64,
    tx: mpsc::UnboundedSender<Event>,
    queued: Arc<AtomicUsize>,
}

#[derive(Debug)]
struct Shared {
    ring: VecDeque<u8>,
    keep: usize,
    bytes_out: u64,
    slot: Option<Slot>,
    next_attachment: u64,
    exit: Option<Option<i64>>,
    kept: Option<BlobRef>,
    kept_done: bool,
    last_io: DateTime<Utc>,
    detached_since: Option<DateTime<Utc>>,
    reader_done: bool,
    size: (u16, u16),
}

struct Session {
    id: Id,
    environment_id: Id,
    purpose: Purpose,
    shell: String,
    record: bool,
    pid: Option<u32>,
    opened_at: DateTime<Utc>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    shared: Mutex<Shared>,
    changed: Condvar,
    closing: AtomicBool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("shell", &self.shell)
            .finish_non_exhaustive()
    }
}

impl Session {
    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn busy(&self) -> bool {
        let Some(pid) = self.pid else { return false };
        if self.shared().exit.is_some() {
            return false;
        }
        self.command_is_running(pid)
    }

    /// The shell leads the group in the foreground at its prompt; a command takes the foreground.
    #[cfg(unix)]
    fn command_is_running(&self, pid: u32) -> bool {
        let master = self.master.lock().unwrap_or_else(PoisonError::into_inner);
        master
            .process_group_leader()
            .is_some_and(|leader| u32::try_from(leader).ok() != Some(pid))
    }

    /// A pseudo-console has no foreground group to ask: a shell is busy while it has a child.
    #[cfg(not(unix))]
    fn command_is_running(&self, pid: u32) -> bool {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let shell = Pid::from_u32(pid);
        system
            .processes()
            .values()
            .any(|process| process.parent() == Some(shell))
    }
}

/// The attachment of one screen to a shell.
#[derive(Debug)]
pub struct Attachment {
    session: Arc<Session>,
    id: u64,
    rx: mpsc::UnboundedReceiver<Event>,
    queued: Arc<AtomicUsize>,
    replay: Bytes,
    control: bool,
    clock: Arc<dyn Clock>,
}

impl Attachment {
    /// What the shell printed last, which is sent first.
    #[must_use]
    pub fn replay(&self) -> &Bytes {
        &self.replay
    }

    /// The size of the screen as it stands.
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        self.session.shared().size
    }

    /// The next thing that happens, or `None` when the attachment is over.
    pub async fn next(&mut self) -> Option<Event> {
        let event = self.rx.recv().await?;
        if let Event::Data(bytes) = &event {
            self.queued.fetch_sub(bytes.len(), Ordering::SeqCst);
            self.session.changed.notify_all();
        }
        Some(event)
    }

    fn require_current(&self) -> Result<()> {
        let shared = self.session.shared();
        if shared.slot.as_ref().is_some_and(|slot| slot.id == self.id) {
            Ok(())
        } else {
            Err(HostError::Busy(
                "another screen has taken this shell".to_owned(),
            ))
        }
    }

    /// Types `bytes` into the shell. Refused when the attachment only watches or has been taken.
    pub fn send_input(&self, bytes: Vec<u8>) -> Result<()> {
        if !self.control {
            return Err(HostError::Unsupported(
                "this attachment only watches".to_owned(),
            ));
        }
        self.require_current()?;
        self.session.shared().last_io = self.clock.now();
        let input = self
            .session
            .input
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match input.as_ref() {
            Some(sender) => sender
                .send(bytes)
                .map_err(|_| HostError::NotFound("the shell has ended".to_owned())),
            None => Err(HostError::NotFound("the shell has ended".to_owned())),
        }
    }

    /// Changes the size of the screen.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.require_current()?;
        if cols == 0 || rows == 0 || cols > TERMINAL_SIZE_MAX || rows > TERMINAL_SIZE_MAX {
            return Err(HostError::Unsupported(format!(
                "a screen is between 1 and {TERMINAL_SIZE_MAX} characters each way"
            )));
        }
        self.session
            .master
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| {
                HostError::Internal(format!("the screen could not be resized: {error}"))
            })?;
        self.session.shared().size = (cols, rows);
        Ok(())
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        let mut shared = self.session.shared();
        if shared.slot.as_ref().is_some_and(|slot| slot.id == self.id) {
            shared.slot = None;
            shared.detached_since = Some(self.clock.now());
        }
        drop(shared);
        self.session.changed.notify_all();
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Registry {
    open: Vec<Id>,
}

/// The pseudo-terminals of one connector.
pub struct Terminals {
    sessions: Mutex<HashMap<Id, Arc<Session>>>,
    clock: Arc<dyn Clock>,
    keep_bytes: usize,
    processes: Option<Processes>,
    reports: Option<Reports>,
    blobs: Option<Arc<dyn BlobSink>>,
    registry: Option<PathBuf>,
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for Terminals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminals")
            .field("open", &self.sessions.lock().map(|s| s.len()).unwrap_or(0))
            .finish_non_exhaustive()
    }
}

/// What a set of terminals is wired to.
#[derive(Default)]
pub struct TerminalLinks {
    /// The processes of the environment, which a shell is noted among.
    pub processes: Option<Processes>,
    /// Where a shell that ends unattended is reported.
    pub reports: Option<Reports>,
    /// Where what a shell printed last is stored when it ends.
    pub blobs: Option<Arc<dyn BlobSink>>,
    /// A directory of the connector's own, in which the open shells are written down.
    pub data_dir: Option<PathBuf>,
}

fn io_err(context: &str, error: impl std::fmt::Display) -> HostError {
    HostError::Internal(format!("{context}: {error}"))
}

impl Terminals {
    /// The shells of a connector, keeping `keep_bytes` of what each prints. Must be made inside a
    /// runtime, which is the one the reports of unattended shells are sent on.
    pub fn new(clock: Arc<dyn Clock>, keep_bytes: usize, links: TerminalLinks) -> Result<Self> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| HostError::Internal("shells are made inside a runtime".to_owned()))?;
        Ok(Self {
            sessions: Mutex::new(HashMap::new()),
            clock,
            keep_bytes,
            processes: links.processes,
            reports: links.reports,
            blobs: links.blobs,
            registry: links.data_dir.map(|dir| dir.join(REGISTRY_FILE)),
            runtime,
        })
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<Id, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The shells that were open when the connector last stopped and are therefore gone: a
    /// connector that was restarted reports each as ended with no exit code. The record is cleared.
    pub fn recover(data_dir: &Path) -> Vec<Id> {
        let path = data_dir.join(REGISTRY_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let lost = serde_json::from_str::<Registry>(&text)
            .map(|registry| registry.open)
            .unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        lost
    }

    fn write_registry(&self) {
        let Some(path) = &self.registry else { return };
        let open: Vec<Id> = self.sessions().keys().copied().collect();
        let Ok(text) = serde_json::to_string(&Registry { open }) else {
            return;
        };
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, text).is_ok() {
            let _ = std::fs::rename(&temp, path);
        }
    }

    /// Starts a pseudo-terminal with the program of `spec` in it.
    pub fn open(&self, spec: OpenSpec) -> Result<Opened> {
        if spec.cols == 0
            || spec.rows == 0
            || spec.cols > TERMINAL_SIZE_MAX
            || spec.rows > TERMINAL_SIZE_MAX
        {
            return Err(HostError::Unsupported(format!(
                "a screen is between 1 and {TERMINAL_SIZE_MAX} characters each way"
            )));
        }
        if self.sessions().contains_key(&spec.id) {
            return Err(HostError::Exists(format!(
                "the terminal {} is open already",
                spec.id
            )));
        }
        if !spec.cwd.is_dir() {
            return Err(HostError::NotFound(format!(
                "{} is not a directory",
                spec.cwd.display()
            )));
        }
        let size = PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| io_err("a pseudo-terminal could not be made", e))?;
        let launcher = self
            .processes
            .as_ref()
            .map_or_else(Vec::new, Processes::launcher);
        let mut command = match launcher.split_first() {
            Some((wrapper, args)) => {
                let mut command = CommandBuilder::new(wrapper);
                for arg in args {
                    command.arg(arg);
                }
                command.arg(&spec.program);
                command
            }
            None => CommandBuilder::new(&spec.program),
        };
        command.env_clear();
        for (key, value) in std::env::vars_os() {
            if key
                .to_str()
                .is_some_and(|name| CONNECTOR_PRIVATE_VARIABLES.contains(&name))
            {
                continue;
            }
            command.env(key, value);
        }
        command.env("TERM", DEFAULT_TERM);
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command.env("ORCH_TERMINAL_ID", spec.id.to_string());
        if spec.login {
            command.arg("-l");
        }
        for argument in &spec.args {
            command.arg(argument);
        }
        command.cwd(&spec.cwd);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| io_err("the program could not be started", e))?;
        drop(pair.slave);
        let pid = child.process_id();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io_err("the terminal could not be read", e))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io_err("the terminal could not be written", e))?;
        let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let now = self.clock.now();
        let shell = spec.program.to_string_lossy().into_owned();
        let session = Arc::new(Session {
            id: spec.id,
            environment_id: spec.environment_id,
            purpose: spec.purpose,
            shell: shell.clone(),
            record: spec.record,
            pid,
            opened_at: now,
            master: Mutex::new(pair.master),
            input: Mutex::new(Some(input_tx)),
            shared: Mutex::new(Shared {
                ring: VecDeque::new(),
                keep: self.keep_bytes,
                bytes_out: 0,
                slot: None,
                next_attachment: 0,
                exit: None,
                kept: None,
                kept_done: false,
                last_io: now,
                detached_since: Some(now),
                reader_done: false,
                size: (spec.cols, spec.rows),
            }),
            changed: Condvar::new(),
            closing: AtomicBool::new(false),
        });
        if let (Some(processes), Some(pid)) = (&self.processes, pid) {
            let kind = if spec.purpose == Purpose::Shell {
                RootKind::Shell(spec.id)
            } else {
                RootKind::Harness
            };
            if let Err(error) = processes.register_root(pid, kind) {
                tracing::warn!(%error, "a shell could not be noted among the environment's processes");
            }
        }
        let clock = Arc::clone(&self.clock);
        {
            let session = Arc::clone(&session);
            let clock = Arc::clone(&clock);
            std::thread::Builder::new()
                .name("shell-read".to_owned())
                .spawn(move || read_loop(&session, reader, &*clock))
                .map_err(|e| io_err("a thread could not be started", e))?;
        }
        {
            let session = Arc::clone(&session);
            std::thread::Builder::new()
                .name("shell-write".to_owned())
                .spawn(move || write_loop(&session, writer, input_rx))
                .map_err(|e| io_err("a thread could not be started", e))?;
        }
        {
            let session = Arc::clone(&session);
            let reports = self.reports.clone();
            let blobs = self.blobs.clone();
            let runtime = self.runtime.clone();
            std::thread::Builder::new()
                .name("shell-wait".to_owned())
                .spawn(move || wait_loop(&session, child, reports, blobs, &runtime))
                .map_err(|e| io_err("a thread could not be started", e))?;
        }
        self.sessions().insert(spec.id, session);
        self.write_registry();
        Ok(Opened { shell, pid })
    }

    fn session(&self, id: Id) -> Result<Arc<Session>> {
        self.sessions()
            .get(&id)
            .cloned()
            .ok_or_else(|| HostError::NotFound(format!("there is no terminal {id}")))
    }

    /// Takes the shell up: the attachment is given what the shell printed last to send first, and
    /// then what it prints. An attachment that holds the shell already is told it was taken.
    /// With `control` false the attachment can only watch.
    pub fn attach(&self, id: Id, control: bool) -> Result<Attachment> {
        let session = self.session(id)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let mut shared = session.shared();
        shared.next_attachment += 1;
        let attachment = shared.next_attachment;
        if let Some(old) = shared.slot.take() {
            let _ = old.tx.send(Event::Taken);
        }
        let replay: Vec<u8> = shared.ring.iter().copied().collect();
        if let Some(code) = shared.exit {
            // A shell that has ended is attached to only to be shown how it ended.
            let _ = tx.send(Event::Exit { code });
        } else {
            shared.slot = Some(Slot {
                id: attachment,
                tx,
                queued: Arc::clone(&queued),
            });
        }
        shared.detached_since = None;
        drop(shared);
        session.changed.notify_all();
        Ok(Attachment {
            session,
            id: attachment,
            rx,
            queued,
            replay: Bytes::from(replay),
            control,
            clock: Arc::clone(&self.clock),
        })
    }

    /// What is open.
    #[must_use]
    pub fn list(&self) -> Vec<TerminalInfo> {
        let sessions: Vec<Arc<Session>> = self.sessions().values().cloned().collect();
        let mut out: Vec<TerminalInfo> = sessions
            .iter()
            .map(|session| {
                let busy = session.busy();
                let shared = session.shared();
                TerminalInfo {
                    id: session.id,
                    environment_id: session.environment_id,
                    purpose: session.purpose,
                    shell: session.shell.clone(),
                    cols: shared.size.0,
                    rows: shared.size.1,
                    attached: shared.slot.is_some(),
                    detached_since: shared.detached_since,
                    busy,
                    last_io_at: shared.last_io,
                    opened_at: session.opened_at,
                    exited: shared.exit.is_some(),
                    bytes_out: i64::try_from(shared.bytes_out).unwrap_or(i64::MAX),
                }
            })
            .collect();
        out.sort_by_key(|info| info.opened_at);
        out
    }

    /// What the shells add up to.
    #[must_use]
    pub fn activity(&self) -> Activity {
        let listed = self.list();
        Activity {
            open: listed.len(),
            busy: listed.iter().filter(|info| info.busy).count(),
            last_io_at: listed.iter().map(|info| info.last_io_at).max(),
        }
    }

    /// Ends the shell `id` and everything it started, and says how it ended and what was kept. A
    /// shell that has ended already is only removed.
    pub async fn close(&self, id: Id) -> Result<Closed> {
        let session = self.session(id)?;
        session.closing.store(true, Ordering::SeqCst);
        let ended = session.shared().exit.is_some();
        if !ended {
            if let (Some(processes), Some(pid)) = (&self.processes, session.pid) {
                let _ = processes.stop_tree(pid, CLOSE_GRACE).await;
            } else if let Some(pid) = session.pid {
                // Without the environment's table the shell's own group is what can be reached.
                let _ = crate::platform::send_group(pid, crate::platform::Sig::Term);
                tokio::time::sleep(CLOSE_GRACE_GROUP_ONLY).await;
                let _ = crate::platform::send_group(pid, crate::platform::Sig::Kill);
            }
            self.wait_ended(&session).await;
        }
        let (exit_code, bytes_out, mut kept, kept_done, ring) = {
            let shared = session.shared();
            (
                shared.exit.flatten(),
                shared.bytes_out,
                shared.kept,
                shared.kept_done,
                shared.ring.iter().copied().collect::<Vec<u8>>(),
            )
        };
        if session.record && !kept_done {
            kept = self.keep(&ring).await;
        }
        self.sessions().remove(&id);
        self.write_registry();
        if let (Some(processes), Some(pid)) = (&self.processes, session.pid) {
            processes.forget_root(pid);
        }
        *session.input.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(Closed {
            exit_code,
            bytes_out: i64::try_from(bytes_out).unwrap_or(i64::MAX),
            kept,
        })
    }

    async fn wait_ended(&self, session: &Arc<Session>) {
        for _ in 0..200 {
            if session.shared().exit.is_some() {
                return;
            }
            tokio::time::sleep(ENDED_POLL).await;
        }
    }

    async fn keep(&self, bytes: &[u8]) -> Option<BlobRef> {
        let sink = self.blobs.as_ref()?;
        keep_output(sink.as_ref(), bytes).await
    }

    /// Closes every shell, as when the environment stops.
    pub async fn close_all(&self) -> Vec<(Id, Closed)> {
        let ids: Vec<Id> = self.sessions().keys().copied().collect();
        let mut closed = Vec::new();
        for id in ids {
            if let Ok(result) = self.close(id).await {
                closed.push((id, result));
            }
        }
        closed
    }
}

/// Stores `bytes` as a blob and names it; `None` when there is no store or it refuses.
async fn keep_output(sink: &dyn BlobSink, bytes: &[u8]) -> Option<BlobRef> {
    let hash = Hash::of(bytes);
    let size = i64::try_from(bytes.len()).ok()?;
    if sink.missing(&[hash]).await.ok()?.is_empty() {
        return Some(BlobRef { hash, size });
    }
    let mut reader = MemoryReader::new(bytes.to_vec());
    sink.put(hash, size, &mut reader).await.ok()?;
    Some(BlobRef { hash, size })
}

fn read_loop(session: &Arc<Session>, mut reader: Box<dyn Read + Send>, clock: &dyn Clock) {
    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let chunk = Bytes::copy_from_slice(&buffer[..count]);
        let delivery = {
            let mut shared = session.shared();
            shared.bytes_out += count as u64;
            shared.ring.extend(chunk.iter().copied());
            let excess = shared.ring.len().saturating_sub(shared.keep);
            if excess > 0 {
                shared.ring.drain(..excess);
            }
            shared.last_io = clock.now();
            shared
                .slot
                .as_ref()
                .map(|slot| (slot.id, slot.tx.clone(), Arc::clone(&slot.queued)))
        };
        if let Some((id, tx, queued)) = delivery {
            queued.fetch_add(count, Ordering::SeqCst);
            if tx.send(Event::Data(chunk)).is_err() {
                let mut shared = session.shared();
                if shared.slot.as_ref().is_some_and(|slot| slot.id == id) {
                    shared.slot = None;
                    shared.detached_since = Some(clock.now());
                }
                continue;
            }
            wait_for_room(session, id, &queued);
        }
    }
    session.shared().reader_done = true;
    session.changed.notify_all();
}

/// Waits while the attachment `id` is more than the high water behind, until it is down to the low
/// water, or lets go, or is taken, or the shell is closed.
fn wait_for_room(session: &Arc<Session>, id: u64, queued: &AtomicUsize) {
    if queued.load(Ordering::SeqCst) <= HIGH_WATER_BYTES {
        return;
    }
    let mut shared = session.shared();
    while queued.load(Ordering::SeqCst) > LOW_WATER_BYTES
        && shared.slot.as_ref().is_some_and(|slot| slot.id == id)
        && !session.closing.load(Ordering::SeqCst)
    {
        shared = session
            .changed
            .wait_timeout(shared, BACKPRESSURE_POLL)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

fn write_loop(
    session: &Arc<Session>,
    mut writer: Box<dyn Write + Send>,
    input: std::sync::mpsc::Receiver<Vec<u8>>,
) {
    while let Ok(bytes) = input.recv() {
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            break;
        }
    }
    // Closing the writer tells the program its input has ended.
    drop(writer);
    drop(session.input.lock().map(|mut input| input.take()));
}

fn wait_loop(
    session: &Arc<Session>,
    mut child: Box<dyn Child + Send + Sync>,
    reports: Option<Reports>,
    blobs: Option<Arc<dyn BlobSink>>,
    runtime: &tokio::runtime::Handle,
) {
    let code = match child.wait() {
        Ok(status) if status.signal().is_none() => Some(i64::from(status.exit_code())),
        _ => None,
    };
    // The last of what it printed is read before the end is announced.
    {
        let mut shared = session.shared();
        let mut waited = Duration::ZERO;
        while !shared.reader_done && waited < EXIT_DRAIN {
            let (guard, _) = session
                .changed
                .wait_timeout(shared, BACKPRESSURE_POLL)
                .unwrap_or_else(PoisonError::into_inner);
            shared = guard;
            waited += BACKPRESSURE_POLL;
        }
    }
    let (attached, bytes_out, ring) = {
        let mut shared = session.shared();
        shared.exit = Some(code);
        let attached = shared.slot.take();
        if let Some(slot) = &attached {
            let _ = slot.tx.send(Event::Exit { code });
        }
        (
            attached.is_some(),
            shared.bytes_out,
            shared.ring.iter().copied().collect::<Vec<u8>>(),
        )
    };
    session.changed.notify_all();
    // Closing the input ends the writing thread, if the program did not already.
    *session.input.lock().unwrap_or_else(PoisonError::into_inner) = None;
    if attached || session.closing.load(Ordering::SeqCst) {
        return;
    }
    // Nobody is attached to tell: the environment's stream is.
    let record = session.record;
    let environment_id = session.environment_id;
    let terminal_id = session.id;
    let session = Arc::clone(session);
    runtime.spawn(async move {
        let kept = match (&blobs, record) {
            (Some(sink), true) => keep_output(sink.as_ref(), &ring).await,
            _ => None,
        };
        {
            let mut shared = session.shared();
            shared.kept = kept;
            shared.kept_done = true;
        }
        if let Some(reports) = reports {
            let exited = PtyExited {
                terminal_id,
                exit_code: code,
                bytes_out: i64::try_from(bytes_out).unwrap_or(i64::MAX),
                kept,
            };
            reports.say(environment_id, Report::PtyExited(exited)).await;
        }
    });
}

/// The shell a person gets in an environment: the one the environment's user has, or the first of
/// the usual ones that exists. Returns its name and its path.
#[must_use]
pub fn find_shell(preferred: Option<&str>) -> Option<(String, PathBuf)> {
    #[cfg(unix)]
    let candidates: Vec<PathBuf> = {
        let mut list: Vec<PathBuf> = Vec::new();
        if let Some(path) = preferred
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("SHELL").map(PathBuf::from))
        {
            list.push(path);
        }
        list.extend(
            [
                "/bin/bash",
                "/usr/bin/bash",
                "/bin/zsh",
                "/usr/bin/zsh",
                "/bin/sh",
            ]
            .map(PathBuf::from),
        );
        list
    };
    #[cfg(windows)]
    let candidates: Vec<PathBuf> = {
        let mut list: Vec<PathBuf> = Vec::new();
        if let Some(path) = preferred.map(PathBuf::from) {
            list.push(path);
        }
        if let Some(comspec) = std::env::var_os("COMSPEC") {
            list.push(PathBuf::from(comspec));
        }
        list.push(PathBuf::from("powershell.exe"));
        list
    };
    candidates
        .into_iter()
        .find(|path| {
            path.is_absolute() && path.is_file() || cfg!(windows) && path.extension().is_some()
        })
        .map(|path| {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, path)
        })
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_is_found_on_a_machine_that_has_one() {
        let (name, path) = find_shell(None).unwrap();
        assert!(path.is_absolute() && path.is_file(), "{path:?}");
        assert!(!name.is_empty());
        assert_eq!(
            find_shell(Some("/bin/sh")).unwrap().1,
            PathBuf::from("/bin/sh")
        );
        let fallback = find_shell(Some("/no/such/shell")).unwrap();
        assert!(
            fallback.1.is_file(),
            "a shell that is not there gives way to one that is"
        );
    }
}
