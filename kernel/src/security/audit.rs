//! Audit journal
//!
//! What every holder of a token did with it: issued (launch), released
//! (exit), refused, handed a file by the user, files touched, hosts
//! reached, camera, mic and screen on and off, settings changed.
//!
//! A holder is the module a token was issued to; the journal names it and
//! numbers its instances. A capability check that passes is only counted
//! (per vault slot, in `capability`), not journalled: it is the expected
//! outcome of every gated host call, and an entry per check would flush
//! everything else out within seconds and put a lock on the hottest path.
//!
//! A repeat of the same (instance, kind, object) among the newest
//! `FOLD_WINDOW` entries raises that entry's count instead of adding one.
//!
//! The journal names files and hosts; reading it takes `Rights::AUDIT`.
//! Nothing here calls into the vault: the vault journals while it holds its
//! lock, so the order is always vault, then journal.

use core::sync::atomic::{AtomicU64, Ordering};

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::capability::{CapId, Rights};

/// Entries kept. A bound that protects the kernel, not a budget: folding
/// keeps a normal day far below it.
const MAX_ENTRIES: usize = 4096;
/// How far back a repeat is folded into an existing entry.
const FOLD_WINDOW: usize = 64;
/// Longest object kept, in bytes; a longer one is cut at a char boundary.
const MAX_OBJECT: usize = 160;
const MAX_HOLDER: usize = 32;

static LOG: Mutex<Journal> = Mutex::new(Journal::new());
/// Capability checks that passed. Relaxed: a running total nobody orders
/// against, on the hottest path in the system.
static CHECKS_PASSED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A token was issued; `rights` holds what it grants.
    Launch,
    /// The token was released; `checks` and `denied` sum up the instance.
    Exit,
    /// A gate refused; `rights` is what it asked for, `object` the call.
    Denied,
    /// The user handed a path over (open dialog, launch on a file).
    Grant,
    Read,
    Write,
    Create,
    Delete,
    Rename,
    Mkdir,
    /// An outbound connection; `object` is the host.
    Net,
    /// A device turned on for this holder; `object` names it.
    DeviceOn,
    DeviceOff,
    /// A system setting changed; `object` is the key.
    Setting,
    /// A token was revoked by another holder.
    Revoke,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Launch => "launch",
            Kind::Exit => "exit",
            Kind::Denied => "DENIED",
            Kind::Grant => "grant",
            Kind::Read => "read",
            Kind::Write => "write",
            Kind::Create => "create",
            Kind::Delete => "delete",
            Kind::Rename => "rename",
            Kind::Mkdir => "mkdir",
            Kind::Net => "net",
            Kind::DeviceOn => "on",
            Kind::DeviceOff => "off",
            Kind::Setting => "setting",
            Kind::Revoke => "revoke",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Position in the journal since boot, never reused.
    pub seq: u64,
    /// UTC seconds of the first occurrence, 0 while the clock is unset.
    pub wall: u64,
    /// Uptime in ms of the first and of the latest occurrence.
    pub up_ms: u64,
    pub last_up_ms: u64,
    /// Instance number of the holder in this boot; 0 = the kernel itself.
    pub inst: u32,
    pub holder: String,
    pub kind: Kind,
    pub rights: Rights,
    /// Occurrences folded into this entry, at least 1.
    pub count: u32,
    pub object: String,
    /// `Exit` only: checks passed and refusals over the instance's life.
    pub checks: u64,
    pub denied: u32,
}

struct Holder {
    cap: CapId,
    inst: u32,
    name: String,
    denied: u32,
}

struct Journal {
    entries: VecDeque<Entry>,
    holders: Vec<Holder>,
    next_seq: u64,
    next_inst: u32,
}

impl Journal {
    const fn new() -> Self {
        Journal { entries: VecDeque::new(), holders: Vec::new(), next_seq: 0, next_inst: 1 }
    }

    fn holder(&self, cap: &CapId) -> (u32, String) {
        match self.holders.iter().find(|h| h.cap == *cap) {
            Some(h) => (h.inst, h.name.clone()),
            None => (0, String::from("kernel")),
        }
    }

    fn push(&mut self, mut e: Entry) {
        if self.entries.len() >= MAX_ENTRIES { self.entries.pop_front(); }
        e.seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(e);
    }

    /// Record `kind` on `object` for `cap`, folding a recent repeat.
    fn note(&mut self, cap: &CapId, kind: Kind, rights: Rights, object: &str, t: Stamp) {
        let (inst, holder) = self.holder(cap);
        let object = cut(object, MAX_OBJECT);
        let start = self.entries.len().saturating_sub(FOLD_WINDOW);
        for i in (start..self.entries.len()).rev() {
            let e = &mut self.entries[i];
            if e.inst == inst && e.kind == kind && e.rights == rights
                && e.holder == holder && e.object == object
            {
                e.count = e.count.saturating_add(1);
                e.last_up_ms = t.up_ms;
                return;
            }
        }
        self.push(Entry {
            seq: 0, wall: t.wall, up_ms: t.up_ms, last_up_ms: t.up_ms,
            inst, holder, kind, rights, count: 1, object, checks: 0, denied: 0,
        });
    }
}

