//! Gateway authentication, machine presence and command/event routing.
//!
//! Gateways dial in, one connection each, and say which machines they hold.
//! [`Link`] is what the rest of the API sees of them: it asks whether a
//! session may start, keeps for each machine one [`ControllerLink`] built
//! from the cursors in the database, writes presence, takes a machine's
//! events in and sends its commands out.
//!
//! **Events.** A machine's events are applied in one transaction together
//! with the move of the stream's cursor, and only after that commits is the
//! machine told, so a stream that is cut at any point is taken in once and in
//! order. What an event changes is for whoever registers an [`EventApplier`];
//! this module itself keeps only what a machine says about itself.
//!
//! **Commands.** Commands are rows ([`crate::dispatch`]); here they are put
//! on the wire whenever the machine has a session, with the references in
//! them replaced by a value sealed to the machine or by a ticket signed then.
//!
//! **Sessions.** A gateway holds the session and tells the API about it; a
//! gateway that goes away takes its sessions with it, and the machines are
//! offline until a gateway says again that it holds them.

use crate::certificates::{CertificateService, CertificateSettings};
use crate::dispatch::{Dispatcher, SecretSource, StoredSecrets};
use crate::tickets::Tickets;
use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use orch_config::ServerConfig;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::link as queries;
use orch_db::queries::resources;
use orch_db::secrets::MasterKey;
use orch_db::{Conn, Db, DbError};
use orch_link::commands::{CommandHandle, Progress};
use orch_link::controller::{ControllerLink, Notice};
use orch_link::streams::Delivery;
use orch_link::tunnels::terminal::{ApiOut, TerminalHub};
use orch_link_contract::backplane::{
    ApiHello, AuthQuery, AuthReply, BackplaneData, BackplaneFrame, Down, DownFrame, Kick,
    MachineDown, MachineUp, STATS_EVERY_SECS, Stats, UpFrame,
};
use orch_link_contract::commands::{Cmd, CmdRes, SerialKey, SessionResync};
use orch_link_contract::events::{Event, EventKind};
use orch_link_contract::frames::{self, Text};
use orch_link_contract::session::Role;
use orch_link_contract::streams::{Eph, ResetReason, Stream};
use orch_link_contract::version::{ByeCode, ConnectorVersions, Limits, MAX_CHANNELS, RejectCode};
use orch_pki::PkiError;
use orch_pki::ca::CertificateAuthority;
use orch_types::{Clock, Id, Json, Seq, Time};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::{OnceCell, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// How the oldest connector the system accepts is named when nothing says
/// otherwise: the first release, which spoke this protocol.
pub const FIRST_CONNECTOR_VERSION: &str = "0.1.0";
/// The share of a new measurement of a machine's rate in the average the
/// system keeps. A quarter: a rate is noisy, and one slow report is not a
/// slow link.
pub const RATE_WEIGHT: f64 = 0.25;
/// The most bytes of one frame of the backplane. A frame of a machine is at
/// most a session's frame limit, and the wrapper around it is a few hundred
/// bytes; this is the bound the link's own socket has for a message.
pub const MAX_BACKPLANE_FRAME_BYTES: usize = 4 * 1024 * 1024;
// JSON framing adds a type name, request reference and separators around a local reply.
// Reserve more than that fixed envelope before accepting a complete canonical DTO.
const PORT_LOCAL_ENVELOPE_BYTES: usize = 128;
/// How long a gateway has to say who it is after it connected.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the system looks at the clocks of its commands, and at which
/// machines have lost their gateway.
const TICK_EVERY: Duration = Duration::from_secs(1);
/// How many ticks pass between two looks for commands that no machine took
/// in time, on machines whose link is not loaded.
const SWEEP_EVERY_TICKS: u32 = 10;
/// How long a connected machine's last-seen time may stand before it is written again: once a
/// minute keeps it within a minute and a quarter of the truth (with the gateway's figures every
/// fifteen seconds) for one write a minute per machine, not one per figure.
const SEEN_EVERY_SECS: i64 = 60;
/// How often the system pings a gateway, in the absence of anything else the
/// gateway says: its own figures come at this pace.
const PING_EVERY: Duration = Duration::from_secs(STATS_EVERY_SECS as u64);
/// How many of these silences end a gateway's connection.
const DEAD_AFTER_PINGS: u32 = 3;
/// How many measurements wait for a subscriber before the slow ones are
/// told they missed some.
const REPORT_BACKLOG: usize = 1024;
/// How many deliveries of one machine are applied at once, each in a transaction of its own and each of
/// another stream: eight times what one at a time takes in, and a quarter of the API's pooled connections
/// as shipped (32), so that one busy machine leaves the rest of the pool to people and to other machines.
const DELIVERIES_AT_ONCE: usize = 8;
/// How many frames of one machine are taken in at once, each of another stream: the same share of the
/// pool as its deliveries, for the frames whose source is checked against the database.
const FRAMES_AT_ONCE: usize = 8;
/// Command results keep their own ordered persistence task without delaying the gateway's reader.
const COMMAND_RESULTS_QUEUE: &str = "command_results";
/// How long a stream of a machine keeps its task with nothing to take in: longer than the pauses of a
/// session that works, short enough that the tasks of sessions that ended go.
const STREAM_IDLE: Duration = Duration::from_secs(30);
/// How long a session stream whose source was checked is taken from its machine without asking the
/// database again: a minute, against a binding that changes when a thread moves to another machine. The
/// check here only keeps another machine's frames out of the stream's order; whatever a delivery does is
/// checked again in its own transaction (`apply_delivery`), so a frame let in on an older word is refused
/// there and counted.
const STREAM_TRUST: Duration = Duration::from_secs(60);
/// How many trusted streams a machine's link keeps before it forgets those whose word is old: far more
/// sessions than one machine runs at once.
const TRUSTED_STREAMS_KEPT: usize = 4096;
/// How many frames of one stream are remembered as come and not taken in: more than a stream's window
/// lets a machine have on the wire in frames of a few events; past it the oldest are forgotten, and
/// only what is said of how long frames waited is the poorer for it.
const ARRIVALS_KEPT: usize = 1024;

/// Registration waits for initial facts instead of treating an empty report as a local policy refusal.
pub(crate) fn require_facts(facts: &Json) -> crate::middleware::requests::ApiResult<()> {
    if facts.get("adapters").and_then(Json::as_array).is_none() {
        return Err(crate::middleware::requests::ApiError::new(
            orch_api_contract::common::ErrorCode::SourceOffline,
            "the machine is waiting to report its initial facts",
        )
        .with("reason", "machine_facts_pending"));
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------- applying events

/// Why events could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The database failed.
    #[error("{0}")]
    Db(#[from] DbError),
    /// The effects could not be applied for another reason; the events are
    /// sent again.
    #[error("{0}")]
    Other(String),
}

/// What an event changes. Called for each delivery inside the transaction in
/// which the stream's cursor is moved, so that the effects and the cursor
/// happen together or not at all; whatever it returns as an error rolls the
/// transaction back and the machine sends the events again. It must be safe
/// to run twice on the same events, as the rules of the link allow.
#[async_trait]
pub trait EventApplier: Send + Sync {
    /// Learns installed harness descriptors from committed facts, outside the event transaction.
    async fn discover(&self, _machine: Id, _retry: bool) -> Result<(), ApplyError> {
        Ok(())
    }

    /// Starts follow-up work only after the delivery and its effects have committed.
    fn committed(&self, _machine: Id, _delivery: &Delivery) {}

    /// A committed physical update fence ended, successfully or with a refusal/failure.
    fn component_update_finished(&self, _machine: Id) {}

    /// Applies the events of one delivery, in their order.
    async fn apply(
        &self,
        tx: &mut Conn,
        machine: Id,
        delivery: &Delivery,
    ) -> Result<(), ApplyError>;

    /// The machine said that events of a stream are gone; the cursor was
    /// set in the same transaction.
    async fn reset(
        &self,
        _tx: &mut Conn,
        _machine: Id,
        _stream: &Stream,
        _reason: ResetReason,
    ) -> Result<(), ApplyError> {
        Ok(())
    }
}

/// The applier that records nothing but the cursor: what stands until the
/// thread engine and the others register theirs.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecordsCursorsOnly;

#[async_trait]
impl EventApplier for RecordsCursorsOnly {
    async fn apply(&self, _: &mut Conn, _: Id, _: &Delivery) -> Result<(), ApplyError> {
        Ok(())
    }
}

/// A measurement that a machine sent and the system does not keep.
#[derive(Debug, Clone)]
pub struct Report {
    /// The machine.
    pub machine_id: Id,
    /// The measurement.
    pub eph: Eph,
}

// ---------------------------------------------------------------- settings

/// What the link states to gateways and machines.
#[derive(Debug, Clone)]
pub struct LinkSettings {
    /// The limits every session states.
    pub limits: Limits,
    /// The connector builds the system accepts and prefers.
    pub versions: ConnectorVersions,
}

impl LinkSettings {
    /// The settings a deployment's configuration gives, for a release of
    /// `release_version`: that is the build the system would have every
    /// machine run.
    #[must_use]
    pub fn from_config(config: &ServerConfig, release_version: &str) -> Self {
        let limits = Limits {
            max_frame_bytes: u32::try_from(config.link.max_frame_bytes)
                .unwrap_or(Limits::default().max_frame_bytes),
            tunnel_window_bytes: u32::try_from(config.link.tunnel_window_bytes)
                .unwrap_or(Limits::default().tunnel_window_bytes),
            max_channels: MAX_CHANNELS,
            ports: orch_link::ports::limits_of(&config.link),
        };
        Self {
            limits,
            versions: ConnectorVersions {
                min: FIRST_CONNECTOR_VERSION.to_owned(),
                recommended: release_version.to_owned(),
            },
        }
    }
}

// ---------------------------------------------------------------- gateway connections

/// What is sent to a gateway.
#[derive(Debug)]
enum Outbound {
    Frame(Box<BackplaneFrame>),
    Data(Bytes),
    Datagram {
        machine: Id,
        packet: orch_link::ports::Datagram,
    },
}

/// One connection of a gateway to the system.
pub(crate) struct GatewayConn {
    generation: u64,
    out: mpsc::UnboundedSender<Outbound>,
    closed: CancellationToken,
    stats: Mutex<Option<Stats>>,
    /// When the gateway's process started, as its hello said; when it connected, where it did not.
    started_at: orch_types::Time,
}

impl std::fmt::Debug for GatewayConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayConn")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl GatewayConn {
    fn send_datagram(&self, machine: Id, packet: orch_link::ports::Datagram) -> bool {
        self.out
            .send(Outbound::Datagram { machine, packet })
            .is_ok()
    }
    fn send(&self, frame: BackplaneFrame) -> bool {
        self.out.send(Outbound::Frame(Box::new(frame))).is_ok()
    }

    fn send_data(&self, data: &BackplaneData) -> bool {
        match data.encode() {
            Ok(bytes) => self.out.send(Outbound::Data(bytes)).is_ok(),
            Err(_) => false,
        }
    }
}

/// Where a machine's session is: the gateway that holds it.
#[derive(Clone)]
struct SessionRoute {
    gateway: Arc<GatewayConn>,
    session_id: Id,
    fingerprint: Option<String>,
    ip: Option<IpAddr>,
}

/// How far a command's state has been written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mirrored {
    Queued,
    Sent,
    Accepted,
    Over,
}

impl Mirrored {
    fn of(progress: &Progress) -> Self {
        match progress {
            Progress::Queued => Self::Queued,
            Progress::Sent => Self::Sent,
            Progress::Accepted => Self::Accepted,
            Progress::Succeeded(_)
            | Progress::Failed(_)
            | Progress::Expired
            | Progress::Cancelled => Self::Over,
        }
    }
}

struct Tracked {
    handle: CommandHandle,
    mirrored: Mirrored,
}

#[derive(Default)]
struct EntryState {
    route: Option<SessionRoute>,
    pending_fingerprint: Option<String>,
    tracked: HashMap<Id, Tracked>,
    /// Explicit request-owned observations have no durable queue row or replay body.
    observations: HashSet<Id>,
    discovering: bool,
    received_through: HashMap<String, i64>,
    discarded_through: HashMap<String, i64>,
    /// Commands put on the wire whose rows do not yet say so: the machine's mirror writes them.
    handed_over: Vec<Id>,
    /// The session streams whose source was last found to be this machine, and when ([`STREAM_TRUST`]).
    trusted: HashMap<String, std::time::Instant>,
    /// When the frames of each stream not taken in yet came, by the last number each carries, so that a
    /// delivery says how long its frames waited here ([`ARRIVALS_KEPT`] a stream at most; C-turns).
    arrived: HashMap<String, std::collections::VecDeque<(i64, std::time::Instant)>>,
}

/// A frame of a machine on its way to the machine's own task, with the gateway and the session it came by.
type UpItem = (Arc<GatewayConn>, Id, UpFrame);

/// The system's end of one machine: its link, where its session is, and the
/// commands it has not finished with.
pub(crate) struct MachineEntry {
    pub(crate) id: Id,
    pub(crate) controller: ControllerLink,
    state: Mutex<EntryState>,
    retired: CancellationToken,
    /// The tasks of the machine's streams, each taking its stream's frames in the order they came
    /// ([`Link::stream_loop`]): their sources are checked against the database there, never in the reader
    /// of a gateway, which serves every machine behind it, and no stream waits for another's.
    streams: Mutex<HashMap<String, mpsc::UnboundedSender<UpItem>>>,
    /// How many of the machine's frames are taken in at once ([`FRAMES_AT_ONCE`]).
    intake: Arc<tokio::sync::Semaphore>,
    /// Asks the machine's mirror ([`Link::mirror_loop`]) to write into the rows what its commands went
    /// through: handed over, acknowledged, answered.
    mirror: tokio::sync::Notify,
    /// How many of the machine's deliveries are applied at once ([`DELIVERIES_AT_ONCE`]).
    deliveries: Arc<tokio::sync::Semaphore>,
}

struct Observation<'a> {
    entry: &'a MachineEntry,
    id: Id,
}

