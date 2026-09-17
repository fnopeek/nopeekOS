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

#![allow(dead_code)]

use crate::shade::widgets::abi::{
    Fill, I420Ref, IconId, Point, RasterTarget, Rasterizer, Rect, Shadow, TextStyle, Token,
};

/// CPU-backed rasterizer. Holds no state between calls — safe to share
/// across raster workers once Phase 9 threading kicks in.
pub struct CpuRasterizer;

impl CpuRasterizer {
    pub fn new() -> Self {
        Self
    }
}

impl Rasterizer for CpuRasterizer {
    fn clear(&mut self, t: &mut RasterTarget, color: Token) {
        let bgra = t.palette.colors[color as usize];
        fill_rect_target(t, 0, 0, t.size.w as i32, t.size.h as i32, bgra, 255);
    }

    fn rect(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill) {
        let (x, y) = window_to_target(t, r.x, r.y);
        let Fill::Solid(tok) = fill;
        let color = t.palette.colors[tok as usize];
        let ba = t.bg_alpha;
        fill_rect_target(t, x, y, r.w as i32, r.h as i32, color, ba);
    }

    fn rect_rounded(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill, radius: u8) {
        let (x, y) = window_to_target(t, r.x, r.y);
        let Fill::Solid(tok) = fill;
        let color = t.palette.colors[tok as usize];
        fill_rounded_rect_target(t, x, y, r.w as i32, r.h as i32, radius as i32, color);
    }

    fn stroke_rounded(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill, width: u8, radius: u8) {
        if width == 0 { return; }
        let (x, y) = window_to_target(t, r.x, r.y);
        let Fill::Solid(tok) = fill;
        let color = t.palette.colors[tok as usize];
        stroke_rounded_rect_target(t, x, y, r.w as i32, r.h as i32, radius as i32, width as i32, color);
    }

    fn text(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, color: Token, p: Point) {
        let size = crate::gui::text::style_desc(style).size_px;
        self.text_px(t, s, style, size, color, p);
    }

    fn text_px(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, size_px: u16,
               color: Token, p: Point) {
        // Caller-resolved glyph colour (style-default or a Tint override).
        let text_color = t.palette.colors[color as usize];

        // Baseline start point in window coords → target-local.
        // `f32::ceil` isn't in core; inline the positive-only version.
        let ascent_f = crate::gui::text::ascent_px(style, size_px);
        let ascent_i = if ascent_f <= 0.0 {
            0
        } else {
            let i = ascent_f as i32;
            if (i as f32) < ascent_f { i + 1 } else { i }
        };
        let (tx, ty_baseline) = window_to_target(t, p.x, p.y + ascent_i);

        // Per-char pen position.
        let mut pen_x: f32 = tx as f32;
        let pen_y_baseline: i32 = ty_baseline;
        let mut prev: Option<char> = None;

        for ch in s.chars() {
            if ch == '\n' || ch == '\r' { continue; }

            // Kerning with previous char (Inter `kern` feature).
            if let Some(prev_ch) = prev {
                pen_x += crate::gui::text::kern_px(prev_ch, ch, style, size_px);
            }

            // Rasterize glyph via cached-path, composite alpha onto
            // target pixels. The cache handles its own GGTT slot too
            // (P10.4 glyph-atlas migration).
            let drew = crate::gui::text::rasterize_cached_px(ch, style, size_px, |glyph| {
                if glyph.width == 0 || glyph.height == 0 {
                    return glyph.advance;
                }
                // Place top-left of glyph bitmap at:
                //   x = pen + glyph.xmin
                //   y = baseline - (glyph.height + glyph.ymin)
                let gx = pen_x as i32 + glyph.xmin as i32;
                let gy = pen_y_baseline
                    - (glyph.height as i32 + glyph.ymin as i32);
                composite_alpha_target(
                    t, gx, gy, glyph.width as u32, glyph.height as u32,
                    &glyph.alpha, text_color,
                );
                glyph.advance
            });

            let adv = drew.unwrap_or(0.0);
            pen_x += adv;
            prev = Some(ch);
        }
    }

