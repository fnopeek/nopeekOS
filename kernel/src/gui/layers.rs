//! Layer-based compositor — composites multiple layers into the shadow buffer.
//!
//! Architecture:
//!   Layer 0 (Background): Wallpaper or aurora, rendered once, cached
//!   Layer 1 (Chrome):     Window borders, tinted backgrounds, bar
//!   Layer 2 (Text):       Terminal text, transparent where no text
//!   Layer 3 (Cursor):     Not a buffer — drawn directly on MMIO (see cursor.rs)
//!
//! Each layer is a full-screen BGRA buffer. Transparent pixels (alpha = 0) are
//! skipped during compositing. Dirty rectangles track which regions need
//! re-compositing.
//!
//! Flow: Layer writes → composite(dirty regions) → shadow buffer → MMIO blit

use spin::Mutex;

/// Layer indices.
pub const LAYER_BG: usize = 0;
pub const LAYER_CHROME: usize = 1;
pub const LAYER_TEXT: usize = 2;
/// Total number of pixel-buffer layers (cursor is MMIO-only).
const LAYER_COUNT: usize = 3;
/// Maximum dirty rectangles tracked per layer.
const MAX_DIRTY: usize = 16;

/// A dirty rectangle.
#[derive(Clone, Copy)]
struct DirtyRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Per-layer state.
struct Layer {
    /// Pixel buffer (BGRA, same size as framebuffer).
    buf: *mut u8,
    /// Buffer size in bytes.
    size: usize,
    /// Dirty rectangles pending compositing.
    dirty: [DirtyRect; MAX_DIRTY],
    dirty_count: usize,
    /// If true, entire layer needs compositing (e.g. after init or clear).
    full_dirty: bool,
}

// SAFETY: Layer buffers are heap-allocated and accessed under LAYERS mutex.
unsafe impl Send for Layer {}

impl Layer {
    const fn empty() -> Self {
        Layer {
            buf: core::ptr::null_mut(),
            size: 0,
            dirty: [DirtyRect { x: 0, y: 0, w: 0, h: 0 }; MAX_DIRTY],
            dirty_count: 0,
            full_dirty: false,
        }
    }
}

/// Global layer state.
struct LayerStack {
    layers: [Layer; LAYER_COUNT],
    width: u32,
    height: u32,
    pitch: u32,
    initialized: bool,
}

impl LayerStack {
    const fn new() -> Self {
        LayerStack {
            layers: [Layer::empty(), Layer::empty(), Layer::empty()],
            width: 0,
            height: 0,
            pitch: 0,
            initialized: false,
        }
    }
}

static LAYERS: Mutex<LayerStack> = Mutex::new(LayerStack::new());

/// Initialize the layer system. Allocates buffers for all layers.
/// Call after framebuffer is initialized.
pub fn init(width: u32, height: u32, pitch: u32) {
    let buf_size = pitch as usize * height as usize;
    let mut stack = LAYERS.lock();

    // Free old buffers if reinitializing
    for layer in &mut stack.layers {
        if !layer.buf.is_null() && layer.size > 0 {
            let layout = alloc::alloc::Layout::from_size_align(layer.size, 16).unwrap();
            // SAFETY: buffer was allocated with this layout
            unsafe { alloc::alloc::dealloc(layer.buf, layout); }
            layer.buf = core::ptr::null_mut();
            layer.size = 0;
        }
    }

    // Growable heap handles allocation — just log the size
    let total_needed = buf_size * LAYER_COUNT;
    crate::kprintln!("[npk] layers: allocating {} MB for {} buffers",
        total_needed / (1024 * 1024), LAYER_COUNT);

    let layout = alloc::alloc::Layout::from_size_align(buf_size, 16)
        .expect("layer buffer layout");

    for layer in &mut stack.layers {
        // SAFETY: layout is valid, checked above
        let buf = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if buf.is_null() {
            crate::kprintln!("[npk] layers: alloc failed ({}MB)", buf_size / (1024 * 1024));
            // Free any already-allocated buffers to prevent memory leak
            for l in &mut stack.layers {
                if !l.buf.is_null() && l.size > 0 {
                    // SAFETY: buffer was just allocated with this layout
                    unsafe { alloc::alloc::dealloc(l.buf, layout); }
                    l.buf = core::ptr::null_mut();
                    l.size = 0;
                }
            }
            return;
        }
        layer.buf = buf;
        layer.size = buf_size;
        layer.full_dirty = true;
        layer.dirty_count = 0;
    }

    stack.width = width;
    stack.height = height;
    stack.pitch = pitch;
    stack.initialized = true;

    crate::kprintln!("[npk] Layer compositor: {}x{}, {}MB per layer, {} layers",
        width, height, buf_size / (1024 * 1024), LAYER_COUNT);
}

