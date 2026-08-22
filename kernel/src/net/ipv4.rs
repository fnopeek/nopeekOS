//! IPv4 — Internet Protocol v4

use super::{eth, arp};
use spin::Mutex;

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_UDP: u8  = 17;
pub const PROTO_TCP: u8  = 6;
const HEADER_LEN: usize  = 20; // no options

static GATEWAY: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 2]); // QEMU default
static SUBNET:  Mutex<[u8; 4]> = Mutex::new([255, 255, 255, 0]);

pub fn set_gateway(ip: [u8; 4]) { *GATEWAY.lock() = ip; }
pub fn set_subnet(mask: [u8; 4]) { *SUBNET.lock() = mask; }
pub fn gateway() -> [u8; 4] { *GATEWAY.lock() }
#[allow(dead_code)]
pub fn subnet() -> [u8; 4] { *SUBNET.lock() }

pub fn prefix_len() -> u8 {
    let m = *SUBNET.lock();
    let bits = ((m[0] as u32) << 24) | ((m[1] as u32) << 16) | ((m[2] as u32) << 8) | (m[3] as u32);
    bits.count_ones() as u8
}

/// Pick the IP whose MAC the next-hop frame is addressed to: the destination
/// itself if it's link-local (or broadcast), otherwise the configured gateway.
/// Public so `tcp::connect` can pre-resolve the same hop ARP-wise before
/// taking any network-stack lock.
pub fn arp_target_for(dst_ip: [u8; 4]) -> [u8; 4] {
    if dst_ip == [255, 255, 255, 255] { return dst_ip; }
    let src = arp::our_ip();
    let mask = *SUBNET.lock();
    let src_masked = [src[0] & mask[0], src[1] & mask[1], src[2] & mask[2], src[3] & mask[3]];
    let dst_masked = [dst_ip[0] & mask[0], dst_ip[1] & mask[1], dst_ip[2] & mask[2], dst_ip[3] & mask[3]];
    if src_masked == dst_masked { dst_ip } else { *GATEWAY.lock() }
}

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

    // The microvm tap gets first refusal, BEFORE the filter below. A guest flow
    // is keyed on the address it went OUT with; asking "is this addressed to the
    // address we happen to hold right now" first throws the reply away whenever
    // that address has moved — a DHCP renewal, a carrier blink — and every live
    // flow dies silently at this line. The tap's own test is "does it match a
    // mapping", which is the question that has an answer. Cheap no-op when no
    // VM is up.
    if crate::microvm::devices::nat::tap_inbound(&data[..total_len.min(data.len())]) {
        return;
    }

    // Accept packets for our IP, broadcast, or during DHCP (IP = 0.0.0.0)
    let our_ip = arp::our_ip();
    if dst_ip != our_ip && dst_ip != [255, 255, 255, 255] && our_ip != [0, 0, 0, 0] { return; }

    // Legacy inline intercept — unreachable now that the tap runs first (same
    // table, so anything it would claim the tap already claimed). Goes with the
    // rest of the inline path.
    if crate::microvm::devices::nat::l3_inbound(&data[..total_len.min(data.len())]) {
        return;
    }

    let payload = &data[ihl..total_len.min(data.len())];

    match protocol {
        PROTO_ICMP => super::icmp::handle_icmp(data, payload),
        PROTO_UDP => super::udp::handle_udp(data, payload),
        PROTO_TCP => super::tcp::handle_tcp(data, payload),
        _ => {}
    }
}

