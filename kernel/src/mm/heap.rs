//! Heap Allocator
//!
//! Segregated free lists with boundary tags. Allocation and free are O(1);
//! the heap grows in 64 MB chunks.
//!
//! Every block carries a header tag (size, bit 0 = free) and a footer copy
//! of the size, so free finds its physical neighbours in O(1). Free blocks
//! are doubly linked in the list of their size class, so coalescing can
//! unlink a neighbour in O(1). Size classes alone are not enough: with a
//! single sorted list, freeing is O(n) too.
//!
//! Each region is bracketed by sentinel blocks that are never free, so
//! coalescing stops at the region boundary without a range check.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use spin::Mutex;

const INITIAL_HEAP: usize = 64 * 1024 * 1024;       // 64MB initial
const GROW_CHUNK: usize = 64 * 1024 * 1024;          // 64MB growth increments
const MAX_HEAP: usize = 2 * 1024 * 1024 * 1024;      // 2GB ceiling
const MAX_REGIONS: usize = 32;
const BLOCK_ALIGN: usize = 16;

/// Header tag and footer copy, eight bytes each.
const TAG: usize = 8;
/// Bit 0 of the header tag. Sizes are 16-aligned, so the bit is unused.
const FREE_BIT: usize = 1;
/// Sentinel block at each region end. Never free.
const SENTINEL: usize = 16;

const HEADER_SIZE: usize = core::mem::size_of::<AllocHeader>();
/// Tag + two list pointers + footer.
const MIN_BLOCK_SIZE: usize = 32;

/// Sits immediately before the payload and leads back to the block start.
/// Alignment can push the payload arbitrarily far past the block start, so
/// this is the only way back.
#[repr(C)]
struct AllocHeader {
    block_start: usize,
    block_size: usize,
}

/// The two list pointers of a free block, right after its tag.
#[repr(C)]
struct FreeNode {
    prev: *mut FreeNode,
    next: *mut FreeNode,
}

/// One bin per 16 bytes up to 512, one per power of two above.
///
/// The 16 is `BLOCK_ALIGN`: block sizes are multiples of it, so each bin
/// below 512 holds exactly one size and its first block always fits. A
/// wider bin would hold several sizes and force a scan. Bins above 512
/// still span a range and may need a short search.
const NBINS: usize = 52;

#[inline]
fn bin_of(size: usize) -> usize {
    if size < 512 {
        size / 16
    } else {
        let mut b = 32;
        let mut s = 512usize;
        while b < NBINS - 1 && s * 2 <= size {
            s *= 2;
            b += 1;
        }
        b
    }
}

/// Statistics. Plain fields only: nothing in here may allocate.
/// `alloc_steps / allocs` should stay near 1.
#[derive(Clone, Copy)]
pub struct HeapCounters {
    pub allocs: u64,
    pub frees: u64,
    pub alloc_steps: u64,
    pub free_steps: u64,
    pub free_nodes: u64,
    pub max_free_nodes: u64,
    pub grows: u64,
    /// Requests per power of two: [0] < 32 B, [1] < 64 B, ... [15] >= 512 KB
    pub size_hist: [u64; 16],
}

struct Heap {
    bins: [*mut FreeNode; NBINS],
    regions: [(usize, usize); MAX_REGIONS], // (start, end) of each chunk
    region_count: usize,
    total_size: usize,
    allocated_bytes: usize,
    c: HeapCounters,
}

unsafe impl Send for Heap {}

// ── Tags ──────────────────────────────────────────────────────────────

#[inline]
unsafe fn tag_of(block: usize) -> usize {
    // SAFETY: `block` is a block start inside a region.
    unsafe { *(block as *const usize) }
}

#[inline]
unsafe fn size_of_block(block: usize) -> usize {
    // SAFETY: wie `tag_of`.
    unsafe { tag_of(block) & !FREE_BIT }
}

#[inline]
unsafe fn is_free(block: usize) -> bool {
    // SAFETY: wie `tag_of`.
    unsafe { tag_of(block) & FREE_BIT != 0 }
}

/// Set header tag and footer together. They must always agree: the footer
/// is the only way to find a block's predecessor.
#[inline]
unsafe fn set_tags(block: usize, size: usize, free: bool) {
    // SAFETY: `block..block+size` lies inside a region.
    unsafe {
        *(block as *mut usize) = size | if free { FREE_BIT } else { 0 };
        *((block + size - TAG) as *mut usize) = size;
    }
}

