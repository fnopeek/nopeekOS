# `kernel/src/net/udp.rs` @ 5e0102684

## L1-3 · `use alloc::vec::Vec;`

```
//! UDP — User Datagram Protocol
//!
//! Stateless, unreliable transport. Foundation for DNS and DHCP.
```

## L11-12 · `const MAX_LISTENERS: usize = 8;`

```
/// Registered UDP listeners: (port, callback buffer)
/// Simple model: one listener per port, incoming data buffered.
```

## L40 · `let mut listeners = LISTENERS.lock();`

```
// Deliver to registered listener
```

## L53-55 · `NO_LISTENER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);`

```
// Nobody was home. A reply that arrives one microsecond after its waiter
// gave up, or on a port whose listener was never registered, is dropped
// here in silence — indistinguishable from a reply that never came.
```

## L60-62 · `pub fn no_listener_stats() -> (u32, u16) {`

```
/// Datagrams dropped because no listener was registered, and the last such
/// port. Read by the DNS failure path: "nothing arrived" and "it arrived and we
/// had already stopped listening" are different faults.
```

## L68-71 · `pub fn rx_total() -> u32 {`

```
/// Every UDP datagram that reached this layer, whoever it was for. Cumulative;
/// callers take a snapshot and report the DELTA, because "2 since boot" says
/// nothing about the lookup that just failed — which is exactly the mistake the
/// first version of this counter invited.
```

## L81 · `pub fn send(dst_ip: [u8; 4], src_port: u16, dst_port: u16, payload: &[u8]) {`

```
/// Send a UDP datagram
```

## L89 · `pkt[HEADER_LEN..].copy_from_slice(payload);`

```
// pkt[6..8] = checksum (0 = disabled for UDP over IPv4)
```

## L95 · `#[allow(dead_code)]`

```
/// Register a listener on a UDP port. Returns false if no slot available.
```

## L99 · `if listeners.iter().flatten().any(|l| l.port == port) { return true; }`

```
// Already listening?
```

## L115 · `pub fn recv(port: u16) -> Option<([u8; 4], u16, Vec<u8>)> {`

```
/// Check if data is available on a port. Returns (src_ip, src_port, data) or None.
```

## L128 · `pub fn unlisten(port: u16) {`

```
/// Stop listening on a port.
```

