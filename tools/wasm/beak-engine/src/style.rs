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

use crate::css::{ElemInfo, Stylesheet};
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

/// The subset of computed properties the renderer consumes. Split by CSS
/// inheritance: font/colour/`white-space` inherit; box/`display` do not.
#[derive(Clone, Copy)]
pub struct ComputedStyle {
    // — inherited —
    pub font_px: f32,
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
    pub bg: Option<Rgb>, // background-color (None = transparent)
    pub border_color: Option<Rgb>,
    pub border_width: f32, // uniform border (all sides)
    // — positioning —
    pub position: Position,
    pub top: Len,
    pub right: Len,
    pub bottom: Len,
    pub left: Len,
    pub is_link: bool,
    pub is_rule: bool, // <hr> — painted as a divider
    pub is_break: bool, // <br> — forced line break in inline flow
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
    pub justify_self: Option<CrossAlign>,
}

impl ComputedStyle {
    /// The initial style for the document root, seeded from the theme.
    pub fn root(theme: &Theme) -> ComputedStyle {
        ComputedStyle {
            font_px: BASE_FONT_PX,
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
            bg: None,
            border_color: None,
            border_width: 0.0,
            position: Position::Static,
            top: Len::Auto,
            right: Len::Auto,
            bottom: Len::Auto,
            left: Len::Auto,
            is_link: false,
            is_rule: false,
            is_break: false,
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
            grid_col_fill: 0,
            grid_col_fill_start: 0,
            grid_col_fill_len: 0,
            grid_col_span: 1,
            grid_col_start: 0,
            grid_row_start: 0,
            grid_row_span: 1,
            justify_self: None,
        }
    }
}

