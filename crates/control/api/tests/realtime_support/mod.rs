//! A whole small system for the tests of the realtime server: a real database, a tree of nodes, one agent behind a scripted machine, the thread engine, the API on a loopback socket, and a client that speaks the protocol as a browser does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use async_trait::async_trait;
use chrono::{DateTime, Duration as Span, Utc};
use futures::{FutureExt, SinkExt, StreamExt};
use orch_access_contract::pages as contract_pages;
use orch_agent_contract::events::{AgentMessagePayload, AgentMessagePhase, ItemStatus};
use orch_api_contract::threads::{ContentPart, Delivery};
use orch_api_core::backplane::Link;
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::realtime::{Realtime, Tuning};
use orch_api_core::registry::Registry;
use orch_api_core::state::{App, AppState};
use orch_authz::Caller;
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_db::queries::access::{PageKey, SYSTEM_ID, testing as access_testing};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::{insert_session, insert_user};
use orch_db::queries::link as link_queries;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::{Conn, Db};
use orch_engine::access::{AccessFault, Answer, Limit, ProviderUse, ResourceAccess};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing, Stage};
use orch_engine::config::EngineConfig;
use orch_engine::inputs::InputRequest;
use orch_engine::{Engine, EngineParts};
use orch_link::commands::{CommandHandle, Progress};
use orch_link::controller::{ControllerLink, Notice};
use orch_link_contract::commands::{
    AgentStartResult, Cmd, Command, CommandKind, CommandResult, Empty, ErrorCode, SessionMode,
    SessionOpenResult, TurnStart,
};
use orch_link_contract::events::{
    DeltaField, EmittedItemKind, Event, ItemCompleted, ItemDelta, ItemStarted, TurnCompleted,
    TurnOutcome, TurnStarted,
};
use orch_link_contract::foreign::ItemPayload;
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use orch_realtime_contract::delta::{Applied, DeltaAction, RowAction};
use orch_realtime_contract::sync::{FeedCursor, FrameAction};
use orch_testkit::database::TestDb;
use orch_testkit::doubles::link::{Hub, MachineRecord, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Id, Json, ManualClock, SystemClock, Time, new_id};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long a test waits for something to happen before it fails.
pub const PATIENCE: Duration = Duration::from_secs(25);

/// Waits for the real shared-watch coordinator to own its database lease before injecting transient changes.
pub async fn coordinator_ready(rig: &Rig) {
    eventually(
        "the transient-watch coordinator to hold its lease",
        || async {
            let mut conn = rig.pool.acquire().await.unwrap();
            orch_db::queries::job_leases::holder(
                &mut conn,
                orch_api_core::realtime::ephemeral::COORDINATOR,
                rig.clock.now(),
            )
            .await
            .unwrap()
            .is_some()
        },
    )
    .await;
}

/// How many sessions of the test's database, other than the asking one, have had a transaction open
/// for longer than `seconds`: none, where no code path keeps one open across its other waits.
pub async fn transactions_open_longer_than(rig: &Rig, seconds: f64) -> i64 {
    let mut conn = rig.pool.acquire().await.unwrap();
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()
          AND pid <> pg_backend_pid() AND xact_start < now() - make_interval(secs => $1)",
    )
    .bind(seconds)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}
/// The model the tests' agent runs.
pub const MODEL: &str = "fake-1";
/// The room, in bytes, that a cramped socket asks of the kernel: it gives a few
/// kilobytes more than is asked, and no less than its own least.
pub const CRAMPED_BUFFER: u32 = 4_096;

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

// ---------------------------------------------------------------- the access double

/// Answers the engine's questions about resources: everybody may.
pub struct OpenAccess;

#[async_trait]
impl ResourceAccess for OpenAccess {
    async fn provider_use(&self, _: &mut Conn, _: &ProviderUse<'_>) -> Result<Answer, AccessFault> {
        Ok(Answer::Allowed { arrangements: None })
    }

    async fn agent_use(&self, _: &mut Conn, _: Id, _: Id, _: Id) -> Result<Answer, AccessFault> {
        Ok(Answer::Allowed { arrangements: None })
    }

    async fn environment_place(&self, _: &mut Conn, _: Id, _: Id) -> Result<Answer, AccessFault> {
        Ok(Answer::Allowed { arrangements: None })
    }

    async fn environment_manage(&self, _: &mut Conn, _: Id, _: Id) -> Result<Answer, AccessFault> {
        Ok(Answer::Allowed { arrangements: None })
    }

    async fn may_pass(&self, _: &mut Conn, _: Id, _: bool, _: &Limit) -> Result<bool, AccessFault> {
        Ok(true)
    }
}

// ---------------------------------------------------------------- the commands double

struct Issued {
    machine: Id,
    cmd: Cmd,
    handle: CommandHandle,
    at: DateTime<Utc>,
}

/// The API's end of a machine's commands: each is kept from the moment it is
/// issued, and one asked for twice under one identifier is made once.
pub struct ScriptedCommands {
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

#[async_trait]
impl Commands for ScriptedCommands {
    async fn retained_body(&self, id: Id) -> Result<Option<orch_types::Json>, CommandsError> {
        Ok(lock(&self.issued)
            .get(&id)
            .map(|entry| entry.cmd.body.clone()))
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        Ok(lock(&self.issued)
            .get(&id)
            .map(|entry| orch_engine::commands::RetainedCommand {
                machine: entry.machine,
                body: entry.cmd.body.clone(),
            }))
    }
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        let mut issued = lock(&self.issued);
        if issued.contains_key(&command.id) {
            return Ok(());
        }
        let now = self.clock.now();
        let deadline = command.deadline.unwrap_or_else(|| {
            Time::new(now + Span::seconds(i64::from(command.kind.deadline_secs())))
        });
        let cmd = Cmd {
            id: command.id,
            kind: command.kind.as_str().to_owned(),
            key: command.key.wire(),
            deadline,
            body: command.body,
        };
        let handle = self.link.commands().issue(cmd.clone());
        issued.insert(
            command.id,
            Issued {
                machine: command.machine,
                cmd,
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
        let Some(mut handle) = lock(&self.issued)
            .get(&id)
            .map(|entry| entry.handle.clone())
        else {
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
}

// ---------------------------------------------------------------- the harness' answers

/// A descriptor of the tests' agent.
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
        "capabilities": {
            "steer": true, "interrupt": true, "approvals": ["command", "file_change", "user_input"],
            "input_images": true, "usage_tokens": true, "memory": "none"
        },
        "parameters": []
    })
}

/// The engine's waits, short, so that a test does not wait for a minute.
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

// ---------------------------------------------------------------- events the harness says

/// An input that waits its turn.
pub fn queued(text: &str) -> InputRequest {
    InputRequest {
        plan: false,
        content: vec![ContentPart::Text {
            text: text.to_owned(),
        }],
        delivery: Delivery::Queue,
        client_id: new_id().to_string(),
        bypass: None,
    }
}

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
        payload: ItemPayload::AgentMessage(AgentMessagePayload {
            text: String::new(),
            text_len: 0,
            phase: Some(AgentMessagePhase::Final),
        }),
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
        status: ItemStatus::Completed,
        payload: ItemPayload::AgentMessage(AgentMessagePayload {
            text: text.to_owned(),
            text_len: i64::try_from(text.chars().count()).unwrap_or(i64::MAX),
            phase: Some(AgentMessagePhase::Final),
        }),
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

// ---------------------------------------------------------------- one API process

/// One API process: a realtime server, the state it serves from, and the
/// loopback address it listens on.
pub struct Server {
    pub realtime: Realtime,
    pub app: App,
    pub addr: SocketAddr,
    pub engine: Engine,
    shutdown: CancellationToken,
    listener_shutdown: CancellationToken,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    task_owners: Arc<AtomicUsize>,
    stopping: tokio::sync::Mutex<()>,
}

struct TaskOwner(Arc<AtomicUsize>);
impl Drop for TaskOwner {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Assertion-unwind fallback only. Successful proofs await stop/shutdown.
        self.engine.stop();
        self.app.link.shutdown();
        self.shutdown.cancel();
        self.listener_shutdown.cancel();
        if tokio::runtime::Handle::try_current().is_ok() {
            // The first poll sends the existing cancellation/connection signals;
            // Drop cannot wait for the asynchronous join on its own runtime.
            let _ = self.realtime.stop().now_or_never();
        }
        for task in lock(&self.tasks).iter() {
            task.abort();
        }
    }
}

