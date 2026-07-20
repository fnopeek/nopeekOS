//! style.rs — computed style + the UA default stylesheet, as data.
//!
//! CONFORMANCE.md's rule: be *standard-shaped* from the start. Slice-0 baked
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

use alloc::string::String;

use crate::css::{ElemInfo, PseudoElem, Stylesheet};
use crate::dom::Element;
use crate::layout::{Rgb, Theme};

/// CSS `display` — only the values our layout implements so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    None,
    Block,
    Inline,
    ListItem,
    /// `<table>` — establishes the (simplified) table formatting context in
    /// `layout.rs`; its `tr`/`td`/`th` descendants are laid by that walker.
    Table,
    /// `display: flex` — flex formatting context (single-line) in `layout.rs`.
    Flex,
    /// `display: grid` — grid formatting context (explicit columns + auto rows).
    Grid,
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
/// only when `width > 0` AND `color` is set (`border-style:none`/no colour → no
/// paint). The four sides are independent (`border-top`/`-right`/… may differ).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct BorderSide {
    pub width: f32,
    pub color: Option<Rgb>,
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
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub pre: bool, // white-space: pre (no collapse, honor newlines)
    pub color: Rgb,
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
    pub contain_size: bool, // `contain: size`/`strict` — content contributes no size
    pub contain_intrinsic: Option<(f32, f32)>, // `contain-intrinsic-size` (w, h) px
    pub bg: Option<Rgb>, // background-color (None = transparent)
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
    pub valign: i8, // vertical-align: super (+1) / sub (-1) / baseline (0) — not inherited
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
}

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
            bold: false,
            italic: false,
            mono: false,
            pre: false,
            color: theme.text,
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
            contain_size: false,
            contain_intrinsic: None,
            bg: None,
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
            valign: 0,
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
        }
    }
}

pub const BASE_FONT_PX: f32 = 16.0;

