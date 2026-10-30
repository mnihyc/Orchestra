//! Codex notifications to neutral events: each item as a step of its kind, with the call as Codex made it kept whole beside it.
//!
//! One [`Mapper`] follows one session. It is fed the notifications of the harness for its thread, and for the
//! threads of the sub-agents that thread starts, one at a time and in order, and answers with the neutral
//! events they come to ([`Mapper::handle`]); it does no input or output of its own except for the files that
//! hold a long output ([`OutputStore`]), so that every rule here is tested with the frames of a real harness.
//!
//! * What a person said is never reported: the engine writes that item itself, and the harness's echo of it
//!   (`userMessage`) is dropped.
//! * Every tool is reported twice: by the kind of thing it was (a command, a change of files, an MCP call, …)
//!   and as Codex made it, with the item's type as the name, its arguments as JSON text and its result as
//!   text. An item of a type the mapper does not know shows as a tool of kind `other`, whole.
//! * Growing text arrives as deltas with offsets in the unit of the field (code points for what is said, bytes
//!   for what a command prints), and the item completes with its final text.
//! * A turn that ends with an item still open closes it: a message or a reasoning cut short by a failure is
//!   `cancelled` or `failed`, and a command that goes on in the background stays open and is marked so, since
//!   Codex completes it when its process ends, whenever that is.
//! * An attempt of the model that fails and is made again leaves the text it had streamed behind: those items
//!   are completed as `cancelled`, and the new attempt's are new items.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use orch_agent_contract::discovery::{RecordBuilder, RecordedEntry};
use orch_agent_contract::events::UsageRequest;
use orch_agent_contract::events::{
    AgentMessagePayload, AgentMessagePhase, BlobRef, CommandAction, DeltaField, ErrorPayload,
    FILE_CHANGE_DIFF_MAX_BYTES, FileChange, FileChangeKind, ItemCompleted, ItemContent, ItemDelta,
    ItemStarted, ItemStatus, ItemUpdated, MAIN_STREAM, MarkerPayload, OUTPUT_MAX_BYTES, OrchTool,
    OutputRef, PlanPayload, PlanStep, PlanStepStatus, RawCall, RawFrame, ReasoningPayload,
    SessionEvent, SessionLost, SessionTitle, SubagentState, TokenUsage, ToolCallPayload,
    ToolPayload, TurnCompleted, TurnError, TurnOutcome, TurnPhase, TurnPhaseKind, TurnStarted,
    UsageUpdated, WebSearchResult, count_diff_lines, sub_stream, turn_error,
};
use orch_agent_contract::events::{ContextShortening, ModelTokens, StreamTokens, TurnUsageReport};
use orch_agent_contract::loopback::ORCH_SERVER;
use orch_agent_contract::session::{ThreadId, TurnId};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde_json::json;

use crate::failure::{self, Notes, Patience, Windows};

/// The longest line shown as the title of a tool call, in characters.
pub const TITLE_MAX_CHARS: usize = 160;

/// How much of the beginning and of the end of a command's output is kept when it is longer than
/// [`OUTPUT_MAX_BYTES`]: the middle is dropped, and a line in its place says how much.
const OUTPUT_KEPT_EACH_END_BYTES: usize = 32 * 1024 * 1024;

// ---------------------------------------------------------------------------------------------------------
// Files that hold a long output

/// Where the whole of a long output is kept, as a file named by its hash, for the connector to upload.
#[derive(Debug, Clone)]
pub struct OutputStore {
    directory: PathBuf,
}

impl OutputStore {
    /// A store that keeps its files in `directory`.
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// The directory the files are in.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Keeps `bytes` and names them by their hash. A file that is there already is left as it is.
    pub fn put(&self, bytes: &[u8]) -> std::io::Result<BlobRef> {
        let hash = Hash::of(bytes);
        let path = self.directory.join(hash.to_string());
        if !path.exists() {
            crate::process::write_private(&path, bytes)?;
        }
        Ok(BlobRef {
            hash,
            size: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
        })
    }
}

/// What the mapper needs of the world around it.
pub struct Context<'a> {
    /// Where the time comes from.
    pub clock: &'a dyn Clock,
    /// Where long outputs are kept.
    pub outputs: &'a OutputStore,
    /// The windows of the plan as the app-server last reported them, for a failure that is a limit.
    pub windows: Option<&'a Windows>,
}

// ---------------------------------------------------------------------------------------------------------
// Small readers of the harness's JSON

fn text<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or_default()
}

fn opt_text(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn whole(value: &Json, key: &str) -> Option<i64> {
    value.get(key).and_then(Json::as_i64)
}

fn count_chars(text: &str) -> i64 {
    i64::try_from(text.chars().count()).unwrap_or(i64::MAX)
}

fn count_bytes(text: &str) -> i64 {
    i64::try_from(text.len()).unwrap_or(i64::MAX)
}

/// The first line of `text` that has something on it, cut to [`TITLE_MAX_CHARS`] characters.
pub fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= TITLE_MAX_CHARS {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(TITLE_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

// ---------------------------------------------------------------------------------------------------------
// Commands

/// Splits a command line into its words the way a POSIX shell does, as far as quoting goes. `None` when a
/// quote is not closed.
#[must_use]
pub fn shell_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => word.push(other),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            escaped @ ('"' | '\\' | '$' | '`') => word.push(escaped),
                            other => {
                                word.push('\\');
                                word.push(other);
                            }
                        },
                        other => word.push(other),
                    }
                }
            }
            '\\' => {
                started = true;
                word.push(chars.next()?);
            }
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            other => {
                started = true;
                word.push(other);
            }
        }
    }
    if started {
        words.push(word);
    }
    Some(words)
}

/// The command a person reads: the script of `bash -lc '<script>'` and its like, which is what the agent
/// wrote, and otherwise the command line as it is.
#[must_use]
pub fn display_command(command: &str) -> String {
    let Some(words) = shell_words(command) else {
        return command.to_owned();
    };
    if words.len() < 3 {
        return command.to_owned();
    }
    let program = Path::new(&words[0])
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !matches!(program.as_str(), "sh" | "bash" | "zsh" | "dash" | "ksh") {
        return command.to_owned();
    }
    let flags = &words[1..words.len() - 1];
    let only_flags = flags.iter().all(|flag| flag.starts_with('-'));
    let runs_a_script = flags
        .iter()
        .any(|flag| flag.starts_with('-') && !flag.starts_with("--") && flag.contains('c'));
    if only_flags && runs_a_script {
        words[words.len() - 1].clone()
    } else {
        command.to_owned()
    }
}

