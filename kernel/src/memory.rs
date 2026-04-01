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

extern "C" {
    static __heap_start: u8;
}

pub fn init(multiboot_info_addr: u32) {
    let mut alloc = ALLOCATOR.lock();

    alloc.mark_all_used();

    if multiboot_info_addr == 0 {
        kprintln!("[npk] WARNING: No Multiboot2 info, memory manager disabled");
        return;
    }

    parse_memory_map(multiboot_info_addr as usize, &mut alloc);

    // Reserve regions that must not be allocated
    alloc.mark_region_used(0, 0x100000); // First 1MB: BIOS, VGA, ROM

    let kernel_end = unsafe { &__heap_start as *const u8 as u64 };
    let kernel_size = kernel_end - 0x100000;
    alloc.mark_region_used(0x100000, kernel_size);

    // SAFETY: first u32 of Multiboot2 info is total size
    let mb_size = unsafe { *(multiboot_info_addr as *const u32) } as u64;
    alloc.mark_region_used(multiboot_info_addr as u64, mb_size);

    let free = alloc.free_count;
    let free_mb = free * PAGE_SIZE / (1024 * 1024);
    let total_mb = alloc.memory_top * PAGE_SIZE / (1024 * 1024);

    kprintln!("[npk] Physical memory: {} MB free ({} frames), {} MB detected",
        free_mb, free, total_mb);
    kprintln!("[npk] Kernel footprint: {} KB", kernel_size / 1024);
}

pub fn allocate_frame() -> Option<u64> {
    ALLOCATOR.lock().allocate()
}

#[allow(dead_code)]
pub fn deallocate_frame(addr: u64) {
    ALLOCATOR.lock().deallocate(addr);
}

/// Allocate `count` contiguous physical frames. Returns base physical address.
pub fn allocate_contiguous(count: usize) -> Option<u64> {
    if count == 0 { return None; }
    let mut alloc = ALLOCATOR.lock();
    let top = alloc.memory_top;
    if count > top { return None; }

    'outer: for start in 0..top - count + 1 {
        for i in 0..count {
            let frame = start + i;
            let (byte, bit) = (frame / 8, frame % 8);
            if alloc.bitmap[byte] & (1 << bit) != 0 {
                continue 'outer;
            }
        }
        for i in 0..count {
            alloc.set_used(start + i);
        }
        return Some((start * PAGE_SIZE) as u64);
    }
    None
}

pub fn reserve_region(base: u64, length: u64) {
    ALLOCATOR.lock().mark_region_used(base, length);
}

pub fn stats() -> (usize, usize) {
    let alloc = ALLOCATOR.lock();
    (alloc.free_count, alloc.free_count * PAGE_SIZE / (1024 * 1024))
}

/// Parse Multiboot2 memory map tag (type 6) and mark available regions
fn parse_memory_map(info_addr: usize, alloc: &mut FrameAllocator) {
    // SAFETY: info_addr is the GRUB-provided Multiboot2 info address,
    // identity-mapped and readable. We validate bounds before each read.
    let total_size = unsafe { *(info_addr as *const u32) } as usize;
    if total_size == 0 || total_size > 1024 * 1024 { return; }

    let mut offset = 8;

    while offset + 8 <= total_size {
        let tag_addr = info_addr + offset;
        let tag_type = unsafe { *(tag_addr as *const u32) };
        let tag_size = unsafe { *((tag_addr + 4) as *const u32) } as usize;

        if tag_type == 0 || tag_size < 8 { break; }

        if tag_type == 6 && tag_size >= 16 {
            let entry_size = unsafe { *((tag_addr + 8) as *const u32) } as usize;

            if entry_size >= 24 {
                let entries_start = tag_addr + 16;
                let entries_end = tag_addr + tag_size;
                let mut entry_addr = entries_start;

                while entry_addr + entry_size <= entries_end {
                    let base = unsafe { *(entry_addr as *const u64) };
                    let length = unsafe { *((entry_addr + 8) as *const u64) };
                    let mem_type = unsafe { *((entry_addr + 16) as *const u32) };

                    if mem_type == 1 && length > 0 {
                        alloc.mark_region_free(base, length);
                    }
                    entry_addr += entry_size;
                }
            }
        }

        offset += (tag_size + 7) & !7; // Tags are 8-byte aligned
    }
}
