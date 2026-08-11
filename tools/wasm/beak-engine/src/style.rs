//! style.rs — computed style + the UA default stylesheet, as data.
//!
//! docs/spec/CONFORMANCE.md's rule: be *standard-shaped* from the start. Slice-0 baked
//! per-tag pixel sizes into the layout code; this replaces that with a real
//! cascade seam:
//!
//! ```text
//!   inherited(parent) → UA sheet(tag) → inline style="…"  → ComputedStyle
//! ```
//!
//! Author `<style>`/linked CSS (selectors, specificity) slot in *between* the
//! UA sheet and inline styles later — the pipeline shape is already correct.
//! Colours resolve against the active `Theme` so pages follow light/dark like
//! the rest of the UI (until pages set their own `color`, which we honor).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::color::ColorVal;
use crate::css::{ElemInfo, PseudoElem, Stylesheet};
use crate::dom::Element;
use crate::layout::{Rgb, Theme};
use crate::forms::ControlKind;

/// CSS `display` — only the values our layout implements so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    None,
    Block,
    Inline,
    /// `display: inline-block` — a block box inside, an atomic inline box
    /// outside: it takes part in a line box like an image, but lays its own
    /// content out with the full block box model.
    InlineBlock,
    ListItem,
    /// `<table>` — establishes the (simplified) table formatting context in
    /// `layout.rs`; its `tr`/`td`/`th` descendants are laid by that walker.
    Table,
    /// `display: flex` — flex formatting context (single-line) in `layout.rs`.
    Flex,
    /// `display: grid` — grid formatting context (explicit columns + auto rows).
    Grid,
    /// `display: table-caption` — a `<caption>` box by any other name. Sized to
    /// the finished table rather than sizing it, so it must be recognised or a
    /// long caption widens the table it describes (MediaWiki's image thumbs are
    /// exactly this: `figure{display:table}` + `figcaption{display:table-caption}`).
    TableCaption,
    /// `display: table-row` — a row inside a (CSS) table. Laid by `layout_table`.
    TableRow,
    /// `display: table-row-group` — a plain row group (`<tbody>`).
    TableRowGroup,
    /// `display: table-header-group` (`<thead>`) — its rows sort before every
    /// other row group regardless of source order (CSS2.1 §17.2.1 / HTML §15).
    TableHeaderGroup,
    /// `display: table-footer-group` (`<tfoot>`) — its rows sort after every
    /// other row group regardless of source order.
    TableFooterGroup,
    /// `display: table-cell` — a cell inside a (CSS) table. Outside a table
    /// context it degrades to a block box.
    TableCell,
    /// `display: table-column`/`table-column-group` (CSS2.1 §17.2.1): these
    /// generate no box of their own (they only carry column properties, which
    /// this engine's simplified table layout doesn't apply per-column). Kept
    /// distinct from `Other`-ish content so `layout.rs` can tell a real column
    /// marker (never rendered, regardless of what tag carries the value) apart
    /// from arbitrary stray content (which anonymous-box-wraps instead).
    TableColumn,
    TableColumnGroup,
}

/// CSS `text-align` — how a block container distributes its line boxes'
/// leftover inline space. `Start`/`End` are the writing-mode-relative pair;
/// this engine is LTR-only, so `Start == Left` and `End == Right` at use time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAlign {
    Start,
    End,
    Left,
    Right,
    Center,
    /// Stretch every line but the last to fill the line box. Not implemented
    /// (our line segments merge adjacent same-style words, so there are no
    /// per-word boxes left to expand) — laid out as `Start`.
    Justify,
}

/// CSS `line-height`. A unitless number inherits AS a number (each descendant
/// resolves it against its own font-size); a length/percentage inherits as the
/// already-computed px. Keeping the two apart is what makes `body{line-height:
/// 1.5}` scale a nested heading instead of squashing it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineHeight {
    /// `normal` — use the face's own line metrics.
    Normal,
    Num(f32),
    Px(f32),
}

impl LineHeight {
    /// The used line-height in px for a box at `font_px`, or `None` for
    /// `normal` (the caller falls back to font metrics).
    pub fn px(self, font_px: f32) -> Option<f32> {
        match self {
            LineHeight::Normal => None,
            LineHeight::Num(n) => Some(n * font_px),
            LineHeight::Px(p) => Some(p),
        }
    }
}

/// CSS `text-transform` — a rendering-time case mapping of the text content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextTransform {
    None,
    Upper,
    Lower,
    Capitalize,
}

/// CSS `list-style-type` — the marker a `display:list-item` box generates.
/// Inherited, so setting it on the `<ul>`/`<ol>` reaches every `<li>`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListStyle {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    DecimalLeadingZero,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

impl ListStyle {
    /// A glyph marker (bullet) rather than a counter string.
    pub fn is_bullet(self) -> bool {
        matches!(self, ListStyle::Disc | ListStyle::Circle | ListStyle::Square)
    }
}

/// CSS `table-layout` — how a table computes its column widths.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableLayout {
    /// Column widths derived from content (the default, content-based sizing).
    Auto,
    /// CSS2 §17.5.2.1 fixed layout: widths come from the table/`<col>`/first-row
    /// cell `width`s; content does not widen columns.
    Fixed,
}

/// A `grid-template-columns` track size.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GridTrack {
    Auto,      // size to column content (max-content)
    Fixed(f32), // px
    Pct(f32),
    Fr(f32), // fraction of leftover space
}

/// Max explicit grid columns we track (content grids rarely exceed this).
pub const MAX_GRID_COLS: usize = 16;

/// Max named grid areas per container (page shells rarely exceed this).
pub const GRID_AREAS_MAX: usize = 12;

/// A `grid-template-areas` region: the area name (FNV-1a hash) and its half-open
/// cell rectangle `[c0,c1) × [r0,r1)` in the template grid.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GridArea {
    pub name: u32,
    pub r0: u8,
    pub r1: u8,
    pub c0: u8,
    pub c1: u8,
}

impl GridArea {
    pub const EMPTY: GridArea = GridArea { name: 0, r0: 0, r1: 0, c0: 0, c1: 0 };
}

/// FNV-1a hash of a grid-area name (0 = "none"). Both `grid-template-areas` and
/// `grid-area` values pass through `apply_one`'s lowercasing, so the hashes match.
pub fn area_hash(name: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    if h == 0 {
        1
    } else {
        h
    }
}

/// Hash a CSS counter name. CSS identifiers are technically case-sensitive, but
/// no page relies on two counters differing only by case, and the two parsers
/// that must agree — this one and the `content: counter(…)` side — reach the
/// name through different paths (`apply_one` already ASCII-lowercases its value,
/// `content` does not), so case-folding here keeps them consistent. Reuses the
/// grid-area FNV so `0` is never a valid hash.
pub fn counter_hash(name: &str) -> u32 {
    area_hash(&name.trim().to_ascii_lowercase())
}

/// Parse a `counter-reset`/`counter-increment` value — a list of `<name>
/// [<integer>]?` pairs — into `out`/`n`. `default` is the value when a name has
/// no explicit integer (0 for reset, 1 for increment). `none` clears the list.
fn parse_counter_ops(v: &str, out: &mut [(u32, i32); COUNTER_OPS_MAX], n: &mut u8, default: i32) {
    *n = 0;
    let t = v.trim();
    if t.is_empty() || t == "none" {
        return;
    }
    let mut it = t.split_whitespace().peekable();
    while let Some(name) = it.next() {
        // A stray keyword (`none`) among names is not a counter; skip it.
        if name == "none" {
            continue;
        }
        // An optional integer follows the name; otherwise the default applies.
        let val = match it.peek().and_then(|s| s.parse::<i32>().ok()) {
            Some(num) => {
                it.next();
                num
            }
            None => default,
        };
        if (*n as usize) < COUNTER_OPS_MAX {
            out[*n as usize] = (counter_hash(name), val);
            *n += 1;
        }
    }
}

/// `justify-content` — main-axis distribution of leftover space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Justify {
    Start,
    End,
    Center,
    Between,
    Around,
    Evenly,
}

/// `align-items` / `align-self` — cross-axis placement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrossAlign {
    Stretch,
    Start,
    Center,
    End,
}

/// `flex-basis` — an item's main-size seed before grow/shrink.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FlexBasis {
    Auto, // use the content's intrinsic main size
    Px(f32),
    Pct(f32),
}

/// One edge of a box's border: its used width (px) and colour. A side paints
/// only when `width > 0` AND `color` is set. The four sides are independent
/// (`border-top`/`-right`/… may differ).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BorderSide {
    /// The USED width — what layout and paint read. It is the specified width
    /// only while a style is in effect, and 0 otherwise.
    pub width: f32,
    /// `None` means `currentColor` — the initial value, and still unresolved.
    /// `finish_borders` turns it into the element's own `color` once the whole
    /// cascade has run, so a later `color` declaration still reaches it.
    pub color: Option<Rgb>,
    /// `border-style: hidden`. Paints exactly like `none` on its own box, but
    /// in a collapsed table it is not the same thing: `hidden` SUPPRESSES the
    /// grid line it meets, beating every other border there (CSS2.1 §17.6.2
    /// rule 1), while `none` is merely the weakest candidate.
    pub hidden: bool,
    /// The specified `border-width`, kept apart from the used one. The two
    /// halves arrive in either order and neither implies the other: a width
    /// with no style paints nothing, a style with no width is `medium`.
    pub spec_width: f32,
    /// A `border-style` other than `none`/`hidden` is in effect.
    pub styled: bool,
    /// `border-color: transparent` — a VALUE, not an absence. The side keeps
    /// its width and paints nothing, which differs from both a colour and from
    /// leaving the property unset (that means `currentColor`).
    pub see_through: bool,
    /// A width or style declaration reached this side. `border: none` is a
    /// DECLARATION, and it computes to the same used width as a side nobody
    /// touched — only this bit tells them apart. It matters where the UA
    /// supplies a frame of its own: a form control's, which the page then
    /// suppresses (`paint_control`).
    pub specified: bool,
}

/// `border-width`'s initial value, `medium`.
pub const BORDER_MEDIUM: f32 = 3.0;

impl Default for BorderSide {
    fn default() -> BorderSide {
        BorderSide { width: 0.0, color: None, hidden: false, spec_width: BORDER_MEDIUM, styled: false, see_through: false, specified: false }
    }
}

impl BorderSide {
    /// Recompute the used width after either half changed. `border-style`'s
    /// initial value is `none`, and that forces the used width to 0 whatever
    /// `border-width` says (CSS2.1 §8.5.3).
    fn sync(&mut self) {
        self.width = if self.styled { self.spec_width } else { 0.0 };
    }
    fn set_spec_width(&mut self, w: f32) {
        self.spec_width = w;
        self.specified = true;
        self.sync();
    }
    /// Apply one `border-color` token, reporting whether it was one. A page
    /// that hides a button's frame writes `border-color: transparent`; treating
    /// that as "no colour parsed" drops the declaration and leaves the frame
    /// standing — which is how Wikipedia's icon buttons came out as empty
    /// rectangles. `rgba(0,0,0,0)` says the same thing and must land here too:
    /// it is how DuckDuckGo reserves the hover frame around every result.
    fn set_color(&mut self, tok: &str, theme: &Theme) -> bool {
        match parse_color_val(tok, theme) {
            Some(ColorVal::Transparent) => {
                self.color = None;
                self.see_through = true;
            }
            Some(ColorVal::Rgb(c)) => {
                self.color = Some(c);
                self.see_through = false;
            }
            None => return false,
        }
        true
    }

    /// Apply one `border-style` token. An unknown one is invalid and leaves the
    /// side alone.
    fn set_style(&mut self, tok: &str) {
        match tok {
            "none" | "hidden" => {
                self.styled = false;
                self.hidden = tok == "hidden";
            }
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
                self.styled = true;
                self.hidden = false;
            }
            _ => return,
        }
        self.specified = true;
        self.sync();
    }
}

/// Resolve every side's `currentColor` against the element's final `color`.
/// Runs after the whole cascade, because `border-style: solid; color: green`
/// and `color: green; border-style: solid` have to mean the same thing.
fn finish_borders(s: &mut ComputedStyle) {
    let c = s.color;
    for side in [&mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left] {
        if side.color.is_none() && !side.see_through && side.width > 0.0 {
            side.color = Some(c);
        }
    }
}

/// A CSS length keyword/value for the box model. `Auto` means "auto" (for
/// width/margins) or "none" (for max-width). `%` is relative to the containing
/// block's content width, resolved at layout time.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Len {
    Auto,
    Px(f32),
    Pct(f32),
    /// `calc()` in affine form `pct% of basis + px` — calc is linear in the
    /// percentage basis, so any mix of `%`/px/em resolves to (pct, px).
    Calc { pct: f32, px: f32 },
}

impl Len {
    /// Resolve to px against a containing-block width; `Auto` → `None`.
    pub fn px(self, cb: f32) -> Option<f32> {
        match self {
            Len::Auto => None,
            Len::Px(p) => Some(p),
            Len::Pct(p) => Some(p / 100.0 * cb),
            Len::Calc { pct, px } => Some(pct / 100.0 * cb + px),
        }
    }
}

/// One axis of `background-position` / `mask-position`.
///
/// A percentage aligns the same fraction of the image with that fraction of
/// the positioning area (css-backgrounds-3 §3.6), so it cannot be resolved
/// until the image's size is known — which is at paint time, since an image
/// may arrive after layout.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BgPos {
    Px(f32),
    /// Fraction 0..1: `offset = (area - image) * f`.
    Pct(f32),
}

/// `background-size` / `mask-size`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BgSize {
    /// Intrinsic size (css-backgrounds-3 §3.9).
    Auto,
    Cover,
    Contain,
    /// Explicit per axis; `None` on an axis means `auto` (keep the aspect
    /// ratio against the other axis). Percentages are of the positioning area.
    Fixed(Option<Len>, Option<Len>),
}

/// A `background-image` or `mask-image` layer.
///
/// `image` is a `url_key` into the stylesheet's URL table, not the string:
/// `ComputedStyle` is `Copy` and must stay that way (it is copied per element
/// and memoised), so it cannot hold an allocation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BgLayer {
    pub image: Option<u64>,
    /// (repeat-x, repeat-y).
    pub repeat: (bool, bool),
    pub pos: (BgPos, BgPos),
    pub size: BgSize,
}

impl BgLayer {
    pub const NONE: BgLayer = BgLayer {
        image: None,
        repeat: (true, true),
        pos: (BgPos::Pct(0.0), BgPos::Pct(0.0)),
        size: BgSize::Auto,
    };
}

/// CSS `position`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// CSS `float`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatKind {
    None,
    Left,
    Right,
}

/// CSS `clear`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClearKind {
    None,
    Left,
    Right,
    Both,
}

/// CSS 2.1 `clip` (§11.1.2). Applies only to absolutely-positioned boxes; the
/// four offsets are resolved px from the element's border-box top-left corner
/// (`top`/`bottom` from the top edge, `left`/`right` from the left edge). `None`
/// on a side = `auto` = that border edge. `Inherit` is a transient value that
/// `resolve` collapses to the parent's computed `clip`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Clip {
    Auto,
    Inherit,
    Rect {
        top: Option<f32>,
        right: Option<f32>,
        bottom: Option<f32>,
        left: Option<f32>,
    },
}

/// CSS `z-index` (CSS2.1 §9.9.1): `auto` or an integer stack level, valid only
/// on a positioned box (`position != static`). `Inherit` is a transient value
/// that `resolve` collapses to the parent's computed `z-index`, same pattern
/// as `Clip::Inherit`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZIndex {
    Auto,
    Value(i32),
    Inherit,
}

/// One `box-shadow` layer, outer only and WITHOUT blur.
///
/// Real pages use two very different things under one name: a soft drop shadow
/// (`0 2px 8px rgba(...)`) and a **hairline rule** (`0 1px #c8ccd1`), which is a
/// zero-blur shadow standing in for a border the author did not want in the box
/// model. The second is a plain rectangle and is what shows up as a missing
/// separator; the first needs a blur kernel and looks fine while absent. So only
/// `blur == 0` is painted, and a blurred shadow keeps being skipped rather than
/// drawn as a hard slab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
    /// `None` = `currentColor`, resolved at PAINT time. Resolving it here would
    /// take whatever `color` happened to be cascaded so far — and `box-shadow`
    /// is routinely written before `color` in the same block.
    pub color: Option<Rgb>,
}

