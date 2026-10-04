# `kernel/src/gui/background.rs` @ 5e0102684

## L1-7 · `use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};`

```
//! Default background + accent lookup.
//!
//! Two modes:
//! 1. A flat dark-grey fill when no wallpaper is set (matches the
//!    login screen's gradient base — consistent system look).
//! 2. A user-supplied wallpaper copied in from `wallpaper.wasm`,
//!    which also extracts a 16-colour theme palette via theme::.
```

## L12 · `const BG_GREY: u32 = 0xFF181820;`

```
/// Default dark-grey background pixel (0xAARRGGBB).
```

## L15-16 · `const DEFAULT_ACCENT: u32 = 0x007B50A0;`

```
/// Accent used when no wallpaper theme is active — kept in sync with
/// `shade::widgets::palette::fallback(Token::Accent)`.
```

## L24-29 · `static mut BLURRED: *mut u8 = core::ptr::null_mut();`

```
/// The wallpaper, heavily blurred, same size and pitch. Glass surfaces
/// (loop, dock, bar) blend over THIS instead of the sharp image: a
/// translucent light panel over sharp texture leaves the texture
/// competing with the text on it, and blur is what makes glass readable.
/// Computed once per wallpaper — the wallpaper is static, so reading it
/// costs a frame exactly what reading the sharp one did.
```

## L35-36 · `const BLUR_DOWN: u32 = 4;`

```
/// Downscale factor for the blur. Blur keeps no detail, so it is computed
/// on a small image and scaled back up.
```

## L40-46 · `static WALLPAPER_GEN: AtomicU32 = AtomicU32::new(0);`

```
/// Bumped every time the wallpaper pixels change. Mixed into the compositor's
/// translucent-glass cache key so a same-theme wallpaper swap invalidates it
/// (the key otherwise tracks colour/geometry only, not the backdrop pixels —
/// and set_wallpaper overwrites the buffer IN PLACE, so clearing the cache
/// alone races a cross-core re-store under the unchanged key). Bump AFTER the
/// pixel write with Release: a core that Acquire-observes the new generation
/// also observes the finished new pixels.
```

## L49 · `pub fn wallpaper_generation() -> u32 {`

```
/// Generation counter of the active wallpaper (see WALLPAPER_GEN).
```

## L60 · `let r = (color >> 16) & 0xFF;`

```
// Ensure accent is bright enough to read on dark window bg.
```

## L100-101 · `WALLPAPER_GEN.fetch_add(1, Ordering::Release);`

```
// After the pixels are fully written: invalidate the glass cache by moving
// the generation forward (Release pairs with the Acquire in the cache key).
```

## L108 · `crate::shade::widgets::refresh_all_scenes();`

```
// Re-rasterize live widget scenes so cached pixels pick up new tokens.
```

## L116 · `fn wallpaper_ptr() -> *const u8 {`

```
/// Raw wallpaper buffer (framebuffer-pitch layout, BGRX u32), or null.
```

## L164-169 · `pub fn draw_background_region_where(shadow: *mut u8, info: &FbInfo,`

```
/// Restore the background in a rect, but only where `want` says so.
///
/// The focus halo wraps the ROUNDED corner of a tile, so it also paints the
/// four corner notches — inside the tile's bounding box, outside its
/// outline. A plain rect restore there would wipe the window's own arc, so
/// the caller masks it down to the pixels the halo can reach.
```

## L182-184 · `unsafe {`

```
// SAFETY: x < info.width and y < info.height, so `off` is inside
// the shadow buffer; the wallpaper, when present, is allocated
// screen-sized with the same pitch (see `set_wallpaper`).
```

## L209 · `pub fn reblur() {`

```
// ── Glass backdrop ────────────────────────────────────────────────────
```

## L211 · `pub fn reblur() {`

```
/// Blur the current wallpaper again — after `set shade.blur`.
```

## L217 · `WALLPAPER_GEN.fetch_add(1, Ordering::Release);`

```
// Every glass cache keys on the generation.
```

## L237 · `let sw = ((w + BLUR_DOWN - 1) / BLUR_DOWN) as usize;`

```
// 1. Average BLUR_DOWN² blocks into a small image, one channel per plane.
```

## L241-242 · `let mut detail_sum: u64 = 0;`

```
// Detail: mean luma standard deviation inside the blocks — the texture
// that the averaging removes, i.e. what would compete with text.
```

## L253-254 · `let px = unsafe { *(wp.add(y as usize * pitch + x as usize * 4) as *const u32) };`

