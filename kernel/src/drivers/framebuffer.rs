//! Framebuffer Console
//!
//! Pixel-based text output using framebuffer from GPU subsystem.
//! Replaces VGA text mode for UEFI systems without legacy VGA.

use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use spin::Mutex;

/// Framebuffer info parsed from Multiboot2.
#[derive(Clone, Copy)]
pub struct FbInfo {
    /// Where the CPU reaches the framebuffer (`paging::ioremap`), not its
    /// physical address.
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
}

/// The framebuffer the display scans out. Its format may differ from the
/// shadows, which are 32 bpp whenever the scanout is 16 bpp (r5g6b5).
#[derive(Clone, Copy)]
struct Scanout {
    addr: u64,
    pitch: u32,
    bpp: u8,
}

pub struct FbConsole {
    info: FbInfo,
    out: Scanout,
    /// Shadow buffer A in RAM (double-buffer: one of the two render targets)
    shadow: *mut u8,
    /// Shadow buffer B in RAM (double-buffer: the other render target)
    shadow_b: *mut u8,
    shadow_size: usize,
    /// Which buffer is front (0 = shadow/A, 1 = shadow_b/B).
    /// Front is displayed (blit source + cursor restore). Back is render target.
    front: usize,
    col: u32,
    row: u32,
    cols: u32,
    rows: u32,
    /// Pixel scale factor (2 for 4K, 1 otherwise). Each glyph pixel becomes scale×scale.
    scale: u32,
    /// GPU address of shadow A (0 = not mapped for GPU blit)
    pub shadow_a_gpu: u64,
    /// GPU address of shadow B (0 = not mapped for GPU blit)
    pub shadow_b_gpu: u64,
}

// SAFETY: FbConsole is only accessed under a spinlock (CONSOLE mutex).
// The shadow pointers are heap-allocated and exclusively owned.
unsafe impl Send for FbConsole {}

/// Cached front-buffer pointer for lock-free cursor drawing.
/// Updated on swap_buffers() and init. Cursor reads this without CONSOLE lock.
static SHADOW_FRONT_CACHED: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());

/// Read cached front-buffer pointer (lock-free, for cursor overlay).
pub fn cached_shadow_front() -> *mut u8 {
    SHADOW_FRONT_CACHED.load(Ordering::Acquire)
}

impl FbConsole {
    pub fn info(&self) -> &FbInfo {
        &self.info
    }

    /// Front buffer pointer (blit source, cursor restore, GPU-composited cursor).
    pub fn front_ptr(&self) -> *mut u8 {
        if self.front == 0 { self.shadow } else { self.shadow_b }
    }

    /// Back buffer pointer (render target for full redraws).
    pub fn shadow_back(&self) -> *mut u8 {
        if self.front == 0 { self.shadow_b } else { self.shadow }
    }

    /// Front buffer pointer + size (for blitting and partial updates).
    pub fn shadow_ptr(&self) -> (*mut u8, usize) {
        (self.front_ptr(), self.shadow_size)
    }

    /// Swap front ↔ back. Call after rendering to back is complete.
    /// Does NOT update the cached cursor pointer — call commit_front() after blit.
    /// This ensures lock-free cursor reads from the buffer that matches MMIO.
    pub fn swap_buffers(&mut self) {
        self.front = 1 - self.front;
    }

    /// Update cached front pointer for lock-free cursor. Call AFTER blit to MMIO.
    /// Until this is called, lock-free cursor reads from the old front (matching MMIO).
    pub fn commit_front(&self) {
        SHADOW_FRONT_CACHED.store(self.front_ptr(), Ordering::Release);
    }

    /// Physical addresses + page count of shadow buffers (identity-mapped).
    pub fn shadow_phys_info(&self) -> (u64, u64, u32) {
        (self.shadow as u64, self.shadow_b as u64, (self.shadow_size / 4096) as u32)
    }

    /// GPU address of the current front buffer (for GPU blit source).
    pub fn front_gpu(&self) -> u64 {
        if self.front == 0 { self.shadow_a_gpu } else { self.shadow_b_gpu }
    }
}

