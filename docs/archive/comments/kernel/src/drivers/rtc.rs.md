# `kernel/src/drivers/rtc.rs` @ 5e0102684

## L1-4 · `use crate::serial::{inb, outb};`

```
//! RTC — Real-Time Clock (CMOS 0x70/0x71)
//!
//! Reads the hardware clock as fallback time source.
//! BCD-encoded registers, no IRQ — just polled reads.
```

## L11 · `const REG_SECONDS: u8 = 0x00;`

```
// CMOS register indices
```

## L22 · `unsafe {`

```
// SAFETY: CMOS ports are standard x86 I/O, always present.
```

## L24 · `outb(CMOS_ADDR, reg | 0x80); // bit 7 = disable NMI`

```
// bit 7 = disable NMI
```

## L33 · `fn wait_ready() {`

```
/// Wait until the RTC update-in-progress bit clears.
```

## L35 · `while read_cmos(REG_STATUS_A) & 0x80 != 0 {`

```
// Spin until bit 7 of Status Register A is 0
```

## L41-42 · `pub fn read_unix_time() -> Option<u64> {`

```
/// Read current time from CMOS RTC and return as Unix timestamp.
/// Returns None only if the clock reads nonsensical values.
```

## L44 · `loop {`

```
// Read twice and compare to avoid torn reads during update
```

## L79 · `let year = 2000u64 + year_raw as u64;`

```
// CMOS year is 0-99, assume 2000+
```

## L85 · `}`

```
// Values changed during read, retry
```

## L89 · `fn datetime_to_unix(year: u64, month: u64, day: u64, hour: u64, min: u64, sec: u64) -> u64 {`

```
/// Convert date/time to Unix timestamp (seconds since 1970-01-01 00:00:00 UTC).
```

## L91 · `let mut days = 0u64;`

```
// Days from 1970 to start of given year
```

## L97 · `let month_days: [u64; 12] = if is_leap(year) {`

```
// Days from start of year to start of given month
```

