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
use crate::style::{BgPos, BgSize};

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
    /// Decoded CSS images (`background-image`/`mask-image`) keyed by
    /// `css::url_key`. Separate from `images` so a page's `<img src="x">` and
    /// a stylesheet's `url(x)` cannot collide, and because these are resolved
    /// on a different clock: `data:` URIs decode during layout, fetched ones
    /// arrive later from the shell.
    css_images: RefCell<HashMap<u64, alloc::rc::Rc<crate::image::Image>>>,
    /// Decoded-BGRA budget for CSS images, separate from `img_budget` so a
    /// page full of icons cannot starve its `<img>`s (or the reverse).
    css_img_budget: core::cell::Cell<usize>,
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
            css_images: RefCell::new(HashMap::new()),
            css_img_budget: core::cell::Cell::new(crate::image::CSS_BUDGET),
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
        // The theme is part of the identity too: `prefers-color-scheme` decides
        // which rules apply, and `resolve_vars` BAKES the winning custom
        // properties into the text it hands on — so a light and a dark sheet
        // are different documents, not the same one read differently.
        let media = crate::css::Media::new(width as f32, self.theme.is_dark());
        let key = fingerprint(html.as_bytes())
            ^ fingerprint(external_css.as_bytes()).rotate_left(17)
            ^ (width as u64) << 40
            ^ (media.dark as u64) << 63;
        if self.sheet.borrow().as_ref().map(|(k, _)| *k) != Some(key) {
            *self.sheet.borrow_mut() = Some((key, crate::css::collect_all(&dom, external_css, media)));
        }
        let held = self.sheet.borrow();
        let sheet = &held.as_ref().unwrap().1;
        let mut lay = crate::layout::layout(&self.fonts, &dom, sheet, &self.images, width, self.viewport_h.get(), &self.theme, forms, self.inspect.get());
        self.resolve_css_images(sheet, &mut lay);
        lay
    }

    /// Turn the CSS image keys a layout needs back into URLs.
    ///
    /// A `data:` URI carries its own bytes, so the engine decodes it here and
    /// the shell never hears about it; everything else is reported in
    /// `css_image_srcs` for the shell to fetch and hand back via
    /// `add_css_image`. Already-decoded keys are skipped, so this stays cheap
    /// across the several layouts one page runs through.
    fn resolve_css_images(&self, sheet: &crate::css::Stylesheet, lay: &mut Layout) {
        for &key in &lay.css_image_keys {
            if self.css_images.borrow().contains_key(&key) {
                continue;
            }
            let Some(url) = sheet.url(key) else { continue };
            if url.starts_with("data:") || url.starts_with("DATA:") {
                if let Some(bytes) = crate::image::decode_data_uri(url) {
                    self.store_css_image(key, &bytes);
                }
            } else {
                lay.css_image_srcs.push((key, alloc::string::String::from(url)));
            }
        }
    }

    fn store_css_image(&self, key: u64, bytes: &[u8]) -> bool {
        if let Some(img) = crate::image::decode(bytes) {
            let budget = self.css_img_budget.get();
            if img.bgra.len() <= budget {
                self.css_img_budget.set(budget - img.bgra.len());
                self.css_images.borrow_mut().insert(key, alloc::rc::Rc::new(img));
                return true;
            }
        }
        false
    }

    /// Store a CSS image the shell fetched (see `Layout::css_image_srcs`).
    /// Costs a repaint, never a re-layout: a background cannot move a box.
    pub fn add_css_image(&self, key: u64, bytes: &[u8]) -> bool {
        self.store_css_image(key, bytes)
    }

    /// Drop the previous page's CSS images (called on navigation).
    pub fn css_images_begin(&self) {
        self.css_images.borrow_mut().clear();
        self.css_img_budget.set(crate::image::CSS_BUDGET);
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
                DrawOp::BgImage { x, y, w: bw, h: bh, key, repeat, pos, size, tint } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + *bh < 0 {
                        continue;
                    }
                    // A missing background draws NOTHING — unlike `<img>`,
                    // there is no placeholder for one: the box is styled and
                    // sized either way, so an absent decoration must simply be
                    // absent rather than a grey frame over the content.
                    if let Some(img) = self.css_images.borrow().get(key) {
                        blit_bg(out, wi, hi, *x, vy, *bw, *bh, img, *repeat, *pos, *size, *tint);
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
/// Resolve `background-size` against the positioning area (css-backgrounds-3
/// §3.9). `auto` on one axis keeps the intrinsic aspect ratio.
fn bg_tile_size(area: (i32, i32), img: (u32, u32), size: BgSize) -> (i32, i32) {
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    let (iw, ih) = (img.0 as f32, img.1 as f32);
    let ratio = iw / ih;
    let (tw, th) = match size {
        BgSize::Auto => (iw, ih),
        BgSize::Cover | BgSize::Contain => {
            let s = if matches!(size, BgSize::Cover) {
                (aw / iw).max(ah / ih)
            } else {
                (aw / iw).min(ah / ih)
            };
            (iw * s, ih * s)
        }
        BgSize::Fixed(fw, fh) => {
            let rw = fw.and_then(|l| l.px(aw));
            let rh = fh.and_then(|l| l.px(ah));
            match (rw, rh) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (a, a / ratio),
                (None, Some(b)) => (b * ratio, b),
                (None, None) => (iw, ih),
            }
        }
    };
    ((libm::roundf(tw) as i32).max(1), (libm::roundf(th) as i32).max(1))
}

fn bg_offset(p: BgPos, area: i32, tile: i32) -> i32 {
    match p {
        BgPos::Px(v) => libm::roundf(v) as i32,
        BgPos::Pct(f) => libm::roundf((area - tile) as f32 * f) as i32,
    }
}

/// Paint one `background-image`/`mask-image` layer into the box `dx,dy,dw,dh`.
/// Everything is clipped to that box; repeating axes tile outward from the
/// positioned origin.
#[allow(clippy::too_many_arguments)]
fn blit_bg(
    out: &mut [u8],
    w: i32,
    h: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    img: &crate::image::Image,
    repeat: (bool, bool),
    pos: (BgPos, BgPos),
    size: BgSize,
    tint: Option<crate::layout::Rgb>,
) {
    if dw <= 0 || dh <= 0 || img.w == 0 || img.h == 0 {
        return;
    }
    let (tw, th) = bg_tile_size((dw, dh), (img.w, img.h), size);
    let ox = dx + bg_offset(pos.0, dw, tw);
    let oy = dy + bg_offset(pos.1, dh, th);
    // Tile range: how many steps back from the origin before leaving the box,
    // and how many forward. A non-repeating axis is the single tile.
    let span = |origin: i32, box_lo: i32, box_hi: i32, tile: i32, rep: bool| -> (i32, i32) {
        if !rep {
            return (0, 0);
        }
        let lo = (box_lo - origin).div_euclid(tile).min(0);
        let hi = (box_hi - origin - 1).div_euclid(tile).max(0);
        (lo, hi)
    };
    let (ix0, ix1) = span(ox, dx, dx + dw, tw, repeat.0);
    let (iy0, iy1) = span(oy, dy, dy + dh, th, repeat.1);
    // Clip to the box AND to the surface in one rect, so the inner loop never
    // tests bounds per pixel.
    let (cx0, cx1) = (dx.max(0), (dx + dw).min(w));
    let (cy0, cy1) = (dy.max(0), (dy + dh).min(h));
    if cx1 <= cx0 || cy1 <= cy0 {
        return;
    }
    for ty in iy0..=iy1 {
        let ty0 = oy + ty * th;
        let (y0, y1) = (ty0.max(cy0), (ty0 + th).min(cy1));
        if y1 <= y0 {
            continue;
        }
        for tx in ix0..=ix1 {
            let tx0 = ox + tx * tw;
            let (x0, x1) = (tx0.max(cx0), (tx0 + tw).min(cx1));
            if x1 <= x0 {
                continue;
            }
            // Source column per destination column, resolved once per tile
            // rather than a multiply+divide per pixel (the interpreter charges
            // ~150× for a per-pixel loop — see the wasmi hot-loop note).
            let cols: Vec<usize> = (x0..x1)
                .map(|px| ((px - tx0) * img.w as i32 / tw).clamp(0, img.w as i32 - 1) as usize * 4)
                .collect();
            for py in y0..y1 {
                let sy = ((py - ty0) * img.h as i32 / th).clamp(0, img.h as i32 - 1);
                let srow = (sy * img.w as i32) as usize * 4;
                let mut di = idx(w, x0, py);
                for &sx in &cols {
                    let si = srow + sx;
                    let a = img.bgra[si + 3] as u32;
                    if a != 0 {
                        // A mask takes only the alpha and paints the tint
                        // through it; a background image paints its own pixels.
                        let src = match tint {
                            Some(c) => [c.2, c.1, c.0],
                            None => [img.bgra[si], img.bgra[si + 1], img.bgra[si + 2]],
                        };
                        if a == 255 {
                            out[di] = src[0];
                            out[di + 1] = src[1];
                            out[di + 2] = src[2];
                            out[di + 3] = 255;
                        } else {
                            let ia = 255 - a;
                            for c in 0..3 {
                                out[di + c] = ((src[c] as u32 * a + out[di + c] as u32 * ia) / 255) as u8;
                            }
                            out[di + 3] = 255;
                        }
                    }
                    di += 4;
                }
            }
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Rgb, Theme};

    /// An inline SVG `data:` URI, quote-safe: the SVG's own attribute quotes
    /// are percent-encoded, so the URI survives being nested inside a CSS
    /// string inside an HTML attribute (which is how real pages ship them).
    const MASK_LEFT_HALF: &str = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22\
        %20width=%2220%22%20height=%2220%22%20viewBox=%220%200%2020%2020%22%3E\
        %3Cpath%20d=%22M0%200%20H10%20V20%20H0%20Z%22%20fill=%22%23000%22/%3E%3C/svg%3E";

    fn light() -> Theme {
        Theme {
            bg: Rgb(255, 255, 255),
            text: Rgb(0, 0, 0),
            heading: Rgb(0, 0, 0),
            link: Rgb(0, 0, 238),
            muted: Rgb(96, 96, 96),
            rule: Rgb(128, 128, 128),
        }
    }

    const PAD: u32 = 20; // layout::PAD — the fixed page gutter

    /// Paint one page and read a pixel back as (r, g, b). `x`/`y` are relative
    /// to the document's top-left content corner, i.e. past the page padding.
    fn pixel_at(html: &str, x: u32, y: u32) -> (u8, u8, u8) {
        page(html, 40, 40)(x, y)
    }

    /// `pixel_at` over a content box of a given size — inline content needs a
    /// line's worth of width. Returns a reader so one paint answers many probes.
    fn page(html: &str, cw: u32, ch: u32) -> impl Fn(u32, u32) -> (u8, u8, u8) {
        let (w, h) = (PAD * 2 + cw, PAD * 2 + ch);
        let mut eng = Engine::new();
        eng.set_theme(light());
        let lay = eng.layout(html, w);
        let mut buf = alloc::vec![0u8; (w * h * 4) as usize];
        eng.paint(&lay, w, h, 0, &mut buf);
        move |x: u32, y: u32| {
            let i = (((y + PAD) * w + x + PAD) * 4) as usize;
            (buf[i + 2], buf[i + 1], buf[i])
        }
    }

    /// A mask paints the element's own background-colour through the image's
    /// alpha — it does NOT paint the image. This SVG is opaque on its left half
    /// only, so the box must be red on the left and untouched on the right.
    #[test]
    fn mask_image_stencils_the_background_colour() {
        // A `data:` URI needs no fetch: the engine decodes it during layout.
        let html = alloc::format!(
            "<div style=\"width:20px;height:20px;background-color:#ff0000;\
             mask-image:url('{MASK_LEFT_HALF}');mask-size:contain;mask-repeat:no-repeat\"></div>"
        );
        assert_eq!(pixel_at(&html, 4, 10), (255, 0, 0), "left half is stencilled red");
        assert_eq!(pixel_at(&html, 16, 10), (255, 255, 255), "right half stays clear");
    }

    /// Without a mask the same box is a plain filled rect — the guard that the
    /// mask path is what changed, not background painting in general.
    #[test]
    fn a_plain_background_colour_still_fills_the_whole_box() {
        let html = "<div style='width:20px;height:20px;background-color:#ff0000'></div>";
        assert_eq!(pixel_at(html, 4, 10), (255, 0, 0));
        assert_eq!(pixel_at(html, 16, 10), (255, 0, 0));
    }

    /// `no-repeat` must leave the rest of the box alone, and the tile must sit
    /// where `background-position` puts it.
    #[test]
    fn background_image_honours_no_repeat_and_position() {
        let svg = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22\
                   %20width=%224%22%20height=%224%22%20viewBox=%220%200%204%204%22%3E\
                   %3Cpath%20d=%22M0%200%20H4%20V4%20H0%20Z%22%20fill=%22%230000ff%22/%3E%3C/svg%3E";
        let html = alloc::format!(
            "<div style=\"width:20px;height:20px;background-image:url('{svg}');\
             background-repeat:no-repeat;background-position:right top\"></div>"
        );
        assert_eq!(pixel_at(&html, 18, 2), (0, 0, 255), "tile sits at the right edge");
        assert_eq!(pixel_at(&html, 2, 2), (255, 255, 255), "and nowhere else");
    }

    /// The spelling MediaWiki actually ships: a double-quoted `url()` whose
    /// payload carries BACKSLASH-ESCAPED quotes. Stopping at the first inner
    /// quote truncates the URI into something that still parses as a URL and
    /// then silently decodes to nothing — so this is a paint test, not a
    /// parse test.
    #[test]
    fn a_data_uri_with_escaped_quotes_still_paints() {
        let svg = "data:image/svg+xml;utf8,<svg xmlns=\\\"http://www.w3.org/2000/svg\\\" \
                   width=\\\"20\\\" height=\\\"20\\\" viewBox=\\\"0 0 20 20\\\">\
                   <path d=\\\"M0 0 H10 V20 H0 Z\\\" fill=\\\"%23000\\\"/></svg>";
        let html = alloc::format!(
            "<style>div{{width:20px;height:20px;background-color:#ff0000;\
             mask-image:url(\"{svg}\");mask-size:contain;mask-repeat:no-repeat}}</style><div></div>"
        );
        assert_eq!(pixel_at(&html, 4, 10), (255, 0, 0), "left half is stencilled red");
        assert_eq!(pixel_at(&html, 16, 10), (255, 255, 255), "right half stays clear");
    }

    /// An inline box has no block geometry — only the fragments it leaves in
    /// line boxes. It still paints a background over them, and its horizontal
    /// padding is part of that background AND advances the text after it.
    #[test]
    fn an_inline_box_paints_its_own_background() {
        let at = page("<span style='background-color:#ff0000;padding-left:10px'>l</span>", 120, 40);
        assert_eq!(at(2, 8), (255, 0, 0), "the left padding is background too");
        assert_eq!(at(2, 34), (255, 255, 255), "and stops below the box");
    }

    /// The `a.external` shape: the icon lives in the padding the box reserves
    /// past its text, so nothing shows unless BOTH the padding advances the
    /// flow and the fragment paints its background image.
    #[test]
    fn an_inline_background_image_lands_in_the_padding() {
        let svg = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22\
                   %20width=%224%22%20height=%224%22%20viewBox=%220%200%204%204%22%3E\
                   %3Cpath%20d=%22M0%200%20H4%20V4%20H0%20Z%22%20fill=%22%230000ff%22/%3E%3C/svg%3E";
        let html = alloc::format!(
            "<span style=\"padding-left:10px;background-image:url('{svg}');\
             background-repeat:no-repeat;background-position:left top\">l</span>"
        );
        let at = page(&html, 120, 40);
        assert_eq!(at(1, 1), (0, 0, 255), "the tile sits in the box's own padding");
        assert_eq!(at(8, 8), (255, 255, 255), "no-repeat leaves the rest alone");
    }

    /// A box that only wraps an icon has no text at all. Its padding still
    /// keeps the line box alive (CSS 2.1 §9.4.2) — otherwise the whole
    /// `.vector-icon` pattern paints nothing.
    #[test]
    fn an_empty_inline_box_still_gets_a_line_to_paint_on() {
        let at = page("<span style='background-color:#ff0000;padding-left:12px'></span>", 120, 40);
        assert_eq!(at(4, 8), (255, 0, 0));
    }

    /// Broken over two lines, an inline box leaves one rectangle per line —
    /// and only the first carries its left border (`box-decoration-break:
    /// slice`, the default).
    #[test]
    fn an_inline_box_broken_over_two_lines_paints_both_fragments() {
        let at = page(
            "<span style='background-color:#ff0000;border-left:4px solid #0000ff'>llllllll llllllll</span>",
            40,
            60,
        );
        assert_eq!(at(1, 1), (0, 0, 255), "the left border opens the first fragment");
        assert_eq!(at(6, 1), (255, 0, 0), "which the background follows");
        assert_eq!(at(6, 21), (255, 0, 0), "the second line carries the background too");
        assert_eq!(at(1, 21), (255, 0, 0), "but not the left border a second time");
    }
}
