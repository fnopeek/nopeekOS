//! ICMP — Internet Control Message Protocol
//!
//! Handles ping (echo request/reply).

use crate::kprintln;
use super::ipv4;

const ECHO_REQUEST: u8 = 8;
const ECHO_REPLY: u8   = 0;

static PING_RECEIVED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static TTL_EXPIRED_FROM: spin::Mutex<Option<[u8; 4]>> = spin::Mutex::new(None);

/// Check if a TTL-expired ICMP was received (for traceroute)
pub fn ttl_expired_from() -> Option<[u8; 4]> {
    TTL_EXPIRED_FROM.lock().take()
}

pub fn ping_received() -> bool {
    PING_RECEIVED.swap(false, core::sync::atomic::Ordering::Relaxed)
}

pub fn handle_icmp(ip_packet: &[u8], data: &[u8]) {
    if data.len() < 8 { return; }

    let icmp_type = data[0];
    let src_ip = <[u8; 4]>::try_from(&ip_packet[12..16]).unwrap();

    match icmp_type {
        ECHO_REQUEST => send_echo_reply(src_ip, data),
        11 => {
            // Time Exceeded (TTL expired in transit) — used by traceroute
            *TTL_EXPIRED_FROM.lock() = Some(src_ip);
        }
        ECHO_REPLY => {
            let seq = u16::from_be_bytes([data[6], data[7]]);
            kprintln!("[npk] PONG from {}.{}.{}.{} seq={}",
                src_ip[0], src_ip[1], src_ip[2], src_ip[3], seq);
            PING_RECEIVED.store(true, core::sync::atomic::Ordering::Relaxed);
        }
        _ => {}
    }
}

fn send_echo_reply(dst_ip: [u8; 4], request: &[u8]) {
    let mut reply = alloc::vec![0u8; request.len()];
    reply.copy_from_slice(request);
    reply[0] = ECHO_REPLY;
    reply[1] = 0; // code

    // Recalculate ICMP checksum
    reply[2] = 0;
    reply[3] = 0;
    let checksum = icmp_checksum(&reply);
    reply[2..4].copy_from_slice(&checksum.to_be_bytes());

    ipv4::send(dst_ip, ipv4::PROTO_ICMP, &reply);
}

/// Send a ping (echo request) to the given IP
pub fn ping(dst_ip: [u8; 4], seq: u16) {
    let mut pkt = [0u8; 64];
    pkt[0] = ECHO_REQUEST;
    pkt[1] = 0;   // code
    pkt[4] = 0;   // identifier high
    pkt[5] = 1;   // identifier low
    pkt[6..8].copy_from_slice(&seq.to_be_bytes());

    // Fill payload with pattern
    for i in 8..64 {
        pkt[i] = i as u8;
    }

    // Checksum
    let checksum = icmp_checksum(&pkt);
    pkt[2..4].copy_from_slice(&checksum.to_be_bytes());

    ipv4::send(dst_ip, ipv4::PROTO_ICMP, &pkt);
    kprintln!("[npk] PING {}.{}.{}.{} seq={}",
        dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3], seq);
}

/// Send ICMP echo with custom TTL (for traceroute)
pub fn ping_ttl(dst_ip: [u8; 4], seq: u16, ttl: u8) {
    let mut pkt = [0u8; 64];
    pkt[0] = ECHO_REQUEST;
    pkt[5] = 1;
    pkt[6..8].copy_from_slice(&seq.to_be_bytes());
    for i in 8..64 { pkt[i] = i as u8; }
    let checksum = icmp_checksum(&pkt);
    pkt[2..4].copy_from_slice(&checksum.to_be_bytes());
    super::ipv4::send_with_ttl(dst_ip, super::ipv4::PROTO_ICMP, &pkt, ttl);
}

fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for i in (0..data.len()).step_by(2) {
        let word = if i + 1 < data.len() {
            u16::from_be_bytes([data[i], data[i + 1]])
        } else {
            (data[i] as u16) << 8
        };
        sum += word as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}
