//! The scene list: what one frame of the desktop is made of
//! (docs/plan/GPU_STACK.md §5.3, §5.4 A2).
//!
//! A frame is a list of items, painted in order over the screen. Items are
//! plain data — rectangles, colours, and sources named by the window that
//! owns their pixels — with no code pointers, so the same list can be
//! handed to a GPU driver module (gpudev) and executed by its shaders. The
//! CPU executor here is the reference: it paints any part of a frame, item
//! by item, into a screen-sized target, clipped to damage rectangles. What
//! it produces is what the GPU executor must produce.
//!
//! Glass (`Item::Panel` with translucent pixels) blurs what is really
//! underneath it. That needs everything below to be painted first in a
//! margin around the panel, so damage touching a panel grows to cover it
//! (`expand_damage`).

use alloc::vec::Vec;

use crate::framebuffer::FbInfo;
use crate::gui::background;
use crate::gui::render;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self { Rect { x, y, w, h } }
    pub fn x1(&self) -> u32 { self.x + self.w }
    pub fn y1(&self) -> u32 { self.y + self.h }
    pub fn is_empty(&self) -> bool { self.w == 0 || self.h == 0 }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.x1().min(o.x1());
        let y1 = self.y1().min(o.y1());
        if x1 <= x0 || y1 <= y0 { Rect::new(x0, y0, 0, 0) } else { Rect::new(x0, y0, x1 - x0, y1 - y0) }
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() { return *o; }
        if o.is_empty() { return *self; }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        Rect::new(x0, y0, self.x1().max(o.x1()) - x0, self.y1().max(o.y1()) - y0)
    }

    /// Grown by `m` on every side, clamped at 0.
    pub fn grow(&self, m: u32) -> Rect {
        let x0 = self.x.saturating_sub(m);
        let y0 = self.y.saturating_sub(m);
        Rect::new(x0, y0, self.x1() + m - x0, self.y1() + m - y0)
    }
}

/// Whose pixels an item shows. The executor asks the owner for them
/// (`Sources`); a GPU executor maps each to its registered buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// A widget window's rasterized scene.
    Widget(u32),
    /// A guest surface's front frame.
    Surface(u32),
    /// A terminal window's text layer (alpha: set where text is).
    TermText(u32),
}

/// How a source fills a destination rectangle of a different size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Stretched to the destination (a window mid-animation shows its
    /// last pixels until it is rendered at the new size).
    Scale,
    /// 1:1, cropped where larger, its last row/column repeated where
    /// smaller (a guest frame on its way to a new tile size).
    Extend,
}

#[derive(Clone, Copy, Debug)]
pub enum Item {
    /// The wallpaper, or the plain background colour without one.
    Background,
    /// Soft halo outside a rounded rect (`render::draw_glow_ring`).
    Glow { rect: Rect, radius: u32, color: u32, band: u32, alpha: u32 },
    /// The blurred wallpaper inside a rounded rect: what terminal glass
    /// sits on.
    GlassBackdrop { rect: Rect, radius: u32 },
    /// Window chrome: border ring, and with `paint_content` the glass
    /// interior (`render::fill_rounded_chrome_aa`).
    Chrome {
        rect: Rect, border_a: u32, border_b: u32, bg: u32,
        rounding: u32, border: u32, border_opacity: u32, bg_opacity: u32,
        paint_content: bool,
    },
    /// Opaque window content in a rounded inner rect, corners anti-aliased.
    Content { src: Source, dst: Rect, radius: u32, fit: Fit },
    /// A panel (bar, dock): per-pixel alpha, translucent pixels as glass
    /// over a blur of what lies beneath.
    Panel { src: Source, dst: Rect },
    /// Text layer: pixels with alpha set replace what is beneath.
    Text { src: Source, dst: Rect },
    /// A colour over what lies beneath, `alpha` 0..=255, rounded corners.
    Fill { rect: Rect, color: u32, alpha: u32, radius: u32 },
    /// The window close glyph (`icons::IconId::X`), `size` px, centred in
    /// `rect`.
    CloseGlyph { rect: Rect, size: u32, color: u32 },
}

pub struct Scene {
    pub items: Vec<Item>,
}

/// Where the pixels of a `Source` are.
pub trait Sources {
    /// Calls `f(pixels, width, height)` with the source's pixels, if it has
    /// any right now.
    fn with_source(&self, src: Source, f: &mut dyn FnMut(&[u32], u32, u32));
}

