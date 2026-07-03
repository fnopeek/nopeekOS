//! layout.rs — hand-rolled block + inline flow over the styled DOM.
//!
//! Walks the DOM (dom.rs) resolving each element's `ComputedStyle` (style.rs)
//! and turns it into a positioned **display list** (`DrawOp`s) + link hit-rects
//! + a total height. Two formatting contexts, per CSS2.1 §9:
//!
//! * **Block** — block-level children stack vertically; adjacent vertical
//!   margins collapse (the common case).
//! * **Inline** — runs of text + inline elements (`<a>`, `<b>`, `<code>`, …)
//!   flow into **line boxes**: greedy word-wrap to the content width, mixed
//!   sizes/colours/weights on one line sharing a baseline. This is what puts
//!   nav links *inline* with their text instead of each on its own line.
//!
//! Scroll-independent: computed once per (content, width); `raster::paint`
//! draws the visible slice at any offset. Flex/Grid/floats/position come next.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use fontdue::Font;

use crate::css::{ElemInfo, Stylesheet};
use crate::dom::{Dom, Element, Node};
use crate::style::{
    self, ComputedStyle, CrossAlign, Display, FlexBasis, GridTrack, Justify, Len, BASE_FONT_PX,
};

/// Resolve a block box's horizontal geometry within a containing block of
/// content width `avail`: CSS2.1 §10.3.3 (used width + margins) plus the
/// §10.4 min/max-width redo. Returns (content-width, content-left-offset =
/// margin-left + padding-left).
fn resolve_block_h(st: &ComputedStyle, avail: f32) -> (f32, f32) {
    let pad = st.pad_left + st.pad_right;
    let (mut cw, mut ml) = solve_h(st.width, st.margin_left, st.margin_right, avail, pad, st.box_border);

    if let Some(maxw) = st.max_width.px(avail) {
        let maxc = if st.box_border { (maxw - pad).max(0.0) } else { maxw };
        if cw > maxc {
            let redo = if st.box_border { maxw } else { maxc };
            (cw, ml) = solve_h(Len::Px(redo), st.margin_left, st.margin_right, avail, pad, st.box_border);
        }
    }
    if let Some(minw) = st.min_width.px(avail) {
        let minc = if st.box_border { (minw - pad).max(0.0) } else { minw };
        if cw < minc {
            let redo = if st.box_border { minw } else { minc };
            (cw, ml) = solve_h(Len::Px(redo), st.margin_left, st.margin_right, avail, pad, st.box_border);
        }
    }
    (cw.max(1.0), ml + st.pad_left)
}

/// Solve used content-width + left margin for one width value. Auto width fills
/// (auto margins → 0); a definite width lets auto margins center / take slack.
fn solve_h(width: Len, ml: Len, mr: Len, avail: f32, pad: f32, border_box: bool) -> (f32, f32) {
    match width.px(avail) {
        None => {
            let ml = ml.px(avail).unwrap_or(0.0);
            let mr = mr.px(avail).unwrap_or(0.0);
            ((avail - ml - mr - pad).max(0.0), ml)
        }
        Some(wv) => {
            let cw = if border_box { (wv - pad).max(0.0) } else { wv };
            let rest = avail - cw - pad;
            let ml_final = match (ml.px(avail), mr.px(avail)) {
                (None, None) => (rest / 2.0).max(0.0), // margin:0 auto → center
                (None, Some(mr)) => (rest - mr).max(0.0),
                (Some(ml), _) => ml,
            };
            (cw, ml_final)
        }
    }
}

/// 8-bit RGB. The rasteriser converts to the buffer's BGRA at blit time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Resolved page colours for the active theme. The shell fills these from the
/// compositor palette (npk_theme_token) so the page follows light/dark like
/// the rest of the UI; `DARK` is the fallback before the query.
#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Rgb,
    pub text: Rgb,
    pub heading: Rgb,
    pub link: Rgb,
    pub muted: Rgb,
    pub rule: Rgb,
}
impl Theme {
    pub const DARK: Theme = Theme {
        bg: Rgb(24, 24, 28),
        text: Rgb(212, 212, 216),
        heading: Rgb(245, 245, 248),
        link: Rgb(96, 165, 250),
        muted: Rgb(148, 148, 154),
        rule: Rgb(58, 58, 64),
    };
}

const PAD: i32 = 20;

/// One paint instruction, positioned in document space (pre-scroll).
pub enum DrawOp {
    /// A run of already-wrapped, same-style text; `y` is the run's top.
    Text { x: i32, y: i32, size: f32, color: Rgb, bold: bool, italic: bool, text: String },
    /// A filled rectangle (divider, list bullet).
    Rect { x: i32, y: i32, w: i32, h: i32, color: Rgb },
}

/// A clickable link's document-space rectangle.
pub struct LinkRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub href: String,
}

pub struct Layout {
    pub ops: Vec<DrawOp>,
    pub links: Vec<LinkRect>,
    /// Total document height (px). May exceed the viewport → scroll.
    pub height: u32,
}

impl Layout {
    /// Link href at a document-space point (caller adds scroll to screen y).
    pub fn hit_test(&self, x: i32, y: i32) -> Option<&str> {
        // Reverse so a link painted later (on top) wins an overlap.
        self.links
            .iter()
            .rev()
            .find(|l| x >= l.x && x < l.x + l.w && y >= l.y && y < l.y + l.h)
            .map(|l| l.href.as_str())
    }
}

// ── small font helpers (no_std: no f32::ceil) ──────────────────────────────