/// Execute a closure with exclusive access to the framebuffer console.
#[track_caller]
pub fn with_fb<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut FbConsole) -> R,
{
    let mut console = CONSOLE.lock();
    // A GPU copy of the front buffer may still be reading the shadows and
    // writing the framebuffer; one that failed is redone here on the CPU.
    if !gpu_copy_done() {
        if let Some(c) = console.as_ref() {
            blit_all(c);
        }
    }
    console.as_mut().map(f)
}

pub static CONSOLE: Mutex<Option<FbConsole>> = Mutex::new(None);

/// When true, write_str/write_byte skip the framebuffer (GUI owns it).
/// kprintln still goes to serial.
static GUI_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_gui_mode(active: bool) {
    GUI_MODE.store(active, Ordering::Release);
}

pub fn gui_mode() -> bool {
    GUI_MODE.load(Ordering::Acquire)
}

/// Whether text output stays off the framebuffer: in GUI mode, and always
/// while the compositor runs, whose frames are drawn under `CONSOLE`.
fn text_console_off() -> bool {
    GUI_MODE.load(Ordering::Relaxed) || compositor_active()
}

#[cfg(target_arch = "x86_64")]
fn compositor_active() -> bool {
    crate::shade::is_active()
}

#[cfg(not(target_arch = "x86_64"))]
fn compositor_active() -> bool {
    false
}

/// Wait for a GPU copy of the front buffer; false if it failed.
#[cfg(target_arch = "x86_64")]
fn gpu_copy_done() -> bool {
    crate::gpu::gpudev::wait_framebuffer()
}

#[cfg(not(target_arch = "x86_64"))]
fn gpu_copy_done() -> bool {
    true
}

/// The console's `FbInfo`, outside `CONSOLE`: rendering runs under
/// `with_fb`, and anything it reaches may ask for the screen size.
static INFO: Mutex<Option<FbInfo>> = Mutex::new(None);

/// Get the current framebuffer resolution.
pub fn get_resolution() -> (u32, u32) {
    let info = get_info();
    (info.width, info.height)
}

/// Get a copy of the current FbInfo.
pub fn get_info() -> FbInfo {
    INFO.lock().unwrap_or(FbInfo { addr: 0, pitch: 0, width: 0, height: 0, bpp: 0 })
}

/// Get shadow buffer physical addresses + page count (for GPU mapping).
pub fn shadow_phys_info() -> Option<(u64, u64, u32)> {
    with_fb(|fb| fb.shadow_phys_info())
}

/// Get front buffer's GPU address (for GPU blit source).
pub fn front_gpu() -> u64 {
    with_fb(|fb| fb.front_gpu()).unwrap_or(0)
}

/// Store the shadows' GPU addresses after GPU mapping.
pub fn set_shadow_gpu(a: u64, b: u64) {
    with_fb(|fb| {
        fb.shadow_a_gpu = a;
        fb.shadow_b_gpu = b;
    });
}

use super::font8x16::FONT;

const FONT_WIDTH: u32 = 8;
const FONT_HEIGHT: u32 = 16;
const FG_COLOR: u32 = 0x00E8E8E8; // Near-white
const BG_COLOR: u32 = 0x00000000; // Black
/// Colour of the `[npk]` tag. Neutral grey: dim enough to read as a tag
/// rather than as content, without putting a hue on every boot line.
static NPK_TAG_COLOR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0x00909090);

/// Rows the console scrolls at once when it fills up.
///
/// A scroll repaints everything, and a framebuffer left uncached by a
/// firmware MTRR (see `diagnose_fb_memory_type`) blits slowly, so scrolling
/// one row per line costs a full blit per boot line. Moving several rows at
/// a time amortises it; the text jumps instead of sliding, as framebuffer
/// consoles do at boot.
const SCROLL_ROWS: u32 = 8;

pub fn set_npk_color(color: u32) {
    NPK_TAG_COLOR.store(color, core::sync::atomic::Ordering::Release);
}

/// Colour of console text other than the tag; `FG_COLOR` except inside
/// `with_text_color`.
static TEXT_COLOR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(FG_COLOR);

/// Run `f` with console text drawn in `color` (a boot banner). The serial
/// line is unaffected.
pub fn with_text_color(color: u32, f: impl FnOnce()) {
    let old = TEXT_COLOR.swap(color, core::sync::atomic::Ordering::AcqRel);
    f();
    TEXT_COLOR.store(old, core::sync::atomic::Ordering::Release);
}