/// Damage that touches a panel covers the panel and a margin around it:
/// its glass blurs what is beneath, which has to be painted there first.
pub fn expand_damage(scene: &Scene, damage: &mut Vec<Rect>, screen: Rect) {
    let margin = background::blur_margin();
    loop {
        let mut grew = false;
        for item in &scene.items {
            let Item::Panel { dst, .. } = item else { continue };
            let need = dst.grow(margin).intersect(&screen);
            for d in damage.iter_mut() {
                if !d.intersect(dst).is_empty() {
                    let u = d.union(&need);
                    if u != *d { *d = u; grew = true; }
                }
            }
        }
        if !grew { break; }
    }
}

/// Paint `scene` into `target` within each damage rectangle.
pub fn execute(scene: &Scene, sources: &dyn Sources, target: *mut u8, info: &FbInfo, damage: &[Rect]) {
    let screen = Rect::new(0, 0, info.width, info.height);
    for d in damage {
        let clip = d.intersect(&screen);
        if clip.is_empty() { continue; }
        for item in &scene.items {
            paint(item, sources, target, info, &clip);
        }
    }
}

fn paint(item: &Item, sources: &dyn Sources, t: *mut u8, info: &FbInfo, clip: &Rect) {
    match *item {
        Item::Background => background::draw_background_region(t, info, clip.x, clip.y, clip.w, clip.h),
        Item::Glow { rect, radius, color, band, alpha } => {
            render::draw_glow_ring(t, info, rect.x, rect.y, rect.w, rect.h, radius, color, band, alpha, clip);
        }
        Item::GlassBackdrop { rect, radius } => {
            background::draw_glass_backdrop(t, info, rect.x, rect.y, rect.w, rect.h, radius, clip);
        }
        Item::Chrome { rect, border_a, border_b, bg, rounding, border, border_opacity, bg_opacity, paint_content } => {
            render::fill_rounded_chrome_aa(t, info, rect.x, rect.y, rect.w, rect.h, border_a, border_b, bg,
                rounding, border, border_opacity, bg_opacity, paint_content, clip);
        }
        Item::Content { src, dst, radius, fit } => {
            let area = dst.intersect(clip);
            if area.is_empty() { return; }
            sources.with_source(src, &mut |px, sw, sh| content(t, info, px, sw, sh, dst, radius, fit, &area));
        }
        Item::Panel { src, dst } => {
            let area = dst.intersect(clip);
            if area.is_empty() { return; }
            sources.with_source(src, &mut |px, sw, sh| panel(t, info, px, sw, sh, dst, &area));
        }
        Item::Text { src, dst } => {
            let area = dst.intersect(clip);
            if area.is_empty() { return; }
            sources.with_source(src, &mut |px, sw, sh| text(t, info, px, sw, sh, dst, &area));
        }
        Item::Fill { rect, color, alpha, radius } => {
            let area = rect.intersect(clip);
            if area.is_empty() || alpha == 0 { return; }
            let r = radius.min(rect.w / 2).min(rect.h / 2);
            for y in area.y..area.y1() {
                for x in area.x..area.x1() {
                    let cov = render::rect_coverage_sdf(x, y, rect.x, rect.y, rect.w, rect.h, r);
                    if cov == 0 { continue; }
                    let a = alpha.min(255) * cov / 255;
                    render::blend_pixel(t, info, x, y, color & 0x00FF_FFFF, a.min(256));
                }
            }
        }
        Item::CloseGlyph { rect, size, color } => {
            let area = rect.intersect(clip);
            if area.is_empty() { return; }
            crate::gui::icons::with_alpha(crate::shade::widgets::abi::IconId::X, size as u16, |asz, glyph| {
                let asz = asz as u32;
                if asz == 0 || glyph.len() < (asz * asz) as usize { return; }
                let ox = rect.x + rect.w.saturating_sub(asz) / 2;
                let oy = rect.y + rect.h.saturating_sub(asz) / 2;
                let g = Rect::new(ox, oy, asz, asz).intersect(&area);
                for y in g.y..g.y1() {
                    for x in g.x..g.x1() {
                        let a = glyph[((y - oy) * asz + (x - ox)) as usize] as u32;
                        if a > 0 {
                            render::blend_pixel(t, info, x, y, color, a + (a >> 7));
                        }
                    }
                }
            });
        }
    }
}

/// The source pixel shown at `(lx, ly)` of a `dw` x `dh` destination.
#[inline]
fn sample(px: &[u32], sw: u32, sh: u32, dw: u32, dh: u32, fit: Fit, lx: u32, ly: u32) -> u32 {
    let (sx, sy) = match fit {
        Fit::Scale if sw != dw || sh != dh => (lx * sw / dw.max(1), ly * sh / dh.max(1)),
        _ => (lx, ly),
    };
    px[(sy.min(sh - 1) * sw + sx.min(sw - 1)) as usize]
}

