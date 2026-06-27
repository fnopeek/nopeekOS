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

use core::sync::atomic::{AtomicBool, Ordering};
use spin::{Mutex, MutexGuard};
use super::virtio_net_dev::VirtioNet;

/// Guest RX/TX IRQ (IRQ10) raised by the net pump, which now runs OUTSIDE
/// `VM_BIG_LOCK` (it no longer touches `VmShared`). The BSP folds this into its
/// `pending_irqs` at a safe injection point. A lock-free atomic instead of
/// `sh.pending_irqs |= 1<<10` so the pump needs no `VmShared` borrow → APs no
/// longer block behind the BSP pump on a TLB-shootdown exit (the csd_lock_wait
/// root). Set by the pump (any caller), consumed by the BSP.
static NET_IRQ_PENDING: AtomicBool = AtomicBool::new(false);

/// Signal that the guest's virtio-net IRQ10 should be injected.
#[inline]
pub fn raise_irq() { NET_IRQ_PENDING.store(true, Ordering::Release); }

/// BSP: take the pending net-IRQ signal (clears it). True ⇒ inject IRQ10.
#[inline]
pub fn take_irq() -> bool { NET_IRQ_PENDING.swap(false, Ordering::AcqRel) }

/// The one microvm virtio-net device. `VirtioNet::new()` is `const`, so this
/// needs no lazy init. Persists across VM runs; `reset()` re-arms it at open.
static NET: Mutex<VirtioNet> = Mutex::new(VirtioNet::new());

/// Acquire the device for a multi-statement access (MMIO dispatch + service).
#[inline]
pub fn lock() -> MutexGuard<'static, VirtioNet> {
    NET.lock()
}

/// Re-initialise to power-on state at VM open. The static outlives a single VM
/// run, so this restores the per-VM-fresh state that `PciBus::new()` used to
/// give the device when it lived inside the bus.
pub fn reset() {
    *NET.lock() = VirtioNet::new();
    NET_IRQ_PENDING.store(false, Ordering::Release);
}
