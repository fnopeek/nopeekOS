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
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub padding_left: f32,
    pub is_link: bool,
    pub is_rule: bool, // <hr> — painted as a divider
    pub is_break: bool, // <br> — forced line break in inline flow
    // — flex container —
    pub flex_row: bool, // flex-direction: row (true) vs column (false)
    pub flex_wrap: bool,
    pub justify: Justify,
    pub align_items: CrossAlign,
    pub gap: f32,
    // — flex item —
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: FlexBasis,
    pub align_self: Option<CrossAlign>,
    pub order: i32,
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
            margin_top: 0.0,
            margin_bottom: 0.0,
            padding_left: 0.0,
            is_link: false,
            is_rule: false,
            is_break: false,
            flex_row: true,
            flex_wrap: false,
            justify: Justify::Start,
            align_items: CrossAlign::Stretch,
            gap: 0.0,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: FlexBasis::Auto,
            align_self: None,
            order: 0,
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
        margin_top: 0.0,
        margin_bottom: 0.0,
        padding_left: 0.0,
        is_link: false,
        is_rule: false,
        is_break: false,
        flex_row: true,
        flex_wrap: false,
        justify: Justify::Start,
        align_items: CrossAlign::Stretch,
        gap: 0.0,
        flex_grow: 0.0,
        flex_shrink: 1.0,
        flex_basis: FlexBasis::Auto,
        align_self: None,
        order: 0,
    };
    ua_rule(&el.tag, parent, theme, &mut s);

    // Author `<style>` rules, applied low→high specificity (ties: doc order).
    if !sheet.is_empty() {
        let info = ElemInfo::of(el);
        let mut matched = sheet.matched(&info, ancestors);
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
            s.padding_left = 26.0;
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
            s.padding_left = 26.0;
        }

        "blockquote" => {
            s.display = Display::Block;
            s.padding_left = 24.0;
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
                // block / grid fall back to block flow for now
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
        "margin-top" => s.margin_top = parse_length(&v, s.font_px).unwrap_or(s.margin_top),
        "margin-bottom" => s.margin_bottom = parse_length(&v, s.font_px).unwrap_or(s.margin_bottom),
        "padding-left" | "margin-left" => {
            s.padding_left = parse_length(&v, s.font_px).unwrap_or(s.padding_left)
        }

        // — flex —
        "flex-direction" => s.flex_row = !v.starts_with("column"),
        "flex-wrap" => s.flex_wrap = v.starts_with("wrap"),
        "flex-flow" => {
            for tok in v.split_whitespace() {
                match tok {
                    "row" | "row-reverse" => s.flex_row = true,
                    "column" | "column-reverse" => s.flex_row = false,
                    "wrap" | "wrap-reverse" => s.flex_wrap = true,
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
        "gap" | "column-gap" | "row-gap" | "grid-gap" => {
            let first = v.split_whitespace().next().unwrap_or(&v);
            if let Some(g) = parse_length(first, s.font_px) {
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

        _ => {}
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

/// Parse a CSS `<color>`: `#rgb`, `#rrggbb`, and the common named colours.
/// `currentcolor`/`inherit` fall through to `None` (keep the inherited value).
fn parse_color(v: &str, _theme: &Theme) -> Option<Rgb> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                Some(Rgb(r * 17, g * 17, b * 17))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Rgb(r, g, b))
            }
            _ => None,
        };
    }
    Some(match v {
        "black" => Rgb(0, 0, 0),
        "white" => Rgb(255, 255, 255),
        "red" => Rgb(220, 38, 38),
        "green" => Rgb(22, 163, 74),
        "blue" => Rgb(37, 99, 235),
        "gray" | "grey" => Rgb(128, 128, 128),
        "silver" => Rgb(192, 192, 192),
        "orange" => Rgb(234, 88, 12),
        "yellow" => Rgb(202, 138, 4),
        "purple" => Rgb(147, 51, 234),
        "navy" => Rgb(30, 58, 138),
        "teal" => Rgb(13, 148, 136),
        "maroon" => Rgb(153, 27, 27),
        "currentcolor" | "inherit" => return None,
        _ => return None,
    })
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
        let st = resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[]);
        assert_eq!(st.display, Display::Block);
        assert!(st.bold);
        assert!(st.font_px > BASE_FONT_PX * 1.5);
        assert_eq!(st.color, theme.heading);
    }

    #[test]
    fn inline_style_attribute_is_parsed() {
        let dom = dom::parse("<body><p style=\"color:#ff0000; font-weight:bold; font-size:20px\">x</p></body>");
        let theme = Theme::DARK;
        let st = resolve(first_el(&dom), &ComputedStyle::root(&theme), &theme, &Stylesheet::empty(), &[]);
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
        let a = resolve(first_el(&dom), &root, &theme, &sheet, &[]);
        assert_eq!(a.color, Rgb(0, 255, 0));
        assert!(a.bold);
        // 2nd <p>: author red+bold, no inline.
        let p2 = match &dom.body().children[1] {
            dom::Node::Element(e) => e,
            _ => panic!(),
        };
        let b = resolve(p2, &root, &theme, &sheet, &[]);
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
                let st = resolve(e, &root, &theme, &Stylesheet::empty(), &[]);
                assert_eq!(st.display, Display::None, "{tag}");
            }
        }
    }
}
