//! The machine's end of a link: one handle that lives as long as the machine.
//!
//! A machine holds one session to its gateway, and sessions come and go. What
//! lives on is [`MachineLink`]: the spool of events that the system has not
//! taken in, the journal of commands it has been given, and the tunnels that
//! are waiting to be taken up again. Around them a supervisor keeps a session
//! going: it dials the endpoints in order, runs the handshake, hands the
//! session to the core and, when it ends, waits as the backoff says and
//! begins again.
//!
//! Everything the machine does with the link goes through the handle:
//!
//! * `publish` puts a durable event into the spool and returns; delivery and
//!   acknowledgement go on by themselves, across any number of sessions;
//! * `report` sends a measurement that is worthless when it is late;
//! * `serve` registers what carries the system's commands out;
//! * `open_tunnel` and `incoming_tunnels` give tunnels as byte streams;
//! * `status` and `measurements` say how the link stands.
//!
//! The core owns the spool and the streams and handles one thing at a time,
//! so that the order in which things happened is the order in which they are
//! dealt with.

use crate::commands::{CommandHandler, CommandServer, ServerOptions};
use crate::framing::Measurements;
use crate::journal::Journal;
use crate::reconnect::{Backoff, Dialer, Endpoints, dial_in_order};
use crate::session::{
    ClientIdentity, EndReason, Established, HandshakeError, SessionEvent, SessionSender, connect,
};
use crate::spool::SpoolOptions;
use crate::streams::{PublishError, StreamSender};
use crate::tunnels::channel::{IncomingTunnels, OpenError, Tunnel, TunnelHub, TunnelOptions};
use crate::tunnels::flow::Gauge;
use orch_link_contract::enrollment::Endpoint;
use orch_link_contract::events::{Event, Report};
use orch_link_contract::frames::Frame;
use orch_link_contract::session::{HEARTBEAT_SECS, MachineFactsRates, Platform, Ready, Role};
use orch_link_contract::streams::{Eph, Stream};
use orch_link_contract::version::RejectCode;
use orch_pki::certificates::{Certificate, LeafKey};
use orch_types::{Clock, Id};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

/// How often the core looks at the clocks: to force the spool to disk, to see
/// whether a stream has stalled, to forget what is old. A fifth of a second is
/// the longest the protocol lets an event wait before it is on disk.
const CORE_TICK: Duration = Duration::from_millis(200);
/// How many ticks pass between two looks at what is old.
const HOUSEKEEPING_EVERY: u32 = 300;

/// How the machine's link stands.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkStatus {
    /// Not started yet.
    Starting,
    /// Trying to reach a gateway.
    Connecting {
        /// How many attempts in a row failed before this one.
        failures: u32,
    },
    /// A session stands.
    Connected {
        /// The session.
        session_id: Id,
        /// The address it was made through.
        endpoint: String,
    },
    /// Waiting before the next attempt.
    Waiting {
        /// How many attempts in a row failed.
        failures: u32,
        /// How long the wait is.
        retry_in: Duration,
        /// What went wrong last.
        why: String,
    },
    /// The gateway refused the session, and what to do about it is for the
    /// machine's owner or its connector, not the link.
    Refused {
        /// A server-authenticated drain grant for a node that cannot enter on its old executable.
        update_permit: Option<orch_link_contract::session::UpdatePermit>,
        /// The code.
        code: RejectCode,
        /// A sentence for the log.
        detail: String,
    },
    /// The link was shut down.
    Stopped,
}

