//! Public gateway installation metadata and observed connector and tool carriers.

use crate::operations::{OperationSpec as Op, TargetResolver as Target};
use crate::routes::Access::{Rule, User};
use orch_access_contract::key_scope::{
    KeyAction::{Manage, Read},
    KeyIdentityKind as I,
    KeyTargetKind::{Identity, Object},
};
use orch_access_contract::permissions::ObjectKind as K;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a connector verifies the entrance's server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GatewayEndpointTrust {
    /// The deployment's certificate authority.
    Pinned,
    /// Public HTTPS plus the gateway's deployment-bound application proof.
    Public,
}

/// An entrance with no URL credential, query or fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayEndpoint {
    pub url: String,
    pub trust: GatewayEndpointTrust,
}

/// One gateway and its aliases, in the operator's catalogue order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayCatalogueEntry {
    pub id: String,
    pub label: String,
    pub region: String,
    pub endpoints: Vec<GatewayEndpoint>,
}

/// Effective installation metadata; a read issues no enrollment token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayCatalogue {
    pub revision: String,
    pub gateways: Vec<GatewayCatalogueEntry>,
}

/// Replace the ordered catalogue without moving a healthy connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayCatalogueUpdateBody {
    pub expected_revision: String,
    pub gateways: Vec<GatewayCatalogueEntry>,
}

/// Write receipt. Reading the resulting catalogue remains a separate operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayCatalogueUpdated {
    pub revision: String,
}

/// Persisted primary selection, reported by the actual connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorGatewaySelection {
    Automatic,
    Selected,
}

/// The median of three verified entrance round trips, measured on the connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayLatency {
    pub gateway_id: String,
    pub endpoint: GatewayEndpoint,
    pub round_trip_ms: u64,
}

/// Primary presence is authoritative; selection and samples are the last capable machine report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectorGatewayStatus {
    pub machine_id: Id,
    pub selection: Option<ConnectorGatewaySelection>,
    pub selected_gateway_id: Option<String>,
    pub primary_gateway_id: Option<String>,
    pub connected: bool,
    pub measured_at: Option<Time>,
    pub measurements: Vec<GatewayLatency>,
}

/// Save one primary selection on the enrolled machine without replacing its identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineGatewaySetBody {
    pub selection: ConnectorGatewaySelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
}

/// The accepted native choice, without reading the machine's existing configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineGatewaySetReceipt {
    pub projection: crate::common::ReceiptProjection,
    pub machine_id: Id,
    pub selection: ConnectorGatewaySelection,
    pub gateway_id: Option<String>,
    pub changed: bool,
}

/// Frozen placement beside an unchanged signed tool ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelGatewayRule {
    LowestCombinedRtt,
    ExecutorPinned,
    ExecutorSamplesUnavailable,
    ExecutorPeerCapability,
}

/// Placement evidence is informational and never grants tunnel access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelGatewayDecision {
    pub rule: TunnelGatewayRule,
    pub source_measured_at: Option<Time>,
    pub destination_measured_at: Option<Time>,
    pub max_sample_age_secs: u64,
    pub combined_round_trip_ms: Option<u64>,
}

/// Frozen placement beside an unchanged signed tool ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelGatewayPlan {
    pub decision: TunnelGatewayDecision,
    pub plan_id: Id,
    pub src: Id,
    pub dst: Id,
    pub preferred_gateway: GatewayCatalogueEntry,
    pub common_fallbacks: Vec<GatewayCatalogueEntry>,
    pub catalogue_revision: String,
}

/// State of this thread's current tool pair, never inferred from a preferred gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelGatewayState {
    Local,
    Pending,
    Attached,
    Unavailable,
    Unknown,
}

/// Safe transport refusal with the frozen attempted gateway identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelGatewayRefusal {
    pub code: crate::common::ErrorCode,
    pub detail: String,
    pub attempted_gateways: Vec<String>,
}

/// Tool carrier for the current thread binding; model carriers do not overwrite it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadTunnelGatewayStatus {
    pub thread_id: Id,
    pub state: TunnelGatewayState,
    pub source_primary_gateway_id: Option<String>,
    pub destination_primary_gateway_id: Option<String>,
    pub attached_gateway_id: Option<String>,
    pub plan: Option<TunnelGatewayPlan>,
    pub refusal: Option<TunnelGatewayRefusal>,
}

crate::endpoints! {
    /// Reads the public installation catalogue without issuing a token.
    GatewayCatalogueGet: GET "/gateways", User, none, none, json GatewayCatalogue, 200,
        operation Op::single(Read, &[Identity(I::System)], Target::System);
    /// Replaces gateway installation metadata under ordinary system administration.
    GatewayCatalogueUpdate: PUT "/gateways", Rule("system administration"), none, json GatewayCatalogueUpdateBody, json GatewayCatalogueUpdated, 200,
        operation Op::receipt(&Op::single(Manage, &[Identity(I::System)], Target::System));
    /// Reads the machine's primary choice and measured entrance latencies.
    MachineGatewayGet: GET "/machines/{machine_id}/gateway", Rule("machine.view, or host.view on its node facade"), none, none, json ConnectorGatewayStatus, 200,
        operation Op::single(Read, &[Object(K::Machine)], Target::TypedPath { parameter: "machine_id", kind: Object(K::Machine) });
    /// Saves the primary gateway choice on its actual machine; the native reply is durable before reconnect.
    MachineGatewaySet: PUT "/machines/{machine_id}/gateway", Rule("machine.manage, or host.manage on its node facade"), none, json MachineGatewaySetBody, json MachineGatewaySetReceipt, 200,
        operation Op::receipt(&Op::facets(&crate::machine_sources::GATEWAY_FACETS));
    /// Reads the actual tool carrier of the thread's current environment binding.
    ThreadTunnelGatewayGet: GET "/threads/{thread_id}/gateway", Rule("thread.view"), none, none, json ThreadTunnelGatewayStatus, 200,
        operation Op::single(Read, &[Object(K::Thread)], Target::TypedPath { parameter: "thread_id", kind: Object(K::Thread) });
}
