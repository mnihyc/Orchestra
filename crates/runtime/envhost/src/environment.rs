//! Attach and detach, workspace roots and environment facts.
//!
//! [`EnvHost`] is everything the connector's environment role needs to carry out the commands of
//! the catalogue that are about an environment. It holds the environments that are attached to
//! this connector, each with its workspace (files, ignore rules, index, watch, manifest), its
//! mounts, its processes, shells, executors and tool servers; and it has one function for each
//! command. The functions take the command's body and give its result, and nothing here talks to
//! the network: where content has to travel, the caller hands in the end of a content transfer
//! that belongs to the command ([`BlobSink`] to put content out, [`BlobSource`] to take it in).
//!
//! **Attaching** an environment finds its workspace root (the one the driver gave a managed unit,
//! or a directory on a person's machine, which has to lie in the roots the machine's owner
//! offers), opens it, finds out whether file names differ by case, reads the ignore rules, walks
//! the tree once to see whether it can be indexed, attaches the mounts, and reports the
//! environment's facts. Attaching again, as happens when the mounts of an in-place environment
//! change, updates the environment and keeps its shells, processes and servers.
//!
//! **What an environment says on its own account** (a directory it was asked to watch changed, a
//! manifest of the tree was uploaded, a shell ended while nobody was attached, its facts changed)
//! goes out through [`Reports`], a bounded queue the connector drains into the environment's
//! stream; measurements are dropped when it is full, since the next replaces them.

use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use orch_agent_contract::discovery::{McpDefinition, McpServerSpec};
use orch_api_contract::processes::{ProcessInfo, ResourceSample};
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_driver_contract::runtime::{Resources, ToolRef};
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_link_contract::commands::{
    BlobRef, EnvAttach, EnvDetach, ExecStart, ExecStartResult, FsArchiveList, FsArchiveRead,
    FsArchiveReadResult, FsCopy, FsFetch, FsFetchResult, FsList, FsListResult, FsManifest,
    FsManifestResult, FsMaterialize, FsMaterializeResult, FsMkdir, FsRemove, FsRename, FsSearch,
    FsSearchResult, FsSnapshot, FsSnapshotResult, FsStat, FsUnwatch, FsWrite, GitDiff,
    GitDiffResult, GitStatus, McpImport, McpImportResult, McpStart, McpStartResult, McpStop,
    ProcHeldResult, ProcList, ProcListResult, ProcResume, ProcSignal, ProcSuspend, PtyClose,
    PtyCloseResult, PtyOpen, PtyOpenResult, Selection, WatchSet,
};
use orch_link_contract::events::{
    EnvManifest, EnvironmentFacts, EnvironmentFactsShell, FsChanged, PathStyle, PtyExited,
};
use orch_link_contract::session::{Arch, Os};
use orch_link_contract::tunnels::Service;
use orch_types::{Clock, Hash, Id, Time};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;

use crate::blobs::{BlobSink, BlobSource, MemoryReader, MemoryWriter, download_checked};
use crate::directories::ThreadDirectory;
use crate::error::{HostError, Result, blocking};
use crate::executors::Executors;
use crate::files::Files;
use crate::git::GitTool;
use crate::ignore::IgnoreRules;
use crate::index::{HashCache, Index, ScanOutcome};
use crate::manifest::{self, Publisher};
use crate::mcp::{McpGates, McpServers, Origin, ServerState};
use crate::mcp_host::{Launch, McpEndpoint, McpHost, McpHostConfig};
use crate::mounts::{MountPolicy, Mounts, RUN_DIR};
use crate::paths::{Locate, RelPath, Workspace};
use crate::platform::{self, Dir};
use crate::processes::{Processes, ScopeKind};
use crate::resources::Sampler;
use crate::search::SearchRequest;
use crate::snapshot::{NoProgress, Progress};
use crate::terminal::{Attachment, OpenSpec, Purpose, TerminalLinks, Terminals, find_shell};
use crate::watch::{Change, WatchConfig, WatchMode, Watcher};

mod directory_watches;
mod git_watches;
mod retirement;

#[cfg(all(test, target_os = "linux"))]
mod directory_lifecycle_tests;
#[cfg(all(test, target_os = "linux"))]
mod directory_retirement_tests;
#[cfg(all(test, target_os = "linux"))]
mod directory_watch_tests;
#[cfg(all(test, target_os = "linux"))]
mod recording_admission_tests;

// ---------------------------------------------------------------------------------------------
// Limits and gates

/// The most bytes of one file that is stored (`storage.max_blob_bytes`): eight gibibytes.
pub const DEFAULT_MAX_BLOB_BYTES: i64 = 8 * 1024 * 1024 * 1024;

/// The most bytes of a text, or of an entry of an archive, that are read to be shown
/// (`[limits] preview_bytes`): two mebibytes.
pub const DEFAULT_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;

/// Above this many entries a workspace is not indexed (`[envhost] index_max_entries`).
pub const DEFAULT_INDEX_MAX_ENTRIES: u64 = 250_000;

/// How often a tree is walked again where watching falls short (`[envhost] rescan_secs`).
pub const DEFAULT_RESCAN: Duration = Duration::from_secs(60);

/// How often a manifest is published while files keep changing (`[envhost] manifest_interval_secs`).
pub const DEFAULT_MANIFEST_INTERVAL: Duration = Duration::from_secs(300);

/// How long a search inside files runs before it says it stopped short.
pub const DEFAULT_SEARCH_TIME: Duration = Duration::from_secs(20);

/// How long the reading of a compressed archive runs before it says it stopped short.
pub const DEFAULT_ARCHIVE_TIME: Duration = Duration::from_secs(20);

/// How many bytes of what a shell printed are kept for whoever takes it up again.
pub const DEFAULT_TERMINAL_KEEP_BYTES: usize = 256 * 1024;

/// The most bytes of a difference of a repository that are returned.
pub const DEFAULT_GIT_DIFF_MAX_BYTES: usize = 2 * 1024 * 1024;

/// How long a run of `git` may take before it is ended: a little under the command's own 30 s.
pub const DEFAULT_GIT_TIME: Duration = Duration::from_secs(25);

/// How many times a tool server that keeps crashing is started again in a window.
pub const DEFAULT_MCP_RESTARTS: u32 = 3;

/// The window in which a tool server may be started again that often: ten minutes.
pub const DEFAULT_MCP_RESTART_WINDOW: Duration = Duration::from_secs(600);

/// The bounds the environment host keeps to.
#[derive(Debug, Clone)]
pub struct Limits {
    /// The most bytes of one file that is stored.
    pub max_blob_bytes: i64,
    /// The most bytes of a text or of an entry of an archive that are read to be shown.
    pub preview_bytes: u64,
    /// Above this many entries a workspace is not indexed.
    pub index_max_entries: u64,
    /// The longest one whole-tree scan is allowed before the index gives up.
    pub index_scan_max: Duration,
    /// How often a tree is walked again where watching falls short.
    pub rescan: Duration,
    /// How often a manifest is published while files keep changing.
    pub manifest_interval: Duration,
    /// How long a search inside files runs.
    pub search_time: Duration,
    /// How long the reading of a compressed archive runs.
    pub archive_time: Duration,
    /// How many bytes of what a shell printed are kept.
    pub terminal_keep_bytes: usize,
    /// The most bytes of a difference of a repository that are returned.
    pub git_diff_max_bytes: usize,
    /// How long a run of `git` may take.
    pub git_time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
            preview_bytes: DEFAULT_PREVIEW_BYTES,
            index_max_entries: DEFAULT_INDEX_MAX_ENTRIES,
            index_scan_max: crate::index::DEFAULT_SCAN_MAX,
            rescan: DEFAULT_RESCAN,
            manifest_interval: DEFAULT_MANIFEST_INTERVAL,
            search_time: DEFAULT_SEARCH_TIME,
            archive_time: DEFAULT_ARCHIVE_TIME,
            terminal_keep_bytes: DEFAULT_TERMINAL_KEEP_BYTES,
            git_diff_max_bytes: DEFAULT_GIT_DIFF_MAX_BYTES,
            git_time: DEFAULT_GIT_TIME,
        }
    }
}

/// What a machine's owner has switched on for work that comes from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gates {
    /// Whether files may be written, renamed, copied and removed from Orch.
    pub remote_write: bool,
    /// Whether files that arrive with a message may be written under `.orch/`.
    pub deliver_files: bool,
    /// Whether shells may be opened.
    pub terminal: bool,
    /// Whether browsers may be started, shown and driven.
    pub browser: bool,
    /// Whether displays may be started, shown and worked in.
    pub vnc: bool,
}

impl Gates {
    /// Everything on: a unit that Orch made, where nobody else's machine is at stake.
    pub const OPEN: Self = Self {
        remote_write: true,
        deliver_files: true,
        terminal: true,
        browser: true,
        vnc: true,
    };

    /// Everything off: a person's machine as it is installed.
    pub const CLOSED: Self = Self {
        remote_write: false,
        deliver_files: false,
        terminal: false,
        browser: false,
        vnc: false,
    };
}

/// The gates as they stand, which the machine's owner may change while the host runs.
#[derive(Debug)]
pub struct GateState {
    gates: RwLock<Gates>,
}

impl GateState {
    /// Gates that start as `gates`.
    #[must_use]
    pub fn new(gates: Gates) -> Self {
        Self {
            gates: RwLock::new(gates),
        }
    }

