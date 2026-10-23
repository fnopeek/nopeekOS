//! WiFi-class control channel (docs/spec/WIFI_CLASS_ABI.md)
//!
//! A kernel-mediated mailbox pair between the vendor WiFi driver
//! (`wifi_*.wasm`) and the device-independent supplicant (`wifid.wasm`).
//! The kernel routes opaque messages only — no WPA / vendor knowledge here
//! (thin transport-kernel). The bulk data path (normal traffic after
//! association) uses the existing netdev mailbox; this channel carries just
//! the control plane: scan / connect / key-install / EAPOL handshake frames.
//!
//!   downlink: manager → driver   (commands:  SCAN, CONNECT, SET_KEY, TX_EAPOL …)
//!   uplink:   driver  → manager  (events:    SCAN_AP, EAPOL_RX, LINK_UP …)

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

/// Largest single control message (an EAPOL frame fits comfortably).
pub const WIFI_MSG_MAX: usize = 2048;
/// Bounded FIFO depth per direction. Control traffic is low-rate; if a side
/// stops draining, new messages are rejected rather than growing unbounded.
const WIFI_QUEUE_DEPTH: usize = 16;

static DOWNLINK: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new()); // manager → driver
static UPLINK: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());   // driver → manager

// Traffic counters. A rejected push is not a hiccup: these carry the 4-way
// handshake, and one lost EAPOL frame ends the association. An uplink that
// fills up means the manager stopped draining — which looks from the outside
// exactly like "WiFi connects but there is no DHCP lease".
static CMDS_SENT: AtomicU32 = AtomicU32::new(0);
static CMDS_DROPPED: AtomicU32 = AtomicU32::new(0);
static EVENTS_SENT: AtomicU32 = AtomicU32::new(0);
static EVENTS_DROPPED: AtomicU32 = AtomicU32::new(0);

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// Core the manager (`wifid`) polls this channel from, +1, plus the tick of its
// last poll. It is a SECOND fiber beside the driver's, and just as load-bearing:
// without it no 4-way completes, the driver never reports authorized, and
// `netdev::send` then refuses every frame — host traffic as much as the guest's.
// The microvm keeps vCPUs off both cores; protecting only the driver's left this
// one sharing with a vCPU.
static MGR_CORE: AtomicU32 = AtomicU32::new(0);
static MGR_SEEN: AtomicU64 = AtomicU64::new(0);

/// Called from the manager's own fiber, on every event poll.
pub fn note_manager_core() {
    // Its core is kept clear of vCPUs; the manager stays on it.
    crate::smp::fiber::pin_current();
    MGR_CORE.store(
        crate::smp::per_core::current_core_id() as u32 + 1, Ordering::Relaxed);
    MGR_SEEN.store(crate::interrupts::ticks(), Ordering::Relaxed);
}

/// The manager's core, if one has polled recently. Self-healing rather than
/// registered: a manager that exited simply stops refreshing, so a dead
/// supplicant does not cost the guest a vCPU for the rest of the boot. `1` maps
/// to Core 0, which never carries a vCPU anyway.
pub fn manager_core() -> Option<usize> {
    // The manager polls every 4-50 ms, so anything past ~2 s is gone.
    const STALE_TICKS: u64 = 200;
    let c = MGR_CORE.load(Ordering::Relaxed);
    if c <= 1 { return None; }
    let age = crate::interrupts::ticks()
        .wrapping_sub(MGR_SEEN.load(Ordering::Relaxed));
    if age >= STALE_TICKS { return None; }
    Some(c as usize - 1)
}

pub struct ChannelStats {
    pub cmds_sent: u32,
    pub cmds_dropped: u32,
    pub cmds_queued: usize,
    pub events_sent: u32,
    pub events_dropped: u32,
    pub events_queued: usize,
}

pub fn stats() -> ChannelStats {
    ChannelStats {
        cmds_sent: CMDS_SENT.load(Ordering::Relaxed),
        cmds_dropped: CMDS_DROPPED.load(Ordering::Relaxed),
        cmds_queued: DOWNLINK.lock().len(),
        events_sent: EVENTS_SENT.load(Ordering::Relaxed),
        events_dropped: EVENTS_DROPPED.load(Ordering::Relaxed),
        events_queued: UPLINK.lock().len(),
    }
}

