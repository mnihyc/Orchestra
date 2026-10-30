//! Caller addresses from socket peers and explicitly trusted proxy chains.
//!
//! Headers describe a caller only when the socket peer is a configured proxy.
//! The last untrusted hop is the caller; earlier entries are its untrusted
//! input. Unreadable chains fall back to the peer rather than guessing a hop.

use http::HeaderMap;
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, PoisonError};

/// One hour, the specified diagnostic interval; one timestamp for every unnamed proxy in a process.
const PROXY_WARNING_SECONDS: i64 = 60 * 60;

/// The bounded diagnostic for private peers carrying a chain the installation has not trusted.
#[derive(Debug, Default)]
pub struct ProxyWarnings {
    last: Mutex<Option<i64>>,
}

impl ProxyWarnings {
    /// Emits at most one diagnostic per hour, using the service's sampled Unix time.
    /// Header contents are never logged and never make an unnamed peer trusted.
    pub fn observe(
        &self,
        peer: IpAddr,
        headers: &HeaderMap,
        trusted_proxy_cidrs: &[String],
        now: i64,
    ) {
        let peer = peer.to_canonical();
        let private = match peer {
            IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
            }
            IpAddr::V6(ip) => {
                ip.is_loopback()
                    || (ip.segments()[0] & 0xfe00 == 0xfc00)
                    || (ip.segments()[0] & 0xffc0 == 0xfe80)
            }
        };
        if !private
            || !(headers.contains_key("x-forwarded-for") || headers.contains_key("forwarded"))
        {
            return;
        }
        let proxies: Vec<IpNet> = trusted_proxy_cidrs
            .iter()
            .filter_map(|cidr| cidr.parse().ok())
            .collect();
        if trusted(peer, &proxies) {
            return;
        }
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|previous| now.saturating_sub(previous) < PROXY_WARNING_SECONDS) {
            return;
        }
        *last = Some(now);
        drop(last);
        tracing::warn!(%peer, "this private proxy is not in trusted_proxy_cidrs; forwarded headers are ignored and callers are counted as the proxy");
    }
}

static PROXY_WARNINGS: ProxyWarnings = ProxyWarnings {
    last: Mutex::new(None),
};

/// The same caller rule on a live service, with the one process-wide unnamed-proxy diagnostic.
#[must_use]
pub fn resolve_observed(
    peer: IpAddr,
    headers: &HeaderMap,
    trusted_proxy_cidrs: &[String],
    now: i64,
) -> IpAddr {
    PROXY_WARNINGS.observe(peer, headers, trusted_proxy_cidrs, now);
    resolve(peer, headers, trusted_proxy_cidrs)
}

/// Finds the caller from the connection and the installation's trusted proxy ranges.
///
/// IPv4-mapped IPv6 addresses use their IPv4 form for both trust and identity.
/// `X-Forwarded-For` takes precedence over `Forwarded`, including when malformed.
#[must_use]
pub fn resolve(peer: IpAddr, headers: &HeaderMap, trusted_proxy_cidrs: &[String]) -> IpAddr {
    let peer = peer.to_canonical();
    let Ok(proxies) = trusted_proxy_cidrs
        .iter()
        .map(|cidr| cidr.parse::<IpNet>())
        .collect::<Result<Vec<_>, _>>()
    else {
        return peer;
    };
    if !trusted(peer, &proxies) {
        return peer;
    }
    let chain = if headers.contains_key("x-forwarded-for") {
        x_forwarded_for(headers)
    } else {
        forwarded(headers)
    };
    chain
        .and_then(|chain| chain.into_iter().rev().find(|ip| !trusted(*ip, &proxies)))
        .unwrap_or(peer)
}

fn trusted(ip: IpAddr, proxies: &[IpNet]) -> bool {
    proxies.iter().any(|proxy| {
        proxy.contains(&ip)
            || matches!(ip, IpAddr::V4(ip) if proxy.contains(&IpAddr::V6(ip.to_ipv6_mapped())))
    })
}