impl Drop for Observation<'_> {
    fn drop(&mut self) {
        lock(&self.entry.state).observations.remove(&self.id);
    }
}

impl MachineEntry {
    fn observation(&self) -> Observation<'_> {
        let id = orch_types::new_id();
        lock(&self.state).observations.insert(id);
        Observation { entry: self, id }
    }

    pub(crate) fn owns_observation(&self, id: Id) -> bool {
        lock(&self.state).observations.contains(&id)
    }

    fn route(&self) -> Option<SessionRoute> {
        lock(&self.state).route.clone()
    }

    /// Whether the machine has a session now.
    pub(crate) fn is_online(&self) -> bool {
        lock(&self.state).route.is_some()
    }

    /// Takes a command into the link and follows it.
    pub(crate) fn adopt(&self, cmd: Cmd, progress: Progress) {
        let id = cmd.id;
        let mut state = lock(&self.state);
        if state.tracked.contains_key(&id) {
            return;
        }
        let handle = self.controller.commands().adopt(cmd, progress);
        let mirrored = Mirrored::of(&handle.progress());
        state.tracked.insert(id, Tracked { handle, mirrored });
    }

    pub(crate) fn reconcile_terminal(&self, id: Id, terminal: Progress) {
        self.controller.commands().reconcile_terminal(id, terminal);
        self.mirror.notify_one();
    }
}

/// Why a machine has no link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EntryError {
    /// The system does not know the machine, or it was deleted.
    #[error("the system does not know that machine")]
    Unknown,
    /// The machine was revoked.
    #[error("the machine was revoked")]
    Revoked,
    /// The database could not be asked.
    #[error("the database could not be asked")]
    Database,
}

impl From<DbError> for EntryError {
    fn from(error: DbError) -> Self {
        tracing::error!(%error, "a machine could not be looked up");
        Self::Database
    }
}

// ---------------------------------------------------------------- the link

/// A pending port report's machine, connection generation and reply channel.
type PortQuery = (
    Id,
    u64,
    Option<Id>,
    tokio::sync::oneshot::Sender<orch_api_contract::ports::PortConnectionList>,
);

pub(crate) struct LinkInner {
    port_queries: Mutex<HashMap<Id, PortQuery>>,
    pub(crate) db: Db,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) settings: LinkSettings,
    store_metrics: bool,
    machine_requests: Mutex<Option<Arc<dyn crate::machine_requests::Handler>>>,
    machine_request_slots: Arc<tokio::sync::Semaphore>,
    port_local_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) certificates: CertificateService,
    pub(crate) tickets: Tickets,
    pub(crate) secrets: Mutex<Arc<dyn SecretSource>>,
    applier: Mutex<Arc<dyn EventApplier>>,
    machines: Mutex<HashMap<Id, Arc<OnceCell<Arc<MachineEntry>>>>>,
    gateways: Mutex<HashMap<String, Arc<GatewayConn>>>,
    generations: AtomicU64,
    terminals: TerminalHub,
    terminal_out: Mutex<Option<mpsc::UnboundedReceiver<ApiOut>>>,
    terminal_channels: Mutex<HashMap<Id, HashSet<u32>>>,
    pub(crate) finished: tokio::sync::Notify,
    /// The commands somebody waits for ([`crate::dispatch::Dispatcher::wait`]), each told by the mirror
    /// that writes its end: one ended command wakes its own waiters, not every waiter's read of its row.
    ended: Mutex<HashMap<Id, Arc<tokio::sync::Notify>>>,
    reports: broadcast::Sender<Report>,
    refusals: report_refusals::Refusals,
    pub(crate) shutdown: CancellationToken,
    started: Mutex<bool>,
}

/// What the rest of the API sees of the machines and the gateways: commands,
/// presence, certificates, tickets and terminals.
#[derive(Clone)]
pub struct Link {
    pub(crate) inner: Arc<LinkInner>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").finish_non_exhaustive()
    }
}

/// What a link is made from.
pub struct LinkParts {
    /// The database.
    pub db: Db,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// The deployment's configuration.
    pub config: Arc<ServerConfig>,
    /// The release the system is: the build every machine would run.
    pub release_version: String,
    /// The certificate authority.
    pub authority: Arc<CertificateAuthority>,
    /// The key that stored secrets are sealed under.
    pub master_key: MasterKey,
}

impl Link {
    pub(crate) fn serve_machine_requests(
        &self,
        handler: Arc<dyn crate::machine_requests::Handler>,
    ) {
        *lock(&self.inner.machine_requests) = Some(handler);
    }

    /// A link over `parts`. Nothing runs until [`Link::start`].
    #[must_use]
    pub fn new(parts: LinkParts) -> Self {
        let LinkParts {
            db,
            clock,
            config,
            release_version,
            authority,
            master_key,
        } = parts;
        let (terminal_tx, terminal_rx) = mpsc::unbounded_channel();
        let (reports, _) = broadcast::channel(REPORT_BACKLOG);
        let certificates = CertificateService::new(
            db.clone(),
            Arc::clone(&clock),
            authority,
            CertificateSettings::from_config(&config),
        );
        let secrets: Arc<dyn SecretSource> = Arc::new(StoredSecrets::new(db.clone(), master_key));
        Self {
            inner: Arc::new(LinkInner {
                port_queries: Mutex::new(HashMap::new()),
                tickets: Tickets::generate(Arc::clone(&clock)),
                settings: LinkSettings::from_config(&config, &release_version),
                store_metrics: config.metrics.store,
                machine_requests: Mutex::new(None),
                machine_request_slots: Arc::new(tokio::sync::Semaphore::new(
                    orch_link_contract::mcp_import::MAX_PENDING,
                )),
                port_local_slots: Arc::new(tokio::sync::Semaphore::new(
                    orch_link_contract::local_ports::LOCAL_CONCURRENT,
                )),
                db,
                clock,
                certificates,
                secrets: Mutex::new(secrets),
                applier: Mutex::new(Arc::new(RecordsCursorsOnly)),
                machines: Mutex::new(HashMap::new()),
                gateways: Mutex::new(HashMap::new()),
                generations: AtomicU64::new(1),
                terminals: TerminalHub::new(terminal_tx)
                    .with_port_limits(orch_link::ports::limits_of(&config.link)),
                terminal_out: Mutex::new(Some(terminal_rx)),
                terminal_channels: Mutex::new(HashMap::new()),
                finished: tokio::sync::Notify::new(),
                ended: Mutex::new(HashMap::new()),
                reports,
                refusals: report_refusals::Refusals::default(),
                shutdown: CancellationToken::new(),
                started: Mutex::new(false),
            }),
        }
    }

    /// A link with an authority and a master key of its own, made here, and
    /// no gateway: for tests of the rest of the API, and for a system that is
    /// run without its machines.
    pub fn detached(
        db: Db,
        clock: Arc<dyn Clock>,
        config: Arc<ServerConfig>,
    ) -> Result<Self, PkiError> {
        let authority = Arc::new(CertificateAuthority::create(
            &config.system.name,
            config.pki.ca_validity_days,
            clock.as_ref(),
        )?);
        let mut key = [0u8; 32];
        rand::fill(&mut key[..]);
        Ok(Self::new(LinkParts {
            db,
            clock,
            config,
            release_version: FIRST_CONNECTOR_VERSION.to_owned(),
            authority,
            master_key: MasterKey::new(key),
        }))
    }

    /// Starts what runs beside requests: the clocks of the commands, the
    /// terminals' frames to their gateways. Presence is forgotten, because
    /// no gateway has yet said which sessions it holds. Called once.
    pub async fn start(&self) -> Result<(), DbError> {
        {
            let mut started = lock(&self.inner.started);
            if *started {
                return Ok(());
            }
            *started = true;
        }
        {
            let mut conn = self.inner.db.acquire().await?;
            resources::reset_presence(&mut conn).await?;
        }
        let link = self.clone();
        tokio::spawn(async move { link.tick_loop().await });
        if let Some(out) = lock(&self.inner.terminal_out).take() {
            let link = self.clone();
            tokio::spawn(async move { link.terminal_loop(out).await });
        }
        Ok(())
    }

