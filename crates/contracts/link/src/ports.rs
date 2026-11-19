//! Live port observation requests and replies, separate from retained command results.

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::True;

/// The native reader's platform support, without implying a live observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortsDiscovery {
    /// Current scoped native discovery is supported on this platform.
    Native,
    /// Current ownership cannot be established on this platform.
    Unsupported,
}
/// Reader and carrier support proved for this platform and release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentPortsFacts {
    /// The current platform reader support.
    pub discovery: PortsDiscovery,
    /// Only transports supported by both reader and carrier.
    pub transports: Vec<orch_api_contract::ports::PortTransport>,
    /// A truthful unsupported explanation.
    pub reason: Option<String>,
}

/// Requests one fresh native measurement of an attached environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortsLook {
    /// The environment whose current native scope is measured.
    pub environment_id: Id,
    /// The runtime generation expected by this reader.
    pub generation: i64,
    /// The fresh live reply reference, never a retained snapshot.
    pub request_ref: Id,
}

/// A command's acknowledgement, with no retained native measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortsLookResult {
    /// The request was accepted for live measurement.
    pub accepted: True,
}

/// The current native measurement on the ephemeral environment stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortsSnapshot {
    /// The query reference, or null for a live watch measurement.
    pub request_ref: Option<Id>,
    /// The canonical current or truthfully unsupported observation.
    pub snapshot: orch_api_contract::ports::PortSnapshot,
}

/// A signed publication delegates raw-address accepts to its physical publishing machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPortTemplate {
    /// Its Host page when present, otherwise the publishing machine; a correlation reference, never authority.
    pub host_id: Id,
    /// The receiver's dedicated TCP port expects a PROXY v2 prefix at the last hop.
    pub proxy_protocol: bool,
}

/// One outside socket accepted by the authenticated node of a signed host template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPortPeer {
    /// A fresh connection/association reference, distinct from the reusable template.
    pub use_id: Id,
    /// The actual outside socket, never a claimed person or link identity.
    pub peer: orch_api_contract::ports::PortConnectionInside,
}

/// Current authority and the recorded ingress of a short-lived port use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortTicketClaims {
    /// The environment that owns the destination socket.
    pub environment_id: Id,
    /// The current runtime generation, checked at the last hop.
    pub generation: i64,
    /// The local port number; a caller supplies no destination address.
    pub port: u16,
    /// The transport admitted for this channel.
    pub transport: orch_api_contract::ports::PortTransport,
    /// The wider opening, or null for a door holder's own use.
    pub opening_id: Option<Id>,
    /// The admitted audience, signed explicitly so a carrier never guesses it from a caller.
    /// None belongs only to a door holder's own use, with no wider opening.
    #[serde(default)]
    pub audience: Option<orch_api_contract::ports::PortOpeningKind>,
    /// The authority revision under which this use was admitted.
    pub authority_revision: i64,
    /// The host allocation revision, when this use comes from a host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocation_revision: Option<i64>,
    /// Only a signed host publication delegates repeated raw-address accepts to its named node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_template: Option<HostPortTemplate>,
    /// The ingress's proved caller and observed outside peer.
    pub source: orch_api_contract::ports::PortConnectionOutside,
}

/// An unspooled gateway signal that fences current port channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortControl {
    /// False ends channels; true permits fresh reconciliation only.
    pub available: bool,
}

/// A bounded current carrier query that is never journalled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortConnectionsQuery {
    /// The fresh correlation reference.
    pub request_ref: Id,
    /// The environment's current registry.
    pub environment_id: Id,
    /// The runtime generation required by this query.
    pub generation: i64,
    /// The canonical filters and page bounds.
    pub query: orch_api_contract::ports::EnvironmentPortConnectionListQuery,
}

/// A gateway or host node's current bounded connection measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortConnectionsReply {
    /// The query this current measurement answers.
    pub request_ref: Id,
    /// The environment whose memory was read.
    pub environment_id: Id,
    /// The canonical measurement, projected again at the API.
    pub connections: orch_api_contract::ports::PortConnectionList,
}

/// One datagram on a port channel that ends at the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackplanePortDatagram {
    /// The channel's enrolled machine.
    pub machine_id: Id,
    /// The API-local association number.
    pub datagram: PortDatagram,
}

impl BackplanePortDatagram {
    /// Kind, machine, channel and length precede the complete packet.
    pub const HEADER_BYTES: usize = PortDatagram::HEADER_BYTES + 16;

