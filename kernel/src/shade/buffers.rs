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
//!
//! A buffer the GPU composes from is registered with gpudev on first use.
//! CPU writes (`deref_mut`, `ptr`, `info`) wait for the GPU job that last
//! read it, and its frames go back to the allocator only after the module
//! unbound them (`gpudev::release`).

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
    /// The pixels for writing through `gui::render`; waits for the GPU.
    pub fn ptr(&self) -> *mut u8 {
        crate::gpu::gpudev::wait_cpu_access(self.phys);
        self.phys() as *mut u8
    }

    /// The buffer as a render target for the `gui::render` and `gui::font`
    /// helpers, which address it in the buffer's own coordinates.
    pub fn info(&self) -> FbInfo {
        crate::gpu::gpudev::wait_cpu_access(self.phys);
        FbInfo { addr: self.phys, pitch: self.width * 4, width: self.width, height: self.height, bpp: 32 }
    }
}

impl Default for PixelBuffer {
    fn default() -> Self {
        Self::empty()
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
        crate::gpu::gpudev::wait_cpu_access(self.phys);
        // SAFETY: as in `deref`; `&mut self` makes the access exclusive.
        unsafe { core::slice::from_raw_parts_mut(self.phys as *mut u32, (self.width * self.height) as usize) }
    }
}

impl Drop for PixelBuffer {
    fn drop(&mut self) {
        if self.phys != 0 && crate::gpu::gpudev::release(self.phys, self.phys, self.pages) {
            crate::memory::deallocate_contiguous(self.phys, self.pages);
        }
    }
}

/// Pixels in frames the kernel does not own: a guest's framebuffer in its
/// RAM (virtio-gpu guest blob). The compositor and the GPU read them where
/// they lie; whoever hands them out must keep the frames alive until
/// `readers` is 0 and gpudev has let go of them (`gpudev::pages_released`).
pub struct ForeignPixels {
    /// Unique for the boot; gpudev names the buffer by it.
    pub serial: u64,
    /// Physical address of every 4 KB page, in order.
    pub pages: alloc::vec::Vec<u64>,
    /// CPU readers copying from the pages right now.
    readers: core::sync::atomic::AtomicU32,
    /// The pages are guest pages pinned for this object
    /// (`GuestMem::pin_page`); the pins go with it.
    pinned: bool,
    /// The picture in them is laid out for the card (tiled, compressed):
    /// only the GPU module reads it, a CPU copy would be scrambled.
    gpu_only: core::sync::atomic::AtomicBool,
}

static FOREIGN_SERIAL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

impl ForeignPixels {
    /// `pages` must be page-aligned physical addresses of frames that stay
    /// allocated while this object can be read.
    pub fn new(pages: alloc::vec::Vec<u64>) -> Self {
        ForeignPixels {
            serial: FOREIGN_SERIAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
            pages,
            readers: core::sync::atomic::AtomicU32::new(0),
            pinned: false,
            gpu_only: core::sync::atomic::AtomicBool::new(false),
        }
    }

    /// `new` for guest pages each pinned once (`GuestMem::pin_page`), which
    /// this object unpins when it goes.
    pub fn new_pinned(pages: alloc::vec::Vec<u64>) -> Self {
        let mut f = Self::new(pages);
        f.pinned = true;
        f
    }

    pub fn bytes(&self) -> u64 {
        self.pages.len() as u64 * 4096
    }

    /// No CPU reader is copying from the pages.
    pub fn unread(&self) -> bool {
        self.readers.load(core::sync::atomic::Ordering::Acquire) == 0
    }

    /// A CPU reader starts; `unread` is false until its `unhold`.
    pub fn hold(&self) {
        self.readers.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
    }

    pub fn unhold(&self) {
        self.readers.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }

    /// The GPU module reads the picture through its layout
    /// (`npk_gpudev_bo_gpu_only`); the CPU must not copy it.
    pub fn set_gpu_only(&self) {
        self.gpu_only.store(true, core::sync::atomic::Ordering::Release);
    }

