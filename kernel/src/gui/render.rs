//! Rendering primitives and damage tracking for the GUI layer.
//!
//! All drawing targets the shadow buffer. DamageTracker records dirty regions
//! and flushes them to MMIO via blit_rect.

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
    // Top
    fill_rect(shadow, info, x, y, w, thickness, color);
    // Bottom
    fill_rect(shadow, info, x, y + h - thickness, w, thickness, color);
    // Left
    fill_rect(shadow, info, x, y + thickness, thickness, h - 2 * thickness, color);
    // Right
    fill_rect(shadow, info, x + w - thickness, y + thickness, thickness, h - 2 * thickness, color);
}

/// Fill a rounded rectangle (filled body + quarter-circle corners).
#[allow(dead_code)]
pub fn fill_rounded_rect(shadow: *mut u8, info: &FbInfo,
                         x: u32, y: u32, w: u32, h: u32,
                         color: u32, radius: u32) {
    fill_rounded_rect_aa(shadow, info, x, y, w, h, color, radius);
}

/// Fill a rounded rectangle with anti-aliased corners.
/// Blends edge pixels with the background for smooth appearance.
pub fn fill_rounded_rect_aa(shadow: *mut u8, info: &FbInfo,
                            x: u32, y: u32, w: u32, h: u32,
                            color: u32, radius: u32) {
    if radius == 0 || w < 2 || h < 2 {
        fill_rect(shadow, info, x, y, w, h, color);
        return;
    }
    let r = radius.min(w / 2).min(h / 2);

    // Center body (no corners needed)
    fill_rect(shadow, info, x + r, y, w - 2 * r, h, color);
    // Left strip (between corners)
    fill_rect(shadow, info, x, y + r, r, h - 2 * r, color);
    // Right strip
    fill_rect(shadow, info, x + w - r, y + r, r, h - 2 * r, color);

    // 16x16 centered subpixel sampling — 256 coverage levels, scale
    // ×32 so sample offsets (2*sx - 15) land symmetric around 0.
    let r_f = r as i32;
    let corners: [(u32, u32, bool, bool); 4] = [
        (x + r,     y + r,     true,  true),
        (x + w - r, y + r,     false, true),
        (x + r,     y + h - r, true,  false),
        (x + w - r, y + h - r, false, false),
    ];
    for &(cx, cy, flip_x, flip_y) in &corners {
        for dy in 0..r {
            for dx in 0..r {
                let mut coverage = 0u32;
                let base_dx = (dx as i32 + 1) * 32;
                let base_dy = (dy as i32 + 1) * 32;
                for sy in 0..16i32 {
                    for sx in 0..16i32 {
                        let sdx = base_dx - 2 * sx - 15;
                        let sdy = base_dy - 2 * sy - 15;
                        if sdx * sdx + sdy * sdy <= r_f * r_f * 1024 {
                            coverage += 1;
                        }
                    }
                }
                if coverage == 0 { continue; }

                let px = if flip_x { cx - 1 - dx } else { cx + dx };
                let py = if flip_y { cy - 1 - dy } else { cy + dy };

                if coverage == 256 {
                    put_pixel(shadow, info, px, py, color);
                } else {
                    let bg = read_pixel(shadow, info, px, py);
                    let blended = blend(color, bg, coverage);
                    put_pixel(shadow, info, px, py, blended);
                }
            }
        }
    }
}

/// Fill a rounded rectangle blended over the existing background.
/// opacity: 0 = fully transparent, 256 = fully opaque.
pub fn fill_rounded_rect_blend(shadow: *mut u8, info: &FbInfo,
                               x: u32, y: u32, w: u32, h: u32,
                               color: u32, radius: u32, opacity: u32) {
    if w < 2 || h < 2 { return; }
    let r = radius.min(w / 2).min(h / 2);
    let r_f = r as i32;

    // For each pixel in the bounding box, determine if inside rounded rect
    for py in y..(y + h).min(info.height) {
        for px in x..(x + w).min(info.width) {
            // Check if pixel is inside the rounded rect
            let in_x = px.saturating_sub(x);
            let in_y = py.saturating_sub(y);

            // Determine corner distance
            let (corner_dx, corner_dy) = {
                let dx = if in_x < r { r - in_x } else if in_x >= w - r { in_x - (w - r) + 1 } else { 0 };
                let dy = if in_y < r { r - in_y } else if in_y >= h - r { in_y - (h - r) + 1 } else { 0 };
                (dx as i32, dy as i32)
            };

            if corner_dx > 0 && corner_dy > 0 {
                // 8x8 subpixel AA — 64 coverage levels, smooth on bright bg.
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
                let alpha = opacity * coverage / 256;
                let bg = read_pixel(shadow, info, px, py);
                put_pixel(shadow, info, px, py, blend(color, bg, alpha));
            } else {
                let bg = read_pixel(shadow, info, px, py);
                put_pixel(shadow, info, px, py, blend(color, bg, opacity));
            }
        }
    }
}

/// Fill a rounded rectangle with a gradient border, blended over the background.
/// The gradient goes from `color_a` to `color_b` at `angle_deg` degrees.
/// Only the border ring (outer rect minus inner content rect) gets the gradient.
pub fn fill_rounded_rect_gradient(shadow: *mut u8, info: &FbInfo,
                                  x: u32, y: u32, w: u32, h: u32,
                                  color_a: u32, color_b: u32,
                                  radius: u32, opacity: u32) {
    if w < 2 || h < 2 { return; }
    let r = radius.min(w / 2).min(h / 2);
    let r_f = r as i32;

    // Precompute gradient direction (45° diagonal: top-left → bottom-right)
    // t = 0 at top-left, t = 1000 at bottom-right
    let diag_max = (w + h) as u64;

    for py in y..(y + h).min(info.height) {
        for px in x..(x + w).min(info.width) {
            let in_x = px.saturating_sub(x);
            let in_y = py.saturating_sub(y);

            // Corner rounding check
            let (corner_dx, corner_dy) = {
                let dx = if in_x < r { r - in_x } else if in_x >= w - r { in_x - (w - r) + 1 } else { 0 };
                let dy = if in_y < r { r - in_y } else if in_y >= h - r { in_y - (h - r) + 1 } else { 0 };
                (dx as i32, dy as i32)
            };

            // Gradient: interpolate along 45° diagonal
            let t = ((in_x as u64 + in_y as u64) * 1000 / diag_max) as u32;
            let color = crate::theme::lerp_color(color_a, color_b, t);

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
                let alpha = opacity * coverage / 256;
                let bg = read_pixel(shadow, info, px, py);
                put_pixel(shadow, info, px, py, blend(color, bg, alpha));
            } else {
                let bg = read_pixel(shadow, info, px, py);
                put_pixel(shadow, info, px, py, blend(color, bg, opacity));
            }
        }
    }
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

            // Gradient interpolation
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


