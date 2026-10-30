//! Where a notifier may connect: an address that is not a public one is refused unless listed.
//!
//! A channel or a destination that an org or a person set up must not be a way to reach the machines behind
//! the system. A host that resolves to anything but a public address is refused unless
//! `outbound.allowed_private_hosts` lists it: on the address it connects to, and again after every redirect.
//! The system's own channels are not held to the list, since a relay on a private network is the usual case
//! for them (`Config::allowed_private_hosts` is `None` for them).
//!
//! What is public is not a matter of three private ranges. The address registry keeps a list of the
//! special-purpose ranges of each family and says of each whether it is reachable from everywhere; a range
//! that is not is not public here either. So the loopback, link-local and private ranges are refused, and
//! with them the range shared between a carrier and its customers (overlays between machines use it), the
//! ranges for benchmarking and for documentation (a network that is not on the Internet may use any of
//! them), and the reserved and multicast ranges. An IPv6 address is public only inside the one range public
//! addresses are given from, and an IPv6 address that stands for an IPv4 one is judged as that one.
//!
//! The built-in notifiers connect through the worker and are held to this by it. An installed notifier opens
//! its own connections, so it is given the list with every call, and its contract suite calls it with a
//! private address to see that it refuses. [`may_connect`] is the one rule, for both. An entry of the list is
//! a host name, an address, or a range such as `10.0.0.0/8`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The IPv4 ranges that are not public: every range the registry's list of special-purpose IPv4 addresses
/// marks as not reachable from everywhere, and the multicast range, where no single machine answers. The two
/// anycast addresses the list lets through inside 192.0.0.0/24 are refused with their range: nothing a
/// notifier calls lives there.
const NOT_PUBLIC_V4: [(Ipv4Addr, u8); 14] = [
    // This network; 0.0.0.0 reaches the machine itself.
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    // Private.
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    // Shared between a carrier and its customers; overlays between machines use it.
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    // Loopback.
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    // Link-local; the metadata service of a cloud answers here.
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    // Private.
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    // Protocol assignments.
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    // Documentation.
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    // Private.
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    // Benchmarking.
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    // Documentation.
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    // Documentation.
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    // Multicast.
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    // Reserved, with the broadcast address.
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// The one range public IPv6 addresses are given from. Whatever lies outside it is not public: the machine
/// itself, link-local, unique local and the former site-local addresses, multicast, and the ranges kept for
/// protocols. A range the registry adds outside it later is refused without a change here.
const GIVEN_OUT_V6: (Ipv6Addr, u8) = (Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3);

/// The ranges inside it that the registry's list of special-purpose IPv6 addresses marks as not reachable
/// from everywhere. The few addresses the list lets through inside the first are refused with their range.
const NOT_PUBLIC_V6: [(Ipv6Addr, u8); 3] = [
    // Protocol assignments: tunnels and benchmarking among them.
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    // Documentation.
    (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
    // Documentation.
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
];

/// The IPv4 address an IPv6 address stands for, where it stands for one: an IPv4 address written as IPv6
/// (`::ffff:a.b.c.d`), one behind a translator (`64:ff9b::/96` holds it in its last 32 bits), or one behind
/// a 6to4 router (`2002::/16` holds it in the 32 bits after the range). What is sent to such an address goes
/// to the IPv4 address, so it is judged as that one.
fn stands_for(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let joined = |high: u16, low: u16| Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
    if let Some(v4) = v6.to_ipv4_mapped() {
        return Some(v4);
    }
    match v6.segments() {
        [0x64, 0xff9b, 0, 0, 0, 0, high, low] | [0x2002, high, low, ..] => Some(joined(high, low)),
        _ => None,
    }
}

/// Whether an address is anything but a public one: a loopback, link-local, private or shared address, the
/// unspecified one (which reaches the machine itself), or one in a range kept for another use. An IPv6
/// address that stands for an IPv4 one counts as that one.
#[must_use]
pub fn is_private(address: IpAddr) -> bool {
    let within = |(network, bits): (IpAddr, u8)| in_range(network, bits, address);
    match address {
        IpAddr::V4(_) => NOT_PUBLIC_V4
            .iter()
            .any(|(network, bits)| within((IpAddr::V4(*network), *bits))),
        IpAddr::V6(v6) => match stands_for(v6) {
            Some(v4) => is_private(IpAddr::V4(v4)),
            None => {
                !within((IpAddr::V6(GIVEN_OUT_V6.0), GIVEN_OUT_V6.1))
                    || NOT_PUBLIC_V6
                        .iter()
                        .any(|(network, bits)| within((IpAddr::V6(*network), *bits)))
            }
        },
    }
}

/// One entry of `outbound.allowed_private_hosts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A host name, compared without regard to case.
    Name(String),
    /// One address.
    Address(IpAddr),
    /// A range: an address and the number of leading bits that count.
    Range(IpAddr, u8),
}

/// Why an entry is not one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EntryError {
    /// It is empty.
    #[error("an entry is a host name, an address or a range")]
    Empty,
    /// A range whose address or length cannot be read.
    #[error("{0:?} is not a range such as 10.0.0.0/8")]
    Range(String),
}

