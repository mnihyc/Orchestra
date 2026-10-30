//! Environment-scoped port reads and assertion refresh over the authenticated live machine link.

use orch_api_contract::ports::{
    EnvironmentPortListQuery, EnvironmentPorts, PortAssertionKeys, PortConnectionInside, PortPeer,
    PortTarget, PortTransport,
};
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The ordinary live-port read window; a caller never waits indefinitely for its signer.
pub const ANSWER_WITHIN_SECS: u64 = 10;
/// The same sixteen waiting live queries per environment as the carrier's read path.
pub const PENDING_PER_ENVIRONMENT: usize = 16;
/// Local programs may have sixty-four simultaneous lookups, including coalesced peer refreshes.
pub const LOCAL_CONCURRENT: usize = 64;
/// One minute of sixty reads a second, per environment, with no unbounded caller-key collection.
pub const LOCAL_PER_MINUTE: u32 = 3_600;
/// A compact current two-end row and its short-lived assertion fit in four thousand bytes.
pub const PEER_REPLY_BYTES: usize = 4_000;

/// One read-only act; no request can open/close a port or choose a person to impersonate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortLocalRead {
    /// Current canonical list, without any link proof.
    List {
        /// The same filters and whole-row continuation as the human/agent list.
        query: EnvironmentPortListQuery,
    },
    /// A stable target, including one which has not started listening yet.
    Address {
        /// The nonzero listener number.
        port: u16,
    },
    /// Refresh only a connection the local carrier currently holds; the API verifies its own record.
    Peer {
        /// The admitted use's reference, never an arbitrary caller identity.
        connection_id: Id,
        /// The program's listener number.
        port: u16,
        /// The actual transport of the local socket.
        transport: PortTransport,
        /// The exact native peer seen by this program before optional PROXY parsing.
        inside: PortConnectionInside,
    },
    /// Public verification context only; no private signing key crosses the link.
    Keys,
}

/// The gateway supplies machine/session identity outside this body; its environment cannot name another machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortLocalAsk {
    /// Ephemeral correlation, never a durable command/idempotency row.
    pub request_id: Id,
    /// The environment attached on this authenticated machine.
    pub environment_id: Id,
    /// Its current runtime generation.
    pub generation: i64,
    /// The read to make under that scope.
    pub read: PortLocalRead,
}

/// A canonical read or a current signed peer, without a link proof or private signer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PortLocalData {
    /// The existing list DTO.
    List {
        /// Current list data; a continuation remains the canonical continuation.
        ports: EnvironmentPorts,
    },
    /// The existing target DTO.
    Address {
        /// Target facts and stable entry addresses.
        target: PortTarget,
    },
    /// A current exact match, or a definite nonmatch; an unavailable signer uses a failure instead.
    Peer {
        /// A matched active connection and fresh assertion, never an ended/reused row.
        peer: Option<PortPeer>,
        /// Public context for independently checking the assertion.
        keys: PortAssertionKeys,
    },
    /// Public verification context.
    Keys {
        /// The issuer, target-specific audience template and public keys.
        keys: PortAssertionKeys,
    },
}

/// A failed read is not a definite nonmatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortLocalFailure {
    /// A safe stable refusal, such as ports.unavailable.
    pub code: String,
    /// A sentence without tokens, request headers or traffic content.
    pub detail: String,
}

/// The one answer to an ephemeral read; data and failure are mutually exclusive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortLocalAnswer {
    /// The read answered.
    pub request_id: Id,
    /// Current data on success, including an explicit null peer only for a definite nonmatch.
    pub data: Option<PortLocalData>,
    /// A refused/unavailable read, never disguised as null peer.
    pub failure: Option<PortLocalFailure>,
}

impl PortLocalAnswer {
    /// An unavailable live read whose caller must not infer a missing peer.
    #[must_use]
    pub fn unavailable(request_id: Id, detail: &str) -> Self {
        Self {
            request_id,
            data: None,
            failure: Some(PortLocalFailure {
                code: "ports.unavailable".into(),
                detail: detail.into(),
            }),
        }
    }
}
