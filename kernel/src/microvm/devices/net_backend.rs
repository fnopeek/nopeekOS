//! Off-vCPU network backend for the microvm (vhost-style).
//!
//! Owns the virtio-net device outside the VM's device lock (`VmShared::dev`)
//! so the data-plane worker fiber can run it — `inject_rx` (RX) +
//! `service_tx` (TX) — on its own core while the vCPUs only ring the notify
//! doorbell, without waiting for their device work. `GuestMem` is `&self`
//! (interior mutability), so the backend shares `&GuestMem`. Lock order: a
//! vCPU may take `VmShared::dev` then this lock; the backend takes only this
//! lock, so no cycle.
//!
//! Single instance: exactly one microvm runs at a time.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};
use super::virtio_net_dev::VirtioNet;

/// Guest TX kick (q1 notify) doorbell, set by the vCPU's net-MMIO exit
/// instead of servicing TX inline. The worker fiber drains it → service_tx +
/// tx_flush on its core, so RX and TX share one core / one NET path and no
/// cross-core NET-lock fight delays the vCPU's TX/ACK egress.
static TX_KICK: AtomicBool = AtomicBool::new(false);
/// Host core the worker fiber runs on, so a vCPU TX-kick can wake it out of its
/// RX-IRQ park promptly (else TX waits for the park timeout → slow ACKs).
static WORKER_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Wakes the data-plane worker wherever it runs; it may share its core with
/// a vCPU, so it is not woken through the per-core kick generation.
pub static NET_BELL: crate::smp::fiber::Doorbell = crate::smp::fiber::Doorbell::new();

/// Worker records its core at startup, for `cores` and placement.
pub fn set_worker_core(core: usize) { WORKER_CORE.store(core, Ordering::Release); }

/// Core the data-plane worker fiber runs on, if one is up. The tap's producer
/// side needs it to ring the doorbell.
pub fn worker_core() -> Option<usize> {
    match WORKER_CORE.load(Ordering::Acquire) {
        usize::MAX => None,
        c => Some(c),
    }
}

/// vCPU: the guest kicked TX (q1). Set the flag and, on the empty→set edge,
/// ring the worker's doorbell.
pub fn note_tx_kick() {
    if !TX_KICK.swap(true, Ordering::AcqRel) {
        NET_BELL.ring();
    }
}

/// Worker: take the TX-kick doorbell (clears it). True ⇒ run service_tx.
pub fn take_tx_kick() -> bool { TX_KICK.swap(false, Ordering::AcqRel) }

/// Lock-free peek (does not clear): is a guest TX kick pending? Used by the
/// data-plane busy-poll so a queued ACK is egressed without HLTing first.
pub fn tx_kick_pending() -> bool { TX_KICK.load(Ordering::Acquire) }

/// Guest RX/TX IRQ (IRQ10) raised by the net pump, which runs outside
/// `VmShared::dev` (it does not touch `VmShared`). The BSP folds this into its
/// `pending_irqs` at a safe injection point. A lock-free atomic, so the
/// pump needs no `VmShared` borrow and APs never block behind it on a
/// TLB-shootdown exit. Set by the pump (any caller), consumed by the BSP.
static NET_IRQ_PENDING: AtomicBool = AtomicBool::new(false);

/// virtio ISR status (read-to-clear). An atomic, not device state: the guest
/// reads it on every interrupt, and behind the device lock that read would
/// wait for the worker's whole RX batch.
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

// ── MSI-X: config + RX + TX vectors ──

pub const MSIX_VECTORS: usize = 3;
pub use super::msix::{TABLE_OFF as MSIX_TABLE_OFF, PBA_OFF as MSIX_PBA_OFF};
static MSIX: super::msix::Msix<MSIX_VECTORS> = super::msix::Msix::new();
pub fn msix_offered() -> bool { MSIX.offered() }

/// One line of MSI-X state for `cores`.
pub fn msix_report() -> Option<([u64; 2], alloc::string::String)> {
    let (n, table) = MSIX.report()?;
    Some((n, alloc::format!("qvec=[{:#x},{:#x}] {}",
        QUEUE_VECTOR[0].load(Ordering::Relaxed), QUEUE_VECTOR[1].load(Ordering::Relaxed), table)))
}

/// virtio `queue_msix_vector` per queue (RX 0, TX 1); 0xFFFF = none.
static QUEUE_VECTOR: [core::sync::atomic::AtomicU16; 2] =
    [const { core::sync::atomic::AtomicU16::new(0xFFFF) }; 2];

fn msix_reset() {
    MSIX.reset(super::msix::wanted());
    for q in QUEUE_VECTOR.iter() { q.store(0xFFFF, Ordering::Release); }
}

pub fn msix_msgctl() -> u16 { MSIX.msgctl() }
pub fn msix_set_msgctl(v: u16) { MSIX.set_msgctl(v) }

pub fn set_queue_vector(queue: u16, vector: u16) {
    if let Some(q) = QUEUE_VECTOR.get(queue as usize) { q.store(vector, Ordering::Release); }
}

/// Signal queue `q` by MSI-X. `false` = MSI-X is off, use INTx (IRQ 10).
pub fn msix_notify(q: u16) -> bool {
    MSIX.notify(QUEUE_VECTOR.get(q as usize).map_or(0xFFFF, |x| x.load(Ordering::Acquire)))
}

/// MMIO into the MSI-X table / PBA (BAR0-relative `off`). `Some` if handled.
pub fn msix_mmio(off: u32, write: Option<u32>) -> Option<u32> { MSIX.mmio(off, write) }

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

/// Lock-free BAR0 range check for the vCPU's NPF/EPT exit dispatch, so an
/// MMIO exit does not take the device mutex just to range-check the gpa
/// (which would collide with the off-vCPU worker holding it). The BAR is
/// fixed at `BAR0_BASE` (Linux keeps it there), so the range test needs no
/// lock. The mutex is taken only after a hit, for the actual MMIO service.
#[inline]
pub fn bar0_in_range(gpa: u64) -> bool {
    gpa >= super::virtio_net_dev::BAR0_BASE
        && gpa < super::virtio_net_dev::BAR0_BASE + super::virtio_net_dev::BAR0_SIZE
}

/// Re-initialise to power-on state at VM open. The static outlives a single
/// VM run, so this restores per-VM-fresh device state.
///
/// Device state only. The worker's attachment (`FULL_ACTIVE`, `WORKER_CORE`)
/// belongs to the worker's start/stop: it is started before `vm_open` and,
/// on its own core, registers before this runs, so resetting it here would
/// detach it.
pub fn reset() {
    *NET.lock() = VirtioNet::new();
    TX_AVAIL_GPA.store(0, Ordering::Release);
    msix_reset();
    ISR.store(0, Ordering::Release);
    NET_IRQ_PENDING.store(false, Ordering::Release);
    TX_KICK.store(false, Ordering::Release);
}
