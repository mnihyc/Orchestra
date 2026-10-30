//! Session (new, resumed, forked or imported), turn (with model and effort), approval and tool operations; the instructions given with a session, and where the agent's notes are kept and by whom.
//!
//! A **session** is one conversation inside a harness process. It is opened with everything the harness
//! needs to run it: where it works, which model at which effort, how it is confined, the instructions it is
//! given, how it reaches the model, what carries out its tool calls, which MCP servers it has, and where its
//! notes are kept and by whom. That is [`SessionOpen`] on the link, which still names the tunnels it will use
//! by ticket, and [`SessionSpec`] for the adapter, which has them as local addresses under one
//! `session_key`. The two share the same [`SessionSettings`].
//!
//! A **turn** is one input answered: [`TurnStart`] carries the text and the images, and states the model,
//! the effort, the parameters, the approval mode and the sandbox mode explicitly, every time. [`TurnSteer`]
//! adds to a running turn. An approval is answered with an [`ApprovalDecision`], and a call of one of Orch's
//! own tools with a result or a [`ToolError`].
//!
//! Where an agent's **notes** are kept is the same for every harness: `.orch/memory` of the thread's
//! directory ([`memory_directory`]). Who keeps them depends on the setting and on what the harness can do
//! there ([`MemoryPlan::decide`]).

use std::collections::BTreeMap;

use orch_types::{Hash, Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The empty control directory at the verified workspace root, excluded from workspace content.
pub const INSTRUCTION_ROOT_MARKER: &str = ".orch-root";

use crate::descriptor::{ApprovalMode, ArrangementType, MemoryKind, SandboxMode, SideReading};
use crate::events::OrchTool;
use crate::history::{ArchiveRef, HistoryRef, HistoryReport, SessionArchive};
/// Native limits use the same values the thread's parameter reader shows.
pub use orch_api_contract::settings::SubagentLimits;

/// The identifier of a thread.
pub type ThreadId = Id;
/// The identifier of a turn.
pub type TurnId = Id;

/// Where, in a thread's directory, an agent's notes are kept.
pub const MEMORY_SUBDIRECTORY: &str = ".orch/memory";
/// The file of that directory that is the index of the notes: one line for each note.
pub const MEMORY_INDEX_FILE: &str = "MEMORY.md";

/// The directory of the notes of a thread whose directory is `thread_directory`.
#[must_use]
pub fn memory_directory(thread_directory: &str) -> String {
    format!(
        "{}/{MEMORY_SUBDIRECTORY}",
        thread_directory.trim_end_matches('/')
    )
}

/// How a session comes to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// A conversation that begins now.
    New,
    /// A conversation the harness holds, continued by its id.
    Resume,
    /// A copy of a session of the same harness process, to a thread of its own or a side thread.
    Fork,
    /// A native archive of another harness process of this kind.
    ImportNative,
    /// A neutral history.
    ImportNeutral,
    /// A session the harness already had, taken as a thread.
    Adopt,
}

/// An attached mount, as a harness is told of it: a further root for its sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MountGrant {
    /// The mount's name.
    pub name: String,
    /// Where it is inside the environment.
    pub path: String,
    /// Whether it may not be written.
    pub read_only: bool,
}

/// Who keeps the notes, as a session is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteKeeper {
    /// The harness, by a memory of its own that the adapter points at the directory.
    Harness,
    /// The agent, with its tools.
    Agent,
}

/// Where an agent's notes are kept and by whom, as a session is told. A session that is told nothing of
/// notes has `null` in its place, and a memory of the harness's own is then to be off.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SessionMemory {
    /// The directory, absolute.
    pub directory: String,
    /// Who keeps what is in it.
    pub kept_by: NoteKeeper,
}

/// What `agent.memory` is set to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemorySetting {
    /// The harness keeps the notes where it can, and the agent where the harness cannot.
    #[default]
    On,
    /// The harness keeps them where it can; where it cannot, nothing is said to the agent about notes.
    Harness,
    /// Nobody keeps them, and a memory of the harness's own is off.
    Off,
}

/// Who keeps the notes of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKeeper {
    /// Its harness, by a memory of its own.
    Harness,
    /// The agent, with its tools: best effort.
    Agent,
    /// Nobody.
    Nobody,
}

/// Why the harness does not keep the notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWhy {
    /// `agent.memory` is off.
    SettingOff,
    /// `agent.memory` asks for the harness alone, and this one cannot.
    SettingHarnessOnly,
    /// The harness has no memory.
    HarnessNone,
    /// It has one for its whole home, which is always off.
    HarnessHome,
    /// It runs with its provider, away from the directory.
    WithProvider,
}

/// What a thread's notes come to: who keeps them, whether its harness could, and if it does not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MemoryPlan {
    /// Who keeps the notes.
    pub kept_by: MemoryKeeper,
    /// Whether this harness, placed as it is for this thread, has a memory it can keep there.
    pub native: bool,
    /// Why the harness does not keep them; `null` where it does.
    pub why: Option<MemoryWhy>,
}

