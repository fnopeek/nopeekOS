//! Rasteriser — draws a `Layout`'s display list into a BGRA pixel buffer.
//!
//! Glyphs come from fontdue (Inter, the same font the compositor uses); the
//! layout is ours. `paint` renders the visible slice at a scroll offset, so
//! the buffer stays viewport-sized regardless of document length. Pure — the
//! same code paints on nopeekOS (into a `Widget::Canvas`) and on the desktop
//! adapter (into a window framebuffer), see BROWSER.md §10.

use alloc::vec::Vec;
use core::cell::RefCell;
use fontdue::Metrics;
use hashbrown::HashMap;

use crate::fonts::Fonts;
use crate::layout::{DrawOp, Layout, Rgb, Theme};

pub struct Engine {
    fonts: Fonts,
    /// Rasterised-glyph cache keyed by (char, size-bits, face-id). fontdue's
    /// rasterise is not free; without this every glyph is re-rasterised every
    /// frame, which makes scrolling lag. Bounded by the glyph set the page uses.
    glyphs: RefCell<HashMap<(u32, u32, u32), (Metrics, Vec<u8>)>>,
    /// Page colours (theme-resolved by the shell; dark until then).
    theme: Theme,
    /// Decoded page images keyed by `<img src>` (set by the shell each nav).
    images: crate::image::ImageMap,
    /// Remaining decoded-BGRA budget for the current page (streaming decode).
    img_budget: usize,
    /// Viewport height (px) — the initial containing block's height, which
    /// `top`/`bottom`/`height` percentages on root-level absolutely positioned
    /// boxes resolve against (CSS 2.1 §10.1). Device state like `theme`, not
    /// page content, so it lives here rather than in every layout signature.
    /// `Cell` for the same reason `glyphs` is a `RefCell`: the shell holds the
    /// engine by shared reference across a frame.
    viewport_h: core::cell::Cell<u32>,
    /// When set, `layout` records an `InspectBox` per element box (the dev
    /// tool). Off by default so the label-formatting cost is only paid while the
    /// user is inspecting; the shell toggles it and re-lays-out.
    inspect: core::cell::Cell<bool>,
    /// The last parsed stylesheet with the fingerprint of the inputs that built
    /// it. Parsing a real page's CSS is a third of a layout, and a page is laid
    /// out several times over its life (images landing, a form key, a resize)
    /// from unchanged bytes — so the parse is repeated for nothing.
    sheet: RefCell<Option<(u64, crate::css::Stylesheet)>>,
}

