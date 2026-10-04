//! CSS custom properties (`--name`) and `var()` (css-variables-1).
//!
//! They are resolved in the cascade, per element: `style::resolve_in`
//! collects them from the rules that actually match the element, inherits
//! them from the parent and substitutes them when a value is applied. This
//! module holds the per-element map and the substitution of a value.
//!
//! A single document-wide map is not enough: frameworks set a variable in a
//! base class and override it in each variant, so only the rules that match
//! the element may decide its value.

use alloc::string::{String, ToString};

fn parse_var_args(input: &str, open: usize) -> Option<(usize, String, Option<String>)> {
    let b = input.as_bytes();
    let n = b.len();
    let first = open + 1;
    let mut i = first;
    let mut depth: i32 = 0;
    let mut comma: Option<usize> = None;
    let mut close: Option<usize> = None;
    while i < n {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            i = skip_string(b, i);
            continue;
        }
        match c {
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    close = Some(i);
                    break;
                }
                depth -= 1;
            }
            b',' if depth == 0 && comma.is_none() => comma = Some(i),
            _ => {}
        }
        i += 1;
    }
    let close = close?;
    let (name_end, fallback) = match comma {
        Some(cp) => (cp, Some(input[cp + 1..close].trim().to_string())),
        None => (close, None),
    };
    let name = input[first..name_end].trim();
    if !name.starts_with("--") {
        return None;
    }
    Some((close + 1, name.to_string(), fallback))
}

// ── low-level helpers ───────────────────────────────────────────────────────

/// `true` if bytes at `i` spell `var(` (case-insensitive on `var`).
///
/// The preceding byte matters: a function name is a whole identifier, so
/// `notvar(--x)` is the function `notvar`, not `var()`.
fn is_var_at(b: &[u8], i: usize) -> bool {
    if i > 0 {
        let p = b[i - 1];
        if p.is_ascii_alphanumeric() || p == b'_' || p == b'-' || p >= 0x80 { return false }
    }
    i + 4 <= b.len()
        && (b[i] | 0x20) == b'v'
        && (b[i + 1] | 0x20) == b'a'
        && (b[i + 2] | 0x20) == b'r'
        && b[i + 3] == b'('
}

/// Cheap scan: does the text contain a `var(` anywhere?
fn contains_var(b: &[u8]) -> bool {
    if b.len() < 4 {
        return false;
    }
    let mut i = 0;
    while i + 4 <= b.len() {
        if is_var_at(b, i) {
            return true;
        }
        i += 1;
    }
    false
}

/// Index one past a `/* … */` comment that starts at `i`.
fn skip_comment(b: &[u8], i: usize) -> usize {
    let n = b.len();
    let mut k = i + 2;
    while k + 1 < n && !(b[k] == b'*' && b[k + 1] == b'/') {
        k += 1;
    }
    // Advance past the closing `*/` (or to end if unterminated).
    if k + 1 < n {
        k + 2
    } else {
        n
    }
}

/// Index one past a `"…"` or `'…'` string that starts at `i` (with `\` escapes).
fn skip_string(b: &[u8], i: usize) -> usize {
    let n = b.len();
    let q = b[i];
    let mut k = i + 1;
    while k < n {
        if b[k] == b'\\' {
            k += 2;
            continue;
        }
        if b[k] == q {
            return k + 1;
        }
        k += 1;
    }
    n
}

/// CSS ident byte (ASCII alnum, `-`, `_`, or any non-ASCII / UTF-8 byte).
fn is_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

// ── Per element, not per document ───────────────────────────────────────────
//
// A custom property is an inherited property, so it belongs in the cascade
// (`style::resolve_in`). This part holds the map and the substitution.

/// The custom properties in effect on an element.
///
/// A flat list rather than a hash map: an element rarely carries more than a
/// few dozen, and a linear compare over short names is cheaper than hashing.
pub type VarMap = alloc::vec::Vec<(alloc::rc::Rc<str>, alloc::rc::Rc<str>)>;

/// Does this value contain a `var()`? A byte scan, so the common case (no
/// `var()`) costs nothing.
pub fn has_var(v: &str) -> bool { contains_var(v.as_bytes()) }

