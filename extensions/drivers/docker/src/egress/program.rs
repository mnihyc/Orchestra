//! The proxy program of one restricted unit: its listeners on the unit's private network, its orders, its heartbeat and its state.
//!
//! The program runs in a container of its own beside the unit, from the connector build the host already keeps
//! (`orch-connector proxy`), as the node's user, with no capability. It listens only on its address on the unit's
//! private network: the HTTP proxy on [`HTTP_PORT`] and the resolver on [`DNS_PORT`]. Every [`POLL`] it reads the
//! orders and the heartbeat the node writes into its directory. It lets traffic flow only while the orders are
//! open and the heartbeat is fresh; otherwise every flow is closed and every request refused, Orch's own
//! included, until the node beats again with orders that are current. What is in force is written back as its
//! state whenever it changes.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use orch_types::Clock;
use tokio::net::{TcpListener, UdpSocket};

use super::admission::Admission;
use super::files::{self, Orders, State};
use super::proxy::{self, Gate};
use super::resolver;

/// The port the HTTP proxy listens on: the one HTTP proxies are known by.
pub const HTTP_PORT: u16 = 3128;

/// The port the resolver listens on: the one resolvers are asked at, which a container's name servers cannot
/// change.
pub const DNS_PORT: u16 = 53;

/// How often the orders and the heartbeat are read: a removed host is cut off within about this.
pub const POLL: Duration = Duration::from_millis(500);

/// How often the node beats.
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);

/// How old the heartbeat may be before the proxy closes: four beats missed, so that one slow write of a busy
/// host does not close a unit's network, and a node that is gone closes it within half a minute.
pub const HEARTBEAT_STALE_SECS: i64 = 20;

/// How far ahead of the proxy's clock a heartbeat may be and still count: the same machine's clock, read twice.
const CLOCK_SLACK_SECS: i64 = 2;
/// A local IPv4 address excludes that single address, rather than its whole private LAN.
const IPV4_HOST_PREFIX: u8 = 32;
/// A local IPv6 address similarly excludes one address.
const IPV6_HOST_PREFIX: u8 = 128;

/// Where the program finds its orders and how it listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Whether the node's host-network observer must report every local and Docker-bridge address range.
    pub host_inventory: bool,
    /// The directory of its orders, its heartbeat and its state.
    pub dir: PathBuf,
    /// Its address on the unit's private network, the only one it listens on.
    pub listen: IpAddr,
}

/// What the orders and the heartbeat let flow now, and why not when nothing does.
#[must_use]
pub fn in_force(
    orders: Option<&Orders>,
    last_beat: Option<i64>,
    now_secs: i64,
) -> (Option<Admission>, Option<&'static str>) {
    let fresh = last_beat.is_some_and(|at| {
        at <= now_secs + CLOCK_SLACK_SECS && now_secs - at <= HEARTBEAT_STALE_SECS
    });
    match orders {
        None => (None, Some("no network policy has been given yet")),
        Some(orders) if !orders.open => (None, Some("the network policy is being applied")),
        Some(_) if !fresh => (
            None,
            Some(
                "the host has lost its link to the system; the network is closed until it is back",
            ),
        ),
        Some(orders) => (Some(orders.admission.clone()), None),
    }
}

/// Runs the program until it is stopped.
async fn host_inventory(dir: &std::path::Path, now: i64) -> Option<Vec<ipnet::IpNet>> {
    let at: i64 = tokio::fs::read_to_string(dir.join("host-addresses-at"))
        .await
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if at > now + CLOCK_SLACK_SECS || now - at > HEARTBEAT_STALE_SECS {
        return None;
    }
    let local = tokio::fs::read_to_string(dir.join("host-fib-trie"))
        .await
        .ok()?;
    let ipv6 = tokio::fs::read_to_string(dir.join("host-if-inet6"))
        .await
        .ok()?;
    let routes = tokio::fs::read_to_string(dir.join("host-routes"))
        .await
        .ok()?;
    let routes6 = tokio::fs::read_to_string(dir.join("host-routes6"))
        .await
        .ok()?;
    let mut ranges = inventory_ranges(&local, &ipv6, &routes, &routes6);
    ranges.sort();
    ranges.dedup();
    // A host network must at least name its loopback; an empty or damaged inventory is never authority.
    (!ranges.is_empty()).then_some(ranges)
}