impl Entry {
    /// Reads an entry: `a.b.c.d/n` or `x::/n` is a range; an IP address is an address; anything else is a
    /// host name.
    pub fn parse(text: &str) -> Result<Self, EntryError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(EntryError::Empty);
        }
        if let Some((network, bits)) = text.split_once('/') {
            let network: IpAddr = network
                .parse()
                .map_err(|_| EntryError::Range(text.to_owned()))?;
            let bits: u8 = bits
                .parse()
                .map_err(|_| EntryError::Range(text.to_owned()))?;
            let widest = if network.is_ipv4() { 32 } else { 128 };
            return if bits <= widest {
                Ok(Self::Range(network, bits))
            } else {
                Err(EntryError::Range(text.to_owned()))
            };
        }
        Ok(text.parse::<IpAddr>().map_or_else(
            |_| Self::Name(text.trim_end_matches('.').to_ascii_lowercase()),
            Self::Address,
        ))
    }

    fn lists(&self, host: &str, address: IpAddr) -> bool {
        match self {
            Self::Name(name) => name.eq_ignore_ascii_case(host.trim_end_matches('.')),
            Self::Address(listed) => same_address(*listed, address),
            Self::Range(network, bits) => in_range(*network, *bits, address),
        }
    }
}

fn unmapped(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 @ IpAddr::V4(_) => v4,
    }
}

fn same_address(a: IpAddr, b: IpAddr) -> bool {
    unmapped(a) == unmapped(b)
}

fn in_range(network: IpAddr, bits: u8, address: IpAddr) -> bool {
    match (unmapped(network), unmapped(address)) {
        (IpAddr::V4(net), IpAddr::V4(addr)) => {
            let mask = prefix_mask_v4(bits);
            u32::from(net) & mask == u32::from(addr) & mask
        }
        (IpAddr::V6(net), IpAddr::V6(addr)) => {
            let mask = prefix_mask_v6(bits);
            u128::from(net) & mask == u128::from(addr) & mask
        }
        _ => false,
    }
}

fn prefix_mask_v4(bits: u8) -> u32 {
    u32::MAX
        .checked_shl(32 - u32::from(bits.min(32)))
        .unwrap_or(0)
}

fn prefix_mask_v6(bits: u8) -> u128 {
    u128::MAX
        .checked_shl(128 - u32::from(bits.min(128)))
        .unwrap_or(0)
}

/// Why a connection is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{host} resolves to {address}, which is not a public address, and outbound.allowed_private_hosts does not list it"
)]
pub struct Refused {
    /// The host that was asked for.
    pub host: String,
    /// The address it resolved to, which is where the connection would go.
    pub address: IpAddr,
}

/// Whether a connection to `host`, which resolved to `address`, is allowed. `allowed` is the list handed to
/// the call: `None` for a channel that is not held to it, otherwise the entries. A public address is always
/// allowed; any other only if an entry lists the host's name or the address. This is
/// asked of the address a connection goes to, and again of every redirect it follows. An entry that cannot
/// be read lists nothing.
pub fn may_connect(host: &str, address: IpAddr, allowed: Option<&[String]>) -> Result<(), Refused> {
    let Some(entries) = allowed else {
        return Ok(());
    };
    if !is_private(address) {
        return Ok(());
    }
    let listed = entries
        .iter()
        .filter_map(|entry| Entry::parse(entry).ok())
        .any(|entry| entry.lists(host, address));
    if listed {
        Ok(())
    } else {
        Err(Refused {
            host: host.to_owned(),
            address,
        })
    }
}