impl MemoryPlan {
    /// What the notes of a thread come to, given the setting, what the adapter declares
    /// (`capabilities.memory`), and whether the harness runs in the thread's environment (and not with its
    /// provider, away from the directory).
    ///
    /// The harness keeps them only when it has a memory that can be pointed at one directory and runs
    /// where that directory is, and the setting does not turn that off. Where it cannot, the agent keeps
    /// them with its tools if the setting is `on`; the setting `harness` leaves them to nobody, and `off`
    /// switches everything off.
    #[must_use]
    pub const fn decide(
        setting: MemorySetting,
        declared: MemoryKind,
        harness_in_environment: bool,
    ) -> Self {
        let native = matches!(declared, MemoryKind::Directory) && harness_in_environment;
        let cannot = match declared {
            MemoryKind::None => MemoryWhy::HarnessNone,
            MemoryKind::Home => MemoryWhy::HarnessHome,
            MemoryKind::Directory => MemoryWhy::WithProvider,
        };
        match setting {
            MemorySetting::Off => Self {
                kept_by: MemoryKeeper::Nobody,
                native,
                why: Some(MemoryWhy::SettingOff),
            },
            _ if native => Self {
                kept_by: MemoryKeeper::Harness,
                native,
                why: None,
            },
            MemorySetting::Harness => Self {
                kept_by: MemoryKeeper::Nobody,
                native,
                why: Some(MemoryWhy::SettingHarnessOnly),
            },
            MemorySetting::On => Self {
                kept_by: MemoryKeeper::Agent,
                native,
                why: Some(cannot),
            },
        }
    }

    /// What a session is told for a thread whose directory is `thread_directory`: `None` when nobody keeps
    /// the notes.
    #[must_use]
    pub fn session_memory(&self, thread_directory: &str) -> Option<SessionMemory> {
        let kept_by = match self.kept_by {
            MemoryKeeper::Harness => NoteKeeper::Harness,
            MemoryKeeper::Agent => NoteKeeper::Agent,
            MemoryKeeper::Nobody => return None,
        };
        Some(SessionMemory {
            directory: memory_directory(thread_directory),
            kept_by,
        })
    }
}

/// The members of a session that are the same on the link and for the adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSettings {
    /// The agent profile.
    pub agent_id: Id,
    /// The thread.
    pub thread_id: ThreadId,
    /// The segment of the thread this session serves.
    pub segment_id: Id,
    /// How the session comes to be.
    pub mode: SessionMode,
    /// For `resume` and `adopt`: the harness's id for the session. For `fork`: the session of the same
    /// harness process that this one is forked from.
    pub native_session_ref: Option<String>,
    /// For a fork and the two imports: only through this turn.
    pub upto_turn_ordinal: Option<u32>,
    /// The environment's workspace root, or the registered directory of a personal environment.
    pub workspace_root: String,
    /// Whether the environment host verified its empty, root-only instruction marker for this open.
    pub instruction_root_marker: bool,
    /// The thread's directory in its environment, absolute: where its work is, and what the harness's own
    /// sandbox may write in the mode `workspace`. It bounds nothing that people reach through the thread.
    pub directory: String,
    /// Where the agent starts, inside the directory.
    pub cwd: String,
    /// The attached mounts: further roots for the harness's sandbox, writable unless read-only.
    pub mounts: Vec<MountGrant>,
    /// The model's routing name, sent explicitly with every session and turn.
    pub model: String,
    /// The reasoning effort; `null` takes none.
    pub effort: Option<String>,
    /// The values of the adapter's parameters that are shown and sent: each is the thread's own
    /// choice where it chose and it is still open, and otherwise what the model starts with (the
    /// service tier, the access program, the context). Beside them, under
    /// [`MODEL_WINDOW_KEY`](crate::parameters::MODEL_WINDOW_KEY), the model's own window where
    /// its entry has one and the adapter publishes the context.
    pub params: Json,
    /// How the harness treats actions it does not already trust.
    pub approval_mode: ApprovalMode,
    /// The exact models the input's author may use on this provider. Auto reviewers stay within this list.
    #[serde(default)]
    pub available_models: Vec<String>,
    /// Limits over the harness's own sub-agents, resolved separately from model parameters.
    #[serde(default)]
    pub subagent_limits: SubagentLimits,
    /// What the harness's own sandbox lets commands do.
    pub sandbox_mode: SandboxMode,
    /// The preamble, then the standing instructions. Nothing of the directory's instruction file: the agent
    /// reads that.
    pub instructions: String,
    /// Where the agent's notes are kept and by whom; `null` when `agent.memory` is off.
    pub memory: Option<SessionMemory>,
    /// The arrangement's type; the members of the provider and the executor say how it is carried out.
    pub arrangement: ArrangementType,
    /// The tools of the `orch` server offered beside whatever carries out the agent's other calls; empty
    /// where the thread's settings or its environment leave it none, and then no `orch` server is given at
    /// all.
    pub orch_tools: Vec<OrchTool>,
    /// Current narrowed input schemas; a changed catalogue is reloaded before the next turn.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub orch_tool_schemas: BTreeMap<String, Json>,
    /// Whether the frames of the harness's own protocol are kept as `raw.frame`.
    pub raw_events: bool,
    /// For a side conversation: how it reads without acting. An adapter enforces a mechanism it declared in
    /// `side_thread_modes` at every open and resume, and refuses the session for one it did not.
    #[serde(default)]
    pub side_reading: Option<SideReading>,
    /// What the people who manage the environment answered about a configuration of the harness's own in the
    /// workspace, and for which files; `None` while nobody was asked. An adapter that finds such a configuration
    /// on the way from the workspace root to where the session works refuses with [`WORKSPACE_CONFIGURATION`]
    /// ([`crate::adapter::AdapterError::workspace_configuration`]) unless the answer is for the files it finds now:
    /// when they change, the question is asked again. A directory that holds nothing is not a configuration.
    #[serde(default)]
    pub workspace_configuration: Option<WorkspaceConfiguration>,
}

