//! A lease: what an OFFER or ACK grants, after systemd v262
//! `dhcp_lease_new_from_message` (`sd-dhcp-lease.c`). Times are relative
//! to `timestamp`, in microseconds.
//!
//! Not parsed: host and domain names, search list, DNR, SIP, captive
//! portal, time zone, 6rd and IPv6-only preferred; nothing here uses them.

use alloc::vec::Vec;
use super::message::{Error, Message};
use super::*;

pub use crate::ip::Route;

#[derive(Clone, Debug, PartialEq)]
pub struct Lease {
    pub address: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub server_address: [u8; 4],
    /// `INFINITY` for a lease that never ends.
    pub lifetime: u64,
    /// 0 when the lease never ends.
    pub t1: u64,
    pub t2: u64,
    pub broadcast: Option<[u8; 4]>,
    pub routers: Vec<[u8; 4]>,
    pub dns: Vec<[u8; 4]>,
    pub ntp: Vec<[u8; 4]>,
    pub static_routes: Vec<Route>,
    pub classless_routes: Vec<Route>,
    pub mtu: Option<u16>,
    /// When the message arrived.
    pub timestamp: u64,
}

/// `IPV4_MIN_REASSEMBLY_SIZE`.
const IPV4_MIN_REASSEMBLY_SIZE: u16 = 576;

/// `in4_addr_default_prefixlen`: the classful length, none for class D and E.
pub fn default_prefixlen(a: [u8; 4]) -> Option<u8> {
    match a[0] {
        0..=127 => Some(8),
        128..=191 => Some(16),
        192..=223 => Some(24),
        _ => None,
    }
}

pub fn prefixlen_to_netmask(len: u8) -> [u8; 4] {
    if len == 0 { [0; 4] } else { (u32::MAX << (32 - len as u32)).to_be_bytes() }
}

/// `in4_addr_netmask_to_prefixlen`: up to the lowest set bit, so a
/// non-contiguous mask reads as the contiguous one that covers it.
pub fn netmask_to_prefixlen(mask: [u8; 4]) -> u8 {
    let m = u32::from_be_bytes(mask);
    if m == 0 { 0 } else { (32 - m.trailing_zeros()) as u8 }
}

pub fn mask(a: [u8; 4], len: u8) -> [u8; 4] {
    (u32::from_be_bytes(a) & u32::from_be_bytes(prefixlen_to_netmask(len))).to_be_bytes()
}

/// `be32_sec_to_usec`.
fn sec_to_usec(t: [u8; 4], max_as_infinity: bool) -> u64 {
    let s = u32::from_be_bytes(t);
    if s == u32::MAX && max_as_infinity { INFINITY } else { s as u64 * USEC_PER_SEC }
}

/// An address a host may hold: not 0/8, loopback, multicast, 240/4 or the
/// limited broadcast. systemd checks only for 0.0.0.0.
pub fn is_unicast(a: [u8; 4]) -> bool {
    use crate::ip::{is_lbcast, is_loopback, is_multicast, is_reserved, is_zeronet};
    !(is_zeronet(a) || is_loopback(a) || is_multicast(a) || is_reserved(a) || is_lbcast(a))
}

/// `dhcp_message_get_option_static_routes` (option 33).
fn static_routes(m: &Message) -> Result<Vec<Route>, Error> {
    let Some(addrs) = m.option_addresses(OPT_STATIC_ROUTE)? else { return Ok(Vec::new()) };
    if addrs.len() % 2 != 0 {
        return Err(Error::BadMsg);
    }
    let mut routes = Vec::new();
    for p in addrs.chunks(2) {
        let Some(len) = default_prefixlen(p[0]) else { continue };
        let dst = mask(p[0], len);
        // RFC 2132 §5.8: the default route is not a valid destination.
        if dst == [0; 4] {
            continue;
        }
        routes.push(Route { dst, len, gw: p[1] });
    }
    Ok(routes)
}

/// `dhcp_message_get_option_classless_static_routes` (RFC 3442).
fn classless_routes(m: &Message, code: u8) -> Result<Vec<Route>, Error> {
    let Some(v) = m.option(code) else { return Ok(Vec::new()) };
    let mut routes = Vec::new();
    let mut i = &v[..];
    while !i.is_empty() {
        let len = i[0];
        i = &i[1..];
        if len > 32 {
            return Err(Error::BadMsg);
        }
        let n = (len as usize).div_ceil(8);
        if n > i.len() {
            return Err(Error::BadMsg);
        }
        let mut dst = [0; 4];
        dst[..n].copy_from_slice(&i[..n]);
        let dst = mask(dst, len);
        i = &i[n..];
        if i.len() < 4 {
            return Err(Error::BadMsg);
        }
        let gw = i[..4].try_into().unwrap();
        i = &i[4..];
        routes.push(Route { dst, len, gw });
    }
    Ok(routes)
}

