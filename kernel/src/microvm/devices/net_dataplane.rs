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
//! their IRQ. The vCPU is never the NIC drainer, so it can't serialize the
//! producer behind the consumer. This REPLACES the hand-rolled surrogate
//! (`INBOUND_Q` staging queue + custom GRO + `drain_inbound`): RX now flows
//! host-NIC → `l3_rewrite_inbound` (address translation only) → guest ring,
//! with NIC-ring backpressure instead of a lossy 1024-deep middle queue.
//!
//! Two modes:
//!   * `full` (the AMD/off-vCPU path): this fiber owns RX **and** TX — the vhost
//!     model above. The vCPU does no net work beyond the TX doorbell + IRQ reap.
//!   * producer-only (`!full`, a POLLED bare-metal NIC with no RX MSI-X): the
//!     legacy path — drain the NIC into the BSP-consumed staging queue
//!     (`nat::rx_producer_drain`), because that NIC needs an independent drainer
//!     but the off-vCPU inject/IRQ-fold is SVM-only today. Migrated to the full
//!     vhost path when VMX mirrors the BSP-kick.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use alloc::collections::VecDeque;
use spin::Mutex;

/// Lock-free RX_STAGE depth + last-drained host-NIC RX used.idx, so the busy-poll
/// can test "is there RX/ACK work?" without taking any lock (the cheap condition
/// that keeps it from being the reverted lock-hammer spin).
static RX_STAGE_LEN: AtomicUsize = AtomicUsize::new(0);
static LAST_RX_USED: AtomicU32 = AtomicU32::new(0);

/// True iff there is RX or TX work to service RIGHT NOW — a fresh host-NIC RX
/// frame (used.idx moved past what we last drained) or a queued ACK (TX kick).
/// Lock-free. Deliberately does NOT include the RX stage: a stage that can't
/// drain because the guest ring is full must NOT busy-spin (the guest frees
/// buffers asynchronously; the next real RX/TX event drains the stage). The
/// `RX_STAGE_LEN` counter is kept for diagnostics / future use.
#[inline]
fn has_work() -> bool {
    let _ = RX_STAGE_LEN.load(Ordering::Relaxed);
    crate::drivers::virtio_net::rx_used_idx() as u32 != LAST_RX_USED.load(Ordering::Relaxed)
        || crate::microvm::devices::net_backend::tx_kick_pending()
}

/// Worker-local RX overflow stage. The host NIC RX ring MUST be kept empty every
/// pass — leaving frames in it lets it overflow → QEMU/slirp drops → the server
/// retransmits → its cwnd collapses (the download lottery). So we ALWAYS drain
/// the whole NIC ring; a frame the guest ring can't take this instant is staged
/// here (lossless) and re-injected oldest-first next pass. Only the data-plane
/// fiber touches it (single consumer). Bounded: on sustained overload (the guest
/// genuinely can't keep up) the oldest staged frame is dropped — but a transient
/// guest-slow window (the common case) is absorbed, never lost.
static RX_STAGE: Mutex<VecDeque<alloc::vec::Vec<u8>>> = Mutex::new(VecDeque::new());
/// ~3 MB of frames ≈ 24 ms at 1 Gbit — far more than any guest-slow transient.
const RX_STAGE_MAX: usize = 2048;

/// Stage a frame the guest ring couldn't take. Drops (recycles) the OLDEST on
/// sustained overload to bound memory; the NIC ring is still emptied either way.
fn stage_push(frame: alloc::vec::Vec<u8>) {
    let mut s = RX_STAGE.lock();
    if s.len() >= RX_STAGE_MAX {
        if let Some(old) = s.pop_front() {
            crate::microvm::devices::nat::recycle_frame(old);
        }
    }
    s.push_back(frame);
    RX_STAGE_LEN.store(s.len(), Ordering::Relaxed);
}