fn ceil_i32(x: f32) -> i32 {
    let c = x as i32;
    if (c as f32) < x { c + 1 } else { c }
}
fn measure(font: &Font, s: &str, size: f32) -> f32 {
    s.chars().map(|c| font.metrics(c, size).advance_width).sum()
}
fn space_width(font: &Font, size: f32) -> f32 {
    font.metrics(' ', size).advance_width
}
fn ascent_i(font: &Font, size: f32) -> i32 {
    font.horizontal_line_metrics(size).map(|m| m.ascent).unwrap_or(size) as i32
}
fn line_gap(font: &Font, size: f32) -> f32 {
    font.horizontal_line_metrics(size).map(|m| m.new_line_size).unwrap_or(size * 1.3)
}

// ── entry point + block/inline tree walk ───────────────────────────────────

/// Per-layout mutable context: the shared inputs (font / theme / author sheet)
/// plus the accumulating display list and the live ancestor `path` (for
/// selector matching). Bundling these keeps the recursive walkers from carrying
/// a dozen arguments each.
struct Ctx<'a> {
    font: &'a Font,
    theme: &'a Theme,
    sheet: &'a Stylesheet,
    ops: Vec<DrawOp>,
    links: Vec<LinkRect>,
    path: Vec<ElemInfo>, // root → … → current parent
}

/// Lay a document out into a scroll-independent display list.
pub fn layout(font: &Font, dom: &Dom, sheet: &Stylesheet, width: u32, theme: &Theme) -> Layout {
    let root = ComputedStyle::root(theme);
    let mut ctx = Ctx { font, theme, sheet, ops: Vec::new(), links: Vec::new(), path: Vec::new() };

    let cx = PAD;
    let cw = (width as i32 - 2 * PAD).max(60);
    let mut y = PAD;

    // Resolve <body> itself so `body { … }` rules inherit into the page, and
    // put it on the ancestor path so `body p` / `.article p` selectors match.
    let body = dom.body();
    let body_style = style::resolve(body, &root, theme, sheet, &[]);
    ctx.path.push(ElemInfo::of(body));
    y = ctx.layout_children(&body.children, &body_style, cx, cw, y);
    y += PAD;

    Layout { ops: ctx.ops, links: ctx.links, height: y.max(1) as u32 }
}