#[derive(Clone, Copy)]
struct Stamp { wall: u64, up_ms: u64 }

/// Read the clocks before the journal lock, so it is never held across them.
fn stamp() -> Stamp {
    Stamp { wall: crate::time::wall::now_or_zero(), up_ms: crate::time::uptime_us() / 1000 }
}

fn cut(s: &str, max: usize) -> String {
    if s.len() <= max { return String::from(s); }
    let mut end = max;
    while !s.is_char_boundary(end) { end -= 1; }
    String::from(&s[..end])
}

/// The microVM guest's id in the journal, outside the vault's random
/// ones. The guest holds no token: its device model bounds it, and the
/// journal records what crosses that boundary (camera, mic, screen).
pub const GUEST: CapId = [0xff; 32];

/// The guest `name` booted.
pub fn guest_started(name: &str) {
    released(&GUEST, 0);
    let n = alloc::format!("{} (VM)", name);
    issued(&GUEST, &n, Rights::empty());
}

/// The guest is gone.
pub fn guest_ended() {
    released(&GUEST, 0);
}

/// A token was issued to `name`: it becomes a holder with a new instance
/// number, and the journal gets its `Launch`.
pub fn issued(cap: &CapId, name: &str, rights: Rights) {
    let t = stamp();
    let mut j = LOG.lock();
    let inst = j.next_inst;
    j.next_inst = j.next_inst.wrapping_add(1).max(1);
    let name = cut(name, MAX_HOLDER);
    j.holders.push(Holder { cap: *cap, inst, name: name.clone(), denied: 0 });
    j.push(Entry {
        seq: 0, wall: t.wall, up_ms: t.up_ms, last_up_ms: t.up_ms,
        inst, holder: name, kind: Kind::Launch, rights, count: 1,
        object: String::new(), checks: 0, denied: 0,
    });
}

/// The token of `cap` was released after `checks` passed checks. The
/// holder ends here; a recycled id starts a new instance.
pub fn released(cap: &CapId, checks: u64) {
    let t = stamp();
    let mut j = LOG.lock();
    let Some(pos) = j.holders.iter().position(|h| h.cap == *cap) else { return };
    let h = j.holders.swap_remove(pos);
    j.push(Entry {
        seq: 0, wall: t.wall, up_ms: t.up_ms, last_up_ms: t.up_ms,
        inst: h.inst, holder: h.name, kind: Kind::Exit, rights: Rights::empty(),
        count: 1, object: String::new(), checks, denied: h.denied,
    });
}

/// A gate refused `cap` the `rights` that `call` needs; `why` is the
/// vault's reason when it is more than missing rights.
pub fn denied(cap: &CapId, rights: Rights, call: &str, why: &str) {
    let t = stamp();
    let mut j = LOG.lock();
    if let Some(h) = j.holders.iter_mut().find(|h| h.cap == *cap) {
        h.denied = h.denied.saturating_add(1);
    }
    if why.is_empty() {
        j.note(cap, Kind::Denied, rights, call, t);
    } else {
        let obj = alloc::format!("{} ({})", call, why);
        j.note(cap, Kind::Denied, rights, &obj, t);
    }
}

/// `cap` did `kind` on `object` (a path, a host, a device, a key).
pub fn record(cap: &CapId, kind: Kind, object: &str) {
    let t = stamp();
    LOG.lock().note(cap, kind, Rights::empty(), object, t);
}

/// The user handed `path` with `rights` to `cap`.
pub fn granted(cap: &CapId, path: &str, rights: Rights) {
    let t = stamp();
    LOG.lock().note(cap, Kind::Grant, rights, path, t);
}

/// A capability check passed. Counted, not journalled; see the module header.
pub fn record_check_passed() {
    CHECKS_PASSED.fetch_add(1, Ordering::Relaxed);
}

pub fn checks_passed() -> u64 {
    CHECKS_PASSED.load(Ordering::Relaxed)
}

/// Entries after `after_seq` (all with `None`), newest last, at most `max`
/// of the newest, holder name filtered when `holder` is not empty.
pub fn since(after_seq: Option<u64>, holder: &str, max: usize) -> Vec<Entry> {
    let j = LOG.lock();
    let mut out: Vec<Entry> = j.entries.iter().rev()
        .filter(|e| after_seq.is_none_or(|s| e.seq > s))
        .filter(|e| holder.is_empty() || e.holder == holder)
        .take(max)
        .cloned()
        .collect();
    out.reverse();
    out
}

/// Entries first written or folded into at or after uptime `since_ms`,
/// oldest first: a reader that keeps them by `seq` stays current with
/// one call per refresh.
pub fn touched_since(since_ms: u64) -> Vec<Entry> {
    LOG.lock().entries.iter().filter(|e| e.last_up_ms >= since_ms).cloned().collect()
}

/// Entries written since boot, folded ones counted once.
pub fn total_count() -> u64 {
    LOG.lock().next_seq
}

/// Name and instance of every live holder.
pub fn holders() -> Vec<(u32, String, CapId)> {
    LOG.lock().holders.iter().map(|h| (h.inst, h.name.clone(), h.cap)).collect()
}
