//! A floor to test shells on: the usual scene of people, a topic, an environment and a thread, with a machine behind a hub that keeps shells as a connector does, a place that keeps their last output, a server on a loopback socket, and a client that speaks to it as a browser does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::Who;
use crate::rig::{Scene, eventually};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use orch_api_contract::terminals::{
    EnvironmentTerminalCreate, EnvironmentTerminalCreateReply, EnvironmentTerminalList,
    EnvironmentTerminalListQuery, TerminalClose, TerminalSession, TerminalState,
    TerminalTicketCreate,
};
use orch_api_core::realtime::{Realtime, Tuning};
use orch_api_core::registry::Registry;
use orch_api_core::state::{App, AppState};
use orch_api_core::testing::{Reply, TestApp};
use orch_build_info::BuildInfo;
use orch_engine::shells::{RecordingFault, Recordings, install_recordings};
use orch_link::tunnels::channel::{IncomingTunnel, IncomingTunnels, Tunnel, TunnelEvent};
use orch_link_contract::commands::{
    AgentPtyOpenResult, BlobRef, Command, CommandError, CommandKind, CommandResult, ErrorCode,
    PtyCloseResult, PtyOpen, PtyOpenResult,
};
use orch_link_contract::tunnels::{OpenErrorCode, Service};
use orch_testkit::doubles::machine::ScriptedMachine;
use orch_types::{Hash, Id};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long a test waits for something that has to happen.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// The most a shell keeps of what it printed, for a screen that comes back: what a connector keeps.
const KEPT_BYTES: usize = 256 * 1024;

// ---------------------------------------------------------------- the place where last outputs are kept

/// Where the last output of shells is put away in these tests: in memory, by hash, with the values of the
/// secrets taken out where the engine asks for that.
#[derive(Default)]
pub struct Store {
    blobs: Mutex<HashMap<Hash, Vec<u8>>>,
    secrets: Mutex<Vec<String>>,
}

impl Store {
    /// The store every test of the process shares: the engine is given one place to keep recordings.
    pub fn shared() -> &'static Arc<Store> {
        static STORE: OnceLock<Arc<Store>> = OnceLock::new();
        STORE.get_or_init(|| {
            let store = Arc::new(Store::default());
            install_recordings(store.clone());
            store
        })
    }

    /// Content as a machine uploaded it, and its reference.
    pub fn uploaded(&self, content: &[u8]) -> BlobRef {
        let hash = Hash::of(content);
        lock(&self.blobs).insert(hash, content.to_vec());
        BlobRef {
            hash,
            size: i64::try_from(content.len()).unwrap(),
        }
    }

    /// A value that is a secret of the environments of these tests.
    pub fn knows_secret(&self, value: &str) {
        lock(&self.secrets).push(value.to_owned());
    }
}

#[async_trait]
impl Recordings for Store {
    async fn keep(
        &self,
        _environment: Id,
        kept: BlobRef,
        mask: bool,
    ) -> Result<BlobRef, RecordingFault> {
        let raw = lock(&self.blobs)
            .get(&kept.hash)
            .cloned()
            .ok_or_else(|| RecordingFault("there is no such blob".to_owned()))?;
        let masked = if mask {
            let secrets = lock(&self.secrets).clone();
            let values: Vec<&str> = secrets.iter().map(String::as_str).collect();
            orch_engine::shells::mask_secrets(&raw, &values)
        } else {
            raw
        };
        Ok(self.uploaded(&masked))
    }

    async fn read(&self, hash: &Hash) -> Result<Option<Vec<u8>>, RecordingFault> {
        Ok(lock(&self.blobs).get(hash).cloned())
    }
}

// ---------------------------------------------------------------- the machine's shells

/// What reaches a shell's attachment from outside.
enum Signal {
    /// Another tunnel was opened to the shell.
    Taken,
    /// The shell was closed.
    Closed,
}

