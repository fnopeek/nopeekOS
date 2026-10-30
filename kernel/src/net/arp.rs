//! ARP — the kernel's side of `net_core::neigh`: the table behind a lock,
//! the frames it sends once the lock is let go, and the waits of callers
//! that need an address now.

use alloc::vec::Vec;
use spin::Mutex;
use net_core::neigh::{Out, Queued, Reply, Table};
use crate::netdev;
use super::eth;

const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16   = 2;
const HTYPE_ETH: u16   = 1;
const PTYPE_IPV4: u16  = 0x0800;

/// How a waiting IPv4 packet goes to the card once its next hop answers.
#[derive(Clone, Copy)]
pub enum Mode {
    /// Wait briefly for ring space.
    Wait,
    /// Refuse a full ring at once.
    Try,
    /// As `Try`, with the TCP or UDP checksum left to the card.
    Partial { tcp: bool },
}

/// An IPv4 packet on its way to a next hop.
pub struct Pkt {
    pub ip: Vec<u8>,
    pub mode: Mode,
}

impl Queued for Pkt {
    fn truesize(&self) -> usize { eth::HEADER_LEN + self.ip.len() }
}

static TABLE: Mutex<Table<Pkt>> = Mutex::new(Table::new());

/// Packets whose next hop never answered (Linux reports them with an ICMP
/// host unreachable to the sender; `ipv4` counts them).
static UNRESOLVED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Our IP address, 0.0.0.0 while unconfigured.
static OUR_IP: Mutex<[u8; 4]> = Mutex::new([0; 4]);

/// A new address: the neighbours learned under the old one go, as Linux's
/// `arp_ifdown` when the interface loses its address.
pub fn set_ip(ip: [u8; 4]) {
    let old = core::mem::replace(&mut *OUR_IP.lock(), ip);
    if old != ip {
        flush();
    }
}
pub fn our_ip() -> [u8; 4] { *OUR_IP.lock() }

fn ticks() -> u64 { crate::interrupts::ticks() }

/// Do what a call into the table left to do, after its lock is gone.
fn run(out: Out<Pkt>) -> Option<Result<(), crate::virtio_net::NetError>> {
    let mut last = None;
    for (target, dst) in out.solicit {
        send_request(target, dst.unwrap_or(eth::BROADCAST));
    }
    for (ha, p) in out.send {
        last = Some(match p.mode {
            Mode::Wait => eth::send_frame(&ha, eth::ETHERTYPE_IPV4, &p.ip),
            Mode::Try => eth::try_send_frame(&ha, eth::ETHERTYPE_IPV4, &p.ip),
            Mode::Partial { tcp } => eth::try_send_frame_csum(
                &ha, eth::ETHERTYPE_IPV4, &p.ip, ((p.ip[0] & 0x0f) as usize) * 4, tcp),
        });
    }
    UNRESOLVED.fetch_add(out.errors.len() as u64, core::sync::atomic::Ordering::Relaxed);
    last
}

/// Send `p` to the next hop `hop` (`ip_finish_output2`): at once when its
/// address is known, else once ARP answers. `Ok(true)` when it went to the
/// card, `Ok(false)` when it waits, the card's refusal otherwise.
pub fn output(hop: [u8; 4], p: Pkt) -> Result<bool, crate::virtio_net::NetError> {
    let mut out = Out::default();
    TABLE.lock().output(&mut out, ticks(), hop, p);
    let mine = !out.send.is_empty();
    let r = run(out);
    match r {
        Some(Err(e)) if mine => Err(e),
        _ => Ok(mine),
    }
}

