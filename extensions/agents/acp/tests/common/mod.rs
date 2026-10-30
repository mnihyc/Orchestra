//! What the tests of the generic adapter share: an agent that speaks the protocol from a pipe and does as a script says, a launcher that starts it, and an adapter started on it.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;
use orch_adapter_acp::AcpAdapter;
use orch_adapter_acp::process::{
    HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit,
};
use orch_adapter_acp::program::AcpProgram;
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use orch_agent_contract::session::{
    ExecAccess, ProviderAccess, SessionMode, SessionSettings, SessionSpec, ThreadId, TurnId,
    TurnStart,
};
use orch_types::{Id, Json, SystemClock, new_id};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, oneshot};

/// How long a test waits for the next event before it decides nothing more is coming.
pub const QUIET: Duration = Duration::from_secs(10);

// -- the agent --------------------------------------------------------------------------------------------

/// One thing the scripted agent does while it answers a prompt.
#[derive(Debug, Clone)]
pub enum Action {
    /// Sends `session/update` with this update.
    Update(Json),
    /// Sends a notification of any name.
    Notify(&'static str, Json),
    /// Asks the client something and waits for the answer, which is kept under `tag`.
    Ask {
        /// The method.
        method: &'static str,
        /// The parameters; `$SESSION` in a string is the session's id.
        params: Json,
        /// Where the answer is kept.
        tag: &'static str,
    },
    /// Waits.
    Sleep(u64),
    /// Waits until `session/cancel` comes.
    UntilCancelled,
    /// Never answers the prompt.
    Hang,
    /// The agent's process ends.
    Die,
    /// Writes a line on its error output.
    Stderr(&'static str),
    /// Ends the prompt with this stop reason instead of the default.
    Stop(&'static str),
    /// Ends the prompt with an error.
    Fail(i64, &'static str),
}

/// What the scripted agent is, and does.
pub struct Behavior {
    /// What it answers to `initialize` under `agentCapabilities`.
    pub capabilities: Json,
    /// The options it lists.
    pub options: Json,
    /// The modes it lists.
    pub modes: Option<Json>,
    /// What it does for each prompt, in turn; an empty queue answers with `You said: <text>`.
    pub prompts: VecDeque<Vec<Action>>,
    /// What it says as the plugin's hello, before it answers `initialize`.
    pub hello: Option<Json>,
    /// Whether it says its hello only when it is asked (`_orch/hello`) and not on its own.
    pub hello_on_request: bool,
    /// What it prints for `--version`.
    pub version: String,
    /// Whether it answers `session/new`.
    pub fails_new: Option<(i64, String)>,
    /// A refusal of a neutral history import.
    pub fails_import: Option<(i64, String)>,
    /// What each process of it writes on its error output as it starts, before it answers anything.
    pub startup_note: Option<String>,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            capabilities: json!({
                "mcpCapabilities": { "http": true },
                "promptCapabilities": { "image": true },
                "sessionCapabilities": { "close": {}, "list": {}, "resume": {} }
            }),
            options: json!([
                { "id": "model", "name": "Model", "category": "model", "type": "select",
                  "currentValue": "[\"orch\",\"scripted\"]",
                  "options": [ { "group": "orch", "name": "orch", "options": [
                      { "value": "[\"orch\",\"scripted\"]", "name": "scripted" },
                      { "value": "[\"orch\",\"second\"]", "name": "second" } ] } ] },
                { "id": "reasoning_effort", "name": "Effort", "category": "thought_level", "type": "select",
                  "currentValue": "", "options": [
                      { "value": "", "name": "Provider default" }, { "value": "low", "name": "Low" }, { "value": "high", "name": "High" } ] }
            ]),
            modes: None,
            prompts: VecDeque::new(),
            hello: None,
            hello_on_request: false,
            version: "0.2.1-rc.2".to_owned(),
            fails_new: None,
            fails_import: None,
            startup_note: None,
        }
    }
}

/// What the agent saw and did, for the test to read back.
#[derive(Default)]
pub struct Record {
    /// Every frame the adapter wrote to it, in order.
    pub frames: Vec<Json>,
    /// What each process of it was started with.
    pub launches: Vec<LaunchRequest>,
    /// The answers it was given to what it asked, by tag.
    pub answers: HashMap<String, Json>,
}

/// The scripted agent: its behavior and what it recorded.
pub struct Agent {
    /// What it does.
    pub behavior: Mutex<Behavior>,
    /// What it saw.
    pub record: Mutex<Record>,
    sessions: AtomicU64,
    processes: AtomicU64,
    /// What ends the process that was started last.
    last_kill: Mutex<Option<Arc<Notify>>>,
}

impl Agent {
    /// An agent with the default behavior.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            behavior: Mutex::new(Behavior::default()),
            record: Mutex::new(Record::default()),
            sessions: AtomicU64::new(0),
            processes: AtomicU64::new(0),
            last_kill: Mutex::new(None),
        })
    }

    /// Ends the process that was started last, as a crash, a signal or the end of an idle program would.
    pub fn end_process(&self) {
        if let Some(kill) = self
            .last_kill
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            kill.notify_one();
        }
    }

    /// Queues what it does for the next prompt that has no script.
    pub fn script(&self, actions: Vec<Action>) {
        self.behavior
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .prompts
            .push_back(actions);
    }

    /// The frames of the adapter of one method, in order.
    #[must_use]
    pub fn frames(&self, method: &str) -> Vec<Json> {
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .frames
            .iter()
            .filter(|frame| frame["method"] == method)
            .cloned()
            .collect()
    }

    /// What it was answered to a question it asked, by tag.
    #[must_use]
    pub fn answer(&self, tag: &str) -> Option<Json> {
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .answers
            .get(tag)
            .cloned()
    }

    /// How many processes of the agent were started to run (not to print their version).
    #[must_use]
    pub fn processes(&self) -> u64 {
        self.processes.load(Ordering::Relaxed)
    }
}

