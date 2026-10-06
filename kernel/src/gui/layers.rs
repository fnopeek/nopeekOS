//! Full-screen layer buffers the compositor draws into and reads from.
//!
//!   Layer 0 (Background): Wallpaper or aurora, rendered once, cached
//!   Layer 1 (Chrome):     Window borders, tinted backgrounds, bar
//!   Layer 2 (Text):       Terminal text, transparent where no text
//!
//! Each layer is a BGRA buffer of `pitch * height` bytes. The cursor is not
//! a layer (see shade/cursor.rs).

use spin::Mutex;

/// Layer indices.
pub const LAYER_BG: usize = 0;
pub const LAYER_CHROME: usize = 1;
pub const LAYER_TEXT: usize = 2;
/// Number of layers.
const LAYER_COUNT: usize = 3;
/// Dirty marks per layer before it counts as fully dirty.
const MAX_DIRTY: usize = 16;

/// Per-layer state.
struct Layer {
    /// Pixel buffer (BGRA, same size as framebuffer).
    buf: *mut u8,
    /// Buffer size in bytes.
    size: usize,
    /// Dirty marks since the last clear.
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
    crate::kdebug!("[npk] layers: allocating {} MB for {} buffers",
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

    crate::kdebug!("[npk] Layer compositor: {}x{}, {}MB per layer, {} layers",
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
/// If not, the BG layer should not be used (resolution changed after init).
pub fn matches_resolution(width: u32, height: u32, pitch: u32) -> bool {
    let stack = LAYERS.lock();
    stack.initialized && stack.width == width && stack.height == height && stack.pitch == pitch
}

/// Mark a region of a layer as dirty (needs re-compositing).
pub fn mark_dirty(layer_idx: usize, _x: u32, _y: u32, _w: u32, _h: u32) {
    let mut stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT || !stack.initialized { return; }
    mark_dirty_inner(&mut stack.layers[layer_idx]);
}

/// Mark entire layer as dirty.
pub fn mark_full_dirty(layer_idx: usize) {
    let mut stack = LAYERS.lock();
    if layer_idx >= LAYER_COUNT { return; }
    stack.layers[layer_idx].full_dirty = true;
}

fn mark_dirty_inner(layer: &mut Layer) {
    if layer.full_dirty { return; } // already fully dirty
    if layer.dirty_count >= MAX_DIRTY {
        layer.full_dirty = true;
        return;
    }
    layer.dirty_count += 1;
}
