//! UDP — User Datagram Protocol
//!
//! Stateless, unreliable transport. Foundation for DNS and DHCP.

use alloc::vec::Vec;
use spin::Mutex;
use super::ipv4;

const HEADER_LEN: usize = 8;

/// Registered UDP listeners: (port, callback buffer)
/// Simple model: one listener per port, incoming data buffered.
const MAX_LISTENERS: usize = 8;
const MAX_RECV_BUF: usize = 2048;

struct UdpListener {
    port: u16,
    buf: Vec<u8>,
    src_ip: [u8; 4],
    src_port: u16,
    has_data: bool,
    /// A hard ICMP error for a datagram it sent: (to address, to port).
    err: Option<([u8; 4], u16)>,
}

static LISTENERS: Mutex<[Option<UdpListener>; MAX_LISTENERS]> = Mutex::new(
    [const { None }; MAX_LISTENERS]
);

pub fn handle_udp(ip_packet: &[u8], data: &[u8]) {
    if data.len() < HEADER_LEN { return; }

    let src_port = u16::from_be_bytes([data[0], data[1]]);
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let length = u16::from_be_bytes([data[4], data[5]]) as usize;

    if length < HEADER_LEN || length > data.len() { return; }
    let payload = &data[HEADER_LEN..length.min(data.len())];
    let src_ip = <[u8; 4]>::try_from(&ip_packet[12..16]).unwrap();
    RX_TOTAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    if dst_port == 68 && super::dhcp::deliver(payload, ip_packet[16..20] == super::arp::our_ip()) {
        return;
    }

    // Deliver to registered listener
    let mut listeners = LISTENERS.lock();
    for slot in listeners.iter_mut().flatten() {
        if slot.port == dst_port {
            let copy_len = payload.len().min(MAX_RECV_BUF);
            slot.buf.clear();
            slot.buf.extend_from_slice(&payload[..copy_len]);
            slot.src_ip = src_ip;
            slot.src_port = src_port;
            slot.has_data = true;
            return;
        }
    }
    drop(listeners);
    // Nobody was home: a reply that arrives one microsecond after its waiter
    // gave up, or on a port whose listener was never registered. Its sender
    // learns so, as from `__udp4_lib_rcv`, if it was sent to our address.
    NO_LISTENER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    NO_LISTENER_PORT.store(dst_port, core::sync::atomic::Ordering::Relaxed);
    if ip_packet[16..20] == super::arp::our_ip() {
        let total = u16::from_be_bytes([ip_packet[2], ip_packet[3]]) as usize;
        ipv4::icmp_send(&ip_packet[..total.min(ip_packet.len())], net_core::ip::ICMP_DEST_UNREACH,
            net_core::ip::ICMP_PORT_UNREACH, 0);
    }
}

/// `__udp4_lib_err`: a hard error (`icmp_err_convert` fatal, or a
/// parameter problem) for a datagram one of our listeners sent. Our
/// clients send to one server and wait for it, as a connected socket does,
/// so it is theirs to see.
pub fn icmp_error(e: &net_core::ip::IcmpError) {
    use net_core::ip::{ICMP_DEST_UNREACH, ICMP_PARAMETERPROB};
    let hard = match e.kind {
        ICMP_DEST_UNREACH => matches!(e.code, 2 | 3 | 6..=10 | 13..=15),
        ICMP_PARAMETERPROB => true,
        _ => false,
    };
    if !hard { return; }
    let t = e.transport();
    let sport = u16::from_be_bytes([t[0], t[1]]);
    let dport = u16::from_be_bytes([t[2], t[3]]);
    let mut listeners = LISTENERS.lock();
    if let Some(l) = listeners.iter_mut().flatten().find(|l| l.port == sport) {
        l.err = Some((e.inner_dst(), dport));
    }
}

/// The hard ICMP error a listener's datagram drew, once.
pub fn take_error(port: u16) -> Option<([u8; 4], u16)> {
    LISTENERS.lock().iter_mut().flatten().find(|l| l.port == port).and_then(|l| l.err.take())
}

/// Datagrams dropped because no listener was registered, and the last such
/// port. Read by the DNS failure path: "nothing arrived" and "it arrived and we
/// had already stopped listening" are different faults.
pub fn no_listener_stats() -> (u32, u16) {
    (NO_LISTENER.load(core::sync::atomic::Ordering::Relaxed),
     NO_LISTENER_PORT.load(core::sync::atomic::Ordering::Relaxed))
}

