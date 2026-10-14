//! Pixel buffers the compositor composes from (docs/plan/GPU_STACK.md §5.4).
//!
//! One type for every window's and panel's pixels: page-aligned, physically
//! contiguous frames from the frame allocator, the same kind of memory the
//! shadows and the wallpaper planes are. A buffer can therefore be handed to
//! a GPU driver module as it is (gpudev registers it by its physical range)
//! while the CPU keeps drawing into it through the identity mapping.
//!
//! Pixels are 32-bit, packed rows (`pitch = width * 4`). Whether the top
//! byte is alpha is up to the user: widget scenes and terminal text layers
//! carry alpha, the rest ignore it.

use core::ops::{Deref, DerefMut};

use crate::framebuffer::FbInfo;

pub struct PixelBuffer {
    phys: u64,
    pages: usize,
    width: u32,
    height: u32,
}

// SAFETY: the buffer owns its frames exclusively; moving it between cores
// (widget scenes are rasterized on app cores) moves that ownership.
unsafe impl Send for PixelBuffer {}

impl PixelBuffer {
    /// A buffer without pixels.
    pub const fn empty() -> Self {
        PixelBuffer { phys: 0, pages: 0, width: 0, height: 0 }
    }

    /// A zeroed `width` x `height` buffer, or an empty one when the frame
    /// allocator has no contiguous range that large.
    pub fn new(width: u32, height: u32) -> Self {
        let bytes = width as usize * height as usize * 4;
        if bytes == 0 {
            return Self::empty();
        }
        let pages = bytes.div_ceil(4096);
        let Some(phys) = crate::memory::allocate_contiguous(pages) else {
            crate::kprintln!("[npk] shade: no memory for a {}x{} pixel buffer", width, height);
            return Self::empty();
        };
        // SAFETY: the frames were just allocated for this buffer and are
        // identity-mapped; `pages * 4096 >= bytes`.
        unsafe { core::ptr::write_bytes(phys as *mut u8, 0, pages * 4096) };
        PixelBuffer { phys, pages, width, height }
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    /// Physical address of the first pixel (also its kernel address).
    pub fn phys(&self) -> u64 { self.phys }
    pub fn ptr(&self) -> *mut u8 { self.phys() as *mut u8 }

    /// The buffer as a render target for the `gui::render` and `gui::font`
    /// helpers, which address it in the buffer's own coordinates.
    pub fn info(&self) -> FbInfo {
        FbInfo { addr: self.phys, pitch: self.width * 4, width: self.width, height: self.height, bpp: 32 }
    }
}

impl Deref for PixelBuffer {
    type Target = [u32];
    fn deref(&self) -> &[u32] {
        if self.phys == 0 {
            return &[];
        }
        // SAFETY: the frames hold width*height pixels and live as long as self.
        unsafe { core::slice::from_raw_parts(self.phys as *const u32, (self.width * self.height) as usize) }
    }
}

impl DerefMut for PixelBuffer {
    fn deref_mut(&mut self) -> &mut [u32] {
        if self.phys == 0 {
            return &mut [];
        }
        // SAFETY: as in `deref`; `&mut self` makes the access exclusive.
        unsafe { core::slice::from_raw_parts_mut(self.phys as *mut u32, (self.width * self.height) as usize) }
    }
}

impl Drop for PixelBuffer {
    fn drop(&mut self) {
        if self.phys != 0 {
            crate::memory::deallocate_contiguous(self.phys, self.pages);
        }
    }
}
