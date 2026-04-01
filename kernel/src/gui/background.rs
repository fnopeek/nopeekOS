//! Procedural background generator.
//!
//! Generates a purple aurora/silk background similar to the reference image.
//! Pure math — no embedded images, zero extra bytes in the kernel.

use crate::framebuffer::FbInfo;
use super::render;

/// Generate the login background into the shadow buffer.
pub fn draw_aurora(shadow: *mut u8, info: &FbInfo) {
    let w = info.width;
    let h = info.height;

    for y in 0..h {
        // Row pointer for fast 32bpp writes
        let row_ptr = if info.bpp == 32 {
            Some(unsafe { shadow.add((y * info.pitch) as usize) as *mut u32 })
        } else {
            None
        };

        for x in 0..w {
            // Normalize to 0.0..1.0
            let nx = x as i32 * 1000 / w as i32; // 0..1000
            let ny = y as i32 * 1000 / h as i32;

            // Diagonal coordinate (defines streak direction — ~60° angle)
            let diag = (nx * 600 + ny * 800) / 1000;

            // Multiple sine-wave streaks at different frequencies
            let s1 = sine_approx(diag * 3 + 200);       // wide streaks
            let s2 = sine_approx(diag * 7 - 150);       // medium streaks
            let s3 = sine_approx(diag * 13 + 500);      // fine detail
            let s4 = sine_approx(diag * 5 + nx / 2);    // diagonal variation

            // Combine streaks (weighted)
            let streak = (s1 * 40 + s2 * 25 + s3 * 10 + s4 * 20) / 100;

            // Radial glow (center-left, slightly below middle)
            let glow_cx = 350i32; // 35% from left
            let glow_cy = 550i32; // 55% from top
            let gdx = nx - glow_cx;
            let gdy = ny - glow_cy;
            let dist_sq = gdx * gdx + gdy * gdy;
            // Smooth falloff: bright at center, fades with distance
            let glow = 255i32.saturating_sub(dist_sq / 600).max(0);

            // Second smaller glow (upper right)
            let gdx2 = nx - 700;
            let gdy2 = ny - 300;
            let dist2 = gdx2 * gdx2 + gdy2 * gdy2;
            let glow2 = 180i32.saturating_sub(dist2 / 800).max(0);

            // Base purple darkness (vignette towards edges)
            let edge_dx = (nx - 500).abs();
            let edge_dy = (ny - 500).abs();
            let vignette = (edge_dx * edge_dx + edge_dy * edge_dy) / 2000;

            // Compose final color
            // Base: very dark purple
            let base_r = 10i32;
            let base_g = 5;
            let base_b = 20;

            // Streak adds purple/violet
            let streak_r = streak * 120 / 255;
            let streak_g = streak * 50 / 255;
            let streak_b = streak * 160 / 255;

            // Glow adds pink-white
            let glow_r = glow * 200 / 255 + glow2 * 140 / 255;
            let glow_g = glow * 160 / 255 + glow2 * 90 / 255;
            let glow_b = glow * 220 / 255 + glow2 * 180 / 255;

            let r = (base_r + streak_r + glow_r - vignette * 15 / 255).clamp(0, 255) as u32;
            let g = (base_g + streak_g + glow_g - vignette * 10 / 255).clamp(0, 255) as u32;
            let b = (base_b + streak_b + glow_b - vignette * 20 / 255).clamp(0, 255) as u32;

            let color = (r << 16) | (g << 8) | b;

            if let Some(ptr) = row_ptr {
                // SAFETY: x < width, within shadow buffer
                unsafe { *ptr.add(x as usize) = color; }
            } else {
                render::put_pixel(shadow, info, x, y, color);
            }
        }
    }
}

/// Integer sine approximation. Input: 0..1000 maps to 0..2π.
/// Returns 0..255 (not -1..1, but 0..1 scaled to byte range).
fn sine_approx(phase: i32) -> i32 {
    // Normalize to 0..1000 range
    let p = ((phase % 1000) + 1000) % 1000;

    // Parabolic sine approximation: 4*x*(1-x) for half period
    let half = if p < 500 { p } else { 1000 - p };
    // half is 0..500, normalize to 0..1000
    let t = half * 2;
    // 4*t*(1-t) where t is 0..1000 → result 0..1000
    let val = (4 * t * (1000 - t)) / 1000;
    // Scale to 0..255
    (val * 255 / 1000).clamp(0, 255)
}