fn content(t: *mut u8, info: &FbInfo, px: &[u32], sw: u32, sh: u32, dst: Rect, radius: u32, fit: Fit,
           area: &Rect) {
    if px.is_empty() || sw == 0 || sh == 0 || px.len() < (sw * sh) as usize { return; }
    let r = radius.min(dst.w / 2).min(dst.h / 2);
    let pitch = info.pitch as usize;
    let direct = fit == Fit::Extend || (sw == dst.w && sh == dst.h);
    let edge = |x: u32, y: u32| {
        let cov = render::rect_coverage_sdf(x, y, dst.x, dst.y, dst.w, dst.h, r);
        if cov > 0 {
            render::blend_pixel(t, info, x, y, sample(px, sw, sh, dst.w, dst.h, fit, x - dst.x, y - dst.y), cov);
        }
    };
    for y in area.y..area.y1() {
        let ly = y - dst.y;
        // Only the corner columns of corner rows need the SDF; the rest of a
        // row is a straight copy.
        let corner_row = r > 0 && (ly < r || ly >= dst.h - r);
        let (mid_lo, mid_hi) = if corner_row { (dst.x + r, dst.x1() - r) } else { (dst.x, dst.x1()) };
        let lo = mid_lo.clamp(area.x, area.x1());
        let hi = mid_hi.clamp(lo, area.x1());
        for x in area.x..lo { edge(x, y); }
        if direct && hi > lo {
            // The source row 1:1, then its last pixel repeated.
            let src_row = (ly.min(sh - 1) * sw) as usize;
            let have = hi.min(dst.x + sw).max(lo);
            // SAFETY: lo..hi lies inside the target row (the area is clipped
            // to the screen); src_row + (have - dst.x) <= src_row + sw.
            unsafe {
                let d = t.add(y as usize * pitch + lo as usize * 4) as *mut u32;
                if have > lo {
                    core::ptr::copy_nonoverlapping(px.as_ptr().add(src_row + (lo - dst.x) as usize), d,
                        (have - lo) as usize);
                }
                let last = px[src_row + sw as usize - 1];
                for i in (have - lo)..(hi - lo) { *d.add(i as usize) = last; }
            }
        } else {
            for x in lo..hi {
                render::put_pixel(t, info, x, y, sample(px, sw, sh, dst.w, dst.h, fit, x - dst.x, ly));
            }
        }
        for x in hi..area.x1() { edge(x, y); }
    }
}

fn panel(t: *mut u8, info: &FbInfo, px: &[u32], sw: u32, sh: u32, dst: Rect, area: &Rect) {
    if px.is_empty() || sw == 0 || sh == 0 || px.len() < (sw * sh) as usize { return; }
    // What lies beneath, blurred, for the translucent pixels.
    let blurred = background::blur_region(t, info, dst);
    let ink = crate::shade::glass::ink();
    let x1 = area.x1().min(dst.x + sw);
    let y1 = area.y1().min(dst.y + sh);
    for y in area.y..y1 {
        let ly = y - dst.y;
        for x in area.x..x1 {
            let p = px[(ly * sw + (x - dst.x)) as usize];
            let a = (p >> 24) & 0xFF;
            if a == 0 { continue; }
            if a < 255 {
                let base = match &blurred {
                    Some(b) => b[(ly * dst.w + (x - dst.x)) as usize],
                    None => render::read_pixel(t, info, x, y),
                };
                render::put_pixel(t, info, x, y, render::glass_blend(ink.fill(p & 0x00FF_FFFF), base, a, ink));
                continue;
            }
            render::blend_pixel(t, info, x, y, p & 0x00FF_FFFF, a);
        }
    }
}

fn text(t: *mut u8, info: &FbInfo, px: &[u32], sw: u32, sh: u32, dst: Rect, area: &Rect) {
    if px.is_empty() || px.len() < (sw * sh) as usize { return; }
    let x1 = area.x1().min(dst.x + sw);
    let y1 = area.y1().min(dst.y + sh);
    for y in area.y..y1 {
        let row = ((y - dst.y) * sw) as usize;
        for x in area.x..x1 {
            let p = px[row + (x - dst.x) as usize];
            if p >> 24 != 0 {
                render::put_pixel(t, info, x, y, p & 0x00FF_FFFF);
            }
        }
    }
}
