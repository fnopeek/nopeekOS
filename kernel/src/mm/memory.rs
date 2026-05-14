//! Physical Memory Manager
//!
//! Bitmap frame allocator for 4KB pages.
//! Parses the Multiboot2 memory map to find available RAM.
//! Principle: deny by default — everything is "used" until the
//! memory map explicitly says a region is available.

use spin::Mutex;
use crate::kprintln;

pub const PAGE_SIZE: usize = 4096;

const MAX_MEMORY: usize = 64 * 1024 * 1024 * 1024; // 64GB (identity-mapped via 1GB huge pages)
const MAX_FRAMES: usize = MAX_MEMORY / PAGE_SIZE;
const BITMAP_SIZE: usize = MAX_FRAMES / 8; // 32KB

static ALLOCATOR: Mutex<FrameAllocator> = Mutex::new(FrameAllocator::new());

struct FrameAllocator {
    /// Bit=1 → used/reserved, Bit=0 → free
    bitmap: [u8; BITMAP_SIZE],
    free_count: usize,
    memory_top: usize,
}

impl FrameAllocator {
    const fn new() -> Self {
        FrameAllocator { bitmap: [0u8; BITMAP_SIZE], free_count: 0, memory_top: 0 }
    }

    fn mark_all_used(&mut self) {
        for byte in self.bitmap.iter_mut() { *byte = 0xFF; }
        self.free_count = 0;
    }

    fn set_free(&mut self, frame: usize) {
        if frame >= MAX_FRAMES { return; }
        let (byte, bit) = (frame / 8, frame % 8);
        if self.bitmap[byte] & (1 << bit) != 0 {
            self.bitmap[byte] &= !(1 << bit);
            self.free_count += 1;
        }
    }

    fn set_used(&mut self, frame: usize) {
        if frame >= MAX_FRAMES { return; }
        let (byte, bit) = (frame / 8, frame % 8);
        if self.bitmap[byte] & (1 << bit) == 0 {
            self.bitmap[byte] |= 1 << bit;
            if self.free_count > 0 { self.free_count -= 1; }
        }
    }

    fn mark_region_free(&mut self, base: u64, length: u64) {
        if length == 0 { return; }
        let start = ((base as usize) + PAGE_SIZE - 1) / PAGE_SIZE; // round up
        let end = ((base + length) as usize) / PAGE_SIZE;          // round down
        for frame in start..end.min(MAX_FRAMES) {
            self.set_free(frame);
            if frame >= self.memory_top { self.memory_top = frame + 1; }
        }
    }

    fn mark_region_used(&mut self, base: u64, length: u64) {
        if length == 0 { return; }
        let start = (base as usize) / PAGE_SIZE;                            // round down (conservative)
        let end = ((base + length) as usize + PAGE_SIZE - 1) / PAGE_SIZE;   // round up (conservative)
        for frame in start..end.min(MAX_FRAMES) {
            self.set_used(frame);
        }
    }

    fn allocate(&mut self) -> Option<u64> {
        let top_byte = (self.memory_top + 7) / 8;
        for byte_idx in 0..top_byte.min(BITMAP_SIZE) {
            if self.bitmap[byte_idx] == 0xFF { continue; }
            for bit in 0..8u8 {
                let frame = byte_idx * 8 + bit as usize;
                if frame >= self.memory_top { return None; }
                if self.bitmap[byte_idx] & (1 << bit) == 0 {
                    self.bitmap[byte_idx] |= 1 << bit;
                    self.free_count -= 1;
                    return Some((frame * PAGE_SIZE) as u64);
                }
            }
        }
        None
    }

    #[allow(dead_code)]
    fn deallocate(&mut self, addr: u64) {
        self.set_free((addr as usize) / PAGE_SIZE);
    }
}

unsafe extern "C" {
    static __heap_start: u8;
}

