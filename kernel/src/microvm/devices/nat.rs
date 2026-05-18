//! Synthetic NAT-gateway for the microvm's virtio-net device (12.3.3).
//!
//! The guest sees a `10.99.0.0/24` link with a single peer at
//! `10.99.0.1` (the synthetic gateway). All non-broadcast traffic the
//! guest emits goes to that peer. This module decides what to do with
//! each frame:
//!
//!   * ARP-Request for 10.99.0.1 → synth ARP-Reply (link-layer)
//!   * UDP to 10.99.0.1:53        → call host `net::dns::resolve`,
//!                                  return synth DNS-Reply
//!   * everything else            → cap-rejected for now (12.3.4 will
//!                                  flesh ICMP / UDP / TCP NAT out)
//!
//! Each handler returns a fully-built virtio-net frame (12-byte virtio
//! header + ethernet + IPv4/UDP/payload) ready to drop into the RX
//! queue. Higher layers in `virtio_net_pci.rs` walk the avail-ring,
//! write the frame into the next driver buffer, and signal the used
//! ring + IRQ.

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;
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

const PORT_DNS: u16 = 53;

const TCP_HDR_LEN: usize = 20;
const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;

// A real web page opens dozens of concurrent TCP connections (6+
// per origin × many origins: CDNs, fonts, analytics) plus HTTP
// keep-alive holds slots open. 4 was a bring-up smoke value — a
// browser instantly hit "session table full, dropping SYN" on
// anything past example.com. Each Option<TcpSession> is ~48 B →
// 128 slots ≈ 6 KB static, trivial. alloc_session already recycles
// None/Closed slots; idle-keep-alive reaping (LRU) is a later
// refinement if 128 ever saturates.
const MAX_TCP_SESSIONS: usize = 128;
/// Cap on per-segment data we inject to the guest. Keeps us well below
/// the typical 1460-byte MSS Linux advertises and avoids fragmenting
/// 1500-byte RX buffers.
const MAX_SEG_PAYLOAD: usize = 1400;

/// Max unacked guest-bound bytes per session (flow-control + rtx-buffer
/// cap). When full, pump stops draining the host conn so the host TCP
/// recv window backpressures the server. ~100 ticks/s (see tcp.rs).
const RTX_WINDOW: usize = 64 * 1024;
const RTO_TICKS: u64 = 50;        // ~500 ms retransmit timeout
const RTX_MAX_RETRIES: u8 = 8;    // ~4 s of no progress → reap

/// Per-VM network policy. Default is "DNS only" — outbound IP traffic
/// for anything else is logged + dropped until 12.3.4 adds a proper
/// NAT session table. Future work threads this through the microvm
/// session cap so different apps can have different policies.
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
    /// Default for 12.3.4 — DNS + TCP, but not raw UDP / ICMP. Plenty
    /// to let a browser-style stack reach HTTP/HTTPS endpoints; raw
    /// pings + UDP services still need an explicit cap.
    pub const fn dns_tcp() -> Self {
        Self { allow_dns: true, allow_icmp: false, allow_udp: false, allow_tcp: true }
    }
}

