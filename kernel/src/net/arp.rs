//! ARP — Address Resolution Protocol
//!
//! Maps IPv4 addresses to MAC addresses.
//! Maintains a small in-memory ARP cache.

use spin::Mutex;
use crate::netdev;
use super::eth;

const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16   = 2;
const HTYPE_ETH: u16   = 1;
const PTYPE_IPV4: u16  = 0x0800;

const CACHE_SIZE: usize = 16;

struct ArpEntry {
    ip: [u8; 4],
    mac: [u8; 6],
    valid: bool,
    /// Tick the mapping was last confirmed. An entry that never expires stays
    /// wrong until the next boot; a stale next-hop MAC breaks everything
    /// through the gateway while LAN-direct traffic keeps working.
    at: u64,
    /// Tick a stale entry was first used again, 0 while fresh. From then on
    /// it is re-asked until confirmed or given up.
    probe_since: u64,
    /// Tick of the last revalidation request.
    asked: u64,
}

/// How long a learned mapping is trusted without asking (Linux:
/// `base_reachable_time`, 30 s). After that the entry is stale, as in
/// Linux's `NUD_STALE`: it still carries traffic, and its next use starts a
/// revalidation. Forgetting it instead would send that packet to L2
/// broadcast, where the gateway drops it.
const ENTRY_TTL_TICKS: u64 = 3000; // 100 Hz → 30 s

/// How long a stale entry in use stays usable without an answer (Linux:
/// `delay_first_probe_time` 5 s + `ucast_probes` 3 × `retrans_time` 1 s).
/// Then it is dropped, so a mapping that changed underneath us (an AP
/// hand-off in a mesh) is learned anew.
const PROBE_WINDOW_TICKS: u64 = 800; // 8 s

/// Gap between revalidation requests for a stale entry in use.
const PROBE_RETRANS_TICKS: u64 = 100; // 1 s

static CACHE: Mutex<[ArpEntry; CACHE_SIZE]> = Mutex::new(
    [const { ArpEntry { ip: [0; 4], mac: [0; 6], valid: false, at: 0, probe_since: 0, asked: 0 } }; CACHE_SIZE]
);

/// Our IP address (set during network init)
static OUR_IP: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 15]); // QEMU user-mode default

pub fn set_ip(ip: [u8; 4]) { *OUR_IP.lock() = ip; }
pub fn our_ip() -> [u8; 4] { *OUR_IP.lock() }

