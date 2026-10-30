//! Protocol negotiation and limits.
//!
//! A gateway and a connector are built at different times and still have
//! to understand each other. Before anything else is said they agree on a
//! protocol version; the gateway then says which connector builds it still
//! accepts and which it would rather see, and states the limits that hold
//! for the life of the session. The figures here are the ones a system
//! ships with: a gateway sends its own in `ready`, and a connector obeys
//! what it is sent. The codes with which a session is refused or ended are
//! here as well.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// The WebSocket subprotocol a session is opened with.
pub const SUBPROTOCOL: &str = "orch-link.v1";
/// The path under which an endpoint serves everything a machine reaches.
pub const PATH_PREFIX: &str = "/link/v1";
/// The path of the session socket on an endpoint.
pub const SESSION_PATH: &str = "/link/v1/session";

/// The protocol version this build speaks.
pub const PROTOCOL_VERSION: u32 = 1;
/// The oldest protocol version a gateway of this build still speaks: the
/// one before its own, so that a system can be updated before its
/// connectors are. There is none before the first.
pub const OLDEST_PROTOCOL_VERSION: u32 = if PROTOCOL_VERSION > 1 {
    PROTOCOL_VERSION - 1
} else {
    1
};

/// The most bytes of one frame, text or binary. A quarter of a mebibyte:
/// a fast link moves data in few frames, and a control frame that waits
/// behind one does not wait long.
pub const MAX_FRAME_BYTES: u32 = 262_144;
/// The credit a receiver gives a new channel. One mebibyte: what an end
/// holds, for each direction of each channel, for a consumer that has
/// stopped reading.
pub const TUNNEL_WINDOW_BYTES: u32 = 1_048_576;
/// The most channels open on one session. Enough for every session of a
/// busy harness process to hold its tunnels at once, and a bound on what
/// one machine can make a gateway keep.
pub const MAX_CHANNELS: u32 = 256;

/// Live TCP connections carried to one environment's ports at once: sixty-four permits the
/// parallel assets of a web application and several colleagues.
pub const PORT_CONNECTIONS_MAX: u32 = 64;
/// Accepted TCP connections waiting for their open result: sixteen absorbs a burst of arrivals
/// without an unbounded wait.
pub const PORT_WAITING_MAX: u32 = 16;
/// The credit and the replay of one TCP direction of a port: a quarter MiB absorbs a slow-link
/// burst and bounds two directions of sixty-four connections to 32 MiB.
pub const PORT_BUFFER_BYTES: u32 = 262_144;
/// A minute of silence ends a UDP sender's association, which its ordinary bursts keep.
pub const PORT_UDP_IDLE_SECS: u32 = 60;
/// UDP sender associations of one environment, each with an inside socket of its own.
pub const PORT_UDP_SENDERS_MAX: u32 = 128;
/// The largest UDP datagram carried: it fits the 4 KiB slow-link frame floor with its header, and
/// the frame cannot carry a larger one.
pub const PORT_UDP_DATAGRAM_BYTES: u32 = 4_000;
/// UDP payload waiting for one environment: a short burst, dropped rather than holding TCP back.
pub const PORT_UDP_BUFFER_BYTES: u32 = 262_144;
/// Ended connections kept per opening or own-use port: a short debugging history.
pub const PORT_RECENT_CONNECTIONS: u32 = 128;
/// Ended connections kept per environment by its gateway and its node together: sixteen groups
/// of 128.
pub const PORT_RECENT_TOTAL: u32 = 2_048;
/// How long an ended connection is kept, in memory only: a fifteen-minute debugging loop.
pub const PORT_RECENT_SECS: u32 = 900;
/// Distinct caller groups per environment, its gateway and its node together; past them a caller
/// is counted as Other.
pub const PORT_CALLER_TOTALS: u32 = 128;

/// The protocol versions a gateway speaks, lowest and highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ProtocolRange {
    /// The oldest version it speaks.
    pub min: u32,
    /// The newest version it speaks.
    pub max: u32,
}

impl ProtocolRange {
    /// What a gateway of this build speaks.
    pub const SUPPORTED: Self = Self {
        min: OLDEST_PROTOCOL_VERSION,
        max: PROTOCOL_VERSION,
    };

    /// Whether `version` is spoken.
    #[must_use]
    pub const fn contains(self, version: u32) -> bool {
        self.min <= version && version <= self.max
    }

