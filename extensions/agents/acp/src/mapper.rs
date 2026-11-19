//! The updates of an ACP session turned into the neutral events of a turn: items that grow, tool calls that end, plans, context, phases.
//!
//! One [`Mapper`] belongs to one session. It is pure: it is given the updates in the order the agent sent them
//! and returns the events they come to, so that every rule of the mapping is a test over frames. What it holds is
//! only what the neutral events need to be consistent: the text item that is growing (a message or reasoning, as
//! the agent streams it), the tool calls that have begun and not ended, the plan item of the turn, and the turn
//! itself.
//!
//! **Text.** An agent streams a message as chunks that share a `messageId`, or, where it gives none, as a run of
//! chunks of one kind. A change of kind or of `messageId`, a tool call, a plan or the end of the turn ends the
//! item that grows. A message that ends because the agent went on to do something is `commentary`; the one that
//! ends the turn is `final`.
//!
//! **Streaming.** A program that gives its messages whole may run a companion plugin that streams each model
//! call as it is written ([`crate::extensions::StreamNotice`]). The frames of a call grow one item of text and
//! one of reasoning, named by the call's `attempt`; the program's own committed message follows with the same
//! attempt in its `_meta.orch`, and it finishes the item that streamed (a text that continues what was shown adds
//! its rest, one that does not replaces the item's text when it ends) instead of starting a second.
//!
//! **Nothing invented.** A count the agent did not give is not made up: the turn's token totals stay zero until a
//! companion plugin reports them, and a model request carries no counts of its own.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use orch_agent_contract::events::{
    AgentMessagePayload, AgentMessagePhase, BlobRef, ItemCompleted, ItemContent, ItemDelta,
    ItemStarted, ItemStatus, ItemUpdated, MAIN_STREAM, PlanPayload, PlanStep, PlanStepStatus,
    ReasoningPayload, SessionEvent, SessionTitle, ToolCallPayload, ToolPayload, TurnCompleted,
    TurnError, TurnOutcome, TurnPhase, TurnPhaseKind, TurnStarted, UsageUpdated,
};
use orch_agent_contract::events::{DeltaField, TokenUsage};
use orch_agent_contract::requests::UsageRequest;
use orch_agent_contract::session::{ThreadId, TurnId};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde_json::json;

use crate::extensions::{Frame, StreamNotice, committed_attempt};
use crate::program::AcpProgram;
use crate::tools::{
    Place, Status, ToolView, call_title, output_of, raw_call, tool_payload, understand,
};
use crate::wire::content_text;

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

/// What the mapper is told once, when the session opens.
#[derive(Debug, Clone)]
pub struct MapperConfig {
    /// The thread.
    pub thread_id: ThreadId,
    /// The segment of the thread.
    pub segment_id: Id,
    /// The manifest.
    pub program: Arc<AcpProgram>,
    /// The thread's directory.
    pub directory: String,
    /// Where the agent starts.
    pub cwd: String,
    /// The MCP servers the session was given.
    pub servers: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Message,
    Thought,
}

/// What is known of one model call that a plugin streams.
#[derive(Default)]
struct Attempt {
    /// The position the next frame is to have; frames before it were seen, and a frame after it means some were lost.
    next_seq: u64,
    /// Frames were lost: the call is not shown live any more, and what the program commits is all there is.
    broken: bool,
    /// Whether text of each kind (reasoning, answer) was shown live.
    streamed: [bool; 2],
}

fn slot(kind: TextKind) -> usize {
    match kind {
        TextKind::Thought => 0,
        TextKind::Message => 1,
    }
}

struct OpenText {
    kind: TextKind,
    message_id: Option<String>,
    attempt: Option<String>,
    native_ref: String,
    text: String,
    chars: i64,
}

struct Turn {
    id: TurnId,
    model: String,
    /// What the companion plugin reported for the turn, request by request.
    reported_requests: bool,
    tokens: TokenUsage,
}

struct Tracked {
    view: ToolView,
    native_ref: String,
    started: Time,
    last: Json,
}

/// The conversion of one session's updates into events.
pub struct Mapper {
    config: MapperConfig,
    clock: Arc<dyn Clock>,
    outputs: OutputStore,
    turn: Option<Turn>,
    text: Option<OpenText>,
    tools: HashMap<String, Tracked>,
    /// The native references given in this session, kept as 64-bit hashes (8 bytes an item, not the text).
    used: HashSet<u64>,
    serial: u64,
    /// Names this run of the program, for the references that are made up here (see [`run_token`]).
    run: String,
    plan: Option<(String, PlanPayload)>,
    compacting: Option<Time>,
    thread_tokens: TokenUsage,
    attempts: HashMap<String, Attempt>,
}

/// What is kept of a native reference to know that it was given.
fn ref_key(native: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    native.hash(&mut hasher);
    hasher.finish()
}

/// How many characters of a new identifier make the token of a run: its last twelve hexadecimal digits are 48 bits
/// that are drawn at random, and a thread meets a few dozen runs of its program in all its life, so two that share a
/// token are not to be expected.
const RUN_TOKEN_CHARS: usize = 12;

/// A token that names one run of a program (one process, from its start to its end).
///
/// A thread goes on through the runs of its program: a program that ends, or is ended, is started again and resumes
/// the same conversation. Orch keeps one approval for each reference a thread has been given, and one item for each
/// reference of a segment, and takes a second with a reference it has already seen for the same event again. So a
/// counter that starts from nothing in each run gave the first question of every new run the name of the first
/// question of the run before, and Orch dropped it. A reference that the adapter makes up carries this token, so that
/// no run repeats the references of an earlier one.
#[must_use]
pub fn run_token() -> String {
    let id = orch_types::new_id().simple().to_string();
    id[id.len().saturating_sub(RUN_TOKEN_CHARS)..].to_owned()
}

