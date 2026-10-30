//! Gateway/API messages and terminal tunnel frames.
//!
//! A gateway holds the sessions of the machines and nothing else: it
//! keeps no state that has to last. Everything it needs to know it asks
//! the system over one connection that it dials itself, the backplane,
//! and everything machines and the system say to each other crosses
//! that connection wrapped in a frame that names the machine.
//!
//! Tunnels between two machines never touch the backplane. The one kind
//! that does is a tunnel whose other end is a person's browser: the
//! system opens it, and its data travels here as binary frames that
//! name the machine and the channel.
//!
//! When the backplane is down a gateway refuses new sessions as busy,
//! stops reading events from machines, so that nothing is acknowledged
//! and machines keep what they have, and goes on relaying the tunnels
//! that are open. When it is back, the gateway reports every session it
//! holds.

use crate::commands::{Cmd, CmdAck, CmdCancel, CmdRes};
use crate::frames::{Frame, FrameError, tagged};
use crate::session::{Role, TlsMode};
use crate::streams::{Ack, Eph, Ev, Evb, Nak, Reset};
use crate::tunnels::{CloseCode, OpenErrorCode, PtyExit, PtyResize};
use crate::version::{ByeCode, ConnectorVersions, Limits, RejectCode};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use orch_types::{Id, Seq};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

/// The path of the backplane on the system's internal listener.
pub const BACKPLANE_PATH: &str = "/internal/backplane";
/// How often a gateway reports its figures, in seconds. The report is
/// also how the system knows that the gateway is alive.
pub const STATS_EVERY_SECS: u32 = 15;
/// How long a gateway waits for the system to say whether a session may
/// start, before it refuses the session as busy. Half of what a machine
/// allows the whole step, so that the refusal still reaches it.
pub const AUTH_QUERY_TIMEOUT_SECS: u32 = 5;

/// The gateway's first word on the backplane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GwHello {
    /// The gateway's name.
    pub gateway_id: String,
    /// The gateway's own version.
    pub version: String,
    /// When the gateway's process started, which its heartbeat reports; a gateway
    /// that does not say it is taken to have started when it connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<orch_types::Time>,
}

/// The system's answer: what a gateway needs to do its work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiHello {
    /// The public key that tickets are signed for, as unpadded
    /// base64url of its bytes.
    pub ticket_public_key: String,
    /// The limits a gateway states to every session.
    pub limits: Limits,
    /// The connector builds a gateway states to every session.
    pub versions: ConnectorVersions,
}

/// A gateway's question whether a session may start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthQuery {
    /// Authenticated owner-selected addresses, distinct from the server's defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Vec<crate::enrollment::Endpoint>>,
    /// A node's explicit preference list for hosted units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted_endpoints: Option<Vec<crate::enrollment::Endpoint>>,
    /// A number the answer repeats.
    pub n: i64,
    /// The machine that asks for a session.
    pub machine_id: Id,
    /// The SHA-256 of the certificate it showed, in lowercase
    /// hexadecimal.
    pub fingerprint: String,
    /// How the gateway knows the machine.
    pub tls: TlsMode,
    /// The address it connected from.
    pub ip: IpAddr,
    /// The connector's own version.
    pub connector_version: String,
    /// The roles it has switched on.
    pub roles: Vec<Role>,
}

/// The system's answer to an [`AuthQuery`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthReply {
    /// The system's current preference list, delivered after authenticating the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Vec<crate::enrollment::Endpoint>>,
    /// Only for a proved node below the accepted build, after the server holds and drains its units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_permit: Option<crate::session::UpdatePermit>,
    /// The number of the question.
    pub n: i64,
    /// Whether the session may start.
    pub ok: bool,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<RejectCode>,
    /// Placement read by the system from its machine record, for operational labels only; never asserted by the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    /// How far each of the machine's streams was taken in, for a session
    /// that may start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursors: Option<BTreeMap<String, Seq>>,
}

/// A gateway's word that a machine has a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineUp {
    /// The machine.
    pub machine_id: Id,
    /// The session.
    pub session_id: Id,
    /// The address it connected from.
    pub ip: IpAddr,
}

