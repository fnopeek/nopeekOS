# `kernel/src/net/ipv4.rs` @ 5e0102684

## L1 · `use super::{eth, arp};`

```
//! IPv4 — Internet Protocol v4
```

## L9 · `const HEADER_LEN: usize  = 20; // no options`

```
// no options
```

## L11 · `static GATEWAY: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 2]); // QEMU default`

```
// QEMU default
```

## L26-29 · `pub fn arp_target_for(dst_ip: [u8; 4]) -> [u8; 4] {`

```
/// Pick the IP whose MAC the next-hop frame is addressed to: the destination
/// itself if it's link-local (or broadcast), otherwise the configured gateway.
/// Public so `tcp::connect` can pre-resolve the same hop ARP-wise before
/// taking any network-stack lock.
```

## L53-59 · `if crate::microvm::devices::nat::tap_inbound(&data[..total_len.min(data.len())]) {`

```
// The microvm tap gets first refusal, BEFORE the filter below. A guest flow
// is keyed on the address it went OUT with; asking "is this addressed to the
// address we happen to hold right now" first throws the reply away whenever
// that address has moved — a DHCP renewal, a carrier blink — and every live
// flow dies silently at this line. The tap's own test is "does it match a
// mapping", which is the question that has an answer. Cheap no-op when no
// VM is up.
```

## L64 · `let our_ip = arp::our_ip();`

```
// Accept packets for our IP, broadcast, or during DHCP (IP = 0.0.0.0)
```

## L78-80 · `pub fn send(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> bool {`

```
/// Send an IPv4 packet. Returns false if the next hop's MAC was unknown and the
/// frame went to L2 broadcast — most gateways drop that, so for the masquerade
/// it is a silent loss and the caller wants to count it.
```

## L85 · `pub fn send_with_ttl(dst_ip: [u8; 4], protocol: u8, payload: &[u8], ttl: u8) -> bool {`

```
/// Send an IPv4 packet with custom TTL (for traceroute)
```

## L99 · `let checksum = ipv4_checksum(&pkt[..HEADER_LEN]);`

```
// Checksum
```

## L103 · `pkt[HEADER_LEN..].copy_from_slice(payload);`

```
// Payload
```

## L106-112 · `let mut resolved = true;`

```
// Resolve next-hop MAC. On cache miss we fire an ARP request so the
// gateway responds and `super::poll` populates the cache before the
// caller's next retry — without it, every fresh-boot first packet
// (TCP SYN, DNS query, etc.) gets sent to L2 broadcast and silently
// dropped by most gateways. Active resolution is left to callers
// that can poll without holding network locks (see `arp::resolve`,
// used by `tcp::connect`).
```

## L116-118 · `eth::BROADCAST`

```
// A broadcast destination IS the broadcast MAC. Asking ARP who owns
// 255.255.255.255 put a nonsense request on the air before every DHCP
// packet, and the answer could only ever be "nobody".
```