impl Default for NetCaps {
    fn default() -> Self { Self::dns_tcp() }
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
    let l4 = &ip[ihl..];

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
            } else {
                if !caps.allow_udp {
                    cap_reject("UDP", dst_ip, dst_port);
                }
                None
            }
        }
        PROTO_TCP => {
            if !caps.allow_tcp {
                let dst_port = if l4.len() >= 4 {
                    u16::from_be_bytes([l4[2], l4[3]])
                } else { 0 };
                cap_reject("TCP", dst_ip, dst_port);
                return None;
            }
            handle_tcp(frame, caps)
        }
        PROTO_ICMP => {
            if !caps.allow_icmp {
                cap_reject("ICMP", dst_ip, 0);
            }
            None
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

    kprintln!("[nat] DNS query: \"{}\" (type={} id={:#06x})", q.name, q.qtype, q.id);

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

    match &outcome {
        DnsOutcome::Answer(ip) => kprintln!("[nat] DNS reply: {} → {}.{}.{}.{}", q.name, ip[0], ip[1], ip[2], ip[3]),
        DnsOutcome::NoData     => kprintln!("[nat] DNS reply: {} → NODATA (type={})", q.name, q.qtype),
        DnsOutcome::NxDomain   => kprintln!("[nat] DNS reply: {} → NXDOMAIN", q.name),
    }
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

// ───────────────────────────── TCP NAT ────────────────────────────────
//
// Termination-style NAT: we emulate a full TCP endpoint on the guest-
// facing side (10.99.0.1:<target_port>) and forward the data stream
// onto a fresh host-side TCP connection opened via `net::tcp`. State
// per session lives in `SESSIONS`. The lifecycle in 5 events:
//
//   1. Guest sends SYN → handle_tcp_syn opens host connection
//      (blocking up to ~10 s — VM is paused anyway), synth SYN+ACK back.
//   2. Guest ACKs → state = Established.
//   3. Guest sends data segment → handle_tcp_data forwards to host
//      via `tcp::send`, synth ACK back.
//   4. `pump` (called from timer-tick) drains host `tcp::recv`, slices
//      into ≤ MAX_SEG_PAYLOAD chunks, injects as TCP-PSH-ACK segments.
//   5. Guest FIN or host close → bidirectional teardown via
//      handle_tcp_fin / pump's host-close detection.
//
// Sequence numbers: we pick a fresh ISN (via host CSPRNG) when sending
// SYN+ACK; we honour the guest's ISN+1 as `rcv_nxt`. From then on we
// keep both pointers up to date as data flows.

#[derive(Clone, Copy, PartialEq, Debug)]
enum TcpState {
    SynRcvd,        // SYN+ACK sent, awaiting guest ACK
    Established,
    HostClosed,     // host FIN/closed, we still need to FIN the guest
    FinWait,        // guest FIN seen, host close issued
    Closed,
}

struct TcpSession {
    guest_port:  u16,
    target_ip:   [u8; 4],
    target_port: u16,
    host_handle: usize,

    state: TcpState,
    snd_nxt: u32,   // next seq we send to guest
    snd_una: u32,   // unacked seq on our side
    rcv_nxt: u32,   // next seq we expect from guest

    /// Last window size guest advertised. We honour it on outgoing
    /// data segments by capping the unacked-bytes-in-flight.
    guest_window: u16,

    /// Tick of last activity (creation / guest segment / pump data).
    /// Drives reaping of sessions the guest abandoned before ACKing
    /// our SYN+ACK — otherwise they pin a slot + host handle forever.
    last_tick: u64,

    /// Unacked bytes already injected to the guest, covering seq range
    /// [snd_una, snd_nxt). We are the TCP sender toward the guest;
    /// without this a single dropped RX segment (guest virtio queue
    /// pressure / Linux drop) is never resent → the stream wedges →
    /// PR_IO_TIMEOUT_ERROR. Bounded by RTX_WINDOW.
    rtx: alloc::vec::Vec<u8>,
    /// Tick the oldest unacked byte was last (re)sent. RTO measured
    /// from here.
    rtx_tick: u64,
    /// Consecutive RTO retransmits with no progress; session is reaped
    /// past RTX_MAX_RETRIES.
    rtx_retries: u8,
}

static SESSIONS: Mutex<[Option<TcpSession>; MAX_TCP_SESSIONS]> = Mutex::new(
    [const { None }; MAX_TCP_SESSIONS]
);

fn find_session(sessions: &mut [Option<TcpSession>; MAX_TCP_SESSIONS], port: u16)
    -> Option<&mut TcpSession>
{
    for s in sessions.iter_mut() {
        if let Some(sess) = s.as_mut() {
            if sess.guest_port == port && sess.state != TcpState::Closed {
                return Some(sess);
            }
        }
    }
    None
}

fn alloc_session(sessions: &mut [Option<TcpSession>; MAX_TCP_SESSIONS]) -> Option<usize> {
    sessions.iter().position(|s| s.is_none() || matches!(s, Some(t) if t.state == TcpState::Closed))
}

/// Dispatch an inbound TCP segment from the guest. Returns a single
/// synthetic reply frame to push back via RX (already wrapped in
/// virtio-net + eth + IPv4 + TCP). `None` means no immediate reply
/// needed — async response data arrives later via `pump`.
/// Process a guest cumulative ACK: drop the acked prefix from the
/// retransmit buffer and advance `snd_una`. Ignores dup / stale /
/// out-of-window acks so a spurious number can't rewind the sender.
fn ack_progress(sess: &mut TcpSession, ack: u32) {
    let outstanding = sess.snd_nxt.wrapping_sub(sess.snd_una);
    let acked = ack.wrapping_sub(sess.snd_una);
    if acked == 0 || acked > outstanding { return; }
    let drain = (acked as usize).min(sess.rtx.len());
    sess.rtx.drain(..drain);
    sess.snd_una = ack;
    sess.rtx_retries = 0;
    sess.rtx_tick = if sess.rtx.is_empty() { 0 } else { crate::interrupts::ticks() };
}

fn handle_tcp(frame: &[u8], _caps: &NetCaps) -> Option<Vec<u8>> {
    if frame.len() < ETH_HDR_LEN + IPV4_HDR_LEN + TCP_HDR_LEN { return None; }
    let ip = &frame[ETH_HDR_LEN..];
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN { return None; }
    let src_ip: [u8; 4] = ip[12..16].try_into().ok()?;
    let dst_ip: [u8; 4] = ip[16..20].try_into().ok()?;
    let _ = src_ip;
    let tcp = &ip[ihl..];
    if tcp.len() < TCP_HDR_LEN { return None; }

    let src_port = u16::from_be_bytes([tcp[0], tcp[1]]);
    let dst_port = u16::from_be_bytes([tcp[2], tcp[3]]);
    let seq      = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
    let ack      = u32::from_be_bytes([tcp[8], tcp[9], tcp[10], tcp[11]]);
    let data_off = ((tcp[12] >> 4) as usize) * 4;
    let flags    = tcp[13];
    let window   = u16::from_be_bytes([tcp[14], tcp[15]]);
    if data_off < TCP_HDR_LEN || data_off > tcp.len() { return None; }
    let payload  = &tcp[data_off..];

    let mut sessions = SESSIONS.lock();

    // SYN → new session
    if flags & TCP_SYN != 0 && flags & TCP_ACK == 0 {
        return tcp_handle_syn(&mut sessions, src_port, dst_ip, dst_port, seq, window);
    }

    // Otherwise, look up by guest src port.
    let sess = match find_session(&mut sessions, src_port) {
        Some(s) => s,
        None => {
            // Unknown session — send RST back to the guest so it gives up.
            kprintln!("[nat] TCP segment for unknown session src_port={} flags={:#04x}",
                      src_port, flags);
            return Some(build_tcp_rst(src_port, dst_port, dst_ip, ack, seq));
        }
    };
    sess.guest_window = window;
    sess.last_tick = crate::interrupts::ticks();

    // RST from guest → tear down host side, kill session.
    if flags & TCP_RST != 0 {
        let _ = crate::net::tcp::close(sess.host_handle);
        sess.state = TcpState::Closed;
        return None;
    }

    // ACK during SynRcvd completes our handshake.
    if sess.state == TcpState::SynRcvd && flags & TCP_ACK != 0 {
        if ack == sess.snd_nxt {
            sess.snd_una = ack;
            sess.state = TcpState::Established;
            kprintln!("[nat] TCP {}→{}.{}.{}.{}:{} established",
                      sess.guest_port,
                      sess.target_ip[0], sess.target_ip[1],
                      sess.target_ip[2], sess.target_ip[3],
                      sess.target_port);
        }
    }

    // Any ACK in Established (data-piggybacked or bare) advances our
    // retransmit buffer. Must run before the data branch returns.
    if sess.state == TcpState::Established && flags & TCP_ACK != 0 {
        ack_progress(sess, ack);
    }

    // Established (or in-flight close) — accept new payload.
    if !payload.is_empty() && sess.state == TcpState::Established {
        if seq == sess.rcv_nxt {
            // Forward to host
            if let Err(e) = crate::net::tcp::send(sess.host_handle, payload) {
                kprintln!("[nat] TCP host send failed: {:?}", e);
            }
            sess.rcv_nxt = sess.rcv_nxt.wrapping_add(payload.len() as u32);
            // ACK back
            let reply = build_tcp_segment(sess, &[], TCP_ACK);
            return Some(reply);
        }
        // Out-of-order — drop; guest will retransmit.
        return None;
    }

    // FIN handling — guest wants to half-close.
    if flags & TCP_FIN != 0 {
        sess.rcv_nxt = sess.rcv_nxt.wrapping_add(1);
        let _ = crate::net::tcp::close(sess.host_handle);
        // FIN+ACK back, then mark closed. We don't track the guest's
        // subsequent ACK of our FIN — if it retransmits we'll RST it
        // (find_session returns None for Closed slots).
        let reply = build_tcp_segment(sess, &[], TCP_FIN | TCP_ACK);
        sess.snd_nxt = sess.snd_nxt.wrapping_add(1);
        sess.state = TcpState::Closed;
        kprintln!("[nat] TCP {}→… guest FIN, sent FIN+ACK", sess.guest_port);
        return Some(reply);
    }

    // Bare ACK already handled by ack_progress() above.
    None
}

fn tcp_handle_syn(
    sessions: &mut [Option<TcpSession>; MAX_TCP_SESSIONS],
    src_port: u16,
    target_ip: [u8; 4],
    target_port: u16,
    guest_isn: u32,
    guest_window: u16,
) -> Option<Vec<u8>> {
    // If there's an existing session for this port (retransmitted SYN
    // or stale), re-use the slot.
    let slot = match alloc_session(sessions) {
        Some(s) => s,
        None => {
            kprintln!("[nat] TCP session table full, dropping SYN");
            return None;
        }
    };

    kprintln!("[nat] TCP SYN: guest:{} → {}.{}.{}.{}:{}",
              src_port,
              target_ip[0], target_ip[1], target_ip[2], target_ip[3],
              target_port);

    // Blocking host connect. The VM is paused at this exit so a 100-
    // 500ms handshake won't trigger any Linux-side retransmit (guest
    // clock is paused too). On failure, send RST back so the guest
    // gives up immediately.
    let host_handle = match crate::net::tcp::connect(target_ip, target_port) {
        Ok(h) => h,
        Err(e) => {
            kprintln!("[nat] TCP host connect failed: {:?}", e);
            return Some(build_tcp_rst(src_port, target_port, target_ip,
                                       0, guest_isn.wrapping_add(1)));
        }
    };

    // Our ISN — derive from CSPRNG so it isn't predictable.
    let our_isn = {
        let bytes = crate::csprng::random_256();
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    };

    let sess = TcpSession {
        guest_port: src_port,
        target_ip,
        target_port,
        host_handle,
        state: TcpState::SynRcvd,
        snd_nxt: our_isn.wrapping_add(1),   // SYN consumes 1 seq
        snd_una: our_isn,
        rcv_nxt: guest_isn.wrapping_add(1),
        guest_window,
        last_tick: crate::interrupts::ticks(),
        rtx: alloc::vec::Vec::new(),
        rtx_tick: 0,
        rtx_retries: 0,
    };

    // SYN+ACK with MSS option (kind=2 len=4 mss=1400). Linux honours
    // this to cap segments it sends to us.
    let reply = build_tcp_synack_with_mss(&sess);
    sessions[slot] = Some(sess);
    Some(reply)
}

/// Build any TCP segment (ACK / FIN+ACK / PSH+ACK / etc.) for this
/// session, optionally carrying `payload`. Sequence numbers come from
/// the current session state. The packet is constructed to look like
/// it came FROM `target_ip:target_port` (the NAT-impersonated remote
/// server) TO `GUEST_IP:guest_port`, so the guest's TCP stack accepts
/// it as a continuation of its connection.
fn build_tcp_segment(sess: &TcpSession, payload: &[u8], flags: u8) -> Vec<u8> {
    build_tcp_segment_at(sess, sess.snd_nxt, payload, flags)
}

/// As `build_tcp_segment` but with an explicit sequence number — used
/// by the retransmit path, which resends from `snd_una`, not `snd_nxt`.
fn build_tcp_segment_at(sess: &TcpSession, seq: u32, payload: &[u8], flags: u8) -> Vec<u8> {
    let total_ip = IPV4_HDR_LEN + TCP_HDR_LEN + payload.len();
    let total = VNET_HDR_LEN + ETH_HDR_LEN + total_ip;
    let mut buf = alloc::vec![0u8; total];
    write_eth(&mut buf, &GUEST_MAC, &GATEWAY_MAC, ETHERTYPE_IPV4);
    fill_ipv4_full(&mut buf, sess.target_ip, GUEST_IP, total_ip, PROTO_TCP);

    let tcp_off = VNET_HDR_LEN + ETH_HDR_LEN + IPV4_HDR_LEN;
    buf[tcp_off + 0..tcp_off + 2].copy_from_slice(&sess.target_port.to_be_bytes());
    buf[tcp_off + 2..tcp_off + 4].copy_from_slice(&sess.guest_port.to_be_bytes());
    buf[tcp_off + 4..tcp_off + 8].copy_from_slice(&seq.to_be_bytes());
    buf[tcp_off + 8..tcp_off + 12].copy_from_slice(&sess.rcv_nxt.to_be_bytes());
    buf[tcp_off + 12] = ((TCP_HDR_LEN / 4) as u8) << 4;
    buf[tcp_off + 13] = flags;
    buf[tcp_off + 14..tcp_off + 16].copy_from_slice(&65535u16.to_be_bytes());
    if !payload.is_empty() {
        buf[tcp_off + TCP_HDR_LEN..tcp_off + TCP_HDR_LEN + payload.len()]
            .copy_from_slice(payload);
    }
    let cksum = tcp_checksum(sess.target_ip, GUEST_IP,
                             &buf[tcp_off..tcp_off + TCP_HDR_LEN + payload.len()]);
    buf[tcp_off + 16..tcp_off + 18].copy_from_slice(&cksum.to_be_bytes());
    buf
}

/// SYN+ACK with a single MSS option (kind=2 len=4 mss=1400). 24-byte
/// TCP header. Linux honours this to cap segments it sends to us.
fn build_tcp_synack_with_mss(sess: &TcpSession) -> Vec<u8> {
    const HDR_WITH_MSS: usize = 24;
    let total_ip = IPV4_HDR_LEN + HDR_WITH_MSS;
    let total = VNET_HDR_LEN + ETH_HDR_LEN + total_ip;
    let mut buf = alloc::vec![0u8; total];
    write_eth(&mut buf, &GUEST_MAC, &GATEWAY_MAC, ETHERTYPE_IPV4);
    fill_ipv4_full(&mut buf, sess.target_ip, GUEST_IP, total_ip, PROTO_TCP);

    let tcp_off = VNET_HDR_LEN + ETH_HDR_LEN + IPV4_HDR_LEN;
    buf[tcp_off + 0..tcp_off + 2].copy_from_slice(&sess.target_port.to_be_bytes());
    buf[tcp_off + 2..tcp_off + 4].copy_from_slice(&sess.guest_port.to_be_bytes());
    // SYN+ACK ISN = our snd_una (snd_nxt-1 since SYN consumes 1 byte)
    buf[tcp_off + 4..tcp_off + 8].copy_from_slice(&sess.snd_una.to_be_bytes());
    buf[tcp_off + 8..tcp_off + 12].copy_from_slice(&sess.rcv_nxt.to_be_bytes());
    buf[tcp_off + 12] = ((HDR_WITH_MSS / 4) as u8) << 4;
    buf[tcp_off + 13] = TCP_SYN | TCP_ACK;
    buf[tcp_off + 14..tcp_off + 16].copy_from_slice(&65535u16.to_be_bytes());
    buf[tcp_off + 20] = 2;
    buf[tcp_off + 21] = 4;
    buf[tcp_off + 22..tcp_off + 24].copy_from_slice(&(MAX_SEG_PAYLOAD as u16).to_be_bytes());
    let cksum = tcp_checksum(sess.target_ip, GUEST_IP,
                             &buf[tcp_off..tcp_off + HDR_WITH_MSS]);
    buf[tcp_off + 16..tcp_off + 18].copy_from_slice(&cksum.to_be_bytes());
    buf
}

/// Standalone RST for unknown sessions — no SESSIONS lookup.
fn build_tcp_rst(
    guest_port: u16,
    target_port: u16,
    target_ip: [u8; 4],
    seq: u32,
    ack: u32,
) -> Vec<u8> {
    let total_ip = IPV4_HDR_LEN + TCP_HDR_LEN;
    let total = VNET_HDR_LEN + ETH_HDR_LEN + total_ip;
    let mut buf = alloc::vec![0u8; total];
    write_eth(&mut buf, &GUEST_MAC, &GATEWAY_MAC, ETHERTYPE_IPV4);
    fill_ipv4_full(&mut buf, target_ip, GUEST_IP, total_ip, PROTO_TCP);
    let tcp_off = VNET_HDR_LEN + ETH_HDR_LEN + IPV4_HDR_LEN;
    buf[tcp_off + 0..tcp_off + 2].copy_from_slice(&target_port.to_be_bytes());
    buf[tcp_off + 2..tcp_off + 4].copy_from_slice(&guest_port.to_be_bytes());
    buf[tcp_off + 4..tcp_off + 8].copy_from_slice(&seq.to_be_bytes());
    buf[tcp_off + 8..tcp_off + 12].copy_from_slice(&ack.to_be_bytes());
    buf[tcp_off + 12] = ((TCP_HDR_LEN / 4) as u8) << 4;
    buf[tcp_off + 13] = TCP_RST | TCP_ACK;
    let cksum = tcp_checksum(target_ip, GUEST_IP, &buf[tcp_off..tcp_off + TCP_HDR_LEN]);
    buf[tcp_off + 16..tcp_off + 18].copy_from_slice(&cksum.to_be_bytes());
    buf
}

/// Fill the IPv4 header with explicit src + dst. Used by TCP NAT
/// where src is the impersonated remote server, not our gateway.
fn fill_ipv4_full(buf: &mut [u8], src_ip: [u8; 4], dst_ip: [u8; 4],
                  total_ip: usize, proto: u8) {
    let off = VNET_HDR_LEN + ETH_HDR_LEN;
    buf[off + 0]  = 0x45;
    buf[off + 1]  = 0;
    buf[off + 2..off + 4].copy_from_slice(&(total_ip as u16).to_be_bytes());
    buf[off + 4..off + 6].copy_from_slice(&0u16.to_be_bytes());
    buf[off + 6..off + 8].copy_from_slice(&0x4000u16.to_be_bytes());
    buf[off + 8]  = 64;
    buf[off + 9]  = proto;
    buf[off + 12..off + 16].copy_from_slice(&src_ip);
    buf[off + 16..off + 20].copy_from_slice(&dst_ip);
    let csum = ipv4_checksum(&buf[off..off + IPV4_HDR_LEN]);
    buf[off + 10..off + 12].copy_from_slice(&csum.to_be_bytes());
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
    host_base: u64,
) -> bool {
    use core::sync::atomic::{AtomicU32, Ordering};
    // Per-chunk pump logging is bring-up diagnostics. With real
    // traffic (a LibreWolf page load = hundreds of 1400-byte chunks)
    // it floods the serial/loop terminal and the kmsg VM-exit cost
    // slows the guest. Log the first few to confirm connectivity,
    // then go silent — same throttle idiom as the virtio-gpu logs.
    static PUMP_LOG: AtomicU32 = AtomicU32::new(0);

    // CRITICAL: drain the host NIC's RX ring. Intel I226-V is a
    // polling driver — no IRQ path calls handle_frame for us. Without
    // this, response packets from the remote server pile up in the
    // NIC's DMA ring while the VM exits keep ticking. Symptom would be
    // `buffered=0` heartbeats forever even though the server replied.
    crate::net::poll();

    // Step 1: snapshot lightweight per-session info under the SESSIONS
    // lock, then drop it. This avoids holding two locks (SESSIONS +
    // host-tcp CONNECTIONS) at the same time — a NIC IRQ that takes
    // CONNECTIONS for incoming-packet dispatch would otherwise deadlock
    // against pump.
    struct Snapshot {
        slot: usize,
        host_handle: usize,
        state: TcpState,
        rtx_len: usize,
    }
    // Reap sweep: a session the guest abandoned before ACKing our
    // SYN+ACK sticks in SynRcvd forever, pinning a NAT slot AND a host
    // TCP slot. Free both after a grace period. Closed sessions also
    // get their host handle closed so the host's 128 slots churn
    // cleanly. ~100 ticks/s (see tcp.rs TimeWait), 5 s grace.
    {
        const SYN_REAP_TICKS: u64 = 500;
        let now = crate::interrupts::ticks();
        let mut sessions = SESSIONS.lock();
        for s in sessions.iter_mut() {
            let drop_it = match s.as_ref() {
                Some(sess) if sess.state == TcpState::SynRcvd
                    && now.wrapping_sub(sess.last_tick) > SYN_REAP_TICKS => true,
                Some(sess) if sess.state == TcpState::Closed => true,
                _ => false,
            };
            if drop_it {
                if let Some(sess) = s.as_ref() {
                    let _ = crate::net::tcp::close(sess.host_handle);
                }
                *s = None;
            }
        }
    }

    let snapshots: alloc::vec::Vec<Snapshot> = {
        let sessions = SESSIONS.lock();
        sessions.iter().enumerate().filter_map(|(i, s)| {
            s.as_ref().filter(|sess| sess.state != TcpState::Closed)
                .map(|sess| Snapshot {
                    slot: i,
                    host_handle: sess.host_handle,
                    state: sess.state,
                    rtx_len: sess.rtx.len(),
                })
        }).collect()
    };
    if snapshots.is_empty() { return false; }

    let mut any = false;
    for snap in &snapshots {
        let now = crate::interrupts::ticks();

        // --- Retransmit pass (first). A guest-bound segment the guest
        // never ACKed (dropped in its virtio RX queue / by Linux) must
        // be resent or the stream wedges. One action per session per
        // pump; new host data waits until the backlog clears. ---
        if snap.rtx_len > 0 {
            let mut sessions = SESSIONS.lock();
            let Some(sess) = sessions[snap.slot].as_mut() else { continue };
            if sess.state == TcpState::Closed { continue }
            if !sess.rtx.is_empty() && sess.rtx_tick != 0
                && now.wrapping_sub(sess.rtx_tick) > RTO_TICKS
            {
                if sess.rtx_retries >= RTX_MAX_RETRIES {
                    // Peer (guest) unreachable for the data — give up.
                    let fin = build_tcp_segment_at(sess, sess.snd_una, &[],
                                                   TCP_FIN | TCP_ACK);
                    let _ = crate::net::tcp::close(sess.host_handle);
                    sess.state = TcpState::Closed;
                    drop(sessions);
                    if net.inject_rx(host_base, &fin) { any = true; }
                    kprintln!("[nat] TCP {} rtx gave up after {} retries",
                              snap.host_handle, RTX_MAX_RETRIES);
                    continue;
                }
                let clen = sess.rtx.len().min(MAX_SEG_PAYLOAD);
                let seg = build_tcp_segment_at(sess, sess.snd_una,
                                               &sess.rtx[..clen],
                                               TCP_PSH | TCP_ACK);
                sess.rtx_retries += 1;
                sess.rtx_tick = now;
                drop(sessions);
                if net.inject_rx(host_base, &seg) { any = true; }
                continue;   // one action per session per pump
            }
        }

        // --- New host data. Only drain the host conn if the unacked
        // window has room — otherwise leave it in the host recv buffer
        // so the host TCP window backpressures the server (no loss). ---
        let want_recv = snap.rtx_len < RTX_WINDOW;
        let mut buf = alloc::vec![0u8; MAX_SEG_PAYLOAD];
        let recv_n = if want_recv {
            crate::net::tcp::recv(snap.host_handle, &mut buf).ok()
        } else {
            None
        };
        let host_alive = crate::net::tcp::is_established(snap.host_handle);

        let mut sessions = SESSIONS.lock();
        let Some(sess) = sessions[snap.slot].as_mut() else { continue };
        if sess.state == TcpState::Closed { continue }

        match recv_n {
            Some(n) if n > 0 => {
                let lg = PUMP_LOG.fetch_add(1, Ordering::Relaxed);
                if lg < 8 {
                    kprintln!("[nat] pump: host->guest {} bytes (seq={} ack={})",
                              n, sess.snd_nxt, sess.rcv_nxt);
                }
                let seg = build_tcp_segment(sess, &buf[..n], TCP_PSH | TCP_ACK);
                // Buffer for retransmit BEFORE injecting — if inject
                // fails the bytes are still recoverable via the rtx
                // path (old code dropped them on a full RX queue).
                sess.rtx.extend_from_slice(&buf[..n]);
                if sess.rtx_tick == 0 { sess.rtx_tick = now; }
                sess.snd_nxt = sess.snd_nxt.wrapping_add(n as u32);
                drop(sessions);
                if net.inject_rx(host_base, &seg) {
                    if lg < 8 {
                        kprintln!("[nat] pump: rx injected ({} bytes, total frame {})",
                                  n, seg.len());
                    }
                    any = true;
                } else {
                    kprintln!("[nat] pump: inject_rx FAILED — rtx will resend");
                }
            }
            _ => {
                // Host closed: only FIN the guest once we've delivered
                // (and it ACKed) everything — don't truncate the body.
                if !host_alive && sess.state == TcpState::Established
                    && sess.rtx.is_empty()
                {
                    let fin = build_tcp_segment(sess, &[], TCP_FIN | TCP_ACK);
                    sess.snd_nxt = sess.snd_nxt.wrapping_add(1);
                    sess.state = TcpState::Closed;
                    let _ = crate::net::tcp::close(sess.host_handle);
                    drop(sessions);
                    if net.inject_rx(host_base, &fin) { any = true; }
                    kprintln!("[nat] TCP {} host closed, FIN injected",
                              snap.host_handle);
                }
            }
        }
    }
    any
}

/// Number of currently-active (non-closed) TCP sessions. The run_linux
/// idle-detection uses this to extend the timeout when traffic is in
/// flight.
pub fn active_session_count() -> usize {
    let sessions = SESSIONS.lock();
    sessions.iter()
        .filter(|s| matches!(s, Some(t) if t.state != TcpState::Closed))
        .count()
}

/// Tear down every session and free the table. Called when a microvm
/// run ends so the next launch starts clean.
pub fn reset_sessions() {
    let mut sessions = SESSIONS.lock();
    for slot in sessions.iter_mut() {
        if let Some(sess) = slot.take() {
            let _ = crate::net::tcp::close(sess.host_handle);
        }
    }
}
