//! Network reach: may a page go where it wants to?
//!
//! A public page must not reach the user's private network.
//! CORS does not cover this:
//! it protects the target server, not the network the browser sits in.
//! Browsers retrofitted the rule as Private Network Access.
//!
//! This file depends on `core` only (no `alloc`, no `crate::`), so
//! `beak-engine` can mount it into its test tree and run the tests below
//! against this one implementation; the kernel has no test harness.

/// How open a network range is. The ordering is the rule:
/// `Local < Private < Public`, and a request may never go from more open
/// to more closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// This machine: 127/8 and the link-local addresses, where local
    /// services may listen.
    Local,
    /// The user's network: 10/8, 172.16/12, 192.168/16, 100.64/10, the
    /// other ranges RFC 6890 marks as not global, and the local link.
    Private,
    /// The open internet.
    Public,
}

/// The range an address belongs to.
///
/// Decided on the address, never the name: a name can resolve elsewhere
/// the second time (DNS rebinding), an address cannot change.
pub fn classify_ip(ip: [u8; 4]) -> Reach {
    match ip {
        // 0.0.0.0/8 means "this host, this network" and some stacks treat it
        // like loopback, so it gets the strictest class.
        [0, ..] => Reach::Local,
        [127, ..] => Reach::Local,
        // Link-local 169.254/16: home of cloud metadata services, the
        // classic target.
        [169, 254, ..] => Reach::Local,
        [10, ..] => Reach::Private,
        [172, b, ..] if (16..=31).contains(&b) => Reach::Private,
        [192, 168, ..] => Reach::Private,
        // Carrier-grade NAT (100.64/10): not routable on the open internet,
        // so treated like 10/8.
        [100, b, ..] if (64..=127).contains(&b) => Reach::Private,
        // The rest of RFC 6890's "Global: False": IETF protocol assignments,
        // the documentation nets, benchmarking, and the reserved 240/4.
        [192, 0, 0, _] | [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _] => Reach::Private,
        [198, 18 | 19, ..] => Reach::Private,
        [b, ..] if b >= 240 => Reach::Private,
        _ => Reach::Public,
    }
}

/// Whether `ip` names one host. A page must never make the kernel send to
/// a group: multicast 224/4 and the limited broadcast (RFC 1122 3.2.1.3).
pub fn is_unicast(ip: [u8; 4]) -> bool {
    !(224..=239).contains(&ip[0]) && ip != [255, 255, 255, 255]
}

/// `classify_ip`, knowing our own link: a host on the local segment (inside
/// `addr`/`mask`, or the gateway) is the user's network even when the network
/// is numbered from public space.
pub fn classify_on_link(ip: [u8; 4], addr: [u8; 4], mask: [u8; 4], gateway: [u8; 4]) -> Reach {
    let c = classify_ip(ip);
    let on_link = addr != [0; 4] && mask != [0; 4]
        && (0..4).all(|i| ip[i] & mask[i] == addr[i] & mask[i]);
    if on_link || (gateway != [0; 4] && ip == gateway) { c.min(Reach::Private) } else { c }
}

/// The directed broadcast of our own subnet, which `is_unicast` cannot see.
pub fn is_subnet_broadcast(ip: [u8; 4], addr: [u8; 4], mask: [u8; 4]) -> bool {
    addr != [0; 4] && mask != [0; 4] && mask != [255; 4]
        && (0..4).all(|i| ip[i] == addr[i] & mask[i] | !mask[i])
}