    /// Writes the machine-routed kind3 form without stream offsets.
    pub fn encode(&self) -> Result<bytes::Bytes, crate::frames::FrameError> {
        let packet = self.datagram.encode()?;
        let mut out =
            bytes::BytesMut::with_capacity(Self::HEADER_BYTES + self.datagram.payload.len());
        out.extend_from_slice(&[PortDatagram::KIND]);
        out.extend_from_slice(self.machine_id.as_bytes());
        out.extend_from_slice(&packet[1..]);
        Ok(out.freeze())
    }

    /// Reads the machine-routed kind3 form, preserving zero-length packets.
    pub fn decode(bytes: bytes::Bytes) -> Result<Self, crate::frames::FrameError> {
        if bytes.len() < Self::HEADER_BYTES {
            return Err(crate::frames::FrameError::Truncated);
        }
        let machine_id =
            Id::from_slice(&bytes[1..17]).map_err(|_| crate::frames::FrameError::Truncated)?;
        let mut packet = bytes::BytesMut::with_capacity(bytes.len() - 16);
        packet.extend_from_slice(&bytes[..1]);
        packet.extend_from_slice(&bytes[17..]);
        Ok(Self {
            machine_id,
            datagram: PortDatagram::decode(packet.freeze())?,
        })
    }
}

/// `host.ports`: asks a host's node to check a declaration of host ports against its own ceiling
/// and to say what it would propose; none checks only, and asks only for the proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostPortsCheck {
    /// The declaration to acknowledge, or none.
    pub ports: Option<orch_api_contract::ports::HostPorts>,
    /// The revision the declaration would be saved at.
    pub revision: i64,
}

/// The node's answer: whether the declaration fits its ceiling, the ceiling, and its proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPortsCheckResult {
    /// The revision checked.
    pub revision: i64,
    /// Whether the declaration fits the ceiling (true when none was given).
    pub accepted: bool,
    /// The ceiling of the node's own configuration.
    pub limits: orch_api_contract::ports::HostPortsLimits,
    /// What the node proposes for a host page now.
    pub proposal: orch_api_contract::ports::HostPortsProposal,
    /// Why the declaration does not fit, where it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The ticket a host's node opens one transport's channels with, toward the environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortPublishTicket {
    /// The transport whose channels it opens.
    pub transport: orch_api_contract::ports::PortTransport,
    /// The ticket, signed for this node, which may open channels until it runs out.
    pub ticket: String,
}

/// `port.publish`: binds a host opening's reviewed number on the host for exactly the transports asked, and
/// carries each TCP connection and each UDP sender to the environment on a channel of its own. The same
/// opening at the same revision publishes nothing twice; its tickets are taken anew.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortPublish {
    /// The opening.
    pub opening_id: Id,
    /// The environment it reaches.
    pub environment_id: Id,
    /// The machine that holds the environment, where the channels end.
    pub environment_machine_id: Id,
    /// The environment's runtime generation.
    pub generation: i64,
    /// The port inside the environment.
    pub port: u16,
    /// The transports to bind: the number is reserved for both whichever are bound.
    pub transports: Vec<orch_api_contract::ports::PortTransport>,
    /// The address callers are given.
    pub incoming_address: String,
    /// `all`, or the one local IP to listen on.
    pub listen: orch_api_contract::ports::PortListen,
    /// The reviewed number on the host.
    pub host_port: u16,
    /// The opening's revision.
    pub revision: i64,
    /// The host declaration's revision the number was reviewed under.
    pub allocation_revision: i64,
    /// Whether each TCP connection begins with a PROXY protocol v2 header naming its caller.
    pub proxy_protocol: bool,
    /// One ticket for each transport bound.
    pub tickets: Vec<PortPublishTicket>,
}

/// Where a publication stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortPublishState {
    /// Every transport asked is bound.
    Open,
    /// A binding failed: nothing of the opening is left bound.
    Failed,
}

/// The node's answer to `port.publish`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortPublishResult {
    /// Where it stands.
    pub state: PortPublishState,
    /// The number on the host.
    pub host_port: u16,
    /// The transports bound: all that were asked, or none.
    pub transports: Vec<orch_api_contract::ports::PortTransport>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `port.close`: ends a host opening's listeners and connections on its node, for that revision or an older one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortClose {
    /// The opening.
    pub opening_id: Id,
    /// The environment it reaches.
    pub environment_id: Id,
    /// The generation it was published for.
    pub generation: i64,
    /// The revision to close: a newer publication stays.
    pub revision: i64,
}

