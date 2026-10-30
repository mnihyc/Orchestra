//! Authenticated session handshake and liveness.
//!
//! A session begins with three words. The gateway speaks first and says
//! which protocol versions it has, a nonce that belongs to this connection
//! alone, and how it knows the machine: by the client certificate of the
//! TLS connection, or, where the connection came through an edge that
//! showed no certificate, not yet. The machine answers with who it is, and
//! in the second case proves that it holds the key of its certificate by
//! signing the nonce, itself, the host it connected to and its own clock.
//! The gateway checks all of it, asks the system whether that machine may
//! have a session, and either declares the session ready, with the limits
//! and with how far each stream of the machine was taken in, or refuses it
//! with a code that tells the machine what to do.
//!
//! Once ready, a session is kept alive by whichever side has been silent: it
//! pings after a heartbeat of sending nothing, and a side that has received
//! nothing for three heartbeats gives the session up. A session is never
//! closed for being slow, only for being dead.

use crate::framing::{
    Connection, Inbound, LinkMeter, MessageSink, MessageSource, Outgoing, SessionOver,
    TransportError, queues, read_loop, send_frame, write_loop,
};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use orch_link_contract::backplane::{AUTH_QUERY_TIMEOUT_SECS, AuthQuery, AuthReply};
use orch_link_contract::enrollment::Endpoint;
use orch_link_contract::frames::{DataFrame, Frame, Text, decode};
use orch_link_contract::session::{
    Auth, AuthProof, Bye, HANDSHAKE_STEP_TIMEOUT_SECS, Hello, NONCE_BYTES, Ping, Platform, Pong,
    Ready, Reject, Role, TlsMode, auth_signing_input, clock_agrees, encode_nonce,
};
use orch_link_contract::version::{
    ByeCode, ConnectorVersions, Limits, MAX_FRAME_BYTES, PROTOCOL_VERSION, ProtocolRange,
    RejectCode, Standing,
};
use orch_pki::certificates::{Certificate, LeafKey};
use orch_pki::verification::{ProofFault, verify_machine_proof};
use orch_types::{Clock, Id, new_id};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// The heartbeats of silence after which a side gives the session up. The
/// contract's reading of "a minute" beside "twenty seconds": three.
pub const DEAD_AFTER_HEARTBEATS: u32 = 3;
/// How long the pump of a session waits at most before looking at the
/// clocks again, whatever else it is waiting for.
const MAX_IDLE_LOOK: Duration = Duration::from_secs(1);
/// The delay named in the refusal when the system cannot be asked: the
/// contract's own figure for how soon to try again.
pub const BUSY_RETRY_SECS: u32 = 5;

/// When a session sends pings and when it gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How long a side sends nothing before it pings.
    pub heartbeat: Duration,
    /// How long a side hears nothing before it gives the session up.
    pub dead_after: Duration,
}

impl Timing {
    /// The timing of a heartbeat of `heartbeat_secs`: three of them make a
    /// dead session.
    #[must_use]
    pub fn of_heartbeat(heartbeat_secs: u32) -> Self {
        Self {
            heartbeat: Duration::from_secs(u64::from(heartbeat_secs)),
            dead_after: Duration::from_secs(u64::from(
                heartbeat_secs.saturating_mul(DEAD_AFTER_HEARTBEATS),
            )),
        }
    }
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The other side closed the connection.
    PeerClosed,
    /// The gateway said it ends the session.
    Bye(ByeCode),
    /// Nothing was heard for as long as a side waits.
    Dead,
    /// The other side broke the protocol.
    Protocol(String),
    /// The connection failed.
    Transport(String),
    /// This side ended it.
    Local,
}

/// What happens on a session.
#[derive(Debug)]
pub enum SessionEvent {
    /// A text frame, other than a ping or a pong, which the session
    /// answers by itself. Retains the reader's allocation while it is queued.
    Frame(Box<Frame>),
    /// A frame of tunnel data.
    Data(DataFrame),
    /// A complete leased port datagram, never a stream or replay item.
    Datagram(crate::ports::Datagram),
    /// The session is over. Nothing follows.
    Ended(EndReason),
}

