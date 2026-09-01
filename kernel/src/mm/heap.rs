//! Heap Allocator
//!
//! Linked-list free-block allocator enabling Vec, Box, String via alloc.
//! First-fit with block splitting, local coalescing, and auto-growth.
//! Starts with 64MB, grows on demand in 64MB chunks from the frame allocator.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use spin::Mutex;
use crate::kprintln;

const INITIAL_HEAP: usize = 64 * 1024 * 1024;       // 64MB initial
const GROW_CHUNK: usize = 64 * 1024 * 1024;          // 64MB growth increments
const MAX_HEAP: usize = 2 * 1024 * 1024 * 1024;      // 2GB ceiling
const MAX_REGIONS: usize = 32;
const BLOCK_ALIGN: usize = 16;
const HEADER_SIZE: usize = core::mem::size_of::<AllocHeader>();
const MIN_BLOCK_SIZE: usize = core::mem::size_of::<FreeNode>();

/// Stored before every allocated block. Used to recover block boundaries on free.
#[repr(C)]
struct AllocHeader {
    block_start: usize,
    block_size: usize,
}

/// Intrusive linked list node stored in free memory blocks.
#[repr(C)]
struct FreeNode {
    size: usize,
    next: *mut FreeNode,
}

/// Zaehler, um die Karte zu haben, bevor umgebaut wird. Reine Felder, kein
/// Format, keine Ausgabe: hier drin darf nichts allozieren.
///
/// Gemessen werden soll, WOHIN die Zeit geht — `steps` sind die besuchten
/// Knoten. Eine Gesamtzeit haette nicht gesagt, ob die Suche beim Belegen
/// oder die adresssortierte Einfuegestelle beim Freigeben teuer ist.
#[derive(Clone, Copy)]
pub struct HeapCounters {
    pub allocs: u64,
    pub frees: u64,
    pub alloc_steps: u64,
    pub free_steps: u64,
    pub free_nodes: u64,
    pub max_free_nodes: u64,
    pub grows: u64,
    /// Anforderungen je Zweierpotenz: [0] < 32 B, [1] < 64 B, ... [15] >= 512 KB
    pub size_hist: [u64; 16],
}

struct Heap {
    free_list: *mut FreeNode,
    regions: [(usize, usize); MAX_REGIONS], // (start, end) of each chunk
    region_count: usize,
    total_size: usize,
    allocated_bytes: usize,
    c: HeapCounters,
}

unsafe impl Send for Heap {}

impl Heap {
    const fn empty() -> Self {
        Heap {
            free_list: ptr::null_mut(),
            regions: [(0, 0); MAX_REGIONS],
            region_count: 0,
            total_size: 0,
            allocated_bytes: 0,
            c: HeapCounters {
                allocs: 0, frees: 0, alloc_steps: 0, free_steps: 0,
                free_nodes: 0, max_free_nodes: 0, grows: 0, size_hist: [0; 16],
            },
        }
    }

    fn init(&mut self, start: usize, size: usize) {
        self.regions[0] = (start, start + size);
        self.region_count = 1;
        self.total_size = size;
        self.allocated_bytes = 0;

        let node = start as *mut FreeNode;
        unsafe {
            (*node).size = size;
            (*node).next = ptr::null_mut();
        }
        self.free_list = node;
        self.c.free_nodes = 1;
        self.c.max_free_nodes = 1;
    }

    /// Check if an address falls within any known heap region.
    fn contains(&self, addr: usize) -> bool {
        for i in 0..self.region_count {
            let (start, end) = self.regions[i];
            if addr >= start && addr < end { return true; }
        }
        false
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        let result = self.try_allocate(&layout);
        if !result.is_null() { return result; }

        // First attempt failed — grow and retry
        let needed = layout.size() + HEADER_SIZE + layout.align();
        if self.grow(needed) {
            self.try_allocate(&layout)
        } else {
            ptr::null_mut()
        }
    }

    fn try_allocate(&mut self, layout: &Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align().max(BLOCK_ALIGN);

        self.c.allocs += 1;
        let mut b = 0usize;
        while b < 15 && size >= (32usize << b) { b += 1; }
        self.c.size_hist[b] += 1;

        let mut prev: *mut FreeNode = ptr::null_mut();
        let mut current = self.free_list;

        while !current.is_null() {
            self.c.alloc_steps += 1;
            let block_start = current as usize;
            let block_size = unsafe { (*current).size };
            let next = unsafe { (*current).next };

            let data_start = align_up(block_start + HEADER_SIZE, align);
            let total_needed = align_up((data_start - block_start) + size, BLOCK_ALIGN)
                .max(MIN_BLOCK_SIZE);

            if block_size >= total_needed {
                let remainder = block_size - total_needed;

                let actual_size = if remainder >= MIN_BLOCK_SIZE {
                    let new_node = (block_start + total_needed) as *mut FreeNode;
                    unsafe {
                        (*new_node).size = remainder;
                        (*new_node).next = next;
                    }
                    if prev.is_null() { self.free_list = new_node; }
                    else { unsafe { (*prev).next = new_node; } }
                    total_needed
                } else {
                    if prev.is_null() { self.free_list = next; }
                    else { unsafe { (*prev).next = next; } }
                    self.c.free_nodes = self.c.free_nodes.saturating_sub(1);
                    block_size
                };

                let header = (data_start - HEADER_SIZE) as *mut AllocHeader;
                unsafe {
                    (*header).block_start = block_start;
                    (*header).block_size = actual_size;
                }
                self.allocated_bytes += actual_size;
                return data_start as *mut u8;
            }

            prev = current;
            current = next;
        }

        ptr::null_mut()
    }