/// The subset of computed properties the renderer consumes. Split by CSS
/// inheritance: font/colour/`white-space` inherit; box/`display` do not.
#[derive(Clone, Copy)]
pub struct ComputedStyle {
    // — inherited —
    pub font_px: f32,
    /// The *parent's* font-size — the base for resolving this element's own
    /// `font-size` in `em`/`%`/`inherit` (CSS: font-size em/% is parent-
    /// relative, NOT relative to the value a UA/earlier rule already set).
    /// Recomputed per element in `inherit_reset`; never compounds.
    pub em_base: f32,
    /// The root element's computed `font-size` — the basis for `rem`.
    pub rem_base: f32,
    /// The viewport, for `vw`/`vh`/`vmin`/`vmax`. Document-global like
    /// `rem_base`, and carried the same way: seeded on the initial style and
    /// copied down by `inherit_reset`, so every `s.units()` has it without
    /// threading two more arguments through the cascade.
    pub vw: f32,
    pub vh: f32,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub pre: bool, // white-space: pre (no collapse, honor newlines)
    /// `white-space: nowrap` — whitespace still collapses, but the line never
    /// breaks at one. Inherited. Real pages use it to keep a label, a
    /// coordinate pair or a table header on one line; wrapping it anyway makes
    /// the box a line taller and, under `position: absolute`, overlaps
    /// whatever it was placed above.
    pub nowrap: bool,
    /// `visibility: hidden`/`collapse` — the box still lays out and still takes
    /// its space, but paints nothing. Inherited, so a descendant can set
    /// `visible` and reappear inside a hidden ancestor (CSS2.1 §11.2).
    pub hidden: bool,
    /// The box (and its whole subtree) is fully transparent — `opacity: 0`.
    /// Unlike `visibility` this cannot be undone further down: opacity groups
    /// the subtree, so a descendant with `opacity: 1` is still invisible. It
    /// stays HIT-TESTABLE, which is exactly what a checkbox-hack click overlay
    /// (`position:absolute; width:100%; height:100%; opacity:0`) needs.
    pub transparent: bool,
    /// This element's OWN `opacity: 0`, before it is folded into `transparent`.
    /// Kept apart so a later declaration in the same cascade can undo an
    /// earlier one, while an ANCESTOR's transparency still can't be undone.
    pub opacity_zero: bool,
    pub color: Rgb,
    pub text_align: TextAlign,
    /// `<center>` and `<div align=center>` centre BLOCK-level children too,
    /// not just inline content — the behaviour browsers spell `text-align:
    /// -moz-center`. Plain CSS `text-align: center` must NOT do this, which
    /// is why it needs its own inherited flag rather than riding on the
    /// alignment value. The `<center><table>` idiom depends on it entirely.
    pub center_blocks: bool,
    pub list_style: ListStyle,
    pub line_height: LineHeight,
    /// `direction: rtl` — the inline base direction. This engine does no bidi
    /// reordering (no RTL faces are embedded); what it does honour is the part
    /// that governs layout of LTR content inside an RTL container: `start`/
    /// `end` text alignment flip, so an unstyled RTL block right-aligns.
    pub rtl: bool,
    pub text_transform: TextTransform,
    /// `text-align-last` — alignment of a block's LAST line. `None` = `auto`,
    /// i.e. defer to `text-align`.
    pub text_align_last: Option<TextAlign>,
    /// `text-indent` — how far the block's FIRST line box starts in from its
    /// content edge. Inherited; a percentage resolves against the containing
    /// block's width. Negative values hang the first line out to the left.
    pub text_indent: Len,
    // — not inherited —
    pub display: Display,
    // — box model (block) —
    pub width: Len,
    pub min_width: Len,
    pub max_width: Len, // Auto = no maximum
    pub height: Len,
    pub min_height: Len,
    pub max_height: Len, // Auto = no maximum
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_left: Len,
    pub margin_right: Len,
    pub pad_top: f32,
    pub pad_right: f32,
    pub pad_bottom: f32,
    pub pad_left: f32,
    pub box_border: bool, // box-sizing: border-box
    /// `appearance: none` — the page opts this form control OUT of the UA
    /// widget look (css-ui-4 §4) and draws the whole thing itself.
    pub appearance_none: bool,
    pub contain_size: bool, // `contain: size`/`strict` — content contributes no size
    pub contain_intrinsic: Option<(f32, f32)>, // `contain-intrinsic-size` (w, h) px
    pub bg: Option<Rgb>, // background-color (None = transparent)
    /// `background-image` + its placement properties.
    pub bg_layer: BgLayer,
    /// `mask-image` + its placement. A mask does not paint the image: it
    /// stencils the element's own `background-color` through the image's alpha
    /// — which is how icon systems (MediaWiki's Vector, Codex) draw a
    /// recolourable icon from one SVG.
    pub mask_layer: BgLayer,
    pub border_top: BorderSide,
    pub border_right: BorderSide,
    pub border_bottom: BorderSide,
    pub border_left: BorderSide,
    // — positioning —
    pub position: Position,
    pub top: Len,
    pub right: Len,
    pub bottom: Len,
    pub left: Len,
    pub z_index: ZIndex,
    pub is_link: bool,
    pub is_rule: bool, // <hr> — painted as a divider
    pub is_break: bool, // <br> — forced line break in inline flow
    /// `vertical-align` — not inherited. On a table cell it aligns the content
    /// box in the row; on an inline-level box it shifts the box on the line.
    pub valign: VAlign,
    /// `text-decoration-line` as `DECO_*` bits. CSS propagates a decoration to
    /// in-flow descendants rather than inheriting it (css-text-decor-3 §1.2);
    /// we inherit, which paints the same pixels for every construct we have —
    /// the difference only shows where a descendant tries to *cancel* one.
    pub deco: u8,
    // — flex container —
    pub flex_row: bool, // flex-direction: row (true) vs column (false)
    pub flex_wrap: bool,
    pub flex_balance: bool, // flex-wrap: balance (css-flexbox-2 line balancing)
    pub justify: Justify,
    pub align_items: CrossAlign,
    pub gap: f32,
    // — flex item —
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: FlexBasis,
    pub align_self: Option<CrossAlign>,
    pub order: i32,
    // — grid container —
    pub grid_ncols: u8,
    pub grid_tracks: [GridTrack; MAX_GRID_COLS],
    pub grid_nrows: u8,
    pub grid_row_tracks: [GridTrack; MAX_GRID_COLS],
    pub grid_auto_rows: GridTrack,
    pub grid_col_gap: f32,
    pub grid_row_gap: f32,
    pub justify_items: CrossAlign,
    // `grid-template-areas` — named regions (container), 0-count = none.
    pub grid_areas: [GridArea; GRID_AREAS_MAX],
    pub grid_area_count: u8,
    // `repeat(auto-fill/auto-fit, …)` in the columns: 0 = none, else the stored
    // one-copy pattern spans `grid_col_fill_start .. +len` and is expanded to fill
    // the container width at layout time.
    pub grid_col_fill: u8,
    pub grid_col_fill_start: u8,
    pub grid_col_fill_len: u8,
    // — grid item —
    pub grid_col_span: u16,
    pub grid_col_start: i16, // 0 = auto placement
    pub grid_row_start: i16, // 0 = auto placement
    pub grid_row_span: u16,
    pub grid_area: u32, // `grid-area: <name>` (FNV-1a hash, 0 = none)
    pub justify_self: Option<CrossAlign>,
    // — float —
    pub float: FloatKind,
    pub clear: ClearKind,
    // — clip (abs-positioned only) —
    pub clip: Clip,
    // — table —
    pub table_layout: TableLayout,
    /// `border-spacing` (horizontal, vertical) in px — the gap the separated
    /// border model leaves between adjacent cell borders, and between the
    /// table's own padding edge and the outermost cells (CSS2.1 §17.6.1).
    /// Inherited, so it reaches cells from the table without a walk.
    pub border_spacing: (f32, f32),
    /// `border-collapse: collapse` — cell borders merge with their neighbours'
    /// and with the table's, and `border-spacing` no longer applies.
    pub border_collapse: bool,
    /// `overflow: hidden`/`clip` on BOTH axes — the box paints nothing of its
    /// content outside its padding box. `auto`/`scroll` deliberately do NOT
    /// set this: without in-page scroll containers, clipping there would hide
    /// content the user is meant to be able to reach.
    pub overflow_clip: bool,
    /// `overflow-wrap`/`word-wrap: break-word` or `word-break: break-all`/
    /// `break-word` — a word longer than its line may be split mid-word rather
    /// than overflowing the box. Inherited, like both source properties.
    pub break_word: bool,
    /// `border-radius`, `[tl, tr, br, bl]` (CSS corner order). Circular — CSS
    /// allows an ellipse per corner (`r1 / r2`), we keep the horizontal radius.
    /// Percentages resolve against the border-box width at paint time.
    pub radius: [Len; 4],
    /// `box-shadow`, first layer, outer, zero-blur only (see `BoxShadow`).
    pub shadow: Option<BoxShadow>,
    /// `transform: translate(...)` as a paint-time offset, in px and in
    /// PERCENT of the box's own size (`Len::Pct`) — the `translate(-50%,-50%)`
    /// centring idiom needs the latter. Only translation: rotation and scale
    /// would need a transformed raster path, and every other transform value
    /// leaves this `None` rather than being approximated.
    pub translate: Option<(Len, Len)>,
    /// `caption-side: bottom` — the caption renders below the table grid
    /// instead of above it. Inherited (CSS2.1 §17.4.1), so it can be set on
    /// either the `<table>` or the `<caption>`.
    pub caption_bottom: bool,
    /// `empty-cells: hide` — a cell with no in-flow content paints neither
    /// border nor background in the separated model (CSS2.1 §17.6.1.1).
    pub empty_cells_hide: bool,
    /// `<table border>` / `<table cellpadding>`: HTML presentational hints that
    /// style the table's CELLS, not the table (HTML §15.3.8). The cells are
    /// several levels down (`tr`, row groups), so they ride down as inherited
    /// state instead of needing an ancestor-attribute lookup. `None` = the
    /// attribute is absent.
    pub attr_cell_border: Option<f32>,
    pub attr_cell_padding: Option<f32>,
    // — CSS counters (css-lists-3 §4) — `(name_hash, value)` pairs. Names are
    // case-folded FNV-1a hashes (like `grid_area`) so `ComputedStyle` stays
    // `Copy` — no `Vec`. Not inherited. `_n` is how many of the slots are used.
    pub counter_reset: [(u32, i32); COUNTER_OPS_MAX],
    pub counter_reset_n: u8,
    pub counter_increment: [(u32, i32); COUNTER_OPS_MAX],
    pub counter_increment_n: u8,
}

/// Max named counters one `counter-reset`/`counter-increment` declaration can
/// carry. Real pages list one or two; extras are dropped (keeps the array
/// `Copy`, no heap).
pub const COUNTER_OPS_MAX: usize = 4;

impl ComputedStyle {
    /// Total horizontal border (left + right) contribution to the box.
    pub fn border_x(&self) -> f32 {
        self.border_left.width + self.border_right.width
    }
    /// Total vertical border (top + bottom) contribution to the box.
    pub fn border_y(&self) -> f32 {
        self.border_top.width + self.border_bottom.width
    }

    /// The initial style for the document root, seeded from the theme.
    pub fn root(theme: &Theme) -> ComputedStyle {
        ComputedStyle {
            font_px: BASE_FONT_PX,
            em_base: BASE_FONT_PX,
            rem_base: BASE_FONT_PX,
            // Overwritten by `layout()` with the real viewport. The default is
            // the reftest canvas, so a bare `ComputedStyle::root()` in a unit
            // test still resolves `vw`/`vh` to something meaningful.
            vw: 800.0,
            vh: 600.0,
            deco: 0,
            caption_bottom: false,
            break_word: false,
            overflow_clip: false,
            radius: [Len::Px(0.0); 4],
            shadow: None,
            translate: None,
            bold: false,
            italic: false,
            mono: false,
            nowrap: false,
            pre: false,
            color: theme.text,
            hidden: false,
            transparent: false,
            opacity_zero: false,
            text_align: TextAlign::Start,
            center_blocks: false,
            list_style: ListStyle::Disc,
            line_height: LineHeight::Normal,
            rtl: false,
            text_transform: TextTransform::None,
            text_align_last: None,
            text_indent: Len::Px(0.0),
            display: Display::Block,
            width: Len::Auto,
            min_width: Len::Auto,
            max_width: Len::Auto,
            height: Len::Auto,
            min_height: Len::Auto,
            max_height: Len::Auto,
            margin_top: 0.0,
            margin_bottom: 0.0,
            margin_left: Len::Px(0.0),
            margin_right: Len::Px(0.0),
            pad_top: 0.0,
            pad_right: 0.0,
            pad_bottom: 0.0,
            pad_left: 0.0,
            box_border: false,
            appearance_none: false,
            contain_size: false,
            contain_intrinsic: None,
            bg: None,
            bg_layer: BgLayer::NONE,
            mask_layer: BgLayer::NONE,
            border_top: BorderSide::default(),
            border_right: BorderSide::default(),
            border_bottom: BorderSide::default(),
            border_left: BorderSide::default(),
            position: Position::Static,
            top: Len::Auto,
            right: Len::Auto,
            bottom: Len::Auto,
            left: Len::Auto,
            z_index: ZIndex::Auto,
            is_link: false,
            is_rule: false,
            is_break: false,
            valign: VAlign::Baseline,
            flex_row: true,
            flex_wrap: false,
            flex_balance: false,
            justify: Justify::Start,
            align_items: CrossAlign::Stretch,
            gap: 0.0,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: FlexBasis::Auto,
            align_self: None,
            order: 0,
            grid_ncols: 0,
            grid_tracks: [GridTrack::Auto; MAX_GRID_COLS],
            grid_nrows: 0,
            grid_row_tracks: [GridTrack::Auto; MAX_GRID_COLS],
            grid_auto_rows: GridTrack::Auto,
            grid_col_gap: 0.0,
            grid_row_gap: 0.0,
            justify_items: CrossAlign::Stretch,
            grid_areas: [GridArea::EMPTY; GRID_AREAS_MAX],
            grid_area_count: 0,
            grid_col_fill: 0,
            grid_col_fill_start: 0,
            grid_col_fill_len: 0,
            grid_col_span: 1,
            grid_col_start: 0,
            grid_row_start: 0,
            grid_row_span: 1,
            grid_area: 0,
            justify_self: None,
            float: FloatKind::None,
            clear: ClearKind::None,
            clip: Clip::Auto,
            table_layout: TableLayout::Auto,
            border_spacing: (0.0, 0.0),
            border_collapse: false,
            empty_cells_hide: false,
            attr_cell_border: None,
            attr_cell_padding: None,
            counter_reset: [(0, 0); COUNTER_OPS_MAX],
            counter_reset_n: 0,
            counter_increment: [(0, 0); COUNTER_OPS_MAX],
            counter_increment_n: 0,
        }
    }
}

pub const BASE_FONT_PX: f32 = 16.0;

/// `border-radius`: 1-4 lengths in the usual corner shorthand order, with an
/// optional `/ <1-4 lengths>` vertical set that we drop (we draw circular
/// corners). All-or-nothing: one unparseable component leaves the property
/// alone rather than applying a half-read shape.
fn parse_radius_shorthand(v: &str, u: Units) -> Option<[Len; 4]> {
    let horiz = v.split('/').next()?;
    let mut it = horiz.split_whitespace();
    let a = parse_len_opt(it.next()?, u)?;
    let nth = |it: &mut core::str::SplitWhitespace| match it.next() {
        None => Some(None),
        Some(t) => parse_len_opt(t, u).map(Some),
    };
    let b = nth(&mut it)?;
    let c = nth(&mut it)?;
    let d = nth(&mut it)?;
    if it.next().is_some() {
        return None;
    }
    Some(match (b, c, d) {
        (None, _, _) => [a; 4],
        (Some(b), None, _) => [a, b, a, b],
        (Some(b), Some(c), None) => [a, b, c, b],
        (Some(b), Some(c), Some(d)) => [a, b, c, d],
    })
}

/// `vertical-align`. Lengths and percentages are not represented — they fall
/// back to `Baseline` rather than being mis-placed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VAlign {
    Baseline,
    Sub,
    Super,
    Top,
    Middle,
    Bottom,
    TextTop,
    TextBottom,
}

fn parse_valign(v: &str) -> Option<VAlign> {
    Some(match v {
        "baseline" => VAlign::Baseline,
        "sub" => VAlign::Sub,
        "super" => VAlign::Super,
        "top" => VAlign::Top,
        "middle" => VAlign::Middle,
        "bottom" => VAlign::Bottom,
        "text-top" => VAlign::TextTop,
        "text-bottom" => VAlign::TextBottom,
        _ => return None,
    })
}

/// `ComputedStyle::deco` bits (`text-decoration-line`).
pub const DECO_UNDERLINE: u8 = 1;
pub const DECO_LINE_THROUGH: u8 = 2;
pub const DECO_OVERLINE: u8 = 4;

/// `text-decoration` / `text-decoration-line`: keep the line keywords, ignore
/// the colour and style components of the shorthand (we draw a solid line in
/// the text's own colour).
fn parse_deco(v: &str) -> u8 {
    let mut d = 0;
    for kw in v.split_whitespace() {
        match kw {
            "underline" => d |= DECO_UNDERLINE,
            "line-through" => d |= DECO_LINE_THROUGH,
            "overline" => d |= DECO_OVERLINE,
            _ => {}
        }
    }
    d
}

/// The bases a length may need that are not the containing block: `em` (the
/// element's own font-size, or its inherited one while `font-size` itself is
/// being resolved), `rem` (the ROOT element's computed font-size) and the
/// viewport for `vw`/`vh`/`vmin`/`vmax`. `em` and `rem` differ the moment a
/// document sets `html { font-size: … }` — the `62.5%` "1rem = 10px" idiom is
/// everywhere, and treating `rem` as `em` scales such a page by 1.6x.
#[derive(Clone, Copy, Debug)]
pub struct Units {
    pub em: f32,
    pub rem: f32,
    /// Viewport width in px — the basis for `vw`, and half of `vmin`/`vmax`.
    pub vw: f32,
    /// Viewport height in px — the basis for `vh`, and the other half.
    pub vh: f32,
}

