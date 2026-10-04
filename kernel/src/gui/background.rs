//! Default background + accent lookup.
//!
//! Two modes:
//! 1. A flat dark-grey fill when no wallpaper is set (matches the
//!    login screen's gradient base — consistent system look).
//! 2. A user-supplied wallpaper copied in from `wallpaper.wasm`,
//!    which also extracts a 16-colour theme palette via theme::.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// The wallpaper, heavily blurred, same size and pitch. Glass surfaces
/// (loop, dock, bar) blend over this instead of the sharp image: a
/// translucent light panel over sharp texture leaves the texture
/// competing with the text on it, and blur is what makes glass readable.
/// Computed once per wallpaper — the wallpaper is static, so reading it
/// costs a frame exactly what reading the sharp one did.
static mut BLURRED: *mut u8 = core::ptr::null_mut();
static mut BLURRED_W: u32 = 0;
static mut BLURRED_H: u32 = 0;
static BLURRED_SET: AtomicBool = AtomicBool::new(false);

/// Downscale factor for the blur. Blur keeps no detail, so it is computed
/// on a small image and scaled back up.
const BLUR_DOWN: u32 = 4;
const BLUR_PASSES: usize = 3;

/// Bumped every time the wallpaper pixels change. Mixed into the compositor's
/// translucent-glass cache key so a same-theme wallpaper swap invalidates it
/// (the key otherwise tracks colour/geometry only, not the backdrop pixels —
/// and set_wallpaper overwrites the buffer in place, so clearing the cache
/// alone races a cross-core re-store under the unchanged key). Bump after the
/// pixel write with Release: a core that Acquire-observes the new generation
/// also observes the finished new pixels.
static WALLPAPER_GEN: AtomicU32 = AtomicU32::new(0);

/// Generation counter of the active wallpaper (see WALLPAPER_GEN).
pub fn wallpaper_generation() -> u32 {
    WALLPAPER_GEN.load(Ordering::Acquire)
}

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

    scale_cover(pixels, w, h, buf, info);

    unsafe {
        WALLPAPER = buf;
        WALLPAPER_W = target_w;
        WALLPAPER_H = target_h;
    }
    compute_blur(buf, info, pages);
    WALLPAPER_SET.store(true, Ordering::Release);
    // After the pixels are fully written: invalidate the glass cache by moving
    // the generation forward (Release pairs with the Acquire in the cache key).
    WALLPAPER_GEN.fetch_add(1, Ordering::Release);

    let pixel_count = (w * h) as usize;
    let palette = crate::theme::extract_palette(pixels, pixel_count);
    crate::theme::set_palette(&palette);

    // Re-rasterize live widget scenes so cached pixels pick up new tokens.
    crate::shade::widgets::refresh_all_scenes();
}

pub fn has_wallpaper() -> bool {
    WALLPAPER_SET.load(Ordering::Acquire)
}

/// Raw wallpaper buffer (framebuffer-pitch layout, BGRX u32), or null.
fn wallpaper_ptr() -> *const u8 {
    if WALLPAPER_SET.load(Ordering::Acquire) { unsafe { WALLPAPER } } else { core::ptr::null() }
}

