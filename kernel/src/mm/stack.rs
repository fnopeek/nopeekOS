//! Kernel stacks with a guard page.
//!
//! A stack carved out of the heap has heap below it: running off its end
//! overwrites whatever object lives there, silently. These stacks live in
//! their own address range instead, one slot each, and the page below every
//! stack is never mapped — overflowing it faults on the first write.
//!
//! The faulting push cannot store the exception frame either, so the fault
//! escalates to a double fault, which runs on its own IST stack
//! (`tss::init_core`) and reports.

use alloc::vec::Vec;
use spin::Mutex;

use super::memory::{self, PAGE_SIZE};
use super::paging::{self, PageFlags};

/// Start of the stack range: 64 TiB, far above the identity map (64 GiB)
/// and forge's instance and code ranges (they end below 17 TiB).
const REGION_BASE: u64 = 64 << 40;

/// Address space per slot: the guard page plus up to `SLOT_BYTES - PAGE`
/// of stack. Unused address space costs nothing.
const SLOT_BYTES: u64 = 16 << 20;

/// Slots handed back by `Drop`, reused before fresh ones.
static FREE: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static NEXT: Mutex<u64> = Mutex::new(0);

/// Whether `addr` is the guard page below one of these stacks.
pub fn is_guard(addr: u64) -> bool {
    addr >= REGION_BASE && (addr - REGION_BASE) % SLOT_BYTES < PAGE_SIZE as u64
}

pub struct KernelStack {
    slot: u64,
    /// Lowest usable address (just above the guard page).
    lo: u64,
    /// One past the highest usable address; the initial stack pointer.
    top: u64,
    phys: u64,
    pages: usize,
}

impl KernelStack {
    /// A zeroed stack of at least `bytes`, rounded up to whole pages.
    /// `None` when the size does not fit a slot or memory is short.
    pub fn new(bytes: usize) -> Option<KernelStack> {
        let pages = bytes.div_ceil(PAGE_SIZE).max(1);
        if (pages as u64 + 1) * PAGE_SIZE as u64 > SLOT_BYTES {
            return None;
        }
        let phys = memory::allocate_contiguous(pages)?;
        let slot = match FREE.lock().pop() {
            Some(s) => s,
            None => {
                let mut next = NEXT.lock();
                let s = *next;
                *next += 1;
                s
            }
        };
        let lo = REGION_BASE + slot * SLOT_BYTES + PAGE_SIZE as u64;
        let flags = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        for i in 0..pages {
            let off = (i * PAGE_SIZE) as u64;
            if paging::map_page(lo + off, phys + off, flags).is_err() {
                for j in 0..i {
                    let _ = paging::unmap_page(lo + (j * PAGE_SIZE) as u64);
                }
                memory::deallocate_contiguous(phys, pages);
                FREE.lock().push(slot);
                return None;
            }
        }
        let top = lo + (pages * PAGE_SIZE) as u64;
        // SAFETY: lo..top was mapped writable just above, to frames this
        // stack owns exclusively.
        unsafe { core::ptr::write_bytes(lo as *mut u8, 0, pages * PAGE_SIZE) };
        Some(KernelStack { slot, lo, top, phys, pages })
    }

    pub fn lo(&self) -> u64 { self.lo }
    pub fn top(&self) -> u64 { self.top }
}

impl Drop for KernelStack {
    /// The owner guarantees nothing runs on the stack any more.
    fn drop(&mut self) {
        for i in 0..self.pages {
            let _ = paging::unmap_page(self.lo + (i * PAGE_SIZE) as u64);
        }
        memory::deallocate_contiguous(self.phys, self.pages);
        FREE.lock().push(self.slot);
    }
}
