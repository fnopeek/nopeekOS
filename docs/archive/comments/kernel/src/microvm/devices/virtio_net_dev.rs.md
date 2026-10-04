# `kernel/src/microvm/devices/virtio_net_dev.rs` @ 5e0102684

## L1-30 · `#![allow(dead_code)]`

```
//! virtio-net device (modern, virtio 1.2) — clean port of the QEMU/Linux
//! reference, replacing the hand-rolled `virtio_net_pci.rs`.
//!
//! What the old device got wrong (and this one does right, 1:1 from
//! `drivers/net/virtio_net.c` + `drivers/virtio/virtio_ring.c`):
//!
//!   * **Mergeable RX buffers** (`VIRTIO_NET_F_MRG_RXBUF`, the QEMU default).
//!     The guest posts single-descriptor buffers (`virtqueue_add_inbuf ...
//!     num_sg=1`); a frame is delivered across N of them, `num_buffers=N` in the
//!     first buffer's header. The old device used `GUEST_TSO4` → "big packets"
//!     mode → each RX buffer a ~19-descriptor page chain → only ~queue_size/19
//!     buffers fit (shallow ring, burst starvation) AND a 19-descriptor walk +
//!     19-page skb reconstruction per frame. Mergeable = deep ring (one buffer
//!     per descriptor) + cheap inject + cheap guest skb.
//!
//!   * **EVENT_IDX** (`VIRTIO_RING_F_EVENT_IDX`). The guest publishes
//!     `used_event` (interrupt me only when used.idx crosses this) in
//!     `avail->ring[size]`; the device publishes `avail_event` (kick me only
//!     when avail.idx crosses this) in `used->ring[size]`. This replaces the
//!     binary NO_INTERRUPT/NO_NOTIFY flags with precise suppression →
//!     dramatically fewer guest IRQs (the EOI storm) and, on RX, zero repost
//!     doorbells (the device polls, so it sets avail_event far ahead).
//!
//!   * **Batched used-ring updates.** A frame's N used entries are written, then
//!     `used.idx` is published once (one release fence), then ONE interrupt
//!     decision — not a fence+ISR per descriptor.
//!
//! The synthetic gateway (ARP/DNS/NAT/GRO/TX-GSO) still lives in `super::nat`;
//! this file owns only the wire-level device. TX (incl. TX-GSO segmentation via
//! `nat::tap_outbound`/`emit_tcp_out`) is unchanged in spirit, ported here.
```

## L40 · `const VIRTIO_VENDOR:     u32 = 0x1AF4;`

```
// ── PCI identity / BAR / capability layout (unchanged — proven enumeration) ──
```

## L65 · `const CC_DEVICE_FEATURE_SELECT: u32 = 0x00;`

```
// Common Cfg register offsets (virtio 1.2 §4.1.4.3).
```

## L86 · `const VIRTIO_NET_F_CSUM:        u32 = 0;   // device handles TX checksum`

```
// ── Feature bits ──
```

## L87 · `const VIRTIO_NET_F_CSUM:        u32 = 0;   // device handles TX checksum`

```
// device handles TX checksum
```

## L88 · `const VIRTIO_NET_F_GUEST_CSUM:  u32 = 1;   // guest handles RX checksum`

```
// guest handles RX checksum
```

## L90 · `const VIRTIO_NET_F_GUEST_TSO4:  u32 = 7;   // guest accepts GSO/TSO4 RX`

```
// guest accepts GSO/TSO4 RX
```

## L92 · `const VIRTIO_NET_F_HOST_TSO4:   u32 = 11;  // guest may send TSO4 (we segment)`

```
// guest may send TSO4 (we segment)
```

## L93 · `const VIRTIO_NET_F_MRG_RXBUF:   u32 = 15;  // mergeable RX buffers (THE big one)`

```
// mergeable RX buffers (THE big one)
```

## L95 · `const VIRTIO_F_VERSION_1:       u32 = 32;  // bit 0 of feature word 1`

```
// bit 0 of feature word 1
```

## L96 · `const VIRTIO_RING_F_EVENT_IDX:  u32 = 29;  // used_event / avail_event`

```
// used_event / avail_event
```

## L98-100 · `const FEAT_LO: u32 =`

```
// Low feature word (bits 0..31) we advertise. Mergeable + GSO both directions +
// EVENT_IDX. NOT HOST_TSO6 (NAT is IPv4-only) and NOT INDIRECT_DESC (TX walker
// doesn't handle indirect tables yet).
```

## L111 · `const FEAT_HI: u32 = 1; // VIRTIO_F_VERSION_1`

```
// High feature word (bits 32..63): only VERSION_1 (bit 32 = bit 0 here).
```

