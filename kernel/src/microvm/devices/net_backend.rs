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

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};
use super::virtio_net_dev::VirtioNet;

/// Guest TX kick (q1 notify) doorbell, set by the vCPU's net-MMIO exit in full
/// mode INSTEAD of servicing TX inline. The worker fiber drains it → service_tx +
/// tx_flush on ITS core, so RX and TX share one core / one NET path: no
/// cross-core NET-lock fight (the worker holding the lock for RX-inject was
/// delaying the vCPU's TX/ACK egress → throttling downloads, which also need
/// prompt ACKs). Stage 2c — the symmetric counterpart to the RX backend.
static TX_KICK: AtomicBool = AtomicBool::new(false);
/// Host core the worker fiber runs on, so a vCPU TX-kick can wake it out of its
/// RX-IRQ park promptly (else TX waits up to the 2 ms park timeout → slow ACKs).
static WORKER_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Worker records its core at startup so TX-kicks can target it.
pub fn set_worker_core(core: usize) { WORKER_CORE.store(core, Ordering::Release); }

/// Core the data-plane worker fiber runs on, if one is up. The tap's producer
/// side needs it to ring the doorbell.
pub fn worker_core() -> Option<usize> {
    match WORKER_CORE.load(Ordering::Acquire) {
        usize::MAX => None,
        c => Some(c),
    }
}

/// vCPU: the guest kicked TX (q1). Set the doorbell and, on the empty→set edge,
/// wake the worker's core (coalesced like the IPI kick).
pub fn note_tx_kick() {
    if !TX_KICK.swap(true, Ordering::AcqRel) {
        let c = WORKER_CORE.load(Ordering::Acquire);
        if c != usize::MAX {
            // The worker parks in `kick_wait` on its own doorbell; this bumps
            // the generation and IPIs its core.
            crate::smp::kick_host_core(c);
        }
    }
}

/// Worker: take the TX-kick doorbell (clears it). True ⇒ run service_tx.
pub fn take_tx_kick() -> bool { TX_KICK.swap(false, Ordering::AcqRel) }

/// Lock-free peek (does NOT clear): is a guest TX kick pending? Used by the
/// data-plane busy-poll so a queued ACK is egressed without HLTing first.
pub fn tx_kick_pending() -> bool { TX_KICK.load(Ordering::Acquire) }

/// Guest RX/TX IRQ (IRQ10) raised by the net pump, which now runs OUTSIDE
/// `VM_BIG_LOCK` (it no longer touches `VmShared`). The BSP folds this into its
/// `pending_irqs` at a safe injection point. A lock-free atomic instead of
/// `sh.pending_irqs |= 1<<10` so the pump needs no `VmShared` borrow → APs no
/// longer block behind the BSP pump on a TLB-shootdown exit (the csd_lock_wait
/// root). Set by the pump (any caller), consumed by the BSP.
static NET_IRQ_PENDING: AtomicBool = AtomicBool::new(false);

/// virtio ISR status (read-to-clear). An atomic, not device state: the guest
/// reads it on every interrupt, and behind the device lock that read waited
/// for the worker's whole RX batch.
static ISR: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
pub fn raise_isr() { ISR.fetch_or(1, Ordering::AcqRel); }
pub fn take_isr() -> u8 { ISR.swap(0, Ordering::AcqRel) }

