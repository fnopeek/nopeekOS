//! Font rendering using Spleen bitmap fonts.
//!
//! Auto-selects font size based on screen resolution:
//!   8x16  for <= 1920px width
//!   16x32 for > 1920px width
//! Clock uses 32x64 (or 16x32 at 1080p).

use crate::framebuffer::FbInfo;
use super::render;
use super::fonts;

/// Font descriptor for a specific size.
struct FontDesc {
    data: &'static [u8],
    glyph_w: u32,
    glyph_h: u32,
    bytes_per_row: u32,
}

const FONT_8X16: FontDesc = FontDesc {
    data: &fonts::FONT_8X16,
    glyph_w: 8, glyph_h: 16, bytes_per_row: 1,
};

const FONT_16X32: FontDesc = FontDesc {
    data: &fonts::FONT_16X32,
    glyph_w: 16, glyph_h: 32, bytes_per_row: 2,
};

const FONT_32X64: FontDesc = FontDesc {
    data: &fonts::FONT_32X64,
    glyph_w: 32, glyph_h: 64, bytes_per_row: 4,
};

/// Auto-detect scale: 2x for >1920px width, else 1x.
pub fn scale_for(screen_width: u32) -> u32 {
    if screen_width > 1920 { 2 } else { 1 }
}

/// Get the appropriate font for a given scale.
fn font_for_scale(scale: u32) -> &'static FontDesc {
    if scale >= 2 { &FONT_16X32 } else { &FONT_8X16 }
}

/// Get the large clock font.
fn clock_font(scale: u32) -> &'static FontDesc {
    if scale >= 2 { &FONT_32X64 } else { &FONT_16X32 }
}

/// Character cell size for UI text at given scale.
pub fn char_size(scale: u32) -> (u32, u32) {
    let f = font_for_scale(scale);
    (f.glyph_w, f.glyph_h)
}

/// Character cell size for large clock text.
pub fn clock_char_size(scale: u32) -> (u32, u32) {
    let f = clock_font(scale);
    (f.glyph_w, f.glyph_h)
}

/// Draw one character using a specific font descriptor.
fn draw_char_with(shadow: *mut u8, info: &FbInfo, font: &FontDesc,
                  ch: u8, px_x: u32, px_y: u32,
                  fg: u32, bg: Option<u32>) {
    let idx = ch as usize;
    let glyph_bytes = font.glyph_h as usize * font.bytes_per_row as usize;
    let start = idx * glyph_bytes;
    if start + glyph_bytes > font.data.len() { return; }

    for gy in 0..font.glyph_h {
        let row_start = start + gy as usize * font.bytes_per_row as usize;
        for gx in 0..font.glyph_w {
            let byte_idx = gx / 8;
            let bit_idx = 7 - (gx % 8);
            let on = font.data[row_start + byte_idx as usize] & (1 << bit_idx) != 0;
            let color = if on {
                fg
            } else {
                match bg {
                    Some(c) => c,
                    None => continue,
                }
            };
            render::put_pixel(shadow, info, px_x + gx, px_y + gy, color);
        }
    }
}

/// Draw a single character at pixel position (UI scale).
pub fn draw_char(shadow: *mut u8, info: &FbInfo,
                 ch: u8, px_x: u32, px_y: u32,
                 fg: u32, bg: Option<u32>, scale: u32) {
    draw_char_with(shadow, info, font_for_scale(scale), ch, px_x, px_y, fg, bg);
}

/// Draw a string. Returns the X position after the last character.
pub fn draw_str(shadow: *mut u8, info: &FbInfo,
                s: &str, px_x: u32, px_y: u32,
                fg: u32, bg: Option<u32>, scale: u32) -> u32 {
    let f = font_for_scale(scale);
    let mut x = px_x;
    for &byte in s.as_bytes() {
        if byte >= 0x20 && byte < 0x7F {
            draw_char_with(shadow, info, f, byte, x, px_y, fg, bg);
            x += f.glyph_w;
        }
    }
    x
}

/// Measure string width in pixels.
pub fn measure_str(s: &str, scale: u32) -> u32 {
    let f = font_for_scale(scale);
    let n = s.as_bytes().iter().filter(|&&b| b >= 0x20 && b < 0x7F).count() as u32;
    n * f.glyph_w
}

/// Draw a string centered horizontally within a given region.
pub fn draw_str_centered(shadow: *mut u8, info: &FbInfo,
                         s: &str, region_x: u32, region_w: u32, py: u32,
                         fg: u32, bg: Option<u32>, scale: u32) {
    let text_w = measure_str(s, scale);
    let x = if text_w < region_w {
        region_x + (region_w - text_w) / 2
    } else {
        region_x
    };
    draw_str(shadow, info, s, x, py, fg, bg, scale);
}

/// Draw a large clock string (uses the bigger font).
pub fn draw_clock_str_centered(shadow: *mut u8, info: &FbInfo,
                               s: &str, region_x: u32, region_w: u32, py: u32,
                               fg: u32, bg: Option<u32>, scale: u32) {
    let f = clock_font(scale);
    let n = s.as_bytes().iter().filter(|&&b| b >= 0x20 && b < 0x7F).count() as u32;
    let text_w = n * f.glyph_w;
    let x = if text_w < region_w {
        region_x + (region_w - text_w) / 2
    } else {
        region_x
    };
    let mut cx = x;
    for &byte in s.as_bytes() {
        if byte >= 0x20 && byte < 0x7F {
            draw_char_with(shadow, info, f, byte, cx, py, fg, bg);
            cx += f.glyph_w;
        }
    }
}
