//! UDP — User Datagram Protocol
//!
//! Stateless, unreliable transport. Foundation for DNS and DHCP.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;
use super::ipv4;

const HEADER_LEN: usize = 8;

/// `net.core.rmem_default`: what a listener holds unread before arrivals
/// are dropped (`__udp_enqueue_schedule_skb`), counted in payload bytes.
const RCVBUF: usize = 212_992;

/// Ephemeral ports (RFC 6056 §3.2: all of 1024-65535), without the range
/// the microVM's masquerade uses.
const EPHEMERAL: core::ops::RangeInclusive<u16> = 1024..=65535;
const NAT_PORTS: core::ops::Range<u16> = 20000..40000;

struct Datagram {
    src_ip: [u8; 4],
    src_port: u16,
    data: Vec<u8>,
}

struct UdpListener {
    port: u16,
    queue: VecDeque<Datagram>,
    queued: usize,
    /// A hard ICMP error for a datagram it sent: (to address, to port).
    err: Option<([u8; 4], u16)>,
}

static LISTENERS: Mutex<Vec<UdpListener>> = Mutex::new(Vec::new());

use net_core::mib::{self, Mib};

/// Datagrams dropped because their listener's buffer was full
/// (`UdpRcvbufErrors`).
pub fn rcvbuf_errors() -> u64 {
    mib::get(Mib::UdpRcvbufErrors)
}

fn new_listener(port: u16) -> UdpListener {
    UdpListener { port, queue: VecDeque::new(), queued: 0, err: None }
}

pub fn handle_udp(ip_packet: &[u8], data: &[u8]) {
    if data.len() < HEADER_LEN { return; }

    let src_port = u16::from_be_bytes([data[0], data[1]]);
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let length = u16::from_be_bytes([data[4], data[5]]) as usize;

    if length < HEADER_LEN || length > data.len() {
        mib::inc(Mib::UdpInErrors);
        return;
    }
    let payload = &data[HEADER_LEN..length.min(data.len())];
    let src_ip = <[u8; 4]>::try_from(&ip_packet[12..16]).unwrap();

    if dst_port == 68 && super::dhcp::deliver(payload, ip_packet[16..20] == super::arp::our_ip()) {
        mib::inc(Mib::UdpInDatagrams);
        return;
    }

    // Deliver to the registered listener, in order, while its buffer has room.
    let mut listeners = LISTENERS.lock();
    if let Some(l) = listeners.iter_mut().find(|l| l.port == dst_port) {
        if l.queued + payload.len() > RCVBUF {
            mib::inc(Mib::UdpRcvbufErrors);
            mib::inc(Mib::UdpInErrors);
            return;
        }
        mib::inc(Mib::UdpInDatagrams);
        l.queued += payload.len();
        l.queue.push_back(Datagram { src_ip, src_port, data: payload.to_vec() });
        return;
    }
    drop(listeners);
    // Nobody was home: a reply that arrives one microsecond after its waiter
    // gave up, or on a port whose listener was never registered. Its sender
    // learns so, as from `__udp4_lib_rcv`, if it was sent to our address.
    if ip_packet[16..20] != super::arp::our_ip() {
        // A broadcast nobody listens for (`__udp4_lib_mcast_deliver`).
        mib::inc(Mib::UdpIgnoredMulti);
        return;
    }
    mib::inc(Mib::UdpNoPorts);
    {
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
    if let Some(l) = listeners.iter_mut().find(|l| l.port == sport) {
        l.err = Some((e.inner_dst(), dport));
    }
}

/// The hard ICMP error a listener's datagram drew, once.
pub fn take_error(port: u16) -> Option<([u8; 4], u16)> {
    LISTENERS.lock().iter_mut().find(|l| l.port == port).and_then(|l| l.err.take())
}


/// UDP checksum over the IPv4 pseudo-header + datagram (RFC 768). A computed
/// zero goes on the wire as 0xFFFF, because zero is the "no checksum" escape.
pub fn checksum(src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) -> u16 {
    use super::csum;
    let c = !csum::fold(csum::pseudo(src_ip, dst_ip, ipv4::PROTO_UDP, udp.len()) + csum::sum(udp));
    if c == 0 { 0xFFFF } else { c }
}

/// Send a UDP datagram. False when it could not go out: no route, larger
/// than the path MTU, or refused by the card (`udp_sendmsg`'s error).
pub fn send(dst_ip: [u8; 4], src_port: u16, dst_port: u16, payload: &[u8]) -> bool {
    send_from(super::arp::our_ip(), dst_ip, src_port, dst_port, payload)
}

/// `send` from `src_ip`.
pub fn send_from(src_ip: [u8; 4], dst_ip: [u8; 4], src_port: u16, dst_port: u16, payload: &[u8]) -> bool {
    let udp_len = (HEADER_LEN + payload.len()) as u16;
    let mut pkt = alloc::vec![0u8; udp_len as usize];

    pkt[0..2].copy_from_slice(&src_port.to_be_bytes());
    pkt[2..4].copy_from_slice(&dst_port.to_be_bytes());
    pkt[4..6].copy_from_slice(&udp_len.to_be_bytes());
    pkt[HEADER_LEN..].copy_from_slice(payload);
    // RFC 1122 4.1.3.4: checksumming defaults to on (Linux `udp_send_skb`).
    let c = checksum(src_ip, dst_ip, &pkt);
    pkt[6..8].copy_from_slice(&c.to_be_bytes());

    let sent = ipv4::send_from(src_ip, dst_ip, ipv4::PROTO_UDP, &pkt);
    if sent {
        mib::inc(Mib::UdpOutDatagrams);
    }
    sent
}

/// The oldest unread datagram on a port: (source address, source port, data).
pub fn recv(port: u16) -> Option<([u8; 4], u16, Vec<u8>)> {
    let mut listeners = LISTENERS.lock();
    let l = listeners.iter_mut().find(|l| l.port == port)?;
    let d = l.queue.pop_front()?;
    l.queued -= d.data.len();
    Some((d.src_ip, d.src_port, d.data))
}

/// Listen on a random free ephemeral port (RFC 6056 §3.3.1), for a client
/// that waits for one reply. The port is the reply's first secret: a fixed
/// one lets anyone aim forged answers at it. None when every port is taken.
pub fn listen_ephemeral() -> Option<u16> {
    let mut listeners = LISTENERS.lock();
    let free = |p: u16, ls: &Vec<UdpListener>| !NAT_PORTS.contains(&p) && !ls.iter().any(|l| l.port == p);
    let span = (*EPHEMERAL.end() - *EPHEMERAL.start()) as u32 + 1;
    let start = (crate::csprng::random_u64() % span as u64) as u32;
    for i in 0..span {
        let p = (*EPHEMERAL.start() as u32 + (start + i) % span) as u16;
        if free(p, &listeners) {
            listeners.push(new_listener(p));
            return Some(p);
        }
    }
    None
}

/// Stop listening on a port; what it had not read goes.
pub fn unlisten(port: u16) {
    LISTENERS.lock().retain(|l| l.port != port);
}