/// Parse Linux's local addresses and Docker bridge routes without running a supplied executable.
#[must_use]
pub fn inventory_ranges(local: &str, ipv6: &str, routes: &str, routes6: &str) -> Vec<ipnet::IpNet> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let mut ranges = Vec::new();
    let mut previous = None;
    for line in local.lines() {
        if line.contains("/32 host LOCAL")
            && let Some(address) = previous
            && let Ok(network) = ipnet::Ipv4Net::new(address, IPV4_HOST_PREFIX)
        {
            ranges.push(ipnet::IpNet::V4(network));
        }
        previous = line
            .split_whitespace()
            .last()
            .and_then(|word| word.parse::<Ipv4Addr>().ok());
    }
    for line in ipv6.lines() {
        if let Some(word) = line.split_whitespace().next()
            && word.len() == 32
            && let Ok(address) = u128::from_str_radix(word, 16)
            && let Ok(network) = ipnet::Ipv6Net::new(Ipv6Addr::from(address), IPV6_HOST_PREFIX)
        {
            ranges.push(ipnet::IpNet::V6(network));
        }
    }
    let bridge = |name: &str| name == "docker0" || name.starts_with("br-");
    for line in routes.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 || !bridge(fields[0]) {
            continue;
        }
        if let (Ok(address), Ok(mask)) = (
            u32::from_str_radix(fields[1], 16),
            u32::from_str_radix(fields[7], 16),
        ) {
            let prefix = u32::from_be_bytes(mask.to_le_bytes()).leading_ones();
            if prefix > 0
                && let Ok(network) =
                    ipnet::Ipv4Net::new(Ipv4Addr::from(address.to_le_bytes()), prefix as u8)
            {
                ranges.push(ipnet::IpNet::V4(network));
            }
        }
    }
    for line in routes6.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || !fields.last().is_some_and(|name| bridge(name)) {
            continue;
        }
        if let (Ok(address), Ok(prefix)) = (
            u128::from_str_radix(fields[0], 16),
            u8::from_str_radix(fields[1], 16),
        ) && prefix > 0
            && let Ok(network) = ipnet::Ipv6Net::new(Ipv6Addr::from(address), prefix)
        {
            ranges.push(ipnet::IpNet::V6(network));
        }
    }
    ranges
}