fn x_forwarded_for(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let mut chain = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        for value in value.to_str().ok()?.split(',') {
            chain.push(value.trim().parse::<IpAddr>().ok()?.to_canonical());
        }
    }
    Some(chain)
}

fn forwarded(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let mut chain = Vec::new();
    for value in headers.get_all("forwarded") {
        for element in split_quoted(value.to_str().ok()?, ',')? {
            let mut address = None;
            let mut names = Vec::new();
            for pair in split_quoted(element, ';')? {
                let (name, value) = pair.trim().split_once('=')?;
                if !token(name)
                    || names
                        .iter()
                        .any(|other: &&str| other.eq_ignore_ascii_case(name))
                {
                    return None;
                }
                names.push(name);
                let value = parameter_value(value)?;
                if name.eq_ignore_ascii_case("for") {
                    address = Some(node_address(&value)?);
                }
            }
            chain.push(address?);
        }
    }
    Some(chain)
}

// Commas and semicolons inside quoted strings do not separate hops or pairs.
fn split_quoted(value: &str, delimiter: char) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if !quoted && character == delimiter {
            parts.push(&value[start..index]);
            start = index + character.len_utf8();
        }
    }
    if quoted || escaped {
        return None;
    }
    parts.push(&value[start..]);
    Some(parts)
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

fn parameter_value(value: &str) -> Option<String> {
    if token(value) {
        return Some(value.to_owned());
    }
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut unquoted = String::new();
    let mut escaped = false;
    for character in inner.chars() {
        if escaped {
            unquoted.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' || character.is_control() {
            return None;
        } else {
            unquoted.push(character);
        }
    }
    (!escaped).then_some(unquoted)
}