    /// The gates now.
    #[must_use]
    pub fn get(&self) -> Gates {
        *self.gates.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the gates.
    pub fn set(&self, gates: Gates) {
        *self.gates.write().unwrap_or_else(PoisonError::into_inner) = gates;
    }

    /// Refuses unless files may be written from outside.
    pub fn require_remote_write(&self) -> Result<()> {
        if self.get().remote_write {
            Ok(())
        } else {
            Err(HostError::RemoteWriteDisabled)
        }
    }

    /// Refuses unless files may be delivered with a message.
    pub fn require_deliver_files(&self) -> Result<()> {
        if self.get().deliver_files {
            Ok(())
        } else {
            Err(HostError::DeliverFilesDisabled)
        }
    }

    /// Refuses unless shells may be opened.
    pub fn require_terminal(&self) -> Result<()> {
        if self.get().terminal {
            Ok(())
        } else {
            Err(HostError::Unsupported(
                "shells are switched off on this machine".to_owned(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reports

/// What an environment host says on its own account, as events of the environment's stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    /// A known browser changed state or tab count, without page content.
    BrowserState(orch_link_contract::browser::BrowserState),
    /// A known display ended without retaining its picture or inputs.
    DisplayState(orch_link_contract::display::DisplayState),
    /// Entries of a watched directory that are new, changed or gone.
    FsChanged(FsChanged),
    /// A watched directory's repository status changed.
    GitChanged(orch_link_contract::events::GitChanged),
    /// Foreground state of the environment's shells, without their bytes.
    Terminals(orch_link_contract::commands::PtyStatusResult),
    /// A manifest of the tree was uploaded.
    Manifest(EnvManifest),
    /// The processes of the environment, as they are now.
    Procs {
        /// When the list was taken.
        as_of: Time,
        /// The processes.
        processes: Vec<ProcessInfo>,
    },
    /// What the environment uses.
    Stats(ResourceSample),
    /// A current scoped port measurement, carried only as a live report.
    Ports(orch_link_contract::ports::PortsSnapshot),
    /// A shell ended while nobody was attached to it.
    PtyExited(PtyExited),
    /// The facts of the environment changed.
    Facts(Box<EnvironmentFacts>),
    /// A tool server was given up on: it crashed more often than its budget allows, or could not be
    /// started again.
    McpFailed {
        /// The name the server is offered under.
        name: String,
        /// Why.
        reason: String,
    },
    /// A tool server that crashed runs again.
    McpRestarted {
        /// The name the server is offered under.
        name: String,
    },
}

impl Report {
    /// Whether losing this report would lose something nothing else says again: the others are
    /// measurements and the next one replaces them.
    #[must_use]
    pub fn is_durable(&self) -> bool {
        matches!(
            self,
            Self::Manifest(_)
                | Self::BrowserState(_)
                | Self::DisplayState(_)
                | Self::PtyExited(_)
                | Self::Facts(_)
                | Self::McpFailed { .. }
                | Self::McpRestarted { .. }
        )
    }
}

/// A report with the environment it is about.
#[derive(Debug, Clone, PartialEq)]
pub struct Reported {
    /// The environment.
    pub environment_id: Id,
    /// What is said.
    pub report: Report,
}

/// Where reports go: a bounded queue the connector drains into the environment's stream.
#[derive(Debug, Clone)]
pub struct Reports {
    sender: mpsc::Sender<Reported>,
}

impl Reports {
    /// A queue of `capacity` reports and the end the connector reads from.
    #[must_use]
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<Reported>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { sender }, receiver)
    }

    /// Says `report`. A measurement is dropped when the queue is full, since the next replaces it;
    /// anything else waits for room.
    pub async fn say(&self, environment_id: Id, report: Report) {
        let reported = Reported {
            environment_id,
            report,
        };
        if reported.report.is_durable() {
            let _ = self.sender.send(reported).await;
        } else {
            let _ = self.sender.try_send(reported);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Settings

/// Where a driver put things in the unit it made, for the connector inside it.
#[derive(Debug, Clone)]
pub struct ManagedPlaces {
    /// The workspace volume (`ORCH_WORKSPACE_DIR`).
    pub workspace_root: PathBuf,
    /// The home volume (`ORCH_HOME_DIR`).
    pub home: Option<PathBuf>,
    /// The file that lists the mounts the driver attached (`ORCH_MOUNTS_FILE`).
    pub mounts_file: Option<PathBuf>,
    /// The directory of harness builds (`ORCH_TOOLS_DIR`).
    pub tools_dir: Option<PathBuf>,
    /// Whether the driver itself keeps writes out of a read-only mount.
    pub mounts_enforced: bool,
}

/// What a connector tells its environment host about itself and the machine.
#[derive(Debug, Clone)]
pub struct HostSettings {
    /// The directory connector's independently proved native controllers.
    pub process_controls: orch_driver_contract::process_limits::ProcessControls,
    /// The connector's data directory, below which the host keeps what has to outlast a run.
    pub data_dir: PathBuf,
    /// The bounds it keeps to.
    pub limits: Limits,
    /// What the machine's owner has switched on.
    pub gates: Gates,
    /// The directories the machine's owner offers for workspaces and mounts; `None` for a unit
    /// where nothing limits them.
    pub allowed_roots: Option<Vec<PathBuf>>,
    /// The offers the machine's owner lets the exec node make; `None` for no limit.
    pub exec_offers_allowed: Option<Vec<String>>,
    /// What the machine's owner has switched on for tool servers.
    pub mcp: McpGates,
    /// How many times a crashing tool server is restarted.
    pub mcp_restarts: u32,
    /// The ports the tool servers may listen on, on the machine itself; `None` lets the system give them.
    pub mcp_ports: Option<RangeInclusive<u16>>,
    /// In how long.
    pub mcp_restart_window: Duration,
    /// Which processes of the machine are the environment's.
    pub process_scope: ScopeKind,
    /// Socket ownership scope; a private network namespace must be independently established.
    pub port_scope: crate::ports::PortScope,
    /// How watching is done.
    pub watch: WatchMode,
    /// Where a driver put things; `None` for a directory on a person's machine.
    pub managed: Option<ManagedPlaces>,
    /// The harness builds the environment has.
    pub tools: Vec<ToolRef>,
    /// The most the unit may use, as its driver set it.
    pub unit_limits: Resources,
    /// The shell a person gets, when the machine's own is not wanted.
    pub shell: Option<String>,
    /// How many times the unit was made anew.
    pub generation: i64,
}

impl HostSettings {
    /// Settings for a directory on a person's machine, where nothing is switched on until its
    /// owner says so and only the directories offered may be used.
    #[must_use]
    pub fn connected(data_dir: PathBuf, roots: Vec<PathBuf>) -> Self {
        Self {
            process_controls: Default::default(),
            data_dir,
            limits: Limits::default(),
            gates: Gates::CLOSED,
            allowed_roots: Some(roots),
            exec_offers_allowed: None,
            mcp: McpGates {
                import: true,
                add: false,
            },
            mcp_restarts: DEFAULT_MCP_RESTARTS,
            mcp_ports: None,
            mcp_restart_window: DEFAULT_MCP_RESTART_WINDOW,
            process_scope: ScopeKind::Started,
            port_scope: crate::ports::PortScope::Processes,
            watch: WatchMode::Native,
            managed: None,
            tools: Vec::new(),
            unit_limits: Resources::default(),
            shell: None,
            generation: 0,
        }
    }

    /// Settings for the connector inside a unit that a driver made.
    #[must_use]
    pub fn managed(data_dir: PathBuf, places: ManagedPlaces, scope: ScopeKind) -> Self {
        Self {
            gates: Gates::OPEN,
            allowed_roots: None,
            mcp: McpGates {
                import: true,
                add: true,
            },
            process_scope: scope,
            managed: Some(places),
            ..Self::connected(data_dir, Vec::new())
        }
    }
}

/// Makes an executor for an environment: every environment has instances of its own, since an
/// executor keeps nothing between sessions but is started once for each.
pub type ExecutorFactory = Arc<dyn Fn() -> Box<dyn Executor> + Send + Sync>;

/// Converts an API or native observation to a link body with the same wire form.
fn wire<T: Serialize, W: DeserializeOwned>(value: &T) -> Result<W> {
    let json = serde_json::to_value(value)?;
    Ok(serde_json::from_value(json)?)
}

// ---------------------------------------------------------------------------------------------
// The environment

/// How long attaching waits for the first walk of the tree before it answers: the facts say the
/// index is kept, the walk goes on, and the facts are sent again when it ends.
const ATTACH_SCAN_WAIT: Duration = Duration::from_secs(10);

/// The most directories one environment may have watched at once.
pub(crate) const DIR_WATCHES_MAX: usize = 256;

/// The shortest interval of a periodic report: a second. Faster is a load and not a view.
const REPORT_INTERVAL_MIN: Duration = Duration::from_secs(1);

/// The longest: a minute, past which a screen would show something old.
const REPORT_INTERVAL_MAX: Duration = Duration::from_secs(60);

/// How often the publisher is asked whether a manifest is due.
const PUBLISH_POLL: Duration = Duration::from_secs(1);

/// The most the compressed list of files of a move may be as it is downloaded: well above the
/// list of a tree at the entry limit, well under what would fill a machine.
const MANIFEST_DOWNLOAD_MAX_BYTES: usize = 512 * 1024 * 1024;

/// What is remembered of an entry of a watched directory, to tell what changed.
type Signature = (crate::platform::Kind, i64, i64, u32, Option<String>);

#[derive(Debug)]
struct DirWatch {
    instance: Id,
    wire_instance: Option<Id>,
    shown: RelPath,
    dir: RelPath,
    location: crate::files::DirectoryLocation,
    last: BTreeMap<String, Signature>,
    truncated: bool,
    invalidated: bool,
}

fn signature_of(entry: &DirEntry) -> Signature {
    let kind = match entry.kind {
        DirEntryKind::File => crate::platform::Kind::File,
        DirEntryKind::Dir => crate::platform::Kind::Dir,
        DirEntryKind::Symlink => crate::platform::Kind::Symlink,
        DirEntryKind::Other => crate::platform::Kind::Other,
    };
    (
        kind,
        entry.size,
        entry.mtime.unix_millis(),
        entry.mode,
        entry.link_target.clone(),
    )
}

/// Which measurements an environment's screens asked for, and how often.
#[derive(Debug, Clone, Copy)]
struct Wanted {
    stats: bool,
    procs: bool,
    ports: bool,
    terminals: bool,
    interval: Duration,
}

/// The first walk of a tree, when it was still going on after attaching waited for it.
type FirstScan = tokio::task::JoinHandle<Result<ScanOutcome>>;

/// A reserved detach, retained from before lifecycle hooks through teardown and its final hook.
pub struct DetachReservation {
    id: Id,
    environment: Option<Arc<Environment>>,
    store: Option<Arc<crate::state_store::DestructiveLease>>,
    cleanup: Option<Arc<retirement::Finalizer>>,
}

/// One attached environment.
pub struct Environment {
    id: Id,
    /// Reattach/rebase never reuses another attachment's directory metadata cursors.
    directory_attachment: Id,
    directory_operations: Arc<crate::directory_pages::Attachment>,
    host: Weak<HostInner>,
    files: Files,
    rules: Arc<IgnoreRules>,
    index: Arc<Index>,
    /// Whether the index holds what a walk of the whole tree found; a manifest is made from it
    /// only then.
    scanned: AtomicBool,
    mounts: Mounts,
    watcher: Mutex<Option<Watcher>>,
    /// Orders initial directory resets before subsequent native change batches.
    directory_reads: tokio::sync::Mutex<()>,
    watches: Mutex<HashMap<Id, DirWatch>>,
    /// Temporary initial reads share physical registrations without retaining HTTP subscriptions.
    directory_captures: Mutex<Vec<(crate::files::DirectoryLocation, usize)>>,
    git_watches: git_watches::Watches,
    publisher: Publisher,
    processes: Processes,
    resources: Resources,
    resource_group: Option<crate::process_limits::UserGroup>,
    ports: crate::ports::Ports,
    sampler: Arc<Sampler>,
    terminals: Arc<Terminals>,
    executors: Arc<Executors>,
    mcp: Arc<McpServers>,
    mcp_host: Arc<McpHost>,
    git: GitTool,
    file_states: Option<Arc<crate::file_states::FileStates>>,
    facts: Mutex<EnvironmentFacts>,
    variables: BTreeMap<String, String>,
    browsers: Arc<crate::browsers::Browsers>,
    windows: Arc<crate::browser_window::Windows>,
    displays: Arc<crate::displays::Displays>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    wanted: Mutex<Option<Wanted>>,
    reporters: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("id", &self.id)
            .field("root", &self.files.root().root_path())
            .finish_non_exhaustive()
    }
}

fn os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Macos
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    }
}

fn arch() -> Arch {
    if cfg!(target_arch = "aarch64") {
        Arch::Aarch64
    } else {
        Arch::X86_64
    }
}

fn path_style() -> PathStyle {
    if cfg!(windows) {
        PathStyle::Windows
    } else {
        PathStyle::Posix
    }
}

fn current_user() -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Environment {
    /// The environment's identifier.
    #[must_use]
    pub fn id(&self) -> Id {
        self.id
    }

    /// Its files.
    #[must_use]
    pub fn files(&self) -> &Files {
        &self.files
    }

    /// Variables and opened secrets supplied at attach, inherited by managed after-start commands.
    #[must_use]
    pub fn variables(&self) -> &BTreeMap<String, String> {
        &self.variables
    }

    /// Its mounts.
    #[must_use]
    pub fn mounts(&self) -> &Mounts {
        &self.mounts
    }

    /// Its processes.
    #[must_use]
    pub fn processes(&self) -> &Processes {
        &self.processes
    }

    /// The same scoped native port reader used for fresh reads and live watches.
    #[must_use]
    pub fn ports(&self) -> &crate::ports::Ports {
        &self.ports
    }

    /// Its shells.
    #[must_use]
    pub fn terminals(&self) -> &Terminals {
        &self.terminals
    }

    /// Its executors.
    #[must_use]
    pub fn executors(&self) -> &Arc<Executors> {
        &self.executors
    }

    /// Its tool servers.
    #[must_use]
    pub fn mcp(&self) -> &Arc<McpServers> {
        &self.mcp
    }

    /// The host that runs its tool servers.
    #[must_use]
    pub fn mcp_host(&self) -> &Arc<McpHost> {
        &self.mcp_host
    }

    /// Its index.
    #[must_use]
    pub fn index(&self) -> &Arc<Index> {
        &self.index
    }

    /// Whether the initial native walk completed; a full-mode index may still be preparing it.
    #[must_use]
    pub fn initial_scan_complete(&self) -> bool {
        self.scanned.load(Ordering::SeqCst)
    }

    /// Its facts as they stand.
    #[must_use]
    pub fn facts(&self) -> EnvironmentFacts {
        lock(&self.facts).clone()
    }

    /// The current attachment generation, without copying its tool/server inventory for a peer lookup.
    #[must_use]
    pub fn generation(&self) -> i64 {
        lock(&self.facts).generation
    }

    /// A turn's watcher cursor, unavailable on live trees or during the first scan.
    #[must_use]
    pub fn file_changes_cursor(&self) -> Option<u64> {
        let cursor = self.index.revision();
        (self.scanned.load(Ordering::SeqCst) && self.index.changes_since(cursor).is_some())
            .then_some(cursor)
    }

    /// File paths observed since a turn's cursor, with no whole-tree walk or git command.
    #[must_use]
    pub fn file_changes_since(&self, cursor: u64) -> Option<Vec<RelPath>> {
        self.index.changes_since(cursor)
    }

    fn follow_index_with_watches(&self) {
        if let Some(watcher) = lock(&self.watcher).as_ref() {
            let git_watched = !self.git_watches.is_empty();
            if (self.index.state().is_full() && self.scanned.load(Ordering::SeqCst)) || git_watched
            {
                watcher.enable_recursive();
            }
            if watcher.is_degraded() {
                self.index.give_up(crate::index::LiveReason::Watches);
            }
            if !self.index.state().is_full() && !git_watched {
                watcher.disable_recursive();
                let root = self.files.root().root_path();
                for watch in lock(&self.watches).values() {
                    watcher.watch_dir(&watch.dir.under(root));
                }
                for (capture, _) in lock(&self.directory_captures).iter() {
                    watcher.watch_dir(&capture.path.under(root));
                }
            }
        }
    }

    /// What the executors of the environment are told of it.
    #[must_use]
    pub fn context(&self) -> EnvironmentContext {
        let facts = self.facts();
        let settings = self.host.upgrade().map(|host| host.settings());
        let (tools, _unit_limits, tools_dir, data_dir) = match &settings {
            Some(settings) => (
                settings.tools.clone(),
                settings.unit_limits,
                settings
                    .managed
                    .as_ref()
                    .and_then(|managed| managed.tools_dir.as_ref())
                    .map(|path| path.to_string_lossy().into_owned()),
                settings
                    .data_dir
                    .join("exec")
                    .join(self.id.to_string())
                    .to_string_lossy()
                    .into_owned(),
            ),
            None => (Vec::new(), Resources::default(), None, String::new()),
        };
        EnvironmentContext {
            process_launcher: self.processes.launcher(),
            environment_id: self.id,
            generation: facts.generation,
            data_dir,
            workspace_root: facts.workspace_root.clone(),
            home: Some(facts.home.clone()).filter(|home| !home.is_empty()),
            tools_dir,
            tools,
            mounts: self.mounts.roots(),
            user: facts.user.clone(),
            shell: orch_agent_contract::facts::ShellInfo {
                name: facts.shell.name.clone(),
                path: facts.shell.path.clone(),
            },
            os: match facts.os {
                Os::Linux => orch_agent_contract::facts::Os::Linux,
                Os::Macos => orch_agent_contract::facts::Os::Macos,
                Os::Windows => orch_agent_contract::facts::Os::Windows,
            },
            arch: match facts.arch {
                Arch::X86_64 => orch_agent_contract::facts::Arch::X86_64,
                Arch::Aarch64 => orch_agent_contract::facts::Arch::Aarch64,
            },
            path_style: match facts.path_style {
                PathStyle::Posix => orch_agent_contract::facts::PathStyle::Posix,
                PathStyle::Windows => orch_agent_contract::facts::PathStyle::Windows,
            },
            limits: self.resources,
            variables: self.variables.clone(),
            mcp_servers_enabled: self.mcp.any_enabled(),
        }
    }

    fn inner(&self) -> Result<Arc<HostInner>> {
        self.host
            .upgrade()
            .ok_or_else(|| HostError::Internal("the host is gone".to_owned()))
    }

    /// The files a command names: a mount of the environment, or its workspace.
    fn files_of(&self, mount: Option<&str>) -> Result<&Files> {
        match mount {
            Some(name) => self.mounts.files(name),
            None => Ok(&self.files),
        }
    }

    /// Reads the facts again from what is attached now and keeps them.
    async fn refresh_facts(&self) -> EnvironmentFacts {
        let context = self.context();
        let executors = self.executors.facts(&context).await;
        let mut facts = lock(&self.facts);
        facts.executors = executors;
        facts.mcp_found = self.mcp.found();
        facts.mcp_servers = self
            .mcp
            .list()
            .into_iter()
            .map(|server| {
                let (state, reason) = match server.state {
                    ServerState::Starting => ("starting", None),
                    ServerState::Running { .. } => ("running", None),
                    ServerState::Stopped => ("stopped", None),
                    ServerState::Failed { reason } => ("failed", Some(reason)),
                };
                orch_link_contract::events::EnvironmentFactsMcpServer {
                    server_id: server.server_id,
                    name: server.name,
                    state: state.into(),
                    reason,
                    revision: server.revision,
                }
            })
            .collect();
        facts.index = self.index.state().mode();
        facts.index_reason = self.index.state().reason();
        facts.mounts = self.mounts.facts();
        facts.clone()
    }

    /// Reads the facts again and, if they differ from what was last said, says them.
    async fn announce_facts(&self) {
        let before = self.facts();
        let after = self.refresh_facts().await;
        if before != after
            && let Ok(host) = self.inner()
        {
            host.reports
                .say(self.id, Report::Facts(Box::new(after)))
                .await;
        }
    }

    // ------------------------------------------------------------------ the watch and the index

    /// Native evidence is admitted before debounce, Git, index refresh or visible directory reads.
    fn admit_directory_change(&self, change: &Change) {
        let Ok(host) = self.inner() else { return };
        match change {
            Change::Paths(paths) => {
                let mut rules = false;
                for path in paths
                    .iter()
                    .filter(|path| path.file_name() == Some(crate::ignore::IGNORE_FILE))
                {
                    self.rules
                        .forget(&path.parent().unwrap_or_else(RelPath::root));
                    rules = true;
                }
                host.directory_pages
                    .invalidate(self.id, self.directory_attachment, paths, rules);
            }
            Change::Rescan | Change::Unavailable => {
                self.rules.forget_all();
                host.directory_pages
                    .invalidate(self.id, self.directory_attachment, &[], true);
            }
        }
    }

    /// Turns what the watch says into the state of the index, the ignore rules and the directories
    /// somebody has open.
    async fn changed(self: &Arc<Self>, change: Change) {
        let directory_loss = matches!(&change, Change::Rescan | Change::Unavailable);
        let (paths, mut rescan) = match change {
            Change::Paths(paths) => (paths, false),
            Change::Rescan => {
                self.index.lost_events();
                self.invalidate_directory_views().await;
                (Vec::new(), true)
            }
            Change::Unavailable => {
                self.invalidate_directory_views().await;
                self.git_watches.changed(self, &[], true).await;
                if let Some(states) = &self.file_states {
                    states.invalidate("live_tree");
                }
                self.index.give_up(crate::index::LiveReason::Watches);
                self.follow_index_with_watches();
                self.announce_facts().await;
                return;
            }
        };
        // Git shares the native batch and only rereads repositories that batch touched.
        self.git_watches.changed(self, &paths, rescan).await;
        self.index.observe(&paths);
        // A different list of what to leave out can change what any path is: the tree is walked again.
        for path in paths
            .iter()
            .filter(|path| path.file_name() == Some(crate::ignore::IGNORE_FILE))
        {
            self.rules
                .forget(&path.parent().unwrap_or_else(RelPath::root));
            rescan = true;
            self.index.lost_events();
        }
        // A tree that was never walked to the end has nothing to bring up to date.
        if let Some(states) = &self.file_states {
            if rescan {
                states.invalidate("not_ready");
            } else {
                states.changed(&paths);
            }
        }
        let work = {
            let environment = Arc::clone(self);
            let paths = paths.clone();
            blocking(move || {
                if !environment.index.state().is_full() {
                    return Ok(None);
                }
                let outcome = if rescan {
                    environment
                        .index
                        .scan(environment.files.root(), &environment.rules)?
                } else {
                    environment.index.refresh(
                        environment.files.root(),
                        &environment.rules,
                        &paths,
                    )?
                };
                if let Some(states) = &environment.file_states {
                    let complete = environment
                        .index
                        .changes_since(environment.index.revision())
                        .is_some();
                    states.refresh(
                        &environment.files,
                        &environment.index,
                        if rescan { None } else { Some(paths.as_slice()) },
                        complete,
                    )?;
                }
                Ok(Some(outcome))
            })
        };
        match work.await {
            Ok(Some(outcome)) => {
                if rescan {
                    self.scanned.store(true, Ordering::SeqCst);
                }
                if outcome.went_live {
                    self.follow_index_with_watches();
                    self.announce_facts().await;
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "the index could not be brought up to date"),
        }
        if !directory_loss {
            self.notify_directories(&paths, rescan).await;
        }
    }

    /// Prepares requested file-state baselines off the path that records or starts a turn.
    fn seed_file_states(self: &Arc<Self>) {
        let environment = Arc::clone(self);
        let task = tokio::spawn(async move {
            if let Err(error) = environment.prepare_file_states().await {
                tracing::warn!(%error, environment=%environment.id, "recorded file coverage could not be prepared");
            }
        });
        lock(&self.tasks).push(task);
    }

    /// Completes preparation from the existing index, without a directory walk or Git process.
    async fn prepare_file_states(self: &Arc<Self>) -> Result<()> {
        let Some(states) = self.file_states.clone() else {
            return Ok(());
        };
        if !self.index.state().is_full() {
            states.invalidate("live_tree");
            return Ok(());
        }
        if !self.initial_scan_complete() {
            return Ok(());
        }
        let environment = Arc::clone(self);
        blocking(move || {
            let complete = environment
                .index
                .changes_since(environment.index.revision())
                .is_some();
            states.refresh(&environment.files, &environment.index, None, complete)
        })
        .await
    }

    /// A lost native baseline is an explicit invalidation, without rescanning it or inventing rows.
    async fn invalidate_directory_views(&self) {
        // Admission already invalidated selectors. Keep presentation behind the initial-reset gate.
        let _reading = self.directory_reads.lock().await;
        let lost: Vec<_> = {
            let mut watches = lock(&self.watches);
            watches
                .iter_mut()
                .filter_map(|(id, watch)| {
                    if watch.invalidated {
                        return None;
                    }
                    watch.invalidated = true;
                    Some((*id, watch.wire_instance, watch.truncated))
                })
                .collect()
        };
        let Ok(host) = self.inner() else { return };
        for (watch_id, watch_instance, truncated) in lost {
            host.reports
                .say(
                    self.id,
                    Report::FsChanged(FsChanged {
                        watch_id,
                        watch_instance,
                        page: None,
                        invalidated: true,
                        reset: false,
                        truncated,
                        changed: Vec::new(),
                        removed: Vec::new(),
                    }),
                )
                .await;
        }
    }

    /// Looks again at every watched directory a change touched and says what differs.
    async fn notify_directories(&self, paths: &[RelPath], rescan: bool) {
        let _reading = self.directory_reads.lock().await;
        let touched: Vec<(Id, Id, RelPath)> = lock(&self.watches)
            .iter()
            .filter(|(_, watch)| {
                rescan
                    || paths.iter().any(|path| {
                        *path == watch.dir
                            || path.parent().is_some_and(|parent| parent == watch.dir)
                    })
            })
            .map(|(id, watch)| (*id, watch.instance, watch.shown.clone()))
            .collect();
        let Ok(host) = self.inner() else { return };
        for (watch_id, instance, dir) in touched {
            // A partial directory has no complete diff baseline. Invalidate it even when
            // the event lies outside its first page, without rebuilding a giant snapshot.
            let uncertain = lock(&self.watches)
                .get(&watch_id)
                .filter(|watch| watch.instance == instance)
                .map(|watch| (watch.truncated || watch.invalidated, watch.truncated));
            let files = self.files.clone();
            let listing = if let Some((true, truncated)) = uncertain {
                Ok(crate::files::Listing {
                    entries: Vec::new(),
                    truncated,
                })
            } else {
                blocking(move || files.list(&dir)).await
            };
            let (entries, truncated) = match listing {
                Ok(listing) => (listing.entries, listing.truncated),
                // A directory that is gone has nothing in it any more.
                Err(HostError::NotFound(_)) => (Vec::new(), false),
                Err(_) => continue,
            };
            let now: BTreeMap<String, Signature> = entries
                .iter()
                .map(|entry| (entry.name.clone(), signature_of(entry)))
                .collect();
            let (changed, removed, truncation_changed, wire_instance, invalidated) = {
                let mut watches = lock(&self.watches);
                let Some(watch) = watches
                    .get_mut(&watch_id)
                    .filter(|watch| watch.instance == instance)
                else {
                    continue;
                };
                let changed: Vec<DirEntry> = entries
                    .iter()
                    .filter(|entry| watch.last.get(&entry.name) != now.get(&entry.name))
                    .cloned()
                    .collect();
                let removed: Vec<String> = watch
                    .last
                    .keys()
                    .filter(|name| !now.contains_key(*name))
                    .cloned()
                    .collect();
                let invalidated = watch.invalidated || watch.truncated || truncated;
                if !invalidated {
                    watch.last = now;
                }
                watch.invalidated = invalidated;
                let truncation_changed = watch.truncated != truncated;
                watch.truncated = truncated;
                (
                    if invalidated { Vec::new() } else { changed },
                    if invalidated { Vec::new() } else { removed },
                    truncation_changed,
                    watch.wire_instance,
                    invalidated,
                )
            };
            if changed.is_empty() && removed.is_empty() && !truncation_changed && !invalidated {
                continue;
            }
            let Ok(changed) = changed
                .iter()
                .map(wire::<_, orch_link_contract::foreign::DirEntry>)
                .collect::<Result<Vec<_>>>()
            else {
                continue;
            };
            host.reports
                .say(
                    self.id,
                    Report::FsChanged(FsChanged {
                        watch_id,
                        watch_instance: wire_instance,
                        page: None,
                        invalidated,
                        reset: false,
                        truncated,
                        changed,
                        removed,
                    }),
                )
                .await;
        }
    }

    // ------------------------------------------------------------------ manifests

    /// Makes the manifest of the whole tree from the index, uploads it and says so. Only a tree
    /// whose index was walked to the end has one; `None` for any other.
    pub async fn publish_manifest(&self, sink: &dyn BlobSink) -> Result<Option<EnvManifest>> {
        if !self.index.state().is_full() || !self.scanned.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let host = self.inner()?;
        // Changes that arrive while the list is made leave the tree marked as changed.
        self.index.take_dirty();
        match self.upload_manifest(&host, sink).await {
            Ok(report) => {
                self.publisher.published();
                host.reports.say(self.id, Report::Manifest(report)).await;
                Ok(Some(report))
            }
            Err(error) => {
                self.index.mark_dirty();
                Err(error)
            }
        }
    }

    async fn upload_manifest(&self, host: &HostInner, sink: &dyn BlobSink) -> Result<EnvManifest> {
        let generation = self.facts().generation;
        let files = self.files.clone();
        let index = Arc::clone(&self.index);
        let now = host.clock.now();
        let built = blocking(move || {
            let entries = manifest::entries_of_index(&index, files.hashes(), files.root());
            let header = manifest::header_for(files.root(), &RelPath::root(), generation, now);
            manifest::build(&header, entries)
        })
        .await?;
        put_bytes(sink, built.hash, built.bytes).await?;
        Ok(EnvManifest {
            manifest_blob: built.hash,
            file_count: built.file_count,
            total_bytes: built.total_bytes,
        })
    }

    // ------------------------------------------------------------------ measurements

    /// Starts the periodic reports that the screens asked for, instead of those running.
    fn start_reporters(self: &Arc<Self>) {
        let wanted = *lock(&self.wanted);
        let mut reporters = lock(&self.reporters);
        for task in reporters.drain(..) {
            task.abort();
        }
        let Some(wanted) = wanted else { return };
        let weak = Arc::downgrade(self);
        if wanted.stats {
            reporters.push(tokio::spawn(report_loop(
                weak.clone(),
                wanted.interval,
                Measure::Stats,
            )));
        }
        if wanted.procs {
            reporters.push(tokio::spawn(report_loop(
                weak.clone(),
                wanted.interval,
                Measure::Procs,
            )));
        }
        if wanted.ports {
            reporters.push(tokio::spawn(report_ports(weak.clone())));
        }
        if wanted.terminals {
            reporters.push(tokio::spawn(report_terminals(weak, wanted.interval)));
        }
    }

    fn stop_tasks(&self) {
        for task in lock(&self.tasks).drain(..) {
            task.abort();
        }
        for task in lock(&self.reporters).drain(..) {
            task.abort();
        }
        if let Some(watcher) = lock(&self.watcher).take() {
            watcher.stop();
        }
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        self.stop_tasks();
    }
}

/// Puts `bytes`, whose hash is `hash`, into the store unless it holds them already.
async fn put_bytes(sink: &dyn BlobSink, hash: Hash, bytes: Vec<u8>) -> Result<()> {
    let lacking = sink
        .missing(&[hash])
        .await
        .map_err(|error| HostError::Content(error.to_string()))?;
    if !lacking.is_empty() {
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        let mut reader = MemoryReader::new(bytes);
        sink.put(hash, size, &mut reader)
            .await
            .map_err(|error| HostError::Content(error.to_string()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The host

struct HostInner {
    #[cfg(all(test, target_os = "linux"))]
    before_record_pin: Mutex<Option<Arc<recording_admission_tests::ReadGate>>>,
    settings: RwLock<HostSettings>,
    clock: Arc<dyn Clock>,
    reports: Reports,
    directory_pages: Arc<crate::directory_pages::DirectoryPages>,
    blobs: Arc<dyn BlobSink>,
    gates: Arc<GateState>,
    executors: Vec<ExecutorFactory>,
    envs: Mutex<HashMap<Id, Arc<Environment>>>,
    retiring: Mutex<HashMap<Id, Arc<retirement::Finalizer>>>,
}

impl HostInner {
    fn settings(&self) -> HostSettings {
        self.settings
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The environment role: every environment attached to this connector.
#[derive(Clone)]
pub struct EnvHost {
    inner: Arc<HostInner>,
}

impl std::fmt::Debug for EnvHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvHost")
            .field("environments", &lock(&self.inner.envs).len())
            .finish_non_exhaustive()
    }
}

impl EnvHost {
    /// The host of a connector. `blobs` is the end of the content transfer for what the host sends
    /// on its own account (a manifest it publishes, what a shell printed last); the commands that
    /// move content are given theirs per call. `executors` make the executors each environment can
    /// run.
    #[must_use]
    pub fn new(
        settings: HostSettings,
        clock: Arc<dyn Clock>,
        reports: Reports,
        blobs: Arc<dyn BlobSink>,
        executors: Vec<ExecutorFactory>,
    ) -> Self {
        let gates = Arc::new(GateState::new(settings.gates));
        let directory_pages = Arc::new(crate::directory_pages::DirectoryPages::new(
            settings.data_dir.join(".orch-tmp-directory-pages"),
            Arc::clone(&clock),
        ));
        Self {
            inner: Arc::new(HostInner {
                #[cfg(all(test, target_os = "linux"))]
                before_record_pin: Mutex::new(None),
                settings: RwLock::new(settings),
                clock,
                reports,
                directory_pages,
                blobs,
                gates,
                executors,
                envs: Mutex::new(HashMap::new()),
                retiring: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The environment attached under `id`.
    pub fn environment(&self, id: Id) -> Result<Arc<Environment>> {
        lock(&self.inner.envs)
            .get(&id)
            .cloned()
            .ok_or_else(|| HostError::NotFound(format!("environment {id} is not attached here")))
    }

    /// Every environment that is attached.
    #[must_use]
    pub fn environments(&self) -> Vec<Arc<Environment>> {
        lock(&self.inner.envs).values().cloned().collect()
    }

    /// Replaces what the machine's owner has switched on, as `machine.policy.set` does.
    pub fn set_policy(
        &self,
        gates: Gates,
        allowed_roots: Option<Vec<PathBuf>>,
        exec_offers: Option<Vec<String>>,
        mcp: McpGates,
    ) {
        self.inner.gates.set(gates);
        {
            let mut settings = self
                .inner
                .settings
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            settings.gates = gates;
            settings.allowed_roots = allowed_roots;
            settings.exec_offers_allowed = exec_offers;
            settings.mcp = mcp;
        }
        for environment in self.environments() {
            environment.mcp.set_gates(mcp);
            let browsers = Arc::clone(&environment.browsers);
            let displays = Arc::clone(&environment.displays);
            tokio::spawn(async move {
                displays.set_allowed(gates.vnc).await;
            });
            tokio::spawn(async move {
                browsers.set_allowed(gates.browser).await;
            });
        }
    }

    /// Says that a turn of a thread in the environment completed, so that a manifest is due if the
    /// tree changed.
    pub fn turn_completed(&self, environment_id: Id) {
        if let Ok(environment) = self.environment(environment_id) {
            environment.publisher.turn_completed();
        }
    }

    /// Publishes a manifest now if the tree changed, as before going offline.
    pub async fn before_offline(&self) {
        for environment in self.environments() {
            if environment.index.is_dirty() {
                let _ = environment
                    .publish_manifest(self.inner.blobs.as_ref())
                    .await;
            }
        }
    }

    // ------------------------------------------------------------------ attach and detach

    /// `env.attach`: attaches the environment, or updates it if it is attached already, and
    /// answers with its facts. `secrets` are the values of `body.secrets`, unsealed by the caller.
    ///
    /// An update keeps what runs in the environment (shells, processes, executors, tool servers,
    /// directories that are watched, the measurements asked for) and takes the new ignore
    /// patterns, mounts and variables.
    pub async fn env_attach(
        &self,
        body: &EnvAttach,
        secrets: &BTreeMap<String, String>,
    ) -> Result<EnvironmentFacts> {
        retirement::require_room(&lock(&self.inner.retiring), body.environment_id)?;
        let settings = self.inner.settings();
        let previous = lock(&self.inner.envs).get(&body.environment_id).cloned();
        let Attached {
            environment,
            first_scan,
            lost_shells,
        } = attach(&self.inner, &settings, body, secrets, previous.as_ref()).await?;
        let (cleanup, start) = {
            let mut environments = lock(&self.inner.envs);
            let mut retiring = lock(&self.inner.retiring);
            retirement::require_room(&retiring, body.environment_id)?;
            if let Some(previous) = environments.get(&body.environment_id) {
                previous.directory_operations.retire();
            }
            if let Some(previous) = environments.get(&body.environment_id)
                && let Some(states) = &previous.file_states
            {
                states.store().retire_attachment();
            }
            self.inner.directory_pages.forget(body.environment_id);
            if let Some(old) = environments.insert(body.environment_id, Arc::clone(&environment)) {
                let (cleanup, start) = retirement::Finalizer::launch(
                    &self.inner,
                    old,
                    retirement::Kind::Replacement,
                    None,
                );
                retiring.insert(body.environment_id, Arc::clone(&cleanup));
                (Some(cleanup), Some(start))
            } else {
                (None, None)
            }
        };
        start_background(&environment, &settings, first_scan);
        if let Some(start) = start {
            let _ = start.send(());
        }
        if let Some(cleanup) = cleanup {
            cleanup.wait().await?;
        }
        // Shells that were open when the connector last stopped went with it: each is reported as
        // ended, with no exit code, so that nobody waits for one that is gone.
        for terminal_id in lost_shells {
            let ended = PtyExited {
                terminal_id,
                exit_code: None,
                bytes_out: 0,
                kept: None,
            };
            self.inner
                .reports
                .say(environment.id, Report::PtyExited(ended))
                .await;
        }
        Ok(environment.facts())
    }

    /// `env.detach`: stops what the environment runs and forgets it. Detaching what is not
    /// attached is not an error: the command may be sent again.
    pub async fn env_detach(&self, body: &EnvDetach) -> Result<()> {
        let reservation = self.reserve_detach(body.environment_id)?;
        self.detach_reserved(&reservation).await
    }

    /// Fences later recorded readers before any hook or destructive work is admitted.
    pub fn reserve_detach(&self, id: Id) -> Result<DetachReservation> {
        let environments = lock(&self.inner.envs);
        let retiring = lock(&self.inner.retiring);
        if let Some(cleanup) = retiring.get(&id) {
            if cleanup.kind == retirement::Kind::Detach && !environments.contains_key(&id) {
                return Ok(DetachReservation {
                    id,
                    environment: None,
                    store: None,
                    cleanup: Some(Arc::clone(cleanup)),
                });
            }
            return Err(HostError::Busy(
                "the previous attachment is still being closed; try again after it finishes"
                    .to_owned(),
            ));
        }
        let environment = environments.get(&id).cloned();
        let store = environment
            .as_ref()
            .and_then(|environment| environment.file_states.as_ref())
            .map(|states| states.store().reserve_destructive().map(Arc::new))
            .transpose()?;
        Ok(DetachReservation {
            id,
            environment,
            store,
            cleanup: None,
        })
    }

    /// Removes only the reserved attachment, permanently fencing its older store clones.
    pub async fn detach_reserved(&self, reservation: &DetachReservation) -> Result<()> {
        if let Some(cleanup) = &reservation.cleanup {
            return cleanup.wait().await;
        }
        let Some(environment) = &reservation.environment else {
            return Ok(());
        };
        let (cleanup, start) = {
            let mut environments = lock(&self.inner.envs);
            let mut retiring = lock(&self.inner.retiring);
            if let Some(cleanup) = retiring.get(&reservation.id) {
                if cleanup.attachment == environment.directory_attachment {
                    // A matching old reservation joins only that old finalizer, even when a
                    // replacement is already live. Never resolve its target through the new map.
                    (Arc::clone(cleanup), None)
                } else {
                    return Err(HostError::Changed(
                        "the environment attachment changed before detach".to_owned(),
                    ));
                }
            } else {
                let Some(current) = environments.get(&reservation.id) else {
                    return Ok(());
                };
                if !Arc::ptr_eq(current, environment) {
                    return Err(HostError::Changed(
                        "the environment attachment changed before detach".to_owned(),
                    ));
                }
                retirement::require_room(&retiring, reservation.id)?;
                environment.directory_operations.retire();
                if let Some(store) = &reservation.store {
                    store.retire();
                }
                environments.remove(&reservation.id);
                self.inner
                    .directory_pages
                    .forget_attachment(reservation.id, environment.directory_attachment);
                let (cleanup, start) = retirement::Finalizer::launch(
                    &self.inner,
                    Arc::clone(environment),
                    retirement::Kind::Detach,
                    reservation.store.clone(),
                );
                retiring.insert(reservation.id, Arc::clone(&cleanup));
                (cleanup, Some(start))
            }
        };
        if let Some(start) = start {
            let _ = start.send(());
        }
        cleanup.wait().await
    }

    /// Observes pins for refusal precedence; destructive detach/import uses an owned reservation.
    pub fn require_recording_idle(&self, id: Id) -> Result<()> {
        let environment = { lock(&self.inner.envs).get(&id).cloned() };
        if let Some(environment) = environment
            && let Some(states) = &environment.file_states
        {
            states.store().require_idle()?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ files

    /// Accepts a port read and emits its fresh observation only as a live report.
    pub async fn ports_look(
        &self,
        body: &orch_link_contract::ports::PortsLook,
    ) -> Result<orch_link_contract::ports::PortsLookResult> {
        let environment = self.environment(body.environment_id)?;
        if environment.facts().generation != body.generation {
            return Err(HostError::Changed(
                "the environment generation changed".to_owned(),
            ));
        }
        let reader = environment.ports().clone();
        let snapshot = blocking(move || Ok(reader.look())).await?;
        let current = self.environment(body.environment_id)?;
        if !Arc::ptr_eq(&current, &environment) {
            return Err(HostError::Changed(
                "the attached environment changed during the read".to_owned(),
            ));
        }
        self.inner
            .reports
            .say(
                body.environment_id,
                Report::Ports(orch_link_contract::ports::PortsSnapshot {
                    request_ref: Some(body.request_ref),
                    snapshot,
                }),
            )
            .await;
        Ok(orch_link_contract::ports::PortsLookResult {
            accepted: orch_link_contract::commands::True,
        })
    }

    /// `fs.list`: lists a directory, and watches it when the command asks.
    pub async fn fs_list(&self, body: &FsList) -> Result<FsListResult> {
        let environment = self.environment(body.environment_id)?;
        if body.watch_id.is_some() && body.mount.is_some() {
            return Err(HostError::Unsupported(
                "a mount is listed when it is asked for and not watched".to_owned(),
            ));
        }
        if body.watch_instance.is_some() && body.watch_id.is_none() {
            return Err(HostError::Unsupported(
                "a directory installation instance needs a watch".to_owned(),
            ));
        }
        if body.watch_id.is_some() && body.page.as_ref().is_some_and(|page| page.cursor.is_some()) {
            return Err(HostError::Unsupported(
                "a watch starts with a fresh snapshot, not an old continuation".to_owned(),
            ));
        }
        let files = environment.files_of(body.mount.as_deref())?.clone();
        let path = files.parse(&body.path)?;
        let requested = body.page.as_ref();
        if requested
            .and_then(|page| page.generation)
            .is_some_and(|generation| generation != environment.generation())
        {
            return Err(HostError::Stale { current: None });
        }
        let owner: crate::directory_pages::Coverage =
            Arc::clone(&environment) as crate::directory_pages::Coverage;
        let mut operation = environment
            .directory_operations
            .begin(&self.inner.directory_pages, Some(owner))
            .await?;
        let cancellation = operation.cancellation();
        let _reading = if body.watch_id.is_some() {
            Some(tokio::select! {
                biased;
                () = cancellation.cancelled() => { operation.check()?; return Err(HostError::Cancelled); }
                reading = tokio::time::timeout(operation.deadline().saturating_duration_since(Instant::now()),
                    environment.directory_reads.lock()) => reading.map_err(|_| HostError::Timeout("the directory metadata read exceeded its command budget".to_owned()))?,
            })
        } else {
            None
        };
        let location = if requested.and_then(|page| page.cursor.as_ref()).is_none() {
            let (files, path) = (files.clone(), path.clone());
            #[cfg(all(test, target_os = "linux"))]
            let testing = Arc::clone(&self.inner.directory_pages.testing);
            Some(
                operation
                    .blocking(move || {
                        #[cfg(all(test, target_os = "linux"))]
                        let _location = testing.before_location()?;
                        files.directory_location(&path)
                    })
                    .await?,
            )
        } else {
            None
        };
        let mut registration = if let Some(watch) = body.watch_id {
            let location = location.as_ref().ok_or_else(|| {
                HostError::Internal("an initial watch needs a directory identity".to_owned())
            })?;
            Some(operation.admit(|| {
                directory_watches::Registration::begin(
                    Arc::clone(&environment),
                    watch,
                    body.watch_instance,
                    path.clone(),
                    location.clone(),
                )
            })?)
        } else {
            None
        };
        let coverage = operation.admit(|| {
            if body.mount.is_none()
                && let Some(location) = location
            {
                match directory_watches::Capture::begin(Arc::clone(&environment), location) {
                    Ok(owner) => Ok(Some(owner as crate::directory_pages::Coverage)),
                    // A plain immutable observation does not pretend native live activation succeeded.
                    Err(HostError::Unsupported(_) | HostError::Busy(_))
                        if body.watch_id.is_none() =>
                    {
                        Ok(None)
                    }
                    Err(error) => Err(error),
                }
            } else {
                Ok(None)
            }
        })?;
        operation.cover(coverage);
        #[cfg(all(test, target_os = "linux"))]
        if registration.is_some() {
            directory_watch_tests::before_snapshot().await;
        }
        let binding = crate::directory_pages::Binding {
            environment: environment.id,
            attachment: environment.directory_attachment,
            generation: Some(environment.generation()),
            mount: body.mount.clone(),
            path: path.as_str().to_owned(),
        };
        let cursor = requested.and_then(|page| page.cursor.as_deref());
        let limit = requested
            .and_then(|page| page.limit)
            .unwrap_or(orch_link_contract::commands::FS_LIST_MAX_ENTRIES);
        let page = self
            .inner
            .directory_pages
            .read_in(
                &operation,
                binding,
                files.clone(),
                path.clone(),
                cursor,
                limit,
            )
            .await?;
        let current = self.environment(body.environment_id)?;
        if !Arc::ptr_eq(&current, &environment) {
            return Err(HostError::Changed(
                "the attached environment changed during the directory read".to_owned(),
            ));
        }
        if let Some(registration) = &registration {
            let (files, path) = (files.clone(), path.clone());
            if operation
                .blocking(move || files.directory_location(&path))
                .await?
                != registration.location
            {
                return Err(HostError::Changed(
                    "the directory changed during its first snapshot".to_owned(),
                ));
            }
        }
        page.validate()?;
        let observation = page.proof();
        let listing = crate::files::Listing {
            truncated: page.next_cursor.is_some(),
            entries: page.entries,
        };
        let metadata = orch_link_contract::commands::FsListPageResult {
            revision: page.revision,
            next_cursor: page.next_cursor,
            as_of: page.as_of,
            observed_from: Some(page.observed_from),
            expires_at: page.expires_at,
        };
        if let Some(registration) = registration.as_mut() {
            operation.check()?;
            registration.baseline(&listing)?;
            let changed = listing
                .entries
                .iter()
                .map(wire)
                .collect::<Result<Vec<_>>>()?;
            #[cfg(all(test, target_os = "linux"))]
            directory_lifecycle_tests::before_delivery().await;
            observation.validate()?;
            operation.check()?;
            self.inner
                .reports
                .say(
                    environment.id,
                    Report::FsChanged(FsChanged {
                        watch_id: registration.id(),
                        watch_instance: body.watch_instance,
                        page: Some(metadata.clone()),
                        invalidated: false,
                        reset: true,
                        truncated: listing.truncated,
                        changed,
                        removed: Vec::new(),
                    }),
                )
                .await;
            observation.validate()?;
            operation.admit(|| registration.commit())?;
        }
        let entries = listing
            .entries
            .iter()
            .map(wire)
            .collect::<Result<Vec<_>>>()?;
        observation.validate()?;
        operation.check()?;
        Ok(FsListResult {
            entries,
            truncated: listing.truncated,
            watch_instance: body.watch_instance,
            page: Some(metadata),
        })
    }

    /// `fs.unwatch`: ends the watch on a directory. A watch that is not there is not an error.
    pub fn fs_unwatch(&self, body: &FsUnwatch) -> Result<()> {
        for environment in self.environments() {
            if environment.git_watches.remove(body.watch_id) {
                environment.follow_index_with_watches();
                return Ok(());
            }
            if directory_watches::unwatch(&environment, body.watch_id) {
                return Ok(());
            }
        }
        Ok(())
    }

    /// `fs.stat`: describes one file or directory.
    pub async fn fs_stat(&self, body: &FsStat) -> Result<FileStat> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files_of(body.mount.as_deref())?.clone();
        let path = files.parse(&body.path)?;
        blocking(move || files.stat(&path)).await
    }

    /// `fs.fetch`: puts a file's content into the store unless the store holds it already.
    pub async fn fs_fetch(&self, body: &FsFetch, sink: &dyn BlobSink) -> Result<FsFetchResult> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files_of(body.mount.as_deref())?;
        let path = files.parse(&body.path)?;
        files.fetch(&path, body.expect, body.max_bytes, sink).await
    }

    /// `fs.write`: writes a file from stored content, if it is still the version the writer saw.
    pub async fn fs_write(&self, body: &FsWrite, source: &dyn BlobSource) -> Result<FileStat> {
        let environment = self.environment(body.environment_id)?;
        let path = environment.files.parse(&body.path)?;
        environment
            .files
            .write(&path, body.blob, body.mode, body.expected_hash, source)
            .await
    }

    /// Makes ready the directory of a thread, as the connector does when a thread starts or its
    /// files are opened: the directory, with its `.orch/` and the file that keeps a repository
    /// around it from listing what Orch keeps there. `directory` is a path from the workspace
    /// root. It writes, so it is refused where files may not be written from outside.
    pub async fn thread_directory(
        &self,
        environment_id: Id,
        directory: &str,
    ) -> Result<ThreadDirectory> {
        let environment = self.environment(environment_id)?;
        let files = environment.files.clone();
        let path = files.target_of(directory)?;
        Files::check_thread_directory(&path)?;
        blocking(move || files.ensure_thread_directory(&path)).await
    }

    /// `fs.mkdir`: makes a directory.
    pub async fn fs_mkdir(&self, body: &FsMkdir) -> Result<FileStat> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files.clone();
        let path = files.parse(&body.path)?;
        if path.as_str() == orch_agent_contract::session::INSTRUCTION_ROOT_MARKER {
            return blocking(move || crate::instruction_root::prepare(files.root(), files.gates()))
                .await;
        }
        blocking(move || files.make_dir(&path)).await
    }

    /// `fs.rename`: renames or moves a file or a directory.
    pub async fn fs_rename(&self, body: &FsRename) -> Result<FileStat> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files.clone();
        let (from, to) = (files.parse(&body.from)?, files.parse(&body.to)?);
        blocking(move || files.rename(&from, &to)).await
    }

    /// `fs.remove`: removes a file or a directory.
    pub async fn fs_remove(&self, body: &FsRemove) -> Result<()> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files.clone();
        let path = files.parse(&body.path)?;
        let recursive = body.recursive;
        blocking(move || files.remove(&path, recursive)).await
    }

    /// `fs.copy`: copies a file, or a directory with what is in it.
    pub async fn fs_copy(&self, body: &FsCopy) -> Result<FileStat> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files.clone();
        let (from, to) = (files.parse(&body.from)?, files.parse(&body.to)?);
        blocking(move || files.copy(&from, &to)).await
    }

    /// `fs.archive.list`: lists what an archive holds, without unpacking it.
    pub async fn fs_archive_list(
        &self,
        body: &FsArchiveList,
    ) -> Result<orch_api_contract::workspace::ArchiveListing> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files.clone();
        let path = files.parse(&body.path)?;
        let inside = body.inside.clone();
        let limit = usize::try_from(body.limit)
            .unwrap_or(usize::MAX)
            .min(orch_api_contract::workspace::ARCHIVE_LIST_LIMIT_MAX as usize);
        blocking(move || files.archive_list(&path, inside.as_deref(), limit)).await
    }

    /// `fs.archive.read`: uploads one entry of an archive for a viewer.
    pub async fn fs_archive_read(
        &self,
        body: &FsArchiveRead,
        sink: &dyn BlobSink,
    ) -> Result<FsArchiveReadResult> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files.clone();
        let path = files.parse(&body.path)?;
        let entry = body.entry.clone();
        let bytes = blocking(move || files.archive_read(&path, &entry)).await?;
        let hash = Hash::of(&bytes);
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        put_bytes(sink, hash, bytes).await?;
        Ok(FsArchiveReadResult {
            blob: BlobRef { hash, size },
        })
    }

    /// `fs.search`: searches the text of the files under a directory.
    pub async fn fs_search(&self, body: &FsSearch) -> Result<FsSearchResult> {
        let environment = self.environment(body.environment_id)?;
        let files = environment.files.clone();
        let path = files.parse(&body.path)?;
        let request = SearchRequest {
            query: body.query.clone(),
            regex: body.regex,
            case_sensitive: body.case,
            glob: body.glob.clone(),
            limit: usize::try_from(body.limit).unwrap_or(usize::MAX),
        };
        let found = blocking(move || files.search(&path, &request)).await?;
        let matches = found.matches.iter().map(wire).collect::<Result<Vec<_>>>()?;
        Ok(FsSearchResult {
            matches,
            truncated: found.truncated,
            searched: found.searched,
        })
    }

    /// Refuses a request for the whole of a tree that is not indexed: such a tree is listed, and
    /// snapshots and manifests of it name the paths they want.
    fn require_selection(environment: &Environment, selection: &Selection) -> Result<()> {
        let whole =
            selection.include.is_empty() || selection.include.iter().any(|path| path.is_empty());
        if whole && !environment.index.state().is_full() {
            return Err(HostError::Unsupported(
                "this tree is too large to be taken whole: name the paths to take".to_owned(),
            ));
        }
        Ok(())
    }

    /// `fs.manifest`: lists a tree as it is now and uploads the list. The list of the whole tree,
    /// with nothing left out, is also what the environment says of itself: it is reported as the
    /// environment's manifest, and the tree counts as published.
    pub async fn fs_manifest(
        &self,
        body: &FsManifest,
        sink: &dyn BlobSink,
    ) -> Result<FsManifestResult> {
        let environment = self.environment(body.environment_id)?;
        Self::require_selection(&environment, &body.selection)?;
        let files = environment.files.clone();
        let selection = body.selection.clone();
        let generation = environment.facts().generation;
        let now = self.inner.clock.now();
        let whole = manifest::resolve(files.root(), &selection)?.is_whole_tree();
        if whole {
            // Changes that arrive while the list is made leave the tree marked as changed.
            environment.index.take_dirty();
        }
        let built = blocking(move || {
            let resolved = manifest::resolve(files.root(), &selection)?;
            let entries = manifest::collect(&files, &resolved)?;
            let header = manifest::header_for(files.root(), &resolved.root, generation, now);
            manifest::build(&header, entries)
        })
        .await;
        let built = match built {
            Ok(built) => built,
            Err(error) => {
                if whole {
                    environment.index.mark_dirty();
                }
                return Err(error);
            }
        };
        let (hash, file_count, total_bytes) = (built.hash, built.file_count, built.total_bytes);
        if let Err(error) = put_bytes(sink, hash, built.bytes).await {
            if whole {
                environment.index.mark_dirty();
            }
            return Err(error);
        }
        if whole {
            environment.publisher.published();
            let report = EnvManifest {
                manifest_blob: hash,
                file_count,
                total_bytes,
            };
            self.inner
                .reports
                .say(environment.id, Report::Manifest(report))
                .await;
        }
        Ok(FsManifestResult {
            manifest_blob: hash,
            file_count,
            total_bytes,
        })
    }

    /// `fs.measure`: a bounded selection preflight, without hashes, uploads or a job.
    pub async fn fs_measure(
        &self,
        body: &orch_link_contract::commands::FsMeasure,
    ) -> Result<orch_link_contract::commands::FsMeasureResult> {
        let environment = self.environment(body.environment_id)?;
        // Invalid patterns are still errors; a whole live tree gets a reviewable refusal.
        manifest::resolve(environment.files.root(), &body.selection)?;
        if Self::require_selection(&environment, &body.selection).is_err() {
            return Ok(crate::measure::refused(
                orch_link_contract::commands::SelectionRefusal::SelectionRequired,
            ));
        }
        let files = environment.files.clone();
        let selection = body.selection.clone();
        blocking(move || crate::measure::selection(&files, &selection)).await
    }

    /// `fs.snapshot`: lists a tree, hashes it and uploads what the store lacks and the list.
    pub async fn fs_snapshot(
        &self,
        body: &FsSnapshot,
        sink: &dyn BlobSink,
        progress: &dyn Progress,
    ) -> Result<FsSnapshotResult> {
        let environment = self.environment(body.environment_id)?;
        Self::require_selection(&environment, &body.selection)?;
        let generation = environment.facts().generation;
        let measured = self
            .fs_measure(&orch_link_contract::commands::FsMeasure {
                environment_id: body.environment_id,
                selection: body.selection.clone(),
            })
            .await?;
        if !measured.complete {
            return Err(HostError::Unsupported(format!(
                "the snapshot selection must be narrowed: {:?}",
                measured.refusal
            )));
        }
        let report = environment
            .files
            .snapshot(
                &body.selection,
                generation,
                self.inner.clock.as_ref(),
                sink,
                progress,
            )
            .await?;
        Ok(report.result)
    }

    /// `fs.materialize`: writes the files of a list into a directory.
    pub async fn fs_materialize(
        &self,
        body: &FsMaterialize,
        source: &dyn BlobSource,
        progress: &dyn Progress,
    ) -> Result<FsMaterializeResult> {
        let environment = self.environment(body.environment_id)?;
        if body.purpose == orch_link_contract::commands::MaterializePurpose::Exact {
            environment.files.gates().require_remote_write()?;
            let states = environment.file_states.as_ref().ok_or_else(|| {
                HostError::Unsupported("the durable exact-file store is unavailable".into())
            })?;
            if let Some(result) =
                crate::exact_materialize::completed(states, body, environment.facts().generation)?
            {
                return Ok(result);
            }
        }
        let mut writer = MemoryWriter::with_limit(MANIFEST_DOWNLOAD_MAX_BYTES);
        download_checked(source, body.manifest_blob, &mut writer)
            .await
            .map_err(|e| HostError::Content(format!("the list of files could not be had: {e}")))?;
        let parsed = manifest::parse(&writer.into_bytes())?;
        if body.purpose == orch_link_contract::commands::MaterializePurpose::Exact {
            let states = environment.file_states.as_ref().ok_or_else(|| {
                HostError::Unsupported("the durable exact-file store is unavailable".into())
            })?;
            return crate::exact_materialize::apply(
                &environment.files,
                states,
                body,
                &parsed,
                source,
                environment.facts().generation,
            )
            .await;
        }
        let report = environment
            .files
            .materialize(body.purpose, &body.target_root, &parsed, source, progress)
            .await?;
        Ok(report.result)
    }

    /// `fs.record`: publishes a complete prepared state, or a durable explained fallback.
    pub async fn fs_record(
        &self,
        body: &orch_link_contract::source::FsRecord,
        sink: &dyn BlobSink,
    ) -> Result<orch_link_contract::source::FsRecordResult> {
        let until = self
            .inner
            .clock
            .now()
            .checked_add_signed(chrono::Duration::seconds(i64::from(
                orch_link_contract::commands::CommandKind::FsRecord.deadline_secs(),
            )))
            .ok_or_else(|| {
                HostError::Unsupported("the recording deadline is invalid".to_owned())
            })?;
        self.fs_record_capture(
            body,
            orch_types::new_id(),
            Time::new(until),
            tokio_util::sync::CancellationToken::new(),
        )
        .await?
        .publish(sink)
        .await
    }

    /// Seals local state, its immutable upload source and retention before the serial key is released.
    /// The command owner signals cancellation and joins this future; it must not detach blocking work.
    pub async fn fs_record_capture(
        &self,
        body: &orch_link_contract::source::FsRecord,
        operation: Id,
        until: Time,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<crate::file_states::CapturedState> {
        if cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        let environment = self.environment(body.environment_id)?;
        let Some(states) = environment.file_states.clone() else {
            return Ok(crate::file_states::CapturedState {
                result: crate::file_states::unavailable("store_unavailable"),
                input: None,
                hold: None,
            });
        };
        if !environment.index.state().is_full() {
            states.invalidate("live_tree");
        } else if environment
            .index
            .changes_since(environment.index.revision())
            .is_none()
        {
            states.invalidate("not_ready");
        }
        let directory = environment.files.parse(&body.directory)?;
        let now = self.inner.clock.time();
        let generation = environment.facts().generation;
        let request = body.clone();
        let kept = states.clone();
        let cached = blocking(move || kept.store().recorded_reply(&request)).await?;
        if cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        if cached.is_none() {
            let needs_seed = states.request(&directory, body.max_bytes);
            if body.record_days > 0
                && body.max_bytes > 0
                && !states.store().has_root(body.snapshot_id)
                && (needs_seed || states.needs_preparing(&directory))
            {
                // A new directory and a re-enabled policy need their indexed baseline before
                // this immutable identity is answered. Otherwise a scheduling race permanently
                // freezes not_ready even though the worker completes a moment later.
                let kept = states.clone();
                let policy = crate::state_store::RecordPolicy {
                    record_days: body.record_days,
                    max_bytes: body.max_bytes,
                };
                blocking(move || kept.store().keep_policy(&directory, policy)).await?;
                environment.prepare_file_states().await?;
            }
        }
        if cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        let files = environment.files.clone();
        let request = body.clone();
        let operation = crate::file_states::RecordOperation {
            id: operation,
            until,
            cancel,
        };
        blocking(move || states.capture(&files, &request, generation, now, &operation)).await
    }

    /// `fs.record.read`: transfers verified captured bytes and the common manifest for a fork.
    pub async fn fs_record_read(
        &self,
        body: &orch_link_contract::source::FsRecordRead,
        operation: Id,
        sink: &dyn BlobSink,
    ) -> Result<orch_link_contract::source::FsRecordReadResult> {
        let environment = self.environment(body.environment_id)?;
        #[cfg(all(test, target_os = "linux"))]
        {
            let gate = { lock(&self.inner.before_record_pin).clone() };
            if let Some(gate) = gate {
                gate.pass().await;
            }
        }
        let states = environment.file_states.clone().ok_or_else(|| {
            HostError::Unsupported("the recorded store is unavailable".to_owned())
        })?;
        let request = body.clone();
        let files = environment.files.clone();
        let index = environment.index.clone();
        let now = self.inner.clock.time();
        let kept = states.clone();
        let read =
            blocking(move || kept.read_recorded(&files, &index, &request, operation, now)).await?;
        for entry in read.entries.iter().filter(|entry| {
            matches!(
                entry.t,
                manifest::EntryKind::File | manifest::EntryKind::Link
            )
        }) {
            if !body.export && body.path.is_none() {
                break;
            }
            let entry = entry.clone();
            let root = read.root.clone();
            let store = states.store().clone();
            let (verified, hash, size) = blocking(move || {
                let verified = store.read_entry(&root, &entry)?;
                let size = i64::try_from(
                    verified
                        .as_file()
                        .metadata()
                        .map_err(|error| HostError::io("looking at recorded bytes", error))?
                        .len(),
                )
                .map_err(|_| HostError::Unsupported("recorded bytes are too large".to_owned()))?;
                let hash = match entry.t {
                    manifest::EntryKind::File => entry.h.ok_or_else(|| {
                        HostError::Unsupported("recorded file hash is missing".to_owned())
                    })?,
                    manifest::EntryKind::Link => Hash::of(
                        entry
                            .l
                            .as_deref()
                            .ok_or_else(|| {
                                HostError::Unsupported("recorded link text is missing".to_owned())
                            })?
                            .as_bytes(),
                    ),
                    _ => {
                        return Err(HostError::Unsupported(
                            "the recorded path is not a file".to_owned(),
                        ));
                    }
                };
                Ok((verified, hash, size))
            })
            .await?;
            let file = verified
                .reopen()
                .map_err(|error| HostError::io("opening verified recorded bytes", error))?;
            let mut reader = crate::blobs::FileReader::new(Arc::new(file));
            sink.put(hash, size, &mut reader)
                .await
                .map_err(|error| HostError::Content(error.to_string()))?;
            if body.path.is_some() {
                return Ok(orch_link_contract::source::FsRecordReadResult { blob: hash, size });
            }
        }
        if body.path.is_some() {
            return Err(HostError::NotFound(
                "the recorded path has no file bytes".to_owned(),
            ));
        }
        let size = i64::try_from(read.manifest.len())
            .map_err(|_| HostError::Unsupported("the recorded manifest is too large".to_owned()))?;
        let mut reader = crate::blobs::MemoryReader::new(read.manifest);
        sink.put(read.root.manifest_blob, size, &mut reader)
            .await
            .map_err(|error| HostError::Content(error.to_string()))?;
        Ok(orch_link_contract::source::FsRecordReadResult {
            blob: read.root.manifest_blob,
            size,
        })
    }

    /// `fs.record.verify`: reads every retained root/object and recovery dependency before source release.
    pub async fn fs_record_verify(
        &self,
        body: &orch_link_contract::source::FsRecordVerify,
    ) -> Result<orch_link_contract::source::FsRecordVerifyResult> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.clone().ok_or_else(|| {
            HostError::Unsupported("the carried file-state store is unavailable".into())
        })?;
        let now = self.inner.clock.time();
        blocking(move || states.verify_kept(now)).await
    }

    fn data_files(
        &self,
        environment: &Environment,
        part: orch_link_contract::source::EnvironmentDataPart,
    ) -> Result<Files> {
        let root = match part {
            orch_link_contract::source::EnvironmentDataPart::Home => {
                PathBuf::from(environment.facts().home)
            }
            orch_link_contract::source::EnvironmentDataPart::FileStates => environment
                .file_states
                .as_ref()
                .ok_or_else(|| {
                    HostError::Unsupported("the recorded-state store is unavailable".into())
                })?
                .store()
                .root()
                .to_owned(),
        };
        if root.as_os_str().is_empty() {
            return Err(HostError::Unsupported(
                "the environment has no declared home".into(),
            ));
        }
        if part == orch_link_contract::source::EnvironmentDataPart::Home {
            let settings = self.inner.settings();
            let (root, _) = open_workspace(&root, settings.allowed_roots.as_deref())?;
            return Ok(crate::data_transfer::at_root(root, &environment.files));
        }
        crate::data_transfer::files(&root, &environment.files)
    }

    /// Measures complete attached data roots before a move, independently from the machine's kind.
    pub async fn fs_data_measure(
        &self,
        body: &orch_link_contract::source::FsDataMeasure,
    ) -> Result<orch_link_contract::source::FsDataMeasureResult> {
        let environment = self.environment(body.environment_id)?;
        let files = self.data_files(&environment, body.part)?;
        let part = body.part;
        let measured = blocking(move || {
            crate::measure::selection(&files, &crate::data_transfer::selection(part))
        })
        .await?;
        if !measured.complete {
            return Err(HostError::Unsupported(
                "the whole data root could not be measured".into(),
            ));
        }
        let recorded = if part == orch_link_contract::source::EnvironmentDataPart::FileStates {
            Some(
                self.fs_record_verify(&orch_link_contract::source::FsRecordVerify {
                    environment_id: body.environment_id,
                })
                .await?,
            )
        } else {
            None
        };
        Ok(orch_link_contract::source::FsDataMeasureResult {
            files: measured.file_count,
            bytes: measured.total_bytes,
            recorded,
        })
    }

    /// Exports a closed root, including raw recorded objects and journals but no connector credentials.
    pub async fn fs_data_export(
        &self,
        body: &orch_link_contract::source::FsDataExport,
        sink: &dyn BlobSink,
        progress: &dyn crate::snapshot::Progress,
    ) -> Result<orch_link_contract::commands::FsSnapshotResult> {
        let environment = self.environment(body.environment_id)?;
        let files = self.data_files(&environment, body.part)?;
        crate::data_transfer::export(
            &files,
            body.part,
            environment.facts().generation,
            self.inner.clock.as_ref(),
            sink,
            progress,
        )
        .await
    }

    /// Writes a checked data snapshot into this candidate's attached generation only.
    pub async fn fs_data_import(
        &self,
        body: &orch_link_contract::source::FsDataImport,
        source: &dyn BlobSource,
        progress: &dyn crate::snapshot::Progress,
    ) -> Result<orch_link_contract::commands::FsMaterializeResult> {
        let environment = self.environment(body.environment_id)?;
        let reservation = environment
            .file_states
            .as_ref()
            .map(|states| states.store().reserve_destructive())
            .transpose()?
            .map(Arc::new);
        let files = self.data_files(&environment, body.part)?;
        let files = match reservation {
            Some(lease) => files.with_recorded_operation(lease),
            None => files,
        };
        let result =
            crate::data_transfer::import(&files, body.part, body.manifest_blob, source, progress)
                .await?;
        if body.part == orch_link_contract::source::EnvironmentDataPart::FileStates {
            self.fs_record_verify(&orch_link_contract::source::FsRecordVerify {
                environment_id: body.environment_id,
            })
            .await?;
        }
        Ok(result)
    }

    /// `fs.record.prune`: releases expired roots while active operations and journals keep theirs.
    pub async fn fs_record_prune(
        &self,
        body: &orch_link_contract::source::FsRecordPrune,
    ) -> Result<orch_link_contract::source::FsRecordPruneResult> {
        let environment = self.environment(body.environment_id)?;
        let Some(states) = environment.file_states.clone() else {
            return Ok(orch_link_contract::source::FsRecordPruneResult { removed: 0 });
        };
        let expired = body.expired_snapshot_ids.clone();
        let now = self.inner.clock.time();
        let removed = blocking(move || states.prune(&expired, now)).await?;
        Ok(orch_link_contract::source::FsRecordPruneResult { removed })
    }

    /// `fs.restore.plan`: uploads the exact complete plan and reports currently tracked writers.
    pub async fn fs_restore_plan(
        &self,
        body: &orch_link_contract::source::FsRestorePlan,
        sink: &dyn BlobSink,
    ) -> Result<orch_link_contract::source::FsRestorePlanResult> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.clone().ok_or_else(|| {
            HostError::Unsupported("the recorded store is unavailable".to_owned())
        })?;
        let files = environment.files.clone();
        let request = body.clone();
        let generation = environment.facts().generation;
        let now = self.inner.clock.time();
        let target = environment.clone();
        let (mut result, bytes) = blocking(move || {
            let (_, mut result) = crate::restore::plan(&files, &states, &request, generation, now)?;
            result.writers_active = target
                .processes
                .list()
                .iter()
                .any(|process| !matches!(process.info.state.as_str(), "T" | "t" | "Z" | "X"));
            let bytes = crate::state_store::read_bounded(
                &states
                    .store()
                    .root()
                    .join("plans")
                    .join(result.plan_blob.to_string()),
                manifest::MANIFEST_MAX_EXPANDED_BYTES,
            )?;
            Ok((result, bytes))
        })
        .await?;
        let size = i64::try_from(bytes.len())
            .map_err(|_| HostError::Unsupported("the restore plan is too large".to_owned()))?;
        let mut reader = crate::blobs::MemoryReader::new(bytes);
        sink.put(result.plan_blob, size, &mut reader)
            .await
            .map_err(|error| HostError::Content(error.to_string()))?;
        // A writer can appear during a slow upload; its latest presence must be shown too.
        let target = environment.clone();
        result.writers_active |= blocking(move || {
            Ok(target
                .processes
                .list()
                .iter()
                .any(|process| !matches!(process.info.state.as_str(), "T" | "t" | "Z" | "X")))
        })
        .await?;
        Ok(result)
    }

    /// `fs.restore`: verifies the frozen plan, refuses active tracked writers, and resumes one job.
    pub async fn fs_restore(
        &self,
        body: &orch_link_contract::source::FsRestore,
        source: &dyn BlobSource,
    ) -> Result<orch_link_contract::source::FsRestoreResult> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.clone().ok_or_else(|| {
            HostError::Unsupported("the recorded store is unavailable".to_owned())
        })?;
        let recovery_states = states.clone();
        let restored =
            async {
                let limit =
                    usize::try_from(manifest::MANIFEST_MAX_EXPANDED_BYTES).map_err(|_| {
                        HostError::Unsupported(
                            "the expanded manifest limit exceeds this platform".to_owned(),
                        )
                    })?;
                let mut writer = MemoryWriter::with_limit(limit);
                download_checked(source, body.plan_blob, &mut writer)
                    .await
                    .map_err(|error| {
                        HostError::Content(format!("the restore plan could not be had: {error}"))
                    })?;
                let bytes = writer.into_bytes();
                let target = environment.clone();
                let request = body.clone();
                let now = self.inner.clock.time();
                blocking(move || {
                    target.files.gates().require_remote_write()?;
                    if target.processes.list().iter().any(|process| {
                        !matches!(process.info.state.as_str(), "T" | "t" | "Z" | "X")
                    }) {
                        return Err(HostError::Changed(
                            "tracked writers are still active; stop or hold them before restoring"
                                .to_owned(),
                        ));
                    }
                    crate::restore::keep_plan(&states, request.plan_blob, &bytes)?;
                    states.invalidate("not_ready");
                    crate::restore::apply(
                        &target.files,
                        &states,
                        &request,
                        target.facts().generation,
                        now,
                    )
                })
                .await
            }
            .await;
        match restored {
            Ok(result) => {
                environment.seed_file_states();
                Ok(result)
            }
            Err(error)
                if crate::restore::recovery_required(&recovery_states, body.job_id)
                    .unwrap_or(true) =>
            {
                Err(HostError::RestoreRecoveryRequired(format!(
                    "the restore must finish or return its applied paths before release: {error}"
                )))
            }
            Err(error) => Err(error),
        }
    }

    /// `workspace.observe`: returns end hashes from a complete watcher baseline only.
    pub async fn workspace_observe(
        &self,
        body: &orch_link_contract::source::WorkspaceObserve,
        sink: &dyn BlobSink,
    ) -> Result<orch_link_contract::source::WorkspaceObserveResult> {
        let environment = self.environment(body.environment_id)?;
        if !environment.index.state().is_full()
            || environment
                .index
                .changes_since(environment.index.revision())
                .is_none()
        {
            return Ok(orch_link_contract::source::WorkspaceObserveResult {
                end_manifest_blob: None,
            });
        }
        let Some(states) = environment.file_states.clone() else {
            return Ok(orch_link_contract::source::WorkspaceObserveResult {
                end_manifest_blob: None,
            });
        };
        let files = environment.files.clone();
        let directory = files.parse(&body.directory)?;
        let generation = environment.facts().generation;
        let now = self.inner.clock.time();
        let observed =
            blocking(move || states.observe(&files, &directory, generation, now)).await?;
        let Some(manifest) = observed else {
            return Ok(orch_link_contract::source::WorkspaceObserveResult {
                end_manifest_blob: None,
            });
        };
        let size = i64::try_from(manifest.bytes.len())
            .map_err(|_| HostError::Unsupported("an end manifest is too large".to_owned()))?;
        let hash = manifest.hash;
        let mut reader = crate::blobs::MemoryReader::new(manifest.bytes);
        sink.put(hash, size, &mut reader)
            .await
            .map_err(|error| HostError::Content(error.to_string()))?;
        Ok(orch_link_contract::source::WorkspaceObserveResult {
            end_manifest_blob: Some(hash),
        })
    }

    // ------------------------------------------------------------------ git

    /// `git.worktree.copy_plan`: measures repository data beside its selected working files.
    pub async fn git_worktree_copy_plan(
        &self,
        body: &orch_link_contract::source::GitWorktreeCopyPlan,
    ) -> Result<orch_link_contract::source::GitWorktreeCopyPlanResult> {
        let environment = self.environment(body.environment_id)?;
        Self::require_selection(&environment, &body.selection)?;
        let measured = self
            .fs_measure(&orch_link_contract::commands::FsMeasure {
                environment_id: body.environment_id,
                selection: body.selection.clone(),
            })
            .await?;
        if !measured.complete {
            return Err(HostError::Unsupported(
                "the worktree's selected files do not fit the bounded copy plan".into(),
            ));
        }
        let files = environment.files.clone();
        let selection = body.selection.clone();
        let directory = body.directory.clone();
        blocking(move || {
            let mapping = crate::git_copy::independent(&files, &selection, &directory)?;
            if mapping.as_ref().is_some_and(|mapping| {
                measured.entry_count.saturating_add(
                    i64::try_from(mapping.entries.len())
                        .unwrap_or(i64::MAX)
                        .saturating_sub(1),
                ) > i64::try_from(orch_link_contract::commands::SNAPSHOT_MAX_ENTRIES)
                    .unwrap_or(i64::MAX)
            }) {
                return Err(HostError::Unsupported(
                    "the files and independent repository together exceed the snapshot entry limit"
                        .into(),
                ));
            }
            Ok(mapping.map_or(
                orch_link_contract::source::GitWorktreeCopyPlanResult {
                    independent: false,
                    additional_files: 0,
                    additional_bytes: 0,
                },
                |mapping| orch_link_contract::source::GitWorktreeCopyPlanResult {
                    independent: true,
                    additional_files: mapping.additional_files,
                    additional_bytes: mapping.additional_bytes,
                },
            ))
        })
        .await
    }

    /// `git.worktree.snapshot`: exports raw files and an independent repository rather than a back link.
    pub async fn git_worktree_snapshot(
        &self,
        body: &orch_link_contract::source::GitWorktreeSnapshot,
        sink: &dyn BlobSink,
        progress: &dyn Progress,
    ) -> Result<orch_link_contract::source::GitWorktreeSnapshotResult> {
        let environment = self.environment(body.environment_id)?;
        self.git_worktree_copy_plan(&orch_link_contract::source::GitWorktreeCopyPlan {
            environment_id: body.environment_id,
            selection: body.selection.clone(),
            directory: body.directory.clone(),
        })
        .await?;
        let (report, independent) = environment
            .files
            .snapshot_independent(
                &body.selection,
                &body.directory,
                environment.facts().generation,
                self.inner.clock.as_ref(),
                sink,
                progress,
            )
            .await?;
        Ok(orch_link_contract::source::GitWorktreeSnapshotResult {
            snapshot: report.result,
            independent,
        })
    }

    /// `git.worktree.standalone`: proves the copied refs and index need no source-host repository.
    pub async fn git_worktree_standalone(
        &self,
        body: &orch_link_contract::source::GitWorktreeStandalone,
    ) -> Result<()> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.as_ref().ok_or_else(|| {
            HostError::Unsupported("the independent checkout needs its owned job store".into())
        })?;
        crate::git_copy::verify_independent(
            &environment.git,
            &environment.files,
            states,
            &body.directory,
        )
        .await
    }

    /// Reads the original completed preparation before a no-longer-needed secret is opened.
    pub fn git_prepare_completed(
        &self,
        body: &orch_link_contract::source::GitPrepare,
    ) -> Result<Option<orch_api_contract::source::SourceInfo>> {
        let environment = self.environment(body.environment_id)?;
        environment.files.gates().require_remote_write()?;
        let states = environment.file_states.as_ref().ok_or_else(|| {
            HostError::Unsupported("source preparation needs its owned journal".into())
        })?;
        crate::git_actions::completed(states, body, environment.facts().generation)
    }

    /// `git.prepare`: a journaled source checkout before this directory starts a session.
    pub async fn git_prepare(
        &self,
        body: &orch_link_contract::source::GitPrepare,
        credential: Option<&str>,
    ) -> Result<orch_api_contract::source::SourceInfo> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.clone().ok_or_else(|| {
            HostError::Unsupported(
                "source preparation needs its durable environment-owned store".to_owned(),
            )
        })?;
        let result = crate::git_actions::prepare(
            &environment.git,
            &environment.files,
            &states,
            body,
            environment.facts().generation,
            self.inner.settings().managed.is_some(),
            credential,
        )
        .await;
        if result.is_ok() {
            environment.seed_file_states();
        }
        result
    }

    /// `git.prepare.cleanup`: removes only empty stages under the original destination handle.
    pub async fn git_prepare_cleanup(
        &self,
        body: &orch_link_contract::source::GitPrepareCleanup,
    ) -> Result<orch_link_contract::source::GitPrepareCleanupResult> {
        let environment = self.environment(body.original.environment_id)?;
        let states = environment.file_states.as_ref().ok_or_else(|| {
            HostError::Unsupported("source cleanup needs its owned journal".into())
        })?;
        crate::git_actions::cleanup(
            &environment.files,
            states,
            &body.original,
            environment.facts().generation,
        )
        .await
    }

    /// `git.worktree`: prepares another branch and directory without changing the source's files or index.
    pub async fn git_worktree(
        &self,
        body: &orch_link_contract::source::GitWorktree,
    ) -> Result<orch_api_contract::source::SourceInfo> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.as_ref().ok_or_else(|| {
            HostError::Unsupported("an own attempt needs the owned file-state store".to_owned())
        })?;
        environment.files.gates().require_remote_write()?;
        if let Some(result) =
            crate::git_worktrees::completed(states, body, environment.facts().generation)?
        {
            return Ok(result);
        }
        let result = crate::git_worktrees::create(
            &environment.git,
            &environment.files,
            states,
            body,
            environment.facts().generation,
            self.inner.clock.time(),
        )
        .await;
        if result.is_ok() {
            environment.seed_file_states();
        }
        result
    }

    /// `git.worktree.cleanup`: removes only the unchanged worktree owned by its original job.
    pub async fn git_worktree_cleanup(
        &self,
        body: &orch_link_contract::source::GitWorktreeCleanup,
    ) -> Result<orch_link_contract::source::GitWorktreeCleanupResult> {
        let environment = self.environment(body.original.environment_id)?;
        let states = environment.file_states.as_ref().ok_or_else(|| {
            HostError::Unsupported("worktree cleanup needs its owned journal".into())
        })?;
        let result = crate::git_worktrees::cleanup(
            &environment.git,
            &environment.files,
            states,
            &body.original,
            environment.facts().generation,
            self.inner.clock.time(),
        )
        .await;
        if result.as_ref().is_ok_and(|result| result.removed) {
            environment.seed_file_states();
        }
        result
    }

    /// `git.worktree.repair`: translates copied pointers without opening any source-host path.
    pub async fn git_worktree_repair(
        &self,
        body: &orch_link_contract::source::GitWorktreeRepair,
        source: &dyn BlobSource,
    ) -> Result<orch_link_contract::source::GitWorktreeRepairResult> {
        let environment = self.environment(body.environment_id)?;
        let mut writer = MemoryWriter::with_limit(MANIFEST_DOWNLOAD_MAX_BYTES);
        download_checked(source, body.manifest_blob, &mut writer)
            .await
            .map_err(|error| HostError::Content(error.to_string()))?;
        let parsed = manifest::parse(&writer.into_bytes())?;
        let files = environment.files.clone();
        let previous = body.previous_root.clone();
        let target = body.target_root.clone();
        let repaired =
            blocking(move || crate::git_copy::repair(&files, &parsed, &previous, &target)).await?;
        environment.seed_file_states();
        Ok(orch_link_contract::source::GitWorktreeRepairResult { repaired })
    }

    /// `git.commit`: makes the reviewed selected-path commit and retains its reconciliation journal.
    pub async fn git_commit(
        &self,
        body: &orch_link_contract::source::GitCommit,
    ) -> Result<orch_api_contract::source::GitCommitResult> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.as_ref().ok_or_else(|| {
            HostError::Unsupported("the owned file-state store is unavailable".to_owned())
        })?;
        let result =
            crate::git_publication::commit(&environment.git, &environment.files, states, body)
                .await;
        if result.is_ok() {
            environment.seed_file_states();
        }
        result
    }

    /// `git.push`: sends only the reviewed commit and exact remote, with scoped authentication.
    pub async fn git_push(
        &self,
        body: &orch_link_contract::source::GitPush,
        credential: Option<&str>,
    ) -> Result<orch_link_contract::source::GitPushResult> {
        let environment = self.environment(body.environment_id)?;
        let states = environment.file_states.as_ref().ok_or_else(|| {
            HostError::Unsupported("the owned file-state store is unavailable".to_owned())
        })?;
        crate::git_publication::push(
            &environment.git,
            &environment.files,
            states,
            body,
            credential,
        )
        .await
    }

    /// `git.status`: the state of the repository in the selected workspace directory.
    pub async fn git_status(
        &self,
        body: &GitStatus,
    ) -> Result<orch_api_contract::workspace::GitStatus> {
        let environment = self.environment(body.environment_id)?;
        let path = body
            .path
            .as_deref()
            .map(|text| environment.files.parse(text))
            .transpose()?;
        if let Some(watch) = body.watch_id {
            environment.git_watches.register(watch, path.clone())?;
            environment.follow_index_with_watches();
            return environment.git_watches.read(&environment, watch).await;
        }
        let status = environment
            .files
            .git_status_at(&environment.git, path.as_ref())
            .await?;
        Ok(status)
    }

    /// `git.diff`: the unified difference of the repository.
    pub async fn git_diff(&self, body: &GitDiff) -> Result<GitDiffResult> {
        let environment = self.environment(body.environment_id)?;
        let path = match &body.path {
            Some(text) => Some(environment.files.parse(text)?),
            None => None,
        };
        environment
            .files
            .git_diff(
                &environment.git,
                path.as_ref(),
                body.staged.unwrap_or(false),
            )
            .await
    }

    // ------------------------------------------------------------------ processes

    /// `proc.list`: the processes of the environment.
    pub async fn proc_list(&self, body: &ProcList) -> Result<ProcListResult> {
        let environment = self.environment(body.environment_id)?;
        let processes = Arc::clone(&environment);
        let listed = tokio::task::spawn_blocking(move || processes.processes.list())
            .await
            .map_err(|e| HostError::Internal(format!("a task of the host failed: {e}")))?;
        let processes = listed
            .iter()
            .map(|p| wire(&p.info))
            .collect::<Result<Vec<_>>>()?;
        Ok(ProcListResult {
            as_of: self.inner.clock.time(),
            processes,
        })
    }

    /// `proc.signal`: signals one process, named by its number and the moment it started.
    pub async fn proc_signal(&self, body: &ProcSignal) -> Result<()> {
        let environment = self.environment(body.environment_id)?;
        let (pid, started_at, signal) = (body.pid, body.started_at, body.signal);
        blocking(move || environment.processes.signal(pid, started_at, signal)).await?;
        self.report_after_signal(body.environment_id).await
    }

    /// `proc.suspend`: holds still every process the environment's agents started.
    pub async fn proc_suspend(&self, body: &ProcSuspend) -> Result<ProcHeldResult> {
        let environment = self.environment(body.environment_id)?;
        let held = blocking(move || environment.processes.suspend()).await?;
        Ok(ProcHeldResult { processes: held })
    }

    /// `proc.resume`: lets go of what `proc.suspend` held still.
    pub async fn proc_resume(&self, body: &ProcResume) -> Result<ProcHeldResult> {
        let environment = self.environment(body.environment_id)?;
        let released = blocking(move || environment.processes.resume()).await?;
        Ok(ProcHeldResult {
            processes: released,
        })
    }

    /// `watch.set`: says which measurements are wanted and how often. Measurements that are not
    /// asked for are not taken.
    pub async fn watch_set(&self, body: &WatchSet) -> Result<()> {
        let environment = self.environment(body.environment_id)?;
        let interval = Duration::from_millis(u64::from(body.interval_ms))
            .clamp(REPORT_INTERVAL_MIN, REPORT_INTERVAL_MAX);
        *lock(&environment.wanted) = Some(Wanted {
            stats: body.stats,
            procs: body.procs,
            ports: body.ports,
            terminals: body.terminals,
            interval,
        });
        environment.start_reporters();
        Ok(())
    }

    // ------------------------------------------------------------------ shells

    /// `pty.open`: starts a shell of one person's own, as a login shell in a directory of the
    /// workspace, with the environment's variables and secrets.
    pub async fn pty_open(&self, body: &PtyOpen) -> Result<PtyOpenResult> {
        let environment = self.environment(body.environment_id)?;
        self.inner.gates.require_terminal()?;
        let settings = self.inner.settings();
        let files = &environment.files;
        let cwd = files.target_of(&body.cwd)?;
        let located = files.root().locate(&cwd, Locate::THROUGH)?;
        if !located.meta.is_some_and(|meta| meta.is_dir()) {
            return Err(HostError::NotFound(format!("{cwd} is not a directory")));
        }
        let (_, program) = find_shell(settings.shell.as_deref()).ok_or_else(|| {
            HostError::Unsupported("this machine has no shell to open".to_owned())
        })?;
        let spec = OpenSpec {
            id: body.terminal_id,
            environment_id: environment.id,
            purpose: Purpose::Shell,
            program,
            args: Vec::new(),
            login: !cfg!(windows),
            cwd: located.real.under(files.root().root_path()),
            env: environment.variables.clone(),
            cols: body.cols,
            rows: body.rows,
            record: body.record,
        };
        let opened = environment.terminals.open(spec)?;
        let service = Service::Pty {
            terminal_id: body.terminal_id,
        }
        .to_string();
        Ok(PtyOpenResult {
            service,
            shell: opened.shell,
        })
    }

    /// `pty.close`: ends a shell and says how it ended.
    pub async fn pty_close(&self, body: &PtyClose) -> Result<PtyCloseResult> {
        for environment in self.environments() {
            if environment
                .terminals
                .list()
                .iter()
                .any(|t| t.id == body.terminal_id)
            {
                let closed = environment.terminals.close(body.terminal_id).await?;
                return Ok(PtyCloseResult {
                    exit_code: closed.exit_code,
                    bytes_out: closed.bytes_out,
                    kept: closed.kept,
                });
            }
        }
        Err(HostError::NotFound(format!(
            "there is no shell {}",
            body.terminal_id
        )))
    }

    /// `pty.status`: selected shells' foreground state, without taking an attachment.
    pub fn pty_status(
        &self,
        body: &orch_link_contract::commands::PtyStatus,
    ) -> Result<orch_link_contract::commands::PtyStatusResult> {
        if body.terminal_ids.len() > 200 {
            return Err(HostError::TooLarge {
                what: "shell observation selection".into(),
                size: i64::try_from(body.terminal_ids.len()).unwrap_or(i64::MAX),
                limit: 200,
            });
        }
        let environment = self.environment(body.environment_id)?;
        let terminals = environment
            .terminals
            .list()
            .into_iter()
            .filter(|shell| {
                shell.purpose == Purpose::Shell
                    && !shell.exited
                    && body.terminal_ids.contains(&shell.id)
            })
            .map(|shell| orch_link_contract::commands::PtyStatusEntry {
                terminal_id: shell.id,
                busy: shell.busy,
            })
            .collect();
        Ok(orch_link_contract::commands::PtyStatusResult { terminals })
    }

    /// Takes the shell `terminal_id` up for a tunnel that was opened to `pty/<terminal id>`: the
    /// attachment is given what the shell printed last, to send first.
    pub fn pty_attach(&self, terminal_id: Id, control: bool) -> Result<Attachment> {
        for environment in self.environments() {
            if environment
                .terminals
                .list()
                .iter()
                .any(|t| t.id == terminal_id)
            {
                return environment.terminals.attach(terminal_id, control);
            }
        }
        Err(HostError::NotFound(format!(
            "there is no shell {terminal_id}"
        )))
    }

    // ------------------------------------------------------------------ executors and tool servers

    /// `exec.start`: starts an executor in the environment.
    pub async fn exec_start(&self, body: &ExecStart) -> Result<ExecStartResult> {
        let environment = self.environment(body.environment_id)?;
        let allowed = self.inner.settings().exec_offers_allowed;
        let context = environment.context();
        let started = environment
            .executors
            .start(&body.executor, allowed.as_deref(), &context)
            .await?;
        let facts = environment.refresh_facts().await;
        self.inner
            .reports
            .say(environment.id, Report::Facts(Box::new(facts)))
            .await;
        Ok(started)
    }

    /// Every executor that runs in an environment of this host, with the environment it runs in.
    pub async fn running_executors(
        &self,
    ) -> Vec<(
        Id,
        String,
        orch_executor_contract::descriptor::ExecutorProtocol,
        ExecutorEndpoint,
    )> {
        let mut found = Vec::new();
        for environment in self.environments() {
            for (kind, protocol, endpoint) in environment.executors.running().await {
                found.push((environment.id, kind, protocol, endpoint));
            }
        }
        found
    }

    /// Every tool server that runs in an environment of this host, with the environment it runs in.
    pub async fn running_servers(&self) -> Vec<(Id, String, McpEndpoint)> {
        let mut found = Vec::new();
        for environment in self.environments() {
            for server in environment.mcp.list() {
                if let Some(endpoint) = environment.mcp_host.endpoint(&server.name).await {
                    found.push((environment.id, server.name, endpoint));
                }
            }
        }
        found
    }

    /// Where the executor `kind` of an environment is reached, while it runs.
    pub async fn executor_endpoint(
        &self,
        environment_id: Id,
        kind: &str,
    ) -> Option<ExecutorEndpoint> {
        let environment = self.environment(environment_id).ok()?;
        environment.executors.endpoint(kind).await
    }

    /// Where the executor `kind` of an environment is reached, started now if it is not running or no
    /// longer answers: a tunnel that arrives after it died finds it again.
    pub async fn executor_ensure(
        &self,
        environment_id: Id,
        kind: &str,
    ) -> Result<ExecutorEndpoint> {
        let environment = self.environment(environment_id)?;
        let allowed = self.inner.settings().exec_offers_allowed;
        let context = environment.context();
        environment
            .executors
            .ensure(kind, allowed.as_deref(), &context)
            .await
    }

    /// The protocol an executor of an environment speaks to a harness, if the environment has it.
    pub async fn executor_protocol(
        &self,
        environment_id: Id,
        kind: &str,
    ) -> Option<orch_executor_contract::descriptor::ExecutorProtocol> {
        let environment = self.environment(environment_id).ok()?;
        environment.executors.protocol_of(kind).await
    }

    /// `mcp.start`: starts a tool server in an environment, from the definition the command brings
    /// `mcp.restart`: restarts just this known server, clearing only its own budget.
    pub async fn mcp_restart(
        &self,
        body: &orch_link_contract::commands::McpRestart,
        secrets: &BTreeMap<String, String>,
    ) -> Result<orch_link_contract::commands::McpRestartResult> {
        let (id, name) = match &body.0 {
            McpStart::Given(given) => (given.environment_id, given.name.as_str()),
            McpStart::Local(local) => (local.environment_id, local.name.as_str()),
        };
        let environment = self.environment(id)?;
        let outcome = match environment.mcp_host.restart(name).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => self.mcp_start(&body.0, secrets).await.map(|_| ()),
            Err(error) => Err(error),
        };
        environment.announce_facts().await;
        outcome?;
        Ok(orch_link_contract::commands::McpRestartResult {
            service: crate::mcp::service_of(name),
            servers: environment.facts().mcp_servers,
        })
    }