    /// The version two sides settle on: the highest of `spoken`, the
    /// versions the other side speaks, that lies in this range. `None`
    /// means that they have no version in common, and the session or the
    /// enrollment is refused as unsupported.
    #[must_use]
    pub fn choose(self, spoken: &[u32]) -> Option<u32> {
        spoken
            .iter()
            .copied()
            .filter(|version| self.contains(*version))
            .max()
    }
}

/// The limits of a session, as its gateway states them when it is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Limits {
    /// The most bytes of one frame.
    pub max_frame_bytes: u32,
    /// The credit a receiver gives a new channel.
    pub tunnel_window_bytes: u32,
    /// The most channels open at once.
    pub max_channels: u32,
    /// The bounds of carrying TCP and UDP to the ports of an environment. Stated only where a
    /// deployment changed one of them; left out, the shipped figures hold.
    #[serde(default, skip_serializing_if = "PortLimits::is_shipped")]
    pub ports: PortLimits,
}

/// The bounds of carrying TCP and UDP to an environment's ports, as a deployment sets them in
/// `[link]` (the keys `port_*`). The gateway states them with the other limits of a session; a
/// connector carries ports within the ones it was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PortLimits {
    /// Live TCP connections of one environment at once.
    pub connections_max: u32,
    /// Accepted TCP connections waiting for their open result.
    pub waiting_max: u32,
    /// The credit and the replay of one TCP direction, in bytes.
    pub buffer_bytes: u32,
    /// The silence that ends a UDP sender's association, in seconds.
    pub udp_idle_secs: u32,
    /// UDP sender associations of one environment.
    pub udp_senders_max: u32,
    /// The largest UDP datagram carried, in bytes; never more than a frame carries.
    pub udp_datagram_bytes: u32,
    /// UDP payload waiting for one environment, in bytes.
    pub udp_buffer_bytes: u32,
    /// Ended connections kept per opening or own-use port.
    pub recent_connections: u32,
    /// Ended connections kept per environment, by its gateway and its node together.
    pub recent_total: u32,
    /// How long an ended connection is kept, in seconds.
    pub recent_secs: u32,
    /// Distinct caller groups per environment, by its gateway and its node together.
    pub caller_totals: u32,
}

impl PortLimits {
    /// The figures a system ships with.
    pub const SHIPPED: Self = Self {
        connections_max: PORT_CONNECTIONS_MAX,
        waiting_max: PORT_WAITING_MAX,
        buffer_bytes: PORT_BUFFER_BYTES,
        udp_idle_secs: PORT_UDP_IDLE_SECS,
        udp_senders_max: PORT_UDP_SENDERS_MAX,
        udp_datagram_bytes: PORT_UDP_DATAGRAM_BYTES,
        udp_buffer_bytes: PORT_UDP_BUFFER_BYTES,
        recent_connections: PORT_RECENT_CONNECTIONS,
        recent_total: PORT_RECENT_TOTAL,
        recent_secs: PORT_RECENT_SECS,
        caller_totals: PORT_CALLER_TOTALS,
    };

    /// Whether these are the shipped figures, which a gateway need not state.
    #[must_use]
    pub fn is_shipped(&self) -> bool {
        *self == Self::SHIPPED
    }
}

impl Default for PortLimits {
    /// The figures a system ships with.
    fn default() -> Self {
        Self::SHIPPED
    }
}

impl Limits {
    /// The most bytes of one frame, as a length.
    #[must_use]
    pub fn frame_bytes(&self) -> usize {
        usize::try_from(self.max_frame_bytes).unwrap_or(usize::MAX)
    }
}

impl Default for Limits {
    /// The limits a system ships with.
    fn default() -> Self {
        Self {
            max_frame_bytes: MAX_FRAME_BYTES,
            tunnel_window_bytes: TUNNEL_WINDOW_BYTES,
            max_channels: MAX_CHANNELS,
            ports: PortLimits::SHIPPED,
        }
    }
}

/// The connector builds a system accepts and prefers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ConnectorVersions {
    /// The oldest connector the system accepts.
    pub min: String,
    /// The connector the system would have every machine run.
    pub recommended: String,
}

/// Where a connector's build stands against what its system accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standing {
    /// Older than the oldest accepted: its session is refused, and it
    /// updates before it tries again.
    TooOld,
    /// Accepted, and older than the recommended build: it updates when it
    /// is allowed to and nothing it hosts is at work.
    Behind,
    /// The recommended build, or a newer one.
    Current,
}