fn node_address(value: &str) -> Option<IpAddr> {
    if let Some(inner) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        return inner
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|ip| IpAddr::V6(ip).to_canonical());
    }
    value
        .parse::<IpAddr>()
        .ok()
        .or_else(|| value.parse::<SocketAddr>().ok().map(|address| address.ip()))
        .map(|ip| ip.to_canonical())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct WarningLog(std::sync::Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for WarningLog {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unnamed_private_proxies_are_said_once_an_hour_across_peers_without_logging_headers() {
        let warnings = ProxyWarnings::default();
        let log = WarningLog::default();
        let output = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || output.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let mut headers = HeaderMap::new();
            warnings.observe("10.0.0.1".parse().unwrap(), &headers, &[], 0);
            headers.insert("x-forwarded-for", "198.51.100.41".parse().unwrap());
            warnings.observe("203.0.113.1".parse().unwrap(), &headers, &[], 0);
            warnings.observe(
                "10.0.0.1".parse().unwrap(),
                &headers,
                &["10.0.0.0/8".to_owned()],
                0,
            );
            assert!(log.0.lock().unwrap().is_empty());
            for request in 0..100 {
                warnings.observe(
                    format!("10.0.0.{}", request + 1).parse().unwrap(),
                    &headers,
                    &[],
                    request,
                );
            }
            warnings.observe("::ffff:10.0.0.1".parse().unwrap(), &headers, &[], 3599);
            assert_eq!(
                String::from_utf8_lossy(&log.0.lock().unwrap())
                    .matches("not in trusted_proxy_cidrs")
                    .count(),
                1
            );
            warnings.observe("100.64.0.1".parse().unwrap(), &headers, &[], 3600);
        });
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text.matches("not in trusted_proxy_cidrs").count(), 2);
        assert!(text.contains("callers are counted as the proxy"));
        assert!(
            !text.contains("198.51.100.41"),
            "untrusted header contents are never logged"
        );
    }

    fn proxies() -> Vec<String> {
        vec!["127.0.0.0/8".to_owned(), "10.0.0.0/8".to_owned()]
    }

    fn address(peer: &str, name: &str, value: &str) -> IpAddr {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        resolve(peer.parse().unwrap(), &headers, &proxies())
    }

    #[test]
    fn two_named_proxies_are_passed_over_and_the_callers_earlier_claim_is_ignored() {
        assert_eq!(
            address(
                "127.0.0.1",
                "x-forwarded-for",
                "198.51.100.41, 203.0.113.12, 10.1.0.2"
            ),
            "203.0.113.12".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn a_direct_caller_cannot_supply_its_address_in_either_header() {
        for name in ["x-forwarded-for", "forwarded"] {
            assert_eq!(
                address("203.0.113.12", name, "198.51.100.41"),
                "203.0.113.12".parse::<IpAddr>().unwrap()
            );
        }
    }

    #[test]
    fn malformed_or_all_trusted_chains_give_the_connection_address() {
        for value in [
            "",
            "unknown, 203.0.113.12",
            "203.0.113.12, bad",
            "127.0.0.2, 10.1.0.2",
            "203.0.113.12,",
        ] {
            assert_eq!(
                address("127.0.0.1", "x-forwarded-for", value),
                "127.0.0.1".parse::<IpAddr>().unwrap()
            );
        }
    }

    #[test]
    fn mapped_ipv4_addresses_have_one_identity_and_the_same_proxy_trust() {
        assert_eq!(
            address(
                "::ffff:127.0.0.1",
                "x-forwarded-for",
                "::ffff:203.0.113.12, ::ffff:10.1.0.2"
            ),
            "203.0.113.12".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            address("::ffff:203.0.113.12", "x-forwarded-for", "198.51.100.41"),
            "203.0.113.12".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn standard_forwarded_walks_the_same_chain_with_quoted_ipv6_and_ports() {
        assert_eq!(
            address(
                "127.0.0.1",
                "forwarded",
                "for=198.51.100.41, For=\"[2001:db8::12]:4711\";proto=https;host=\"name,with;separators\", for=\"10.1.0.2:443\""
            ),
            "2001:db8::12".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn standard_forwarded_cannot_hide_an_unreadable_or_duplicate_hop() {
        for value in [
            "for=unknown",
            "for=_hidden",
            "for=203.0.113.12;For=198.51.100.41",
            "for=\"[2001:db8::12]",
            "for=203.0.113.12, proto=https",
            "for=203.0.113.12;host=a;Host=b",
        ] {
            assert_eq!(
                address("127.0.0.1", "forwarded", value),
                "127.0.0.1".parse::<IpAddr>().unwrap()
            );
        }
    }

    #[test]
    fn repeated_header_lines_are_one_ordered_chain_and_xff_takes_precedence() {
        let mut headers = HeaderMap::new();
        headers.append(
            "x-forwarded-for",
            "198.51.100.41, 203.0.113.12".parse().unwrap(),
        );
        headers.append("x-forwarded-for", "10.1.0.2".parse().unwrap());
        headers.insert("forwarded", "for=198.51.100.99".parse().unwrap());
        let peer = "127.0.0.1".parse().unwrap();
        assert_eq!(
            resolve(peer, &headers, &proxies()),
            "203.0.113.12".parse::<IpAddr>().unwrap()
        );
        headers.insert("x-forwarded-for", "bad".parse().unwrap());
        assert_eq!(resolve(peer, &headers, &proxies()), peer);
    }

    #[test]
    fn unreadable_header_bytes_and_absent_headers_give_the_connection() {
        let peer = "127.0.0.1".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(resolve(peer, &headers, &proxies()), peer);
        headers.insert(
            "x-forwarded-for",
            http::HeaderValue::from_bytes(&[0xff]).unwrap(),
        );
        assert_eq!(resolve(peer, &headers, &proxies()), peer);
    }

    #[test]
    fn no_proxy_is_trusted_by_default_and_malformed_configuration_stops_startup() {
        let config: crate::ServerConfig = crate::load_text("", "test", |_| None).unwrap();
        assert!(config.api.trusted_proxy_cidrs.is_empty());
        let error = crate::load_text::<crate::ServerConfig>(
            "[api]\ntrusted_proxy_cidrs = [\"typo\"]",
            "test",
            |_| None,
        )
        .unwrap_err();
        assert!(error.to_string().starts_with("api.trusted_proxy_cidrs"));
    }
}
