//! Bitmap font rendering with integer scaling.
//!
//! Uses the CP437 8x16 font from framebuffer.rs.
//! Scale 1x = 8x16, 2x = 16x32 (for 4K displays).

use crate::framebuffer::{FONT, FbInfo};
use super::render;

const GLYPH_W: u32 = 8;
const GLYPH_H: u32 = 16;

/// Auto-detect scale: 2x for >1920px width, else 1x.
pub fn scale_for(screen_width: u32) -> u32 {
    if screen_width > 1920 { 2 } else { 1 }
}

/// Character cell size at given scale.
pub fn char_size(scale: u32) -> (u32, u32) {
    (GLYPH_W * scale, GLYPH_H * scale)
}

/// Draw one character at pixel position (px_x, px_y).
/// If bg is None, only foreground pixels are drawn (transparent background).
pub fn draw_char(shadow: *mut u8, info: &FbInfo,
                 ch: u8, px_x: u32, px_y: u32,
                 fg: u32, bg: Option<u32>, scale: u32) {
    let start = ch as usize * GLYPH_H as usize;
    if start + GLYPH_H as usize > FONT.len() { return; }

    for gy in 0..GLYPH_H {
        let bits = FONT[start + gy as usize];
        for gx in 0..GLYPH_W {
            let on = bits & (0x80 >> gx) != 0;
            let color = if on {
                fg
            } else {
                match bg {
                    Some(c) => c,
                    None => continue,
                }
            };
            // Write a scale×scale pixel block
            for sy in 0..scale {
                for sx in 0..scale {
                    render::put_pixel(shadow, info,
                        px_x + gx * scale + sx,
                        px_y + gy * scale + sy,
                        color);
                }
            }
        }
    }
}

/// Draw a string. Returns the X position after the last character.
pub fn draw_str(shadow: *mut u8, info: &FbInfo,
                s: &str, px_x: u32, px_y: u32,
                fg: u32, bg: Option<u32>, scale: u32) -> u32 {
    let (cw, _) = char_size(scale);
    let mut x = px_x;
    for &byte in s.as_bytes() {
        if byte >= 0x20 && byte < 0x7F {
            draw_char(shadow, info, byte, x, px_y, fg, bg, scale);
            x += cw;
        }
    }
    x
}

/// Measure string width in pixels (printable ASCII only).
pub fn measure_str(s: &str, scale: u32) -> u32 {
    let n = s.as_bytes().iter().filter(|&&b| b >= 0x20 && b < 0x7F).count() as u32;
    n * GLYPH_W * scale
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
