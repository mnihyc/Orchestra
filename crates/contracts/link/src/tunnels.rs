//! Tickets, channels, flow control and resume messages.
//!
//! A tunnel is an ordered stream of bytes between two machines, or
//! between the system and a machine, which the gateway relays. Who may
//! open what is decided by the system alone: it signs a ticket, and a
//! gateway opens a channel for whoever presents one that names them.
//!
//! Each end of a channel sends only as much as the other end has given
//! it credit for, and the other end gives more as its consumer takes the
//! bytes, so that a consumer that stops reading stops the sender and
//! fills nobody's memory. Each end also keeps what it sent until the
//! other end has acknowledged it. When a session drops, both ends hold
//! their local sockets open for a while; the end that opened the tunnel
//! then resumes it on its new session, each side says how much it has,
//! and each sends again from there. A program at either end of a tunnel
//! never learns that a link was lost.
//!
//! [`SendWindow`] and [`RecvWindow`] are that arithmetic for one
//! direction of one channel. They hold no bytes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::time::Duration;

/// PTY output starts with this many bytes: an unsigned big-endian replay byte count, followed by
/// exactly that many kept bytes and then live output. Input remains the ordinary byte stream.
pub const PTY_REPLAY_HEADER_BYTES: usize = 8;

/// How many bytes an end keeps of what it sent and the other end has not
/// acknowledged. At the limit it stops reading from its local side.
/// Eight mebibytes: several seconds of a fast link, and a bound on what
/// a resumption can ask for.
pub const REPLAY_BUFFER_BYTES: u64 = 8 * 1024 * 1024;
/// How long both ends keep their local sockets open after a session
/// dropped, waiting for the tunnel to be resumed. A minute covers a
/// reconnect with its pauses, and is short enough that a program is not
/// left talking to nobody.
pub const RESUME_GRACE_SECS: u32 = 60;
/// An end acknowledges what it received at least this often in bytes,
/// so that the other end's kept copy stays far below its limit.
pub const ACK_EVERY_BYTES: u64 = 256 * 1024;
/// An end acknowledges what it received at least this often while data
/// flows.
pub const ACK_EVERY: Duration = Duration::from_secs(1);
/// How long a destination may take to answer an opening before the
/// opener is told that its peer is offline.
pub const OPEN_TIMEOUT_SECS: u32 = 10;
/// How long a channel that carries one exchange may sit without a byte
/// before it is closed.
pub const EXCHANGE_IDLE_SECS: u32 = 600;
/// How much of a shell's last output its connector keeps while nobody
/// is attached, and sends first to whoever attaches.
pub const PTY_REPLAY_BYTES: u64 = 256 * 1024;
/// The fewest bytes a data frame carries when there are that many to
/// send: below this the frame's own header starts to count.
pub const MIN_DATA_FRAME_BYTES: usize = 4 * 1024;
/// The share of a second of the link's measured rate that one data
/// frame is sized to, so that a control frame behind it waits about
/// that long at most.
pub const DATA_FRAME_SECONDS_DIVISOR: u64 = 4;
/// The version every ticket begins with.
pub const TICKET_VERSION: &str = "v1";
/// What stands for the system where a ticket names who opens a tunnel.
pub const API_SOURCE: &str = "api";

/// What a tunnel carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    /// A harness's connection to its executor: whole messages, each
    /// framed inside the stream as [`ExecMessage`].
    Exec,
    /// One model request and its answer.
    Llm,
    /// One request to a tool server and its answer.
    Mcp,
    /// A terminal's raw bytes, both ways.
    Pty,
    /// A browser's framed pictures and input, shared by every viewer.
    Browser,
    /// One display viewer's raw RFB connection.
    Rfb,
    /// An admitted environment port, carrying TCP or datagrams.
    Port,
}

impl TunnelKind {
    /// Stable wire/measurement category, with no channel or object identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exec => "exec",
            Self::Llm => "llm",
            Self::Mcp => "mcp",
            Self::Pty => "pty",
            Self::Browser => "browser",
            Self::Rfb => "rfb",
            Self::Port => "port",
        }
    }
    /// How long a channel of this kind may sit without a byte before it
    /// is closed; `None` for one that may be silent as long as it likes.
    #[must_use]
    pub const fn idle_secs(self) -> Option<u32> {
        match self {
            Self::Llm | Self::Mcp => Some(EXCHANGE_IDLE_SECS),
            Self::Exec | Self::Pty | Self::Browser | Self::Rfb | Self::Port => None,
        }
    }
}

/// What a tunnel ends at on its destination.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Service {
    /// The VNC connection of one display viewer.
    Rfb {
        /// The display.
        display_id: Id,
    },
    /// The connection to a known browser.
    Browser {
        /// The browser.
        browser_id: Id,
    },
    /// An executor of an environment, by its kind.
    Exec {
        /// The executor kind.
        executor: String,
    },
    /// The forwarder, for the requests of one lease.
    Llm {
        /// The lease.
        lease_id: Id,
    },
    /// A tool server of an environment, by its name.
    Mcp {
        /// The server's name.
        server: String,
    },
    /// A person's shell.
    Pty {
        /// The shell.
        terminal_id: Id,
    },
    /// A harness's own terminal.
    AgentPty {
        /// The thread whose harness it is.
        thread_id: Id,
    },
    /// A short-lived admitted environment port use.
    Port {
        /// The admitted use, distinct from its durable opening.
        use_id: Id,
    },
}

impl Service {
    /// The kind of tunnel that reaches it.
    #[must_use]
    pub const fn kind(&self) -> TunnelKind {
        match self {
            Self::Browser { .. } => TunnelKind::Browser,
            Self::Rfb { .. } => TunnelKind::Rfb,
            Self::Exec { .. } => TunnelKind::Exec,
            Self::Llm { .. } => TunnelKind::Llm,
            Self::Mcp { .. } => TunnelKind::Mcp,
            Self::Pty { .. } | Self::AgentPty { .. } => TunnelKind::Pty,
            Self::Port { .. } => TunnelKind::Port,
        }
    }
}