/// What a machine is and how it reaches the system.
#[derive(Debug)]
pub struct MachineConfig {
    /// The machine.
    pub machine_id: Id,
    /// The connector's own version.
    pub connector_version: String,
    /// What it is built for.
    pub platform: Platform,
    /// The roles it has switched on.
    pub roles: Vec<Role>,
    /// The key and certificate with which it proves itself where the
    /// connection shows no certificate.
    pub proof: Option<(LeafKey, Certificate)>,
    /// The addresses of the gateway, in the order enrollment gave.
    pub endpoints: Vec<Endpoint>,
    /// Whether new authenticated system lists replace this effective list.
    pub follow_system_endpoints: bool,
    /// An explicit local preference list for this node's hosted units.
    pub hosted_endpoints: Option<Vec<Endpoint>>,
    /// The directory of the connector; the spool and the journal are kept in
    /// it.
    pub data_dir: PathBuf,
    /// The limits of the spool.
    pub spool: SpoolOptions,
    /// How long a command's journal entry is kept.
    pub journal_keep: Duration,
    /// Whether a person connected the machine; the system made none of
    /// those that are not.
    pub connected: bool,
    /// How its tunnels wait for a new session and how much they keep.
    pub tunnels: TunnelOptions,
    /// A count of the bytes its tunnels hold, for whoever watches that the
    /// bound holds.
    pub tunnel_gauge: Option<Arc<Gauge>>,
}

