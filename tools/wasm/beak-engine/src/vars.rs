//! vars.rs — CSS custom properties (`--name`) + `var()` resolution.
//!
//! Modern sites (esp. Bootstrap 5's `--bs-*`) define custom properties and
//! consume them via `var(--name, fallback)`. This module resolves them as a
//! pre-pass over the raw stylesheet text (before `css::parse`), so the rest of
//! the engine never has to know about variables. Host-testable, no OS.
//!
//! Pipeline: [collect] all `--name: value` declarations document-wide into a
//! map (last wins) → [expand] every `var(...)` to a fixpoint. The rewritten
//! CSS is plain and feeds the existing `css::parse` unchanged. The `--name:
//! value;` declarations are left in place — the downstream parser ignores
//! unknown `--` properties, so they are harmless.
//!
//! `@media` IS honoured: a block that doesn't hold at the current viewport is
//! skipped wholesale, so its declarations never enter the map. The cascade
//! already skipped such *rules*; collecting their *values* anyway let a
//! `prefers-color-scheme:dark` palette win over `:root`.
//!
//! SCOPING LIMITATION (v1): declarations are collected document-wide, not
//! per-selector. A `--x` set inside `.foo{}` is treated as visible everywhere,
//! and if the same name is declared twice the *later* declaration wins for the
//! whole document. This is intentionally coarse; it covers the overwhelmingly
//! common case (all of Bootstrap defines its palette on `:root{…}`). True
//! cascade-scoped custom properties are out of scope.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Expand every `var(--name, fallback)` in a stylesheet using its `--name:
/// value;` custom-property declarations, and return the rewritten CSS (which
/// then feeds the existing `css::parse`). Handles nested `var()`, fallbacks,
/// missing variables, and whitespace variations. Document-scoped (see module
/// note). Cheap fast-path: returns the input unchanged when it has no `var(`.
pub fn resolve_vars(css: &str, viewport_w: f32, root_classes: &[&str]) -> String {
    // Fast path: nothing to do. Also covers the common case of a sheet with no
    // custom properties at all.
    if !contains_var(css.as_bytes()) {
        return css.to_string();
    }

    let mut map: BTreeMap<String, String> = BTreeMap::new();
    // Names whose winning value came from an *unconditional* (`:root`/`html`/
    // `body`/`*`) block — a later *conditional* block (a theme/state class such
    // as `html.skin-theme-clientpref-night`) must not overwrite these.
    let mut uncond: BTreeSet<String> = BTreeSet::new();
    collect(css, &mut map, &mut uncond, viewport_w, root_classes);

    // Expand to a fixpoint. A variable's value (or a fallback) may itself hold
    // more `var()`; each pass resolves one layer, so we loop until stable.
    // Capped at MAX_PASSES to bound cyclic references (e.g. --a: var(--b);
    // --b: var(--a)) — they simply stop mutating and any residual var() is
    // left in place rather than looping forever.
    const MAX_PASSES: usize = 16;
    let (mut cur, _) = expand_once(css, &map);
    for _ in 1..MAX_PASSES {
        if !contains_var(cur.as_bytes()) {
            break;
        }
        let (next, changed) = expand_once(&cur, &map);
        if !changed {
            break;
        }
        cur = next;
    }
    cur
}

// ── collection ──────────────────────────────────────────────────────────────