/// Cheap content fingerprint (FNV-1a over 8-byte words). Identity by pointer
/// would be wrong here: the shell parses into one static buffer, so a different
/// document can land at the same address with the same length.
fn fingerprint(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut it = b.chunks_exact(8);
    for c in &mut it {
        h ^= u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for &x in it.remainder() {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ b.len() as u64
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Parse the embedded font faces. Cheap enough to build once and reuse
    /// across page loads (the shell keeps one `Engine`).
    pub fn new() -> Engine {
        Engine {
            fonts: Fonts::new(),
            glyphs: RefCell::new(HashMap::new()),
            theme: Theme::DARK,
            images: crate::image::ImageMap::new(),
            img_budget: crate::image::TOTAL_BUDGET,
            // 600 keeps the historical behaviour of the reftest canvas for any
            // caller that never sets it.
            viewport_h: core::cell::Cell::new(600),
            inspect: core::cell::Cell::new(false),
            sheet: RefCell::new(None),
        }
    }

    /// Tell the engine how tall the viewport is (see `viewport_h`).
    pub fn set_viewport_h(&self, h: u32) {
        if h > 0 {
            self.viewport_h.set(h);
        }
    }

    /// Enable/disable the inspect dev tool. When on, the next `layout` records
    /// an element box per node into `Layout::inspect`; the shell re-lays-out.
    pub fn set_inspect(&self, on: bool) {
        self.inspect.set(on);
    }

    /// Set the page colours (the shell resolves these from the compositor
    /// palette so the page follows light/dark).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Start a fresh page's image set: clear the previous decode + reset the
    /// per-page budget. The shell then fetches + `add_image`s each `<img>` ONE
    /// AT A TIME (streaming) so the compressed bytes never pile up — decode the
    /// image, keep only its pixels, reuse the same fetch scratch for the next.
    pub fn images_begin(&mut self) {
        self.images.clear();
        self.img_budget = crate::image::TOTAL_BUDGET;
        // Drop the previous page's rasterised glyphs. The cache is keyed by
        // (char, size, face) and never evicts, so without this it grows across
        // every navigation (and every distinct font size) until the heap OOMs.
        // Bounding it to one page's working set costs only a lazy re-rasterise
        // of the visible glyphs on the first paint after a nav.
        self.glyphs.get_mut().clear();
    }

    /// Decode ONE image and store it under `src`. The compressed `bytes` are
    /// borrowed (dropped by the caller right after) — only the decoded pixels
    /// are retained. Over-budget / undecodable → skipped (renders a
    /// placeholder). Returns whether the image was stored.
    pub fn add_image(&mut self, src: &str, bytes: &[u8]) -> bool {
        if let Some(img) = crate::image::decode(bytes) {
            if img.bgra.len() <= self.img_budget {
                self.img_budget -= img.bgra.len();
                self.images.insert(src.into(), alloc::rc::Rc::new(img));
                return true;
            }
        }
        false
    }

    /// Decode + store a whole batch at once (holds all compressed bytes) — kept
    /// for tests / non-streaming callers; the shell uses `images_begin` +
    /// `add_image` to avoid hoarding.
    pub fn set_images(&mut self, pairs: &[(alloc::string::String, Vec<u8>)]) {
        self.images_begin();
        for (src, bytes) in pairs {
            self.add_image(src, bytes);
        }
    }

    /// Parse + lay out a document at `width`. Scroll-independent. Collects the
    /// page's `<style>` blocks into the author stylesheet used by the cascade.
    pub fn layout(&self, html: &str, width: u32) -> Layout {
        self.layout_ext(html, "", width)
    }

    /// Like `layout`, but also applies `external_css` — the concatenated bytes
    /// of the page's `<link rel=stylesheet>` files, which the shell fetches
    /// (the engine is host-free) and passes in. External CSS cascades before
    /// inline `<style>` (document/head order).
    pub fn layout_ext(&self, html: &str, external_css: &str, width: u32) -> Layout {
        self.layout_forms(html, external_css, width, &crate::forms::FormState::default())
    }

    /// Like `layout_ext`, but paints the page's form controls with the user's
    /// live state (typed text, checked boxes, focus + caret). The shell keeps
    /// one `FormState` per page and re-lays out when it changes.
    pub fn layout_forms(
        &self,
        html: &str,
        external_css: &str,
        width: u32,
        forms: &crate::forms::FormState,
    ) -> Layout {
        let dom = crate::dom::parse(html);
        // The cascade also reads the document's own `<style>` blocks and the
        // viewport width (media queries), so both are part of the identity.
        let key = fingerprint(html.as_bytes())
            ^ fingerprint(external_css.as_bytes()).rotate_left(17)
            ^ (width as u64) << 40;
        if self.sheet.borrow().as_ref().map(|(k, _)| *k) != Some(key) {
            *self.sheet.borrow_mut() = Some((key, crate::css::collect_all(&dom, external_css, width as f32)));
        }
        let held = self.sheet.borrow();
        let sheet = &held.as_ref().unwrap().1;
        crate::layout::layout(&self.fonts, &dom, sheet, &self.images, width, self.viewport_h.get(), &self.theme, forms, self.inspect.get())
    }

    /// Lay out with the UA sheet ONLY — no author `<style>`/`<link>` CSS
    /// (reader mode; BROWSER.md §9.7 "never worse than clean content").
    pub fn layout_ua(&self, html: &str, width: u32) -> Layout {
        self.layout_ua_forms(html, width, &crate::forms::FormState::default())
    }

    /// Reader mode with live form state (see `layout_forms`).
    pub fn layout_ua_forms(&self, html: &str, width: u32, forms: &crate::forms::FormState) -> Layout {
        let dom = crate::dom::parse(html);
        crate::layout::layout(
            &self.fonts,
            &dom,
            &crate::css::Stylesheet::empty(),
            &self.images,
            width,
            self.viewport_h.get(),
            &self.theme,
            forms,
            self.inspect.get(),
        )
    }

    /// Paint the slice `[scroll_y, scroll_y + h)` into `out` (must be
    /// `w * h * 4` BGRA bytes).
    pub fn paint(&self, layout: &Layout, w: u32, h: u32, scroll_y: i32, out: &mut [u8]) {
        let (wi, hi) = (w as i32, h as i32);
        // Canvas = the propagated body background (falls back to theme bg).
        fill(out, wi, hi, 0, 0, wi, hi, layout.bg);
        for op in &layout.ops {
            match op {
                DrawOp::Rect { x, y, w: rw, h: rh, color } => {
                    fill(out, wi, hi, *x, *y - scroll_y, *rw, *rh, *color);
                }
                DrawOp::RoundRect { x, y, w: rw, h: rh, r, color, ring } => {
                    fill_round(out, wi, hi, *x, *y - scroll_y, *rw, *rh, *r, *color, *ring);
                }
                DrawOp::Text { x, y, size, color, bold, italic, mono, text } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + (*size as i32) + 6 < 0 {
                        continue; // fully off-screen line → skip
                    }
                    self.draw_run(out, wi, hi, *x, vy, *size, *color, *bold, *italic, *mono, text);
                }
                DrawOp::Image { x, y, w: iw, h: ih, src, alt } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + *ih < 0 {
                        continue;
                    }
                    // Look the pixels up at PAINT time, so an image that
                    // arrives after layout needs only a repaint. A miss (not
                    // fetched yet, or an undecodable format) draws the
                    // placeholder that layout used to emit as separate ops.
                    match self.images.get(src) {
                        Some(img) => blit_image(out, wi, hi, *x, vy, *iw, *ih, img),
                        None => self.draw_img_placeholder(out, wi, hi, *x, vy, *iw, *ih, alt),
                    }
                }
            }
        }
    }

    /// The box an `<img>` shows while its pixels are missing: a thin frame
    /// plus the alt text. Lives here rather than in layout so that an image
    /// arriving later swaps the placeholder for the picture without the
    /// display list changing at all.
    #[allow(clippy::too_many_arguments)]
    fn draw_img_placeholder(
        &self,
        out: &mut [u8],
        wi: i32,
        hi: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        alt: &str,
    ) {
        let c = self.theme.rule;
        fill(out, wi, hi, x, y, w, 1, c);
        fill(out, wi, hi, x, y + h - 1, w, 1, c);
        fill(out, wi, hi, x, y, 1, h, c);
        fill(out, wi, hi, x + w - 1, y, 1, h, c);
        if !alt.is_empty() && w > 24 {
            self.draw_run(out, wi, hi, x + 4, y + 4, 13.0, self.theme.muted, false, false, false, alt);
        }
    }

    /// Draw a run at `(x, y=run-top)` in the face selected by `bold`/`italic`/
    /// `mono` (see `Fonts::pick`) — real weight/slant/monospace, no synthesis.
    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &self,
        out: &mut [u8],
        w: i32,
        h: i32,
        x: i32,
        y: i32,
        size: f32,
        color: Rgb,
        bold: bool,
        italic: bool,
        mono: bool,
        text: &str,
    ) {
        let font = self.fonts.pick(bold, italic, mono);
        let face = Fonts::face_id(bold, italic, mono);
        let ascent = font.horizontal_line_metrics(size).map(|m| m.ascent).unwrap_or(size);
        let baseline = y + ascent as i32;
        let mut pen = x as f32;
        // One borrow for the whole run instead of three per character.
        let mut cache = self.glyphs.borrow_mut();
        for ch in text.chars() {
            let key = (ch as u32, size.to_bits(), face);
            let (m, cov) = cache.entry(key).or_insert_with(|| font.rasterize(ch, size));
            let gx0 = pen as i32 + m.xmin;
            let gy0 = baseline - m.ymin - m.height as i32;
            pen += m.advance_width;
            // Clip the glyph box against the buffer once; the inner loop then
            // walks a row by offset and never re-tests a bound.
            let (cx0, cx1) = (gx0.max(0), (gx0 + m.width as i32).min(w));
            let (cy0, cy1) = (gy0.max(0), (gy0 + m.height as i32).min(h));
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            for py in cy0..cy1 {
                let row = (py - gy0) as usize * m.width + (cx0 - gx0) as usize;
                let mut i = idx(w, cx0, py);
                for gx in 0..(cx1 - cx0) as usize {
                    let a = cov[row + gx];
                    if a != 0 {
                        blend_at(out, i, color, a);
                    }
                    i += 4;
                }
            }
        }
    }
}