/// The starting point for any freshly-resolved style: the inherited slice
/// copied from `parent`, non-inherited properties reset to their CSS initial
/// value. Shared by `resolve()` (a real element) and `resolve_pseudo()` (a
/// `::before`/`::after` generated box, which inherits from its originating
/// element the same way a child would).
fn inherit_reset(parent: &ComputedStyle) -> ComputedStyle {
    ComputedStyle {
        font_px: parent.font_px,
        em_base: parent.font_px,
        // `rem` is root-relative: inherited untouched, never reset per element.
        rem_base: parent.rem_base,
        // Document-global, same as `rem_base`.
        vw: parent.vw,
        vh: parent.vh,
        bold: parent.bold,
        italic: parent.italic,
        mono: parent.mono,
        pre: parent.pre,
        nowrap: parent.nowrap,
        hidden: parent.hidden,
        transparent: parent.transparent,
        opacity_zero: false,
        color: parent.color,
        text_align: parent.text_align,
        center_blocks: parent.center_blocks,
        list_style: parent.list_style,
        line_height: parent.line_height,
        rtl: parent.rtl,
        text_transform: parent.text_transform,
        text_align_last: parent.text_align_last,
        text_indent: parent.text_indent,
        deco: parent.deco,
        caption_bottom: parent.caption_bottom,
        break_word: parent.break_word,
        overflow_clip: false,
        radius: [Len::Px(0.0); 4],
        shadow: None,
        translate: None,
        display: Display::Inline, // CSS initial `display` is inline
        width: Len::Auto,
        min_width: Len::Auto,
        max_width: Len::Auto,
        height: Len::Auto,
        min_height: Len::Auto,
        max_height: Len::Auto,
        margin_top: 0.0,
        margin_bottom: 0.0,
        margin_left: Len::Px(0.0),
        margin_right: Len::Px(0.0),
        pad_top: 0.0,
        pad_right: 0.0,
        pad_bottom: 0.0,
        pad_left: 0.0,
        box_border: false,
        appearance_none: false,
        contain_size: false,
        contain_intrinsic: None,
        bg: None,
        bg_layer: BgLayer::NONE,
        mask_layer: BgLayer::NONE,
        border_top: BorderSide::default(),
        border_right: BorderSide::default(),
        border_bottom: BorderSide::default(),
        border_left: BorderSide::default(),
        position: Position::Static,
        top: Len::Auto,
        right: Len::Auto,
        bottom: Len::Auto,
        left: Len::Auto,
        z_index: ZIndex::Auto,
        is_link: false,
        is_rule: false,
        is_break: false,
        valign: VAlign::Baseline,
        flex_row: true,
        flex_wrap: false,
        flex_balance: false,
        justify: Justify::Start,
        align_items: CrossAlign::Stretch,
        gap: 0.0,
        flex_grow: 0.0,
        flex_shrink: 1.0,
        flex_basis: FlexBasis::Auto,
        align_self: None,
        order: 0,
        grid_ncols: 0,
        grid_tracks: [GridTrack::Auto; MAX_GRID_COLS],
        grid_nrows: 0,
        grid_row_tracks: [GridTrack::Auto; MAX_GRID_COLS],
        grid_auto_rows: GridTrack::Auto,
        grid_col_gap: 0.0,
        grid_row_gap: 0.0,
        justify_items: CrossAlign::Stretch,
        grid_areas: [GridArea::EMPTY; GRID_AREAS_MAX],
        grid_area_count: 0,
        grid_col_fill: 0,
        grid_col_fill_start: 0,
        grid_col_fill_len: 0,
        grid_col_span: 1,
        grid_col_start: 0,
        grid_row_start: 0,
        grid_row_span: 1,
        grid_area: 0,
        justify_self: None,
        float: FloatKind::None,
        clear: ClearKind::None,
        clip: Clip::Auto,
        table_layout: TableLayout::Auto,
        border_spacing: parent.border_spacing,
        border_collapse: parent.border_collapse,
        empty_cells_hide: parent.empty_cells_hide,
        attr_cell_border: parent.attr_cell_border,
        attr_cell_padding: parent.attr_cell_padding,
        // Counters are not inherited: reset to empty on every element.
        counter_reset: [(0, 0); COUNTER_OPS_MAX],
        counter_reset_n: 0,
        counter_increment: [(0, 0); COUNTER_OPS_MAX],
        counter_increment_n: 0,
    }
}

/// The style for an anonymous box (CSS2.1 §17.2.1): inherited properties
/// (color/font/…) come from `parent` exactly as for a real child, every
/// non-inherited property is the CSS initial value (`inherit_reset`), and
/// `display` is set to whatever box the layout algorithm needs to generate
/// (`Table`/`TableRow`/`TableCell`/…) — an anonymous box has no source
/// element, so nothing else can set it.
pub fn anon_inherit(parent: &ComputedStyle, display: Display) -> ComputedStyle {
    let mut s = inherit_reset(parent);
    s.display = display;
    s
}

/// Resolve an element's computed style by the cascade: inherit from `parent`,
/// apply the UA rule for its tag, then matching author `<style>` rules (by
/// specificity + order), then any inline `style="…"` (highest). `ancestors` is
/// the root→…→parent chain, for descendant/child selector matching.
pub fn resolve(
    el: &Element,
    parent: &ComputedStyle,
    theme: &Theme,
    sheet: &Stylesheet,
    ancestors: &[ElemInfo],
    prev_siblings: &[ElemInfo],
    sib_count: u32,
    viewport_w: f32,
) -> ComputedStyle {
    let mut s = inherit_reset(parent);
    ua_rule(&el.tag, parent, theme, &mut s);
    // `:any-link { text-decoration: underline }` (HTML rendering §15.3.9). It
    // needs the `href`, which `ua_rule` doesn't see — a bare `<a name=…>`
    // anchor is not a link and is not underlined.
    if el.tag == "a" && el.attr("href").is_some() {
        s.deco |= DECO_UNDERLINE;
    }
    // A button-like control is `box-sizing: border-box` in the UA sheet (HTML
    // rendering §15.5.1) — unlike a text field, which stays content-box. It is
    // what pages build on: Google puts a `height:30px` button inside a
    // `height:30px` bordered wrapper and expects it to fit exactly. Read as
    // content-box the button came out 8px taller than its own frame and hung
    // out the bottom. `ua_rule` can't do this — it only sees the tag, and
    // `<input>` is a button or a text field depending on its `type`.
    if matches!(
        crate::forms::kind_of(el),
        Some(ControlKind::Submit | ControlKind::Reset | ControlKind::Button
            | ControlKind::File | ControlKind::Select)
    ) {
        s.box_border = true;
    }

    // HTML's `dir` attribute is a presentational hint for `direction`: it sits
    // between the UA sheet and the author cascade, so author CSS still wins.
    match el.attr("dir") {
        Some("rtl") => s.rtl = true,
        Some("ltr") => s.rtl = false,
        _ => {}
    }
    // `<table>`'s presentational attributes (HTML §15.3.8). `cellspacing` is
    // the one the reftest corpus leans on: it writes `cellspacing="0"`, and
    // without this the UA's 2px default silently applies where the page asked
    // for none. `border`/`cellpadding` style the cells, so they ride down as
    // inherited state (see `attr_cell_border`).
    if el.tag == "table" {
        if let Some(n) = el.attr("cellspacing").and_then(|v| parse_length(v.trim(), s.units())) {
            s.border_spacing = (n.max(0.0), n.max(0.0));
        }
        if let Some(n) = el.attr("cellpadding").and_then(|v| parse_length(v.trim(), s.units())) {
            s.attr_cell_padding = Some(n.max(0.0));
        }
        if let Some(n) = el.attr("border").and_then(|v| parse_length(v.trim(), s.units())) {
            s.attr_cell_border = Some(n.max(0.0));
            if n > 0.0 {
                for side in [&mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left] {
                    side.set_style("solid");
                    side.set_spec_width(n);
                    side.color = Some(theme.rule);
                }
            }
        }
    }

    // `bgcolor` is a presentational hint for `background-color` (HTML §15.3.3),
    // the same family as `<table border>`/`cellpadding` above, and it sits
    // between the UA sheet and the author cascade so author CSS still wins.
    // Old table-built pages carry their whole colour scheme in it — Hacker
    // News' orange masthead is a `bgcolor` on a `<td>`.
    if let Some(c) = el.attr("bgcolor").and_then(|v| parse_color(v.trim(), theme)) {
        s.bg = Some(c);
    }

    // `width`/`height` as presentational hints (HTML Rendering §15.3.5-6).
    // Table-built pages do their centring with spacer cells — Google's home
    // page is `<td width="25%">&nbsp;</td>` either side of the search box —
    // and an ignored attribute collapses the spacer to nothing, which slams
    // the content against the left edge. The value is a "dimension": a bare
    // number is pixels, a trailing `%` a percentage.
    //
    // Images are NOT in this list: `img_box` already reads their attributes,
    // and setting the property here too would apply the hint twice.
    if matches!(el.tag.as_str(), "table" | "td" | "th" | "col" | "colgroup" | "hr") {
        if let Some(l) = el.attr("width").and_then(parse_dimension_attr) {
            s.width = l;
        }
        if let Some(l) = el.attr("height").and_then(parse_dimension_attr) {
            s.height = l;
        }
    }

    // `align` is a presentational hint for `text-align` (HTML Rendering
    // §15.3.3). It is inherited, so a cell's `align="center"` centres
    // everything inside it — which is the other half of how table-built pages
    // centre: `<td width="25%">` spacers place the cell, `align="center"`
    // places the content INSIDE it. With only the first, Google's search box
    // sat at the left edge of a correctly-centred cell.
    //
    // `<table align>` is deliberately absent: there it means float/auto
    // margins, not text alignment, and treating it as this would centre a
    // table's text instead of the table.
    if matches!(
        el.tag.as_str(),
        "td" | "th" | "tr" | "thead" | "tbody" | "tfoot" | "col" | "colgroup"
            | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
    ) {
        match el.attr("align").map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("center") => {
                s.text_align = TextAlign::Center;
                // `<div align=center>` is the other spelling of `<center>` and
                // gets the same block-centring; a cell's `align` does not.
                s.center_blocks = el.tag == "div";
            }
            Some(v) if v.eq_ignore_ascii_case("left") => s.text_align = TextAlign::Left,
            Some(v) if v.eq_ignore_ascii_case("right") => s.text_align = TextAlign::Right,
            Some(v) if v.eq_ignore_ascii_case("justify") => s.text_align = TextAlign::Justify,
            _ => {}
        }
    }

    // Author cascade WITH `!important` (CSS Cascade 4 §6.3): two passes. Normal
    // declarations first (UA < author-normal < inline-normal), then `!important`
    // on top (author-important < inline-important) — so an `!important` decl
    // wins its property regardless of specificity/order.
    let inline = el.attr("style");
    if !sheet.is_empty() {
        let info = ElemInfo::of(el);
        let mut matched = sheet.matched(&info, ancestors, prev_siblings, sib_count, crate::css::Media::new(viewport_w, theme.is_dark()));
        matched.sort_by_key(|(spec, order, _)| (*spec, *order));
        // Pass 1 — normal <style> declarations, low→high specificity.
        for (_, _, decls) in &matched {
            for (p, v) in *decls {
                let (val, imp) = split_important(v);
                if !imp {
                    apply_one(p, val, theme, &mut s);
                }
            }
        }
        if let Some(decls) = inline {
            apply_declarations_pass(decls, theme, &mut s, false);
        }
        // Pass 2 — `!important` <style> declarations, low→high specificity.
        for (_, _, decls) in &matched {
            for (p, v) in *decls {
                let (val, imp) = split_important(v);
                if imp {
                    apply_one(p, val, theme, &mut s);
                }
            }
        }
        if let Some(decls) = inline {
            apply_declarations_pass(decls, theme, &mut s, true);
        }
    } else if let Some(decls) = inline {
        apply_declarations_pass(decls, theme, &mut s, false);
        apply_declarations_pass(decls, theme, &mut s, true);
    }
    // `clip: inherit` takes the parent's computed value (clip is not inherited
    // by default, so this is resolved here rather than in the initial slice).
    if matches!(s.clip, Clip::Inherit) {
        s.clip = parent.clip;
    }
    // `z-index: inherit`, same pattern (z-index is not inherited by default).
    if matches!(s.z_index, ZIndex::Inherit) {
        s.z_index = parent.z_index;
    }
    // `overflow` on the root element — and on `<body>` while the root keeps
    // `visible` — propagates to the VIEWPORT, and the element's own used value
    // becomes `visible` (css-overflow-3 §3.3). So the box itself neither clips
    // nor establishes a formatting context.
    if el.tag == "html" || el.tag == "body" {
        s.overflow_clip = false;
    }
    // A float or an out-of-flow box is blockified (css-display-3 §2.7): it
    // never joins a line box, so `inline`/`inline-block` there is just a block.
    // `inline` matters for generated content — a page underlines its active tab
    // with `a::after { position: absolute; … }` and states no display at all,
    // relying on exactly this rule to give it a box.
    if matches!(s.display, Display::Inline | Display::InlineBlock)
        && (s.float != FloatKind::None || matches!(s.position, Position::Absolute | Position::Fixed))
    {
        s.display = Display::Block;
    }
    // `vertical-align` applies to inline-level boxes and table cells only
    // (CSS2.1 §10.8.1). An out-of-flow or block-level box is never aligned in
    // a line box, and leaving the value on it would ride down into the text
    // runs the box creates and shift its whole content — which is exactly what
    // `vertical-align-sub-001` catches (two absolutely positioned spans that
    // must coincide).
    // A `<td>`/`<th>` carries `display: block` from the UA sheet — the table
    // machinery recognises cells by tag/role, not by display — so the tag has
    // to be part of the test.
    let is_cell = matches!(s.display, Display::TableCell) || el.tag == "td" || el.tag == "th";
    if !is_cell
        && (matches!(s.position, Position::Absolute | Position::Fixed)
            || s.float != FloatKind::None
            || !matches!(s.display, Display::Inline | Display::InlineBlock))
    {
        s.valign = VAlign::Baseline;
    }
    // Opacity groups the subtree: a transparent ancestor wins over anything
    // this element declares, but within this element the cascade decides.
    s.transparent |= s.opacity_zero;
    finish_borders(&mut s);
    s
}

/// Resolve `el`'s `::before`/`::after` generated box: the winning `content`
/// declaration (by the same specificity/order cascade as any other property)
/// plus the pseudo-element's own computed style. Returns `None` when there is
/// no matching rule, `content` is `none`/`normal`/unparseable (`attr()`,
/// `counter()`, `open-quote`, `url()`, … are out of scope — docs/spec/CONFORMANCE.md's
/// forward-compatible rule: produce nothing rather than mis-render), or the
/// pseudo box itself computes to `display: none`.
///
/// `own` is `el`'s OWN already-resolved computed style — the pseudo box
/// inherits from it exactly as a real child element would.
#[allow(clippy::too_many_arguments)]
pub fn resolve_pseudo(
    el: &Element,
    own: &ComputedStyle,
    theme: &Theme,
    sheet: &Stylesheet,
    ancestors: &[ElemInfo],
    prev_siblings: &[ElemInfo],
    sib_count: u32,
    viewport_w: f32,
    pseudo: PseudoElem,
) -> Option<(Vec<ContentPiece>, ComputedStyle)> {
    if sheet.is_empty() {
        return None;
    }
    let info = ElemInfo::of(el);
    let mut matched = sheet.matched_pseudo(&info, ancestors, prev_siblings, sib_count, crate::css::Media::new(viewport_w, theme.is_dark()), pseudo);
    if matched.is_empty() {
        return None;
    }
    matched.sort_by_key(|(spec, order, _)| (*spec, *order));
    // The `content` declarations in cascade order (later overrides earlier). An
    // INVALID one is dropped at parse time (CSS Syntax 3 §4), so the winner is
    // the LAST one that parses — not simply the last one. The template may
    // reference counters (`counter()`/`counters()`), resolved later against the
    // layout-time counter stack; a plain string is a single `Text` piece.
    let mut content_vals: Vec<&str> = Vec::new();
    for (_, _, decls) in &matched {
        for (p, v) in *decls {
            if p == "content" {
                content_vals.push(v.as_str());
            }
        }
    }
    let template = content_vals.iter().rev().find_map(|v| parse_content_template(v))?;
    let mut s = inherit_reset(own);
    for pass_imp in [false, true] {
        for (_, _, decls) in &matched {
            for (p, v) in *decls {
                if p == "content" {
                    continue;
                }
                let (val, imp) = split_important(v);
                if imp == pass_imp {
                    apply_one(p, val, theme, &mut s);
                }
            }
        }
    }
    // Layout only knows how to place the pseudo box as an anonymous INLINE
    // text run (see `layout.rs`'s `pseudo()`); `display: none` produces no
    // box. Any other display (`block`, `list-item`, …) is a box shape we
    // don't lay out here — docs/spec/CONFORMANCE.md's forward-compatible rule: produce
    // nothing rather than render it wrong. An explicit `width`/`height` is
    // the same story a level down: generated content is emitted as a plain
    // text run, so a sized spacer (the common `content: "…"; display:
    // inline-block; width: N%` idiom some reftest references use as an
    // indent trick) would flow as unsized text instead of reserving that
    // width — visibly wrong, so skip it too.
    // Same blockification as a real element (css-display-3 §2.7) — a generated
    // box that is floated or out of flow never joins a line box. This is what
    // gives `a::after { position: absolute }` a box when the page states no
    // display at all.
    if matches!(s.display, Display::Inline | Display::InlineBlock)
        && (s.float != FloatKind::None || matches!(s.position, Position::Absolute | Position::Fixed))
    {
        s.display = Display::Block;
    }
    s.transparent |= s.opacity_zero;
    finish_borders(&mut s);
    Some((template, s))
}

impl ComputedStyle {
    /// Does this generated element produce a BOX we lay out as a rectangle —
    /// the CSS-icon idiom, `content: ""` plus a size plus a `background-image`?
    ///
    /// Deliberately a closed list. `display: none` produces nothing, and the
    /// table-internal roles have no content box of their own, so generated
    /// content in them renders nothing at all — `before-content-display-012`
    /// puts `content: "FAIL"` on a `display: table-column-group` and asserts
    /// that nothing appears. Anything not listed here and not `inline` keeps
    /// the old forward-compatible answer: produce nothing rather than guess.
    pub fn is_generated_box(&self) -> bool {
        matches!(
            self.display,
            Display::Block | Display::InlineBlock | Display::ListItem | Display::Flex | Display::Grid | Display::Table
        )
    }
}

/// One component of a resolved `content` value on a `::before`/`::after` box.
/// A plain string is a single `Text`; `counter()`/`counters()` stay symbolic
/// because their value depends on layout-time counter state, resolved in
/// `layout.rs`.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentPiece {
    Text(String),
    /// `counter(name, style)` — the innermost in-scope value of `name`.
    Counter { name: u32, style: ListStyle },
    /// `counters(name, sep, style)` — every in-scope value of `name`, joined
    /// by `sep` (outermost first).
    Counters { name: u32, sep: String, style: ListStyle },
    /// `attr(name)` — the originating element's attribute as a string, or the
    /// empty string when it has no such attribute (CSS2.1 §12.2). Lowercased,
    /// which is how the HTML parser stores attribute names.
    Attr(String),
}