```
// SAFETY: x < width, y < height; the wallpaper is
// screen-sized with the framebuffer pitch (set_wallpaper).
```

## L275 · `let mut tmp = alloc::vec![0u32; sw * sh];`

```
// 2. Separable box blur, a few passes (none at radius 0).
```

## L279 · `box_pass(plane, &mut tmp, sw, sh, 1, sw, radius);   // columns`

```
// columns
```

## L280 · `box_pass(&tmp, plane, sh, sw, sw, 1, radius);       // rows`

```
// rows
```

## L284-285 · `{`

```
// What the glass will sit on: luma percentiles and mean colour of the
// blurred image. `shade::glass` derives its values from these.
```

## L311 · `let half = (BLUR_DOWN / 2) as i32;`

```
// 3. Bilinear back up to screen size (sample centres of the blocks).
```

## L329 · `unsafe { *(dst.add(y as usize * pitch + x as usize * 4) as *mut u32) = px; }`

```
// SAFETY: same bounds and layout as the wallpaper buffer above.
```

## L344-345 · `fn box_pass(src: &[u32], dst: &mut [u32], lines: usize, len: usize, stride: usize, step: usize,`

```
/// One box-blur pass along a line direction. `lines` × `len` samples,
/// `step` between samples of a line, `stride` between lines. Edges clamp.
```

## L362-363 · `pub fn glass_source_ptr() -> *const u8 {`

```
/// What translucent glass is precomputed from: the blurred wallpaper when
/// it is ready, else the sharp one, else null (no wallpaper).
```

## L372-374 · `pub fn draw_glass_backdrop(shadow: *mut u8, info: &FbInfo,`

```
/// Put the blurred wallpaper under a glass window: inside the rounded rect
/// only, so the corners outside it keep the sharp wallpaper the gap shows.
/// Call after the sharp restore. No wallpaper → nothing to blur, no-op.
```

## L390-391 · `unsafe { core::ptr::copy_nonoverlapping(bl.add(off), shadow.add(off), (x1 - x0) as usize * 4); }`

```
// SAFETY: x0..x1 and y lie inside the screen; both buffers are
// screen-sized with the framebuffer pitch.
```

## L396-399 · `pub fn glass_base_at(info: &FbInfo, x: u32, y: u32, current: u32) -> u32 {`

```
/// What a glass pixel at (x,y) should blend over. Where the shadow still
/// holds exactly the wallpaper, nothing else is underneath, and the blurred
/// copy takes its place; over a window the window stays (the dock can
/// slide over a tile).
```

## L403-404 · `unsafe {`

```
// SAFETY: caller passes on-screen coordinates; both buffers are
// screen-sized with the framebuffer pitch.
```

## L418 · `fn scale_cover(src: &[u8], w: u32, h: u32, dst: *mut u8, info: &FbInfo) {`

```
// ── Scaling ───────────────────────────────────────────────────────────
```

## L420-425 · `fn scale_cover(src: &[u8], w: u32, h: u32, dst: *mut u8, info: &FbInfo) {`

```
/// Fit the image to the screen the way a photo frame does: fill it, keep
/// the aspect ratio, crop the overhang evenly (centre). Shrinking averages
/// every source pixel a screen pixel covers — a 4K image on an HD screen
/// otherwise keeps every second pixel and drops the rest, and fine lines
/// turn into stairs. Enlarging interpolates bilinearly. Done once, when the
/// wallpaper is set.
```

## L430-431 · `let (cw, ch) = if w64 * th > h64 * tw {`

```
// Source window that maps onto the screen, in 16.16 fixed point:
// the larger of the two scale factors wins, the other axis is cropped.
```

## L433 · `(h64 * tw * 65536 / th, h64 * 65536)        // wider than the screen`

```
// wider than the screen
```

## L435 · `(w64 * 65536, w64 * th * 65536 / tw)        // taller (or equal)`

```
// taller (or equal)
```

## L454 · `let (sx0, sx1) = (fx0 >> 16, ((fx1 + 65535) >> 16).max((fx0 >> 16) + 1));`

```
// Box average over the covered source pixels.
```

## L467 · `let cx = ((fx0 + fx1) / 2).saturating_sub(32768);`

```
// Bilinear at the pixel centre.
```

## L478-479 · `unsafe { *row.add(tx as usize) = (r << 16) | (g << 8) | b; }`

```
// SAFETY: tx < width, ty < height; `dst` is screen-sized with
// the framebuffer pitch (allocated in set_wallpaper).
```

