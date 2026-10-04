# `kernel/src/notify.rs` @ 5e0102684

## L1-12 · `use core::sync::atomic::{AtomicU32, Ordering};`

```
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
```

## L18 · `pub const TOPIC_WINDOWS: u32 = 1 << 0;`

```
/// Windows: focus, titles, workspaces, the window list.
```

## L20 · `pub const TOPIC_BATTERY: u32 = 1 << 1;`

```
/// Battery state or charge (`battery::report`).
```

## L22 · `pub const TOPIC_VOLUME: u32 = 1 << 2;`

```
/// Master volume (`audio::set_volume`).
```

## L24-26 · `pub const TOPIC_CONFIG: u32 = 1 << 3;`

```
/// A file under `sys/config/` was written, deleted or moved (npkFS). Lets
/// a module pick up an edited config at once instead of re-reading it on
/// a timer.
```

## L29 · `pub fn path_changed(path: &str) {`

```
/// npkFS changed `path`: notify `TOPIC_CONFIG` if it is a config file.
```

## L47-48 · `pub fn subscribe(w: Waker) {`

```
/// Make `w` a watcher. Idempotent; a full table means `w` is simply not
/// woken by topics (its own deadline still ends the wait).
```

## L55-56 · `s.pending.store(0, Ordering::Release);`

```
// A change before this point is not the watcher's to see; it
// reads the current state after subscribing anyway.
```

## L63 · `pub fn forget(w: Waker) {`

```
/// Drop `w`'s subscription (its fiber ended).
```

## L73 · `pub fn take(w: Waker) -> u32 {`

```
/// Take the topics that changed for `w` since it last asked. 0 = none.
```

## L80 · `pub fn notify(topics: u32) {`

```
/// `topics` changed: mark them for every watcher and wake it.
```