impl Ctx<'_> {
    /// Block formatting context: lay `nodes` as a vertical stack, grouping
    /// consecutive inline-level content into line boxes. Returns the y below
    /// the last child.
    fn layout_children(&mut self, nodes: &[Node], parent: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let mut y = y0;
        let mut inline = Inline::new();
        let mut carry = 0.0f32; // previous block's (collapsible) bottom margin
        let mut had_block = false;

        for node in nodes {
            match node {
                Node::Text(t) => inline.text(self.font, t, parent, None),
                Node::Element(el) => {
                    let st = style::resolve(el, parent, self.theme, self.sheet, &self.path);
                    match st.display {
                        Display::None => {}
                        Display::Inline => {
                            self.path.push(ElemInfo::of(el));
                            self.collect_inline(el, &st, None, &mut inline);
                            self.path.pop();
                        }
                        Display::Block
                        | Display::ListItem
                        | Display::Table
                        | Display::Flex
                        | Display::Grid => {
                            if !inline.is_empty() {
                                y = inline.flow(self.font, x, w, y, &mut self.ops, &mut self.links);
                                inline = Inline::new();
                                carry = 0.0;
                            }
                            let top = if had_block { carry.max(st.margin_top) } else { st.margin_top };
                            y += top as i32;
                            self.path.push(ElemInfo::of(el));
                            y = self.layout_box(el, &st, x, w, y);
                            self.path.pop();
                            carry = st.margin_bottom;
                            had_block = true;
                        }
                    }
                }
            }
        }
        if !inline.is_empty() {
            y = inline.flow(self.font, x, w, y, &mut self.ops, &mut self.links);
        } else if had_block {
            y += carry as i32;
        }
        y
    }

    /// Lay one block-level box with the CSS block box model: resolve the
    /// horizontal box (margins incl. `auto`-centering, width, min/max-width,
    /// padding) within the containing block's content width `w`, add vertical
    /// padding, then lay the content. This is what makes `max-width` + `margin:
    /// 0 auto` **centered containers** work.
    fn layout_block(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let (cw, off_left) = resolve_block_h(st, w as f32);
        let content_x = x + off_left as i32;
        let content_w = cw.max(1.0) as i32;

        // Border-box geometry (the caller already advanced past margin-top → y0
        // is the border-box top). Background is inserted at `bg_idx` so it lands
        // behind the box's content; the border is drawn on top of the edges.
        let box_left = content_x - st.pad_left as i32;
        let box_w = content_w + (st.pad_left + st.pad_right) as i32;
        let bg_idx = self.ops.len();

        let mut y = y0 + st.pad_top as i32;
        if st.is_rule {
            self.ops.push(DrawOp::Rect { x: content_x, y: y + 1, w: content_w.max(1), h: 1, color: self.theme.rule });
            return y + 3 + st.pad_bottom as i32;
        }
        if st.display == Display::ListItem {
            let s = 4;
            let by = y + (st.font_px * 0.55) as i32;
            self.ops.push(DrawOp::Rect { x: content_x - 12, y: by, w: s, h: s, color: self.theme.muted });
        }

        y = if st.pre {
            layout_pre(self.font, el, st, content_x, content_w, y, &mut self.ops)
        } else {
            self.layout_children(&el.children, st, content_x, content_w, y)
        };
        y += st.pad_bottom as i32;

        self.paint_box_decoration(st, box_left, y0, box_w, y - y0, bg_idx);
        y
    }

    /// Insert the block's `background-color` behind its content (at `bg_idx`)
    /// and stroke its `border` on the border-box edges.
    fn paint_box_decoration(&mut self, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, bg_idx: usize) {
        if w <= 0 || h <= 0 {
            return;
        }
        if let Some(bg) = st.bg {
            self.ops.insert(bg_idx, DrawOp::Rect { x, y, w, h, color: bg });
        }
        if let Some(bc) = st.border_color {
            let b = st.border_width as i32;
            if b > 0 {
                self.ops.push(DrawOp::Rect { x, y, w, h: b, color: bc }); // top
                self.ops.push(DrawOp::Rect { x, y: y + h - b, w, h: b, color: bc }); // bottom
                self.ops.push(DrawOp::Rect { x, y, w: b, h, color: bc }); // left
                self.ops.push(DrawOp::Rect { x: x + w - b, y, w: b, h, color: bc }); // right
            }
        }
    }

    /// Simplified table layout: rows stack; cells sit in auto-width columns.
    /// Column widths come from cell content (preferred, clamped to fit the
    /// available width, never below the longest word). No colspan/rowspan/
    /// border-collapse yet — enough to make infoboxes + data tables readable.
    fn layout_table(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        const PADC: i32 = 6; // per-cell padding

        // <caption> renders as a block above the grid.
        let mut y = y0;
        for c in &el.children {
            if let Node::Element(e) = c {
                if e.tag == "caption" {
                    let cs = style::resolve(e, st, self.theme, self.sheet, &self.path);
                    self.path.push(ElemInfo::of(e));
                    y = self.layout_children(&e.children, &cs, x, w, y);
                    self.path.pop();
                }
            }
        }

        let mut rows: Vec<Vec<&Element>> = Vec::new();
        collect_rows(el, &mut rows);
        rows.retain(|r| !r.is_empty());
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0).min(64);
        if ncols == 0 {
            return y;
        }

        // Intrinsic widths per column: preferred (whole content on one line)
        // and minimum (longest single word — never wrap inside a word).
        let mut pref = vec![0.0f32; ncols];
        let mut minw = vec![0.0f32; ncols];
        for row in &rows {
            for (c, cell) in row.iter().enumerate().take(ncols) {
                let (p, m) = self.intrinsic_width(cell);
                pref[c] = pref[c].max(p);
                minw[c] = minw[c].max(m);
            }
        }

        // Resolve column content widths to fit `avail` (excluding cell padding).
        let avail = (w - 2 * PADC * ncols as i32).max(ncols as i32 * 10) as f32;
        let total: f32 = pref.iter().sum();
        let mut colw = pref.clone();
        if total > avail && total > 0.0 {
            for c in 0..ncols {
                colw[c] = (avail * pref[c] / total).max(minw[c]);
            }
        }

        // Lay each row: cells side by side, row height = tallest cell.
        for row in &rows {
            let mut cx = x;
            let mut row_h = 0i32;
            for (c, cell) in row.iter().enumerate().take(ncols) {
                let cw = colw[c] as i32;
                let cs = style::resolve(cell, st, self.theme, self.sheet, &self.path);
                if cs.display == Display::None {
                    cx += cw + 2 * PADC;
                    continue;
                }
                self.path.push(ElemInfo::of(cell));
                let bottom = self.layout_children(&cell.children, &cs, cx + PADC, cw, y + PADC);
                self.path.pop();
                row_h = row_h.max(bottom - (y + PADC));
                cx += cw + 2 * PADC;
            }
            let row_bottom = y + row_h + 2 * PADC;
            // subtle row separator
            self.ops.push(DrawOp::Rect { x, y: row_bottom, w: (cx - x).max(1), h: 1, color: self.theme.rule });
            y = row_bottom + 1;
        }
        y
    }

    /// (preferred, minimum) content width of a box: preferred = all text on one
    /// line, minimum = the widest single word. Approximated over concatenated
    /// text (ignores nested font sizes) — fine for auto table/flex sizing.
    fn intrinsic_width(&self, el: &Element) -> (f32, f32) {
        let mut text = String::new();
        gather_text(el, &mut text);
        let pref = measure(self.font, text.trim(), BASE_FONT_PX);
        let min = text
            .split_whitespace()
            .map(|wd| measure(self.font, wd, BASE_FONT_PX))
            .fold(0.0f32, f32::max);
        (pref, min)
    }

    /// Dispatch a block-level box to the right formatting context.
    fn layout_box(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        match st.display {
            Display::Table => self.layout_table(el, st, x, w, y),
            Display::Flex => self.layout_flex(el, st, x, w, y),
            Display::Grid => self.layout_grid(el, st, x, w, y),
            _ => self.layout_block(el, st, x, w, y),
        }
    }

    /// Grid layout (css-grid-2 subset): explicit `grid-template-columns`
    /// (px/%/fr/auto/`repeat`), row-major **auto-placement**, column `span`,
    /// `gap`; row heights are content-driven (auto). No explicit line
    /// placement, `grid-template-rows`/`areas`, dense flow, or item alignment.
    fn layout_grid(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let ncols = st.grid_ncols as usize;
        // No template → fall back to normal block flow (single implicit column).
        if ncols == 0 {
            return self.layout_children(&el.children, st, x, w, y0);
        }

        let mut items: Vec<(&Element, ComputedStyle)> = Vec::new();
        for c in &el.children {
            if let Node::Element(ce) = c {
                let cs = style::resolve(ce, st, self.theme, self.sheet, &self.path);
                if cs.display != Display::None {
                    items.push((ce, cs));
                }
            }
        }
        if items.is_empty() {
            return self.layout_children(&el.children, st, x, w, y0);
        }

        // Row-major auto-placement: (col, span, row) per item.
        let mut place: Vec<(usize, usize, usize)> = Vec::with_capacity(items.len());
        let (mut col, mut row) = (0usize, 0usize);
        for (_, s) in &items {
            let span = (s.grid_col_span as usize).clamp(1, ncols);
            if col + span > ncols {
                row += 1;
                col = 0;
            }
            place.push((col, span, row));
            col += span;
            if col >= ncols {
                row += 1;
                col = 0;
            }
        }
        let nrows = place.iter().map(|(_, _, r)| r + 1).max().unwrap_or(0);
        let gap = st.gap;

        // Column sizing: fixed/% resolve directly, `auto` = max content of its
        // single-span items, `fr` splits the remaining space.
        let avail = w as f32;
        let mut auto_content = vec![0.0f32; ncols];
        for (i, (el_i, _)) in items.iter().enumerate() {
            let (c, span, _) = place[i];
            if span == 1 {
                auto_content[c] = auto_content[c].max(self.intrinsic_width(el_i).0);
            }
        }
        let mut colw = vec![0.0f32; ncols];
        let (mut fr_sum, mut used) = (0.0f32, 0.0f32);
        for c in 0..ncols {
            match st.grid_tracks[c] {
                GridTrack::Fixed(px) => {
                    colw[c] = px;
                    used += px;
                }
                GridTrack::Pct(p) => {
                    colw[c] = p / 100.0 * avail;
                    used += colw[c];
                }
                GridTrack::Auto => {
                    colw[c] = auto_content[c];
                    used += colw[c];
                }
                GridTrack::Fr(f) => fr_sum += f,
            }
        }
        let gaps_w = gap * (ncols as f32 - 1.0).max(0.0);
        let leftover = (avail - gaps_w - used).max(0.0);
        if fr_sum > 0.0 {
            for c in 0..ncols {
                if let GridTrack::Fr(f) = st.grid_tracks[c] {
                    colw[c] = leftover * f / fr_sum;
                }
            }
        }
        let mut colx = vec![0.0f32; ncols];
        let mut acc = x as f32;
        for c in 0..ncols {
            colx[c] = acc;
            acc += colw[c] + gap;
        }

        // Lay rows top to bottom; row height = tallest cell in the row.
        let mut y = y0;
        for r in 0..nrows {
            let row_top = y;
            let mut row_h = 0i32;
            for (i, (el_i, s)) in items.iter().enumerate() {
                let (c, span, ir) = place[i];
                if ir != r {
                    continue;
                }
                let mut iw = gap * (span as f32 - 1.0).max(0.0);
                for k in 0..span {
                    iw += colw[c + k];
                }
                self.path.push(ElemInfo::of(el_i));
                let bottom = self.layout_box(el_i, s, colx[c] as i32, iw.max(1.0) as i32, row_top);
                self.path.pop();
                row_h = row_h.max(bottom - row_top);
            }
            y = row_top + row_h;
            if r + 1 < nrows {
                y += gap as i32;
            }
        }
        y
    }

    /// Single-line flex layout (css-flexbox-1 subset): row or column direction,
    /// `flex-grow`/`-shrink`/`-basis`, `gap`, `justify-content`, `align-items`/
    /// `align-self`, `order`. No wrapping/reverse/`margin:auto` yet.
    fn layout_flex(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // Flex items = child elements (display:none excluded). Bare text between
        // items is dropped (rare); a text-only "flex" box falls back to block.
        let mut items: Vec<(&Element, ComputedStyle)> = Vec::new();
        for c in &el.children {
            if let Node::Element(ce) = c {
                let cs = style::resolve(ce, st, self.theme, self.sheet, &self.path);
                if cs.display != Display::None {
                    items.push((ce, cs));
                }
            }
        }
        if items.is_empty() {
            return self.layout_children(&el.children, st, x, w, y0);
        }
        items.sort_by_key(|(_, s)| s.order); // stable → equal order keeps DOM order

        if st.flex_row {
            self.flex_row(&items, st, x, w, y0)
        } else {
            self.flex_column(&items, st, x, w, y0)
        }
    }

    fn flex_row(&mut self, items: &[(&Element, ComputedStyle)], st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let n = items.len();
        let avail = w as f32;
        let gap = st.gap;
        let gaps_total = gap * (n as f32 - 1.0).max(0.0);

        // Flex base main-size (width) per item, then resolve grow/shrink.
        let mut base = alloc::vec![0.0f32; n];
        for (i, (el, s)) in items.iter().enumerate() {
            base[i] = match s.flex_basis {
                FlexBasis::Px(p) => p,
                FlexBasis::Pct(p) => p / 100.0 * avail,
                FlexBasis::Auto => self.intrinsic_width(el).0,
            };
        }
        let sum_base: f32 = base.iter().sum();
        let free = avail - sum_base - gaps_total;
        let mut size = base.clone();
        if free > 0.5 {
            let tg: f32 = items.iter().map(|(_, s)| s.flex_grow).sum();
            if tg > 0.0 {
                for (i, (_, s)) in items.iter().enumerate() {
                    size[i] = base[i] + free * s.flex_grow / tg;
                }
            }
        } else if free < -0.5 {
            let ts: f32 = items.iter().enumerate().map(|(i, (_, s))| s.flex_shrink * base[i]).sum();
            if ts > 0.0 {
                for (i, (el, s)) in items.iter().enumerate() {
                    let min = self.intrinsic_width(el).1;
                    size[i] = (base[i] + free * (s.flex_shrink * base[i]) / ts).max(min);
                }
            }
        }

        // Distribute any leftover (grow didn't take) per justify-content.
        let used: f32 = size.iter().sum::<f32>() + gaps_total;
        let leftover = (avail - used).max(0.0);
        let (offset, extra_gap) = match st.justify {
            Justify::Start => (0.0, 0.0),
            Justify::End => (leftover, 0.0),
            Justify::Center => (leftover / 2.0, 0.0),
            Justify::Between => (0.0, if n > 1 { leftover / (n as f32 - 1.0) } else { 0.0 }),
            Justify::Around => (leftover / (2.0 * n as f32), leftover / n as f32),
            Justify::Evenly => (leftover / (n as f32 + 1.0), leftover / (n as f32 + 1.0)),
        };

        // Lay each item as a block at its resolved width; record op range+height.
        let mut main = x as f32 + offset;
        let mut ranges: Vec<(usize, usize, i32)> = Vec::with_capacity(n); // (op0, link0, height)
        for (i, (el, s)) in items.iter().enumerate() {
            let iw = size[i].max(1.0) as i32;
            let op0 = self.ops.len();
            let link0 = self.links.len();
            self.path.push(ElemInfo::of(el));
            let bottom = self.layout_box(el, s, main as i32, iw, y0);
            self.path.pop();
            ranges.push((op0, link0, bottom - y0));
            main += size[i] + gap + extra_gap;
        }

        // Cross-axis (vertical) alignment within the line box.
        let line_cross = ranges.iter().map(|(_, _, h)| *h).max().unwrap_or(0);
        for (i, (_, s)) in items.iter().enumerate() {
            let (op0, link0, h) = ranges[i];
            let op1 = ranges.get(i + 1).map(|r| r.0).unwrap_or(self.ops.len());
            let link1 = ranges.get(i + 1).map(|r| r.1).unwrap_or(self.links.len());
            let dy = match s.align_self.unwrap_or(st.align_items) {
                CrossAlign::Stretch | CrossAlign::Start => 0,
                CrossAlign::Center => (line_cross - h) / 2,
                CrossAlign::End => line_cross - h,
            };
            if dy != 0 {
                self.shift_ops(op0, op1, link0, link1, dy);
            }
        }
        y0 + line_cross
    }

    /// Column flex ≈ block stacking with `gap` + cross-axis (horizontal)
    /// alignment. Height is content-driven (auto), so grow/shrink/justify along
    /// the main axis don't apply.
    fn flex_column(&mut self, items: &[(&Element, ComputedStyle)], st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let mut y = y0;
        for (i, (el, s)) in items.iter().enumerate() {
            let align = s.align_self.unwrap_or(st.align_items);
            let iw = match align {
                CrossAlign::Stretch => w,
                _ => (self.intrinsic_width(el).0 as i32).clamp(1, w),
            };
            let ix = match align {
                CrossAlign::Stretch | CrossAlign::Start => x,
                CrossAlign::Center => x + (w - iw) / 2,
                CrossAlign::End => x + (w - iw),
            };
            self.path.push(ElemInfo::of(el));
            y = self.layout_box(el, s, ix, iw, y);
            self.path.pop();
            if i + 1 < items.len() {
                y += st.gap as i32;
            }
        }
        y
    }

    /// Shift a contiguous slice of already-emitted ops + links by `dy` (used to
    /// place a flex item on the cross axis after the line's size is known).
    fn shift_ops(&mut self, o0: usize, o1: usize, l0: usize, l1: usize, dy: i32) {
        for op in &mut self.ops[o0..o1] {
            match op {
                DrawOp::Text { y, .. } => *y += dy,
                DrawOp::Rect { y, .. } => *y += dy,
            }
        }
        for lk in &mut self.links[l0..l1] {
            lk.y += dy;
        }
    }
}

