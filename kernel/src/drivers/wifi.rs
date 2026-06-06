//! WiFi-class control channel (WIFI_CLASS_ABI.md)
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

fn push(q: &Mutex<VecDeque<Vec<u8>>>, msg: &[u8]) -> bool {
    if msg.is_empty() || msg.len() > WIFI_MSG_MAX {
        return false;
    }
    let mut q = q.lock();
    if q.len() >= WIFI_QUEUE_DEPTH {
        return false; // back-pressure: caller retries
    }
    q.push_back(msg.to_vec());
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
pub fn send_cmd(msg: &[u8]) -> bool { push(&DOWNLINK, msg) }
/// Dequeue the next event from the driver into `out`; None if empty / too small.
pub fn poll_event(out: &mut [u8]) -> Option<usize> { pop(&UPLINK, out) }

// ── Driver side (wifi_*.wasm) ──
/// Enqueue an event for the manager. Returns false if full / oversized.
pub fn send_event(msg: &[u8]) -> bool { push(&UPLINK, msg) }
/// Dequeue the next command from the manager into `out`; None if empty / too small.
pub fn poll_cmd(out: &mut [u8]) -> Option<usize> { pop(&DOWNLINK, out) }

/// Drop all queued messages — called when a driver or manager exits so a new
/// instance starts with an empty channel (stale commands/events don't leak
/// across a re-launch).
pub fn reset() {
    DOWNLINK.lock().clear();
    UPLINK.lock().clear();
}
