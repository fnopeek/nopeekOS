//! Procedural background generator with multiple color schemes.
//!
//! Supports two modes:
//! 1. Procedural aurora (7 themes, zero embedded images)
//! 2. Custom wallpaper (set via WASM module, pixels in wallpaper buffer)

use core::sync::atomic::{AtomicU8, AtomicBool, Ordering};
use crate::framebuffer::FbInfo;
use super::render;

/// Cached aurora background — rendered once, then memcpy for regions.
static mut AURORA_CACHE: *mut u8 = core::ptr::null_mut();
static mut AURORA_CACHE_PITCH: u32 = 0;
static mut AURORA_CACHE_W: u32 = 0;
static mut AURORA_CACHE_H: u32 = 0;
static AURORA_CACHED: AtomicBool = AtomicBool::new(false);

/// Custom wallpaper buffer (set via WASM, overrides aurora).
static mut WALLPAPER: *mut u8 = core::ptr::null_mut();
static mut WALLPAPER_W: u32 = 0;
static mut WALLPAPER_H: u32 = 0;
static WALLPAPER_SET: AtomicBool = AtomicBool::new(false);

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
pub static SCHEMES: [ColorScheme; 8] = [
    // 0: Deep Blue (black with blue tones)
    ColorScheme {
        base_r: 2, base_g: 3, base_b: 12,
        streak_r: 15, streak_g: 40, streak_b: 120,
        glow_r: 40, glow_g: 80, glow_b: 180,
        glow2_r: 20, glow2_g: 50, glow2_b: 130,
        accent: 0x002060B0,
        border_focus: 0x002060B0,
    },
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

/// Select a color scheme at boot. Default: Deep Blue (0).
pub fn init() {
    // Use Deep Blue as default; set to random with:
    // let idx = (crate::csprng::random_u64() % SCHEMES.len() as u64) as u8;
    ACTIVE_SCHEME.store(0, Ordering::Release);
}

/// Get the currently active scheme.
pub fn active_scheme() -> &'static ColorScheme {
    let idx = ACTIVE_SCHEME.load(Ordering::Acquire) as usize;
    &SCHEMES[idx % SCHEMES.len()]
}

/// Get the accent color — from theme if active, otherwise from aurora scheme.
pub fn accent_color() -> u32 {
    if crate::theme::is_active() {
        crate::theme::accent()
    } else {
        active_scheme().accent
    }
}

/// Set a custom wallpaper from raw BGRA pixel data.
/// The data is copied into a kernel-owned buffer and scaled to fit the framebuffer.
pub fn set_wallpaper(pixels: &[u8], w: u32, h: u32, info: &FbInfo) {
    let target_w = info.width;
    let target_h = info.height;
    let size = target_h as usize * info.pitch as usize;
    let pages = (size + 4095) / 4096;

    // Allocate (or reuse) wallpaper buffer
    let buf = if !unsafe { WALLPAPER.is_null() } && unsafe { WALLPAPER_W == target_w && WALLPAPER_H == target_h } {
        unsafe { WALLPAPER }
    } else {
        match crate::memory::allocate_contiguous(pages) {
            Some(addr) => addr as *mut u8,
            None => return,
        }
    };

    // Bilinear-ish scale: nearest-neighbor for speed (good enough for wallpapers)
    for ty in 0..target_h {
        let sy = (ty as u64 * h as u64 / target_h as u64) as u32;
        for tx in 0..target_w {
            let sx = (tx as u64 * w as u64 / target_w as u64) as u32;
            let src_off = (sy * w + sx) as usize * 4;
            if src_off + 3 >= pixels.len() { continue; }
            let b = pixels[src_off] as u32;
            let g = pixels[src_off + 1] as u32;
            let r = pixels[src_off + 2] as u32;
            let pixel = (r << 16) | (g << 8) | b;
            let dst_off = (ty * info.pitch + tx * 4) as usize;
            // SAFETY: bounds checked by allocation size
            unsafe { *(buf.add(dst_off) as *mut u32) = pixel; }
        }
    }

    // SAFETY: single-core
    unsafe {
        WALLPAPER = buf;
        WALLPAPER_W = target_w;
        WALLPAPER_H = target_h;
    }
    WALLPAPER_SET.store(true, Ordering::Release);

    // Extract theme from wallpaper pixels
    let pixel_count = (w * h) as usize;
    let palette = crate::theme::extract_palette(pixels, pixel_count);
    crate::theme::set_palette(&palette);
}

/// Check if a custom wallpaper is active.
pub fn has_wallpaper() -> bool {
    WALLPAPER_SET.load(Ordering::Acquire)
}

/// Clear the custom wallpaper, revert to aurora.
pub fn clear_wallpaper() {
    WALLPAPER_SET.store(false, Ordering::Release);
    crate::theme::clear();
}

/// Draw background (wallpaper if set, otherwise aurora) — full screen.
pub fn draw_background(shadow: *mut u8, info: &FbInfo) {
    if has_wallpaper() {
        draw_wallpaper(shadow, info);
    } else {
        draw_aurora(shadow, info);
    }
}

/// Draw background region (wallpaper if set, otherwise aurora).
pub fn draw_background_region(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
    if has_wallpaper() {
        draw_wallpaper_region(shadow, info, rx, ry, rw, rh);
    } else {
        draw_aurora_region(shadow, info, rx, ry, rw, rh);
    }
}

/// Copy wallpaper buffer to shadow buffer (full screen).
fn draw_wallpaper(shadow: *mut u8, info: &FbInfo) {
    let wp = unsafe { WALLPAPER };
    if wp.is_null() { return; }
    let size = info.height as usize * info.pitch as usize;
    // SAFETY: wallpaper buffer is same size as shadow
    unsafe { core::ptr::copy_nonoverlapping(wp, shadow, size); }
}

