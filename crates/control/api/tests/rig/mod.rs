//! A whole system for the tests of threads over HTTP: the application with every bound request and the access engine behind it, a thread engine that sends its commands to a scripted machine reached through a hub, and the rows of a provider, an agent profile and an environment that were made through the queries because their requests belong to another part.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{Who, make_org, make_project, make_topic, person};
use crate::support::World;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_api_contract::nodes::{Org, Project, Topic};
use orch_api_contract::threads::{ContentPart, ItemPayload};
use orch_api_core::backplane::Link;
use orch_api_core::registry::Registry;
use orch_api_core::state::AppState;
use orch_api_core::testing::TestApp;
use orch_api_core::thread_access::AuthzAccess;
use orch_build_info::BuildInfo;
use orch_db::Db;
use orch_db::queries::access::{NewEntry, PageKey, insert_entry};
use orch_db::queries::link as link_queries;
use orch_db::queries::resources::testing::insert_machine;
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing, Stage};
use orch_engine::config::EngineConfig;
use orch_engine::live::Discard;
use orch_engine::{Engine, EngineParts};
use orch_link::commands::{CommandHandle, Progress};
use orch_link::controller::{ControllerLink, Notice};
use orch_link_contract::commands::{
    AgentStartResult, Cmd, Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode,
    SessionMode, SessionOpenResult, TurnStart,
};
use orch_link_contract::events::{
    DeltaField, EmittedItemKind, Event, ItemCompleted, ItemDelta, ItemStarted, TurnCompleted,
    TurnOutcome, TurnStarted,
};
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use orch_testkit::doubles::link::{Hub, MachineRecord, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long a test waits for something to happen before it fails.
pub const PATIENCE: Duration = Duration::from_secs(25);

/// The model the tests' agent runs.
pub const MODEL: &str = "fake-1";

/// Waits until `check` holds, looking again every few milliseconds.
pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let started = Instant::now();
    loop {
        if check().await {
            return;
        }
        assert!(started.elapsed() < PATIENCE, "gave up waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// What the tests' agent declares of itself.
pub fn descriptor() -> Json {
    json!({
        "kind": "fake", "adapter_version": 1,
        "harness": { "name": "Fake", "version": "1.0.0", "supported_range": ">=1.0.0", "status": "supported" },
        "arrangements": [
            { "type": "together" },
            { "type": "separated", "executor": "orch-exec", "executor_version": "*", "tool_sets": ["plain"] }
        ],
        "native_provider": { "harness_kind": "fake", "title": "Fake subscription", "arrangements": ["separated"] },
        "forwarded_protocols": ["openai-responses"],
        "approval_modes": ["ask", "on_request", "bypass"],
        "sandbox_modes": ["read_only", "workspace", "open"],
        "side_thread_modes": ["read_only"],
        "capabilities": {
            "steer": true, "interrupt": true, "approvals": ["command", "file_change", "user_input"],
            "input_images": true, "usage_tokens": true, "memory": "none"
        },
        "parameters": []
    })
}

/// The waits of the engine in a test: short, so that nothing waits for a minute.
pub fn engine_config() -> EngineConfig {
    EngineConfig {
        interrupt_force: Duration::from_millis(1_500),
        start_timeout: Duration::from_millis(6_000),
        sweep_every: Duration::from_millis(40),
        recheck_every: Duration::from_millis(250),
        post_commit_grace: Duration::from_millis(5),
        sweep_batch: 200,
        // Growing text is held as a deployment holds it, for a tenth of its interval.
        stream_flush: Duration::from_millis(100),
        cap_check: Duration::ZERO,
        ..EngineConfig::default()
    }
}

// ---------------------------------------------------------------- the engine's commands

struct Issued {
    machine: Id,
    kind: String,
    body: Json,
    handle: CommandHandle,
    at: DateTime<Utc>,
}

/// The API's end of a machine's commands: each is kept from the moment it is issued, and a command asked
/// for twice under one identifier is made once.
pub struct ScriptedCommands {
    /// Optional exact native mapping used by operation tests; absent means unsupported.
    pub native: Mutex<Option<orch_api_contract::native::NativeCatalog>>,
    link: ControllerLink,
    issued: Mutex<HashMap<Id, Issued>>,
    clock: Arc<dyn Clock>,
}

fn stage_of(progress: &Progress) -> Stage {
    match progress {
        Progress::Queued => Stage::Queued,
        Progress::Sent => Stage::Sent,
        Progress::Accepted => Stage::Accepted,
        _ => Stage::Over,
    }
}

fn outcome_of(progress: &Progress) -> Option<Outcome> {
    match progress {
        Progress::Succeeded(body) => Some(Outcome::Succeeded(body.clone())),
        Progress::Failed(error) => Some(Outcome::Failed(error.clone())),
        Progress::Expired => Some(Outcome::Expired),
        Progress::Cancelled => Some(Outcome::Cancelled),
        _ => None,
    }
}

impl ScriptedCommands {
    /// How many commands of any kind the API issued to the machine.
    pub fn issued_total(&self) -> usize {
        lock(&self.issued).len()
    }

    /// How many commands of a kind were issued.
    pub fn issued_of(&self, kind: &str) -> usize {
        lock(&self.issued)
            .values()
            .filter(|entry| entry.kind == kind)
            .count()
    }
}

#[async_trait]
impl Commands for ScriptedCommands {
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        if command.kind != CommandKind::SessionNativeActions {
            return Err(CommandsError::Other(
                "this scripted observation is unsupported".to_owned(),
            ));
        }
        let mut catalog = lock(&self.native).clone().unwrap_or_else(|| {
            orch_api_contract::native::NativeCatalog::unavailable(
                orch_api_contract::native::NativeCatalogState::Unsupported,
                "The scripted session offers no native selectors",
            )
        });
        if catalog.state == orch_api_contract::native::NativeCatalogState::Ready {
            catalog.revision = format!(
                "{}:{}",
                command.body["segment_id"].as_str().unwrap_or_default(),
                catalog.revision
            );
        }
        serde_json::to_value(catalog)
            .map(Outcome::Succeeded)
            .map_err(|error| CommandsError::Body(error.to_string()))
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        Ok(lock(&self.issued).get(&id).map(|entry| entry.body.clone()))
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        Ok(lock(&self.issued)
            .get(&id)
            .map(|entry| orch_engine::commands::RetainedCommand {
                machine: entry.machine,
                body: entry.body.clone(),
            }))
    }
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        let mut issued = lock(&self.issued);
        if issued.contains_key(&command.id) {
            return Ok(());
        }
        let now = self.clock.now();
        let deadline = command.deadline.unwrap_or_else(|| {
            Time::new(now + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())))
        });
        let cmd = Cmd {
            id: command.id,
            kind: command.kind.as_str().to_owned(),
            key: command.key.wire(),
            deadline,
            body: command.body,
        };
        let kind = cmd.kind.clone();
        let body = cmd.body.clone();
        let handle = self.link.commands().issue(cmd);
        issued.insert(
            command.id,
            Issued {
                machine: command.machine,
                body,
                kind,
                handle,
                at: now,
            },
        );
        Ok(())
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        Ok(lock(&self.issued).get(&id).map(|entry| {
            let progress = entry.handle.progress();
            Info {
                stage: stage_of(&progress),
                issued_at: entry.at,
                outcome: outcome_of(&progress),
            }
        }))
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        let handle = lock(&self.issued)
            .get(&id)
            .map(|entry| entry.handle.clone());
        let Some(mut handle) = handle else {
            return Err(CommandsError::Other("there is no such command".to_owned()));
        };
        let progress = handle.wait().await;
        outcome_of(&progress).ok_or(CommandsError::Stopping)
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        let open = lock(&self.issued)
            .get(&id)
            .is_some_and(|entry| !entry.handle.progress().is_final());
        if open {
            self.link.commands().cancel(id);
        }
        Ok(open)
    }

    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        // Missing fixture commands were never issued; an issued command needs the native handler's answer.
        Ok(!lock(&self.issued).contains_key(&id) || self.link.commands().answered(id))
    }
}