impl std::fmt::Display for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Browser { browser_id } => write!(f, "browser/{browser_id}"),
            Self::Rfb { display_id } => write!(f, "rfb/{display_id}"),
            Self::Exec { executor } => write!(f, "exec/{executor}"),
            Self::Llm { lease_id } => write!(f, "llm/{lease_id}"),
            Self::Mcp { server } => write!(f, "mcp/{server}"),
            Self::Pty { terminal_id } => write!(f, "pty/{terminal_id}"),
            Self::AgentPty { thread_id } => write!(f, "agent-pty/{thread_id}"),
            Self::Port { use_id } => write!(f, "port/{use_id}"),
        }
    }
}

/// A text that names no service.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not the name of a service")]
pub struct NotAService(pub String);

impl std::str::FromStr for Service {
    type Err = NotAService;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || NotAService(text.to_owned());
        let (kind, name) = text
            .split_once('/')
            .filter(|(_, name)| !name.is_empty())
            .ok_or_else(bad)?;
        let id = || {
            name.parse::<Id>()
                .ok()
                .filter(|id| id.to_string() == name)
                .ok_or_else(bad)
        };
        match kind {
            "exec" => Ok(Self::Exec {
                executor: name.to_owned(),
            }),
            "llm" => Ok(Self::Llm { lease_id: id()? }),
            "mcp" => Ok(Self::Mcp {
                server: name.to_owned(),
            }),
            "browser" => Ok(Self::Browser { browser_id: id()? }),
            "rfb" => Ok(Self::Rfb { display_id: id()? }),
            "pty" => Ok(Self::Pty { terminal_id: id()? }),
            "agent-pty" => Ok(Self::AgentPty { thread_id: id()? }),
            "port" => Ok(Self::Port { use_id: id()? }),
            _ => Err(bad()),
        }
    }
}

/// Who opens a tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TicketSource {
    /// The system itself, for a tunnel whose other end is a browser.
    Api,
    /// A machine.
    Machine(Id),
}

impl std::fmt::Display for TicketSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api => f.write_str(API_SOURCE),
            Self::Machine(id) => id.fmt(f),
        }
    }
}

impl Serialize for TicketSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TicketSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        if text == API_SOURCE {
            return Ok(Self::Api);
        }
        text.parse()
            .map(Self::Machine)
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for TicketSource {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TicketSource".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string" })
    }
}

/// What a ticket allows: who may open a tunnel, to which machine, to
/// which service there, and until when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketClaims {
    /// The ticket's own identifier.
    pub jti: Id,
    /// The kind of tunnel.
    pub kind: TunnelKind,
    /// Who may open it: the ticket is good for nobody else.
    pub src: TicketSource,
    /// The machine it ends at.
    pub dst: Id,
    /// The service it ends at there.
    pub service: String,
    /// The thread it serves, where it serves one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// The tool set the executor lists on every channel of this ticket,
    /// and no other; present for an executor that has several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_set: Option<String>,
    /// The environment whose executor or tool server this ticket may reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_environment_id: Option<Id>,
    /// The generation of that environment when the tool ticket was issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_environment_generation: Option<i64>,
    /// Port admission facts, absent from every other tunnel kind.
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub port: Option<crate::ports::PortTicketClaims>,
    /// Until when channels may be opened with it, in seconds since 1970.
    pub exp: i64,
}

impl TicketClaims {
    /// Whether the ticket lets `presenter` open a channel at `now`, and
    /// with which answer it is refused when it does not.
    pub fn check(&self, presenter: TicketSource, now: Time) -> Result<(), OpenErrorCode> {
        const SECOND_MS: i64 = 1_000;
        if now.unix_millis() >= self.exp.saturating_mul(SECOND_MS) {
            return Err(OpenErrorCode::Expired);
        }
        let names_its_kind = self
            .service
            .parse::<Service>()
            .is_ok_and(|service| service.kind() == self.kind);
        let port_holds = match (&self.kind, &self.port) {
            (TunnelKind::Port, Some(port)) => {
                port.port > 0
                    && port.generation >= 0
                    && self.service.parse::<Service>().is_ok_and(
                        |service| matches!(service,Service::Port {use_id} if use_id==self.jti),
                    )
            }
            (TunnelKind::Port, None) | (_, Some(_)) => false,
            (_, None) => true,
        };
        if self.src != presenter || !names_its_kind || !port_holds {
            return Err(OpenErrorCode::BadTicket);
        }
        Ok(())
    }
}

/// Why a text is not a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    /// It is not three parts joined by dots, the first being the
    /// version.
    #[error("a ticket is `v1`, its claims and its signature, joined by dots")]
    Shape,
    /// A part is not unpadded base64url.
    #[error("a part of the ticket is not base64url")]
    Encoding,
    /// The claims are not what a ticket states.
    #[error("the claims of the ticket cannot be read")]
    Claims,
}

/// A ticket, taken apart. Whether its signature holds is for whoever
/// has the system's public key to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    signed: String,
    claims: TicketClaims,
    signature: Vec<u8>,
}

impl Ticket {
    /// The text that is signed for `claims`: the version and the claims,
    /// joined by a dot.
    pub fn signing_input(claims: &TicketClaims) -> Result<String, serde_json::Error> {
        Ok(format!(
            "{TICKET_VERSION}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims)?)
        ))
    }

    /// The ticket: what was signed, and the signature over its bytes.
    #[must_use]
    pub fn assemble(signing_input: &str, signature: &[u8]) -> String {
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    /// Takes a ticket apart.
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        let (signed, signature) = text.rsplit_once('.').ok_or(TicketError::Shape)?;
        let claims = signed
            .strip_prefix(TICKET_VERSION)
            .and_then(|rest| rest.strip_prefix('.'))
            .filter(|claims| !claims.contains('.'))
            .ok_or(TicketError::Shape)?;
        let claims = URL_SAFE_NO_PAD
            .decode(claims)
            .map_err(|_| TicketError::Encoding)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| TicketError::Encoding)?;
        let claims = serde_json::from_slice(&claims).map_err(|_| TicketError::Claims)?;
        Ok(Self {
            signed: signed.to_owned(),
            claims,
            signature,
        })
    }

    /// The bytes the signature is over.
    #[must_use]
    pub fn signed(&self) -> &[u8] {
        self.signed.as_bytes()
    }

    /// The signature.
    #[must_use]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    /// What the ticket allows, if its signature holds.
    #[must_use]
    pub const fn claims(&self) -> &TicketClaims {
        &self.claims
    }
}