/// Copy wallpaper region to shadow buffer.
fn draw_wallpaper_region(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
    let wp = unsafe { WALLPAPER };
    if wp.is_null() {
        draw_aurora_region(shadow, info, rx, ry, rw, rh);
        return;
    }
    let pitch = info.pitch as usize;
    let x0 = rx as usize;
    let x1 = ((rx + rw) as usize).min(info.width as usize);
    let bytes = (x1 - x0) * 4;
    for y in ry..(ry + rh).min(info.height) {
        let off = y as usize * pitch + x0 * 4;
        // SAFETY: copying from wallpaper to shadow within bounds
        unsafe { core::ptr::copy_nonoverlapping(wp.add(off), shadow.add(off), bytes); }
    }
}

/// Generate the full aurora into the cache, then copy to shadow buffer.
pub fn draw_aurora(shadow: *mut u8, info: &FbInfo) {
    ensure_cache(info);
    // Copy full cache → shadow
    let cache = unsafe { AURORA_CACHE };
    if !cache.is_null() {
        let size = info.height as usize * info.pitch as usize;
        // SAFETY: copying cached aurora to shadow buffer
        unsafe { core::ptr::copy_nonoverlapping(cache, shadow, size); }
    }
}

/// Redraw a region of the aurora background (fast memcpy from cache).
pub fn draw_aurora_region(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
    ensure_cache(info);
    let cache = unsafe { AURORA_CACHE };
    if !cache.is_null() {
        // Copy region from cache → shadow (row by row)
        let pitch = info.pitch as usize;
        let x0 = rx as usize;
        let x1 = ((rx + rw) as usize).min(info.width as usize);
        let bytes = (x1 - x0) * 4;
        for y in ry..(ry + rh).min(info.height) {
            let off = y as usize * pitch + x0 * 4;
            // SAFETY: copying from cache to shadow within bounds
            unsafe { core::ptr::copy_nonoverlapping(cache.add(off), shadow.add(off), bytes); }
        }
        return;
    }
    // Fallback: render directly (no cache available)
    draw_aurora_direct(shadow, info, rx, ry, rw, rh);
}

/// Ensure the aurora cache exists and is populated.
fn ensure_cache(info: &FbInfo) {
    if AURORA_CACHED.load(Ordering::Acquire) {
        let (cw, ch) = unsafe { (AURORA_CACHE_W, AURORA_CACHE_H) };
        if cw == info.width && ch == info.height { return; } // Cache valid
    }
    // Allocate cache buffer
    let size = info.height as usize * info.pitch as usize;
    let pages = (size + 4095) / 4096;
    let cache = match crate::memory::allocate_contiguous(pages) {
        Some(addr) => addr as *mut u8,
        None => return, // No memory — fall back to direct rendering
    };
    // Render aurora into cache
    draw_aurora_direct(cache, info, 0, 0, info.width, info.height);
    // SAFETY: single-core, no concurrent access
    unsafe {
        AURORA_CACHE = cache;
        AURORA_CACHE_PITCH = info.pitch;
        AURORA_CACHE_W = info.width;
        AURORA_CACHE_H = info.height;
    }
    AURORA_CACHED.store(true, Ordering::Release);
}

/// Invalidate the aurora cache (call when scheme changes).
pub fn invalidate_cache() {
    AURORA_CACHED.store(false, Ordering::Release);
}

/// Render aurora directly to a buffer (the expensive computation).
fn draw_aurora_direct(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
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

            // Compose with scheme colors (>> 8 instead of / 255 = ~10x faster)
            let r = (s.base_r
                + ((streak * s.streak_r) >> 8)
                + ((glow * s.glow_r) >> 8)
                + ((glow2 * s.glow2_r) >> 8)
                - ((vignette * 15) >> 8)).clamp(0, 255) as u32;
            let g = (s.base_g
                + ((streak * s.streak_g) >> 8)
                + ((glow * s.glow_g) >> 8)
                + ((glow2 * s.glow2_g) >> 8)
                - ((vignette * 10) >> 8)).clamp(0, 255) as u32;
            let b = (s.base_b
                + ((streak * s.streak_b) >> 8)
                + ((glow * s.glow_b) >> 8)
                + ((glow2 * s.glow2_b) >> 8)
                - ((vignette * 20) >> 8)).clamp(0, 255) as u32;

            let color = (r << 16) | (g << 8) | b;

            if let Some(ptr) = row_ptr {
                unsafe { *ptr.add(x as usize) = color; }
            } else {
                render::put_pixel(shadow, info, x, y, color);
            }
        }
    }
}

/// Pre-computed sine lookup table (256 entries, compile-time generated).
/// Maps phase 0..255 to amplitude 0..255 (one full period).
/// Eliminates runtime multiply/divide — single table lookup per call.
static SINE_LUT: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let p = (i * 1000 / 256) as i32;
        let half = if p < 500 { p } else { 1000 - p };
        let t = half * 2;
        let val = 4 * t * (1000 - t) / 1000;
        let scaled = val * 255 / 1000;
        table[i] = if scaled > 255 { 255 } else if scaled < 0 { 0 } else { scaled as u8 };
        i += 1;
    }
    table
};

/// Fast sine approximation via lookup table.
/// Input: any i32 phase. Output: 0..255.
/// ~5x faster than runtime polynomial (1 modulo + 1 load vs 2 mod + 3 mul + 2 div).
#[inline(always)]
fn sine_approx(phase: i32) -> i32 {
    // Map any phase to 0..255 index (256 = one full period ≈ old 1000)
    let idx = (((phase * 256 / 1000) % 256) + 256) as usize % 256;
    SINE_LUT[idx] as i32
}
