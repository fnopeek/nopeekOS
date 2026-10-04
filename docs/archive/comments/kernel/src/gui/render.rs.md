# `kernel/src/gui/render.rs` @ 5e0102684

## L1-9 · `use crate::framebuffer::{FbConsole, FbInfo};`

```
//! Rendering primitives and damage tracking for the GUI layer.
//!
//! All drawing targets the shadow buffer. DamageTracker records dirty regions
//! and flushes them to MMIO via blit_rect.
//!
//! Rounded-rect AA is signed-distance-field based (`arc_coverage_sdf`):
//! one analytic distance per pixel + smoothstep over a fixed ~1.18 px
//! band. No supersampling. Same approach as Hyprland's shaders, ported
//! to integer Q24.8 fixed-point.
```

## L13 · `#[derive(Clone, Copy)]`

```
/// A rectangular dirty region (pixel coordinates).
```

## L22 · `pub struct DamageTracker {`

```
/// Tracks dirty regions, merges on overflow, flushes to MMIO.
```

## L87 · `pub fn put_pixel(shadow: *mut u8, info: &FbInfo, x: u32, y: u32, color: u32) {`

```
/// Write a single pixel to the shadow buffer.
```

## L92 · `unsafe { *(shadow.add(offset) as *mut u32) = color; }`

```
// SAFETY: bounds checked above, shadow buffer is large enough
```

## L105 · `pub fn fill_rect(shadow: *mut u8, info: &FbInfo, x: u32, y: u32, w: u32, h: u32, color: u32) {`

```
/// Fill a rectangle with a solid color (fast path for 32bpp).
```

## L113 · `unsafe { *row_ptr.add(col as usize) = color; }`

```
// SAFETY: col < width, row < height, within shadow buffer
```

## L127 · `pub fn read_pixel(shadow: *mut u8, info: &FbInfo, x: u32, y: u32) -> u32 {`

```
/// Read a pixel from the shadow buffer.
```

## L132 · `unsafe { *(shadow.add(offset) as *const u32) }`

```
// SAFETY: bounds checked above
```

## L139 · `#[inline(always)]`

```
/// Alpha blend: mix foreground and background. alpha = 0..256 (0=bg, 256=fg).
```

## L149 · `fn luma(c: u32) -> u32 {`

```
/// Rec. 601 luma, 0..255.
```

