//! Egress modes, the network policy frozen into a runtime's request, and what a driver reports of a unit's network.
//!
//! A unit is `open`: it reaches wherever its network lets it. Or it is `restricted`: it reaches Orch's own
//! transport and the other host names its place allows, through a proxy its host owns, and nothing else. A
//! template whose `network.mode` is `none` is the restricted unit with no other name at all.
//!
//! The policy a runtime is made with is frozen into its request ([`NetworkPolicy`]): the mode, the other names,
//! the trusted Orch endpoints that are always admitted, and a revision of each. The trusted endpoints come from
//! the configuration of the node that runs the unit, never from a template or a caller. Whoever enforces the
//! policy answers what is in force ([`UnitNetwork`]): the revisions installed, whether outward traffic flows, and
//! the last host it refused with why. Nothing of a request (a path, a query, a header, a body) is ever in it.

use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How far a unit may reach outward.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EgressMode {
    /// Wherever its network lets it.
    #[default]
    Open,
    /// Orch's own transport and the other names its place allows, through its host's proxy, and nothing else.
    Restricted,
}

impl EgressMode {
    /// The mode as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Restricted => "restricted",
        }
    }
}

/// What a trusted Orch endpoint is for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TransportPurpose {
    /// The gateway a connector keeps its link to.
    Gateway,
    /// A remote model forwarder.
    Model,
    /// The origin blobs are fetched from.
    Blob,
}

/// One trusted Orch endpoint a restricted unit always reaches: an exact authority, protocol and port, with no
/// path, query, token or credential.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct NetworkTransport {
    /// The host name the unit asks for, lower case.
    pub authority: String,
    /// The protocol spoken over it, as the endpoint's address names it: `wss`, `https`.
    pub protocol: String,
    /// The port.
    pub port: u16,
    /// What it is for.
    pub purpose: TransportPurpose,
}

/// The network policy a runtime is made with: the request's `network_policy`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkPolicy {
    /// The mode it is to have.
    pub mode: EgressMode,
    /// The other host names a restricted unit reaches, canonical (lower case, without a final dot); empty:
    /// only Orch.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// The trusted Orch endpoints a restricted unit always reaches. The node that runs the unit fills them in
    /// from its own configuration; what a request says here is not taken.
    #[serde(default)]
    pub always: Vec<NetworkTransport>,
    /// The revision of the trusted endpoints, as the node computed it.
    #[serde(default)]
    pub always_revision: String,
    /// The revision of the mode and the other names, as the system resolved them.
    #[serde(default)]
    pub revision: String,
}

impl NetworkPolicy {
    /// Whether the unit is to be restricted.
    #[must_use]
    pub const fn restricted(&self) -> bool {
        matches!(self.mode, EgressMode::Restricted)
    }
}

/// Whether a unit's network is as its policy says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkReadiness {
    /// The policy is installed and enforced, and what it admits flows.
    Ready,
    /// The policy is not in force: outward traffic is closed until it is.
    Unavailable,
}

/// The last request a unit's proxy refused: the host it named and why, and nothing of the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkDenial {
    /// The host the request named, as the proxy read it.
    pub host: String,
    /// When.
    pub at: Time,
    /// Why, in a few words.
    pub reason: String,
}

/// What is in force for a unit's network, as the driver that enforces it reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnitNetwork {
    /// The mode the unit has.
    pub mode: EgressMode,
    /// The mode the unit's request or its retained requirement asks for.
    pub required_mode: EgressMode,
    /// The other host names in force.
    pub allowed_hosts: Vec<String>,
    /// The trusted Orch endpoints in force.
    pub always: Vec<NetworkTransport>,
    /// The revision of the trusted endpoints in force.
    pub always_revision: String,
    /// The revision of the other names in force.
    pub revision: String,
    /// Whether the policy is in force.
    pub state: NetworkReadiness,
    /// Why it is not, in a sentence fit to show.
    pub reason: Option<String>,
    /// The last host the proxy refused.
    pub last_denied: Option<NetworkDenial>,
    /// Where the unit's programs find the proxy: `http://<address>:<port>`; none for an open unit.
    pub proxy_url: Option<String>,
    /// The networks the unit and its proxy stand on, each with its subnet, gateway and their address there;
    /// empty where the driver makes none of its own.
    #[serde(default)]
    pub networks: Vec<UnitNetworkAddress>,
}