impl Mapper {
    /// How many references the session has given and how many calls are open: what the mapper holds that grows with the
    /// work of a session.
    #[must_use]
    pub fn tracked(&self) -> (usize, usize) {
        (self.used.len(), self.tools.len())
    }

    /// A mapper for a session.
    #[must_use]
    pub fn new(config: MapperConfig, clock: Arc<dyn Clock>, outputs: OutputStore) -> Self {
        Self {
            config,
            clock,
            outputs,
            turn: None,
            text: None,
            tools: HashMap::new(),
            used: HashSet::new(),
            serial: 0,
            run: run_token(),
            plan: None,
            compacting: None,
            thread_tokens: TokenUsage::default(),
            attempts: HashMap::new(),
        }
    }

    /// The token that names this run of the program, which the references made up here carry.
    #[must_use]
    pub fn run(&self) -> &str {
        &self.run
    }

    /// The turn that runs, if one does.
    #[must_use]
    pub fn turn_id(&self) -> Option<TurnId> {
        self.turn.as_ref().map(|turn| turn.id)
    }

    /// What the tool calls are read against.
    #[must_use]
    pub fn place(&self) -> Place<'_> {
        Place {
            program: &self.config.program,
            directory: &self.config.directory,
            cwd: &self.config.cwd,
            servers: &self.config.servers,
        }
    }

    /// A call that has begun, whether or not it has ended.
    #[must_use]
    pub fn tool(&self, id: &str) -> Option<&ToolView> {
        self.tools.get(id).map(|tracked| &tracked.view)
    }

    /// The tokens the thread has used, as far as the plugin reported them.
    #[must_use]
    pub fn thread_tokens(&self) -> TokenUsage {
        self.thread_tokens
    }

    /// A reference for an item that no item of the segment has had: the agent's own name for it where it gave one, else
    /// a number of this run, which carries the run's token because the next run counts from nothing again.
    fn fresh_ref(&mut self, prefix: &str, id: Option<&str>) -> String {
        self.serial += 1;
        let mut native = match id {
            Some(id) => format!("{prefix}:{id}"),
            None => format!("{prefix}:{}:{}", self.run, self.serial),
        };
        while !self.used.insert(ref_key(&native)) {
            self.serial += 1;
            native = format!("{native}~{}", self.serial);
        }
        native
    }

    /// A turn of Orch begins.
    pub fn begin_turn(
        &mut self,
        turn_id: TurnId,
        native_ref: &str,
        model: &str,
    ) -> Vec<SessionEvent> {
        self.turn = Some(Turn {
            id: turn_id,
            model: model.to_owned(),
            reported_requests: false,
            tokens: TokenUsage::default(),
        });
        vec![SessionEvent::TurnStarted(TurnStarted {
            turn_id,
            native_turn_ref: native_ref.to_owned(),
        })]
    }

    /// Whether a turn runs.
    #[must_use]
    pub fn running(&self) -> bool {
        self.turn.is_some()
    }

    /// Turns one `update` of a `session/update` into events. An update that means nothing to a turn gives none.
    pub fn update(&mut self, update: &Json) -> Vec<SessionEvent> {
        let Some(kind) = update.get("sessionUpdate").and_then(Json::as_str) else {
            return Vec::new();
        };
        match kind {
            "agent_message_chunk" => self.chunk(TextKind::Message, update),
            "agent_thought_chunk" => self.chunk(TextKind::Thought, update),
            "tool_call" => self.tool_call(update),
            "tool_call_update" => self.tool_update(update),
            "plan" => self.plan_update(update),
            "usage_update" => self.usage_update(update),
            "compaction_update" => self.compaction_update(update),
            "session_info_update" => update
                .get("title")
                .and_then(Json::as_str)
                .map(|title| title.trim())
                .filter(|title| !title.is_empty())
                .map(|title| {
                    vec![SessionEvent::SessionTitle(SessionTitle {
                        title: title.chars().take(120).collect(),
                    })]
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    // -- text ---------------------------------------------------------------------------------------------

    fn chunk(&mut self, kind: TextKind, update: &Json) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let Some(turn_id) = self.turn_id() else {
            tracing::debug!("a chunk of text arrived outside a turn of Orch, and is not reported");
            return out;
        };
        let Some(text) = update.get("content").and_then(content_text) else {
            return out;
        };
        let message_id = update.get("messageId").and_then(Json::as_str);
        let attempt = committed_attempt(update);
        let open_of_attempt = attempt.is_some_and(|attempt| {
            self.text
                .as_ref()
                .is_some_and(|open| open.kind == kind && open.attempt.as_deref() == Some(attempt))
        });
        let shown = attempt.is_some_and(|attempt| {
            self.attempts
                .get(attempt)
                .is_some_and(|known| known.streamed[slot(kind)])
        });
        if shown && !open_of_attempt {
            // What was shown live has ended already (the call went on to another kind of text); this says the same.
            return out;
        }
        let same = open_of_attempt
            || self.text.as_ref().is_some_and(|open| {
                open.kind == kind
                    && open.attempt.is_none()
                    && open.message_id.as_deref() == message_id
            });
        if !same {
            if text.is_empty() {
                return out;
            }
            self.start_text(turn_id, kind, message_id, message_id, attempt, &mut out);
        }
        if open_of_attempt && let Some(open) = self.text.as_mut() {
            open.message_id = message_id.map(str::to_owned);
        }
        let piece = if open_of_attempt {
            let Some(open) = self.text.as_mut() else {
                return out;
            };
            match text.strip_prefix(open.text.as_str()) {
                Some(rest) => rest.to_owned(),
                None => {
                    // What streamed is not a start of the whole: the completed item carries the whole text.
                    open.text = text;
                    open.chars = i64::try_from(open.text.chars().count()).unwrap_or(i64::MAX);
                    return out;
                }
            }
        } else {
            text
        };
        self.grow(&piece, &mut out);
        out
    }

    /// Begins an item of text, after the one that grew has ended.
    fn start_text(
        &mut self,
        turn_id: TurnId,
        kind: TextKind,
        reference: Option<&str>,
        message_id: Option<&str>,
        attempt: Option<&str>,
        out: &mut Vec<SessionEvent>,
    ) {
        out.extend(self.complete_text(false));
        let prefix = match kind {
            TextKind::Message => "msg",
            TextKind::Thought => "thought",
        };
        let native_ref = self.fresh_ref(prefix, reference);
        let content = match kind {
            TextKind::Message => ItemContent::AgentMessage(AgentMessagePayload {
                text: String::new(),
                text_len: 0,
                phase: None,
            }),
            TextKind::Thought => ItemContent::Reasoning(ReasoningPayload {
                summary: String::new(),
                text_len: 0,
                hidden: false,
            }),
        };
        out.push(SessionEvent::ItemStarted(ItemStarted {
            turn_id,
            native_ref: native_ref.clone(),
            stream: MAIN_STREAM.to_owned(),
            parent_native_ref: None,
            content,
        }));
        self.text = Some(OpenText {
            kind,
            message_id: message_id.map(str::to_owned),
            attempt: attempt.map(str::to_owned),
            native_ref,
            text: String::new(),
            chars: 0,
        });
    }

    /// Adds to the item that grows.
    fn grow(&mut self, piece: &str, out: &mut Vec<SessionEvent>) {
        let Some(open) = self.text.as_mut() else {
            return;
        };
        if piece.is_empty() {
            return;
        }
        let field = match open.kind {
            TextKind::Message => DeltaField::Text,
            TextKind::Thought => DeltaField::Summary,
        };
        out.push(SessionEvent::ItemDelta(ItemDelta::text(
            &open.native_ref,
            field,
            open.chars,
            piece,
        )));
        open.chars += i64::try_from(piece.chars().count()).unwrap_or(i64::MAX);
        open.text.push_str(piece);
    }

    /// The frames of a model call that a companion plugin streams, shown as they come.
    pub fn stream(&mut self, notice: &StreamNotice) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let Some(turn_id) = self.turn_id() else {
            return out;
        };
        let (skip, broken) = {
            let known = self.attempts.entry(notice.attempt.clone()).or_default();
            let frames = u64::try_from(notice.frames.len()).unwrap_or(u64::MAX);
            let skip = known.next_seq.saturating_sub(notice.seq);
            if notice.seq > known.next_seq {
                known.broken = true;
            }
            known.next_seq = known.next_seq.max(notice.seq.saturating_add(frames));
            (usize::try_from(skip).unwrap_or(usize::MAX), known.broken)
        };
        for frame in notice.frames.iter().skip(skip) {
            let (kind, delta) = match frame {
                Frame::Text { delta } => (TextKind::Message, delta),
                Frame::Reasoning { delta } => (TextKind::Thought, delta),
                Frame::End { outcome } => {
                    if outcome == "abandoned"
                        && self.text.as_ref().is_some_and(|open| {
                            open.attempt.as_deref() == Some(notice.attempt.as_str())
                        })
                    {
                        out.extend(self.complete_text(false));
                    }
                    continue;
                }
                Frame::Start | Frame::ToolInput | Frame::Other => continue,
            };
            if broken || delta.is_empty() {
                continue;
            }
            let growing = self.text.as_ref().is_some_and(|open| {
                open.kind == kind && open.attempt.as_deref() == Some(notice.attempt.as_str())
            });
            if !growing {
                self.start_text(
                    turn_id,
                    kind,
                    Some(&notice.attempt),
                    None,
                    Some(&notice.attempt),
                    &mut out,
                );
            }
            if let Some(known) = self.attempts.get_mut(&notice.attempt) {
                known.streamed[slot(kind)] = true;
            }
            self.grow(delta, &mut out);
        }
        out
    }

    /// Ends the item that grows, if one does; `last` says it is the one that ends the turn.
    fn complete_text(&mut self, last: bool) -> Option<SessionEvent> {
        let open = self.text.take()?;
        let payload = match open.kind {
            TextKind::Message => serde_json::to_value(AgentMessagePayload {
                text: open.text,
                text_len: open.chars,
                phase: Some(if last {
                    AgentMessagePhase::Final
                } else {
                    AgentMessagePhase::Commentary
                }),
            }),
            TextKind::Thought => serde_json::to_value(ReasoningPayload {
                summary: open.text,
                text_len: open.chars,
                hidden: false,
            }),
        }
        .unwrap_or(Json::Null);
        Some(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: open.native_ref,
            status: ItemStatus::Completed,
            payload,
            output_blob: None,
        }))
    }

    // -- tool calls ---------------------------------------------------------------------------------------

    fn read_view(&self, update: &Json, base: Option<&ToolView>) -> Option<ToolView> {
        let id = update.get("toolCallId").and_then(Json::as_str)?.to_owned();
        let mut view = base.cloned().unwrap_or_else(|| ToolView {
            id: id.clone(),
            name: String::new(),
            title: String::new(),
            kind: "other".to_owned(),
            understood: crate::tools::Understood::Other,
            status: Status::Pending,
            raw_input: Json::Null,
            raw_output: None,
            content: Vec::new(),
            locations: Vec::new(),
        });
        if let Some(name) = update.get("name").and_then(Json::as_str) {
            view.name = name.to_owned();
        }
        if let Some(title) = update.get("title").and_then(Json::as_str) {
            view.title = title.to_owned();
        }
        if view.name.is_empty() {
            view.name = view.title.clone();
        }
        if let Some(kind) = update.get("kind").and_then(Json::as_str) {
            view.kind = kind.to_owned();
        }
        if let Some(status) = update
            .get("status")
            .and_then(Json::as_str)
            .and_then(Status::read)
        {
            view.status = status;
        }
        if let Some(content) = update.get("content").and_then(Json::as_array) {
            view.content = content.clone();
        }
        if let Some(locations) = update.get("locations").and_then(Json::as_array) {
            view.locations = locations.clone();
        }
        if let Some(input) = update.get("rawInput").filter(|input| !input.is_null()) {
            view.raw_input = input.clone();
        }
        if let Some(output) = update.get("rawOutput").filter(|output| !output.is_null()) {
            view.raw_output = Some(output.clone());
        }
        view.understood = understand(&self.place(), &view.name, &view.title, &view.kind);
        Some(view)
    }

    /// The call a request of the agent names, as the adapter knows it: what the earlier updates said of it
    /// (`known`), with what the request itself says laid over.
    #[must_use]
    pub fn view_of(&self, tool_call: &Json, known: Option<&ToolView>) -> Option<ToolView> {
        self.read_view(tool_call, known)
    }

    fn tool_payload_of(
        &self,
        view: &ToolView,
        started: Time,
        ended: Option<Time>,
    ) -> ToolCallPayload {
        let place = self.place();
        ToolCallPayload {
            tool: tool_payload(view, &place),
            title: call_title(view, &place),
            started_at: started,
            duration_ms: ended.map(|ended| {
                ended
                    .unix_millis()
                    .saturating_sub(started.unix_millis())
                    .max(0)
            }),
            raw: raw_call(view),
        }
    }

    fn tool_call(&mut self, update: &Json) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let Some(turn_id) = self.turn_id() else {
            return out;
        };
        let Some(id) = update.get("toolCallId").and_then(Json::as_str) else {
            return out;
        };
        if self.tools.contains_key(id) {
            return self.tool_update(update);
        }
        out.extend(self.complete_text(false));
        let Some(view) = self.read_view(update, None) else {
            return out;
        };
        let started = self.clock.time();
        let native_ref = self.fresh_ref("tool", Some(id));
        let payload = self.tool_payload_of(&view, started, None);
        let last = serde_json::to_value(&payload).unwrap_or(Json::Null);
        out.push(SessionEvent::ItemStarted(ItemStarted {
            turn_id,
            native_ref: native_ref.clone(),
            stream: MAIN_STREAM.to_owned(),
            parent_native_ref: None,
            content: ItemContent::ToolCall(payload),
        }));
        let ended = view.status.ended();
        self.tools.insert(
            id.to_owned(),
            Tracked {
                view,
                native_ref,
                started,
                last,
            },
        );
        if ended {
            out.extend(self.finish_tool(id, None));
        }
        out
    }

    fn tool_update(&mut self, update: &Json) -> Vec<SessionEvent> {
        let Some(id) = update.get("toolCallId").and_then(Json::as_str) else {
            return Vec::new();
        };
        if !self.tools.contains_key(id) {
            if self.turn.is_none() {
                return Vec::new();
            }
            // An update of a call that was never announced makes the call.
            let mut announced = update.clone();
            if announced.get("title").is_none() {
                announced["title"] = json!("tool");
            }
            if let Some(object) = announced.as_object_mut() {
                object.insert("sessionUpdate".into(), json!("tool_call"));
            }
            return self.tool_call(&announced);
        }
        let Some(base) = self.tools.get(id).map(|tracked| tracked.view.clone()) else {
            return Vec::new();
        };
        let Some(view) = self.read_view(update, Some(&base)) else {
            return Vec::new();
        };
        let ended = view.status.ended();
        if let Some(tracked) = self.tools.get_mut(id) {
            tracked.view = view;
        }
        if ended {
            return self.finish_tool(id, None);
        }
        let Some(tracked) = self.tools.get(id) else {
            return Vec::new();
        };
        let payload = self.tool_payload_of(&tracked.view, tracked.started, None);
        let now = serde_json::to_value(&payload).unwrap_or(Json::Null);
        if now == tracked.last {
            return Vec::new();
        }
        let native_ref = tracked.native_ref.clone();
        if let Some(tracked) = self.tools.get_mut(id) {
            tracked.last = now.clone();
        }
        vec![SessionEvent::ItemUpdated(ItemUpdated {
            native_ref,
            payload: json!({ "tool": now["tool"], "title": now["title"], "raw": now["raw"] }),
        })]
    }

    /// Ends a call. `forced` is how it ended where the agent did not say: interrupted or failed.
    fn finish_tool(&mut self, id: &str, forced: Option<ItemStatus>) -> Vec<SessionEvent> {
        let Some(mut tracked) = self.tools.remove(id) else {
            return Vec::new();
        };
        if let Some(status) = forced {
            tracked.view.status = if status == ItemStatus::Completed {
                Status::Completed
            } else {
                Status::Failed
            };
        }
        let ended = self.clock.time();
        let mut payload = self.tool_payload_of(&tracked.view, tracked.started, Some(ended));
        let blob = self.keep_long_output(&mut payload.tool, &tracked.view);
        let status = forced.unwrap_or(if tracked.view.status == Status::Failed {
            ItemStatus::Failed
        } else {
            ItemStatus::Completed
        });
        vec![SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: tracked.native_ref,
            status,
            payload: serde_json::to_value(&payload).unwrap_or(Json::Null),
            output_blob: blob,
        })]
    }

    /// Keeps the whole of an output that is longer than what stays inline, as a file, and says where.
    fn keep_long_output(&self, tool: &mut ToolPayload, view: &ToolView) -> Option<BlobRef> {
        let output = match tool {
            ToolPayload::Command { output, .. } => Some(output),
            ToolPayload::FileRead { content, .. } | ToolPayload::WebFetch { content, .. } => {
                Some(content)
            }
            _ => None,
        }?;
        if !output.truncated {
            return None;
        }
        let whole = output_of(view);
        match self.outputs.put(whole.as_bytes()) {
            Ok(blob) => {
                output.blob = Some(blob.hash);
                Some(blob)
            }
            Err(error) => {
                tracing::warn!(%error, "the whole of a long output could not be kept");
                None
            }
        }
    }

    // -- plans, usage, phases ------------------------------------------------------------------------------

    fn plan_update(&mut self, update: &Json) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let Some(turn_id) = self.turn_id() else {
            return out;
        };
        let steps: Vec<PlanStep> = update
            .get("entries")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let text = entry.get("content").and_then(Json::as_str)?.to_owned();
                let status = match entry.get("status").and_then(Json::as_str) {
                    Some("in_progress") => PlanStepStatus::InProgress,
                    Some("completed") => PlanStepStatus::Completed,
                    _ => PlanStepStatus::Pending,
                };
                Some(PlanStep { text, status })
            })
            .collect();
        let payload = PlanPayload {
            explanation: None,
            steps,
        };
        match &mut self.plan {
            Some((native_ref, held)) => {
                *held = payload.clone();
                out.push(SessionEvent::ItemUpdated(ItemUpdated {
                    native_ref: native_ref.clone(),
                    payload: json!({ "steps": payload.steps }),
                }));
            }
            None => {
                out.extend(self.complete_text(false));
                let native_ref = self.fresh_ref("plan", Some(&turn_id.to_string()));
                out.push(SessionEvent::ItemStarted(ItemStarted {
                    turn_id,
                    native_ref: native_ref.clone(),
                    stream: MAIN_STREAM.to_owned(),
                    parent_native_ref: None,
                    content: ItemContent::Plan(payload.clone()),
                }));
                self.plan = Some((native_ref, payload));
            }
        }
        out
    }

    fn usage_update(&mut self, update: &Json) -> Vec<SessionEvent> {
        let Some(turn) = self.turn.as_ref() else {
            return Vec::new();
        };
        let used = update.get("used").and_then(Json::as_i64);
        let size = update.get("size").and_then(Json::as_i64);
        if used.is_none() && size.is_none() {
            return Vec::new();
        }
        vec![SessionEvent::UsageUpdated(UsageUpdated {
            turn_id: turn.id,
            turn: turn.tokens,
            thread: self.thread_tokens,
            context_window: size.filter(|size| *size > 0),
            context_used: used.filter(|used| *used >= 0),
            report: None,
        })]
    }

    fn compaction_update(&mut self, update: &Json) -> Vec<SessionEvent> {
        let Some(turn_id) = self.turn_id() else {
            return Vec::new();
        };
        let at = self.clock.time();
        let kind = match update.get("status").and_then(Json::as_str) {
            Some("in_progress") => {
                self.compacting = Some(at);
                TurnPhaseKind::Compacting
            }
            Some(_) => {
                self.compacting = None;
                TurnPhaseKind::Working
            }
            None => return Vec::new(),
        };
        vec![SessionEvent::TurnPhase(TurnPhase { turn_id, kind, at })]
    }

    /// The companion plugin says that the agent started or ended compacting.
    pub fn phase(&mut self, compacting: bool) -> Vec<SessionEvent> {
        self.compaction_update(
            &json!({ "status": if compacting { "in_progress" } else { "completed" } }),
        )
    }

    /// The companion plugin reports one model call; its tokens join the turn's and the thread's.
    pub fn model_call(&mut self, request: UsageRequest) -> Vec<(TurnId, UsageRequest)> {
        let Some(turn) = self.turn.as_mut() else {
            return Vec::new();
        };
        turn.reported_requests = true;
        let usage = request.tokens.usage();
        turn.tokens = add(turn.tokens, usage);
        self.thread_tokens = add(self.thread_tokens, usage);
        vec![(turn.id, request)]
    }

    // -- the end of a turn ---------------------------------------------------------------------------------

    /// The turn ends: everything that is open ends with it, and the turn is complete. The second answer is the
    /// observation of the turn's model request, where nothing finer was reported.
    pub fn end_turn(
        &mut self,
        outcome: TurnOutcome,
        error: Option<TurnError>,
    ) -> (Vec<SessionEvent>, Option<(TurnId, UsageRequest)>) {
        let mut out = Vec::new();
        let Some(turn) = self.turn.take() else {
            return (out, None);
        };
        self.attempts.clear();
        out.extend(self.complete_text(outcome == TurnOutcome::Completed));
        let forced = match outcome {
            TurnOutcome::Interrupted => ItemStatus::Cancelled,
            TurnOutcome::Failed => ItemStatus::Failed,
            TurnOutcome::Completed => ItemStatus::Cancelled,
        };
        let mut open: Vec<String> = self.tools.keys().cloned().collect();
        open.sort();
        for id in open {
            out.extend(self.finish_tool(&id, Some(forced)));
        }
        if let Some((native_ref, held)) = self.plan.take() {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref,
                status: ItemStatus::Completed,
                payload: serde_json::to_value(held).unwrap_or(Json::Null),
                output_blob: None,
            }));
        }
        if self.compacting.take().is_some() {
            out.push(SessionEvent::TurnPhase(TurnPhase {
                turn_id: turn.id,
                kind: TurnPhaseKind::Working,
                at: self.clock.time(),
            }));
        }
        out.push(SessionEvent::TurnCompleted(TurnCompleted {
            turn_id: turn.id,
            status: outcome,
            error: error.clone(),
        }));
        let request = (!turn.reported_requests).then(|| {
            let mut request = UsageRequest {
                id: format!("turn:{}", turn.id),
                model: Some(turn.model.clone()),
                ..UsageRequest::default()
            };
            match (outcome, &error) {
                (TurnOutcome::Completed, _) => request.finish("done", None, None),
                (_, Some(error)) => {
                    request.finish(&error.code, Some(&error.message), error.said.as_deref())
                }
                (_, None) => request.finish("forced", Some("The turn was stopped."), None),
            }
            (turn.id, request)
        });
        self.serial += 1;
        (out, request)
    }
}