#[inline]
fn idx(w: i32, x: i32, y: i32) -> usize {
    ((y * w + x) * 4) as usize
}

/// Fill a rect by building ONE row and copying it, rather than storing four
/// bytes per pixel.
///
/// This is the hottest loop in the app. A frame clears the canvas and then
/// paints roughly another viewport of backgrounds on top, so about 3.7 M pixels
/// are written per scroll step — and under the wasmi interpreter every one of
/// those byte stores is an interpreted instruction with its own bounds check.
/// `copy_within` compiles to `memory.copy`, a single instruction the host
/// executes as a native memmove, so an N-pixel row costs log2(N) copies to
/// build plus one copy per further row instead of 4·N·rows stores.
fn fill(out: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, c: Rgb) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + rw).min(w);
    let y1 = (y + rh).min(h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let row = ((x1 - x0) * 4) as usize;
    let first = idx(w, x0, y0);
    out[first] = c.2; // B
    out[first + 1] = c.1; // G
    out[first + 2] = c.0; // R
    out[first + 3] = 255; // A
    let mut done = 4;
    while done < row {
        let n = done.min(row - done);
        out.copy_within(first..first + n, first + done);
        done += n;
    }
    for py in (y0 + 1)..y1 {
        let dst = idx(w, x0, py);
        out.copy_within(first..first + row, dst);
    }
}