/// Scan the whole stylesheet for `--name: value` declarations and record them
/// (later declaration overwrites earlier). Strings and `/* comments */` are
/// skipped so a `--x:` inside them is never mistaken for a declaration. A
/// `--name` that is *used* (`var(--name)`) is not a declaration because it is
/// not followed by `:`, so it is ignored here.
fn collect(css: &str, map: &mut BTreeMap<String, String>, uncond: &mut BTreeSet<String>, viewport_w: f32, root_classes: &[&str]) {
    let b = css.as_bytes();
    let n = b.len();
    let mut i = 0;
    // Track the block a declaration sits in: `depth` (0 = between rules), and
    // whether the current top-level rule's selector is unconditional-root.
    // `sel_start` marks where the pending selector prelude began.
    let mut depth: i32 = 0;
    let mut sel_start = 0usize;
    let mut block_uncond = false;
    // A block whose ONLY selectors qualify the root by a class the document
    // does not carry can never apply — its values must be ignored outright.
    let mut block_dead = false;
    while i < n {
        // Skip comments.
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
            i = skip_comment(b, i);
            continue;
        }
        // `@media` gates its body on the viewport — a variable declared inside
        // a block that doesn't apply must not be collected. The cascade already
        // skips such rules; without this the *values* leaked anyway, so a
        // `prefers-color-scheme:dark` palette (or any mobile-breakpoint
        // override) silently won document-wide. Other at-rules are walked
        // through as before.
        if b[i] == b'@' {
            let mut j = i + 1;
            while j < n && (b[j].is_ascii_alphabetic() || b[j] == b'-') {
                j += 1;
            }
            if css[i + 1..j].eq_ignore_ascii_case("media") {
                let mut k = j;
                while k < n && b[k] != b'{' && b[k] != b';' {
                    k += 1;
                }
                if k >= n || b[k] == b';' {
                    i = (k + 1).min(n);
                    continue;
                }
                let close = crate::css::matching_brace(b, k, n);
                if crate::css::media_matches(&css[j..k], viewport_w) {
                    // The media body is a fresh rule list — its own selectors
                    // decide unconditional-ness, so recurse (depth resets).
                    collect(&css[k + 1..close], map, uncond, viewport_w, root_classes);
                }
                i = (close + 1).min(n);
                sel_start = i;
                continue;
            }
            i = j;
            continue;
        }
        // Skip strings.
        if b[i] == b'"' || b[i] == b'\'' {
            i = skip_string(b, i);
            continue;
        }
        // Block boundaries — track which rule (and whether its selector is
        // unconditional-root) each declaration sits in.
        if b[i] == b'{' {
            if depth == 0 {
                let sel = css[sel_start..i].trim();
                block_uncond = is_unconditional_root(sel);
                block_dead = !block_uncond && root_selector_excluded(sel, root_classes);
            }
            depth += 1;
            i += 1;
            continue;
        }
        if b[i] == b'}' {
            if depth > 0 {
                depth -= 1;
            }
            if depth == 0 {
                sel_start = i + 1;
            }
            i += 1;
            continue;
        }
        if b[i] == b';' && depth == 0 {
            sel_start = i + 1;
            i += 1;
            continue;
        }
        // Candidate custom-property declaration: `--` ident … `:`.
        if b[i] == b'-' && i + 1 < n && b[i + 1] == b'-' {
            let name_start = i;
            let mut j = i + 2;
            while j < n && is_name(b[j]) {
                j += 1;
            }
            // Must have at least one name char after the leading `--`.
            if j > name_start + 2 {
                let mut k = j;
                while k < n && is_ws(b[k]) {
                    k += 1;
                }
                if k < n && b[k] == b':' {
                    // It's a declaration. Read the value up to the terminating
                    // `;` or `}` at paren-depth 0 (skipping strings/comments so
                    // a `;` inside them does not end the value early).
                    let val_start = k + 1;
                    let v = read_value_end(b, val_start);
                    let name = css[name_start..j].to_string();
                    let value = css[val_start..v].trim().to_string();
                    // A declaration at rule level (depth 1) is "unconditional"
                    // only if its selector is a bare root; deeper nesting
                    // (@supports/@keyframes) is collected as before. An
                    // unconditional value overwrites freely and is protected;
                    // a conditional one may not clobber a protected value.
                    if depth == 1 && block_dead {
                        i = v;
                        continue;
                    }
                    let is_uncond = depth != 1 || block_uncond;
                    if is_uncond {
                        map.insert(name.clone(), value);
                        uncond.insert(name);
                    } else if !uncond.contains(&name) {
                        map.insert(name, value);
                    }
                    i = v;
                    continue;
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
}

/// Does a selector list apply unconditionally — i.e. does any alternative
/// target the document root with no class/id/attribute/state qualifier
/// (`:root`, `html`, `body`, `*`, `html body`, …)? A theme or state override
/// like `html.skin-theme-clientpref-night` or `:root[data-theme=dark]` adds a
/// qualifier we cannot confirm at collection time, so its custom-property
/// values must not overwrite the unconditional default (dark palettes were
/// otherwise winning document-wide on any site that ships one).
fn is_unconditional_root(sel: &str) -> bool {
    sel.split(',').any(|alt| {
        let a = alt.trim();
        if a.is_empty() {
            return false;
        }
        // Any class, id, or attribute qualifier makes it conditional.
        if a.contains('.') || a.contains('#') || a.contains('[') {
            return false;
        }
        // The only pseudo allowed is `:root`; anything else (`:where`, `:not`,
        // `:hover`, a pseudo-element, …) is conditional.
        !a.replace(":root", "").contains(':')
    })
}

/// Does this selector list target the root ONLY through classes the document's
/// root element does not have? MediaWiki ships one definition per user
/// preference — `html.vector-feature-custom-font-size-clientpref-1{--font-size-
/// medium:1rem}` next to `…-clientpref-2{…:1.25rem}` — and the page carries
/// exactly one of those classes. Taking the last one seen made every page
/// render at the largest preference (20px instead of 16px), which inflated
/// every measurement on the page.
///
/// Only root-targeting alternatives (`html`/`:root` plus classes) can be
/// judged. Anything else — a plain `.foo`, a descendant selector, an id or
/// attribute — might match somewhere, so it keeps the old permissive
/// behaviour and the block stays alive.
fn root_selector_excluded(sel: &str, root_classes: &[&str]) -> bool {
    let mut saw_root_qualified = false;
    for alt in sel.split(',') {
        let a = alt.trim();
        if a.is_empty() {
            continue;
        }
        let Some(classes) = root_compound_classes(a) else {
            return false; // not judgeable → keep the block
        };
        if classes.iter().all(|c| root_classes.contains(c)) {
            return false; // this alternative does match the root
        }
        saw_root_qualified = true;
    }
    saw_root_qualified
}

/// Split a single `html.a.b` / `:root.a` compound into its classes, or `None`
/// if it is not a plain class-qualified root selector.
fn root_compound_classes(a: &str) -> Option<Vec<&str>> {
    let rest = a.strip_prefix("html").or_else(|| a.strip_prefix(":root"))?;
    if rest.is_empty() || !rest.starts_with('.') {
        return None;
    }
    // Classes only — an id, attribute, pseudo or combinator is out of scope.
    if rest.contains(['#', '[', ':', ' ', '>', '+', '~']) {
        return None;
    }
    Some(rest.split('.').filter(|c| !c.is_empty()).collect())
}

/// Return the index one past the end of a declaration value that starts at
/// `start`, i.e. the position of the terminating `;`/`}` (or end of input).
fn read_value_end(b: &[u8], start: usize) -> usize {
    let n = b.len();
    let mut v = start;
    let mut depth: i32 = 0;
    while v < n {
        let c = b[v];
        if c == b'/' && v + 1 < n && b[v + 1] == b'*' {
            v = skip_comment(b, v);
            continue;
        }
        if c == b'"' || c == b'\'' {
            v = skip_string(b, v);
            continue;
        }
        match c {
            b'(' => depth += 1,
            b')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            b';' if depth == 0 => break,
            b'}' if depth == 0 => break,
            _ => {}
        }
        v += 1;
    }
    v
}

// ── expansion ───────────────────────────────────────────────────────────────

/// One left-to-right pass replacing each top-level `var(...)` with its resolved
/// value. Returns the rewritten string and whether any replacement happened.
/// `var(` inside strings/comments is left untouched. Replacements are inserted
/// verbatim (they may still contain `var()`, resolved on the next pass).
fn expand_once(input: &str, map: &BTreeMap<String, String>) -> (String, bool) {
    let b = input.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n + 16);
    let mut i = 0;
    let mut changed = false;
    while i < n {
        // Copy comments verbatim.
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let end = skip_comment(b, i);
            out.extend_from_slice(&b[i..end]);
            i = end;
            continue;
        }
        // Copy strings verbatim (a `var(` inside a string is literal text).
        if b[i] == b'"' || b[i] == b'\'' {
            let end = skip_string(b, i);
            out.extend_from_slice(&b[i..end]);
            i = end;
            continue;
        }
        // `var(` not glued to a preceding ident char (so `xvar(` / `--my-var(`
        // are not matched).
        if is_var_at(b, i) && !(i > 0 && is_name(b[i - 1])) {
            if let Some((end, name, fallback)) = parse_var_args(input, i + 3) {
                let repl = match map.get(&name) {
                    Some(v) => v.as_str(),
                    None => fallback.as_deref().unwrap_or(""),
                };
                out.extend_from_slice(repl.as_bytes());
                changed = true;
                i = end;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let s = match String::from_utf8(out) {
        Ok(s) => s,
        // Cannot happen (we only copy exact byte ranges of a valid &str and
        // insert valid UTF-8), but never panic on stylesheet input.
        Err(e) => String::from_utf8_lossy(&e.into_bytes()).into_owned(),
    };
    (s, changed)
}

/// Parse the argument list of a `var(` whose `(` is at byte index `open`.
/// Returns `(index-after-close-paren, name, fallback)`. The name is the first
/// argument (must start with `--`); the fallback, if a top-level comma is
/// present, is the raw remaining text (may itself contain commas, parens, and
/// nested `var()`). Whitespace around name/fallback is trimmed. `None` if the
/// parens are unbalanced or the first argument isn't a custom-property name —
/// then the caller leaves the text as-is.
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
fn is_var_at(b: &[u8], i: usize) -> bool {
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

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_root_var() {
        let out = resolve_vars(":root{--c:#f00} a{color:var(--c)}", 800.0, &[]);
        assert!(out.contains("color:#f00"), "{out}");
    }

    #[test]
    fn fallback_used_when_undefined() {
        let out = resolve_vars("a{color:var(--missing, blue)}", 800.0, &[]);
        assert!(out.contains("color:blue"), "{out}");
    }

    #[test]
    fn fallback_ignored_when_defined() {
        let out = resolve_vars(":root{--c:green} a{color:var(--c, blue)}", 800.0, &[]);
        assert!(out.contains("color:green"), "{out}");
        assert!(!out.contains("color:blue"), "{out}");
    }

    #[test]
    fn undefined_no_fallback_is_empty() {
        // `color:var(--nope)` → `color:` (invalid at computed-value time ~ "").
        let out = resolve_vars("a{color:var(--nope)}", 800.0, &[]);
        assert!(out.contains("a{color:}"), "{out}");
    }

    #[test]
    fn nested_var_in_value() {
        let out = resolve_vars(":root{--a:var(--b);--b:#0d6efd} a{color:var(--a)}", 800.0, &[]);
        assert!(out.contains("color:#0d6efd"), "{out}");
    }

    #[test]
    fn nested_var_in_fallback() {
        let out = resolve_vars(":root{--b:orange} a{color:var(--missing, var(--b))}", 800.0, &[]);
        assert!(out.contains("color:orange"), "{out}");
    }

    #[test]
    fn deeply_nested_fallback_with_calc() {
        // Fallback holds calc() with its own nested var() + fallback.
        let out = resolve_vars("a{width:var(--x, calc(1px + var(--y, 2px)))}", 800.0, &[]);
        assert!(out.contains("width:calc(1px + 2px)"), "{out}");
    }

    #[test]
    fn fallback_with_commas_and_parens() {
        // rgba() inside the fallback must survive intact (paren-depth parse).
        let out = resolve_vars("a{box-shadow:0 0 0 var(--x, rgba(0,0,0,.1))}", 800.0, &[]);
        assert!(out.contains("box-shadow:0 0 0 rgba(0,0,0,.1)"), "{out}");
    }

    #[test]
    fn defined_var_wins_over_rgba_fallback() {
        let out = resolve_vars(":root{--x:#111} a{color:var(--x, rgba(0,0,0,.1))}", 800.0, &[]);
        assert!(out.contains("color:#111"), "{out}");
        assert!(!out.contains("rgba"), "{out}");
    }

    #[test]
    fn multiple_uses() {
        let out = resolve_vars(":root{--c:#123456} a{color:var(--c)} b{border-color:var(--c)}", 800.0, &[]);
        assert!(out.contains("color:#123456"), "{out}");
        assert!(out.contains("border-color:#123456"), "{out}");
    }

    #[test]
    fn last_unconditional_declaration_wins() {
        // Among equally unconditional root declarations, the later one wins.
        let out = resolve_vars(":root{--c:red} html{--c:green} a{color:var(--c)}", 800.0, &[]);
        assert!(out.contains("color:green"), "{out}");
    }

    #[test]
    fn qualified_declaration_does_not_override_the_unconditional_one() {
        // A class-qualified definition is conditional on that class being
        // present; it must not win document-wide (the 0.1.51 dark-mode leak,
        // which turned every page dark because one theme class defined the
        // whole palette).
        let out = resolve_vars(":root{--c:red} .x{--c:green} a{color:var(--c)}", 800.0, &[]);
        assert!(out.contains("color:red"), "{out}");
    }

    #[test]
    fn whitespace_variations() {
        let out = resolve_vars(":root{--c:#abc} a{color:var( --c )}", 800.0, &[]);
        assert!(out.contains("color:#abc"), "{out}");
        let out2 = resolve_vars("a{color:var(  --missing ,  blue  )}", 800.0, &[]);
        assert!(out2.contains("color:blue"), "{out2}");
    }

    #[test]
    fn uppercase_var_function() {
        // CSS function names are case-insensitive.
        let out = resolve_vars(":root{--c:#0f0} a{color:VAR(--c)}", 800.0, &[]);
        assert!(out.contains("color:#0f0"), "{out}");
    }

    #[test]
    #[test]
    fn media_block_that_does_not_apply_is_not_collected() {
        // The dark-mode palette must not beat `:root` at a viewport where the
        // query doesn't hold — www.wikipedia.org paints its whole page from
        // `--background-color-base`, and the leak turned every site dark.
        let css = ":root{--bg:#fff} @media (prefers-color-scheme:dark){:root{--bg:#101418}} a{background:var(--bg)}";
        assert!(resolve_vars(css, 1880.0, &[]).contains("background:#fff"));

        // Width queries gate on the actual viewport, both ways.
        let css = ":root{--pad:32px} @media (max-width:480px){:root{--pad:8px}} a{padding:var(--pad)}";
        assert!(resolve_vars(css, 1880.0, &[]).contains("padding:32px"));
        assert!(resolve_vars(css, 400.0, &[]).contains("padding:8px"));

        // A block that DOES hold still contributes, nested ones included.
        let css = ":root{--c:red} @media screen{@media (min-width:600px){:root{--c:green}}} a{color:var(--c)}";
        assert!(resolve_vars(css, 1880.0, &[]).contains("color:green"));
    }

    fn no_vars_passthrough() {
        let css = "a{color:red;font-weight:bold}";
        assert_eq!(resolve_vars(css, 800.0, &[]), css);
    }

    #[test]
    fn var_inside_string_not_expanded() {
        let out = resolve_vars(r#":root{--c:red} a{content:"var(--c)"}"#, 800.0, &[]);
        assert!(out.contains(r#"content:"var(--c)""#), "{out}");
    }

    #[test]
    fn declaration_in_comment_ignored() {
        // A `--x:` inside a comment must not be collected as a real value.
        let out = resolve_vars("/* --c: red */ :root{--c:blue} a{color:var(--c)}", 800.0, &[]);
        assert!(out.contains("color:blue"), "{out}");
    }

    #[test]
    fn cyclic_reference_terminates() {
        // Must not hang; residual var() is acceptable, just no infinite loop.
        let out = resolve_vars(":root{--a:var(--b);--b:var(--a)} x{color:var(--a)}", 800.0, &[]);
        let _ = out; // reaching here means it terminated
    }

    #[test]
    fn bootstrap_like_chain() {
        let css = ":root{--bs-blue:#0d6efd;--bs-primary:var(--bs-blue)}\
                   .btn{background-color:var(--bs-primary);border-color:var(--bs-primary, #ccc)}";
        let out = resolve_vars(css, 800.0, &[]);
        assert!(out.contains("background-color:#0d6efd"), "{out}");
        assert!(out.contains("border-color:#0d6efd"), "{out}");
    }

    #[test]
    fn empty_fallback() {
        let out = resolve_vars("a{margin:var(--m,)}", 800.0, &[]);
        assert!(out.contains("a{margin:}"), "{out}");
    }
}
