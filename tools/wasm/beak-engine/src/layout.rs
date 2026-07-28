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
use alloc::vec;
use alloc::vec::Vec;
use fontdue::Font;

use crate::css::{ElemInfo, PseudoElem, Stylesheet};
use crate::dom::{Dom, Element, Node};
use crate::forms::{ControlKind, FormState};
use crate::image::ImageMap;
use crate::style::{
    self, BorderSide, ClearKind, Clip, ComputedStyle, ContentPiece, CrossAlign, Display, FlexBasis,
    FloatKind, GridTrack, Justify, Len, ListStyle, Position, TableLayout, TextAlign, TextTransform,
    ZIndex, BASE_FONT_PX,
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
/// border box must not overlap floats (CSS2.1 §9.4.1). We can detect the
/// formatting-context displays (flex/grid/table); `overflow != visible` also
/// does but isn't tracked in the style yet.
fn establishes_bfc(st: &ComputedStyle) -> bool {
    matches!(st.display, Display::Flex | Display::Grid | Display::Table)
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
            DrawOp::Rect { y, .. } | DrawOp::Text { y, .. } | DrawOp::Image { y, .. } => *y += dy,
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
            DrawOp::Image { x, y, w, h, .. } => {
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
}

const PAD: i32 = 20;

/// One paint instruction, positioned in document space (pre-scroll).
pub enum DrawOp {
    /// A run of already-wrapped, same-style text; `y` is the run's top.
    Text { x: i32, y: i32, size: f32, color: Rgb, bold: bool, italic: bool, mono: bool, text: String },
    /// A filled rectangle (divider, list bullet).
    Rect { x: i32, y: i32, w: i32, h: i32, color: Rgb },
    /// A decoded image, scaled to `w`×`h` at blit time.
    /// An `<img>` box. Carries the `src` KEY, not the decoded pixels: the
    /// rasteriser looks the image up when it paints, and draws a placeholder
    /// on a miss. That way an image arriving after layout costs a repaint
    /// instead of a full re-layout — which on a real article is the
    /// difference between ~15 ms and ~145 ms, per image batch.
    Image { x: i32, y: i32, w: i32, h: i32, src: String, alt: String },
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
    ops: Vec<DrawOp>,
    links: Vec<LinkRect>,
    controls: Vec<ControlRect>,
    /// Live form-control state (typed values, checked boxes, focus) — read
    /// only; the shell owns it and re-lays out when it changes.
    forms: &'a FormState,
    path: Vec<ElemInfo>, // root → … → current parent
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
    stack_ops: Vec<(i32, usize, usize)>,
    stack_links: Vec<(i32, usize, usize)>,
    /// Depth of currently-open *tracked* (recorded) stacking ranges. Only a
    /// box at depth 0 gets recorded — a z-indexed box nested inside another
    /// already-tracked one paints as part of its ancestor's range instead
    /// (full nested stacking contexts, e.g. an explicit `z-index` inside
    /// another explicit `z-index`, are out of scope — sibling ordering is
    /// the common case these reftests need).
    stack_depth: u32,
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
}

impl Ctx<'_> {
    /// Whether `st` should open a new tracked stacking range right now: it is
    /// positioned, has an explicit `z-index`, and isn't already nested inside
    /// another tracked range.
    fn should_track_stack(&self, st: &ComputedStyle) -> bool {
        st.position != Position::Static && matches!(st.z_index, ZIndex::Value(_)) && self.stack_depth == 0
    }

    /// Record one box's emitted `ops[op_start..op_end]` / `links[link_start..
    /// link_end]` as its own stacking-order unit. Empty ranges are skipped.
    fn record_stack_entry(&mut self, z: i32, op_start: usize, op_end: usize, link_start: usize, link_end: usize) {
        if op_end > op_start {
            self.stack_ops.push((z, op_start, op_end));
        }
        if link_end > link_start {
            self.stack_links.push((z, link_start, link_end));
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
fn reorder_by_z<T>(items: Vec<T>, ranges: &[(i32, usize, usize)]) -> Vec<T> {
    if ranges.is_empty() {
        return items;
    }
    let mut sorted_ranges = ranges.to_vec();
    sorted_ranges.sort_by_key(|r| r.1); // by start — already ascending, but be safe
    let mut it = items.into_iter();
    let mut cursor = 0usize;
    let mut blocks: Vec<(i32, Vec<T>)> = Vec::new();
    for (z, start, end) in sorted_ranges {
        if start > cursor {
            blocks.push((0, (&mut it).take(start - cursor).collect()));
        }
        blocks.push((z, (&mut it).take(end - start).collect()));
        cursor = end;
    }
    let rest: Vec<T> = it.collect();
    if !rest.is_empty() {
        blocks.push((0, rest));
    }
    // Stable: equal-`z` blocks (all the untracked spans + any explicit
    // `z-index: 0`) keep the relative order they were built in above.
    blocks.sort_by_key(|(z, _)| *z);
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
    let initial = ComputedStyle::root(theme);
    let html_el = dom.root_element();
    let mut root = style::resolve(html_el, &initial, theme, sheet, &[], &[], 0, width as f32);
    root.rem_base = root.font_px;
    let cx = PAD;
    let cw = (width as i32 - 2 * PAD).max(60);
    let mut ctx = Ctx {
        fonts,
        theme,
        sheet,
        images,
        guessed: core::cell::RefCell::new(Vec::new()),
        ops: Vec::new(),
        links: Vec::new(),
        controls: Vec::new(),
        forms,
        path: Vec::new(),
        // Initial containing block: the viewport. Its height is definite —
        // that is what makes `top:0; bottom:0` on a root-level abspos box
        // stretch to the window rather than collapse (CSS 2.1 §10.1).
        cb: (cx, PAD, cw, Some(viewport_h as i32)),
        viewport_w: width as f32,
        floats: Vec::new(),
        stack_ops: Vec::new(),
        stack_links: Vec::new(),
        stack_depth: 0,
        marker_ord: 0,
        counters: Counters::default(),
        inspect,
        inspects: Vec::new(),
        intrinsic: BTreeMap::new(),
    };

    let mut y = PAD;

    // Resolve <body> itself so `body { … }` rules inherit into the page, and
    // put it on the ancestor path so `body p` / `.article p` selectors match.
    let body = dom.body();
    let html_info = [ElemInfo::of(html_el)];
    let anc: &[ElemInfo] = if core::ptr::eq(html_el, body) { &[] } else { &html_info };
    let body_style = style::resolve(body, &root, theme, sheet, anc, &[], 0, width as f32);
    ctx.path.extend_from_slice(anc);
    ctx.path.push(ElemInfo::of(body));
    // A `display: table`/`flex`/`grid` `<body>` must itself establish that
    // formatting context — otherwise its `table-row`/`-cell` children have no
    // `table` ancestor and (correctly, per CSS2.1 §17.2.1) get wrapped in
    // their own anonymous table, which is not what the author asked for.
    y = if establishes_bfc(&body_style) {
        ctx.layout_box(body, &body_style, cx, cw, y)
    } else {
        ctx.layout_children(&body.children, &body_style, Some(body), cx, cw, y)
    };
    // A float can extend below the last in-flow line — grow the page to contain it.
    let float_bottom = ctx.floats.iter().map(|f| f.bottom).max().unwrap_or(0);
    y = y.max(float_bottom);
    y += PAD;

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
        std::eprintln!("[stack] ops={} ranges={:?}", ctx.ops.len(), ctx.stack_ops);
    }
    let ops = reorder_by_z(ctx.ops, &ctx.stack_ops);
    let links = reorder_by_z(ctx.links, &ctx.stack_links);
    Layout {
        ops,
        links,
        controls: ctx.controls,
        height: y.max(1) as u32,
        bg: canvas_bg,
        guessed_image_srcs: ctx.guessed.into_inner(),
        inspect: ctx.inspects,
    }
}

impl Ctx<'_> {
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
    fn place_float(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y: i32) {
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
        // position `y`. Drop below earlier floats until the margin box fits.
        let mut fy = y;
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
    fn layout_children(&mut self, nodes: &[Node], parent: &ComputedStyle, owner: Option<&Element>, x: i32, w: i32, y0: i32) -> i32 {
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
        nodes: &[Node],
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
            if let Some((text, ps)) = self.pseudo(owner, parent, PseudoElem::Before) {
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
            let st = style::resolve(el, parent, self.theme, self.sheet, &self.path, &siblings, sib_count, self.viewport_w);
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
                let (iw, ih) = self.img_box(el);
                let alt = el.attr("alt").unwrap_or("").trim().to_string();
                let src = el.attr("src").unwrap_or("").to_string();
                inline.image(src, iw, ih, None, alt, st.hidden, st.transparent);
                self.path.pop();
                continue;
            }
            // Form controls are atomic inline boxes too — and their children
            // (a `<button>`'s label, a `<select>`'s options) never lay out as
            // page content. Same treatment in `collect_inline`, since most
            // controls sit inside inline context.
            if let Some(kind) = crate::forms::kind_of(el) {
                if kind != ControlKind::Hidden {
                    self.path.push(ElemInfo::of(el));
                    // An absolutely-positioned control is out of flow, like any
                    // other abspos box — the checkbox-hack toggle overlay
                    // (`position:absolute; width:100%; height:100%; opacity:0`)
                    // must NOT advance the line, or its full-size box inflates
                    // the container by the whole page height.
                    if matches!(st.position, Position::Absolute | Position::Fixed) {
                        self.layout_abs(el, &st, x, anchor + open.value() as i32);
                    } else {
                        let ctl = self.control_box(el, &st, kind, w as f32);
                        inline.control(ctl);
                    }
                    self.path.pop();
                }
                continue;
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
                self.place_float(el, &st, x, w, anchor);
                continue;
            }
            if st.display == Display::Inline {
                self.path.push(ElemInfo::of(el));
                self.collect_inline(el, &st, None, &mut inline, x, w, anchor);
                self.path.pop();
                continue;
            }
            // Block-level, in normal flow. Flush pending inline content first —
            // a line box separates margins, so the open margin commits here.
            if !inline.is_empty() {
                let ly = anchor + open.value() as i32;
                let nb = inline.flow(self.fonts, self.theme, x, w, ly, &self.floats, parent.text_align, parent.text_align_last, parent.rtl, parent.line_height.px(parent.font_px).unwrap_or(0.0), &mut self.ops, &mut self.links, &mut self.controls);
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
            let out = if establishes_bfc(&st) {
                let mut t = open;
                t.add(st.margin_top);
                let by = anchor + t.value() as i32;
                let (bx, bw, byy) = self.avoid_floats_bfc(&st, x, w, by);
                let saved = core::mem::take(&mut self.floats);
                let bottom = self.layout_box(el, &st, bx, bw, byy);
                self.record_inspect(el, &st, bx, byy, bw, bottom - byy);
                self.floats = saved;
                BoxOut { bottom, top_y: byy, open: Collapse::one(st.margin_bottom), through: false }
            } else {
                let o = self.flow_block_impl(el, &st, x, w, anchor, open, false);
                if !o.through {
                    // `w` is the containing block's content width — for the
                    // full-width blocks that make up most of a page (and the
                    // "dark band across the page" case) it IS the box width.
                    self.record_inspect(el, &st, x, o.top_y, w, o.bottom - o.top_y);
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
            if track {
                if let ZIndex::Value(z) = st.z_index {
                    self.record_stack_entry(z, op0, self.ops.len(), link0, self.links.len());
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
            if let Some((text, ps)) = self.pseudo(owner, parent, PseudoElem::After) {
                inline.text(&text, &ps, None);
            }
        }
        if !inline.is_empty() {
            let ly = anchor + open.value() as i32;
            let nb = inline.flow(self.fonts, self.theme, x, w, ly, &self.floats, parent.text_align, parent.text_align_last, parent.rtl, parent.line_height.px(parent.font_px).unwrap_or(0.0), &mut self.ops, &mut self.links, &mut self.controls);
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
        let anc = self.path.len().saturating_sub(1);
        let (template, ps) =
            style::resolve_pseudo(owner, own, self.theme, self.sheet, &self.path[..anc], &[], 0, self.viewport_w, kind)?;
        Some((self.render_content(&template), ps))
    }

    /// Resolve a `content` template to its final text, reading any
    /// `counter()`/`counters()` against the current counter scope.
    fn render_content(&self, template: &[ContentPiece]) -> String {
        let mut out = String::new();
        for piece in template {
            match piece {
                ContentPiece::Text(s) => out.push_str(s),
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
    fn layout_block(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
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
    fn flow_block_impl(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, base_y: i32, incoming: Collapse, isolated: bool) -> BoxOut {
        let (cw, off_left) = resolve_block_h(st, w as f32);
        let content_x = x + off_left as i32;
        let content_w = cw.max(1.0) as i32;

        let box_left = content_x - st.pad_left as i32 - st.border_left.width as i32;
        let box_w = content_w + (st.pad_left + st.pad_right) as i32 + st.border_x() as i32;
        let bg_idx = self.ops.len();

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
            return BoxOut { bottom: y + 3 + pb, top_y: prov_top_y, open: Collapse::one(if isolated { 0.0 } else { st.margin_bottom }), through: false };
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
        // descendants (border-box top ≈ `prov_top_y`).
        let prev_cb = self.cb;
        if st.position != Position::Static {
            self.cb = (content_x, prov_top_y, content_w, definite_cb_height(st));
        }
        let flow = if st.pre {
            let ly = child_anchor + child_incoming.value() as i32;
            let nb = layout_pre(self.fonts.pick(st.bold, st.italic, st.mono), el, st, content_x, content_w, ly, &mut self.ops);
            Flow { bottom: nb, open: Collapse::default(), first_top: ly, committed: true }
        } else {
            self.flow_children(&el.children, st, Some(el), content_x, content_w, child_anchor, child_incoming)
        };
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
                return BoxOut { bottom: base_y, top_y: base_y, open, through: true };
            }
            let box_bottom = border_top_y + bt + pt + ch + pb + bb;
            self.paint_box_decoration(st, box_left, border_top_y, box_w, box_bottom - border_top_y, bg_idx);
            return BoxOut { bottom: box_bottom, top_y: border_top_y, open: out_bottom_margin, through: false };
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
        self.paint_box_decoration(st, box_left, border_top_y, box_w, box_bottom - border_top_y, bg_idx);
        BoxOut { bottom: box_bottom, top_y: border_top_y, open: out_open, through: false }
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
    fn img_box(&self, el: &Element) -> (i32, i32) {
        let img = el.attr("src").and_then(|s| self.images.get(s));
        let (iw, ih) = img.map(|i| (i.w as f32, i.h as f32)).unwrap_or((0.0, 0.0));
        let attr = |n: &str| el.attr(n).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
        let (aw, ah) = (attr("width"), attr("height"));
        if img.is_none() && (aw.is_none() || ah.is_none()) {
            if let Some(src) = el.attr("src") {
                self.guessed.borrow_mut().push(String::from(src));
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
        let (mut w, mut h) = match kind {
            ControlKind::Checkbox | ControlKind::Radio => {
                let s = (size * 0.9).max(12.0) as i32;
                (s, s)
            }
            ControlKind::TextArea => {
                let cols = el.attr("cols").and_then(|c| c.trim().parse::<f32>().ok()).unwrap_or(30.0);
                let rows = el.attr("rows").and_then(|r| r.trim().parse::<f32>().ok()).unwrap_or(3.0);
                (
                    (cols * ch_w) as i32 + 2 * pad_x + 2,
                    (rows * line) as i32 + 2 * CTL_PAD_Y + 2,
                )
            }
            ControlKind::Text | ControlKind::Password => {
                let cols = el.attr("size").and_then(|c| c.trim().parse::<f32>().ok()).unwrap_or(20.0);
                (
                    (cols * ch_w) as i32 + 2 * pad_x + 2,
                    ceil_i32(line) + 2 * CTL_PAD_Y + 2,
                )
            }
            ControlKind::Select => (
                ceil_i32(measure(font, &text, size)) + 2 * pad_x + CTL_ARROW + 2,
                ceil_i32(line) + 2 * CTL_PAD_Y + 2,
            ),
            _ => (
                ceil_i32(measure(font, &text, size)) + 2 * (pad_x + 4) + 2,
                ceil_i32(line) + 2 * CTL_PAD_Y + 2,
            ),
        };
        if let Some(cw) = st.width.px(avail) {
            // A CSS width is a content width unless `box-sizing: border-box`.
            w = if st.box_border { cw as i32 } else { cw as i32 + 2 * pad_x + 2 };
        }
        // A percentage height resolves against the containing block's HEIGHT
        // (§10.5), never `avail` (its width) — the checkbox-hack overlay is
        // `width:100%; height:100%`, and measuring its height off the width
        // made it as tall as its container is wide. An indefinite CB height
        // leaves the percentage unresolvable, so the intrinsic height stands.
        if let Some(chh) = vert_len(st.height, self.cb.3) {
            h = if st.box_border { chh as i32 } else { chh as i32 + 2 * CTL_PAD_Y + 2 };
        }
        if let Some(mx) = st.max_width.px(avail) {
            w = w.min(mx as i32);
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
            style: RunStyle { hidden: st.hidden, transparent: st.transparent, size, color: st.color, bold: st.bold, italic: st.italic, mono: st.mono, valign: 0, lh: st.line_height.px(size).unwrap_or(0.0) },
        }
    }

    /// Lay a `position:absolute`/`fixed` box, out of flow, at a position derived
    /// from the containing block (`self.cb`) + `top`/`right`/`bottom`/`left`.
    /// The element is `el`, already pushed onto `self.path` by the caller.
    fn layout_abs(&mut self, el: &Element, st: &ComputedStyle, static_x: i32, static_y: i32) {
        let (cbx, cby, cbw, cbh) = self.cb;
        let avail = cbw as f32;
        let left = st.left.px(avail);
        let right = st.right.px(avail);
        let width = match (st.width.px(avail), left, right) {
            (Some(wd), _, _) => wd,
            (None, Some(l), Some(r)) => (avail - l - r).max(0.0),
            _ => self.intrinsic_width(el, st).0.min(avail), // shrink-to-fit
        };
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
        if track {
            if let ZIndex::Value(z) = st.z_index {
                self.record_stack_entry(z, start, self.ops.len(), link_start, self.links.len());
            }
        }
        // The out-of-flow box, at its final (post-bottom-shift) position.
        let dy = bottom - box_bottom;
        self.record_inspect(el, st, px as i32, py as i32 + dy, w_i, box_bottom - py as i32);
    }

    /// Insert the block's `background-color` behind its content (at `bg_idx`)
    /// and stroke its `border` on the border-box edges.
    /// Insert a box's `background-color` behind the content it already emitted
    /// (at `bg_idx`). Split out from `paint_box_decoration` because a table can
    /// paint its background — an opaque infobox must not let the article text
    /// it floats over show through — while its BORDER still can't be drawn from
    /// here: the table box has no resolved border box yet, and guessing one
    /// puts the stroke in the wrong place (measured: 5 reftests).
    fn insert_bg(&mut self, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, bg_idx: usize) {
        if w <= 0 || h <= 0 || st.hidden || st.transparent {
            return;
        }
        let Some(bg) = st.bg else { return };
        self.ops.insert(bg_idx, DrawOp::Rect { x, y, w, h, color: bg });
        // `insert` shifts every later op up by one slot — any already-recorded
        // stacking range overlapping or after `bg_idx` (a descendant's tracked
        // z-index range, recorded before this, its ancestor's own background
        // gets painted in) must shift too. Half-open `[s, e)`: a range that
        // already ends at-or-before `bg_idx` is untouched (`e > bg_idx`,
        // strict — `e == bg_idx` means the insertion lands right after the
        // range, not inside it).
        for (_, s, e) in &mut self.stack_ops {
            if *s >= bg_idx {
                *s += 1;
            }
            if *e > bg_idx {
                *e += 1;
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
        self.insert_bg(st, x, y, w, h, bg_idx);
        // Each side paints independently on the border-box edge.
        let side = |ops: &mut Vec<DrawOp>, s: &BorderSide, rect: (i32, i32, i32, i32)| {
            if let (Some(c), true) = (s.color, s.width > 0.0) {
                let (rx, ry, rw, rh) = rect;
                if rw > 0 && rh > 0 {
                    ops.push(DrawOp::Rect { x: rx, y: ry, w: rw, h: rh, color: c });
                }
            }
        };
        let (bt, br, bb, bl) = (
            st.border_top.width as i32,
            st.border_right.width as i32,
            st.border_bottom.width as i32,
            st.border_left.width as i32,
        );
        side(&mut self.ops, &st.border_top, (x, y, w, bt));
        side(&mut self.ops, &st.border_bottom, (x, y + h - bb, w, bb));
        side(&mut self.ops, &st.border_left, (x, y, bl, h));
        side(&mut self.ops, &st.border_right, (x + w - br, y, br, h));
    }

    /// Simplified table layout. Two column models: `table-layout: auto` sizes
    /// columns from cell content (readable infoboxes + data tables); `table-
    /// layout: fixed` (CSS2 §17.5.2.1) takes column widths from the table/
    /// `<col>`/first-row cell `width`s and distributes the rest, painting each
    /// cell's own box (background/border/padding). Rows/cells are recognised by
    /// HTML tag (`tr`/`td`/`th`/`thead`…) or `display: table-*`; anonymous
    /// boxes fill any missing row/row-group/cell wrapper (CSS2 §17.2.1).
    fn layout_table(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // <caption> renders as a block above the grid.
        let mut y = y0;
        for c in &el.children {
            if let Node::Element(e) = c {
                if e.tag == "caption" {
                    let cs = style::resolve(e, st, self.theme, self.sheet, &self.path, &[], 0, self.viewport_w);
                    self.path.push(ElemInfo::of(e));
                    y = self.layout_children(&e.children, &cs, Some(e), x, w, y);
                    self.path.pop();
                }
            }
        }
        self.layout_table_body(&el.children, st, x, w, y)
    }

    /// The table's row grid (everything but `<caption>`): shared by a real
    /// `<table>` (`el.children`, above) and an anonymous table synthesized in
    /// `flow_children` around a stray run of table-part siblings that has no
    /// `table`/`inline-table` ancestor (CSS2 §17.2.1) — an anonymous table
    /// can't have a `<caption>` child (nothing selects an anonymous box), so
    /// only the row-collection step is shared.
    fn layout_table_body(&mut self, nodes: &[Node], st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        let mut rows = self.collect_table_rows(nodes, st);
        rows.retain(|r| !r.is_empty());
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0).min(64);
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
        let frame = (st.pad_left + st.pad_right + st.border_x()) as i32;
        let inner_w = (w - frame).max(0);
        let (colw, paint_cells) = if st.table_layout == TableLayout::Fixed {
            (self.fixed_columns(&rows, ncols, st, inner_w), true)
        } else {
            (self.auto_columns(&rows, ncols, st, inner_w), false)
        };
        // The table's own border box: border, then padding, then the row grid.
        // Getting the border edge in here is what lets the table paint its own
        // decoration at all — laying the grid at `x + pad_left` (no border
        // offset) put every stroke a border-width off.
        let (btl, btt) = (st.border_left.width as i32, st.border_top.width as i32);
        let inner_x = x + btl + st.pad_left as i32;
        let content_top = y0 + btt + st.pad_top as i32;
        let bg_idx = self.ops.len();
        let bottom = self.lay_table_rows(&rows, ncols, &colw, st, inner_x, content_top, paint_cells);
        let table_bottom = bottom + st.pad_bottom as i32 + st.border_bottom.width as i32;
        // A table box paints its own background and border like any other box;
        // only per-cell decoration is left to the boxes inside (see above).
        // Without this a floated infobox is transparent and the article text it
        // overlaps shows straight through it. Its used width comes from the
        // columns it actually produced, not from the space it was offered.
        let table_w = colw.iter().sum::<i32>() + frame;
        self.insert_bg(st, x, y0, table_w, table_bottom - y0, bg_idx);
        table_bottom
    }

    /// Auto table sizing (CSS2 §17.5.2.2, approximated): each column takes the
    /// widest cell's *border-box* preferred width (content + that cell's
    /// padding/border, or its explicit `width`). The table shrink-wraps to that,
    /// shrinking columns proportionally (never below their minimum) only when
    /// they overflow the available width; an explicit table `width` wider than
    /// the content spreads the slack across columns.
    fn auto_columns(&mut self, rows: &[Vec<Cell>], ncols: usize, st: &ComputedStyle, w: i32) -> Vec<i32> {
        let mut pref = vec![0.0f32; ncols];
        let mut minw = vec![0.0f32; ncols];
        for row in rows {
            for (c, cell) in row.iter().enumerate().take(ncols) {
                let cs = self.cell_style(cell, st);
                let frame = cs.pad_left + cs.pad_right + cs.border_x();
                let (p, m) = self.intrinsic_width_cell(cell, &cs);
                let spec = match cs.width.px(w as f32) {
                    Some(v) if cs.box_border => v,
                    Some(v) => v + frame,
                    None => 0.0,
                };
                pref[c] = pref[c].max((p + frame).max(spec));
                minw[c] = minw[c].max((m + frame).max(spec));
            }
        }
        // Dev: which column made a table too wide, and which cell drove it.
        #[cfg(feature = "diag-boxes")]
        {
            extern crate std;
            let who = self.path.last().map(|e| e.classes.join(".")).unwrap_or_default();
            std::eprintln!("[cols] {who} avail={w} cols={ncols} total={:.0} pref={:?}", pref.iter().sum::<f32>(), pref.iter().map(|v| *v as i32).collect::<Vec<_>>());
            for (c, p) in pref.iter().enumerate() {
                let mut widest = (0.0f32, String::new());
                for row in rows.iter() {
                    if let Some(cell) = row.get(c) {
                        let cs = self.cell_style(cell, st);
                        let (cp, _) = self.intrinsic_width_cell(cell, &cs);
                        if cp > widest.0 {
                            let mut t = String::new();
                            match cell {
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
        } else if !table_auto && total < content_w && total > 0.0 {
            let extra = (content_w - total) / ncols as f32;
            for c in 0..ncols {
                colw[c] += extra;
            }
        }
        colw.iter().map(|v| (v + 0.5) as i32).collect()
    }

    /// `table-layout: fixed` column sizing (CSS2 §17.5.2.1): column widths come
    /// from the first row's cell `width`s (each a *border-box* width), and the
    /// rest of the table's used width is split equally across the remaining
    /// columns; content never widens a column.
    fn fixed_columns(&self, rows: &[Vec<Cell>], ncols: usize, st: &ComputedStyle, w: i32) -> Vec<i32> {
        let content_w = table_content_width(st, w as f32);
        // Per-column border-box width; None = "auto" (share the leftover).
        let mut fixed: Vec<Option<f32>> = vec![None; ncols];
        if let Some(first) = rows.first() {
            for (c, cell) in first.iter().enumerate().take(ncols) {
                let cs = self.cell_style(cell, st);
                if let Some(cw) = cs.width.px(content_w) {
                    let border_box = if cs.box_border {
                        cw
                    } else {
                        cw + cs.pad_left + cs.pad_right + cs.border_x()
                    };
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

    /// A cell's own computed style: a real cell resolves normally (`st` — the
    /// table's style — stands in for its row's, an existing approximation);
    /// an anonymous cell gets the CSS2.1 §17.2.1 anonymous-box style (inherited
    /// properties from `st`, every other property at its initial value).
    fn cell_style(&self, cell: &Cell, st: &ComputedStyle) -> ComputedStyle {
        match cell {
            Cell::Real(e) => style::resolve(e, st, self.theme, self.sheet, &self.path, &[], 0, self.viewport_w),
            Cell::Anon(_) => style::anon_inherit(st, Display::TableCell),
        }
    }

    /// Lay a table's rows given resolved (border-box) column widths. Cells sit
    /// side by side; each cell box stretches to the row's tallest cell and paints
    /// its own background/border, with content placed inside its padding.
    fn lay_table_rows(&mut self, rows: &[Vec<Cell>], ncols: usize, colw: &[i32], st: &ComputedStyle, x: i32, y0: i32, paint_cells: bool) -> i32 {
        let mut y = y0;
        for row in rows {
            // Pass 1: resolve cell styles + measure the tallest cell.
            let mut cells: Vec<(ComputedStyle, i32, i32, i32)> = Vec::new(); // (style, cell_x, content_x, content_w)
            let mut cx = x;
            for (c, cell) in row.iter().enumerate().take(ncols) {
                let cw = colw[c];
                let cs = self.cell_style(cell, st);
                let content_x = cx + cs.border_left.width as i32 + cs.pad_left as i32;
                let content_w = (cw - cs.border_x() as i32 - (cs.pad_left + cs.pad_right) as i32).max(0);
                cells.push((cs, cx, content_x, content_w));
                cx += cw;
            }
            // Row height = the tallest cell border-box (content, or explicit height).
            let mut row_h = 0i32;
            for (c, (cs, _, content_x, content_w)) in cells.iter().enumerate() {
                let content_y = y + cs.border_top.width as i32 + cs.pad_top as i32;
                let mut ch = if cs.display == Display::None {
                    0
                } else {
                    self.measure_cell_height(&row[c], cs, *content_x, *content_w, content_y)
                };
                if let Len::Px(h) = cs.height {
                    let hb = if cs.box_border {
                        (h as i32 - (cs.pad_top + cs.pad_bottom) as i32 - cs.border_y() as i32).max(0)
                    } else {
                        h as i32
                    };
                    ch = ch.max(hb);
                }
                let cell_box_h = ch + (cs.pad_top + cs.pad_bottom) as i32 + cs.border_y() as i32;
                row_h = row_h.max(cell_box_h);
            }
            // Pass 2: emit content + paint each cell's border-box at row height.
            for (c, (cs, cell_x, content_x, content_w)) in cells.iter().enumerate() {
                if cs.display == Display::None {
                    continue;
                }
                let content_y = y + cs.border_top.width as i32 + cs.pad_top as i32;
                let bg_idx = self.ops.len();
                match row[c] {
                    Cell::Real(e) => {
                        self.path.push(ElemInfo::of(e));
                        let _ = self.layout_children(&e.children, cs, Some(e), *content_x, *content_w, content_y);
                        self.path.pop();
                    }
                    Cell::Anon(nodes) => {
                        let _ = self.layout_children(nodes, cs, None, *content_x, *content_w, content_y);
                    }
                }
                if paint_cells {
                    self.paint_box_decoration(cs, *cell_x, y, colw[c], row_h, bg_idx);
                }
            }
            y += row_h;
        }
        y
    }

    /// Lay an element's children to measure their flowed height without emitting
    /// any draw ops (used to size table rows before painting cell boxes).
    fn measure_children_height(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        let (o, l, c) = (self.ops.len(), self.links.len(), self.controls.len());
        // Stacking ranges index into `ops`/`links`, so a discarded speculative
        // layout has to drop the ones it recorded too — otherwise they survive
        // pointing into a vector that was truncated behind them, and
        // `reorder_by_z` (which needs disjoint ascending ranges) slices the
        // real display list at the wrong offsets.
        let (so, sl) = (self.stack_ops.len(), self.stack_links.len());
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
        self.floats.truncate(fl);
        (bottom - y).max(0)
    }

    /// Same as `measure_children_height`, for a table cell that may be an
    /// anonymous box (no owning element to push on `self.path`).
    fn measure_cell_height(&mut self, cell: &Cell, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        match cell {
            Cell::Real(e) => self.measure_children_height(e, st, x, w, y),
            Cell::Anon(nodes) => {
                let (o, l, c) = (self.ops.len(), self.links.len(), self.controls.len());
                let (so, sl) = (self.stack_ops.len(), self.stack_links.len());
                let fl = self.floats.len();
                let bottom = self.layout_children(nodes, st, None, x, w.max(0), y);
                self.ops.truncate(o);
                self.links.truncate(l);
                self.controls.truncate(c);
                self.stack_ops.truncate(so);
                self.stack_links.truncate(sl);
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
                let st = style::resolve(e, parent, self.theme, self.sheet, &self.path, &[], 0, self.viewport_w);
                match st.display {
                    Display::TableRow => TableRole::Row,
                    Display::TableRowGroup => TableRole::RowGroup,
                    Display::TableHeaderGroup => TableRole::HeaderGroup,
                    Display::TableFooterGroup => TableRole::FooterGroup,
                    Display::TableCell => TableRole::Cell,
                    Display::TableColumn | Display::TableColumnGroup => TableRole::Skip,
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
    fn collect_table_rows<'a>(&self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<Vec<Cell<'a>>> {
        let mut header = Vec::new();
        let mut body = Vec::new();
        let mut footer = Vec::new();
        self.collect_rows_into(nodes, parent, &mut header, &mut body, &mut footer);
        header.extend(body);
        header.extend(footer);
        header
    }

    /// Walk `nodes` (a table's or row-group's children), bucketing each row by
    /// group kind. A child that is a `table-row`/`-row-group`/`-header-group`/
    /// `-footer-group` is a proper table child and recurses/becomes a row
    /// directly; any other maximal run of consecutive siblings (stray cells,
    /// stray text, stray elements — anything that isn't a proper table child)
    /// is wrapped in ONE anonymous row (whitespace-only text neither starts
    /// nor breaks a run, and is dropped if it's all a run ever contained).
    fn collect_rows_into<'a>(
        &self,
        nodes: &'a [Node],
        parent: &ComputedStyle,
        header: &mut Vec<Vec<Cell<'a>>>,
        body: &mut Vec<Vec<Cell<'a>>>,
        footer: &mut Vec<Vec<Cell<'a>>>,
    ) {
        let mut run_start: Option<usize> = None;
        let mut run_has_content = false;
        for (i, n) in nodes.iter().enumerate() {
            let role = match n {
                Node::Element(e) => Some(self.table_role(e, parent)),
                Node::Text(_) => None,
            };
            match role {
                Some(TableRole::Row) | Some(TableRole::RowGroup) | Some(TableRole::HeaderGroup) | Some(TableRole::FooterGroup) => {
                    if let Some(s) = run_start.take() {
                        if run_has_content {
                            body.push(self.partition_cells(&nodes[s..i], parent));
                        }
                        run_has_content = false;
                    }
                    let Node::Element(e) = n else { unreachable!() };
                    match role {
                        Some(TableRole::Row) => body.push(self.partition_cells(&e.children, parent)),
                        Some(TableRole::RowGroup) => self.collect_rows_into(&e.children, parent, header, body, footer),
                        Some(TableRole::HeaderGroup) => {
                            let (mut h, mut b, mut f) = (Vec::new(), Vec::new(), Vec::new());
                            self.collect_rows_into(&e.children, parent, &mut h, &mut b, &mut f);
                            header.extend(h);
                            header.extend(b);
                            header.extend(f);
                        }
                        Some(TableRole::FooterGroup) => {
                            let (mut h, mut b, mut f) = (Vec::new(), Vec::new(), Vec::new());
                            self.collect_rows_into(&e.children, parent, &mut h, &mut b, &mut f);
                            footer.extend(h);
                            footer.extend(b);
                            footer.extend(f);
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
        }
        if let Some(s) = run_start {
            if run_has_content {
                body.push(self.partition_cells(&nodes[s..], parent));
            }
        }
    }

    /// Partition a row's children into cells (CSS2 §17.2.1): a proper
    /// `table-cell` child stays its own (real) cell; any other maximal run of
    /// consecutive siblings (stray text, stray non-cell elements) is wrapped in
    /// ONE anonymous cell. Shared by a real `<tr>`'s children and an anonymous
    /// row's coalesced node run.
    fn partition_cells<'a>(&self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<Cell<'a>> {
        let mut cells = Vec::new();
        let mut run_start: Option<usize> = None;
        let mut run_has_content = false;
        for (i, n) in nodes.iter().enumerate() {
            let role = match n {
                Node::Element(e) => Some(self.table_role(e, parent)),
                Node::Text(_) => None,
            };
            match role {
                Some(TableRole::Cell) => {
                    if let Some(s) = run_start.take() {
                        if run_has_content {
                            cells.push(Cell::Anon(&nodes[s..i]));
                        }
                        run_has_content = false;
                    }
                    let Node::Element(e) = n else { unreachable!() };
                    cells.push(Cell::Real(e));
                }
                Some(TableRole::Skip) => {}
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
        }
        if let Some(s) = run_start {
            if run_has_content {
                cells.push(Cell::Anon(&nodes[s..]));
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
    fn intrinsic_width(&mut self, el: &Element, st: &ComputedStyle) -> (f32, f32) {
        if let Some(hit) = self.intrinsic.get(&el.seq) {
            return *hit;
        }
        // A control has no text children to measure — without this it sizes to
        // 0 as a flex/grid item and disappears.
        let out = if let Some(kind) = crate::forms::kind_of(el) {
            if kind == ControlKind::Hidden {
                (0.0, 0.0)
            } else {
                let rst = ComputedStyle::root(self.theme);
                let w = self.control_box(el, &rst, kind, 0.0).w as f32;
                (w, w)
            }
        } else {
            // `el`'s children cascade with `el` as their parent, so it has to
            // be on the ancestor path — unless a caller (the abspos path) put
            // it there already. Without this their descendant selectors match
            // against `el`'s parent and resolve the wrong `display`, which is
            // exactly what the anonymous-table-object reftests measure.
            let push = self.path.last().map(|p| p.seq) != Some(el.seq);
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
        self.intrinsic.insert(el.seq, out);
        out
    }

    /// `intrinsic_width` over a bare node slice — an anonymous cell has no
    /// owning element to gather text from. `st` is the style the slice's
    /// content inherits from.
    fn intrinsic_width_nodes(&mut self, nodes: &[Node], st: &ComputedStyle) -> (f32, f32) {
        let (mut pref, mut min) = (0.0f32, 0.0f32);
        let mut run = String::new();
        self.intrinsic_walk(nodes, st, &mut run, &mut pref, &mut min);
        flush_run(self.fonts, st, &mut run, &mut pref, &mut min, side_by_side(st));
        (pref, min)
    }

    /// A table's own (max-content, min-content) width: each column takes its
    /// widest cell, and the table is the sum of its columns. Deliberately the
    /// same decomposition `auto_columns` lays out with — `collect_table_rows`
    /// owns the CSS2.1 §17.2.1 anonymous-object fixup, so measuring through it
    /// keeps the measurement and the layout from drifting apart.
    fn intrinsic_table(&mut self, nodes: &[Node], st: &ComputedStyle) -> (f32, f32) {
        let mut rows = self.collect_table_rows(nodes, st);
        rows.retain(|r| !r.is_empty());
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0).min(64);
        if ncols == 0 {
            return (0.0, 0.0);
        }
        let (mut pref, mut minw) = (vec![0.0f32; ncols], vec![0.0f32; ncols]);
        for row in &rows {
            for (c, cell) in row.iter().enumerate().take(ncols) {
                let cs = self.cell_style(cell, st);
                let frame = cs.pad_left + cs.pad_right + cs.border_x();
                let (p, m) = self.intrinsic_width_cell(cell, &cs);
                pref[c] = pref[c].max(p + frame);
                minw[c] = minw[c].max(m + frame);
            }
        }
        (pref.iter().sum(), minw.iter().sum())
    }

    /// Walk `nodes` as one block container's contents, accumulating inline
    /// content into `run` and folding each block-level child's own measurement
    /// into `pref`/`min`. `st` is the parent style the children cascade from;
    /// `self.path` must already end at their parent.
    fn intrinsic_walk(&mut self, nodes: &[Node], st: &ComputedStyle, run: &mut String, pref: &mut f32, min: &mut f32) {
        let horiz = side_by_side(st);
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
                TableSeg::Node(n) => self.intrinsic_node(n, st, run, pref, min, horiz),
            }
        }
    }

    /// One node of a block container's content walk (see `intrinsic_walk`).
    /// `horiz` says whether this container's children sit side by side, so a
    /// finished box adds to the running width instead of competing with it.
    fn intrinsic_node(&mut self, n: &Node, st: &ComputedStyle, run: &mut String, pref: &mut f32, min: &mut f32, horiz: bool) {
        let el = match n {
            Node::Text(t) => {
                run.push_str(t);
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
        let cs = style::resolve(el, st, self.theme, self.sheet, &self.path, &[], 0, self.viewport_w);
        // Not rendered, or out of flow → contributes no intrinsic width.
        if cs.display == Display::None || matches!(cs.position, Position::Absolute | Position::Fixed) {
            return;
        }
        // An inline box's text joins the line its parent is building.
        if cs.display == Display::Inline && crate::forms::kind_of(el).is_none() && el.tag != "img" {
            self.path.push(ElemInfo::of(el));
            self.intrinsic_walk(&el.children, &cs, run, pref, min);
            self.path.pop();
            return;
        }
        // Everything else is a box of its own: an atomic inline (image, form
        // control) or a block-level child. Either way it ends the current line.
        let (p, m) = if el.tag == "img" {
            self.path.push(ElemInfo::of(el));
            let (iw, _) = self.img_box(el);
            self.path.pop();
            (iw as f32, iw as f32)
        } else {
            let (p, m) = self.intrinsic_width(el, &cs);
            let frame = cs.pad_left + cs.pad_right + cs.border_x();
            (p + frame, m + frame)
        };
        flush_run(self.fonts, st, run, pref, min, horiz);
        if horiz {
            *pref += p;
            *min += m;
        } else {
            *pref = pref.max(p);
            *min = min.max(m);
        }
    }

    /// `intrinsic_width`, dispatching on whether the cell is real or anonymous.
    fn intrinsic_width_cell(&mut self, cell: &Cell, st: &ComputedStyle) -> (f32, f32) {
        match cell {
            Cell::Real(e) => self.intrinsic_width(e, st),
            Cell::Anon(nodes) => self.intrinsic_width_nodes(nodes, st),
        }
    }

    /// Split `nodes` into pass-through single nodes and maximal runs of
    /// `table-row`/`-row-group`/`-header-group`/`-footer-group`/`-cell`
    /// siblings (whitespace-only text between them doesn't break a run). A run
    /// found here has no `table`/`inline-table` ancestor — `flow_children`
    /// wraps it in one anonymous `table` box (CSS2 §17.2.1) instead of laying
    /// each part out as an ordinary block.
    fn segment_table_runs<'a>(&self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<TableSeg<'a>> {
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
    fn layout_box(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
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
    fn layout_grid(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // No template at all → a grid degenerates to a block box.
        if st.grid_ncols == 0 && st.grid_nrows == 0 {
            return self.layout_block(el, st, x, w, y0);
        }

        // Container horizontal box (mirrors `layout_block`).
        let (cw, off_left) = resolve_block_h(st, w as f32);
        let content_x = x + off_left as i32;
        let content_w = cw.max(1.0) as i32;
        let box_left = content_x - st.pad_left as i32;
        let box_w = content_w + (st.pad_left + st.pad_right) as i32;
        let bg_idx = self.ops.len();
        let content_top = y0 + st.pad_top as i32;

        let prev_cb = self.cb;
        if st.position != Position::Static {
            self.cb = (content_x, content_top, content_w, definite_cb_height(st));
        }
        let content_h = self.grid_content(el, st, content_x, content_w, content_top);
        self.cb = prev_cb;

        // Explicit / min / max height clamp the content-box height.
        let pad_v = st.pad_top as i32 + st.pad_bottom as i32;
        let px_h = |len: Len| match len {
            Len::Px(h) if st.box_border => Some((h as i32 - pad_v).max(0)),
            Len::Px(h) => Some(h as i32),
            _ => None,
        };
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

        let y = content_top + ch + st.pad_bottom as i32;
        self.paint_box_decoration(st, box_left, y0, box_w, y - y0, bg_idx);
        y
    }

    /// Lay a grid container's items inside its content box `(x, w, y0)`, returning
    /// the content-box height. `self.cb` is already set for the container.
    fn grid_content(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
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
        for c in &el.children {
            if let Node::Element(ce) = c {
                let cs = style::resolve(ce, st, self.theme, self.sheet, &self.path, &[], 0, self.viewport_w);
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
        let pad_v = st.pad_top + st.pad_bottom;
        let def_h: Option<f32> = match st.height {
            Len::Px(h) if st.box_border => Some((h - pad_v).max(0.0)),
            Len::Px(h) => Some(h),
            _ => None,
        };

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
    fn measure_box_height(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {
        let (o, l, c) = (self.ops.len(), self.links.len(), self.controls.len());
        // Stacking ranges index into `ops`/`links`, so a discarded speculative
        // layout has to drop the ones it recorded too — otherwise they survive
        // pointing into a vector that was truncated behind them, and
        // `reorder_by_z` (which needs disjoint ascending ranges) slices the
        // real display list at the wrong offsets.
        let (so, sl) = (self.stack_ops.len(), self.stack_links.len());
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
    fn layout_flex(&mut self, el: &Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {
        // Flex items = in-flow child elements; abspos children are out of flow.
        let mut items: Vec<(&Element, ComputedStyle)> = Vec::new();
        for c in &el.children {
            if let Node::Element(ce) = c {
                let cs = style::resolve(ce, st, self.theme, self.sheet, &self.path, &[], 0, self.viewport_w);
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
        // Empty flex box: fall back to block so its own box decoration still paints.
        if items.is_empty() {
            return self.layout_block(el, st, x, w, y0);
        }
        items.sort_by_key(|(_, s)| s.order); // stable → equal order keeps DOM order

        // Container horizontal box (mirrors `layout_block`/`layout_grid`).
        let (cw, off_left) = resolve_block_h(st, w as f32);
        let content_x = x + off_left as i32;
        let content_w = cw.max(1.0) as i32;
        let box_left = content_x - st.pad_left as i32;
        let box_w = content_w + (st.pad_left + st.pad_right) as i32;
        let bg_idx = self.ops.len();
        let content_top = y0 + st.pad_top as i32;

        let prev_cb = self.cb;
        if st.position != Position::Static {
            self.cb = (content_x, content_top, content_w, definite_cb_height(st));
        }

        // Definite container content height (for cross-stretch / main-axis flex).
        let pad_v = st.pad_top + st.pad_bottom;
        let def_h: Option<f32> = match st.height {
            Len::Px(h) if st.box_border => Some((h - pad_v).max(0.0)),
            Len::Px(h) => Some(h),
            _ => None,
        };

        let content_h = if st.flex_row {
            self.flex_row(&items, st, content_x, content_w, content_top, def_h)
        } else {
            self.flex_column(&items, st, content_x, content_w, content_top, def_h)
        };
        self.cb = prev_cb;

        // Explicit / min / max height clamp the content-box height.
        let pad_vi = st.pad_top as i32 + st.pad_bottom as i32;
        let px_h = |len: Len| match len {
            Len::Px(h) if st.box_border => Some((h as i32 - pad_vi).max(0)),
            Len::Px(h) => Some(h as i32),
            _ => None,
        };
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

        let y = content_top + ch + st.pad_bottom as i32;
        self.paint_box_decoration(st, box_left, y0, box_w, y - y0, bg_idx);
        y
    }

    /// Row flex (main axis = horizontal, cross axis = vertical). `def_cross` is
    /// the container's definite content height (cross size) if any. Returns the
    /// content-box height consumed by all lines.
    fn flex_row(
        &mut self,
        items: &[(&Element, ComputedStyle)],
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
                h_nat[k] = self.measure_box_height(el, &s_meas, item_x[k] as i32, size[k].max(1.0) as i32, cross_y);
            }

            // Line cross size: a single unwrapped line fills a definite container
            // height; otherwise it's the tallest item margin box.
            let nat_line = (0..ln)
                .map(|k| li[k].cm_lead as i32 + h_nat[k] + li[k].cm_trail as i32)
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
                let _ = self.layout_box(el, &s2, item_x[k] as i32, size[k].max(1.0) as i32, y);
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
        items: &[(&Element, ComputedStyle)],
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
    fn flex_metrics(&mut self, items: &[(&Element, ComputedStyle)], avail: f32, row: bool) -> Vec<FlexItem> {
        let mut out = Vec::with_capacity(items.len());
        for (el, s) in items {
            let (main_pad, cross_pad) = if row {
                (s.pad_left + s.pad_right, s.pad_top + s.pad_bottom)
            } else {
                (s.pad_top + s.pad_bottom, s.pad_left + s.pad_right)
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
            let (pref, minc) = self.intrinsic_width(el, s);
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
                | DrawOp::Image { x, y, .. } => {
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
    let mut size: Vec<f32> = li.iter().map(|it| it.hypo).collect();
    let sum: f32 = size.iter().sum();
    let free = avail - sum - gaps_total;
    if free > 0.5 {
        let tg: f32 = li.iter().map(|it| it.grow).sum();
        if tg > 0.0 {
            for (i, it) in li.iter().enumerate() {
                size[i] = (it.base + free * it.grow / tg).clamp(it.floor.min(it.ceil), it.ceil);
            }
        }
    } else if free < -0.5 {
        let ts: f32 = li.iter().map(|it| it.shrink * it.base).sum();
        if ts > 0.0 {
            for (i, it) in li.iter().enumerate() {
                size[i] = (it.base + free * (it.shrink * it.base) / ts).clamp(it.floor.min(it.ceil), it.ceil);
            }
        }
    }
    size
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
    if row {
        s2.width = main_px(main, s.pad_left + s.pad_right);
        if let Some(c) = forced_cross {
            let pad_v = s.pad_top + s.pad_bottom;
            s2.height = Len::Px(if s.box_border { c } else { (c - pad_v).max(0.0) });
        }
    } else {
        // Column: main axis is vertical (height), cross is horizontal (width).
        s2.width = main_px(main, s.pad_left + s.pad_right);
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

/// Measure the inline text collected so far as one line and fold it into a
/// box's running (max-content, min-content), then clear it. `white-space:
/// normal` collapses every whitespace run — including the newlines and
/// indentation between sibling tags in pretty-printed markup — to one space
/// first, or source formatting would count as visible width.
fn flush_run(fonts: &crate::fonts::Fonts, st: &ComputedStyle, run: &mut String, pref: &mut f32, min: &mut f32, horiz: bool) {
    if run.is_empty() {
        return;
    }
    // `white-space: pre` keeps the source line breaks, so each source line is
    // its own line box and the widest one wins — collapsing them into one
    // would measure a whole code block as a single enormous line.
    if st.pre {
        let font = fonts.pick(st.bold, st.italic, st.mono);
        let mut widest = 0.0f32;
        for line in run.lines() {
            // Trailing spaces hang past the line box, so they never widen it
            // (css-text-3 §8). Leading ones DO count under `pre`.
            widest = widest.max(measure(font, line.trim_end(), st.font_px));
        }
        run.clear();
        if horiz {
            *pref += widest;
            *min += widest;
        } else {
            *pref = pref.max(widest);
            *min = min.max(widest);
        }
        return;
    }
    let collapsed = collapse_whitespace(run);
    run.clear();
    // The run's OWN font, not `regular()`: monospace advances wider than the
    // proportional face, so measuring mono content with it under-sizes every
    // auto table column that holds code.
    let font = fonts.pick(st.bold, st.italic, st.mono);
    let size = st.font_px;
    let p = measure(font, collapsed.trim(), size);
    let m = collapsed.split_whitespace().map(|wd| measure(font, wd, size)).fold(0.0f32, f32::max);
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

impl Ctx<'_> {
    /// Collect an inline element's subtree into the current inline run
    /// (recursing through nested inline elements, carrying each one's style +
    /// link href). `el` is already on `self.path` when this is called.
    fn collect_inline(&mut self, el: &Element, st: &ComputedStyle, href: Option<&str>, inline: &mut Inline, bx: i32, bw: i32, by: i32) {
        if st.is_break {
            inline.brk();
            return;
        }
        // An `<img>` inside inline content (e.g. `<a><img></a>` — Wikipedia's
        // thumbnails) is an atomic inline box; carry the enclosing link so it
        // stays clickable.
        if el.tag == "img" {
            let (iw, ih) = self.img_box(el);
            let src = el.attr("src").unwrap_or("").to_string();
            inline.image(src, iw, ih, href, el.attr("alt").unwrap_or("").trim().to_string(), st.hidden, st.transparent);
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
        if let Some((text, ps)) = self.pseudo(el, st, PseudoElem::Before) {
            inline.text(&text, &ps, href);
        }
        for c in &el.children {
            match c {
                Node::Text(t) => inline.text(t, st, href),
                Node::Element(ce) => {
                    let cs = style::resolve(ce, st, self.theme, self.sheet, &self.path, &[], 0, self.viewport_w);
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
                    self.path.push(ElemInfo::of(ce));
                    self.collect_inline(ce, &cs, href, inline, bx, bw, by);
                    self.path.pop();
                }
            }
        }
        if let Some((text, ps)) = self.pseudo(el, st, PseudoElem::After) {
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
    valign: i8, // vertical-align: super (+1) / sub (-1) / baseline (0)
    /// Used `line-height` in px, or 0 for `normal` (use the face's metrics).
    lh: f32,
}

/// One inline item: a word, an atomic `<img>`, a form control, or a `<br>`.
enum Item {
    Word { text: String, style: RunStyle, href: Option<String>, space_before: bool },
    /// An inline box that generated no content of its own. It still contributes
    /// its leading to any line box it lands in (CSS 2.1 §10.8) — `<span
    /// style="line-height:5"></span>X` is a tall line — but it never makes a
    /// line non-empty, so a line holding nothing else is still not generated.
    Strut(RunStyle),
    Image { src: String, w: i32, h: i32, href: Option<String>, alt: String, space_before: bool, hidden: bool, transparent: bool },
    Control { ctl: CtlBox, space_before: bool },
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
    style: RunStyle,
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
    let border = if ctl.focused { theme.link } else { mix(theme.rule, theme.text, 40) };
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
            stroke_rect(ops, x, top, w, h, border);
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
            stroke_rect(ops, x, top, w, h, border);
            let tx = x + CTL_PAD_X + 1;
            let lh = ceil_i32(line_gap(font, ctl.style.size));
            let ty = top + (h - lh) / 2;
            if ctl.kind == ControlKind::TextArea {
                // Multi-line: honour hard newlines and wrap on width, top-
                // aligned, clipped to the rows that fit in the box.
                let inner_w = (w - 2 * CTL_PAD_X - 2).max(1) as f32;
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
                let inner = (w - 2 * CTL_PAD_X - 2).max(0) as f32;
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
                let inner = (w - 2 * CTL_PAD_X - 2) as f32;
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
    fn text(&mut self, raw: &str, st: &ComputedStyle, href: Option<&str>) {
        let rs = RunStyle { hidden: st.hidden, transparent: st.transparent, size: st.font_px, color: st.color, bold: st.bold, italic: st.italic, mono: st.mono, valign: st.valign, lh: st.line_height.px(st.font_px).unwrap_or(0.0) };
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
    fn image(&mut self, src: String, w: i32, h: i32, href: Option<&str>, alt: String, hidden: bool, transparent: bool) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Image { src, w, h, href: href.map(|s| s.to_string()), alt, space_before, hidden, transparent });
    }

    /// Add an atomic form control to the inline run.
    fn control(&mut self, ctl: CtlBox) {
        let space_before = self.pending_space && !self.items.is_empty();
        self.pending_space = false;
        self.items.push(Item::Control { ctl, space_before });
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
        strut: f32,
        ops: &mut Vec<DrawOp>,
        links: &mut Vec<LinkRect>,
        controls: &mut Vec<ControlRect>,
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
        let mut pen = l0 as f32;
        let mut line_ascent = 0.0f32;
        let mut gap = 0.0f32;
        let mut right = r0 as f32;

        for item in &self.items {
            match item {
                Item::Strut(style) => {
                    let (asc, lb) = run_metrics(face(style), style.size, style.lh);
                    line_ascent = line_ascent.max(asc);
                    gap = gap.max(lb);
                }
                Item::Break => {
                    if line.is_empty() {
                        y += ceil_i32(strut_h);
                    } else {
                        y = emit_line(fonts, theme, &mut line, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls);
                    }
                    let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                    pen = bl as f32;
                    right = br as f32;
                    line_ascent = 0.0;
                    gap = 0.0;
                }
                Item::Word { text, style, href, space_before } => {
                    let ww = measure(face(style), text, style.size);
                    let sw = if *space_before { space_width(face(style), style.size) } else { 0.0 };
                    if !line.is_empty() && pen + sw + ww > right {
                        y = emit_line(fonts, theme, &mut line, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls);
                        let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                        pen = bl as f32;
                        right = br as f32;
                        line_ascent = 0.0;
                        gap = 0.0;
                    }
                    let lead = if line.is_empty() { 0.0 } else { sw };
                    let sx = (pen + lead) as i32;
                    let merge = matches!(line.last(), Some(Placed::Text(last)) if last.style == *style && last.href == *href);
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
                    let (asc, lb) = run_metrics(face(style), style.size, style.lh);
                    line_ascent = line_ascent.max(asc);
                    gap = gap.max(lb);
                }
                Item::Image { src, w: iw, h: ih, href, alt, space_before, hidden, transparent } => {
                    // Fit the image to the content width, keeping aspect.
                    let (mut bw, mut bh) = (*iw as f32, *ih as f32);
                    if bw > w as f32 {
                        bh *= w as f32 / bw;
                        bw = w as f32;
                    }
                    let (bw, bh) = (bw.max(1.0) as i32, bh.max(1.0) as i32);
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX) } else { 0.0 };
                    if !line.is_empty() && pen + sw + bw as f32 > right {
                        y = emit_line(fonts, theme, &mut line, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls);
                        let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                        pen = bl as f32;
                        right = br as f32;
                        line_ascent = 0.0;
                        gap = 0.0;
                    }
                    let lead = if line.is_empty() { 0.0 } else { sw };
                    let sx = (pen + lead) as i32;
                    line.push(Placed::Image {
                        x: sx,
                        w: bw,
                        h: bh,
                        src: src.clone(),
                        href: href.clone(),
                        alt: alt.clone(),
                        hidden: *hidden,
                        transparent: *transparent,
                    });
                    pen += lead + bw as f32;
                    line_ascent = line_ascent.max(bh as f32);
                    gap = gap.max(bh as f32 + 2.0);
                }
                Item::Control { ctl, space_before } => {
                    let sw = if *space_before { space_width(fonts.regular(), BASE_FONT_PX) } else { 0.0 };
                    if !line.is_empty() && pen + sw + ctl.w as f32 > right {
                        y = emit_line(fonts, theme, &mut line, y, line_ascent, gap, align_dx(align, rtl, pen, right), ops, links, controls);
                        let (bl, br) = band_of(floats, y, y + lh, x, x + w);
                        pen = bl as f32;
                        right = br as f32;
                        line_ascent = 0.0;
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
        if !line.is_empty() {
            let a = align_last.unwrap_or(align);
            y = emit_line(fonts, theme, &mut line, y, line_ascent, gap, align_dx(a, rtl, pen, right), ops, links, controls);
        }
        y
    }
}

/// One item placed on the current line: a same-style text run, an image, or a
/// form control (borrowed from the inline run — it is only measured once).
enum Placed<'a> {
    Text(Seg),
    Image { x: i32, w: i32, h: i32, src: String, href: Option<String>, alt: String, hidden: bool, transparent: bool },
    Control { x: i32, ctl: &'a CtlBox },
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
fn emit_line(
    fonts: &crate::fonts::Fonts,
    theme: &Theme,
    line: &mut Vec<Placed<'_>>,
    y: i32,
    line_ascent: f32,
    gap: f32,
    dx: i32,
    ops: &mut Vec<DrawOp>,
    links: &mut Vec<LinkRect>,
    controls: &mut Vec<ControlRect>,
) -> i32 {
    let line_top = y;
    let baseline = y + line_ascent as i32;
    let box_h = ceil_i32(gap).max(1);
    for placed in line.drain(..) {
        match placed {
            Placed::Text(seg) => {
                let font = fonts.pick(seg.style.bold, seg.style.italic, seg.style.mono);
                let mut top = baseline - ascent_i(font, seg.style.size);
                // vertical-align: raise a superscript, drop a subscript off the
                // shared baseline (the run is already at its reduced sup/sub size).
                match seg.style.valign {
                    1 => top -= (seg.style.size * 0.42) as i32,
                    -1 => top += (seg.style.size * 0.18) as i32,
                    _ => {}
                }
                // A hidden run is not a click target either — otherwise a
                // collapsed dropdown leaves invisible links over the article.
                if let (Some(h), false) = (&seg.href, seg.style.hidden) {
                    let sw = measure(font, &seg.text, seg.style.size);
                    links.push(LinkRect { x: seg.x + dx, y: line_top, w: ceil_i32(sw), h: box_h, href: h.clone() });
                }
                if !seg.style.hidden && !seg.style.transparent {
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
            Placed::Control { x, ctl } => {
                let top = baseline - (ctl.h - CTL_PAD_Y);
                paint_control(fonts, theme, ctl, x + dx, top, ops, controls);
            }
            Placed::Image { x, w, h, src, href, alt, hidden, transparent } => {
                let x = x + dx;
                let top = baseline - h; // image bottom sits on the baseline
                if let (Some(href), false) = (&href, hidden) {
                    links.push(LinkRect { x, y: top, w, h, href: href.clone() });
                }
                // Emitted whether or not the pixels have arrived — the
                // rasteriser draws the placeholder when the lookup misses.
                if !hidden && !transparent {
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
        let sheet = crate::css::collect(&dom, 800.0);
        layout(&fonts(), &dom, &sheet, &crate::image::ImageMap::new(), w, 600, &Theme::DARK, &FormState::default(), false)
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
        // cb = the relative div's content box; its left = PAD(20), top = PAD(20).
        assert_eq!(badge.0, 20 + 30, "abs left = cb.left + left");
        assert!(badge.1 >= 20 + 8 && badge.1 <= 20 + 8 + 6, "abs top ≈ cb.top + top");
        // out of flow: the sibling <p> lands where it would with no badge at all.
        let without = lay("<body><div style=\"position:relative\"><p>flow</p></div></body>", 800);
        assert_eq!(flow_y(&l), flow_y(&without), "absolute badge does not shift the following text");
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
        let sheet = crate::css::collect(&dom, 800.0);
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
        // list text is indented past the plain content edge (PAD=20)
        assert!(texts(&l).iter().all(|(x, _, _)| *x > 20));
    }
}

/// The definite **padding-box** height of a positioned box — what `top`/`bottom`
/// percentages on its absolutely-positioned descendants resolve against
/// (CSS 2.1 §9.3.2). Only an explicit `height` counts: abspos children are laid
/// out during the parent's child walk, before its content height exists.
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

fn definite_cb_height(st: &ComputedStyle) -> Option<i32> {
    let pad_v = st.pad_top as i32 + st.pad_bottom as i32;
    match st.height {
        // `box-sizing:border-box` → the used height already spans the padding.
        Len::Px(h) if st.box_border => Some(h as i32),
        Len::Px(h) => Some(h as i32 + pad_v),
        _ => None,
    }
}