/// The guest's hot BAR accesses without the device lock: the ISR read, and
/// the TX doorbell while the worker owns the TX ring. `Some(value)` if
/// handled (0 for a write), `None` → take the lock for the full path.
pub fn mmio_fast(off: u32, write: bool) -> Option<u64> {
    use super::virtio_net_dev::{ISR_OFF, ISR_LEN, NOTIFY_OFF, NOTIFY_LEN, NOTIFY_OFF_MULTIPLIER};
    if !write && (ISR_OFF..ISR_OFF + ISR_LEN).contains(&off) {
        return Some(take_isr() as u64);
    }
    if write && full_active() && (NOTIFY_OFF..NOTIFY_OFF + NOTIFY_LEN).contains(&off)
        && (off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER == 1
    {
        note_tx_kick();
        return Some(0);
    }
    None
}

/// Guest TX ring position for the worker's lock-free "anything queued?" look
/// while the doorbell is off: avail ring address and the consumed index.
static TX_AVAIL_GPA: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static TX_LAST_AVAIL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub fn set_tx_ring(avail_gpa: u64, last_avail: u16) {
    TX_AVAIL_GPA.store(avail_gpa, Ordering::Release);
    TX_LAST_AVAIL.store(last_avail as u32, Ordering::Release);
}
/// avail.idx (offset 2 of the avail ring) moved past what the worker took.
pub fn tx_ring_pending(mem: &super::guest_mem::GuestMem) -> bool {
    let gpa = TX_AVAIL_GPA.load(Ordering::Acquire);
    gpa != 0 && mem.read_u16(gpa + 2)
        .is_some_and(|top| top as u32 != TX_LAST_AVAIL.load(Ordering::Acquire))
}

// ── MSI-X (PCI 3.0 §6.8.2): config + RX + TX vectors ──
//
// The table lives in atomics, not in the device: the data-plane worker fires
// a queue's vector straight into the target vCPU's LAPIC (posted + kick) —
// KVM's irqfd → MSI route — without the device lock and without the BSP.

pub const MSIX_VECTORS: usize = 3;
/// BAR0 offsets of the table (16 bytes per entry) and the pending-bit array.
pub const MSIX_TABLE_OFF: u32 = 0x2000;
pub const MSIX_PBA_OFF: u32 = 0x3000;
const MSIX_ENTRY_MASKED: u32 = 1;

/// The capability is offered at all (`set microvm_msix on`,
/// read at VM open). Off by default until the path is proven: 0.446.0 had
/// it on and the guest's network stayed silent.
static MSIX_OFFERED: AtomicBool = AtomicBool::new(false);
pub fn msix_offered() -> bool { MSIX_OFFERED.load(Ordering::Acquire) }

/// `cores` probe: messages sent, messages latched while masked.
static MSIX_SENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static MSIX_LATCHED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// One line of MSI-X state for `cores`.
pub fn msix_report() -> Option<([u64; 2], alloc::string::String)> {
    if !msix_offered() { return None; }
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let _ = write!(s, "en={} fmask={} qvec=[{:#x},{:#x}] pba={:#x}",
        MSIX_ENABLED.load(Ordering::Relaxed) as u8,
        MSIX_FUNC_MASK.load(Ordering::Relaxed) as u8,
        QUEUE_VECTOR[0].load(Ordering::Relaxed), QUEUE_VECTOR[1].load(Ordering::Relaxed),
        MSIX_PENDING.load(Ordering::Relaxed));
    for v in 0..MSIX_VECTORS {
        let _ = write!(s, " v{}={:#x}/{:#x}/{}", v,
            MSIX_ADDR[v].load(Ordering::Relaxed), MSIX_DATA[v].load(Ordering::Relaxed),
            MSIX_CTRL[v].load(Ordering::Relaxed) & 1);
    }
    Some(([MSIX_SENT.load(Ordering::Relaxed), MSIX_LATCHED.load(Ordering::Relaxed)], s))
}

static MSIX_ENABLED: AtomicBool = AtomicBool::new(false);
static MSIX_FUNC_MASK: AtomicBool = AtomicBool::new(false);
static MSIX_ADDR: [core::sync::atomic::AtomicU64; MSIX_VECTORS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MSIX_VECTORS];
static MSIX_DATA: [core::sync::atomic::AtomicU32; MSIX_VECTORS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MSIX_VECTORS];
/// Vector control; every entry starts masked (PCI spec).
static MSIX_CTRL: [core::sync::atomic::AtomicU32; MSIX_VECTORS] =
    [const { core::sync::atomic::AtomicU32::new(MSIX_ENTRY_MASKED) }; MSIX_VECTORS];
static MSIX_PENDING: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// virtio `queue_msix_vector` per queue (RX 0, TX 1); 0xFFFF = none.
static QUEUE_VECTOR: [core::sync::atomic::AtomicU16; 2] =
    [const { core::sync::atomic::AtomicU16::new(0xFFFF) }; 2];

fn msix_reset() {
    MSIX_OFFERED.store(
        crate::config::get("microvm_msix").is_some_and(|v| v.trim().eq_ignore_ascii_case("on")),
        Ordering::Release,
    );
    MSIX_ENABLED.store(false, Ordering::Release);
    MSIX_FUNC_MASK.store(false, Ordering::Release);
    for i in 0..MSIX_VECTORS {
        MSIX_ADDR[i].store(0, Ordering::Relaxed);
        MSIX_DATA[i].store(0, Ordering::Relaxed);
        MSIX_CTRL[i].store(MSIX_ENTRY_MASKED, Ordering::Relaxed);
    }
    MSIX_PENDING.store(0, Ordering::Release);
    for q in QUEUE_VECTOR.iter() { q.store(0xFFFF, Ordering::Release); }
}

/// Message control word of the capability (read side).
pub fn msix_msgctl() -> u16 {
    (MSIX_VECTORS as u16 - 1)
        | if MSIX_ENABLED.load(Ordering::Acquire) { 1 << 15 } else { 0 }
        | if MSIX_FUNC_MASK.load(Ordering::Acquire) { 1 << 14 } else { 0 }
}

/// Guest wrote the message control word: enable / function mask.
pub fn msix_set_msgctl(v: u16) {
    MSIX_ENABLED.store(v & (1 << 15) != 0, Ordering::Release);
    MSIX_FUNC_MASK.store(v & (1 << 14) != 0, Ordering::Release);
    msix_flush_pending();
}

pub fn set_queue_vector(queue: u16, vector: u16) {
    if let Some(q) = QUEUE_VECTOR.get(queue as usize) { q.store(vector, Ordering::Release); }
}

/// Send vector `v`'s message, or latch it in the PBA while it is masked.
fn msix_fire(v: usize) {
    if MSIX_FUNC_MASK.load(Ordering::Acquire)
        || MSIX_CTRL[v].load(Ordering::Acquire) & MSIX_ENTRY_MASKED != 0
    {
        MSIX_PENDING.fetch_or(1 << v, Ordering::AcqRel);
        MSIX_LATCHED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    MSIX_SENT.fetch_add(1, Ordering::Relaxed);
    // Message address: 0xFEE, destination ID in bits 19:12, destination mode
    // (logical) in bit 2. Data: vector in 7:0, delivery mode in 10:8.
    let addr = MSIX_ADDR[v].load(Ordering::Acquire);
    let data = MSIX_DATA[v].load(Ordering::Acquire);
    crate::microvm::cpu::svm::lapic::deliver_msg(
        (addr >> 12) as u8,
        addr & (1 << 2) != 0,
        ((data >> 8) & 0x7) as u8,
        data as u8,
        crate::microvm::cpu::svm::lapic::vcpu_on_this_core(),
    );
}

/// Unmasked entries with a pending bit fire now (PCI: on unmask).
fn msix_flush_pending() {
    if !MSIX_ENABLED.load(Ordering::Acquire) || MSIX_FUNC_MASK.load(Ordering::Acquire) { return; }
    for v in 0..MSIX_VECTORS {
        if MSIX_CTRL[v].load(Ordering::Acquire) & MSIX_ENTRY_MASKED == 0
            && MSIX_PENDING.fetch_and(!(1 << v), Ordering::AcqRel) & (1 << v) != 0
        {
            msix_fire(v);
        }
    }
}

/// Signal queue `q` by MSI-X. `false` = MSI-X is off, use INTx (IRQ 10).
pub fn msix_notify(q: u16) -> bool {
    if !msix_offered() || !MSIX_ENABLED.load(Ordering::Acquire) { return false; }
    let v = QUEUE_VECTOR.get(q as usize).map_or(0xFFFF, |x| x.load(Ordering::Acquire)) as usize;
    if v < MSIX_VECTORS { msix_fire(v); }
    true // with MSI-X on, a queue without a vector has no interrupt at all
}

/// MMIO into the MSI-X table / PBA (BAR0-relative `off`). `Some` if handled.
pub fn msix_mmio(off: u32, write: Option<u32>) -> Option<u32> {
    if !msix_offered() { return None; }
    if (MSIX_TABLE_OFF..MSIX_TABLE_OFF + 16 * MSIX_VECTORS as u32).contains(&off) {
        let rel = off - MSIX_TABLE_OFF;
        let v = (rel / 16) as usize;
        let field = rel % 16;
        return Some(match (field, write) {
            (0, None) => MSIX_ADDR[v].load(Ordering::Acquire) as u32,
            (4, None) => (MSIX_ADDR[v].load(Ordering::Acquire) >> 32) as u32,
            (8, None) => MSIX_DATA[v].load(Ordering::Acquire),
            (12, None) => MSIX_CTRL[v].load(Ordering::Acquire),
            (0, Some(x)) => {
                let a = MSIX_ADDR[v].load(Ordering::Acquire);
                MSIX_ADDR[v].store((a & !0xFFFF_FFFF) | x as u64, Ordering::Release); 0
            }
            (4, Some(x)) => {
                let a = MSIX_ADDR[v].load(Ordering::Acquire);
                MSIX_ADDR[v].store((a & 0xFFFF_FFFF) | (x as u64) << 32, Ordering::Release); 0
            }
            (8, Some(x)) => { MSIX_DATA[v].store(x, Ordering::Release); 0 }
            (12, Some(x)) => {
                MSIX_CTRL[v].store(x & MSIX_ENTRY_MASKED, Ordering::Release);
                msix_flush_pending();
                0
            }
            _ => 0,
        });
    }
    if (MSIX_PBA_OFF..MSIX_PBA_OFF + 8).contains(&off) {
        return Some(if write.is_none() && off == MSIX_PBA_OFF {
            MSIX_PENDING.load(Ordering::Acquire)
        } else { 0 });
    }
    None
}

/// Signal that the guest's virtio-net IRQ10 should be injected.
#[inline]
pub fn raise_irq() { NET_IRQ_PENDING.store(true, Ordering::Release); }

/// BSP: take the pending net-IRQ signal (clears it). True ⇒ inject IRQ10.
#[inline]
pub fn take_irq() -> bool { NET_IRQ_PENDING.swap(false, Ordering::AcqRel) }

/// Non-consuming peek — does the worker have an RX IRQ waiting? Used by the BSP's
/// lock-free warm halt-poll to break out and VMRUN (where `take_irq` injects it).
#[inline]
pub fn irq_pending() -> bool { NET_IRQ_PENDING.load(Ordering::Acquire) }

/// True while the data-plane worker owns the guest's rings. The vCPU reads it
/// to know that a TX kick belongs on the doorbell, not in its own hands.
static FULL_ACTIVE: AtomicBool = AtomicBool::new(false);
#[inline]
pub fn full_active() -> bool { FULL_ACTIVE.load(Ordering::Acquire) }
#[inline]
pub fn set_full_active(on: bool) { FULL_ACTIVE.store(on, Ordering::Release); }

/// The one microvm virtio-net device. `VirtioNet::new()` is `const`, so this
/// needs no lazy init. Persists across VM runs; `reset()` re-arms it at open.
static NET: Mutex<VirtioNet> = Mutex::new(VirtioNet::new());

/// Acquire the device for a multi-statement access (MMIO dispatch + service).
#[inline]
pub fn lock() -> MutexGuard<'static, VirtioNet> {
    NET.lock()
}

/// LOCK-FREE BAR0 range check for the vCPU's NPF/EPT exit dispatch. Every non-blk
/// MMIO exit used to take the device mutex JUST to range-check the gpa — which
/// collided with the off-vCPU worker holding it (the ACK-jitter contention). The
/// BAR is fixed at `BAR0_BASE` (Linux keeps it there; confirmed in the boot log),
/// so the range test needs no lock. The mutex is taken only AFTER a hit, for the
/// actual MMIO service.
#[inline]
pub fn bar0_in_range(gpa: u64) -> bool {
    gpa >= super::virtio_net_dev::BAR0_BASE
        && gpa < super::virtio_net_dev::BAR0_BASE + super::virtio_net_dev::BAR0_SIZE
}

/// Re-initialise to power-on state at VM open. The static outlives a single VM
/// run, so this restores the per-VM-fresh state that `PciBus::new()` used to
/// give the device when it lived inside the bus.
///
/// Device state only. The worker's attachment (`FULL_ACTIVE`, `WORKER_CORE`)
/// belongs to the worker's start/stop: it is started before `vm_open` and, on
/// its own core, registers before this runs — a reset here detached it.
pub fn reset() {
    *NET.lock() = VirtioNet::new();
    TX_AVAIL_GPA.store(0, Ordering::Release);
    msix_reset();
    ISR.store(0, Ordering::Release);
    NET_IRQ_PENDING.store(false, Ordering::Release);
    TX_KICK.store(false, Ordering::Release);
}
