//! Interrupt mask and halting on this core.

/// PSTATE.I in DAIF.
const DAIF_I: u64 = 1 << 7;

/// This core's interrupt state as `irq_save` found it.
#[derive(Clone, Copy)]
pub struct IrqState(u64);

fn daif() -> u64 {
    let v: u64;
    // SAFETY: reads DAIF, no other effect.
    unsafe { core::arch::asm!("mrs {}, DAIF", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Are interrupts enabled on this core?
#[inline]
pub fn irqs_enabled() -> bool {
    daif() & DAIF_I == 0
}

/// Mask interrupts on this core.
#[inline]
pub fn irq_disable() {
    // SAFETY: sets PSTATE.I; no memory or stack effect.
    unsafe { core::arch::asm!("msr DAIFSet, #2", options(nomem, nostack, preserves_flags)) };
}

/// Mask interrupts and return the state as it was, for `irq_restore`.
#[inline]
pub fn irq_save() -> IrqState {
    let s = IrqState(daif());
    irq_disable();
    s
}

/// Enable interrupts again if they were enabled in `state`.
#[inline]
pub fn irq_restore(state: IrqState) {
    if state.0 & DAIF_I == 0 {
        // SAFETY: interrupts were enabled when `irq_save` ran.
        unsafe { core::arch::asm!("msr DAIFClr, #2", options(nomem, nostack, preserves_flags)) };
    }
}

/// Run `f` with this core's interrupts masked, restoring the previous state.
#[inline]
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let s = irq_save();
    let r = f();
    irq_restore(s);
    r
}

/// Stop this core for good: everything masked, wait, never resume.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: masks all exceptions, then waits; nothing resumes this core.
        unsafe { core::arch::asm!("msr DAIFSet, #0xf", "wfi", options(nomem, nostack)) };
    }
}

/// This core's logical index (0 = the boot core), as `set_core_index`
/// stored it. Linux keeps its per-CPU offset in the same register; the
/// MPIDR itself is no index (the Pi 5 numbers its cores in affinity 1).
pub fn current_core() -> usize {
    let v: u64;
    // SAFETY: reads TPIDR_EL1 (TPIDR_EL2 under VHE), no other effect.
    unsafe { core::arch::asm!("mrs {}, TPIDR_EL1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v as usize
}

/// Record this core's logical index for `current_core`.
pub fn set_core_index(i: usize) {
    // SAFETY: TPIDR_EL1 holds nothing else in this kernel.
    unsafe { core::arch::asm!("msr TPIDR_EL1, {}", in(reg) i as u64, options(nomem, nostack, preserves_flags)) };
}

/// This core's affinity fields of MPIDR_EL1 (Aff3..Aff0), as the device
/// tree's `cpu` nodes give them in `reg`.
pub fn mpidr() -> u64 {
    let v: u64;
    // SAFETY: reads MPIDR_EL1, no other effect.
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v & 0xff_00ff_ffff
}

/// The other cores are not started yet; nothing to stop.
pub fn stop_other_cores() {}

/// The exception level the kernel runs at (1, or 2 with VHE).
pub fn exception_level() -> u8 {
    let el: u64;
    // SAFETY: reads CurrentEL, no other effect.
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags)) };
    ((el >> 2) & 3) as u8
}
