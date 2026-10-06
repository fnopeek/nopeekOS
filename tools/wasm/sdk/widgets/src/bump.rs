//! A fixed-size bump allocator with a reset mark.
//!
//! For apps that rebuild everything per frame: allocate freely, then
//! `reset(mark)` back to the state taken after the long-lived data was set
//! up. Nothing is freed individually. An app whose state outlives a frame
//! in ways that are hard to keep below the mark is better served by the
//! growing heap (`heap`, feature `heap`).
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: nopeek_widgets::bump::Bump<{ 2 * 1024 * 1024 }> =
//!     nopeek_widgets::bump::Bump::new();
//! ```

use core::alloc::{GlobalAlloc, Layout};
use core::cell::{Cell, UnsafeCell};

pub struct Bump<const N: usize> {
    heap: UnsafeCell<[u8; N]>,
    pos: Cell<usize>,
}

// SAFETY: a wasm module runs on one thread; nothing shares the allocator.
unsafe impl<const N: usize> Sync for Bump<N> {}

impl<const N: usize> Bump<N> {
    pub const fn new() -> Self {
        Bump { heap: UnsafeCell::new([0; N]), pos: Cell::new(0) }
    }

    /// The current fill level, to `reset` to later.
    pub fn mark(&self) -> usize {
        self.pos.get()
    }

    /// Drop everything allocated since `mark`. Every allocation made after
    /// it must be dead by now: its memory is handed out again.
    pub fn reset(&self, mark: usize) {
        self.pos.set(mark.min(N));
    }
}

impl<const N: usize> Default for Bump<N> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<const N: usize> GlobalAlloc for Bump<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = self.heap.get() as *mut u8;
        // Align the address, not the offset: the array itself need not be
        // aligned beyond 1.
        let start = base as usize + self.pos.get();
        let aligned = (start + layout.align() - 1) & !(layout.align() - 1);
        let Some(end) = aligned.checked_add(layout.size()) else { return core::ptr::null_mut() };
        if end > base as usize + N {
            return core::ptr::null_mut();
        }
        self.pos.set(end - base as usize);
        base.wrapping_add(aligned - base as usize)
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}
