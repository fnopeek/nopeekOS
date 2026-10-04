# `kernel/src/shade/cursor.rs` @ 5e0102684

## L1-11 · `use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};`

```
//! Software mouse cursor — OVERLAY approach.
//!
//! The cursor is NEVER drawn on the shadow buffer. Instead:
//! - Shadow buffer stays clean (scene only)
//! - Cursor is drawn directly on the MMIO framebuffer
//! - On move: restore old area from shadow→MMIO, draw cursor at new pos on MMIO
//! - No save/restore array needed, no ghost cursors possible
//!
//! Lock-free fast path: mouse position is stored as atomics.
//! Core 0 (input) writes position in ~2ns without any lock.
//! Cursor overlay reads atomics and draws directly to MMIO.
```

## L15 · `const CURSOR_W: u32 = 15;`

```
/// Cursor dimensions.
```

## L19 · `pub fn cursor_size() -> (u32, u32) { eff_dims() }`

```
/// Effective cursor size (w, h) — for callers that blit just the cursor rect.
```

## L22-31 · `const SU_N: usize = ((CURSOR_W * SIZE_MAX_PCT / 100) * (CURSOR_H * SIZE_MAX_PCT / 100)) as usize;`

```
// ── Save-under cursor (no recomposite on a pure move) ───────────────────
//
// The render path keeps the scene in the shadow buffer with the cursor
// BAKED in (atomic blit → no flicker). To move the cursor without
// recompositing the whole scene we remember the clean pixels under it
// (`SAVE_UNDER`): on a move we restore them (erase the old cursor), then
// save + bake at the new spot and blit only the small affected region.
// Core-0 / CONSOLE-lock serialized; the Mutex just satisfies the borrow
// checker for the static array.
// Sized for the largest cursor (SIZE_MAX_PCT) so scaling never overflows it.
```

## L36 · `static SU_W: AtomicU32 = AtomicU32::new(0);   // dims the cursor was baked at`

```
// dims the cursor was baked at
```

## L40 · `pub fn saved_pos() -> (i32, i32) { (SU_X.load(Ordering::Relaxed), SU_Y.load(Ordering::Relaxed)) }`

```
/// Position the cursor is currently baked at (the rect to erase on a move).
```

## L44-45 · `pub fn save_under_and_bake(buf: *mut u8, info: &crate::framebuffer::FbInfo) {`

```
/// Save the clean pixels under the cursor's CURRENT position from `buf`,
/// then bake the cursor there. `buf` is a shadow buffer (plain RAM).
```

## L61 · `unsafe {`

```
// SAFETY: bounds-checked offset into a kernel-owned shadow buffer.
```

## L78-79 · `pub fn restore_under(buf: *mut u8, info: &crate::framebuffer::FbInfo) {`

```
/// Restore the saved clean pixels to `buf` at the saved position — erases
/// the baked cursor so the scene under it is intact again.
```

## L97 · `unsafe { *(buf.add(off) as *mut u32) = su[idx]; }`

```
// SAFETY: bounds-checked offset into a kernel-owned shadow buffer.
```

## L103 · `static ATOMIC_X: AtomicI32 = AtomicI32::new(0);`

```
// ── Lock-free mouse position (written by Core 0, read by anyone) ──
```

## L113-116 · `static SPEED: AtomicI32 = AtomicI32::new(220);`

```
/// Pointer speed in percent (100 = 1:1 with the device deltas). Touchpads
/// deliver small deltas, so the default scales up. Tunable via the `mouse`
/// intent / `mouse_speed` config. Sub-100 (slow-down) is smooth because the
/// fractional remainder is accumulated below.
```

## L121 · `pub fn set_speed(percent: i32) { SPEED.store(percent.clamp(25, 600), Ordering::Relaxed); }`

```
/// Set pointer speed (percent, clamped 25..=600).
```

## L123 · `pub fn speed() -> i32 { SPEED.load(Ordering::Relaxed) }`

```
/// Current pointer speed (percent).
```

## L126 · `const SIZE_MAX_PCT: u32 = 300;`

```
/// Cursor size in percent of the base bitmap (100 = 16×22). Clamped 50..=MAX.
```

## L130 · `pub fn set_size(percent: i32) { SIZE.store(percent.clamp(50, SIZE_MAX_PCT as i32), Ordering::Relaxed); }`

```
/// Set cursor size (percent, clamped 50..=300).
```

## L132 · `pub fn size() -> i32 { SIZE.load(Ordering::Relaxed) }`

```
/// Current cursor size (percent).
```

## L135 · `fn eff_dims() -> (u32, u32) {`

```
/// Effective (scaled) cursor dimensions in pixels.
```