/// Re-inject staged frames (oldest first) while the guest ring has room.
fn inject_stage(
    dev: &mut crate::microvm::devices::virtio_net_dev::VirtioNet,
    gm: &crate::microvm::devices::guest_mem::GuestMem,
    injected: &mut bool,
) {
    let mut s = RX_STAGE.lock();
    while !s.is_empty() && dev.rx_avail_count(gm) > 0 {
        let f = s.pop_front().unwrap();
        if dev.inject_rx(gm, &f) { *injected = true; }
        crate::microvm::devices::nat::recycle_frame(f);
    }
    RX_STAGE_LEN.store(s.len(), Ordering::Relaxed);
}

/// Drop the RX stage at VM teardown so a stale frame can't leak into the next run.
pub fn reset_stage() {
    let mut s = RX_STAGE.lock();
    while let Some(f) = s.pop_front() {
        crate::microvm::devices::nat::recycle_frame(f);
    }
    RX_STAGE_LEN.store(0, Ordering::Relaxed);
}

/// Worker wakeup attribution (surfaced in `cores`): irq = host RX MSI-X woke us
/// (event-driven, µs); timeout = fell to the safety park; polled = no MSI-X
/// vector (yield_sleep). The 4th slot (`spin`) is retained at 0 for the `cores`
/// tuple shape.
static WAKE_IRQ: AtomicU64 = AtomicU64::new(0);
static WAKE_TIMEOUT: AtomicU64 = AtomicU64::new(0);
static WAKE_POLLED: AtomicU64 = AtomicU64::new(0);
/// busy = the halt-poll caught work and stayed warm (no HLT). High during an
/// active transfer = the RX→ACK loop is running hot → ACKs prompt → no TLP.
static WAKE_BUSY: AtomicU64 = AtomicU64::new(0);

/// (irq, timeout, polled, busy) — double-sample for a per-second rate.
pub fn wake_snapshot() -> (u64, u64, u64, u64) {
    (WAKE_IRQ.load(Ordering::Relaxed),
     WAKE_TIMEOUT.load(Ordering::Relaxed),
     WAKE_POLLED.load(Ordering::Relaxed),
     WAKE_BUSY.load(Ordering::Relaxed))
}

static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// True between start_worker and stop_worker. Core 0's `net::poll()` yields the
/// NIC drain to this fiber while it's set.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// True while the data-plane fiber owns the host NIC drain.
pub fn active() -> bool { ACTIVE.load(Ordering::Acquire) }

/// Producer stack: `netdev::recv` → `l3_rewrite_inbound` (rewrite + checksums) →
/// `inject_rx` (guest-memory writes) is a deep chain; the default 128 KiB fiber
/// stack has no guard page. 512 KiB is generous headroom.
const WORKER_STACK_BYTES: usize = 512 * 1024;

/// Event-park safety cap (ms). The real wake is the host NIC RX IRQ (routed to
/// this core) or the TX doorbell (`note_tx_kick` bumps the RX vector's fired
/// count + IPIs this core). This only bounds a quiet link so a held state still
/// re-checks — kept short because the wake is reliably event-driven.
const PARK_SAFETY_MS: u64 = 2;

/// Halt-poll budget (µs) during an active transfer before falling to the HLT
/// park — the KVM `halt_poll_ns` analogue. ~1 ms bridges the natural inter-burst
/// gap so an ACK never waits long enough to trip the server's Tail-Loss-Probe
/// (~2×SRTT). Only spent while `recently_active`, on the reserved worker core.
const BUSY_POLL_US: u64 = 1000;

/// Spawn the data-plane fiber on `core` (load-aware, never Core 0). Idempotent
/// within a VM session. `full` = the off-vCPU vhost path (RX+TX on this core).
/// On a `!full` IRQ-driven NIC the BSP keeps the ring drained itself, so the
/// fiber is a NO-OP there; it runs `!full` only for a POLLED NIC that needs an
/// independent drainer.
pub fn start_worker(core: usize, full: bool) {
    if !full && crate::drivers::virtio_net::rx_irq_vector() != 0 { return; }
    if WORKER_RUNNING.swap(true, Ordering::AcqRel) { return; }
    STOP.store(false, Ordering::Release);
    crate::smp::fiber::admit_with_stack(core, worker_entry, full as u64, WORKER_STACK_BYTES);
}

