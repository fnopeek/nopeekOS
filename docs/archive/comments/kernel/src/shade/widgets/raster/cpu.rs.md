# `kernel/src/shade/widgets/raster/cpu.rs` @ 5e0102684

## L1-12 · `#![allow(dead_code)]`

```
//! CPU rasterizer — software-draws rects, text (fontdue), icon stubs.
//!
//! Target-agnostic: the same code paints a tile or a composition layer.
//! All coordinates the compositor passes in are **window-space**; we
//! subtract `target.origin` to get target-local positions, then clip to
//! `target.size`. This is what makes tile-boundary drawing "just work"
//! — a draw that straddles two tiles clips in-place on each.
//!
//! P10.5 implements the essentials: clear, rect (solid fill),
//! text (glyph composite via `gui::text`), icon (stub until P10.9),
//! canvas_copy (raw BGRA memcpy). blur / shadow / effect are default
//! no-ops from the trait — GPU backend implements them (Phase 12).
```

## L20-21 · `pub struct CpuRasterizer;`

```
/// CPU-backed rasterizer. Holds no state between calls — safe to share
/// across raster workers once Phase 9 threading kicks in.
```

## L66 · `let text_color = t.palette.colors[color as usize];`

```
// Caller-resolved glyph colour (style-default or a Tint override).
```

## L69-70 · `let ascent_f = crate::gui::text::ascent_px(style, size_px);`

```
// Baseline start point in window coords → target-local.
// `f32::ceil` isn't in core; inline the positive-only version.
```

## L80 · `let mut pen_x: f32 = tx as f32;`

```
// Per-char pen position.
```

## L88 · `if let Some(prev_ch) = prev {`

```
// Kerning with previous char (Inter `kern` feature).
```

## L93-95 · `let drew = crate::gui::text::rasterize_cached_px(ch, style, size_px, |glyph| {`

```
// Rasterize glyph via cached-path, composite alpha onto
// target pixels. The cache handles its own GGTT slot too
// (P10.4 glyph-atlas migration).
```

## L100-102 · `let gx = pen_x as i32 + glyph.xmin as i32;`

```
// Place top-left of glyph bitmap at:
//   x = pen + glyph.xmin
//   y = baseline - (glyph.height + glyph.ymin)
```

## L120 · `if id == IconId::None { return; }`

```
// Skip None sentinel — caller asked for no icon.
```

## L125-126 · `match crate::gui::icons::alpha_for(id, size) {`

```
// Phosphor atlas path — picks nearest-but-not-smaller size.
// Falls back to stub square if atlas isn't loaded yet.
```

## L132 · `composite_alpha_scaled(t, x, y, size as u32, atlas_size as u32, &alpha, bgra);`

```
// Nearest-neighbour scale atlas_size → size.
```

## L137-138 · `fill_rect_target(t, x, y, size as i32, size as i32, bgra, 200);`

```
// Atlas not yet loaded or icon missing — stub square
// keeps layout debuggable.
```

## L179-183 · `let one_to_one =`

```
// One destination pixel per source pixel: BGRA little-endian already
// IS the target word, so a row is a straight copy. Measured 21x
// cheaper than the scaling walk, and it is the case a canvas painted
// at its own size (`npk_canvas_rect`) hits on every commit. The
// third term is what makes the unchecked slice below sound.
```

## L212-216 · `let c = p.coeffs;`

```
// Y'CbCr -> BGR happens HERE, not in the module, and per DESTINATION
// pixel, not per source pixel. A 1080p frame in a 720p window costs
// 0.92 Mpx instead of 2.07, and it runs native (SSE/AVX2 are in the
// target spec) instead of inside an interpreter-free but SIMD-free
// forge module, where the same loop measured 145 % of a core.
```

## L223-225 · `for py in f.y0..f.y1 {`

```
// Every index below is inside its plane because `canvas::commit_i420`
// proved it: `sy < sh`, `sx < sw`, `y.len() >= ys*sh`,
// `u.len()/v.len() >= cs*ceil(sh/2)`.
```

## L249 · `fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}`

```
// blur / shadow / effect use the default trait impls (no-op).
```

## L255 · `struct CanvasFit {`