/// How far a rounded rect's left and right edges move inwards on the row whose
/// top is `row_y` (rect-local), in fractional pixels. Radii are `[tl, tr, br,
/// bl]` (CSS corner order) and are treated as circular — CSS allows an ellipse
/// per corner, we take one radius.
fn round_insets(row_y: f32, rh: f32, r: [f32; 4]) -> (f32, f32) {
    let [tl, tr, br, bl] = r;
    let cy = row_y + 0.5; // sample the row's centre
    let inset = |rad: f32, dy: f32| {
        if rad <= 0.0 || dy <= 0.0 {
            0.0
        } else {
            rad - libm::sqrtf((rad * rad - dy * dy).max(0.0))
        }
    };
    let pick = |top: f32, bot: f32| {
        if cy < top {
            inset(top, top - cy)
        } else if cy > rh - bot {
            inset(bot, cy - (rh - bot))
        } else {
            0.0
        }
    };
    (pick(tl, bl), pick(tr, br))
}

/// Fill one row's horizontal span with fractional ends: the interior is a solid
/// run, the two boundary pixels get partial coverage. That antialiasing is what
/// keeps a 2px corner from looking like a chopped pixel.
fn fill_span(out: &mut [u8], w: i32, h: i32, y: i32, xl: f32, xr: f32, c: Rgb) {
    if y < 0 || y >= h || xr <= xl {
        return;
    }
    let (l, rr) = (libm::floorf(xl), libm::ceilf(xr));
    // Solid interior first, then the two fractional edges over it.
    let (i0, i1) = ((l as i32 + 1).max(0), ((rr as i32) - 1).min(w));
    if i1 > i0 {
        fill(out, w, h, i0, y, i1 - i0, 1, c);
    }
    let mut edge = |px: f32, cov: f32| {
        let xi = px as i32;
        if cov > 0.004 && xi >= 0 && xi < w {
            blend_at(out, idx(w, xi, y), c, (cov.min(1.0) * 255.0) as u8);
        }
    };
    // A span narrower than one pixel covers a single pixel partially.
    if rr - l <= 1.0 {
        edge(l, xr - xl);
        return;
    }
    edge(l, 1.0 - (xl - l));
    edge(rr - 1.0, 1.0 - (rr - xr));
}

/// Fill a rounded rect, or — when `ring > 0` — only a border of that thickness
/// along its inside edge. Radii are in px, `[tl, tr, br, bl]`.
///
/// A solid fill only walks rows inside the corner bands; everything between
/// them is ONE `fill` call. So a page-tall background with a 2px radius still
/// costs one `memory.copy` per row instead of a per-pixel loop over millions
/// of pixels.
#[allow(clippy::too_many_arguments)]
fn fill_round(out: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, r: [f32; 4], c: Rgb, ring: f32) {
    if rw <= 0 || rh <= 0 {
        return;
    }
    if r.iter().all(|&v| v <= 0.0) && ring <= 0.0 {
        fill(out, w, h, x, y, rw, rh, c);
        return;
    }
    let (fx, fy, fw, fh) = (x as f32, y as f32, rw as f32, rh as f32);
    // A radius may not exceed half the box, and CSS scales ALL of them by one
    // factor when any pair overflows its side (css-backgrounds-3 §5.5) —
    // clamping each corner on its own would change the shape.
    let mut scale = 1.0f32;
    for (sum, extent) in [(r[0] + r[1], fw), (r[3] + r[2], fw), (r[0] + r[3], fh), (r[1] + r[2], fh)] {
        if sum > extent && sum > 0.0 {
            scale = scale.min(extent / sum);
        }
    }
    let r = [r[0] * scale, r[1] * scale, r[2] * scale, r[3] * scale];
    let span = |out: &mut [u8], py: i32| {
        let (li, ri) = round_insets(py as f32 - fy, fh, r);
        fill_span(out, w, h, py, fx + li, fx + fw - ri, c);
    };

    if ring <= 0.0 {
        let top = ceil_f(r[0].max(r[1])).min(rh);
        let bot = ceil_f(r[2].max(r[3])).min(rh - top);
        for py in y..(y + top) {
            span(out, py);
        }
        fill(out, w, h, x, y + top, rw, rh - top - bot, c);
        for py in (y + rh - bot)..(y + rh) {
            span(out, py);
        }
        return;
    }

    // Ring: the hole's radii shrink with the border but never go negative — a
    // border thicker than the radius leaves a square inner corner, as browsers
    // do. Rows above and below the hole are border across their whole span.
    let inner = [
        (r[0] - ring).max(0.0),
        (r[1] - ring).max(0.0),
        (r[2] - ring).max(0.0),
        (r[3] - ring).max(0.0),
    ];
    let (iy0, iy1, ih) = (fy + ring, fy + fh - ring, fh - 2.0 * ring);
    for py in y.max(0)..(y + rh).min(h) {
        let cy = py as f32 + 0.5;
        if cy < iy0 || cy > iy1 || ih <= 0.0 {
            span(out, py);
            continue;
        }
        let (li, ri) = round_insets(py as f32 - fy, fh, r);
        let (ili, iri) = round_insets(cy - iy0 - 0.5, ih, inner);
        fill_span(out, w, h, py, fx + li, fx + ring + ili, c);
        fill_span(out, w, h, py, fx + fw - ring - iri, fx + fw - ri, c);
    }
}

