//! NAT for the microvm's virtio-net device.
//!
//! The guest sees a `10.99.0.0/24` link with a single peer at
//! `10.99.0.1` (the synthetic gateway). Per guest TX frame:
//!
//!   * ARP-Request for 10.99.0.1 → synth ARP-Reply (link-layer)
//!   * UDP to 10.99.0.1:53        → host `net::dns::resolve`, synth
//!                                  DNS-Reply
//!   * everything else (TCP/UDP/QUIC to real remotes) → **L3
//!     masquerade**: we do NOT terminate. Outbound packets are SNAT'd
//!     to our host IP + a masquerade port and sent via the host IP
//!     layer; replies are intercepted in `net::ipv4` (`l3_inbound`),
//!     rewritten back to the guest, and injected by `pump`. The
//!     guest's real Linux TCP/UDP/QUIC runs end-to-end with the
//!     server — reliability/ordering/SACK/window-scaling are theirs,
//!     not ours. See the `L3 masquerade NAT` section below.
//!
//! ARP/DNS handlers return a fully-built virtio-net frame (virtio hdr
//! + ethernet + IPv4/UDP/payload); `virtio_net_pci.rs` walks the
//! avail-ring, writes it to a driver buffer, signals used + IRQ.

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;
use super::guest_mem::GuestMem;
use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;

/// Locally-administered guest MAC. Must match the one virtio-net
/// advertises through its device-cfg MAC field.
pub const GUEST_MAC:   [u8; 6] = [0x52, 0x54, 0x00, 0x6E, 0x70, 0x6B];
/// MAC the host pretends to be on the synthetic gateway.
pub const GATEWAY_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x6E, 0x70, 0x01];
/// Synthetic gateway IP. ARP, DNS, and (later) NAT all live here.
pub const GATEWAY_IP:  [u8; 4] = [10, 99, 0, 1];
/// Guest IP — PID-1 hard-codes the same value via SIOCSIFADDR.
pub const GUEST_IP:    [u8; 4] = [10, 99, 0, 2];

const VNET_HDR_LEN: usize = 12;
const ETH_HDR_LEN:  usize = 14;
const ARP_LEN:      usize = 28;
const IPV4_HDR_LEN: usize = 20;
const UDP_HDR_LEN:  usize = 8;
const DNS_HDR_LEN:  usize = 12;

const ETHERTYPE_IPV4: u16 = 0x0800;
const ETHERTYPE_ARP:  u16 = 0x0806;

const PROTO_ICMP: u8 = 1;
const PROTO_UDP:  u8 = 17;
const PROTO_TCP:  u8 = 6;
const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY:   u8 = 0;

const PORT_DNS: u16 = 53;

/// TCP header length without options — used to bound L4 checksum
/// recompute in the L3 path.
const TCP_HDR_LEN: usize = 20;

/// Per-VM network policy. The browser default (`dns_tcp`) allows DNS +
/// TCP + UDP (QUIC) through the L3 masquerade; ICMP still needs an
/// explicit cap. Future work threads this through the microvm session
/// cap so different apps can have different policies.
#[derive(Clone, Copy, Debug)]
pub struct NetCaps {
    pub allow_dns:  bool,
    pub allow_icmp: bool,
    pub allow_udp:  bool,
    pub allow_tcp:  bool,
}

impl NetCaps {
    pub const fn dns_only() -> Self {
        Self { allow_dns: true, allow_icmp: false, allow_udp: false, allow_tcp: false }
    }
    /// Browser default: DNS + TCP + UDP (QUIC/HTTP-3) + ICMP echo (ping /
    /// reachability probes) via L3 masquerade.
    pub const fn dns_tcp() -> Self {
        Self { allow_dns: true, allow_icmp: true, allow_udp: true, allow_tcp: true }
    }
}

impl Default for NetCaps {
    fn default() -> Self { Self::dns_tcp() }
}

