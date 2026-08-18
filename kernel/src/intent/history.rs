//! Persistent intent history.
//!
//! One shared log at `.system/history`, encrypted at rest like every
//! other object. Loaded lazily on first use after unlock, rewritten
//! when a line is committed. Sessions seed from it, so what you typed
//! survives a reboot instead of dying with the window.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

/// Path of the encrypted history blob.
pub const HISTORY_OBJECT: &str = ".system/history";

/// Ring size of the stored log. Oldest lines fall out first.
const MAX_LINES: usize = 500;
/// What actually bounds the write: 500 pasted URLs would otherwise turn
/// every Enter into a 250 KB re-encrypt.
const MAX_BYTES: usize = 16 * 1024;

/// Markers that make a line too hot for disk — `store /sys/config/wifi_psk
/// <pass>` is the live example. Such lines stay in the session's ring (Up
/// still finds them) but never reach the store.
const SECRET_MARKERS: &[&str] = &["psk", "passwd", "password", "passphrase", "secret"];

struct Store {
    /// Oldest first.
    lines: Vec<String>,
    loaded: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store { lines: Vec::new(), loaded: false });

fn is_secret(line: &str) -> bool {
    let mut lower = String::with_capacity(line.len());
    for c in line.chars() { lower.extend(c.to_lowercase()); }
    SECRET_MARKERS.iter().any(|m| lower.contains(m))
}

/// Readable/writable only once identity is established — before that
/// npkFS has no key to decrypt with.
fn unlocked() -> bool {
    crate::npkfs::is_mounted() && crate::crypto::get_master_key().is_some()
}

fn trim(s: &mut Store) {
    if s.lines.len() > MAX_LINES {
        let drop = s.lines.len() - MAX_LINES;
        s.lines.drain(..drop);
    }
    let mut bytes: usize = s.lines.iter().map(|l| l.len() + 1).sum();
    let mut drop = 0;
    while bytes > MAX_BYTES && drop < s.lines.len() {
        bytes -= s.lines[drop].len() + 1;
        drop += 1;
    }
    if drop > 0 { s.lines.drain(..drop); }
}

fn load_locked(s: &mut Store) {
    if s.loaded || !unlocked() { return; }
    s.loaded = true;
    if let Ok((data, _)) = crate::npkfs::fetch(HISTORY_OBJECT) {
        if let Ok(text) = core::str::from_utf8(&data) {
            for line in text.lines() {
                if !line.is_empty() { s.lines.push(line.to_string()); }
            }
        }
    }
    trim(s);
}

fn serialize(s: &Store) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.lines.iter().map(|l| l.len() + 1).sum());
    for l in &s.lines {
        out.extend_from_slice(l.as_bytes());
        out.push(b'\n');
    }
    out
}

/// The stored lines, oldest first. Empty while the system is locked.
pub fn snapshot() -> Vec<String> {
    let mut s = STORE.lock();
    load_locked(&mut s);
    s.lines.clone()
}

/// Record a committed line. No-op for empty, duplicate and secret-bearing
/// lines, and while the system is locked.
pub fn push(line: &str) {
    if line.is_empty() || is_secret(line) || !unlocked() { return; }
    let blob = {
        let mut s = STORE.lock();
        load_locked(&mut s);
        if s.lines.last().map(|l| l.as_str()) == Some(line) { return; }
        s.lines.push(line.to_string());
        trim(&mut s);
        serialize(&s)
    };
    // Written outside the lock: the store write is milliseconds of crypto
    // plus disk, and a peer fiber pushing meanwhile would spin on it.
    let _ = crate::npkfs::upsert(HISTORY_OBJECT, &blob, crate::capability::CAP_NULL);
}

/// Drop the stored log. Returns how many lines went. The freed blob stays
/// on disk as an encrypted orphan until `gc` sweeps it.
pub fn clear() -> usize {
    let n = {
        let mut s = STORE.lock();
        load_locked(&mut s);
        let n = s.lines.len();
        s.lines.clear();
        s.loaded = true;
        n
    };
    let _ = crate::npkfs::delete(HISTORY_OBJECT);
    n
}
