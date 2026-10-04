# `kernel/src/gui/layers.rs` @ 5e0102684

## L1-13 · `use spin::Mutex;`

```
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
```

## L17 · `pub const LAYER_BG: usize = 0;`

```
/// Layer indices.
```

## L21 · `const LAYER_COUNT: usize = 3;`

```
/// Total number of pixel-buffer layers (cursor is MMIO-only).
```

## L23 · `const MAX_DIRTY: usize = 16;`

```
/// Maximum dirty rectangles tracked per layer.
```

## L26 · `#[derive(Clone, Copy)]`

```
/// A dirty rectangle.
```

## L35 · `struct Layer {`

```
/// Per-layer state.
```

## L37 · `buf: *mut u8,`

```
/// Pixel buffer (BGRA, same size as framebuffer).
```

## L39 · `size: usize,`

```
/// Buffer size in bytes.
```

## L41 · `dirty: [DirtyRect; MAX_DIRTY],`

```
/// Dirty rectangles pending compositing.
```

## L44 · `full_dirty: bool,`

```
/// If true, entire layer needs compositing (e.g. after init or clear).
```

## L48 · `unsafe impl Send for Layer {}`

```
// SAFETY: Layer buffers are heap-allocated and accessed under LAYERS mutex.
```

## L63 · `struct LayerStack {`

```
/// Global layer state.
```

## L86-87 · `pub fn init(width: u32, height: u32, pitch: u32) {`

```
/// Initialize the layer system. Allocates buffers for all layers.
/// Call after framebuffer is initialized.
```

## L92 · `for layer in &mut stack.layers {`

```
// Free old buffers if reinitializing
```

## L96 · `unsafe { alloc::alloc::dealloc(layer.buf, layout); }`

```
// SAFETY: buffer was allocated with this layout
```

## L103 · `let total_needed = buf_size * LAYER_COUNT;`

```
// Growable heap handles allocation — just log the size
```

## L112 · `let buf = unsafe { alloc::alloc::alloc_zeroed(layout) };`

```
// SAFETY: layout is valid, checked above
```

## L116 · `for l in &mut stack.layers {`

```
// Free any already-allocated buffers to prevent memory leak
```

## L119 · `unsafe { alloc::alloc::dealloc(l.buf, layout); }`

```
// SAFETY: buffer was just allocated with this layout
```

## L142 · `pub fn is_initialized() -> bool {`

```
/// Check if the layer system is initialized.
```

## L147 · `pub fn clear(layer_idx: usize) {`

```
/// Clear a layer (fill with transparent black).
```

## L152 · `unsafe { core::ptr::write_bytes(layer.buf, 0, layer.size); }`

```
// SAFETY: buffer is valid and sized correctly
```

## L158 · `pub fn clear_rect(layer_idx: usize, x: u32, y: u32, w: u32, h: u32) {`

```
/// Clear a rectangular region of a layer (fill with transparent black).
```

## L173 · `unsafe { core::ptr::write_bytes(layer.buf.add(off), 0, bytes); }`

```
// SAFETY: bounds checked above
```

## L180 · `#[inline]`

```
/// Write a solid-color pixel to a layer.
```

## L187 · `unsafe { *(stack.layers[layer_idx].buf.add(off) as *mut u32) = color; }`

```
// SAFETY: bounds checked above
```

## L191-192 · `pub fn write_rect(layer_idx: usize, x: u32, y: u32, w: u32, h: u32,`

```
/// Write a rectangular block of pixels to a layer.
/// `pixels` must be BGRA, `src_pitch` bytes per row.
```

## L209 · `unsafe {`

```
// SAFETY: caller guarantees pixels buffer is large enough
```

## L222-226 · `pub fn buffer(layer_idx: usize) -> Option<(*mut u8, u32, u32, u32)> {`

```
/// Get raw pointer to a layer buffer for direct writes.
/// Caller must call `mark_dirty` after writing.
///
/// SAFETY: Caller must ensure writes stay within (pitch * height) bytes.
/// Caller must hold no other lock on LAYERS.
```

## L233-234 · `pub fn matches_resolution(width: u32, height: u32, pitch: u32) -> bool {`

```
/// Check if layer dimensions match the current framebuffer.
/// If not, the BG layer should NOT be used (resolution changed after init).
```

## L240 · `pub fn mark_dirty(layer_idx: usize, x: u32, y: u32, w: u32, h: u32) {`

```
/// Mark a region of a layer as dirty (needs re-compositing).
```

## L247 · `pub fn mark_full_dirty(layer_idx: usize) {`

```
/// Mark entire layer as dirty.
```

## L255 · `if layer.full_dirty { return; } // already fully dirty`

```
// already fully dirty
```

## L257 · `layer.full_dirty = true;`

```
// Too many rects — promote to full dirty
```

## L265-268 · `pub fn composite(shadow: *mut u8, mmio: u64, pitch: u32, width: u32, height: u32)`

```
/// Composite all dirty regions from layers into the shadow buffer, then blit to MMIO.
/// Returns list of regions that were blitted (for cursor redraw).
///
/// Compositing order: Layer 0 (BG) → Layer 1 (Chrome, alpha-blend) → Layer 2 (Text, overlay)
```

## L277 · `let mut regions = alloc::vec::Vec::new();`

```
// Collect all dirty regions across all layers into a merged set
```

## L282 · `regions.clear();`

```
// Entire screen is dirty
```

## L303 · `for &(rx, ry, rw, rh) in &regions {`

```
// Composite each dirty region
```

## L314-315 · `let mut pixel = unsafe { *(bg.add(off) as *const u32) };`

```
// Start with background (opaque, always present)
// SAFETY: all layer buffers are pitch*height bytes, bounds checked
```

## L318 · `let chrome_px = unsafe { *(chrome.add(off) as *const u32) };`

```
// Blend chrome (alpha in high byte)
```

## L327 · `let text_px = unsafe { *(text.add(off) as *const u32) };`

```
// Overlay text (non-zero = opaque text pixel)
```

## L333 · `unsafe { *(shadow.add(off) as *mut u32) = pixel; }`

```
// Write to shadow buffer
```

## L337 · `let seg_off = row_off + rx as usize * 4;`

```
// Blit this scanline segment from shadow to MMIO
```

## L340 · `unsafe {`

```
// SAFETY: shadow and MMIO are valid for full framebuffer size
```

## L351 · `for layer in &mut stack.layers {`

```
// Clear all dirty flags
```

## L360 · `#[inline]`

```
/// Fast alpha blend: result = bg * (1-alpha/255) + fg * (alpha/255)
```

## L375 · `fn merge_region(regions: &mut alloc::vec::Vec<(u32, u32, u32, u32)>,`

```
/// Merge a new rect into the region list. If it overlaps an existing region, expand it.
```

## L378 · `for r in regions.iter_mut() {`

```
// Try to merge with existing region
```

## L381 · `if x <= rx + rw + 16 && x + w + 16 >= rx &&`

```
// Check overlap (with some slack for adjacent rects)
```

## L384 · `let nx = x.min(rx);`

```
// Expand to union
```

## L396 · `pub fn has_dirty() -> bool {`

```
/// Check if any layer has dirty regions pending.
```

