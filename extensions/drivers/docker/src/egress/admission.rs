//! Canonical host names, what a restricted unit's proxy admits, and the addresses no other name may reach.
//!
//! A request is decided by the name it asks for, before any lookup and before any socket: the exact authority
//! and port of one of Orch's trusted endpoints, or one of the other host names the unit's place allows, and
//! nothing else. A name is compared in its canonical form (lower case, IDNA, no final dot), so `Example.COM.`
//! and `example.com` are one name, and a literal address (in any of the forms a parser accepts, `0x7f.1` among
//! them) is never a name. An other name that resolves into the host's own addresses, Orch's units, loopback or
//! link-local is refused even so: a list of names cannot make the proxy a socket to a host service.

use std::net::IpAddr;

use ipnet::IpNet;
use orch_driver_contract::network::NetworkTransport;
use serde::{Deserialize, Serialize};

/// The longest host name there is, in octets.
const NAME_MAX: usize = 253;

/// The longest label of a host name, in octets.
const LABEL_MAX: usize = 63;

/// Why a text is not a host name a list may hold or a request may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameFault {
    /// Nothing is there.
    #[error("no host name")]
    Empty,
    /// It is an address, which only Orch's own transport may be reached at.
    #[error("an address, not a host name")]
    Address,
    /// It names many hosts at once.
    #[error("a wildcard, not one host name")]
    Wildcard,
    /// It has a port, a path, a user or something else that is no part of a name.
    #[error("not a host name")]
    NotAName,
    /// It is longer than a name can be.
    #[error("longer than a host name can be")]
    TooLong,
}

/// The canonical form of a host name: lower case, IDNA, without a final dot. Addresses, wildcards, ports, paths
/// and anything else that is not one host's name are refused.
pub fn canonical_name(raw: &str) -> Result<String, NameFault> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if trimmed.is_empty() {
        return Err(NameFault::Empty);
    }
    if trimmed.contains('*') {
        return Err(NameFault::Wildcard);
    }
    let bare = trimmed
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(trimmed);
    if bare.parse::<IpAddr>().is_ok() {
        return Err(NameFault::Address);
    }
    if trimmed.chars().any(|c| {
        matches!(c, ':' | '/' | '?' | '#' | '@' | '[' | ']' | '\\' | '%') || c.is_whitespace()
    }) {
        return Err(NameFault::NotAName);
    }
    if trimmed.len() > NAME_MAX {
        return Err(NameFault::TooLong);
    }
    let domain = match url::Host::parse(trimmed) {
        Ok(url::Host::Domain(domain)) => domain,
        // What a lenient parser reads as an address (`0x7f.1`, `2130706433`) is an address.
        Ok(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => return Err(NameFault::Address),
        Err(_) => return Err(NameFault::NotAName),
    };
    if domain.len() > NAME_MAX {
        return Err(NameFault::TooLong);
    }
    let labels_fit = domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= LABEL_MAX
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    });
    if !labels_fit {
        return Err(NameFault::NotAName);
    }
    Ok(domain)
}

/// What the proxy of one unit admits: the trusted Orch endpoints, the other names, and the address ranges no
/// other name may resolve into. The node writes it; the proxy holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admission {
    /// The revision of the other names.
    pub revision: String,
    /// The revision of the trusted endpoints.
    pub always_revision: String,
    /// The trusted Orch endpoints, always admitted at their exact authority and port.
    pub always: Vec<NetworkTransport>,
    /// The other host names, canonical.
    pub allowed_hosts: Vec<String>,
    /// Address ranges no other name may resolve into: the host's own addresses and the networks of Orch's units.
    #[serde(default)]
    pub refused: Vec<IpNet>,
}

/// What the proxy does with a request for a host and a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// One of Orch's trusted endpoints: reached by its name, with nothing refused of its address.
    Trusted(String),
    /// One of the other names: reached only at addresses that are not refused.
    Other(String),
    /// Refused, with the host as it can be shown and why.
    Denied {
        /// The host, canonical where it could be read, otherwise as much of what was asked as is safe to show.
        host: String,
        /// Why, in a few words.
        reason: &'static str,
    },
}