fn push(q: &Mutex<VecDeque<Vec<u8>>>, msg: &[u8], sent: &AtomicU32, dropped: &AtomicU32) -> bool {
    if msg.is_empty() || msg.len() > WIFI_MSG_MAX {
        dropped.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let mut q = q.lock();
    if q.len() >= WIFI_QUEUE_DEPTH {
        dropped.fetch_add(1, Ordering::Relaxed);
        return false; // back-pressure: caller retries
    }
    q.push_back(msg.to_vec());
    sent.fetch_add(1, Ordering::Relaxed);
    true
}

fn pop(q: &Mutex<VecDeque<Vec<u8>>>, out: &mut [u8]) -> Option<usize> {
    let mut q = q.lock();
    let msg = q.front()?;
    if msg.len() > out.len() {
        return None; // caller's buffer too small — leave it queued
    }
    let msg = q.pop_front().unwrap();
    out[..msg.len()].copy_from_slice(&msg);
    Some(msg.len())
}

// ── Manager side (wifid.wasm) ──
/// Enqueue a command for the driver. Returns false if full / oversized.
pub fn send_cmd(msg: &[u8]) -> bool {
    let ok = push(&DOWNLINK, msg, &CMDS_SENT, &CMDS_DROPPED);
    if ok { wake(&CMD_WAKER); }
    ok
}
/// Dequeue the next event from the driver into `out`; None if empty / too small.
pub fn poll_event(out: &mut [u8]) -> Option<usize> { pop(&UPLINK, out) }

// ── Driver side (wifi_*.wasm) ──
/// Enqueue an event for the manager. Returns false if full / oversized.
pub fn send_event(msg: &[u8]) -> bool {
    let ok = push(&UPLINK, msg, &EVENTS_SENT, &EVENTS_DROPPED);
    if ok { wake(&EVENT_WAKER); }
    ok
}

/// The driver fiber reading commands, and the manager fiber reading
/// events — registered when they wait (`npk_wait`), signalled on each push.
static CMD_WAKER: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(crate::smp::fiber::NO_WAKER);
static EVENT_WAKER: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(crate::smp::fiber::NO_WAKER);

fn wake(slot: &core::sync::atomic::AtomicU32) {
    let w = slot.load(Ordering::Acquire);
    if w != crate::smp::fiber::NO_WAKER {
        crate::smp::fiber::signal(w, crate::smp::fiber::SIG_WIFI);
    }
}

pub fn set_cmd_waker(w: crate::smp::fiber::Waker) { CMD_WAKER.store(w, Ordering::Release); }
pub fn set_event_waker(w: crate::smp::fiber::Waker) { EVENT_WAKER.store(w, Ordering::Release); }
pub fn cmd_pending() -> bool { !DOWNLINK.lock().is_empty() }
/// Something for the manager: a driver event or a user request.
pub fn event_pending() -> bool { !UPLINK.lock().is_empty() || !REQUESTS.lock().is_empty() }
/// Dequeue the next command from the manager into `out`; None if empty / too small.
pub fn poll_cmd(out: &mut [u8]) -> Option<usize> { pop(&DOWNLINK, out) }

/// Drop all queued messages — called when a driver or manager exits so a new
/// instance starts with an empty channel (stale commands/events don't leak
/// across a re-launch).
pub fn reset() {
    DOWNLINK.lock().clear();
    UPLINK.lock().clear();
}

// ── User requests to the manager (docs/plan/WLAN_MANAGER.md §4) ──
//
// The `wlan` intent and the `link` tool ask; `wifid` decides. A password that comes
// with a request is stored in the vault here and never travels further:
// `wifid` reads it from the vault like every other known network.

pub const REQ_SCAN: u8 = 0x01;
pub const REQ_CONNECT: u8 = 0x02;
pub const REQ_DISCONNECT: u8 = 0x03;
pub const REQ_FORGET: u8 = 0x04;
/// The known networks changed (added, auto, priority); re-read them.
pub const REQ_KNOWN_CHANGED: u8 = 0x05;
/// From the UI only, never queued: `[op][ssid_len][ssid][on u8]`.
pub const REQ_SET_AUTO: u8 = 0x06;

static REQUESTS: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());
static REQS_SENT: AtomicU32 = AtomicU32::new(0);
static REQS_DROPPED: AtomicU32 = AtomicU32::new(0);

