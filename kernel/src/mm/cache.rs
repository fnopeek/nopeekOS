//! Cache line write-back for memory a device reads or writes.

/// CLFLUSHOPT is present (CPUID.(EAX=7,ECX=0):EBX[23]); 0 = not asked yet.
#[cfg(target_arch = "x86_64")]
static CLFLUSHOPT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

#[cfg(target_arch = "x86_64")]
fn has_clflushopt() -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    match CLFLUSHOPT.load(Relaxed) {
        0 => {
            let yes = core::arch::x86_64::__cpuid_count(7, 0).ebx & (1 << 23) != 0;
            CLFLUSHOPT.store(if yes { 2 } else { 1 }, Relaxed);
            yes
        }
        v => v == 2,
    }
}

/// Write back and invalidate the cache lines of `len` bytes at the
/// identity-mapped address `addr` (Linux `clflush_cache_range`): for memory
/// a device writes or reads past the CPU caches. CLFLUSHOPT where the CPU
/// has it — unordered among itself, so the lines flush in parallel, and the
/// fences around the loop order it against everything else.
#[cfg(target_arch = "x86_64")]
pub fn clflush_range(addr: u64, len: u64) {
    if len == 0 {
        return;
    }
    // SAFETY: fences, no memory operands.
    unsafe { core::arch::asm!("mfence", options(nostack, preserves_flags)) };
    let mut line = addr & !63;
    if has_clflushopt() {
        while line < addr + len {
            // SAFETY: as below; clflushopt has the same effect on the line.
            unsafe { core::arch::asm!("clflushopt [{}]", in(reg) line, options(nostack, preserves_flags)) };
            line += 64;
        }
    } else {
        while line < addr + len {
            // SAFETY: the caller names identity-mapped memory it may access;
            // clflush only writes the line back and drops it.
            unsafe { core::arch::asm!("clflush [{}]", in(reg) line, options(nostack, preserves_flags)) };
            line += 64;
        }
    }
    // SAFETY: as above.
    unsafe { core::arch::asm!("mfence", options(nostack, preserves_flags)) };
}

/// Clean and invalidate the data cache lines of `len` bytes at the
/// identity-mapped address `addr` to the point of coherency (Linux arm64
/// `dcache_clean_inval_poc`): for memory a device writes or reads past the
/// CPU caches. The line size comes from CTR_EL0.DminLine.
#[cfg(target_arch = "aarch64")]
pub fn clflush_range(addr: u64, len: u64) {
    if len == 0 {
        return;
    }
    let ctr: u64;
    // SAFETY: reads CTR_EL0, no other effect.
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let line = 4u64 << ((ctr >> 16) & 0xF);
    // SAFETY: a barrier, no memory effect of its own.
    unsafe { core::arch::asm!("dsb ish", options(nostack, preserves_flags)) };
    let mut a = addr & !(line - 1);
    while a < addr + len {
        // SAFETY: the caller names identity-mapped memory it may access;
        // cache maintenance by address changes no data.
        unsafe { core::arch::asm!("dc civac, {}", in(reg) a, options(nostack, preserves_flags)) };
        a += line;
    }
    // SAFETY: a barrier, no memory effect of its own.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}