/// Flatten a table's rows (through `thead`/`tbody`/`tfoot`) into lists of cells.
fn collect_rows<'a>(el: &'a Element, rows: &mut Vec<Vec<&'a Element>>) {
    for c in &el.children {
        if let Node::Element(e) = c {
            match e.tag.as_str() {
                "tr" => {
                    let cells = e
                        .children
                        .iter()
                        .filter_map(|cc| match cc {
                            Node::Element(ce) if ce.tag == "td" || ce.tag == "th" => Some(ce),
                            _ => None,
                        })
                        .collect();
                    rows.push(cells);
                }
                "thead" | "tbody" | "tfoot" => collect_rows(e, rows),
                _ => {}
            }
        }
    }
}

/// `white-space: pre` — honor newlines and runs of spaces; no word-wrap.
fn layout_pre(
    font: &Font,
    el: &Element,
    st: &ComputedStyle,
    x: i32,
    _w: i32,
    y0: i32,
    ops: &mut Vec<DrawOp>,
) -> i32 {
    let mut raw = String::new();
    gather_text(el, &mut raw);
    // Browsers strip a single leading newline right after <pre>.
    let raw = raw.strip_prefix('\n').unwrap_or(&raw);
    let lh = ceil_i32(line_gap(font, st.font_px));
    let asc = ascent_i(font, st.font_px);
    let mut y = y0;
    for line in raw.split('\n') {
        let text = line.replace('\t', "    ");
        if !text.is_empty() {
            ops.push(DrawOp::Text {
                x,
                y: y + (lh - asc) / 2,
                size: st.font_px,
                color: st.color,
                bold: st.bold,
                italic: st.italic,
                text,
            });
        }
        y += lh;
    }
    y
}