    /// Ends everything that runs: gateways are let go, links are retired.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        for gateway in lock(&self.inner.gateways).values() {
            gateway.closed.cancel();
        }
        for cell in lock(&self.inner.machines).values() {
            if let Some(entry) = cell.get() {
                entry.retired.cancel();
            }
        }
    }

    /// The commands the rest of the API issues, and waits for.
    #[must_use]
    pub fn dispatcher(&self) -> Dispatcher {
        Dispatcher::new(self.clone())
    }

    pub(crate) fn component_update_finished(&self, machine: Id) {
        let applier = Arc::clone(&lock(&self.inner.applier));
        applier.component_update_finished(machine);
    }

    /// The certificate authority and what is made with it.
    #[must_use]
    pub fn certificates(&self) -> &CertificateService {
        &self.inner.certificates
    }

    /// The tickets that open tunnels.
    #[must_use]
    pub fn tickets(&self) -> &Tickets {
        &self.inner.tickets
    }

    /// The terminals the system opens to machines.
    #[must_use]
    pub fn terminals(&self) -> &TerminalHub {
        &self.inner.terminals
    }

    /// Registers what carries out the events of machines.
    pub fn set_applier(&self, applier: Arc<dyn EventApplier>) {
        *lock(&self.inner.applier) = applier;
    }

    /// Registers where the values of stored secrets come from.
    pub fn set_secret_source(&self, secrets: Arc<dyn SecretSource>) {
        *lock(&self.inner.secrets) = secrets;
    }

    /// The measurements machines send, as they come. A subscriber that is
    /// too slow is told how many it missed.
    #[must_use]
    pub fn reports(&self) -> broadcast::Receiver<Report> {
        self.inner.reports.subscribe()
    }

    /// Reads a native session through the live command route without a database result or replay.
    pub async fn session_native_actions(
        &self,
        machine: Id,
        body: &orch_link_contract::commands::SessionNativeActions,
    ) -> Result<orch_api_contract::native::NativeCatalog, EntryError> {
        const WAIT: Duration = Duration::from_secs(30);
        let entry = self.entry(machine).await?;
        if entry.route().is_none() {
            return Err(EntryError::Unknown);
        }
        let deadline = Time::new(self.inner.clock.now() + chrono::Duration::seconds(30));
        let observation = entry.observation();
        let mut handle = entry
            .controller
            .commands()
            .issue_typed(
                observation.id,
                &SerialKey::Thread(body.thread_id),
                deadline,
                body,
            )
            .map_err(|_| EntryError::Database)?;
        let id = handle.id();
        let result = tokio::time::timeout(
            WAIT,
            handle.wait_for::<orch_link_contract::commands::SessionNativeActions>(),
        )
        .await;
        if result.is_err() {
            entry.controller.commands().cancel(id);
        }
        entry.controller.commands().forget_ended();
        result
            .map_err(|_| EntryError::Database)?
            .map_err(|_| EntryError::Database)
    }

    /// Reads a native session through the live command route without a database result or replay.
    pub async fn session_background(
        &self,
        machine: Id,
        body: &orch_link_contract::commands::SessionBackground,
    ) -> Result<orch_link_contract::background::NativeBackground, EntryError> {
        const WAIT: Duration = Duration::from_secs(15);
        let entry = self.entry(machine).await?;
        if entry.route().is_none() {
            return Err(EntryError::Unknown);
        }
        let deadline = Time::new(self.inner.clock.now() + chrono::Duration::seconds(15));
        let observation = entry.observation();
        let mut handle = entry
            .controller
            .commands()
            .issue_typed(
                observation.id,
                &SerialKey::Thread(body.thread_id),
                deadline,
                body,
            )
            .map_err(|_| EntryError::Database)?;
        let id = handle.id();
        let result = tokio::time::timeout(
            WAIT,
            handle.wait_for::<orch_link_contract::commands::SessionBackground>(),
        )
        .await;
        if result.is_err() {
            entry.controller.commands().cancel(id);
        }
        entry.controller.commands().forget_ended();
        result
            .map_err(|_| EntryError::Database)?
            .map_err(|_| EntryError::Database)
    }

    /// Has the harness of an open session count what fills its context, through the live command route
    /// without a database result or replay: the engine keeps what it answers itself.
    pub async fn session_context(
        &self,
        machine: Id,
        body: &orch_link_contract::commands::SessionContext,
    ) -> Result<orch_link_contract::commands::SessionContextResult, EntryError> {
        // The harness asks the vendor's counter about each part: a little longer than a read of its roster.
        const WAIT: Duration = Duration::from_secs(25);
        let entry = self.entry(machine).await?;
        if entry.route().is_none() {
            return Err(EntryError::Unknown);
        }
        let deadline = Time::new(self.inner.clock.now() + chrono::Duration::seconds(25));
        let observation = entry.observation();
        let mut handle = entry
            .controller
            .commands()
            .issue_typed(
                observation.id,
                &SerialKey::Thread(body.thread_id),
                deadline,
                body,
            )
            .map_err(|_| EntryError::Database)?;
        let id = handle.id();
        let result = tokio::time::timeout(
            WAIT,
            handle.wait_for::<orch_link_contract::commands::SessionContext>(),
        )
        .await;
        if result.is_err() {
            entry.controller.commands().cancel(id);
        }
        entry.controller.commands().forget_ended();
        result
            .map_err(|_| EntryError::Database)?
            .map_err(|_| EntryError::Database)
    }

    /// Reads a bounded current gateway registry over the live backplane, never command storage.
    async fn carrier_port_connections(
        &self,
        machine: Id,
        environment: Id,
        generation: i64,
        query: orch_api_contract::ports::EnvironmentPortConnectionListQuery,
        node: bool,
    ) -> Result<orch_api_contract::ports::PortConnectionList, EntryError> {
        let route = self
            .entry(machine)
            .await?
            .route()
            .ok_or(EntryError::Unknown)?;
        let request_ref = orch_types::new_id();
        let (answer, waiting) = tokio::sync::oneshot::channel();
        {
            let mut pending = lock(&self.inner.port_queries);
            if pending
                .values()
                .filter(|(env, _, _, _)| *env == environment)
                .count()
                >= orch_link::ports::TCP_WAITING
            {
                return Err(EntryError::Database);
            }
            pending.insert(
                request_ref,
                (
                    environment,
                    route.gateway.generation,
                    node.then_some(machine),
                    answer,
                ),
            );
        }
        let _registration = PortQueryRegistration {
            link: self,
            reference: request_ref,
        };
        let request = orch_link_contract::ports::PortConnectionsQuery {
            request_ref,
            environment_id: environment,
            generation,
            query,
        };
        if node {
            route.gateway.send(BackplaneFrame::Down(Down {
                machine_id: machine,
                frame: DownFrame::PortConnectionsQuery(request),
            }));
        } else {
            route
                .gateway
                .send(BackplaneFrame::PortConnectionsQuery(request));
        }
        const LIVE_QUERY_DEADLINE: Duration = Duration::from_secs(15);
        let result = tokio::time::timeout(LIVE_QUERY_DEADLINE, waiting).await;
        lock(&self.inner.port_queries).remove(&request_ref);
        result
            .map_err(|_| EntryError::Database)?
            .map_err(|_| EntryError::Database)
    }

    /// Whether a machine has a session now.
    #[must_use]
    pub fn is_online(&self, machine: Id) -> bool {
        lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned())
            .is_some_and(|entry| entry.is_online())
    }

    /// Whether events a machine with a session sent on `stream` are here and not taken in yet (C-turns):
    /// checked and handed on, but not committed, whether a transaction has them, they wait for one or
    /// they wait behind one. What the engine waits to hear from a harness (that a turn began) may be
    /// among them: under load a hundred sessions' events were taken in a minute and more after they
    /// came, and turns their harness had begun at once were given up as never begun.
    #[must_use]
    pub fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        let Some(entry) = self.loaded_entry(machine) else {
            return false;
        };
        if !entry.is_online() {
            return false;
        }
        let name = stream.to_string();
        let Some(received) = lock(&entry.state).received_through.get(&name).copied() else {
            return false;
        };
        received > entry.controller.cursors().get(&name).copied().unwrap_or(0)
    }

    /// The session a machine has now, if it has one.
    #[must_use]
    pub fn session_of(&self, machine: Id) -> Option<Id> {
        self.loaded_entry(machine)
            .and_then(|entry| entry.route())
            .map(|route| route.session_id)
    }

    /// The machines that have a session now.
    #[must_use]
    pub fn online_machines(&self) -> Vec<Id> {
        let cells: Vec<_> = lock(&self.inner.machines).values().cloned().collect();
        cells
            .into_iter()
            .filter_map(|cell| cell.get().cloned())
            .filter(|entry| entry.is_online())
            .map(|entry| entry.id)
            .collect()
    }

    /// How many gateways are connected.
    #[must_use]
    pub fn gateways_connected(&self) -> usize {
        lock(&self.inner.gateways).len()
    }

    /// The figures a gateway last reported, by its name.
    #[must_use]
    pub fn gateway_stats(&self, gateway_id: &str) -> Option<Stats> {
        lock(&self.inner.gateways)
            .get(gateway_id)
            .and_then(|gateway| lock(&gateway.stats).clone())
    }

    // ------------------------------------------------------------ machines

    /// The link of a machine, made from the database if it is not there.
    ///
    /// Boxed, because making a link starts tasks that may ask for another
    /// link (a reset asks a machine for a command), and the type of such a
    /// loop cannot be written without a box in it.
    pub(crate) fn entry(&self, id: Id) -> BoxFuture<'_, Result<Arc<MachineEntry>, EntryError>> {
        Box::pin(async move {
            let cell = lock(&self.inner.machines).entry(id).or_default().clone();
            let made = cell.get_or_try_init(|| self.create_entry(id)).await;
            match made {
                Ok(entry) => Ok(Arc::clone(entry)),
                Err(error) => {
                    lock(&self.inner.machines).remove(&id);
                    Err(error)
                }
            }
        })
    }

    async fn create_entry(&self, id: Id) -> Result<Arc<MachineEntry>, EntryError> {
        let (row, cursors, open) = {
            let mut conn = self.inner.db.acquire().await?;
            let row = resources::machine(&mut conn, id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(EntryError::Unknown)?;
            if !row.is_active() {
                return Err(EntryError::Revoked);
            }
            let cursors = queries::cursors(&mut conn, id).await?;
            let open = queries::open_commands(&mut conn, id).await?;
            (row, cursors, open)
        };
        #[cfg(test)]
        command_admission_tests::entry_loaded(id).await;
        let roles = roles_of(&row.roles);
        let (controller, outputs) =
            ControllerLink::new(id, roles, cursors, Arc::clone(&self.inner.clock));
        let entry = Arc::new(MachineEntry {
            id,
            controller,
            state: Mutex::new(EntryState::default()),
            retired: CancellationToken::new(),
            streams: Mutex::new(HashMap::new()),
            intake: Arc::new(tokio::sync::Semaphore::new(FRAMES_AT_ONCE)),
            mirror: tokio::sync::Notify::new(),
            deliveries: Arc::new(tokio::sync::Semaphore::new(DELIVERIES_AT_ONCE)),
        });
        for row in open {
            let progress = if row.state == "accepted" {
                Progress::Accepted
            } else {
                Progress::Queued
            };
            entry.adopt(crate::dispatch::cmd_of(&row), progress);
        }
        let link = self.clone();
        let for_notices = Arc::clone(&entry);
        tokio::spawn(async move { link.notice_loop(for_notices, outputs.notices).await });
        let link = self.clone();
        let for_down = Arc::clone(&entry);
        tokio::spawn(async move { link.down_loop(for_down, outputs.down).await });
        let link = self.clone();
        let for_mirror = Arc::clone(&entry);
        tokio::spawn(async move { link.mirror_loop(for_mirror).await });
        Ok(entry)
    }

    /// The link of a machine, if one is loaded.
    pub(crate) fn loaded_entry(&self, id: Id) -> Option<Arc<MachineEntry>> {
        lock(&self.inner.machines)
            .get(&id)
            .and_then(|cell| cell.get().cloned())
    }

    /// Replaces the link of a machine by one made anew from the database:
    /// after its roles changed, or when what the link holds and the database
    /// disagree.
    pub(crate) async fn rebuild(&self, id: Id) {
        let old = lock(&self.inner.machines).remove(&id);
        if let Some(entry) = old.and_then(|cell| cell.get().cloned()) {
            entry.retired.cancel();
        }
    }

    /// Revokes a machine: its row says so, what was asked of it is
    /// cancelled, and a session it holds is ended at once.
    pub async fn revoke_machine(
        &self,
        machine: Id,
        by: Actor,
        origin: &Origin,
    ) -> Result<bool, DbError> {
        let changed = self.inner.certificates.revoke(machine, by, origin).await?;
        self.let_go(machine).await;
        Ok(changed)
    }

    /// Lets a machine whose revocation was committed go: a session it holds is ended at once, and
    /// what was asked of it is cancelled.
    pub async fn let_go(&self, machine: Id) {
        let entry = lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned());
        if let Some(entry) = entry {
            if let Some(route) = entry.route() {
                route.gateway.send(BackplaneFrame::Kick(Kick {
                    machine_id: machine,
                    code: ByeCode::Revoked,
                }));
            }
            let open: Vec<Id> = lock(&entry.state).tracked.keys().copied().collect();
            for id in open {
                entry.controller.commands().cancel(id);
            }
            self.sync_commands(&entry).await;
            if let Some(route) = entry.route() {
                entry.controller.session_down(route.session_id);
            }
            lock(&entry.state).route = None;
            self.rebuild(machine).await;
        }
    }

    // ------------------------------------------------------------ authorizing a session

    /// Whether a machine may have a session, as a gateway asks.
    async fn authorize(&self, query: &AuthQuery) -> AuthReply {
        let refused = |code: RejectCode| AuthReply {
            endpoints: None,
            update_permit: None,
            n: query.n,
            ok: false,
            code: Some(code),
            cursors: None,
            placement: None,
        };
        if self.inner.shutdown.is_cancelled() {
            return refused(RejectCode::Busy);
        }
        match self.try_authorize(query).await {
            Ok((cursors, placement, update_permit)) => {
                let needs_update = query.roles.contains(&Role::Node)
                    && matches!(
                        self.inner
                            .settings
                            .versions
                            .standing(&query.connector_version),
                        Ok(orch_link_contract::version::Standing::TooOld)
                    );
                let endpoints = match self.connection_addresses(query, !needs_update).await {
                    Ok(endpoints) => endpoints,
                    Err(error) => {
                        tracing::warn!(%error, "the machine's address list could not be read");
                        return refused(RejectCode::Busy);
                    }
                };
                AuthReply {
                    endpoints: Some(endpoints),
                    update_permit,
                    n: query.n,
                    ok: !needs_update,
                    code: needs_update.then_some(RejectCode::UpgradeRequired),
                    cursors: (!needs_update).then_some(cursors),
                    placement: Some(placement),
                }
            }
            Err(code) => refused(code),
        }
    }

    async fn connection_addresses(
        &self,
        query: &AuthQuery,
        keep: bool,
    ) -> Result<Vec<orch_link_contract::enrollment::Endpoint>, DbError> {
        use orch_db::queries::settings::{self, Stored};
        let mut conn = self.inner.db.acquire().await?;
        let system = self.inner.certificates.advertised_endpoints();
        if keep && query.roles.contains(&Role::Node) {
            let effective = query
                .hosted_endpoints
                .as_ref()
                .or(query.endpoints.as_ref())
                .unwrap_or(&system);
            let value = serde_json::to_value(effective)
                .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
            settings::put(
                &mut conn,
                None,
                &format!("machine_endpoints:{}", query.machine_id),
                &Stored {
                    value: &value,
                    locked: false,
                    by: None,
                    at: self.inner.clock.now(),
                },
                None,
            )
            .await?;
        }
        crate::certificates::endpoints_for_machine(&mut conn, query.machine_id, &system).await
    }

    async fn try_authorize(
        &self,
        query: &AuthQuery,
    ) -> Result<
        (
            BTreeMap<String, Seq>,
            String,
            Option<orch_link_contract::session::UpdatePermit>,
        ),
        RejectCode,
    > {
        let busy = |error: DbError| {
            tracing::error!(%error, "a session could not be authorized");
            RejectCode::Busy
        };
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.begin().await.map_err(busy)?;
        // Turn admission takes this fence before any FK can pin a machine. Keep the same
        // order before the certificate row lock, including on a new-version reconnect.
        orch_db::queries::component_updates::lock(&mut conn, query.machine_id)
            .await
            .map_err(busy)?;
        let row = resources::reenrollment::lock(&mut conn, query.machine_id)
            .await
            .map_err(busy)?
            .filter(|row| row.deleted_at.is_none());
        let Some(row) = row else {
            conn.rollback().await.map_err(DbError::from).map_err(busy)?;
            return Err(RejectCode::UnknownMachine);
        };
        if !row.is_active() {
            conn.rollback().await.map_err(DbError::from).map_err(busy)?;
            return Err(RejectCode::Revoked);
        }
        let current = hex::encode(&row.cert_fingerprint);
        if query.fingerprint != current {
            let renewed = row
                .link()
                .next
                .is_some_and(|next| next.fingerprint == query.fingerprint);
            let promoted = renewed
                && resources::promote_next_certificate(&mut conn, row.id, &query.fingerprint, now)
                    .await
                    .map_err(busy)?;
            if !promoted {
                // Finish our own rollback before replying. Drop only queues it, which can leave
                // this rejected certificate's version fence busy for the next valid session.
                conn.rollback().await.map_err(DbError::from).map_err(busy)?;
                return Err(RejectCode::Revoked);
            }
        }
        let roles: Vec<String> = query.roles.iter().map(role_name).collect();
        let changed_roles = roles != row.roles;
        let minimum_node = row.roles.iter().any(|role| role == "node")
            && query.roles.contains(&Role::Node)
            && matches!(
                self.inner
                    .settings
                    .versions
                    .standing(&query.connector_version),
                Ok(orch_link_contract::version::Standing::TooOld)
            );
        resources::record_session(&mut conn, row.id, &query.connector_version, &roles, now)
            .await
            .map_err(busy)?;
        let update = orch_db::queries::component_updates::among(&mut conn, &[row.id])
            .await
            .map_err(busy)?
            .pop();
        let finished_update = if update.as_ref().is_some_and(|update| {
            !crate::component_updates::newer(&update.recommended_version, &query.connector_version)
        }) {
            orch_db::queries::component_updates::finish(&mut conn, row.id, "current", None, now)
                .await
                .map_err(busy)?
        } else {
            if !minimum_node && update.is_some_and(|update| update.state == "updating") {
                // The old process may have stopped after acknowledging but before installation.
                // Preserve its drain fence and give this session a fresh journal identity.
                orch_db::queries::component_updates::reconnected(&mut conn, row.id, now)
                    .await
                    .map_err(busy)?;
            }
            false
        };
        let update_permit = if minimum_node {
            let recommended = &self.inner.settings.versions.recommended;
            self.request_automatic_component_update(
                &mut conn,
                row.id,
                &query.connector_version,
                recommended,
            )
            .await
            .map_err(busy)?;
            if !orch_db::queries::component_updates::busy(&mut conn, row.id)
                .await
                .map_err(busy)?
            {
                orch_db::queries::component_updates::grant_minimum(&mut conn, row.id, now)
                    .await
                    .map_err(busy)?;
                Some(orch_link_contract::session::UpdatePermit {
                    recommended: recommended.clone(),
                })
            } else {
                None
            }
        } else {
            None
        };
        conn.commit().await.map_err(DbError::from).map_err(busy)?;
        if finished_update {
            self.component_update_finished(row.id);
        }
        if changed_roles {
            self.rebuild(row.id).await;
        }
        let entry = self.entry(row.id).await.map_err(|error| match error {
            EntryError::Unknown => RejectCode::UnknownMachine,
            EntryError::Revoked => RejectCode::Revoked,
            EntryError::Database => RejectCode::Busy,
        })?;
        lock(&entry.state).pending_fingerprint = Some(query.fingerprint.clone());
        Ok((entry.controller.cursors(), row.placement, update_permit))
    }

    // ------------------------------------------------------------ what a gateway says

    async fn machine_up(&self, gateway: &Arc<GatewayConn>, up: MachineUp) {
        self.attach(gateway, up.machine_id, up.session_id, Some(up.ip))
            .await;
    }

    /// Records that a gateway holds a session of a machine, and tells the
    /// machine's link. A machine the system does not accept is let go.
    async fn attach(
        &self,
        gateway: &Arc<GatewayConn>,
        machine: Id,
        session: Id,
        ip: Option<IpAddr>,
    ) {
        let entry = match self.entry(machine).await {
            Ok(entry) => entry,
            Err(EntryError::Database) => return,
            Err(EntryError::Unknown | EntryError::Revoked) => {
                gateway.send(BackplaneFrame::Kick(Kick {
                    machine_id: machine,
                    code: ByeCode::Revoked,
                }));
                return;
            }
        };
        {
            let mut state = lock(&entry.state);
            let fingerprint = state.pending_fingerprint.take();
            state.route = Some(SessionRoute {
                gateway: Arc::clone(gateway),
                session_id: session,
                fingerprint,
                ip,
            });
        }
        entry.controller.session_up(session);
    }

    async fn write_presence(&self, machine: Id, online: bool, ip: Option<IpAddr>) {
        let now = self.inner.clock.now();
        let result = async {
            let mut conn = self.inner.db.acquire().await?;
            resources::set_session_presence(
                &mut conn,
                machine,
                online,
                ip,
                self.session_of(machine),
                now,
            )
            .await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, %machine, "presence could not be written");
        } else if let Err(error) = crate::realtime::ephemeral::broadcast(
            &self.inner.db,
            &crate::realtime::ephemeral::Message::Machine {
                machine,
                session: if online {
                    self.session_of(machine)
                } else {
                    None
                },
            },
        )
        .await
        {
            tracing::debug!(%error, "transient watches could not be told of machine presence");
        }
    }

    async fn machine_down(&self, down: MachineDown) {
        let entry = lock(&self.inner.machines)
            .get(&down.machine_id)
            .and_then(|cell| cell.get().cloned());
        let Some(entry) = entry else { return };
        let ours = {
            let mut state = lock(&entry.state);
            let ours = state
                .route
                .as_ref()
                .is_some_and(|route| route.session_id == down.session_id);
            if ours {
                state.route = None;
            }
            ours
        };
        entry.controller.session_down(down.session_id);
        if ours {
            // The commands sent on that session wait again; their rows are written by the mirror.
            entry.mirror.notify_one();
            self.close_terminals_of(down.machine_id);
        }
    }

    /// Takes a frame a machine sent without waiting for the database (C-turns). An acknowledgement or a
    /// result of a command reaches the command's issuer at once, and the machine's mirror writes the row a
    /// moment later; every other frame goes, in the order it came, to the machine's own task, which checks
    /// its source against the database and takes it in there ([`Link::up`]). The reader of a gateway serves
    /// every machine behind it: under load, acknowledgements stood behind the checks of events long enough
    /// for their commands to be failed as unacknowledged, or expired by the sweep while their machine ran
    /// them, and every machine's frames waited for one pool.
    async fn route_up(&self, gateway: &Arc<GatewayConn>, machine: Id, session: Id, frame: UpFrame) {
        let mut entry = lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned());
        if entry.as_ref().is_none_or(|entry| !entry.is_online()) {
            // As in `up`: what comes on a session says that it exists as well as the word would have.
            self.attach(gateway, machine, session, None).await;
            entry = lock(&self.inner.machines)
                .get(&machine)
                .and_then(|cell| cell.get().cloned());
        }
        let Some(entry) = entry else { return };
        if entry.route().is_none_or(|route| {
            route.session_id != session || !Arc::ptr_eq(&route.gateway, gateway)
        }) {
            return;
        }
        match frame {
            UpFrame::CmdAck(_) => {
                entry.controller.up(session, frame);
                entry.mirror.notify_one();
            }
            UpFrame::CmdRes(result) => {
                entry
                    .controller
                    .up(session, UpFrame::CmdRes(result.clone()));
                entry.mirror.notify_one();
                // A cancelled issuer already has its first outcome. The authenticated native
                // reply must still persist its stop proof and any file that actually finished.
                self.to_stream(
                    &entry,
                    COMMAND_RESULTS_QUEUE.to_owned(),
                    (Arc::clone(gateway), session, UpFrame::CmdRes(result)),
                );
            }
            frame => match report_sources::frame_stream(&frame).map(|(name, _)| name.to_owned()) {
                // A stream's frames go to the stream's own task, in the order they came: the order of a
                // stream holds, and no stream waits for another's check (a hundred threads' sessions
                // stood in one line behind one machine's database checks, and their turns were given up
                // before their start was heard).
                Some(stream) => {
                    if let Some((_, last)) = report_sources::frame_range(&frame) {
                        Self::note_arrival(&entry, &stream, last);
                    }
                    self.to_stream(&entry, stream, (Arc::clone(gateway), session, frame));
                }
                // The rest touch no database here (the requests a machine makes are served on tasks of
                // their own).
                None => self.up(gateway, machine, session, frame).await,
            },
        }
    }

    /// Notes when a frame of `stream` whose events go up to `last` came, unless it is one sent again
    /// ([`EntryState::arrived`]).
    fn note_arrival(entry: &MachineEntry, stream: &str, last: i64) {
        let mut state = lock(&entry.state);
        if state
            .received_through
            .get(stream)
            .is_some_and(|through| last <= *through)
        {
            return;
        }
        let arrived = state.arrived.entry(stream.to_owned()).or_default();
        if arrived.len() >= ARRIVALS_KEPT {
            arrived.pop_front();
        }
        arrived.push_back((last, std::time::Instant::now()));
    }

    /// How many frames of `stream` a delivery through `last` took in, and when the first and the last of
    /// them came ([`Link::note_arrival`]).
    fn arrivals_taken(
        entry: &MachineEntry,
        stream: &str,
        last: i64,
    ) -> (
        usize,
        Option<std::time::Instant>,
        Option<std::time::Instant>,
    ) {
        let mut state = lock(&entry.state);
        let Some(arrived) = state.arrived.get_mut(stream) else {
            return (0, None, None);
        };
        let (mut frames, mut first, mut latest) = (0, None, None);
        while arrived.front().is_some_and(|(through, _)| *through <= last) {
            if let Some((_, at)) = arrived.pop_front() {
                frames += 1;
                first.get_or_insert(at);
                latest = Some(at);
            }
        }
        if arrived.is_empty() {
            state.arrived.remove(stream);
        }
        (frames, first, latest)
    }

    /// Hands a frame to the task of its stream, and starts one where the stream has none.
    fn to_stream(&self, entry: &Arc<MachineEntry>, stream: String, item: UpItem) {
        let mut streams = lock(&entry.streams);
        let item = match streams.get(&stream) {
            Some(sender) => match sender.send(item) {
                Ok(()) => return,
                Err(mpsc::error::SendError(item)) => item,
            },
            None => item,
        };
        let (sender, frames) = mpsc::unbounded_channel();
        let _ = sender.send(item);
        streams.insert(stream.clone(), sender);
        let link = self.clone();
        let owner = Arc::downgrade(entry);
        let machine = entry.id;
        tokio::spawn(async move { link.stream_loop(owner, machine, stream, frames).await });
    }

    /// The task of one stream of a machine ([`Link::to_stream`]): its frames are taken in by [`Link::up`],
    /// one after the other in the order they came, at most [`FRAMES_AT_ONCE`] of the machine's at a
    /// time. A stream with nothing for [`STREAM_IDLE`] gives its task up; the table of streams is held
    /// while a frame is handed over, so none is lost between the two.
    async fn stream_loop(
        &self,
        owner: std::sync::Weak<MachineEntry>,
        machine: Id,
        stream: String,
        mut frames: mpsc::UnboundedReceiver<UpItem>,
    ) {
        loop {
            let next = tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                next = tokio::time::timeout(STREAM_IDLE, frames.recv()) => next,
            };
            let item = match next {
                Ok(Some(item)) => item,
                Ok(None) => return,
                Err(_idle) => {
                    let Some(entry) = owner.upgrade() else { return };
                    let mut streams = lock(&entry.streams);
                    match frames.try_recv() {
                        Ok(item) => item,
                        Err(_) => {
                            streams.remove(&stream);
                            return;
                        }
                    }
                }
            };
            let permit = match owner.upgrade() {
                Some(entry) => Arc::clone(&entry.intake).acquire_owned().await.ok(),
                None => None,
            };
            let (gateway, session, frame) = item;
            self.up(&gateway, machine, session, frame).await;
            drop(permit);
        }
    }

    /// The machine's mirror: writes what its commands went through into their rows whenever something
    /// happened to them ([`Link::sync_commands`]), one pass at a time, and once more when its link is let
    /// go, so that nothing a machine said is lost from the rows.
    async fn mirror_loop(&self, entry: Arc<MachineEntry>) {
        loop {
            tokio::select! {
                () = entry.retired.cancelled() => break,
                () = self.inner.shutdown.cancelled() => return,
                () = entry.mirror.notified() => {}
            }
            self.sync_commands(&entry).await;
        }
        self.sync_commands(&entry).await;
    }

    async fn up(&self, gateway: &Arc<GatewayConn>, machine: Id, session: Id, frame: UpFrame) {
        let mut entry = lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned());
        if entry.as_ref().is_none_or(|entry| !entry.is_online()) {
            // The word that this session exists did not get through: what
            // comes on it says so as well as that word would have.
            self.attach(gateway, machine, session, None).await;
            entry = lock(&self.inner.machines)
                .get(&machine)
                .and_then(|cell| cell.get().cloned());
        }
        let Some(entry) = entry else { return };
        if entry.route().is_none_or(|route| {
            route.session_id != session || !Arc::ptr_eq(&route.gateway, gateway)
        }) {
            return;
        }
        if let UpFrame::PortConnectionsReply(reply) = frame {
            let pending = {
                let mut pending = lock(&self.inner.port_queries);
                if pending
                    .get(&reply.request_ref)
                    .is_some_and(|(env, generation, owner, _)| {
                        *env == reply.environment_id
                            && *generation == gateway.generation
                            && *owner == Some(machine)
                    })
                {
                    pending.remove(&reply.request_ref)
                } else {
                    None
                }
            };
            if let Some((_, _, _, answer)) = pending {
                // A host node reports observed address callers only; it cannot supply a proved person/link.
                let valid = reply.connections.owner
                    == orch_api_contract::ports::PortConnectionOwner::Node
                    && reply.connections.connections.as_ref().is_some_and(|rows| {
                        rows.len() <= 200
                            && rows.iter().all(|row| {
                                row.environment_id == reply.environment_id
                                    && matches!(
                                        &row.outside.caller,
                                        orch_api_contract::ports::PortCaller::Address
                                    )
                            })
                    })
                    && reply.connections.totals.as_ref().is_some_and(|totals| {
                        totals.len()
                            <= orch_link::ports::share(
                                orch_api_contract::ports::PortConnectionOwner::Node,
                                self.inner.settings.limits.ports.caller_totals,
                            ) + 1
                            && totals.iter().all(|total| match &total.caller {
                                Some(orch_api_contract::ports::PortCaller::Address) => !total.other,
                                None => total.other && total.address.is_none(),
                                Some(_) => false,
                            })
                    });
                if valid {
                    let _ = answer.send(reply.connections);
                }
            }
            return;
        }
        if let UpFrame::PortLocalAsk(request) = frame {
            let handler = lock(&self.inner.machine_requests).clone();
            // Reserve room for the frame tag/correlation; a whole DTO that does not fit is
            // refused, never silently truncated or put into a retained command result.
            let reply_bytes = self
                .inner
                .settings
                .limits
                .frame_bytes()
                .saturating_sub(PORT_LOCAL_ENVELOPE_BYTES);
            let Ok(permit) = Arc::clone(&self.inner.port_local_slots).try_acquire_owned() else {
                gateway.send(BackplaneFrame::Down(Down {
                    machine_id: machine,
                    frame: DownFrame::PortLocalAnswer(
                        orch_link_contract::local_ports::PortLocalAnswer::unavailable(
                            request.request_id,
                            "the live read slots are full",
                        ),
                    ),
                }));
                return;
            };
            tokio::spawn(async move {
                let _permit = permit;
                let id = request.request_id;
                let answer = match handler {
                    Some(handler) => tokio::time::timeout(
                        std::time::Duration::from_secs(
                            orch_link_contract::local_ports::ANSWER_WITHIN_SECS,
                        ),
                        handler.local_ports(machine, request),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        orch_link_contract::local_ports::PortLocalAnswer::unavailable(
                            id,
                            "the live read timed out",
                        )
                    }),
                    None => orch_link_contract::local_ports::PortLocalAnswer::unavailable(
                        id,
                        "the system is not ready for inside port reads",
                    ),
                };
                let answer =
                    if serde_json::to_vec(&answer).is_ok_and(|bytes| bytes.len() <= reply_bytes) {
                        answer
                    } else {
                        orch_link_contract::local_ports::PortLocalAnswer::unavailable(
                            id,
                            "the whole live reply exceeds this link's frame bound",
                        )
                    };
                if let Some(route) = entry.route() {
                    route.gateway.send(BackplaneFrame::Down(Down {
                        machine_id: machine,
                        frame: DownFrame::PortLocalAnswer(answer),
                    }));
                }
            });
            return;
        }
        if let UpFrame::McpImportAsk(request) = frame {
            let handler = lock(&self.inner.machine_requests).clone();
            let permit = Arc::clone(&self.inner.machine_request_slots).try_acquire_owned();
            let Ok(permit) = permit else {
                gateway.send(BackplaneFrame::Down(Down {
                    machine_id: machine,
                    frame: DownFrame::McpImportAnswer(crate::machine_requests::unavailable(
                        request.request_id,
                        "the system is busy with local imports",
                    )),
                }));
                return;
            };
            // Copying uses the ordinary command path on this very link. Never wait for it in
            // the backplane reader, which must keep receiving that command's answer.
            tokio::spawn(async move {
                let _permit = permit;
                let id = request.request_id;
                let answer = match handler {
                    Some(handler) => match tokio::time::timeout(
                        std::time::Duration::from_secs(
                            orch_link_contract::mcp_import::ANSWER_WITHIN_SECS,
                        ),
                        handler.import(machine, request),
                    )
                    .await
                    {
                        Ok(answer) => answer,
                        Err(_) => crate::machine_requests::unavailable(
                            id,
                            "the import is unconfirmed; inspect Tools before retrying",
                        ),
                    },
                    None => crate::machine_requests::unavailable(
                        id,
                        "the system is not ready to file local imports",
                    ),
                };
                if let Some(route) = entry.route() {
                    route.gateway.send(BackplaneFrame::Down(Down {
                        machine_id: machine,
                        frame: DownFrame::McpImportAnswer(answer),
                    }));
                }
            });
            return;
        }
        if let Some((name, through)) = report_sources::frame_stream(&frame)
            && let Ok(stream) = name.parse::<Stream>()
        {
            // The events of a session stream whose source was checked lately go in without asking the
            // database again ([`STREAM_TRUST`]): each frame of a hundred sessions waited for a pooled
            // connection here, and the turns' starts were heard minutes late (C-turns).
            let trustable = matches!(stream, Stream::Session { .. })
                && matches!(frame, UpFrame::Ev(_) | UpFrame::Evb(_));
            let trusted = trustable
                && lock(&entry.state)
                    .trusted
                    .get(name)
                    .is_some_and(|at| at.elapsed() < STREAM_TRUST);
            let allowed = if trusted {
                Ok(true)
            } else {
                async {
                    report_sources::accepts_frame(
                        &mut *self.inner.db.acquire().await?,
                        machine,
                        &stream,
                        &frame,
                    )
                    .await
                }
                .await
            };
            match &allowed {
                Ok(true) if trustable && !trusted => {
                    let mut state = lock(&entry.state);
                    if state.trusted.len() >= TRUSTED_STREAMS_KEPT {
                        state.trusted.retain(|_, at| at.elapsed() < STREAM_TRUST);
                    }
                    state
                        .trusted
                        .insert(name.to_owned(), std::time::Instant::now());
                }
                Ok(false) => {
                    lock(&entry.state).trusted.remove(name);
                }
                _ => {}
            }
            match allowed {
                Ok(false) => {
                    // A node can still send its own work after discarding another host's report.
                    // Keep consecutive refusals in the link's acknowledgement order, with no DB
                    // cursor. Whole foreign streams and gaps are discarded before ordering.
                    if let Some((first, last)) = report_sources::frame_range(&frame) {
                        let known = entry.controller.cursors().get(name).copied().unwrap_or(0);
                        let next = lock(&entry.state)
                            .received_through
                            .get(name)
                            .copied()
                            .unwrap_or(known)
                            .saturating_add(1);
                        let owned = async {
                            let mut conn = self.inner.db.acquire().await?;
                            report_sources::owns(&mut conn, machine, &stream).await
                        }
                        .await;
                        if first <= next && matches!(owned, Ok(true)) {
                            lock(&entry.state)
                                .received_through
                                .insert(name.to_owned(), last.max(next - 1));
                            entry.controller.up(session, frame);
                            return;
                        }
                    }
                    // A foreign spool is discarded even after reconnect, when its numbers may be
                    // beyond the unchanged database cursor. It never enters stream ordering.
                    self.record_report_refusal(
                        machine,
                        &stream,
                        report_sources::frame_count(&frame),
                    )
                    .await;
                    if let Some(seq) = through {
                        gateway.send(BackplaneFrame::Down(Down {
                            machine_id: machine,
                            frame: DownFrame::Ack(orch_link_contract::streams::Ack {
                                stream: name.to_owned(),
                                seq,
                            }),
                        }));
                    }
                    return;
                }
                Ok(true) => {}
                Err(error) => {
                    tracing::warn!(%error, %machine, "a report's source could not be checked");
                    return;
                }
            }
            if let Some((first, last)) = report_sources::frame_range(&frame) {
                let known = entry.controller.cursors().get(name).copied().unwrap_or(0);
                let mut state = lock(&entry.state);
                let received = state
                    .received_through
                    .entry(name.to_owned())
                    .or_insert(known);
                if first <= received.saturating_add(1) {
                    *received = (*received).max(last);
                }
            }
        }
        let touches_commands = matches!(frame, UpFrame::CmdAck(_) | UpFrame::CmdRes(_));
        if let UpFrame::CmdRes(result) = &frame {
            let confirmed = async {
                let mut tx = self.inner.db.begin().await?;
                if let Ok(written) = &result.outcome {
                    orch_engine::jobs::workspace_upload::retain_native_result(
                        &mut tx,
                        entry.id,
                        result.id,
                        written,
                        self.inner.clock.now(),
                    )
                    .await?;
                }
                sqlx::query(
                    "UPDATE commands SET stop_confirmed_at=$3 WHERE id=$1 AND machine_id=$2
                    AND stop_confirmed_at IS NULL",
                )
                .bind(result.id)
                .bind(entry.id)
                .bind(self.inner.clock.now())
                .execute(&mut *tx)
                .await?;
                tx.commit().await.map_err(DbError::from)?;
                Ok::<_, DbError>(())
            }
            .await;
            if let Err(error) = confirmed {
                tracing::warn!(%error, "a machine stop acknowledgement could not be saved");
            }
        }
        entry.controller.up(session, frame);
        if touches_commands {
            self.sync_commands(&entry).await;
        }
    }

    // ------------------------------------------------------------ a machine's own task

    async fn notice_loop(
        &self,
        entry: Arc<MachineEntry>,
        mut notices: mpsc::UnboundedReceiver<Notice>,
    ) {
        loop {
            let notice = tokio::select! {
                () = entry.retired.cancelled() => return,
                () = self.inner.shutdown.cancelled() => return,
                notice = notices.recv() => notice,
            };
            let Some(notice) = notice else { return };
            match notice {
                Notice::Deliver(delivery) => {
                    // Deliveries of different streams are applied side by side, a few at a time: the link
                    // hands a stream's next delivery only once its last one committed or was given up, so
                    // every stream keeps its order (C-turns: every thread's events waited on this task for
                    // every other thread's transaction).
                    let Ok(permit) = Arc::clone(&entry.deliveries).acquire_owned().await else {
                        return;
                    };
                    let link = self.clone();
                    let for_delivery = Arc::clone(&entry);
                    tokio::spawn(async move {
                        let _permit = permit;
                        link.apply_delivery(for_delivery.id, &for_delivery.controller, delivery)
                            .await;
                    });
                }
                Notice::Reset(notice) => self.apply_reset(&entry, &notice).await,
                Notice::Report(eph) => {
                    let allowed = async {
                        let Ok(stream) = eph.stream.parse::<Stream>() else {
                            return Ok(false);
                        };
                        orch_db::queries::machine_reports::accepts(
                            &mut *self.inner.db.acquire().await?,
                            entry.id,
                            &stream,
                            &[(eph.kind.as_str(), &eph.body)],
                        )
                        .await
                    }
                    .await;
                    if !matches!(allowed, Ok::<_, DbError>(true)) {
                        if matches!(allowed, Ok(false))
                            && let Ok(stream) = eph.stream.parse::<Stream>()
                        {
                            self.record_report_refusal(entry.id, &stream, 1).await;
                        }
                        continue;
                    }
                    let report = Report {
                        machine_id: entry.id,
                        eph,
                    };
                    let _ = self.inner.reports.send(report.clone());
                    if let Err(error) =
                        crate::realtime::ephemeral::forward_report(&self.inner.db, &report).await
                    {
                        tracing::debug!(%error, "a transient report could not be forwarded");
                    }
                }
                Notice::Online { .. } => {
                    let ip = entry.route().and_then(|route| route.ip);
                    self.write_presence(entry.id, true, ip).await;
                }
                Notice::Offline { .. } => {
                    // Another session may have taken the place of the one that
                    // ended; the machine is offline only if none stands.
                    if !entry.is_online() {
                        self.write_presence(entry.id, false, None).await;
                    }
                }
            }
        }
    }

    /// Commits a controller's delivery through the event transaction before acknowledging it.
    pub(crate) async fn apply_delivery(
        &self,
        machine: Id,
        controller: &ControllerLink,
        delivery: Delivery,
    ) -> bool {
        enum Taken {
            Applied(Option<crate::realtime::raw_traffic::Boundary>),
            Refused,
            CursorMismatch,
        }
        let stream = delivery.stream.to_string();
        let Some(first) = delivery.events.first() else {
            controller.committed(&delivery);
            return true;
        };
        let expected = first.seq - 1;
        // How long the delivery waited for a pooled connection and held its transaction (C-turns).
        let entered = std::time::Instant::now();
        let mut begun = entered;
        // The development guard names a machine's delivery in what it reports.
        let result = orch_db::pool::serving("machine delivery", async {
            let mut tx = self.inner.db.begin().await?;
            begun = std::time::Instant::now();
            // Reports can change provider availability or discover owned resources: what they change
            // moves the authorization epochs at commit; nothing is locked for it here.
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            if !orch_db::queries::machine_reports::accepts(
                &mut tx,
                machine,
                &delivery.stream,
                &delivery
                    .events
                    .iter()
                    .map(|event| (event.kind.as_str(), &event.body))
                    .collect::<Vec<_>>(),
            )
            .await?
            {
                return Ok(Taken::Refused);
            }
            if delivery
                .events
                .iter()
                .any(|event| event.kind == EventKind::ProviderFound.as_str())
            {
                // Discovery may create owned resources and their access pages: an authority change,
                // whose epochs move at commit.
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            }
            let raw_boundary = crate::realtime::raw_traffic::before(&mut tx, &delivery).await?;
            let now = self.inner.clock.now();
            self.apply_machine_facts(&mut tx, machine, &delivery, now)
                .await?;
            self.apply_component_updates(&mut tx, machine, &delivery)
                .await?;
            self.apply_found_providers(&mut tx, machine, &delivery, now)
                .await?;
            let applier = Arc::clone(&lock(&self.inner.applier));
            applier.apply(&mut tx, machine, &delivery).await?;
            let skipped = self.loaded_entry(machine).is_some_and(|entry| {
                lock(&entry.state)
                    .discarded_through
                    .get(&stream)
                    .is_some_and(|last| *last >= expected)
            });
            let expected = if skipped {
                let persisted = queries::cursors(&mut tx, machine)
                    .await?
                    .get(&stream)
                    .copied()
                    .unwrap_or(0);
                if persisted > expected {
                    return Ok(Taken::CursorMismatch);
                }
                persisted
            } else {
                expected
            };
            if !queries::advance_cursor(&mut tx, machine, &stream, expected, delivery.last_seq, now)
                .await?
            {
                return Ok(Taken::CursorMismatch);
            }
            tx.commit().await.map_err(DbError::from)?;
            applier.committed(machine, &delivery);
            if delivery.stream == Stream::Machine
                && delivery
                    .events
                    .iter()
                    .any(|event| event.kind == EventKind::MachineUpdateFailed.as_str())
            {
                applier.component_update_finished(machine);
            }
            Ok::<_, ApplyError>(Taken::Applied(raw_boundary))
        })
        .await;
        let ended = std::time::Instant::now();
        match result {
            Ok(Taken::Applied(boundary)) => {
                let kind = delivery.stream.family().prefix();
                metrics::counter!("orch_link_events_total","stream_kind"=>kind)
                    .increment(delivery.events.len() as u64);
                for event in &delivery.events {
                    let lag = (self.inner.clock.now() - event.at.get())
                        .num_milliseconds()
                        .max(0) as f64
                        / 1000.0;
                    metrics::histogram!("orch_link_event_lag_seconds","stream_kind"=>kind)
                        .record(lag);
                }
                // From the frames' arrival here to their commit, and where that time went (C-turns: the
                // engine, and a person, see a turn begin only when its `turn.started` is committed).
                let (frames, first_came, last_came) =
                    self.loaded_entry(machine).map_or((0, None, None), |entry| {
                        Self::arrivals_taken(&entry, &stream, delivery.last_seq)
                    });
                let millis = |from: std::time::Instant, to: std::time::Instant| {
                    u64::try_from(to.saturating_duration_since(from).as_millis())
                        .unwrap_or(u64::MAX)
                };
                if let Some(came) = first_came {
                    metrics::histogram!("orch_link_delivery_wait_seconds","stream_kind"=>kind)
                        .record(ended.saturating_duration_since(came).as_secs_f64());
                }
                metrics::histogram!("orch_link_delivery_transaction_seconds","stream_kind"=>kind)
                    .record(ended.saturating_duration_since(begun).as_secs_f64());
                tracing::debug!(
                    target: "orch_link_intake",
                    stream_kind = kind,
                    events = delivery.events.len(),
                    frames,
                    first_came_ms = first_came.map(|came| millis(came, ended)),
                    last_came_ms = last_came.map(|came| millis(came, ended)),
                    before_apply_ms = first_came.map(|came| millis(came, entered)),
                    connection_ms = millis(entered, begun),
                    transaction_ms = millis(begun, ended),
                    "a delivery was committed"
                );
                if let Some(entry) = self.loaded_entry(machine) {
                    lock(&entry.state).discarded_through.remove(&stream);
                }
                controller.committed(&delivery);
                if delivery.stream == Stream::Machine
                    && let Some(entry) = self.loaded_entry(machine)
                {
                    self.discover(&entry, true);
                }
                if let Err(error) =
                    crate::realtime::raw_traffic::after(&self.inner.db, boundary, &delivery).await
                {
                    tracing::debug!(%error, "committed diagnostic or sign-in events could not be forwarded");
                }
                true
            }
            Ok(Taken::Refused) => {
                if let Some(entry) = self.loaded_entry(machine) {
                    lock(&entry.state)
                        .discarded_through
                        .insert(stream.clone(), delivery.last_seq);
                }
                self.record_report_refusal(machine, &delivery.stream, delivery.events.len())
                    .await;
                controller.committed(&delivery);
                true
            }
            Ok(Taken::CursorMismatch) => {
                tracing::error!(
                    %machine, stream,
                    "the cursor in the database is not where these events continue; the machine's link is made anew"
                );
                controller.abandoned(&delivery);
                self.rebuild(machine).await;
                false
            }
            Err(error) => {
                tracing::warn!(%error, %machine, stream, "events could not be applied; they will be sent again");
                controller.abandoned(&delivery);
                false
            }
        }
    }

    fn discover(&self, entry: &Arc<MachineEntry>, retry: bool) {
        {
            let mut state = lock(&entry.state);
            if state.discovering || state.route.is_none() {
                return;
            }
            state.discovering = true;
        }
        let entry = Arc::clone(entry);
        let applier = Arc::clone(&lock(&self.inner.applier));
        let shutdown = self.inner.shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => {}
                result = applier.discover(entry.id, retry) => {
                    if let Err(error) = result { tracing::warn!(machine = %entry.id, %error, "startup harness discovery failed"); }
                }
            }
            lock(&entry.state).discovering = false;
        });
    }

    async fn apply_found_providers(
        &self,
        conn: &mut Conn,
        machine: Id,
        delivery: &Delivery,
        now: DateTime<Utc>,
    ) -> Result<(), ApplyError> {
        if delivery.stream != Stream::Machine {
            return Ok(());
        }
        use orch_db::queries::{
            binding_start, resources::binding as registrations, resources::routes as rows,
        };
        for event in &delivery.events {
            let Ok(Event::ProviderFound(found)) = Event::decode(&event.kind, event.body.clone())
            else {
                continue;
            };
            let Some((owner, home)) = binding_start::discovery_owner(conn, machine).await? else {
                continue;
            };
            let place = resources::thread_engine::machine_place(conn, machine).await?;
            if place.as_ref().is_none_or(|place| {
                !place.roles.iter().any(|role| role == "provider")
                    || !place.roles.iter().any(|role| role == "agent")
            }) {
                continue;
            }
            if let Some((provider, _)) =
                binding_start::found_subscription(conn, machine, &found.harness_kind).await?
            {
                if binding_start::set_identity(conn, provider, found.identity.as_deref(), now)
                    .await?
                {
                    orch_db::sequences::notify_user_event(
                        conn,
                        &[owner],
                        &serde_json::json!({"what": "resources"}),
                    )
                    .await?;
                }
                continue;
            }
            // A new registration needs the owner's personal home; without one the owner chooses its home with
            // "Use here", and nothing is placed in another org on their behalf.
            let Some(home) = home else {
                continue;
            };
            let agent = match registrations::machine_agent(conn, machine, home, &found.harness_kind)
                .await?
            {
                Some(agent) => agent,
                None => {
                    let agent = orch_types::new_id();
                    rows::insert_agent(
                        conn,
                        &rows::NewAgent {
                            id: agent,
                            name: &found.harness_kind,
                            owner_id: owner,
                            home_id: home,
                            harness_kind: &found.harness_kind,
                            machine_id: Some(machine),
                            profile: &serde_json::json!({}),
                        },
                        now,
                    )
                    .await?;
                    orch_authz::ownership::start_resource(
                        conn,
                        orch_access_contract::decisions::ObjectRef {
                            kind: orch_access_contract::permissions::ObjectKind::Agent,
                            id: agent,
                        },
                        owner,
                        orch_authz::ownership::NewAccess::Defaults,
                        orch_types::Time::new(now),
                    )
                    .await
                    .map_err(|error| ApplyError::Other(error.to_string()))?;
                    agent
                }
            };
            let provider = orch_types::new_id();
            // Discovery grants no credential copies; the owner can allow further arrangements on its page.
            let arrangements = vec!["separated".to_owned()];
            let facts = serde_json::to_value(orch_agent_contract::facts::ProviderFacts {
                identity: found.identity.clone(),
                ..Default::default()
            })
            .map_err(|error| ApplyError::Other(error.to_string()))?;
            registrations::insert_machine_subscription(
                conn,
                &registrations::NewMachineSubscription {
                    id: provider,
                    name: &found.harness_kind,
                    owner_id: owner,
                    home_id: home,
                    harness_kind: &found.harness_kind,
                    agent_id: agent,
                    machine_id: machine,
                    arrangements: &arrangements,
                    facts: &facts,
                },
                now,
            )
            .await?;
            orch_authz::ownership::start_resource(
                conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Provider,
                    id: provider,
                },
                owner,
                orch_authz::ownership::NewAccess::Defaults,
                orch_types::Time::new(now),
            )
            .await
            .map_err(|error| ApplyError::Other(error.to_string()))?;
            orch_db::sequences::notify_user_event(
                conn,
                &[owner],
                &serde_json::json!({"what": "resources"}),
            )
            .await?;
        }
        Ok(())
    }

    /// What a machine says about itself is kept, whatever else its events
    /// change: the latest report in a delivery stands.
    async fn apply_machine_facts(
        &self,
        tx: &mut Conn,
        machine: Id,
        delivery: &Delivery,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let latest = delivery
            .events
            .iter()
            .rev()
            .find(|event| event.kind == EventKind::MachineFacts.as_str());
        let Some(event) = latest else { return Ok(()) };
        let Ok(Event::MachineFacts(facts)) = Event::decode(&event.kind, event.body.clone()) else {
            tracing::warn!(%machine, "a report of a machine's facts could not be read");
            return Ok(());
        };
        resources::set_machine_facts(
            tx,
            machine,
            &event.body,
            (facts.rates.up_bps, facts.rates.down_bps),
            RATE_WEIGHT,
            now,
        )
        .await
    }

    async fn apply_reset(
        &self,
        entry: &Arc<MachineEntry>,
        notice: &orch_link::streams::ResetNotice,
    ) {
        let stream = notice.stream.to_string();
        tracing::warn!(
            machine = %entry.id, stream, from = notice.from, reason = ?notice.reason,
            "a machine says that events of a stream are gone"
        );
        let result = async {
            let mut tx = self.inner.db.begin().await?;
            if !report_sources::owns(&mut tx, entry.id, &notice.stream).await? {
                return Ok(false);
            }
            let now = self.inner.clock.now();
            queries::set_cursor(&mut tx, entry.id, &stream, notice.from.max(1) - 1, now).await?;
            let applier = Arc::clone(&lock(&self.inner.applier));
            applier
                .reset(&mut tx, entry.id, &notice.stream, notice.reason)
                .await?;
            tx.commit().await.map_err(DbError::from)?;
            Ok::<_, ApplyError>(true)
        }
        .await;
        match result {
            Ok(true) => {
                let mut state = lock(&entry.state);
                state
                    .received_through
                    .insert(stream.clone(), notice.from.max(1) - 1);
                state.discarded_through.remove(&stream);
            }
            Ok(false) => {
                self.record_report_refusal(entry.id, &notice.stream, 1)
                    .await;
                return;
            }
            Err(error) => {
                tracing::error!(%error, machine = %entry.id, stream, "a reset could not be recorded");
                return;
            }
        }
        if let (true, Stream::Session { thread_id, .. }) = (notice.resync, notice.stream) {
            let resync = self
                .dispatcher()
                .issue(
                    entry.id,
                    &SerialKey::Thread(thread_id),
                    &SessionResync { thread_id },
                )
                .await;
            if let Err(error) = resync {
                tracing::error!(%error, machine = %entry.id, "a session could not be asked to report again");
            }
        }
    }

    async fn down_loop(
        &self,
        entry: Arc<MachineEntry>,
        mut down: mpsc::UnboundedReceiver<DownFrame>,
    ) {
        loop {
            let frame = tokio::select! {
                () = entry.retired.cancelled() => return,
                () = self.inner.shutdown.cancelled() => return,
                frame = down.recv() => frame,
            };
            let Some(frame) = frame else { return };
            let Some(route) = entry.route() else {
                // The machine has no session: what was meant for it is sent
                // again when it has, as the rules of the link have it.
                continue;
            };
            let frame = match frame {
                DownFrame::Cmd(cmd) => {
                    let id = cmd.id;
                    let original = cmd.clone();
                    let prepared = self
                        .prepare(&entry, route.fingerprint.as_deref(), cmd)
                        .await;
                    let admitted = match prepared {
                        Ok(cmd) => self.admit_wire_command(&entry, &original, cmd).await,
                        Err(error) => Err(error),
                    };
                    match admitted {
                        Ok(Some(cmd)) => DownFrame::Cmd(cmd),
                        Ok(None) => continue,
                        Err(error) => {
                            tracing::warn!(%error, command = %id, "a command cannot be put on the wire");
                            entry.controller.commands().on_cmd_res(&CmdRes {
                                id,
                                outcome: Err(error),
                            });
                            entry.mirror.notify_one();
                            continue;
                        }
                    }
                }
                other => other,
            };
            // Publication is synchronous and holds no database lease. Rebuild/route
            // replacement cannot let an older output task publish after its successor.
            self.send_current(&entry, &route, frame);
        }
    }

    fn send_current(
        &self,
        entry: &Arc<MachineEntry>,
        route: &SessionRoute,
        frame: DownFrame,
    ) -> bool {
        let machines = lock(&self.inner.machines);
        let current = machines.get(&entry.id).and_then(|cell| cell.get());
        if !current.is_some_and(|current| Arc::ptr_eq(current, entry))
            || entry.retired.is_cancelled()
        {
            return false;
        }
        let mut state = lock(&entry.state);
        if !state.route.as_ref().is_some_and(|current| {
            current.session_id == route.session_id && Arc::ptr_eq(&current.gateway, &route.gateway)
        }) {
            return false;
        }
        let command = match &frame {
            DownFrame::Cmd(command) => Some(command.id),
            _ => None,
        };
        let sent = route.gateway.send(BackplaneFrame::Down(Down {
            machine_id: entry.id,
            frame,
        }));
        if sent && let Some(id) = command {
            state.handed_over.push(id);
            entry.mirror.notify_one();
        }
        sent
    }

    /// Puts what the issuer follows into the rows: the commands handed to a gateway, and the states a
    /// command went through since the last time, all on one pooled connection (a busy pool is waited for
    /// once a pass, not once a command). Whoever waits for a command that ended is told.
    pub(crate) async fn sync_commands(&self, entry: &MachineEntry) {
        let (handed_over, changes): (Vec<Id>, Vec<(Id, Progress, Mirrored)>) = {
            let mut state = lock(&entry.state);
            let handed_over = std::mem::take(&mut state.handed_over);
            let mut changes = Vec::new();
            for (id, tracked) in &mut state.tracked {
                let progress = tracked.handle.progress();
                let now = Mirrored::of(&progress);
                if now != tracked.mirrored {
                    changes.push((*id, progress, tracked.mirrored));
                    tracked.mirrored = now;
                }
            }
            (handed_over, changes)
        };
        if !handed_over.is_empty() || !changes.is_empty() {
            match self.inner.db.acquire().await {
                Ok(mut conn) => {
                    let now = self.inner.clock.now();
                    // The hand-overs before the states: a later state of the same command is then the one
                    // that stands (each write moves a row only forward).
                    for id in handed_over {
                        if let Err(error) = queries::mark_sent(&mut conn, id, now).await {
                            tracing::warn!(%error, command = %id, "a hand-over could not be written");
                        }
                    }
                    for (id, progress, before) in changes {
                        if let Err(error) = write_progress(&mut conn, id, &progress, now).await {
                            tracing::warn!(%error, command = %id, "the state of a command could not be written");
                            if let Some(tracked) = lock(&entry.state).tracked.get_mut(&id) {
                                tracked.mirrored = before;
                            }
                            continue;
                        }
                        if progress.is_final() {
                            lock(&entry.state).tracked.remove(&id);
                            self.command_ended(id);
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, machine = %entry.id, "the states of commands could not be written");
                    let mut state = lock(&entry.state);
                    state.handed_over.extend(handed_over);
                    for (id, _, before) in changes {
                        if let Some(tracked) = state.tracked.get_mut(&id) {
                            tracked.mirrored = before;
                        }
                    }
                }
            }
        }
        entry.controller.commands().forget_ended();
    }

    /// Tells whoever waits for command `id` that its row says it is over.
    fn command_ended(&self, id: Id) {
        if let Some(waiting) = lock(&self.inner.ended).get(&id) {
            waiting.notify_waiters();
        }
    }

    /// What a caller that waits for command `id` is told by ([`Link::command_ended`]); kept while
    /// somebody waits.
    pub(crate) fn end_signal(&self, id: Id) -> EndSignal<'_> {
        let notify = Arc::clone(
            lock(&self.inner.ended)
                .entry(id)
                .or_insert_with(|| Arc::new(tokio::sync::Notify::new())),
        );
        EndSignal {
            link: self,
            id,
            notify,
        }
    }

    // ------------------------------------------------------------ the clocks

    async fn tick_loop(&self) {
        let mut ticks: u32 = 0;
        let mut interval = tokio::time::interval(TICK_EVERY);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            ticks = ticks.wrapping_add(1);
            if let Err(error) = self.advance_component_updates().await {
                tracing::warn!(%error, "physical component updates could not be advanced");
            }
            let entries: Vec<Arc<MachineEntry>> = lock(&self.inner.machines)
                .values()
                .filter_map(|cell| cell.get().cloned())
                .collect();
            for entry in &entries {
                self.adopt_watch_commands(entry).await;
                entry
                    .controller
                    .commands()
                    .tick(tokio::time::Instant::now());
                self.sync_commands(entry).await;
                if ticks.is_multiple_of(SWEEP_EVERY_TICKS) {
                    self.discover(entry, false);
                }
            }
            if ticks.is_multiple_of(SWEEP_EVERY_TICKS) {
                // A command a loaded link follows is ended by that link (its issuer lets one that no machine
                // took expire, and its mirror writes it); its row may not yet say that its machine took it,
                // and the sweep must not end one its machine is running (C-turns).
                let followed: Vec<Id> = entries
                    .iter()
                    .flat_map(|entry| {
                        lock(&entry.state)
                            .tracked
                            .keys()
                            .copied()
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let swept = async {
                    let mut conn = self.inner.db.acquire().await?;
                    queries::expire_unstarted(&mut conn, self.inner.clock.now(), &followed).await
                }
                .await;
                match swept {
                    Ok(expired) if !expired.is_empty() => self.inner.finished.notify_waiters(),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "expired commands could not be swept"),
                }
            }
        }
    }

    // ------------------------------------------------------------ terminals

    async fn terminal_loop(&self, mut out: mpsc::UnboundedReceiver<ApiOut>) {
        loop {
            let item = tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                item = out.recv() => item,
            };
            let Some(item) = item else { return };
            let machine = match &item {
                ApiOut::Text(frame) => machine_of(frame),
                ApiOut::Data(data) => Some(data.machine_id),
                ApiOut::Datagram { machine, .. } => Some(*machine),
            };
            let Some(machine) = machine else { continue };
            self.note_terminal_frame(&item);
            let gateway = lock(&self.inner.machines)
                .get(&machine)
                .and_then(|cell| cell.get().cloned())
                .and_then(|entry| entry.route())
                .map(|route| route.gateway);
            if let ApiOut::Datagram { machine, packet } = item {
                if let Some(gateway) = gateway {
                    gateway.send_datagram(machine, packet);
                }
                continue;
            }
            let delivered = match (&gateway, &item) {
                (Some(gateway), ApiOut::Text(frame)) => gateway.send(frame.as_ref().clone()),
                (Some(gateway), ApiOut::Data(data)) => gateway.send_data(data),
                (Some(_), ApiOut::Datagram { .. }) => false,
                (None, _) => false,
            };
            if !delivered
                && let ApiOut::Text(frame) = &item
                && let BackplaneFrame::TunOpen(open) = frame.as_ref()
            {
                // Nobody can carry the opening: say so now, not after the
                // time an opening is waited for.
                self.inner.terminals.on_frame(BackplaneFrame::TunOpenErr(
                    orch_link_contract::backplane::TunOpenErr {
                        machine_id: open.machine_id,
                        channel: open.channel,
                        code: orch_link_contract::tunnels::OpenErrorCode::PeerOffline,
                        detail: "the machine has no session".to_owned(),
                    },
                ));
            }
        }
    }

    fn note_terminal_frame(&self, item: &ApiOut) {
        let mut channels = lock(&self.inner.terminal_channels);
        if let ApiOut::Text(frame) = item {
            match frame.as_ref() {
                BackplaneFrame::TunOpen(open) => {
                    channels
                        .entry(open.machine_id)
                        .or_default()
                        .insert(open.channel);
                }
                BackplaneFrame::TunClose(close) => {
                    if let Some(open) = channels.get_mut(&close.machine_id) {
                        open.remove(&close.channel);
                    }
                }
                _ => {}
            }
        }
    }

    /// Ends the terminals of a machine whose session is gone: the tunnels
    /// the gateway held for them are gone with it.
    fn close_terminals_of(&self, machine: Id) {
        let open = lock(&self.inner.terminal_channels).remove(&machine);
        for channel in open.into_iter().flatten() {
            self.inner.terminals.on_frame(BackplaneFrame::TunClose(
                orch_link_contract::backplane::TunClose {
                    machine_id: machine,
                    channel,
                    code: orch_link_contract::tunnels::CloseCode::Reset,
                    detail: "the machine's session ended".to_owned(),
                },
            ));
        }
    }

    // ------------------------------------------------------------ a gateway's connection

    /// Serves one connection of a gateway until it ends.
    pub(crate) async fn serve_gateway(&self, socket: WebSocket) {
        let (mut sink, mut source) = socket.split();
        let first = tokio::time::timeout(HELLO_TIMEOUT, source.next()).await;
        let hello = match first {
            Ok(Some(Ok(Message::Text(text)))) => {
                match frames::decode::<BackplaneFrame>(text.as_str(), MAX_BACKPLANE_FRAME_BYTES) {
                    Ok(Text::Known(BackplaneFrame::GwHello(hello))) => hello,
                    _ => return,
                }
            }
            _ => return,
        };
        let (out, mut queued) = mpsc::unbounded_channel();
        let gateway = Arc::new(GatewayConn {
            generation: self.inner.generations.fetch_add(1, Ordering::Relaxed),
            out,
            closed: CancellationToken::new(),
            stats: Mutex::new(None),
            started_at: hello
                .started_at
                .unwrap_or_else(|| orch_types::Time::new(self.inner.clock.now())),
        });
        let replaced =
            lock(&self.inner.gateways).insert(hello.gateway_id.clone(), Arc::clone(&gateway));
        if let Some(old) = replaced {
            tracing::info!(gateway = %hello.gateway_id, "a gateway came back before its old connection was given up");
            old.closed.cancel();
        }
        tracing::info!(gateway = %hello.gateway_id, version = %hello.version, "a gateway connected");
        gateway.send(BackplaneFrame::ApiHello(ApiHello {
            ticket_public_key: self.inner.tickets.public_key(),
            limits: self.inner.settings.limits,
            versions: self.inner.settings.versions.clone(),
        }));
        self.heartbeat(&hello.version, &gateway).await;

        let writer_closed = gateway.closed.clone();
        let writer = tokio::spawn(async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let message = tokio::select! {
                    () = writer_closed.cancelled() => break,
                    _ = ping.tick() => Message::Ping(Bytes::new()),
                    item = queued.recv() => match item {
                        None => break,
                        Some(Outbound::Frame(frame)) => match frames::encode(frame.as_ref(), MAX_BACKPLANE_FRAME_BYTES) {
                            Ok(text) => Message::Text(text.into()),
                            Err(error) => {
                                tracing::error!(%error, "a frame of the backplane cannot be written");
                                continue;
                            }
                        },
                        Some(Outbound::Data(bytes)) => Message::Binary(bytes),
                        Some(Outbound::Datagram {machine,packet}) => {
                            let data=orch_link_contract::ports::BackplanePortDatagram {machine_id:machine,datagram:packet.frame.clone()};
                            let Ok(bytes)=data.encode() else {continue;};
                            if sink.send(Message::Binary(bytes)).await.is_err() {break;}
                            packet.delivered();
                            continue;
                        },
                    },
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        let dead_after = PING_EVERY * DEAD_AFTER_PINGS;
        loop {
            let message = tokio::select! {
                () = gateway.closed.cancelled() => break,
                () = self.inner.shutdown.cancelled() => break,
                message = tokio::time::timeout(dead_after, source.next()) => message,
            };
            let message = match message {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(_)) | None) => break,
                Err(_) => {
                    tracing::warn!(gateway = %hello.gateway_id, "a gateway has been silent for too long");
                    break;
                }
            };
            match message {
                Message::Text(text) => {
                    match frames::decode::<BackplaneFrame>(text.as_str(), MAX_BACKPLANE_FRAME_BYTES)
                    {
                        Ok(Text::Known(frame)) => {
                            self.on_frame(&gateway, &hello.version, frame).await
                        }
                        Ok(Text::Unknown(kind)) => {
                            tracing::debug!(kind, "a frame of an unknown type was passed over");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "a gateway sent a frame that is not one");
                            break;
                        }
                    }
                }
                Message::Binary(bytes)
                    if bytes.first() == Some(&orch_link_contract::ports::PortDatagram::KIND) =>
                {
                    if let Ok(data) =
                        orch_link_contract::ports::BackplanePortDatagram::decode(bytes)
                    {
                        self.inner.terminals.on_datagram(data);
                    }
                }
                Message::Binary(bytes) => match BackplaneData::decode(bytes) {
                    Ok(data) => self.inner.terminals.on_data(data),
                    Err(error) => {
                        tracing::warn!(%error, "a gateway sent data that is not data");
                        break;
                    }
                },
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
        gateway.closed.cancel();
        let _ = writer.await;
        self.gateway_lost(&gateway, &hello.gateway_id).await;
    }

    async fn on_frame(&self, gateway: &Arc<GatewayConn>, version: &str, frame: BackplaneFrame) {
        if !frame.sent_by(orch_link_contract::backplane::BackplaneSide::Gateway) {
            tracing::warn!("a gateway sent a frame that only the system sends");
            return;
        }
        match frame {
            BackplaneFrame::PortConnectionsReply(reply) => {
                let pending = {
                    let mut pending = lock(&self.inner.port_queries);
                    if pending
                        .get(&reply.request_ref)
                        .is_some_and(|(env, generation, owner, _)| {
                            *env == reply.environment_id
                                && *generation == gateway.generation
                                && owner.is_none()
                        })
                    {
                        pending.remove(&reply.request_ref)
                    } else {
                        None
                    }
                };
                if let Some((_, _, _, answer)) = pending {
                    let _ = answer.send(reply.connections);
                }
            }
            BackplaneFrame::AuthQuery(query) => {
                let link = self.clone();
                let gateway = Arc::clone(gateway);
                tokio::spawn(async move {
                    let reply = link.authorize(&query).await;
                    gateway.send(BackplaneFrame::AuthReply(reply));
                });
            }
            BackplaneFrame::MachineUp(up) => self.machine_up(gateway, up).await,
            BackplaneFrame::MachineDown(down) => self.machine_down(down).await,
            BackplaneFrame::Up(up) => {
                self.route_up(gateway, up.machine_id, up.session_id, up.frame)
                    .await
            }
            BackplaneFrame::Stats(mut stats) => {
                let samples = std::mem::take(&mut stats.metrics);
                *lock(&gateway.stats) = Some(stats);
                // What the figures write goes to the database on a task of its own: this reader serves every
                // machine behind the gateway, and waits for no pool (C-turns).
                let link = self.clone();
                let gateway = Arc::clone(gateway);
                let version = version.to_owned();
                tokio::spawn(async move {
                    if link.inner.store_metrics {
                        let result = async {
                            let mut conn = link.inner.db.acquire().await?;
                            orch_db::queries::metrics::write_process_samples(
                                &mut conn,
                                link.inner.clock.now(),
                                "gateway",
                                run_id(gateway.generation),
                                &samples,
                            )
                            .await
                        }
                        .await;
                        if let Err(error) = result {
                            tracing::debug!(%error,"the gateway's measured interval could not be stored");
                        }
                    }
                    link.heartbeat(&version, &gateway).await;
                    link.touch_seen(&gateway).await;
                });
            }
            frame @ (BackplaneFrame::TunOpenOk(_)
            | BackplaneFrame::TunOpenErr(_)
            | BackplaneFrame::TunCredit(_)
            | BackplaneFrame::TunEof(_)
            | BackplaneFrame::TunClose(_)
            | BackplaneFrame::TunText(_)) => {
                if let BackplaneFrame::TunClose(close) = &frame
                    && let Some(open) =
                        lock(&self.inner.terminal_channels).get_mut(&close.machine_id)
                {
                    open.remove(&close.channel);
                }
                self.inner.terminals.on_frame(frame);
            }
            BackplaneFrame::GwHello(_)
            | BackplaneFrame::ApiHello(_)
            | BackplaneFrame::AuthReply(_)
            | BackplaneFrame::Down(_)
            | BackplaneFrame::Kick(_)
            | BackplaneFrame::TunOpen(_) => {}
            BackplaneFrame::PortConnectionsQuery(_) => {}
        }
    }

    /// The machines whose sessions this gateway holds are seen now: their last-seen time moves,
    /// at most once in [`SEEN_EVERY_SECS`] for each, at the gateway's figures (every
    /// `STATS_EVERY_SECS`), so a machine that stays connected never reads as seen long ago.
    async fn touch_seen(&self, gateway: &Arc<GatewayConn>) {
        let held: Vec<Id> = lock(&self.inner.machines)
            .values()
            .filter_map(|cell| cell.get().cloned())
            .filter(|entry| {
                entry
                    .route()
                    .is_some_and(|route| Arc::ptr_eq(&route.gateway, gateway))
            })
            .map(|entry| entry.id)
            .collect();
        if held.is_empty() {
            return;
        }
        let now = self.inner.clock.now();
        let result = async {
            let mut conn = self.inner.db.acquire().await?;
            resources::touch_seen(&mut conn, &held, now, SEEN_EVERY_SECS).await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "the machines' last-seen times could not be written");
        }
    }

    async fn heartbeat(&self, version: &str, gateway: &GatewayConn) {
        let now = self.inner.clock.now();
        let run = run_id(gateway.generation);
        let result = async {
            let mut conn = self.inner.db.acquire().await?;
            let started_at = gateway.started_at.get();
            orch_db::heartbeat::beat(&mut conn, "gateway", run, version, started_at, now).await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "the gateway's heartbeat could not be written");
        }
    }

    /// A gateway's connection ended: the sessions it held are gone with it.
    async fn gateway_lost(&self, gateway: &Arc<GatewayConn>, gateway_id: &str) {
        {
            let mut gateways = lock(&self.inner.gateways);
            if gateways
                .get(gateway_id)
                .is_some_and(|current| current.generation == gateway.generation)
            {
                gateways.remove(gateway_id);
            }
        }
        tracing::info!(gateway = %gateway_id, "a gateway is gone");
        let entries: Vec<Arc<MachineEntry>> = lock(&self.inner.machines)
            .values()
            .filter_map(|cell| cell.get().cloned())
            .collect();
        for entry in entries {
            let lost = {
                let mut state = lock(&entry.state);
                match &state.route {
                    Some(route) if route.gateway.generation == gateway.generation => {
                        let session = route.session_id;
                        state.route = None;
                        Some(session)
                    }
                    _ => None,
                }
            };
            if let Some(session) = lost {
                entry.controller.session_down(session);
                self.sync_commands(&entry).await;
                self.close_terminals_of(entry.id);
            }
        }
    }
}

