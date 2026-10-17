//! Memory management
//!
//! Physical frame allocator, heap allocator, virtual memory (paging).

pub mod memory;
pub mod heap;
pub mod paging;
pub mod stack;

/// CLFLUSHOPT is present (CPUID.(EAX=7,ECX=0):EBX[23]); 0 = not asked yet.
static CLFLUSHOPT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

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
