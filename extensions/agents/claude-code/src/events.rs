//! Stream messages to neutral events: each tool use as a step of its kind, with the call as Claude Code made it kept whole beside it; sub-agent streams, usage, and unknown frames kept raw.
//!
//! [`EventMapper`] reads the harness's messages one at a time and says what happened, in the neutral
//! vocabulary of Orch. It does no input or output of its own, so every rule here is tested with literal
//! messages. What it keeps between messages is only what the stream itself forces: which blocks of which
//! message are open, which tool uses wait for their result, the list of things to do, and the tokens used.
//!
//! The harness writes each content block of a message twice, as growing pieces and then whole, and the
//! whole one arrives before the block is closed; a sub-agent's blocks arrive whole only. Both are handled
//! alike: a block that was never started is started when it arrives complete. What a person said is never
//! reported: the harness echoes it, and the echo only tells the adapter that the message was taken in.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use orch_agent_contract::events::UsageRequest;
use orch_agent_contract::events::{
    AgentMessagePayload, AgentMessagePhase, DeltaField, ErrorPayload, ItemCompleted, ItemContent,
    ItemDelta, ItemStarted, ItemStatus, ItemUpdated, MAIN_STREAM, MarkerPayload, PlanPayload,
    PlanStep, PlanStepStatus, RawFrame, ReasoningPayload, SessionEvent, TokenUsage,
    TokensReportedApart, ToolCallPayload, ToolPayload, TurnCompleted, TurnError, TurnOutcome,
    TurnPhase, TurnPhaseKind, UsageUpdated,
};
use orch_agent_contract::events::{
    ContextCount, ContextFill, ContextShortening, ModelTokens, StreamTokens, TurnUsageReport,
};
use orch_agent_contract::session::TurnId;
use orch_types::{Clock, Json};
use serde_json::json;

use crate::config::{FAST_MODE_PAUSED, RAW_FRAME_MAX_BYTES, RETRY_NOTICE_ATTEMPT};
use crate::failure::{self, ErrorMessage, LimitInfo, Notes};
use crate::stream::{Frame, FrameKind};
use crate::tools::{
    Family, PlanOp, ToolContext, ToolOutcome, call_title, family_of, raw_call, tool_payload,
};

/// Which way a raw frame went, from the harness.
pub const FROM_HARNESS: &str = "from_harness";
/// Which way a raw frame went, to the harness.
pub const TO_HARNESS: &str = "to_harness";

/// What the mapper needs to know about the session it reads.
#[derive(Debug, Clone)]
pub struct MapperSettings {
    /// The thread's directory, against which paths are shown.
    pub directory: String,
    /// Where the agent starts, inside it.
    pub cwd: String,
    /// Whether every frame is also kept as a diagnostic.
    pub raw_events: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    ToolUse,
}

#[derive(Debug, Clone)]
struct Block {
    native_ref: String,
    kind: BlockKind,
    text: String,
    chars: i64,
    completed: bool,
}

#[derive(Debug, Clone, Default)]
struct Message {
    /// Whether the message ended with a reason for stopping, as one that arrived whole does.
    stopped: bool,
    next_complete: u64,
    text_refs: Vec<String>,
    input_tokens: i64,
    cache_read: i64,
    cache_write: i64,
    output_tokens: i64,
    thinking_tokens: i64,
    /// For a sub-agent's message: its use was added to the sub-agent's share already.
    tallied: bool,
}

#[derive(Debug, Clone)]
struct ToolState {
    name: String,
    family: Family,
    input: Json,
    partial_input: String,
    started_at: orch_types::Time,
    stream: String,
    parent: Option<String>,
    item_started: bool,
    async_launched: bool,
    generation: u64,
    progress_title: Option<String>,
    native_terminal: Option<ItemStatus>,
    origin_turn: Option<TurnId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeTaskKey {
    pub task_id: String,
    pub run_id: Option<String>,
}

impl NativeTaskKey {
    pub(crate) fn from_value(value: &Json) -> Option<Self> {
        let task_id = value.get("task_id")?.as_str()?.to_owned();
        if task_id.is_empty() {
            return None;
        }
        Some(Self {
            task_id,
            run_id: value
                .get("run_id")
                .and_then(Json::as_str)
                .filter(|run| !run.is_empty())
                .map(str::to_owned),
        })
    }
}

#[derive(Debug, Clone)]
struct NativeTask {
    key: NativeTaskKey,
    tool_ref: String,
    local_bash: bool,
    resumed: bool,
    tool_reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Task {
    subject: String,
    status: PlanStepStatus,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredTool {
    name: String,
    input: Json,
    partial_input: String,
    started_at: orch_types::Time,
    stream: String,
    parent: Option<String>,
    item_started: bool,
    async_launched: bool,
}

impl From<ToolState> for StoredTool {
    fn from(tool: ToolState) -> Self {
        Self {
            name: tool.name,
            input: tool.input,
            partial_input: tool.partial_input,
            started_at: tool.started_at,
            stream: tool.stream,
            parent: tool.parent,
            item_started: tool.item_started,
            async_launched: tool.async_launched,
        }
    }
}
impl From<StoredTool> for ToolState {
    fn from(tool: StoredTool) -> Self {
        Self {
            family: family_of(&tool.name),
            name: tool.name,
            input: tool.input,
            partial_input: tool.partial_input,
            started_at: tool.started_at,
            stream: tool.stream,
            parent: tool.parent,
            item_started: tool.item_started,
            async_launched: tool.async_launched,
            generation: 0,
            progress_title: None,
            native_terminal: None,
            origin_turn: None,
        }
    }
}

/// Maps complete historical frames with disk-backed message counters and pending tools.
pub(crate) struct HistoryMapper {
    mapper: EventMapper,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    messages: crate::history::spool::Map<u64>,
    tools: crate::history::spool::Map<StoredTool>,
    background: crate::history::spool::Map<StoredTool>,
    plans: crate::history::spool::Map<Task>,
    plan_dirty: bool,
}

impl HistoryMapper {
    pub(crate) fn new(
        settings: MapperSettings,
        clock: Arc<dyn Clock>,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            mapper: EventMapper::new(settings, clock),
            messages: crate::history::spool::Map::with_cancel(Arc::clone(&cancel))?,
            tools: crate::history::spool::Map::with_cancel(Arc::clone(&cancel))?,
            background: crate::history::spool::Map::with_cancel(Arc::clone(&cancel))?,
            plans: crate::history::spool::Map::with_cancel(Arc::clone(&cancel))?,
            plan_dirty: false,
            cancel,
        })
    }

    pub(crate) fn begin_turn(&mut self, turn: TurnId) -> std::io::Result<()> {
        self.messages = crate::history::spool::Map::with_cancel(Arc::clone(&self.cancel))?;
        self.tools = crate::history::spool::Map::with_cancel(Arc::clone(&self.cancel))?;
        self.plan_dirty = false;
        self.mapper.begin_turn(turn);
        self.mapper.take_requests();
        Ok(())
    }

    pub(crate) fn on_frame(&mut self, frame: &Frame) -> std::io::Result<Vec<SessionEvent>> {
        let message = frame
            .message()
            .and_then(|message| message.get("id"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned();
        if frame.text("type") == Some("assistant") {
            self.mapper.messages.insert(
                (MAIN_STREAM.to_owned(), message.clone()),
                Message {
                    next_complete: self.messages.get(&message)?.unwrap_or_default(),
                    ..Message::default()
                },
            );
        }
        for block in frame.blocks() {
            let id = match block.get("type").and_then(Json::as_str) {
                Some("tool_use") => block.get("id"),
                Some("tool_result") => block.get("tool_use_id"),
                _ => None,
            }
            .and_then(Json::as_str);
            if let Some(id) = id {
                let tool = self.tools.remove(id)?.or(self.background.remove(id)?);
                if let Some(tool) = tool {
                    self.mapper.tools.insert(id.to_owned(), tool.into());
                }
            }
        }
        // Historical frames contain whole messages, never live deltas or timed request boundaries.
        let mut out = Vec::new();
        match frame.text("type") {
            Some("assistant") => self.mapper.assistant(frame, &mut out),
            Some("user") => {
                for block in frame.blocks() {
                    let plan = block
                        .get("tool_use_id")
                        .and_then(Json::as_str)
                        .and_then(|id| self.mapper.tools.get(id))
                        .and_then(|tool| {
                            if let Family::Plan(op) = tool.family {
                                Some((op, tool.input.clone()))
                            } else {
                                None
                            }
                        });
                    if let Some((op, input)) = plan {
                        if let Some(id) = block.get("tool_use_id").and_then(Json::as_str) {
                            self.mapper.tools.remove(id);
                        }
                        self.apply_plan(
                            op,
                            &input,
                            &ToolOutcome::from_block(&block, frame.get("tool_use_result")),
                            &mut out,
                        )?;
                    } else {
                        self.mapper.user(&Frame::from_value(json!({"type":"user", "message":{"content":[block]}, "tool_use_result":frame.get("tool_use_result")})), &mut out);
                    }
                }
            }
            _ => {}
        }
        if frame.text("type") == Some("assistant")
            && let Some(mapped) = self
                .mapper
                .messages
                .get(&(MAIN_STREAM.to_owned(), message.clone()))
        {
            self.messages.insert(&message, &mapped.next_complete)?;
        }
        for (id, tool) in self.mapper.tools.drain() {
            if tool.async_launched {
                self.background.insert(&id, &tool.into())?;
            } else {
                self.tools.insert(&id, &tool.into())?;
            }
        }
        // Whole historical blocks cannot receive later stream deltas. Their counter and open tool
        // state are on disk; the final answer itself remains in pending_message until its phase is known.
        self.mapper.messages.clear();
        self.mapper.blocks.clear();
        self.mapper.pending_reasoning.clear();
        self.mapper.background_changes.clear();
        Ok(out)
    }

    fn apply_plan(
        &mut self,
        op: PlanOp,
        input: &Json,
        outcome: &ToolOutcome,
        out: &mut Vec<SessionEvent>,
    ) -> std::io::Result<()> {
        if outcome.is_error {
            return Ok(());
        }
        let structured = outcome.structured.as_ref();
        match op {
            PlanOp::Create => {
                let id = structured
                    .and_then(|value| value.pointer("/task/id"))
                    .map(json_id)
                    .unwrap_or_default();
                if id.is_empty() {
                    return Ok(());
                }
                self.plans.insert(
                    &id,
                    &Task {
                        subject: input
                            .get("subject")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        status: PlanStepStatus::Pending,
                    },
                )?;
            }
            PlanOp::Update => {
                let Some(id) = input.get("taskId").map(json_id) else {
                    return Ok(());
                };
                if input.get("status").and_then(Json::as_str) == Some("deleted") {
                    self.plans.remove(&id)?;
                } else if let Some(mut task) = self.plans.get(&id)? {
                    if let Some(subject) = input.get("subject").and_then(Json::as_str) {
                        task.subject = subject.to_owned();
                    }
                    if let Some(status @ ("pending" | "in_progress" | "completed")) =
                        input.get("status").and_then(Json::as_str)
                    {
                        task.status = step_status(Some(status));
                    }
                    self.plans.insert(&id, &task)?;
                }
            }
            PlanOp::List => {
                let Some(tasks) = structured
                    .and_then(|value| value.get("tasks"))
                    .and_then(Json::as_array)
                else {
                    return Ok(());
                };
                self.plans = crate::history::spool::Map::with_cancel(Arc::clone(&self.cancel))?;
                for task in tasks {
                    self.plans.insert(
                        &task.get("id").map(json_id).unwrap_or_default(),
                        &Task {
                            subject: task
                                .get("subject")
                                .and_then(Json::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            status: step_status(task.get("status").and_then(Json::as_str)),
                        },
                    )?;
                }
            }
            PlanOp::Write => {
                let Some(tasks) = input.get("todos").and_then(Json::as_array) else {
                    return Ok(());
                };
                self.plans = crate::history::spool::Map::with_cancel(Arc::clone(&self.cancel))?;
                for (position, task) in tasks.iter().enumerate() {
                    self.plans.insert(
                        &format!("{position:04}"),
                        &Task {
                            subject: task
                                .get("content")
                                .and_then(Json::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            status: step_status(task.get("status").and_then(Json::as_str)),
                        },
                    )?;
                }
            }
            PlanOp::Get => return Ok(()),
        }
        if self.mapper.plan_for != self.mapper.turn_id() {
            self.emit_plan(out)?;
        } else {
            self.plan_dirty = true;
        }
        Ok(())
    }

    fn emit_plan(&mut self, out: &mut Vec<SessionEvent>) -> std::io::Result<()> {
        let Some(turn) = self.mapper.turn_id() else {
            return Ok(());
        };
        let mut cursor = 0;
        let mut tasks = Vec::new();
        let mut bytes = 0_usize;
        while let Some((key, task)) = self.plans.next(&mut cursor)? {
            // A plan is one atomic native item. Include spare vector slots and worst-case JSON escaping
            // before collecting it, so even a malformed ever-growing task list cannot exhaust the host.
            bytes = bytes
                .saturating_add(std::mem::size_of::<(String, Task)>().saturating_mul(2))
                .saturating_add(key.capacity())
                .saturating_add(task.subject.capacity().saturating_mul(6));
            if bytes > crate::config::FRAME_MAX_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the historical plan exceeds the native item limit",
                ));
            }
            tasks.push((key, task));
        }
        tasks.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let steps: Vec<PlanStep> = tasks
            .into_iter()
            .map(|(_, task)| PlanStep {
                text: task.subject,
                status: task.status,
            })
            .collect();
        let native_ref = format!("plan:{turn}");
        if self.mapper.plan_for == Some(turn) {
            out.push(SessionEvent::ItemUpdated(ItemUpdated {
                native_ref,
                payload: json!({"steps":steps}),
            }));
        } else {
            self.mapper.plan_for = Some(turn);
            out.extend(self.mapper.started(
                &native_ref,
                MAIN_STREAM,
                None,
                ItemContent::Plan(PlanPayload {
                    explanation: None,
                    steps,
                }),
            ));
        }
        Ok(())
    }

    pub(crate) fn close_step(
        &mut self,
        interrupted: bool,
        cursor: &mut u64,
    ) -> std::io::Result<Option<Vec<SessionEvent>>> {
        let mut out = Vec::new();
        if self.plan_dirty {
            self.emit_plan(&mut out)?;
            self.plan_dirty = false;
            return Ok(Some(out));
        }
        let frame = Frame::from_value(json!({
            "type":"result", "subtype":if interrupted { "error_during_execution" } else { "success" },
            "is_error":interrupted, "terminal_reason":if interrupted { "aborted_streaming" } else { "completed" }, "usage":{}
        }));
        let (status, _) = outcome_of(
            &frame,
            self.mapper.interrupted,
            &self.mapper.failure,
            self.mapper.clock.now(),
            &chrono::Local,
        );
        self.mapper
            .complete_message(status == TurnOutcome::Completed, &mut out);
        if !out.is_empty() {
            return Ok(Some(out));
        }
        while let Some((id, tool)) = self.tools.next(cursor)? {
            if tool.async_launched {
                continue;
            }
            self.tools.remove(&id)?;
            self.mapper.tools.insert(id, tool.into());
            self.mapper.close_open_items(
                if self.mapper.interrupted {
                    ItemStatus::Cancelled
                } else {
                    ItemStatus::Failed
                },
                &mut out,
            );
            return Ok(Some(out));
        }
        Ok(None)
    }

    pub(crate) fn end_turn(&mut self, interrupted: bool) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        self.mapper.result(&Frame::from_value(json!({
            "type":"result", "subtype":if interrupted { "error_during_execution" } else { "success" },
            "is_error":interrupted, "terminal_reason":if interrupted { "aborted_streaming" } else { "completed" }, "usage":{}
        })), &mut out);
        out
    }

    #[cfg(test)]
    pub(crate) fn retained_rows(&self) -> usize {
        self.mapper.messages.len()
            + self.mapper.blocks.len()
            + self.mapper.tools.len()
            + self.mapper.pending_reasoning.len()
            + self.mapper.tasks.len()
            + self.mapper.request_reports.len()
            + self.mapper.active_messages.len()
            + self.mapper.sub_tally.len()
            + self.mapper.echoed.len()
            + usize::from(self.mapper.pending_message.is_some())
    }
}

/// Reads the harness's messages and says what happened.
#[derive(Debug)]
pub struct EventMapper {
    requests: crate::performance::Requests,
    request_reports: Vec<(TurnId, UsageRequest)>,
    request_owners: HashMap<String, TurnId>,
    active_messages: HashMap<String, String>,
    settings: MapperSettings,
    clock: Arc<dyn Clock>,
    turn: Option<TurnId>,
    last_turn: Option<TurnId>,
    interrupted: bool,
    blocks: HashMap<(String, String, u64), Block>,
    /// Empty thinking blocks stay private until readable text arrives.
    pending_reasoning: HashMap<(String, String, u64), SessionEvent>,
    /// The last whole main-stream message is finalized when the turn's outcome makes its phase certain.
    pending_message: Option<ItemCompleted>,
    messages: HashMap<(String, String), Message>,
    tools: HashMap<String, ToolState>,
    native_session: Option<String>,
    native_generation: u64,
    native_tasks: HashMap<String, NativeTask>,
    background_records: BTreeMap<String, orch_agent_contract::background::BackgroundTask>,
    background_changes: Vec<orch_agent_contract::background::BackgroundTask>,
    tasks: BTreeMap<String, Task>,
    plan_for: Option<TurnId>,
    turn_tally: TokenUsage,
    thread_tally: TokenUsage,
    context_used: Option<i64>,
    /// Each sub-agent's use during the turn, by the stream of its items, summed from its messages.
    sub_tally: BTreeMap<String, TokenUsage>,
    /// Each model's running total at the last result of the process. `None` while it is not known: a process
    /// that resumed a session starts from the totals the harness restored.
    model_base: Option<BTreeMap<String, TokenUsage>>,
    /// The window of the model in force, as the last result said.
    window: Option<i64>,
    /// Where the harness shortens the conversation, as its last breakdown of the context said.
    compact_at: Option<i64>,
    /// A shortening of the conversation that waits for the next prompt to say its size after it.
    pending_shortening: Option<ContextShortening>,
    /// The last report of use, which a breakdown of the context that comes after it is carried with.
    last_usage: Option<UsageUpdated>,
    /// Whether the harness answers what fills its context when asked; until it says it does not.
    breakdown: bool,
    compacting: bool,
    announced_compaction: bool,
    echoed: Vec<String>,
    retry_noticed: bool,
    /// The model, by the name the harness's last start states (`system/init`), suffix of a larger window and all.
    running_model: Option<String>,
    /// What the stream has said of a failure during this turn, for the `result` that ends it.
    failure: Notes,
    /// The row of the table of failures that found the last failed turn, for a log and for the tests.
    last_failure_row: Option<&'static str>,
}

fn chars(text: &str) -> i64 {
    i64::try_from(text.chars().count()).unwrap_or(i64::MAX)
}

fn stream_of(parent: Option<&str>) -> (String, Option<String>) {
    match parent {
        Some(id) => (
            orch_agent_contract::events::sub_stream(id),
            Some(id.to_owned()),
        ),
        None => (MAIN_STREAM.to_owned(), None),
    }
}

impl EventMapper {
    /// A mapper for a session.
    #[must_use]
    pub fn new(settings: MapperSettings, clock: Arc<dyn Clock>) -> Self {
        Self {
            requests: crate::performance::Requests::default(),
            request_reports: Vec::new(),
            request_owners: HashMap::new(),
            active_messages: HashMap::new(),
            settings,
            clock,
            turn: None,
            last_turn: None,
            interrupted: false,
            blocks: HashMap::new(),
            pending_reasoning: HashMap::new(),
            pending_message: None,
            messages: HashMap::new(),
            tools: HashMap::new(),
            native_session: None,
            native_generation: 0,
            native_tasks: HashMap::new(),
            background_records: BTreeMap::new(),
            background_changes: Vec::new(),
            tasks: BTreeMap::new(),
            plan_for: None,
            turn_tally: TokenUsage::default(),
            thread_tally: TokenUsage::default(),
            context_used: None,
            sub_tally: BTreeMap::new(),
            model_base: None,
            window: None,
            compact_at: None,
            pending_shortening: None,
            last_usage: None,
            breakdown: true,
            compacting: false,
            announced_compaction: false,
            echoed: Vec::new(),
            retry_noticed: false,
            running_model: None,
            failure: Notes::default(),
            last_failure_row: None,
        }
    }