/// `ceil` as an i32 — `core` has no `f32::ceil` in `no_std`.
fn ceil_f(v: f32) -> i32 {
    libm::ceilf(v.max(0.0)) as i32
}

/// Blend `c` at `a`/255 coverage over the pixel starting at byte `i`. Takes the
/// offset rather than (x, y) so the caller can walk a row by adding 4 instead of
/// recomputing `y * w + x` for every pixel it touches.
#[inline]
fn blend_at(out: &mut [u8], i: usize, c: Rgb, a: u8) {
    if a == 255 {
        out[i] = c.2;
        out[i + 1] = c.1;
        out[i + 2] = c.0;
        out[i + 3] = 255;
        return;
    }
    let a = a as u32;
    let ia = 255 - a;
    out[i] = ((c.2 as u32 * a + out[i] as u32 * ia) / 255) as u8; // B
    out[i + 1] = ((c.1 as u32 * a + out[i + 1] as u32 * ia) / 255) as u8; // G
    out[i + 2] = ((c.0 as u32 * a + out[i + 2] as u32 * ia) / 255) as u8; // R
    out[i + 3] = 255;
}

/// Nearest-neighbour scale a decoded `img` (BGRA) into a `dw`×`dh` box at
/// (dx, dy), alpha-blending over `out`. Clipped to the buffer.
fn blit_image(out: &mut [u8], w: i32, h: i32, dx: i32, dy: i32, dw: i32, dh: i32, img: &crate::image::Image) {
    if dw <= 0 || dh <= 0 || img.w == 0 || img.h == 0 {
        return;
    }
    let (iw, ih) = (img.w as i32, img.h as i32);
    let x0 = dx.max(0);
    let x1 = (dx + dw).min(w);
    let y0 = dy.max(0);
    let y1 = (dy + dh).min(h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // The source column for each destination column, resolved once for the
    // whole blit instead of a multiply, divide and clamp per pixel.
    let cols: Vec<usize> = (x0..x1).map(|px| ((px - dx) * iw / dw).clamp(0, iw - 1) as usize * 4).collect();
    for py in y0..y1 {
        let sy = ((py - dy) * ih / dh).clamp(0, ih - 1);
        let srow = (sy * iw) as usize * 4;
        let mut di = idx(w, x0, py);
        for &sx in &cols {
            let si = srow + sx;
            let a = img.bgra[si + 3];
            if a == 255 {
                out[di..di + 4].copy_from_slice(&img.bgra[si..si + 4]);
            } else if a != 0 {
                let (a, ia) = (a as u32, 255 - a as u32);
                out[di] = ((img.bgra[si] as u32 * a + out[di] as u32 * ia) / 255) as u8;
                out[di + 1] = ((img.bgra[si + 1] as u32 * a + out[di + 1] as u32 * ia) / 255) as u8;
                out[di + 2] = ((img.bgra[si + 2] as u32 * a + out[di + 2] as u32 * ia) / 255) as u8;
                out[di + 3] = 255;
            }
            di += 4;
        }
    }
}
