//! Memory management
//!
//! Physical frame allocator, heap allocator, virtual memory (paging).

pub mod memory;
pub mod heap;
pub mod paging;
pub mod stack;

/// Write back and invalidate the cache lines of `len` bytes at the
/// identity-mapped address `addr` (Linux `clflush_cache_range`): for memory
/// a device writes or reads past the CPU caches.
pub fn clflush_range(addr: u64, len: u64) {
    if len == 0 {
        return;
    }
    // SAFETY: fences, no memory operands.
    unsafe { core::arch::asm!("mfence", options(nostack, preserves_flags)) };
    let mut line = addr & !63;
    while line < addr + len {
        // SAFETY: the caller names identity-mapped memory it may access;
        // clflush only writes the line back and drops it.
        unsafe { core::arch::asm!("clflush [{}]", in(reg) line, options(nostack, preserves_flags)) };
        line += 64;
    }
    // SAFETY: as above.
    unsafe { core::arch::asm!("mfence", options(nostack, preserves_flags)) };
}