fn gather_text(el: &Element, out: &mut String) {
    for c in &el.children {
        match c {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => gather_text(e, out),
        }
    }
}

impl Ctx<'_> {
    /// Collect an inline element's subtree into the current inline run
    /// (recursing through nested inline elements, carrying each one's style +
    /// link href). `el` is already on `self.path` when this is called.
    fn collect_inline(&mut self, el: &Element, st: &ComputedStyle, href: Option<&str>, inline: &mut Inline) {
        if st.is_break {
            inline.brk();
            return;
        }
        let href = if st.is_link { el.attr("href").or(href) } else { href };
        for c in &el.children {
            match c {
                Node::Text(t) => inline.text(self.font, t, st, href),
                Node::Element(ce) => {
                    let cs = style::resolve(ce, st, self.theme, self.sheet, &self.path);
                    if cs.display != Display::None {
                        self.path.push(ElemInfo::of(ce));
                        self.collect_inline(ce, &cs, href, inline);
                        self.path.pop();
                    }
                }
            }
        }
    }
}

// ── inline formatting context ──────────────────────────────────────────────

/// The visual attributes a text run needs to be measured + painted. Two runs
/// merge into one `DrawOp` only if these match (fewer ops, same pixels).
#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    size: f32,
    color: Rgb,
    bold: bool,
    italic: bool,
}