/// Check if the layer system is initialized.
pub fn is_initialized() -> bool {
    LAYERS.lock().initialized
}

/// Clear a layer (fill with transparent black).
pub fn clear(layer_idx: usize) {
    let mut stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT || !stack.initialized { return; }
    let layer = &mut stack.layers[layer_idx];
    // SAFETY: buffer is valid and sized correctly
    unsafe { core::ptr::write_bytes(layer.buf, 0, layer.size); }
    layer.full_dirty = true;
    layer.dirty_count = 0;
}

/// Clear a rectangular region of a layer (fill with transparent black).
pub fn clear_rect(layer_idx: usize, x: u32, y: u32, w: u32, h: u32) {
    let mut stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT || !stack.initialized { return; }
    let pitch = stack.pitch as usize;
    let sw = stack.width;
    let sh = stack.height;
    let layer = &mut stack.layers[layer_idx];

    let x1 = (x + w).min(sw);
    let y1 = (y + h).min(sh);
    let bytes = ((x1 - x) as usize) * 4;

    for row in y..y1 {
        let off = row as usize * pitch + x as usize * 4;
        // SAFETY: bounds checked above
        unsafe { core::ptr::write_bytes(layer.buf.add(off), 0, bytes); }
    }

    mark_dirty_inner(layer, x, y, w, h);
}

/// Write a solid-color pixel to a layer.
#[inline]
pub fn put_pixel(layer_idx: usize, x: u32, y: u32, color: u32) {
    let stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT || !stack.initialized { return; }
    if x >= stack.width || y >= stack.height { return; }
    let off = y as usize * stack.pitch as usize + x as usize * 4;
    // SAFETY: bounds checked above
    unsafe { *(stack.layers[layer_idx].buf.add(off) as *mut u32) = color; }
}

/// Write a rectangular block of pixels to a layer.
/// `pixels` must be BGRA, `src_pitch` bytes per row.
pub fn write_rect(layer_idx: usize, x: u32, y: u32, w: u32, h: u32,
                  pixels: *const u8, src_pitch: usize) {
    let mut stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT || !stack.initialized { return; }
    let dst_pitch = stack.pitch as usize;
    let sw = stack.width;
    let sh = stack.height;
    let layer = &mut stack.layers[layer_idx];

    let x1 = (x + w).min(sw);
    let y1 = (y + h).min(sh);
    let copy_bytes = ((x1 - x) as usize) * 4;

    for row in 0..(y1 - y) {
        let dst_off = (y + row) as usize * dst_pitch + x as usize * 4;
        let src_off = row as usize * src_pitch;
        // SAFETY: caller guarantees pixels buffer is large enough
        unsafe {
            core::ptr::copy_nonoverlapping(
                pixels.add(src_off),
                layer.buf.add(dst_off),
                copy_bytes,
            );
        }
    }

    mark_dirty_inner(layer, x, y, w, h);
}

/// Get raw pointer to a layer buffer for direct writes.
/// Caller must call `mark_dirty` after writing.
///
/// SAFETY: Caller must ensure writes stay within (pitch * height) bytes.
/// Caller must hold no other lock on LAYERS.
pub fn buffer(layer_idx: usize) -> Option<(*mut u8, u32, u32, u32)> {
    let stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT || !stack.initialized { return None; }
    Some((stack.layers[layer_idx].buf, stack.width, stack.height, stack.pitch))
}

/// Check if layer dimensions match the current framebuffer.
/// If not, the BG layer should NOT be used (resolution changed after init).
pub fn matches_resolution(width: u32, height: u32, pitch: u32) -> bool {
    let stack = LAYERS.lock();
    stack.initialized && stack.width == width && stack.height == height && stack.pitch == pitch
}

/// Mark a region of a layer as dirty (needs re-compositing).
pub fn mark_dirty(layer_idx: usize, x: u32, y: u32, w: u32, h: u32) {
    let mut stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT || !stack.initialized { return; }
    mark_dirty_inner(&mut stack.layers[layer_idx], x, y, w, h);
}

/// Mark entire layer as dirty.
pub fn mark_full_dirty(layer_idx: usize) {
    let mut stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT { return; }
    stack.layers[layer_idx].full_dirty = true;
}

fn mark_dirty_inner(layer: &mut Layer, x: u32, y: u32, w: u32, h: u32) {
    if layer.full_dirty { return; } // already fully dirty
    if layer.dirty_count >= MAX_DIRTY {
        // Too many rects — promote to full dirty
        layer.full_dirty = true;
        return;
    }
    layer.dirty[layer.dirty_count] = DirtyRect { x, y, w, h };
    layer.dirty_count += 1;
}