```
// ── Canvas geometry + the source walk ────────────────────────────────
```

## L257-260 · `struct CanvasFit {`

```
/// Where a contain-fit canvas lands in the target and which part of it
/// survives the clip. Shared by both canvas blits so the two cannot drift
/// apart — the fit, the zoom and the pan clamp are one piece of arithmetic
/// with one owner.
```

## L275-276 · `let rw = rect.w;`

```
// Contain-fit: the larger of the two ratios that still keeps the whole
// image inside the rect, aspect preserved.
```

## L280 · `let by_w_h = (sh * rw + sw / 2) / sw; // height if scaled to rw`

```
// height if scaled to rw
```

## L284 · `let by_h_w = (sw * rh + sh / 2) / sh; // width if scaled to rh`

```
// width if scaled to rh
```

## L288-289 · `let z = zoom_q88.max(1) as u64;`

```
// Zoom multiplies the fitted size; the image stays centred and the
// rect crops it. 256 = plain fit.
```

## L295-299 · `let over_x = (dst_w as i32 - rw as i32).max(0) / 2;`

```
// Signed: zoomed past the rect the origin goes negative. Pan is clamped
// to the overhang — the amount by which the scaled image exceeds the
// rect — so dragging stops at the image edge instead of pulling it off
// into the background. Nothing overhangs on an axis that still fits, so
// that axis simply cannot be moved.
```

## L307-310 · `let (clx0, cly0, clx1, cly1) = local_clip(t);`

```
// Walk only the pixels that can survive: the target clip, the canvas
// rect and the scaled image all intersected up front. Iterating the
// whole destination and skipping per pixel is, at high zoom, most of
// the work for nothing.
```

## L321-328 · `#[derive(Clone, Copy)]`

```
/// Nearest-neighbour source index that WALKS instead of being divided for.
///
/// `idx` grows by the whole part of the step and `acc` carries the
/// remainder, tipping one further exactly when it reaches `den`. That is
/// the same number as `d * num / den` — verified bit-for-bit against the
/// division over 35 combinations of destination size and offset
/// (`<tools>/mediabench/src/blit_equiv.rs`) — without a 64-bit division
/// per pixel. At 1080p that division ran two million times per frame.
```

## L339-341 · `fn new(d0: i32, num: u32, den: u32) -> Step {`

```
/// `d0` is the first destination offset, and must not be negative —
/// both callers derive it from a bound that already clamped against
/// the origin.
```

## L364 · `fn window_to_target(t: &RasterTarget, wx: i32, wy: i32) -> (i32, i32) {`

```
// ── Pixel helpers ────────────────────────────────────────────────────
```

## L366-367 · `fn window_to_target(t: &RasterTarget, wx: i32, wy: i32) -> (i32, i32) {`

```
/// Convert a point from window coordinates to target-local (pixel
/// offset inside target.pixels).
```

## L372-375 · `#[inline]`

```
/// Effective draw bounds in target-local coords: the target size,
/// further narrowed by any active `Widget::Scroll` clip rect. Every
/// blit helper clamps to this so scrolled content can't paint outside
/// its viewport.
```

## L388-389 · `fn fill_rect_target(t: &mut RasterTarget, x: i32, y: i32, w: i32, h: i32, color: u32, alpha: u8) {`

```
/// Fill a rectangle in target-local coordinates, clipping to the
/// target size. `alpha` is 0..=255; 255 = fully opaque overwrite.
```

## L418-419 · `fn composite_alpha_target(`

```
/// Composite an alpha bitmap onto the target using a single solid
/// color. Matches fontdue's 1-byte-per-pixel output.
```

## L446-447 · `fn composite_alpha_scaled(`

```
/// Nearest-neighbour scale an atlas alpha bitmap onto the target.
/// `size` is the final edge length; `atlas_size` is the source edge.
```

## L461-465 · `let downscale = atlas_size > size;`

```
// Downscale: average the source box a destination pixel covers.
// Nearest-neighbour drops whole rows of a 1.5 px phosphor stroke, so
// an 18 px icon taken from the 24 px atlas came out visibly ragged.
// Upscale keeps nearest — the atlas has a size above every request we
// make, so this is the path that runs.
```