/// Runs the proxy until it is stopped, closing it whenever either trusted watchdog loses its report.
pub async fn run(options: Options, clock: Arc<dyn Clock>) -> io::Result<()> {
    let gate = Arc::new(Gate::new());
    let http = TcpListener::bind(SocketAddr::new(options.listen, HTTP_PORT)).await?;
    let dns = Arc::new(UdpSocket::bind(SocketAddr::new(options.listen, DNS_PORT)).await?);
    tokio::spawn(proxy::serve(http, gate.clone(), clock.clone()));
    tokio::spawn(resolver::serve(dns, gate.clone()));
    let mut written: Option<State> = None;
    loop {
        let orders: Option<Orders> = files::read_json(&options.dir, files::ORDERS).await;
        let last_beat = files::last_beat(&options.dir).await;
        let now = clock.now().timestamp();
        let (mut admission, mut reason) = in_force(orders.as_ref(), last_beat, now);
        if options.host_inventory {
            match host_inventory(&options.dir, now).await {
                Some(ranges) => {
                    if let Some(admission) = admission.as_mut() {
                        admission.refused.extend(ranges);
                        admission.refused.sort();
                        admission.refused.dedup();
                    }
                }
                None => {
                    admission = None;
                    reason =
                        Some("the host address inventory is unavailable; the network stays closed");
                }
            }
        }
        if gate.current() != admission {
            gate.set(admission.clone());
        }
        let state = State {
            revision: orders
                .as_ref()
                .map(|orders| orders.admission.revision.clone()),
            always_revision: orders
                .as_ref()
                .map(|orders| orders.admission.always_revision.clone()),
            flowing: admission.is_some(),
            reason: reason.map(str::to_owned),
            last_denied: gate.last_denied(),
        };
        if written.as_ref() != Some(&state) {
            match files::write_json(&options.dir, files::STATE, &state).await {
                Ok(()) => written = Some(state),
                Err(error) => tracing::warn!(%error, "the proxy could not write its state"),
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn listed_aliases_cannot_reach_any_host_interface_or_a_new_docker_bridge() {
        use super::*;
        let ranges = inventory_ranges(
            "|-- 127.0.0.1\n /32 host LOCAL\n|-- 192.168.44.9\n /32 host LOCAL\n|-- 203.0.113.17\n /32 host LOCAL\n",
            "20010db8000000000000000000000001 02 40 00 00 eth0\n",
            "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\nbr-new 00002CAE 00000000 0001 0 0 0 0000FFFF 0 0 0\neth0 002CA8C0 00000000 0001 0 0 0 00FFFFFF 0 0 0\n",
            "",
        );
        let admission = Admission {
            allowed_hosts: vec!["listed.example".to_owned()],
            refused: ranges,
            ..Admission::default()
        };
        assert!(matches!(
            admission.decide("listed.example", 443),
            crate::egress::admission::Decision::Other(_)
        ));
        for address in ["192.168.44.9", "203.0.113.17", "2001:db8::1", "174.44.0.2"] {
            assert!(
                admission.refuses(address.parse().unwrap()),
                "a listed alias to {address} must be refused"
            );
        }
        assert!(
            !admission.refuses("192.168.44.10".parse().unwrap()),
            "an unrelated private service is still eligible"
        );
    }

    #[tokio::test]
    async fn a_missing_or_expired_host_inventory_never_authorizes_outward_traffic() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        assert!(host_inventory(dir.path(), 100).await.is_none());
        for name in [
            "host-fib-trie",
            "host-if-inet6",
            "host-routes",
            "host-routes6",
        ] {
            tokio::fs::write(
                dir.path().join(name),
                if name == "host-fib-trie" {
                    "|-- 127.0.0.1\n /32 host LOCAL\n"
                } else {
                    ""
                },
            )
            .await
            .unwrap();
        }
        tokio::fs::write(dir.path().join("host-addresses-at"), "100")
            .await
            .unwrap();
        assert!(host_inventory(dir.path(), 100).await.is_some());
        assert!(
            host_inventory(dir.path(), 100 + HEARTBEAT_STALE_SECS + 1)
                .await
                .is_none()
        );
    }
    use super::*;

    fn orders(open: bool) -> Orders {
        Orders {
            admission: Admission {
                revision: "r1".into(),
                ..Admission::default()
            },
            open,
        }
    }

    #[test]
    fn traffic_flows_only_with_open_orders_and_a_fresh_heartbeat() {
        let now = 1_760_000_000;
        assert!(in_force(None, Some(now), now).0.is_none());
        assert!(in_force(Some(&orders(false)), Some(now), now).0.is_none());
        assert!(
            in_force(Some(&orders(true)), None, now).0.is_none(),
            "a node that never beat"
        );
        assert!(
            in_force(
                Some(&orders(true)),
                Some(now - HEARTBEAT_STALE_SECS - 1),
                now
            )
            .0
            .is_none(),
            "a node gone quiet"
        );
        assert!(
            in_force(Some(&orders(true)), Some(now + 60), now)
                .0
                .is_none(),
            "a beat from the future"
        );
        let (admission, reason) = in_force(Some(&orders(true)), Some(now - 3), now);
        assert_eq!(admission.unwrap().revision, "r1");
        assert!(reason.is_none());
    }
}
