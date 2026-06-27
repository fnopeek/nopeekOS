//! Dedicated host-NIC RX producer fiber for the microvm.
//!
//! ROOT CAUSE this fixes (2026-06-25 holistic analysis): the whole microvm RX
//! path — host-NIC drain (`poll_rx_only`) → `l3_inbound`/GRO → `inject_rx` into
//! the guest → guest-TX/ACK egress — ran SERIALLY on the single BSP vCPU core,
//! behind one `DEVICE` mutex, with the other worker cores (40-67% idle) unable
//! to help. That one core both drained the NIC (producer) and injected into the
//! guest (consumer), non-overlapping, so throughput capped at ~⅔ native despite
//! spare CPU, and the BSP parking in lulls produced ~10 ms `drainmax` stalls
//! (the stutter), because the host RX IRQ rarely coincided with the brief park.
//!
//! This module splits producer from consumer across cores, the Linux-NAPI
//! topology: a dedicated fiber on ANOTHER core drains the NIC + runs the IP
//! stack + GRO into `INBOUND_Q`, parked EVENT-DRIVEN on the host NIC RX IRQ
//! (routed to ITS core → reliable wake, stable MSI-X dest, no fight). The BSP
//! vCPU then only pops `INBOUND_Q` and injects (`drain_inbound`) — the two
//! halves pipeline. `INBOUND_Q` is the SPSC-style handoff (a Mutex<VecDeque>,
//! already MPSC-safe). The producer touches ONLY the NIC + IP stack + the
//! staging queue/GRO; the BSP owns the guest virtqueue + guest memory. No shared
//! device state crosses cores on the hot path.
//!
//! Gated by `active()`: while the producer runs, the BSP skips its own NIC drain
//! and Core 0's `net::poll()` skips the drain too (`net::mod` `skip_nic_drain`),
//! so the producer is the SOLE drainer (the single-consumer-ring invariant the
//! POLLING guard protects). If the producer is stopped, the old BSP-drains path
//! is restored unchanged.

use core::sync::atomic::{AtomicBool, Ordering};

static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// True between start_worker and stop_worker. The BSP pump and Core 0's
/// net::poll() check this to yield the NIC drain to this fiber.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// True while the dedicated RX producer owns the host NIC drain.
pub fn active() -> bool { ACTIVE.load(Ordering::Acquire) }

/// Producer stack. `poll_rx_only` → `eth::handle_frame` → IP/TCP stack can run a
/// deep call chain; the default 128 KiB fiber stack has no guard page (silent
/// smash on overflow). 512 KiB is generous headroom for the RX path.
const WORKER_STACK_BYTES: usize = 512 * 1024;

/// Master switch. The producer/consumer split (NIC drain on a separate core) is
/// gated to a POLLED host NIC (`rx_irq_vector() == 0`) in `start_worker` — see
/// there for why. On an IRQ-driven NIC (QEMU virtio) it was HW-measured net-
/// negative (the INBOUND_Q handoff raised rxlat 20µs→270µs → RTT → lower
/// throughput, with the BSP keeping up anyway); on a polled bare-metal NIC the
/// BSP-sole-drainer CANNOT keep up (it parks/compute-stalls → the tiny 32-desc
/// NIC ring overflows silently → the measured ~190ms drainmax → 40 Mbit while
/// host-direct on the SAME NIC does ~600). There the independent drainer is the
/// fix, not a regression. See project_browser_net_perf / project_microvm_rx_gro.
const ENABLED: bool = true;

/// Spawn the RX producer on `core` (load-aware, never Core 0). Idempotent within
/// a VM session. NO-OP on an IRQ-driven NIC (the BSP keeps the ring drained there
/// and the handoff only adds RTT); runs ONLY when the host NIC is polled.
pub fn start_worker(core: usize, full: bool) {
    if !ENABLED { return; }
    // Producer-only mode (full=false): only a POLLED host NIC (no RX MSI-X) needs
    // an independent drainer — on an IRQ-driven NIC (QEMU) the BSP keeps up and
    // the producer→BSP handoff was net-negative, so skip it there.
    // Full mode (Stage 2b, full=true): this fiber ALSO injects, so the WHOLE RX
    // path runs off the vCPU on ONE core (no cross-core handoff) — that wins even
    // on an IRQ-driven NIC, so it runs regardless of the vector.
    if !full && crate::drivers::virtio_net::rx_irq_vector() != 0 { return; }
    if WORKER_RUNNING.swap(true, Ordering::AcqRel) { return; }
    STOP.store(false, Ordering::Release);
    // ACTIVE is set by the fiber itself on its first iteration, NOT here: the
    // gates (BSP pump, Core 0 skip_nic_drain) hand the NIC drain to the producer
    // the instant ACTIVE flips. Setting it before the fiber is actually scheduled
    // (admit only queues it; the worker core picks it up ≤ one tick later) would
    // open a window where everyone yields the drain but the producer isn't live
    // yet → nobody drains. Until the fiber runs, the old BSP-drains path stands.
    crate::smp::fiber::admit_with_stack(core, worker_entry, full as u64, WORKER_STACK_BYTES);
}

