//! IPv4 — Internet Protocol v4

use super::{eth, arp};
use spin::Mutex;

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_UDP: u8  = 17;
pub const PROTO_TCP: u8  = 6;
const HEADER_LEN: usize  = 20; // no options

static GATEWAY: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 2]); // QEMU default
static SUBNET:  Mutex<[u8; 4]> = Mutex::new([255, 255, 255, 0]);

/// Datagrams dropped by the receive checks, by `net_core::ip::Drop`.
static IN_DROPS: [core::sync::atomic::AtomicU64; net_core::ip::Drop::COUNT] =
    [const { core::sync::atomic::AtomicU64::new(0) }; net_core::ip::Drop::COUNT];

/// Datagrams refused for want of a route (Linux MIB `OutNoRoutes`).
static OUT_NO_ROUTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn drop_in(d: net_core::ip::Drop) {
    IN_DROPS[d as usize].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Every receive check that dropped something, with its count, and the
/// datagrams refused on the way out.
pub fn drops() -> (alloc::vec::Vec<(&'static str, u64)>, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let ins = net_core::ip::Drop::ALL.iter()
        .map(|d| (d.name(), IN_DROPS[*d as usize].load(Relaxed)))
        .filter(|(_, n)| *n > 0)
        .collect();
    (ins, OUT_NO_ROUTES.load(Relaxed))
}

/// Our address and mask.
pub fn iface() -> net_core::ip::Iface {
    net_core::ip::Iface { ip: arp::our_ip(), mask: *SUBNET.lock() }
}

/// Fragments dropped for want of reassembly (Linux MIB `ReasmFails`, #36).
static REASM_FAILS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn reasm_fails() -> u64 {
    REASM_FAILS.load(core::sync::atomic::Ordering::Relaxed)
}

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

pub fn handle_ipv4(data: &[u8], csum_ok: bool, l2: net_core::ip::PktType) {
    if data.len() < HEADER_LEN { return; }

    let version = data[0] >> 4;
    let ihl = (data[0] & 0x0F) as usize * 4;
    if version != 4 || ihl < 20 || data.len() < ihl { return; }

    let total_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if total_len < ihl || total_len > data.len() { return; }
    // ip_rcv_core: a header whose checksum does not hold is dropped.
    if super::csum::fold(super::csum::sum(&data[..ihl])) != 0xFFFF { return; }

    // No reassembly yet (#36): a fragment parsed as a whole datagram hands
    // payload bytes to L4 as headers, unchecked. Dropped before anything,
    // the microvm tap included.
    if u16::from_be_bytes([data[6], data[7]]) & 0x3FFF != 0 {
        REASM_FAILS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return;
    }

    let protocol = data[9];
    let src_ip = <[u8; 4]>::try_from(&data[12..16]).unwrap();
    let dst_ip = <[u8; 4]>::try_from(&data[16..20]).unwrap();

    // ip_rcv_options, and the martian checks that hold whatever our address.
    if let Err(d) = net_core::ip::options_ok(&data[HEADER_LEN..ihl])
        .and_then(|()| net_core::ip::early(l2, src_ip, dst_ip))
    {
        drop_in(d);
        return;
    }

    // The microvm tap gets first refusal, before the filter below. A guest
    // flow is keyed on the address it went out with; filtering on our current
    // address first would drop its replies whenever that address moved (DHCP
    // renewal, carrier blink). The tap matches against its own mappings.
    // Cheap no-op when no VM is up.
    if crate::microvm::devices::nat::tap_inbound(&data[..total_len], csum_ok) {
        return;
    }

    let payload = &data[ihl..total_len];
    let dhcp_reply = protocol == PROTO_UDP && payload.len() >= 4
        && u16::from_be_bytes([payload[2], payload[3]]) == 68
        && super::dhcp::is_running();
    let dest = match iface().input_route(l2, src_ip, dst_ip, dhcp_reply) {
        Ok(d) => d,
        Err(d) => { drop_in(d); return; }
    };
    // The L4 checksum, unless the card verified it (skb_checksum_init).
    if !csum_ok && !l4_checksum_holds(protocol, src_ip, dst_ip, payload) {
        return;
    }

    // TCP takes only datagrams to our address (`tcp_v4_rcv`, RFC 1122
    // §4.2.3.10); ICMP to a broadcast address is ignored, echo requests
    // included (`icmp_echo_ignore_broadcasts`).
    let local = dest == net_core::ip::Dest::Local;
    match protocol {
        PROTO_ICMP if local => super::icmp::handle_icmp(data, payload),
        PROTO_UDP => super::udp::handle_udp(data, payload),
        PROTO_TCP if local => super::tcp::handle_tcp(data, payload),
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

/// Send an IPv4 packet. Returns false if the next hop's MAC was unknown: the
/// packet then waits for its ARP answer (`arp::output`).
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

/// `send` that reports a refused frame instead of dropping it: `Ok(true)`
/// once the card took it, `Ok(false)` while it waits for its next hop's
/// address, `Err(QueueFull)` when the ring is full and the caller should keep
/// the packet and retry.
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

/// Build and hand over one packet. Returns whether it went to the card (or
/// waits for its next hop's address), and the card's verdict.
fn transmit(dst_ip: [u8; 4], protocol: u8, payload: &[u8], ttl: u8, tx: Tx,
            head: Forwarded)
    -> (bool, Result<(), crate::virtio_net::NetError>)
{
    // __mkroute_output: no loopback device, and 0/8 and 240/4 lead nowhere.
    if !iface().output_ok(dst_ip, false) {
        OUT_NO_ROUTES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return (false, Ok(()));
    }
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

    let mode = match tx {
        Tx::Wait => arp::Mode::Wait,
        Tx::Try => arp::Mode::Try,
        Tx::TryPartial => arp::Mode::Partial { tcp: protocol == PROTO_TCP },
    };
    let hop = arp_target_for(dst_ip);
    if hop == [255, 255, 255, 255] {
        // A broadcast destination is the broadcast MAC; asking ARP who owns
        // 255.255.255.255 can only ever get no answer.
        let sent = match mode {
            arp::Mode::Wait => eth::send_frame(&eth::BROADCAST, eth::ETHERTYPE_IPV4, &pkt),
            _ => eth::try_send_frame(&eth::BROADCAST, eth::ETHERTYPE_IPV4, &pkt),
        };
        return (true, sent);
    }
    // A unicast datagram never goes to the link-layer broadcast address
    // (RFC 1122 §3.3.6): with the next hop unknown it waits in the
    // neighbour's queue until ARP answers.
    match arp::output(hop, arp::Pkt { ip: pkt, mode }) {
        Ok(sent) => (sent, Ok(())),
        Err(e) => (true, Err(e)),
    }
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
