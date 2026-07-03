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
use crate::style::{self, ComputedStyle, Display, BASE_FONT_PX};

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
                        Display::Block | Display::ListItem | Display::Table => {
                            if !inline.is_empty() {
                                y = inline.flow(self.font, x, w, y, &mut self.ops, &mut self.links);
                                inline = Inline::new();
                                carry = 0.0;
                            }
                            let top = if had_block { carry.max(st.margin_top) } else { st.margin_top };
                            y += top as i32;
                            self.path.push(ElemInfo::of(el));
                            y = if st.display == Display::Table {
                                self.layout_table(el, &st, x, w, y)
                            } else {
                                self.layout_block(el, &st, x, w, y)
                            };
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

    /// Lay one block-level box: rule / list bullet / preformatted / normal flow.
    fn layout_block(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let y = y0;
        if st.is_rule {
            self.ops.push(DrawOp::Rect { x, y: y + 1, w: w.max(1), h: 1, color: self.theme.rule });
            return y + 3;
        }

        let content_x = x + st.padding_left as i32;
        let content_w = (w - st.padding_left as i32).max(20);

        if st.display == Display::ListItem {
            let s = 4;
            let by = y + (st.font_px * 0.55) as i32;
            self.ops.push(DrawOp::Rect { x: content_x - 12, y: by, w: s, h: s, color: self.theme.muted });
        }

        if st.pre {
            return layout_pre(self.font, el, st, content_x, content_w, y, &mut self.ops);
        }

        self.layout_children(&el.children, st, content_x, content_w, y)
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
                let (p, m) = self.cell_intrinsic(cell);
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

    /// (preferred, minimum) content width of a cell: preferred = all text on one
    /// line, minimum = the widest single word. Approximated over concatenated
    /// text (ignores nested font sizes) — fine for the auto-width heuristic.
    fn cell_intrinsic(&self, cell: &Element) -> (f32, f32) {
        let mut text = String::new();
        gather_text(cell, &mut text);
        let pref = measure(self.font, text.trim(), BASE_FONT_PX);
        let min = text
            .split_whitespace()
            .map(|wd| measure(self.font, wd, BASE_FONT_PX))
            .fold(0.0f32, f32::max);
        (pref, min)
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
    fn list_items_get_bullets_and_indent() {
        let l = lay("<body><ul><li>one</li><li>two</li></ul></body>", 800);
        let bullets = l.ops.iter().filter(|o| matches!(o, DrawOp::Rect { .. })).count();
        assert_eq!(bullets, 2, "one bullet per li");
        // list text is indented past the plain content edge (PAD=20)
        assert!(texts(&l).iter().all(|(x, _, _)| *x > 20));
    }
}