    /// The row of the table of failures (see [`failure`]) that found the last turn that failed.
    #[must_use]
    pub fn last_failure_row(&self) -> Option<&'static str> {
        self.last_failure_row
    }

    /// A turn begins: what follows belongs to it.
    pub fn begin_turn(&mut self, turn: TurnId) {
        self.request_reports.clear();
        if !self.background_running() {
            self.requests = crate::performance::Requests::default();
        }
        let asked = self.requests.asked(self.clock.time());
        self.request_reports.push((turn, asked));
        self.active_messages.clear();
        self.turn = Some(turn);
        self.last_turn = Some(turn);
        self.interrupted = false;
        self.turn_tally = TokenUsage::default();
        self.sub_tally.clear();
        self.retry_noticed = false;
        self.announced_compaction = false;
        self.failure = Notes::default();
    }

    /// Rejected native work is metered under its origin without reusing its request ids.
    pub(crate) fn begin_rejected_wake(&mut self, turn: TurnId) {
        self.turn = Some(turn);
        self.last_turn = Some(turn);
        self.interrupted = true;
        self.turn_tally = TokenUsage::default();
        self.sub_tally.clear();
        self.failure = Notes::default();
        let asked = self.requests.asked(self.clock.time());
        self.request_reports.push((turn, asked));
    }

    /// A process of the session was started: one that began the conversation counts its use from nothing;
    /// one that resumed or forked it starts from totals the harness restored, which are not known here.
    pub fn process_started(&mut self, fresh: bool) {
        self.model_base = fresh.then(BTreeMap::new);
        self.native_generation = self.native_generation.saturating_add(1);
        self.native_tasks.clear();
        self.background_records.clear();
        self.background_changes.clear();
    }

    pub(crate) fn set_native_session(&mut self, session: &str) {
        self.native_session = Some(session.to_owned());
    }

    /// The turn was asked to stop.
    pub fn note_interrupt(&mut self) {
        self.interrupted = true;
    }

    /// Whether a turn is under way.
    #[must_use]
    pub fn turn_active(&self) -> bool {
        self.turn.is_some()
    }

