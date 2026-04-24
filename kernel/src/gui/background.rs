//! Default background + accent lookup.
//!
//! Two modes:
//! 1. A flat dark-grey fill when no wallpaper is set (matches the
//!    login screen's gradient base — consistent system look).
//! 2. A user-supplied wallpaper copied in from `wallpaper.wasm`,
//!    which also extracts a 16-colour theme palette via theme::.

use core::sync::atomic::{AtomicBool, Ordering};
use crate::framebuffer::FbInfo;

/// Default dark-grey background pixel (0xAARRGGBB).
const BG_GREY: u32 = 0xFF181820;

/// Accent used when no wallpaper theme is active — kept in sync with
/// `shade::widgets::palette::fallback(Token::Accent)`.
const DEFAULT_ACCENT: u32 = 0x007B50A0;

static mut WALLPAPER: *mut u8 = core::ptr::null_mut();
static mut WALLPAPER_W: u32 = 0;
static mut WALLPAPER_H: u32 = 0;
static WALLPAPER_SET: AtomicBool = AtomicBool::new(false);

pub fn init() {}

pub fn accent_color() -> u32 {
    let color = if crate::theme::is_active() {
        crate::theme::accent()
    } else {
        return DEFAULT_ACCENT;
    };
    // Ensure accent is bright enough to read on dark window bg.
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    let lum = (r * 299 + g * 587 + b * 114) / 1000;
    if lum < 100 {
        let boost = 100 - lum;
        let r = (r + boost).min(255);
        let g = (g + boost).min(255);
        let b = (b + boost).min(255);
        (r << 16) | (g << 8) | b
    } else {
        color
    }
}

pub fn set_wallpaper(pixels: &[u8], w: u32, h: u32, info: &FbInfo) {
    let target_w = info.width;
    let target_h = info.height;
    let size = target_h as usize * info.pitch as usize;
    let pages = (size + 4095) / 4096;

    let buf = if !unsafe { WALLPAPER.is_null() } && unsafe { WALLPAPER_W == target_w && WALLPAPER_H == target_h } {
        unsafe { WALLPAPER }
    } else {
        match crate::memory::allocate_contiguous(pages) {
            Some(addr) => addr as *mut u8,
            None => return,
        }
    };

    // Nearest-neighbour scale to framebuffer size.
    for ty in 0..target_h {
        if ty % 256 == 0 { crate::xhci::poll_events(); }
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
            unsafe { *(buf.add(dst_off) as *mut u32) = pixel; }
        }
    }

    unsafe {
        WALLPAPER = buf;
        WALLPAPER_W = target_w;
        WALLPAPER_H = target_h;
    }
    WALLPAPER_SET.store(true, Ordering::Release);

    let pixel_count = (w * h) as usize;
    let palette = crate::theme::extract_palette(pixels, pixel_count);
    crate::theme::set_palette(&palette);

    // Re-rasterize live widget scenes so cached pixels pick up new tokens.
    crate::shade::widgets::refresh_all_scenes();
}

pub fn has_wallpaper() -> bool {
    WALLPAPER_SET.load(Ordering::Acquire)
}

pub fn clear_wallpaper() {
    WALLPAPER_SET.store(false, Ordering::Release);
    crate::theme::clear();
    crate::shade::widgets::refresh_all_scenes();
}

pub fn draw_background(shadow: *mut u8, info: &FbInfo) {
    if has_wallpaper() {
        draw_wallpaper(shadow, info);
    } else {
        fill_grey(shadow, info, 0, 0, info.width, info.height);
    }
}

pub fn draw_background_region(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
    if has_wallpaper() {
        draw_wallpaper_region(shadow, info, rx, ry, rw, rh);
    } else {
        fill_grey(shadow, info, rx, ry, rw, rh);
    }
}

fn fill_grey(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
    let pitch = info.pitch as usize;
    let x1 = ((rx + rw) as usize).min(info.width as usize);
    let y1 = (ry + rh).min(info.height);
    for y in ry..y1 {
        let row = unsafe { shadow.add(y as usize * pitch) as *mut u32 };
        for x in (rx as usize)..x1 {
            unsafe { *row.add(x) = BG_GREY; }
        }
    }
}

fn draw_wallpaper(shadow: *mut u8, info: &FbInfo) {
    let wp = unsafe { WALLPAPER };
    if wp.is_null() { return; }
    let size = info.height as usize * info.pitch as usize;
    unsafe { core::ptr::copy_nonoverlapping(wp, shadow, size); }
}

fn draw_wallpaper_region(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {
    let wp = unsafe { WALLPAPER };
    if wp.is_null() {
        fill_grey(shadow, info, rx, ry, rw, rh);
        return;
    }
    let pitch = info.pitch as usize;
    let x0 = rx as usize;
    let x1 = ((rx + rw) as usize).min(info.width as usize);
    let bytes = (x1 - x0) * 4;
    for y in ry..(ry + rh).min(info.height) {
        let off = y as usize * pitch + x0 * 4;
        unsafe { core::ptr::copy_nonoverlapping(wp.add(off), shadow.add(off), bytes); }
    }
}
