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
}

/// Send a UDP datagram
pub fn send(dst_ip: [u8; 4], src_port: u16, dst_port: u16, payload: &[u8]) {
    let udp_len = (HEADER_LEN + payload.len()) as u16;
    let mut pkt = alloc::vec![0u8; udp_len as usize];

    pkt[0..2].copy_from_slice(&src_port.to_be_bytes());
    pkt[2..4].copy_from_slice(&dst_port.to_be_bytes());
    pkt[4..6].copy_from_slice(&udp_len.to_be_bytes());
    // pkt[6..8] = checksum (0 = disabled for UDP over IPv4)
    pkt[HEADER_LEN..].copy_from_slice(payload);

    ipv4::send(dst_ip, ipv4::PROTO_UDP, &pkt);
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

/// Stop listening on a port.
pub fn unlisten(port: u16) {
    let mut listeners = LISTENERS.lock();
    for slot in listeners.iter_mut() {
        if slot.as_ref().map_or(false, |l| l.port == port) {
            *slot = None;
        }
    }
}
