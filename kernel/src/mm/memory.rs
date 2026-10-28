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
    regions: &'static [crate::boot_info::MemoryRegion],
    /// Set once the map is in; until then only MMIO-typed regions pass.
    complete: bool,
}

static FIRMWARE_MAP: Mutex<FirmwareMap> = Mutex::new(FirmwareMap {
    regions: &[],
    complete: false,
});

/// `(base, length, uefi type)` of a firmware region.
fn span(r: &crate::boot_info::MemoryRegion) -> (u64, u64, u32) {
    (r.physical_start, r.page_count.saturating_mul(PAGE_SIZE as u64), r.uefi_type)
}

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
    let found = map.regions.iter().map(span)
        .find(|(b, l, _)| addr >= *b && addr < b.saturating_add(*l));
    match found {
        Some((_, _, t)) => !is_ram_type(t),
        None => map.complete,
    }
}

/// Whether `[base, base + len)` lies inside one region the firmware typed
/// as not RAM, such as the frame buffer an APU's firmware carves out of
/// memory. Only such a region may be mapped by a module in one large
/// piece: its size is the firmware's, not the module's.
pub fn is_firmware_device_region(base: u64, len: u64) -> bool {
    let Some(end) = base.checked_add(len) else { return false };
    // SAFETY: linker-provided symbols; only their addresses are taken.
    let (image_lo, image_hi) = unsafe {
        (&__image_base as *const u8 as u64, &__heap_start as *const u8 as u64)
    };
    if base < image_hi && image_lo < end { return false; }
    let map = FIRMWARE_MAP.lock();
    map.regions.iter().map(span)
        .any(|(b, l, t)| !is_ram_type(t) && base >= b && end <= b.saturating_add(l))
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
        m.regions = boot_info.regions();
        m.complete = !m.regions.is_empty();
    }

    let free = alloc.free_count;
    USABLE_FRAMES.store(free, core::sync::atomic::Ordering::Relaxed);
    let free_mb = free * PAGE_SIZE / (1024 * 1024);
    let total_mb = alloc.memory_top * PAGE_SIZE / (1024 * 1024);

    // SAFETY: only the address of the linker symbol is taken; it is never read.
    let _kernel_end_marker = unsafe { &__heap_start as *const u8 as u64 };

    kprintln!("[npk] Physical memory: {} MB free ({} frames), {} MB detected",
        free_mb, free, total_mb);
    crate::kdebug!("[npk] UEFI regions: {} ({} usable)",
        boot_info.region_count, boot_info.usable_regions().count());
}

pub fn allocate_frame() -> Option<u64> {
    with_reclaim(1, || ALLOCATOR.lock().allocate())
}

/// What gives memory back when an allocation finds none: called with the
/// frames wanted, returns the frames it freed (Linux `struct shrinker`).
/// A shrinker must not allocate, and must not wait for a lock its caller
/// may hold.
pub type Shrinker = fn(usize) -> usize;

static SHRINKERS: spin::Mutex<alloc::vec::Vec<Shrinker>> = spin::Mutex::new(alloc::vec::Vec::new());

pub fn register_shrinker(s: Shrinker) {
    SHRINKERS.lock().push(s);
}

/// Ask the shrinkers for `want` frames; returns how many they freed.
pub fn reclaim(want: usize) -> usize {
    let Some(list) = SHRINKERS.try_lock() else { return 0 };
    let mut freed = 0;
    for s in list.iter() {
        if freed >= want { break; }
        freed += s(want - freed);
    }
    freed
}

/// `alloc`, and once more after the shrinkers freed something.
fn with_reclaim<T>(want: usize, alloc: impl Fn() -> Option<T>) -> Option<T> {
    alloc().or_else(|| if reclaim(want) > 0 { alloc() } else { None })
}

#[allow(dead_code)]
pub fn deallocate_frame(addr: u64) {
    ALLOCATOR.lock().deallocate(addr);
}

/// Allocate contiguous frames below a physical address limit.
/// `limit_bytes` = 0 means no limit (use all memory).
pub fn allocate_contiguous_below(count: usize, limit_bytes: u64) -> Option<u64> {
    with_reclaim(count, || contiguous_below(count, limit_bytes))
}