/// Why a channel was not opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenErrorCode {
    /// The ticket is not one, is not signed by the system, or is not
    /// its presenter's.
    BadTicket,
    /// The ticket is past its time.
    Expired,
    /// The machine at the other end is not connected, or did not answer
    /// in time.
    PeerOffline,
    /// The machine at the other end has no such service.
    NoSuchService,
    /// The machine at the other end will not open it.
    Refused,
    /// A session has as many channels as it may.
    TooManyChannels,
}

/// How a channel ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloseCode {
    /// As it should.
    Ok,
    /// Something failed at one end.
    Error,
    /// It could not be resumed: the wait ran out, or an end no longer
    /// held the bytes the other needed. The local sockets are closed.
    Reset,
}

/// The opening of a channel: the request of whoever opens it, or what
/// the gateway makes of that for the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Open {
    /// From the opener to the gateway.
    Request(OpenRequest),
    /// From the gateway to the destination, once the ticket held.
    Offer(Box<OpenOffer>),
}

/// A request to open a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenRequest {
    /// The number the opener gives the channel on its session.
    #[serde(rename = "c")]
    pub channel: u32,
    /// The ticket that allows it.
    pub ticket: String,
    /// The accepting node's observed socket, allowed only with a signed host publication template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_peer: Option<crate::ports::HostPortPeer>,
}

/// A channel offered to its destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenOffer {
    /// The number the gateway gives the channel on the destination's
    /// session.
    #[serde(rename = "c")]
    pub channel: u32,
    /// The kind of tunnel.
    pub kind: TunnelKind,
    /// The service it is to end at.
    pub service: String,
    /// Who opens it.
    pub from: TicketSource,
    /// What the ticket allows.
    pub claims: TicketClaims,
}

impl Open {
    /// The channel number.
    #[must_use]
    pub const fn channel(&self) -> u32 {
        match self {
            Self::Request(request) => request.channel,
            Self::Offer(offer) => offer.channel,
        }
    }
}

/// The word that a channel is open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenOk {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// The name the tunnel keeps across sessions, by which it is
    /// resumed.
    pub cid: Id,
    /// Actual inside peer for a port channel, absent on other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<orch_api_contract::ports::PortConnectionInside>,
}

/// The word that a channel was not opened.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct OpenErr {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// Why.
    pub code: OpenErrorCode,
    /// A sentence for whoever reads the log.
    #[serde(default)]
    pub detail: String,
}

/// More credit: the receiver has room for this many more bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Credit {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// How many more bytes may be sent.
    pub n: u64,
}

/// The end of one direction of a channel: nothing more comes this way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Eof {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
}

/// The end of a channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Close {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// How it ended.
    pub code: CloseCode,
    /// A sentence for whoever reads the log.
    #[serde(default)]
    pub detail: String,
}

/// An acknowledgement of bytes received on a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ChAck {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// How many bytes of this direction were received in all.
    #[serde(rename = "o")]
    pub offset: u64,
}

/// The opener's request to carry a tunnel on from a new session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Resume {
    /// The number the channel has on the new session.
    #[serde(rename = "c")]
    pub channel: u32,
    /// A ticket that allows the tunnel.
    pub ticket: String,
    /// The name the tunnel was given when it was opened.
    pub jti_channel: Id,
    /// How many bytes the sender of this frame has received in all.
    pub recv: u64,
}

/// The destination's word that a tunnel goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ResumeOk {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// How many bytes the sender of this frame has received in all.
    pub recv: u64,
}

/// A terminal's new size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PtyResize {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// The width of the screen, in characters.
    pub cols: u16,
    /// The height of the screen, in lines.
    pub rows: u16,
}

/// The end of the process in a terminal, sent before the channel is
/// closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PtyExit {
    /// The channel.
    #[serde(rename = "c")]
    pub channel: u32,
    /// What the process exited with.
    pub code: i64,
}

/// An end of a session, as far as channel numbers go. Each end numbers
/// the channels it opens, and the two never choose the same number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The machine: odd numbers.
    Machine,
    /// The gateway: even numbers.
    Gateway,
}

impl Side {
    /// The first number this side gives a channel.
    #[must_use]
    pub const fn first(self) -> u32 {
        match self {
            Self::Machine => 1,
            Self::Gateway => 2,
        }
    }

    /// Whether `channel` is a number this side gives.
    #[must_use]
    pub const fn owns(self, channel: u32) -> bool {
        channel != 0 && (channel % 2 == 1) == matches!(self, Self::Machine)
    }

    /// The number this side tries after `channel`. Numbers come round
    /// again after the last; whoever asks leaves out those still in use.
    #[must_use]
    pub const fn next(self, channel: u32) -> u32 {
        match channel.checked_add(2) {
            Some(next) if self.owns(next) => next,
            _ => self.first(),
        }
    }
}

/// How many bytes of payload a data frame carries on a link that moved
/// `bytes_per_second` over the last seconds, when a frame may have
/// `max_payload_bytes` at most: a fixed share of a second of that rate,
/// so that a control frame behind it waits no longer than that.
#[must_use]
pub fn data_frame_bytes(bytes_per_second: u64, max_payload_bytes: usize) -> usize {
    let share =
        usize::try_from(bytes_per_second / DATA_FRAME_SECONDS_DIVISOR).unwrap_or(usize::MAX);
    share.max(MIN_DATA_FRAME_BYTES).min(max_payload_bytes)
}