/// The code of the refusal of a workspace that holds a configuration of the harness's own nobody has answered for.
pub const WORKSPACE_CONFIGURATION: &str = "workspace_configuration";

/// How many entries of a workspace's own harness configuration are read for [`configuration_files`].
const CONFIGURATION_ENTRIES: usize = 512;

/// How much of each file of it is read for [`configuration_files`].
const CONFIGURATION_BYTES: u64 = 1 << 20;

/// The hash of what a workspace's own harness configuration holds at `places` (relative to `root`): every file and
/// link under them, by path and content, so that an answer holds for these files and the question is asked again
/// when they change. The one way every adapter names what it found, so that the system and the adapter agree. A
/// large tree counts by what was read: at most 512 entries, and a mebibyte of each file.
#[must_use]
pub fn configuration_files(root: &std::path::Path, places: &[String]) -> String {
    use std::io::Read;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut pending: Vec<std::path::PathBuf> =
        places.iter().map(|place| root.join(place)).collect();
    let mut visited = 0_usize;
    while let Some(path) = pending.pop() {
        visited += 1;
        if visited > CONFIGURATION_ENTRIES {
            entries.push(("…".to_owned(), b"more".to_vec()));
            break;
        }
        let relative = path.strip_prefix(root).map_or_else(
            |_| path.display().to_string(),
            |relative| relative.display().to_string(),
        );
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&path)
                    .map(|target| target.display().to_string())
                    .unwrap_or_default();
                entries.push((relative, format!("link {target}").into_bytes()));
            }
            Ok(meta) if meta.is_dir() => {
                if let Ok(read) = std::fs::read_dir(&path) {
                    pending.extend(read.flatten().map(|entry| entry.path()));
                }
            }
            Ok(_) => {
                let mut content = Vec::new();
                if let Ok(file) = std::fs::File::open(&path) {
                    let _ = file.take(CONFIGURATION_BYTES).read_to_end(&mut content);
                }
                entries.push((relative, content));
            }
            Err(_) => entries.push((relative, b"unreadable".to_vec())),
        }
    }
    entries.sort();
    let mut all = Vec::new();
    for (path, content) in entries {
        all.extend_from_slice(path.as_bytes());
        all.push(0);
        all.extend_from_slice(&content);
        all.push(0);
    }
    Hash::of(&all).to_string()
}

/// The answer about a workspace's own harness configuration, as the environment remembers it for the harness.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceConfiguration {
    /// Trust or Keep it out.
    pub answer: ConfigTrust,
    /// The hash of the files the answer was given for, as the adapter's refusal named it.
    pub files: String,
}

/// What a person answered about a workspace's own harness configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigTrust {
    /// The harness uses it, as it does in a folder it trusts.
    Trust,
    /// The harness works without it, as it does in a folder it does not trust.
    KeepOut,
}

/// A request to the gateway to open a tunnel, written into a command in place of the signed ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    /// Model calls through the provider role.
    Llm,
    /// The harness's connection to its executor.
    Exec,
    /// An MCP server of the environment.
    Mcp,
}

/// What a ticket reference asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketRequest {
    /// The kind of tunnel.
    pub kind: TicketKind,
    /// Further members that the kind needs.
    #[serde(flatten)]
    pub rest: BTreeMap<String, Json>,
}

/// `{ "$ticket": { "kind": "llm" } }`: the API stores this reference, and the gateway replaces it by a
/// freshly signed tunnel ticket when the command is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketRef {
    /// The request.
    #[serde(rename = "$ticket")]
    pub ticket: TicketRequest,
}

impl TicketRef {
    /// A reference to a ticket of `kind` with nothing further asked.
    #[must_use]
    pub fn of(kind: TicketKind) -> Self {
        Self {
            ticket: TicketRequest {
                kind,
                rest: BTreeMap::new(),
            },
        }
    }
}

/// How the harness reaches the model, as `session.open` names it on the link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "path", rename_all = "snake_case")]
pub enum ProviderBinding {
    /// The provider's login, used directly in the home where it is kept.
    Native {
        /// The provider.
        provider_id: Id,
    },
    /// Orch's forwarder, which adds the key.
    Forwarded {
        /// The provider.
        provider_id: Id,
        /// The wire protocol the harness speaks to the forwarder.
        protocol: String,
        /// The tunnel to the provider role, when it is not beside the harness.
        ticket: TicketRef,
    },
    /// A copy of the credential, which arrives by `agent.credential.set`.
    Shared {
        /// The provider.
        provider_id: Id,
        /// The form of the copy.
        form: String,
    },
    /// The credential itself, which arrives by `agent.credential.set`.
    Handover {
        /// The provider.
        provider_id: Id,
        /// The form.
        form: String,
    },
}