/// The starting point for any freshly-resolved style: the inherited slice
/// copied from `parent`, non-inherited properties reset to their CSS initial
/// value. Shared by `resolve()` (a real element) and `resolve_pseudo()` (a
/// `::before`/`::after` generated box, which inherits from its originating
/// element the same way a child would).
fn inherit_reset(parent: &ComputedStyle) -> ComputedStyle {
    ComputedStyle {
        font_px: parent.font_px,
        em_base: parent.font_px,
        bold: parent.bold,
        italic: parent.italic,
        mono: parent.mono,
        pre: parent.pre,
        color: parent.color,
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
        contain_size: false,
        contain_intrinsic: None,
        bg: None,
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
        valign: 0,
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

    // Author `<style>` rules, applied low→high specificity (ties: doc order).
    if !sheet.is_empty() {
        let info = ElemInfo::of(el);
        let mut matched = sheet.matched(&info, ancestors, prev_siblings, sib_count, viewport_w);
        matched.sort_by_key(|(spec, order, _)| (*spec, *order));
        for (_, _, decls) in matched {
            for (p, v) in decls {
                apply_one(p, v, theme, &mut s);
            }
        }
    }

    if let Some(decls) = el.attr("style") {
        apply_declarations(decls, theme, &mut s);
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
    s
}

/// Resolve `el`'s `::before`/`::after` generated box: the winning `content`
/// declaration (by the same specificity/order cascade as any other property)
/// plus the pseudo-element's own computed style. Returns `None` when there is
/// no matching rule, `content` is `none`/`normal`/unparseable (`attr()`,
/// `counter()`, `open-quote`, `url()`, … are out of scope — CONFORMANCE.md's
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
) -> Option<(String, ComputedStyle)> {
    if sheet.is_empty() {
        return None;
    }
    let info = ElemInfo::of(el);
    let mut matched = sheet.matched_pseudo(&info, ancestors, prev_siblings, sib_count, viewport_w, pseudo);
    if matched.is_empty() {
        return None;
    }
    matched.sort_by_key(|(spec, order, _)| (*spec, *order));
    // The winning `content` value: later (higher-specificity/later-order)
    // declarations override earlier ones, same as every other property.
    let mut content_val: Option<&str> = None;
    for (_, _, decls) in &matched {
        for (p, v) in *decls {
            if p == "content" {
                content_val = Some(v.as_str());
            }
        }
    }
    let text = parse_content_string(content_val?)?;
    let mut s = inherit_reset(own);
    for (_, _, decls) in &matched {
        for (p, v) in *decls {
            if p != "content" {
                apply_one(p, v, theme, &mut s);
            }
        }
    }
    // Layout only knows how to place the pseudo box as an anonymous INLINE
    // text run (see `layout.rs`'s `pseudo()`); `display: none` produces no
    // box. Any other display (`block`, `list-item`, …) is a box shape we
    // don't lay out here — CONFORMANCE.md's forward-compatible rule: produce
    // nothing rather than render it wrong. An explicit `width`/`height` is
    // the same story a level down: `display: inline-block` computes to the
    // same `Display::Inline` this engine uses for plain inline text (no
    // separate inline-block box type), so a sized spacer (the common
    // `content: "…"; display: inline-block; width: N%` idiom some reftest
    // references use as an indent/spacer trick) would otherwise flow as
    // plain unsized text instead of reserving that width — visibly wrong,
    // so skip it too.
    if s.display != Display::Inline || s.width != Len::Auto || s.height != Len::Auto {
        return None;
    }
    Some((text, s))
}

/// Parse a CSS `content` value that is one or more concatenated `<string>`
/// tokens (`"a" 'b'`) into the literal text they produce. Any other component
/// (`attr()`, `counter()`/`counters()`, `open-quote`/`close-quote`/…,
/// `url()`) is out of scope: rather than mis-render, the whole value produces
/// no content — the caller then generates nothing, per CONFORMANCE.md's
/// forward-compatible rule. `none`/`normal` also produce nothing (no box).
///
/// Handles css-syntax-3 §4.3.7 string escapes: `\` + 1-6 hex digits (+ one
/// optional trailing whitespace) is a code point (`\A` = U+000A, the common
/// "forced line break" idiom in generated content); `\` + an actual newline
/// is a line continuation (produces nothing); `\` + any other character is
/// that literal character.
pub fn parse_content_string(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("normal") {
        return None;
    }
    let mut out = String::new();
    let mut chars = v.chars().peekable();
    let mut any = false;
    loop {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        let Some(&q) = chars.peek() else { break };
        if q != '"' && q != '\'' {
            // Anything that isn't a quoted string (`attr(...)`, `counter(...)`,
            // `open-quote`, a bare `url(...)`, …) — unsupported, so the whole
            // value contributes nothing rather than a mangled partial string.
            return None;
        }
        chars.next(); // opening quote
        loop {
            match chars.next() {
                None => return None, // unterminated string → invalid value
                Some(c) if c == q => break,
                Some('\\') => match chars.peek().copied() {
                    None => {}
                    Some(c) if c == '\n' => {
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
                        if let Some(&w) = chars.peek() {
                            if w.is_whitespace() {
                                chars.next(); // one trailing whitespace terminates the escape
                            }
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
        any = true;
    }
    if any { Some(out) } else { None }
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

        // Block containers.
        "html" | "body" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav"
        | "aside" | "figure" | "figcaption" | "form" | "address" | "details" | "summary"
        | "tbody" | "thead" | "tfoot" | "tr" | "fieldset" => {
            s.display = Display::Block;
        }

        // Tables. `<table>` gets the table formatting context; cells are block
        // containers for their own content (`th` also bold). `tr`/`tbody`/… are
        // walked by `layout_table`, so their display is only a fallback.
        "table" => {
            s.display = Display::Table;
            s.margin_top = em * 0.5;
            s.margin_bottom = em * 0.5;
        }
        "td" => s.display = Display::Block,
        "th" => {
            s.display = Display::Block;
            s.bold = true;
        }
        "caption" => {
            s.display = Display::Block;
            s.bold = true;
            s.margin_bottom = em * 0.3;
        }
        "p" => {
            s.display = Display::Block;
            s.margin_top = em * 0.85;
            s.margin_bottom = em * 0.85;
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
        "i" | "em" | "cite" | "var" | "dfn" => s.italic = true,
        "code" | "kbd" | "samp" | "tt" => s.mono = true,
        "small" => s.font_px = em * 0.85,
        "big" => s.font_px = em * 1.15,
        "mark" => s.color = theme.link,
        "br" => s.is_break = true,
        // Superscript / subscript: smaller, raised/lowered off the baseline.
        "sup" => {
            s.font_px = em * 0.75;
            s.valign = 1;
        }
        "sub" => {
            s.font_px = em * 0.75;
            s.valign = -1;
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
fn apply_declarations(decls: &str, theme: &Theme, s: &mut ComputedStyle) {
    for decl in decls.split(';') {
        let mut it = decl.splitn(2, ':');
        let prop = match it.next() {
            Some(p) => p.trim().to_ascii_lowercase(),
            None => continue,
        };
        let val = match it.next() {
            Some(v) => v.trim(),
            None => continue,
        };
        if prop.is_empty() || val.is_empty() {
            continue;
        }
        apply_one(&prop, val, theme, s);
    }
}

/// Apply a single `prop: val` declaration. Shared by inline styles now and by
/// author `<style>` rules later.
pub fn apply_one(prop: &str, val: &str, theme: &Theme, s: &mut ComputedStyle) {
    let v = val.trim().to_ascii_lowercase();
    match prop {
        "display" => {
            s.display = match v.as_str() {
                "none" => Display::None,
                "list-item" => Display::ListItem,
                "inline" | "inline-block" => Display::Inline,
                "table" | "inline-table" => Display::Table,
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
                _ => parse_length(&v, base),
            };
            if let Some(px) = px {
                s.font_px = px.clamp(6.0, 200.0);
            }
        }
        "white-space" => match v.as_str() {
            "pre" | "pre-wrap" | "pre-line" => s.pre = true,
            "normal" | "nowrap" => s.pre = false,
            // `inherit`/`unset`/garbage: an invalid or non-recomputable value
            // drops (CSS Syntax 3 §4), keeping whatever the cascade already
            // set — for `inherit` specifically, that's already the parent's
            // value, since `pre` is copied from `parent` before this runs.
            _ => {}
        },
        "font-family" => {
            s.mono = v.contains("mono") || v.contains("courier") || v.contains("consol");
        }
        // — box model —
        "width" => set_size(&mut s.width, &v, s.font_px),
        "min-width" => set_size(&mut s.min_width, &v, s.font_px),
        "max-width" => set_max(&mut s.max_width, &v, s.font_px),
        "height" => set_size(&mut s.height, &v, s.font_px),
        "min-height" => set_size(&mut s.min_height, &v, s.font_px),
        "max-height" => set_max(&mut s.max_height, &v, s.font_px),
        "box-sizing" => s.box_border = v == "border-box",
        "contain" => s.contain_size = v.split_whitespace().any(|k| k == "size" || k == "strict"),
        "contain-intrinsic-size" => {
            // definite length(s): one → both axes, two → (width, height).
            let mut it = v.split_whitespace().filter_map(|t| parse_length(t, s.font_px));
            if let Some(a) = it.next() {
                s.contain_intrinsic = Some((a, it.next().unwrap_or(a)));
            }
        }
        "margin" => {
            let em = s.font_px;
            let (t, r, b, l) = four_values(&v);
            s.margin_top = margin_tb(t, em);
            s.margin_right = margin_lr(r, em);
            s.margin_bottom = margin_tb(b, em);
            s.margin_left = margin_lr(l, em);
        }
        "margin-top" => s.margin_top = margin_tb(&v, s.font_px),
        "margin-bottom" => s.margin_bottom = margin_tb(&v, s.font_px),
        "margin-left" => s.margin_left = margin_lr(&v, s.font_px),
        "margin-right" => s.margin_right = margin_lr(&v, s.font_px),
        "padding" => {
            let em = s.font_px;
            let (t, r, b, l) = four_values(&v);
            s.pad_top = parse_pad(t, em, 0.0);
            s.pad_right = parse_pad(r, em, 0.0);
            s.pad_bottom = parse_pad(b, em, 0.0);
            s.pad_left = parse_pad(l, em, 0.0);
        }
        "padding-top" => s.pad_top = parse_pad(&v, s.font_px, s.pad_top),
        "padding-right" => s.pad_right = parse_pad(&v, s.font_px, s.pad_right),
        "padding-bottom" => s.pad_bottom = parse_pad(&v, s.font_px, s.pad_bottom),
        "padding-left" => s.pad_left = parse_pad(&v, s.font_px, s.pad_left),

        // — background + border —
        "background-color" | "background" => {
            let vt = v.trim();
            if vt == "none" || vt == "transparent" {
                s.bg = None;
            } else if let Some(c) = parse_color(vt, theme) {
                // Whole value is a colour — handles space-separated function
                // colours like `rgb(0% 50% 0%)` / `hsl(120 100% 25%)`.
                s.bg = Some(c);
            } else {
                // `background` shorthand may carry more than a colour → take the
                // first token that parses as one; gradients/images are ignored.
                for tok in css_tokens(vt) {
                    if let Some(c) = parse_color(tok, theme) {
                        s.bg = Some(c);
                        break;
                    }
                }
            }
        }
        "border" => {
            let side = parse_border_shorthand(&v, s.font_px, theme, s.color);
            s.border_top = side;
            s.border_right = side;
            s.border_bottom = side;
            s.border_left = side;
        }
        "border-top" => s.border_top = parse_border_shorthand(&v, s.font_px, theme, s.color),
        "border-right" => s.border_right = parse_border_shorthand(&v, s.font_px, theme, s.color),
        "border-bottom" => s.border_bottom = parse_border_shorthand(&v, s.font_px, theme, s.color),
        "border-left" => s.border_left = parse_border_shorthand(&v, s.font_px, theme, s.color),
        "border-width" => {
            let em = s.font_px;
            if let Some(t4) = four_sides(&css_tokens(&v)) {
                for (side, tok) in [
                    &mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left,
                ].into_iter().zip(t4) {
                    if let Some(w) = border_width_kw(tok, em) {
                        side.width = w;
                    }
                }
            }
        }
        "border-color" => {
            if let Some(t4) = four_sides(&css_tokens(&v)) {
                for (side, tok) in [
                    &mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left,
                ].into_iter().zip(t4) {
                    if let Some(c) = parse_color(tok, theme) {
                        side.color = Some(c);
                    }
                }
            }
        }
        "border-style" => {
            if let Some(t4) = four_sides(&css_tokens(&v)) {
                for (side, tok) in [
                    &mut s.border_top, &mut s.border_right, &mut s.border_bottom, &mut s.border_left,
                ].into_iter().zip(t4) {
                    if tok == "none" || tok == "hidden" {
                        side.width = 0.0;
                    }
                }
            }
        }
        "border-top-width" => set_side_width(&mut s.border_top, &v, s.font_px),
        "border-right-width" => set_side_width(&mut s.border_right, &v, s.font_px),
        "border-bottom-width" => set_side_width(&mut s.border_bottom, &v, s.font_px),
        "border-left-width" => set_side_width(&mut s.border_left, &v, s.font_px),
        "border-top-color" => s.border_top.color = parse_color(&v, theme).or(s.border_top.color),
        "border-right-color" => s.border_right.color = parse_color(&v, theme).or(s.border_right.color),
        "border-bottom-color" => s.border_bottom.color = parse_color(&v, theme).or(s.border_bottom.color),
        "border-left-color" => s.border_left.color = parse_color(&v, theme).or(s.border_left.color),
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
            if v == "none" || v == "hidden" {
                match prop {
                    "border-top-style" => s.border_top.width = 0.0,
                    "border-right-style" => s.border_right.width = 0.0,
                    "border-bottom-style" => s.border_bottom.width = 0.0,
                    _ => s.border_left.width = 0.0,
                }
            }
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
                        parse_length(t, s.font_px).map(Some)
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
        "top" => s.top = parse_len(&v, s.font_px),
        "right" => s.right = parse_len(&v, s.font_px),
        "bottom" => s.bottom = parse_len(&v, s.font_px),
        "left" => s.left = parse_len(&v, s.font_px),
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
            let row = it.next().and_then(|t| parse_length(t, s.font_px));
            let col = it.next().and_then(|t| parse_length(t, s.font_px)).or(row);
            if let Some(r) = row {
                s.grid_row_gap = r;
                s.gap = r;
            }
            if let Some(c) = col {
                s.grid_col_gap = c;
            }
        }
        "column-gap" => {
            if let Some(g) = parse_length(v.trim(), s.font_px) {
                s.grid_col_gap = g;
                s.gap = g;
            }
        }
        "row-gap" => {
            if let Some(g) = parse_length(v.trim(), s.font_px) {
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
        "flex-basis" => s.flex_basis = parse_basis(&v, s.font_px),
        "order" => {
            if let Ok(o) = v.parse::<i32>() {
                s.order = o;
            }
        }
        "flex" => apply_flex_shorthand(&v, s),

        // — grid —
        "grid-template-columns" => {
            let t = parse_grid_tracks(&v);
            s.grid_ncols = t.n;
            s.grid_tracks = t.tracks;
            s.grid_col_fill = t.fill;
            s.grid_col_fill_start = t.fill_start;
            s.grid_col_fill_len = t.fill_len;
        }
        "grid-template-rows" => {
            let t = parse_grid_tracks(&v);
            s.grid_nrows = t.n;
            s.grid_row_tracks = t.tracks;
        }
        "grid-auto-rows" => s.grid_auto_rows = parse_track(v.trim()),
        "grid-template-areas" => set_grid_areas(s, &v),
        // `grid` / `grid-template` shorthand: `<rows> / <cols>` (areas/flow forms
        // are not supported — they fall through to the row/column split).
        "grid" | "grid-template" => {
            // The shorthand may carry `grid-template-areas` strings.
            if v.contains('"') || v.contains('\'') {
                set_grid_areas(s, &v);
            }
            if let Some((rows, cols)) = split_slash(&v) {
                let r = parse_grid_tracks(rows.trim());
                s.grid_nrows = r.n;
                s.grid_row_tracks = r.tracks;
                let c = parse_grid_tracks(cols.trim());
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
fn parse_len_opt(v: &str, em: f32) -> Option<Len> {
    let v = v.trim();
    if v == "auto" {
        return Some(Len::Auto);
    }
    if v.len() >= 5 && v[..5].eq_ignore_ascii_case("calc(") {
        return parse_calc_affine(v, em);
    }
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().ok().map(Len::Pct);
    }
    parse_length(v, em).map(Len::Px)
}

fn parse_len(v: &str, em: f32) -> Len {
    parse_len_opt(v, em).unwrap_or(Len::Auto)
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
fn set_size(slot: &mut Len, v: &str, em: f32) {
    if let Some(l) = parse_len_opt(v, em).filter(size_non_negative) {
        *slot = l;
    }
}

/// `max-width`/`max-height`: `none` = no maximum (Auto); else a non-negative size.
fn set_max(slot: &mut Len, v: &str, em: f32) {
    if v.trim() == "none" {
        *slot = Len::Auto;
    } else if let Some(l) = parse_len_opt(v, em).filter(size_non_negative) {
        *slot = l;
    }
}

/// Resolve a `calc()` to affine `(pct, px)` form via the full values resolver:
/// evaluate with a %-basis of 0 (→ the px part) and 100 (→ px + pct), so any
/// `%`/px/em mix collapses to `pct% of basis + px`. `rem` approximated by `em`,
/// `vw`/`vh` unavailable here (0) — covers the common `calc(% ± px/em)` forms.
fn parse_calc_affine(v: &str, em: f32) -> Option<Len> {
    let at0 = crate::values::resolve_length(
        v,
        &crate::values::LenCtx { em, rem: em, pct_basis: 0.0, vw: 0.0, vh: 0.0 },
    )?;
    let at100 = crate::values::resolve_length(
        v,
        &crate::values::LenCtx { em, rem: em, pct_basis: 100.0, vw: 0.0, vh: 0.0 },
    )?;
    let pct = at100 - at0;
    if (-0.001..0.001).contains(&pct) {
        Some(Len::Px(at0))
    } else {
        Some(Len::Calc { pct, px: at0 })
    }
}

/// Top/bottom margin: `auto` computes to 0 for block boxes.
fn margin_tb(v: &str, em: f32) -> f32 {
    if v.trim() == "auto" { 0.0 } else { parse_length(v, em).unwrap_or(0.0) }
}

/// Left/right margin keeps `auto` (drives centering / slack).
fn margin_lr(v: &str, em: f32) -> Len {
    parse_len(v, em)
}

/// A padding length. Negative is invalid (padding ≥ 0) → keeps `prior`.
fn parse_pad(v: &str, em: f32, prior: f32) -> f32 {
    match parse_length(v.trim(), em) {
        Some(p) if p >= 0.0 => p,
        _ => prior,
    }
}

/// A border-width keyword/length → px. `thin`/`medium`/`thick` = 1/3/5px.
fn border_width_kw(tok: &str, em: f32) -> Option<f32> {
    match tok.trim() {
        "" => None,
        "thin" => Some(1.0),
        "medium" => Some(3.0),
        "thick" => Some(5.0),
        t => parse_length(t, em).map(|w| w.max(0.0)),
    }
}

/// Assign one side's border width (keeps the prior value on an invalid one).
fn set_side_width(side: &mut BorderSide, v: &str, em: f32) {
    if let Some(w) = border_width_kw(v.trim(), em) {
        side.width = w;
    }
}

/// Parse a `border`/`border-<side>` shorthand (`<width> || <style> || <color>`,
/// any order) into a side. `none`/`hidden` → no border. A specified border with
/// no explicit colour uses `currentColor` (the element's `color`).
fn parse_border_shorthand(v: &str, em: f32, theme: &Theme, current: Rgb) -> BorderSide {
    let (mut width, mut color, mut none, mut styled) = (None, None, false, false);
    for tok in css_tokens(v) {
        match tok {
            "none" | "hidden" => none = true,
            "thin" => {
                width = Some(1.0);
                styled = true;
            }
            "medium" => {
                width = Some(3.0);
                styled = true;
            }
            "thick" => {
                width = Some(5.0);
                styled = true;
            }
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
                styled = true
            }
            _ => {
                if let Some(c) = parse_color(tok, theme) {
                    color = Some(c);
                } else if let Some(w) = parse_length(tok, em) {
                    width = Some(w.max(0.0));
                }
            }
        }
    }
    if none {
        return BorderSide { width: 0.0, color: None };
    }
    // `border-width` initial is 'medium' (3px) once a border is otherwise given.
    let w = width.unwrap_or(if styled || color.is_some() { 3.0 } else { 0.0 });
    let c = color.or(if w > 0.0 { Some(current) } else { None });
    BorderSide { width: w, color: c }
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
fn parse_grid_tracks(v: &str) -> TrackList {
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
            let sub = parse_grid_tracks(parts.next().unwrap_or("").trim());
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
            tracks[n] = parse_track(&tok);
            n += 1;
        }
    }
    TrackList { n: n as u8, tracks, fill, fill_start, fill_len }
}

fn parse_track(t: &str) -> GridTrack {
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
        parse_track(max_part)
    } else {
        // A fixed length: px/rem/em/pt/cm/… (rem/em resolve against the root font).
        match parse_length(t, BASE_FONT_PX) {
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

fn parse_basis(v: &str, em: f32) -> FlexBasis {
    match v {
        "auto" | "content" | "max-content" | "min-content" | "fit-content" => FlexBasis::Auto,
        _ => {
            if let Some(pct) = v.strip_suffix('%') {
                pct.trim().parse::<f32>().map(FlexBasis::Pct).unwrap_or(FlexBasis::Auto)
            } else {
                parse_length(v, em).map(FlexBasis::Px).unwrap_or(FlexBasis::Auto)
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
            basis = Some(parse_basis(tok, s.font_px));
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
fn parse_length(v: &str, em_base: f32) -> Option<f32> {
    let v = v.trim();
    // Font-relative first so "rem" is matched before the "em" suffix eats it.
    // `rem` is ROOT-relative (not em_base) — else nested rem compounds wrongly.
    if let Some(n) = v.strip_suffix("rem") {
        return n.trim().parse::<f32>().ok().map(|f| f * BASE_FONT_PX);
    }
    if let Some(n) = v.strip_suffix("em") {
        return n.trim().parse::<f32>().ok().map(|f| f * em_base);
    }
    if let Some(n) = v.strip_suffix('%') {
        // No containing measure here → treat % of em (rough; refined later).
        return n.trim().parse::<f32>().ok().map(|f| f * em_base / 100.0);
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
    fn head_and_script_are_display_none() {
        let theme = Theme::DARK;
        let root = ComputedStyle::root(&theme);
        for tag in ["head", "script", "style", "title"] {
            let html = alloc::format!("<{tag}>x</{tag}>");
            let dom = dom::parse(&html);
            if let dom::Node::Element(e) = &dom.root.children[0] {
                let st = resolve(e, &root, &theme, &Stylesheet::empty(), &[], &[], 0, 1000.0);
                assert_eq!(st.display, Display::None, "{tag}");
            }
        }
    }
}