/// The sending end of a session: a cheap handle that may be held anywhere.
#[derive(Debug, Clone)]
pub struct SessionSender {
    out: Outgoing,
    limits: Limits,
    meter: Arc<LinkMeter>,
    ended: CancellationToken,
}

impl SessionSender {
    /// Sends one already leased packet through the shared fair scheduler.
    pub fn send_datagram(&self, packet: crate::ports::Datagram) -> Result<(), SessionOver> {
        self.out.datagram(packet)
    }
    /// Sends a frame that must not wait behind events or data.
    pub fn send(&self, frame: impl Into<Frame>) -> Result<(), SessionOver> {
        self.out.control(&frame.into(), self.limits.frame_bytes())
    }

    /// Sends a frame of events.
    pub fn send_events(&self, frame: impl Into<Frame>) -> Result<(), SessionOver> {
        self.out.events(&frame.into(), self.limits.frame_bytes())
    }

    /// Sends a text frame that must not overtake the tunnel data sent
    /// before it: the end of a stream, a clean close.
    pub fn send_ordered(&self, frame: impl Into<Frame>) -> Result<(), SessionOver> {
        self.out.ordered(&frame.into(), self.limits.frame_bytes())
    }

    /// Sends a frame of tunnel data.
    pub fn send_data(&self, frame: &DataFrame) -> Result<(), SessionOver> {
        self.out.data(frame, self.limits.frame_bytes())
    }

    /// Sends a binary frame that is already written.
    pub fn send_data_bytes(&self, bytes: Bytes) -> Result<(), SessionOver> {
        self.out.data_bytes(bytes)
    }

    /// The limits of this session.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// The most bytes of a frame.
    #[must_use]
    pub fn max_frame_bytes(&self) -> usize {
        self.limits.frame_bytes()
    }

    /// How many bytes of payload the next data frame should carry: a
    /// quarter of a second of what the link has been seen to carry.
    #[must_use]
    pub fn data_frame_bytes(&self) -> usize {
        self.meter
            .data_frame_bytes(DataFrame::max_payload(self.max_frame_bytes()))
    }

    /// How many bytes of events a frame should carry, sized the same way.
    #[must_use]
    pub fn event_frame_bytes(&self) -> usize {
        self.meter.data_frame_bytes(self.max_frame_bytes())
    }

    /// What is seen of the link.
    #[must_use]
    pub const fn meter(&self) -> &Arc<LinkMeter> {
        &self.meter
    }

    /// Says why the gateway ends the session, then ends it.
    pub fn bye(&self, code: ByeCode) {
        let _ = self.send(Bye { code });
        self.ended.cancel();
    }

    /// Ends the session from this side.
    pub fn end(&self) {
        self.ended.cancel();
    }

    /// Whether the session is still there.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.ended.is_cancelled() && !self.out.is_closed()
    }

    /// A signal that comes when the session is over.
    #[must_use]
    pub const fn ended(&self) -> &CancellationToken {
        &self.ended
    }
}

/// The receiving end of a session.
#[derive(Debug)]
pub struct SessionInbox {
    events: mpsc::UnboundedReceiver<SessionEvent>,
}

impl SessionInbox {
    /// The next event; `None` once the session is over and `Ended` was
    /// taken.
    pub async fn next(&mut self) -> Option<SessionEvent> {
        self.events.recv().await
    }
}

/// When a side last sent and received something.
#[derive(Debug)]
struct Activity {
    base: Instant,
    sent: AtomicU64,
    received: AtomicU64,
}