    /// `mcp.start`: starts a tool server in an environment, from the definition the command brings
    /// (`secrets` are the values of the secret variables it names, unsealed by the caller) or from the
    /// copy this machine kept of one that was imported here.
    pub async fn mcp_start(
        &self,
        body: &McpStart,
        secrets: &BTreeMap<String, String>,
    ) -> Result<McpStartResult> {
        let (environment_id, name, origin, definition) = match body {
            McpStart::Given(given) => {
                let spec: McpServerSpec = given.spec.clone();
                let definition = McpDefinition {
                    spec,
                    secrets: secrets
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.clone(),
                                orch_plugin_contract::Secret::new(value.clone()),
                            )
                        })
                        .collect(),
                };
                // A definition that arrives with the command is written from Orch, whichever page it came
                // from, and runs as the user the connector runs as: it takes the machine's permission.
                (
                    given.environment_id,
                    given.name.clone(),
                    Origin::Own,
                    definition,
                )
            }
            McpStart::Local(local) => {
                let environment = self.environment(local.environment_id)?;
                let definition = environment.mcp.load(local.server_id)?;
                (
                    local.environment_id,
                    local.name.clone(),
                    Origin::Imported(local.server_id),
                    definition,
                )
            }
        };
        let environment = self.environment(environment_id)?;
        let facts = environment.facts();
        let launch = Launch {
            workspace: PathBuf::from(&facts.workspace_root),
            home: Some(PathBuf::from(&facts.home))
                .filter(|home| !facts.home.is_empty() && home.is_dir()),
        };
        environment
            .mcp_host
            .start_identified(
                &name,
                origin,
                definition,
                &launch,
                match body {
                    McpStart::Given(given) => given.server_id,
                    McpStart::Local(local) => Some(local.server_id),
                },
            )
            .await?;
        environment.announce_facts().await;
        Ok(McpStartResult {
            service: crate::mcp::service_of(&name),
        })
    }

    /// `mcp.stop`: stops a tool server. One that is not running is already stopped.
    pub async fn mcp_stop(&self, body: &McpStop) -> Result<()> {
        let environment = self.environment(body.environment_id)?;
        if environment.mcp.get(&body.name).is_some() {
            environment.mcp_host.stop(&body.name).await?;
            environment.announce_facts().await;
        }
        Ok(())
    }

    /// Where the tool server `name` of an environment is reached, while it runs.
    pub async fn mcp_endpoint(&self, environment_id: Id, name: &str) -> Option<McpEndpoint> {
        let environment = self.environment(environment_id).ok()?;
        environment.mcp_host.endpoint(name).await
    }

    /// Why the tool server `name` of an environment cannot be reached, when it was given up on.
    #[must_use]
    pub fn mcp_failure(&self, environment_id: Id, name: &str) -> Option<String> {
        let environment = self.environment(environment_id).ok()?;
        match environment.mcp.get(name)?.state {
            ServerState::Failed { reason } => Some(reason),
            _ => None,
        }
    }

    /// `mcp.forget`: deletes the copy of a definition that `mcp.import` kept. The copies belong
    /// to the machine, so no environment has to be attached for this.
    pub fn mcp_forget(&self, server_id: Id) -> Result<()> {
        crate::mcp::forget_copy(&self.inner.settings().data_dir, server_id)
    }

    /// Reads the facts of the environment again, after something the connector changed in it (the
    /// tool servers found in a harness's configuration, say), says them if they differ from what
    /// was last said, and returns them.
    pub async fn facts_changed(&self, environment_id: Id) -> Result<EnvironmentFacts> {
        let environment = self.environment(environment_id)?;
        environment.announce_facts().await;
        Ok(environment.facts())
    }

    /// `mcp.import`: keeps a copy of a definition an adapter read from a harness's configuration.
    pub fn mcp_import(
        &self,
        body: &McpImport,
        definition: &McpDefinition,
        tools: Option<i64>,
    ) -> Result<McpImportResult> {
        self.environment(body.environment_id)?
            .mcp
            .import(body, definition, tools)
    }
}

