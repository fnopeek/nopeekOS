# `kernel/src/microvm/devices/net_backend.rs` @ 5e0102684

## L1-18 · `use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};`

```
//! Off-vCPU network backend for the microvm (vhost-style), Stage 1.
//!
//! Owns the virtio-net device OUTSIDE `VmShared`/`VM_BIG_LOCK` so a future
//! dedicated backend fiber (Stage 2) can run the whole data-plane — host-NIC
//! drain + `inject_rx` (RX) + `service_tx` (TX) — on its own core while the
//! vCPUs only ring the notify doorbell. Today the device is still serviced
//! inline from the vCPU exit handlers (behavior-neutral); they just reach it
//! through this lock instead of the old `sh.pci.virtio_net` field.
//!
//! Why out of `VmShared`: the run loop hands the lock holder an exclusive
//! `&mut VmShared`. A second core touching `virtio_net` while a vCPU holds that
//! borrow would alias. `GuestMem` is already `&self` (interior mutability), so
//! the backend can share `&GuestMem` soundly — only the device STATE needed to
//! move out. Lock order: a vCPU may take `VM_BIG_LOCK` then this lock; the
//! backend takes ONLY this lock (never `VM_BIG_LOCK`), so no cycle.
//!
//! Single instance: exactly one microvm runs at a time. A future multi-microvm
//! world makes this per-VM (an array keyed by VM id).
```

## L24-29 · `static TX_KICK: AtomicBool = AtomicBool::new(false);`

```
/// Guest TX kick (q1 notify) doorbell, set by the vCPU's net-MMIO exit in full
/// mode INSTEAD of servicing TX inline. The worker fiber drains it → service_tx +
/// tx_flush on ITS core, so RX and TX share one core / one NET path: no
/// cross-core NET-lock fight (the worker holding the lock for RX-inject was
/// delaying the vCPU's TX/ACK egress → throttling downloads, which also need
/// prompt ACKs). Stage 2c — the symmetric counterpart to the RX backend.
```

## L31-32 · `static WORKER_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);`

```
/// Host core the worker fiber runs on, so a vCPU TX-kick can wake it out of its
/// RX-IRQ park promptly (else TX waits up to the 2 ms park timeout → slow ACKs).
```

## L35 · `pub fn set_worker_core(core: usize) { WORKER_CORE.store(core, Ordering::Release); }`

```
/// Worker records its core at startup so TX-kicks can target it.
```

## L38-39 · `pub fn worker_core() -> Option<usize> {`

```
/// Core the data-plane worker fiber runs on, if one is up. The tap's producer
/// side needs it to ring the doorbell.
```

## L47-48 · `pub fn note_tx_kick() {`

```
/// vCPU: the guest kicked TX (q1). Set the doorbell and, on the empty→set edge,
/// wake the worker's core (coalesced like the IPI kick).
```

## L53-54 · `crate::smp::kick_host_core(c);`

```
// The worker parks in `kick_wait` on its own doorbell; this bumps
// the generation and IPIs its core.
```

## L60 · `pub fn take_tx_kick() -> bool { TX_KICK.swap(false, Ordering::AcqRel) }`

```
/// Worker: take the TX-kick doorbell (clears it). True ⇒ run service_tx.
```

## L63-64 · `pub fn tx_kick_pending() -> bool { TX_KICK.load(Ordering::Acquire) }`

```
/// Lock-free peek (does NOT clear): is a guest TX kick pending? Used by the
/// data-plane busy-poll so a queued ACK is egressed without HLTing first.
```

## L67-72 · `static NET_IRQ_PENDING: AtomicBool = AtomicBool::new(false);`

```
/// Guest RX/TX IRQ (IRQ10) raised by the net pump, which now runs OUTSIDE
/// `VM_BIG_LOCK` (it no longer touches `VmShared`). The BSP folds this into its
/// `pending_irqs` at a safe injection point. A lock-free atomic instead of
/// `sh.pending_irqs |= 1<<10` so the pump needs no `VmShared` borrow → APs no
/// longer block behind the BSP pump on a TLB-shootdown exit (the csd_lock_wait
/// root). Set by the pump (any caller), consumed by the BSP.
```

## L75-77 · `static ISR: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);`

```
/// virtio ISR status (read-to-clear). An atomic, not device state: the guest
/// reads it on every interrupt, and behind the device lock that read waited
/// for the worker's whole RX batch.
```

## L82-84 · `pub fn mmio_fast(off: u32, write: bool) -> Option<u64> {`

```
/// The guest's hot BAR accesses without the device lock: the ISR read, and
/// the TX doorbell while the worker owns the TX ring. `Some(value)` if
/// handled (0 for a write), `None` → take the lock for the full path.
```