/// Initialize framebuffer console from GPU subsystem.
/// Call after gpu::init() has detected and configured a display.
#[cfg(target_arch = "x86_64")]
pub fn init_from_gpu() {
    let gpu_fb = match crate::gpu::framebuffer_info() {
        Some(fb) => fb,
        None => {
            crate::kprintln!("[npk] Framebuffer: not available (VGA text mode fallback)");
            return;
        }
    };

    let pitch = gpu_fb.pitch;
    let width = gpu_fb.width;
    let height = gpu_fb.height;
    let bpp = gpu_fb.bpp;

    crate::kprintln!("[npk] Framebuffer: {}x{} @ {:#x} ({}bpp, {})",
        width, height, gpu_fb.addr, bpp, crate::gpu::driver_name());

    // Map the framebuffer for the CPU; `addr` below is the mapped address.
    // Write-combining is essential: the GOP MMIO framebuffer is otherwise
    // uncached on Intel hardware (firmware MTRR), so every CPU write is a
    // separate UC transaction (QEMU's fb is RAM and hides this). WC batches
    // writes into bursts. PAT index 5 = WC is programmed in paging::init.
    let addr = if crate::gpu::is_native() {
        gpu_fb.addr
    } else {
        let flags = crate::paging::PageFlags::PRESENT
            | crate::paging::PageFlags::WRITABLE
            | crate::paging::PageFlags::WRITE_COMBINE;
        match crate::paging::ioremap(gpu_fb.addr, pitch as u64 * height as u64, flags) {
            Ok(a) => a,
            Err(e) => {
                crate::kprintln!("[npk] Framebuffer: cannot map it: {}", e);
                return;
            }
        }
    };

    install(FbInfo { addr, pitch, width, height, bpp });
}

/// Run the console on a framebuffer the firmware set up and left mapped at
/// `addr` (the CPU address): 32 bpp x8r8g8b8 or 16 bpp r5g6b5.
#[cfg(not(target_arch = "x86_64"))]
pub fn init_from_firmware(addr: u64, width: u32, height: u32, pitch: u32, bpp: u8) {
    install(FbInfo { addr, pitch, width, height, bpp });
}

/// Set up shadows and console state for the mapped framebuffer `fb`.
fn install(fb: FbInfo) {
    let FbInfo { width, height, .. } = fb;
    let out = Scanout { addr: fb.addr, pitch: fb.pitch, bpp: fb.bpp };
    // The shadows are drawn at 32 bpp; a 16 bpp scanout is converted on
    // the way out, and the console's surface is then the 32 bpp shadow
    // geometry over the same address.
    let info = if fb.bpp == 16 {
        FbInfo { addr: fb.addr, pitch: width * 4, width, height, bpp: 32 }
    } else {
        fb
    };
    let pitch = info.pitch;
    // 2x pixel scaling for 4K (each glyph pixel → 2×2 block).
    // Threshold matches gui::font::scale_for — keep both in step.
    let scale = if width > 2560 { 2u32 } else { 1 };
    let eff_w = FONT_WIDTH * scale;
    let eff_h = FONT_HEIGHT * scale;
    let cols = width / eff_w;
    let rows = height / eff_h;

    // Free old shadow buffers before allocating new ones (prevents OOM on mode switch)
    {
        SHADOW_FRONT_CACHED.store(core::ptr::null_mut(), Ordering::Release);
        *INFO.lock() = None;
        let old = CONSOLE.lock().take();
        if let Some(old_console) = old {
            let old_layout = alloc::alloc::Layout::from_size_align(
                old_console.shadow_size, 4096).unwrap();
            if !old_console.shadow.is_null() && old_console.shadow_size > 0 {
                // SAFETY: allocated below with this same layout; the console is
                // out of CONSOLE and the cached front pointer is cleared, so
                // nothing reaches it.
                unsafe { alloc::alloc::dealloc(old_console.shadow, old_layout); }
            }
            if !old_console.shadow_b.is_null() && old_console.shadow_size > 0 {
                // SAFETY: as for shadow above.
                unsafe { alloc::alloc::dealloc(old_console.shadow_b, old_layout); }
            }
        }
    }

    // Allocate two shadow buffers for double-buffering (fast WB-cached RAM)
    // Page-aligned (4096) so a GPU can map them for its copies
    let shadow_size = pitch as usize * height as usize;
    // A zero-sized layout may not be allocated; a mode without rows or
    // pitch is not one to draw on anyway.
    if shadow_size == 0 {
        crate::kprintln!("[npk] Framebuffer: mode has no pixels ({}x{}, pitch {})", width, height, pitch);
        return;
    }
    let layout = alloc::alloc::Layout::from_size_align(shadow_size, 4096)
        .expect("shadow buffer layout");
    // SAFETY: `layout` has a non-zero size (checked above).
    let shadow = unsafe { alloc::alloc::alloc_zeroed(layout) };
    // SAFETY: as above.
    let shadow_b = unsafe { alloc::alloc::alloc_zeroed(layout) };

    if shadow.is_null() || shadow_b.is_null() {
        crate::kprintln!("[npk] Framebuffer: shadow alloc failed ({}KB x2)", shadow_size / 1024);
        // SAFETY: returned by alloc_zeroed with this layout and not shared yet.
        if !shadow.is_null() { unsafe { alloc::alloc::dealloc(shadow, layout); } }
        // SAFETY: returned by alloc_zeroed with this layout and not shared yet.
        if !shadow_b.is_null() { unsafe { alloc::alloc::dealloc(shadow_b, layout); } }
        return;
    }

    crate::kdebug!("[npk] Framebuffer: double-buffer {}KB x2", shadow_size / 1024);

    // Clear MMIO framebuffer
    clear_screen(&out, height);

    // Initialize cached pointer for lock-free cursor
    SHADOW_FRONT_CACHED.store(shadow, Ordering::Release);
    crate::fatal::set_framebuffer(out.addr, out.pitch, width, height, out.bpp);

    *CONSOLE.lock() = Some(FbConsole {
        info, out, shadow, shadow_b, shadow_size, front: 0,
        col: 0, row: 0, cols, rows, scale,
        shadow_a_gpu: 0, shadow_b_gpu: 0,
    });
    *INFO.lock() = Some(info);
}

