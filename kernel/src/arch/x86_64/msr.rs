//! Model-specific registers.

/// # Safety
/// `msr` must exist on this CPU (otherwise #GP), and reading it must have
/// no side effect the caller does not intend.
pub unsafe fn read(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: the caller's contract.
    unsafe {
        // No `nomem`: callers order MSR reads against memory accesses
        // (counter snapshots, errata sequences).
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
            options(nostack, preserves_flags));
    }
    (hi as u64) << 32 | lo as u64
}

/// # Safety
/// `msr` must exist and `v` must be a value the CPU accepts; an MSR
/// write can change paging, interrupts or the CPU's mode.
pub unsafe fn write(msr: u32, v: u64) {
    // SAFETY: the caller's contract.
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
            options(nostack, preserves_flags));
    }
}