struct Shared {
    writer: tokio::sync::Mutex<tokio::io::DuplexStream>,
    /// The process's error output.
    stderr: tokio::sync::Mutex<tokio::io::DuplexStream>,
    asked: Mutex<HashMap<i64, oneshot::Sender<Json>>>,
    next_ask: AtomicU64,
    cancelled: Notify,
    cancel_seen: std::sync::atomic::AtomicBool,
}

impl Shared {
    async fn send(&self, frame: &Json) {
        let mut writer = self.writer.lock().await;
        let _ = writer.write_all(format!("{frame}\n").as_bytes()).await;
        let _ = writer.flush().await;
    }

    /// Writes a line on the error output.
    async fn say(&self, line: &str) {
        let mut writer = self.stderr.lock().await;
        let _ = writer.write_all(format!("{line}\n").as_bytes()).await;
        let _ = writer.flush().await;
    }
}

fn substitute(value: &mut Json, session: &str) {
    match value {
        Json::String(text) => *text = text.replace("$SESSION", session),
        Json::Array(items) => items.iter_mut().for_each(|item| substitute(item, session)),
        Json::Object(map) => map.values_mut().for_each(|item| substitute(item, session)),
        _ => {}
    }
}

async fn run_prompt(
    agent: Arc<Agent>,
    shared: Arc<Shared>,
    id: Json,
    session: String,
    actions: Vec<Action>,
    kill: Arc<Notify>,
) {
    let mut stop = "end_turn".to_owned();
    for action in actions {
        match action {
            Action::Update(mut update) => {
                substitute(&mut update, &session);
                shared
                    .send(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": update } }))
                    .await;
            }
            Action::Notify(method, mut params) => {
                substitute(&mut params, &session);
                shared
                    .send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
                    .await;
            }
            Action::Ask {
                method,
                mut params,
                tag,
            } => {
                substitute(&mut params, &session);
                let ask = i64::try_from(shared.next_ask.fetch_add(1, Ordering::Relaxed)).unwrap();
                let (sender, receiver) = oneshot::channel();
                shared.asked.lock().unwrap().insert(ask, sender);
                shared
                    .send(
                        &json!({ "jsonrpc": "2.0", "id": ask, "method": method, "params": params }),
                    )
                    .await;
                let answer = receiver.await.unwrap_or(Json::Null);
                agent
                    .record
                    .lock()
                    .unwrap()
                    .answers
                    .insert(tag.to_owned(), answer);
            }
            Action::Sleep(millis) => tokio::time::sleep(Duration::from_millis(millis)).await,
            Action::UntilCancelled => {
                if !shared.cancel_seen.load(Ordering::Relaxed) {
                    shared.cancelled.notified().await;
                }
                stop = "cancelled".to_owned();
            }
            Action::Hang => {
                std::future::pending::<()>().await;
            }
            Action::Die => {
                kill.notify_one();
                return;
            }
            Action::Stderr(line) => shared.say(line).await,
            Action::Stop(reason) => stop = reason.to_owned(),
            Action::Fail(code, message) => {
                shared
                    .send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
                    .await;
                return;
            }
        }
    }
    shared
        .send(&json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": stop } }))
        .await;
}

/// Starts the scripted agent as one process: the streams the adapter is given, and what ends it.
fn spawn(agent: Arc<Agent>) -> (HarnessProcess, Arc<Notify>) {
    // adapter -> agent
    let (adapter_out, agent_in) = tokio::io::duplex(1 << 20);
    // agent -> adapter
    let (agent_out, adapter_in) = tokio::io::duplex(1 << 20);
    // agent -> the adapter's note of what the program said
    let (agent_err, adapter_err) = tokio::io::duplex(1 << 16);
    let kill = Arc::new(Notify::new());
    let (ended_sender, ended) = oneshot::channel::<()>();
    let shared = Arc::new(Shared {
        writer: tokio::sync::Mutex::new(agent_out),
        stderr: tokio::sync::Mutex::new(agent_err),
        asked: Mutex::new(HashMap::new()),
        next_ask: AtomicU64::new(1000),
        cancelled: Notify::new(),
        cancel_seen: std::sync::atomic::AtomicBool::new(false),
    });
    let task_kill = Arc::clone(&kill);
    tokio::spawn(async move {
        let note = agent.behavior.lock().unwrap().startup_note.clone();
        if let Some(note) = note {
            shared.say(&note).await;
        }
        let mut lines = BufReader::new(agent_in).lines();
        loop {
            let line = tokio::select! {
                line = lines.next_line() => match line { Ok(Some(line)) => line, _ => break },
                () = task_kill.notified() => break,
            };
            let Ok(frame) = serde_json::from_str::<Json>(&line) else {
                continue;
            };
            agent.record.lock().unwrap().frames.push(frame.clone());
            handle(&agent, &shared, frame, &task_kill).await;
        }
        let _ = ended_sender.send(());
    });
    struct Control {
        kill: Arc<Notify>,
        ended: Option<oneshot::Receiver<()>>,
    }
    #[async_trait]
    impl ProcessControl for Control {
        async fn wait(&mut self) -> ProcessExit {
            if let Some(ended) = self.ended.take() {
                let _ = ended.await;
            }
            ProcessExit {
                code: Some(0),
                detail: String::new(),
            }
        }
        fn kill(&mut self) {
            self.kill.notify_one();
        }
    }
    (
        HarnessProcess {
            stdin: Box::new(adapter_out),
            stdout: Box::new(adapter_in),
            stderr: Some(Box::new(adapter_err)),
            control: Box::new(Control {
                kill: Arc::clone(&kill),
                ended: Some(ended),
            }),
        },
        kill,
    )
}

async fn handle(agent: &Arc<Agent>, shared: &Arc<Shared>, frame: Json, kill: &Arc<Notify>) {
    let id = frame.get("id").cloned();
    let method = frame
        .get("method")
        .and_then(Json::as_str)
        .map(str::to_owned);
    let params = frame.get("params").cloned().unwrap_or(Json::Null);
    let Some(method) = method else {
        // An answer to a question of the agent.
        if let Some(id) = id.and_then(|id| id.as_i64())
            && let Some(sender) = shared.asked.lock().unwrap().remove(&id)
        {
            let _ = sender.send(
                frame
                    .get("result")
                    .cloned()
                    .unwrap_or_else(|| frame.get("error").cloned().unwrap_or(Json::Null)),
            );
        }
        return;
    };
    let reply = |result: Json| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    match method.as_str() {
        "initialize" => {
            let (capabilities, hello) = {
                let behavior = agent.behavior.lock().unwrap();
                (
                    behavior.capabilities.clone(),
                    behavior
                        .hello
                        .clone()
                        .filter(|_| !behavior.hello_on_request),
                )
            };
            if let Some(hello) = hello {
                shared
                    .send(&json!({ "jsonrpc": "2.0", "method": "_orch/hello", "params": hello }))
                    .await;
            }
            shared
                .send(&reply(json!({
                    "protocolVersion": 1,
                    "agentInfo": { "name": "scripted-agent", "version": "0.0.1" },
                    "agentCapabilities": capabilities,
                    "authMethods": []
                })))
                .await;
        }
        "session/new" => {
            let (options, modes, fails) = {
                let behavior = agent.behavior.lock().unwrap();
                (
                    behavior.options.clone(),
                    behavior.modes.clone(),
                    behavior.fails_new.clone(),
                )
            };
            if let Some((code, message)) = fails {
                shared
                    .send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
                    .await;
                return;
            }
            let number = agent.sessions.fetch_add(1, Ordering::Relaxed) + 1;
            let mut result =
                json!({ "sessionId": format!("sess-{number}"), "configOptions": options });
            if let Some(modes) = modes {
                result["modes"] = modes;
            }
            shared.send(&reply(result)).await;
        }
        "session/resume" | "session/load" => {
            let (options, modes) = {
                let behavior = agent.behavior.lock().unwrap();
                (behavior.options.clone(), behavior.modes.clone())
            };
            let mut result = json!({ "configOptions": options });
            if let Some(modes) = modes {
                result["modes"] = modes;
            }
            shared.send(&reply(result)).await;
        }
        "session/set_config_option" => {
            let options = {
                let mut behavior = agent.behavior.lock().unwrap();
                if let Some(options) = behavior.options.as_array_mut() {
                    for option in options {
                        if option["id"] == params["configId"] {
                            option["currentValue"] = params["value"].clone();
                        }
                    }
                }
                behavior.options.clone()
            };
            shared
                .send(&reply(json!({ "configOptions": options })))
                .await;
        }
        "session/set_mode" => {
            if let Some(modes) = agent.behavior.lock().unwrap().modes.as_mut() {
                modes["currentModeId"] = params["modeId"].clone();
            }
            shared.send(&reply(json!({}))).await;
        }
        "session/prompt" => {
            let session = params["sessionId"].as_str().unwrap_or_default().to_owned();
            let text = params["prompt"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let actions = agent.behavior.lock().unwrap().prompts.pop_front().unwrap_or_else(|| {
                vec![
                    Action::Update(json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "type": "text", "text": format!("You said: {text}") } })),
                ]
            });
            shared.cancel_seen.store(false, Ordering::Relaxed);
            tokio::spawn(run_prompt(
                Arc::clone(agent),
                Arc::clone(shared),
                id.unwrap_or(Json::Null),
                session,
                actions,
                Arc::clone(kill),
            ));
        }
        "session/cancel" => {
            shared.cancel_seen.store(true, Ordering::Relaxed);
            shared.cancelled.notify_waiters();
            shared.cancelled.notify_one();
        }
        "session/close" => shared.send(&reply(json!({}))).await,
        "_orch/hello" => {
            let hello = agent.behavior.lock().unwrap().hello.clone();
            match hello {
                Some(hello) => shared.send(&reply(hello)).await,
                None => {
                    shared
                        .send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "no plugin here" } }))
                        .await;
                }
            }
        }
        "_orch/input/steer" => {
            shared
                .send(&reply(
                    json!({ "messageId": params["messageId"], "accepted": true }),
                ))
                .await;
        }
        "_orch/plan/set" => shared.send(&reply(json!({ "outcome": "committed" }))).await,
        "_orch/approval/set" => shared.send(&reply(json!({ "mode": params["mode"] }))).await,
        "_orch/history/import" => {
            let refusal = agent.behavior.lock().unwrap().fails_import.clone();
            if let Some((code, message)) = refusal {
                shared.send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })).await;
                return;
            }
            shared
                .send(&reply(json!({ "sessionId": "imported-1", "report": {
                    "fidelity": "structured", "itemsTotal": 4,
                    "byKind": { "agent_message": { "kept": 2, "converted": 0, "dropped": 0 } },
                    "dropped": [], "warnings": [] } })))
                .await;
        }
        _ => {
            shared
                .send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "no such method" } }))
                .await;
        }
    }
}

