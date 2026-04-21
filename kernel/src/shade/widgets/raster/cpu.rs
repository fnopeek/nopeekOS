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
        let color = match fill {
            Fill::Solid(tok) => t.palette.colors[tok as usize],
            _                => return,
        };
        fill_rect_target(t, x, y, r.w as i32, r.h as i32, color, 255);
    }

    fn text(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, p: Point) {
        // Resolve text color: Muted style uses OnSurfaceMuted,
        // everything else OnSurface. (Theme color is not yet exposed
        // through RasterTarget — palette lookup via Token enum.)
        let color_tok = match style {
            TextStyle::Muted => Token::OnSurfaceMuted,
            _                => Token::OnSurface,
        };
        let text_color = t.palette.colors[color_tok as usize];

        // Baseline start point in window coords → target-local.
        // `f32::ceil` isn't in core; inline the positive-only version.
        let ascent_f = crate::gui::text::ascent(style);
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
                pen_x += crate::gui::text::kern(prev_ch, ch, style);
            }

            // Rasterize glyph via cached-path, composite alpha onto
            // target pixels. The cache handles its own GGTT slot too
            // (P10.4 glyph-atlas migration).
            let drew = crate::gui::text::rasterize_cached(ch, style, |glyph| {
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
        let stride = t.stride as usize;
        for cy in 0..h {
            let dst_row = cy as usize * stride;
            let src_row = cy as usize * (w as usize) * 4;
            if cy as u32 >= t.size.h { break; }
            for cx in 0..w {
                if cx >= t.size.w { break; }
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

/// Fill a rectangle in target-local coordinates, clipping to the
/// target size. `alpha` is 0..=255; 255 = fully opaque overwrite.
fn fill_rect_target(t: &mut RasterTarget, x: i32, y: i32, w: i32, h: i32, color: u32, alpha: u8) {
    if w <= 0 || h <= 0 { return; }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(t.size.w as i32);
    let y1 = (y + h).min(t.size.h as i32);
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
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(t.size.w as i32);
    let y1 = (y + h as i32).min(t.size.h as i32);
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
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + size as i32).min(t.size.w as i32);
    let y1 = (y + size as i32).min(t.size.h as i32);
    if x0 >= x1 || y0 >= y1 { return; }

    let stride = t.stride as usize;
    for py in y0..y1 {
        let ly = (py - y) as u32;
        let sy = (ly * atlas_size / size) as usize;
        let dst_base = py as usize * stride;
        for px in x0..x1 {
            let lx = (px - x) as u32;
            let sx = (lx * atlas_size / size) as usize;
            let a_idx = sy * atlas_size as usize + sx;
            if a_idx >= alpha.len() { continue; }
            let a = alpha[a_idx];
            if a == 0 { continue; }
            let dst = t.pixels[dst_base + px as usize];
            t.pixels[dst_base + px as usize] = blend_over(dst, color, a);
        }
    }
}

/// Standard "over" alpha blend with 8-bit src alpha. Keeps dst alpha
/// at 0xFF (targets are always opaque for now).
fn blend_over(dst: u32, src: u32, src_alpha: u8) -> u32 {
    if src_alpha == 0 { return dst; }
    if src_alpha == 255 { return src; }

    let sa  = src_alpha as u32;
    let inv = 255 - sa;

    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8)  & 0xFF;
    let db =  dst        & 0xFF;

    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8)  & 0xFF;
    let sb =  src        & 0xFF;

    let r = (sr * sa + dr * inv) / 255;
    let g = (sg * sa + dg * inv) / 255;
    let b = (sb * sa + db * inv) / 255;

    0xFF_00_00_00 | (r << 16) | (g << 8) | b
}