#[derive(Clone, Copy)]
enum Measure {
    Stats,
    Procs,
}

async fn report_ports(environment: Weak<Environment>) {
    let Some(current) = environment.upgrade() else {
        return;
    };
    let mut changes = current.ports.watch();
    drop(current);
    let mut last: Option<orch_api_contract::ports::PortSnapshot> = None;
    loop {
        if changes.changed().await.is_err() {
            return;
        }
        let Some(snapshot) = changes.borrow_and_update().clone() else {
            continue;
        };
        if let Some(was) = &last {
            let mut compared = snapshot.clone();
            compared.as_of = was.as_of;
            if compared == *was {
                continue;
            }
        }
        last = Some(snapshot.clone());
        let Some(environment) = environment.upgrade() else {
            return;
        };
        let Ok(host) = environment.inner() else {
            return;
        };
        host.reports
            .say(
                environment.id,
                Report::Ports(orch_link_contract::ports::PortsSnapshot {
                    request_ref: None,
                    snapshot,
                }),
            )
            .await;
    }
}

async fn report_terminals(environment: Weak<Environment>, interval: Duration) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last = None;
    loop {
        ticks.tick().await;
        let Some(environment) = environment.upgrade() else {
            return;
        };
        let Ok(host) = environment.inner() else {
            return;
        };
        let mut terminals: Vec<_> = environment
            .terminals
            .list()
            .into_iter()
            .filter(|shell| shell.purpose == Purpose::Shell && !shell.exited)
            .map(|shell| orch_link_contract::commands::PtyStatusEntry {
                terminal_id: shell.id,
                busy: shell.busy,
            })
            .collect();
        terminals.sort_by_key(|shell| shell.terminal_id);
        let data = orch_link_contract::commands::PtyStatusResult { terminals };
        if last.as_ref() == Some(&data) {
            continue;
        }
        last = Some(data.clone());
        host.reports
            .say(environment.id, Report::Terminals(data))
            .await;
    }
}

