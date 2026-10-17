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

pub fn handle_ipv4(data: &[u8], csum_ok: bool) {
    if data.len() < HEADER_LEN { return; }

    let version = data[0] >> 4;
    let ihl = (data[0] & 0x0F) as usize * 4;
    if version != 4 || ihl < 20 || data.len() < ihl { return; }

    let total_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if total_len < ihl || total_len > data.len() { return; }
    // ip_rcv_core: a header whose checksum does not hold is dropped.
    if super::csum::fold(super::csum::sum(&data[..ihl])) != 0xFFFF { return; }

    let protocol = data[9];
    let src_ip = <[u8; 4]>::try_from(&data[12..16]).unwrap();
    let dst_ip = <[u8; 4]>::try_from(&data[16..20]).unwrap();

    // The microvm tap gets first refusal, before the filter below. A guest
    // flow is keyed on the address it went out with; filtering on our current
    // address first would drop its replies whenever that address moved (DHCP
    // renewal, carrier blink). The tap matches against its own mappings.
    // Cheap no-op when no VM is up.
    if crate::microvm::devices::nat::tap_inbound(&data[..total_len], csum_ok) {
        return;
    }

    // Accept packets for our IP, broadcast, or during DHCP (IP = 0.0.0.0)
    let our_ip = arp::our_ip();
    if dst_ip != our_ip && dst_ip != [255, 255, 255, 255] && our_ip != [0, 0, 0, 0] { return; }

    let payload = &data[ihl..total_len];
    // The L4 checksum, unless the card verified it (skb_checksum_init). A
    // fragment is not whole, so its checksum cannot be checked yet.
    let fragment = u16::from_be_bytes([data[6], data[7]]) & 0x3FFF != 0;
    if !csum_ok && !fragment && !l4_checksum_holds(protocol, src_ip, dst_ip, payload) {
        return;
    }

    match protocol {
        PROTO_ICMP => super::icmp::handle_icmp(data, payload),
        PROTO_UDP => super::udp::handle_udp(data, payload),
        PROTO_TCP => super::tcp::handle_tcp(data, payload),
        _ => {}
    }
}

/// `tcp_v4_rcv`, `udp_rcv`, `icmp_rcv`: the payload's checksum holds. A
/// UDP datagram without one (0) is accepted, as RFC 768 allows over IPv4.
fn l4_checksum_holds(protocol: u8, src: [u8; 4], dst: [u8; 4], payload: &[u8]) -> bool {
    use super::csum::{fold, pseudo, sum};
    match protocol {
        PROTO_TCP => fold(pseudo(src, dst, PROTO_TCP, payload.len()) + sum(payload)) == 0xFFFF,
        PROTO_UDP => {
            if payload.len() < 8 {
                return false;
            }
            if payload[6] == 0 && payload[7] == 0 {
                return true;
            }
            let ulen = u16::from_be_bytes([payload[4], payload[5]]) as usize;
            if ulen < 8 || ulen > payload.len() {
                return false;
            }
            fold(pseudo(src, dst, PROTO_UDP, ulen) + sum(&payload[..ulen])) == 0xFFFF
        }
        PROTO_ICMP => fold(sum(payload)) == 0xFFFF,
        _ => true,
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
    transmit(dst_ip, protocol, payload, ttl, Tx::Wait, Forwarded::default()).0
}

/// The header fields a forwarded packet keeps from its sender: TOS (with the
/// ECN bits), identification and DF. A packet we originate has none of them.
#[derive(Clone, Copy, Default)]
pub struct Forwarded {
    pub tos: u8,
    pub id: u16,
    pub df: bool,
}

/// `send` that reports a refused frame instead of dropping it: `Ok(resolved)`
/// once the card took it, `Err(QueueFull)` when its ring is full and the
/// caller should keep the packet and retry.
pub fn try_send(dst_ip: [u8; 4], protocol: u8, payload: &[u8], head: Forwarded)
    -> Result<bool, crate::virtio_net::NetError>
{
    let (resolved, sent) = transmit(dst_ip, protocol, payload, 64, Tx::Try, head);
    sent.map(|()| resolved)
}

/// `try_send` for a TCP or UDP payload whose check field holds the
/// pseudo-header seed (`CHECKSUM_PARTIAL`); the card completes it.
pub fn try_send_partial(dst_ip: [u8; 4], protocol: u8, payload: &[u8], head: Forwarded)
    -> Result<bool, crate::virtio_net::NetError>
{
    let (resolved, sent) = transmit(dst_ip, protocol, payload, 64, Tx::TryPartial, head);
    sent.map(|()| resolved)
}

#[derive(Clone, Copy, PartialEq)]
enum Tx {
    /// Wait briefly for ring space.
    Wait,
    /// Refuse a full ring at once.
    Try,
    /// As `Try`, with the L4 checksum left to the card.
    TryPartial,
}

/// Build and hand over one packet. Returns whether the next hop's MAC was
/// known, and the card's verdict.
fn transmit(dst_ip: [u8; 4], protocol: u8, payload: &[u8], ttl: u8, tx: Tx,
            head: Forwarded)
    -> (bool, Result<(), crate::virtio_net::NetError>)
{
    let src_ip = arp::our_ip();
    let total_len = (HEADER_LEN + payload.len()) as u16;

    let mut pkt = alloc::vec![0u8; total_len as usize];

    pkt[0] = 0x45;
    pkt[1] = head.tos;
    pkt[2..4].copy_from_slice(&total_len.to_be_bytes());
    pkt[4..6].copy_from_slice(&head.id.to_be_bytes());
    if head.df { pkt[6] = 0x40; }
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
        // A broadcast destination is the broadcast MAC; asking ARP who owns
        // 255.255.255.255 can only ever get no answer.
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

    let sent = match tx {
        Tx::Wait => eth::send_frame(&dst_mac, eth::ETHERTYPE_IPV4, &pkt),
        Tx::Try => eth::try_send_frame(&dst_mac, eth::ETHERTYPE_IPV4, &pkt),
        Tx::TryPartial => eth::try_send_frame_csum(&dst_mac, eth::ETHERTYPE_IPV4, &pkt,
            HEADER_LEN, protocol == PROTO_TCP),
    };
    (resolved, sent)
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