fn contiguous_below(count: usize, limit_bytes: u64) -> Option<u64> {
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

/// Allocate `count` contiguous frames whose base is a multiple of `align`
/// frames (a power of two), wholly below `limit_bytes`: what Linux's
/// buddy allocator gives a `GFP_DMA32` request of a power-of-two size, for
/// hardware that wants its tables naturally aligned under 4 GiB.
pub fn allocate_contiguous_aligned_below(count: usize, align: usize, limit_bytes: u64) -> Option<u64> {
    with_reclaim(count, || contiguous_aligned_below(count, align, limit_bytes))
}

fn contiguous_aligned_below(count: usize, align: usize, limit_bytes: u64) -> Option<u64> {
    if count == 0 || align == 0 || !align.is_power_of_two() {
        return None;
    }
    let mut alloc = ALLOCATOR.lock();
    let top = ((limit_bytes / PAGE_SIZE as u64) as usize).min(alloc.memory_top);
    if count > top {
        return None;
    }
    let mut start = (top - count) & !(align - 1);
    loop {
        if (start..start + count).all(|f| alloc.bitmap[f / 8] & (1 << (f % 8)) == 0) {
            for f in start..start + count {
                alloc.set_used(f);
            }
            return Some((start * PAGE_SIZE) as u64);
        }
        if start < align {
            return None;
        }
        start -= align;
    }
}

/// Allocate `count` contiguous physical frames, searching from the top of
/// memory, where RAM is almost always free. Returns the base address.
pub fn allocate_contiguous(count: usize) -> Option<u64> {
    allocate_contiguous_below(count, 0)
}

/// Allocate `count` contiguous frames whose base is a multiple of `align`
/// frames, searching from the top of memory. `align` is a power of two and
/// both are multiples of 8, so every candidate is whole bitmap bytes and is
/// free iff those bytes are zero.
pub fn allocate_contiguous_aligned(count: usize, align: usize) -> Option<u64> {
    with_reclaim(count, || contiguous_aligned(count, align))
}

fn contiguous_aligned(count: usize, align: usize) -> Option<u64> {
    if count == 0 || count % 8 != 0 || align < 8 || !align.is_power_of_two() {
        return None;
    }
    let mut alloc = ALLOCATOR.lock();
    let top = alloc.memory_top.min(MAX_FRAMES);
    if count > top { return None; }
    let mut start = (top - count) & !(align - 1);
    loop {
        if alloc.bitmap[start / 8..(start + count) / 8].iter().all(|&b| b == 0) {
            for i in 0..count {
                alloc.set_used(start + i);
            }
            return Some((start * PAGE_SIZE) as u64);
        }
        if start < align { return None; }
        start -= align;
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

/// Whether any page of `[base, base + length)` is RAM in the firmware map
/// (a type the allocator may hand out).
pub fn firmware_ram_in(base: u64, length: u64) -> bool {
    let map = FIRMWARE_MAP.lock();
    map.regions.iter().map(span).any(|(b, l, t)| {
        is_ram_type(t) && b < base.saturating_add(length) && base < b.saturating_add(l)
    })
}

pub fn reserve_region(base: u64, length: u64) {
    ALLOCATOR.lock().mark_region_used(base, length);
}

/// Frames free when the allocator came up: the RAM this machine has for
/// everything after the kernel image.
static USABLE_FRAMES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// RAM the allocator manages, in bytes (`USABLE_FRAMES`).
pub fn usable_bytes() -> u64 {
    (USABLE_FRAMES.load(core::sync::atomic::Ordering::Relaxed) * PAGE_SIZE) as u64
}

/// The top of physical memory: one past the highest frame there is.
pub fn memory_top_bytes() -> u64 {
    (ALLOCATOR.lock().memory_top * PAGE_SIZE) as u64
}

pub fn stats() -> (usize, usize) {
    let alloc = ALLOCATOR.lock();
    (alloc.free_count, alloc.free_count * PAGE_SIZE / (1024 * 1024))
}

