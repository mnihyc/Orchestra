//! Scoped process discovery, attribution by thread, safe signaling, and suspending and resuming around a move.
//!
//! Which processes of the machine belong to an environment depends on what it is. In a unit that
//! a driver made for it, **every** process is the environment's except the connector and the
//! executors and MCP hosts the connector itself runs (their descendants are listed). On a person's
//! machine, and where the driver cannot say, only the processes **started for the environment** are
//! (by an executor, a harness or a shell) and everything that descends from them, followed by
//! parentage, by process group and by session, and remembered once seen so that a process whose
//! parent has gone is not lost.
//!
//! Each process carries the thread and the tool call it was started for when the executor that
//! started it said which environment variables hold them; a process without them takes those of
//! its nearest ancestor that has them.
//!
//! **A signal reaches only the process it names.** It names it by number and by the moment it
//! started, the number is never 0, 1, the host's own or one outside the scope, the start is
//! checked in the same step as the signal where the system has process handles, and a tree is
//! treated whole by holding every member still while the members are found, so that nothing can
//! escape by forking meanwhile. A signal to a group goes only to groups of processes the
//! environment started. Everything else is `not_found`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::DateTime;
use orch_api_contract::processes::ProcessInfo;
use orch_executor_contract::descriptor::Attribution;
use orch_link_contract::commands::Signal;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};
use sysinfo::{
    Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind, Users,
};

use crate::error::{HostError, Result};
use crate::platform::{self, Sig};

/// How many rounds a tree is searched for members it has not frozen yet before the signal is sent
/// to those found. A tree that forks faster than this is being fed by a program written to escape,
/// and the signal then goes to what is held.
const FREEZE_ROUNDS: usize = 32;

/// How many ancestors are followed to find the ids of a process that has none of its own: a chain
/// longer than that is a loop in a table that changed while it was read.
const ANCESTOR_STEPS_MAX: usize = 64;

/// How many times a tree is killed again for processes that appeared after the last kill: each
/// round finds what the previous one could not have seen.
const KILL_ROUNDS: usize = 8;

/// How long the end of a tree is waited for after it was killed.
const KILL_WAIT: Duration = Duration::from_secs(5);

/// How often the end of a tree is looked at while it is waited for.
const POLL: Duration = Duration::from_millis(25);

/// What a process the environment started was started as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootKind {
    /// A command of an executor.
    Executor,
    /// A harness.
    Harness,
    /// An MCP host, which runs on its own account.
    Server,
    /// A person's shell, which belongs to the shell session.
    Shell(Id),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Root {
    start_ms: i64,
    kind: RootKind,
}

/// Which processes are the environment's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeKind {
    /// Every process of the unit but the protected ones.
    All,
    /// Only the processes started for the environment, and what descends from them.
    Started,
}

/// A process as the host lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    /// What the list shows of it.
    pub info: ProcessInfo,
    /// The moment it started, in milliseconds since 1970, which with its number names it.
    pub start_ms: i64,
    /// The process group it is in, where the system has them.
    pub group: Option<u32>,
    /// The shell session it belongs to, when it descends from a person's shell.
    pub shell: Option<Id>,
}

/// One process as the system's table gives it: what finding a tree needs.
#[derive(Debug, Clone, Copy)]
struct Row {
    pid: u32,
    ppid: Option<u32>,
    group: Option<u32>,
    session: Option<u32>,
    start_ms: i64,
    zombie: bool,
    state: char,
}

impl From<platform::ProcRow> for Row {
    fn from(row: platform::ProcRow) -> Self {
        Self {
            pid: row.pid,
            ppid: Some(row.ppid).filter(|p| *p > 0),
            group: Some(row.group).filter(|g| *g > 0),
            session: Some(row.session).filter(|s| *s > 0),
            start_ms: row.start_ms,
            zombie: matches!(row.state, 'Z' | 'X'),
            state: row.state,
        }
    }
}

/// What is known of a process that is read only for those that are listed.
#[derive(Debug, Clone, Default)]
struct Detail {
    name: String,
    command: String,
    user: String,
    cpu: f64,
    rss: u64,
    thread: Option<Id>,
    item: Option<Id>,
}

#[derive(Debug)]
struct State {
    system: System,
    users: Users,
    roots: BTreeMap<u32, Root>,
    seen: BTreeMap<u32, i64>,
    protected: BTreeMap<u32, i64>,
    environments: HashMap<(u32, i64), BTreeMap<String, String>>,
    held: Vec<Held>,
}

/// What `suspend` held still, so that `resume` lets go of exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Held {
    pid: u32,
    start_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct SavedProcesses {
    version: u32,
    #[serde(default)]
    launcher: Vec<String>,
    scope: ScopeKind,
    roots: BTreeMap<u32, Root>,
    seen: BTreeMap<u32, i64>,
    held: Vec<Held>,
}