impl ConnectorVersions {
    /// Where a connector of `version` stands.
    pub fn standing(&self, version: &str) -> Result<Standing, NotAVersion> {
        let version: ReleaseVersion = version.parse()?;
        if version < self.min.parse()? {
            Ok(Standing::TooOld)
        } else if version < self.recommended.parse()? {
            Ok(Standing::Behind)
        } else {
            Ok(Standing::Current)
        }
    }
}

/// A text that is not a release version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a release version such as 0.1.3")]
pub struct NotAVersion(pub String);

/// A release version, `major.minor.patch` with an optional pre-release
/// after `-` and build metadata after `+`. Versions order as semantic
/// versions do: by the three numbers, a pre-release before its release,
/// and build metadata not at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReleaseVersion {
    core: [u64; 3],
    pre: Vec<PreRelease>,
}

/// One dot-separated part of a pre-release.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PreRelease {
    /// Digits only: compared as a number, and before any text.
    Number(u64),
    /// Anything else: compared as text.
    Text(String),
}

impl std::str::FromStr for ReleaseVersion {
    type Err = NotAVersion;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || NotAVersion(text.to_owned());
        let without_build = text
            .split_once('+')
            .map_or(text, |(version, _build)| version);
        let (numbers, pre) = match without_build.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre)),
            None => (without_build, None),
        };
        let mut core = [0u64; 3];
        let mut parts = numbers.split('.');
        for slot in &mut core {
            let part = parts.next().ok_or_else(bad)?;
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(bad());
            }
            *slot = part.parse().map_err(|_| bad())?;
        }
        if parts.next().is_some() {
            return Err(bad());
        }
        let mut identifiers = Vec::new();
        for part in pre.into_iter().flat_map(|pre| pre.split('.')) {
            if part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(bad());
            }
            identifiers.push(match part.parse::<u64>() {
                Ok(number) if part.bytes().all(|byte| byte.is_ascii_digit()) => {
                    PreRelease::Number(number)
                }
                _ => PreRelease::Text(part.to_owned()),
            });
        }
        Ok(Self {
            core,
            pre: identifiers,
        })
    }
}

impl Ord for ReleaseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for ReleaseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PreRelease {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Number(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Number(_)) => Ordering::Greater,
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PreRelease {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Why a gateway refuses a session before it is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    /// The system knows no machine of that identifier.
    UnknownMachine,
    /// The machine was revoked, or presents another certificate than the
    /// one it is known by.
    Revoked,
    /// The proof that the machine holds its certificate's key does not
    /// hold: a wrong signature, another host, or a nonce that is not this
    /// connection's.
    BadProof,
    /// The machine's clock and the gateway's are too far apart.
    ClockSkew,
    /// The machine's certificate has expired.
    CertificateExpired,
    /// The two sides have no protocol version in common.
    ProtocolUnsupported,
    /// The connector is older than the system accepts.
    UpgradeRequired,
    /// The gateway cannot ask the system about the machine right now.
    Busy,
}

/// What a machine does about a refusal before it tries again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Remedy {
    /// Renew its certificate.
    Renew,
    /// Update the connector.
    Update,
    /// Wait for the delay the refusal names.
    Wait,
}

impl RejectCode {
    /// What a machine can do about this refusal by itself. `None` is a
    /// refusal that only a person can lift.
    #[must_use]
    pub const fn remedy(self) -> Option<Remedy> {
        match self {
            Self::CertificateExpired => Some(Remedy::Renew),
            Self::UpgradeRequired => Some(Remedy::Update),
            Self::Busy => Some(Remedy::Wait),
            Self::UnknownMachine
            | Self::Revoked
            | Self::BadProof
            | Self::ClockSkew
            | Self::ProtocolUnsupported => None,
        }
    }
}