/// Stop the fiber at VM teardown and wait (bounded) for it to exit so the host's
/// own networking reclaims the NIC drain.
pub fn stop_worker() {
    if !WORKER_RUNNING.load(Ordering::Acquire) { return; }
    ACTIVE.store(false, Ordering::Release);
    crate::microvm::devices::net_backend::set_full_active(false);
    STOP.store(true, Ordering::Release);
    for _ in 0..50_000_000u64 {
        if !WORKER_RUNNING.load(Ordering::Acquire) { break; }
        core::hint::spin_loop();
    }
    reset_stage();
}

/// vhost `handle_rx` + `handle_tx` for the full off-vCPU path: one pass on this
/// core, then wake the guest.
///
/// Lock discipline (the ACK-jitter fix): the host-NIC drain + L3 rewrite is the
/// SLOW part (per-frame DMA copy + masquerade) and touches ONLY the host NIC +
/// NAT — NOT the guest device — so it runs WITHOUT `net_backend` (POLLING guard
/// only). The device mutex is then held for just the SHORT guest-ring critical
/// section (inject + TX service). The vCPU spins on that same mutex for its
/// per-IRQ ISR read, which sits on the guest's ACK/NAPI path; holding it across
/// the whole drain (the old code) stalled ACK egress → spurious TLP. Keep it short.
fn service_full(gm: &crate::microvm::devices::guest_mem::GuestMem) {
    use crate::microvm::devices::{nat, net_backend};

    // ── Phase 1: drain the host NIC LOCK-FREE (POLLING guard only) and rewrite
    //    each masqueraded frame into a staging Vec. No net_backend lock held, so
    //    the vCPU's ISR-read / ACK MMIO exits never wait on this slow host I/O. ──
    let mut rewritten: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
    let mut host_frames: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
    if crate::net::try_acquire_drain() {
        let mut buf = [0u8; crate::drivers::virtio_net::MTU];
        while let Some(len) = crate::netdev::recv(&mut buf) {
            if len < 14 { continue; }
            let ethertype = u16::from_be_bytes([buf[12], buf[13]]);
            if ethertype == crate::net::eth::ETHERTYPE_IPV4 {
                if let Some(gframe) = nat::l3_rewrite_inbound(&buf[14..len]) {
                    rewritten.push(gframe);
                    continue;
                }
            }
            // Not masqueraded guest traffic (ARP / host IPv4) — host stack, later.
            host_frames.push(buf[..len].to_vec());
        }
        crate::net::release_drain();
        // Mark the NIC drained to here so has_work() only fires on a NEW frame.
        LAST_RX_USED.store(
            crate::drivers::virtio_net::rx_used_idx() as u32, Ordering::Relaxed);
    }

    // ── Phase 2: SHORT device-mutex section — guest-ring work only. Inject any
    //    previously staged frames first (in order), then the freshly drained ones;
    //    a frame the guest ring can't take is STAGED (lossless), never dropped /
    //    left in the NIC ring. Then service the guest TX ring (symmetric with RX,
    //    polled every pass). ──
    let mut injected = false;
    let (rx_raise, tx_raise) = {
        let mut dev = net_backend::lock();
        inject_stage(&mut dev, gm, &mut injected);
        for gframe in rewritten {
            if dev.rx_avail_count(gm) > 0 && dev.inject_rx(gm, &gframe) {
                injected = true;
                nat::recycle_frame(gframe);
            } else {
                stage_push(gframe);
            }
        }
        // vhost_signal (RX): raise IRQ10 only when used.idx crossed used_event.
        let rx_raise = injected && dev.rx_should_interrupt(gm);
        // handle_tx: poll the guest TX ring every pass (take_tx_kick clears the
        // doorbell so the halt-poll's tx_kick_pending resets). Cheap if empty.
        net_backend::take_tx_kick();
        let tx_raise = dev.service_queues(1, gm);
        (rx_raise, tx_raise)
    }; // device mutex released here — vCPU ACK exits no longer wait on the drain

    // Host-stack frames (outside the device lock).
    for f in &host_frames { crate::net::eth::handle_frame(f); }
    // Flush any guest TX / host egress queued into the host NIC TX ring.
    crate::virtio_net::tx_flush();

    // Mark the data plane active so the worker's halt-poll (recently_active)
    // stays warm through this transfer — RX in or an ACK out both count.
    if injected || tx_raise {
        nat::mark_active();
    }

    // irqfd: wake the guest. Raise IRQ10 when EVENT_IDX says so; otherwise still
    // kick the vCPU scheduler so a parked vCPU NAPI-polls the non-empty ring (a
    // parked vCPU can't poll on its own).
    if rx_raise || tx_raise {
        net_backend::raise_irq();
        crate::microvm::cpu::svm::kick_bsp_net_irq();
    } else if injected {
        crate::microvm::cpu::svm::kick_bsp_net_irq();
    }
}

