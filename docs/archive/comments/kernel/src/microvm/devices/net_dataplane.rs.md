# `kernel/src/microvm/devices/net_dataplane.rs` @ 5e0102684

## L1-22 · `use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};`

```
//! Off-vCPU virtio-net data plane for the microvm — the vhost-net model.
//!
//! Ported 1:1 from the in-kernel virtio-net DEVICE backend Linux runs:
//!   * `drivers/vhost/net.c` — `handle_rx` / `handle_tx` (the worker pulls the
//!     tap one frame at a time, copies into the guest ring while the guest has
//!     RX buffers, and STOPS when it doesn't — real backpressure, no synthetic
//!     staging queue).
//!   * `drivers/vhost/vhost.c` — `vhost_add_used_and_signal` / `vhost_signal` /
//!     `vhost_notify` (EVENT_IDX: raise the guest IRQ only when used.idx crosses
//!     the driver's `used_event` threshold). Our `VirtioNet::inject_rx` +
//!     `rx_should_interrupt` implement that decision.
//!   * `virt/kvm/eventfd.c` — irqfd: RX-ready → IRQ inject + vCPU wake in one.
//!     Here: `raise_irq()` (folds IRQ10) + `kick_bsp_net_irq()` (wakes the
//!     parked vCPU fiber).
//!
//! Why off-vCPU: the whole RX+TX data plane runs on ONE dedicated core (this
//! fiber), so the vCPUs only ring the TX doorbell (a lock-free flag) and reap
//! their IRQ. The vCPU is never the drainer, so it can't serialize the producer
//! behind the consumer.
//!
//! One mode, both vendors, every card. The fiber reads the tap and writes the
//! guest's rings; it does not know what kind of card put the frame there.
```

## L26-29 · `#[inline]`

```
/// True iff there is RX or TX work RIGHT NOW — a frame in the tap or a queued
/// guest TX kick. Lock-free, and card-neutral: this used to compare the QEMU
/// virtio NIC's used.idx, which on both target machines reads 0 forever, so the
/// test was `0 != 0` and the worker believed there was never anything to do.
```

## L38-41 · `static WAKE_IRQ: AtomicU64 = AtomicU64::new(0);`

```
/// Worker wakeup attribution (surfaced in `cores`): irq = host RX MSI-X woke us
/// (event-driven, µs); timeout = fell to the safety park; polled = no MSI-X
/// vector (yield_sleep). The 4th slot (`spin`) is retained at 0 for the `cores`
/// tuple shape.
```

## L45-46 · `static WAKE_BUSY: AtomicU64 = AtomicU64::new(0);`

```
/// busy = the halt-poll caught work and stayed warm (no HLT). High during an
/// active transfer = the RX→ACK loop is running hot → ACKs prompt → no TLP.
```

## L49 · `pub fn wake_snapshot() -> (u64, u64, u64, u64) {`

```
/// (irq, timeout, polled, busy) — double-sample for a per-second rate.
```

## L57-59 · `static RX_PASS_FRAMES: AtomicU64 = AtomicU64::new(0);`

```
/// RX cadence, for `cores`. `FRAMES`/`PASSES` give the average inject batch;
/// `GAP_MAX` is the peak TSC between successive non-empty passes (= the
/// inter-burst gap that, if longer than the warm window, drops us into a park).
```

## L65 · `fn note_rx_pass(n: u64, now: u64) {`

```
/// Record one non-empty RX drain pass: `n` frames drained, at TSC `now`.
```

## L78-80 · `pub fn rx_pass_stats() -> (u64, u64, u64) {`

```
/// (frames cumulative, passes cumulative, gap_max TSC). `gap_max` is swap-reset on
/// read so two calls bracket a window: the t0 call clears it, the t1 call returns
/// the window peak. `cores` diffs frames/passes for avg batch.
```

