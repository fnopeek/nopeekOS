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
    self, BgLayer, BgPos, BgSize, BorderSide, ClearKind, Clip, ComputedStyle, ContentAlign,
    ContentPiece, CrossAlign, Display, FlexBasis, FloatKind, GridTrack, Intrinsic, Justify, Len, ListStyle,
    ObjectFit, Overflow, Position, TableLayout,
    TextAlign, TextTransform, ZIndex, BASE_FONT_PX,
};


/// The deferred recipe for a positioned box's containing-block HEIGHT.
///
/// §10.1 makes a positioned box the containing block for its absolutely
/// positioned descendants, and that block's height is a USED height — known
/// only once the box has been laid out. Computing it eagerly means a full
/// speculative layout of the whole box, and almost nothing ever reads the
/// answer: only a descendant with `bottom`, or a percentage `top`/`height`,
/// resolves against it. On a large article 266 boxes paid for that and it was
/// over half of the layout.
///
/// So the recipe is kept instead, and run on the first read — in the context
/// the eager measurement would have seen, which is what the saved `path_len`,
/// `cb`, `cb_h` and `floats` restore. Everything here is either `Copy` or, in
/// the case of `floats`, empty in the common case (an empty `Vec` clone does
/// not allocate).
struct PendingCbH<'a> {
    el: &'a Element,
    st: ComputedStyle,
    x: i32,
    w: i32,
    y: i32,
    /// Border box → padding box, subtracted from the measured height.
    border_y: i32,
    path_len: usize,
    cb: PosCb,
    cb_h: Option<f32>,
    floats: Vec<FloatRect>,
    /// The answer, once someone has asked. `cb.3` caches it too, but only for
    /// as long as that particular `cb` value lives.
    resolved: Option<i32>,
}

/// The positioned containing block: `(x, y, width, height)` plus, when the
/// height is not yet known, the index of the recipe that computes it.
/// Deliberately still `Copy` and still a tuple-ish value — the extra slot
/// makes the compiler visit every site that installs or restores a containing
/// block, which is the point: a pending recipe that outlives its `cb` would
/// hand some unrelated descendant the wrong height.
type PosCb = (i32, i32, i32, Option<i32>, Option<u32>);

/// Where everything a layout RECORDS stood before a speculative run — see
/// `Ctx::spec_mark`. One list, deliberately: a recorded vector that is not
/// rolled back leaks trial-run entries into the real page, and that has now
/// happened twice (`stack_ops`/`floats`, then `hover_boxes`). A new side table
/// is added here and is then rolled back by every speculative site at once.
#[derive(Clone, Copy)]
struct SpecMark {
    ops: usize,
    links: usize,
    controls: usize,
    stack_ops: usize,
    stack_links: usize,
    float_ops: usize,
    float_links: usize,
    floats: usize,
    inspects: usize,
    hover_boxes: usize,
}

/// A speculative flex-item placement additionally moves the containing block.
#[derive(Clone, Copy)]
struct FlexMark {
    spec: SpecMark,
    cb: PosCb,
}

/// Sites that ask for a speculative height. A distinct key per site in the
/// `measured` memo, because the same element asked about by two of them is two
/// different questions with two different derived styles. Only the column axis
/// still measures speculatively — a flex ROW lays its items out for real and
/// keeps the result (see `flex_row`).
const MEAS_FLEX_COL: u8 = 1;

/// Identity of one speculative height measurement, for the `measured` memo.
///
/// `site` separates the call sites, because the same element can be measured
/// by two of them with different styles — a `position: relative` flex item is
/// measured once as a flex item (with `flex_item_style` applied) and once as
/// its own containing block. The style itself is NOT hashed: at every site it
/// is derived from the element's resolved style (which `styled` already keys
/// by identity) plus `arg`, so the pair identifies it exactly.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
struct MeasureKey {
    site: u8,
    seq: u32,
    x: i32,
    y: i32,
    w: i32,
    /// The site's own style-deriving argument (flex main size), as bits.
    arg: u32,
    /// Everything ambient that the measurement can read: the containing
    /// block's height for percentages, and whether floats are in play (they
    /// make the answer depend on where the box sits).
    cb_h: u64,
    cb_def_h: i64,
    floats: u32,
}

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
    st.bfc_root
        || matches!(st.display,
                    Display::Flex | Display::InlineFlex | Display::Grid | Display::Table)
        || st.overflow_x != Overflow::Visible
        || st.overflow_y != Overflow::Visible
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

/// The image-store key of an inline `<svg>`. `seq` is the document-order index
/// the parser assigns, so the key is stable across re-layouts of the same
/// document and cannot collide with a page's own `src` (no URL has this shape).
fn svg_key(el: &Element) -> alloc::string::String {
    alloc::format!("svg:{}", el.seq)
}

