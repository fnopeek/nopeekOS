//! Heap Allocator
//!
//! Linked-list free-block allocator enabling Vec, Box, String via alloc.
//! First-fit with block splitting and coalescing.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use spin::Mutex;
use crate::kprintln;

const INITIAL_HEAP_SIZE: usize = 1024 * 1024; // 1MB
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

struct Heap {
    free_list: *mut FreeNode,
    heap_start: usize,
    heap_end: usize,
    allocated_bytes: usize,
}

unsafe impl Send for Heap {}

impl Heap {
    const fn empty() -> Self {
        Heap { free_list: ptr::null_mut(), heap_start: 0, heap_end: 0, allocated_bytes: 0 }
    }

    fn init(&mut self, start: usize, size: usize) {
        self.heap_start = start;
        self.heap_end = start + size;
        self.allocated_bytes = 0;

        let node = start as *mut FreeNode;
        unsafe {
            (*node).size = size;
            (*node).next = ptr::null_mut();
        }
        self.free_list = node;
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align().max(BLOCK_ALIGN);

        let mut prev: *mut FreeNode = ptr::null_mut();
        let mut current = self.free_list;

        while !current.is_null() {
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

    fn deallocate(&mut self, ptr: *mut u8) {
        if ptr.is_null() { return; }
        let data_addr = ptr as usize;
        if data_addr < self.heap_start || data_addr >= self.heap_end { return; }

        let header = unsafe { &*((data_addr - HEADER_SIZE) as *const AllocHeader) };
        let block_start = header.block_start;
        let block_size = header.block_size;

        if block_start < self.heap_start || block_start + block_size > self.heap_end { return; }

        self.allocated_bytes -= block_size;

        // Insert into address-sorted free list
        let new_node = block_start as *mut FreeNode;
        unsafe {
            (*new_node).size = block_size;
            (*new_node).next = ptr::null_mut();
        }

        if self.free_list.is_null() || block_start < self.free_list as usize {
            unsafe { (*new_node).next = self.free_list; }
            self.free_list = new_node;
        } else {
            let mut current = self.free_list;
            loop {
                let next = unsafe { (*current).next };
                if next.is_null() || block_start < next as usize {
                    unsafe {
                        (*new_node).next = next;
                        (*current).next = new_node;
                    }
                    break;
                }
                current = next;
            }
        }

        self.coalesce();
    }

    /// Merge adjacent free blocks to reduce fragmentation
    fn coalesce(&mut self) {
        let mut current = self.free_list;
        while !current.is_null() {
            let next = unsafe { (*current).next };
            if next.is_null() { break; }

            let current_end = current as usize + unsafe { (*current).size };
            if current_end == next as usize {
                unsafe {
                    (*current).size += (*next).size;
                    (*current).next = (*next).next;
                }
            } else {
                current = next;
            }
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

extern "C" {
    static __heap_start: u8;
}

pub fn init() {
    let heap_start = unsafe { &__heap_start as *const u8 as usize };
    crate::memory::reserve_region(heap_start as u64, INITIAL_HEAP_SIZE as u64);
    HEAP.inner.lock().init(heap_start, INITIAL_HEAP_SIZE);
    kprintln!("[npk] Heap: {} KB at {:#x}", INITIAL_HEAP_SIZE / 1024, heap_start);
}

#[allow(dead_code)]
pub fn stats() -> (usize, usize) {
    let heap = HEAP.inner.lock();
    (heap.allocated_bytes, heap.heap_end - heap.heap_start)
}

#[inline]
fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
