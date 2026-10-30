//! What the tests of the scripted adapter share: an adapter with a home of its own, sessions opened the way the connector opens them, and the events a turn makes read back in the order they came, each with the moment it came at in the paused time of the test.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone as _;
use futures::StreamExt as _;
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use orch_agent_contract::session::{
    ExecAccess, ProviderAccess, SessionMode, SessionOpened, SessionSettings, SessionSpec, ThreadId,
    TurnId, TurnStart,
};
use orch_types::{Clock, Id, Json, ManualClock, new_id};
use serde_json::json;

/// How long a test waits for the next event before it decides nothing more is coming: longer than the
/// longest wait the adapter has (two minutes for a tool of Orch). The time is the paused time of the
/// test, which moves on by itself while every task waits, so the wait costs nothing.
pub const QUIET: Duration = Duration::from_secs(300);

/// One event the adapter reported, as the plugin transport writes it.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The name: the kind for a scoped session/request event, otherwise `provider.login`, `provider.facts`,
    /// `harness.descriptor` or `harness.status`.
    pub name: String,
    /// The body, without the thread and segment for a scoped session/request event.
    pub body: Json,
    /// The thread, for a session event or model-request report.
    pub thread: Option<ThreadId>,
    /// The segment, for a session event or model-request report.
    pub segment: Option<Id>,
    /// How long after the adapter was started the test took it.
    pub at: Duration,
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

/// The events of one name, in order.
#[must_use]
pub fn of<'a>(events: &'a [Seen], name: &str) -> Vec<&'a Seen> {
    events.iter().filter(|event| event.is(name)).collect()
}

/// Raw model observations, retaining nullable counts, boundaries and terminal outcomes.
#[must_use]
pub fn model_requests(events: &[Seen]) -> Vec<orch_agent_contract::requests::UsageRequest> {
    of(events, "model.request")
        .iter()
        .map(|event| serde_json::from_value(event.body["request"].clone()).unwrap())
        .collect()
}

/// The text of every agent message that was completed, in order.
#[must_use]
pub fn said(events: &[Seen]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.is("item.completed"))
        .filter_map(|event| event.body["payload"]["text"].as_str())
        .map(str::to_owned)
        .collect()
}

/// The `item.started` event of an item, by its native reference.
#[must_use]
pub fn started<'a>(events: &'a [Seen], native_ref: &str) -> &'a Seen {
    events
        .iter()
        .find(|event| event.is("item.started") && event.body["native_ref"] == native_ref)
        .unwrap_or_else(|| panic!("no item {native_ref} was started"))
}

/// The `item.completed` event of an item, by its native reference.
#[must_use]
pub fn completed<'a>(events: &'a [Seen], native_ref: &str) -> &'a Seen {
    events
        .iter()
        .find(|event| event.is("item.completed") && event.body["native_ref"] == native_ref)
        .unwrap_or_else(|| panic!("no item {native_ref} was completed"))
}