/// Parse a CSS `content` value into its component pieces: concatenated
/// `<string>` tokens (`"a" 'b'`), `counter()`/`counters()` and `attr()`, in
/// order. Any OTHER component (`open-quote`/`close-quote`, `url()`, an unknown
/// identifier) is out of scope: rather than mis-render, the WHOLE value
/// produces no content — the caller then generates nothing, per
/// docs/spec/CONFORMANCE.md's forward-compatible rule. `none`/`normal` also produce
/// nothing (no box).
pub fn parse_content_template(v: &str) -> Option<Vec<ContentPiece>> {
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("normal") {
        return None;
    }
    let mut pieces: Vec<ContentPiece> = Vec::new();
    let mut chars = v.chars().peekable();
    loop {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        let Some(&c0) = chars.peek() else { break };
        if c0 == '"' || c0 == '\'' {
            pieces.push(ContentPiece::Text(parse_string_token(&mut chars)?));
            continue;
        }
        // An identifier or function token: read up to whitespace or '('.
        let mut ident = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '(' {
                break;
            }
            ident.push(c);
            chars.next();
        }
        let lname = ident.to_ascii_lowercase();
        if matches!(chars.peek(), Some('(')) && (lname == "counter" || lname == "counters") {
            chars.next(); // consume '('
            let inside = read_until_close(&mut chars)?;
            let args = split_top_commas(&inside);
            let name = args.first().map(|s| s.trim()).filter(|s| !s.is_empty())?;
            let name_hash = counter_hash(name);
            // A wrong argument count or an unrecognised `<counter-style>` makes
            // the WHOLE `content` value invalid — it is dropped so an earlier
            // valid declaration wins (CSS Syntax 3 §4), and an unimplemented but
            // syntactically-valid style falls into the same "produce nothing"
            // bucket per docs/spec/CONFORMANCE.md's forward-compatible rule.
            if lname == "counter" {
                // `counter(name)` | `counter(name, <style>)`
                if args.len() > 2 {
                    return None;
                }
                let style = match args.get(1) {
                    None => ListStyle::Decimal,
                    Some(s) => parse_list_style(s.trim())?,
                };
                pieces.push(ContentPiece::Counter { name: name_hash, style });
            } else {
                // `counters(name, <sep>)` | `counters(name, <sep>, <style>)`
                if !(2..=3).contains(&args.len()) {
                    return None;
                }
                let sep = unquote_string(args.get(1)?.trim())?;
                let style = match args.get(2) {
                    None => ListStyle::Decimal,
                    Some(s) => parse_list_style(s.trim())?,
                };
                pieces.push(ContentPiece::Counters { name: name_hash, sep, style });
            }
            continue;
        }
        if matches!(chars.peek(), Some('(')) && lname == "attr" {
            chars.next(); // consume '('
            let inside = read_until_close(&mut chars)?;
            // CSS2.1 `attr(X)` takes exactly one argument — an attribute NAME,
            // not a string. The type/fallback arguments are css-values-5 and
            // would change what the value means, so they invalidate it here
            // rather than being ignored.
            let args = split_top_commas(&inside);
            let [name] = args.as_slice() else { return None };
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return None;
            }
            pieces.push(ContentPiece::Attr(name.to_ascii_lowercase()));
            continue;
        }
        // `open-quote`, `url(...)`, or an unknown identifier — unsupported, so
        // the whole value contributes nothing.
        return None;
    }
    if pieces.is_empty() { None } else { Some(pieces) }
}

