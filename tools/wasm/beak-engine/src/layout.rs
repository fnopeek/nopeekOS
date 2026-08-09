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

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use core::cell::RefCell;
use alloc::vec;
use alloc::vec::Vec;
use fontdue::Font;

use crate::css::{ElemInfo, PseudoElem, Stylesheet};
use crate::dom::{Dom, Element, Node};
use crate::forms::{ControlKind, FormState};
use crate::image::ImageMap;
use crate::style::{
    self, BgPos, BgSize, BorderSide, ClearKind, Clip, ComputedStyle, ContentPiece, CrossAlign,
    Display, FlexBasis, FloatKind, GridTrack, Justify, Len, ListStyle, Position, TableLayout,
    TextAlign, TextTransform, ZIndex, BASE_FONT_PX,
};

/// An active float's exclusion rectangle (document space) within a block
/// formatting context. Line boxes and later content avoid these.
#[derive(Clone, Copy)]
struct FloatRect {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
    is_left: bool, // float:left (true) vs float:right (false)
}

/// Whether a block-level box establishes a new block formatting context, so its
/// border box must not overlap floats (CSS2.1 §9.4.1): the formatting-context
/// displays (flex/grid/table) and a box that clips its overflow.
fn establishes_bfc(st: &ComputedStyle) -> bool {
    matches!(st.display, Display::Flex | Display::Grid | Display::Table) || st.overflow_clip
}

/// A set of adjoining vertical margins (CSS2.1 §8.3.1). Collapsing margins do
/// not simply take the maximum: the used value is the largest positive margin
/// plus the most negative one (negatives are deducted from the positive max, or
/// from zero when every adjoining margin is negative).
#[derive(Clone, Copy, Default)]
struct Collapse {
    pos: f32,
    neg: f32,
}

impl Collapse {
    fn one(m: f32) -> Self {
        let mut c = Self::default();
        c.add(m);
        c
    }
    /// Fold one more adjoining margin into the set.
    fn add(&mut self, m: f32) {
        if m >= 0.0 {
            if m > self.pos {
                self.pos = m;
            }
        } else if m < self.neg {
            self.neg = m;
        }
    }
    /// Fold another whole set of adjoining margins in.
    fn merge(&mut self, o: Collapse) {
        if o.pos > self.pos {
            self.pos = o.pos;
        }
        if o.neg < self.neg {
            self.neg = o.neg;
        }
    }
    /// The single used margin the collapsed set resolves to.
    fn value(self) -> f32 {
        self.pos + self.neg
    }
}

/// Result of flowing a run of block/inline children with margin collapsing.
struct Flow {
    /// Y of the bottom edge of the last committed (non-collapsing) content.
    bottom: i32,
    /// Trailing adjoining margin left open at the bottom (not yet committed).
    open: Collapse,
    /// Border-box top of the first committed content (valid iff `committed`).
    first_top: i32,
    /// Whether any content was committed (vs. everything collapsing through).
    committed: bool,
}

/// Result of laying one block-level box in normal flow.
struct BoxOut {
    /// Border-box bottom (== `top_y` when the box collapses through).
    bottom: i32,
    /// Border-box top actually used.
    top_y: i32,
    /// Adjoining margin the box exposes to the next sibling / its parent: its
    /// bottom margin, or — when it collapses through — its whole collapsed set.
    open: Collapse,
    /// The box has no content, border, padding or height: its top and bottom
    /// margins are adjoining and it occupies no vertical space.
    through: bool,
    /// The box's OWN used border-box left edge and width. Not the containing
    /// block's — `max-width`, `margin: 0 auto`, an explicit `width` or plain
    /// margins all make the two differ, and the inspect tool reported the
    /// parent's numbers for years because they coincide on a plain
    /// `width: auto` block.
    box_x: i32,
    box_w: i32,
}

/// The role a child element plays inside a table box (CSS2.1 §17.2.1).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TableRole {
    Row,
    RowGroup,
    /// `table-header-group` (`<thead>`) — rows sort before every other group.
    HeaderGroup,
    /// `table-footer-group` (`<tfoot>`) — rows sort after every other group.
    FooterGroup,
    Cell,
    /// `caption`/`col`/`colgroup` — recognised table structure that generates
    /// no box of its own; never wrapped, never breaks a stray-content run.
    Skip,
    /// Anything else: text or an element that isn't a table part. A run of
    /// consecutive `Other`/`Cell`/text nodes gets wrapped into an anonymous
    /// box (a row, if found directly in a table/row-group; a cell, if found
    /// directly in a row) rather than being silently dropped.
    Other,
}

/// A table cell box: a real `<td>`/`<th>`/`display:table-cell` element, or an
/// anonymous cell wrapping a run of sibling nodes that CSS2.1 §17.2.1 requires
/// boxing (stray text/inline content found directly inside a table row, or a
/// stray element — including a lone cell — found directly inside a table).
#[derive(Clone, Copy)]
enum Cell<'a> {
    Real(&'a Element),
    Anon(&'a [Node]),
}

/// A table cell together with the style it resolved to. The style is settled
/// once, while the row is collected — that is the only place where both the
/// cell's position among its siblings (`td:first-child`) and its real parent
/// chain (`tbody tr td`, and inheritance from the row) are known.
struct StyledCell<'a> {
    cell: Cell<'a>,
    st: ComputedStyle,
}

/// One row of a table's grid, with the boxes it belongs to. Keeping the `<tr>`
/// and its row group here (rather than returning bare cell lists) is what lets
/// a row be styled at all: its own background, and `position: relative`, which
/// moves the whole row — cells included — after it is laid out.
struct Row<'a> {
    /// The `<tr>`/`display:table-row` element and its resolved style. Absent
    /// for an anonymous row wrapping stray content — no element, so no
    /// selector can reach it and it paints nothing of its own.
    el: Option<(&'a Element, ComputedStyle)>,
    /// The `<tbody>`/`<thead>`/`<tfoot>` this row came from. Consecutive rows
    /// carrying the same group form that group's box.
    group: Option<(&'a Element, ComputedStyle)>,
    cells: Vec<StyledCell<'a>>,
}

/// Where a table row / row group box began in the output. Everything emitted
/// from here on belongs to it, which is what lets its background go BEHIND its
/// cells and `position: relative` move the whole thing afterwards.
#[derive(Clone, Copy)]
struct TablePart {
    op: usize,
    link: usize,
    ctl: usize,
}

/// One segment of a `flow_children` node list: either a single node laid out
/// normally, or a maximal run of stray table-part siblings (CSS2 §17.2.1)
/// laid out together as one anonymous `table` box. See `segment_table_runs`.
enum TableSeg<'a> {
    Node(&'a Node),
    Table(&'a [Node]),
}

/// A table's content-box width available to its columns: the used `width`
/// (resolved against `avail`) minus the table's own padding+border under
/// `box-sizing: border-box`; `width: auto` falls back to the full available
/// width so an auto-width fixed table still fills its container.
fn table_content_width(st: &ComputedStyle, avail: f32) -> f32 {
    match st.width.px(avail) {
        Some(wd) if st.box_border => (wd - (st.pad_left + st.pad_right) - st.border_x()).max(0.0),
        Some(wd) => wd.max(0.0),
        None => avail.max(0.0),
    }
}

/// Narrow the x-range `[cl, cr]` by floats overlapping the band `[top, bot)`.
fn band_of(floats: &[FloatRect], top: i32, bot: i32, cl: i32, cr: i32) -> (i32, i32) {
    let (mut l, mut r) = (cl, cr);
    for f in floats {
        if f.bottom > top && f.top < bot {
            if f.is_left {
                l = l.max(f.right);
            } else {
                r = r.min(f.left);
            }
        }
    }
    (l, r.max(l))
}

/// Resolve a block box's horizontal geometry within a containing block of
/// content width `avail`: CSS2.1 §10.3.3 (used width + margins) plus the
/// §10.4 min/max-width redo. Returns (content-width, content-left-offset =
/// margin-left + padding-left).
/// A replaced element whose content we do not lay out — an `<iframe>`'s
/// document, a `<video>`'s frames, a `<canvas>`'s bitmap, an `<object>`'s
/// plugin. What it has is a BOX, and CSS2.1 §10.3.2 / §10.6.2 give a replaced
/// element with no intrinsic size **300 × 150**; HTML maps the presentational
/// `width`/`height` attributes onto it, which is how a video embed states its
/// size. Returns the intrinsic content size, or `None` for anything else.
///
/// `<img>` is deliberately not here: it has real intrinsic dimensions once its
/// pixels land, and its own path (`img_box`) tracks whether the box was guessed.
fn replaced_intrinsic(el: &Element) -> Option<(f32, f32)> {
    if !matches!(el.tag.as_str(), "iframe" | "video" | "canvas" | "object" | "embed") {
        return None;
    }
    // `<object>` is the exception: when its resource cannot be obtained it
    // represents its FALLBACK content and is not replaced at all (HTML §4.8.7).
    // We never load a plugin, so a fallback is exactly what a browser shows —
    // `flexbox_object` measures precisely that. Without a fallback it is still
    // an empty replaced box. `<param>` is metadata, not content.
    if el.tag == "object" {
        let renders = el.children.iter().any(|n| match n {
            Node::Element(c) => c.tag != "param",
            Node::Text(t) => !t.trim().is_empty(),
        });
        if renders {
            return None;
        }
    }
    let attr = |n: &str| {
        el.attr(n)
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
            .filter(|v| *v >= 0.0)
    };
    Some((attr("width").unwrap_or(300.0), attr("height").unwrap_or(150.0)))
}

fn resolve_block_h(st: &ComputedStyle, avail: f32) -> (f32, f32) {
    // Horizontal padding + border both sit between the content box and the
    // margin edge (border-box `width` includes them; content-box adds them).
    let pad = st.pad_left + st.pad_right + st.border_x();
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
    (cw.max(1.0), ml + st.pad_left + st.border_left.width)
}

/// Clip the display-list ops in `ops[start..]` to the document-space rectangle
/// `[cl, ct) .. [cr, cb)`. Filled rects are intersected (pixel-exact); text and
/// images are kept whole if their box overlaps the rect, dropped otherwise (a
/// flat display list can't clip glyph runs mid-way). An empty rect removes the
/// whole range — the CSS 2.1 `clip` case where nothing of the box is painted.
/// Slide a range of already-emitted draw ops vertically. Used to place a
/// bottom-anchored absolutely positioned box, whose final y is only known once
/// its height has been laid out.
fn translate_ops(ops: &mut [DrawOp], dy: i32) {
    for op in ops {
        match op {
            DrawOp::Rect { y, .. }
            | DrawOp::RoundRect { y, .. }
            | DrawOp::Text { y, .. }
            | DrawOp::Image { y, .. }
            | DrawOp::BgImage { y, .. } => *y += dy,
        }
    }
}

/// Move a detached op list (an `inline-block`'s, laid out at the origin) to
/// where its line box put it.
fn translate_op_list(ops: &mut [DrawOp], dx: i32, dy: i32) {
    for op in ops {
        match op {
            DrawOp::Rect { x, y, .. }
            | DrawOp::RoundRect { x, y, .. }
            | DrawOp::Text { x, y, .. }
            | DrawOp::Image { x, y, .. }
            | DrawOp::BgImage { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
    }
}

fn clip_ops(ops: &mut Vec<DrawOp>, start: usize, cl: i32, ct: i32, cr: i32, cb: i32) {
    if start >= ops.len() {
        return;
    }
    if cr <= cl || cb <= ct {
        ops.truncate(start);
        return;
    }
    let tail = ops.split_off(start);
    for op in tail {
        match op {
            DrawOp::Rect { x, y, w, h, color } => {
                let nx = x.max(cl);
                let ny = y.max(ct);
                let nx1 = (x + w).min(cr);
                let ny1 = (y + h).min(cb);
                if nx1 > nx && ny1 > ny {
                    ops.push(DrawOp::Rect { x: nx, y: ny, w: nx1 - nx, h: ny1 - ny, color });
                }
            }
            // A rounded box that the clip fully contains keeps its corners;
            // one the clip cuts degrades to a square rect, which is wrong at
            // the corners but never paints outside the clip.
            DrawOp::RoundRect { x, y, w, h, color, .. } => {
                if x >= cl && y >= ct && x + w <= cr && y + h <= cb {
                    ops.push(op);
                } else {
                    let (nx, ny) = (x.max(cl), y.max(ct));
                    let (nx1, ny1) = ((x + w).min(cr), (y + h).min(cb));
                    if nx1 > nx && ny1 > ny {
                        ops.push(DrawOp::Rect { x: nx, y: ny, w: nx1 - nx, h: ny1 - ny, color });
                    }
                }
            }
            // Kept whole when it overlaps, like `Image`: the layer's origin
            // is its box, so shrinking the rect would MOVE the background
            // rather than crop it. Over-paints only when a clip cuts through a
            // box that has one.
            DrawOp::Image { x, y, w, h, .. } | DrawOp::BgImage { x, y, w, h, .. } => {
                if x < cr && x + w > cl && y < cb && y + h > ct {
                    ops.push(op);
                }
            }
            DrawOp::Text { x, y, size, .. } => {
                let bottom = y + size as i32 + 4;
                if x < cr && y < cb && bottom > ct {
                    ops.push(op);
                }
            }
        }
    }
}

/// `position:relative` paint offset (dx, dy): `left`/`top` win over `right`/
/// `bottom`; `%` resolves against the containing block's content width.
/// `transform: translate(...)` as whole pixels. Percentages are of the box's
/// OWN border box (CSS Transforms 1 §8) — which is what makes
/// `translate(-50%, -50%)` centre a box on the point it is positioned at, and
/// why this cannot reuse `rel_offset`'s containing-block basis.
fn translate_offset(st: &ComputedStyle, box_w: i32, box_h: i32) -> (i32, i32) {
    let Some((tx, ty)) = st.translate else { return (0, 0) };
    let at = |l: Len, basis: i32| match l {
        Len::Px(p) => p as i32,
        Len::Pct(p) => (p / 100.0 * basis as f32) as i32,
        Len::Calc { pct, px } => (pct / 100.0 * basis as f32 + px) as i32,
        Len::Auto => 0,
    };
    (at(tx, box_w), at(ty, box_h))
}

fn rel_offset(st: &ComputedStyle, cb_w: f32) -> (i32, i32) {
    let dx = st
        .left
        .px(cb_w)
        .map(|l| l as i32)
        .or_else(|| st.right.px(cb_w).map(|r| -(r as i32)))
        .unwrap_or(0);
    let dy = st
        .top
        .px(cb_w)
        .map(|t| t as i32)
        .or_else(|| st.bottom.px(cb_w).map(|b| -(b as i32)))
        .unwrap_or(0);
    (dx, dy)
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

    /// Is this a dark palette? Answers `prefers-color-scheme` — the page theme
    /// IS the user's colour-scheme preference here, since the shell resolves it
    /// from the compositor palette. Rec. 601 luma on the page background.
    pub fn is_dark(&self) -> bool {
        let Rgb(r, g, b) = self.bg;
        (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 < 128
    }
}

/// One paint instruction, positioned in document space (pre-scroll).
pub enum DrawOp {
    /// A run of already-wrapped, same-style text; `y` is the run's top.
    Text { x: i32, y: i32, size: f32, color: Rgb, bold: bool, italic: bool, mono: bool, text: String },
    /// A filled rectangle (divider, list bullet).
    Rect { x: i32, y: i32, w: i32, h: i32, color: Rgb },
    /// A `border-radius` box. `r` is `[tl, tr, br, bl]` in px; `ring` is 0 for
    /// a solid fill, or the border thickness to stroke along the inside edge.
    /// Kept apart from `Rect` so the plain case stays one `memory.copy` per
    /// row — the rounded one has to walk its corner rows.
    RoundRect { x: i32, y: i32, w: i32, h: i32, r: [f32; 4], color: Rgb, ring: f32 },
    /// A decoded image, scaled to `w`×`h` at blit time.
    /// An `<img>` box. Carries the `src` KEY, not the decoded pixels: the
    /// rasteriser looks the image up when it paints, and draws a placeholder
    /// on a miss. That way an image arriving after layout costs a repaint
    /// instead of a full re-layout — which on a real article is the
    /// difference between ~15 ms and ~145 ms, per image batch.
    Image { x: i32, y: i32, w: i32, h: i32, src: String, alt: String },
    /// A `background-image` or `mask-image` layer over the box `x,y,w,h` (the
    /// background positioning area). Carries the `url_key`, not the pixels —
    /// same reason as `Image`: an asset arriving late costs a repaint, never a
    /// re-layout, because a background never affects geometry.
    ///
    /// `tint: Some(c)` is the mask case — the image's alpha stencils colour
    /// `c` instead of its own pixels being drawn.
    BgImage {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        key: u64,
        repeat: (bool, bool),
        pos: (BgPos, BgPos),
        size: BgSize,
        tint: Option<Rgb>,
    },
}

/// The bottom edge a draw op reaches, for sizing the scrollable page.
fn op_bottom(op: &DrawOp) -> i32 {
    match op {
        // A text run's `y` is its top; `size` over-estimates the descent
        // slightly, which is the safe direction for a scroll extent.
        DrawOp::Text { y, size, .. } => y + ceil_i32(*size),
        DrawOp::Rect { y, h, .. }
        | DrawOp::RoundRect { y, h, .. }
        | DrawOp::Image { y, h, .. }
        | DrawOp::BgImage { y, h, .. } => y + h,
    }
}

/// `border-radius` resolved to px against the border-box width. Percentages
/// resolve per-axis in CSS; we draw circular corners, so the width is the one
/// basis.
fn radii_px(st: &ComputedStyle, w: i32) -> [f32; 4] {
    let cb = w as f32;
    let one = |l: Len| l.px(cb).unwrap_or(0.0).max(0.0);
    [one(st.radius[0]), one(st.radius[1]), one(st.radius[2]), one(st.radius[3])]
}

/// The border's `(width, colour)` when all four sides carry the same visible
/// one, else `None`.
fn uniform_border(st: &ComputedStyle) -> Option<(f32, Rgb)> {
    let t = &st.border_top;
    let (w, c) = (t.width, t.color?);
    if w <= 0.0 {
        return None;
    }
    for side in [&st.border_right, &st.border_bottom, &st.border_left] {
        if side.width != w || side.color != Some(c) {
            return None;
        }
    }
    Some((w, c))
}

/// One box's background layer, bottom-up: colour (or a mask stencilling it),
/// then the image. Shared by block boxes, which insert it UNDER content they
/// have already emitted, and by inline-box fragments, which push it ahead of
/// their line's text. The keys are resolved by the caller — only it knows
/// where to register the image the layout still needs.
fn bg_ops(st: &ComputedStyle, bg: Option<u64>, mask: Option<u64>, x: i32, y: i32, w: i32, h: i32, out: &mut Vec<DrawOp>) {
    match (st.bg, mask) {
        (Some(color), Some(key)) => out.push(DrawOp::BgImage {
            x,
            y,
            w,
            h,
            key,
            repeat: st.mask_layer.repeat,
            pos: st.mask_layer.pos,
            size: st.mask_layer.size,
            tint: Some(color),
        }),
        (Some(color), None) => {
            let r = radii_px(st, w);
            out.push(if r.iter().any(|&v| v > 0.0) {
                DrawOp::RoundRect { x, y, w, h, r, color, ring: 0.0 }
            } else {
                DrawOp::Rect { x, y, w, h, color }
            });
        }
        // A mask with no colour to stencil paints nothing at all.
        (None, _) => {}
    }
    if let Some(key) = bg {
        out.push(DrawOp::BgImage {
            x,
            y,
            w,
            h,
            key,
            repeat: st.bg_layer.repeat,
            pos: st.bg_layer.pos,
            size: st.bg_layer.size,
            tint: None,
        });
    }
}

/// One box's four border edges. `sides` says whether the box's left and right
/// edges belong to THIS rectangle — a fragment of an inline box that continues
/// on from the previous line, or breaks onto the next one, carries neither
/// (the `box-decoration-break: slice` default).
fn border_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, sides: (bool, bool), out: &mut Vec<DrawOp>) {
    // A rounded border can only be stroked as one shape, so it needs all four
    // sides to agree; anything else falls through to the four independent
    // edges below (square corners, visibly wrong only once the radius is
    // larger than the border).
    let r = radii_px(st, w);
    if sides == (true, true) && r.iter().any(|&v| v > 0.0) {
        if let Some((bw, bc)) = uniform_border(st) {
            out.push(DrawOp::RoundRect { x, y, w, h, r, color: bc, ring: bw });
            return;
        }
    }
    // Each side paints independently on the border-box edge.
    let side = |out: &mut Vec<DrawOp>, s: &BorderSide, rect: (i32, i32, i32, i32)| {
        if let (Some(c), true) = (s.color, s.width > 0.0) {
            let (rx, ry, rw, rh) = rect;
            if rw > 0 && rh > 0 {
                out.push(DrawOp::Rect { x: rx, y: ry, w: rw, h: rh, color: c });
            }
        }
    };
    let (bt, br, bb, bl) = (
        st.border_top.width as i32,
        st.border_right.width as i32,
        st.border_bottom.width as i32,
        st.border_left.width as i32,
    );
    side(out, &st.border_top, (x, y, w, bt));
    side(out, &st.border_bottom, (x, y + h - bb, w, bb));
    if sides.0 {
        side(out, &st.border_left, (x, y, bl, h));
    }
    if sides.1 {
        side(out, &st.border_right, (x + w - br, y, br, h));
    }
}

/// A clickable link's document-space rectangle.
pub struct LinkRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub href: String,
}

/// A laid-out element's document-space box plus a human label — the data behind
/// beak's "inspect" dev tool. Recorded only when inspection is enabled (see
/// `Ctx::inspect`); the shell hit-tests these and shows the deepest box under
/// the cursor so a mis-placed element can be named on the device.
pub struct InspectBox {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Tree depth (ancestor count) — the deepest box containing a point is the
    /// most specific element there.
    pub depth: u16,
    /// `tag#id.class  W×H  display:… float:… position:…` — enough to find the
    /// element in the page and see the geometry/box properties that went wrong.
    pub label: String,
}

/// An interactive form control's document-space rectangle. The shell hit-tests
/// these to give a control focus / activate it; `seq` identifies the element
/// across re-layouts (`dom::Element::seq`).
pub struct ControlRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub seq: u32,
    pub kind: ControlKind,
}

pub struct Layout {
    pub ops: Vec<DrawOp>,
    pub links: Vec<LinkRect>,
    pub controls: Vec<ControlRect>,
    /// Total document height (px). May exceed the viewport → scroll.
    pub height: u32,
    /// Canvas background — the `<body>` background propagated to the whole
    /// viewport (CSS backgrounds §3.11.2), else the theme background.
    pub bg: Rgb,
    /// `src`s whose `<img>` box was GUESSED (no pixels yet, and no
    /// `width`/`height` pair to size it definitely).
    ///
    /// The shell uses this to decide what an arriving image costs: a `src`
    /// that is NOT in here has a definite box, so its pixels only need a
    /// REPAINT; one that is in here can still move the page when it decodes,
    /// which warrants a re-layout. On a real article that is the difference
    /// between ~15 ms and ~145 ms — and under the device's WASM interpreter,
    /// between a page that scrolls while it loads and one that freezes.
    pub guessed_image_srcs: Vec<String>,
    /// `url_key`s of the CSS images (`background-image`/`mask-image`) this
    /// layout actually needs — i.e. the ones that won the cascade on a box we
    /// painted, not every `url()` in the stylesheet. The engine turns these
    /// back into URLs (via the sheet's table) for the shell to fetch.
    pub css_image_keys: Vec<u64>,
    /// The subset of `css_image_keys` the shell still has to fetch, as
    /// (key, URL) — `data:` URIs are resolved by the engine itself and never
    /// appear here. Filled in by `Engine::layout_ext`, which holds the sheet.
    pub css_image_srcs: Vec<(u64, String)>,
    /// Element boxes for the inspect dev tool (empty unless inspection was on).
    pub inspect: Vec<InspectBox>,
}

impl Layout {
    /// The deepest (most specific) inspect box containing a document-space
    /// point, for the inspect dev tool. Ties break toward the one recorded
    /// later (painted on top).
    pub fn hit_inspect(&self, x: i32, y: i32) -> Option<&InspectBox> {
        self.inspect
            .iter()
            .filter(|b| x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h)
            .max_by_key(|b| b.depth)
    }

    /// Link href at a document-space point (caller adds scroll to screen y).
    pub fn hit_test(&self, x: i32, y: i32) -> Option<&str> {
        // Reverse so a link painted later (on top) wins an overlap.
        self.links
            .iter()
            .rev()
            .find(|l| x >= l.x && x < l.x + l.w && y >= l.y && y < l.y + l.h)
            .map(|l| l.href.as_str())
    }

    /// Form control at a document-space point. Checked BEFORE `hit_test` by the
    /// shell: a control nested in a link (a search button inside an `<a>`) must
    /// take the click itself.
    pub fn hit_control(&self, x: i32, y: i32) -> Option<&ControlRect> {
        self.controls
            .iter()
            .rev()
            .find(|c| x >= c.x && x < c.x + c.w && y >= c.y && y < c.y + c.h)
    }
}

// ── small font helpers (no_std: no f32::ceil) ──────────────────────────────

/// Half the x-height that `vertical-align: middle` measures against (CSS2.1
/// §10.8.1 says the parent's, which is not threaded this far down). The line
/// SIZING and the PLACEMENT must use the identical value — with two different
/// approximations a middle-aligned box is sized into one line and painted
/// against another, and lands outside its own line box.
const MIDDLE_HALF_X: f32 = crate::style::BASE_FONT_PX * 0.25;

fn ceil_i32(x: f32) -> i32 {
    let c = x as i32;
    if (c as f32) < x { c + 1 } else { c }
}
fn measure(font: &Font, s: &str, size: f32) -> f32 {
    s.chars().map(|c| font.metrics(c, size).advance_width).sum()
}
/// Byte length of the longest prefix of `s` that fits in `avail` px, snapped
/// back to a legal break. Returns 0 when not even the first cluster fits — the
/// caller decides whether to try a fresh line or force one through (never
/// returning 0 forever is the caller's job, not this function's).
fn fit_prefix(font: &Font, s: &str, size: f32, avail: f32) -> usize {
    let mut used = 0.0;
    let mut end = s.len();
    for (i, c) in s.char_indices() {
        let adv = font.metrics(c, size).advance_width;
        if used + adv > avail {
            end = i;
            break;
        }
        used += adv;
    }
    cluster_boundary(s, end)
}

/// Does `c` bind to the character BEFORE it? Zero-width joiner sequences,
/// variation selectors, skin-tone modifiers, keycaps, combining marks,
/// regional-indicator pairs and tag sequences (the Wales/Scotland/England
/// flags) are all one user-perceived character, and
/// `word-break: break-all` may still not split one (css-text-3 §5.1 breaks
/// between grapheme clusters, not code points).
fn joins_back(c: char) -> bool {
    matches!(c,
        '\u{200D}' | '\u{FE0E}' | '\u{FE0F}' | '\u{20E3}'
        | '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}'
        | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}'
        | '\u{1F3FB}'..='\u{1F3FF}' | '\u{1F1E6}'..='\u{1F1FF}'
        | '\u{E0020}'..='\u{E007F}')
}

/// The largest legal break offset at or before `n`.
fn cluster_boundary(s: &str, mut n: usize) -> usize {
    while n > 0 && n < s.len() {
        let prev = s[..n].chars().next_back().unwrap();
        let next = s[n..].chars().next().unwrap();
        if prev != '\u{200D}' && !joins_back(next) {
            break;
        }
        n -= prev.len_utf8();
    }
    n
}

/// End of the first grapheme cluster in `s` — what a line that cannot fit even
/// one cluster is forced to take, so the loop always makes progress.
fn first_cluster(s: &str) -> usize {
    let mut it = s.char_indices();
    let Some((_, first)) = it.next() else { return 0 };
    let mut n = first.len_utf8();
    let mut prev = first;
    for (i, c) in it {
        if prev != '\u{200D}' && !joins_back(c) {
            return i;
        }
        prev = c;
        n = i + c.len_utf8();
    }
    n
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

/// One inline run's contribution to its line box: `(ascent above the shared
/// baseline, box height)`. With `line-height: normal` these are the face's own
/// metrics — unchanged from before line-height existed. An explicit
/// line-height distributes its difference from the content height as
/// half-leading above and below the baseline (CSS 2.1 §10.8.1), so a value
/// under the content height legitimately yields a negative half and lets
/// consecutive lines overlap.
fn run_metrics(font: &Font, size: f32, lh: f32) -> (f32, f32) {
    let m = font.horizontal_line_metrics(size);
    let asc = m.map(|m| m.ascent).unwrap_or(size);
    if lh <= 0.0 {
        return (asc, line_gap(font, size));
    }
    let desc = m.map(|m| m.descent.abs()).unwrap_or(0.0);
    (asc + (lh - (asc + desc)) / 2.0, lh)
}

// ── CSS counters (css-lists-3 §4) ───────────────────────────────────────────

/// One counter instance on the scope stack: its value, plus the tree DEPTH
/// (`path.len()`) of the element whose `counter-reset` created it — used to tell
/// an ancestor's counter (nest a new instance) from a sibling's (overwrite the
/// existing one).
#[derive(Clone, Copy)]
struct CounterInst {
    name: u32,
    value: i32,
    depth: usize,
}

/// The scoped counter state threaded through the tree walk. A stack of
/// instances (innermost last); `counter()` reads the innermost value of a name,
/// `counters()` walks every in-scope instance of it (outermost first). Scope is
/// bounded by truncating the stack back to a saved length when a subtree's child
/// list ends (see `flow_children`/`collect_inline`), which implements the
/// "descendants + following siblings" scope of css-lists-3 §4.4 closely enough
/// for the content web.
#[derive(Default)]
struct Counters {
    stack: Vec<CounterInst>,
}

impl Counters {
    /// Apply an element's `counter-reset` then `counter-increment` (spec order),
    /// given its tree depth. Reset nests a new instance when the innermost
    /// same-name counter belongs to an ancestor (shallower depth), else it
    /// overwrites that instance's value (self/sibling). Increment auto-creates a
    /// counter at 0 first if none is in scope.
    fn enter(&mut self, st: &ComputedStyle, depth: usize) {
        for &(name, value) in &st.counter_reset[..st.counter_reset_n as usize] {
            match self.stack.iter().rposition(|c| c.name == name) {
                Some(i) if self.stack[i].depth >= depth => {
                    // self or a sibling at the same level → overwrite in place.
                    self.stack[i].value = value;
                    self.stack[i].depth = depth;
                }
                _ => self.stack.push(CounterInst { name, value, depth }),
            }
        }
        for &(name, delta) in &st.counter_increment[..st.counter_increment_n as usize] {
            match self.stack.iter().rposition(|c| c.name == name) {
                Some(i) => self.stack[i].value += delta,
                None => self.stack.push(CounterInst { name, value: delta, depth }),
            }
        }
    }

    /// The innermost in-scope value of `name` (0 if none), for `counter()`.
    fn value(&self, name: u32) -> i32 {
        self.stack.iter().rev().find(|c| c.name == name).map(|c| c.value).unwrap_or(0)
    }

    /// Every in-scope value of `name`, outermost first, for `counters()`.
    fn values(&self, name: u32) -> Vec<i32> {
        self.stack.iter().filter(|c| c.name == name).map(|c| c.value).collect()
    }
}

// ── entry point + block/inline tree walk ───────────────────────────────────

/// Per-layout mutable context: the shared inputs (font / theme / author sheet)
/// plus the accumulating display list and the live ancestor `path` (for
/// selector matching). Bundling these keeps the recursive walkers from carrying
/// a dozen arguments each.
struct Ctx<'a> {
    fonts: &'a crate::fonts::Fonts,
    theme: &'a Theme,
    sheet: &'a Stylesheet,
    images: &'a ImageMap,
    /// `src`s whose `<img>` box had to be GUESSED — no decoded pixels and no
    /// `width`/`height` pair. Only for these does a later decode move the
    /// page, so only their arrival justifies a re-layout.
    ///
    /// A plain bool here was wrong: one image that never arrives (a 403, an
    /// undecodable format) kept it true forever, so every later batch forced
    /// a full re-layout even when all of ITS images had definite boxes. On a
    /// real article that was 5.7 s of frozen UI per batch.
    guessed: core::cell::RefCell<Vec<String>>,
    /// `url_key`s of the CSS images this layout referenced. Deliberately a
    /// SET (deduped on insert), not an append-only log: a throwaway
    /// measurement layout paints boxes too, and its entries must be
    /// indistinguishable from the real pass's rather than something the
    /// measure helpers have to remember to roll back.
    css_images: core::cell::RefCell<Vec<u64>>,
    ops: Vec<DrawOp>,
    links: Vec<LinkRect>,
    controls: Vec<ControlRect>,
    /// Live form-control state (typed values, checked boxes, focus) — read
    /// only; the shell owns it and re-lays out when it changes.
    forms: &'a FormState,
    path: Vec<ElemInfo<'a>>, // root → … → current parent
    /// Positioned containing block (x, y, width, height) for
    /// `position:absolute` descendants — the nearest ancestor with
    /// `position != static`, else page. The height is `None` unless the
    /// establishing box has an explicit one: abspos children are laid out
    /// during the parent's child walk, so a content-derived height isn't
    /// known yet. `top`/`bottom` percentages need it (CSS 2.1 §9.3.2).
    cb: (i32, i32, i32, Option<i32>),
    /// Viewport width (px) — the layout width — for `@media` evaluation.
    viewport_w: f32,
    /// Active floats in the current block formatting context — line boxes and
    /// later blocks flow around them. Saved/restored when entering a new BFC.
    floats: Vec<FloatRect>,
    /// Recorded `(z_index, op_start, op_end)` / `(z_index, link_start,
    /// link_end)` ranges for the **outermost** positioned boxes with an
    /// explicit (non-`auto`) `z-index` — one contiguous slice of `ops`/`links`
    /// per box (CSS2.1 §9.9). `layout()` stable-sorts by `z_index` at the end
    /// so negative levels paint behind, positive ones in front, and everything
    /// else (`auto`/untracked) keeps its in-order position.
    /// `(z-index, paint layer, op_start, op_end)`. The layer separates the
    /// sub-orders CSS2.1 Appendix E puts INSIDE one z-index: in-flow block
    /// boxes paint below floats, and floats below positioned boxes.
    stack_ops: Vec<(i32, i32, usize, usize)>,
    stack_links: Vec<(i32, i32, usize, usize)>,
    /// Op / link ranges emitted by non-positioned floats. Kept apart from
    /// `stack_ops` because a float MAY sit inside a tracked z-index range —
    /// MediaWiki wraps a whole article in one — and `reorder_by_z` needs
    /// disjoint ranges. `split_float_ranges` merges the two at the end by
    /// cutting the enclosing range around the float.
    float_ops: Vec<(usize, usize)>,
    float_links: Vec<(usize, usize)>,
    /// How many out-of-flow boxes have been laid out so far, split by whether
    /// they escape a positioned ancestor. `overflow: hidden` compares these
    /// across its content to see whether anything inside it left its clip's
    /// jurisdiction (CSS2.1 §11.1.1) — see `clip_overflow`.
    abs_count: u32,
    fixed_count: u32,
    /// The containing block's CONTENT height, when it is definite — what a
    /// percentage `height`/`min-`/`max-height` resolves against (CSS2.1 §10.5).
    /// `None` means the containing block's height depends on its content, and
    /// then a percentage computes to `auto`. That fallback is the whole reason
    /// this is an `Option`: `html { height: 100% }` is an everyday idiom, and
    /// guessing a height for it truncates pages.
    cb_h: Option<f32>,
    /// Document y of the last line box's baseline emitted so far. An
    /// `inline-block` aligns on the baseline of ITS last line box (CSS2.1
    /// §10.8.1), which is only known once its content has been laid out.
    last_baseline: Option<i32>,
    /// Depth of currently-open *tracked* (recorded) stacking ranges. Only a
    /// box at depth 0 gets recorded — a z-indexed box nested inside another
    /// already-tracked one paints as part of its ancestor's range instead
    /// (full nested stacking contexts, e.g. an explicit `z-index` inside
    /// another explicit `z-index`, are out of scope — sibling ordering is
    /// the common case these reftests need).
    stack_depth: u32,
    /// Nesting guard for float ranges, independent of `stack_depth`.
    float_depth: u32,
    /// The list counter for the `display:list-item` box about to be laid out.
    /// `flow_children` owns one counter per child run and stamps it here right
    /// before descending, so the marker code reads the right ordinal without
    /// threading it through every box-layout signature. A nested list can't
    /// clobber it: the inner list is laid out from inside the outer item's
    /// children, i.e. after its marker was already emitted.
    marker_ord: i32,
    /// CSS counter state (`counter-reset`/`-increment`, read by `counter()`).
    counters: Counters,
    /// When set, element boxes are recorded into `inspects` for the dev tool.
    inspect: bool,
    inspects: Vec<InspectBox>,
    /// Memoised `intrinsic_width` results, keyed by element `seq`. Measuring a
    /// subtree now cascades every descendant, and the same element is asked
    /// repeatedly (a table sizes its columns over several passes) — without
    /// this the cascade work would multiply.
    intrinsic: BTreeMap<u32, (f32, f32)>,
    /// Set while `measure_box_height` is resolving a positioned box's own
    /// containing-block height. That measurement re-enters the same box, which
    /// would ask for the same height again — one level is all the answer needs.
    measuring_cb_h: core::cell::Cell<bool>,
    /// Memoised `style::resolve` results, keyed by a hash of everything the
    /// cascade reads (see `style_key`) — so this is a pure cache, not a policy.
    /// A real article cascades the SAME element about twelve times: every
    /// throwaway measurement re-walks its subtree, and selector matching is
    /// ~90 % of layout, so that multiplier is most of the cost of a page.
    styles: core::cell::RefCell<BTreeMap<u64, ComputedStyle>>,
}

/// Hash the inputs `style::resolve` actually depends on. Elements are
/// identified by `seq` rather than by content, which makes the whole ancestor
/// chain and sibling list a handful of integer mixes.
///
/// `parent` is not hashed in full — only the inherited values a cascade can
/// read back (font size, colour, weight, direction). The chain of ancestor
/// `seq`s already determines which element the parent IS; the fingerprint is
/// there for the call sites that hand a cell the table's style rather than the
/// row's, so those cannot collide with each other.
fn style_key(el: &Element, parent: &ComputedStyle, ancestors: &[ElemInfo], prev: &[ElemInfo], sib_count: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(el.seq as u64);
    mix(sib_count as u64);
    mix(prev.len() as u64);
    for a in ancestors {
        mix(a.seq() as u64 | 0x1_0000_0000);
    }
    for p in prev {
        mix(p.seq() as u64 | 0x2_0000_0000);
    }
    mix(parent.font_px.to_bits() as u64);
    mix(parent.color.0 as u64 | (parent.color.1 as u64) << 8 | (parent.color.2 as u64) << 16);
    mix(parent.bold as u64 | (parent.italic as u64) << 1 | (parent.mono as u64) << 2 | (parent.rtl as u64) << 3);
    h
}

impl<'a> Ctx<'a> {
    /// `style::resolve` through the memo. Every cascade inside the layout goes
    /// through here so a re-measured subtree costs a map lookup, not a full
    /// selector match against the page's stylesheet.
    fn styled(&self, el: &Element, parent: &ComputedStyle, prev: &[ElemInfo], sib_count: u32) -> ComputedStyle {
        let key = style_key(el, parent, &self.path, prev, sib_count);
        if let Some(s) = self.styles.borrow().get(&key) {
            return *s;
        }
        let s = style::resolve(el, parent, self.theme, self.sheet, &self.path, prev, sib_count, self.viewport_w);
        self.styles.borrow_mut().insert(key, s);
        s
    }

    /// Whether `st` should open a new tracked stacking range right now: it is
    /// positioned, has an explicit `z-index`, and isn't already nested inside
    /// another tracked range.
    ///
    /// **`z-index: auto` must stay untracked.** A tracked range is our stand-in
    /// for a stacking context, and `auto` does not establish one: tracking a
    /// `position: relative` wrapper made it swallow its children's ranges, so
    /// their z-indexes stopped ordering against each other at all.
    fn should_track_stack(&self, st: &ComputedStyle) -> bool {
        st.position != Position::Static && matches!(st.z_index, ZIndex::Value(_)) && self.stack_depth == 0
    }

    /// Resolve percentage `height`/`min-`/`max-height` against the containing
    /// block ONCE, at the entry to laying the box out. Everything downstream
    /// then matches on `Len::Px` exactly as before — which is the point: the
    /// two earlier attempts at percentage heights each taught one code path to
    /// resolve them and measured WORSE, because the other paths still read the
    /// same box as `auto` and the two answers disagreed.
    ///
    /// Returns `None` when nothing needs resolving, so the common case does not
    /// copy a 1 kB `ComputedStyle`.
    fn resolve_pct_heights(&self, st: &ComputedStyle) -> Option<ComputedStyle> {
        let pct = |l: Len| matches!(l, Len::Pct(_) | Len::Calc { .. });
        if !(pct(st.height) || pct(st.min_height) || pct(st.max_height)) {
            return None;
        }
        let cbh = self.cb_h;
        // §10.5: against an indefinite containing block a percentage behaves as
        // `auto`. `min-height` is the exception the spec spells out — it falls
        // back to 0, which is its initial value anyway.
        let one = |l: Len, auto: Len| match l {
            Len::Pct(_) | Len::Calc { .. } => match vert_len(l, cbh.map(|h| h as i32)) {
                Some(v) => Len::Px(v.max(0.0)),
                None => auto,
            },
            other => other,
        };
        let mut out = *st;
        out.height = one(st.height, Len::Auto);
        out.min_height = one(st.min_height, Len::Px(0.0));
        out.max_height = one(st.max_height, Len::Auto);
        Some(out)
    }

    /// Record one box's emitted `ops[op_start..op_end]` / `links[link_start..
    /// link_end]` as its own stacking-order unit. Empty ranges are skipped.
    fn record_stack_entry(&mut self, z: i32, layer: i32, op_start: usize, op_end: usize, link_start: usize, link_end: usize) {
        if op_end > op_start {
            self.stack_ops.push((z, layer, op_start, op_end));
        }
        if link_end > link_start {
            self.stack_links.push((z, layer, link_start, link_end));
        }
    }

    /// Record `el`'s box `(x, y, w, h)` for the inspect dev tool. No-op unless
    /// inspection is enabled, so the label formatting cost is only paid when the
    /// user is actually inspecting.
    fn record_inspect(&mut self, el: &Element, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32) {
        if !self.inspect {
            return;
        }
        let mut label = el.tag.clone();
        if let Some(id) = el.attr("id") {
            label.push('#');
            label.push_str(id);
        }
        if let Some(cls) = el.attr("class") {
            for c in cls.split_whitespace().take(5) {
                label.push('.');
                label.push_str(c);
            }
        }
        label.push_str(&alloc::format!("  {w}×{h}  {}", display_name(st.display)));
        match st.float {
            FloatKind::Left => label.push_str(" float:left"),
            FloatKind::Right => label.push_str(" float:right"),
            FloatKind::None => {}
        }
        match st.position {
            Position::Relative => label.push_str(" position:relative"),
            Position::Absolute => label.push_str(" position:absolute"),
            Position::Fixed => label.push_str(" position:fixed"),
            Position::Sticky => label.push_str(" position:sticky"),
            Position::Static => {}
        }
        if st.hidden {
            label.push_str(" visibility:hidden");
        }
        if let Some(bg) = st.bg {
            label.push_str(&alloc::format!(" bg:#{:02x}{:02x}{:02x}", bg.0, bg.1, bg.2));
        }
        self.inspects.push(InspectBox { x, y, w, h, depth: self.path.len() as u16, label });
    }
}

/// A short name for a `Display` value, for the inspect label.
fn display_name(d: Display) -> &'static str {
    match d {
        Display::Block => "block",
        Display::Inline => "inline",
        Display::InlineBlock => "inline-block",
        Display::ListItem => "list-item",
        Display::Table => "table",
        Display::Flex => "flex",
        Display::Grid => "grid",
        Display::None => "none",
        _ => "table-part",
    }
}

/// Stable-reorder `items` so the tracked `(z_index, start, end)` ranges sort
/// by `z_index` (negative before, positive after), while every byte NOT
/// covered by a range — and any range at `z_index == 0` — keeps its original
/// relative position (a plain stable sort with untracked spans implicitly
/// keyed `0`). Ranges must be non-overlapping (guaranteed by `stack_depth`
/// gating at collection time).
/// Paint layers WITHIN one z-index (CSS2.1 Appendix E, steps 3 and 4): in-flow
/// block boxes, then non-positioned floats. Untracked spans of the display list
/// are in-flow content and take layer 0, which is what lifts a float above the
/// block backgrounds and borders emitted after it. A `z-index: 0` box stays in
/// layer 0 too: Appendix E would paint it above floats (step 6), but hoisting
/// it above in-flow content it merely follows in the document breaks more than
/// the overlap it fixes.
/// In-flow content — every untracked span of the list, and an explicit
/// `z-index` range, which orders by its `z` and keeps document order at 0.
const LAYER_IN_FLOW: i32 = 0;
/// Non-positioned floats: above the in-flow block boxes around them.
const LAYER_FLOAT: i32 = 1;

/// Merge the float ranges into the tracked z-index ranges so `reorder_by_z`
/// still sees a disjoint, ascending list. A float inside a tracked range would
/// otherwise overlap it — so that range is CUT around the float: the pieces
/// keep the parent's `(z, layer)` and the float becomes `(parent z, float
/// layer)`, which sorts it to the end of that parent's group and nowhere else.
/// A float outside every range is simply `(0, float layer)`.
fn split_float_ranges(
    stacks: &[(i32, i32, usize, usize)],
    floats: &[(usize, usize)],
) -> Vec<(i32, i32, usize, usize)> {
    if floats.is_empty() {
        return stacks.to_vec();
    }
    let mut out: Vec<(i32, i32, usize, usize)> = Vec::with_capacity(stacks.len() + floats.len() * 2);
    let mut taken = alloc::vec![false; floats.len()];
    for &(z, layer, s, e) in stacks {
        // Tracked ranges never nest (see `should_track_stack`), so each float
        // lands in at most one of them.
        let mut inner: Vec<(usize, usize)> = Vec::new();
        for (i, &(fs, fe)) in floats.iter().enumerate() {
            if !taken[i] && fs >= s && fe <= e {
                taken[i] = true;
                inner.push((fs, fe));
            }
        }
        inner.sort_unstable();
        let mut cursor = s;
        for (fs, fe) in inner {
            if fs > cursor {
                out.push((z, layer, cursor, fs));
            }
            out.push((z, LAYER_FLOAT, fs, fe));
            cursor = fe;
        }
        if cursor < e {
            out.push((z, layer, cursor, e));
        }
    }
    for (i, &(fs, fe)) in floats.iter().enumerate() {
        if !taken[i] {
            out.push((0, LAYER_FLOAT, fs, fe));
        }
    }
    out.sort_unstable_by_key(|r| r.2);
    out
}

fn reorder_by_z<T>(items: Vec<T>, ranges: &[(i32, i32, usize, usize)]) -> Vec<T> {
    if ranges.is_empty() {
        return items;
    }
    let mut sorted_ranges = ranges.to_vec();
    sorted_ranges.sort_by_key(|r| r.2); // by start — already ascending, but be safe
    let mut it = items.into_iter();
    let mut cursor = 0usize;
    let mut blocks: Vec<((i32, i32), Vec<T>)> = Vec::new();
    for (z, layer, start, end) in sorted_ranges {
        if start > cursor {
            blocks.push(((0, 0), (&mut it).take(start - cursor).collect()));
        }
        blocks.push(((z, layer), (&mut it).take(end - start).collect()));
        cursor = end;
    }
    let rest: Vec<T> = it.collect();
    if !rest.is_empty() {
        blocks.push(((0, 0), rest));
    }
    // Stable: blocks with the same (z, layer) — all the untracked in-flow spans,
    // and any explicit `z-index: 0` — keep the relative order built above.
    blocks.sort_by_key(|(k, _)| *k);
    blocks.into_iter().flat_map(|(_, v)| v).collect()
}

/// Lay a document out into a scroll-independent display list.
pub fn layout(
    fonts: &crate::fonts::Fonts,
    dom: &Dom,
    sheet: &Stylesheet,
    images: &ImageMap,
    width: u32,
    viewport_h: u32,
    theme: &Theme,
    forms: &FormState,
    inspect: bool,
) -> Layout {
    // The root element is never painted, but `html { … }` still cascades into
    // the document — and its `font-size` is the basis for every `rem`.
    let mut initial = ComputedStyle::root(theme);
    // Seed the viewport before the first cascade: `vw`/`vh` on `html` itself
    // have to resolve, and every descendant inherits these two down.
    initial.vw = width as f32;
    initial.vh = viewport_h as f32;
    let html_el = dom.root_element();
    let mut root = style::resolve(html_el, &initial, theme, sheet, &[], &[], 0, width as f32);
    root.rem_base = root.font_px;
    let cx = 0;
    let cw = (width as i32).max(60);
    let mut ctx = Ctx {
        fonts,
        theme,
        sheet,
        images,
        guessed: core::cell::RefCell::new(Vec::new()),
        css_images: core::cell::RefCell::new(Vec::new()),
        ops: Vec::new(),
        links: Vec::new(),
        controls: Vec::new(),
        forms,
        path: Vec::new(),
        // Initial containing block: the viewport, anchored at the CANVAS
        // origin (CSS2.1 §10.1) — not at the page's content box. `left: 100px`
        // on a box with no positioned ancestor means 100px from the window
        // edge, whatever inset the page content sits at. Its height is
        // definite, which is what makes `top:0; bottom:0` on a root-level
        // abspos box stretch to the window rather than collapse.
        cb: (0, 0, width as i32, Some(viewport_h as i32)),
        viewport_w: width as f32,
        abs_count: 0,
        fixed_count: 0,
        cb_h: Some(viewport_h as f32),
        last_baseline: None,
        floats: Vec::new(),
        stack_ops: Vec::new(),
        stack_links: Vec::new(),
        float_ops: Vec::new(),
        float_links: Vec::new(),
        stack_depth: 0,
        float_depth: 0,
        marker_ord: 0,
        counters: Counters::default(),
        inspect,
        inspects: Vec::new(),
        intrinsic: BTreeMap::new(),
        measuring_cb_h: core::cell::Cell::new(false),
        styles: core::cell::RefCell::new(BTreeMap::new()),
    };

    // Resolve <body> for the canvas-background rule below; layout reaches it
    // as an ordinary child of the root.
    let body = dom.body();
    let html_info = [ElemInfo::of(html_el)];
    let anc: &[ElemInfo] = if core::ptr::eq(html_el, body) { &[] } else { &html_info };
    let body_style = style::resolve(body, &root, theme, sheet, anc, &[], 0, width as f32);

    // The ROOT ELEMENT IS A BOX. It used to be skipped — layout started at
    // `<body>`'s children, inside a hardcoded 20px page inset — so `html
    // { position: absolute }`, its border, its width and `<body>`'s own margin
    // all meant nothing. Laying it out like any other block is what makes the
    // whole `abspos-containing-block-initial` family measurable, and it is
    // where the page inset now comes from: `<body>`'s UA margin.
    // NOTE: 0.3.13 resolved a percentage `height` on the root against the
    // viewport here — the ICB's height IS definite, so it looked right. It was
    // measured OUT again in 0.3.14: it fixed none of the two tests it was
    // added for, cost `abspos-containing-block-006`, and truncated every page
    // that writes the everyday `html { height: 100% }` to one viewport, which
    // stopped scrolling dead. Percentage heights belong with general
    // percentage-height support, not as a special case for the root.
    let mut y;
    if root.display == Display::None {
        // `html { display: none }` — the root generates no box, so the document
        // renders nothing at all (`root-box-003`). Only the canvas keeps its
        // propagated background.
        y = 0;
    } else if core::ptr::eq(html_el, body) {
        // A document with no `<html>` at all: the synthetic container is both.
        ctx.path.push(ElemInfo::of(body));
        y = ctx.layout_children(&body.children, &body_style, Some(body), cx, cw, 0);
    } else {
        ctx.path.push(ElemInfo::of(html_el));
        if matches!(root.position, Position::Absolute | Position::Fixed) {
            // An out-of-flow root resolves against the ICB like any other
            // out-of-flow box — it just has no in-flow position to fall back to.
            ctx.layout_abs(html_el, &root, cx, 0);
            y = viewport_h as i32;
        } else {
            y = ctx.layout_box(html_el, &root, cx, cw, 0);
        }
        ctx.path.pop();
    }
    // A float can extend below the last in-flow line — grow the page to contain it.
    let float_bottom = ctx.floats.iter().map(|f| f.bottom).max().unwrap_or(0);
    y = y.max(float_bottom);
    // The page's scrollable height is how far the PAINTED content reaches, not
    // where the root box ends. `html { height: 100% }` is an everyday idiom and
    // it makes the root box exactly one viewport tall — everything below it
    // still scrolls in every browser. Taking the root's border-box bottom alone
    // truncated such a page to the window and killed scrolling outright.
    let painted_bottom = ctx.ops.iter().map(op_bottom).max().unwrap_or(0);
    y = y.max(painted_bottom);

    // The body's background propagates to the whole canvas (a bare `<body
    // background>` fills the viewport, not just the body box).
    // Canvas background (CSS 2.1 §14.2): the ROOT element's background is
    // propagated to the canvas; `<body>`'s is used only when the root's is
    // transparent. Honouring `html { color }` without this paints white text
    // on a white canvas for every "this page should be green" reftest.
    let canvas_bg = root.bg.or(body_style.bg).unwrap_or(theme.bg);
    // z-index stacking order (CSS2.1 §9.9 / Appendix E): reorder the flat,
    // tree-order display list so negative-z ranges paint first (behind) and
    // positive-z ranges paint last (in front) of everything else.
    #[cfg(feature = "diag-boxes")]
    {
        extern crate std;
        std::eprintln!("[stack] ops={} ranges={:?} floats={:?}", ctx.ops.len(), ctx.stack_ops, ctx.float_ops);
    }
    let op_ranges = split_float_ranges(&ctx.stack_ops, &ctx.float_ops);
    let link_ranges = split_float_ranges(&ctx.stack_links, &ctx.float_links);
    let ops = reorder_by_z(ctx.ops, &op_ranges);
    let links = reorder_by_z(ctx.links, &link_ranges);
    Layout {
        ops,
        links,
        controls: ctx.controls,
        height: y.max(1) as u32,
        bg: canvas_bg,
        guessed_image_srcs: ctx.guessed.into_inner(),
        css_image_keys: ctx.css_images.into_inner(),
        css_image_srcs: Vec::new(),
        inspect: ctx.inspects,
    }
}

impl<'a> Ctx<'a> {
    /// Narrow an x-range `[cl, cr]` by any active floats overlapping the
    /// vertical band `[top, bot)`. Returns the (left, right) available there.
    fn float_band(&self, top: i32, bot: i32, cl: i32, cr: i32) -> (i32, i32) {
        band_of(&self.floats, top, bot, cl, cr)
    }

    /// Position a block that establishes a new BFC so its border box does not
    /// overlap active floats (CSS2.1 §9.5): shift it into the widest available
    /// band at its top, dropping below any float a definite width can't fit
    /// beside. Returns the adjusted (margin-box left, available width, top).
    fn avoid_floats_bfc(&self, st: &ComputedStyle, x: i32, w: i32, y: i32) -> (i32, i32, i32) {
        if self.floats.is_empty() {
            return (x, w, y);
        }
        let ml = st.margin_left.px(w as f32).unwrap_or(0.0).max(0.0);
        let mr = st.margin_right.px(w as f32).unwrap_or(0.0).max(0.0);
        // Outer (margin-box) width a definite width demands; `auto` fills the band.
        let need = match st.width {
            Len::Auto => None,
            other => other.px(w as f32).map(|v| {
                let border = if st.box_border {
                    v
                } else {
                    v + st.pad_left + st.pad_right + st.border_x()
                };
                ceil_i32(border + ml + mr)
            }),
        };
        let mut by = y;
        loop {
            let (bl, br) = self.float_band(by, by + 1, x, x + w);
            let avail = br - bl;
            let fits = match need {
                Some(n) => n <= avail,
                None => true,
            };
            if fits || avail >= w {
                break;
            }
            let next = self.floats.iter().filter(|f| f.bottom > by).map(|f| f.bottom).min();
            match next {
                Some(nb) if nb > by => by = nb,
                _ => break,
            }
        }
        let (bl, br) = self.float_band(by, by + 1, x, x + w);
        (bl.max(x), (br - bl).max(1), by)
    }

    /// The y at or below which floats on the cleared side(s) no longer intrude.
    fn clear_below(&self, clear: ClearKind, y: i32) -> i32 {
        let mut ny = y;
        for f in &self.floats {
            let hit = match clear {
                ClearKind::Both => true,
                ClearKind::Left => f.is_left,
                ClearKind::Right => !f.is_left,
                ClearKind::None => false,
            };
            if hit {
                ny = ny.max(f.bottom);
            }
        }
        ny
    }

    /// Place a `float:left|right` box (CSS2.1 §9.5.1). Computes the float's
    /// margin-box width (shrink-to-fit for `auto`), finds the highest position
    /// where that margin box fits beside earlier floats on either side (dropping
    /// below the ones it can't fit beside), lays the box out isolated in its own
    /// BFC, and records its margin box as an exclusion rect. Does not advance
    /// normal flow. `x`/`w` are the BFC content box; `y` the static flow top.
    fn place_float(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) {
        let is_left = st.float == FloatKind::Left;
        let ml = st.margin_left.px(w as f32).unwrap_or(0.0).max(0.0);
        let mr = st.margin_right.px(w as f32).unwrap_or(0.0).max(0.0);
        let pad_border = st.pad_left + st.pad_right + st.border_x();
        // Content width: shrink-to-fit for `auto` (min(max(min-content, avail),
        // preferred)); a definite width is used directly (may overflow the CB).
        let content_w = match st.width {
            Len::Auto => {
                let (pref, min) = self.intrinsic_width(el, st);
                let avail = (w as f32 - ml - mr - pad_border).max(0.0);
                pref.min(avail).max(min).max(0.0)
            }
            other => {
                let v = other.px(w as f32).unwrap_or(0.0);
                if st.box_border { (v - pad_border).max(0.0) } else { v }
            }
        };
        // Margin-box outer width (never below 1px, never the whole CB for a
        // shrink-to-fit float, but a definite width may exceed the CB).
        let fw = (ceil_i32(content_w + pad_border + ml + mr)).max(1);
        // Float margins never collapse: the margin box top is the static flow
        // position `y`. `clear` applies to floats as well (CSS2.1 §9.5.2), so
        // first drop below every earlier float on the cleared side — without
        // it Wikipedia's `clear:right` article thumbnails wedge in beside the
        // infobox instead of below it, squeezing the text to a few characters
        // per line. Then drop further until the margin box actually fits.
        let mut fy = self.clear_below(st.clear, y).max(y);
        loop {
            let (bl, br) = self.float_band(fy, fy + 1, x, x + w);
            if fw <= br - bl || br - bl >= w {
                break;
            }
            let next = self
                .floats
                .iter()
                .filter(|f| f.bottom > fy)
                .map(|f| f.bottom)
                .min();
            match next {
                Some(nb) if nb > fy => fy = nb,
                _ => break,
            }
        }
        #[cfg(feature = "diag-boxes")]
        {
            extern crate std;
            let who = el.attr("class").unwrap_or(&el.tag);
            std::eprintln!("[float] {who}: static_y={y} placed_fy={fy} fw={fw} x={x} w={w} floats={}", self.floats.len());
        }
        let (bl, br) = self.float_band(fy, fy + 1, x, x + w);
        // Margin-box left edge: left floats pack left, right floats pack right.
        let mbox_left = if is_left { bl } else { (br - fw).max(bl) };
        // The border box sits below the margin box top by `margin-top`.
        let border_top = fy + st.margin_top as i32;
        self.path.push(ElemInfo::of(el));
        // The float's own contents establish a new BFC — isolate its inner floats.
        let saved = core::mem::take(&mut self.floats);
        // `layout_box` re-adds margin-left + padding from `mbox_left`; passing the
        // margin-box width lets an `auto`-width child fill the shrink-to-fit box.
        let border_bottom = self.layout_box(el, st, mbox_left, fw, border_top);
        self.record_inspect(el, st, mbox_left + ml as i32, border_top, (fw as f32 - ml - mr) as i32, border_bottom - border_top);
        self.floats = saved;
        self.path.pop();
        self.floats.push(FloatRect {
            left: mbox_left,
            right: mbox_left + fw,
            top: fy,
            bottom: border_bottom + st.margin_bottom as i32,
            is_left,
        });
    }

    /// Lay `nodes` as an independent block formatting context (a table cell,
    /// grid item, the page root, …). Returns the y below the last child, with
    /// the last in-flow block's bottom margin committed — margins do not
    /// collapse out of an established BFC. `owner` is the element these nodes
    /// belong to, for `::before`/`::after` generated content — `None` for an
    /// anonymous box (CSS2.1 §17.2.1 table objects): an anonymous box has no
    /// source element, so it cannot be selected and cannot generate one.
    fn layout_children(&mut self, nodes: &'a [Node], parent: &ComputedStyle, owner: Option<&Element>, x: i32, w: i32, y0: i32) -> i32 {
        let flow = self.flow_children(nodes, parent, owner, x, w, y0, Collapse::default());
        flow.bottom + flow.open.value() as i32
    }

    /// Block formatting: lay `nodes` as a vertical stack, grouping consecutive
    /// inline-level content into line boxes and collapsing adjoining vertical
    /// margins (CSS2.1 §8.3.1). `anchor_y` is the collapse edge at entry (the
    /// bottom of the previous content); `incoming` is any margin already open
    /// there (e.g. a parent's top margin collapsing into its first child).
    fn flow_children(
        &mut self,
        nodes: &'a [Node],
        parent: &ComputedStyle,
        owner: Option<&Element>,
        x: i32,
        w: i32,
        anchor_y: i32,
        incoming: Collapse,
    ) -> Flow {
        let mut anchor = anchor_y; // bottom of last committed content
        let mut open = incoming; // adjoining margin not yet committed
        let mut committed = false;
        let mut first_top = anchor_y;
        // Counter scope: any counter a child of this run resets lives until this
        // child list ends (its descendants + following siblings). Truncate the
        // stack back to here on the way out (css-lists-3 §4.4 scope boundary).
        let counter_base = self.counters.stack.len();
        let mut inline = Inline::new();
        // `owner::before` — an anonymous inline box carrying its `content`
        // string, inserted ahead of `owner`'s real children (CSS2.1 §12.1).
        // An anonymous `owner` (a table object with no source element) can't
        // be selected, so it can't generate one.
        if let Some(owner) = owner {
            if let Some(b) = self.pseudo_box(owner, parent, PseudoElem::Before, w) {
                inline.atomic(b);
            } else if let Some((text, ps)) = self.pseudo(owner, parent, PseudoElem::Before) {
                inline.text(&text, &ps, None);
            }
        }
        // Preceding element siblings (document order) for `+`/`~` combinators,
        // and the total element-sibling count for `:nth-child`/`:last-child`.
        let mut siblings: Vec<ElemInfo> = Vec::new();
        let sib_count = nodes.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        // `<ol start="n">` seeds the list counter; the first item lands on `n`.
        let mut list_ord: i32 = owner
            .filter(|o| o.tag == "ol")
            .and_then(|o| o.attr("start"))
            .and_then(|s| s.trim().parse::<i32>().ok())
            .map(|n| n - 1)
            .unwrap_or(0);

        // A run of `table-row`/`-row-group`/`-header-group`/`-footer-group`/
        // `-cell` siblings found here (not already inside table/row layout)
        // has no `table` ancestor: CSS2.1 §17.2.1 wraps the whole run in one
        // anonymous `table` box rather than laying each part out as an
        // ordinary block.
        let segs = self.segment_table_runs(nodes, parent);
        for seg in &segs {
            let node = match seg {
                TableSeg::Table(run) => {
                    let anon_st = style::anon_inherit(parent, Display::Table);
                    let mut t = open;
                    t.add(anon_st.margin_top);
                    let by = anchor + t.value() as i32;
                    let (bx, bw, byy) = self.avoid_floats_bfc(&anon_st, x, w, by);
                    let saved = core::mem::take(&mut self.floats);
                    let bottom = self.layout_table_body(run, &anon_st, bx, bw, byy);
                    self.floats = saved;
                    if !committed {
                        first_top = byy;
                        committed = true;
                    }
                    anchor = bottom;
                    open = Collapse::one(anon_st.margin_bottom);
                    continue;
                }
                TableSeg::Node(node) => *node,
            };
            let el = match node {
                Node::Text(t) => {
                    inline.text(t, parent, None);
                    continue;
                }
                Node::Element(el) => el,
            };
            let st = self.styled(el, parent, &siblings, sib_count);
            siblings.push(ElemInfo::of(el));
            if st.display == Display::None {
                continue;
            }
            // Apply this element's counter-reset/-increment before laying it
            // out, so its `::before`/`::after` and descendants see the updated
            // values. Depth = its ancestor count (it is not yet on `path`).
            self.counters.enter(&st, self.path.len());
            if st.display == Display::ListItem {
                // `<li value="n">` restarts the counter at n (HTML §4.4.8).
                list_ord = el
                    .attr("value")
                    .and_then(|v| v.trim().parse::<i32>().ok())
                    .unwrap_or(list_ord + 1);
                self.marker_ord = list_ord;
            }
            // `<img>` is an atomic inline box: add it to the current inline run
            // (a lone `<img>` flows as one item → its own line; an `<img>` in an
            // `<a>`/`<span>` flows with the text). Nested imgs are handled in
            // `collect_inline`; this catches direct children of any display.
            if el.tag == "img" {
                self.path.push(ElemInfo::of(el));
                let (iw, ih) = self.img_box(el, &st);
                let alt = el.attr("alt").unwrap_or("").trim().to_string();
                let src = el.attr("src").unwrap_or("").to_string();
                inline.image(src, iw, ih, None, alt, st.hidden, st.transparent, self.image_deco(&st));
                self.path.pop();
                continue;
            }
            // Every other replaced element is an atomic inline box too, and one
            // that lays out through the block model — same as an `inline-block`,
            // which is what `inline_block_box` builds. (A floated or out-of-flow
            // one was blockified in `styled`, so it never reaches here.)
            if st.display == Display::Inline && replaced_intrinsic(el).is_some() {
                if let Some(b) = self.inline_block_box(el, &st, w) {
                    inline.atomic(b);
                }
                continue;
            }
            // Form controls are atomic inline boxes too — and their children
            // (a `<button>`'s label, a `<select>`'s options) never lay out as
            // page content. Same treatment in `collect_inline`, since most
            // controls sit inside inline context.
            if let Some(kind) = crate::forms::kind_of(el) {
                if kind == ControlKind::Hidden {
                    continue;
                }
                // An absolutely-positioned control is out of flow, like any
                // other abspos box — the checkbox-hack toggle overlay
                // (`position:absolute; width:100%; height:100%; opacity:0`)
                // must NOT advance the line, or its full-size box inflates
                // the container by the whole page height.
                if matches!(st.position, Position::Absolute | Position::Fixed) {
                    self.path.push(ElemInfo::of(el));
                    self.layout_abs(el, &st, x, anchor + open.value() as i32);
                    self.path.pop();
                    continue;
                }
                // A control the page made BLOCK-LEVEL falls through to the
                // block path below, which paints it without a line box.
                //
                // An atomic inline sits on the baseline, so its parent comes out
                // the control's height PLUS the descender — 2px on a 32px field.
                // That is what doubles the bottom rule of a search box whose
                // wrapper is pulled onto the group's border with `margin: -1px`:
                // ours drew the field's edge 2px above the group's, where every
                // browser has them coincide.
                //
                // Gated on a definite width, because this path takes the
                // caller's width: `display:block` on a control means full width
                // only when the page also asked for it, which the `display:block;
                // width:100%` idiom (Codex, Bootstrap) always does. A bare
                // block-level control keeps its intrinsic width as an inline.
                let block_level = matches!(st.display, Display::Block | Display::Flex | Display::Grid)
                    && !matches!(st.width, Len::Auto);
                if !block_level {
                    self.path.push(ElemInfo::of(el));
                    let ctl = self.control_box(el, &st, kind, w as f32);
                    inline.control(ctl);
                    self.path.pop();
                    continue;
                }
            }
            // `position:absolute`/`fixed` are out of flow → laid at a
            // containing-block-relative position, not advancing the flow.
            if matches!(st.position, Position::Absolute | Position::Fixed) {
                self.path.push(ElemInfo::of(el));
                self.layout_abs(el, &st, x, anchor + open.value() as i32);
                self.path.pop();
                continue;
            }
            // `float:left|right` — out of normal flow, placed at the current
            // flow edge; following inline + blocks flow around it.
            if st.float != FloatKind::None {
                // The float's margin-box top is its STATIC position, which is
                // below the margin still open from the preceding block — a
                // float doesn't collapse with it, but it doesn't ignore it
                // either. `open` stays untouched: the float is out of flow, so
                // the next in-flow block still collapses through it.
                // A float paints ABOVE the in-flow block boxes around it
                // (Appendix E steps 3/4). Recording its range is what stops a
                // later sibling's border — MediaWiki's `div.mw-heading` rule,
                // say — from being drawn across it. `stack_depth` bounds the
                // NESTING (a float inside a float is covered by the outer one),
                // not whether we record at all: the enclosing z-index range, if
                // any, gets cut around this one at the end.
                let track = self.float_depth == 0;
                let (fop0, flink0) = (self.ops.len(), self.links.len());
                if track {
                    self.float_depth += 1;
                }
                self.place_float(el, &st, x, w, anchor + open.value() as i32);
                if track {
                    self.float_depth -= 1;
                    if self.ops.len() > fop0 {
                        self.float_ops.push((fop0, self.ops.len()));
                    }
                    if self.links.len() > flink0 {
                        self.float_links.push((flink0, self.links.len()));
                    }
                }
                continue;
            }
            if matches!(st.display, Display::Inline | Display::InlineBlock) {
                let ib = self.inline_box_of(el, &st, w).map(|b| inline.open_box(b));
                self.path.push(ElemInfo::of(el));
                self.collect_inline(el, &st, None, &mut inline, x, w, anchor);
                self.path.pop();
                if let Some(i) = ib {
                    inline.close_box(i);
                }
                continue;
            }
            // Block-level, in normal flow. Flush pending inline content first —
            // a line box separates margins, so the open margin commits here.
            if !inline.is_empty() {
                let ly = anchor + open.value() as i32;
                let nb = inline.flow(self.fonts, self.theme, x, w, ly, &self.floats, parent.text_align, parent.text_align_last, parent.rtl, parent.text_indent.px(w as f32).unwrap_or(0.0), parent.line_height.px(parent.font_px).unwrap_or(0.0), &mut self.ops, &mut self.links, &mut self.controls, &mut self.inspects, &mut self.last_baseline);
                if !committed {
                    first_top = ly;
                    committed = true;
                }
                anchor = nb;
                open = Collapse::default();
                inline = Inline::new();
            }
            // `clear` introduces clearance, dropping the block below the floats
            // and separating margins: commit the open margin, then clear.
            if st.clear != ClearKind::None {
                let base = anchor + open.value() as i32;
                let cleared = self.clear_below(st.clear, base);
                if cleared > base {
                    anchor = cleared;
                    open = Collapse::default();
                }
            }
            self.path.push(ElemInfo::of(el));
            let op0 = self.ops.len();
            let link0 = self.links.len();
            let ctl0 = self.controls.len();
            // An explicit `z-index` on a positioned (relative/sticky) box
            // opens a tracked stacking range (CSS2.1 §9.9), same as abspos —
            // unless already nested inside another tracked range.
            let track = self.should_track_stack(&st);
            if track {
                self.stack_depth += 1;
            }
            // A block that establishes a new BFC (flex/grid/table) keeps its
            // border box clear of active floats (CSS2.1 §9.5) and does not
            // collapse its margins with its children; its top margin still
            // collapses with the preceding flow, its bottom margin stays open.
            // A form control is atomic: it takes the box-making path so
            // `layout_box` paints it as a CONTROL. Without this a block-level
            // control fell into `flow_block_impl` and was laid out as an
            // ordinary block — CSS border, no face, no value, no placeholder.
            let out = if establishes_bfc(&st) || crate::forms::kind_of(el).is_some() {
                let mut t = open;
                t.add(st.margin_top);
                let by = anchor + t.value() as i32;
                let (bx, bw, byy) = self.avoid_floats_bfc(&st, x, w, by);
                let saved = core::mem::take(&mut self.floats);
                let bottom = self.layout_box(el, &st, bx, bw, byy);
                self.record_inspect(el, &st, bx, byy, bw, bottom - byy);
                self.floats = saved;
                BoxOut { bottom, top_y: byy, open: Collapse::one(st.margin_bottom), through: false, box_x: bx, box_w: bw }
            } else {
                let o = self.flow_block_impl(el, &st, x, w, anchor, open, false);
                if !o.through {
                    // The box's OWN border box. Reporting the containing
                    // block's `x`/`w` here made every device report about a
                    // centred or max-width container wrong: MediaWiki's
                    // `.mw-page-container` (max-width 99.75rem, margin 0 auto)
                    // paints 1596 px wide at x=162 and was reported as
                    // 1920 wide at x=0.
                    self.record_inspect(el, &st, o.box_x, o.top_y, o.box_w, o.bottom - o.top_y);
                }
                o
            };
            if track {
                self.stack_depth -= 1;
            }
            // `position:relative` stays in flow but its paint shifts by top/left.
            if st.position == Position::Relative {
                let (dx, dy) = rel_offset(&st, w as f32);
                if dx != 0 || dy != 0 {
                    self.shift_ops(op0, self.ops.len(), link0, self.links.len(), ctl0, dx, dy);
                }
            }
            // `transform: translate(...)` — the same paint-time shift, but its
            // percentages are of the BOX, not the containing block.
            let (tdx, tdy) = translate_offset(&st, out.box_w, out.bottom - out.top_y);
            if tdx != 0 || tdy != 0 {
                self.shift_ops(op0, self.ops.len(), link0, self.links.len(), ctl0, tdx, tdy);
            }
            if track {
                if let ZIndex::Value(z) = st.z_index {
                    self.record_stack_entry(z, LAYER_IN_FLOW, op0, self.ops.len(), link0, self.links.len());
                }
            }
            self.path.pop();
            if out.through {
                // Nothing committed: the box's margins stay adjoining.
                open = out.open;
            } else {
                if !committed {
                    first_top = out.top_y;
                    committed = true;
                }
                anchor = out.bottom;
                open = out.open;
            }
        }
        // `owner::after` — appended behind the real children, before the
        // final line-box flush so it shares a line with trailing inline
        // content (or starts its own, if the last child was block-level).
        if let Some(owner) = owner {
            if let Some(b) = self.pseudo_box(owner, parent, PseudoElem::After, w) {
                inline.atomic(b);
            } else if let Some((text, ps)) = self.pseudo(owner, parent, PseudoElem::After) {
                inline.text(&text, &ps, None);
            }
        }
        if !inline.is_empty() {
            let ly = anchor + open.value() as i32;
            let nb = inline.flow(self.fonts, self.theme, x, w, ly, &self.floats, parent.text_align, parent.text_align_last, parent.rtl, parent.text_indent.px(w as f32).unwrap_or(0.0), parent.line_height.px(parent.font_px).unwrap_or(0.0), &mut self.ops, &mut self.links, &mut self.controls, &mut self.inspects, &mut self.last_baseline);
            if !committed {
                first_top = ly;
                committed = true;
            }
            anchor = nb;
            open = Collapse::default();
        }
        // Leave the counter scope this child list opened.
        self.counters.stack.truncate(counter_base);
        Flow { bottom: anchor, open, first_top, committed }
    }

    /// `owner`'s `::before`/`::after` generated box, if `owner`'s own cascade
    /// (already resolved as `own`) has a matching rule with a supported
    /// `content` string. `self.path`'s last entry is always `owner` itself at
    /// every call site (the uniform `path.push(ElemInfo::of(el))` before any
    /// box-laying call), so its ancestors are everything before that.
    fn pseudo(&self, owner: &Element, own: &ComputedStyle, kind: PseudoElem) -> Option<(String, ComputedStyle)> {
        let (text, ps) = self.pseudo_content(owner, own, kind)?;
        // Only a plain inline generated element is a text run. A box-shaped one
        // is `pseudo_box`'s job, and anything else (`display: none`, the
        // table-internal roles) produces nothing at all.
        (ps.display == Display::Inline).then_some((text, ps))
    }

    fn pseudo_content(&self, owner: &Element, own: &ComputedStyle, kind: PseudoElem) -> Option<(String, ComputedStyle)> {
        let anc = self.path.len().saturating_sub(1);
        let (template, ps) =
            style::resolve_pseudo(owner, own, self.theme, self.sheet, &self.path[..anc], &[], 0, self.viewport_w, kind)?;
        Some((self.render_content(owner, &template), ps))
    }

    /// Place an out-of-flow `::before`/`::after` now that its originating box's
    /// geometry is known. Its containing block is that box's PADDING box, so
    /// this can only run once the box is finished — which is why it hangs off
    /// the end of the block and flex paths rather than the child walk. Only for
    /// a POSITIONED owner: for a static one the containing block is some
    /// ancestor, and this box is not it.
    ///
    /// This is how a page underlines its active tab —
    /// `a::after { position: absolute; bottom: 0; left: 0; width: 100%;
    /// height: 2px }` — and how MediaWiki hangs the magnify icon off a thumb.
    fn place_abs_pseudos(&mut self, el: &Element, st: &ComputedStyle, bx: i32, by: i32, bw: i32, bh: i32) {
        if st.position == Position::Static {
            return;
        }
        let (px, py, pw, ph) = (
            bx + st.border_left.width as i32,
            by + st.border_top.width as i32,
            (bw - st.border_x() as i32).max(0),
            (bh - st.border_y() as i32).max(0),
        );
        for kind in [PseudoElem::Before, PseudoElem::After] {
            let Some((text, ps)) = self.pseudo_content(el, st, kind) else { continue };
            if !ps.is_generated_box()
                || !matches!(ps.position, Position::Absolute | Position::Fixed)
                || ps.hidden
                || ps.transparent
            {
                continue;
            }
            let (aw, ah) = (pw as f32, ph as f32);
            let frame_x = ps.pad_left + ps.pad_right + ps.border_x();
            let frame_y = ps.pad_top + ps.pad_bottom + ps.border_y();
            let font = self.fonts.pick(ps.bold, ps.italic, ps.mono);
            let cw = match ps.width.px(aw) {
                Some(v) if v >= 0.0 => v,
                _ => measure(font, text.trim(), ps.font_px),
            };
            let ch = match vert_len(ps.height, Some(ph)) {
                Some(v) if v >= 0.0 => v,
                _ if text.trim().is_empty() => 0.0,
                _ => line_gap(font, ps.font_px),
            };
            let (w, h) = ((cw + frame_x).max(0.0) as i32, (ch + frame_y).max(0.0) as i32);
            if w <= 0 || h <= 0 {
                continue;
            }
            // `left` wins over `right`; with neither the box sits at the
            // containing block's start edge (§10.3.7 with a static position we
            // do not track for generated content).
            let x = match (ps.left.px(aw), ps.right.px(aw)) {
                (Some(l), _) => px + l as i32,
                (None, Some(r)) => px + pw - w - r as i32,
                _ => px,
            };
            let y = match (vert_len(ps.top, Some(ph)), vert_len(ps.bottom, Some(ph))) {
                (Some(t), _) => py + t as i32,
                (None, Some(b)) => py + ph - h - b as i32,
                _ => py,
            };
            let mut ops: Vec<DrawOp> = Vec::new();
            bg_ops(&ps, self.bg_key(ps.bg_layer.image), self.bg_key(ps.mask_layer.image), x, y, w, h, &mut ops);
            border_ops(&ps, x, y, w, h, (true, true), &mut ops);
            if !text.trim().is_empty() {
                ops.push(DrawOp::Text {
                    x: x + (ps.border_left.width + ps.pad_left) as i32,
                    y: y + (ps.border_top.width + ps.pad_top) as i32,
                    size: ps.font_px,
                    color: ps.color,
                    bold: ps.bold,
                    italic: ps.italic,
                    mono: ps.mono,
                    text: text.trim().into(),
                });
            }
            self.ops.append(&mut ops);
        }
    }

    /// The finished rectangle of a `::before`/`::after` that carries a box of
    /// its own — the CSS-icon idiom, `content: ""` plus a size plus a
    /// `background-image`. Every layout path can place one of these: an inline
    /// run puts it on a line like an `inline-block`, a flex container reserves
    /// it at the start (or end) of its main axis.
    ///
    /// `width`/`height` come from the style when definite; otherwise the text
    /// decides, as for any shrink-to-fit box. Percentages resolve against
    /// `avail_w`.
    fn pseudo_box(&mut self, owner: &Element, own: &ComputedStyle, kind: PseudoElem, avail_w: i32) -> Option<AtomicBox> {
        let (text, ps) = self.pseudo_content(owner, own, kind)?;
        if !ps.is_generated_box() || ps.hidden || ps.transparent {
            return None;
        }
        // An out-of-flow generated box needs a containing block and offsets we
        // do not resolve for pseudo-elements yet. Placing it IN the flow puts
        // it somewhere it never belongs — MediaWiki underlines the active tab
        // with `a::after { position: absolute; bottom: 0; height: 2px }`, and
        // in-flow that draws a line straight through the tab's text. Produce
        // nothing rather than render it wrong.
        if matches!(ps.position, Position::Absolute | Position::Fixed) {
            return None;
        }
        let cbw = avail_w as f32;
        let frame_x = ps.pad_left + ps.pad_right + ps.border_x();
        let frame_y = ps.pad_top + ps.pad_bottom + ps.border_y();
        let font = self.fonts.pick(ps.bold, ps.italic, ps.mono);
        let cw = match ps.width.px(cbw) {
            Some(v) if v >= 0.0 => v,
            _ => measure(font, text.trim(), ps.font_px),
        };
        let ch = match ps.height.px(cbw) {
            Some(v) if v >= 0.0 => v,
            _ if text.trim().is_empty() => 0.0,
            _ => line_gap(font, ps.font_px),
        };
        let (ml, mr) = (
            ps.margin_left.px(cbw).unwrap_or(0.0).max(0.0),
            ps.margin_right.px(cbw).unwrap_or(0.0).max(0.0),
        );
        let (bw, bh) = ((cw + frame_x).max(0.0) as i32, (ch + frame_y).max(0.0) as i32);
        if bw <= 0 && bh <= 0 {
            return None;
        }
        let mut ops: Vec<DrawOp> = Vec::new();
        let bx = ml as i32;
        let by = ps.margin_top as i32;
        bg_ops(&ps, self.bg_key(ps.bg_layer.image), self.bg_key(ps.mask_layer.image), bx, by, bw, bh, &mut ops);
        border_ops(&ps, bx, by, bw, bh, (true, true), &mut ops);
        if !text.trim().is_empty() {
            ops.push(DrawOp::Text {
                x: bx + (ps.border_left.width + ps.pad_left) as i32,
                y: by + (ps.border_top.width + ps.pad_top) as i32,
                size: ps.font_px,
                color: ps.color,
                bold: ps.bold,
                italic: ps.italic,
                mono: ps.mono,
                text: text.trim().into(),
            });
        }
        let h = bh + (ps.margin_top + ps.margin_bottom) as i32;
        Some(AtomicBox {
            ops,
            links: Vec::new(),
            controls: Vec::new(),
            inspects: Vec::new(),
            w: bw + (ml + mr) as i32,
            h,
            baseline: h,
            valign: ps.valign,
        })
    }

    /// Resolve a `content` template to its final text, reading any
    /// `counter()`/`counters()` against the current counter scope and any
    /// `attr()` off `owner` — the element the pseudo-element hangs on.
    fn render_content(&self, owner: &Element, template: &[ContentPiece]) -> String {
        let mut out = String::new();
        for piece in template {
            match piece {
                ContentPiece::Text(s) => out.push_str(s),
                // A missing attribute is the empty string, not a dropped value
                // (CSS2.1 §12.2) — an empty `::before` box is still generated.
                ContentPiece::Attr(name) => out.push_str(owner.attr(name).unwrap_or("")),
                ContentPiece::Counter { name, style } => {
                    out.push_str(&format_counter(*style, self.counters.value(*name)))
                }
                ContentPiece::Counters { name, sep, style } => {
                    let vals = self.counters.values(*name);
                    // An out-of-scope counter is treated as a single 0.
                    if vals.is_empty() {
                        out.push_str(&format_counter(*style, 0));
                    } else {
                        for (i, v) in vals.iter().enumerate() {
                            if i > 0 {
                                out.push_str(sep);
                            }
                            out.push_str(&format_counter(*style, *v));
                        }
                    }
                }
            }
        }
        out
    }

    /// Lay one block-level box with the CSS block box model: resolve the
    /// horizontal box (margins incl. `auto`-centering, width, min/max-width,
    /// padding) within the containing block's content width `w`, add vertical
    /// padding, then lay the content. This is what makes `max-width` + `margin:
    /// 0 auto` **centered containers** work.
    fn layout_block(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // `isolated`: `y0` is the border-box top (the caller — a float, cell,
        // flex item, abs box — already positioned it and owns its margins), so
        // no parent/sibling margin collapsing applies to this box's own edges.
        self.flow_block_impl(el, st, x, w, y0, Collapse::default(), true).bottom
    }

    /// Lay one block-level box with the CSS block box model and margin
    /// collapsing. In flow (`isolated == false`), `base_y` is the collapse edge
    /// and `incoming` the open adjoining margin: the box's top margin collapses
    /// with them (and, if it has no top border/padding, with its first child);
    /// its bottom margin collapses with its last child (auto height) and is
    /// left open for the next sibling. When `isolated`, `base_y` is the
    /// border-box top and margins are committed, not propagated.
    fn flow_block_impl(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, base_y: i32, incoming: Collapse, isolated: bool) -> BoxOut {
        let resolved = self.resolve_pct_heights(st);
        let st = resolved.as_ref().unwrap_or(st);
        let (mut cw, off_left) = resolve_block_h(st, w as f32);
        // §10.3.4: a replaced element with `width: auto` takes its INTRINSIC
        // width. It does not fill its container the way a block box does, so
        // `resolve_block_h`'s auto-solve is the wrong answer for it.
        if let (Some((iw, _)), Len::Auto) = (replaced_intrinsic(el), st.width) {
            let frame = st.pad_left + st.pad_right + st.border_x();
            cw = clamp_len(iw + frame, st.min_width, st.max_width, st.box_border, frame) - frame;
        }
        let content_x = x + off_left as i32;
        let content_w = cw.max(1.0) as i32;

        let box_left = content_x - st.pad_left as i32 - st.border_left.width as i32;
        let box_w = content_w + (st.pad_left + st.pad_right) as i32 + st.border_x() as i32;
        let bg_idx = self.ops.len();
        let clip_marks = (bg_idx, self.abs_count, self.fixed_count);

        let bt = st.border_top.width as i32;
        let bb = st.border_bottom.width as i32;
        let pt = st.pad_top as i32;
        let pb = st.pad_bottom as i32;

        // Top margin. In flow it collapses with the incoming margin; a box with
        // no top border/padding also collapses it with its first child.
        let mut top = incoming;
        if !isolated {
            top.add(st.margin_top);
        }
        let collapse_top = !isolated && bt == 0 && pt == 0;
        // Provisional border-box top (exact unless the first child grows `top`).
        let prov_top_y = if isolated { base_y } else { base_y + top.value() as i32 };

        // Where children start, and what open margin flows into them.
        let (child_anchor, child_incoming) = if collapse_top {
            (base_y, top)
        } else {
            (prov_top_y + bt + pt, Collapse::default())
        };

        // `<hr>` renders a rule at the content top.
        if st.is_rule {
            let y = prov_top_y + bt + pt;
            if !st.hidden && !st.transparent {
                self.ops.push(DrawOp::Rect { x: content_x, y: y + 1, w: content_w.max(1), h: 1, color: self.theme.rule });
            }
            return BoxOut { bottom: y + 3 + pb, top_y: prov_top_y, open: Collapse::one(if isolated { 0.0 } else { st.margin_bottom }), through: false, box_x: box_left, box_w };
        }
        // The `display:list-item` marker box, outside the content edge.
        // `list-style-type:none` generates none at all — Wikipedia's nav/TOC
        // lists rely on that, and a bullet there is pure noise.
        if st.display == Display::ListItem && st.list_style != ListStyle::None && !st.hidden && !st.transparent {
            let top = prov_top_y + bt + pt;
            if st.list_style.is_bullet() {
                let s = 4;
                self.ops.push(DrawOp::Rect {
                    x: content_x - 12,
                    y: top + (st.font_px * 0.55) as i32,
                    w: s,
                    h: s,
                    color: self.theme.muted,
                });
            } else {
                // A counter marker is right-aligned against the content edge,
                // like every browser's `::marker` box.
                let label = marker_label(st.list_style, self.marker_ord);
                let mw = measure(self.fonts.pick(st.bold, st.italic, st.mono), &label, st.font_px);
                self.ops.push(DrawOp::Text {
                    x: content_x - 8 - ceil_i32(mw),
                    y: top,
                    size: st.font_px,
                    color: st.color,
                    bold: st.bold,
                    italic: st.italic,
                    mono: st.mono,
                    text: label,
                });
            }
        }

        // A positioned block becomes the containing block for `absolute`
        // descendants — its PADDING box (§10.1). `prov_top_y` is the border-box
        // top, so the padding edge is one border down.
        let prev_cb = self.cb;
        if st.position != Position::Static {
            let pad_top_y = prov_top_y + st.border_top.width as i32 + st.pad_top as i32;
            let mut cb = padding_cb(st, content_x, pad_top_y, content_w);
            // §10.1: the containing block for an absolutely positioned
            // descendant is this box's PADDING box — a USED height, definite
            // once laid out even when `height` is `auto`. That is a different
            // question from `cb_h` below, where §10.5 rightly leaves an auto
            // height indefinite for IN-FLOW children.
            //
            // Treating both as indefinite made `top: 50%` on an abspos child
            // unresolvable, so it fell back to its static position. With the
            // `top:50%` + `translate(-50%)` centring idiom that puts the box a
            // full box-height too low — which is where Wikipedia's search
            // magnifier ended up, half outside its `overflow:hidden` field.
            if cb.3.is_none() && !self.measuring_cb_h.get() {
                self.measuring_cb_h.set(true);
                // `measure_box_height` returns the box's BORDER-box height; the
                // containing block is its PADDING box, so the two borders come
                // off.
                let h = self.measure_box_height(el, st, content_x, content_w, prov_top_y);
                self.measuring_cb_h.set(false);
                cb.3 = Some((h - st.border_y() as i32).max(0));
            }
            self.cb = cb;
        }
        // This box's own content height is what a percentage height on a CHILD
        // resolves against — and only when it is definite. An `auto` height
        // depends on those very children, so it stays indefinite and their
        // percentages fall back to `auto` (§10.5).
        let prev_cb_h = self.cb_h;
        self.cb_h = content_height_of(st, st.height);
        let flow = if replaced_intrinsic(el).is_some() {
            // A replaced element's children are not page content — an
            // `<iframe>`'s fallback text, a `<video>`'s `<source>` list, a
            // `<canvas>`'s alternative. Nothing commits; the box is the whole
            // of it, and its height comes from the intrinsic size below.
            Flow { bottom: child_anchor, open: Collapse::default(), first_top: child_anchor, committed: false }
        } else if st.pre {
            let ly = child_anchor + child_incoming.value() as i32;
            let nb = layout_pre(self.fonts.pick(st.bold, st.italic, st.mono), el, st, content_x, content_w, ly, &mut self.ops);
            Flow { bottom: nb, open: Collapse::default(), first_top: ly, committed: true }
        } else {
            self.flow_children(&el.children, st, Some(el), content_x, content_w, child_anchor, child_incoming)
        };
        self.cb_h = prev_cb_h;
        self.cb = prev_cb;

        // Resolve the border-box top: when the top margin collapsed through, the
        // box's border box sits at the first committed child's border-box top.
        let border_top_y = if collapse_top && flow.committed { flow.first_top } else { prov_top_y };
        let content_top = border_top_y + bt + pt;

        // Explicit `height`/`min`/`max-height` (definite lengths only; `%` needs
        // a definite CB height we don't track). Border-box subtracts pad+border.
        let pad_v = pt + pb + bt + bb;
        let px_h = |len: Len| -> Option<i32> {
            match len {
                Len::Px(h) if st.box_border => Some((h as i32 - pad_v).max(0)),
                Len::Px(h) => Some(h as i32),
                _ => None,
            }
        };
        let out_bottom_margin = Collapse::one(if isolated { 0.0 } else { st.margin_bottom });

        if !flow.committed {
            // No in-flow content. Its explicit box height, if any.
            let mut ch = 0;
            if let Some(h) = px_h(st.height) {
                ch = h;
            } else if let Some((_, ih)) = replaced_intrinsic(el) {
                // §10.6.2: `height: auto` on a replaced element is its
                // intrinsic height, not the zero its (unrendered) content says.
                ch = ih as i32;
            } else if st.contain_size {
                // Size containment: content contributes no size, so an auto
                // height falls back to `contain-intrinsic-size`'s height.
                if let Some((_, ih)) = st.contain_intrinsic {
                    ch = ih as i32;
                }
            }
            if let Some(mn) = px_h(st.min_height) {
                ch = ch.max(mn);
            }
            if let Some(mx) = px_h(st.max_height) {
                ch = ch.min(mx);
            }
            // A box with no content, border, padding or height collapses through:
            // its top and bottom margins are adjoining.
            if collapse_top && bb == 0 && pb == 0 && ch == 0 {
                let mut open = top;
                open.merge(flow.open);
                open.add(st.margin_bottom);
                return BoxOut { bottom: base_y, top_y: base_y, open, through: true, box_x: box_left, box_w };
            }
            let box_bottom = border_top_y + bt + pt + ch + pb + bb;
            self.clip_overflow(st, clip_marks, box_left, border_top_y, box_w, box_bottom - border_top_y);
            self.paint_box_decoration(st, box_left, border_top_y, box_w, box_bottom - border_top_y, bg_idx);
            return BoxOut { bottom: box_bottom, top_y: border_top_y, open: out_bottom_margin, through: false, box_x: box_left, box_w };
        }

        // Box with committed content. The last child's trailing margin
        // (`flow.open`) collapses with this box's bottom margin only when the
        // box has auto height and no bottom border/padding separating them.
        let auto_height = !matches!(st.height, Len::Px(_));
        let collapse_bottom = !isolated && bb == 0 && pb == 0 && auto_height;
        let mut ch = (flow.bottom - content_top).max(0);
        let out_open;
        if collapse_bottom {
            // `min-height` taller than the content introduces space below it,
            // trapping the trailing margin inside the box.
            let mn = px_h(st.min_height).unwrap_or(0);
            if mn > ch {
                ch = (flow.bottom + flow.open.value() as i32 - content_top).max(0).max(mn);
                if let Some(mx) = px_h(st.max_height) {
                    ch = ch.min(mx);
                }
                out_open = out_bottom_margin;
            } else {
                if let Some(mx) = px_h(st.max_height) {
                    ch = ch.min(mx);
                }
                let mut o = flow.open;
                o.add(st.margin_bottom);
                out_open = o;
            }
        } else {
            // Bottom border/padding or a definite height: commit the trailing
            // child margin into the content box.
            ch = (flow.bottom + flow.open.value() as i32 - content_top).max(0);
            if let Some(h) = px_h(st.height) {
                ch = h;
            }
            if let Some(mn) = px_h(st.min_height) {
                ch = ch.max(mn);
            }
            if let Some(mx) = px_h(st.max_height) {
                ch = ch.min(mx);
            }
            out_open = out_bottom_margin;
        }
        let box_bottom = content_top + ch + pb + bb;
        self.clip_overflow(st, clip_marks, box_left, border_top_y, box_w, box_bottom - border_top_y);
        self.paint_box_decoration(st, box_left, border_top_y, box_w, box_bottom - border_top_y, bg_idx);
        BoxOut { bottom: box_bottom, top_y: border_top_y, open: out_open, through: false, box_x: box_left, box_w }
    }

    /// The decoded image (if any) + natural box size for an `<img>`: from the
    /// `width`/`height` attributes, else the decoded intrinsic size, else a
    /// fallback. Not clamped to the line width — `flow` fits it when placing.
    /// Size an `<img>` box.
    ///
    /// Also records, via `guessed`, whether this box had to guess:
    /// with both `width` and `height` given the geometry is definite and the
    /// pixels arriving later change nothing, so the shell can repaint instead
    /// of re-laying-out. Without them the box depends on the decoded size, and
    /// a later decode really does move the page.
    fn img_box(&self, el: &Element, st: &ComputedStyle) -> (i32, i32) {
        let img = el.attr("src").and_then(|s| self.images.get(s));
        let (iw, ih) = img.map(|i| (i.w as f32, i.h as f32)).unwrap_or((0.0, 0.0));
        let attr = |n: &str| el.attr(n).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
        // A definite CSS length beats the presentational attribute (HTML
        // §15.3). Only px counts: a percentage needs the containing block,
        // which a replaced element's own box measurement does not have here, so
        // it stays as indefinite as `auto`. Wikipedia sizes its footer wordmark
        // this way (`style="width:7.5em;height:1.125em"`) — without this the
        // box is not just the wrong size, it also counts as GUESSED, and every
        // guess costs a full re-layout the moment the pixels land.
        let css = |l: Len| match l {
            Len::Px(v) if v >= 0.0 => Some(v),
            _ => None,
        };
        let (aw, ah) = (css(st.width).or_else(|| attr("width")), css(st.height).or_else(|| attr("height")));
        if img.is_none() && (aw.is_none() || ah.is_none()) {
            if let Some(src) = el.attr("src") {
                // Throwaway measurements size the same `<img>` several times,
                // so without this the list holds one entry per measurement pass
                // and the shell rescans them all on every image that lands.
                let mut g = self.guessed.borrow_mut();
                if !g.iter().any(|s| s == src) {
                    g.push(String::from(src));
                }
            }
        }
        let bw = aw.unwrap_or(if iw > 0.0 { iw } else { 100.0 });
        let bh = match ah {
            Some(h) => h,
            None if iw > 0.0 => bw * ih / iw,
            None => bw * 0.75,
        };
        (bw.max(1.0) as i32, bh.max(1.0) as i32)
    }

    /// Measure a form control and capture what it displays right now (the
    /// user's typed value, else the authored default). Controls are atomic
    /// inline boxes — they never wrap, and their children never lay out.
    fn control_box(&self, el: &Element, st: &ComputedStyle, kind: ControlKind, avail: f32) -> CtlBox {
        let font = self.fonts.pick(st.bold, st.italic, st.mono);
        let size = st.font_px;
        let ch_w = measure(font, "0", size).max(1.0);
        let line = line_gap(font, size);
        let default = default_value(el, kind);
        let raw = self.forms.value_or(el.seq, &default).to_string();
        let focused = self.forms.focus == Some(el.seq);

        // What the box shows: typed text (bulleted for a password), the
        // placeholder when empty, or a button/select label.
        let (mut text, ghost) = match kind {
            ControlKind::Password => (repeat_char('•', raw.chars().count()), false),
            ControlKind::Text | ControlKind::TextArea if raw.is_empty() => {
                (el.attr("placeholder").unwrap_or("").to_string(), true)
            }
            ControlKind::Select => {
                let label = select_label(el, &raw);
                (label, false)
            }
            ControlKind::Submit | ControlKind::Reset | ControlKind::Button => {
                (button_label(el, kind, &raw), false)
            }
            ControlKind::File => ("Datei wählen".to_string(), true),
            _ => (raw.clone(), false),
        };
        if matches!(kind, ControlKind::Checkbox | ControlKind::Radio) {
            text.clear();
        }

        // Intrinsic size, then let a definite CSS width/height win (real pages
        // size their search fields in CSS, not with `size=`).
        let pad_x = CTL_PAD_X;
        // The frame is part of the box, and it is the page's when the page
        // styled it — a control with `border: none` is exactly as tall as its
        // content, and a `border: 2px` one two pixels taller per side.
        let border = ctl_border(st);
        let (bx, by) = (border[1].w + border[3].w, border[0].w + border[2].w);
        let (mut w, mut h) = match kind {
            ControlKind::Checkbox | ControlKind::Radio => {
                let s = (size * 0.9).max(12.0) as i32;
                (s, s)
            }
            ControlKind::TextArea => {
                let cols = el.attr("cols").and_then(|c| c.trim().parse::<f32>().ok()).unwrap_or(30.0);
                let rows = el.attr("rows").and_then(|r| r.trim().parse::<f32>().ok()).unwrap_or(3.0);
                (
                    (cols * ch_w) as i32 + 2 * pad_x + bx,
                    (rows * line) as i32 + 2 * CTL_PAD_Y + by,
                )
            }
            ControlKind::Text | ControlKind::Password => {
                let cols = el.attr("size").and_then(|c| c.trim().parse::<f32>().ok()).unwrap_or(20.0);
                (
                    (cols * ch_w) as i32 + 2 * pad_x + bx,
                    ceil_i32(line) + 2 * CTL_PAD_Y + by,
                )
            }
            ControlKind::Select => (
                ceil_i32(measure(font, &text, size)) + 2 * pad_x + CTL_ARROW + bx,
                ceil_i32(line) + 2 * CTL_PAD_Y + by,
            ),
            _ => (
                ceil_i32(measure(font, &text, size)) + 2 * (pad_x + 4) + bx,
                ceil_i32(line) + 2 * CTL_PAD_Y + by,
            ),
        };
        if let Some(cw) = st.width.px(avail) {
            // A CSS width is a content width unless `box-sizing: border-box`.
            w = if st.box_border { cw as i32 } else { cw as i32 + 2 * pad_x + bx };
        }
        // A percentage height resolves against the containing block's HEIGHT
        // (§10.5), never `avail` (its width) — the checkbox-hack overlay is
        // `width:100%; height:100%`, and measuring its height off the width
        // made it as tall as its container is wide. An indefinite CB height
        // leaves the percentage unresolvable, so the intrinsic height stands.
        if let Some(chh) = vert_len(st.height, self.cb.3) {
            h = if st.box_border { chh as i32 } else { chh as i32 + 2 * CTL_PAD_Y + by };
        }
        if let Some(mx) = st.max_width.px(avail) {
            w = w.min(mx as i32);
        }
        if let Some(mn) = st.min_width.px(avail) {
            w = w.max(mn as i32);
        }
        // `min-height` on a control is how real pages give a search field its
        // height (Codex: `min-height: 32px`). Without it the control keeps its
        // intrinsic line height and sits short inside its own flex row.
        if let Some(mn) = vert_len(st.min_height, self.cb.3) {
            h = h.max(if st.box_border { mn as i32 } else { mn as i32 + 2 * CTL_PAD_Y + by });
        }
        if let Some(mx) = vert_len(st.max_height, self.cb.3) {
            h = h.min(if st.box_border { mx as i32 } else { mx as i32 + 2 * CTL_PAD_Y + by });
        }

        // Caret: the shell keeps a byte offset; painting counts characters.
        let caret = if focused && kind.is_text() {
            Some(raw[..self.forms.caret.min(raw.len())].chars().count())
        } else {
            None
        };
        CtlBox {
            seq: el.seq,
            kind,
            w: w.max(8),
            h: h.max(8),
            text,
            ghost,
            checked: self.forms.checked_or(el.seq, el.attr("checked").is_some()),
            focused,
            caret,
            bg: st.bg,
            pad_l: (st.pad_left as i32).max(CTL_PAD_X),
            border,
            style: RunStyle { hidden: st.hidden, transparent: st.transparent, size, color: st.color, bold: st.bold, italic: st.italic, mono: st.mono, valign: crate::style::VAlign::Baseline, deco: st.deco, break_word: st.break_word, nowrap: st.nowrap, lh: st.line_height.px(size).unwrap_or(0.0) },
        }
    }

    /// Lay a `position:absolute`/`fixed` box, out of flow, at a position derived
    /// from the containing block (`self.cb`) + `top`/`right`/`bottom`/`left`.
    /// The element is `el`, already pushed onto `self.path` by the caller.
    fn layout_abs(&mut self, el: &'a Element, st: &ComputedStyle, static_x: i32, static_y: i32) {
        if st.position == Position::Fixed {
            self.fixed_count += 1;
        } else {
            self.abs_count += 1;
        }
        let (cbx, cby, cbw, cbh) = self.cb;
        // An out-of-flow box resolves its percentage height against the
        // containing block it is positioned in, not against whatever in-flow
        // ancestor happens to be open (§10.5 + §10.1).
        let prev_cb_h = self.cb_h;
        self.cb_h = cbh.map(|h| h as f32);
        let avail = cbw as f32;
        let left = st.left.px(avail);
        let right = st.right.px(avail);
        let width = match (st.width.px(avail), left, right) {
            (Some(wd), _, _) => wd,
            (None, Some(l), Some(r)) => (avail - l - r).max(0.0),
            _ => {
                // Shrink-to-fit (§10.3.7). `intrinsic_width` returns a CONTENT
                // width, but what goes to `layout_box` is read as a containing
                // block and has margin/padding/border taken off it AGAIN — so
                // the box lost its own frame twice and its content overflowed
                // by exactly that much. Floats and inline-blocks hand over the
                // MARGIN-box width for this reason; this path did not.
                let frame = st.margin_left.px(avail).unwrap_or(0.0)
                    + st.margin_right.px(avail).unwrap_or(0.0)
                    + st.pad_left
                    + st.pad_right
                    + st.border_x();
                (self.intrinsic_width(el, st).0 + frame).min(avail)
            }
        };
        // `min-width`/`max-width` apply to an out-of-flow box like any other
        // (CSS2.1 §10.4) — the height path already went through them, the width
        // did not. A shrink-to-fit box with no content is the case that shows
        // it: MediaWiki's search magnifier is an empty absolutely positioned
        // span sized only by `min-width`, and without the clamp it came out
        // ONE pixel wide.
        let width = clamp_len(width, st.min_width, st.max_width, st.box_border, st.pad_left + st.pad_right + st.border_x());
        // Horizontal: an offset pins to the CB edge; with both `left`/`right`
        // auto the box keeps its **static position** (CSS2.1 §10.3.7).
        let px = if let Some(l) = left {
            cbx as f32 + l
        } else if let Some(r) = right {
            cbx as f32 + avail - r - width
        } else {
            static_x as f32
        };
        // Vertical offsets resolve against the CB **height**, never its width
        // (§9.3.2) — getting that wrong stretches every percentage-positioned
        // layout by the CB's aspect ratio. An indefinite CB height leaves a
        // percentage unresolvable here (the parent's content height doesn't
        // exist yet while its children are laid out), so it behaves as `auto`.
        let top = vert_len(st.top, cbh);
        let bottom = vert_len(st.bottom, cbh);

        // §10.6.4: `top` + `bottom` with `height:auto` stretches the box to the
        // gap between them. Over-constrained (all three given) → `bottom` is
        // the one that gets ignored, which is what the `top` arm below does.
        let mut st_owned = *st;
        // NOTE: a percentage `height` here would also resolve against `cbh`
        // (§10.5), but doing it in the abspos path ALONE measured worse: an
        // in-flow `height:100%` parent still collapses to auto, and the
        // mismatch between the two paths breaks more than it fixes. It belongs
        // with general percentage-height support, not here.
        let mut overridden = false;
        if let (Some(t), Some(b), Some(h), Len::Auto) = (top, bottom, cbh, st.height) {
            st_owned.height = Len::Px((h as f32 - t - b).max(0.0));
            overridden = true;
        }
        let st = if overridden { &st_owned } else { st };

        // `top` pins to the CB top; `bottom` pins the box's bottom edge, which
        // needs its own height — only known once it is laid out. So lay it out
        // at the static position and slide the finished box (and everything it
        // emitted) into place. With neither offset the static position is the
        // answer already (§10.6.4).
        let (py, shift_to_bottom) = match (top, bottom, cbh) {
            (Some(t), _, _) => (cby as f32 + t, None),
            (None, Some(b), Some(h)) => (static_y as f32, Some(cby as f32 + h as f32 - b)),
            _ => (static_y as f32, None),
        };
        // layout_box → layout_block re-establishes the CB for its own children.
        let w_i = width.max(1.0) as i32;
        let start = self.ops.len();
        let link_start = self.links.len();
        let ctl_start = self.controls.len();
        // An explicit `z-index` on this positioned box opens a tracked
        // stacking range for it (CSS2.1 §9.9) — unless it's already nested
        // inside another tracked range, which absorbs it instead.
        let track = self.should_track_stack(st);
        if track {
            self.stack_depth += 1;
        }
        let box_bottom = self.layout_box(el, st, px as i32, w_i, py as i32);
        if track {
            self.stack_depth -= 1;
        }
        // Bottom-anchored: now that the used height is known, translate the box
        // (all ops/links/controls it just emitted) so its bottom edge lands on
        // the offset. Indices stay valid — nothing is inserted or removed, so
        // any stacking range recorded inside is untouched.
        let bottom = if let Some(target) = shift_to_bottom {
            let dy = (target - box_bottom as f32) as i32;
            if dy != 0 {
                translate_ops(&mut self.ops[start..], dy);
                for l in &mut self.links[link_start..] {
                    l.y += dy;
                }
                for c in &mut self.controls[ctl_start..] {
                    c.y += dy;
                }
            }
            box_bottom + dy
        } else {
            box_bottom
        };

        // `clip: rect(...)` (CSS 2.1 §11.1.2) — clip this box (and its
        // descendants, all emitted into `ops[start..]`) to a rectangle whose
        // offsets are measured from the border-box top-left corner.
        if let Clip::Rect { top, right, bottom: cbot, left } = st.clip {
            // Border box, mirroring `layout_block`'s geometry.
            let (cw, off_left) = resolve_block_h(st, w_i as f32);
            let ml = off_left - st.pad_left - st.border_left.width;
            let bl = px as i32 + ml as i32; // border-box left
            let bt = py as i32; // border-box top (== the y0 layout_block used)
            let br = bl + cw as i32 + (st.pad_left + st.pad_right) as i32 + st.border_x() as i32;
            let bb = bottom; // border-box bottom (layout_box return)
            // Clip edges (auto = the corresponding border edge).
            let cl = bl + left.map(|v| v as i32).unwrap_or(0);
            let cr = bl + right.map(|v| v as i32).unwrap_or(br - bl);
            let ct = bt + top.map(|v| v as i32).unwrap_or(0);
            let cb = bt + cbot.map(|v| v as i32).unwrap_or(bb - bt);
            clip_ops(&mut self.ops, start, cl, ct, cr, cb);
        }
        // `transform: translate(...)`, same paint-time shift as in flow. This is
        // where the `top:50%` + `translate(-50%)` centring idiom lands, so an
        // out-of-flow box that skipped it sat half a box too low — far enough
        // to be clipped away by an `overflow:hidden` parent.
        let (tdx, tdy) = translate_offset(st, w_i, bottom - py as i32);
        if tdx != 0 || tdy != 0 {
            self.shift_ops(start, self.ops.len(), link_start, self.links.len(), ctl_start, tdx, tdy);
        }
        if track {
            if let ZIndex::Value(z) = st.z_index {
                self.record_stack_entry(z, LAYER_IN_FLOW, start, self.ops.len(), link_start, self.links.len());
            }
        }
        // The out-of-flow box, at its final (post-bottom-shift) position.
        let dy = bottom - box_bottom;
        self.record_inspect(el, st, px as i32, py as i32 + dy, w_i, box_bottom - py as i32);
        self.cb_h = prev_cb_h;
    }

    /// Insert the block's `background-color` behind its content (at `bg_idx`)
    /// and stroke its `border` on the border-box edges.
    /// Insert a box's `background-color` behind the content it already emitted
    /// (at `bg_idx`). Split out from `paint_box_decoration` because a table can
    /// paint its background — an opaque infobox must not let the article text
    /// it floats over show through — while its BORDER still can't be drawn from
    /// here: the table box has no resolved border box yet, and guessing one
    /// puts the stroke in the wrong place (measured: 5 reftests).
    /// `overflow: hidden` — drop whatever the box's content painted outside its
    /// padding box. `marks` is `(first op index, abs_count, fixed_count)` taken
    /// BEFORE the content was laid out. Call BEFORE `paint_box_decoration`, so
    /// the box's own background and border are not clipped by it.
    ///
    /// Skipped when a descendant recorded a z-index stacking range inside the
    /// span: `clip_ops` rebuilds the tail and can drop ops, which would leave
    /// those ranges pointing at the wrong slots — and a scrambled display list
    /// is a far worse defect than an unclipped overflow.
    fn clip_overflow(&mut self, st: &ComputedStyle, marks: (usize, u32, u32), box_left: i32, box_top: i32, box_w: i32, box_h: i32) {
        let (start, abs0, fixed0) = marks;
        if !st.overflow_clip || start >= self.ops.len() {
            return;
        }
        if self.stack_ops.iter().any(|(_, _, _, e)| *e > start) {
            return;
        }
        // An out-of-flow descendant is clipped only by an ancestor in its
        // CONTAINING-BLOCK chain (CSS2.1 §11.1.1). A `position: static` box is
        // not the containing block of an absolutely positioned descendant, and
        // nothing but the viewport is for a fixed one — so a box that let one
        // escape cannot clip its span at all. The display list is flat, so the
        // escapee's ops can't be excluded individually.
        if self.fixed_count > fixed0 || (st.position == Position::Static && self.abs_count > abs0) {
            return;
        }
        let cl = box_left + st.border_left.width as i32;
        let ct = box_top + st.border_top.width as i32;
        let cr = box_left + box_w - st.border_right.width as i32;
        let cb = box_top + box_h - st.border_bottom.width as i32;
        clip_ops(&mut self.ops, start, cl, ct, cr, cb);
    }

    fn insert_bg(&mut self, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, bg_idx: usize) {
        if w <= 0 || h <= 0 || st.hidden || st.transparent {
            return;
        }
        let mut layer: Vec<DrawOp> = Vec::new();
        let masked = self.bg_key(st.mask_layer.image);
        bg_ops(st, self.bg_key(st.bg_layer.image), masked, x, y, w, h, &mut layer);
        if layer.is_empty() {
            return;
        }
        self.insert_ops_at(bg_idx, layer);
    }

    /// Splice ops into the display list at `at`, keeping every recorded
    /// stacking/float range consistent.
    ///
    /// `insert` shifts every later op up by `n` slots — any already-recorded
    /// range overlapping or after `at` (a descendant's tracked z-index range,
    /// recorded before its ancestor's background gets painted in) must shift
    /// too. Half-open `[s, e)`: a range that already ends at-or-before `at` is
    /// untouched (`e > at`, strict — `e == at` means the insertion lands right
    /// after the range, not inside it).
    fn insert_ops_at(&mut self, at: usize, ops: Vec<DrawOp>) {
        let n = ops.len();
        if n == 0 {
            return;
        }
        for (i, op) in ops.into_iter().enumerate() {
            self.ops.insert(at + i, op);
        }
        for (_, _, s, e) in &mut self.stack_ops {
            if *s >= at {
                *s += n;
            }
            if *e > at {
                *e += n;
            }
        }
        for (s, e) in &mut self.float_ops {
            if *s >= at {
                *s += n;
            }
            if *e > at {
                *e += n;
            }
        }
    }

    /// Note that this layout needs a CSS image, and hand back its key.
    fn bg_key(&self, image: Option<u64>) -> Option<u64> {
        let key = image?;
        let mut used = self.css_images.borrow_mut();
        if !used.contains(&key) {
            used.push(key);
        }
        Some(key)
    }

    /// Paint one border edge rect (the collapsed model draws grid lines
    /// individually rather than a box's four sides together).
    fn paint_edge(&mut self, s: &BorderSide, x: i32, y: i32, w: i32, h: i32) {
        if let (Some(c), true) = (s.color, s.width > 0.0) {
            if w > 0 && h > 0 {
                self.ops.push(DrawOp::Rect { x, y, w, h, color: c });
            }
        }
    }

    fn paint_box_decoration(&mut self, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, bg_idx: usize) {
        // `visibility:hidden` suppresses this box's own background and border.
        // Bailing before the `bg_idx` insert is what keeps the recorded
        // stacking ranges intact — nothing is inserted, so nothing shifts.
        if w <= 0 || h <= 0 || st.hidden || st.transparent {
            return;
        }
        #[cfg(feature = "diag-boxes")]
        if h > 700 || w > self.viewport_w as i32 {
            let who = self.path.last().map(|e| {
                let mut s = e.tag.clone();
                if let Some(id) = &e.id { s.push('#'); s.push_str(id); }
                for c in e.classes.iter().take(2) { s.push('.'); s.push_str(c); }
                s
            }).unwrap_or_else(|| String::from("?"));
            extern crate std;
            std::println!("[box] {who}: x={x} y={y} w={w} h={h}");
        }
        // Background first, THEN the shadow — both splice in at `bg_idx`, so
        // whatever is inserted last ends up underneath.
        self.insert_bg(st, x, y, w, h, bg_idx);
        self.insert_shadow(st, x, y, w, h, bg_idx);
        border_ops(st, x, y, w, h, (true, true), &mut self.ops);
    }

    /// Paint the box's `box-shadow` behind its background. Only the zero-blur
    /// case — which on real pages is a hairline separator, not a drop shadow.
    /// MediaWiki draws the rule under the article tabs with
    /// `box-shadow: 0 1px #c8ccd1`, and without this the page simply lacks it.
    ///
    /// Inserted at `bg_idx` BEFORE the background, so it ends up underneath;
    /// `insert_bg` then shifts the recorded stacking ranges for its own ops the
    /// same way, and both insertions are accounted for.
    fn insert_shadow(&mut self, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, bg_idx: usize) {
        let Some(sh) = st.shadow else { return };
        if sh.blur != 0.0 {
            return;
        }
        let sx = x + sh.dx as i32 - sh.spread as i32;
        let sy = y + sh.dy as i32 - sh.spread as i32;
        let sw = w + 2 * sh.spread as i32;
        let shh = h + 2 * sh.spread as i32;
        if sw <= 0 || shh <= 0 {
            return;
        }
        // An OUTER shadow is not painted inside the border box (CSS Backgrounds
        // 3 §7.1.1) — the box is cut out of it. Without that the shadow is a
        // full-size copy of the box, and since these boxes are usually
        // transparent it floods the whole row instead of leaving the 1px strip
        // the author wanted. Subtracting one rect from another gives at most
        // four pieces: a band above, a band below, and the left/right slivers
        // of the rows in between.
        let color = sh.color.unwrap_or(st.color);
        let (sx1, sy1) = (sx + sw, sy + shh);
        let (x1, y1) = (x + w, y + h);
        let mut parts: Vec<DrawOp> = Vec::new();
        let mut push = |px: i32, py: i32, pw: i32, ph: i32| {
            if pw > 0 && ph > 0 {
                parts.push(DrawOp::Rect { x: px, y: py, w: pw, h: ph, color });
            }
        };
        push(sx, sy, sw, y.min(sy1) - sy);
        push(sx, y1.max(sy), sw, sy1 - y1.max(sy));
        let (my0, my1) = (sy.max(y), sy1.min(y1));
        if my1 > my0 {
            push(sx, my0, x.min(sx1) - sx, my1 - my0);
            push(x1.max(sx), my0, sx1 - x1.max(sx), my1 - my0);
        }
        self.insert_ops_at(bg_idx, parts);
    }

    /// Simplified table layout. Two column models: `table-layout: auto` sizes
    /// columns from cell content (readable infoboxes + data tables); `table-
    /// layout: fixed` (CSS2 §17.5.2.1) takes column widths from the table/
    /// `<col>`/first-row cell `width`s and distributes the rest, painting each
    /// cell's own box (background/border/padding). Rows/cells are recognised by
    /// HTML tag (`tr`/`td`/`th`/`thead`…) or `display: table-*`; anonymous
    /// boxes fill any missing row/row-group/cell wrapper (CSS2 §17.2.1).
    fn layout_table(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // <caption> renders as a block on the table's top or bottom edge
        // (CSS2.1 §17.4.1), per its own `caption-side` — aligned with the
        // TABLE box, so the table's horizontal margins have to come off first.
        // `layout_table_body` applies them to the grid; without the same shift
        // here a floated table with a left margin puts its caption a margin's
        // width further left than the rows above it, which is exactly what
        // MediaWiki's image thumbs (`margin-left: 1.4em`) show.
        let cbw = w as f32;
        let ml = st.margin_left.px(cbw).unwrap_or(0.0) as i32;
        let mr = st.margin_right.px(cbw).unwrap_or(0.0) as i32;
        let (cx, cw) = (x + ml, (w - ml - mr).max(0));
        let mut y = self.layout_captions(el, st, cx, cw, y0, false);
        y = self.layout_table_body(&el.children, st, x, w, y);
        self.layout_captions(el, st, cx, cw, y, true)
    }

    /// Lay out the caption children whose `caption-side` puts them on the
    /// requested edge, stacked at `y0`. Returns the y below them. A caption is
    /// recognised by `display: table-caption` as well as by the `<caption>`
    /// tag — MediaWiki's image thumbs are a `figure{display:table}` with a
    /// `figcaption{display:table-caption}`, and reading only the tag turns the
    /// caption into stray content that widens the table instead of wrapping to
    /// it.
    fn layout_captions(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32, bottom: bool) -> i32 {
        let mut y = y0;
        for c in &el.children {
            if let Node::Element(e) = c {
                let cs = self.styled(e, st, &[], 0);
                if e.tag != "caption" && cs.display != Display::TableCaption {
                    continue;
                }
                if cs.display == Display::None || cs.caption_bottom != bottom {
                    continue;
                }
                // A caption is a block-level box of its own, not a bare run of
                // children: it takes a width/height, a background and a border,
                // and `position: relative` moves it like any other box (the
                // caller of `layout_box` normally applies that — here that
                // caller is us).
                self.path.push(ElemInfo::of(e));
                let part = self.part_start();
                y = self.layout_box(e, &cs, x, w, y);
                if cs.position == Position::Relative {
                    let (dx, dy) = rel_offset(&cs, w as f32);
                    if dx != 0 || dy != 0 {
                        self.shift_ops(part.op, self.ops.len(), part.link, self.links.len(), part.ctl, dx, dy);
                    }
                }
                self.path.pop();
            }
        }
        y
    }

    /// The table's row grid (everything but `<caption>`): shared by a real
    /// `<table>` (`el.children`, above) and an anonymous table synthesized in
    /// `flow_children` around a stray run of table-part siblings that has no
    /// `table`/`inline-table` ancestor (CSS2 §17.2.1) — an anonymous table
    /// can't have a `<caption>` child (nothing selects an anonymous box), so
    /// only the row-collection step is shared.
    fn layout_table_body(&mut self, nodes: &'a [Node], st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let mut rows = self.collect_table_rows(nodes, st);
        rows.retain(|r| !r.cells.is_empty());
        let ncols = rows.iter().map(|r| row_columns(&r.cells).1).max().unwrap_or(0).min(64);
        if ncols == 0 {
            return y0;
        }

        // `fixed` tables paint each cell's own box (backgrounds/borders are the
        // point of the spec). The `auto` model leaves cell decoration to the
        // block boxes inside each cell — painting per-cell backgrounds/borders
        // there would (without collapsed-border resolution) draw borders that
        // should be hidden and swatches the reference omits.
        // The columns share the table's CONTENT box, so the space they may use
        // is what is left of `w` after the table's own border and padding —
        // otherwise the grid overflows the border box by exactly that much.
        // Horizontal margins apply to a table box like any other block-level
        // box, but the enclosing BFC branch only carries the vertical ones —
        // flex and grid pick these up inside `resolve_block_h`, which a table
        // can't use (its `width:auto` shrink-wraps instead of filling).
        let (ml_len, mr_len) = (st.margin_left, st.margin_right);
        let (ml, mr) = (ml_len.px(w as f32).unwrap_or(0.0) as i32, mr_len.px(w as f32).unwrap_or(0.0) as i32);
        let avail = (w - ml - mr).max(0);
        // In the collapsed model the table has neither padding nor a border box
        // of its own (CSS2.1 §17.6.2) — the outermost cell borders ARE the
        // table's frame, so the grid starts flush at the table's edge and the
        // cells draw every grid line, outer ones included.
        let collapse = st.border_collapse;
        let frame = if collapse { 0 } else { (st.pad_left + st.pad_right + st.border_x()) as i32 };
        // Separated border model: `border-spacing` runs between every pair of
        // columns AND once along each outer edge, so `ncols + 1` gaps come out
        // of the content box before the columns share what is left.
        let (sx, sy) = spacing_of(st);
        let gaps = sx * (ncols as i32 + 1);
        let inner_w = (avail - frame - gaps).max(0);
        let colw = if st.table_layout == TableLayout::Fixed {
            self.fixed_columns(&rows, ncols, st, inner_w)
        } else {
            self.auto_columns(&rows, ncols, st, inner_w)
        };
        // The table's own border box: border, then padding, then the row grid.
        // Getting the border edge in here is what lets the table paint its own
        // decoration at all — laying the grid at `x + pad_left` (no border
        // offset) put every stroke a border-width off.
        let (btl, btt) = (st.border_left.width as i32, st.border_top.width as i32);
        // A table's used width is known only once its columns are, so `auto`
        // margins can only be resolved here (CSS2.1 §10.3.3 over §17.5.2): both
        // auto centres the table, one auto pushes it to the other edge.
        let table_w = colw.iter().sum::<i32>() + gaps + frame;
        let slack = (avail - table_w).max(0);
        let off = match (ml_len, mr_len) {
            (Len::Auto, Len::Auto) => ml + slack / 2,
            (Len::Auto, _) => ml + slack,
            // Inside `<center>` a table is centred even with zero margins —
            // that is what `-moz-center` does, and it is the whole reason the
            // `<center><table>` idiom worked. Google's home page centres its
            // search box that way, so without it a correctly sized table still
            // sits hard against the left edge.
            _ if st.center_blocks => ml + slack / 2,
            _ => ml,
        };
        let x = x + off;
        let (inner_x, content_top) = if collapse {
            (x, y0)
        } else {
            (x + btl + st.pad_left as i32 + sx, y0 + btt + st.pad_top as i32 + sy)
        };
        let bg_idx = self.ops.len();
        let bottom = self.lay_table_rows(&rows, ncols, &colw, st, inner_x, content_top);
        let mut table_bottom = if collapse { bottom } else { bottom + sy + st.pad_bottom as i32 + st.border_bottom.width as i32 };
        // `height` on a table is a MINIMUM for its box, never a maximum
        // (CSS2.1 §17.5.3) — rows keep the height their content needs, and a
        // table shorter than its `height` grows to it. The rows themselves are
        // not stretched into the extra space; that is the "distribute over
        // rows" part of §17.5.3 and needs per-row percentage heights first.
        let frame_y = if collapse { 0.0 } else { st.pad_top + st.pad_bottom + st.border_y() };
        let min_h = |len: Len| match len {
            Len::Px(h) if st.box_border => Some(h),
            Len::Px(h) => Some(h + frame_y),
            _ => None,
        };
        if let Some(h) = min_h(st.height).or_else(|| min_h(st.min_height)) {
            table_bottom = table_bottom.max(y0 + h as i32);
        }
        // A table box paints its own background and border like any other box;
        // only per-cell decoration is left to the boxes inside (see above).
        // Without this a floated infobox is transparent and the article text it
        // overlaps shows straight through it. Its used width comes from the
        // columns it actually produced, not from the space it was offered.
        if collapse {
            self.insert_bg(st, x, y0, table_w, table_bottom - y0, bg_idx);
        } else {
            self.paint_box_decoration(st, x, y0, table_w, table_bottom - y0, bg_idx);
        }
        table_bottom
    }

    fn part_start(&self) -> TablePart {
        TablePart { op: self.ops.len(), link: self.links.len(), ctl: self.controls.len() }
    }

    /// Close a table row or row-group box around everything emitted since
    /// `part`: its background goes behind that range, and `position: relative`
    /// then moves box and content together. Rows and row groups take a
    /// background but never a border — the separated model ignores border
    /// properties on them (CSS2.1 §17.6.1), and the collapsed model resolves
    /// every grid line at the cells.
    fn finish_table_part(&mut self, cs: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, part: TablePart, cb_w: f32) {
        self.insert_bg(cs, x, y, w, h, part.op);
        if cs.position == Position::Relative {
            let (dx, dy) = rel_offset(cs, cb_w);
            if dx != 0 || dy != 0 {
                self.shift_ops(part.op, self.ops.len(), part.link, self.links.len(), part.ctl, dx, dy);
            }
        }
    }

    /// Auto table sizing (CSS2 §17.5.2.2, approximated): each column takes the
    /// widest cell's *border-box* preferred width (content + that cell's
    /// padding/border, or its explicit `width`). The table shrink-wraps to that,
    /// shrinking columns proportionally (never below their minimum) only when
    /// they overflow the available width; an explicit table `width` wider than
    /// the content spreads the slack across columns.
    fn auto_columns(&mut self, rows: &[Row<'a>], ncols: usize, st: &ComputedStyle, w: i32) -> Vec<i32> {
        // A cell percentage is a fraction of the TABLE, not of whatever space
        // the table was offered — resolving it against the available width
        // makes `width="25%"` mean a quarter of the viewport in a narrower
        // table.
        let pct_basis = table_content_width(st, w as f32);
        let mut pref = vec![0.0f32; ncols];
        let mut minw = vec![0.0f32; ncols];
        // Columns a cell pinned with an explicit `width`. Slack belongs to
        // the OTHERS — see the distribution below.
        let mut sized = vec![false; ncols];
        // Single-column cells define their column outright; cells spanning
        // several only have to fit ACROSS them, so they run in a second pass
        // once the single-span widths are known.
        for pass_span in [false, true] {
            for row in rows {
                let (starts, _) = row_columns(&row.cells);
                let depth = self.push_row_path(row);
                for (i, sc) in row.cells.iter().enumerate() {
                    let c = starts[i];
                    let span = cell_span(&sc.cell);
                    if c >= ncols || (span > 1) != pass_span {
                        continue;
                    }
                    let (wp, wm) = self.cell_pref_min(&sc.cell, &sc.st, Some(pct_basis), st.border_collapse);
                    if span == 1 {
                        pref[c] = pref[c].max(wp);
                        minw[c] = minw[c].max(wm);
                        if sc.st.width != Len::Auto {
                            sized[c] = true;
                        }
                    } else {
                        spread_span(&mut pref, c, span, wp);
                        spread_span(&mut minw, c, span, wm);
                    }
                }
                self.path.truncate(depth);
            }
        }
        // Dev: which column made a table too wide, and which cell drove it.
        #[cfg(feature = "diag-boxes")]
        {
            extern crate std;
            let who = self.path.last().map(|e| e.classes.join(".")).unwrap_or_default();
            std::eprintln!("[cols] {who} avail={w} font={} cols={ncols} total={:.0} pref={:?}", st.font_px, pref.iter().sum::<f32>(), pref.iter().map(|v| *v as i32).collect::<Vec<_>>());
            for (c, p) in pref.iter().enumerate() {
                let mut widest = (0.0f32, String::new());
                for row in rows.iter() {
                    if let Some(sc) = row.cells.get(c) {
                        let depth = self.push_row_path(row);
                        let (cp, _) = self.intrinsic_width_cell(&sc.cell, &sc.st);
                        self.path.truncate(depth);
                        if cp > widest.0 {
                            let mut t = String::new();
                            match sc.cell {
                                Cell::Real(e) => gather_text(&e.children, &mut t),
                                Cell::Anon(n) => gather_text(n, &mut t),
                            }
                            widest = (cp, collapse_whitespace(&t).trim().chars().take(70).collect());
                        }
                    }
                }
                std::eprintln!("       col{c} pref={:.0} widest={:.0} <- {:?}", p, widest.0, widest.1);
            }
        }
        let content_w = table_content_width(st, w as f32);
        let table_auto = st.width == Len::Auto;
        let cap = if table_auto { w as f32 } else { content_w };
        let total: f32 = pref.iter().sum();
        let mut colw = pref.clone();
        if total > cap && total > 0.0 {
            for c in 0..ncols {
                colw[c] = (cap * pref[c] / total).max(minw[c]);
            }
        } else if !table_auto && total < content_w {
            // No `total > 0` guard: a table whose columns all measure zero
            // (every cell empty) still has to reach its specified `width` —
            // otherwise `table { width: 100px }` around empty cells collapses
            // to its border alone, which is what a `display: table` root with
            // an empty `<body>` does.
            //
            // The slack goes to the columns that did NOT ask for a width
            // (CSS2.1 §17.5.2.2). Spreading it over all of them widened the
            // sized ones past what they asked for: `25% | auto | 25%` came out
            // 41% | 18% | 41%, so the middle cell — the one holding the
            // content — ended up the narrowest of the three. Only when every
            // column is pinned does the slack spread across all of them,
            // because then there is nowhere else for it to go.
            let slack = content_w - total;
            let free = sized.iter().filter(|s| !**s).count();
            if free > 0 {
                let extra = slack / free as f32;
                for c in 0..ncols {
                    if !sized[c] {
                        colw[c] += extra;
                    }
                }
            } else {
                let extra = slack / ncols as f32;
                for c in 0..ncols {
                    colw[c] += extra;
                }
            }
        }
        colw.iter().map(|v| (v + 0.5) as i32).collect()
    }

    /// `table-layout: fixed` column sizing (CSS2 §17.5.2.1): column widths come
    /// from the first row's cell `width`s (each a *border-box* width), and the
    /// rest of the table's used width is split equally across the remaining
    /// columns; content never widens a column.
    fn fixed_columns(&self, rows: &[Row], ncols: usize, st: &ComputedStyle, w: i32) -> Vec<i32> {
        let content_w = table_content_width(st, w as f32);
        // Per-column border-box width; None = "auto" (share the leftover).
        let mut fixed: Vec<Option<f32>> = vec![None; ncols];
        if let Some(first) = rows.first() {
            let (starts, _) = row_columns(&first.cells);
            for (i, sc) in first.cells.iter().enumerate() {
                let c = starts[i];
                // A first-row cell that spans columns doesn't pin any single
                // one of them (CSS2 §17.5.2.1 reads widths per column).
                if c >= ncols || cell_span(&sc.cell) > 1 {
                    continue;
                }
                let cs = sc.st;
                if let Some(cw) = cs.width.px(content_w) {
                    let (bl, br, _, _) = cell_borders(&cs, st.border_collapse);
                    let border_box = if cs.box_border { cw } else { cw + cs.pad_left + cs.pad_right + bl + br };
                    fixed[c] = Some(border_box.max(0.0));
                }
            }
        }
        let sum_fixed: f32 = fixed.iter().filter_map(|o| *o).sum();
        let auto_count = fixed.iter().filter(|o| o.is_none()).count();
        let leftover = content_w - sum_fixed;
        // Remaining table space is divided equally between auto columns; if every
        // column is sized, the slack is spread over all of them instead.
        let auto_w = if auto_count > 0 { (leftover / auto_count as f32).max(0.0) } else { 0.0 };
        let extra = if auto_count == 0 && leftover > 0.0 { leftover / ncols as f32 } else { 0.0 };
        fixed
            .iter()
            .map(|o| match o {
                Some(v) => (v + extra + 0.5) as i32,
                None => (auto_w + 0.5) as i32,
            })
            .collect()
    }

    /// Put a row's own ancestors on the path for the duration of the work its
    /// cells do, and return the depth to truncate back to. Measurement and
    /// layout BOTH go through this — a cell's descendants must resolve against
    /// the same ancestor chain in either, or the widths drift apart.
    fn push_row_path(&mut self, row: &Row<'a>) -> usize {
        let depth = self.path.len();
        if let Some((g, _)) = row.group {
            self.path.push(ElemInfo::of(g));
        }
        if let Some((e, _)) = row.el {
            self.path.push(ElemInfo::of(e));
        }
        depth
    }

    /// Lay a table's rows given resolved (border-box) column widths. Cells sit
    /// side by side; each cell box stretches to the row's tallest cell and paints
    /// its own background/border, with content placed inside its padding.
    fn lay_table_rows(&mut self, rows: &[Row<'a>], ncols: usize, colw: &[i32], st: &ComputedStyle, x: i32, y0: i32) -> i32 {
        // The gaps around the outside are added by the caller, which owns the
        // table's padding edge; these are the ones BETWEEN cells and rows.
        let (sx, sy) = spacing_of(st);
        let col_x = |c: usize| colw[..c].iter().sum::<i32>() + sx * c as i32;
        let collapse = st.border_collapse;
        // A row (and a row group) spans every column plus the spacing between
        // them, but not the outer spacing the caller owns — that is the box
        // its background covers and the containing block its cells see.
        let grid_w = colw.iter().sum::<i32>() + sx * ncols.saturating_sub(1) as i32;
        // The previous row's (style, x, width), so a cell can resolve the grid
        // line it shares with the cell above it in the collapsed model.
        let mut prev_row: Vec<(ComputedStyle, i32, i32)> = Vec::new();
        // The row above, so its bottom border joins the conflict at the line
        // the two rows share (only the lower cell paints that line).
        let mut prev_row_el: Option<(u32, ComputedStyle)> = None;
        let nrows = rows.len();
        let mut y = y0;
        let outer_cb = self.cb;
        // The open row group: its style, where its box and ops start, and the
        // bottom of its last row so far. Rows of one group are contiguous, so
        // the group closes when a row with a different one comes along.
        let mut group: Option<(u32, ComputedStyle, TablePart, i32)> = None;
        let mut last_bottom = y0;
        for (ri, row) in rows.iter().enumerate() {
            if let Some((seq, gst, part, top)) = group {
                if row.group.map(|(g, _)| g.seq) != Some(seq) {
                    self.finish_table_part(&gst, x, top, grid_w, last_bottom - top, part, grid_w as f32);
                    group = None;
                }
            }
            if let (None, Some((g, gst))) = (group, row.group) {
                group = Some((g.seq, gst, self.part_start(), y));
            }
            // Pass 1: place the cells + measure the tallest one. Their styles
            // were settled when the row was collected.
            let row_depth = self.push_row_path(row);
            let mut cells: Vec<(ComputedStyle, i32, i32, i32, i32)> = Vec::new(); // (style, cell_x, cell_w, content_x, content_w)
            let (starts, _) = row_columns(&row.cells);
            for (i, sc) in row.cells.iter().enumerate() {
                let c = starts[i];
                if c >= ncols {
                    break;
                }
                // A spanning cell is as wide as all the columns it covers.
                let end = (c + cell_span(&sc.cell)).min(ncols);
                // A spanning cell swallows the gaps between the columns it
                // covers along with the columns themselves.
                let cw = colw[c..end].iter().sum::<i32>() + sx * (end - c).saturating_sub(1) as i32;
                let cx = x + col_x(c);
                let cs = sc.st;
                let (bl, br, _, _) = cell_borders(&cs, collapse);
                let content_x = cx + bl as i32 + cs.pad_left as i32;
                let content_w = (cw - (bl + br) as i32 - (cs.pad_left + cs.pad_right) as i32).max(0);
                cells.push((cs, cx, cw, content_x, content_w));
            }
            // Row height = the tallest cell border-box (content, or explicit height).
            let mut row_h = 0i32;
            let mut box_hs: Vec<i32> = Vec::with_capacity(cells.len());
            for (c, (cs, _, _, content_x, content_w)) in cells.iter().enumerate() {
                let (_, _, bt, bb) = cell_borders(cs, collapse);
                let content_y = y + bt as i32 + cs.pad_top as i32;
                let mut ch = if cs.display == Display::None {
                    0
                } else {
                    self.measure_cell_height(&row.cells[c].cell, cs, *content_x, *content_w, content_y)
                };
                if let Len::Px(h) = cs.height {
                    let hb = if cs.box_border {
                        (h as i32 - (cs.pad_top + cs.pad_bottom) as i32 - (bt + bb) as i32).max(0)
                    } else {
                        h as i32
                    };
                    ch = ch.max(hb);
                }
                let cell_box_h = ch + (cs.pad_top + cs.pad_bottom) as i32 + (bt + bb) as i32;
                box_hs.push(cell_box_h);
                row_h = row_h.max(cell_box_h);
            }
            // Pass 2: emit content + paint each cell's border-box at row height.
            // A positioned row (or, failing that, row group) is the containing
            // block its cells' absolutely positioned descendants resolve
            // against — the row's height is known now, the group's is not yet.
            let row_part = self.part_start();
            let row_pos = row.el.map(|(_, rst)| rst.position != Position::Static).unwrap_or(false);
            if row_pos {
                self.cb = (x, y, grid_w, Some(row_h));
            } else if let Some((_, gst, _, top)) = group {
                if gst.position != Position::Static {
                    self.cb = (x, top, grid_w, None);
                }
            }
            for (c, (cs, cell_x, cell_w, content_x, content_w)) in cells.iter().enumerate() {
                if cs.display == Display::None {
                    continue;
                }
                let content_y = y + cell_borders(cs, collapse).2 as i32 + cs.pad_top as i32;
                let bg_idx = self.ops.len();
                let (link0, ctl0) = (self.links.len(), self.controls.len());
                let cell_cb = self.cb;
                if cs.position != Position::Static {
                    self.cb = (*content_x, y, *content_w, Some(row_h));
                }
                match row.cells[c].cell {
                    Cell::Real(e) => {
                        self.path.push(ElemInfo::of(e));
                        let _ = self.layout_children(&e.children, cs, Some(e), *content_x, *content_w, content_y);
                        self.path.pop();
                    }
                    Cell::Anon(nodes) => {
                        let _ = self.layout_children(nodes, cs, None, *content_x, *content_w, content_y);
                    }
                }
                self.cb = cell_cb;
                // `vertical-align` in the row (CSS2.1 §17.5.3). The content was
                // laid out at the cell's top; middle/bottom just slide the ops
                // it produced down by the leftover of the row height. `baseline`
                // (the initial value) would align the cells' first-line
                // baselines — we treat it as `top`, which is what it degrades
                // to for equal-size single-line cells.
                let slack = (row_h - box_hs[c]).max(0);
                let dy = match cs.valign {
                    crate::style::VAlign::Middle => slack / 2,
                    crate::style::VAlign::Bottom => slack,
                    _ => 0,
                };
                if dy != 0 {
                    self.shift_ops(bg_idx, self.ops.len(), link0, self.links.len(), ctl0, 0, dy);
                }
                if collapse {
                    // Each grid line is drawn exactly once, by the cell above/
                    // left of it, as the winner of the two borders that meet
                    // there. The outer lines resolve against the table's own
                    // border, which is why the table paints none itself.
                    let above = prev_row.iter().find(|(_, px, pw)| *px < cell_x + cell_w && px + pw > *cell_x);
                    let left = c.checked_sub(1).and_then(|i| cells.get(i));
                    let mut top = collapsed_edge(&cs.border_top, above.map_or(&st.border_top, |(a, _, _)| &a.border_bottom));
                    let mut lft = collapsed_edge(&cs.border_left, left.map_or(&st.border_left, |(l, _, _, _, _)| &l.border_right));
                    // A cell is not the only box at its grid lines: its row and
                    // its row group meet them too (§17.6.2), which is how a
                    // `tr`/`tbody { border-style: hidden }` suppresses the
                    // borders of the cells inside it. The row's own left/right
                    // edges only exist at the ends of the row, and a group's
                    // top/bottom only where the group starts and ends.
                    for outer in [row.el.map(|(_, s)| s), row.group.map(|(_, s)| s)].into_iter().flatten() {
                        top = collapsed_edge(&top, &outer.border_top);
                        if c == 0 {
                            lft = collapsed_edge(&lft, &outer.border_left);
                        }
                    }
                    if let Some((_, prst)) = prev_row_el {
                        top = collapsed_edge(&top, &prst.border_bottom);
                    }
                    self.insert_bg(cs, *cell_x, y, *cell_w, row_h, bg_idx);
                    // A collapsed border straddles the grid line: half of it
                    // falls in each of the two cells that meet there.
                    let half = |w: f32| (w / 2.0) as i32;
                    self.paint_edge(&top, *cell_x, y - half(top.width), *cell_w, top.width as i32);
                    self.paint_edge(&lft, cell_x - half(lft.width), y, lft.width as i32, row_h);
                    if c + 1 == cells.len() {
                        let mut r = collapsed_edge(&cs.border_right, &st.border_right);
                        for outer in [row.el.map(|(_, s)| s), row.group.map(|(_, s)| s)].into_iter().flatten() {
                            r = collapsed_edge(&r, &outer.border_right);
                        }
                        self.paint_edge(&r, cell_x + cell_w - half(r.width), y, r.width as i32, row_h);
                    }
                    if ri + 1 == nrows {
                        let mut b = collapsed_edge(&cs.border_bottom, &st.border_bottom);
                        for outer in [row.el.map(|(_, s)| s), row.group.map(|(_, s)| s)].into_iter().flatten() {
                            b = collapsed_edge(&b, &outer.border_bottom);
                        }
                        self.paint_edge(&b, *cell_x, y + row_h - half(b.width), *cell_w, b.width as i32);
                    }
                } else {
                    self.paint_box_decoration(cs, *cell_x, y, *cell_w, row_h, bg_idx);
                }
                // A relative cell takes its own box with it, so this has to run
                // after the decoration was inserted — unlike the `vertical-align`
                // slide above, which moves the content inside a cell that stays.
                if cs.position == Position::Relative {
                    let (dx, dy) = rel_offset(cs, grid_w as f32);
                    if dx != 0 || dy != 0 {
                        self.shift_ops(bg_idx, self.ops.len(), link0, self.links.len(), ctl0, dx, dy);
                    }
                }
            }
            self.cb = outer_cb;
            self.path.truncate(row_depth);
            if let Some((_, rst)) = row.el {
                self.finish_table_part(&rst, x, y, grid_w, row_h, row_part, grid_w as f32);
            }
            prev_row = cells.iter().map(|(cs, cx, cw, _, _)| (*cs, *cx, *cw)).collect();
            prev_row_el = row.el.map(|(e, s)| (e.seq, s));
            last_bottom = y + row_h;
            y = last_bottom + sy;
        }
        if let Some((_, gst, part, top)) = group {
            self.finish_table_part(&gst, x, top, grid_w, last_bottom - top, part, grid_w as f32);
        }
        // The trailing gap belongs BETWEEN rows, not after the last one — the
        // caller adds the outer one.
        y - if rows.is_empty() { 0 } else { sy }
    }

    /// Lay an element's children to measure their flowed height without emitting
    /// any draw ops (used to size table rows before painting cell boxes).
    fn measure_children_height(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        let (o, l, c) = (self.ops.len(), self.links.len(), self.controls.len());
        // Stacking ranges index into `ops`/`links`, so a discarded speculative
        // layout has to drop the ones it recorded too — otherwise they survive
        // pointing into a vector that was truncated behind them, and
        // `reorder_by_z` (which needs disjoint ascending ranges) slices the
        // real display list at the wrong offsets.
        let (so, sl) = (self.stack_ops.len(), self.stack_links.len());
        let (fo, flk) = (self.float_ops.len(), self.float_links.len());
        // Floats live on past the box that placed them, so a discarded layout
        // leaks exclusion rects into the real one: the next float finds a BFC
        // that looks full and drops below phantom neighbours.
        let fl = self.floats.len();
        self.path.push(ElemInfo::of(el));
        let bottom = self.layout_children(&el.children, st, Some(el), x, w.max(0), y);
        self.path.pop();
        self.ops.truncate(o);
        self.links.truncate(l);
        self.controls.truncate(c);
        self.stack_ops.truncate(so);
        self.stack_links.truncate(sl);
        self.float_ops.truncate(fo);
        self.float_links.truncate(flk);
        self.floats.truncate(fl);
        (bottom - y).max(0)
    }

    /// Same as `measure_children_height`, for a table cell that may be an
    /// anonymous box (no owning element to push on `self.path`).
    fn measure_cell_height(&mut self, cell: &Cell<'a>, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        match cell {
            Cell::Real(e) => self.measure_children_height(e, st, x, w, y),
            Cell::Anon(nodes) => {
                let (o, l, c) = (self.ops.len(), self.links.len(), self.controls.len());
                let (so, sl) = (self.stack_ops.len(), self.stack_links.len());
                let (fo, flk) = (self.float_ops.len(), self.float_links.len());
                let fl = self.floats.len();
                let bottom = self.layout_children(nodes, st, None, x, w.max(0), y);
                self.ops.truncate(o);
                self.links.truncate(l);
                self.controls.truncate(c);
                self.stack_ops.truncate(so);
                self.stack_links.truncate(sl);
                self.float_ops.truncate(fo);
                self.float_links.truncate(flk);
                self.floats.truncate(fl);
                (bottom - y).max(0)
            }
        }
    }

    /// Classify a table child by tag, else by its computed `display` (CSS
    /// tables). Only elements are passed in.
    fn table_role(&self, e: &Element, parent: &ComputedStyle) -> TableRole {
        match e.tag.as_str() {
            "tr" => TableRole::Row,
            "thead" => TableRole::HeaderGroup,
            "tbody" => TableRole::RowGroup,
            "tfoot" => TableRole::FooterGroup,
            "td" | "th" => TableRole::Cell,
            "caption" | "col" | "colgroup" => TableRole::Skip,
            _ => {
                let st = self.styled(e, parent, &[], 0);
                match st.display {
                    Display::TableRow => TableRole::Row,
                    Display::TableRowGroup => TableRole::RowGroup,
                    Display::TableHeaderGroup => TableRole::HeaderGroup,
                    Display::TableFooterGroup => TableRole::FooterGroup,
                    Display::TableCell => TableRole::Cell,
                    Display::TableColumn | Display::TableColumnGroup | Display::TableCaption => TableRole::Skip,
                    _ => TableRole::Other,
                }
            }
        }
    }

    /// Collect a table's rows (CSS2 §17.2.1 anonymous table objects), in final
    /// render order: any `table-header-group` rows first, then every other row
    /// (plain `<tr>`/`table-row`, `table-row-group`, and any stray content
    /// coalesced into anonymous rows) in document order, then any
    /// `table-footer-group` rows last — regardless of their source order.
    fn collect_table_rows(&mut self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<Row<'a>> {
        let mut header = Vec::new();
        let mut body = Vec::new();
        let mut footer = Vec::new();
        self.collect_rows_into(nodes, parent, None, &mut header, &mut body, &mut footer);
        header.extend(body);
        header.extend(footer);
        // A row (or a whole row group) set to `display: none` generates no box:
        // it takes no height and no column width. Dropping it here rather than
        // at paint time is what keeps measurement and layout agreeing — both
        // sides of the table go through this one function.
        let visible = |s: &Option<(&Element, ComputedStyle)>| s.is_none_or(|(_, st)| st.display != Display::None);
        header.retain(|r: &Row| visible(&r.el) && visible(&r.group));
        header
    }

    /// Walk `nodes` (a table's or row-group's children), bucketing each row by
    /// group kind. A child that is a `table-row`/`-row-group`/`-header-group`/
    /// `-footer-group` is a proper table child and recurses/becomes a row
    /// directly; any other maximal run of consecutive siblings (stray cells,
    /// stray text, stray elements — anything that isn't a proper table child)
    /// is wrapped in ONE anonymous row (whitespace-only text neither starts
    /// nor breaks a run, and is dropped if it's all a run ever contained).
    fn collect_rows_into(
        &mut self,
        nodes: &'a [Node],
        parent: &ComputedStyle,
        group: Option<(&'a Element, ComputedStyle)>,
        header: &mut Vec<Row<'a>>,
        body: &mut Vec<Row<'a>>,
        footer: &mut Vec<Row<'a>>,
    ) {
        let mut run_start: Option<usize> = None;
        let mut run_has_content = false;
        // Rows and row groups cascade like any other element: `:nth-child` on
        // a `<tr>` is zebra striping, and every element child counts towards it
        // — `<caption>`/`<col>` included, since they are DOM siblings even
        // though they generate no row.
        let mut siblings: Vec<ElemInfo> = Vec::new();
        let sib_count = nodes.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        for (i, n) in nodes.iter().enumerate() {
            let role = match n {
                Node::Element(e) => Some(self.table_role(e, parent)),
                Node::Text(_) => None,
            };
            match role {
                Some(TableRole::Row) | Some(TableRole::RowGroup) | Some(TableRole::HeaderGroup) | Some(TableRole::FooterGroup) => {
                    if let Some(s) = run_start.take() {
                        if run_has_content {
                            body.push(Row { el: None, group, cells: self.partition_cells(&nodes[s..i], parent) });
                        }
                        run_has_content = false;
                    }
                    let Node::Element(e) = n else { unreachable!() };
                    let est = self.styled(e, parent, &siblings, sib_count);
                    match role {
                        // A row's cells cascade from the row, with the row on
                        // the path — `tbody tr td` and `td:first-child` both
                        // need that, and it has to hold here because this is
                        // where a cell's style is settled for good.
                        Some(TableRole::Row) => {
                            self.path.push(ElemInfo::of(e));
                            let cells = self.partition_cells(&e.children, &est);
                            self.path.pop();
                            body.push(Row { el: Some((e, est)), group, cells })
                        }
                        Some(TableRole::RowGroup) => {
                            self.path.push(ElemInfo::of(e));
                            self.collect_rows_into(&e.children, &est, Some((e, est)), header, body, footer);
                            self.path.pop();
                        }
                        Some(TableRole::HeaderGroup) | Some(TableRole::FooterGroup) => {
                            let (mut h, mut b, mut f) = (Vec::new(), Vec::new(), Vec::new());
                            self.path.push(ElemInfo::of(e));
                            self.collect_rows_into(&e.children, &est, Some((e, est)), &mut h, &mut b, &mut f);
                            self.path.pop();
                            let out = if role == Some(TableRole::HeaderGroup) { &mut *header } else { &mut *footer };
                            out.extend(h);
                            out.extend(b);
                            out.extend(f);
                        }
                        _ => unreachable!(),
                    }
                }
                Some(TableRole::Skip) => {
                    // `<caption>`/`<col>`/`<colgroup>` generate no box and are
                    // fully transparent to the stray-content run around them.
                }
                _ => {
                    // A stray cell, stray non-table element, or non-whitespace
                    // text: not a proper table child, so it joins the run.
                    let has_content = match n {
                        Node::Text(t) => !t.trim().is_empty(),
                        Node::Element(_) => true,
                    };
                    if run_start.is_none() {
                        run_start = Some(i);
                    }
                    run_has_content |= has_content;
                }
            }
            if let Node::Element(e) = n {
                siblings.push(ElemInfo::of(e));
            }
        }
        if let Some(s) = run_start {
            if run_has_content {
                body.push(Row { el: None, group, cells: self.partition_cells(&nodes[s..], parent) });
            }
        }
    }

    /// Partition a row's children into cells (CSS2 §17.2.1): a proper
    /// `table-cell` child stays its own (real) cell; any other maximal run of
    /// consecutive siblings (stray text, stray non-cell elements) is wrapped in
    /// ONE anonymous cell. Shared by a real `<tr>`'s children and an anonymous
    /// row's coalesced node run.
    /// `parent` is the ROW's style: a cell inherits from its row, and an
    /// anonymous cell takes the §17.2.1 anonymous-box style from it. The
    /// caller must have the row on `self.path` — the cells' selectors are
    /// resolved here, and measurement and layout have to agree on that path.
    fn partition_cells(&self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<StyledCell<'a>> {
        let mut cells = Vec::new();
        let mut run_start: Option<usize> = None;
        let mut run_has_content = false;
        let mut siblings: Vec<ElemInfo> = Vec::new();
        let sib_count = nodes.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        let anon = |cell| StyledCell { cell, st: style::anon_inherit(parent, Display::TableCell) };
        for (i, n) in nodes.iter().enumerate() {
            let role = match n {
                Node::Element(e) => Some(self.table_role(e, parent)),
                Node::Text(_) => None,
            };
            match role {
                Some(TableRole::Cell) => {
                    if let Some(s) = run_start.take() {
                        if run_has_content {
                            cells.push(anon(Cell::Anon(&nodes[s..i])));
                        }
                        run_has_content = false;
                    }
                    let Node::Element(e) = n else { unreachable!() };
                    let st = self.styled(e, parent, &siblings, sib_count);
                    cells.push(StyledCell { cell: Cell::Real(e), st });
                }
                // A caption/`<col>` is a PROPER table child, so it ends the run
                // of consecutive stray siblings rather than sitting inside it
                // (CSS2.1 §17.2.1 wraps consecutive non-table children only).
                // The anonymous cell is a contiguous slice, so leaving the run
                // open here would swallow the caption's text and size the
                // column to it — which is how a MediaWiki image thumb came out
                // as wide as its caption instead of as wide as its image.
                Some(TableRole::Skip) => {
                    if let Some(s) = run_start.take() {
                        if run_has_content {
                            cells.push(anon(Cell::Anon(&nodes[s..i])));
                        }
                        run_has_content = false;
                    }
                }
                _ => {
                    let has_content = match n {
                        Node::Text(t) => !t.trim().is_empty(),
                        Node::Element(_) => true,
                    };
                    if run_start.is_none() {
                        run_start = Some(i);
                    }
                    run_has_content |= has_content;
                }
            }
            if let Node::Element(e) = n {
                siblings.push(ElemInfo::of(e));
            }
        }
        if let Some(s) = run_start {
            if run_has_content {
                cells.push(anon(Cell::Anon(&nodes[s..])));
            }
        }
        cells
    }

    /// (max-content, min-content) width of a box's contents: max-content = the
    /// widest line when nothing wraps, min-content = the widest unbreakable
    /// word. `st` is the box's OWN resolved style — using a fixed reference
    /// size here regressed nested anonymous tables (CSS2.1 §17.2.1): a cell
    /// whose font differs would get a column sized at the wrong scale.
    ///
    /// Two things the flat "concatenate every descendant's text" shortcut got
    /// wrong, both of which real pages lean on:
    ///
    /// * Out-of-flow (`absolute`/`fixed`) and `display:none` descendants
    ///   contribute nothing (css-sizing-3 §4). A CSS-only dropdown hangs its
    ///   panel off the button as an abspos child; counting it made the button
    ///   as wide as all 62 language names laid end to end (~4150 px).
    /// * A block-level child starts its own line, so a block container's
    ///   max-content is its WIDEST child, not the sum of every descendant.
    fn intrinsic_width(&mut self, el: &'a Element, st: &ComputedStyle) -> (f32, f32) {
        if let Some(hit) = self.intrinsic.get(&el.seq) {
            return *hit;
        }
        // A replaced element measures as its intrinsic width, both ways — it
        // neither wraps nor shrinks below itself. Without this it sizes to 0 as
        // a flex/grid item, a table cell or a shrink-to-fit out-of-flow box.
        let out = if let Some((iw, _)) = replaced_intrinsic(el) {
            (iw, iw)
        // A control has no text children to measure — without this it sizes to
        // 0 as a flex/grid item and disappears.
        } else if let Some(kind) = crate::forms::kind_of(el) {
            if kind == ControlKind::Hidden {
                (0.0, 0.0)
            } else {
                // Measure it with ITS OWN style, the way it will be painted. A
                // root style here read the label at the root font size and lost
                // every declared size and the page's frame, so a shrink-to-fit
                // wrapper reserved 9px more than the control paints — Google's
                // search button sat in a box wider than itself.
                // [[feedback-intrinsic-shared-path]]
                let mut mst = *st;
                // Percentages have no basis while measuring, so they behave as
                // `auto` (css-sizing-3 §4.1) rather than resolving against 0.
                for len in [&mut mst.width, &mut mst.min_width, &mut mst.max_width] {
                    if matches!(len, Len::Pct(_) | Len::Calc { .. }) {
                        *len = Len::Auto;
                    }
                }
                let w = self.control_box(el, &mst, kind, 0.0).w as f32;
                (w, w)
            }
        } else {
            // `el`'s children cascade with `el` as their parent, so it has to
            // be on the ancestor path — unless a caller (the abspos path) put
            // it there already. Without this their descendant selectors match
            // against `el`'s parent and resolve the wrong `display`, which is
            // exactly what the anonymous-table-object reftests measure.
            let push = self.path.last().map(|p| p.seq()) != Some(el.seq);
            if push {
                self.path.push(ElemInfo::of(el));
            }
            let got = if st.display == Display::Table {
                self.intrinsic_table(&el.children, st)
            } else {
                self.intrinsic_width_nodes(&el.children, st)
            };
            if push {
                self.path.pop();
            }
            got
        };
        // Whole pixels, rounded UP. A max-content width is a REQUIREMENT — the
        // width at which the content does not wrap — so a consumer that turns
        // 678.4 into a used width of 678 loses the last word to a second line.
        // Floats and inline-blocks learned this and ceil themselves; flex items
        // and shrink-to-fit out-of-flow boxes truncated, which is why a root
        // `display:flex` wrapped text its `display:block` reference did not.
        let out = (ceil_i32(out.0) as f32, ceil_i32(out.1) as f32);
        self.intrinsic.insert(el.seq, out);
        out
    }

    /// `intrinsic_width` over a bare node slice — an anonymous cell has no
    /// owning element to gather text from. `st` is the style the slice's
    /// content inherits from.
    fn intrinsic_width_nodes(&mut self, nodes: &'a [Node], st: &ComputedStyle) -> (f32, f32) {
        let (mut pref, mut min) = (0.0f32, 0.0f32);
        let mut run = Run::default();
        self.intrinsic_walk(nodes, st, &mut run, &mut pref, &mut min);
        flush_run(self.fonts, st, &mut run, &mut pref, &mut min, side_by_side(st));
        (pref, min)
    }

    /// A table's own (max-content, min-content) width: each column takes its
    /// widest cell, and the table is the sum of its columns. Deliberately the
    /// same decomposition `auto_columns` lays out with — `collect_table_rows`
    /// owns the CSS2.1 §17.2.1 anonymous-object fixup, so measuring through it
    /// keeps the measurement and the layout from drifting apart.
    fn intrinsic_table(&mut self, nodes: &'a [Node], st: &ComputedStyle) -> (f32, f32) {
        let mut rows = self.collect_table_rows(nodes, st);
        rows.retain(|r| !r.cells.is_empty());
        let ncols = rows.iter().map(|r| row_columns(&r.cells).1).max().unwrap_or(0).min(64);
        if ncols == 0 {
            return (0.0, 0.0);
        }
        let (mut pref, mut minw) = (vec![0.0f32; ncols], vec![0.0f32; ncols]);
        for pass_span in [false, true] {
            for row in &rows {
                let (starts, _) = row_columns(&row.cells);
                let depth = self.push_row_path(row);
                for (i, sc) in row.cells.iter().enumerate() {
                    let c = starts[i];
                    let span = cell_span(&sc.cell);
                    if c >= ncols || (span > 1) != pass_span {
                        continue;
                    }
                    let (p, m) = self.cell_pref_min(&sc.cell, &sc.st, None, st.border_collapse);
                    if span == 1 {
                        pref[c] = pref[c].max(p);
                        minw[c] = minw[c].max(m);
                    } else {
                        spread_span(&mut pref, c, span, p);
                        spread_span(&mut minw, c, span, m);
                    }
                }
                self.path.truncate(depth);
            }
        }
        (pref.iter().sum(), minw.iter().sum())
    }

    /// Walk `nodes` as one block container's contents, accumulating inline
    /// content into `run` and folding each block-level child's own measurement
    /// into `pref`/`min`. `st` is the parent style the children cascade from;
    /// `self.path` must already end at their parent.
    fn intrinsic_walk(&mut self, nodes: &'a [Node], st: &ComputedStyle, run: &mut Run, pref: &mut f32, min: &mut f32) {
        let horiz = side_by_side(st);
        // The measure walk resolves styles the same way the LAYOUT walk does —
        // with the preceding siblings and the sibling count. Passing `&[]`/`0`
        // made every sibling-combinator rule (`+`, `~`) invisible to width
        // measurement while layout applied it, so the two disagreed about the
        // same box. Codex hides an icon-only button's label with
        // `.cdx-button--icon-only span + span { position: absolute }`: layout
        // took it out of flow, the measurement still counted its text, and
        // Wikipedia's hamburger came out ~80px too wide — pushing the logo and
        // the search box right across the whole header.
        let mut siblings: Vec<ElemInfo> = Vec::new();
        let sib_count = nodes.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        // A stray run of table parts (rows/cells with no table ancestor) is one
        // anonymous table box, measured as such — not as loose siblings.
        for seg in self.segment_table_runs(nodes, st) {
            match seg {
                TableSeg::Table(part) => {
                    flush_run(self.fonts, st, run, pref, min, horiz);
                    let (p, m) = self.intrinsic_table(part, st);
                    if horiz {
                        *pref += p;
                        *min += m;
                    } else {
                        *pref = pref.max(p);
                        *min = min.max(m);
                    }
                }
                TableSeg::Node(n) => {
                    self.intrinsic_node(n, st, run, pref, min, horiz, &siblings, sib_count);
                    if let Node::Element(e) = n {
                        siblings.push(ElemInfo::of(e));
                    }
                }
            }
        }
    }

    /// One node of a block container's content walk (see `intrinsic_walk`).
    /// `horiz` says whether this container's children sit side by side, so a
    /// finished box adds to the running width instead of competing with it.
    #[allow(clippy::too_many_arguments)]
    fn intrinsic_node(&mut self, n: &'a Node, st: &ComputedStyle, run: &mut Run, pref: &mut f32, min: &mut f32, horiz: bool, prev: &[ElemInfo<'a>], sib_count: u32) {
        let el = match n {
            Node::Text(t) => {
                run.text.push_str(t);
                return;
            }
            Node::Element(e) => e,
        };
        // A forced break ends the line even at max-content, so the text on
        // either side of it never adds up. Wikipedia's infoboxes label their
        // cells across two or three `<br>` lines; measuring those as one line
        // made the label column ~2x too wide and squeezed the article text.
        if el.tag == "br" {
            flush_run(self.fonts, st, run, pref, min, horiz);
            return;
        }
        let cs = self.styled(el, st, prev, sib_count);
        // Not rendered, or out of flow → contributes no intrinsic width.
        if cs.display == Display::None || matches!(cs.position, Position::Absolute | Position::Fixed) {
            return;
        }
        // An inline box's text joins the line its parent is building.
        if cs.display == Display::Inline
            && crate::forms::kind_of(el).is_none()
            && el.tag != "img"
            && replaced_intrinsic(el).is_none()
        {
            run.frame += inline_frame(&cs, 0.0);
            self.path.push(ElemInfo::of(el));
            self.intrinsic_walk(&el.children, &cs, run, pref, min);
            self.path.pop();
            return;
        }
        // Everything else is a box of its own: an atomic inline (image, form
        // control) or a block-level child. Either way it ends the current line.
        let (p, m) = if el.tag == "img" {
            self.path.push(ElemInfo::of(el));
            let (iw, _) = self.img_box(el, &cs);
            self.path.pop();
            (iw as f32, iw as f32)
        } else {
            let (p, m) = self.intrinsic_width(el, &cs);
            // A child contributes its MARGIN box: its margins have to fit
            // inside the parent's shrink-to-fit width too (css-sizing-3 §4).
            // A percentage margin resolves against a width that does not exist
            // yet, so it is indefinite here and contributes nothing.
            let margins = cs.margin_left.px(0.0).unwrap_or(0.0) + cs.margin_right.px(0.0).unwrap_or(0.0);
            let frame = cs.pad_left + cs.pad_right + cs.border_x() + margins;
            // A definite `width` fixes the child's outer width, so that — not
            // what its content would prefer — is what it contributes to its
            // parent's shrink-to-fit (css-sizing-3 §4). A percentage stays
            // indefinite while the parent's own width is still unknown.
            match cs.width {
                Len::Px(v) => {
                    let outer = if cs.box_border { v } else { v + frame };
                    let outer = clamp_len(outer, cs.min_width, cs.max_width, cs.box_border, frame);
                    (outer, outer)
                }
                _ => (p + frame, m + frame),
            }
        };
        // An atomic inline sits ON the current line — it does not end it.
        // `inline-block`, an image, a form control: all of them are
        // inline-LEVEL, so their widths add to the line's the same way a word
        // does. Treating them as block-level children (which is what falling
        // through to `pref.max(p)` below does) measures a container of two
        // inline-blocks as ONE of them wide, and they then have no room beside
        // each other and stack — Google's header bar is exactly this shape.
        // (Reaching here with `display:inline` means an image, a form control
        // or another replaced box — the plain-inline branch above already
        // returned. A FLOATED box leaves the line, so it is not one of these.)
        let atomic_inline = matches!(cs.display, Display::InlineBlock | Display::Inline);
        if atomic_inline && cs.float == FloatKind::None {
            run.atomic += p;
            run.atomic_min = run.atomic_min.max(m);
            return;
        }
        flush_run(self.fonts, st, run, pref, min, horiz);
        if horiz {
            *pref += p;
            *min += m;
            return;
        }
        // Floated siblings sit side by side, so a block container's max-content
        // width is their SUM, not the widest of them. Taking the widest sized a
        // `float: right` <ul> of icons to ONE icon — and its own floated <li>
        // children then had no room beside each other and stacked vertically,
        // which is exactly what the Wikipedia footer showed. At MIN-content each
        // float gets its own line, so there the widest still wins.
        if cs.float != FloatKind::None {
            run.floats += p;
            *pref = pref.max(run.floats);
        } else {
            run.floats = 0.0;
            *pref = pref.max(p);
        }
        *min = min.max(m);
    }

    /// `intrinsic_width`, dispatching on whether the cell is real or anonymous.
    fn intrinsic_width_cell(&mut self, cell: &Cell<'a>, st: &ComputedStyle) -> (f32, f32) {
        match cell {
            Cell::Real(e) => self.intrinsic_width(e, st),
            Cell::Anon(nodes) => self.intrinsic_width_nodes(nodes, st),
        }
    }

    /// A cell's (max-content, min-content) BORDER-BOX width, honouring an
    /// explicit `width` on the cell itself (CSS2.1 §17.5.2.2). `avail` is the
    /// basis a percentage width resolves against, or `None` while the table's
    /// own width is still being measured — a percentage is indefinite then and
    /// contributes nothing. Shared by `auto_columns` (layout) and
    /// `intrinsic_table` (measurement) so the two cannot drift apart.
    fn cell_pref_min(&mut self, cell: &Cell<'a>, cs: &ComputedStyle, avail: Option<f32>, collapse: bool) -> (f32, f32) {
        let (bl, br, _, _) = cell_borders(cs, collapse);
        let frame = cs.pad_left + cs.pad_right + bl + br;
        let (p, m) = self.intrinsic_width_cell(cell, cs);
        let spec = match avail {
            Some(a) => cs.width.px(a),
            None => match cs.width {
                Len::Px(v) => Some(v),
                _ => None,
            },
        };
        let spec = match spec {
            Some(v) if cs.box_border => v,
            Some(v) => v + frame,
            None => 0.0,
        };
        ((p + frame).max(spec), (m + frame).max(spec))
    }

    /// Split `nodes` into pass-through single nodes and maximal runs of
    /// `table-row`/`-row-group`/`-header-group`/`-footer-group`/`-cell`
    /// siblings (whitespace-only text between them doesn't break a run). A run
    /// found here has no `table`/`inline-table` ancestor — `flow_children`
    /// wraps it in one anonymous `table` box (CSS2 §17.2.1) instead of laying
    /// each part out as an ordinary block.
    fn segment_table_runs(&self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<TableSeg<'a>> {
        fn is_table_part(role: TableRole) -> bool {
            matches!(role, TableRole::Row | TableRole::RowGroup | TableRole::HeaderGroup | TableRole::FooterGroup | TableRole::Cell)
        }
        let mut segs = Vec::with_capacity(nodes.len());
        let mut i = 0;
        while i < nodes.len() {
            let starts_run = matches!(&nodes[i], Node::Element(e) if is_table_part(self.table_role(e, parent)));
            if starts_run {
                let mut last = i;
                let mut j = i + 1;
                while j < nodes.len() {
                    match &nodes[j] {
                        Node::Text(t) if t.trim().is_empty() => j += 1,
                        Node::Element(e) if is_table_part(self.table_role(e, parent)) => {
                            last = j;
                            j += 1;
                        }
                        _ => break,
                    }
                }
                segs.push(TableSeg::Table(&nodes[i..=last]));
                i = last + 1;
            } else {
                segs.push(TableSeg::Node(&nodes[i]));
                i += 1;
            }
        }
        segs
    }

    /// Dispatch a block-level box to the right formatting context.
    fn layout_box(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        // A form control is atomic wherever it lands. `flow_children` and
        // `collect_inline` catch the in-flow cases (so a field flows with the
        // text beside it); this catches every other box-making path — flex and
        // grid items, table cells, absolutely positioned controls. Real search
        // boxes sit in `display:flex` rows, so missing this rendered NOTHING.
        if let Some(kind) = crate::forms::kind_of(el) {
            if kind == ControlKind::Hidden {
                return y;
            }
            let mut ctl = self.control_box(el, st, kind, w as f32);
            // Here the CALLER already resolved this box: `w` is the flex item's
            // main size / the grid column / the table cell, and a definite
            // height is the stretched cross size. Painting the control's own
            // intrinsic size instead would overlap the next item and ignore the
            // stretch every grid/flex item gets by default.
            if !matches!(kind, ControlKind::Checkbox | ControlKind::Radio) {
                ctl.w = w.max(8);
                if let Some(hh) = st.height.px(w as f32) {
                    ctl.h = (hh as i32).max(8);
                }
            }
            let h_i = ctl.h;
            paint_control(self.fonts, self.theme, &ctl, x, y, &mut self.ops, &mut self.controls);
            return y + h_i;
        }
        // The block path resolves percentage heights itself (it is entered
        // directly from the flow loop too); the other three come through here.
        let resolved = self.resolve_pct_heights(st);
        let st = resolved.as_ref().unwrap_or(st);
        match st.display {
            Display::Table => self.layout_table(el, st, x, w, y),
            Display::Flex => self.layout_flex(el, st, x, w, y),
            Display::Grid => self.layout_grid(el, st, x, w, y),
            _ => self.layout_block(el, st, x, w, y),
        }
    }

    /// Grid layout (css-grid-2 subset). Handles the container box model (width/
    /// margins/padding/background/border/explicit height), explicit
    /// `grid-template-columns`/`-rows` (px/%/fr/auto/`repeat`), `grid-auto-rows`,
    /// the `grid`/`grid-template` `<rows> / <cols>` shorthand, row-major
    /// auto-placement with explicit line placement (`grid-column`/`grid-row`,
    /// start line + span), separate `row-gap`/`column-gap`, and item alignment
    /// (`justify-items`/`align-items`/`justify-self`/`align-self`, incl. the
    /// default `stretch`). Not yet: named lines/areas, `repeat(auto-fill)`,
    /// dense packing, subgrid, or `align-content`/`justify-content`.
    fn layout_grid(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // No template at all → a grid degenerates to a block box.
        if st.grid_ncols == 0 && st.grid_nrows == 0 {
            return self.layout_block(el, st, x, w, y0);
        }

        // Container horizontal box (mirrors `layout_block`) — border included,
        // same as the flex container: `width` is the CONTENT box, the border
        // box is that plus padding and border.
        let (cw, off_left) = resolve_block_h(st, w as f32);
        let content_x = x + off_left as i32;
        let content_w = cw.max(1.0) as i32;
        let box_left = content_x - st.pad_left as i32 - st.border_left.width as i32;
        let box_w = content_w + (st.pad_left + st.pad_right) as i32 + st.border_x() as i32;
        let bg_idx = self.ops.len();
        let content_top = y0 + st.pad_top as i32 + st.border_top.width as i32;

        let prev_cb = self.cb;
        if st.position != Position::Static {
            self.cb = padding_cb(st, content_x, content_top, content_w);
        }
        let prev_cb_h = self.cb_h;
        self.cb_h = content_height_of(st, st.height);
        let content_h = self.grid_content(el, st, content_x, content_w, content_top);
        self.cb_h = prev_cb_h;
        self.cb = prev_cb;

        // Explicit / min / max height clamp the content-box height.
        let px_h = |len: Len| content_height_of(st, len).map(|v| v as i32);
        let mut ch = content_h;
        if let Some(h) = px_h(st.height) {
            ch = h;
        }
        if let Some(mn) = px_h(st.min_height) {
            ch = ch.max(mn);
        }
        if let Some(mx) = px_h(st.max_height) {
            ch = ch.min(mx);
        }

        let y = content_top + ch + st.pad_bottom as i32 + st.border_bottom.width as i32;
        self.paint_box_decoration(st, box_left, y0, box_w, y - y0, bg_idx);
        self.place_abs_pseudos(el, st, box_left, y0, box_w, y - y0);
        y
    }

    /// Lay a grid container's items inside its content box `(x, w, y0)`, returning
    /// the content-box height. `self.cb` is already set for the container.
    fn grid_content(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // Column tracks, expanding any `repeat(auto-fill/auto-fit, …)` to fill the
        // container width.
        let mut tracks: Vec<GridTrack> = st.grid_tracks[..st.grid_ncols as usize].to_vec();
        if st.grid_col_fill != 0
            && st.grid_col_fill_len > 0
            && (st.grid_col_fill_start as usize + st.grid_col_fill_len as usize) <= tracks.len()
        {
            let start = st.grid_col_fill_start as usize;
            let len = st.grid_col_fill_len as usize;
            let avail = w as f32;
            let g = st.grid_col_gap;
            let px_of = |t: GridTrack| match t {
                GridTrack::Fixed(p) => Some(p),
                GridTrack::Pct(p) => Some(p / 100.0 * avail),
                _ => None,
            };
            let mut pat_size = 0.0f32;
            let mut definite = len > 0;
            for &t in &tracks[start..start + len] {
                match px_of(t) {
                    Some(p) => pat_size += p,
                    None => definite = false,
                }
            }
            let per_rep = pat_size + len as f32 * g;
            if definite && per_rep > 0.0 {
                let fixed: f32 = tracks
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i < start || *i >= start + len)
                    .map(|(_, &t)| px_of(t).unwrap_or(0.0))
                    .sum();
                // `as i64` truncates toward zero (= floor for the positive ratio).
                let count = (((avail - fixed + g) / per_rep) as i64).max(1) as usize;
                let pat: Vec<GridTrack> = tracks[start..start + len].to_vec();
                let mut nt: Vec<GridTrack> = tracks[..start].to_vec();
                for _ in 0..count {
                    if nt.len() + len > style::MAX_GRID_COLS {
                        break;
                    }
                    nt.extend_from_slice(&pat);
                }
                nt.extend_from_slice(&tracks[start + len..]);
                nt.truncate(style::MAX_GRID_COLS);
                tracks = nt;
            }
        }
        if tracks.is_empty() {
            tracks.push(GridTrack::Auto); // rows-only grid → one implicit column
        }
        let ncols = tracks.len();

        // Grid items = in-flow child elements; abspos children are out of flow.
        let mut items: Vec<(&Element, ComputedStyle)> = Vec::new();
        let sib_count = el.children.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        let mut siblings: Vec<ElemInfo> = Vec::new();
        for c in &el.children {
            if let Node::Element(ce) = c {
                let mut cs = self.styled(ce, st, &siblings, sib_count);
                siblings.push(ElemInfo::of(ce));
                if matches!(cs.display, Display::Inline | Display::InlineBlock) {
                    cs.display = Display::Block;
                }
                if cs.display == Display::None {
                    continue;
                }
                if matches!(cs.position, Position::Absolute | Position::Fixed) {
                    self.path.push(ElemInfo::of(ce));
                    self.layout_abs(ce, &cs, self.cb.0, self.cb.1);
                    self.path.pop();
                    continue;
                }
                items.push((ce, cs));
            }
        }

        let col_gap = st.grid_col_gap;
        let row_gap = st.grid_row_gap;

        // Definite container content height (for %/fr row resolution).
        let def_h: Option<f32> = content_height_of(st, st.height);

        // — placement — (col, colspan, row, rowspan) per item, row-major flow
        // honouring explicit `grid-column`/`grid-row` start lines + spans.
        let resolve_col = |line: i16| -> usize {
            if line > 0 {
                (line as usize - 1).min(ncols - 1)
            } else if line < 0 {
                (ncols as i16 + line).clamp(0, ncols as i16 - 1) as usize
            } else {
                0
            }
        };
        // Row line → 0-based row index; the row count is not yet known, so
        // negative lines (relative to the last row) fall back to the first row.
        let resolve_row = |line: i16| -> usize { (line.max(1) as usize) - 1 };
        let colmask = |c: usize, span: usize| -> u32 {
            let mut m = 0u32;
            for k in c..(c + span).min(ncols) {
                m |= 1u32 << k;
            }
            m
        };
        let fits = |occ: &Vec<u32>, r: usize, mask: u32, rspan: usize| -> bool {
            (r..r + rspan).all(|rr| rr >= occ.len() || occ[rr] & mask == 0)
        };
        let mut occ: Vec<u32> = Vec::new();
        let mut place: Vec<(usize, usize, usize, usize)> = Vec::with_capacity(items.len());
        let (mut cur_r, mut cur_c) = (0usize, 0usize);
        for (_, s) in &items {
            // Named `grid-area` placement (from the container's grid-template-areas)
            // takes priority over line/auto placement.
            if s.grid_area != 0 {
                if let Some(a) =
                    st.grid_areas[..st.grid_area_count as usize].iter().find(|a| a.name == s.grid_area)
                {
                    let c0 = (a.c0 as usize).min(ncols.saturating_sub(1));
                    let cspan = (a.c1 as usize).min(ncols).max(c0 + 1) - c0;
                    let fr = a.r0 as usize;
                    let rspan = (a.r1.saturating_sub(a.r0)).max(1) as usize;
                    let mask = colmask(c0, cspan);
                    while occ.len() < fr + rspan {
                        occ.push(0);
                    }
                    for rr in fr..fr + rspan {
                        occ[rr] |= mask;
                    }
                    place.push((c0, cspan, fr, rspan));
                    continue;
                }
            }
            let has_col = s.grid_col_start != 0;
            let has_row = s.grid_row_start != 0;
            let (col, cspan) = if has_col {
                let c = resolve_col(s.grid_col_start);
                (c, (s.grid_col_span as usize).clamp(1, ncols - c))
            } else {
                (usize::MAX, (s.grid_col_span as usize).clamp(1, ncols))
            };
            let rspan = (s.grid_row_span as usize).max(1);

            let (fc, fr) = if has_col && has_row {
                (col, resolve_row(s.grid_row_start))
            } else if has_col {
                let mask = colmask(col, cspan);
                let mut r = 0;
                while !fits(&occ, r, mask, rspan) {
                    r += 1;
                }
                (col, r)
            } else if has_row {
                let r = resolve_row(s.grid_row_start);
                let mut c = 0;
                while c + cspan <= ncols && !fits(&occ, r, colmask(c, cspan), rspan) {
                    c += 1;
                }
                if c + cspan > ncols {
                    c = 0;
                }
                (c, r)
            } else {
                let (mut r, mut c) = (cur_r, cur_c);
                loop {
                    if c + cspan > ncols {
                        r += 1;
                        c = 0;
                    }
                    if fits(&occ, r, colmask(c, cspan), rspan) {
                        break;
                    }
                    c += 1;
                }
                cur_r = r;
                cur_c = c + cspan;
                (c, r)
            };
            let mask = colmask(fc, cspan);
            while occ.len() < fr + rspan {
                occ.push(0);
            }
            for rr in fr..fr + rspan {
                occ[rr] |= mask;
            }
            place.push((fc, cspan, fr, rspan));
        }

        // — column sizing — fixed/% direct, `auto` = max-content of single-span
        // items, `fr` splits the leftover.
        let avail = w as f32;
        let mut auto_content = vec![0.0f32; ncols];
        for (i, (el_i, s_i)) in items.iter().enumerate() {
            let (c, cspan, _, _) = place[i];
            if cspan == 1 {
                auto_content[c] = auto_content[c].max(self.intrinsic_width(el_i, s_i).0);
            }
        }
        let mut colw = vec![0.0f32; ncols];
        let (mut fr_sum, mut used) = (0.0f32, 0.0f32);
        for c in 0..ncols {
            match tracks[c] {
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
        let gaps_w = col_gap * (ncols as f32 - 1.0).max(0.0);
        let leftover = (avail - gaps_w - used).max(0.0);
        if fr_sum > 0.0 {
            for c in 0..ncols {
                if let GridTrack::Fr(f) = tracks[c] {
                    colw[c] = leftover * f / fr_sum;
                }
            }
        }
        let mut colx = vec![0.0f32; ncols];
        let mut acc = x as f32;
        for c in 0..ncols {
            colx[c] = acc;
            acc += colw[c] + col_gap;
        }

        // Per-item content-box horizontal placement (needed before measuring the
        // natural height at that width).
        let cell_width = |c: usize, span: usize| -> f32 {
            let mut cw = col_gap * (span as f32 - 1.0).max(0.0);
            for k in 0..span {
                cw += colw[c + k];
            }
            cw
        };
        let mut hpos: Vec<(f32, f32)> = Vec::with_capacity(items.len()); // (ix, iw)
        let mut nat_h: Vec<i32> = Vec::with_capacity(items.len());
        for (i, (el_i, s)) in items.iter().enumerate() {
            let (c, cspan, _, _) = place[i];
            let cw = cell_width(c, cspan);
            let jself = s.justify_self.unwrap_or(st.justify_items);
            let width_auto = matches!(s.width, Len::Auto);
            let (ix, iw) = if jself == CrossAlign::Stretch && width_auto {
                (colx[c], cw)
            } else {
                let uw = match s.width {
                    Len::Auto => self.intrinsic_width(el_i, s).0,
                    other => other.px(cw).unwrap_or(0.0),
                }
                .min(cw)
                .max(1.0);
                let ix = match jself {
                    CrossAlign::End => colx[c] + cw - uw,
                    CrossAlign::Center => colx[c] + (cw - uw) / 2.0,
                    _ => colx[c],
                };
                (ix, uw)
            };
            let s_copy = *s;
            let nh = self.measure_box_height(el_i, &s_copy, ix as i32, iw as i32, y0);
            hpos.push((ix, iw));
            nat_h.push(nh);
        }

        // — row sizing —
        let nrows = occ.len().max(st.grid_nrows as usize);
        let row_track = |r: usize| -> GridTrack {
            if r < st.grid_nrows as usize {
                st.grid_row_tracks[r]
            } else {
                st.grid_auto_rows
            }
        };
        let row_def = |r: usize| -> Option<f32> {
            match row_track(r) {
                GridTrack::Fixed(px) => Some(px),
                GridTrack::Pct(p) => def_h.map(|h| p / 100.0 * h),
                _ => None,
            }
        };
        let mut row_h = vec![0.0f32; nrows];
        for r in 0..nrows {
            if let Some(d) = row_def(r) {
                row_h[r] = d;
            }
        }
        for i in 0..items.len() {
            let (_, _, r, rspan) = place[i];
            if rspan == 1 && row_def(r).is_none() {
                row_h[r] = row_h[r].max(nat_h[i] as f32);
            }
        }
        for i in 0..items.len() {
            let (_, _, r, rspan) = place[i];
            if rspan > 1 {
                let cur: f32 = (r..r + rspan).map(|rr| row_h[rr]).sum::<f32>()
                    + row_gap * (rspan as f32 - 1.0);
                if (cur as i32) < nat_h[i] {
                    if let Some(last) = (r..r + rspan).rev().find(|&rr| row_def(rr).is_none()) {
                        row_h[last] += nat_h[i] as f32 - cur;
                    }
                }
            }
        }
        // `fr` rows share the container's leftover definite height.
        if let Some(dh) = def_h {
            let fr_rows: Vec<(usize, f32)> = (0..nrows)
                .filter_map(|r| match row_track(r) {
                    GridTrack::Fr(f) => Some((r, f)),
                    _ => None,
                })
                .collect();
            let frsum: f32 = fr_rows.iter().map(|(_, f)| f).sum();
            if frsum > 0.0 {
                let usedh: f32 =
                    row_h.iter().sum::<f32>() + row_gap * (nrows as f32 - 1.0).max(0.0);
                let extra = (dh - usedh).max(0.0);
                for (r, f) in fr_rows {
                    row_h[r] = extra * f / frsum;
                }
            }
        }
        let mut row_y = vec![0i32; nrows];
        let mut yy = y0;
        for r in 0..nrows {
            row_y[r] = yy;
            yy += row_h[r] as i32;
            if r + 1 < nrows {
                yy += row_gap as i32;
            }
        }

        // — final item placement with cross-axis alignment / stretch —
        for (i, (el_i, s)) in items.iter().enumerate() {
            let (_, _, r, rspan) = place[i];
            let (ix, iw) = hpos[i];
            let cell_y = row_y[r];
            let mut cell_h = row_gap * (rspan as f32 - 1.0).max(0.0);
            for k in 0..rspan {
                cell_h += row_h[r + k];
            }
            let aself = s.align_self.unwrap_or(st.align_items);
            let height_auto = matches!(s.height, Len::Auto);
            let mut s2 = *s;
            if aself == CrossAlign::Stretch && height_auto && cell_h > 0.0 {
                s2.height = Len::Px(cell_h);
            }
            let op0 = self.ops.len();
            let link0 = self.links.len();
            let ctl0 = self.controls.len();
            self.path.push(ElemInfo::of(el_i));
            // NOTE: css-grid-2 §6.6 says a grid item's percentage height
            // resolves against its GRID AREA, and the row tracks are sized by
            // now, so it could be answered right here — `self.cb_h =
            // Some(cell_h)` guarded on the spanned rows being definite. It
            // MEASURES WORSE: 4052 against 4056 for keeping the container's
            // content height. Guarding on definite tracks changed nothing, so
            // the difference is not the circularity — something downstream
            // (the `align-self: stretch` branch above already gives an
            // auto-height item the row's height) compensates for the coarser
            // answer. Parked with the number rather than taken on faith.
            let bottom = self.layout_box(el_i, &s2, ix as i32, (iw as i32).max(1), cell_y);
            self.path.pop();
            let laid_h = bottom - cell_y;
            let dy = match aself {
                CrossAlign::Center => (cell_h as i32 - laid_h) / 2,
                CrossAlign::End => cell_h as i32 - laid_h,
                _ => 0,
            };
            if dy != 0 {
                self.shift_ops(op0, self.ops.len(), link0, self.links.len(), ctl0, 0, dy);
            }
        }

        yy - y0
    }

    /// Lay a box just to measure its natural height, discarding the emitted ops
    /// (used for grid auto-row sizing before the real placement pass).
    fn measure_box_height(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        let (o, l, c) = (self.ops.len(), self.links.len(), self.controls.len());
        // Stacking ranges index into `ops`/`links`, so a discarded speculative
        // layout has to drop the ones it recorded too — otherwise they survive
        // pointing into a vector that was truncated behind them, and
        // `reorder_by_z` (which needs disjoint ascending ranges) slices the
        // real display list at the wrong offsets.
        let (so, sl) = (self.stack_ops.len(), self.stack_links.len());
        let (fo, flk) = (self.float_ops.len(), self.float_links.len());
        // Floats live on past the box that placed them, so a discarded layout
        // leaks exclusion rects into the real one: the next float finds a BFC
        // that looks full and drops below phantom neighbours.
        let fl = self.floats.len();
        let prev_cb = self.cb;
        self.path.push(ElemInfo::of(el));
        let bottom = self.layout_box(el, st, x, w.max(1), y);
        self.path.pop();
        self.ops.truncate(o);
        self.links.truncate(l);
        self.controls.truncate(c);
        self.stack_ops.truncate(so);
        self.stack_links.truncate(sl);
        self.float_ops.truncate(fo);
        self.float_links.truncate(flk);
        self.floats.truncate(fl);
        self.cb = prev_cb;
        bottom - y
    }

    /// Flex layout (css-flexbox-1 subset): row/column direction, the container's
    /// own box model (width/margins/padding/background/border/explicit height,
    /// establishing the containing block), `flex-grow`/`-shrink`/`-basis` with
    /// the automatic content minimum size, per-item margins (incl. `margin:auto`
    /// on the main axis), `gap`, `justify-content`, `align-items`/`align-self`
    /// (start/center/end/stretch), and `flex-wrap` (multi-line). Not yet:
    /// reverse directions, `align-content`, baseline alignment.
    fn layout_flex(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // Flex items = in-flow child elements; abspos children are out of flow.
        // Structural selectors count EVERY element sibling, so the position is
        // tracked independently of which children become items.
        let mut items: Vec<(&Element, ComputedStyle)> = Vec::new();
        let sib_count = el.children.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        let mut siblings: Vec<ElemInfo> = Vec::new();
        for c in &el.children {
            if let Node::Element(ce) = c {
                let mut cs = self.styled(ce, st, &siblings, sib_count);
                siblings.push(ElemInfo::of(ce));
                // A flex item is blockified (css-display-3 §2.7).
                if matches!(cs.display, Display::Inline | Display::InlineBlock) {
                    cs.display = Display::Block;
                }
                if cs.display == Display::None {
                    continue;
                }
                if matches!(cs.position, Position::Absolute | Position::Fixed) {
                    self.path.push(ElemInfo::of(ce));
                    self.layout_abs(ce, &cs, self.cb.0, self.cb.1);
                    self.path.pop();
                    continue;
                }
                items.push((ce, cs));
            }
        }
        // A `::before`/`::after` with a box of its own is a flex item like any
        // other child (CSS Display 3 §2.2). It is a fixed rectangle and never a
        // flexible length, so instead of threading a second item KIND through
        // the whole §9.7 machinery it is reserved off the main axis here and
        // the real items share what is left. Exact for the idiom this serves —
        // a `content: ""` box with a definite width.
        // Only the LEADING one: a trailing box would have to sit right behind
        // the last item, and reserving it off the axis puts it at the
        // container's far edge instead (`flexbox_generated` measures exactly
        // that gap). The icon-before-content idiom this serves needs the lead;
        // the tail waits until generated content is a real flex item.
        let lead_box = self.pseudo_box(el, st, PseudoElem::Before, w);
        let tail_box: Option<AtomicBox> = None;
        // Empty flex box: fall back to block so its own box decoration still paints.
        if items.is_empty() && lead_box.is_none() && tail_box.is_none() {
            return self.layout_block(el, st, x, w, y0);
        }
        items.sort_by_key(|(_, s)| s.order); // stable → equal order keeps DOM order

        // Container horizontal box (mirrors `layout_block`/`layout_grid`). The
        // border counts: a flex container with `width: 40em; border: 1px` has a
        // 642px border box like any other block, and leaving it out made every
        // bordered flex container two pixels narrow AND shifted its content.
        let (cw, off_left) = resolve_block_h(st, w as f32);
        let content_x = x + off_left as i32;
        let content_w = cw.max(1.0) as i32;
        let box_left = content_x - st.pad_left as i32 - st.border_left.width as i32;
        let box_w = content_w + (st.pad_left + st.pad_right) as i32 + st.border_x() as i32;
        let bg_idx = self.ops.len();
        let content_top = y0 + st.pad_top as i32 + st.border_top.width as i32;
        // Along the main axis in a row. `content_x`/`content_w` shrink around
        // the generated boxes so the real items never overlap them.
        let row = st.flex_row;
        let (lead_w, tail_w) = (
            lead_box.as_ref().map_or(0, |b| if row { b.w } else { 0 }),
            tail_box.as_ref().map_or(0, |b| if row { b.w } else { 0 }),
        );
        let gen_x = content_x;
        let (content_x, content_w) = (content_x + lead_w, (content_w - lead_w - tail_w).max(1));

        let prev_cb = self.cb;
        if st.position != Position::Static {
            self.cb = padding_cb(st, content_x, content_top, content_w);
        }

        // Definite container content height (for cross-stretch / main-axis flex).
        let def_h: Option<f32> = content_height_of(st, st.height);

        let prev_cb_h = self.cb_h;
        self.cb_h = def_h;
        let mut content_h = if items.is_empty() {
            0
        } else if st.flex_row {
            self.flex_row(&items, st, content_x, content_w, content_top, def_h)
        } else {
            self.flex_column(&items, st, content_x, content_w, content_top, def_h)
        };
        // Place the generated boxes now that the line's height is known: on the
        // main axis in a row (centred on the cross axis, which is what
        // `align-items: center` does for the icon idiom), stacked before and
        // after the content in a column.
        self.cb_h = prev_cb_h;
        for (b, at_start) in [(lead_box, true), (tail_box, false)] {
            let Some(mut b) = b else { continue };
            let (dx, dy) = if row {
                let line_h = def_h.map(|v| v as i32).unwrap_or(content_h).max(b.h);
                content_h = content_h.max(b.h);
                let cx = if at_start { gen_x } else { gen_x + lead_w + content_w };
                (cx, content_top + (line_h - b.h) / 2)
            } else if at_start {
                let d = (content_x, content_top);
                content_h += b.h;
                d
            } else {
                let d = (content_x, content_top + content_h);
                content_h += b.h;
                d
            };
            translate_op_list(&mut b.ops, dx, dy);
            self.ops.append(&mut b.ops);
        }
        self.cb = prev_cb;

        // Explicit / min / max height clamp the content-box height.
        let px_h = |len: Len| content_height_of(st, len).map(|v| v as i32);
        let mut ch = content_h;
        if let Some(h) = px_h(st.height) {
            ch = h;
        }
        if let Some(mn) = px_h(st.min_height) {
            ch = ch.max(mn);
        }
        if let Some(mx) = px_h(st.max_height) {
            ch = ch.min(mx);
        }

        let y = content_top + ch + st.pad_bottom as i32 + st.border_bottom.width as i32;
        self.paint_box_decoration(st, box_left, y0, box_w, y - y0, bg_idx);
        self.place_abs_pseudos(el, st, box_left, y0, box_w, y - y0);
        y
    }

    /// Row flex (main axis = horizontal, cross axis = vertical). `def_cross` is
    /// the container's definite content height (cross size) if any. Returns the
    /// content-box height consumed by all lines.
    fn flex_row(
        &mut self,
        items: &[(&'a Element, ComputedStyle)],
        st: &ComputedStyle,
        x: i32,
        w: i32,
        y0: i32,
        def_cross: Option<f32>,
    ) -> i32 {
        let avail = w as f32;
        let main_gap = st.gap;
        let line_gap = st.grid_row_gap; // cross-axis gap between wrapped lines

        // — per-item metrics (content-box main size = width) —
        let m = self.flex_metrics(items, avail, true);

        // — line breaking (flex-wrap) —
        let lines = flex_break_lines(&m, avail, main_gap, st.flex_wrap, st.flex_balance);

        let mut cross_y = y0;
        for line in &lines {
            let (idx0, idx1) = (line.0, line.1);
            let li = &m[idx0..idx1];
            let ln = li.len();
            let gaps_total = main_gap * (ln as f32 - 1.0).max(0.0);

            // Resolve flexible lengths within this line's available main size.
            let size = resolve_flex_line(li, avail, gaps_total);

            // Leftover main space → justify-content, unless main-axis auto margins
            // absorb it.
            let used: f32 = li
                .iter()
                .zip(&size)
                .map(|(it, &sz)| it.m_lead + it.m_trail + sz + it.main_pad)
                .sum::<f32>()
                + gaps_total;
            let leftover = avail - used;
            let n_auto: usize = li.iter().map(|it| it.m_lead_auto as usize + it.m_trail_auto as usize).sum();
            let (offset, extra_gap, auto_each) = if leftover > 0.5 && n_auto > 0 {
                (0.0, 0.0, leftover / n_auto as f32)
            } else {
                let lo = leftover.max(0.0);
                let (o, g) = match st.justify {
                    Justify::Start => (0.0, 0.0),
                    Justify::End => (lo, 0.0),
                    Justify::Center => (lo / 2.0, 0.0),
                    Justify::Between => (0.0, if ln > 1 { lo / (ln as f32 - 1.0) } else { 0.0 }),
                    Justify::Around => (lo / (2.0 * ln as f32), lo / ln as f32),
                    Justify::Evenly => (lo / (ln as f32 + 1.0), lo / (ln as f32 + 1.0)),
                };
                (o, g, 0.0)
            };

            // Main-axis positions (border-box left) per item.
            let mut item_x = alloc::vec![0.0f32; ln];
            let mut main = x as f32 + offset;
            for k in 0..ln {
                main += li[k].m_lead + if li[k].m_lead_auto { auto_each } else { 0.0 };
                item_x[k] = main;
                let box_main = size[k] + li[k].main_pad;
                main += box_main + li[k].m_trail + if li[k].m_trail_auto { auto_each } else { 0.0 }
                    + main_gap + extra_gap;
            }

            // Natural cross size (height) at the resolved width, to size the line.
            let mut h_nat = alloc::vec![0i32; ln];
            for k in 0..ln {
                let (el, s) = items[idx0 + k];
                let s_meas = flex_item_style(&s, size[k], None, true);
                let box_main = (size[k] + li[k].main_pad).max(1.0) as i32;
                h_nat[k] = self.measure_box_height(el, &s_meas, item_x[k] as i32, box_main, cross_y);
            }

            // Line cross size: a single unwrapped line fills a definite container
            // height; otherwise it's the tallest item margin box.
            //
            // The line is sized from each item's HYPOTHETICAL cross size
            // (Flexbox §9.4 step 7) — the natural one clamped by its own
            // `min-`/`max-height`. Using the raw natural height left the line
            // short of any item held open by a `min-height`, and that item then
            // hung out past the container: Wikipedia's search button is
            // `min-height: 32px` inside a 30px-tall line.
            let nat_line = (0..ln)
                .map(|k| {
                    let hypo = clamp_cross(h_nat[k] as f32, li[k].min_cross, li[k].max_cross);
                    li[k].cm_lead as i32 + hypo as i32 + li[k].cm_trail as i32
                })
                .max()
                .unwrap_or(0);
            let line_cross = if lines.len() == 1 {
                def_cross.map(|c| c as i32).unwrap_or(nat_line)
            } else {
                nat_line
            };

            // Place each item within the line box on the cross axis.
            for k in 0..ln {
                let (el, s) = items[idx0 + k];
                let align = s.align_self.unwrap_or(st.align_items);
                let inner = (line_cross - li[k].cm_lead as i32 - li[k].cm_trail as i32).max(0);
                let stretch = align == CrossAlign::Stretch && li[k].cross_auto;
                let (forced_h, y) = if stretch {
                    let target = clamp_cross(inner as f32, li[k].min_cross, li[k].max_cross);
                    (Some(target), cross_y + li[k].cm_lead as i32)
                } else {
                    let h = h_nat[k];
                    let y = match align {
                        CrossAlign::End => cross_y + line_cross - li[k].cm_trail as i32 - h,
                        CrossAlign::Center => cross_y + li[k].cm_lead as i32 + (inner - h) / 2,
                        _ => cross_y + li[k].cm_lead as i32, // start / stretch-with-def-size / baseline
                    };
                    (None, y)
                };
                let s2 = flex_item_style(&s, size[k], forced_h, true);
                self.path.push(ElemInfo::of(el));
                // `layout_box` takes the box the caller resolved — the item's
                // BORDER box. `size[k]` is its content size, so the item's own
                // padding and border have to go back on, or a control (which
                // paints exactly this width) loses them and clips its label.
                let box_main = (size[k] + li[k].main_pad).max(1.0) as i32;
                let _ = self.layout_box(el, &s2, item_x[k] as i32, box_main, y);
                self.path.pop();
            }

            cross_y += line_cross + line_gap as i32;
        }
        (cross_y - line_gap.max(0.0) as i32 - y0).max(0)
    }

    /// Column flex (main axis = vertical, cross axis = horizontal). `def_cross`
    /// is the container's definite content height (main size) if any.
    fn flex_column(
        &mut self,
        items: &[(&'a Element, ComputedStyle)],
        st: &ComputedStyle,
        x: i32,
        w: i32,
        y0: i32,
        def_cross: Option<f32>,
    ) -> i32 {
        let n = items.len();
        let avail = w as f32; // cross-axis available (width)
        let gap = st.gap;

        // Cross-axis (horizontal) width + position, plus main-axis (vertical)
        // margins, per item. Cross axis is never flexed (grow/shrink are main).
        let mut cross_w = alloc::vec![0.0f32; n];
        let mut ix = alloc::vec![0i32; n];
        let mut mm_lead = alloc::vec![0.0f32; n];
        let mut mm_trail = alloc::vec![0.0f32; n];
        let mut h_nat = alloc::vec![0i32; n];
        for (i, (el, s)) in items.iter().enumerate() {
            let pad_h = s.pad_left + s.pad_right;
            let to_content = |px: f32| if s.box_border { (px - pad_h).max(0.0) } else { px };
            let ml = s.margin_left.px(avail).unwrap_or(0.0);
            let mr = s.margin_right.px(avail).unwrap_or(0.0);
            let align = s.align_self.unwrap_or(st.align_items);
            let width_auto = matches!(s.width, Len::Auto);
            let stretch = align == CrossAlign::Stretch && width_auto;
            let mut wd = if stretch {
                (avail - ml - mr).max(1.0)
            } else {
                s.width.px(avail).map(to_content).unwrap_or_else(|| self.intrinsic_width(el, s).0)
            };
            if let Some(mx) = s.max_width.px(avail) {
                wd = wd.min(to_content(mx));
            }
            if let Some(mn) = s.min_width.px(avail) {
                wd = wd.max(to_content(mn));
            }
            wd = wd.clamp(1.0, avail.max(1.0));
            cross_w[i] = wd;
            ix[i] = match align {
                CrossAlign::End => x + (avail - mr - wd) as i32,
                CrossAlign::Center => x + (ml + (avail - ml - mr - wd) / 2.0) as i32,
                _ => x + ml as i32, // start / stretch
            };
            mm_lead[i] = s.margin_top;
            mm_trail[i] = s.margin_bottom;
            let s_meas = flex_item_style(s, wd, None, false);
            h_nat[i] = self.measure_box_height(el, &s_meas, ix[i], wd.max(1.0) as i32, y0);
        }

        // Total intrinsic main size (heights + vertical margins + gaps).
        let gaps_total = gap * (n as f32 - 1.0).max(0.0);
        let sum_h: f32 = (0..n).map(|i| mm_lead[i] + h_nat[i] as f32 + mm_trail[i]).sum();
        let intrinsic = sum_h + gaps_total;
        // A definite container height gives free main space → justify-content.
        let free = def_cross.map(|c| c - intrinsic).unwrap_or(0.0).max(0.0);
        let (offset, extra_gap) = match st.justify {
            Justify::End => (free, 0.0),
            Justify::Center => (free / 2.0, 0.0),
            Justify::Between => (0.0, if n > 1 { free / (n as f32 - 1.0) } else { 0.0 }),
            Justify::Around => (free / (2.0 * n as f32), free / n as f32),
            Justify::Evenly => (free / (n as f32 + 1.0), free / (n as f32 + 1.0)),
            Justify::Start => (0.0, 0.0),
        };

        let mut y = y0 as f32 + offset;
        for (i, (el, s)) in items.iter().enumerate() {
            y += mm_lead[i];
            let s2 = flex_item_style(s, cross_w[i], None, false);
            self.path.push(ElemInfo::of(el));
            let bottom = self.layout_box(el, &s2, ix[i], cross_w[i].max(1.0) as i32, y as i32);
            self.path.pop();
            y = bottom as f32 + mm_trail[i];
            if i + 1 < n {
                y += gap + extra_gap;
            }
        }
        let used = (y as i32 - y0).max(0);
        match def_cross {
            Some(c) => used.max(c as i32),
            None => used,
        }
    }

    /// Per-item flex metrics on the main axis (row: width; column: width used as
    /// cross). `row` selects which margins/paddings are the main vs cross axis.
    fn flex_metrics(&mut self, items: &[(&'a Element, ComputedStyle)], avail: f32, row: bool) -> Vec<FlexItem> {
        let mut out = Vec::with_capacity(items.len());
        for (el, s) in items {
            // Padding AND border on each axis: every consumer below adds
            // `main_pad` to a CONTENT size to get a border box, so leaving the
            // border out makes each of them short by it.
            let (main_pad, cross_pad) = if row {
                (s.pad_left + s.pad_right + s.border_x(), s.pad_top + s.pad_bottom + s.border_y())
            } else {
                (s.pad_top + s.pad_bottom + s.border_y(), s.pad_left + s.pad_right + s.border_x())
            };
            // Main-axis leading/trailing margins (row: left/right; column: top/bottom).
            let (m_lead_len, m_trail_len) = if row {
                (s.margin_left, s.margin_right)
            } else {
                (Len::Px(s.margin_top), Len::Px(s.margin_bottom))
            };
            let m_lead_auto = matches!(m_lead_len, Len::Auto);
            let m_trail_auto = matches!(m_trail_len, Len::Auto);
            let m_lead = m_lead_len.px(avail).unwrap_or(0.0);
            let m_trail = m_trail_len.px(avail).unwrap_or(0.0);
            // Cross-axis margins.
            let (cm_lead, cm_trail) = if row {
                (s.margin_top, s.margin_bottom)
            } else {
                (s.margin_left.px(avail).unwrap_or(0.0), s.margin_right.px(avail).unwrap_or(0.0))
            };
            // The item's main-axis size property (row: width; column: height).
            let (main_size, min_size, max_size) = if row {
                (s.width, s.min_width, s.max_width)
            } else {
                (s.width, s.min_width, s.max_width) // column cross axis is width
            };
            let to_content = |px: f32| if s.box_border { (px - main_pad).max(0.0) } else { px };
            let spec = main_size.px(avail).map(to_content);
            // `intrinsic_width` reports the element's CONTENT width — its own
            // padding and border are added by whoever lays it out. A CONTROL is
            // the exception: it has no children to measure, so `control_box`
            // hands back the finished box, painted with the control's own
            // chrome and ignoring the CSS padding entirely.
            //
            // The flex algorithm takes bases as content sizes and removes every
            // item's padding+border from the line once, in `resolve_flex_line`'s
            // `fixed`. So a control has to give that back, or it reserves chrome
            // it never uses and a growing sibling is short by exactly that much
            // — Wikipedia's search field stopped 22px (its button's padding)
            // before the group's right edge.
            let control_chrome = if crate::forms::kind_of(el).is_some() { main_pad } else { 0.0 };
            let (pref_bb, minc_bb) = self.intrinsic_width(el, s);
            let (pref, minc) = ((pref_bb - control_chrome).max(0.0), (minc_bb - control_chrome).max(0.0));
            let base = match s.flex_basis {
                FlexBasis::Px(p) => to_content(p),
                FlexBasis::Pct(p) => to_content(p / 100.0 * avail),
                FlexBasis::Auto => spec.unwrap_or(pref),
            };
            // Automatic minimum size = min(content min, specified suggestion).
            let mut floor = minc.min(spec.unwrap_or(minc));
            if let Some(mn) = min_size.px(avail) {
                floor = floor.max(to_content(mn));
            }
            let ceil = max_size.px(avail).map(to_content).unwrap_or(f32::INFINITY);
            let hypo = base.clamp(floor.min(ceil), ceil);
            // Cross-axis stretch is possible only when the cross size is auto.
            let cross_auto = if row {
                matches!(s.height, Len::Auto)
            } else {
                matches!(s.width, Len::Auto)
            };
            let (min_cross, max_cross) = if row {
                (
                    s.min_height.px(avail).unwrap_or(0.0),
                    s.max_height.px(avail).unwrap_or(f32::INFINITY),
                )
            } else {
                (0.0, f32::INFINITY)
            };
            out.push(FlexItem {
                m_lead,
                m_trail,
                m_lead_auto,
                m_trail_auto,
                main_pad,
                cross_pad,
                cm_lead,
                cm_trail,
                base,
                hypo,
                floor,
                ceil,
                grow: s.flex_grow,
                shrink: s.flex_shrink,
                cross_auto,
                min_cross,
                max_cross,
            });
        }
        out
    }

    /// Shift a contiguous slice of already-emitted ops + links by `(dx, dy)` —
    /// used to place a flex item on the cross axis, and to offset a
    /// `position:relative` box after it is laid in flow.
    fn shift_ops(&mut self, o0: usize, o1: usize, l0: usize, l1: usize, c0: usize, dx: i32, dy: i32) {
        for op in &mut self.ops[o0..o1] {
            match op {
                DrawOp::Text { x, y, .. }
                | DrawOp::Rect { x, y, .. }
                | DrawOp::RoundRect { x, y, .. }
                | DrawOp::Image { x, y, .. }
                | DrawOp::BgImage { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
            }
        }
        for lk in &mut self.links[l0..l1] {
            lk.x += dx;
            lk.y += dy;
        }
        // Control hit rects must follow their painted box (relative offsets,
        // flex cross-alignment) or clicks land where the box used to be.
        for c in &mut self.controls[c0..] {
            c.x += dx;
            c.y += dy;
        }
    }
}

/// Resolved main-axis metrics for one flex item (all content-box px). Margins/
/// paddings are split into the main axis (`m_*`, `main_pad`) and cross axis
/// (`cm_*`, `cross_pad`) by the caller's direction.
struct FlexItem {
    m_lead: f32,
    m_trail: f32,
    m_lead_auto: bool,
    m_trail_auto: bool,
    main_pad: f32,
    #[allow(dead_code)]
    cross_pad: f32,
    cm_lead: f32,
    cm_trail: f32,
    base: f32,
    hypo: f32, // base clamped to [floor, ceil]
    floor: f32,
    ceil: f32,
    grow: f32,
    shrink: f32,
    cross_auto: bool, // cross size is `auto` → eligible for stretch
    min_cross: f32,
    max_cross: f32,
}

/// Greedily fill lines until the next item's outer main size would overflow
/// `cap` (at least one item per line).
fn flex_pack_lines(m: &[FlexItem], cap: f32, gap: f32) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut used = 0.0f32;
    let mut count = 0usize;
    for (i, it) in m.iter().enumerate() {
        let outer = it.m_lead + it.m_trail + it.main_pad + it.hypo;
        let add = if count == 0 { outer } else { outer + gap };
        if count > 0 && used + add > cap + 0.5 {
            lines.push((start, i));
            start = i;
            used = outer;
            count = 1;
        } else {
            used += add;
            count += 1;
        }
    }
    lines.push((start, m.len()));
    lines
}

/// Partition items into flex lines. Without `wrap`, one line holds everything.
/// With `wrap`, greedily pack to `avail`. With `balance` (css-flexbox-2), pack
/// into the fewest lines, then shrink the line capacity to the smallest value
/// that still fits that many lines — evening the items across the lines.
fn flex_break_lines(m: &[FlexItem], avail: f32, gap: f32, wrap: bool, balance: bool) -> Vec<(usize, usize)> {
    if !wrap || m.is_empty() {
        return alloc::vec![(0, m.len())];
    }
    let greedy = flex_pack_lines(m, avail, gap);
    if !balance || greedy.len() <= 1 {
        return greedy;
    }
    let target = greedy.len();
    let max_item = m
        .iter()
        .map(|it| it.m_lead + it.m_trail + it.main_pad + it.hypo)
        .fold(0.0f32, f32::max);
    let (mut lo, mut hi) = (max_item, avail);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if flex_pack_lines(m, mid, gap).len() <= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    flex_pack_lines(m, hi, gap)
}

/// Resolve flexible lengths for one line: grow into positive free space (by
/// `flex-grow`) or shrink out of negative free space (by `flex-shrink × base`),
/// each clamped to `[floor, ceil]`. Returns the used content main size per item.
fn resolve_flex_line(li: &[FlexItem], avail: f32, gaps_total: f32) -> Vec<f32> {
    let n = li.len();
    // Margins, padding and borders do not flex: take them out once, and let the
    // content boxes share what is left. Leaving them in handed the line every
    // item's padding as extra free space.
    let fixed: f32 = li.iter().map(|it| it.m_lead + it.m_trail + it.main_pad).sum::<f32>() + gaps_total;
    let inner = avail - fixed;
    let clamp = |it: &FlexItem, v: f32| v.clamp(it.floor.min(it.ceil), it.ceil);

    // §9.7.1 — grow or shrink is decided once, from the hypothetical sizes.
    let hypo_sum: f32 = li.iter().map(|it| it.hypo).sum();
    let growing = hypo_sum < inner;
    let factor = |it: &FlexItem| if growing { it.grow } else { it.shrink };

    // §9.7.2 — freeze the inflexible ones straight away: no flex factor, or a
    // base size already past the hypothetical size in the direction we would
    // move it (min/max already had the last word there).
    let mut target: Vec<f32> = Vec::with_capacity(n);
    let mut frozen: Vec<bool> = Vec::with_capacity(n);
    for it in li {
        let stuck = factor(it) == 0.0 || (growing && it.base > it.hypo) || (!growing && it.base < it.hypo);
        frozen.push(stuck);
        target.push(if stuck { it.hypo } else { it.base });
    }

    // §9.7.3 — free space counts frozen items at their target and the rest at
    // their flex base size.
    let free_of = |target: &[f32], frozen: &[bool]| -> f32 {
        inner - (0..n).map(|i| if frozen[i] { target[i] } else { li[i].base }).sum::<f32>()
    };
    let initial_free = free_of(&target, &frozen);

    // §9.7.4 — the loop. Every round freezes at least one item, so `n` rounds
    // always finish. This is the part a single pass cannot do: space clamped
    // away from one item has to come back round to the items that can still
    // take it, which is exactly what the `flex-0/1/N-*` family measures.
    for _ in 0..n {
        if frozen.iter().all(|f| *f) {
            break;
        }
        let mut remaining = free_of(&target, &frozen);
        let fsum: f32 = (0..n).filter(|&i| !frozen[i]).map(|i| factor(&li[i])).sum();
        // Flex factors totalling less than one only claim that fraction of the
        // ORIGINAL free space; the remainder stays with the container.
        if fsum < 1.0 {
            let capped = initial_free * fsum;
            if capped.abs() < remaining.abs() {
                remaining = capped;
            }
        }
        // Growing shares out by flex-grow; shrinking by flex-shrink weighted
        // with the base size, so a big item gives up more than a small one.
        let scaled = |i: usize| if growing { li[i].grow } else { li[i].shrink * li[i].base };
        let denom: f32 = (0..n).filter(|&i| !frozen[i]).map(scaled).sum();
        let mut viol = alloc::vec![0.0f32; n];
        let mut total_viol = 0.0f32;
        for i in 0..n {
            if frozen[i] {
                continue;
            }
            let unclamped = if denom > 0.0 { li[i].base + remaining * scaled(i) / denom } else { li[i].base };
            target[i] = clamp(&li[i], unclamped);
            viol[i] = target[i] - unclamped;
            total_viol += viol[i];
        }
        // §9.7.4e — freeze whoever was pulled past a limit; their violation is
        // what the next round redistributes. No violation → everyone is done.
        for i in 0..n {
            if frozen[i] {
                continue;
            }
            frozen[i] = total_viol == 0.0 || (total_viol > 0.0 && viol[i] > 0.0) || (total_viol < 0.0 && viol[i] < 0.0);
        }
    }
    target
}

/// Clamp a stretched cross size to the item's cross min/max.
fn clamp_cross(v: f32, min: f32, max: f32) -> f32 {
    v.max(min).min(max).max(0.0)
}

/// Build a flex item's style for layout: force its main-axis size (`main`,
/// content-box) and, when stretching, its cross-axis size (`forced_cross`,
/// border-box). Item margins are zeroed — the flex code positions the item by
/// hand — while keeping the item's own box-sizing for padding conversion.
fn flex_item_style(s: &ComputedStyle, main: f32, forced_cross: Option<f32>, row: bool) -> ComputedStyle {
    let mut s2 = *s;
    s2.margin_left = Len::Px(0.0);
    s2.margin_right = Len::Px(0.0);
    s2.margin_top = 0.0;
    s2.margin_bottom = 0.0;
    let main_px = |content: f32, pad: f32| Len::Px(if s.box_border { content + pad } else { content });
    let main_chrome = s.pad_left + s.pad_right + s.border_x();
    if row {
        s2.width = main_px(main, main_chrome);
        if let Some(c) = forced_cross {
            // `c` is the stretched BORDER-box cross size. `Len::Px` means a
            // border-box value under `box-sizing:border-box` and a content-box
            // one otherwise (see `content_height_of`), so the content-box case
            // has to give back padding AND border. Leaving the border out made
            // a bordered flex item exactly `border_y()` taller than the line it
            // was stretched into — the same box-model twin that bucket item 31
            // removed from `layout_flex` and `layout_grid`; this was the third
            // copy.
            let inner_v = s.pad_top + s.pad_bottom + s.border_y();
            s2.height = Len::Px(if s.box_border { c } else { (c - inner_v).max(0.0) });
        }
    } else {
        // Column: main axis is vertical (height), cross is horizontal (width).
        s2.width = main_px(main, main_chrome);
        let _ = forced_cross; // column forces cross via `width` above
    }
    s2
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
    gather_text(&el.children, &mut raw);
    // Browsers strip a single leading newline right after <pre>.
    let raw = raw.strip_prefix('\n').unwrap_or(&raw);
    // `line-height` governs a preformatted block's line advance exactly as it
    // does an inline formatting context's; the baseline sits at the content
    // ascent plus half the leading.
    let used_lh = st.line_height.px(st.font_px).unwrap_or(0.0);
    let (run_asc, run_h) = run_metrics(font, st.font_px, used_lh);
    let lh = ceil_i32(run_h).max(1);
    let asc = ascent_i(font, st.font_px);
    let baseline_off = if used_lh > 0.0 { run_asc as i32 - asc } else { (lh - asc) / 2 };
    let mut y = y0;
    for line in raw.split('\n') {
        let text = line.replace('\t', "    ");
        if !text.is_empty() && !st.hidden && !st.transparent {
            ops.push(DrawOp::Text {
                x,
                y: y + baseline_off,
                size: st.font_px,
                color: st.color,
                bold: st.bold,
                italic: st.italic,
                mono: st.mono,
                text,
            });
        }
        y += lh;
    }
    y
}

/// The line being measured: its text, plus the horizontal frame that rides on
/// it. The frame is what inline boxes on the line reserve (margin + border +
/// padding) — unbreakable width, so it adds to the min- and max-content
/// measurement alike.
#[derive(Default)]
struct Run {
    text: String,
    frame: f32,
    /// Running sum of the margin-box widths of a consecutive group of floated
    /// siblings. They sit SIDE BY SIDE, so at max-content they add up instead
    /// of competing; any non-float box ends the group.
    floats: f32,
    /// Sum of the outer widths of the atomic inline boxes on this line —
    /// `inline-block`, images, form controls. They sit ON the line next to the
    /// text, so at max-content they add to it. Measuring them as block-level
    /// children instead took the WIDEST of them, which is why a shrink-to-fit
    /// box around two inline-blocks came out one-child wide and stacked them.
    atomic: f32,
    /// The widest min-content among those boxes. A line CAN break between two
    /// of them, so at MIN-content they compete rather than add.
    atomic_min: f32,
}

/// The horizontal space one inline box adds to its line. `flow` advances the
/// pen by exactly this (as `InlineBox::lead + trail`), so the measurement has
/// to count it too or every shrink-to-fit box around a padded `<span>` comes
/// out too narrow. `cb` is 0 while measuring: a percentage margin has no basis
/// yet, so it contributes nothing.
fn inline_frame(st: &ComputedStyle, cb: f32) -> f32 {
    st.margin_left.px(cb).unwrap_or(0.0)
        + st.margin_right.px(cb).unwrap_or(0.0)
        + st.border_x()
        + st.pad_left
        + st.pad_right
}

/// Measure the inline text collected so far as one line and fold it into a
/// box's running (max-content, min-content), then clear it. `white-space:
/// normal` collapses every whitespace run — including the newlines and
/// indentation between sibling tags in pretty-printed markup — to one space
/// first, or source formatting would count as visible width.
fn flush_run(fonts: &crate::fonts::Fonts, st: &ComputedStyle, run: &mut Run, pref: &mut f32, min: &mut f32, horiz: bool) {
    if run.text.is_empty() && run.frame == 0.0 && run.atomic == 0.0 && run.atomic_min == 0.0 {
        return;
    }
    let frame = core::mem::take(&mut run.frame);
    let atomic = core::mem::take(&mut run.atomic);
    let atomic_min = core::mem::take(&mut run.atomic_min);
    // `white-space: pre` keeps the source line breaks, so each source line is
    // its own line box and the widest one wins — collapsing them into one
    // would measure a whole code block as a single enormous line.
    if st.pre {
        let font = fonts.pick(st.bold, st.italic, st.mono);
        let mut widest = 0.0f32;
        for line in run.text.lines() {
            // Trailing spaces hang past the line box, so they never widen it
            // (css-text-3 §8). Leading ones DO count under `pre`.
            widest = widest.max(measure(font, line.trim_end(), st.font_px));
        }
        let widest = widest + frame + atomic;
        run.text.clear();
        let widest_min = widest.max(atomic_min);
        if horiz {
            *pref += widest;
            *min += widest_min;
        } else {
            *pref = pref.max(widest);
            *min = min.max(widest_min);
        }
        return;
    }
    let collapsed = collapse_whitespace(&run.text);
    run.text.clear();
    // The run's OWN font, not `regular()`: monospace advances wider than the
    // proportional face, so measuring mono content with it under-sizes every
    // auto table column that holds code.
    let font = fonts.pick(st.bold, st.italic, st.mono);
    let size = st.font_px;
    let p = measure(font, collapsed.trim(), size) + frame + atomic;
    // `white-space: nowrap` has no break opportunities, so min-content is the
    // whole line — not its widest word. Without this a shrink-to-fit box around
    // a nowrap run is sized to one word and the run hangs out of it.
    let m = if st.nowrap {
        p
    } else {
        let words = collapsed.split_whitespace().map(|wd| measure(font, wd, size)).fold(0.0f32, f32::max) + frame;
        // The line may break either side of an atomic inline, so its own
        // min-content competes with the widest word rather than adding to it.
        words.max(atomic_min)
    };
    // Inside a row (or any inline-axis container) a run of stray inline content
    // is one anonymous cell sitting BESIDE its siblings, so it adds to the
    // row's width instead of competing with it (CSS2.1 §17.2.1).
    if horiz {
        *pref += p;
        *min += m;
    } else {
        *pref = pref.max(p);
        *min = min.max(m);
    }
}

fn gather_text(nodes: &[Node], out: &mut String) {
    for c in nodes {
        match c {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => gather_text(&e.children, out),
        }
    }
}

/// Collapse every run of whitespace to a single space (CSS2.1 `white-space:
/// normal`), so measuring concatenated multi-node text (`intrinsic_width`)
/// doesn't count source-formatting newlines/indentation as visible width.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out
}

impl<'a> Ctx<'a> {
    /// Collect an inline element's subtree into the current inline run
    /// (recursing through nested inline elements, carrying each one's style +
    /// link href). `el` is already on `self.path` when this is called.
    /// Lay an `inline-block` out at the origin and capture everything it
    /// painted, so the line box can place a finished rectangle. Width is
    /// shrink-to-fit for `auto` (CSS2.1 §10.3.9, the same formula floats use).
    ///
    /// The box establishes its own block formatting context, so the parent's
    /// floats must not reach into it — and its own must not leak out
    /// ([[feedback-speculative-layout-state]]: every throwaway context has to
    /// put back what it took).
    fn inline_block_box(&mut self, el: &'a Element, st: &ComputedStyle, avail_w: i32) -> Option<AtomicBox> {
        if st.hidden || st.transparent {
            return None;
        }
        let cbw = avail_w as f32;
        let ml = st.margin_left.px(cbw).unwrap_or(0.0).max(0.0);
        let mr = st.margin_right.px(cbw).unwrap_or(0.0).max(0.0);
        let pad_border = st.pad_left + st.pad_right + st.border_x();
        let content_w = match st.width {
            Len::Auto => {
                let (pref, min) = self.intrinsic_width(el, st);
                let room = (cbw - ml - mr - pad_border).max(0.0);
                pref.min(room).max(min).max(0.0)
            }
            other => {
                let v = other.px(cbw).unwrap_or(0.0);
                if st.box_border { (v - pad_border).max(0.0) } else { v }
            }
        };
        let outer_w = ceil_i32(content_w + pad_border + ml + mr).max(1);

        let (o0, l0, c0) = (self.ops.len(), self.links.len(), self.controls.len());
        let i0 = self.inspects.len();
        let saved_floats = core::mem::take(&mut self.floats);
        let saved_baseline = self.last_baseline.take();
        self.path.push(ElemInfo::of(el));
        // `layout_box` re-adds margin-left + padding, so it gets the MARGIN-box
        // width — the same contract `place_float` uses.
        let border_bottom = self.layout_box(el, st, 0, outer_w, st.margin_top as i32);
        self.path.pop();
        self.floats = saved_floats;
        let inner_baseline = self.last_baseline.take();
        self.last_baseline = saved_baseline;

        let ops: Vec<DrawOp> = self.ops.drain(o0..).collect();
        let links: Vec<LinkRect> = self.links.drain(l0..).collect();
        let controls: Vec<ControlRect> = self.controls.drain(c0..).collect();
        let inspects: Vec<InspectBox> = self.inspects.drain(i0..).collect();
        let h = (border_bottom + st.margin_bottom as i32).max(0);
        // The box aligns on its LAST line box's baseline; with no in-flow line
        // box, or when it clips its overflow, it aligns on its bottom margin
        // edge instead (CSS2.1 §10.8.1).
        let baseline = match inner_baseline {
            Some(b) if !st.overflow_clip => b.clamp(0, h),
            _ => h,
        };
        Some(AtomicBox { ops, links, controls, inspects, w: outer_w, h, baseline, valign: st.valign })
    }

    /// The inline box an inline-level child needs, if any: one that paints
    /// something of its own or reserves horizontal space. An `<img>`, a form
    /// control and an `inline-block` are atomic — each already lays out and
    /// paints its own box — and a `<br>` has none at all.
    fn inline_box_of(&self, el: &Element, st: &ComputedStyle, cb_w: i32) -> Option<InlineBox> {
        if st.is_break || st.display != Display::Inline || el.tag == "img" || crate::forms::kind_of(el).is_some() {
            return None;
        }
        let cb = cb_w as f32;
        // `lead + trail` is `inline_frame` split at the content — the intrinsic
        // measurement counts the same total, keep the two in step.
        let (ml, mr) = (st.margin_left.px(cb).unwrap_or(0.0), st.margin_right.px(cb).unwrap_or(0.0));
        let lead = ml + st.border_left.width + st.pad_left;
        let trail = st.pad_right + st.border_right.width + mr;
        let edge = |s: &BorderSide| s.width > 0.0 && s.color.is_some();
        let paints = st.bg.is_some()
            || st.bg_layer.image.is_some()
            || st.mask_layer.image.is_some()
            || edge(&st.border_top)
            || edge(&st.border_right)
            || edge(&st.border_bottom)
            || edge(&st.border_left);
        if !paints && lead == 0.0 && trail == 0.0 {
            return None;
        }
        Some(InlineBox {
            st: *st,
            bg: self.bg_key(st.bg_layer.image),
            mask: self.bg_key(st.mask_layer.image),
            lead,
            trail,
            margin_left: ml,
            margin_right: mr,
        })
    }

    /// The box decoration an `<img>` paints around its pixels. A replaced
    /// element is atomic in the inline flow but it still has a box: MediaWiki
    /// frames every thumbnail with `border: 1px solid` on the `<img>` itself,
    /// and without this the picture sits in its figure with no frame at all.
    /// Reuses `InlineBox` for the values; only the vertical extent differs —
    /// an image's content box is the image, not a font's ascent + descent.
    fn image_deco(&self, st: &ComputedStyle) -> Option<InlineBox> {
        let edge = |s: &BorderSide| s.width > 0.0 && s.color.is_some();
        let paints = st.bg.is_some()
            || edge(&st.border_top)
            || edge(&st.border_right)
            || edge(&st.border_bottom)
            || edge(&st.border_left);
        if !paints {
            return None;
        }
        Some(InlineBox {
            st: *st,
            bg: None,
            mask: None,
            lead: st.border_left.width + st.pad_left,
            trail: st.pad_right + st.border_right.width,
            margin_left: 0.0,
            margin_right: 0.0,
        })
    }

    fn collect_inline(&mut self, el: &'a Element, st: &ComputedStyle, href: Option<&str>, inline: &mut Inline, bx: i32, bw: i32, by: i32) {
        if st.is_break {
            inline.brk();
            return;
        }
        // `display: inline-block` — lay the whole box out now (block model,
        // shrink-to-fit width) into its own display list, and hand the line
        // box a finished rectangle. Position comes later, in `emit_line`.
        if st.display == Display::InlineBlock {
            if let Some(b) = self.inline_block_box(el, st, bw) {
                inline.atomic(b);
            }
            return;
        }
        // An `<img>` inside inline content (e.g. `<a><img></a>` — Wikipedia's
        // thumbnails) is an atomic inline box; carry the enclosing link so it
        // stays clickable.
        if el.tag == "img" {
            let (iw, ih) = self.img_box(el, st);
            let src = el.attr("src").unwrap_or("").to_string();
            inline.image(src, iw, ih, href, el.attr("alt").unwrap_or("").trim().to_string(), st.hidden, st.transparent, self.image_deco(st));
            return;
        }
        // …and every other replaced element, laid out through the block model
        // and handed to the line as a finished rectangle.
        if replaced_intrinsic(el).is_some() {
            if let Some(b) = self.inline_block_box(el, st, bw) {
                inline.atomic(b);
            }
            return;
        }
        if let Some(kind) = crate::forms::kind_of(el) {
            if kind != ControlKind::Hidden {
                let ctl = self.control_box(el, st, kind, bw as f32);
                inline.control(ctl);
            }
            return;
        }
        let href = if st.is_link { el.attr("href").or(href) } else { href };
        let n0 = inline.item_count();
        // `el` was already counter-entered by the caller; bound the counters its
        // own descendants reset to this subtree (mirrors `flow_children`).
        let counter_base = self.counters.stack.len();
        // `el::before` — same anonymous-inline-box treatment as the block
        // path (`flow_children`), just feeding this inline run instead.
        if let Some(b) = self.pseudo_box(el, st, PseudoElem::Before, bw) {
            inline.atomic(b);
        } else if let Some((text, ps)) = self.pseudo(el, st, PseudoElem::Before) {
            inline.text(&text, &ps, href);
        }
        let sib_count = el.children.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        let mut siblings: Vec<ElemInfo> = Vec::new();
        for c in &el.children {
            match c {
                Node::Text(t) => inline.text(t, st, href),
                Node::Element(ce) => {
                    let cs = self.styled(ce, st, &siblings, sib_count);
                    siblings.push(ElemInfo::of(ce));
                    if cs.display == Display::None {
                        continue;
                    }
                    self.counters.enter(&cs, self.path.len());
                    // A floated inline element leaves the inline flow and is placed
                    // as a float; surrounding text wraps around it.
                    if cs.float != FloatKind::None {
                        self.place_float(ce, &cs, bx, bw, by);
                        continue;
                    }
                    let ib = self.inline_box_of(ce, &cs, bw).map(|b| inline.open_box(b));
                    self.path.push(ElemInfo::of(ce));
                    self.collect_inline(ce, &cs, href, inline, bx, bw, by);
                    self.path.pop();
                    if let Some(i) = ib {
                        inline.close_box(i);
                    }
                }
            }
        }
        if let Some(b) = self.pseudo_box(el, st, PseudoElem::After, bw) {
            inline.atomic(b);
        } else if let Some((text, ps)) = self.pseudo(el, st, PseudoElem::After) {
            inline.text(&text, &ps, href);
        }
        self.counters.stack.truncate(counter_base);
        if inline.item_count() == n0 {
            inline.strut(st);
        }
    }
}

// ── inline formatting context ──────────────────────────────────────────────

/// The visual attributes a text run needs to be measured + painted. Two runs
/// merge into one `DrawOp` only if these match (fewer ops, same pixels).
#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    /// `visibility:hidden` on the run's own style: it still measures and still
    /// takes its place on the line, it just isn't painted.
    hidden: bool,
    /// `opacity:0` on the run: painted as nothing, but still a click target.
    transparent: bool,
    size: f32,
    color: Rgb,
    bold: bool,
    italic: bool,
    mono: bool,
    valign: crate::style::VAlign,
    /// `text-decoration-line` bits (`style::DECO_*`).
    deco: u8,
    /// `overflow-wrap`/`word-break` allow splitting this run mid-word.
    break_word: bool,
    /// `white-space: nowrap` — this run's spaces are not break opportunities,
    /// so the line grows past its box rather than wrapping.
    nowrap: bool,
    /// Used `line-height` in px, or 0 for `normal` (use the face's metrics).
    lh: f32,
}

/// An inline-block's finished display list, laid out at the origin and
/// translated into place once the line box knows where it sits.
struct AtomicBox {
    ops: Vec<DrawOp>,
    links: Vec<LinkRect>,
    controls: Vec<ControlRect>,
    /// Inspect boxes recorded while laying this box out at the origin. They
    /// move with it — without that the dev tool reports every box inside an
    /// `inline-block` at the page's top-left corner, which reads as a layout
    /// bug that is not there.
    inspects: Vec<InspectBox>,
    /// Margin-box size — what the line reserves.
    w: i32,
    h: i32,
    /// Distance from the margin-box top to the baseline the line aligns on.
    baseline: i32,
    /// How the box sits on the line (CSS2.1 §10.8.1). Ignoring this put every
    /// atomic inline on the baseline, so a row of `inline-block`s of differing
    /// heights came out as a STAIRCASE — MediaWiki galleries, icon rows and
    /// badges all set `vertical-align: top` for exactly that reason.
    valign: crate::style::VAlign,
}

#[derive(Clone)]
/// An inline-level box that decorates itself or reserves horizontal space —
/// `<a class="external">` with its arrow icon, a badged `<span>`. Unlike a
/// block box it has no geometry of its own: it takes as many rectangles as it
/// has line boxes. Vertical padding and borders paint but never change the
/// line's height (CSS 2.1 §10.6.1); horizontal ones advance the flow.
struct InlineBox {
    st: ComputedStyle,
    /// Image keys, already registered with the layout that needs them — `flow`
    /// paints without a `Ctx` to ask.
    bg: Option<u64>,
    mask: Option<u64>,
    /// Resolved px the box adds before its content (`margin + border +
    /// padding`) and after it.
    lead: f32,
    trail: f32,
    margin_left: f32,
    margin_right: f32,
}

/// One inline item: a word, an atomic `<img>`, a form control, or a `<br>`.
enum Item {
    Word { text: String, style: RunStyle, href: Option<String>, space_before: bool },
    /// An inline box opens / closes around the items between them. Both index
    /// `Inline::boxes`; they nest, so a box always closes the innermost open one.
    /// The opening marker carries any collapsed space that precedes the box —
    /// that space belongs to the text around it, so it advances the pen OUTSIDE
    /// the box's background.
    BoxStart { bx: usize, space_before: bool },
    BoxEnd(usize),
    /// An inline box that generated no content of its own. It still contributes
    /// its leading to any line box it lands in (CSS 2.1 §10.8) — `<span
    /// style="line-height:5"></span>X` is a tall line — but it never makes a
    /// line non-empty, so a line holding nothing else is still not generated.
    Strut(RunStyle),
    Image { src: String, w: i32, h: i32, href: Option<String>, alt: String, space_before: bool, hidden: bool, transparent: bool, deco: Option<alloc::boxed::Box<InlineBox>> },
    Control { ctl: CtlBox, space_before: bool },
    /// `display: inline-block` — laid out already, waiting for its position.
    /// The finished display list is MOVED out when the line box places it;
    /// `flow` only has a shared borrow of the item list (`Placed::Control`
    /// borrows from it), hence the cell. Each `Inline` is flowed exactly once.
    Atomic { box_: RefCell<Option<AtomicBox>>, space_before: bool },
    Break,
}

// Form-control chrome metrics (px).
const CTL_PAD_X: i32 = 6;
const CTL_PAD_Y: i32 = 3;
const CTL_ARROW: i32 = 14;

/// A measured form control, ready to place on a line and paint.
struct CtlBox {
    seq: u32,
    kind: ControlKind,
    w: i32,
    h: i32,
    /// Displayed text: value, placeholder, or button/select label.
    text: String,
    /// `text` is a placeholder → paint it muted.
    ghost: bool,
    checked: bool,
    focused: bool,
    /// Caret position in characters, when this control has keyboard focus.
    caret: Option<usize>,
    /// The control's own `background-color`, if the page styled it.
    bg: Option<Rgb>,
    /// Leading text inset. Controls are atomic — we paint them with our own
    /// metrics — but a page that reserves room for an icon does it with
    /// `padding-left`, and ignoring that puts the text on top of the icon
    /// (Wikipedia's search field asks for 36px to clear its magnifier). CSS
    /// only ever WIDENS the inset; it cannot squeeze the text below `CTL_PAD_X`.
    pad_l: i32,
    /// The frame, in paint order top/right/bottom/left.
    border: [CtlSide; 4],
    style: RunStyle,
}

/// One edge of a control's frame. The UA gives every control a 1px one; a page
/// that writes any `border` longhand or shorthand owns all four instead —
/// including `border: none`, which is a declaration and not an absence.
#[derive(Clone, Copy)]
struct CtlSide {
    w: i32,
    /// `None` = paint in the UA's frame colour (the page named none).
    color: Option<Rgb>,
    /// `border-color: transparent` — keeps the width, paints nothing.
    transparent: bool,
}

/// A control's frame: the author's four sides once the page touched any of
/// them, else the UA's 1px. Google wraps its search button in a bordered
/// `<span>` and writes `border: none` on the `<input>`; painting our own frame
/// anyway put a second rectangle 1px down and right of the first.
fn ctl_border(st: &ComputedStyle) -> [CtlSide; 4] {
    let sides = [&st.border_top, &st.border_right, &st.border_bottom, &st.border_left];
    let owned = sides.iter().any(|s| s.specified);
    sides.map(|s| CtlSide {
        // An unstyled side still takes the author's `border-color` — the UA
        // frame is a real border, so colouring it is all a page needs to do.
        w: if owned { s.width as i32 } else { 1 },
        color: s.color,
        transparent: s.see_through,
    })
}

/// The control's authored default value (what it shows before the user edits).
fn default_value(el: &Element, kind: ControlKind) -> String {
    match kind {
        ControlKind::TextArea => {
            let mut s = String::new();
            gather_text(&el.children, &mut s);
            s
        }
        ControlKind::Select => {
            let (opts, sel) = crate::forms::options_of(el);
            sel.or_else(|| opts.first().map(|(v, _)| v.clone())).unwrap_or_default()
        }
        _ => el.attr("value").unwrap_or("").to_string(),
    }
}

fn repeat_char(c: char, n: usize) -> String {
    let mut s = String::with_capacity(n);
    for _ in 0..n.min(256) {
        s.push(c);
    }
    s
}

/// Label of the currently selected `<option>` (falls back to the raw value).
fn select_label(el: &Element, value: &str) -> String {
    let (opts, _) = crate::forms::options_of(el);
    opts.iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| value.to_string())
}

fn button_label(el: &Element, kind: ControlKind, value: &str) -> String {
    if el.tag == "button" {
        let mut s = String::new();
        gather_text(&el.children, &mut s);
        let s = collapse_whitespace(&s).trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    if !value.is_empty() {
        return value.to_string();
    }
    match kind {
        ControlKind::Reset => "Zurücksetzen".to_string(),
        _ => "Absenden".to_string(),
    }
}

/// Blend `t`/255 of `b` into `a` — control faces/borders are derived from the
/// page theme so they read correctly on light and dark backgrounds alike.
fn mix(a: Rgb, b: Rgb, t: u32) -> Rgb {
    let f = |x: u8, y: u8| (((x as u32) * (255 - t) + (y as u32) * t) / 255) as u8;
    Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// Paint one control's chrome + text at (x, top) and record its hit rect.
/// The UA palette to draw a form control's chrome from, given the colour its
/// text inherited. `theme` is used as-is when the two agree, so a page that
/// says nothing keeps following the device; only a page that paints against
/// the theme gets a flipped palette. Approximates CSS Color Adjust's
/// `color-scheme`, which real pages almost never declare.
fn surface_palette(theme: &Theme, text: Rgb) -> Theme {
    let light_text = luma(text) >= 128;
    if light_text == theme.is_dark() {
        return *theme;
    }
    if light_text {
        Theme::DARK
    } else {
        Theme {
            bg: Rgb(255, 255, 255),
            text: Rgb(32, 33, 34),
            heading: Rgb(32, 33, 34),
            link: Rgb(51, 102, 204),
            muted: Rgb(114, 119, 124),
            rule: Rgb(162, 169, 177),
        }
    }
}

/// Rec. 601 luma, the same measure `Theme::is_dark` uses.
fn luma(c: Rgb) -> u32 {
    (c.0 as u32 * 299 + c.1 as u32 * 587 + c.2 as u32 * 114) / 1000
}

fn paint_control(
    fonts: &crate::fonts::Fonts,
    theme: &Theme,
    ctl: &CtlBox,
    x: i32,
    top: i32,
    ops: &mut Vec<DrawOp>,
    controls: &mut Vec<ControlRect>,
) {
    // A `visibility:hidden` control paints nothing and takes no clicks — it is
    // not registered, so it can't sit as an invisible target over the page.
    if ctl.style.hidden {
        return;
    }
    let (w, h) = (ctl.w, ctl.h);
    // `opacity:0`: paint nothing, but keep the hit rect — this is the
    // checkbox-hack overlay that a CSS-only dropdown is toggled with.
    if ctl.style.transparent {
        controls.push(ControlRect { x, y: top, w, h, seq: ctl.seq, kind: ctl.kind });
        return;
    }
    let font = fonts.pick(ctl.style.bold, ctl.style.italic, ctl.style.mono);
    // A control's chrome follows the SURFACE IT SITS ON, not the device theme.
    // Wikipedia paints itself light whatever the desktop is set to (its dark
    // mode is opt-in, gated on a class), so a face mixed from a dark theme is a
    // black box on a white page. The signal that is actually to hand is the
    // control's own inherited text colour: light text means a dark surface
    // behind it, and dark text a light one.
    let theme = &surface_palette(theme, ctl.style.color);
    let border = if ctl.focused { theme.link } else { mix(theme.rule, theme.text, 40) };
    let frame = |ops: &mut Vec<DrawOp>| stroke_frame(ops, x, top, w, h, &ctl.border, border, ctl.focused);
    // A page that styles its own button (`background-color`) wins; otherwise
    // the UA face is derived from the theme so it reads on light and dark.
    let face = ctl.bg.unwrap_or(match ctl.kind {
        // Buttons get a raised face; text fields stay flat like the page.
        ControlKind::Submit | ControlKind::Reset | ControlKind::Button | ControlKind::File
        | ControlKind::Select => mix(theme.bg, theme.text, 28),
        _ => mix(theme.bg, theme.text, 8),
    });

    match ctl.kind {
        ControlKind::Checkbox | ControlKind::Radio => {
            ops.push(DrawOp::Rect { x, y: top, w, h, color: face });
            frame(ops);
            if ctl.checked {
                let i = (w / 4).max(2);
                ops.push(DrawOp::Rect {
                    x: x + i,
                    y: top + i,
                    w: w - 2 * i,
                    h: h - 2 * i,
                    color: theme.link,
                });
            }
        }
        _ => {
            ops.push(DrawOp::Rect { x, y: top, w, h, color: face });
            frame(ops);
            let tx = x + ctl.pad_l + 1;
            let lh = ceil_i32(line_gap(font, ctl.style.size));
            let ty = top + (h - lh) / 2;
            if ctl.kind == ControlKind::TextArea {
                // Multi-line: honour hard newlines and wrap on width, top-
                // aligned, clipped to the rows that fit in the box.
                let inner_w = (w - ctl.pad_l - CTL_PAD_X - 2).max(1) as f32;
                let rows = ((h - 2 * CTL_PAD_Y - 2) / lh.max(1)).max(1);
                let mut ly = top + CTL_PAD_Y + 1;
                let color = if ctl.ghost { theme.muted } else { ctl.style.color };
                for line in wrap_lines(font, &ctl.text, ctl.style.size, inner_w, rows as usize) {
                    ops.push(DrawOp::Text {
                        x: tx,
                        y: ly,
                        size: ctl.style.size,
                        color,
                        bold: ctl.style.bold,
                        italic: ctl.style.italic,
                        mono: ctl.style.mono,
                        text: line,
                    });
                    ly += lh;
                }
                controls.push(ControlRect { x, y: top, w, h, seq: ctl.seq, kind: ctl.kind });
                return;
            }
            if !ctl.text.is_empty() {
                // Clip an over-long value to the box (no inner scrolling yet):
                // keep the tail visible, which is where the caret is.
                let inner = (w - ctl.pad_l - CTL_PAD_X - 2).max(0) as f32;
                let text = clip_text_tail(font, &ctl.text, ctl.style.size, inner);
                // A button's label is centred in its box (HTML §button-layout);
                // a field's value is not.
                let tx = match ctl.kind {
                    ControlKind::Submit | ControlKind::Reset | ControlKind::Button
                    | ControlKind::File => {
                        let tw = measure(font, &text, ctl.style.size);
                        tx.max(x + ((w as f32 - tw) / 2.0) as i32)
                    }
                    _ => tx,
                };
                ops.push(DrawOp::Text {
                    x: tx,
                    y: ty,
                    size: ctl.style.size,
                    color: if ctl.ghost { theme.muted } else { ctl.style.color },
                    bold: ctl.style.bold,
                    italic: ctl.style.italic,
                    mono: ctl.style.mono,
                    text,
                });
            }
            if ctl.kind == ControlKind::Select {
                // A downward chevron, drawn as a stack of narrowing bars.
                let cx = x + w - CTL_PAD_X - CTL_ARROW / 2;
                let cy = top + h / 2 - 2;
                for i in 0..4 {
                    ops.push(DrawOp::Rect {
                        x: cx - 4 + i,
                        y: cy + i,
                        w: 9 - 2 * i,
                        h: 1,
                        color: ctl.style.color,
                    });
                }
            }
            if let Some(caret) = ctl.caret {
                let upto: String = ctl.text.chars().take(caret).collect();
                let cw = measure(font, &upto, ctl.style.size);
                let inner = (w - ctl.pad_l - CTL_PAD_X - 2) as f32;
                let cx = tx + cw.min(inner.max(0.0)) as i32;
                ops.push(DrawOp::Rect {
                    x: cx,
                    y: ty + 1,
                    w: 1,
                    h: ceil_i32(line_gap(font, ctl.style.size)) - 2,
                    color: theme.link,
                });
            }
        }
    }
    controls.push(ControlRect { x, y: top, w, h, seq: ctl.seq, kind: ctl.kind });
}

/// Paint a control's frame: each side its own width and colour, `ua` standing
/// in wherever the page named none. A side the page suppressed (`border: none`,
/// `border-color: transparent`) paints nothing at all.
///
/// Focus is the one thing the page cannot take away: a control with no frame
/// left still gets a 1px ring while it has the keyboard, because that ring is
/// an OUTLINE — it says where typing goes, and a page hiding its border never
/// meant to hide that.
fn stroke_frame(ops: &mut Vec<DrawOp>, x: i32, y: i32, w: i32, h: i32, sides: &[CtlSide; 4], ua: Rgb, focused: bool) {
    let visible = |s: &CtlSide| s.w > 0 && !s.transparent;
    if focused && !sides.iter().any(visible) {
        stroke_rect(ops, x, y, w, h, ua);
        return;
    }
    let [t, r, b, l] = *sides;
    // Widths are clamped to the box so a frame thicker than its control still
    // reads as a frame instead of painting past the far edge.
    for (side, rect) in [
        (t, (x, y, w, t.w.min(h))),
        (r, (x + w - r.w.min(w), y, r.w.min(w), h)),
        (b, (x, y + h - b.w.min(h), w, b.w.min(h))),
        (l, (x, y, l.w.min(w), h)),
    ] {
        if visible(&side) {
            let (rx, ry, rw, rh) = rect;
            // Focus recolours the whole frame, author colours included — the
            // control has the keyboard, and that has to be visible on a page
            // that gave its fields a colour of their own.
            let color = if focused { ua } else { side.color.unwrap_or(ua) };
            ops.push(DrawOp::Rect { x: rx, y: ry, w: rw, h: rh, color });
        }
    }
}

fn stroke_rect(ops: &mut Vec<DrawOp>, x: i32, y: i32, w: i32, h: i32, color: Rgb) {
    ops.push(DrawOp::Rect { x, y, w, h: 1, color });
    ops.push(DrawOp::Rect { x, y: y + h - 1, w, h: 1, color });
    ops.push(DrawOp::Rect { x, y, w: 1, h, color });
    ops.push(DrawOp::Rect { x: x + w - 1, y, w: 1, h, color });
}

/// Break `text` into at most `max_rows` lines that fit `max_w`, splitting on
/// hard newlines first and then greedily on words (a `<textarea>`'s content).
fn wrap_lines(font: &Font, text: &str, size: f32, max_w: f32, max_rows: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        if out.len() >= max_rows {
            break;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            let cand = if line.is_empty() {
                word.to_string()
            } else {
                alloc::format!("{line} {word}")
            };
            if measure(font, &cand, size) <= max_w || line.is_empty() {
                line = cand;
            } else {
                out.push(core::mem::take(&mut line));
                if out.len() >= max_rows {
                    return out;
                }
                line = word.to_string();
            }
        }
        out.push(line);
    }
    out.truncate(max_rows);
    out
}

/// Trim `text` from the LEFT until it fits `max_w` (the caret sits at the end
/// of a field the user is typing into, so the tail is what matters).
fn clip_text_tail(font: &Font, text: &str, size: f32, max_w: f32) -> String {
    if measure(font, text, size) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let s: String = chars[start..].iter().collect();
        if measure(font, &s, size) <= max_w {
            return s;
        }
        start += 1;
    }
    String::new()
}

/// Accumulates inline content, then flows it into line boxes. Whitespace
/// collapses per `white-space: normal`: a run of spaces (within a text node or
/// across inline-element boundaries) becomes at most one inter-word space.
struct Inline {
    items: Vec<Item>,
    /// Every inline box opened in this run, in tree order — so painting them
    /// by index paints an ancestor's background under its descendant's.
    boxes: Vec<InlineBox>,
    pending_space: bool,
}

impl Inline {
    fn new() -> Inline {
        Inline { items: Vec::new(), boxes: Vec::new(), pending_space: false }
    }
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add collapsed text from one text node under style `st`.
    fn text(&mut self, raw: &str, st: &ComputedStyle, href: Option<&str>) {
        let rs = RunStyle { hidden: st.hidden, transparent: st.transparent, size: st.font_px, color: st.color, bold: st.bold, italic: st.italic, mono: st.mono, valign: st.valign, deco: st.deco, break_word: st.break_word, nowrap: st.nowrap, lh: st.line_height.px(st.font_px).unwrap_or(0.0) };
        let mut word = String::new();
        for ch in raw.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    let w = transform_word(core::mem::take(&mut word), st.text_transform);
                    self.push_word(w, rs, href);
                }
                if !self.items.is_empty() {
                    self.pending_space = true;
                }
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            let w = transform_word(word, st.text_transform);
            self.push_word(w, rs, href);
        }
    }

    fn push_word(&mut self, text: String, style: RunStyle, href: Option<&str>) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Word { text, style, href: href.map(|s| s.to_string()), space_before });
    }

    /// Add an atomic `<img>` (decoded or a placeholder) to the inline run,
    /// carrying the enclosing link so an image-in-a-link stays clickable.
    #[allow(clippy::too_many_arguments)]
    fn image(&mut self, src: String, w: i32, h: i32, href: Option<&str>, alt: String, hidden: bool, transparent: bool, deco: Option<InlineBox>) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Image { src, w, h, href: href.map(|s| s.to_string()), alt, space_before, hidden, transparent, deco: deco.map(alloc::boxed::Box::new) });
    }

    /// Add a laid-out `inline-block` to the inline run.
    fn atomic(&mut self, box_: AtomicBox) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Atomic { box_: RefCell::new(Some(box_)), space_before });
    }

    /// Add an atomic form control to the inline run.
    fn control(&mut self, ctl: CtlBox) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Control { ctl, space_before });
    }

    /// Open an inline box around the items that follow. Deliberately leaves
    /// `pending_space` alone: a space before `<a>` belongs to the word inside
    /// it, not to the box.
    fn open_box(&mut self, b: InlineBox) -> usize {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        let i = self.boxes.len();
        self.boxes.push(b);
        self.items.push(Item::BoxStart { bx: i, space_before });
        i
    }

    fn close_box(&mut self, i: usize) {
        self.items.push(Item::BoxEnd(i));
    }

    fn brk(&mut self) {
        self.items.push(Item::Break);
        self.pending_space = false;
    }

    fn strut(&mut self, st: &ComputedStyle) {
        self.items.push(Item::Strut(RunStyle {
            hidden: st.hidden,
            transparent: st.transparent,
            size: st.font_px,
            color: st.color,
            bold: st.bold,
            italic: st.italic,
            mono: st.mono,
            valign: st.valign,
            deco: st.deco,
            break_word: st.break_word,
            nowrap: st.nowrap,
            lh: st.line_height.px(st.font_px).unwrap_or(0.0),
        }));
    }

    fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Flow the accumulated items into line boxes starting at `y0`; append the
    /// resulting `DrawOp`s + `LinkRect`s. Returns the y below the last line.
    /// `theme` supplies placeholder colours for undecodable images.
    #[allow(clippy::too_many_arguments)]
    fn flow(
        &self,
        fonts: &crate::fonts::Fonts,
        theme: &Theme,
        x: i32,
        w: i32,
        y0: i32,
        floats: &[FloatRect],
        align: TextAlign,
        align_last: Option<TextAlign>,
        rtl: bool,
        // `text-indent` in px: only the FIRST line box starts in from the
        // content edge — every later one resets the pen to the float band.
        indent: f32,
        strut: f32,
        ops: &mut Vec<DrawOp>,
        links: &mut Vec<LinkRect>,
        controls: &mut Vec<ControlRect>,
        inspects: &mut Vec<InspectBox>,
        last_baseline: &mut Option<i32>,
    ) -> i32 {
        // Each word/segment measures with its own face (a monospace run advances
        // differently from proportional Inter), so glyph positions match what
        // the raster later paints via the same `Fonts::pick`.
        let face = |s: &RunStyle| fonts.pick(s.bold, s.italic, s.mono);
        let mut y = y0;
        let mut line: Vec<Placed> = Vec::new();
        // Each line's usable [left, right] narrows around floats at its y-band.
        // The strut: an empty line box is as tall as the block's own
        // line-height, and that is also the band height float avoidance probes.
        let strut_h = if strut > 0.0 { strut } else { line_gap(fonts.regular(), BASE_FONT_PX) };
        let lh = ceil_i32(strut_h).max(1);
        let (l0, r0) = band_of(floats, y, y + lh, x, x + w);
        let mut pen = l0 as f32 + indent;
        let mut line_ascent = 0.0f32;
        // How far the deepest item on the line reaches BELOW the baseline. Only
        // needed to size a line around a `vertical-align: middle` box; text
        // carries its descent inside its own line-box height already.
        let mut line_below = 0.0f32;
        let mut gap = 0.0f32;
        let mut right = r0 as f32;
        // Inline boxes currently spanning the pen, innermost last, and the
        // fragments they have finished on the line being built.
        let mut open: Vec<OpenFrag> = Vec::new();
        let mut frags: Vec<Frag> = Vec::new();
        // Where the last text run left the pen. Two runs merge into one op only
        // if the second starts exactly where the first ended — an inline box's
        // edge (its space, margin, border, padding) moves the pen in between,
        // and merging across that would draw the second run at the first one's
        // pen and lose the gap.
        let mut run_end = f32::NAN;

        for item in &self.items {
            match item {
                Item::BoxStart { bx, space_before } => {
                    let b = &self.boxes[*bx];
                    if *space_before && !line.is_empty() {
                        // The box's own face, not `regular()`: a box inherits
                        // the font of the run whose space this is, so it is the
                        // closest thing to hand — and a monospace space is much
                        // wider than a proportional one.
                        pen += space_width(fonts.pick(b.st.bold, b.st.italic, b.st.mono), b.st.font_px);
                    }
                    open.push(OpenFrag { bx: *bx, x0: Some((pen + b.margin_left) as i32), left: true });
                    pen += b.lead;
                }
                Item::BoxEnd(i) => {
                    let b = &self.boxes[*i];
                    // The border box ends where the content does plus the right
                    // padding and border; the margin stays outside it.
                    let x1 = (pen + b.trail - b.margin_right) as i32;
                    pen += b.trail;
                    if let Some(k) = open.iter().rposition(|o| o.bx == *i) {
                        let o = open.remove(k);
                        frags.push(Frag { bx: *i, x0: o.x0, x1, left: o.left, right: true });
                    }
                }
                Item::Strut(style) => {
                    let (asc, lb) = run_metrics(face(style), style.size, style.lh);
                    line_ascent = line_ascent.max(asc);
                    gap = gap.max(lb);
                }
                Item::Break => {
                    break_frags(&mut open, &mut frags, pen);
                    if !line_exists(&line, &frags) {
                        frags.clear();
                        y += ceil_i32(strut_h);
                    } else {
                        *last_baseline = Some(y + line_ascent as i32);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects);
                    }
                    let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                    pen = bl as f32;
                    right = br as f32;
                    line_ascent = 0.0;
                    line_below = 0.0;
                    gap = 0.0;
                }
                Item::Word { text, style, href, space_before } => {
                    let ww = measure(face(style), text, style.size);
                    let sw = if *space_before { space_width(face(style), style.size) } else { 0.0 };
                    // `white-space: nowrap`: the space before this word is not a
                    // break opportunity, so the line overflows instead.
                    if !style.nowrap && !line.is_empty() && pen + sw + ww > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects);
                        let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                        pen = bl as f32;
                        right = br as f32;
                        line_ascent = 0.0;
                        line_below = 0.0;
                        gap = 0.0;
                    }
                    let mut lead = if line.is_empty() { 0.0 } else { sw };
                    // `overflow-wrap: break-word` — the word is wider than a
                    // whole line, so wrapping it whole would just overflow the
                    // box. Split it across lines at the last character that
                    // fits. A line that can't take even one character is
                    // already as narrow as it will get (a float band), so force
                    // one character through rather than spin.
                    if style.break_word && pen + lead + ww > right {
                        let f = face(style);
                        let mut rest = text.as_str();
                        while !rest.is_empty() {
                            let mut n = fit_prefix(f, rest, style.size, right - pen - lead);
                            if n == 0 {
                                if !line.is_empty() {
                                    *last_baseline = Some(y + line_ascent as i32);
                                    break_frags(&mut open, &mut frags, pen);
                                    y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects);
                                    let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                                    pen = bl as f32;
                                    right = br as f32;
                                    line_ascent = 0.0;
                                    line_below = 0.0;
                                    gap = 0.0;
                                    lead = 0.0;
                                    continue;
                                }
                                n = first_cluster(rest);
                                if n == 0 {
                                    break;
                                }
                            }
                            let (head, tail) = rest.split_at(n);
                            line.push(Placed::Text(Seg {
                                x: (pen + lead) as i32,
                                text: head.into(),
                                style: *style,
                                href: href.clone(),
                            }));
                            pen += lead + measure(f, head, style.size);
                            run_end = pen;
                            lead = 0.0;
                            let (asc, lb) = run_metrics(f, style.size, style.lh);
                            line_ascent = line_ascent.max(asc);
                            gap = gap.max(lb);
                            rest = tail;
                        }
                        continue;
                    }
                    let sx = (pen + lead) as i32;
                    let merge = pen == run_end
                        && matches!(line.last(), Some(Placed::Text(last)) if last.style == *style && last.href == *href);
                    if merge {
                        if let Some(Placed::Text(last)) = line.last_mut() {
                            if lead > 0.0 {
                                last.text.push(' ');
                            }
                            last.text.push_str(text);
                        }
                    } else {
                        line.push(Placed::Text(Seg { x: sx, text: text.clone(), style: *style, href: href.clone() }));
                    }
                    pen += lead + ww;
                    run_end = pen;
                    let (asc, lb) = run_metrics(face(style), style.size, style.lh);
                    line_ascent = line_ascent.max(asc);
                    gap = gap.max(lb);
                }
                Item::Atomic { box_, space_before } => {
                    let Some(b) = box_.borrow_mut().take() else { continue };
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX) } else { 0.0 };
                    if !line.is_empty() && pen + sw + b.w as f32 > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects);
                        let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                        pen = bl as f32;
                        right = br as f32;
                        line_ascent = 0.0;
                        line_below = 0.0;
                        gap = 0.0;
                    }
                    let lead = if line.is_empty() { 0.0 } else { sw };
                    pen += lead + b.w as f32;
                    // How far this box reaches above and below the baseline
                    // decides how tall the line box has to be. A `middle` box
                    // straddles the baseline, so half of it hangs ABOVE — the
                    // line has to grow for that half or the box paints outside
                    // its own line, which is what pushed MediaWiki's gallery
                    // thumbnails up out of their frames. `top`/`bottom` are
                    // measured against the line box itself and so contribute
                    // only their height.
                    let half_x = MIDDLE_HALF_X;
                    let (above, below) = match b.valign {
                        crate::style::VAlign::Top
                        | crate::style::VAlign::TextTop
                        | crate::style::VAlign::Bottom
                        | crate::style::VAlign::TextBottom => (0.0, 0.0),
                        crate::style::VAlign::Middle => (b.h as f32 / 2.0 + half_x, b.h as f32 / 2.0 - half_x),
                        _ => (b.baseline as f32, (b.h - b.baseline) as f32),
                    };
                    line_ascent = line_ascent.max(above);
                    line_below = line_below.max(below);
                    gap = gap.max(b.h as f32).max(line_ascent + line_below);
                    line.push(Placed::Atomic { x: (pen - b.w as f32) as i32, box_: b });
                }
                Item::Image { src, w: iw, h: ih, href, alt, space_before, hidden, transparent, deco } => {
                    // The frame an `<img>` paints around itself is part of the
                    // space it takes on the line — measure with it, or the
                    // border overlaps whatever comes next.
                    let (fl, fr) = deco.as_ref().map_or((0.0, 0.0), |d| (d.lead, d.trail));
                    // Fit the image to the content width, keeping aspect.
                    let (mut bw, mut bh) = (*iw as f32, *ih as f32);
                    if bw > w as f32 {
                        bh *= w as f32 / bw;
                        bw = w as f32;
                    }
                    let (bw, bh) = (bw.max(1.0) as i32, bh.max(1.0) as i32);
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX) } else { 0.0 };
                    if !line.is_empty() && pen + sw + bw as f32 > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects);
                        let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                        pen = bl as f32;
                        right = br as f32;
                        line_ascent = 0.0;
                        line_below = 0.0;
                        gap = 0.0;
                    }
                    let lead = if line.is_empty() { 0.0 } else { sw };
                    let sx = (pen + lead + fl) as i32;
                    line.push(Placed::Image {
                        x: sx,
                        w: bw,
                        h: bh,
                        src: src.clone(),
                        href: href.clone(),
                        alt: alt.clone(),
                        hidden: *hidden,
                        transparent: *transparent,
                        deco: deco.clone(),
                    });
                    pen += lead + fl + bw as f32 + fr;
                    line_ascent = line_ascent.max(bh as f32);
                    gap = gap.max(bh as f32 + 2.0);
                }
                Item::Control { ctl, space_before } => {
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX) } else { 0.0 };
                    if !line.is_empty() && pen + sw + ctl.w as f32 > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects);
                        let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                        pen = bl as f32;
                        right = br as f32;
                        line_ascent = 0.0;
                        line_below = 0.0;
                        gap = 0.0;
                    }
                    let lead = if line.is_empty() { 0.0 } else { sw };
                    let sx = (pen + lead) as i32;
                    // The control's box sits ON the text baseline like an
                    // inline-block, minus its bottom padding so a field and the
                    // label beside it look aligned.
                    line.push(Placed::Control { x: sx, ctl });
                    pen += lead + ctl.w as f32;
                    line_ascent = line_ascent.max(ctl.h as f32 - CTL_PAD_Y as f32);
                    gap = gap.max(ctl.h as f32 + 2.0);
                }
            }
        }
        break_frags(&mut open, &mut frags, pen);
        if line_exists(&line, &frags) {
            let a = align_last.unwrap_or(align);
            *last_baseline = Some(y + line_ascent as i32);
            y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(a, rtl, pen, right), ops, links, controls, inspects);
        }
        y
    }
}

/// One item placed on the current line: a same-style text run, an image, or a
/// form control (borrowed from the inline run — it is only measured once).
enum Placed<'a> {
    Text(Seg),
    Atomic { x: i32, box_: AtomicBox },
    Image { x: i32, w: i32, h: i32, src: String, href: Option<String>, alt: String, hidden: bool, transparent: bool, deco: Option<alloc::boxed::Box<InlineBox>> },
    Control { x: i32, ctl: &'a CtlBox },
}

/// An inline box spanning the pen: its fragment on the current line is open.
struct OpenFrag {
    bx: usize,
    /// Border-box left edge, or `None` on a line the box only continues onto —
    /// there the fragment starts at whatever content the line starts with.
    x0: Option<i32>,
    /// This fragment carries the box's left border + padding.
    left: bool,
}

/// One inline box's rectangle on one line box. A box that spans three lines
/// leaves three of these, and only the first/last carry its left/right edge
/// (the `box-decoration-break: slice` default).
struct Frag {
    bx: usize,
    x0: Option<i32>,
    x1: i32,
    left: bool,
    right: bool,
}

/// The line is about to be emitted: close every open inline-box fragment at
/// the current pen. The boxes stay open — their next fragment begins on the
/// next line, no longer carrying the left edge and starting wherever that
/// line's content does.
/// Does this line box exist at all? It does if it holds content — or if an
/// inline box on it reserves horizontal space: margins, borders and padding on
/// an inline box keep an otherwise empty line alive (CSS 2.1 §9.4.2), which is
/// how an icon-only `<span>` gets a box to paint its background into.
fn line_exists(line: &[Placed<'_>], frags: &[Frag]) -> bool {
    !line.is_empty() || frags.iter().any(|f| f.x1 > f.x0.unwrap_or(f.x1))
}

fn break_frags(open: &mut [OpenFrag], frags: &mut Vec<Frag>, pen: f32) {
    for o in open.iter_mut() {
        frags.push(Frag { bx: o.bx, x0: o.x0, x1: pen as i32, left: o.left, right: false });
        o.x0 = None;
        o.left = false;
    }
}

/// One same-style segment placed on the current line.
struct Seg {
    x: i32,
    text: String,
    style: RunStyle,
    href: Option<String>,
}

/// `text-transform` applied to one whitespace-delimited word. `capitalize`
/// uppercases the first letter of each word, which is exactly what a word here
/// is — the caller has already split on whitespace.
fn transform_word(w: String, tt: TextTransform) -> String {
    match tt {
        TextTransform::None => w,
        TextTransform::Upper => w.chars().flat_map(|c| c.to_uppercase()).collect(),
        TextTransform::Lower => w.chars().flat_map(|c| c.to_lowercase()).collect(),
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(w.len());
            let mut first = true;
            for c in w.chars() {
                if first && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                    first = false;
                } else {
                    out.push(c);
                }
            }
            out
        }
    }
}

/// The marker string for a counter-style `list-style-type` at ordinal `n`.
fn marker_label(ls: ListStyle, n: i32) -> String {
    match ls {
        ListStyle::Decimal => alloc::format!("{n}."),
        ListStyle::LowerAlpha => alloc::format!("{}.", alpha_counter(n, false)),
        ListStyle::UpperAlpha => alloc::format!("{}.", alpha_counter(n, true)),
        ListStyle::LowerRoman => alloc::format!("{}.", roman_counter(n, false)),
        ListStyle::UpperRoman => alloc::format!("{}.", roman_counter(n, true)),
        _ => String::new(),
    }
}

/// A `counter()`/`counters()` value formatted in `style` — the bare number, no
/// trailing separator (unlike a list `marker_label`, which appends `.`).
/// `disc`/`circle`/`square` render their glyph (as browsers do); `none` is
/// empty. Unknown/`decimal` fall through to plain decimal.
fn format_counter(style: ListStyle, n: i32) -> String {
    match style {
        ListStyle::LowerAlpha => alpha_counter(n, false),
        ListStyle::UpperAlpha => alpha_counter(n, true),
        ListStyle::LowerRoman => roman_counter(n, false),
        ListStyle::UpperRoman => roman_counter(n, true),
        // Pad 1..9 to two digits (`01`); everything else is plain decimal.
        ListStyle::DecimalLeadingZero => alloc::format!("{n:02}"),
        ListStyle::Disc => "•".into(),
        ListStyle::Circle => "◦".into(),
        ListStyle::Square => "▪".into(),
        ListStyle::None => String::new(),
        _ => alloc::format!("{n}"),
    }
}

/// Bijective base-26: 1→a, 26→z, 27→aa. Out-of-range ordinals fall back to the
/// decimal representation, as CSS requires of an exhausted counter style.
fn alpha_counter(n: i32, upper: bool) -> String {
    if n < 1 {
        return alloc::format!("{n}");
    }
    let base = if upper { b'A' } else { b'a' };
    let mut out: Vec<u8> = Vec::new();
    let mut v = n;
    while v > 0 {
        let rem = (v - 1) % 26;
        out.push(base + rem as u8);
        v = (v - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Additive Roman numerals (CSS `lower-roman`/`upper-roman`). Only 1..=3999 is
/// representable; anything else falls back to decimal.
fn roman_counter(n: i32, upper: bool) -> String {
    if !(1..=3999).contains(&n) {
        return alloc::format!("{n}");
    }
    const VALS: [(i32, &str, &str); 13] = [
        (1000, "m", "M"), (900, "cm", "CM"), (500, "d", "D"), (400, "cd", "CD"),
        (100, "c", "C"), (90, "xc", "XC"), (50, "l", "L"), (40, "xl", "XL"),
        (10, "x", "X"), (9, "ix", "IX"), (5, "v", "V"), (4, "iv", "IV"), (1, "i", "I"),
    ];
    let mut v = n;
    let mut out = String::new();
    for (val, lo, up) in VALS {
        while v >= val {
            out.push_str(if upper { up } else { lo });
            v -= val;
        }
    }
    out
}

/// `text-align`'s inline shift for one finished line: `pen` is the x just past
/// the last placed item, `right` the line box's right edge. LTR only, so
/// `start`/`left`/`justify` never shift. A line that overflows its box (a long
/// unbreakable word) has no slack to distribute and stays put.
fn align_dx(align: TextAlign, rtl: bool, pen: f32, right: f32) -> i32 {
    let slack = right - pen;
    if slack <= 0.0 {
        return 0;
    }
    // `start`/`end` are direction-relative; `left`/`right` never are.
    let to_right = match align {
        TextAlign::Right => true,
        TextAlign::Left => false,
        TextAlign::Start | TextAlign::Justify => rtl,
        TextAlign::End => !rtl,
        TextAlign::Center => return (slack / 2.0) as i32,
    };
    if to_right { slack as i32 } else { 0 }
}

/// Emit one completed line at a shared baseline; return the next line's top y.
/// Text runs sit on the baseline (`top + ascent == baseline`); images are
/// bottom-aligned to the baseline. Images in a link get a `LinkRect` too.
#[allow(clippy::too_many_arguments)]
/// Underline / line-through / overline for one text run, in the run's own
/// colour. Positions are metric-free approximations of the font's decoration
/// metrics: below the baseline, at roughly half the x-height, and at the cap
/// top. Emitted BEFORE the glyphs so a thick line never eats a descender.
fn push_decorations(style: &RunStyle, x: i32, w: i32, baseline: i32, ops: &mut Vec<DrawOp>) {
    if w <= 0 {
        return;
    }
    let h = ((style.size / 14.0) as i32).max(1);
    let mut line = |y: i32| ops.push(DrawOp::Rect { x, y, w, h, color: style.color });
    if style.deco & crate::style::DECO_UNDERLINE != 0 {
        line(baseline + (style.size * 0.08) as i32);
    }
    if style.deco & crate::style::DECO_LINE_THROUGH != 0 {
        line(baseline - (style.size * 0.27) as i32);
    }
    if style.deco & crate::style::DECO_OVERLINE != 0 {
        line(baseline - (style.size * 0.78) as i32);
    }
}

/// Where a placed item starts on its line — the left edge of a fragment that
/// only continues onto this line, since the box's own left edge is a line above.
fn placed_x(p: &Placed<'_>) -> i32 {
    match p {
        Placed::Text(s) => s.x,
        Placed::Atomic { x, .. } | Placed::Image { x, .. } | Placed::Control { x, .. } => *x,
    }
}

/// Paint one fragment of an inline box. The rectangle is the box's own content
/// area — its font's ascent + descent, NOT the line box (CSS 2.1 §10.6.1) —
/// grown by its padding and border. Vertical padding therefore spills over the
/// neighbouring lines instead of pushing them apart, which is what CSS asks for.
fn paint_frag(
    fonts: &crate::fonts::Fonts,
    b: &InlineBox,
    x0: i32,
    x1: i32,
    baseline: i32,
    sides: (bool, bool),
    ops: &mut Vec<DrawOp>,
) {
    let st = &b.st;
    if st.hidden || st.transparent {
        return;
    }
    let font = fonts.pick(st.bold, st.italic, st.mono);
    let m = font.horizontal_line_metrics(st.font_px);
    let asc = m.map(|m| m.ascent).unwrap_or(st.font_px);
    let desc = m.map(|m| m.descent.abs()).unwrap_or(0.0);
    let top = baseline - (asc + st.pad_top + st.border_top.width) as i32;
    let h = (asc + desc + st.pad_top + st.pad_bottom + st.border_y()) as i32;
    let w = x1 - x0;
    if w <= 0 || h <= 0 {
        return;
    }
    bg_ops(st, b.bg, b.mask, x0, top, w, h, ops);
    border_ops(st, x0, top, w, h, sides, ops);
}

fn emit_line(
    fonts: &crate::fonts::Fonts,
    theme: &Theme,
    line: &mut Vec<Placed<'_>>,
    frags: &mut Vec<Frag>,
    boxes: &[InlineBox],
    y: i32,
    line_ascent: f32,
    gap: f32,
    dx: i32,
    ops: &mut Vec<DrawOp>,
    links: &mut Vec<LinkRect>,
    controls: &mut Vec<ControlRect>,
    inspects: &mut Vec<InspectBox>,
) -> i32 {
    let line_top = y;
    let baseline = y + line_ascent as i32;
    let box_h = ceil_i32(gap).max(1);
    // Inline-box decoration goes down before anything on the line, so text sits
    // on its own background. Sorted by box index — allocation order is tree
    // order, which puts an ancestor's background under its descendant's.
    if !frags.is_empty() {
        let head = line.iter().map(placed_x).min().unwrap_or(0);
        frags.sort_by_key(|f| f.bx);
        for f in frags.drain(..) {
            paint_frag(fonts, &boxes[f.bx], f.x0.unwrap_or(head) + dx, f.x1 + dx, baseline, (f.left, f.right), ops);
        }
    }
    for placed in line.drain(..) {
        match placed {
            Placed::Text(seg) => {
                let font = fonts.pick(seg.style.bold, seg.style.italic, seg.style.mono);
                let mut top = baseline - ascent_i(font, seg.style.size);
                // vertical-align: raise a superscript, drop a subscript off the
                // shared baseline (the run is already at its reduced sup/sub size).
                match seg.style.valign {
                    crate::style::VAlign::Super => top -= (seg.style.size * 0.42) as i32,
                    crate::style::VAlign::Sub => top += (seg.style.size * 0.18) as i32,
                    _ => {}
                }
                // A hidden run is not a click target either — otherwise a
                // collapsed dropdown leaves invisible links over the article.
                if let (Some(h), false) = (&seg.href, seg.style.hidden) {
                    let sw = measure(font, &seg.text, seg.style.size);
                    links.push(LinkRect { x: seg.x + dx, y: line_top, w: ceil_i32(sw), h: box_h, href: h.clone() });
                }
                if !seg.style.hidden && !seg.style.transparent {
                    if seg.style.deco != 0 {
                        let w = ceil_i32(measure(font, &seg.text, seg.style.size));
                        let run_baseline = top + ascent_i(font, seg.style.size);
                        push_decorations(&seg.style, seg.x + dx, w, run_baseline, ops);
                    }
                    ops.push(DrawOp::Text {
                        x: seg.x + dx,
                        y: top,
                        size: seg.style.size,
                        color: seg.style.color,
                        bold: seg.style.bold,
                        italic: seg.style.italic,
                        mono: seg.style.mono,
                        text: seg.text,
                    });
                }
            }
            Placed::Atomic { x, mut box_ } => {
                // CSS2.1 §10.8.1. `baseline` puts the box's own baseline on the
                // line's — with the approximation `baseline == h` that is its
                // bottom margin edge, which is what a block-ish inline-block
                // does. `top`/`bottom` measure against the LINE BOX instead,
                // and that is the case real pages lean on: without it a row of
                // differently tall `inline-block`s descends like a staircase.
                use crate::style::VAlign;
                let dy = match box_.valign {
                    VAlign::Top | VAlign::TextTop => line_top,
                    VAlign::Bottom | VAlign::TextBottom => line_top + box_h - box_.h,
                    // Approximate: the box's midpoint against the baseline
                    // raised by half an x-height, taken as a fraction of the
                    // line's ascent (the parent's font metrics are not threaded
                    // this far down).
                    VAlign::Middle => baseline - MIDDLE_HALF_X as i32 - box_.h / 2,
                    VAlign::Baseline | VAlign::Sub | VAlign::Super => baseline - box_.baseline,
                };
                let (dx, dy) = (x + dx, dy);
                translate_op_list(&mut box_.ops, dx, dy);
                for lk in &mut box_.links {
                    lk.x += dx;
                    lk.y += dy;
                }
                for c in &mut box_.controls {
                    c.x += dx;
                    c.y += dy;
                }
                for b in &mut box_.inspects {
                    b.x += dx;
                    b.y += dy;
                }
                ops.append(&mut box_.ops);
                links.append(&mut box_.links);
                controls.append(&mut box_.controls);
                inspects.append(&mut box_.inspects);
            }
            Placed::Control { x, ctl } => {
                let top = baseline - (ctl.h - CTL_PAD_Y);
                paint_control(fonts, theme, ctl, x + dx, top, ops, controls);
            }
            Placed::Image { x, w, h, src, href, alt, hidden, transparent, deco } => {
                let x = x + dx;
                let top = baseline - h; // image bottom sits on the baseline
                if let (Some(href), false) = (&href, hidden) {
                    links.push(LinkRect { x, y: top, w, h, href: href.clone() });
                }
                // Emitted whether or not the pixels have arrived — the
                // rasteriser draws the placeholder when the lookup misses.
                if !hidden && !transparent {
                    // The image's own box, under its pixels: a replaced element
                    // is atomic in the flow but still paints a background and a
                    // border (MediaWiki frames every thumbnail this way).
                    if let Some(d) = &deco {
                        let st = &d.st;
                        let (bx, by) = (x - d.lead as i32, top - (st.pad_top + st.border_top.width) as i32);
                        let bw = w + (d.lead + d.trail) as i32;
                        let bh = h + (st.pad_top + st.pad_bottom + st.border_y()) as i32;
                        bg_ops(st, None, None, bx, by, bw, bh, ops);
                        border_ops(st, bx, by, bw, bh, (true, true), ops);
                    }
                    ops.push(DrawOp::Image { x, y: top, w, h, src, alt });
                }
            }
        }
    }
    line_top + box_h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    fn fonts() -> crate::fonts::Fonts {
        crate::fonts::Fonts::new()
    }

    fn lay(html: &str, w: u32) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom, crate::css::Media::new(800.0, false));
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, &FormState::default(), false)
    }

    fn lay_inspect(html: &str, w: u32) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom, crate::css::Media::new(800.0, false));
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, &FormState::default(), true)
    }

    #[test]
    fn inspect_reports_the_box_not_its_containing_block() {
        // The device debugging tool has to agree with the pixels. It used to
        // report the CONTAINING BLOCK's x/width, which coincides with the box
        // only for a plain `width: auto` block — so every report about a
        // centred or max-width container (MediaWiki's `.mw-page-container`)
        // carried the viewport's numbers instead of the box's.
        let l = lay_inspect(
            "<body style=\"margin:0\"><div id=c style=\"max-width:600px;margin:0 auto;background:#f00\">x</div></body>",
            1000,
        );
        let b = l.inspect.iter().find(|b| b.label.starts_with("div#c")).expect("inspect box");
        assert_eq!((b.x, b.w), (200, 600), "centred max-width box");
        // … and it matches what is actually painted.
        let red = rects(&l).into_iter().find(|(.., c)| *c == Rgb(0xff, 0, 0)).unwrap();
        assert_eq!((red.0, red.2), (b.x, b.w));
    }

    #[test]
    fn a_percentage_height_needs_a_definite_containing_block() {
        let inner_h = |outer: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><div style=\"{outer}\">\
                     <div style=\"height:50%;background:#f00\"></div></div></body>"
                ),
                800,
            );
            rects(&l).into_iter().find(|(.., c)| *c == Rgb(0xff, 0, 0)).map(|(_, _, _, h, _)| h).unwrap_or(0)
        };
        // Definite parent → half of its CONTENT height.
        assert_eq!(inner_h("height:200px"), 100);
        // `box-sizing: border-box` — the content box is what a % measures.
        assert_eq!(inner_h("height:220px;padding:10px;box-sizing:border-box"), 100);
        // Indefinite parent → the percentage behaves as `auto` (CSS2.1 §10.5),
        // which for an empty box is zero. Guessing a height here is what
        // truncated pages the two earlier attempts at this.
        assert_eq!(inner_h("background:#eee"), 0);
    }

    #[test]
    fn html_height_100_percent_does_not_truncate_the_page() {
        // The 0.3.13 regression, nailed down: `html { height: 100% }` makes the
        // root box exactly one viewport tall, and the page still has to scroll.
        // `Layout::height` is the painted extent, not the root box's bottom —
        // that fix (0.3.14) is what made general percentage heights safe to add.
        let body: String = (0..60)
            .map(|i| alloc::format!("<p>Absatz {i} mit genug Text fuer mehrere Zeilen.</p>"))
            .collect();
        let l = lay(
            &alloc::format!("<html><head><style>html,body{{height:100%;margin:0}}</style></head><body>{body}</body></html>"),
            800,
        );
        assert!(l.height > 600, "page collapsed to one viewport: {}", l.height);
    }

    #[test]
    fn vertical_align_places_atomic_inlines_against_the_line_box() {
        // Every atomic inline used to sit on the baseline, so a row of
        // `inline-block`s of differing heights descended like a staircase —
        // MediaWiki galleries, icon rows and badges all set `vertical-align`
        // for exactly this.
        let tops = |va: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><div>\
                     <span style=\"display:inline-block;vertical-align:{va};background:#f00;width:60px;height:40px\"></span>\
                     <span style=\"display:inline-block;vertical-align:{va};background:#00f;width:60px;height:140px\"></span>\
                     </div></body>"
                ),
                600,
            );
            let f = |c: Rgb| rects(&l).into_iter().find(|(.., cc)| *cc == c).map(|(_, y, _, h, _)| (y, h)).unwrap();
            (f(Rgb(0xff, 0, 0)), f(Rgb(0, 0, 0xff)))
        };
        let ((ry, _), (by, _)) = tops("top");
        assert_eq!(ry, by, "top-aligned boxes share the line box top");
        let ((ry, rh), (by, bh)) = tops("bottom");
        assert_eq!(ry + rh, by + bh, "bottom-aligned boxes share its bottom");
        // A `middle` box straddles the baseline, so the line has to grow around
        // it — otherwise it paints above its own line (the gallery thumbnails
        // hung out of their frames).
        let ((ry, _), (by, _)) = tops("middle");
        assert!(ry > by, "the short box sits lower: {ry} vs {by}");
        assert!(by >= 0, "the tall box stays inside the line box, got {by}");
    }

    #[test]
    fn floated_siblings_add_up_at_max_content() {
        // Floats sit side by side, so a shrink-to-fit box around a row of them
        // is as wide as their SUM. Taking the widest sized Wikipedia's
        // `float: right` footer <ul> to one icon, and its floated <li> children
        // then stacked vertically instead of sitting in a row.
        let l = lay(
            "<body style=\"margin:0\"><ul style=\"float:right;margin:0;padding:0;list-style:none\">\
             <li style=\"float:left\"><div style=\"width:40px;height:20px;background:#f00\"></div></li>\
             <li style=\"float:left\"><div style=\"width:40px;height:20px;background:#00f\"></div></li>\
             </ul></body>",
            600,
        );
        let red = rects(&l).into_iter().find(|(.., c)| *c == Rgb(0xff, 0, 0)).unwrap();
        let blue = rects(&l).into_iter().find(|(.., c)| *c == Rgb(0, 0, 0xff)).unwrap();
        assert_eq!(red.1, blue.1, "the two floats share a line, not stack");
        assert_eq!(blue.0 - red.0, 40, "and sit directly beside each other");
    }

    #[test]
    fn overflow_hidden_drops_what_falls_outside_the_box() {
        let lines = |css: &str| {
            lay(
                &format!("<body><div style=\"width:200px;height:40px;{css}\">one<br>two<br>three<br>four</div></body>"),
                400,
            )
            .ops
            .iter()
            .filter(|o| matches!(o, DrawOp::Text { .. }))
            .count()
        };
        // Four lines in a 40px box: without clipping all four still paint.
        assert_eq!(lines(""), 4);
        // `hidden` keeps only what fits.
        assert!(lines("overflow:hidden") < 4);
        // `auto`/`scroll` deliberately do not clip — we have no scroll
        // container, so clipping would hide reachable content.
        assert_eq!(lines("overflow:auto"), 4);
        assert_eq!(lines("overflow:hidden auto"), 4);
    }

    #[test]
    fn break_word_splits_an_overlong_word_instead_of_overflowing() {
        let long = "Donaudampfschifffahrtsgesellschaftskapitaenswitwe";
        let run = |css: &str| {
            let l = lay(&format!("<body><div style=\"width:80px;{css}\">{long}</div></body>"), 300);
            let t = texts(&l);
            let widest = t.iter().map(|(x, _, s)| x + (s.len() as i32)).max().unwrap_or(0);
            (t.len(), widest)
        };
        // Without it the word stays one run that overflows its 80px box.
        let (lines, _) = run("");
        assert_eq!(lines, 1);
        // With it the word is split across several lines …
        let (lines, _) = run("overflow-wrap:break-word");
        assert!(lines > 1, "expected a split, got {lines} run(s)");
        // … and the legacy spellings mean the same thing.
        assert!(run("word-wrap:break-word").0 > 1);
        assert!(run("word-break:break-all").0 > 1);
        // Every piece must fit the box — that is the whole point.
        let l = lay(&format!("<body><div style=\"width:80px;overflow-wrap:break-word\">{long}</div></body>"), 300);
        for (x, _, _) in texts(&l) {
            assert!(x < 80 + 8, "piece starts at {x}, outside the 80px box");
        }
        // A grapheme cluster is never split, however narrow the box: an emoji
        // ZWJ sequence must stay whole (css-text-3 §5.1, `line-breaking-014`).
        let emoji = "\u{1F468}\u{200D}\u{1F4BB}\u{1F469}\u{200D}\u{1F467}";
        let l = lay(&format!("<body><div style=\"width:8px;word-break:break-all\">{emoji}</div></body>"), 300);
        for (_, _, t) in texts(&l) {
            assert!(!t.starts_with('\u{200D}') && !t.ends_with('\u{200D}'), "split at a ZWJ: {t:?}");
        }
    }

    #[test]
    fn border_radius_rounds_the_background_and_a_uniform_border() {
        let round = |l: &Layout| {
            l.ops.iter().find_map(|o| match o {
                DrawOp::RoundRect { r, ring, .. } => Some((*r, *ring)),
                _ => None,
            })
        };
        // Shorthand, resolved against the border-box width.
        let l = lay("<body><div style=\"width:100px;height:40px;background:#f00;border-radius:8px\">x</div></body>", 400);
        assert_eq!(round(&l), Some(([8.0; 4], 0.0)));
        // Percentages resolve; four values map to the CSS corner order.
        let l = lay("<body><div style=\"width:100px;height:40px;background:#f00;border-radius:1px 2px 3px 4px\">x</div></body>", 400);
        assert_eq!(round(&l), Some(([1.0, 2.0, 3.0, 4.0], 0.0)));
        // A uniform border becomes one stroked ring …
        let l = lay("<body><div style=\"width:100px;height:40px;border:3px solid #000;border-radius:8px\">x</div></body>", 400);
        assert_eq!(round(&l).map(|(_, ring)| ring), Some(3.0));
        // … a mismatched one falls back to the four square edges.
        let l = lay("<body><div style=\"width:100px;height:40px;border:3px solid #000;border-top-width:9px;border-radius:8px\">x</div></body>", 400);
        assert_eq!(round(&l), None);
        // No radius → the plain (fast) rect op, unchanged.
        let l = lay("<body><div style=\"width:100px;height:40px;background:#f00\">x</div></body>", 400);
        assert_eq!(round(&l), None);
    }

    #[test]
    fn vertical_align_positions_cell_content_in_the_row() {
        // One tall cell sets the row height; the short cell's text moves.
        let y_of = |va: &str| {
            let l = lay(
                &format!(
                    "<body><table><tr>\
                     <td style=\"height:120px\">TALL</td>\
                     <td style=\"vertical-align:{va}\">SHORT</td>\
                     </tr></table></body>"
                ),
                400,
            );
            texts(&l).into_iter().find(|(_, _, t)| t.contains("SHORT")).map(|(_, y, _)| y).unwrap()
        };
        let (top, mid, bot) = (y_of("top"), y_of("middle"), y_of("bottom"));
        assert!(top < mid && mid < bot, "top {top} / middle {mid} / bottom {bot}");
        // The initial value degrades to `top` for us (no cross-cell baselines).
        assert_eq!(y_of("baseline"), top);
    }

    #[test]
    fn a_table_takes_its_specified_width_and_height() {
        let red = Rgb(0xff, 0, 0);
        let box_of = |css: &str| {
            let l = lay(
                &format!("<body style=\"margin:0\"><table style=\"border-spacing:0;background:#f00;{css}\">\
                          <tr><td></td></tr></table></body>"),
                400,
            );
            rects(&l).into_iter().find(|(.., c)| *c == red).map(|(_, _, w, h, _)| (w, h)).unwrap()
        };
        // Empty cells used to collapse the table onto its border: the columns
        // measure zero, so nothing ever claimed the specified width.
        assert_eq!(box_of("width:100px;height:60px"), (100, 60));
        // `height` is a MINIMUM (CSS2.1 §17.5.3) — taller content wins.
        let (_, h) = box_of("width:100px;height:1px");
        assert!(h > 1, "content keeps its height, got {h}");
        // `min-height` does the same job.
        assert_eq!(box_of("width:100px;min-height:60px"), (100, 60));
        // `box-sizing: border-box` counts the border in, not on top.
        assert_eq!(
            box_of("width:100px;height:60px;border:10px solid #00f;box-sizing:border-box"),
            (100, 60)
        );
    }

    #[test]
    fn a_shrink_to_fit_box_wraps_its_content_margin_box() {
        let blue = Rgb(0, 0, 0xff);
        // An out-of-flow box with `width: auto` shrink-wraps. Its own frame was
        // subtracted twice (once here, once by the block path that reads the
        // handed-over width as a containing block), and a child's margins never
        // reached the measurement at all.
        let outer = |inner: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><div style=\"border:10px solid #00f;position:absolute;top:0\">\
                     <div style=\"{inner}\"></div></div></body>"
                ),
                800,
            );
            rects(&l).into_iter().find(|(.., c)| *c == blue).map(|(_, _, w, _, _)| w).unwrap()
        };
        // 200 content + 20 child border + 20 own border.
        assert_eq!(outer("border:10px solid #f00;width:200px;height:60px"), 240);
        // … + 50px margins on each side.
        assert_eq!(outer("border:10px solid #f00;width:200px;height:60px;margin:0 50px"), 340);
    }

    /// Inline-blocks sit side by side ON a line, so a shrink-to-fit container
    /// has to be wide enough for their SUM. Measuring them as block-level
    /// children takes the widest instead, and they then have no room beside
    /// each other and stack — which is how Google's `float:right` header bar
    /// came out one word wide with "Gmail" and "Bilder" on separate lines.
    #[test]
    fn a_shrink_to_fit_box_fits_its_inline_blocks_side_by_side() {
        let blue = Rgb(0, 0, 0xff);
        let bar = |float: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><div style=\"{float}background:#00f\">\
                     <div style=\"display:inline-block;width:60px;height:20px\"></div>\
                     <div style=\"display:inline-block;width:60px;height:20px\"></div>\
                     </div></body>"
                ),
                800,
            );
            rects(&l).into_iter().find(|(.., c)| *c == blue).map(|(_, _, w, h, _)| (w, h)).unwrap()
        };
        // Two 60px children beside each other: at least 120 wide, one row tall.
        let (w, h) = bar("float:right;");
        assert!(w >= 120, "float:right shrink-to-fit was {w}px, needs >= 120");
        assert!(h < 40, "they stacked: {h}px tall");
        let (w, h) = bar("float:left;");
        assert!(w >= 120, "float:left shrink-to-fit was {w}px, needs >= 120");
        assert!(h < 40, "they stacked: {h}px tall");
    }

    /// `<td width="25%">` is a presentational hint, not CSS — it carries no
    /// unit, so the CSS length parser rejects it. Table-built pages centre
    /// with exactly this (Google's home page puts the search box between two
    /// 25% spacer cells); ignoring it collapses the spacer and slams the
    /// content to the left edge.
    #[test]
    fn a_width_attribute_on_a_cell_is_a_presentational_hint() {
        let red = Rgb(0xff, 0, 0);
        let left_edge = |spacer: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><table cellpadding=\"0\" cellspacing=\"0\">\
                     <tr><td {spacer}></td>\
                     <td><div style=\"background:#f00;width:40px;height:20px\"></div></td></tr>\
                     </table></body>"
                ),
                800,
            );
            rects(&l).into_iter().find(|(.., c)| *c == red).map(|(x, ..)| x).unwrap()
        };
        assert_eq!(left_edge("width=\"200\""), 200, "a bare number is pixels");
        assert_eq!(left_edge("width=\"25%\""), 200, "25% of an 800px table");
        // Author CSS still wins over the hint.
        assert_eq!(left_edge("width=\"200\" style=\"width:100px\""), 100);
    }

    /// The other half of table-built centring: `<td width="25%">` spacers put
    /// the CELL in the middle, `align="center"` puts the content in the middle
    /// of the cell. With only the first, Google's search box sat at the left
    /// edge of a correctly-placed cell.
    #[test]
    fn an_align_attribute_is_a_presentational_hint_for_text_align() {
        let red = Rgb(0xff, 0, 0);
        let left_edge = |align: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><table cellpadding=\"0\" cellspacing=\"0\">\
                     <tr><td width=\"400\" {align}>\
                     <div style=\"background:#f00;width:40px;height:20px;display:inline-block\"></div>\
                     </td></tr></table></body>"
                ),
                800,
            );
            rects(&l).into_iter().find(|(.., c)| *c == red).map(|(x, ..)| x).unwrap()
        };
        assert_eq!(left_edge(""), 0, "no hint: content starts at the cell's edge");
        assert_eq!(left_edge("align=\"center\""), 180, "(400 - 40) / 2");
        assert_eq!(left_edge("align=\"right\""), 360, "400 - 40");
        // Author CSS still wins over the hint.
        assert_eq!(left_edge("align=\"center\" style=\"text-align:left\""), 0);
    }

    /// A table wider than its content hands the slack to the columns that did
    /// NOT ask for a width. Spreading it over all of them widened the sized
    /// ones past what they asked for: `25% | auto | 25%` came out 41/18/41,
    /// so the middle column — the one with the content in it — ended up the
    /// narrowest of the three.
    #[test]
    fn table_slack_goes_to_the_columns_without_a_width() {
        let green = Rgb(0, 0x80, 0);
        let middle = |mid: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><table cellpadding=\"0\" cellspacing=\"0\" style=\"width:800px\">\
                     <tr><td width=\"25%\">&nbsp;</td>\
                     <td {mid} style=\"background:#008000\">x</td>\
                     <td width=\"25%\">&nbsp;</td></tr></table></body>"
                ),
                // Deliberately WIDER than the table: a cell percentage is a
                // fraction of the table, not of the space it was offered.
                900,
            );
            rects(&l).into_iter().find(|(.., c)| *c == green).map(|(x, _, w, ..)| (x, w)).unwrap()
        };
        // The auto column takes ALL of it: 800 - 200 - 200 = 400.
        assert_eq!(middle(""), (200, 400));
        // Spelling the same thing out explicitly must agree.
        assert_eq!(middle("width=\"50%\""), (200, 400));
    }

    /// `<center>` centres block-level children, not only inline content —
    /// browsers spell it `text-align: -moz-center`, and the `<center><table>`
    /// idiom is built on it. Plain CSS `text-align: center` must NOT do this,
    /// or every centred paragraph would drag its block children along.
    #[test]
    fn center_centres_a_table_but_plain_text_align_does_not() {
        let green = Rgb(0, 0x80, 0);
        let table_x = |wrap_open: &str, wrap_close: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\">{wrap_open}\
                     <table cellpadding=\"0\" cellspacing=\"0\" style=\"background:#008000\">\
                     <tr><td style=\"width:100px;height:20px\"></td></tr></table>\
                     {wrap_close}</body>"
                ),
                500,
            );
            rects(&l).into_iter().find(|(.., c)| *c == green).map(|(x, ..)| x).unwrap()
        };
        assert_eq!(table_x("<center>", "</center>"), 200, "(500 - 100) / 2");
        assert_eq!(table_x("<div align=\"center\">", "</div>"), 200, "the other spelling");
        assert_eq!(
            table_x("<div style=\"text-align:center\">", "</div>"),
            0,
            "plain CSS centring moves inline content only"
        );
    }

    #[test]
    fn a_max_content_width_is_rounded_up_not_truncated() {
        // A max-content width is the width at which the content does NOT wrap.
        // Truncating a fractional one to whole pixels loses the last word —
        // and the shrink-to-fit paths disagreed about it: a float ceiled, a
        // flex item truncated, so the same text wrapped in one and not in the
        // other.
        let lines = |display: &str| {
            let l = lay(
                &format!(
                    "<body style=\"margin:0\"><div style=\"display:{display}\">\
                     <div id=t>Wrapping here would be one word too early</div></div></body>"
                ),
                600,
            );
            texts(&l).len()
        };
        assert_eq!(lines("flex"), lines("block"), "flex item wraps where a block does not");
        assert_eq!(lines("flex"), 1, "the text fits on one line either way");
    }

    #[test]
    fn table_rows_and_row_groups_are_boxes_of_their_own() {
        let red = Rgb(0xff, 0, 0);
        let table = |css: &str| {
            lay(
                &format!(
                    "<body><style>table{{border-spacing:0}}td{{padding:0;width:50px;height:20px}}{css}</style>\
                     <table><tbody><tr id=a><td>A</td><td>B</td></tr>\
                     <tr id=b><td>C</td><td>D</td></tr></tbody></table></body>"
                ),
                400,
            )
        };
        let reds = |l: &Layout| rects(l).into_iter().filter(|(.., c)| *c == red).collect::<Vec<_>>();
        // A row background spans every column, not just one cell.
        let l = table("#a{background:#f00}");
        let (_, ry, rw, rh, _) = reds(&l)[0];
        assert_eq!((rw, rh), (100, 20), "row box spans both columns");
        // … and it sits BEHIND its cells: the text is emitted after the fill.
        let text_a = texts(&l).into_iter().find(|(.., t)| *t == "A").unwrap();
        assert_eq!(text_a.1 >= ry, true, "row background covers its cell text");
        assert!(
            l.ops.iter().position(|o| matches!(o, DrawOp::Rect { color, .. } if *color == red))
                < l.ops.iter().position(|o| matches!(o, DrawOp::Text { text, .. } if text == "A")),
            "row background must be painted before the cell content"
        );
        // `position:relative` moves the whole row — background and cells.
        let base = table("#b{background:#f00}");
        let moved = table("#b{background:#f00;position:relative;top:7px;left:3px}");
        let (bx, by, ..) = reds(&base)[0];
        let (mx, my, ..) = reds(&moved)[0];
        assert_eq!((mx - bx, my - by), (3, 7), "row box moved");
        let ty = |l: &Layout| texts(l).into_iter().find(|(.., t)| *t == "C").map(|(x, y, _)| (x, y)).unwrap();
        let (bcx, bcy) = ty(&base);
        let (mcx, mcy) = ty(&moved);
        assert_eq!((mcx - bcx, mcy - bcy), (3, 7), "cell content moved with its row");
        // A row group is a box too, spanning all of its rows.
        let l = table("tbody{background:#f00}");
        let (.., gw, gh, _) = reds(&l)[0];
        assert_eq!((gw, gh), (100, 40), "row group box spans both rows");
        // Rows have sibling context, so `:nth-child` can stripe them.
        let l = table("tr:nth-child(2){background:#f00}");
        let stripes = reds(&l);
        assert_eq!(stripes.len(), 1, "exactly one row is striped");
        assert_eq!(stripes[0].1, by, "the SECOND row (where #b sits), not the first");
    }

    #[test]
    fn a_float_paints_above_later_block_borders() {
        // MediaWiki's shape: a right-floated infobox, then headings whose
        // `border-bottom` rule runs the full content width. The rule is a later
        // in-flow block box, so it must paint UNDER the float (CSS2.1 Appendix
        // E: in-flow blocks, then floats) — otherwise it is drawn straight
        // across the table.
        let l = lay(
            "<body><style>\
             .box{float:right;width:100px;height:200px;background:#00f}\
             h2{border-bottom:1px solid #f00;margin:0}\
             </style><div class=box></div><h2>A</h2><h2>B</h2></body>",
            400,
        );
        let idx = |c: Rgb| l.ops.iter().position(|o| matches!(o, DrawOp::Rect { color, .. } if *color == c));
        let float_at = idx(Rgb(0, 0, 0xff)).expect("float background");
        let rule_at = idx(Rgb(0xff, 0, 0)).expect("heading rule");
        assert!(float_at > rule_at, "float must paint after the heading rules ({float_at} vs {rule_at})");
        // A raised z-index still wins over the float layer …
        let over = |z: &str| {
            let l = lay(
                &format!(
                    "<body><style>\
                     .box{{float:right;width:100px;height:200px;background:#00f}}\
                     .over{{position:relative;z-index:{z};width:50px;height:50px;background:#0f0}}\
                     </style><div class=box></div><div class=over></div></body>"
                ),
                400,
            );
            let idx = |c: Rgb| l.ops.iter().position(|o| matches!(o, DrawOp::Rect { color, .. } if *color == c));
            idx(Rgb(0, 0xff, 0)).unwrap() > idx(Rgb(0, 0, 0xff)).unwrap()
        };
        assert!(over("1"), "z-index:1 paints above a float");
        // … and a negative one still loses to it.
        assert!(!over("-1"), "z-index:-1 paints below a float");
        // The float layer has to work INSIDE a tracked z-index range too:
        // MediaWiki wraps a whole article in one positioned container, which is
        // why the first attempt (float ranges only at depth 0) fixed nothing on
        // the real page. The enclosing range gets cut around the float instead.
        let l = lay(
            "<body><style>\
             .wrap{position:relative;z-index:0}\
             .box{float:right;width:100px;height:200px;background:#00f}\
             h2{border-bottom:1px solid #f00;margin:0}\
             </style><div class=wrap><div class=box></div><h2>A</h2></div></body>",
            400,
        );
        let idx = |c: Rgb| l.ops.iter().position(|o| matches!(o, DrawOp::Rect { color, .. } if *color == c));
        assert!(
            idx(Rgb(0, 0, 0xff)) > idx(Rgb(0xff, 0, 0)),
            "a float inside a positioned wrapper still paints above the rules in it"
        );
    }

#[test]
fn dbg_wiki_shape() {
    extern crate std;
    // Wikipedia's shape: floated TABLE (not a div), then a heading with a rule.
    let l = lay(
        "<body><style>\
         table.infobox{float:right;width:100px;background:#00f;border-collapse:collapse}\
         td{height:100px;padding:0}\
         .mw-heading{border-bottom:1px solid #f00;margin:0}\
         </style><table class=infobox><tr><td>X</td></tr></table>\
         <div class=mw-heading><h2>Head</h2></div><p>text</p></body>",
        400,
    );
    for o in l.ops.iter() {
        match o {
            DrawOp::Rect { x, y, w, h, color } => std::println!("RECT {x} {y} {w} {h} {:?}", (color.0, color.1, color.2)),
            DrawOp::Text { x, y, text, .. } => std::println!("TEXT {x} {y} {text}"),
            _ => {}
        }
    }
}

    #[test]
    fn cells_cascade_from_their_row() {
        let red = Rgb(0xff, 0, 0);
        let table = |css: &str| {
            lay(
                &format!(
                    "<body><style>table{{border-spacing:0}}td{{padding:0;width:50px;height:20px}}{css}</style>\
                     <table><tbody><tr><td>A</td><td>B</td></tr></tbody></table></body>"
                ),
                400,
            )
        };
        let reds = |l: &Layout| rects(l).into_iter().filter(|(.., c)| *c == red).collect::<Vec<_>>();
        // The row and its group are on the ancestor path, so a descendant
        // selector through them matches at all.
        assert_eq!(reds(&table("tbody tr td{background:#f00}")).len(), 2, "`tbody tr td` must match");
        // A cell knows its position among its siblings.
        let first = reds(&table("td:first-child{background:#f00}"));
        assert_eq!(first.len(), 1, "only the first cell");
        assert_eq!(first[0].2, 50, "and it is a cell box, not the row");
        assert_eq!(reds(&table("td:last-child{background:#f00}"))[0].0, first[0].0 + 50, "last child is the other one");
        // Inherited properties reach the cell through the row, not from the
        // table two levels up.
        let colour_of = |css: &str| {
            lay(
                &format!("<body><style>{css}</style><table><tbody><tr><td>A</td></tr></tbody></table></body>"),
                400,
            )
            .ops
            .iter()
            .find_map(|o| match o {
                DrawOp::Text { text, color, .. } if text == "A" => Some(*color),
                _ => None,
            })
            .unwrap()
        };
        assert_eq!(colour_of("tr{color:#ff0000}"), red, "a cell inherits from its row");
        assert_eq!(colour_of("tbody{color:#ff0000}"), red, "… and through the row group");
    }

    #[test]
    fn caption_side_moves_the_caption_below_the_grid() {
        let cap_vs_cell = |css: &str| {
            let l = lay(
                &format!("<body><style>{css}</style><table><caption>CAP</caption><tr><td>CELL</td></tr></table></body>"),
                400,
            );
            let find = |needle: &str| {
                texts(&l).into_iter().find(|(_, _, t)| t.contains(needle)).map(|(_, y, _)| y).unwrap()
            };
            (find("CAP"), find("CELL"))
        };
        let (cap, cell) = cap_vs_cell("");
        assert!(cap < cell, "default caption-side:top — {cap} !< {cell}");
        // Inherited, so setting it on the table reaches the caption.
        let (cap, cell) = cap_vs_cell("table{caption-side:bottom}");
        assert!(cap > cell, "caption-side:bottom on the table — {cap} !> {cell}");
        let (cap, cell) = cap_vs_cell("caption{caption-side:bottom}");
        assert!(cap > cell, "caption-side:bottom on the caption — {cap} !> {cell}");
    }

    #[test]
    fn links_are_underlined_unless_the_author_says_otherwise() {
        let rects = |l: &Layout| l.ops.iter().filter(|o| matches!(o, DrawOp::Rect { .. })).count();
        // A real link gets a decoration rect …
        assert_eq!(rects(&lay("<body><a href=\"/x\">hi</a></body>", 400)), 1);
        // … a bare named anchor is not a link, so it does not.
        assert_eq!(rects(&lay("<body><a name=\"x\">hi</a></body>", 400)), 0);
        // … and author CSS can take it away.
        let off = lay("<body><style>a{text-decoration:none}</style><a href=\"/x\">hi</a></body>", 400);
        assert_eq!(rects(&off), 0);
        // `line-through` sits above the baseline, `underline` below it.
        let strike = lay("<body><span style=\"text-decoration:line-through\">hi</span></body>", 400);
        let under = lay("<body><span style=\"text-decoration:underline\">hi</span></body>", 400);
        let y = |l: &Layout| {
            l.ops
                .iter()
                .find_map(|o| match o {
                    DrawOp::Rect { y, .. } => Some(*y),
                    _ => None,
                })
                .unwrap()
        };
        assert!(y(&strike) < y(&under), "{} !< {}", y(&strike), y(&under));
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

    fn rects(l: &Layout) -> Vec<(i32, i32, i32, i32, Rgb)> {
        l.ops
            .iter()
            .filter_map(|o| match o {
                DrawOp::Rect { x, y, w, h, color } => Some((*x, *y, *w, *h, *color)),
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

    /// `attr()` in generated content reads the originating element's
    /// attribute. A missing one is the EMPTY STRING, not a dropped declaration
    /// — the box is still generated, so the brackets around it still show.
    #[test]
    fn generated_content_reads_an_attribute() {
        let text = |css: &str, markup: &str| {
            let l = lay(&alloc::format!("<body><style>p::before{{content:{css}}}</style>{markup}</body>"), 800);
            texts(&l).iter().map(|(_, _, t)| (*t).to_string()).collect::<Vec<_>>().join("|")
        };
        assert!(text("attr(data-x)", "<p data-x=\"HELLO\">y</p>").contains("HELLO"));
        assert!(text("'[' attr(data-gone) ']'", "<p>y</p>").contains("[]"), "absent → empty string");
        // The attribute NAME is case-insensitive (the parser lowercases it);
        // the VALUE keeps its case.
        assert!(text("attr(DATA-X)", "<p data-x=\"MiXeD\">y</p>").contains("MiXeD"));
        // A type/fallback argument is css-values-5 — out of scope, so the whole
        // declaration is dropped rather than half-applied.
        assert!(!text("attr(data-x px)", "<p data-x=\"5\">y</p>").contains('5'));
    }

    /// `text-indent` moves the FIRST line box only — every later line starts at
    /// the content edge again.
    #[test]
    fn text_indent_moves_only_the_first_line() {
        let words = "lorem ipsum dolor sit amet ".repeat(20);
        let l = lay(&alloc::format!("<body><p style=\"text-indent:60px\">{words}</p></body>"), 300);
        let xs: Vec<i32> = texts(&l).iter().map(|(x, _, _)| *x).collect();
        let plain = lay(&alloc::format!("<body><p>{words}</p></body>"), 300);
        let base = texts(&plain)[0].0;
        assert_eq!(xs[0], base + 60, "the first line is indented");
        assert!(xs[1..].iter().all(|&x| x == base), "the rest is not: {xs:?}");
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
    fn sup_is_smaller_and_raised_above_sub() {
        let l = lay("<body><p>base<sup>up</sup><sub>dn</sub></p></body>", 2000);
        let find = |t: &str| {
            l.ops.iter().find_map(|o| match o {
                DrawOp::Text { y, size, text, .. } if text == t => Some((*y, *size)),
                _ => None,
            })
        };
        let (up_y, up_sz) = find("up").expect("sup run");
        let (dn_y, dn_sz) = find("dn").expect("sub run");
        assert!(up_sz < BASE_FONT_PX && dn_sz < BASE_FONT_PX, "sup/sub render smaller");
        assert!(up_y < dn_y, "superscript sits above subscript");
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
    fn position_relative_shifts_paint_but_keeps_flow() {
        // The relative <p> is nudged right+down; the following <p> keeps the
        // normal-flow position (relative reserves its original space).
        let base = lay("<body><p>a</p><p>b</p></body>", 800);
        let rel = lay("<body><p style=\"position:relative; left:40px; top:15px\">a</p><p>b</p></body>", 800);
        let ax = |l: &Layout| l.ops.iter().find_map(|o| match o { DrawOp::Text { x, text, .. } if text == "a" => Some(*x), _ => None }).unwrap();
        let by = |l: &Layout| l.ops.iter().find_map(|o| match o { DrawOp::Text { y, text, .. } if text == "b" => Some(*y), _ => None }).unwrap();
        assert_eq!(ax(&rel), ax(&base) + 40, "relative shifts x by left");
        assert_eq!(by(&rel), by(&base), "following block keeps its flow position");
    }

    #[test]
    fn position_absolute_uses_containing_block_and_leaves_flow() {
        // The absolute badge is positioned at cb.left+left / cb.top+top; the
        // sibling <p> flows as if the badge weren't there.
        let l = lay(
            "<body><div style=\"position:relative\">\
             <span style=\"position:absolute; left:30px; top:8px\">badge</span>\
             <p>flow</p></div></body>",
            800,
        );
        let badge = l.ops.iter().find_map(|o| match o { DrawOp::Text { x, y, text, .. } if text == "badge" => Some((*x, *y)), _ => None }).unwrap();
        let flow_y = |ll: &Layout| ll.ops.iter().find_map(|o| match o { DrawOp::Text { y, text, .. } if text == "flow" => Some(*y), _ => None }).unwrap();
        // cb = the relative div's content box, which starts at the body's UA
        // margin (8px) — the page has no gutter of its own.
        assert_eq!(badge.0, 8 + 30, "abs left = cb.left + left");
        assert!(badge.1 >= 8 + 8 && badge.1 <= 8 + 8 + 6, "abs top ≈ cb.top + top");
        // out of flow: the sibling <p> lands where it would with no badge at all.
        let without = lay("<body><div style=\"position:relative\"><p>flow</p></div></body>", 800);
        assert_eq!(flow_y(&l), flow_y(&without), "absolute badge does not shift the following text");
    }

    #[test]
    fn max_width_container_centers_and_pads() {
        // `.container { max-width:400px; margin:0 auto; padding:20px }` on an
        // 800px viewport (body content width 784): the box is capped to 400 and
        // centered → left margin (784-400)/2 = 192, +body margin(8) +pad_left(20)
        // → x≈220.
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
    fn the_page_is_as_tall_as_what_it_paints() {
        // `Layout::height` is the scrollable extent, and the shell scrolls by
        // it — so a root box SHORTER than its content must not shorten the
        // page. `html { height: 100% }` is an everyday idiom; taking the root's
        // border-box bottom for the page height truncated such a page to one
        // viewport and stopped scrolling outright (0.3.13 → fixed in 0.3.14).
        let long: String = (0..60).map(|i| alloc::format!("<p>Zeile {i} mit etwas Text</p>")).collect();
        let plain = lay(&alloc::format!("<body>{long}</body>"), 800).height;
        assert!(plain > 2000, "60 paragraphs are a long page, got {plain}");
        for css in ["html,body{height:100%}", "html{height:100%}", "body{height:50%}"] {
            let h = lay(&alloc::format!("<html><head><style>{css}</style></head><body>{long}</body></html>"), 800).height;
            assert!(h > 2000, "{css}: page must stay scrollable, got {h}");
        }
    }

    /// Flexbox §9.4 step 7: a line is as tall as its items' HYPOTHETICAL cross
    /// sizes — the natural size clamped by the item's own `min-`/`max-height`.
    /// Sizing the line from the raw natural height left it short of any item
    /// held open by a `min-height`, and that item then hung out below the
    /// container. This is Wikipedia's search bar: a `min-height: 32px` button
    /// beside a shorter field, and the button's border sat 2px past the group's.
    #[test]
    fn a_flex_line_is_as_tall_as_its_tallest_items_minimum() {
        let l = lay(
            "<body><div style=\"display:flex;background:#eee\">\
             <div style=\"background:#0f0\">kurz</div>\
             <div style=\"min-height:60px;background:#00f\">hoch</div>\
             </div></body>",
            800,
        );
        let r = rects(&l);
        let find = |c: Rgb| *r.iter().find(|(_, _, _, _, k)| *k == c).unwrap_or_else(|| panic!("no {c:?} rect"));
        let green = find(Rgb(0, 255, 0));
        let blue = find(Rgb(0, 0, 255));
        let container = find(Rgb(238, 238, 238));
        assert_eq!(blue.3, 60, "the min-height item keeps its minimum");
        assert_eq!(green.3, 60, "align-items:stretch pulls its neighbour to the same line");
        assert!(
            container.1 + container.3 >= blue.1 + blue.3,
            "container bottom {} must not sit above the item's {}",
            container.1 + container.3,
            blue.1 + blue.3
        );
    }

    /// A bordered flex item stretched to the line must end up exactly as tall
    /// as the line — not `border_y()` taller. `flex_item_style` handed the
    /// stretched BORDER-box size back as a content height with only the padding
    /// removed, which is the same box-model twin bucket item 31 removed from
    /// two other places.
    #[test]
    fn stretching_a_bordered_flex_item_does_not_add_its_border() {
        let l = lay(
            "<body><div style=\"display:flex\">\
             <div style=\"height:50px;background:#0f0\">a</div>\
             <div style=\"border:5px solid #000;background:#00f\">b</div>\
             </div></body>",
            800,
        );
        let r = rects(&l);
        let blue = r.iter().find(|(_, _, _, _, c)| *c == Rgb(0, 0, 255)).expect("bordered item");
        assert_eq!(blue.3, 50, "border box matches the 50px line, borders included");
    }

    /// An OUTER `box-shadow` is cut out of its own border box (CSS Backgrounds 3
    /// §7.1.1), so `0 1px <color>` leaves exactly a 1px strip below the box.
    /// Real pages use that as a hairline separator far more often than as a drop
    /// shadow — MediaWiki rules off its article tabs with it, and painting the
    /// shadow as an unclipped copy floods the whole row instead.
    #[test]
    fn a_zero_blur_box_shadow_is_a_hairline_outside_the_box() {
        let shadow_rects = |css: &str| -> Vec<(i32, i32, i32, i32, Rgb)> {
            let l = lay(&alloc::format!("<body><div style=\"{css}\">x</div></body>"), 400);
            rects(&l).into_iter().filter(|(_, _, _, _, c)| *c == Rgb(1, 2, 3)).collect()
        };
        // A rule under the box: one strip, its height the y-offset, and it sits
        // BELOW the border box rather than over it.
        let r = shadow_rects("height:20px;box-shadow:0 1px rgb(1,2,3)");
        assert_eq!(r.len(), 1, "one strip, got {r:?}");
        assert_eq!(r[0].3, 1, "1px tall");
        assert_eq!(r[0].1, 8 + 20, "directly under the 20px box");
        // A spread with no offset rings the box on all four sides.
        assert_eq!(shadow_rects("height:20px;box-shadow:0 0 0 3px rgb(1,2,3)").len(), 4);
        // Fully covered by its own box → nothing to paint.
        assert!(shadow_rects("height:20px;box-shadow:0 0 rgb(1,2,3)").is_empty());
        // A BLURRED shadow is skipped rather than drawn as a hard slab, and an
        // inner one is a different paint entirely.
        assert!(shadow_rects("height:20px;box-shadow:0 2px 8px rgb(1,2,3)").is_empty());
        assert!(shadow_rects("height:20px;box-shadow:inset 0 1px rgb(1,2,3)").is_empty());
        // `currentColor` is the LAST colour, not whatever was cascaded when the
        // shadow was parsed — same rule the border sides follow.
        let l = lay("<body><div style=\"height:20px;box-shadow:0 1px;color:rgb(1,2,3)\">x</div></body>", 400);
        assert!(rects(&l).iter().any(|(_, _, _, h, c)| *h == 1 && *c == Rgb(1, 2, 3)));
    }

    /// A control the page made block-level is a BLOCK box, not an atomic inline.
    /// An atomic inline sits on the baseline, so its parent came out the
    /// control's height plus the descender — which drew a second rule 2px under
    /// a search field whose wrapper is pulled onto the group border with
    /// `margin: -1px`. It must still paint as a CONTROL (face, value,
    /// placeholder), not as an ordinary block with a CSS border.
    #[test]
    fn a_block_level_control_is_a_block_box_and_still_paints_as_a_control() {
        let l = lay(
            "<body><div style=\"background:#0f0;width:300px\">\
             <input style=\"display:block;width:100%;box-sizing:border-box;height:32px\" \
             placeholder=\"such\"></div></body>",
            400,
        );
        let r = rects(&l);
        let parent = r.iter().find(|(_, _, _, _, c)| *c == Rgb(0, 255, 0)).expect("parent");
        assert_eq!(parent.3, 32, "no line-box descender under the control");
        assert!(
            l.ops.iter().any(|o| matches!(o, DrawOp::Text { text, .. } if text == "such")),
            "the placeholder must still be painted"
        );
        // The inline default keeps its line box — this changes only what the
        // page explicitly asked to be block-level.
        let inl = lay(
            "<body><div style=\"background:#0f0;width:300px\">\
             <input style=\"height:32px;box-sizing:border-box\"></div></body>",
            400,
        );
        let p2 = *rects(&inl).iter().find(|(_, _, _, _, c)| *c == Rgb(0, 255, 0)).expect("parent");
        assert!(p2.3 > 32, "an inline control still sits on a baseline, got {}", p2.3);
    }

    /// `transform: translate(...)` shifts the paint, and its percentages are of
    /// the BOX — that is what makes `translate(-50%,-50%)` centre. Together with
    /// `top: 50%` against a positioned ancestor of AUTO height (§10.1: the
    /// containing block is its used padding box, definite once laid out) this is
    /// the icon-centring idiom every component library uses. Taking the
    /// containing block from the SPECIFIED height left `top:50%` unresolvable,
    /// so the box fell back to its static position — a full box-height too low.
    #[test]
    fn an_icon_centres_with_top_50_percent_and_a_translate() {
        let l = lay(
            "<body><div style=\"position:relative;width:300px\">\
             <div style=\"height:32px\"></div>\
             <span style=\"position:absolute;top:50%;left:9px;width:20px;height:20px;\
             background:#c00;transform:translateY(-50%)\"></span></div></body>",
            400,
        );
        let icon = *rects(&l).iter().find(|(_, _, _, _, c)| *c == Rgb(204, 0, 0)).expect("icon");
        // Parent's padding box is 32 tall from y=8 → 50 % is 24, less half the
        // icon = y+6. Falling back to the static position would give y+32.
        assert_eq!(icon.1, 8 + 6, "centred, not at its static position");
        assert_eq!(icon.0, 8 + 9, "left is untouched by translateY");
        // A pixel translate on an in-flow box shifts the paint without moving
        // anything else, and `translate(x,y)` takes both axes.
        let l2 = lay(
            "<body><div style=\"width:50px;height:10px;background:#0c0;\
             transform:translate(20px,5px)\"></div></body>",
            400,
        );
        let b = *rects(&l2).iter().find(|(_, _, _, _, c)| *c == Rgb(0, 204, 0)).expect("box");
        assert_eq!((b.0, b.1), (8 + 20, 8 + 5));
        // Anything that is not a translation is dropped rather than guessed at.
        let l3 = lay(
            "<body><div style=\"width:50px;height:10px;background:#00c;\
             transform:rotate(45deg)\"></div></body>",
            400,
        );
        let c = *rects(&l3).iter().find(|(_, _, _, _, k)| *k == Rgb(0, 0, 204)).expect("box");
        assert_eq!((c.0, c.1), (8, 8), "a rotation must not move the box instead");
    }

    /// The width MEASUREMENT must resolve styles with the same sibling context
    /// the layout walk uses, or a sibling-combinator rule is applied by one and
    /// ignored by the other — and the two then disagree about the same box.
    ///
    /// Every component library hides an icon-only button's label with the
    /// visually-hidden idiom on `span + span`. Measuring without the siblings
    /// left the label in flow for sizing purposes, so the button came out as
    /// wide as its hidden text: Wikipedia's hamburger was ~80px too wide and
    /// shoved the logo and the search field right across the whole header.
    #[test]
    fn shrink_to_fit_sees_sibling_combinator_rules() {
        let l = lay(
            "<html><head><style>\
             .btn span + span{position:absolute;width:1px;height:1px;overflow:hidden}\
             .btn{display:inline-block;background:#0f0}\
             </style></head><body>\
             <span class=\"btn\"><span>I</span><span>Hauptmenü</span></span>\
             </body></html>",
            600,
        );
        let btn = *rects(&l).iter().find(|(_, _, _, _, c)| *c == Rgb(0, 255, 0)).expect("button");
        assert!(btn.2 < 30, "only the icon counts, got {}px wide", btn.2);
    }

    /// The presentational half of the old web: `<center>` is a BLOCK
    /// (HTML rendering §15.3.2) and `bgcolor` is a background hint (§15.3.3).
    /// Left as the initial `inline`, `<center>` swallows what it wraps into a
    /// line box — and a `<table>` inside it collapses into running text.
    /// Hacker News wraps its whole page in one and paints its masthead with
    /// `bgcolor`, so it rendered as a single grey paragraph.
    #[test]
    fn center_is_a_block_and_bgcolor_paints() {
        let l = lay(
            "<body><center><table><tr><td>A1</td><td>A2</td></tr>\
             <tr><td>A3</td></tr></table></center></body>",
            500,
        );
        let ys: Vec<i32> = l.ops.iter().filter_map(|o| match o {
            DrawOp::Text { y, text, .. } if text.trim() == "A1" || text.trim() == "A3" => Some(*y),
            _ => None,
        }).collect();
        assert_eq!(ys.len(), 2, "two cells on two rows, got {:?}", l.ops.len());
        assert!(ys[1] > ys[0], "the second row sits BELOW the first, not inline");
        // `bgcolor` paints, and author CSS still outranks it.
        let bg = |html: &str| {
            rects(&lay(html, 500)).into_iter().map(|(_, _, _, _, c)| c).collect::<Vec<_>>()
        };
        assert!(bg("<body><table><tr><td bgcolor=\"#ff6600\">x</td></tr></table></body>")
            .contains(&Rgb(255, 102, 0)));
        assert!(bg("<body><table><tr><td bgcolor=\"#ff6600\" style=\"background:#00ff00\">x</td></tr></table></body>")
            .contains(&Rgb(0, 255, 0)), "author CSS wins over the attribute");
    }

    #[test]
    fn a_replaced_element_with_no_intrinsic_size_is_300_by_150() {
        // CSS2.1 §10.3.2 + §10.6.2. We never load a frame, a video or a canvas
        // bitmap — but the BOX is still there, and on the real web that box is
        // every video embed and every embedded map.
        let box_of = |html: &str| {
            let l = lay(html, 800);
            rects(&l)
                .into_iter()
                .find(|(_, _, _, _, c)| *c == Rgb(255, 0, 0))
                .map(|(_, _, w, h, _)| (w, h))
        };
        for tag in ["iframe", "video", "canvas", "embed", "object"] {
            let html = std::format!("<body><{tag} style=\"background:#ff0000\"></{tag}></body>");
            assert_eq!(box_of(&html), Some((300, 150)), "<{tag}> with no size");
        }
        // The presentational attributes size it — how a video embed states 16:9.
        assert_eq!(
            box_of("<body><iframe width=560 height=315 style=\"background:#ff0000\"></iframe></body>"),
            Some((560, 315)),
        );
        // CSS wins over the attribute, and `height: auto` still falls back to
        // the intrinsic 150 rather than to the zero its content would give.
        assert_eq!(
            box_of("<body><iframe width=560 style=\"width:200px;background:#ff0000\"></iframe></body>"),
            Some((200, 150)),
        );
    }

    #[test]
    fn an_object_with_fallback_content_is_not_a_replaced_box() {
        // HTML §4.8.7: when the resource cannot be obtained — and ours never
        // can — the element represents its fallback content instead.
        let l = lay("<body><object>fallback text</object></body>", 800);
        assert!(texts(&l).iter().any(|(_, _, t)| t.contains("fallback")), "fallback renders");
        // With nothing to fall back to it stays an empty replaced box.
        let l = lay("<body><object style=\"background:#ff0000\"></object></body>", 800);
        let r = rects(&l).into_iter().find(|(_, _, _, _, c)| *c == Rgb(255, 0, 0));
        assert_eq!(r.map(|(_, _, w, h, _)| (w, h)), Some((300, 150)));
    }

    #[test]
    fn img_box_is_emitted_and_sized_before_its_pixels_arrive() {
        // With both dimensions given, the box is definite: layout emits ONE
        // image op carrying the src, at the authored size, and does NOT need
        // the pixels. Drawing the placeholder is the rasteriser's job now, so
        // the arriving image is a repaint rather than a re-layout.
        let l = lay("<body><img src=\"/x.png\" alt=\"Foto\" width=\"200\" height=\"100\"></body>", 800);
        let img: Vec<_> = l.ops.iter().filter_map(|o| match o {
            DrawOp::Image { w, h, src, alt, .. } => Some((*w, *h, src.as_str(), alt.as_str())),
            _ => None,
        }).collect();
        assert_eq!(img.len(), 1, "one image op");
        assert_eq!(img[0], (200, 100, "/x.png", "Foto"));
        assert!(l.guessed_image_srcs.is_empty(), "definite width+height → repaint suffices");
    }

    #[test]
    fn img_without_dimensions_flags_a_relayout() {
        // No width/height and no decoded pixels → the box is a guess, so a
        // later decode really does move the page and the shell must re-lay-out.
        let l = lay("<body><img src=\"/x.png\" alt=\"Foto\"></body>", 800);
        assert_eq!(l.guessed_image_srcs, vec!["/x.png".to_string()], "guessed box → re-layout needed");
    }

    /// Lay out with live form state (what the shell does while the user types).
    fn lay_forms(html: &str, w: u32, st: &FormState) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom, crate::css::Media::new(800.0, false));
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, st, false)
    }

    #[test]
    fn form_controls_flow_inline_and_are_hit_testable() {
        // A search form: label text, field and button share one line, and each
        // control is clickable at its painted rect.
        let l = lay(
            "<body><form action=/s>Suche: <input name=q size=20>\
             <input type=submit value=Los></form></body>",
            2000,
        );
        assert_eq!(l.controls.len(), 2);
        let (field, button) = (&l.controls[0], &l.controls[1]);
        assert_eq!(field.kind, ControlKind::Text);
        assert!(button.kind.is_submit());
        assert_eq!(field.y, button.y, "field + button on the same line");
        assert!(button.x >= field.x + field.w, "button follows the field");
        // The label sits on that same line, to the left of the field.
        let t = texts(&l);
        let label = t.iter().find(|(_, _, s)| s.starts_with("Suche")).expect("label");
        assert!(label.0 < field.x);
        // Hit-test: a point inside the field finds the field, not the button.
        let hit = l.hit_control(field.x + 4, field.y + 4).expect("hit");
        assert_eq!(hit.seq, field.seq);
        assert!(l.hit_control(field.x - 40, field.y + 4).is_none());
        assert!(l.ops.iter().any(|o| matches!(o, DrawOp::Text { text, .. } if text == "Los")));
    }

    #[test]
    fn controls_render_once_in_flex_grid_and_table_contexts() {
        // Real search boxes sit in a `display:flex` row, which reaches children
        // through `layout_box` — NOT the in-flow walk. And table/grid sizing
        // lays boxes out speculatively to measure them, discarding the ops; the
        // control hit rects must be discarded with them or every control is
        // recorded several times (at stale positions → clicks miss).
        let l = lay(
            "<body><form action=/s>\
             <div style=\"display:flex\"><input name=q style=\"width:300px\"><button>Los</button></div>\
             <div style=\"display:grid; grid-template-columns:1fr 1fr\"><input name=a><input name=b></div>\
             <table><tr><td><input name=c></td><td>Text</td></tr></table>\
             </form></body>",
            1000,
        );
        assert_eq!(l.controls.len(), 5, "each control recorded exactly once");
        let (field, button) = (&l.controls[0], &l.controls[1]);
        // As a flex item the control fills the box flex resolved for it, so it
        // sits flush beside its neighbour instead of overlapping it.
        assert_eq!(field.w, 300, "CSS width sizes the flex item's control");
        assert!(button.x >= field.x + field.w, "flex row places them side by side");
        // Grid items stretch to their column (`justify-items: stretch`), share a
        // row, and the table's control lands below both.
        assert_eq!(l.controls[2].y, l.controls[3].y);
        assert_eq!(l.controls[2].w, l.controls[3].w);
        assert!(l.controls[2].w > 300, "1fr column stretches the field");
        assert!(l.controls[4].y > l.controls[2].y);
    }

    /// A form control's chrome follows the surface it sits on. The engine runs
    /// on a DARK theme here, so a page that says nothing keeps dark controls —
    /// but a page that paints itself light (Wikipedia does, whatever the
    /// desktop is set to) must not get a black box on its white background.
    #[test]
    fn control_chrome_follows_the_page_not_the_device_theme() {
        // The control's face is the first rect painted for it.
        let face = |css: &str| {
            let l = lay(&alloc::format!("<body{css}><input type=text></body>"), 400);
            rects(&l).into_iter().map(|r| r.4).next().expect("control face")
        };
        let dark_page = face("");
        let light_page = face(" style=\"color:#202122\"");
        assert!(luma(dark_page) < 128, "dark page keeps a dark control: {dark_page:?}");
        assert!(luma(light_page) > 200, "light page gets a light control: {light_page:?}");
    }

    #[test]
    fn typed_value_and_caret_render_only_when_focused() {
        let html = "<body><form action=/s><input name=q placeholder=Suchbegriff></form></body>";
        // Empty + unfocused → the placeholder, no caret.
        let l = lay(html, 800);
        assert!(l.ops.iter().any(|o| matches!(o, DrawOp::Text { text, .. } if text == "Suchbegriff")));
        let seq = l.controls[0].seq;
        let plain_rects = rects(&l).len();

        // Typed + focused → the value, plus a 1px caret rect.
        let mut st = FormState::default();
        st.set_value(seq, "nopeek".to_string());
        st.focus = Some(seq);
        st.caret = 6;
        let l2 = lay_forms(html, 800, &st);
        assert!(l2.ops.iter().any(|o| matches!(o, DrawOp::Text { text, .. } if text == "nopeek")));
        assert!(!l2.ops.iter().any(|o| matches!(o, DrawOp::Text { text, .. } if text == "Suchbegriff")));
        assert_eq!(rects(&l2).len(), plain_rects + 1, "the caret is the one extra rect");
    }

    #[test]
    fn hidden_inputs_and_control_children_never_paint() {
        // A hidden field takes no space; a <button>'s label paints inside the
        // button, and a <select>'s options never leak into page text.
        let l = lay(
            "<body><form action=/s><input type=hidden name=t value=x>\
             <select name=s><option value=a>Alpha<option value=b selected>Beta</select>\
             <button>Senden</button></form></body>",
            800,
        );
        assert_eq!(l.controls.len(), 2, "hidden input renders no box");
        assert_eq!(l.controls[0].kind, ControlKind::Select);
        let t = texts(&l);
        assert!(t.iter().any(|(_, _, s)| *s == "Beta"), "select shows the selected option");
        assert!(!t.iter().any(|(_, _, s)| *s == "Alpha"), "unselected options stay hidden");
        assert!(t.iter().any(|(_, _, s)| *s == "Senden"));
    }

    /// Google wraps its search button in a bordered `<span>` and writes
    /// `border: none` on the `<input>`. Painting our own frame regardless put a
    /// second rectangle a pixel down and right of the wrapper's — the "shadow"
    /// on both home-page buttons.
    #[test]
    fn a_page_that_styles_a_controls_border_owns_it() {
        let plain = lay("<body><input type=submit value=OK></body>", 400);
        let bare = lay(
            "<body><input type=submit value=OK style=\"border:none\"></body>",
            400,
        );
        // The UA frame is four 1px rects around the face; `border: none` is a
        // declaration, not an absence, and removes all four.
        assert_eq!(rects(&plain).len(), rects(&bare).len() + 4, "border:none keeps no frame");

        // The page's own widths and colours, per side.
        let styled = lay(
            "<body><input type=submit value=OK \
             style=\"border:3px solid #ff0000;border-bottom-width:7px\"></body>",
            400,
        );
        let red: Vec<_> = rects(&styled).into_iter().filter(|r| r.4 == Rgb(255, 0, 0)).collect();
        assert_eq!(red.len(), 4, "one rect per side, got {red:?}");
        assert!(red.iter().any(|r| r.3 == 7), "the thick bottom side, got {red:?}");
        assert_eq!(red.iter().filter(|r| r.2 == 3 || r.3 == 3).count(), 3, "three 3px sides");

        // `border-color: transparent` keeps the width and paints nothing — the
        // idiom for reserving a frame's space without showing it.
        let clear = lay(
            "<body><input type=submit value=OK \
             style=\"border:3px solid transparent\"></body>",
            400,
        );
        assert_eq!(rects(&clear).len(), 1, "only the face is painted");
    }

    /// A control measured with a ROOT style read its label at the root font
    /// size and lost every declared size, so a shrink-to-fit wrapper reserved
    /// more width than the control paints — the button sat in a box wider than
    /// itself, with a strip of the wrapper showing on the right.
    #[test]
    fn a_control_measures_with_its_own_style_not_a_root_one() {
        let l = lay(
            "<body><span style=\"display:inline-block;background:#0f0\">\
             <input type=submit value=\"Google Suche\" \
             style=\"border:none;font-size:11px\"></span></body>",
            800,
        );
        let r = rects(&l);
        let wrapper = r.iter().find(|x| x.4 == Rgb(0, 255, 0)).expect("wrapper");
        let face = r.iter().find(|x| x.4 != Rgb(0, 255, 0)).expect("control face");
        assert_eq!(wrapper.2, face.2, "wrapper reserves exactly the control's width");
    }

    /// A button-like control is border-box in the UA sheet (HTML rendering
    /// §15.5.1); a text field is not. Read as content-box, Google's
    /// `height:30px` button came out 8px taller than the `height:30px` wrapper
    /// it was built to fit and hung out the bottom.
    #[test]
    fn a_button_is_border_box_and_a_text_field_is_not() {
        let face_h = |html: &str| rects(&lay(html, 400))[0].3;
        assert_eq!(
            face_h("<body><input type=submit value=OK style=\"height:30px;border:none\"></body>"),
            30,
            "a button's height is its whole box"
        );
        let field = face_h("<body><input type=text style=\"height:30px;border:none\"></body>");
        assert!(field > 30, "a text field adds its padding to a content height, got {field}");
        // An explicit `box-sizing` from the page still wins over the UA sheet.
        assert!(
            face_h(
                "<body><input type=submit value=OK \
                 style=\"height:30px;border:none;box-sizing:content-box\"></body>"
            ) > 30,
            "the page can opt back out"
        );
    }

    /// Focus is the one part of the frame a page cannot take away: the ring
    /// says where typing goes, and `border: none` was never meant to hide that.
    #[test]
    fn a_borderless_control_still_shows_a_focus_ring() {
        let html = "<body><form action=/s><input name=q style=\"border:none\"></form></body>";
        let l = lay(html, 400);
        let seq = l.controls[0].seq;
        let mut st = FormState::default();
        st.focus = Some(seq);
        let focused = lay_forms(html, 400, &st);
        assert_eq!(rects(&focused).len(), rects(&l).len() + 4 + 1, "ring plus caret");
    }

    #[test]
    fn checkbox_paints_its_mark_only_when_checked() {
        let html = "<body><form action=/s><input type=checkbox name=a></form></body>";
        let l = lay(html, 800);
        let seq = l.controls[0].seq;
        let unchecked = rects(&l).len();
        let mut st = FormState::default();
        st.set_value(seq, String::new());
        let f = crate::forms::collect(&dom::parse(html));
        st.toggle(&f, seq);
        let l2 = lay_forms(html, 800, &st);
        assert_eq!(rects(&l2).len(), unchecked + 1, "the tick is one filled rect");
    }

    #[test]
    fn img_in_a_link_flows_inline_and_is_clickable() {
        // Wikipedia's pattern: <a><img></a> among text. The image must flow on
        // the same line as the surrounding words AND be a clickable link.
        let l = lay(
            "<body><p>vor <a href=\"/x\"><img src=\"/i.png\" alt=\"pic\" width=\"40\" height=\"30\"></a> nach</p></body>",
            2000,
        );
        let t = texts(&l);
        let vor = *t.iter().find(|(_, _, s)| *s == "vor").expect("vor");
        let nach = *t.iter().find(|(_, _, s)| *s == "nach").expect("nach");
        assert_eq!(vor.1, nach.1, "text before + after the inline image share a line");
        assert!(nach.0 > vor.0 + 40, "the image took horizontal space between them");
        assert!(l.links.iter().any(|lk| lk.href == "/x"), "the linked image is clickable");
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
    fn flex_container_paints_its_own_box() {
        // A flex container honours its own width/height/background (it establishes
        // the box, not just a passthrough for its items).
        let l = lay(
            "<body><div style=\"display:flex; background:#112233; width:200px; height:50px\">\
             <span>x</span></div></body>",
            800,
        );
        let bg = l.ops.iter().find_map(|o| match o {
            DrawOp::Rect { w, h, color, .. } if *color == Rgb(0x11, 0x22, 0x33) => Some((*w, *h)),
            _ => None,
        });
        let (w, h) = bg.expect("flex container background rect emitted");
        assert!((w - 200).abs() <= 2, "container width honoured (got {w})");
        assert!((h - 50).abs() <= 2, "container height honoured (got {h})");
    }

    #[test]
    fn flex_align_items_stretch_fills_cross_size() {
        // Default align-items:stretch → an auto-height item fills the container's
        // definite cross size (100px), not just its one-line content height.
        let l = lay(
            "<body><div style=\"display:flex; height:100px\">\
             <span style=\"background:#00ff00\">x</span></div></body>",
            800,
        );
        let h = l.ops.iter().find_map(|o| match o {
            DrawOp::Rect { h, color, .. } if *color == Rgb(0, 0xff, 0) => Some(*h),
            _ => None,
        });
        assert!(h.expect("green item rect") > 80, "stretched item ~fills the 100px cross size");
    }

    #[test]
    fn flex_column_stacks_with_item_margins() {
        // Column direction stacks items vertically; their (non-collapsing) margins
        // separate them.
        let l = lay(
            "<body><div style=\"display:flex; flex-direction:column\">\
             <div style=\"margin:10px\">a</div><div style=\"margin:10px\">b</div></div></body>",
            800,
        );
        let t = texts(&l);
        let a = *t.iter().find(|(_, _, s)| *s == "a").expect("a");
        let b = *t.iter().find(|(_, _, s)| *s == "b").expect("b");
        assert_eq!(a.0, b.0, "column items share a left edge");
        assert!(b.1 > a.1 + 30, "b stacks below a with margin gap (got dy {})", b.1 - a.1);
    }

    #[test]
    fn flex_wrap_moves_overflowing_item_to_next_line() {
        // Two 120px items in a 200px wrap container → the 2nd wraps below the 1st.
        let l = lay(
            "<body><div style=\"display:flex; flex-wrap:wrap; width:200px\">\
             <div style=\"width:120px\">a</div><div style=\"width:120px\">b</div></div></body>",
            800,
        );
        let t = texts(&l);
        let a = *t.iter().find(|(_, _, s)| *s == "a").expect("a");
        let b = *t.iter().find(|(_, _, s)| *s == "b").expect("b");
        assert_eq!(a.0, b.0, "wrapped item returns to the start edge");
        assert!(b.1 > a.1, "the 2nd item wrapped onto a new line");
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
    fn grid_template_rows_and_separate_gaps() {
        // 2×2 grid, fixed 50px rows, `gap: 40px 20px` (row-gap 40, column-gap 20).
        let l = lay(
            "<body><div style=\"display:grid; grid-template-columns:100px 100px; \
             grid-template-rows:50px 50px; gap:40px 20px\">\
             <div>a</div><div>b</div><div>c</div><div>d</div></div></body>",
            800,
        );
        let t = texts(&l);
        let g = |s: &str| *t.iter().find(|(_, _, x)| *x == s).expect(s);
        let (a, b, c) = (g("a"), g("b"), g("c"));
        assert_eq!(b.0 - a.0, 120, "column gap 20 → track 100 + gap 20");
        assert_eq!(c.0, a.0, "c under a in column 0");
        assert_eq!(c.1 - a.1, 90, "row 50 + row-gap 40");
    }

    #[test]
    fn grid_explicit_line_placement() {
        // `grid-column: 3` puts the item in the third track (200px offset); a
        // later `grid-row: 2` item drops to the second row.
        let l = lay(
            "<body><div style=\"display:grid; grid-template-columns:100px 100px 100px; \
             grid-template-rows:50px 50px\">\
             <div>x</div><div style=\"grid-column:3\">y</div>\
             <div style=\"grid-row:2\">z</div></div></body>",
            800,
        );
        let t = texts(&l);
        let g = |s: &str| *t.iter().find(|(_, _, x)| *x == s).expect(s);
        let (x, y, z) = (g("x"), g("y"), g("z"));
        assert_eq!(y.0 - x.0, 200, "grid-column:3 → third track");
        assert_eq!(y.1, x.1, "y stays on row 1");
        assert_eq!(z.0, x.0, "z auto-flows into column 0");
        assert_eq!(z.1 - x.1, 50, "grid-row:2 → second row (50px down)");
    }

    #[test]
    fn grid_auto_fill_expands_to_container_width() {
        // `repeat(auto-fill, 100px)` in a 300px grid → exactly 3 columns.
        let l = lay(
            "<body><div style=\"display:grid; width:300px; \
             grid-template-columns:repeat(auto-fill,100px)\">\
             <div>a</div><div>b</div><div>c</div><div>d</div></div></body>",
            800,
        );
        let t = texts(&l);
        let g = |s: &str| *t.iter().find(|(_, _, x)| *x == s).expect(s);
        let (a, b, c, d) = (g("a"), g("b"), g("c"), g("d"));
        assert_eq!(a.1, b.1, "a,b same row");
        assert_eq!(b.1, c.1, "b,c same row (3 columns fit)");
        assert_eq!(b.0 - a.0, 100);
        assert_eq!(c.0 - b.0, 100);
        assert_eq!(d.0, a.0, "4th item wraps under column 0");
        assert!(d.1 > a.1, "4th item on the next row");
    }

    #[test]
    fn grid_item_stretches_to_fixed_row_height() {
        // An auto-height item defaults to `align-self: stretch` → its background
        // fills the 80px explicit row.
        let l = lay(
            "<body><div style=\"display:grid; grid-template-columns:100px; \
             grid-template-rows:80px\">\
             <div style=\"background:#ff0000\">a</div></div></body>",
            800,
        );
        let red = rects(&l).into_iter().find(|(_, _, _, _, c)| *c == Rgb(255, 0, 0));
        assert_eq!(red.map(|r| r.3), Some(80), "item bg stretches to the 80px row");
    }

    #[test]
    fn grid_shorthand_rows_slash_columns() {
        // `grid: <rows> / <columns>` sets both track lists.
        let l = lay(
            "<body><div style=\"display:grid; grid:50px / 100px 100px\">\
             <div>a</div><div>b</div></div></body>",
            800,
        );
        let t = texts(&l);
        let g = |s: &str| *t.iter().find(|(_, _, x)| *x == s).expect(s);
        let (a, b) = (g("a"), g("b"));
        assert_eq!(a.1, b.1, "two columns → same row");
        assert_eq!(b.0 - a.0, 100, "second column at 100px");
    }

    #[test]
    fn list_items_get_bullets_and_indent() {
        let l = lay("<body><ul><li>one</li><li>two</li></ul></body>", 800);
        let bullets = l.ops.iter().filter(|o| matches!(o, DrawOp::Rect { .. })).count();
        assert_eq!(bullets, 2, "one bullet per li");
        // list text is indented past the plain content edge (the body margin)
        assert!(texts(&l).iter().all(|(x, _, _)| *x > 8));
    }
}

/// The definite **padding-box** height of a positioned box — what `top`/`bottom`
/// percentages on its absolutely-positioned descendants resolve against
/// (CSS 2.1 §9.3.2). Only an explicit `height` counts: abspos children are laid
/// out during the parent's child walk, before its content height exists.

/// `colspan` (HTML §4.9.11): how many columns a cell occupies. `0` means "to
/// the end of the row group" in old HTML and was dropped from the spec, so it
/// folds to 1 like any other unparseable value.
fn cell_span(cell: &Cell) -> usize {
    match cell {
        Cell::Real(e) => e.attr("colspan").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 64),
        Cell::Anon(_) => 1,
    }
}

/// The start column of every cell in `row`, and how many columns the row
/// occupies in total. Without `rowspan` a row's cells simply pack left to
/// right, each taking `colspan` slots.
fn row_columns(row: &[StyledCell]) -> (Vec<usize>, usize) {
    let mut starts = Vec::with_capacity(row.len());
    let mut c = 0usize;
    for sc in row {
        starts.push(c);
        c += cell_span(&sc.cell);
    }
    (starts, c)
}

/// Widen `track[c .. c+span]` just enough that it totals `want`, sharing the
/// shortfall equally. CSS2 §17.5.2.2 leaves the distribution up to the UA; a
/// spanning cell must never dictate a single column's width, which is what
/// made a `<td colspan="2" style="width:290px">` infobox header blow column 0
/// up to the width meant for the whole table.
fn spread_span(track: &mut [f32], c: usize, span: usize, want: f32) {
    let end = (c + span).min(track.len());
    if end <= c {
        return;
    }
    let have: f32 = track[c..end].iter().sum();
    if want <= have {
        return;
    }
    let extra = (want - have) / (end - c) as f32;
    for t in &mut track[c..end] {
        *t += extra;
    }
}

/// A cell's used border widths (left, right, top, bottom). In the collapsed
/// model a border is shared with the neighbouring cell and sits centred on the
/// grid line, so only HALF of it lies inside this cell (CSS2.1 §17.6.2) — that
/// half is what the column widths and the content box have to account for.
fn cell_borders(cs: &ComputedStyle, collapse: bool) -> (f32, f32, f32, f32) {
    let (l, r, t, b) = (cs.border_left.width, cs.border_right.width, cs.border_top.width, cs.border_bottom.width);
    if collapse {
        (l / 2.0, r / 2.0, t / 2.0, b / 2.0)
    } else {
        (l, r, t, b)
    }
}

/// The border that wins a collapsed grid line (CSS2.1 §17.6.2.1). Width-first,
/// which is the part that decides real tables; the full style-then-element
/// priority chain only matters when the widths tie, and a tie already draws the
/// same line at the same size.
fn collapsed_edge(a: &BorderSide, b: &BorderSide) -> BorderSide {
    // Rule 1 first: one `hidden` among the boxes meeting at a grid line
    // suppresses that line outright, however wide the others are.
    if a.hidden || b.hidden {
        return BorderSide { hidden: true, ..BorderSide::default() };
    }
    if b.width > a.width {
        *b
    } else {
        *a
    }
}

/// A table's used `border-spacing` in px. The collapsed border model merges
/// adjacent borders instead of spacing them, so it ignores the property
/// entirely (CSS2.1 §17.6.1).
fn spacing_of(st: &ComputedStyle) -> (i32, i32) {
    if st.border_collapse {
        (0, 0)
    } else {
        (st.border_spacing.0 as i32, st.border_spacing.1 as i32)
    }
}

/// Clamp an outer (border-box) width to `min-width`/`max-width`, which are
/// content-box lengths unless `box-sizing: border-box` is in effect. Only
/// definite px limits apply — a percentage limit needs a containing block that
/// intrinsic sizing does not have yet.
fn clamp_len(outer: f32, min_w: Len, max_w: Len, box_border: bool, frame: f32) -> f32 {
    let to_outer = |v: f32| if box_border { v } else { v + frame };
    let mut out = outer;
    if let Len::Px(mx) = max_w {
        out = out.min(to_outer(mx));
    }
    if let Len::Px(mn) = min_w {
        out = out.max(to_outer(mn));
    }
    out.max(0.0)
}

/// Whether a box lays its children out along the inline axis, so their
/// max-contents add up rather than the widest one winning.
fn side_by_side(st: &ComputedStyle) -> bool {
    match st.display {
        Display::TableRow => true,
        Display::Flex => st.flex_row,
        Display::Grid => true,
        _ => false,
    }
}

/// Resolve a vertical length against a containing-block height (CSS2.1 §9.3.2 /
/// §10.5). A percentage needs a definite CB height; an indefinite one (the
/// parent's content height doesn't exist yet while its children lay out) leaves
/// it unresolvable, which behaves as `auto`.
fn vert_len(len: Len, cbh: Option<i32>) -> Option<f32> {
    match len {
        Len::Auto => None,
        Len::Px(p) => Some(p),
        Len::Pct(p) => cbh.map(|h| p / 100.0 * h as f32),
        Len::Calc { pct, px } if pct == 0.0 => Some(px),
        Len::Calc { pct, px } => cbh.map(|h| pct / 100.0 * h as f32 + px),
    }
}

/// The CONTENT-box height a definite `height`/`min-`/`max-height` asks for.
/// Under `box-sizing: border-box` the used height spans padding AND border;
/// flex and grid each subtracted only the padding, so every bordered container
/// with a definite height came out two border-widths too tall — and a root
/// `display:flex` stretched between `top`/`bottom` overshot the viewport.
fn content_height_of(st: &ComputedStyle, len: Len) -> Option<f32> {
    match len {
        Len::Px(h) if st.box_border => Some((h - st.pad_top - st.pad_bottom - st.border_y()).max(0.0)),
        Len::Px(h) => Some(h),
        _ => None,
    }
}

fn definite_cb_height(st: &ComputedStyle) -> Option<i32> {
    let pad_v = st.pad_top as i32 + st.pad_bottom as i32;
    match st.height {
        // `box-sizing:border-box` → the used height already spans padding AND
        // border, so the padding box is that minus the border.
        Len::Px(h) if st.box_border => Some((h as i32 - st.border_y() as i32).max(0)),
        Len::Px(h) => Some(h as i32 + pad_v),
        _ => None,
    }
}

/// The containing block a positioned box establishes for its absolutely
/// positioned descendants: its **padding** box, not its content box (CSS2.1
/// §10.1). Given the box's content origin and width, back out to the padding
/// edges — `top: 0` sits just inside the border, and `left: 0` at the padding
/// edge, so a padded container does not push its abspos children inwards.
fn padding_cb(st: &ComputedStyle, content_x: i32, content_top: i32, content_w: i32) -> (i32, i32, i32, Option<i32>) {
    (
        content_x - st.pad_left as i32,
        content_top - st.pad_top as i32,
        content_w + (st.pad_left + st.pad_right) as i32,
        definite_cb_height(st),
    )
}