/// A shell as a connector keeps it: output remembered for whoever comes back, one attachment at a
/// time, a small language to type (`echo words`, `exit 3`, `flood 1000000`), and the size of its screen.
pub struct FakeShell {
    pub id: Id,
    output: Mutex<Vec<u8>>,
    pub size: Mutex<(u16, u16)>,
    pub typed: Mutex<Vec<u8>>,
    attachment: Mutex<Option<mpsc::UnboundedSender<Signal>>>,
    pub exit: Mutex<Option<i64>>,
    pub printed: AtomicI64,
    pub written_to_tunnel: AtomicI64,
    pub waiting_for_tunnel: AtomicBool,
    pub busy: AtomicBool,
    /// A deliberate pause inside replay proves that its boundary is independent of timing.
    pub replay_pause: Mutex<Option<Duration>>,
    pub live_after_replay: Mutex<Vec<u8>>,
}

impl FakeShell {
    fn new(id: Id, size: (u16, u16)) -> Arc<Self> {
        Arc::new(Self {
            id,
            output: Mutex::new(Vec::new()),
            size: Mutex::new(size),
            typed: Mutex::new(Vec::new()),
            attachment: Mutex::new(None),
            exit: Mutex::new(None),
            printed: AtomicI64::new(0),
            written_to_tunnel: AtomicI64::new(0),
            waiting_for_tunnel: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            replay_pause: Mutex::new(None),
            live_after_replay: Mutex::new(Vec::new()),
        })
    }

    /// What the shell remembers of what it printed.
    pub fn screen(&self) -> Vec<u8> {
        lock(&self.output).clone()
    }

    /// Everything that was typed into it.
    pub fn typed_text(&self) -> String {
        String::from_utf8_lossy(&lock(&self.typed)).into_owned()
    }

    fn remember(&self, bytes: &[u8]) {
        let mut output = lock(&self.output);
        output.extend_from_slice(bytes);
        let over = output.len().saturating_sub(KEPT_BYTES);
        output.drain(..over);
        self.printed
            .fetch_add(i64::try_from(bytes.len()).unwrap(), Ordering::SeqCst);
    }

    /// Prints something while nobody is attached: the shell runs on by itself.
    pub fn print_alone(&self, text: &str) {
        self.remember(text.as_bytes());
    }

    /// Keeps arbitrary terminal bytes, including invalid UTF-8, for reconnect tests.
    pub fn print_bytes_alone(&self, bytes: &[u8]) {
        self.remember(bytes);
    }

    async fn print(&self, tunnel: &mut Tunnel, bytes: &[u8]) -> bool {
        self.remember(bytes);
        for piece in bytes.chunks(16 * 1024) {
            let mut writing = std::pin::pin!(tunnel.write_all(piece));
            let result = std::future::poll_fn(|cx| {
                let result = writing.as_mut().poll(cx);
                self.waiting_for_tunnel
                    .store(result.is_pending(), Ordering::SeqCst);
                result
            })
            .await;
            if result.is_err() {
                return false;
            }
            self.written_to_tunnel
                .fetch_add(i64::try_from(piece.len()).unwrap(), Ordering::SeqCst);
        }
        true
    }