// ===========================================================================
// L3 masquerade NAT
//
// We do NOT terminate TCP. The guest's real Linux TCP/UDP/QUIC talks
// end-to-end with the real server; we only rewrite IP packets:
//   outbound  guest(10.99.0.2:p → R:q)  →  send from our_ip:HP → R:q
//   inbound   R:q → our_ip:HP           →  inject  R:q → 10.99.0.2:p
// Reliability, ordering, SACK, window-scaling, QUIC: all owned by Linux
// and the server. We are a stateless-ish packet rewriter + a 4-tuple
// table — no rtx/ack/RTO logic, none of the termination brittleness.
// ===========================================================================

use core::sync::atomic::{AtomicBool, Ordering as AtOrd};
use alloc::collections::VecDeque;

const L3_MAX: usize = 256;
/// Masquerade host-port pool. Strictly below the host TCP stack's own
/// ephemeral range (49152..=65534, net/tcp.rs) so a guest flow can
/// never alias a host-originated connection (OTA `update`, `https`).
const L3_PORT_LO: u16 = 20000;
const L3_PORT_HI: u16 = 40000;
const L3_TCP_IDLE_TICKS: u64 = 12_000; // ~2 min  (~100 ticks/s)
const L3_UDP_IDLE_TICKS: u64 = 3_000;  // ~30 s

#[derive(Clone, Copy)]
struct L3Map {
    proto: u8,
    guest_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    host_port: u16,
    last_tick: u64,
}

static L3: Mutex<[Option<L3Map>; L3_MAX]> = Mutex::new([const { None }; L3_MAX]);
/// Gates the host-RX inbound intercept. Off ⇒ `l3_inbound` is a cheap
/// `false` so a guest-less host (plain OTA/https) is never touched.
static L3_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Rewritten guest-bound frames produced from the host-RX context,
/// drained + injected by `pump` on the VM thread (same split the old
/// termination pump used, to keep virtio access on one thread).
static INBOUND_Q: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());

/// Find an existing mapping for this guest flow or allocate one.
/// Returns the masquerade host port.
fn l3_map_out(proto: u8, gport: u16, rip: [u8; 4], rport: u16, now: u64) -> Option<u16> {
    let mut tbl = L3.lock();
    // Existing?
    for m in tbl.iter_mut().flatten() {
        if m.proto == proto && m.guest_port == gport
            && m.remote_ip == rip && m.remote_port == rport
        {
            m.last_tick = now;
            return Some(m.host_port);
        }
    }
    // Allocate a host port not currently in the table.
    let mut hp = L3_PORT_LO;
    'scan: while hp < L3_PORT_HI {
        if !tbl.iter().flatten().any(|m| m.host_port == hp) { break 'scan; }
        hp += 1;
    }
    if hp >= L3_PORT_HI { return None; }
    let slot = tbl.iter_mut().find(|s| s.is_none())?;
    *slot = Some(L3Map { proto, guest_port: gport, remote_ip: rip,
                          remote_port: rport, host_port: hp, last_tick: now });
    Some(hp)
}

/// Reverse lookup for an inbound reply: (proto, host_port) + remote
/// must match. Returns the guest port to deliver to.
fn l3_map_in(proto: u8, hport: u16, rip: [u8; 4], rport: u16, now: u64) -> Option<u16> {
    let mut tbl = L3.lock();
    for m in tbl.iter_mut().flatten() {
        if m.proto == proto && m.host_port == hport
            && m.remote_ip == rip && m.remote_port == rport
        {
            m.last_tick = now;
            return Some(m.guest_port);
        }
    }
    None
}

/// Recompute the TCP/UDP checksum after an address/port rewrite.
/// TCP checksum is mandatory; UDP-over-IPv4 may be zero, which we use
/// (cheaper, always valid) so QUIC payload size isn't a concern.
fn fix_l4_checksum(proto: u8, src_ip: [u8; 4], dst_ip: [u8; 4], l4: &mut [u8]) {
    if proto == PROTO_TCP {
        if l4.len() < TCP_HDR_LEN { return; }
        l4[16] = 0; l4[17] = 0;
        let c = tcp_checksum(src_ip, dst_ip, l4);
        l4[16..18].copy_from_slice(&c.to_be_bytes());
    } else if proto == PROTO_UDP {
        if l4.len() < UDP_HDR_LEN { return; }
        l4[6] = 0; l4[7] = 0; // 0 = checksum disabled (valid for IPv4)
    }
}