async fn report_loop(environment: Weak<Environment>, interval: Duration, measure: Measure) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let Some(environment) = environment.upgrade() else {
            return;
        };
        let Ok(host) = environment.inner() else {
            return;
        };
        match measure {
            Measure::Stats => {
                let sampler = Arc::clone(&environment.sampler);
                if let Ok(sample) = tokio::task::spawn_blocking(move || sampler.sample()).await {
                    host.reports
                        .say(environment.id, Report::Stats(sample))
                        .await;
                }
            }
            Measure::Procs => {
                let target = Arc::clone(&environment);
                if let Ok(listed) =
                    tokio::task::spawn_blocking(move || target.processes.list()).await
                {
                    let processes = listed.into_iter().map(|listed| listed.info).collect();
                    host.reports
                        .say(
                            environment.id,
                            Report::Procs {
                                as_of: host.clock.time(),
                                processes,
                            },
                        )
                        .await;
                }
            }
        }
    }
}

/// A freshly built environment, the first walk of its tree if that was still going on, and the
/// shells a previous run of the connector left open.
struct Attached {
    environment: Arc<Environment>,
    first_scan: Option<FirstScan>,
    lost_shells: Vec<Id>,
}

/// Opens the workspace root and finds out how it spells its names.
fn open_workspace(configured: &Path, allowed: Option<&[PathBuf]>) -> Result<(Workspace, bool)> {
    if let Some(roots) = allowed {
        let real = std::fs::canonicalize(configured)
            .map_err(|error| HostError::io(format!("opening {}", configured.display()), error))?;
        if !roots
            .iter()
            .any(|root| std::fs::canonicalize(root).is_ok_and(|root| real.starts_with(root)))
        {
            return Err(HostError::RootNotAllowed(format!(
                "this machine's owner does not offer {}",
                configured.display()
            )));
        }
    }
    let dir = Dir::open_path(configured)
        .map_err(|error| HostError::io(format!("opening {}", configured.display()), error))?;
    let case_sensitive =
        Workspace::probe_case_sensitivity(&dir).unwrap_or(platform::DEFAULT_CASE_SENSITIVE);
    Ok((
        Workspace::open(configured, &[], path_style(), case_sensitive)?,
        case_sensitive,
    ))
}

