//! Persist, resume, export, import, fork and resync scripted sessions; notes kept by the harness or left to the agent, as configured.
//!
//! A session is a file in the home of the fake harness, `sessions/<native session ref>.json`, in the
//! format `fake-v1`: the settings it was opened with, its turns, the items it said and the counters
//! that keep their references unique. Everything the fake says is written there when an item is final,
//! so a session that is resumed has its history, a fork is a copy of the file through a turn, an export
//! is the file or its items as lines, and a resync says again what the file holds.
//!
//! The fake's notes are lines of `MEMORY.md` in the directory a session is given for them.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::discovery::NativeSession;
use orch_agent_contract::events::HarnessRequest;
use orch_agent_contract::events::{
    BlobRef, ItemKind, ItemStatus, MAIN_STREAM, TokenUsage, TurnError, TurnOutcome,
};
use orch_agent_contract::history::{Dropped, Fidelity, HistoryReport, KindCounts, ReportBuilder};
use orch_agent_contract::session::{
    ApprovalDecision, MEMORY_INDEX_FILE, SessionMemory, SessionSpec, ThreadId, ToolError,
};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

use crate::events::lock;

/// The format of a native export: the session file itself.
pub const NATIVE_FORMAT: &str = "fake-v1";
/// The directory of the home that holds the sessions.
pub const SESSIONS_DIR: &str = "sessions";
/// The directory of the home that holds what was exported.
pub const EXPORTS_DIR: &str = "exports";
/// The file of the home that remembers what has been numbered.
const STATE_FILE: &str = "state.json";

/// What a session numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    /// Items: `item_<n>`.
    Item,
    /// Requests for a decision: `req_<n>`.
    Request,
    /// Calls of Orch's tools: `call_<n>`.
    Call,
}

impl Counter {
    const fn key(self) -> &'static str {
        match self {
            Self::Item => "item",
            Self::Request => "request",
            Self::Call => "call",
        }
    }
}

/// An item that is final, as the session keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    /// The harness's id for it.
    pub native_ref: String,
    /// The turn it belongs to, by its place in the session.
    pub turn_ordinal: u32,
    /// `main` or `sub:<id>`.
    pub stream: String,
    /// For an item of a sub-agent, the item that started it.
    pub parent_native_ref: Option<String>,
    /// What kind of item.
    pub kind: ItemKind,
    /// How it ended.
    pub status: ItemStatus,
    /// What it held at the end.
    pub payload: Json,
    /// The blob that holds the whole of a long output, where there is one.
    #[serde(default)]
    pub output_blob: Option<BlobRef>,
}

/// A turn that ended, as the session keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    /// Its place in the session, from 1.
    pub ordinal: u32,
    /// Orch's id for it.
    pub turn_id: Id,
    /// The harness's id for it.
    pub native_turn_ref: String,
    /// What it was told.
    pub input: String,
    /// How it ended.
    pub status: TurnOutcome,
    /// Why, for a failure.
    pub error: Option<TurnError>,
    /// What it used.
    pub usage: TokenUsage,
}

/// The file of a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionFile {
    /// Always [`NATIVE_FORMAT`].
    pub format: String,
    /// The harness's id for the session.
    pub native_session_ref: String,
    /// When it was made.
    pub created_at: Time,
    /// When it last changed.
    pub updated_at: Time,
    /// The thread's directory when it was opened.
    pub directory: String,
    /// Where the agent started.
    pub cwd: String,
    /// The model it was opened with.
    pub model: String,
    /// The effort it was opened with.
    pub effort: Option<String>,
    /// The parameters it was opened with.
    pub params: Json,
    /// The instructions it was opened with.
    pub instructions: String,
    /// What it was told of its notes.
    pub memory: Option<SessionMemory>,
    /// The title the harness gave it.
    pub title: Option<String>,
    /// The turns that ended, in order.
    pub turns: Vec<TurnRecord>,
    /// The items that are final, in the order they ended.
    pub items: Vec<ItemRecord>,
    /// What the turns used together.
    pub usage: TokenUsage,
    /// What has been numbered so far.
    pub counters: BTreeMap<String, u64>,
}

