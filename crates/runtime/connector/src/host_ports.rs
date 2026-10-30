//! A host's ports as its node sees them: the operator's ceiling, a declaration checked against it, and the
//! proposal a host page is filled with.
//!
//! The ceiling is the `[ports]` table of the node's configuration, set by whoever runs the machine and by nobody
//! from Orch: a fixed incoming address, the one address the node may listen on (or `all`), and the ranges a
//! declaration's ranges must lie in, each absent member setting no ceiling and `ranges = []` permitting no host
//! opening at all. A host page saves a declaration (`host.ports`): the node accepts it only within the ceiling.
//! The proposal fills the page's three fields and binds, reserves and opens nothing: every address where the
//! ceiling permits it, the configured incoming address (the system adds the address its gateway saw where the node
//! knows none, since a node behind NAT cannot know its own outside address), and the lowest block of sixteen free
//! numbers within the ceiling's ranges, or within the dynamic and private ports when it names none. Free means bound
//! by no TCP or UDP socket of either family now, by this node's reading of the kernel's tables, and allocated to no
//! opening of the node's.

use std::collections::BTreeSet;
use std::net::IpAddr;

use orch_api_contract::ports::{
    HostPorts, HostPortsLimits, HostPortsProposal, HostPortsProposalSources,
    HostPortsProposalValues, PortRange,
};
use orch_config::server::Ports;
use orch_link_contract::ports::{HostPortsCheck, HostPortsCheckResult, PortPublish};
use orch_types::Time;
use sha2::{Digest, Sha256};

/// What `listen` is written as for every address of both families.
pub const ALL: &str = "all";
/// How many numbers in a row a proposal offers: a few services at once, and few enough that a host's free
/// numbers are not spent on one page.
pub const PROPOSED_BLOCK: u16 = 16;
/// Where a proposal looks when the ceiling names no ranges: the dynamic and private ports, which no service
/// is registered for.
pub const DYNAMIC_FROM: u16 = 49_152;
/// The last port number.
pub const DYNAMIC_TO: u16 = 65_535;

/// The one policy member's optional local constraints, used when reviewing a declaration.
#[must_use]
pub fn policy_limits(ports: &orch_link_contract::policy::MachinePorts) -> Ports {
    let Some(limits) = &ports.limits else {
        return Ports::default();
    };
    Ports {
        incoming_address: limits.incoming_address.clone(),
        listen: limits.listen.clone(),
        ranges: limits.ranges.as_ref().map(|ranges| {
            ranges
                .iter()
                .map(|range| orch_config::server::PortsRange {
                    from: range.from,
                    to: range.to,
                })
                .collect()
        }),
    }
}

/// Actual listeners are bounded by the current declared address and ranges, including withdrawal.
#[must_use]
pub fn policy_listeners(ports: &orch_link_contract::policy::MachinePorts) -> Ports {
    if ports.permits(orch_api_contract::ports::PortOpeningKind::Host)
        && let Some(publish) = &ports.publish
    {
        return Ports {
            incoming_address: Some(publish.incoming_address.clone()),
            listen: Some(publish.listen.clone()),
            ranges: Some(
                publish
                    .ranges
                    .iter()
                    .map(|range| orch_config::server::PortsRange {
                        from: range.from,
                        to: range.to,
                    })
                    .collect(),
            ),
        };
    }
    Ports {
        ranges: Some(Vec::new()),
        ..Default::default()
    }
}
/// The kernel's tables of the sockets of this machine's network, in both families.
const SOCKET_TABLES: [&str; 4] = [
    "/proc/net/tcp",
    "/proc/net/tcp6",
    "/proc/net/udp",
    "/proc/net/udp6",
];

/// The ceiling as the contract says it, with its revision: a digest of what it says, so that a changed ceiling
/// has another revision and an unchanged one the same, across restarts.
#[must_use]
pub fn limits(ports: &Ports) -> HostPortsLimits {
    let ranges = ports.ranges.as_ref().map(|ranges| {
        ranges
            .iter()
            .map(|range| PortRange {
                from: range.from,
                to: range.to,
            })
            .collect::<Vec<_>>()
    });
    let mut limits = HostPortsLimits {
        incoming_address: ports.incoming_address.clone(),
        listen: ports.listen.clone(),
        ranges,
        revision: 0,
    };
    limits.revision = revision_of(&limits);
    limits
}

