//! Rendering primitives and damage tracking for the GUI layer.
//!
//! All drawing targets the shadow buffer. DamageTracker records dirty regions
//! and flushes them to MMIO via blit_rect.
//!
//! Rounded-rect AA is signed-distance-field based (`arc_coverage_sdf`):
//! one analytic distance per pixel + smoothstep over a fixed ~1.18 px
//! band. No supersampling. Same approach as Hyprland's shaders, ported
//! to integer Q24.8 fixed-point.

use crate::framebuffer::{FbConsole, FbInfo};

/// A rectangular dirty region (pixel coordinates).
#[derive(Clone, Copy)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Tracks dirty regions, merges on overflow, flushes to MMIO.
pub struct DamageTracker {
    rects: [Option<DirtyRect>; 16],
    count: usize,
    screen_w: u32,
    screen_h: u32,
}

impl DamageTracker {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            rects: [None; 16],
            count: 0,
            screen_w: w,
            screen_h: h,
        }
    }

    #[allow(dead_code)]
    pub fn mark(&mut self, x: u32, y: u32, w: u32, h: u32) {
        if self.count >= 16 {
            self.merge_all();
        }
        self.rects[self.count] = Some(DirtyRect { x, y, w, h });
        self.count += 1;
    }

    pub fn mark_all(&mut self) {
        self.count = 0;
        self.rects[0] = Some(DirtyRect { x: 0, y: 0, w: self.screen_w, h: self.screen_h });
        self.count = 1;
    }

    pub fn flush(&mut self, console: &FbConsole) {
        for i in 0..self.count {
            if let Some(r) = self.rects[i] {
                crate::framebuffer::blit_rect(console, r.x, r.y, r.w, r.h);
            }
        }
        self.count = 0;
        for r in self.rects.iter_mut() { *r = None; }
    }

    fn merge_all(&mut self) {
        let mut min_x = self.screen_w;
        let mut min_y = self.screen_h;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        for i in 0..self.count {
            if let Some(r) = self.rects[i] {
                min_x = min_x.min(r.x);
                min_y = min_y.min(r.y);
                max_x = max_x.max(r.x + r.w);
                max_y = max_y.max(r.y + r.h);
            }
        }
        for r in self.rects.iter_mut() { *r = None; }
        self.count = 1;
        self.rects[0] = Some(DirtyRect {
            x: min_x, y: min_y,
            w: max_x.saturating_sub(min_x),
            h: max_y.saturating_sub(min_y),
        });
    }
}

/// Write a single pixel to the shadow buffer.
pub fn put_pixel(shadow: *mut u8, info: &FbInfo, x: u32, y: u32, color: u32) {
    if x >= info.width || y >= info.height { return; }
    if info.bpp == 32 {
        let offset = (y * info.pitch + x * 4) as usize;
        // SAFETY: bounds checked above, shadow buffer is large enough
        unsafe { *(shadow.add(offset) as *mut u32) = color; }
    } else {
        let bpp = (info.bpp as u32 + 7) / 8;
        let offset = (y * info.pitch + x * bpp) as usize;
        unsafe {
            *shadow.add(offset) = (color & 0xFF) as u8;
            *shadow.add(offset + 1) = ((color >> 8) & 0xFF) as u8;
            *shadow.add(offset + 2) = ((color >> 16) & 0xFF) as u8;
        }
    }
}

/// Fill a rectangle with a solid color (fast path for 32bpp).
pub fn fill_rect(shadow: *mut u8, info: &FbInfo, x: u32, y: u32, w: u32, h: u32, color: u32) {
    let x_end = (x + w).min(info.width);
    let y_end = (y + h).min(info.height);
    if info.bpp == 32 {
        for row in y..y_end {
            let row_ptr = unsafe { shadow.add((row * info.pitch) as usize) as *mut u32 };
            for col in x..x_end {
                // SAFETY: col < width, row < height, within shadow buffer
                unsafe { *row_ptr.add(col as usize) = color; }
            }
        }
    } else {
        for row in y..y_end {
            for col in x..x_end {
                put_pixel(shadow, info, col, row, color);
            }
        }
    }
}

/// Draw a border (outline) of given thickness.
#[allow(dead_code)]
pub fn draw_border(shadow: *mut u8, info: &FbInfo,
                   x: u32, y: u32, w: u32, h: u32, color: u32, thickness: u32) {
    fill_rect(shadow, info, x, y, w, thickness, color);
    fill_rect(shadow, info, x, y + h - thickness, w, thickness, color);
    fill_rect(shadow, info, x, y + thickness, thickness, h - 2 * thickness, color);
    fill_rect(shadow, info, x + w - thickness, y + thickness, thickness, h - 2 * thickness, color);
}