impl Activity {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            sent: AtomicU64::new(0),
            received: AtomicU64::new(0),
        }
    }

    fn millis(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn since(&self, mark: &AtomicU64) -> Duration {
        Duration::from_millis(self.millis().saturating_sub(mark.load(Ordering::Relaxed)))
    }
}

/// Starts the tasks of a session on a connection whose handshake is done.
fn start(
    connection: Connection,
    limits: Limits,
    timing: Timing,
    meter: Arc<LinkMeter>,
) -> (SessionSender, SessionInbox) {
    let Connection { sink, source, .. } = connection;
    let (out, queued) = queues();
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let ended = CancellationToken::new();
    let activity = Arc::new(Activity::new());
    let sender = SessionSender {
        out: out.clone(),
        limits,
        meter: Arc::clone(&meter),
        ended: ended.clone(),
    };

    let writer_failed = inbound_tx.clone();
    let write_meter = Arc::clone(&meter);
    let write_activity = Arc::clone(&activity);
    let write_ended = ended.clone();
    tokio::spawn(async move {
        let counting = ActivitySink {
            inner: sink,
            activity: write_activity,
        };
        if let Err(error) = write_loop(Box::new(counting), queued, write_meter, write_ended).await {
            let _ = writer_failed.send(Inbound::Lost(Some(error)));
        }
    });
    let read_activity = Arc::clone(&activity);
    tokio::spawn(read_loop(
        Box::new(ActivitySource {
            inner: source,
            activity: read_activity,
        }),
        inbound_tx,
        limits.frame_bytes(),
        limits.ports,
        Arc::clone(&meter),
    ));
    tokio::spawn(pump(
        inbound_rx, events_tx, out, limits, timing, meter, activity, ended,
    ));
    (sender, SessionInbox { events: events_rx })
}

/// Notes in an [`Activity`] when something is sent.
struct ActivitySink {
    inner: Box<dyn MessageSink>,
    activity: Arc<Activity>,
}

#[async_trait]
impl MessageSink for ActivitySink {
    async fn ready(&mut self) {
        self.inner.ready().await;
    }

    async fn send(&mut self, message: crate::framing::Message) -> Result<(), TransportError> {
        self.inner.send(message).await?;
        self.activity
            .sent
            .store(self.activity.millis(), Ordering::Relaxed);
        Ok(())
    }

    async fn close(&mut self) {
        self.inner.close().await;
    }
}

/// Notes in an [`Activity`] when something arrives.
struct ActivitySource {
    inner: Box<dyn MessageSource>,
    activity: Arc<Activity>,
}

#[async_trait]
impl MessageSource for ActivitySource {
    async fn recv(&mut self) -> Result<Option<crate::framing::Message>, TransportError> {
        let message = self.inner.recv().await?;
        self.activity
            .received
            .store(self.activity.millis(), Ordering::Relaxed);
        Ok(message)
    }
}