/// The processes of one environment.
#[derive(Debug, Clone)]
pub struct Processes {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    launcher: Mutex<Vec<String>>,
    scope: ScopeKind,
    clock: Arc<dyn Clock>,
    attributions: Mutex<Vec<Attribution>>,
    state: Mutex<State>,
    store: Mutex<Option<PathBuf>>,
    suspension: Mutex<()>,
}

fn state_letter(status: ProcessStatus) -> (char, bool) {
    match status {
        ProcessStatus::Run => ('R', false),
        ProcessStatus::Sleep => ('S', false),
        ProcessStatus::Idle => ('I', false),
        ProcessStatus::Stop | ProcessStatus::Tracing => ('T', false),
        ProcessStatus::Zombie => ('Z', true),
        ProcessStatus::Dead => ('X', true),
        ProcessStatus::UninterruptibleDiskSleep => ('D', false),
        _ => ('?', false),
    }
}

fn time_of(ms: i64) -> Time {
    Time::new(DateTime::from_timestamp_millis(ms).unwrap_or_default())
}

impl Processes {
    /// The processes of an environment with the given scope.
    #[must_use]
    pub fn new(scope: ScopeKind, clock: Arc<dyn Clock>) -> Self {
        let state = State {
            system: System::new(),
            users: Users::new_with_refreshed_list(),
            roots: BTreeMap::new(),
            seen: BTreeMap::new(),
            protected: BTreeMap::new(),
            environments: HashMap::new(),
            held: Vec::new(),
        };
        Self {
            inner: Arc::new(Inner {
                launcher: Mutex::new(Vec::new()),
                scope,
                clock,
                attributions: Mutex::new(Vec::new()),
                state: Mutex::new(state),
                store: Mutex::new(None),
                suspension: Mutex::new(()),
            }),
        }
    }

