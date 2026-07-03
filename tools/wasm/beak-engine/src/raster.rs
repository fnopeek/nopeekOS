//! Rasteriser — draws a `Layout`'s display list into a BGRA pixel buffer.
//!
//! Glyphs come from fontdue (Inter, the same font the compositor uses); the
//! layout is ours. `paint` renders the visible slice at a scroll offset, so
//! the buffer stays viewport-sized regardless of document length. Pure — the
//! same code paints on nopeekOS (into a `Widget::Canvas`) and on the desktop
//! adapter (into a window framebuffer), see BROWSER.md §10.

use alloc::vec::Vec;
use core::cell::RefCell;
use fontdue::{Font, FontSettings, Metrics};
use hashbrown::HashMap;

use crate::layout::{DrawOp, Layout, Rgb, Theme};

static FONT_BYTES: &[u8] = include_bytes!("../assets/inter.ttf");

pub struct Engine {
    font: Font,
    /// Rasterised-glyph cache keyed by (char, size-bits). fontdue's rasterise
    /// is not free; without this every glyph is re-rasterised every frame,
    /// which makes scrolling lag. Bounded by the glyph set the page uses.
    glyphs: RefCell<HashMap<(u32, u32), (Metrics, Vec<u8>)>>,
    /// Page colours (theme-resolved by the shell; dark until then).
    theme: Theme,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Parse the embedded Inter font. Cheap enough to build once and reuse
    /// across page loads (the shell keeps one `Engine`).
    pub fn new() -> Engine {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default())
            .expect("embedded inter.ttf is a valid TrueType font");
        Engine {
            font,
            glyphs: RefCell::new(HashMap::new()),
            theme: Theme::DARK,
        }
    }

    /// Set the page colours (the shell resolves these from the compositor
    /// palette so the page follows light/dark).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Parse + lay out a document at `width`. Scroll-independent.
    pub fn layout(&self, html: &str, width: u32) -> Layout {
        let dom = crate::dom::parse(html);
        crate::layout::layout(&self.font, &dom, width, &self.theme)
    }

    /// Paint the slice `[scroll_y, scroll_y + h)` into `out` (must be
    /// `w * h * 4` BGRA bytes).
    pub fn paint(&self, layout: &Layout, w: u32, h: u32, scroll_y: i32, out: &mut [u8]) {
        let (wi, hi) = (w as i32, h as i32);
        fill(out, wi, hi, 0, 0, wi, hi, self.theme.bg);
        for op in &layout.ops {
            match op {
                DrawOp::Rect { x, y, w: rw, h: rh, color } => {
                    fill(out, wi, hi, *x, *y - scroll_y, *rw, *rh, *color);
                }
                DrawOp::Text { x, y, size, color, bold, italic, text } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + (*size as i32) + 6 < 0 {
                        continue; // fully off-screen line → skip
                    }
                    self.draw_run(out, wi, hi, *x, vy, *size, *color, *bold, *italic, text);
                }
            }
        }
    }

    /// Draw a run at `(x, y=run-top)`. `bold`/`italic` have no dedicated font
    /// face (single Inter), so they're synthesised: **bold** = a 1px horizontal
    /// smear (double-blend at px+1); *italic* = a faux slant sheared around the
    /// baseline. Good enough to distinguish `<b>`/`<i>`; a real bold/italic
    /// face is a later font-loading step.
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
        text: &str,
    ) {
        let ascent = self
            .font
            .horizontal_line_metrics(size)
            .map(|m| m.ascent)
            .unwrap_or(size);
        let baseline = y + ascent as i32;
        let mut pen = x as f32;
        for ch in text.chars() {
            let key = (ch as u32, size.to_bits());
            if !self.glyphs.borrow().contains_key(&key) {
                let g = self.font.rasterize(ch, size);
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
                let shear = if italic { (((baseline - py) as f32) * 0.21) as i32 } else { 0 };
                let row = gy * m.width;
                for gx in 0..m.width {
                    let a = cov[row + gx];
                    if a == 0 {
                        continue;
                    }
                    let px = gx0 + gx as i32 + shear;
                    if px >= 0 && px < w {
                        blend(out, w, px, py, color, a);
                    }
                    if bold {
                        let pxb = px + 1;
                        if pxb >= 0 && pxb < w {
                            blend(out, w, pxb, py, color, a);
                        }
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