// Commands for transient watches may have been issued by another API process.
impl Link {
    async fn adopt_watch_commands(&self, entry: &MachineEntry) {
        if !entry.is_online() {
            return;
        }
        let open = async {
            let mut conn = self.inner.db.acquire().await?;
            queries::open_commands(&mut conn, entry.id).await
        }
        .await;
        match open {
            Ok(open) => {
                for row in open {
                    if matches!(row.kind.as_str(), "watch.set" | "fs.list" | "fs.unwatch") {
                        let progress = if row.state == "accepted" {
                            Progress::Accepted
                        } else {
                            Progress::Queued
                        };
                        entry.adopt(crate::dispatch::cmd_of(&row), progress);
                    }
                }
            }
            Err(error) => {
                tracing::debug!(%error, "watch commands issued elsewhere could not be read")
            }
        }
    }
}

/// The identifier a gateway's connection reports as its run: the same for
/// the life of one connection.
fn run_id(generation: u64) -> Id {
    let mut bytes = [0u8; 16];
    bytes[8..].copy_from_slice(&generation.to_be_bytes());
    Id::from_bytes(bytes)
}

fn machine_of(frame: &BackplaneFrame) -> Option<Id> {
    match frame {
        BackplaneFrame::TunOpen(f) => Some(f.machine_id),
        BackplaneFrame::TunCredit(f) => Some(f.machine_id),
        BackplaneFrame::TunEof(f) => Some(f.machine_id),
        BackplaneFrame::TunClose(f) => Some(f.machine_id),
        BackplaneFrame::TunText(f) => Some(f.machine_id),
        _ => None,
    }
}