## L89-90 · `static ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// True between start_worker and stop_worker. Core 0's `net::poll()` yields the
/// NIC drain to this fiber while it's set.
```

## L93 · `pub fn active() -> bool { ACTIVE.load(Ordering::Acquire) }`

```
/// True while the data-plane fiber owns the host NIC drain.
```

## L96-98 · `const WORKER_STACK_BYTES: usize = 512 * 1024;`

```
/// `tap_pop` → `inject_rx` (guest-memory writes) → `tap_outbound` (masquerade +
/// segmentation) is a deep chain, and the default 128 KiB fiber stack has no
/// guard page. 512 KiB is generous headroom.
```

## L101-103 · `const PARK_SAFETY_MS: u64 = 2;`

```
/// Park safety cap (ms). The real wake is the tap doorbell or a guest TX kick;
/// this only bounds a quiet link so a held state still re-checks, and it is what
/// paces the poll of a card that raises no interrupt.
```

## L106-109 · `const BUSY_POLL_US: u64 = 1000;`

```
/// Halt-poll budget (µs) during an active transfer before falling to the HLT
/// park — the KVM `halt_poll_ns` analogue. ~1 ms bridges the natural inter-burst
/// gap so an ACK never waits long enough to trip the server's Tail-Loss-Probe
/// (~2×SRTT). Only spent while `recently_active`, on the reserved worker core.
```

## L112-116 · `pub fn start_worker(core: usize) {`

```
/// Spawn the data-plane fiber on `core` (load-aware, never Core 0). Idempotent
/// within a VM session. `full` = the off-vCPU vhost path (RX+TX on this core).
/// On a `!full` IRQ-driven NIC the BSP keeps the ring drained itself, so the
/// fiber is a NO-OP there; it runs `!full` only for a POLLED NIC that needs an
/// independent drainer.
```

## L120-121 · `crate::smp::fiber::admit_with_stack(core, worker_entry, 0, WORKER_STACK_BYTES);`

```
// The card's RX interrupt stays with the NAPI fiber (`net::napi`), which
// drains the card into the tap. This worker reads the tap, not a card.
```

## L125-130 · `#[inline]`

```
/// The active card raises no RX interrupt, so SOMEBODY has to poll it — Linux
/// gives such a device a poller too. Doing it here is not the old "the worker
/// drains the NIC and therefore only ever sees what IT pulled": the frames go
/// through the same one door as everyone else's (`eth::handle_frame` →
/// `nat::tap_inbound`), and the worker's own wake still hangs on its doorbell,
/// not on this card.
```

## L134-135 · `pub fn stop_worker() {`

```
/// Stop the fiber at VM teardown and wait (bounded) for it to exit so the host's
/// own networking reclaims the NIC drain.
```

## L137-140 · `crate::microvm::devices::gpu_backend::stop_worker();`

```
// The off-vCPU GPU worker shares this lifecycle (both spawned in
// vcpu_fiber_task); stop it here so every net-worker teardown site covers it
// too (else a leaked GPU fiber + a stale WORKER_RUNNING would block the next
// VM's GPU worker from starting). Idempotent (its own RUNNING guard).
```

## L154-155 · `const RX_PKT_WEIGHT: u64 = 256;`

```
/// `VHOST_NET_PKT_WEIGHT` (drivers/vhost/net.c): frames one pass may move before
/// yielding, so a saturated link cannot starve everything else on this core.
```

## L158-170 · `fn service_full(gm: &crate::microvm::devices::guest_mem::GuestMem) {`

```
/// vhost `handle_rx` + `handle_tx`: one pass on this core, then wake the guest.
///
/// The fiber does NOT touch a network card. Whoever drains the host NIC — Core
/// 0, a recv spin, or the AX200's WASM driver from its own fiber — ends in
/// `nat::tap_inbound`, and this reads the tap. That is what makes one data path
/// possible: where a frame ENTERS no longer decides whether this worker can see
/// it. Under the old shape the WASM driver delivered straight into
/// `eth::handle_frame`, which this function never looked at.
///
/// Lock discipline (the ACK-jitter fix): the device mutex is held only for the
/// SHORT guest-ring section (inject + TX ring walk). The vCPU spins on that same
/// mutex for its per-IRQ ISR read, which sits on the guest's ACK/NAPI path, so
/// the expensive masquerade + segmentation runs outside it.
```

## L174-178 · `let mut injected = false;`

```
// ── handle_rx: move frames from the TAP into the guest RX ring while the
//    guest has buffers, and STOP when it doesn't. vhost leaves the frame in
//    the socket and waits to be told buffers were refilled — it stages it
//    nowhere else. That is the whole of the backpressure: the tap fills,
//    the producer counts a drop, the far end slows down. ──
```

## L184 · `if dev.rx_avail_count(gm) == 0 { break; }   // get_rx_bufs -> 0`

```
// get_rx_bufs -> 0
```

## L192-194 · `nat::tap_push_front(frame);`

```
// `vhost_discard_vq_desc`: inject_rx rolled its descriptors back,
// so this frame was never consumed. Return it to the head and
// stop — the guest must run before retrying is worth anything.
```

## L199 · `let rx_raise = injected && dev.rx_should_interrupt(gm);`

```
// vhost_signal (RX): raise IRQ10 only when used.idx crossed used_event.
```

## L201-202 · `net_backend::take_tx_kick();`

```
// handle_tx: poll the guest TX ring every pass (take_tx_kick clears the
// doorbell so the halt-poll's tx_kick_pending resets). Cheap if empty.
```

## L207 · `}; // device mutex released.`

```
// device mutex released.
```

## L212-214 · `let tx_advanced = !tx_payloads.is_empty();`

```
// ── handle_tx, lock-free: masquerade + segment + hand to the host NIC. On a
//    bulk upload this is the long pole; outside the device mutex it cannot
//    stall the vCPU's ACK/ISR exits. ──
```

## L221-223 · `let (tx_raise, reply_rx_raise) = {`

```
// Short locked section: set the TX ISR, inject any synthetic replies (ARP /
// DNS), decide the raise. This fiber is the sole consumer of both guest
// rings, so dropping the lock in between is race-free.
```

## L230-231 · `crate::netdev::tx_flush();`

```
// Flush any guest egress batched into the host NIC's TX ring. Host-stack
// frames are no longer this fiber's business — it does not drain a card.
```

## L234 · `if injected || tx_raise {`

```
// Mark the data plane active so the halt-poll stays warm through a transfer.
```

## L239-243 · `let rx_intx = rx_raise && !net_backend::msix_notify(0);`

```
// irqfd: wake the guest only when EVENT_IDX asks for an interrupt. Without
// one the guest is inside its NAPI poll and reads the ring itself; a halted
// guest has re-armed used_event, so new entries always cross it.
// With MSI-X each queue's vector goes straight to its vCPU (irqfd → MSI);
// only INTx still takes the detour through the BSP's IRQ 10.
```

## L274 · `let now = crate::interrupts::ticks();`

```
// Host-originated TCP timers (OTA/https) at most ~100 Hz — never per-wake.
```

## L281-291 · `let gm = crate::microvm::devices::guest_mem::active();`

```
// HALT-POLL (KVM/NAPI busy-poll, the Linux model): during an ACTIVE
// transfer, stay WARM instead of HLTing between bursts. The RX→ACK loop
// (worker injects RX → guest ACKs → worker egresses the ACK) must not hit
// the ~1 ms HLT/timer granularity: a delayed ACK makes the server fire a
// Tail-Loss-Probe → SPURIOUS retransmit (measured: dsack==retrans, lost=0)
// → its cwnd/pacing get confused → throughput collapses (the lottery). So
// busy-poll the LOCK-FREE has_work() condition for up to BUSY_POLL_US; the
// instant RX arrives or an ACK is queued, loop and service it in µs. This
// is NOT the reverted lock-hammer spin — between events it only reads two
// atomics + cpu_relax, never the device lock. Reserved worker core +
// gated on recently_active (idle → HLT at once, no core-burn).
```

## L294 · `if let Some(gm) = gm {`

```
// Polling: the guest need not kick TX — this loop reads the ring.
```

## L306-307 · `crate::smp::fiber::yield_ready();`

```
// Cooperative core: a peer fiber here (the NAPI fiber filling
// the tap) runs between two looks instead of after the window.
```

## L312 · `continue; // stay warm — service the work on the next loop pass`

```
// stay warm — service the work on the next loop pass
```

## L314-315 · `}`

```
// No work and the active window expired → the transfer paused; fall
// through to the event-park (HLT) so an idle worker never burns the core.
```

## L317-318 · `if let Some(gm) = gm {`

```
// Before parking the doorbell must ring again; a frame queued while it
// was off is served now instead of waiting for the park's timeout.
```

## L325-331 · `crate::microvm::devices::nat::set_worker_parked(true);`

```
// Park on OUR OWN doorbell — never on some card's MSI-X. `tap_push` wakes
// this core on the tap's empty→occupied edge and `note_tx_kick` on a
// guest TX kick; both go through `kick_host_core`, which bumps this
// core's kick generation BEFORE the IPI. Announce the park first, then
// re-check, so a frame that lands in the arming window is never lost:
// either we see it here, or the producer sees `parked` and kicks, and the
// scheduler re-tests the generation on every scan.
```