## L112 · `const FEAT_HI: u32 = 1; // VIRTIO_F_VERSION_1`

```
// VIRTIO_F_VERSION_1
```

## L117 · `const NUM_QUEUES: u16 = 2;           // q0 = RX, q1 = TX`

```
// q0 = RX, q1 = TX
```

## L118 · `const MAX_QUEUE_SIZE: u16 = 1024;    // QEMU default depth; mergeable = 1 buf/desc`

```
// QEMU default depth; mergeable = 1 buf/desc
```

## L120-121 · `const VNET_HDR_LEN: usize = 12;`

```
// virtio-net header is 12 bytes under VERSION_1 (virtio_net_hdr_v1, incl.
// num_buffers @ offset 10). `nat` builds frames with this 12-byte prefix.
```

## L125 · `const VRING_DESC_F_NEXT:  u16 = 1;`

```
// Split-ring descriptor flags (virtio 1.2 §2.7.5).
```

## L128 · `const VRING_AVAIL_F_NO_INTERRUPT: u16 = 1;`

```
// Ring flags (used when EVENT_IDX is NOT negotiated).
```

## L132 · `#[derive(Default, Clone, Copy)]`

```
// ── Split virtqueue ───────────────────────────────────────────────────────
```

## L139 · `driver_lo: u32, driver_hi: u32,   // avail ring`

```
// avail ring
```

## L140 · `device_lo: u32, device_hi: u32,   // used ring`

```
// used ring
```

## L141 · `last_avail_idx: u16,              // next avail entry the device will consume`

```
// next avail entry the device will consume
```

## L142 · `used_idx: u16,                    // device's running used.idx`

```
// device's running used.idx
```

## L143-145 · `last_irq_used_idx: u16,`

```
/// `used.idx` value at the last interrupt we raised — the `old_idx` for
/// `vring_need_event`. Lets us interrupt only when used.idx crosses the
/// guest's `used_event` since we last told it.
```

## L147-150 · `signalled_used_valid: bool,`

```
/// QEMU's `signalled_used_valid`: false until the first notify decision on
/// this queue. Without it there is no guaranteed first interrupt after a
/// reset — the first crossing depends on the guest's zeroed `used_event`
/// happening to land inside the window.
```

## L161 · `struct Desc { addr: u64, len: u32, flags: u16, next: u16 }`

```
/// One split-ring descriptor.
```

## L178 · `mem.read_u16(avail_gpa + 2)   // avail: flags(2) idx(2) ring[]`

```
// avail: flags(2) idx(2) ring[]
```

## L188 · `#[inline]`

```
/// `used_event` (guest's interrupt threshold) lives at `avail->ring[size]`.
```

## L194-195 · `#[inline]`

```
/// Write one used-ring element at slot `(used_base_idx) % size` WITHOUT
/// advancing used.idx (the publish is batched). used: flags(2) idx(2) ring[].
```

## L202 · `#[inline]`

```
/// Publish a new used.idx (release fence so the element writes settle first).
```

## L208 · `#[inline]`

```
/// `avail_event` (device's kick threshold) lives at `used->ring[size]`.
```

## L214-215 · `#[inline]`

```
/// vring_need_event (virtio_ring.h): true iff `new_idx` has reached/passed the
/// guest's `event_idx` threshold since `old_idx`. Wrapping u16 arithmetic.
```

## L221 · `pub struct VirtioNet {`

```
// ── Device ────────────────────────────────────────────────────────────────
```

## L238 · `tx_scratch: alloc::vec::Vec<u8>,`

```
/// Reusable TX read buffer (grown once; a GSO super-frame is ≤64 KiB).
```

## L240 · `tx_notify_off: bool,`

```
/// TX doorbell off while the worker polls the ring (vhost_disable_notify).
```

## L294 · `pub fn pci_read_dword(&self, reg: u8) -> u32 {`

```
// ── PCI config space (identical layout to the proven device) ──
```

## L305 · `0x3C => 0x0000_010A,   // IRQ line 10, INTA`

```
// IRQ line 10, INTA
```

## L322 · `0x88 => 0x11 | ((super::net_backend::msix_msgctl() as u32) << 16),`

```
// MSI-X capability (ID 0x11): table + PBA in BAR0.
```

## L345 · `pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {`

```
// ── MMIO dispatch (off is BAR0-relative, computed by the run loop) ──
```

## L353 · `super::net_backend::take_isr() as u64 & width_mask(width)`

```
// ISR status is read-to-clear.
```

## L423-429 · `kprintln!(`

```
// GRO exists ONLY on this path. The AMD backend uses
// `tap_inbound`, which is pure: no coalescing, no
// staging queue. So every super-frame we assemble here has
// never run anywhere but on Intel hardware, and it is the
// one place inbound where we take what Linux is about to
// receive, rebuild it, and put our own caps on it.
//
```

