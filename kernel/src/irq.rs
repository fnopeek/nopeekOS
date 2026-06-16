//! Device-interrupt subsystem: MSI-X routing → LAPIC vector → fiber wake.
//!
//! The host kernel has no IOAPIC and the legacy 8259 PIC is fully masked
//! (re-init traps via SMI on HP/Insyde firmware — see `interrupts::init`).
//! So real-hardware device interrupts are routed via **MSI-X**: we program
//! a device's MSI-X table entry to deliver to a chosen LAPIC vector with a
//! chosen destination APIC ID. MSI-X writes go straight to the LAPIC
//! (message address `0xFEE0_0000 | apic<<12`), bypassing the PIC/IOAPIC
//! entirely, so the HP firmware quirk never bites.
//!
//! The ISR (in `interrupts.rs`) does the minimum: bump a per-vector atomic
//! fired-count + LAPIC EOI. A driver fiber parks via `wait()` until the
//! count advances (or a timeout). Crucially we target the device's MSI-X at
//! the APIC of the core running the driver fiber, so the interrupt itself
//! wakes that core out of HLT → the worker loop re-runs the scheduler →
//! the parked fiber resumes. No polling, no IPI: the IRQ is the wake.
//!
//! This closes the fiber scheduler's open "event-wake" hole and is the
//! foundation every poll-based HW driver (NVMe/NIC/audio_hda/xHCI) migrates
//! onto. First beneficiary: NVMe completion.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::drivers::pci::{self, PciAddr};
use crate::interrupts::{DEVICE_IRQ_VEC_BASE, DEVICE_IRQ_VEC_COUNT};

/// Per-vector fired count, bumped by the ISR. Indexed by IDT vector (full
/// 256 so the ISR indexes without a bounds branch). A driver snapshots the
/// count before submitting a command, then waits for it to advance.
static IRQ_FIRED: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Bump the fired count for `vector`. Called ONLY from the device ISR.
/// Release so a parked fiber observing the advance also sees the device
/// data the IRQ signalled.
#[inline]
pub fn note_fired(vector: u8) {
    IRQ_FIRED[vector as usize].fetch_add(1, Ordering::Release);
}

/// Current fired count for `vector`. Acquire pairs with `note_fired`.
#[inline]
pub fn fired_count(vector: u8) -> u64 {
    IRQ_FIRED[vector as usize].load(Ordering::Acquire)
}

/// Next free device-IRQ vector slot. Vectors are never freed (a driver lives
/// for the boot); the pool (`DEVICE_IRQ_VEC_COUNT`) is sized for all expected
/// HW drivers.
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Allocate a fresh LAPIC vector from the device-IRQ pool, or None if
/// exhausted. The matching IDT entry is already installed (`interrupts::init`).
pub fn alloc_vector() -> Option<u8> {
    let s = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    if s < DEVICE_IRQ_VEC_COUNT {
        Some(DEVICE_IRQ_VEC_BASE + s as u8)
    } else {
        None
    }
}

/// Snapshot the fired count for `vector` BEFORE submitting the device
/// command (ringing the doorbell). Pass the returned token to `wait`. This
/// closes the lost-wakeup window: an IRQ that fires between submit and park
/// still advances the count past the snapshot, so `wait` returns at once.
#[inline]
pub fn arm(vector: u8) -> u64 {
    fired_count(vector)
}

/// Park the current fiber until `vector` fires (its count moves past `since`)
/// or `timeout_ms` elapses. Returns true if the IRQ fired, false on timeout.
/// MUST be called from inside a fiber (returns false otherwise — the caller
/// should fall back to polling).
pub fn wait(vector: u8, since: u64, timeout_ms: u64) -> bool {
    crate::smp::fiber::irq_wait(vector, since, timeout_ms)
}

/// Allocate a vector and program `dev`'s MSI-X table `entry` to deliver it to
/// the CURRENT core's LAPIC. Call this from the driver fiber so the IRQ wakes
/// exactly the core that will service it. Returns the vector, or None if the
/// device has no usable MSI-X capability or the vector pool is exhausted.
pub fn register(dev: PciAddr, entry: u16) -> Option<u8> {
    let vector = alloc_vector()?;
    let dest = crate::interrupts::current_apic_id();
    if pci::program_msix(dev, entry, vector, dest) {
        Some(vector)
    } else {
        None
    }
}