pub const BASE_FONT_PX: f32 = 16.0;

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
    viewport_w: f32,
) -> ComputedStyle {
    // Start from the inherited slice; reset the non-inherited slice to initial.
    let mut s = ComputedStyle {
        font_px: parent.font_px,
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
        bg: None,
        border_color: None,
        border_width: 0.0,
        position: Position::Static,
        top: Len::Auto,
        right: Len::Auto,
        bottom: Len::Auto,
        left: Len::Auto,
        is_link: false,
        is_rule: false,
        is_break: false,
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
        grid_col_fill: 0,
        grid_col_fill_start: 0,
        grid_col_fill_len: 0,
        grid_col_span: 1,
        grid_col_start: 0,
        grid_row_start: 0,
        grid_row_span: 1,
        justify_self: None,
    };
    ua_rule(&el.tag, parent, theme, &mut s);

    // Author `<style>` rules, applied low→high specificity (ties: doc order).
    if !sheet.is_empty() {
        let info = ElemInfo::of(el);
        let mut matched = sheet.matched(&info, ancestors, viewport_w);
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
    s
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
        // span / label / abbr / time / sup / sub / u / s / … → plain inline.
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
                "flex" | "inline-flex" => Display::Flex,
                "grid" | "inline-grid" | "grid-lanes" | "inline-grid-lanes" => Display::Grid,
                _ => Display::Block,
            };
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
            if let Some(px) = parse_length(&v, s.font_px) {
                s.font_px = px.clamp(6.0, 200.0);
            }
        }
        "white-space" => {
            s.pre = matches!(v.as_str(), "pre" | "pre-wrap" | "pre-line");
        }
        "font-family" => {
            s.mono = v.contains("mono") || v.contains("courier") || v.contains("consol");
        }
        // — box model —
        "width" => s.width = parse_len(&v, s.font_px),
        "min-width" => s.min_width = parse_len(&v, s.font_px),
        "max-width" => s.max_width = if v == "none" { Len::Auto } else { parse_len(&v, s.font_px) },
        "height" => s.height = parse_len(&v, s.font_px),
        "min-height" => s.min_height = parse_len(&v, s.font_px),
        "max-height" => s.max_height = if v == "none" { Len::Auto } else { parse_len(&v, s.font_px) },
        "box-sizing" => s.box_border = v == "border-box",
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
            s.pad_top = parse_length(t, em).unwrap_or(0.0);
            s.pad_right = parse_length(r, em).unwrap_or(0.0);
            s.pad_bottom = parse_length(b, em).unwrap_or(0.0);
            s.pad_left = parse_length(l, em).unwrap_or(0.0);
        }
        "padding-top" => s.pad_top = parse_length(&v, s.font_px).unwrap_or(s.pad_top),
        "padding-right" => s.pad_right = parse_length(&v, s.font_px).unwrap_or(s.pad_right),
        "padding-bottom" => s.pad_bottom = parse_length(&v, s.font_px).unwrap_or(s.pad_bottom),
        "padding-left" => s.pad_left = parse_length(&v, s.font_px).unwrap_or(s.pad_left),

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
        "border" | "border-top" | "border-bottom" | "border-left" | "border-right" => {
            if v == "none" || v == "0" || v.starts_with("0 ") {
                s.border_color = None;
                s.border_width = 0.0;
            } else {
                for tok in css_tokens(&v) {
                    if let Some(c) = parse_color(tok, theme) {
                        s.border_color = Some(c);
                    } else if let Some(bw) = parse_length(tok, s.font_px) {
                        s.border_width = bw;
                    }
                }
                if s.border_color.is_some() && s.border_width == 0.0 {
                    s.border_width = 1.0; // `medium` default when only colour given
                }
            }
        }
        "border-color" => s.border_color = parse_color(&v, theme),
        "border-width" => s.border_width = parse_length(&v, s.font_px).unwrap_or(s.border_width),

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
        "top" => s.top = parse_len(&v, s.font_px),
        "right" => s.right = parse_len(&v, s.font_px),
        "bottom" => s.bottom = parse_len(&v, s.font_px),
        "left" => s.left = parse_len(&v, s.font_px),

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
        // `grid` / `grid-template` shorthand: `<rows> / <cols>` (areas/flow forms
        // are not supported — they fall through to the row/column split).
        "grid" | "grid-template" => {
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

/// A `<length>`/`auto`/`%` value for the box model.
fn parse_len(v: &str, em: f32) -> Len {
    let v = v.trim();
    if v == "auto" {
        return Len::Auto;
    }
    if v.len() >= 5 && v[..5].eq_ignore_ascii_case("calc(") {
        if let Some(l) = parse_calc_affine(v, em) {
            return l;
        }
    }
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().map(Len::Pct).unwrap_or(Len::Auto);
    }
    parse_length(v, em).map(Len::Px).unwrap_or(Len::Auto)
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
    } else if t.starts_with("minmax(") {
        if t.contains("fr") { GridTrack::Fr(1.0) } else { GridTrack::Auto }
    } else if let Some(px) = t.strip_suffix("px") {
        GridTrack::Fixed(px.trim().parse().unwrap_or(0.0))
    } else {
        t.parse::<f32>().map(GridTrack::Fixed).unwrap_or(GridTrack::Auto)
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
    if let Some(n) = v.strip_suffix("px") {
        n.trim().parse::<f32>().ok()
    } else if let Some(n) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        n.trim().parse::<f32>().ok().map(|f| f * em_base)
    } else if let Some(n) = v.strip_suffix('%') {
        // No containing measure here → treat % of em (rough; refined later).
        n.trim().parse::<f32>().ok().map(|f| f * em_base / 100.0)
    } else {
        v.parse::<f32>().ok()
    }
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
        let st = resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[], 1000.0);
        assert_eq!(st.display, Display::Block);
        assert!(st.bold);
        assert!(st.font_px > BASE_FONT_PX * 1.5);
        assert_eq!(st.color, theme.heading);
    }

    #[test]
    fn inline_style_attribute_is_parsed() {
        let dom = dom::parse("<body><p style=\"color:#ff0000; font-weight:bold; font-size:20px\">x</p></body>");
        let theme = Theme::DARK;
        let st = resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[], 1000.0);
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
        let a = resolve(first_el(&dom), &root, &theme, &sheet, &[], 1000.0);
        assert_eq!(a.color, Rgb(0, 255, 0));
        assert!(a.bold);
        // 2nd <p>: author red+bold, no inline.
        let p2 = match &dom.body().children[1] {
            dom::Node::Element(e) => e,
            _ => panic!(),
        };
        let b = resolve(p2, &root, &theme, &sheet, &[], 1000.0);
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
                let st = resolve(e, &root, &theme, &Stylesheet::empty(), &[], 1000.0);
                assert_eq!(st.display, Display::None, "{tag}");
            }
        }
    }
}