fn worker_entry(arg: u64) {
    let full = arg != 0;
    ACTIVE.store(true, Ordering::Release);
    if full {
        crate::microvm::devices::net_backend::set_worker_core(
            crate::smp::per_core::current_core_id());
        crate::microvm::devices::net_backend::set_full_active(true);
    }
    let mut last_tick = crate::interrupts::ticks();
    loop {
        if STOP.load(Ordering::Acquire) {
            WORKER_RUNNING.store(false, Ordering::Release);
            return;
        }

        if full {
            if let Some(gm) = crate::microvm::devices::guest_mem::active() {
                service_full(gm);
            }
        } else {
            // Producer-only (polled NIC): legacy staging-queue drain; BSP injects.
            crate::microvm::devices::nat::rx_producer_drain();
        }

        // Host-originated TCP timers (OTA/https) at most ~100 Hz — never per-wake.
        let now = crate::interrupts::ticks();
        if now != last_tick {
            crate::net::tcp::tick_connections();
            last_tick = now;
        }

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
        if full && crate::microvm::devices::nat::recently_active() {
            let freq = crate::interrupts::tsc_freq();
            let deadline = crate::interrupts::rdtsc()
                + BUSY_POLL_US.saturating_mul((freq / 1_000_000).max(1));
            let mut got = false;
            while crate::interrupts::rdtsc() < deadline {
                if has_work() { got = true; break; }
                core::hint::spin_loop();
            }
            if got {
                WAKE_BUSY.fetch_add(1, Ordering::Relaxed);
                continue; // stay warm — service the work on the next loop pass
            }
            // Budget exhausted with no work → the transfer paused; fall through
            // to the event-park (HLT) so an idle worker never burns the core.
        }

        // Park EVENT-DRIVEN on the host NIC RX IRQ (routed to this core). Arm
        // AFTER the drain, then drain once more, so a frame that arrived in the
        // arming window already advanced the fired count → irq_wait returns at
        // once (no lost wakeup). The TX doorbell also wakes us: `note_tx_kick`
        // bumps this vector's fired count + IPIs this core.
        let vec = crate::drivers::virtio_net::rx_irq_vector();
        if vec != 0 {
            let since = crate::irq::arm(vec);
            if full {
                if let Some(gm) = crate::microvm::devices::guest_mem::active() {
                    service_full(gm);
                }
            } else {
                crate::microvm::devices::nat::rx_producer_drain();
            }
            if crate::smp::fiber::irq_wait(vec, since, PARK_SAFETY_MS) {
                WAKE_IRQ.fetch_add(1, Ordering::Relaxed);
            } else {
                WAKE_TIMEOUT.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            WAKE_POLLED.fetch_add(1, Ordering::Relaxed);
            crate::smp::fiber::yield_sleep(1);
        }
    }
}
