# `kernel/src/net/arp.rs` @ 5e0102684

## L1-4 · `use spin::Mutex;`

```
//! ARP — Address Resolution Protocol
//!
//! Maps IPv4 addresses to MAC addresses.
//! Maintains a small in-memory ARP cache.
```

## L21-25 · `at: u64,`

```
/// Tick the mapping was last confirmed. An entry that never expires cannot
/// be wrong only once: it is wrong until the next boot. Symptom seen on the
/// device — everything through the gateway silently failed (DNS, TCP to the
/// internet) while LAN-direct traffic kept working, which is exactly what a
/// stale next-hop MAC looks like from the outside.
```

## L29-31 · `const ENTRY_TTL_TICKS: u64 = 3000; // 100 Hz → 30 s`

```
/// How long a learned mapping is trusted. Linux revalidates a reachable
/// neighbour after 30 s; we simply forget, which costs one ARP round trip on the
/// next use and cannot outlive a topology change (an AP hand-off in a mesh).
```

## L32 · `const ENTRY_TTL_TICKS: u64 = 3000; // 100 Hz → 30 s`

```
// 100 Hz → 30 s
```

## L38 · `static OUR_IP: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 15]); // QEMU user-mode default`

```
/// Our IP address (set during network init)
```

## L39 · `static OUR_IP: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 15]); // QEMU user-mode default`

```
// QEMU user-mode default
```

## L51 · `cache_insert(sender_ip, sender_mac);`

```
// Learn sender's MAC
```

## L57 · `let our_mac = netdev::mac().unwrap_or([0; 6]);`

```
// Send ARP reply
```

## L62 · `reply[4] = 6; // hardware size`

```
// hardware size
```

## L63 · `reply[5] = 4; // protocol size`

```
// protocol size
```

## L74 · `pub fn request(target_ip: [u8; 4]) {`

```
/// Send an ARP request for the given IP
```

## L87 · `pkt[18..24].copy_from_slice(&[0; 6]); // unknown target MAC`

```
// unknown target MAC
```

## L93-101 · `pub fn announce() {`

```
/// Announce our address on the current interface (gratuitous ARP: an ARP
/// request for our OWN address, so everyone on the segment refreshes the
/// IP→MAC binding).
///
/// The address is global state while the MAC belongs to whichever interface is
/// active. Switching from wired to WiFi keeps the address and silently changes
/// the MAC — but the router still has the old one cached and keeps sending our
/// traffic to an interface we are no longer listening on, for as long as its
/// ARP entry lives. Announcing is what makes the handover take effect.
```

## L118 · `pkt[24..28].copy_from_slice(&our_ip); // target = ourselves`

```
// target = ourselves
```

## L122 · `pub fn lookup(ip: [u8; 4]) -> Option<[u8; 6]> {`

```
/// Lookup MAC for IP in ARP cache
```

## L128-130 · `e.valid = false;`

```
// Expired rather than refreshed in place: the next `resolve` re-asks,
// which is one round trip and the only way a mapping that changed
// underneath us can ever be corrected.
```

## L137-139 · `const RETRANS_TICKS: u64 = 5; // 50 ms`

```
/// Gap between ARP retransmits, in 100 Hz ticks. Linux waits a second between
/// probes; we retry inside a caller that is already blocked on the answer, so
/// the useful interval is one WiFi round trip, not one second.
```

## L140 · `const RETRANS_TICKS: u64 = 5; // 50 ms`

```
// 50 ms
```

## L142-154 · `pub fn resolve(ip: [u8; 4], timeout_ticks: u64) -> Option<[u8; 6]> {`

```
/// Resolve IP → MAC. On cache hit, returns immediately. On miss, sends an ARP
/// request every `RETRANS_TICKS` and polls the network stack until the reply
/// lands or the timeout (in 100 Hz ticks) elapses.
///
/// Retransmitting is the point. A single request is a coin flip over WiFi — the
/// request is a broadcast frame, the reply a unicast one, and losing either left
/// nothing to try again. The caller then sent its real packet to L2 broadcast,
/// which the gateway drops: the first DNS query after boot failed until
/// something else happened to warm the cache. Wired hid this for years.
///
/// MUST NOT be called while holding any network-stack lock (CONNECTIONS,
/// etc.) — `super::poll` dispatches through the same locks and would
/// deadlock.
```

## L175 · `if let Some(entry) = cache.iter_mut().find(|e| e.valid && e.ip == ip) {`

```
// Update existing or find empty slot
```

## L184-185 · `if let Some(entry) = cache.iter_mut().find(|e| !e.valid) {`

```
// Prefer a free slot, else the oldest — a full table of stale entries must
// not lock out the one address we actually need.
```

## L195-197 · `pub fn table() -> alloc::vec::Vec<([u8; 4], [u8; 6], u64)> {`

```
/// Every mapping we currently trust, for `net` to print: (ip, mac, age in
/// seconds). A wrong next hop is invisible from the outside — it looks like the
/// far end is down — so the table has to be readable.
```