impl Server {
    /// Serves `app` on a port the system assigns. With a send buffer, what the
    /// server writes to a client that does not read fills the line after a few
    /// kilobytes, as it does on a slow link.
    async fn serve(
        app: App,
        realtime: Realtime,
        engine: Engine,
        send_buffer: Option<u32>,
        database: Option<Arc<TestDb>>,
    ) -> Self {
        // Standalone websocket fixtures bypass TestApp; compose the same installed test key
        // before serving metadata, preserving an explicitly installed caller key.
        orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let router = orch_api_core::server::router(app.clone(), registry);
        let socket = TcpSocket::new_v4().unwrap();
        if let Some(size) = send_buffer {
            socket.set_send_buffer_size(size).unwrap();
        }
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(128).unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let listener_shutdown = shutdown.child_token();
        let stopping = listener_shutdown.clone();
        let server = Self {
            realtime,
            app,
            addr,
            engine,
            shutdown,
            listener_shutdown,
            tasks: Mutex::new(Vec::new()),
            task_owners: Arc::new(AtomicUsize::new(0)),
            stopping: tokio::sync::Mutex::new(()),
        };
        server.own(async move {
            let _database = database;
            orch_api_core::server::serve_plain(listener, router, async move {
                stopping.cancelled().await;
            })
            .await
            .expect("the owned fixture HTTP server ends cleanly");
        });
        server
    }

    fn own(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        self.task_owners.fetch_add(1, Ordering::SeqCst);
        let owner = TaskOwner(Arc::clone(&self.task_owners));
        let task = tokio::spawn(async move {
            let _owner = owner;
            future.await;
        });
        lock(&self.tasks).push(task);
    }

    pub fn task_owners(&self) -> usize {
        self.task_owners.load(Ordering::SeqCst)
    }

    /// A process of the API over `db` with an engine of its own that sends
    /// nothing to any machine: for the tests that need rows and no turn.
    pub async fn over(db: &Db, clock: &Arc<dyn Clock>, tuning: Tuning) -> Self {
        Self::over_config(db, clock, tuning, ServerConfig::default()).await
    }

    /// The same process with an explicit proxy trust policy, for real handshake-origin proofs.
    pub async fn over_config(
        db: &Db,
        clock: &Arc<dyn Clock>,
        tuning: Tuning,
        config: ServerConfig,
    ) -> Self {
        let config = Arc::new(config);
        let link = Link::detached(db.clone(), Arc::clone(clock), Arc::clone(&config)).unwrap();
        let realtime = Realtime::tuned(db, clock, tuning);
        let engine =
            orch_api_core::thread_engine::detached_with(db, clock, &config, &link, realtime.sink());
        realtime.start(engine.clone());
        realtime.listening().await;
        let app: App = Arc::new(AppState {
            builtin_sources: Arc::default(),
            host_ports: Default::default(),
            db: db.clone(),
            config,
            clock: Arc::clone(clock),
            link,
            engine: engine.clone(),
            realtime: realtime.clone(),
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs: orch_api_core::blobs::Blobs::in_memory(),
            cache: orch_api_core::cache::Cache::detached(),
        });
        Self::serve(app, realtime, engine, None, None).await
    }

    /// The process stops: its sockets are closed and its listener is gone.
    pub async fn stop(&self) {
        let _stopping = self.stopping.lock().await;
        let end = tokio::time::Instant::now() + PATIENCE;
        self.engine.stop();
        self.app.link.shutdown();
        self.realtime.stop().await;
        self.shutdown.cancel();
        let tasks = std::mem::take(&mut *lock(&self.tasks));
        let mut failures = Vec::new();
        for mut task in tasks {
            match tokio::time::timeout_at(end, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(if error.is_panic() {
                    "owned fixture task panicked"
                } else {
                    "owned fixture task was unexpectedly cancelled"
                }),
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    failures.push("owned fixture task exceeded the original fixture patience");
                }
            }
        }
        assert_eq!(
            self.task_owners(),
            0,
            "every owned server/applier task has retired before fixture cleanup"
        );
        assert!(
            failures.is_empty(),
            "fixture owners did not shut down cleanly: {failures:?}"
        );
    }

    /// Only the browsers' side of the process goes away: its sockets are closed and its listener is
    /// gone, while the engine and the machines' link keep committing what machines say, as the
    /// process a machine is connected to would.
    pub async fn close_sockets(&self) {
        self.realtime.stop().await;
        self.listener_shutdown.cancel();
    }

    /// A client connected to this process.
    pub async fn client(&self) -> Client {
        Client::connect(self.addr).await
    }
}

/// Tuning with short waits, so that a test does not wait for a minute: the
/// sweep is the only thing left at the protocol's own pace when a test says so.
pub fn quick(config: &ServerConfig) -> Tuning {
    let mut tuning = Tuning::from_config(config);
    tuning.sweep_every = Duration::from_millis(400);
    tuning.epoch_every = Duration::from_millis(100);
    tuning.fanout_gap = Duration::from_millis(20);
    tuning.tick = Duration::from_millis(100);
    tuning.close_grace = Duration::from_secs(3);
    tuning
}

// ---------------------------------------------------------------- the world

/// Everything a test stands on.
pub struct Rig {
    pub pool: Db,
    pub clock: Arc<dyn Clock>,
    pub server: Server,
    pub machines: Hub,
    pub machine: Arc<ScriptedMachine>,
    pub machine_id: Id,
    pub ada: Id,
    pub root: Id,
    pub project: Id,
    pub topic: Id,
    pub agent: Id,
    pub provider: Id,
    pub environment: Id,
    pub token: String,
    commands: Arc<ScriptedCommands>,
    seen: Arc<Mutex<Vec<Command>>>,
    sequence: AtomicU64,
    publishing: tokio::sync::Mutex<()>,
    /// The clock of the API process (engine, application and realtime server) when a test holds its time
    /// still ([`Rig::with_time_held`], [`Rig::cramped_with_time_held`]).
    held_time: Option<ManualClock>,
    /// The final database owner; callbacks retain a lease until joined or fully aborted.
    pub db: Arc<TestDb>,
}

impl Rig {
    /// A world with one topic, one agent on one provider in one environment, all on one machine that is online.
    pub async fn new() -> Self {
        Self::build(None, false, |_| {}).await
    }