    /// Grow heap by requesting contiguous frames from the physical memory manager.
    fn grow(&mut self, min_size: usize) -> bool {
        if self.total_size >= MAX_HEAP { return false; }
        if self.region_count >= MAX_REGIONS { return false; }

        let chunk = min_size.max(GROW_CHUNK).min(MAX_HEAP - self.total_size);
        let frames = (chunk + 4095) / 4096;

        // SAFETY: memory::allocate_contiguous uses its own lock (memory::ALLOCATOR),
        // independent of the heap lock we're holding. No deadlock possible.
        // NOTE: no kprintln here — we're inside GlobalAlloc::alloc,
        // and kprintln can allocate (capture_bytes → String::push_str) → deadlock.
        if let Some(base) = crate::memory::allocate_contiguous(frames) {
            let start = base as usize;
            let size = frames * 4096;

            self.regions[self.region_count] = (start, start + size);
            self.region_count += 1;
            self.total_size += size;
            self.c.grows += 1;

            // Add new chunk as free block (coalesces locally with neighbors)
            self.insert_free_block(start, size);
            true
        } else {
            false
        }
    }

    /// Insert a free block into the address-sorted free list and coalesce locally.
    /// Since the list is always sorted, we only need to check merge with prev and next — O(1) merge.
    fn insert_free_block(&mut self, block_start: usize, mut block_size: usize) {
        let new_node = block_start as *mut FreeNode;

        // Find insertion point
        let mut prev: *mut FreeNode = ptr::null_mut();
        let mut current = self.free_list;
        while !current.is_null() && (current as usize) < block_start {
            self.c.free_steps += 1;
            prev = current;
            current = unsafe { (*current).next };
        }
        // Ein neuer Knoten; die Verschmelzungen unten nehmen ihn ggf. wieder weg.
        self.c.free_nodes += 1;
        if self.c.free_nodes > self.c.max_free_nodes {
            self.c.max_free_nodes = self.c.free_nodes;
        }

        // Merge with next block?
        if !current.is_null() && block_start + block_size == current as usize {
            self.c.free_nodes = self.c.free_nodes.saturating_sub(1);
            block_size += unsafe { (*current).size };
            unsafe { (*new_node).next = (*current).next; }
        } else {
            unsafe { (*new_node).next = current; }
        }
        unsafe { (*new_node).size = block_size; }

        // Merge with prev block?
        if !prev.is_null() && (prev as usize) + unsafe { (*prev).size } == block_start {
            self.c.free_nodes = self.c.free_nodes.saturating_sub(1);
            unsafe {
                (*prev).size += block_size;
                (*prev).next = (*new_node).next;
            }
        } else if prev.is_null() {
            self.free_list = new_node;
        } else {
            unsafe { (*prev).next = new_node; }
        }
    }

    fn deallocate(&mut self, ptr: *mut u8) {
        if ptr.is_null() { return; }
        let data_addr = ptr as usize;
        if !self.contains(data_addr) { return; }

        let header = unsafe { &*((data_addr - HEADER_SIZE) as *const AllocHeader) };
        let block_start = header.block_start;
        let block_size = header.block_size;

        if !self.contains(block_start) { return; }

        self.c.frees += 1;
        self.allocated_bytes -= block_size;
        self.insert_free_block(block_start, block_size);
    }
}

struct LockedHeap {
    inner: Mutex<Heap>,
}

impl LockedHeap {
    const fn new() -> Self {
        LockedHeap { inner: Mutex::new(Heap::empty()) }
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner.lock().allocate(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.inner.lock().deallocate(ptr);
    }
}

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::new();

unsafe extern "C" {
    static __heap_start: u8;
}

pub fn init() {
    let heap_start = unsafe { &__heap_start as *const u8 as usize };
    crate::memory::reserve_region(heap_start as u64, INITIAL_HEAP as u64);
    HEAP.inner.lock().init(heap_start, INITIAL_HEAP);
    kprintln!("[npk] Heap: {} MB (grows on demand, max {} MB)",
        INITIAL_HEAP / (1024 * 1024), MAX_HEAP / (1024 * 1024));
}

/// Die Zaehler herausholen. Kopie, damit der Aufrufer drucken kann, ohne den
/// Heap-Lock zu halten — kprintln alloziert.
pub fn counters() -> HeapCounters {
    HEAP.inner.lock().c
}

pub fn reset_counters() {
    let mut h = HEAP.inner.lock();
    h.c.allocs = 0; h.c.frees = 0; h.c.alloc_steps = 0; h.c.free_steps = 0;
    h.c.grows = 0; h.c.size_hist = [0; 16];
    // free_nodes NICHT zuruecksetzen: das ist ein Zustand, keine Zaehlung.
    h.c.max_free_nodes = h.c.free_nodes;
}

pub fn stats() -> (usize, usize) {
    let heap = HEAP.inner.lock();
    (heap.allocated_bytes, heap.total_size)
}

#[inline]
fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