    pub fn gpu_only(&self) -> bool {
        self.gpu_only.load(core::sync::atomic::Ordering::Acquire)
    }
}

impl Drop for ForeignPixels {
    fn drop(&mut self) {
        if self.pinned {
            crate::microvm::devices::guest_mem::unpin_frames(&self.pages);
        }
    }
}

/// A picture in `ForeignPixels`: 32-bit pixels, `pitch` bytes per row, the
/// first at byte `offset`.
#[derive(Clone)]
pub struct ForeignView {
    pub pixels: alloc::sync::Arc<ForeignPixels>,
    pub offset: u32,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
}

impl ForeignView {
    /// The view is valid: every pixel lies in the pages and no pixel
    /// straddles two of them.
    pub fn fits(&self) -> bool {
        if self.width == 0 || self.height == 0 || self.offset % 4 != 0 || self.pitch % 4 != 0
            || (self.width as u64) * 4 > self.pitch as u64
        {
            return false;
        }
        let end = self.offset as u64 + (self.height as u64 - 1) * self.pitch as u64 + self.width as u64 * 4;
        end <= self.pixels.bytes()
    }

    /// Bytes from the first pixel to the end of the last.
    pub fn size(&self) -> u64 {
        (self.height as u64 - 1) * self.pitch as u64 + self.width as u64 * 4
    }

    fn at(&self, byte: u64) -> *const u8 {
        let page = self.pixels.pages[(byte / 4096) as usize];
        (page + byte % 4096) as *const u8
    }

    /// Drop the CPU's cached copy of the rows of `rect`, for pages a device
    /// writes past the caches; the view must `fit`.
    pub fn clflush_rect(&self, rect: (u32, u32, u32, u32)) {
        let (x, y, w, h) = rect;
        if x >= self.width || y >= self.height {
            return;
        }
        let w = w.min(self.width - x) as u64 * 4;
        for row in y..y + h.min(self.height - y) {
            let mut byte = self.offset as u64 + row as u64 * self.pitch as u64 + x as u64 * 4;
            let end = byte + w;
            while byte < end {
                let n = (4096 - byte % 4096).min(end - byte);
                crate::mm::clflush_range(self.at(byte) as u64, n);
                byte += n;
            }
        }
    }

    /// Pixel `(x, y)`; the view must `fit`.
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        let byte = self.offset as u64 + y as u64 * self.pitch as u64 + x as u64 * 4;
        // SAFETY: `fits` keeps the pixel inside the pages, 4-aligned within
        // one page; the frames are identity-mapped and alive while the
        // caller holds the view (`ForeignPixels`).
        unsafe { core::ptr::read_volatile(self.at(byte) as *const u32) }
    }

    /// Copy the rows of `rect` into `dst`, a buffer of the view's size.
    pub fn copy_into(&self, dst: &mut PixelBuffer, rect: (u32, u32, u32, u32)) {
        if dst.width() != self.width || dst.height() != self.height || !self.fits() {
            return;
        }
        let (rx, ry, rw, rh) = rect;
        let rx = rx.min(self.width);
        let ry = ry.min(self.height);
        let rw = rw.min(self.width - rx);
        let rh = rh.min(self.height - ry);
        let stride = self.width as usize;
        let out: &mut [u32] = dst;
        for row in ry..ry + rh {
            let mut byte = self.offset as u64 + row as u64 * self.pitch as u64 + rx as u64 * 4;
            let mut i = row as usize * stride + rx as usize;
            let end = i + rw as usize;
            while i < end {
                let in_page = ((4096 - byte % 4096) / 4) as usize;
                let n = in_page.min(end - i);
                // SAFETY: the run lies in one identity-mapped page of the
                // view (`fits`), `out[i..i + n]` lies in the destination row,
                // and the two do not overlap (the kernel owns `dst`).
                unsafe {
                    core::ptr::copy_nonoverlapping(self.at(byte) as *const u32, out[i..].as_mut_ptr(), n);
                }
                i += n;
                byte += n as u64 * 4;
            }
        }
    }
}