## L456-457 · `super::net_backend::set_queue_vector(0, 0xFFFF);`

```
// Queue vectors are virtio state (the MSI-X table itself is PCI state
// and survives a device reset).
```

## L496 · `pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {`

```
// ── Queue servicing (called from the MMIO-notify VM-exit) ──
```

## L500-502 · `0 => { self.arm_rx_no_kick(mem); false }`

```
// RX kick: the guest added buffers. We poll RX (via inject_rx from the
// pump), so we don't service here — but suppress further RX kicks by
// pushing avail_event far ahead (EVENT_IDX). Nothing to deliver now.
```

## L508-510 · `fn arm_rx_no_kick(&mut self, mem: &GuestMem) {`

```
/// With EVENT_IDX, tell the guest NOT to kick the RX queue (we poll it): set
/// avail_event past the current avail.idx by ~the whole ring. Without
/// EVENT_IDX, fall back to the used.flags NO_NOTIFY bit.
```

## L516 · `set_avail_event(mem, q.used_gpa(), q.size, top.wrapping_add(q.size));`

```
// A target the guest's avail.idx won't reach for a full ring → no kick.
```

## L519 · `mem.write_u16(q.used_gpa(), VRING_USED_F_NO_NOTIFY);`

```
// used.flags |= NO_NOTIFY
```

## L524-530 · `pub fn inject_rx(&mut self, mem: &GuestMem, payload: &[u8]) -> bool {`

```
// ── RX: deliver one frame into the guest, mergeable buffers ──
/// Inject one full frame (payload incl. 12-byte vnet header). Consumes as
/// many single-descriptor RX buffers as the frame needs, writes one used
/// element per buffer, sets `num_buffers=N` in the first buffer's header,
/// and publishes used.idx once. Does NOT raise the interrupt — the caller
/// batches that via `rx_should_interrupt`. Returns false (rolling back) if
/// the guest hasn't posted enough buffers for the whole frame.
```

## L537 · `if avail_top == start_avail { return false; }   // no buffers at all`

```
// no buffers at all
```

## L547-549 · `q.last_avail_idx = start_avail;`

```
// Out of buffers mid-frame → can't deliver a partial frame.
// Roll back: used.idx was never published, so just un-consume the
// avail entries. Caller requeues the frame and retries.
```

## L560 · `q.last_avail_idx = start_avail; return false;   // malformed (driver-readable)`

```
// malformed (driver-readable)
```

## L571 · `mem.write_u16(first_addr + NUM_BUFFERS_OFF, nbuf);`

```
// num_buffers (LE u16) in the first buffer's vnet header.
```

## L573 · `q.used_idx = start_used.wrapping_add(nbuf);`

```
// Publish used.idx (+N) with a release fence.
```

## L580-583 · `pub fn rx_should_interrupt(&mut self, mem: &GuestMem) -> bool {`

```
/// After a drain pass: should we raise IRQ10 for the RX queue? EVENT_IDX →
/// only when used.idx has crossed the guest's used_event since our last IRQ;
/// else honour the avail.flags NO_INTERRUPT bit. Records the threshold so the
/// next decision uses the right `old_idx`.
```

## L587-603 · `fence(Ordering::SeqCst);`

```
// QEMU virtio_should_notify opens with `smp_mb()` and the comment "We
// need to expose used array entries before checking used event." That
// barrier was not ported, and on x86 it is the ONE reordering the
// hardware allows: our store of used.idx may still sit in the store
// buffer while the load of used_event below executes. The guest does its
// half correctly (store used_event; mfence; load used.idx), so a missing
// fence on our side is a textbook Dekker miss — both sides read stale and
// neither wakes the other.
//
// Ordinarily a lost notification is a hiccup. Here it is terminal,
// because `last_irq_used_idx` below only moves FORWARD: once it has
// passed the guest's used_event, `need_event` is false for every future
// check, and the guest can only move used_event from inside NAPI, which
// only runs on an interrupt. Nothing in the device re-opens that loop —
// and with the RX ring drained, nothing is injected either, so the
// decision is never even re-evaluated. That is the absorbing state:
// works, then one handshake is missed, then silence.
```

## L607-611 · `let old = q.last_irq_used_idx;`