/// Parse ONE CSS `<string>` token, with `chars` positioned on the opening
/// quote. Consumes through the closing quote. Handles css-syntax-3 §4.3.7
/// escapes: `\` + 1-6 hex digits (+ one optional trailing whitespace) is a code
/// point (`\A` = U+000A, the "forced line break" idiom); `\` + an actual
/// newline is a line continuation (no output); `\` + any other char is that
/// literal char. Returns `None` on an unterminated string.
fn parse_string_token(chars: &mut core::iter::Peekable<core::str::Chars>) -> Option<String> {
    let q = chars.next()?; // opening quote
    let mut out = String::new();
    loop {
        match chars.next() {
            None => return None, // unterminated → invalid value
            Some(c) if c == q => break,
            Some('\\') => match chars.peek().copied() {
                None => {}
                Some('\n') => {
                    chars.next(); // escaped newline: line continuation, no output
                }
                Some(c) if c.is_ascii_hexdigit() => {
                    let mut hex = String::new();
                    while hex.len() < 6 {
                        match chars.peek() {
                            Some(h) if h.is_ascii_hexdigit() => {
                                hex.push(*h);
                                chars.next();
                            }
                            _ => break,
                        }
                    }
                    if matches!(chars.peek(), Some(w) if w.is_whitespace()) {
                        chars.next(); // one trailing whitespace terminates the escape
                    }
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(c) => {
                    out.push(c);
                    chars.next();
                }
            },
            Some(c) => out.push(c),
        }
    }
    Some(out)
}

/// Read the raw text inside a `(` … `)` (the `(` already consumed), balancing
/// nested parens and skipping over quoted strings so a `,`/`)` inside a string
/// doesn't terminate. Returns `None` if unterminated.
fn read_until_close(chars: &mut core::iter::Peekable<core::str::Chars>) -> Option<String> {
    let mut depth = 1;
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                depth += 1;
                out.push(c);
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(out);
                }
                out.push(c);
            }
            '"' | '\'' => {
                out.push(c);
                while let Some(q) = chars.next() {
                    out.push(q);
                    if q == c {
                        break;
                    }
                    if q == '\\' {
                        if let Some(e) = chars.next() {
                            out.push(e);
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    None
}

/// Split a function-argument list on top-level commas (commas inside quotes or
/// nested parens don't split). Each arg is returned trimmed.
fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut chs = s.chars();
    while let Some(c) = chs.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                } else if c == '\\' {
                    if let Some(e) = chs.next() {
                        cur.push(e);
                    }
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth -= 1;
                    cur.push(c);
                }
                ',' if depth == 0 => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(c),
            },
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// Unwrap a single quoted `<string>` argument (the `counters()` separator) to
/// its literal text. `None` if it isn't a proper quoted string.
fn unquote_string(s: &str) -> Option<String> {
    let mut chars = s.trim().chars().peekable();
    match chars.peek() {
        Some('"') | Some('\'') => parse_string_token(&mut chars),
        _ => None,
    }
}

/// The UA default stylesheet (HTML rendering §15), expressed as code over the
/// computed style. `em` sizes are relative to the *parent* font (per CSS), so
/// nested headings/lists scale naturally. Kept close to a browser's defaults.
fn ua_rule(tag: &str, parent: &ComputedStyle, theme: &Theme, s: &mut ComputedStyle) {
    let em = parent.font_px;
    match tag {
        // Non-rendered subtrees.
        "head" | "title" | "meta" | "link" | "script" | "style" | "noscript" | "template" => {
            s.display = Display::None;
        }

        // `<body>` is the block container that insets the page, and the inset
        // is its UA MARGIN (HTML's rendering section says 8px) — not a fixed
        // page padding. A reftest that writes `body { margin: 0 }` means it,
        // and so does a page that lays itself edge to edge.
        "body" => {
            s.display = Display::Block;
            s.margin_top = 8.0;
            s.margin_bottom = 8.0;
            s.margin_left = Len::Px(8.0);
            s.margin_right = Len::Px(8.0);
        }

        // Block containers.
        "html" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav"
        | "aside" | "figure" | "figcaption" | "form" | "address" | "details" | "summary"
        | "tbody" | "thead" | "tfoot" | "tr" | "fieldset" => {
            s.display = Display::Block;
        }
        // `<center>` is `display: block; text-align: center` (HTML rendering
        // §15.3.2). Left as the initial `inline` it swallows whatever it wraps
        // into a line box — and a `<table>` inside it collapses to running text.
        // Hacker News wraps its ENTIRE page in one, so the whole site rendered
        // as a single paragraph.
        "center" => {
            s.display = Display::Block;
            s.text_align = TextAlign::Center;
            s.center_blocks = true;
        }

        // Tables. `<table>` gets the table formatting context; cells are block
        // containers for their own content (`th` also bold). `tr`/`tbody`/… are
        // walked by `layout_table`, so their display is only a fallback.
        // No default margin: the HTML UA sheet gives `<table>` none (only
        // `border-spacing`), and inventing one shifts everything after a table
        // by half an em relative to what every reftest reference assumes.
        "table" => {
            s.display = Display::Table;
            // The HTML UA sheet's `border-spacing: 2px` (HTML §15.3.8). It is
            // inherited, so every cell sees it without walking back up.
            s.border_spacing = (2.0, 2.0);
            s.border_collapse = false;
        }
        "td" | "th" => {
            s.display = Display::Block;
            // `padding: 1px` from the UA sheet, overridden by `<table
            // cellpadding>` when that attribute is present.
            let p = s.attr_cell_padding.unwrap_or(1.0);
            s.pad_top = p;
            s.pad_right = p;
            s.pad_bottom = p;
            s.pad_left = p;
            // `<table border>` gives every cell a 1px inset border.
            if s.attr_cell_border.is_some_and(|b| b > 0.0) {
                for side in [&mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left] {
                    side.set_style("solid");
                    side.set_spec_width(1.0);
                    side.color = Some(theme.rule);
                }
            }
            if tag == "th" {
                s.bold = true;
                s.text_align = TextAlign::Center;
            }
        }
        "caption" => {
            s.display = Display::Block;
            s.bold = true;
            s.margin_bottom = em * 0.3;
            s.text_align = TextAlign::Center;
        }
        "p" => {
            s.display = Display::Block;
            s.margin_top = em;
            s.margin_bottom = em;
        }

        // Headings: font-size in em of the parent, bold, heading colour.
        "h1" => heading(s, theme, em, 1.9, 0.60),
        "h2" => heading(s, theme, em, 1.5, 0.70),
        "h3" => heading(s, theme, em, 1.25, 0.80),
        "h4" => heading(s, theme, em, 1.05, 0.90),
        "h5" => heading(s, theme, em, 0.95, 1.00),
        "h6" => heading(s, theme, em, 0.85, 1.10),

        // Lists.
        "ul" | "ol" => {
            s.display = Display::Block;
            s.pad_left = 26.0;
            s.margin_top = em * 0.5;
            s.margin_bottom = em * 0.5;
            s.list_style = if tag == "ol" { ListStyle::Decimal } else { ListStyle::Disc };
        }
        "li" => s.display = Display::ListItem,
        "dl" => {
            s.display = Display::Block;
            s.margin_top = em * 0.5;
            s.margin_bottom = em * 0.5;
        }
        "dt" => s.display = Display::Block,
        "dd" => {
            s.display = Display::Block;
            s.pad_left = 26.0;
        }

        "blockquote" => {
            s.display = Display::Block;
            s.pad_left = 24.0;
            s.margin_top = em * 0.6;
            s.margin_bottom = em * 0.6;
            s.color = theme.muted;
        }
        "pre" => {
            s.display = Display::Block;
            s.mono = true;
            s.pre = true;
            s.margin_top = em * 0.6;
            s.margin_bottom = em * 0.6;
        }

        "hr" => {
            s.display = Display::Block;
            s.is_rule = true;
            s.margin_top = em * 0.6;
            s.margin_bottom = em * 0.6;
        }

        // Inline styling.
        "a" => {
            s.is_link = true;
            s.color = theme.link;
        }
        "b" | "strong" => s.bold = true,
        "u" | "ins" => s.deco |= DECO_UNDERLINE,
        "s" | "del" | "strike" => s.deco |= DECO_LINE_THROUGH,
        "i" | "em" | "cite" | "var" | "dfn" => s.italic = true,
        "code" | "kbd" | "samp" | "tt" => s.mono = true,
        "small" => s.font_px = em * 0.85,
        "big" => s.font_px = em * 1.15,
        "mark" => s.color = theme.link,
        "br" => s.is_break = true,
        // Superscript / subscript: smaller, raised/lowered off the baseline.
        "sup" => {
            s.font_px = em * 0.75;
            s.valign = VAlign::Super;
        }
        "sub" => {
            s.font_px = em * 0.75;
            s.valign = VAlign::Sub;
        }
        // span / label / abbr / time / u / s / … → plain inline.
        _ => {}
    }
}

fn heading(s: &mut ComputedStyle, theme: &Theme, em: f32, scale: f32, margin_em: f32) {
    s.display = Display::Block;
    s.font_px = em * scale;
    s.bold = true;
    s.color = theme.heading;
    s.margin_top = s.font_px * margin_em;
    s.margin_bottom = s.font_px * margin_em * 0.7;
}

/// Parse and apply a `style="a: b; c: d"` declaration list. This is real CSS
/// declaration syntax (css-syntax-3), just without selectors — the same parser
/// a `<style>` rule body will use. Unknown properties are ignored (forward
/// compatible, like a browser).
/// Split a declaration value into (value, is_important). `!important` is a
/// trailing flag (css-syntax-3): optional whitespace, then `!important`
/// (case-insensitive).
fn split_important(v: &str) -> (&str, bool) {
    let t = v.trim_end();
    let n = t.len();
    if n >= 10 && t.is_char_boundary(n - 10) && t[n - 10..].eq_ignore_ascii_case("!important") {
        (t[..n - 10].trim_end(), true)
    } else {
        (v, false)
    }
}

/// Apply the `style="…"` declarations whose importance matches `important`, so
/// callers run the two cascade passes. css-syntax-3 syntax, unknown props skipped.
fn apply_declarations_pass(decls: &str, theme: &Theme, s: &mut ComputedStyle, important: bool) {
    for decl in crate::css::split_decls(decls) {
        let mut it = decl.splitn(2, ':');
        let prop = match it.next() {
            Some(p) => p.trim().to_ascii_lowercase(),
            None => continue,
        };
        let raw = match it.next() {
            Some(v) => v.trim(),
            None => continue,
        };
        let (val, imp) = split_important(raw);
        if prop.is_empty() || val.is_empty() || imp != important {
            continue;
        }
        apply_one(&prop, val, theme, s);
    }
}

/// Apply a single `prop: val` declaration. Shared by inline styles now and by
/// author `<style>` rules later.
impl ComputedStyle {
    /// The `em`/`rem` bases for parsing this element's declarations.
    pub fn units(&self) -> Units {
        Units { em: self.font_px, rem: self.rem_base, vw: self.vw, vh: self.vh }
    }
}

pub fn apply_one(prop: &str, val: &str, theme: &Theme, s: &mut ComputedStyle) {
    let v = val.trim().to_ascii_lowercase();
    // Font-relative bases for this element, taken once: `apply_one` handles a
    // single declaration, so `font-size` (which uses its own inherited base)
    // is the only property that could move them, and it does so for the NEXT
    // call — matching the cascade's declaration order.
    let u = s.units();
    match prop {
        "display" => {
            s.display = match v.as_str() {
                "none" => Display::None,
                "list-item" => Display::ListItem,
                "inline" => Display::Inline,
                "inline-block" => Display::InlineBlock,
                "table" | "inline-table" => Display::Table,
                "table-caption" => Display::TableCaption,
                "table-row" => Display::TableRow,
                "table-row-group" => Display::TableRowGroup,
                "table-header-group" => Display::TableHeaderGroup,
                "table-footer-group" => Display::TableFooterGroup,
                "table-cell" => Display::TableCell,
                "table-column" => Display::TableColumn,
                "table-column-group" => Display::TableColumnGroup,
                "flex" | "inline-flex" => Display::Flex,
                "grid" | "inline-grid" | "grid-lanes" | "inline-grid-lanes" => Display::Grid,
                _ => Display::Block,
            };
        }
        "table-layout" => {
            s.table_layout = if v == "fixed" { TableLayout::Fixed } else { TableLayout::Auto };
        }
        "border-collapse" => s.border_collapse = v == "collapse",
        "empty-cells" => s.empty_cells_hide = v == "hide",
        "vertical-align" => {
            if let Some(a) = parse_valign(&v) {
                s.valign = a;
            }
        }
        // `word-wrap` is the legacy alias of `overflow-wrap`; `word-break:
        // break-word` is a deprecated spelling with the same effect. All three
        // land on one flag — we break at a character, not by script rules, so
        // `break-all` is not distinguished from `break-word`.
        // Two values are `x y`; a single one applies to both. Only a box that
        // clips on BOTH axes is clipped here (see `overflow_clip`).
        "overflow" => {
            let mut it = v.split_whitespace();
            let x = it.next().unwrap_or("");
            let y = it.next().unwrap_or(x);
            let clips = |k: &str| k == "hidden" || k == "clip";
            s.overflow_clip = clips(x) && clips(y);
        }
        "overflow-wrap" | "word-wrap" => s.break_word = v == "break-word" || v == "anywhere",
        "word-break" => s.break_word = v == "break-all" || v == "break-word",
        "border-radius" => {
            if let Some(rs) = parse_radius_shorthand(&v, u) {
                s.radius = rs;
            }
        }
        "transform" => {
            s.translate = parse_translate(&v, u);
        }
        "box-shadow" => {
            let t = v.trim();
            if t.eq_ignore_ascii_case("none") {
                s.shadow = None;
            } else if let Some(sh) = parse_box_shadow(first_layer(t), u) {
                s.shadow = Some(sh);
            }
        }
        "border-top-left-radius" | "border-top-right-radius" | "border-bottom-right-radius"
        | "border-bottom-left-radius" => {
            // One corner takes `h v`; we keep the horizontal radius.
            if let Some(n) = parse_len_opt(v.split_whitespace().next().unwrap_or(""), u) {
                let i = match prop {
                    "border-top-left-radius" => 0,
                    "border-top-right-radius" => 1,
                    "border-bottom-right-radius" => 2,
                    _ => 3,
                };
                s.radius[i] = n;
            }
        }
        "caption-side" => match v.as_str() {
            "bottom" => s.caption_bottom = true,
            "top" => s.caption_bottom = false,
            _ => {}
        },
        // One length applies to both axes; two give horizontal then vertical.
        "border-spacing" => {
            let mut it = v.split_whitespace().filter_map(|p| parse_length(p, u));
            if let Some(h) = it.next() {
                let vert = it.next().unwrap_or(h);
                s.border_spacing = (h.max(0.0), vert.max(0.0));
            }
        }
        "color" => {
            if let Some(c) = parse_color(&v, theme) {
                s.color = c;
            }
        }
        "font-weight" => {
            s.bold = matches!(v.as_str(), "bold" | "bolder" | "600" | "700" | "800" | "900");
        }
        "font-style" => {
            s.italic = matches!(v.as_str(), "italic" | "oblique");
        }
        "font-size" => {
            // em/%/inherit/relative keywords resolve against the PARENT font
            // (em_base), NOT the running value — so nothing compounds and a
            // later cascade winner (incl. `inherit`) is exact, not multiplied.
            let base = s.em_base;
            let px = match v.as_str() {
                "inherit" | "unset" => Some(base),
                "xx-small" => Some(BASE_FONT_PX * 0.5625),
                "x-small" => Some(BASE_FONT_PX * 0.625),
                "small" => Some(BASE_FONT_PX * 0.8125),
                "medium" => Some(BASE_FONT_PX),
                "large" => Some(BASE_FONT_PX * 1.125),
                "x-large" => Some(BASE_FONT_PX * 1.5),
                "xx-large" => Some(BASE_FONT_PX * 2.0),
                "larger" => Some(base * 1.2),
                "smaller" => Some(base / 1.2),
                _ => parse_length(&v, Units { em: base, ..s.units() }),
            };
            if let Some(px) = px {
                s.font_px = px.clamp(6.0, 200.0);
            }
        }
        // `line-height: normal | <number> | <length> | <percentage>`. A bare
        // number stays a number (inherits as a ratio); everything else computes
        // to px against THIS element's font-size, per CSS 2.1 §10.8.1.
        "line-height" => {
            let t = v.trim();
            s.line_height = if t == "normal" {
                LineHeight::Normal
            } else if let Some(p) = t.strip_suffix('%') {
                match p.trim().parse::<f32>() {
                    Ok(n) => LineHeight::Px(n / 100.0 * s.font_px),
                    Err(_) => s.line_height,
                }
            } else if let Ok(n) = t.parse::<f32>() {
                LineHeight::Num(n)
            } else {
                match parse_len_opt(t, u) {
                    Some(Len::Px(px)) => LineHeight::Px(px),
                    _ => s.line_height,
                }
            };
        }
        "font" => apply_font_shorthand(&v, theme, s),
        "text-decoration" | "text-decoration-line" => {
            // The shorthand resets the line to `none` when it names no line
            // keyword, so colour/style-only values legitimately clear it.
            if v != "inherit" && v != "unset" {
                s.deco = parse_deco(&v);
            }
        }
        "text-transform" => {
            s.text_transform = match v.as_str() {
                "uppercase" => TextTransform::Upper,
                "lowercase" => TextTransform::Lower,
                "capitalize" => TextTransform::Capitalize,
                "none" => TextTransform::None,
                _ => s.text_transform,
            };
        }
        // Applies to the last line of a block (and so to a block with only
        // one). `auto` defers to `text-align`.
        "text-align-last" => {
            s.text_align_last = match v.as_str() {
                "left" => Some(TextAlign::Left),
                "right" => Some(TextAlign::Right),
                "center" => Some(TextAlign::Center),
                "justify" => Some(TextAlign::Justify),
                "start" => Some(TextAlign::Start),
                "end" => Some(TextAlign::End),
                "auto" => None,
                _ => s.text_align_last,
            };
        }
        "text-indent" => {
            if let Some(l) = parse_len_opt(&v, u) {
                s.text_indent = l;
            }
        }
        "direction" => match v.as_str() {
            "rtl" => s.rtl = true,
            "ltr" => s.rtl = false,
            _ => {}
        },
        "text-align" => {
            s.text_align = match v.as_str() {
                "left" => TextAlign::Left,
                "right" => TextAlign::Right,
                "center" => TextAlign::Center,
                "justify" => TextAlign::Justify,
                "end" => TextAlign::End,
                "start" => TextAlign::Start,
                // `match-parent` on a LTR root computes to `left`; `inherit`/
                // `unset` are already the inherited value we started from.
                "match-parent" | "inherit" | "unset" => s.text_align,
                _ => s.text_align,
            };
        }
        "list-style-type" => {
            if let Some(ls) = parse_list_style(&v) {
                s.list_style = ls;
            }
        }
        // `list-style: <type> || <position> || <image>` in any order; we only
        // consume the type keyword. `none` legitimately means "no marker".
        "list-style" => {
            for part in v.split_whitespace() {
                if let Some(ls) = parse_list_style(part) {
                    s.list_style = ls;
                    break;
                }
            }
        }
        // CSS counters (css-lists-3 §4). `content: counter(…)` reads these at
        // layout time; the values themselves are resolved in `layout.rs`, which
        // maintains the scoped counter stack.
        "counter-reset" => parse_counter_ops(&v, &mut s.counter_reset, &mut s.counter_reset_n, 0),
        "counter-increment" => {
            parse_counter_ops(&v, &mut s.counter_increment, &mut s.counter_increment_n, 1)
        }
        "white-space" => match v.as_str() {
            "pre" => {
                s.pre = true;
                s.nowrap = false;
            }
            "pre-wrap" | "pre-line" => {
                s.pre = true;
                s.nowrap = false;
            }
            "normal" => {
                s.pre = false;
                s.nowrap = false;
            }
            "nowrap" => {
                s.pre = false;
                s.nowrap = true;
            }
            // `inherit`/`unset`/garbage: an invalid or non-recomputable value
            // drops (CSS Syntax 3 §4), keeping whatever the cascade already
            // set — for `inherit` specifically, that's already the parent's
            // value, since `pre` is copied from `parent` before this runs.
            _ => {}
        },
        // `collapse` differs from `hidden` only on table rows/columns (where it
        // removes the track); everywhere else the spec says treat it as
        // `hidden`, and we have no row-removal to do.
        // Only fully-transparent is modelled: anything in between needs real
        // alpha compositing in the rasteriser. Never cleared — see the field.
        "opacity" => {
            if let Ok(o) = v.trim().parse::<f32>() {
                s.opacity_zero = o <= 0.001;
            }
        }
        "visibility" => match v.as_str() {
            "hidden" | "collapse" => s.hidden = true,
            "visible" => s.hidden = false,
            _ => {}
        },
        "font-family" => {
            s.mono = v.contains("mono") || v.contains("courier") || v.contains("consol");
        }
        // — box model —
        "width" => set_size(&mut s.width, &v, u),
        "min-width" => set_size(&mut s.min_width, &v, u),
        "max-width" => set_max(&mut s.max_width, &v, u),
        "height" => set_size(&mut s.height, &v, u),
        "min-height" => set_size(&mut s.min_height, &v, u),
        "max-height" => set_max(&mut s.max_height, &v, u),
        "box-sizing" => s.box_border = v == "border-box",
        // css-ui-4 §4. Only `none` concerns us: it says "do not draw the UA
        // widget", and the page then supplies the whole look. Every other
        // value (`auto`, `button`, `textfield`, …) keeps our chrome. The
        // prefixed spellings still carry the real web's styled controls.
        "appearance" | "-webkit-appearance" | "-moz-appearance" => s.appearance_none = v == "none",
        "contain" => s.contain_size = v.split_whitespace().any(|k| k == "size" || k == "strict"),
        "contain-intrinsic-size" => {
            // definite length(s): one → both axes, two → (width, height).
            let mut it = v.split_whitespace().filter_map(|t| parse_length(t, u));
            if let Some(a) = it.next() {
                s.contain_intrinsic = Some((a, it.next().unwrap_or(a)));
            }
        }
        "margin" => {
            let u = s.units();
            let (t, r, b, l) = four_values(&v);
            s.margin_top = margin_tb(t, u);
            s.margin_right = margin_lr(r, u);
            s.margin_bottom = margin_tb(b, u);
            s.margin_left = margin_lr(l, u);
        }
        "margin-top" | "margin-block-start" => s.margin_top = margin_tb(&v, u),
        "margin-bottom" | "margin-block-end" => s.margin_bottom = margin_tb(&v, u),
        "margin-left" | "margin-inline-start" => s.margin_left = margin_lr(&v, u),
        "margin-right" | "margin-inline-end" => s.margin_right = margin_lr(&v, u),
        // Logical two-value shorthands, LTR/horizontal-tb: `margin-inline` is
        // (left, right), `margin-block` is (top, bottom); one value sets both.
        "margin-inline" => {
            let p = split_sides(&v);
            s.margin_left = margin_lr(p[0], u);
            s.margin_right = margin_lr(p[1], u);
        }
        "margin-block" => {
            let p = split_sides(&v);
            s.margin_top = margin_tb(p[0], u);
            s.margin_bottom = margin_tb(p[1], u);
        }
        "padding" => {
            let u = s.units();
            let (t, r, b, l) = four_values(&v);
            s.pad_top = parse_pad(t, u, 0.0);
            s.pad_right = parse_pad(r, u, 0.0);
            s.pad_bottom = parse_pad(b, u, 0.0);
            s.pad_left = parse_pad(l, u, 0.0);
        }
        "padding-top" | "padding-block-start" => s.pad_top = parse_pad(&v, u, s.pad_top),
        "padding-right" | "padding-inline-end" => s.pad_right = parse_pad(&v, u, s.pad_right),
        "padding-bottom" | "padding-block-end" => s.pad_bottom = parse_pad(&v, u, s.pad_bottom),
        "padding-left" | "padding-inline-start" => s.pad_left = parse_pad(&v, u, s.pad_left),
        "padding-inline" => {
            let p = split_sides(&v);
            s.pad_left = parse_pad(p[0], u, s.pad_left);
            s.pad_right = parse_pad(p[1], u, s.pad_right);
        }
        "padding-block" => {
            let p = split_sides(&v);
            s.pad_top = parse_pad(p[0], u, s.pad_top);
            s.pad_bottom = parse_pad(p[1], u, s.pad_bottom);
        }

        // — background + border —
        // `background-color` is a single property; `background` is a shorthand
        // that resets every longhand it covers — including the image — and is
        // applied as a unit or not at all.
        "background-color" => {
            let vt = v.trim();
            if vt == "none" {
                s.bg = None;
            } else if let Some(cv) = parse_color_val(vt, theme) {
                // Handles space-separated function colours like
                // `rgb(0% 50% 0%)` / `hsl(120 100% 25%)`.
                s.bg = match cv {
                    ColorVal::Rgb(c) => Some(c),
                    ColorVal::Transparent => None,
                };
            }
        }
        "background" => {
            let vt = v.trim();
            if let Some(cv) = parse_color_val(vt, theme) {
                // The whole value is one colour — the overwhelmingly common
                // case, and the only one where a function colour's internal
                // spaces must not be read as separate tokens.
                s.bg = match cv {
                    ColorVal::Rgb(c) => Some(c),
                    ColorVal::Transparent => None,
                };
                s.bg_layer = BgLayer::NONE;
            } else if let Some((color, layer)) = parse_bg_shorthand(val, &v, u, theme) {
                s.bg = color;
                s.bg_layer = layer;
            }
        }
        "background-image" => s.bg_layer.image = parse_bg_image(val),
        "background-repeat" => {
            if let Some(r) = parse_bg_repeat(&v) {
                s.bg_layer.repeat = r;
            }
        }
        "background-position" => {
            if let Some(p) = parse_bg_pos(&v, u) {
                s.bg_layer.pos = p;
            }
        }
        "background-size" => {
            if let Some(sz) = parse_bg_size(&v, u) {
                s.bg_layer.size = sz;
            }
        }
        // `mask` is still shipped prefixed by the icon systems that use it, and
        // the two spellings are the same property to us.
        "mask" | "-webkit-mask" => {
            if let Some((_, layer)) = parse_bg_shorthand(val, &v, u, theme) {
                s.mask_layer = layer;
            }
        }
        "mask-image" | "-webkit-mask-image" => s.mask_layer.image = parse_bg_image(val),
        "mask-repeat" | "-webkit-mask-repeat" => {
            if let Some(r) = parse_bg_repeat(&v) {
                s.mask_layer.repeat = r;
            }
        }
        "mask-position" | "-webkit-mask-position" => {
            if let Some(p) = parse_bg_pos(&v, u) {
                s.mask_layer.pos = p;
            }
        }
        "mask-size" | "-webkit-mask-size" => {
            if let Some(sz) = parse_bg_size(&v, u) {
                s.mask_layer.size = sz;
            }
        }
        "border" => {
            let side = parse_border_shorthand(&v, u, theme);
            s.border_top = side;
            s.border_right = side;
            s.border_bottom = side;
            s.border_left = side;
        }
        "border-top" => s.border_top = parse_border_shorthand(&v, u, theme),
        "border-right" => s.border_right = parse_border_shorthand(&v, u, theme),
        "border-bottom" => s.border_bottom = parse_border_shorthand(&v, u, theme),
        "border-left" => s.border_left = parse_border_shorthand(&v, u, theme),
        "border-width" => {
            let u = s.units();
            if let Some(t4) = four_sides(&css_tokens(&v)) {
                for (side, tok) in [
                    &mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left,
                ].into_iter().zip(t4) {
                    if let Some(w) = border_width_kw(tok, u) {
                        side.set_spec_width(w);
                    }
                }
            }
        }
        "border-color" => {
            if let Some(t4) = four_sides(&css_tokens(&v)) {
                for (side, tok) in [
                    &mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left,
                ].into_iter().zip(t4) {
                    side.set_color(tok, theme);
                }
            }
        }
        "border-style" => {
            if let Some(t4) = four_sides(&css_tokens(&v)) {
                for (side, tok) in [
                    &mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left,
                ].into_iter().zip(t4) {
                    side.set_style(tok);
                }
            }
        }
        "border-top-width" => set_side_width(&mut s.border_top, &v, u),
        "border-right-width" => set_side_width(&mut s.border_right, &v, u),
        "border-bottom-width" => set_side_width(&mut s.border_bottom, &v, u),
        "border-left-width" => set_side_width(&mut s.border_left, &v, u),
        "border-top-color" => { s.border_top.set_color(v.trim(), theme); }
        "border-right-color" => { s.border_right.set_color(v.trim(), theme); }
        "border-bottom-color" => { s.border_bottom.set_color(v.trim(), theme); }
        "border-left-color" => { s.border_left.set_color(v.trim(), theme); }
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
            let side = match prop {
                "border-top-style" => &mut s.border_top,
                "border-right-style" => &mut s.border_right,
                "border-bottom-style" => &mut s.border_bottom,
                _ => &mut s.border_left,
            };
            side.set_style(&v);
        }

        // — positioning —
        "position" => {
            s.position = match v.as_str() {
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                "fixed" => Position::Fixed,
                "sticky" => Position::Sticky,
                _ => Position::Static,
            };
        }
        "float" => {
            s.float = match v.as_str() {
                "left" => FloatKind::Left,
                "right" => FloatKind::Right,
                _ => FloatKind::None,
            };
        }
        "clear" => {
            s.clear = match v.as_str() {
                "left" => ClearKind::Left,
                "right" => ClearKind::Right,
                "both" => ClearKind::Both,
                _ => ClearKind::None,
            };
        }
        "clip" => {
            let vt = v.trim();
            if vt == "auto" {
                s.clip = Clip::Auto;
            } else if vt == "inherit" {
                s.clip = Clip::Inherit;
            } else if let Some(inner) = vt
                .strip_prefix("rect(")
                .and_then(|x| x.strip_suffix(')'))
            {
                // Four <length>|auto components, comma- or space-separated.
                let norm = inner.replace(',', " ");
                let parts: alloc::vec::Vec<&str> = norm.split_whitespace().collect();
                let comp = |t: &str| -> Option<Option<f32>> {
                    if t == "auto" {
                        Some(None)
                    } else {
                        parse_length(t, u).map(Some)
                    }
                };
                if parts.len() == 4 {
                    if let (Some(top), Some(right), Some(bottom), Some(left)) =
                        (comp(parts[0]), comp(parts[1]), comp(parts[2]), comp(parts[3]))
                    {
                        s.clip = Clip::Rect { top, right, bottom, left };
                    }
                }
            }
        }
        "top" => s.top = parse_len(&v, u),
        "right" => s.right = parse_len(&v, u),
        "bottom" => s.bottom = parse_len(&v, u),
        "left" => s.left = parse_len(&v, u),
        "z-index" => {
            s.z_index = match v.as_str() {
                "auto" => ZIndex::Auto,
                "inherit" => ZIndex::Inherit,
                other => match parse_saturating_i32(other) {
                    Some(n) => ZIndex::Value(n),
                    // Invalid <integer> → declaration ignored (keeps whatever
                    // the cascade already had, per CSS error handling).
                    None => s.z_index,
                },
            };
        }

        // — flex —
        "flex-direction" => s.flex_row = !v.starts_with("column"),
        "flex-wrap" => {
            s.flex_balance = v == "balance";
            s.flex_wrap = v.starts_with("wrap") || s.flex_balance;
        }
        "flex-flow" => {
            for tok in v.split_whitespace() {
                match tok {
                    "row" | "row-reverse" => s.flex_row = true,
                    "column" | "column-reverse" => s.flex_row = false,
                    "wrap" | "wrap-reverse" => s.flex_wrap = true,
                    "balance" => {
                        s.flex_wrap = true;
                        s.flex_balance = true;
                    }
                    "nowrap" => s.flex_wrap = false,
                    _ => {}
                }
            }
        }
        "justify-content" => {
            s.justify = match v.as_str() {
                "flex-end" | "end" | "right" => Justify::End,
                "center" => Justify::Center,
                "space-between" => Justify::Between,
                "space-around" => Justify::Around,
                "space-evenly" => Justify::Evenly,
                _ => Justify::Start,
            };
        }
        "align-items" => s.align_items = parse_cross(&v).unwrap_or(CrossAlign::Stretch),
        "align-self" => s.align_self = parse_cross(&v),
        // `gap` shorthand is `<row-gap> <column-gap>`; the longhands set one axis.
        "gap" | "grid-gap" => {
            let mut it = v.split_whitespace();
            let row = it.next().and_then(|t| parse_length(t, u));
            let col = it.next().and_then(|t| parse_length(t, u)).or(row);
            if let Some(r) = row {
                s.grid_row_gap = r;
                s.gap = r;
            }
            if let Some(c) = col {
                s.grid_col_gap = c;
            }
        }
        "column-gap" => {
            if let Some(g) = parse_length(v.trim(), u) {
                s.grid_col_gap = g;
                s.gap = g;
            }
        }
        "row-gap" => {
            if let Some(g) = parse_length(v.trim(), u) {
                s.grid_row_gap = g;
                s.gap = g;
            }
        }
        "flex-grow" => {
            if let Ok(f) = v.parse::<f32>() {
                s.flex_grow = f;
            }
        }
        "flex-shrink" => {
            if let Ok(f) = v.parse::<f32>() {
                s.flex_shrink = f;
            }
        }
        "flex-basis" => s.flex_basis = parse_basis(&v, u),
        "order" => {
            if let Ok(o) = v.parse::<i32>() {
                s.order = o;
            }
        }
        "flex" => apply_flex_shorthand(&v, s),

        // — grid —
        "grid-template-columns" => {
            let t = parse_grid_tracks(&v, s.units());
            s.grid_ncols = t.n;
            s.grid_tracks = t.tracks;
            s.grid_col_fill = t.fill;
            s.grid_col_fill_start = t.fill_start;
            s.grid_col_fill_len = t.fill_len;
        }
        "grid-template-rows" => {
            let t = parse_grid_tracks(&v, s.units());
            s.grid_nrows = t.n;
            s.grid_row_tracks = t.tracks;
        }
        "grid-auto-rows" => s.grid_auto_rows = parse_track(v.trim(), s.units()),
        "grid-template-areas" => set_grid_areas(s, &v),
        // `grid` / `grid-template` shorthand: `<rows> / <cols>` (areas/flow forms
        // are not supported — they fall through to the row/column split).
        "grid" | "grid-template" => {
            // The shorthand may carry `grid-template-areas` strings.
            if v.contains('"') || v.contains('\'') {
                set_grid_areas(s, &v);
            }
            if let Some((rows, cols)) = split_slash(&v) {
                let r = parse_grid_tracks(rows.trim(), s.units());
                s.grid_nrows = r.n;
                s.grid_row_tracks = r.tracks;
                let c = parse_grid_tracks(cols.trim(), s.units());
                s.grid_ncols = c.n;
                s.grid_tracks = c.tracks;
                s.grid_col_fill = c.fill;
                s.grid_col_fill_start = c.fill_start;
                s.grid_col_fill_len = c.fill_len;
            }
        }
        "grid-column" => {
            let (start, span) = parse_line_placement(&v);
            s.grid_col_start = start;
            s.grid_col_span = span;
        }
        "grid-row" => {
            let (start, span) = parse_line_placement(&v);
            s.grid_row_start = start;
            s.grid_row_span = span;
        }
        "grid-column-start" => s.grid_col_start = parse_line(v.trim()).unwrap_or(0),
        "grid-row-start" => s.grid_row_start = parse_line(v.trim()).unwrap_or(0),
        "grid-area" => {
            // `grid-area: <name>` (custom ident) → named placement. The numeric
            // `row / col / …` form is left to grid-row/grid-column longhands.
            let name = v.trim();
            if !name.is_empty()
                && !name.contains('/')
                && name.parse::<i32>().is_err()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                s.grid_area = area_hash(name);
            }
        }
        "justify-items" => s.justify_items = parse_cross(&v).unwrap_or(CrossAlign::Stretch),
        "justify-self" => s.justify_self = parse_cross(&v),
        "place-items" => {
            let mut it = v.split_whitespace();
            let a = it.next().unwrap_or("");
            let j = it.next().unwrap_or(a);
            s.align_items = parse_cross(a).unwrap_or(CrossAlign::Stretch);
            s.justify_items = parse_cross(j).unwrap_or(CrossAlign::Stretch);
        }
        "place-self" => {
            let mut it = v.split_whitespace();
            let a = it.next().unwrap_or("");
            let j = it.next().unwrap_or(a);
            s.align_self = parse_cross(a);
            s.justify_self = parse_cross(j);
        }

        _ => {}
    }
}