fn role_name(role: &Role) -> String {
    match serde_json::to_value(role) {
        Ok(Json::String(name)) => name,
        _ => String::new(),
    }
}

fn roles_of(names: &[String]) -> Vec<Role> {
    names
        .iter()
        .filter_map(|name| serde_json::from_value(Json::String(name.clone())).ok())
        .collect()
}

struct PortQueryRegistration<'a> {
    link: &'a Link,
    reference: Id,
}
impl Drop for PortQueryRegistration<'_> {
    fn drop(&mut self) {
        lock(&self.link.inner.port_queries).remove(&self.reference);
    }
}

/// Writes one state of a command into its row, on a connection the caller holds. Each write moves a row
/// only forward, whatever was written before it.
async fn write_progress(
    conn: &mut Conn,
    id: Id,
    progress: &Progress,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    match progress {
        Progress::Queued => {
            queries::mark_queued(conn, id, now).await?;
        }
        // Handing over is written from the hand-overs the link keeps ([`Link::sync_commands`]).
        Progress::Sent => {}
        Progress::Accepted => {
            queries::mark_accepted(conn, id, now).await?;
        }
        Progress::Succeeded(result) => {
            queries::finish_command(conn, id, &queries::Ending::Succeeded(result), now).await?;
        }
        Progress::Failed(error) => {
            let error = serde_json::to_value(error).unwrap_or(Json::Null);
            queries::finish_command(conn, id, &queries::Ending::Failed(&error), now).await?;
        }
        Progress::Expired => {
            queries::finish_command(conn, id, &queries::Ending::Expired, now).await?;
        }
        Progress::Cancelled => {
            queries::finish_command(conn, id, &queries::Ending::Cancelled, now).await?;
        }
    }
    Ok(())
}