/// Why a gateway ends a session that was ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ByeCode {
    /// The same machine opened another session, which takes this one's
    /// place.
    Replaced,
    /// The other side sent something the protocol does not allow.
    ProtocolError,
    /// The machine was revoked.
    Revoked,
    /// The machine was switched off in the system.
    Disabled,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions() -> ConnectorVersions {
        ConnectorVersions {
            min: "0.1.0".into(),
            recommended: "0.1.3".into(),
        }
    }

    #[test]
    fn two_sides_settle_on_the_newest_version_both_speak() {
        let gateway = ProtocolRange { min: 2, max: 3 };
        assert_eq!(gateway.choose(&[1, 2, 3, 4]), Some(3));
        assert_eq!(gateway.choose(&[1, 2]), Some(2));
    }

    #[test]
    fn sides_with_no_version_in_common_do_not_settle() {
        let gateway = ProtocolRange { min: 2, max: 3 };
        assert_eq!(gateway.choose(&[1]), None);
        assert_eq!(gateway.choose(&[]), None);
    }

    #[test]
    fn a_gateway_speaks_its_own_version_and_the_one_before() {
        assert!(ProtocolRange::SUPPORTED.contains(PROTOCOL_VERSION));
        assert!(!ProtocolRange::SUPPORTED.contains(PROTOCOL_VERSION + 1));
        assert_eq!(
            ProtocolRange::SUPPORTED.choose(&[PROTOCOL_VERSION]),
            Some(PROTOCOL_VERSION)
        );
    }

    #[test]
    fn a_connector_older_than_the_oldest_accepted_has_to_update_first() {
        assert_eq!(versions().standing("0.0.9"), Ok(Standing::TooOld));
    }

    #[test]
    fn a_connector_between_the_oldest_and_the_recommended_is_behind() {
        assert_eq!(versions().standing("0.1.0"), Ok(Standing::Behind));
        assert_eq!(versions().standing("0.1.2"), Ok(Standing::Behind));
    }

    #[test]
    fn a_connector_at_or_past_the_recommended_build_is_current() {
        assert_eq!(versions().standing("0.1.3"), Ok(Standing::Current));
        assert_eq!(versions().standing("0.2.0"), Ok(Standing::Current));
    }

    #[test]
    fn versions_compare_by_their_numbers_and_not_as_text() {
        let older: ReleaseVersion = "0.9.0".parse().unwrap();
        let newer: ReleaseVersion = "0.10.0".parse().unwrap();
        assert!(older < newer);
    }

    #[test]
    fn a_pre_release_comes_before_its_release_and_build_metadata_does_not_count() {
        let candidate: ReleaseVersion = "0.2.0-rc.2".parse().unwrap();
        let earlier: ReleaseVersion = "0.2.0-rc.1".parse().unwrap();
        let release: ReleaseVersion = "0.2.0".parse().unwrap();
        let rebuilt: ReleaseVersion = "0.2.0+412".parse().unwrap();
        assert!(earlier < candidate);
        assert!(candidate < release);
        assert_eq!(release.cmp(&rebuilt), Ordering::Equal);
    }

    #[test]
    fn a_text_that_is_no_version_is_refused() {
        for text in ["", "1", "1.2", "1.2.x", "1.2.3.4", "v1.2.3", "1.2.3-"] {
            assert!(text.parse::<ReleaseVersion>().is_err(), "{text}");
        }
        assert!(versions().standing("latest").is_err());
    }

    #[test]
    fn a_machine_knows_what_to_do_about_three_refusals() {
        assert_eq!(RejectCode::CertificateExpired.remedy(), Some(Remedy::Renew));
        assert_eq!(RejectCode::UpgradeRequired.remedy(), Some(Remedy::Update));
        assert_eq!(RejectCode::Busy.remedy(), Some(Remedy::Wait));
        assert_eq!(RejectCode::Revoked.remedy(), None);
    }

    #[test]
    fn changed_port_bounds_travel_with_the_limits_and_shipped_ones_are_left_out() {
        let shipped = serde_json::to_value(Limits::default()).unwrap();
        assert!(shipped.get("ports").is_none());
        let changed = Limits {
            ports: PortLimits {
                connections_max: 8,
                udp_idle_secs: 30,
                ..PortLimits::SHIPPED
            },
            ..Limits::default()
        };
        let written = serde_json::to_value(changed).unwrap();
        assert_eq!(written["ports"]["connections_max"], 8);
        assert_eq!(written["ports"]["udp_idle_secs"], 30);
        let read: Limits = serde_json::from_value(written).unwrap();
        assert_eq!(read, changed);
    }

    #[test]
    fn limits_are_written_as_the_gateway_states_them() {
        let limits: Limits = serde_json::from_str(
            r#"{ "max_frame_bytes": 262144, "tunnel_window_bytes": 1048576, "max_channels": 256 }"#,
        )
        .unwrap();
        assert_eq!(limits, Limits::default());
        assert_eq!(
            serde_json::to_value(RejectCode::UpgradeRequired).unwrap(),
            "upgrade_required"
        );
        assert_eq!(
            serde_json::to_value(ByeCode::ProtocolError).unwrap(),
            "protocol_error"
        );
    }
}