## L509-512 · `fn corner_coverage(dx: i32, dy: i32, r: i32) -> u8 {`

```
// 16x16 subpixel coverage, centered (256 levels). Solid fills at
// small radii need this density — 64 levels produced visible alpha
// plateaus on same-colour inner content where gradient strokes
// could hide them in colour variance.
```

## L533 · `let ba = t.bg_alpha;  // background fill opacity (255 = opaque)`

```
// background fill opacity (255 = opaque)
```

## L603-604 · `fn rect_coverage(px: i32, py: i32, rx: i32, ry: i32, rw: i32, rh: i32, r: i32) -> u8 {`

```
// Pixel coverage inside a rounded rectangle. Returns 255 for straight
// edges / interior, 0 outside, AA for the four corner arcs.
```

## L613-625 · `let cx = if px < rx + r { rx + r - 1 } else { rx + rw - r };`

```
// Dieselben Mittelpunkte wie `fill_rounded_rect_target`, und das ist
// keine Kosmetik. Dort ist der Versatz `r - 1 - col`, der Mittelpunkt
// liegt also auf Pixel `rx + r - 1` und `rx + rw - r`. Hier stand
// `rx + r` und `rx + rw - 1 - r` — um EINEN Pixel daneben, auf allen
// vier Ecken, seit es die Funktion gibt.
//
// Bei den Radien, mit denen bisher gemalt wurde (4 bis 8), ist das
// ein Versatz unter der Aufmerksamkeitsschwelle. Bei einer PILLE
// (r = h/2) kippt es: mit `ry + r` und `ry + rh - 1 - r` liegt der
// UNTERE Mittelpunkt einen Pixel UEBER dem oberen, die beiden
// Halbkreise ueberlappen verkehrt, und die Form wird in der Mitte
// eingeschnuert. Der Strich lief dann sichtbar neben seiner eigenen
// Flaeche — gemeldet an der Bar, h = 32, r = 16.
```

## L631-632 · `fn blend_over(dst: u32, src: u32, src_alpha: u8) -> u32 {`

```
/// Standard "over" alpha blend with 8-bit src alpha. Keeps dst alpha
/// at 0xFF (targets are always opaque for now).
```

## L638 · `let da = (dst >> 24) & 0xFF;        // destination alpha (0 for a transparent scene)`

```
// destination alpha (0 for a transparent scene)
```

## L639-644 · `let dst_contrib = da * (255 - sa) / 255;`

```
// Straight-alpha "over": preserves the alpha channel instead of forcing
// opaque. For an OPAQUE dst (da == 255) this reduces to the old formula
// (out_a = 255, same colour) — so opaque scenes are byte-identical and
// every existing app renders unchanged. A transparent scene accumulates
// real coverage alpha, which the compositor then composites over the
// wallpaper (translucent bar pills, crisp glyphs, no halo).
```

## L667-673 · `#[test]`

```
/// Strich und Fuellung muessen DIESELBE Form meinen.
///
/// `fill_rounded_rect_target` baut seine Ecken zeilenweise mit
/// `corner_coverage(r-1-col, r-1-row, r)`. `rect_coverage` — von dem
/// der STRICH lebt — rechnet dieselbe Ecke analytisch. Standen die
/// Mittelpunkte einen Pixel auseinander, lief der Rahmen neben seiner
/// eigenen Flaeche.
```

## L681 · `for &(px, py) in &[`

```
// alle vier Ecken gegen die Fuellung halten
```

## L683 · `(col, row),                 // oben links`

```
// oben links
```

## L684 · `(w - 1 - col, row),         // oben rechts`

```
// oben rechts
```

## L685 · `(col, h - 1 - row),         // unten links`

```
// unten links
```

## L686 · `(w - 1 - col, h - 1 - row), // unten rechts`

```
// unten rechts
```

## L701-704 · `#[test]`

```
/// Eine Pille (r = h/2) ist der Fall, in dem der alte Fehler kippte:
/// die zwei Bogenmittelpunkte tauschten die Reihenfolge und schnuerten
/// die Form in der Mitte ein. Die Silhouette muss senkrecht
/// spiegelsymmetrisch sein und in der Mitte am breitesten.
```