```
// QEMU virtio_should_notify: `old = signalled_used; signalled_used =
// used_idx; need_event(used_event, used_idx, old)`. signalled_used is
// updated on EVERY check, NOT only when we fire — else last_irq runs
// ahead of the guest's used_event and need_event stays false forever
// (no IRQ → idle NAPI never wakes → network hangs after a while).
```

## L616-619 · `!valid || need_event(ev, q.used_idx, old)`

```
// `!v || need_event(...)`, QEMU's exact expression. The first
// decision after a reset has no meaningful `old`, so it must always
// notify — otherwise the very first crossing can be swallowed and
// there is no second chance.
```

## L626-627 · `pub fn rx_wants_irq(&mut self, mem: &GuestMem) -> bool {`

```
/// Back-compat name used by the pump: true iff the guest currently wants an
/// RX interrupt. Delegates to the EVENT_IDX-aware decision.
```

## L632-633 · `pub fn rx_avail_count(&self, mem: &GuestMem) -> u64 {`

```
/// RX buffers the guest has posted (avail - consumed). With mergeable each is
/// one descriptor, so this is the true depth (no /19 big-packets penalty).
```

## L643-648 · `fn service_tx(&mut self, mem: &GuestMem) -> bool {`

```
// ── TX: read guest frames, segment GSO, forward; batched used + EVENT_IDX ──
/// Combined TX service for the INLINE callers (Intel/VMX + AMD non-full):
/// drain the ring (cheap, device-touching), emit the segments, finish. AMD
/// full mode does NOT use this — the worker calls the three pieces below with
/// the expensive `process_tx` emit run OUTSIDE the device mutex (the TX half
/// of the v0.226.63 ACK-jitter fix; see net_dataplane::service_full).
```

## L662-666 · `pub fn drain_tx_payloads(&mut self, mem: &GuestMem) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {`

```
/// Phase 2a (under the device mutex, CHEAP): walk the guest TX avail ring,
/// copy each frame's bytes into an owned Vec, publish the used ring. No
/// segmentation / checksum / host-NIC send here — those are the expensive
/// part and run lock-free in the caller (`nat::tap_outbound`). Returns one
/// owned payload per consumed frame (len == frames consumed = "advanced").
```

## L691-694 · `loop {`

```
// EVENT_IDX "process, then arm notification, then re-check" loop:
// after draining we publish avail_event = last consumed avail.idx so
// the guest kicks on the NEXT frame; we re-read avail.idx to catch a
// frame that landed in the arming window (else TX stalls after one).
```

## L703-705 · `let d = match read_desc(mem, q.desc_gpa(), idx, q.size) {`

```
// A broken chain pushes what we have so far as if it were
// a whole frame. Nothing downstream can tell a truncated
// packet from a short one, so say it here.
```

## L715 · `payloads.push(frame[..off].to_vec());   // own it; emit lock-free later`

```
// own it; emit lock-free later
```

## L717 · `used_fill(mem, q.used_gpa(), q.size, start_used.wrapping_add(nused), head, 0);`

```
// TX buffers are device-read-only → used len = 0 (virtio spec).
```

## L723-724 · `let target = if notify_off {`

```
// Arm: kick me when avail.idx passes what I've consumed — or,
// while the worker polls, not before a full ring.
```

## L747-750 · `pub fn tx_set_notify(&mut self, mem: &GuestMem, on: bool) -> bool {`

```
/// vhost_disable_notify / vhost_enable_notify for the TX ring. Off: the
/// guest does not kick until a full ring is queued — the worker is polling
/// and reads the ring itself. On: kick on the next frame, then re-check;
/// `true` = a frame landed while it was off, so do not park yet.
```

## L766-773 · `pub fn tx_finish(&mut self, mem: &GuestMem, advanced: bool,`

```
/// Phase 2c (under the device mutex, CHEAP): set the TX ISR, inject any
/// synthetic RX replies (ARP/DNS) the emit produced, and decide whether to
/// raise IRQ10. `advanced` = at least one TX frame was consumed in the drain.
/// Returns `(tx_raise, rx_raise)` — which QUEUE wants its interrupt. Under
/// INTx both meant IRQ 10 and the guest's ISR handler walked every queue;
/// with MSI-X each queue has its own vector, and a synthetic RX reply
/// signalled on TX's vector was never seen (the guest waited forever on its
/// first DNS answer).
```

## L779 · `if self.tx_should_interrupt(mem) { tx_raise = true; }  // NAPI-TX reap`

```
// NAPI-TX reap
```

## L782 · `let mut rx_advanced = false;`

```
// Inject any synthetic replies (ARP/DNS) into RX.
```

## L795-796 · `pub fn caps(&self) -> NetCaps { self.caps }`

```
/// The negotiated capability mask (Copy) — the worker reads it once under the
/// device lock, then drives `nat::tap_outbound` lock-free.
```

## L802 · `fence(Ordering::SeqCst); // same barrier, same reason as the RX side`

```
// same barrier, same reason as the RX side
```

## L808 · `q.last_irq_used_idx = q.used_idx;   // signalled_used every check (QEMU)`

```
// signalled_used every check (QEMU)
```