/// Initialize the framebuffer from BootInfo. Delegates to `gpu::init`
/// (which wraps the GOP-via-UEFI framebuffer driver) then sets up our
/// shadow + scaling state.
#[cfg(target_arch = "x86_64")]
pub fn init_from_boot_info(boot_info: &crate::boot_info::BootInfo) {
    crate::gpu::init(boot_info);
    init_from_gpu();
}

fn clear_screen(out: &Scanout, height: u32) {
    let fb = out.addr as *mut u8;
    let size = out.pitch as usize * height as usize;
    // SAFETY: out.addr is the CPU mapping of the framebuffer, pitch * height
    // bytes long (ioremap in init_from_gpu, the native driver's mapping, or
    // the firmware's framebuffer in the identity map).
    unsafe { core::ptr::write_bytes(fb, 0, size); }
}

/// Copy the rectangle `(x, y, w, h)` of the front buffer to the scanout,
/// converting 32 bpp to r5g6b5 when the scanout is 16 bpp.
fn scanout(console: &FbConsole, x: u32, y: u32, w: u32, h: u32) {
    let info = &console.info;
    let out = &console.out;
    let (x, y) = (x.min(info.width), y.min(info.height));
    let (w, h) = (w.min(info.width - x), h.min(info.height - y));
    if w == 0 || h == 0 { return; }
    let src = console.front_ptr();
    let dst = out.addr as *mut u8;
    if out.bpp == info.bpp {
        let bytes = (info.bpp as usize).div_ceil(8);
        let pitch = info.pitch as usize;
        if x == 0 && w == info.width && out.pitch == info.pitch {
            // Whole rows: one copy per 256-row chunk.
            let mut row = y as usize;
            let end = (y + h) as usize;
            while row < end {
                let rows = 256.min(end - row);
                // SAFETY: rows y..y+h lie below height, so the range lies in
                // both the front buffer and the scanout (pitch * height bytes
                // each), which do not overlap.
                unsafe { core::ptr::copy_nonoverlapping(src.add(row * pitch), dst.add(row * pitch), rows * pitch) };
                row += rows;
            }
        } else {
            for row in y..y + h {
                let s = row as usize * pitch + x as usize * bytes;
                let d = row as usize * out.pitch as usize + x as usize * bytes;
                // SAFETY: x + w <= width and row < height keep both spans in
                // their buffers.
                unsafe { core::ptr::copy_nonoverlapping(src.add(s), dst.add(d), w as usize * bytes) };
            }
        }
        return;
    }
    for row in y..y + h {
        let s = src.wrapping_add(row as usize * info.pitch as usize) as *const u32;
        let d = dst.wrapping_add(row as usize * out.pitch as usize) as *mut u16;
        for col in x..x + w {
            // SAFETY: col < width and row < height: pixel (col, row) lies in
            // the 32 bpp front buffer and in the 16 bpp scanout.
            unsafe {
                let p = *s.add(col as usize);
                let v = ((p >> 8) & 0xf800) | ((p >> 5) & 0x07e0) | ((p >> 3) & 0x001f);
                core::ptr::write_volatile(d.add(col as usize), v as u16);
            }
        }
    }
}