/// Outbound SNAT: rewrite the guest's L4 source port to a masquerade
/// host port and send from our host IP. The guest's TCP/UDP semantics
/// (seq/ack/window/options/QUIC) pass through untouched.
fn l3_outbound(proto: u8, src_port: u16, dst_ip: [u8; 4],
                dst_port: u16, l4: &[u8]) -> Option<Vec<u8>> {
    let now = crate::interrupts::ticks();
    let hp = match l3_map_out(proto, src_port, dst_ip, dst_port, now) {
        Some(p) => p,
        None => { kprintln!("[nat] L3 table full, dropping flow"); return None; }
    };
    let mut seg = l4.to_vec();
    seg[0..2].copy_from_slice(&hp.to_be_bytes());          // src port → host port
    let our_ip = crate::net::arp::our_ip();
    fix_l4_checksum(proto, our_ip, dst_ip, &mut seg);
    crate::net::ipv4::send(dst_ip, proto, &seg);
    L3_ACTIVE.store(true, AtOrd::Release);
    None
}

/// Outbound NAT for a guest ICMP echo request (so ping + the browser's
/// reachability probes to the DNS servers work instead of cap-rejecting). The
/// ICMP Identifier is the flow key, masqueraded like a port; only echo
/// requests (type 8) are forwarded, everything else dropped. Mirrors
/// `l3_outbound` but rewrites the id + uses the pseudo-header-less ICMP
/// checksum.
fn l3_icmp_outbound(dst_ip: [u8; 4], l4: &[u8]) -> Option<Vec<u8>> {
    if l4.len() < 8 || l4[0] != ICMP_ECHO_REQUEST {
        return None;
    }
    let guest_id = u16::from_be_bytes([l4[4], l4[5]]);
    let now = crate::interrupts::ticks();
    let hp = l3_map_out(PROTO_ICMP, guest_id, dst_ip, 0, now)?;
    let mut seg = l4.to_vec();
    seg[4..6].copy_from_slice(&hp.to_be_bytes()); // id → masquerade id
    fix_icmp_checksum(&mut seg);
    crate::net::ipv4::send(dst_ip, PROTO_ICMP, &seg);
    L3_ACTIVE.store(true, AtOrd::Release);
    None
}

