# `kernel/src/drivers/wifi.rs` @ 5e0102684

## L1-11 · `use alloc::collections::VecDeque;`

```
//! WiFi-class control channel (docs/spec/WIFI_CLASS_ABI.md)
//!
//! A kernel-mediated mailbox pair between the vendor WiFi driver
//! (`wifi_*.wasm`) and the device-independent supplicant (`wifid.wasm`).
//! The kernel routes opaque messages only — no WPA / vendor knowledge here
//! (thin transport-kernel). The bulk data path (normal traffic after
//! association) uses the existing netdev mailbox; this channel carries just
//! the control plane: scan / connect / key-install / EAPOL handshake frames.
//!
//!   downlink: manager → driver   (commands:  SCAN, CONNECT, SET_KEY, TX_EAPOL …)
//!   uplink:   driver  → manager  (events:    SCAN_AP, EAPOL_RX, LINK_UP …)
```

## L17 · `pub const WIFI_MSG_MAX: usize = 2048;`

```
/// Largest single control message (an EAPOL frame fits comfortably).
```

## L19-20 · `const WIFI_QUEUE_DEPTH: usize = 16;`

```
/// Bounded FIFO depth per direction. Control traffic is low-rate; if a side
/// stops draining, new messages are rejected rather than growing unbounded.
```

## L23 · `static DOWNLINK: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new()); // manager → driver`

```
// manager → driver
```

## L24 · `static UPLINK: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());   // driver → manager`

```
// driver → manager
```

## L26-29 · `static CMDS_SENT: AtomicU32 = AtomicU32::new(0);`

```
// Traffic counters. A rejected push is not a hiccup: these carry the 4-way
// handshake, and one lost EAPOL frame ends the association. An uplink that
// fills up means the manager stopped draining — which looks from the outside
// exactly like "WiFi connects but there is no DHCP lease".
```

## L37-42 · `static MGR_CORE: AtomicU32 = AtomicU32::new(0);`

```
// Core the manager (`wifid`) polls this channel from, +1, plus the tick of its
// last poll. It is a SECOND fiber beside the driver's, and just as load-bearing:
// without it no 4-way completes, the driver never reports authorized, and
// `netdev::send` then refuses every frame — host traffic as much as the guest's.
// The microvm keeps vCPUs off both cores; protecting only the driver's left this
// one sharing with a vCPU.
```

## L46 · `pub fn note_manager_core() {`

```
/// Called from the manager's own fiber, on every event poll.
```

## L53-56 · `pub fn manager_core() -> Option<usize> {`

```
/// The manager's core, if one has polled recently. Self-healing rather than
/// registered: a manager that exited simply stops refreshing, so a dead
/// supplicant does not cost the guest a vCPU for the rest of the boot. `1` maps
/// to Core 0, which never carries a vCPU anyway.
```

## L58 · `const STALE_TICKS: u64 = 200;`

```
// The manager polls every 4-50 ms, so anything past ~2 s is gone.
```

## L96 · `return false; // back-pressure: caller retries`

```
// back-pressure: caller retries
```

## L107 · `return None; // caller's buffer too small — leave it queued`

```
// caller's buffer too small — leave it queued
```

## L114-115 · `pub fn send_cmd(msg: &[u8]) -> bool {`

```
// ── Manager side (wifid.wasm) ──
/// Enqueue a command for the driver. Returns false if full / oversized.
```

## L121 · `pub fn poll_event(out: &mut [u8]) -> Option<usize> { pop(&UPLINK, out) }`

```
/// Dequeue the next event from the driver into `out`; None if empty / too small.
```

## L124-125 · `pub fn send_event(msg: &[u8]) -> bool {`

```
// ── Driver side (wifi_*.wasm) ──
/// Enqueue an event for the manager. Returns false if full / oversized.
```

## L132-133 · `static CMD_WAKER: core::sync::atomic::AtomicU32 =`

```
/// The driver fiber reading commands, and the manager fiber reading
/// events — registered when they wait (`npk_wait`), signalled on each push.
```

## L150 · `pub fn poll_cmd(out: &mut [u8]) -> Option<usize> { pop(&DOWNLINK, out) }`

```
/// Dequeue the next command from the manager into `out`; None if empty / too small.
```

## L153-155 · `pub fn reset() {`

```
/// Drop all queued messages — called when a driver or manager exits so a new
/// instance starts with an empty channel (stale commands/events don't leak
/// across a re-launch).
```

