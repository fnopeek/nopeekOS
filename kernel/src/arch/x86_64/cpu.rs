//! Interrupt flag and halting on this core.

/// RFLAGS.IF.
const IF: u64 = 1 << 9;

/// This core's interrupt state as `irq_save` found it.
#[derive(Clone, Copy)]
pub struct IrqState(u64);

fn rflags() -> u64 {
    let rflags: u64;
    // SAFETY: reads RFLAGS, no other effect.
    unsafe { core::arch::asm!("pushfq; pop {}", out(reg) rflags, options(preserves_flags)) };
    rflags
}

/// Are interrupts enabled on this core?
#[inline]
pub fn irqs_enabled() -> bool {
    rflags() & IF != 0
}

/// Mask interrupts on this core.
#[inline]
pub fn irq_disable() {
    // SAFETY: clears IF; no memory or stack effect.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

/// Mask interrupts and return the state as it was, for `irq_restore`. For
/// a critical section that does not fit a closure (`without_interrupts`).
#[inline]
pub fn irq_save() -> IrqState {
    let rflags: u64;
    // SAFETY: save RFLAGS and clear IF; the caller restores via `irq_restore`.
    unsafe { core::arch::asm!("pushfq; pop {}; cli", out(reg) rflags) };
    IrqState(rflags)
}

/// Enable interrupts again if they were enabled in `state`.
#[inline]
pub fn irq_restore(state: IrqState) {
    if state.0 & IF != 0 {
        // SAFETY: IF was set when `irq_save` ran.
        unsafe { core::arch::asm!("sti") };
    }
}

/// Run `f` with this core's interrupts masked, restoring the previous state.
///
/// For a spin lock that an ISR on the same core may also take: held with
/// interrupts on, the timer IRQ lands inside the critical section and spins
/// on the lock its own core holds, deadlocking the core. Cheap
/// (pushfq/cli/sti), so keep `f` short; it cannot be interrupted.
#[inline]
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let s = irq_save();
    let r = f();
    irq_restore(s);
    r
}

/// Stop this core for good: interrupts off, halt, never resume.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: interrupts off, then wait; nothing resumes this core.
        unsafe { core::arch::asm!("cli; hlt", options(nomem, nostack)) };
    }
}