/// `arp_rcv` for the frame `frame` (Ethernet header included).
pub fn handle_arp(frame: &[u8]) {
    super::dhcp::arp_input(&frame[eth::HEADER_LEN..]);
    let our_mac = netdev::mac().unwrap_or([0; 6]);
    let to_us = frame[0..6] == our_mac;
    let our_ip = our_ip();
    let mut out = Out::default();
    let reply = TABLE.lock().arp_input(&mut out, ticks(), &frame[eth::HEADER_LEN..], to_us, our_ip);
    if let Some(Reply::Answer { dst_ha, dst_ip }) = reply {
        let mut r = [0u8; 28];
        r[0..2].copy_from_slice(&HTYPE_ETH.to_be_bytes());
        r[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
        r[4] = 6; // hardware size
        r[5] = 4; // protocol size
        r[6..8].copy_from_slice(&ARP_REPLY.to_be_bytes());
        r[8..14].copy_from_slice(&our_mac);
        r[14..18].copy_from_slice(&our_ip);
        r[18..24].copy_from_slice(&dst_ha);
        r[24..28].copy_from_slice(&dst_ip);
        let _ = eth::send_frame(&dst_ha, eth::ETHERTYPE_ARP, &r);
    }
    run(out);
}

/// The table's timers (`neigh_timer_handler`) and its collection.
pub fn tick() {
    let mut out = Out::default();
    TABLE.lock().tick(&mut out, ticks(), crate::csprng::random_u64());
    run(out);
}

fn send_request(target_ip: [u8; 4], dst: [u8; 6]) {
    let our_mac = netdev::mac().unwrap_or([0; 6]);
    let our_ip = our_ip();

    let mut pkt = [0u8; 28];
    pkt[0..2].copy_from_slice(&HTYPE_ETH.to_be_bytes());
    pkt[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
    pkt[4] = 6;
    pkt[5] = 4;
    pkt[6..8].copy_from_slice(&ARP_REQUEST.to_be_bytes());
    pkt[8..14].copy_from_slice(&our_mac);
    pkt[14..18].copy_from_slice(&our_ip);
    pkt[18..24].copy_from_slice(&[0; 6]); // unknown target MAC
    pkt[24..28].copy_from_slice(&target_ip);

    let _ = eth::send_frame(&dst, eth::ETHERTYPE_ARP, &pkt);
}

/// Ask for `target_ip` now, by broadcast, and keep an entry for it so the
/// answer is taken. A caller that waits on the answer (`resolve`, a SYN
/// held for its next hop) asks again sooner than the table's one second,
/// since over WiFi either frame of an exchange can be lost.
pub fn request(target_ip: [u8; 4]) {
    let mut out = Out::default();
    let asked = TABLE.lock().resolve(&mut out, ticks(), target_ip);
    run(out);
    if !asked {
        send_request(target_ip, eth::BROADCAST);
    }
}

/// An ARP request by broadcast from `spa` for `tpa` (`arp_send_packet`):
/// an RFC 5227 probe when `spa` is 0.0.0.0, an announcement when both are
/// our address.
pub fn send_acd(spa: [u8; 4], tpa: [u8; 4]) {
    let our_mac = netdev::mac().unwrap_or([0; 6]);
    let mut pkt = [0u8; 28];
    pkt[0..2].copy_from_slice(&HTYPE_ETH.to_be_bytes());
    pkt[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
    pkt[4] = 6;
    pkt[5] = 4;
    pkt[6..8].copy_from_slice(&ARP_REQUEST.to_be_bytes());
    pkt[8..14].copy_from_slice(&our_mac);
    pkt[14..18].copy_from_slice(&spa);
    pkt[24..28].copy_from_slice(&tpa);
    let _ = eth::send_frame(&eth::BROADCAST, eth::ETHERTYPE_ARP, &pkt);
}

/// The link-layer address of `ip`, if it may be used.
pub fn lookup(ip: [u8; 4]) -> Option<[u8; 6]> {
    TABLE.lock().lookup(ip)
}

/// Gap between ARP retransmits, in 100 Hz ticks. Linux waits a second between
/// probes; we retry inside a caller that is already blocked on the answer, so
/// the useful interval is one WiFi round trip, not one second.
const RETRANS_TICKS: u64 = 5; // 50 ms

/// Resolve IP → MAC. On cache hit, returns immediately. On miss, sends an ARP
/// request every `RETRANS_TICKS` and polls the network stack until the reply
/// lands or the timeout (in 100 Hz ticks) elapses.
///
/// Must not be called while holding any network-stack lock (the TCP table,
/// etc.) — `super::poll` dispatches through the same locks and would
/// deadlock.
pub fn resolve(ip: [u8; 4], timeout_ticks: u64) -> Option<[u8; 6]> {
    if ip == [0; 4] { return None; }
    if let Some(mac) = lookup(ip) { return Some(mac); }
    let t0 = ticks();
    let mut next_try = t0;
    loop {
        let now = ticks();
        if now.wrapping_sub(t0) >= timeout_ticks { return None; }
        if now >= next_try {
            request(ip);
            next_try = now + RETRANS_TICKS;
        }
        super::poll();
        if let Some(mac) = lookup(ip) { return Some(mac); }
        core::hint::spin_loop();
    }
}

/// Forget every neighbour, and what waited for them: the link now leads to
/// another network (`neigh_changeaddr`, `neigh_carrier_down`).
pub fn flush() {
    TABLE.lock().flush();
}

/// Packets dropped for a next hop that never answered, a full queue or a
/// full table.
pub fn unresolved() -> u64 {
    UNRESOLVED.load(core::sync::atomic::Ordering::Relaxed) + TABLE.lock().drops
}

/// Every neighbour, for `net` to print: (ip, mac, state, seconds since its
/// last change). A wrong next hop is invisible from the outside — it looks
/// like the far end is down — so the table has to be readable.
pub fn table() -> Vec<([u8; 4], [u8; 6], &'static str, u64)> {
    TABLE.lock().entries(ticks()).into_iter()
        .map(|(ip, ha, nud, age)| (ip, ha, nud.name(), age))
        .collect()
}
