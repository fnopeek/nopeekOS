//! IPv4 — Internet Protocol v4

use super::{eth, arp};
use spin::Mutex;

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_UDP: u8  = 17;
pub const PROTO_TCP: u8  = 6;
const HEADER_LEN: usize  = 20; // no options

static SUBNET: Mutex<[u8; 4]> = Mutex::new([0; 4]);
static ROUTES: Mutex<net_core::ip::Routes> = Mutex::new(net_core::ip::Routes::new());

/// Datagrams dropped by the receive checks, by `net_core::ip::Drop`.
static IN_DROPS: [core::sync::atomic::AtomicU64; net_core::ip::Drop::COUNT] =
    [const { core::sync::atomic::AtomicU64::new(0) }; net_core::ip::Drop::COUNT];

use net_core::mib::{self, Mib};

fn drop_in(d: net_core::ip::Drop) {
    IN_DROPS[d as usize].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    use net_core::ip::Drop;
    match d {
        Drop::BadOptions | Drop::SourceRoute => mib::inc(Mib::IpInHdrErrors),
        Drop::MartianDest | Drop::NotForUs => mib::inc(Mib::IpInAddrErrors),
        _ => {}
    }
}

/// Every receive check that dropped something, with its count, and the
/// datagrams refused on the way out.
pub fn drops() -> (alloc::vec::Vec<(&'static str, u64)>, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let ins = net_core::ip::Drop::ALL.iter()
        .map(|d| (d.name(), IN_DROPS[*d as usize].load(Relaxed)))
        .filter(|(_, n)| *n > 0)
        .collect();
    (ins, mib::get(Mib::IpOutNoRoutes))
}

/// The largest IPv4 datagram the Ethernet devices carry.
pub const DEVICE_MTU: u32 = (crate::netdev::MTU - eth::HEADER_LEN) as u32;

/// The interface MTU: the device's, or less when DHCP says so (option 26).
static MTU: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(DEVICE_MTU);


/// Set the interface MTU, within 68 (RFC 791) and what the device carries.
pub fn set_mtu(mtu: u32) {
    MTU.store(mtu.clamp(68, DEVICE_MTU), core::sync::atomic::Ordering::Relaxed);
}

pub fn mtu() -> u32 {
    MTU.load(core::sync::atomic::Ordering::Relaxed)
}

/// Path MTUs learned from Fragmentation Needed.
static PMTU: Mutex<net_core::ip::Pmtu> = Mutex::new(net_core::ip::Pmtu::new());

/// The path MTU to `dst`.
pub fn path_mtu(dst: [u8; 4]) -> u32 {
    PMTU.lock().get(dst, mtu(), crate::interrupts::ticks())
}

/// A router said the path to `dst` carries only `mtu`.
pub fn update_pmtu(dst: [u8; 4], mtu: u32) {
    PMTU.lock().update(dst, mtu, self::mtu(), crate::interrupts::ticks());
}

/// Forget the learned path MTUs: they belong to the network we left.
pub fn flush_pmtu() {
    PMTU.lock().flush();
}

/// The limits on the ICMP errors we send.
static ICMP_LIMIT: Mutex<net_core::ip::IcmpLimit> = Mutex::new(net_core::ip::IcmpLimit::new());

/// ICMP errors held back by the limits (`IcmpOutRateLimitHost`).
pub fn icmp_limited() -> u64 {
    mib::get(Mib::IcmpOutRateLimitHost)
}

/// `__icmp_send`: tell the sender of `orig`, a datagram that came to our
/// address in a unicast frame, what became of it; within the limits.
pub fn icmp_send(orig: &[u8], kind: u8, code: u8, info: u32) {
    let Some(msg) = net_core::ip::icmp_error_message(orig, kind, code, info) else { return };
    let dst: [u8; 4] = orig[12..16].try_into().unwrap();
    let allowed = ICMP_LIMIT.lock().allow(dst, kind, code, crate::interrupts::ticks(),
        || (crate::csprng::random_u64() % 3) as i64);
    if !allowed {
        mib::inc(Mib::IcmpOutRateLimitHost);
        return;
    }
    mib::inc(Mib::IcmpOutMsgs);
    mib::inc(match kind {
        net_core::ip::ICMP_DEST_UNREACH => Mib::IcmpOutDestUnreachs,
        net_core::ip::ICMP_TIME_EXCEEDED => Mib::IcmpOutTimeExcds,
        _ => Mib::IcmpOutParmProbs,
    });
    let src = arp::our_ip();
    let head = Forwarded { tos: net_core::ip::TOS_INTERNETCONTROL, id: select_ident(dst, src, PROTO_ICMP), df: false };
    let _ = transmit(dst, PROTO_ICMP, &msg, 64, Tx::Wait, Some(head), None);
}

/// The identification counters for datagrams that may be fragmented.
static IDENTS: spin::Once<Mutex<net_core::ip::Idents>> = spin::Once::new();

/// `__ip_select_ident`.
fn select_ident(dst: [u8; 4], src: [u8; 4], proto: u8) -> u16 {
    let idents = IDENTS.call_once(|| Mutex::new(net_core::ip::Idents::new(
        crate::csprng::random_256(), crate::memory::usable_bytes(),
        || crate::csprng::random_u64() as u32)));
    idents.lock().select(dst, src, proto, crate::interrupts::ticks() as u32,
        |range| if range == 0 { 0 } else { (crate::csprng::random_u64() % range as u64) as u32 })
}

/// Datagrams larger than the MTU, refused: nothing fragments them
/// (`IpFragFails`).
pub fn too_big() -> u64 {
    mib::get(Mib::IpFragFails)
}

/// Our address and mask.
pub fn iface() -> net_core::ip::Iface {
    net_core::ip::Iface { ip: arp::our_ip(), mask: *SUBNET.lock() }
}

/// The reassembly queues.
static DEFRAG: Mutex<net_core::frag::Defrag> = Mutex::new(net_core::frag::Defrag::new());

/// Datagrams reassembled, and reassemblies that failed or timed out (Linux
/// MIB `ReasmOKs`, `ReasmFails`, `ReasmTimeout`).
pub fn reasm_stats() -> (u64, u64, u64) {
    let d = DEFRAG.lock();
    (d.oks, d.fails, d.timeouts)
}

/// `ip_expire`: incomplete datagrams past 30 s go, and whoever sent one to
/// our address whose fragment 0 came learns so (RFC 1122 §3.3.2 MUST).
pub fn tick() {
    let firsts = DEFRAG.lock().expire(crate::interrupts::ticks());
    let our = arp::our_ip();
    for f in firsts {
        if f[16..20] == our {
            icmp_send(&f, net_core::ip::ICMP_TIME_EXCEEDED, 1, 0);
        }
    }
}

pub fn set_subnet(mask: [u8; 4]) { *SUBNET.lock() = mask; }
pub fn subnet() -> [u8; 4] { *SUBNET.lock() }

pub fn set_routes(routes: &[net_core::ip::Route]) { ROUTES.lock().set(routes); }
pub fn clear_routes() { ROUTES.lock().clear(); }
pub fn routes() -> alloc::vec::Vec<net_core::ip::Route> { ROUTES.lock().list().to_vec() }

/// The gateway of the default route.
pub fn default_gateway() -> Option<[u8; 4]> { ROUTES.lock().default_gateway() }

/// The default gateway, 0.0.0.0 without one.
pub fn gateway() -> [u8; 4] { default_gateway().unwrap_or([0; 4]) }

pub fn prefix_len() -> u8 {
    let m = *SUBNET.lock();
    let bits = ((m[0] as u32) << 24) | ((m[1] as u32) << 16) | ((m[2] as u32) << 8) | (m[3] as u32);
    bits.count_ones() as u8
}

/// The IP whose MAC the frame to `dst_ip` is addressed to: the limited
/// broadcast itself, else the route's next hop; `None` without a route.
pub fn arp_target_for(dst_ip: [u8; 4]) -> Option<[u8; 4]> {
    if dst_ip == [255, 255, 255, 255] { return Some(dst_ip); }
    ROUTES.lock().next_hop(dst_ip)
}

pub fn handle_ipv4(data: &[u8], csum_ok: bool, l2: net_core::ip::PktType) {
    mib::inc(Mib::IpInReceives);
    if data.len() < HEADER_LEN {
        mib::inc(Mib::IpInHdrErrors);
        return;
    }

    let version = data[0] >> 4;
    let ihl = (data[0] & 0x0F) as usize * 4;
    let total_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    // ip_rcv_core: a malformed header, or one whose checksum does not hold.
    if version != 4 || ihl < 20 || data.len() < ihl
        || total_len < ihl || total_len > data.len()
        || super::csum::fold(super::csum::sum(&data[..ihl])) != 0xFFFF
    {
        mib::inc(Mib::IpInHdrErrors);
        return;
    }

    let protocol = data[9];
    let src_ip = <[u8; 4]>::try_from(&data[12..16]).unwrap();
    let dst_ip = <[u8; 4]>::try_from(&data[16..20]).unwrap();

    // ip_rcv_options, and the martian checks that hold whatever our address.
    if let Err((d, at)) = net_core::ip::options_ok(&data[HEADER_LEN..ihl]) {
        // A malformed list is reported back (`ip_options_compile`), when
        // the datagram was ours.
        if d == net_core::ip::Drop::BadOptions
            && iface().input_route(l2, src_ip, dst_ip, false) == Ok(net_core::ip::Dest::Local)
        {
            icmp_send(&data[..total_len], net_core::ip::ICMP_PARAMETERPROB, 0,
                ((HEADER_LEN + at) as u32) << 24);
        }
        drop_in(d);
        return;
    }
    if let Err(d) = net_core::ip::early(l2, src_ip, dst_ip) {
        drop_in(d);
        return;
    }

    // ip_defrag, before the microVM's masquerade and before local delivery
    // (`ipv4_conntrack_defrag`, `ip_local_deliver`): a fragment waits for
    // the rest, and the whole datagram goes on from here.
    if u16::from_be_bytes([data[6], data[7]]) & 0x3FFF != 0 {
        mib::inc(Mib::IpReasmReqds);
        let r = DEFRAG.lock().input(&data[..total_len], crate::interrupts::ticks());
        if let net_core::frag::Defragged::Complete(whole) = r {
            handle_ipv4(&whole, false, l2);
        }
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
        && super::dhcp::accepts_any_dst();
    let dest = match iface().input_route(l2, src_ip, dst_ip, dhcp_reply) {
        Ok(d) => d,
        Err(d) => { drop_in(d); return; }
    };
    // The L4 checksum, unless the card verified it (skb_checksum_init).
    if !csum_ok && !l4_checksum_holds(protocol, src_ip, dst_ip, payload) {
        let (errs, csum) = match protocol {
            PROTO_TCP => (Mib::TcpInErrs, Mib::TcpInCsumErrors),
            PROTO_UDP => (Mib::UdpInErrors, Mib::UdpInCsumErrors),
            _ => (Mib::IcmpInErrors, Mib::IcmpInCsumErrors),
        };
        mib::inc(errs);
        mib::inc(csum);
        return;
    }

    // TCP takes only datagrams to our address (`tcp_v4_rcv`, RFC 1122
    // §4.2.3.10); ICMP to a broadcast address is ignored, echo requests
    // included (`icmp_echo_ignore_broadcasts`).
    let local = dest == net_core::ip::Dest::Local;
    if matches!(protocol, PROTO_UDP) || (local && matches!(protocol, PROTO_ICMP | PROTO_TCP)) {
        mib::inc(Mib::IpInDelivers);
    }
    match protocol {
        PROTO_ICMP if local => super::icmp::handle_icmp(data, payload),
        PROTO_UDP => super::udp::handle_udp(data, payload),
        PROTO_TCP if local => super::tcp::handle_tcp(data, payload),
        PROTO_ICMP | PROTO_TCP => {}
        // ip_local_deliver_finish: nobody speaks it.
        _ if local => {
            mib::inc(Mib::IpInUnknownProtos);
            icmp_send(&data[..total_len], net_core::ip::ICMP_DEST_UNREACH,
                net_core::ip::ICMP_PROT_UNREACH, 0)
        }
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
    matches!(transmit(dst_ip, protocol, payload, ttl, Tx::Wait, None, None), Ok(Handed::Card))
}

/// Send from `src`, whatever our address is (the DHCP client's packet
/// socket sends from 0.0.0.0). True once it is on its way: taken by the
/// card, or waiting for ARP, which `neigh_resolve_output` reports as sent.
pub fn send_from(src: [u8; 4], dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> bool {
    matches!(transmit(dst_ip, protocol, payload, 64, Tx::Wait, None, Some(src)),
        Ok(Handed::Card | Handed::Queued))
}

/// Send a TCP segment of a connection: DF set (`IP_PMTUDISC_WANT`), its
/// identification from the connection's `inet_id`. True once it is on its
/// way: taken by the card, or queued until ARP answers, which counts as sent
/// as `neigh_resolve_output` returns success for it. False when the card
/// refused it or it has no route (`tcp_transmit_skb` reporting an error), so
/// the connection keeps it for later.
pub fn send_tcp(dst_ip: [u8; 4], segment: &[u8], id: u16) -> bool {
    matches!(transmit(dst_ip, PROTO_TCP, segment, 64, Tx::Wait, Some(Forwarded { tos: 0, id, df: true }), None),
        Ok(Handed::Card | Handed::Queued))
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
    transmit(dst_ip, protocol, payload, 64, Tx::Try, Some(head), None).map(|h| h == Handed::Card)
}

/// `try_send` for a TCP or UDP payload whose check field holds the
/// pseudo-header seed (`CHECKSUM_PARTIAL`); the card completes it.
pub fn try_send_partial(dst_ip: [u8; 4], protocol: u8, payload: &[u8], head: Forwarded)
    -> Result<bool, crate::virtio_net::NetError>
{
    transmit(dst_ip, protocol, payload, 64, Tx::TryPartial, Some(head), None).map(|h| h == Handed::Card)
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

/// Where `transmit` left a packet the card did not refuse.
#[derive(Clone, Copy, PartialEq)]
enum Handed {
    /// Taken by the card.
    Card,
    /// Waiting in the neighbour's queue for ARP.
    Queued,
    /// Dropped: no route, or larger than the path MTU.
    Nowhere,
}

/// Build and hand over one packet: where it went, or the card's refusal. `head`: the
/// fields of a TCP segment or a forwarded packet; `None` for a datagram of
/// ours that no connection sends, which gets them as
/// `ip_select_ident_segs` does for an unconnected socket. `src`: the source
/// address, ours by default.
fn transmit(dst_ip: [u8; 4], protocol: u8, payload: &[u8], ttl: u8, tx: Tx,
            head: Option<Forwarded>, src: Option<[u8; 4]>)
    -> Result<Handed, crate::virtio_net::NetError>
{
    mib::inc(Mib::IpOutRequests);
    // __mkroute_output: no loopback device, and 0/8 and 240/4 lead nowhere.
    if !iface().output_ok(dst_ip, false) {
        mib::inc(Mib::IpOutNoRoutes);
        return Ok(Handed::Nowhere);
    }
    // ip_finish_output: nothing larger than the path MTU goes out (RFC 1122
    // §3.3.3 MMS_S); there is no local fragmentation.
    if (HEADER_LEN + payload.len()) as u32 > path_mtu(dst_ip) {
        mib::inc(Mib::IpFragFails);
        return Ok(Handed::Nowhere);
    }
    let src_ip = src.unwrap_or_else(arp::our_ip);
    let total_len = (HEADER_LEN + payload.len()) as u16;
    let head = head.unwrap_or_else(|| if protocol == PROTO_UDP {
        // DF set (`IP_PMTUDISC_WANT`): an atomic datagram needs no unique
        // identification (RFC 6864 §4.1).
        Forwarded { tos: 0, id: 0, df: true }
    } else {
        // ICMP's socket is `IP_PMTUDISC_DONT`: no DF, a unique identification.
        Forwarded { tos: 0, id: select_ident(dst_ip, src_ip, protocol), df: false }
    });

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
    let Some(hop) = arp_target_for(dst_ip) else {
        mib::inc(Mib::IpOutNoRoutes);
        return Ok(Handed::Nowhere);
    };
    if hop == [255, 255, 255, 255] {
        // A broadcast destination is the broadcast MAC; asking ARP who owns
        // 255.255.255.255 can only ever get no answer.
        let sent = match mode {
            arp::Mode::Wait => eth::send_frame(&eth::BROADCAST, eth::ETHERTYPE_IPV4, &pkt),
            _ => eth::try_send_frame(&eth::BROADCAST, eth::ETHERTYPE_IPV4, &pkt),
        };
        return sent.map(|()| Handed::Card);
    }
    // A unicast datagram never goes to the link-layer broadcast address
    // (RFC 1122 §3.3.6): with the next hop unknown it waits in the
    // neighbour's queue until ARP answers.
    arp::output(hop, arp::Pkt { ip: pkt, mode })
        .map(|sent| if sent { Handed::Card } else { Handed::Queued })
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    !super::csum::fold(super::csum::sum(header))
}