/// What to show if the raster fails. An icon's accessible name is its
/// `aria-label`/`<title>`, which is also what a page gives a control whose
/// only content is that icon.
fn svg_alt(el: &Element, is_svg: bool) -> alloc::string::String {
    if !is_svg {
        return el.attr("alt").unwrap_or("").trim().to_string();
    }
    el.attr("aria-label")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

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

/// Move a detached op list (an `inline-block`'s, laid out at the origin) to
/// where its line box put it.
fn translate_op_list(ops: &mut [DrawOp], dx: i32, dy: i32) {
    for op in ops {
        match op {
            DrawOp::Rect { x, y, .. }
            | DrawOp::RoundRect { x, y, .. }
            | DrawOp::Shadow { x, y, .. }
            | DrawOp::Text { x, y, .. }
            | DrawOp::Image { x, y, .. }
            | DrawOp::BgImage { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
    }
}

/// The box's `box-shadow`, painted behind its background. Only the zero-blur
/// case — which on real pages is a hairline separator, not a drop shadow.
/// MediaWiki draws the rule under the article tabs with
/// `box-shadow: 0 1px #c8ccd1`, and without this the page simply lacks it.
///
/// A free function, not a method, because `repaint_hover` has to produce the
/// very same ops from the very same style — a second copy of the rule there
/// would drift, and one that merely FORGOT the shadow silently left the tab
/// underline behind while recolouring the text above it.
fn shadow_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, out: &mut Vec<DrawOp>) {
    // Der WEICHE zuerst: er liegt hinter dem scharfen. Das ist die Form, in
    // der Bootstrap seine Schatten schreibt (`0 .5rem 1rem rgba(0,0,0,.15)`),
    // und bis 0.61.0 fiel sie ganz weg — nur `blur == 0` wurde gemalt.
    if let Some(sh) = st.shadow_soft {
        let sx = x + sh.dx as i32 - sh.spread as i32;
        let sy = y + sh.dy as i32 - sh.spread as i32;
        let sw = w + 2 * sh.spread as i32;
        let shh = h + 2 * sh.spread as i32;
        if sw > 0 && shh > 0 {
            out.push(DrawOp::Shadow { x: sx, y: sy, w: sw, h: shh, blur: sh.blur,
                                      color: sh.color.unwrap_or(st.color),
                                      dx: sh.dx as i32, dy: sh.dy as i32, spread: sh.spread as i32 });
        }
    }
    let Some(sh) = st.shadow else { return };
    if !sh.paints() {
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
    let mut push = |px: i32, py: i32, pw: i32, ph: i32| {
        if pw > 0 && ph > 0 {
            out.push(DrawOp::Rect { x: px, y: py, w: pw, h: ph, color });
        }
    };
    push(sx, sy, sw, y.min(sy1) - sy);
    push(sx, y1.max(sy), sw, sy1 - y1.max(sy));
    let (my0, my1) = (sy.max(y), sy1.min(y1));
    if my1 > my0 {
        push(sx, my0, x.min(sx1) - sx, my1 - my0);
        push(x1.max(sx), my0, sx1 - x1.max(sx), my1 - my0);
    }
}

/// Der INNERE Schatten, gemalt ueber den Hintergrund und unter den Rahmen.
///
/// Ohne Weichzeichnung ist er ein Rechteck mit einem Loch: der Kasten minus
/// dem, was der Schatten freilaesst. Bootstrap streift damit seine Tabellen
/// (`inset 0 0 0 9999px`) — bei so einer Ausdehnung ist das Loch leer und der
/// Schatten fuellt die ganze Zelle.
fn inset_shadow_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, out: &mut Vec<DrawOp>) {
    let Some(sh) = st.shadow_inset else { return };
    let color = sh.color.unwrap_or(st.color);
    // Das Loch: der Kasten, verschoben und nach innen geschrumpft.
    let (hx, hy) = (x + sh.dx as i32 + sh.spread as i32, y + sh.dy as i32 + sh.spread as i32);
    let (hw, hh) = (w - 2 * sh.spread as i32, h - 2 * sh.spread as i32);
    let (hx1, hy1) = (hx + hw, hy + hh);
    let (x1, y1) = (x + w, y + h);
    let mut push = |px: i32, py: i32, pw: i32, ph: i32| {
        if pw > 0 && ph > 0 {
            out.push(DrawOp::Rect { x: px, y: py, w: pw, h: ph, color });
        }
    };
    if hw <= 0 || hh <= 0 {
        push(x, y, w, h);
        return;
    }
    push(x, y, w, hy.clamp(y, y1) - y);
    push(x, hy1.clamp(y, y1), w, y1 - hy1.clamp(y, y1));
    let (my0, my1) = (hy.clamp(y, y1), hy1.clamp(y, y1));
    if my1 > my0 {
        push(x, my0, hx.clamp(x, x1) - x, my1 - my0);
        push(hx1.clamp(x, x1), my0, x1 - hx1.clamp(x, x1), my1 - my0);
    }
}


/// Clip the display-list ops in `ops[start..]` to the document-space rectangle
/// `[cl, ct) .. [cr, cb)`. Filled rects are intersected (pixel-exact); text and
/// images are kept whole if their box overlaps the rect, dropped otherwise (a
/// flat display list can't clip glyph runs mid-way). An empty rect removes the
/// whole range — the CSS 2.1 `clip` case where nothing of the box is painted.
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
            // Ein weicher Schatten wird nur ganz oder gar nicht behalten:
            // ihn zuzuschneiden hiesse, seine Deckung neu zu rechnen, und die
            // entsteht erst beim Malen.
            DrawOp::Shadow { x, y, w, h, blur, color, dx, dy, spread } => {
                if x >= cl && y >= ct && x + w <= cr && y + h <= cb {
                    ops.push(DrawOp::Shadow { x, y, w, h, blur, color, dx, dy, spread });
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

/// The colour transform an element actually paints with: its `filter`, then
/// its `opacity`. Both end up in the same matrix — `ColorFilter` already has
/// an alpha factor, because `filter: opacity()` needs one — so opacity costs
/// no second pass over the ops.
///
/// It is an APPROXIMATION of what the spec asks for. Real `opacity` composites
/// the element and its subtree as one group: two overlapping descendants are
/// flattened first, then faded together. Scaling each op's alpha instead lets
/// them show through each other. Getting that exactly right needs an offscreen
/// buffer per stacking context; this is the version that costs nothing.
fn effective_filter(st: &ComputedStyle) -> Option<crate::color::ColorFilter> {
    let fade = st.opacity < 0.999;
    match (st.filter, fade) {
        (f, false) => f,
        (None, true) => Some(crate::color::ColorFilter { a: st.opacity, ..crate::color::ColorFilter::IDENTITY }),
        (Some(f), true) => {
            Some(f.then(crate::color::ColorFilter { a: st.opacity, ..crate::color::ColorFilter::IDENTITY }))
        }
    }
}

/// Eine Farbe mit der aufgesammelten Inline-Deckung vormultiplizieren.
/// `k == 1.0` (der Normalfall) laesst sie unangetastet — auch in den Bits.
fn faded(c: Rgba, k: f32) -> Rgba {
    if k >= 0.999 {
        return c;
    }
    Rgba { c: c.c, a: (c.a as f32 * k.clamp(0.0, 1.0)) as u8 }
}

/// Derselbe Stil, mit der Inline-Deckung schon in den Farben. Fuer den
/// SCHMUCK eines Inline-Kastens — Hintergrund, Rahmen, Umriss —, der wie sein
/// Text keinen eigenen Befehlsbereich hat.
///
/// Bewusst nur die Farben, nicht die Bilder: ein Hintergrundbild wird ueber
/// seinen Schluessel erst beim Malen aufgeloest, und ein halbdurchsichtiges
/// Bild braucht einen Filterindex am Befehl. Das ist eine eigene Runde; hier
/// stuende sonst eine Halbheit.
fn fade_style(st: &ComputedStyle) -> ComputedStyle {
    let k = st.inline_fade;
    if k >= 0.999 {
        return *st;
    }
    let mut s = *st;
    s.color = faded(s.color, k);
    s.bg = s.bg.map(|c| faded(c, k));
    for b in [&mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left,
              &mut s.outline] {
        b.color = b.color.map(|c| faded(c, k));
    }
    s
}

/// Intern one `filter` transform, deduped, and return its 1-based index.
fn filter_key(table: &mut Vec<crate::color::ColorFilter>, f: crate::color::ColorFilter) -> u16 {
    let i = table.iter().position(|e| *e == f).unwrap_or_else(|| {
        table.push(f);
        table.len() - 1
    });
    (i + 1).min(u16::MAX as usize) as u16
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
        // `translate` takes no intrinsic keyword; `auto` there is zero.
        Len::Auto | Len::Intrinsic(_) => 0,
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

/// 8-bit RGB plus the alpha a page asked for. `Rgb` stays the opaque value the
/// theme, the image decoders and the compositor speak; alpha lives only on the
/// path a `<color>` travels — cascade → display list → rasteriser — because
/// that is the only path where a page can ask for it and the backdrop it must
/// composite over is known (at paint time, not before).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub c: Rgb,
    pub a: u8,
}

impl Rgba {
    pub const fn opaque(c: Rgb) -> Rgba {
        Rgba { c, a: 255 }
    }
    /// The fast paths (`memory.copy` fills, direct stores) are only valid for
    /// an opaque colour; everything else has to read the destination back.
    pub const fn is_opaque(&self) -> bool {
        self.a == 255
    }
}

impl Rgba {
    /// This colour composited over an opaque one. The canvas is the only place
    /// that must flatten early: it IS the ground, so there is nothing left to
    /// blend against at paint time.
    pub fn over(self, dst: Rgb) -> Rgb {
        if self.is_opaque() {
            return self.c;
        }
        let (a, ia) = (self.a as u32, 255 - self.a as u32);
        let ch = |s: u8, d: u8| ((s as u32 * a + d as u32 * ia) / 255) as u8;
        Rgb(ch(self.c.0, dst.0), ch(self.c.1, dst.1), ch(self.c.2, dst.2))
    }
}

/// Unit tests state colours as opaque `Rgb` literals; comparing the two
/// directly keeps those assertions about the CHANNELS rather than restating the
/// wrapper on every line. Deliberately test-only — production code that means
/// "opaque and this colour" should say so.
#[cfg(test)]
impl PartialEq<Rgb> for Rgba {
    fn eq(&self, other: &Rgb) -> bool {
        self.is_opaque() && self.c == *other
    }
}

impl From<Rgb> for Rgba {
    fn from(c: Rgb) -> Rgba {
        Rgba::opaque(c)
    }
}

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
#[derive(Clone)]
pub enum DrawOp {
    /// A run of already-wrapped, same-style text; `y` is the run's top.
    /// `sp` is `(letter-spacing, word-spacing)` in px — the run measures and
    /// paints at the same advance only because both read this one value.
    Text {
        x: i32,
        y: i32,
        size: f32,
        color: Rgba,
        bold: bool,
        italic: bool,
        mono: bool,
        sp: (f32, f32),
        text: String,
    },
    /// A filled rectangle (divider, list bullet).
    Rect { x: i32, y: i32, w: i32, h: i32, color: Rgba },
    /// Ein WEICHER Schlagschatten: der Kasten, unter dem er liegt, plus der
    /// Weichzeichnungsradius. Der Maler rechnet die Deckung selbst aus.
    ///
    /// Eigener Befehl und kein Haufen `Rect`: ein weicher Schatten hat an
    /// jedem Pixel eine andere Deckung, und die entsteht erst beim Malen.
    /// Ein scharfer (`blur == 0`) bleibt, was er war — vier Rechtecke, weil
    /// er auf echten Seiten meist ein Haarstrich statt eines Schattens ist.
    /// `x,y,w,h` is the shadow's own rect — the border box moved by
    /// `dx,dy` and grown by `spread`. The three CSS numbers ride along
    /// because the painter needs the BORDER BOX back: an outer shadow is not
    /// painted inside it (css-backgrounds-3 §7.1.1), and that cut-out is a
    /// different rectangle as soon as there is an offset or a spread. Keeping
    /// them (rather than the border box itself) is what makes the op survive
    /// a translation untouched.
    Shadow { x: i32, y: i32, w: i32, h: i32, blur: f32, color: Rgba, dx: i32, dy: i32, spread: i32 },
    /// A `border-radius` box. `r` is `[tl, tr, br, bl]` in px; `ring` is 0 for
    /// a solid fill, or the border thickness to stroke along the inside edge.
    /// Kept apart from `Rect` so the plain case stays one `memory.copy` per
    /// row — the rounded one has to walk its corner rows.
    RoundRect { x: i32, y: i32, w: i32, h: i32, r: [f32; 4], color: Rgba, ring: f32 },
    /// A decoded image, scaled to `w`×`h` at blit time.
    /// An `<img>` box. Carries the `src` KEY, not the decoded pixels: the
    /// rasteriser looks the image up when it paints, and draws a placeholder
    /// on a miss. That way an image arriving after layout costs a repaint
    /// instead of a full re-layout — which on a real article is the
    /// difference between ~15 ms and ~145 ms, per image batch.
    /// `fit` is `object-fit`: the box is `w`×`h` either way, the picture
    /// inside it is placed by the rasteriser, which is the only place the
    /// intrinsic size is known (the pixels are looked up at paint time).
    /// `filter` is a 1-based index into `Layout::filters`, 0 for none — the
    /// pixels only exist at paint time, so the transform has to travel with
    /// the op rather than being applied to a colour here.
    Image { x: i32, y: i32, w: i32, h: i32, src: String, alt: String, fit: ObjectFit, filter: u16 },
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
        /// The painting area (`background-clip`) as `(x, y, w, h)`. `x..h` above
        /// are the POSITIONING area (`background-origin`); the two are the same
        /// rectangle only when neither property is set and the box has no
        /// border.
        clip: (i32, i32, i32, i32),
        key: u64,
        repeat: (bool, bool),
        pos: (BgPos, BgPos),
        size: BgSize,
        tint: Option<Rgba>,
        /// Same 1-based index as `Image::filter`. A MASK never uses it: it
        /// paints `tint` through the image's alpha, so the transform lands on
        /// that colour at layout time instead.
        filter: u16,
    },
}

/// Ein Flex-Kind: ein Element, oder ein ANONYMER Kasten um einen nackten
/// Textlauf (css-flexbox-1 §4).
///
/// Der anonyme traegt seinen fertigen Kasten mit sich — er hat kein Element,
/// also auch keinen Weg durch `layout_box`, und `AtomicBox` ist genau die
/// Form, die das Layout dafuer schon hat (ein `::before` ist derselbe Fall).
enum Kid<'a> {
    El(&'a Element),
    Anon(AtomicBox),
}

impl<'a> Kid<'a> {
    fn el(&self) -> Option<&'a Element> {
        match self { Kid::El(e) => Some(e), Kid::Anon(_) => None }
    }
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
        // Der weiche Rand reicht ueber den Kasten hinaus, aber er ist
        // durchsichtig und soll die Seite nicht laenger machen.
        DrawOp::Shadow { y, h, .. } => y + h,
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
fn uniform_border(st: &ComputedStyle) -> Option<(f32, Rgba)> {
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
    // Three rectangles, two of them used here: `background-clip` says where the
    // paint may land, `background-origin` where the image is anchored and what
    // a percentage size resolves against. They default DIFFERENTLY — border box
    // and padding box — so a bordered box with a centred image centres it
    // inside the border while its colour still runs under it.
    let (cx, cy, cw, ch) = st.bg_clip.shrink(st, x, y, w, h);
    let (ox, oy, ow, oh) = st.bg_origin.shrink(st, x, y, w, h);
    let clip = (cx, cy, cw, ch);
    match (st.bg, mask) {
        (Some(color), Some(key)) => out.push(DrawOp::BgImage {
            x: ox,
            y: oy,
            w: ow,
            h: oh,
            clip,
            key,
            repeat: st.mask_layer.repeat,
            pos: st.mask_layer.pos,
            size: st.mask_layer.size,
            tint: Some(color),
            filter: 0,
        }),
        (Some(color), None) => {
            if cw <= 0 || ch <= 0 {
                return;
            }
            // A corner radius is measured on the border box; clipping the
            // background inwards pulls the curve in with it by the same amount
            // (css-backgrounds-3 §5.3), never below zero.
            let r = radii_px(st, w);
            let inset = (cx - x).max(cy - y) as f32;
            let r = [
                (r[0] - inset).max(0.0),
                (r[1] - inset).max(0.0),
                (r[2] - inset).max(0.0),
                (r[3] - inset).max(0.0),
            ];
            out.push(if r.iter().any(|&v| v > 0.0) {
                DrawOp::RoundRect { x: cx, y: cy, w: cw, h: ch, r, color, ring: 0.0 }
            } else {
                DrawOp::Rect { x: cx, y: cy, w: cw, h: ch, color }
            });
        }
        // A mask with no colour to stencil paints nothing at all.
        (None, _) => {}
    }
    if let Some(key) = bg {
        out.push(DrawOp::BgImage {
            x: ox,
            y: oy,
            w: ow,
            h: oh,
            clip,
            key,
            repeat: st.bg_layer.repeat,
            pos: st.bg_layer.pos,
            size: st.bg_layer.size,
            tint: None,
            filter: 0,
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
    outline_ops(st, x, y, w, h, sides, out);
}

/// The `outline` ring (css-ui-4 §3). Unlike a border it takes NO space — it is
/// drawn outside the border box, offset outwards by `outline-offset`, and the
/// layout never sees it. That is the whole reason it exists: a focus ring has
/// to be able to appear without moving the page under the reader.
fn outline_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, sides: (bool, bool), out: &mut Vec<DrawOp>) {
    let o = &st.outline;
    let (Some(color), true) = (o.color, o.width > 0.0) else {
        return;
    };
    let (ow, off) = (o.width as i32, st.outline_offset as i32);
    // Grow the border box by the offset, then lay the ring OUTSIDE that.
    let (rx, ry) = (x - off - ow, y - off - ow);
    let (rw, rh) = (w + 2 * (off + ow), h + 2 * (off + ow));
    if rw <= 0 || rh <= 0 {
        return;
    }
    let r = radii_px(st, w);
    if r.iter().any(|&v| v > 0.0) {
        // A rounded box's outline follows its curve, widened by the ring's own
        // distance from the box (css-ui-4 §3.4).
        let grow = (off + ow) as f32;
        let rr = [r[0] + grow, r[1] + grow, r[2] + grow, r[3] + grow];
        out.push(DrawOp::RoundRect { x: rx, y: ry, w: rw, h: rh, r: rr, color, ring: o.width });
        return;
    }
    let mut edge = |ex: i32, ey: i32, ew: i32, eh: i32| {
        if ew > 0 && eh > 0 {
            out.push(DrawOp::Rect { x: ex, y: ey, w: ew, h: eh, color });
        }
    };
    edge(rx, ry, rw, ow);
    edge(rx, ry + rh - ow, rw, ow);
    // An inline box that continues onto the next line carries no side edge,
    // exactly as its border does.
    if sides.0 {
        edge(rx, ry, ow, rh);
    }
    if sides.1 {
        edge(rx + rw - ow, ry, ow, rh);
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
    /// Where this control's ops sit in `Layout::ops`, and the box they were
    /// painted from. A control's own state — focus, checked, the typed value,
    /// the caret — changes far more often than the page does, and repainting
    /// that range beats laying the document out again by three orders of
    /// magnitude ([[project-beak-pointer-and-repaint]]).
    at: usize,
    len: usize,
    paint: CtlBox,
}

pub struct Layout {
    pub ops: Vec<DrawOp>,
    /// The viewport width this was laid out at. A layout that does not say so
    /// cannot be re-read later — `repaint_hover` has to resolve a style the
    /// same way the layout did, and `vw`/media queries are part of that.
    pub width: u32,
    pub links: Vec<LinkRect>,
    pub controls: Vec<ControlRect>,
    /// Total document height (px). May exceed the viewport → scroll.
    pub height: u32,
    /// Did this layout actually depend on the viewport HEIGHT? When false, a
    /// purely vertical resize cannot move a single box, so the shell may reuse
    /// this layout and just re-clip — the difference between a repaint and a
    /// full re-layout (~6.4 s on device for a big article).
    ///
    /// Sound in the direction that matters: it over-reports (value-equality on
    /// the containing block, any matched `vh` rule), never under-reports.
    pub viewport_h_used: bool,
    /// What the three pipeline phases cost, in whatever unit the caller's
    /// clock counts (see `Engine::set_clock`). Zero when no clock is set.
    ///
    /// The device reports parse+cascade+layout as ONE number, which is exactly
    /// the number we cannot act on: a host profile says the box layout
    /// dominates, but the host is not an interpreter and the phases do not
    /// scale alike under one. Splitting it needs a clock, and the engine has
    /// no host functions by design — so the caller lends it one.
    pub phase: [u64; 3],
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
    /// Inline `<svg>` elements this layout painted, as (seq, colour, w, h).
    ///
    /// An inline SVG is a replaced element with no `src`, and it cannot be
    /// rasterised before the cascade runs: `currentColor` — what practically
    /// every icon set paints with — IS the element's computed `color`, and the
    /// box is decided by CSS, not by the SVG's own attributes. So layout states
    /// what it needs and `Engine::resolve_inline_svgs` renders it afterwards,
    /// the same split `css_image_srcs` already uses.
    pub inline_svgs: Vec<(u32, Rgb, u32, u32)>,
    /// Element boxes for the inspect dev tool (empty unless inspection was on).
    pub inspect: Vec<InspectBox>,
    /// Element boxes for pointer hit-testing (empty unless the sheet has
    /// `:hover` rules).
    pub hover_boxes: Vec<HoverBox>,
    /// The `filter` colour transforms this layout used, referenced by the
    /// 1-based index an image op carries. A side table rather than a field on
    /// the op: a `ColorFilter` is 52 bytes and `filter` is rare, so carrying
    /// one per op would roughly double the display list on every page that has
    /// no filter at all.
    pub filters: Vec<crate::color::ColorFilter>,
}

/// An element's box, for deciding what the pointer is inside.
///
/// Deliberately not `InspectBox`: that one carries a formatted label, and this
/// list exists on every page with a hover rule, not only while a developer is
/// inspecting.
#[derive(Clone, Copy)]
pub struct HoverBox {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub seq: u32,
    /// Where this element's box decoration BELONGS in the display list, named
    /// by the op that sits there rather than by an index.
    ///
    /// A background that only exists while the pointer is inside has nothing
    /// to replace — it has to be inserted, and a box inserts its decoration
    /// ahead of everything it paints. An index would be the obvious way to say
    /// where that is and the wrong one: the list is still inserted into,
    /// clipped and reordered by z after this is recorded, and every one of
    /// those moves it. Content does not move.
    ///
    /// `None` when the box painted nothing at all — then there is no "ahead of"
    /// to speak of, and a repaint hands the page to a layout.
    pub anchor: Option<OpKey>,
    /// Where this fragment's own decoration is painted, which is NOT the hit
    /// rect: an inline box's background covers its font's ascent + descent plus
    /// padding, not the line box (CSS 2.1 §10.6.1).
    pub paint: (i32, i32, i32, i32),
    /// Which of the left/right borders this fragment draws — a box broken
    /// across lines draws them only on its outer ends.
    pub sides: (bool, bool),
    /// A block box paints its `box-shadow`; an inline fragment does not.
    pub shadow: bool,
    /// Which pseudo-element this box belongs to. `None` is the element itself;
    /// a `::before`/`::after` gets its own box because a hover rule reaches it
    /// — MediaWiki underlines the article tabs with `a:hover::after`, and a
    /// repaint that had no rectangle for it could only give up.
    pub pseudo: crate::css::PseudoElem,
    /// The anchor names the op the decoration goes AFTER, not before it. An
    /// absolutely positioned pseudo is appended at the end of what its
    /// originating element painted, so what it can name is its predecessor.
    pub anchor_after: bool,
    /// Does this box paint text of its own? A pseudo's `content` string is not
    /// part of what the element SAYS, so a colour change on one cannot be
    /// repainted from the display list alone.
    pub has_text: bool,
    /// Does this box take part in `:hover`? False for one recorded only so a
    /// `<summary>` can be clicked — the pointer being inside it is not a
    /// cascade event, and reporting it would repaint on every page that has a
    /// `<details>` and no hover rule at all.
    pub hoverable: bool,
    /// Clicking this box opens/closes its `<details>`.
    ///
    /// It rides in `hover_boxes` rather than in a list of its own because this
    /// list is ALREADY carried through everything a hit rect has to survive:
    /// the rollback mark, the relative-offset shift, and the drain into an
    /// `AtomicBox`. A fourth parallel list would have to repeat all three, and
    /// the one time that was done by hand it shipped with a missing shift
    /// (0.25.0, see `shift_since`).
    pub toggle: bool,
}

/// Enough of an op to find it again: kind, position, and the two numbers that
/// tell same-shaped ops apart. Deliberately small and `Copy` — one of these
/// hangs off every hit rect on the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpKey {
    kind: u8,
    x: i32,
    y: i32,
    a: i32,
    b: i32,
}

/// The key of an op, for `HoverBox::anchor`.
fn op_key(op: &DrawOp) -> OpKey {
    match op {
        DrawOp::Text { x, y, size, text, .. } => {
            OpKey { kind: 0, x: *x, y: *y, a: size.to_bits() as i32, b: text.len() as i32 }
        }
        DrawOp::Rect { x, y, w, h, .. } => OpKey { kind: 1, x: *x, y: *y, a: *w, b: *h },
        DrawOp::RoundRect { x, y, w, h, .. } => OpKey { kind: 2, x: *x, y: *y, a: *w, b: *h },
        DrawOp::Image { x, y, w, h, .. } => OpKey { kind: 3, x: *x, y: *y, a: *w, b: *h },
        DrawOp::BgImage { x, y, w, h, .. } => OpKey { kind: 4, x: *x, y: *y, a: *w, b: *h },
        DrawOp::Shadow { x, y, w, h, .. } => OpKey { kind: 5, x: *x, y: *y, a: *w, b: *h },
    }
}

impl Layout {
    /// Does any box this layout painted for one of `srcs` reach into the
    /// vertical band `[top, bottom)` of the document?
    ///
    /// The shell asks this before repainting for an arriving `<img>`. A
    /// repaint is the WHOLE viewport — 1902x1000x4 = 7,6 MB of fill, ~50 ms
    /// on the device — and an image that landed below the fold cannot change
    /// a single visible pixel. Painting for it is all of the cost and none of
    /// the picture. Nothing is lost: scrolling marks the page dirty anyway,
    /// so the image is drawn the moment it can be seen.
    ///
    /// Answered from the display list rather than from a side table, because
    /// the display list is where an image's PLACED box lives — `img_box` only
    /// measures, and the y a repaint cares about is decided when the box is
    /// flowed. One pass per arriving batch, not per image.
    pub fn images_in_band(&self, srcs: &[&str], top: i32, bottom: i32) -> bool {
        self.ops.iter().any(|op| match op {
            DrawOp::Image { y, h, src, .. } => {
                *y < bottom && y + h > top && srcs.iter().any(|s| *s == src.as_str())
            }
            _ => false,
        })
    }

    /// As [`Self::images_in_band`], for `background-image`/`mask-image` layers.
    ///
    /// Tested against the op's CLIP rectangle, not its positioning area: the
    /// clip is what actually gets painted, and with `background-origin` or a
    /// border the two are different rectangles.
    pub fn css_images_in_band(&self, keys: &[u64], top: i32, bottom: i32) -> bool {
        self.ops.iter().any(|op| match op {
            DrawOp::BgImage { clip, key, .. } => {
                clip.1 < bottom && clip.1 + clip.3 > top && keys.contains(key)
            }
            _ => false,
        })
    }

    /// The deepest (most specific) inspect box containing a document-space
    /// point, for the inspect dev tool. Ties break toward the one recorded
    /// later (painted on top).
    pub fn hit_inspect(&self, x: i32, y: i32) -> Option<&InspectBox> {
        self.inspect
            .iter()
            .filter(|b| x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h)
            .max_by_key(|b| b.depth)
    }

    /// The `<summary>` at a document-space point, by element `seq` — the
    /// disclosure control the shell toggles. Innermost wins, so a `<details>`
    /// nested inside another one's summary opens the inner section.
    pub fn hit_toggle(&self, x: i32, y: i32) -> Option<u32> {
        self.hover_boxes
            .iter()
            .filter(|b| b.toggle && x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h)
            .max_by_key(|b| b.seq)
            .map(|b| b.seq)
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

    /// Every element the pointer is inside, at a document-space point —
    /// ascending `seq`, which is document order.
    ///
    /// It is a LIST, not the innermost element: CSS hovers an element and all
    /// its ancestors, which is what `nav:hover a` and every dropdown menu on
    /// the web relies on. Containment does that for free — an ancestor's box
    /// encloses its descendant's — without keeping a parent pointer per box.
    /// Die `seq`-Kette unter dem Punkt — ALLE Elemente, nicht nur die
    /// `:hover`-faehigen. Der Weg vom Klickpunkt zum Knoten fuer die
    /// Ereigniszustellung; braucht `Engine::set_hit_all`.
    pub fn element_chain(&self, x: i32, y: i32) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .hover_boxes
            .iter()
            .filter(|b| b.pseudo == crate::css::PseudoElem::None)
            .filter(|b| x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h)
            .map(|b| b.seq)
            .collect();
        // Ein Steuerelement hat KEINEN `hover_box`: es ist ein atomarer
        // Inline-Kasten, seine Kinder laufen nie durchs Layout, und damit
        // kommt es nie an `record_inspect` vorbei. Ohne diese Zeile endet die
        // Kette beim ELTERNTEIL — der Behandler eines `<button>` feuert nie,
        // sein `onclick`-Attribut auch nicht, und `e.target` ist der falsche
        // Knoten.
        //
        // Am Geraet sah das aus wie „der Klick kommt gar nicht an": vier
        // `control-activate` und keine einzige Zeile von der Seite. Der
        // host-seitige Selftest hatte es nicht gefunden, weil er die Kette
        // aus dem BAUM baute statt aus dem Layout — `examples/hitchk.rs`
        // schliesst genau diese Luecke.
        //
        // Bewusst hier und nicht in `record_inspect`: „darf der Zeiger diesen
        // Kasten treffen" ist nicht „reagiert dieses Element auf `:hover`".
        // Die zwei Fragen zusammenzulegen hat schon einmal sechs volle
        // Layouts je Mausbewegung gekostet ([[feedback_hitting_is_not_hovering]]).
        v.extend(self.controls.iter()
            .filter(|c| x >= c.x && x < c.x + c.w && y >= c.y && y < c.y + c.h)
            .map(|c| c.seq));
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn hover_at(&self, x: i32, y: i32) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .hover_boxes
            .iter()
            // A pseudo-element's box is recorded for repainting, not for
            // hit-testing — extending the pointer's reach would change which
            // element it is inside, which is a different question.
            .filter(|b| b.pseudo == crate::css::PseudoElem::None && b.hoverable)
            .filter(|b| x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h)
            .map(|b| b.seq)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
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
/// The characters CSS collapses (css-text-3 §4.1.1: the "white space"
/// characters are space, tab and the newlines). Rust's `char::is_whitespace`
/// is the Unicode `White_Space` property, which also covers U+00A0 and U+3000 —
/// and both of those exist precisely so they do NOT collapse or offer a break.
fn is_css_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{000C}')
}

/// A character that HANGS at the end of a line: it is painted, but it does not
/// count towards the line's width (css-text-3 §4.1.3 phase II removes a
/// trailing sequence of collapsible spaces AND other space separators). These
/// are the space separators that do not collapse — U+00A0 is deliberately not
/// among them, since a no-break space is content.
fn is_hangable_space(c: char) -> bool {
    is_css_space(c)
        || matches!(c, '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}'
            | '\u{202F}' | '\u{205F}' | '\u{3000}')
}

/// A zero-width formatting character: it is not a typographic character unit,
/// so no letter-spacing is added after it (css-text-3 §8.2).
fn is_zero_width_format(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{206A}'..='\u{206F}'
        | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{00AD}')
}

/// The extra advance `sp` adds after `c`.
pub(crate) fn char_spacing(c: char, sp: (f32, f32)) -> f32 {
    if is_zero_width_format(c) {
        return 0.0;
    }
    // Word-spacing lands on the word separators css-text-3 §8.1 names —
    // notably NOT U+3000 IDEOGRAPHIC SPACE.
    let ws = matches!(c, ' ' | '\u{00A0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}');
    sp.0 + if ws { sp.1 } else { 0.0 }
}

fn measure(font: &Font, s: &str, size: f32) -> f32 {
    s.chars().map(|c| font.metrics(c, size).advance_width).sum()
}

/// `measure` plus `(letter-spacing, word-spacing)`. Letter-spacing lands after
/// EVERY character including the last — that is what an inline box measures as
/// in every engine, and the reftests are written against it. Word-spacing lands
/// on the word separator itself (css-text-3 §8.1: U+0020 and U+00A0).
fn measure_sp(font: &Font, s: &str, size: f32, sp: (f32, f32)) -> f32 {
    if sp == (0.0, 0.0) {
        return measure(font, s, size);
    }
    s.chars().map(|c| font.metrics(c, size).advance_width + char_spacing(c, sp)).sum()
}
/// Byte length of the longest prefix of `s` that fits in `avail` px, snapped
/// back to a legal break. Returns 0 when not even the first cluster fits — the
/// caller decides whether to try a fresh line or force one through (never
/// returning 0 forever is the caller's job, not this function's).
fn fit_prefix(font: &Font, s: &str, size: f32, avail: f32, sp: (f32, f32)) -> usize {
    let mut used = 0.0;
    let mut end = s.len();
    for (i, c) in s.char_indices() {
        let adv = font.metrics(c, size).advance_width + char_spacing(c, sp);
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

/// The advance of the space BETWEEN two words. `sp` is the run's
/// `(letter-spacing, word-spacing)`: both apply to a word separator.
fn space_width(font: &Font, size: f32, sp: (f32, f32)) -> f32 {
    font.metrics(' ', size).advance_width + sp.0 + sp.1
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
    /// Fuer JEDES Element einen Treffer-Kasten aufzeichnen. Ohne das gibt es
    /// keinen Weg vom Klickpunkt zum Knoten — und nur Elemente mit einer
    /// `:hover`-Regel haetten einen.
    hit_all: bool,
    /// `src`s whose `<img>` box had to be GUESSED — no decoded pixels and no
    /// `width`/`height` pair. Only for these does a later decode move the
    /// page, so only their arrival justifies a re-layout.
    ///
    /// A plain bool here was wrong: one image that never arrives (a 403, an
    /// undecodable format) kept it true forever, so every later batch forced
    /// a full re-layout even when all of ITS images had definite boxes. On a
    /// real article that was 5.7 s of frozen UI per batch.
    guessed: core::cell::RefCell<Vec<String>>,
    /// Inline `<svg>` render requests — see `Layout::inline_svgs`.
    inline_svgs: core::cell::RefCell<Vec<(u32, Rgb, u32, u32)>>,
    /// `url_key`s of the CSS images this layout referenced. Deliberately a
    /// SET (deduped on insert), not an append-only log: a throwaway
    /// measurement layout paints boxes too, and its entries must be
    /// indistinguishable from the real pass's rather than something the
    /// measure helpers have to remember to roll back.
    css_images: core::cell::RefCell<Vec<u64>>,
    ops: Vec<DrawOp>,
    links: Vec<LinkRect>,
    controls: Vec<ControlRect>,
    /// `filter` transforms, deduped; an image op holds a 1-based index here.
    filters: Vec<crate::color::ColorFilter>,
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
    cb: PosCb,
    /// Deferred containing-block heights, indexed by `cb.4`. A stack: it is
    /// truncated when the box that pushed its recipe goes out of scope.
    cb_pend: Vec<PendingCbH<'a>>,
    /// Viewport width (px) — the layout width — for `@media` evaluation.
    viewport_w: f32,
    /// The viewport height this layout was built for — the initial containing
    /// block's height, and the basis every `vh` resolved against.
    viewport_h: f32,
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
    /// Set for the duration of ONE `layout_abs` call: an out-of-flow box was
    /// reached while a line box was still open. It has to sort above that
    /// line's ops even though it was emitted first — see `LAYER_POSITIONED`.
    abs_over_open_line: bool,
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
    /// `seq`s of the elements the pointer is currently inside, ascending.
    /// Usually EMPTY, which is why every `ElemInfo` can afford to consult it:
    /// the check is one `is_empty()` on a path walked ~30 000× per layout.
    hover: &'a [u32],
    /// Element boxes the shell hit-tests on pointer movement. Only collected
    /// when the sheet has `:hover` rules at all — a page without them must not
    /// pay for a list nobody reads.
    hover_boxes: Vec<HoverBox>,
    /// Did anything in this layout actually consume the viewport HEIGHT — a
    /// `vh`/`vmin`/`vmax` length that won the cascade, or a box resolved
    /// against the initial containing block? `Cell` because the style walk
    /// runs behind `&self`.
    vh_used: core::cell::Cell<bool>,
    /// Set while laying out a box whose definite height came from the viewport
    /// (a `%` against the ICB). Read by `flow_children` right after the box,
    /// which knows whether anything follows it to be pushed around.
    vp_height_box: core::cell::Cell<bool>,
    /// Memoised `intrinsic_width` results, keyed by element `seq`. Measuring a
    /// subtree now cascades every descendant, and the same element is asked
    /// repeatedly (a table sizes its columns over several passes) — without
    /// this the cascade work would multiply.
    intrinsic: BTreeMap<u32, (f32, f32)>,
    /// Set while `measure_box_height` is resolving a positioned box's own
    /// containing-block height. That measurement re-enters the same box, which
    /// would ask for the same height again — one level is all the answer needs.
    measuring_cb_h: core::cell::Cell<bool>,
    /// Memoised `measure_box_height` results — see `measured_h`. A speculative
    /// measurement lays a whole subtree out and throws the result away, and
    /// nested ones repeat: measuring a flex item that is itself a flex
    /// container re-measures its items, and the enclosing box is measured
    /// again for every level above it. On a real article that made the same
    /// element's box run through layout 34 times on average and 256 times at
    /// worst — powers of two, the signature of a doubling per nesting level.
    /// This collapses that back to once per distinct question.
    measured: core::cell::RefCell<BTreeMap<MeasureKey, i32>>,
    /// Memoised `style::resolve_pseudo` results — the SAME cascade work as
    /// `styles`, for the `::before`/`::after` box, and it had no cache at all.
    /// Measured under the interpreter it was 51 % of a whole layout: 62 340
    /// calls for 2 316 elements, almost all of them searching the entire sheet
    /// only to answer "this element generates nothing".
    ///
    /// Only the CASCADE result is cached. The content template is rendered
    /// fresh on every hit, because `content: counter(x)` depends on the counter
    /// state at that point in the walk, not on the element.
    pseudos: core::cell::RefCell<BTreeMap<u64, Option<(Vec<crate::style::ContentPiece>, ComputedStyle)>>>,
    /// Memoised `segment_table_runs` results. The measure walk
    /// (`intrinsic_walk`) and the layout walk (`flow_children`) segment the
    /// SAME child lists independently, and each classification cascades the
    /// child to read its `display` — measured, 82 % of the calls repeat a list
    /// already segmented, and the classification is 25 % of a whole layout.
    ///
    /// Keyed by the node slice's identity AND the ancestor chain, because the
    /// cascade that decides a role reads the chain: the same `<div>` can be a
    /// table row in one context and not in another.
    segs: core::cell::RefCell<BTreeMap<u64, Vec<(u32, u32, bool)>>>,
    /// Memoised `style::resolve` results, keyed by a hash of everything the
    /// cascade reads (see `style_key`) — so this is a pure cache, not a policy.
    /// A real article cascades the SAME element about twelve times: every
    /// throwaway measurement re-walks its subtree, and selector matching is
    /// ~90 % of layout, so that multiplier is most of the cost of a page.
    styles: core::cell::RefCell<BTreeMap<u64, ComputedStyle>>,
    /// Die Custom Properties je Element, nach `seq`.
    ///
    /// Sie stehen NICHT in `ComputedStyle`: der ist `Copy` und wird je
    /// Element kopiert; eine `Rc` darin haette die ganze Layoutschicht
    /// umgeworfen. Also laufen sie daneben — und weil eine Custom Property
    /// eine geerbte Eigenschaft ist, braucht jedes Element den Eintrag seines
    /// Elternteils. Wer selbst keine setzt, TEILT dessen Karte (`Rc`), sonst
    /// koestete Bootstraps 200-Namen-Palette je Element eine Kopie.
    varmaps: core::cell::RefCell<BTreeMap<u32, alloc::rc::Rc<crate::vars::VarMap>>>,
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
    mix(parent.color.c.0 as u64
        | (parent.color.c.1 as u64) << 8
        | (parent.color.c.2 as u64) << 16
        | (parent.color.a as u64) << 24);
    mix(parent.bold as u64 | (parent.italic as u64) << 1 | (parent.mono as u64) << 2 | (parent.rtl as u64) << 3);
    h
}

impl<'a> Ctx<'a> {
    /// An `ElemInfo` that knows whether the pointer is inside this element.
    /// Every construction inside the layout goes through here — a bare
    /// `ElemInfo::of` would silently report "not hovered" and the page would
    /// stay frozen under the pointer for exactly the elements it forgot.
    fn info(&self, el: &'a Element) -> ElemInfo<'a> {
        ElemInfo::of_hovered(el, self.hover)
    }

    /// Does a `display: contents` element hold nothing but inline-level
    /// content? Then its children belong in the line box the parent is already
    /// building, and putting them anywhere else splits a line the reference
    /// keeps whole (`P<fieldset style=display:contents>A…` is one word).
    ///
    /// If ANY child is block-level the parent's flow has to break for it
    /// regardless, and a transparent block — which is what `resolve` has
    /// already made of this style, zero margins and all — lands the same
    /// pixels while keeping the block/anonymous-block split intact.
    ///
    /// Costs one style resolve per child, paid only for `display: contents`.
    /// `styled` memoises on the same key the real walk uses, so the walk that
    /// follows reads them back out of the map.
    fn contents_is_inline(&mut self, el: &'a Element, st: &ComputedStyle) -> bool {
        self.path.push(self.info(el));
        let n = el.children.iter().filter(|c| matches!(c, Node::Element(_))).count() as u32;
        let mut sibs: Vec<ElemInfo> = Vec::new();
        let mut inline_only = true;
        for c in &el.children {
            let Node::Element(ce) = c else { continue };
            let cs = self.styled(ce, st, &sibs, n);
            sibs.push(self.info(ce));
            inline_only = match cs.display {
                Display::None | Display::Inline | Display::InlineBlock
                | Display::InlineFlex => true,
                // Nested unboxing — `details, summary { display: contents }`
                // is one element's contents inside another's.
                Display::Contents => self.contents_is_inline(ce, &cs),
                _ => false,
            };
            if !inline_only {
                break;
            }
        }
        self.path.pop();
        inline_only
    }

    /// `style::resolve` through the memo. Every cascade inside the layout goes
    /// through here so a re-measured subtree costs a map lookup, not a full
    /// selector match against the page's stylesheet.
    /// Record a viewport-HEIGHT dependency that is unconditional. A `vh` cap
    /// (`max-`/`min-height`) is NOT one: it only moves geometry when it
    /// actually clamps, which `clamp_vh` decides once the content height is
    /// known.
    fn note_vh(&self, s: &ComputedStyle) {
        if s.vh_seen & crate::style::VH_DIRECT != 0 {
            self.vh_used.set(true);
        }
    }

    /// A `vh`-derived `max-height`/`min-height` that actually changed the used
    /// height IS a viewport-height dependency; one that never binds is not.
    fn note_vh_clamp(&self, st: &ComputedStyle, before: i32, after: i32) {
        if before == after {
            return;
        }
        let bit = if after < before { crate::style::VH_MAX_HEIGHT } else { crate::style::VH_MIN_HEIGHT };
        if st.vh_seen & bit != 0 {
            self.vh_used.set(true);
        }
    }

    /// Die Custom Properties, die ein Kind von `seq` erbt.
    fn vars_of(&self, seq: u32) -> alloc::rc::Rc<crate::vars::VarMap> {
        self.varmaps.borrow().get(&seq).cloned().unwrap_or_default()
    }

    fn styled(&self, el: &Element, parent: &ComputedStyle, prev: &[ElemInfo], sib_count: u32) -> ComputedStyle {
        let key = style_key(el, parent, &self.path, prev, sib_count);
        if let Some(s) = self.styles.borrow().get(&key) {
            self.note_vh(s);
            return *s;
        }
        let inherited = match self.path.last() {
            Some(p) => self.vars_of(p.el.seq),
            None => alloc::rc::Rc::new(crate::vars::VarMap::new()),
        };
        let mut own = None;
        let s = style::resolve_in(&self.info(el), parent, self.theme, self.sheet, &self.path,
            prev, sib_count, self.viewport_w, &inherited, &mut own);
        // Wer selbst nichts setzt, teilt die Karte des Elternteils — dieselbe
        // `Rc`, kein Kopieren. Der Eintrag muss trotzdem da sein, sonst faende
        // ein Kind nichts und die Vererbung risse an dieser Stelle ab.
        self.varmaps.borrow_mut().insert(el.seq, match own {
            Some(m) => alloc::rc::Rc::new(m),
            None => inherited,
        });
        self.note_vh(&s);
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
        // A percentage height resolving against the viewport does NOT by itself
        // move anything: `html, body { height: 100% }` is on nearly every site
        // and only fixes those boxes' own bottom edge, with nothing after them.
        // What moves content is a box like that having FOLLOWING content — so
        // the box is only marked here, and `flow_children` raises the flag if
        // something actually comes after it. Compared by value, not identity:
        // an ancestor that happens to be exactly one viewport tall
        // over-reports, which only costs us today's re-layout.
        if cbh == Some(self.viewport_h as f32) {
            self.vp_height_box.set(true);
        }
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
    fn record_inspect(&mut self, el: &Element, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, op0: usize) {
        // Same call site, own switch: the pointer needs these boxes on any page
        // with a `:hover` rule, whether or not anyone is inspecting — and a
        // `<summary>` needs one whether or not the page hovers anything.
        // ZWEI Fragen, und sie zusammenzulegen war ein Fehler mit Messwert:
        // „darf der Zeiger diesen Kasten TREFFEN" ist nicht „reagiert dieses
        // Element auf `:hover`". Mit `hit_all` bekam jedes Element
        // `hoverable = true`, also galt jede Mausbewegung als Stilwechsel —
        // auf Wikipedia sechs volle Layouts a 130 ms fuer nichts.
        let hoverable = self.sheet.hover_set.may_match(el);
        if w > 0 && h > 0 && (hoverable || st.is_summary || self.hit_all) {
            let anchor = self.ops.get(op0).map(op_key);
            self.hover_boxes.push(HoverBox {
                x,
                y,
                w,
                h,
                seq: el.seq,
                anchor,
                paint: (x, y, w, h),
                sides: (true, true),
                shadow: true,
                pseudo: PseudoElem::None,
                anchor_after: false,
                has_text: false,
                hoverable,
                toggle: st.is_summary,
            });
        }
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
            label.push_str(&alloc::format!(" bg:#{:02x}{:02x}{:02x}{:02x}", bg.c.0, bg.c.1, bg.c.2, bg.a));
        }
        self.inspects.push(InspectBox { x, y, w, h, depth: self.path.len() as u16, label });
    }
}

/// A short name for a `Display` value, for the inspect label.
pub(crate) fn display_name(d: Display) -> &'static str {
    match d {
        Display::Block => "block",
        Display::Inline => "inline",
        Display::InlineBlock => "inline-block",
        Display::InlineFlex => "inline-flex",
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
/// An out-of-flow box that was emitted while a line box was still open.
///
/// Appendix E paints positioned boxes in step 8, after the in-flow inline
/// content of step 7 — but the display list is built in visit order, and a line
/// is not written until it BREAKS. So an abspos box reached mid-line lands in
/// the list ahead of text that precedes it in the document, and paints under it.
///
/// Flushing the line instead would be wrong: `foo<div style=position:absolute>
/// </div>bar` is one line, and breaking it early moves `bar`. So the box is
/// lifted over exactly that line and nothing else. Lifting positioned boxes
/// wholesale was measured twice and is worse both times: out-of-flow only gives
/// +25/-21 (`border-005` — an absolute box FIRST, a `position: relative` box
/// after it, both step 8, so document order must decide and lifting one of them
/// hands it to the loser), and every positioned box gives +16/-46.
const LAYER_POSITIONED: i32 = 2;

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
    hover: &[u32],
    hit_all: bool,
) -> Layout {
    // The root element is never painted, but `html { … }` still cascades into
    // the document — and its `font-size` is the basis for every `rem`.
    let mut initial = ComputedStyle::root(theme);
    // Seed the viewport before the first cascade: `vw`/`vh` on `html` itself
    // have to resolve, and every descendant inherits these two down.
    initial.vw = width as f32;
    initial.vh = viewport_h as f32;
    let html_el = dom.root_element();
    let (mut root_own, no_vars) = (None, crate::vars::VarMap::new());
    let mut root = style::resolve_in(&ElemInfo::of_hovered(html_el, hover), &initial, theme,
        sheet, &[], &[], 0, width as f32, &no_vars, &mut root_own);
    let root_vars = root_own.unwrap_or_default();
    root.rem_base = root.font_px;
    let cx = 0;
    let cw = (width as i32).max(60);
    let mut ctx = Ctx {
        fonts,
        theme,
        sheet,
        images,
        guessed: core::cell::RefCell::new(Vec::new()),
        inline_svgs: core::cell::RefCell::new(Vec::new()),
        css_images: core::cell::RefCell::new(Vec::new()),
        ops: Vec::new(),
        links: Vec::new(),
        controls: Vec::new(),
        filters: Vec::new(),
        forms,
        path: Vec::new(),
        // Initial containing block: the viewport, anchored at the CANVAS
        // origin (CSS2.1 §10.1) — not at the page's content box. `left: 100px`
        // on a box with no positioned ancestor means 100px from the window
        // edge, whatever inset the page content sits at. Its height is
        // definite, which is what makes `top:0; bottom:0` on a root-level
        // abspos box stretch to the window rather than collapse.
        cb: (0, 0, width as i32, Some(viewport_h as i32), None),
        cb_pend: Vec::new(),
        viewport_w: width as f32,
        viewport_h: viewport_h as f32,
        vh_used: core::cell::Cell::new(false),
        vp_height_box: core::cell::Cell::new(false),
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
        abs_over_open_line: false,
        float_depth: 0,
        marker_ord: 0,
        counters: Counters::default(),
        inspect,
        hit_all,
        inspects: Vec::new(),
        hover,
        hover_boxes: Vec::new(),
        intrinsic: BTreeMap::new(),
        measuring_cb_h: core::cell::Cell::new(false),
        measured: core::cell::RefCell::new(BTreeMap::new()),
        pseudos: core::cell::RefCell::new(BTreeMap::new()),
        segs: core::cell::RefCell::new(BTreeMap::new()),
        styles: core::cell::RefCell::new(BTreeMap::new()),
        varmaps: core::cell::RefCell::new(BTreeMap::new()),
    };
    // Die Wurzelpalette. `:root{--bs-…}` ist die Karte, aus der alles andere
    // liest — ohne diesen Eintrag erbt niemand etwas.
    ctx.varmaps.borrow_mut().insert(html_el.seq, alloc::rc::Rc::new(root_vars));

    // Resolve <body> for the canvas-background rule below; layout reaches it
    // as an ordinary child of the root.
    let body = dom.body();
    let html_info = [ctx.info(html_el)];
    let anc: &[ElemInfo] = if core::ptr::eq(html_el, body) { &[] } else { &html_info };
    // Auch der Rumpf erbt die Wurzelpalette — er wird hier fuer die
    // Leinwandfarbe aufgeloest, also ausserhalb des Baumlaufs.
    let body_inherited = ctx.vars_of(html_el.seq);
    let mut body_own = None;
    let body_style = style::resolve_in(&ctx.info(body), &root, theme, sheet, anc, &[], 0,
        width as f32, &body_inherited, &mut body_own);
    ctx.varmaps.borrow_mut().insert(body.seq, match body_own {
        Some(m) => alloc::rc::Rc::new(m),
        None => body_inherited,
    });

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
        ctx.path.push(ctx.info(body));
        y = ctx.layout_children(&body.children, &body_style, Some(body), cx, cw, 0);
    } else {
        ctx.path.push(ctx.info(html_el));
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
    // The canvas is the ground: a translucent body background has nothing
    // under it but the theme, so it is flattened here rather than at paint.
    let canvas_bg = root.bg.or(body_style.bg).map_or(theme.bg, |c| c.over(theme.bg));
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
        inline_svgs: ctx.inline_svgs.into_inner(),
        viewport_h_used: ctx.vh_used.get(),
        phase: [0; 3],
        inspect: ctx.inspects,
        hover_boxes: ctx.hover_boxes,
        filters: ctx.filters,
        width,
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
    fn avoid_floats_bfc(
        &mut self,
        // `None` for an ANONYMOUS box: there is no element to lay out twice,
        // so it keeps the first-row placement.
        el: Option<&'a Element>,
        st: &ComputedStyle,
        x: i32,
        w: i32,
        y: i32,
    ) -> (i32, i32, i32) {
        if self.floats.is_empty() {
            return (x, w, y);
        }
        let ml = st.margin_left.px(w as f32).unwrap_or(0.0).max(0.0);
        let mr = st.margin_right.px(w as f32).unwrap_or(0.0).max(0.0);
        // Outer (margin-box) width the box demands.
        let frame = st.pad_left + st.pad_right + st.border_x();
        let need = match st.width {
            // `auto` shrinks into the band — but the MARGINS and the frame do
            // not shrink with it. If they alone do not fit, the border box
            // would sit inside the float, which §9.5 forbids a BFC root, so
            // the box goes below instead. Treating auto as "always fits" put
            // a `margin-left` wide enough to clear the float straight on top
            // of it.
            Len::Auto | Len::Intrinsic(_) => Some(ceil_i32(ml + mr + frame)),
            other => other.px(w as f32).map(|v| {
                let border = if st.box_border { v } else { v + frame };
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
        // The whole BORDER BOX has to clear the floats, not just its first row
        // (CSS2.1 §9.5): a float whose top is BELOW this box's top still
        // overlaps it, and the box has no way to narrow partway down. Its
        // height is only known by laying it out, so the candidate position is
        // measured and the box dropped past whatever cuts into it. Bounded,
        // because each retry starts below one more float bottom and a page can
        // stack a lot of them.
        for _ in 0..8 {
            let Some(el) = el else { break };
            let (bl, br) = self.float_band(by, by + 1, x, x + w);
            let (bx, bw) = (bl.max(x), (br - bl).max(1));
            let h = self.measure_box_height(el, st, bx, bw, by).max(1);
            let (bl2, br2) = self.float_band(by, by + h, x, x + w);
            if bl2 <= bl && br2 >= br {
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
            Len::Intrinsic(k) => {
                let (pref, min) = self.intrinsic_width(el, st);
                let avail = (w as f32 - ml - mr - pad_border).max(0.0);
                intrinsic_size(k, pref, min, avail)
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
        self.path.push(self.info(el));
        // The float's own contents establish a new BFC — isolate its inner floats.
        let saved = core::mem::take(&mut self.floats);
        // `layout_box` re-adds margin-left + padding from `mbox_left`; passing the
        // margin-box width lets an `auto`-width child fill the shrink-to-fit box.
        let op0 = self.ops.len();
        let border_bottom = self.layout_box(el, st, mbox_left, fw, border_top);
        self.record_inspect(el, st, mbox_left + ml as i32, border_top, (fw as f32 - ml - mr) as i32, border_bottom - border_top, op0);
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
        // A viewport-derived-height child is waiting to see whether anything
        // follows it in this flow.
        let mut vp_pending = false;
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
                    let (bx, bw, byy) = self.avoid_floats_bfc(None, &anon_st, x, w, by);
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
            let mut st = self.styled(el, parent, &siblings, sib_count);
            siblings.push(self.info(el));
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
            // `display: contents` generates no box: the children go where this
            // element's box would have been. Inline-level content joins the
            // line box already open here; anything else takes the block path
            // below, where the style `resolve` stripped makes the box it
            // builds transparent — zero margins, no border, no background,
            // `width: auto` — so it neither paints nor moves its children.
            if st.display == Display::Contents {
                // `white-space: pre` is a whole-BOX path here (`layout_pre`
                // owns the element's text, newlines and all), so an unboxed
                // element would never reach it and its source line breaks
                // would collapse. The transparent block is where that path
                // still runs — and it is what a bare `pre` block renders as
                // anyway, so this loses nothing the inline route would give.
                if !st.pre && self.contents_is_inline(el, &st) {
                    self.path.push(self.info(el));
                    self.collect_inline(el, &st, None, &mut inline, x, w, anchor);
                    self.path.pop();
                    continue;
                }
                st.display = Display::Block;
            }
            // `<img>` is an atomic inline box: add it to the current inline run
            // (a lone `<img>` flows as one item → its own line; an `<img>` in an
            // `<a>`/`<span>` flows with the text). Nested imgs are handled in
            // `collect_inline`; this catches direct children of any display.
            if el.tag == "img" || el.tag == "svg" {
                // Out of flow FIRST, exactly as the control branch below does.
                // This branch matches on the TAG, so the blockification in
                // `styled` does not route an abspos image past it the way it
                // does every other replaced element — it landed on the line and
                // grew the page by its own height. Found via Wikipedia's 1×1
                // autologin pixel; a 40×40 overlay image cost 40px.
                if matches!(st.position, Position::Absolute | Position::Fixed) {
                    self.path.push(self.info(el));
                    self.abs_over_open_line = !inline.is_empty();
                    self.layout_abs(el, &st, x, anchor + open.value() as i32);
                    self.abs_over_open_line = false;
                    self.path.pop();
                    continue;
                }
                self.path.push(self.info(el));
                let svg = el.tag == "svg";
                let (iw, ih) = if svg { self.svg_box(el, &st) } else { self.img_box(el, &st) };
                let alt = svg_alt(el, svg);
                let src = if svg { svg_key(el) } else { el.attr("src").unwrap_or("").to_string() };
                let fx = self.filter_index(&st);
                inline.image(src, iw, ih, None, alt, st.hidden, st.transparent, st.object_fit, fx, self.image_deco(&st));
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
                    self.path.push(self.info(el));
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
                    self.path.push(self.info(el));
                    let ctl = self.control_box(el, &st, kind, w as f32);
                    inline.control(ctl);
                    self.path.pop();
                    continue;
                }
            }
            // `position:absolute`/`fixed` are out of flow → laid at a
            // containing-block-relative position, not advancing the flow.
            if matches!(st.position, Position::Absolute | Position::Fixed) {
                self.path.push(self.info(el));
                self.abs_over_open_line = !inline.is_empty();
                self.layout_abs(el, &st, x, anchor + open.value() as i32);
                self.abs_over_open_line = false;
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
            if matches!(st.display, Display::Inline | Display::InlineBlock | Display::InlineFlex) {
                let ib = self.inline_box_of(el, &st, w).map(|b| inline.open_box(b));
                self.path.push(self.info(el));
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
                let nb = inline.flow(self.fonts, self.theme, x, w, ly, &self.floats, parent.text_align, parent.text_align_last, parent.rtl, parent.text_indent.px(w as f32).unwrap_or(0.0), parent.line_height.px(parent.font_px).unwrap_or(0.0), &mut self.ops, &mut self.links, &mut self.controls, &mut self.inspects, &mut self.hover_boxes, &mut self.last_baseline);
                if !committed {
                    first_top = ly;
                    committed = true;
                }
                anchor = nb;
                open = Collapse::default();
                inline = Inline::new();
            }
            // `clear` introduces clearance, dropping the block below the floats
            // and separating margins. §9.5.2 measures against the box's
            // HYPOTHETICAL position — where its border top edge would sit with
            // `clear: none`, so with its own top margin already collapsed in —
            // and then clearance SETS that edge: the margin is consumed, not
            // added on top of it. Clearing against the bare anchor instead put
            // every cleared box one whole top margin too low.
            if st.clear != ClearKind::None {
                let mut hypo = open;
                hypo.add(st.margin_top);
                let own = Collapse::one(st.margin_top).value() as i32;
                let base = anchor + hypo.value() as i32;
                let cleared = self.clear_below(st.clear, base);
                if cleared > base {
                    // `flow_block_impl` re-adds the top margin to the anchor it
                    // is handed, so hand it the one that lands the border edge
                    // exactly on `cleared`.
                    anchor = cleared - own;
                    open = Collapse::default();
                }
            }
            self.path.push(self.info(el));
            let vp_mark = self.vp_height_box.replace(false);
            let m0 = self.spec_mark();
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
                let (bx, bw, byy) = self.avoid_floats_bfc(Some(el), &st, x, w, by);
                let saved = core::mem::take(&mut self.floats);
                let op0 = self.ops.len();
                let bottom = self.layout_box(el, &st, bx, bw, byy);
                self.record_inspect(el, &st, bx, byy, bw, bottom - byy, op0);
                self.floats = saved;
                BoxOut { bottom, top_y: byy, open: Collapse::one(st.margin_bottom), through: false, box_x: bx, box_w: bw }
            } else {
                let op0 = self.ops.len();
                let o = self.flow_block_impl(el, &st, x, w, anchor, open, false);
                if !o.through {
                    // The box's OWN border box. Reporting the containing
                    // block's `x`/`w` here made every device report about a
                    // centred or max-width container wrong: MediaWiki's
                    // `.mw-page-container` (max-width 99.75rem, margin 0 auto)
                    // paints 1596 px wide at x=162 and was reported as
                    // 1920 wide at x=0.
                    self.record_inspect(el, &st, o.box_x, o.top_y, o.box_w, o.bottom - o.top_y, op0);
                }
                o
            };
            if track {
                self.stack_depth -= 1;
            }
            // This child's height came from the viewport. That only MOVES
            // anything if content follows it in this flow — `html, body
            // { height: 100% }` has nothing after it, a mid-page `height: 50vh`
            // banner has everything after it.
            if self.vp_height_box.replace(vp_mark) && !out.through {
                vp_pending = true;
            } else if vp_pending {
                // Something committed after such a box: its bottom edge, and so
                // this content's position, tracks the viewport height.
                self.vh_used.set(true);
                vp_pending = false;
            }
            // `position:relative` stays in flow but its paint shifts by top/left.
            if st.position == Position::Relative {
                let (dx, dy) = rel_offset(&st, w as f32);
                if dx != 0 || dy != 0 {
                    self.shift_ops(&m0, dx, dy);
                }
            }
            // `transform: translate(...)` — the same paint-time shift, but its
            // percentages are of the BOX, not the containing block.
            let (tdx, tdy) = translate_offset(&st, out.box_w, out.bottom - out.top_y);
            if tdx != 0 || tdy != 0 {
                self.shift_ops(&m0, tdx, tdy);
            }
            if track {
                if let ZIndex::Value(z) = st.z_index {
                    self.record_stack_entry(z, LAYER_IN_FLOW, m0.ops, self.ops.len(), m0.links, self.links.len());
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
        // A generated `::after` carrying `clear` is BLOCK-level: the open line
        // closes before it and it takes clearance like any other block. This is
        // the clearfix idiom — `.cw::after { content: ""; display: block;
        // clear: both }` — how a very large part of the real web makes a
        // container contain its floats. The box is zero-sized by definition, so
        // `pseudo_box` below drops it; what matters is that the content edge
        // follows it down past the floats. On a line box `clear` means nothing.
        if let Some(clear) = owner.and_then(|o| self.pseudo_clear(o, parent, PseudoElem::After)) {
            if !inline.is_empty() {
                let ly = anchor + open.value() as i32;
                let nb = inline.flow(self.fonts, self.theme, x, w, ly, &self.floats, parent.text_align, parent.text_align_last, parent.rtl, parent.text_indent.px(w as f32).unwrap_or(0.0), parent.line_height.px(parent.font_px).unwrap_or(0.0), &mut self.ops, &mut self.links, &mut self.controls, &mut self.inspects, &mut self.hover_boxes, &mut self.last_baseline);
                if !committed {
                    first_top = ly;
                    committed = true;
                }
                anchor = nb;
                open = Collapse::default();
                inline = Inline::new();
            }
            let base = anchor + open.value() as i32;
            // Clearance stops the top margin collapsing through, so the
            // container's border box stays where the flow put it — the cleared
            // box adds HEIGHT below, it does not drag the whole container down.
            if !committed {
                first_top = base;
                committed = true;
            }
            let cleared = self.clear_below(clear, base);
            if cleared > base {
                anchor = cleared;
                open = Collapse::default();
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
            let nb = inline.flow(self.fonts, self.theme, x, w, ly, &self.floats, parent.text_align, parent.text_align_last, parent.rtl, parent.text_indent.px(w as f32).unwrap_or(0.0), parent.line_height.px(parent.font_px).unwrap_or(0.0), &mut self.ops, &mut self.links, &mut self.controls, &mut self.inspects, &mut self.hover_boxes, &mut self.last_baseline);
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

    /// The `clear` on `owner`'s generated box, when it has one that takes part
    /// in the flow. `None` for a text-only pseudo (`clear` needs a block box),
    /// an out-of-flow one (it clears nothing) or no generated box at all.
    fn pseudo_clear(&self, owner: &Element, own: &ComputedStyle, kind: PseudoElem) -> Option<ClearKind> {
        let (_, ps) = self.pseudo_content(owner, own, kind)?;
        (ps.clear != ClearKind::None
            && ps.is_generated_box()
            && !matches!(ps.position, Position::Absolute | Position::Fixed))
        .then_some(ps.clear)
    }

    fn pseudo_content(&self, owner: &Element, own: &ComputedStyle, kind: PseudoElem) -> Option<(String, ComputedStyle)> {
        let anc = self.path.len().saturating_sub(1);
        // Same inputs the cascade reads, plus which pseudo-element is asked
        // about — `prev`/`sib_count` are constant here, so they add nothing.
        let key = style_key(owner, own, &self.path[..anc], &[], 0) ^ ((kind as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        if let Some(hit) = self.pseudos.borrow().get(&key) {
            let (template, ps) = hit.as_ref()?;
            return Some((self.render_content(owner, template), *ps));
        }
        let got =
            style::resolve_pseudo(&self.info(owner), own, self.theme, self.sheet, &self.path[..anc], &[], 0, self.viewport_w, kind);
        self.pseudos.borrow_mut().insert(key, got.clone());
        let (template, ps) = got?;
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
            let aw = pw as f32;
            let frame_x = ps.pad_left + ps.pad_right + ps.border_x();
            let frame_y = ps.pad_top + ps.pad_bottom + ps.border_y();
            let font = self.fonts.pick(ps.bold, ps.italic, ps.mono);
            let cw = match ps.width.px(aw) {
                Some(v) if v >= 0.0 => v,
                _ => measure_sp(font, text.trim(), ps.font_px, (ps.letter_spacing, ps.word_spacing)),
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
            // A pointer rule can reach this box (`a:hover::after` is how
            // MediaWiki underlines the article tabs), and repainting it needs
            // a rectangle. It goes in AFTER whatever the element has painted
            // so far — which, when the box paints nothing at rest, is the only
            // thing there is to name it by.
            if self.sheet.hover_set.may_match(el) {
                let anchor = self.ops.last().map(op_key);
                self.hover_boxes.push(HoverBox {
                    x,
                    y,
                    w,
                    h,
                    seq: el.seq,
                    anchor,
                    paint: (x, y, w, h),
                    sides: (true, true),
                    shadow: false,
                    pseudo: kind,
                    anchor_after: true,
                    has_text: !text.trim().is_empty(),
                    hoverable: true,
                    toggle: false,
                });
            }
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
                    sp: (ps.letter_spacing, ps.word_spacing),
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
    /// Die eigenen Breiten eines Flex-Kindes. Ein anonymer Kasten hat sie
    /// schon — er wurde beim Sammeln gemessen.
    fn kid_intrinsic(&mut self, kid: &Kid<'a>, s: &ComputedStyle) -> (f32, f32) {
        match kid {
            Kid::El(e) => self.intrinsic_width(e, s),
            Kid::Anon(b) => (b.w as f32, b.w as f32),
        }
    }

    /// Einen fertigen Kasten an seinen Platz legen.
    fn place_atomic(&mut self, b: &AtomicBox, x: i32, y: i32) {
        let mut ops = b.ops.clone();
        translate_op_list(&mut ops, x, y);
        self.ops.extend(ops);
    }

    /// Der anonyme Kasten um einen nackten Textlauf in einem Flex-Container.
    ///
    /// Einzeilig, wie der Kasten eines `::before` auch: die Breite ist die
    /// gemessene Textbreite, die Hoehe eine Zeile. Ein anonymer Kasten, der
    /// UMBRICHT, braeuchte die volle Inline-Maschinerie und damit ein
    /// Element, das es hier nicht gibt — benannt statt still.
    fn anon_text_box(&mut self, text: &str, st: &ComputedStyle, avail_w: i32) -> Option<AtomicBox> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        let font = self.fonts.pick(st.bold, st.italic, st.mono);
        let sp = (st.letter_spacing, st.word_spacing);
        let w = measure_sp(font, t, st.font_px, sp).min(avail_w as f32).max(1.0);
        let h = line_gap(font, st.font_px).max(1.0);
        // Der Text sitzt an der Oberkante des Kastens, ohne eigenen
        // Durchschuss.
        //
        // Benannt, weil es nicht ganz stimmt: ein Nachbar-Element setzt seine
        // erste Zeile mit halbem Durchschuss, und in einer Schrift, deren
        // Zeilenabstand groesser ist als ihre Groesse, steht der anonyme Lauf
        // dadurch bis zu zwei Pixel hoeher. Ein fester Ausgleich waere
        // geraten: er stimmte in einer Schrift und waere in der naechsten
        // wieder daneben. Richtig ist, die erste Zeile durch dieselbe
        // Inline-Maschinerie zu legen wie ein Element — und die braucht ein
        // Element, das es hier nicht gibt.
        let lead = 0.0f32;
        let ops = alloc::vec![DrawOp::Text {
            x: 0,
            y: ceil_i32(lead),
            size: st.font_px,
            color: st.color,
            bold: st.bold,
            italic: st.italic,
            mono: st.mono,
            sp,
            text: alloc::string::String::from(t),
        }];
        Some(AtomicBox {
            ops,
            links: Vec::new(),
            controls: Vec::new(),
            inspects: Vec::new(),
            hover_boxes: Vec::new(),
            w: ceil_i32(w),
            h: ceil_i32(h),
            baseline: ceil_i32(h),
            valign: st.valign,
        })
    }

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
            _ => measure_sp(font, text.trim(), ps.font_px, (ps.letter_spacing, ps.word_spacing)),
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
                sp: (ps.letter_spacing, ps.word_spacing),
                text: text.trim().into(),
            });
        }
        let h = bh + (ps.margin_top + ps.margin_bottom) as i32;
        Some(AtomicBox {
            ops,
            links: Vec::new(),
            controls: Vec::new(),
            inspects: Vec::new(),
            hover_boxes: Vec::new(),
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
        // NOTE: an intrinsic keyword on an IN-FLOW block's `width` is left on
        // the `auto` path deliberately. Sizing it from `intrinsic_width` was
        // built and MEASURED: +12/−12 and three references that stopped
        // rendering. `intrinsic_width` walks children as block content, so on
        // a grid, flex or table box it answers about the wrong formatting
        // context — and `width: fit-content` on a `display:grid` wrapper is
        // how several grid REFERENCES frame themselves, which turns the error
        // into losses in families that have nothing to do with sizing.
        // Restricting it by the box's own display did not help, because the
        // keyword usually sits on a plain wrapper AROUND such a box. This
        // wants a display-aware intrinsic measurement, not a special case.
        // The out-of-flow, float, flex-item and grid-item paths DO honour it —
        // those measure the box themselves.
        let aspect = with_aspect_height(st, cw);
        let st = aspect.as_ref().unwrap_or(st);
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
                self.ops.push(DrawOp::Rect { x: content_x, y: y + 1, w: content_w.max(1), h: 1, color: self.theme.rule.into() });
            }
            return BoxOut { bottom: y + 3 + pb, top_y: prov_top_y, open: Collapse::one(if isolated { 0.0 } else { st.margin_bottom }), through: false, box_x: box_left, box_w };
        }
        // The `display:list-item` marker box, outside the content edge.
        // `list-style-type:none` generates none at all — Wikipedia's nav/TOC
        // lists rely on that, and a bullet there is pure noise.
        if st.display == Display::ListItem && st.list_style != ListStyle::None && !st.hidden && !st.transparent {
            let top = prov_top_y + bt + pt;
            if st.list_style.is_disclosure() {
                // A triangle out of rows of `Rect`, not a glyph: the subsetted
                // Inter faces carry no U+25B8/U+25BE, so a text marker would
                // paint nothing on exactly the pages that need it. `n` steps
                // give a 2n-1 wide, n tall triangle.
                let n = ((st.font_px * 0.30) as i32).clamp(3, 7);
                // Both orientations centre on the same point, so the marker
                // does not jump sideways when the section is opened.
                let (cx, cy) = (content_x - 12 + n / 2, top + (st.font_px * 0.55) as i32);
                let open = st.list_style == ListStyle::DisclosureOpen;
                let (x0, y0) = if open {
                    (cx - (2 * n - 1) / 2, cy - n / 2)
                } else {
                    (cx - n / 2, cy - (2 * n - 1) / 2)
                };
                let c = self.theme.muted.into();
                for i in 0..n {
                    let (x, y, w, h) = if open {
                        // Pointing down: rows narrowing towards the tip.
                        (x0 + i, y0 + i, 2 * (n - i) - 1, 1)
                    } else {
                        // Pointing right: columns shortening towards the tip.
                        (x0 + i, y0 + i, 1, 2 * (n - i) - 1)
                    };
                    self.ops.push(DrawOp::Rect { x, y, w, h, color: c });
                }
            } else if st.list_style.is_bullet() {
                // Die FORM ist der Wert dieser Eigenschaft: `disc` ist eine
                // gefuellte Scheibe, `circle` ein Ring, `square` ein Quadrat.
                // Alle drei als Quadrat zu malen macht sie ununterscheidbar —
                // und eine verschachtelte Liste, die ihre Ebenen genau darueber
                // auseinanderhaelt, sieht dann auf jeder Ebene gleich aus.
                //
                // Die Groesse folgt der Schrift (Browser nehmen rund ein
                // Drittel der Schriftgroesse), damit der Punkt in einer kleinen
                // Liste nicht klobig und in einer grossen nicht verloren wirkt.
                let s = ((st.font_px * 0.33) as i32).clamp(4, 9);
                let (x, y) = (content_x - 12, top + (st.font_px * 0.5) as i32);
                let color = self.theme.muted.into();
                match st.list_style {
                    ListStyle::Square => {
                        self.ops.push(DrawOp::Rect { x, y, w: s, h: s, color });
                    }
                    // `circle` ist hohl — ein Ring von einem Pixel.
                    ListStyle::Circle => {
                        self.ops.push(DrawOp::RoundRect {
                            x, y, w: s, h: s, r: [s as f32 / 2.0; 4], color, ring: 1.0,
                        });
                    }
                    _ => {
                        self.ops.push(DrawOp::RoundRect {
                            x, y, w: s, h: s, r: [s as f32 / 2.0; 4], color, ring: 0.0,
                        });
                    }
                }
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
                    sp: (0.0, 0.0),
                    text: label,
                });
            }
        }

        // A positioned block becomes the containing block for `absolute`
        // descendants — its PADDING box (§10.1). `prov_top_y` is the border-box
        // top, so the padding edge is one border down.
        let prev_cb = self.cb;
        let prev_pend = self.cb_pend.len();
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
                cb.4 = Some(self.cb_pend.len() as u32);
                self.cb_pend.push(PendingCbH {
                    el,
                    st: *st,
                    x: content_x,
                    w: content_w,
                    y: prov_top_y,
                    border_y: st.border_y() as i32,
                    path_len: self.path.len(),
                    cb: prev_cb,
                    cb_h: self.cb_h,
                    floats: self.floats.clone(),
                    resolved: None,
                });
            }
            self.cb = cb;
        }
        // This box's own content height is what a percentage height on a CHILD
        // resolves against — and only when it is definite. An `auto` height
        // depends on those very children, so it stays indefinite and their
        // percentages fall back to `auto` (§10.5).
        let prev_cb_h = self.cb_h;
        self.cb_h = content_height_of(st, st.height);
        // Floats already active here belong to an enclosing formatting context;
        // anything added below is this box's own (§10.6.7, resolved after the
        // children).
        let float0 = self.floats.len();
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
        // The recipes pushed inside this box can no longer be reached: every
        // `cb` that referred to one has been restored past it.
        self.cb_pend.truncate(prev_pend);

        // Resolve the border-box top: when the top margin collapsed through, the
        // box's border box sits at the first committed child's border-box top.
        let border_top_y = if collapse_top && flow.committed { flow.first_top } else { prov_top_y };
        let content_top = border_top_y + bt + pt;

        // §10.6.7: a box that establishes a block formatting context and has an
        // auto height grows to contain its own floats. A box that does NOT
        // establish one never does — its floats escape to the enclosing
        // context, which is exactly why a bare `<div>` around a float measures
        // zero and an `overflow:hidden` wrapper must not. `isolated` marks the
        // BFC roots the caller positions itself (float, cell, flex item,
        // inline-block, abspos, root); `establishes_bfc` the in-flow ones.
        let auto_height = !matches!(st.height, Len::Px(_));
        let float_bottom = (auto_height && (isolated || establishes_bfc(st)))
            .then(|| self.floats[float0..].iter().map(|f| f.bottom).max())
            .flatten();

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

        // Size containment takes this branch even with content in it: under
        // `contain: size` the content contributes NO size (css-contain-2 §3.1),
        // so the box is measured exactly as if it were empty — the content
        // still paints, it just overflows.
        if !flow.committed || st.contain_size {
            // No in-flow content. Its explicit box height, if any.
            let mut ch = 0;
            if let Some(h) = px_h(st.height) {
                ch = h;
            } else if st.contain_size {
                if let Some((_, ih)) = st.contain_intrinsic {
                    ch = ih as i32;
                }
            } else if let Some((_, ih)) = replaced_intrinsic(el) {
                // §10.6.2: `height: auto` on a replaced element is its
                // intrinsic height, not the zero its (unrendered) content says.
                ch = ih as i32;
            }
            // A container whose ONLY content is a float: no line box ever
            // committed, so the float alone decides the height. A contained
            // box takes nothing from its floats either.
            if let (Some(fb), false) = (float_bottom, st.contain_size) {
                ch = ch.max(fb - content_top);
            }
            if let Some(mn) = px_h(st.min_height) {
                let was = ch; ch = ch.max(mn); self.note_vh_clamp(st, was, ch);
            }
            if let Some(mx) = px_h(st.max_height) {
                let was = ch; ch = ch.min(mx); self.note_vh_clamp(st, was, ch);
            }
            // A box with no content, border, padding or height collapses through:
            // its top and bottom margins are adjoining.
            // Collapsing through needs the box to be genuinely empty; a
            // contained box holds content and must not let margins meet.
            if collapse_top && bb == 0 && pb == 0 && ch == 0 && !flow.committed {
                let mut open = top;
                open.merge(flow.open);
                open.add(st.margin_bottom);
                return BoxOut { bottom: base_y, top_y: base_y, open, through: true, box_x: box_left, box_w };
            }
            let box_bottom = border_top_y + bt + pt + ch + pb + bb;
            self.clip_overflow(st, clip_marks, box_left, border_top_y, box_w, box_bottom - border_top_y);
            self.paint_box_decoration(st, box_left, border_top_y, box_w, box_bottom - border_top_y, bg_idx);
            self.apply_filter(st, bg_idx);
            return BoxOut { bottom: box_bottom, top_y: border_top_y, open: out_bottom_margin, through: false, box_x: box_left, box_w };
        }

        // Box with committed content. The last child's trailing margin
        // (`flow.open`) collapses with this box's bottom margin only when the
        // box has auto height and no bottom border/padding separating them.
        let collapse_bottom = !isolated && bb == 0 && pb == 0 && auto_height;
        let mut ch = (flow.bottom - content_top).max(0);
        // A float reaching past the last line box extends the content edge.
        if let Some(fb) = float_bottom {
            ch = ch.max(fb - content_top);
        }
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
            if let Some(fb) = float_bottom {
                ch = ch.max(fb - content_top);
            }
            if let Some(h) = px_h(st.height) {
                ch = h;
            }
            if let Some(mn) = px_h(st.min_height) {
                let was = ch; ch = ch.max(mn); self.note_vh_clamp(st, was, ch);
            }
            if let Some(mx) = px_h(st.max_height) {
                let was = ch; ch = ch.min(mx); self.note_vh_clamp(st, was, ch);
            }
            out_open = out_bottom_margin;
        }
        let box_bottom = content_top + ch + pb + bb;
        self.clip_overflow(st, clip_marks, box_left, border_top_y, box_w, box_bottom - border_top_y);
        self.paint_box_decoration(st, box_left, border_top_y, box_w, box_bottom - border_top_y, bg_idx);
        self.apply_filter(st, bg_idx);
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
    /// The box of an inline `<svg>`, and the render request that fills it.
    ///
    /// Unlike an `<img>` this never has to be guessed: the intrinsic size is in
    /// the markup (`width`/`height`, else the `viewBox`, else CSS's 300×150
    /// default for a replaced element with no intrinsic size), so the box is
    /// definite on the FIRST layout and arriving pixels only need a repaint.
    fn svg_box(&self, el: &Element, st: &ComputedStyle) -> (i32, i32) {
        let attr = |n: &str| el.attr(n).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
        let vb = el.attr("viewBox").and_then(|v| {
            let n: Vec<f32> = v.split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter(|t| !t.is_empty())
                .filter_map(|t| t.parse::<f32>().ok())
                .collect();
            (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| (n[2], n[3]))
        });
        let css = |l: Len| match l {
            Len::Px(v) if v >= 0.0 => Some(v),
            _ => None,
        };
        let (aw, ah) = (css(st.width).or_else(|| attr("width")), css(st.height).or_else(|| attr("height")));
        let (iw, ih) = vb.unwrap_or((300.0, 150.0));
        // One given side keeps the intrinsic ratio, as for any replaced element.
        let (w, h) = match (aw, ah) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, w * ih / iw),
            (None, Some(h)) => (h * iw / ih, h),
            (None, None) => (iw, ih),
        };
        let (w, h) = (w.max(0.0) as i32, h.max(0.0) as i32);
        if w > 0 && h > 0 {
            self.inline_svgs
                .borrow_mut()
                .push((el.seq, st.color.c, w as u32, h as u32));
        }
        (w, h)
    }

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
    fn control_box(&mut self, el: &Element, st: &ComputedStyle, kind: ControlKind, avail: f32) -> CtlBox {
        // `&mut` only so a percentage height can ask for the containing
        // block's height, which may still be a deferred recipe. The common
        // case never asks — resolving would lay a whole box out.
        let pct = |l: Len| matches!(l, Len::Pct(_) | Len::Calc { .. });
        let cbh = if pct(st.height) || pct(st.min_height) || pct(st.max_height) {
            self.cb_height()
        } else {
            self.cb.3
        };
        let font = self.fonts.pick(st.bold, st.italic, st.mono);
        let size = st.font_px;
        let ch_w = measure(font, "0", size).max(1.0);
        // Die Zeilenhoehe, die die SEITE gesetzt hat, sonst die der Schrift.
        // Ohne das war jedes Feld so hoch wie sein Schriftbild: Bootstrap gibt
        // `.form-control` `line-height: 1.5`, und ein Feld, das 24 statt 14 px
        // Zeile bekommt, ist am Ende 38 statt 28 px hoch — der Unterschied
        // zwischen „sieht komisch aus" und „sieht aus wie im Browser".
        let line = st.line_height.px(size).filter(|v| *v > 0.0).unwrap_or_else(|| line_gap(font, size));
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
        // **Eine Polsterung, zwei Benutzer.** Die Breite wurde mit
        // `CTL_PAD_X + 4` gerechnet, gemalt wurde mit `max(CSS, CTL_PAD_X)` —
        // und sobald eine Seite ihre Knoepfe selbst polstert (Bootstrap gibt
        // `.btn` 16 px), war der Kasten zu schmal fuer seine eigene
        // Beschriftung. `Gross` wurde als `ross` gemalt, weil der Maler die zu
        // lange Zeichenkette vorne abschnitt. Beide Seiten lesen jetzt
        // dieselben zwei Zahlen ([[feedback_intrinsic_shared_path]]).
        //
        // Die UA-Untergrenze bleibt: ein Knopf ohne eigene Polsterung soll
        // nicht am Text kleben, und `+ 4` ist, was er dafuer immer hatte.
        let ua_min = if kind.is_submit() || kind == ControlKind::File {
            CTL_PAD_X + 4
        } else {
            CTL_PAD_X
        };
        let pad_l = (st.pad_left as i32).max(ua_min);
        let pad_r = (st.pad_right as i32).max(ua_min);
        // Senkrecht dasselbe: `.form-control` bringt `padding: .375rem .75rem`
        // mit, und ohne sie steht ein Feld 6 px zu flach in seiner Zeile.
        let pad_t = (st.pad_top as i32).max(CTL_PAD_Y);
        let pad_b = (st.pad_bottom as i32).max(CTL_PAD_Y);
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
                    (cols * ch_w) as i32 + pad_l + pad_r + bx,
                    (rows * line) as i32 + pad_t + pad_b + by,
                )
            }
            ControlKind::Text | ControlKind::Password => {
                let cols = el.attr("size").and_then(|c| c.trim().parse::<f32>().ok()).unwrap_or(20.0);
                (
                    (cols * ch_w) as i32 + pad_l + pad_r + bx,
                    ceil_i32(line) + pad_t + pad_b + by,
                )
            }
            ControlKind::Select => (
                ceil_i32(measure(font, &text, size)) + pad_l + pad_r + CTL_ARROW + bx,
                ceil_i32(line) + pad_t + pad_b + by,
            ),
            _ => (
                ceil_i32(measure(font, &text, size)) + pad_l + pad_r + bx,
                ceil_i32(line) + pad_t + pad_b + by,
            ),
        };
        if let Some(cw) = st.width.px(avail) {
            // A CSS width is a content width unless `box-sizing: border-box`.
            w = if st.box_border { cw as i32 } else { cw as i32 + pad_l + pad_r + bx };
        }
        // A percentage height resolves against the containing block's HEIGHT
        // (§10.5), never `avail` (its width) — the checkbox-hack overlay is
        // `width:100%; height:100%`, and measuring its height off the width
        // made it as tall as its container is wide. An indefinite CB height
        // leaves the percentage unresolvable, so the intrinsic height stands.
        if let Some(chh) = vert_len(st.height, cbh) {
            h = if st.box_border { chh as i32 } else { chh as i32 + pad_t + pad_b + by };
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
        if let Some(mn) = vert_len(st.min_height, cbh) {
            h = h.max(if st.box_border { mn as i32 } else { mn as i32 + 2 * CTL_PAD_Y + by });
        }
        if let Some(mx) = vert_len(st.max_height, cbh) {
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
            placeholder: el.attr("placeholder").unwrap_or("").to_string(),
            checked: self.forms.checked_or(el.seq, el.attr("checked").is_some()),
            focused,
            caret,
            bg: st.bg,
            // Eine Seite, die dem Steuerelement einen Hintergrund gibt — auch
            // `transparent` —, malt seine Flaeche selbst. So macht es jeder
            // Browser, und Bootstraps `.btn-outline-*` verlaesst sich darauf.
            appearance_none: st.appearance_none || (st.bg_set && st.bg.is_none()),
            bg_img: self.bg_key(st.bg_layer.image).map(|k| (k, st.bg_layer)),
            pad_l,
            pad_r,
            border,
            radius: radii_px(st, w.max(8)),
            style: RunStyle { hidden: st.hidden, transparent: st.transparent, size, color: st.color, bold: st.bold, italic: st.italic, mono: st.mono, valign: crate::style::VAlign::Baseline, deco: st.deco, deco_color: st.deco_color, break_word: st.break_word, nowrap: st.nowrap, lh: st.line_height.px(size).unwrap_or(0.0), sp: (st.letter_spacing, st.word_spacing) },
        }
    }

    /// Lay a `position:absolute`/`fixed` box, out of flow, at a position derived
    /// from the containing block (`self.cb`) + `top`/`right`/`bottom`/`left`.
    /// The element is `el`, already pushed onto `self.path` by the caller.
    fn layout_abs(&mut self, el: &'a Element, st: &ComputedStyle, static_x: i32, static_y: i32) {
        // Read FIRST. Resolving the containing block below can lay this very
        // box out speculatively and roll it back, and that pass would otherwise
        // consume the flag and leave the real pass with nothing.
        let over_line = core::mem::take(&mut self.abs_over_open_line);
        if st.position == Position::Fixed {
            self.fixed_count += 1;
        } else {
            self.abs_count += 1;
        }
        // The containing block's height may still be a deferred recipe — ask
        // for it before anything reads it (§10.1).
        let cbh = self.cb_height();
        let (cbx, cby, cbw, ..) = self.cb;
        // An out-of-flow box against the INITIAL containing block moves with
        // the viewport height only if it actually reads that height: `bottom`
        // anchors it to the far edge, and a percentage `top`/`height` scales
        // with it. A box placed by `top`/`left` alone does not care how tall
        // the viewport is.
        if cbh == Some(self.viewport_h as i32)
            && (st.bottom != Len::Auto
                || matches!(st.top, Len::Pct(_) | Len::Calc { .. })
                || matches!(st.height, Len::Pct(_) | Len::Calc { .. }))
        {
            self.vh_used.set(true);
        }
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
        // `layout_box` is handed the BORDER-box top, so the vertical margins
        // belong here: §10.6.4 puts the box at `top + margin-top` below the
        // containing block's edge, and the static position is where its MARGIN
        // box would have sat. Leaving them out placed an absolutely positioned
        // box its own `margin-top` too high — visible the moment a page uses
        // `margin` instead of `top` to nudge an overlay.
        let mt = st.margin_top;
        let mb = st.margin_bottom;
        let (py, shift_to_bottom) = match (top, bottom, cbh) {
            (Some(t), _, _) => (cby as f32 + t + mt, None),
            (None, Some(b), Some(h)) => (static_y as f32 + mt, Some(cby as f32 + h as f32 - b - mb)),
            _ => (static_y as f32 + mt, None),
        };
        // layout_box → layout_block re-establishes the CB for its own children.
        let w_i = width.max(1.0) as i32;
        let m0 = self.spec_mark();
        let start = m0.ops;
        // An explicit `z-index` on this positioned box opens a tracked
        // stacking range for it (CSS2.1 §9.9) — unless it's already nested
        // inside another tracked range, which absorbs it instead. A box
        // reached mid-line opens one too, to climb over that line.
        let track = (self.should_track_stack(st) || over_line) && self.stack_depth == 0;
        if track {
            self.stack_depth += 1;
        }
        let box_bottom = self.layout_box(el, st, px as i32, w_i, py as i32);
        // A replaced element out of flow still has to be PAINTED. `layout_box`
        // gives it a rectangle — borders, background, the space it occupies —
        // but the picture itself is only ever emitted by the inline path, so
        // routing an abspos `<img>` here left an empty box behind. The render
        // gate is what said so: `tailwind` lost seven draw ops at unchanged
        // height the moment images stopped riding on the line.
        //
        // The size comes from `img_box`, not from `w_i`: a positioned replaced
        // element with `width: auto` takes its INTRINSIC width (§10.3.7), while
        // `w_i` is what a non-replaced block would have stretched to.
        if el.tag == "img" || el.tag == "svg" {
            let svg = el.tag == "svg";
            let (iw, ih) = if svg { self.svg_box(el, st) } else { self.img_box(el, st) };
            // No `hidden`/`transparent` guard: the inline path emits the op
            // either way and lets paint decide, and dropping it here made the
            // op counts disagree between the two paths for the same picture.
            if iw > 0 && ih > 0 {
                let src = if svg { svg_key(el) } else { el.attr("src").unwrap_or("").to_string() };
                let (alt, fit, filter) = (svg_alt(el, svg), st.object_fit, self.filter_index(st));
                self.ops.push(DrawOp::Image {
                    x: px as i32 + (st.border_left.width + st.pad_left) as i32,
                    y: py as i32 + (st.border_top.width + st.pad_top) as i32,
                    w: iw,
                    h: ih,
                    src,
                    alt,
                    fit,
                    filter,
                });
            }
        }
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
                self.shift_ops(&m0, 0, dy);
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
            self.shift_ops(&m0, tdx, tdy);
        }
        if track {
            let (z, layer) = match st.z_index {
                ZIndex::Value(z) => (z, LAYER_IN_FLOW),
                _ => (0, LAYER_POSITIONED),
            };
            self.record_stack_entry(z, layer, m0.ops, self.ops.len(), m0.links, self.links.len());
        }
        // The out-of-flow box, at its final (post-bottom-shift) position.
        let dy = bottom - box_bottom;
        self.record_inspect(el, st, px as i32, py as i32 + dy, w_i, box_bottom - py as i32, m0.ops);
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
        if (!st.overflow_x.clips() && !st.overflow_y.clips()) || start >= self.ops.len() {
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
        // An axis that does not clip is given the whole plane, so one call
        // covers `overflow-x: hidden; overflow-y: auto` without a second path.
        let (cl, cr) = if st.overflow_x.clips() {
            (box_left + st.border_left.width as i32, box_left + box_w - st.border_right.width as i32)
        } else {
            (i32::MIN / 4, i32::MAX / 4)
        };
        let (ct, cb) = if st.overflow_y.clips() {
            (box_top + st.border_top.width as i32, box_top + box_h - st.border_bottom.width as i32)
        } else {
            (i32::MIN / 4, i32::MAX / 4)
        };
        if st.ellipsis && st.overflow_x.clips() {
            self.ellipsize(start, cr);
        }
        clip_ops(&mut self.ops, start, cl, ct, cr, cb);
    }

    /// `text-overflow: ellipsis` — a text run that would cross the box's right
    /// clip edge is cut back and ends in `…` instead (css-ui-4 §5.2).
    ///
    /// Done here, on the finished display list, rather than during line
    /// breaking: the property does not change layout at all — the line is
    /// measured, broken and positioned as if it were `clip`, and only what
    /// gets PAINTED differs. Doing it any earlier would move the box.
    ///
    /// `clip_ops` keeps a text run whole when it merely overlaps the clip
    /// (glyphs are not clipped per pixel), so without this a `.text-truncate`
    /// box does not just lack the `…` — its text runs on out of the box.
    fn ellipsize(&mut self, start: usize, cr: i32) {
        for op in &mut self.ops[start..] {
            let DrawOp::Text { x, size, bold, italic, mono, sp, text, .. } = op else { continue };
            let font = self.fonts.pick(*bold, *italic, *mono);
            if *x + ceil_i32(measure_sp(font, text, *size, *sp)) <= cr {
                continue;
            }
            let dots = measure_sp(font, "\u{2026}", *size, *sp);
            let avail = (cr - *x) as f32 - dots;
            // No room for even the ellipsis: the run is past the edge entirely
            // and `clip_ops` will drop it, so leave it be rather than emitting
            // a lone `…` at a position the line never reserved.
            if avail <= 0.0 {
                continue;
            }
            let keep = fit_prefix(font, text, *size, avail, *sp);
            text.truncate(keep);
            text.push('\u{2026}');
        }
    }

    /// `filter` — recolour everything the box painted, itself and its subtree.
    ///
    /// The property applies to the whole subtree and cannot be cancelled from
    /// inside it, which in a FLAT display list is exactly the op range the box
    /// produced. Called after `paint_box_decoration`, so the box's own
    /// background and border — spliced in at the head of that range — are in it.
    ///
    /// Colours are transformed here rather than at paint, because here they are
    /// known. An image's pixels are not: it travels as a key and is looked up
    /// when it is drawn, so those ops get an index into `filters` instead.
    /// Applying the matrix twice IS the composition of two filters, which is
    /// what a filtered box inside a filtered box means — the image index is
    /// composed by hand for the same reason.
    fn apply_filter(&mut self, st: &ComputedStyle, start: usize) {
        let Some(f) = effective_filter(st) else { return };
        if start >= self.ops.len() {
            return;
        }
        let mut table = core::mem::take(&mut self.filters);
        for op in &mut self.ops[start..] {
            match op {
                DrawOp::Text { color, .. }
                | DrawOp::Rect { color, .. }
                | DrawOp::Shadow { color, .. }
                | DrawOp::RoundRect { color, .. } => *color = f.apply(*color),
                DrawOp::Image { filter, .. } => {
                    let inner = (*filter as usize).checked_sub(1).map(|i| table[i]);
                    *filter = filter_key(&mut table, inner.map_or(f, |i| i.then(f)));
                }
                // A mask paints `tint` THROUGH the image's alpha, so the
                // filter belongs on that colour, not on the stencil's pixels.
                DrawOp::BgImage { tint: Some(c), .. } => *c = f.apply(*c),
                DrawOp::BgImage { filter, .. } => {
                    let inner = (*filter as usize).checked_sub(1).map(|i| table[i]);
                    *filter = filter_key(&mut table, inner.map_or(f, |i| i.then(f)));
                }
            }
        }
        self.filters = table;
    }

    /// Register a `filter` and hand back the 1-based index an image op carries.
    /// An `<img>` with a filter of its OWN needs this before its op exists:
    /// the op is emitted from the line box, which has no `Ctx` to ask.
    fn filter_index(&mut self, st: &ComputedStyle) -> u16 {
        match effective_filter(st) {
            None => 0,
            Some(f) => {
                let mut table = core::mem::take(&mut self.filters);
                let i = filter_key(&mut table, f);
                self.filters = table;
                i
            }
        }
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
        // CSS 2.1 Appendix E paints a box as shadow, background, border — and
        // ALL of it before any descendant. All three splice in at `bg_idx`, so
        // whatever is inserted last ends up underneath: border, background,
        // shadow, in that order.
        //
        // The border used to be APPENDED instead, which put it on top of the
        // box's own descendants. That is invisible while a child stays inside
        // its parent's content box — and wrong the moment one does not, which
        // is exactly what a negative margin is for: the child's border landed
        // UNDER the parent's instead of over it.
        let mut border: Vec<DrawOp> = Vec::new();
        border_ops(st, x, y, w, h, (true, true), &mut border);
        self.insert_ops_at(bg_idx, border);
        // Der INNERE Schatten liegt ueber dem Hintergrund; er wird also vor
        // ihm eingefuegt, damit er nach der Verschiebung durch `insert_bg`
        // darueber steht.
        let mut inset = Vec::new();
        inset_shadow_ops(st, x, y, w, h, &mut inset);
        self.insert_ops_at(bg_idx, inset);
        self.insert_bg(st, x, y, w, h, bg_idx);
        self.insert_shadow(st, x, y, w, h, bg_idx);
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
        let mut parts = Vec::new();
        shadow_ops(st, x, y, w, h, &mut parts);
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
        let sib_count = el.children.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        let mut siblings: Vec<ElemInfo> = Vec::new();
        for c in &el.children {
            if let Node::Element(e) = c {
                let cs = self.styled(e, st, &siblings, sib_count);
                siblings.push(self.info(e));
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
                self.path.push(self.info(e));
                let part = self.part_start();
                y = self.layout_box(e, &cs, x, w, y);
                if cs.position == Position::Relative {
                    let (dx, dy) = rel_offset(&cs, w as f32);
                    if dx != 0 || dy != 0 {
                        self.shift_ops(&part, dx, dy);
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
            // A table with no rows is still a BOX. `width`/`height` on it are
            // content-box dimensions like anywhere else, so a bordered empty
            // table paints a frame of exactly that size — dropping out here
            // painted nothing at all, which is what an empty `<table>` with a
            // background looked like.
            let bg_idx = self.ops.len();
            let cbw = w as f32;
            let frame_x = st.pad_left + st.pad_right + st.border_x();
            let frame_y = st.pad_top + st.pad_bottom + st.border_y();
            let cw = st.width.px(cbw).map(|v| if st.box_border { v - frame_x } else { v }).unwrap_or(0.0);
            let ch = match st.height {
                Len::Px(h) if st.box_border => h - frame_y,
                Len::Px(h) => h,
                _ => 0.0,
            };
            let (bw, bh) = ((cw + frame_x).max(0.0) as i32, (ch + frame_y).max(0.0) as i32);
            let ml = st.margin_left.px(cbw).unwrap_or(0.0) as i32;
            self.paint_box_decoration(st, x + ml, y0, bw, bh, bg_idx);
            return y0 + bh;
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

    fn part_start(&self) -> SpecMark {
        self.spec_mark()
    }

    /// Close a table row or row-group box around everything emitted since
    /// `part`: its background goes behind that range, and `position: relative`
    /// then moves box and content together. Rows and row groups take a
    /// background but never a border — the separated model ignores border
    /// properties on them (CSS2.1 §17.6.1), and the collapsed model resolves
    /// every grid line at the cells.
    fn finish_table_part(&mut self, cs: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, part: SpecMark, cb_w: f32) {
        self.insert_bg(cs, x, y, w, h, part.ops);
        if cs.position == Position::Relative {
            let (dx, dy) = rel_offset(cs, cb_w);
            if dx != 0 || dy != 0 {
                self.shift_ops(&part, dx, dy);
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
        // A table is a shrink-to-fit box: with `width: auto` and every column
        // pinned by the first row, the table is exactly those columns wide. It
        // does NOT fill its container the way a block does — which is what put
        // a 200px cell's border across the whole page.
        let content_w = if st.width == Len::Auto && auto_count == 0 {
            sum_fixed.min(content_w)
        } else {
            content_w
        };
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
            self.path.push(self.info(g));
        }
        if let Some((e, _)) = row.el {
            self.path.push(self.info(e));
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
        let mut group: Option<(u32, ComputedStyle, SpecMark, i32)> = None;
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
                self.cb = (x, y, grid_w, Some(row_h), None);
            } else if let Some((_, gst, _, top)) = group {
                if gst.position != Position::Static {
                    // A row GROUP's height is not known yet, and unlike a
                    // positioned block there is no box to measure for it.
                    self.cb = (x, top, grid_w, None, None);
                }
            }
            for (c, (cs, cell_x, cell_w, content_x, content_w)) in cells.iter().enumerate() {
                if cs.display == Display::None {
                    continue;
                }
                let content_y = y + cell_borders(cs, collapse).2 as i32 + cs.pad_top as i32;
                let m0 = self.spec_mark();
                let bg_idx = m0.ops;
                let cell_cb = self.cb;
                if cs.position != Position::Static {
                    self.cb = (*content_x, y, *content_w, Some(row_h), None);
                }
                match row.cells[c].cell {
                    Cell::Real(e) => {
                        self.path.push(self.info(e));
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
                    self.shift_ops(&m0, 0, dy);
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
                    // Auch im zusammengefassten Modell malt eine Zelle ihre
                    // Schatten. Hier stand nur `insert_bg`, und damit fielen
                    // sie weg — was genau die gestreifte Tabelle traf:
                    // Bootstrap streift mit `box-shadow: inset`, und sein
                    // Reboot setzt `border-collapse: collapse` auf JEDE
                    // Tabelle. Beide Wege muessen dasselbe malen, sonst
                    // haengt das Aussehen einer Zelle daran, welches
                    // Randmodell die Seite gewaehlt hat.
                    let mut inset = Vec::new();
                    inset_shadow_ops(cs, *cell_x, y, *cell_w, row_h, &mut inset);
                    self.insert_ops_at(bg_idx, inset);
                    self.insert_bg(cs, *cell_x, y, *cell_w, row_h, bg_idx);
                    self.insert_shadow(cs, *cell_x, y, *cell_w, row_h, bg_idx);
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
                        self.shift_ops(&m0, dx, dy);
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
        let m = self.spec_mark();
        self.path.push(self.info(el));
        let bottom = self.layout_children(&el.children, st, Some(el), x, w.max(0), y);
        self.path.pop();
        self.spec_rollback(&m);
        (bottom - y).max(0)
    }

    /// Same as `measure_children_height`, for a table cell that may be an
    /// anonymous box (no owning element to push on `self.path`).
    fn measure_cell_height(&mut self, cell: &Cell<'a>, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        match cell {
            Cell::Real(e) => self.measure_children_height(e, st, x, w, y),
            Cell::Anon(nodes) => {
                let m = self.spec_mark();
                let bottom = self.layout_children(nodes, st, None, x, w.max(0), y);
                self.spec_rollback(&m);
                (bottom - y).max(0)
            }
        }
    }

    /// Classify a table child by tag, else by its computed `display` (CSS
    /// tables). Only elements are passed in.
    fn table_role(&self, e: &Element, parent: &ComputedStyle, prev: &[ElemInfo], sib_count: u32) -> TableRole {
        match e.tag.as_str() {
            "tr" => TableRole::Row,
            "thead" => TableRole::HeaderGroup,
            "tbody" => TableRole::RowGroup,
            "tfoot" => TableRole::FooterGroup,
            "td" | "th" => TableRole::Cell,
            "caption" | "col" | "colgroup" => TableRole::Skip,
            _ => {
                let st = self.styled(e, parent, prev, sib_count);
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
                Node::Element(e) => Some(self.table_role(e, parent, &siblings, sib_count)),
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
                            self.path.push(self.info(e));
                            let cells = self.partition_cells(&e.children, &est);
                            self.path.pop();
                            body.push(Row { el: Some((e, est)), group, cells })
                        }
                        Some(TableRole::RowGroup) => {
                            self.path.push(self.info(e));
                            self.collect_rows_into(&e.children, &est, Some((e, est)), header, body, footer);
                            self.path.pop();
                        }
                        Some(TableRole::HeaderGroup) | Some(TableRole::FooterGroup) => {
                            let (mut h, mut b, mut f) = (Vec::new(), Vec::new(), Vec::new());
                            self.path.push(self.info(e));
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
                siblings.push(self.info(e));
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
                Node::Element(e) => Some(self.table_role(e, parent, &siblings, sib_count)),
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
                siblings.push(self.info(e));
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
        // Size containment: the content contributes NOTHING to the box's own
        // size (css-contain-2 §3.1), so both intrinsic widths come from
        // `contain-intrinsic-size` — or are zero when it says nothing. This
        // has to sit ahead of every content-measuring branch below, including
        // the replaced one: the point of the property is that the box sizes
        // as if it held a single child of exactly that size.
        let out = if st.contain_size {
            let w = st.contain_intrinsic.map_or(0.0, |(iw, _)| iw);
            (w, w)
        } else if let Some((iw, _)) = replaced_intrinsic(el) {
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
                self.path.push(self.info(el));
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
                        siblings.push(self.info(e));
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
            self.path.push(self.info(el));
            self.intrinsic_walk(&el.children, &cs, run, pref, min);
            self.path.pop();
            return;
        }
        // Everything else is a box of its own: an atomic inline (image, form
        // control) or a block-level child. Either way it ends the current line.
        let (p, m) = if el.tag == "img" {
            self.path.push(self.info(el));
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
        let atomic_inline = matches!(cs.display,
            Display::InlineBlock | Display::InlineFlex | Display::Inline);
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
        // The sibling context every `table_role` here must see. Built once:
        // `elems` is every element child in order, `before[i]` how many of them
        // precede node `i`. Passing `&[], 0` instead (as this used to) is not
        // just wrong for `:nth-child` — it also gives the cascade cache a
        // second, incompatible key for the same element, and those repeat
        // misses were 90 % of all repeat misses on a real page.
        // Identity of this question: which node list, in which ancestor chain.
        let key = {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            let mut mix = |v: u64| {
                h ^= v;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            };
            mix(nodes.as_ptr() as u64);
            mix(nodes.len() as u64);
            for a in &self.path {
                mix(a.seq() as u64 | 0x1_0000_0000);
            }
            h
        };
        if let Some(hit) = self.segs.borrow().get(&key) {
            return hit
                .iter()
                .map(|&(a, b, table)| {
                    if table {
                        TableSeg::Table(&nodes[a as usize..b as usize])
                    } else {
                        TableSeg::Node(&nodes[a as usize])
                    }
                })
                .collect();
        }
        let elems: Vec<ElemInfo> = nodes
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) => Some(self.info(e)),
                _ => None,
            })
            .collect();
        let sib_count = elems.len() as u32;
        let mut before = Vec::with_capacity(nodes.len());
        {
            let mut k = 0usize;
            for n in nodes {
                before.push(k);
                if matches!(n, Node::Element(_)) {
                    k += 1;
                }
            }
        }
        let mut segs = Vec::with_capacity(nodes.len());
        let mut spans: Vec<(u32, u32, bool)> = Vec::with_capacity(nodes.len());
        let mut i = 0;
        while i < nodes.len() {
            let starts_run = matches!(&nodes[i], Node::Element(e) if is_table_part(self.table_role(e, parent, &elems[..before[i]], sib_count)));
            if starts_run {
                let mut last = i;
                let mut j = i + 1;
                while j < nodes.len() {
                    match &nodes[j] {
                        Node::Text(t) if t.trim().is_empty() => j += 1,
                        Node::Element(e) if is_table_part(self.table_role(e, parent, &elems[..before[j]], sib_count)) => {
                            last = j;
                            j += 1;
                        }
                        _ => break,
                    }
                }
                segs.push(TableSeg::Table(&nodes[i..=last]));
                spans.push((i as u32, last as u32 + 1, true));
                i = last + 1;
            } else {
                segs.push(TableSeg::Node(&nodes[i]));
                spans.push((i as u32, i as u32 + 1, false));
                i += 1;
            }
        }
        self.segs.borrow_mut().insert(key, spans);
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
        // `flow_block_impl` applies its own `filter` over its own op range —
        // doing it again here would compose the transform with itself, and an
        // inversion applied twice is no inversion at all.
        let f0 = self.ops.len();
        let bottom = match st.display {
            Display::Table => self.layout_table(el, st, x, w, y),
            Display::Flex | Display::InlineFlex => self.layout_flex(el, st, x, w, y),
            Display::Grid => self.layout_grid(el, st, x, w, y),
            _ => return self.layout_block(el, st, x, w, y),
        };
        self.apply_filter(st, f0);
        bottom
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
        let aspect = with_aspect_height(st, cw);
        let st = aspect.as_ref().unwrap_or(st);
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
            let g = st.grid_col_gap.px(avail).unwrap_or(0.0);
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
            // Rows-only grid → one IMPLICIT column, which `grid-auto-columns`
            // sizes exactly as `grid-auto-rows` sizes an implicit row.
            tracks.push(st.grid_auto_cols);
        }
        let ncols = tracks.len();

        // Grid items = in-flow child elements; abspos children are out of flow.
        let mut items: Vec<(&Element, ComputedStyle)> = Vec::new();
        let mut abs_items: Vec<(&Element, ComputedStyle)> = Vec::new();
        let sib_count = el.children.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        let mut siblings: Vec<ElemInfo> = Vec::new();
        for c in &el.children {
            if let Node::Element(ce) = c {
                let mut cs = self.styled(ce, st, &siblings, sib_count);
                siblings.push(self.info(ce));
                if matches!(cs.display, Display::Inline | Display::InlineBlock | Display::InlineFlex) {
                    cs.display = Display::Block;
                }
                if cs.display == Display::None {
                    continue;
                }
                if matches!(cs.position, Position::Absolute | Position::Fixed) {
                    // Deferred: a positioned child that names a grid line has
                    // that GRID AREA as its containing block (css-grid §9), and
                    // no track has a size yet. One that names none keeps the
                    // container's padding box.
                    abs_items.push((ce, cs));
                    continue;
                }
                items.push((ce, cs));
            }
        }

        // A percentage gap resolves against the container's own content box
        // on that axis; an indefinite height makes a percentage row-gap zero.
        let col_gap = st.grid_col_gap.px(w as f32).unwrap_or(0.0);
        let row_gap = st
            .grid_row_gap
            .px(content_height_of(st, st.height).unwrap_or(0.0))
            .unwrap_or(0.0);

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
                    Len::Intrinsic(k) => {
                        let (pref, min) = self.intrinsic_width(el_i, s);
                        intrinsic_size(k, pref, min, cw)
                    }
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
            let m0 = self.spec_mark();
            self.path.push(self.info(el_i));
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
                self.shift_ops(&m0, 0, dy);
            }
        }

        // — positioned children, now that every track has a size —
        //
        // css-grid §9: an absolutely positioned child whose `grid-row`/
        // `grid-column` names lines is contained by that GRID AREA; on an axis
        // where it names none, the containing block stays the grid container's
        // padding box. The two axes are decided separately, which is why this
        // is not one `if`.
        let prev_cb = self.cb;
        for (el_i, s) in &abs_items {
            let (px0, py0, pw, ph, pend) = prev_cb;
            let (mut ax, mut aw) = (px0, pw);
            let (mut ay, mut ah) = (py0, ph);
            if ncols > 0 && s.grid_col_start != 0 {
                let c = resolve_col(s.grid_col_start).min(ncols - 1);
                let cspan = (s.grid_col_span as usize).clamp(1, ncols - c);
                ax = colx[c] as i32;
                aw = cell_width(c, cspan).max(1.0) as i32;
            }
            if nrows > 0 && s.grid_row_start != 0 {
                let r = resolve_row(s.grid_row_start).min(nrows - 1);
                let rspan = (s.grid_row_span as usize).clamp(1, nrows - r);
                ay = row_y[r];
                let mut h = row_gap * (rspan as f32 - 1.0).max(0.0);
                for k in 0..rspan {
                    h += row_h[r + k];
                }
                ah = Some(h as i32);
            }
            self.cb = if ah == ph && ax == px0 && aw == pw && ay == py0 {
                prev_cb
            } else {
                (ax, ay, aw, ah, if ah == ph { pend } else { None })
            };
            let (sx, sy) = (self.cb.0, self.cb.1);
            self.path.push(self.info(el_i));
            self.layout_abs(el_i, s, sx, sy);
            self.path.pop();
        }
        self.cb = prev_cb;

        yy - y0
    }

    /// Lay a box just to measure its natural height, discarding the emitted ops
    /// (used for grid auto-row sizing before the real placement pass).
    /// The positioned containing block's height, running the deferred recipe
    /// (`PendingCbH`) on first read and caching the answer into `cb.3`.
    ///
    /// Everything that resolves a percentage against the containing block's
    /// height goes through here rather than reading `cb.3` — that is what
    /// makes the deferral invisible.
    fn cb_height(&mut self) -> Option<i32> {
        if let Some(h) = self.cb.3 {
            return Some(h);
        }
        let idx = self.cb.4? as usize;
        if let Some(h) = self.cb_pend[idx].resolved {
            self.cb.3 = Some(h);
            return Some(h);
        }
        // The same guard the eager version had: measuring the box re-enters it
        // and asks for its own height again. One level is all the answer needs.
        if self.measuring_cb_h.get() {
            return None;
        }
        self.measuring_cb_h.set(true);
        // Restore the context the measurement would have run in eagerly: it
        // happens now from somewhere inside the box's own subtree, where the
        // ancestor path is longer and the containing block, its height and the
        // active floats have all moved on.
        let p = &self.cb_pend[idx];
        let (el, st, x, w, y, border_y, path_len) = (p.el, p.st, p.x, p.w, p.y, p.border_y, p.path_len);
        let (pcb, pcb_h) = (p.cb, p.cb_h);
        let pfloats = p.floats.clone();
        let tail = self.path.split_off(path_len);
        let save_cb = core::mem::replace(&mut self.cb, pcb);
        let save_cb_h = core::mem::replace(&mut self.cb_h, pcb_h);
        let save_floats = core::mem::replace(&mut self.floats, pfloats);
        let h = self.measure_box_height(el, &st, x, w, y);
        self.floats = save_floats;
        self.cb_h = save_cb_h;
        self.cb = save_cb;
        self.path.extend(tail);
        self.measuring_cb_h.set(false);
        // `measure_box_height` returns the BORDER-box height; the containing
        // block is the PADDING box, so the two borders come off.
        let used = Some((h - border_y).max(0));
        self.cb_pend[idx].resolved = used;
        self.cb.3 = used;
        used
    }

    /// `measure_box_height` through the `measured` memo. `site` and `arg`
    /// identify which question is being asked — see `MeasureKey`.
    fn measured_h(&mut self, site: u8, arg: f32, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        let key = MeasureKey {
            site,
            seq: el.seq,
            x,
            y,
            w,
            arg: arg.to_bits(),
            cb_h: self.cb_h.map(|v| v.to_bits() as u64).unwrap_or(u64::MAX),
            // Not resolved on purpose: a pending containing block is
            // identified by its recipe index, so two different ones cannot
            // share a key without either being measured.
            cb_def_h: match (self.cb.3, self.cb.4) {
                (Some(h), _) => h as i64,
                (None, Some(i)) => -2 - i as i64,
                (None, None) => -1,
            },
            floats: self.floats.len() as u32,
        };
        if let Some(h) = self.measured.borrow().get(&key) {
            return *h;
        }
        let h = self.measure_box_height(el, st, x, w, y);
        self.measured.borrow_mut().insert(key, h);
        h
    }

    /// Everything one speculative flex-item placement can move — exactly the
    /// state `measure_box_height` rolls back, so keeping a placement instead of
    /// discarding it is the only difference between the two.
    fn flex_mark(&self) -> FlexMark {
        FlexMark { spec: self.spec_mark(), cb: self.cb }
    }

    fn flex_rollback(&mut self, m: &FlexMark) {
        self.spec_rollback(&m.spec);
        self.cb = m.cb;
    }

    /// Where every recorded vector stands right now.
    fn spec_mark(&self) -> SpecMark {
        SpecMark {
            ops: self.ops.len(),
            links: self.links.len(),
            controls: self.controls.len(),
            stack_ops: self.stack_ops.len(),
            stack_links: self.stack_links.len(),
            float_ops: self.float_ops.len(),
            float_links: self.float_links.len(),
            floats: self.floats.len(),
            inspects: self.inspects.len(),
            hover_boxes: self.hover_boxes.len(),
        }
    }

    /// Drop everything a discarded layout recorded.
    ///
    /// Stacking ranges index into `ops`/`links`, so a speculative run has to
    /// drop the ones it recorded too — otherwise they survive pointing into a
    /// vector that was truncated behind them, and `reorder_by_z` (which needs
    /// disjoint ascending ranges) slices the real display list at the wrong
    /// offsets. Floats live on past the box that placed them, so a leak puts
    /// exclusion rects into the real layout: the next float finds a BFC that
    /// looks full and drops below phantom neighbours. And `hover_boxes` /
    /// `inspects` are hit-test geometry — a trial run records them at trial
    /// COORDINATES, so the pointer lights up an element the page never painted
    /// there.
    fn spec_rollback(&mut self, m: &SpecMark) {
        self.ops.truncate(m.ops);
        self.links.truncate(m.links);
        self.controls.truncate(m.controls);
        self.stack_ops.truncate(m.stack_ops);
        self.stack_links.truncate(m.stack_links);
        self.float_ops.truncate(m.float_ops);
        self.float_links.truncate(m.float_links);
        self.floats.truncate(m.floats);
        self.inspects.truncate(m.inspects);
        self.hover_boxes.truncate(m.hover_boxes);
    }

    fn measure_box_height(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        let m = self.spec_mark();
        let prev_cb = self.cb;
        self.path.push(self.info(el));
        let bottom = self.layout_box(el, st, x, w.max(1), y);
        self.path.pop();
        self.spec_rollback(&m);
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
        // Ein nackter Textlauf zwischen den Kindern ist laut css-flexbox-1 §4
        // ein ANONYMER Flex-Kasten — er verschwand hier bisher spurlos, weil
        // die Schleife unten nur Elemente aufsammelte. `<div class="flex">Label
        // <span>x</span></div>` verlor sein „Label".

        // Flex items = in-flow child elements; abspos children are out of flow.
        // Structural selectors count EVERY element sibling, so the position is
        // tracked independently of which children become items.
        let mut items: Vec<(Kid<'a>, ComputedStyle)> = Vec::new();
        let sib_count = el.children.iter().filter(|n| matches!(n, Node::Element(_))).count() as u32;
        let mut siblings: Vec<ElemInfo> = Vec::new();
        for c in &el.children {
            if let Node::Text(t) = c {
                // Nur Laeufe mit sichtbarem Inhalt: reiner Leerraum zwischen
                // zwei Kaesten erzeugt keinen Kasten (§4).
                if t.trim().is_empty() {
                    continue;
                }
                let cs = crate::style::anon_inherit(st, Display::Block);
                if let Some(b) = self.anon_text_box(t, &cs, w) {
                    items.push((Kid::Anon(b), cs));
                }
                continue;
            }
            if let Node::Element(ce) = c {
                let mut cs = self.styled(ce, st, &siblings, sib_count);
                siblings.push(self.info(ce));
                // A flex item is blockified (css-display-3 §2.7).
                if matches!(cs.display, Display::Inline | Display::InlineBlock | Display::InlineFlex) {
                    cs.display = Display::Block;
                }
                if cs.display == Display::None {
                    continue;
                }
                if matches!(cs.position, Position::Absolute | Position::Fixed) {
                    self.path.push(self.info(ce));
                    self.layout_abs(ce, &cs, self.cb.0, self.cb.1);
                    self.path.pop();
                    continue;
                }
                items.push((Kid::El(ce), cs));
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
        let aspect = with_aspect_height(st, cw);
        let st = aspect.as_ref().unwrap_or(st);
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
        items: &[(Kid<'a>, ComputedStyle)],
        st: &ComputedStyle,
        x: i32,
        w: i32,
        y0: i32,
        def_cross: Option<f32>,
    ) -> i32 {
        let avail = w as f32;
        // Row flex: the MAIN axis is horizontal, so the gap between items is
        // `column-gap` and the gap between wrapped lines is `row-gap`. Reading
        // one value for both made `gap: 10px 20px` put the row gap between the
        // items instead of between the lines.
        let main_gap = st.grid_col_gap.px(avail).unwrap_or(0.0);
        let line_gap = st.grid_row_gap.px(def_cross.unwrap_or(0.0)).unwrap_or(0.0);

        // — per-item metrics (content-box main size = width) —
        let m = self.flex_metrics(items, avail, true);

        // — line breaking (flex-wrap) —
        let lines = flex_break_lines(&m, avail, main_gap, st.flex_wrap, st.flex_balance);

        // `align-content` packs the LINES in whatever cross space the container
        // has left over, so every line's cross size must be known before the
        // first line is placed. A multi-line container therefore lays its lines
        // out once to measure them, throws that placement away, and does it
        // again where they really go. Single-line containers keep the one-pass
        // path: align-content has no effect on them (css-flexbox-1 §8.4), and
        // neither does the packing default `start`.
        let n_lines = lines.len();
        let pack = n_lines > 1 && st.align_content != ContentAlign::Start;
        let mut forced: Vec<Option<i32>> = alloc::vec![None; n_lines];
        let (mut offset_cross, mut extra_line_gap) = (0i32, 0i32);
        if pack && let Some(cross) = def_cross {
            let mark = self.flex_mark();
            let mut nat: Vec<i32> = Vec::with_capacity(n_lines);
            let mut y = y0;
            for &line in &lines {
                let h = self.flex_row_line(items, st, &m, x, avail, main_gap, line, y, None);
                nat.push(h);
                y += h + line_gap as i32;
            }
            self.flex_rollback(&mark);
            let gaps = line_gap * (n_lines as f32 - 1.0);
            // NOT clamped at zero: the default overflow behaviour is `unsafe`
            // (css-align-3 §5.3), so `center` on lines that do not fit spills
            // equally out of both ends rather than piling up at the start.
            // Only the distributions have a spec'd fallback, below.
            let free = cross - nat.iter().sum::<i32>() as f32 - gaps;
            let nf = n_lines as f32;
            match st.align_content {
                // Every line grows by an equal share. The share is taken
                // CUMULATIVELY so the integer rounding cannot drift: the last
                // line still ends exactly on the container's content edge.
                ContentAlign::Stretch if free > 0.0 => {
                    for (i, h) in nat.iter().enumerate() {
                        let so_far = (free * i as f32 / nf) as i32;
                        let upto = (free * (i + 1) as f32 / nf) as i32;
                        forced[i] = Some(h + upto - so_far);
                    }
                }
                ContentAlign::End => offset_cross = free as i32,
                ContentAlign::Center => offset_cross = (free / 2.0) as i32,
                ContentAlign::Between if free > 0.0 => {
                    extra_line_gap = (free / (nf - 1.0)) as i32;
                }
                ContentAlign::Around | ContentAlign::Evenly if free <= 0.0 => {
                    // Both fall back to `center` when the lines overflow
                    // (css-align-3 §5.4), not to `start`.
                    offset_cross = (free / 2.0) as i32;
                }
                ContentAlign::Around => {
                    offset_cross = (free / (2.0 * nf)) as i32;
                    extra_line_gap = (free / nf) as i32;
                }
                ContentAlign::Evenly => {
                    offset_cross = (free / (nf + 1.0)) as i32;
                    extra_line_gap = (free / (nf + 1.0)) as i32;
                }
                // `stretch` cannot shrink a line and `space-between` piles up
                // at the start — both are `flex-start` once the space is gone.
                ContentAlign::Start | ContentAlign::Stretch | ContentAlign::Between => {}
            }
        } else if n_lines == 1 {
            forced[0] = def_cross.map(|c| c as i32);
        }

        let mut cross_y = y0 + offset_cross;
        for (i, &line) in lines.iter().enumerate() {
            let line_cross =
                self.flex_row_line(items, st, &m, x, avail, main_gap, line, cross_y, forced[i]);
            cross_y += line_cross;
            if i + 1 < n_lines {
                cross_y += line_gap as i32 + extra_line_gap;
            }
        }
        (cross_y - y0).max(0)
    }

    /// Lay one flex line out at `cross_y` and return the cross size it used.
    /// `forced_cross` is the size the line was GIVEN (a single line filling a
    /// definite container, or a share handed out by `align-content: stretch`);
    /// `None` means it sizes to its tallest item, which is also what the
    /// measuring pass asks for.
    #[allow(clippy::too_many_arguments)]
    fn flex_row_line(
        &mut self,
        items: &[(Kid<'a>, ComputedStyle)],
        st: &ComputedStyle,
        m: &[FlexItem],
        x: i32,
        avail: f32,
        main_gap: f32,
        line: (usize, usize),
        cross_y: i32,
        forced_cross: Option<i32>,
    ) -> i32 {
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

        // Natural cross size (height) at the resolved width, to size the
        // line — laid out AT the spot the item will most likely keep, and
        // the ops KEPT instead of discarded.
        //
        // Measuring a flex item already lays its whole subtree out; the old
        // code threw that away and then laid the identical thing again, so
        // every nesting level doubled the work (2^5 on MediaWiki's header).
        // Counted on real pages, 93–96 % of items end up at exactly
        // `(item_x[k], cross_y)` with their natural height, so the second
        // pass was almost always a byte-for-byte repeat of the first.
        let mut h_nat = alloc::vec![0i32; ln];
        let mut marks: Vec<FlexMark> = Vec::with_capacity(ln);
        for k in 0..ln {
            let (kid, s) = (&items[idx0 + k].0, items[idx0 + k].1);
            let s_meas = flex_item_style(&s, size[k], None, true);
            let box_main = (size[k] + li[k].main_pad).max(1.0) as i32;
            let mark = self.flex_mark();
            let bottom = match kid {
                Kid::El(el) => {
                    self.path.push(self.info(el));
                    let b = self.layout_box(el, &s_meas, item_x[k] as i32, box_main, cross_y);
                    self.path.pop();
                    b
                }
                // Ein anonymer Kasten ist schon fertig — er wird nur gelegt.
                Kid::Anon(b) => {
                    self.place_atomic(b, item_x[k] as i32, cross_y);
                    cross_y + b.h
                }
            };
            h_nat[k] = bottom - cross_y;
            // Keep the ops, but NOT the ambient state a discarded
            // measurement used to drop: a flex item is its own block
            // formatting context, so its floats must not reach its
            // siblings, and the containing block it installed is gone with
            // it. Leaving those standing made every later item on the line
            // flow around phantom exclusions.
            self.floats.truncate(mark.spec.floats);
            self.cb = mark.cb;
            marks.push(mark);
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
        let line_cross = forced_cross.unwrap_or(nat_line);

        // Now that the line's cross size is known, work out where each item
        // really goes, and find the FIRST one the speculative pass got
        // wrong. A forced height counts as wrong even when the number
        // matches: it changes the derived style, so the subtree below can
        // resolve differently.
        let mut first_redo = ln;
        let mut plan: Vec<(Option<f32>, i32)> = Vec::with_capacity(ln);
        for k in 0..ln {
            let s = &items[idx0 + k].1;
            let align = s.align_self.unwrap_or(st.align_items);
            let inner = (line_cross - li[k].cm_lead as i32 - li[k].cm_trail as i32).max(0);
            // An `auto` cross margin eats the line's free cross space, and
            // that overrides both `align-self` and the stretch (css-flexbox-1
            // §8.1 / §9.4 step 11) — `mt-auto` on a row item means bottom, no
            // matter what the container aligns to.
            let cross_m_auto = li[k].cm_lead_auto || li[k].cm_trail_auto;
            let stretch = align == CrossAlign::Stretch && li[k].cross_auto && !cross_m_auto;
            let (forced_h, y) = if stretch {
                let target = clamp_cross(inner as f32, li[k].min_cross, li[k].max_cross);
                (Some(target), cross_y + li[k].cm_lead as i32)
            } else if cross_m_auto {
                let free = (inner - h_nat[k]).max(0) as f32;
                let share = free / (li[k].cm_lead_auto as u32 + li[k].cm_trail_auto as u32) as f32;
                let lead = if li[k].cm_lead_auto { share } else { li[k].cm_lead };
                (None, cross_y + lead as i32)
            } else {
                let h = h_nat[k];
                let y = match align {
                    CrossAlign::End => cross_y + line_cross - li[k].cm_trail as i32 - h,
                    CrossAlign::Center => cross_y + li[k].cm_lead as i32 + (inner - h) / 2,
                    _ => cross_y + li[k].cm_lead as i32, // start / stretch-with-def-size / baseline
                };
                (None, y)
            };
            if first_redo == ln && (forced_h.is_some() || y != cross_y) {
                first_redo = k;
            }
            plan.push((forced_h, y));
        }

        // `ops` is one sequential list, so redoing item k means dropping
        // everything emitted from k onward — hence the first mismatch, not
        // each one. Worst case this is exactly the two passes it replaced.
        if first_redo < ln {
            self.flex_rollback(&marks[first_redo]);
            for k in first_redo..ln {
                let (kid, s) = (&items[idx0 + k].0, items[idx0 + k].1);
                let (forced_h, y) = plan[k];
                let s2 = flex_item_style(&s, size[k], forced_h, true);
                let Kid::El(el) = kid else {
                    // Der anonyme Kasten aendert sich durch die zweite Runde
                    // nicht — er hat keine Kinder, die anders fielen.
                    if let Kid::Anon(b) = kid { self.place_atomic(b, item_x[k] as i32, y); }
                    continue;
                };
                self.path.push(self.info(el));
                // `layout_box` takes the box the caller resolved — the
                // item's BORDER box. `size[k]` is its content size, so the
                // item's own padding and border have to go back on, or a
                // control (which paints exactly this width) loses them and
                // clips its label.
                let box_main = (size[k] + li[k].main_pad).max(1.0) as i32;
                let _ = self.layout_box(el, &s2, item_x[k] as i32, box_main, y);
                self.path.pop();
            }
        }
        line_cross
    }


    /// Column flex (main axis = vertical, cross axis = horizontal). `def_cross`
    /// is the container's definite content height (main size) if any.
    fn flex_column(
        &mut self,
        items: &[(Kid<'a>, ComputedStyle)],
        st: &ComputedStyle,
        x: i32,
        w: i32,
        y0: i32,
        def_cross: Option<f32>,
    ) -> i32 {
        let n = items.len();
        let avail = w as f32; // cross-axis available (width)
        // Column flex: the main axis is vertical, so `row-gap` separates the
        // items, resolved against the container's own definite height.
        let gap = st.grid_row_gap.px(def_cross.unwrap_or(0.0)).unwrap_or(0.0);

        // Cross-axis (horizontal) width + position, plus main-axis (vertical)
        // margins, per item. Cross axis is never flexed (grow/shrink are main).
        let mut cross_w = alloc::vec![0.0f32; n];
        let mut ix = alloc::vec![0i32; n];
        let mut mm_lead = alloc::vec![0.0f32; n];
        let mut mm_trail = alloc::vec![0.0f32; n];
        let mut ma_lead = alloc::vec![false; n];
        let mut ma_trail = alloc::vec![false; n];
        let mut h_nat = alloc::vec![0i32; n];
        for (i, (el, s)) in items.iter().enumerate() {
            let pad_h = s.pad_left + s.pad_right;
            let to_content = |px: f32| if s.box_border { (px - pad_h).max(0.0) } else { px };
            let ml = s.margin_left.px(avail).unwrap_or(0.0);
            let mr = s.margin_right.px(avail).unwrap_or(0.0);
            let align = s.align_self.unwrap_or(st.align_items);
            let width_auto = matches!(s.width, Len::Auto);
            // Cross axis of a column = horizontal. An `auto` margin there takes
            // the free width and cancels the stretch (css-flexbox-1 §9.4 step
            // 11) — without that, `mx-auto` on a stretched item has nothing
            // left to centre and reads as ignored.
            let ml_auto = matches!(s.margin_left, Len::Auto);
            let mr_auto = matches!(s.margin_right, Len::Auto);
            let stretch = align == CrossAlign::Stretch && width_auto && !(ml_auto || mr_auto);
            let mut wd = if stretch {
                (avail - ml - mr).max(1.0)
            } else if let Len::Intrinsic(k) = s.width {
                let (pref, min) = self.kid_intrinsic(el, s);
                intrinsic_size(k, pref, min, (avail - ml - mr).max(0.0))
            } else {
                s.width.px(avail).map(to_content).unwrap_or_else(|| self.kid_intrinsic(el, s).0)
            };
            if let Some(mx) = s.max_width.px(avail) {
                wd = wd.min(to_content(mx));
            }
            if let Some(mn) = s.min_width.px(avail) {
                wd = wd.max(to_content(mn));
            }
            wd = wd.clamp(1.0, avail.max(1.0));
            cross_w[i] = wd;
            ix[i] = if ml_auto || mr_auto {
                let free = (avail - ml - mr - wd).max(0.0);
                let lead = if ml_auto && mr_auto {
                    free / 2.0
                } else if ml_auto {
                    free
                } else {
                    0.0
                };
                x + (ml + lead) as i32
            } else {
                match align {
                    CrossAlign::End => x + (avail - mr - wd) as i32,
                    CrossAlign::Center => x + (ml + (avail - ml - mr - wd) / 2.0) as i32,
                    _ => x + ml as i32, // start / stretch
                }
            };
            mm_lead[i] = s.margin_top;
            mm_trail[i] = s.margin_bottom;
            ma_lead[i] = s.margin_top_auto;
            ma_trail[i] = s.margin_bottom_auto;
            let s_meas = flex_item_style(s, wd, None, false);
            h_nat[i] = match el {
                Kid::El(e) => self.measured_h(MEAS_FLEX_COL, wd, e, &s_meas, ix[i], wd.max(1.0) as i32, y0),
                Kid::Anon(b) => b.h,
            };
        }

        // Total intrinsic main size (heights + vertical margins + gaps).
        let gaps_total = gap * (n as f32 - 1.0).max(0.0);
        let sum_h: f32 = (0..n).map(|i| mm_lead[i] + h_nat[i] as f32 + mm_trail[i]).sum();
        let intrinsic = sum_h + gaps_total;
        // A definite container height gives free main space → justify-content.
        let free = def_cross.map(|c| c - intrinsic).unwrap_or(0.0).max(0.0);
        // Main-axis auto margins take the free space FIRST; `justify-content`
        // only ever sees what they leave (css-flexbox-1 §8.1). This is what
        // makes `mt-auto` on the last child of a fixed-height column pin it to
        // the bottom — the card-footer pattern.
        let n_auto: usize = (0..n).map(|i| ma_lead[i] as usize + ma_trail[i] as usize).sum();
        let (offset, extra_gap, auto_each) = if free > 0.5 && n_auto > 0 {
            (0.0, 0.0, free / n_auto as f32)
        } else {
            let (o, g) = match st.justify {
                Justify::End => (free, 0.0),
                Justify::Center => (free / 2.0, 0.0),
                Justify::Between => (0.0, if n > 1 { free / (n as f32 - 1.0) } else { 0.0 }),
                Justify::Around => (free / (2.0 * n as f32), free / n as f32),
                Justify::Evenly => (free / (n as f32 + 1.0), free / (n as f32 + 1.0)),
                Justify::Start => (0.0, 0.0),
            };
            (o, g, 0.0)
        };

        let mut y = y0 as f32 + offset;
        for (i, (el, s)) in items.iter().enumerate() {
            y += mm_lead[i] + if ma_lead[i] { auto_each } else { 0.0 };
            let s2 = flex_item_style(s, cross_w[i], None, false);
            let bottom = match el {
                Kid::El(e) => {
                    self.path.push(self.info(e));
                    let b = self.layout_box(e, &s2, ix[i], cross_w[i].max(1.0) as i32, y as i32);
                    self.path.pop();
                    b
                }
                Kid::Anon(b) => {
                    self.place_atomic(b, ix[i], y as i32);
                    y as i32 + b.h
                }
            };
            y = bottom as f32 + mm_trail[i] + if ma_trail[i] { auto_each } else { 0.0 };
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
    fn flex_metrics(&mut self, items: &[(Kid<'a>, ComputedStyle)], avail: f32, row: bool) -> Vec<FlexItem> {
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
            // An `auto` margin is free space on ITS axis, so which of the four
            // counts as main and which as cross flips with the direction. The
            // vertical pair carries its keyword beside the number, because in
            // normal flow it is used as zero.
            let h_lead_auto = matches!(s.margin_left, Len::Auto);
            let h_trail_auto = matches!(s.margin_right, Len::Auto);
            let (m_lead_auto, m_trail_auto) = if row {
                (h_lead_auto, h_trail_auto)
            } else {
                (s.margin_top_auto, s.margin_bottom_auto)
            };
            let (cm_lead_auto, cm_trail_auto) = if row {
                (s.margin_top_auto, s.margin_bottom_auto)
            } else {
                (h_lead_auto, h_trail_auto)
            };
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
            let control_chrome = match el.el() {
                Some(e) if crate::forms::kind_of(e).is_some() => main_pad,
                _ => 0.0,
            };
            let (pref_bb, minc_bb) = self.kid_intrinsic(el, s);
            let (pref, minc) = ((pref_bb - control_chrome).max(0.0), (minc_bb - control_chrome).max(0.0));
            let base = match s.flex_basis {
                FlexBasis::Px(p) => to_content(p),
                FlexBasis::Pct(p) => to_content(p / 100.0 * avail),
                FlexBasis::Auto => spec.unwrap_or(pref),
            };
            // Automatic minimum size = min(content min, specified suggestion) —
            // but only while the item's overflow is `visible` on the main axis.
            // A scroll container has no automatic minimum (css-flexbox-1 §4.5):
            // its content is meant to be clipped, so it may shrink to nothing.
            let scrolls = if row { s.overflow_x.scrolls() } else { s.overflow_y.scrolls() };
            let mut floor = if scrolls { 0.0 } else { minc.min(spec.unwrap_or(minc)) };
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
                cm_lead_auto,
                cm_trail_auto,
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

    /// Shift everything recorded since `m` by `(dx, dy)` — used to place a
    /// flex item on the cross axis, and to offset a `position:relative` box
    /// after it is laid in flow.
    ///
    /// It takes the whole mark rather than a few explicit indices so that a
    /// side table added later cannot be forgotten here: hit rects that do not
    /// follow their painted box put the pointer where the box used to be, and
    /// `hover_boxes` shipped in 0.25.0 with exactly that defect.
    fn shift_ops(&mut self, m: &SpecMark, dx: i32, dy: i32) {
        for op in &mut self.ops[m.ops..] {
            match op {
                DrawOp::Text { x, y, .. }
                | DrawOp::Rect { x, y, .. }
                | DrawOp::RoundRect { x, y, .. }
                | DrawOp::Shadow { x, y, .. }
                | DrawOp::Image { x, y, .. }
                | DrawOp::BgImage { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
            }
        }
        for lk in &mut self.links[m.links..] {
            lk.x += dx;
            lk.y += dy;
        }
        // Hit rects must follow their painted box (relative offsets, flex
        // cross-alignment) or the click — or the pointer — lands where the box
        // used to be.
        for c in &mut self.controls[m.controls..] {
            c.x += dx;
            c.y += dy;
        }
        for b in &mut self.hover_boxes[m.hover_boxes..] {
            b.x += dx;
            b.y += dy;
            // The anchor names an op inside this very range, so it moves with
            // it — a shifted box pointing at an unshifted anchor would look up
            // an op that no longer exists.
            b.paint.0 += dx;
            b.paint.1 += dy;
            if let Some(a) = &mut b.anchor {
                a.x += dx;
                a.y += dy;
            }
        }
        for b in &mut self.inspects[m.inspects..] {
            b.x += dx;
            b.y += dy;
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
    cm_lead_auto: bool,
    cm_trail_auto: bool,
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
                sp: (st.letter_spacing, st.word_spacing),
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
    let sp = (st.letter_spacing, st.word_spacing);
    // `white-space: pre` keeps the source line breaks, so each source line is
    // its own line box and the widest one wins — collapsing them into one
    // would measure a whole code block as a single enormous line.
    if st.pre {
        let font = fonts.pick(st.bold, st.italic, st.mono);
        let mut widest = 0.0f32;
        for line in run.text.lines() {
            // Trailing spaces hang past the line box, so they never widen it
            // (css-text-3 §8). Leading ones DO count under `pre`.
            widest = widest.max(measure_sp(font, line.trim_end_matches(is_hangable_space), st.font_px, sp));
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
    let p = measure_sp(
        font,
        collapsed.trim_start_matches(is_css_space).trim_end_matches(is_hangable_space),
        size,
        sp,
    ) + frame
        + atomic;
    // `white-space: nowrap` has no break opportunities, so min-content is the
    // whole line — not its widest word. Without this a shrink-to-fit box around
    // a nowrap run is sized to one word and the run hangs out of it.
    let m = if st.nowrap {
        p
    } else {
        let words =
            collapsed
                .split(is_css_space)
                .filter(|w| !w.is_empty())
                .map(|wd| measure_sp(font, wd, size, sp))
                .fold(0.0f32, f32::max)
                + frame;
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
        if is_css_space(ch) {
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
            Len::Intrinsic(k) => {
                let (pref, min) = self.intrinsic_width(el, st);
                let room = (cbw - ml - mr - pad_border).max(0.0);
                intrinsic_size(k, pref, min, room)
            }
            other => {
                let v = other.px(cbw).unwrap_or(0.0);
                if st.box_border { (v - pad_border).max(0.0) } else { v }
            }
        };
        let outer_w = ceil_i32(content_w + pad_border + ml + mr).max(1);

        let (o0, l0, c0) = (self.ops.len(), self.links.len(), self.controls.len());
        let (i0, h0) = (self.inspects.len(), self.hover_boxes.len());
        let saved_floats = core::mem::take(&mut self.floats);
        let saved_baseline = self.last_baseline.take();
        self.path.push(self.info(el));
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
        let hover_boxes: Vec<HoverBox> = self.hover_boxes.drain(h0..).collect();
        let h = (border_bottom + st.margin_bottom as i32).max(0);
        // The box aligns on its LAST line box's baseline; with no in-flow line
        // box, or when it clips its overflow, it aligns on its bottom margin
        // edge instead (CSS2.1 §10.8.1).
        let baseline = match inner_baseline {
            Some(b) if !st.overflow_clip() => b.clamp(0, h),
            _ => h,
        };
        Some(AtomicBox { ops, links, controls, inspects, hover_boxes, w: outer_w, h, baseline, valign: st.valign })
    }

    /// The inline box an inline-level child needs, if any: one that paints
    /// something of its own or reserves horizontal space. An `<img>`, a form
    /// control and an `inline-block` are atomic — each already lays out and
    /// paints its own box — and a `<br>` has none at all.
    fn inline_box_of(&self, el: &Element, st: &ComputedStyle, cb_w: i32) -> Option<InlineBox> {
        if st.is_break || st.display != Display::Inline || el.tag == "img" || el.tag == "svg" || crate::forms::kind_of(el).is_some() {
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
        // A box that paints nothing and reserves no space normally has no
        // reason to exist — but a hover rule needs its RECTANGLE even when it
        // is invisible at rest, and a bare `<a href>` is exactly that box. Miss
        // this and the pointer finds every element except the ones it aims at.
        let hover_seq = self.sheet.hover_set.may_match(el).then_some(el.seq);
        if !paints && lead == 0.0 && trail == 0.0 && hover_seq.is_none() {
            return None;
        }
        Some(InlineBox {
            st: fade_style(st),
            hover_seq,
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
            // An image's own box; a hover rule on it is caught by the element's
            // block-level record, not here.
            hover_seq: None,
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
        if matches!(st.display, Display::InlineBlock | Display::InlineFlex) {
            if let Some(b) = self.inline_block_box(el, st, bw) {
                inline.atomic(b);
            }
            return;
        }
        // An `<img>` inside inline content (e.g. `<a><img></a>` — Wikipedia's
        // thumbnails) is an atomic inline box; carry the enclosing link so it
        // stays clickable.
        if el.tag == "img" || el.tag == "svg" {
            let svg = el.tag == "svg";
            let (iw, ih) = if svg { self.svg_box(el, st) } else { self.img_box(el, st) };
            let src = if svg { svg_key(el) } else { el.attr("src").unwrap_or("").to_string() };
            let fx = self.filter_index(st);
            inline.image(src, iw, ih, href, svg_alt(el, svg), st.hidden, st.transparent, st.object_fit, fx, self.image_deco(st));
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
        // A `<button>` under `display: contents` is unboxed like any other
        // element — its label becomes ordinary inline content of the parent,
        // and the UA widget it would otherwise draw is exactly the box the
        // property says must not exist.
        if let Some(kind) = crate::forms::kind_of(el).filter(|_| st.display != Display::Contents) {
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
                    siblings.push(self.info(ce));
                    if cs.display == Display::None {
                        continue;
                    }
                    self.counters.enter(&cs, self.path.len());
                    // `position:absolute`/`fixed` leaves the inline flow the same
                    // way it leaves the block flow — `flow_children` has had this
                    // branch all along and this one did not, so an out-of-flow box
                    // that happened to be INLINE-level stayed on the line and grew
                    // the page with it. Wikipedia's 1×1 autologin pixel is exactly
                    // that shape, and a 40×40 abspos `<img>` added its full height.
                    // Ahead of the float test because `float` computes to `none` on
                    // a positioned box (css-display-3 §2.7).
                    if matches!(cs.position, Position::Absolute | Position::Fixed) {
                        self.path.push(self.info(ce));
                        self.abs_over_open_line = !inline.is_empty();
                        self.layout_abs(ce, &cs, bx, by);
                        self.abs_over_open_line = false;
                        self.path.pop();
                        continue;
                    }
                    // A floated inline element leaves the inline flow and is placed
                    // as a float; surrounding text wraps around it.
                    if cs.float != FloatKind::None {
                        self.place_float(ce, &cs, bx, bw, by);
                        continue;
                    }
                    let ib = self.inline_box_of(ce, &cs, bw).map(|b| inline.open_box(b));
                    self.path.push(self.info(ce));
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
    color: Rgba,
    bold: bool,
    italic: bool,
    mono: bool,
    valign: crate::style::VAlign,
    /// `text-decoration-line` bits (`style::DECO_*`).
    deco: u8,
    /// `text-decoration-color`; `None` = `currentColor`.
    deco_color: Option<Rgba>,
    /// `overflow-wrap`/`word-break` allow splitting this run mid-word.
    break_word: bool,
    /// `white-space: nowrap` — this run's spaces are not break opportunities,
    /// so the line grows past its box rather than wrapping.
    nowrap: bool,
    /// Used `line-height` in px, or 0 for `normal` (use the face's metrics).
    lh: f32,
    /// `(letter-spacing, word-spacing)` in px. Every width in this file that
    /// belongs to a run goes through `measure_sp`, so a run measures and paints
    /// at the same advance — the two must not drift apart.
    sp: (f32, f32),
}

/// An inline-block's finished display list, laid out at the origin and
/// translated into place once the line box knows where it sits.
struct AtomicBox {
    ops: Vec<DrawOp>,
    links: Vec<LinkRect>,
    controls: Vec<ControlRect>,
    /// Hit rects recorded while laying this box out at the origin. They move
    /// with it — without that every box inside an `inline-block` is reported at
    /// the page's top-left corner, which reads as a layout bug that is not
    /// there. It was one for `:hover`: the pointer lit up links it was nowhere
    /// near, and the real link answered to nothing.
    inspects: Vec<InspectBox>,
    hover_boxes: Vec<HoverBox>,
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
    /// `seq` of the element this box came from, but only if a `:hover` rule
    /// could react to it. `None` for the overwhelming majority, which is what
    /// keeps the hit-test list short — an inline box is where LINKS live, so
    /// without this the pointer would miss exactly what it aims at.
    hover_seq: Option<u32>,
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
    Image { src: String, w: i32, h: i32, href: Option<String>, alt: String, space_before: bool, hidden: bool, transparent: bool, fit: ObjectFit, filter: u16, deco: Option<alloc::boxed::Box<InlineBox>> },
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
#[derive(Clone)]
struct CtlBox {
    seq: u32,
    kind: ControlKind,
    w: i32,
    h: i32,
    /// Displayed text: value, placeholder, or button/select label.
    text: String,
    /// `text` is a placeholder → paint it muted.
    ghost: bool,
    /// The element's `placeholder`, kept for a repaint: emptying a field has to
    /// bring it back, and the repaint has no element to ask.
    placeholder: String,
    checked: bool,
    focused: bool,
    /// Caret position in characters, when this control has keyboard focus.
    caret: Option<usize>,
    /// The control's own `background-color`, if the page styled it.
    bg: Option<Rgba>,
    /// `appearance: none` — the page draws this control itself, so we paint no
    /// UA face at all (css-ui-4 §4).
    appearance_none: bool,
    /// The page's own `background-image` (resolved key + placement). A control
    /// that opted out of the UA look carries its icon this way — DDG's search
    /// button is a bare box with a magnifier here and nothing else.
    bg_img: Option<(u64, BgLayer)>,
    /// Leading text inset. Controls are atomic — we paint them with our own
    /// metrics — but a page that reserves room for an icon does it with
    /// `padding-left`, and ignoring that puts the text on top of the icon
    /// (Wikipedia's search field asks for 36px to clear its magnifier). CSS
    /// only ever WIDENS the inset; it cannot squeeze the text below `CTL_PAD_X`.
    pad_l: i32,
    /// Die rechte Polsterung — dieselbe Zahl, mit der die Breite gerechnet
    /// wurde. Der Maler nahm frueher `CTL_PAD_X`, und die Differenz zur
    /// gemessenen Breite schnitt die Beschriftung ab.
    pad_r: i32,
    /// The frame, in paint order top/right/bottom/left.
    border: [CtlSide; 4],
    /// `border-radius` in px, top-left clockwise. A control is painted with our
    /// own metrics, so the page's radius has to be CARRIED here — it is not a
    /// detail: every button on a Bootstrap or Tailwind page is rounded, and
    /// square corners are the first thing that reads as „not a browser".
    radius: [f32; 4],
    style: RunStyle,
}

/// One edge of a control's frame. The UA gives every control a 1px one; a page
/// that writes any `border` longhand or shorthand owns all four instead —
/// including `border: none`, which is a declaration and not an absence.
#[derive(Clone, Copy)]
struct CtlSide {
    w: i32,
    /// `None` = paint in the UA's frame colour (the page named none).
    color: Option<Rgba>,
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
    // HTML §4.10.5.1.20: on an `<input>`, `value=""` is an explicit EMPTY
    // label — only a MISSING attribute gets the UA default. Pages put their
    // own icon on the button by CSS and rely on it staying empty; DDG's search
    // button is a magnifier that way, and "Absenden" painted straight over it.
    if el.tag == "input" && el.attr("value").is_some() {
        return String::new();
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
    let at = ops.len();
    // Jede Rueckkehr aus dieser Funktion meldet ihren Bereich — sonst zeigt
    // ein Eintrag auf Befehle, die ein anderes Element gemalt hat.
    let rect = |ops: &Vec<DrawOp>, ctl: &CtlBox| ControlRect {
        x, y: top, w: ctl.w, h: ctl.h, seq: ctl.seq, kind: ctl.kind,
        at, len: ops.len() - at, paint: ctl.clone(),
    };
    // A `visibility:hidden` control paints nothing and takes no clicks — it is
    // not registered, so it can't sit as an invisible target over the page.
    if ctl.style.hidden {
        return;
    }
    let (w, h) = (ctl.w, ctl.h);
    // `opacity:0`: paint nothing, but keep the hit rect — this is the
    // checkbox-hack overlay that a CSS-only dropdown is toggled with.
    if ctl.style.transparent {
        controls.push(rect(ops, ctl));
        return;
    }
    let font = fonts.pick(ctl.style.bold, ctl.style.italic, ctl.style.mono);
    // A control's chrome follows the SURFACE IT SITS ON, not the device theme.
    // Wikipedia paints itself light whatever the desktop is set to (its dark
    // mode is opt-in, gated on a class), so a face mixed from a dark theme is a
    // black box on a white page. The signal that is actually to hand is the
    // control's own inherited text colour: light text means a dark surface
    // behind it, and dark text a light one.
    let theme = &surface_palette(theme, ctl.style.color.c);
    let border = Rgba::opaque(if ctl.focused { theme.link } else { mix(theme.rule, theme.text, 40) });
    let round = ctl.radius.iter().any(|r| *r > 0.5);
    // Eine gerundete Ecke kann nicht aus vier Rechtecken bestehen. Solange alle
    // vier Seiten dieselbe Breite und Farbe haben — bei Knoepfen und Feldern
    // immer —, ist der Rahmen EIN Ring; sonst bleibt es beim eckigen Rahmen,
    // und das ist die ehrlichere Naeherung als eine Ecke, die nur auf einer
    // Seite rund waere.
    let ring: Option<(f32, Rgba)> = {
        let [t, r, b, l] = ctl.border;
        let same = [r, b, l].iter().all(|s| s.w == t.w && s.transparent == t.transparent
                                            && s.color.map(|c| c.c) == t.color.map(|c| c.c));
        (round && same && t.w > 0 && !t.transparent)
            .then(|| (t.w as f32, if ctl.focused { border } else { t.color.unwrap_or(border) }))
    };
    let frame = |ops: &mut Vec<DrawOp>| match ring {
        Some((bw, color)) => ops.push(DrawOp::RoundRect {
            x, y: top, w, h, r: ctl.radius, color, ring: bw,
        }),
        None => stroke_frame(ops, x, top, w, h, &ctl.border, border, ctl.focused),
    };
    // Die Flaeche — gerundet, wenn die Seite es sagt.
    let face_op = |ops: &mut Vec<DrawOp>, color: Rgba| {
        if round {
            ops.push(DrawOp::RoundRect { x, y: top, w, h, r: ctl.radius, color, ring: 0.0 });
        } else {
            ops.push(DrawOp::Rect { x, y: top, w, h, color });
        }
    };
    // A page that styles its own button (`background-color`) wins; otherwise
    // the UA face is derived from the theme so it reads on light and dark.
    // `appearance: none` (css-ui-4 §4) removes the question: the page opted
    // out of the UA widget, so there is NO default face — only what the page
    // paints itself. Our chrome otherwise filled in a box over a control the
    // page wanted bare, and `surface_palette` guessed that shade from the
    // control's own text colour, so a white icon glyph turned it black on a
    // white page.
    let face: Option<Rgba> = match ctl.bg {
        Some(c) => Some(c),
        None if ctl.appearance_none => None,
        None => Some(match ctl.kind {
            // Buttons get a raised face; text fields stay flat like the page.
            ControlKind::Submit | ControlKind::Reset | ControlKind::Button | ControlKind::File
            | ControlKind::Select => mix(theme.bg, theme.text, 28).into(),
            _ => mix(theme.bg, theme.text, 8).into(),
        }),
    };

    // The page's own background image sits on the face, under the frame and
    // any text — that is where an icon-only button keeps its icon.
    let bg_img = |ops: &mut Vec<DrawOp>| {
        if let Some((key, layer)) = ctl.bg_img {
            ops.push(DrawOp::BgImage {
                x, y: top, w, h, clip: (x, top, w, h), key,
                repeat: layer.repeat,
                pos: layer.pos,
                size: layer.size,
                tint: None,
                filter: 0,
            });
        }
    };
    match ctl.kind {
        // Ein Radioknopf ist RUND, ein Kaestchen eckig. Das ist keine
        // Geschmacksfrage: die Form IST die Bedeutung — rund heisst „eine aus
        // dieser Gruppe", eckig heisst „unabhaengig an oder aus". Beide als
        // Quadrat zu malen nimmt dem Benutzer die Auskunft, ob seine Wahl die
        // anderen ausschliesst.
        ControlKind::Radio => {
            let r = [(w.min(h) as f32) / 2.0; 4];
            if let Some(face) = face {
                ops.push(DrawOp::RoundRect { x, y: top, w, h, r, color: face, ring: 0.0 });
            }
            bg_img(ops);
            let bw = ctl.border[0].w.max(1) as f32;
            ops.push(DrawOp::RoundRect { x, y: top, w, h, r, color: border, ring: bw });
            if ctl.checked {
                let i = (w / 4).max(2);
                let (iw, ih) = (w - 2 * i, h - 2 * i);
                ops.push(DrawOp::RoundRect {
                    x: x + i, y: top + i, w: iw, h: ih,
                    r: [(iw.min(ih) as f32) / 2.0; 4],
                    color: theme.link.into(), ring: 0.0,
                });
            }
        }
        ControlKind::Checkbox => {
            if let Some(face) = face {
                face_op(ops, face);
            }
            bg_img(ops);
            frame(ops);
            if ctl.checked {
                let i = (w / 4).max(2);
                ops.push(DrawOp::Rect {
                    x: x + i,
                    y: top + i,
                    w: w - 2 * i,
                    h: h - 2 * i,
                    color: theme.link.into(),
                });
            }
        }
        _ => {
            if let Some(face) = face {
                face_op(ops, face);
            }
            bg_img(ops);
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
                let color = if ctl.ghost { theme.muted.into() } else { ctl.style.color };
                for line in wrap_lines(font, &ctl.text, ctl.style.size, inner_w, rows as usize) {
                    ops.push(DrawOp::Text {
                        x: tx,
                        y: ly,
                        size: ctl.style.size,
                        color,
                        bold: ctl.style.bold,
                        italic: ctl.style.italic,
                        mono: ctl.style.mono,
                        sp: ctl.style.sp,
                        text: line,
                    });
                    ly += lh;
                }
                controls.push(rect(ops, ctl));
                return;
            }
            if !ctl.text.is_empty() {
                // Clip an over-long value to the box. WHICH END is dropped is
                // not a detail: a field being typed into must keep its tail,
                // where the caret is — but a LABEL must keep its head, because
                // it is a name, and a name clipped at the front is a different
                // word. Google's consent buttons read "lle ablehnen".
                let inner = (w - ctl.pad_l - ctl.pad_r).max(0) as f32;
                let text = if ctl.kind.is_submit() || ctl.kind == ControlKind::File {
                    clip_text_head(font, &ctl.text, ctl.style.size, inner)
                } else {
                    clip_text_tail(font, &ctl.text, ctl.style.size, inner)
                };
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
                    color: if ctl.ghost { theme.muted.into() } else { ctl.style.color },
                    bold: ctl.style.bold,
                    italic: ctl.style.italic,
                    mono: ctl.style.mono,
                    sp: ctl.style.sp,
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
                    color: theme.link.into(),
                });
            }
        }
    }
    controls.push(rect(ops, ctl));
}

/// Paint a control's frame: each side its own width and colour, `ua` standing
/// in wherever the page named none. A side the page suppressed (`border: none`,
/// `border-color: transparent`) paints nothing at all.
///
/// Focus is the one thing the page cannot take away: a control with no frame
/// left still gets a 1px ring while it has the keyboard, because that ring is
/// an OUTLINE — it says where typing goes, and a page hiding its border never
/// meant to hide that.
fn stroke_frame(ops: &mut Vec<DrawOp>, x: i32, y: i32, w: i32, h: i32, sides: &[CtlSide; 4], ua: Rgba, focused: bool) {
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

fn stroke_rect(ops: &mut Vec<DrawOp>, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
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
/// Trim from the END until it fits — for a label, which is read from the
/// front. The counterpart of `clip_text_tail`, which trims from the front for
/// a field whose caret is at the back.
fn clip_text_head(font: &Font, text: &str, size: f32, max_w: f32) -> String {
    if measure(font, text, size) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut end = chars.len();
    while end > 0 {
        let s: String = chars[..end].iter().collect();
        if measure(font, &s, size) <= max_w {
            return s;
        }
        end -= 1;
    }
    String::new()
}

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
        // Die Deckung der Inline-Vorfahren steckt HIER in der Farbe: ein
        // Inline-Kasten hat keinen Befehlsbereich, ueber den sie spaeter
        // gelegt werden koennte (siehe `ComputedStyle::inline_fade`). Zwei
        // Laeufe verschmelzen nur bei gleicher `RunStyle` — die verschiedene
        // Alpha trennt sie also von selbst, ohne dass der Verschmelzer davon
        // wissen muss.
        let rs = RunStyle { hidden: st.hidden, transparent: st.transparent, size: st.font_px,
            color: faded(st.color, st.inline_fade),
            deco_color: st.deco_color.map(|c| faded(c, st.inline_fade)), bold: st.bold, italic: st.italic, mono: st.mono, valign: st.valign, deco: st.deco, break_word: st.break_word, nowrap: st.nowrap, lh: st.line_height.px(st.font_px).unwrap_or(0.0), sp: (st.letter_spacing, st.word_spacing) };
        let mut word = String::new();
        for ch in raw.chars() {
            if is_css_space(ch) {
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
    fn image(&mut self, src: String, w: i32, h: i32, href: Option<&str>, alt: String, hidden: bool, transparent: bool, fit: ObjectFit, filter: u16, deco: Option<InlineBox>) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Image { src, w, h, href: href.map(|s| s.to_string()), alt, space_before, hidden, transparent, fit, filter, deco: deco.map(alloc::boxed::Box::new) });
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
            deco_color: st.deco_color,
            break_word: st.break_word,
            nowrap: st.nowrap,
            lh: st.line_height.px(st.font_px).unwrap_or(0.0),
            sp: (st.letter_spacing, st.word_spacing),
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
        hover_boxes: &mut Vec<HoverBox>,
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
                        pen += space_width(
                            fonts.pick(b.st.bold, b.st.italic, b.st.mono),
                            b.st.font_px,
                            (b.st.letter_spacing, b.st.word_spacing),
                        );
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
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects, hover_boxes);
                    }
                    let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                    pen = bl as f32;
                    right = br as f32;
                    line_ascent = 0.0;
                    line_below = 0.0;
                    gap = 0.0;
                }
                Item::Word { text, style, href, space_before } => {
                    let ww = measure_sp(face(style), text, style.size, style.sp);
                    let sw =
                        if *space_before { space_width(face(style), style.size, style.sp) } else { 0.0 };
                    // `white-space: nowrap`: the space before this word is not a
                    // break opportunity, so the line overflows instead.
                    if !style.nowrap && !line.is_empty() && pen + sw + ww > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects, hover_boxes);
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
                            let mut n = fit_prefix(f, rest, style.size, right - pen - lead, style.sp);
                            if n == 0 {
                                if !line.is_empty() {
                                    *last_baseline = Some(y + line_ascent as i32);
                                    break_frags(&mut open, &mut frags, pen);
                                    y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects, hover_boxes);
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
                            pen += lead + measure_sp(f, head, style.size, style.sp);
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
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX, (0.0, 0.0)) } else { 0.0 };
                    if !line.is_empty() && pen + sw + b.w as f32 > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects, hover_boxes);
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
                Item::Image { src, w: iw, h: ih, href, alt, space_before, hidden, transparent, fit, filter, deco } => {
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
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX, (0.0, 0.0)) } else { 0.0 };
                    if !line.is_empty() && pen + sw + bw as f32 > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects, hover_boxes);
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
                        fit: *fit,
                        filter: *filter,
                        deco: deco.clone(),
                    });
                    pen += lead + fl + bw as f32 + fr;
                    line_ascent = line_ascent.max(bh as f32);
                    gap = gap.max(bh as f32 + 2.0);
                }
                Item::Control { ctl, space_before } => {
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX, (0.0, 0.0)) } else { 0.0 };
                    if !line.is_empty() && pen + sw + ctl.w as f32 > right {
                        *last_baseline = Some(y + line_ascent as i32);
                        break_frags(&mut open, &mut frags, pen);
                        y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls, inspects, hover_boxes);
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
            y = emit_line(fonts, theme, &mut line, &mut frags, &self.boxes, y, line_ascent, gap, align_dx(a, rtl, pen, right), ops, links, controls, inspects, hover_boxes);
        }
        y
    }
}

/// One item placed on the current line: a same-style text run, an image, or a
/// form control (borrowed from the inline run — it is only measured once).
enum Placed<'a> {
    Text(Seg),
    Atomic { x: i32, box_: AtomicBox },
    Image { x: i32, w: i32, h: i32, src: String, href: Option<String>, alt: String, hidden: bool, transparent: bool, fit: ObjectFit, filter: u16, deco: Option<alloc::boxed::Box<InlineBox>> },
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
        ListStyle::LowerGreek => alloc::format!("{}.", greek_counter(n)),
        ListStyle::Armenian => alloc::format!("{}.", additive_counter(n, &ARMENIAN)),
        ListStyle::Georgian => alloc::format!("{}.", additive_counter(n, &GEORGIAN)),
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
        ListStyle::LowerGreek => greek_counter(n),
        ListStyle::Armenian => additive_counter(n, &ARMENIAN),
        ListStyle::Georgian => additive_counter(n, &GEORGIAN),
        // Pad 1..9 to two digits (`01`); everything else is plain decimal.
        ListStyle::DecimalLeadingZero => alloc::format!("{n:02}"),
        ListStyle::Disc => "•".into(),
        ListStyle::Circle => "◦".into(),
        ListStyle::Square => "▪".into(),
        // As `counter()` text these are characters, not the drawn triangle a
        // `<summary>` marker gets — css-counter-styles-3 names exactly these.
        ListStyle::DisclosureClosed => "▸".into(),
        ListStyle::DisclosureOpen => "▾".into(),
        ListStyle::None => String::new(),
        _ => alloc::format!("{n}"),
    }
}

/// `lower-greek`: bijective base-24 over α..ω with FINAL SIGMA left out
/// (css-counter-styles-3 §6.1) — 24 letters, not the 25 the block holds.
fn greek_counter(n: i32) -> String {
    const G: [char; 24] = [
        '\u{3b1}', '\u{3b2}', '\u{3b3}', '\u{3b4}', '\u{3b5}', '\u{3b6}', '\u{3b7}', '\u{3b8}',
        '\u{3b9}', '\u{3ba}', '\u{3bb}', '\u{3bc}', '\u{3bd}', '\u{3be}', '\u{3bf}', '\u{3c0}',
        '\u{3c1}', '\u{3c3}', '\u{3c4}', '\u{3c5}', '\u{3c6}', '\u{3c7}', '\u{3c8}', '\u{3c9}',
    ];
    if n < 1 {
        return alloc::format!("{n}");
    }
    let mut out: Vec<char> = Vec::new();
    let mut v = n;
    while v > 0 {
        out.push(G[((v - 1) % 24) as usize]);
        v = (v - 1) / 24;
    }
    out.reverse();
    out.into_iter().collect()
}

/// An additive counter style: the largest weight that fits, repeatedly. Used
/// by `armenian` and `georgian`, which have no positional notation at all.
/// Out of range falls back to decimal, as CSS requires of an exhausted style.
fn additive_counter(n: i32, table: &[(i32, char)]) -> String {
    let max: i32 = table.iter().map(|(w, _)| *w).max().unwrap_or(0) * 10 - 1;
    if n < 1 || n > max {
        return alloc::format!("{n}");
    }
    let mut v = n;
    let mut out = String::new();
    for &(w, ch) in table {
        while v >= w {
            out.push(ch);
            v -= w;
        }
    }
    out
}

const ARMENIAN: [(i32, char); 36] = [
    (9000, '\u{554}'), (8000, '\u{553}'), (7000, '\u{552}'), (6000, '\u{551}'), (5000, '\u{550}'),
    (4000, '\u{54f}'), (3000, '\u{54e}'), (2000, '\u{54d}'), (1000, '\u{54c}'),
    (900, '\u{54b}'), (800, '\u{54a}'), (700, '\u{549}'), (600, '\u{548}'), (500, '\u{547}'),
    (400, '\u{546}'), (300, '\u{545}'), (200, '\u{544}'), (100, '\u{543}'),
    (90, '\u{542}'), (80, '\u{541}'), (70, '\u{540}'), (60, '\u{53f}'), (50, '\u{53e}'),
    (40, '\u{53d}'), (30, '\u{53c}'), (20, '\u{53b}'), (10, '\u{53a}'),
    (9, '\u{539}'), (8, '\u{538}'), (7, '\u{537}'), (6, '\u{536}'), (5, '\u{535}'),
    (4, '\u{534}'), (3, '\u{533}'), (2, '\u{532}'), (1, '\u{531}'),
];

const GEORGIAN: [(i32, char); 37] = [
    (10000, '\u{10f5}'), (9000, '\u{10f0}'), (8000, '\u{10ef}'), (7000, '\u{10f4}'),
    (6000, '\u{10ee}'), (5000, '\u{10ed}'), (4000, '\u{10ec}'), (3000, '\u{10eb}'),
    (2000, '\u{10ea}'), (1000, '\u{10e9}'),
    (900, '\u{10e8}'), (800, '\u{10e7}'), (700, '\u{10e6}'), (600, '\u{10e5}'), (500, '\u{10e4}'),
    (400, '\u{10f3}'), (300, '\u{10e2}'), (200, '\u{10e1}'), (100, '\u{10e0}'),
    (90, '\u{10df}'), (80, '\u{10de}'), (70, '\u{10dd}'), (60, '\u{10f2}'), (50, '\u{10dc}'),
    (40, '\u{10db}'), (30, '\u{10da}'), (20, '\u{10d9}'), (10, '\u{10d8}'),
    (9, '\u{10d7}'), (8, '\u{10f1}'), (7, '\u{10d6}'), (6, '\u{10d5}'), (5, '\u{10d4}'),
    (4, '\u{10d3}'), (3, '\u{10d2}'), (2, '\u{10d1}'), (1, '\u{10d0}'),
];

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
    let color = style.deco_color.unwrap_or(style.color);
    if color.a == 0 {
        return;
    }
    let mut line = |y: i32| ops.push(DrawOp::Rect { x, y, w, h, color });
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

/// Resolve ONE element's computed style outside a layout, with a given pointer
/// state — by descending from the root exactly the way `layout` does, through
/// the same `style::resolve`.
///
/// `subtree` also resolves the element's descendants, up to that many. A hover
/// rule does not only restyle what the pointer is IN — `nav:hover a`, every
/// dropdown on the web, styles a DESCENDANT from a state that lives on the
/// ancestor. Resolving only the carrier left those runs painted in the resting
/// colour with nothing to say that anything had been missed.
///
/// The descent is the price of not keeping a computed style per element alive
/// between layouts: it is one `resolve` per ancestor plus one per preceding
/// sibling at each level, against ~8300 for a page. It is not a second copy of
/// the cascade; the function it calls is the one layout calls.
pub fn resolve_out_of_band(
    dom: &Dom,
    sheet: &Stylesheet,
    theme: &Theme,
    width: u32,
    viewport_h: u32,
    seq: u32,
    hover: &[u32],
    subtree: usize,
    out: &mut Vec<StyleProbe>,
) -> Option<StyleProbe> {
    let mut initial = ComputedStyle::root(theme);
    initial.vw = width as f32;
    initial.vh = viewport_h as f32;
    let html_el = dom.root_element();
    let mut root = style::resolve(
        &ElemInfo::of_hovered(html_el, hover), &initial, theme, sheet, &[], &[], 0, width as f32,
    );
    root.rem_base = root.font_px;
    let mut anc = vec![ElemInfo::of_hovered(html_el, hover)];
    let own = if html_el.seq == seq {
        Some(StyleProbe { own: root, before: None, after: None })
    } else {
        descend(html_el, &root, &mut anc, seq, u32::MAX, sheet, theme, width as f32, hover)
    }?;
    if subtree > 0 {
        let el = find_seq(html_el, seq)?;
        if !resolve_kids(el, &own.own, &mut anc, sheet, theme, width as f32, hover, subtree, out) {
            return None; // bigger than the fast path is worth
        }
    }
    Some(own)
}

/// One element's computed style, plus the pseudo-elements that hang off it.
///
/// `::before`/`::after` are here because a hover rule reaches them —
/// MediaWiki underlines the article tabs with `a:hover::after{background}`,
/// so a pass that looked only at real elements saw a colour change on the
/// text and quietly missed the line under it. Their boxes are generated
/// during layout and never recorded, so a changed one gives up.
#[derive(Clone, Copy)]
pub struct StyleProbe {
    pub own: ComputedStyle,
    pub before: Option<ComputedStyle>,
    pub after: Option<ComputedStyle>,
}

/// Do these two probes paint their pseudo-elements differently?
pub fn pseudos_differ(a: &StyleProbe, b: &StyleProbe) -> bool {
    let one = |x: &Option<ComputedStyle>, y: &Option<ComputedStyle>| match (x, y) {
        (None, None) => false,
        (Some(p), Some(q)) => box_differs(p, q) || p.color != q.color || p.deco != q.deco,
        _ => true,
    };
    one(&a.before, &b.before) || one(&a.after, &b.after)
}

/// Resolve one element's style and its two pseudo-elements.
#[allow(clippy::too_many_arguments)]
fn probe_of<'a>(
    e: &'a Element,
    own: ComputedStyle,
    anc: &[ElemInfo<'a>],
    prev: &[ElemInfo<'a>],
    sib_count: u32,
    sheet: &Stylesheet,
    theme: &Theme,
    vw: f32,
    hover: &[u32],
) -> StyleProbe {
    let info = ElemInfo::of_hovered(e, hover);
    let one = |which| {
        style::resolve_pseudo(&info, &own, theme, sheet, anc, prev, sib_count, vw, which)
            .map(|(_, st)| st)
    };
    StyleProbe {
        own,
        before: one(crate::css::PseudoElem::Before),
        after: one(crate::css::PseudoElem::After),
    }
}

/// The element with this `seq`, or `None`.
fn find_seq(el: &Element, seq: u32) -> Option<&Element> {
    if el.seq == seq {
        return Some(el);
    }
    for c in &el.children {
        if let Node::Element(e) = c {
            if let Some(f) = find_seq(e, seq) {
                return Some(f);
            }
        }
    }
    None
}

/// The element with this `seq`, for callers outside this module.
pub fn find_seq_pub(dom: &Dom, seq: u32) -> Option<&Element> {
    find_seq(&dom.root, seq)
}

/// Everything an element's subtree says, whitespace collapsed — see
/// `HoverRepaint::text`.
pub fn subtree_text(el: &Element, out: &mut String) {
    for c in &el.children {
        match c {
            Node::Text(t) => {
                for w in t.split_whitespace() {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(w);
                }
            }
            Node::Element(e) => subtree_text(e, out),
        }
    }
}

/// Resolve every descendant of `el`, appending to `out`. False once more than
/// `budget` of them exist — a subtree that large is not worth repainting one
/// element at a time, and laying out is the honest answer.
#[allow(clippy::too_many_arguments)]
fn resolve_kids<'a>(
    el: &'a Element,
    own: &ComputedStyle,
    anc: &mut Vec<ElemInfo<'a>>,
    sheet: &Stylesheet,
    theme: &Theme,
    vw: f32,
    hover: &[u32],
    budget: usize,
    out: &mut Vec<StyleProbe>,
) -> bool {
    let kids: Vec<&Element> = el
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    let sib_count = kids.len() as u32;
    let mut prev: Vec<ElemInfo> = Vec::new();
    anc.push(ElemInfo::of_hovered(el, hover));
    for e in &kids {
        if out.len() >= budget {
            anc.pop();
            return false;
        }
        let st = style::resolve(&ElemInfo::of_hovered(e, hover), own, theme, sheet, anc, &prev, sib_count, vw);
        out.push(probe_of(e, st, anc, &prev, sib_count, sheet, theme, vw, hover));
        if !resolve_kids(e, &st, anc, sheet, theme, vw, hover, budget, out) {
            anc.pop();
            return false;
        }
        prev.push(ElemInfo::of_hovered(e, hover));
    }
    anc.pop();
    true
}

/// Walk into the child whose subtree holds `seq`. Seqs are handed out in
/// document order, so a child's subtree is `[child.seq, next_sibling.seq)` —
/// which is what `bound` carries for the last child.
#[allow(clippy::too_many_arguments)]
fn descend<'a>(
    el: &'a Element,
    parent: &ComputedStyle,
    anc: &mut Vec<ElemInfo<'a>>,
    seq: u32,
    bound: u32,
    sheet: &Stylesheet,
    theme: &Theme,
    vw: f32,
    hover: &[u32],
) -> Option<StyleProbe> {
    let kids: Vec<&Element> = el
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    let sib_count = kids.len() as u32;
    let mut prev: Vec<ElemInfo> = Vec::new();
    for (i, e) in kids.iter().enumerate() {
        let info = ElemInfo::of_hovered(e, hover);
        let st = style::resolve(&info, parent, theme, sheet, anc, &prev, sib_count, vw);
        if e.seq == seq {
            return Some(probe_of(e, st, anc, &prev, sib_count, sheet, theme, vw, hover));
        }
        let next = kids.get(i + 1).map_or(bound, |n| n.seq);
        if seq > e.seq && seq < next {
            anc.push(ElemInfo::of_hovered(e, hover));
            let r = descend(e, &st, anc, seq, next, sheet, theme, vw, hover);
            // On success the chain STAYS — the caller resolves the element's
            // descendants next, and they need their real ancestors. Popping it
            // here left `.tabs li:hover a` unable to match anything, so a rule
            // that styles a descendant quietly did nothing.
            if r.is_none() {
                anc.pop();
            }
            return r;
        }
        prev.push(ElemInfo::of_hovered(e, hover));
    }
    None
}

/// Repaint one element in a finished display list, for a pointer change that
/// cannot move anything (`css::Class::Paint`).
///
/// The point is what it does NOT do: no parse, no cascade over the page, no
/// box arithmetic. A pointer entering a link on Wikipedia's Main_Page changes
/// 1 op of 723 and adds 1 more — measured — and used to cost a full layout,
/// 25 ms on the dev box and ~1950 ms on the device, for 0.06 % of the viewport.
///
/// Correctness is CHECKED, not argued. Everything this touches is regenerated
/// through the very functions layout used (`bg_ops`, `border_ops`,
/// `push_decorations`), and the old state is regenerated too and has to be
/// found in the list exactly where it is replaced. Anything ambiguous returns
/// `false` and the caller lays out, which is what it did before.
/// Ein Steuerelement neu malen, ohne die Seite neu auszulegen.
///
/// **Warum das die groesste einzelne Zahl in beak ist:** jeder Tastendruck in
/// einem Feld war bisher ein `bump_content_gen("form-key")`, also ein volles
/// Auslegen des Dokuments. Auf Wikipedia sind das 280 ms — je Zeichen. Was
/// sich dabei wirklich aendert, ist der Malbereich EINES Kastens.
///
/// Erlaubt ist das, weil `:focus`, `:focus-within` und `:active` bei uns in
/// `never_matches` stehen: Tastaturbesitz kann durch die Kaskade gar nichts
/// umstylen. `:checked` kann es sehr wohl — der Kaestchen-Trick ist
/// `input:checked ~ .menu{display:block}` —, also entscheidet dort
/// `may_restyle`, und im Zweifel wird ausgelegt.
///
/// Die Geometrie wird NICHT neu gerechnet: der Kasten behaelt seine Masse. Ein
/// Wert, der breiter ist als sein Feld, wird beschnitten (wie vorher) statt
/// mitzuwandern — das ist die benannte Grenze dieses Weges.
pub fn repaint_controls(
    lay: &mut Layout,
    fonts: &crate::fonts::Fonts,
    theme: &Theme,
    state: &crate::forms::FormState,
    may_restyle: &dyn Fn(u32) -> bool,
) -> Result<(), &'static str> {
    // Erst planen, dann anwenden — ein Lauf, der auf halbem Weg aufgibt, liesse
    // eine halb neu gemalte Seite stehen ([[repaint_hover]] macht es genauso).
    let mut plan: Vec<(usize, Vec<DrawOp>, CtlBox)> = Vec::new();
    for (i, c) in lay.controls.iter().enumerate() {
        let old = &c.paint;
        let focused = state.focus == Some(old.seq);
        let checked = state.checked_or(old.seq, old.checked);
        // Der angezeigte Text: nur wer getippt hat, aendert ihn.
        let (text, ghost, raw) = match state.value_set(old.seq) {
            None => (old.text.clone(), old.ghost, None),
            Some(v) => match old.kind {
                ControlKind::Password => (repeat_char('•', v.chars().count()), false, Some(v)),
                ControlKind::Text | ControlKind::TextArea if v.is_empty() => {
                    (old.placeholder.clone(), true, Some(v))
                }
                // Ein `<select>` zeigt die Beschriftung seiner gewaehlten
                // Option, und die steht im Baum, nicht im Zustand. Auslegen.
                ControlKind::Select => return Err("select label comes from the tree"),
                _ => (v.to_string(), false, Some(v)),
            },
        };
        let caret = if focused && old.kind.is_text() {
            let r = raw.unwrap_or(&old.text);
            Some(r[..state.caret.min(r.len())].chars().count())
        } else {
            None
        };
        if focused == old.focused && checked == old.checked && text == old.text
            && ghost == old.ghost && caret == old.caret
        {
            continue;
        }
        if checked != old.checked && may_restyle(old.seq) {
            return Err("a :checked rule reaches this control");
        }
        let mut next = old.clone();
        next.focused = focused;
        next.checked = checked;
        next.text = text;
        next.ghost = ghost;
        next.caret = caret;
        let mut ops = Vec::new();
        let mut throwaway = Vec::new();
        paint_control(fonts, theme, &next, c.x, c.y, &mut ops, &mut throwaway);
        plan.push((i, ops, next));
    }
    if plan.is_empty() {
        return Ok(());
    }
    // Von hinten nach vorn: eine Ersetzung verschiebt nur, was DAHINTER liegt,
    // und die Eintraege davor bleiben gueltig.
    plan.sort_by_key(|(i, ..)| core::cmp::Reverse(lay.controls[*i].at));
    for (i, ops, next) in plan {
        let (at, len) = (lay.controls[i].at, lay.controls[i].len);
        let delta = ops.len() as isize - len as isize;
        lay.ops.splice(at..at + len, ops);
        lay.controls[i].len = (len as isize + delta) as usize;
        lay.controls[i].paint = next;
        if delta != 0 {
            for other in lay.controls.iter_mut() {
                if other.at > at {
                    other.at = (other.at as isize + delta) as usize;
                }
            }
        }
    }
    Ok(())
}

pub fn repaint_hover(
    lay: &mut Layout,
    fonts: &crate::fonts::Fonts,
    groups: &[HoverRepaint],
) -> Result<(), &'static str> {
    // Plan every edit first and apply nothing until all of them are known to
    // be possible: a pass that patched as it went left a half-repainted page
    // behind whenever it gave up in the middle.
    let mut edits: Vec<Edit> = Vec::new();
    for g in groups {
        if g.boxes.is_empty() {
            return Err("no recorded box");
        }
        plan_one(lay, fonts, g, &mut edits)?;
    }
    // Nothing to do is a RESULT, not a failure — and the common one. Most of a
    // page sits inside something a `:hover` rule COULD match without any rule
    // actually applying, and `border-color` on a side with no width is a style
    // that changes and paints nothing. Each of those pointer moves used to cost
    // a full layout that produced a byte-identical display list.
    //
    // What must not pass silently is a change this pass did not account for —
    // and that is decided per element in `plan_one`, by comparing the ops the
    // two styles PRODUCE rather than the fields they differ in.
    // Two elements laying claim to the same op cannot both be right.
    edits.sort_by_key(|e| e.at);
    // Two elements laying claim to the same slot cannot both be right, and two
    // insertions at the same index have no order between them.
    if edits.windows(2).any(|w| w[0].at + w[0].len > w[1].at || w[0].at == w[1].at) {
        return Err("two elements claim the same op");
    }
    for e in edits.into_iter().rev() {
        lay.ops.splice(e.at..e.at + e.len, e.ops);
    }
    Ok(())
}

/// One planned replacement: `ops[at .. at+len]` becomes `ops`.
struct Edit {
    at: usize,
    len: usize,
    ops: Vec<DrawOp>,
}

/// One element the pointer entered or left, with everything needed to repaint
/// it: where it painted, and how it and its subtree are styled before and after.
pub struct HoverRepaint {
    /// The element's own fragments — one per line for an inline box — each
    /// with the anchor that says where its decoration belongs.
    pub boxes: Vec<HoverBox>,
    /// The element itself, then every descendant, before and after. The
    /// unchanged ones are here too: a run painted in a colour some OTHER
    /// element also uses cannot be told apart, and this pass has to know that
    /// rather than recolour the wrong text.
    pub pairs: Vec<(StyleProbe, StyleProbe)>,
    /// Everything this element's subtree says, whitespace collapsed.
    ///
    /// A rectangle is not proof of ownership: an element's border box can
    /// enclose text that belongs to something else entirely — a table cell's
    /// box contains the footnote marker of a link that is not inside it — and
    /// two links on a page share a colour. Requiring the run to be part of what
    /// this element actually SAYS is what tells them apart. A run that is not
    /// found is left alone, and if that leaves nothing to do the page is laid
    /// out instead.
    pub text: String,
}

/// Is `(x, y)` inside any of the element's fragments?
fn in_boxes(boxes: &[HoverBox], x: i32, y: i32) -> bool {
    boxes.iter().any(|b| x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h)
}

/// The box decoration this style paints at `rect`, in display-list order.
///
/// A background IMAGE is deliberately not resolved: its key belongs to the
/// layout, and a repaint that guessed one would paint the wrong picture. A
/// style that wants one gives up instead.
fn deco_ops(st: &ComputedStyle, b: &HoverBox) -> Option<Vec<DrawOp>> {
    if st.bg_layer.image.is_some() || st.mask_layer.image.is_some() {
        return None;
    }
    let (x, y, w, h) = b.paint;
    if st.hidden || st.transparent || w <= 0 || h <= 0 {
        return Some(Vec::new());
    }
    let mut v = Vec::new();
    if b.shadow {
        shadow_ops(st, x, y, w, h, &mut v);
    }
    bg_ops(st, None, None, x, y, w, h, &mut v);
    inset_shadow_ops(st, x, y, w, h, &mut v);
    border_ops(st, x, y, w, h, b.sides, &mut v);
    Some(v)
}

/// Do these two styles paint the element's own BOX differently? Text aside,
/// this is everything a box draws for itself.
fn box_differs(a: &ComputedStyle, b: &ComputedStyle) -> bool {
    a.bg != b.bg
        || a.bg_layer != b.bg_layer
        || a.mask_layer != b.mask_layer
        || a.border_top != b.border_top
        || a.border_right != b.border_right
        || a.border_bottom != b.border_bottom
        || a.border_left != b.border_left
        || a.outline != b.outline
        || a.outline_offset != b.outline_offset
        || a.radius != b.radius
        || a.shadow != b.shadow
        || a.hidden != b.hidden
        || a.transparent != b.transparent
}

/// Two ops the rasteriser would draw identically.
fn op_eq(a: &DrawOp, b: &DrawOp) -> bool {
    match (a, b) {
        (
            DrawOp::Rect { x: ax, y: ay, w: aw, h: ah, color: ac },
            DrawOp::Rect { x: bx, y: by, w: bw, h: bh, color: bc },
        ) => (ax, ay, aw, ah, ac) == (bx, by, bw, bh, bc),
        (
            DrawOp::RoundRect { x: ax, y: ay, w: aw, h: ah, r: ar, color: ac, ring: ag },
            DrawOp::RoundRect { x: bx, y: by, w: bw, h: bh, r: br, color: bc, ring: bg },
        ) => (ax, ay, aw, ah, ac) == (bx, by, bw, bh, bc) && ar == br && ag == bg,
        (
            DrawOp::Text { x: ax, y: ay, size: asz, color: ac, bold: ab, italic: ai, mono: am, sp: asp, text: at },
            DrawOp::Text { x: bx, y: by, size: bsz, color: bc, bold: bb, italic: bi, mono: bm, sp: bsp, text: bt },
        ) => (ax, ay, ac, ab, ai, am, at) == (bx, by, bc, bb, bi, bm, bt) && asz == bsz && asp == bsp,
        _ => false,
    }
}

/// Where the op with this key sits, if it sits there exactly once. The key
/// names an op by content, so it survives every insertion, clip and z-reorder
/// that happened after it was recorded.
fn find_key_once(ops: &[DrawOp], key: OpKey) -> Option<usize> {
    let mut at = None;
    for (i, op) in ops.iter().enumerate() {
        if op_key(op) == key {
            if at.is_some() {
                return None;
            }
            at = Some(i);
        }
    }
    at
}

/// Where `want` sits in `ops`, if it sits there exactly once.
fn find_once(ops: &[DrawOp], want: &[DrawOp]) -> Option<usize> {
    if want.is_empty() || want.len() > ops.len() {
        return None;
    }
    let mut at = None;
    for i in 0..=ops.len() - want.len() {
        if (0..want.len()).all(|k| op_eq(&ops[i + k], &want[k])) {
            if at.is_some() {
                return None; // two boxes look alike — patch neither
            }
            at = Some(i);
        }
    }
    at
}

/// Is this run part of what the element says? Whitespace is collapsed on both
/// sides because a run is already wrapped and the source is not.
fn says(subtree: &str, run: &str) -> bool {
    let norm = |t: &str| {
        let mut o = String::with_capacity(t.len());
        for w in t.split_whitespace() {
            if !o.is_empty() {
                o.push(' ');
            }
            o.push_str(w);
        }
        o
    };
    let r = norm(run);
    !r.is_empty() && subtree.contains(&r)
}

/// One text substitution: runs painted in `off` become `on`, and their
/// decorations are re-emitted.
struct Sub {
    off: ComputedStyle,
    on: ComputedStyle,
}


/// `Err` = cannot be done with certainty, lay out instead. The reason is
/// carried out so a page that keeps taking the slow path can say WHY once,
/// rather than looking like the feature simply does not work.
fn plan_one(
    lay: &Layout,
    fonts: &crate::fonts::Fonts,
    g: &HoverRepaint,
    edits: &mut Vec<Edit>,
) -> Result<(), &'static str> {
    // A DESCENDANT's pseudo-element has no recorded rect — only the carrier's
    // do — so one that repaints is out of reach.
    if g.pairs[1..].iter().any(|(a, b)| pseudos_differ(a, b)) {
        return Err("a descendant's pseudo-element repaints");
    }
    let (off_probe, on_probe) = (&g.pairs[0].0, &g.pairs[0].1);
    let (own_off, own_on) = (&off_probe.own, &on_probe.own);

    // ── every box this element paints: its own, and its pseudo-elements ───
    // Each rect is a border box, which a paint-only change cannot have moved.
    for b in &g.boxes {
        let (off, on) = match b.pseudo {
            PseudoElem::None => (own_off, own_on),
            PseudoElem::Before => match (&off_probe.before, &on_probe.before) {
                (Some(a), Some(c)) => (a, c),
                _ => return Err("a pseudo-element appears or vanishes"),
            },
            PseudoElem::After => match (&off_probe.after, &on_probe.after) {
                (Some(a), Some(c)) => (a, c),
                _ => return Err("a pseudo-element appears or vanishes"),
            },
        };
        if !box_differs(off, on) {
            continue;
        }
        // A pseudo's `content` string is not part of what the element SAYS, so
        // its own run cannot be identified the way the element's runs are.
        if b.has_text && (off.color != on.color || off.deco != on.deco) {
            return Err("a pseudo-element's own text would have to be repainted");
        }
        let (Some(was), Some(now)) = (deco_ops(off, b), deco_ops(on, b)) else {
            return Err("a background image");
        };
        if was.len() == now.len() && was.iter().zip(&now).all(|(a, c)| op_eq(a, c)) {
            continue; // this box looks the same in both states
        }
        if !was.is_empty() {
            // There is something to replace, and it has to be found where it is
            // replaced. The two lists may differ in length: a border that gains
            // a side, a background that goes away entirely.
            let Some(at) = find_once(&lay.ops, &was) else {
                return Err("the old decoration is not findable");
            };
            edits.push(Edit { at, len: was.len(), ops: now });
        } else {
            // Nothing to replace — a background that only exists under the
            // pointer. A box puts its decoration in AHEAD of everything it
            // paints; an absolutely positioned pseudo goes in AFTER everything
            // its element painted. The anchor says by what, and which side.
            let Some(key) = b.anchor else {
                return Err("the box painted nothing to anchor to");
            };
            let Some(at) = find_key_once(&lay.ops, key) else {
                return Err("the anchor is gone or ambiguous");
            };
            edits.push(Edit { at: at + b.anchor_after as usize, len: 0, ops: now });
        }
    }
    // A pseudo-element that repaints but has no rectangle of its own: the
    // inline and flex paths generate one without recording it.
    for (kind, off, on) in [
        (PseudoElem::Before, &off_probe.before, &on_probe.before),
        (PseudoElem::After, &off_probe.after, &on_probe.after),
    ] {
        let (Some(off), Some(on)) = (off, on) else {
            if off.is_some() != on.is_some() {
                return Err("a pseudo-element appears or vanishes");
            }
            continue;
        };
        if (box_differs(off, on) || off.color != on.color || off.deco != on.deco)
            && !g.boxes.iter().any(|b| b.pseudo == kind)
        {
            return Err("a pseudo-element repaints and has no rectangle");
        }
    }

    // A DESCENDANT that repaints its own box is out of reach: its rect was
    // never recorded, only the carrier's.
    if g.pairs[1..].iter().any(|(a, b)| box_differs(&a.own, &b.own)) {
        return Err("a descendant repaints its own box");
    }
    // Becoming invisible does not recolour anything — it takes whole ops out
    // of the list, text included, and puts them back later.
    if g.pairs.iter().any(|(a, b)| {
        (a.own.hidden, a.own.transparent, a.own.opacity_zero)
            != (b.own.hidden, b.own.transparent, b.own.opacity_zero)
    }) {
        return Err("something becomes invisible");
    }

    // ── the text inside it ────────────────────────────────────────────────
    let mut subs: Vec<Sub> = Vec::new();
    for (off, on) in g.pairs.iter().map(|(a, b)| (&a.own, &b.own)) {
        if off.color == on.color && off.deco == on.deco {
            continue;
        }
        // Two elements in this subtree painted in the same colour that must
        // now become different ones — a run cannot be assigned to either.
        if subs.iter().any(|s| s.off.color == off.color && (s.on.color != on.color || s.on.deco != on.deco)) {
            return Err("two elements share a colour and must not share the next");
        }
        subs.push(Sub { off: *off, on: *on });
    }
    // A run painted in a colour that some UNCHANGED element also uses would be
    // recoloured by mistake.
    if g.pairs.iter().map(|(a, b)| (&a.own, &b.own)).any(|(off, on)| {
        off.color == on.color && off.deco == on.deco && subs.iter().any(|s| s.off.color == off.color)
    }) {
        return Err("an unchanged element shares the colour");
    }
    if subs.is_empty() {
        return Ok(());
    }
    let sub_for = |op: &DrawOp| -> Option<&Sub> {
        let DrawOp::Text { x, y, color, text, .. } = op else { return None };
        if !in_boxes(&g.boxes, *x, *y) || !says(&g.text, text) {
            return None;
        }
        subs.iter().find(|s| s.off.color == *color)
    };

    // Recolouring can MERGE two runs. The line builder joins neighbouring
    // segments that share a face, so two runs the page painted apart —
    // `46° 58′ 50″ N, 8° 20′ 20″ O` split across three links — become ONE op
    // the moment they agree on a colour, with a single underline across the
    // whole thing instead of three. A patch cannot produce that.
    //
    // The test is deliberately blunt: give up whenever a repainted run ends up
    // looking like the run it TOUCHES. Whether they really merge also depends
    // on the `href` behind them, which the display list no longer carries — so
    // the only honest answer from here is "maybe", and maybe means lay out.
    let face = |op: &DrawOp| -> Option<(i32, Rgba, u32, bool, bool, bool)> {
        let DrawOp::Text { y, size, color, bold, italic, mono, .. } = op else { return None };
        let c = sub_for(op).map_or(*color, |s| s.on.color);
        Some((*y, c, size.to_bits(), *bold, *italic, *mono))
    };
    let right_edge = |op: &DrawOp| -> i32 {
        let DrawOp::Text { x, size, bold, italic, mono, sp, text, .. } = op else { return i32::MIN };
        x + ceil_i32(measure_sp(fonts.pick(*bold, *italic, *mono), text, *size, *sp))
    };
    let texts: Vec<usize> =
        (0..lay.ops.len()).filter(|i| matches!(lay.ops[*i], DrawOp::Text { .. })).collect();
    for w in texts.windows(2) {
        let (a, b) = (&lay.ops[w[0]], &lay.ops[w[1]]);
        if sub_for(a).is_none() && sub_for(b).is_none() {
            continue;
        }
        let DrawOp::Text { x: bx, .. } = b else { continue };
        if face(a) == face(b) && (right_edge(a) - bx).abs() <= 1 {
            return Err("recolouring would merge two runs");
        }
    }

    let mut touched = 0usize;
    for (i, op) in lay.ops.iter().enumerate() {
        let Some(sub) = sub_for(op) else { continue };
        let DrawOp::Text { x, y, size, bold, italic, mono, sp, text, .. } = op else { continue };
        // Everything `push_decorations` was given, recovered from the op it was
        // emitted next to — the run's own width and baseline, measured with the
        // same face at the same size AND the same spacing. No second copy of
        // the rule.
        let (x, y, size) = (*x, *y, *size);
        let font = fonts.pick(*bold, *italic, *mono);
        let run_w = ceil_i32(measure_sp(font, text, size, *sp));
        let baseline = y + ascent_i(font, size);
        let mut run = RunStyle {
            hidden: false,
            transparent: false,
            size,
            color: sub.off.color,
            bold: *bold,
            italic: *italic,
            mono: *mono,
            valign: sub.off.valign,
            deco: sub.off.deco,
            deco_color: sub.off.deco_color,
            break_word: false,
            nowrap: false,
            lh: 0.0,
            sp: *sp,
        };
        let mut was = Vec::new();
        push_decorations(&run, x, run_w, baseline, &mut was);
        run.color = sub.on.color;
        run.deco = sub.on.deco;
        run.deco_color = sub.on.deco_color;
        let mut now = Vec::new();
        push_decorations(&run, x, run_w, baseline, &mut now);
        now.push(DrawOp::Text {
            x,
            y,
            size,
            color: sub.on.color,
            bold: *bold,
            italic: *italic,
            mono: *mono,
            sp: *sp,
            text: text.clone(),
        });

        // A run's decorations sit immediately before it.
        let Some(start) = i.checked_sub(was.len()) else {
            return Err("a run's decorations are not where they should be");
        };
        if !(0..was.len()).all(|k| op_eq(&lay.ops[start + k], &was[k])) {
            return Err("a run's decorations are not where they should be");
        }
        edits.push(Edit { at: start, len: was.len() + 1, ops: now });
        touched += 1;
    }
    // A colour changed and not one run carried it. That is the ordinary case
    // for a container whose text belongs to a link with a colour of its own —
    // the link keeps its colour, so there is genuinely nothing to repaint, and
    // a full layout produces a byte-identical list. What must not be mistaken
    // for it is a run this pass SKIPPED: one that carries the colour but could
    // not be shown to be part of what the element says.
    if touched == 0 {
        let skipped = lay.ops.iter().any(|op| match op {
            DrawOp::Text { x, y, color, text, .. } => {
                in_boxes(&g.boxes, *x, *y)
                    && subs.iter().any(|s| s.off.color == *color)
                    && !says(&g.text, text)
            }
            _ => false,
        });
        if skipped {
            return Err("a run that might belong to this element was skipped");
        }
    }
    Ok(())
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
    let (x, y, w, h) = frag_rect(fonts, b, x0, x1, baseline);
    if w <= 0 || h <= 0 {
        return;
    }
    bg_ops(st, b.bg, b.mask, x, y, w, h, ops);
    border_ops(st, x, y, w, h, sides, ops);
}

/// The rectangle one fragment of an inline box decorates — NOT its line box.
///
/// One source, because the pointer repaint has to regenerate exactly what
/// `paint_frag` produced. Handing it the hit rect instead made every inline
/// background one pixel too tall, which is the difference between a patch that
/// matches a layout and one that does not.
fn frag_rect(
    fonts: &crate::fonts::Fonts,
    b: &InlineBox,
    x0: i32,
    x1: i32,
    baseline: i32,
) -> (i32, i32, i32, i32) {
    let st = &b.st;
    let font = fonts.pick(st.bold, st.italic, st.mono);
    let m = font.horizontal_line_metrics(st.font_px);
    let asc = m.map(|m| m.ascent).unwrap_or(st.font_px);
    let desc = m.map(|m| m.descent.abs()).unwrap_or(0.0);
    let top = baseline - (asc + st.pad_top + st.border_top.width) as i32;
    let h = (asc + desc + st.pad_top + st.pad_bottom + st.border_y()) as i32;
    (x0, top, x1 - x0, h)
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
    hover_boxes: &mut Vec<HoverBox>,
) -> i32 {
    let line_top = y;
    let baseline = y + line_ascent as i32;
    let box_h = ceil_i32(gap).max(1);
    // An inline box's decoration goes in where the fragment begins — but that
    // op does not exist yet when the hit rect is recorded, so the index is
    // parked and turned into a content key once the line is done. Within this
    // function `ops` is only ever APPENDED to, so an index taken here still
    // means the same slot at the end of it.
    let mut pending_anchor: Vec<(usize, usize)> = Vec::new();
    // Inline-box decoration goes down before anything on the line, so text sits
    // on its own background. Sorted by box index — allocation order is tree
    // order, which puts an ancestor's background under its descendant's.
    if !frags.is_empty() {
        let head = line.iter().map(placed_x).min().unwrap_or(0);
        frags.sort_by_key(|f| f.bx);
        for f in frags.drain(..) {
            let b = &boxes[f.bx];
            let (x0, x1) = (f.x0.unwrap_or(head) + dx, f.x1 + dx);
            // A box spanning three lines leaves three fragments and therefore
            // three hit rects — which is right: the pointer is inside the box
            // wherever any of its fragments is.
            if let Some(seq) = b.hover_seq {
                if x1 > x0 {
                    pending_anchor.push((hover_boxes.len(), ops.len()));
                    hover_boxes.push(HoverBox {
                        x: x0,
                        y: line_top,
                        w: x1 - x0,
                        h: box_h,
                        seq,
                        anchor: None,
                        paint: frag_rect(fonts, b, x0, x1, baseline),
                        sides: (f.left, f.right),
                        shadow: false,
                        pseudo: crate::css::PseudoElem::None,
                        anchor_after: false,
                        has_text: false,
                        hoverable: true,
                        toggle: false,
                    });
                }
            }
            paint_frag(fonts, b, x0, x1, baseline, (f.left, f.right), ops);
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
                    let sw = measure_sp(font, &seg.text, seg.style.size, seg.style.sp);
                    links.push(LinkRect { x: seg.x + dx, y: line_top, w: ceil_i32(sw), h: box_h, href: h.clone() });
                }
                if !seg.style.hidden && !seg.style.transparent {
                    if seg.style.deco != 0 {
                        let w = ceil_i32(measure_sp(font, &seg.text, seg.style.size, seg.style.sp));
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
                        sp: seg.style.sp,
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
                for b in &mut box_.hover_boxes {
                    b.x += dx;
                    b.y += dy;
                    b.paint.0 += dx;
                    b.paint.1 += dy;
                    if let Some(a) = &mut b.anchor {
                        a.x += dx;
                        a.y += dy;
                    }
                }
                ops.append(&mut box_.ops);
                links.append(&mut box_.links);
                controls.append(&mut box_.controls);
                inspects.append(&mut box_.inspects);
                hover_boxes.append(&mut box_.hover_boxes);
            }
            Placed::Control { x, ctl } => {
                let top = baseline - (ctl.h - CTL_PAD_Y);
                paint_control(fonts, theme, ctl, x + dx, top, ops, controls);
            }
            Placed::Image { x, w, h, src, href, alt, hidden, transparent, fit, filter, deco } => {
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
                    ops.push(DrawOp::Image { x, y: top, w, h, src, alt, fit, filter });
                }
            }
        }
    }
    for (hb, at) in pending_anchor {
        hover_boxes[hb].anchor = ops.get(at).map(op_key);
    }
    line_top + box_h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    /// Every weight in an additive table must be positive: a zero would make
    /// `while v >= w` spin forever, and a counter style is reachable from any
    /// page's CSS.
    #[test]
    fn additive_counter_tables_have_no_zero_weight() {
        for (w, _) in ARMENIAN.iter().chain(GEORGIAN.iter()) {
            assert!(*w > 0, "zero weight would not terminate");
        }
        // Strictly descending, or the greedy loop emits the wrong glyph.
        for t in [&ARMENIAN[..], &GEORGIAN[..]] {
            assert!(t.windows(2).all(|p| p[0].0 > p[1].0));
        }
    }

    #[test]
    fn greek_armenian_and_georgian_counters_read_right() {
        assert_eq!(greek_counter(1), "\u{3b1}");
        assert_eq!(greek_counter(24), "\u{3c9}");
        // 25 wraps to a two-letter form — 24 letters, final sigma excluded.
        assert_eq!(greek_counter(25), "\u{3b1}\u{3b1}");
        assert_eq!(additive_counter(1, &ARMENIAN), "\u{531}");
        // 1988 = 1000 + 900 + 80 + 8
        assert_eq!(additive_counter(1988, &ARMENIAN), "\u{54c}\u{54b}\u{541}\u{538}");
        assert_eq!(additive_counter(1, &GEORGIAN), "\u{10d0}");
        // Out of range falls back to decimal rather than looping or truncating.
        assert_eq!(additive_counter(0, &ARMENIAN), "0");
        assert_eq!(additive_counter(999_999, &GEORGIAN), "999999");
    }

    fn fonts() -> crate::fonts::Fonts {
        crate::fonts::Fonts::new()
    }

    fn lay(html: &str, w: u32) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom, crate::css::Media::new(800.0, false));
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, &FormState::default(), false, &[], false)
    }

    fn lay_inspect(html: &str, w: u32) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom, crate::css::Media::new(800.0, false));
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, &FormState::default(), true, &[], false)
    }

    fn lay_hover(html: &str, w: u32, hover: &[u32]) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom, crate::css::Media::new(800.0, false));
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, &FormState::default(), false, hover, false)
    }

    /// Only the elements a `:hover` rule can actually react to get a box —
    /// the invalidation set. On Wikipedia's Main_Page that is the difference
    /// between 8327 boxes and a handful, and it is what makes 98.7 % of
    /// pointer movement cost nothing at all.
    ///
    /// The carrier of the `:hover` is what must be hit-testable, NOT the
    /// selector's subject: in `nav:hover a` the pointer is inside the `<nav>`.
    #[test]
    fn only_hover_carriers_get_a_box() {
        let page = |css: &str| {
            let html = alloc::format!(
                "<body><style>{css}</style><nav><a href=\"/\">x</a></nav></body>"
            );
            let l = lay_hover(&html, 400, &[]);
            let mut tags: Vec<u32> = l.hover_boxes.iter().map(|b| b.seq).collect();
            tags.sort_unstable();
            tags
        };

        // `a:hover` — the link carries it, the `<nav>` around it does not.
        let only_a = page("a:hover{background:#f00}");
        assert_eq!(only_a.len(), 1, "just the link: {only_a:?}");

        // `nav:hover a` — now the NAV is the carrier, and it is the one the
        // pointer has to be found inside.
        let only_nav = page("nav:hover a{color:#0f0}");
        assert_eq!(only_nav.len(), 1, "just the nav: {only_nav:?}");
        assert!(only_nav[0] < only_a[0], "the nav comes before the link in document order");

        // A compound that names nothing can match anything → collect everything
        // rather than freeze the page under the pointer.
        assert!(page("*:hover{color:#f00}").len() > 2);
    }

    /// The pointer hovers the element it is inside AND every ancestor that
    /// contains it — `nav:hover a` (every dropdown on the web) styles a
    /// descendant from a state that lives on the parent.
    ///
    /// This is the geometry half; that the restyle reaches a PIXEL is
    /// `raster::tests::hover_repaints_the_element_under_the_pointer`.
    #[test]
    fn the_pointer_hovers_an_element_and_every_ancestor_containing_it() {
        // Both elements carry a hover rule, so both are hit-testable.
        let html = "<body><style>nav:hover{background:#eee} a:hover{background:#f00}</style>\
                    <nav><a href=\"/\">x</a></nav></body>";
        let l = lay_hover(html, 400, &[]);
        assert_eq!(l.hover_boxes.len(), 2);

        let link = l.hover_boxes.iter().max_by_key(|b| b.seq).expect("a box");
        let hovered = l.hover_at(link.x + link.w / 2, link.y + link.h / 2);
        assert_eq!(hovered.len(), 2, "the link AND the nav around it: {hovered:?}");
        assert!(hovered.windows(2).all(|w| w[0] < w[1]), "ascending — `of_hovered` bisects");

        // A point outside every box hovers nothing.
        assert!(l.hover_at(-5, -5).is_empty());
    }

    /// CSS 2.1 Appendix E: a box paints its background AND its border before
    /// any descendant. The background already did; the border was appended
    /// after the content, so it landed on top of the box's own children.
    ///
    /// Invisible while a child stays inside its parent's content box — and
    /// wrong the moment one does not, which is what a negative margin is FOR.
    /// The CSS2.1 suite tests exactly that idiom: pull a child left by the
    /// parent's border width so its own border covers it, and check no red
    /// shows. 62 of those went from fail to pass.
    #[test]
    fn a_box_paints_its_border_under_its_descendants() {
        // The child's black border is pulled onto the parent's red one.
        let l = lay(
            "<body style=\"margin:0\"><div style=\"border-left:20px solid #f00;height:50px\">\
             <div style=\"border-left:20px solid #000;margin-left:-20px;height:50px\"></div>\
             </div></body>",
            800,
        );
        let seen: alloc::vec::Vec<Rgb> = rects(&l)
            .into_iter()
            .filter(|(x, _, w, ..)| *x == 0 && *w == 20)
            .map(|(.., c)| c)
            .collect();
        assert_eq!(
            seen,
            alloc::vec![Rgb(0xff, 0, 0), Rgb(0, 0, 0)],
            "parent's border first, child's over it",
        );

        // And the background still goes under the border of the SAME box.
        let l = lay(
            "<body style=\"margin:0\"><div style=\"background:#00f;border:10px solid #0f0;\
             width:100px;height:50px\"></div></body>",
            800,
        );
        let order: alloc::vec::Vec<Rgb> = rects(&l).into_iter().map(|(.., c)| c).collect();
        let bg = order.iter().position(|c| *c == Rgb(0, 0, 0xff)).expect("background");
        let bd = order.iter().position(|c| *c == Rgb(0, 0xff, 0)).expect("border");
        assert!(bg < bd, "background under its own border: {order:?}");
    }

    /// A hit rect has to sit where the box is PAINTED, on every path that
    /// moves a box after laying it out.
    ///
    /// An `inline-block` is laid out at the ORIGIN and translated onto its
    /// line; a `position:relative` box is laid in flow and then offset; a
    /// `vertical-align`ed table cell slides its content down. Every one of
    /// those moved the ops and left the hit rects behind — so the pointer lit
    /// up elements it was nowhere near (Wikipedia's whole sister-project row
    /// answered to the top-left corner) and the link actually under the
    /// pointer answered to nothing.
    #[test]
    fn a_hit_rect_follows_its_box_when_the_box_moves() {
        let check = |inner: &str, what: &str| {
            let html = alloc::format!(
                "<body style=\"margin:0\"><style>a:hover{{color:#0f0}}</style>{inner}</body>"
            );
            let l = lay_hover(&html, 400, &[]);
            let b = l.hover_boxes.iter().max_by_key(|b| b.seq).expect(what);
            let red = rects(&l).into_iter().find(|(.., c)| *c == Rgb(0xff, 0, 0)).expect(what);
            assert_eq!((b.x, b.y), (red.0, red.1), "{what}: hit rect vs painted box");
        };
        const LINK: &str = "<a href=\"/\" style=\"display:block;width:30px;height:20px;background:#f00\"></a>";
        // Laid out at the origin, then translated onto the line box.
        check(
            &alloc::format!("<p style=\"margin:40px\">t <span style=\"display:inline-block\">{LINK}</span></p>"),
            "inline-block",
        );
        // Laid out in flow, then offset by `position:relative`.
        check(
            "<a href=\"/\" style=\"display:block;position:relative;left:25px;top:15px;\
             width:30px;height:20px;background:#f00\"></a>",
            "position:relative",
        );
        // A flex item is measured, discarded, then laid out for real.
        check(
            &alloc::format!("<div style=\"display:flex;flex-direction:column;height:200px\"><div>{LINK}</div></div>"),
            "flex column item",
        );
    }

    /// A discarded trial layout must not leave its hit rects behind. It
    /// records them at TRIAL coordinates, so a leak points the pointer at a
    /// rectangle the page never painted — and records the same element once
    /// per trial, which is how Wikipedia's Main_Page came to carry 5131 hit
    /// rects for 656 real ones.
    #[test]
    fn a_speculative_layout_leaves_no_hit_rects_behind() {
        let html = "<body style=\"margin:0\"><style>a:hover{color:#0f0}</style>\
                    <div style=\"display:flex;flex-direction:column;height:200px\"><div>\
                    <a href=\"/\" style=\"display:block;width:30px;height:20px;background:#f00\"></a>\
                    </div></div></body>";
        let l = lay_hover(html, 400, &[]);
        assert_eq!(
            l.hover_boxes.len(), 1,
            "one element, one rect: {:?}",
            l.hover_boxes.iter().map(|b| (b.seq, b.x, b.y)).collect::<alloc::vec::Vec<_>>(),
        );
    }

    /// A page with no `:hover` rule must not pay for a list nobody reads.
    #[test]
    fn a_sheet_without_hover_rules_collects_no_boxes() {
        let l = lay_hover("<body><style>a{color:#00f}</style><a href=\"/\">x</a></body>", 400, &[]);
        assert!(l.hover_boxes.is_empty());
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
            DrawOp::Rect { x, y, w, h, color } => std::println!("RECT {x} {y} {w} {h} {:?}", (color.c.0, color.c.1, color.c.2, color.a)),
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
    fn a_fully_transparent_border_takes_space_but_paints_nothing() {
        // DuckDuckGo reserves the hover frame around every search result with
        // `border: 1px solid rgba(0,0,0,0)`. Painting the carrier colour put a
        // black box around each result.
        let boxes = |css: &str| {
            let html = alloc::format!("<body><style>div{{{css}}}</style><div>hi</div></body>");
            let l = lay(&html, 400);
            let n = l
                .ops
                .iter()
                .filter(|o| matches!(o, DrawOp::Rect { .. } | DrawOp::RoundRect { .. }))
                .count();
            (n, l.height)
        };
        let (opaque, h_opaque) = boxes("border:1px solid #000");
        let (clear, h_clear) = boxes("border:1px solid rgba(0,0,0,0)");
        assert!(opaque > 0, "an opaque border paints");
        assert_eq!(clear, 0, "a transparent border paints nothing");
        // It is a border, not an absence: the box is still the same size.
        assert_eq!(h_opaque, h_clear, "transparent border still occupies space");
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


    // ── <details>/<summary> (HTML §4.11.1) ─────────────────────────────────

    /// The whole point: a CLOSED `<details>` shows its summary and nothing
    /// else. Measured on MDN, 117 of 119 sections are closed — rendered open
    /// they turn the page into one endless scroll.
    #[test]
    fn a_closed_details_renders_only_its_summary() {
        let html = "<body><details><summary>head</summary><p>body text</p></details></body>";
        let l = lay(html, 800);
        let t: Vec<&str> = texts(&l).iter().map(|(_, _, s)| *s).collect();
        assert!(t.contains(&"head"), "{t:?}");
        assert!(!t.iter().any(|s| s.contains("body text")), "closed content leaked: {t:?}");

        let open = lay(
            "<body><details open><summary>head</summary><p>body text</p></details></body>",
            800,
        );
        let t2: Vec<&str> = texts(&open).iter().map(|(_, _, s)| *s).collect();
        assert!(t2.iter().any(|s| s.contains("body text")), "open content missing: {t2:?}");
        assert!(open.height > l.height, "open must be taller: {} !> {}", open.height, l.height);
    }

    /// Author CSS must not be able to reveal the skipped contents: a browser
    /// hides them through the shadow tree, where no page rule reaches.
    #[test]
    fn author_css_cannot_reveal_a_closed_details() {
        let l = lay(
            "<body><style>details:not([open]) > p { display: block !important }</style>\
             <details><summary>head</summary><p>body text</p></details></body>",
            800,
        );
        let t: Vec<&str> = texts(&l).iter().map(|(_, _, s)| *s).collect();
        assert!(!t.iter().any(|s| s.contains("body text")), "{t:?}");
    }

    /// Only the FIRST `<summary>` is the control; a second one is ordinary
    /// content and is skipped with the rest.
    #[test]
    fn only_the_first_summary_is_the_control() {
        let l = lay(
            "<body><details><summary>one</summary><summary>two</summary></details></body>",
            800,
        );
        let t: Vec<&str> = texts(&l).iter().map(|(_, _, s)| *s).collect();
        assert!(t.contains(&"one"), "{t:?}");
        assert!(!t.contains(&"two"), "{t:?}");
        assert_eq!(l.hover_boxes.iter().filter(|b| b.toggle).count(), 1);
    }

    /// No `<summary>` child means the UA provides the legend. Without one the
    /// element renders as nothing at all and its contents become unreachable
    /// — worse than showing everything.
    #[test]
    fn a_details_without_a_summary_gets_the_ua_legend() {
        let l = lay("<body><details><p>body text</p></details></body>", 800);
        let t: Vec<&str> = texts(&l).iter().map(|(_, _, s)| *s).collect();
        assert!(t.contains(&"Details"), "{t:?}");
        assert!(!t.iter().any(|s| s.contains("body text")), "{t:?}");
        assert_eq!(l.hover_boxes.iter().filter(|b| b.toggle).count(), 1, "must be clickable");
    }

    /// A `<summary>` outside a `<details>` is a plain block: no marker, and
    /// nothing to click.
    #[test]
    fn a_stray_summary_is_a_plain_block() {
        let l = lay("<body><summary>lonely</summary></body>", 800);
        assert!(texts(&l).iter().any(|(_, _, s)| *s == "lonely"));
        assert_eq!(l.hover_boxes.iter().filter(|b| b.toggle).count(), 0);
    }

    /// The marker is drawn, points the right way, and stays clickable when the
    /// page removes it — most pages write `summary { list-style: none }`.
    #[test]
    fn the_disclosure_marker_turns_and_the_box_stays_clickable() {
        let closed = lay("<body><details><summary>x</summary><p>y</p></details></body>", 800);
        let open = lay("<body><details open><summary>x</summary><p>y</p></details></body>", 800);
        // A right-pointing triangle is columns (w == 1), a down-pointing one
        // is rows (h == 1).
        let cols = |l: &Layout| {
            l.ops.iter().filter(|o| matches!(o, DrawOp::Rect { w: 1, h, .. } if *h > 1)).count()
        };
        let rows = |l: &Layout| {
            l.ops.iter().filter(|o| matches!(o, DrawOp::Rect { h: 1, w, .. } if *w > 1)).count()
        };
        assert!(cols(&closed) > 0 && rows(&closed) == 0, "closed must point right");
        assert!(rows(&open) > 0 && cols(&open) == 0, "open must point down");

        let bare = lay(
            "<body><style>summary { list-style: none }</style>\
             <details><summary>x</summary><p>y</p></details></body>",
            800,
        );
        assert_eq!(cols(&bare), 0, "the page removed the marker");
        assert_eq!(
            bare.hover_boxes.iter().filter(|b| b.toggle).count(),
            1,
            "…but the section must still be openable"
        );
    }

    /// The toggle rect covers the summary, and hit-testing finds it there and
    /// not over the rest of the page.
    #[test]
    fn hit_toggle_finds_the_summary_and_only_there() {
        let l = lay("<body><details open><summary>head</summary><p>body</p></details></body>", 800);
        let b = *l.hover_boxes.iter().find(|b| b.toggle).expect("a toggle rect");
        assert_eq!(l.hit_toggle(b.x + b.w / 2, b.y + b.h / 2), Some(b.seq));
        assert_eq!(l.hit_toggle(b.x + b.w / 2, b.y + b.h + 40), None, "below the summary");
    }

    /// `display: contents` does NOT reparent: a `<summary>` under an unboxed
    /// `<div>` is still not the `<details>`' control, and the whole `<div>` is
    /// skipped with everything in it (css-display-3, and the wpt reftest
    /// `display-contents-details-001`). The mirror risk is the one that would
    /// hurt: if the ancestor chain dropped unboxed elements, every grandchild
    /// of a closed `<details>` would look like a child and vanish.
    #[test]
    fn display_contents_does_not_reparent_a_summary() {
        let l = lay(
            "<body><details><div style=\'display:contents\'>\
             <summary>inner</summary>deep</div></details></body>",
            800,
        );
        let t: Vec<&str> = texts(&l).iter().map(|(_, _, s)| *s).collect();
        assert!(t.contains(&"Details"), "the UA legend stands in: {t:?}");
        assert!(!t.contains(&"inner"), "a nested summary is not the control: {t:?}");
        assert!(!t.iter().any(|s| s.contains("deep")), "{t:?}");

        // The mirror: an OPEN details must still show what is nested under an
        // unboxed child.
        let o = lay(
            "<body><details open><summary>head</summary>\
             <div style=\'display:contents\'><p>deep</p></div></details></body>",
            800,
        );
        let t2: Vec<&str> = texts(&o).iter().map(|(_, _, s)| *s).collect();
        assert!(t2.iter().any(|s| s.contains("deep")), "{t2:?}");
    }

    /// A grandchild is not a child: only the `<details>`' own element children
    /// are skipped, and they take their subtrees with them because they are
    /// `display:none`. If the ancestor chain were ever flattened this test
    /// would keep the page from silently losing everything one level down.
    #[test]
    fn only_direct_children_of_a_details_are_skipped() {
        let l = lay(
            "<body><section><details open><summary>head</summary>\
             <div><p>deep</p></div></details></section></body>",
            800,
        );
        let t: Vec<&str> = texts(&l).iter().map(|(_, _, s)| *s).collect();
        assert!(t.iter().any(|s| s.contains("deep")), "{t:?}");
    }

    /// A `<dialog>` without `open` is not rendered (HTML §4.11.4). Left
    /// visible, a modal's content lands in the middle of the flow — and the
    /// page's own `dialog[open]` rules would never have hidden it, because a
    /// browser does that in the UA sheet.
    #[test]
    fn a_dialog_renders_only_when_open() {
        let shut = lay("<body><p>page</p><dialog><p>modal</p></dialog></body>", 800);
        let t: Vec<&str> = texts(&shut).iter().map(|(_, _, s)| *s).collect();
        assert!(t.contains(&"page"), "{t:?}");
        assert!(!t.contains(&"modal"), "{t:?}");

        let open = lay("<body><p>page</p><dialog open><p>modal</p></dialog></body>", 800);
        let t2: Vec<&str> = texts(&open).iter().map(|(_, _, s)| *s).collect();
        assert!(t2.contains(&"modal"), "{t2:?}");

        // Unlike a closed `<details>`, this one IS an ordinary UA rule: a page
        // that shows its dialog with CSS still can.
        let forced = lay(
            "<body><style>dialog { display: block }</style>\
             <dialog><p>modal</p></dialog></body>",
            800,
        );
        assert!(texts(&forced).iter().any(|(_, _, s)| *s == "modal"));
    }

    /// `<noscript>` renders when scripting is off (HTML §15.3.1) — and beak has
    /// no scripting at all. Pages put lazy-loading `<img>` fallbacks in there,
    /// which is content we were throwing away.
    #[test]
    fn noscript_content_renders_because_we_have_no_script() {
        let l = lay("<body><p>a</p><noscript><p>fallback</p></noscript></body>", 800);
        let t: Vec<&str> = texts(&l).iter().map(|(_, _, s)| *s).collect();
        assert!(t.contains(&"fallback"), "{t:?}");

        // An `<img>` inside it has to reach the shell's fetch list, or the
        // fallback renders as an empty box.
        let l2 = lay(
            "<body><noscript><img src=\'/late.png\' width=\'40\' height=\'20\'></noscript></body>",
            800,
        );
        assert!(
            l2.ops.iter().any(|o| matches!(o, DrawOp::Image { src, .. } if src == "/late.png")),
            "the fallback image must be laid out"
        );

        // `<script>`/`<style>` stay hidden — only noscript moved.
        let l3 = lay("<body><script>var x = 1</script><style>p{}</style>after</body>", 800);
        let t3: Vec<&str> = texts(&l3).iter().map(|(_, _, s)| *s).collect();
        assert!(!t3.iter().any(|s| s.contains("var x")), "{t3:?}");
        assert!(!t3.iter().any(|s| s.contains("p{}")), "{t3:?}");
        assert!(t3.iter().any(|s| s.contains("after")), "{t3:?}");
    }

    fn rects(l: &Layout) -> Vec<(i32, i32, i32, i32, Rgb)> {
        l.ops
            .iter()
            .filter_map(|o| match o {
                DrawOp::Rect { x, y, w, h, color } => Some((*x, *y, *w, *h, color.c)),
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
        assert_eq!(color_of("red"), Some(Rgba::opaque(Rgb(255, 0, 0))));
        assert_eq!(color_of("blue"), Some(Rgba::opaque(Rgb(0, 0, 255)))); // #id wins over p
        assert_eq!(color_of("green"), Some(Rgba::opaque(Rgb(0, 255, 0)))); // .box a matched
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
        // Ein INNERER Schatten malt seit 0.62.0 — und zwar INNEN: die Fuellung
        // minus dem Loch, das er freilaesst. `inset 0 1px` verschiebt das Loch
        // um eins nach unten, uebrig bleibt ein Streifen oben IM Kasten.
        // Bootstrap streift damit seine Tabellen.
        let ins = shadow_rects("height:20px;box-shadow:inset 0 1px rgb(1,2,3)");
        assert_eq!(ins.len(), 1, "ein Streifen, got {ins:?}");
        assert_eq!(ins[0].3, 1, "einen Pixel hoch");
        // `currentColor` is the LAST colour, not whatever was cascaded when the
        // shadow was parsed — same rule the border sides follow.
        let l = lay("<body><div style=\"height:20px;box-shadow:0 1px;color:rgb(1,2,3)\">x</div></body>", 400);
        assert!(rects(&l).iter().any(|(_, _, _, h, c)| *h == 1 && *c == Rgb(1, 2, 3)));
    }

    /// A `box-shadow` list is painted from the first layer we HAVE a paint for,
    /// not from layer one. DuckDuckGo's searchbox ring is the third layer of
    /// `0 10px 20px …, 0 2px 6px …, 0 0 0 1px rgba(0,0,0,.08)`; taking layer one
    /// picked a blurred shadow, which paint then skipped, so the box lost its
    /// outline entirely. Measured over duckduckgo.com and two Wikipedia
    /// articles: 7 declarations hide their only sharp layer behind a blurred
    /// one, and none has two paintable layers.
    #[test]
    /// Ein nackter Textlauf in einem Flex-Container ist ein ANONYMER Kasten
    /// (css-flexbox-1 §4) — und verschwand bis 0.66.0 spurlos.
    ///
    /// `<div class="flex">Label<span>x</span></div>` verlor sein „Label".
    /// Das ist die schlimmste Sorte Layoutfehler: er LOESCHT Text, statt ihn
    /// falsch zu setzen, und auf dem Schirm fehlt nur etwas, von dem niemand
    /// weiss, dass es da sein sollte.
    #[test]
    fn a_bare_text_run_in_a_flex_container_is_an_anonymous_item() {
        let l = lay("<body><div style=\"display:flex\">Label<span>zweites</span></div></body>", 400);
        let texts: alloc::vec::Vec<(i32, i32, &str)> = l.ops.iter().filter_map(|o| match o {
            DrawOp::Text { x, y, text, .. } => Some((*x, *y, text.as_str())),
            _ => None,
        }).collect();
        assert_eq!(texts.len(), 2, "beide Laeufe, got {texts:?}");
        assert_eq!(texts[0].2, "Label");
        // Nebeneinander, nicht untereinander — und auf derselben Zeile.
        assert!(texts[1].0 > texts[0].0, "das zweite Kind steht rechts: {texts:?}");
        assert_eq!(texts[0].1, texts[1].1, "gleiche Grundlinie: {texts:?}");
    }

    /// Reiner Leerraum zwischen zwei Kaesten erzeugt KEINEN Kasten (§4) —
    /// sonst bekaeme jede eingerueckte Quelle unsichtbare Flex-Kinder.
    #[test]
    fn whitespace_between_flex_items_makes_no_box() {
        let l = lay("<body><div style=\"display:flex\">\n  <span>a</span>\n  <span>b</span>\n</div></body>", 400);
        let n = l.ops.iter().filter(|o| matches!(o, DrawOp::Text { .. })).count();
        assert_eq!(n, 2, "nur die beiden Kinder");
    }

    /// css-flexbox-1 §8.1: eine `auto`-Marge frisst den freien Platz auf IHRER
    /// Achse. In der Spalte ist das die Hauptachse — `margin-top:auto` auf dem
    /// letzten Kind heftet es an den Boden (das Karten-Fussmuster).
    #[test]
    fn an_auto_top_margin_pins_the_last_column_item_to_the_bottom() {
        let l = lay(
            "<body style=\"margin:0\"><div style=\"display:flex;flex-direction:column;height:200px\">             <div>oben</div><div style=\"margin-top:auto\">unten</div></div></body>",
            400,
        );
        let ys: alloc::vec::Vec<i32> = l.ops.iter().filter_map(|o| match o {
            DrawOp::Text { y, .. } => Some(*y),
            _ => None,
        }).collect();
        assert_eq!(ys.len(), 2, "{ys:?}");
        assert_eq!(ys[0], 0, "das erste Kind bleibt oben: {ys:?}");
        assert!(ys[1] >= 175, "das zweite steht unten, nicht direkt darunter: {ys:?}");
    }

    /// Auf der Querachse ueberstimmt sie `align-items` — und den Stretch.
    /// `mt-auto` in einer ZEILE heisst unten, `my-auto` heisst Mitte.
    #[test]
    fn auto_cross_margins_beat_align_items_in_a_row() {
        let l = lay(
            "<body style=\"margin:0\"><div style=\"display:flex;height:200px;align-items:flex-start\">             <div>a</div><div style=\"margin-top:auto\">b</div>             <div style=\"margin-top:auto;margin-bottom:auto\">c</div></div></body>",
            400,
        );
        let ys: alloc::vec::Vec<i32> = l.ops.iter().filter_map(|o| match o {
            DrawOp::Text { y, .. } => Some(*y),
            _ => None,
        }).collect();
        assert_eq!(ys.len(), 3, "{ys:?}");
        assert_eq!(ys[0], 0, "align-items gilt weiter, wo keine auto-Marge steht: {ys:?}");
        assert!(ys[1] >= 175, "mt-auto = unten: {ys:?}");
        assert!((ys[2] - 90).abs() <= 3, "my-auto = Mitte: {ys:?}");
    }

    /// In der Spalte ist links/rechts die QUERachse: `mx-auto` zentriert, und
    /// dazu muss der Stretch weichen — sonst ist der Kasten schon so breit wie
    /// die Zeile und es bleibt nichts zu verteilen.
    #[test]
    fn mx_auto_centres_a_column_item_instead_of_stretching_it() {
        let l = lay(
            "<body style=\"margin:0\"><div style=\"display:flex;flex-direction:column;width:300px\">             <div style=\"margin-left:auto;margin-right:auto\">m</div></div></body>",
            400,
        );
        let x = l.ops.iter().find_map(|o| match o {
            DrawOp::Text { x, .. } => Some(*x),
            _ => None,
        }).expect("Text");
        assert!(x > 100 && x < 200, "zentriert in 300px, nicht bei 0: x={x}");
    }

    /// `opacity` unter 1 verblasst den Kasten UND seinen Teilbaum. Gemessen
    /// wird die Alpha der Befehle, nicht die Farbe — die bleibt.
    #[test]
    fn opacity_fades_the_box_and_its_subtree() {
        let l = lay(
            "<body style=\"margin:0\"><div style=\"opacity:.5;background:#ff0000;width:40px;height:20px\">             <span>t</span></div></body>",
            200,
        );
        let (mut fill, mut text) = (None, None);
        for o in l.ops.iter() {
            match o {
                DrawOp::Rect { color, w, .. } if *w == 40 => fill = Some(*color),
                DrawOp::Text { color, .. } => text = Some(*color),
                _ => {}
            }
        }
        let f = fill.expect("Fuellung");
        assert_eq!((f.c.0, f.c.1, f.c.2), (255, 0, 0), "die Farbe bleibt");
        assert!((f.a as i32 - 128).abs() <= 2, "halb durchsichtig, a={}", f.a);
        assert!((text.expect("Text").a as i32 - 128).abs() <= 2, "der Teilbaum auch");
    }

    /// Eine Schatten-Schicht mit Alpha 0 malt nichts — und darf deshalb den
    /// einen scharfen Platz nicht belegen. Tailwind stellt genau so eine als
    /// Platzhalter VOR den echten Ring.
    #[test]
    fn a_transparent_shadow_layer_does_not_take_the_slot() {
        let l = lay(
            "<body style=\"margin:0\"><div style=\"box-shadow:0 0 #0000, 0 0 0 4px #0000ff;             background:#fff;width:40px;height:14px\">x</div></body>",
            200,
        );
        let blue = l.ops.iter().filter(|o| matches!(o,
            DrawOp::Rect { color, .. } if (color.c.0, color.c.1, color.c.2) == (0, 0, 255))).count();
        assert_eq!(blue, 4, "vier Balken um den Kasten, got {blue}");
    }

    /// `currentcolor` ist die Vorgabe von `box-shadow` — ausgeschrieben muss
    /// sie dasselbe heissen wie weggelassen, sonst ist die Schicht ungueltig.
    #[test]
    fn box_shadow_accepts_a_written_out_currentcolor() {
        let l = lay(
            "<body style=\"margin:0\"><div style=\"color:#00aa00;box-shadow:0 0 0 4px currentcolor;             background:#fff;width:40px;height:14px\">x</div></body>",
            200,
        );
        let green = l.ops.iter().filter(|o| matches!(o,
            DrawOp::Rect { color, .. } if (color.c.0, color.c.1, color.c.2) == (0, 170, 0))).count();
        assert_eq!(green, 4, "der Ring traegt die Textfarbe, got {green}");
    }

    /// Ein Steuerelement wird mit UNSEREN Massen gemalt, also muss die Seite
    /// ihren `border-radius` mitgeben koennen. Ohne ihn hatte JEDER Knopf auf
    /// einer Bootstrap- oder Tailwind-Seite scharfe Ecken — das erste, was
    /// nach „kein Browser" aussieht.
    #[test]
    fn a_control_keeps_the_page_border_radius() {
        let l = lay(
            "<body style=\"margin:0\"><button style=\"border-radius:6px;background:#0d6efd;\
             border:1px solid #0d6efd\">Knopf</button></body>",
            400,
        );
        let rounded: alloc::vec::Vec<f32> = l.ops.iter().filter_map(|o| match o {
            DrawOp::RoundRect { r, .. } => Some(r[0]),
            _ => None,
        }).collect();
        assert!(rounded.len() >= 2, "Flaeche UND Rahmen gerundet, got {rounded:?}");
        assert!(rounded.iter().all(|r| (*r - 6.0).abs() < 0.01), "{rounded:?}");
        // Und ohne Radius bleibt es beim Rechteck — kein Ring, wo keiner hin soll.
        let sq = lay(
            "<body style=\"margin:0\"><button style=\"background:#0d6efd\">Knopf</button></body>",
            400,
        );
        assert!(!sq.ops.iter().any(|o| matches!(o, DrawOp::RoundRect { .. })), "eckig bleibt eckig");
    }

    /// Ein WEICHER Schatten wird gemalt — und zwar weich.
    ///
    /// Bis 0.61.0 fiel er ganz weg (nur `blur == 0` wurde gemalt), und das
    /// betraf jede Bootstrap-Karte, jeden Dialog und jedes Menue: sie lagen
    /// flach auf der Seite statt darueber.
    #[test]
    fn a_blurred_shadow_is_painted_and_fades() {
        let l = lay("<body><div style=\"height:20px;box-shadow:0 4px 12px rgb(0,0,0)\">x</div></body>", 400);
        let n = l.ops.iter().filter(|o| matches!(o, DrawOp::Shadow { .. })).count();
        assert_eq!(n, 1, "genau ein weicher Schatten");
        // Und er ist wirklich weich: die Deckung faellt nach aussen ab. Ohne
        // die Pruefung koennte er ein harter Klotz sein und der Test gruen.
        let mut buf = alloc::vec![0u8; 400 * 120 * 4];
        let eng = crate::Engine::new();
        eng.paint(&l, 400, 120, 0, &mut buf);
        let at = |x: usize, y: usize| buf[(y * 400 + x) * 4] as i32;
        let (nah, fern) = (at(200, 40), at(200, 52));
        assert!(nah < fern, "unter dem Kasten muss es nach aussen heller werden: {nah} vs {fern}");
    }

    fn a_shadow_list_paints_its_first_paintable_layer_not_its_first() {
        let shadow_rects = |css: &str| -> Vec<(i32, i32, i32, i32, Rgb)> {
            let l = lay(&alloc::format!("<body><div style=\"{css}\">x</div></body>"), 400);
            rects(&l).into_iter().filter(|(_, _, _, _, c)| *c == Rgb(1, 2, 3)).collect()
        };
        // The DDG shape: two blurred layers, then the 1px spread that is the
        // visible ring. Four sides, because a pure spread rings the box.
        assert_eq!(
            shadow_rects(
                "height:20px;box-shadow:0 10px 20px rgb(9,9,9),0 2px 6px rgb(9,9,9),\
                 0 0 0 1px rgb(1,2,3)"
            )
            .len(),
            4,
            "the sharp third layer rings the box"
        );
        // An `inset` layer is skipped like a blurred one — we have no inner
        // shadow — and the search continues past it.
        let r = shadow_rects("height:20px;box-shadow:inset 0 0 0 2px rgb(9,9,9),0 1px rgb(1,2,3)");
        assert_eq!(r.len(), 1, "one strip below, got {r:?}");
        assert_eq!(r[0].1, 8 + 20);
        // A list with nothing paintable in it paints nothing.
        assert!(shadow_rects("height:20px;box-shadow:0 2px 8px rgb(1,2,3),inset 0 1px rgb(1,2,3)")
            .is_empty());
        // `inset` is VALID CSS, just unpaintable — so it REPLACES an earlier
        // shadow rather than being dropped as a bad value and leaving it up.
        assert!(
            shadow_rects("height:20px;box-shadow:0 1px rgb(1,2,3);box-shadow:inset 0 1px rgb(1,2,3)")
                .is_empty(),
            "the inset declaration wins the cascade and paints nothing"
        );
        // A layer we cannot READ still invalidates the whole declaration, so
        // the box keeps the shadow it already had.
        assert_eq!(
            shadow_rects("height:20px;box-shadow:0 1px rgb(1,2,3);box-shadow:0 1px wobble(3)")
                .len(),
            1,
            "an unreadable value drops the declaration, not the previous shadow"
        );
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

    /// A line box is not written until it BREAKS, so an out-of-flow box reached
    /// mid-line lands in the display list ahead of text that precedes it in the
    /// document — and paints under it. CSS 2.1 Appendix E puts positioned boxes
    /// in step 8, after that inline content in step 7.
    ///
    /// The box is lifted over exactly that one line. Flushing the line instead
    /// would break `foo<div style=position:absolute></div>bar` onto two lines,
    /// and lifting positioned boxes wholesale is worse: out-of-flow-only
    /// measured +25/−21 against the reftests, every positioned box +16/−46.
    #[test]
    fn an_abspos_box_reached_mid_line_paints_over_that_line() {
        let order = |html: &str| -> Vec<Rgb> {
            rects(&lay(html, 800)).into_iter().map(|(_, _, _, _, c)| c).collect()
        };
        // The green box covers the red one exactly; only paint order decides
        // whether any red is left, and the green one comes LATER in the source.
        let ops = order(
            "<body><div style=\"position:relative\">\
             <iframe style=\"display:inline;border:3px solid rgb(255,0,0)\"></iframe>\
             <div style=\"position:absolute;top:0;width:300px;height:150px;\
             border:3px solid rgb(0,128,0)\"></div></div></body>",
        );
        let red = ops.iter().position(|c| *c == Rgb(255, 0, 0));
        let green = ops.iter().rposition(|c| *c == Rgb(0, 128, 0));
        assert!(red.is_some() && green.is_some(), "both boxes painted: {ops:?}");
        assert!(green > red, "the abspos box paints last: {ops:?}");
        // …and it is lifted over the line only, not over a box that FOLLOWS it.
        // Both of these are step 8, so document order decides and the blue one
        // wins — this is the shape (`CSS2/border-005`) that a blanket hoist got
        // wrong.
        let ops = order(
            "<body><div style=\"position:relative\">\
             <div style=\"position:absolute;top:0;width:99px;height:99px;\
             background:rgb(255,0,0)\"></div>\
             <div style=\"position:relative;width:99px;height:99px;\
             background:rgb(0,0,255)\"></div></div></body>",
        );
        let red = ops.iter().rposition(|c| *c == Rgb(255, 0, 0));
        let blue = ops.iter().rposition(|c| *c == Rgb(0, 0, 255));
        assert!(blue > red, "an in-flow positioned box after it still wins: {ops:?}");
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

    #[test]
    fn an_image_below_the_fold_is_not_in_the_visible_band() {
        // A repaint is the whole viewport, so the shell asks before paying for
        // one. Two images, one on screen and one far below a 500 px fold.
        let l = lay(
            "<body><img src=\"/top.png\" width=\"100\" height=\"100\">\
             <div style=\"height:2000px\"></div>\
             <img src=\"/low.png\" width=\"100\" height=\"100\"></body>",
            800,
        );
        assert!(l.images_in_band(&["/top.png"], 0, 500), "on screen → repaint");
        assert!(!l.images_in_band(&["/low.png"], 0, 500), "below the fold → nothing to show");
        // Scrolled down to it, the same image is worth a repaint — which is why
        // skipping one loses nothing: scrolling marks the page dirty anyway.
        assert!(l.images_in_band(&["/low.png"], 1800, 2800), "scrolled to → repaint");
        // A batch repaints if ANY of its images is visible.
        assert!(l.images_in_band(&["/low.png", "/top.png"], 0, 500), "one visible is enough");
        // A src this layout never placed cannot be visible.
        assert!(!l.images_in_band(&["/nowhere.png"], 0, 100_000), "not painted → not visible");
    }

    #[test]
    fn a_background_below_the_fold_is_not_in_the_visible_band() {
        let l = lay(
            "<body><div style=\"height:2000px\"></div>\
             <div id=x style=\"height:100px;background-image:url(/bg.png)\"></div></body>",
            800,
        );
        let keys: Vec<u64> = l.ops.iter().filter_map(|o| match o {
            DrawOp::BgImage { key, .. } => Some(*key),
            _ => None,
        }).collect();
        assert_eq!(keys.len(), 1, "one background layer");
        assert!(!l.css_images_in_band(&keys, 0, 500), "below the fold");
        assert!(l.css_images_in_band(&keys, 1900, 2900), "scrolled to");
    }

    /// Lay out with live form state (what the shell does while the user types).
    fn lay_forms(html: &str, w: u32, st: &FormState) -> Layout {
        let dom = dom::parse(html);
        let sheet = crate::css::collect(&dom, crate::css::Media::new(800.0, false));
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, st, false, &[], false)
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
    fn letter_spacing_widens_a_run_and_moves_what_follows() {
        // `letter-spacing` lands after every character, so a right-aligned run
        // of five characters starts 5 × the spacing further left.
        let plain = lay("<body><div style=\"width:400px;text-align:right\">abcde</div></body>", 800);
        let spaced =
            lay("<body><div style=\"width:400px;text-align:right;letter-spacing:4px\">abcde</div></body>", 800);
        let x = |l: &Layout| {
            l.ops
                .iter()
                .find_map(|o| match o {
                    DrawOp::Text { x, text, .. } if text == "abcde" => Some(*x),
                    _ => None,
                })
                .expect("the run")
        };
        assert_eq!(x(&plain) - x(&spaced), 20, "five characters, 4px each");
    }

    #[test]
    fn nbsp_is_not_a_break_opportunity_and_has_a_width() {
        // `&nbsp;` exists so a line does NOT break there. It is also not
        // collapsible, so four of them are four characters wide, not one space.
        let one = lay("<body><div style=\"width:400px\">a\u{00A0}b</div></body>", 800);
        let four = lay("<body><div style=\"width:400px\">a\u{00A0}\u{00A0}\u{00A0}\u{00A0}b</div></body>", 800);
        let wid = |l: &Layout| {
            l.ops
                .iter()
                .find_map(|o| match o {
                    DrawOp::Text { text, .. } if text.contains('b') => Some(text.chars().count()),
                    _ => None,
                })
                .unwrap_or(0)
        };
        assert_eq!(wid(&one), 3, "one nbsp stays one character in the run");
        assert_eq!(wid(&four), 6, "four nbsp do not collapse into one");
    }

    #[test]
    fn align_content_centers_the_lines_in_a_taller_container() {
        // Two 120px items in a 200px wrap container = two 40px lines, packed
        // into 200px of cross space: 120px is left over, half of it above.
        let l = lay(
            "<body><div style=\"display:flex; flex-wrap:wrap; width:200px; height:200px; \
             align-content:center\"><div style=\"width:120px; height:40px\">a</div>\
             <div style=\"width:120px; height:40px\">b</div></div></body>",
            800,
        );
        let t = texts(&l);
        let a = *t.iter().find(|(_, _, s)| *s == "a").expect("a");
        let b = *t.iter().find(|(_, _, s)| *s == "b").expect("b");
        assert!(a.1 >= 60, "the first line starts well below the container top, was y={}", a.1);
        assert_eq!(b.1 - a.1, 40, "the lines stay their own size, one below the other");
    }

    #[test]
    fn align_content_stretch_grows_every_line_to_fill_the_container() {
        // The initial value: no leftover cross space survives, so the second
        // line starts halfway down a 200px container.
        let l = lay(
            "<body><div style=\"display:flex; flex-wrap:wrap; width:200px; height:200px\">\
             <div style=\"width:120px; height:40px\">a</div>\
             <div style=\"width:120px; height:40px\">b</div></div></body>",
            800,
        );
        let t = texts(&l);
        let a = *t.iter().find(|(_, _, s)| *s == "a").expect("a");
        let b = *t.iter().find(|(_, _, s)| *s == "b").expect("b");
        assert_eq!(a.1, 8, "the first line still starts at the container top (body margin)");
        assert_eq!(b.1 - a.1, 100, "each line took half the container, was {}", b.1 - a.1);
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
        // `disc` ist eine SCHEIBE, kein Quadrat — die Form IST der Wert der
        // Eigenschaft, sonst sind `disc`, `circle` und `square` auf dem Schirm
        // dasselbe Zeichen und eine verschachtelte Liste verliert ihre Ebenen.
        let discs: alloc::vec::Vec<f32> = l.ops.iter().filter_map(|o| match o {
            DrawOp::RoundRect { r, w, h, ring, .. } if w == h && *ring == 0.0 => Some(r[0] / *w as f32),
            _ => None,
        }).collect();
        assert_eq!(discs.len(), 2, "ein Punkt je <li>, got {discs:?}");
        assert!(discs.iter().all(|f| (*f - 0.5).abs() < 0.01), "voll gerundet: {discs:?}");
        // list text is indented past the plain content edge (the body margin)
        assert!(texts(&l).iter().all(|(x, _, _)| *x > 8));
    }

    /// Ein `opacity` auf einem INLINE-Element verblasst seinen Text und seinen
    /// Schmuck. Ein Inline-Kasten bekommt keinen eigenen Befehlsbereich, ueber
    /// den es nachtraeglich gelegt werden koennte — die Deckung faehrt deshalb
    /// im Stil mit, und Vorfahren multiplizieren sich auf.
    #[test]
    fn opacity_on_an_inline_element_fades_its_run() {
        let l = lay(
            "<body style=\"margin:0\"><p>a <span style=\"opacity:.5;background:#ff0000\">halb</span> b</p></body>",
            400,
        );
        let alphas: alloc::vec::Vec<(u8, &str)> = l.ops.iter().filter_map(|o| match o {
            DrawOp::Text { color, text, .. } => Some((color.a, text.as_str())),
            _ => None,
        }).collect();
        let half = alphas.iter().find(|(_, t)| *t == "halb").expect("der Lauf ist getrennt");
        assert!((half.0 as i32 - 127).abs() <= 2, "halb durchsichtig: {alphas:?}");
        assert!(alphas.iter().filter(|(_, t)| *t != "halb").all(|(a, _)| *a == 255),
                "die Nachbarn nicht: {alphas:?}");
        // Und der Hintergrund des Inline-Kastens genauso.
        let bg = l.ops.iter().find_map(|o| match o {
            DrawOp::Rect { color, .. } if (color.c.0, color.c.1, color.c.2) == (255, 0, 0) => Some(color.a),
            _ => None,
        }).expect("Hintergrund");
        assert!((bg as i32 - 127).abs() <= 2, "auch der Schmuck, a={bg}");
    }

    /// Und sie multipliziert sich mit der des BLOCKS darueber, statt sie zu
    /// ersetzen: der Block legt seine ueber den ganzen Befehlsbereich.
    #[test]
    fn inline_and_block_opacity_multiply() {
        let l = lay(
            "<body style=\"margin:0\"><p style=\"opacity:.5\">a <span style=\"opacity:.5\">viertel</span></p></body>",
            400,
        );
        let a: alloc::vec::Vec<(u8, &str)> = l.ops.iter().filter_map(|o| match o {
            DrawOp::Text { color, text, .. } => Some((color.a, text.as_str())),
            _ => None,
        }).collect();
        let outer = a.iter().find(|(_, t)| t.trim() == "a").expect("aussen").0;
        let inner = a.iter().find(|(_, t)| *t == "viertel").expect("innen").0;
        assert!((outer as i32 - 128).abs() <= 2, "aussen halb: {a:?}");
        assert!((inner as i32 - 64).abs() <= 2, "innen viertel: {a:?}");
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
        Display::Flex | Display::InlineFlex => st.flex_row,
        Display::Grid => true,
        _ => false,
    }
}

/// The used size an intrinsic keyword asks for, given the box's own
/// (max-content, min-content) pair and the room it has — css-sizing-3 §5.
///
/// `fit-content` is the shrink-to-fit formula CSS2.1 already used for floats;
/// naming it as a keyword only lets the author ask for it explicitly.
fn intrinsic_size(k: Intrinsic, max_c: f32, min_c: f32, avail: f32) -> f32 {
    match k {
        Intrinsic::Min => min_c,
        Intrinsic::Max => max_c,
        Intrinsic::Fit => max_c.min(avail.max(min_c)),
    }
}

/// Resolve a vertical length against a containing-block height (CSS2.1 §9.3.2 /
/// §10.5). A percentage needs a definite CB height; an indefinite one (the
/// parent's content height doesn't exist yet while its children lay out) leaves
/// it unresolvable, which behaves as `auto`.
fn vert_len(len: Len, cbh: Option<i32>) -> Option<f32> {
    match len {
        // An intrinsic keyword on the block axis is the CONTENT height, which
        // no containing block can supply — unresolvable here, like `auto`.
        Len::Auto | Len::Intrinsic(_) => None,
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
/// Resolve `aspect-ratio` into a used height, once the box's content width is
/// known. Only the width→height direction: that is the one pages use (a card,
/// a video embed, an image placeholder holding its shape while it loads), and
/// the other needs a definite height that a block box in flow does not have.
fn with_aspect_height(st: &ComputedStyle, cw: f32) -> Option<ComputedStyle> {
    let r = st.aspect_ratio?;
    if !matches!(st.height, Len::Auto) {
        return None;
    }
    let mut s = *st;
    // The ratio governs whichever box `box-sizing` names, so under
    // `border-box` the width it divides is the border-box width — and the
    // height it yields is one too, which is exactly what `content_height_of`
    // then takes the frame back off.
    let frame = if s.box_border { s.pad_left + s.pad_right + s.border_x() } else { 0.0 };
    s.height = Len::Px(((cw + frame) / r).max(0.0));
    Some(s)
}

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
fn padding_cb(st: &ComputedStyle, content_x: i32, content_top: i32, content_w: i32) -> PosCb {
    (
        content_x - st.pad_left as i32,
        content_top - st.pad_top as i32,
        content_w + (st.pad_left + st.pad_right) as i32,
        definite_cb_height(st),
        None,
    )
}
