//! ICMP — Internet Control Message Protocol
//!
//! Answers echo requests, passes errors to the protocols they concern, and
//! carries the echo probes of `ping` and `traceroute`.

use super::ipv4;

const ECHO_REQUEST: u8 = 8;
const ECHO_REPLY: u8   = 0;

/// The echo probe in flight: to whom, with which identifier and sequence
/// number, and what came back for it.
struct Probe {
    dst: [u8; 4],
    id: u16,
    seq: u16,
    answer: Option<Answer>,
}

/// What came back for a probe.
#[derive(Clone, Copy)]
pub enum Answer {
    /// The echo reply, from the destination.
    Echo,
    /// A router on the way: its TTL ran out (traceroute's hop).
    TimeExceeded([u8; 4]),
    /// Destination Unreachable with `code`, from the router or host that
    /// said so.
    Unreachable([u8; 4], u8),
}

static PROBE: spin::Mutex<Option<Probe>> = spin::Mutex::new(None);

/// What came back for the probe in flight, once.
pub fn answer() -> Option<Answer> {
    PROBE.lock().as_mut().and_then(|p| p.answer.take())
}

/// Note `a` for the probe in flight if it is `(dst, id, seq)`, as a ping
/// socket matches (`ping_rcv`): anything else, a late reply to an earlier
/// probe or a stranger's, does not count.
fn credit(dst: [u8; 4], id: u16, seq: u16, a: Answer) {
    if let Some(p) = PROBE.lock().as_mut() {
        if p.dst == dst && p.id == id && p.seq == seq {
            p.answer = Some(a);
        }
    }
}

pub fn handle_icmp(ip_packet: &[u8], data: &[u8]) {
    if data.len() < 8 { return; }

    let icmp_type = data[0];
    let src_ip = <[u8; 4]>::try_from(&ip_packet[12..16]).unwrap();

    // icmp_unreach: an error about a datagram of ours goes to its protocol.
    if let Some(e) = net_core::ip::icmp_error(data, super::arp::our_ip()) {
        match e.inner_proto() {
            ipv4::PROTO_TCP => super::tcp::icmp_error(&e),
            ipv4::PROTO_UDP => super::udp::icmp_error(&e),
            // About one of our echo probes: the quoted request names it.
            ipv4::PROTO_ICMP if e.transport()[0] == ECHO_REQUEST => {
                let t = e.transport();
                let id = u16::from_be_bytes([t[4], t[5]]);
                let seq = u16::from_be_bytes([t[6], t[7]]);
                let a = if e.kind == net_core::ip::ICMP_TIME_EXCEEDED {
                    Answer::TimeExceeded(src_ip)
                } else {
                    Answer::Unreachable(src_ip, e.code)
                };
                credit(e.inner_dst(), id, seq, a);
            }
            _ => {}
        }
    }

    match icmp_type {
        // Unconfigured, a reply would carry 0.0.0.0 as its source.
        ECHO_REQUEST if super::arp::our_ip() != [0; 4] => send_echo_reply(src_ip, data),
        ECHO_REPLY => {
            let id = u16::from_be_bytes([data[4], data[5]]);
            let seq = u16::from_be_bytes([data[6], data[7]]);
            credit(src_ip, id, seq, Answer::Echo);
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

    let _ = ipv4::send(dst_ip, ipv4::PROTO_ICMP, &reply);
}

/// Send an echo request to `dst_ip` with identifier `id`, sequence `seq`
/// and TTL `ttl`, and wait for what comes back for it (`answer`).
pub fn probe(dst_ip: [u8; 4], id: u16, seq: u16, ttl: u8) {
    let mut pkt = [0u8; 64];
    pkt[0] = ECHO_REQUEST;
    pkt[1] = 0;   // code
    pkt[4..6].copy_from_slice(&id.to_be_bytes());
    pkt[6..8].copy_from_slice(&seq.to_be_bytes());

    // Fill payload with pattern
    for i in 8..64 {
        pkt[i] = i as u8;
    }

    // Checksum
    let checksum = icmp_checksum(&pkt);
    pkt[2..4].copy_from_slice(&checksum.to_be_bytes());

    *PROBE.lock() = Some(Probe { dst: dst_ip, id, seq, answer: None });
    let _ = ipv4::send_with_ttl(dst_ip, ipv4::PROTO_ICMP, &pkt, ttl);
}

/// The probe is over; later answers go nowhere.
pub fn probe_done() {
    *PROBE.lock() = None;
}

fn icmp_checksum(data: &[u8]) -> u16 {
    !super::csum::fold(super::csum::sum(data))
}