/// One inline item: a word (with its run style + optional link) or a `<br>`.
enum Item {
    Word { text: String, style: RunStyle, href: Option<String>, space_before: bool },
    Break,
}

/// Accumulates inline content, then flows it into line boxes. Whitespace
/// collapses per `white-space: normal`: a run of spaces (within a text node or
/// across inline-element boundaries) becomes at most one inter-word space.
struct Inline {
    items: Vec<Item>,
    pending_space: bool,
}

impl Inline {
    fn new() -> Inline {
        Inline { items: Vec::new(), pending_space: false }
    }
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add collapsed text from one text node under style `st`.
    fn text(&mut self, _font: &Font, raw: &str, st: &ComputedStyle, href: Option<&str>) {
        let rs = RunStyle { size: st.font_px, color: st.color, bold: st.bold, italic: st.italic };
        let mut word = String::new();
        for ch in raw.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    self.push_word(core::mem::take(&mut word), rs, href);
                }
                if !self.items.is_empty() {
                    self.pending_space = true;
                }
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            self.push_word(word, rs, href);
        }
    }

    fn push_word(&mut self, text: String, style: RunStyle, href: Option<&str>) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Word { text, style, href: href.map(|s| s.to_string()), space_before });
    }

    fn brk(&mut self) {
        self.items.push(Item::Break);
        self.pending_space = false;
    }

    /// Flow the accumulated items into line boxes starting at `y0`; append the
    /// resulting `DrawOp`s + `LinkRect`s. Returns the y below the last line.
    fn flow(
        &self,
        font: &Font,
        x: i32,
        w: i32,
        y0: i32,
        ops: &mut Vec<DrawOp>,
        links: &mut Vec<LinkRect>,
    ) -> i32 {
        let mut y = y0;
        let mut line: Vec<Seg> = Vec::new();
        let mut pen = x as f32;
        let mut line_ascent = 0.0f32;
        let mut gap = 0.0f32;
        let right = (x + w) as f32;

        for item in &self.items {
            match item {
                Item::Break => {
                    if line.is_empty() {
                        y += ceil_i32(line_gap(font, BASE_FONT_PX));
                    } else {
                        y = emit_line(font, &mut line, y, line_ascent, gap, ops, links);
                    }
                    pen = x as f32;
                    line_ascent = 0.0;
                    gap = 0.0;
                }
                Item::Word { text, style, href, space_before } => {
                    let ww = measure(font, text, style.size);
                    let sw = if *space_before { space_width(font, style.size) } else { 0.0 };
                    if !line.is_empty() && pen + sw + ww > right {
                        y = emit_line(font, &mut line, y, line_ascent, gap, ops, links);
                        pen = x as f32;
                        line_ascent = 0.0;
                        gap = 0.0;
                    }
                    let lead = if line.is_empty() { 0.0 } else { sw };
                    let sx = (pen + lead) as i32;
                    let merge = matches!(line.last(), Some(last) if last.style == *style && last.href == *href);
                    if merge {
                        let last = line.last_mut().unwrap();
                        if lead > 0.0 {
                            last.text.push(' ');
                        }
                        last.text.push_str(text);
                    } else {
                        line.push(Seg { x: sx, text: text.clone(), style: *style, href: href.clone() });
                    }
                    pen += lead + ww;
                    line_ascent = line_ascent.max(font.horizontal_line_metrics(style.size).map(|m| m.ascent).unwrap_or(style.size));
                    gap = gap.max(line_gap(font, style.size));
                }
            }
        }
        if !line.is_empty() {
            y = emit_line(font, &mut line, y, line_ascent, gap, ops, links);
        }
        y
    }
}

/// One same-style segment placed on the current line.
struct Seg {
    x: i32,
    text: String,
    style: RunStyle,
    href: Option<String>,
}