/// The node's answer to `port.close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortCloseResult {
    /// Whether anything of that revision was bound and is closed now.
    pub closed: bool,
}

/// The body of `port.state`, sequenced durably on the machine's stream (legacy nodes may use their node stream):
/// a public opening its physical publisher no longer serves because its ports policy forbids it. The system marks
/// the opening unavailable with the reason, and its number free; a later publication may serve it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortState {
    /// The opening.
    pub opening_id: Id,
    /// The revision of the opening that the node held.
    pub revision: i64,
    /// Where it stands on the node.
    pub state: PortPublishState,
    /// Why, as a person reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One unreplayed port datagram on an already admitted association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDatagram {
    /// The association's session-local channel.
    pub channel: u32,
    /// One complete datagram, including an empty one.
    pub payload: bytes::Bytes,
}

impl PortDatagram {
    /// The binary kind reserved for port datagrams.
    pub const KIND: u8 = 3;
    /// Kind, channel and length occupy nine bytes.
    pub const HEADER_BYTES: usize = 9;
    /// A packet fits the common four-KiB slow-link frame floor.
    pub const MAX_BYTES: usize = 4_000;

    /// Encodes one bounded complete datagram.
    pub fn encode(&self) -> Result<bytes::Bytes, crate::frames::FrameError> {
        use bytes::BufMut;
        if self.payload.len() > Self::MAX_BYTES {
            return Err(crate::frames::FrameError::TooLarge {
                bytes: self.payload.len(),
                max: Self::MAX_BYTES,
            });
        }
        let mut out = bytes::BytesMut::with_capacity(Self::HEADER_BYTES + self.payload.len());
        out.put_u8(Self::KIND);
        out.put_u32(self.channel);
        out.put_u32(self.payload.len() as u32);
        out.extend_from_slice(&self.payload);
        Ok(out.freeze())
    }

    /// Decodes the exact frame length without joining or splitting packets.
    pub fn decode(mut bytes: bytes::Bytes) -> Result<Self, crate::frames::FrameError> {
        use crate::frames::FrameError;
        use bytes::Buf;
        if bytes.len() < Self::HEADER_BYTES {
            return Err(FrameError::Truncated);
        }
        let kind = bytes.get_u8();
        if kind != Self::KIND {
            return Err(FrameError::UnknownKind(kind));
        }
        let channel = bytes.get_u32();
        let len = bytes.get_u32() as usize;
        if len > Self::MAX_BYTES {
            return Err(FrameError::TooLarge {
                bytes: len,
                max: Self::MAX_BYTES,
            });
        }
        if bytes.len() != len {
            return Err(FrameError::Truncated);
        }
        Ok(Self {
            channel,
            payload: bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_retained_port_command_result_acknowledges_only_acceptance() {
        let result = PortsLookResult { accepted: True };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({"accepted": true})
        );
        assert!(serde_json::from_value::<PortsLookResult>(json!({"accepted": false})).is_err());
    }

    #[test]
    fn a_fresh_port_query_names_only_the_environment_generation_and_reply_reference() {
        let written = json!({"environment_id":"0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "generation":3,
            "request_ref":"0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c"});
        let request: PortsLook = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), written);
        let mut wrong = written;
        wrong["target_address"] = json!("192.0.2.1");
        assert!(serde_json::from_value::<PortsLook>(wrong).is_err());
    }

    #[test]
    fn udp_keeps_empty_and_complete_packets_and_refuses_oversize_or_false_lengths() {
        use bytes::Bytes;
        for payload in [
            Bytes::new(),
            Bytes::from_static(b"hello"),
            Bytes::from(vec![1; PortDatagram::MAX_BYTES]),
        ] {
            let frame = PortDatagram {
                channel: 7,
                payload,
            };
            assert_eq!(
                PortDatagram::decode(frame.encode().unwrap()).unwrap(),
                frame
            );
        }
        let frame = PortDatagram {
            channel: 1,
            payload: Bytes::from(vec![0; PortDatagram::MAX_BYTES + 1]),
        };
        assert!(frame.encode().is_err());
        assert!(PortDatagram::decode(Bytes::from_static(&[3, 0, 0, 0, 1, 0, 0, 0, 1])).is_err());
        let control: crate::frames::Frame =
            serde_json::from_value(json!({"t":"port_control", "available":false})).unwrap();
        assert_eq!(
            serde_json::to_value(control).unwrap(),
            json!({"t":"port_control", "available":false})
        );
    }
}