/// Write pixel to shadow buffer (RAM, fast).
fn put_pixel_shadow(console: &FbConsole, x: u32, y: u32, color: u32) {
    let info = &console.info;
    if x >= info.width || y >= info.height { return; }
    if info.bpp == 32 {
        // Single aligned 32-bit write (BGRA/RGBA, 99% of UEFI framebuffers)
        let offset = (y * info.pitch + x * 4) as usize;
        // SAFETY: shadow buffer is large enough, offset checked by bounds above
        unsafe { *(console.shadow.add(offset) as *mut u32) = color; }
    } else {
        let bytes_per_pixel = (info.bpp as u32 + 7) / 8;
        let offset = (y * info.pitch + x * bytes_per_pixel) as usize;
        // SAFETY: x and y are checked above, so the three bytes of pixel (x, y)
        // lie in the shadow buffer of pitch * height bytes for bpp >= 24. Every
        // framebuffer in use is 32 bpp, so narrower formats never get here.
        unsafe {
            *console.shadow.add(offset) = (color & 0xFF) as u8;
            *console.shadow.add(offset + 1) = ((color >> 8) & 0xFF) as u8;
            *console.shadow.add(offset + 2) = ((color >> 16) & 0xFF) as u8;
        }
    }
}

/// Draw character to shadow buffer with specified foreground color.
/// At scale > 1, each glyph pixel becomes a scale×scale block.
fn draw_char_colored(console: &FbConsole, c: u8, col: u32, row: u32, fg: u32) {
    let s = console.scale;
    let x0 = col * FONT_WIDTH * s;
    let y0 = row * FONT_HEIGHT * s;
    let glyph = &FONT[c as usize * FONT_HEIGHT as usize..];

    for dy in 0..FONT_HEIGHT {
        if dy as usize >= glyph.len() { break; }
        let bits = glyph[dy as usize];
        for dx in 0..FONT_WIDTH {
            let color = if bits & (0x80 >> dx) != 0 { fg } else { BG_COLOR };
            // Write scale×scale block
            for sy in 0..s {
                for sx in 0..s {
                    put_pixel_shadow(console, x0 + dx * s + sx, y0 + dy * s + sy, color);
                }
            }
        }
    }
}

/// Draw character to shadow buffer.
fn draw_char(console: &FbConsole, c: u8, col: u32, row: u32) {
    draw_char_colored(console, c, col, row, FG_COLOR);
}

/// Blit a region of the front buffer to the MMIO framebuffer.
fn blit_region(console: &FbConsole, y_start: u32, y_end: u32) {
    let info = &console.info;
    let h = y_end.saturating_sub(y_start);
    if present(console, 0, y_start, info.width, h) { return; }
    scanout(console, 0, y_start, info.width, h);
}

/// Blit entire front buffer to MMIO framebuffer.
fn blit_all(console: &FbConsole) {
    if present(console, 0, 0, console.info.width, console.info.height) { return; }
    scanout(console, 0, 0, console.info.width, console.info.height);
}

/// Blit a single character cell from front buffer to MMIO.
fn blit_char(console: &FbConsole, col: u32, row: u32) {
    let s = console.scale;
    let (x, y) = (col * FONT_WIDTH * s, row * FONT_HEIGHT * s);
    if present(console, x, y, FONT_WIDTH * s, FONT_HEIGHT * s) { return; }
    scanout(console, x, y, FONT_WIDTH * s, FONT_HEIGHT * s);
}