pub fn clear_wallpaper() {
    WALLPAPER_SET.store(false, Ordering::Release);
    BLURRED_SET.store(false, Ordering::Release);
    crate::shade::glass::set_stats(None);
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

/// Restore the background in a rect, but only where `want` says so.
///
/// The focus halo wraps the rounded corner of a tile, so it also paints the
/// four corner notches — inside the tile's bounding box, outside its
/// outline. A plain rect restore there would wipe the window's own arc, so
/// the caller masks it down to the pixels the halo can reach.
pub fn draw_background_region_where(shadow: *mut u8, info: &FbInfo,
                                    rx: u32, ry: u32, rw: u32, rh: u32,
                                    want: impl Fn(u32, u32) -> bool) {
    let wp = unsafe { WALLPAPER };
    let pitch = info.pitch as usize;
    let x1 = (rx + rw).min(info.width);
    let y1 = (ry + rh).min(info.height);
    for y in ry..y1 {
        let row = y as usize * pitch;
        for x in rx..x1 {
            if !want(x, y) { continue }
            let off = row + x as usize * 4;
            // SAFETY: x < info.width and y < info.height, so `off` is inside
            // the shadow buffer; the wallpaper, when present, is allocated
            // screen-sized with the same pitch (see `set_wallpaper`).
            unsafe {
                let px = if wp.is_null() { BG_GREY } else { *(wp.add(off) as *const u32) };
                *(shadow.add(off) as *mut u32) = px;
            }
        }
    }
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

// ── Glass backdrop ────────────────────────────────────────────────────

/// Blur the current wallpaper again — after `set shade.blur`.
pub fn reblur() {
    if !has_wallpaper() { return; }
    let info = crate::framebuffer::get_info();
    let pages = (info.height as usize * info.pitch as usize + 4095) / 4096;
    compute_blur(unsafe { WALLPAPER }, &info, pages);
    // Every glass cache keys on the generation.
    WALLPAPER_GEN.fetch_add(1, Ordering::Release);
}

fn compute_blur(wp: *const u8, info: &FbInfo, pages: usize) {
    BLURRED_SET.store(false, Ordering::Release);
    let (w, h, pitch) = (info.width, info.height, info.pitch as usize);
    let dst = if unsafe { !BLURRED.is_null() && BLURRED_W == w && BLURRED_H == h } {
        unsafe { BLURRED }
    } else {
        match crate::memory::allocate_contiguous(pages) {
            Some(addr) => addr as *mut u8,
            None => {
                crate::kprintln!("[npk] glass: no memory for the blurred wallpaper ({} pages), glass stays sharp", pages);
                return;
            }
        }
    };
    let t0 = crate::interrupts::ticks();

    // 1. Average BLUR_DOWN² blocks into a small image, one channel per plane.
    let sw = ((w + BLUR_DOWN - 1) / BLUR_DOWN) as usize;
    let sh = ((h + BLUR_DOWN - 1) / BLUR_DOWN) as usize;
    let mut planes = [alloc::vec![0u32; sw * sh], alloc::vec![0u32; sw * sh], alloc::vec![0u32; sw * sh]];
    // Detail: mean luma standard deviation inside the blocks — the texture
    // that the averaging removes, i.e. what would compete with text.
    let mut detail_sum: u64 = 0;
    for sy in 0..sh {
        if sy % 32 == 0 { crate::xhci::poll_events(); }
        for sx in 0..sw {
            let (x0, y0) = (sx as u32 * BLUR_DOWN, sy as u32 * BLUR_DOWN);
            let (x1, y1) = ((x0 + BLUR_DOWN).min(w), (y0 + BLUR_DOWN).min(h));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            let (mut ls, mut lq) = (0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    // SAFETY: x < width, y < height; the wallpaper is
                    // screen-sized with the framebuffer pitch (set_wallpaper).
                    let px = unsafe { *(wp.add(y as usize * pitch + x as usize * 4) as *const u32) };
                    let (pr, pg, pb) = ((px >> 16) & 0xFF, (px >> 8) & 0xFF, px & 0xFF);
                    r += pr; g += pg; b += pb; n += 1;
                    let l = (pr * 77 + pg * 150 + pb * 29) >> 8;
                    ls += l; lq += l * l;
                }
            }
            let n1 = n.max(1);
            let mean = ls / n1;
            detail_sum += isqrt((lq / n1).saturating_sub(mean * mean)) as u64;
            let i = sy * sw + sx;
            planes[0][i] = r / n.max(1);
            planes[1][i] = g / n.max(1);
            planes[2][i] = b / n.max(1);
        }
    }

    let detail = (detail_sum / (sw * sh).max(1) as u64) as u32;
    let radius = crate::shade::glass::blur_radius(detail);

    // 2. Separable box blur, a few passes (none at radius 0).
    let mut tmp = alloc::vec![0u32; sw * sh];
    for plane in planes.iter_mut() {
        for _ in 0..if radius == 0 { 0 } else { BLUR_PASSES } {
            box_pass(plane, &mut tmp, sw, sh, 1, sw, radius);   // columns
            box_pass(&tmp, plane, sh, sw, sw, 1, radius);       // rows
        }
    }

    // What the glass will sit on: luma percentiles and mean colour of the
    // blurred image. `shade::glass` derives its values from these.
    {
        let mut hist = [0u32; 256];
        let (mut rs, mut gs, mut bs) = (0u64, 0u64, 0u64);
        for i in 0..sw * sh {
            let (r, g, b) = (planes[0][i], planes[1][i], planes[2][i]);
            hist[((r * 77 + g * 150 + b * 29) >> 8).min(255) as usize] += 1;
            rs += r as u64; gs += g as u64; bs += b as u64;
        }
        let total = (sw * sh) as u32;
        let pct = |p: u32| {
            let want = total * p / 100;
            let mut acc = 0u32;
            for (v, &c) in hist.iter().enumerate() {
                acc += c;
                if acc > want { return v as u32; }
            }
            255
        };
        let n = (sw * sh).max(1) as u64;
        crate::shade::glass::set_stats(Some(crate::shade::glass::Stats {
            p10: pct(10), p50: pct(50), p90: pct(90), detail,
            mean_rgb: ((rs / n) as u32) << 16 | ((gs / n) as u32) << 8 | (bs / n) as u32,
        }));
    }

    // 3. Bilinear back up to screen size (sample centres of the blocks).
    let half = (BLUR_DOWN / 2) as i32;
    for y in 0..h {
        if y % 256 == 0 { crate::xhci::poll_events(); }
        let fy = ((y as i32 - half).max(0) as u32 * 256 / BLUR_DOWN) as usize;
        let (y0, wy) = ((fy >> 8).min(sh - 1), (fy & 0xFF) as u32);
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w {
            let fx = ((x as i32 - half).max(0) as u32 * 256 / BLUR_DOWN) as usize;
            let (x0, wx) = ((fx >> 8).min(sw - 1), (fx & 0xFF) as u32);
            let x1 = (x0 + 1).min(sw - 1);
            let mut px = 0u32;
            for (c, plane) in planes.iter().enumerate() {
                let top = plane[y0 * sw + x0] * (256 - wx) + plane[y0 * sw + x1] * wx;
                let bot = plane[y1 * sw + x0] * (256 - wx) + plane[y1 * sw + x1] * wx;
                let v = (top * (256 - wy) + bot * wy) >> 16;
                px |= v.min(255) << (16 - 8 * c as u32);
            }
            // SAFETY: same bounds and layout as the wallpaper buffer above.
            unsafe { *(dst.add(y as usize * pitch + x as usize * 4) as *mut u32) = px; }
        }
    }

    unsafe {
        BLURRED = dst;
        BLURRED_W = w;
        BLURRED_H = h;
    }
    BLURRED_SET.store(true, Ordering::Release);
    crate::kdebug!("[npk] glass: wallpaper blurred in {} ms (detail {}, blur {})",
        (crate::interrupts::ticks() - t0) * 10, detail, radius);
}

/// One box-blur pass along a line direction. `lines` × `len` samples,
/// `step` between samples of a line, `stride` between lines. Edges clamp.
fn box_pass(src: &[u32], dst: &mut [u32], lines: usize, len: usize, stride: usize, step: usize,
            radius: usize) {
    let r = radius as isize;
    let n = (2 * r + 1) as u32;
    let last = len as isize - 1;
    for l in 0..lines {
        let base = l * stride;
        let at = |i: isize| src[base + (i.clamp(0, last) as usize) * step];
        let mut sum: u32 = (-r..=r).map(at).sum();
        for i in 0..len as isize {
            dst[base + i as usize * step] = sum / n;
            sum = sum + at(i + r + 1) - at(i - r);
        }
    }
}

/// What translucent glass is precomputed from: the blurred wallpaper when
/// it is ready, else the sharp one, else null (no wallpaper).
pub fn glass_source_ptr() -> *const u8 {
    if blurred_ready() { unsafe { BLURRED } } else { wallpaper_ptr() }
}

fn blurred_ready() -> bool {
    WALLPAPER_SET.load(Ordering::Acquire) && BLURRED_SET.load(Ordering::Acquire)
}

/// Put the blurred wallpaper under a glass window: inside the rounded rect
/// only, so the corners outside it keep the sharp wallpaper the gap shows.
/// Call after the sharp restore. No wallpaper → nothing to blur, no-op.
pub fn draw_glass_backdrop(shadow: *mut u8, info: &FbInfo,
                           rx: u32, ry: u32, rw: u32, rh: u32, radius: u32) {
    if !blurred_ready() { return; }
    let bl = unsafe { BLURRED };
    let pitch = info.pitch as usize;
    let r = radius.min(rw / 2).min(rh / 2);
    let y1 = (ry + rh).min(info.height);
    for y in ry..y1 {
        let ly = y - ry;
        let from_edge = if ly < r { r - ly } else if ly >= rh - r { ly + 1 - (rh - r) } else { 0 };
        let inset = if from_edge == 0 { 0 } else { r - isqrt(r * r - (from_edge * from_edge).min(r * r)) };
        let x0 = rx + inset;
        let x1 = (rx + rw).saturating_sub(inset).min(info.width);
        if x1 <= x0 { continue; }
        let off = y as usize * pitch + x0 as usize * 4;
        // SAFETY: x0..x1 and y lie inside the screen; both buffers are
        // screen-sized with the framebuffer pitch.
        unsafe { core::ptr::copy_nonoverlapping(bl.add(off), shadow.add(off), (x1 - x0) as usize * 4); }
    }
}

/// What a glass pixel at (x,y) should blend over. Where the shadow still
/// holds exactly the wallpaper, nothing else is underneath, and the blurred
/// copy takes its place; over a window the window stays (the dock can
/// slide over a tile).
pub fn glass_base_at(info: &FbInfo, x: u32, y: u32, current: u32) -> u32 {
    if !blurred_ready() { return current; }
    let off = y as usize * info.pitch as usize + x as usize * 4;
    // SAFETY: caller passes on-screen coordinates; both buffers are
    // screen-sized with the framebuffer pitch.
    unsafe {
        let sharp = *(WALLPAPER.add(off) as *const u32);
        if (sharp ^ current) & 0x00FF_FFFF == 0 { *(BLURRED.add(off) as *const u32) } else { current }
    }
}

fn isqrt(v: u32) -> u32 {
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x { x = y; y = (x + v / x) / 2; }
    x
}

// ── Scaling ───────────────────────────────────────────────────────────

/// Fit the image to the screen the way a photo frame does: fill it, keep
/// the aspect ratio, crop the overhang evenly (centre). Shrinking averages
/// every source pixel a screen pixel covers — a 4K image on an HD screen
/// otherwise keeps every second pixel and drops the rest, and fine lines
/// turn into stairs. Enlarging interpolates bilinearly. Done once, when the
/// wallpaper is set.
fn scale_cover(src: &[u8], w: u32, h: u32, dst: *mut u8, info: &FbInfo) {
    let (tw, th, pitch) = (info.width as u64, info.height as u64, info.pitch as usize);
    let (w64, h64) = (w as u64, h as u64);
    if w == 0 || h == 0 || src.len() < (w64 * h64 * 4) as usize { return; }
    // Source window that maps onto the screen, in 16.16 fixed point:
    // the larger of the two scale factors wins, the other axis is cropped.
    let (cw, ch) = if w64 * th > h64 * tw {
        (h64 * tw * 65536 / th, h64 * 65536)        // wider than the screen
    } else {
        (w64 * 65536, w64 * th * 65536 / tw)        // taller (or equal)
    };
    let ox = (w64 * 65536 - cw) / 2;
    let oy = (h64 * 65536 - ch) / 2;
    let px_at = |x: u64, y: u64| -> (u32, u32, u32) {
        let o = ((y.min(h64 - 1) * w64 + x.min(w64 - 1)) * 4) as usize;
        (src[o + 2] as u32, src[o + 1] as u32, src[o] as u32)
    };
    let shrinking = cw >= tw * 65536;

    for ty in 0..th {
        if ty % 128 == 0 { crate::xhci::poll_events(); }
        let fy0 = oy + ty * ch / th;
        let fy1 = oy + (ty + 1) * ch / th;
        let row = unsafe { dst.add(ty as usize * pitch) as *mut u32 };
        for tx in 0..tw {
            let fx0 = ox + tx * cw / tw;
            let fx1 = ox + (tx + 1) * cw / tw;
            let (r, g, b) = if shrinking {
                // Box average over the covered source pixels.
                let (sx0, sx1) = (fx0 >> 16, ((fx1 + 65535) >> 16).max((fx0 >> 16) + 1));
                let (sy0, sy1) = (fy0 >> 16, ((fy1 + 65535) >> 16).max((fy0 >> 16) + 1));
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for sy in sy0..sy1.min(h64) {
                    for sx in sx0..sx1.min(w64) {
                        let (pr, pg, pb) = px_at(sx, sy);
                        r += pr; g += pg; b += pb; n += 1;
                    }
                }
                let n = n.max(1);
                (r / n, g / n, b / n)
            } else {
                // Bilinear at the pixel centre.
                let cx = ((fx0 + fx1) / 2).saturating_sub(32768);
                let cy = ((fy0 + fy1) / 2).saturating_sub(32768);
                let (x0, y0) = (cx >> 16, cy >> 16);
                let (wx, wy) = (((cx >> 8) & 0xFF) as u32, ((cy >> 8) & 0xFF) as u32);
                let (a, bq, c, d) = (px_at(x0, y0), px_at(x0 + 1, y0), px_at(x0, y0 + 1), px_at(x0 + 1, y0 + 1));
                let mix = |p: u32, q: u32, s: u32, t: u32| {
                    ((p * (256 - wx) + q * wx) * (256 - wy) + (s * (256 - wx) + t * wx) * wy) >> 16
                };
                (mix(a.0, bq.0, c.0, d.0), mix(a.1, bq.1, c.1, d.1), mix(a.2, bq.2, c.2, d.2))
            };
            // SAFETY: tx < width, ty < height; `dst` is screen-sized with
            // the framebuffer pitch (allocated in set_wallpaper).
            unsafe { *row.add(tx as usize) = (r << 16) | (g << 8) | b; }
        }
    }
}
