//! Theme system — 16-color palette derived from wallpaper or aurora.
//!
//! Colors are extracted via Median-Cut quantization from raw pixel data.
//! The palette drives border gradients, shadebar, accent colors, etc.

use core::sync::atomic::{AtomicBool, Ordering};

/// 16-color theme palette (pywal-compatible ordering).
/// color0 = darkest (background), color1..7 = dominant, color8..15 = bright variants.
static mut PALETTE: [u32; 16] = [0; 16];

/// Gradient border colors: start and end color for 45° linear gradient.
static mut BORDER_GRADIENT: (u32, u32) = (0, 0);

/// Whether a custom theme is active (vs. aurora default).
static THEME_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set the full 16-color palette and derive border gradient.
pub fn set_palette(colors: &[u32; 16]) {
    // SAFETY: single-core
    unsafe {
        PALETTE = *colors;
        // Border gradient: color1 → color2 (like Hyprland pywal setup)
        BORDER_GRADIENT = (colors[1], colors[2]);
    }
    THEME_ACTIVE.store(true, Ordering::Release);
}

/// Get the full palette.
pub fn palette() -> [u32; 16] {
    // SAFETY: single-core
    unsafe { PALETTE }
}

/// Whether a custom theme is active.
pub fn is_active() -> bool {
    THEME_ACTIVE.load(Ordering::Acquire)
}

/// Get border gradient (start_color, end_color).
pub fn border_gradient() -> (u32, u32) {
    // SAFETY: single-core
    unsafe { BORDER_GRADIENT }
}

/// Get background color (color0 — darkest).
pub fn bg_color() -> u32 {
    unsafe { PALETTE[0] }
}

/// Get accent color (color1 — primary dominant).
pub fn accent() -> u32 {
    unsafe { PALETTE[1] }
}

/// Get inactive border color (color0 with some brightness).
pub fn inactive_border() -> u32 {
    unsafe { PALETTE[8] } // bright variant of bg
}

/// Clear the custom theme, revert to aurora defaults.
pub fn clear() {
    THEME_ACTIVE.store(false, Ordering::Release);
}

/// Interpolate between two colors. t = 0..1000 (0 = a, 1000 = b).
pub fn lerp_color(a: u32, b: u32, t: u32) -> u32 {
    let t = t.min(1000);
    let inv = 1000 - t;
    let r = (((a >> 16) & 0xFF) * inv + ((b >> 16) & 0xFF) * t) / 1000;
    let g = (((a >> 8) & 0xFF) * inv + ((b >> 8) & 0xFF) * t) / 1000;
    let bl = ((a & 0xFF) * inv + (b & 0xFF) * t) / 1000;
    (r << 16) | (g << 8) | bl
}

// --- Median-Cut Color Extraction ---

/// Extract a 16-color palette from raw BGRA pixel data.
/// Uses Median-Cut quantization (same algorithm as pywal).
pub fn extract_palette(pixels: &[u8], pixel_count: usize) -> [u32; 16] {
    // Sample pixels (skip transparent, near-black, near-white)
    let mut samples = alloc::vec::Vec::new();
    let step = (pixel_count / 8192).max(1); // Sample ~8k pixels max
    for i in (0..pixel_count).step_by(step) {
        let off = i * 4;
        if off + 3 >= pixels.len() { break; }
        let b = pixels[off] as u32;
        let g = pixels[off + 1] as u32;
        let r = pixels[off + 2] as u32;
        // Skip near-black and near-white (not useful for theming)
        let lum = (r * 299 + g * 587 + b * 114) / 1000;
        if lum < 15 || lum > 240 { continue; }
        samples.push((r, g, b));
    }

    if samples.is_empty() {
        return default_palette();
    }

    // Median-Cut: split into 16 buckets
    let mut buckets: alloc::vec::Vec<alloc::vec::Vec<(u32, u32, u32)>> = alloc::vec![samples];

    while buckets.len() < 16 {
        // Find bucket with largest color range
        let mut best_idx = 0;
        let mut best_range = 0u32;
        for (i, bucket) in buckets.iter().enumerate() {
            if bucket.len() < 2 { continue; }
            let range = channel_range(bucket);
            if range > best_range {
                best_range = range;
                best_idx = i;
            }
        }
        if best_range == 0 { break; }

        // Split along channel with largest range
        let bucket = buckets.remove(best_idx);
        let (a, b) = median_split(bucket);
        buckets.push(a);
        buckets.push(b);
    }

    // Average each bucket to get palette color
    let mut colors = [0u32; 16];
    for (i, bucket) in buckets.iter().enumerate() {
        if i >= 16 { break; }
        colors[i] = bucket_average(bucket);
    }

    // Sort by luminance (darkest first, like pywal)
    colors[..buckets.len().min(16)].sort_by_key(|c| luminance(*c));

    // Fill remaining slots with brightened variants
    let count = buckets.len().min(16);
    for i in count..16 {
        colors[i] = brighten(colors[i % count], 40);
    }

    colors
}