## L154 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// How a glass fill meets what lies beneath it (see `shade::glass`).
```

## L157 · `pub ceil: u32,`

```
/// Most luma under the (light) text on glass; 255 = none.
```

## L159 · `pub tint: u32,`

```
/// Wallpaper colour mixed into the fill, and how much (×256).
```

## L167 · `pub fn fill(&self, fg: u32) -> u32 {`

```
/// The fill colour with the wallpaper's tint mixed in.
```

## L177-184 · `pub fn glass_blend(fg: u32, bg: u32, alpha: u32, ink: GlassInk) -> u32 {`

```
/// Blend a glass fill over what lies beneath, holding the contrast bound.
///
/// `alpha` (0..256) is the fill weight where the backdrop allows it. Where
/// the backdrop is brighter than `ceil`, the fill gets just as much more
/// weight as it takes to bring it down to the ceiling — text stays legible
/// over a bright patch while the rest shows through at `alpha`. Over a
/// blurred backdrop this varies smoothly.
/// `fg` must already carry the tint (`GlassInk::fill`).
```

## L195-200 · `const AA_S_Q8: i32 = 150;       // 0.5876 px in Q24.8 (≈ M_PI / 5.34666)`

```
// ── Signed-distance-field rounded-corner AA (Hyprland-style) ──────────
//
// Q24.8 fixed-point. Pixel center at (px+0.5, py+0.5). Corner-arc-center
// at (rx+r, ry+r). One sqrt + smoothstep per pixel, no supersampling.
// AA half-band is `AA_S_Q8` ≈ 0.586 px (matches Hyprland's
// `M_PI / 5.34666` smoothing constant).
```

## L202 · `const AA_S_Q8: i32 = 150;       // 0.5876 px in Q24.8 (≈ M_PI / 5.34666)`

```
// 0.5876 px in Q24.8 (≈ M_PI / 5.34666)
```

## L203 · `const AA_TWO_S_Q8: i32 = 300;   // 2 * AA_S_Q8`

```
// 2 * AA_S_Q8
```

## L205 · `fn isqrt_u64(n: u64) -> u32 {`

```
/// Integer sqrt via Newton-Raphson. Converges in O(log n) iterations.
```

## L217-219 · `#[inline]`

```
/// SDF coverage 0..=256 of a sub-pixel position (`cdx_q8`, `cdy_q8`)
/// from the corner-arc center, against an arc of radius `r_q8` (Q24.8).
/// 256 = fully inside, 0 = fully outside, smoothstep ramp in between.
```

## L224 · `let d2 = (dx * dx + dy * dy) as u64;          // Q48.16`

```
// Q48.16
```

## L225 · `let d = isqrt_u64(d2) as i32;                 // Q24.8`

```
// Q24.8
```

## L226 · `let signed = d - r_q8;                        // Q24.8`

```
// Q24.8
```

## L235-237 · `pub fn rect_coverage_sdf(px: u32, py: u32,`

```
/// SDF coverage 0..=256 of pixel `(px, py)` inside a rounded rect at
/// `(rx, ry, rw, rh)` with corner radius `r`. AA only at the four arcs;
/// straight edges are 256 (inside) / 0 (outside).
```

## L249-250 · `let cdx_int = if in_x < r_i {`

```
// Pixel offsets from the relevant corner-arc-center, with the
// arc-centers placed at (r, r), (rw-r, r), (r, rh-r), (rw-r, rh-r).
```

## L256 · `return 256;     // x is on a straight edge`

```
// x is on a straight edge
```

## L263 · `return 256;     // y is on a straight edge`

```
// y is on a straight edge
```

## L266 · `let cdx_q8 = cdx_int * 256 + 128;`

```
// Pixel center +0.5 → Q24.8 offset of 128 from the integer offset.
```

## L272-277 · `pub fn rrect_distance_sdf(px: u32, py: u32,`

```
/// Signed distance (Q24.8) from the outline of the rounded rect to the
/// CENTRE of pixel `(px, py)`. Negative inside, positive outside.
///
/// The arc centres are the ones `rect_coverage_sdf` places — `(rx+r, ry+r)`
/// and its three mirrors — so the halo and the chrome can never disagree
/// about the shape they are drawing.
```

## L291-299 · `pub fn draw_glow_ring(shadow: *mut u8, info: &FbInfo,`

```
/// Soft halo just OUTSIDE a rounded rect, alpha falling off with distance.
///
/// The focus border is a hairline in a wallpaper-derived colour, so on a busy
/// or similarly-coloured wallpaper it disappears. A halo does not depend on
/// the colour underneath: it lifts the tile off whatever is behind it.
///
/// Paints strictly outside the rect (coverage 0), so window content and the
/// chrome's own AA edge stay untouched. Lives in the tile gap — the caller
/// keeps `width` inside it, and restores that band before repainting.
```

## L314-319 · `let d = rrect_distance_sdf(px, py, x, y, w, h, r).max(0);`

```
// One distance, not a ring search. Counting whole rings and then
// scaling by THAT ring's edge coverage looks right on a straight
// edge (coverage is always 256 there) and falls apart on the arcs,
// where every pixel lands in some ring's ~1.17 px fringe: the halo
// came out as a fan of steps with black gaps in it. Measured on the
// corner diagonal: 55 55 10 53 17 17 0 17 17 1.
```

## L322-323 · `let f = (((band_q8 + 128 - d) as i64 * 256) / band_q8 as i64).min(256) as u32;`

```
// Same profile the ring search had on a straight edge: column `i`
// out has its centre at d = i - 0.5 and got (width + 1 - i) / width.
```

## L332-333 · `let corner_lo = (y + r).min(y1);`

```
// Only the band is walked: rows that can hold a rounded corner get the
// full width, the straight middle just the two side strips.
```

## L346 · `pub fn fill_rounded_rect_aa(shadow: *mut u8, info: &FbInfo,`

```
// ── Public rounded-rect helpers ────────────────────────────────────────
```

## L349-351 · `pub fn fill_rounded_rect_aa(shadow: *mut u8, info: &FbInfo,`

```
/// Fill a rounded rectangle with anti-aliased corners (SDF).
/// Body + side strips drawn with `fill_rect`; only the four corner
/// squares (size r×r) iterate the SDF coverage helper.
```

## L394-418 · `static GLASS_TINT: spin::Mutex<Option<(u64, u32, alloc::vec::Vec<u32>)>> =`

```
/// Single-pass chrome painter. Outer SDF curve at radius `rounding`,
/// inner SDF curve at radius `rounding - border`, concentric — so the
/// radial border is uniform `border` everywhere along the curve. One
/// distance + smoothstep per curve, no supersampling.
///
/// `paint_content == true` (terminal windows): full layered chrome —
/// border ring with outer-fringe AA, inner area filled with `bg_color`,
/// inner-fringe blends content ↔ border. The terminal renderer paints
/// text on top of the bg_color.
///
/// `paint_content == false` (widget windows): the chrome paints solid
/// border in the entire (border-ring + inner-fringe) band and leaves
/// the inner-full area untouched. The widget blit then fills the inner
/// area with its own SDF AA against the border. This keeps the widget's
/// own background (cards, panes) from being undercut by `bg_color`
/// bleeding through the inner-fringe.
///
/// `border_a == border_b` paints solid; different values give a 45°
/// gradient (top-left → bottom-right).
// Precomputed "glass" tint: blend(bg_color, wallpaper, opacity) for the whole
// screen, cached and recomputed only when bg/opacity/wallpaper change. The
// translucent terminal chrome interior memcpys a row from this instead of
// blending per pixel, so it's ~2ms at ANY window size — which is what makes
// the dock glide (it resizes the terminal every frame, so the chrome cache
// can't catch it) smooth. (Recompute is ~one-off on a theme/wallpaper change.)
```

## L423-427 · `let wp = crate::gui::background::glass_source_ptr();`

```
// The blurred wallpaper when there is one — glass is readable because
// of the blur. The generation is in the key, not just the pointer:
// `set_wallpaper` overwrites the SAME buffer in place, so a pointer key
// kept the old wallpaper inside every terminal until bg_color changed
// (a light/dark switch).
```

## L445 · `let wprow = unsafe { wp.add(py * pitch_px * 4) as *const u32 };`

```
// SAFETY: wp is screen-sized (pitch*height); py<height, px<width.
```

## L456-458 · `Some((buf.as_ptr(), *pp as usize))`

```
// SAFETY: render is Core-0-only and the buffer is reallocated only on a
// key change (can't recur within this frame), so the pointer stays valid
// for the duration of this fill_rounded_chrome_aa call.
```

## L485-493 · `let straight_lo = inner_y + r_in;`

```
// The inner-full area (outer==inner==256) is the bulk of a window's
// pixels; at 4K that's millions, and computing rect_coverage_sdf TWICE per
// pixel for all of them was comp.render's entire cost (~96ms for a widget,
// ~40ms even for one terminal). On the vertically-straight rows that span
// is the known rectangle [skip_lo, skip_hi) — short-circuit the SDF there
// (outer=inner=256): widget mode `continue`s it (the widget blit paints
// it); an opaque content fill becomes a plain store; a translucent one
// still blends over the wallpaper but without the SDF. The border ring +
// the four rounded corners keep the full per-pixel SDF. Visually identical.
```

## L499-501 · `let tint = if paint_content && !opaque_fill {`

```
// Translucent terminal interior → memcpy a row from the precomputed glass
// tint (any size, ~2ms) instead of per-pixel blend. Border ring + corners
// keep the per-pixel SDF path below.
```

## L507-509 · `let ink = if paint_content { crate::shade::glass::ink() } else { GlassInk::PLAIN };`

```
// The per-pixel paths (corners, fringe, no-tint fallback) must use the
// same ink as the tint, or the straight middle sits on the glass like a
// pasted-on rectangle.
```

## L514-515 · `let paint_px = |px: u32, py: u32| {`

```
// Per-pixel SDF path — used for the border ring + the four rounded corners
// (everything that is NOT the known-interior straight span).
```

## L542 · `put_pixel(shadow, info, px, py, blend(border_color, bg_pixel, bo));`

```
// Border ring (outside the inner rect): solid border over wallpaper.
```

## L545-548 · `put_pixel(shadow, info, px, py, glass_blend(glass_fill, bg_pixel, go, ink));`

```
// Deep interior — MUST match the straight-row glass tint exactly
// (glass over wallpaper, NO border tint), else the corner bands
// show a border-coloured bar where the per-pixel path used to add
// the tint but the straight middle no longer does.
```

## L551 · `let after_border = blend(border_color, bg_pixel, bo);`

```
// Inner fringe: AA transition from border to glass over ~1 px.
```

## L564-565 · `for px in x..skip_lo.min(x_max) { paint_px(px, py); }`

```
// Straight row: per-pixel border on the left, fast interior, border on
// the right.
```

## L572 · `unsafe {`

```
// SAFETY: row within fb; [lo,hi) within width.
```

## L578-579 · `unsafe {`

```
// SAFETY: tint is screen-sized (tpitch*height); py<height,
// hi<=width; Core-0-only so tptr is valid this frame.
```

## L604-606 · `pub fn blend_pixel(shadow: *mut u8, info: &FbInfo, x: u32, y: u32, src: u32, alpha: u32) {`

```
/// Blend `src` over the existing shadow-buffer pixel at (x, y) with
/// `alpha` ∈ 0..=256. Read-modify-write helper for paths that already
/// know the coverage analytically (e.g. SDF-aware widget blits).
```

## L617 · `pub fn fill_rounded_rect_alpha(buf: *mut u8, info: &FbInfo,`

```
// ── Layer-aware rendering (writes alpha channel for compositing) ───────
```

## L619-622 · `pub fn fill_rounded_rect_alpha(buf: *mut u8, info: &FbInfo,`

```
/// Fill a rounded rectangle with color + alpha byte for layer compositing.
/// Unlike fill_rounded_rect_blend, this does NOT read existing pixels —
/// it writes color with the alpha byte set in the high byte.
/// The layer compositor handles blending with lower layers.
```

## L663 · `pub fn fill_rounded_rect_gradient_alpha(buf: *mut u8, info: &FbInfo,`

```
/// Fill a rounded rectangle with a gradient + alpha byte for layer compositing.
```

## L712-713 · `fn halo(px: u32, py: u32, x: u32, y: u32, w: u32, h: u32, r: u32,`

```
/// Alpha of the focus halo at a pixel, exactly as `draw_glow_ring`
/// computes it — without a framebuffer.
```

## L724-729 · `#[test]`

```
/// The halo must never get brighter as it gets further from the tile.
/// The ring search this replaced broke that on the four arcs — it took
/// the first whole ring reaching a pixel and scaled by THAT ring's edge
/// coverage, so a pixel could land in a fringe worth almost nothing
/// while its outer neighbour sat fully inside the next ring. Measured
/// over the same cases below: up to 79 of 100 brighter outwards.
```

## L754-756 · `#[test]`

```
/// The straight edges are what the ring search already got right, and
/// the profile there must not move: column `i` out is
/// `(band + 1 - i) / band` of the full strength.
```