/// The items that were started, as `(native_ref, kind)`, in order.
#[must_use]
pub fn items(events: &[Seen]) -> Vec<(String, String)> {
    events
        .iter()
        .filter(|event| event.is("item.started"))
        .map(|event| {
            (
                event.body["native_ref"].as_str().unwrap().to_owned(),
                event.body["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A session the test opened.
#[derive(Debug, Clone)]
pub struct Session {
    /// The thread.
    pub thread: ThreadId,
    /// The segment.
    pub segment: Id,
    /// The harness's reference for it.
    pub native_ref: String,
}

/// The adapter under test, with its home and its working directory.
pub struct Fake {
    /// The adapter.
    pub adapter: FakeAdapter,
    /// What it reports.
    pub events: EventStream,
    /// Its home: `sessions/`, `exports/`, `outputs/` and the counters.
    pub home: tempfile::TempDir,
    /// The directory the sessions work in.
    pub work: tempfile::TempDir,
    /// The agent profile.
    pub agent: Id,
    /// The provider.
    pub provider: Id,
    /// The wall clock of the adapter, which a test moves by hand.
    pub clock: ManualClock,
    /// What starting answered.
    pub started: AgentStarted,
    sink: EventSink,
    began: tokio::time::Instant,
}

impl Fake {
    /// An adapter with the profile configuration `{}`.
    pub async fn start() -> Self {
        Self::start_with(json!({})).await
    }

    /// An adapter started with `config` as its profile's configuration.
    pub async fn start_with(config: Json) -> Self {
        let home = tempfile::tempdir().unwrap();
        Self::start_in(home, config).await
    }

    /// An adapter started on a home that exists.
    pub async fn start_in(home: tempfile::TempDir, config: Json) -> Self {
        let clock = ManualClock::new(
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
                .unwrap(),
        );
        let mut adapter = FakeAdapter::new(Arc::new(clock.clone()) as Arc<dyn Clock>);
        let (sink, events) = EventSink::channel();
        let agent = new_id();
        let provider = new_id();
        let started = adapter
            .start(
                Self::agent_start(provider, agent, home.path(), config),
                sink.clone(),
            )
            .await
            .unwrap();
        Self {
            adapter,
            events,
            home,
            work: tempfile::tempdir().unwrap(),
            agent,
            provider,
            clock,
            started,
            sink,
            began: tokio::time::Instant::now(),
        }
    }

    /// What the connector gives an adapter to start a harness.
    #[must_use]
    pub fn agent_start(provider: Id, agent: Id, home: &Path, config: Json) -> AgentStart {
        serde_json::from_value(json!({
            "provider_id": provider, "agent_id": agent, "harness_kind": "fake",
            "profile": { "harness_version": null, "config": config, "env": {} },
            "home": home.display().to_string(),
            "provider_config": null,
            "loopback": { "base_url": "http://127.0.0.1:1", "token": "t".repeat(64) },
            "binary": null
        }))
        .unwrap()
    }

    /// The sink the adapter reports into (for a second adapter that shares the stream).
    #[must_use]
    pub fn sink(&self) -> EventSink {
        self.sink.clone()
    }

    /// The home's path.
    #[must_use]
    pub fn home_path(&self) -> PathBuf {
        self.home.path().to_owned()
    }

    /// The working directory's path, as a session is told it.
    #[must_use]
    pub fn work_path(&self) -> String {
        self.work.path().display().to_string()
    }

    /// The file of a session in the home.
    #[must_use]
    pub fn file_of(&self, native_ref: &str) -> PathBuf {
        self.home
            .path()
            .join("sessions")
            .join(format!("{native_ref}.json"))
    }

    /// The spec of a session in the working directory, opened as `mode`.
    #[must_use]
    pub fn spec(&self, mode: SessionMode) -> SessionSpec {
        let directory = self.work_path();
        SessionSpec {
            settings: SessionSettings {
                agent_id: self.agent,
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
                model: "fake-large".to_owned(),
                effort: Some("medium".to_owned()),
                params: json!({}),
                approval_mode: ApprovalMode::OnRequest,
                sandbox_mode: SandboxMode::Workspace,
                instructions: "You are working inside Orch.".to_owned(),
                available_models: vec!["fake-large".to_owned()],
                subagent_limits: Default::default(),
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
                provider_id: self.provider,
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

    /// Opens a session by `spec`, takes the `session.opened` it reports, and returns what opening answered
    /// beside it.
    pub async fn open_full(&mut self, spec: SessionSpec) -> (Session, SessionOpened) {
        let thread = spec.settings.thread_id;
        let segment = spec.settings.segment_id;
        let opened = self.adapter.open_session(spec).await.unwrap();
        let event = self.next().await;
        assert!(event.is("session.opened"), "{event:?}");
        assert_eq!(event.body["native_session_ref"], opened.native_session_ref);
        assert_eq!(event.body["resumed"], opened.resumed);
        let session = Session {
            thread,
            segment,
            native_ref: opened.native_session_ref.clone(),
        };
        (session, opened)
    }

    /// Opens a session by `spec`, and takes the `session.opened` it reports.
    pub async fn open(&mut self, spec: SessionSpec) -> Session {
        self.open_full(spec).await.0
    }

    /// The same session opened again by its reference, as a new thread's segment would.
    pub async fn resume(&mut self, native_ref: &str) -> Session {
        let mut spec = self.spec(SessionMode::Resume);
        spec.settings.native_session_ref = Some(native_ref.to_owned());
        self.open(spec).await
    }

    /// What a session's file holds.
    #[must_use]
    pub fn file(&self, native_ref: &str) -> Json {
        let bytes = std::fs::read(self.file_of(native_ref)).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// A new session in the working directory.
    pub async fn session(&mut self) -> Session {
        let spec = self.spec(SessionMode::New);
        self.open(spec).await
    }

    /// The turn that says `text`.
    #[must_use]
    pub fn turn_of(&self, session: &Session, text: &str) -> TurnStart {
        TurnStart {
            plan: false,
            thread_id: session.thread,
            turn_id: new_id(),
            text: text.to_owned(),
            images: vec![],
            model: "fake-large".to_owned(),
            effort: Some("high".to_owned()),
            params: json!({ "tenant": "acme" }),
            approval_mode: ApprovalMode::OnRequest,
            sandbox_mode: SandboxMode::Workspace,
        }
    }

    /// Starts a turn and returns at once.
    pub async fn begin(&mut self, session: &Session, text: &str) -> TurnId {
        let turn = self.turn_of(session, text);
        let id = turn.turn_id;
        self.adapter.start_turn(turn).await.unwrap();
        id
    }

    /// Runs a turn to its end: everything from `turn.started` to `turn.completed`, or to the loss of the
    /// session.
    pub async fn say(&mut self, session: &Session, text: &str) -> Vec<Seen> {
        self.begin(session, text).await;
        self.until(|event| event.is("turn.completed") || event.is("session.lost"))
            .await
    }

    /// Runs the lines as a script.
    pub async fn script(&mut self, session: &Session, lines: &[&str]) -> Vec<Seen> {
        let text = format!("#!fake\n{}", lines.join("\n"));
        self.say(session, &text).await
    }

    fn see(&self, event: AdapterEvent) -> Seen {
        let at = self.began.elapsed();
        let (name, mut body) = event.to_wire().unwrap();
        let (thread, segment) = match &event {
            AdapterEvent::Session {
                thread_id,
                segment_id,
                ..
            } => {
                if let Some(members) = body.as_object_mut() {
                    members.remove("thread_id");
                    members.remove("segment_id");
                }
                (Some(*thread_id), Some(*segment_id))
            }
            AdapterEvent::ModelRequest(report) => {
                if let Some(members) = body.as_object_mut() {
                    members.remove("thread_id");
                    members.remove("segment_id");
                }
                (Some(report.thread_id), Some(report.segment_id))
            }
            _ => (None, None),
        };
        Seen {
            name: name.to_owned(),
            body,
            thread,
            segment,
            at,
        }
    }

    /// Signs the harness in by `fake_code` (the account of the tenant `acme`) and takes the events that
    /// tells.
    pub async fn sign_in(&mut self) -> Vec<Seen> {
        use orch_agent_contract::login::{LoginRequest, LoginValues};
        self.adapter
            .login(LoginRequest {
                method: "fake_code".to_owned(),
                values: LoginValues::default(),
                config: json!({ "tenant": "acme" }),
            })
            .await
            .unwrap();
        self.adapter
            .login_input(LoginValues::default())
            .await
            .unwrap();
        self.until(|event| event.is("provider.facts")).await
    }

    /// Everything up to the sign-in's report that it stands in `state` (`awaiting_user`, `completed`,
    /// `failed`, `cancelled`…).
    pub async fn login_state(&mut self, state: &str) -> Vec<Seen> {
        let wanted = state.to_owned();
        self.until(move |event| event.is("provider.login") && event.body["state"] == wanted)
            .await
    }

    /// The next event; the test fails if none comes for [`QUIET`] of its paused time.
    pub async fn next(&mut self) -> Seen {
        match tokio::time::timeout(QUIET, self.events.next()).await {
            Ok(Some(event)) => self.see(event),
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
                    let event = self.see(event);
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

    /// Everything that comes until nothing has for a minute of the paused time.
    pub async fn quiet(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        while let Ok(Some(event)) = tokio::time::timeout(QUIET, self.events.next()).await {
            let event = self.see(event);
            seen.push(event);
        }
        seen
    }

    /// What has been reported already and not read, without waiting.
    pub fn queued(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            let event = self.see(event);
            seen.push(event);
        }
        seen
    }

    /// Lets the paused time run on by `millis` and reads what was reported meanwhile.
    pub async fn wait(&mut self, millis: u64) -> Vec<Seen> {
        tokio::time::sleep(Duration::from_millis(millis)).await;
        self.queued()
    }
}