/// Starts the scripted agent for every launch of the adapter; `--version` prints the version and ends.
pub struct Starter {
    /// The agent.
    pub agent: Arc<Agent>,
}

#[async_trait]
impl Launcher for Starter {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        self.agent
            .record
            .lock()
            .unwrap()
            .launches
            .push(request.clone());
        if request.args == ["--version"] {
            let version = format!("{}\n", self.agent.behavior.lock().unwrap().version);
            struct Done;
            #[async_trait]
            impl ProcessControl for Done {
                async fn wait(&mut self) -> ProcessExit {
                    ProcessExit {
                        code: Some(0),
                        detail: String::new(),
                    }
                }
                fn kill(&mut self) {}
            }
            return Ok(HarnessProcess {
                stdin: Box::new(tokio::io::sink()),
                stdout: Box::new(std::io::Cursor::new(version.into_bytes())),
                stderr: None,
                control: Box::new(Done),
            });
        }
        self.agent.processes.fetch_add(1, Ordering::Relaxed);
        let (process, kill) = spawn(Arc::clone(&self.agent));
        *self
            .agent
            .last_kill
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(kill);
        Ok(process)
    }
}

// -- the adapter ------------------------------------------------------------------------------------------

/// One event the adapter reported, as the plugin transport writes it.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The name.
    pub name: String,
    /// The body, without the thread and segment.
    pub body: Json,
    /// The thread, for a session event or a model-request report.
    pub thread: Option<ThreadId>,
}