/// Answers pings, measures the round trip, pings when this side has been
/// silent, gives the session up when the other has, and hands everything
/// else on.
#[expect(
    clippy::too_many_arguments,
    reason = "the tasks of a session meet here, and each of these is a different thing they share"
)]
async fn pump(
    mut inbound: mpsc::UnboundedReceiver<Inbound>,
    events: mpsc::UnboundedSender<SessionEvent>,
    out: Outgoing,
    limits: Limits,
    timing: Timing,
    meter: Arc<LinkMeter>,
    activity: Arc<Activity>,
    ended: CancellationToken,
) {
    let max = limits.frame_bytes();
    let mut next_ping: i64 = 1;
    let mut waiting_for: Option<(i64, Instant)> = None;
    let mut last_ping = Instant::now();
    let reason = loop {
        // The clocks are looked at on every turn, not only when nothing
        // arrives: a link that is never quiet must still be pinged, and a
        // dead one given up.
        if activity.since(&activity.received) >= timing.dead_after {
            break EndReason::Dead;
        }
        // A side pings when it has sent nothing for a heartbeat, and, so that
        // the round trip of a link that is never quiet is still known, at
        // least once in each heartbeat.
        if activity.since(&activity.sent) >= timing.heartbeat
            || last_ping.elapsed() >= timing.heartbeat
        {
            let n = next_ping;
            next_ping += 1;
            last_ping = Instant::now();
            waiting_for = Some((n, last_ping));
            let _ = out.control(&Ping { n }.into(), max);
        }
        let until_ping = timing
            .heartbeat
            .saturating_sub(activity.since(&activity.sent))
            .min(timing.heartbeat.saturating_sub(last_ping.elapsed()));
        let until_dead = timing
            .dead_after
            .saturating_sub(activity.since(&activity.received));
        let wait = until_ping.min(until_dead).min(MAX_IDLE_LOOK);
        tokio::select! {
            () = ended.cancelled() => break EndReason::Local,
            item = inbound.recv() => match item {
                None => break EndReason::PeerClosed,
                Some(Inbound::Frame(frame)) => match frame.as_ref() {
                    Frame::Ping(Ping { n }) => {
                        let _ = out.control(&Pong { n: *n }.into(), max);
                    }
                    Frame::Pong(Pong { n }) => {
                        if let Some((expected, at)) = waiting_for.take() {
                            if expected == *n {
                                meter.observe_round_trip(at.elapsed());
                            } else {
                                waiting_for = Some((expected, at));
                            }
                        }
                    }
                    Frame::Bye(Bye { code }) => break EndReason::Bye(*code),
                    _ => {
                        if events.send(SessionEvent::Frame(frame)).is_err() {
                            break EndReason::Local;
                        }
                    }
                },
                Some(Inbound::Unknown(kind)) => tracing::debug!(kind, "a frame of an unknown type was passed over"),
                Some(Inbound::Data(frame)) => {
                    if events.send(SessionEvent::Data(frame)).is_err() {
                        break EndReason::Local;
                    }
                }
                Some(Inbound::Datagram(packet)) => {
                    if events.send(SessionEvent::Datagram(packet)).is_err() {break EndReason::Local;}
                }
                Some(Inbound::Broken(error)) => {
                    let _ = out.control(&Bye { code: error.bye() }.into(), max);
                    break EndReason::Protocol(error.to_string());
                }
                Some(Inbound::Lost(None)) => break EndReason::PeerClosed,
                Some(Inbound::Lost(Some(error))) => break EndReason::Transport(error.to_string()),
            },
            () = tokio::time::sleep(wait) => {}
        }
    };
    ended.cancel();
    let _ = events.send(SessionEvent::Ended(reason));
}

// ---------------------------------------------------------------- the machine's side

/// Who a machine says it is, and what it proves it with.
#[derive(Debug)]
pub struct ClientIdentity {
    /// Owner-selected addresses, when this machine overrides the system.
    pub endpoints: Option<Vec<Endpoint>>,
    /// A node's explicit addresses for hosted units.
    pub hosted_endpoints: Option<Vec<Endpoint>>,

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
}