fn add(a: TokenUsage, b: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: a.input_tokens + b.input_tokens,
        cached_input_tokens: a.cached_input_tokens + b.cached_input_tokens,
        cache_write_tokens: a.cache_write_tokens + b.cache_write_tokens,
        output_tokens: a.output_tokens + b.output_tokens,
        reasoning_tokens: a.reasoning_tokens + b.reasoning_tokens,
        total_tokens: a.total_tokens + b.total_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::ManualClock;

    fn mapper() -> Mapper {
        let program = Arc::new(
            AcpProgram::from_json(&json!({
                "kind": "demo", "title": "Demo", "supported_range": "*",
                "tools": [ { "names": ["bash"], "family": "command" } ]
            }))
            .unwrap(),
        );
        let dir = std::env::temp_dir().join(format!("orch-acp-mapper-{}", orch_types::new_id()));
        Mapper::new(
            MapperConfig {
                thread_id: orch_types::new_id(),
                segment_id: orch_types::new_id(),
                program,
                directory: "/w".into(),
                cwd: "/w".into(),
                servers: vec!["orch".into()],
            },
            Arc::new(ManualClock::new("2026-10-29T00:00:00Z".parse().unwrap())),
            OutputStore::new(dir),
        )
    }

    fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
        events.iter().map(SessionEvent::kind).collect()
    }

    fn text_chunk(kind: &str, id: &str, text: &str) -> Json {
        json!({ "sessionUpdate": kind, "messageId": id, "content": { "type": "text", "text": text } })
    }

    #[test]
    fn two_runs_of_a_program_never_give_an_item_they_number_themselves_the_same_reference() {
        // The agent names no message, so the mapper numbers the item; a run that starts again counts from one.
        let first_item = |mapper: &mut Mapper| {
            mapper.begin_turn(orch_types::new_id(), "t1", "m");
            let events = mapper.update(&json!({
                "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "hello" }
            }));
            events
                .iter()
                .find_map(|event| match event {
                    SessionEvent::ItemStarted(started) => Some(started.native_ref.clone()),
                    _ => None,
                })
                .expect("an item began")
        };
        let (mut earlier, mut later) = (mapper(), mapper());
        let (named_earlier, named_later) = (first_item(&mut earlier), first_item(&mut later));
        assert_ne!(
            named_earlier, named_later,
            "Orch keeps one item for each reference of a segment, and a restarted program resumes the same segment"
        );
        assert_eq!(named_earlier, format!("msg:{}:1", earlier.run()));
        assert_eq!(named_later, format!("msg:{}:1", later.run()));
    }

    #[test]
    fn a_name_the_agent_gave_an_item_is_kept_as_it_is() {
        let mut mapper = mapper();
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        let events = mapper.update(&text_chunk("agent_message_chunk", "m-7", "hello"));
        let SessionEvent::ItemStarted(started) = &events[0] else {
            panic!("an item began")
        };
        assert_eq!(started.native_ref, "msg:m-7");
    }

    #[test]
    fn a_thought_and_a_message_of_one_message_id_are_two_items_and_the_last_message_is_final() {
        let mut mapper = mapper();
        let turn = orch_types::new_id();
        assert_eq!(kinds(&mapper.begin_turn(turn, "t1", "m")), ["turn.started"]);
        let first = mapper.update(&text_chunk("agent_thought_chunk", "m1", "think "));
        assert_eq!(kinds(&first), ["item.started", "item.delta"]);
        let second = mapper.update(&text_chunk("agent_thought_chunk", "m1", "more"));
        assert_eq!(kinds(&second), ["item.delta"]);
        let SessionEvent::ItemDelta(delta) = &second[0] else {
            panic!()
        };
        assert_eq!((delta.offset, delta.len), (6, 4));
        let message = mapper.update(&text_chunk("agent_message_chunk", "m1", "Done."));
        assert_eq!(
            kinds(&message),
            ["item.completed", "item.started", "item.delta"]
        );
        let SessionEvent::ItemCompleted(thought) = &message[0] else {
            panic!()
        };
        assert_eq!(thought.payload["summary"], "think more");
        let (end, request) = mapper.end_turn(TurnOutcome::Completed, None);
        assert_eq!(kinds(&end), ["item.completed", "turn.completed"]);
        let SessionEvent::ItemCompleted(last) = &end[0] else {
            panic!()
        };
        assert_eq!(last.payload["phase"], "final");
        assert_eq!(last.payload["text"], "Done.");
        let (turn_id, request) = request.expect("a turn with no finer report has its own request");
        assert_eq!(turn_id, turn);
        assert!(request.check().is_ok());
        assert_eq!(request.tokens.input_tokens, None, "no count is invented");
    }

    #[test]
    fn a_message_that_goes_on_to_a_tool_call_is_commentary_and_the_call_runs_its_life() {
        let mut mapper = mapper();
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        mapper.update(&text_chunk("agent_message_chunk", "m1", "Let me check."));
        let started = mapper.update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "bash", "kind": "other",
            "status": "in_progress", "rawInput": { "command": "echo hi", "description": "greet" }
        }));
        assert_eq!(kinds(&started), ["item.completed", "item.started"]);
        let SessionEvent::ItemCompleted(message) = &started[0] else {
            panic!()
        };
        assert_eq!(message.payload["phase"], "commentary");
        let SessionEvent::ItemStarted(call) = &started[1] else {
            panic!()
        };
        let ItemContent::ToolCall(payload) = &call.content else {
            panic!()
        };
        assert_eq!(payload.title, "greet");
        assert!(
            matches!(&payload.tool, ToolPayload::Command { command, .. } if command == "echo hi")
        );
        assert!(mapper.tool("c1").is_some());
        let done = mapper.update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
            "content": [ { "type": "content", "content": { "type": "text", "text": "hi\n" } } ]
        }));
        assert_eq!(kinds(&done), ["item.completed"]);
        let SessionEvent::ItemCompleted(done) = &done[0] else {
            panic!()
        };
        assert_eq!(done.status, ItemStatus::Completed);
        assert_eq!(done.payload["tool"]["output"]["inline"], "hi\n");
        assert_eq!(done.payload["tool"]["exit_code"], 0);
        assert!(done.payload["duration_ms"].is_i64());
        assert!(mapper.tool("c1").is_none(), "an ended call is not kept");
    }

    #[test]
    fn an_interrupt_ends_the_open_text_and_the_open_calls_and_the_request_says_how() {
        let mut mapper = mapper();
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        mapper.update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "bash", "kind": "other", "rawInput": {} }));
        mapper.update(&text_chunk("agent_message_chunk", "m2", "half a sent"));
        let (events, request) = mapper.end_turn(TurnOutcome::Interrupted, None);
        assert_eq!(
            kinds(&events),
            ["item.completed", "item.completed", "turn.completed"]
        );
        let SessionEvent::ItemCompleted(call) = &events[1] else {
            panic!()
        };
        assert_eq!(call.status, ItemStatus::Cancelled);
        let (_, request) = request.unwrap();
        assert!(matches!(
            request.outcome,
            Some(orch_agent_contract::requests::RequestOutcome::Error { .. })
        ));
        assert!(request.check().is_ok());
    }

    #[test]
    fn a_plan_is_one_item_for_the_turn_updated_in_place() {
        let mut mapper = mapper();
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        let first = mapper.update(&json!({ "sessionUpdate": "plan", "entries": [
            { "content": "a", "priority": "high", "status": "in_progress" }, { "content": "b", "priority": "low", "status": "pending" } ] }));
        assert_eq!(kinds(&first), ["item.started"]);
        let second = mapper.update(&json!({ "sessionUpdate": "plan", "entries": [
            { "content": "a", "priority": "high", "status": "completed" }, { "content": "b", "priority": "low", "status": "in_progress" } ] }));
        assert_eq!(kinds(&second), ["item.updated"]);
        let (end, _) = mapper.end_turn(TurnOutcome::Completed, None);
        assert_eq!(kinds(&end), ["item.completed", "turn.completed"]);
    }

    #[test]
    fn context_use_is_reported_as_size_and_fill_and_a_chunk_outside_a_turn_is_not_reported() {
        let mut mapper = mapper();
        assert!(
            mapper
                .update(&text_chunk("agent_message_chunk", "m", "stray"))
                .is_empty()
        );
        assert!(
            mapper
                .update(&json!({ "sessionUpdate": "usage_update", "used": 5, "size": 10 }))
                .is_empty()
        );
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        let usage = mapper
            .update(&json!({ "sessionUpdate": "usage_update", "used": 5702, "size": 128000 }));
        let SessionEvent::UsageUpdated(usage) = &usage[0] else {
            panic!()
        };
        assert_eq!(
            (usage.context_used, usage.context_window),
            (Some(5702), Some(128000))
        );
        assert_eq!(usage.turn.total_tokens, 0);
    }

    #[test]
    fn a_companion_plugins_model_calls_replace_the_envelope_and_add_up_for_the_thread() {
        let mut mapper = mapper();
        let turn = orch_types::new_id();
        mapper.begin_turn(turn, "t1", "m");
        let mut request = UsageRequest {
            id: "r1".into(),
            ..UsageRequest::default()
        };
        request.tokens.input_tokens = Some(100);
        request.tokens.output_tokens = Some(20);
        request.tokens.total_tokens = Some(120);
        request.finish("done", None, None);
        let reported = mapper.model_call(request);
        assert_eq!(reported.len(), 1);
        assert_eq!(mapper.thread_tokens().total_tokens, 120);
        let (_, envelope) = mapper.end_turn(TurnOutcome::Completed, None);
        assert!(
            envelope.is_none(),
            "the finer report stands in for the envelope"
        );
    }

    #[test]
    fn a_title_the_agent_reports_is_kept_to_a_hundred_and_twenty_characters() {
        let mut mapper = mapper();
        let events = mapper
            .update(&json!({ "sessionUpdate": "session_info_update", "title": "x".repeat(300) }));
        let SessionEvent::SessionTitle(title) = &events[0] else {
            panic!()
        };
        assert_eq!(title.title.chars().count(), 120);
    }

    fn frames(attempt: &str, seq: u64, frames: Json) -> StreamNotice {
        StreamNotice::read(&json!({ "sessionId": "s", "turn": 1, "step": 0, "attempt": attempt, "seq": seq, "frames": frames }))
            .unwrap()
    }

    fn committed(kind: &str, id: &str, attempt: &str, text: &str) -> Json {
        let mut update = text_chunk(kind, id, text);
        update["_meta"] = json!({ "orch": { "attempt": attempt, "turn": 1, "step": 0 } });
        update
    }

    #[test]
    fn a_streamed_call_grows_its_items_and_the_committed_messages_finish_them_without_a_second_item()
     {
        let mut mapper = mapper();
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        let live = mapper.stream(&frames(
            "a1",
            0,
            json!([
                { "kind": "start" },
                { "kind": "reasoning", "index": 0, "delta": "hm" },
                { "kind": "reasoning", "index": 0, "delta": "m" },
                { "kind": "text", "index": 1, "delta": "Hel" },
            ]),
        ));
        assert_eq!(
            kinds(&live),
            [
                "item.started",
                "item.delta",
                "item.delta",
                "item.completed",
                "item.started",
                "item.delta"
            ]
        );
        let live = mapper.stream(&frames(
            "a1",
            4,
            json!([
                { "kind": "text", "index": 1, "delta": "lo" },
                { "kind": "end", "outcome": "committed" },
            ]),
        ));
        assert_eq!(kinds(&live), ["item.delta"]);
        // The program commits what it logged: the thought that ended already says the same, the message adds its rest.
        assert!(
            mapper
                .update(&committed("agent_thought_chunk", "m1", "a1", "hmm"))
                .is_empty()
        );
        assert!(
            mapper
                .update(&committed("agent_message_chunk", "m1", "a1", "Hello"))
                .is_empty()
        );
        let (end, _) = mapper.end_turn(TurnOutcome::Completed, None);
        assert_eq!(kinds(&end), ["item.completed", "turn.completed"]);
        let SessionEvent::ItemCompleted(item) = &end[0] else {
            panic!()
        };
        assert_eq!(
            (
                item.payload["text"].as_str(),
                item.payload["phase"].as_str()
            ),
            (Some("Hello"), Some("final"))
        );
    }

    #[test]
    fn a_committed_text_that_goes_beyond_what_streamed_adds_its_rest_and_one_that_differs_replaces_the_text()
     {
        let mut mapper = mapper();
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        mapper.stream(&frames(
            "a1",
            0,
            json!([ { "kind": "text", "index": 0, "delta": "Hel" } ]),
        ));
        let events = mapper.update(&committed("agent_message_chunk", "m1", "a1", "Hello"));
        let SessionEvent::ItemDelta(delta) = &events[0] else {
            panic!()
        };
        assert_eq!((delta.offset, delta.data.as_str()), (3, "lo"));
        let mut other = mapper_with_turn();
        other.stream(&frames(
            "a2",
            0,
            json!([ { "kind": "text", "index": 0, "delta": "Hex" } ]),
        ));
        assert!(
            other
                .update(&committed("agent_message_chunk", "m2", "a2", "Hello"))
                .is_empty()
        );
        let (end, _) = other.end_turn(TurnOutcome::Completed, None);
        let SessionEvent::ItemCompleted(item) = &end[0] else {
            panic!()
        };
        assert_eq!(
            item.payload["text"], "Hello",
            "the finished item carries the committed text"
        );
    }

    fn mapper_with_turn() -> Mapper {
        let mut mapper = mapper();
        mapper.begin_turn(orch_types::new_id(), "t1", "m");
        mapper
    }

    #[test]
    fn frames_that_repeat_are_skipped_and_frames_that_were_lost_end_the_live_view_of_that_call() {
        let mut mapper = mapper_with_turn();
        let first = mapper.stream(&frames(
            "a1",
            0,
            json!([ { "kind": "text", "index": 0, "delta": "ab" } ]),
        ));
        assert_eq!(kinds(&first), ["item.started", "item.delta"]);
        assert!(
            mapper
                .stream(&frames(
                    "a1",
                    0,
                    json!([ { "kind": "text", "index": 0, "delta": "ab" } ])
                ))
                .is_empty()
        );
        let lost = mapper.stream(&frames(
            "a1",
            5,
            json!([ { "kind": "text", "index": 0, "delta": "zz" } ]),
        ));
        assert!(lost.is_empty(), "frames after a gap are not shown");
        let events = mapper.update(&committed("agent_message_chunk", "m1", "a1", "abcdef"));
        let SessionEvent::ItemDelta(delta) = &events[0] else {
            panic!()
        };
        assert_eq!(
            delta.data, "cdef",
            "the committed text still completes what was shown"
        );
    }

    #[test]
    fn an_abandoned_call_ends_what_it_showed_and_a_call_nobody_streamed_is_shown_whole() {
        let mut mapper = mapper_with_turn();
        mapper.stream(&frames("a1", 0, json!([ { "kind": "text", "index": 0, "delta": "half" }, { "kind": "end", "outcome": "abandoned" } ])));
        let whole = mapper.update(&committed("agent_message_chunk", "m2", "a2", "Whole."));
        assert_eq!(
            kinds(&whole),
            ["item.started", "item.delta"],
            "an attempt that never streamed is a message like any other"
        );
        let (end, _) = mapper.end_turn(TurnOutcome::Completed, None);
        assert_eq!(kinds(&end), ["item.completed", "turn.completed"]);
    }

    #[test]
    fn frames_outside_a_turn_are_not_shown() {
        let mut mapper = mapper();
        assert!(
            mapper
                .stream(&frames(
                    "a1",
                    0,
                    json!([ { "kind": "text", "index": 0, "delta": "x" } ])
                ))
                .is_empty()
        );
    }

    #[test]
    fn a_session_of_a_hundred_thousand_calls_keeps_eight_bytes_of_reference_for_each_and_no_open_call()
     {
        let mut mapper = mapper_with_turn();
        let mut started = 0;
        for number in 0..100_000 {
            let events = mapper.update(&json!({
                "sessionUpdate": "tool_call", "toolCallId": format!("c{number}"), "title": "bash",
                "kind": "other", "status": "completed", "rawInput": { "command": "true" }
            }));
            started += events
                .iter()
                .filter(|event| event.kind() == "item.started")
                .count();
        }
        assert_eq!(started, 100_000);
        assert_eq!(
            mapper.tracked(),
            (100_000, 0),
            "a reference is a number, and no ended call is held"
        );
        let (end, _) = mapper.end_turn(TurnOutcome::Completed, None);
        assert_eq!(kinds(&end), ["turn.completed"]);
    }

    #[test]
    fn a_reference_given_twice_in_a_session_is_made_different_without_keeping_the_first_in_full() {
        let mut mapper = mapper_with_turn();
        let call = |id: &str| json!({ "sessionUpdate": "tool_call", "toolCallId": id, "title": "bash", "kind": "other", "status": "completed", "rawInput": {} });
        let first = mapper.update(&call("same"));
        let second = mapper.update(&call("same"));
        let reference = |events: &[SessionEvent]| match &events[0] {
            SessionEvent::ItemStarted(started) => started.native_ref.clone(),
            other => panic!("{other:?}"),
        };
        assert_ne!(reference(&first), reference(&second));
    }
}