/// Read a pixel from the shadow buffer.
fn read_pixel(shadow: *mut u8, info: &FbInfo, x: u32, y: u32) -> u32 {
    if x >= info.width || y >= info.height { return 0; }
    if info.bpp == 32 {
        let offset = (y * info.pitch + x * 4) as usize;
        // SAFETY: bounds checked above
        unsafe { *(shadow.add(offset) as *const u32) }
    } else {
        0
    }
}

/// Alpha blend: mix foreground and background. alpha = 0..256 (0=bg, 256=fg).
#[inline(always)]
fn blend(fg: u32, bg: u32, alpha: u32) -> u32 {
    let inv = 256 - alpha;
    let r = (((fg >> 16) & 0xFF) * alpha + ((bg >> 16) & 0xFF) * inv) >> 8;
    let g = (((fg >> 8) & 0xFF) * alpha + ((bg >> 8) & 0xFF) * inv) >> 8;
    let b = ((fg & 0xFF) * alpha + (bg & 0xFF) * inv) >> 8;
    (r << 16) | (g << 8) | b
}

// ── Signed-distance-field rounded-corner AA (Hyprland-style) ──────────
//
// Q24.8 fixed-point. Pixel center at (px+0.5, py+0.5). Corner-arc-center
// at (rx+r, ry+r). One sqrt + smoothstep per pixel, no supersampling.
// AA half-band is `AA_S_Q8` ≈ 0.586 px (matches Hyprland's
// `M_PI / 5.34666` smoothing constant).

const AA_S_Q8: i32 = 150;       // 0.5876 px in Q24.8 (≈ M_PI / 5.34666)
const AA_TWO_S_Q8: i32 = 300;   // 2 * AA_S_Q8