/// Builds an environment from the command and what the host already has of it.
async fn attach(
    host: &Arc<HostInner>,
    settings: &HostSettings,
    body: &EnvAttach,
    secrets: &BTreeMap<String, String>,
    previous: Option<&Arc<Environment>>,
) -> Result<Attached> {
    if body
        .data_generation
        .is_some_and(|generation| generation < 0)
    {
        return Err(HostError::Unsupported(
            "an owned-data generation is not negative".into(),
        ));
    }
    if !crate::RUNTIME_VERIFIED {
        return Err(HostError::Unsupported("the environment runtime is declared for Linux; this platform awaits native containment and process verification".into()));
    }
    // The controller supplies the current or candidate generation at attach time.
    // Managed connectors also start before that generation is known to their host settings.
    let generation = body.data_generation.unwrap_or(settings.generation);
    let resources = body.resources.unwrap_or(settings.unit_limits);
    let resource_group = if settings.process_scope == ScopeKind::Started {
        if let Some(group) = previous.and_then(|old| old.resource_group.as_ref()) {
            group
                .update(&resources, settings.process_controls)
                .await
                .map_err(HostError::Unsupported)?;
            Some(group.clone())
        } else {
            crate::process_limits::UserGroup::open(
                body.environment_id,
                &resources,
                settings.process_controls,
            )
            .await
            .map_err(HostError::Unsupported)?
        }
    } else {
        None
    };
    let launcher = resource_group
        .as_ref()
        .map_or_else(Vec::new, crate::process_limits::UserGroup::launcher);
    // Where the workspace is: what the driver gave a unit, or the root a person's machine is told,
    // which has to be one of the directories its owner offers.
    let configured: PathBuf = match (&settings.managed, &body.workspace_root) {
        (Some(managed), _) => managed.workspace_root.clone(),
        (None, Some(root)) => PathBuf::from(root),
        (None, None) => {
            return Err(HostError::Unsupported(
                "a directory on a machine is attached with its root".to_owned(),
            ));
        }
    };
    let (workspace, case_sensitive) = {
        let (configured, allowed) = (configured.clone(), settings.allowed_roots.clone());
        blocking(move || open_workspace(&configured, allowed.as_deref())).await?
    };
    let rules = Arc::new(IgnoreRules::new(&body.ignore, case_sensitive));
    for rejected in rules.rejected() {
        tracing::warn!(pattern = %rejected, "an ignore pattern is not a pattern and is left out");
    }
    let files = Files::workspace(
        workspace,
        Arc::clone(&rules),
        Arc::new(HashCache::new()),
        settings.limits.clone(),
        Arc::clone(&host.gates),
    );

    // The tree: indexed, or listed live. A walk that outlasts the wait goes on by itself.
    let index = Arc::new(Index::with_scan_limit(
        body.index,
        settings.limits.index_max_entries,
        settings.limits.index_scan_max,
    ));
    let mut first_scan = None;
    if index.state().is_full() {
        let (index, files, rules) = (Arc::clone(&index), files.clone(), Arc::clone(&rules));
        let mut walk = tokio::task::spawn_blocking(move || index.scan(files.root(), &rules));
        match tokio::time::timeout(ATTACH_SCAN_WAIT, &mut walk).await {
            Ok(finished) => {
                finished.map_err(|error| {
                    HostError::Internal(format!("a task of the host failed: {error}"))
                })??;
            }
            Err(_) => first_scan = Some(walk),
        }
    }

    // Mounts.
    let home = settings
        .managed
        .as_ref()
        .and_then(|managed| managed.home.clone())
        .or_else(|| {
            body.data_sources
                .as_ref()
                .and_then(|sources| sources.home.directory())
                .map(PathBuf::from)
        })
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    let root_text = files.root().root_path().to_string_lossy().into_owned();
    let home_text = home
        .as_ref()
        .map(|home| home.to_string_lossy().into_owned());
    let mounts = match (
        &body.mounts,
        settings
            .managed
            .as_ref()
            .and_then(|managed| managed.mounts_file.as_ref()),
    ) {
        (Some(requests), _) => {
            let mut reserved = vec![root_text.as_str(), RUN_DIR];
            if let Some(home) = &home_text {
                reserved.push(home.as_str());
            }
            Mounts::attach_in_place(
                requests,
                &MountPolicy {
                    roots: settings.allowed_roots.clone(),
                },
                &reserved,
                &settings.limits,
                &host.gates,
            )
        }
        (None, Some(file)) if file.is_file() => Mounts::from_driver_file(
            file,
            crate::mounts::driver_mounts_enforced(Some(file)),
            &settings.limits,
            &host.gates,
        )?,
        (None, _) => Mounts::none(),
    };

    // Kept browser profiles belong outside the workspace and never enter a snapshot or a fork.
    let browser_directory = if settings.managed.is_some() {
        PathBuf::from(home_text.as_deref().unwrap_or_default()).join(".orch/browsers")
    } else {
        settings
            .data_dir
            .join("browsers")
            .join(body.environment_id.to_string())
    };
    let mut browser_variables = body.variables.clone();
    for name in secrets.keys() {
        browser_variables.remove(name);
    }
    browser_variables.insert(
        "ORCH_BROWSER_PROFILES".to_owned(),
        browser_directory.to_string_lossy().into_owned(),
    );
    let windows = match previous {
        Some(old) => Arc::clone(&old.windows),
        None => crate::browser_window::Windows::with_store(
            browser_variables.clone(),
            settings
                .data_dir
                .join("displays")
                .join(body.environment_id.to_string()),
        )
        .map_err(|error| HostError::Internal(error.to_string()))?,
    };
    windows.set_launcher(launcher.clone());
    let browsers = match previous {
        Some(old) if old.facts().generation == generation => Arc::clone(&old.browsers),
        _ => crate::browsers::Browsers::new(
            body.environment_id,
            browser_directory.clone(),
            browser_variables,
            settings.managed.is_some(),
            generation,
            Arc::clone(&host.clock),
            host.reports.clone(),
        )
        .map_err(|e| HostError::Internal(e.to_string()))?,
    };
    browsers.set_launcher(launcher.clone());
    let screens = browsers
        .look()
        .await
        .map_err(|e| HostError::Internal(e.to_string()))?;
    browsers.set_allowed(settings.gates.browser).await;
    let displays = previous
        .map(|old| Arc::clone(&old.displays))
        .unwrap_or_else(|| {
            crate::displays::Displays::new(
                body.environment_id,
                files.root().root_path().to_path_buf(),
                Arc::clone(&windows),
                host.reports.clone(),
                settings.managed.is_some(),
            )
        });
    displays.recover_owned().await;
    displays.set_allowed(settings.gates.vnc).await;

    // What the environment's commands find in their environment: its variables, and its secrets
    // which win where a name is in both.
    let mut variables = body.variables.clone();
    variables.extend(
        secrets
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    variables.insert(
        "ORCH_BROWSER_PROFILES".to_owned(),
        browser_directory.to_string_lossy().into_owned(),
    );

    // The pieces that outlast an update.
    let processes = match previous {
        Some(old) => old.processes.clone(),
        None => {
            let processes = Processes::with_store(
                settings.process_scope,
                Arc::clone(&host.clock),
                settings
                    .data_dir
                    .join("processes")
                    .join(body.environment_id.to_string())
                    .join("held.json"),
            )?;
            if settings.process_scope == ScopeKind::All {
                let _ = processes.protect(platform::own_pid());
            }
            processes
        }
    };
    if !launcher.is_empty() && processes.launcher().is_empty() && !processes.list().is_empty() {
        return Err(HostError::Unsupported("detach this runtime before adding resource ceilings, so its existing processes can restart within them".into()));
    }
    processes.set_launcher(launcher.clone())?;
    let executors = match previous {
        Some(old) => Arc::clone(&old.executors),
        None => {
            let registry = Arc::new(Executors::new());
            for factory in &host.executors {
                registry.register(factory()).await?;
            }
            registry
        }
    };
    for attribution in executors.attributions().await {
        processes.add_attribution(attribution);
    }
    let mcp = match previous {
        Some(old) => Arc::clone(&old.mcp),
        None => Arc::new(McpServers::new(
            Arc::clone(&host.clock),
            settings.data_dir.clone(),
            settings.mcp_restarts,
            settings.mcp_restart_window,
            settings.mcp,
        )),
    };
    let mcp_host = match previous {
        Some(old) => Arc::clone(&old.mcp_host),
        None => Arc::new(McpHost::new(
            Arc::clone(&mcp),
            processes.clone(),
            host.reports.clone(),
            body.environment_id,
            McpHostConfig {
                ports: settings.mcp_ports.clone(),
                ..McpHostConfig::default()
            },
        )),
    };
    let mut lost_shells = Vec::new();
    let terminals = match previous {
        Some(old) => Arc::clone(&old.terminals),
        None => {
            let data_dir = settings
                .data_dir
                .join("terminals")
                .join(body.environment_id.to_string());
            std::fs::create_dir_all(&data_dir)
                .map_err(|error| HostError::io(format!("making {}", data_dir.display()), error))?;
            lost_shells = Terminals::recover(&data_dir);
            let links = TerminalLinks {
                processes: Some(processes.clone()),
                reports: Some(host.reports.clone()),
                blobs: Some(Arc::clone(&host.blobs)),
                data_dir: Some(data_dir),
            };
            Arc::new(Terminals::new(
                Arc::clone(&host.clock),
                settings.limits.terminal_keep_bytes,
                links,
            )?)
        }
    };
    let ports = match previous {
        Some(old)
            if old.facts().generation == generation
                && old.ports().scope() == settings.port_scope =>
        {
            old.ports().clone()
        }
        _ => crate::ports::Ports::new(
            body.environment_id,
            generation,
            settings.port_scope,
            processes.clone(),
            Arc::clone(&host.clock),
        ),
    };
    displays.protect_ports(ports.clone()).await;
    let sampler = Arc::new(Sampler::with_cgroup(
        settings.process_scope,
        processes.clone(),
        Arc::clone(&host.clock),
        files.root().root_path().to_path_buf(),
        resource_group
            .as_ref()
            .map(|group| group.cgroup().to_owned())
            .or_else(|| {
                (settings.process_scope == ScopeKind::All)
                    .then(crate::platform::os::own_cgroup_dir)
                    .flatten()
            }),
    ));
    let git = tokio::task::spawn_blocking(GitTool::discover)
        .await
        .map_err(|error| HostError::Internal(format!("a task of the host failed: {error}")))?;
    let shell = find_shell(settings.shell.as_deref()).map(|(name, path)| EnvironmentFactsShell {
        name,
        path: path.to_string_lossy().into_owned(),
    });
    let facts = EnvironmentFacts {
        browser_profiles_dir: Some(browser_directory.to_string_lossy().into_owned()),
        looked_at: Some(browsers.looked_at()),
        browser: screens.browser,
        vnc: screens.vnc,
        ports: Some(orch_link_contract::ports::EnvironmentPortsFacts {
            discovery: if cfg!(target_os = "linux") {
                orch_link_contract::ports::PortsDiscovery::Native
            } else {
                orch_link_contract::ports::PortsDiscovery::Unsupported
            },
            transports: if cfg!(target_os = "linux") {
                vec![
                    orch_api_contract::ports::PortTransport::Tcp,
                    orch_api_contract::ports::PortTransport::Udp,
                ]
            } else {
                Vec::new()
            },
            reason: (!cfg!(target_os = "linux"))
                .then(|| "scoped native port discovery is unavailable on this platform".into()),
        }),
        os: os(),
        arch: arch(),
        shell: shell.unwrap_or(EnvironmentFactsShell {
            name: String::new(),
            path: String::new(),
        }),
        user: current_user(),
        home: home_text.unwrap_or_default(),
        workspace_root: root_text,
        path_style: path_style(),
        case_sensitive,
        executors: Vec::new(),
        mcp_found: Vec::new(),
        mcp_servers: Vec::new(),
        index: index.state().mode(),
        index_reason: index.state().reason(),
        mounts: mounts.facts(),
        private_views: false,
        suspend: platform::CAN_SUSPEND,
        git: git.available(),
        generation,
    };
    let state_root = settings
        .managed
        .as_ref()
        .and_then(|managed| managed.home.as_ref())
        .map_or_else(
            || {
                settings
                    .data_dir
                    .join("file-states")
                    .join(match body.data_generation {
                        Some(generation) if generation > 0 => {
                            format!("{}-g{generation}", body.environment_id)
                        }
                        _ => body.environment_id.to_string(),
                    })
            },
            |home| {
                home.join(".orch/file-states")
                    .join(body.environment_id.to_string())
            },
        );
    let file_states = match crate::state_store::ObjectStore::open(
        &state_root,
        files.root().root_path(),
    ) {
        Ok(store) => Some(Arc::new(crate::file_states::FileStates::new(store))),
        Err(error) => {
            tracing::warn!(%error, environment=%body.environment_id, store=%state_root.display(),
                "the environment's recorded-file store could not be opened");
            None
        }
    };
    let environment = Arc::new(Environment {
        id: body.environment_id,
        directory_attachment: orch_types::new_id(),
        directory_operations: crate::directory_pages::Attachment::new(),
        host: Arc::downgrade(host),
        files,
        rules,
        index,
        scanned: AtomicBool::new(first_scan.is_none()),
        mounts,
        watcher: Mutex::new(None),
        directory_reads: tokio::sync::Mutex::new(()),
        watches: Mutex::new(
            previous
                .map(|old| std::mem::take(&mut *lock(&old.watches)))
                .unwrap_or_default(),
        ),
        directory_captures: Mutex::new(Vec::new()),
        git_watches: previous
            .map(|old| old.git_watches.take())
            .unwrap_or_default(),
        publisher: Publisher::new(Arc::clone(&host.clock), settings.limits.manifest_interval),
        processes,
        resources,
        resource_group,
        ports,
        sampler,
        terminals,
        executors,
        mcp,
        mcp_host,
        git,
        file_states,
        facts: Mutex::new(facts),
        variables,
        browsers,
        windows,
        displays,
        tasks: Mutex::new(Vec::new()),
        wanted: Mutex::new(previous.and_then(|old| *lock(&old.wanted))),
        reporters: Mutex::new(Vec::new()),
    });
    environment.refresh_facts().await;
    Ok(Attached {
        environment,
        first_scan,
        lost_shells,
    })
}

/// Starts the watch, the pump that turns it into state, the publisher of manifests, the wait for
/// the first walk of a large tree, and the measurements the screens asked for.
fn start_background(
    environment: &Arc<Environment>,
    settings: &HostSettings,
    first_scan: Option<FirstScan>,
) {
    let root = environment.files.root().root_path().to_path_buf();
    let config = WatchConfig {
        mode: settings.watch,
        quiet: WatchConfig::QUIET,
        longest: WatchConfig::LONGEST,
        rescan: settings.limits.rescan,
    };
    let observed = Arc::downgrade(environment);
    let admission: crate::watch::Admission = Arc::new(move |change| {
        if let Some(environment) = observed.upgrade() {
            environment.admit_directory_change(&change);
        }
    });
    let (watcher, mut changes) = Watcher::start_observed(
        &root,
        (first_scan.is_none() && environment.index.state().is_full())
            || !environment.git_watches.is_empty(),
        config,
        Some(admission),
    );
    if watcher.is_degraded() {
        environment.index.give_up(crate::index::LiveReason::Watches);
        watcher.disable_recursive();
        let mut facts = lock(&environment.facts);
        facts.index = environment.index.state().mode();
        facts.index_reason = environment.index.state().reason();
    }
    for watch in lock(&environment.watches).values() {
        watcher.watch_dir(&watch.dir.under(&root));
    }
    *lock(&environment.watcher) = Some(watcher);
    if !environment.git_watches.is_empty() {
        let current = Arc::clone(environment);
        lock(&environment.tasks).push(tokio::spawn(async move {
            current.git_watches.changed(&current, &[], true).await;
        }));
    }
    if first_scan.is_none() {
        environment.seed_file_states();
    }

    let weak = Arc::downgrade(environment);
    let pump = tokio::spawn(async move {
        while let Some(change) = changes.recv().await {
            let Some(environment) = weak.upgrade() else {
                return;
            };
            environment.changed(change).await;
        }
    });
    let weak = Arc::downgrade(environment);
    let publish = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PUBLISH_POLL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some(environment) = weak.upgrade() else {
                return;
            };
            let Ok(host) = environment.inner() else {
                return;
            };
            if environment
                .publisher
                .due(environment.index.is_dirty())
                .is_some()
                && let Err(error) = environment.publish_manifest(host.blobs.as_ref()).await
            {
                tracing::debug!(%error, "a manifest could not be published; it is tried again");
            }
        }
    });
    let mut tasks = vec![pump, publish];
    if let Some(walk) = first_scan {
        let weak = Arc::downgrade(environment);
        tasks.push(tokio::spawn(async move {
            let outcome = walk.await;
            let Some(environment) = weak.upgrade() else {
                return;
            };
            match outcome {
                Ok(Ok(_)) => {
                    environment.scanned.store(true, Ordering::SeqCst);
                    environment.seed_file_states();
                }
                // The next rescan walks the tree again.
                Ok(Err(error)) => tracing::warn!(%error, "the tree could not be walked"),
                Err(error) => tracing::warn!(%error, "the walk of the tree was cut short"),
            }
            environment.follow_index_with_watches();
            environment.announce_facts().await;
        }));
    }
    lock(&environment.tasks).extend(tasks);
    environment.start_reporters();
}