/// Stop the producer at VM teardown and wait (bounded) for it to exit, so the
/// host's own networking (Core 0 `net::poll`, OTA) reclaims the NIC drain.
pub fn stop_worker() {
    if !WORKER_RUNNING.load(Ordering::Acquire) { return; }
    // Clear ACTIVE + FULL first so the BSP/Core-0 resume draining (and the BSP
    // resumes its own RX pump) immediately, even before the fiber observes STOP.
    ACTIVE.store(false, Ordering::Release);
    crate::microvm::devices::net_backend::set_full_active(false);
    STOP.store(true, Ordering::Release);
    for _ in 0..50_000_000u64 {
        if !WORKER_RUNNING.load(Ordering::Acquire) { break; }
        core::hint::spin_loop();
    }
}

/// Stage 2b consumer: drain INBOUND_Q into the guest RX ring on THIS core, then
/// signal + kick the BSP to inject IRQ10 if the guest wants it. Only in full
/// mode; gracefully no-ops if guest memory was already torn down (active()=None).
#[inline]
fn full_consume(full: bool) {
    if !full { return; }
    if let Some(gm) = crate::microvm::devices::guest_mem::active() {
        let want_irq = super::nat::drain_to_guest(
            &mut crate::microvm::devices::net_backend::lock(), gm);
        if want_irq {
            super::nat::note_worker_raise();
            crate::microvm::devices::net_backend::raise_irq();
            crate::microvm::cpu::svm::kick_bsp_net_irq();
        }
    }
}

/// Stage 2c TX: service the guest TX queue on THIS core when the vCPU rang the
/// TX doorbell — process_tx (segment/NAT/send) + tx_flush, so RX and TX run on
/// one core / one NET path. Raises IRQ10 for the TX-completion (so the guest
/// reaps its TX descriptors) + any synthetic ARP/DNS reply, via the same
/// lock-free raise + BSP kick as RX.
#[inline]
fn full_tx(full: bool) {
    if !full || !crate::microvm::devices::net_backend::take_tx_kick() { return; }
    if let Some(gm) = crate::microvm::devices::guest_mem::active() {
        let raise = crate::microvm::devices::net_backend::lock().service_queues(1, gm);
        // Egress the frames service_tx queued into the host NIC TX ring (the
        // worker owns TX egress now → no cross-core race on that ring).
        crate::virtio_net::tx_flush();
        if raise {
            crate::microvm::devices::net_backend::raise_irq();
            crate::microvm::cpu::svm::kick_bsp_net_irq();
        }
    }
}

fn worker_entry(arg: u64) {
    let full = arg != 0;
    // Now that we're actually scheduled, claim the NIC drain (closes the
    // start_worker→admit launch gap where the gates would yield to a not-yet-live
    // producer). stop_worker clears this first, before STOP, for a clean handoff.
    ACTIVE.store(true, Ordering::Release);
    // Record our core so a vCPU TX-kick can wake us promptly (Stage 2c).
    if full {
        crate::microvm::devices::net_backend::set_worker_core(
            crate::smp::per_core::current_core_id());
    }
    // Publish full-backend mode only once we're live, so the BSP keeps its own RX
    // pump until this fiber actually starts draining (no RX gap at launch).
    if full { crate::microvm::devices::net_backend::set_full_active(true); }
    let mut last_tick = crate::interrupts::ticks();
    loop {
        if STOP.load(Ordering::Acquire) {
            WORKER_RUNNING.store(false, Ordering::Release);
            return;
        }

        // Drain the host NIC into INBOUND_Q + flush any GRO burst past its
        // latency budget. This is the producer half of the pipeline.
        super::nat::rx_producer_drain();
        // Full mode: also consume (inject into the guest) — the whole RX path on
        // this core, off the vCPU — and service guest TX (Stage 2c).
        full_consume(full);
        full_tx(full);

        // Host-stack TCP timers (retransmit/RTO) — the guest uses its own TCP via
        // L3-NAT so this is only for host-originated connections (OTA/https). Run
        // it at most ~100 Hz, never per-wake, to avoid the 128-slot scan + lock
        // on the hot RX path.
        let now = crate::interrupts::ticks();
        if now != last_tick {
            crate::net::tcp::tick_connections();
            last_tick = now;
        }

        // Park EVENT-DRIVEN on the host NIC RX IRQ, routed to THIS core. The
        // snapshot is taken BEFORE the drain above conceptually closes the
        // lost-wakeup window via the re-arm here: arm() snapshots the current
        // fired-count and routes the IRQ to this core; a packet that arrived
        // during the drain already advanced the count, so irq_wait returns at
        // once and we loop to drain it. The 2 ms timeout bounds the wait so a
        // held GRO burst still flushes and a polled NIC (vector 0) still drains.
        let vec = crate::drivers::virtio_net::rx_irq_vector();
        if vec != 0 {
            let since = crate::irq::arm(vec);
            super::nat::rx_producer_drain();
            full_consume(full);
            full_tx(full);
            crate::smp::fiber::irq_wait(vec, since, 2);
        } else {
            // Polled NIC (no RX MSI-X): short sleep, still off the BSP core.
            full_consume(full);
            full_tx(full);
            crate::smp::fiber::yield_sleep(1);
        }
    }
}