fn channel_range(pixels: &[(u32, u32, u32)]) -> u32 {
    let (mut rmin, mut rmax) = (255, 0);
    let (mut gmin, mut gmax) = (255, 0);
    let (mut bmin, mut bmax) = (255, 0);
    for &(r, g, b) in pixels {
        rmin = rmin.min(r); rmax = rmax.max(r);
        gmin = gmin.min(g); gmax = gmax.max(g);
        bmin = bmin.min(b); bmax = bmax.max(b);
    }
    (rmax - rmin).max(gmax - gmin).max(bmax - bmin)
}

fn median_split(mut pixels: alloc::vec::Vec<(u32, u32, u32)>) -> (alloc::vec::Vec<(u32, u32, u32)>, alloc::vec::Vec<(u32, u32, u32)>) {
    // Find which channel has the largest range
    let (mut rmin, mut rmax) = (255u32, 0u32);
    let (mut gmin, mut gmax) = (255u32, 0u32);
    let (mut bmin, mut bmax) = (255u32, 0u32);
    for &(r, g, b) in &pixels {
        rmin = rmin.min(r); rmax = rmax.max(r);
        gmin = gmin.min(g); gmax = gmax.max(g);
        bmin = bmin.min(b); bmax = bmax.max(b);
    }
    let r_range = rmax - rmin;
    let g_range = gmax - gmin;
    let b_range = bmax - bmin;

    if r_range >= g_range && r_range >= b_range {
        pixels.sort_by_key(|p| p.0);
    } else if g_range >= b_range {
        pixels.sort_by_key(|p| p.1);
    } else {
        pixels.sort_by_key(|p| p.2);
    }

    let mid = pixels.len() / 2;
    let b = pixels.split_off(mid);
    (pixels, b)
}

fn bucket_average(pixels: &[(u32, u32, u32)]) -> u32 {
    if pixels.is_empty() { return 0; }
    let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
    for &(r, g, b) in pixels {
        sr += r as u64;
        sg += g as u64;
        sb += b as u64;
    }
    let n = pixels.len() as u64;
    let r = (sr / n) as u32;
    let g = (sg / n) as u32;
    let b = (sb / n) as u32;
    (r << 16) | (g << 8) | b
}

fn luminance(color: u32) -> u32 {
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    (r * 299 + g * 587 + b * 114) / 1000
}

fn brighten(color: u32, amount: u32) -> u32 {
    let r = (((color >> 16) & 0xFF) + amount).min(255);
    let g = (((color >> 8) & 0xFF) + amount).min(255);
    let b = ((color & 0xFF) + amount).min(255);
    (r << 16) | (g << 8) | b
}

fn default_palette() -> [u32; 16] {
    [
        0x001A1A2E, 0x0016213E, 0x000F3460, 0x00533483,
        0x00E94560, 0x000097B2, 0x0087CEEB, 0x00E8E8E8,
        0x003A3A4E, 0x002A3A5E, 0x001F4470, 0x00634493,
        0x00F95570, 0x0010A7C2, 0x0097DEFB, 0x00F8F8F8,
    ]
}
