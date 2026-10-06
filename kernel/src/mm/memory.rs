//! Physical Memory Manager
//!
//! Bitmap frame allocator for 4KB pages.
//! Parses the UEFI memory map to find available RAM.
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
    /// No free frame lies in a bitmap byte below this one; `allocate` starts
    /// here instead of at frame 0.
    next_byte: usize,
}

impl FrameAllocator {
    const fn new() -> Self {
        FrameAllocator { bitmap: [0u8; BITMAP_SIZE], free_count: 0, memory_top: 0, next_byte: 0 }
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
            self.next_byte = self.next_byte.min(byte);
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
        let top_byte = ((self.memory_top + 7) / 8).min(BITMAP_SIZE);
        for byte_idx in self.next_byte..top_byte {
            let b = self.bitmap[byte_idx];
            if b == 0xFF { continue; }
            self.next_byte = byte_idx;
            let frame = byte_idx * 8 + b.trailing_ones() as usize;
            if frame >= self.memory_top { return None; }
            self.bitmap[byte_idx] |= 1 << (frame % 8);
            self.free_count -= 1;
            return Some((frame * PAGE_SIZE) as u64);
        }
        self.next_byte = top_byte;
        None
    }

    #[allow(dead_code)]
    fn deallocate(&mut self, addr: u64) {
        self.set_free((addr as usize) / PAGE_SIZE);
    }
}

unsafe extern "C" {
    static __image_base: u8;
    static __heap_start: u8;
}

/// The whole firmware memory map as `(base, length, uefi type)`, and
/// whether it was cut short. Answers one question: may a module map or
/// read this physical address?
struct FirmwareMap {
    regions: [(u64, u64, u32); crate::boot_info::MAX_MEMORY_REGIONS],
    count: usize,
    truncated: bool,
}

static FIRMWARE_MAP: Mutex<FirmwareMap> = Mutex::new(FirmwareMap {
    regions: [(0, 0, 0); crate::boot_info::MAX_MEMORY_REGIONS],
    count: 0,
    truncated: true,
});

/// Memory types that hold RAM someone owns: the kernel image (loader code
/// and data), the frames the kernel hands out (boot services, conventional),
/// firmware runtime services, and persistent or unusable RAM.
fn is_ram_type(t: u32) -> bool {
    use crate::boot_info::*;
    matches!(t,
        UEFI_LOADER_CODE | UEFI_LOADER_DATA
        | UEFI_BOOT_SERVICES_CODE | UEFI_BOOT_SERVICES_DATA
        | UEFI_RUNTIME_SERVICES_CODE | UEFI_RUNTIME_SERVICES_DATA
        | UEFI_CONVENTIONAL_MEMORY | UEFI_UNUSABLE_MEMORY
        | UEFI_PAL_CODE | UEFI_PERSISTENT_MEMORY)
}

/// Whether `addr` is a device or firmware window a module may map or read:
/// MMIO, reserved, ACPI tables and NVS (where ACPI OpRegions live), or no
/// region at all. Never RAM and never the kernel image, whatever the map
/// says. Fails closed: without a complete map only regions the firmware
/// typed as MMIO pass.
pub fn is_device_window(addr: u64) -> bool {
    // SAFETY: linker-provided symbols; only their addresses are taken.
    let (image_lo, image_hi) = unsafe {
        (&__image_base as *const u8 as u64, &__heap_start as *const u8 as u64)
    };
    if addr >= image_lo && addr < image_hi { return false; }
    let map = FIRMWARE_MAP.lock();
    let found = map.regions[..map.count].iter()
        .find(|(b, l, _)| addr >= *b && addr < b.saturating_add(*l));
    match found {
        Some(&(_, _, t)) => !is_ram_type(t),
        None => !map.truncated,
    }
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

    // Keep the whole map while we still have it.
    {
        let mut m = FIRMWARE_MAP.lock();
        let n = (boot_info.region_count as usize).min(m.regions.len());
        for (dst, r) in m.regions.iter_mut().zip(&boot_info.regions[..n]) {
            *dst = (r.physical_start, r.page_count * PAGE_SIZE as u64, r.uefi_type);
        }
        m.count = n;
        // A full table may have lost descriptors at the stub.
        m.truncated = n >= crate::boot_info::MAX_MEMORY_REGIONS;
    }

    let free = alloc.free_count;
    let free_mb = free * PAGE_SIZE / (1024 * 1024);
    let total_mb = alloc.memory_top * PAGE_SIZE / (1024 * 1024);

    let _kernel_end_marker = unsafe { &__heap_start as *const u8 as u64 };

    kprintln!("[npk] Physical memory: {} MB free ({} frames), {} MB detected",
        free_mb, free, total_mb);
    crate::kdebug!("[npk] UEFI regions: {} ({} usable)",
        boot_info.region_count, boot_info.usable_regions().count());
}

pub fn allocate_frame() -> Option<u64> {
    ALLOCATOR.lock().allocate()
}

#[allow(dead_code)]
pub fn deallocate_frame(addr: u64) {
    ALLOCATOR.lock().deallocate(addr);
}

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

/// Allocate `count` contiguous physical frames, searching from the top of
/// memory, where RAM is almost always free. Returns the base address.
pub fn allocate_contiguous(count: usize) -> Option<u64> {
    allocate_contiguous_below(count, 0)
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