/// What carries out the harness's tool calls, as `session.open` names it on the link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecBinding {
    /// The executor; `null` when the harness is in the environment and carries out its own tools.
    pub executor: Option<String>,
    /// For an executor that brings its tools as an MCP server: the tool set its environment's offers come
    /// to for this harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_set: Option<String>,
    /// The environment.
    pub environment_id: Id,
    /// Its generation.
    pub generation: i64,
    /// The tunnel to the executor; none for the tool set `custom`, which has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<TicketRef>,
}

/// An MCP server of the environment, as `session.open` names it on the link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpBinding {
    /// The server's name.
    pub name: String,
    /// The tunnel to it.
    pub ticket: TicketRef,
}

/// The body of the command `session.open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpen {
    /// What is the same for the adapter.
    #[serde(flatten)]
    pub settings: SessionSettings,
    /// For `import_native`: the archive.
    pub archive: Option<ArchiveRef>,
    /// For `import_neutral`: the history, as JSON Lines of items, the main stream, oldest first.
    pub history: Option<HistoryRef>,
    /// How the harness reaches the model.
    pub provider: ProviderBinding,
    /// What carries out its tool calls.
    pub exec: ExecBinding,
    /// The environment's MCP servers it has.
    pub mcp: Vec<McpBinding>,
}

/// How the harness reaches the model, as the adapter is told: the addresses are the connector's own, under
/// the session's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "path", rename_all = "snake_case")]
pub enum ProviderAccess {
    /// The provider's login, used directly in the home where it is kept.
    Native {
        /// The provider.
        provider_id: Id,
    },
    /// The forwarder at `/llm/<session key>/…` of the connector's listener.
    Forwarded {
        /// The provider.
        provider_id: Id,
        /// The wire protocol the harness speaks to it.
        protocol: String,
    },
    /// A copy of the credential, which arrives by `set_credential`.
    Shared {
        /// The provider.
        provider_id: Id,
        /// The form of the copy.
        form: String,
    },
    /// The credential itself, which arrives by `set_credential`.
    Handover {
        /// The provider.
        provider_id: Id,
        /// The form.
        form: String,
    },
}

/// What carries out the harness's tool calls, as the adapter is told: reached at `/exec/<session key>` for
/// a protocol of the harness's own, and as the MCP server named `exec` for a tool set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecAccess {
    /// The executor; `null` when the harness is in the environment and carries out its own tools.
    pub executor: Option<String>,
    /// The tool set, for an executor that brings its tools as an MCP server; `custom` has no server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_set: Option<String>,
    /// The environment.
    pub environment_id: Id,
    /// Its generation.
    pub generation: i64,
}

/// A file the connector fetched for the adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocalFile {
    /// Where it is on this machine.
    pub path: String,
    /// Its size in bytes.
    pub size: i64,
}

/// What the adapter is given to open a session ([`crate::adapter::AgentAdapter::open_session`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSpec {
    /// What is the same on the link.
    #[serde(flatten)]
    pub settings: SessionSettings,
    /// 128 random bits as 32 lowercase hexadecimal digits: it names this session in the addresses of the
    /// connector's listener ([`crate::loopback`]).
    pub session_key: String,
    /// For `import_native`: the archive, fetched.
    pub archive: Option<SessionArchive>,
    /// For `import_neutral`: the history, fetched.
    pub history: Option<LocalFile>,
    /// How the harness reaches the model.
    pub provider: ProviderAccess,
    /// What carries out its tool calls.
    pub exec: ExecAccess,
    /// The names of the environment's MCP servers it has, each at `/mcp/<session key>/<name>`.
    pub mcp: Vec<String>,
    /// The skills it is given, laid out on this machine for its thread; none when it is given none.
    #[serde(default)]
    pub skills: Option<SessionSkills>,
}

/// The skills a session is given, as the connector lays them out for its thread: each skill's files in
/// `<dir>/skills/<name>/`, with `SKILL.md` at the top. Each program's adapter takes the directory in its own way
/// (a capability root, a plugin it makes of the root as the session opens). The directory is the thread's own and
/// stays where it is while what is in it changes from version to version; nothing of it is in a home another thread
/// shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSkills {
    /// The thread's directory of skills.
    pub dir: String,
    /// The skills in it, by the names their `SKILL.md` gives.
    pub names: Vec<String>,
}

/// The result of opening a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpened {
    /// The harness's id for the session.
    pub native_session_ref: String,
    /// Whether it continued a session that existed.
    pub resumed: bool,
    /// For an import: what was kept, converted and dropped.
    pub import_report: Option<HistoryReport>,
    /// The instruction files the harness reads for this session, in the order it reads them: as the harness
    /// reported them, or as its documented loading rule finds them. Empty where the adapter knows none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instruction_sources: Vec<InstructionFile>,
    /// The native permission reviewer's model for this session, where Auto was requested.
    #[serde(default)]
    pub approval_reviewer: Option<orch_api_contract::settings::ApprovalReviewer>,
}

/// How an adapter knows that its harness reads an instruction file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionEvidence {
    /// The harness said so when the session was opened.
    Reported,
    /// The harness's documented loading rule says so; the harness said nothing.
    DocumentedRule,
}