// ---------------------------------------------------------------- what the scripted machine saw

/// The commands the scripted machine ran, in the order it ran them, and the failures it is to answer
/// the next commands of a kind with.
#[derive(Default)]
pub struct Seen {
    commands: Mutex<Vec<Command>>,
    refusals: Mutex<HashMap<CommandKind, VecDeque<CommandError>>>,
}

impl Seen {
    pub fn all(&self) -> Vec<Command> {
        lock(&self.commands).clone()
    }

    pub fn of_kind(&self, kind: CommandKind) -> Vec<Command> {
        self.all()
            .into_iter()
            .filter(|command| command.kind() == kind)
            .collect()
    }

    /// Answers the next command of `kind` the machine is given with `error`.
    pub fn refuse(&self, kind: CommandKind, error: CommandError) {
        lock(&self.refusals)
            .entry(kind)
            .or_default()
            .push_back(error);
    }
}

// ---------------------------------------------------------------- the events a harness says

/// The harness began the turn.
pub fn turn_began(turn: Id) -> Event {
    Event::TurnStarted(TurnStarted {
        turn_id: turn,
        native_turn_ref: Some(format!("native-{turn}")),
    })
}

/// The agent began a message.
pub fn message_began(turn: Id, native_ref: &str) -> Event {
    Event::ItemStarted(ItemStarted {
        turn_id: turn,
        native_ref: native_ref.to_owned(),
        stream: "main".to_owned(),
        parent_native_ref: None,
        kind: EmittedItemKind::AgentMessage,
        payload: orch_link_contract::foreign::ItemPayload::AgentMessage(
            orch_agent_contract::events::AgentMessagePayload {
                text: String::new(),
                text_len: 0,
                phase: Some(orch_agent_contract::events::AgentMessagePhase::Final),
            },
        ),
    })
}