    /// Metadata/watch proofs own their job transitions and must not race an already running supervisor.
    /// Native history/accounting constructors retain their original started engine and report behavior.
    pub async fn with_unstarted_engine() -> Self {
        Self::build_mode(None, false, false, |_| {}).await
    }

    /// The already connected scripted machine's controller, for a target-local stored-command bridge.
    pub fn machine_controller(&self) -> ControllerLink {
        self.commands.link.clone()
    }

    /// As [`Rig::new`], with the realtime server's times and limits changed.
    pub async fn tuned(change: impl FnOnce(&mut Tuning)) -> Self {
        Self::build(None, false, change).await
    }

    /// As [`Rig::tuned`], and the server's socket has little room in the kernel
    /// for what it writes: a reader that does not read stops it after a few
    /// kilobytes.
    pub async fn cramped(change: impl FnOnce(&mut Tuning)) -> Self {
        Self::build(Some(CRAMPED_BUFFER), false, change).await
    }

    /// As [`Rig::new`], and the clock of the API process stands still until
    /// [`Rig::let_time_pass`] moves it: what the server times (how long a token is
    /// good for) is over when the test says so, not when a busy machine has let
    /// that much time go by. The lifetime of a token made by [`Rig::token_for`]
    /// counts on that clock.
    pub async fn with_time_held() -> Self {
        Self::build(None, true, |_| {}).await
    }

    /// A permission-only native-row proof needs the real initial source reads
    /// published before its samples. Start the normal supervisor after that
    /// exact setup chain; the held clock leaves no subsequent read due.
    pub async fn with_time_held_and_joined_provider_sources() -> Self {
        Self::build_with_source_setup(None, true, true, true, |_| {}).await
    }

    /// As [`Rig::cramped`], with the clock of the API process held (see
    /// [`Rig::with_time_held`]): how long a reader may take nothing is over when
    /// the test says so.
    pub async fn cramped_with_time_held(change: impl FnOnce(&mut Tuning)) -> Self {
        Self::build(Some(CRAMPED_BUFFER), true, change).await
    }

    /// Moves the API process's clock forward; the rig was built with its time held.
    pub fn let_time_pass(&self, by: Span) {
        self.held_time
            .as_ref()
            .expect("the rig was built with the server's time held")
            .advance(by);
    }

    async fn build(
        send_buffer: Option<u32>,
        hold_time: bool,
        change: impl FnOnce(&mut Tuning),
    ) -> Self {
        Self::build_mode(send_buffer, hold_time, true, change).await
    }

    async fn build_mode(
        send_buffer: Option<u32>,
        hold_time: bool,
        start_engine: bool,
        change: impl FnOnce(&mut Tuning),
    ) -> Self {
        Self::build_with_source_setup(send_buffer, hold_time, start_engine, false, change).await
    }