## L99-100 · `static TX_AVAIL_GPA: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// Guest TX ring position for the worker's lock-free "anything queued?" look
/// while the doorbell is off: avail ring address and the consumed index.
```

## L107 · `pub fn tx_ring_pending(mem: &super::guest_mem::GuestMem) -> bool {`

```
/// avail.idx (offset 2 of the avail ring) moved past what the worker took.
```

## L114-118 · `pub const MSIX_VECTORS: usize = 3;`

```
// ── MSI-X (PCI 3.0 §6.8.2): config + RX + TX vectors ──
//
// The table lives in atomics, not in the device: the data-plane worker fires
// a queue's vector straight into the target vCPU's LAPIC (posted + kick) —
// KVM's irqfd → MSI route — without the device lock and without the BSP.
```

## L121 · `pub const MSIX_TABLE_OFF: u32 = 0x2000;`

```
/// BAR0 offsets of the table (16 bytes per entry) and the pending-bit array.
```

## L126-128 · `static MSIX_OFFERED: AtomicBool = AtomicBool::new(false);`

```
/// The capability is offered at all (`set microvm_msix on`,
/// read at VM open). Off by default until the path is proven: 0.446.0 had
/// it on and the guest's network stayed silent.
```

## L132 · `static MSIX_SENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// `cores` probe: messages sent, messages latched while masked.
```

## L136 · `pub fn msix_report() -> Option<([u64; 2], alloc::string::String)> {`

```
/// One line of MSI-X state for `cores`.
```

## L160 · `static MSIX_CTRL: [core::sync::atomic::AtomicU32; MSIX_VECTORS] =`

```
/// Vector control; every entry starts masked (PCI spec).
```

## L164 · `static QUEUE_VECTOR: [core::sync::atomic::AtomicU16; 2] =`

```
/// virtio `queue_msix_vector` per queue (RX 0, TX 1); 0xFFFF = none.
```

## L184 · `pub fn msix_msgctl() -> u16 {`

```
/// Message control word of the capability (read side).
```

## L191 · `pub fn msix_set_msgctl(v: u16) {`

```
/// Guest wrote the message control word: enable / function mask.
```

## L202 · `fn msix_fire(v: usize) {`

```
/// Send vector `v`'s message, or latch it in the PBA while it is masked.
```

## L212-213 · `let addr = MSIX_ADDR[v].load(Ordering::Acquire);`

```
// Message address: 0xFEE, destination ID in bits 19:12, destination mode
// (logical) in bit 2. Data: vector in 7:0, delivery mode in 10:8.
```

## L225 · `fn msix_flush_pending() {`

```
/// Unmasked entries with a pending bit fire now (PCI: on unmask).
```

## L237 · `pub fn msix_notify(q: u16) -> bool {`

```
/// Signal queue `q` by MSI-X. `false` = MSI-X is off, use INTx (IRQ 10).
```

## L242 · `true // with MSI-X on, a queue without a vector has no interrupt at all`

```
// with MSI-X on, a queue without a vector has no interrupt at all
```

## L245 · `pub fn msix_mmio(off: u32, write: Option<u32>) -> Option<u32> {`

```
/// MMIO into the MSI-X table / PBA (BAR0-relative `off`). `Some` if handled.
```

## L282 · `#[inline]`

```
/// Signal that the guest's virtio-net IRQ10 should be injected.
```

## L286 · `#[inline]`

```
/// BSP: take the pending net-IRQ signal (clears it). True ⇒ inject IRQ10.
```

## L290-291 · `#[inline]`

```
/// Non-consuming peek — does the worker have an RX IRQ waiting? Used by the BSP's
/// lock-free warm halt-poll to break out and VMRUN (where `take_irq` injects it).
```

## L295-296 · `static FULL_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// True while the data-plane worker owns the guest's rings. The vCPU reads it
/// to know that a TX kick belongs on the doorbell, not in its own hands.
```

## L303-304 · `static NET: Mutex<VirtioNet> = Mutex::new(VirtioNet::new());`

```
/// The one microvm virtio-net device. `VirtioNet::new()` is `const`, so this
/// needs no lazy init. Persists across VM runs; `reset()` re-arms it at open.
```

## L307 · `#[inline]`

```
/// Acquire the device for a multi-statement access (MMIO dispatch + service).
```

## L313-318 · `#[inline]`

```
/// LOCK-FREE BAR0 range check for the vCPU's NPF/EPT exit dispatch. Every non-blk
/// MMIO exit used to take the device mutex JUST to range-check the gpa — which
/// collided with the off-vCPU worker holding it (the ACK-jitter contention). The
/// BAR is fixed at `BAR0_BASE` (Linux keeps it there; confirmed in the boot log),
/// so the range test needs no lock. The mutex is taken only AFTER a hit, for the
/// actual MMIO service.
```

## L325-331 · `pub fn reset() {`

```
/// Re-initialise to power-on state at VM open. The static outlives a single VM
/// run, so this restores the per-VM-fresh state that `PciBus::new()` used to
/// give the device when it lived inside the bus.
///
/// Device state only. The worker's attachment (`FULL_ACTIVE`, `WORKER_CORE`)
/// belongs to the worker's start/stop: it is started before `vm_open` and, on
/// its own core, registers before this runs — a reset here detached it.
```