/// Why a channel's arithmetic does not add up. Each of these means that
/// the other end broke the protocol, or that the tunnel cannot go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WindowError {
    /// More was to be sent than the other end gave credit for, or than
    /// may be kept for a resumption.
    #[error("{asked} bytes were to be sent where {allowed} may be")]
    NoRoom {
        /// What was to be sent.
        asked: u64,
        /// What may be sent now.
        allowed: u64,
    },
    /// Bytes arrived beyond the credit that was given.
    #[error("the other end sent past its credit")]
    Overrun,
    /// Bytes arrived that do not follow those before them.
    #[error("bytes at {offset} do not follow the {received} received")]
    Gap {
        /// Where the bytes that arrived begin.
        offset: u64,
        /// How many were received before.
        received: u64,
    },
    /// The other end acknowledged, or claims to have, more than was
    /// sent.
    #[error("the other end claims {claimed} bytes where {sent} were sent")]
    Ahead {
        /// What it claims.
        claimed: u64,
        /// What was sent.
        sent: u64,
    },
    /// The other end needs bytes again that are no longer kept.
    #[error("the bytes from {from} are no longer kept")]
    Lost {
        /// Where the bytes it needs begin.
        from: u64,
    },
    /// A count ran past what can be counted.
    #[error("a count of bytes overflowed")]
    Overflow,
    /// More was handed on than had arrived.
    #[error("more bytes were delivered than were received")]
    Undelivered,
}

/// One direction of a channel, as the end that sends it counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendWindow {
    /// Bytes put on the link: the offset of the next frame.
    sent: u64,
    /// Bytes the receiver has allowed in all.
    granted: u64,
    /// Bytes the receiver has acknowledged in all.
    acknowledged: u64,
    /// How many unacknowledged bytes may be kept.
    replay_limit: u64,
}

impl SendWindow {
    /// A new channel: the receiver's first credit is `window_bytes`.
    #[must_use]
    pub const fn new(window_bytes: u64, replay_limit: u64) -> Self {
        Self {
            sent: 0,
            granted: window_bytes,
            acknowledged: 0,
            replay_limit,
        }
    }

    /// The offset the next frame carries.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.sent
    }

    /// How many bytes are kept for a resumption: sent, and not yet
    /// acknowledged.
    #[must_use]
    pub const fn kept(&self) -> u64 {
        self.sent - self.acknowledged
    }

    /// How many bytes may be sent now: no more than the credit left, and
    /// no more than may still be kept. At zero the end stops reading
    /// from its local side.
    #[must_use]
    pub const fn sendable(&self) -> u64 {
        let credit = self.granted.saturating_sub(self.sent);
        let room = self.replay_limit.saturating_sub(self.kept());
        if credit < room { credit } else { room }
    }

    /// Counts a frame of `len` bytes as sent. Returns its offset.
    pub fn send(&mut self, len: u64) -> Result<u64, WindowError> {
        let allowed = self.sendable();
        if len > allowed {
            return Err(WindowError::NoRoom {
                asked: len,
                allowed,
            });
        }
        let offset = self.sent;
        self.sent += len;
        Ok(offset)
    }

    /// Takes more credit from the receiver.
    pub fn credit(&mut self, more: u64) -> Result<(), WindowError> {
        self.granted = self
            .granted
            .checked_add(more)
            .ok_or(WindowError::Overflow)?;
        Ok(())
    }

    /// Takes the receiver's word that it has `offset` bytes in all.
    /// Returns how many bytes need no longer be kept.
    pub fn acknowledge(&mut self, offset: u64) -> Result<u64, WindowError> {
        if offset > self.sent {
            return Err(WindowError::Ahead {
                claimed: offset,
                sent: self.sent,
            });
        }
        let freed = offset.saturating_sub(self.acknowledged);
        self.acknowledged += freed;
        Ok(freed)
    }

    /// Carries the direction on after a broken session: the receiver
    /// has `peer_received` bytes in all. Returns the offset to send
    /// again from; everything before it may be let go. The channel
    /// starts over with a first credit of `window_bytes` beyond what the
    /// receiver has.
    pub fn resume(&mut self, peer_received: u64, window_bytes: u64) -> Result<u64, WindowError> {
        if peer_received > self.sent {
            return Err(WindowError::Ahead {
                claimed: peer_received,
                sent: self.sent,
            });
        }
        if peer_received < self.acknowledged {
            return Err(WindowError::Lost {
                from: peer_received,
            });
        }
        self.acknowledged = peer_received;
        self.sent = peer_received;
        self.granted = peer_received
            .checked_add(window_bytes)
            .ok_or(WindowError::Overflow)?;
        Ok(peer_received)
    }
}

/// What to do with the payload of a data frame that arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Accepted {
    /// How many of its first bytes were received before, and are left
    /// out; the rest follows what was received and is taken.
    pub skip: u64,
}

/// One direction of a channel, as the end that receives it counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecvWindow {
    /// Bytes taken from the link, in order.
    received: u64,
    /// Bytes handed to the local consumer.
    delivered: u64,
    /// The offset the sender may reach with the credit given so far.
    granted: u64,
    /// The offset last acknowledged to the sender.
    acknowledged: u64,
    /// Credit earned by handing bytes on, and not yet given.
    owed: u64,
    /// Bytes up to this offset earn no credit when they are handed on:
    /// they were held when the channel started over with a full credit.
    uncredited_through: u64,
}

impl RecvWindow {
    /// A new channel: the sender starts with a credit of `window_bytes`.
    #[must_use]
    pub const fn new(window_bytes: u64) -> Self {
        Self {
            received: 0,
            delivered: 0,
            granted: window_bytes,
            acknowledged: 0,
            owed: 0,
            uncredited_through: 0,
        }
    }

    /// How many bytes were received in all: what an acknowledgement and
    /// a resumption state.
    #[must_use]
    pub const fn received(&self) -> u64 {
        self.received
    }

    /// How many bytes were received and are not handed on yet.
    #[must_use]
    pub const fn held(&self) -> u64 {
        self.received - self.delivered
    }