    async fn build_with_source_setup(
        send_buffer: Option<u32>,
        hold_time: bool,
        start_engine: bool,
        join_sources: bool,
        change: impl FnOnce(&mut Tuning),
    ) -> Self {
        assert!(
            !join_sources || hold_time && start_engine,
            "joined source setup retains the normal started engine and a held observation clock"
        );
        let db = Arc::new(TestDb::fresh().await);
        let pool = Db::from_pool(
            PgPoolOptions::new()
                .max_connections(24)
                .connect(&db.url)
                .await
                .expect("a pool of its own"),
        );
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let now = clock.now();
        let mut conn = pool.acquire().await.unwrap();
        let ada = new_id();
        insert_user(&mut conn, ada, "ada").await.unwrap();
        let (root, project, topic) = (new_id(), new_id(), new_id());
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        access_testing::insert_member(&mut conn, root, ada, root)
            .await
            .unwrap();
        insert_node(&mut conn, project, "project", Some(root), "Web", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        for node in [root, project, topic] {
            access_testing::name_owner(&mut conn, node, ("user", ada), now)
                .await
                .unwrap();
        }
        let page = PageKey::new("system", SYSTEM_ID, "defaults");
        for shipped in contract_pages::shipped() {
            access_testing::add_entry(
                &mut conn,
                &page,
                shipped.profile.as_str(),
                shipped.permission.as_str(),
                ("role", None, Some(shipped.role.as_str())),
                None,
                now,
            )
            .await
            .unwrap();
        }
        let machine_id = insert_machine(&mut conn, "m1", "connected").await.unwrap();
        sqlx::query("UPDATE machines SET roles = ARRAY['agent','environment','provider'], online = true,
                     facts = facts || $2 WHERE id = $1")
            .bind(machine_id)
            .bind(json!({
                "adapters": [{ "kind": "fake", "adapter_version": 1, "builtin": true }],
                "harnesses": [{ "kind": "fake", "path": "/test/harness", "version": "1.0.0", "daemon": false }],
                "policy": { "agent.harnesses": [{ "kind": "fake", "path": "/test/harness" }] }
            }))
            .execute(&mut *conn)
            .await
            .unwrap();
        let (agent, provider, environment, template) = (new_id(), new_id(), new_id(), new_id());
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
             VALUES ($1, 'Fake agent', $2, $3, $3, 'fake', '{}', $4)",
        )
        .bind(agent)
        .bind(ada)
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
        .bind(ada)
        .bind(root)
        .bind(agent)
        .bind(machine_id)
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
        .bind(ada)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'template', 'test', '{}')")
            .bind(template)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                       template_id, generation, status)
             VALUES ($1, 'Workspace', $2, $3, 'managed', $4, '/workspace', $5, $6, 1, 'running')",
        )
        .bind(environment)
        .bind(ada)
        .bind(root)
        .bind(topic)
        .bind(machine_id)
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
        // owner_id records attribution. The same Owners pages as resource
        // creation give the fixture maker actual HTTP view/manage authority.
        for (kind, id) in [
            (
                orch_access_contract::permissions::ResourceKind::Agent,
                agent,
            ),
            (
                orch_access_contract::permissions::ResourceKind::Provider,
                provider,
            ),
            (
                orch_access_contract::permissions::ResourceKind::Environment,
                environment,
            ),
        ] {
            orch_authz::ownership::start_resource(
                &mut conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: kind.into(),
                    id,
                },
                ada,
                orch_authz::ownership::NewAccess::Defaults,
                Time::new(now),
            )
            .await
            .unwrap();
        }
        let token = new_token(ACCESS_TOKEN_PREFIX);
        insert_session(
            &mut conn,
            new_id(),
            ada,
            &token_hash(&token),
            now + Span::days(1),
        )
        .await
        .unwrap();
        drop(conn);

        // The machine and the hub between it and the system.
        let pki = TestPki::new(Arc::clone(&clock));
        let identity = pki.machine(machine_id);
        let machines = Hub::new(Arc::clone(&clock), pki.certificate());
        let roles = vec![Role::Agent, Role::Environment, Role::Provider];
        let (link, notices) = machines.add_machine(
            machine_id,
            roles.clone(),
            Default::default(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        let machine = Arc::new(ScriptedMachine::start(
            &machines,
            &clock,
            &identity,
            &MachineSpec {
                roles,
                ..MachineSpec::default()
            },
        ));
        let commands = Arc::new(ScriptedCommands {
            link: link.clone(),
            issued: Mutex::new(HashMap::new()),
            clock: Arc::clone(&clock),
        });

        let mut configured = ServerConfig::default();
        configured.system.public_url = "https://orch.test".into();
        configured.gateway.endpoints[0].url = "wss://gateway.test:9443".into();
        let config = Arc::new(configured);
        let mut tuning = quick(&config);
        change(&mut tuning);
        // Every row of the rig is older than a clock that is held from here on.
        let held_time = hold_time.then(|| ManualClock::new(clock.now()));
        let api_clock: Arc<dyn Clock> = match &held_time {
            Some(held) => Arc::new(held.clone()),
            None => Arc::clone(&clock),
        };
        let realtime = Realtime::tuned(&pool, &api_clock, tuning);
        let engine = Engine::new(EngineParts {
            db: pool.clone(),
            clock: Arc::clone(&api_clock),
            config: engine_config(),
            commands: commands.clone(),
            access: Arc::new(OpenAccess),
            live: realtime.sink(),
        });
        if start_engine && !join_sources {
            engine.start();
        }
        realtime.start(engine.clone());
        realtime.listening().await;
        let api_link =
            Link::detached(pool.clone(), Arc::clone(&clock), Arc::clone(&config)).unwrap();
        let app: App = Arc::new(AppState {
            builtin_sources: Arc::default(),
            host_ports: Default::default(),
            db: pool.clone(),
            config,
            clock: Arc::clone(&api_clock),
            link: api_link,
            engine: engine.clone(),
            realtime: realtime.clone(),
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs: orch_api_core::blobs::Blobs::in_memory(),
            cache: orch_api_core::cache::Cache::detached(),
        });
        let server = Server::serve(
            app,
            realtime,
            engine.clone(),
            send_buffer,
            Some(Arc::clone(&db)),
        )
        .await;

        let rig = Self {
            db,
            pool,
            clock: api_clock,
            server,
            machines,
            machine,
            machine_id,
            ada,
            root,
            project,
            topic,
            agent,
            provider,
            environment,
            token,
            commands,
            seen: Arc::new(Mutex::new(Vec::new())),
            sequence: AtomicU64::new(0),
            publishing: tokio::sync::Mutex::new(()),
            held_time,
        };
        rig.answer_commands();
        rig.spawn_applier(engine, link, notices, !start_engine);
        let machines = rig.machines.clone();
        eventually("the machine connects", || {
            let up = machines.session_of(machine_id).is_some();
            async move { up }
        })
        .await;
        if join_sources {
            rig.join_initial_provider_sources().await;
            rig.server.engine.start();
        }
        rig
    }

    async fn join_initial_provider_sources(&self) {
        tokio::time::timeout(PATIENCE, async {
            let source_kinds = [CommandKind::ProviderRefresh, CommandKind::ProviderDiscoverModels];
            assert!(lock(&self.commands.issued).values().all(|entry| !source_kinds.iter().any(|kind| entry.cmd.kind == kind.as_str())),
                "provider setup must own the initial source operation, not race an earlier unjoined worker");
            let facts = self.server.engine.refresh_provider(self.provider).await
                .unwrap_or_else(|_| panic!("provider setup phase=facts: the actual initial observation failed internally"));
            assert!(matches!(facts, orch_engine::provider_setup::Refreshed::Failed(_)),
                "provider setup phase=facts: the unchanged scripted native reader must report its real unsupported refusal");
            let models = self.server.engine.discover_provider_models(self.provider).await
                .unwrap_or_else(|_| panic!("provider setup phase=models: the actual initial observation failed internally"));
            assert!(models.is_none(), "provider setup phase=models: the unchanged native script supplies no model list");
            let issued: Vec<Cmd> = lock(&self.commands.issued).values()
                .filter(|entry| source_kinds.iter().any(|kind| entry.cmd.kind == kind.as_str()))
                .map(|entry| entry.cmd.clone()).collect();
            assert_eq!(issued.len(), 2, "one awaited facts read and one awaited model read form this initial setup");
            for kind in source_kinds {
                let selected: Vec<_> = issued.iter().filter(|cmd| cmd.kind == kind.as_str()).collect();
                assert_eq!(selected.len(), 1, "each native source operation has exactly one admitted identity");
                let cmd = selected[0];
                assert!(cmd.body["provider_id"] == self.provider.to_string()
                    && cmd.key == orch_link_contract::commands::SerialKey::Provider(self.provider).wire(),
                    "the completed source command must belong to this provider and its actual native queue");
                assert_eq!(self.machine.handler.runs_of(cmd.id), 1, "the real scripted machine executes this source identity once");
                let status = self.commands.status(cmd.id).await
                    .unwrap_or_else(|_| panic!("provider setup phase=terminal: the native command supplier could not be read"))
                    .expect("provider setup phase=terminal: the admitted native source identity must remain present");
                assert!(matches!(status.outcome, Some(Outcome::Failed(error)) if error.known() == Some(ErrorCode::Unsupported)),
                    "provider setup phase=terminal: accepted/sent/timeout is not the actual unsupported native outcome");
                let bytes = tokio::fs::read(self.machine.data_dir().join("journal").join(format!("{}.json", cmd.id))).await
                    .unwrap_or_else(|_| panic!("provider setup phase=journal: the exact native completion receipt is unavailable"));
                let journal: orch_link::journal::Entry = serde_json::from_slice(&bytes)
                    .unwrap_or_else(|_| panic!("provider setup phase=journal: the native receipt is not a complete entry"));
                assert!(journal.cmd == *cmd, "provider setup phase=journal: native body/key/deadline must equal the actual admitted command");
                let result = journal.result().expect("provider setup phase=journal: only durable native Done completes this setup");
                assert!(result.id == cmd.id && result.outcome.as_ref().is_err_and(|error| error.known() == Some(ErrorCode::Unsupported)),
                    "provider setup phase=journal: the native Done receipt must retain the same actual unsupported outcome");
            }
            let mut conn = self.pool.acquire().await
                .unwrap_or_else(|_| panic!("provider setup phase=publication: the metadata connection is unavailable"));
            let (sources, asked): (Json, Option<bool>) = sqlx::query_as("SELECT sources,facts_requested_at=$2 FROM providers WHERE id=$1")
                .bind(self.provider).bind(self.clock.now()).fetch_one(&mut *conn).await
                .unwrap_or_else(|_| panic!("provider setup phase=publication: the exact provider source state is unavailable"));
            assert!(asked == Some(true), "provider setup phase=publication: the real facts attempt must claim the held observation clock");
            for element in ["facts", "models"] {
                let error: orch_provider_contract::SourceChoiceLastError = serde_json::from_value(sources[element]["last_error"].clone())
                    .unwrap_or_else(|_| panic!("provider setup phase=publication: each actual failed native read must publish its bounded source error"));
                assert!(error.code == orch_provider_contract::SourceErrorCode::Unavailable && error.at == self.clock.time(),
                    "provider setup phase=publication: the actual native refusal must publish at this unchanged source's held observation cut");
                assert!(sources[element]["by"] == "default" && sources[element].get("descriptor").is_none_or(Value::is_null)
                    && sources[element]["described_at"].is_null(),
                    "provider setup phase=publication: a failed read cannot invent another choice or learned descriptor");
            }
            let configured = &self.server.engine.config().provider_facts;
            // LIMIT 1 is an existence check: no due result means no scheduled
            // source task can begin at this fixed clock after the real join.
            let due = orch_db::queries::provider_facts::due_reads(&mut conn, self.clock.now(),
                orch_db::queries::provider_facts::ReadSchedule { recent: configured.recent, active: configured.active,
                    idle: configured.idle, refused: configured.refused }, 1).await
                .unwrap_or_else(|_| panic!("provider setup phase=cadence: the actual source admission schedule could not be read"));
            assert!(due.is_empty(), "provider setup phase=cadence: no source read is due before starting the normal supervisor");
        }).await.expect("the actual initial source chain must complete within the original fixture patience");
    }

    /// What the API does with a machine's events: applies them in a transaction
    /// that moves the stream's cursor, and tells the link they are committed.
    fn spawn_applier(
        &self,
        engine: Engine,
        link: ControllerLink,
        mut notices: mpsc::UnboundedReceiver<Notice>,
        forward_reports: bool,
    ) {
        let (pool, machine) = (self.pool.clone(), self.machine_id);
        let clock = Arc::clone(&self.clock);
        let database = Arc::clone(&self.db);
        let stopping = self.server.shutdown.clone();
        self.server.own(async move {
            let _database = database;
            loop {
                let notice = tokio::select! { biased;
                    () = stopping.cancelled() => break,
                    notice = notices.recv() => match notice { Some(notice) => notice, None => break },
                };
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
                    Notice::Report(eph) if forward_reports => {
                        orch_api_core::realtime::ephemeral::forward_report(&pool,
                            &orch_api_core::backplane::Report {machine_id:machine,eph}).await
                            .expect("the metadata fixture forwards the actual machine's transient report");
                    }
                    _ => {}
                }
            }
        });
    }

    /// Retire native fixture owners while the runtime can still complete their DB work.
    pub async fn shutdown(&self) {
        tokio::time::timeout(PATIENCE, async {
            self.server.engine.stop();
            self.server.app.link.shutdown();
            self.server.realtime.stop().await;
            self.machine.link.shutdown().await;
            self.server.stop().await;
            eventually("all fixture socket readers to retire", || async {
                self.server.realtime.connections() == 0
            })
            .await;
            self.pool.pool().close().await;
            self.db.pool.close().await;
            assert_eq!(self.server.task_owners(), 0);
            assert_eq!(self.pool.pool().size(), 0);
            assert_eq!(self.db.pool.size(), 0);
        })
        .await
        .expect("the complete joined fixture shutdown stays within its original patience");
    }

    /// The scripted machine answers the commands a harness answers.
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
            CommandKind::LeaseOpen,
            CommandKind::LeaseClose,
            CommandKind::OrchToolResult,
        ] {
            let seen = Arc::clone(&self.seen);
            handler.on(kind, move |command| {
                lock(&seen).push(command.clone());
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
                    // A session opened where another one of the thread stood (after a switch) is
                    // opened as any other, in the lane the system names.
                    Command::SessionOpenFenced(fenced) => {
                        CommandResult::SessionOpenFenced(SessionOpenResult {
                            native_session_ref: fenced
                                .session
                                .native_session_ref
                                .clone()
                                .unwrap_or_else(|| format!("native-{}", fenced.session.segment_id)),
                            resumed: fenced.session.mode == SessionMode::Resume,
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
                    Command::LeaseOpen(_) => CommandResult::LeaseOpen(Empty {}),
                    Command::LeaseClose(_) => CommandResult::LeaseClose(
                        serde_json::from_value(json!({ "requests": 0, "usage": null })).map_err(
                            |e| {
                                orch_link_contract::commands::CommandError::new(
                                    ErrorCode::Internal,
                                    e.to_string(),
                                )
                            },
                        )?,
                    ),
                    Command::OrchToolResult(_) => CommandResult::OrchToolResult(Empty {}),
                    other => {
                        return Err(orch_link_contract::commands::CommandError::new(
                            ErrorCode::Unsupported,
                            format!("no script for {}", other.kind()),
                        ));
                    }
                })
            });
        }
    }

    // ------------------------------------------------------------ the tree

    /// A thread in the topic, bound to the agent, the provider and the environment.
    pub async fn thread(&self, name: &str) -> Id {
        let created = self
            .server
            .engine
            .create_thread(
                &Caller::session(self.ada),
                orch_engine::binding::NewThread {
                    topic: self.topic,
                    name: name.to_owned(),
                    agent: self.agent,
                    provider: self.provider,
                    environment: self.environment,
                    directory: "web".to_owned(),
                    arrangement: None,
                    model: Some(MODEL.to_owned()),
                    effort: None,
                },
            )
            .await;
        match created {
            Ok(created) => created.thread,
            Err(error) => {
                // An arrangement is chosen from what the machines say of themselves.
                let machines: Vec<(Vec<String>, bool, serde_json::Value)> =
                    sqlx::query_as("SELECT roles, online, facts FROM machines")
                        .fetch_all(self.pool.pool())
                        .await
                        .unwrap_or_default();
                panic!("a thread is created: {error:?}; the machines: {machines:?}")
            }
        }
    }

    /// A person who is a member of the root org and holds nothing else, with an access token.
    pub async fn person(&self, name: &str) -> (Id, String) {
        let mut conn = self.pool.acquire().await.unwrap();
        let id = new_id();
        insert_user(&mut conn, id, name).await.unwrap();
        access_testing::insert_member(&mut conn, self.root, id, self.root)
            .await
            .unwrap();
        let token = self.token_of(&mut conn, id, Span::days(1)).await;
        (id, token)
    }

    /// An access token of `user` that is good for `lifetime`.
    pub async fn token_for(&self, user: Id, lifetime: Span) -> String {
        let mut conn = self.pool.acquire().await.unwrap();
        self.token_of(&mut conn, user, lifetime).await
    }

    async fn token_of(&self, conn: &mut Conn, user: Id, lifetime: Span) -> String {
        let token = new_token(ACCESS_TOKEN_PREFIX);
        insert_session(
            conn,
            new_id(),
            user,
            &token_hash(&token),
            self.held_time
                .as_ref()
                .map_or_else(|| self.clock.now(), Clock::now)
                + lifetime,
        )
        .await
        .unwrap();
        token
    }

    /// Gives `role` on the topic to a person; the row that gives it is returned, to take it away again.
    pub async fn give(&self, person: Id, role: &str) -> Id {
        let mut conn = self.pool.acquire().await.unwrap();
        access_testing::give_role(&mut conn, self.topic, "user", person, role, self.ada, None)
            .await
            .unwrap()
    }

    /// Takes a role away, and moves the authorization epoch as a change of access does.
    pub async fn take(&self, role: Id) {
        let mut conn = self.pool.acquire().await.unwrap();
        access_testing::take_role_away(&mut conn, role, self.ada, self.clock.now())
            .await
            .unwrap();
        orch_db::queries::access::bump_epoch(&mut conn)
            .await
            .unwrap();
    }

    /// Moves the authorization epoch, as every change of who may do what does.
    pub async fn bump_epoch(&self) {
        let mut conn = self.pool.acquire().await.unwrap();
        orch_db::queries::access::bump_epoch(&mut conn)
            .await
            .unwrap();
    }

    /// Suspends a person everywhere, and moves the epoch as a suspension does.
    pub async fn suspend(&self, user: Id) {
        let mut conn = self.pool.acquire().await.unwrap();
        orch_db::queries::identity::testing::suspend(&mut conn, user)
            .await
            .unwrap();
        orch_db::queries::access::bump_epoch(&mut conn)
            .await
            .unwrap();
    }

    /// An API token of a person, which is no key to the socket.
    pub async fn api_token(&self, user: Id) -> String {
        let mut conn = self.pool.acquire().await.unwrap();
        let token = new_token(orch_api_core::middleware::authentication::API_TOKEN_PREFIX);
        orch_db::queries::identity::testing::insert_api_token(&mut conn, user, &token_hash(&token))
            .await
            .unwrap();
        token
    }

    /// Ends the session an access token belongs to, as signing out does.
    pub async fn sign_out(&self, token: &str) {
        sqlx::query(
            "UPDATE sessions SET revoked_at = now()
              WHERE id = (SELECT session_id FROM access_tokens WHERE token_hash = $1)",
        )
        .bind(token_hash(token))
        .execute(self.pool.pool())
        .await
        .unwrap();
    }

    // ------------------------------------------------------------ the database, as the tests read it

    /// The state of a turn.
    pub async fn turn_state(&self, turn: Id) -> String {
        sqlx::query_scalar("SELECT state FROM turns WHERE id = $1")
            .bind(turn)
            .fetch_one(self.pool.pool())
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

    /// The segment a thread is in now.
    pub async fn segment_of(&self, thread: Id) -> Id {
        sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(self.pool.pool())
            .await
            .unwrap()
    }

    /// What the thread's own request for it returns, from the start.
    pub async fn rows(&self, thread: Id) -> orch_engine::views::ThreadSyncData {
        self.server
            .engine
            .sync(thread, Some(self.ada), None, 500)
            .await
            .unwrap()
    }

    /// The text of every agent message of a thread, by item.
    pub async fn texts(&self, thread: Id) -> BTreeMap<Id, String> {
        self.rows(thread)
            .await
            .items
            .iter()
            .filter_map(|item| match &item.payload {
                orch_api_contract::threads::ItemPayload::AgentMessage(message) => {
                    Some((item.id, message.text.clone()))
                }
                _ => None,
            })
            .collect()
    }

    // ------------------------------------------------------------ the machine

    /// The `turn.start` commands the machine ran for a thread, in order.
    pub fn turn_starts(&self, thread: Id) -> Vec<TurnStart> {
        lock(&self.seen)
            .iter()
            .filter_map(|command| match command {
                Command::TurnStart(start) if start.thread_id == thread => Some(start.clone()),
                Command::TurnStartFenced(start) if start.thread_id == thread => {
                    Some((*start.turn).clone())
                }
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

    /// The harness says something about a thread's session.
    pub async fn say(&self, thread: Id, event: Event) {
        self.say_many(thread, std::slice::from_ref(&event)).await;
    }

    /// Hands over an ordered native batch and waits until its last report is committed.
    pub async fn say_many(&self, thread: Id, events: &[Event]) {
        if events.is_empty() {
            return;
        }
        // This scripted producer has one ordered event lane. Returning means
        // its own report committed, not only that its local spool accepted it.
        let _publishing = self.publishing.lock().await;
        let segment = self.segment_of(thread).await;
        if let Some(opening) =
            orch_testkit::doubles::sessions::prepare(self.pool.pool(), thread, segment)
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
            orch_testkit::doubles::sessions::opened(self.pool.pool(), segment, &result)
                .await
                .unwrap();
        }
        let stream = Stream::Session {
            thread_id: thread,
            segment_id: segment,
        };
        let bodies: Vec<_> = events.iter().map(|event| event.body().unwrap()).collect();
        let reports: Vec<_> = events
            .iter()
            .zip(&bodies)
            .map(|(event, body)| (event.kind().as_str(), body))
            .collect();
        let admitted = {
            let mut conn = self.pool.acquire().await.unwrap();
            orch_db::queries::machine_reports::accepts(
                &mut conn,
                self.machine_id,
                &stream,
                &reports,
            )
            .await
            .unwrap()
        };
        if !admitted {
            self.native_diagnostics(thread, "native source admission refused")
                .await;
            panic!(
                "fixture {} does not belong to its recorded native thread/segment/turn",
                events[0].kind().as_str()
            );
        }
        let before = self
            .commands
            .link
            .cursors()
            .get(&stream.to_string())
            .copied()
            .unwrap_or(0);
        let expected = before
            .checked_add(i64::try_from(events.len()).unwrap())
            .unwrap();
        // publish_now hands every event to the real spool before waiting for its durability,
        // allowing the link's normal batching and one disk flush for the ordered group.
        let handed: Vec<_> = events
            .iter()
            .map(|event| self.machine.link.publish_now(&stream, event))
            .collect();
        futures::future::try_join_all(handed).await.unwrap();
        let committed = tokio::time::timeout(PATIENCE, async {
            loop {
                if self
                    .commands
                    .link
                    .cursors()
                    .get(&stream.to_string())
                    .copied()
                    .unwrap_or(0)
                    >= expected
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if committed.is_err() {
            self.native_diagnostics(thread, "native receipt did not commit")
                .await;
            panic!(
                "fixture {} did not commit within {PATIENCE:?}",
                events[0].kind().as_str()
            );
        }
    }

    /// Evidence after a failed proof, never an alternate way to satisfy a
    /// socket assertion. Only state/identity/kinds are included, not content.
    pub async fn native_diagnostics(&self, thread: Id, phase: &str) {
        let state=tokio::time::timeout(Duration::from_secs(2),
            sqlx::query_scalar::<_,Json>("SELECT jsonb_build_object('state',t.state,
                'blocked',t.blocked_reason,'current_turn',t.current_turn_id,'current_segment',t.current_segment_id,
                'segment_machine',s.machine_id,'session_open',s.native_session_ref IS NOT NULL,
                'last_turn',(SELECT jsonb_build_object('id',u.id,'state',u.state,
                    'end_work',u.end_work IS NOT NULL,'native_end',u.native_end_confirmed,'seq',u.seq_updated)
                    FROM turns u WHERE u.thread_id=t.node_id ORDER BY u.ordinal DESC LIMIT 1),
                'max_machine_cursor',(SELECT max(last_seq) FROM link_cursors WHERE machine_id=$2))
                FROM threads t LEFT JOIN segments s ON s.id=t.current_segment_id WHERE t.node_id=$1")
                .bind(thread).bind(self.machine_id)
                .fetch_optional(&self.db.pool)).await;
        eprintln!("native Rig phase={phase} state={state:?}");
    }

    /// An input is sent and the machine is asked to run the turn; the turn is returned.
    pub async fn send(&self, thread: Id, text: &str) -> TurnStart {
        self.server
            .engine
            .submit_input(&Caller::session(self.ada), thread, queued(text))
            .await
            .unwrap();
        let nth = self.sequence.fetch_add(1, Ordering::SeqCst);
        self.turn_start(thread, usize::try_from(nth).unwrap()).await
    }

    /// The harness takes the turn up and begins a message.
    pub async fn begin_message(&self, thread: Id, turn: &TurnStart, native_ref: &str) {
        self.say(thread, turn_began(turn.turn_id)).await;
        self.say(thread, message_began(turn.turn_id, native_ref))
            .await;
    }

    /// The harness finishes the message and the turn, and what the end leaves to do after it has
    /// committed too: what is read next is the thread at rest.
    pub async fn finish_turn(&self, thread: Id, turn: &TurnStart, native_ref: &str, text: &str) {
        self.say(thread, message_done(native_ref, text)).await;
        self.say(thread, turn_ended(turn.turn_id, TurnOutcome::Completed))
            .await;
        self.wait_turn(turn.turn_id, "completed").await;
        self.wait_turn_followed_up(turn.turn_id).await;
    }

    /// Waits until the follow-up of a turn that ended (its usage, notifications, context and
    /// summaries) has committed: it is a transaction of its own after the end, and stamps the
    /// thread again.
    pub async fn wait_turn_followed_up(&self, turn: Id) {
        eventually("the turn's follow-up to commit", || async move {
            sqlx::query_scalar::<_, bool>("SELECT end_work IS NULL FROM turns WHERE id = $1")
                .bind(turn)
                .fetch_one(self.pool.pool())
                .await
                .unwrap()
        })
        .await;
    }
}

async fn set_online(pool: &Db, machine: Id, online: bool) {
    sqlx::query("UPDATE machines SET online = $2 WHERE id = $1")
        .bind(machine)
        .bind(online)
        .execute(pool.pool())
        .await
        .unwrap();
}

// ---------------------------------------------------------------- a client

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What a client heard.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// A frame, as JSON.
    Frame(Value),
    /// The socket was closed with this code.
    Closed(Option<u16>),
}

/// A client that speaks the protocol as a browser does, on a loopback socket.
pub struct Client {
    socket: Socket,
    /// How many pings were sent.
    pings: i64,
    pending: VecDeque<Value>,
}

fn request(
    addr: SocketAddr,
    protocols: &[&str],
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut request = format!("ws://{addr}/api/v1/ws")
        .into_client_request()
        .unwrap();
    if !protocols.is_empty() {
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_str(&protocols.join(", ")).unwrap(),
        );
    }
    request
}

impl Client {
    /// Connects, offering the protocol.
    pub async fn connect(addr: SocketAddr) -> Self {
        Self::connect_offering(addr, &["orch.v1"]).await
    }

    /// Connects, offering these subprotocols.
    pub async fn connect_offering(addr: SocketAddr, protocols: &[&str]) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(request(addr, protocols))
            .await
            .unwrap();
        Self {
            socket,
            pings: 0,
            pending: VecDeque::new(),
        }
    }

    /// A real WebSocket handshake carrying the supplied forwarded/browser headers.
    pub async fn connect_headers(addr: SocketAddr, headers: &[(&str, &str)]) -> Self {
        let mut request = request(addr, &["orch.v1"]);
        for (name, value) in headers {
            request.headers_mut().insert(
                tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes())
                    .unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        Self {
            socket,
            pings: 0,
            pending: VecDeque::new(),
        }
    }

    /// Connects with the little room in the kernel's buffers that makes a reader slow at once:
    /// what the server writes to it fills the line after a few kilobytes.
    pub async fn connect_cramped(addr: SocketAddr, receive_buffer: u32) -> Self {
        let socket = TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(receive_buffer).unwrap();
        let stream = socket.connect(addr).await.unwrap();
        let (socket, _) = tokio_tungstenite::client_async(
            request(addr, &["orch.v1"]),
            MaybeTlsStream::Plain(stream),
        )
        .await
        .unwrap();
        Self {
            socket,
            pings: 0,
            pending: VecDeque::new(),
        }
    }

    /// Sends a frame.
    pub async fn send(&mut self, frame: Value) {
        self.socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    /// Sends text that is whatever it is.
    pub async fn send_text(&mut self, text: &str) {
        self.socket
            .send(Message::Text(text.to_owned().into()))
            .await
            .unwrap();
    }

    /// Sends a binary frame.
    pub async fn send_binary(&mut self, bytes: &[u8]) {
        self.socket
            .send(Message::Binary(bytes.to_vec().into()))
            .await
            .unwrap();
    }

    /// The next thing the server did, within `wait`: a frame, or the close.
    pub async fn hear_within(&mut self, wait: Duration) -> Option<Heard> {
        if let Some(frame) = self.pending.pop_front() {
            return Some(Heard::Frame(frame));
        }
        self.hear_wire_within(wait).await
    }

    async fn hear_wire_within(&mut self, wait: Duration) -> Option<Heard> {
        let heard = tokio::time::timeout(wait, async {
            loop {
                match self.socket.next().await {
                    None => return Heard::Closed(None),
                    Some(Err(_)) => return Heard::Closed(None),
                    Some(Ok(Message::Text(text))) => {
                        return Heard::Frame(serde_json::from_str(text.as_str()).unwrap());
                    }
                    Some(Ok(Message::Close(frame))) => {
                        return Heard::Closed(frame.map(|frame| u16::from(frame.code)));
                    }
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        heard.ok()
    }

    /// The next frame, which has to come within the patience of a test.
    pub async fn frame(&mut self) -> Value {
        match self.hear_within(PATIENCE).await {
            Some(Heard::Frame(frame)) => frame,
            other => panic!("a frame was expected, and {other:?} came"),
        }
    }

    /// The next frame, which has to be `op`.
    pub async fn expect(&mut self, op: &str) -> Value {
        let frame = self.frame().await;
        assert_eq!(frame["op"], op, "{frame}");
        frame
    }

    /// Frames until one of `op` comes; the others are returned with it.
    pub async fn until(&mut self, op: &str) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let frame = self.frame().await;
            let found = frame["op"] == op;
            seen.push(frame);
            if found {
                return seen;
            }
        }
    }

    /// Nothing comes for a while.
    pub async fn is_quiet_for(&mut self, wait: Duration) -> bool {
        self.hear_within(wait).await.is_none()
    }

    /// The code the socket is closed with, which has to come within the patience of a test; frames before it are let go.
    pub async fn closed(&mut self) -> Option<u16> {
        let started = Instant::now();
        loop {
            let left = PATIENCE.saturating_sub(started.elapsed());
            match self.hear_within(left).await {
                Some(Heard::Closed(code)) => return code,
                Some(Heard::Frame(_)) => {}
                None => panic!("the socket was not closed"),
            }
        }
    }

    /// Says who this is and returns the `ready` frame.
    pub async fn auth(&mut self, token: &str) -> Value {
        self.send(json!({ "op": "auth", "token": token })).await;
        self.expect("ready").await
    }

    /// A connection that is authenticated.
    pub async fn signed_in(addr: SocketAddr, token: &str) -> Self {
        let mut client = Self::connect(addr).await;
        client.auth(token).await;
        client
    }

    /// Follows a feed.
    pub async fn sub(&mut self, id: i64, feed: &str, since: Option<i64>) {
        let mut frame = json!({ "op": "sub", "id": id, "feed": feed });
        if let Some(since) = since {
            frame["since"] = json!(since);
        }
        self.send(frame).await;
    }

    /// Follows the raw protocol traffic of `thread`'s native segment `segment`. A raw subscription
    /// names the segment and the projection the client holds, which is a new thread's first.
    pub async fn sub_raw(&mut self, id: i64, thread: Id, segment: Id) {
        self.send(json!({
            "op": "sub", "id": id, "feed": format!("thread:{thread}:raw"),
            "raw_segment_id": segment, "projection_revision": 1
        }))
        .await;
    }

    /// A ping, and the round trip until its pong in milliseconds.
    pub async fn ping(&mut self) -> u128 {
        self.pings += 1;
        let started = Instant::now();
        self.send(json!({ "op": "ping", "n": self.pings })).await;
        loop {
            let frame = self.frame().await;
            if frame["op"] == "pong" {
                assert_eq!(frame["n"], self.pings);
                return started.elapsed().as_millis();
            }
        }
    }

    /// Sends the required application heartbeat without consuming state frames.
    /// The history observer will process its pong through its ordinary read loop.
    pub async fn keep_alive(&mut self) {
        self.pings += 1;
        self.send(json!({"op":"ping","n":self.pings})).await;
    }

    /// A held-time test must observe the server hearing each heartbeat before
    /// advancing another interval. Retain every intervening frame for assertions.
    pub async fn heartbeat(&mut self) {
        self.keep_alive().await;
        let n = self.pings;
        tokio::time::timeout(PATIENCE, async {
            loop {
                match self.hear_wire_within(PATIENCE).await {
                    Some(Heard::Frame(frame)) if frame["op"] == "pong" && frame["n"] == n => break,
                    Some(Heard::Frame(frame)) => self.pending.push_back(frame),
                    other => {
                        panic!("the active socket did not acknowledge its heartbeat: {other:?}")
                    }
                }
            }
        })
        .await
        .expect("the heartbeat keeps its original fixture deadline");
    }
}

// ---------------------------------------------------------------- what a browser keeps

/// What a browser keeps of one thread: its rows by identifier and the text of
/// its messages, built as the protocol says a client builds it.
#[derive(Debug, Default)]
pub struct Mirror {
    pub cursor: Option<FeedCursor>,
    /// Rows by identifier: the item, as the last frame said it.
    pub items: BTreeMap<Id, Value>,
    /// The text of each message, as the client holds it.
    pub text: BTreeMap<Id, String>,
    applied: HashMap<Id, Applied>,
    /// The sequences the `sync` frames said, in the order they came.
    pub seqs: Vec<(i64, i64)>,
    /// Deltas that were appended.
    pub appended: usize,
    /// Frames that asked the client to read again.
    pub resyncs: usize,
    /// Deltas that were ignored, being repeats or older than a row.
    pub ignored: usize,
    /// Deltas that left a hole.
    pub gaps: usize,
}

impl Mirror {
    /// A client that holds nothing of the thread yet, as of `seq`.
    pub fn loaded(seq: i64) -> Self {
        Self {
            cursor: Some(FeedCursor::loaded(seq)),
            ..Self::default()
        }
    }

    /// Takes in a frame of the thread's feed.
    pub fn take(&mut self, frame: &Value) {
        match frame["op"].as_str() {
            Some("sync") => self.sync(frame),
            Some("delta") => self.delta(frame),
            Some("resync") => self.resyncs += 1,
            _ => {}
        }
    }

    fn sync(&mut self, frame: &Value) {
        let (prev, seq) = (
            frame["prev"].as_i64().unwrap(),
            frame["seq"].as_i64().unwrap(),
        );
        self.seqs.push((prev, seq));
        if let Some(cursor) = self.cursor.as_mut() {
            match cursor.on_sync(prev, seq) {
                FrameAction::Apply => {}
                FrameAction::Drop => return,
                FrameAction::Resync => {
                    self.resyncs += 1;
                    return;
                }
            }
        }
        for item in frame["data"]["items"].as_array().into_iter().flatten() {
            self.row(item);
        }
    }

    fn row(&mut self, item: &Value) {
        let id: Id = item["id"].as_str().unwrap().parse().unwrap();
        let stamp = item["seq_updated"].as_i64().unwrap();
        if self
            .items
            .get(&id)
            .is_some_and(|held| held["seq_updated"].as_i64().unwrap() >= stamp)
        {
            return;
        }
        self.items.insert(id, item.clone());
        if item["kind"] == "agent_message" {
            let row_text = item["payload"]["text"].as_str().unwrap_or_default();
            let len = item["payload"]["text_len"].as_i64().unwrap();
            let applied = self
                .applied
                .entry(id)
                .or_insert_with(|| Applied::from_row(0));
            if applied.on_row(len) == RowAction::Replace {
                self.text.insert(id, row_text.to_owned());
            }
        }
    }

    fn delta(&mut self, frame: &Value) {
        if frame["field"] != "text" {
            return;
        }
        let id: Id = frame["item_id"].as_str().unwrap().parse().unwrap();
        let Some(applied) = self.applied.get_mut(&id) else {
            // The row of the item has not come: the text comes with it.
            self.ignored += 1;
            return;
        };
        let (offset, len) = (
            frame["offset"].as_i64().unwrap(),
            frame["len"].as_i64().unwrap(),
        );
        match applied.on_delta(offset, len) {
            DeltaAction::Append => {
                self.text
                    .entry(id)
                    .or_default()
                    .push_str(frame["data"].as_str().unwrap());
                self.appended += 1;
            }
            DeltaAction::Ignore => self.ignored += 1,
            DeltaAction::Gap => self.gaps += 1,
        }
    }
}

// ---------------------------------------------------------------- a slow link

/// A line between a client and a server that delays what passes in each
/// direction and limits how fast the server's side reaches the client, as a
/// slow link does. What a client connects to is [`SlowLine::addr`].
pub struct SlowLine {
    pub addr: SocketAddr,
}

async fn relay(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    delay: Duration,
    bytes_per_second: Option<u64>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // What was read waits here until its time, so that the delay is a delay of
    // the line and not of the reader. The line holds little, as a slow one does:
    // when it is full the sender is held back, which is what a slow reader is.
    let (waiting, mut arrived) = mpsc::channel::<(tokio::time::Instant, Vec<u8>)>(2);
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 2_048];
        loop {
            match from.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    let at = tokio::time::Instant::now() + delay;
                    if waiting.send((at, buffer[..read].to_vec())).await.is_err() {
                        return;
                    }
                }
            }
        }
    });
    while let Some((at, chunk)) = arrived.recv().await {
        tokio::time::sleep_until(at).await;
        if to.write_all(&chunk).await.is_err() {
            return;
        }
        if let Some(rate) = bytes_per_second {
            let seconds = chunk.len() as f64 / rate as f64;
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        }
    }
    let _ = to.shutdown().await;
}

/// A connection to `target` with little room in the kernel for what it receives.
async fn connect_cramped(target: SocketAddr) -> std::io::Result<TcpStream> {
    let socket = TcpSocket::new_v4()?;
    socket.set_recv_buffer_size(CRAMPED_BUFFER)?;
    socket.connect(target).await
}

impl SlowLine {
    /// A line to `target`: each way takes `one_way`, and the server reaches
    /// the client at `down` bytes a second at most.
    pub async fn shaped(target: SocketAddr, one_way: Duration, down: Option<u64>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = connect_cramped(target).await else {
                    continue;
                };
                let _ = client.set_nodelay(true);
                let _ = server.set_nodelay(true);
                let (client_read, client_write) = client.into_split();
                let (server_read, server_write) = server.into_split();
                tokio::spawn(relay(client_read, server_write, one_way, None));
                tokio::spawn(relay(server_read, client_write, one_way, down));
            }
        });
        Self { addr }
    }
}