    /// The ids of the inputs the harness has taken in since this was last asked.
    pub fn take_echoed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.echoed)
    }

    /// The raw diagnostic for a frame that went to the harness.
    #[must_use]
    pub fn raw_outgoing(&self, line: &Json) -> Option<SessionEvent> {
        self.settings
            .raw_events
            .then(|| raw_event(TO_HARNESS, line))
    }

    fn turn_id(&self) -> Option<TurnId> {
        self.turn.or(self.last_turn)
    }

    /// Raw request observations, drained by the runtime before turn completion is emitted.
    pub fn take_requests(&mut self) -> Vec<(TurnId, UsageRequest)> {
        std::mem::take(&mut self.request_reports)
    }

    /// Closes pending request reports when the process exits without a result frame.
    pub fn abort_requests(&mut self, code: &str, message: &str) {
        let Some(turn) = self.turn.or(self.last_turn) else {
            return;
        };
        for request in self.requests.abort(code, Some(message), None) {
            self.request_reports.push((turn, request));
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // Frames

    /// Reads one message of the harness and says what happened.
    pub fn on_frame(&mut self, frame: &Frame) -> Vec<SessionEvent> {
        let arrived = self.clock.time();
        let mut out = Vec::new();
        match frame.kind() {
            FrameKind::StreamEvent => self.stream_event(frame, &mut out),
            FrameKind::Assistant => self.assistant(frame, &mut out),
            FrameKind::User => self.user(frame, &mut out),
            FrameKind::System => self.system(frame, &mut out),
            FrameKind::Result => self.result(frame, &mut out),
            FrameKind::Other(name) if name == "rate_limit_event" => self.rate_limit_event(frame),
            FrameKind::ControlRequest
            | FrameKind::ControlResponse
            | FrameKind::ControlCancelRequest
            | FrameKind::KeepAlive
            | FrameKind::Other(_) => {}
        }
        let completed = out.iter().find_map(|event| match event {
            SessionEvent::TurnCompleted(end) => Some(end),
            _ => None,
        });
        let code = completed
            .and_then(|end| end.error.as_ref())
            .map(|e| e.code.as_str())
            .unwrap_or(
                if completed.is_some_and(|end| end.status == TurnOutcome::Interrupted) {
                    "forced"
                } else if completed.is_some_and(|end| end.status == TurnOutcome::Failed) {
                    "harness_error"
                } else {
                    "done"
                },
            );
        let message = completed
            .and_then(|end| end.error.as_ref())
            .map(|error| error.message.as_str());
        let requests = self.requests.observe(frame, arrived, Some((code, message)));
        for mut request in requests {
            if let Some(orch_agent_contract::requests::RequestOutcome::Error { said, .. }) =
                &mut request.outcome
                && let Some(words) = completed
                    .and_then(|end| end.error.as_ref())
                    .and_then(|error| error.said.clone())
            {
                *said = Some(words);
            }
            if request.subagent && !self.request_owners.contains_key(&request.id) {
                let parent = frame.parent_tool_use_id();
                let owner = parent
                    .and_then(|parent| self.tools.get(parent))
                    .and_then(|tool| tool.origin_turn)
                    .or_else(|| {
                        parent
                            .and_then(|parent| {
                                self.background_records
                                    .values()
                                    .find(|task| task.native_item_ref == parent)
                            })
                            .map(|task| task.origin_turn)
                    });
                if let Some(owner) = owner {
                    self.request_owners.insert(request.id.clone(), owner);
                }
            }
            self.request_observation(request);
        }
        if !self.background_changes.is_empty() {
            out.push(SessionEvent::BackgroundTasks(
                orch_agent_contract::background::BackgroundTasks {
                    tasks: std::mem::take(&mut self.background_changes),
                    self_waking: true,
                    native_idle: false,
                    native_wake: None,
                },
            ));
        }
        if self.settings.raw_events && !matches!(frame.kind(), FrameKind::KeepAlive) {
            out.push(raw_event(FROM_HARNESS, frame.raw()));
        }
        out
    }

    fn started(
        &self,
        native_ref: &str,
        stream: &str,
        parent: Option<&str>,
        content: ItemContent,
    ) -> Option<SessionEvent> {
        Some(SessionEvent::ItemStarted(ItemStarted {
            turn_id: self.turn_id()?,
            native_ref: native_ref.to_owned(),
            stream: stream.to_owned(),
            parent_native_ref: parent.map(str::to_owned),
            content,
        }))
    }

    fn tool_context(&self) -> ToolContext<'_> {
        ToolContext {
            directory: &self.settings.directory,
            cwd: &self.settings.cwd,
        }
    }

    // --- stream events -----------------------------------------------------------------------------------

    fn stream_event(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let Some(event) = frame.get("event") else {
            return;
        };
        let parent = frame.parent_tool_use_id();
        let (stream, parent_ref) = stream_of(parent);
        let message_id = frame
            .text("api_message_id")
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .or_else(|| self.active_messages.get(&stream).cloned())
            .unwrap_or_default();
        let index = event.get("index").and_then(Json::as_u64).unwrap_or(0);
        match event.get("type").and_then(Json::as_str) {
            Some("message_start") => {
                if parent.is_none() {
                    // An answer is being written and is not known to be whole until its message says why it
                    // stopped; a failure that comes before that has cut it off.
                    self.failure.cut = true;
                }
                let id = event
                    .pointer("/message/id")
                    .and_then(Json::as_str)
                    .unwrap_or(&message_id)
                    .to_owned();
                self.active_messages.insert(stream.clone(), id.clone());
                if parent.is_some() {
                    // A sub-agent of the turn: its share is reported, nothing yet.
                    self.sub_tally.entry(stream.clone()).or_default();
                }
                let message = self.messages.entry((stream, id)).or_default();
                if let Some(usage) = event.pointer("/message/usage") {
                    absorb_usage(message, usage);
                }
            }
            Some("content_block_start") => {
                let Some(block) = event.get("content_block") else {
                    return;
                };
                let key = (stream.clone(), message_id.clone(), index);
                match block.get("type").and_then(Json::as_str) {
                    Some("text") => self.open_text(key, &stream, parent_ref.as_deref(), false, out),
                    Some("thinking") => {
                        self.open_text(key, &stream, parent_ref.as_deref(), true, out);
                    }
                    Some("tool_use") => {
                        let id = block.get("id").and_then(Json::as_str).unwrap_or_default();
                        let name = block.get("name").and_then(Json::as_str).unwrap_or_default();
                        self.open_tool(id, name, &json!({}), &stream, parent_ref.as_deref(), out);
                        self.blocks.insert(
                            key,
                            Block {
                                native_ref: id.to_owned(),
                                kind: BlockKind::ToolUse,
                                text: String::new(),
                                chars: 0,
                                completed: false,
                            },
                        );
                    }
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                let key = (stream, message_id, index);
                let delta = event.get("delta");
                match delta
                    .and_then(|delta| delta.get("type"))
                    .and_then(Json::as_str)
                {
                    Some("text_delta") => {
                        let text = delta
                            .and_then(|delta| delta.get("text"))
                            .and_then(Json::as_str)
                            .unwrap_or_default();
                        self.grow(&key, DeltaField::Text, text, out);
                    }
                    Some("thinking_delta") => {
                        let text = delta
                            .and_then(|delta| delta.get("thinking"))
                            .and_then(Json::as_str)
                            .unwrap_or_default();
                        self.grow(&key, DeltaField::Summary, text, out);
                    }
                    Some("input_json_delta") => {
                        let piece = delta
                            .and_then(|delta| delta.get("partial_json"))
                            .and_then(Json::as_str)
                            .unwrap_or_default();
                        if let Some(tool) = self
                            .tool_of_block(&key)
                            .and_then(|id| self.tools.get_mut(&id))
                        {
                            tool.partial_input.push_str(piece);
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let key = (stream, message_id, index);
                // A block of the main conversation that was never sent whole is not done by being closed: the
                // harness closes the blocks of a stream that broke too. The end of its message tells which.
                let unsealed = self
                    .blocks
                    .get(&key)
                    .is_some_and(|block| !block.completed && block.kind != BlockKind::ToolUse);
                if key.0 == MAIN_STREAM && unsealed {
                    return;
                }
                self.close_block(&key, out);
            }
            Some("message_delta") => {
                if parent.is_some() {
                    // A sub-agent's message says its output as it ends; its input came with its start.
                    if let Some(usage) = event.get("usage")
                        && let Some(message) = self.messages.get_mut(&(stream, message_id))
                    {
                        absorb_usage(message, usage);
                    }
                    return;
                }
                let key = (stream, message_id);
                let stop = event
                    .pointer("/delta/stop_reason")
                    .and_then(Json::as_str)
                    .map(str::to_owned);
                if let Some(usage) = event.get("usage")
                    && let Some(message) = self.messages.get_mut(&key)
                {
                    absorb_usage(message, usage);
                }
                if let Some(stop) = stop {
                    self.failure.refusal = if stop == "refusal" {
                        event.get("delta").cloned()
                    } else {
                        None
                    };
                    if let Some(message) = self.messages.get_mut(&key) {
                        message.stopped = true;
                    }
                    self.failure.cut = false;
                    self.complete_unsealed(&key, out);
                    self.phase_texts(&key, &stop, out);
                }
            }
            Some("message_stop") => {
                if parent.is_some() {
                    self.sub_message_done(&(stream, message_id));
                    return;
                }
                let key = (stream, message_id);
                if !self
                    .messages
                    .get(&key)
                    .is_some_and(|message| message.stopped)
                {
                    // The message ended with no reason for stopping: the stream broke. What it left open was
                    // never finished, and the answer stays marked as cut off.
                    self.close_open_items(ItemStatus::Cancelled, out);
                }
                self.message_done(&key, out);
            }
            _ => {}
        }
    }

    fn request_observation(&mut self, request: UsageRequest) {
        let turn_id = if request.subagent {
            self.request_owners.get(&request.id).copied()
        } else {
            self.turn_id()
        };
        let Some(turn_id) = turn_id else { return };
        if request.subagent && request.outcome.is_some() && request.tokens.total_tokens.is_some() {
            self.request_owners.remove(&request.id);
        }
        self.request_reports.push((turn_id, request));
    }

    /// The text blocks of a message that ended with a reason for stopping and that the harness closed without
    /// sending them whole are done.
    fn complete_unsealed(&mut self, message: &(String, String), out: &mut Vec<SessionEvent>) {
        let mut keys: Vec<(String, String, u64)> = self
            .blocks
            .iter()
            .filter(|(key, block)| {
                key.0 == message.0
                    && key.1 == message.1
                    && !block.completed
                    && block.kind != BlockKind::ToolUse
            })
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort_by_key(|key| key.2);
        for key in keys {
            self.close_block(&key, out);
        }
    }

    fn open_text(
        &mut self,
        key: (String, String, u64),
        stream: &str,
        parent: Option<&str>,
        thinking: bool,
        out: &mut Vec<SessionEvent>,
    ) {
        if self.blocks.contains_key(&key) {
            return;
        }
        if !thinking && stream == MAIN_STREAM {
            self.complete_message(false, out);
        }
        let native_ref = format!("{}:{}", key.1, key.2);
        let content = if thinking {
            ItemContent::Reasoning(ReasoningPayload {
                summary: String::new(),
                text_len: 0,
                hidden: false,
            })
        } else {
            ItemContent::AgentMessage(AgentMessagePayload {
                text: String::new(),
                text_len: 0,
                phase: (stream == MAIN_STREAM).then_some(AgentMessagePhase::Commentary),
            })
        };
        if thinking {
            if let Some(started) = self.started(&native_ref, stream, parent, content) {
                self.pending_reasoning.insert(key.clone(), started);
            }
        } else {
            out.extend(self.started(&native_ref, stream, parent, content));
        }
        if !thinking {
            self.messages
                .entry((key.0.clone(), key.1.clone()))
                .or_default()
                .text_refs
                .push(native_ref.clone());
        }
        self.blocks.insert(
            key,
            Block {
                native_ref,
                kind: if thinking {
                    BlockKind::Thinking
                } else {
                    BlockKind::Text
                },
                text: String::new(),
                chars: 0,
                completed: false,
            },
        );
    }

    fn grow(
        &mut self,
        key: &(String, String, u64),
        field: DeltaField,
        piece: &str,
        out: &mut Vec<SessionEvent>,
    ) {
        if !piece.is_empty() {
            out.extend(self.pending_reasoning.remove(key));
        }
        let Some(block) = self.blocks.get_mut(key) else {
            return;
        };
        if piece.is_empty() {
            return;
        }
        out.push(SessionEvent::ItemDelta(ItemDelta::text(
            &block.native_ref,
            field,
            block.chars,
            piece,
        )));
        block.text.push_str(piece);
        block.chars += chars(piece);
    }

    fn tool_of_block(&self, key: &(String, String, u64)) -> Option<String> {
        self.blocks
            .get(key)
            .filter(|block| block.kind == BlockKind::ToolUse)
            .map(|block| block.native_ref.clone())
    }

    fn close_block(&mut self, key: &(String, String, u64), out: &mut Vec<SessionEvent>) {
        let Some(block) = self.blocks.get(key) else {
            return;
        };
        if block.completed {
            return;
        }
        match block.kind {
            BlockKind::Text | BlockKind::Thinking => {
                let text = block.text.clone();
                let thinking = block.kind == BlockKind::Thinking;
                self.complete_text(key, &text, thinking, out);
            }
            BlockKind::ToolUse => {
                if let Some(block) = self.blocks.get_mut(key) {
                    block.completed = true;
                }
                // The complete input is parsed from what was streamed when the whole block did not arrive.
                let id = key_tool_id(self, key);
                if let Some(id) = id {
                    self.finish_input(&id, out);
                }
            }
        }
    }

    fn complete_text(
        &mut self,
        key: &(String, String, u64),
        text: &str,
        thinking: bool,
        out: &mut Vec<SessionEvent>,
    ) {
        if let Some(started) = self.pending_reasoning.remove(key) {
            if text.is_empty() {
                if let Some(block) = self.blocks.get_mut(key) {
                    block.completed = true;
                }
                return;
            }
            out.push(started);
        }
        let Some(block) = self.blocks.get_mut(key) else {
            return;
        };
        block.completed = true;
        block.text = text.to_owned();
        block.chars = chars(text);
        let payload = if thinking {
            serde_json::to_value(ReasoningPayload {
                summary: text.to_owned(),
                text_len: chars(text),
                hidden: false,
            })
        } else {
            serde_json::to_value(AgentMessagePayload {
                text: text.to_owned(),
                text_len: chars(text),
                phase: (key.0 == MAIN_STREAM).then_some(AgentMessagePhase::Commentary),
            })
        };
        if let Ok(payload) = payload {
            let done = ItemCompleted {
                native_ref: block.native_ref.clone(),
                status: ItemStatus::Completed,
                payload,
                output_blob: None,
            };
            if !thinking && key.0 == MAIN_STREAM {
                // The whole frame can arrive without any text deltas. Show it immediately, while its
                // completion waits for the next message or the result that identifies the answer.
                out.push(SessionEvent::ItemUpdated(ItemUpdated {
                    native_ref: done.native_ref.clone(),
                    payload: done.payload.clone(),
                }));
                self.complete_message(false, out);
                self.pending_message = Some(done);
            } else {
                out.push(SessionEvent::ItemCompleted(done));
            }
        }
    }

    fn complete_message(&mut self, final_answer: bool, out: &mut Vec<SessionEvent>) {
        if let Some(mut done) = self.pending_message.take() {
            done.payload["phase"] = json!(if final_answer {
                AgentMessagePhase::Final
            } else {
                AgentMessagePhase::Commentary
            });
            out.push(SessionEvent::ItemCompleted(done));
        }
    }

    /// Said on the way, or the answer: known when the message ends.
    fn phase_texts(&mut self, key: &(String, String), stop: &str, out: &mut Vec<SessionEvent>) {
        let Some(message) = self.messages.get(key) else {
            return;
        };
        let refs = message.text_refs.clone();
        for native_ref in &refs {
            out.push(SessionEvent::ItemUpdated(ItemUpdated {
                native_ref: native_ref.clone(),
                payload: json!({ "phase": AgentMessagePhase::Commentary }),
            }));
        }
        if matches!(stop, "tool_use" | "pause_turn") {
            self.complete_message(false, out);
        }
    }

    fn message_done(&mut self, key: &(String, String), out: &mut Vec<SessionEvent>) {
        let Some(message) = self.messages.get(key) else {
            return;
        };
        let parts = message_usage(message);
        // What fills the context is this request's prompt: this harness counts its input without the parts
        // read from or written to the cache, so all three make it.
        let prompt = parts.input + parts.cache_read + parts.cache_write;
        self.context_used = Some(prompt);
        let this = TokenUsage::from_parts(parts);
        self.turn_tally = self.turn_tally.plus(this);
        let shortened = self.pending_shortening.take().map(|mut shortening| {
            if shortening.after.is_none() {
                shortening.after = Some(prompt);
            }
            shortening
        });
        if let Some(turn_id) = self.turn_id() {
            let subagents = self.subagent_total();
            let turn = self.turn_tally.plus(subagents);
            let event = UsageUpdated {
                turn_id,
                turn,
                thread: self.thread_tally.plus(turn),
                context_window: None,
                context_used: self.context_used,
                report: Some(self.report(subagents, shortened, Vec::new())),
            };
            self.last_usage = Some(event.clone());
            out.push(SessionEvent::UsageUpdated(event));
        }
    }

    /// A sub-agent's message ended: what it used is its sub-agent's share of the turn. Its stream events count
    /// it once (the complete messages of a stream repeat a placeholder of the output).
    fn sub_message_done(&mut self, key: &(String, String)) {
        let Some(message) = self.messages.get_mut(key) else {
            return;
        };
        if std::mem::replace(&mut message.tallied, true) {
            return;
        }
        let used = TokenUsage::from_parts(message_usage(message));
        let share = self.sub_tally.entry(key.0.clone()).or_default();
        *share = share.plus(used);
    }

    /// What the sub-agents of the turn used so far, together.
    fn subagent_total(&self) -> TokenUsage {
        self.sub_tally
            .values()
            .fold(TokenUsage::default(), |sum, share| sum.plus(*share))
    }

    /// Where the harness shortens the conversation: as its last breakdown said, else below the window by its
    /// own margin.
    fn compact_point(&self) -> Option<i64> {
        self.compact_at
            .or_else(|| self.window.map(|window| window - COMPACT_MARGIN))
            .filter(|at| *at > 0)
    }

    /// What the turn reported beyond its totals so far.
    fn report(
        &self,
        subagents: TokenUsage,
        shortened: Option<ContextShortening>,
        models: Vec<ModelTokens>,
    ) -> TurnUsageReport {
        TurnUsageReport {
            api_time_ms: None,
            subagents: Some(subagents),
            by_subagent: self
                .sub_tally
                .iter()
                .map(|(stream, tokens)| StreamTokens {
                    stream: stream.clone(),
                    tokens: *tokens,
                })
                .collect(),
            models,
            compact_at: self.compact_point(),
            shortened,
            context: None,
            breakdown: self.breakdown,
        }
    }

    /// The harness answered that it does not break its context down (an older version): from now on Orch
    /// estimates it.
    pub fn breakdown_refused(&mut self) {
        self.breakdown = false;
    }

    /// The harness answered a request for what fills its context: the breakdown in Orch's words, and the last
    /// report of use carried with it for the turn it was taken after. Either is `None` where there is none: an
    /// answer that is no breakdown, or no use reported yet.
    pub fn context_answered(
        &mut self,
        answer: &Json,
        count: ContextCount,
    ) -> (Option<ContextFill>, Option<SessionEvent>) {
        let Some(fill) = crate::context_usage::fill_of(answer, count, self.clock.time()) else {
            return (None, None);
        };
        if fill.compact_at.is_some() {
            self.compact_at = fill.compact_at;
        }
        let event = self.last_usage.clone().map(|mut event| {
            let report = event.report.get_or_insert_with(TurnUsageReport::default);
            report.compact_at = fill.compact_at.or(report.compact_at);
            // A shortening was reported once, with the use it came with.
            report.shortened = None;
            report.context = Some(fill.clone());
            SessionEvent::UsageUpdated(event)
        });
        (Some(fill), event)
    }

    // --- complete assistant messages ---------------------------------------------------------------------

    fn assistant(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        if frame.parent_tool_use_id().is_none()
            && let Some(message) = frame.message()
            && orch_provider_contract::failure::completed_refusal(message).is_some()
        {
            self.failure.refusal = Some(message.clone());
            return;
        }
        if frame.parent_tool_use_id().is_none()
            && frame
                .message()
                .and_then(|message| message.get("stop_reason"))
                .and_then(Json::as_str)
                .is_some_and(|reason| reason != "refusal")
        {
            self.failure.refusal = None;
        }
        if frame
            .get("is_api_error_message")
            .and_then(Json::as_bool)
            .unwrap_or(false)
        {
            // The turn's end carries this failure; it is not something the agent said. What the message says of
            // it, in fields, is kept for that end.
            self.api_error_message(frame);
            return;
        }
        let parent = frame.parent_tool_use_id();
        let (stream, parent_ref) = stream_of(parent);
        let message_id = frame
            .message()
            .and_then(|message| message.get("id"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned();
        for block in frame.blocks() {
            let index = {
                let message = self
                    .messages
                    .entry((stream.clone(), message_id.clone()))
                    .or_default();
                let index = message.next_complete;
                message.next_complete += 1;
                index
            };
            let key = (stream.clone(), message_id.clone(), index);
            match block.get("type").and_then(Json::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Json::as_str).unwrap_or_default();
                    if text.is_empty() && !self.blocks.contains_key(&key) {
                        continue;
                    }
                    self.open_text(key.clone(), &stream, parent_ref.as_deref(), false, out);
                    self.complete_text(&key, text, false, out);
                }
                Some("thinking") => {
                    let text = block
                        .get("thinking")
                        .and_then(Json::as_str)
                        .unwrap_or_default();
                    self.open_text(key.clone(), &stream, parent_ref.as_deref(), true, out);
                    self.complete_text(&key, text, true, out);
                }
                Some("redacted_thinking") => {
                    self.open_text(key.clone(), &stream, parent_ref.as_deref(), true, out);
                    if self.pending_reasoning.remove(&key).is_some() {
                        if let Some(open) = self.blocks.get_mut(&key) {
                            open.completed = true;
                        }
                        continue;
                    }
                    if let Some(open) = self.blocks.get_mut(&key) {
                        open.completed = true;
                        let native_ref = open.native_ref.clone();
                        if let Ok(payload) = serde_json::to_value(ReasoningPayload {
                            summary: String::new(),
                            text_len: 0,
                            hidden: true,
                        }) {
                            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                                native_ref,
                                status: ItemStatus::Completed,
                                payload,
                                output_blob: None,
                            }));
                        }
                    }
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(Json::as_str).unwrap_or_default();
                    let name = block.get("name").and_then(Json::as_str).unwrap_or_default();
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    self.blocks
                        .entry(key)
                        .and_modify(|open| open.completed = true)
                        .or_insert_with(|| Block {
                            native_ref: id.to_owned(),
                            kind: BlockKind::ToolUse,
                            text: String::new(),
                            chars: 0,
                            completed: true,
                        });
                    self.tool_input_known(id, name, input, &stream, parent_ref.as_deref(), out);
                }
                _ => {}
            }
        }
    }

    // --- tools ---------------------------------------------------------------------------------------------

    fn open_tool(
        &mut self,
        id: &str,
        name: &str,
        input: &Json,
        stream: &str,
        parent: Option<&str>,
        out: &mut Vec<SessionEvent>,
    ) {
        if self.tools.contains_key(id) {
            return;
        }
        if stream == MAIN_STREAM {
            self.complete_message(false, out);
        }
        let family = family_of(name);
        let hidden = matches!(family, Family::Plan(_));
        let state = ToolState {
            name: name.to_owned(),
            family: family.clone(),
            input: input.clone(),
            partial_input: String::new(),
            started_at: self.clock.time(),
            stream: stream.to_owned(),
            parent: parent.map(str::to_owned),
            item_started: !hidden,
            async_launched: false,
            generation: self.native_generation,
            progress_title: None,
            native_terminal: None,
            origin_turn: self.turn_id(),
        };
        if !hidden {
            let payload = self.call_payload(&state, id, None);
            out.extend(self.started(id, stream, parent, ItemContent::ToolCall(payload)));
        }
        self.tools.insert(id.to_owned(), state);
    }

    fn call_payload(
        &self,
        state: &ToolState,
        id: &str,
        outcome: Option<&ToolOutcome>,
    ) -> ToolCallPayload {
        let ctx = self.tool_context();
        let mut tool = tool_payload(&state.family, &state.name, id, &state.input, outcome, &ctx);
        if outcome.is_none()
            && let ToolPayload::Subagent {
                state: native_state,
                ..
            } = &mut tool
            && let Some(terminal) = &state.native_terminal
        {
            *native_state = match terminal {
                ItemStatus::Completed => orch_agent_contract::events::SubagentState::Completed,
                ItemStatus::Cancelled => orch_agent_contract::events::SubagentState::Interrupted,
                _ => orch_agent_contract::events::SubagentState::Failed,
            };
        }
        let duration_ms = outcome
            .map(|_| (self.clock.time().unix_millis() - state.started_at.unix_millis()).max(0));
        ToolCallPayload {
            tool,
            title: outcome
                .is_none()
                .then(|| state.progress_title.clone())
                .flatten()
                .unwrap_or_else(|| call_title(&state.family, &state.name, &state.input, &ctx)),
            started_at: state.started_at,
            duration_ms,
            raw: raw_call(&state.name, &state.input, outcome),
        }
    }

    /// The whole input of a tool use is known: a call that was started with an empty one is brought up to
    /// date, one never started (a sub-agent's) is started now.
    fn tool_input_known(
        &mut self,
        id: &str,
        name: &str,
        input: Json,
        stream: &str,
        parent: Option<&str>,
        out: &mut Vec<SessionEvent>,
    ) {
        if !self.tools.contains_key(id) {
            // Never started (a sub-agent's call arrives whole): it is started with all that is known.
            self.open_tool(id, name, &input, stream, parent, out);
            return;
        }
        let Some(state) = self.tools.get_mut(id) else {
            return;
        };
        state.input = input;
        state.partial_input.clear();
        let state = state.clone();
        if state.item_started {
            let payload = self.call_payload(&state, id, None);
            if let Ok(payload) = serde_json::to_value(&payload) {
                out.push(SessionEvent::ItemUpdated(ItemUpdated {
                    native_ref: id.to_owned(),
                    payload: json!({ "tool": payload["tool"], "title": payload["title"], "raw": payload["raw"] }),
                }));
            }
        }
    }

    /// The input of a tool use that arrived as pieces only.
    fn finish_input(&mut self, id: &str, out: &mut Vec<SessionEvent>) {
        let Some(state) = self.tools.get(id) else {
            return;
        };
        if state.partial_input.is_empty() {
            return;
        }
        let Ok(input) = serde_json::from_str::<Json>(&state.partial_input) else {
            return;
        };
        let (name, stream, parent) = (
            state.name.clone(),
            state.stream.clone(),
            state.parent.clone(),
        );
        self.tool_input_known(id, &name, input, &stream, parent.as_deref(), out);
    }

    // --- user messages: results and echoes ----------------------------------------------------------------

    fn user(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let structured = frame.get("tool_use_result");
        for block in frame.blocks() {
            match block.get("type").and_then(Json::as_str) {
                Some("tool_result") => {
                    let Some(id) = block.get("tool_use_id").and_then(Json::as_str) else {
                        continue;
                    };
                    let outcome = ToolOutcome::from_block(&block, structured);
                    self.tool_result(id, outcome, out);
                }
                Some("text") => {
                    // What a person said, or the note that a turn was stopped: neither is reported.
                    if frame
                        .get("isReplay")
                        .and_then(Json::as_bool)
                        .unwrap_or(false)
                        && let Some(uuid) = frame.uuid()
                    {
                        self.echoed.push(uuid.to_owned());
                    }
                }
                _ => {}
            }
        }
    }

    fn tool_result(&mut self, id: &str, outcome: ToolOutcome, out: &mut Vec<SessionEvent>) {
        let Some(mut state) = self.tools.remove(id) else {
            return;
        };
        if let Family::Plan(op) = state.family {
            self.apply_plan(op, &state.input, &outcome, out);
            return;
        }
        if matches!(state.family, Family::Subagent) {
            let launched = outcome
                .structured
                .as_ref()
                .and_then(|result| result.get("status"))
                .and_then(Json::as_str)
                == Some("async_launched");
            if launched {
                state.async_launched = true;
                self.tools.insert(id.to_owned(), state);
                return;
            }
        }
        let native_background = outcome
            .structured
            .as_ref()
            .and_then(|result| result.get("backgroundTaskId"))
            .and_then(Json::as_str)
            .filter(|id| !id.is_empty());
        if matches!(state.family, Family::Command)
            && (state.input.get("run_in_background").and_then(Json::as_bool) == Some(true)
                || state.async_launched
                || native_background.is_some())
            && !outcome.is_error
        {
            // A native launch is not an exit. Keep the command open until a native task result,
            // even across this turn's completion; no unproved exact End control is advertised.
            state.async_launched = true;
            let mut payload = self.call_payload(&state, id, Some(&outcome));
            if let ToolPayload::Command {
                exit_code,
                background,
                ..
            } = &mut payload.tool
            {
                *exit_code = None;
                *background = true;
            }
            if let Ok(payload) = serde_json::to_value(payload) {
                out.push(SessionEvent::ItemUpdated(ItemUpdated {
                    native_ref: id.to_owned(),
                    payload,
                }));
            }
            self.tools.insert(id.to_owned(), state);
            if let Some(task) = native_background {
                self.bind_native_task(
                    NativeTaskKey {
                        task_id: task.to_owned(),
                        run_id: None,
                    },
                    id,
                    false,
                );
            }
            return;
        }
        let status = if self.interrupted && outcome.is_error {
            ItemStatus::Cancelled
        } else if outcome.is_error {
            ItemStatus::Failed
        } else {
            ItemStatus::Completed
        };
        let payload = self.call_payload(&state, id, Some(&outcome));
        if let Ok(payload) = serde_json::to_value(&payload) {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref: id.to_owned(),
                status,
                payload,
                output_blob: None,
            }));
        }
    }

    // --- the list of things to do ------------------------------------------------------------------------

    fn apply_plan(
        &mut self,
        op: PlanOp,
        input: &Json,
        outcome: &ToolOutcome,
        out: &mut Vec<SessionEvent>,
    ) {
        if outcome.is_error {
            return;
        }
        let structured = outcome.structured.as_ref();
        match op {
            PlanOp::Create => {
                let id = structured
                    .and_then(|result| result.pointer("/task/id"))
                    .map(json_id)
                    .unwrap_or_default();
                if id.is_empty() {
                    return;
                }
                let subject = input
                    .get("subject")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.tasks.insert(
                    id,
                    Task {
                        subject,
                        status: PlanStepStatus::Pending,
                    },
                );
            }
            PlanOp::Update => {
                let Some(id) = input.get("taskId").map(json_id) else {
                    return;
                };
                match input.get("status").and_then(Json::as_str) {
                    Some("deleted") => {
                        self.tasks.remove(&id);
                    }
                    status => {
                        if let Some(task) = self.tasks.get_mut(&id) {
                            if let Some(subject) = input.get("subject").and_then(Json::as_str) {
                                task.subject = subject.to_owned();
                            }
                            match status {
                                Some("pending") => task.status = PlanStepStatus::Pending,
                                Some("in_progress") => task.status = PlanStepStatus::InProgress,
                                Some("completed") => task.status = PlanStepStatus::Completed,
                                _ => {}
                            }
                        }
                    }
                }
            }
            PlanOp::List => {
                let Some(listed) = structured
                    .and_then(|result| result.get("tasks"))
                    .and_then(Json::as_array)
                else {
                    return;
                };
                self.tasks = listed
                    .iter()
                    .map(|task| {
                        (
                            task.get("id").map(json_id).unwrap_or_default(),
                            Task {
                                subject: task
                                    .get("subject")
                                    .and_then(Json::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                status: step_status(task.get("status").and_then(Json::as_str)),
                            },
                        )
                    })
                    .collect();
            }
            PlanOp::Write => {
                let Some(todos) = input.get("todos").and_then(Json::as_array) else {
                    return;
                };
                self.tasks = todos
                    .iter()
                    .enumerate()
                    .map(|(position, todo)| {
                        (
                            format!("{position:04}"),
                            Task {
                                subject: todo
                                    .get("content")
                                    .and_then(Json::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                status: step_status(todo.get("status").and_then(Json::as_str)),
                            },
                        )
                    })
                    .collect();
            }
            PlanOp::Get => return,
        }
        self.emit_plan(out);
    }

    fn emit_plan(&mut self, out: &mut Vec<SessionEvent>) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let steps: Vec<PlanStep> = self
            .tasks
            .values()
            .map(|task| PlanStep {
                text: task.subject.clone(),
                status: task.status,
            })
            .collect();
        let native_ref = format!("plan:{turn_id}");
        if self.plan_for == Some(turn_id) {
            out.push(SessionEvent::ItemUpdated(ItemUpdated {
                native_ref,
                payload: json!({ "steps": steps }),
            }));
        } else {
            self.plan_for = Some(turn_id);
            out.extend(self.started(
                &native_ref,
                MAIN_STREAM,
                None,
                ItemContent::Plan(PlanPayload {
                    explanation: None,
                    steps,
                }),
            ));
        }
    }

    // --- system messages ---------------------------------------------------------------------------------

    fn system(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        if matches!(
            frame.subtype(),
            Some("task_started" | "task_progress" | "task_updated" | "task_notification")
        ) && self
            .native_session
            .as_ref()
            .is_some_and(|session| frame.text("session_id") != Some(session.as_str()))
        {
            return;
        }
        match frame.subtype() {
            Some("init") => {
                self.running_model = frame.text("model").map(str::to_owned);
                for server in frame
                    .get("mcp_servers")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    let status = server
                        .get("status")
                        .and_then(Json::as_str)
                        .unwrap_or_default();
                    if matches!(status, "failed" | "needs-auth") {
                        let name = server
                            .get("name")
                            .and_then(Json::as_str)
                            .unwrap_or("an MCP server");
                        self.error_item(
                            "mcp_server_failed",
                            &format!("The tool server {name} could not be used ({status})."),
                            false,
                            frame.uuid().unwrap_or(name),
                            out,
                        );
                    }
                }
            }
            Some("status") => {
                let compacting = frame.text("status") == Some("compacting");
                if compacting && !self.compacting {
                    self.compacting = true;
                    self.announced_compaction = true;
                    self.phase(TurnPhaseKind::Compacting, out);
                } else if !compacting && self.compacting && frame.get("compact_result").is_some() {
                    self.compacting = false;
                    self.phase(TurnPhaseKind::Working, out);
                }
            }
            Some("compact_boundary") => {
                // How large the conversation was before, and after where the harness says; otherwise the next
                // prompt says it.
                let meta = frame.get("compact_metadata");
                let tokens = |member: &str| {
                    meta.and_then(|meta| meta.get(member))
                        .and_then(Json::as_i64)
                };
                self.pending_shortening = Some(ContextShortening {
                    at: self.clock.time(),
                    before: tokens("pre_tokens"),
                    after: tokens("post_tokens"),
                    automatic: meta
                        .and_then(|meta| meta.get("trigger"))
                        .and_then(Json::as_str)
                        .map(|trigger| trigger == "auto"),
                });
                // A harness that said when it began lets the engine write the marker with its duration;
                // one that only says afterwards gets the marker from here.
                if !std::mem::take(&mut self.announced_compaction) {
                    let native_ref = frame.uuid().unwrap_or("compacted").to_owned();
                    let duration_ms = frame
                        .get("compact_metadata")
                        .and_then(|meta| meta.get("duration_ms"))
                        .and_then(Json::as_i64);
                    out.extend(self.started(
                        &native_ref,
                        MAIN_STREAM,
                        None,
                        ItemContent::Marker(MarkerPayload::Compacted { duration_ms }),
                    ));
                    if let Ok(payload) =
                        serde_json::to_value(MarkerPayload::Compacted { duration_ms })
                    {
                        out.push(SessionEvent::ItemCompleted(ItemCompleted {
                            native_ref,
                            status: ItemStatus::Completed,
                            payload,
                            output_blob: None,
                        }));
                    }
                }
            }
            Some("api_retry") => {
                self.failure.retries.note(frame);
                let attempt = frame.get("attempt").and_then(Json::as_u64).unwrap_or(0);
                if attempt >= RETRY_NOTICE_ATTEMPT && !self.retry_noticed {
                    self.retry_noticed = true;
                    let status = frame
                        .get("error_status")
                        .and_then(Json::as_i64)
                        .map_or_else(|| "no answer".to_owned(), |status| status.to_string());
                    let kind = frame.text("error").unwrap_or("an error");
                    let max = frame.get("max_retries").and_then(Json::as_u64).unwrap_or(0);
                    self.error_item(
                        "upstream_retrying",
                        &format!(
                            "The model service answers {status} ({kind}); the harness tries again (attempt {attempt} of {max})."
                        ),
                        true,
                        frame.uuid().unwrap_or("retry"),
                        out,
                    );
                }
            }
            Some("permission_denied") => {
                let tool = frame.text("tool_name").unwrap_or("a tool");
                let why = frame.text("message").unwrap_or("it was not allowed");
                self.error_item(
                    "permission_denied",
                    &format!("{tool} was not run: {why}"),
                    false,
                    frame.uuid().unwrap_or(tool),
                    out,
                );
            }
            Some("background_tasks_changed") => self.background_set(frame, out),
            Some("task_started") => self.task_started(frame, out),
            Some("task_progress") => self.task_progress(frame, out),
            Some("task_updated") => self.task_updated(frame, out),
            Some("task_notification") => self.task_ended(frame, out),
            _ => {}
        }
    }

    /// Current owned work, retaining terminal facts until a follow-up consumes them.
    pub(crate) fn background_tasks(&self) -> Vec<orch_agent_contract::background::BackgroundTask> {
        self.background_records.values().cloned().collect()
    }

    pub(crate) fn background_running(&self) -> bool {
        self.background_records
            .values()
            .any(|task| task.state == orch_agent_contract::background::BackgroundTaskState::Running)
    }

    fn observe_background(&mut self, frame: &Frame, tool_ref: &str, status: Option<&str>) {
        use orch_agent_contract::background::{
            BackgroundTask, BackgroundTaskKind, BackgroundTaskState,
        };
        let Some(tool) = self.tools.get(tool_ref) else {
            return;
        };
        let Some(origin_turn) = tool.origin_turn.or(self.turn_id()) else {
            return;
        };
        let key = NativeTaskKey::from_value(frame.raw()).or_else(|| {
            self.native_tasks
                .values()
                .find(|task| task.tool_ref == tool_ref)
                .map(|task| task.key.clone())
        });
        let Some(key) = key else { return };
        let identity = serde_json::json!([key.task_id, key.run_id]).to_string();
        let prior = self.background_records.get(&identity);
        if prior.is_none()
            && self.background_records.len() >= orch_agent_contract::background::SNAPSHOT_COMMANDS
        {
            self.background_records
                .retain(|_, task| task.state == BackgroundTaskState::Running);
            if self.background_records.len() >= orch_agent_contract::background::SNAPSHOT_COMMANDS {
                return;
            }
        }
        let prior = self.background_records.get(&identity);
        let is_background = tool.async_launched
            || frame.get("is_backgrounded").and_then(Json::as_bool) == Some(true)
            || frame
                .get("patch")
                .and_then(|patch| patch.get("is_backgrounded"))
                .and_then(Json::as_bool)
                == Some(true)
            || tool.input.get("run_in_background").and_then(Json::as_bool) == Some(true);
        if !is_background && prior.is_none() {
            return;
        }
        let state = if prior.is_some_and(|task| task.state == BackgroundTaskState::Stopped) {
            BackgroundTaskState::Stopped
        } else {
            match status {
                Some("completed") => BackgroundTaskState::Completed,
                Some("failed" | "killed") => BackgroundTaskState::Failed,
                Some("stopped") => BackgroundTaskState::Stopped,
                _ => prior.map_or(BackgroundTaskState::Running, |task| task.state),
            }
        };
        let kind = match tool.family {
            Family::Command => BackgroundTaskKind::Command,
            Family::Subagent => BackgroundTaskKind::Subagent,
            _ => return,
        };
        let label = frame
            .text("description")
            .or_else(|| {
                frame
                    .get("patch")
                    .and_then(|patch| patch.get("description"))
                    .and_then(Json::as_str)
            })
            .or_else(|| prior.map(|task| task.label.as_str()))
            .or_else(|| tool.input.get("name").and_then(Json::as_str))
            .or_else(|| tool.input.get("description").and_then(Json::as_str))
            .or_else(|| tool.input.get("command").and_then(Json::as_str))
            .unwrap_or(&key.task_id);
        let mut end = label
            .len()
            .min(orch_agent_contract::background::COMMAND_PREVIEW_BYTES);
        while !label.is_char_boundary(end) {
            end -= 1;
        }
        let label = label[..end].to_owned();
        let task = BackgroundTask {
            id: key.task_id,
            run_id: key.run_id,
            native_item_ref: tool_ref.to_owned(),
            origin_turn,
            kind,
            label,
            name: tool
                .input
                .get("name")
                .and_then(Json::as_str)
                .map(str::to_owned),
            nickname: None,
            role: tool
                .input
                .get("subagent_type")
                .and_then(Json::as_str)
                .map(str::to_owned),
            state,
            exit_code: frame
                .get("exit_code")
                .and_then(Json::as_i64)
                .or_else(|| prior.and_then(|task| task.exit_code)),
            can_end: true,
        };
        if prior != Some(&task) {
            self.background_records.insert(identity, task.clone());
            self.background_changes.push(task);
        }
    }

    fn background_set(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let Some(tasks) = frame.get("tasks").and_then(Json::as_array) else {
            return;
        };
        for task in tasks
            .iter()
            .take(orch_agent_contract::background::SNAPSHOT_COMMANDS)
        {
            if task.get("ambient").and_then(Json::as_bool) == Some(true) {
                continue;
            }
            let mut value = task.clone();
            value["type"] = json!("system");
            value["subtype"] = json!("task_started");
            value["is_backgrounded"] = json!(true);
            self.task_started(&Frame::from_value(value), out);
        }
    }

    fn bind_native_task(&mut self, key: NativeTaskKey, id: &str, local_bash: bool) -> bool {
        let Some(tool) = self.tools.get(id) else {
            return false;
        };
        if tool.generation != self.native_generation
            || !matches!(tool.family, Family::Command | Family::Subagent)
        {
            return false;
        }
        let mut resumed = false;
        let mut tool_reused = false;
        let mut local_bash = local_bash;
        if let Some(old) = self.native_tasks.get(&key.task_id) {
            if old.key.run_id.is_some() && key.run_id.is_none() {
                return old.tool_ref == id;
            }
            if let (Some(old_run), Some(run)) = (&old.key.run_id, &key.run_id)
                // Native run ids sort by when the runs opened; their format is otherwise opaque.
                && (run < old_run || (run == old_run && old.tool_ref != id))
            {
                return false;
            }
            let newer = old.key.run_id.is_some() && old.key.run_id != key.run_id;
            resumed = old.resumed || newer;
            tool_reused = old.tool_ref == id && (old.tool_reused || newer);
            local_bash |= old.local_bash && old.tool_ref == id;
        }
        self.native_tasks
            .retain(|_, task| self.tools.contains_key(&task.tool_ref));
        if !self.native_tasks.contains_key(&key.task_id)
            && self.native_tasks.len() == orch_agent_contract::background::SNAPSHOT_COMMANDS
        {
            return false;
        }
        let weight = |task: &NativeTask| {
            std::mem::size_of::<NativeTask>()
                .saturating_add(task.key.task_id.capacity().saturating_mul(2))
                .saturating_add(task.key.run_id.as_ref().map_or(0, String::capacity))
                .saturating_add(task.tool_ref.capacity())
        };
        let task = NativeTask {
            key,
            tool_ref: id.to_owned(),
            local_bash,
            resumed,
            tool_reused,
        };
        let used = self
            .native_tasks
            .values()
            .map(weight)
            .fold(0usize, usize::saturating_add)
            .saturating_sub(self.native_tasks.get(&task.key.task_id).map_or(0, weight));
        if used.saturating_add(weight(&task)) > crate::config::FRAME_MAX_BYTES {
            return false;
        }
        self.native_tasks.insert(task.key.task_id.clone(), task);
        true
    }

    fn task_tool(&self, frame: &Frame, legacy: bool) -> Option<String> {
        let explicit = frame.text("tool_use_id");
        let frame_key = NativeTaskKey::from_value(frame.raw());
        let compatible = |task: &NativeTask| {
            frame_key.as_ref() == Some(&task.key)
                || (!task.resumed
                    && frame_key.as_ref().is_some_and(|key| {
                        key.task_id == task.key.task_id
                            && (key.run_id.is_none() || task.key.run_id.is_none())
                    }))
                || (!task.tool_reused
                    && explicit == Some(task.tool_ref.as_str())
                    && frame_key.as_ref().is_none_or(|key| {
                        key.task_id == task.key.task_id
                            && (key.run_id.is_none() || task.key.run_id.is_none())
                    }))
        };
        let id = if let Some(key) = &frame_key {
            if let Some(task) = self.native_tasks.get(&key.task_id) {
                if !compatible(task) || explicit.is_some_and(|id| id != task.tool_ref) {
                    return None;
                }
                task.tool_ref.as_str()
            } else if legacy {
                explicit?
            } else {
                return None;
            }
        } else if legacy {
            explicit?
        } else {
            return None;
        };
        if legacy
            && self
                .native_tasks
                .values()
                .any(|task| task.tool_ref == id && !compatible(task))
        {
            return None;
        }
        let tool = self.tools.get(id)?;
        (tool.generation == self.native_generation).then(|| id.to_owned())
    }

    fn task_started(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let Some(key) = NativeTaskKey::from_value(frame.raw()) else {
            return;
        };
        let resumed = self.native_tasks.get(&key.task_id).is_some_and(|old| {
            old.key.run_id.is_some() && key.run_id.is_some() && old.key.run_id != key.run_id
        });
        if let Some(old) = self.native_tasks.get(&key.task_id)
            && old.key.run_id.is_some()
            && key.run_id.is_none()
            && (old.tool_reused
                || (old.resumed && frame.text("tool_use_id") != Some(old.tool_ref.as_str())))
        {
            return;
        }
        let Some(id) = frame.text("tool_use_id").map(str::to_owned).or_else(|| {
            self.native_tasks
                .get(&key.task_id)
                .map(|task| task.tool_ref.clone())
        }) else {
            return;
        };
        let Some(tool) = self.tools.get(&id) else {
            return;
        };
        if tool.native_terminal.is_some() && !resumed {
            return;
        }
        match (frame.text("task_type"), &tool.family) {
            (Some("local_bash"), Family::Command)
            | (Some("local_agent"), Family::Subagent)
            // The native kind is optional; an observed call already supplies its kind.
            | (None, Family::Command | Family::Subagent) => {}
            _ => return,
        }
        if !self.bind_native_task(key, &id, frame.text("task_type") == Some("local_bash")) {
            return;
        }
        self.observe_background(frame, &id, Some("running"));
        self.update_native_task(
            &id,
            frame.get("is_backgrounded").and_then(Json::as_bool) == Some(true),
            frame.text("description"),
            resumed.then_some("running"),
            out,
        );
    }

    fn task_progress(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let Some(id) = self.task_tool(frame, false) else {
            return;
        };
        if self
            .tools
            .get(&id)
            .is_some_and(|tool| tool.native_terminal.is_some())
        {
            return;
        }
        self.observe_background(frame, &id, None);
        self.update_native_task(
            &id,
            false,
            frame.text("summary").or_else(|| frame.text("description")),
            None,
            out,
        );
    }

    fn task_updated(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let Some(id) = self.task_tool(frame, false) else {
            return;
        };
        // Only a newer registered run reopens a task; a late same-run patch cannot do so.
        if self
            .tools
            .get(&id)
            .is_some_and(|tool| tool.native_terminal.is_some())
        {
            return;
        }
        let Some(patch) = frame.get("patch") else {
            return;
        };
        self.observe_background(frame, &id, patch.get("status").and_then(Json::as_str));
        self.update_native_task(
            &id,
            patch.get("is_backgrounded").and_then(Json::as_bool) == Some(true),
            patch.get("description").and_then(Json::as_str),
            patch.get("status").and_then(Json::as_str),
            out,
        );
    }

    fn update_native_task(
        &mut self,
        id: &str,
        backgrounded: bool,
        title: Option<&str>,
        status: Option<&str>,
        out: &mut Vec<SessionEvent>,
    ) {
        let Some(state) = self.tools.get_mut(id) else {
            return;
        };
        let mut changed = backgrounded && !state.async_launched;
        state.async_launched |= backgrounded;
        if status == Some("running") {
            changed |= state.native_terminal.take().is_some();
        }
        let terminal = match status {
            Some("completed") => Some(ItemStatus::Completed),
            Some("failed" | "killed") => Some(ItemStatus::Failed),
            _ => None,
        };
        if let Some(terminal) = terminal {
            changed |= state.native_terminal.as_ref() != Some(&terminal);
            state.native_terminal = Some(terminal);
        }
        if let Some(title) = title.filter(|title| !title.is_empty()) {
            let mut end = title
                .len()
                .min(orch_agent_contract::background::COMMAND_PREVIEW_BYTES);
            while !title.is_char_boundary(end) {
                end -= 1;
            }
            let title = title[..end].to_owned();
            changed |= state.progress_title.as_ref() != Some(&title);
            state.progress_title = Some(title);
        }
        if !changed {
            return;
        }
        let state = state.clone();
        let mut payload = self.call_payload(&state, id, None);
        if state.async_launched
            && let ToolPayload::Command {
                background,
                exit_code,
                ..
            } = &mut payload.tool
        {
            *background = true;
            *exit_code = None;
        }
        if let Ok(payload) = serde_json::to_value(payload) {
            out.push(SessionEvent::ItemUpdated(ItemUpdated {
                native_ref: id.to_owned(),
                payload,
            }));
        }
    }

    /// A `rate_limit_event`: where the plan's windows stand. Only what a rejection says is kept for a failure.
    fn rate_limit_event(&mut self, frame: &Frame) {
        if let Some(info) = frame.get("rate_limit_info") {
            self.failure.limit = Some(LimitInfo::read(info));
        }
    }

    /// The assistant message that wraps an API error: its class, its typed kind, its status, its words and,
    /// where it carries them, the parameters of the limit that was reached.
    fn api_error_message(&mut self, frame: &Frame) {
        if let Some(info) = frame
            .get("api_error_params")
            .and_then(|params| params.get("rate_limit_info"))
        {
            self.failure.limit = Some(LimitInfo::read(info));
        }
        let text = frame
            .blocks()
            .iter()
            .filter_map(|block| block.get("text").and_then(Json::as_str))
            .collect::<Vec<_>>()
            .join("");
        self.failure.message = Some(ErrorMessage {
            class: frame.text("error").map(str::to_owned),
            typed: frame.text("api_error").map(str::to_owned),
            status: frame.get("api_error_status").and_then(Json::as_i64),
            text,
        });
    }

    fn phase(&self, kind: TurnPhaseKind, out: &mut Vec<SessionEvent>) {
        if let Some(turn_id) = self.turn {
            out.push(SessionEvent::TurnPhase(TurnPhase {
                turn_id,
                kind,
                at: self.clock.time(),
            }));
        }
    }

    fn error_item(
        &self,
        code: &str,
        message: &str,
        retryable: bool,
        native_ref: &str,
        out: &mut Vec<SessionEvent>,
    ) {
        let native_ref = format!("error:{native_ref}");
        let payload = ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
        };
        out.extend(self.started(
            &native_ref,
            MAIN_STREAM,
            None,
            ItemContent::Error(payload.clone()),
        ));
        if let Ok(payload) = serde_json::to_value(payload) {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref,
                status: ItemStatus::Failed,
                payload,
                output_blob: None,
            }));
        }
    }

    /// A sub-agent that ran in the background is done: its call, which stayed open, is closed.
    fn task_ended(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let Some(id) = self.task_tool(frame, true) else {
            return;
        };
        let Some(state) = self.tools.get(&id) else {
            return;
        };
        if !state.async_launched {
            return;
        }
        if !matches!(
            frame.text("status"),
            Some("completed" | "failed" | "killed" | "stopped")
        ) {
            return;
        }
        let state = state.clone();
        self.observe_background(frame, &id, frame.text("status"));
        let summary = frame.text("summary").unwrap_or_default().to_owned();
        let failed = matches!(frame.text("status"), Some("failed" | "killed"));
        let stopped = frame.text("status") == Some("stopped");
        let outcome = ToolOutcome {
            text: summary,
            is_error: failed || stopped,
            structured: None,
        };
        self.tools.remove(&id);
        self.native_tasks.retain(|_, task| task.tool_ref != id);
        let mut payload = self.call_payload(&state, &id, Some(&outcome));
        if let ToolPayload::Command { exit_code, .. } = &mut payload.tool {
            *exit_code = frame.get("exit_code").and_then(Json::as_i64);
        }
        if stopped && let ToolPayload::Subagent { state, .. } = &mut payload.tool {
            *state = orch_agent_contract::events::SubagentState::Interrupted;
        }
        if let Ok(payload) = serde_json::to_value(&payload) {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref: id.to_owned(),
                status: if stopped {
                    ItemStatus::Cancelled
                } else if failed {
                    ItemStatus::Failed
                } else {
                    ItemStatus::Completed
                },
                payload,
                output_blob: None,
            }));
        }
    }

    // --- the end of a turn -------------------------------------------------------------------------------

    fn result(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        let Some(turn_id) = self.turn.take() else {
            return;
        };
        self.last_turn = Some(turn_id);
        // A text block still open when the turn ends was cut off.
        if self
            .blocks
            .values()
            .any(|block| !block.completed && block.kind != BlockKind::ToolUse)
        {
            self.failure.cut = true;
        }
        let (status, error) = outcome_of(
            frame,
            self.interrupted,
            &self.failure,
            self.clock.now(),
            &chrono::Local,
        );
        self.complete_message(status == TurnOutcome::Completed, out);
        // Whatever was left open is closed: a call the agent made and never saw the end of.
        self.close_open_items(
            if self.interrupted {
                ItemStatus::Cancelled
            } else {
                ItemStatus::Failed
            },
            out,
        );
        // The vendor's limit for the fast mode was reached during the turn: the harness went on at the ordinary speed by
        // itself, and says so here. The turn's use is recorded with the speed it was asked for.
        if frame.text("fast_mode_state") == Some(FAST_MODE_PAUSED) {
            self.error_item(
                "fast_mode_paused",
                "The vendor's limit for the fast mode was reached; the harness went on at the ordinary speed.",
                false,
                &format!("fast-mode:{turn_id}"),
                out,
            );
        }
        let usage = frame.get("usage");
        if let Some(usage) = usage {
            self.turn_tally = TokenUsage::from_parts(parts_of(usage));
        }
        // The usage lists every model the session has used, those before a change of the context too (the name
        // carries the suffix of a larger window): the window in force is the one of the model the process
        // runs, and, where that is not listed by that name, the largest listed.
        let window = frame
            .get("modelUsage")
            .and_then(Json::as_object)
            .and_then(|models| {
                let window_of = |model: &Json| model.get("contextWindow").and_then(Json::as_i64);
                self.running_model
                    .as_deref()
                    .and_then(|name| models.get(name))
                    .and_then(window_of)
                    .or_else(|| models.values().filter_map(window_of).max())
            });
        if window.is_some() {
            self.window = window;
        }
        // `usage` is the turn's main conversation alone; `modelUsage` is the running total of every model the
        // process used, its sub-agents and its shortenings of the conversation included. The turn's whole use is
        // what that total grew by since the result before, where the total it grew from is known.
        let main = self.turn_tally;
        let (whole, models) = match self.model_changes(frame) {
            Some(models) => (
                Some(
                    models
                        .iter()
                        .fold(TokenUsage::default(), |sum, model| sum.plus(model.tokens)),
                ),
                models,
            ),
            None => (None, Vec::new()),
        };
        let counted = self.subagent_total();
        // Sub-agents whose messages said no numbers are what the whole holds beyond the main conversation.
        let subagents = match whole {
            Some(whole) if counted.total_tokens == 0 && !self.sub_tally.is_empty() => {
                less(whole, main)
            }
            _ => counted,
        };
        let turn = match whole {
            Some(whole) if whole.total_tokens >= main.total_tokens => whole,
            _ => main.plus(counted),
        };
        let thread = self.thread_tally.plus(turn);
        let shortened = self.pending_shortening.take();
        let event = UsageUpdated {
            turn_id,
            turn,
            thread,
            context_window: window,
            context_used: self.context_used,
            report: Some(TurnUsageReport {
                api_time_ms: frame.get("duration_api_ms").and_then(Json::as_i64),
                ..self.report(subagents, shortened, models)
            }),
        };
        self.last_usage = Some(event.clone());
        out.push(SessionEvent::UsageUpdated(event));
        self.thread_tally = thread;
        if let Some(error) = &error {
            let row = failure::row_name(frame, &self.failure);
            tracing::debug!(row, code = %error.code, "the turn failed");
            self.last_failure_row = Some(row);
        }
        out.push(SessionEvent::TurnCompleted(TurnCompleted {
            turn_id,
            status,
            error,
        }));
        self.interrupted = false;
        self.compacting = false;
        self.messages.clear();
        self.blocks.clear();
    }

    /// Closes everything still open, with `status`: for the end of a turn and for the death of a process.
    pub fn close_open_items(&mut self, status: ItemStatus, out: &mut Vec<SessionEvent>) {
        self.complete_message(false, out);
        let ids: Vec<String> = self
            .tools
            .iter()
            .filter(|(_, state)| !state.async_launched)
            .map(|(id, _)| id.clone())
            .collect();
        let open: Vec<(String, ToolState)> = ids
            .into_iter()
            .filter_map(|id| self.tools.remove(&id).map(|state| (id, state)))
            .collect();
        for (id, state) in open {
            if !state.item_started {
                continue;
            }
            let outcome = ToolOutcome {
                text: "The call did not finish.".to_owned(),
                is_error: true,
                structured: None,
            };
            let payload = self.call_payload(&state, &id, Some(&outcome));
            if let Ok(payload) = serde_json::to_value(&payload) {
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref: id,
                    status,
                    payload,
                    output_blob: None,
                }));
            }
        }
        let unfinished: Vec<(String, String, bool)> = self
            .blocks
            .iter()
            .filter(|(key, block)| {
                !block.completed
                    && block.kind != BlockKind::ToolUse
                    && !self.pending_reasoning.contains_key(*key)
            })
            .map(|(_, block)| {
                (
                    block.native_ref.clone(),
                    block.text.clone(),
                    block.kind == BlockKind::Thinking,
                )
            })
            .collect();
        for (native_ref, text, thinking) in unfinished {
            let payload = if thinking {
                serde_json::to_value(ReasoningPayload {
                    summary: text.clone(),
                    text_len: chars(&text),
                    hidden: false,
                })
            } else {
                serde_json::to_value(AgentMessagePayload {
                    text: text.clone(),
                    text_len: chars(&text),
                    phase: None,
                })
            };
            if let Ok(payload) = payload {
                out.push(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref,
                    status,
                    payload,
                    output_blob: None,
                }));
            }
        }
        self.blocks.clear();
        self.pending_reasoning.clear();
    }

    /// The host ends the turn itself, because the harness cannot go on and its process is being ended: what
    /// was open is closed as failed, and the turn completes with the error given. Nothing when no turn is under
    /// way.
    pub fn fail_turn(&mut self, code: &str, message: &str, retryable: bool) -> Vec<SessionEvent> {
        let Some(turn_id) = self.turn else {
            return Vec::new();
        };
        for request in self.requests.abort(code, Some(message), None) {
            self.request_reports.push((turn_id, request));
        }
        let mut out = self.abandon_turn();
        out.push(SessionEvent::TurnCompleted(TurnCompleted {
            turn_id,
            status: TurnOutcome::Failed,
            error: Some(TurnError::new(code, message, retryable)),
        }));
        out
    }

    /// The host ends the turn itself with the error it has made (see [`failure::gave_up_trying`]): what was open
    /// is closed as failed. Nothing when no turn is under way.
    pub fn fail_turn_with(&mut self, error: TurnError) -> Vec<SessionEvent> {
        let Some(turn_id) = self.turn else {
            return Vec::new();
        };
        for request in self
            .requests
            .abort(&error.code, Some(&error.message), error.said.as_deref())
        {
            self.request_reports.push((turn_id, request));
        }
        let mut out = self.abandon_turn();
        out.push(SessionEvent::TurnCompleted(TurnCompleted {
            turn_id,
            status: TurnOutcome::Failed,
            error: Some(error),
        }));
        out
    }

    /// What the harness has announced about trying again during this turn.
    #[must_use]
    pub fn retries(&self) -> &failure::Retries {
        &self.failure.retries
    }

    /// Whether the harness waits for something other than the model: a tool that runs.
    #[must_use]
    pub fn waiting_on_a_tool(&self) -> bool {
        self.tools
            .values()
            .any(|tool| tool.item_started && !tool.async_launched)
    }

    /// The turn ended because the process did: no result will come, and what was open is closed.
    pub fn abandon_turn(&mut self) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        self.close_open_items(ItemStatus::Failed, &mut out);
        self.tools.clear();
        self.turn = None;
        self.interrupted = false;
        self.messages.clear();
        self.compacting = false;
        out
    }
}

