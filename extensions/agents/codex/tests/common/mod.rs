//! The rig of the tests that run the real Codex in Docker against the scripted model server.
//!
//! The harness is run only inside a container of the image `orch-dev-harness`, named
//! `orch-test-codex-<label>-<launcher>-<n>` (the launcher's own process and number, so that two tests of one
//! label never ask for one name) and removed when the test ends, whether it passed or not. The container
//! shares the host's network, so that it reaches the scripted model server, which listens on a port of the
//! loopback in 41700..42000; a home and a workspace made for the test are mounted at their own paths, so that
//! what the adapter writes into the home is what the harness reads. No key is involved anywhere: the token the
//! harness presents to the server is made up for the test.
//! The input-prefix binary can instead run wholly inside an owned container:
//! its explicit guarded mode uses the production direct launcher and no mounts.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test binary uses some of the rig")]

pub mod fake;
pub mod relay;
pub mod remote;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orch_adapter_codex::CodexAdapter;
use orch_adapter_codex::process::{
    DirectLauncher, HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl,
    ProcessExit,
};
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use orch_agent_contract::events::SessionEvent;
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::{
    ExecAccess, ProviderAccess, SessionMode, SessionOpened, SessionSettings, SessionSpec, ThreadId,
    TurnStart,
};
use orch_plugin_contract::Secret;
use orch_testkit::doubles::model::{MockConfig, MockLlm};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use tempfile::TempDir;

/// The image the harness runs in.
pub const IMAGE: &str = "orch-dev-harness";

/// The image the tests run the harness in: `ORCH_HARNESS_IMAGE` when it is set (to hold another version of the
/// harness against the recorded failures), else [`IMAGE`].
pub fn image() -> String {
    std::env::var("ORCH_HARNESS_IMAGE")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| IMAGE.to_owned())
}

/// The model the scripted server answers to; the harness does not know it, which is the plain-list shape of
/// its tools.
pub const MODEL: &str = "mock-large";

/// How long a test waits for an event before it fails.
pub const WAIT: Duration = Duration::from_secs(90);

fn prefix_container_mode() -> bool {
    static MODE: OnceLock<bool> = OnceLock::new();
    *MODE.get_or_init(|| {
        let prefix = orch_testkit::prefix_history::container_prefix_proof_mode(module_path!());
        let configuration =
            orch_testkit::native_configuration::container_proof_mode(module_path!());
        prefix || configuration
    })
}