    fn icon(&mut self, t: &mut RasterTarget, id: IconId, size: u16, color: Token, p: Point) {
        // Skip None sentinel — caller asked for no icon.
        if id as u16 == 0 { return; }
        let (x, y) = window_to_target(t, p.x, p.y);
        let bgra = t.palette.colors[color as usize];

        // Phosphor atlas path — picks nearest-but-not-smaller size.
        // Falls back to stub square if atlas isn't loaded yet.
        match crate::gui::icons::alpha_for(id, size) {
            Some((atlas_size, alpha)) => {
                if atlas_size == size {
                    composite_alpha_target(t, x, y, size as u32, size as u32, &alpha, bgra);
                } else {
                    // Nearest-neighbour scale atlas_size → size.
                    composite_alpha_scaled(t, x, y, size as u32, atlas_size as u32, &alpha, bgra);
                }
            }
            None => {
                // Atlas not yet loaded or icon missing — stub square
                // keeps layout debuggable.
                fill_rect_target(t, x, y, size as i32, size as i32, bgra, 200);
            }
        }
    }

    fn canvas_copy(&mut self, t: &mut RasterTarget, src: &[u8], w: u16, h: u16) {
        let w = w as u32;
        let h = h as u32;
        if src.len() < (w * h * 4) as usize { return; }
        let (clx0, cly0, clx1, cly1) = local_clip(t);
        let stride = t.stride as usize;
        for cy in 0..h {
            let dst_row = cy as usize * stride;
            let src_row = cy as usize * (w as usize) * 4;
            if (cy as i32) < cly0 { continue; }
            if cy as i32 >= cly1 { break; }
            for cx in 0..w {
                if (cx as i32) < clx0 { continue; }
                if cx as i32 >= clx1 { break; }
                let src_off = src_row + (cx as usize) * 4;
                let dst_off = dst_row + cx as usize;
                let b = src[src_off]     as u32;
                let g = src[src_off + 1] as u32;
                let r = src[src_off + 2] as u32;
                let a = src[src_off + 3] as u32;
                t.pixels[dst_off] = (a << 24) | (r << 16) | (g << 8) | b;
            }
        }
    }

    fn canvas_blit(&mut self, t: &mut RasterTarget, src: &[u8], sw: u32, sh: u32,
                   rect: Rect, zoom_q88: u32, pan: (i32, i32)) {
        if src.len() < (sw as usize) * (sh as usize) * 4 { return; }
        let Some(f) = canvas_fit(t, sw, sh, rect, zoom_q88, pan) else { return };

        let stride = t.stride as usize;
        let n = (f.x1 - f.x0) as usize;
        let mut ys = Step::new(f.y0 - f.oy, sh, f.dst_h);
        let xs0 = Step::new(f.x0 - f.ox, sw, f.dst_w);

        // One destination pixel per source pixel: BGRA little-endian already
        // IS the target word, so a row is a straight copy. Measured 21x
        // cheaper than the scaling walk, and it is the case a canvas painted
        // at its own size (`npk_canvas_rect`) hits on every commit. The
        // third term is what makes the unchecked slice below sound.
        let one_to_one =
            sw == f.dst_w && sh == f.dst_h && xs0.idx as usize + n <= sw as usize;

        for py in f.y0..f.y1 {
            let src_row = (ys.idx.min(sh - 1) as usize) * (sw as usize) * 4;
            let dst_row = (py as usize) * stride + f.x0 as usize;
            let out = &mut t.pixels[dst_row..dst_row + n];
            if one_to_one {
                let s = src_row + xs0.idx as usize * 4;
                for (d, c) in out.iter_mut().zip(src[s..s + n * 4].chunks_exact(4)) {
                    *d = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                }
            } else {
                let mut xs = xs0;
                for d in out.iter_mut() {
                    let o = src_row + (xs.idx.min(sw - 1) as usize) * 4;
                    *d = u32::from_le_bytes([src[o], src[o + 1], src[o + 2], src[o + 3]]);
                    xs.step();
                }
            }
            ys.step();
        }
    }

