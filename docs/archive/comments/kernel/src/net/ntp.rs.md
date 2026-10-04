# `kernel/src/net/ntp.rs` @ 5e0102684

## L1-4 · `use spin::Mutex;`

```
//! NTP — Network Time Protocol (SNTP client)
//!
//! Simple NTP query to get wall-clock time.
//! Uses UDP port 123, parses NTP v4 response.
```

## L11 · `const NTP_EPOCH_OFFSET: u64 = 2_208_988_800; // seconds from 1900 to 1970`

```
// seconds from 1900 to 1970
```

## L13 · `static WALL_CLOCK: Mutex<Option<(u64, u64)>> = Mutex::new(None); // (unix_secs, tick_at_sync)`

```
/// Stored wall-clock time: Unix timestamp at the tick when it was set
```

## L14 · `static WALL_CLOCK: Mutex<Option<(u64, u64)>> = Mutex::new(None); // (unix_secs, tick_at_sync)`

```
// (unix_secs, tick_at_sync)
```

## L16 · `pub fn set_time(unix_secs: u64) {`

```
/// Set wall clock directly (e.g. from RTC).
```

## L22 · `pub fn sync_via_dns(hostname: &str) -> bool {`

```
/// Resolve NTP server hostname via DNS, then sync.
```

## L27 · `sync([10, 0, 2, 3])`

```
// Fallback: QEMU user-mode gateway
```

## L32 · `pub fn sync(server_ip: [u8; 4]) -> bool {`

```
/// Sync time from an NTP server. Blocking.
```

## L34 · `let mut req = [0u8; 48];`

```
// Build SNTP request (48 bytes)
```

## L36 · `req[0] = 0x23; // LI=0, Version=4, Mode=3 (client)`

```
// LI=0, Version=4, Mode=3 (client)
```

## L38-39 · `let _ = super::arp::resolve(super::ipv4::arp_target_for(server_ip), 10);`

```
// Warm the next hop's MAC — see `dns::resolve`: a blind spin pays its
// timeout even when the entry is already cached.
```

## L52 · `let secs = u32::from_be_bytes([data[40], data[41], data[42], data[43]]) as u64;`

```
// Transmit timestamp at offset 40 (seconds since 1900-01-01)
```

## L63 · `if crate::interrupts::ticks() - t0 > 300 { break; } // 3s timeout`

```
// 3s timeout
```

## L71-72 · `pub fn unix_time() -> Option<u64> {`

```
/// Get current Unix timestamp (seconds since 1970-01-01).
/// Returns None if NTP hasn't synced yet.
```

## L77 · `let elapsed_secs = (now_tick - base_tick) / 100; // 100Hz ticks`

```
// 100Hz ticks
```

## L81-82 · `pub fn format_time(unix: u64) -> alloc::string::String {`

```
/// Format Unix timestamp as local time using timezone config.
/// Shows "YYYY-MM-DD HH:MM:SS UTC+N" or "UTC" if no offset.
```