/// Every UDP datagram that reached this layer, whoever it was for. Cumulative;
/// callers take a snapshot and report the delta, since a total since boot
/// says nothing about the lookup that just failed.
pub fn rx_total() -> u32 {
    RX_TOTAL.load(core::sync::atomic::Ordering::Relaxed)
}

static RX_TOTAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

static NO_LISTENER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static NO_LISTENER_PORT: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// UDP checksum over the IPv4 pseudo-header + datagram (RFC 768). A computed
/// zero goes on the wire as 0xFFFF, because zero is the "no checksum" escape.
pub fn checksum(src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) -> u16 {
    use super::csum;
    let c = !csum::fold(csum::pseudo(src_ip, dst_ip, ipv4::PROTO_UDP, udp.len()) + csum::sum(udp));
    if c == 0 { 0xFFFF } else { c }
}

/// Send a UDP datagram
pub fn send(dst_ip: [u8; 4], src_port: u16, dst_port: u16, payload: &[u8]) {
    send_from(super::arp::our_ip(), dst_ip, src_port, dst_port, payload);
}

/// Send a UDP datagram from `src_ip`.
pub fn send_from(src_ip: [u8; 4], dst_ip: [u8; 4], src_port: u16, dst_port: u16, payload: &[u8]) {
    let udp_len = (HEADER_LEN + payload.len()) as u16;
    let mut pkt = alloc::vec![0u8; udp_len as usize];

    pkt[0..2].copy_from_slice(&src_port.to_be_bytes());
    pkt[2..4].copy_from_slice(&dst_port.to_be_bytes());
    pkt[4..6].copy_from_slice(&udp_len.to_be_bytes());
    pkt[HEADER_LEN..].copy_from_slice(payload);
    // RFC 1122 4.1.3.4: checksumming defaults to on (Linux `udp_send_skb`).
    let c = checksum(src_ip, dst_ip, &pkt);
    pkt[6..8].copy_from_slice(&c.to_be_bytes());

    let _ = ipv4::send_from(src_ip, dst_ip, ipv4::PROTO_UDP, &pkt);
}

/// Register a listener on a UDP port. Returns false if no slot available.
#[allow(dead_code)]
pub fn listen(port: u16) -> bool {
    let mut listeners = LISTENERS.lock();
    // Already listening?
    if listeners.iter().flatten().any(|l| l.port == port) { return true; }
    if let Some(slot) = listeners.iter_mut().find(|s| s.is_none()) {
        *slot = Some(UdpListener {
            port,
            buf: Vec::with_capacity(MAX_RECV_BUF),
            src_ip: [0; 4],
            src_port: 0,
            has_data: false,
            err: None,
        });
        true
    } else {
        false
    }
}

/// Check if data is available on a port. Returns (src_ip, src_port, data) or None.
pub fn recv(port: u16) -> Option<([u8; 4], u16, Vec<u8>)> {
    let mut listeners = LISTENERS.lock();
    for slot in listeners.iter_mut().flatten() {
        if slot.port == port && slot.has_data {
            slot.has_data = false;
            let data = slot.buf.clone();
            return Some((slot.src_ip, slot.src_port, data));
        }
    }
    None
}

/// Listen on a random free port in the ephemeral range (RFC 6056), for a
/// client that waits for one reply. The port is the reply's first secret:
/// a fixed one lets anyone aim forged answers at it. None if every try is
/// taken or the table is full.
pub fn listen_ephemeral() -> Option<u16> {
    let mut listeners = LISTENERS.lock();
    for _ in 0..16 {
        let port = 49152 + (crate::csprng::random_u64() % 16384) as u16;
        if listeners.iter().flatten().any(|l| l.port == port) { continue; }
        let slot = listeners.iter_mut().find(|s| s.is_none())?;
        *slot = Some(UdpListener {
            port,
            buf: Vec::with_capacity(MAX_RECV_BUF),
            src_ip: [0; 4],
            src_port: 0,
            has_data: false,
            err: None,
        });
        return Some(port);
    }
    None
}

/// Stop listening on a port.
pub fn unlisten(port: u16) {
    let mut listeners = LISTENERS.lock();
    for slot in listeners.iter_mut() {
        if slot.as_ref().map_or(false, |l| l.port == port) {
            *slot = None;
        }
    }
}