/// Scroll shadow buffer only (no MMIO blit — caller handles batching).
/// Move the console up by `n` text rows, clearing the ones freed at the
/// bottom. Returns the row the cursor lands on.
fn scroll_shadow(console: &mut FbConsole, n: u32) -> u32 {
    let total_rows = console.rows;
    let n = n.clamp(1, total_rows);
    let row_bytes = console.info.pitch as usize * (FONT_HEIGHT * console.scale) as usize;
    let keep = (total_rows - n) as usize;

    // SAFETY: n <= total_rows and total_rows * row_bytes <= shadow_size, so
    // source, destination and the cleared tail lie in the shadow buffer;
    // ptr::copy allows the overlap; &mut console excludes other users.
    unsafe {
        if keep > 0 {
            core::ptr::copy(
                console.shadow.add(row_bytes * n as usize),
                console.shadow,
                row_bytes * keep,
            );
        }
        core::ptr::write_bytes(
            console.shadow.add(row_bytes * keep),
            0,
            row_bytes * n as usize,
        );
    }
    total_rows - n
}

/// Scroll with immediate blit (for single-byte writes like write_byte).
fn scroll(console: &mut FbConsole) {
    let row = scroll_shadow(console, SCROLL_ROWS);
    console.row = row;
    blit_all(console);
}

/// With a driver module presenting the framebuffer
/// (`gpudev::present_by_module`), a rectangle of the front buffer copied
/// through it: the display would not show a CPU write. False when there is
/// no such module, or the copy was not taken.
#[cfg(target_arch = "x86_64")]
fn present(console: &FbConsole, x: u32, y: u32, w: u32, h: u32) -> bool {
    if !crate::gpu::gpudev::present_by_module() {
        return false;
    }
    let info = &console.info;
    let src = console.front_gpu();
    let (w, h) = (w.min(info.width.saturating_sub(x)), h.min(info.height.saturating_sub(y)));
    if w == 0 || h == 0 {
        return true;
    }
    src != 0 && crate::gpu::gpu_blit_rect(src, info.pitch, crate::gpu::fb_gpu_addr(), info.pitch, x, y, w, h)
}

#[cfg(not(target_arch = "x86_64"))]
fn present(_: &FbConsole, _: u32, _: u32, _: u32, _: u32) -> bool {
    false
}

/// Blit a rectangular region from front buffer to MMIO framebuffer.
pub fn blit_rect(console: &FbConsole, x: u32, y: u32, w: u32, h: u32) {
    if console.info.bpp != 32 { return; }
    if present(console, x, y, w, h) { return; }
    scanout(console, x, y, w, h);
}

/// On-demand version of the boot-time MTRR diagnostic (the `mtrr` intent),
/// so it can be read without catching the fast-scrolling boot log.
#[cfg(target_arch = "x86_64")]
pub fn dump_memory_type() {
    let addr = with_fb(|fb| fb.info().addr).unwrap_or(0);
    if addr == 0 {
        crate::kprintln!("[mtrr] no active framebuffer");
        return;
    }
    crate::kprintln!("[mtrr] native_gpu={} (WC mapping only applied for GOP)",
        crate::gpu::is_native());
    // Which display device is this, and which driver presents to it.
    if let Some(dev) = crate::pci::find_by_class(0x03, 0x00) {
        crate::kprintln!(
            "[gfx] display PCI {:02x}:{:02x}.{} [{:04x}:{:04x}] driver={}",
            dev.addr.bus, dev.addr.device, dev.addr.function,
            dev.vendor_id, dev.device_id, crate::gpu::driver_name(),
        );
    }
    diagnose_fb_memory_type(addr);
}