pub fn init(boot_info: &crate::boot_info::BootInfo) {
    let mut alloc = ALLOCATOR.lock();
    alloc.mark_all_used();

    // Walk the UEFI memory map. Conventional + BootServices Code/Data
    // are usable RAM (BootServices regions become ours after
    // ExitBootServices). LoaderCode/Data is our PE image and the
    // stack/heap UEFI allocated for us — leave those marked used so
    // we don't overwrite running code.
    for region in boot_info.usable_regions() {
        let length = region.page_count * PAGE_SIZE as u64;
        if length == 0 { continue; }
        alloc.mark_region_free(region.physical_start, length);
    }

    // First 1 MB on x86 is special — BIOS-compat MMIO holes (VGA at
    // 0xA0000, ROM at 0xC0000) live there even under UEFI. Carve it
    // out as a defence-in-depth guard; UEFI's map usually already
    // omits this range but firmware quirks happen.
    alloc.mark_region_used(0, 0x100000);

    // The kernel image itself is the EfiLoaderCode/EfiLoaderData
    // region in the UEFI map — already left as "used" by the loop
    // above. The boot_info struct lives in BSS (= part of the kernel
    // image), so no separate reservation needed.

    let free = alloc.free_count;
    let free_mb = free * PAGE_SIZE / (1024 * 1024);
    let total_mb = alloc.memory_top * PAGE_SIZE / (1024 * 1024);

    let _kernel_end_marker = unsafe { &__heap_start as *const u8 as u64 };

    kprintln!("[npk] Physical memory: {} MB free ({} frames), {} MB detected",
        free_mb, free, total_mb);
    kprintln!("[npk] UEFI regions: {} ({} usable)",
        boot_info.region_count, boot_info.usable_regions().count());
}

pub fn allocate_frame() -> Option<u64> {
    ALLOCATOR.lock().allocate()
}

#[allow(dead_code)]
pub fn deallocate_frame(addr: u64) {
    ALLOCATOR.lock().deallocate(addr);
}

/// Allocate `count` contiguous physical frames. Returns base physical address.
/// Allocate contiguous frames below a physical address limit.
/// `limit_bytes` = 0 means no limit (use all memory).
pub fn allocate_contiguous_below(count: usize, limit_bytes: u64) -> Option<u64> {
    if count == 0 { return None; }
    let mut alloc = ALLOCATOR.lock();
    let top = if limit_bytes > 0 {
        let max_frame = (limit_bytes / PAGE_SIZE as u64) as usize;
        max_frame.min(alloc.memory_top)
    } else {
        alloc.memory_top
    };
    if count > top { return None; }

    let mut start = top - count;
    loop {
        let mut ok = true;
        let mut skip_to = start;
        for i in 0..count {
            let frame = start + i;
            let (byte, bit) = (frame / 8, frame % 8);
            if alloc.bitmap[byte] & (1 << bit) != 0 {
                ok = false;
                if start == 0 { return None; }
                skip_to = if frame > count { frame - count } else { 0 };
                break;
            }
        }
        if ok {
            for i in 0..count {
                alloc.set_used(start + i);
            }
            return Some((start * PAGE_SIZE) as u64);
        }
        if skip_to >= start {
            if start == 0 { return None; }
            start -= 1;
        } else {
            start = skip_to;
        }
    }
}

pub fn allocate_contiguous(count: usize) -> Option<u64> {
    if count == 0 { return None; }
    let mut alloc = ALLOCATOR.lock();
    let top = alloc.memory_top;
    if count > top { return None; }

    // Search from top of memory downward — high RAM is almost always free,
    // avoids O(n*count) scan through busy low memory regions.
    let mut start = top - count;
    loop {
        let mut ok = true;
        let mut skip_to = start;
        for i in 0..count {
            let frame = start + i;
            let (byte, bit) = (frame / 8, frame % 8);
            if alloc.bitmap[byte] & (1 << bit) != 0 {
                // Frame is used — skip past it
                ok = false;
                if start == 0 { return None; }
                skip_to = if frame > count { frame - count } else { 0 };
                break;
            }
        }
        if ok {
            for i in 0..count {
                alloc.set_used(start + i);
            }
            return Some((start * PAGE_SIZE) as u64);
        }
        if skip_to >= start {
            if start == 0 { return None; }
            start -= 1;
        } else {
            start = skip_to;
        }
    }
}

/// Deallocate `count` contiguous physical frames starting at `base`.
pub fn deallocate_contiguous(base: u64, count: usize) {
    let mut alloc = ALLOCATOR.lock();
    let start_frame = (base as usize) / PAGE_SIZE;
    for i in 0..count {
        alloc.set_free(start_frame + i);
    }
}

pub fn reserve_region(base: u64, length: u64) {
    ALLOCATOR.lock().mark_region_used(base, length);
}

pub fn stats() -> (usize, usize) {
    let alloc = ALLOCATOR.lock();
    (alloc.free_count, alloc.free_count * PAGE_SIZE / (1024 * 1024))
}