## L141 · `fn blend(bg: u32, fg: u32, a: u32) -> u32 {`

```
/// Alpha-blend `fg` over `bg` at coverage `a` (0..=255).
```

## L150 · `fn in_poly(poly: &[(f32, f32)], x: f32, y: f32) -> bool {`

```
/// Even-odd point-in-polygon test in reference space.
```

## L166-167 · `fn poly_cov(poly: &[(f32, f32)], col: u32, row: u32, ew: u32, eh: u32) -> f32 {`

```
/// Supersampled coverage (0..=1) of `poly` at output pixel (col,row), mapping
/// the pixel into the REF_W×REF_H reference space. SS×SS subsamples.
```

## L183-186 · `fn cursor_sample_aa(col: u32, row: u32, ew: u32, eh: u32) -> (u32, u32) {`

```
/// Anti-aliased cursor sample at output pixel (col,row) for effective dims
/// (ew,eh). Rasterizes the vector arrow (OUTER outline + INNER dark fill)
/// with supersampled coverage → (grayscale color, alpha 0..=255); alpha 0 =
/// transparent. Resolution-independent: smooth at any cursor size.
```

## L188 · `let a = poly_cov(&OUTER, col, row, ew, eh);      // outline silhouette → alpha`

```
// outline silhouette → alpha
```

## L190 · `let i = poly_cov(&INNER, col, row, ew, eh);      // dark-fill fraction`

```
// dark-fill fraction
```

## L191 · `let lum = ((235.0 * (1.0 - i) + 30.0 * i) as u32) & 0xff;`

```
// Outline ≈ white (235), interior ≈ near-black (30), mixed by fill cover.
```

## L196-197 · `pub fn update_atomic(dx: i8, dy: i8, buttons: u8) {`

```
/// Update mouse position atomically. NO LOCK needed.
/// Called from Core 0 input polling — takes ~2 nanoseconds.
```

## L203-204 · `let s = SPEED.load(Ordering::Relaxed);`

```
// Scale device deltas by SPEED%, accumulating the fractional remainder so
// slow speeds still move on small deltas and fast speeds stay smooth.
```

## L223 · `pub fn atomic_pos() -> (i32, i32) {`

```
/// Read current atomic mouse position
```

## L228 · `pub fn atomic_buttons() -> (u8, u8) {`

```
/// Read atomic buttons (current, previous)
```

## L233 · `#[allow(dead_code)]`

```
/// Was left button just clicked? (lock-free)
```

## L240 · `#[allow(dead_code)]`

```
/// Was right button just clicked? (lock-free)
```

## L247 · `pub fn has_button_event() -> bool {`

```
/// Any button action that needs compositor attention? (click, release)
```

## L253 · `pub fn set_screen_size(w: u32, h: u32) {`

```
/// Set screen dimensions for atomic clamping
```

## L259 · `pub fn init_atomic(screen_w: u32, screen_h: u32) {`

```
/// Initialize atomic position (centered)
```

## L266-272 · `const REF_W: f32 = 30.0;`

```
/// Vector arrow cursor. A classic `left_ptr`: tip (hotspot) at (0,0), a
/// light outline (OUTER silhouette) around a dark fill (INNER = OUTER inset
/// by the outline width, computed offline). Both are rasterized with
/// supersampled point-in-polygon coverage in a REF_W×REF_H reference space,
/// so the cursor is resolution-independent — smooth at any size, unlike the
/// old 16×22 bitmap that pixelated at 1:1. REF is exactly 2× the base dims
/// (15×22) so the default maps 1 ref = 0.5 px with no aspect distortion.
```

## L276-277 · `static OUTER: [(f32, f32); 4] = [`

```
/// Outline silhouette (outer boundary of the light stroke), tip at (0,0).
/// A tail-less arrowhead: vertical left edge, concave bottom notch, wing.
```

## L279 · `(0.0,  0.0),   // tip / hotspot`

```
// tip / hotspot
```

## L280 · `(0.0, 29.0),   // bottom of the left edge`

```
// bottom of the left edge
```

## L281 · `(9.5, 21.5),   // notch (concave bottom)`

```
// notch (concave bottom)
```

## L282 · `(22.5, 21.5),  // right wing`

```
// right wing
```

## L285 · `static INNER: [(f32, f32); 4] = [`

```
/// Dark fill — OUTER inset by the outline width (precomputed miter offset).
```

## L293 · `#[allow(dead_code)]`

```
/// Mouse state — position, buttons, and overlay tracking.
```

## L302 · `drawn_x: i32,`

```
/// Last position where cursor was drawn on MMIO (for restore).
```

## L305 · `drawn: bool,`