/// Progress that nobody listens to, for the commands whose caller does not report it.
#[must_use]
pub fn no_progress() -> NoProgress {
    NoProgress
}

// Browsers and profiles are served by the environment's host, outside its workspace.
impl Environment {
    /// The browser registry of this environment.
    #[must_use]
    pub fn browsers(&self) -> &Arc<crate::browsers::Browsers> {
        &self.browsers
    }
    /// The display registry shared by browser windows and the VNC door.
    #[must_use]
    pub fn displays(&self) -> &Arc<crate::displays::Displays> {
        &self.displays
    }
}

impl EnvHost {
    /// Looks again for browser and display programs and reports the newly found facts.
    pub async fn screen_look(
        &self,
        body: &orch_link_contract::browser::ScreenLook,
    ) -> std::result::Result<
        orch_link_contract::browser::ScreenLookResult,
        crate::browsers::BrowserError,
    > {
        let environment = self
            .environment(body.environment_id)
            .map_err(|_| crate::browsers::BrowserError::NotFound("environment".to_owned()))?;
        let mut result = environment.browsers.look().await?;
        result.displays = environment.displays.list().await;
        let facts = {
            let mut facts = lock(&environment.facts);
            facts.browser = result.browser.clone();
            facts.vnc = result.vnc.clone();
            facts.looked_at = Some(environment.browsers.looked_at());
            facts.clone()
        };
        self.inner
            .reports
            .say(environment.id, Report::Facts(Box::new(facts)))
            .await;
        Ok(result)
    }