/// Why a handshake did not end in a session.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// The gateway refused the session.
    #[error("the session was refused: {0:?}")]
    Rejected(Reject),
    /// The gateway ended the session before it was ready.
    #[error("the gateway ended the session: {0:?}")]
    Bye(ByeCode),
    /// The gateway and the machine have no protocol version in common.
    #[error("the gateway speaks protocol versions {min} to {max}")]
    ProtocolUnsupported {
        /// The oldest version the gateway speaks.
        min: u32,
        /// The newest.
        max: u32,
    },
    /// The connection failed.
    #[error("{0}")]
    Transport(#[from] TransportError),
    /// A step took longer than it may.
    #[error("the {0} took too long")]
    TimedOut(&'static str),
    /// The other side said something that is not part of a handshake.
    #[error("{0}")]
    Malformed(String),
    /// The gateway asks for a proof and the machine has no key to give it
    /// with, or does not know which host it connected to.
    #[error("{0}")]
    CannotProve(&'static str),
}

/// A session that was established.
#[derive(Debug)]
pub struct Established {
    /// The gateway's word that the session stands.
    pub ready: Ready,
    /// The protocol version the two speak.
    pub protocol_version: u32,
    /// How the gateway knew the machine.
    pub tls: TlsMode,
    /// How far the machine's clock is ahead of the gateway's, in
    /// milliseconds, as the gateway's `hello` showed.
    pub clock_offset_ms: i64,
    /// The host the machine connected to.
    pub host: Option<String>,
    /// The sending end.
    pub sender: SessionSender,
    /// The receiving end.
    pub inbox: SessionInbox,
}

fn step_timeout() -> Duration {
    Duration::from_secs(u64::from(HANDSHAKE_STEP_TIMEOUT_SECS))
}

async fn read_text(
    source: &mut dyn MessageSource,
    step: &'static str,
) -> Result<Frame, HandshakeError> {
    loop {
        let message = tokio::time::timeout(step_timeout(), source.recv())
            .await
            .map_err(|_| HandshakeError::TimedOut(step))??;
        match message {
            None => return Err(TransportError::Closed.into()),
            Some(crate::framing::Message::Binary(_)) => {
                return Err(HandshakeError::Malformed(
                    "tunnel data before the session was ready".to_owned(),
                ));
            }
            Some(crate::framing::Message::Text(text)) => {
                match decode::<Frame>(&text, MAX_FRAME_BYTES as usize) {
                    Ok(Text::Known(frame)) => return Ok(frame),
                    Ok(Text::Unknown(_)) => {}
                    Err(error) => return Err(HandshakeError::Malformed(error.to_string())),
                }
            }
        }
    }
}

/// Runs a machine's side of the handshake on `connection`, and, if the gateway
/// declares the session ready, starts it.
pub async fn connect(
    mut connection: Connection,
    identity: &ClientIdentity,
    clock: &dyn Clock,
) -> Result<Established, HandshakeError> {
    let max = MAX_FRAME_BYTES as usize;
    let Frame::Hello(hello) = read_text(connection.source.as_mut(), "hello").await? else {
        return Err(HandshakeError::Malformed(
            "the gateway did not begin with hello".to_owned(),
        ));
    };
    let protocol_version =
        hello
            .protocol
            .choose(&[PROTOCOL_VERSION])
            .ok_or(HandshakeError::ProtocolUnsupported {
                min: hello.protocol.min,
                max: hello.protocol.max,
            })?;
    let sent_at = clock.time();
    let host = connection.peer.host.clone();
    let proof = match hello.tls {
        TlsMode::Mutual => None,
        TlsMode::Server => {
            let (key, certificate) = identity.proof.as_ref().ok_or(HandshakeError::CannotProve(
                "the machine has no key to prove itself with",
            ))?;
            let host = host.as_deref().ok_or(HandshakeError::CannotProve(
                "the machine does not know which host it connected to",
            ))?;
            let signed = auth_signing_input(&hello.nonce, identity.machine_id, host, sent_at);
            Some(AuthProof {
                certificate_pem: certificate.pem(),
                signature: URL_SAFE_NO_PAD.encode(key.sign(signed.as_bytes())),
            })
        }
    };
    let auth = Auth {
        endpoints: identity.endpoints.clone(),
        hosted_endpoints: identity.hosted_endpoints.clone(),
        machine_id: identity.machine_id,
        protocol_version,
        connector_version: identity.connector_version.clone(),
        platform: identity.platform,
        roles: identity.roles.clone(),
        sent_at,
        proof,
    };
    send_frame(connection.sink.as_mut(), &auth.into(), max).await?;
    match read_text(connection.source.as_mut(), "ready").await? {
        Frame::Ready(ready) => {
            let limits = ready.limits;
            let timing = Timing::of_heartbeat(ready.heartbeat_secs);
            let offset = clock.time().unix_millis() - hello.server_time.unix_millis();
            let (sender, inbox) = start(connection, limits, timing, LinkMeter::new());
            Ok(Established {
                ready,
                protocol_version,
                tls: hello.tls,
                clock_offset_ms: offset,
                host,
                sender,
                inbox,
            })
        }
        Frame::Reject(reject) => {
            connection.sink.close().await;
            Err(HandshakeError::Rejected(reject))
        }
        Frame::Bye(Bye { code }) => Err(HandshakeError::Bye(code)),
        other => Err(HandshakeError::Malformed(format!(
            "the gateway answered with {} where ready was due",
            orch_link_contract::frames::Tagged::type_name(&other)
        ))),
    }
}

// ---------------------------------------------------------------- the gateway's side

/// The system, as a gateway asks it whether a session may start.
#[async_trait]
pub trait SessionAuthority: Send + Sync {
    /// Whether the machine may have a session, and how far each of its
    /// streams was taken in.
    async fn authorize(&self, query: AuthQuery) -> AuthReply;
}

/// What a gateway needs to run a handshake.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// The gateway's name.
    pub gateway_id: String,
    /// The limits it states to every session.
    pub limits: Limits,
    /// The connector builds it accepts and prefers.
    pub versions: ConnectorVersions,
    /// How often a side pings when silent, in seconds.
    pub heartbeat_secs: u32,
    /// The system's authority, which machine certificates must chain to.
    pub authority: Certificate,
}

/// A session that a gateway accepted.
#[derive(Debug)]
pub struct Accepted {
    /// What the machine said about itself.
    pub auth: Auth,
    /// The session.
    pub session_id: Id,
    /// What the machine presents to the HTTPS endpoints on a public path.
    pub session_token: String,
    /// The SHA-256 of the machine's certificate, in hexadecimal.
    pub fingerprint: String,
    /// How the machine was known.
    pub tls: TlsMode,
    /// Authoritative placement supplied by the system, or unknown with an older system.
    pub placement: Option<String>,
    /// How far each stream of the machine was taken in.
    pub cursors: std::collections::BTreeMap<String, i64>,
    /// The address the machine connected from, where known.
    pub remote_ip: Option<std::net::IpAddr>,
    /// The sending end.
    pub sender: SessionSender,
    /// The receiving end.
    pub inbox: SessionInbox,
}

/// Why a gateway did not accept a session.
#[derive(Debug, thiserror::Error)]
pub enum AcceptError {
    /// The session was refused, and the machine was told.
    #[error("refused with {code:?}: {detail}")]
    Refused {
        /// Why.
        code: RejectCode,
        /// A sentence for the log.
        detail: String,
    },
    /// The connection failed or the machine broke the protocol before it
    /// could be told anything.
    #[error("{0}")]
    Failed(String),
}

impl From<TransportError> for AcceptError {
    fn from(error: TransportError) -> Self {
        Self::Failed(error.to_string())
    }
}

async fn refuse(
    connection: &mut Connection,
    code: RejectCode,
    detail: impl Into<String>,
    retry_after_secs: Option<u32>,
) -> AcceptError {
    refuse_with_update(connection, code, detail, retry_after_secs, None).await
}

async fn refuse_with_update(
    connection: &mut Connection,
    code: RejectCode,
    detail: impl Into<String>,
    retry_after_secs: Option<u32>,
    update_permit: Option<orch_link_contract::session::UpdatePermit>,
) -> AcceptError {
    let detail = detail.into();
    let reject = Reject {
        update_permit,
        code,
        detail: detail.clone(),
        retry_after_secs,
    };
    let _ = send_frame(
        connection.sink.as_mut(),
        &reject.into(),
        MAX_FRAME_BYTES as usize,
    )
    .await;
    connection.sink.close().await;
    AcceptError::Refused { code, detail }
}

fn random_text() -> String {
    let mut bytes = [0u8; NONCE_BYTES];
    rand::fill(&mut bytes[..]);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Runs a gateway's side of the handshake on a connection that was just
/// accepted. The machine is known by the client certificate the connection
/// carries, or, where it carries none, by its signature over what the
/// gateway sent.
#[expect(
    clippy::too_many_lines,
    reason = "the handshake is one sequence of checks, each of which refuses with its own code; splitting it would only hide the order"
)]
pub async fn accept(
    mut connection: Connection,
    gateway: &GatewayConfig,
    authority: &dyn SessionAuthority,
    clock: &dyn Clock,
) -> Result<Accepted, AcceptError> {
    let max = MAX_FRAME_BYTES as usize;
    let nonce_bytes = {
        let mut bytes = [0u8; NONCE_BYTES];
        rand::fill(&mut bytes[..]);
        bytes
    };
    let nonce = encode_nonce(&nonce_bytes);
    let tls = if connection.peer.client_certificate.is_some() {
        TlsMode::Mutual
    } else {
        TlsMode::Server
    };
    let hello = Hello {
        protocol: ProtocolRange::SUPPORTED,
        nonce: nonce.clone(),
        server_time: clock.time(),
        gateway_id: gateway.gateway_id.clone(),
        tls,
    };
    send_frame(connection.sink.as_mut(), &hello.into(), max).await?;
    let auth = match read_text(connection.source.as_mut(), "auth").await {
        Ok(Frame::Auth(auth)) => auth,
        Ok(_) => {
            return Err(AcceptError::Failed(
                "the machine did not answer with auth".to_owned(),
            ));
        }
        Err(error) => return Err(AcceptError::Failed(error.to_string())),
    };
    let now = clock.time();

    if !ProtocolRange::SUPPORTED.contains(auth.protocol_version) {
        return Err(refuse(
            &mut connection,
            RejectCode::ProtocolUnsupported,
            format!(
                "protocol version {} is not spoken here",
                auth.protocol_version
            ),
            None,
        )
        .await);
    }
    let needs_update = !matches!(
        gateway.versions.standing(&auth.connector_version),
        Ok(Standing::Behind | Standing::Current)
    );
    if needs_update
        && !auth
            .roles
            .contains(&orch_link_contract::session::Role::Node)
    {
        return Err(refuse(
            &mut connection,
            RejectCode::UpgradeRequired,
            format!(
                "connectors older than {} are not accepted",
                gateway.versions.min
            ),
            None,
        )
        .await);
    }

    // Who the machine is, by the certificate it is known by.
    let certificate = match tls {
        TlsMode::Mutual => {
            let Some(certificate) = connection.peer.client_certificate.clone() else {
                return Err(refuse(
                    &mut connection,
                    RejectCode::BadProof,
                    "no certificate",
                    None,
                )
                .await);
            };
            if !certificate.valid_at(now) {
                let code = if certificate.not_after() < now {
                    RejectCode::CertificateExpired
                } else {
                    RejectCode::BadProof
                };
                return Err(refuse(
                    &mut connection,
                    code,
                    "the certificate is not valid now",
                    None,
                )
                .await);
            }
            certificate
        }
        TlsMode::Server => {
            let Some(proof) = auth.proof.as_ref() else {
                return Err(refuse(
                    &mut connection,
                    RejectCode::BadProof,
                    "a proof is required",
                    None,
                )
                .await);
            };
            if !clock_agrees(auth.sent_at, now) {
                return Err(refuse(
                    &mut connection,
                    RejectCode::ClockSkew,
                    "the machine's clock is too far from the gateway's",
                    None,
                )
                .await);
            }
            let Some(host) = connection.peer.host.clone() else {
                return Err(refuse(
                    &mut connection,
                    RejectCode::BadProof,
                    "no host was named",
                    None,
                )
                .await);
            };
            let signed = auth_signing_input(&nonce, auth.machine_id, &host, auth.sent_at);
            let signature = URL_SAFE_NO_PAD.decode(&proof.signature).unwrap_or_default();
            match verify_machine_proof(
                &gateway.authority,
                &proof.certificate_pem,
                &signature,
                signed.as_bytes(),
                now,
            ) {
                Ok(certificate) => certificate,
                Err(ProofFault::Expired) => {
                    return Err(refuse(
                        &mut connection,
                        RejectCode::CertificateExpired,
                        "the certificate has expired",
                        None,
                    )
                    .await);
                }
                Err(fault) => {
                    return Err(refuse(
                        &mut connection,
                        RejectCode::BadProof,
                        fault.to_string(),
                        None,
                    )
                    .await);
                }
            }
        }
    };
    if certificate.machine_id() != Some(auth.machine_id) {
        return Err(refuse(
            &mut connection,
            RejectCode::BadProof,
            "the certificate does not name the machine",
            None,
        )
        .await);
    }

    // Whether the system lets that machine in.
    let fingerprint = certificate.fingerprint();
    let ip = connection
        .peer
        .remote_ip
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let query = AuthQuery {
        endpoints: auth.endpoints.clone(),
        hosted_endpoints: auth.hosted_endpoints.clone(),
        n: now.unix_millis(),
        machine_id: auth.machine_id,
        fingerprint: fingerprint.clone(),
        tls,
        ip,
        connector_version: auth.connector_version.clone(),
        roles: auth.roles.clone(),
    };
    let reply = tokio::time::timeout(
        Duration::from_secs(u64::from(AUTH_QUERY_TIMEOUT_SECS)),
        authority.authorize(query),
    )
    .await;
    let reply = match reply {
        Ok(reply) if reply.ok => reply,
        Ok(reply) => {
            let code = reply.code.unwrap_or(RejectCode::UnknownMachine);
            let retry = (code == RejectCode::Busy).then_some(BUSY_RETRY_SECS);
            let permit = if code == RejectCode::UpgradeRequired
                && auth
                    .roles
                    .contains(&orch_link_contract::session::Role::Node)
            {
                reply.update_permit
            } else {
                None
            };
            return Err(refuse_with_update(
                &mut connection,
                code,
                "the system does not let this machine in",
                retry,
                permit,
            )
            .await);
        }
        Err(_) => {
            return Err(refuse(
                &mut connection,
                RejectCode::Busy,
                "the system did not answer",
                Some(BUSY_RETRY_SECS),
            )
            .await);
        }
    };

    if needs_update {
        // A node first proves its current certificate and nonce. The API may then grant an
        // idle update, but this old build still gets no session and no lifecycle commands.
        return Err(refuse_with_update(
            &mut connection,
            RejectCode::UpgradeRequired,
            format!(
                "node builds older than {} are not accepted; waiting for its hosted turns to drain",
                gateway.versions.min
            ),
            None,
            reply.update_permit,
        )
        .await);
    }
    let session_id = new_id();
    let session_token = random_text();
    let placement = reply.placement;
    let cursors = reply.cursors.unwrap_or_default();
    let ready = Ready {
        endpoints: reply.endpoints,
        session_id,
        session_token: session_token.clone(),
        heartbeat_secs: gateway.heartbeat_secs,
        limits: gateway.limits,
        cursors: cursors.clone(),
        versions: gateway.versions.clone(),
    };
    send_frame(connection.sink.as_mut(), &ready.into(), max).await?;
    let remote_ip = connection.peer.remote_ip;
    let (sender, inbox) = start(
        connection,
        gateway.limits,
        Timing::of_heartbeat(gateway.heartbeat_secs),
        LinkMeter::new(),
    );
    Ok(Accepted {
        auth,
        session_id,
        session_token,
        fingerprint,
        tls,
        placement,
        cursors,
        remote_ip,
        sender,
        inbox,
    })
}