fn command_actions(item: &Json) -> Vec<CommandAction> {
    item.get("commandActions")
        .and_then(Json::as_array)
        .map(|actions| {
            actions
                .iter()
                .map(|action| match text(action, "type") {
                    "read" => CommandAction::Read {
                        path: text(action, "path").to_owned(),
                    },
                    "listFiles" => CommandAction::List {
                        path: opt_text(action, "path"),
                    },
                    "search" => CommandAction::Search {
                        query: text(action, "query").to_owned(),
                        path: opt_text(action, "path"),
                    },
                    _ => CommandAction::Other,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A text longer than [`OUTPUT_MAX_BYTES`] without its middle, with a line in its place that says how much
/// was left out.
fn cap_middle(output: &str) -> std::borrow::Cow<'_, str> {
    let limit = usize::try_from(OUTPUT_MAX_BYTES).unwrap_or(usize::MAX);
    if output.len() <= limit {
        return std::borrow::Cow::Borrowed(output);
    }
    let mut head = OUTPUT_KEPT_EACH_END_BYTES;
    while !output.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = output.len() - OUTPUT_KEPT_EACH_END_BYTES;
    while !output.is_char_boundary(tail) {
        tail += 1;
    }
    std::borrow::Cow::Owned(format!(
        "{}\n[… {} bytes left out of the middle …]\n{}",
        &output[..head],
        tail - head,
        &output[tail..]
    ))
}

// ---------------------------------------------------------------------------------------------------------
// File changes

fn relative_to(path: &str, directory: &str) -> String {
    if directory.is_empty() {
        return path.to_owned();
    }
    Path::new(path)
        .strip_prefix(directory)
        .ok()
        .filter(|rest| !rest.as_os_str().is_empty())
        .map_or_else(
            || path.to_owned(),
            |rest| rest.to_string_lossy().into_owned(),
        )
}

/// The unified diff, without file headers, of a file that is all new (`before` empty) or all gone (`after`
/// empty), with three lines of context.
#[must_use]
pub fn whole_file_diff(before: &str, after: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .to_string()
}

fn cut_diff(diff: String) -> (String, bool) {
    let limit = usize::try_from(FILE_CHANGE_DIFF_MAX_BYTES).unwrap_or(usize::MAX);
    if diff.len() <= limit {
        return (diff, false);
    }
    let mut end = limit;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    let end = diff[..end].rfind('\n').map_or(end, |line_end| line_end + 1);
    (diff[..end].to_owned(), true)
}

/// The change of one file as the neutral model has it, from an entry of a `fileChange` item.
#[must_use]
pub fn file_change(change: &Json, directory: &str) -> FileChange {
    let raw_path = text(change, "path");
    let kind = change.get("kind").cloned().unwrap_or(Json::Null);
    let given = text(change, "diff");
    let binary = given.contains('\0');
    let (kind, path, old_path, diff) = match text(&kind, "type") {
        "add" => (
            FileChangeKind::Add,
            relative_to(raw_path, directory),
            None,
            whole_file_diff("", given),
        ),
        "delete" => (
            FileChangeKind::Delete,
            relative_to(raw_path, directory),
            None,
            whole_file_diff(given, ""),
        ),
        _ => {
            let moved_to = opt_text(&kind, "move_path");
            match moved_to {
                Some(target) => (
                    FileChangeKind::Rename,
                    relative_to(&target, directory),
                    Some(relative_to(raw_path, directory)),
                    given.to_owned(),
                ),
                None => (
                    FileChangeKind::Modify,
                    relative_to(raw_path, directory),
                    None,
                    given.to_owned(),
                ),
            }
        }
    };
    if binary {
        return FileChange {
            path,
            change: kind,
            old_path,
            diff: None,
            added: 0,
            removed: 0,
            binary: true,
            truncated: false,
        };
    }
    let (added, removed) = count_diff_lines(&diff);
    let (diff, truncated) = cut_diff(diff);
    FileChange {
        path,
        change: kind,
        old_path,
        diff: (!diff.is_empty() || kind != FileChangeKind::Modify).then_some(diff),
        added,
        removed,
        binary: false,
        truncated,
    }
}

/// The changes of a `fileChange` item.
#[must_use]
pub fn file_changes(item: &Json, directory: &str) -> Vec<FileChange> {
    item.get("changes")
        .and_then(Json::as_array)
        .map(|changes| {
            changes
                .iter()
                .map(|change| file_change(change, directory))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------
// Errors

// ---------------------------------------------------------------------------------------------------------
// The mapper

/// What the mapper is told about the session it follows.
#[derive(Debug, Clone)]
pub struct MapperConfig {
    /// The thread of Orch.
    pub thread_id: ThreadId,
    /// The segment of the thread.
    pub segment_id: Id,
    /// The harness's id for the thread.
    pub native_thread: String,
    /// The thread's directory, which the paths shown are relative to.
    pub directory: String,
}

/// The turn that runs.
#[derive(Debug)]
struct TurnState {
    id: TurnId,
    native: String,
    usage: TokenUsage,
    /// The sub-agents' share of `usage`.
    subagents: TokenUsage,
    /// Each sub-agent's share, by the stream of its items.
    by_subagent: BTreeMap<String, TokenUsage>,
    plan_started: bool,
    compaction_item: Option<String>,
    /// What the harness announced about trying again, and about the answer being written.
    notes: Notes,
    /// The failure the harness reported last, classified, for the end of the turn.
    failure: Option<TurnError>,
    /// Since when the harness has been announcing that it tries again, with no word of an answer between.
    retrying_since: Option<Time>,
    /// The model the harness moved the turn to, when it ran on another than the one chosen.
    ran_on: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    AgentMessage,
    Reasoning,
    Command,
    Tool,
    UserInput,
}

/// An item that has begun and is not yet complete.
#[derive(Debug)]
struct OpenItem {
    turn_id: TurnId,
    /// Only a matching native turn id proves that Stop owns this command.
    command_turn: Option<TurnId>,
    kind: ItemKind,
    stream: String,
    started_at: Time,
    text: String,
    text_chars: i64,
    summary_parts: i64,
    phase: Option<AgentMessagePhase>,
    output: OutputRef,
    output_bytes: i64,
    background: bool,
    /// The call as it was last reported, for a tool: what an item that is cut short is completed with.
    call: Option<ToolCallPayload>,
}

impl OpenItem {
    fn new(kind: ItemKind, stream: &str, started_at: Time, turn_id: TurnId) -> Self {
        Self {
            turn_id,
            command_turn: None,
            kind,
            stream: stream.to_owned(),
            started_at,
            text: String::new(),
            text_chars: 0,
            summary_parts: 0,
            phase: None,
            output: OutputRef::default(),
            output_bytes: 0,
            background: false,
            call: None,
        }
    }
}

/// A sub-agent's thread.
#[derive(Debug, Clone)]
struct Child {
    parent_item: Option<String>,
    turn_id: Option<TurnId>,
}

/// Follows one session.
#[derive(Debug)]
pub struct Mapper {
    requests: crate::performance::Requests,
    request_turn: Option<TurnId>,
    request_reports: Vec<(TurnId, UsageRequest)>,
    config: MapperConfig,
    pending_turn: Option<TurnId>,
    turn: Option<TurnState>,
    foreign_turn: Option<String>,
    items: HashMap<String, OpenItem>,
    /// Thinking is published only when the harness supplies readable text.
    pending_reasoning: HashMap<String, SessionEvent>,
    children: HashMap<String, Child>,
    spawn_items: HashMap<String, String>,
    open_spawn: Option<String>,
    /// The state each sub-agent's card last said, by the sub-agent's thread: its spawn call's card follows the
    /// sub-agent, which goes on after the call that started it has completed.
    child_states: HashMap<String, SubagentState>,
    thread_usage: TokenUsage,
    children_usage: TokenUsage,
    /// The size of the last prompt of the session's own thread, for a shortening's size before it.
    last_prompt: Option<i64>,
    /// A shortening of the conversation that waits for the next prompt to say its size after it.
    pending_shortening: Option<ContextShortening>,
    /// The row of the table of failures that found the last failure, for a log and for the tests.
    last_failure_row: Option<&'static str>,
    /// When the harness last said anything about this session.
    last_heard: Option<Time>,
}

impl TurnState {
    /// What the turn reported beyond its totals so far.
    fn report(
        &self,
        compact_at: Option<i64>,
        shortened: Option<ContextShortening>,
    ) -> TurnUsageReport {
        TurnUsageReport {
            api_time_ms: None,
            subagents: Some(self.subagents),
            by_subagent: self
                .by_subagent
                .iter()
                .map(|(stream, tokens)| StreamTokens {
                    stream: stream.clone(),
                    tokens: *tokens,
                })
                .collect(),
            // The harness reports no use by model; a turn it moved to another model ran on that one alone.
            models: self
                .ran_on
                .iter()
                .map(|model| ModelTokens {
                    model: model.clone(),
                    tokens: self.usage,
                })
                .collect(),
            compact_at,
            shortened,
            context: None,
            // This harness says nothing of what fills its context: Orch estimates it.
            breakdown: false,
        }
    }
}

impl Mapper {
    /// A mapper for the session `config` describes.
    #[must_use]
    pub fn new(config: MapperConfig) -> Self {
        Self {
            requests: crate::performance::Requests::default(),
            request_turn: None,
            request_reports: Vec::new(),
            config,
            pending_turn: None,
            turn: None,
            foreign_turn: None,
            items: HashMap::new(),
            pending_reasoning: HashMap::new(),
            children: HashMap::new(),
            spawn_items: HashMap::new(),
            open_spawn: None,
            child_states: HashMap::new(),
            thread_usage: TokenUsage::default(),
            children_usage: TokenUsage::default(),
            last_prompt: None,
            pending_shortening: None,
            last_failure_row: None,
            last_heard: None,
        }
    }

    /// Gives up on the turn that runs if it has waited for too long: the harness has said nothing for
    /// `patience.silence` while only the model was awaited (no command runs, no person is asked), or it has
    /// announced for `patience.retrying` that it tries again, with no word of an answer between. The turn
    /// then ends as failed, with what is open closed, and the harness's own end of it, when it comes, is
    /// dropped. Gives the harness's id of the turn, which is to be interrupted, and the events.
    pub fn give_up_if_tired(
        &mut self,
        ctx: &Context<'_>,
        patience: &Patience,
        asking_a_person: bool,
    ) -> Option<(String, Vec<SessionEvent>)> {
        let now = ctx.clock.now();
        let turn = self.turn.as_ref()?;
        let secs_since = |since: Time| (now - since.get()).num_seconds();
        let waiting_for_more_than_the_model = asking_a_person
            || self.items.values().any(|item| {
                matches!(
                    item.kind,
                    ItemKind::Command | ItemKind::Tool | ItemKind::UserInput
                )
            });
        let trying = patience
            .retrying
            .zip(turn.retrying_since)
            .filter(|(after, since)| {
                secs_since(*since) >= i64::try_from(after.as_secs()).unwrap_or(i64::MAX)
            });
        let silent = patience
            .silence
            .filter(|_| !waiting_for_more_than_the_model)
            .zip(self.last_heard)
            .filter(|(after, heard)| {
                secs_since(*heard) >= i64::try_from(after.as_secs()).unwrap_or(i64::MAX)
            });
        let error = if let Some((_, since)) = trying {
            failure::gave_up_trying(&turn.notes, secs_since(since))
        } else if let Some((_, heard)) = silent {
            failure::gave_up_waiting(secs_since(heard))
        } else {
            return None;
        };
        let native = turn.native.clone();
        let thread = self.config.native_thread.clone();
        for mut request in self.requests.abort_all(&error.code, Some(&error.message)) {
            if let Some(orch_agent_contract::requests::RequestOutcome::Error { said, .. }) =
                &mut request.outcome
            {
                *said = error.said.clone();
            }
            self.publish_request(request);
        }
        let mut out = Vec::new();
        self.finish_turn(
            &thread,
            TurnOutcome::Failed,
            ItemStatus::Failed,
            ctx,
            &mut out,
            |_, _| error,
        );
        self.foreign_turn = Some(native.clone());
        Some((native, out))
    }

    /// The row of the table of failures (see [`failure`]) that found the last failure of the session.
    #[must_use]
    pub fn last_failure_row(&self) -> Option<&'static str> {
        self.last_failure_row
    }

    /// The thread of Orch.
    #[must_use]
    pub fn thread_id(&self) -> ThreadId {
        self.config.thread_id
    }

    /// The segment of the thread.
    #[must_use]
    pub fn segment_id(&self) -> Id {
        self.config.segment_id
    }

    /// The harness's id for the thread.
    #[must_use]
    pub fn native_thread(&self) -> &str {
        &self.config.native_thread
    }

    /// Whether `thread` is the session's own thread or one of its sub-agents'.
    #[must_use]
    pub fn follows(&self, thread: &str) -> bool {
        thread == self.config.native_thread || self.children.contains_key(thread)
    }

    /// Child identities declared by native spawn items or thread notices, for the process's routing table.
    pub fn child_threads(&self) -> impl Iterator<Item = &str> {
        self.children.keys().map(String::as_str)
    }

    /// Says that a turn of Orch is about to be started in the harness; its `turn/started` is the next.
    pub fn expect_turn(&mut self, turn_id: TurnId) {
        for request in self.requests.abort_all(
            "session_lost",
            Some("The previous turn ended before its request report."),
        ) {
            self.publish_request(request);
        }
        self.request_turn = Some(turn_id);
        self.pending_turn = Some(turn_id);
    }

    /// Records the moment the input is given to the harness, before awaiting its turn/start reply.
    pub fn request_asked(&mut self, at: Time) {
        self.requests = crate::performance::Requests::default();
        self.requests.asked(&self.config.native_thread, at);
    }

    /// Raw model-request observations, carried separately from conversation events.
    pub fn take_requests(&mut self) -> Vec<(TurnId, UsageRequest)> {
        std::mem::take(&mut self.request_reports)
    }

    /// Takes back what [`Mapper::expect_turn`] said, when the harness did not accept the turn.
    pub fn forget_expected_turn(&mut self) {
        self.pending_turn = None;
    }

    /// Whether a turn is expected or runs.
    #[must_use]
    pub fn turn_in_progress(&self) -> bool {
        self.pending_turn.is_some() || self.turn.is_some()
    }

    /// The turn that runs: the neutral id and the harness's.
    #[must_use]
    pub fn running_turn(&self) -> Option<(TurnId, String)> {
        self.turn
            .as_ref()
            .map(|turn| (turn.id, turn.native.clone()))
    }

    /// Whether an open command was started by this turn, including one without a process id yet.
    #[must_use]
    pub fn owns_command(&self, turn: TurnId, item: &str) -> bool {
        self.items
            .get(item)
            .is_some_and(|open| open.kind == ItemKind::Command && open.command_turn == Some(turn))
    }

    /// The recorded turn of a native command, without guessing from the active turn.
    #[must_use]
    pub fn command_turn(&self, item: &str) -> Option<TurnId> {
        self.items
            .get(item)
            .filter(|open| open.kind == ItemKind::Command)
            .and_then(|open| open.command_turn)
    }

    /// Counts only open commands correlated to this exact stopped turn.
    #[must_use]
    pub fn open_commands(&self, turn: TurnId) -> i64 {
        i64::try_from(
            self.items
                .keys()
                .filter(|item| self.owns_command(turn, item))
                .count(),
        )
        .unwrap_or(i64::MAX)
    }

    /// Leaves commands whose end is not known open, with that uncertainty stated on their lines.
    pub fn commands_may_still_run(&mut self, turn: TurnId) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let mut ids: Vec<_> = self.items.keys().cloned().collect();
        ids.sort();
        for id in ids {
            if !self.owns_command(turn, &id) {
                continue;
            }
            if let Some(open) = self.items.get_mut(&id)
                && let Some(call) = open.call.as_mut()
            {
                call.title = format!("Command may still be running: {}", call.title);
                out.push(SessionEvent::ItemUpdated(ItemUpdated {
                    native_ref: id,
                    payload: json!({ "title": call.title }),
                }));
            }
        }
        out
    }

    /// The turn that runs, as a neutral id.
    #[must_use]
    pub fn current_turn_id(&self) -> Option<TurnId> {
        self.turn.as_ref().map(|turn| turn.id)
    }

    /// The changes of the file-change item `item`, as it was last reported, if it is open.
    #[must_use]
    pub fn file_changes_of(&self, item: &str) -> Vec<FileChange> {
        self.items
            .get(item)
            .and_then(|open| open.call.as_ref())
            .and_then(|call| match &call.tool {
                ToolPayload::FileChange { changes } => Some(changes.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn stream_of(&self, thread: &str) -> Option<String> {
        if thread == self.config.native_thread {
            Some(MAIN_STREAM.to_owned())
        } else if self.children.contains_key(thread) {
            Some(sub_stream(thread))
        } else {
            None
        }
    }

    fn turn_for(&self, thread: &str) -> Option<TurnId> {
        if thread == self.config.native_thread {
            self.turn.as_ref().map(|turn| turn.id)
        } else {
            self.children.get(thread).and_then(|child| child.turn_id)
        }
    }

    /// The events a notification of the harness comes to. `method` and `params` are as the harness sent
    /// them; a notification that is not of this session, or means nothing to Orch, gives none.
    pub fn handle(&mut self, method: &str, params: &Json, ctx: &Context<'_>) -> Vec<SessionEvent> {
        let arrived = ctx.clock.time();
        let mut out = Vec::new();
        self.last_heard = Some(ctx.clock.time());
        match method {
            "thread/started" => self.thread_started(params),
            "turn/started" => self.turn_started(params, &mut out),
            "turn/completed" => self.turn_completed(params, &mut out, ctx),
            "item/started" => self.item_started(params, &mut out, ctx),
            "item/completed" => self.item_completed(params, &mut out, ctx),
            "item/agentMessage/delta" => self.text_delta(params, DeltaField::Text, &mut out),
            "item/reasoning/summaryTextDelta" => {
                self.text_delta(params, DeltaField::Summary, &mut out);
            }
            "item/reasoning/textDelta" => self.text_delta(params, DeltaField::Summary, &mut out),
            "item/reasoning/summaryPartAdded" => self.summary_part(params, &mut out),
            "item/commandExecution/outputDelta" => self.output_delta(params, &mut out),
            "item/fileChange/patchUpdated" => self.patch_updated(params, &mut out),
            "turn/plan/updated" => self.plan_updated(params, &mut out),
            "thread/tokenUsage/updated" => self.usage_updated(params, &mut out),
            "item/autoApprovalReview/completed"
                if text(params, "threadId") == self.config.native_thread =>
            {
                out.extend(crate::review::completed(
                    params,
                    self.turn_for(text(params, "threadId")),
                    ctx.clock.time(),
                ));
            }
            "thread/compacted" => self.compacted(params, &mut out, ctx),
            "model/rerouted" => self.rerouted(params, &mut out),
            "thread/name/updated" => Self::named(params, &self.config.native_thread, &mut out),
            "error" => self.error(params, &mut out, ctx),
            "warning" if failure::row_name(params) == "credential_refused" => {
                self.error(params, &mut out, ctx);
            }
            _ => {}
        }
        self.observe_request(method, params, arrived, &out);
        out
    }

    fn observe_request(
        &mut self,
        method: &str,
        params: &Json,
        arrived: Time,
        out: &[SessionEvent],
    ) {
        if !self.follows(text(params, "threadId")) {
            return;
        }
        if method == "thread/tokenUsage/updated"
            && text(params, "threadId") == self.config.native_thread
            && self
                .turn
                .as_ref()
                .is_none_or(|turn| turn.native != text(params, "turnId"))
        {
            return;
        }
        let error = out.iter().find_map(|event| match event {
            SessionEvent::TurnCompleted(t) => t.error.as_ref(),
            _ => None,
        });
        let status = text(params.pointer("/turn").unwrap_or(&Json::Null), "status");
        let code = error
            .map(|e| e.code.as_str())
            .unwrap_or(if status == "interrupted" {
                "forced"
            } else if status == "failed" {
                "harness_error"
            } else {
                "done"
            });
        let mut requests = self.requests.observe(
            method,
            params,
            arrived,
            Some((code, error.map(|e| e.message.as_str()))),
        );
        if method == "turn/completed" && text(params, "threadId") == self.config.native_thread {
            requests.extend(
                self.requests
                    .abort_all(code, error.map(|e| e.message.as_str())),
            );
        }
        for mut request in requests {
            if let Some(orch_agent_contract::requests::RequestOutcome::Error { said, .. }) =
                &mut request.outcome
                && let Some(words) = error.and_then(|error| error.said.clone())
            {
                *said = Some(words);
            }
            self.publish_request(request);
        }
    }

    fn publish_request(&mut self, request: UsageRequest) {
        let turn_id = self.request_turn;
        let Some(turn_id) = turn_id else {
            return;
        };
        self.request_reports.push((turn_id, request));
    }

    // -- threads -------------------------------------------------------------------------------------

    fn thread_started(&mut self, params: &Json) {
        let thread = params.get("thread").unwrap_or(&Json::Null);
        let id = text(thread, "id");
        let parent = text(thread, "parentThreadId");
        if id.is_empty() || parent.is_empty() || id == self.config.native_thread {
            return;
        }
        if self.follows(parent) && !self.children.contains_key(id) {
            let parent_item = self.open_spawn.clone();
            if let Some(item) = &parent_item {
                self.spawn_items.insert(id.to_owned(), item.clone());
            }
            self.children.insert(
                id.to_owned(),
                Child {
                    parent_item,
                    turn_id: self.turn.as_ref().map(|turn| turn.id),
                },
            );
        }
    }

    fn spawn_children(&mut self, item: &Json, parent_thread: &str) {
        if text(item, "type") != "collabAgentToolCall" || text(item, "tool") != "spawnAgent" {
            return;
        }
        let spawn = text(item, "id");
        if spawn.is_empty() {
            return;
        }
        let turn_id = self.turn_for(parent_thread);
        for child in item
            .get("receiverThreadIds")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            if child.is_empty() || child == self.config.native_thread || child == parent_thread {
                continue;
            }
            // The final native spawn identity wins over a tentative association from an earlier notice.
            self.spawn_items.insert(child.to_owned(), spawn.to_owned());
            self.children.insert(
                child.to_owned(),
                Child {
                    parent_item: Some(spawn.to_owned()),
                    turn_id,
                },
            );
        }
    }

    fn named(params: &Json, native_thread: &str, out: &mut Vec<SessionEvent>) {
        if text(params, "threadId") != native_thread {
            return;
        }
        let title = text(params, "threadName");
        if !title.is_empty() {
            out.push(SessionEvent::SessionTitle(SessionTitle {
                title: title.chars().take(TITLE_MAX_CHARS).collect(),
            }));
        }
    }

    // -- turns ---------------------------------------------------------------------------------------

    fn turn_started(&mut self, params: &Json, out: &mut Vec<SessionEvent>) {
        if text(params, "threadId") != self.config.native_thread {
            return;
        }
        let native = text(params.get("turn").unwrap_or(&Json::Null), "id").to_owned();
        if native.is_empty() {
            return;
        }
        let Some(id) = self.pending_turn.take() else {
            tracing::debug!(turn = %native, "a turn started that Orch did not ask for; its events are not reported");
            self.foreign_turn = Some(native);
            return;
        };
        self.foreign_turn = None;
        self.turn = Some(TurnState {
            id,
            native: native.clone(),
            usage: TokenUsage::default(),
            subagents: TokenUsage::default(),
            by_subagent: BTreeMap::new(),
            plan_started: false,
            compaction_item: None,
            notes: Notes::default(),
            failure: None,
            retrying_since: None,
            ran_on: None,
        });
        out.push(SessionEvent::TurnStarted(TurnStarted {
            turn_id: id,
            native_turn_ref: native,
        }));
    }

    fn turn_completed(&mut self, params: &Json, out: &mut Vec<SessionEvent>, ctx: &Context<'_>) {
        let thread = text(params, "threadId");
        let turn = params.get("turn").unwrap_or(&Json::Null);
        let native = text(turn, "id");
        let status = text(turn, "status");
        let (outcome, item_status) = match status {
            "completed" => (TurnOutcome::Completed, ItemStatus::Completed),
            "interrupted" => (TurnOutcome::Interrupted, ItemStatus::Cancelled),
            _ => (TurnOutcome::Failed, ItemStatus::Failed),
        };
        if thread != self.config.native_thread {
            if self.children.contains_key(thread) {
                self.close_items_of(thread, item_status, false, out);
            }
            return;
        }
        if self.foreign_turn.as_deref() == Some(native) {
            self.foreign_turn = None;
            return;
        }
        if self
            .turn
            .as_ref()
            .is_none_or(|state| state.native != native)
        {
            return;
        }
        self.finish_turn(thread, outcome, item_status, ctx, out, |mapper, state| {
            if status != "failed" {
                return TurnError::new(
                    turn_error::HARNESS_ERROR,
                    "the harness returned an unrecognized turn status",
                    false,
                )
                .saying(status);
            }
            // What the turn ends with is read from the error it carries; the one the harness reported
            // as it happened is what is left where the turn carries none.
            turn.get("error")
                .filter(|error| error.is_object())
                .map(|error| {
                    mapper.last_failure_row = Some(failure::row_name(error));
                    failure::classify(
                        error,
                        &state.notes,
                        ctx.windows,
                        ctx.clock.now(),
                        &chrono::Local,
                    )
                })
                .or_else(|| state.failure.clone())
                .unwrap_or_else(|| {
                    TurnError::new(
                        turn_error::HARNESS_ERROR,
                        "the turn failed without a reason from the harness",
                        false,
                    )
                })
        });
    }

    /// Ends the turn that runs: what is open is closed with `item_status`, and the turn completes with
    /// `outcome`, and, for a failure, with the error `error_of` makes of the turn's state.
    fn finish_turn(
        &mut self,
        thread: &str,
        outcome: TurnOutcome,
        item_status: ItemStatus,
        ctx: &Context<'_>,
        out: &mut Vec<SessionEvent>,
        error_of: impl FnOnce(&mut Self, &TurnState) -> TurnError,
    ) {
        self.close_items_of(thread, item_status, true, out);
        let Some(state) = self.turn.take() else {
            return;
        };
        if state.compaction_item.is_some() {
            out.push(SessionEvent::TurnPhase(TurnPhase {
                turn_id: state.id,
                kind: TurnPhaseKind::Working,
                at: ctx.clock.time(),
            }));
        }
        if state.plan_started {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref: plan_ref(&state.native),
                status: item_status,
                payload: Json::Null,
                output_blob: None,
            }));
        }
        let error = (outcome == TurnOutcome::Failed).then(|| error_of(self, &state));
        out.push(SessionEvent::TurnCompleted(TurnCompleted {
            turn_id: state.id,
            status: outcome,
            error,
        }));
    }

    /// Completes the items of a thread that are still open. A command whose process goes on is left open
    /// and marked, when `keep_background` says so: Codex completes it when the process ends.
    fn close_items_of(
        &mut self,
        thread: &str,
        status: ItemStatus,
        keep_background: bool,
        out: &mut Vec<SessionEvent>,
    ) {
        let Some(stream) = self.stream_of(thread) else {
            return;
        };
        let mut ids: Vec<String> = self
            .items
            .iter()
            .filter(|(_, item)| {
                item.stream == stream
                    && (!keep_background
                        || self
                            .turn
                            .as_ref()
                            .is_none_or(|turn| item.turn_id == turn.id))
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        for id in ids {
            let goes_on = self
                .items
                .get(&id)
                .is_some_and(|item| item.kind == ItemKind::Command);
            if keep_background && goes_on {
                if let Some(item) = self.items.get_mut(&id)
                    && !item.background
                {
                    item.background = true;
                    if let Some(call) = item.call.as_mut() {
                        if let ToolPayload::Command { background, .. } = &mut call.tool {
                            *background = true;
                        }
                        out.push(SessionEvent::ItemUpdated(ItemUpdated {
                            native_ref: id.clone(),
                            payload: json!({ "tool": call.tool }),
                        }));
                    }
                }
                continue;
            }
            self.complete_cut_short(&id, status, out);
        }
    }

    /// Completes an item that ends without the harness saying so, with what it had so far.
    fn complete_cut_short(&mut self, id: &str, status: ItemStatus, out: &mut Vec<SessionEvent>) {
        let Some(mut item) = self.items.remove(id) else {
            return;
        };
        if self.pending_reasoning.remove(id).is_some() {
            return;
        }
        let payload = match item.kind {
            ItemKind::AgentMessage => serde_json::to_value(AgentMessagePayload {
                text: item.text,
                text_len: item.text_chars,
                phase: item.phase,
            }),
            ItemKind::Reasoning => serde_json::to_value(ReasoningPayload {
                hidden: item.text.is_empty(),
                summary: item.text,
                text_len: item.text_chars,
            }),
            ItemKind::Command | ItemKind::Tool | ItemKind::UserInput => match item.call.take() {
                Some(mut call) => {
                    if let ToolPayload::Command { output, .. } = &mut call.tool {
                        *output = item.output.clone();
                        call.raw.output = Some(item.output);
                    }
                    serde_json::to_value(call)
                }
                None => Ok(Json::Null),
            },
        }
        .unwrap_or(Json::Null);
        out.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: id.to_owned(),
            status,
            payload,
            output_blob: None,
        }));
    }

    // -- items ---------------------------------------------------------------------------------------

    /// Codex reports this call as a server request, without item/started or item/completed.
    /// Keep that observed request in the same item lifecycle as ordinary native calls.
    pub fn user_input_requested(
        &mut self,
        params: &Json,
        title: &str,
        now: Time,
    ) -> Option<SessionEvent> {
        let id = text(params, "itemId");
        let turn = self.turn.as_ref()?;
        if id.is_empty() || self.items.contains_key(id) || text(params, "turnId") != turn.native {
            return None;
        }
        let call = ToolCallPayload {
            tool: ToolPayload::Other {
                name: "request_user_input".into(),
                arguments: Some(params.clone()),
                result: None,
            },
            title: title.to_owned(),
            started_at: now,
            duration_ms: None,
            raw: RawCall {
                name: "item/tool/requestUserInput".into(),
                input: OutputRef::of(&params.to_string()),
                output: None,
                is_error: false,
            },
        };
        let mut open = OpenItem::new(ItemKind::UserInput, MAIN_STREAM, now, turn.id);
        open.call = Some(call.clone());
        self.items.insert(id.to_owned(), open);
        Some(started_event(
            turn.id,
            id,
            MAIN_STREAM,
            None,
            ItemContent::ToolCall(call),
        ))
    }

    /// Retain the actual written response, without claiming the harness has resolved it yet.
    pub fn user_input_answered(&mut self, id: &str, response: &Json) {
        let Some(open) = self
            .items
            .get_mut(id)
            .filter(|item| item.kind == ItemKind::UserInput)
        else {
            return;
        };
        if let Some(call) = &mut open.call {
            call.raw.output = Some(OutputRef::of(&response.to_string()));
            if let ToolPayload::Other { result, .. } = &mut call.tool {
                *result = Some(response.clone());
            }
        }
    }

    /// Only serverRequest/resolved confirms an answer; an unobserved answer is cancellation.
    pub fn user_input_resolved(&mut self, id: &str, now: Time) -> Vec<SessionEvent> {
        let Some(open) = self
            .items
            .get_mut(id)
            .filter(|item| item.kind == ItemKind::UserInput)
        else {
            return Vec::new();
        };
        let status = if let Some(call) = &mut open.call {
            call.duration_ms = Some((now.unix_millis() - open.started_at.unix_millis()).max(0));
            if call.raw.output.is_some() {
                ItemStatus::Completed
            } else {
                ItemStatus::Cancelled
            }
        } else {
            ItemStatus::Cancelled
        };
        let mut out = Vec::new();
        self.complete_cut_short(id, status, &mut out);
        out
    }

    fn item_started(&mut self, params: &Json, out: &mut Vec<SessionEvent>, ctx: &Context<'_>) {
        let thread = text(params, "threadId");
        let Some(stream) = self.stream_of(thread) else {
            return;
        };
        let Some(turn_id) = self.turn_for(thread) else {
            return;
        };
        let item = params.get("item").unwrap_or(&Json::Null);
        let id = text(item, "id");
        if id.is_empty() || self.items.contains_key(id) {
            return;
        }
        let started_at = params
            .get("startedAtMs")
            .and_then(Json::as_i64)
            .and_then(time_of_millis)
            .unwrap_or_else(|| ctx.clock.time());
        let parent = self
            .children
            .get(thread)
            .and_then(|child| child.parent_item.clone());
        let kind = text(item, "type");
        if !matches!(
            kind,
            "userMessage" | "hookPrompt" | "functionCallOutput" | "plan" | "contextCompaction"
        ) && let Some(turn) = self.turn.as_mut()
        {
            turn.notes.streamed = true;
            turn.retrying_since = None;
        }
        match kind {
            "userMessage" | "hookPrompt" | "functionCallOutput" | "plan" => {}
            "contextCompaction" => {
                if let Some(turn) = self.turn.as_mut() {
                    turn.compaction_item = Some(id.to_owned());
                }
                out.push(SessionEvent::TurnPhase(TurnPhase {
                    turn_id,
                    kind: TurnPhaseKind::Compacting,
                    at: started_at,
                }));
            }
            "agentMessage" => {
                let body = text(item, "text").to_owned();
                let chars = count_chars(&body);
                let phase = message_phase(item);
                let mut open = OpenItem::new(ItemKind::AgentMessage, &stream, started_at, turn_id);
                open.text.clone_from(&body);
                open.text_chars = chars;
                open.phase = phase;
                self.items.insert(id.to_owned(), open);
                out.push(started_event(
                    turn_id,
                    id,
                    &stream,
                    parent,
                    ItemContent::AgentMessage(AgentMessagePayload {
                        text: body,
                        text_len: chars,
                        phase,
                    }),
                ));
            }
            "reasoning" => {
                let summary = reasoning_summary(item);
                let chars = count_chars(&summary);
                let mut open = OpenItem::new(ItemKind::Reasoning, &stream, started_at, turn_id);
                open.text.clone_from(&summary);
                open.text_chars = chars;
                open.summary_parts = item
                    .get("summary")
                    .and_then(Json::as_array)
                    .map_or(0, |parts| i64::try_from(parts.len()).unwrap_or(i64::MAX));
                self.items.insert(id.to_owned(), open);
                let started = started_event(
                    turn_id,
                    id,
                    &stream,
                    parent,
                    ItemContent::Reasoning(ReasoningPayload {
                        summary,
                        text_len: chars,
                        hidden: false,
                    }),
                );
                if chars == 0 {
                    self.pending_reasoning.insert(id.to_owned(), started);
                } else {
                    out.push(started);
                }
            }
            _ => {
                let Some((call, _)) = self.tool_call(item, started_at, None, ctx) else {
                    return;
                };
                let is_command = kind == "commandExecution";
                let mut open = OpenItem::new(
                    if is_command {
                        ItemKind::Command
                    } else {
                        ItemKind::Tool
                    },
                    &stream,
                    started_at,
                    turn_id,
                );
                if is_command {
                    open.command_turn = self
                        .turn
                        .as_ref()
                        .filter(|turn| turn.native == text(params, "turnId"))
                        .map(|turn| turn.id);
                }
                open.call = Some(call.clone());
                if kind == "collabAgentToolCall" && text(item, "tool") == "spawnAgent" {
                    self.open_spawn = Some(id.to_owned());
                    self.spawn_children(item, thread);
                }
                self.items.insert(id.to_owned(), open);
                out.push(started_event(
                    turn_id,
                    id,
                    &stream,
                    parent,
                    ItemContent::ToolCall(call),
                ));
            }
        }
    }

    fn item_completed(&mut self, params: &Json, out: &mut Vec<SessionEvent>, ctx: &Context<'_>) {
        let thread = text(params, "threadId");
        if self.stream_of(thread).is_none() {
            return;
        }
        let item = params.get("item").unwrap_or(&Json::Null);
        let id = text(item, "id");
        let kind = text(item, "type");
        match kind {
            "contextCompaction" => {
                self.compaction_ended(id, out, ctx);
                return;
            }
            "userMessage" | "hookPrompt" | "functionCallOutput" => return,
            "subAgentActivity" => {
                self.sub_agent_activity(item, out);
                return;
            }
            "plan" => {
                self.plan_item_completed(item, out);
                return;
            }
            _ => {}
        }
        if !self.items.contains_key(id) {
            // Completed without having been seen to start, as when a session joins late: start it first.
            let synthetic = json!({ "item": item, "threadId": thread, "startedAtMs": params.get("startedAtMs") });
            self.item_started(&synthetic, out, ctx);
        }
        let Some(open) = self.items.remove(id) else {
            return;
        };
        self.spawn_children(item, thread);
        if kind == "collabAgentToolCall" && self.open_spawn.as_deref() == Some(id) {
            self.open_spawn = None;
        }
        if kind == "collabAgentToolCall" {
            self.children_reported(item, out);
        }
        match open.kind {
            ItemKind::AgentMessage => {
                let body = text(item, "text").to_owned();
                let payload = AgentMessagePayload {
                    text_len: count_chars(&body),
                    text: body,
                    phase: message_phase(item).or(open.phase),
                };
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref: id.to_owned(),
                    status: ItemStatus::Completed,
                    payload: serde_json::to_value(payload).unwrap_or(Json::Null),
                    output_blob: None,
                }));
            }
            ItemKind::Reasoning => {
                let mut summary = reasoning_summary(item);
                if summary.is_empty() {
                    summary = open.text;
                }
                if let Some(started) = self.pending_reasoning.remove(id) {
                    if summary.is_empty() {
                        return;
                    }
                    out.push(started);
                }
                let payload = ReasoningPayload {
                    text_len: count_chars(&summary),
                    summary,
                    hidden: false,
                };
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref: id.to_owned(),
                    status: ItemStatus::Completed,
                    payload: serde_json::to_value(payload).unwrap_or(Json::Null),
                    output_blob: None,
                }));
            }
            ItemKind::Command | ItemKind::Tool | ItemKind::UserInput => {
                let status = item_status_of(item);
                let Some((mut call, blob)) =
                    self.tool_call(item, open.started_at, Some(&open), ctx)
                else {
                    return;
                };
                if call.duration_ms.is_none() {
                    call.duration_ms = params
                        .get("completedAtMs")
                        .and_then(Json::as_i64)
                        .map(|end| (end - open.started_at.unix_millis()).max(0));
                }
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref: id.to_owned(),
                    status,
                    payload: serde_json::to_value(call).unwrap_or(Json::Null),
                    output_blob: blob,
                }));
            }
        }
    }

    // -- deltas --------------------------------------------------------------------------------------

    fn text_delta(&mut self, params: &Json, field: DeltaField, out: &mut Vec<SessionEvent>) {
        let id = text(params, "itemId");
        let delta = text(params, "delta");
        if delta.is_empty() {
            return;
        }
        out.extend(self.pending_reasoning.remove(id));
        let Some(item) = self.items.get_mut(id) else {
            return;
        };
        let offset = item.text_chars;
        item.text.push_str(delta);
        item.text_chars += count_chars(delta);
        out.push(SessionEvent::ItemDelta(ItemDelta::text(
            id, field, offset, delta,
        )));
    }

    fn summary_part(&mut self, params: &Json, out: &mut Vec<SessionEvent>) {
        let id = text(params, "itemId");
        let Some(item) = self.items.get_mut(id) else {
            return;
        };
        item.summary_parts += 1;
        if item.summary_parts > 1 && !item.text.is_empty() {
            let offset = item.text_chars;
            item.text.push_str("\n\n");
            item.text_chars += 2;
            out.push(SessionEvent::ItemDelta(ItemDelta::text(
                id,
                DeltaField::Summary,
                offset,
                "\n\n",
            )));
        }
    }

    fn output_delta(&mut self, params: &Json, out: &mut Vec<SessionEvent>) {
        let id = text(params, "itemId");
        let delta = text(params, "delta");
        if delta.is_empty() {
            return;
        }
        let Some(item) = self.items.get_mut(id) else {
            return;
        };
        let offset = item.output_bytes;
        item.output_bytes += count_bytes(delta);
        item.output.push(delta);
        out.push(SessionEvent::ItemDelta(ItemDelta::output(
            id,
            offset,
            delta.as_bytes(),
        )));
    }

    fn patch_updated(&mut self, params: &Json, out: &mut Vec<SessionEvent>) {
        let id = text(params, "itemId");
        let Some(item) = self.items.get_mut(id) else {
            return;
        };
        let changes: Vec<FileChange> = params
            .get("changes")
            .and_then(Json::as_array)
            .map(|changes| {
                changes
                    .iter()
                    .map(|change| file_change(change, &self.config.directory))
                    .collect()
            })
            .unwrap_or_default();
        let Some(call) = item.call.as_mut() else {
            return;
        };
        call.tool = ToolPayload::FileChange { changes };
        out.push(SessionEvent::ItemUpdated(ItemUpdated {
            native_ref: id.to_owned(),
            payload: json!({ "tool": call.tool }),
        }));
    }

    // -- plan ----------------------------------------------------------------------------------------

    fn plan_updated(&mut self, params: &Json, out: &mut Vec<SessionEvent>) {
        if text(params, "threadId") != self.config.native_thread {
            return;
        }
        let steps: Vec<PlanStep> = params
            .get("plan")
            .and_then(Json::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| PlanStep {
                        text: text(step, "step").to_owned(),
                        status: match text(step, "status") {
                            "completed" => PlanStepStatus::Completed,
                            "inProgress" => PlanStepStatus::InProgress,
                            _ => PlanStepStatus::Pending,
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.put_plan(
            PlanPayload {
                explanation: opt_text(params, "explanation"),
                steps,
            },
            out,
        );
    }

    fn plan_item_completed(&mut self, item: &Json, out: &mut Vec<SessionEvent>) {
        self.put_plan(
            PlanPayload {
                explanation: opt_text(item, "text"),
                steps: Vec::new(),
            },
            out,
        );
    }

    /// The turn's plan: one item for each turn, which is begun by the first plan and updated in place.
    fn put_plan(&mut self, plan: PlanPayload, out: &mut Vec<SessionEvent>) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let native_ref = plan_ref(&turn.native);
        if turn.plan_started {
            let mut payload = json!({ "explanation": plan.explanation });
            if !plan.steps.is_empty() {
                payload["steps"] = json!(plan.steps);
            }
            out.push(SessionEvent::ItemUpdated(ItemUpdated {
                native_ref,
                payload,
            }));
        } else {
            turn.plan_started = true;
            out.push(SessionEvent::ItemStarted(ItemStarted {
                turn_id: turn.id,
                native_ref,
                stream: MAIN_STREAM.to_owned(),
                parent_native_ref: None,
                content: ItemContent::Plan(plan),
            }));
        }
    }

    // -- compaction ----------------------------------------------------------------------------------

    fn compaction_ended(&mut self, id: &str, out: &mut Vec<SessionEvent>, ctx: &Context<'_>) {
        if self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.compaction_item.as_deref() == Some(id))
        {
            self.note_shortening(ctx.clock.time());
        }
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        if turn.compaction_item.as_deref() == Some(id) {
            turn.compaction_item = None;
            out.push(SessionEvent::TurnPhase(TurnPhase {
                turn_id: turn.id,
                kind: TurnPhaseKind::Working,
                at: ctx.clock.time(),
            }));
        }
    }

    fn compacted(&mut self, params: &Json, out: &mut Vec<SessionEvent>, ctx: &Context<'_>) {
        if text(params, "threadId") != self.config.native_thread {
            return;
        }
        if self.turn.is_none() {
            return;
        }
        // The same shortening is noted once: its item's end may have noted it already.
        if self.pending_shortening.is_none() {
            self.note_shortening(ctx.clock.time());
        }
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        if turn.compaction_item.take().is_some() {
            out.push(SessionEvent::TurnPhase(TurnPhase {
                turn_id: turn.id,
                kind: TurnPhaseKind::Working,
                at: ctx.clock.time(),
            }));
            return;
        }
        // Only the second message came: the marker alone, without a duration.
        let marker = MarkerPayload::Compacted { duration_ms: None };
        let native_ref = format!(
            "compacted-{}-{}",
            turn.native,
            ctx.clock.time().unix_millis()
        );
        out.push(SessionEvent::ItemStarted(ItemStarted {
            turn_id: turn.id,
            native_ref: native_ref.clone(),
            stream: MAIN_STREAM.to_owned(),
            parent_native_ref: None,
            content: ItemContent::Marker(marker.clone()),
        }));
        out.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref,
            status: ItemStatus::Completed,
            payload: serde_json::to_value(marker).unwrap_or(Json::Null),
            output_blob: None,
        }));
    }

    // -- usage ---------------------------------------------------------------------------------------

    fn usage_updated(&mut self, params: &Json, out: &mut Vec<SessionEvent>) {
        let thread = text(params, "threadId");
        let usage = params.get("tokenUsage").unwrap_or(&Json::Null);
        let total = token_usage(usage.get("total").unwrap_or(&Json::Null));
        let last = token_usage(usage.get("last").unwrap_or(&Json::Null));
        let window = whole(usage, "modelContextWindow");
        let native_turn = text(params, "turnId");
        if thread == self.config.native_thread {
            self.thread_usage = total;
            // The harness repeats the last usage of a thread when it is resumed, under the id of the turn
            // that made it: that is no use of the turn that runs.
            let Some(turn) = self.turn.as_mut().filter(|turn| turn.native == native_turn) else {
                return;
            };
            turn.usage = turn.usage.plus(last);
            // What fills the context is the prompt of the last request: its input, which already holds the
            // cached and the cache-write parts as this harness counts. Its output is in the next prompt.
            let prompt = last.input_tokens;
            let shortened = self.pending_shortening.take().map(|mut shortening| {
                shortening.after = Some(prompt);
                shortening
            });
            self.last_prompt = Some(prompt);
            out.push(SessionEvent::UsageUpdated(UsageUpdated {
                turn_id: turn.id,
                turn: turn.usage,
                thread: self.thread_usage.plus(self.children_usage),
                context_window: window,
                context_used: Some(prompt),
                report: Some(turn.report(window.map(compact_point), shortened)),
            }));
        } else if self.children.contains_key(thread) {
            self.children_usage = self.children_usage.plus(last);
            let Some(turn) = self.turn.as_mut() else {
                return;
            };
            turn.usage = turn.usage.plus(last);
            turn.subagents = turn.subagents.plus(last);
            let mine = turn.by_subagent.entry(sub_stream(thread)).or_default();
            *mine = mine.plus(last);
            out.push(SessionEvent::UsageUpdated(UsageUpdated {
                turn_id: turn.id,
                turn: turn.usage,
                thread: self.thread_usage.plus(self.children_usage),
                context_window: None,
                context_used: None,
                report: Some(turn.report(None, None)),
            }));
        }
    }

    /// The conversation was shortened: its size before is the last prompt, its size after the next one.
    fn note_shortening(&mut self, at: Time) {
        self.pending_shortening = Some(ContextShortening {
            at,
            before: self.last_prompt,
            after: None,
            automatic: None,
        });
    }

    // -- a turn moved to another model ---------------------------------------------------------------

    /// The harness moved the turn to another model than the one chosen (`model/rerouted`): the turn says so
    /// where it happened, and its use is reported under the model that ran.
    fn rerouted(&mut self, params: &Json, out: &mut Vec<SessionEvent>) {
        if text(params, "threadId") != self.config.native_thread {
            return;
        }
        let to = text(params, "toModel");
        let Some(turn) = self
            .turn
            .as_mut()
            .filter(|turn| turn.native == text(params, "turnId") && !to.is_empty())
        else {
            return;
        };
        turn.ran_on = Some(to.to_owned());
        let why = match text(params, "reason") {
            "highRiskCyberActivity" => "it judged the request high-risk cyber activity".to_owned(),
            "" => "it gave no reason".to_owned(),
            other => format!("its reason: {other}"),
        };
        let native_ref = format!("rerouted-{}-{to}", turn.native);
        let payload = ErrorPayload {
            code: turn_error::MODEL_REROUTED.to_owned(),
            message: format!(
                "The harness ran this turn on {to} instead of {}: {why}.",
                text(params, "fromModel")
            ),
            retryable: false,
        };
        out.push(started_event(
            turn.id,
            &native_ref,
            MAIN_STREAM,
            None,
            ItemContent::Error(payload.clone()),
        ));
        out.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref,
            status: ItemStatus::Completed,
            payload: serde_json::to_value(payload).unwrap_or(Json::Null),
            output_blob: None,
        }));
    }

    // -- errors --------------------------------------------------------------------------------------

    fn error(&mut self, params: &Json, out: &mut Vec<SessionEvent>, ctx: &Context<'_>) {
        if text(params, "threadId") != self.config.native_thread {
            return;
        }
        let error = params.get("error").unwrap_or(params);
        let Some(turn) = self.turn.as_ref().filter(|turn| {
            text(params, "turnId").is_empty() || turn.native == text(params, "turnId")
        }) else {
            return;
        };
        let failure = failure::classify(
            error,
            &turn.notes,
            ctx.windows,
            ctx.clock.now(),
            &chrono::Local,
        );
        if failure.code == turn_error::PROVIDER_UNAUTHORIZED {
            let native = turn.native.clone();
            let native_ref = format!("error-{native}");
            let payload = ErrorPayload {
                code: failure.code.clone(),
                message: failure.message.clone(),
                retryable: failure.retryable,
            };
            out.push(started_event(
                turn.id,
                &native_ref,
                MAIN_STREAM,
                None,
                ItemContent::Error(payload.clone()),
            ));
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref,
                status: ItemStatus::Failed,
                payload: serde_json::to_value(payload).unwrap_or(Json::Null),
                output_blob: None,
            }));
            self.last_failure_row = Some(failure::row_name(error));
            let thread = self.config.native_thread.clone();
            self.finish_turn(
                &thread,
                TurnOutcome::Failed,
                ItemStatus::Failed,
                ctx,
                out,
                |_, _| failure,
            );
            // The vendor has already refused the credential. A late final notice from the harness's
            // retry loop must not end the turn twice or replace that refusal with a generic failure.
            self.foreign_turn = Some(native);
            return;
        }
        if params.get("willRetry").and_then(Json::as_bool) == Some(true) {
            // The request is made again from the start: what the failed attempt streamed is left behind.
            tracing::debug!(
                message = text(error, "message"),
                "the harness retries the request"
            );
            if let Some(turn) = self.turn.as_mut() {
                turn.notes.retries += 1;
                turn.notes
                    .first_retry_at
                    .get_or_insert_with(|| ctx.clock.time());
                turn.retrying_since.get_or_insert_with(|| ctx.clock.time());
                turn.notes.streamed = false;
                turn.notes.last_retry = error
                    .get("additionalDetails")
                    .and_then(Json::as_str)
                    .or_else(|| error.get("message").and_then(Json::as_str))
                    .map(str::to_owned);
            }
            let mut ids: Vec<String> = self
                .items
                .iter()
                .filter(|(_, item)| {
                    matches!(item.kind, ItemKind::AgentMessage | ItemKind::Reasoning)
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.sort();
            for id in ids {
                self.complete_cut_short(&id, ItemStatus::Cancelled, out);
            }
            if let Some(turn) = self.turn.as_ref() {
                let details = text(error, "additionalDetails");
                let message = format!(
                    "The harness is trying again: {}{}{}",
                    text(error, "message"),
                    if details.is_empty() { "" } else { ": " },
                    details
                );
                let native_ref = format!("retry-{}-{}", turn.native, turn.notes.retries);
                let payload = ErrorPayload {
                    code: failure.code,
                    message,
                    retryable: false,
                };
                out.push(started_event(
                    turn.id,
                    &native_ref,
                    MAIN_STREAM,
                    None,
                    ItemContent::Error(payload.clone()),
                ));
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref,
                    status: ItemStatus::Completed,
                    payload: serde_json::to_value(payload).unwrap_or(Json::Null),
                    output_blob: None,
                }));
            }
            return;
        }
        let Some(turn) = self
            .turn
            .as_mut()
            .filter(|turn| turn.native == text(params, "turnId"))
        else {
            return;
        };
        turn.failure = Some(failure.clone());
        self.last_failure_row = Some(failure::row_name(error));
        let native_ref = format!("error-{}-{}", turn.native, ctx.clock.time().unix_millis());
        let payload = ErrorPayload {
            code: failure.code,
            message: failure.message,
            retryable: failure.retryable,
        };
        out.push(SessionEvent::ItemStarted(ItemStarted {
            turn_id: turn.id,
            native_ref: native_ref.clone(),
            stream: MAIN_STREAM.to_owned(),
            parent_native_ref: None,
            content: ItemContent::Error(payload.clone()),
        }));
        out.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref,
            status: ItemStatus::Failed,
            payload: serde_json::to_value(payload).unwrap_or(Json::Null),
            output_blob: None,
        }));
    }

    // -- sub-agents ----------------------------------------------------------------------------------

    fn sub_agent_activity(&mut self, item: &Json, out: &mut Vec<SessionEvent>) {
        let child = text(item, "agentThreadId");
        let Some(spawn) = self.spawn_items.get(child).cloned() else {
            return;
        };
        let state = match text(item, "kind") {
            "interrupted" => SubagentState::Interrupted,
            "completed" => SubagentState::Completed,
            _ => SubagentState::Running,
        };
        let Some(call) = self
            .items
            .get_mut(&spawn)
            .and_then(|open| open.call.as_mut())
        else {
            // The spawn call has completed and the sub-agent goes on: its card is told what became of it.
            self.child_changed(child, state, opt_text(item, "model"), out);
            return;
        };
        self.child_states.insert(child.to_owned(), state);
        if let ToolPayload::Subagent {
            state: current,
            model,
            ..
        } = &mut call.tool
        {
            *current = state;
            // The activity names the model the sub-agent runs on, where the spawn asked for none.
            if let Some(running) = opt_text(item, "model") {
                *model = Some(running);
            }
        }
        out.push(SessionEvent::ItemUpdated(ItemUpdated {
            native_ref: spawn,
            payload: json!({ "tool": call.tool }),
        }));
    }

    /// Tells the card of the call that started the sub-agent of thread `child` its new state, where it
    /// changed, after that call has completed: only the state and the model are told, so that the rest of
    /// the card stays as the call left it.
    fn child_changed(
        &mut self,
        child: &str,
        state: SubagentState,
        model: Option<String>,
        out: &mut Vec<SessionEvent>,
    ) {
        let Some(spawn) = self.spawn_items.get(child).cloned() else {
            return;
        };
        if self.child_states.get(child) == Some(&state) && model.is_none() {
            return;
        }
        self.child_states.insert(child.to_owned(), state);
        let mut tool = json!({ "state": state });
        if let Some(model) = model {
            tool["model"] = json!(model);
        }
        out.push(SessionEvent::ItemUpdated(ItemUpdated {
            native_ref: spawn,
            payload: json!({ "tool": tool }),
        }));
    }

    /// What a completed call of the collaboration tools says of the sub-agents it names (`agentsStates`):
    /// each one's card follows.
    fn children_reported(&mut self, item: &Json, out: &mut Vec<SessionEvent>) {
        let Some(states) = item.get("agentsStates").and_then(Json::as_object) else {
            return;
        };
        let mut reported: Vec<(String, SubagentState)> = states
            .iter()
            .filter_map(|(child, state)| {
                child_state(text(state, "status")).map(|state| (child.clone(), state))
            })
            .collect();
        reported.sort_by(|a, b| a.0.cmp(&b.0));
        for (child, state) in reported {
            if self.spawn_items.get(&child).map(String::as_str) == Some(text(item, "id")) {
                // The spawn call itself: its own card says it.
                self.child_states.insert(child, state);
                continue;
            }
            self.child_changed(&child, state, None, out);
        }
    }

    // -- the tool calls ------------------------------------------------------------------------------

    /// The call a tool item stands for, both ways, and the blob that holds the whole of its output.
    #[allow(
        clippy::too_many_lines,
        reason = "one arm for each kind of tool item, each building the two forms of its call"
    )]
    fn tool_call(
        &self,
        item: &Json,
        started_at: Time,
        open: Option<&OpenItem>,
        ctx: &Context<'_>,
    ) -> Option<(ToolCallPayload, Option<BlobRef>)> {
        let kind = text(item, "type");
        let directory = &self.config.directory;
        let mut blob = None;
        let keep = |body: &str, blob: &mut Option<BlobRef>| -> OutputRef {
            let capped = cap_middle(body);
            let mut output = OutputRef::of(&capped);
            if output.truncated {
                match ctx.outputs.put(capped.as_bytes()) {
                    Ok(kept) => {
                        output = output.with_blob(kept.hash);
                        blob.get_or_insert(kept);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "the whole of a long output could not be kept");
                    }
                }
            }
            output
        };
        let (tool, title, input, result, is_error) = match kind {
            "commandExecution" => {
                let command = text(item, "command");
                let shown = display_command(command);
                let status = text(item, "status");
                let output = match (item.get("aggregatedOutput").and_then(Json::as_str), open) {
                    (Some(body), _) => keep(body, &mut blob),
                    (None, Some(open)) => open.output.clone(),
                    (None, None) => OutputRef::default(),
                };
                let exit_code = whole(item, "exitCode");
                let tool = ToolPayload::Command {
                    command: shown.clone(),
                    cwd: text(item, "cwd").to_owned(),
                    exit_code,
                    output: output.clone(),
                    process_id: opt_text(item, "processId"),
                    background: open.is_some_and(|open| open.background),
                    actions: command_actions(item),
                };
                let input = json!({
                    "command": command, "cwd": item.get("cwd"), "processId": item.get("processId"),
                    "source": item.get("source"), "commandActions": item.get("commandActions"),
                });
                let failed = matches!(status, "failed" | "declined")
                    || exit_code.is_some_and(|code| code != 0);
                let result = (status != "inProgress" && status != "declined").then_some(output);
                (tool, first_line(&shown), input, result, failed)
            }
            "fileChange" => {
                let changes = file_changes(item, directory);
                let title = match changes.as_slice() {
                    [] => "Edit files".to_owned(),
                    [one] => format!("Edit {}", one.path),
                    many => format!("Edit {} files", many.len()),
                };
                let status = text(item, "status");
                let input = json!({ "changes": item.get("changes") });
                let result =
                    (status != "inProgress" && status != "declined").then(|| OutputRef::of(status));
                let failed = matches!(status, "failed" | "declined");
                (
                    ToolPayload::FileChange { changes },
                    title,
                    input,
                    result,
                    failed,
                )
            }
            "mcpToolCall" => {
                let server = text(item, "server").to_owned();
                let tool_name = text(item, "tool").to_owned();
                let arguments = item.get("arguments").cloned().unwrap_or(Json::Null);
                let result = item
                    .get("result")
                    .filter(|result| !result.is_null())
                    .cloned();
                let error = item
                    .get("error")
                    .filter(|error| !error.is_null())
                    .map(|error| text(error, "message").to_owned());
                let orch = (server == ORCH_SERVER)
                    .then(|| OrchTool::parse(&tool_name))
                    .flatten();
                let tool = match orch {
                    Some(tool) => ToolPayload::Orch {
                        tool,
                        arguments: arguments.clone(),
                        result: result.clone(),
                    },
                    None => ToolPayload::Mcp {
                        server: server.clone(),
                        tool: tool_name.clone(),
                        arguments: arguments.clone(),
                        result: result.clone(),
                        error: error.clone(),
                    },
                };
                let input = json!({ "server": server, "tool": tool_name, "arguments": arguments });
                let output = result
                    .as_ref()
                    .map(|result| keep(&result.to_string(), &mut blob))
                    .or_else(|| error.as_deref().map(OutputRef::of));
                let failed = error.is_some() || text(item, "status") == "failed";
                (
                    tool,
                    format!("{server}: {tool_name}"),
                    input,
                    output,
                    failed,
                )
            }
            "webSearch" => {
                let query = opt_text(item, "query")
                    .or_else(|| {
                        item.get("action")
                            .and_then(|action| opt_text(action, "query"))
                    })
                    .unwrap_or_default();
                let results: Vec<WebSearchResult> = item
                    .get("results")
                    .and_then(Json::as_array)
                    .map(|found| {
                        found
                            .iter()
                            .filter_map(|result| {
                                Some(WebSearchResult {
                                    title: text(result, "title").to_owned(),
                                    url: result.get("url").and_then(Json::as_str)?.to_owned(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let input = json!({ "query": item.get("query"), "action": item.get("action") });
                let output = item
                    .get("results")
                    .filter(|results| !results.is_null())
                    .map(|results| keep(&results.to_string(), &mut blob));
                let title = format!("Search the web: {}", first_line(&query));
                (
                    ToolPayload::WebSearch { query, results },
                    title,
                    input,
                    output,
                    false,
                )
            }
            "collabAgentToolCall" => {
                let child = item
                    .get("receiverThreadIds")
                    .and_then(Json::as_array)
                    .and_then(|ids| ids.first())
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                let tool_name = text(item, "tool");
                let state = if tool_name == "spawnAgent" {
                    // A spawn's card stands for the sub-agent it started: the call completes once the
                    // sub-agent is started, and the sub-agent's own state is what the card says.
                    item.get("agentsStates")
                        .and_then(|states| states.get(child))
                        .and_then(|state| child_state(text(state, "status")))
                        .unwrap_or(match text(item, "status") {
                            "failed" => SubagentState::Failed,
                            "interrupted" => SubagentState::Interrupted,
                            _ => SubagentState::Running,
                        })
                } else {
                    match text(item, "status") {
                        "completed" => SubagentState::Completed,
                        "failed" => SubagentState::Failed,
                        "interrupted" => SubagentState::Interrupted,
                        _ => SubagentState::Running,
                    }
                };
                let tool = ToolPayload::Subagent {
                    child_stream: if child.is_empty() {
                        String::new()
                    } else {
                        sub_stream(child)
                    },
                    name: None,
                    role: None,
                    prompt: opt_text(item, "prompt"),
                    state,
                    model: opt_text(item, "model"),
                };
                let input = json!({
                    "tool": tool_name, "senderThreadId": item.get("senderThreadId"),
                    "receiverThreadIds": item.get("receiverThreadIds"), "prompt": item.get("prompt"),
                    "model": item.get("model"), "reasoningEffort": item.get("reasoningEffort"),
                });
                let output = item
                    .get("agentsStates")
                    .filter(|states| states.as_object().is_some_and(|states| !states.is_empty()))
                    .map(|states| OutputRef::of(&states.to_string()));
                (
                    tool,
                    format!("Sub-agent: {tool_name}"),
                    input,
                    output,
                    state == SubagentState::Failed,
                )
            }
            "imageView" | "imageGeneration" => {
                let path = if kind == "imageView" {
                    text(item, "path")
                } else {
                    text(item, "savedPath")
                };
                let caption = opt_text(item, "revisedPrompt");
                let input = if kind == "imageView" {
                    json!({ "path": path })
                } else {
                    json!({ "revisedPrompt": item.get("revisedPrompt"), "status": item.get("status") })
                };
                let title = format!("Image: {}", first_line(&relative_to(path, directory)));
                let tool = match image_blob(path, ctx.outputs) {
                    Some((hash, media_type)) => ToolPayload::Image {
                        blob: hash,
                        media_type,
                        name: Path::new(path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned()),
                        caption,
                    },
                    None => ToolPayload::Other {
                        name: kind.to_owned(),
                        arguments: Some(input.clone()),
                        result: None,
                    },
                };
                (tool, title, input, None, false)
            }
            "dynamicToolCall" => {
                let arguments = json!({
                    "namespace": item.get("namespace"), "tool": item.get("tool"),
                    "arguments": item.get("arguments"),
                });
                let result = item
                    .get("contentItems")
                    .filter(|items| !items.is_null())
                    .cloned();
                let failed = item.get("success").and_then(Json::as_bool) == Some(false);
                let output = result
                    .as_ref()
                    .map(|result| keep(&result.to_string(), &mut blob));
                (
                    ToolPayload::Other {
                        name: kind.to_owned(),
                        arguments: Some(arguments.clone()),
                        result,
                    },
                    text(item, "tool").to_owned(),
                    arguments,
                    output,
                    failed,
                )
            }
            "" => return None,
            other => {
                // Sleeps, review markers and whatever a later harness adds: kept whole, as a tool of kind
                // `other` named by the item's type.
                let mut arguments = item.clone();
                if let Some(members) = arguments.as_object_mut() {
                    members.remove("id");
                    members.remove("type");
                }
                (
                    ToolPayload::Other {
                        name: other.to_owned(),
                        arguments: Some(arguments.clone()),
                        result: None,
                    },
                    other.to_owned(),
                    arguments,
                    None,
                    false,
                )
            }
        };
        let raw = RawCall {
            name: kind.to_owned(),
            input: keep(&input.to_string(), &mut blob),
            output: result,
            is_error,
        };
        Some((
            ToolCallPayload {
                tool,
                title,
                started_at,
                duration_ms: whole(item, "durationMs"),
                raw,
            },
            blob,
        ))
    }

    // -- the end of the session ----------------------------------------------------------------------

    /// What a session whose harness is gone comes to: every item that was open ends, and the session is
    /// lost.
    pub fn lost(&mut self, reason: &str) -> Vec<SessionEvent> {
        for request in self.requests.abort_all("session_lost", Some(reason)) {
            self.publish_request(request);
        }
        let mut out = Vec::new();
        let mut ids: Vec<String> = self.items.keys().cloned().collect();
        ids.sort();
        for id in ids {
            self.complete_cut_short(&id, ItemStatus::Cancelled, &mut out);
        }
        // A sub-agent of the session goes with it.
        let mut running: Vec<String> = self
            .child_states
            .iter()
            .filter(|(_, state)| **state == SubagentState::Running)
            .map(|(child, _)| child.clone())
            .collect();
        running.sort();
        for child in running {
            self.child_changed(&child, SubagentState::Interrupted, None, &mut out);
        }
        self.turn = None;
        self.pending_turn = None;
        out.push(SessionEvent::SessionLost(SessionLost {
            reason: reason.to_owned(),
        }));
        out
    }
}

/// What a sub-agent's state is, from the status a call of the collaboration tools gives it; none for a
/// status this adapter does not know.
fn child_state(status: &str) -> Option<SubagentState> {
    match status {
        "pendingInit" | "running" => Some(SubagentState::Running),
        "completed" => Some(SubagentState::Completed),
        "errored" => Some(SubagentState::Failed),
        "interrupted" | "shutdown" | "notFound" => Some(SubagentState::Interrupted),
        _ => None,
    }
}

/// A raw frame of the harness's protocol, for the session's diagnostics.
#[must_use]
pub fn raw_frame(direction: &str, payload: &Json) -> SessionEvent {
    SessionEvent::RawFrame(RawFrame {
        direction: direction.to_owned(),
        payload: payload.clone(),
    })
}

fn plan_ref(native_turn: &str) -> String {
    format!("plan-{native_turn}")
}

fn started_event(
    turn_id: TurnId,
    native_ref: &str,
    stream: &str,
    parent: Option<String>,
    content: ItemContent,
) -> SessionEvent {
    SessionEvent::ItemStarted(ItemStarted {
        turn_id,
        native_ref: native_ref.to_owned(),
        stream: stream.to_owned(),
        parent_native_ref: if stream == MAIN_STREAM { None } else { parent },
        content,
    })
}

fn time_of_millis(millis: i64) -> Option<Time> {
    chrono::DateTime::from_timestamp_millis(millis).map(Time::new)
}

fn message_phase(item: &Json) -> Option<AgentMessagePhase> {
    match text(item, "phase") {
        "commentary" => Some(AgentMessagePhase::Commentary),
        "final_answer" | "partial_answer" => Some(AgentMessagePhase::Final),
        _ => None,
    }
}

/// The summary of a reasoning item: its parts, one after the other with a blank line between.
fn reasoning_summary(item: &Json) -> String {
    item.get("summary")
        .and_then(Json::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

fn item_status_of(item: &Json) -> ItemStatus {
    match text(item, "status") {
        "failed" => ItemStatus::Failed,
        "declined" | "interrupted" => ItemStatus::Cancelled,
        "inProgress" => ItemStatus::InProgress,
        _ => ItemStatus::Completed,
    }
}

/// A breakdown as the harness counts it: its input includes the cached and the cache-write parts, as OpenAI
/// counts, and its output the reasoning, which is how Orch's tokens are counted too.
fn token_usage(breakdown: &Json) -> TokenUsage {
    TokenUsage {
        input_tokens: whole(breakdown, "inputTokens").unwrap_or_default(),
        cached_input_tokens: whole(breakdown, "cachedInputTokens").unwrap_or_default(),
        // Zero where the upstream writes no cache apart (it is a member with a default of its own).
        cache_write_tokens: whole(breakdown, "cacheWriteInputTokens").unwrap_or_default(),
        output_tokens: whole(breakdown, "outputTokens").unwrap_or_default(),
        reasoning_tokens: whole(breakdown, "reasoningOutputTokens").unwrap_or_default(),
        total_tokens: whole(breakdown, "totalTokens").unwrap_or_default(),
    }
}

/// How much of the window it works with this harness reports as usable, in twentieths: nineteen, 95 % (it was
/// seen to report 828 400 of 872 000, and 258 400 of 272 000).
const REPORTED_TWENTIETHS: i64 = 19;
/// How many twentieths make the whole window.
const TWENTIETHS: i64 = 20;

/// Where the harness shortens the conversation, from the window it reports: that is 95 % of the window it works
/// with, and it shortens at nine tenths of that window, its own rule and the one the adapter writes out (for a
/// reported 258 400, at 244 800).
fn compact_point(reported_window: i64) -> i64 {
    crate::parameters::compact_limit(
        reported_window.saturating_mul(TWENTIETHS) / REPORTED_TWENTIETHS,
    )
}

/// The picture a tool item points to, kept as a blob of its own, with the type its name says. `None` when
/// the file is not on this machine or is not a picture of a kind the mapper knows.
fn image_blob(path: &str, outputs: &OutputStore) -> Option<(Hash, String)> {
    if path.is_empty() {
        return None;
    }
    let media_type = match Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    let kept = outputs.put(&bytes).ok()?;
    Some((kept.hash, media_type.to_owned()))
}

/// The events a history of turns comes to when it is reported again, as `item.started` and `item.completed`
/// for each item and `turn.completed` for each turn that ended, under turn ids that `turn_id` makes from the
/// harness's. A history read twice gives the same events, which a receiver applies once.
pub fn replay(
    turns: &[Json],
    config: MapperConfig,
    ctx: &Context<'_>,
    turn_id: impl Fn(&str) -> TurnId,
) -> Vec<SessionEvent> {
    Replay::new(config).page(turns, ctx, turn_id)
}

/// A history reported again a page of turns at a time: the mapper is kept from one page to the next, and the
/// turns of a page are let go once its events are made, so that a history of any length is held no more than a
/// page at once.
pub struct Replay {
    mapper: Mapper,
    native_thread: String,
}

impl Replay {
    /// A report of the history of the thread `config` names, before its first page.
    #[must_use]
    pub fn new(config: MapperConfig) -> Self {
        Self {
            native_thread: config.native_thread.clone(),
            mapper: Mapper::new(config),
        }
    }

    /// The events of the next turns of the history, oldest first, as [`replay`] makes them.
    pub fn page(
        &mut self,
        turns: &[Json],
        ctx: &Context<'_>,
        turn_id: impl Fn(&str) -> TurnId,
    ) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        for turn in turns {
            let native = text(turn, "id");
            if native.is_empty() || text(turn, "status") == "inProgress" {
                continue;
            }
            let clock = recorded_clock(turn);
            let recorded = Context {
                clock: &clock,
                outputs: ctx.outputs,
                windows: ctx.windows,
            };
            self.mapper.expect_turn(turn_id(native));
            let _ = self.mapper.handle(
                "turn/started",
                &json!({ "threadId": self.native_thread, "turn": { "id": native } }),
                &recorded,
            );
            for item in turn
                .get("items")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                let completed =
                    json!({ "threadId": self.native_thread, "turnId": native, "item": item });
                out.extend(self.mapper.handle("item/completed", &completed, &recorded));
            }
            out.extend(self.mapper.handle(
                "turn/completed",
                &json!({ "threadId": self.native_thread, "turn": turn }),
                &recorded,
            ));
        }
        out
    }
}

/// The moment a turn of the harness states in seconds under `key`.
fn turn_time(turn: &Json, key: &str) -> Option<Time> {
    whole(turn, key)
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(Time::new)
}

/// Full turn pages have native turn dates, not the item-list timing envelope.
/// Synthetic history events use that stable anchor: start, otherwise known end.
/// This is a coarse fallback, not a recovered per-item start time.
/// A legacy turn with neither uses epoch zero for the mandatory tool timestamp;
/// its optional recorded `at` remains unknown. Reading never dates old work anew.
fn recorded_clock(turn: &Json) -> orch_types::ManualClock {
    let at = turn_time(turn, "startedAt")
        .or_else(|| turn_time(turn, "completedAt"))
        .map_or(chrono::DateTime::UNIX_EPOCH, Time::get);
    orch_types::ManualClock::new(at)
}

/// The words of a person's message, and how many of its parts were something other than words (a picture, a
/// sound, a skill or a file named), which are not carried.
fn said_words(item: &Json) -> (String, i64) {
    let mut words: Vec<&str> = Vec::new();
    let mut other = 0i64;
    for part in item
        .get("content")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        match (text(part, "type"), part.get("text").and_then(Json::as_str)) {
            ("text", Some(said)) => words.push(said),
            _ => other += 1,
        }
    }
    (words.join("\n\n"), other)
}

/// What a history of turns comes to as a record, for a person who adopts the thread: for each turn that ended,
/// what the person said in it and the items the mapper makes of the rest, each where it began in the harness's
/// record and under the `native_ref` that [`replay`] gives the same item. A picture or a file that came with the
/// words is counted and not carried.
pub fn record(
    turns: &[Json],
    config: MapperConfig,
    ctx: &Context<'_>,
    turn_id: impl Fn(&str) -> TurnId,
) -> Vec<Vec<RecordedEntry>> {
    let native_thread = config.native_thread.clone();
    let mut mapper = Mapper::new(config);
    let mut record = RecordBuilder::new();
    for turn in turns {
        let native = text(turn, "id");
        if native.is_empty() || text(turn, "status") == "inProgress" {
            continue;
        }
        let started_at = turn_time(turn, "startedAt");
        let ended_at = turn_time(turn, "completedAt").or(started_at);
        let clock = recorded_clock(turn);
        let recorded = Context {
            clock: &clock,
            outputs: ctx.outputs,
            windows: ctx.windows,
        };
        record.begin_turn();
        mapper.expect_turn(turn_id(native));
        let _ = mapper.handle(
            "turn/started",
            &json!({ "threadId": native_thread, "turn": { "id": native } }),
            &recorded,
        );
        for item in turn
            .get("items")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if text(item, "type") == "userMessage" {
                let (words, left_out) = said_words(item);
                record.said(text(item, "id"), words, left_out, started_at);
                continue;
            }
            let completed = json!({ "threadId": native_thread, "turnId": native, "item": item });
            record.events(
                mapper.handle("item/completed", &completed, &recorded),
                ended_at,
            );
        }
        record.events(
            mapper.handle(
                "turn/completed",
                &json!({ "threadId": native_thread, "turn": turn }),
                &recorded,
            ),
            ended_at,
        );
    }
    record.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_agent_contract::events::{HarnessRequestKind, OUTPUT_INLINE_MAX_BYTES};
    use orch_types::ManualClock;

    const THREAD: &str = "thr_main";
    const TURN: &str = "turn_1";

    fn id(n: u8) -> Id {
        format!("0199abcd-1234-7000-8000-0000000000{n:02}")
            .parse()
            .unwrap()
    }

    struct Rig {
        mapper: Mapper,
        outputs: OutputStore,
        clock: ManualClock,
        windows: Option<Windows>,
        _dir: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            Self {
                mapper: Mapper::new(MapperConfig {
                    thread_id: id(1),
                    segment_id: id(2),
                    native_thread: THREAD.into(),
                    directory: "/workspace/web".into(),
                }),
                outputs: OutputStore::new(dir.path().join("outputs")),
                clock: ManualClock::new(
                    chrono::Utc
                        .with_ymd_and_hms(2026, 10, 11, 10, 0, 0)
                        .unwrap(),
                ),
                windows: None,
                _dir: dir,
            }
        }

        fn feed(&mut self, method: &str, params: Json) -> Vec<SessionEvent> {
            let ctx = Context {
                clock: &self.clock,
                outputs: &self.outputs,
                windows: self.windows.as_ref(),
            };
            self.mapper.handle(method, &params, &ctx)
        }

        fn begin(&mut self) -> Vec<SessionEvent> {
            self.mapper.expect_turn(id(9));
            self.feed(
                "turn/started",
                json!({ "threadId": THREAD, "turn": { "id": TURN } }),
            )
        }

        fn item_started(&mut self, item: Json) -> Vec<SessionEvent> {
            self.feed("item/started", json!({ "threadId": THREAD, "turnId": TURN, "item": item, "startedAtMs": 1_791_716_400_000_i64 }))
        }

        fn item_completed(&mut self, item: Json) -> Vec<SessionEvent> {
            self.feed("item/completed", json!({ "threadId": THREAD, "turnId": TURN, "item": item, "completedAtMs": 1_791_716_401_500_i64 }))
        }

        fn end(&mut self, status: &str, error: Json) -> Vec<SessionEvent> {
            self.feed("turn/completed", json!({ "threadId": THREAD, "turn": { "id": TURN, "status": status, "error": error } }))
        }
    }

    fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
        events.iter().map(SessionEvent::kind).collect()
    }

    #[test]
    fn a_missing_or_unknown_turn_status_is_not_reported_as_success() {
        for status in ["", "new_status"] {
            let mut rig = Rig::new();
            rig.begin();
            let events = rig.end(status, Json::Null);
            let [SessionEvent::TurnCompleted(end)] = events.as_slice() else {
                panic!("one completed event");
            };
            assert_eq!(end.status, TurnOutcome::Failed);
            let error = end.error.as_ref().expect("the failure has a reason");
            assert_eq!(error.code, turn_error::HARNESS_ERROR);
            assert!(error.message.contains("unrecognized turn status"));
            assert_eq!(error.said.as_deref(), Some(status));
        }
    }

    #[test]
    fn native_question_rpc_keeps_its_input_and_answer_until_native_resolution() {
        let mut rig = Rig::new();
        rig.begin();
        let request = json!({"threadId":THREAD,"turnId":TURN,"itemId":"question",
            "questions":[{"id":"choice","question":"Which?"}]});
        let started = rig
            .mapper
            .user_input_requested(&request, "Which?", rig.clock.time())
            .unwrap();
        let SessionEvent::ItemStarted(item) = started else {
            panic!("question source")
        };
        assert_eq!(item.turn_id, id(9));
        assert_eq!(item.native_ref, "question");
        let ItemContent::ToolCall(call) = item.content else {
            panic!("native call")
        };
        assert_eq!(
            serde_json::from_str::<Json>(&call.raw.input.inline).unwrap(),
            request
        );
        assert!(call.raw.output.is_none());
        assert!(
            rig.mapper
                .user_input_requested(&request, "Which?", rig.clock.time())
                .is_none()
        );
        let response = json!({"answers":{"choice":{"answers":["  exact answer  "]}}});
        rig.mapper.user_input_answered("question", &response);
        assert!(
            rig.mapper.items.contains_key("question"),
            "writing an answer does not confirm native resolution"
        );
        let completed = rig.mapper.user_input_resolved("question", rig.clock.time());
        let [SessionEvent::ItemCompleted(item)] = completed.as_slice() else {
            panic!("one completion")
        };
        assert_eq!(item.status, ItemStatus::Completed);
        let call: ToolCallPayload = serde_json::from_value(item.payload.clone()).unwrap();
        assert_eq!(
            serde_json::from_str::<Json>(&call.raw.output.unwrap().inline).unwrap(),
            response
        );
        assert!(
            rig.mapper
                .user_input_resolved("question", rig.clock.time())
                .is_empty()
        );
        assert_eq!(kinds(&rig.end("completed", Json::Null)), ["turn.completed"]);
    }

    #[test]
    fn native_question_withdrawal_and_turn_or_session_loss_never_invent_success() {
        for (end, answered) in [
            ("resolved", false),
            ("turn", false),
            ("turn", true),
            ("lost", false),
            ("lost", true),
        ] {
            let mut rig = Rig::new();
            rig.begin();
            let request =
                json!({"threadId":THREAD,"turnId":TURN,"itemId":"question","questions":[]});
            assert!(
                rig.mapper
                    .user_input_requested(&request, "Question", rig.clock.time())
                    .is_some()
            );
            if answered {
                rig.mapper
                    .user_input_answered("question", &json!({"answers":{}}));
            }
            let events = match end {
                "resolved" => rig.mapper.user_input_resolved("question", rig.clock.time()),
                "turn" => rig.end("interrupted", Json::Null),
                _ => rig.mapper.lost("gone"),
            };
            let ended: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemCompleted(item) if item.native_ref == "question" => {
                        Some(item)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(ended.len(), 1);
            assert_eq!(ended[0].status, ItemStatus::Cancelled);
            assert!(
                rig.mapper
                    .user_input_resolved("question", rig.clock.time())
                    .is_empty()
            );
        }
    }

    #[test]
    fn a_question_from_an_old_native_turn_cannot_be_an_item_of_the_current_turn() {
        let mut rig = Rig::new();
        rig.begin();
        let request =
            json!({"threadId":THREAD,"turnId":"old-turn","itemId":"question","questions":[]});
        assert!(
            rig.mapper
                .user_input_requested(&request, "Question", rig.clock.time())
                .is_none()
        );
        assert!(rig.mapper.items.is_empty());
    }

    fn message(id: &str, text: &str) -> Json {
        json!({ "type": "agentMessage", "id": id, "text": text, "phase": null, "memoryCitation": null, "delivery": null, "questions": null })
    }

    fn command(id: &str, status: &str, process: Json, output: Json, exit: Json) -> Json {
        json!({ "type": "commandExecution", "id": id, "pluginId": null, "scriptPath": null,
                "command": "/bin/bash -lc 'cargo test && echo done'", "cwd": "/workspace/web/api", "processId": process,
                "source": "agent", "status": status, "commandActions": [ { "type": "unknown", "command": "cargo test && echo done" } ],
                "aggregatedOutput": output, "exitCode": exit, "durationMs": null })
    }

    #[test]
    fn what_a_person_said_is_never_reported_and_what_the_agent_says_arrives_in_pieces() {
        let mut rig = Rig::new();
        let began = rig.begin();
        let SessionEvent::TurnStarted(started) = &began[0] else {
            panic!("the turn begins");
        };
        assert_eq!(
            (started.turn_id, started.native_turn_ref.as_str()),
            (id(9), TURN)
        );
        let user = json!({ "type": "userMessage", "id": "u1", "clientId": null, "content": [ { "type": "text", "text": "hello", "text_elements": [] } ] });
        assert!(rig.item_started(user.clone()).is_empty() && rig.item_completed(user).is_empty());

        let started = rig.item_started(message("m1", ""));
        let SessionEvent::ItemStarted(item) = &started[0] else {
            panic!("an item");
        };
        assert_eq!(
            (item.native_ref.as_str(), item.stream.as_str(), item.turn_id),
            ("m1", "main", id(9))
        );
        let delta = |text: &str| {
            rig.feed(
                "item/agentMessage/delta",
                json!({ "threadId": THREAD, "turnId": TURN, "itemId": "m1", "delta": text }),
            )
        };
        let _ = delta;
        let first = rig.feed(
            "item/agentMessage/delta",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "m1", "delta": "añb" }),
        );
        let second = rig.feed(
            "item/agentMessage/delta",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "m1", "delta": "cd" }),
        );
        let (SessionEvent::ItemDelta(one), SessionEvent::ItemDelta(two)) = (&first[0], &second[0])
        else {
            panic!("deltas");
        };
        assert_eq!(
            (one.offset, one.len),
            (0, 3),
            "the offsets count code points"
        );
        assert_eq!((two.offset, two.len), (3, 2));
        let done = rig.item_completed(message("m1", "añbcd"));
        let SessionEvent::ItemCompleted(item) = &done[0] else {
            panic!("completed");
        };
        assert_eq!(item.status, ItemStatus::Completed);
        assert_eq!(
            item.payload,
            json!({ "text": "añbcd", "text_len": 5, "phase": null })
        );
    }

    #[test]
    fn a_command_is_reported_as_the_agent_wrote_it_and_as_the_harness_made_it() {
        let mut rig = Rig::new();
        rig.begin();
        let started = rig.item_started(command(
            "c1",
            "inProgress",
            Json::Null,
            Json::Null,
            Json::Null,
        ));
        let SessionEvent::ItemStarted(item) = &started[0] else {
            panic!("an item");
        };
        let ItemContent::ToolCall(call) = &item.content else {
            panic!("a tool call");
        };
        assert_eq!(call.title, "cargo test && echo done");
        let ToolPayload::Command {
            command: shown,
            cwd,
            exit_code,
            background,
            process_id,
            actions,
            ..
        } = &call.tool
        else {
            panic!("a command");
        };
        assert_eq!(shown, "cargo test && echo done");
        assert_eq!(
            (cwd.as_str(), *exit_code, *background, process_id.as_deref()),
            ("/workspace/web/api", None, false, None)
        );
        assert_eq!(actions, &[CommandAction::Other]);
        assert_eq!(call.raw.name, "commandExecution");
        let raw: Json = serde_json::from_str(&call.raw.input.inline).unwrap();
        assert_eq!(
            raw["command"], "/bin/bash -lc 'cargo test && echo done'",
            "the harness's own command line, whole"
        );
        assert!(call.raw.output.is_none());
        assert_eq!(call.started_at.to_string(), "2026-10-11T11:00:00.000Z");

        let output = rig.feed(
            "item/commandExecution/outputDelta",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "c1", "delta": "ok\n" }),
        );
        let SessionEvent::ItemDelta(delta) = &output[0] else {
            panic!("output");
        };
        assert_eq!(
            (delta.field, delta.offset, delta.len),
            (DeltaField::Output, 0, 3)
        );
        let done = rig.item_completed(command(
            "c1",
            "completed",
            json!("4242"),
            json!("ok\nall green\n"),
            json!(0),
        ));
        let SessionEvent::ItemCompleted(item) = &done[0] else {
            panic!("completed");
        };
        assert_eq!(item.status, ItemStatus::Completed);
        let call: ToolCallPayload = serde_json::from_value(item.payload.clone()).unwrap();
        let ToolPayload::Command {
            exit_code,
            output,
            process_id,
            ..
        } = &call.tool
        else {
            panic!("a command");
        };
        assert_eq!(
            (*exit_code, output.inline.as_str(), process_id.as_deref()),
            (Some(0), "ok\nall green\n", Some("4242"))
        );
        assert_eq!(
            call.duration_ms,
            Some(1500),
            "from the moments the harness gives"
        );
        assert_eq!(call.raw.output.as_ref().unwrap().inline, "ok\nall green\n");
        assert!(!call.raw.is_error && item.output_blob.is_none());
    }

    #[test]
    fn a_command_that_fails_or_is_refused_says_so_in_both_forms() {
        let mut rig = Rig::new();
        rig.begin();
        rig.item_started(command(
            "c1",
            "inProgress",
            json!("1"),
            Json::Null,
            Json::Null,
        ));
        let failed = rig.item_completed(command(
            "c1",
            "failed",
            json!("1"),
            json!("boom\n"),
            json!(101),
        ));
        let SessionEvent::ItemCompleted(item) = &failed[0] else {
            panic!("completed");
        };
        assert_eq!(item.status, ItemStatus::Failed);
        let call: ToolCallPayload = serde_json::from_value(item.payload.clone()).unwrap();
        assert!(
            call.raw.is_error
                && matches!(
                    call.tool,
                    ToolPayload::Command {
                        exit_code: Some(101),
                        ..
                    }
                )
        );
        rig.item_started(command(
            "c2",
            "inProgress",
            Json::Null,
            Json::Null,
            Json::Null,
        ));
        let refused = rig.item_completed(command(
            "c2",
            "declined",
            Json::Null,
            Json::Null,
            Json::Null,
        ));
        let SessionEvent::ItemCompleted(item) = &refused[0] else {
            panic!("completed");
        };
        assert_eq!(item.status, ItemStatus::Cancelled);
        let call: ToolCallPayload = serde_json::from_value(item.payload.clone()).unwrap();
        assert!(
            call.raw.output.is_none() && call.raw.is_error,
            "a call that was refused has no output"
        );
    }

    #[test]
    fn a_long_output_keeps_its_end_inline_and_the_whole_in_a_file_named_by_its_hash() {
        let mut rig = Rig::new();
        rig.begin();
        rig.item_started(command(
            "c1",
            "inProgress",
            json!("1"),
            Json::Null,
            Json::Null,
        ));
        let whole = format!("{}END", "line of output\n".repeat(5_000));
        let done = rig.item_completed(command(
            "c1",
            "completed",
            json!("1"),
            json!(whole.clone()),
            json!(0),
        ));
        let SessionEvent::ItemCompleted(item) = &done[0] else {
            panic!("completed");
        };
        let call: ToolCallPayload = serde_json::from_value(item.payload.clone()).unwrap();
        let ToolPayload::Command { output, .. } = &call.tool else {
            panic!("a command");
        };
        assert!(output.truncated && output.inline.ends_with("END"));
        assert!(i64::try_from(output.inline.len()).unwrap() <= OUTPUT_INLINE_MAX_BYTES);
        assert_eq!(output.total_len, i64::try_from(whole.len()).unwrap());
        let blob = item.output_blob.clone().expect("the whole is kept");
        assert_eq!(output.blob, Some(blob.hash));
        assert_eq!(blob.size, i64::try_from(whole.len()).unwrap());
        let kept =
            std::fs::read_to_string(rig.outputs.directory().join(blob.hash.to_string())).unwrap();
        assert_eq!(kept, whole);
        assert_eq!(Hash::of(kept.as_bytes()), blob.hash);
    }

    #[test]
    fn the_shell_around_a_command_is_taken_off_only_where_it_is_one() {
        assert_eq!(
            display_command("/bin/bash -lc 'echo hi && pwd'"),
            "echo hi && pwd"
        );
        assert_eq!(display_command("bash -c \"ls -la\""), "ls -la");
        assert_eq!(
            display_command("sh -c 'echo \"quoted\"'"),
            "echo \"quoted\""
        );
        assert_eq!(display_command("/usr/bin/zsh -l -c 'make'"), "make");
        assert_eq!(display_command("git status"), "git status");
        assert_eq!(
            display_command("python3 -c 'print(1)'"),
            "python3 -c 'print(1)'"
        );
        assert_eq!(display_command("bash script.sh arg"), "bash script.sh arg");
        assert_eq!(display_command("bash -lc 'unclosed"), "bash -lc 'unclosed");
        assert_eq!(
            shell_words(r#"a 'b c' "d\"e" f\ g"#),
            Some(vec!["a".into(), "b c".into(), "d\"e".into(), "f g".into()])
        );
        assert_eq!(first_line("\n  first line  \nsecond"), "first line");
        assert_eq!(
            first_line(&"x".repeat(500)).chars().count(),
            TITLE_MAX_CHARS
        );
    }

    #[test]
    fn a_change_of_files_is_a_difference_for_each_file_in_the_form_the_neutral_model_has() {
        let directory = "/workspace/web";
        let added = file_change(
            &json!({ "path": "/workspace/web/new.txt", "kind": { "type": "add" }, "diff": "hello\nworld\n" }),
            directory,
        );
        assert_eq!(
            (
                added.path.as_str(),
                added.change,
                added.added,
                added.removed
            ),
            ("new.txt", FileChangeKind::Add, 2, 0)
        );
        assert_eq!(
            added.diff.as_deref(),
            Some("@@ -0,0 +1,2 @@\n+hello\n+world\n")
        );
        let deleted = file_change(
            &json!({ "path": "/workspace/web/old.txt", "kind": { "type": "delete" }, "diff": "gone\n" }),
            directory,
        );
        assert_eq!(
            (deleted.change, deleted.added, deleted.removed),
            (FileChangeKind::Delete, 0, 1)
        );
        assert_eq!(deleted.diff.as_deref(), Some("@@ -1 +0,0 @@\n-gone\n"));
        let changed = file_change(
            &json!({ "path": "/workspace/web/src/a.rs", "kind": { "type": "update", "move_path": null }, "diff": "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n" }),
            directory,
        );
        assert_eq!(
            (
                changed.path.as_str(),
                changed.change,
                changed.added,
                changed.removed
            ),
            ("src/a.rs", FileChangeKind::Modify, 1, 1)
        );
        let moved = file_change(
            &json!({ "path": "/workspace/web/c.txt", "kind": { "type": "update", "move_path": "/workspace/web/d.txt" }, "diff": "@@ -1 +1 @@\n-a\n+b\n" }),
            directory,
        );
        assert_eq!(
            (moved.change, moved.path.as_str(), moved.old_path.as_deref()),
            (FileChangeKind::Rename, "d.txt", Some("c.txt"))
        );
        let outside = file_change(
            &json!({ "path": "/etc/hosts", "kind": { "type": "update", "move_path": null }, "diff": "@@ -1 +1 @@\n-a\n+b\n" }),
            directory,
        );
        assert_eq!(
            outside.path, "/etc/hosts",
            "a path outside the thread's directory stays absolute"
        );
        let binary = file_change(
            &json!({ "path": "/workspace/web/p.png", "kind": { "type": "add" }, "diff": "\u{0}PNG" }),
            directory,
        );
        assert!(binary.binary && binary.diff.is_none());
        let huge = file_change(
            &json!({ "path": "/workspace/web/big.txt", "kind": { "type": "add" }, "diff": "a line\n".repeat(60_000) }),
            directory,
        );
        assert!(
            huge.truncated
                && i64::try_from(huge.diff.as_ref().unwrap().len()).unwrap()
                    <= FILE_CHANGE_DIFF_MAX_BYTES
        );
        assert_eq!(huge.added, 60_000, "counted before the difference was cut");
    }

    #[test]
    fn a_change_of_files_grows_while_it_is_written() {
        let mut rig = Rig::new();
        rig.begin();
        let item =
            json!({ "type": "fileChange", "id": "f1", "status": "inProgress", "changes": [] });
        let started = rig.item_started(item.clone());
        assert!(
            matches!(&started[0], SessionEvent::ItemStarted(i) if matches!(&i.content,
            ItemContent::ToolCall(call) if matches!(&call.tool, ToolPayload::FileChange { changes } if changes.is_empty())))
        );
        let updated = rig.feed(
            "item/fileChange/patchUpdated",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "f1",
                    "changes": [ { "path": "/workspace/web/a.txt", "kind": { "type": "add" }, "diff": "x\n" } ] }),
        );
        let SessionEvent::ItemUpdated(update) = &updated[0] else {
            panic!("an update");
        };
        assert_eq!(update.payload["tool"]["kind"], "file_change");
        assert_eq!(update.payload["tool"]["changes"][0]["path"], "a.txt");
        let mut finished = item;
        finished["status"] = json!("completed");
        finished["changes"] =
            json!([ { "path": "/workspace/web/a.txt", "kind": { "type": "add" }, "diff": "x\n" } ]);
        let done = rig.item_completed(finished);
        let SessionEvent::ItemCompleted(item) = &done[0] else {
            panic!("completed");
        };
        assert_eq!(item.payload["title"], "Edit a.txt");
        assert_eq!(item.payload["raw"]["name"], "fileChange");
    }

    #[test]
    fn reasoning_starts_with_its_first_readable_piece_and_keeps_its_summary() {
        let mut rig = Rig::new();
        rig.begin();
        assert!(
            rig.item_started(
                json!({ "type": "reasoning", "id": "r1", "summary": [], "content": [] })
            )
            .is_empty()
        );
        let delta = |rig: &mut Rig, part: i64, text: &str| {
            rig.feed("item/reasoning/summaryTextDelta", json!({ "threadId": THREAD, "turnId": TURN, "itemId": "r1", "delta": text, "summaryIndex": part }))
        };
        rig.feed(
            "item/reasoning/summaryPartAdded",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "r1", "summaryIndex": 0 }),
        );
        let a = delta(&mut rig, 0, "First thought.");
        let boundary = rig.feed(
            "item/reasoning/summaryPartAdded",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "r1", "summaryIndex": 1 }),
        );
        let b = delta(&mut rig, 1, "Second.");
        let (SessionEvent::ItemDelta(a), SessionEvent::ItemDelta(gap), SessionEvent::ItemDelta(b)) =
            (&a[1], &boundary[0], &b[0])
        else {
            panic!("deltas");
        };
        assert_eq!((a.field, a.offset), (DeltaField::Summary, 0));
        assert_eq!(
            (gap.data.as_str(), gap.offset),
            ("\n\n", 14),
            "a new part is set apart by a blank line"
        );
        assert_eq!(b.offset, 16);
        let done = rig.item_completed(json!({ "type": "reasoning", "id": "r1", "summary": ["First thought.", "Second."], "content": [] }));
        let SessionEvent::ItemCompleted(item) = &done[0] else {
            panic!("completed");
        };
        assert_eq!(
            item.payload,
            json!({ "summary": "First thought.\n\nSecond.", "text_len": 23, "hidden": false })
        );
    }

    #[test]
    fn thinking_that_ends_empty_never_becomes_a_line_even_when_the_turn_is_interrupted() {
        let mut rig = Rig::new();
        rig.begin();
        let empty = json!({ "type": "reasoning", "id": "r2", "summary": [], "content": [] });
        assert!(rig.item_started(empty.clone()).is_empty());
        assert!(rig.item_completed(empty.clone()).is_empty());
        assert!(rig.item_completed(empty.clone()).is_empty());
        assert!(rig.item_started(empty).is_empty());
        let interrupted = rig.end("interrupted", Json::Null);
        assert_eq!(kinds(&interrupted), ["turn.completed"]);
    }

    #[test]
    fn the_plan_of_a_turn_is_one_item_that_is_updated_in_place_and_ends_with_the_turn() {
        let mut rig = Rig::new();
        rig.begin();
        let update = |rig: &mut Rig, steps: Json| {
            rig.feed(
                "turn/plan/updated",
                json!({ "threadId": THREAD, "turnId": TURN, "explanation": "why", "plan": steps }),
            )
        };
        let first = update(
            &mut rig,
            json!([ { "step": "read", "status": "inProgress" }, { "step": "fix", "status": "pending" } ]),
        );
        let SessionEvent::ItemStarted(item) = &first[0] else {
            panic!("the plan begins");
        };
        assert_eq!(item.native_ref, "plan-turn_1");
        let ItemContent::Plan(plan) = &item.content else {
            panic!("a plan");
        };
        assert_eq!(
            (
                plan.explanation.as_deref(),
                plan.steps.len(),
                plan.steps[0].status
            ),
            (Some("why"), 2, PlanStepStatus::InProgress)
        );
        let second = update(
            &mut rig,
            json!([ { "step": "read", "status": "completed" }, { "step": "fix", "status": "inProgress" } ]),
        );
        let SessionEvent::ItemUpdated(changed) = &second[0] else {
            panic!("the plan is updated");
        };
        assert_eq!(changed.payload["steps"][0]["status"], "completed");
        let end = rig.end("completed", Json::Null);
        assert_eq!(kinds(&end), ["item.completed", "turn.completed"]);
    }

    #[test]
    fn a_turn_the_harness_moved_to_another_model_says_so_and_reports_its_use_under_that_model() {
        let mut rig = Rig::new();
        rig.begin();
        let moved = rig.feed(
            "model/rerouted",
            json!({ "threadId": THREAD, "turnId": TURN, "fromModel": "gpt-6-sol",
                    "toModel": "gpt-6-sol-safe", "reason": "highRiskCyberActivity" }),
        );
        assert_eq!(kinds(&moved), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(notice) = &moved[0] else {
            panic!("the notice");
        };
        assert!(
            matches!(&notice.content, ItemContent::Error(error)
                if error.code == turn_error::MODEL_REROUTED && !error.retryable
                    && error.message.contains("gpt-6-sol-safe instead of gpt-6-sol")),
            "{notice:?}"
        );
        assert!(
            matches!(&moved[1], SessionEvent::ItemCompleted(done) if done.status == ItemStatus::Completed),
            "a notice, not a failure"
        );
        // Another thread's or another turn's word moves nothing here.
        assert!(
            rig.feed(
                "model/rerouted",
                json!({ "threadId": "elsewhere", "turnId": TURN, "fromModel": "a", "toModel": "b", "reason": "highRiskCyberActivity" }),
            )
            .is_empty()
        );
        let used = rig.feed(
            "thread/tokenUsage/updated",
            json!({ "threadId": THREAD, "turnId": TURN, "tokenUsage": {
                "total": { "totalTokens": 120, "inputTokens": 100, "cachedInputTokens": 0, "cacheWriteInputTokens": 0, "outputTokens": 20, "reasoningOutputTokens": 0 },
                "last": { "totalTokens": 120, "inputTokens": 100, "cachedInputTokens": 0, "cacheWriteInputTokens": 0, "outputTokens": 20, "reasoningOutputTokens": 0 },
                "modelContextWindow": 258_400 } }),
        );
        let SessionEvent::UsageUpdated(update) = &used[0] else {
            panic!("use");
        };
        let report = update.report.as_ref().expect("a report");
        assert_eq!(report.models.len(), 1);
        assert_eq!(report.models[0].model, "gpt-6-sol-safe");
        assert_eq!(report.models[0].tokens.total_tokens, 120);
    }

    #[test]
    fn use_is_summed_over_the_requests_of_a_turn_and_a_repeated_one_is_no_use_of_it() {
        let mut rig = Rig::new();
        let usage = |turn: &str, total: i64, last: i64| {
            json!({ "threadId": THREAD, "turnId": turn, "tokenUsage": {
                "total": { "totalTokens": total, "inputTokens": total - 2, "cachedInputTokens": 1, "cacheWriteInputTokens": 0, "outputTokens": 2, "reasoningOutputTokens": 1 },
                "last": { "totalTokens": last, "inputTokens": last - 1, "cachedInputTokens": 0, "cacheWriteInputTokens": 0, "outputTokens": 1, "reasoningOutputTokens": 0 },
                "modelContextWindow": 258_400 } })
        };
        // The usage of an earlier turn comes again when the thread is resumed.
        assert!(
            rig.feed("thread/tokenUsage/updated", usage("turn_0", 8_000, 8_000))
                .is_empty()
        );
        rig.begin();
        let first = rig.feed("thread/tokenUsage/updated", usage(TURN, 8_600, 600));
        let second = rig.feed("thread/tokenUsage/updated", usage(TURN, 9_500, 900));
        let (SessionEvent::UsageUpdated(one), SessionEvent::UsageUpdated(two)) =
            (&first[0], &second[0])
        else {
            panic!("use");
        };
        assert_eq!((one.turn.total_tokens, two.turn.total_tokens), (600, 1_500));
        assert_eq!(
            two.thread.total_tokens, 9_500,
            "the thread is what the harness says it has used"
        );
        // The context holds the prompt of the last request, its input; the output is in the next prompt.
        assert_eq!(
            (two.context_window, two.context_used),
            (Some(258_400), Some(899))
        );
        assert!(two.turn.is_consistent());
        assert_eq!(
            (two.turn.cached_input_tokens, two.turn.reasoning_tokens),
            (0, 0)
        );
        let report = two.report.as_ref().expect("a report beside the totals");
        assert_eq!(
            report.compact_at,
            Some(244_800),
            "nine tenths of the window of which the harness reports 95 %"
        );
        assert_eq!(report.subagents, Some(TokenUsage::default()));
    }

    #[test]
    fn cache_writes_sub_agents_and_a_shortening_are_carried_whole() {
        let mut rig = Rig::new();
        rig.begin();
        let request = |thread: &str, turn: &str, input: i64, cached: i64, written: i64| {
            json!({ "threadId": thread, "turnId": turn, "tokenUsage": {
                "total": { "totalTokens": input + 10, "inputTokens": input, "cachedInputTokens": cached,
                           "cacheWriteInputTokens": written, "outputTokens": 10, "reasoningOutputTokens": 4 },
                "last": { "totalTokens": input + 10, "inputTokens": input, "cachedInputTokens": cached,
                          "cacheWriteInputTokens": written, "outputTokens": 10, "reasoningOutputTokens": 4 },
                "modelContextWindow": 258_400 } })
        };
        let first = rig.feed(
            "thread/tokenUsage/updated",
            request(THREAD, TURN, 1_000, 600, 300),
        );
        let SessionEvent::UsageUpdated(first) = &first[0] else {
            panic!("use");
        };
        assert_eq!(
            (
                first.turn.cached_input_tokens,
                first.turn.cache_write_tokens
            ),
            (600, 300),
            "the input written to a cache is counted apart, inside the input"
        );
        assert!(first.turn.is_consistent());
        assert_eq!(
            first.context_used,
            Some(1_000),
            "the cached parts are not added to the input again"
        );
        // A compaction, then the next prompt says what is left.
        rig.item_started(json!({ "type": "contextCompaction", "id": "k9" }));
        rig.item_completed(json!({ "type": "contextCompaction", "id": "k9" }));
        let after = rig.feed(
            "thread/tokenUsage/updated",
            request(THREAD, TURN, 400, 0, 0),
        );
        let SessionEvent::UsageUpdated(after) = &after[0] else {
            panic!("use");
        };
        let shortened = after
            .report
            .as_ref()
            .and_then(|report| report.shortened.clone())
            .expect("the shortening, with its sizes");
        assert_eq!(
            (shortened.before, shortened.after),
            (Some(1_000), Some(400))
        );
        // A sub-agent's use belongs to the turn, and to the sub-agent's share of it.
        rig.feed(
            "thread/started",
            json!({ "thread": { "id": "thr_helper", "parentThreadId": THREAD } }),
        );
        let child = rig.feed(
            "thread/tokenUsage/updated",
            request("thr_helper", "turn_h", 200, 0, 0),
        );
        let SessionEvent::UsageUpdated(child) = &child[0] else {
            panic!("use");
        };
        let report = child.report.as_ref().expect("a report");
        assert_eq!(
            report.subagents.map(|tokens| tokens.total_tokens),
            Some(210)
        );
        assert_eq!(report.by_subagent.len(), 1);
        assert_eq!(report.by_subagent[0].stream, "sub:thr_helper");
        assert_eq!(child.turn.total_tokens, 1_010 + 410 + 210);
        assert_eq!(
            child.context_used, None,
            "a sub-agent's prompt is not this context"
        );
    }

    #[test]
    fn compaction_is_a_phase_of_the_turn_and_a_marker_when_only_its_end_is_heard() {
        let mut rig = Rig::new();
        rig.begin();
        let begun = rig.item_started(json!({ "type": "contextCompaction", "id": "k1" }));
        let SessionEvent::TurnPhase(phase) = &begun[0] else {
            panic!("a phase");
        };
        assert_eq!(
            (phase.kind, phase.turn_id),
            (TurnPhaseKind::Compacting, id(9))
        );
        assert_eq!(phase.at.to_string(), "2026-10-11T11:00:00.000Z");
        let ended = rig.item_completed(json!({ "type": "contextCompaction", "id": "k1" }));
        assert!(
            matches!(&ended[0], SessionEvent::TurnPhase(phase) if phase.kind == TurnPhaseKind::Working)
        );
        let marker = rig.feed(
            "thread/compacted",
            json!({ "threadId": THREAD, "turnId": TURN }),
        );
        assert_eq!(kinds(&marker), ["item.started", "item.completed"]);
        assert!(matches!(&marker[0], SessionEvent::ItemStarted(item)
            if matches!(item.content, ItemContent::Marker(MarkerPayload::Compacted { duration_ms: None }))));
        // A turn that ends while it compacts leaves no phase behind.
        rig.item_started(json!({ "type": "contextCompaction", "id": "k2" }));
        let end = rig.end("completed", Json::Null);
        assert_eq!(kinds(&end), ["turn.phase", "turn.completed"]);
    }

    #[test]
    fn a_turn_is_told_apart_by_how_it_ended_and_the_items_it_left_open_end_with_it() {
        let mut rig = Rig::new();
        rig.begin();
        rig.item_started(message("m1", ""));
        rig.feed(
            "item/agentMessage/delta",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "m1", "delta": "half" }),
        );
        rig.item_started(command(
            "c1",
            "inProgress",
            Json::Null,
            Json::Null,
            Json::Null,
        ));
        rig.item_started(command(
            "c2",
            "inProgress",
            json!("77"),
            Json::Null,
            Json::Null,
        ));
        rig.feed(
            "item/commandExecution/outputDelta",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "c2", "delta": "tick\n" }),
        );
        let end = rig.end("interrupted", Json::Null);
        assert_eq!(
            kinds(&end),
            [
                "item.updated",
                "item.updated",
                "item.completed",
                "turn.completed"
            ]
        );
        let SessionEvent::ItemUpdated(unknown_process) = &end[0] else {
            panic!("the command whose process was not named yet");
        };
        assert_eq!(
            (
                unknown_process.native_ref.as_str(),
                unknown_process.payload["tool"]["background"].clone()
            ),
            ("c1", json!(true)),
            "{end:?}"
        );
        let SessionEvent::ItemUpdated(background) = &end[1] else {
            panic!("the background command");
        };
        assert_eq!(
            (
                background.native_ref.as_str(),
                background.payload["tool"]["background"].clone()
            ),
            ("c2", json!(true))
        );
        let SessionEvent::ItemCompleted(said) = &end[2] else {
            panic!("the message");
        };
        assert_eq!(
            (said.native_ref.as_str(), said.status),
            ("m1", ItemStatus::Cancelled)
        );
        assert_eq!(
            said.payload,
            json!({ "text": "half", "text_len": 4, "phase": null })
        );
        let SessionEvent::TurnCompleted(ended) = &end[3] else {
            panic!("the turn");
        };
        assert_eq!(ended.status, TurnOutcome::Interrupted);
        // The command that went on is completed when its process ends, after the turn.
        let late = rig.feed(
            "item/commandExecution/outputDelta",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "c2", "delta": "tock\n" }),
        );
        assert!(matches!(&late[0], SessionEvent::ItemDelta(delta) if delta.offset == 5));
        let finished = rig.item_completed(command(
            "c2",
            "completed",
            json!("77"),
            json!("tick\ntock\n"),
            json!(0),
        ));
        assert!(
            matches!(&finished[0], SessionEvent::ItemCompleted(item) if item.status == ItemStatus::Completed
            && item.payload["tool"]["background"] == json!(true))
        );
    }

    #[test]
    fn a_request_that_is_made_again_leaves_what_the_failed_attempt_streamed_cancelled() {
        let mut rig = Rig::new();
        rig.begin();
        rig.item_started(message("m1", ""));
        rig.feed(
            "item/agentMessage/delta",
            json!({ "threadId": THREAD, "turnId": TURN, "itemId": "m1", "delta": "par" }),
        );
        let retry = rig.feed(
            "error",
            json!({ "threadId": THREAD, "turnId": TURN, "willRetry": true,
                    "error": { "message": "Reconnecting... 1/5", "codexErrorInfo": { "responseStreamDisconnected": { "httpStatusCode": null } } } }),
        );
        assert_eq!(
            kinds(&retry),
            ["item.completed", "item.started", "item.completed"]
        );
        assert!(
            matches!(&retry[0], SessionEvent::ItemCompleted(item) if item.status == ItemStatus::Cancelled && item.native_ref == "m1")
        );
        rig.item_started(message("m2", ""));
        rig.item_completed(message("m2", "partial text"));
        let fail = json!({ "message": "stream disconnected before completion", "codexErrorInfo": "other", "additionalDetails": null });
        let last = rig.feed(
            "error",
            json!({ "threadId": THREAD, "turnId": TURN, "willRetry": false, "error": fail }),
        );
        assert_eq!(kinds(&last), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(item) = &last[0] else {
            panic!("the error");
        };
        assert!(
            matches!(&item.content, ItemContent::Error(error) if error.code == "provider_unreachable" && error.retryable)
        );
        let end = rig.end("failed", fail);
        let Some(SessionEvent::TurnCompleted(ended)) = end.last() else {
            panic!("the turn");
        };
        assert_eq!(ended.status, TurnOutcome::Failed);
        let error = ended.error.as_ref().unwrap();
        assert_eq!(error.code, "provider_unreachable");
        assert!(error.incomplete, "the last attempt wrote part of an answer");
        assert!(
            error
                .said
                .as_deref()
                .unwrap()
                .contains("tried again 1 times"),
            "{error:?}"
        );
    }

    #[test]
    fn a_turn_that_ends_in_the_plans_limit_carries_the_moment_the_rate_limits_gave() {
        let mut rig = Rig::new();
        rig.windows = Some(Windows::read(&json!({
            "primary": { "usedPercent": 100, "windowDurationMins": 300, "resetsAt": 1_791_810_152 },
            "secondary": { "usedPercent": 40, "windowDurationMins": 10080, "resetsAt": null }
        })));
        rig.begin();
        let fail = json!({ "message": "You\u{2019}ve hit your usage limit. Visit https://example.test or try again at 9:02 PM.",
                           "codexErrorInfo": "usageLimitExceeded", "additionalDetails": null });
        let last = rig.feed(
            "error",
            json!({ "threadId": THREAD, "turnId": TURN, "willRetry": false, "error": fail }),
        );
        assert!(matches!(&last[0], SessionEvent::ItemStarted(item)
                if matches!(&item.content, ItemContent::Error(error) if error.code == "provider_exhausted" && !error.retryable)));
        let end = rig.end("failed", fail);
        let Some(SessionEvent::TurnCompleted(ended)) = end.last() else {
            panic!("the turn");
        };
        let error = ended.error.as_ref().unwrap();
        assert_eq!(error.code, "provider_exhausted");
        assert_eq!(
            error.resets_at.map(|time| time.unix_millis()),
            Some(1_791_810_152_000)
        );
        assert!(
            error
                .said
                .as_deref()
                .unwrap()
                .contains("try again at 9:02 PM")
        );
        // A turn that carries no error of its own ends with the one the harness reported as it happened.
        let mut rig = Rig::new();
        rig.begin();
        rig.feed(
            "error",
            json!({ "threadId": THREAD, "turnId": TURN, "willRetry": false,
                    "error": { "message": "unexpected status 502 Bad Gateway", "codexErrorInfo": { "httpConnectionFailed": { "httpStatusCode": 502 } } } }),
        );
        let end = rig.end("failed", Json::Null);
        let Some(SessionEvent::TurnCompleted(ended)) = end.last() else {
            panic!("the turn");
        };
        assert_eq!(ended.error.as_ref().unwrap().code, "provider_busy");
    }

    #[test]
    fn a_turn_the_harness_started_by_itself_is_not_reported() {
        let mut rig = Rig::new();
        let by_itself = rig.feed(
            "turn/started",
            json!({ "threadId": THREAD, "turn": { "id": "turn_x" } }),
        );
        assert!(by_itself.is_empty());
        assert!(
            rig.item_started(message("m1", "")).is_empty(),
            "there is no turn of Orch to put it in"
        );
        assert!(
            rig.feed(
                "turn/completed",
                json!({ "threadId": THREAD, "turn": { "id": "turn_x", "status": "completed" } })
            )
            .is_empty()
        );
        assert!(!rig.mapper.turn_in_progress());
    }

    #[test]
    fn the_calls_of_mcp_servers_and_of_orchs_own_tools_are_told_apart() {
        let mut rig = Rig::new();
        rig.begin();
        let item = |server: &str, tool: &str| {
            json!({ "type": "mcpToolCall", "id": format!("{server}-{tool}"), "server": server, "tool": tool,
            "status": "completed", "arguments": { "title": "x" }, "result": { "content": [ { "type": "text", "text": "done" } ], "structuredContent": null, "_meta": null },
            "error": null, "durationMs": 12, "appContext": null, "mcpAppUi": null, "pluginId": null, "readOnlyHint": null })
        };
        let github = rig.item_completed(item("github", "create_issue"));
        let call: ToolCallPayload = serde_json::from_value(match &github[1] {
            SessionEvent::ItemCompleted(i) => i.payload.clone(),
            _ => panic!("completed"),
        })
        .unwrap();
        assert!(
            matches!(&call.tool, ToolPayload::Mcp { server, tool, error: None, .. } if server == "github" && tool == "create_issue")
        );
        assert_eq!(
            (
                call.title.as_str(),
                call.duration_ms,
                call.raw.name.as_str()
            ),
            ("github: create_issue", Some(12), "mcpToolCall")
        );
        let orch = rig.item_completed(item("orch", "announce"));
        let call: ToolCallPayload = serde_json::from_value(match &orch[1] {
            SessionEvent::ItemCompleted(i) => i.payload.clone(),
            _ => panic!("completed"),
        })
        .unwrap();
        assert!(matches!(
            &call.tool,
            ToolPayload::Orch {
                tool: OrchTool::Announce,
                ..
            }
        ));
        let unknown = rig.item_completed(item("orch", "not_one_of_the_registered_tools"));
        let call: ToolCallPayload = serde_json::from_value(match &unknown[1] {
            SessionEvent::ItemCompleted(i) => i.payload.clone(),
            _ => panic!("completed"),
        })
        .unwrap();
        assert!(
            matches!(&call.tool, ToolPayload::Mcp { .. }),
            "an unregistered tool of that server is an MCP call"
        );
        let broken = rig.item_completed(json!({ "type": "mcpToolCall", "id": "b1", "server": "github", "tool": "x", "status": "failed",
            "arguments": {}, "result": null, "error": { "message": "rate limited" }, "durationMs": null }));
        let SessionEvent::ItemCompleted(done) = &broken[1] else {
            panic!("completed");
        };
        assert_eq!(done.status, ItemStatus::Failed);
        assert!(done.payload["raw"]["is_error"].as_bool().unwrap());
        assert_eq!(done.payload["tool"]["error"], "rate limited");
    }

    #[test]
    fn what_the_mapper_does_not_know_is_kept_whole_as_a_tool_of_kind_other() {
        let mut rig = Rig::new();
        rig.begin();
        let started = rig
            .item_started(json!({ "type": "holographicProjection", "id": "h1", "intensity": 11 }));
        let SessionEvent::ItemStarted(item) = &started[0] else {
            panic!("an item");
        };
        let ItemContent::ToolCall(call) = &item.content else {
            panic!("a tool call");
        };
        assert!(
            matches!(&call.tool, ToolPayload::Other { name, arguments: Some(arguments), .. }
            if name == "holographicProjection" && arguments["intensity"] == 11)
        );
        assert_eq!(call.raw.name, "holographicProjection");
        let slept = rig.item_started(json!({ "type": "sleep", "id": "s1", "durationMs": 5000 }));
        assert!(
            matches!(&slept[0], SessionEvent::ItemStarted(item) if matches!(&item.content,
            ItemContent::ToolCall(call) if matches!(&call.tool, ToolPayload::Other { name, .. } if name == "sleep")))
        );
        // What is not an item at all is nothing.
        assert!(
            rig.feed("some/new/notification", json!({ "threadId": THREAD }))
                .is_empty()
        );
    }

    #[test]
    fn a_spawn_result_binds_its_child_without_a_thread_notice_even_after_the_parent_finishes() {
        let mut rig = Rig::new();
        rig.begin();
        rig.item_started(json!({"type":"collabAgentToolCall","id":"spawn","tool":"spawnAgent","status":"inProgress","receiverThreadIds":[],"agentsStates":{}}));
        assert!(!rig.mapper.follows("thr_child"));
        rig.item_completed(json!({"type":"collabAgentToolCall","id":"spawn","tool":"spawnAgent","status":"completed","receiverThreadIds":["thr_child"],"agentsStates":{"thr_child":{"status":"pendingInit"}}}));
        assert!(rig.mapper.follows("thr_child"));
        rig.feed(
            "turn/completed",
            json!({"threadId":THREAD,"turn":{"id":TURN,"status":"completed"}}),
        );
        assert!(!rig.mapper.turn_in_progress());
        let late = rig.feed("item/started", json!({"threadId":"thr_child","turnId":"child-turn","item":message("child-message", "native child marker")}));
        assert!(
            matches!(&late[..], [SessionEvent::ItemStarted(item)] if item.stream == "sub:thr_child" && item.parent_native_ref.as_deref() == Some("spawn") && item.turn_id == id(9))
        );
        assert!(rig.feed("item/started", json!({"threadId":"unrelated","turnId":"foreign","item":message("foreign-message", "unrelated")})).is_empty());
    }

    #[test]
    fn a_sub_agent_s_card_follows_the_sub_agent_after_the_call_that_started_it_has_completed() {
        let mut rig = Rig::new();
        rig.begin();
        rig.item_started(json!({"type":"collabAgentToolCall","id":"spawn","tool":"spawnAgent","status":"inProgress","receiverThreadIds":[],"agentsStates":{},"model":"gpt-6-mini"}));
        let spawned = rig.item_completed(json!({"type":"collabAgentToolCall","id":"spawn","tool":"spawnAgent","status":"completed","receiverThreadIds":["thr_child"],"agentsStates":{"thr_child":{"status":"pendingInit"}},"model":"gpt-6-mini"}));
        let card = spawned
            .iter()
            .find_map(|event| match event {
                SessionEvent::ItemCompleted(done) if done.native_ref == "spawn" => {
                    Some(done.payload.clone())
                }
                _ => None,
            })
            .expect("the spawn's card completes");
        assert_eq!(
            card["tool"]["state"], "running",
            "the call completed; the sub-agent it started runs"
        );
        assert_eq!(card["tool"]["model"], "gpt-6-mini");
        let state_of = |events: &[SessionEvent]| -> Vec<(String, Json)> {
            events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemUpdated(update) => {
                        Some((update.native_ref.clone(), update.payload.clone()))
                    }
                    _ => None,
                })
                .collect()
        };
        let ended = rig.item_completed(json!({"type":"subAgentActivity","id":"act1","kind":"completed","model":null,"reasoningEffort":null,"agentThreadId":"thr_child","agentPath":"/root/t"}));
        assert_eq!(
            state_of(&ended),
            [(
                "spawn".to_owned(),
                json!({ "tool": { "state": "completed" } })
            )]
        );
        let again = rig.item_completed(json!({"type":"subAgentActivity","id":"act2","kind":"completed","model":null,"reasoningEffort":null,"agentThreadId":"thr_child","agentPath":"/root/t"}));
        assert!(
            state_of(&again).is_empty(),
            "what the card says already is not said again"
        );
        // A second sub-agent, whose end a call waiting for it reports.
        rig.item_started(json!({"type":"collabAgentToolCall","id":"spawn2","tool":"spawnAgent","status":"inProgress","receiverThreadIds":[],"agentsStates":{}}));
        rig.item_completed(json!({"type":"collabAgentToolCall","id":"spawn2","tool":"spawnAgent","status":"completed","receiverThreadIds":["thr_other"],"agentsStates":{"thr_other":{"status":"running"}}}));
        rig.item_started(json!({"type":"collabAgentToolCall","id":"wait1","tool":"wait","status":"inProgress","receiverThreadIds":["thr_other"],"agentsStates":{}}));
        let waited = rig.item_completed(json!({"type":"collabAgentToolCall","id":"wait1","tool":"wait","status":"completed","receiverThreadIds":["thr_other"],"agentsStates":{"thr_other":{"status":"errored","message":"it crashed"}}}));
        assert_eq!(
            state_of(&waited),
            [(
                "spawn2".to_owned(),
                json!({ "tool": { "state": "failed" } })
            )]
        );
        // A third that still runs when the session goes: its card says it was stopped.
        rig.item_started(json!({"type":"collabAgentToolCall","id":"spawn3","tool":"spawnAgent","status":"inProgress","receiverThreadIds":[],"agentsStates":{}}));
        rig.item_completed(json!({"type":"collabAgentToolCall","id":"spawn3","tool":"spawnAgent","status":"completed","receiverThreadIds":["thr_third"],"agentsStates":{"thr_third":{"status":"running"}}}));
        let lost = rig.mapper.lost("the harness ended");
        assert_eq!(
            state_of(&lost),
            [(
                "spawn3".to_owned(),
                json!({ "tool": { "state": "interrupted" } })
            )]
        );
    }

    #[test]
    fn the_work_of_a_sub_agent_is_in_a_stream_of_its_own_under_the_call_that_started_it() {
        let mut rig = Rig::new();
        rig.begin();
        let spawn = |status: &str| {
            json!({ "type": "collabAgentToolCall", "id": "spawn1", "tool": "spawnAgent", "status": status,
            "senderThreadId": THREAD, "receiverThreadIds": ["thr_child"], "prompt": "look at the tests", "model": null,
            "reasoningEffort": null, "agentsStates": {} })
        };
        let started = rig.item_started(spawn("inProgress"));
        let SessionEvent::ItemStarted(item) = &started[0] else {
            panic!("an item");
        };
        assert!(matches!(&item.content, ItemContent::ToolCall(call)
            if matches!(&call.tool, ToolPayload::Subagent { child_stream, prompt, state, .. }
                if child_stream == "sub:thr_child" && prompt.as_deref() == Some("look at the tests") && *state == SubagentState::Running)));
        assert!(
            rig.feed(
                "thread/started",
                json!({ "thread": { "id": "thr_child", "parentThreadId": THREAD } })
            )
            .is_empty()
        );
        assert!(rig.mapper.follows("thr_child"));
        let child_item = rig.feed(
            "item/started",
            json!({ "threadId": "thr_child", "turnId": "turn_child", "item": message("cm1", ""), "startedAtMs": 1 }),
        );
        let SessionEvent::ItemStarted(inner) = &child_item[0] else {
            panic!("the child's item");
        };
        assert_eq!(
            (
                inner.stream.as_str(),
                inner.parent_native_ref.as_deref(),
                inner.turn_id
            ),
            ("sub:thr_child", Some("spawn1"), id(9))
        );
        let usage = rig.feed(
            "thread/tokenUsage/updated",
            json!({ "threadId": "thr_child", "turnId": "turn_child", "tokenUsage": {
                "total": { "totalTokens": 50, "inputTokens": 40, "cachedInputTokens": 0, "cacheWriteInputTokens": 0, "outputTokens": 10, "reasoningOutputTokens": 0 },
                "last": { "totalTokens": 50, "inputTokens": 40, "cachedInputTokens": 0, "cacheWriteInputTokens": 0, "outputTokens": 10, "reasoningOutputTokens": 0 },
                "modelContextWindow": 1000 } }),
        );
        assert!(
            matches!(&usage[0], SessionEvent::UsageUpdated(u) if u.turn.total_tokens == 50 && u.thread.total_tokens == 50)
        );
        let activity = rig.item_completed(
            json!({ "type": "subAgentActivity", "id": "act1", "kind": "completed", "model": null,
            "reasoningEffort": null, "agentThreadId": "thr_child", "agentPath": "/root/t" }),
        );
        let SessionEvent::ItemUpdated(update) = &activity[0] else {
            panic!("the call is updated");
        };
        assert_eq!(
            (
                update.native_ref.as_str(),
                update.payload["tool"]["state"].clone()
            ),
            ("spawn1", json!("completed"))
        );
        assert_eq!(update.payload["tool"]["kind"], "subagent");
        // The child's own turn ends its items.
        let ended = rig.feed("turn/completed", json!({ "threadId": "thr_child", "turn": { "id": "turn_child", "status": "completed" } }));
        assert_eq!(kinds(&ended), ["item.completed"]);
    }

    #[test]
    fn a_session_whose_harness_is_gone_ends_what_was_open_and_is_lost() {
        let mut rig = Rig::new();
        rig.begin();
        rig.item_started(message("m1", ""));
        rig.item_started(command(
            "c1",
            "inProgress",
            json!("5"),
            Json::Null,
            Json::Null,
        ));
        let lost = rig.mapper.lost("the harness exited with code 1");
        assert_eq!(
            kinds(&lost),
            ["item.completed", "item.completed", "session.lost"]
        );
        assert!(matches!(&lost[2], SessionEvent::SessionLost(l) if l.reason.contains("code 1")));
        assert!(!rig.mapper.turn_in_progress());
    }

    #[test]
    fn a_history_that_is_told_again_comes_to_the_same_events() {
        let turns = vec![json!({
            "id": "t1", "status": "completed", "error": null, "items": [
                { "type": "userMessage", "id": "u1", "clientId": null, "content": [] },
                message("m1", "done"),
                command("c1", "completed", json!("9"), json!("ok\n"), json!(0)),
            ] })];
        let config = MapperConfig {
            thread_id: id(1),
            segment_id: id(2),
            native_thread: THREAD.into(),
            directory: "/w".into(),
        };
        let rig = Rig::new();
        let ctx = Context {
            clock: &rig.clock,
            outputs: &rig.outputs,
            windows: None,
        };
        let turn_id = |native: &str| if native == "t1" { id(7) } else { id(8) };
        let once = replay(&turns, config.clone(), &ctx, turn_id);
        rig.clock.advance(chrono::Duration::days(1));
        let twice = replay(&turns, config, &ctx, turn_id);
        assert_eq!(once, twice);
        assert_eq!(
            kinds(&once),
            [
                "item.started",
                "item.completed",
                "item.started",
                "item.completed",
                "turn.completed"
            ]
        );
        assert!(
            matches!(once.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.turn_id == id(7) && ended.status == TurnOutcome::Completed)
        );
        let running = vec![json!({ "id": "t2", "status": "inProgress", "items": [] })];
        assert!(
            replay(
                &running,
                MapperConfig {
                    thread_id: id(1),
                    segment_id: id(2),
                    native_thread: THREAD.into(),
                    directory: "/w".into()
                },
                &ctx,
                turn_id
            )
            .is_empty()
        );
    }

    #[test]
    fn a_history_told_a_page_at_a_time_comes_to_the_events_of_the_whole() {
        let turns: Vec<Json> = (0..7)
            .map(|index| {
                json!({
                    "id": format!("t{index}"), "status": "completed", "error": null, "items": [
                        { "type": "userMessage", "id": format!("u{index}"), "clientId": null, "content": [] },
                        message(&format!("m{index}"), "done"),
                        command(&format!("c{index}"), "completed", json!("9"), json!("ok\n"), json!(0)),
                    ] })
            })
            .collect();
        let config = MapperConfig {
            thread_id: id(1),
            segment_id: id(2),
            native_thread: THREAD.into(),
            directory: "/w".into(),
        };
        let rig = Rig::new();
        let ctx = Context {
            clock: &rig.clock,
            outputs: &rig.outputs,
            windows: None,
        };
        let turn_id = |native: &str| id(native.trim_start_matches('t').parse::<u8>().unwrap() + 10);
        let whole = replay(&turns, config.clone(), &ctx, turn_id);
        assert!(!whole.is_empty());
        for size in [1, 2, 3, 7] {
            let mut paged = Replay::new(config.clone());
            let told: Vec<SessionEvent> = turns
                .chunks(size)
                .flat_map(|page| paged.page(page, &ctx, turn_id))
                .collect();
            assert_eq!(told, whole, "pages of {size} turns");
        }
    }

    #[test]
    fn a_full_record_and_completed_turn_cut_keep_all_fields_across_read_clocks() {
        let mut tool = command(
            "c1",
            "completed",
            json!("9"),
            json!("kept tool result\n"),
            json!(0),
        );
        tool["durationMs"] = json!(37);
        let turns = vec![
            json!({"id":"t1","status":"completed","error":null,"startedAt":100,"completedAt":102,"items":[
                {"type":"userMessage","id":"u1","content":[{"type":"text","text":"first"}]},
                message("before-tool", "before"), tool, message("m1", "after")
            ]}),
            json!({"id":"t2","status":"completed","error":null,"startedAt":110,"completedAt":112,"items":[
                {"type":"userMessage","id":"u2","content":[{"type":"text","text":"second"}]}, message("m2", "second answer")
            ]}),
            json!({"id":"t3","status":"completed","error":null,"startedAt":120,"completedAt":122,"items":[
                {"type":"userMessage","id":"u3","content":[{"type":"text","text":"third"}]}, message("m3", "third answer")
            ]}),
        ];
        let source_before = serde_json::to_vec(&turns).unwrap();
        let config = MapperConfig {
            thread_id: id(1),
            segment_id: id(2),
            native_thread: THREAD.into(),
            directory: "/w".into(),
        };
        let rig = Rig::new();
        let ctx = Context {
            clock: &rig.clock,
            outputs: &rig.outputs,
            windows: None,
        };
        let turn_id = |native: &str| match native {
            "t1" => id(7),
            "t2" => id(8),
            _ => id(9),
        };
        let read_at = rig.clock.time();
        let parent = record(&turns, config.clone(), &ctx, turn_id);
        assert_eq!(
            rig.clock.time(),
            read_at,
            "historical mapping cannot rewind the live clock it was given"
        );
        assert_eq!(parent.iter().map(Vec::len).collect::<Vec<_>>(), [4, 2, 2]);
        let identities = parent
            .iter()
            .flatten()
            .map(|entry| match entry {
                RecordedEntry::Said(words) => words.native_ref.as_str(),
                RecordedEntry::Item(item) => item.native_ref.as_str(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            [
                "said:u1",
                "before-tool",
                "c1",
                "m1",
                "said:u2",
                "m2",
                "said:u3",
                "m3"
            ]
        );
        let RecordedEntry::Item(item) = &parent[0][2] else {
            panic!("the actual command must retain its original position")
        };
        let ItemContent::ToolCall(call) = &item.content else {
            panic!("the command must remain a structured tool")
        };
        assert_eq!(call.started_at.unix_millis(), 100_000);
        assert_eq!(call.duration_ms, Some(37));
        assert_eq!(item.at.map(Time::unix_millis), Some(102_000));
        assert_eq!(call.raw.name, "commandExecution");
        assert_eq!(
            call.raw.output.as_ref().unwrap().inline,
            "kept tool result\n"
        );
        let raw: Json = serde_json::from_str(&call.raw.input.inline).unwrap();
        assert_eq!(
            raw,
            json!({"command":"/bin/bash -lc 'cargo test && echo done'","cwd":"/workspace/web/api","processId":"9","source":"agent","commandActions":[{"type":"unknown","command":"cargo test && echo done"}]})
        );
        let (serialized, kept, count, omitted) = orch_agent_contract::discovery::record_lines(
            &parent,
            orch_agent_contract::discovery::RECORD_MAX_BYTES,
        )
        .unwrap();
        assert_eq!((kept, count, omitted), (3, 8, 0));
        rig.clock.advance(chrono::Duration::milliseconds(87));
        let child = record(&turns[..2], config.clone(), &ctx, turn_id);
        assert_eq!(child.iter().map(Vec::len).collect::<Vec<_>>(), [4, 2]);
        assert_eq!(
            child.as_slice(),
            &parent[..2],
            "the complete six-entry cut, including timing/raw/tool fields, must equal the original prefix"
        );
        rig.clock.advance(chrono::Duration::days(2));
        let reread_at = rig.clock.time();
        let reread = record(&turns, config, &ctx, turn_id);
        assert_eq!(rig.clock.time(), reread_at);
        assert_eq!(
            reread, parent,
            "every retained field must be independent of read time"
        );
        assert_eq!(
            orch_agent_contract::discovery::record_lines(
                &reread,
                orch_agent_contract::discovery::RECORD_MAX_BYTES
            )
            .unwrap()
            .0,
            serialized
        );
        assert_eq!(serde_json::to_vec(&turns).unwrap(), source_before);
    }

    #[test]
    fn recorded_timing_uses_only_valid_native_dates_and_keeps_absent_dates_unknown() {
        let rig = Rig::new();
        let ctx = Context {
            clock: &rig.clock,
            outputs: &rig.outputs,
            windows: None,
        };
        let config = MapperConfig {
            thread_id: id(1),
            segment_id: id(2),
            native_thread: THREAD.into(),
            directory: "/w".into(),
        };
        for (start, end, anchor, said_at, item_at) in [
            (Json::Null, Json::Null, 0, None, None),
            (json!(10), Json::Null, 10_000, Some(10_000), Some(10_000)),
            (Json::Null, json!(12), 12_000, None, Some(12_000)),
            (json!("invalid"), json!(12), 12_000, None, Some(12_000)),
            (json!(i64::MAX), Json::Null, 0, None, None),
            (json!(0), Json::Null, 0, Some(0), Some(0)),
        ] {
            let opaque = json!({"type":"futureTool","id":"opaque","arguments":{"kept":[1,"opaque value"]},"status":"completed"});
            let turns = vec![
                json!({"id":"t1","status":"completed","error":null,"startedAt":start,"completedAt":end,"items":[
                    {"type":"userMessage","id":"u1","content":[{"type":"text","text":"legacy words"}]}, opaque
                ]}),
            ];
            let first = record(&turns, config.clone(), &ctx, |_| id(7));
            let events = replay(&turns, config.clone(), &ctx, |_| id(7));
            assert_eq!(
                kinds(&events),
                ["item.started", "item.completed", "turn.completed"]
            );
            let SessionEvent::ItemStarted(started) = &events[0] else {
                panic!("replay must retain the same native tool")
            };
            let ItemContent::ToolCall(replayed) = &started.content else {
                panic!("replay must retain structured timing")
            };
            assert_eq!(replayed.started_at.unix_millis(), anchor);
            rig.clock.advance(chrono::Duration::hours(3));
            assert_eq!(record(&turns, config.clone(), &ctx, |_| id(7)), first);
            assert_eq!(replay(&turns, config.clone(), &ctx, |_| id(7)), events);
            assert_eq!(first[0].len(), 2);
            let RecordedEntry::Said(words) = &first[0][0] else {
                panic!("the authored words must remain first")
            };
            assert_eq!(words.at.map(Time::unix_millis), said_at);
            let RecordedEntry::Item(item) = &first[0][1] else {
                panic!("the unknown native item must remain whole")
            };
            assert_eq!(item.at.map(Time::unix_millis), item_at);
            let ItemContent::ToolCall(call) = &item.content else {
                panic!("the unknown tool must remain a tool")
            };
            assert_eq!(call.started_at.unix_millis(), anchor);
            assert_eq!(
                call.duration_ms, None,
                "a coarse turn anchor must not invent a tool duration"
            );
            assert_eq!(call.raw.name, "futureTool");
            assert_eq!(
                serde_json::from_str::<Json>(&call.raw.input.inline).unwrap(),
                json!({"arguments":{"kept":[1,"opaque value"]},"status":"completed"})
            );
        }
    }

    #[test]
    fn a_live_completion_seen_without_its_start_keeps_the_native_millisecond_timing() {
        let mut rig = Rig::new();
        rig.begin();
        let events = rig.feed(
            "item/completed",
            json!({"threadId":THREAD,"turnId":TURN,"startedAtMs":123_456,"completedAtMs":123_581,
            "item":command("late", "completed", Json::Null, json!("done"), json!(0))}),
        );
        assert_eq!(kinds(&events), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(started) = &events[0] else {
            panic!("the late start must be explicit")
        };
        let ItemContent::ToolCall(call) = &started.content else {
            panic!("the late command must remain structured")
        };
        assert_eq!(call.started_at.unix_millis(), 123_456);
        let SessionEvent::ItemCompleted(completed) = &events[1] else {
            panic!("the original command must complete")
        };
        assert_eq!(started.native_ref, "late");
        assert_eq!(completed.native_ref, "late");
        let call: ToolCallPayload = serde_json::from_value(completed.payload.clone()).unwrap();
        assert_eq!(call.started_at.unix_millis(), 123_456);
        assert_eq!(call.duration_ms, Some(125));
        assert_eq!(call.raw.output.unwrap().inline, "done");
    }

    #[test]
    fn ordinary_live_start_and_missing_timestamp_fallback_keep_their_existing_clocks() {
        let mut rig = Rig::new();
        rig.begin();
        let at = rig.clock.time();
        let begun = rig.item_started(command(
            "known",
            "inProgress",
            Json::Null,
            Json::Null,
            Json::Null,
        ));
        let SessionEvent::ItemStarted(started) = &begun[0] else {
            panic!("the normal native start must be reported")
        };
        let ItemContent::ToolCall(first) = &started.content else {
            panic!("the normal command must remain structured")
        };
        assert_eq!(first.started_at.unix_millis(), 1_791_716_400_000);
        rig.clock.advance(chrono::Duration::seconds(1));
        let ended = rig.feed("item/completed", json!({"threadId":THREAD,"turnId":TURN,"startedAtMs":0,"completedAtMs":1_791_716_400_080_i64,
            "item":command("known", "completed", Json::Null, json!("known done"), json!(0))}));
        assert_eq!(kinds(&ended), ["item.completed"]);
        let SessionEvent::ItemCompleted(completed) = &ended[0] else {
            panic!("the known command must complete once")
        };
        let call: ToolCallPayload = serde_json::from_value(completed.payload.clone()).unwrap();
        assert_eq!(
            call.started_at, first.started_at,
            "a completion cannot retime an already observed native start"
        );
        assert_eq!(call.duration_ms, Some(80));
        let arrived = rig.clock.time();
        assert!(arrived > at);
        let late = rig.feed(
            "item/completed",
            json!({"threadId":THREAD,"turnId":TURN,"completedAtMs":arrived.unix_millis()+50,
            "item":command("missing", "completed", Json::Null, json!("missing done"), json!(0))}),
        );
        assert_eq!(kinds(&late), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(started) = &late[0] else {
            panic!("the missing live start must be reported")
        };
        let ItemContent::ToolCall(call) = &started.content else {
            panic!("the live command must remain structured")
        };
        assert_eq!(
            call.started_at, arrived,
            "only a genuinely live frame without native timing uses its arrival clock"
        );
        let SessionEvent::ItemCompleted(completed) = &late[1] else {
            panic!("the live fallback must still complete")
        };
        let call: ToolCallPayload = serde_json::from_value(completed.payload.clone()).unwrap();
        assert_eq!(call.started_at, arrived);
        assert_eq!(call.duration_ms, Some(50));
    }

    #[test]
    fn the_title_the_harness_gives_a_thread_is_the_sessions_title() {
        let mut rig = Rig::new();
        let named = rig.feed(
            "thread/name/updated",
            json!({ "threadId": THREAD, "threadName": "Fix the login redirect" }),
        );
        assert!(
            matches!(&named[0], SessionEvent::SessionTitle(title) if title.title == "Fix the login redirect")
        );
        assert!(
            rig.feed(
                "thread/name/updated",
                json!({ "threadId": "thr_other", "threadName": "x" })
            )
            .is_empty()
        );
        let _ = HarnessRequestKind::Command;
    }
}
