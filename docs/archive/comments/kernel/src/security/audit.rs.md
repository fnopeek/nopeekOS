# `kernel/src/security/audit.rs` @ 5e0102684

## L1-12 · `use core::sync::atomic::{AtomicU64, Ordering};`

```
//! Audit Log
//!
//! Ring buffer of the capability operations worth keeping: create, revoke,
//! deny, expire. Oldest entries are overwritten when full.
//!
//! A capability check that PASSES is deliberately not an entry, only a
//! counter. It is the expected outcome of every gated host call, so recording
//! it wrapped the 1024-entry ring roughly a thousand times per hour — the
//! denials and grants the log exists for were gone within seconds, buried
//! under routine successes. It also put one global lock in front of every
//! host call on a six-core scheduler. Counting keeps the number and gives the
//! ring back to the events that carry information.
```

## L24-25 · `static CHECKS_PASSED: AtomicU64 = AtomicU64::new(0);`

```
/// Capability checks that passed. Relaxed: a running total nobody orders
/// against, on the hottest path in the system.
```

## L68-69 · `v.resize(MAX_ENTRIES, AuditEntry {`

```
// Filler for the unwritten slots. `recent()` never returns them:
// it bounds the read by `total_count`.
```

## L110 · `pub fn record_check_passed() {`

```
/// A capability check passed. Counted, not recorded — see the module header.
```