impl SessionFile {
    fn new(native_session_ref: &str, spec: &SessionSpec, now: Time) -> Self {
        Self {
            format: NATIVE_FORMAT.to_owned(),
            native_session_ref: native_session_ref.to_owned(),
            created_at: now,
            updated_at: now,
            directory: spec.settings.directory.clone(),
            cwd: spec.settings.cwd.clone(),
            model: spec.settings.model.clone(),
            effort: spec.settings.effort.clone(),
            params: spec.settings.params.clone(),
            instructions: spec.settings.instructions.clone(),
            memory: spec.settings.memory.clone(),
            title: None,
            turns: Vec::new(),
            items: Vec::new(),
            usage: TokenUsage::default(),
            counters: BTreeMap::new(),
        }
    }

    /// The same session, as it stood after the turn `through`: the later turns, their items and their
    /// use are left out, and the counters stay so that nothing said later is numbered again.
    fn through(mut self, through: Option<u32>) -> Self {
        let Some(through) = through else {
            return self;
        };
        self.turns.retain(|turn| turn.ordinal <= through);
        self.items.retain(|item| item.turn_ordinal <= through);
        self.usage = self
            .turns
            .iter()
            .fold(TokenUsage::default(), |sum, turn| sum.plus(turn.usage));
        self
    }
}

fn io_error(what: &str, error: &std::io::Error) -> AdapterError {
    AdapterError::Internal(format!("{what}: {error}"))
}