/// Composite all dirty regions from layers into the shadow buffer, then blit to MMIO.
/// Returns list of regions that were blitted (for cursor redraw).
///
/// Compositing order: Layer 0 (BG) → Layer 1 (Chrome, alpha-blend) → Layer 2 (Text, overlay)
pub fn composite(shadow: *mut u8, mmio: u64, pitch: u32, width: u32, height: u32)
    -> alloc::vec::Vec<(u32, u32, u32, u32)>
{
    let mut stack = LAYERS.lock();
    if !stack.initialized {
        return alloc::vec::Vec::new();
    }

    // Collect all dirty regions across all layers into a merged set
    let mut regions = alloc::vec::Vec::new();

    for layer in &stack.layers {
        if layer.full_dirty {
            // Entire screen is dirty
            regions.clear();
            regions.push((0u32, 0u32, width, height));
            break;
        }
        for i in 0..layer.dirty_count {
            let r = &layer.dirty[i];
            merge_region(&mut regions, r.x, r.y, r.w, r.h);
        }
    }

    if regions.is_empty() {
        return regions;
    }

    let pitch = pitch as usize;
    let bg = stack.layers[LAYER_BG].buf;
    let chrome = stack.layers[LAYER_CHROME].buf;
    let text = stack.layers[LAYER_TEXT].buf;
    let mmio_ptr = mmio as *mut u8;

    // Composite each dirty region
    for &(rx, ry, rw, rh) in &regions {
        let x1 = (rx + rw).min(width);
        let y1 = (ry + rh).min(height);

        for y in ry..y1 {
            let row_off = y as usize * pitch;

            for x in rx..x1 {
                let off = row_off + x as usize * 4;

                // Start with background (opaque, always present)
                // SAFETY: all layer buffers are pitch*height bytes, bounds checked
                let mut pixel = unsafe { *(bg.add(off) as *const u32) };

                // Blend chrome (alpha in high byte)
                let chrome_px = unsafe { *(chrome.add(off) as *const u32) };
                let chrome_alpha = chrome_px >> 24;
                if chrome_alpha == 255 {
                    pixel = chrome_px | 0xFF000000;
                } else if chrome_alpha > 0 {
                    pixel = alpha_blend(pixel, chrome_px, chrome_alpha);
                }

                // Overlay text (non-zero = opaque text pixel)
                let text_px = unsafe { *(text.add(off) as *const u32) };
                if text_px != 0 {
                    pixel = text_px | 0xFF000000;
                }

                // Write to shadow buffer
                unsafe { *(shadow.add(off) as *mut u32) = pixel; }
            }

            // Blit this scanline segment from shadow to MMIO
            let seg_off = row_off + rx as usize * 4;
            let seg_len = (x1 - rx) as usize * 4;
            // SAFETY: shadow and MMIO are valid for full framebuffer size
            unsafe {
                core::ptr::copy_nonoverlapping(
                    shadow.add(seg_off),
                    mmio_ptr.add(seg_off),
                    seg_len,
                );
            }
        }
    }

    // Clear all dirty flags
    for layer in &mut stack.layers {
        layer.dirty_count = 0;
        layer.full_dirty = false;
    }

    regions
}

/// Fast alpha blend: result = bg * (1-alpha/255) + fg * (alpha/255)
#[inline]
fn alpha_blend(bg: u32, fg: u32, alpha: u32) -> u32 {
    let inv = 255 - alpha;
    let rb_bg = bg & 0x00FF00FF;
    let g_bg = (bg >> 8) & 0x000000FF;
    let rb_fg = fg & 0x00FF00FF;
    let g_fg = (fg >> 8) & 0x000000FF;

    let rb = ((rb_bg * inv + rb_fg * alpha) >> 8) & 0x00FF00FF;
    let g = (((g_bg * inv + g_fg * alpha) >> 8) & 0xFF) << 8;

    rb | g | 0xFF000000
}

/// Merge a new rect into the region list. If it overlaps an existing region, expand it.
fn merge_region(regions: &mut alloc::vec::Vec<(u32, u32, u32, u32)>,
                x: u32, y: u32, w: u32, h: u32) {
    // Try to merge with existing region
    for r in regions.iter_mut() {
        let (rx, ry, rw, rh) = *r;
        // Check overlap (with some slack for adjacent rects)
        if x <= rx + rw + 16 && x + w + 16 >= rx &&
           y <= ry + rh + 16 && y + h + 16 >= ry {
            // Expand to union
            let nx = x.min(rx);
            let ny = y.min(ry);
            let nx2 = (x + w).max(rx + rw);
            let ny2 = (y + h).max(ry + rh);
            *r = (nx, ny, nx2 - nx, ny2 - ny);
            return;
        }
    }
    regions.push((x, y, w, h));
}

/// Check if any layer has dirty regions pending.
pub fn has_dirty() -> bool {
    let stack = LAYERS.lock();
    for layer in &stack.layers {
        if layer.full_dirty || layer.dirty_count > 0 {
            return true;
        }
    }
    false
}