/// Why a request was refused, in the words its answer and the unit's state use.
pub mod reasons {
    /// The name is on neither list.
    pub const NOT_LISTED: &str = "not an allowed host";
    /// The request asked for an address.
    pub const ADDRESS: &str = "an address is never allowed, only names";
    /// What was asked is no host name.
    pub const NOT_A_NAME: &str = "not a host name";
    /// The name resolves only into refused addresses.
    pub const REFUSED_ADDRESS: &str = "the name resolves to an address that is not allowed";
    /// The name does not resolve.
    pub const NO_ADDRESS: &str = "the name has no address";
    /// The network is closed until its policy is in force.
    pub const CLOSED: &str = "the network is closed until its policy is applied";
    /// The request is not one the proxy reads.
    pub const MALFORMED: &str = "not a request the proxy reads";
    /// The request's host header and its address disagree.
    pub const HOST_MISMATCH: &str = "the request's host header names another host";
    /// The host could not be reached.
    pub const UNREACHABLE: &str = "the host could not be reached";
    /// The policy changed and no longer admits the host.
    pub const WITHDRAWN: &str = "no longer allowed";
}

/// What of a refused host may be shown: a name or an address alone, never malformed request content.
#[must_use]
pub fn shown(raw: &str) -> String {
    canonical_name(raw).unwrap_or_else(|_| {
        raw.parse::<IpAddr>()
            .map_or_else(|_| "invalid_host".to_owned(), |address| address.to_string())
    })
}

impl Admission {
    /// Decides a request for `host` at `port`, before any lookup or socket.
    #[must_use]
    pub fn decide(&self, host: &str, port: u16) -> Decision {
        let literal = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        if let Ok(address) = literal.parse::<IpAddr>() {
            let address = address.to_string();
            if self
                .always
                .iter()
                .any(|trusted| trusted.authority == address && trusted.port == port)
            {
                return Decision::Trusted(address);
            }
        }
        let name = match canonical_name(host) {
            Ok(name) => name,
            Err(NameFault::Address) => {
                return Decision::Denied {
                    host: shown(host),
                    reason: reasons::ADDRESS,
                };
            }
            Err(_) => {
                return Decision::Denied {
                    host: shown(host),
                    reason: reasons::NOT_A_NAME,
                };
            }
        };
        if self
            .always
            .iter()
            .any(|trusted| trusted.authority == name && trusted.port == port)
        {
            return Decision::Trusted(name);
        }
        if self.allowed_hosts.contains(&name) {
            return Decision::Other(name);
        }
        Decision::Denied {
            host: name,
            reason: reasons::NOT_LISTED,
        }
    }

    /// Whether a name may be looked up for the unit at all: a trusted authority or an other name.
    #[must_use]
    pub fn names(&self, name: &str) -> bool {
        self.always.iter().any(|trusted| trusted.authority == name)
            || self.allowed_hosts.iter().any(|allowed| allowed == name)
    }