/// The home of the fake harness: where its sessions and exports are.
#[derive(Debug, Clone)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// The home at `root`, made if it is not there.
    pub fn open(root: &Path) -> Result<Self, AdapterError> {
        for directory in [SESSIONS_DIR, EXPORTS_DIR] {
            std::fs::create_dir_all(root.join(directory))
                .map_err(|e| io_error("the home of the fake harness cannot be made", &e))?;
        }
        Ok(Self {
            root: root.to_owned(),
        })
    }

    /// The home's directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_path(&self, native_session_ref: &str) -> PathBuf {
        self.root
            .join(SESSIONS_DIR)
            .join(format!("{native_session_ref}.json"))
    }

    /// Numbers something in the home: the next session, the next export. It goes on from where an
    /// earlier run of the fake stopped.
    fn next_number(&self, what: &str) -> Result<u64, AdapterError> {
        let path = self.root.join(STATE_FILE);
        let mut state: BTreeMap<String, u64> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let next = state.get(what).copied().unwrap_or(0) + 1;
        state.insert(what.to_owned(), next);
        let bytes =
            serde_json::to_vec_pretty(&state).map_err(|e| AdapterError::Internal(e.to_string()))?;
        write_atomically(&path, &bytes)?;
        Ok(next)
    }

    /// A new reference for a session.
    pub fn next_session_ref(&self) -> Result<String, AdapterError> {
        Ok(format!("fake_{}", self.next_number("session")?))
    }

    /// A new path for an export with `extension`.
    pub fn next_export_path(
        &self,
        native_session_ref: &str,
        extension: &str,
    ) -> Result<PathBuf, AdapterError> {
        let number = self.next_number("export")?;
        Ok(self
            .root
            .join(EXPORTS_DIR)
            .join(format!("{native_session_ref}-{number}.{extension}")))
    }

    /// Writes a session.
    pub fn save(&self, file: &SessionFile) -> Result<(), AdapterError> {
        let bytes =
            serde_json::to_vec_pretty(file).map_err(|e| AdapterError::Internal(e.to_string()))?;
        write_atomically(&self.session_path(&file.native_session_ref), &bytes)
    }

    /// Reads the session `native_session_ref`.
    pub fn load(&self, native_session_ref: &str) -> Result<SessionFile, AdapterError> {
        let bytes = std::fs::read(self.session_path(native_session_ref)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AdapterError::SessionNotFound(native_session_ref.to_owned())
            } else {
                io_error("a session cannot be read", &e)
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|e| {
            AdapterError::HarnessError(format!("the session {native_session_ref} is damaged: {e}"))
        })
    }

    /// Makes a session for `spec`.
    pub fn create(&self, spec: &SessionSpec, now: Time) -> Result<SessionFile, AdapterError> {
        let file = SessionFile::new(&self.next_session_ref()?, spec, now);
        self.save(&file)?;
        Ok(file)
    }

    /// A copy of the session `source`, through the turn `through`, under a reference of its own.
    pub fn fork(
        &self,
        source: &str,
        through: Option<u32>,
        spec: &SessionSpec,
        now: Time,
    ) -> Result<SessionFile, AdapterError> {
        let mut file = self.load(source)?.through(through);
        file.native_session_ref = self.next_session_ref()?;
        file.created_at = now;
        file.updated_at = now;
        self.reopened_as(&mut file, spec);
        self.save(&file)?;
        Ok(file)
    }

    /// What a session that is opened again takes of the new settings: where it works and what it is
    /// told. What it said stays.
    pub fn reopened_as(&self, file: &mut SessionFile, spec: &SessionSpec) {
        file.directory.clone_from(&spec.settings.directory);
        file.cwd.clone_from(&spec.settings.cwd);
        file.model.clone_from(&spec.settings.model);
        file.effort.clone_from(&spec.settings.effort);
        file.params = spec.settings.params.clone();
        file.instructions.clone_from(&spec.settings.instructions);
        file.memory.clone_from(&spec.settings.memory);
    }

    /// The sessions of the home, newest first.
    pub fn list(&self) -> Result<Vec<NativeSession>, AdapterError> {
        let mut found = Vec::new();
        let entries = std::fs::read_dir(self.root.join(SESSIONS_DIR))
            .map_err(|e| io_error("the sessions cannot be listed", &e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(file) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<SessionFile>(&bytes).ok())
            else {
                continue;
            };
            found.push(NativeSession {
                native_session_ref: file.native_session_ref,
                title: file.title,
                cwd: Some(file.cwd),
                updated_at: Some(file.updated_at),
                adopted_thread_id: None,
            });
        }
        found.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.native_session_ref.cmp(&a.native_session_ref))
        });
        Ok(found)
    }

    /// Writes the session `file` through the turn `through` as a native export, and says where.
    pub fn export_native(
        &self,
        file: &SessionFile,
        through: Option<u32>,
    ) -> Result<(PathBuf, i64), AdapterError> {
        let path = self.next_export_path(&file.native_session_ref, "json")?;
        let bytes = serde_json::to_vec_pretty(&file.clone().through(through))
            .map_err(|e| AdapterError::Internal(e.to_string()))?;
        write_atomically(&path, &bytes)?;
        Ok((path, size_of(&bytes)))
    }

    /// Writes the main stream of the session as the neutral history, one item to a line, oldest
    /// first.
    pub fn export_neutral(
        &self,
        file: &SessionFile,
        through: Option<u32>,
    ) -> Result<(PathBuf, i64), AdapterError> {
        let path = self.next_export_path(&file.native_session_ref, "jsonl")?;
        let text = neutral_lines(&file.clone().through(through));
        write_atomically(&path, text.as_bytes())?;
        Ok((path, size_of(text.as_bytes())))
    }

    /// Reads a native export into a new session of this home.
    pub fn import_native(
        &self,
        archive: &Path,
        through: Option<u32>,
        spec: &SessionSpec,
        now: Time,
    ) -> Result<(SessionFile, HistoryReport), AdapterError> {
        let bytes = std::fs::read(archive).map_err(|e| {
            AdapterError::ImportNativeFailed(format!("the archive cannot be read: {e}"))
        })?;
        let source: SessionFile = serde_json::from_slice(&bytes)
            .map_err(|e| AdapterError::ImportNativeFailed(format!("not a fake-v1 session: {e}")))?;
        if source.format != NATIVE_FORMAT {
            return Err(AdapterError::ImportNativeFailed(format!(
                "the format {:?} is not {NATIVE_FORMAT}",
                source.format
            )));
        }
        let mut file = source.through(through);
        file.native_session_ref = self.next_session_ref()?;
        file.created_at = now;
        file.updated_at = now;
        self.reopened_as(&mut file, spec);
        let mut report = ReportBuilder::new();
        for item in &file.items {
            report.keep(kind_name(item.kind));
        }
        self.save(&file)?;
        Ok((file, report.finish(Fidelity::Lossless)))
    }

    /// Reads the neutral history into a new session of this home.
    pub fn import_neutral(
        &self,
        history: &Path,
        through: Option<u32>,
        spec: &SessionSpec,
        now: Time,
    ) -> Result<(SessionFile, HistoryReport), AdapterError> {
        let text = std::fs::read_to_string(history)
            .map_err(|e| AdapterError::HarnessError(format!("the history cannot be read: {e}")))?;
        let mut file = SessionFile::new(&self.next_session_ref()?, spec, now);
        let mut report = ReportBuilder::new();
        let mut ordinal = 0u32;
        for (number, line) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let value: Json = serde_json::from_str(line).map_err(|e| {
                AdapterError::HarnessError(format!("line {}: not JSON: {e}", number + 1))
            })?;
            let kind = value.get("kind").and_then(Json::as_str).unwrap_or_default();
            let stream = value
                .get("stream")
                .and_then(Json::as_str)
                .unwrap_or(MAIN_STREAM);
            if stream != MAIN_STREAM {
                report.drop_item(kind, "the neutral history is the main stream only");
                continue;
            }
            if kind == "user_message" {
                ordinal += 1;
                file.turns.push(TurnRecord {
                    ordinal,
                    turn_id: value
                        .pointer("/payload/input_id")
                        .and_then(Json::as_str)
                        .and_then(|id| id.parse().ok())
                        .unwrap_or_else(orch_types::new_id),
                    native_turn_ref: format!("turn_{ordinal}"),
                    input: message_text(&value),
                    status: TurnOutcome::Completed,
                    error: None,
                    usage: TokenUsage::default(),
                });
                report.keep(kind);
                continue;
            }
            let Some(item_kind) = parse_kind(kind) else {
                report.drop_item(kind, "the fake harness keeps no item of this kind");
                continue;
            };
            let native_ref = value
                .get("native_ref")
                .and_then(Json::as_str)
                .filter(|wanted| !file.items.iter().any(|i| i.native_ref == *wanted))
                .map_or_else(
                    || format!("item_{}", next_in(&mut file.counters, Counter::Item)),
                    str::to_owned,
                );
            file.items.push(ItemRecord {
                native_ref,
                turn_ordinal: ordinal.max(1),
                stream: MAIN_STREAM.to_owned(),
                parent_native_ref: None,
                kind: item_kind,
                status: value
                    .get("status")
                    .and_then(|s| serde_json::from_value(s.clone()).ok())
                    .unwrap_or(ItemStatus::Completed),
                payload: value.get("payload").cloned().unwrap_or(Json::Null),
                output_blob: None,
            });
            report.convert(kind);
        }
        file.counters
            .entry(Counter::Item.key().to_owned())
            .or_insert(0);
        let mut file = file.through(through);
        file.usage = TokenUsage::default();
        self.save(&file)?;
        let mut report = report.finish(Fidelity::Structured);
        report.warnings.push(
            "the fake harness keeps the items of the history, not what the model saw".to_owned(),
        );
        Ok((file, report))
    }
}

