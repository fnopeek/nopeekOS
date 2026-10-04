//! Network reach: may a page go where it wants to?
//!
//! A public page must not reach the user's private network
//! (`docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1 V2). CORS does not cover this:
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
    /// The user's network: 10/8, 172.16/12, 192.168/16, 100.64/10.
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
        _ => Reach::Public,
    }
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

    /// The ordering carries the whole rule, so it is tested, not assumed.
    #[test]
    fn the_ordering_is_the_rule() {
        assert!(Reach::Local < Reach::Private);
        assert!(Reach::Private < Reach::Public);
    }
}