pub fn var_get<'a>(map: &'a VarMap, name: &str) -> Option<&'a str> {
    let v = map.iter().find(|(k, _)| &**k == name).map(|(_, v)| &**v)?;
    // `--x: initial` makes the property guaranteed-invalid (css-variables-1
    // §3.1): it counts as unset, and `var(--x, fallback)` takes the fallback.
    // Frameworks use this to switch values.
    if v.trim() == "initial" { return None }
    Some(v)
}

/// Set or replace. Replacing keeps the list from growing with every
/// overridden declaration.
pub fn var_set(map: &mut VarMap, name: &str, value: &str) {
    match map.iter_mut().find(|(k, _)| &**k == name) {
        Some(slot) => slot.1 = alloc::rc::Rc::from(value),
        None => map.push((alloc::rc::Rc::from(name), alloc::rc::Rc::from(value))),
    }
}

/// Substitute `var()` in a value against this element's map.
///
/// `skip` is the name whose own value is being computed: it must not
/// substitute itself. `--x: var(--x, 1rem)` means "the inherited value, else
/// 1rem"; finding itself would leave a `var()` behind and invalidate the
/// declaration.
pub fn expand(value: &str, map: &VarMap, skip: Option<&str>) -> String {
    if !contains_var(value.as_bytes()) {
        return value.into();
    }
    let mut cur = expand_pass(value, map, skip);
    for _ in 1..MAX_PASSES {
        if !contains_var(cur.0.as_bytes()) || !cur.1 {
            break;
        }
        cur = expand_pass(&cur.0, map, skip);
    }
    cur.0
}

/// Cycle cap: `--a: var(--b); --b: var(--a)` never terminates on its own.
/// Anything still carrying a `var()` afterwards is invalid, which is the
/// correct answer.
const MAX_PASSES: usize = 16;