/// A gateway's word that a session ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineDown {
    /// The machine.
    pub machine_id: Id,
    /// The session that ended: an older one than the machine's current
    /// session changes nothing.
    pub session_id: Id,
    /// How it ended.
    pub reason: String,
}

/// A text frame that a machine sent for the system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Up {
    /// The machine.
    pub machine_id: Id,
    /// The session it came on.
    pub session_id: Id,
    /// The frame.
    pub frame: UpFrame,
}

/// A text frame that the system sends a machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Down {
    /// The machine.
    pub machine_id: Id,
    /// The frame.
    pub frame: DownFrame,
}

/// The system's order to end a machine's session now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Kick {
    /// The machine.
    pub machine_id: Id,
    /// Why, which is what the machine is told.
    pub code: ByeCode,
}

/// A gateway's figures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Stats {
    /// How many sessions it holds.
    pub sessions: i64,
    /// How many channels are open on them.
    pub channels: i64,
    /// How many bytes of tunnel data it has relayed.
    pub bytes_relayed: i64,
    /// Internal operational aggregates since the last report; absent on older gateways.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metrics: Vec<orch_types::measurements::MetricSample>,
}

/// The system's request to open a tunnel to a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunOpen {
    /// The machine the tunnel ends at.
    pub machine_id: Id,
    /// The number the system gives the channel.
    pub channel: u32,
    /// The ticket that allows it.
    pub ticket: String,
}

/// The word that such a tunnel is open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunOpenOk {
    /// The machine.
    pub machine_id: Id,
    /// The channel.
    pub channel: u32,
    /// The name the machine gave the tunnel.
    pub cid: Id,
    /// Actual inside peer for a port channel, absent on other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<orch_api_contract::ports::PortConnectionInside>,
}

/// The word that such a tunnel was not opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunOpenErr {
    /// The machine.
    pub machine_id: Id,
    /// The channel.
    pub channel: u32,
    /// Why.
    pub code: OpenErrorCode,
    /// A sentence for whoever reads the log.
    #[serde(default)]
    pub detail: String,
}

/// More credit for the sender of such a tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunCredit {
    /// The machine.
    pub machine_id: Id,
    /// The channel.
    pub channel: u32,
    /// How many more bytes may be sent.
    pub n: u64,
}

/// The end of one direction of such a tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunEof {
    /// The machine.
    pub machine_id: Id,
    /// The channel.
    pub channel: u32,
}

/// The end of such a tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunClose {
    /// The machine.
    pub machine_id: Id,
    /// The channel.
    pub channel: u32,
    /// How it ended.
    pub code: CloseCode,
    /// A sentence for whoever reads the log.
    #[serde(default)]
    pub detail: String,
}

/// A text frame of such a tunnel's channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TunText {
    /// The machine.
    pub machine_id: Id,
    /// The channel.
    pub channel: u32,
    /// The frame, as the machine gets or sent it. The channel number in
    /// it is the one of whoever reads it.
    pub frame: ChannelText,
}

tagged! {
    /// A text frame of the backplane.
    BackplaneFrame {
        /// The gateway's first word.
        GwHello(GwHello) = "gw_hello",
        /// The system's answer to it.
        ApiHello(ApiHello) = "api_hello",
        /// A gateway's question whether a session may start.
        AuthQuery(AuthQuery) = "auth_query",
        /// The system's answer to it.
        AuthReply(AuthReply) = "auth_reply",
        /// A gateway's word that a machine has a session.
        MachineUp(MachineUp) = "machine_up",
        /// A gateway's word that a session ended.
        MachineDown(MachineDown) = "machine_down",
        /// A text frame that a machine sent for the system.
        Up(Up) = "up",
        /// A text frame that the system sends a machine.
        Down(Down) = "down",
        /// The system's order to end a machine's session.
        Kick(Kick) = "kick",
        /// A gateway's figures.
        Stats(Stats) = "stats",
        /// The system's request to open a tunnel to a machine.
        TunOpen(TunOpen) = "tun_open",
        /// The word that such a tunnel is open.
        TunOpenOk(TunOpenOk) = "tun_open_ok",
        /// The word that such a tunnel was not opened.
        TunOpenErr(TunOpenErr) = "tun_open_err",
        /// More credit for the sender of such a tunnel.
        TunCredit(TunCredit) = "tun_credit",
        /// The end of one direction of such a tunnel.
        TunEof(TunEof) = "tun_eof",
        /// The end of such a tunnel.
        TunClose(TunClose) = "tun_close",
        /// A text frame of such a tunnel's channel.
        TunText(TunText) = "tun_text",
        /// A current in-memory gateway connection query.
        PortConnectionsQuery(crate::ports::PortConnectionsQuery) = "port_connections_query",
        /// A current unreplayed gateway connection answer.
        PortConnectionsReply(crate::ports::PortConnectionsReply) = "port_connections_reply",
    }
}