    fn canvas_blit_i420(&mut self, t: &mut RasterTarget, p: &I420Ref, sw: u32, sh: u32,
                        rect: Rect, zoom_q88: u32, pan: (i32, i32)) {
        let Some(f) = canvas_fit(t, sw, sh, rect, zoom_q88, pan) else { return };

        // Y'CbCr -> BGR happens HERE, not in the module, and per DESTINATION
        // pixel, not per source pixel. A 1080p frame in a 720p window costs
        // 0.92 Mpx instead of 2.07, and it runs native (SSE/AVX2 are in the
        // target spec) instead of inside an interpreter-free but SIMD-free
        // forge module, where the same loop measured 145 % of a core.
        let c = p.coeffs;
        let stride = t.stride as usize;
        let n = (f.x1 - f.x0) as usize;
        let mut ys = Step::new(f.y0 - f.oy, sh, f.dst_h);
        let xs0 = Step::new(f.x0 - f.ox, sw, f.dst_w);

        // Every index below is inside its plane because `canvas::commit_i420`
        // proved it: `sy < sh`, `sx < sw`, `y.len() >= ys*sh`,
        // `u.len()/v.len() >= cs*ceil(sh/2)`.
        for py in f.y0..f.y1 {
            let sy = ys.idx.min(sh - 1) as usize;
            let y_row = sy * p.ys;
            let c_row = (sy >> 1) * p.cs;
            let dst_row = (py as usize) * stride + f.x0 as usize;
            let out = &mut t.pixels[dst_row..dst_row + n];
            let mut xs = xs0;
            for d in out.iter_mut() {
                let sx = xs.idx.min(sw - 1) as usize;
                let yy = (p.y[y_row + sx] as i32 - c.y_off) * c.y_mul;
                let uu = p.u[c_row + (sx >> 1)] as i32 - 128;
                let vv = p.v[c_row + (sx >> 1)] as i32 - 128;
                let r = ((yy + c.r_v * vv + 128) >> 8).clamp(0, 255) as u32;
                let g = ((yy - c.g_u * uu - c.g_v * vv + 128) >> 8).clamp(0, 255) as u32;
                let b = ((yy + c.b_u * uu + 128) >> 8).clamp(0, 255) as u32;
                *d = 0xff00_0000 | (r << 16) | (g << 8) | b;
                xs.step();
            }
            ys.step();
        }
    }


    // blur / shadow / effect use the default trait impls (no-op).
    fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}
    fn shadow(&mut self, _t: &mut RasterTarget, _r: Rect, _s: Shadow) {}
    fn effect(&mut self, _t: &mut RasterTarget, _r: Rect, _id: crate::shade::widgets::abi::EffectId) {}
}

// ── Canvas geometry + the source walk ────────────────────────────────