/// What kind of instruction file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionFileKind {
    /// A file of instructions: a directory's, an override, a person's own, a rule, an import.
    File,
    /// The index of the agent's notes.
    Memory,
}

/// One instruction file a harness reads, as its adapter knows it. A file of the person's own home carries no path,
/// only its label: what is reported never opens anybody's home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstructionFile {
    /// What kind.
    pub kind: InstructionFileKind,
    /// What it is, in a few words that name no home: "AGENTS.md", "the person's own AGENTS.md".
    pub label: String,
    /// Where it is, absolute, in the environment the session works in; none for a file of the person's home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether it lies in the home of the person whose harness this is.
    #[serde(default)]
    pub home: bool,
    /// How the adapter knows it is read.
    pub evidence: InstructionEvidence,
    /// How many bytes it holds; none where that could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// How many of them the harness takes in; none where that is not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_bytes: Option<u64>,
    /// How many it leaves out by its own bound; none where that is not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_bytes: Option<u64>,
    /// How many lines it holds; none where that could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u64>,
    /// How many of them the harness takes in; none where that is not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_lines: Option<u64>,
    /// Why a value is not known, or how a size was derived ("derived from the 32 KiB project budget").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The native reason for a reported load, such as an include or a nested directory traversal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_reason: Option<String>,
    /// The file which imported this one, only inside the environment's workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
    /// The separate native line limit of an index, where documented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_limit: Option<u64>,
    /// The separate native byte limit, where documented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_limit: Option<u64>,
}

/// How many lines a text holds: its line breaks, and a last line without one.
fn line_count(body: &[u8]) -> u64 {
    let breaks = body.iter().filter(|byte| **byte == b'\n').count();
    let last = usize::from(body.last().is_some_and(|byte| *byte != b'\n'));
    u64::try_from(breaks + last).unwrap_or(u64::MAX)
}

impl InstructionFile {
    /// A file at `path` read with `evidence`, measured where it can be read here: its bytes and lines, and what is
    /// left of a bound the harness puts on what it reads (`budget`: the bytes still free of it when this file is
    /// read), where it has one. A file that cannot be read keeps its sizes unknown, with the reason.
    #[must_use]
    pub fn measured(
        label: impl Into<String>,
        path: &std::path::Path,
        evidence: InstructionEvidence,
        budget: Option<u64>,
    ) -> Self {
        let mut file = Self {
            kind: InstructionFileKind::File,
            label: label.into(),
            path: Some(path.display().to_string()),
            home: false,
            evidence,
            bytes: None,
            loaded_bytes: None,
            cut_bytes: None,
            lines: None,
            loaded_lines: None,
            reason: None,
            load_reason: None,
            parent_path: None,
            line_limit: None,
            byte_limit: budget,
        };
        match std::fs::read(path) {
            Ok(body) => {
                let bytes = u64::try_from(body.len()).unwrap_or(u64::MAX);
                let lines = line_count(&body);
                file.bytes = Some(bytes);
                file.lines = Some(lines);
                match budget {
                    Some(budget) => {
                        let loaded = bytes.min(budget);
                        let kept = usize::try_from(loaded)
                            .unwrap_or(usize::MAX)
                            .min(body.len());
                        file.loaded_bytes = Some(loaded);
                        file.cut_bytes = Some(bytes - loaded);
                        file.loaded_lines = Some(line_count(&body[..kept]));
                        if loaded < bytes {
                            file.reason = Some(format!(
                                "cut at the harness's bound of {budget} bytes, derived from its configuration"
                            ));
                        }
                    }
                    None => {
                        file.loaded_bytes = Some(bytes);
                        file.cut_bytes = Some(0);
                        file.loaded_lines = Some(lines);
                    }
                }
            }
            Err(error) => {
                file.reason = Some(format!("the file could not be read here: {}", error.kind()));
            }
        }
        file
    }

    /// A file whose sizes are not read, with why: one of a home, or one in an environment the adapter does not read.
    #[must_use]
    pub fn unmeasured(
        label: impl Into<String>,
        path: Option<&std::path::Path>,
        evidence: InstructionEvidence,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind: InstructionFileKind::File,
            label: label.into(),
            path: path.map(|path| path.display().to_string()),
            home: false,
            evidence,
            bytes: None,
            loaded_bytes: None,
            cut_bytes: None,
            lines: None,
            loaded_lines: None,
            reason: Some(reason.into()),
            load_reason: None,
            parent_path: None,
            line_limit: None,
            byte_limit: None,
        }
    }

    /// The same file as one of the person's own home: its label kept, its path not given.
    #[must_use]
    pub fn of_home(mut self) -> Self {
        self.path = None;
        self.parent_path = None;
        self.home = true;
        self
    }
}

/// The directories from `root` down to `cwd`, both ends included, where `cwd` lies under `root`; none otherwise.
#[must_use]
pub fn directory_chain(root: &str, cwd: &str) -> Option<Vec<std::path::PathBuf>> {
    let root = std::path::Path::new(root);
    let relative = std::path::Path::new(cwd).strip_prefix(root).ok()?;
    let mut directory = root.to_path_buf();
    let mut chain = vec![directory.clone()];
    for part in relative.components() {
        directory.push(part);
        chain.push(directory.clone());
    }
    Some(chain)
}

