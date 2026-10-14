//! The full-screen background layer the compositor starts every full frame
//! from: the wallpaper (or aurora), rendered once and cached, a BGRA buffer
//! of `pitch * height` bytes.

use spin::Mutex;

/// Layer indices.
pub const LAYER_BG: usize = 0;
/// Number of layers.
const LAYER_COUNT: usize = 1;

/// Per-layer state.
struct Layer {
    /// Pixel buffer (BGRA, same size as framebuffer).
    buf: *mut u8,
    /// Buffer size in bytes.
    size: usize,
}

// SAFETY: Layer buffers are heap-allocated and accessed under LAYERS mutex.
unsafe impl Send for Layer {}

impl Layer {
    const fn empty() -> Self {
        Layer {
            buf: core::ptr::null_mut(),
            size: 0,
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
            layers: [Layer::empty()],
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

/// Get raw pointer to a layer buffer for direct writes.
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