impl Seen {
    /// Whether it is an event of that name.
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }
}

/// The names of events, in order.
#[must_use]
pub fn names(events: &[Seen]) -> Vec<&str> {
    events.iter().map(|event| event.name.as_str()).collect()
}

/// The program the tests drive: a program whose own words are those of DeepSeek Harness, where a test needs them,
/// and plain otherwise.
#[must_use]
pub fn program(extra: Json) -> AcpProgram {
    let mut data = json!({
        "kind": "demo", "title": "Demo agent", "supported_range": ">=0.2.1-rc.2, <0.3.0",
        "programs": ["demo-agent"],
        "args": ["--acp"],
        "env": { "DEMO_TOKEN": "{llm_token}" },
        "arrangements": ["key_separated", "together"],
        "approvals": { "ask": { "permission_mode": "ask" }, "bypass": { "permission_mode": "never" } },
        "protocols": { "api": { "openai-chat": "openai-completions" }, "base_suffix": { "openai-chat": "/v1" } },
        "tools": [ { "names": ["bash"], "family": "command" } ],
        "cancel_grace_ms": 300
    });
    if let (Some(data), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            data.insert(key.clone(), value.clone());
        }
    }
    AcpProgram::from_json(&data).unwrap()
}

/// The spec of a session in `work`, opened as `mode`.
#[must_use]
pub fn spec_in(work: &Path, agent_id: Id, provider: Id, mode: SessionMode) -> SessionSpec {
    let directory = work.display().to_string();
    SessionSpec {
        settings: SessionSettings {
            agent_id,
            thread_id: new_id(),
            segment_id: new_id(),
            mode,
            native_session_ref: None,
            upto_turn_ordinal: None,
            workspace_root: directory.clone(),
            instruction_root_marker: false,
            directory: directory.clone(),
            cwd: directory,
            mounts: vec![],
            model: "scripted".to_owned(),
            effort: None,
            params: json!({}),
            approval_mode: ApprovalMode::Ask,
            sandbox_mode: SandboxMode::Open,
            instructions: "You are working inside Orch.".to_owned(),
            available_models: vec!["scripted".to_owned(), "second".to_owned()],
            subagent_limits: Default::default(),
            memory: None,
            arrangement: ArrangementType::KeySeparated,
            orch_tools: vec![orch_agent_contract::events::OrchTool::ListShared],
            orch_tool_schemas: Default::default(),
            raw_events: false,
            side_reading: None,
            workspace_configuration: None,
        },
        session_key: format!("{:0>32}", new_id().simple()),
        archive: None,
        history: None,
        provider: ProviderAccess::Forwarded {
            provider_id: provider,
            protocol: "openai-chat".to_owned(),
        },
        exec: ExecAccess {
            executor: None,
            tool_set: None,
            environment_id: new_id(),
            generation: 1,
        },
        mcp: vec![],
        skills: None,
    }
}

