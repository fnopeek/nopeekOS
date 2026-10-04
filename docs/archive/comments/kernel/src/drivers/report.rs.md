# `kernel/src/drivers/report.rs` @ 5e0102684

## L1-7 · `use alloc::string::{String, ToString};`

```
//! Driver self-reports.
//!
//! A bound WASM driver publishes a short plain-text status snapshot here
//! (`npk_driver_report`); an intent prints it back. The kernel stores bytes and
//! a timestamp and never parses the content — what a driver considers worth
//! reporting stays the driver's business, so this works for any device class
//! without vendor knowledge in the kernel.
```

## L13 · `pub const REPORT_MAX: usize = 4096;`

```
/// Largest snapshot a driver may publish.
```

## L16 · `const SLOTS: usize = 4;`

```
/// How many drivers can hold a slot at once.
```

## L27 · `pub fn store(name: &str, text: &str) {`

```
/// Replace `name`'s snapshot. Oldest slot is evicted when the table is full.
```

## L49 · `pub fn get(name: &str) -> Option<(String, u64)> {`

```
/// The snapshot published by `name`, with the tick-milliseconds it was stored.
```

## L55 · `pub fn names() -> Vec<String> {`

```
/// Every driver currently holding a slot, newest first.
```

## L64-65 · `pub fn clear(name: &str) {`

```
/// Drop a driver's slot — called when its module exits, so a dead driver's
/// numbers can't be read as live ones.
```