/// An image that goes with a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImageInput {
    /// The image, by its hash.
    pub blob: Hash,
    /// Its type, such as `image/png`.
    pub media_type: String,
    /// Its name.
    pub name: String,
    /// Where the connector put it on this machine. Binary content is never sent inline: it is a file the
    /// adapter reads. Absent on the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A turn to start: the body of the command `turn.start`, and what the adapter is given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnStart {
    /// The thread.
    pub thread_id: ThreadId,
    /// The turn.
    pub turn_id: TurnId,
    /// The text, as the engine built it from the input.
    pub text: String,
    /// The images, when the agent and the model take them.
    pub images: Vec<ImageInput>,
    /// The model's routing name, passed to the harness on every turn.
    pub model: String,
    /// The reasoning effort for this turn; `null` takes none.
    pub effort: Option<String>,
    /// The values of the adapter's parameters that are shown and sent, resolved for this turn as
    /// they are for a session, with the model's own window beside them.
    pub params: Json,
    /// How the harness treats actions it does not already trust.
    pub approval_mode: ApprovalMode,
    /// What the harness's own sandbox lets commands do.
    pub sandbox_mode: SandboxMode,
    /// Whether the turn plans: the agent reads and proposes a plan, and changes nothing until a person
    /// proceeds. Only given to a harness that declares `plan_mode`; the approval mode stays as it is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan: bool,
}

/// Something to add to a running turn: the body of the command `turn.steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnSteer {
    /// The thread.
    pub thread_id: ThreadId,
    /// The turn that is running.
    pub turn_id: TurnId,
    /// The text.
    pub text: String,
    /// The images.
    pub images: Vec<ImageInput>,
}

/// What a person answered to an approval. Which form it takes follows from the request's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ApprovalDecision {
    /// An answer to a request that has options: the id of the option chosen.
    Option {
        /// The option.
        option_id: String,
        /// A note.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Answers to the questions of a `user_input` request: the choices, by question id.
    Answers {
        /// The choices, by question id.
        answers: BTreeMap<String, Vec<String>>,
        /// A note.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// An answer to an MCP server's request for input.
    Elicitation {
        /// What the person did with it.
        action: ElicitationAction,
        /// What they gave, when they accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Json>,
    },
}

/// What a person did with an MCP server's request for input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    /// They gave what was asked.
    Accept,
    /// They refused to.
    Decline,
    /// They dismissed it.
    Cancel,
}