    /// Whether an other name may not be reached at `address`: loopback, link-local, unspecified, multicast and
    /// broadcast addresses never, and neither the ranges the node refused.
    #[must_use]
    pub fn refuses(&self, address: IpAddr) -> bool {
        let address = address.to_canonical();
        let never = match address {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.is_broadcast()
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unicast_link_local()
                    || v6.is_unspecified()
                    || v6.is_multicast()
            }
        };
        never || self.refused.iter().any(|range| range.contains(&address))
    }

    /// Whether `host` is still admitted, for a flow that was opened to it under an earlier policy.
    #[must_use]
    pub fn still_admits(&self, host: &str, port: u16) -> bool {
        !matches!(self.decide(host, port), Decision::Denied { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::network::TransportPurpose;

    fn admission() -> Admission {
        Admission {
            revision: "r1".into(),
            always_revision: "a1".into(),
            always: vec![NetworkTransport {
                authority: "gateway.orch.internal".into(),
                protocol: "wss".into(),
                port: 9443,
                purpose: TransportPurpose::Gateway,
            }],
            allowed_hosts: vec!["pypi.org".into(), "xn--bcher-kva.example".into()],
            refused: vec!["172.17.0.0/16".parse().unwrap()],
        }
    }

    #[test]
    fn a_name_is_compared_in_lower_case_idna_without_its_final_dot() {
        assert_eq!(canonical_name("PyPI.org.").unwrap(), "pypi.org");
        assert_eq!(
            canonical_name("Bücher.example").unwrap(),
            "xn--bcher-kva.example"
        );
        assert_eq!(
            canonical_name(" files.pythonhosted.org ").unwrap(),
            "files.pythonhosted.org"
        );
    }

    #[test]
    fn addresses_wildcards_ports_and_paths_are_no_names() {
        for (raw, fault) in [
            ("", NameFault::Empty),
            ("10.0.0.1", NameFault::Address),
            ("[::1]", NameFault::Address),
            ("::1", NameFault::Address),
            ("0x7f.1", NameFault::Address),
            ("2130706433", NameFault::Address),
            ("*.example.com", NameFault::Wildcard),
            ("example.com:443", NameFault::NotAName),
            ("example.com/path", NameFault::NotAName),
            ("user@example.com", NameFault::NotAName),
            ("-bad.example", NameFault::NotAName),
            ("a..b", NameFault::NotAName),
        ] {
            assert_eq!(canonical_name(raw), Err(fault), "{raw:?}");
        }
        let long = format!("{}.com", "a".repeat(NAME_MAX));
        assert_eq!(canonical_name(&long), Err(NameFault::TooLong));
    }

    #[test]
    fn the_gateway_is_admitted_only_at_its_exact_port_and_an_other_name_at_any() {
        let admission = admission();
        assert_eq!(
            admission.decide("gateway.orch.internal", 9443),
            Decision::Trusted("gateway.orch.internal".into())
        );
        assert!(matches!(
            admission.decide("gateway.orch.internal", 22),
            Decision::Denied {
                reason: reasons::NOT_LISTED,
                ..
            }
        ));
        assert_eq!(
            admission.decide("PYPI.org", 443),
            Decision::Other("pypi.org".into())
        );
        assert_eq!(
            admission.decide("bücher.example", 80),
            Decision::Other("xn--bcher-kva.example".into())
        );
        assert_eq!(
            admission.decide("evil.example", 443),
            Decision::Denied {
                host: "evil.example".into(),
                reason: reasons::NOT_LISTED
            }
        );
        assert!(matches!(
            admission.decide("127.0.0.1", 80),
            Decision::Denied {
                reason: reasons::ADDRESS,
                ..
            }
        ));
    }

    #[test]
    fn an_empty_list_admits_only_orch() {
        let only_orch = Admission {
            allowed_hosts: Vec::new(),
            ..admission()
        };
        assert!(matches!(
            only_orch.decide("pypi.org", 443),
            Decision::Denied { .. }
        ));
        assert!(matches!(
            only_orch.decide("gateway.orch.internal", 9443),
            Decision::Trusted(_)
        ));
    }

    #[test]
    fn an_other_name_never_reaches_the_host_its_units_or_loopback() {
        let admission = admission();
        for refused in [
            "127.0.0.1",
            "::1",
            "169.254.169.254",
            "fe80::1",
            "0.0.0.0",
            "172.17.0.1",
            "::ffff:127.0.0.1",
            "224.0.0.1",
        ] {
            assert!(admission.refuses(refused.parse().unwrap()), "{refused}");
        }
        for allowed in ["151.101.0.223", "2a04:4e42::223", "10.20.30.40"] {
            assert!(!admission.refuses(allowed.parse().unwrap()), "{allowed}");
        }
    }

    #[test]
    fn what_is_shown_of_a_refused_host_is_short_and_printable() {
        assert_eq!(shown("EVIL.example."), "evil.example");
        let shown_raw = shown("bad host\r\nX-Header: y");
        assert!(!shown_raw.contains('\r') && !shown_raw.contains(' '));
        assert!(shown(&"x".repeat(500)).len() <= NAME_MAX);
    }
}
