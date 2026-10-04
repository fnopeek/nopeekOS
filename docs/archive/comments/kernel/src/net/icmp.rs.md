# `kernel/src/net/icmp.rs` @ 5e0102684

## L1-3 · `use crate::kprintln;`

```
//! ICMP — Internet Control Message Protocol
//!
//! Handles ping (echo request/reply).
```

## L14 · `pub fn ttl_expired_from() -> Option<[u8; 4]> {`

```
/// Check if a TTL-expired ICMP was received (for traceroute)
```

## L32 · `}`

```
// Time Exceeded (TTL expired in transit) — used by traceroute
```

## L49 · `reply[1] = 0; // code`

```
// code
```

## L51 · `reply[2] = 0;`

```
// Recalculate ICMP checksum
```

## L60 · `pub fn ping(dst_ip: [u8; 4], seq: u16) {`

```
/// Send a ping (echo request) to the given IP
```

## L64 · `pkt[1] = 0;   // code`

```
// code
```

## L65 · `pkt[4] = 0;   // identifier high`

```
// identifier high
```

## L66 · `pkt[5] = 1;   // identifier low`

```
// identifier low
```

## L69 · `for i in 8..64 {`

```
// Fill payload with pattern
```

## L74 · `let checksum = icmp_checksum(&pkt);`

```
// Checksum
```

## L83 · `pub fn ping_ttl(dst_ip: [u8; 4], seq: u16, ttl: u8) {`

```
/// Send ICMP echo with custom TTL (for traceroute)
```