/// Integer sqrt via Newton-Raphson. Converges in O(log n) iterations.
fn isqrt_u64(n: u64) -> u32 {
    if n < 2 { return n as u32; }
    let mut x = n;
    let mut y = (n + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x as u32
}

/// SDF coverage 0..=256 of a sub-pixel position (`cdx_q8`, `cdy_q8`)
/// from the corner-arc center, against an arc of radius `r_q8` (Q24.8).
/// 256 = fully inside, 0 = fully outside, smoothstep ramp in between.
#[inline]
fn arc_coverage_sdf(cdx_q8: i32, cdy_q8: i32, r_q8: i32) -> u32 {
    let dx = cdx_q8 as i64;
    let dy = cdy_q8 as i64;
    let d2 = (dx * dx + dy * dy) as u64;          // Q48.16
    let d = isqrt_u64(d2) as i32;                 // Q24.8
    let signed = d - r_q8;                        // Q24.8
    if signed <= -AA_S_Q8 { return 256; }
    if signed >=  AA_S_Q8 { return 0;   }
    let t = ((signed + AA_S_Q8) as i64 * 256 / AA_TWO_S_Q8 as i64) as i32;
    let three_minus_2t = 3 * 256 - 2 * t;
    let smoothed = ((t as u64) * (t as u64) * (three_minus_2t as u64) / 65536) as u32;
    256u32.saturating_sub(smoothed)
}

/// SDF coverage 0..=256 of pixel `(px, py)` inside a rounded rect at
/// `(rx, ry, rw, rh)` with corner radius `r`. AA only at the four arcs;
/// straight edges are 256 (inside) / 0 (outside).
pub fn rect_coverage_sdf(px: u32, py: u32,
                         rx: u32, ry: u32, rw: u32, rh: u32, r: u32) -> u32 {
    if px < rx || py < ry || px >= rx + rw || py >= ry + rh { return 0; }
    if r == 0 { return 256; }

    let in_x = (px - rx) as i32;
    let in_y = (py - ry) as i32;
    let r_i = r as i32;
    let rw_i = rw as i32;
    let rh_i = rh as i32;

    // Pixel offsets from the relevant corner-arc-center, with the
    // arc-centers placed at (r, r), (rw-r, r), (r, rh-r), (rw-r, rh-r).
    let cdx_int = if in_x < r_i {
        r_i - 1 - in_x
    } else if in_x >= rw_i - r_i {
        in_x - (rw_i - r_i)
    } else {
        return 256;     // x is on a straight edge
    };
    let cdy_int = if in_y < r_i {
        r_i - 1 - in_y
    } else if in_y >= rh_i - r_i {
        in_y - (rh_i - r_i)
    } else {
        return 256;     // y is on a straight edge
    };

    // Pixel center +0.5 → Q24.8 offset of 128 from the integer offset.
    let cdx_q8 = cdx_int * 256 + 128;
    let cdy_q8 = cdy_int * 256 + 128;
    arc_coverage_sdf(cdx_q8, cdy_q8, r_i * 256)
}

/// Soft halo just OUTSIDE a rounded rect, alpha falling off with distance.
///
/// The focus border is a hairline in a wallpaper-derived colour, so on a busy
/// or similarly-coloured wallpaper it disappears. A halo does not depend on
/// the colour underneath: it lifts the tile off whatever is behind it.
///
/// Paints strictly outside the rect (coverage 0), so window content and the
/// chrome's own AA edge stay untouched. Lives in the tile gap — the caller
/// keeps `width` inside it, and restores that band before repainting.
pub fn draw_glow_ring(shadow: *mut u8, info: &FbInfo,
                      x: u32, y: u32, w: u32, h: u32, r: u32,
                      color: u32, width: u32, alpha: u32) {
    if width == 0 || alpha == 0 || w < 2 || h < 2 { return; }
    let r = r.min(w / 2).min(h / 2);
    let x0 = x.saturating_sub(width);
    let y0 = y.saturating_sub(width);
    let x1 = (x + w + width).min(info.width);
    let y1 = (y + h + width).min(info.height);
    if x1 <= x0 || y1 <= y0 { return; }

    let paint = |px: u32, py: u32| {
        if rect_coverage_sdf(px, py, x, y, w, h, r) > 0 { return; }
        // Distance to the tile in whole pixels = the first grown ring that
        // reaches this pixel. Nearest ring is brightest.
        for i in 1..=width {
            let gx = x.saturating_sub(i);
            let gy = y.saturating_sub(i);
            let gw = w + i + (x - gx);
            let gh = h + i + (y - gy);
            let cov = rect_coverage_sdf(px, py, gx, gy, gw, gh, r + i);
            if cov == 0 { continue; }
            let falloff = (width + 1 - i) * 256 / width;
            let a = (alpha * falloff / 256) * cov / 256;
            if a > 0 {
                let bg = read_pixel(shadow, info, px, py);
                put_pixel(shadow, info, px, py, blend(color, bg, a.min(256)));
            }
            return;
        }
    };

    // Only the band is walked: rows that can hold a rounded corner get the
    // full width, the straight middle just the two side strips.
    let corner_lo = (y + r).min(y1);
    let corner_hi = (y + h).saturating_sub(r).max(corner_lo);
    for py in y0..y1 {
        if py < corner_lo || py >= corner_hi {
            for px in x0..x1 { paint(px, py); }
        } else {
            for px in x0..x.min(x1) { paint(px, py); }
            for px in (x + w).clamp(x0, x1)..x1 { paint(px, py); }
        }
    }
}

// ── Public rounded-rect helpers ────────────────────────────────────────

/// Fill a rounded rectangle (filled body + AA quarter-circle corners).
#[allow(dead_code)]
pub fn fill_rounded_rect(shadow: *mut u8, info: &FbInfo,
                         x: u32, y: u32, w: u32, h: u32,
                         color: u32, radius: u32) {
    fill_rounded_rect_aa(shadow, info, x, y, w, h, color, radius);
}

/// Fill a rounded rectangle with anti-aliased corners (SDF).
/// Body + side strips drawn with `fill_rect`; only the four corner
/// squares (size r×r) iterate the SDF coverage helper.
pub fn fill_rounded_rect_aa(shadow: *mut u8, info: &FbInfo,
                            x: u32, y: u32, w: u32, h: u32,
                            color: u32, radius: u32) {
    if radius == 0 || w < 2 || h < 2 {
        fill_rect(shadow, info, x, y, w, h, color);
        return;
    }
    let r = radius.min(w / 2).min(h / 2);

    fill_rect(shadow, info, x + r, y, w - 2 * r, h, color);
    fill_rect(shadow, info, x, y + r, r, h - 2 * r, color);
    fill_rect(shadow, info, x + w - r, y + r, r, h - 2 * r, color);

    let r_q8 = r as i32 * 256;
    let corners: [(u32, u32, bool, bool); 4] = [
        (x + r,     y + r,     true,  true),
        (x + w - r, y + r,     false, true),
        (x + r,     y + h - r, true,  false),
        (x + w - r, y + h - r, false, false),
    ];
    for &(cx, cy, flip_x, flip_y) in &corners {
        for dy in 0..r {
            let cdy_q8 = dy as i32 * 256 + 128;
            for dx in 0..r {
                let cdx_q8 = dx as i32 * 256 + 128;
                let coverage = arc_coverage_sdf(cdx_q8, cdy_q8, r_q8);
                if coverage == 0 { continue; }

                let px = if flip_x { cx - 1 - dx } else { cx + dx };
                let py = if flip_y { cy - 1 - dy } else { cy + dy };

                if coverage == 256 {
                    put_pixel(shadow, info, px, py, color);
                } else {
                    let bg = read_pixel(shadow, info, px, py);
                    put_pixel(shadow, info, px, py, blend(color, bg, coverage));
                }
            }
        }
    }
}

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
static GLASS_TINT: spin::Mutex<Option<(u64, u32, alloc::vec::Vec<u32>)>> =
    spin::Mutex::new(None);

fn ensure_glass_tint(bg_color: u32, opacity: u32, info: &FbInfo) -> Option<(*const u32, usize)> {
    let wp = crate::gui::background::wallpaper_ptr();
    if wp.is_null() { return None; }
    let pitch_px = info.pitch as usize / 4;
    let (width, height) = (info.width as usize, info.height as usize);
    let mut k = 0xcbf29ce484222325u64;
    for v in [bg_color as u64, opacity as u64, wp as usize as u64,
              info.width as u64, info.height as u64] {
        k ^= v;
        k = k.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut g = GLASS_TINT.lock();
    if g.as_ref().map(|(key, _, _)| *key != k).unwrap_or(true) {
        let mut buf = alloc::vec![0u32; pitch_px * height];
        for py in 0..height {
            // SAFETY: wp is screen-sized (pitch*height); py<height, px<width.
            let wprow = unsafe { wp.add(py * pitch_px * 4) as *const u32 };
            let base = py * pitch_px;
            for px in 0..width {
                let wpx = unsafe { *wprow.add(px) };
                buf[base + px] = blend(bg_color, wpx, opacity);
            }
        }
        *g = Some((k, pitch_px as u32, buf));
    }
    let (_, pp, buf) = g.as_ref().unwrap();
    // SAFETY: render is Core-0-only and the buffer is reallocated only on a
    // key change (can't recur within this frame), so the pointer stays valid
    // for the duration of this fill_rounded_chrome_aa call.
    Some((buf.as_ptr(), *pp as usize))
}

pub fn fill_rounded_chrome_aa(
    shadow: *mut u8, info: &FbInfo,
    x: u32, y: u32, w: u32, h: u32,
    border_a: u32, border_b: u32, bg_color: u32,
    rounding: u32, border: u32,
    border_opacity: u32, bg_opacity: u32,
    paint_content: bool,
) {
    if w < 2 || h < 2 { return; }
    let r_out = rounding.min(w / 2).min(h / 2);
    let border_px = border.min(r_out).min(w / 2).min(h / 2);
    let inner_x = x + border_px;
    let inner_y = y + border_px;
    let inner_w = w - 2 * border_px;
    let inner_h = h - 2 * border_px;
    let r_in = r_out.saturating_sub(border_px).min(inner_w / 2).min(inner_h / 2);
    let x_max = (x + w).min(info.width);
    let y_max = (y + h).min(info.height);
    let diag_max = (w + h).max(1) as u64;
    let solid = border_a == border_b;
    let bo = border_opacity.min(255);
    let go = bg_opacity.min(255);

    // The inner-full area (outer==inner==256) is the bulk of a window's
    // pixels; at 4K that's millions, and computing rect_coverage_sdf TWICE per
    // pixel for all of them was comp.render's entire cost (~96ms for a widget,
    // ~40ms even for one terminal). On the vertically-straight rows that span
    // is the known rectangle [skip_lo, skip_hi) — short-circuit the SDF there
    // (outer=inner=256): widget mode `continue`s it (the widget blit paints
    // it); an opaque content fill becomes a plain store; a translucent one
    // still blends over the wallpaper but without the SDF. The border ring +
    // the four rounded corners keep the full per-pixel SDF. Visually identical.
    let straight_lo = inner_y + r_in;
    let straight_hi = (inner_y + inner_h).saturating_sub(r_in);
    let skip_lo = (inner_x + 2).min(x_max);
    let skip_hi = (inner_x + inner_w).saturating_sub(2).max(skip_lo);
    let opaque_fill = paint_content && go >= 255;
    // Translucent terminal interior → memcpy a row from the precomputed glass
    // tint (any size, ~2ms) instead of per-pixel blend. Border ring + corners
    // keep the per-pixel SDF path below.
    let tint = if paint_content && !opaque_fill {
        ensure_glass_tint(bg_color, go, info)
    } else {
        None
    };
    let pitch = info.pitch as usize;

    // Per-pixel SDF path — used for the border ring + the four rounded corners
    // (everything that is NOT the known-interior straight span).
    let mut paint_px = |px: u32, py: u32| {
        let outer = rect_coverage_sdf(px, py, x, y, w, h, r_out);
        if outer == 0 { return; }
        let border_color = if solid {
            border_a
        } else {
            let t = (((px - x) as u64 + (py - y) as u64) * 1000 / diag_max) as u32;
            crate::theme::lerp_color(border_a, border_b, t.min(1000))
        };
        let bg_pixel = read_pixel(shadow, info, px, py);
        if outer < 256 {
            let alpha = (outer * bo / 256).min(255);
            put_pixel(shadow, info, px, py, blend(border_color, bg_pixel, alpha));
            return;
        }
        let inner = if border_px == 0 {
            256
        } else {
            rect_coverage_sdf(px, py, inner_x, inner_y, inner_w, inner_h, r_in)
        };
        if !paint_content {
            if inner == 256 { return; }
            put_pixel(shadow, info, px, py, blend(border_color, bg_pixel, bo));
            return;
        }
        if inner == 0 {
            // Border ring (outside the inner rect): solid border over wallpaper.
            put_pixel(shadow, info, px, py, blend(border_color, bg_pixel, bo));
        } else if inner == 256 {
            // Deep interior — MUST match the straight-row glass tint exactly
            // (glass over wallpaper, NO border tint), else the corner bands
            // show a border-coloured bar where the per-pixel path used to add
            // the tint but the straight middle no longer does.
            put_pixel(shadow, info, px, py, blend(bg_color, bg_pixel, go));
        } else {
            // Inner fringe: AA transition from border to glass over ~1 px.
            let after_border = blend(border_color, bg_pixel, bo);
            let bg_alpha = (go * inner / 256).min(255);
            put_pixel(shadow, info, px, py, blend(bg_color, after_border, bg_alpha));
        }
    };

    for py in y..y_max {
        let straight = skip_hi > skip_lo && py >= straight_lo && py < straight_hi;
        if !straight {
            for px in x..x_max { paint_px(px, py); }
            continue;
        }
        // Straight row: per-pixel border on the left, fast interior, border on
        // the right.
        for px in x..skip_lo.min(x_max) { paint_px(px, py); }
        let lo = skip_lo.min(x_max);
        let hi = skip_hi.min(x_max);
        if paint_content && hi > lo {
            let span = (hi - lo) as usize;
            if opaque_fill {
                // SAFETY: row within fb; [lo,hi) within width.
                unsafe {
                    let dst = shadow.add(py as usize * pitch + lo as usize * 4) as *mut u32;
                    for i in 0..span { *dst.add(i) = bg_color; }
                }
            } else if let Some((tptr, tpitch)) = tint {
                // SAFETY: tint is screen-sized (tpitch*height); py<height,
                // hi<=width; Core-0-only so tptr is valid this frame.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        tptr.add(py as usize * tpitch + lo as usize),
                        shadow.add(py as usize * pitch + lo as usize * 4) as *mut u32,
                        span,
                    );
                }
            } else {
                for px in lo..hi {
                    let bc = if solid {
                        border_a
                    } else {
                        let t = (((px - x) as u64 + (py - y) as u64) * 1000 / diag_max) as u32;
                        crate::theme::lerp_color(border_a, border_b, t.min(1000))
                    };
                    let after_border = blend(bc, read_pixel(shadow, info, px, py), bo);
                    put_pixel(shadow, info, px, py, blend(bg_color, after_border, go));
                }
            }
        }
        for px in skip_hi.max(x)..x_max { paint_px(px, py); }
    }
}