/// A CSS `<integer>`, saturating to the 32-bit signed range instead of
/// rejecting out-of-range literals as invalid (CSS Values & Units — Range
/// Checking: values outside the supported range are clamped, not dropped).
/// Accepts an optional leading `+`/`-` and ASCII digits only; `None` for
/// anything else (empty, non-digit, signs-only).
fn parse_saturating_i32(v: &str) -> Option<i32> {
    let t = v.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut acc: i64 = 0;
    for b in digits.bytes() {
        acc = acc.saturating_mul(10).saturating_add((b - b'0') as i64);
    }
    let signed = if neg { -acc } else { acc };
    Some(signed.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// A `<length>`/`auto`/`%` value for the box model.
/// Fallible length parse. `None` = the value is invalid, so the caller must
/// KEEP the previously-cascaded value — an invalid declaration is dropped, it
/// does not reset the property to its default (CSS Syntax 3 §4). `auto` is a
/// valid keyword and returns `Some(Len::Auto)`.
fn parse_len_opt(v: &str, u: Units) -> Option<Len> {
    let v = v.trim();
    if v == "auto" {
        return Some(Len::Auto);
    }
    if is_math_fn(v) {
        return parse_calc_affine(v, u);
    }
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().ok().map(Len::Pct);
    }
    parse_length(v, u).map(Len::Px)
}

/// Does this value start with a CSS math function? `values.rs` evaluates all
/// four; this is only the gate that sends them there. `min`/`max`/`clamp` were
/// missing from it, so `width: max(20px, 10px)` fell through to a plain length
/// parse, failed, and became `auto` — while the same expression inside a custom
/// property resolved fine, because `vars.rs` calls the resolver directly.
fn is_math_fn(v: &str) -> bool {
    ["calc(", "min(", "max(", "clamp("]
        .iter()
        .any(|f| v.len() >= f.len() && v[..f.len()].eq_ignore_ascii_case(f))
}

fn parse_len(v: &str, u: Units) -> Len {
    parse_len_opt(v, u).unwrap_or(Len::Auto)
}

// ── background-image / mask-image ───────────────────────────────────────────

/// The first layer of a comma-separated `<bg-layer>` list. Splitting has to be
/// paren-aware: a `data:` URI is full of commas.
fn first_layer(v: &str) -> &str {
    let b = v.as_bytes();
    let (mut depth, mut quote) = (0i32, 0u8);
    for i in 0..b.len() {
        match b[i] {
            q @ (b'"' | b'\'') if quote == 0 => quote = q,
            q if q == quote => quote = 0,
            b'(' if quote == 0 => depth += 1,
            b')' if quote == 0 => depth = (depth - 1).max(0),
            b',' if quote == 0 && depth == 0 => return v[..i].trim(),
            _ => {}
        }
    }
    v.trim()
}

/// `background-image`/`mask-image` → a URL key. Values we cannot paint
/// (gradients, `none`, `element()`) leave the layer imageless.
fn parse_bg_image(val: &str) -> Option<u64> {
    crate::css::url_value(first_layer(val)).map(|u| crate::css::url_key(&u))
}

fn parse_bg_repeat(v: &str) -> Option<(bool, bool)> {
    let t: Vec<&str> = css_tokens(v);
    let one = |s: &str| match s {
        "no-repeat" => Some(false),
        "repeat" | "round" | "space" => Some(true),
        _ => None,
    };
    match t.as_slice() {
        ["repeat-x"] => Some((true, false)),
        ["repeat-y"] => Some((false, true)),
        [a] => one(a).map(|r| (r, r)),
        [a, b] => Some((one(a)?, one(b)?)),
        _ => None,
    }
}

/// One `background-position` component: a keyword, a length or a percentage.
/// `Some((axis, pos))` where `axis` is `Some(false)` for horizontal-only
/// keywords, `Some(true)` for vertical-only, `None` when it fits either.
fn parse_pos_component(v: &str, u: Units) -> Option<(Option<bool>, BgPos)> {
    match v {
        "left" => Some((Some(false), BgPos::Pct(0.0))),
        "right" => Some((Some(false), BgPos::Pct(1.0))),
        "top" => Some((Some(true), BgPos::Pct(0.0))),
        "bottom" => Some((Some(true), BgPos::Pct(1.0))),
        "center" => Some((None, BgPos::Pct(0.5))),
        _ => match parse_len_opt(v, u)? {
            Len::Pct(p) => Some((None, BgPos::Pct(p / 100.0))),
            Len::Px(p) => Some((None, BgPos::Px(p))),
            Len::Calc { pct, px } if pct == 0.0 => Some((None, BgPos::Px(px))),
            _ => None,
        },
    }
}

/// `background-position` (css-backgrounds-3 §3.6), one- and two-value forms.
/// A keyword binds to its own axis regardless of order, so `center right`
/// means x=right, y=center.
fn parse_bg_pos(v: &str, u: Units) -> Option<(BgPos, BgPos)> {
    let t = css_tokens(v);
    let (mut x, mut y) = (None, None);
    match t.as_slice() {
        [a] => {
            let (axis, p) = parse_pos_component(a, u)?;
            match axis {
                Some(true) => y = Some(p),
                _ => x = Some(p),
            }
        }
        [a, b] => {
            let (ax, pa) = parse_pos_component(a, u)?;
            let (bx, pb) = parse_pos_component(b, u)?;
            // Reject a pair that names the same axis twice (`left right`).
            if ax == Some(true) || bx == Some(false) {
                if ax == Some(false) || bx == Some(true) {
                    return None;
                }
                y = Some(pa);
                x = Some(pb);
            } else {
                x = Some(pa);
                y = Some(pb);
            }
        }
        _ => return None,
    }
    Some((x.unwrap_or(BgPos::Pct(0.5)), y.unwrap_or(BgPos::Pct(0.5))))
}

fn parse_bg_size(v: &str, u: Units) -> Option<BgSize> {
    let t = css_tokens(v);
    let axis = |s: &str| -> Option<Option<Len>> {
        if s == "auto" {
            return Some(None);
        }
        match parse_len_opt(s, u)? {
            Len::Auto => Some(None),
            l => Some(Some(l)),
        }
    };
    match t.as_slice() {
        ["cover"] => Some(BgSize::Cover),
        ["contain"] => Some(BgSize::Contain),
        ["auto"] => Some(BgSize::Auto),
        [a] => Some(BgSize::Fixed(axis(a)?, None)),
        [a, b] => Some(BgSize::Fixed(axis(a)?, axis(b)?)),
        _ => None,
    }
}

/// The `background`/`mask` shorthand, parsed as a unit: `(colour, layer)`.
///
/// `None` means the value is INVALID and the whole declaration must be dropped
/// (css-syntax-3 §4) — `background: "red"` and `background:\0020red` are the
/// reftests that insist on it. That distinction is the whole reason this
/// returns a result instead of mutating: a shorthand resets every longhand it
/// covers, so treating an unparseable value as "no colour named" would clear a
/// perfectly good background instead of leaving it alone.
fn parse_bg_shorthand(
    val: &str,
    v: &str,
    u: Units,
    theme: &Theme,
) -> Option<(Option<Rgb>, BgLayer)> {
    let mut layer = BgLayer::NONE;
    let mut color = None;
    layer.image = parse_bg_image(val);
    // Position and size are written `<position> / <size>`; everything else is
    // order-free. Collect the unclaimed tokens and split them on the slash.
    // The slash need not be spaced (`center/contain`), so give it room first.
    let spaced = first_layer(v).replace('/', " / ");
    let mut parts: Vec<&str> = Vec::new();
    for tok in css_tokens(&spaced) {
        if let Some(r) = parse_bg_repeat(tok) {
            layer.repeat = r;
        } else if let Some(cv) = parse_color_val(tok, theme) {
            color = match cv {
                ColorVal::Rgb(c) => Some(c),
                ColorVal::Transparent => None,
            };
        } else if matches!(
            tok,
            "scroll" | "fixed" | "local" | "border-box" | "padding-box" | "content-box" | "none"
        ) || tok.starts_with("url(")
            // A gradient is valid CSS we cannot paint. Accepting it keeps the
            // reset (`background: <gradient>` HAS no colour) instead of
            // dropping the declaration and leaving a stale one in place.
            || tok.contains("-gradient(")
        {
            // attachment / origin / clip / the image — not the layer's placement
        } else {
            parts.push(tok);
        }
    }
    let slash = parts.iter().position(|t| *t == "/");
    let (pos_toks, size_toks) = match slash {
        Some(i) => (&parts[..i], &parts[i + 1..]),
        None => (&parts[..], &parts[parts.len()..]),
    };
    if !pos_toks.is_empty() {
        layer.pos = parse_bg_pos(&pos_toks.join(" "), u)?;
    }
    if !size_toks.is_empty() {
        layer.size = parse_bg_size(&size_toks.join(" "), u)?;
    }
    Some((color, layer))
}

/// `width`/`height`/`min`/`max` reject negative used lengths as invalid.
fn size_non_negative(l: &Len) -> bool {
    match l {
        Len::Px(p) | Len::Pct(p) => *p >= 0.0,
        Len::Auto | Len::Calc { .. } => true,
    }
}

/// Assign a size property only if the value is valid AND non-negative, else
/// keep the prior value (invalid declaration dropped).
/// Parse an HTML *dimension* attribute value: a bare number is pixels, a
/// trailing `%` is a percentage (HTML §2.4.4.4). Deliberately NOT the CSS
/// length parser — `width="200"` carries no unit and CSS would reject it,
/// which is precisely how these attributes came to be ignored.
fn parse_dimension_attr(v: &str) -> Option<Len> {
    let v = v.trim();
    let (num, pct) = match v.strip_suffix('%') {
        Some(n) => (n.trim(), true),
        None => (v, false),
    };
    // Trailing junk is not a number: HTML's own rule is to stop at the first
    // non-digit, but a strict parse keeps a typo from becoming a silent 0.
    let n: f32 = num.parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    Some(if pct { Len::Pct(n) } else { Len::Px(n) })
}

fn set_size(slot: &mut Len, v: &str, u: Units) {
    if let Some(l) = parse_len_opt(v, u).filter(size_non_negative) {
        *slot = l;
    }
}

/// `max-width`/`max-height`: `none` = no maximum (Auto); else a non-negative size.
fn set_max(slot: &mut Len, v: &str, u: Units) {
    if v.trim() == "none" {
        *slot = Len::Auto;
    } else if let Some(l) = parse_len_opt(v, u).filter(size_non_negative) {
        *slot = l;
    }
}

/// Resolve a `calc()` to affine `(pct, px)` form via the full values resolver:
/// evaluate with a %-basis of 0 (→ the px part) and 100 (→ px + pct), so any
/// `%`/px/em/vw mix collapses to `pct% of basis + px`.
fn parse_calc_affine(v: &str, u: Units) -> Option<Len> {
    let at0 = crate::values::resolve_length(
        v,
        &crate::values::LenCtx { em: u.em, rem: u.rem, pct_basis: 0.0, vw: u.vw, vh: u.vh },
    )?;
    let at100 = crate::values::resolve_length(
        v,
        &crate::values::LenCtx { em: u.em, rem: u.rem, pct_basis: 100.0, vw: u.vw, vh: u.vh },
    )?;
    let pct = at100 - at0;
    if (-0.001..0.001).contains(&pct) {
        Some(Len::Px(at0))
    } else {
        Some(Len::Calc { pct, px: at0 })
    }
}

/// Whether a token can start a `font-size` — the shorthand's anchor: everything
/// before it is style/variant/weight, everything after it is the family.
fn is_font_size_token(t: &str) -> bool {
    let head = t.split('/').next().unwrap_or("");
    matches!(
        head,
        "xx-small" | "x-small" | "small" | "medium" | "large" | "x-large" | "xx-large"
            | "larger" | "smaller"
    ) || head.starts_with(|c: char| c.is_ascii_digit() || c == '.')
}

/// `font: [<style> || <variant> || <weight>] <size>[/<line-height>] <family>`
/// (CSS 2.1 §15.8). Resetting every sub-property it does not mention is the
/// whole point of the shorthand, so unspecified style/weight/line-height go
/// back to their initial values rather than keeping what the cascade had.
fn apply_font_shorthand(v: &str, theme: &Theme, s: &mut ComputedStyle) {
    let t = v.trim();
    if t.is_empty() {
        return;
    }
    // `inherit` restores the parent's font; `em_base` IS the parent's size.
    // System-font keywords have no user-configurable faces here, so they take
    // the UA body font.
    if matches!(t, "inherit" | "unset" | "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar") {
        s.font_px = if t == "inherit" || t == "unset" { s.em_base } else { BASE_FONT_PX };
        s.line_height = LineHeight::Normal;
        return;
    }
    let toks: Vec<&str> = t.split_whitespace().collect();
    let Some(i) = toks.iter().position(|k| is_font_size_token(k)) else {
        return; // no size → not a valid font shorthand, change nothing
    };
    let (lead, rest) = toks.split_at(i);
    let (mut bold, mut italic) = (false, false);
    for k in lead {
        match *k {
            "normal" | "small-caps" | "lighter" | "condensed" | "expanded" => {}
            "italic" | "oblique" => italic = true,
            "bold" | "bolder" => bold = true,
            w => match w.parse::<u32>() {
                Ok(n) => bold = n >= 600,
                Err(_) => return, // an unknown keyword invalidates the whole value
            },
        }
    }
    let mut parts = rest[0].splitn(2, '/');
    let size = parts.next().unwrap_or("");
    let lh = parts.next().map(|s| s.to_string());
    s.bold = bold;
    s.italic = italic;
    s.line_height = LineHeight::Normal;
    apply_one("font-size", size, theme, s);
    if let Some(lh) = lh {
        if !lh.is_empty() {
            apply_one("line-height", &lh, theme, s);
        }
    }
    if rest.len() > 1 {
        apply_one("font-family", &rest[1..].join(" "), theme, s);
    }
}

/// `<a> [<b>]` — a two-sided logical shorthand. One value applies to both.
fn split_sides(v: &str) -> [&str; 2] {
    let mut it = v.split_whitespace();
    let a = it.next().unwrap_or("0");
    [a, it.next().unwrap_or(a)]
}

/// A `list-style-type` keyword, or `None` for anything we don't render as a
/// marker (`inside`/`outside`/`url(…)`/an unknown counter style).
fn parse_list_style(v: &str) -> Option<ListStyle> {
    Some(match v {
        "none" => ListStyle::None,
        "disc" => ListStyle::Disc,
        "circle" => ListStyle::Circle,
        "square" => ListStyle::Square,
        "decimal" => ListStyle::Decimal,
        "decimal-leading-zero" => ListStyle::DecimalLeadingZero,
        "lower-alpha" | "lower-latin" => ListStyle::LowerAlpha,
        "upper-alpha" | "upper-latin" => ListStyle::UpperAlpha,
        "lower-roman" => ListStyle::LowerRoman,
        "upper-roman" => ListStyle::UpperRoman,
        _ => return None,
    })
}

/// Top/bottom margin: `auto` computes to 0 for block boxes.
fn margin_tb(v: &str, u: Units) -> f32 {
    if v.trim() == "auto" { 0.0 } else { parse_length(v, u).unwrap_or(0.0) }
}

/// Left/right margin keeps `auto` (drives centering / slack).
fn margin_lr(v: &str, u: Units) -> Len {
    parse_len(v, u)
}

/// A padding length. Negative is invalid (padding ≥ 0) → keeps `prior`.
/// `transform` → a translation, or `None` for anything else.
///
/// `translate(x[,y])` / `translateX(x)` / `translateY(y)` only. A rotation or a
/// scale is deliberately dropped rather than approximated: half a transform
/// moves a box to a place neither the author nor the untransformed layout
/// intended. Percentages resolve against the BOX's own size, not the containing
/// block, so they are kept as `Len::Pct` until paint.
fn parse_translate(v: &str, u: Units) -> Option<(Len, Len)> {
    let t = v.trim();
    let (name, args) = t.split_once('(')?;
    let args = args.strip_suffix(')')?;
    let name = name.trim();
    let mut parts = args.split(',').map(str::trim);
    let first = parse_len_opt(parts.next()?, u)?;
    let second = match parts.next() {
        Some(p) => Some(parse_len_opt(p, u)?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    let zero = Len::Px(0.0);
    if name.eq_ignore_ascii_case("translate") {
        Some((first, second.unwrap_or(zero)))
    } else if name.eq_ignore_ascii_case("translateX") {
        second.is_none().then_some((first, zero))
    } else if name.eq_ignore_ascii_case("translateY") {
        second.is_none().then_some((zero, first))
    } else {
        None
    }
}

/// One `box-shadow` layer: `[<color>]? <dx> <dy> [<blur>] [<spread>] [<color>]?`.
/// `inset` is recognised and rejected — an inner shadow is a different paint,
/// and drawing it as an outer one would put a slab OUTSIDE the box.
/// Lengths keep their order; the colour may sit at either end (CSS Backgrounds 3
/// §7.1). An omitted colour stays `None` = `currentColor`, resolved at paint.
fn parse_box_shadow(v: &str, u: Units) -> Option<BoxShadow> {
    let mut lens: [f32; 4] = [0.0; 4];
    let mut n = 0usize;
    let mut color: Option<Rgb> = None;
    for tok in css_tokens(v) {
        if tok.eq_ignore_ascii_case("inset") {
            return None;
        }
        if let Some(px) = parse_length(tok, u) {
            if n < 4 {
                lens[n] = px;
                n += 1;
            }
            continue;
        }
        if let Some(c) = parse_color(tok, &Theme::DARK) {
            color = Some(c);
            continue;
        }
        // An unknown token invalidates the layer rather than being ignored —
        // otherwise a value we cannot read paints something the author never
        // asked for.
        return None;
    }
    if n < 2 {
        return None;
    }
    Some(BoxShadow {
        dx: lens[0],
        dy: lens[1],
        blur: if n > 2 { lens[2] } else { 0.0 },
        spread: if n > 3 { lens[3] } else { 0.0 },
        color,
    })
}

fn parse_pad(v: &str, u: Units, prior: f32) -> f32 {
    let v = v.trim();
    // `parse_length` knows units, not functions — so `padding: calc(…)` was
    // dropped wholesale and the side kept its previous value. A percentage
    // still resolves roughly here (it belongs to the containing block's WIDTH,
    // which this parse cannot see); the math functions are exact.
    let px = if is_math_fn(v) {
        crate::values::resolve_length(
            v,
            &crate::values::LenCtx { em: u.em, rem: u.rem, pct_basis: 0.0, vw: u.vw, vh: u.vh },
        )
    } else {
        parse_length(v, u)
    };
    match px {
        Some(p) if p >= 0.0 => p,
        _ => prior,
    }
}

/// A border-width keyword/length → px. `thin`/`medium`/`thick` = 1/3/5px.
/// A NEGATIVE length is invalid, not zero: the declaration is dropped and the
/// side keeps the width it had (`border-top-width-012` and its siblings turn
/// on exactly that difference).
fn border_width_kw(tok: &str, u: Units) -> Option<f32> {
    match tok.trim() {
        "" => None,
        "thin" => Some(1.0),
        "medium" => Some(BORDER_MEDIUM),
        "thick" => Some(5.0),
        t => parse_length(t, u).filter(|w| *w >= 0.0),
    }
}

/// Assign one side's border width (keeps the prior value on an invalid one).
fn set_side_width(side: &mut BorderSide, v: &str, u: Units) {
    if let Some(w) = border_width_kw(v.trim(), u) {
        side.set_spec_width(w);
    }
}

/// Parse a `border`/`border-<side>` shorthand (`<width> || <style> || <color>`,
/// any order) into a side. `none`/`hidden` → no border. A specified border with
/// no explicit colour uses `currentColor` (the element's `color`).
fn parse_border_shorthand(v: &str, u: Units, theme: &Theme) -> BorderSide {
    // A shorthand resets every side it names, so this starts from the initial
    // value rather than from whatever came before.
    let mut side = BorderSide::default();
    let mut width = None;
    for tok in css_tokens(v) {
        match tok {
            "none" | "hidden" | "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge"
            | "inset" | "outset" => side.set_style(tok),
            "thin" | "medium" | "thick" => width = border_width_kw(tok, u),
            _ => {
                if !side.set_color(tok, theme) {
                    if let Some(w) = parse_length(tok, u).filter(|w| *w >= 0.0) {
                        width = Some(w);
                    }
                }
            }
        }
    }
    if let Some(w) = width {
        side.set_spec_width(w);
    }
    // The shorthand resets the whole side whatever it names, so writing it at
    // all is taking control of the frame — `border: red` suppresses one just as
    // `border: none` does.
    side.specified = true;
    side
}

/// Expand 1–4 CSS box-side tokens into [top, right, bottom, left].
fn four_sides<'a>(toks: &[&'a str]) -> Option<[&'a str; 4]> {
    match toks.len() {
        1 => Some([toks[0], toks[0], toks[0], toks[0]]),
        2 => Some([toks[0], toks[1], toks[0], toks[1]]),
        3 => Some([toks[0], toks[1], toks[2], toks[1]]),
        4 => Some([toks[0], toks[1], toks[2], toks[3]]),
        _ => None,
    }
}

/// Expand a 1–4 token box shorthand into (top, right, bottom, left).
fn four_values(v: &str) -> (&str, &str, &str, &str) {
    let p: alloc::vec::Vec<&str> = v.split_whitespace().collect();
    match p.len() {
        0 => ("0", "0", "0", "0"),
        1 => (p[0], p[0], p[0], p[0]),
        2 => (p[0], p[1], p[0], p[1]),
        3 => (p[0], p[1], p[2], p[1]),
        _ => (p[0], p[1], p[2], p[3]),
    }
}

/// A parsed track list: `n` tracks in `tracks`, plus—if the source held a
/// `repeat(auto-fill|auto-fit, …)`—the one-copy pattern's span (`fill_start` ..
/// `+fill_len`) and `fill` kind (1 = auto-fill, 2 = auto-fit) so layout can
/// expand it to the container width.
pub struct TrackList {
    pub n: u8,
    pub tracks: [GridTrack; MAX_GRID_COLS],
    pub fill: u8,
    pub fill_start: u8,
    pub fill_len: u8,
}

/// Parse a `grid-template-*` value, expanding `repeat(n, …)` and recording any
/// `repeat(auto-fill|auto-fit, …)`. `[line-name]` tokens are skipped.
/// Truncates at `MAX_GRID_COLS`.
fn parse_grid_tracks(v: &str, u: Units) -> TrackList {
    let mut tracks = [GridTrack::Auto; MAX_GRID_COLS];
    let mut n = 0usize;
    let (mut fill, mut fill_start, mut fill_len) = (0u8, 0u8, 0u8);
    for tok in split_top_level(v) {
        if n >= MAX_GRID_COLS {
            break;
        }
        if tok.starts_with('[') {
            continue; // line name — no track
        }
        let inner = tok.strip_prefix("repeat(").and_then(|s| s.strip_suffix(')'));
        if let Some(inner) = inner {
            let mut parts = inner.splitn(2, ',');
            let count_s = parts.next().unwrap_or("").trim();
            let sub = parse_grid_tracks(parts.next().unwrap_or("").trim(), u);
            let auto = match count_s {
                "auto-fill" => 1u8,
                "auto-fit" => 2u8,
                _ => 0u8,
            };
            if auto != 0 {
                // Store one copy; layout repeats it to fill the width.
                fill = auto;
                fill_start = n as u8;
                fill_len = sub.n;
                for t in sub.tracks.iter().take(sub.n as usize) {
                    if n < MAX_GRID_COLS {
                        tracks[n] = *t;
                        n += 1;
                    }
                }
            } else {
                let count = count_s.parse::<usize>().unwrap_or(1);
                for _ in 0..count {
                    for t in sub.tracks.iter().take(sub.n as usize) {
                        if n < MAX_GRID_COLS {
                            tracks[n] = *t;
                            n += 1;
                        }
                    }
                }
            }
        } else {
            tracks[n] = parse_track(&tok, u);
            n += 1;
        }
    }
    TrackList { n: n as u8, tracks, fill, fill_start, fill_len }
}

fn parse_track(t: &str, u: Units) -> GridTrack {
    let t = t.trim();
    if t == "auto" || t == "min-content" || t == "max-content" {
        GridTrack::Auto
    } else if let Some(f) = t.strip_suffix("fr") {
        GridTrack::Fr(f.trim().parse().unwrap_or(1.0))
    } else if let Some(p) = t.strip_suffix('%') {
        GridTrack::Pct(p.trim().parse().unwrap_or(0.0))
    } else if let Some(inner) = t.strip_prefix("minmax(").and_then(|r| r.strip_suffix(')')) {
        // minmax(min, max): size by the MAX (min=0 lets it shrink to fit). A
        // bare max length must become a Fixed CAP — not unbounded `Auto`
        // max-content, which blows a `minmax(0,59.25rem)` content column up to
        // the whole article's unwrapped width.
        let max_part = inner.split(',').nth(1).unwrap_or(inner).trim();
        parse_track(max_part, u)
    } else {
        // A fixed length: px/em/rem/pt/cm/vw/… against the element's own units.
        match parse_length(t, u) {
            Some(px) => GridTrack::Fixed(px),
            None => GridTrack::Auto,
        }
    }
}

/// Split on whitespace, but keep `repeat(…)` / `minmax(…)` (parens) intact.
fn split_top_level(v: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut out = alloc::vec::Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in v.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(core::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Split a value at the top-level `/` (respecting `repeat(…)`/`minmax(…)`
/// parens), returning `(before, after)`. `None` if there is no top-level slash.
/// Parse `grid-template-areas` strings into the container's named-area map. Each
/// quoted string is a row; whitespace-separated tokens are cell names (`.` =
/// empty). An area's rectangle is the bounding box of its cells.
fn set_grid_areas(s: &mut ComputedStyle, v: &str) {
    let bytes = v.as_bytes();
    let mut names: alloc::vec::Vec<(u32, u8, u8, u8, u8)> = alloc::vec::Vec::new();
    let (mut i, mut r) = (0usize, 0u8);
    while i < bytes.len() {
        while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\'' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let q = bytes[i];
        i += 1;
        let s0 = i;
        while i < bytes.len() && bytes[i] != q {
            i += 1;
        }
        let row = &v[s0..i.min(v.len())];
        i += 1;
        for (c, tok) in row.split_whitespace().enumerate() {
            if tok == "." {
                continue;
            }
            let (h, c) = (area_hash(tok), c as u8);
            if let Some(a) = names.iter_mut().find(|a| a.0 == h) {
                a.1 = a.1.min(r);
                a.2 = a.2.max(r + 1);
                a.3 = a.3.min(c);
                a.4 = a.4.max(c + 1);
            } else if names.len() < GRID_AREAS_MAX {
                names.push((h, r, r + 1, c, c + 1));
            }
        }
        r += 1;
    }
    if names.is_empty() {
        return;
    }
    for (k, &(h, r0, r1, c0, c1)) in names.iter().enumerate() {
        s.grid_areas[k] = GridArea { name: h, r0, r1, c0, c1 };
    }
    s.grid_area_count = names.len() as u8;
    s.grid_nrows = s.grid_nrows.max(r);
    // With no explicit `grid-template-columns`, the template width defines the
    // (auto-sized) column tracks.
    if s.grid_ncols == 0 {
        let ncols = names.iter().map(|a| a.4).max().unwrap_or(0).min(MAX_GRID_COLS as u8);
        for k in 0..ncols as usize {
            s.grid_tracks[k] = GridTrack::Auto;
        }
        s.grid_ncols = ncols;
    }
}

fn split_slash(v: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    for (i, ch) in v.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            '/' if depth == 0 => return Some((&v[..i], &v[i + 1..])),
            _ => {}
        }
    }
    None
}

/// A single grid-line spec → its integer index (`0`/`None` for `auto`/named).
fn parse_line(t: &str) -> Option<i16> {
    let t = t.trim();
    if t.is_empty() || t == "auto" {
        return None;
    }
    t.parse::<i16>().ok()
}

/// `grid-column`/`grid-row` → `(start_line, span)`. `start_line == 0` means
/// auto-placement. Handles `span N`, `A / B`, `A / span N`, `span N / B`, `A`.
fn parse_line_placement(v: &str) -> (i16, u16) {
    let v = v.trim();
    if let Some((a, b)) = split_slash(v) {
        let (a, b) = (a.trim(), b.trim());
        let a_span = a.strip_prefix("span").map(|s| s.trim().parse::<u16>().unwrap_or(1));
        let b_span = b.strip_prefix("span").map(|s| s.trim().parse::<u16>().unwrap_or(1));
        match (a_span, b_span) {
            (Some(sp), _) => {
                // `span N / B` → end at B, start = B − N.
                let start = parse_line(b).map(|bl| bl - sp as i16).unwrap_or(0);
                (start, sp.max(1))
            }
            (None, Some(sp)) => (parse_line(a).unwrap_or(0), sp.max(1)),
            (None, None) => {
                let start = parse_line(a).unwrap_or(0);
                let span = match (parse_line(a), parse_line(b)) {
                    (Some(al), Some(bl)) if bl > al => (bl - al) as u16,
                    _ => 1,
                };
                (start, span.max(1))
            }
        }
    } else if let Some(s) = v.strip_prefix("span") {
        (0, s.trim().parse().unwrap_or(1))
    } else {
        (parse_line(v).unwrap_or(0), 1)
    }
}

fn parse_cross(v: &str) -> Option<CrossAlign> {
    Some(match v {
        "flex-start" | "start" | "self-start" | "baseline" => CrossAlign::Start,
        "flex-end" | "end" | "self-end" => CrossAlign::End,
        "center" => CrossAlign::Center,
        "stretch" | "normal" => CrossAlign::Stretch,
        _ => return None,
    })
}

fn parse_basis(v: &str, u: Units) -> FlexBasis {
    match v {
        "auto" | "content" | "max-content" | "min-content" | "fit-content" => FlexBasis::Auto,
        _ => {
            if let Some(pct) = v.strip_suffix('%') {
                pct.trim().parse::<f32>().map(FlexBasis::Pct).unwrap_or(FlexBasis::Auto)
            } else {
                parse_length(v, u).map(FlexBasis::Px).unwrap_or(FlexBasis::Auto)
            }
        }
    }
}

/// `flex` shorthand: keywords (`none`/`auto`/`initial`) or `grow [shrink] [basis]`.
/// A bare number `flex:1` = `1 1 0` (per spec `<n> 1 0%`).
fn apply_flex_shorthand(v: &str, s: &mut ComputedStyle) {
    match v {
        "none" => {
            s.flex_grow = 0.0;
            s.flex_shrink = 0.0;
            s.flex_basis = FlexBasis::Auto;
            return;
        }
        "auto" => {
            s.flex_grow = 1.0;
            s.flex_shrink = 1.0;
            s.flex_basis = FlexBasis::Auto;
            return;
        }
        "initial" => {
            s.flex_grow = 0.0;
            s.flex_shrink = 1.0;
            s.flex_basis = FlexBasis::Auto;
            return;
        }
        _ => {}
    }
    let mut nums = alloc::vec::Vec::new();
    let mut basis = None;
    for tok in v.split_whitespace() {
        if let Ok(f) = tok.parse::<f32>() {
            nums.push(f);
        } else {
            basis = Some(parse_basis(tok, s.units()));
        }
    }
    match nums.len() {
        0 => {}
        1 => {
            s.flex_grow = nums[0];
            s.flex_shrink = 1.0;
        }
        _ => {
            s.flex_grow = nums[0];
            s.flex_shrink = nums[1];
        }
    }
    s.flex_basis = basis.unwrap_or(if nums.is_empty() { FlexBasis::Auto } else { FlexBasis::Px(0.0) });
}

/// Parse a CSS `<length>` to px. Supports `px`, `em`/`rem` (relative to
/// `em_base`), and bare numbers (treated as px).
fn parse_length(v: &str, u: Units) -> Option<f32> {
    let v = v.trim();
    // Font-relative first so "rem" is matched before the "em" suffix eats it.
    // `rem` is ROOT-relative (not em_base) — else nested rem compounds wrongly.
    if let Some(n) = v.strip_suffix("rem") {
        return n.trim().parse::<f32>().ok().map(|f| f * u.rem);
    }
    if let Some(n) = v.strip_suffix("em") {
        return n.trim().parse::<f32>().ok().map(|f| f * u.em);
    }
    if let Some(n) = v.strip_suffix('%') {
        // No containing measure here → treat % of em (rough; refined later).
        return n.trim().parse::<f32>().ok().map(|f| f * u.em / 100.0);
    }
    // Viewport-percentage units (CSS Values 3 §5.1.2). BEFORE the absolute
    // table: `vmin` ends in `in`, so the inch arm would eat it otherwise.
    const VP: &[(&str, fn(&Units) -> f32)] = &[
        ("vmin", |u| if u.vw < u.vh { u.vw } else { u.vh }),
        ("vmax", |u| if u.vw > u.vh { u.vw } else { u.vh }),
        ("vw", |u| u.vw),
        ("vh", |u| u.vh),
    ];
    for (suf, basis) in VP {
        if let Some(n) = v.strip_suffix(suf) {
            return n.trim().parse::<f32>().ok().map(|f| f / 100.0 * basis(&u));
        }
    }
    // Absolute units → CSS reference pixels (1in = 96px, CSS Values 3 §5.2).
    const ABS: &[(&str, f32)] = &[
        ("px", 1.0),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("q", 96.0 / 25.4 / 4.0),
    ];
    for (suf, mul) in ABS {
        if let Some(n) = v.strip_suffix(suf) {
            return n.trim().parse::<f32>().ok().map(|f| f * mul);
        }
    }
    v.parse::<f32>().ok()
}

/// Parse a CSS `<color>`. Delegates to the full `color` module — hex
/// (#rgb/#rgba/#rrggbb/#rrggbbaa), rgb()/rgba()/hsl()/hsla(), and all 148 CSS
/// named colours. `None` keeps the inherited value (`currentcolor`/`inherit`/
/// `transparent`/unparseable), preserving the caller's contract.
fn parse_color(v: &str, _theme: &Theme) -> Option<Rgb> {
    crate::color::parse_color(v)
}

/// As [`parse_color`], but keeps "fully transparent" apart from "no value".
/// Use it wherever the property HAS a paint-nothing state (a border side, a
/// background); `parse_color` alone silently turns `rgba(0,0,0,0)` into the
/// inherited colour.
fn parse_color_val(v: &str, _theme: &Theme) -> Option<ColorVal> {
    crate::color::parse_color_val(v)
}

/// Split a CSS value on top-level whitespace, keeping parenthesised groups
/// (`rgb(0% 50% 0%)`, `calc(1px + 2px)`) intact as single tokens. Needed
/// because CSS function values contain internal spaces that a naive
/// `split_whitespace` would shred. Values here are ASCII, so byte slicing is
/// safe on the whitespace/paren boundaries.
fn css_tokens(v: &str) -> alloc::vec::Vec<&str> {
    let mut out = alloc::vec::Vec::new();
    let b = v.as_bytes();
    let (mut depth, mut start): (i32, Option<usize>) = (0, None);
    for i in 0..b.len() {
        let c = b[i];
        match c {
            b'(' => depth += 1,
            b')' => depth = (depth - 1).max(0),
            _ => {}
        }
        let is_ws = matches!(c, b' ' | b'\t' | b'\n' | b'\r');
        if depth == 0 && is_ws {
            if let Some(s0) = start.take() {
                out.push(&v[s0..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s0) = start {
        out.push(&v[s0..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css;
    use crate::dom;

    fn first_el(dom: &dom::Dom) -> &Element {
        match &dom.body().children[0] {
            dom::Node::Element(e) => e,
            _ => panic!("expected element"),
        }
    }

    #[test]
    fn ua_sheet_gives_headings_size_weight_and_colour() {
        let dom = dom::parse("<body><h1>x</h1></body>");
        let theme = Theme::DARK;
        let st = resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[], &[], 0, 1000.0);
        assert_eq!(st.display, Display::Block);
        assert!(st.bold);
        assert!(st.font_px > BASE_FONT_PX * 1.5);
        assert_eq!(st.color, theme.heading);
    }

    /// The four viewport-percentage units, everywhere a length is read.
    /// `vmin` is the one that needs care: it ends in `in`, so the inch arm of
    /// the absolute table eats it unless the viewport arms come first.
    #[test]
    fn viewport_units_resolve_against_the_viewport() {
        let st = |css: &str| {
            let html = alloc::format!("<body><p style=\"{css}\">x</p></body>");
            let dom = dom::parse(&html);
            let theme = Theme::DARK;
            let mut initial = ComputedStyle::root(&theme);
            initial.vw = 1000.0;
            initial.vh = 500.0;
            resolve(first_el(&dom), &initial, &theme, &Stylesheet::empty(), &[], &[], 0, 1000.0)
        };
        assert_eq!(st("width:50vw").width, Len::Px(500.0));
        assert_eq!(st("height:40vh").height, Len::Px(200.0));
        assert_eq!(st("width:10vmin").width, Len::Px(50.0), "vmin is the SHORTER side");
        assert_eq!(st("width:10vmax").width, Len::Px(100.0), "vmax is the longer one");
        assert_eq!(st("max-width:25vw").max_width, Len::Px(250.0));
        assert_eq!(st("padding-left:10vw").pad_left, 100.0);
        // A viewport unit is a length like any other: it composes with `calc()`
        // and it is a valid `font-size`, where it must NOT be read as an `em`.
        assert_eq!(st("width:calc(50vw - 20px)").width, Len::Px(480.0));
        assert_eq!(st("font-size:5vw").font_px, 50.0);
        // Nothing above may disturb the inch/mm arms that follow it.
        assert_eq!(st("width:1in").width, Len::Px(96.0));
    }

    /// The CSS math functions have to reach the BOX MODEL, not just custom
    /// properties. `values.rs` evaluated all four from the start, but
    /// `parse_len_opt` only routed `calc(`, so `width: max(20px, 10px)` failed
    /// its length parse and fell back to `auto`; and `parse_pad` called
    /// `parse_length` directly, so `padding: calc(…)` was dropped entirely.
    #[test]
    fn math_functions_reach_the_box_model() {
        let st = |css: &str| {
            let html = alloc::format!("<body><p style=\"{css}\">x</p></body>");
            let dom = dom::parse(&html);
            let theme = Theme::DARK;
            resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[], &[], 0, 1000.0)
        };
        assert_eq!(st("width:max(20px,10px)").width, Len::Px(20.0));
        assert_eq!(st("width:min(20px,40px)").width, Len::Px(20.0));
        assert_eq!(st("width:clamp(10px,20px,40px)").width, Len::Px(20.0));
        assert_eq!(st("width:MAX(20px,10px)").width, Len::Px(20.0), "function names are case-insensitive");
        // Nested, and mixed with the units a real page writes.
        assert_eq!(st("width:max(calc(1rem + 4px),10px)").width, Len::Px(20.0));
        assert_eq!(st("padding-left:calc(8px + 8px + calc(1rem + 4px))").pad_left, 36.0);
        assert_eq!(st("padding-left:max(12px,4px)").pad_left, 12.0);
        // A negative padding is invalid and must leave the side alone, exactly
        // as a plain negative length does.
        assert_eq!(st("padding-left:8px;padding-left:calc(0px - 4px)").pad_left, 8.0);
    }

    /// `border-width` and `border-style` are independent halves and neither
    /// implies the other: a width alone paints nothing, a style alone is
    /// `medium`, and the colour defaults to `currentColor` however late in the
    /// declaration block the `color` arrives.
    #[test]
    fn a_border_needs_both_a_style_and_a_width() {
        let side = |css: &str| {
            let html = alloc::format!("<body><p style=\"{css}\">x</p></body>");
            let dom = dom::parse(&html);
            let theme = Theme::DARK;
            resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[], &[], 0, 1000.0)
                .border_top
        };
        assert_eq!(side("border-top-width:5px").width, 0.0, "a width with no style is not a border");
        assert_eq!(side("border-top-style:solid").width, 3.0, "a style with no width is medium");
        assert_eq!(side("border-top-style:solid;border-top-width:5px").width, 5.0);
        assert_eq!(side("border-top-width:5px;border-top-style:solid").width, 5.0, "either order");
        assert_eq!(side("border-top:5px solid;border-top-style:none").width, 0.0, "none takes it away");
        // An invalid width leaves the specified one alone — it does not fall
        // back to 0, which is what `border-top-width-012` checks.
        assert_eq!(side("border-top-style:solid;border-top-width:-1pt").width, 3.0);
        let c = Rgb(0, 128, 0);
        assert_eq!(side("border-top-style:solid;color:#008000").color, Some(c), "currentColor, resolved late");
        assert_eq!(side("color:#008000;border-top-style:solid").color, Some(c), "either order");
        // `transparent` is a VALUE: the width stays, nothing paints, and it is
        // not the same as leaving the colour unset (that means currentColor).
        let t = side("border-top:1px solid #f00;border-top-color:transparent");
        assert_eq!(t.width, 1.0, "a transparent border still takes its space");
        assert_eq!(t.color, None, "and paints nothing");
        assert_eq!(side("border-top:1px solid transparent").color, None, "also in the shorthand");
        // …and a real colour after it wins back.
        assert_eq!(side("border-top-color:transparent;border-top:1px solid #f00").color, Some(Rgb(255, 0, 0)));
    }

    #[test]
    fn inline_style_attribute_is_parsed() {
        let dom = dom::parse("<body><p style=\"color:#ff0000; font-weight:bold; font-size:20px\">x</p></body>");
        let theme = Theme::DARK;
        let st = resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[], &[], 0, 1000.0);
        assert_eq!(st.color, Rgb(255, 0, 0));
        assert!(st.bold);
        assert_eq!(st.font_px, 20.0);
    }

    #[test]
    fn author_stylesheet_applies_and_inline_wins() {
        let dom = dom::parse(
            "<body><p class=\"lead\" style=\"color:#00ff00\">x</p><p class=\"lead\">y</p></body>",
        );
        let theme = Theme::DARK;
        let sheet = css::parse(".lead { color: #ff0000; font-weight: bold }");
        let root = ComputedStyle::root(&theme);
        // 1st <p>: author sets red+bold, inline overrides colour to green.
        let a = resolve(first_el(&dom), &root, &theme, &sheet, &[], &[], 0, 1000.0);
        assert_eq!(a.color, Rgb(0, 255, 0));
        assert!(a.bold);
        // 2nd <p>: author red+bold, no inline.
        let p2 = match &dom.body().children[1] {
            dom::Node::Element(e) => e,
            _ => panic!(),
        };
        let b = resolve(p2, &root, &theme, &sheet, &[], &[], 0, 1000.0);
        assert_eq!(b.color, Rgb(255, 0, 0));
        assert!(b.bold);
    }

    #[test]
    fn important_beats_specificity_and_inline() {
        let theme = Theme::DARK;
        let root = ComputedStyle::root(&theme);
        // !important on a low-specificity class beats a higher-specificity #id.
        let dom = dom::parse("<body><p id=\"x\" class=\"b\">x</p></body>");
        let sheet = css::parse("#x{color:#ff0000} .b{color:#00ff00 !important}");
        let st = resolve(first_el(&dom), &root, &theme, &sheet, &[], &[], 0, 1000.0);
        assert_eq!(st.color, Rgb(0, 255, 0), "!important beats #id specificity");
        // author !important beats a normal inline style.
        let dom2 = dom::parse("<body><p class=\"b\" style=\"color:#ff0000\">x</p></body>");
        let sheet2 = css::parse(".b{color:#00ff00 !important}");
        let st2 = resolve(first_el(&dom2), &root, &theme, &sheet2, &[], &[], 0, 1000.0);
        assert_eq!(st2.color, Rgb(0, 255, 0), "author !important beats inline normal");
        // a later normal declaration must NOT override an earlier !important.
        let dom3 = dom::parse("<body><p class=\"b\">x</p></body>");
        let sheet3 = css::parse(".b{color:#00ff00 !important} p{color:#ff0000}");
        let st3 = resolve(first_el(&dom3), &root, &theme, &sheet3, &[], &[], 0, 1000.0);
        assert_eq!(st3.color, Rgb(0, 255, 0), "normal cannot override !important");
    }

    #[test]
    fn head_and_script_are_display_none() {
        let theme = Theme::DARK;
        let root = ComputedStyle::root(&theme);
        for tag in ["head", "script", "style", "title"] {
            let html = alloc::format!("<{tag}>x</{tag}>");
            let dom = dom::parse(&html);
            // Find by tag, not by position: the parser implies <html>/<body>
            // around the fragment (HTML Standard §13.2.6).
            fn find<'a>(el: &'a Element, tag: &str) -> Option<&'a Element> {
                for c in &el.children {
                    if let dom::Node::Element(e) = c {
                        if e.tag == tag {
                            return Some(e);
                        }
                        if let Some(f) = find(e, tag) {
                            return Some(f);
                        }
                    }
                }
                None
            }
            let e = find(&dom.root, tag).expect(tag);
            let st = resolve(e, &root, &theme, &Stylesheet::empty(), &[], &[], 0, 1000.0);
            assert_eq!(st.display, Display::None, "{tag}");
        }
    }

    /// The value arrives at `apply_one` lowercased for keyword matching — a
    /// `data:` URI must NOT be taken from that copy, or its base64 payload is
    /// silently corrupted.
    #[test]
    fn data_uri_keeps_its_case() {
        let theme = Theme::DARK;
        let mut st = ComputedStyle::root(&theme);
        let uri = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
        apply_one("background-image", &alloc::format!("url({uri})"), &theme, &mut st);
        assert_eq!(st.bg_layer.image, Some(crate::css::url_key(uri)));
        assert_ne!(
            st.bg_layer.image,
            Some(crate::css::url_key(&uri.to_ascii_lowercase())),
            "the key must be over the original bytes"
        );
    }

    /// A `data:` URI is full of commas; the layer split must not cut it.
    #[test]
    fn data_uri_survives_the_layer_split() {
        let theme = Theme::DARK;
        let mut st = ComputedStyle::root(&theme);
        let uri = "data:image/svg+xml,%3Csvg viewBox='0,0,4,4'%3E%3C/svg%3E";
        apply_one("mask-image", &alloc::format!("url(\"{uri}\")"), &theme, &mut st);
        assert_eq!(st.mask_layer.image, Some(crate::css::url_key(uri)));
    }

    #[test]
    fn background_shorthand_resets_the_image_but_background_color_does_not() {
        let theme = Theme::DARK;
        let mut st = ComputedStyle::root(&theme);
        apply_one("background-image", "url(a.png)", &theme, &mut st);
        apply_one("background-color", "red", &theme, &mut st);
        assert!(st.bg_layer.image.is_some(), "background-color is not a shorthand");
        apply_one("background", "red", &theme, &mut st);
        assert_eq!(st.bg_layer.image, None, "the shorthand resets every longhand it covers");
    }

    /// A keyword binds to its own axis whatever the order — `center right`
    /// means x=right, y=center (css-backgrounds-3 §3.6).
    #[test]
    fn background_position_keywords_bind_per_axis() {
        let theme = Theme::DARK;
        let mut st = ComputedStyle::root(&theme);
        apply_one("background-position", "center right", &theme, &mut st);
        assert_eq!(st.bg_layer.pos, (BgPos::Pct(1.0), BgPos::Pct(0.5)));
        apply_one("background-position", "bottom", &theme, &mut st);
        assert_eq!(st.bg_layer.pos, (BgPos::Pct(0.5), BgPos::Pct(1.0)));
        // Two horizontal keywords are not a position at all → declaration dropped.
        let before = st.bg_layer.pos;
        apply_one("background-position", "left right", &theme, &mut st);
        assert_eq!(st.bg_layer.pos, before);
    }

    #[test]
    fn background_size_and_repeat() {
        let theme = Theme::DARK;
        let mut st = ComputedStyle::root(&theme);
        apply_one("background-size", "0.857em", &theme, &mut st);
        let em = st.font_px * 0.857;
        assert_eq!(st.bg_layer.size, BgSize::Fixed(Some(Len::Px(em)), None));
        apply_one("background-repeat", "no-repeat", &theme, &mut st);
        assert_eq!(st.bg_layer.repeat, (false, false));
        apply_one("background-repeat", "repeat-x", &theme, &mut st);
        assert_eq!(st.bg_layer.repeat, (true, false));
    }

    /// The form the icon systems ship: one shorthand carrying url, position,
    /// size and repeat, with an unspaced slash.
    #[test]
    fn mask_shorthand_carries_position_and_size() {
        let theme = Theme::DARK;
        let mut st = ComputedStyle::root(&theme);
        apply_one("-webkit-mask", "url(i.svg) center/contain no-repeat", &theme, &mut st);
        assert_eq!(st.mask_layer.image, Some(crate::css::url_key("i.svg")));
        assert_eq!(st.mask_layer.pos, (BgPos::Pct(0.5), BgPos::Pct(0.5)));
        assert_eq!(st.mask_layer.size, BgSize::Contain);
        assert_eq!(st.mask_layer.repeat, (false, false));
    }

    /// We cannot paint a gradient, and pretending we can would be worse than
    /// leaving the box flat.
    #[test]
    fn gradient_leaves_the_layer_imageless() {
        let theme = Theme::DARK;
        let mut st = ComputedStyle::root(&theme);
        apply_one("background-image", "linear-gradient(red, blue)", &theme, &mut st);
        assert_eq!(st.bg_layer.image, None);
    }
}
