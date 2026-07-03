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

use crate::dom::Element;
use crate::layout::{Rgb, Theme};

/// CSS `display` — only the values our layout implements so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    None,
    Block,
    Inline,
    ListItem,
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
        }
    }
}

pub const BASE_FONT_PX: f32 = 16.0;

/// Resolve an element's computed style: inherit from `parent`, apply the UA
/// rule for its tag, then any inline `style="…"`.
pub fn resolve(el: &Element, parent: &ComputedStyle, theme: &Theme) -> ComputedStyle {
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
    };
    ua_rule(&el.tag, parent, theme, &mut s);
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
        | "table" | "tbody" | "thead" | "tr" | "fieldset" => {
            s.display = Display::Block;
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
                // block / flex / grid / table all fall back to block flow for now
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
        _ => {}
    }
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
    use crate::dom;

    fn only_el(html: &str) -> dom::Dom {
        dom::parse(html)
    }

    #[test]
    fn ua_sheet_gives_headings_size_weight_and_colour() {
        let dom = only_el("<body><h1>x</h1></body>");
        let theme = Theme::DARK;
        let root = ComputedStyle::root(&theme);
        let body = dom.body();
        let h1 = match &body.children[0] {
            dom::Node::Element(e) => e,
            _ => panic!(),
        };
        let st = resolve(h1, &root, &theme);
        assert_eq!(st.display, Display::Block);
        assert!(st.bold);
        assert!(st.font_px > BASE_FONT_PX * 1.5);
        assert_eq!(st.color, theme.heading);
    }

    #[test]
    fn inline_style_attribute_is_parsed() {
        let dom = only_el("<body><p style=\"color:#ff0000; font-weight:bold; font-size:20px\">x</p></body>");
        let theme = Theme::DARK;
        let root = ComputedStyle::root(&theme);
        let p = match &dom.body().children[0] {
            dom::Node::Element(e) => e,
            _ => panic!(),
        };
        let st = resolve(p, &root, &theme);
        assert_eq!(st.color, Rgb(255, 0, 0));
        assert!(st.bold);
        assert_eq!(st.font_px, 20.0);
    }

    #[test]
    fn head_and_script_are_display_none() {
        let theme = Theme::DARK;
        let root = ComputedStyle::root(&theme);
        for tag in ["head", "script", "style", "title"] {
            let html = alloc::format!("<{tag}>x</{tag}>");
            let dom = dom::parse(&html);
            if let dom::Node::Element(e) = &dom.root.children[0] {
                assert_eq!(resolve(e, &root, &theme).display, Display::None, "{tag}");
            }
        }
    }
}