/// The message grew by `data`, which came after `before` characters.
pub fn message_grew(native_ref: &str, before: i64, data: &str) -> Event {
    Event::ItemDelta(ItemDelta {
        native_ref: native_ref.to_owned(),
        field: DeltaField::Text,
        offset: before,
        len: i64::try_from(data.chars().count()).unwrap_or(i64::MAX),
        data: data.to_owned(),
    })
}

/// The message is final.
pub fn message_done(native_ref: &str, text: &str) -> Event {
    Event::ItemCompleted(ItemCompleted {
        native_ref: native_ref.to_owned(),
        status: orch_agent_contract::events::ItemStatus::Completed,
        payload: orch_link_contract::foreign::ItemPayload::AgentMessage(
            orch_agent_contract::events::AgentMessagePayload {
                text: text.to_owned(),
                text_len: i64::try_from(text.chars().count()).unwrap_or(i64::MAX),
                phase: Some(orch_agent_contract::events::AgentMessagePhase::Final),
            },
        ),
        output_blob: None,
    })
}

/// The turn ended as `outcome`.
pub fn turn_ended(turn: Id, outcome: TurnOutcome) -> Event {
    Event::TurnCompleted(TurnCompleted {
        turn_id: turn,
        status: outcome,
        error: None,
    })
}

/// The harness asks whether a command may run.
pub fn command_asked(turn: Id, request_ref: &str) -> Event {
    Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
        turn_id: turn,
        request_ref: request_ref.to_owned(),
        title: "Run a command".to_owned(),
        request: serde_json::from_value(json!({
            "request_kind": "command",
            "detail": { "command": "rm -rf build", "cwd": "/workspace/web", "reason": null },
            "options": [
                { "id": "allow_once", "label": "Allow once", "style": "allow", "scope": "once" },
                { "id": "deny", "label": "Deny", "style": "deny", "scope": null }
            ]
        }))
        .unwrap(),
    })
}

