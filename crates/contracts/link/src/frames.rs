//! Text and binary frames of a session: their names, their coding and their size.
//!
//! A session carries two sorts of frame. A text frame is one JSON object
//! whose member `t` says what it is: everything that is said about the
//! session itself, about streams, about commands and about channels. A
//! binary frame is tunnel data: a few bytes that say which channel and
//! where in its stream, then the data.
//!
//! A text frame of a type nobody knows is passed over, so that a newer
//! peer can say more than an older one understands. A binary frame of a
//! kind nobody knows ends the session, because nothing can be made of
//! bytes without knowing what they are.

use crate::commands::{Cmd, CmdAck, CmdCancel, CmdRes};
use crate::session::{Auth, Bye, Hello, Ping, Pong, Ready, Reject};
use crate::streams::{Ack, Eph, Ev, Evb, Nak, Reset};
use crate::tunnels::{
    ChAck, Close, Credit, Eof, Open, OpenErr, OpenOk, PtyExit, PtyResize, Resume, ResumeOk,
};
use crate::version::ByeCode;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use orch_types::Json;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The member of a text frame that says what it is.
pub const TYPE_MEMBER: &str = "t";

/// A family of text frames that are told apart by their type.
pub trait Tagged: Serialize + DeserializeOwned {
    /// Every type of the family, as it is written.
    const TYPES: &'static [&'static str];

    /// The type of this frame, as it is written.
    fn type_name(&self) -> &'static str;
}