pub fn handle_arp(data: &[u8]) {
    if data.len() < 28 { return; }
    let op = u16::from_be_bytes([data[6], data[7]]);
    let sender_mac = <[u8; 6]>::try_from(&data[8..14]).unwrap();
    let sender_ip = <[u8; 4]>::try_from(&data[14..18]).unwrap();
    let target_ip = <[u8; 4]>::try_from(&data[24..28]).unwrap();

    // Learn sender's MAC
    cache_insert(sender_ip, sender_mac);

    let our_ip = *OUR_IP.lock();

    if op == ARP_REQUEST && target_ip == our_ip {
        // Send ARP reply
        let our_mac = netdev::mac().unwrap_or([0; 6]);
        let mut reply = [0u8; 28];
        reply[0..2].copy_from_slice(&HTYPE_ETH.to_be_bytes());
        reply[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
        reply[4] = 6; // hardware size
        reply[5] = 4; // protocol size
        reply[6..8].copy_from_slice(&ARP_REPLY.to_be_bytes());
        reply[8..14].copy_from_slice(&our_mac);
        reply[14..18].copy_from_slice(&our_ip);
        reply[18..24].copy_from_slice(&sender_mac);
        reply[24..28].copy_from_slice(&sender_ip);

        let _ = eth::send_frame(&sender_mac, eth::ETHERTYPE_ARP, &reply);
    }
}

/// Send an ARP request for the given IP
pub fn request(target_ip: [u8; 4]) {
    let our_mac = netdev::mac().unwrap_or([0; 6]);
    let our_ip = *OUR_IP.lock();

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

    let _ = eth::send_frame(&eth::BROADCAST, eth::ETHERTYPE_ARP, &pkt);
}

/// Announce our address on the current interface (gratuitous ARP: an ARP
/// request for our own address, so everyone on the segment refreshes the
/// IP→MAC binding).
///
/// The address is global state while the MAC belongs to whichever interface is
/// active. Switching from wired to WiFi keeps the address and silently changes
/// the MAC — but the router still has the old one cached and keeps sending our
/// traffic to an interface we are no longer listening on, for as long as its
/// ARP entry lives. Announcing is what makes the handover take effect.
pub fn announce() {
    let our_ip = *OUR_IP.lock();
    if our_ip == [0, 0, 0, 0] {
        return;
    }
    let our_mac = netdev::mac().unwrap_or([0; 6]);

    let mut pkt = [0u8; 28];
    pkt[0..2].copy_from_slice(&HTYPE_ETH.to_be_bytes());
    pkt[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
    pkt[4] = 6;
    pkt[5] = 4;
    pkt[6..8].copy_from_slice(&ARP_REQUEST.to_be_bytes());
    pkt[8..14].copy_from_slice(&our_mac);
    pkt[14..18].copy_from_slice(&our_ip);
    pkt[18..24].copy_from_slice(&[0; 6]);
    pkt[24..28].copy_from_slice(&our_ip); // target = ourselves
    let _ = eth::send_frame(&eth::BROADCAST, eth::ETHERTYPE_ARP, &pkt);
}

/// Lookup MAC for IP in ARP cache. A stale entry is returned and re-asked
/// for at most once per `PROBE_RETRANS_TICKS`; one unconfirmed for
/// `PROBE_WINDOW_TICKS` is dropped.
pub fn lookup(ip: [u8; 4]) -> Option<[u8; 6]> {
    let now = crate::interrupts::ticks();
    let (mac, ask) = {
        let mut cache = CACHE.lock();
        let e = cache.iter_mut().find(|e| e.valid && e.ip == ip)?;
        if now.saturating_sub(e.at) <= ENTRY_TTL_TICKS {
            return Some(e.mac);
        }
        if e.probe_since == 0 {
            e.probe_since = now.max(1);
        } else if now.saturating_sub(e.probe_since) > PROBE_WINDOW_TICKS {
            e.valid = false;
            return None;
        }
        let due = e.asked == 0 || now.saturating_sub(e.asked) >= PROBE_RETRANS_TICKS;
        if due {
            e.asked = now.max(1);
        }
        (e.mac, due)
    };
    if ask {
        request(ip);
    }
    Some(mac)
}

/// Gap between ARP retransmits, in 100 Hz ticks. Linux waits a second between
/// probes; we retry inside a caller that is already blocked on the answer, so
/// the useful interval is one WiFi round trip, not one second.
const RETRANS_TICKS: u64 = 5; // 50 ms

/// Resolve IP → MAC. On cache hit, returns immediately. On miss, sends an ARP
/// request every `RETRANS_TICKS` and polls the network stack until the reply
/// lands or the timeout (in 100 Hz ticks) elapses.
///
/// Retransmitting matters over WiFi: the request is a broadcast frame and the
/// reply a unicast one, and either can be lost. Without a reply the caller's
/// packet goes to L2 broadcast, which the gateway drops.
///
/// Must not be called while holding any network-stack lock (CONNECTIONS,
/// etc.) — `super::poll` dispatches through the same locks and would
/// deadlock.
pub fn resolve(ip: [u8; 4], timeout_ticks: u64) -> Option<[u8; 6]> {
    if let Some(mac) = lookup(ip) { return Some(mac); }
    let t0 = crate::interrupts::ticks();
    let mut next_try = t0;
    loop {
        let now = crate::interrupts::ticks();
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

fn cache_insert(ip: [u8; 4], mac: [u8; 6]) {
    let now = crate::interrupts::ticks();
    let mut cache = CACHE.lock();
    // Update existing or find empty slot
    if let Some(entry) = cache.iter_mut().find(|e| e.valid && e.ip == ip) {
        if entry.mac != mac {
            crate::kprintln!("[npk] arp: {}.{}.{}.{} moved to a new MAC", ip[0], ip[1], ip[2], ip[3]);
        }
        entry.mac = mac;
        entry.at = now;
        entry.probe_since = 0;
        entry.asked = 0;
        return;
    }
    // Prefer a free slot, else the oldest — a full table of stale entries must
    // not lock out the one address we actually need.
    if let Some(entry) = cache.iter_mut().find(|e| !e.valid) {
        *entry = ArpEntry { ip, mac, valid: true, at: now, probe_since: 0, asked: 0 };
        return;
    }
    if let Some(entry) = cache.iter_mut().min_by_key(|e| e.at) {
        *entry = ArpEntry { ip, mac, valid: true, at: now, probe_since: 0, asked: 0 };
    }
}

/// Every mapping we currently trust, for `net` to print: (ip, mac, age in
/// seconds). A wrong next hop is invisible from the outside — it looks like the
/// far end is down — so the table has to be readable.
pub fn table() -> alloc::vec::Vec<([u8; 4], [u8; 6], u64)> {
    let now = crate::interrupts::ticks();
    let cache = CACHE.lock();
    cache.iter().filter(|e| e.valid)
        .map(|e| (e.ip, e.mac, now.saturating_sub(e.at) / 100))
        .collect()
}