/// Whether the image of the harness is on this machine.
pub fn image_present() -> bool {
    if prefix_container_mode() {
        return true;
    }
    std::process::Command::new("docker")
        .args(["image", "inspect", &image()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Removes containers by name, whatever state they are in.
pub fn remove_containers(names: &[String]) {
    if prefix_container_mode() {
        assert!(
            names.is_empty(),
            "container prefix proof does not support container fault-kill/removal semantics"
        );
        return;
    }
    if names.is_empty() {
        return;
    }
    let _ = std::process::Command::new("docker")
        .arg("rm")
        .arg("-f")
        .args(names)
        .output();
}

struct DockerControl {
    child: tokio::process::Child,
    name: String,
}

#[async_trait]
impl ProcessControl for DockerControl {
    async fn wait(&mut self) -> ProcessExit {
        match self.child.wait().await {
            Ok(status) => ProcessExit {
                code: status.code(),
                detail: String::new(),
            },
            Err(error) => ProcessExit {
                code: None,
                detail: error.to_string(),
            },
        }
    }

    fn kill(&mut self) {
        // Killing the client of `docker run` does not stop the container: the container is removed.
        remove_containers(std::slice::from_ref(&self.name));
        let _ = self.child.start_kill();
    }
}

/// How many launchers this process has made: the number names a launcher's containers.
static LAUNCHERS: AtomicU32 = AtomicU32::new(0);

/// Starts the programs of the adapter inside the image.
pub struct DockerLauncher {
    label: String,
    /// This launcher among those of every process: its containers' names hold it.
    instance: String,
    counter: AtomicU32,
    mounts: Vec<PathBuf>,
    network: String,
    strict: bool,
    pub containers: Mutex<Vec<String>>,
    /// The programs the adapter asked to be run, with their arguments, for the tests that look at them.
    pub launched: Mutex<Vec<LaunchRequest>>,
}

impl DockerLauncher {
    pub fn new(label: &str, mounts: Vec<PathBuf>) -> Self {
        Self {
            label: label.to_owned(),
            instance: format!(
                "{}-{}",
                std::process::id(),
                LAUNCHERS.fetch_add(1, Ordering::SeqCst)
            ),
            counter: AtomicU32::new(0),
            mounts,
            network: "host".to_owned(),
            strict: true,
            containers: Mutex::new(Vec::new()),
            launched: Mutex::new(Vec::new()),
        }
    }

    /// The same launcher with the container cut off from every network.
    pub fn without_network(mut self) -> Self {
        assert!(
            !prefix_container_mode(),
            "container prefix proof does not emulate a per-launch network change"
        );
        self.network = "none".to_owned();
        self
    }

    /// The same launcher with the workspace left out of the container: the harness cannot reach it, and a
    /// session's tools are carried out elsewhere.
    pub fn without_workspace(mut self) -> Self {
        assert!(
            !prefix_container_mode(),
            "container prefix proof does not emulate hiding its workspace"
        );
        self.mounts.retain(|mount| {
            !mount
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("orch-codex-ws-"))
        });
        self
    }

    /// The same launcher not asking the harness to refuse unknown settings.
    pub fn lenient(mut self) -> Self {
        assert!(
            !prefix_container_mode(),
            "container prefix proof keeps the strict native configuration"
        );
        self.strict = false;
        self
    }
}

#[async_trait]
impl Launcher for DockerLauncher {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        if prefix_container_mode() {
            assert!(
                self.strict && self.network == "host",
                "container prefix proof requires the ordinary launcher shape"
            );
            let mut request = request;
            let native = orch_testkit::prefix_history::prefix_native_args_offset(
                &request.program,
                &request.args,
                "codex",
                &self.mounts,
            )
            .unwrap_or_else(|reason| panic!("{reason}"));
            if request.args.get(native).map(String::as_str) == Some("app-server") {
                request
                    .args
                    .insert(native + 1, "--strict-config".to_owned());
            }
            self.launched.lock().unwrap().push(request.clone());
            return DirectLauncher.launch(request).await;
        }
        let number = self.counter.fetch_add(1, Ordering::SeqCst);
        let name = format!("orch-test-codex-{}-{}-{number}", self.label, self.instance);
        self.containers.lock().unwrap().push(name.clone());
        self.launched.lock().unwrap().push(request.clone());
        let mut command = tokio::process::Command::new("docker");
        command.args([
            "run",
            "-i",
            "--rm",
            "--name",
            &name,
            "--network",
            &self.network,
        ]);
        for mount in &self.mounts {
            command.arg("-v").arg(format!("{0}:{0}", mount.display()));
        }
        for key in request.env.keys() {
            command.arg("-e").arg(key);
        }
        command.envs(&request.env);
        command.arg(image()).arg(&request.program);
        let mut args = request.args.clone();
        if self.strict && args.first().map(String::as_str) == Some("app-server") {
            args.insert(1, "--strict-config".to_owned());
        }
        command.args(&args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|source| LaunchError::Spawn {
            program: "docker".to_owned(),
            source,
        })?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        Ok(HarnessProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: Some(Box::new(stderr)),
            control: Box::new(DockerControl { child, name }),
        })
    }
}

/// A scripted model server on a free port of 41700..42000.
pub async fn start_mock(config: MockConfig) -> MockLlm {
    for port in 41700..42000u16 {
        if let Ok(mock) =
            MockLlm::start_on(SocketAddr::from(([127, 0, 0, 1], port)), config.clone()).await
        {
            return mock;
        }
    }
    panic!("no free port in 41700..42000 for the scripted model server");
}

/// A key for a session: 32 lowercase hexadecimal digits.
pub fn session_key() -> String {
    format!("{:032x}", new_id().as_u128())
}