/// Why the link could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The spool could not be opened.
    #[error("{0}")]
    Spool(#[from] crate::spool::SpoolError),
    /// The journal could not be opened.
    #[error("{0}")]
    Journal(#[from] crate::journal::JournalError),
}

enum Input {
    RawAfter {
        stream: Stream,
        after: i64,
        reply: oneshot::Sender<Result<(), PublishError>>,
    },
    Publish {
        stream: Stream,
        // Environment facts carry discovery lists; keep other queue entries small.
        event: Box<Event>,
        reply: oneshot::Sender<Result<(), PublishError>>,
    },
    Report {
        eph: Eph,
    },
    Ready(Box<SessionStart>),
    Frame(Box<Frame>),
    Data(orch_link_contract::frames::DataFrame),
    Datagram(crate::ports::Datagram),
    Ended,
    Shutdown(oneshot::Sender<()>),
}

/// What the core needs of a session that stands.
#[derive(Debug)]
struct SessionStart {
    ready: Ready,
    sender: SessionSender,
    clock_offset_ms: i64,
}

#[derive(Debug)]
struct Shared {
    directory: PathBuf,
    access_token: Mutex<Option<String>>,
    endpoints: Arc<Mutex<Vec<Endpoint>>>,
    follows_system: bool,
    hosted_endpoints: Option<Vec<Endpoint>>,
    active_endpoint: Arc<Mutex<Option<Endpoint>>>,
    advertised: watch::Sender<Option<Vec<Endpoint>>>,
    inputs: mpsc::UnboundedSender<Input>,
    status: watch::Receiver<LinkStatus>,
    commands: CommandServer,
    meter: Mutex<Option<Measurements>>,
    session: Mutex<Option<SessionSender>>,
    versions: Mutex<Option<orch_link_contract::version::ConnectorVersions>>,
    mcp_imports: Mutex<
        std::collections::HashMap<
            Id,
            oneshot::Sender<orch_link_contract::mcp_import::McpImportAnswer>,
        >,
    >,
    local_ports: Mutex<
        std::collections::HashMap<
            Id,
            (
                Id,
                oneshot::Sender<orch_link_contract::local_ports::PortLocalAnswer>,
            ),
        >,
    >,
    port_records: Mutex<Option<Arc<crate::ports::PortRecords>>>,
    tunnels: TunnelHub,
}

/// The handle on a machine's link.
#[derive(Debug, Clone)]
pub struct MachineLink {
    shared: Arc<Shared>,
}

/// Why a request from the local owner has no definitive answer from the system.
#[derive(Debug, thiserror::Error)]
pub enum MachineRequestError {
    /// No authenticated session can carry it.
    #[error("the connector is not connected to the system")]
    Offline,
    /// The bounded pending set is full or already carries this key.
    #[error("too many local imports are awaiting the system")]
    Busy,
    /// The system may have committed before the reply was lost; never claim it certainly failed.
    #[error("the import was not confirmed; inspect the machine's Tools before trying again")]
    Unconfirmed,
}

struct PendingImport {
    shared: Arc<Shared>,
    id: Id,
}
struct PendingPortRead {
    shared: Arc<Shared>,
    id: Id,
}
impl Drop for PendingPortRead {
    fn drop(&mut self) {
        self.shared
            .local_ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}
impl Drop for PendingImport {
    fn drop(&mut self) {
        self.shared
            .mcp_imports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

impl MachineLink {
    /// Current port control from the enrolled gateway; a host listener retires on false.
    #[must_use]
    pub fn port_control(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shared.tunnels.port_control()
    }
    /// Starts the link: opens the spool and the journal and begins to keep a
    /// session going. Must be called inside a Tokio runtime.
    pub fn start(
        config: MachineConfig,
        dialer: Arc<dyn Dialer>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StartError> {
        let journal_dir = config.data_dir.join("journal");
        let (streams, recovery) = StreamSender::open_for_machine(
            &config.data_dir,
            config.spool,
            config.roles.clone(),
            Instant::now(),
        )?;
        if !recovery.damaged.is_empty() {
            tracing::warn!(streams = ?recovery.damaged, "the spool was damaged; the system will be told");
        }
        let journal = Journal::open(&journal_dir, config.journal_keep, clock.time())?;
        let commands = CommandServer::new(
            journal,
            Arc::clone(&clock),
            ServerOptions {
                roles: config.roles.clone(),
                connected: config.connected,
            },
        );
        let (inputs, receiver) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(LinkStatus::Starting);
        let tunnels = TunnelHub::with_options(config.tunnels, config.tunnel_gauge.clone());
        let (advertised, _) = watch::channel(None);
        let endpoints_now = Arc::new(Mutex::new(config.endpoints.clone()));
        let active_endpoint = Arc::new(Mutex::new(None));
        let shared = Arc::new(Shared {
            directory: config.data_dir.clone(),
            access_token: Mutex::new(None),
            follows_system: config.follow_system_endpoints,
            hosted_endpoints: config.hosted_endpoints.clone(),
            endpoints: Arc::clone(&endpoints_now),
            active_endpoint: Arc::clone(&active_endpoint),
            advertised: advertised.clone(),
            inputs: inputs.clone(),
            status: status_rx,
            commands: commands.clone(),
            meter: Mutex::new(None),
            session: Mutex::new(None),
            versions: Mutex::new(None),
            mcp_imports: Mutex::new(std::collections::HashMap::new()),
            local_ports: Mutex::new(std::collections::HashMap::new()),
            port_records: Mutex::new(None),
            tunnels: tunnels.clone(),
        });
        let identity = ClientIdentity {
            machine_id: config.machine_id,
            connector_version: config.connector_version,
            platform: config.platform,
            roles: config.roles,
            proof: config.proof,
            endpoints: (!config.follow_system_endpoints).then_some(config.endpoints.clone()),
            hosted_endpoints: config.hosted_endpoints.clone(),
        };
        tokio::spawn(core(
            receiver,
            streams,
            commands,
            tunnels,
            Arc::clone(&shared),
            Arc::clone(&clock),
        ));
        tokio::spawn(supervise(
            identity,
            Endpoints::new(config.endpoints),
            dialer,
            clock,
            inputs,
            status_tx,
            Arc::clone(&shared),
        ));
        Ok(Self { shared })
    }

    /// The effective preference list, including a server refresh when no local override holds it.
    #[must_use]
    pub fn endpoints(&self) -> Vec<Endpoint> {
        self.shared
            .endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The preference list this node gives its environments.
    #[must_use]
    pub fn hosted_endpoints(&self) -> Vec<Endpoint> {
        self.shared
            .hosted_endpoints
            .clone()
            .unwrap_or_else(|| self.endpoints())
    }

    /// Whether this physical machine follows the system's preference list.
    #[must_use]
    pub fn follows_system(&self) -> bool {
        self.shared.follows_system
    }

    /// The address and trust mode of the actual standing connection.
    #[must_use]
    pub fn connected_endpoint(&self) -> Option<Endpoint> {
        self.shared
            .active_endpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The standing session's ephemeral credential for content requests through the front.
    #[must_use]
    pub fn session_token(&self) -> Option<String> {
        self.shared
            .access_token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Authenticated system lists, including those a local override declines to use.
    #[must_use]
    pub fn advertised_endpoints(&self) -> watch::Receiver<Option<Vec<Endpoint>>> {
        self.shared.advertised.subscribe()
    }

    /// Puts a durable event into the spool and returns. The event is sent as
    /// soon as the stream has room, and again after any break, until the
    /// system has taken it in.
    pub async fn publish(&self, stream: &Stream, event: &Event) -> Result<(), PublishError> {
        self.publish_now(stream, event).await
    }

    /// [`MachineLink::publish`], the event handed to the link at once and the answer left to wait for
    /// (C-turns): whoever has several events hands them all over in their order and then waits, and they
    /// go into the spool in one turn of the link, under one forcing to disk, not in one turn each.
    pub fn publish_now(
        &self,
        stream: &Stream,
        event: &Event,
    ) -> impl std::future::Future<Output = Result<(), PublishError>> + Send + 'static {
        let handed = if event.kind().travels_on(stream) {
            let (reply, answer) = oneshot::channel();
            let _ = self.shared.inputs.send(Input::Publish {
                stream: *stream,
                event: Box::new(event.clone()),
                reply,
            });
            Ok(answer)
        } else {
            Err(PublishError::WrongStream {
                kind: event.kind().as_str().to_owned(),
                stream: stream.to_string(),
            })
        };
        async move {
            match handed {
                Ok(answer) => answer.await.unwrap_or(Ok(())),
                Err(error) => Err(error),
            }
        }
    }

    /// Raises the capture allocator above the server's cursor without changing spooled frame ids.
    pub async fn raw_after(&self, stream: &Stream, after: i64) -> Result<(), PublishError> {
        let (reply, answer) = oneshot::channel();
        let _ = self.shared.inputs.send(Input::RawAfter {
            stream: *stream,
            after,
            reply,
        });
        answer.await.unwrap_or(Ok(()))
    }

    /// Sends a measurement. It is not kept: while no session stands it is
    /// dropped.
    pub fn report(&self, stream: &Stream, at: orch_types::Time, report: &Report) {
        if let Ok(eph) = Eph::of(stream, at, report) {
            let _ = self.shared.inputs.send(Input::Report { eph });
        }
    }

    /// Registers what carries the system's commands out.
    pub fn serve(&self, handler: Arc<dyn CommandHandler>) {
        self.shared.commands.serve(handler);
    }

    /// How the link stands now, and a way to wait for it to change.
    #[must_use]
    pub fn status(&self) -> watch::Receiver<LinkStatus> {
        self.shared.status.clone()
    }

    /// The latest authenticated gateway's minimum and recommended connector builds, retained while offline.
    #[must_use]
    pub fn versions(&self) -> Option<orch_link_contract::version::ConnectorVersions> {
        self.shared
            .versions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Asks the system to file one local import. The same request id is safe to repeat after an uncertain answer.
    pub async fn import_mcp(
        &self,
        request: orch_link_contract::mcp_import::McpImportAsk,
    ) -> Result<orch_link_contract::mcp_import::McpImportAnswer, MachineRequestError> {
        let sender = self
            .shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(MachineRequestError::Offline)?;
        let (tell, told) = oneshot::channel();
        let id = request.request_id;
        {
            let mut pending = self
                .shared
                .mcp_imports
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if pending.len() >= orch_link_contract::mcp_import::MAX_PENDING
                || pending.contains_key(&id)
            {
                return Err(MachineRequestError::Busy);
            }
            pending.insert(id, tell);
        }
        let _pending = PendingImport {
            shared: Arc::clone(&self.shared),
            id,
        };
        sender
            .send(Frame::McpImportAsk(request))
            .map_err(|_| MachineRequestError::Offline)?;
        tokio::time::timeout(
            Duration::from_secs(orch_link_contract::mcp_import::ANSWER_WITHIN_SECS),
            told,
        )
        .await
        .map_err(|_| MachineRequestError::Unconfirmed)?
        .map_err(|_| MachineRequestError::Offline)
    }

    /// Reads only this machine's current attached environment over its authenticated control link.
    /// Correlations expire and disappear on cancellation; no command or event is retained.
    pub async fn local_ports(
        &self,
        request: orch_link_contract::local_ports::PortLocalAsk,
    ) -> Result<orch_link_contract::local_ports::PortLocalAnswer, MachineRequestError> {
        let sender = self
            .shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(MachineRequestError::Offline)?;
        let (send, receive) = oneshot::channel();
        let id = request.request_id;
        {
            let mut pending = self
                .shared
                .local_ports
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if pending.contains_key(&id)
                || pending
                    .values()
                    .filter(|(environment, _)| *environment == request.environment_id)
                    .count()
                    >= orch_link_contract::local_ports::PENDING_PER_ENVIRONMENT
            {
                return Err(MachineRequestError::Busy);
            }
            pending.insert(id, (request.environment_id, send));
        }
        let _pending = PendingPortRead {
            shared: Arc::clone(&self.shared),
            id,
        };
        sender
            .send(Frame::PortLocalAsk(request))
            .map_err(|_| MachineRequestError::Offline)?;
        tokio::time::timeout(
            Duration::from_secs(orch_link_contract::local_ports::ANSWER_WITHIN_SECS),
            receive,
        )
        .await
        .map_err(|_| MachineRequestError::Unconfirmed)?
        .map_err(|_| MachineRequestError::Offline)
    }

    /// Waits until a session stands.
    pub async fn connected(&self) {
        let mut status = self.status();
        loop {
            if matches!(*status.borrow_and_update(), LinkStatus::Connected { .. }) {
                return;
            }
            if status.changed().await.is_err() {
                return;
            }
        }
    }

    /// What is seen of the link: the round trip and the rate in each
    /// direction.
    #[must_use]
    pub fn measurements(&self) -> Measurements {
        self.shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|session| session.meter().measurements())
            .or(*self
                .shared
                .meter
                .lock()
                .unwrap_or_else(PoisonError::into_inner))
            .unwrap_or_default()
    }

    /// How many bytes of payload the next data frame carries, as the link has
    /// been seen to carry them; `None` while no session stands.
    #[must_use]
    pub fn data_frame_bytes(&self) -> Option<usize> {
        self.shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(SessionSender::data_frame_bytes)
    }

    /// The figures of the link as a machine reports them in its facts.
    #[must_use]
    pub fn rates(&self) -> MachineFactsRates {
        self.measurements().as_facts()
    }

    /// How far this machine's clock is ahead of the system's, in milliseconds, as
    /// the last session's handshake measured it (the system's time in its hello
    /// against this clock); 0 before the first session. Reported in the facts.
    #[must_use]
    pub fn clock_offset_ms(&self) -> i64 {
        self.shared.commands.clock_offset_ms()
    }

    /// Opens a tunnel with the ticket the system signed, and waits for the
    /// other end to take it.
    pub async fn open_tunnel(&self, ticket: &str) -> Result<Tunnel, OpenError> {
        self.shared.tunnels.open(ticket).await
    }

    /// Opens one accepted host socket under this node's publication template.
    pub async fn open_host_port(
        &self,
        ticket: &str,
        peer: orch_link_contract::ports::HostPortPeer,
    ) -> Result<Tunnel, OpenError> {
        self.shared.tunnels.open_host_port(ticket, peer).await
    }

    /// The current gateway's negotiated port bounds.
    #[must_use]
    pub fn port_limits(&self) -> orch_link_contract::version::PortLimits {
        self.shared.tunnels.port_limits()
    }

    /// Reserves a node's pending UDP packet in its environment's existing shared tunnel budget.
    pub fn reserve_port_datagram(
        &self,
        environment: Id,
        payload: bytes::Bytes,
    ) -> Option<crate::ports::Datagram> {
        self.shared
            .tunnels
            .reserve_port_datagram(environment, payload)
    }

    /// Registers the node's memory-only host record on the current authenticated control path.
    pub fn serve_port_records(&self, records: Arc<crate::ports::PortRecords>) {
        *self
            .shared
            .port_records
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(records);
    }

    /// Tunnels that other machines open to this one.
    #[must_use]
    pub fn incoming_tunnels(&self) -> IncomingTunnels {
        self.shared.tunnels.incoming()
    }

    /// Joins cancelled local captures and publications before a graceful physical-program stop.
    pub async fn cancel_recordings(&self) {
        self.shared.commands.cancel_recordings().await;
    }

    /// Ends the session, forces the spool to disk and stops.
    pub async fn shutdown(&self) {
        let (done, finished) = oneshot::channel();
        if self.shared.inputs.send(Input::Shutdown(done)).is_ok() {
            let _ = finished.await;
        }
    }
}

/// Keeps a session going: dials, runs the handshake, hands the session to the
/// core, and, when it ends, waits and begins again.
async fn supervise(
    identity: ClientIdentity,
    mut endpoints: Endpoints,
    dialer: Arc<dyn Dialer>,
    clock: Arc<dyn Clock>,
    inputs: mpsc::UnboundedSender<Input>,
    status: watch::Sender<LinkStatus>,
    shared: Arc<Shared>,
) {
    let mut backoff = Backoff::default();
    loop {
        if inputs.is_closed() {
            return;
        }
        status.send_replace(LinkStatus::Connecting {
            failures: backoff.failures(),
        });
        let mut named_by_gateway = None;
        let why = match dial_in_order(dialer.as_ref(), &endpoints).await {
            Err(failures) => failures
                .into_iter()
                .map(|(endpoint, error)| format!("{}: {error}", endpoint.url))
                .collect::<Vec<_>>()
                .join("; "),
            Ok((connection, endpoint)) => {
                if let Err(error) = keep_connected_endpoint(&shared.directory, &endpoint) {
                    tracing::warn!(%error, "the verified connection address could not be saved");
                }
                match connect(connection, &identity, clock.as_ref()).await {
                    Ok(established) => {
                        endpoints.worked(&endpoint);
                        *shared
                            .access_token
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) =
                            Some(established.ready.session_token.clone());
                        *shared
                            .active_endpoint
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = Some(endpoint.clone());
                        if let Some(listed) = &established.ready.endpoints {
                            shared.advertised.send_replace(Some(listed.clone()));
                            if shared.follows_system
                                && *shared
                                    .endpoints
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    != *listed
                            {
                                *shared
                                    .endpoints
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner) = listed.clone();
                                endpoints = Endpoints::new(listed.clone());
                            }
                        }
                        backoff.succeeded();
                        status.send_replace(LinkStatus::Connected {
                            session_id: established.ready.session_id,
                            endpoint: endpoint.url.clone(),
                        });
                        let ended = run_session(established, &inputs).await;
                        format!("the session ended: {ended:?}")
                    }
                    Err(HandshakeError::Rejected(reject)) => {
                        named_by_gateway = reject
                            .retry_after_secs
                            .map(|secs| Duration::from_secs(u64::from(secs)));
                        status.send_replace(LinkStatus::Refused {
                            update_permit: reject.update_permit.clone(),
                            code: reject.code,
                            detail: reject.detail.clone(),
                        });
                        format!("refused: {:?}", reject.code)
                    }
                    Err(error) => error.to_string(),
                }
            }
        };
        *shared
            .active_endpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        *shared
            .access_token
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        let retry_in = backoff.failed(named_by_gateway);
        let failures = backoff.failures();
        if !matches!(*status.borrow(), LinkStatus::Refused { .. }) {
            status.send_replace(LinkStatus::Waiting {
                failures,
                retry_in,
                why,
            });
        }
        tokio::time::sleep(retry_in).await;
    }
}

/// The last address whose transport was verified, including an authenticated update refusal.
pub fn last_endpoint(data_dir: &std::path::Path) -> std::io::Result<Endpoint> {
    let bytes = std::fs::read(data_dir.join("connection.json"))?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

fn keep_connected_endpoint(data_dir: &std::path::Path, endpoint: &Endpoint) -> std::io::Result<()> {
    use std::io::Write;
    let mut temporary = tempfile::NamedTempFile::new_in(data_dir)?;
    temporary.write_all(&serde_json::to_vec(endpoint).map_err(std::io::Error::other)?)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(data_dir.join("connection.json"))
        .map_err(|error| error.error)?;
    Ok(())
}

/// Forwards the events of a session to the core until it ends.
async fn run_session(established: Established, inputs: &mpsc::UnboundedSender<Input>) -> EndReason {
    let Established {
        sender,
        mut inbox,
        ready,
        clock_offset_ms,
        ..
    } = established;
    let start = SessionStart {
        ready,
        sender,
        clock_offset_ms,
    };
    if inputs.send(Input::Ready(Box::new(start))).is_err() {
        return EndReason::Local;
    }
    loop {
        match inbox.next().await {
            Some(SessionEvent::Frame(frame)) => {
                if inputs.send(Input::Frame(frame)).is_err() {
                    return EndReason::Local;
                }
            }
            Some(SessionEvent::Data(data)) => {
                if inputs.send(Input::Data(data)).is_err() {
                    return EndReason::Local;
                }
            }
            Some(SessionEvent::Datagram(packet)) => {
                if inputs.send(Input::Datagram(packet)).is_err() {
                    return EndReason::Local;
                }
            }
            Some(SessionEvent::Ended(reason)) => {
                let _ = inputs.send(Input::Ended);
                return reason;
            }
            None => {
                let _ = inputs.send(Input::Ended);
                return EndReason::PeerClosed;
            }
        }
    }
}

/// Forces the files of the spool to disk, where blocking is fine.
async fn sync_files(files: Vec<std::fs::File>) {
    if files.is_empty() {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        for file in files {
            let _ = file.sync_data();
        }
    })
    .await;
}

async fn core(
    mut inputs: mpsc::UnboundedReceiver<Input>,
    mut streams: StreamSender,
    commands: CommandServer,
    tunnels: TunnelHub,
    shared: Arc<Shared>,
    clock: Arc<dyn Clock>,
) {
    let mut session: Option<SessionSender> = None;
    let mut ticker = tokio::time::interval(CORE_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks: u32 = 0;
    let mut stall_after = Duration::from_secs(u64::from(HEARTBEAT_SECS));
    let mut shutdown: Option<oneshot::Sender<()>> = None;

    'running: loop {
        let first = tokio::select! {
            input = inputs.recv() => match input {
                Some(input) => Some(input),
                None => break 'running,
            },
            _ = ticker.tick() => None,
        };
        let mut batch: Vec<Input> = first.into_iter().collect();
        while let Ok(more) = inputs.try_recv() {
            batch.push(more);
        }
        let now = Instant::now();
        if batch.is_empty() {
            ticks = ticks.wrapping_add(1);
            streams.on_stall(stall_after, now);
            if ticks.is_multiple_of(HOUSEKEEPING_EVERY) {
                if let Ok(frames) = streams.expire(clock.time(), now) {
                    send_all(session.as_ref(), frames);
                }
                let _ = commands.purge();
            }
        }
        for input in batch {
            match input {
                Input::RawAfter {
                    stream,
                    after,
                    reply,
                } => {
                    let _ = reply.send(streams.raw_after(&stream, after));
                }
                Input::Publish {
                    stream,
                    event,
                    reply,
                } => {
                    let outcome = match (event.body(), event.kind().as_str()) {
                        (Ok(body), kind) => streams.publish(&stream, clock.time(), kind, body, now),
                        (Err(error), _) => Err(PublishError::Spool(crate::spool::SpoolError::Io {
                            path: "event".to_owned(),
                            source: std::io::Error::other(error),
                        })),
                    };
                    match outcome {
                        Ok(frames) => {
                            let _ = reply.send(Ok(()));
                            send_all(session.as_ref(), frames);
                        }
                        Err(error) => {
                            let _ = reply.send(Err(error));
                        }
                    }
                }
                Input::Report { eph } => {
                    if let Some(open) = session.as_ref() {
                        let _ = open.send_events(Frame::Eph(eph));
                    }
                }
                Input::Ready(start) => {
                    let SessionStart {
                        ready,
                        sender,
                        clock_offset_ms,
                    } = *start;
                    *shared
                        .versions
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(ready.versions.clone());
                    stall_after = Duration::from_secs(u64::from(ready.heartbeat_secs.max(1)));
                    *shared
                        .session
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(sender.clone());
                    commands.set_session(Some(sender.clone()), clock_offset_ms);
                    tunnels.session_up(sender.clone());
                    match streams.on_ready(&ready.cursors, sender.max_frame_bytes(), now) {
                        Ok(frames) => send_all(Some(&sender), frames),
                        Err(error) => {
                            tracing::error!(%error, "the streams could not be brought up to date")
                        }
                    }
                    session = Some(sender);
                }
                Input::Frame(frame) => {
                    let frame = *frame;
                    if let Frame::PortConnectionsQuery(query) = frame {
                        let records = shared
                            .port_records
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone();
                        if let (Some(records), Some(sender)) = (records, session.as_ref()) {
                            records.set_limits(&shared.tunnels.port_limits());
                            let connections = records.list(
                                query.environment_id,
                                query.generation,
                                &query.query,
                                true,
                                clock.time(),
                            );
                            let _ = sender.send(Frame::PortConnectionsReply(
                                orch_link_contract::ports::PortConnectionsReply {
                                    request_ref: query.request_ref,
                                    environment_id: query.environment_id,
                                    connections,
                                },
                            ));
                        }
                        continue;
                    }
                    if let Frame::McpImportAnswer(answer) = frame {
                        if let Some(tell) = shared
                            .mcp_imports
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .remove(&answer.request_id)
                        {
                            let _ = tell.send(answer);
                        }
                        continue;
                    }
                    if let Frame::PortLocalAnswer(answer) = frame {
                        if let Some((_, tell)) = shared
                            .local_ports
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .remove(&answer.request_id)
                        {
                            let _ = tell.send(answer);
                        }
                        continue;
                    }
                    dispatch(
                        frame,
                        &mut streams,
                        &commands,
                        &tunnels,
                        session.as_ref(),
                        now,
                    )
                    .await;
                }
                Input::Data(data) => tunnels.on_data(data),
                Input::Datagram(packet) => tunnels.on_datagram(packet),
                Input::Ended => {
                    if let Some(ended) = session.take() {
                        *shared.meter.lock().unwrap_or_else(PoisonError::into_inner) =
                            Some(ended.meter().measurements());
                    }
                    *shared
                        .session
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = None;
                    streams.on_session_down();
                    commands.set_session(None, 0);
                    tunnels.session_down();
                }
                Input::Shutdown(done) => {
                    shutdown = Some(done);
                    break;
                }
            }
        }
        if shutdown.is_some() {
            break 'running;
        }
        if let Some(open) = session.as_ref() {
            pump(&mut streams, open, now).await;
        }
        // Whatever was spooled is forced to disk within a tick, sent or not.
        if streams.is_dirty()
            && let Ok(files) = streams.sync_handles()
        {
            sync_files(files).await;
        }
    }
    if let Some(open) = session.as_ref() {
        open.end();
    }
    if let Ok(files) = streams.sync_handles() {
        sync_files(files).await;
    }
    if let Some(done) = shutdown {
        let _ = done.send(());
    }
}

fn send_all(session: Option<&SessionSender>, frames: Vec<Frame>) {
    if let Some(session) = session {
        for frame in frames {
            let _ = session.send_events(frame);
        }
    }
}

/// Frames what may go on the wire and sends it, the files forced to disk
/// first: nothing is on the wire that is not on disk.
async fn pump(streams: &mut StreamSender, session: &SessionSender, now: Instant) {
    let frames = match streams.pump(session.event_frame_bytes(), now) {
        Ok(frames) => frames,
        Err(error) => {
            tracing::error!(%error, "the spool failed");
            return;
        }
    };
    if frames.is_empty() {
        return;
    }
    if let Ok(files) = streams.sync_handles() {
        sync_files(files).await;
    }
    for frame in frames {
        if session.send_events(frame).is_err() {
            return;
        }
    }
}

async fn dispatch(
    frame: Frame,
    streams: &mut StreamSender,
    commands: &CommandServer,
    tunnels: &TunnelHub,
    session: Option<&SessionSender>,
    now: Instant,
) {
    match frame {
        Frame::Ack(ack) => {
            if let Err(error) = streams.on_ack(&ack, now) {
                tracing::error!(%error, "an acknowledgement could not be applied");
            }
        }
        Frame::Nak(nak) => match streams.on_nak(&nak, now) {
            Ok(frames) => send_all(session, frames),
            Err(error) => tracing::error!(%error, "a request to send again could not be served"),
        },
        Frame::Cmd(cmd) => commands.on_cmd(cmd).await,
        Frame::CmdCancel(cancel) => commands.on_cancel(cancel).await,
        other => tunnels.on_frame(other),
    }
}