/// Size of the physical predecessor, read from its footer.
#[inline]
unsafe fn prev_size(block: usize) -> usize {
    // SAFETY: every block is preceded by a block or a sentinel, both with
    // a valid footer.
    unsafe { *((block - TAG) as *const usize) }
}

impl Heap {
    const fn empty() -> Self {
        Heap {
            bins: [ptr::null_mut(); NBINS],
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

    /// Push a free block onto the front of its bin. O(1).
    unsafe fn bin_push(&mut self, block: usize, size: usize) {
        let b = bin_of(size);
        let node = (block + TAG) as *mut FreeNode;
        // SAFETY: `block` is free and at least MIN_BLOCK_SIZE, so both
        // pointers lie inside it.
        unsafe {
            (*node).prev = ptr::null_mut();
            (*node).next = self.bins[b];
            if !self.bins[b].is_null() {
                (*self.bins[b]).prev = node;
            }
        }
        self.bins[b] = node;
        self.c.free_nodes += 1;
        if self.c.free_nodes > self.c.max_free_nodes {
            self.c.max_free_nodes = self.c.free_nodes;
        }
    }

    /// Unlink without a search. This is why the lists are doubly linked:
    /// coalescing removes a neighbour, not the head.
    unsafe fn bin_remove(&mut self, block: usize, size: usize) {
        let b = bin_of(size);
        let node = (block + TAG) as *mut FreeNode;
        // SAFETY: `node` is linked in exactly this bin.
        unsafe {
            let p = (*node).prev;
            let n = (*node).next;
            if p.is_null() { self.bins[b] = n; } else { (*p).next = n; }
            if !n.is_null() { (*n).prev = p; }
        }
        self.c.free_nodes = self.c.free_nodes.saturating_sub(1);
    }

    fn init(&mut self, start: usize, size: usize) {
        self.regions[0] = (start, start + size);
        self.region_count = 1;
        self.total_size = size;
        self.allocated_bytes = 0;
        // SAFETY: the region is reserved and at least 64 MB large.
        unsafe { self.lay_out_region(start, size) };
    }

    /// Sentinel, payload block, sentinel. The sentinels are never free, so
    /// coalescing stops at the region boundary by itself.
    unsafe fn lay_out_region(&mut self, start: usize, size: usize) {
        let body = size - 2 * SENTINEL;
        // SAFETY: the caller owns a region of this size.
        unsafe {
            set_tags(start, SENTINEL, false);
            set_tags(start + SENTINEL + body, SENTINEL, false);
            set_tags(start + SENTINEL, body, true);
            self.bin_push(start + SENTINEL, body);
        }
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

        let needed = layout.size() + TAG + HEADER_SIZE + layout.align() + TAG;
        if self.grow(needed) {
            self.try_allocate(&layout)
        } else {
            ptr::null_mut()
        }
    }

    /// Minimum block size for `size` bytes at `align`, including tag,
    /// header and footer.
    #[inline]
    fn need_for(block_start: usize, size: usize, align: usize) -> (usize, usize) {
        let data = align_up(block_start + TAG + HEADER_SIZE, align);
        let total = align_up((data - block_start) + size + TAG, BLOCK_ALIGN)
            .max(MIN_BLOCK_SIZE);
        (data, total)
    }

    fn try_allocate(&mut self, layout: &Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align().max(BLOCK_ALIGN);

        self.c.allocs += 1;
        let mut h = 0usize;
        while h < 15 && size >= (32usize << h) { h += 1; }
        self.c.size_hist[h] += 1;

        // The lower bound must already include alignment, or the bin choice
        // lands one class too low and the search walks every too-small block
        // in it: `TAG + HEADER_SIZE` is 24, but a 16-aligned block start
        // pushes the payload to 32.
        let data_off = align_up(TAG + HEADER_SIZE, align);
        let lower = align_up(data_off + size + TAG, BLOCK_ALIGN)
            .max(MIN_BLOCK_SIZE);
        let first = bin_of(lower);

        for b in first..NBINS {
            let mut node = self.bins[b];
            while !node.is_null() {
                self.c.alloc_steps += 1;
                let block = node as usize - TAG;
                // SAFETY: `node` is in the free list, so `block` is a free
                // block with valid tags.
                let bsize = unsafe { size_of_block(block) };
                let (data, need) = Self::need_for(block, size, align);
                if bsize >= need {
                    // SAFETY: `block` is free and large enough.
                    unsafe { self.bin_remove(block, bsize) };
                    let rest = bsize - need;
                    let actual = if rest >= MIN_BLOCK_SIZE {
                        // SAFETY: both parts lie inside the original block.
                        unsafe {
                            set_tags(block, need, false);
                            set_tags(block + need, rest, true);
                            self.bin_push(block + need, rest);
                        }
                        need
                    } else {
                        // SAFETY: as above.
                        unsafe { set_tags(block, bsize, false) };
                        bsize
                    };
                    let header = (data - HEADER_SIZE) as *mut AllocHeader;
                    // SAFETY: `data - HEADER_SIZE` lies after the tag and
                    // before the payload of the same block.
                    unsafe {
                        (*header).block_start = block;
                        (*header).block_size = actual;
                    }
                    self.allocated_bytes += actual;
                    return data as *mut u8;
                }
                // SAFETY: `node` is a valid list node.
                node = unsafe { (*node).next };
            }
        }
        ptr::null_mut()
    }

    /// Grow heap by requesting contiguous frames from the physical memory manager.
    fn grow(&mut self, min_size: usize) -> bool {
        if self.total_size >= MAX_HEAP { return false; }
        if self.region_count >= MAX_REGIONS { return false; }

        let chunk = (min_size + 2 * SENTINEL).max(GROW_CHUNK).min(MAX_HEAP - self.total_size);
        let frames = chunk.div_ceil(4096);

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

            // SAFETY: the region was just allocated and is ours.
            unsafe { self.lay_out_region(start, size) };
            true
        } else {
            false
        }
    }

    fn deallocate(&mut self, ptr: *mut u8) {
        if ptr.is_null() { return; }
        let data_addr = ptr as usize;
        if !self.contains(data_addr) { return; }

        // SAFETY: `data_addr` came from `try_allocate`, so the header sits
        // immediately before it.
        let header = unsafe { &*((data_addr - HEADER_SIZE) as *const AllocHeader) };
        let mut block = header.block_start;
        let mut size = header.block_size;

        if !self.contains(block) { return; }

        self.c.frees += 1;
        self.allocated_bytes -= size;

        // Coalesce forward. The end sentinel is never free, so this stops
        // at the region boundary.
        // SAFETY: `block + size` is a block start or the sentinel.
        unsafe {
            let next = block + size;
            if is_free(next) {
                self.c.free_steps += 1;
                let ns = size_of_block(next);
                self.bin_remove(next, ns);
                size += ns;
            }
            // And backward, via the predecessor's footer.
            let ps = prev_size(block);
            if ps != SENTINEL && is_free(block - ps) {
                self.c.free_steps += 1;
                self.bin_remove(block - ps, ps);
                block -= ps;
                size += ps;
            }
            set_tags(block, size, true);
            self.bin_push(block, size);
        }
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

/// The initial heap comes from the frame allocator like every later grow:
/// only frames the firmware map called usable RAM. The bytes after the
/// image are not known to be RAM.
pub fn init() {
    let frames = INITIAL_HEAP / crate::memory::PAGE_SIZE;
    let heap_start = crate::memory::allocate_contiguous(frames)
        .expect("no contiguous RAM for the initial heap") as usize;
    HEAP.inner.lock().init(heap_start, INITIAL_HEAP);
    crate::kdebug!("[npk] Heap: {} MB (max {} MB)",
        INITIAL_HEAP / (1024 * 1024), MAX_HEAP / (1024 * 1024));
}

/// Snapshot of the counters, so the caller can print without holding the
/// heap lock (kprintln allocates).
pub fn counters() -> HeapCounters {
    HEAP.inner.lock().c
}

pub fn reset_counters() {
    let mut h = HEAP.inner.lock();
    h.c.allocs = 0; h.c.frees = 0; h.c.alloc_steps = 0; h.c.free_steps = 0;
    h.c.grows = 0; h.c.size_hist = [0; 16];
    // Do not reset free_nodes: it is state, not a count.
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
