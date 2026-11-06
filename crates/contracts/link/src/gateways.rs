//! Operator-advertised gateways, shared by enrollment and live connections.

use crate::enrollment::Endpoint;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Read-only discovery beside the verified enrollment entrance.
pub const DISCOVERY_PATH: &str = "/link/v1/gateways";
/// Gateway-local authenticated round-trip probe, independent of the API backplane.
pub const PROBE_PATH: &str = "/link/v1/probe";
/// A fresh random nonce in each round-trip request.
pub const PROBE_NONCE_HEADER: &str = "x-orch-probe-nonce";
/// Domain separation for a gateway's proof through a public TLS edge.
pub const PROBE_CONTEXT: &str = "orch-gateway-probe-v1";

/// How this connector's primary gateway is chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GatewaySelection {
    /// Measure verified entrances at enrollment and every reconnect.
    #[default]
    Automatic,
    /// Keep the gateway the local operator selected, even while it is offline.
    Selected,
}

/// A local preference, separate from system advertisements and endpoint overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GatewayChoice {
    /// Automatic or a fixed selected gateway.
    pub selection: GatewaySelection,
    /// Required only for a fixed selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
}

/// A gateway's proof of identity and possession of the key, bound to one probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayProbeReply {
    /// The logical gateway this entrance reaches.
    pub gateway_id: String,
    /// The request nonce, repeated exactly.
    pub nonce: String,
    /// The gateway service certificate under the installation's CA.
    pub certificate_pem: String,
    /// P-256 signature as unpadded base64url of the 64-byte r/s pair.
    pub signature: String,
}

/// The signed context includes the requested host as well as the nonce and gateway.
#[must_use]
pub fn probe_signing_input(nonce: &str, gateway_id: &str, host: &str) -> String {
    format!("{PROBE_CONTEXT}\n{nonce}\n{gateway_id}\n{host}")
}

/// One verified entrance's median round trip, not a credential or admission grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayMeasurement {
    /// The gateway proved by this entrance.
    pub gateway_id: String,
    /// The actual entrance measured.
    pub endpoint: Endpoint,
    /// Median of three request/reply measurements after TLS setup.
    pub round_trip_ms: u64,
}

/// One logical gateway. Its entrances all reach the same local session registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayInstance {
    /// Stable operator-selected identity, unchanged when an entrance moves.
    pub id: String,
    /// The operator's name shown in the selector.
    pub label: String,
    /// Descriptive region, never a geolocation or authorization rule.
    pub region: String,
    /// Verified pinned/public entrances of this gateway, in preference order.
    pub endpoints: Vec<Endpoint>,
}

/// Installation metadata, with the same CA pin verification as enrollment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayDiscovery {
    /// Public certificate of the installation's authority, never its private key.
    pub ca_pem: String,
    /// The gateways the operator advertises to this installation's machines.
    pub gateways: Vec<GatewayInstance>,
}