impl Lease {
    /// `dhcp_lease_new_from_message`; `None` when a mandatory part is
    /// missing. A malformed optional part is left out.
    pub fn from_message(m: &Message, fallback_lifetime: u64) -> Option<Lease> {
        if m.yiaddr == [0; 4] || !is_unicast(m.yiaddr) {
            return None;
        }
        let subnet_mask = match m.option_addr(OPT_SUBNET_MASK) {
            Some(mask) => mask,
            None => prefixlen_to_netmask(default_prefixlen(m.yiaddr)?),
        };
        let server_address = m.option_addr(OPT_SERVER_IDENTIFIER)?;

        let mut lifetime = match m.option_exact(OPT_LEASE_TIME, 4) {
            Some(t) if t != [0; 4] => sec_to_usec(t.try_into().unwrap(), true),
            _ if fallback_lifetime != 0 => fallback_lifetime,
            _ => return None,
        };
        // A very short lease makes the link unstable.
        if lifetime <= 30 * USEC_PER_SEC {
            lifetime = 30 * USEC_PER_SEC;
        }
        let (mut t1, mut t2) = (0, 0);
        if lifetime != INFINITY {
            t2 = m.option_exact(OPT_REBINDING_TIME, 4)
                .map_or(0, |t| sec_to_usec(t.try_into().unwrap(), true));
            // RFC 2131 §4.4.5: T2 defaults to 0.875 of the lease.
            if t2 == 0 || t2 >= lifetime {
                t2 = lifetime * 7 / 8;
            }
            t1 = m.option_exact(OPT_RENEWAL_TIME, 4)
                .map_or(0, |t| sec_to_usec(t.try_into().unwrap(), true));
            // RFC 2131 §4.4.5: T1 defaults to half of it.
            if t1 == 0 || t1 >= t2 {
                t1 = lifetime / 2;
            }
            if t1 >= t2 {
                t2 = lifetime * 7 / 8;
            }
        }

        let addrs = |code| m.option_addresses(code).ok().flatten().unwrap_or_default();
        let mut classless = classless_routes(m, OPT_CLASSLESS_STATIC_ROUTE).unwrap_or_default();
        if classless.is_empty() {
            classless = classless_routes(m, OPT_PRIVATE_CLASSLESS_STATIC_ROUTE).unwrap_or_default();
        }
        Some(Lease {
            address: m.yiaddr,
            subnet_mask,
            server_address,
            lifetime,
            t1,
            t2,
            broadcast: m.option_addr(OPT_BROADCAST),
            routers: addrs(OPT_ROUTER),
            dns: addrs(OPT_DOMAIN_NAME_SERVER),
            ntp: addrs(OPT_NTP_SERVER),
            static_routes: static_routes(m).unwrap_or_default(),
            classless_routes: classless,
            // RFC 791, RFC 1122: below 576 a link is not usable for IPv4.
            mtu: m.option_u16(OPT_MTU_INTERFACE).filter(|&v| v >= IPV4_MIN_REASSEMBLY_SIZE),
            timestamp: 0,
        })
    }

    /// `sd_dhcp_lease_get_prefix`.
    pub fn prefixlen(&self) -> u8 {
        netmask_to_prefixlen(self.subnet_mask)
    }

    /// `dhcp4_get_router`: the first router that is set.
    pub fn router(&self) -> Option<[u8; 4]> {
        self.routers.iter().copied().find(|&r| r != [0; 4])
    }

    /// `lease_equal`.
    pub fn same_binding(&self, b: &Lease) -> bool {
        self.address == b.address && self.subnet_mask == b.subnet_mask && self.routers == b.routers
    }

    /// The routes networkd installs for this lease (`networkd-dhcp4.c`):
    /// the classless routes, or else the static routes and a default route
    /// via the router (RFC 3442: classless routes override both), each
    /// gateway with an on-link route to itself, and the connected prefix.
    /// A route to our own address or to loopback is not a route.
    pub fn routes(&self) -> Vec<Route> {
        let len = self.prefixlen();
        let mut out = alloc::vec![Route { dst: mask(self.address, len), len, gw: [0; 4] }];
        let mut add = |r: Route| {
            if crate::ip::is_loopback(r.dst) || r.dst == self.address {
                return;
            }
            if r.gw != [0; 4] {
                let to_gw = Route { dst: r.gw, len: 32, gw: [0; 4] };
                if !out.contains(&to_gw) {
                    out.push(to_gw);
                }
            }
            if !out.contains(&r) {
                out.push(r);
            }
        };
        if !self.classless_routes.is_empty() {
            for &r in &self.classless_routes {
                add(r);
            }
            return out;
        }
        for &r in &self.static_routes {
            add(r);
        }
        if let Some(gw) = self.router() {
            add(Route { dst: [0; 4], len: 0, gw });
        }
        out
    }
}