    /// Serves one attachment until it ends.
    async fn serve(self: Arc<Self>, mut tunnel: Tunnel) {
        let (signals, mut told) = mpsc::unbounded_channel();
        // One screen at a time: a second tunnel ends the first, and says why.
        let before = lock(&self.attachment).replace(signals);
        if let Some(before) = before {
            let _ = before.send(Signal::Taken);
        }
        let mut events = tunnel.take_events().unwrap();
        let replay = self.screen();
        if tunnel
            .write_all(&u64::try_from(replay.len()).unwrap().to_be_bytes())
            .await
            .is_err()
        {
            return;
        }
        if !replay.is_empty() {
            self.written_to_tunnel
                .fetch_add(i64::try_from(replay.len()).unwrap(), Ordering::SeqCst);
            let pause = *lock(&self.replay_pause);
            if let Some(pause) = pause {
                let split = replay.len() / 2;
                if tunnel.write_all(&replay[..split]).await.is_err() {
                    return;
                }
                tokio::time::sleep(pause).await;
                if tunnel.write_all(&replay[split..]).await.is_err() {
                    return;
                }
            } else if tunnel.write_all(&replay).await.is_err() {
                return;
            }
        }
        let live = lock(&self.live_after_replay).clone();
        if !live.is_empty() && tunnel.write_all(&live).await.is_err() {
            return;
        }
        let mut line = String::new();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            tokio::select! {
                signal = told.recv() => {
                    match signal {
                        Some(Signal::Taken) => tunnel.fail("another screen took this shell"),
                        Some(Signal::Closed) | None => {
                            tunnel.exited(lock(&self.exit).unwrap_or(-1));
                            tunnel.close();
                        }
                    }
                    return;
                }
                event = events.recv() => {
                    if let Some(TunnelEvent::Resize { cols, rows }) = event {
                        *lock(&self.size) = (cols, rows);
                    }
                }
                read = tunnel.read(&mut buffer) => {
                    let Ok(count) = read else { break };
                    if count == 0 {
                        break;
                    }
                    lock(&self.typed).extend_from_slice(&buffer[..count]);
                    // The terminal echoes what is typed.
                    if !self.print(&mut tunnel, &buffer[..count]).await {
                        break;
                    }
                    line.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    while let Some(end) = line.find('\n') {
                        let command: String = line.drain(..=end).collect();
                        if !self.run(command.trim(), &mut tunnel).await {
                            return;
                        }
                    }
                }
            }
        }
        // The tunnel went; the shell did not.
        let mut attachment = lock(&self.attachment);
        if attachment
            .as_ref()
            .is_some_and(mpsc::UnboundedSender::is_closed)
        {
            *attachment = None;
        }
    }

    /// Runs a typed line. False where the shell ended.
    async fn run(&self, command: &str, tunnel: &mut Tunnel) -> bool {
        if let Some(code) = command.strip_prefix("exit ") {
            let code: i64 = code.trim().parse().unwrap_or(0);
            *lock(&self.exit) = Some(code);
            *lock(&self.attachment) = None;
            tunnel.exited(code);
            tunnel.close();
            return false;
        }
        if let Some(words) = command.strip_prefix("echo ") {
            return self.print(tunnel, format!("{words}\r\n").as_bytes()).await;
        }
        if let Some(count) = command.strip_prefix("flood ") {
            let count: usize = count.trim().parse().unwrap_or(0);
            return self.print(tunnel, &vec![b'x'; count]).await
                && self.print(tunnel, b"\r\nflooded\r\n").await;
        }
        true
    }
}

/// The machine's shells and what it was asked.
#[derive(Default)]
pub struct Fake {
    shells: Mutex<HashMap<Id, Arc<FakeShell>>>,
    pub opened: Mutex<Vec<PtyOpen>>,
    pub closed: Mutex<Vec<Id>>,
    /// What `pty.close` says was kept of the output of the next shell it closes.
    pub kept: Mutex<Option<BlobRef>>,
    /// The failure the next `pty.open` ends with.
    pub refuse: Mutex<Option<CommandError>>,
    /// The agent terminals that were opened: thread, and whether typing was let through.
    pub agent_opened: Mutex<Vec<(Id, bool)>>,
}

impl Fake {
    /// The shell `id`, if the machine has it.
    pub fn shell(&self, id: Id) -> Option<Arc<FakeShell>> {
        lock(&self.shells).get(&id).cloned()
    }

    fn open(&self, open: &PtyOpen) -> Result<CommandResult, CommandError> {
        if let Some(error) = lock(&self.refuse).take() {
            return Err(error);
        }
        lock(&self.opened).push(open.clone());
        lock(&self.shells).insert(
            open.terminal_id,
            FakeShell::new(open.terminal_id, (open.cols, open.rows)),
        );
        Ok(CommandResult::PtyOpen(PtyOpenResult {
            service: Service::Pty {
                terminal_id: open.terminal_id,
            }
            .to_string(),
            shell: "/bin/fakesh".to_owned(),
        }))
    }

