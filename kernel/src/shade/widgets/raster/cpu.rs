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
    Fill, IconId, Point, RasterTarget, Rasterizer, Rect, Shadow, TextStyle, Token,
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

    fn canvas_blit(&mut self, t: &mut RasterTarget, src: &[u8], sw: u32, sh: u32, rect: Rect) {
        if sw == 0 || sh == 0 || rect.w == 0 || rect.h == 0 { return; }
        if src.len() < (sw * sh * 4) as usize { return; }
        // Contain-fit: largest integer scale (numerator/denominator) that
        // keeps the whole image inside the rect, preserving aspect.
        // dst_w = sw * rect.w / sw capped... compute via the smaller ratio.
        let rw = rect.w;
        let rh = rect.h;
        // Choose dst size: scale so that dst_w<=rw and dst_h<=rh, max one.
        let (dst_w, dst_h) = {
            // fit by width vs by height, pick the one that fits both.
            let by_w_h = (sh * rw + sw / 2) / sw; // height if scaled to rw
            if by_w_h <= rh {
                (rw, by_w_h.max(1))
            } else {
                let by_h_w = (sw * rh + sh / 2) / sh; // width if scaled to rh
                (by_h_w.max(1), rh)
            }
        };
        let (rx, ry) = window_to_target(t, rect.x, rect.y);
        let ox = rx + ((rw - dst_w) / 2) as i32;
        let oy = ry + ((rh - dst_h) / 2) as i32;
        let (clx0, cly0, clx1, cly1) = local_clip(t);
        let stride = t.stride as usize;
        for dy in 0..dst_h {
            let py = oy + dy as i32;
            if py < cly0 || py >= cly1 { continue; }
            let sy = (dy * sh) / dst_h; // nearest-neighbour
            let src_row = (sy as usize) * (sw as usize) * 4;
            let dst_row = (py as usize) * stride;
            for dx in 0..dst_w {
                let px = ox + dx as i32;
                if px < clx0 || px >= clx1 { continue; }
                let sx = (dx * sw) / dst_w;
                let src_off = src_row + (sx as usize) * 4;
                let b = src[src_off]     as u32;
                let g = src[src_off + 1] as u32;
                let r = src[src_off + 2] as u32;
                let a = src[src_off + 3] as u32;
                t.pixels[dst_row + px as usize] = (a << 24) | (r << 16) | (g << 8) | b;
            }
        }
    }

    // blur / shadow / effect use the default trait impls (no-op).
    fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}
    fn shadow(&mut self, _t: &mut RasterTarget, _r: Rect, _s: Shadow) {}
    fn effect(&mut self, _t: &mut RasterTarget, _r: Rect, _id: crate::shade::widgets::abi::EffectId) {}
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

    let cx = if px < rx + r { rx + r } else { rx + rw - 1 - r };
    let cy = if py < ry + r { ry + r } else { ry + rh - 1 - r };
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
