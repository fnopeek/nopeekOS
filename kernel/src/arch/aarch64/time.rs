//! The generic timer's virtual counter as the kernel's counter.

use core::sync::atomic::{AtomicU64, Ordering};

/// The counter: CNTVCT_EL0, read after an ISB so it is not taken early.
#[inline]
pub fn counter() -> u64 {
    let v: u64;
    // SAFETY: reads the virtual count, no other effect.
    unsafe { core::arch::asm!("isb", "mrs {}, CNTVCT_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

static FREQ: AtomicU64 = AtomicU64::new(0);

/// Counter frequency in Hz.
#[inline]
pub fn counter_hz() -> u64 {
    FREQ.load(Ordering::Relaxed)
}

/// Take the frequency the firmware programmed into CNTFRQ_EL0.
pub fn calibrate_counter() {
    let f: u64;
    // SAFETY: reads CNTFRQ_EL0, no other effect.
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) f, options(nomem, nostack, preserves_flags)) };
    FREQ.store(f & 0xffff_ffff, Ordering::Relaxed);
}