fn key_tool_id(mapper: &EventMapper, key: &(String, String, u64)) -> Option<String> {
    mapper.tool_of_block(key)
}

fn json_id(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn step_status(status: Option<&str>) -> PlanStepStatus {
    match status {
        Some("in_progress") => PlanStepStatus::InProgress,
        Some("completed") => PlanStepStatus::Completed,
        _ => PlanStepStatus::Pending,
    }
}

/// A diagnostic for a frame; a frame larger than a diagnostic may be is kept as a stub.
fn raw_event(direction: &str, frame: &Json) -> SessionEvent {
    let size = frame.to_string().len();
    let payload = if size > RAW_FRAME_MAX_BYTES {
        json!({
            "truncated": true,
            "bytes": size,
            "type": frame.get("type"),
            "subtype": frame.get("subtype"),
        })
    } else {
        frame.clone()
    };
    SessionEvent::RawFrame(RawFrame {
        direction: direction.to_owned(),
        payload,
    })
}

// ---------------------------------------------------------------------------------------------------------
// Usage

fn absorb_usage(message: &mut Message, usage: &Json) {
    let number = |member: &str| usage.get(member).and_then(Json::as_i64);
    if let Some(value) = number("input_tokens") {
        message.input_tokens = value;
    }
    if let Some(value) = number("cache_read_input_tokens") {
        message.cache_read = value;
    }
    if let Some(value) = number("cache_creation_input_tokens") {
        message.cache_write = value;
    }
    if let Some(value) = number("output_tokens") {
        message.output_tokens = value;
    }
    if let Some(value) = usage
        .pointer("/output_tokens_details/thinking_tokens")
        .and_then(Json::as_i64)
    {
        message.thinking_tokens = value;
    }
}

fn message_usage(message: &Message) -> TokensReportedApart {
    let reasoning = message.thinking_tokens.min(message.output_tokens).max(0);
    TokensReportedApart {
        input: message.input_tokens,
        cache_read: message.cache_read,
        cache_write: message.cache_write,
        output: message.output_tokens - reasoning,
        reasoning,
    }
}

fn parts_of(usage: &Json) -> TokensReportedApart {
    let mut message = Message::default();
    absorb_usage(&mut message, usage);
    message_usage(&message)
}

/// How far below its window the harness shortens a conversation, until a breakdown of its context says where:
/// the room it keeps for an answer (20 000) and its own reserve (13 000). It shortens a context of 1 000 000
/// at about 967 000.
const COMPACT_MARGIN: i64 = 33_000;

/// One model's running total as `modelUsage` gives it: the input without the parts read from or written to
/// the cache, as this harness counts, and the thinking inside the output.
fn model_tokens(usage: &Json) -> TokenUsage {
    let number = |member: &str| usage.get(member).and_then(Json::as_i64).unwrap_or(0).max(0);
    let output = number("outputTokens");
    let reasoning = number("thinkingTokens").min(output);
    TokenUsage::from_parts(TokensReportedApart {
        input: number("inputTokens"),
        cache_read: number("cacheReadInputTokens"),
        cache_write: number("cacheCreationInputTokens"),
        output: output - reasoning,
        reasoning,
    })
}

/// What `a` holds beyond `b`, part by part and never below nothing, its parts kept inside its whole.
fn less(a: TokenUsage, b: TokenUsage) -> TokenUsage {
    let part = |x: i64, y: i64| (x - y).max(0);
    let input = part(a.input_tokens, b.input_tokens);
    let cached = part(a.cached_input_tokens, b.cached_input_tokens).min(input);
    let written = part(a.cache_write_tokens, b.cache_write_tokens).min(input - cached);
    let output = part(a.output_tokens, b.output_tokens);
    let reasoning = part(a.reasoning_tokens, b.reasoning_tokens).min(output);
    TokenUsage {
        input_tokens: input,
        cached_input_tokens: cached,
        cache_write_tokens: written,
        output_tokens: output,
        reasoning_tokens: reasoning,
        total_tokens: input + output,
    }
}

impl EventMapper {
    /// What each model used since the result before, from the running totals of `modelUsage`; `None` where the
    /// totals they grew from are not known, or the harness gave none (a process that crashed gives zeros).
    fn model_changes(&mut self, frame: &Frame) -> Option<Vec<ModelTokens>> {
        let now: BTreeMap<String, TokenUsage> = frame
            .get("modelUsage")
            .and_then(Json::as_object)
            .map(|models| {
                models
                    .iter()
                    .map(|(name, usage)| (name.clone(), model_tokens(usage)))
                    .collect()
            })
            .unwrap_or_default();
        if now.values().all(|tokens| tokens.total_tokens == 0) {
            // What the next result grows from is not known.
            self.model_base = None;
            return None;
        }
        let base = self.model_base.replace(now.clone())?;
        Some(
            now.iter()
                .filter_map(|(model, total)| {
                    let before = base.get(model).copied().unwrap_or_default();
                    // A total below the one before began again from nothing: the conversation was cleared.
                    let grew = if before.total_tokens > total.total_tokens {
                        *total
                    } else {
                        less(*total, before)
                    };
                    (grew.total_tokens > 0).then(|| ModelTokens {
                        model: model.clone(),
                        tokens: grew,
                    })
                })
                .collect(),
        )
    }
}

// ---------------------------------------------------------------------------------------------------------
// How a turn ended

/// How a turn ended and, for a failure, why, from the message that closes it and what the stream said before it
/// (see [`failure`]): the harness's own fields decide the code, its words are kept, and the moment a limit ends
/// is read from them, or from its sentence in the zone that names or else `runtime`'s.
#[must_use]
pub fn outcome_of<Z: chrono::TimeZone>(
    frame: &Frame,
    interrupted: bool,
    notes: &Notes,
    now: chrono::DateTime<chrono::Utc>,
    runtime: &Z,
) -> (TurnOutcome, Option<TurnError>) {
    let is_error = frame
        .get("is_error")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let reason = frame.text("terminal_reason").unwrap_or_default();
    let subtype = frame.subtype().unwrap_or_default();
    if reason.starts_with("aborted") || (interrupted && (is_error || notes.refusal.is_some())) {
        return (TurnOutcome::Interrupted, None);
    }
    if !is_error && subtype == "success" && notes.refusal.is_none() {
        return (TurnOutcome::Completed, None);
    }
    (
        TurnOutcome::Failed,
        Some(failure::classify(frame, notes, now, runtime)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::events::{ToolPayload, sub_stream};
    use orch_types::{ManualClock, new_id};

    #[test]
    fn a_documented_refusal_stop_fails_even_when_the_harness_result_says_success() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"refusal","stop_reason":"refusal","stop_details":{"category":"cyber"},"content":[{"type":"text","text":"Request declined"}]}}),
                json!({"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed"}),
            ],
        );
        let failure = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::TurnCompleted(end) => end.error.as_ref(),
                _ => None,
            })
            .unwrap();
        assert_eq!(failure.code, "model_refused");
        assert_eq!(failure.reason.as_deref(), Some("cyber"));
        assert_eq!(failure.said.as_deref(), Some("Request declined"));
    }

    #[test]
    fn a_native_success_after_a_refusal_is_not_automatically_repeated() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"refusal","stop_reason":"refusal","content":[{"type":"text","text":"Request declined"}]}}),
                json!({"type":"assistant","message":{"id":"answer","stop_reason":"end_turn","content":[{"type":"text","text":"The harness recovered"}]}}),
                json!({"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed"}),
            ],
        );
        let ended = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::TurnCompleted(end) => Some(end),
                _ => None,
            })
            .unwrap();
        assert_eq!(ended.status, TurnOutcome::Completed);
        assert!(ended.error.is_none());
    }

    fn mapper(raw: bool) -> (EventMapper, TurnId) {
        let clock = ManualClock::new("2026-10-11T10:00:00Z".parse().unwrap());
        let mut mapper = EventMapper::new(
            MapperSettings {
                directory: "/workspace/web".into(),
                cwd: "/workspace/web".into(),
                raw_events: raw,
            },
            Arc::new(clock),
        );
        let turn = new_id();
        mapper.begin_turn(turn);
        (mapper, turn)
    }

    fn frame(value: Json) -> Frame {
        Frame::from_value(value)
    }

    fn feed(mapper: &mut EventMapper, frames: Vec<Json>) -> Vec<SessionEvent> {
        frames
            .into_iter()
            .flat_map(|value| mapper.on_frame(&frame(value)))
            .collect()
    }

    fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
        events.iter().map(SessionEvent::kind).collect()
    }

    fn text_stream(message: &str, index: u64, pieces: &[&str]) -> Vec<Json> {
        let mut frames = vec![
            json!({ "type": "stream_event", "api_message_id": message, "parent_tool_use_id": null,
            "event": { "type": "content_block_start", "index": index, "content_block": { "type": "text", "text": "" } } }),
        ];
        for piece in pieces {
            frames.push(json!({ "type": "stream_event", "api_message_id": message, "parent_tool_use_id": null,
                "event": { "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": piece } } }));
        }
        frames.push(json!({ "type": "assistant", "parent_tool_use_id": null,
            "message": { "id": message, "content": [{ "type": "text", "text": pieces.concat() }] } }));
        frames.push(
            json!({ "type": "stream_event", "api_message_id": message, "parent_tool_use_id": null,
            "event": { "type": "content_block_stop", "index": index } }),
        );
        frames
    }

    #[test]
    fn thinking_that_ends_empty_never_becomes_a_line_even_when_the_turn_is_interrupted() {
        for end in ["whole", "interrupted"] {
            let (mut mapper, _) = mapper(false);
            let started = feed(
                &mut mapper,
                vec![json!({ "type": "stream_event", "api_message_id": "empty",
                "event": { "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } } })],
            );
            assert!(started.is_empty());
            if end == "whole" {
                let completed = feed(
                    &mut mapper,
                    vec![json!({ "type": "assistant", "message": {
                    "id": "empty", "content": [{ "type": "thinking", "thinking": "" }] } })],
                );
                assert!(completed.is_empty());
                let redacted = feed(
                    &mut mapper,
                    vec![json!({ "type": "assistant", "message": {
                    "id": "redacted", "content": [{ "type": "redacted_thinking", "data": "hidden" }] } })],
                );
                assert!(redacted.is_empty());
            } else {
                mapper.note_interrupt();
            }
            let ended = feed(&mut mapper, vec![result_frame(json!({}))]);
            assert!(!ended.iter().any(|event| matches!(
                event,
                SessionEvent::ItemStarted(_) | SessionEvent::ItemCompleted(_)
            )));
        }
    }

    #[test]
    fn readable_thinking_starts_before_its_first_delta_and_completes_whole() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "stream_event", "api_message_id": "thought", "event": {
                "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } } }),
                json!({ "type": "stream_event", "api_message_id": "thought", "event": {
                "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "A thought." } } }),
                json!({ "type": "assistant", "message": { "id": "thought",
                "content": [{ "type": "thinking", "thinking": "A thought." }] } }),
            ],
        );
        assert_eq!(
            kinds(&events),
            ["item.started", "item.delta", "item.completed"]
        );
        let SessionEvent::ItemDelta(delta) = &events[1] else {
            panic!("a delta")
        };
        assert_eq!(delta.offset, 0);
        let SessionEvent::ItemCompleted(done) = &events[2] else {
            panic!("completed")
        };
        assert_eq!(done.payload["summary"], "A thought.");
    }

    #[test]
    fn what_the_agent_says_arrives_as_pieces_and_then_whole() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            text_stream("msg_1", 0, &["Hel", "lo ", "wörld"]),
        );
        assert_eq!(
            kinds(&events),
            [
                "item.started",
                "item.delta",
                "item.delta",
                "item.delta",
                "item.updated"
            ]
        );
        let SessionEvent::ItemDelta(second) = &events[2] else {
            panic!("a delta");
        };
        assert_eq!(
            (second.offset, second.len, second.data.as_str()),
            (3, 3, "lo ")
        );
        let SessionEvent::ItemDelta(third) = &events[3] else {
            panic!("a delta");
        };
        assert_eq!(third.offset, 6, "offsets count code points");
        let SessionEvent::ItemUpdated(done) = &events[4] else {
            panic!("the whole text is already shown");
        };
        assert_eq!(done.payload["text"], "Hello wörld");
        assert_eq!(done.payload["text_len"], 11);
        assert_eq!(done.native_ref, "msg_1:0");
        let ended = feed(&mut mapper, vec![result_frame(json!({}))]);
        let SessionEvent::ItemCompleted(done) = &ended[0] else {
            panic!("completed")
        };
        assert_eq!(done.payload["phase"], "final");
    }

    #[test]
    fn the_last_text_of_a_message_that_ends_the_turn_is_the_answer_and_the_rest_is_said_on_the_way()
    {
        let (mut mapper, _) = mapper(false);
        let mut frames = text_stream("msg_1", 0, &["Let me look."]);
        frames.extend(text_stream("msg_1", 1, &["Done."]));
        frames.push(json!({ "type": "stream_event", "api_message_id": "msg_1", "parent_tool_use_id": null,
            "event": { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 9 } } }));
        frames.push(result_frame(json!({})));
        let events = feed(&mut mapper, frames);
        let phases: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemCompleted(done) if done.payload.get("phase").is_some() => Some((
                    done.native_ref.as_str(),
                    done.payload["phase"].as_str().unwrap(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(phases, [("msg_1:0", "commentary"), ("msg_1:1", "final")]);
    }

    #[test]
    fn whole_messages_without_native_phases_mark_only_the_last_answer_of_a_natural_turn_final() {
        for ending in ["completed", "interrupted", "failed"] {
            let (mut mapper, _) = mapper(false);
            let mut events = feed(
                &mut mapper,
                vec![
                    json!({ "type": "assistant", "message": { "id": "first", "content": [{ "type": "text", "text": "Working." }] } }),
                    json!({ "type": "assistant", "message": { "id": "answer", "content": [{ "type": "text", "text": "Done." }] } }),
                ],
            );
            if ending == "interrupted" {
                mapper.note_interrupt();
            }
            events.extend(feed(&mut mapper, vec![result_frame(match ending {
                "failed" => json!({ "subtype": "error_during_execution", "is_error": true, "errors": ["Something failed"] }),
                "interrupted" => json!({ "subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted" }),
                _ => json!({}),
            })]));
            let phases: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemCompleted(done) if done.payload.get("phase").is_some() => {
                        Some((
                            done.native_ref.as_str(),
                            done.payload["phase"].as_str().unwrap(),
                        ))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                phases,
                [
                    ("first:0", "commentary"),
                    (
                        "answer:0",
                        if ending == "completed" {
                            "final"
                        } else {
                            "commentary"
                        }
                    )
                ]
            );
        }
    }

    #[test]
    fn a_tool_line_gets_its_command_or_path_when_the_assistant_supplies_the_whole_call() {
        for (name, input, member, expected) in [
            (
                "Bash",
                json!({ "command": "sleep 5" }),
                "command",
                "sleep 5",
            ),
            (
                "Read",
                json!({ "file_path": "/workspace/web/src/main.rs" }),
                "path",
                "src/main.rs",
            ),
        ] {
            let (mut mapper, _) = mapper(false);
            let started = feed(
                &mut mapper,
                vec![
                    json!({ "type": "stream_event", "api_message_id": "tool", "event": {
                "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "call", "name": name, "input": {} } } }),
                ],
            );
            assert_eq!(kinds(&started), ["item.started"]);
            assert!(feed(&mut mapper, vec![json!({ "type": "stream_event", "api_message_id": "tool", "event": {
                "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": input.to_string() } } })]).is_empty());
            let whole = feed(
                &mut mapper,
                vec![json!({ "type": "assistant", "message": { "id": "tool",
                "content": [{ "type": "tool_use", "id": "call", "name": name, "input": input }] } })],
            );
            assert_eq!(kinds(&whole), ["item.updated"], "the tool is still running");
            let SessionEvent::ItemUpdated(update) = &whole[0] else {
                panic!("updated")
            };
            assert_eq!(update.payload["tool"][member], expected);
            assert_eq!(
                serde_json::from_str::<Json>(
                    update.payload["raw"]["input"]["inline"].as_str().unwrap()
                )
                .unwrap(),
                input
            );
            let result = feed(
                &mut mapper,
                vec![json!({ "type": "user", "message": { "content": [
                { "type": "tool_result", "tool_use_id": "call", "content": "finished", "is_error": false } ] } })],
            );
            assert_eq!(kinds(&result), ["item.completed"]);
        }
    }

    #[test]
    fn a_sub_agent_speaks_in_a_stream_of_its_own_and_arrives_whole() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "assistant", "parent_tool_use_id": "toolu_agent",
                "message": { "id": "msg_sub", "content": [{ "type": "text", "text": "Sub-agent speaking." }] } }),
            ],
        );
        assert_eq!(kinds(&events), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(started) = &events[0] else {
            panic!("started");
        };
        assert_eq!(started.stream, sub_stream("toolu_agent"));
        assert_eq!(started.parent_native_ref.as_deref(), Some("toolu_agent"));
    }

    #[test]
    fn a_command_is_started_with_what_is_known_brought_up_to_date_and_completed_with_its_output() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "stream_event", "api_message_id": "m", "parent_tool_use_id": null,
                    "event": { "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {} } } }),
                json!({ "type": "stream_event", "api_message_id": "m", "parent_tool_use_id": null,
                    "event": { "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"command\":\"ls\"}" } } }),
                json!({ "type": "assistant", "parent_tool_use_id": null,
                    "message": { "id": "m", "content": [{ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": { "command": "ls", "description": "List" } }] } }),
                json!({ "type": "user", "parent_tool_use_id": null,
                    "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "toolu_1", "content": "a\nb", "is_error": false }] },
                    "tool_use_result": { "stdout": "a\nb", "stderr": "", "interrupted": false } }),
            ],
        );
        assert_eq!(
            kinds(&events),
            ["item.started", "item.updated", "item.completed"]
        );
        let SessionEvent::ItemCompleted(done) = &events[2] else {
            panic!("completed");
        };
        assert_eq!(done.status, ItemStatus::Completed);
        assert_eq!(done.payload["tool"]["kind"], "command");
        assert_eq!(done.payload["tool"]["command"], "ls");
        assert_eq!(done.payload["tool"]["exit_code"], 0);
        assert_eq!(done.payload["tool"]["output"]["inline"], "a\nb");
        assert_eq!(done.payload["title"], "List");
        assert_eq!(done.payload["raw"]["name"], "Bash");
        assert_eq!(
            done.payload["raw"]["input"]["inline"],
            r#"{"command":"ls","description":"List"}"#
        );
    }

    #[test]
    fn a_command_that_failed_is_a_failed_step_with_its_exit_status() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "assistant", "parent_tool_use_id": null,
                    "message": { "id": "m", "content": [{ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": { "command": "exit 3" } }] } }),
                json!({ "type": "user", "parent_tool_use_id": null,
                    "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "toolu_1", "content": "Exit code 3\nboom", "is_error": true }] },
                    "tool_use_result": "Error: Exit code 3\nboom" }),
            ],
        );
        let SessionEvent::ItemCompleted(done) = events.last().unwrap() else {
            panic!("completed");
        };
        assert_eq!(done.status, ItemStatus::Failed);
        assert_eq!(done.payload["tool"]["exit_code"], 3);
        assert_eq!(done.payload["tool"]["output"]["inline"], "boom");
    }

    #[test]
    fn the_harness_waits_for_a_tool_while_a_call_has_no_result_and_for_the_model_otherwise() {
        let (mut mapper, _) = mapper(false);
        assert!(!mapper.waiting_on_a_tool());
        feed(
            &mut mapper,
            vec![json!({ "type": "assistant", "parent_tool_use_id": null,
                "message": { "id": "m", "content": [{ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": { "command": "sleep 100" } }] } })],
        );
        assert!(
            mapper.waiting_on_a_tool(),
            "a command runs and says nothing for as long as it takes"
        );
        feed(
            &mut mapper,
            vec![json!({ "type": "user", "parent_tool_use_id": null,
                "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "toolu_1", "content": "done", "is_error": false }] },
                "tool_use_result": { "stdout": "done", "stderr": "", "interrupted": false } })],
        );
        assert!(!mapper.waiting_on_a_tool());
    }

    #[test]
    fn a_call_that_never_saw_its_result_is_closed_with_the_turn() {
        let (mut mapper, turn) = mapper(false);
        feed(
            &mut mapper,
            vec![json!({ "type": "assistant", "parent_tool_use_id": null,
                "message": { "id": "m", "content": [{ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": { "command": "sleep 100" } }] } })],
        );
        mapper.note_interrupt();
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "result", "subtype": "error_during_execution", "is_error": true,
                "terminal_reason": "aborted_tools", "usage": { "input_tokens": 10, "output_tokens": 2 } }),
            ],
        );
        assert_eq!(
            kinds(&events),
            ["item.completed", "usage.updated", "turn.completed"]
        );
        let SessionEvent::ItemCompleted(closed) = &events[0] else {
            panic!("completed");
        };
        assert_eq!(closed.status, ItemStatus::Cancelled);
        let SessionEvent::TurnCompleted(end) = &events[2] else {
            panic!("the turn ended");
        };
        assert_eq!((end.turn_id, end.status), (turn, TurnOutcome::Interrupted));
    }

    #[test]
    fn what_a_person_said_is_never_reported_and_its_echo_tells_the_input_was_taken_in() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "user", "isReplay": true, "uuid": "u-1", "parent_tool_use_id": null,
                    "message": { "role": "user", "content": [{ "type": "text", "text": "do it" }] } }),
                json!({ "type": "user", "parent_tool_use_id": null,
                    "message": { "role": "user", "content": [{ "type": "text", "text": "[Request interrupted by user]" }] } }),
            ],
        );
        assert!(events.is_empty());
        assert_eq!(mapper.take_echoed(), vec!["u-1".to_owned()]);
        assert!(mapper.take_echoed().is_empty());
    }

    fn result_frame(extra: Json) -> Json {
        let mut base = json!({ "type": "result", "subtype": "success", "is_error": false, "terminal_reason": "completed",
            "usage": { "input_tokens": 100, "cache_read_input_tokens": 40, "cache_creation_input_tokens": 10,
                       "output_tokens": 25, "output_tokens_details": { "thinking_tokens": 5 } },
            "modelUsage": { "m": { "contextWindow": 200000 } } });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    }

    #[test]
    fn a_turn_ends_with_its_use_counted_as_the_contract_counts_it() {
        let (mut mapper, turn) = mapper(false);
        let events = feed(&mut mapper, vec![result_frame(json!({}))]);
        let SessionEvent::UsageUpdated(usage) = &events[0] else {
            panic!("usage");
        };
        assert_eq!(usage.turn_id, turn);
        assert_eq!(
            usage.turn.input_tokens, 150,
            "cached and written tokens are parts of the input"
        );
        assert_eq!(usage.turn.cached_input_tokens, 40);
        assert_eq!(usage.turn.output_tokens, 25);
        assert_eq!(usage.turn.reasoning_tokens, 5);
        assert_eq!(usage.turn.total_tokens, 175);
        assert!(usage.turn.is_consistent());
        assert_eq!(usage.context_window, Some(200_000));
        let SessionEvent::TurnCompleted(end) = &events[1] else {
            panic!("the turn ended");
        };
        assert_eq!(end.status, TurnOutcome::Completed);
        assert!(!mapper.turn_active());
    }

    #[test]
    fn a_turn_s_use_is_the_whole_tree_with_its_sub_agents_share_apart() {
        let (mut mapper, turn) = mapper(false);
        mapper.process_started(true);
        let sub = |event: Json| json!({ "type": "stream_event", "api_message_id": "s1", "parent_tool_use_id": "toolu_agent", "event": event });
        let main = |event: Json| json!({ "type": "stream_event", "api_message_id": "m1", "parent_tool_use_id": null, "event": event });
        let mut frames = vec![
            main(
                json!({ "type": "message_start", "message": { "id": "m1", "usage": { "input_tokens": 10, "cache_read_input_tokens": 1_000, "cache_creation_input_tokens": 200, "output_tokens": 1 } } }),
            ),
            main(
                json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 50 } }),
            ),
            main(json!({ "type": "message_stop" })),
            sub(
                json!({ "type": "message_start", "message": { "id": "s1", "usage": { "input_tokens": 300, "output_tokens": 1 } } }),
            ),
            sub(
                json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 40 } }),
            ),
            sub(json!({ "type": "message_stop" })),
        ];
        // The running totals of the process: the main conversation's request and the sub-agent's, on two models.
        frames.push(json!({ "type": "result", "subtype": "success", "is_error": false, "terminal_reason": "completed",
            "usage": { "input_tokens": 10, "cache_read_input_tokens": 1_000, "cache_creation_input_tokens": 200, "output_tokens": 50 },
            "modelUsage": {
                "claude-opus-5": { "inputTokens": 10, "outputTokens": 50, "cacheReadInputTokens": 1_000, "cacheCreationInputTokens": 200, "contextWindow": 200_000 },
                "claude-haiku-5": { "inputTokens": 300, "outputTokens": 40, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "contextWindow": 200_000 }
            } }));
        let events = feed(&mut mapper, frames);
        let last = events
            .iter()
            .rev()
            .find_map(|event| match event {
                SessionEvent::UsageUpdated(usage) => Some(usage),
                _ => None,
            })
            .expect("the turn's use");
        assert_eq!(last.turn_id, turn);
        assert_eq!(
            last.turn.total_tokens,
            1_260 + 340,
            "the main conversation and the sub-agent together"
        );
        assert!(last.turn.is_consistent());
        assert_eq!(
            last.context_used,
            Some(1_210),
            "the prompt: its input with the parts read from and written to the cache"
        );
        let report = last.report.as_ref().expect("a report");
        assert_eq!(report.subagents.map(|share| share.total_tokens), Some(340));
        assert_eq!(report.by_subagent[0].stream, "sub:toolu_agent");
        assert_eq!(report.models.len(), 2);
        assert_eq!(report.compact_at, Some(200_000 - COMPACT_MARGIN));
        // The next result grows from these totals: only what was used since is the next turn's.
        mapper.begin_turn(turn);
        let next = feed(
            &mut mapper,
            vec![
                json!({ "type": "result", "subtype": "success", "is_error": false, "terminal_reason": "completed",
            "usage": { "input_tokens": 5, "output_tokens": 5 },
            "modelUsage": {
                "claude-opus-5": { "inputTokens": 15, "outputTokens": 55, "cacheReadInputTokens": 1_000, "cacheCreationInputTokens": 200, "contextWindow": 200_000 },
                "claude-haiku-5": { "inputTokens": 300, "outputTokens": 40, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "contextWindow": 200_000 }
            } }),
            ],
        );
        let SessionEvent::UsageUpdated(next) = &next[0] else {
            panic!("usage");
        };
        assert_eq!(next.turn.total_tokens, 10);
        // A breakdown that comes after is carried with that use, for the turn it was taken after.
        let answer = json!({ "categories": [{ "name": "Messages", "tokens": 1_000, "color": "x", "kind": "used" }],
            "totalTokens": 1_000, "maxTokens": 200_000, "isAutoCompactEnabled": true, "autoCompactThreshold": 160_000,
            "mcpTools": [], "memoryFiles": [], "agents": [],
            "messageBreakdown": { "toolCallTokens": 0, "toolResultTokens": 0, "attachmentTokens": 0, "assistantMessageTokens": 600,
                "userMessageTokens": 400, "redirectedContextTokens": 0, "unattributedTokens": 0, "toolCallsByType": [], "attachmentsByType": [] } });
        let (fill, event) = mapper.context_answered(&answer, ContextCount::Estimated);
        assert_eq!(fill.map(|fill| fill.used), Some(1_000));
        let Some(SessionEvent::UsageUpdated(carried)) = event else {
            panic!("the breakdown is carried with the last use");
        };
        assert_eq!(
            carried.turn.total_tokens, 10,
            "the totals are the ones reported, unchanged"
        );
        let report = carried.report.expect("a report");
        assert_eq!(report.compact_at, Some(160_000));
        assert!(report.context.is_some());
    }

    #[test]
    fn the_window_in_force_is_the_one_of_the_model_the_process_runs_and_not_the_largest_the_session_ever_had()
     {
        let both = json!({ "modelUsage": {
            "m": { "contextWindow": 200_000 }, "m[1m]": { "contextWindow": 1_000_000 }
        } });
        let init = |model: &str| json!({ "type": "system", "subtype": "init", "uuid": "i", "model": model });
        let window_after = |first: Json| {
            let (mut mapper, _) = mapper_quiet();
            let events = feed(&mut mapper, vec![first, result_frame(both.clone())]);
            let Some(SessionEvent::UsageUpdated(updated)) = events
                .iter()
                .find(|event| matches!(event, SessionEvent::UsageUpdated(_)))
            else {
                panic!("usage");
            };
            updated.context_window
        };
        // A process started with the larger window, and one started with the smaller after the session had both.
        assert_eq!(window_after(init("m[1m]")), Some(1_000_000));
        assert_eq!(window_after(init("m")), Some(200_000));
        // A name the usage does not list, or none stated: the largest listed.
        assert_eq!(window_after(init("another")), Some(1_000_000));
        assert_eq!(
            window_after(json!({ "type": "system", "subtype": "init", "uuid": "i" })),
            Some(1_000_000)
        );
    }

    #[test]
    // Constructed-frame evidence only; a real plan's fast-mode cooldown is recorded separately.
    fn a_turn_the_fast_mode_was_paused_in_says_so_before_its_use_and_one_that_was_not_says_nothing()
    {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![result_frame(json!({ "fast_mode_state": "cooldown" }))],
        );
        assert_eq!(
            kinds(&events),
            [
                "item.started",
                "item.completed",
                "usage.updated",
                "turn.completed"
            ]
        );
        let SessionEvent::ItemStarted(notice) = &events[0] else {
            panic!("a notice");
        };
        let ItemContent::Error(error) = &notice.content else {
            panic!("an error item");
        };
        assert_eq!(error.code, "fast_mode_paused");
        assert!(!error.retryable && error.message.contains("ordinary speed"));
        for state in ["on", "off"] {
            let (mut mapper, _) = mapper_quiet();
            let events = feed(
                &mut mapper,
                vec![result_frame(json!({ "fast_mode_state": state }))],
            );
            assert_eq!(
                kinds(&events),
                ["usage.updated", "turn.completed"],
                "{state}"
            );
        }
    }

    fn failure(status: Option<i64>, reason: &str, text: &str) -> (TurnOutcome, Option<TurnError>) {
        outcome_of(
            &frame(
                json!({ "type": "result", "subtype": "success", "is_error": true, "terminal_reason": reason,
                "api_error_status": status, "result": text }),
            ),
            false,
            &Notes::default(),
            "2026-10-12T10:00:00Z".parse().unwrap(),
            &chrono::Utc,
        )
    }

    #[test]
    fn a_failure_is_given_the_code_that_says_what_the_people_can_do_about_it() {
        let code = |outcome: (TurnOutcome, Option<TurnError>)| {
            let error = outcome.1.unwrap();
            assert_eq!(outcome.0, TurnOutcome::Failed);
            (error.code, error.retryable)
        };
        assert_eq!(
            code(failure(Some(401), "api_error", "Not logged in")),
            ("provider_unauthorized".into(), false)
        );
        assert_eq!(
            code(failure(Some(402), "api_error", "Credit balance is too low")),
            ("provider_exhausted".into(), false)
        );
        assert_eq!(
            code(failure(
                Some(404),
                "api_error",
                "There's an issue with the selected model (x)"
            )),
            ("model_unavailable".into(), false)
        );
        assert_eq!(
            code(failure(Some(400), "prompt_too_long", "Prompt is too long")),
            ("context_overflow".into(), false)
        );
        assert_eq!(
            code(failure(
                Some(429),
                "api_error",
                "You've hit your limit · resets 3pm"
            )),
            ("provider_exhausted".into(), false)
        );
        assert_eq!(
            code(failure(Some(429), "api_error", "Request rejected (429)")),
            ("provider_busy".into(), true)
        );
        assert_eq!(
            code(failure(Some(500), "api_error", "Internal server error")),
            ("provider_busy".into(), true)
        );
        assert_eq!(
            code(failure(None, "api_error", "Connection lost (ECONNRESET)")),
            ("provider_unreachable".into(), true)
        );
        assert_eq!(
            code(failure(Some(400), "api_error", "Invalid request")),
            ("harness_error".into(), false)
        );
    }

    #[test]
    fn a_turn_that_was_stopped_is_interrupted_whichever_way_the_harness_says_it() {
        let by_reason = outcome_of(
            &frame(
                json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted_streaming" }),
            ),
            false,
            &Notes::default(),
            "2026-10-12T10:00:00Z".parse().unwrap(),
            &chrono::Utc,
        );
        assert_eq!(by_reason, (TurnOutcome::Interrupted, None));
        let by_request = outcome_of(
            &frame(
                json!({ "type": "result", "subtype": "error_during_execution", "is_error": true }),
            ),
            true,
            &Notes::default(),
            "2026-10-12T10:00:00Z".parse().unwrap(),
            &chrono::Utc,
        );
        assert_eq!(by_request.0, TurnOutcome::Interrupted);
    }

    #[test]
    fn the_failure_of_a_model_call_is_not_something_the_agent_said() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "assistant", "is_api_error_message": true, "error": "invalid_request",
                "message": { "id": "x", "model": "<synthetic>", "content": [{ "type": "text", "text": "API Error: 400" }] } }),
            ],
        );
        assert!(events.is_empty());
    }

    #[test]
    fn a_compaction_that_is_announced_is_a_phase_and_one_that_is_not_is_a_marker() {
        let (mut mapper, turn) = mapper(false);
        let announced = feed(
            &mut mapper,
            vec![
                json!({ "type": "system", "subtype": "status", "status": "compacting" }),
                json!({ "type": "system", "subtype": "status", "status": null, "compact_result": "success" }),
                json!({ "type": "system", "subtype": "compact_boundary", "uuid": "b1", "compact_metadata": { "duration_ms": 55 } }),
            ],
        );
        assert_eq!(kinds(&announced), ["turn.phase", "turn.phase"]);
        let SessionEvent::TurnPhase(first) = &announced[0] else {
            panic!("a phase");
        };
        assert_eq!(
            (first.turn_id, first.kind),
            (turn, TurnPhaseKind::Compacting)
        );
        let silent = feed(
            &mut mapper,
            vec![
                json!({ "type": "system", "subtype": "compact_boundary", "uuid": "b2", "compact_metadata": { "duration_ms": 70 } }),
            ],
        );
        assert_eq!(kinds(&silent), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(marker) = &silent[0] else {
            panic!("a marker");
        };
        assert_eq!(
            marker.content,
            ItemContent::Marker(MarkerPayload::Compacted {
                duration_ms: Some(70)
            })
        );
    }

    #[test]
    fn the_things_to_do_are_one_item_for_the_turn_that_is_updated_in_place() {
        let (mut mapper, turn) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "assistant", "parent_tool_use_id": null, "message": { "id": "m", "content": [
                    { "type": "tool_use", "id": "t1", "name": "TaskCreate", "input": { "subject": "Read the code", "description": "d" } }] } }),
                json!({ "type": "user", "parent_tool_use_id": null,
                    "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "created" }] },
                    "tool_use_result": { "task": { "id": "1", "subject": "Read the code" } } }),
                json!({ "type": "assistant", "parent_tool_use_id": null, "message": { "id": "m2", "content": [
                    { "type": "tool_use", "id": "t2", "name": "TaskUpdate", "input": { "taskId": "1", "status": "completed" } }] } }),
                json!({ "type": "user", "parent_tool_use_id": null,
                    "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t2", "content": "ok" }] },
                    "tool_use_result": { "success": true, "taskId": "1", "updatedFields": ["status"] } }),
            ],
        );
        assert_eq!(kinds(&events), ["item.started", "item.updated"]);
        let SessionEvent::ItemStarted(started) = &events[0] else {
            panic!("started");
        };
        assert_eq!(started.native_ref, format!("plan:{turn}"));
        let ItemContent::Plan(plan) = &started.content else {
            panic!("a plan");
        };
        assert_eq!(
            plan.steps,
            vec![PlanStep {
                text: "Read the code".into(),
                status: PlanStepStatus::Pending
            }]
        );
        let SessionEvent::ItemUpdated(update) = &events[1] else {
            panic!("updated");
        };
        assert_eq!(update.payload["steps"][0]["status"], "completed");
    }

    #[test]
    fn a_tool_of_orch_and_a_tool_of_another_server_are_told_apart() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "assistant", "parent_tool_use_id": null, "message": { "id": "m", "content": [
                    { "type": "tool_use", "id": "o1", "name": "mcp__orch__list_shared", "input": { "depth": 2 } },
                    { "type": "tool_use", "id": "g1", "name": "mcp__github__create_issue", "input": { "title": "x" } }] } }),
            ],
        );
        let tools: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemStarted(started) => match &started.content {
                    ItemContent::ToolCall(call) => Some(call.tool.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert!(matches!(tools[0], ToolPayload::Orch { .. }));
        assert!(matches!(tools[1], ToolPayload::Mcp { .. }));
    }

    #[test]
    fn a_sub_agent_that_is_running_stays_open_until_it_is_done() {
        let (mut mapper, _) = mapper(false);
        feed(
            &mut mapper,
            vec![
                json!({ "type": "assistant", "parent_tool_use_id": null, "message": { "id": "m", "content": [
                { "type": "tool_use", "id": "ag1", "name": "Agent", "input": { "description": "Look", "prompt": "p", "subagent_type": "general-purpose" } }] } }),
            ],
        );
        let launched = feed(
            &mut mapper,
            vec![json!({ "type": "user", "parent_tool_use_id": null,
                "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "ag1", "content": "Async agent launched" }] },
                "tool_use_result": { "status": "async_launched", "agentId": "a1" } })],
        );
        assert!(launched.is_empty(), "the call is not over");
        let done = feed(
            &mut mapper,
            vec![
                json!({ "type": "system", "subtype": "task_notification", "tool_use_id": "ag1", "status": "completed", "summary": "found it" }),
            ],
        );
        let SessionEvent::ItemCompleted(closed) = &done[0] else {
            panic!("completed");
        };
        assert_eq!(closed.payload["tool"]["state"], "completed");
        assert_eq!(closed.payload["tool"]["child_stream"], "sub:ag1");
    }

    #[test]
    fn a_background_command_keeps_its_live_line_until_a_native_task_result() {
        let (mut mapper, _) = mapper(false);
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","parent_tool_use_id":null,"message":{"id":"m","content":[{"type":"tool_use","id":"bg","name":"Bash","input":{"command":"sleep 300","run_in_background":true}}]}}),
            ],
        );
        let launched = feed(
            &mut mapper,
            vec![
                json!({"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bg","content":"background task launched"}]},"tool_use_result":{"backgroundTaskId":"task"}}),
            ],
        );
        let SessionEvent::ItemUpdated(command) = &launched[0] else {
            panic!("a launch updates the live command")
        };
        assert_eq!(command.payload["tool"]["background"], true);
        assert!(command.payload["tool"]["exit_code"].is_null());
        let turn_end = feed(&mut mapper, vec![result_frame(json!({}))]);
        assert!(
            !turn_end.iter().any(
                |event| matches!(event,SessionEvent::ItemCompleted(item) if item.native_ref=="bg")
            ),
            "an ended turn is not the command's exit"
        );
        let ended = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_notification","tool_use_id":"bg","status":"killed","summary":"ended"}),
            ],
        );
        let SessionEvent::ItemCompleted(command) = &ended[0] else {
            panic!("the native task result ends the command")
        };
        assert_eq!(command.status, ItemStatus::Failed);
        assert!(
            command.payload["tool"]["exit_code"].is_null(),
            "a missing native exit code stays unknown"
        );
        assert!(crate::descriptor::capabilities().background_commands);
    }

    #[test]
    fn a_native_stopped_task_closes_its_existing_background_item_once() {
        for (name, input, launched) in [
            (
                "Bash",
                json!({"command":"sleep 300","run_in_background":true}),
                json!({"backgroundTaskId":"task"}),
            ),
            (
                "Agent",
                json!({"description":"Look","prompt":"Read the code","subagent_type":"general-purpose"}),
                json!({"status":"async_launched","agentId":"task"}),
            ),
        ] {
            let (mut mapper, _) = mapper(false);
            feed(
                &mut mapper,
                vec![
                    json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"bg","name":name,"input":input}]}}),
                    json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bg","content":"launched"}]},"tool_use_result":launched}),
                ],
            );
            let turn_end = feed(&mut mapper, vec![result_frame(json!({}))]);
            assert!(
                !turn_end.iter().any(
                    |event| matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == "bg")
                ),
                "the turn ending is not native task completion"
            );
            let stopped = json!({
                "type":"system","subtype":"task_notification","session_id":"session",
                "uuid":"stopped","task_id":"task","run_id":"run-1","tool_use_id":"bg",
                "status":"stopped","output_file":"/workspace/task-output","summary":"Stopped by the user"
            });
            let ended = feed(&mut mapper, vec![stopped.clone()]);
            let ended_items: Vec<_> = ended
                .iter()
                .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
                .cloned()
                .collect();
            let [SessionEvent::ItemCompleted(item)] = ended_items.as_slice() else {
                panic!("the native stopped notification closes {name}: {ended:?}");
            };
            assert_eq!(item.native_ref, "bg");
            assert_eq!(item.status, ItemStatus::Cancelled);
            assert_eq!(item.payload["raw"]["name"], name);
            assert_eq!(
                serde_json::from_str::<Json>(
                    item.payload["raw"]["input"]["inline"].as_str().unwrap()
                )
                .unwrap(),
                input
            );
            if name == "Bash" {
                assert!(item.payload["tool"]["exit_code"].is_null());
            } else {
                assert_eq!(item.payload["tool"]["state"], "interrupted");
                assert_eq!(item.payload["tool"]["child_stream"], "sub:bg");
            }
            assert!(feed(&mut mapper, vec![stopped]).is_empty());
        }
    }

    #[test]
    fn owned_background_events_keep_the_origin_across_turns_and_refresh_the_native_live_set() {
        use orch_agent_contract::background::BackgroundTaskState;
        let (mut mapper, origin) = mapper(false);
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"start","content":[{"type":"tool_use","id":"bg","name":"Bash","input":{"command":"sleep 1","run_in_background":true,"description":"Build"}}]}}),
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"bg","content":"started"}]},"tool_use_result":{"backgroundTaskId":"task"}}),
                json!({"type":"system","subtype":"task_started","task_id":"task","run_id":"run","tool_use_id":"bg","task_type":"local_bash","description":"Build","is_backgrounded":true}),
            ],
        );
        let refreshed = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"background_tasks_changed","tasks":[
                    {"task_id":"task","run_id":"run","task_type":"local_bash","description":"Build assets"},
                    {"task_id":"ambient","task_type":"local_agent","description":"Watcher","ambient":true}
                ]}),
            ],
        );
        assert!(refreshed.iter().any(
            |event| matches!(event, SessionEvent::BackgroundTasks(report)
            if report.tasks.iter().any(|task| task.label == "Build assets"))
        ));
        feed(&mut mapper, vec![result_frame(json!({}))]);
        mapper.begin_turn(orch_types::new_id());
        let ended = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_notification","task_id":"task","run_id":"run","tool_use_id":"bg","status":"failed","exit_code":2,"summary":"Build failed"}),
            ],
        );
        let report = ended
            .iter()
            .find_map(|event| match event {
                SessionEvent::BackgroundTasks(report) => Some(report),
                _ => None,
            })
            .expect("the native completion has a separate owned task report");
        assert!(report.self_waking);
        assert_eq!(report.tasks.len(), 1);
        assert_eq!(report.tasks[0].origin_turn, origin);
        assert_eq!(report.tasks[0].native_item_ref, "bg");
        assert_eq!(report.tasks[0].state, BackgroundTaskState::Failed);
        assert_eq!(report.tasks[0].exit_code, Some(2));
        assert!(!mapper.background_running());
    }

    #[test]
    fn a_native_background_transition_keeps_the_original_command_until_its_task_ends() {
        let (mut mapper, _) = mapper(false);
        let input = json!({"command":"printf ready","description":"Wait"});
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"bg","name":"Bash","input":input}]}}),
                json!({"type":"system","subtype":"task_started","session_id":"session","uuid":"started","task_id":"task","run_id":"run-1","tool_use_id":"bg","task_type":"local_bash","description":"Wait","is_backgrounded":false}),
            ],
        );
        let backgrounded = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_updated","session_id":"session","uuid":"backgrounded","task_id":"task","run_id":"run-1","patch":{"is_backgrounded":true}}),
            ],
        );
        let backgrounded_items: Vec<_> = backgrounded
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemUpdated(item)] = backgrounded_items.as_slice() else {
            panic!("the native transition updates the existing command: {backgrounded:?}");
        };
        assert_eq!(item.native_ref, "bg");
        assert_eq!(item.payload["tool"]["background"], true);
        assert!(item.payload["tool"]["exit_code"].is_null());
        assert_eq!(
            serde_json::from_str::<Json>(item.payload["raw"]["input"]["inline"].as_str().unwrap())
                .unwrap(),
            input
        );
        let turn_end = feed(&mut mapper, vec![result_frame(json!({}))]);
        assert!(
            !turn_end.iter().any(
                |event| matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == "bg")
            ),
            "a natively backgrounded command outlives its turn"
        );
        let ended = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_notification","session_id":"session","uuid":"stopped","task_id":"task","run_id":"run-1","status":"stopped","output_file":"/workspace/task-output","summary":"Stopped by the user"}),
            ],
        );
        let ended_items: Vec<_> = ended
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemCompleted(item)] = ended_items.as_slice() else {
            panic!("the exact task identity closes its original command: {ended:?}");
        };
        assert_eq!(item.native_ref, "bg");
        assert_eq!(item.status, ItemStatus::Cancelled);
        assert!(item.payload["tool"]["exit_code"].is_null());
    }

    #[test]
    fn native_subagent_progress_updates_its_existing_lane_without_inventing_usage() {
        let (mut mapper, _) = mapper(false);
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"ag","name":"Agent","input":{"description":"Look","prompt":"Read the code","subagent_type":"general-purpose"}}]}}),
                json!({"type":"system","subtype":"task_started","session_id":"session","uuid":"started","task_id":"task","run_id":"run-1","tool_use_id":"ag","task_type":"local_agent","description":"Look","is_backgrounded":true}),
            ],
        );
        let progress = json!({
            "type":"system","subtype":"task_progress","session_id":"session","uuid":"progress",
            "task_id":"task","run_id":"run-1","tool_use_id":"ag","description":"Look",
            "summary":"Reading the request handler","usage":{"total_tokens":12,"tool_uses":2,"duration_ms":100}
        });
        let events = feed(&mut mapper, vec![progress.clone()]);
        let events_items: Vec<_> = events
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemUpdated(item)] = events_items.as_slice() else {
            panic!("native progress updates only the existing subagent: {events:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(item.payload["title"], "Reading the request handler");
        assert_eq!(item.payload["tool"]["child_stream"], "sub:ag");
        assert_eq!(item.payload["tool"]["prompt"], "Read the code");
        assert_eq!(item.payload["tool"]["state"], "running");
        assert!(item.payload["duration_ms"].is_null());
        let ended = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_notification","session_id":"session","uuid":"completed","task_id":"task","run_id":"run-1","status":"completed","output_file":"/workspace/task-output","summary":"Read it"}),
            ],
        );
        let ended_items: Vec<_> = ended
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemCompleted(item)] = ended_items.as_slice() else {
            panic!("native completion closes the same subagent: {ended:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(item.payload["tool"]["state"], "completed");
        assert!(feed(&mut mapper, vec![progress]).is_empty());
    }

    #[test]
    fn native_task_updates_require_the_known_tool_session_and_current_run() {
        let older_run = "7f4dd67e-a8bb-487f-98a0-83e1b656c13a";
        let current_run = "b1ee84f0-5d27-45ae-9a6a-f418fe710387";
        let newer_run = "f01e7336-ab7f-4b14-9768-1f54d1c98080";
        let (mut mapper, _) = mapper(false);
        mapper.set_native_session("session");
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"ag","name":"Agent","input":{"description":"Look","prompt":"Read"}},{"type":"tool_use","id":"plan","name":"TaskCreate","input":{"subject":"Read"}}]}}),
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":current_run,"tool_use_id":"ag","task_type":"local_agent","description":"Look","is_backgrounded":true}),
            ],
        );
        let refused = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"unknown","run_id":older_run,"tool_use_id":"unowned","task_type":"local_agent","description":"Other"}),
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"plan","run_id":older_run,"tool_use_id":"plan","task_type":"local_agent","description":"Not a subagent"}),
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"wrong-kind","run_id":older_run,"tool_use_id":"ag","task_type":"local_bash","description":"Not a shell"}),
                json!({"type":"system","subtype":"task_progress","session_id":"other","task_id":"task","run_id":current_run,"summary":"Wrong session"}),
                json!({"type":"system","subtype":"task_progress","session_id":"session","task_id":"task","run_id":older_run,"summary":"Older run"}),
                json!({"type":"system","subtype":"task_progress","session_id":"session","task_id":"unknown","run_id":current_run,"tool_use_id":"ag","summary":"Unknown task"}),
                json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"task","run_id":current_run,"tool_use_id":"unowned","status":"completed","summary":"Wrong tool"}),
                json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"other-task","run_id":current_run,"tool_use_id":"ag","status":"completed","summary":"Wrong task"}),
                json!({"type":"system","subtype":"task_notification","task_id":"task","run_id":current_run,"tool_use_id":"ag","status":"completed","summary":"Missing session"}),
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":older_run,"tool_use_id":"ag","task_type":"local_agent","description":"Late start"}),
            ],
        );
        assert!(
            refused.is_empty(),
            "unknown or stale tasks change no owned line"
        );
        let valid = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_progress","session_id":"session","task_id":"task","run_id":current_run,"summary":"Still reading"}),
            ],
        );
        let valid_items: Vec<_> = valid
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemUpdated(item)] = valid_items.as_slice() else {
            panic!("the current owned task still updates: {valid:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(item.payload["title"], "Still reading");
        mapper.process_started(false);
        assert!(
            feed(
                &mut mapper,
                vec![
                    json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":newer_run,"tool_use_id":"ag","task_type":"local_agent","description":"Old process tool"}),
                    json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"task","run_id":current_run,"tool_use_id":"ag","status":"stopped","summary":"Old process result"}),
                ],
            )
            .is_empty()
        );
    }

    #[test]
    fn a_native_terminal_update_keeps_the_completion_bookend_and_rejects_late_progress() {
        let (mut mapper, _) = mapper(false);
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"ag","name":"Agent","input":{"description":"Look","prompt":"Read"}}]}}),
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":"run-1","tool_use_id":"ag","task_type":"local_agent","description":"Look","is_backgrounded":true}),
            ],
        );
        let updated = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_updated","session_id":"session","task_id":"task","run_id":"run-1","patch":{"status":"completed","description":"Finished"}}),
            ],
        );
        let updated_items: Vec<_> = updated
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemUpdated(item)] = updated_items.as_slice() else {
            panic!("the native terminal patch updates its existing subagent: {updated:?}");
        };
        assert_eq!(item.payload["tool"]["state"], "completed");
        assert!(item.payload["raw"]["output"].is_null());
        let late = json!({"type":"system","subtype":"task_progress","session_id":"session","task_id":"task","run_id":"run-1","summary":"Late running description"});
        assert!(feed(&mut mapper, vec![late.clone()]).is_empty());
        assert!(
            feed(
                &mut mapper,
                vec![
                    json!({"type":"system","subtype":"task_updated","session_id":"session","task_id":"task","run_id":"run-1","patch":{"status":"running","description":"Late running patch"}}),
                    json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":"run-1","tool_use_id":"ag","task_type":"local_agent","description":"Duplicate same-run start","is_backgrounded":true}),
                ],
            )
            .is_empty()
        );
        let completed = json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"task","run_id":"run-1","status":"completed","summary":"The full native completion summary"});
        let ended = feed(&mut mapper, vec![completed.clone()]);
        let ended_items: Vec<_> = ended
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemCompleted(item)] = ended_items.as_slice() else {
            panic!("the bookend retains the actual native summary: {ended:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(
            item.payload["raw"]["output"]["inline"],
            "The full native completion summary"
        );
        assert!(feed(&mut mapper, vec![completed, late]).is_empty());
    }

    #[test]
    fn native_optional_fields_preserve_an_explicit_known_tool_bookend() {
        let (mut mapper, _) = mapper(false);
        mapper.set_native_session("session");
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"ag","name":"Agent","input":{"description":"Look","prompt":"Read"}}]}}),
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":"b1ee84f0-5d27-45ae-9a6a-f418fe710387","tool_use_id":"ag","description":"Look","is_backgrounded":true}),
            ],
        );
        let progress = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_progress","session_id":"session","task_id":"task","tool_use_id":"ag","summary":"Known tool, optional run"}),
            ],
        );
        let progress_items: Vec<_> = progress
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemUpdated(item)] = progress_items.as_slice() else {
            panic!("an explicit known tool still updates without optional fields: {progress:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(item.payload["tool"]["child_stream"], "sub:ag");
        let ended = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"task","tool_use_id":"ag","status":"completed","summary":"Read it"}),
            ],
        );
        let ended_items: Vec<_> = ended
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemCompleted(item)] = ended_items.as_slice() else {
            panic!("the explicit known tool retains its completion: {ended:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(item.status, ItemStatus::Completed);
    }

    #[test]
    fn native_resumption_can_keep_its_tool_and_rejects_only_stale_or_ambiguous_bookends() {
        let older_run = "7f4dd67e-a8bb-487f-98a0-83e1b656c13a";
        let current_run = "b1ee84f0-5d27-45ae-9a6a-f418fe710387";
        let (mut mapper, _) = mapper(false);
        mapper.set_native_session("session");
        feed(
            &mut mapper,
            vec![
                json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"ag","name":"Agent","input":{"description":"Look","prompt":"Read"}}]}}),
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":older_run,"tool_use_id":"ag","task_type":"local_agent","description":"Look","is_backgrounded":true}),
                json!({"type":"system","subtype":"task_updated","session_id":"session","task_id":"task","run_id":older_run,"patch":{"status":"completed","description":"Finished an earlier run"}}),
            ],
        );
        let resumed = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_started","session_id":"session","task_id":"task","run_id":current_run,"description":"Resumed","is_backgrounded":true}),
            ],
        );
        let resumed_items: Vec<_> = resumed
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemUpdated(item)] = resumed_items.as_slice() else {
            panic!("the known logical task resumes in its existing lane: {resumed:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(item.payload["tool"]["state"], "running");
        assert_eq!(item.payload["tool"]["child_stream"], "sub:ag");
        assert!(
            feed(
                &mut mapper,
                vec![
                    json!({"type":"system","subtype":"task_progress","session_id":"session","task_id":"task","run_id":older_run,"tool_use_id":"ag","summary":"Late old progress"}),
                    json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"task","run_id":older_run,"tool_use_id":"ag","status":"completed","summary":"Late old result"}),
                    json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"task","tool_use_id":"ag","status":"completed","summary":"Cannot identify which of two runs ended"}),
                ],
            )
            .is_empty()
        );
        let ended = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_notification","session_id":"session","task_id":"task","run_id":current_run,"status":"completed","summary":"The resumed run finished"}),
            ],
        );
        let ended_items: Vec<_> = ended
            .iter()
            .filter(|event| !matches!(event, SessionEvent::BackgroundTasks(_)))
            .cloned()
            .collect();
        let [SessionEvent::ItemCompleted(item)] = ended_items.as_slice() else {
            panic!("the current run closes the same owned line: {ended:?}");
        };
        assert_eq!(item.native_ref, "ag");
        assert_eq!(item.payload["tool"]["prompt"], "Read");
        assert_eq!(
            item.payload["raw"]["output"]["inline"],
            "The resumed run finished"
        );
    }

    #[test]
    fn a_foreign_task_frame_is_retained_only_as_a_raw_diagnostic() {
        let (mut mapper, _) = mapper(true);
        mapper.set_native_session("session");
        let events = feed(
            &mut mapper,
            vec![
                json!({"type":"system","subtype":"task_started","session_id":"other","task_id":"task","run_id":"run","tool_use_id":"unowned","task_type":"local_agent","description":"Other"}),
            ],
        );
        assert_eq!(kinds(&events), ["raw.frame"]);
    }

    #[test]
    fn the_fifth_failed_attempt_in_a_row_is_told_once() {
        let (mut mapper, _) = mapper(false);
        let attempt = |n: u64| {
            json!({ "type": "system", "subtype": "api_retry", "attempt": n, "max_retries": 10,
                "error_status": 529, "error": "overloaded", "uuid": format!("r{n}") })
        };
        assert!(feed(&mut mapper, vec![attempt(1), attempt(4)]).is_empty());
        let fifth = feed(&mut mapper, vec![attempt(5), attempt(6)]);
        assert_eq!(kinds(&fifth), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(notice) = &fifth[0] else {
            panic!("a notice");
        };
        let ItemContent::Error(error) = &notice.content else {
            panic!("an error item");
        };
        assert!(error.retryable && error.message.contains("529"));
    }

    #[test]
    fn with_diagnostics_on_every_frame_is_kept_and_a_huge_one_is_kept_as_a_stub() {
        let (mut mapper, _) = mapper(true);
        let events = feed(
            &mut mapper,
            vec![
                json!({ "type": "system", "subtype": "thinking_tokens", "estimated_tokens": 3 }),
                json!({ "type": "mystery", "blob": "x".repeat(RAW_FRAME_MAX_BYTES + 1) }),
            ],
        );
        assert_eq!(kinds(&events), ["raw.frame", "raw.frame"]);
        let SessionEvent::RawFrame(stub) = &events[1] else {
            panic!("a raw frame");
        };
        assert_eq!(stub.payload["truncated"], true);
        assert_eq!(stub.direction, FROM_HARNESS);
        let (mut quiet, _) = mapper_quiet();
        assert!(feed(&mut quiet, vec![json!({ "type": "mystery" })]).is_empty());
    }

    fn mapper_quiet() -> (EventMapper, TurnId) {
        mapper(false)
    }

    #[test]
    fn a_tool_server_that_failed_to_start_is_reported_on_the_thread() {
        let (mut mapper, _) = mapper(false);
        let events = feed(
            &mut mapper,
            vec![json!({ "type": "system", "subtype": "init", "uuid": "i1",
                "mcp_servers": [{ "name": "orch", "status": "connected" }, { "name": "tmux", "status": "failed" }] })],
        );
        assert_eq!(kinds(&events), ["item.started", "item.completed"]);
        let SessionEvent::ItemStarted(notice) = &events[0] else {
            panic!("a notice");
        };
        let ItemContent::Error(error) = &notice.content else {
            panic!("an error item");
        };
        assert_eq!(error.code, "mcp_server_failed");
        assert!(error.message.contains("tmux"));
    }

    #[test]
    fn a_turn_the_host_ends_closes_what_was_open_and_completes_with_the_error_it_names() {
        let (mut mapper, turn) = mapper(false);
        let _ = feed(
            &mut mapper,
            vec![
                json!({ "type": "stream_event", "event": { "type": "message_start", "message": { "id": "m1", "role": "assistant", "content": [] } } }),
                json!({ "type": "stream_event", "event": { "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } } }),
                json!({ "type": "stream_event", "event": { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Half a sen" } } }),
            ],
        );
        let events = mapper.fail_turn("provider_unauthorized", "refused", true);
        let SessionEvent::TurnCompleted(end) = events.last().unwrap() else {
            panic!("the turn completed");
        };
        assert_eq!(end.turn_id, turn);
        assert_eq!(end.status, TurnOutcome::Failed);
        let error = end.error.as_ref().unwrap();
        assert_eq!(
            (error.code.as_str(), error.retryable),
            ("provider_unauthorized", true)
        );
        assert!(
            events.iter().any(|event| matches!(event, SessionEvent::ItemCompleted(done) if done.status == ItemStatus::Failed)),
            "the half sentence is closed as failed"
        );
        assert!(!mapper.turn_active());
        assert!(
            mapper.fail_turn("again", "no turn", false).is_empty(),
            "nothing to end twice"
        );
    }
}