fn expand_pass(input: &str, map: &VarMap, skip: Option<&str>) -> (String, bool) {
    let b = input.as_bytes();
    let mut out = String::with_capacity(input.len() + 16);
    let mut i = 0;
    let mut changed = false;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let j = skip_comment(b, i);
            out.push_str(&input[i..j]);
            i = j;
            continue;
        }
        if b[i] == b'"' || b[i] == b'\'' {
            let j = skip_string(b, i);
            out.push_str(&input[i..j]);
            i = j;
            continue;
        }
        if is_var_at(b, i) {
            if let Some((end, name, fallback)) = parse_var_args(input, i + 3) {
                let hit = if skip == Some(name.as_str()) { None } else { var_get(map, &name) };
                match (hit, fallback) {
                    (Some(v), _) => out.push_str(v),
                    (None, Some(f)) => out.push_str(&f),
                    // No value and no fallback: the `var()` stays, the value parser fails
                    // on it, and the declaration is dropped (css-variables-1 §3).
                    (None, None) => { out.push_str(&input[i..end]); i = end; continue }
                }
                changed = true;
                i = end;
                continue;
            }
        }
        let ch_len = input[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    (out, changed)
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::DrawOp;

    fn map(pairs: &[(&str, &str)]) -> VarMap {
        pairs.iter().map(|(k, v)| (alloc::rc::Rc::from(*k), alloc::rc::Rc::from(*v))).collect()
    }

    /// The colour a page paints on its first text run.
    ///
    /// Checks a cascaded value end to end: parser, matching, cascade,
    /// inheritance and substitution. A probe on `expand` alone says nothing
    /// about the cascade.
    fn painted_text(html: &str, css: &str) -> Option<(u8, u8, u8)> {
        let mut eng = crate::Engine::new();
        let lay = eng.layout_ext(html, css, 800);
        lay.ops.iter().find_map(|o| match o {
            DrawOp::Text { color, .. } => Some((color.c.0, color.c.1, color.c.2)),
            _ => None,
        })
    }

    /// All fill colours of a page. A list rather than "the first": which fill is
    /// the canvas and which the element depends on page structure, and the probe
    /// checks the value, not the paint order.
    fn fills(html: &str, css: &str) -> alloc::vec::Vec<(u8, u8, u8)> {
        let mut eng = crate::Engine::new();
        let lay = eng.layout_ext(html, css, 800);
        lay.ops.iter().filter_map(|o| match o {
            DrawOp::Rect { color, .. } | DrawOp::RoundRect { color, .. } =>
                Some((color.c.0, color.c.1, color.c.2)),
            _ => None,
        }).collect()
    }

    /// A custom property is substituted on use, not when it is set
    /// (css-variables-1 §3). Otherwise the first rule would freeze the cascade
    /// state, and a later rule that sets a used variable would come too late.
    #[test]
    fn a_variable_may_be_set_after_the_one_that_uses_it() {
        let c = painted_text(
            "<p class=\"a b\">x</p>",
            ".a { --outer: var(--inner, #00ff00); color: var(--outer) } .b { --inner: #ff0000 }",
        );
        assert_eq!(c, Some((255, 0, 0)), "die spaetere Regel entscheidet, nicht der Ausweichwert");
    }

    // ── The cascade: who decides the value ──────────────────────────────────

    /// A rule that does not match the element must not supply its variables,
    /// even if it comes last in the sheet.
    #[test]
    fn a_rule_that_does_not_match_must_not_decide_the_value() {
        let css = ".a{--x:#00f;color:var(--x)} .b{--x:#f00} .c{--x:#0f0}";
        assert_eq!(painted_text("<p class='a b'>x</p>", css), Some((255, 0, 0)));
    }

    /// Conversely: the element's own rule wins over a same-named one elsewhere.
    #[test]
    fn the_element_own_declaration_wins_over_a_later_foreign_one() {
        let css = ".btn{--bg:transparent;background-color:var(--bg)}\
                   .btn-primary{--bg:#0d6efd}\
                   .btn-link{--bg:transparent}";
        let f = fills("<p class='btn btn-primary'>x</p>", css);
        assert!(f.contains(&(13, 110, 253)), "die Fuellung des Elements fehlt: {f:?}");
    }

    /// A custom property is inherited.
    #[test]
    fn a_custom_property_inherits_to_descendants() {
        let css = ".wrap{--c:#0f0} .deep{color:var(--c)}";
        assert_eq!(painted_text("<div class='wrap'><div><p class='deep'>x</p></div></div>", css),
                   Some((0, 255, 0)));
    }

    /// A descendant may override it without affecting the ancestor.
    #[test]
    fn a_descendant_may_shadow_an_inherited_value() {
        let css = ".wrap{--c:#f00} .inner{--c:#00f} .t{color:var(--c)}";
        assert_eq!(painted_text("<div class='wrap'><div class='inner'><p class='t'>x</p></div></div>", css),
                   Some((0, 0, 255)));
    }

    #[test]
    fn root_palette_reaches_everything() {
        assert_eq!(painted_text("<p>x</p>", ":root{--c:#f00} p{color:var(--c)}"), Some((255, 0, 0)));
    }

    /// A comment before a rule is not part of its selector.
    #[test]
    fn a_theme_block_that_matches_nothing_stays_out() {
        let css = "/*! Thing v5.3.3 (https://example.com/) */\
                   :root,[data-t=light]{--bg:#fff}\
                   [data-t=dark]{--bg:#000}\
                   p{color:var(--bg)}";
        assert_eq!(painted_text("<p>x</p>", css), Some((255, 255, 255)));
    }

    /// When the attribute is present, the dark block wins; otherwise the rule
    /// above would only test "never".
    #[test]
    fn the_same_theme_block_wins_when_it_does_match() {
        let css = ":root,[data-t=light]{--bg:#fff} [data-t=dark]{--bg:#000} p{color:var(--bg)}";
        assert_eq!(painted_text("<html data-t='dark'><body><p>x</p></body></html>", css),
                   Some((0, 0, 0)));
    }

    /// Of several definitions keyed on a document attribute, only the one that
    /// matches may win.
    #[test]
    fn only_the_class_the_root_carries_counts() {
        let css = "html.pref-1{--s:#f00} html.pref-2{--s:#0f0} p{color:var(--s)}";
        assert_eq!(painted_text("<html class='pref-1'><body><p>x</p></body></html>", css),
                   Some((255, 0, 0)));
    }

    /// Specificity beats order, as for any other property.
    #[test]
    fn specificity_decides_before_order() {
        let css = "#id{--c:#f00} .cls{--c:#0f0} p{color:var(--c)}";
        assert_eq!(painted_text("<p id='id' class='cls'>x</p>", css), Some((255, 0, 0)));
    }

    /// An `@media` that does not apply supplies no variables.
    #[test]
    fn a_media_block_that_does_not_apply_contributes_nothing() {
        let css = ":root{--c:#f00} @media (max-width:480px){:root{--c:#0f0}} p{color:var(--c)}";
        assert_eq!(painted_text("<p>x</p>", css), Some((255, 0, 0)));
    }

    // ── Substitution: what a value becomes ──────────────────────────────────

    #[test]
    fn simple_substitution() {
        assert_eq!(expand("var(--c)", &map(&[("--c", "#f00")]), None), "#f00");
    }

    #[test]
    fn fallback_used_when_undefined() {
        assert_eq!(expand("var(--missing, blue)", &map(&[]), None), "blue");
    }

    #[test]
    fn fallback_ignored_when_defined() {
        assert_eq!(expand("var(--c, blue)", &map(&[("--c", "green")]), None), "green");
    }

    /// No value and no fallback: the `var()` stays, the value parser fails on
    /// it, and the declaration is dropped, as css-variables-1 §3 requires. An
    /// empty value would be something else.
    #[test]
    fn undefined_without_fallback_stays_and_invalidates() {
        assert_eq!(expand("var(--nope)", &map(&[]), None), "var(--nope)");
    }

    #[test]
    fn nested_var_in_value() {
        assert_eq!(expand("var(--a)", &map(&[("--a", "var(--b)"), ("--b", "#0f0")]), None), "#0f0");
    }

    #[test]
    fn nested_var_in_fallback() {
        assert_eq!(expand("var(--x, calc(1px + var(--y, 2px)))", &map(&[]), None),
                   "calc(1px + 2px)");
    }

    #[test]
    fn fallback_with_commas_and_parens() {
        assert_eq!(expand("0 0 0 var(--x, rgba(0,0,0,.1))", &map(&[]), None),
                   "0 0 0 rgba(0,0,0,.1)");
    }

    #[test]
    fn whitespace_variations() {
        assert_eq!(expand("var( --c )", &map(&[("--c", "#abc")]), None), "#abc");
        assert_eq!(expand("var(  --missing ,  blue  )", &map(&[]), None), "blue");
    }

    #[test]
    fn uppercase_var_function() {
        assert_eq!(expand("VAR(--c)", &map(&[("--c", "#0f0")]), None), "#0f0");
    }

    #[test]
    fn var_inside_string_is_not_expanded() {
        assert_eq!(expand(r#""var(--c)""#, &map(&[("--c", "red")]), None), r#""var(--c)""#);
    }

    #[test]
    fn multiple_uses_in_one_value() {
        assert_eq!(expand("var(--c) var(--c)", &map(&[("--c", "1px")]), None), "1px 1px");
    }

    /// `--fs: var(--fs, 1rem)` means "the inherited value, else 1rem". If it
    /// found itself, a `var()` would remain and the declaration would be invalid.
    #[test]
    fn a_self_referential_declaration_takes_the_fallback() {
        assert_eq!(expand("var(--fs,1rem)", &map(&[]), Some("--fs")), "1rem");
    }

    /// A cycle of two names does not loop forever.
    #[test]
    fn a_cycle_terminates() {
        let m = map(&[("--a", "var(--b)"), ("--b", "var(--a)")]);
        let out = expand("var(--a)", &m, None);
        assert!(out.contains("var("), "der Ring haette einen Wert liefern muessen? {out}");
    }

    #[test]
    fn a_deep_chain_is_not_a_cycle() {
        let m = map(&[("--a", "var(--b)"), ("--b", "var(--c)"), ("--c", "#0f0")]);
        assert_eq!(expand("var(--a)", &m, None), "#0f0");
    }

    /// A palette passed on through nested variables.
    #[test]
    fn bootstrap_like_chain() {
        let m = map(&[("--bs-blue", "#0d6efd"), ("--bs-primary", "var(--bs-blue)")]);
        assert_eq!(expand("var(--bs-primary)", &m, None), "#0d6efd");
        assert_eq!(expand("var(--bs-primary, #ccc)", &m, None), "#0d6efd");
    }

    #[test]
    fn a_value_without_var_is_returned_unchanged() {
        assert_eq!(expand("1px solid red", &map(&[("--c", "x")]), None), "1px solid red");
    }
}