/// Everything a test needs: a harness in Docker, the server it talks to, and the adapter in between.
pub struct Rig {
    pub adapter: CodexAdapter,
    pub events: EventStream,
    pub sink: EventSink,
    pub mock: MockLlm,
    pub launcher: Arc<DockerLauncher>,
    pub home: TempDir,
    pub workspace: TempDir,
    pub token: String,
    pub agent_id: Id,
    pub provider_id: Id,
    pub environment_id: Id,
    pub backlog: Vec<AdapterEvent>,
    /// The port of the listener the harness is told is the connector's, where it is not the scripted server's.
    pub loopback_port: Option<u16>,
    prefix_proof_succeeded: bool,
    prefix_evidence_files: Vec<PathBuf>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        if matches!(
            module_path!().split("::").next(),
            Some("input_prefix" | "session_configuration")
        ) && !self.prefix_proof_succeeded
        {
            let bodies: Vec<_> = self
                .model_requests()
                .into_iter()
                .map(|request| request.body)
                .collect();
            match orch_testkit::prefix_history::preserve_prefix_failure(
                "codex",
                self.home.path(),
                self.workspace.path(),
                &self.prefix_evidence_files,
                &bodies,
                &[&self.token],
                orch_adapter_codex::protocol::MAX_FRAME_BYTES,
            ) {
                Ok(_) => {}
                Err(_) => eprintln!("prefix failure evidence could not be fully retained"),
            }
        }
        let names = self.launcher.containers.lock().unwrap().clone();
        remove_containers(&names);
    }
}

impl Rig {
    /// A rig, or `None` (with a message) where the image of the harness is not on this machine.
    pub async fn start(label: &str) -> Option<Self> {
        Self::start_with(label, |launcher| launcher).await
    }

    /// A rig whose launcher is shaped by `shape`.
    pub async fn start_with(
        label: &str,
        shape: impl FnOnce(DockerLauncher) -> DockerLauncher,
    ) -> Option<Self> {
        if !image_present() {
            eprintln!(
                "SKIPPED {label}: the image {IMAGE} is not on this machine (docker image inspect failed); \
                 this test runs the real Codex in Docker"
            );
            return None;
        }
        let home = tempfile::Builder::new()
            .prefix("orch-codex-home-")
            .tempdir()
            .unwrap();
        let workspace = tempfile::Builder::new()
            .prefix("orch-codex-ws-")
            .tempdir()
            .unwrap();
        let token = format!("tok-{:032x}", new_id().as_u128());
        let mock = start_mock(MockConfig::default().with_credential(&token)).await;
        let directories = vec![home.path().to_path_buf(), workspace.path().to_path_buf()];
        let launcher = shape(DockerLauncher::new(label, directories.clone()));
        if prefix_container_mode() {
            assert_eq!(
                launcher.mounts, directories,
                "container prefix proof retains both generated home/workspace directories"
            );
            assert!(
                launcher.strict && launcher.network == "host",
                "container prefix proof refuses a special launcher shape"
            );
        }
        let launcher = Arc::new(launcher);
        let adapter = CodexAdapter::with_launcher(Arc::clone(&launcher) as Arc<dyn Launcher>);
        let (sink, events) = EventSink::channel();
        Some(Self {
            adapter,
            events,
            sink,
            mock,
            launcher,
            home,
            workspace,
            token,
            agent_id: new_id(),
            provider_id: new_id(),
            environment_id: new_id(),
            backlog: Vec::new(),
            loopback_port: None,
            prefix_proof_succeeded: false,
            prefix_evidence_files: Vec::new(),
        })
    }

    pub fn home_path(&self) -> &Path {
        self.home.path()
    }

    pub fn prefix_evidence_file(&mut self, path: &Path) {
        self.prefix_evidence_files.push(path.to_owned());
    }
    pub fn prefix_proof_succeeded(&mut self) {
        self.prefix_proof_succeeded = true;
    }
    pub fn keep_prefix_evidence(&self) -> std::io::Result<PathBuf> {
        let bodies: Vec<_> = self
            .model_requests()
            .into_iter()
            .map(|request| request.body)
            .collect();
        orch_testkit::prefix_history::preserve_prefix_failure(
            "codex",
            self.home.path(),
            self.workspace.path(),
            &self.prefix_evidence_files,
            &bodies,
            &[&self.token],
            orch_adapter_codex::protocol::MAX_FRAME_BYTES,
        )
    }

    pub fn workspace_path(&self) -> &Path {
        self.workspace.path()
    }

    pub fn agent_start(&self) -> AgentStart {
        AgentStart {
            provider_id: self.provider_id,
            agent_id: self.agent_id,
            harness_kind: "codex".to_owned(),
            profile: AgentProfile::default(),
            home: Some(self.home.path().to_string_lossy().into_owned()),
            provider_config: json!({}),
            loopback: Loopback::new(
                self.loopback_port
                    .unwrap_or_else(|| self.mock.addr().port()),
                Secret::new(self.token.clone()),
            ),
            binary: None,
        }
    }