/// What was said in a thread, in the order it was made: `user: ...` and `agent: ...`.
pub fn said(items: &[orch_api_contract::threads::Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match &item.payload {
            ItemPayload::UserMessage(message) => Some(format!(
                "user: {}",
                message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
            ItemPayload::AgentMessage(message) => Some(format!("agent: {}", message.text)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- the rig

/// The application, its database, a machine behind a hub, and the thread engine between them.
pub struct Rig {
    pub world: World,
    pub hub: Hub,
    pub machine: Arc<ScriptedMachine>,
    pub machine_id: Id,
    pub commands: Arc<ScriptedCommands>,
    pub seen: Arc<Seen>,
    pub engine: Engine,
    pub db: Db,
}

impl Rig {
    /// A system with one machine that is online and has the roles of an agent, an environment and a provider.
    pub async fn new() -> Self {
        let mut world = World::new().await;
        let old = world.app.app.clone();
        let db = old.db.clone();
        let clock: Arc<dyn Clock> = Arc::clone(&old.clock);
        let mut conn = db.acquire().await.unwrap();
        let machine_id = insert_machine(&mut conn, "m1", "connected").await.unwrap();
        sqlx::query(
            "UPDATE machines SET roles = ARRAY['agent','environment','provider'], online = true, facts = $2 WHERE id = $1",
        )
        .bind(machine_id)
        .bind(json!({
            "adapters": [{"kind":"fake","adapter_version":1,"builtin":true,"default_home_owned":true}],
            "harnesses": [{"kind":"fake","path":"/test/harness","version":"1.0.0","daemon":false}],
            "policy": {"agent.harnesses":[{"kind":"fake","path":"/test/harness"}],
                "provider.arrangements":["separated","key_separated","credential_shared","handed_over"]}
        }))
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        let pki = TestPki::new(Arc::clone(&clock));
        let identity = pki.machine(machine_id);
        let hub = Hub::new(Arc::clone(&clock), pki.certificate());
        let roles = vec![Role::Agent, Role::Environment, Role::Provider];
        let (link, notices) = hub.add_machine(
            machine_id,
            roles.clone(),
            Default::default(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        let machine = Arc::new(ScriptedMachine::start(
            &hub,
            &clock,
            &identity,
            &MachineSpec {
                roles,
                ..MachineSpec::default()
            },
        ));
        let commands = Arc::new(ScriptedCommands {
            native: Mutex::new(None),
            link: link.clone(),
            issued: Mutex::new(HashMap::new()),
            clock: Arc::clone(&clock),
        });
        let engine = Engine::new(EngineParts {
            db: db.clone(),
            clock: Arc::clone(&clock),
            config: engine_config(),
            commands: commands.clone(),
            access: Arc::new(AuthzAccess::new(Arc::clone(&clock))),
            live: Arc::new(Discard),
        });
        // What keeps the files of a copy until the thread's next turn, as the API's own engine has.
        engine.attach_parcels(Arc::new(orch_engine::parcels::WorkspaceParcels::new(
            &engine,
        )));
        // The content store, which the engine is given before it starts, as the API's own is: a fork's
        // or a switch's history is kept there, and the API reads from the same one.
        let blobs = orch_api_core::blobs::Blobs::in_memory();
        engine.attach_blobs(blobs.store());
        engine.start();
        let link_for_state =
            Link::detached(db.clone(), Arc::clone(&clock), Arc::clone(&old.config))
                .expect("a link without gateways can be made");
        let state = Arc::new(AppState {
            builtin_sources: old.builtin_sources.clone(),
            host_ports: old.host_ports.clone(),
            db: db.clone(),
            config: Arc::clone(&old.config),
            clock: Arc::clone(&clock),
            link: link_for_state,
            engine: engine.clone(),
            realtime: orch_api_core::realtime::Realtime::new(&db, &clock, &old.config),
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs,
            cache: orch_api_core::cache::Cache::detached(),
        });
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        world.app = TestApp::over(state, world.app.clock.clone(), registry);
        let rig = Self {
            world,
            hub,
            machine,
            machine_id,
            commands,
            seen: Arc::new(Seen::default()),
            engine,
            db,
        };
        rig.answer_commands();
        rig.spawn_applier(link, notices);
        let (hub, machine) = (rig.hub.clone(), rig.machine_id);
        eventually("the machine connects", || {
            let up = hub.session_of(machine).is_some();
            async move { up }
        })
        .await;
        rig
    }

    /// What the API does with a machine's events: applies them in a transaction that moves the stream's
    /// cursor, and tells the link they are committed (or that they are not).
    fn spawn_applier(&self, link: ControllerLink, mut notices: mpsc::UnboundedReceiver<Notice>) {
        let (pool, machine) = (self.db.clone(), self.machine_id);
        let engine = self.engine.clone();
        let clock = Arc::clone(&self.world.app.app.clock);
        tokio::spawn(async move {
            while let Some(notice) = notices.recv().await {
                match notice {
                    Notice::Deliver(delivery) => {
                        let applied = async {
                            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
                            engine
                                .apply_events(&mut tx, machine, &delivery)
                                .await
                                .map_err(|e| e.to_string())?;
                            let first = delivery.events.first().map_or(1, |event| event.seq);
                            let moved = link_queries::advance_cursor(
                                &mut tx,
                                machine,
                                &delivery.stream.to_string(),
                                first - 1,
                                delivery.last_seq,
                                clock.now(),
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                            if !moved {
                                return Ok::<bool, String>(false);
                            }
                            tx.commit().await.map_err(|e| e.to_string())?;
                            Ok(true)
                        }
                        .await;
                        match applied {
                            Ok(true) => link.committed(&delivery),
                            Ok(false) => link.abandoned(&delivery),
                            Err(error) => {
                                eprintln!("events could not be applied: {error}");
                                link.abandoned(&delivery);
                            }
                        }
                    }
                    Notice::Online { .. } => {
                        set_online(&pool, machine, true).await;
                        let _ = engine.machine_online(machine).await;
                    }
                    Notice::Offline { .. } => {
                        set_online(&pool, machine, false).await;
                        let _ = engine.machine_offline(machine).await;
                    }
                    _ => {}
                }
            }
        });
    }

    /// The scripted machine answers the commands a harness answers, and keeps what it was asked.
    fn answer_commands(&self) {
        let handler = &self.machine.handler;
        for kind in [
            CommandKind::AgentStart,
            CommandKind::SessionOpen,
            CommandKind::SessionOpenFenced,
            CommandKind::TurnStart,
            CommandKind::TurnStartFenced,
            CommandKind::TurnSteer,
            CommandKind::TurnInterrupt,
            CommandKind::ApprovalResolve,
            CommandKind::SessionClose,
            CommandKind::SessionCloseFenced,
            CommandKind::LeaseOpen,
            CommandKind::LeaseClose,
            CommandKind::OrchToolResult,
        ] {
            let seen = Arc::clone(&self.seen);
            handler.on(kind, move |command| {
                lock(&seen.commands).push(command.clone());
                if let Some(error) = lock(&seen.refusals)
                    .get_mut(&kind)
                    .and_then(VecDeque::pop_front)
                {
                    return Err(error);
                }
                Ok(match command {
                    Command::AgentStart(_) => CommandResult::AgentStart(AgentStartResult {
                        reused: false,
                        harness_version: "1.0.0".to_owned(),
                        descriptor: serde_json::from_value(descriptor()).unwrap(),
                    }),
                    Command::SessionOpen(open) => CommandResult::SessionOpen(SessionOpenResult {
                        native_session_ref: format!("native-{}", open.segment_id),
                        resumed: open.mode == SessionMode::Resume,
                        import_report: None,
                        instruction_sources: Vec::new(),
                        approval_reviewer: None,
                    }),
                    Command::SessionOpenFenced(open) => {
                        CommandResult::SessionOpenFenced(SessionOpenResult {
                            native_session_ref: format!("native-{}", open.session.segment_id),
                            resumed: open.session.mode == SessionMode::Resume,
                            import_report: None,
                            instruction_sources: Vec::new(),
                            approval_reviewer: None,
                        })
                    }
                    Command::TurnStart(_) => CommandResult::TurnStart(Empty {}),
                    Command::TurnStartFenced(_) => CommandResult::TurnStartFenced(Empty {}),
                    Command::TurnSteer(_) => CommandResult::TurnSteer(Empty {}),
                    Command::TurnInterrupt(_) => CommandResult::TurnInterrupt(Empty {}),
                    Command::ApprovalResolve(_) => CommandResult::ApprovalResolve(Empty {}),
                    Command::SessionClose(_) => CommandResult::SessionClose(Empty {}),
                    Command::SessionCloseFenced(_) => CommandResult::SessionCloseFenced(Empty {}),
                    Command::LeaseOpen(_) => CommandResult::LeaseOpen(Empty {}),
                    Command::LeaseClose(_) => CommandResult::LeaseClose(
                        serde_json::from_value(json!({ "requests": 0, "usage": null }))
                            .map_err(|e| CommandError::new(ErrorCode::Internal, e.to_string()))?,
                    ),
                    Command::OrchToolResult(_) => CommandResult::OrchToolResult(Empty {}),
                    other => {
                        return Err(CommandError::new(
                            ErrorCode::Unsupported,
                            format!("no script for {}", other.kind()),
                        ));
                    }
                })
            });
        }
    }

    // ------------------------------------------------------------ the machine, as the tests drive it

    /// The `turn.start` commands the machine ran for a thread, in order.
    pub fn turn_starts(&self, thread: Id) -> Vec<TurnStart> {
        self.seen
            .all()
            .into_iter()
            .filter_map(|command| match command {
                Command::TurnStart(start) if start.thread_id == thread => Some(start),
                Command::TurnStartFenced(start) if start.thread_id == thread => Some(*start.turn),
                _ => None,
            })
            .collect()
    }

    /// Waits for the machine to have run a thread's `nth` turn start (from 0), and returns it.
    pub async fn turn_start(&self, thread: Id, nth: usize) -> TurnStart {
        let mut found = None;
        eventually(&format!("turn.start number {nth}"), || {
            found = self.turn_starts(thread).get(nth).cloned();
            let done = found.is_some();
            async move { done }
        })
        .await;
        found.expect("a turn start was seen")
    }

    /// How the agent profiles take Orch's tools (`mcp`, or `none`): as stored, and as their harness says
    /// again whenever it starts, since what it reports replaces what was stored.
    pub async fn tools_of_agents(&self, tools: Json) {
        sqlx::query(
            "UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,orch_tools}', $1)",
        )
        .bind(&tools)
        .execute(self.db.pool())
        .await
        .unwrap();
        let mut declares = descriptor();
        declares["capabilities"]["orch_tools"] = tools;
        let seen = Arc::clone(&self.seen);
        self.machine
            .handler
            .on(CommandKind::AgentStart, move |command| {
                lock(&seen.commands).push(command.clone());
                if let Some(error) = lock(&seen.refusals)
                    .get_mut(&CommandKind::AgentStart)
                    .and_then(VecDeque::pop_front)
                {
                    return Err(error);
                }
                Ok(CommandResult::AgentStart(AgentStartResult {
                    reused: false,
                    harness_version: "1.0.0".to_owned(),
                    descriptor: serde_json::from_value(declares.clone()).unwrap(),
                }))
            });
    }

    /// The segment a thread is in now.
    pub async fn segment_of(&self, thread: Id) -> Id {
        sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }

    /// The harness says something about a thread's session.
    pub async fn say(&self, thread: Id, event: Event) {
        let segment = self.segment_of(thread).await;
        if let Some(opening) =
            orch_testkit::doubles::sessions::prepare(self.db.pool(), thread, segment)
                .await
                .unwrap()
        {
            let command = Outgoing::new(
                new_id(),
                opening.machine,
                orch_link_contract::commands::SerialKey::Thread(thread),
                &opening.body,
            )
            .unwrap();
            let result = orch_engine::commands::run(self.commands.as_ref(), command)
                .await
                .unwrap();
            let Outcome::Succeeded(result) = result else {
                panic!("the fixture session did not open: {result:?}")
            };
            orch_testkit::doubles::sessions::opened(self.db.pool(), segment, &result)
                .await
                .unwrap();
        }
        self.machine
            .publish(
                &Stream::Session {
                    thread_id: thread,
                    segment_id: segment,
                },
                &event,
            )
            .await;
    }

    /// The harness takes the turn up and answers with `reply`.
    pub async fn answer(&self, thread: Id, turn: &TurnStart, native_ref: &str, reply: &str) {
        self.say(thread, turn_began(turn.turn_id)).await;
        self.say(thread, message_began(turn.turn_id, native_ref))
            .await;
        self.say(thread, message_grew(native_ref, 0, reply)).await;
        self.say(thread, message_done(native_ref, reply)).await;
    }

    /// The state of a turn.
    pub async fn turn_state(&self, turn: Id) -> String {
        sqlx::query_scalar("SELECT state FROM turns WHERE id = $1")
            .bind(turn)
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }

    /// Waits for a turn to be in a state.
    pub async fn wait_turn(&self, turn: Id, state: &str) {
        eventually(&format!("turn in state {state}"), || async move {
            self.turn_state(turn).await == state
        })
        .await;
    }

    /// The state of a thread: (state, blocked reason).
    pub async fn state(&self, thread: Id) -> (String, Option<String>) {
        sqlx::query_as("SELECT state, blocked_reason FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }

    /// Waits for a thread to be in a state.
    pub async fn wait_state(&self, thread: Id, state: &str) {
        eventually(&format!("thread in state {state}"), || async move {
            self.state(thread).await.0 == state
        })
        .await;
    }

    /// A whole turn: waits for the machine to be asked for the thread's `nth` turn, answers, and ends it.
    pub async fn run_turn(&self, thread: Id, nth: usize, reply: &str) -> TurnStart {
        let start = self.turn_start(thread, nth).await;
        self.answer(thread, &start, &format!("m{nth}"), reply).await;
        self.say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        self.wait_turn(start.turn_id, "completed").await;
        start
    }

    // ------------------------------------------------------------ the people and the tree

    /// A person with a bearer token.
    pub async fn person(&self, username: &str) -> Who {
        person(&self.world, username).await
    }

    /// An org of `owner`'s with a project and a topic in it.
    pub async fn tree(&self, owner: &Who) -> Tree {
        let org = make_org(&self.world, owner, "Acme", "acme").await;
        let project = make_project(&self.world, owner, org.base.id, "Web").await;
        let topic = make_topic(&self.world, owner, project.base.id, "Login").await;
        Tree {
            org,
            project,
            topic,
        }
    }

    /// A provider that runs the tests' agent, the agent profile that holds its login, a model, and an
    /// environment on the machine, all `owner`'s and homed in the org, each with `owner` in its Owners row.
    pub async fn resources(&self, owner: &Who, tree: &Tree) -> Resources {
        let root = tree.org.base.id;
        let (agent, provider, environment, template) = (new_id(), new_id(), new_id(), new_id());
        let mut conn = self.db.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
             VALUES ($1, 'Fake agent', $2, $3, $3, 'fake', '{}', $4)",
        )
        .bind(agent)
        .bind(owner.id())
        .bind(root)
        .bind(descriptor())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                    machine_id, arrangements, status)
             VALUES ($1, 'Fake subscription', $2, $3, $3, 'subscription', 'fake', $4, $5,
                     ARRAY['separated'], 'ready')",
        )
        .bind(provider)
        .bind(owner.id())
        .bind(root)
        .bind(agent)
        .bind(self.machine_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, 'Fake One', $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(provider)
        .bind(MODEL)
        .bind(json!({ "efforts": ["low", "high"], "default_effort": "low", "input_images": true }))
        .bind(owner.id())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'template', 'test', '{}')",
        )
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                       machine_id, template_id, generation, status, facts)
             VALUES ($1, 'Workspace', $2, $3, 'managed', $4, '/workspace', $5, $6, 1, 'running', $7)",
        )
        .bind(environment)
        .bind(owner.id())
        .bind(root)
        .bind(tree.topic.base.id)
        .bind(self.machine_id)
        .bind(template)
        .bind(json!({"executors":[{"kind":"orch-exec","available":true,"sandbox_modes":["read_only","workspace","open"],"tool_sets":["plain"]}]}))
        .execute(&mut *conn)
        .await
        .unwrap();
        for (kind, id) in [
            ("agent", agent),
            ("provider", provider),
            ("environment", environment),
        ] {
            insert_entry(
                &mut conn,
                &NewEntry {
                    id: new_id(),
                    page: &PageKey::new(kind, id, "live"),
                    profile: "any",
                    permission: "access.own",
                    holder_kind: "user",
                    holder_id: Some(owner.id()),
                    holder_role: None,
                    options: &json!({}),
                    expires_at: None,
                    note: "",
                    source: "creation",
                    created_by: Some(owner.id()),
                    at: self.world.now(),
                },
            )
            .await
            .unwrap();
        }
        Resources {
            agent,
            provider,
            environment,
        }
    }

    /// Names a person in the Use row of a resource, for some models or all.
    pub async fn lets_use(&self, kind: &str, id: Id, permission: &str, who: &Who, options: Json) {
        let mut conn = self.db.acquire().await.unwrap();
        insert_entry(
            &mut conn,
            &NewEntry {
                id: new_id(),
                page: &PageKey::new(kind, id, "live"),
                profile: "any",
                permission,
                holder_kind: "user",
                holder_id: Some(who.id()),
                holder_role: None,
                options: &options,
                expires_at: None,
                note: "",
                source: "page",
                created_by: None,
                at: self.world.now(),
            },
        )
        .await
        .unwrap();
    }
}

/// The nodes of a test: an org, a project in it, and a topic in that.
pub struct Tree {
    pub org: Org,
    pub project: Project,
    pub topic: Topic,
}

/// The resources a thread runs on.
#[derive(Debug, Clone, Copy)]
pub struct Resources {
    pub agent: Id,
    pub provider: Id,
    pub environment: Id,
}

async fn set_online(pool: &Db, machine: Id, online: bool) {
    sqlx::query("UPDATE machines SET online = $2 WHERE id = $1")
        .bind(machine)
        .bind(online)
        .execute(pool.pool())
        .await
        .unwrap();
}

// ---------------------------------------------------------------- a scene with people

/// The usual scene of a test: Ada owns an org with a topic and everything a thread runs on; Bea is a
/// stranger; Cai may read; Dan drives threads of the org but has no use of its provider; and Eve works
/// in the org as a developer and uses the provider.
pub struct Scene {
    pub rig: Rig,
    pub ada: Who,
    pub bea: Who,
    pub cai: Who,
    pub dan: Who,
    pub eve: Who,
    pub tree: Tree,
    pub on: Resources,
}

impl Scene {
    pub async fn new() -> Self {
        let rig = Rig::new().await;
        let ada = rig.person("ada").await;
        let bea = rig.person("bea").await;
        let cai = rig.person("cai").await;
        let dan = rig.person("dan").await;
        let eve = rig.person("eve").await;
        let tree = rig.tree(&ada).await;
        let on = rig.resources(&ada, &tree).await;
        let org = tree.org.base.id;
        crate::places::join(&rig.world, org, org, &cai, "viewer", &ada).await;
        crate::places::join(&rig.world, org, org, &dan, "developer", &ada).await;
        crate::places::join(&rig.world, org, org, &eve, "developer", &ada).await;
        rig.lets_use("provider", on.provider, "provider.use", &eve, json!({}))
            .await;
        rig.lets_use("agent", on.agent, "agent.use", &eve, json!({}))
            .await;
        rig.lets_use(
            "environment",
            on.environment,
            "environment.manage",
            &eve,
            json!({}),
        )
        .await;
        Self {
            rig,
            ada,
            bea,
            cai,
            dan,
            eve,
            tree,
            on,
        }
    }

    /// A thread made by `by` in the topic, on what the scene has.
    pub async fn thread_by(&self, by: &Who, name: &str) -> orch_api_contract::threads::Thread {
        use orch_api_contract::binding::EnvironmentChoice;
        use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
        self.rig
            .world
            .app
            .call(ThreadCreate, &[&self.tree.topic.base.id])
            .token(&by.token)
            .body(&CreateThread {
                name: Some(name.to_owned()),
                binding: Some(CreateThreadBinding {
                    agent_id: self.on.agent,
                    provider_id: self.on.provider,
                    arrangement: None,
                }),
                model: Some(MODEL.to_owned()),
                environment: Some(EnvironmentChoice::Use(self.on.environment)),
                directory: Some("web".to_owned()),
                ..CreateThread::default()
            })
            .send()
            .await
            .ok()
    }

    /// A thread made by Ada.
    pub async fn thread(&self) -> orch_api_contract::threads::Thread {
        self.thread_by(&self.ada, "Fix login").await
    }

    /// `by` says `words` to the thread.
    pub async fn say(
        &self,
        by: &Who,
        thread: Id,
        words: &str,
        delivery: orch_api_contract::threads::Delivery,
        client_id: &str,
    ) -> orch_api_core::testing::Reply<orch_api_contract::threads::ThreadInputCreateReply> {
        use orch_api_contract::threads::{ThreadInputCreate, ThreadInputCreateBody};
        self.rig
            .world
            .app
            .call(ThreadInputCreate, &[&thread])
            .token(&by.token)
            .body(&ThreadInputCreateBody {
                plan: false,
                content: vec![ContentPart::Text {
                    text: words.to_owned(),
                }],
                delivery,
                client_id: client_id.to_owned(),
                bypass: None,
                native_action: None,
            })
            .send()
            .await
    }

    /// Ada queues `words`, the machine takes the turn up, answers with `reply`, and the turn ends.
    pub async fn exchange(&self, thread: Id, nth: usize, words: &str, reply: &str) {
        let sent = self
            .say(
                &self.ada,
                thread,
                words,
                orch_api_contract::threads::Delivery::Queue,
                &format!("exchange-{nth}"),
            )
            .await;
        assert!(sent.status.is_success(), "{:?}", sent.body);
        self.rig.run_turn(thread, nth, reply).await;
        self.rig.wait_state(thread, "idle").await;
    }
}
