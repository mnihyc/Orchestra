//! Operator-advertised gateways, shared by enrollment and live connections.

use crate::enrollment::Endpoint;
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A separate subprotocol prevents an older gateway accepting an auxiliary as a primary.
pub const TUNNEL_SUBPROTOCOL: &str = "orch-link.tunnel.v1";
/// Negotiate this before sending auxiliary machine authentication.
pub const TUNNEL_CAPABILITY: &str = "tunnel_side_attachment";
/// Close unused auxiliaries after a minute; references and live channels protect them.
pub const TUNNEL_IDLE_SECS: u64 = 60;

/// Control remains on the primary; an auxiliary carries only tunnel traffic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionPurpose {
    /// The one authoritative control session.
    #[default]
    Primary,
    /// An explicitly negotiated tunnel-only attachment.
    Tunnel,
}

impl SessionPurpose {
    /// Preserve the absent legacy primary value on the wire.
    #[must_use]
    pub const fn is_primary(&self) -> bool {
        matches!(self, Self::Primary)
    }
}

/// Frozen carrier placement; none of these fields grants tool authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelPlan {
    /// Placement identity.
    pub plan_id: Id,
    /// The ticket's actual authenticated source.
    pub src: Id,
    /// Its actual executor/provider peer.
    pub dst: Id,
    /// The executor's primary by default.
    pub preferred_gateway: GatewayInstance,
    /// Candidates for the next open, in this order.
    pub common_fallbacks: Vec<GatewayInstance>,
    /// The catalogue used to prepare these entrances.
    pub catalogue_revision: String,
}

impl TunnelPlan {
    /// Each attempt traverses one gateway, never a chain.
    pub fn gateways(&self) -> impl Iterator<Item = &GatewayInstance> {
        std::iter::once(&self.preferred_gateway).chain(self.common_fallbacks.iter())
    }
}

/// The original signed ticket with adjacent non-authoritative routing metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutedTicket {
    /// Unchanged v1 capability text.
    pub ticket: String,
    /// Placement designated by the API for this pair.
    pub route: TunnelPlan,
}

/// A legacy string holder asks for placement only when its peer is elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelRouteAsk {
    /// Ephemeral request, not a journalled command.
    pub request_id: Id,
    /// Capability whose source must be the requesting machine.
    pub ticket: String,
}

/// Carrier metadata only, never model or tool bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelRouteAnswer {
    /// Matching request.
    pub request_id: Id,
    /// The prepared placement, if available.
    pub plan: Option<TunnelPlan>,
    /// Safe refusal reason.
    pub detail: Option<String>,
}

/// Have a peer attach to one approved candidate through its primary control session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelPrepareAsk {
    /// Ephemeral source request.
    pub request_id: Id,
    /// Source-bound capability, checked again by the API.
    pub ticket: String,
    /// Frozen source/destination and fallback order.
    pub plan: TunnelPlan,
    /// Logical candidate, never a caller-supplied URL.
    pub gateway_id: String,
}

/// The API's transport-only request to the destination's primary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelAttach {
    /// Pending source request.
    pub request_id: Id,
    /// Approved pair and candidate list.
    pub plan: TunnelPlan,
    /// Logical candidate to attach without changing the primary.
    pub gateway_id: String,
    /// Ticket-bound lifetime of the transport reference.
    pub expires_at: i64,
}

/// Destination acknowledgement to the API, never a business command result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelAttachResult {
    /// Matching forwarded attachment request.
    pub request_id: Id,
    /// The destination transport is ready.
    pub ok: bool,
    /// Safe label/reason on refusal.
    pub detail: Option<String>,
}

/// API answer to the source's preparation request, after destination confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelPrepareAnswer {
    /// Matching original source request, distinct from the forwarded request.
    pub request_id: Id,
    /// Both ends may attempt the designated carrier.
    pub ok: bool,
    /// Safe label/reason on refusal.
    pub detail: Option<String>,
}

/// Release only the references belonging to the authenticated pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelRelease {
    /// Placement to release.
    pub plan_id: Id,
    /// Original source, checked by the API.
    pub src: Id,
    /// Original peer.
    pub dst: Id,
}

/// Actual tool carrier metadata, with the original capability as its source binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelSelected {
    /// Frozen metadata of the carrier actually used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<TunnelPlan>,
    /// Source-bound tool capability.
    pub ticket: String,
    /// Placement that became ready.
    pub plan_id: Id,
    /// Logical gateway that identified itself.
    pub gateway_id: String,
}

/// Source-bound transport failure, with no native request or response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelFailed {
    /// Structured sides let the API compose safe text from its own gateway labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<GatewayAttachFailure>,
    /// Original source/thread/destination capability.
    pub ticket: String,
    /// Attempted frozen placement.
    pub plan: TunnelPlan,
    /// Stable IDs rather than caller-supplied URLs.
    pub attempted_gateways: Vec<String>,
    /// Safe gateway labels and which side failed.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GatewayAttachSide {
    Source,
    Peer,
    Connection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayAttachFailure {
    pub gateway_id: String,
    pub side: GatewayAttachSide,
}

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

/// Last primary probe result reported with durable machine facts, never a live browser probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayStatusReport {
    pub choice: GatewayChoice,
    pub measured_at: Option<orch_types::Time>,
    pub measurements: Vec<GatewayMeasurement>,
}

/// Authenticated metadata refresh; it never requests a primary move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayAdvertisement {
    pub gateways: Vec<GatewayInstance>,
    pub endpoints: Vec<Endpoint>,
}

/// Installation and status metadata must never expose credentials in a legacy URL.
#[must_use]
pub fn public_endpoint(endpoint: &Endpoint) -> Option<Endpoint> {
    let mut address = url::Url::parse(&endpoint.url).ok()?;
    let mut shown = endpoint.clone();
    if !address.username().is_empty()
        || address.password().is_some()
        || address.query().is_some()
        || address.fragment().is_some()
        || !matches!(address.path(), "" | "/")
    {
        address.set_username("").ok()?;
        address.set_password(None).ok()?;
        address.set_query(None);
        address.set_fragment(None);
        address.set_path("");
        shown.url = address.to_string();
    }
    Some(shown)
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