/// What the connector gives an adapter to start a harness, with a home belonging to the test.
#[must_use]
pub fn agent_start(provider: Id, agent: Id, home: &Path) -> AgentStart {
    Rig::agent_start(provider, agent, home)
}

/// The turn that says `text`, for the thread of `spec`.
#[must_use]
pub fn turn_for(spec: &SessionSpec, text: &str) -> TurnStart {
    TurnStart {
        plan: false,
        thread_id: spec.settings.thread_id,
        turn_id: new_id(),
        text: text.to_owned(),
        images: vec![],
        model: "scripted".to_owned(),
        effort: None,
        params: json!({}),
        approval_mode: ApprovalMode::Ask,
        sandbox_mode: SandboxMode::Open,
    }
}

/// The adapter under test, with its home, its working directory and the scripted agent behind it.
pub struct Rig {
    /// The adapter.
    pub adapter: AcpAdapter,
    /// What it reports.
    pub events: EventStream,
    /// The agent.
    pub agent: Arc<Agent>,
    /// The home.
    pub home: tempfile::TempDir,
    /// The directory the sessions work in.
    pub work: tempfile::TempDir,
    /// The agent profile.
    pub agent_id: Id,
    /// The provider.
    pub provider: Id,
    /// What starting answered.
    pub started: AgentStarted,
}