    fn close(&self, shell: Id) -> Result<CommandResult, CommandError> {
        lock(&self.closed).push(shell);
        let Some(held) = lock(&self.shells).remove(&shell) else {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                format!("there is no shell {shell}"),
            ));
        };
        if let Some(attached) = lock(&held.attachment).take() {
            let _ = attached.send(Signal::Closed);
        }
        let exit_code = *lock(&held.exit);
        Ok(CommandResult::PtyClose(PtyCloseResult {
            exit_code,
            bytes_out: held.printed.load(Ordering::SeqCst),
            kept: lock(&self.kept).take(),
        }))
    }

    /// The machine loses every shell, as a connector that was restarted does.
    pub fn forget_shells(&self) {
        lock(&self.shells).clear();
    }

    async fn tunnels(self: Arc<Self>, mut incoming: IncomingTunnels) {
        while let Some(tunnel) = incoming.next().await {
            let fake = Arc::clone(&self);
            tokio::spawn(async move { fake.accept(tunnel).await });
        }
    }

    async fn accept(&self, incoming: IncomingTunnel) {
        let id = match incoming.service.parse::<Service>() {
            Ok(Service::Pty { terminal_id }) => terminal_id,
            Ok(Service::AgentPty { thread_id }) => thread_id,
            _ => {
                incoming.refuse(OpenErrorCode::NoSuchService, "no such service here");
                return;
            }
        };
        let Some(shell) = self.shell(id) else {
            incoming.refuse(OpenErrorCode::NoSuchService, "there is no such shell");
            return;
        };
        if let Ok(tunnel) = incoming.accept() {
            shell.serve(tunnel).await;
        }
    }

    /// Plays a connector: answers the commands about shells and keeps the shells behind the tunnels.
    fn plays(self: &Arc<Self>, machine: &ScriptedMachine) {
        let fake = Arc::clone(self);
        machine
            .handler
            .on(CommandKind::PtyStatus, move |command| match command {
                Command::PtyStatus(status) => Ok(CommandResult::PtyStatus(
                    orch_link_contract::commands::PtyStatusResult {
                        terminals: status
                            .terminal_ids
                            .iter()
                            .filter_map(|id| fake.shell(*id))
                            .filter(|shell| lock(&shell.exit).is_none())
                            .map(|shell| orch_link_contract::commands::PtyStatusEntry {
                                terminal_id: shell.id,
                                busy: shell.busy.load(Ordering::SeqCst),
                            })
                            .collect(),
                    },
                )),
                other => Err(CommandError::new(
                    ErrorCode::Internal,
                    other.kind().to_string(),
                )),
            });
        let fake = Arc::clone(self);
        machine
            .handler
            .on(CommandKind::PtyOpen, move |command| match command {
                Command::PtyOpen(open) => fake.open(open),
                other => Err(CommandError::new(
                    ErrorCode::Internal,
                    other.kind().to_string(),
                )),
            });
        let fake = Arc::clone(self);
        machine
            .handler
            .on(CommandKind::PtyClose, move |command| match command {
                Command::PtyClose(close) => fake.close(close.terminal_id),
                other => Err(CommandError::new(
                    ErrorCode::Internal,
                    other.kind().to_string(),
                )),
            });
        let fake = Arc::clone(self);
        machine
            .handler
            .on(CommandKind::AgentPtyOpen, move |command| match command {
                Command::AgentPtyOpen(open) => {
                    lock(&fake.agent_opened).push((open.thread_id, open.control));
                    lock(&fake.shells)
                        .entry(open.thread_id)
                        .or_insert_with(|| FakeShell::new(open.thread_id, (120, 32)));
                    Ok(CommandResult::AgentPtyOpen(AgentPtyOpenResult {
                        service: Service::AgentPty {
                            thread_id: open.thread_id,
                        }
                        .to_string(),
                    }))
                }
                other => Err(CommandError::new(
                    ErrorCode::Internal,
                    other.kind().to_string(),
                )),
            });
        tokio::spawn(Arc::clone(self).tunnels(machine.link.incoming_tunnels()));
    }
}

// ---------------------------------------------------------------- the floor

/// The scene, the machine's shells, and a server on a socket when a test asks for one.
pub struct Floor {
    pub scene: Scene,
    pub fake: Arc<Fake>,
    pub store: &'static Arc<Store>,
    pub thread: Id,
    pub environment: Id,
    served: Mutex<Option<SocketAddr>>,
}

impl Floor {
    /// A scene whose environment lets shells be opened, and whose machine keeps them.
    pub async fn new() -> Self {
        Self::tuned(|_| {}).await
    }