/// An end of the backplane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackplaneSide {
    /// The gateway, which dials.
    Gateway,
    /// The system, which is dialled.
    Api,
}

impl BackplaneFrame {
    /// Whether `side` is one that sends frames of this type.
    #[must_use]
    pub const fn sent_by(&self, side: BackplaneSide) -> bool {
        match self {
            Self::GwHello(_)
            | Self::AuthQuery(_)
            | Self::MachineUp(_)
            | Self::MachineDown(_)
            | Self::Up(_)
            | Self::Stats(_)
            | Self::TunOpenOk(_)
            | Self::TunOpenErr(_)
            | Self::PortConnectionsReply(_) => matches!(side, BackplaneSide::Gateway),
            Self::ApiHello(_)
            | Self::AuthReply(_)
            | Self::Down(_)
            | Self::Kick(_)
            | Self::TunOpen(_)
            | Self::PortConnectionsQuery(_) => {
                matches!(side, BackplaneSide::Api)
            }
            Self::TunCredit(_) | Self::TunEof(_) | Self::TunClose(_) | Self::TunText(_) => true,
        }
    }
}

tagged! {
    /// A text frame of a machine that is for the system, and that a
    /// gateway therefore passes up.
    UpFrame {
        /// A local import asked by the authenticated machine, as its owner.
        McpImportAsk(crate::mcp_import::McpImportAsk) = "mcp_import_ask",
        /// The authenticated machine's environment-local read/assertion refresh.
        PortLocalAsk(crate::local_ports::PortLocalAsk) = "port_local_ask",
        /// A host node's transient port record reply.
        PortConnectionsReply(crate::ports::PortConnectionsReply) = "port_connections_reply",
        /// One event of a stream.
        Ev(Ev) = "ev",
        /// Several events of one stream.
        Evb(Evb) = "evb",
        /// A measurement that is not kept.
        Eph(Eph) = "eph",
        /// A machine's word that events of a stream are gone.
        Reset(Reset) = "reset",
        /// A machine's word that it has a command.
        CmdAck(CmdAck) = "cmd_ack",
        /// How a command ended.
        CmdRes(CmdRes) = "cmd_res",
    }
}

tagged! {
    /// A text frame of the system that is for a machine, and that a
    /// gateway therefore passes down.
    DownFrame {
        /// The system's answer to a local import request.
        McpImportAnswer(crate::mcp_import::McpImportAnswer) = "mcp_import_answer",
        /// The current scoped read; no row goes into an event spool or command journal.
        PortLocalAnswer(crate::local_ports::PortLocalAnswer) = "port_local_answer",
        /// A transient query of a host node's own memory.
        PortConnectionsQuery(crate::ports::PortConnectionsQuery) = "port_connections_query",
        /// The system's word that it has taken a stream in through a
        /// number.
        Ack(Ack) = "ack",
        /// The system's word that events of a stream are missing.
        Nak(Nak) = "nak",
        /// A command.
        Cmd(Cmd) = "cmd",
        /// The system's wish that a command not be carried out.
        CmdCancel(CmdCancel) = "cmd_cancel",
    }
}