/// Emit one completed line's segments at a shared baseline; return the next
/// line's top y. Each run's `y` is set so `top + ascent(size) == baseline`,
/// which is exactly how the rasteriser reconstructs the baseline → mixed sizes
/// on a line align.
fn emit_line(
    font: &Font,
    line: &mut Vec<Seg>,
    y: i32,
    line_ascent: f32,
    gap: f32,
    ops: &mut Vec<DrawOp>,
    links: &mut Vec<LinkRect>,
) -> i32 {
    let line_top = y;
    let baseline = y + line_ascent as i32;
    let box_h = ceil_i32(gap).max(1);
    for seg in line.drain(..) {
        let top = baseline - ascent_i(font, seg.style.size);
        if let Some(h) = &seg.href {
            let sw = measure(font, &seg.text, seg.style.size);
            links.push(LinkRect { x: seg.x, y: line_top, w: ceil_i32(sw), h: box_h, href: h.clone() });
        }
        ops.push(DrawOp::Text {
            x: seg.x,
            y: top,
            size: seg.style.size,
            color: seg.style.color,
            bold: seg.style.bold,
            italic: seg.style.italic,
            text: seg.text,
        });
    }
    line_top + box_h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use fontdue::{Font, FontSettings};

    fn font() -> Font {
        Font::from_bytes(
            include_bytes!("../assets/inter.ttf") as &[u8],
            FontSettings::default(),
        )
        .unwrap()
    }

    fn lay(html: &str, w: u32) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom);
        layout(&font(), &dom, &sheet, w, &Theme::DARK)
    }

    fn texts(l: &Layout) -> Vec<(i32, i32, &str)> {
        l.ops
            .iter()
            .filter_map(|o| match o {
                DrawOp::Text { x, y, text, .. } => Some((*x, *y, text.as_str())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn links_flow_inline_with_surrounding_text() {
        // "Hello <a>Wikipedia</a> and <a>Phosphor</a> here" must lay onto ONE
        // line (wide viewport) — the whole point of inline flow.
        let l = lay(
            "<body><p>Hello <a href=\"/w\">Wikipedia</a> and <a href=\"/p\">Phosphor</a> here</p></body>",
            2000,
        );
        let t = texts(&l);
        assert!(!t.is_empty());
        let first_y = t[0].1;
        assert!(t.iter().all(|(_, y, _)| *y == first_y), "inline runs share a line: {t:?}");
        // Both links are clickable and to the right of the leading text.
        assert_eq!(l.links.len(), 2);
        assert!(l.links[0].x > t[0].0);
        assert!(l.links[1].x > l.links[0].x);
    }

    #[test]
    fn long_paragraph_wraps_to_multiple_lines() {
        let words = "lorem ipsum dolor sit amet ".repeat(20);
        let l = lay(&alloc::format!("<body><p>{words}</p></body>"), 300);
        let ys: Vec<i32> = texts(&l).iter().map(|(_, y, _)| *y).collect();
        let distinct = {
            let mut v = ys.clone();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        assert!(distinct > 3, "expected several wrapped lines, got {distinct}");
    }

    #[test]
    fn headings_are_bigger_and_bold() {
        let l = lay("<body><h1>Title</h1><p>body</p></body>", 800);
        let h = l.ops.iter().find_map(|o| match o {
            DrawOp::Text { size, bold, text, .. } if text == "Title" => Some((*size, *bold)),
            _ => None,
        });
        let (hs, hb) = h.expect("heading run");
        assert!(hs > BASE_FONT_PX * 1.5);
        assert!(hb);
    }

    #[test]
    fn bold_and_italic_runs_carry_flags() {
        let l = lay("<body><p>a <b>bee</b> <i>eye</i> z</p></body>", 2000);
        let bold = l.ops.iter().any(|o| matches!(o, DrawOp::Text { text, bold: true, .. } if text == "bee"));
        let ital = l.ops.iter().any(|o| matches!(o, DrawOp::Text { text, italic: true, .. } if text == "eye"));
        assert!(bold, "bold run");
        assert!(ital, "italic run");
    }

    #[test]
    fn author_style_block_colours_and_selects() {
        // A type rule colours all <p>; a descendant rule colours links inside
        // .box only; specificity: #id beats the type rule on the same element.
        let l = lay(
            "<html><head><style>\
             p { color: #ff0000 } \
             .box a { color: #00ff00 } \
             #hi { color: #0000ff }\
             </style></head><body>\
             <p>red</p>\
             <p id=\"hi\">blue</p>\
             <div class=\"box\"><a href=\"/x\">green</a></div>\
             </body></html>",
            2000,
        );
        let color_of = |t: &str| {
            l.ops.iter().find_map(|o| match o {
                DrawOp::Text { text, color, .. } if text == t => Some(*color),
                _ => None,
            })
        };
        assert_eq!(color_of("red"), Some(Rgb(255, 0, 0)));
        assert_eq!(color_of("blue"), Some(Rgb(0, 0, 255))); // #id wins over p
        assert_eq!(color_of("green"), Some(Rgb(0, 255, 0))); // .box a matched
    }

    #[test]
    fn table_lays_cells_in_columns_and_stacks_rows() {
        let l = lay(
            "<body><table>\
             <tr><th>Land</th><td>Schweiz</td></tr>\
             <tr><th>Kanton</th><td>Nidwalden</td></tr>\
             </table></body>",
            800,
        );
        let t = texts(&l);
        let cell = |s: &str| *t.iter().find(|(_, _, txt)| *txt == s).expect(s);
        let land = cell("Land");
        let schweiz = cell("Schweiz");
        let kanton = cell("Kanton");
        assert_eq!(land.1, schweiz.1, "cells of a row share a y (same row)");
        assert!(schweiz.0 > land.0, "2nd column sits to the right of the 1st");
        assert!(kanton.1 > land.1, "row 2 is below row 1");
        assert!(
            l.ops.iter().any(|o| matches!(o, DrawOp::Text { text, bold: true, .. } if text == "Land")),
            "th renders bold"
        );
    }

    #[test]
    fn max_width_container_centers_and_pads() {
        // `.container { max-width:400px; margin:0 auto; padding:20px }` on an
        // 800px viewport (body content width 760): the box is capped to 400 and
        // centered → left margin (760-400)/2 = 180, +PAD(20) +pad_left(20) → x≈220.
        let l = lay(
            "<body><div style=\"max-width:400px; margin:0 auto; padding:20px\"><p>hi</p></div></body>",
            800,
        );
        let x = l.ops.iter().find_map(|o| match o {
            DrawOp::Text { x, text, .. } if text == "hi" => Some(*x),
            _ => None,
        }).unwrap();
        assert!((190..=250).contains(&x), "container centered+padded → x≈220, got {x}");
    }

    #[test]
    fn block_background_paints_behind_content() {
        let l = lay("<body><div style=\"background:#202030; padding:10px\"><p>x</p></div></body>", 800);
        let bg = l.ops.iter().position(|o| matches!(o, DrawOp::Rect { color, .. } if *color == Rgb(0x20, 0x20, 0x30)));
        let tx = l.ops.iter().position(|o| matches!(o, DrawOp::Text { text, .. } if text == "x"));
        assert!(bg.is_some(), "background rect emitted");
        assert!(bg < tx, "background paints before (behind) the text");
    }

    #[test]
    fn flex_row_places_items_side_by_side() {
        // Two flex:1 items in a row → side by side, splitting the width, NOT
        // stacked. Without flex they'd be one-below-the-other.
        let l = lay(
            "<body><div style=\"display:flex; gap:10px\">\
             <div style=\"flex:1\">left</div>\
             <div style=\"flex:1\">right</div>\
             </div></body>",
            800,
        );
        let t = texts(&l);
        let left = *t.iter().find(|(_, _, s)| *s == "left").expect("left");
        let right = *t.iter().find(|(_, _, s)| *s == "right").expect("right");
        assert_eq!(left.1, right.1, "row items share a y (not stacked)");
        assert!(right.0 > left.0 + 200, "2nd item pushed right by the 1st's grown width");
    }

    #[test]
    fn flex_justify_content_end_pushes_items_right() {
        let start = lay("<body><div style=\"display:flex\"><span>x</span></div></body>", 800);
        let end = lay("<body><div style=\"display:flex; justify-content:flex-end\"><span>x</span></div></body>", 800);
        let sx = start.ops.iter().find_map(|o| match o { DrawOp::Text { x, text, .. } if text == "x" => Some(*x), _ => None }).unwrap();
        let ex = end.ops.iter().find_map(|o| match o { DrawOp::Text { x, text, .. } if text == "x" => Some(*x), _ => None }).unwrap();
        assert!(ex > sx + 400, "justify-content:flex-end moves the item far right");
    }

    #[test]
    fn grid_places_items_in_columns_and_wraps_rows() {
        // 3 columns, 4 items → items 1-3 on row 1 (distinct x, same y), item 4
        // wraps to row 2 under item 1.
        let l = lay(
            "<body><div style=\"display:grid; grid-template-columns:repeat(3,1fr); gap:10px\">\
             <div>a</div><div>b</div><div>c</div><div>d</div></div></body>",
            900,
        );
        let t = texts(&l);
        let g = |s: &str| *t.iter().find(|(_, _, x)| *x == s).expect(s);
        let (a, b, c, d) = (g("a"), g("b"), g("c"), g("d"));
        assert_eq!(a.1, b.1, "a,b same row");
        assert_eq!(b.1, c.1, "b,c same row");
        assert!(a.0 < b.0 && b.0 < c.0, "columns left→right");
        assert_eq!(a.0, d.0, "d wraps under a (col 0)");
        assert!(d.1 > a.1, "d on the next row");
    }

    #[test]
    fn grid_column_span_widens_an_item() {
        // col 2 spans both tracks → its content box starts at col 0 (full width).
        let l = lay(
            "<body><div style=\"display:grid; grid-template-columns:1fr 1fr\">\
             <div>x</div><div style=\"grid-column:span 2\">wide</div></div></body>",
            800,
        );
        let t = texts(&l);
        let x = *t.iter().find(|(_, _, s)| *s == "x").unwrap();
        let wide = *t.iter().find(|(_, _, s)| *s == "wide").unwrap();
        assert!(wide.1 > x.1, "spanning item wraps to the next row");
        assert_eq!(wide.0, x.0, "span-2 item starts at column 0");
    }

    #[test]
    fn list_items_get_bullets_and_indent() {
        let l = lay("<body><ul><li>one</li><li>two</li></ul></body>", 800);
        let bullets = l.ops.iter().filter(|o| matches!(o, DrawOp::Rect { .. })).count();
        assert_eq!(bullets, 2, "one bullet per li");
        // list text is indented past the plain content edge (PAD=20)
        assert!(texts(&l).iter().all(|(x, _, _)| *x > 20));
    }
}
