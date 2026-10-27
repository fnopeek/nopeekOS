//! Topics a module can watch instead of polling.
//!
//! The source reports a change instead of panels polling for it: whoever
//! changes a topic (window list, battery, volume, config) calls
//! `notify(topic)`, and every fiber that waits on it (`npk_wait` with
//! `WAIT_STATE`) is signalled.
//!
//! A subscription is a (waker, pending topics) slot. It is taken the first
//! time a fiber waits and dropped when the fiber ends (`fiber::free_waker`
//! calls `forget`). The list grows with the watchers; `notify` signals them
//! after the lock is dropped.

use crate::smp::fiber::{self, Waker};

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
/// The status items changed (`status`), or the network link a kernel item
/// shows.
pub const TOPIC_STATUS: u32 = 1 << 4;

/// npkFS changed `path`: notify `TOPIC_CONFIG` if it is a config file.
pub fn path_changed(path: &str) {
    if path.trim_start_matches('/').starts_with("sys/config/") {
        notify(TOPIC_CONFIG);
    }
}

/// Watchers and the topics that changed for each since it last asked.
static SUBS: spin::Mutex<alloc::vec::Vec<(Waker, u32)>> = spin::Mutex::new(alloc::vec::Vec::new());

/// Make `w` a watcher. Idempotent.
pub fn subscribe(w: Waker) {
    let mut subs = SUBS.lock();
    if !subs.iter().any(|&(s, _)| s == w) {
        // A change before this point is not the watcher's to see; it
        // reads the current state after subscribing anyway.
        subs.push((w, 0));
    }
}

/// Drop `w`'s subscription (its fiber ended).
pub fn forget(w: Waker) {
    SUBS.lock().retain(|&(s, _)| s != w);
}

/// Take the topics that changed for `w` since it last asked. 0 = none.
pub fn take(w: Waker) -> u32 {
    SUBS.lock().iter_mut()
        .find(|(s, _)| *s == w)
        .map_or(0, |(_, p)| core::mem::take(p))
}

/// `topics` changed: mark them for every watcher and wake it.
pub fn notify(topics: u32) {
    let woken: alloc::vec::Vec<Waker> = {
        let mut subs = SUBS.lock();
        for (_, p) in subs.iter_mut() { *p |= topics; }
        subs.iter().map(|&(w, _)| w).collect()
    };
    for w in woken {
        fiber::signal(w, fiber::SIG_STATE);
    }
}