/// Blend `src` over the existing shadow-buffer pixel at (x, y) with
/// `alpha` ∈ 0..=256. Read-modify-write helper for paths that already
/// know the coverage analytically (e.g. SDF-aware widget blits).
pub fn blend_pixel(shadow: *mut u8, info: &FbInfo, x: u32, y: u32, src: u32, alpha: u32) {
    if alpha == 0 { return; }
    if alpha >= 256 {
        put_pixel(shadow, info, x, y, src);
        return;
    }
    let dst = read_pixel(shadow, info, x, y);
    put_pixel(shadow, info, x, y, blend(src, dst, alpha));
}

// ── Layer-aware rendering (writes alpha channel for compositing) ───────

/// Fill a rounded rectangle with color + alpha byte for layer compositing.
/// Unlike fill_rounded_rect_blend, this does NOT read existing pixels —
/// it writes color with the alpha byte set in the high byte.
/// The layer compositor handles blending with lower layers.
pub fn fill_rounded_rect_alpha(buf: *mut u8, info: &FbInfo,
                               x: u32, y: u32, w: u32, h: u32,
                               color: u32, radius: u32, alpha: u32) {
    if w < 2 || h < 2 { return; }
    let r = radius.min(w / 2).min(h / 2);
    let r_f = r as i32;
    let base = (alpha.min(255) << 24) | (color & 0x00FFFFFF);

    for py in y..(y + h).min(info.height) {
        for px in x..(x + w).min(info.width) {
            let in_x = px.saturating_sub(x);
            let in_y = py.saturating_sub(y);

            let (corner_dx, corner_dy) = {
                let dx = if in_x < r { r - in_x } else if in_x >= w - r { in_x - (w - r) + 1 } else { 0 };
                let dy = if in_y < r { r - in_y } else if in_y >= h - r { in_y - (h - r) + 1 } else { 0 };
                (dx as i32, dy as i32)
            };

            if corner_dx > 0 && corner_dy > 0 {
                let mut coverage = 0u32;
                for sy in 0..16i32 {
                    for sx in 0..16i32 {
                        let sdx = corner_dx * 32 + 2 * sx - 15;
                        let sdy = corner_dy * 32 + 2 * sy - 15;
                        if sdx * sdx + sdy * sdy <= r_f * r_f * 1024 {
                            coverage += 1;
                        }
                    }
                }
                if coverage == 0 { continue; }
                let a = (alpha * coverage / 256).min(255);
                put_pixel(buf, info, px, py, (a << 24) | (color & 0x00FFFFFF));
            } else {
                put_pixel(buf, info, px, py, base);
            }
        }
    }
}