/// A session the test opened.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The thread.
    pub thread: ThreadId,
    /// The segment.
    pub segment: Id,
    /// The agent's id for it.
    pub native: String,
}

impl Rig {
    /// An adapter for `program`, started.
    pub async fn start(program: AcpProgram, agent: Arc<Agent>) -> Self {
        let home = tempfile::tempdir().unwrap();
        Self::start_in(program, agent, home, false).await
    }

    /// The same, for a program whose companion plugin is a file the adapter carries (here, one that is never read).
    pub async fn start_plugin(program: AcpProgram, agent: Arc<Agent>) -> Self {
        let home = tempfile::tempdir().unwrap();
        Self::start_in(program, agent, home, true).await
    }

    async fn start_in(
        program: AcpProgram,
        agent: Arc<Agent>,
        home: tempfile::TempDir,
        plugin: bool,
    ) -> Self {
        let mut adapter = AcpAdapter::with_launcher(
            program,
            Arc::new(Starter {
                agent: Arc::clone(&agent),
            }),
            Arc::new(SystemClock),
        );
        if plugin {
            adapter = adapter.with_plugin(b"// the plugin, which the scripted agent never reads");
        }
        let (sink, events) = EventSink::channel();
        let agent_id = new_id();
        let provider = new_id();
        let start = Self::agent_start(provider, agent_id, home.path());
        let started = adapter.start(start, sink).await.unwrap();
        Self {
            adapter,
            events,
            agent,
            home,
            work: tempfile::tempdir().unwrap(),
            agent_id,
            provider,
            started,
        }
    }

    /// What the connector gives an adapter to start a harness.
    #[must_use]
    pub fn agent_start(provider: Id, agent: Id, home: &Path) -> AgentStart {
        serde_json::from_value(json!({
            "provider_id": provider, "agent_id": agent, "harness_kind": "demo",
            "profile": { "harness_version": null, "config": {}, "env": {} },
            "home": home.display().to_string(),
            "provider_config": null,
            "loopback": { "base_url": "http://127.0.0.1:41999", "token": "t".repeat(64) },
            "binary": "demo-agent"
        }))
        .unwrap()
    }

    /// The spec of a session in the working directory, opened as `mode`.
    #[must_use]
    pub fn spec(&self, mode: SessionMode) -> SessionSpec {
        spec_in(self.work.path(), self.agent_id, self.provider, mode)
    }
    /// Opens a session by `spec`, and takes the `session.opened` it reports.
    pub async fn open(&mut self, spec: SessionSpec) -> Opened {
        let thread = spec.settings.thread_id;
        let segment = spec.settings.segment_id;
        let opened = self.adapter.open_session(spec).await.unwrap();
        let seen = self.until(|event| event.is("session.opened")).await;
        assert_eq!(
            seen.last().unwrap().body["native_session_ref"],
            opened.native_session_ref
        );
        Opened {
            thread,
            segment,
            native: opened.native_session_ref,
        }
    }