/// Declares a family of text frames: an enum with one variant for each
/// type, each holding the frame's members.
macro_rules! tagged {
    (
        $(#[$family_doc:meta])*
        $name:ident { $( $(#[$doc:meta])* $variant:ident($body:ty) = $tag:literal, )* }
    ) => {
        $(#[$family_doc])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "t")]
        #[allow(
            clippy::large_enum_variant,
            reason = "a frame is read once and handed on; boxing its largest type would cost every reader an indirection"
        )]
        pub enum $name {
            $( $(#[$doc])* #[serde(rename = $tag)] $variant($body), )*
        }

        impl $crate::frames::Tagged for $name {
            const TYPES: &'static [&'static str] = &[ $( $tag, )* ];

            fn type_name(&self) -> &'static str {
                match self { $( Self::$variant(_) => $tag, )* }
            }
        }

        $(
            impl From<$body> for $name {
                fn from(body: $body) -> Self {
                    Self::$variant(body)
                }
            }
        )*
    };
}
pub(crate) use tagged;

tagged! {
    /// A text frame of a session between a machine and a gateway.
    Frame {
        /// The gateway's first word on a new connection.
        Hello(Hello) = "hello",
        /// The machine's answer: who it is.
        Auth(Auth) = "auth",
        /// The gateway's word that the session stands.
        Ready(Ready) = "ready",
        /// The gateway's refusal of the session.
        Reject(Reject) = "reject",
        /// The gateway's word that it ends the session.
        Bye(Bye) = "bye",
        /// A question whether the other side is there.
        Ping(Ping) = "ping",
        /// The answer to it.
        Pong(Pong) = "pong",
        /// The gateway's current authority signal for port channels only.
        PortControl(crate::ports::PortControl) = "port_control",
        /// One event of a stream.
        Ev(Ev) = "ev",
        /// Several events of one stream.
        Evb(Evb) = "evb",
        /// The system's word that it has taken a stream in through a
        /// number.
        Ack(Ack) = "ack",
        /// The system's word that events of a stream are missing.
        Nak(Nak) = "nak",
        /// A machine's word that events of a stream are gone.
        Reset(Reset) = "reset",
        /// A measurement that is not kept.
        Eph(Eph) = "eph",
        /// A command.
        Cmd(Cmd) = "cmd",
        /// A machine's word that it has a command.
        CmdAck(CmdAck) = "cmd_ack",
        /// How a command ended.
        CmdRes(CmdRes) = "cmd_res",
        /// The system's wish that a command not be carried out.
        CmdCancel(CmdCancel) = "cmd_cancel",
        /// The local machine owner asks the system to file an imported MCP copy.
        McpImportAsk(crate::mcp_import::McpImportAsk) = "mcp_import_ask",
        /// The system answers that local import, including a safe refusal.
        McpImportAnswer(crate::mcp_import::McpImportAnswer) = "mcp_import_answer",
        /// An environment-local read/peer assertion request, never a retained command.
        PortLocalAsk(crate::local_ports::PortLocalAsk) = "port_local_ask",
        /// Current inside read data or an explicit unavailable/refused answer.
        PortLocalAnswer(crate::local_ports::PortLocalAnswer) = "port_local_answer",
        /// A current query of the host node's bounded port record, never a retained command.
        PortConnectionsQuery(crate::ports::PortConnectionsQuery) = "port_connections_query",
        /// The node's current numeric/caller facts.
        PortConnectionsReply(crate::ports::PortConnectionsReply) = "port_connections_reply",
        /// The opening of a channel.
        Open(Open) = "open",
        /// The word that a channel is open.
        OpenOk(OpenOk) = "open_ok",
        /// The word that a channel was not opened.
        OpenErr(OpenErr) = "open_err",
        /// More credit for the sender of a channel.
        Credit(Credit) = "credit",
        /// The end of one direction of a channel.
        Eof(Eof) = "eof",
        /// The end of a channel.
        Close(Close) = "close",
        /// An acknowledgement of bytes received on a channel.
        ChAck(ChAck) = "ch_ack",
        /// The request to carry a tunnel on from a new session.
        Resume(Resume) = "resume",
        /// The word that a tunnel goes on.
        ResumeOk(ResumeOk) = "resume_ok",
        /// A terminal's new size.
        PtyResize(PtyResize) = "pty_resize",
        /// The end of the process in a terminal.
        PtyExit(PtyExit) = "pty_exit",
    }
}

/// What a text frame is about, which decides who deals with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// The session itself: its start, its end and its liveness. Machine
    /// and gateway settle these between them.
    Session,
    /// A stream of events, or a measurement beside one. These pass
    /// between a machine and the system; a gateway carries them.
    Stream,
    /// A command. These pass between the system and a machine; a
    /// gateway carries them.
    Command,
    /// A channel of a tunnel. A gateway relays these between the two
    /// ends.
    Tunnel,
}

impl Frame {
    /// What the frame is about.
    #[must_use]
    pub const fn family(&self) -> Family {
        match self {
            Self::Hello(_)
            | Self::Auth(_)
            | Self::Ready(_)
            | Self::Reject(_)
            | Self::Bye(_)
            | Self::Ping(_)
            | Self::Pong(_)
            | Self::PortControl(_) => Family::Session,
            Self::Ev(_)
            | Self::Evb(_)
            | Self::Ack(_)
            | Self::Nak(_)
            | Self::Reset(_)
            | Self::Eph(_) => Family::Stream,
            Self::Cmd(_)
            | Self::CmdAck(_)
            | Self::CmdRes(_)
            | Self::CmdCancel(_)
            | Self::McpImportAsk(_)
            | Self::McpImportAnswer(_) => Family::Command,
            Self::PortConnectionsQuery(_)
            | Self::PortConnectionsReply(_)
            | Self::PortLocalAsk(_)
            | Self::PortLocalAnswer(_) => Family::Command,
            Self::Open(_)
            | Self::OpenOk(_)
            | Self::OpenErr(_)
            | Self::Credit(_)
            | Self::Eof(_)
            | Self::Close(_)
            | Self::ChAck(_)
            | Self::Resume(_)
            | Self::ResumeOk(_)
            | Self::PtyResize(_)
            | Self::PtyExit(_) => Family::Tunnel,
        }
    }
}

/// A text frame as it was read.
#[derive(Debug, Clone, PartialEq)]
pub enum Text<F> {
    /// A frame of a known type.
    Known(F),
    /// A frame of a type this build does not know, which is passed over.
    Unknown(String),
}

/// Why something is not a frame. Every one of these means that the
/// other side broke the protocol.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// It is larger than a frame may be.
    #[error("a frame of {bytes} bytes where {max} are the most")]
    TooLarge {
        /// Its size.
        bytes: usize,
        /// The most a frame may have.
        max: usize,
    },
    /// A text frame is not JSON.
    #[error("the frame is not JSON: {0}")]
    NotJson(#[source] serde_json::Error),
    /// A text frame is not an object that says what it is.
    #[error("the frame is not an object with a type")]
    Untyped,
    /// A text frame of a known type does not have that type's members.
    #[error("the frame `{kind}` does not have the members of its type: {source}")]
    Malformed {
        /// Its type.
        kind: String,
        /// What is wrong with it.
        source: serde_json::Error,
    },
    /// A frame could not be written.
    #[error("the frame cannot be written: {0}")]
    Unwritable(#[source] serde_json::Error),
    /// A binary frame is of a kind nobody knows.
    #[error("{0} is not a kind of binary frame")]
    UnknownKind(u8),
    /// A binary frame ends before its header does.
    #[error("the binary frame is shorter than its header")]
    Truncated,
    /// A binary frame carries no data.
    #[error("the binary frame carries no data")]
    Empty,
}

impl FrameError {
    /// The code a session is ended with over this.
    #[must_use]
    pub const fn bye(&self) -> ByeCode {
        ByeCode::ProtocolError
    }
}

/// Writes a text frame, which may have `max_frame_bytes` at most.
pub fn encode<F: Tagged>(frame: &F, max_frame_bytes: usize) -> Result<String, FrameError> {
    let text = serde_json::to_string(frame).map_err(FrameError::Unwritable)?;
    if text.len() > max_frame_bytes {
        return Err(FrameError::TooLarge {
            bytes: text.len(),
            max: max_frame_bytes,
        });
    }
    Ok(text)
}

/// Reads a text frame, which may have `max_frame_bytes` at most.
pub fn decode<F: Tagged>(text: &str, max_frame_bytes: usize) -> Result<Text<F>, FrameError> {
    if text.len() > max_frame_bytes {
        return Err(FrameError::TooLarge {
            bytes: text.len(),
            max: max_frame_bytes,
        });
    }
    let value: Json = serde_json::from_str(text).map_err(FrameError::NotJson)?;
    let kind = value
        .get(TYPE_MEMBER)
        .and_then(Json::as_str)
        .ok_or(FrameError::Untyped)?
        .to_owned();
    if !F::TYPES.contains(&kind.as_str()) {
        return Ok(Text::Unknown(kind));
    }
    serde_json::from_value(value)
        .map(Text::Known)
        .map_err(|source| FrameError::Malformed { kind, source })
}

/// A binary frame of a session: data of one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFrame {
    /// The channel, by the number it has on this session.
    pub channel: u32,
    /// How many bytes were sent on the channel in this direction before
    /// this frame.
    pub offset: u64,
    /// The data: at least one byte.
    pub payload: Bytes,
}

impl DataFrame {
    /// The first byte of a frame of tunnel data.
    pub const KIND: u8 = 1;
    /// The bytes before the data: the kind, the channel as four bytes
    /// and the offset as eight, highest byte first.
    pub const HEADER_BYTES: usize = 13;

    /// The most data one frame carries when a frame may have
    /// `max_frame_bytes`.
    #[must_use]
    pub const fn max_payload(max_frame_bytes: usize) -> usize {
        max_frame_bytes.saturating_sub(Self::HEADER_BYTES)
    }

    /// The bytes before the data, for a writer that sends header and
    /// data without joining them.
    #[must_use]
    pub fn header(&self) -> [u8; Self::HEADER_BYTES] {
        let mut header = [0u8; Self::HEADER_BYTES];
        header[0] = Self::KIND;
        header[1..5].copy_from_slice(&self.channel.to_be_bytes());
        header[5..].copy_from_slice(&self.offset.to_be_bytes());
        header
    }

    /// Writes the frame, which may have `max_frame_bytes` at most.
    pub fn encode(&self, max_frame_bytes: usize) -> Result<Bytes, FrameError> {
        let bytes = Self::HEADER_BYTES + self.payload.len();
        if self.payload.is_empty() {
            return Err(FrameError::Empty);
        }
        if bytes > max_frame_bytes {
            return Err(FrameError::TooLarge {
                bytes,
                max: max_frame_bytes,
            });
        }
        let mut frame = BytesMut::with_capacity(bytes);
        frame.put_slice(&self.header());
        frame.put_slice(&self.payload);
        Ok(frame.freeze())
    }

    /// Reads a binary frame, which may have `max_frame_bytes` at most.
    /// The data is not copied.
    pub fn decode(mut frame: Bytes, max_frame_bytes: usize) -> Result<Self, FrameError> {
        match frame.first() {
            None => return Err(FrameError::Truncated),
            Some(&Self::KIND) => {}
            Some(&other) => return Err(FrameError::UnknownKind(other)),
        }
        if frame.len() > max_frame_bytes {
            return Err(FrameError::TooLarge {
                bytes: frame.len(),
                max: max_frame_bytes,
            });
        }
        if frame.len() < Self::HEADER_BYTES {
            return Err(FrameError::Truncated);
        }
        if frame.len() == Self::HEADER_BYTES {
            return Err(FrameError::Empty);
        }
        frame.advance(1);
        let channel = frame.get_u32();
        let offset = frame.get_u64();
        Ok(Self {
            channel,
            offset,
            payload: frame,
        })
    }

    /// The offset of the byte after this frame's last.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.payload.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streams::EvbEntry;
    use crate::version::MAX_FRAME_BYTES;
    use serde_json::json;

    const MAX: usize = MAX_FRAME_BYTES as usize;
    const A: &str = "0199c1a0-0000-7000-8000-00000000000a";
    const B: &str = "0199c1a0-0000-7000-8000-00000000000b";

    #[test]
    fn an_inside_read_has_only_environment_scope_and_its_answer_has_no_command_journal_effect() {
        let request = json!({"t":"port_local_ask","request_id":A,"environment_id":B,"generation":7,
            "read":{"kind":"peer","connection_id":A,"port":8080,"transport":"tcp","inside":{"address":"127.0.0.1","port":43123}}});
        assert_eq!(round_trip(request.clone()).family(), Family::Command);
        let mut forged = request.clone();
        forged["machine_id"] = json!(A);
        assert!(serde_json::from_value::<Frame>(forged).is_err());
        let mut forged = request;
        forged["read"]["caller"] = json!({"kind":"person","id":A});
        assert!(serde_json::from_value::<Frame>(forged).is_err());
        let answer = json!({"t":"port_local_answer","request_id":A,"data":null,
            "failure":{"code":"ports.unavailable","detail":"the current signer is unavailable"}});
        assert_eq!(round_trip(answer).family(), Family::Command);
    }

    /// Reads `example` as a frame, writes it again and expects the same JSON.
    fn round_trip(example: Json) -> Frame {
        let text = example.to_string();
        let Text::Known(frame) = decode::<Frame>(&text, MAX).unwrap() else {
            panic!("an unknown type: {text}")
        };
        let written: Json = serde_json::from_str(&encode(&frame, MAX).unwrap()).unwrap();
        assert_eq!(written, example);
        assert_eq!(frame.type_name(), example["t"]);
        frame
    }

    #[test]
    fn the_frames_of_a_handshake_read_and_write_as_the_protocol_has_them() {
        let frames = [
            json!({ "t": "hello", "protocol": { "min": 1, "max": 1 }, "nonce": "bm9uY2U",
                    "server_time": "2026-10-08T17:04:05.000Z", "gateway_id": "compose-gateway", "tls": "mutual" }),
            json!({ "t": "auth", "machine_id": A, "protocol_version": 1, "connector_version": "0.1.0",
                    "platform": { "os": "linux", "arch": "x86_64" }, "roles": ["agent", "environment"],
                    "sent_at": "2026-10-08T17:04:05.120Z" }),
            json!({ "t": "ready", "session_id": B, "session_token": "t0k3n", "heartbeat_secs": 20,
                    "limits": { "max_frame_bytes": 262144, "tunnel_window_bytes": 1048576, "max_channels": 256 },
                    "cursors": { "machine": 12 }, "versions": { "min": "0.1.0", "recommended": "0.1.3" } }),
            json!({ "t": "reject", "code": "busy", "detail": "the API is unreachable", "retry_after_secs": 5 }),
            json!({ "t": "bye", "code": "replaced" }),
            json!({ "t": "ping", "n": 7 }),
            json!({ "t": "pong", "n": 7 }),
        ];
        for frame in frames {
            assert_eq!(round_trip(frame).family(), Family::Session);
        }
    }

    #[test]
    fn the_frames_of_streams_read_and_write_as_the_protocol_has_them() {
        let stream = format!("session:{A}:{B}");
        let frames = [
            json!({ "t": "ev", "s": stream, "q": 41, "at": "2026-10-08T17:04:05.000Z", "k": "session.title",
                    "b": { "title": "Fix login" } }),
            json!({ "t": "evb", "s": stream, "e": [
                { "q": 42, "at": "2026-10-08T17:04:05.100Z", "k": "item.delta",
                  "b": { "native_ref": "i9", "field": "text", "offset": 0, "len": 2, "data": "Hi" } } ] }),
            json!({ "t": "ack", "s": stream, "q": 43 }),
            json!({ "t": "nak", "s": stream, "expected": 44 }),
            json!({ "t": "reset", "s": stream, "from": 50, "reason": "spool_lost" }),
            json!({ "t": "eph", "s": format!("env:{A}"), "k": "stats.sample", "at": "2026-10-08T17:04:05.000Z",
                    "b": { "cpu_percent": 12.5 } }),
        ];
        for frame in frames {
            assert_eq!(round_trip(frame).family(), Family::Stream);
        }
    }

    #[test]
    fn the_frames_of_commands_read_and_write_as_the_protocol_has_them() {
        let frames = [
            json!({ "t": "cmd", "id": A, "k": "session.close", "key": B, "deadline": "2026-10-08T17:04:35.000Z",
                    "b": { "thread_id": B } }),
            json!({ "t": "cmd_ack", "id": A }),
            json!({ "t": "cmd_res", "id": A, "ok": true, "b": {} }),
            json!({ "t": "cmd_res", "id": A, "ok": false,
                    "err": { "code": "timeout", "detail": "the harness did not answer", "retryable": true } }),
            json!({ "t": "cmd_cancel", "id": A }),
            json!({"t":"mcp_import_ask","request_id":A,"environment_id":B,"source":"codex","scope":"user","directory":null,"name":"files"}),
            json!({"t":"mcp_import_answer","request_id":A,"server_id":null,"code":"policy.mcp_import_disabled","detail":"this machine permits no MCP imports"}),
        ];
        for frame in frames {
            assert_eq!(round_trip(frame).family(), Family::Command);
        }
    }

    #[test]
    fn the_frames_of_tunnels_read_and_write_as_the_protocol_has_them() {
        let claims = json!({ "jti": B, "kind": "exec", "src": A, "dst": B, "service": "exec/codex-exec-server",
                             "thread_id": B, "exp": 1791500000 });
        let frames = [
            json!({ "t": "open", "c": 7, "ticket": "v1.e30.c2ln" }),
            json!({ "t": "open", "c": 12, "kind": "exec", "service": "exec/codex-exec-server", "from": A,
                    "claims": claims }),
            json!({ "t": "open_ok", "c": 12, "cid": B }),
            json!({ "t": "open_err", "c": 7, "code": "no_such_service", "detail": "no such executor" }),
            json!({ "t": "credit", "c": 7, "n": 65536 }),
            json!({ "t": "eof", "c": 7 }),
            json!({ "t": "close", "c": 7, "code": "ok", "detail": "" }),
            json!({ "t": "ch_ack", "c": 7, "o": 262144 }),
            json!({ "t": "resume", "c": 9, "ticket": "v1.e30.c2ln", "jti_channel": B, "recv": 4096 }),
            json!({ "t": "resume_ok", "c": 14, "recv": 8192 }),
            json!({ "t": "pty_resize", "c": 4, "cols": 120, "rows": 40 }),
            json!({ "t": "pty_exit", "c": 4, "code": 0 }),
        ];
        for frame in frames {
            assert_eq!(round_trip(frame).family(), Family::Tunnel);
        }
    }

    #[test]
    fn a_text_frame_of_a_type_nobody_knows_is_passed_over() {
        let read = decode::<Frame>(r#"{"t":"weather","sky":"clear"}"#, MAX).unwrap();
        assert_eq!(read, Text::Unknown("weather".into()));
    }

    #[test]
    fn a_text_frame_that_says_nothing_about_what_it_is_breaks_the_protocol() {
        assert!(matches!(
            decode::<Frame>("not json", MAX),
            Err(FrameError::NotJson(_))
        ));
        assert!(matches!(
            decode::<Frame>(r#"{"n":1}"#, MAX),
            Err(FrameError::Untyped)
        ));
        assert!(matches!(
            decode::<Frame>(r#"["ping"]"#, MAX),
            Err(FrameError::Untyped)
        ));
        assert!(matches!(
            decode::<Frame>(r#"{"t":7}"#, MAX),
            Err(FrameError::Untyped)
        ));
    }

    #[test]
    fn a_text_frame_of_a_known_type_without_its_members_breaks_the_protocol() {
        let error = decode::<Frame>(r#"{"t":"ping"}"#, MAX).unwrap_err();
        assert!(matches!(&error, FrameError::Malformed { kind, .. } if kind == "ping"));
        assert_eq!(error.bye(), ByeCode::ProtocolError);
    }

    #[test]
    fn a_text_frame_larger_than_the_session_allows_is_neither_written_nor_read() {
        let frame = Frame::Ping(Ping { n: 1_000_000 });
        let text = encode(&frame, MAX).unwrap();
        assert!(matches!(
            encode(&frame, text.len() - 1),
            Err(FrameError::TooLarge { .. })
        ));
        assert!(matches!(
            decode::<Frame>(&text, text.len() - 1),
            Err(FrameError::TooLarge { .. })
        ));
        assert!(decode::<Frame>(&text, text.len()).is_ok());
    }

    #[test]
    fn a_batch_is_counted_to_the_byte_before_it_is_written() {
        let stream = format!("session:{A}:{B}");
        let events: Vec<EvbEntry> = (0..5)
            .map(|place| EvbEntry {
                seq: 40 + place,
                at: "2026-10-08T17:04:05.000Z".parse().unwrap(),
                kind: "item.delta".into(),
                body: json!({ "native_ref": "i9", "field": "text", "offset": place, "len": 3, "data": "abc" }),
            })
            .collect();
        let sizes: Vec<usize> = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap().len())
            .collect();
        for count in 0..=events.len() {
            let frame = Frame::Evb(Evb {
                stream: stream.clone(),
                events: events[..count].to_vec(),
            });
            assert_eq!(
                Evb::frame_bytes(&stream, &sizes[..count]),
                encode(&frame, MAX).unwrap().len()
            );
        }
        let three = Evb::frame_bytes(&stream, &sizes[..3]);
        assert_eq!(Evb::fitting(&stream, &sizes, three), 3);
        assert_eq!(Evb::fitting(&stream, &sizes, three - 1), 2);
        assert_eq!(
            Evb::fitting(&stream, &sizes, 10),
            0,
            "not even the first event fits"
        );
        assert_eq!(Evb::fitting(&stream, &sizes, MAX), 5);
    }

    #[test]
    fn tunnel_data_is_written_with_its_channel_and_its_place_in_front() {
        let frame = DataFrame {
            channel: 7,
            offset: 0x0102_0304_0506_0708,
            payload: Bytes::from_static(b"data"),
        };
        let bytes = frame.encode(MAX).unwrap();
        assert_eq!(
            &bytes[..],
            &[
                1, 0, 0, 0, 7, 1, 2, 3, 4, 5, 6, 7, 8, b'd', b'a', b't', b'a'
            ]
        );
        assert_eq!(DataFrame::decode(bytes, MAX).unwrap(), frame);
        assert_eq!(frame.end(), 0x0102_0304_0506_0708 + 4);
    }

    #[test]
    fn a_data_frame_carries_at_least_one_byte_and_at_most_what_the_session_allows() {
        let most = DataFrame::max_payload(MAX);
        assert_eq!(most, 262_131);
        let full = DataFrame {
            channel: 1,
            offset: 0,
            payload: Bytes::from(vec![0u8; most]),
        };
        assert_eq!(full.encode(MAX).unwrap().len(), MAX);
        let over = DataFrame {
            channel: 1,
            offset: 0,
            payload: Bytes::from(vec![0u8; most + 1]),
        };
        assert!(matches!(over.encode(MAX), Err(FrameError::TooLarge { .. })));
        let none = DataFrame {
            channel: 1,
            offset: 0,
            payload: Bytes::new(),
        };
        assert!(matches!(none.encode(MAX), Err(FrameError::Empty)));
        assert!(matches!(
            DataFrame::decode(Bytes::from(vec![1u8; 13]), MAX),
            Err(FrameError::Empty)
        ));
        assert!(matches!(
            DataFrame::decode(Bytes::from(vec![1u8; 12]), MAX),
            Err(FrameError::Truncated)
        ));
        assert!(matches!(
            DataFrame::decode(Bytes::from(vec![1u8; MAX + 1]), MAX),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_binary_frame_of_a_kind_nobody_knows_ends_the_session() {
        let error = DataFrame::decode(
            Bytes::from_static(&[9, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 42]),
            MAX,
        )
        .unwrap_err();
        assert!(matches!(error, FrameError::UnknownKind(9)));
        assert_eq!(error.bye(), ByeCode::ProtocolError);
        assert!(matches!(
            DataFrame::decode(Bytes::new(), MAX),
            Err(FrameError::Truncated)
        ));
    }
}