fn size_of(bytes: &[u8]) -> i64 {
    i64::try_from(bytes.len()).unwrap_or(i64::MAX)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), AdapterError> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".writing");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, bytes).map_err(|e| io_error("a file cannot be written", &e))?;
    std::fs::rename(&temporary, path).map_err(|e| io_error("a file cannot be put in place", &e))
}

fn next_in(counters: &mut BTreeMap<String, u64>, counter: Counter) -> u64 {
    let slot = counters.entry(counter.key().to_owned()).or_insert(0);
    *slot += 1;
    *slot
}

/// The name of a kind of item, as it is written.
#[must_use]
pub fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::AgentMessage => "agent_message",
        ItemKind::Reasoning => "reasoning",
        ItemKind::Plan => "plan",
        ItemKind::ToolCall => "tool_call",
        ItemKind::Marker => "marker",
        ItemKind::Error => "error",
    }
}

fn parse_kind(name: &str) -> Option<ItemKind> {
    [
        ItemKind::AgentMessage,
        ItemKind::Reasoning,
        ItemKind::Plan,
        ItemKind::ToolCall,
        ItemKind::Marker,
        ItemKind::Error,
    ]
    .into_iter()
    .find(|kind| kind_name(*kind) == name)
}

fn message_text(line: &Json) -> String {
    line.pointer("/payload/parts")
        .and_then(Json::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Json::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .or_else(|| {
            line.pointer("/payload/text")
                .and_then(Json::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// The neutral history of a session: what each turn was told as a `user_message`, then what the agent
/// said in the main stream, one JSON object to a line.
#[must_use]
pub fn neutral_lines(file: &SessionFile) -> String {
    let mut lines = Vec::new();
    for turn in &file.turns {
        lines.push(serde_json::json!({
            "kind": "user_message", "status": "completed", "stream": MAIN_STREAM,
            "turn_ordinal": turn.ordinal,
            "payload": {
                "parts": [ { "type": "text", "text": turn.input } ],
                "input_id": turn.turn_id, "delivery": "queue", "source": "user"
            }
        }));
        for item in file
            .items
            .iter()
            .filter(|item| item.turn_ordinal == turn.ordinal && item.stream == MAIN_STREAM)
        {
            lines.push(serde_json::json!({
                "kind": kind_name(item.kind), "status": item.status, "stream": MAIN_STREAM,
                "turn_ordinal": turn.ordinal, "native_ref": item.native_ref,
                "payload": item.payload
            }));
        }
    }
    let mut text = lines
        .iter()
        .map(Json::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// What a neutral export says of a kind of item it left out, for a report to cite.
#[must_use]
pub fn dropped_of(report: &HistoryReport, kind: &str) -> Option<Dropped> {
    report.dropped.iter().find(|d| d.kind == kind).cloned()
}

/// How many items of each kind a report counts, for a test to read.
#[must_use]
pub fn counts_of(report: &HistoryReport, kind: &str) -> KindCounts {
    report.by_kind.get(kind).copied().unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------
// The notes

/// Appends a note to the `MEMORY.md` of `directory`.
pub fn append_note(directory: &str, note: &str) -> Result<(), AdapterError> {
    use std::io::Write;
    std::fs::create_dir_all(directory).map_err(|e| io_error("the notes cannot be kept", &e))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(Path::new(directory).join(MEMORY_INDEX_FILE))
        .map_err(|e| io_error("the notes cannot be kept", &e))?;
    writeln!(file, "- {note}").map_err(|e| io_error("the notes cannot be kept", &e))
}

/// The notes of the `MEMORY.md` of `directory`: one for each line that begins with `- `.
#[must_use]
pub fn read_notes(directory: &str) -> Vec<String> {
    std::fs::read_to_string(Path::new(directory).join(MEMORY_INDEX_FILE))
        .map(|text| {
            text.lines()
                .filter_map(|line| line.strip_prefix("- "))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------
// A session that is open

/// A turn that runs.
pub struct ActiveTurn {
    /// The turn.
    pub turn_id: Id,
    /// Fires when the turn is to end: an interrupt, a close, a stop.
    pub cancel: CancellationToken,
    /// The task that runs it.
    pub task: Option<tokio::task::JoinHandle<()>>,
}

/// A request for a decision that waits.
pub struct PendingApproval {
    /// What was asked.
    pub request: HarnessRequest,
    /// Where the decision goes.
    pub reply: oneshot::Sender<ApprovalDecision>,
}

/// A session that is open in the fake harness.
pub struct LiveSession {
    /// The thread.
    pub thread_id: ThreadId,
    /// The segment of the thread.
    pub segment_id: Id,
    /// What the session was opened with.
    pub spec: SessionSpec,
    /// The notes it loaded when it opened.
    pub loaded_notes: Vec<String>,
    home: Home,
    file: Mutex<SessionFile>,
    /// The turn that runs.
    pub turn: Mutex<Option<ActiveTurn>>,
    /// The requests that wait for a decision, by reference.
    pub approvals: Mutex<HashMap<String, PendingApproval>>,
    /// The calls of Orch's tools that wait for a result, by reference.
    pub tool_calls: Mutex<HashMap<String, oneshot::Sender<Result<Json, ToolError>>>>,
    /// What was added to the running turn and has not been taken in.
    pub steers: Mutex<VecDeque<String>>,
    /// Told when a steer is added.
    pub steer_notify: Notify,
    /// The commands a person allowed for the rest of the session.
    pub allowed_commands: Mutex<HashSet<String>>,
    /// Scripted commands retained by this exact session, never real operating-system processes.
    pub background:
        Mutex<HashMap<String, orch_agent_contract::background::NativeBackgroundCommand>>,
    /// Exact scripted ends let concurrent callers distinguish an already-ended command.
    pub background_ended: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for LiveSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSession")
            .field("thread", &self.thread_id)
            .finish_non_exhaustive()
    }
}

impl LiveSession {
    /// A live session over `file`.
    #[must_use]
    pub fn new(
        home: Home,
        file: SessionFile,
        spec: SessionSpec,
        loaded_notes: Vec<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            thread_id: spec.settings.thread_id,
            segment_id: spec.settings.segment_id,
            spec,
            loaded_notes,
            home,
            file: Mutex::new(file),
            turn: Mutex::new(None),
            approvals: Mutex::new(HashMap::new()),
            tool_calls: Mutex::new(HashMap::new()),
            steers: Mutex::new(VecDeque::new()),
            steer_notify: Notify::new(),
            allowed_commands: Mutex::new(HashSet::new()),
            background: Mutex::new(HashMap::new()),
            background_ended: Mutex::new(HashSet::new()),
        })
    }

    /// The session as it is kept.
    #[must_use]
    pub fn snapshot(&self) -> SessionFile {
        lock(&self.file).clone()
    }

    /// The harness's id for the session.
    #[must_use]
    pub fn native_session_ref(&self) -> String {
        lock(&self.file).native_session_ref.clone()
    }

    /// Changes the file and writes it.
    pub fn update<R>(&self, change: impl FnOnce(&mut SessionFile) -> R) -> R {
        let mut file = lock(&self.file);
        let result = change(&mut file);
        if let Err(error) = self.home.save(&file) {
            tracing::warn!(%error, "a session could not be written");
        }
        result
    }

    /// Takes the next number of something.
    pub fn next_counter(&self, counter: Counter) -> u64 {
        self.update(|file| next_in(&mut file.counters, counter))
    }

    /// Keeps an item that is final.
    pub fn record_item(&self, item: ItemRecord) {
        self.update(|file| file.items.push(item));
    }

    /// Keeps a turn that ended, and what it used.
    pub fn record_turn(&self, turn: TurnRecord, now: Time) {
        self.update(|file| {
            file.usage = file.usage.plus(turn.usage);
            file.updated_at = now;
            file.turns.push(turn);
        });
    }

    /// How many turns the session has had, ended or running.
    #[must_use]
    pub fn turns_so_far(&self) -> u32 {
        let file = lock(&self.file);
        u32::try_from(file.turns.len()).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
    use orch_agent_contract::session::{ExecAccess, ProviderAccess, SessionMode, SessionSettings};

    fn spec(mode: SessionMode) -> SessionSpec {
        SessionSpec {
            settings: SessionSettings {
                agent_id: orch_types::new_id(),
                thread_id: orch_types::new_id(),
                segment_id: orch_types::new_id(),
                mode,
                native_session_ref: None,
                upto_turn_ordinal: None,
                workspace_root: "/work".into(),
                instruction_root_marker: false,
                directory: "/work".into(),
                cwd: "/work".into(),
                mounts: vec![],
                model: "fake-large".into(),
                effort: None,
                params: Json::Null,
                approval_mode: ApprovalMode::Ask,
                sandbox_mode: SandboxMode::Workspace,
                available_models: vec!["fake-large".to_owned()],
                subagent_limits: Default::default(),
                instructions: String::new(),
                memory: None,
                arrangement: ArrangementType::Together,
                orch_tools: vec![],
                orch_tool_schemas: Default::default(),
                raw_events: false,
                side_reading: None,
                workspace_configuration: None,
            },
            session_key: "0".repeat(32),
            archive: None,
            history: None,
            provider: ProviderAccess::Native {
                provider_id: orch_types::new_id(),
            },
            exec: ExecAccess {
                executor: None,
                tool_set: None,
                environment_id: orch_types::new_id(),
                generation: 1,
            },
            mcp: vec![],
            skills: None,
        }
    }

    fn now() -> Time {
        "2026-10-11T10:00:00.000Z".parse().unwrap()
    }

    fn item(ordinal: u32, native_ref: &str, text: &str) -> ItemRecord {
        ItemRecord {
            native_ref: native_ref.into(),
            turn_ordinal: ordinal,
            stream: MAIN_STREAM.into(),
            parent_native_ref: None,
            kind: ItemKind::AgentMessage,
            status: ItemStatus::Completed,
            payload: serde_json::json!({ "text": text, "text_len": text.len(), "phase": "final" }),
            output_blob: None,
        }
    }

    fn two_turns(home: &Home) -> SessionFile {
        let mut file = home.create(&spec(SessionMode::New), now()).unwrap();
        for ordinal in 1..=2u32 {
            file.turns.push(TurnRecord {
                ordinal,
                turn_id: orch_types::new_id(),
                native_turn_ref: format!("turn_{ordinal}"),
                input: format!("question {ordinal}"),
                status: TurnOutcome::Completed,
                error: None,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_tokens: 15,
                    ..TokenUsage::default()
                },
            });
            file.items.push(item(
                ordinal,
                &format!("item_{ordinal}"),
                &format!("answer {ordinal}"),
            ));
        }
        file.usage = file
            .turns
            .iter()
            .fold(TokenUsage::default(), |s, t| s.plus(t.usage));
        home.save(&file).unwrap();
        file
    }

    #[test]
    fn a_session_that_was_written_is_read_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        let file = two_turns(&home);
        assert_eq!(home.load(&file.native_session_ref).unwrap(), file);
        assert_eq!(
            home.load("fake_999").unwrap_err().code(),
            "session_not_found"
        );
    }

    #[test]
    fn sessions_are_numbered_on_from_where_an_earlier_run_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let first = Home::open(dir.path()).unwrap();
        assert_eq!(first.next_session_ref().unwrap(), "fake_1");
        assert_eq!(first.next_session_ref().unwrap(), "fake_2");
        let again = Home::open(dir.path()).unwrap();
        assert_eq!(again.next_session_ref().unwrap(), "fake_3");
    }

    #[test]
    fn a_fork_through_a_turn_has_that_history_and_a_reference_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        let file = two_turns(&home);
        let forked = home
            .fork(
                &file.native_session_ref,
                Some(1),
                &spec(SessionMode::Fork),
                now(),
            )
            .unwrap();
        assert_ne!(forked.native_session_ref, file.native_session_ref);
        assert_eq!(forked.turns.len(), 1);
        assert_eq!(forked.items.len(), 1);
        assert_eq!(
            forked.usage.total_tokens, 15,
            "the use of the turns that are kept"
        );
        assert_eq!(
            home.load(&file.native_session_ref).unwrap().turns.len(),
            2,
            "the source is not touched"
        );
    }

    #[test]
    fn a_native_export_imports_into_a_session_with_the_same_history() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        let file = two_turns(&home);
        let (path, size) = home.export_native(&file, None).unwrap();
        assert!(size > 0);
        let other = tempfile::tempdir().unwrap();
        let elsewhere = Home::open(other.path()).unwrap();
        let (imported, report) = elsewhere
            .import_native(&path, None, &spec(SessionMode::ImportNative), now())
            .unwrap();
        assert_eq!(imported.turns, file.turns);
        assert_eq!(imported.items, file.items);
        assert_eq!(report.fidelity, Fidelity::Lossless);
        assert_eq!(counts_of(&report, "agent_message").kept, 2);
    }

    #[test]
    fn a_neutral_export_imports_as_items_of_the_main_stream_only() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        let mut file = two_turns(&home);
        file.items.push(ItemRecord {
            stream: "sub:scout".into(),
            parent_native_ref: Some("item_1".into()),
            ..item(1, "item_9", "from a sub-agent")
        });
        let (path, _) = home.export_neutral(&file, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.lines().count(),
            4,
            "two questions, two answers, no sub-agent"
        );
        let (imported, report) = home
            .import_neutral(&path, None, &spec(SessionMode::ImportNeutral), now())
            .unwrap();
        assert_eq!(imported.turns.len(), 2);
        assert_eq!(imported.turns[1].input, "question 2");
        assert_eq!(imported.items.len(), 2);
        assert_eq!(report.fidelity, Fidelity::Structured);
    }

    #[test]
    fn a_neutral_history_with_an_item_the_fake_does_not_keep_says_what_was_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"kind\":\"user_message\",\"payload\":{\"parts\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
                "{\"kind\":\"announcement\",\"payload\":{}}\n",
                "{\"kind\":\"agent_message\",\"payload\":{\"text\":\"hello\"}}\n"
            ),
        )
        .unwrap();
        let (imported, report) = home
            .import_neutral(&path, None, &spec(SessionMode::ImportNeutral), now())
            .unwrap();
        assert_eq!(imported.items.len(), 1);
        assert_eq!(dropped_of(&report, "announcement").unwrap().count, 1);
    }

    #[test]
    fn notes_are_lines_of_the_index_and_are_read_back_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("memory");
        let notes = notes.to_str().unwrap();
        assert!(read_notes(notes).is_empty());
        append_note(notes, "the build needs libssl").unwrap();
        append_note(notes, "deploys go out on Tuesday").unwrap();
        assert_eq!(
            read_notes(notes),
            ["the build needs libssl", "deploys go out on Tuesday"]
        );
    }

    #[test]
    fn the_sessions_of_a_home_are_listed_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        let old = home.create(&spec(SessionMode::New), now()).unwrap();
        let later: Time = "2026-10-11T11:00:00.000Z".parse().unwrap();
        let new = home.create(&spec(SessionMode::New), later).unwrap();
        let listed = home.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].native_session_ref, new.native_session_ref);
        assert_eq!(listed[1].native_session_ref, old.native_session_ref);
    }
}