    /// A new session in the working directory.
    pub async fn session(&mut self) -> Opened {
        let spec = self.spec(SessionMode::New);
        self.open(spec).await
    }

    /// The turn that says `text`.
    #[must_use]
    pub fn turn_of(&self, session: &Opened, text: &str) -> TurnStart {
        TurnStart {
            plan: false,
            thread_id: session.thread,
            turn_id: new_id(),
            text: text.to_owned(),
            images: vec![],
            model: "scripted".to_owned(),
            effort: None,
            params: json!({}),
            approval_mode: ApprovalMode::Ask,
            sandbox_mode: SandboxMode::Open,
        }
    }

    /// Starts a turn and returns its id at once.
    pub async fn begin(&mut self, session: &Opened, text: &str) -> TurnId {
        let turn = self.turn_of(session, text);
        let id = turn.turn_id;
        self.adapter.start_turn(turn).await.unwrap();
        id
    }

    /// Runs a turn to its end.
    pub async fn say(&mut self, session: &Opened, text: &str) -> Vec<Seen> {
        self.begin(session, text).await;
        self.until(|event| event.is("turn.completed") || event.is("session.lost"))
            .await
    }

    fn see(event: AdapterEvent) -> Seen {
        let (name, mut body) = event.to_wire().unwrap();
        let thread = match &event {
            AdapterEvent::Session { thread_id, .. } => Some(*thread_id),
            AdapterEvent::ModelRequest(report) => Some(report.thread_id),
            _ => None,
        };
        if let Some(members) = body.as_object_mut() {
            members.remove("thread_id");
            members.remove("segment_id");
        }
        Seen {
            name: name.to_owned(),
            body,
            thread,
        }
    }

    /// The next event; the test fails if none comes for [`QUIET`].
    pub async fn next(&mut self) -> Seen {
        match tokio::time::timeout(QUIET, self.events.next()).await {
            Ok(Some(event)) => Self::see(event),
            Ok(None) => panic!("the adapter's stream closed"),
            Err(_) => panic!("nothing came for {QUIET:?}"),
        }
    }

    /// Everything up to and including the first event `done` accepts.
    pub async fn until(&mut self, mut done: impl FnMut(&Seen) -> bool) -> Vec<Seen> {
        let mut seen = Vec::new();
        loop {
            match tokio::time::timeout(QUIET, self.events.next()).await {
                Ok(Some(event)) => {
                    let event = Self::see(event);
                    let finished = done(&event);
                    seen.push(event);
                    if finished {
                        return seen;
                    }
                }
                Ok(None) => panic!("the adapter's stream closed after {:?}", names(&seen)),
                Err(_) => panic!("nothing more came for {QUIET:?}; so far {:?}", names(&seen)),
            }
        }
    }

    /// What has been reported already and not read, without waiting.
    pub fn queued(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            seen.push(Self::see(event));
        }
        seen
    }
}

/// An update of a text chunk.
#[must_use]
pub fn message(kind: &str, id: &str, text: &str) -> Action {
    Action::Update(
        json!({ "sessionUpdate": kind, "messageId": id, "content": { "type": "text", "text": text } }),
    )
}

/// An update that begins a tool call.
#[must_use]
pub fn tool_call(id: &str, title: &str, input: Json) -> Action {
    Action::Update(
        json!({ "sessionUpdate": "tool_call", "toolCallId": id, "title": title, "kind": "other", "status": "in_progress", "rawInput": input }),
    )
}

/// An update that ends a tool call.
#[must_use]
pub fn tool_done(id: &str, status: &str, text: &str) -> Action {
    Action::Update(
        json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status,
        "content": [ { "type": "content", "content": { "type": "text", "text": text } } ] }),
    )
}

/// A request to be allowed a tool call, with the usual two options.
#[must_use]
pub fn permission(call: &str, tag: &'static str) -> Action {
    Action::Ask {
        method: "session/request_permission",
        params: json!({ "sessionId": "$SESSION", "toolCall": { "toolCallId": call }, "options": [
            { "optionId": "allow-once", "name": "Allow once", "kind": "allow_once" },
            { "optionId": "reject-once", "name": "Reject", "kind": "reject_once" } ] }),
        tag,
    }
}
