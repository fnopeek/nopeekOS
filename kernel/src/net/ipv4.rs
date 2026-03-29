//! IPv4 — Internet Protocol v4

use crate::kprintln;
use super::{eth, arp};

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_UDP: u8  = 17;
pub const PROTO_TCP: u8  = 6;
const HEADER_LEN: usize  = 20; // no options

pub fn handle_ipv4(data: &[u8]) {
    if data.len() < HEADER_LEN { return; }

    let version = data[0] >> 4;
    let ihl = (data[0] & 0x0F) as usize * 4;
    if version != 4 || ihl < 20 || data.len() < ihl { return; }

    let total_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if total_len > data.len() { return; }

    let protocol = data[9];
    let _src_ip = &data[12..16];
    let dst_ip = <[u8; 4]>::try_from(&data[16..20]).unwrap();

    // Accept packets for our IP, broadcast, or during DHCP (IP = 0.0.0.0)
    let our_ip = arp::our_ip();
    if dst_ip != our_ip && dst_ip != [255, 255, 255, 255] && our_ip != [0, 0, 0, 0] { return; }

    let payload = &data[ihl..total_len.min(data.len())];

    match protocol {
        PROTO_ICMP => super::icmp::handle_icmp(data, payload),
        PROTO_UDP => super::udp::handle_udp(data, payload),
        PROTO_TCP => super::tcp::handle_tcp(data, payload),
        _ => {}
    }
}

/// Send an IPv4 packet
pub fn send(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) {
    send_with_ttl(dst_ip, protocol, payload, 64);
}

/// Send an IPv4 packet with custom TTL (for traceroute)
pub fn send_with_ttl(dst_ip: [u8; 4], protocol: u8, payload: &[u8], ttl: u8) {
    let src_ip = arp::our_ip();
    let total_len = (HEADER_LEN + payload.len()) as u16;

    let mut pkt = alloc::vec![0u8; total_len as usize];

    pkt[0] = 0x45;
    pkt[2..4].copy_from_slice(&total_len.to_be_bytes());
    pkt[8] = ttl;
    pkt[9] = protocol;
    pkt[12..16].copy_from_slice(&src_ip);
    pkt[16..20].copy_from_slice(&dst_ip);

    // Checksum
    let checksum = ipv4_checksum(&pkt[..HEADER_LEN]);
    pkt[10..12].copy_from_slice(&checksum.to_be_bytes());

    // Payload
    pkt[HEADER_LEN..].copy_from_slice(payload);

    // Resolve MAC (use gateway for non-local, or ARP cache)
    let dst_mac = arp::lookup(dst_ip).unwrap_or(
        // Default gateway MAC: try the gateway IP (10.0.2.2 in QEMU user-mode)
        arp::lookup([10, 0, 2, 2]).unwrap_or(eth::BROADCAST)
    );

    let _ = eth::send_frame(&dst_mac, eth::ETHERTYPE_IPV4, &pkt);
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    for i in (0..header.len()).step_by(2) {
        let word = if i + 1 < header.len() {
            u16::from_be_bytes([header[i], header[i + 1]])
        } else {
            (header[i] as u16) << 8
        };
        sum += word as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}