/// What a unit's network is to it, as the operator's hooks and the environment's facts say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitNetworkRole {
    /// The network of the unit itself: open to the outside for an open unit, or for a restricted one its
    /// private network, whose only other member is its proxy.
    Unit,
    /// The network a restricted unit's proxy goes out on; the unit is not on it.
    Egress,
}

/// One network a unit, or its proxy, stands on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnitNetworkAddress {
    /// The network's name on the host.
    pub name: String,
    /// What it is to the unit.
    pub role: UnitNetworkRole,
    /// Its subnet, as `address/prefix`.
    pub subnet: String,
    /// Its gateway: the host's address on it; `null` for a network whose bridge has no address on the host.
    pub gateway: Option<String>,
    /// The address there of the unit (role `unit`) or of its proxy (role `egress`); `null` while it is not
    /// running.
    pub address: Option<String>,
}

impl UnitNetwork {
    /// An open unit's network: nothing to enforce, and ready.
    #[must_use]
    pub fn open(policy: &NetworkPolicy) -> Self {
        Self {
            mode: EgressMode::Open,
            required_mode: policy.mode,
            allowed_hosts: Vec::new(),
            always: Vec::new(),
            always_revision: policy.always_revision.clone(),
            revision: policy.revision.clone(),
            state: NetworkReadiness::Ready,
            reason: None,
            last_denied: None,
            proxy_url: None,
            networks: Vec::new(),
        }
    }

    /// A unit whose requested policy is not in force, for the reason given.
    #[must_use]
    pub fn unavailable(policy: &NetworkPolicy, reason: impl Into<String>) -> Self {
        Self {
            mode: policy.mode,
            required_mode: policy.mode,
            allowed_hosts: policy.allowed_hosts.clone(),
            always: policy.always.clone(),
            always_revision: policy.always_revision.clone(),
            revision: policy.revision.clone(),
            state: NetworkReadiness::Unavailable,
            reason: Some(reason.into()),
            last_denied: None,
            proxy_url: None,
            networks: Vec::new(),
        }
    }

    /// Whether what is in force is what `policy` asks for, and flows.
    #[must_use]
    pub fn is_ready_for(&self, policy: &NetworkPolicy) -> bool {
        self.state == NetworkReadiness::Ready
            && self.mode == policy.mode
            && (policy.mode == EgressMode::Open || self.revision == policy.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_policy_reads_the_request_member_and_an_absent_one_is_open() {
        let policy: NetworkPolicy = serde_json::from_value(json!({
            "mode": "restricted",
            "allowed_hosts": ["pypi.org"],
            "always": [{ "authority": "gateway.orch.internal", "protocol": "wss", "port": 9443, "purpose": "gateway" }],
            "always_revision": "a1",
            "revision": "b2"
        }))
        .unwrap();
        assert!(policy.restricted());
        assert_eq!(policy.always[0].purpose, TransportPurpose::Gateway);
        assert_eq!(policy.always[0].port, 9443);
        let open: NetworkPolicy = serde_json::from_value(json!({ "mode": "open" })).unwrap();
        assert_eq!(open, NetworkPolicy::default());
    }

    #[test]
    fn an_open_unit_is_ready_and_a_restricted_one_only_at_the_revision_asked_for() {
        let open = NetworkPolicy::default();
        assert!(UnitNetwork::open(&open).is_ready_for(&open));
        let restricted = NetworkPolicy {
            mode: EgressMode::Restricted,
            revision: "r2".to_owned(),
            ..NetworkPolicy::default()
        };
        let mut applied = UnitNetwork::unavailable(&restricted, "applying");
        assert!(!applied.is_ready_for(&restricted));
        applied.state = NetworkReadiness::Ready;
        applied.reason = None;
        assert!(applied.is_ready_for(&restricted));
        applied.revision = "r1".to_owned();
        assert!(
            !applied.is_ready_for(&restricted),
            "an older list is not the one asked for"
        );
    }
}
