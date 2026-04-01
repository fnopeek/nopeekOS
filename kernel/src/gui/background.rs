//! Procedural background generator with multiple color schemes.
//!
//! 7 aurora themes, randomly selected at boot.
//! Pure math — no embedded images, zero extra bytes in the kernel.

use core::sync::atomic::{AtomicU8, Ordering};
use crate::framebuffer::FbInfo;
use super::render;

/// Currently active color scheme (set once at boot).
static ACTIVE_SCHEME: AtomicU8 = AtomicU8::new(0);

/// Color scheme definition: base color, streak tint, glow tint, accent for UI.
#[derive(Clone, Copy)]
pub struct ColorScheme {
    // Base (darkest background)
    pub base_r: i32, pub base_g: i32, pub base_b: i32,
    // Streak color multipliers (0..255)
    pub streak_r: i32, pub streak_g: i32, pub streak_b: i32,
    // Primary glow color
    pub glow_r: i32, pub glow_g: i32, pub glow_b: i32,
    // Secondary glow color
    pub glow2_r: i32, pub glow2_g: i32, pub glow2_b: i32,
    // UI accent color (for [npk] tag, borders)
    pub accent: u32,
    // Input field border (focused)
    pub border_focus: u32,
}

/// All available color schemes.
pub static SCHEMES: [ColorScheme; 7] = [
    // 0: Violet Aurora (original)
    ColorScheme {
        base_r: 10, base_g: 5, base_b: 20,
        streak_r: 120, streak_g: 50, streak_b: 160,
        glow_r: 200, glow_g: 160, glow_b: 220,
        glow2_r: 140, glow2_g: 90, glow2_b: 180,
        accent: 0x007B50A0,
        border_focus: 0x007B50A0,
    },
    // 1: Ocean Blue
    ColorScheme {
        base_r: 5, base_g: 10, base_b: 25,
        streak_r: 30, streak_g: 100, streak_b: 180,
        glow_r: 100, glow_g: 200, glow_b: 240,
        glow2_r: 60, glow2_g: 140, glow2_b: 200,
        accent: 0x003090D0,
        border_focus: 0x003090D0,
    },
    // 2: Emerald Green
    ColorScheme {
        base_r: 5, base_g: 15, base_b: 10,
        streak_r: 40, streak_g: 160, streak_b: 80,
        glow_r: 120, glow_g: 230, glow_b: 160,
        glow2_r: 60, glow2_g: 180, glow2_b: 100,
        accent: 0x0030A060,
        border_focus: 0x0030A060,
    },
    // 3: Sunset Amber
    ColorScheme {
        base_r: 20, base_g: 8, base_b: 5,
        streak_r: 180, streak_g: 80, streak_b: 20,
        glow_r: 240, glow_g: 180, glow_b: 80,
        glow2_r: 200, glow2_g: 100, glow2_b: 40,
        accent: 0x00D08030,
        border_focus: 0x00D08030,
    },
    // 4: Rose Pink
    ColorScheme {
        base_r: 18, base_g: 5, base_b: 12,
        streak_r: 160, streak_g: 40, streak_b: 100,
        glow_r: 230, glow_g: 140, glow_b: 180,
        glow2_r: 180, glow2_g: 80, glow2_b: 140,
        accent: 0x00C050A0,
        border_focus: 0x00C050A0,
    },
    // 5: Arctic Ice
    ColorScheme {
        base_r: 8, base_g: 12, base_b: 18,
        streak_r: 80, streak_g: 140, streak_b: 180,
        glow_r: 180, glow_g: 220, glow_b: 245,
        glow2_r: 120, glow2_g: 180, glow2_b: 220,
        accent: 0x0070B0D0,
        border_focus: 0x0070B0D0,
    },
    // 6: Nebula (multi-color, pattern variant)
    ColorScheme {
        base_r: 8, base_g: 5, base_b: 15,
        streak_r: 100, streak_g: 60, streak_b: 160,
        glow_r: 220, glow_g: 120, glow_b: 80,   // warm glow
        glow2_r: 60, glow2_g: 160, glow2_b: 220, // cool glow (contrast!)
        accent: 0x009060C0,
        border_focus: 0x009060C0,
    },
];