/// Read-only: report the framebuffer's effective memory type (firmware
/// MTRRs vs our PAT WC). A UC MTRR over this region overrides PAT WC (UC
/// always wins) and makes the blit slow.
#[cfg(target_arch = "x86_64")]
fn diagnose_fb_memory_type(fb_addr: u64) {
    use crate::arch::x86_64::msr;
    /// IA32_MTRRCAP; its VCNT bounds the variable-range MTRRs that exist.
    struct MtrrCap(u64);
    impl MtrrCap {
        fn read() -> Self {
            // SAFETY: MTRRs are present on every x86_64 CPU; IA32_MTRRCAP
            // is read-only.
            MtrrCap(unsafe { msr::read(0xFE) })
        }
        fn vcnt(&self) -> u32 { (self.0 & 0xFF) as u32 }
        fn def_type(&self) -> u64 {
            // SAFETY: IA32_MTRR_DEF_TYPE exists with MTRRs; reading it has
            // no side effect.
            unsafe { msr::read(0x2FF) }
        }
        /// (IA32_MTRR_PHYSBASEn, IA32_MTRR_PHYSMASKn).
        fn var(&self, n: u32) -> (u64, u64) {
            assert!(n < self.vcnt());
            // SAFETY: the pair exists for every n below VCNT; reading it
            // has no side effect.
            unsafe { (msr::read(0x200 + 2 * n), msr::read(0x201 + 2 * n)) }
        }
    }
    fn type_name(t: u8) -> &'static str {
        match t { 0 => "UC", 1 => "WC", 4 => "WT", 5 => "WP", 6 => "WB", _ => "??" }
    }

    let mtrr = MtrrCap::read();
    let cap = mtrr.0;
    let vcnt = mtrr.vcnt();
    let def = mtrr.def_type();
    let def_type = (def & 0xFF) as u8;
    crate::kprintln!(
        "[mtrr] cap vcnt={} wc_supported={} | def_type={} mtrr_enabled={} | fb @ {:#x}",
        vcnt, (cap >> 10) & 1, type_name(def_type), (def >> 11) & 1, fb_addr,
    );

    // Effective MTRR type for fb_addr: scan valid variable MTRRs (UC wins,
    // else WT over WB, else the matching type, else the default type).
    let mut eff: Option<u8> = None;
    for n in 0..vcnt.min(16) {
        let (base, mask) = mtrr.var(n);
        if mask & (1 << 11) == 0 { continue; } // V bit clear → not valid
        let mtype = (base & 0xFF) as u8;
        let phys_base = base & 0x000F_FFFF_FFFF_F000;
        let phys_mask = mask & 0x000F_FFFF_FFFF_F000;
        if (fb_addr & phys_mask) == (phys_base & phys_mask) {
            let size = (!phys_mask & 0x000F_FFFF_FFFF_F000).wrapping_add(0x1000);
            crate::kprintln!("[mtrr]  var{}: base={:#x} size={:#x} type={} COVERS fb",
                n, phys_base, size, type_name(mtype));
            eff = Some(match (eff, mtype) {
                (_, 0) | (Some(0), _) => 0,
                (Some(4), 6) | (Some(6), 4) => 4,
                (_, t) => t,
            });
        }
    }
    let eff_type = eff.unwrap_or(def_type);
    crate::kprintln!(
        "[mtrr] => FB effective MTRR type = {} {}",
        type_name(eff_type),
        if eff_type == 1 { "(WC already — blit should be fast)" }
        else { "(not WC: overrides the PAT WC request, blits run uncached)" },
    );

    // Read back the FB PTE and decode its PAT index to confirm we asked for WC.
    if let Some((entry, level)) = crate::paging::leaf_entry(fb_addr) {
        let pwt = (entry >> 3) & 1;
        let pcd = (entry >> 4) & 1;
        let pat_bit = if level == 3 { (entry >> 7) & 1 } else { (entry >> 12) & 1 };
        let idx = (pat_bit << 2) | (pcd << 1) | pwt;
        crate::kprintln!("[mtrr] FB PTE level={} (3=4KB) PAT_idx={} (5=WC requested) raw={:#x}",
            level, idx, entry);
    }
}

/// Emit a single character to the console with a given foreground color.
fn emit_char(console: &mut FbConsole, byte: u8, fg: u32,
             dirty_min: &mut u32, dirty_max: &mut u32, scrolled: &mut bool) {
    match byte {
        b'\n' => {
            console.col = 0;
            console.row += 1;
            if console.row >= console.rows {
                console.row = scroll_shadow(console, SCROLL_ROWS);
                *scrolled = true;
            }
        }
        b'\r' => { console.col = 0; }
        0x08 => {
            if console.col > 0 {
                console.col -= 1;
                draw_char(console, b' ', console.col, console.row);
                *dirty_min = (*dirty_min).min(console.row);
                *dirty_max = (*dirty_max).max(console.row);
            }
        }
        b if b >= 0x20 && b < 0x7F => {
            draw_char_colored(console, b, console.col, console.row, fg);
            *dirty_min = (*dirty_min).min(console.row);
            *dirty_max = (*dirty_max).max(console.row);
            console.col += 1;
            if console.col >= console.cols {
                console.col = 0;
                console.row += 1;
                if console.row >= console.rows {
                    console.row = scroll_shadow(console, SCROLL_ROWS);
                    *scrolled = true;
                }
            }
        }
        _ => {}
    }
}

