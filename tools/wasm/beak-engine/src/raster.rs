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
        }
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
        let sheet = crate::css::collect_all(&dom, external_css);
        crate::layout::layout(&self.fonts, &dom, &sheet, &self.images, width, &self.theme, forms)
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
            &self.theme,
            forms,
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
                DrawOp::Text { x, y, size, color, bold, italic, mono, text } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + (*size as i32) + 6 < 0 {
                        continue; // fully off-screen line → skip
                    }
                    self.draw_run(out, wi, hi, *x, vy, *size, *color, *bold, *italic, *mono, text);
                }
                DrawOp::Image { x, y, w: iw, h: ih, img } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + *ih < 0 {
                        continue;
                    }
                    blit_image(out, wi, hi, *x, vy, *iw, *ih, img);
                }
            }
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
        for ch in text.chars() {
            let key = (ch as u32, size.to_bits(), face);
            if !self.glyphs.borrow().contains_key(&key) {
                let g = font.rasterize(ch, size);
                self.glyphs.borrow_mut().insert(key, g);
            }
            let cache = self.glyphs.borrow();
            let (m, cov) = cache.get(&key).unwrap();
            let gx0 = pen as i32 + m.xmin;
            let gy0 = baseline - m.ymin - m.height as i32;
            for gy in 0..m.height {
                let py = gy0 + gy as i32;
                if py < 0 || py >= h {
                    continue;
                }
                let row = gy * m.width;
                for gx in 0..m.width {
                    let a = cov[row + gx];
                    if a == 0 {
                        continue;
                    }
                    let px = gx0 + gx as i32;
                    if px >= 0 && px < w {
                        blend(out, w, px, py, color, a);
                    }
                }
            }
            pen += m.advance_width;
        }
    }
}

#[inline]
fn idx(w: i32, x: i32, y: i32) -> usize {
    ((y * w + x) * 4) as usize
}

fn fill(out: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, c: Rgb) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + rw).min(w);
    let y1 = (y + rh).min(h);
    for py in y0..y1 {
        for px in x0..x1 {
            let i = idx(w, px, py);
            out[i] = c.2; // B
            out[i + 1] = c.1; // G
            out[i + 2] = c.0; // R
            out[i + 3] = 255; // A
        }
    }
}

fn blend(out: &mut [u8], w: i32, x: i32, y: i32, c: Rgb, a: u8) {
    let i = idx(w, x, y);
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
    for ry in 0..dh {
        let py = dy + ry;
        if py < 0 || py >= h {
            continue;
        }
        let sy = (ry * ih / dh).clamp(0, ih - 1);
        for rx in 0..dw {
            let px = dx + rx;
            if px < 0 || px >= w {
                continue;
            }
            let sx = (rx * iw / dw).clamp(0, iw - 1);
            let si = ((sy * iw + sx) * 4) as usize;
            let a = img.bgra[si + 3] as u32;
            if a == 0 {
                continue;
            }
            let di = idx(w, px, py);
            if a == 255 {
                out[di] = img.bgra[si];
                out[di + 1] = img.bgra[si + 1];
                out[di + 2] = img.bgra[si + 2];
            } else {
                let ia = 255 - a;
                out[di] = ((img.bgra[si] as u32 * a + out[di] as u32 * ia) / 255) as u8;
                out[di + 1] = ((img.bgra[si + 1] as u32 * a + out[di + 1] as u32 * ia) / 255) as u8;
                out[di + 2] = ((img.bgra[si + 2] as u32 * a + out[di + 2] as u32 * ia) / 255) as u8;
            }
            out[di + 3] = 255;
        }
    }
}