/// May a document of class `from` reach an address of class `to`?
///
/// Never from more open to more closed: a public page stays outside, a
/// private one may reach its own network and the internet, a local one
/// may reach everything.
pub fn allows(from: Reach, to: Reach) -> bool {
    to >= from
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_private_ranges_are_private() {
        for ip in [[10, 0, 0, 1], [10, 255, 255, 255], [192, 168, 1, 1],
                   [172, 16, 0, 1], [172, 31, 255, 254], [100, 64, 0, 1],
                   [100, 127, 255, 255]] {
            assert_eq!(classify_ip(ip), Reach::Private, "{ip:?}");
        }
    }

    #[test]
    fn the_local_ranges_are_local() {
        for ip in [[127, 0, 0, 1], [127, 1, 2, 3], [169, 254, 169, 254], [0, 0, 0, 0]] {
            assert_eq!(classify_ip(ip), Reach::Local, "{ip:?}");
        }
    }

    /// The neighbours of the private ranges are public; an off-by-one here
    /// would be a silent hole.
    #[test]
    fn the_neighbours_of_the_private_ranges_are_public() {
        for ip in [[9, 255, 255, 255], [11, 0, 0, 1],
                   [172, 15, 0, 1], [172, 32, 0, 1],
                   [192, 167, 0, 1], [192, 169, 0, 1],
                   [100, 63, 255, 255], [100, 128, 0, 1],
                   [126, 0, 0, 1], [128, 0, 0, 1],
                   [169, 253, 0, 1], [169, 255, 0, 1],
                   [8, 8, 8, 8], [1, 1, 1, 1]] {
            assert_eq!(classify_ip(ip), Reach::Public, "{ip:?}");
        }
    }

    #[test]
    fn a_public_page_never_reaches_inward() {
        assert!(!allows(Reach::Public, Reach::Private));
        assert!(!allows(Reach::Public, Reach::Local));
        assert!(allows(Reach::Public, Reach::Public));
    }

    #[test]
    fn a_page_may_always_reach_outward() {
        assert!(allows(Reach::Private, Reach::Public));
        assert!(allows(Reach::Local, Reach::Public));
        assert!(allows(Reach::Local, Reach::Private));
    }

    /// A router's admin page may load its own assets.
    #[test]
    fn the_router_page_may_load_its_own_assets() {
        assert!(allows(Reach::Private, Reach::Private));
        assert!(allows(Reach::Local, Reach::Local));
    }

    #[test]
    fn the_rfc6890_non_global_ranges_are_private() {
        for ip in [[192, 0, 0, 9], [192, 0, 2, 1], [198, 18, 0, 1], [198, 19, 255, 1],
                   [198, 51, 100, 7], [203, 0, 113, 7], [240, 0, 0, 1]] {
            assert_eq!(classify_ip(ip), Reach::Private, "{ip:?}");
        }
        for ip in [[198, 17, 0, 1], [198, 20, 0, 1], [192, 0, 1, 1], [192, 88, 99, 1]] {
            assert_eq!(classify_ip(ip), Reach::Public, "{ip:?}");
        }
    }

    #[test]
    fn groups_are_not_unicast() {
        assert!(!is_unicast([255, 255, 255, 255]));
        assert!(!is_unicast([224, 0, 0, 251]));
        assert!(!is_unicast([239, 255, 255, 250]));
        assert!(is_unicast([8, 8, 8, 8]));
        assert!(is_subnet_broadcast([192, 168, 1, 255], [192, 168, 1, 7], [255, 255, 255, 0]));
        assert!(!is_subnet_broadcast([192, 168, 1, 254], [192, 168, 1, 7], [255, 255, 255, 0]));
    }

    /// A LAN numbered from public space is still the user's network.
    #[test]
    fn a_public_lan_is_private() {
        let (addr, mask, gw) = ([130, 59, 10, 20], [255, 255, 255, 0], [130, 59, 10, 1]);
        assert_eq!(classify_on_link([130, 59, 10, 77], addr, mask, gw), Reach::Private);
        assert_eq!(classify_on_link([130, 59, 11, 77], addr, mask, gw), Reach::Public);
        assert_eq!(classify_on_link([127, 0, 0, 1], addr, mask, gw), Reach::Local);
    }

    /// The ordering carries the whole rule, so it is tested, not assumed.
    #[test]
    fn the_ordering_is_the_rule() {
        assert!(Reach::Local < Reach::Private);
        assert!(Reach::Private < Reach::Public);
    }
}
