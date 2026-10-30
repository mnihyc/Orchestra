//! What the tests of the adapter stand on: the real harness in a container, a scripted model server on the
//! loopback, and the adapter between them.
//!
//! The harness is the program of the image `orch-dev-harness`, started with `docker run -i` the way a plain
//! program would be started in production: the adapter builds the same launch description and a launcher
//! turns it into a container command. Directories are mounted at the same path inside as outside, so that
//! every path the adapter computes is true in both places. A test whose Docker or image is absent says so
//! and passes without running.
//! Input-prefix and session-configuration binaries have separate guarded in-container modes:
//! it uses the production direct launcher without creating nested containers or mounts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses a part of this")]

pub mod forwarder;
pub mod mcp;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_adapter_claude_code::process::{DirectLauncher, LaunchSpec, Launcher};
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use orch_agent_contract::events::{ApprovalRequested, ItemContent, SessionEvent, TurnOutcome};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::login::LoginState;
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::{
    ApprovalDecision, ExecAccess, ProviderAccess, SessionMode, SessionOpened, SessionSettings,
    SessionSpec, ThreadId, TurnId, TurnStart,
};
use orch_plugin_contract::Secret;
use orch_testkit::doubles::model::{MockConfig, MockLlm};
use orch_types::{Id, SystemClock, new_id};
use serde_json::json;
use tempfile::TempDir;
use tokio::process::Command;

/// The image the harness runs in.
pub const IMAGE: &str = "orch-dev-harness";

/// The image the tests run the harness in: `ORCH_HARNESS_IMAGE` when it is set (to hold another version of the
/// harness against the recorded failures), else [`IMAGE`].
#[must_use]
pub fn image() -> String {
    std::env::var("ORCH_HARNESS_IMAGE")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| IMAGE.to_owned())
}

/// The key of the sessions of the tests: 128 bits as 32 hexadecimal digits.
pub const SESSION_KEY: &str = "0123456789abcdef0123456789abcdef";

/// The token the harness presents to the model server, which the server insists on.
pub const TOKEN: &str = "tttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttt";

static CONTAINERS: AtomicU64 = AtomicU64::new(0);
static PORTS: AtomicU64 = AtomicU64::new(0);

fn prefix_container_mode() -> bool {
    static MODE: OnceLock<bool> = OnceLock::new();
    *MODE.get_or_init(|| {
        let prefix = orch_testkit::prefix_history::container_prefix_proof_mode(module_path!());
        let configuration =
            orch_testkit::native_configuration::container_proof_mode(module_path!());
        prefix || configuration
    })
}

/// Starts a container for each program, names it, and removes every one of them when it is dropped.
#[derive(Debug)]
pub struct DockerLauncher {
    mounts: Vec<PathBuf>,
    names: Mutex<Vec<String>>,
    prefix: String,
    /// Original native launch descriptions, including the prefix wire gate.
    pub launched: Mutex<Vec<LaunchSpec>>,
}

impl DockerLauncher {
    fn new(mounts: Vec<PathBuf>) -> Self {
        Self {
            mounts,
            names: Mutex::new(Vec::new()),
            launched: Mutex::new(Vec::new()),
            prefix: format!(
                "orch-test-claude-{}-{}",
                std::process::id(),
                new_id().simple()
            ),
        }
    }