/// Why a call of one of Orch's tools did not succeed, as the harness is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolError {
    /// A code.
    pub code: String,
    /// What to do instead, in one line.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_instruction_file_is_measured_with_what_a_bound_leaves_of_it_and_a_home_file_keeps_no_path()
     {
        let directory =
            std::env::temp_dir().join(format!("orch-instruction-file-{}", orch_types::new_id()));
        std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
        let path = directory.join("AGENTS.md");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap_or_else(|error| panic!("{error}"));
        let whole =
            InstructionFile::measured("AGENTS.md", &path, InstructionEvidence::Reported, None);
        assert_eq!(
            (whole.bytes, whole.loaded_bytes, whole.cut_bytes),
            (Some(14), Some(14), Some(0))
        );
        assert_eq!((whole.lines, whole.loaded_lines), (Some(3), Some(3)));
        let cut =
            InstructionFile::measured("AGENTS.md", &path, InstructionEvidence::Reported, Some(8));
        assert_eq!(
            (cut.loaded_bytes, cut.cut_bytes, cut.loaded_lines),
            (Some(8), Some(6), Some(2))
        );
        assert!(cut.reason.is_some(), "a cut says why");
        let home = whole.of_home();
        assert!(home.home && home.path.is_none());
        let missing = InstructionFile::measured(
            "AGENTS.md",
            &directory.join("none"),
            InstructionEvidence::DocumentedRule,
            None,
        );
        assert_eq!(missing.bytes, None);
        assert!(missing.reason.is_some(), "an unknown size says why");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A session as the link carries it, with the tunnels named by ticket.
    fn open() -> Json {
        json!({
          "agent_id": "0199abcd-1234-7000-8000-000000000001",
          "thread_id": "0199abcd-1234-7000-8000-000000000002",
          "segment_id": "0199abcd-1234-7000-8000-000000000003",
          "mode": "import_native",
          "native_session_ref": null,
          "archive": { "format": "codex-rollout-v1", "blob": "0".repeat(64), "size": 37309 },
          "history": null,
          "upto_turn_ordinal": 12,
          "workspace_root": "/workspace", "instruction_root_marker": true,
          "directory": "/workspace/web", "cwd": "/workspace/web/api",
          "mounts": [ { "name": "src", "path": "/data/src", "read_only": true } ],
          "model": "gpt-6-sol", "effort": "high", "params": {}, "approval_mode": "on_request", "sandbox_mode": "workspace",
          "available_models": [], "subagent_limits": {"at_once":8,"depth":1}, "side_reading": null, "workspace_configuration": null,
          "instructions": "You are working inside Orch.",
          "memory": { "directory": "/workspace/web/.orch/memory", "kept_by": "harness" },
          "arrangement": "separated",
          "provider": { "path": "forwarded", "provider_id": "0199abcd-1234-7000-8000-000000000004",
                        "protocol": "openai-responses", "ticket": { "$ticket": { "kind": "llm" } } },
          "exec": { "executor": "codex-exec-server", "environment_id": "0199abcd-1234-7000-8000-000000000005",
                    "generation": 3, "ticket": { "$ticket": { "kind": "exec" } } },
          "mcp": [ { "name": "tmux", "ticket": { "$ticket": { "kind": "mcp" } } } ],
          "orch_tools": ["list_shared", "read_shared", "search_shared", "download_shared", "edit_document",
                         "send_message", "announce", "request_capability"],
          "raw_events": true
        })
    }

    #[test]
    fn the_command_that_opens_a_session_reads_and_writes_as_the_spec_shows_it() {
        let read: SessionOpen = serde_json::from_value(open()).unwrap();
        assert_eq!(read.settings.mode, SessionMode::ImportNative);
        assert_eq!(read.settings.upto_turn_ordinal, Some(12));
        assert_eq!(read.settings.orch_tools.len(), 8);
        assert_eq!(read.settings.orch_tools[5], OrchTool::SendMessage);
        assert_eq!(read.archive.as_ref().unwrap().format, "codex-rollout-v1");
        assert!(
            matches!(read.provider, ProviderBinding::Forwarded { ref protocol, .. } if protocol == "openai-responses")
        );
        assert_eq!(read.exec.executor.as_deref(), Some("codex-exec-server"));
        assert_eq!(read.mcp[0].ticket.ticket.kind, TicketKind::Mcp);
        assert_eq!(serde_json::to_value(&read).unwrap(), open());
    }

    #[test]
    fn what_carries_out_the_tools_has_three_forms_and_the_set_custom_has_no_ticket() {
        let in_the_environment: ExecBinding = serde_json::from_value(
            json!({ "executor": null, "environment_id": "0199abcd-1234-7000-8000-000000000005", "generation": 3 }),
        )
        .unwrap();
        assert!(in_the_environment.executor.is_none() && in_the_environment.ticket.is_none());
        let written = serde_json::to_value(&in_the_environment).unwrap();
        assert_eq!(written["executor"], json!(null));
        assert!(written.get("ticket").is_none());
        let through_a_set: ExecBinding = serde_json::from_value(json!({
            "executor": "orch-exec", "tool_set": "claude-code", "environment_id": "0199abcd-1234-7000-8000-000000000005",
            "generation": 3, "ticket": { "$ticket": { "kind": "mcp" } }
        }))
        .unwrap();
        assert_eq!(through_a_set.tool_set.as_deref(), Some("claude-code"));
        let custom: ExecBinding = serde_json::from_value(json!({
            "executor": "orch-exec", "tool_set": "custom", "environment_id": "0199abcd-1234-7000-8000-000000000005",
            "generation": 3
        }))
        .unwrap();
        assert!(custom.ticket.is_none());
    }

    #[test]
    fn the_four_paths_to_the_model_are_told_apart_by_their_path() {
        let provider = "0199abcd-1234-7000-8000-000000000004";
        for (written, wanted) in [
            (
                json!({ "path": "native", "provider_id": provider }),
                "native",
            ),
            (
                json!({ "path": "shared", "provider_id": provider, "form": "access_token" }),
                "shared",
            ),
            (
                json!({ "path": "handover", "provider_id": provider, "form": "login" }),
                "handover",
            ),
        ] {
            let read: ProviderBinding = serde_json::from_value(written.clone()).unwrap();
            assert_eq!(serde_json::to_value(&read).unwrap(), written);
            assert_eq!(serde_json::to_value(&read).unwrap()["path"], wanted);
        }
        let ticket = TicketRef::of(TicketKind::Llm);
        assert_eq!(
            serde_json::to_value(&ticket).unwrap(),
            json!({ "$ticket": { "kind": "llm" } })
        );
    }

    #[test]
    fn a_ticket_keeps_the_members_its_kind_needs() {
        let ticket: TicketRef =
            serde_json::from_value(json!({ "$ticket": { "kind": "mcp", "server": "tmux" } }))
                .unwrap();
        assert_eq!(ticket.ticket.rest["server"], json!("tmux"));
        assert_eq!(
            serde_json::to_value(&ticket).unwrap(),
            json!({ "$ticket": { "kind": "mcp", "server": "tmux" } })
        );
    }

    #[test]
    fn a_turn_states_the_model_and_the_effort_every_time() {
        let turn: TurnStart = serde_json::from_value(json!({
            "thread_id": "0199abcd-1234-7000-8000-000000000002",
            "turn_id": "0199abcd-1234-7000-8000-000000000006",
            "text": "Fix the redirect.",
            "images": [ { "blob": "0".repeat(64), "media_type": "image/png", "name": "shot.png" } ],
            "model": "gpt-6-sol", "effort": null, "params": { "service_tier": "fast" },
            "approval_mode": "ask", "sandbox_mode": "read_only"
        }))
        .unwrap();
        assert_eq!(turn.effort, None);
        assert!(turn.images[0].path.is_none());
        let written = serde_json::to_value(&turn).unwrap();
        assert_eq!(
            written["effort"],
            json!(null),
            "no effort is written as null, not left out"
        );
        assert!(written["images"][0].get("path").is_none());
    }

    #[test]
    fn an_approval_is_answered_by_an_option_by_answers_or_by_an_action() {
        let option: ApprovalDecision =
            serde_json::from_value(json!({ "option_id": "once", "note": "ok" })).unwrap();
        assert!(
            matches!(option, ApprovalDecision::Option { ref option_id, .. } if option_id == "once")
        );
        let answers: ApprovalDecision =
            serde_json::from_value(json!({ "answers": { "q1": ["Postgres"] } })).unwrap();
        assert!(
            matches!(answers, ApprovalDecision::Answers { ref answers, .. } if answers["q1"] == ["Postgres"])
        );
        let elicitation: ApprovalDecision =
            serde_json::from_value(json!({ "action": "accept", "content": { "name": "x" } }))
                .unwrap();
        assert!(matches!(
            elicitation,
            ApprovalDecision::Elicitation {
                action: ElicitationAction::Accept,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(ApprovalDecision::Elicitation {
                action: ElicitationAction::Cancel,
                content: None
            })
            .unwrap(),
            json!({ "action": "cancel" })
        );
    }

    #[test]
    fn the_notes_of_a_thread_are_in_one_place_whatever_the_harness() {
        assert_eq!(
            memory_directory("/workspace/web"),
            "/workspace/web/.orch/memory"
        );
        assert_eq!(
            memory_directory("/workspace/web/"),
            "/workspace/web/.orch/memory"
        );
        assert_eq!(MEMORY_INDEX_FILE, "MEMORY.md");
    }

    #[test]
    fn who_keeps_the_notes_follows_from_the_setting_and_from_what_the_harness_can_do_where_it_runs()
    {
        use MemoryKeeper::{Agent, Harness, Nobody};
        use MemorySetting::{Harness as HarnessOnly, Off, On};
        use MemoryWhy::*;
        let plan = MemoryPlan::decide;
        // `directory`: the harness keeps them where it runs in the environment; with its provider the agent does.
        let kept = plan(On, MemoryKind::Directory, true);
        assert_eq!((kept.kept_by, kept.native, kept.why), (Harness, true, None));
        let away = plan(On, MemoryKind::Directory, false);
        assert_eq!(
            (away.kept_by, away.native, away.why),
            (Agent, false, Some(WithProvider))
        );
        // `home`: always off; the agent keeps them with its tools.
        let home = plan(On, MemoryKind::Home, true);
        assert_eq!((home.kept_by, home.why), (Agent, Some(HarnessHome)));
        // `none`.
        let none = plan(On, MemoryKind::None, true);
        assert_eq!((none.kept_by, none.why), (Agent, Some(HarnessNone)));
        // The setting `harness`: where the harness cannot, nothing is said to the agent about notes.
        let only = plan(HarnessOnly, MemoryKind::Home, true);
        assert_eq!((only.kept_by, only.why), (Nobody, Some(SettingHarnessOnly)));
        assert_eq!(
            plan(HarnessOnly, MemoryKind::Directory, true).kept_by,
            Harness
        );
        // The setting `off` switches everything off, a memory of the harness's own too.
        let off = plan(Off, MemoryKind::Directory, true);
        assert_eq!(
            (off.kept_by, off.native, off.why),
            (Nobody, true, Some(SettingOff))
        );
    }

    #[test]
    fn a_session_is_told_where_the_notes_are_and_by_whom_or_nothing() {
        let harness = MemoryPlan::decide(MemorySetting::On, MemoryKind::Directory, true);
        assert_eq!(
            harness.session_memory("/workspace/web"),
            Some(SessionMemory {
                directory: "/workspace/web/.orch/memory".into(),
                kept_by: NoteKeeper::Harness
            })
        );
        let agent = MemoryPlan::decide(MemorySetting::On, MemoryKind::None, true);
        assert_eq!(
            agent.session_memory("/w").unwrap().kept_by,
            NoteKeeper::Agent
        );
        assert_eq!(
            MemoryPlan::decide(MemorySetting::Off, MemoryKind::Directory, true)
                .session_memory("/w"),
            None
        );
    }

    #[test]
    fn what_an_adapter_is_given_has_the_session_key_and_no_tickets() {
        let open: SessionOpen = serde_json::from_value(open()).unwrap();
        let spec = SessionSpec {
            settings: open.settings.clone(),
            session_key: "0123456789abcdef0123456789abcdef".into(),
            archive: Some(SessionArchive {
                format: "codex-rollout-v1".into(),
                path: "/tmp/in.jsonl".into(),
                size: 37309,
            }),
            history: None,
            provider: ProviderAccess::Forwarded {
                provider_id: "0199abcd-1234-7000-8000-000000000004".parse().unwrap(),
                protocol: "openai-responses".into(),
            },
            exec: ExecAccess {
                executor: Some("codex-exec-server".into()),
                tool_set: None,
                environment_id: "0199abcd-1234-7000-8000-000000000005".parse().unwrap(),
                generation: 3,
            },
            mcp: vec!["tmux".into()],
            skills: None,
        };
        let written = serde_json::to_value(&spec).unwrap();
        assert!(!written.to_string().contains("$ticket"));
        assert_eq!(written["session_key"], "0123456789abcdef0123456789abcdef");
        let back: SessionSpec = serde_json::from_value(written).unwrap();
        assert_eq!(back, spec);
    }
}
