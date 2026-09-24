//! Topics a module can watch instead of polling.
//!
//! The panels used to ask every few hundred milliseconds whether anything
//! changed — the clock, the window list, the battery, the volume — and
//! almost always nothing had. Here the SOURCE reports a change: whoever
//! changes a topic calls `notify(topic)`, and every fiber that waits on it
//! (`npk_wait` with `WAIT_STATE`) is signalled. `docs/plan/CORES_AND_EVENTS.md`
//! §3.4 "Panels abonnieren statt abfragen".
//!
//! A subscription is a (waker, pending topics) slot. It is taken the first
//! time a fiber waits and dropped when the fiber ends (`fiber::free_waker`
//! calls `forget`). Lock-free: `notify` may come from any core.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::smp::fiber::{self, Waker, NO_WAKER};

/// Windows: focus, titles, workspaces, the window list.
pub const TOPIC_WINDOWS: u32 = 1 << 0;
/// Battery state or charge (`battery::report`).
pub const TOPIC_BATTERY: u32 = 1 << 1;
/// Master volume (`audio::set_volume`).
pub const TOPIC_VOLUME: u32 = 1 << 2;
/// A file under `sys/config/` was written, deleted or moved (npkFS). Lets
/// a module pick up an edited config at once instead of re-reading it on
/// a timer.
pub const TOPIC_CONFIG: u32 = 1 << 3;

/// npkFS changed `path`: notify `TOPIC_CONFIG` if it is a config file.
pub fn path_changed(path: &str) {
    if path.trim_start_matches('/').starts_with("sys/config/") {
        notify(TOPIC_CONFIG);
    }
}

const MAX_SUBS: usize = 32;

struct Sub {
    waker: AtomicU32,
    pending: AtomicU32,
}

static SUBS: [Sub; MAX_SUBS] = [const {
    Sub { waker: AtomicU32::new(NO_WAKER), pending: AtomicU32::new(0) }
}; MAX_SUBS];

/// Make `w` a watcher. Idempotent; a full table means `w` is simply not
/// woken by topics (its own deadline still ends the wait).
pub fn subscribe(w: Waker) {
    if SUBS.iter().any(|s| s.waker.load(Ordering::Acquire) == w) {
        return;
    }
    for s in SUBS.iter() {
        if s.waker.compare_exchange(NO_WAKER, w, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            // A change before this point is not the watcher's to see; it
            // reads the current state after subscribing anyway.
            s.pending.store(0, Ordering::Release);
            return;
        }
    }
}

/// Drop `w`'s subscription (its fiber ended).
pub fn forget(w: Waker) {
    for s in SUBS.iter() {
        if s.waker.load(Ordering::Acquire) == w {
            s.pending.store(0, Ordering::Relaxed);
            s.waker.store(NO_WAKER, Ordering::Release);
        }
    }
}

/// Take the topics that changed for `w` since it last asked. 0 = none.
pub fn take(w: Waker) -> u32 {
    SUBS.iter()
        .find(|s| s.waker.load(Ordering::Acquire) == w)
        .map_or(0, |s| s.pending.swap(0, Ordering::AcqRel))
}

/// `topics` changed: mark them for every watcher and wake it.
pub fn notify(topics: u32) {
    for s in SUBS.iter() {
        let w = s.waker.load(Ordering::Acquire);
        if w != NO_WAKER {
            s.pending.fetch_or(topics, Ordering::AcqRel);
            fiber::signal(w, fiber::SIG_STATE);
        }
    }
}