/// The manager's own view (state, link, scan list), as it last published it.
static STATUS: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Largest published status.
pub const STATUS_MAX: usize = 4096;

fn request(msg: &[u8]) -> bool {
    let ok = push(&REQUESTS, msg, &REQS_SENT, &REQS_DROPPED);
    if ok { wake(&EVENT_WAKER); }
    ok
}

/// Dequeue the next user request into `out`; None if empty / too small.
pub fn poll_request(out: &mut [u8]) -> Option<usize> { pop(&REQUESTS, out) }

pub fn publish_status(text: &[u8]) {
    {
        let mut s = STATUS.lock();
        if s.as_slice() == &text[..text.len().min(STATUS_MAX)] {
            return;
        }
        s.clear();
        s.extend_from_slice(&text[..text.len().min(STATUS_MAX)]);
    }
    // The `link` tool shows it; it waits on the status topic.
    crate::notify::notify(crate::notify::TOPIC_STATUS);
}

pub fn status() -> Vec<u8> { STATUS.lock().clone() }

/// An SSID is 0..=32 arbitrary bytes (802.11 §9.4.2.2).
pub const SSID_MAX: usize = 32;

fn ssid_msg(op: u8, ssid: &[u8]) -> Option<Vec<u8>> {
    if ssid.is_empty() || ssid.len() > SSID_MAX { return None; }
    let mut m = Vec::with_capacity(2 + ssid.len());
    m.push(op);
    m.push(ssid.len() as u8);
    m.extend_from_slice(ssid);
    Some(m)
}

pub fn request_scan() -> bool { request(&[REQ_SCAN]) }
pub fn request_disconnect() -> bool { request(&[REQ_DISCONNECT]) }

fn valid_psk(p: &[u8]) -> bool {
    (8..=63).contains(&p.len()) && p.iter().all(|&b| (0x20..0x7f).contains(&b))
}

/// Store a network's password (the vault GUI), keeping its options.
pub fn save_password(ssid: &[u8], psk: &[u8]) -> Result<(), &'static str> {
    if ssid.is_empty() || ssid.len() > SSID_MAX { return Err("invalid SSID"); }
    if !valid_psk(psk) { return Err("a WPA2 password is 8 to 63 printable characters"); }
    let mut e = known(ssid).unwrap_or_default();
    e.psk = psk.to_vec();
    store(ssid, &e)?;
    known_changed();
    Ok(())
}

/// Tell the manager the saved networks changed.
pub fn known_changed() { request(&[REQ_KNOWN_CHANGED]); }

/// Connect to `ssid`. A password given here is stored first (a new network,
/// or a changed password); without one the network must already be known.
pub fn request_connect(ssid: &[u8], psk: Option<&[u8]>) -> Result<(), &'static str> {
    let msg = ssid_msg(REQ_CONNECT, ssid).ok_or("invalid SSID")?;
    if let Some(p) = psk {
        if !valid_psk(p) {
            return Err("a WPA2 password is 8 to 63 printable characters");
        }
        let mut e = known(ssid).unwrap_or_default();
        e.psk = p.to_vec();
        store(ssid, &e)?;
    } else if known(ssid).is_none() {
        return Err("unknown network: a password is needed");
    }
    if request(&msg) { Ok(()) } else { Err("wifid is not taking requests") }
}

pub fn forget(ssid: &[u8]) -> Result<(), &'static str> {
    let msg = ssid_msg(REQ_FORGET, ssid).ok_or("invalid SSID")?;
    if !crate::security::keyvault::system_delete(&entry_name(ssid)) {
        return Err("not a known network");
    }
    request(&msg);
    Ok(())
}

pub fn set_auto(ssid: &[u8], on: bool) -> Result<(), &'static str> {
    let mut e = known(ssid).ok_or("not a known network")?;
    e.auto = on;
    store(ssid, &e)?;
    request(&[REQ_KNOWN_CHANGED]);
    Ok(())
}