/// Fill a rounded rectangle with a gradient + alpha byte for layer compositing.
pub fn fill_rounded_rect_gradient_alpha(buf: *mut u8, info: &FbInfo,
                                        x: u32, y: u32, w: u32, h: u32,
                                        color_a: u32, color_b: u32,
                                        radius: u32, alpha: u32) {
    if w < 2 || h < 2 { return; }
    let r = radius.min(w / 2).min(h / 2);
    let r_f = r as i32;
    let diag_max = (w + h) as u64;

    for py in y..(y + h).min(info.height) {
        for px in x..(x + w).min(info.width) {
            let in_x = px.saturating_sub(x);
            let in_y = py.saturating_sub(y);

            let (corner_dx, corner_dy) = {
                let dx = if in_x < r { r - in_x } else if in_x >= w - r { in_x - (w - r) + 1 } else { 0 };
                let dy = if in_y < r { r - in_y } else if in_y >= h - r { in_y - (h - r) + 1 } else { 0 };
                (dx as i32, dy as i32)
            };

            let t = ((in_x as u64 + in_y as u64) * 1000 / diag_max.max(1)) as u32;
            let color = crate::theme::lerp_color(color_a, color_b, t.min(1000));

            if corner_dx > 0 && corner_dy > 0 {
                let mut coverage = 0u32;
                for sy in 0..16i32 {
                    for sx in 0..16i32 {
                        let sdx = corner_dx * 32 + 2 * sx - 15;
                        let sdy = corner_dy * 32 + 2 * sy - 15;
                        if sdx * sdx + sdy * sdy <= r_f * r_f * 1024 {
                            coverage += 1;
                        }
                    }
                }
                if coverage == 0 { continue; }
                let a = (alpha * coverage / 256).min(255);
                put_pixel(buf, info, px, py, (a << 24) | (color & 0x00FFFFFF));
            } else {
                put_pixel(buf, info, px, py, (alpha.min(255) << 24) | (color & 0x00FFFFFF));
            }
        }
    }
}