/// Send an IPv4 packet. Returns false if the next hop's MAC was unknown and the
/// frame went to L2 broadcast — most gateways drop that, so for the masquerade
/// it is a silent loss and the caller wants to count it.
pub fn send(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> bool {
    send_with_ttl(dst_ip, protocol, payload, 64)
}

/// Send an IPv4 packet with custom TTL (for traceroute)
pub fn send_with_ttl(dst_ip: [u8; 4], protocol: u8, payload: &[u8], ttl: u8) -> bool {
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

    // Resolve next-hop MAC. On cache miss we fire an ARP request so the
    // gateway responds and `super::poll` populates the cache before the
    // caller's next retry — without it, every fresh-boot first packet
    // (TCP SYN, DNS query, etc.) gets sent to L2 broadcast and silently
    // dropped by most gateways. Active resolution is left to callers
    // that can poll without holding network locks (see `arp::resolve`,
    // used by `tcp::connect`).
    let mut resolved = true;
    let arp_target = arp_target_for(dst_ip);
    let dst_mac = if arp_target == [255, 255, 255, 255] {
        // A broadcast destination IS the broadcast MAC. Asking ARP who owns
        // 255.255.255.255 put a nonsense request on the air before every DHCP
        // packet, and the answer could only ever be "nobody".
        eth::BROADCAST
    } else {
        match arp::lookup(arp_target) {
            Some(mac) => mac,
            None => {
                arp::request(arp_target);
                resolved = false;
                eth::BROADCAST
            }
        }
    };

    let _ = eth::send_frame(&dst_mac, eth::ETHERTYPE_IPV4, &pkt);
    resolved
}

/// Send a TCP segment with HOST-NIC checksum + TSO offload (the microvm
/// masquerade TX fast path — symmetric with RX: forward the frame, let the
/// device segment + checksum). `tcp_seg` is the (port-rewritten) TCP header +
/// payload whose checksum field already holds the pseudo-header PARTIAL
/// (CHECKSUM_PARTIAL); `gso_size` is the MSS (0 ⇒ checksum-only). Builds the IP +
/// ethernet headers like `send`, then hands the whole frame to
/// `virtio_net::send_offload` (no SW segmentation / checksum). Returns false (and
/// the caller may retry via the SW path) if the NIC can't take it.
pub fn send_offload(dst_ip: [u8; 4], protocol: u8, tcp_seg: &[u8], gso_size: u16) -> bool {
    if tcp_seg.len() < 20 { return false; }
    let src_ip = arp::our_ip();
    let total_len = HEADER_LEN + tcp_seg.len();
    if total_len > 0xFFFF { return false; } // IP total-length is u16
    // IP packet (the device recomputes per-segment total-length + checksum).
    let mut pkt = alloc::vec![0u8; total_len];
    pkt[0] = 0x45;
    pkt[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    pkt[8] = 64;
    pkt[9] = protocol;
    pkt[12..16].copy_from_slice(&src_ip);
    pkt[16..20].copy_from_slice(&dst_ip);
    let checksum = ipv4_checksum(&pkt[..HEADER_LEN]);
    pkt[10..12].copy_from_slice(&checksum.to_be_bytes());
    pkt[HEADER_LEN..].copy_from_slice(tcp_seg);

    let arp_target = arp_target_for(dst_ip);
    let dst_mac = match arp::lookup(arp_target) {
        Some(mac) => mac,
        None => { arp::request(arp_target); return false; } // no L2-broadcast for bulk TX
    };
    let src_mac = crate::netdev::mac().unwrap_or([0; 6]);

    // Ethernet frame for the host NIC.
    let mut frame = alloc::vec![0u8; eth::HEADER_LEN + pkt.len()];
    frame[0..6].copy_from_slice(&dst_mac);
    frame[6..12].copy_from_slice(&src_mac);
    frame[12..14].copy_from_slice(&eth::ETHERTYPE_IPV4.to_be_bytes());
    frame[eth::HEADER_LEN..].copy_from_slice(&pkt);

    // csum_start = eth(14) + IP(20) = TCP offset; csum_offset = 16 (TCP check);
    // hdr_len = csum_start + TCP header length.
    let tcp_hl = (((tcp_seg[12] >> 4) & 0x0F) as usize * 4) as u16;
    let csum_start = (eth::HEADER_LEN + HEADER_LEN) as u16;
    let hdr_len = csum_start + tcp_hl;
    crate::virtio_net::send_offload(&frame, gso_size, csum_start, 16, hdr_len).is_ok()
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