    /// The same, with the times of the realtime server set by `tune`: how long a browser may say nothing
    /// and how often a connection looks at its own clocks.
    pub async fn tuned(tune: impl FnOnce(&mut Tuning)) -> Self {
        let mut scene = Scene::new().await;
        let old = scene.rig.world.app.app.clone();
        let mut tuning = Tuning::from_config(&old.config);
        tune(&mut tuning);
        let app: App = Arc::new(AppState {
            host_ports: old.host_ports.clone(),
            builtin_sources: old.builtin_sources.clone(),
            db: old.db.clone(),
            config: Arc::clone(&old.config),
            clock: Arc::clone(&old.clock),
            link: old.link.clone(),
            engine: old.engine.clone(),
            realtime: Realtime::tuned(&old.db, &old.clock, tuning),
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs: orch_api_core::blobs::Blobs::in_memory(),
            cache: orch_api_core::cache::Cache::detached(),
        });
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        scene.rig.world.app = TestApp::over(
            Arc::clone(&app),
            scene.rig.world.app.clock.clone(),
            registry,
        );
        let thread = scene.thread().await.base.id;
        let environment = scene.on.environment;
        sqlx::query("UPDATE environments SET terminal = true WHERE id = $1")
            .bind(environment)
            .execute(scene.rig.db.pool())
            .await
            .unwrap();
        let app = scene.rig.world.app.app.clone();
        scene
            .rig
            .hub
            .set_ticket_verifier(app.link.tickets().verifier());
        app.realtime.terminals().through(scene.rig.hub.terminals());
        let fake = Arc::new(Fake::default());
        fake.plays(&scene.rig.machine);
        Self {
            scene,
            fake,
            store: Store::shared(),
            thread,
            environment,
            served: Mutex::new(None),
        }
    }