pub fn set_prio(ssid: &[u8], prio: i8) -> Result<(), &'static str> {
    let mut e = known(ssid).ok_or("not a known network")?;
    e.prio = prio;
    store(ssid, &e)?;
    request(&[REQ_KNOWN_CHANGED]);
    Ok(())
}

// ── Known networks: vault system tier, `net:<SSID>` ──

/// A known network (docs/plan/WLAN_MANAGER.md §1).
pub struct KnownNet {
    pub auto: bool,
    pub prio: i8,
    /// 0 = the default from sys/config/wifi, 1 = 2.4 GHz only, 2 = 5 GHz only.
    pub band: u8,
    pub psk: Vec<u8>,
}

impl Default for KnownNet {
    fn default() -> Self { KnownNet { auto: true, prio: 0, band: 0, psk: Vec::new() } }
}

const NET_V1: u8 = 0x01;

impl KnownNet {
    /// v1 record, or a bare password (as the vault GUI first wrote them).
    pub fn decode(b: &[u8]) -> Option<KnownNet> {
        if b.first() != Some(&NET_V1) {
            return Some(KnownNet { psk: b.to_vec(), ..KnownNet::default() });
        }
        let (&flags, &prio, &band, &len) = (b.get(1)?, b.get(2)?, b.get(3)?, b.get(4)?);
        let psk = b.get(5..5 + len as usize)?;
        Some(KnownNet { auto: flags & 1 != 0, prio: prio as i8, band, psk: psk.to_vec() })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(5 + self.psk.len());
        v.extend_from_slice(&[NET_V1, self.auto as u8, self.prio as u8, self.band, self.psk.len() as u8]);
        v.extend_from_slice(&self.psk);
        v
    }
}

/// The vault entry name. The SSID is raw bytes; a name is a string, so a
/// non-UTF-8 SSID is hex-escaped byte by byte.
pub fn entry_name(ssid: &[u8]) -> alloc::string::String {
    let mut n = alloc::string::String::from("net:");
    match core::str::from_utf8(ssid) {
        Ok(s) if !s.contains('\\') => n.push_str(s),
        _ => {
            for &b in ssid {
                if b.is_ascii_graphic() && b != b'\\' || b == b' ' {
                    n.push(b as char);
                } else {
                    let _ = core::fmt::Write::write_fmt(&mut n, format_args!("\\x{:02x}", b));
                }
            }
        }
    }
    n
}

pub fn known(ssid: &[u8]) -> Option<KnownNet> {
    crate::security::keyvault::system_get(&entry_name(ssid)).and_then(|v| KnownNet::decode(&v))
}

fn store(ssid: &[u8], e: &KnownNet) -> Result<(), &'static str> {
    if crate::security::keyvault::system_set(&entry_name(ssid), &e.encode()) {
        Ok(())
    } else {
        Err("the vault could not store the network")
    }
}

/// Inverse of `entry_name` (without the `net:` prefix).
fn ssid_of(name: &str) -> Vec<u8> {
    let b = name.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'x') && i + 3 < b.len() && name.is_char_boundary(i + 4) {
            if let Ok(v) = u8::from_str_radix(&name[i + 2..i + 4], 16) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Every known network with its raw SSID.
pub fn known_all() -> Vec<(Vec<u8>, KnownNet)> {
    crate::security::keyvault::system_list()
        .into_iter()
        .filter_map(|n| {
            let ssid = ssid_of(n.strip_prefix("net:")?);
            let e = known(&ssid)?;
            Some((ssid, e))
        })
        .collect()
}

/// `known_all` for the manager: per network `[ssid_len u8][ssid][rec_len u8][v1 record]`.
pub fn known_wire() -> Vec<u8> {
    let mut out = Vec::new();
    for (ssid, e) in known_all() {
        if ssid.is_empty() || ssid.len() > SSID_MAX { continue; }
        let rec = e.encode();
        out.push(ssid.len() as u8);
        out.extend_from_slice(&ssid);
        out.push(rec.len() as u8);
        out.extend_from_slice(&rec);
    }
    out
}