/// A positive number that changes with what the ceiling says.
fn revision_of(limits: &HostPortsLimits) -> i64 {
    let said = serde_json::to_vec(&(&limits.incoming_address, &limits.listen, &limits.ranges))
        .unwrap_or_default();
    let digest = Sha256::digest(&said);
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(first) & i64::MAX
}

/// The ranges in order, with those that overlap or touch joined.
#[must_use]
pub fn merged(ranges: &[PortRange]) -> Vec<PortRange> {
    let mut sorted: Vec<PortRange> = ranges
        .iter()
        .filter(|range| range.from <= range.to)
        .cloned()
        .collect();
    sorted.sort_by_key(|range| (range.from, range.to));
    let mut joined: Vec<PortRange> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match joined.last_mut() {
            Some(last) if u32::from(range.from) <= u32::from(last.to) + 1 => {
                last.to = last.to.max(range.to);
            }
            _ => joined.push(range),
        }
    }
    joined
}

/// The ranges as a person reads them: `49152-49167, 50000`.
fn written(ranges: &[PortRange]) -> String {
    ranges
        .iter()
        .map(|range| {
            if range.from == range.to {
                range.from.to_string()
            } else {
                format!("{}-{}", range.from, range.to)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `declaration` lies within the ceiling `limits`; why not, where it does not.
pub fn check(declaration: &HostPorts, limits: &HostPortsLimits) -> Result<(), String> {
    let incoming = declaration.incoming_address.trim();
    if incoming.is_empty() {
        return Err("the incoming address is empty: name the address callers are given".to_owned());
    }
    if let Some(fixed) = &limits.incoming_address
        && fixed != incoming
    {
        return Err(format!(
            "ports (environment role): the machine's operator fixes the incoming address as {fixed}"
        ));
    }
    let listen = declaration.listen.as_str();
    if listen != ALL && listen.parse::<IpAddr>().is_err() {
        return Err(format!("{listen} is neither all nor one IP address"));
    }
    match limits.listen.as_deref() {
        None | Some(ALL) => {}
        Some(only) if only == listen => {}
        Some(only) => {
            return Err(format!(
                "ports (environment role): the machine's operator lets it listen on {only} alone"
            ));
        }
    }
    if declaration.ranges.is_empty() {
        return Err("a declaration names at least one range".to_owned());
    }
    if let Some(range) = declaration
        .ranges
        .iter()
        .find(|range| range.from == 0 || range.from > range.to)
    {
        return Err(format!(
            "{}-{} is not a range of port numbers",
            range.from, range.to
        ));
    }
    if let Some(allowed) = &limits.ranges {
        let space = merged(allowed);
        if let Some(outside) = declaration.ranges.iter().find(|range| {
            !space
                .iter()
                .any(|outer| outer.from <= range.from && range.to <= outer.to)
        }) {
            return Err(if space.is_empty() {
                "ports (environment role): the machine's operator permits no host opening"
                    .to_owned()
            } else {
                format!(
                    "{} lies outside the ranges ports (environment role): the machine's operator permits ({})",
                    written(std::slice::from_ref(outside)),
                    written(&space)
                )
            });
        }
    }
    Ok(())
}

/// Whether the ceiling `ports` lets the node serve `publication` now: its number, on its listen address, given
/// at its incoming address; why not, where it does not. The ceiling is read at each publication, so that a
/// narrowed one refuses before another admission.
pub fn permitted(ports: &Ports, publication: &PortPublish) -> Result<(), String> {
    check(
        &HostPorts {
            incoming_address: publication.incoming_address.clone(),
            listen: publication.listen.clone(),
            ranges: vec![PortRange {
                from: publication.host_port,
                to: publication.host_port,
            }],
        },
        &limits(ports),
    )
}

/// The lowest `size` numbers in a row within `space` of which none is in `taken`.
#[must_use]
pub fn free_block(space: &[PortRange], taken: &BTreeSet<u16>, size: u16) -> Option<PortRange> {
    let size = u32::from(size.max(1));
    for range in merged(space) {
        let (mut start, end) = (u32::from(range.from.max(1)), u32::from(range.to));
        while start + size - 1 <= end {
            let last = start + size - 1;
            let low = u16::try_from(start).ok()?;
            let high = u16::try_from(last).ok()?;
            match taken.range(low..=high).next_back() {
                // Past the highest taken number of this window is the next window that may be free.
                Some(blocked) => start = u32::from(*blocked) + 1,
                None => {
                    return Some(PortRange {
                        from: low,
                        to: high,
                    });
                }
            }
        }
    }
    None
}

/// What the node proposes for a host page under `limits`, with `taken` the numbers that are not free.
#[must_use]
pub fn propose(limits: &HostPortsLimits, taken: &BTreeSet<u16>, now: Time) -> HostPortsProposal {
    let (listen, listen_source) = match limits.listen.as_deref() {
        Some(only) if only != ALL => (
            only.to_owned(),
            "the one address ports (environment role): the machine's operator permits",
        ),
        Some(_) => (
            ALL.to_owned(),
            "every address of both families, which ports (environment role): the machine's operator permits",
        ),
        None => (
            ALL.to_owned(),
            "every address of both families: ports (environment role): the machine's operator sets no ceiling",
        ),
    };
    let (incoming_address, incoming_source) = match &limits.incoming_address {
        Some(fixed) => (
            Some(fixed.clone()),
            "ports (environment role): the machine's operator",
        ),
        None => (
            None,
            "not known on the host itself; the system proposes the address its gateway last saw the host connect from",
        ),
    };
    let space = limits.ranges.as_ref().map_or_else(
        || {
            vec![PortRange {
                from: DYNAMIC_FROM,
                to: DYNAMIC_TO,
            }]
        },
        |ranges| merged(ranges),
    );
    let (ranges, ranges_source, reason) = if space.is_empty() {
        (
            Vec::new(),
            "ports (environment role): the machine's operator permits no host opening".to_owned(),
            Some("ports (environment role): the machine's operator permits no host opening (its ranges are empty)".to_owned()),
        )
    } else {
        let within = if limits.ranges.is_some() {
            format!(
                "the ranges ports (environment role): the machine's operator permits ({})",
                written(&space)
            )
        } else {
            format!("the dynamic and private ports ({DYNAMIC_FROM}-{DYNAMIC_TO})")
        };
        match free_block(&space, taken, PROPOSED_BLOCK) {
            Some(block) => (
                vec![block],
                format!(
                    "the lowest {PROPOSED_BLOCK} free numbers in a row within {within}, bound by no TCP or UDP socket now"
                ),
                None,
            ),
            None => (
                Vec::new(),
                format!("no {PROPOSED_BLOCK} free numbers in a row within {within}"),
                Some(format!(
                    "no {PROPOSED_BLOCK} numbers in a row are free within {within}: free some, or widen the ranges"
                )),
            ),
        }
    };
    HostPortsProposal {
        values: HostPortsProposalValues {
            incoming_address,
            listen,
            ranges,
        },
        sources: HostPortsProposalSources {
            incoming_address: incoming_source.to_owned(),
            listen: listen_source.to_owned(),
            ranges: ranges_source,
        },
        as_of: now,
        reason,
        limits: limits.clone(),
    }
}

/// The local port of one line of a kernel socket table (`sl local_address rem_address st …`, the address
/// `HEX:HEX`), if the line is one.
fn local_port(line: &str) -> Option<u16> {
    let local = line.split_whitespace().nth(1)?;
    let (_, port) = local.rsplit_once(':')?;
    u16::from_str_radix(port, 16).ok().filter(|port| *port != 0)
}

/// The numbers bound by a TCP or UDP socket of this machine's network now, in either family. A TCP socket
/// counts in every state, since a number an established connection holds cannot be bound to a new listener
/// either. A table that cannot be read adds nothing: the proposal is then only less careful, and binding
/// still fails truthfully.
#[must_use]
pub fn taken_now() -> BTreeSet<u16> {
    let mut taken = BTreeSet::new();
    for table in SOCKET_TABLES {
        if let Ok(text) = std::fs::read_to_string(table) {
            taken.extend(text.lines().skip(1).filter_map(local_port));
        }
    }
    taken
}

/// The node's answer to `host.ports`: whether `body`'s declaration lies within the ceiling `ports`, the
/// ceiling, and the proposal now, with `taken` the numbers that are not free.
#[must_use]
pub fn answer(
    ports: &Ports,
    body: &HostPortsCheck,
    taken: &BTreeSet<u16>,
    now: Time,
) -> HostPortsCheckResult {
    let limits = limits(ports);
    let checked = body
        .ports
        .as_ref()
        .map(|declaration| check(declaration, &limits));
    HostPortsCheckResult {
        revision: body.revision,
        accepted: !matches!(checked, Some(Err(_))),
        reason: checked.and_then(Result::err),
        proposal: propose(&limits, taken, now),
        limits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_config::server::PortsRange;

    fn now() -> Time {
        Time::new(
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 14, 12, 0, 0)
                .unwrap(),
        )
    }

    fn range(from: u16, to: u16) -> PortRange {
        PortRange { from, to }
    }

    fn declaration(listen: &str, ranges: Vec<PortRange>) -> HostPorts {
        HostPorts {
            incoming_address: "host.example.net".into(),
            listen: listen.into(),
            ranges,
        }
    }

    fn ceiling(
        incoming: Option<&str>,
        listen: Option<&str>,
        ranges: Option<Vec<(u16, u16)>>,
    ) -> Ports {
        Ports {
            incoming_address: incoming.map(str::to_owned),
            listen: listen.map(str::to_owned),
            ranges: ranges.map(|ranges| {
                ranges
                    .into_iter()
                    .map(|(from, to)| PortsRange { from, to })
                    .collect()
            }),
        }
    }

    #[test]
    fn a_declaration_within_the_ceiling_is_accepted_and_one_that_widens_it_is_refused() {
        let open = limits(&ceiling(None, None, None));
        assert!(check(&declaration("all", vec![range(50_000, 50_015)]), &open).is_ok());
        let narrow = limits(&ceiling(
            Some("host.example.net"),
            Some("192.0.2.10"),
            Some(vec![(50_000, 50_010), (50_011, 50_020)]),
        ));
        assert!(
            check(
                &declaration("192.0.2.10", vec![range(50_005, 50_015)]),
                &narrow
            )
            .is_ok()
        );
        assert!(check(&declaration("all", vec![range(50_005, 50_015)]), &narrow).is_err());
        assert!(
            check(
                &declaration("192.0.2.10", vec![range(50_015, 50_021)]),
                &narrow
            )
            .is_err()
        );
        let mut elsewhere = declaration("192.0.2.10", vec![range(50_000, 50_001)]);
        elsewhere.incoming_address = "other.example.net".into();
        assert!(check(&elsewhere, &narrow).is_err());
        let nothing = limits(&ceiling(None, None, Some(vec![])));
        assert!(check(&declaration("all", vec![range(50_000, 50_001)]), &nothing).is_err());
        assert!(check(&declaration("somewhere", vec![range(1, 2)]), &open).is_err());
        assert!(check(&declaration("all", vec![range(9, 8)]), &open).is_err());
        assert!(check(&declaration("all", vec![]), &open).is_err());
    }

    #[test]
    fn a_publication_is_served_only_within_the_ceiling_as_it_is_now() {
        let publication = |listen: &str, host_port: u16| PortPublish {
            opening_id: orch_types::new_id(),
            environment_id: orch_types::new_id(),
            environment_machine_id: orch_types::new_id(),
            generation: 1,
            port: 8080,
            transports: vec![orch_api_contract::ports::PortTransport::Tcp],
            incoming_address: "host.example.net".into(),
            listen: listen.into(),
            host_port,
            revision: 1,
            allocation_revision: 1,
            proxy_protocol: false,
            tickets: Vec::new(),
        };
        let open = ceiling(None, None, None);
        assert_eq!(permitted(&open, &publication("all", 50_000)), Ok(()));
        let narrowed = ceiling(None, Some("192.0.2.10"), Some(vec![(50_000, 50_015)]));
        assert!(
            permitted(&narrowed, &publication("all", 50_000)).is_err(),
            "all is wider than the one address the ceiling permits now"
        );
        assert!(
            permitted(&narrowed, &publication("192.0.2.10", 49_999)).is_err(),
            "a number outside the ranges"
        );
        assert_eq!(
            permitted(&narrowed, &publication("192.0.2.10", 50_003)),
            Ok(())
        );
        let moved = ceiling(Some("other.example.net"), None, None);
        assert!(
            permitted(&moved, &publication("all", 50_003)).is_err(),
            "the operator fixed another incoming address"
        );
        let none = ceiling(None, None, Some(Vec::new()));
        assert!(
            permitted(&none, &publication("all", 50_003)).is_err(),
            "an empty list permits no host opening"
        );
    }

    #[test]
    fn the_ceilings_revision_follows_what_it_says() {
        let one = limits(&ceiling(None, Some("all"), Some(vec![(50_000, 50_100)])));
        let same = limits(&ceiling(None, Some("all"), Some(vec![(50_000, 50_100)])));
        let other = limits(&ceiling(None, Some("all"), Some(vec![(50_000, 50_101)])));
        assert_eq!(one.revision, same.revision);
        assert_ne!(one.revision, other.revision);
        assert!(one.revision >= 0);
    }

    #[test]
    fn a_proposal_takes_the_lowest_free_block_and_says_where_each_value_comes_from() {
        let taken: BTreeSet<u16> = [49_152, 49_160, 49_170].into_iter().collect();
        let proposal = propose(&limits(&ceiling(None, None, None)), &taken, now());
        assert_eq!(proposal.values.ranges, vec![range(49_171, 49_186)]);
        assert_eq!(proposal.values.listen, ALL);
        assert!(proposal.values.incoming_address.is_none());
        assert!(proposal.reason.is_none());

        let fixed = limits(&ceiling(
            Some("node.example.net"),
            Some("192.0.2.10"),
            Some(vec![(60_000, 60_015)]),
        ));
        let proposal = propose(&fixed, &BTreeSet::new(), now());
        assert_eq!(
            proposal.values.incoming_address.as_deref(),
            Some("node.example.net")
        );
        assert_eq!(proposal.values.listen, "192.0.2.10");
        assert_eq!(proposal.values.ranges, vec![range(60_000, 60_015)]);

        let full: BTreeSet<u16> = [60_007].into_iter().collect();
        let proposal = propose(&fixed, &full, now());
        assert!(proposal.values.ranges.is_empty());
        assert!(proposal.reason.is_some());

        let none = propose(
            &limits(&ceiling(None, None, Some(vec![]))),
            &BTreeSet::new(),
            now(),
        );
        assert!(none.values.ranges.is_empty());
        assert!(none.reason.is_some());
    }

    #[test]
    fn touching_ranges_are_one_and_a_block_may_span_them() {
        assert_eq!(
            merged(&[range(10, 19), range(1, 9), range(30, 40)]),
            vec![range(1, 19), range(30, 40)]
        );
        let block = free_block(&[range(100, 107), range(108, 120)], &BTreeSet::new(), 16);
        assert_eq!(block, Some(range(100, 115)));
        assert_eq!(
            free_block(&[range(65_530, 65_535)], &BTreeSet::new(), 16),
            None
        );
    }

    #[test]
    fn a_kernel_socket_line_gives_its_local_port() {
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode";
        let line = "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1";
        let six = "   1: 00000000000000000000000000000000:C350 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000  1000 0 99 2";
        assert_eq!(local_port(line), Some(8080));
        assert_eq!(local_port(six), Some(50_000));
        assert_eq!(local_port("garbage"), None);
        let taken: BTreeSet<u16> = [header, line, six]
            .iter()
            .skip(1)
            .filter_map(|each| local_port(each))
            .collect();
        assert_eq!(taken, [8080, 50_000].into_iter().collect());
    }

    #[test]
    fn the_answer_says_why_a_declaration_is_refused_and_still_proposes() {
        let ports = ceiling(None, Some("192.0.2.10"), None);
        let refused = answer(
            &ports,
            &HostPortsCheck {
                ports: Some(declaration("all", vec![range(50_000, 50_015)])),
                revision: 4,
            },
            &BTreeSet::new(),
            now(),
        );
        assert!(!refused.accepted);
        assert!(refused.reason.is_some());
        assert_eq!(refused.revision, 4);
        assert_eq!(refused.proposal.values.listen, "192.0.2.10");
        let only_asked = answer(
            &ports,
            &HostPortsCheck {
                ports: None,
                revision: 0,
            },
            &BTreeSet::new(),
            now(),
        );
        assert!(only_asked.accepted);
        assert!(only_asked.reason.is_none());
    }
}