    /// Starts the harness.
    pub async fn boot(&mut self) -> AgentStarted {
        let start = self.agent_start();
        let started = self.adapter.start(start, self.sink.clone()).await.unwrap();
        if prefix_container_mode() {
            assert_eq!(
                started.harness_version, "0.165.0",
                "container prefix proof requires the exact pinned native CLI"
            );
        }
        started
    }

    /// The description of a session of a new conversation in the rig's workspace.
    pub fn spec(&self, thread: ThreadId, mode: SessionMode, native: Option<&str>) -> SessionSpec {
        let directory = self.workspace.path().to_string_lossy().into_owned();
        plain_spec(
            &directory,
            self.agent_id,
            self.provider_id,
            self.environment_id,
            thread,
            mode,
            native,
        )
    }

    /// Opens a session of a new conversation.
    pub async fn open_new(&mut self, thread: ThreadId) -> SessionOpened {
        let spec = self.spec(thread, SessionMode::New, None);
        self.adapter.open_session(spec).await.unwrap()
    }

    /// A turn that says `text`.
    pub fn turn(&self, thread: ThreadId, text: &str) -> TurnStart {
        TurnStart {
            plan: false,
            thread_id: thread,
            turn_id: new_id(),
            text: text.to_owned(),
            images: Vec::new(),
            model: MODEL.to_owned(),
            effort: None,
            params: json!({}),
            approval_mode: ApprovalMode::Bypass,
            sandbox_mode: SandboxMode::Open,
        }
    }

    /// The next event, or a failure when none comes in time.
    pub async fn next_event(&mut self) -> AdapterEvent {
        if !self.backlog.is_empty() {
            return self.backlog.remove(0);
        }
        tokio::time::timeout(WAIT, self.events.next())
            .await
            .expect("an event within the wait")
            .expect("the stream of events is open")
    }

    /// Takes events up to and including the first for which `done` holds.
    pub async fn until(&mut self, done: impl Fn(&AdapterEvent) -> bool) -> Vec<AdapterEvent> {
        let mut taken = Vec::new();
        loop {
            let event = self.next_event().await;
            let finished = done(&event);
            taken.push(event);
            if finished {
                return taken;
            }
        }
    }

    /// Takes events until a turn has ended, and returns the session events of it.
    pub async fn until_turn_ends(&mut self) -> Vec<SessionEvent> {
        let taken = self
            .until(|event| {
                matches!(
                    event,
                    AdapterEvent::Session {
                        event: SessionEvent::TurnCompleted(_),
                        ..
                    }
                )
            })
            .await;
        session_events(&taken)
    }

    /// Takes events up to the first request for approval, and returns the events before it and the request.
    pub async fn until_approval(
        &mut self,
    ) -> (
        Vec<SessionEvent>,
        orch_agent_contract::events::ApprovalRequested,
    ) {
        let taken = self
            .until(|event| {
                matches!(
                    event,
                    AdapterEvent::Session {
                        event: SessionEvent::ApprovalRequested(_),
                        ..
                    }
                )
            })
            .await;
        let mut events = session_events(&taken);
        let Some(SessionEvent::ApprovalRequested(request)) = events.pop() else {
            panic!("a request for approval ends what was taken");
        };
        (events, request)
    }

    /// Answers a request for approval by choosing one of its options.
    pub async fn choose(&mut self, thread: ThreadId, request_ref: &str, option: &str) {
        self.adapter
            .resolve_approval(
                thread,
                request_ref,
                orch_agent_contract::session::ApprovalDecision::Option {
                    option_id: option.to_owned(),
                    note: None,
                },
            )
            .await
            .unwrap();
    }

    /// Runs a turn that says `text` to its end and returns the session events of it.
    pub async fn run_turn(&mut self, thread: ThreadId, text: &str) -> Vec<SessionEvent> {
        let turn = self.turn(thread, text);
        self.adapter.start_turn(turn).await.unwrap();
        self.until_turn_ends().await
    }

    /// Everything that is waiting to be read, without waiting for more.
    pub fn drain(&mut self) -> Vec<AdapterEvent> {
        let mut taken: Vec<AdapterEvent> = std::mem::take(&mut self.backlog);
        while let Ok(event) = self.events.try_recv() {
            taken.push(event);
        }
        taken
    }