/// The address of the machine itself, for tests that call a notifier with a private one.
pub const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// The loopback address in the other family.
pub const LOOPBACK_V6: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn loopback_link_local_and_private_addresses_are_private_and_public_ones_are_not() {
        for private in [
            "127.0.0.1",
            "127.8.9.10",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "::",
            "fe80::1",
            "fd12:3456::1",
            "::ffff:10.0.0.5",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_private(ip(private)), "{private}");
        }
        for public in [
            "8.8.8.8",
            "172.32.0.1",
            "172.15.255.255",
            "192.169.0.1",
            "93.184.216.34",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_private(ip(public)), "{public}");
        }
        assert!(is_private(LOOPBACK) && is_private(LOOPBACK_V6));
    }

    #[test]
    fn every_range_that_is_not_reachable_from_everywhere_is_private_up_to_its_edges() {
        // The first and the last address of each range, and the public address on either side of it.
        for (before, first, last, after) in [
            (
                "100.63.255.255",
                "100.64.0.0",
                "100.127.255.255",
                "100.128.0.0",
            ),
            (
                "126.255.255.255",
                "127.0.0.0",
                "127.255.255.255",
                "128.0.0.0",
            ),
            (
                "169.253.255.255",
                "169.254.0.0",
                "169.254.255.255",
                "169.255.0.0",
            ),
            (
                "172.15.255.255",
                "172.16.0.0",
                "172.31.255.255",
                "172.32.0.0",
            ),
            ("191.255.255.255", "192.0.0.0", "192.0.0.255", "192.0.1.0"),
            ("192.0.1.255", "192.0.2.0", "192.0.2.255", "192.0.3.0"),
            (
                "192.167.255.255",
                "192.168.0.0",
                "192.168.255.255",
                "192.169.0.0",
            ),
            (
                "198.17.255.255",
                "198.18.0.0",
                "198.19.255.255",
                "198.20.0.0",
            ),
            (
                "198.51.99.255",
                "198.51.100.0",
                "198.51.100.255",
                "198.51.101.0",
            ),
            (
                "203.0.112.255",
                "203.0.113.0",
                "203.0.113.255",
                "203.0.114.0",
            ),
            ("9.255.255.255", "10.0.0.0", "10.255.255.255", "11.0.0.0"),
        ] {
            assert!(!is_private(ip(before)), "{before} is public");
            assert!(is_private(ip(first)), "{first}");
            assert!(is_private(ip(last)), "{last}");
            assert!(!is_private(ip(after)), "{after} is public");
        }
        for private in [
            "0.1.2.3",
            "0.255.255.255",
            "224.0.0.1",
            "239.255.255.255",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(is_private(ip(private)), "{private}");
        }
        for public in ["1.0.0.0", "1.1.1.1", "223.255.255.255"] {
            assert!(!is_private(ip(public)), "{public}");
        }
    }

    #[test]
    fn an_ipv6_address_is_public_only_inside_the_range_public_ones_are_given_from() {
        for private in [
            "fec0::1",
            "ff02::1",
            "ff0e::1",
            "100::1",
            "64:ff9b:1::1",
            "5f00::1",
            "4000::1",
            "1fff:ffff::1",
            "2001::1",
            "2001:2::1",
            "2001:1ff:ffff::1",
            "2001:db8::1",
            "2001:db8:ffff:ffff::1",
            "3fff::1",
            "3fff:fff:ffff::1",
            "::7f00:1",
            "::a00:1",
        ] {
            assert!(is_private(ip(private)), "{private}");
        }
        for public in [
            "2000::1",
            "2001:200::1",
            "2001:db7:ffff::1",
            "2001:db9::1",
            "2620:fe::fe",
            "2a00:1450:4001::64",
            "3ffe:ffff::1",
            "3fff:1000::1",
        ] {
            assert!(!is_private(ip(public)), "{public}");
        }
    }

    #[test]
    fn an_ipv6_address_that_stands_for_an_ipv4_one_is_judged_as_that_one() {
        for private in [
            "64:ff9b::a00:1",
            "64:ff9b::7f00:1",
            "64:ff9b::6464:6464",
            "64:ff9b::a9fe:a9fe",
            "2002:a00:1::1",
            "2002:7f00:1::",
            "2002:c0a8:101:5::9",
            "::ffff:100.100.100.200",
            "::ffff:198.18.0.1",
        ] {
            assert!(is_private(ip(private)), "{private}");
        }
        for public in ["64:ff9b::808:808", "2002:808:808::1", "::ffff:1.1.1.1"] {
            assert!(!is_private(ip(public)), "{public}");
        }
        let none: [String; 0] = [];
        assert!(
            may_connect("hooks.example", ip("64:ff9b::a00:1"), Some(&none)).is_err(),
            "a private address behind a translator is refused as the private address is"
        );
        assert_eq!(
            may_connect(
                "hooks.example",
                ip("64:ff9b::a00:1"),
                Some(&["hooks.example".to_owned()])
            ),
            Ok(()),
            "and is allowed by its host's name as any other"
        );
    }

    #[test]
    fn a_public_host_is_always_allowed_and_a_private_one_only_when_it_is_listed() {
        let none: [String; 0] = [];
        assert_eq!(
            may_connect("api.telegram.org", ip("149.154.167.220"), Some(&none)),
            Ok(())
        );
        let refused = may_connect("internal.example", ip("10.0.0.7"), Some(&none)).unwrap_err();
        assert_eq!(
            refused,
            Refused {
                host: "internal.example".into(),
                address: ip("10.0.0.7")
            }
        );
        assert!(refused.to_string().contains("10.0.0.7"));
        assert!(may_connect("localhost", LOOPBACK, Some(&none)).is_err());
        assert_eq!(
            may_connect("localhost", LOOPBACK, None),
            Ok(()),
            "the system's own channels are not held to the list"
        );
    }

    #[test]
    fn a_shared_address_cannot_reach_an_internal_service_without_an_exception() {
        let endpoint = ip("100.100.100.200");
        let empty: [String; 0] = [];
        assert!(may_connect("relay.example", endpoint, Some(&empty)).is_err());
        assert!(
            may_connect("relay.example", ip("::ffff:100.100.100.200"), Some(&empty)).is_err(),
            "writing the same address as IPv6 does not make it public"
        );
        assert_eq!(
            may_connect(
                "relay.example",
                endpoint,
                Some(&["relay.example".to_owned()])
            ),
            Ok(()),
            "a listed host still works"
        );
        assert_eq!(
            may_connect("relay.example", endpoint, None),
            Ok(()),
            "the system's own channel keeps its configured access"
        );
    }

    #[test]
    fn a_private_host_is_allowed_by_its_name_its_address_or_a_range_that_holds_it() {
        let list = [
            "hooks.corp.example".to_owned(),
            "10.0.0.5".to_owned(),
            "192.168.0.0/16".to_owned(),
            "fd00::/8".to_owned(),
        ];
        let allowed = Some(list.as_slice());
        assert_eq!(
            may_connect("HOOKS.corp.example.", ip("10.9.9.9"), allowed),
            Ok(()),
            "by name, without case or the final dot"
        );
        assert_eq!(
            may_connect("db", ip("10.0.0.5"), allowed),
            Ok(()),
            "by address"
        );
        assert_eq!(
            may_connect("db", ip("::ffff:10.0.0.5"), allowed),
            Ok(()),
            "an IPv4 address written as IPv6 is the same"
        );
        assert_eq!(
            may_connect("db", ip("192.168.77.1"), allowed),
            Ok(()),
            "by range"
        );
        assert_eq!(
            may_connect("db", ip("fd12::1"), allowed),
            Ok(()),
            "by a range of the other family"
        );
        assert!(may_connect("db", ip("10.0.0.6"), allowed).is_err());
        assert!(
            may_connect("db", ip("192.169.0.1"), allowed).is_ok(),
            "it is a public address anyway"
        );
        assert!(may_connect("db", ip("172.20.0.1"), allowed).is_err());
    }

    #[test]
    fn entries_are_read_as_names_addresses_and_ranges_and_a_bad_one_lists_nothing() {
        assert_eq!(
            Entry::parse("Hooks.Example."),
            Ok(Entry::Name("hooks.example".into()))
        );
        assert_eq!(Entry::parse("10.0.0.5"), Ok(Entry::Address(ip("10.0.0.5"))));
        assert_eq!(
            Entry::parse("10.0.0.0/8"),
            Ok(Entry::Range(ip("10.0.0.0"), 8))
        );
        assert_eq!(Entry::parse("fd00::/8"), Ok(Entry::Range(ip("fd00::"), 8)));
        assert_eq!(Entry::parse(""), Err(EntryError::Empty));
        assert!(matches!(
            Entry::parse("10.0.0.0/33"),
            Err(EntryError::Range(_))
        ));
        assert!(matches!(Entry::parse("host/8"), Err(EntryError::Range(_))));
        let list = ["10.0.0.0/33".to_owned(), "   ".to_owned()];
        assert!(may_connect("db", ip("10.0.0.1"), Some(list.as_slice())).is_err());
        // A range of zero bits holds every address of its family.
        let all = ["0.0.0.0/0".to_owned()];
        assert_eq!(
            may_connect("db", ip("10.0.0.1"), Some(all.as_slice())),
            Ok(())
        );
        assert!(
            may_connect("db", ip("::1"), Some(all.as_slice())).is_err(),
            "not the other family"
        );
    }
}