    /// Starts or joins the browser of one kept profile.
    pub async fn browser_open(
        &self,
        body: &orch_link_contract::browser::BrowserOpen,
    ) -> std::result::Result<
        orch_link_contract::browser::BrowserOpenResult,
        crate::browsers::BrowserError,
    > {
        let environment = self
            .environment(body.environment_id)
            .map_err(|_| crate::browsers::BrowserError::NotFound("environment".to_owned()))?;
        let opened = environment.browsers.open(body).await?;
        if let Some(browser) = environment
            .browsers
            .get(opened.running.unwrap_or(body.browser_id))
            .await
            && let Some(pid) = browser.owned_pid()
        {
            let _ = environment
                .processes
                .register_root(pid, crate::processes::RootKind::Server);
        }
        Ok(opened)
    }

    /// Takes in a browser an agent started locally.
    pub async fn browser_adopt(
        &self,
        body: &orch_link_contract::browser::BrowserAdopt,
    ) -> std::result::Result<
        orch_link_contract::browser::BrowserAdoptResult,
        crate::browsers::BrowserError,
    > {
        self.environment(body.environment_id)
            .map_err(|_| crate::browsers::BrowserError::NotFound("environment".to_owned()))?
            .browsers
            .adopt(body)
            .await
    }

    /// Asks a known browser to quit and stops only a process Orch owns.
    pub async fn browser_close(
        &self,
        body: &orch_link_contract::browser::BrowserClose,
    ) -> std::result::Result<(), crate::browsers::BrowserError> {
        for environment in self.environments() {
            if environment.browsers.get(body.browser_id).await.is_some() {
                return environment.browsers.close(body.browser_id).await;
            }
        }
        Ok(())
    }

    /// Changes a named profile while it is unused.
    pub async fn browser_profile(
        &self,
        body: &orch_link_contract::browser::BrowserProfileCommand,
    ) -> std::result::Result<
        orch_link_contract::browser::BrowserProfileResult,
        crate::browsers::BrowserError,
    > {
        self.environment(body.environment_id)
            .map_err(|_| crate::browsers::BrowserError::NotFound("environment".to_owned()))?
            .browsers
            .profile(body)
            .await
    }
}

// A browser with a window asks for one virtual screen, without changing its workspace.
impl EnvHost {
    /// Opens the display used by a browser window in a managed environment.
    pub async fn display_open(
        &self,
        body: &orch_link_contract::browser_window::DisplayOpen,
    ) -> std::result::Result<
        orch_link_contract::browser_window::DisplayOpenResult,
        crate::displays::DisplayError,
    > {
        let environment = self
            .environment(body.environment_id)
            .map_err(|_| crate::displays::DisplayError::NotFound)?;
        environment
            .displays
            .open(body.display_id, &body.geometry)
            .await
    }
}

// Process watchers receive a new snapshot immediately after a successful signal.
impl EnvHost {
    async fn report_after_signal(&self, id: Id) -> Result<()> {
        let environment = self.environment(id)?;
        if !lock(&environment.wanted).is_some_and(|wanted| wanted.procs) {
            return Ok(());
        }
        let target = Arc::clone(&environment);
        let listed = blocking(move || Ok(target.processes.list())).await?;
        self.inner
            .reports
            .say(
                id,
                Report::Procs {
                    as_of: self.inner.clock.time(),
                    processes: listed.into_iter().map(|listed| listed.info).collect(),
                },
            )
            .await;
        Ok(())
    }
}