tagged! {
    /// A text frame of a terminal's channel.
    ChannelText {
        /// The terminal's new size, from the system.
        PtyResize(PtyResize) = "pty_resize",
        /// The end of the process in it, from the machine.
        PtyExit(PtyExit) = "pty_exit",
    }
}

impl TryFrom<Frame> for UpFrame {
    type Error = Box<Frame>;

    /// The frame as one a gateway passes up; a frame that is not for
    /// the system is given back.
    fn try_from(frame: Frame) -> Result<Self, Self::Error> {
        match frame {
            Frame::McpImportAsk(body) => Ok(Self::McpImportAsk(body)),
            Frame::PortLocalAsk(body) => Ok(Self::PortLocalAsk(body)),
            Frame::PortConnectionsReply(body) => Ok(Self::PortConnectionsReply(body)),
            Frame::Ev(body) => Ok(Self::Ev(body)),
            Frame::Evb(body) => Ok(Self::Evb(body)),
            Frame::Eph(body) => Ok(Self::Eph(body)),
            Frame::Reset(body) => Ok(Self::Reset(body)),
            Frame::CmdAck(body) => Ok(Self::CmdAck(body)),
            Frame::CmdRes(body) => Ok(Self::CmdRes(body)),
            other => Err(Box::new(other)),
        }
    }
}

impl From<DownFrame> for Frame {
    fn from(frame: DownFrame) -> Self {
        match frame {
            DownFrame::McpImportAnswer(body) => Self::McpImportAnswer(body),
            DownFrame::PortLocalAnswer(body) => Self::PortLocalAnswer(body),
            DownFrame::PortConnectionsQuery(body) => Self::PortConnectionsQuery(body),
            DownFrame::Ack(body) => Self::Ack(body),
            DownFrame::Nak(body) => Self::Nak(body),
            DownFrame::Cmd(body) => Self::Cmd(body),
            DownFrame::CmdCancel(body) => Self::CmdCancel(body),
        }
    }
}

impl From<ChannelText> for Frame {
    fn from(frame: ChannelText) -> Self {
        match frame {
            ChannelText::PtyResize(body) => Self::PtyResize(body),
            ChannelText::PtyExit(body) => Self::PtyExit(body),
        }
    }
}

/// A binary frame of the backplane: data of a tunnel that ends at the
/// system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackplaneData {
    /// The machine at the tunnel's other end.
    pub machine_id: Id,
    /// The channel, by the number the system gave it.
    pub channel: u32,
    /// How many bytes were sent on the channel in this direction before
    /// this frame.
    pub offset: u64,
    /// The data: at least one byte.
    pub payload: Bytes,
}

impl BackplaneData {
    /// The first byte of such a frame.
    pub const KIND: u8 = 2;
    /// The bytes before the data: the kind, the machine's identifier as
    /// sixteen bytes, the channel as four and the offset as eight,
    /// highest byte first.
    pub const HEADER_BYTES: usize = 29;

    /// Writes the frame.
    pub fn encode(&self) -> Result<Bytes, FrameError> {
        if self.payload.is_empty() {
            return Err(FrameError::Empty);
        }
        let mut frame = BytesMut::with_capacity(Self::HEADER_BYTES + self.payload.len());
        frame.put_u8(Self::KIND);
        frame.put_slice(self.machine_id.as_bytes());
        frame.put_u32(self.channel);
        frame.put_u64(self.offset);
        frame.put_slice(&self.payload);
        Ok(frame.freeze())
    }