    /// Kills every container this launcher started, as a crash of the machine would.
    pub fn kill_all(&self) {
        assert!(
            !prefix_container_mode(),
            "container prefix proof does not emulate container fault-kill semantics"
        );
        let names = self.names.lock().unwrap().clone();
        for name in names {
            let _ = StdCommand::new("docker")
                .args(["kill", &name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    /// Removes every container this launcher started, running or not.
    fn remove_all(&self) {
        let names = self.names.lock().unwrap().clone();
        if prefix_container_mode() {
            assert!(
                names.is_empty(),
                "container prefix proof cannot own nested containers"
            );
            return;
        }
        for name in names {
            let _ = StdCommand::new("docker")
                .args(["rm", "-f", &name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

impl Launcher for DockerLauncher {
    fn command(&self, spec: &LaunchSpec) -> Command {
        if prefix_container_mode() {
            orch_testkit::prefix_history::prefix_native_args_offset(
                &spec.program,
                &spec.args,
                "claude",
                &self.mounts,
            )
            .unwrap_or_else(|reason| panic!("{reason}"));
            self.launched.lock().unwrap().push(spec.clone());
            return DirectLauncher.command(spec);
        }
        self.launched.lock().unwrap().push(spec.clone());
        let name = format!(
            "{}-{}",
            self.prefix,
            CONTAINERS.fetch_add(1, Ordering::Relaxed)
        );
        self.names.lock().unwrap().push(name.clone());
        let mut command = Command::new("docker");
        command.args(["run", "-i", "--rm", "--name", &name, "--network", "host"]);
        for mount in &self.mounts {
            command.arg("-v").arg(format!("{0}:{0}", mount.display()));
        }
        command.arg("-w").arg(&spec.cwd);
        // What names a place on the host (a search path, a home, a shell) means nothing in the container.
        const HOST_ONLY: [&str; 9] = [
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "SHELL",
            "TMPDIR",
            "XDG_RUNTIME_DIR",
            "TERM",
            "NO_COLOR",
        ];
        for (name, value) in &spec.env {
            if !HOST_ONLY.contains(&name.as_str()) {
                command.arg("-e").arg(format!("{name}={value}"));
            }
        }
        command.arg(image()).arg(&spec.program).args(&spec.args);
        command
    }
}

impl Drop for DockerLauncher {
    fn drop(&mut self) {
        self.remove_all();
    }
}

/// Whether the image of the harness is there.
fn image_is_there() -> bool {
    if prefix_container_mode() {
        return true;
    }
    StdCommand::new("docker")
        .args(["image", "inspect", &image()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A scripted model server on a free port of the range reserved for tests (41000 to 41999). Another test, or
/// another person's stack, may hold any of them: the next free one is taken.
pub async fn model_server() -> MockLlm {
    let start = PORTS.fetch_add(1, Ordering::Relaxed);
    let base = u64::from(std::process::id()) * 31 + start * 17;
    for attempt in 0..400 {
        let port = 41_000 + ((base + attempt * 7) % 1000);
        let address = SocketAddr::from(([127, 0, 0, 1], u16::try_from(port).unwrap()));
        let config = MockConfig::default().with_credential(TOKEN);
        if let Ok(server) = MockLlm::start_on(address, config).await {
            return server;
        }
    }
    panic!("no free port between 41000 and 41999");
}

/// The harness, the model server and the adapter, ready to be used.
pub struct Rig {
    pub adapter: ClaudeCodeAdapter,
    pub events: EventStream,
    sink: EventSink,
    pub model: MockLlm,
    pub home: TempDir,
    pub work: TempDir,
    pub launcher: Arc<DockerLauncher>,
    pub thread: ThreadId,
    pub segment: Id,
    pub session_key: String,
    /// A program to start instead of the harness, for the tests of what the adapter does when a process
    /// misbehaves in ways the real one cannot be made to.
    pub program: Option<String>,
    prefix_proof_succeeded: bool,
}

impl Rig {
    /// A rig, or `None` when the tests cannot run here (and a line says why).
    pub async fn new() -> Option<Self> {
        if !image_is_there() {
            eprintln!(
                "skipped: Docker or the image {IMAGE} (Node, git, ripgrep and the harness installed, user 1000) is not available here"
            );
            return None;
        }
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let launcher = Arc::new(DockerLauncher::new(vec![
            home.path().to_path_buf(),
            work.path().to_path_buf(),
        ]));
        let (sink, events) = EventSink::channel();
        let model = model_server().await;
        let adapter = ClaudeCodeAdapter::with_launcher(Arc::new(SystemClock), launcher.clone());
        Some(Self {
            adapter,
            events,
            sink,
            model,
            home,
            work,
            launcher,
            thread: new_id(),
            segment: new_id(),
            session_key: SESSION_KEY.to_owned(),
            program: None,
            prefix_proof_succeeded: false,
        })
    }

    /// A rig whose harness is the program given, started directly: nothing of Docker is needed.
    pub async fn standing_in(program: &Path) -> Self {
        assert!(
            !prefix_container_mode(),
            "container prefix proof requires the pinned CLI, not a stand-in process"
        );
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let launcher = Arc::new(DockerLauncher::new(vec![
            home.path().to_path_buf(),
            work.path().to_path_buf(),
        ]));
        let (sink, events) = EventSink::channel();
        let model = model_server().await;
        Self {
            adapter: ClaudeCodeAdapter::new(Arc::new(SystemClock)),
            events,
            sink,
            model,
            home,
            work,
            launcher,
            thread: new_id(),
            segment: new_id(),
            session_key: SESSION_KEY.to_owned(),
            program: Some(program.display().to_string()),
            prefix_proof_succeeded: false,
        }
    }

    /// The address of the connector's listener, which here is the model server.
    pub fn loopback(&self) -> Loopback {
        Loopback::new(self.model.addr().port(), Secret::new(TOKEN))
    }

    pub fn prefix_proof_succeeded(&mut self) {
        self.prefix_proof_succeeded = true;
    }

    /// Starts the harness with its loopback somewhere else than the model server: the model is then
    /// reached through the address in the profile's environment.
    pub async fn start_beside(&mut self, loopback: Loopback) -> AgentStarted {
        let env = BTreeMap::from([
            ("CLAUDE_CODE_MAX_RETRIES".to_owned(), "1".to_owned()),
            ("ANTHROPIC_BASE_URL".to_owned(), self.model.url()),
            ("ANTHROPIC_AUTH_TOKEN".to_owned(), TOKEN.to_owned()),
        ]);
        self.start_on(json!({}), env, loopback).await
    }

    /// Starts the harness with its loopback somewhere else than the model server and no address of the model
    /// in its profile: it is given the loopback's, as in production.
    pub async fn start_through(
        &mut self,
        env: BTreeMap<String, String>,
        loopback: Loopback,
    ) -> AgentStarted {
        self.start_on(json!({}), env, loopback).await
    }

    /// Starts the harness with a home of its own and the options of a profile.
    pub async fn start(
        &mut self,
        config: serde_json::Value,
        env: BTreeMap<String, String>,
    ) -> AgentStarted {
        let loopback = self.loopback();
        self.start_on(config, env, loopback).await
    }

    async fn start_on(
        &mut self,
        config: serde_json::Value,
        env: BTreeMap<String, String>,
        loopback: Loopback,
    ) -> AgentStarted {
        if prefix_container_mode() {
            assert!(
                self.program.is_none(),
                "container prefix proof requires its pinned native program"
            );
        }
        let start = AgentStart {
            provider_id: new_id(),
            agent_id: new_id(),
            harness_kind: "claude-code".into(),
            profile: AgentProfile {
                harness_version: None,
                attach: None,
                config,
                env,
            },
            home: Some(self.home.path().display().to_string()),
            provider_config: json!({}),
            loopback,
            binary: self.program.clone(),
        };
        let started = self.adapter.start(start, self.sink.clone()).await.unwrap();
        if prefix_container_mode() {
            assert_eq!(
                started.harness_version, "2.1.312",
                "container prefix proof requires the exact pinned native CLI"
            );
        }
        started
    }

    /// Starts it as a test usually wants: a few retries at most, so that an error resolves quickly.
    pub async fn start_quick(&mut self) -> AgentStarted {
        self.start(
            json!({}),
            BTreeMap::from([("CLAUDE_CODE_MAX_RETRIES".to_owned(), "1".to_owned())]),
        )
        .await
    }

    /// The thread, segment and key the rig speaks for now.
    pub fn lane(&self) -> Lane {
        Lane {
            thread: self.thread,
            segment: self.segment,
            key: self.session_key.clone(),
        }
    }

    /// Speaks for another lane from now on.
    pub fn set_lane(&mut self, lane: &Lane) {
        self.thread = lane.thread;
        self.segment = lane.segment;
        self.session_key = lane.key.clone();
    }

    /// Speaks for a lane that is new: a thread of its own and a session key of its own.
    pub fn new_lane(&mut self) -> Lane {
        let lane = Lane {
            thread: new_id(),
            segment: new_id(),
            key: new_id().simple().to_string(),
        };
        self.set_lane(&lane);
        lane
    }

    /// The text of the last request the model server received, whole.
    pub fn last_request(&self) -> String {
        let requests = self.model.requests();
        requests
            .last()
            .expect("a request was made")
            .body
            .to_string()
    }

    /// The directory of the thread.
    pub fn directory(&self) -> String {
        self.work.path().display().to_string()
    }

    /// The session of a thread working beside its own tools, forwarded through the model server.
    pub fn spec(&self, mode: SessionMode, native_session_ref: Option<&str>) -> SessionSpec {
        SessionSpec {
            settings: SessionSettings {
                agent_id: new_id(),
                thread_id: self.thread,
                segment_id: self.segment,
                mode,
                native_session_ref: native_session_ref.map(str::to_owned),
                upto_turn_ordinal: None,
                workspace_root: self.directory(),
                instruction_root_marker: false,
                directory: self.directory(),
                cwd: self.directory(),
                mounts: Vec::new(),
                model: "mock-large".into(),
                effort: None,
                params: json!({}),
                approval_mode: ApprovalMode::Ask,
                sandbox_mode: SandboxMode::Open,
                instructions: String::new(),
                available_models: vec!["mock-large".to_owned()],
                subagent_limits: Default::default(),
                memory: None,
                arrangement: ArrangementType::KeySeparated,
                orch_tools: Vec::new(),
                orch_tool_schemas: Default::default(),
                raw_events: false,
                side_reading: None,
                workspace_configuration: None,
            },
            session_key: self.session_key.clone(),
            archive: None,
            history: None,
            provider: ProviderAccess::Forwarded {
                provider_id: new_id(),
                protocol: "anthropic-messages".into(),
            },
            exec: ExecAccess {
                executor: None,
                tool_set: None,
                environment_id: new_id(),
                generation: 1,
            },
            mcp: Vec::new(),
            skills: None,
        }
    }

    /// Opens a new session and takes the event that says so.
    pub async fn open(&mut self) -> SessionOpened {
        let spec = self.spec(SessionMode::New, None);
        self.open_with(spec).await
    }

    /// Opens the session as it is described and takes the event that says so.
    pub async fn open_with(&mut self, spec: SessionSpec) -> SessionOpened {
        let opened = self.adapter.open_session(spec).await.unwrap();
        let event = self
            .until(|event| matches!(event, SessionEvent::SessionOpened(_)), 20)
            .await;
        let SessionEvent::SessionOpened(said) = event.last().unwrap() else {
            panic!("the session opened");
        };
        assert_eq!(said.native_session_ref, opened.native_session_ref);
        opened
    }

    /// A turn with the given text, in the mode ask.
    pub fn turn(&self, text: &str) -> TurnStart {
        TurnStart {
            plan: false,
            thread_id: self.thread,
            turn_id: new_id(),
            text: text.to_owned(),
            images: Vec::new(),
            model: "mock-large".into(),
            effort: None,
            params: json!({}),
            approval_mode: ApprovalMode::Ask,
            sandbox_mode: SandboxMode::Open,
        }
    }

    /// Reads events until `done` says one of them is the last, or the time is up.
    pub async fn until(
        &mut self,
        done: impl Fn(&SessionEvent) -> bool,
        secs: u64,
    ) -> Vec<SessionEvent> {
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.events.next()).await {
                Ok(Some(AdapterEvent::Session { event, .. })) => {
                    let last = done(&event);
                    seen.push(event);
                    if last {
                        return seen;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => panic!(
                    "the events did not come in {secs} s; seen so far: {:#?}",
                    seen.iter().map(SessionEvent::kind).collect::<Vec<_>>()
                ),
            }
        }
    }

    /// Reads the progress of a sign-in until `done` says one of its states is the last, or the time is up.
    pub async fn logins_until(
        &mut self,
        done: impl Fn(&LoginState) -> bool,
        secs: u64,
    ) -> Vec<LoginState> {
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.events.next()).await {
                Ok(Some(AdapterEvent::Login(state))) => {
                    let last = done(&state);
                    seen.push(state);
                    if last {
                        return seen;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => panic!(
                    "the sign-in did not get there in {secs} s; seen so far: {:#?}",
                    seen.iter().map(|state| state.state).collect::<Vec<_>>()
                ),
            }
        }
    }

    /// Everything that is waiting now, and what comes in the next moments.
    pub async fn drain(&mut self, quiet_ms: u64) -> Vec<SessionEvent> {
        let mut seen = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(quiet_ms), self.events.next()).await
        {
            if let AdapterEvent::Session { event, .. } = event {
                seen.push(event);
            }
        }
        seen
    }

    /// Reads events to the end of the turn.
    pub async fn until_the_turn_ends(&mut self) -> Vec<SessionEvent> {
        self.until(|event| matches!(event, SessionEvent::TurnCompleted(_)), 90)
            .await
    }

    /// Runs a turn to its end, answering nothing.
    pub async fn run(&mut self, text: &str) -> (TurnId, Vec<SessionEvent>) {
        let turn = self.turn(text);
        let id = turn.turn_id;
        self.adapter.start_turn(turn).await.unwrap();
        (id, self.until_the_turn_ends().await)
    }

    /// Runs a turn to its end, answering every request of the harness as `decide` says.
    pub async fn run_deciding(
        &mut self,
        text: &str,
        mut decide: impl FnMut(&ApprovalRequested) -> ApprovalDecision,
    ) -> (TurnId, Vec<SessionEvent>) {
        let turn = self.turn(text);
        let id = turn.turn_id;
        self.adapter.start_turn(turn).await.unwrap();
        (id, self.deciding_to_the_end(&mut decide).await)
    }

    /// Reads to the end of the running turn, answering requests.
    pub async fn deciding_to_the_end(
        &mut self,
        decide: &mut impl FnMut(&ApprovalRequested) -> ApprovalDecision,
    ) -> Vec<SessionEvent> {
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let event = match tokio::time::timeout(remaining, self.events.next()).await {
                Ok(Some(AdapterEvent::Session { event, .. })) => event,
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => panic!(
                    "the turn did not end; seen: {:#?}",
                    seen.iter().map(SessionEvent::kind).collect::<Vec<_>>()
                ),
            };
            if let SessionEvent::ApprovalRequested(ask) = &event {
                let decision = decide(ask);
                self.adapter
                    .resolve_approval(self.thread, &ask.request_ref, decision)
                    .await
                    .unwrap();
            }
            let over = matches!(event, SessionEvent::TurnCompleted(_));
            seen.push(event);
            if over {
                return seen;
            }
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if matches!(
            module_path!().split("::").next(),
            Some("input_prefix" | "session_configuration")
        ) && !self.prefix_proof_succeeded
        {
            let bodies: Vec<_> = self
                .model
                .requests()
                .into_iter()
                .filter(|request| request.format.is_some())
                .map(|request| request.body)
                .collect();
            match orch_testkit::prefix_history::preserve_prefix_failure(
                "claude",
                self.home.path(),
                self.work.path(),
                &[],
                &bodies,
                &[TOKEN],
                orch_adapter_claude_code::config::FRAME_MAX_BYTES,
            ) {
                Ok(_) => {}
                Err(_) => eprintln!("prefix failure evidence could not be fully retained"),
            }
        }
        self.launcher.remove_all();
    }
}

/// One thread's identity in the rig.
#[derive(Debug, Clone)]
pub struct Lane {
    pub thread: ThreadId,
    pub segment: Id,
    pub key: String,
}

/// The decision that lets the one thing happen.
pub fn allow(_: &ApprovalRequested) -> ApprovalDecision {
    ApprovalDecision::Option {
        option_id: "allow_once".into(),
        note: None,
    }
}

/// The decision that refuses it.
pub fn deny(_: &ApprovalRequested) -> ApprovalDecision {
    ApprovalDecision::Option {
        option_id: "deny".into(),
        note: Some("The person said no.".into()),
    }
}

/// The kinds of events, for a test that wants the order.
pub fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events.iter().map(SessionEvent::kind).collect()
}

/// The text the agent said in the main stream, as the completed items give it.
pub fn said(events: &[SessionEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(done) if done.payload.get("text").is_some() => {
                done.payload["text"].as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// How the turn in `events` ended.
pub fn outcome(events: &[SessionEvent]) -> TurnOutcome {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::TurnCompleted(end) => Some(end.status),
            _ => None,
        })
        .expect("the turn ended")
}

/// The tool calls that were started, by their payload.
pub fn tool_calls(events: &[SessionEvent]) -> Vec<&orch_agent_contract::events::ToolCallPayload> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(started) => match &started.content {
                ItemContent::ToolCall(call) => Some(call),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Whether a path exists.
pub fn exists(path: &Path) -> bool {
    path.exists()
}