    /// Names `who` in the row of `permission` on the page of the thread, and nowhere else: they hold that
    /// and what they hold otherwise. None of the doors is on a thread's page.
    pub async fn names_on_the_thread(&self, who: &Who, permission: &str) {
        let mut conn = self.pool().acquire().await.unwrap();
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("node", self.thread, "live"),
            "any",
            permission,
            ("user", Some(who.id()), None),
            None,
            self.scene.rig.world.now(),
        )
        .await
        .unwrap();
    }

    /// Names `who` in the row of `permission` on the page of the environment, and nowhere else: they hold
    /// that and what they hold otherwise. The doors are on the environment's page.
    pub async fn names_on_the_environment(&self, who: &Who, permission: &str) {
        let mut conn = self.pool().acquire().await.unwrap();
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("environment", self.environment, "live"),
            "any",
            permission,
            ("user", Some(who.id()), None),
            None,
            self.scene.rig.world.now(),
        )
        .await
        .unwrap();
    }

    pub fn app(&self) -> orch_api_core::state::App {
        self.scene.rig.world.app.app.clone()
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        self.scene.rig.db.pool()
    }

    // ------------------------------------------------------------ requests

    /// `who` opens a shell through the thread.
    pub async fn open(&self, who: &Who) -> Reply<EnvironmentTerminalCreateReply> {
        self.open_with(who, Some(self.thread), json!({ "cols": 100, "rows": 30 }))
            .await
    }

    /// `who` opens a shell through `thread` (none: from the environment's own page) with this body.
    pub async fn open_with(
        &self,
        who: &Who,
        thread: Option<Id>,
        body: Value,
    ) -> Reply<EnvironmentTerminalCreateReply> {
        let mut call = self
            .scene
            .rig
            .world
            .app
            .call(EnvironmentTerminalCreate, &[&self.environment])
            .token(&who.token)
            .raw_body(&body);
        if let Some(thread) = thread {
            call = call.raw_query(&format!("thread={thread}"));
        }
        call.send().await
    }

    /// A shell opened through the thread, which has to succeed.
    pub async fn shell_of(&self, who: &Who) -> EnvironmentTerminalCreateReply {
        let reply = self.open(who).await;
        reply.ok()
    }

    /// The shells `who` is shown in the environment, through the thread.
    pub async fn list(&self, who: &Who, state: Option<&str>) -> Vec<TerminalSession> {
        let mut query = format!("thread={}", self.thread);
        if let Some(state) = state {
            query.push_str(&format!("&state={state}"));
        }
        self.scene
            .rig
            .world
            .app
            .call(EnvironmentTerminalList, &[&self.environment])
            .token(&who.token)
            .raw_query(&query)
            .send()
            .await
            .ok()
            .items
    }

    /// The shells `who` is shown from the environment's own page, with no thread named.
    pub async fn list_from_the_page(&self, who: &Who) -> Vec<TerminalSession> {
        self.scene
            .rig
            .world
            .app
            .call(EnvironmentTerminalList, &[&self.environment])
            .token(&who.token)
            .query(&EnvironmentTerminalListQuery::default())
            .send()
            .await
            .ok()
            .items
    }

    pub async fn ticket(
        &self,
        who: &Who,
        shell: Id,
    ) -> Reply<orch_api_contract::terminals::TerminalTicket> {
        self.scene
            .rig
            .world
            .app
            .call(TerminalTicketCreate, &[&shell])
            .token(&who.token)
            .send()
            .await
    }

    pub async fn close(&self, who: &Who, shell: Id) -> Reply<TerminalSession> {
        self.scene
            .rig
            .world
            .app
            .call(TerminalClose, &[&shell])
            .token(&who.token)
            .send()
            .await
    }

    // ------------------------------------------------------------ the database, as the tests read it

    /// The row of a shell, as stored.
    pub async fn row(&self, shell: Id) -> orch_db::queries::terminals::SessionRow {
        let mut conn = self.pool().acquire().await.unwrap();
        orch_db::queries::terminals::get(&mut conn, shell)
            .await
            .unwrap()
            .unwrap()
    }

    /// Waits until the row of a shell is as `holds` says.
    pub async fn wait_for(
        &self,
        shell: Id,
        what: &str,
        holds: impl Fn(&orch_db::queries::terminals::SessionRow) -> bool,
    ) {
        let holds = &holds;
        let this = self;
        eventually(what, || async move { holds(&this.row(shell).await) }).await;
    }

    /// The history lines of the environment about shells, oldest first: (action, summary).
    pub async fn history(&self) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT action, summary FROM audit_log
              WHERE environment_id = $1 AND history AND action LIKE 'terminal.%' ORDER BY at, id",
        )
        .bind(self.environment)
        .fetch_all(self.pool())
        .await
        .unwrap()
    }

    /// The details of the history lines of one action.
    pub async fn details(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar(
            "SELECT details FROM audit_log WHERE environment_id = $1 AND action = $2 ORDER BY at, id",
        )
        .bind(self.environment)
        .bind(action)
        .fetch_all(self.pool())
        .await
        .unwrap()
    }

    // ------------------------------------------------------------ the server

    /// Starts the realtime server and serves the application on a loopback socket.
    pub async fn serve(&self) -> SocketAddr {
        if let Some(addr) = *lock(&self.served) {
            return addr;
        }
        let app = self.app();
        app.realtime.start(self.scene.rig.engine.clone());
        app.realtime.listening().await;
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let router = orch_api_core::server::router(app, registry);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ =
                orch_api_core::server::serve_plain(listener, router, std::future::pending()).await;
        });
        *lock(&self.served) = Some(addr);
        addr
    }

    /// A screen on a shell with this ticket.
    pub async fn screen(&self, shell: Id, ticket: &str) -> Screen {
        let addr = self.serve().await;
        Screen::connect(
            addr,
            &format!("/api/v1/ws/terminals/{shell}?ticket={ticket}"),
        )
        .await
    }

    /// A screen on the agent's own terminal of the thread with this ticket.
    pub async fn agent_screen(&self, ticket: &str) -> Screen {
        let addr = self.serve().await;
        Screen::connect(
            addr,
            &format!("/api/v1/ws/agent-terminals/{}?ticket={ticket}", self.thread),
        )
        .await
    }

    /// Stops what the floor started.
    pub async fn stop(&self) {
        self.app().realtime.stop().await;
    }
}

// ---------------------------------------------------------------- a screen

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What a screen heard.
#[derive(Debug)]
pub enum Heard {
    Text(Value),
    Bytes(Vec<u8>),
    Closed(Option<u16>),
}

/// A browser's end of a terminal socket.
pub struct Screen {
    socket: Socket,
    /// Everything the shell printed that this screen was sent, in order.
    pub shown: Vec<u8>,
    /// The text frames it was sent, in order.
    pub frames: Vec<Value>,
}

impl Screen {
    pub async fn connect(addr: SocketAddr, path: &str) -> Self {
        Self::connect_offering(addr, path, &["orch.terminal.v1"]).await
    }