    /// Reads such a frame. The data is not copied.
    pub fn decode(mut frame: Bytes) -> Result<Self, FrameError> {
        match frame.first() {
            None => return Err(FrameError::Truncated),
            Some(&Self::KIND) => {}
            Some(&other) => return Err(FrameError::UnknownKind(other)),
        }
        if frame.len() < Self::HEADER_BYTES {
            return Err(FrameError::Truncated);
        }
        if frame.len() == Self::HEADER_BYTES {
            return Err(FrameError::Empty);
        }
        frame.advance(1);
        let mut machine = [0u8; 16];
        frame.copy_to_slice(&mut machine);
        let channel = frame.get_u32();
        let offset = frame.get_u64();
        Ok(Self {
            machine_id: Id::from_bytes(machine),
            channel,
            offset,
            payload: frame,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{Tagged, Text, decode, encode};
    use orch_types::Json;
    use serde_json::json;

    const MACHINE: &str = "0199c1a0-0000-7000-8000-00000000000a";
    const SESSION: &str = "0199c1a0-0000-7000-8000-00000000000b";
    const OTHER: &str = "0199c1a0-0000-7000-8000-00000000000c";

    fn round_trip(example: Json) -> BackplaneFrame {
        let text = example.to_string();
        let Text::Known(frame) = decode::<BackplaneFrame>(&text, usize::MAX).unwrap() else {
            panic!("an unknown type: {text}")
        };
        let written: Json = serde_json::from_str(&encode(&frame, usize::MAX).unwrap()).unwrap();
        assert_eq!(written, example);
        assert_eq!(frame.type_name(), example["t"]);
        frame
    }

    #[test]
    fn a_gateway_and_the_system_introduce_themselves() {
        let hello = round_trip(
            json!({ "t": "gw_hello", "gateway_id": "compose-gateway", "version": "0.1.0" }),
        );
        assert!(hello.sent_by(BackplaneSide::Gateway) && !hello.sent_by(BackplaneSide::Api));
        let answer = round_trip(json!({
            "t": "api_hello", "ticket_public_key": "cHVibGljIGtleQ",
            "limits": { "max_frame_bytes": 262144, "tunnel_window_bytes": 1048576, "max_channels": 256 },
            "versions": { "min": "0.1.0", "recommended": "0.1.3" }
        }));
        assert!(answer.sent_by(BackplaneSide::Api) && !answer.sent_by(BackplaneSide::Gateway));
    }

    #[test]
    fn a_gateway_asks_whether_a_session_may_start_and_is_told() {
        round_trip(json!({
            "t": "auth_query", "n": 3, "machine_id": MACHINE, "fingerprint": "9f".repeat(32), "tls": "mutual",
            "ip": "203.0.113.7", "connector_version": "0.1.0", "roles": ["agent", "environment"]
        }));
        round_trip(json!({ "t": "auth_reply", "n": 3, "ok": true, "cursors": { "machine": 12 } }));
        round_trip(json!({ "t": "auth_reply", "n": 4, "ok": false, "code": "revoked" }));
    }

    #[test]
    fn a_gateway_reports_sessions_and_its_own_figures() {
        round_trip(
            json!({ "t": "machine_up", "machine_id": MACHINE, "session_id": SESSION, "ip": "2001:db8::7" }),
        );
        round_trip(
            json!({ "t": "machine_down", "machine_id": MACHINE, "session_id": SESSION, "reason": "replaced" }),
        );
        round_trip(
            json!({ "t": "stats", "sessions": 12, "channels": 31, "bytes_relayed": 9000000000i64 }),
        );
        round_trip(json!({ "t": "kick", "machine_id": MACHINE, "code": "revoked" }));
    }

    #[test]
    fn frames_between_a_machine_and_the_system_cross_wrapped_and_whole() {
        let up = round_trip(json!({
            "t": "up", "machine_id": MACHINE, "session_id": SESSION,
            "frame": { "t": "ev", "s": "machine", "q": 5, "at": "2026-10-08T17:04:05.000Z", "k": "machine.facts", "b": {} }
        }));
        assert!(up.sent_by(BackplaneSide::Gateway));
        let down = round_trip(json!({
            "t": "down", "machine_id": MACHINE, "frame": { "t": "ack", "s": "machine", "q": 5 }
        }));
        assert!(down.sent_by(BackplaneSide::Api));
        round_trip(json!({
            "t": "down", "machine_id": MACHINE,
            "frame": { "t": "cmd", "id": OTHER, "k": "machine.probe", "key": "machine",
                       "deadline": "2026-10-08T17:04:35.000Z", "b": {} }
        }));
    }

    #[test]
    fn only_frames_for_the_system_are_passed_up() {
        let event: Frame = Ev {
            stream: "machine".into(),
            seq: 5,
            at: "2026-10-08T17:04:05.000Z".parse().unwrap(),
            kind: "machine.facts".into(),
            body: json!({}),
        }
        .into();
        assert!(UpFrame::try_from(event).is_ok());
        let ping: Frame = crate::session::Ping { n: 1 }.into();
        assert_eq!(UpFrame::try_from(ping.clone()), Err(Box::new(ping)));
        let down = json!({ "t": "down", "machine_id": MACHINE, "frame": { "t": "ping", "n": 1 } })
            .to_string();
        assert!(matches!(
            decode::<BackplaneFrame>(&down, usize::MAX),
            Err(FrameError::Malformed { .. })
        ));
    }

    #[test]
    fn a_frame_passed_down_is_the_frame_the_machine_gets() {
        let ack = DownFrame::Ack(Ack {
            stream: "machine".into(),
            seq: 5,
        });
        let frame = Frame::from(ack);
        assert_eq!(frame.type_name(), "ack");
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            json!({ "t": "ack", "s": "machine", "q": 5 })
        );
    }

    #[test]
    fn a_tunnel_that_ends_at_the_system_is_opened_fed_and_closed_over_the_backplane() {
        round_trip(
            json!({ "t": "tun_open", "machine_id": MACHINE, "channel": 4, "ticket": "v1.e30.c2ln" }),
        );
        round_trip(
            json!({ "t": "tun_open_ok", "machine_id": MACHINE, "channel": 4, "cid": OTHER }),
        );
        round_trip(
            json!({ "t": "tun_open_err", "machine_id": MACHINE, "channel": 4, "code": "peer_offline", "detail": "" }),
        );
        round_trip(json!({ "t": "tun_credit", "machine_id": MACHINE, "channel": 4, "n": 65536 }));
        round_trip(json!({ "t": "tun_eof", "machine_id": MACHINE, "channel": 4 }));
        round_trip(
            json!({ "t": "tun_close", "machine_id": MACHINE, "channel": 4, "code": "ok", "detail": "" }),
        );
        let resize = round_trip(json!({
            "t": "tun_text", "machine_id": MACHINE, "channel": 4,
            "frame": { "t": "pty_resize", "c": 4, "cols": 120, "rows": 40 }
        }));
        assert!(resize.sent_by(BackplaneSide::Api) && resize.sent_by(BackplaneSide::Gateway));
        round_trip(json!({
            "t": "tun_text", "machine_id": MACHINE, "channel": 4, "frame": { "t": "pty_exit", "c": 4, "code": 130 }
        }));
    }

    #[test]
    fn terminal_data_crosses_the_backplane_with_its_machine_and_channel_in_front() {
        let frame = BackplaneData {
            machine_id: MACHINE.parse().unwrap(),
            channel: 4,
            offset: 258,
            payload: Bytes::from_static(b"ls\n"),
        };
        let bytes = frame.encode().unwrap();
        assert_eq!(bytes.len(), BackplaneData::HEADER_BYTES + 3);
        assert_eq!(bytes[0], 2);
        assert_eq!(&bytes[1..17], frame.machine_id.as_bytes());
        assert_eq!(&bytes[17..29], &[0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 1, 2]);
        assert_eq!(BackplaneData::decode(bytes).unwrap(), frame);
    }

    #[test]
    fn bytes_that_are_no_backplane_data_are_refused() {
        assert!(matches!(
            BackplaneData::decode(Bytes::from(vec![1u8; 40])),
            Err(FrameError::UnknownKind(1))
        ));
        assert!(matches!(
            BackplaneData::decode(Bytes::from(vec![2u8; 28])),
            Err(FrameError::Truncated)
        ));
        assert!(matches!(
            BackplaneData::decode(Bytes::from(vec![2u8; 29])),
            Err(FrameError::Empty)
        ));
        let empty = BackplaneData {
            machine_id: MACHINE.parse().unwrap(),
            channel: 4,
            offset: 0,
            payload: Bytes::new(),
        };
        assert!(matches!(empty.encode(), Err(FrameError::Empty)));
    }
}
