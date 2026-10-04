# `kernel/src/intent/history.rs` @ 5e0102684

## L1-6 · `use alloc::string::{String, ToString};`

```
//! Persistent intent history.
//!
//! One shared log at `.system/history`, encrypted at rest like every
//! other object. Loaded lazily on first use after unlock, rewritten
//! when a line is committed. Sessions seed from it, so what you typed
//! survives a reboot instead of dying with the window.
```

## L12 · `pub const HISTORY_OBJECT: &str = ".system/history";`

```
/// Path of the encrypted history blob.
```

## L15 · `const MAX_LINES: usize = 500;`

```
/// Ring size of the stored log. Oldest lines fall out first.
```

## L17-18 · `const MAX_BYTES: usize = 16 * 1024;`

```
/// What actually bounds the write: 500 pasted URLs would otherwise turn
/// every Enter into a 250 KB re-encrypt.
```

## L21-23 · `const SECRET_MARKERS: &[&str] = &["psk", "passwd", "password", "passphrase", "secret"];`

```
/// Markers that make a line too hot for disk — `store /sys/config/wifi_psk
/// <pass>` is the live example. Such lines stay in the session's ring (Up
/// still finds them) but never reach the store.
```

## L27 · `lines: Vec<String>,`

```
/// Oldest first.
```

## L40-41 · `fn unlocked() -> bool {`

```
/// Readable/writable only once identity is established — before that
/// npkFS has no key to decrypt with.
```

## L82 · `pub fn snapshot() -> Vec<String> {`

```
/// The stored lines, oldest first. Empty while the system is locked.
```

## L89-90 · `pub fn push(line: &str) {`

```
/// Record a committed line. No-op for empty, duplicate and secret-bearing
/// lines, and while the system is locked.
```

## L101-108 · `if crate::smp::scheduler::worker_count() == 0 {`

```
// Written outside the lock: the store write is milliseconds of crypto
// plus disk, and a peer fiber pushing meanwhile would spin on it.
//
// **And not on Core 0.** `push` runs on every Enter in the shell, and the
// shell runs on Core 0: the encryption and the disk write stood between
// the key press and the next frame. One writer task on a worker takes
// the NEWEST blob — two Enters in quick succession must not race two
// writes and land the older one last. (`docs/plan/CORES_AND_EVENTS.md`)
```

## L117 · `static PENDING: spin::Mutex<Option<alloc::vec::Vec<u8>>> = spin::Mutex::new(None);`

```
/// The newest serialized history waiting to be written.
```

## L119 · `static WRITER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);`

```
/// A writer task is queued or running.
```

## L129 · `if PENDING.lock().is_none() || WRITER.swap(true, Ordering::AcqRel) {`

```
// A push between the last take and the clear left its blob behind.
```

## L136-137 · `pub fn clear() -> usize {`

```
/// Drop the stored log. Returns how many lines went. The freed blob stays
/// on disk as an encrypted orphan until `gc` sweeps it.
```