    /// The requests the scripted server received for the model, in order.
    pub fn model_requests(&self) -> Vec<orch_testkit::doubles::model::RecordedRequest> {
        self.mock
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST" && request.path.ends_with("/responses"))
            .collect()
    }
}

/// The texts of the messages of the person and of the agent that a request to the model carries, in order.
pub fn prompt_texts(request: &orch_testkit::doubles::model::RecordedRequest) -> Vec<String> {
    request.body["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["type"] == "message" && matches!(item["role"].as_str(), Some("user" | "assistant"))
        })
        .flat_map(|item| {
            item["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|part| part["text"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The session events among `events`, in order.
pub fn session_events(events: &[AdapterEvent]) -> Vec<SessionEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            AdapterEvent::Session { event, .. } => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// The kinds of the events, as the wire writes them.
pub fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events.iter().map(SessionEvent::kind).collect()
}

/// A script for the scripted server, as the text of a turn: a line `#!mock` and the directives after it.
pub fn script(prompt: &str, directives: &[&str]) -> String {
    format!("{prompt}\n#!mock\n{}\n", directives.join("\n"))
}

/// Reads a member of a payload that an event carries as JSON.
pub fn member<'a>(value: &'a Json, path: &[&str]) -> &'a Json {
    path.iter().fold(value, |at, key| &at[*key])
}

/// The environment variables of a launch request, for the tests that look at what was passed.
pub fn env_of(request: &LaunchRequest) -> &BTreeMap<String, String> {
    &request.env
}

/// An `auth.json` of the shape the harness writes, with identity claims and tokens that belong to nobody.
pub fn fake_auth_json(email: &str, plan: &str, account: &str) -> String {
    use base64::Engine as _;
    let part =
        |value: &Json| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    let jwt = |claims: &Json| {
        format!(
            "{}.{}.{}",
            part(&json!({ "alg": "RS256", "typ": "JWT" })),
            part(claims),
            part(&json!("signature"))
        )
    };
    let id_token = jwt(&json!({
        "iss": "https://auth.example.test", "aud": "app", "sub": "user-1", "email": email,
        "exp": 4_102_444_800_i64, "iat": 1_791_676_800_i64,
        "https://api.openai.com/auth": { "chatgpt_plan_type": plan, "chatgpt_account_id": account, "chatgpt_user_id": "user-1" }
    }));
    let access = jwt(&json!({ "iss": "https://auth.example.test", "exp": 4_102_444_800_i64 }));
    json!({
        "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
        "tokens": { "id_token": id_token, "access_token": access, "refresh_token": "not-a-real-refresh-token", "account_id": account },
        "last_refresh": "2026-10-11T10:00:00Z"
    })
    .to_string()
}

/// The description of a session in `directory`, forwarded to the connector's listener, run by the harness
/// itself, asking nothing and confined by nothing.
pub fn plain_spec(
    directory: &str,
    agent_id: Id,
    provider_id: Id,
    environment_id: Id,
    thread: ThreadId,
    mode: SessionMode,
    native: Option<&str>,
) -> SessionSpec {
    SessionSpec {
        settings: SessionSettings {
            agent_id,
            thread_id: thread,
            segment_id: new_id(),
            mode,
            native_session_ref: native.map(str::to_owned),
            upto_turn_ordinal: None,
            workspace_root: directory.to_owned(),
            instruction_root_marker: false,
            directory: directory.to_owned(),
            cwd: directory.to_owned(),
            mounts: Vec::new(),
            model: MODEL.to_owned(),
            effort: None,
            params: json!({}),
            approval_mode: ApprovalMode::Bypass,
            sandbox_mode: SandboxMode::Open,
            instructions: "You are working inside Orch.".to_owned(),
            available_models: vec![MODEL.to_owned()],
            subagent_limits: Default::default(),
            memory: None,
            arrangement: ArrangementType::KeySeparated,
            orch_tools: Vec::new(),
            orch_tool_schemas: Default::default(),
            raw_events: false,
            side_reading: None,
            workspace_configuration: None,
        },
        session_key: session_key(),
        archive: None,
        history: None,
        provider: ProviderAccess::Forwarded {
            provider_id,
            protocol: "openai-responses".to_owned(),
        },
        exec: ExecAccess {
            executor: None,
            tool_set: None,
            environment_id,
            generation: 1,
        },
        mcp: Vec::new(),
        skills: None,
    }
}