/// Select a random color scheme at boot.
pub fn init() {
    let idx = (crate::csprng::random_u64() % SCHEMES.len() as u64) as u8;
    ACTIVE_SCHEME.store(idx, Ordering::Release);
}

/// Get the currently active scheme.
pub fn active_scheme() -> &'static ColorScheme {
    let idx = ACTIVE_SCHEME.load(Ordering::Acquire) as usize;
    &SCHEMES[idx % SCHEMES.len()]
}

/// Get the accent color for the active scheme.
pub fn accent_color() -> u32 {
    active_scheme().accent
}

/// Generate the background into the shadow buffer.
pub fn draw_aurora(shadow: *mut u8, info: &FbInfo) {
    draw_aurora_region(shadow, info, 0, 0, info.width, info.height);
}

/// Redraw a region of the aurora background.
pub fn draw_aurora_region(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
    let w = info.width;
    let h = info.height;
    let s = active_scheme();
    let is_nebula = ACTIVE_SCHEME.load(Ordering::Relaxed) == 6;

    for y in ry..(ry + rh).min(h) {
        let row_ptr = if info.bpp == 32 {
            Some(unsafe { shadow.add((y * info.pitch) as usize) as *mut u32 })
        } else {
            None
        };

        for x in rx..(rx + rw).min(w) {
            let nx = x as i32 * 1000 / w as i32;
            let ny = y as i32 * 1000 / h as i32;

            // Diagonal streaks
            let diag = if is_nebula {
                // Nebula: swirling pattern (mix of diagonal + radial)
                let radial = ((nx - 500) * (nx - 500) + (ny - 500) * (ny - 500)) / 100;
                (nx * 400 + ny * 600 + radial) / 1000
            } else {
                (nx * 600 + ny * 800) / 1000
            };

            let s1 = sine_approx(diag * 3 + 200);
            let s2 = sine_approx(diag * 7 - 150);
            let s3 = sine_approx(diag * 13 + 500);
            let s4 = sine_approx(diag * 5 + nx / 2);
            let streak = (s1 * 40 + s2 * 25 + s3 * 10 + s4 * 20) / 100;

            // Primary glow (center-left)
            let gdx = nx - 350;
            let gdy = ny - 550;
            let glow = 255i32.saturating_sub((gdx * gdx + gdy * gdy) / 600).max(0);

            // Secondary glow (upper-right)
            let gdx2 = nx - 700;
            let gdy2 = ny - 300;
            let glow2 = 180i32.saturating_sub((gdx2 * gdx2 + gdy2 * gdy2) / 800).max(0);

            // Vignette
            let edge_dx = (nx - 500).abs();
            let edge_dy = (ny - 500).abs();
            let vignette = (edge_dx * edge_dx + edge_dy * edge_dy) / 2000;

            // Compose with scheme colors
            let r = (s.base_r
                + streak * s.streak_r / 255
                + glow * s.glow_r / 255
                + glow2 * s.glow2_r / 255
                - vignette * 15 / 255).clamp(0, 255) as u32;
            let g = (s.base_g
                + streak * s.streak_g / 255
                + glow * s.glow_g / 255
                + glow2 * s.glow2_g / 255
                - vignette * 10 / 255).clamp(0, 255) as u32;
            let b = (s.base_b
                + streak * s.streak_b / 255
                + glow * s.glow_b / 255
                + glow2 * s.glow2_b / 255
                - vignette * 20 / 255).clamp(0, 255) as u32;

            let color = (r << 16) | (g << 8) | b;

            if let Some(ptr) = row_ptr {
                unsafe { *ptr.add(x as usize) = color; }
            } else {
                render::put_pixel(shadow, info, x, y, color);
            }
        }
    }
}

/// Integer sine approximation. Input: 0..1000 maps to 0..2π.
/// Returns 0..255.
fn sine_approx(phase: i32) -> i32 {
    let p = ((phase % 1000) + 1000) % 1000;
    let half = if p < 500 { p } else { 1000 - p };
    let t = half * 2;
    let val = (4 * t * (1000 - t)) / 1000;
    (val * 255 / 1000).clamp(0, 255)
}