    /// Restores ownership and exact suspended identities from this environment's private journal.
    pub fn with_store(scope: ScopeKind, clock: Arc<dyn Clock>, path: PathBuf) -> Result<Self> {
        let this = Self::new(scope, clock);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let saved: SavedProcesses = serde_json::from_slice(&bytes).map_err(|error| {
                    HostError::Internal(format!("reading the suspended-process journal: {error}"))
                })?;
                if saved.version != 1 || saved.scope != scope {
                    return Err(HostError::Internal(
                        "the suspended-process journal has a different version or scope".to_owned(),
                    ));
                }
                let mut state = this.state();
                state.roots = saved.roots;
                state.seen = saved.seen;
                state.held = saved.held;
                *this
                    .inner
                    .launcher
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = saved.launcher;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(HostError::io(
                    "reading the suspended-process journal",
                    error,
                ));
            }
        }
        *this
            .inner
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path);
        this.persist()?;
        Ok(this)
    }

    fn persist(&self) -> Result<()> {
        let store = self
            .inner
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(path) = store.as_ref() else {
            return Ok(());
        };
        let state = self.state();
        let bytes = serde_json::to_vec(&SavedProcesses {
            version: 1,
            launcher: self.launcher(),
            scope: self.inner.scope,
            roots: state.roots.clone(),
            seen: state.seen.clone(),
            held: state.held.clone(),
        })
        .map_err(|error| {
            HostError::Internal(format!("writing the suspended-process journal: {error}"))
        })?;
        drop(state);
        let parent = path
            .parent()
            .ok_or_else(|| HostError::Internal("the process journal has no parent".to_owned()))?;
        std::fs::create_dir_all(parent).map_err(|error| {
            HostError::io("making the private process journal directory", error)
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(
                |error| HostError::io("protecting the process journal directory", error),
            )?;
        }
        let temporary = parent.join(format!(".processes-{}.tmp", orch_types::new_id().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| HostError::io("opening the process journal", error))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| HostError::io("saving the process journal", error))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| HostError::io("replacing the process journal", error))?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| HostError::io("saving the process journal directory", error))?;
        Ok(())
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Which processes are the environment's.
    #[must_use]
    pub fn scope(&self) -> ScopeKind {
        self.inner.scope
    }

    /// The same native aggregate before every owned child, set before it can be started.
    pub fn set_launcher(&self, launcher: Vec<String>) -> Result<()> {
        *self
            .inner
            .launcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = launcher;
        self.persist()
    }

    /// The declared native aggregate for executor and terminal launches.
    #[must_use]
    pub fn launcher(&self) -> Vec<String> {
        self.inner
            .launcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Creates a process within this environment's aggregate before any program instruction runs.
    #[must_use]
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
        crate::process_limits::command(program, &self.launcher())
    }

    /// Learns the variables through which the commands an executor starts carry their ids.
    pub fn add_attribution(&self, attribution: Attribution) {
        let mut all = self
            .inner
            .attributions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !all.contains(&attribution) {
            all.push(attribution);
        }
    }

    /// Notes that the process `pid`, which the host or an executor just started, belongs to the
    /// environment, with everything that descends from it.
    pub fn register_root(&self, pid: u32, kind: RootKind) -> Result<()> {
        let start_ms = start_of(pid)
            .ok_or_else(|| HostError::NotFound(format!("process {pid} is not there")))?;
        self.state().roots.insert(pid, Root { start_ms, kind });
        self.persist()
    }

    /// Forgets that `pid` was started for the environment (what descends from it is not forgotten
    /// until it ends).
    pub fn forget_root(&self, pid: u32) {
        self.state().roots.remove(&pid);
    }

    /// Notes that `pid` is the connector or one of the processes it runs on its own account: it is
    /// neither listed nor signalled, though what descends from it is.
    pub fn protect(&self, pid: u32) -> Result<()> {
        let start_ms = start_of(pid)
            .ok_or_else(|| HostError::NotFound(format!("process {pid} is not there")))?;
        self.state().protected.insert(pid, start_ms);
        Ok(())
    }

    /// Whether this native number still names the process whose start was observed.
    #[must_use]
    pub fn is_current(&self, pid: u32, start_ms: i64) -> bool {
        start_of(pid) == Some(start_ms)
    }

    /// Current protected process numbers whose sockets must remain Internal.
    #[must_use]
    pub fn protected_pids(&self) -> Vec<u32> {
        let protected = self.state().protected.clone();
        protected
            .into_iter()
            .filter(|(pid, start)| self.is_current(*pid, *start))
            .map(|(pid, _)| pid)
            .chain(std::iter::once(platform::own_pid()))
            .collect()
    }

    /// Starts `command` as a process of the environment, in a process group of its own so that it
    /// and everything it starts can be told apart from the rest of the machine.
    pub fn spawn_tracked(
        &self,
        command: &mut std::process::Command,
        kind: RootKind,
    ) -> std::io::Result<std::process::Child> {
        platform::prepare_group(command);
        let child = command.spawn()?;
        if let Err(error) = self.register_root(child.id(), kind) {
            tracing::warn!(pid = child.id(), %error, "a process just started could not be noted");
        }
        Ok(child)
    }

    // ------------------------------------------------------------------ the table

    /// Every process of the machine, as little as it takes to find trees among them.
    fn rows(&self) -> Vec<Row> {
        let rows: Vec<Row> = match platform::os::table() {
            Some(table) => table.into_iter().map(Row::from).collect(),
            None => self.portable_rows(),
        };
        let alive: BTreeSet<(u32, i64)> = rows.iter().map(|row| (row.pid, row.start_ms)).collect();
        let mut state = self.state();
        state.environments.retain(|key, _| alive.contains(key));
        state
            .seen
            .retain(|pid, start| alive.contains(&(*pid, *start)));
        rows
    }

    /// The table from the portable library, where the system has no file to read it from.
    fn portable_rows(&self) -> Vec<Row> {
        let mut guard = self.state();
        let state = &mut *guard;
        state.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        state
            .system
            .processes()
            .iter()
            .map(|(pid, process)| {
                let number = pid.as_u32();
                let (letter, zombie) = state_letter(process.status());
                Row {
                    pid: number,
                    ppid: process.parent().map(Pid::as_u32),
                    group: platform::process_group_of(number),
                    session: process
                        .session_id()
                        .map(Pid::as_u32)
                        .or_else(|| platform::session_of(number)),
                    start_ms: i64::try_from(process.start_time())
                        .unwrap_or(0)
                        .saturating_mul(1000),
                    zombie,
                    state: letter,
                }
            })
            .collect()
    }

    /// What is read for the processes that are listed: their names, command lines, owners, use of
    /// memory and CPU, and the ids their executors gave them.
    fn details(&self, rows: &[&Row]) -> HashMap<u32, Detail> {
        let attributions = self
            .inner
            .attributions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let wanted: BTreeSet<&str> = attributions
            .iter()
            .flat_map(|a| [a.thread_id.as_str(), a.tool_call_id.as_str()])
            .collect();
        let mut guard = self.state();
        let state = &mut *guard;
        let mut kind = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet);
        if !wanted.is_empty() {
            kind = kind.with_environ(UpdateKind::OnlyIfNotSet);
        }
        let ids: Vec<Pid> = rows.iter().map(|row| Pid::from_u32(row.pid)).collect();
        state
            .system
            .refresh_processes_specifics(ProcessesToUpdate::Some(&ids), true, kind);
        let mut out = HashMap::new();
        for row in rows {
            let Some(process) = state.system.process(Pid::from_u32(row.pid)) else {
                continue;
            };
            let environment = state
                .environments
                .entry((row.pid, row.start_ms))
                .or_insert_with(|| {
                    let mut found = BTreeMap::new();
                    for line in process.environ() {
                        let line = line.to_string_lossy();
                        if let Some((key, value)) = line.split_once('=')
                            && wanted.contains(key)
                        {
                            found.insert(key.to_owned(), value.to_owned());
                        }
                    }
                    found
                });
            let mut thread = None;
            let mut item = None;
            for attribution in &attributions {
                let found = attribution.read(environment);
                thread = thread.or_else(|| {
                    found
                        .thread_id
                        .as_deref()
                        .and_then(|text| text.parse().ok())
                });
                item = item.or_else(|| {
                    found
                        .tool_call_id
                        .as_deref()
                        .and_then(|text| text.parse().ok())
                });
            }
            let name = process.name().to_string_lossy().into_owned();
            let command = {
                let parts: Vec<String> = process
                    .cmd()
                    .iter()
                    .map(|part| part.to_string_lossy().into_owned())
                    .collect();
                if parts.is_empty() {
                    format!("[{name}]")
                } else {
                    parts.join(" ")
                }
            };
            let user = process
                .user_id()
                .map(|uid| {
                    state
                        .users
                        .get_user_by_id(uid)
                        .map_or_else(|| uid.to_string(), |user| user.name().to_owned())
                })
                .unwrap_or_default();
            out.insert(
                row.pid,
                Detail {
                    name,
                    command,
                    user,
                    cpu: f64::from(process.cpu_usage()),
                    rss: process.memory(),
                    thread,
                    item,
                },
            );
        }
        out
    }

    /// The processes the environment owns, as `rows` shows them, and remembers them.
    fn visible(&self, rows: &[Row]) -> BTreeSet<u32> {
        let mut state = self.state();
        let by_pid: BTreeMap<u32, &Row> = rows.iter().map(|row| (row.pid, row)).collect();
        match self.inner.scope {
            ScopeKind::All => {
                let own = platform::own_pid();
                let protected: BTreeSet<u32> = state
                    .protected
                    .iter()
                    .filter(|(pid, start)| {
                        by_pid.get(pid).is_some_and(|row| row.start_ms == **start)
                    })
                    .map(|(pid, _)| *pid)
                    .chain(std::iter::once(own))
                    .collect();
                rows.iter()
                    .map(|row| row.pid)
                    .filter(|pid| !protected.contains(pid))
                    .collect()
            }
            ScopeKind::Started => {
                let carriers = self.started_by_executors(&mut state, rows);
                let roots: Vec<u32> = state
                    .roots
                    .iter()
                    .filter(|(pid, root)| {
                        by_pid
                            .get(pid)
                            .is_some_and(|row| row.start_ms == root.start_ms)
                    })
                    .map(|(pid, _)| *pid)
                    .collect();
                let seen: BTreeSet<u32> = state
                    .seen
                    .iter()
                    .filter(|(pid, start)| {
                        by_pid.get(pid).is_some_and(|row| row.start_ms == **start)
                    })
                    .map(|(pid, _)| *pid)
                    .collect();
                let found = grow(
                    rows,
                    roots
                        .iter()
                        .chain(seen.iter())
                        .chain(carriers.iter())
                        .copied()
                        .collect(),
                    &state.roots,
                );
                for pid in &found {
                    if let Some(row) = by_pid.get(pid) {
                        state.seen.insert(*pid, row.start_ms);
                    }
                }
                found
            }
        }
    }

    /// The children of the host's own process that an executor started. An executor runs inside the
    /// host, so what it starts is a child of the host's process; it is told apart from what else the host
    /// starts (a call to `git`, say) by the variables through which an executor's commands say which
    /// thread they are for, which the executors declared.
    fn started_by_executors(&self, state: &mut State, rows: &[Row]) -> Vec<u32> {
        let names: BTreeSet<String> = self
            .inner
            .attributions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .flat_map(|a| [a.thread_id.clone(), a.tool_call_id.clone()])
            .collect();
        if names.is_empty() {
            return Vec::new();
        }
        let own = platform::own_pid();
        let children: Vec<&Row> = rows
            .iter()
            .filter(|row| row.ppid == Some(own) && !row.zombie)
            .collect();
        if children.is_empty() {
            return Vec::new();
        }
        let ids: Vec<Pid> = children.iter().map(|row| Pid::from_u32(row.pid)).collect();
        state.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&ids),
            true,
            ProcessRefreshKind::nothing().with_environ(UpdateKind::OnlyIfNotSet),
        );
        children
            .into_iter()
            .filter(|row| {
                state
                    .system
                    .process(Pid::from_u32(row.pid))
                    .is_some_and(|process| {
                        process.environ().iter().any(|line| {
                            line.to_string_lossy()
                                .split_once('=')
                                .is_some_and(|(key, _)| names.contains(key))
                        })
                    })
            })
            .map(|row| row.pid)
            .collect()
    }

    // ------------------------------------------------------------------ listing

    /// Lists the processes of the environment.
    #[must_use]
    pub fn list(&self) -> Vec<Listed> {
        let rows = self.rows();
        let visible = self.visible(&rows);
        let by_pid: BTreeMap<u32, &Row> = rows.iter().map(|row| (row.pid, row)).collect();
        let listed_rows: Vec<&Row> = rows
            .iter()
            .filter(|row| visible.contains(&row.pid))
            .collect();
        let details = self.details(&listed_rows);
        let roots = self.state().roots.clone();
        let mut out = Vec::new();
        for row in listed_rows {
            let detail = details.get(&row.pid).cloned().unwrap_or_default();
            // A process without ids takes those of the nearest ancestor that has them.
            let (mut thread, mut item) = (detail.thread, detail.item);
            let mut shell = None;
            let mut cursor = Some(row.pid);
            let mut steps = 0;
            while let Some(pid) = cursor {
                steps += 1;
                if steps > ANCESTOR_STEPS_MAX {
                    break;
                }
                let Some(ancestor) = by_pid.get(&pid) else {
                    break;
                };
                if thread.is_none() && item.is_none() {
                    let found = details.get(&pid);
                    thread = found.and_then(|d| d.thread);
                    item = found.and_then(|d| d.item);
                }
                if shell.is_none()
                    && let Some(Root {
                        kind: RootKind::Shell(id),
                        start_ms,
                    }) = roots.get(&pid)
                    && *start_ms == ancestor.start_ms
                {
                    shell = Some(*id);
                }
                cursor = ancestor.ppid.filter(|parent| *parent != pid);
            }
            out.push(Listed {
                info: ProcessInfo {
                    pid: row.pid,
                    ppid: row.ppid.unwrap_or(0),
                    name: detail.name.clone(),
                    command: detail.command.clone(),
                    user: detail.user.clone(),
                    state: row.state.to_string(),
                    cpu_percent: detail.cpu,
                    memory_bytes: i64::try_from(detail.rss).unwrap_or(i64::MAX),
                    started_at: time_of(row.start_ms),
                    thread_id: thread,
                    item_id: item,
                },
                start_ms: row.start_ms,
                group: row.group,
                shell,
            });
        }
        out.sort_by_key(|listed| listed.info.pid);
        out
    }

    /// When the list was taken.
    #[must_use]
    pub fn now(&self) -> Time {
        self.inner.clock.time()
    }

    /// The listed processes with those of `thread` first, then the rest, each in the order of
    /// their numbers.
    #[must_use]
    pub fn ordered_for(listed: Vec<Listed>, thread: Option<Id>) -> Vec<Listed> {
        let (mut own, rest): (Vec<_>, Vec<_>) = listed
            .into_iter()
            .partition(|p| thread.is_some() && p.info.thread_id == thread);
        own.extend(rest);
        own
    }

    // ------------------------------------------------------------------ signalling

    /// Sends `signal` to the process `pid` that started at `started_at`.
    ///
    /// `INT` goes to that process, and to its whole group when it leads one, as a key press at a
    /// terminal would. `TERM` and `KILL` go to the process and everything that descends from it.
    pub fn signal(&self, pid: i64, started_at: Time, signal: Signal) -> Result<()> {
        let number = u32::try_from(pid)
            .map_err(|_| HostError::NotFound(format!("{pid} is not a process")))?;
        if number <= 1 {
            return Err(HostError::Unsupported(
                "process 1 and below are never signalled".to_owned(),
            ));
        }
        if number == platform::own_pid() {
            return Err(HostError::Unsupported(
                "the host does not signal itself".to_owned(),
            ));
        }
        let rows = self.rows();
        let visible = self.visible(&rows);
        let target = rows
            .iter()
            .find(|row| row.pid == number && visible.contains(&number))
            .ok_or_else(|| {
                HostError::NotFound(format!("process {number} is not one of this environment's"))
            })?;
        if time_of(target.start_ms) != started_at {
            return Err(HostError::NotFound(format!(
                "process {number} did not start then: the number names another process now"
            )));
        }
        match signal {
            Signal::Int => {
                let leads_a_group = target.group == Some(number);
                let sent = if leads_a_group {
                    platform::send_group(number, Sig::Int)
                } else {
                    platform::send_signal(number, Some(target.start_ms), Sig::Int)
                };
                sent.map_err(|error| signalling(number, &error))
            }
            Signal::Term => self.signal_tree(number, Sig::Term).map(|_| ()),
            Signal::Kill => self.signal_tree(number, Sig::Kill).map(|_| ()),
        }
    }

    /// Holds every member of the tree of `root` still while the members are found, sends `sig` to
    /// each, and lets them go on when the signal is not one that ends them. Says who was reached.
    pub fn signal_tree(&self, root: u32, sig: Sig) -> Result<Vec<(u32, i64)>> {
        let mut frozen: Vec<(u32, i64)> = Vec::new();
        let mut known: BTreeSet<u32> = BTreeSet::new();
        for _ in 0..FREEZE_ROUNDS {
            let rows = self.rows();
            let visible = self.visible(&rows);
            let mut seeds: BTreeSet<u32> = known.iter().copied().collect();
            if visible.contains(&root) {
                seeds.insert(root);
            }
            let members = tree_from(&rows, &seeds, &visible);
            let starts: BTreeMap<u32, i64> =
                rows.iter().map(|row| (row.pid, row.start_ms)).collect();
            let fresh: Vec<u32> = members
                .into_iter()
                .filter(|pid| !known.contains(pid))
                .collect();
            if fresh.is_empty() {
                break;
            }
            for pid in fresh {
                let start = starts.get(&pid).copied();
                match platform::send_signal(pid, start, Sig::Stop) {
                    Ok(()) => {
                        known.insert(pid);
                        frozen.push((pid, start.unwrap_or(0)));
                    }
                    // Gone since the table was read, or a number another process has now.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) if pid == root => return Err(signalling(pid, &error)),
                    Err(error) => {
                        tracing::debug!(pid, %error, "a member of a process tree could not be held still")
                    }
                }
            }
        }
        if frozen.is_empty() {
            return Err(HostError::NotFound(format!(
                "process {root} is not one of this environment's"
            )));
        }
        for (pid, start) in &frozen {
            let _ = platform::send_signal(*pid, Some(*start), sig);
        }
        if sig != Sig::Kill {
            for (pid, start) in &frozen {
                let _ = platform::send_signal(*pid, Some(*start), Sig::Continue);
            }
        }
        Ok(frozen)
    }

    /// Ends the tree of `root`: asks with `TERM`, waits up to `grace` for every member to end, and
    /// kills those that did not, and what they started since, until none is left.
    pub async fn stop_tree(&self, root: u32, grace: Duration) -> Result<StopReport> {
        let this = self.clone();
        let members = tokio::task::spawn_blocking(move || this.signal_tree(root, Sig::Term))
            .await
            .map_err(|error| {
                HostError::Internal(format!("a task of the host failed: {error}"))
            })??;
        let mut everyone: BTreeMap<u32, i64> = members.into_iter().collect();
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            if self.survivors(&everyone).await.is_empty() {
                return Ok(StopReport {
                    killed: false,
                    members: everyone.len(),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        for _ in 0..KILL_ROUNDS {
            let alive = self.survivors(&everyone).await;
            if alive.is_empty() {
                break;
            }
            for pid in alive {
                let this = self.clone();
                let reached = tokio::task::spawn_blocking(move || this.signal_tree(pid, Sig::Kill))
                    .await
                    .map_err(|error| {
                        HostError::Internal(format!("a task of the host failed: {error}"))
                    })?;
                if let Ok(reached) = reached {
                    everyone.extend(reached);
                }
            }
        }
        let end = tokio::time::Instant::now() + KILL_WAIT;
        while !self.survivors(&everyone).await.is_empty() {
            if tokio::time::Instant::now() >= end {
                return Err(HostError::Timeout(format!(
                    "process {root} and what it started did not end"
                )));
            }
            tokio::time::sleep(POLL).await;
        }
        Ok(StopReport {
            killed: true,
            members: everyone.len(),
        })
    }

    /// Which of the processes (by number and start) are alive and not just waiting to be reaped.
    async fn survivors(&self, everyone: &BTreeMap<u32, i64>) -> Vec<u32> {
        let this = self.clone();
        let wanted = everyone.clone();
        tokio::task::spawn_blocking(move || {
            let rows = this.rows();
            rows.iter()
                .filter(|row| !row.zombie && wanted.get(&row.pid) == Some(&row.start_ms))
                .map(|row| row.pid)
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    // ------------------------------------------------------------------ holding still

    /// Holds still every process the environment's agents started, by its verified identity,
    /// so that the files stand still while they are copied. Says how many
    /// processes were held. Where the platform cannot, it says so.
    pub fn suspend(&self) -> Result<i64> {
        if !platform::CAN_SUSPEND {
            return Err(HostError::Unsupported(
                "this platform cannot hold processes still".to_owned(),
            ));
        }
        let _operation = self
            .inner
            .suspension
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for _ in 0..FREEZE_ROUNDS {
            let rows = self.rows();
            let visible = self.visible(&rows);
            let pending: Vec<Held> = rows
                .iter()
                .filter(|row| {
                    row.pid > 1
                        && row.pid != platform::own_pid()
                        && visible.contains(&row.pid)
                        && !row.zombie
                        && row.state != 'T'
                })
                .map(|row| Held {
                    pid: row.pid,
                    start_ms: row.start_ms,
                })
                .collect();
            if pending.is_empty() {
                let held = self.state().held.clone();
                return Ok(i64::try_from(
                    rows.iter()
                        .filter(|row| {
                            visible.contains(&row.pid)
                                && row.state == 'T'
                                && held.contains(&Held {
                                    pid: row.pid,
                                    start_ms: row.start_ms,
                                })
                        })
                        .count(),
                )
                .unwrap_or(i64::MAX));
            }
            for item in pending {
                {
                    let mut state = self.state();
                    if !state.held.contains(&item) {
                        state.held.push(item);
                    }
                }
                // Save the intent before STOP, so a crash at either side of the signal can resume it.
                self.persist()?;
                if let Err(error) = platform::send_signal(item.pid, Some(item.start_ms), Sig::Stop)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(signalling(item.pid, &error));
                }
            }
        }
        Err(HostError::Timeout(
            "the environment's processes did not settle while being suspended".to_owned(),
        ))
    }

    /// Lets go of what `suspend` held still. Says how many processes were let go.
    pub fn resume(&self) -> Result<i64> {
        if !platform::CAN_SUSPEND {
            return Err(HostError::Unsupported(
                "this platform cannot hold processes still".to_owned(),
            ));
        }
        let _operation = self
            .inner
            .suspension
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let held = self.state().held.clone();
        let rows = self.rows();
        let visible = self.visible(&rows);
        let mut count = 0i64;
        for item in held {
            if item.pid > 1
                && item.pid != platform::own_pid()
                && visible.contains(&item.pid)
                && rows
                    .iter()
                    .any(|row| row.pid == item.pid && row.start_ms == item.start_ms && !row.zombie)
            {
                platform::send_signal(item.pid, Some(item.start_ms), Sig::Continue)
                    .map_err(|error| signalling(item.pid, &error))?;
                count += 1;
            }
            // The identity ended, was reused or is outside this environment: forget it without a signal.
            self.state().held.retain(|saved| *saved != item);
            self.persist()?;
        }
        Ok(count)
    }

    /// How many processes the environment has now, and what they use together: the total of the
    /// CPU in percent of one CPU and of the resident memory in bytes.
    #[must_use]
    pub fn totals(&self) -> (i64, f64, i64) {
        let rows = self.rows();
        let visible = self.visible(&rows);
        let live: Vec<&Row> = rows
            .iter()
            .filter(|row| visible.contains(&row.pid) && !row.zombie)
            .collect();
        let details = self.details(&live);
        let cpu: f64 = details.values().map(|d| d.cpu).sum();
        let memory = details.values().fold(0i64, |sum, d| {
            sum.saturating_add(i64::try_from(d.rss).unwrap_or(i64::MAX))
        });
        (i64::try_from(live.len()).unwrap_or(i64::MAX), cpu, memory)
    }
}

/// How the end of a tree went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopReport {
    /// Whether the members had to be killed because they did not end on `TERM` in time.
    pub killed: bool,
    /// How many processes were reached.
    pub members: usize,
}

fn signalling(pid: u32, error: &std::io::Error) -> HostError {
    match error.kind() {
        std::io::ErrorKind::NotFound => HostError::NotFound(format!("process {pid} is gone")),
        _ => HostError::io(
            format!("signalling process {pid}"),
            std::io::Error::new(error.kind(), error.to_string()),
        ),
    }
}

/// When the process `pid` started, in milliseconds since 1970, to the finest the platform gives.
fn start_of(pid: u32) -> Option<i64> {
    if let Some(exact) = platform::os::exact_start_ms(pid) {
        return Some(exact);
    }
    let mut system = System::new();
    let number = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[number]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(number).map(|process| {
        i64::try_from(process.start_time())
            .unwrap_or(0)
            .saturating_mul(1000)
    })
}

/// The processes of `raws` that descend from the seeds, by parentage, by a process group or a
/// session that a member leads, and the seeds themselves; the registered roots lead their own
/// groups and sessions whether or not they are still alive.
fn grow(rows: &[Row], seeds: BTreeSet<u32>, roots: &BTreeMap<u32, Root>) -> BTreeSet<u32> {
    let mut found: BTreeSet<u32> = seeds;
    let own_group = platform::own_process_group();
    let led: BTreeSet<u32> = roots.keys().copied().collect();
    loop {
        let before = found.len();
        let leaders: BTreeSet<u32> = found.iter().copied().chain(led.iter().copied()).collect();
        for raw in rows {
            if found.contains(&raw.pid) {
                continue;
            }
            let by_parent = raw
                .ppid
                .is_some_and(|parent| found.contains(&parent) && parent > 1);
            let by_group = raw.group.is_some_and(|group| {
                leaders.contains(&group) && group > 1 && Some(group) != own_group
            });
            let by_session = raw.session.is_some_and(|session| {
                leaders.contains(&session)
                    && session > 1
                    && Some(session) != platform::process_group_of(platform::own_pid())
            });
            if by_parent || by_group || by_session {
                found.insert(raw.pid);
            }
        }
        if found.len() == before {
            break;
        }
    }
    // A process is never its own ancestor and the host is never part of what it signals.
    found.remove(&platform::own_pid());
    found.retain(|pid| *pid > 1);
    found
}

/// The tree of `seeds` inside `visible`: the seeds, and everything that descends from them.
fn tree_from(rows: &[Row], seeds: &BTreeSet<u32>, visible: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut members: BTreeSet<u32> = seeds
        .iter()
        .copied()
        .filter(|pid| visible.contains(pid))
        .collect();
    loop {
        let before = members.len();
        for raw in rows {
            if members.contains(&raw.pid) || !visible.contains(&raw.pid) {
                continue;
            }
            let by_parent = raw.ppid.is_some_and(|parent| members.contains(&parent));
            let by_group = raw.group.is_some_and(|group| {
                members.contains(&group) && Some(group) != platform::own_process_group()
            });
            if by_parent || by_group {
                members.insert(raw.pid);
            }
        }
        if members.len() == before {
            return members;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pid: u32, ppid: u32, group: u32) -> Row {
        Row {
            pid,
            ppid: Some(ppid),
            group: Some(group),
            session: Some(group),
            start_ms: 1,
            zombie: false,
            state: 'S',
        }
    }

    #[test]
    fn a_tree_is_what_descends_by_parent_or_by_the_group_a_member_leads() {
        // 100 leads a group; 101 is its child; 102 left home (its parent died: now 1) but stays in the group;
        // 103 is 101's child in a group of its own; 200 is unrelated.
        let raws = vec![
            raw(100, 50, 100),
            raw(101, 100, 100),
            raw(102, 1, 100),
            raw(103, 101, 103),
            raw(104, 1, 103),
            raw(200, 1, 200),
        ];
        let visible: BTreeSet<u32> = raws.iter().map(|r| r.pid).collect();
        let seeds: BTreeSet<u32> = [100].into_iter().collect();
        let tree = tree_from(&raws, &seeds, &visible);
        assert_eq!(
            tree.into_iter().collect::<Vec<_>>(),
            [100, 101, 102, 103, 104]
        );
    }

    #[test]
    fn a_tree_never_leaves_what_is_visible() {
        let raws = vec![raw(100, 50, 100), raw(101, 100, 100), raw(102, 101, 102)];
        let visible: BTreeSet<u32> = [100, 101].into_iter().collect();
        let seeds: BTreeSet<u32> = [100].into_iter().collect();
        assert_eq!(
            tree_from(&raws, &seeds, &visible).len(),
            2,
            "102 is not the environment's, so it is not reached"
        );
    }

    #[test]
    fn what_descends_from_a_started_process_is_the_environments_and_the_rest_of_the_machine_is_not()
    {
        let mut roots = BTreeMap::new();
        roots.insert(
            100,
            Root {
                start_ms: 1,
                kind: RootKind::Executor,
            },
        );
        let raws = vec![
            raw(100, 50, 100),
            raw(101, 100, 100),
            raw(150, 1, 100),
            raw(300, 50, 300),
            raw(301, 300, 300),
        ];
        let found = grow(&raws, [100].into_iter().collect(), &roots);
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            [100, 101, 150],
            "the group of the root is the root's even when its parent is gone"
        );
        // The root has ended, and its group lives on: it is still found by the group the root led.
        let without_root = vec![raw(101, 1, 100), raw(150, 1, 100), raw(300, 50, 300)];
        assert_eq!(
            grow(&without_root, BTreeSet::new(), &roots)
                .into_iter()
                .collect::<Vec<_>>(),
            [101, 150]
        );
    }

    #[test]
    fn listed_processes_are_ordered_with_one_threads_own_first() {
        let thread: Id = orch_types::new_id();
        let make = |pid: u32, thread_id: Option<Id>| Listed {
            info: ProcessInfo {
                pid,
                ppid: 1,
                name: String::new(),
                command: String::new(),
                user: String::new(),
                state: "S".into(),
                cpu_percent: 0.0,
                memory_bytes: 0,
                started_at: time_of(0),
                thread_id,
                item_id: None,
            },
            start_ms: 0,
            group: None,
            shell: None,
        };
        let ordered = Processes::ordered_for(
            vec![
                make(1, None),
                make(2, Some(thread)),
                make(3, None),
                make(4, Some(thread)),
            ],
            Some(thread),
        );
        assert_eq!(
            ordered.iter().map(|p| p.info.pid).collect::<Vec<_>>(),
            [2, 4, 1, 3]
        );
        let unordered = Processes::ordered_for(vec![make(2, Some(thread)), make(1, None)], None);
        assert_eq!(
            unordered.iter().map(|p| p.info.pid).collect::<Vec<_>>(),
            [2, 1]
        );
    }
}