    /// Takes a data frame of `len` bytes that says it begins at
    /// `offset`. Bytes that were received before are left out; bytes
    /// that leave a hole, or pass the credit that was given, break the
    /// protocol.
    pub fn accept(&mut self, offset: u64, len: u64) -> Result<Accepted, WindowError> {
        if offset > self.received {
            return Err(WindowError::Gap {
                offset,
                received: self.received,
            });
        }
        let end = offset.checked_add(len).ok_or(WindowError::Overflow)?;
        if end > self.granted {
            return Err(WindowError::Overrun);
        }
        let skip = (self.received - offset).min(len);
        self.received = self.received.max(end);
        Ok(Accepted { skip })
    }

    /// Counts `len` bytes as handed to the local consumer, which earns
    /// the sender as much credit.
    pub fn deliver(&mut self, len: u64) -> Result<(), WindowError> {
        let delivered = self
            .delivered
            .checked_add(len)
            .filter(|total| *total <= self.received);
        let delivered = delivered.ok_or(WindowError::Undelivered)?;
        let credited_from = self.delivered.max(self.uncredited_through);
        self.owed += delivered.saturating_sub(credited_from);
        self.delivered = delivered;
        Ok(())
    }

    /// The credit to give the sender now; zero when none is owed. Once
    /// taken it counts as given.
    pub fn take_credit(&mut self) -> u64 {
        let credit = std::mem::take(&mut self.owed);
        self.granted = self.granted.saturating_add(credit);
        credit
    }

    /// Whether an acknowledgement is due, `since_last` after the last
    /// one: enough bytes arrived, or some arrived and enough time
    /// passed.
    #[must_use]
    pub fn ack_due(&self, since_last: Duration) -> bool {
        let unacknowledged = self.received - self.acknowledged;
        unacknowledged >= ACK_EVERY_BYTES || (unacknowledged > 0 && since_last >= ACK_EVERY)
    }

    /// The offset to acknowledge now. Once taken it counts as
    /// acknowledged.
    pub fn take_ack(&mut self) -> u64 {
        self.acknowledged = self.received;
        self.received
    }

    /// Carries the direction on after a broken session. Returns what to
    /// tell the sender: how many bytes were received in all. The channel
    /// starts over with a first credit of `window_bytes`; what is still
    /// held from before earns no further credit when it is handed on,
    /// so that the sender is never allowed more than the two together.
    pub fn resume(&mut self, window_bytes: u64) -> u64 {
        self.granted = self.received.saturating_add(window_bytes);
        self.acknowledged = self.received;
        self.owed = 0;
        self.uncredited_through = self.received;
        self.received
    }
}

/// Whether a message between a harness and its executor is text or
/// bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecOpcode {
    /// Text.
    Text,
    /// Bytes.
    Binary,
}

impl ExecOpcode {
    const TEXT: u8 = 1;
    const BINARY: u8 = 2;

    /// The byte it is written as.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Text => Self::TEXT,
            Self::Binary => Self::BINARY,
        }
    }
}

/// One whole message between a harness and its executor. A tunnel is a
/// stream of bytes, so each message is written into it with what it is
/// and how long it is in front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecMessage {
    /// Text or bytes.
    pub opcode: ExecOpcode,
    /// The message.
    pub payload: Bytes,
}

/// Why the bytes of a tunnel are not a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExecFramingError {
    /// The first byte says neither text nor bytes.
    #[error("{0} is neither text nor bytes")]
    Opcode(u8),
    /// The message is longer than may be taken.
    #[error("a message of {len} bytes where {max} are the most")]
    TooLong {
        /// Its stated length.
        len: u64,
        /// The most that is taken.
        max: usize,
    },
}

impl ExecMessage {
    /// The bytes in front of every message: what it is, then its length
    /// as four bytes, highest first.
    pub const HEADER_BYTES: usize = 5;

    /// Writes the message into `stream`.
    pub fn write(&self, stream: &mut BytesMut) -> Result<(), ExecFramingError> {
        let len = u32::try_from(self.payload.len()).map_err(|_| ExecFramingError::TooLong {
            len: self.payload.len() as u64,
            max: u32::MAX as usize,
        })?;
        stream.reserve(Self::HEADER_BYTES + self.payload.len());
        stream.put_u8(self.opcode.byte());
        stream.put_u32(len);
        stream.put_slice(&self.payload);
        Ok(())
    }