/// Where a contain-fit canvas lands in the target and which part of it
/// survives the clip. Shared by both canvas blits so the two cannot drift
/// apart — the fit, the zoom and the pan clamp are one piece of arithmetic
/// with one owner.
struct CanvasFit {
    ox: i32,
    oy: i32,
    dst_w: u32,
    dst_h: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

fn canvas_fit(t: &RasterTarget, sw: u32, sh: u32, rect: Rect,
              zoom_q88: u32, pan: (i32, i32)) -> Option<CanvasFit> {
    if sw == 0 || sh == 0 || rect.w == 0 || rect.h == 0 { return None; }
    // Contain-fit: the larger of the two ratios that still keeps the whole
    // image inside the rect, aspect preserved.
    let rw = rect.w;
    let rh = rect.h;
    let (fit_w, fit_h) = {
        let by_w_h = (sh * rw + sw / 2) / sw; // height if scaled to rw
        if by_w_h <= rh {
            (rw, by_w_h.max(1))
        } else {
            let by_h_w = (sw * rh + sh / 2) / sh; // width if scaled to rh
            (by_h_w.max(1), rh)
        }
    };
    // Zoom multiplies the fitted size; the image stays centred and the
    // rect crops it. 256 = plain fit.
    let z = zoom_q88.max(1) as u64;
    let dst_w = (((fit_w as u64 * z) / 256).max(1)).min(u32::MAX as u64) as u32;
    let dst_h = (((fit_h as u64 * z) / 256).max(1)).min(u32::MAX as u64) as u32;

    let (rx, ry) = window_to_target(t, rect.x, rect.y);
    // Signed: zoomed past the rect the origin goes negative. Pan is clamped
    // to the overhang — the amount by which the scaled image exceeds the
    // rect — so dragging stops at the image edge instead of pulling it off
    // into the background. Nothing overhangs on an axis that still fits, so
    // that axis simply cannot be moved.
    let over_x = (dst_w as i32 - rw as i32).max(0) / 2;
    let over_y = (dst_h as i32 - rh as i32).max(0) / 2;
    let pan_x = pan.0.clamp(-over_x, over_x);
    let pan_y = pan.1.clamp(-over_y, over_y);
    let ox = rx + (rw as i32 - dst_w as i32) / 2 + pan_x;
    let oy = ry + (rh as i32 - dst_h as i32) / 2 + pan_y;

    // Walk only the pixels that can survive: the target clip, the canvas
    // rect and the scaled image all intersected up front. Iterating the
    // whole destination and skipping per pixel is, at high zoom, most of
    // the work for nothing.
    let (clx0, cly0, clx1, cly1) = local_clip(t);
    let x0 = clx0.max(rx).max(ox);
    let y0 = cly0.max(ry).max(oy);
    let x1 = clx1.min(rx + rw as i32).min(ox + dst_w as i32);
    let y1 = cly1.min(ry + rh as i32).min(oy + dst_h as i32);
    if x0 >= x1 || y0 >= y1 { return None; }

    Some(CanvasFit { ox, oy, dst_w, dst_h, x0, y0, x1, y1 })
}

/// Nearest-neighbour source index that WALKS instead of being divided for.
///
/// `idx` grows by the whole part of the step and `acc` carries the
/// remainder, tipping one further exactly when it reaches `den`. That is
/// the same number as `d * num / den` — verified bit-for-bit against the
/// division over 35 combinations of destination size and offset
/// (`<tools>/mediabench/src/blit_equiv.rs`) — without a 64-bit division
/// per pixel. At 1080p that division ran two million times per frame.
#[derive(Clone, Copy)]
struct Step {
    idx: u32,
    acc: u64,
    whole: u32,
    rem: u64,
    den: u64,
}

impl Step {
    /// `d0` is the first destination offset, and must not be negative —
    /// both callers derive it from a bound that already clamped against
    /// the origin.
    fn new(d0: i32, num: u32, den: u32) -> Step {
        let n = (d0.max(0) as u64) * num as u64;
        Step {
            idx: (n / den as u64) as u32,
            acc: n % den as u64,
            whole: num / den,
            rem: (num % den) as u64,
            den: den as u64,
        }
    }

