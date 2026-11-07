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
use crate::gateways::PrimaryGateway;
use crate::journal::Journal;
use crate::reconnect::{Backoff, DialError, Dialer, Endpoints};
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

mod auxiliary;

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
    /// The local primary choice and the latest authenticated gateway catalogue.
    pub gateway: PrimaryGateway,
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
    /// The machine's existing key could not be retained for its tunnel carriers.
    #[error("{0}")]
    Identity(#[from] orch_pki::PkiError),
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

#[cfg(all(test, feature = "test-observers"))]
#[allow(clippy::unwrap_used, reason = "observer controls fail by panicking")]
mod input_observer_tests {
    use super::*;

    #[tokio::test]
    async fn actual_input_enqueue_and_batch_selection_keep_unprocessed_tail_and_failed_send_distinct()
     {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let queue = crate::test_observers::QueueGauge::new("machine.inputs");
        let observed = queue.receiver();
        for offset in [0, 4, 8] {
            assert!(
                send_input(
                    &sender,
                    Input::Data(orch_link_contract::frames::DataFrame {
                        channel: 3,
                        offset,
                        payload: bytes::Bytes::from_static(b"data"),
                    }),
                    &queue
                )
                .is_ok()
            );
        }
        let mut batch = vec![receiver.recv().await.unwrap()];
        while let Ok(item) = receiver.try_recv() {
            batch.push(item);
        }
        let mut flights: std::collections::VecDeque<_> = batch
            .iter()
            .map(|item| observed.selected(item.observed_bytes()))
            .collect();
        assert_eq!(queue.snapshot().in_flight_messages, 3);
        assert_eq!(
            queue.snapshot().in_flight_payload_bytes,
            3 * (orch_link_contract::frames::DataFrame::HEADER_BYTES + 4)
        );
        flights.pop_front().unwrap().processed();
        // Core cancellation/shutdown can drop a tail already selected into its local batch.
        drop(flights);
        let ended = queue.snapshot();
        assert_eq!(
            (
                ended.processed,
                ended.sent,
                ended.selected_without_completion
            ),
            (1, 0, 2)
        );
        assert_eq!(
            (
                ended.queued_messages,
                ended.in_flight_messages,
                ended.in_flight_payload_bytes
            ),
            (0, 0, 0)
        );
        drop(receiver);
        assert!(send_input(&sender, Input::Ended, &queue).is_err());
        assert_eq!(queue.snapshot().failed_enqueues, 1);
        drop(observed);
        assert!(queue.snapshot().receiver_closed);
        assert!(!queue.snapshot().inconsistent);
    }
}

#[cfg(feature = "test-observers")]
impl Input {
    fn observed_bytes(&self) -> Option<usize> {
        match self {
            Self::RawAfter { stream, after, .. } => {
                serde_json::to_vec(&(stream.to_string(), after))
                    .ok()
                    .map(|bytes| bytes.len())
            }
            Self::Publish { stream, event, .. } => {
                let body = event.body().ok()?;
                serde_json::to_vec(&(stream.to_string(), event.kind().as_str(), body))
                    .ok()
                    .map(|bytes| bytes.len())
            }
            Self::Report { eph } => serde_json::to_vec(eph).ok().map(|bytes| bytes.len()),
            Self::Ready(start) => serde_json::to_vec(&start.ready)
                .ok()
                .map(|bytes| bytes.len()),
            Self::Frame(frame) => orch_link_contract::frames::encode(frame.as_ref(), usize::MAX)
                .ok()
                .map(|text| text.len()),
            Self::Data(data) => data.encode(usize::MAX).ok().map(|bytes| bytes.len()),
            Self::Datagram(packet) => packet.frame.encode().ok().map(|bytes| bytes.len()),
            // These carry only local control/response handles, no serialized payload.
            Self::Ended | Self::Shutdown(_) => Some(0),
        }
    }
}

#[inline]
fn send_input(
    inputs: &mpsc::UnboundedSender<Input>,
    input: Input,
    #[cfg(feature = "test-observers")] observed: &Arc<crate::test_observers::QueueGauge>,
) -> Result<(), mpsc::error::SendError<Input>> {
    #[cfg(feature = "test-observers")]
    {
        let bytes = input.observed_bytes();
        return observed.send(inputs, input, bytes);
    }
    #[cfg(not(feature = "test-observers"))]
    inputs.send(input)
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
    auxiliary: Arc<auxiliary::Pool>,
    incoming: Mutex<Option<IncomingTunnels>>,
    primary_attaching: Mutex<Option<String>>,
    tunnel_idle_secs: std::sync::atomic::AtomicU64,
    primary_changed: tokio::sync::Notify,
    gateway: Mutex<PrimaryGateway>,
    gateway_id: Mutex<Option<String>>,
    last_primary_gateway: Mutex<Option<String>>,
    gateway_measurements: Mutex<Vec<orch_link_contract::gateways::GatewayMeasurement>>,
    gateway_measured_at: Mutex<Option<orch_types::Time>>,
    advertised_gateways: watch::Sender<Option<Vec<orch_link_contract::gateways::GatewayInstance>>>,
    directory: PathBuf,
    access_token: Mutex<Option<String>>,
    endpoints: Arc<Mutex<Vec<Endpoint>>>,
    follows_system: bool,
    hosted_endpoints: Option<Vec<Endpoint>>,
    active_endpoint: Arc<Mutex<Option<Endpoint>>>,
    advertised: watch::Sender<Option<Vec<Endpoint>>>,
    inputs: mpsc::UnboundedSender<Input>,
    #[cfg(feature = "test-observers")]
    observed_inputs: Arc<crate::test_observers::QueueGauge>,
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

impl Drop for Shared {
    fn drop(&mut self) {
        self.auxiliary.shutdown();
    }
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
    /// Live tunnel-only transports, excluding the one authoritative primary.
    #[must_use]
    pub fn auxiliary_sessions(&self) -> usize {
        self.shared.auxiliary.sessions()
    }
    /// The actual primary gateway, distinct from the entrance URL and auxiliary carriers.
    #[must_use]
    pub fn gateway_id(&self) -> Option<String> {
        self.shared
            .gateway_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Authenticated system catalogue, separate from a primary-only local address override.
    #[must_use]
    pub fn gateway_catalogue(&self) -> Vec<orch_link_contract::gateways::GatewayInstance> {
        self.shared
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .catalogue
            .clone()
    }

    /// Authenticated catalogue updates, including those a local primary override does not use.
    #[must_use]
    pub fn advertised_gateways(
        &self,
    ) -> watch::Receiver<Option<Vec<orch_link_contract::gateways::GatewayInstance>>> {
        self.shared.advertised_gateways.subscribe()
    }

    /// Latest verified entrance measurements; not a permission or admission grant.
    #[must_use]
    pub fn gateway_measurements(&self) -> Vec<orch_link_contract::gateways::GatewayMeasurement> {
        self.shared
            .gateway_measurements
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Last probe time and the connector's own persisted primary choice.
    #[must_use]
    pub fn gateway_status(&self) -> orch_link_contract::gateways::GatewayStatusReport {
        orch_link_contract::gateways::GatewayStatusReport {
            choice: self
                .shared
                .gateway
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .choice
                .clone(),
            measured_at: *self
                .shared
                .gateway_measured_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            measurements: self
                .gateway_measurements()
                .into_iter()
                .filter_map(|mut sample| {
                    sample.endpoint =
                        orch_link_contract::gateways::public_endpoint(&sample.endpoint)?;
                    Some(sample)
                })
                .collect(),
        }
    }

    /// The local operator changed the persisted primary choice. Reconnect explicitly.
    pub fn select_gateway(&self, choice: orch_link_contract::gateways::GatewayChoice) {
        self.shared
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .choice = choice;
        if let Some(sender) = self
            .shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            sender.end();
        }
        self.shared.primary_changed.notify_one();
    }

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
        #[cfg(feature = "test-observers")]
        crate::test_observers::machine_started(config.machine_id);
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
        #[cfg(feature = "test-observers")]
        let observed_inputs = crate::test_observers::QueueGauge::new("machine.inputs");
        let (status_tx, status_rx) = watch::channel(LinkStatus::Starting);
        let budget = Arc::new(tokio::sync::Semaphore::new(
            orch_link_contract::version::MAX_CHANNELS as usize,
        ));
        let tunnels = TunnelHub::with_shared_budget(
            config.tunnels,
            config.tunnel_gauge.clone(),
            Arc::clone(&budget),
        );
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        let auxiliary_identity = ClientIdentity {
            purpose: orch_link_contract::gateways::SessionPurpose::Tunnel,
            gateway_id: None,
            machine_id: config.machine_id,
            connector_version: config.connector_version.clone(),
            platform: config.platform,
            roles: config.roles.clone(),
            proof: config
                .proof
                .as_ref()
                .map(|(key, certificate)| key.duplicate().map(|key| (key, certificate.clone())))
                .transpose()?,
            endpoints: None,
            hosted_endpoints: None,
        };
        let (advertised, _) = watch::channel(None);
        let (advertised_gateways, _) = watch::channel(None);
        let endpoints_now = Arc::new(Mutex::new(config.endpoints.clone()));
        let active_endpoint = Arc::new(Mutex::new(None));
        let shared = Arc::new_cyclic(|parent| Shared {
            auxiliary: auxiliary::Pool::new(
                parent.clone(),
                auxiliary_identity,
                Arc::clone(&dialer),
                Arc::clone(&clock),
                config.tunnels,
                config.tunnel_gauge.clone(),
                Arc::clone(&budget),
                incoming_tx.clone(),
            ),
            incoming: Mutex::new(Some(IncomingTunnels::from_receiver(incoming_rx))),
            primary_attaching: Mutex::new(None),
            tunnel_idle_secs: std::sync::atomic::AtomicU64::new(
                orch_link_contract::gateways::TUNNEL_IDLE_SECS,
            ),
            primary_changed: tokio::sync::Notify::new(),
            gateway: Mutex::new(config.gateway),
            gateway_id: Mutex::new(None),
            last_primary_gateway: Mutex::new(None),
            gateway_measurements: Mutex::new(Vec::new()),
            gateway_measured_at: Mutex::new(None),
            advertised_gateways,
            directory: config.data_dir.clone(),
            access_token: Mutex::new(None),
            follows_system: config.follow_system_endpoints,
            hosted_endpoints: config.hosted_endpoints.clone(),
            endpoints: Arc::clone(&endpoints_now),
            active_endpoint: Arc::clone(&active_endpoint),
            advertised: advertised.clone(),
            inputs: inputs.clone(),
            #[cfg(feature = "test-observers")]
            observed_inputs: Arc::clone(&observed_inputs),
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
        shared.auxiliary.start_idle();
        let mut primary_incoming = tunnels.incoming();
        tokio::spawn(async move {
            while let Some(tunnel) = primary_incoming.next().await {
                if incoming_tx.send(tunnel).is_err() {
                    return;
                }
            }
        });
        let identity = ClientIdentity {
            purpose: Default::default(),
            gateway_id: None,
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
            #[cfg(feature = "test-observers")]
            observed_inputs.receiver(),
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
            let _ = send_input(
                &self.shared.inputs,
                Input::Publish {
                    stream: *stream,
                    event: Box::new(event.clone()),
                    reply,
                },
                #[cfg(feature = "test-observers")]
                &self.shared.observed_inputs,
            );
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
        let _ = send_input(
            &self.shared.inputs,
            Input::RawAfter {
                stream: *stream,
                after,
                reply,
            },
            #[cfg(feature = "test-observers")]
            &self.shared.observed_inputs,
        );
        answer.await.unwrap_or(Ok(()))
    }

    /// Sends a measurement. It is not kept: while no session stands it is
    /// dropped.
    pub fn report(&self, stream: &Stream, at: orch_types::Time, report: &Report) {
        if let Ok(eph) = Eph::of(stream, at, report) {
            let _ = send_input(
                &self.shared.inputs,
                Input::Report { eph },
                #[cfg(feature = "test-observers")]
                &self.shared.observed_inputs,
            );
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
        let opened = self.shared.tunnels.open(ticket).await;
        if matches!(
            &opened,
            Err(OpenError::Refused {
                code: orch_link_contract::tunnels::OpenErrorCode::PeerOffline,
                ..
            })
        ) && !self
            .shared
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .catalogue
            .is_empty()
        {
            return self.shared.auxiliary.open(ticket, None, None).await;
        }
        opened
    }

    /// Open on the API-designated pair carrier; the signed capability is unchanged.
    pub async fn open_routed_tunnel(
        &self,
        ticket: &str,
        route: &orch_link_contract::gateways::TunnelPlan,
    ) -> Result<Tunnel, OpenError> {
        self.shared.auxiliary.open(ticket, Some(route), None).await
    }

    /// Shared adapter for existing signed-string consumers and new adjacent plans.
    pub async fn open_tunnel_with_route(
        &self,
        ticket: &str,
        route: Option<&orch_link_contract::gateways::TunnelPlan>,
    ) -> Result<Tunnel, OpenError> {
        match route {
            Some(route) => self.open_routed_tunnel(ticket, route).await,
            None => self.open_tunnel(ticket).await,
        }
    }

    /// Keep the shared transport while the existing session binding needs these plans.
    pub fn retain_tunnel_plans(
        &self,
        plans: Vec<orch_link_contract::gateways::TunnelPlan>,
    ) -> Arc<crate::gateways::TunnelLease> {
        self.shared.auxiliary.lease(plans)
    }

    /// Opens one accepted host socket under this node's publication template.
    pub async fn open_host_port(
        &self,
        ticket: &str,
        peer: orch_link_contract::ports::HostPortPeer,
    ) -> Result<Tunnel, OpenError> {
        if !*self.shared.tunnels.port_control().borrow() {
            return Err(OpenError::NoSession);
        }
        let opened = self
            .shared
            .tunnels
            .open_host_port(ticket, peer.clone())
            .await;
        if matches!(
            &opened,
            Err(OpenError::Refused {
                code: orch_link_contract::tunnels::OpenErrorCode::PeerOffline,
                ..
            })
        ) && !self
            .shared
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .catalogue
            .is_empty()
        {
            return self.shared.auxiliary.open(ticket, None, Some(peer)).await;
        }
        opened
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
        self.shared
            .incoming
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| IncomingTunnels::from_receiver(mpsc::unbounded_channel().1))
    }

    /// Joins cancelled local captures and publications before a graceful physical-program stop.
    pub async fn cancel_recordings(&self) {
        self.shared.commands.cancel_recordings().await;
    }

    /// Ends the session, forces the spool to disk and stops.
    pub async fn shutdown(&self) {
        self.shared.auxiliary.shutdown();
        let (done, finished) = oneshot::channel();
        if send_input(
            &self.shared.inputs,
            Input::Shutdown(done),
            #[cfg(feature = "test-observers")]
            &self.shared.observed_inputs,
        )
        .is_ok()
        {
            let _ = finished.await;
        }
    }
}

/// Keeps a session going: dials, runs the handshake, hands the session to the
/// core, and, when it ends, waits and begins again.
async fn supervise(
    mut identity: ClientIdentity,
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
        let policy = shared
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        endpoints.replace(
            shared
                .endpoints
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        );
        let attempt = tokio::select! {
            () = shared.primary_changed.notified() => continue,
            attempt = establish_primary(
                &mut identity,
                &policy,
                &endpoints,
                dialer.as_ref(),
                clock.as_ref(),
                &shared,
            ) => attempt,
        };
        let mut named_by_gateway = None;
        let why = match attempt {
            Ok((established, endpoint)) => {
                let previous = shared
                    .last_primary_gateway
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .replace(established.gateway_id.clone());
                if previous
                    .as_ref()
                    .is_some_and(|previous| *previous != established.gateway_id)
                {
                    // Cross-gateway resumption is deferred; preserve the ordinary reset/recovery.
                    shared.tunnels.reset_all();
                }
                endpoints.worked(&endpoint);
                *shared
                    .gateway_id
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(established.gateway_id.clone());
                *shared
                    .primary_attaching
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
                shared.tunnel_idle_secs.store(
                    established.ready.tunnel_session_idle_secs,
                    std::sync::atomic::Ordering::Relaxed,
                );
                *shared
                    .access_token
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) =
                    Some(established.ready.session_token.clone());
                *shared
                    .active_endpoint
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(endpoint.clone());
                if !established.ready.gateways.is_empty() {
                    let catalogue = established.ready.gateways.clone();
                    shared
                        .gateway
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .catalogue = catalogue.clone();
                    shared.advertised_gateways.send_replace(Some(catalogue));
                }
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
                        // Preserve the successful entrance for tie breaking only.
                        endpoints.replace(listed.clone());
                    }
                }
                backoff.succeeded();
                status.send_replace(LinkStatus::Connected {
                    session_id: established.ready.session_id,
                    endpoint: endpoint.url.clone(),
                });
                let ended = run_session(
                    established,
                    &inputs,
                    &shared.primary_changed,
                    #[cfg(feature = "test-observers")]
                    &shared.observed_inputs,
                )
                .await;
                format!("the session ended: {ended:?}")
            }
            Err(PrimaryFailure::Selection(detail)) => detail,
            Err(PrimaryFailure::Dial(failures)) => failures
                .into_iter()
                .map(|(endpoint, error)| format!("{}: {error}", endpoint.url))
                .collect::<Vec<_>>()
                .join("; "),
            Err(PrimaryFailure::Handshake(HandshakeError::Rejected(reject))) => {
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
            Err(PrimaryFailure::Handshake(error)) => error.to_string(),
        };
        *shared
            .active_endpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        *shared
            .primary_attaching
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        *shared
            .gateway_id
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
        tokio::select! {
            () = shared.primary_changed.notified() => {},
            () = tokio::time::sleep(retry_in) => {},
        }
    }
}

enum PrimaryFailure {
    Selection(String),
    Dial(Vec<(Endpoint, DialError)>),
    Handshake(HandshakeError),
}

async fn establish_primary(
    identity: &mut ClientIdentity,
    policy: &PrimaryGateway,
    endpoints: &Endpoints,
    dialer: &dyn Dialer,
    clock: &dyn Clock,
    shared: &Shared,
) -> Result<(Established, Endpoint), PrimaryFailure> {
    let (candidates, measurements) = policy
        .candidates(dialer, endpoints.list(), endpoints.last_worked())
        .await
        .map_err(PrimaryFailure::Selection)?;
    let measured_at = (!measurements.is_empty()).then(|| clock.time());
    *shared
        .gateway_measurements
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = measurements;
    *shared
        .gateway_measured_at
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = measured_at;
    let mut failures = Vec::new();
    let mut busy = None;
    for candidate in candidates {
        if let Some(id) = &candidate.gateway_id {
            *shared
                .primary_attaching
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(id.clone());
            shared.auxiliary.close_gateway(id).await;
        }
        let endpoint = candidate.endpoint;
        let connection = match dialer.dial(&endpoint).await {
            Ok(connection) => connection,
            Err(error) => {
                failures.push((endpoint, error));
                continue;
            }
        };
        identity.gateway_id = candidate.gateway_id;
        if let Err(error) = keep_connected_endpoint(&shared.directory, &endpoint) {
            tracing::warn!(%error, "the verified connection address could not be saved");
        }
        match connect(connection, identity, clock).await {
            Ok(established) => return Ok((established, endpoint)),
            Err(HandshakeError::Rejected(reject)) if reject.code == RejectCode::Busy => {
                busy = Some(reject);
            }
            // A refused identity/update must not be retried through another gateway.
            Err(error @ HandshakeError::Rejected(_)) => {
                return Err(PrimaryFailure::Handshake(error));
            }
            Err(error) => failures.push((endpoint, DialError::Connect(error.to_string()))),
        }
    }
    if let Some(reject) = busy {
        return Err(PrimaryFailure::Handshake(HandshakeError::Rejected(reject)));
    }
    Err(PrimaryFailure::Dial(failures))
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
async fn run_session(
    established: Established,
    inputs: &mpsc::UnboundedSender<Input>,
    primary_changed: &tokio::sync::Notify,
    #[cfg(feature = "test-observers")] observed: &Arc<crate::test_observers::QueueGauge>,
) -> EndReason {
    let Established {
        sender,
        mut inbox,
        ready,
        clock_offset_ms,
        ..
    } = established;
    let operator_sender = sender.clone();
    let start = SessionStart {
        ready,
        sender,
        clock_offset_ms,
    };
    if send_input(
        inputs,
        Input::Ready(Box::new(start)),
        #[cfg(feature = "test-observers")]
        observed,
    )
    .is_err()
    {
        return EndReason::Local;
    }
    loop {
        let event = tokio::select! {
            event = inbox.next() => event,
            () = primary_changed.notified() => { operator_sender.end(); continue; },
        };
        match event {
            Some(SessionEvent::Frame(frame)) => {
                if send_input(
                    inputs,
                    Input::Frame(frame),
                    #[cfg(feature = "test-observers")]
                    observed,
                )
                .is_err()
                {
                    return EndReason::Local;
                }
            }
            Some(SessionEvent::Data(data)) => {
                if send_input(
                    inputs,
                    Input::Data(data),
                    #[cfg(feature = "test-observers")]
                    observed,
                )
                .is_err()
                {
                    return EndReason::Local;
                }
            }
            Some(SessionEvent::Datagram(packet)) => {
                if send_input(
                    inputs,
                    Input::Datagram(packet),
                    #[cfg(feature = "test-observers")]
                    observed,
                )
                .is_err()
                {
                    return EndReason::Local;
                }
            }
            Some(SessionEvent::Ended(reason)) => {
                let _ = send_input(
                    inputs,
                    Input::Ended,
                    #[cfg(feature = "test-observers")]
                    observed,
                );
                return reason;
            }
            None => {
                let _ = send_input(
                    inputs,
                    Input::Ended,
                    #[cfg(feature = "test-observers")]
                    observed,
                );
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
    #[cfg(feature = "test-observers")] observed: crate::test_observers::ReceiverLifetime,
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
        #[cfg(feature = "test-observers")]
        let mut flights: std::collections::VecDeque<_> = batch
            .iter()
            .map(|input| observed.selected(input.observed_bytes()))
            .collect();
        while let Ok(more) = inputs.try_recv() {
            #[cfg(feature = "test-observers")]
            flights.push_back(observed.selected(more.observed_bytes()));
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
            #[cfg(feature = "test-observers")]
            let flight = flights.pop_front();
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
                    if let Frame::Gateways(advertisement) = frame {
                        shared
                            .gateway
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .catalogue = advertisement.gateways.clone();
                        shared
                            .advertised_gateways
                            .send_replace(Some(advertisement.gateways));
                        if shared.follows_system {
                            *shared
                                .endpoints
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner) =
                                advertisement.endpoints.clone();
                        }
                        shared
                            .advertised
                            .send_replace(Some(advertisement.endpoints));
                        continue;
                    }
                    if shared.auxiliary.control(&frame) {
                        continue;
                    }
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
                        #[cfg(feature = "test-observers")]
                        if let Some(flight) = flight {
                            flight.processed();
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
                        #[cfg(feature = "test-observers")]
                        if let Some(flight) = flight {
                            flight.processed();
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
                        #[cfg(feature = "test-observers")]
                        if let Some(flight) = flight {
                            flight.processed();
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
                    #[cfg(feature = "test-observers")]
                    if let Some(flight) = flight {
                        flight.processed();
                    }
                    break;
                }
            }
            #[cfg(feature = "test-observers")]
            if let Some(flight) = flight {
                flight.processed();
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