/// Write a string to the framebuffer console.
pub fn write_str(s: &str) {
    // GUI owns the framebuffer — skip text console writes
    if text_console_off() { return; }
    let mut console = CONSOLE.lock();
    let console = match console.as_mut() {
        Some(c) => c,
        None => return, // No framebuffer
    };

    // Draw everything to shadow buffer first (no MMIO writes yet)
    let mut dirty_min_row: u32 = console.rows;
    let mut dirty_max_row: u32 = 0;
    let mut scrolled = false;

    // [npk] tag coloring: detect pattern and color those chars
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Check for [npk] pattern
        let (byte, fg) = if i + 5 <= bytes.len() && &bytes[i..i+5] == b"[npk]" {
            // Emit all 5 chars in accent color
            for j in 0..5 {
                emit_char(console, bytes[i + j], NPK_TAG_COLOR.load(core::sync::atomic::Ordering::Relaxed),
                          &mut dirty_min_row, &mut dirty_max_row, &mut scrolled);
            }
            i += 5;
            continue;
        } else {
            (bytes[i], TEXT_COLOR.load(core::sync::atomic::Ordering::Relaxed))
        };
        i += 1;

        emit_char(console, byte, fg, &mut dirty_min_row, &mut dirty_max_row, &mut scrolled);
    }

    // Single blit at the end: either full (if scrolled) or just dirty rows
    let s = console.scale;
    if scrolled {
        blit_all(console);
    } else if dirty_max_row >= dirty_min_row {
        let y_start = dirty_min_row * FONT_HEIGHT * s;
        let y_end = (dirty_max_row + 1) * FONT_HEIGHT * s;
        blit_region(console, y_start, y_end);
    }
}

/// Write a single byte to framebuffer (for echo from serial read).
pub fn write_byte(byte: u8) {
    if text_console_off() { return; }
    let mut console = CONSOLE.lock();
    let console = match console.as_mut() {
        Some(c) => c,
        None => return,
    };
    match byte {
        b'\n' => {
            console.col = 0;
            console.row += 1;
            if console.row >= console.rows {
                scroll(console);
            }
        }
        b'\r' => { console.col = 0; }
        0x08 => {
            if console.col > 0 {
                console.col -= 1;
                draw_char(console, b' ', console.col, console.row);
                blit_char(console, console.col, console.row);
            }
        }
        b if b >= 0x20 && b < 0x7F => {
            draw_char(console, b, console.col, console.row);
            blit_char(console, console.col, console.row);
            console.col += 1;
            if console.col >= console.cols {
                console.col = 0;
                console.row += 1;
                if console.row >= console.rows {
                    scroll(console);
                }
            }
        }
        _ => {}
    }
}

/// Clear the framebuffer screen (both shadow buffers + MMIO).
#[track_caller]
pub fn clear() {
    let mut console = CONSOLE.lock();
    // As in `with_fb`: a GPU copy still writing the framebuffer would put
    // the old frame back over the cleared one.
    gpu_copy_done();
    let console = match console.as_mut() {
        Some(c) => c,
        None => return,
    };
    // SAFETY: shadow and shadow_b are this console's own buffers of
    // shadow_size bytes, and the framebuffer holds pitch * height bytes,
    // which shadow_size equals; CONSOLE is held, and the GPU copy is done.
    unsafe {
        core::ptr::write_bytes(console.shadow, 0, console.shadow_size);
        core::ptr::write_bytes(console.shadow_b, 0, console.shadow_size);
    }
    console.col = 0;
    console.row = 0;
    if present(console, 0, 0, console.info.width, console.info.height) { return; }
    clear_screen(&console.out, console.info.height);
}

#[allow(dead_code)]
/// Check if framebuffer is available.
pub fn is_available() -> bool {
    CONSOLE.lock().is_some()
}