    #[inline]
    fn step(&mut self) {
        self.idx += self.whole;
        self.acc += self.rem;
        if self.acc >= self.den {
            self.acc -= self.den;
            self.idx += 1;
        }
    }
}

// ── Pixel helpers ────────────────────────────────────────────────────

/// Convert a point from window coordinates to target-local (pixel
/// offset inside target.pixels).
fn window_to_target(t: &RasterTarget, wx: i32, wy: i32) -> (i32, i32) {
    (wx - t.origin.x, wy - t.origin.y)
}

/// Effective draw bounds in target-local coords: the target size,
/// further narrowed by any active `Widget::Scroll` clip rect. Every
/// blit helper clamps to this so scrolled content can't paint outside
/// its viewport.
#[inline]
fn local_clip(t: &RasterTarget) -> (i32, i32, i32, i32) {
    let (mut x0, mut y0, mut x1, mut y1) = (0i32, 0i32, t.size.w as i32, t.size.h as i32);
    if let Some((cx0, cy0, cx1, cy1)) = t.clip {
        x0 = x0.max(cx0);
        y0 = y0.max(cy0);
        x1 = x1.min(cx1);
        y1 = y1.min(cy1);
    }
    (x0, y0, x1, y1)
}

/// Fill a rectangle in target-local coordinates, clipping to the
/// target size. `alpha` is 0..=255; 255 = fully opaque overwrite.
fn fill_rect_target(t: &mut RasterTarget, x: i32, y: i32, w: i32, h: i32, color: u32, alpha: u8) {
    if w <= 0 || h <= 0 { return; }
    let (cx0, cy0, cx1, cy1) = local_clip(t);
    let x0 = x.max(cx0);
    let y0 = y.max(cy0);
    let x1 = (x + w).min(cx1);
    let y1 = (y + h).min(cy1);
    if x0 >= x1 || y0 >= y1 { return; }

    let stride = t.stride as usize;
    if alpha == 255 {
        for py in y0..y1 {
            let base = py as usize * stride;
            for px in x0..x1 {
                t.pixels[base + px as usize] = color;
            }
        }
    } else {
        for py in y0..y1 {
            let base = py as usize * stride;
            for px in x0..x1 {
                let dst = t.pixels[base + px as usize];
                t.pixels[base + px as usize] = blend_over(dst, color, alpha);
            }
        }
    }
}

/// Composite an alpha bitmap onto the target using a single solid
/// color. Matches fontdue's 1-byte-per-pixel output.
fn composite_alpha_target(
    t: &mut RasterTarget,
    x: i32, y: i32, w: u32, h: u32, alpha: &[u8], color: u32,
) {
    if w == 0 || h == 0 { return; }
    let (cx0, cy0, cx1, cy1) = local_clip(t);
    let x0 = x.max(cx0);
    let y0 = y.max(cy0);
    let x1 = (x + w as i32).min(cx1);
    let y1 = (y + h as i32).min(cy1);
    if x0 >= x1 || y0 >= y1 { return; }

    let stride = t.stride as usize;
    for py in y0..y1 {
        let sy = (py - y) as usize;
        let dst_base = py as usize * stride;
        for px in x0..x1 {
            let sx = (px - x) as usize;
            let a = alpha[sy * w as usize + sx];
            if a == 0 { continue; }
            let dst = t.pixels[dst_base + px as usize];
            t.pixels[dst_base + px as usize] = blend_over(dst, color, a);
        }
    }
}

/// Nearest-neighbour scale an atlas alpha bitmap onto the target.
/// `size` is the final edge length; `atlas_size` is the source edge.
fn composite_alpha_scaled(
    t: &mut RasterTarget,
    x: i32, y: i32, size: u32, atlas_size: u32, alpha: &[u8], color: u32,
) {
    if size == 0 || atlas_size == 0 { return; }
    let (cx0, cy0, cx1, cy1) = local_clip(t);
    let x0 = x.max(cx0);
    let y0 = y.max(cy0);
    let x1 = (x + size as i32).min(cx1);
    let y1 = (y + size as i32).min(cy1);
    if x0 >= x1 || y0 >= y1 { return; }

    let stride = t.stride as usize;
    // Downscale: average the source box a destination pixel covers.
    // Nearest-neighbour drops whole rows of a 1.5 px phosphor stroke, so
    // an 18 px icon taken from the 24 px atlas came out visibly ragged.
    // Upscale keeps nearest — the atlas has a size above every request we
    // make, so this is the path that runs.
    let downscale = atlas_size > size;
    for py in y0..y1 {
        let ly = (py - y) as u32;
        let sy0 = (ly * atlas_size / size) as usize;
        let sy1 = if downscale {
            (((ly + 1) * atlas_size / size) as usize).max(sy0 + 1).min(atlas_size as usize)
        } else { sy0 + 1 };
        let dst_base = py as usize * stride;
        for px in x0..x1 {
            let lx = (px - x) as u32;
            let sx0 = (lx * atlas_size / size) as usize;
            let sx1 = if downscale {
                (((lx + 1) * atlas_size / size) as usize).max(sx0 + 1).min(atlas_size as usize)
            } else { sx0 + 1 };

            let mut sum = 0u32;
            let mut n = 0u32;
            for sy in sy0..sy1 {
                let row = sy * atlas_size as usize;
                for sx in sx0..sx1 {
                    let a_idx = row + sx;
                    if a_idx >= alpha.len() { continue; }
                    sum += alpha[a_idx] as u32;
                    n += 1;
                }
            }
            if n == 0 { continue; }
            let a = (sum / n) as u8;
            if a == 0 { continue; }
            let dst = t.pixels[dst_base + px as usize];
            t.pixels[dst_base + px as usize] = blend_over(dst, color, a);
        }
    }
}

fn put_pixel_blended(t: &mut RasterTarget, x: i32, y: i32, color: u32, alpha: u8) {
    let (cx0, cy0, cx1, cy1) = local_clip(t);
    if x < cx0 || y < cy0 || x >= cx1 || y >= cy1 { return; }
    let base = y as usize * t.stride as usize + x as usize;
    let dst = t.pixels[base];
    t.pixels[base] = blend_over(dst, color, alpha);
}

// 16x16 subpixel coverage, centered (256 levels). Solid fills at
// small radii need this density — 64 levels produced visible alpha
// plateaus on same-colour inner content where gradient strokes
// could hide them in colour variance.
fn corner_coverage(dx: i32, dy: i32, r: i32) -> u8 {
    let base_dx = dx * 32;
    let base_dy = dy * 32;
    let r_scaled = r * 32;
    let r2 = r_scaled * r_scaled;
    let mut covered = 0u32;
    for sy in 0..16 {
        let sdy = base_dy + 2 * sy - 15;
        for sx in 0..16 {
            let sdx = base_dx + 2 * sx - 15;
            if sdx * sdx + sdy * sdy <= r2 {
                covered += 1;
            }
        }
    }
    (covered * 255 / 256) as u8
}

fn fill_rounded_rect_target(t: &mut RasterTarget, x: i32, y: i32, w: i32, h: i32, radius: i32, color: u32) {
    if w <= 0 || h <= 0 { return; }
    let ba = t.bg_alpha;  // background fill opacity (255 = opaque)
    let r = radius.min(w / 2).min(h / 2).max(0);
    if r == 0 {
        fill_rect_target(t, x, y, w, h, color, ba);
        return;
    }

    fill_rect_target(t, x, y + r, w, h - 2 * r, color, ba);

    for row in 0..r {
        let top_y = y + row;
        let bot_y = y + h - 1 - row;
        let dy_off = r - 1 - row;

        let mid_x0 = x + r;
        let mid_w  = w - 2 * r;
        if mid_w > 0 {
            fill_rect_target(t, mid_x0, top_y, mid_w, 1, color, ba);
            if bot_y != top_y {
                fill_rect_target(t, mid_x0, bot_y, mid_w, 1, color, ba);
            }
        }

        for col in 0..r {
            let left_x  = x + col;
            let right_x = x + w - 1 - col;
            let dx_off = r - 1 - col;
            let a = ((corner_coverage(dx_off, dy_off, r) as u32 * ba as u32) / 255) as u8;
            if a == 0 { continue; }
            put_pixel_blended(t, left_x, top_y, color, a);
            put_pixel_blended(t, right_x, top_y, color, a);
            if bot_y != top_y {
                put_pixel_blended(t, left_x, bot_y, color, a);
                put_pixel_blended(t, right_x, bot_y, color, a);
            }
        }
    }
}

fn stroke_rounded_rect_target(t: &mut RasterTarget, x: i32, y: i32, w: i32, h: i32, radius: i32, width: i32, color: u32) {
    if w <= 0 || h <= 0 || width <= 0 { return; }
    let r_out = radius.min(w / 2).min(h / 2).max(0);
    let r_in  = (r_out - width).max(0);
    let inner_x = x + width;
    let inner_y = y + width;
    let inner_w = (w - 2 * width).max(0);
    let inner_h = (h - 2 * width).max(0);

    let (cx0, cy0, cx1, cy1) = local_clip(t);
    let x0 = x.max(cx0);
    let y0 = y.max(cy0);
    let x1 = (x + w).min(cx1);
    let y1 = (y + h).min(cy1);
    if x0 >= x1 || y0 >= y1 { return; }

    let ba = t.bg_alpha;
    for py in y0..y1 {
        for px in x0..x1 {
            let a_out = rect_coverage(px, py, x, y, w, h, r_out);
            let a_in  = if inner_w > 0 && inner_h > 0 {
                rect_coverage(px, py, inner_x, inner_y, inner_w, inner_h, r_in)
            } else { 0 };
            let a = ((a_out.saturating_sub(a_in) as u32 * ba as u32) / 255) as u8;
            if a > 0 {
                put_pixel_blended(t, px, py, color, a);
            }
        }
    }
}

// Pixel coverage inside a rounded rectangle. Returns 255 for straight
// edges / interior, 0 outside, AA for the four corner arcs.
fn rect_coverage(px: i32, py: i32, rx: i32, ry: i32, rw: i32, rh: i32, r: i32) -> u8 {
    if px < rx || py < ry || px >= rx + rw || py >= ry + rh { return 0; }
    if r <= 0 { return 255; }

    let in_x_core = px >= rx + r && px < rx + rw - r;
    let in_y_core = py >= ry + r && py < ry + rh - r;
    if in_x_core || in_y_core { return 255; }

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
    let cx = if px < rx + r { rx + r - 1 } else { rx + rw - r };
    let cy = if py < ry + r { ry + r - 1 } else { ry + rh - r };
    corner_coverage(px - cx, py - cy, r)
}

/// Standard "over" alpha blend with 8-bit src alpha. Keeps dst alpha
/// at 0xFF (targets are always opaque for now).
fn blend_over(dst: u32, src: u32, src_alpha: u8) -> u32 {
    if src_alpha == 0 { return dst; }
    if src_alpha == 255 { return 0xFF00_0000 | (src & 0x00FF_FFFF); }

    let sa = src_alpha as u32;
    let da = (dst >> 24) & 0xFF;        // destination alpha (0 for a transparent scene)
    // Straight-alpha "over": preserves the alpha channel instead of forcing
    // opaque. For an OPAQUE dst (da == 255) this reduces to the old formula
    // (out_a = 255, same colour) — so opaque scenes are byte-identical and
    // every existing app renders unchanged. A transparent scene accumulates
    // real coverage alpha, which the compositor then composites over the
    // wallpaper (translucent bar pills, crisp glyphs, no halo).
    let dst_contrib = da * (255 - sa) / 255;
    let out_a = sa + dst_contrib;
    if out_a == 0 { return 0; }

    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8)  & 0xFF;
    let db =  dst        & 0xFF;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8)  & 0xFF;
    let sb =  src        & 0xFF;

    let r = (sr * sa + dr * dst_contrib) / out_a;
    let g = (sg * sa + dg * dst_contrib) / out_a;
    let b = (sb * sa + db * dst_contrib) / out_a;

    (out_a << 24) | (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod round_tests {
    use super::{corner_coverage, rect_coverage};

    /// Strich und Fuellung muessen DIESELBE Form meinen.
    ///
    /// `fill_rounded_rect_target` baut seine Ecken zeilenweise mit
    /// `corner_coverage(r-1-col, r-1-row, r)`. `rect_coverage` — von dem
    /// der STRICH lebt — rechnet dieselbe Ecke analytisch. Standen die
    /// Mittelpunkte einen Pixel auseinander, lief der Rahmen neben seiner
    /// eigenen Flaeche.
    #[test]
    fn stroke_and_fill_agree_on_every_corner() {
        for &(w, h) in &[(64, 32), (40, 24), (200, 36), (33, 33), (48, 48)] {
            for r in 1..=(w.min(h) / 2) {
                for row in 0..r {
                    for col in 0..r {
                        let want = corner_coverage(r - 1 - col, r - 1 - row, r);
                        // alle vier Ecken gegen die Fuellung halten
                        for &(px, py) in &[
                            (col, row),                 // oben links
                            (w - 1 - col, row),         // oben rechts
                            (col, h - 1 - row),         // unten links
                            (w - 1 - col, h - 1 - row), // unten rechts
                        ] {
                            let got = rect_coverage(px, py, 0, 0, w, h, r);
                            assert_eq!(
                                got, want,
                                "w={w} h={h} r={r} Pixel ({px},{py}): \
                                 Strich {got} gegen Fuellung {want}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Eine Pille (r = h/2) ist der Fall, in dem der alte Fehler kippte:
    /// die zwei Bogenmittelpunkte tauschten die Reihenfolge und schnuerten
    /// die Form in der Mitte ein. Die Silhouette muss senkrecht
    /// spiegelsymmetrisch sein und in der Mitte am breitesten.
    #[test]
    fn pill_is_symmetric_and_widest_in_the_middle() {
        let (w, h) = (200, 32);
        let r = h / 2;
        let left_edge = |y: i32| (0..w).find(|&x| rect_coverage(x, y, 0, 0, w, h, r) > 0);
        for y in 0..h / 2 {
            assert_eq!(
                left_edge(y),
                left_edge(h - 1 - y),
                "Pille bei y={y} nicht spiegelsymmetrisch zu y={}",
                h - 1 - y
            );
        }
        let mid = left_edge(h / 2 - 1).unwrap();
        assert_eq!(mid, 0, "die Pille muss auf halber Hoehe die Kante beruehren");
        for y in 0..h {
            assert!(
                left_edge(y).unwrap() >= mid,
                "y={y} ragt links ueber die breiteste Stelle hinaus"
            );
        }
    }
}