    /// Takes the next whole message from the front of `stream`, when
    /// one has arrived whole; `None` leaves the bytes where they are
    /// until more come. A message longer than `max_bytes` is refused as
    /// soon as its length is known.
    pub fn take(stream: &mut BytesMut, max_bytes: usize) -> Result<Option<Self>, ExecFramingError> {
        if stream.len() < Self::HEADER_BYTES {
            return Ok(None);
        }
        let opcode = match stream[0] {
            ExecOpcode::TEXT => ExecOpcode::Text,
            ExecOpcode::BINARY => ExecOpcode::Binary,
            other => return Err(ExecFramingError::Opcode(other)),
        };
        let stated = u32::from_be_bytes([stream[1], stream[2], stream[3], stream[4]]);
        let len = usize::try_from(stated).ok().filter(|len| *len <= max_bytes);
        let len = len.ok_or(ExecFramingError::TooLong {
            len: u64::from(stated),
            max: max_bytes,
        })?;
        if stream.len() - Self::HEADER_BYTES < len {
            return Ok(None);
        }
        stream.advance(Self::HEADER_BYTES);
        Ok(Some(Self {
            opcode,
            payload: stream.split_to(len).freeze(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    const MACHINE: &str = "0199c1a0-0000-7000-8000-00000000000a";
    const OTHER: &str = "0199c1a0-0000-7000-8000-00000000000b";
    const THING: &str = "0199c1a0-0000-7000-8000-00000000000c";
    const WINDOW: u64 = 1024 * 1024;

    fn id(text: &str) -> Id {
        text.parse().unwrap()
    }

    fn round_trip<T: Serialize + DeserializeOwned>(example: Value) -> T {
        let read: T = serde_json::from_value(example.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), example);
        read
    }

    fn claims() -> TicketClaims {
        TicketClaims {
            jti: id(THING),
            kind: TunnelKind::Exec,
            src: TicketSource::Machine(id(MACHINE)),
            dst: id(OTHER),
            service: "exec/codex-exec-server".into(),
            thread_id: Some(id(THING)),
            tool_set: None,
            tool_environment_id: None,
            tool_environment_generation: None,
            port: None,
            exp: 1_791_500_000,
        }
    }

    fn before_expiry() -> Time {
        "2026-10-08T17:04:05.000Z".parse().unwrap()
    }

    #[test]
    fn a_tickets_claims_read_and_write_as_the_protocol_has_them() {
        let read: TicketClaims = round_trip(json!({
            "jti": THING, "kind": "mcp", "src": MACHINE, "dst": OTHER, "service": "mcp/exec",
            "thread_id": THING, "tool_set": "claude-code", "exp": 1791500000
        }));
        assert_eq!(read.tool_set.as_deref(), Some("claude-code"));
        let shell: TicketClaims = round_trip(json!({
            "jti": THING, "kind": "pty", "src": "api", "dst": OTHER, "service": format!("pty/{THING}"), "exp": 1791500000
        }));
        assert_eq!(shell.src, TicketSource::Api);
        assert_eq!(shell.thread_id, None);
    }

    #[test]
    fn a_ticket_is_put_together_and_taken_apart_without_loss() {
        let input = Ticket::signing_input(&claims()).unwrap();
        assert!(input.starts_with("v1."));
        let text = Ticket::assemble(&input, b"signature");
        let ticket = Ticket::parse(&text).unwrap();
        assert_eq!(ticket.claims(), &claims());
        assert_eq!(ticket.signed(), input.as_bytes());
        assert_eq!(ticket.signature(), b"signature");
    }

    #[test]
    fn a_text_that_is_no_ticket_is_refused() {
        let input = Ticket::signing_input(&claims()).unwrap();
        let good = Ticket::assemble(&input, b"signature");
        assert_eq!(Ticket::parse("v1.only-two"), Err(TicketError::Shape));
        assert_eq!(
            Ticket::parse(&good.replacen("v1", "v2", 1)),
            Err(TicketError::Shape)
        );
        assert_eq!(
            Ticket::parse(&format!("{good}.more")),
            Err(TicketError::Shape)
        );
        assert_eq!(Ticket::parse("v1.!!!.c2ln"), Err(TicketError::Encoding));
        assert_eq!(Ticket::parse("v1.e30.c2ln"), Err(TicketError::Claims));
    }

    #[test]
    fn a_ticket_opens_channels_for_the_one_it_names_until_it_lapses() {
        let machine = TicketSource::Machine(id(MACHINE));
        assert_eq!(claims().check(machine, before_expiry()), Ok(()));
        assert_eq!(
            claims().check(machine, before_expiry()),
            Ok(()),
            "a ticket opens many channels"
        );
    }

    #[test]
    fn a_ticket_presented_by_another_than_it_names_is_refused() {
        let other = TicketSource::Machine(id(OTHER));
        assert_eq!(
            claims().check(other, before_expiry()),
            Err(OpenErrorCode::BadTicket)
        );
        assert_eq!(
            claims().check(TicketSource::Api, before_expiry()),
            Err(OpenErrorCode::BadTicket)
        );
    }

    #[test]
    fn a_ticket_past_its_time_is_refused() {
        let machine = TicketSource::Machine(id(MACHINE));
        let at_expiry: Time = "2026-10-08T22:53:20.000Z".parse().unwrap();
        assert_eq!(at_expiry.unix_millis(), 1_791_500_000_000);
        assert_eq!(
            claims().check(machine, at_expiry),
            Err(OpenErrorCode::Expired)
        );
    }

    #[test]
    fn a_ticket_whose_service_is_not_of_its_kind_is_refused() {
        let machine = TicketSource::Machine(id(MACHINE));
        let mut wrong = claims();
        wrong.service = format!("pty/{THING}");
        assert_eq!(
            wrong.check(machine, before_expiry()),
            Err(OpenErrorCode::BadTicket)
        );
    }

    #[test]
    fn every_service_is_written_and_read_by_its_name() {
        let services = [
            (
                Service::Exec {
                    executor: "codex-exec-server".into(),
                },
                "exec/codex-exec-server".to_owned(),
            ),
            (
                Service::Llm {
                    lease_id: id(THING),
                },
                format!("llm/{THING}"),
            ),
            (
                Service::Mcp {
                    server: "tmux".into(),
                },
                "mcp/tmux".to_owned(),
            ),
            (
                Service::Pty {
                    terminal_id: id(THING),
                },
                format!("pty/{THING}"),
            ),
            (
                Service::AgentPty {
                    thread_id: id(THING),
                },
                format!("agent-pty/{THING}"),
            ),
        ];
        for (service, name) in services {
            assert_eq!(service.to_string(), name);
            assert_eq!(name.parse::<Service>(), Ok(service));
        }
        for name in ["", "exec", "exec/", "llm/not-an-id", "ssh/host"] {
            assert!(name.parse::<Service>().is_err(), "{name}");
        }
        assert_eq!(
            Service::AgentPty {
                thread_id: id(THING)
            }
            .kind(),
            TunnelKind::Pty
        );
    }

    #[test]
    fn tunnel_frames_read_and_write_as_the_protocol_has_them() {
        let request: Open = round_trip(json!({ "c": 7, "ticket": "v1.e30.c2ln" }));
        assert!(matches!(request, Open::Request(_)));
        let offer: Open = round_trip(json!({
            "c": 12, "kind": "exec", "service": "exec/codex-exec-server", "from": MACHINE,
            "claims": serde_json::to_value(claims()).unwrap()
        }));
        assert!(matches!(offer, Open::Offer(_)));
        assert_eq!(offer.channel(), 12);
        round_trip::<OpenOk>(json!({ "c": 12, "cid": THING }));
        round_trip::<OpenErr>(json!({ "c": 7, "code": "peer_offline", "detail": "no answer" }));
        round_trip::<Credit>(json!({ "c": 7, "n": 65536 }));
        round_trip::<Eof>(json!({ "c": 7 }));
        round_trip::<Close>(json!({ "c": 7, "code": "reset", "detail": "not resumed in time" }));
        round_trip::<ChAck>(json!({ "c": 7, "o": 262144 }));
        round_trip::<Resume>(
            json!({ "c": 9, "ticket": "v1.e30.c2ln", "jti_channel": THING, "recv": 4096 }),
        );
        round_trip::<ResumeOk>(json!({ "c": 14, "recv": 8192 }));
        round_trip::<PtyResize>(json!({ "c": 4, "cols": 120, "rows": 40 }));
        round_trip::<PtyExit>(json!({ "c": 4, "code": 0 }));
    }

    #[test]
    fn machines_number_their_channels_odd_and_the_gateway_even() {
        assert_eq!(Side::Machine.first(), 1);
        assert_eq!(Side::Machine.next(1), 3);
        assert_eq!(Side::Gateway.next(2), 4);
        assert!(Side::Machine.owns(7) && !Side::Gateway.owns(7));
        assert!(Side::Gateway.owns(12) && !Side::Machine.owns(12));
        assert!(!Side::Gateway.owns(0) && !Side::Machine.owns(0));
        assert_eq!(Side::Machine.next(u32::MAX), 1);
        assert_eq!(Side::Gateway.next(u32::MAX - 1), 2);
    }

    #[test]
    fn a_data_frame_is_a_quarter_second_of_the_link_within_its_bounds() {
        let max = 262_144 - 13;
        assert_eq!(data_frame_bytes(93_750, max), 23_437, "750 kbit/s");
        assert_eq!(
            data_frame_bytes(1_000, max),
            MIN_DATA_FRAME_BYTES,
            "a thin line still moves whole frames"
        );
        assert_eq!(
            data_frame_bytes(100_000_000, max),
            max,
            "a fast link fills the frame"
        );
    }

    #[test]
    fn a_sender_never_has_more_outstanding_than_its_credit() {
        let mut send = SendWindow::new(1_000, REPLAY_BUFFER_BYTES);
        assert_eq!(send.send(600), Ok(0));
        assert_eq!(send.sendable(), 400);
        assert_eq!(
            send.send(401),
            Err(WindowError::NoRoom {
                asked: 401,
                allowed: 400
            })
        );
        assert_eq!(send.send(400), Ok(600));
        assert_eq!(send.sendable(), 0);
        send.credit(250).unwrap();
        assert_eq!(send.send(250), Ok(1_000));
    }

    #[test]
    fn a_sender_stops_when_it_keeps_as_much_as_it_may_for_a_resumption() {
        let mut send = SendWindow::new(10_000, 1_000);
        send.send(1_000).unwrap();
        assert_eq!(send.sendable(), 0, "credit is left, room to keep is not");
        assert_eq!(send.acknowledge(700), Ok(700));
        assert_eq!(send.kept(), 300);
        assert_eq!(send.sendable(), 700);
        assert_eq!(
            send.acknowledge(500),
            Ok(0),
            "an older acknowledgement frees nothing"
        );
        assert_eq!(
            send.acknowledge(1_001),
            Err(WindowError::Ahead {
                claimed: 1_001,
                sent: 1_000
            })
        );
    }

    #[test]
    fn a_receiver_takes_bytes_in_order_and_gives_credit_as_they_are_handed_on() {
        let mut recv = RecvWindow::new(1_000);
        assert_eq!(recv.accept(0, 600), Ok(Accepted { skip: 0 }));
        assert_eq!(recv.take_credit(), 0, "nothing was handed on yet");
        recv.deliver(450).unwrap();
        assert_eq!(recv.held(), 150);
        assert_eq!(recv.take_credit(), 450);
        assert_eq!(recv.take_credit(), 0);
        assert_eq!(recv.deliver(151), Err(WindowError::Undelivered));
    }

    #[test]
    fn bytes_that_leave_a_hole_or_pass_the_credit_break_the_protocol() {
        let mut recv = RecvWindow::new(1_000);
        recv.accept(0, 100).unwrap();
        assert_eq!(
            recv.accept(200, 10),
            Err(WindowError::Gap {
                offset: 200,
                received: 100
            })
        );
        assert_eq!(recv.accept(100, 901), Err(WindowError::Overrun));
        assert_eq!(recv.accept(100, 900), Ok(Accepted { skip: 0 }));
    }

    #[test]
    fn bytes_that_arrive_twice_are_taken_once() {
        let mut recv = RecvWindow::new(1_000);
        recv.accept(0, 100).unwrap();
        assert_eq!(recv.accept(0, 100), Ok(Accepted { skip: 100 }));
        assert_eq!(recv.accept(60, 100), Ok(Accepted { skip: 40 }));
        assert_eq!(recv.received(), 160);
    }

    #[test]
    fn an_acknowledgement_is_due_after_enough_bytes_or_enough_time() {
        let mut recv = RecvWindow::new(WINDOW);
        assert!(!recv.ack_due(Duration::from_secs(5)), "nothing arrived");
        recv.accept(0, 1_000).unwrap();
        assert!(!recv.ack_due(Duration::from_millis(999)));
        assert!(recv.ack_due(Duration::from_secs(1)));
        assert_eq!(recv.take_ack(), 1_000);
        assert!(!recv.ack_due(Duration::from_secs(5)));
        recv.accept(1_000, ACK_EVERY_BYTES).unwrap();
        assert!(recv.ack_due(Duration::ZERO));
    }

    #[test]
    fn a_hundred_mebibytes_cross_a_one_mebibyte_window_and_nobody_holds_more_than_it_may() {
        const TOTAL: u64 = 100 * 1024 * 1024;
        const FRAME: u64 = 262_144 - 13;
        let mut send = SendWindow::new(WINDOW, REPLAY_BUFFER_BYTES);
        let mut recv = RecvWindow::new(WINDOW);
        let mut delivered = 0;
        let mut step = 0u64;
        while delivered < TOTAL {
            step += 1;
            let len = send.sendable().min(FRAME).min(TOTAL - send.offset());
            if len > 0 {
                let offset = send.send(len).unwrap();
                assert_eq!(recv.accept(offset, len), Ok(Accepted { skip: 0 }));
            }
            assert!(
                recv.held() <= WINDOW,
                "the receiver holds {} bytes",
                recv.held()
            );
            assert!(send.kept() <= REPLAY_BUFFER_BYTES);
            // The consumer reads at its own pace, and sometimes not at all.
            let taken = if step.is_multiple_of(7) {
                0
            } else {
                recv.held().min(100_000 + step % 50_000)
            };
            recv.deliver(taken).unwrap();
            delivered += taken;
            send.credit(recv.take_credit()).unwrap();
            if recv.ack_due(Duration::ZERO) {
                send.acknowledge(recv.take_ack()).unwrap();
            }
        }
        assert_eq!(recv.received(), TOTAL);
        assert_eq!(send.offset(), TOTAL);
    }

    #[test]
    fn a_resumed_channel_sends_again_from_what_the_other_end_has() {
        let mut send = SendWindow::new(WINDOW, REPLAY_BUFFER_BYTES);
        let mut recv = RecvWindow::new(WINDOW);
        send.send(5_000).unwrap();
        recv.accept(0, 3_000).unwrap();
        // The session drops: 2 000 bytes were on their way and are gone.
        let peer_received = recv.resume(WINDOW);
        assert_eq!(peer_received, 3_000);
        assert_eq!(send.resume(peer_received, WINDOW), Ok(3_000));
        assert_eq!(send.offset(), 3_000);
        assert_eq!(send.kept(), 0);
        let offset = send.send(2_000).unwrap();
        assert_eq!(recv.accept(offset, 2_000), Ok(Accepted { skip: 0 }));
        assert_eq!(recv.received(), 5_000);
    }

    #[test]
    fn a_resumed_channel_never_lets_the_receiver_hold_more_than_two_windows() {
        let mut send = SendWindow::new(WINDOW, REPLAY_BUFFER_BYTES);
        let mut recv = RecvWindow::new(WINDOW);
        let offset = send.send(WINDOW).unwrap();
        recv.accept(offset, WINDOW).unwrap();
        // Nothing was handed on when the session dropped.
        send.resume(recv.resume(WINDOW), WINDOW).unwrap();
        let offset = send.send(send.sendable()).unwrap();
        recv.accept(offset, WINDOW).unwrap();
        assert_eq!(recv.held(), 2 * WINDOW);
        // Handing on what was held before the drop earns no credit.
        recv.deliver(WINDOW).unwrap();
        assert_eq!(recv.take_credit(), 0);
        recv.deliver(1_000).unwrap();
        assert_eq!(recv.take_credit(), 1_000);
    }

    #[test]
    fn a_channel_whose_bytes_are_no_longer_kept_cannot_be_resumed() {
        let mut send = SendWindow::new(WINDOW, REPLAY_BUFFER_BYTES);
        send.send(5_000).unwrap();
        send.acknowledge(4_000).unwrap();
        assert_eq!(
            send.resume(3_000, WINDOW),
            Err(WindowError::Lost { from: 3_000 })
        );
        assert_eq!(
            send.resume(5_001, WINDOW),
            Err(WindowError::Ahead {
                claimed: 5_001,
                sent: 5_000
            })
        );
    }

    #[test]
    fn only_exchanges_are_closed_for_sitting_idle() {
        assert_eq!(TunnelKind::Llm.idle_secs(), Some(600));
        assert_eq!(TunnelKind::Mcp.idle_secs(), Some(600));
        assert_eq!(TunnelKind::Exec.idle_secs(), None);
        assert_eq!(TunnelKind::Pty.idle_secs(), None);
    }

    #[test]
    fn messages_of_an_executor_keep_their_bounds_inside_a_stream_of_bytes() {
        let text = ExecMessage {
            opcode: ExecOpcode::Text,
            payload: Bytes::from_static(br#"{"id":1}"#),
        };
        let bytes = ExecMessage {
            opcode: ExecOpcode::Binary,
            payload: Bytes::from_static(&[0, 255, 7]),
        };
        let mut stream = BytesMut::new();
        text.write(&mut stream).unwrap();
        bytes.write(&mut stream).unwrap();
        assert_eq!(&stream[..5], &[1, 0, 0, 0, 8]);
        assert_eq!(ExecMessage::take(&mut stream, 1024), Ok(Some(text)));
        assert_eq!(ExecMessage::take(&mut stream, 1024), Ok(Some(bytes)));
        assert_eq!(ExecMessage::take(&mut stream, 1024), Ok(None));
    }

    #[test]
    fn a_message_that_arrives_in_pieces_is_taken_when_it_is_whole() {
        let message = ExecMessage {
            opcode: ExecOpcode::Text,
            payload: Bytes::from_static(b"hello, executor"),
        };
        let mut whole = BytesMut::new();
        message.write(&mut whole).unwrap();
        let mut stream = BytesMut::new();
        for byte in &whole[..whole.len() - 1] {
            stream.put_u8(*byte);
            assert_eq!(ExecMessage::take(&mut stream, 1024), Ok(None));
        }
        stream.put_u8(whole[whole.len() - 1]);
        assert_eq!(ExecMessage::take(&mut stream, 1024), Ok(Some(message)));
        assert!(stream.is_empty());
    }

    #[test]
    fn bytes_that_are_no_message_and_messages_too_long_are_refused() {
        let mut unknown = BytesMut::from(&[9u8, 0, 0, 0, 1, 0][..]);
        assert_eq!(
            ExecMessage::take(&mut unknown, 1024),
            Err(ExecFramingError::Opcode(9))
        );
        let mut long = BytesMut::from(&[2u8, 0, 0, 4, 1][..]);
        assert_eq!(
            ExecMessage::take(&mut long, 1024),
            Err(ExecFramingError::TooLong {
                len: 1025,
                max: 1024
            })
        );
    }
}