/// ICMP checksum: ones-complement sum over the whole ICMP message (no pseudo-
/// header, unlike TCP/UDP). Zero the field first, then fold carries.
fn fix_icmp_checksum(icmp: &mut [u8]) {
    if icmp.len() < 4 {
        return;
    }
    icmp[2] = 0;
    icmp[3] = 0;
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < icmp.len() {
        sum += u16::from_be_bytes([icmp[i], icmp[i + 1]]) as u32;
        i += 2;
    }
    if i < icmp.len() {
        sum += (icmp[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let c = !(sum as u16);
    icmp[2..4].copy_from_slice(&c.to_be_bytes());
}

/// Host-RX intercept. `ip` is a full IPv4 packet already filtered to
/// our IP. If it matches a masquerade mapping, rewrite it back to the
/// guest, enqueue for `pump`, and return true (consume — the host
/// stack must NOT also process it). Cheap `false` when no VM is up.
pub fn l3_inbound(ip: &[u8]) -> bool {
    if !L3_ACTIVE.load(AtOrd::Acquire) { return false; }
    if ip.len() < IPV4_HDR_LEN { return false; }
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN || ip.len() < ihl { return false; }
    let proto = ip[9];
    if proto != PROTO_TCP && proto != PROTO_UDP && proto != PROTO_ICMP { return false; }
    let src_ip: [u8; 4] = ip[12..16].try_into().unwrap();
    let l4 = &ip[ihl..];
    // ICMP echo reply: the Identifier (l4[4..6]) is the masquerade key (no
    // ports). TCP/UDP: remote port + masquerade host port are l4[0..2]/[2..4].
    let (remote_port, host_port) = if proto == PROTO_ICMP {
        if l4.len() < 8 || l4[0] != ICMP_ECHO_REPLY { return false; }
        (0u16, u16::from_be_bytes([l4[4], l4[5]]))
    } else {
        if l4.len() < 4 { return false; }
        (u16::from_be_bytes([l4[0], l4[1]]), u16::from_be_bytes([l4[2], l4[3]]))
    };
    let now = crate::interrupts::ticks();
    let gport = match l3_map_in(proto, host_port, src_ip, remote_port, now) {
        Some(g) => g,
        None => return false,
    };

    // Rewrite: dst IP → guest, L4 dst port → guest port; recompute
    // both checksums. Wrap in vnet + eth (gateway → guest).
    let mut frame = alloc::vec![0u8; VNET_HDR_LEN + ETH_HDR_LEN + ip.len()];
    write_eth(&mut frame, &GUEST_MAC, &GATEWAY_MAC, ETHERTYPE_IPV4);
    let ip_off = VNET_HDR_LEN + ETH_HDR_LEN;
    frame[ip_off..].copy_from_slice(ip);
    frame[ip_off + 16..ip_off + 20].copy_from_slice(&GUEST_IP);
    frame[ip_off + 10] = 0; frame[ip_off + 11] = 0;
    let ipc = ipv4_checksum(&frame[ip_off..ip_off + ihl]);
    frame[ip_off + 10..ip_off + 12].copy_from_slice(&ipc.to_be_bytes());
    let l4_off = ip_off + ihl;
    if proto == PROTO_ICMP {
        // Rewrite the echo-reply Identifier back to the guest's + recompute
        // the ICMP checksum (the IP dst rewrite above doesn't affect it).
        frame[l4_off + 4..l4_off + 6].copy_from_slice(&gport.to_be_bytes());
        fix_icmp_checksum(&mut frame[l4_off..]);
    } else {
        frame[l4_off + 2..l4_off + 4].copy_from_slice(&gport.to_be_bytes());
        fix_l4_checksum(proto, src_ip, GUEST_IP, &mut frame[l4_off..]);
    }

    INBOUND_Q.lock().push_back(frame);
    true
}

/// Drop idle mappings so the table can't fill over a long session.
fn l3_reap(now: u64) {
    let mut tbl = L3.lock();
    for slot in tbl.iter_mut() {
        if let Some(m) = slot.as_ref() {
            let idle = now.wrapping_sub(m.last_tick);
            let max = if m.proto == PROTO_TCP { L3_TCP_IDLE_TICKS }
                      else { L3_UDP_IDLE_TICKS };
            if idle > max { *slot = None; }
        }
    }
}

/// Tear down all L3 state (VM stopped). Idempotent.
pub fn l3_reset() {
    L3_ACTIVE.store(false, AtOrd::Release);
    *L3.lock() = [const { None }; L3_MAX];
    INBOUND_Q.lock().clear();
}

/// Classify a guest TX frame (virtio-net hdr + ethernet) and produce
/// zero or more RX frames to inject back. Side-effects: kprintln on
/// cap-rejects so the operator can see why a packet went nowhere.
pub fn process_tx(payload: &[u8], caps: &NetCaps) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if payload.len() < VNET_HDR_LEN + ETH_HDR_LEN { return out; }
    let frame = &payload[VNET_HDR_LEN..];
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);

    match ethertype {
        ETHERTYPE_ARP => {
            if let Some(rep) = handle_arp(frame) { out.push(rep); }
        }
        ETHERTYPE_IPV4 => {
            if let Some(rep) = handle_ipv4(frame, caps) { out.push(rep); }
        }
        _ => {
            // Quiet: IPv6 / LLDP / STP / etc. — guest has nothing
            // useful to do with them on this synthetic link.
        }
    }
    out
}

/// ARP-Request for `GATEWAY_IP` → build matching ARP-Reply.
fn handle_arp(frame: &[u8]) -> Option<Vec<u8>> {
    if frame.len() < ETH_HDR_LEN + ARP_LEN { return None; }
    let arp = &frame[ETH_HDR_LEN..];
    let oper = u16::from_be_bytes([arp[6], arp[7]]);
    if oper != 1 { return None; }                   // not a request
    if &arp[24..28] != GATEWAY_IP { return None; }

    let mut sender_mac = [0u8; 6];
    sender_mac.copy_from_slice(&arp[8..14]);
    let mut sender_ip = [0u8; 4];
    sender_ip.copy_from_slice(&arp[14..18]);

    let mut reply = alloc::vec![0u8; VNET_HDR_LEN + ETH_HDR_LEN + ARP_LEN];
    write_eth(&mut reply, &sender_mac, &GATEWAY_MAC, ETHERTYPE_ARP);
    let arp_off = VNET_HDR_LEN + ETH_HDR_LEN;
    reply[arp_off + 0] = 0x00; reply[arp_off + 1] = 0x01; // htype = Ethernet
    reply[arp_off + 2] = 0x08; reply[arp_off + 3] = 0x00; // ptype = IPv4
    reply[arp_off + 4] = 6;
    reply[arp_off + 5] = 4;
    reply[arp_off + 6] = 0x00; reply[arp_off + 7] = 0x02; // oper = REPLY
    reply[arp_off +  8..arp_off + 14].copy_from_slice(&GATEWAY_MAC);
    reply[arp_off + 14..arp_off + 18].copy_from_slice(&GATEWAY_IP);
    reply[arp_off + 18..arp_off + 24].copy_from_slice(&sender_mac);
    reply[arp_off + 24..arp_off + 28].copy_from_slice(&sender_ip);
    Some(reply)
}

/// IPv4 dispatch: only UDP→10.99.0.1:53 has a real handler today.
/// Everything else logs a cap-reject and returns None.
fn handle_ipv4(frame: &[u8], caps: &NetCaps) -> Option<Vec<u8>> {
    if frame.len() < ETH_HDR_LEN + IPV4_HDR_LEN { return None; }
    let ip = &frame[ETH_HDR_LEN..];
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN || frame.len() < ETH_HDR_LEN + ihl { return None; }

    let proto = ip[9];
    let src_ip: [u8; 4] = ip[12..16].try_into().ok()?;
    let dst_ip: [u8; 4] = ip[16..20].try_into().ok()?;
    // Clamp L4 to the IP total-length. The guest TX buffer is bigger
    // than the packet (min-frame / driver padding); &ip[ihl..] would
    // append that garbage to every outbound segment → the server
    // misframes the response → Firefox reads a wild length → ~4 GiB
    // alloc → crash. Inbound is already clamped in net/ipv4.rs.
    let ip_total = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    if ip_total < ihl || ip_total > ip.len() { return None; }
    let l4 = &ip[ihl..ip_total];

    match proto {
        PROTO_UDP => {
            if l4.len() < UDP_HDR_LEN { return None; }
            let src_port = u16::from_be_bytes([l4[0], l4[1]]);
            let dst_port = u16::from_be_bytes([l4[2], l4[3]]);
            let udp_len  = u16::from_be_bytes([l4[4], l4[5]]) as usize;
            if udp_len < UDP_HDR_LEN || udp_len > l4.len() { return None; }
            let dgram = &l4[UDP_HDR_LEN..udp_len];

            if dst_ip == GATEWAY_IP && dst_port == PORT_DNS {
                if !caps.allow_dns {
                    kprintln!("[nat] DNS query dropped (cap allow_dns=false)");
                    return None;
                }
                handle_dns(src_ip, src_port, dgram)
            } else if dst_ip == GATEWAY_IP {
                None    // other gateway-directed UDP: nothing here
            } else {
                if !caps.allow_udp {
                    cap_reject("UDP", dst_ip, dst_port);
                    return None;
                }
                l3_outbound(PROTO_UDP, src_port, dst_ip, dst_port, l4)
            }
        }
        PROTO_TCP => {
            if l4.len() < 4 { return None; }
            let src_port = u16::from_be_bytes([l4[0], l4[1]]);
            let dst_port = u16::from_be_bytes([l4[2], l4[3]]);
            if !caps.allow_tcp {
                cap_reject("TCP", dst_ip, dst_port);
                return None;
            }
            l3_outbound(PROTO_TCP, src_port, dst_ip, dst_port, l4)
        }
        PROTO_ICMP => {
            if !caps.allow_icmp {
                cap_reject("ICMP", dst_ip, 0);
                return None;
            }
            l3_icmp_outbound(dst_ip, l4)
        }
        _ => None,
    }
}

/// rcode-relevant outcome of a lookup. The NoData vs NxDomain split is
/// load-bearing: a non-A query (AAAA / HTTPS-SVCB type 65) on a name
/// that exists MUST be NOERROR/NODATA, not NXDOMAIN. Firefox queries
/// the HTTPS RR before every connection and reads NXDOMAIN as "host
/// does not exist" → "secure site not available".
enum DnsOutcome {
    Answer([u8; 4]),  // A record
    NoData,           // NOERROR, no answer — name exists, no such RR type
    NxDomain,         // name does not exist
}

/// Parse a DNS query, resolve A records via the host resolver, and
/// synthesize the reply with the correct rcode.
fn handle_dns(src_ip: [u8; 4], src_port: u16, dgram: &[u8]) -> Option<Vec<u8>> {
    let q = parse_dns_query(dgram)?;

    let outcome = if q.qtype == 1 {
        match crate::net::dns::resolve(q.name.as_str()) {
            Some(ip) => DnsOutcome::Answer(ip),
            None     => DnsOutcome::NxDomain,
        }
    } else {
        // AAAA / HTTPS-SVCB / etc.: we don't serve the record, but the
        // name exists. NODATA — NXDOMAIN here poisons the whole host.
        DnsOutcome::NoData
    };

    let dns_payload = build_dns_reply(&q, &outcome);
    let frame = build_ipv4_udp_reply(src_ip, src_port, PORT_DNS, &dns_payload);
    let _ = outcome;
    Some(frame)
}

struct DnsQuery {
    id: u16,
    name: String,
    name_bytes: Vec<u8>,   // raw labels incl. terminating 0, for reply echo
    qtype: u16,
    qclass: u16,
}

fn parse_dns_query(dgram: &[u8]) -> Option<DnsQuery> {
    if dgram.len() < DNS_HDR_LEN { return None; }
    let id = u16::from_be_bytes([dgram[0], dgram[1]]);
    let qdcount = u16::from_be_bytes([dgram[4], dgram[5]]);
    if qdcount < 1 { return None; }

    let mut pos = DNS_HDR_LEN;
    let name_start = pos;
    let mut name = String::new();
    loop {
        if pos >= dgram.len() { return None; }
        let len = dgram[pos] as usize;
        if len == 0 { pos += 1; break; }
        if len & 0xC0 != 0 { return None; }   // pointer in query — unusual
        if pos + 1 + len > dgram.len() { return None; }
        if !name.is_empty() { name.push('.'); }
        for &b in &dgram[pos + 1..pos + 1 + len] {
            // Conservative: stringify printable ASCII only, else bail.
            if !(0x20..0x7F).contains(&b) { return None; }
            name.push(b as char);
        }
        pos += 1 + len;
    }
    let name_bytes = dgram[name_start..pos].to_vec();
    if pos + 4 > dgram.len() { return None; }
    let qtype  = u16::from_be_bytes([dgram[pos],     dgram[pos + 1]]);
    let qclass = u16::from_be_bytes([dgram[pos + 2], dgram[pos + 3]]);
    Some(DnsQuery { id, name, name_bytes, qtype, qclass })
}

fn build_dns_reply(q: &DnsQuery, out: &DnsOutcome) -> Vec<u8> {
    let (rcode, ancount): (u16, u16) = match out {
        DnsOutcome::Answer(_) => (0, 1),
        DnsOutcome::NoData    => (0, 0),
        DnsOutcome::NxDomain  => (3, 0),
    };
    let mut p = Vec::with_capacity(64);
    // Header
    p.extend_from_slice(&q.id.to_be_bytes());
    let flags: u16 = 0x8180 | rcode;                                // QR | RD | RA | rcode
    p.extend_from_slice(&flags.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes());                       // QDCOUNT
    p.extend_from_slice(&ancount.to_be_bytes());                    // ANCOUNT
    p.extend_from_slice(&0u16.to_be_bytes());                       // NSCOUNT
    p.extend_from_slice(&0u16.to_be_bytes());                       // ARCOUNT
    // Question (echo)
    p.extend_from_slice(&q.name_bytes);
    p.extend_from_slice(&q.qtype.to_be_bytes());
    p.extend_from_slice(&q.qclass.to_be_bytes());
    // Answer (only for an A hit)
    if let DnsOutcome::Answer(ip) = out {
        // NAME — compression pointer back to question (offset 12).
        p.push(0xC0); p.push(0x0C);
        p.extend_from_slice(&1u16.to_be_bytes());   // TYPE = A
        p.extend_from_slice(&1u16.to_be_bytes());   // CLASS = IN
        p.extend_from_slice(&60u32.to_be_bytes());  // TTL = 60s
        p.extend_from_slice(&4u16.to_be_bytes());   // RDLENGTH
        p.extend_from_slice(ip);
    }
    p
}

/// Build a full virtio-net frame (12-byte virtio hdr + eth + IPv4 + UDP
/// + payload) addressed `GATEWAY → GUEST` for source/dst ports given.
fn build_ipv4_udp_reply(
    dst_ip: [u8; 4],
    dst_port: u16,
    src_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let total_ip_len = IPV4_HDR_LEN + UDP_HDR_LEN + payload.len();
    let total = VNET_HDR_LEN + ETH_HDR_LEN + total_ip_len;
    let mut buf = alloc::vec![0u8; total];

    write_eth(&mut buf, &GUEST_MAC, &GATEWAY_MAC, ETHERTYPE_IPV4);

    let ip_off = VNET_HDR_LEN + ETH_HDR_LEN;
    buf[ip_off + 0]  = 0x45;                                    // version + IHL
    buf[ip_off + 1]  = 0;                                        // TOS
    buf[ip_off + 2..ip_off + 4].copy_from_slice(&(total_ip_len as u16).to_be_bytes());
    buf[ip_off + 4..ip_off + 6].copy_from_slice(&0u16.to_be_bytes()); // id
    buf[ip_off + 6..ip_off + 8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
    buf[ip_off + 8]  = 64;                                       // TTL
    buf[ip_off + 9]  = PROTO_UDP;
    // checksum = 0 placeholder
    buf[ip_off + 12..ip_off + 16].copy_from_slice(&GATEWAY_IP);
    buf[ip_off + 16..ip_off + 20].copy_from_slice(&dst_ip);
    let ip_csum = ipv4_checksum(&buf[ip_off..ip_off + IPV4_HDR_LEN]);
    buf[ip_off + 10..ip_off + 12].copy_from_slice(&ip_csum.to_be_bytes());

    let udp_off = ip_off + IPV4_HDR_LEN;
    let udp_len = (UDP_HDR_LEN + payload.len()) as u16;
    buf[udp_off + 0..udp_off + 2].copy_from_slice(&src_port.to_be_bytes());
    buf[udp_off + 2..udp_off + 4].copy_from_slice(&dst_port.to_be_bytes());
    buf[udp_off + 4..udp_off + 6].copy_from_slice(&udp_len.to_be_bytes());
    // checksum zero (allowed for UDP-over-IPv4)
    buf[udp_off + UDP_HDR_LEN..].copy_from_slice(payload);

    buf
}

fn write_eth(buf: &mut [u8], dst: &[u8; 6], src: &[u8; 6], ethertype: u16) {
    // virtio_net_hdr at offset 0..12. Per virtio 1.2 §5.1.6.4.1, with
    // VIRTIO_F_VERSION_1 negotiated and VIRTIO_NET_F_MRG_RXBUF NOT
    // negotiated, num_buffers (bytes 10..12, LE) MUST be 1 or Linux's
    // virtio_net driver drops the packet silently in receive_buf().
    // Everything else stays zero (no offloads, no GSO).
    buf[10] = 1;
    buf[11] = 0;
    let off = VNET_HDR_LEN;
    buf[off..off + 6].copy_from_slice(dst);
    buf[off + 6..off + 12].copy_from_slice(src);
    buf[off + 12..off + 14].copy_from_slice(&ethertype.to_be_bytes());
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < header.len() {
        sum += u16::from_be_bytes([header[i], header[i + 1]]) as u32;
        i += 2;
    }
    if i < header.len() {
        sum += (header[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn cap_reject(proto: &str, ip: [u8; 4], port: u16) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= 8 { return; }   // limit log spam
    kprintln!(
        "[nat] {} {}.{}.{}.{}:{} dropped (cap-reject)",
        proto, ip[0], ip[1], ip[2], ip[3], port,
    );
}

/// TCP checksum: pseudo-header (src_ip, dst_ip, zero, proto, tcp_len) +
/// the segment itself. Caller passes the same src/dst IPs that go in
/// the IPv4 header.
fn tcp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], tcp_segment: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    // pseudo-header
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += PROTO_TCP as u32;
    sum += tcp_segment.len() as u32;
    let mut i = 0;
    while i + 1 < tcp_segment.len() {
        sum += u16::from_be_bytes([tcp_segment[i], tcp_segment[i + 1]]) as u32;
        i += 2;
    }
    if i < tcp_segment.len() {
        sum += (tcp_segment[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Drain host-side TCP recv buffers for every active session and
/// inject the resulting TCP segments into the guest's RX queue. Called
/// from the timer-tick path so async response data reaches the guest
/// without it having to emit fresh traffic.
///
/// Returns `true` iff at least one segment was injected (caller fires
/// IRQ 10 to wake the guest's virtio-net driver).
pub fn pump(
    net: &mut super::virtio_net_pci::VirtioNet,
    mem: &GuestMem,
) -> bool {
    use core::sync::atomic::{AtomicU32, Ordering};
    static PUMP_LOG: AtomicU32 = AtomicU32::new(0);

    // CRITICAL: drain the host NIC RX ring. Intel I226-V is a polling
    // driver — nothing else calls handle_frame, so server replies (and
    // our l3_inbound intercept) only run because of this poll.
    crate::net::poll();

    let now = crate::interrupts::ticks();
    l3_reap(now);

    // Deliver every rewritten reply the host-RX intercept queued.
    let mut any = false;
    loop {
        let frame = { INBOUND_Q.lock().pop_front() };
        let Some(frame) = frame else { break };
        if net.inject_rx(mem, &frame) {
            any = true;
        } else {
            // Guest RX queue full — requeue and retry next pump.
            INBOUND_Q.lock().push_front(frame);
            break;
        }
    }
    let _ = PUMP_LOG;
    any
}

/// Number of currently-active (non-closed) TCP sessions. The run_linux
/// idle-detection uses this to extend the timeout when traffic is in
/// flight.
pub fn active_session_count() -> usize {
    // Drives VM idle-detection: keep the guest scheduled while any
    // masquerade flow is live or a reply is still queued.
    L3.lock().iter().flatten().count() + INBOUND_Q.lock().len()
}

/// Tear down NAT state when a microvm run ends so the next launch —
/// and the host's own networking — start clean.
pub fn reset_sessions() {
    l3_reset();
}