    pub async fn connect_offering(addr: SocketAddr, path: &str, protocols: &[&str]) -> Self {
        let mut request = format!("ws://{addr}{path}").into_client_request().unwrap();
        if !protocols.is_empty() {
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_str(&protocols.join(", ")).unwrap(),
            );
        }
        let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        Self {
            socket,
            shown: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// Connects with little room in the kernel for what the server writes, so that a screen that does
    /// not read holds up the line after a few kilobytes, as a slow link does.
    pub async fn connect_cramped(addr: SocketAddr, path: &str, receive_buffer: u32) -> Self {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(receive_buffer).unwrap();
        let stream = socket.connect(addr).await.unwrap();
        let mut request = format!("ws://{addr}{path}").into_client_request().unwrap();
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("orch.terminal.v1"),
        );
        let (socket, _) = tokio_tungstenite::client_async(request, MaybeTlsStream::Plain(stream))
            .await
            .unwrap();
        Self {
            socket,
            shown: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// What comes next, within `wait`. Bytes and frames are also kept.
    pub async fn hear_within(&mut self, wait: Duration) -> Option<Heard> {
        let heard = tokio::time::timeout(wait, async {
            loop {
                match self.socket.next().await {
                    None | Some(Err(_)) => return Heard::Closed(None),
                    Some(Ok(Message::Text(text))) => {
                        let frame: Value = serde_json::from_str(text.as_str()).unwrap();
                        self.frames.push(frame.clone());
                        return Heard::Text(frame);
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        self.shown.extend_from_slice(&bytes);
                        return Heard::Bytes(bytes.to_vec());
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

    /// The next text frame, which has to come; bytes before it are kept.
    pub async fn frame(&mut self) -> Value {
        loop {
            match self.hear_within(PATIENCE).await {
                Some(Heard::Text(frame)) => return frame,
                Some(Heard::Bytes(_)) => {}
                other => panic!("a frame was expected, and {other:?} came"),
            }
        }
    }

    /// The `ready` frame, and with it the replay it announces: the screen has the shell as it was.
    pub async fn ready(&mut self) -> Value {
        let ready = self.frame().await;
        assert_eq!(ready["op"], "ready", "{ready}");
        let replay = usize::try_from(ready["replay_bytes"].as_i64().unwrap()).unwrap();
        while self.shown.len() < replay {
            match self.hear_within(PATIENCE).await {
                Some(Heard::Bytes(_)) => {}
                other => panic!("the replay of {replay} bytes was expected, and {other:?} came"),
            }
        }
        ready
    }

    /// Types.
    pub async fn type_(&mut self, text: &str) {
        self.socket
            .send(Message::Binary(text.as_bytes().to_vec().into()))
            .await
            .unwrap();
    }

    /// Sends a text frame.
    pub async fn send(&mut self, frame: Value) {
        self.socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    /// Reads until what the shell printed to this screen holds `wanted`.
    pub async fn until(&mut self, wanted: &str) -> String {
        loop {
            let text = String::from_utf8_lossy(&self.shown).into_owned();
            if text.contains(wanted) {
                return text;
            }
            match self.hear_within(PATIENCE).await {
                Some(Heard::Closed(code)) => {
                    panic!(
                        "the socket closed ({code:?}) before {wanted:?} came; the screen showed {text:?}"
                    )
                }
                None => panic!("{wanted:?} never came; the screen showed {text:?}"),
                Some(_) => {}
            }
        }
    }

    /// Reads to the end: the frames that came, and how the socket was closed.
    pub async fn until_the_end(&mut self) -> Option<u16> {
        loop {
            match self.hear_within(PATIENCE).await {
                Some(Heard::Closed(code)) => return code,
                None => panic!("the socket never closed"),
                Some(_) => {}
            }
        }
    }

    /// The ops of the text frames heard so far.
    pub fn ops(&self) -> Vec<String> {
        self.frames
            .iter()
            .map(|frame| frame["op"].as_str().unwrap_or("?").to_owned())
            .collect()
    }

    /// The `closed` frame's reason, if one was heard.
    pub fn closed_reason(&self) -> Option<String> {
        self.frames
            .iter()
            .find(|frame| frame["op"] == "closed")
            .and_then(|frame| frame["reason"].as_str().map(str::to_owned))
    }

    /// Drops the socket without a word, as a closed tab does.
    pub async fn drop_it(mut self) {
        let _ = self.socket.close(None).await;
    }
}

/// A session is open, or not.
pub fn is_open(session: &TerminalSession) -> bool {
    session.state == TerminalState::Open
}