/// A caller's wait for one command to end ([`Link::end_signal`]): told by the mirror that writes its end,
/// and forgotten when the last one that waits for that command stops.
pub(crate) struct EndSignal<'a> {
    link: &'a Link,
    id: Id,
    notify: Arc<tokio::sync::Notify>,
}

impl EndSignal<'_> {
    /// Resolves when the command is said to have ended, from the moment this is made and enabled.
    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

impl Drop for EndSignal<'_> {
    fn drop(&mut self) {
        let mut ended = lock(&self.link.inner.ended);
        // The table holds one, this wait another: nobody else waits for the command.
        if ended
            .get(&self.id)
            .is_some_and(|kept| Arc::ptr_eq(kept, &self.notify) && Arc::strong_count(kept) <= 2)
        {
            ended.remove(&self.id);
        }
    }
}

// Replacement enrollment and certificate handover through session authorization.
#[cfg(test)]
mod component_update_tests;
#[cfg(test)]
#[path = "backplane/reenrollment_tests.rs"]
mod reenrollment_tests;

#[cfg(test)]
#[path = "backplane/binding_tests.rs"]
mod binding_tests;

mod port_records;
#[path = "backplane/report_sources.rs"]
mod report_sources;

#[cfg(test)]
#[path = "backplane/session_source_tests.rs"]
mod session_source_tests;

#[cfg(test)]
#[path = "backplane/machine_report_tests.rs"]
mod machine_report_tests;

#[path = "backplane/report_refusals.rs"]
mod report_refusals;

#[cfg(test)]
pub(crate) mod command_admission_tests;