```
/// Whether cursor is currently drawn on MMIO framebuffer.
```

## L320 · `pub fn init(&mut self, screen_w: u32, screen_h: u32) {`

```
/// Initialize with screen dimensions. Centers the cursor.
```

## L328 · `pub fn update(&mut self, dx: i8, dy: i8, buttons: u8) {`

```
/// Update position from relative mouse movement. Clamps to screen.
```

## L342-346 · `pub fn middle_clicked(&self) -> bool {`

```
/// Die mittlere Taste. Beide Zeigerwege liefern sie schon: die PS/2-Maske
/// ist `b0 & 0x07`, das HID-Boot-Protokoll hat sie auf demselben Bit —
/// niemand hat bisher nur danach gefragt. Der Browser tut es: ein
/// Mittelklick auf einen Link ist „in neuem Tab oeffnen", und ohne diese
/// Taste gibt es die Geste gar nicht.
```

## L366 · `static DRAWN_LF_X: AtomicI32 = AtomicI32::new(0);`

```
// ── Shared cursor-drawn state (used by both lock-free and inner paths) ──
```

## L370 · `static DRAWN_LF_W: AtomicU32 = AtomicU32::new(0);   // dims of the last MMIO-drawn cursor`

```
// dims of the last MMIO-drawn cursor
```

## L374-375 · `fn drawn_erase_dims() -> (u32, u32) {`

```
/// Rect (w,h) to erase for the last MMIO-drawn cursor (falls back to current
/// effective size if nothing drawn yet).
```

## L382-384 · `#[allow(dead_code)]`

```
/// Draw cursor from IRQ context — no locks, no shadow restore.
/// Writes directly to MMIO using cached framebuffer info.
/// Any trail artifact is cleaned up by the next render_frame().
```

## L397-398 · `#[allow(dead_code)]`

```
/// Draw cursor after scene blit. Erases old position ONLY if cursor moved
/// (no blink when stationary, no ghost when moved).
```

## L410 · `if DRAWN_LF.load(Ordering::Relaxed) {`

```
// Erase old cursor only if it moved (avoid blink when stationary)
```

## L426 · `static IRQ_FB_ADDR: AtomicU64 = AtomicU64::new(0);`

```
/// Cached framebuffer MMIO address for IRQ-safe cursor draw
```

## L430 · `pub fn cache_fb_info(addr: u64, pitch: u32) {`

```
/// Cache framebuffer info for IRQ cursor draw. Call after GPU init.
```

## L436 · `fn draw_cursor_on_mmio(mmio: *mut u8, bg_buf: *const u8, pitch: usize, sw: i32, sh: i32, x: i32, y: i32) {`

```
/// Draw cursor bitmap directly to MMIO framebuffer at given position.
```

## L448-449 · `let bg = unsafe {`

```
// Blend over the clean shadow pixel if provided, else read back the
// current MMIO pixel (slow path; only the IRQ fallback with no shadow).
```

## L454 · `unsafe { core::ptr::write_volatile(mmio.add(off) as *mut u32, blend(bg, color, a)); }`

```
// SAFETY: writing to MMIO framebuffer within bounds
```

## L462-476 · `pub fn draw_cursor_on_shadow(shadow: *mut u8, info: &crate::framebuffer::FbInfo) {`

```
/// Paint cursor bitmap into the back-shadow buffer at the current
/// atomic position. Called at the very end of compose so the cursor
/// becomes part of the same shadow that gets blitted to MMIO — no
/// separate post-blit cursor write, no race between blit and cursor.
///
/// Critical for high-frequency surface tiles (microvm browser at 60Hz
/// FLUSH): the previous design re-blitted the entire shadow on every
/// frame, then re-drew the cursor over MMIO. Display refresh could
/// catch the brief window where the blit had landed but the cursor
/// re-write hadn't yet — visible as cursor flicker right after
/// stopping mouse movement (the moving case masked the flicker with
/// the moving cursor's blur).
///
/// Shadow is the single source of truth now: blit copies cursor along
/// with the rest of the scene atomically (from the display's view).
```

## L494-495 · `unsafe { let p = shadow.add(off) as *mut u32; *p = blend(*p, color, a); }`

```
// SAFETY: alpha-blend over the existing back-shadow pixel. Shadow
// is a kernel-owned allocation, not MMIO — plain load/store fine.
```

## L501 · `fn blit_shadow_to_mmio(shadow: *mut u8, mmio: *mut u8, pitch: usize,`

```
/// Copy a small rectangle from shadow buffer to MMIO framebuffer (restore clean pixels).
```

## L512 · `unsafe {`

```
// SAFETY: copying from shadow buffer to MMIO framebuffer
```

