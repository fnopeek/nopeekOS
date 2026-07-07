//! css.rs — author stylesheet parsing (css-syntax-3 subset) + selector match.
//!
//! Slice-0.2 gave us the UA sheet (as data) + inline `style="…"`. This adds the
//! middle cascade layer: author rules from `<style>` blocks. The pipeline is
//! now the real thing —
//!
//! ```text
//!   inherited(parent) → UA sheet → author <style> (specificity) → inline
//! ```
//!
//! Selectors: type / `.class` / `#id` / `*`, compounds (`div.a#b`), descendant
//! (space) + child (`>`) combinators, comma lists. Right-to-left matching with
//! an ancestor stack + `(id, class, type)` specificity. Unsupported bits
//! (pseudo-classes, `[attr]`, `+`/`~` siblings, `@media` bodies) are dropped,
//! not mis-applied — forward-compatible like a browser (CONFORMANCE.md).
//! External `<link>` stylesheets need a sub-resource fetch → later; the parser
//! + cascade here are exactly what that will reuse.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::dom::{Dom, Element, Node};

/// The identity a selector matches against: tag + id + classes.
#[derive(Clone)]
pub struct ElemInfo {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
}

impl ElemInfo {
    pub fn of(el: &Element) -> ElemInfo {
        let id = el.attr("id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let classes = el
            .attr("class")
            .map(|c| c.split_whitespace().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        ElemInfo { tag: el.tag.clone(), id, classes }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Comb {
    Descendant,
    Child,
    /// `A + B` — B's immediately preceding element sibling matches A.
    Adjacent,
    /// `A ~ B` — some preceding element sibling of B matches A.
    General,
}

/// A compound selector: an optional type + optional id + zero-or-more classes.
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

impl Compound {
    fn matches(&self, e: &ElemInfo) -> bool {
        if let Some(t) = &self.tag {
            if *t != e.tag {
                return false;
            }
        }
        if let Some(id) = &self.id {
            if e.id.as_deref() != Some(id.as_str()) {
                return false;
            }
        }
        self.classes.iter().all(|c| e.classes.iter().any(|x| x == c))
    }
}

/// A complex selector: compounds left→right, with the combinator that precedes
/// each compound after the first (`combs[k]` sits left of `compounds[k+1]`).
pub struct Selector {
    compounds: Vec<Compound>,
    combs: Vec<Comb>,
    spec: u32,
}

impl Selector {
    /// Right-to-left match: the last compound must match `subject`, then earlier
    /// compounds must match ancestors per their combinators. `ancestors` is
    /// root→…→parent order. Descendant matching is nearest-first (no backtrack —
    /// enough for content selectors; noted as a shortcut).
    fn matches(&self, subject: &ElemInfo, ancestors: &[ElemInfo], prev_siblings: &[ElemInfo]) -> bool {
        let last = self.compounds.len() - 1;
        if !self.compounds[last].matches(subject) {
            return false;
        }
        let mut anc = ancestors.len() as isize - 1; // immediate parent
        let mut sib = prev_siblings.len() as isize - 1; // immediately preceding sibling
        // Sibling combinators (`+`/`~`) only resolve while we're still matching at
        // the subject's own level — once an ancestor combinator moves the context
        // up, we no longer have that ancestor's siblings, so drop rather than
        // mis-apply (covers the common `A + B`, `A ~ B`, `.x .a + .b` cases).
        let mut at_subject = true;
        let mut ci = last as isize - 1;
        while ci >= 0 {
            let comb = self.combs[ci as usize];
            let comp = &self.compounds[ci as usize];
            match comb {
                Comb::Child => {
                    if anc < 0 || !comp.matches(&ancestors[anc as usize]) {
                        return false;
                    }
                    anc -= 1;
                    at_subject = false;
                }
                Comb::Descendant => {
                    let mut a = anc;
                    let mut found = false;
                    while a >= 0 {
                        if comp.matches(&ancestors[a as usize]) {
                            found = true;
                            break;
                        }
                        a -= 1;
                    }
                    if !found {
                        return false;
                    }
                    anc = a - 1;
                    at_subject = false;
                }
                Comb::Adjacent => {
                    if !at_subject || sib < 0 || !comp.matches(&prev_siblings[sib as usize]) {
                        return false;
                    }
                    sib -= 1;
                }
                Comb::General => {
                    if !at_subject {
                        return false;
                    }
                    let mut a = sib;
                    let mut found = false;
                    while a >= 0 {
                        if comp.matches(&prev_siblings[a as usize]) {
                            found = true;
                            break;
                        }
                        a -= 1;
                    }
                    if !found {
                        return false;
                    }
                    sib = a - 1;
                }
            }
            ci -= 1;
        }
        true
    }
}

/// A parsed `@media` condition. We evaluate only the width features that
/// Bootstrap and WordPress breakpoints rely on (`min-width`/`max-width`, in px)
/// plus the `screen`/`all` media types; a query naming any other media type or
/// feature (orientation, prefers-*, print, …) is marked `understood = false`
/// and never matches, so we never mis-apply its rules.
#[derive(Clone, Copy)]
pub struct MediaCond {
    min_width: Option<f32>,
    max_width: Option<f32>,
    understood: bool,
}

impl MediaCond {
    fn matches(&self, viewport_w: f32) -> bool {
        self.understood
            && self.min_width.is_none_or(|m| viewport_w >= m)
            && self.max_width.is_none_or(|m| viewport_w <= m)
    }
}

/// One `selectors { declarations }` rule; `order` is document position (for
/// same-specificity tie-breaking, last wins). `media` is the `@media`
/// condition list it sits inside (comma = OR), or `None` when unconditional.
pub struct Rule {
    selectors: Vec<Selector>,
    decls: Vec<(String, String)>,
    order: u32,
    media: Option<Vec<MediaCond>>,
}

/// A parsed author stylesheet.
pub struct Stylesheet {
    rules: Vec<Rule>,
}

impl Stylesheet {
    pub fn empty() -> Stylesheet {
        Stylesheet { rules: Vec::new() }
    }
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// All declaration blocks that match `subject` (given its `ancestors`),
    /// each tagged with the winning selector's specificity + document order.
    /// Caller sorts ascending and applies in order (later overrides earlier).
    pub fn matched<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        viewport_w: f32,
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        let mut out = Vec::new();
        for rule in &self.rules {
            // Skip rules inside an `@media` block whose condition doesn't hold
            // at this viewport width.
            if let Some(conds) = &rule.media {
                if !conds.iter().any(|c| c.matches(viewport_w)) {
                    continue;
                }
            }
            let mut best: Option<u32> = None;
            for sel in &rule.selectors {
                if sel.matches(subject, ancestors, prev_siblings) {
                    best = Some(best.map_or(sel.spec, |b| b.max(sel.spec)));
                }
            }
            if let Some(spec) = best {
                out.push((spec, rule.order, rule.decls.as_slice()));
            }
        }
        out
    }
}

/// Gather + parse every `<style>` block in the document into one stylesheet.
pub fn collect(dom: &Dom) -> Stylesheet {
    collect_all(dom, "")
}

/// Author stylesheet = already-fetched external `<link>` CSS (document order:
/// `<head>` first) followed by inline `<style>` blocks. The shell fetches the
/// linked files (the engine is host-free) and hands their bytes in as `external`.
pub fn collect_all(dom: &Dom, external: &str) -> Stylesheet {
    let mut css = String::from(external);
    css.push('\n');
    gather_style_text(&dom.root, &mut css);
    if css.trim().is_empty() {
        return Stylesheet::empty();
    }
    // Expand CSS custom properties (`var(--x)`) as a pre-pass so the parser +
    // cascade never see variables — modern sites (Bootstrap's `--bs-*`) lean
    // on them heavily.
    let css = crate::vars::resolve_vars(&css);
    parse(&css)
}

/// Hrefs of every `<link rel="stylesheet">` in the document, for the shell to
/// fetch as sub-resources.
pub fn stylesheet_links(dom: &Dom) -> Vec<String> {
    let mut out = Vec::new();
    collect_links(&dom.root, &mut out);
    out
}

fn collect_links(el: &Element, out: &mut Vec<String>) {
    for c in &el.children {
        if let Node::Element(e) = c {
            if e.tag == "link" {
                let is_ss = e
                    .attr("rel")
                    .unwrap_or("")
                    .split_whitespace()
                    .any(|r| r.eq_ignore_ascii_case("stylesheet"));
                if is_ss {
                    if let Some(h) = e.attr("href") {
                        if !h.trim().is_empty() {
                            out.push(h.trim().to_string());
                        }
                    }
                }
            }
            collect_links(e, out);
        }
    }
}

fn gather_style_text(el: &Element, out: &mut String) {
    for c in &el.children {
        if let Node::Element(e) = c {
            if e.tag == "style" {
                for cc in &e.children {
                    if let Node::Text(t) = cc {
                        out.push_str(t);
                        out.push('\n');
                    }
                }
            } else {
                gather_style_text(e, out);
            }
        }
    }
}

/// Parse a stylesheet body into rules (css-syntax-3 subset). Descends INTO
/// `@media` blocks (their rules apply conditionally on the viewport); other
/// at-rules (`@keyframes`/`@font-face`/`@supports`/`@import`) are skipped.
pub fn parse(css: &str) -> Stylesheet {
    // XHTML `<style>` bodies wrap the CSS in a `<![CDATA[ … ]]>` marker (the
    // CSS2.1 reftest suite does this pervasively). It's raw text to us, so strip
    // the markers before parsing — real CSS never contains them.
    let css = if css.contains("<![CDATA[") {
        css.replace("<![CDATA[", " ").replace("]]>", " ")
    } else {
        String::from(css)
    };
    let css = strip_comments(&css);
    let mut rules = Vec::new();
    let mut order = 0u32;
    parse_into(&css, 0, css.len(), None, &mut rules, &mut order);
    Stylesheet { rules }
}

/// Scan `css[start..end]` for rules, tagging each with the given `media`
/// context, and recurse into nested `@media` blocks. `order` is threaded so
/// document order is preserved across (and into) media blocks.
fn parse_into(
    css: &str,
    start: usize,
    end: usize,
    media: Option<&Vec<MediaCond>>,
    rules: &mut Vec<Rule>,
    order: &mut u32,
) {
    let bytes = css.as_bytes();
    let mut i = start;
    while i < end {
        while i < end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= end {
            break;
        }
        if bytes[i] == b'@' {
            // Read the at-keyword to tell `@media` (descend) from the rest (skip).
            let mut j = i + 1;
            while j < end && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'-') {
                j += 1;
            }
            if css[i + 1..j].eq_ignore_ascii_case("media") {
                let mut k = j;
                while k < end && bytes[k] != b'{' && bytes[k] != b';' {
                    k += 1;
                }
                if k >= end || bytes[k] == b';' {
                    i = (k + 1).min(end);
                    continue;
                }
                let conds = parse_media_query(&css[j..k]);
                let close = matching_brace(bytes, k, end);
                parse_into(css, k + 1, close, Some(&conds), rules, order);
                i = (close + 1).min(end);
            } else if css[i + 1..j].eq_ignore_ascii_case("supports") {
                // Descend into `@supports` when the condition holds; else skip
                // the block (its rules never apply). Keeps any enclosing @media.
                let mut k = j;
                while k < end && bytes[k] != b'{' && bytes[k] != b';' {
                    k += 1;
                }
                if k >= end || bytes[k] == b';' {
                    i = (k + 1).min(end);
                    continue;
                }
                let close = matching_brace(bytes, k, end);
                if supports_cond(&css[j..k]) {
                    parse_into(css, k + 1, close, media, rules, order);
                }
                i = (close + 1).min(end);
            } else {
                i = skip_at_rule(bytes, i).min(end);
            }
            continue;
        }
        let sel_start = i;
        while i < end && bytes[i] != b'{' && bytes[i] != b'}' {
            i += 1;
        }
        if i >= end || bytes[i] == b'}' {
            break;
        }
        let sel_text = &css[sel_start..i];
        i += 1; // '{'
        let body_start = i;
        while i < end && bytes[i] != b'}' {
            i += 1;
        }
        let body = &css[body_start..i.min(end)];
        if i < end {
            i += 1; // '}'
        }
        let selectors = parse_selector_list(sel_text);
        let decls = parse_decls(body);
        if !selectors.is_empty() && !decls.is_empty() {
            rules.push(Rule { selectors, decls, order: *order, media: media.cloned() });
            *order += 1;
        }
    }
}

/// Index of the `}` that closes the `{` at/after `open` (or `end` if unbalanced).
fn matching_brace(bytes: &[u8], open: usize, end: usize) -> usize {
    let mut depth = 0i32;
    let mut i = open;
    while i < end {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    end
}

/// Evaluate an `@supports` condition. Handles `not`, top-level `and`/`or`, and
/// `(prop: value)` leaves. A colour-property leaf is supported iff the value
/// parses as a colour; other feature leaves are assumed supported (render-what-
/// the-author-intended bias — we implement most box/flex/grid properties).
fn supports_cond(cond: &str) -> bool {
    let c = cond.trim();
    if c.is_empty() {
        return false;
    }
    if let Some(rest) = strip_ci_prefix(c, "not ") {
        return !supports_cond(rest);
    }
    if let Some(parts) = split_top(c, " and ") {
        return parts.iter().all(|p| supports_cond(p));
    }
    if let Some(parts) = split_top(c, " or ") {
        return parts.iter().any(|p| supports_cond(p));
    }
    // Unwrap one layer of grouping parens.
    let inner = c.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(c).trim();
    if inner != c
        && (split_top(inner, " and ").is_some()
            || split_top(inner, " or ").is_some()
            || strip_ci_prefix(inner, "not ").is_some())
    {
        return supports_cond(inner);
    }
    match inner.split_once(':') {
        Some((prop, val)) => supports_decl(prop.trim(), val.trim()),
        None => false,
    }
}

fn supports_decl(prop: &str, val: &str) -> bool {
    let p = prop.to_ascii_lowercase();
    if p == "color" || p == "background" || p == "fill" || p == "stroke" || p.ends_with("-color") {
        return crate::color::parse_color(val).is_some();
    }
    !val.is_empty()
}

/// Case-insensitive prefix strip.
fn strip_ci_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Split `s` on `sep` at paren-depth 0. `None` if `sep` never occurs at top
/// level (so the caller can treat `s` as a single term).
fn split_top<'a>(s: &'a str, sep: &str) -> Option<Vec<&'a str>> {
    let b = s.as_bytes();
    let sb = sep.as_bytes();
    let (mut depth, mut i, mut last) = (0i32, 0usize, 0usize);
    let mut parts = Vec::new();
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => depth = (depth - 1).max(0),
            _ => {}
        }
        if depth == 0 && i + sb.len() <= b.len() && &b[i..i + sb.len()] == sb {
            parts.push(s[last..i].trim());
            i += sb.len();
            last = i;
            continue;
        }
        i += 1;
    }
    if parts.is_empty() {
        return None;
    }
    parts.push(s[last..].trim());
    Some(parts)
}

/// Parse a `@media` prelude (comma = OR) into conditions. Only `min-width`/
/// `max-width` (px) plus the `screen`/`all`/`only` media types are evaluated;
/// any other media type or feature marks that branch not-understood so it never
/// matches (we never mis-apply a rule we cannot evaluate).
fn parse_media_query(prelude: &str) -> Vec<MediaCond> {
    prelude
        .split(',')
        .map(|q| {
            let mut cond = MediaCond { min_width: None, max_width: None, understood: true };
            let ql = q.to_ascii_lowercase();
            for part in ql.split("and") {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                if let Some(inner) = p.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
                    let mut kv = inner.splitn(2, ':');
                    let feat = kv.next().unwrap_or("").trim();
                    let val = kv.next().unwrap_or("").trim();
                    match feat {
                        "min-width" => cond.min_width = parse_px(val),
                        "max-width" => cond.max_width = parse_px(val),
                        _ => cond.understood = false,
                    }
                } else {
                    for word in p.split_whitespace() {
                        match word {
                            "screen" | "all" | "only" => {}
                            _ => cond.understood = false,
                        }
                    }
                }
            }
            cond
        })
        .collect()
}

/// A media-feature `<length>` — px only (Bootstrap/WP breakpoints are all px).
fn parse_px(v: &str) -> Option<f32> {
    let v = v.trim();
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let b = css.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            // A comment is a token separator (css-syntax-3), NOT nothing —
            // replace it with a space so `12px/* */solid` doesn't glue into
            // `12pxsolid` (and `hsl(120/* */75%…)` tokenises correctly).
            out.push(' ');
        } else {
            out.push(css[i..].chars().next().unwrap());
            i += css[i..].chars().next().unwrap().len_utf8();
        }
    }
    out
}

fn skip_at_rule(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && bytes[i] != b';' && bytes[i] != b'{' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'{' {
        let mut depth = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        i
    } else {
        (i + 1).min(bytes.len())
    }
}

fn parse_selector_list(text: &str) -> Vec<Selector> {
    text.split(',').filter_map(|s| parse_selector(s.trim())).collect()
}

fn parse_selector(text: &str) -> Option<Selector> {
    let mut compounds: Vec<Compound> = Vec::new();
    let mut combs: Vec<Comb> = Vec::new();
    let mut pending = Comb::Descendant;
    for tok in tokenize_selector(text) {
        match tok.as_str() {
            ">" => {
                pending = Comb::Child;
                continue;
            }
            "+" => {
                pending = Comb::Adjacent;
                continue;
            }
            "~" => {
                pending = Comb::General;
                continue;
            }
            _ => {}
        }
        let comp = parse_compound(&tok)?;
        if !compounds.is_empty() {
            combs.push(pending);
        }
        compounds.push(comp);
        pending = Comb::Descendant;
    }
    if compounds.is_empty() {
        return None;
    }
    let spec = specificity(&compounds);
    Some(Selector { compounds, combs, spec })
}

/// Split into compound tokens + `>` tokens on whitespace / `>` boundaries.
fn tokenize_selector(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(core::mem::take(&mut cur));
            }
        } else if ch == '>' || ch == '+' || ch == '~' {
            if !cur.is_empty() {
                out.push(core::mem::take(&mut cur));
            }
            out.push(ch.to_string());
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_compound(tok: &str) -> Option<Compound> {
    if tok.is_empty() || tok.contains([':', '[', ']', '+', '~', '(', ')']) {
        return None; // unsupported selector features → drop the whole selector
    }
    let bytes = tok.as_bytes();
    let mut tag = None;
    let mut id = None;
    let mut classes = Vec::new();
    let mut i = 0;
    if bytes[0] == b'*' {
        i = 1;
    } else if bytes[0].is_ascii_alphabetic() {
        let s = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
            i += 1;
        }
        tag = Some(tok[s..i].to_ascii_lowercase());
    }
    while i < bytes.len() {
        let marker = bytes[i];
        if marker != b'.' && marker != b'#' {
            return None;
        }
        i += 1;
        let s = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_') {
            i += 1;
        }
        if i == s {
            return None;
        }
        if marker == b'.' {
            classes.push(tok[s..i].to_string());
        } else {
            id = Some(tok[s..i].to_string());
        }
    }
    Some(Compound { tag, id, classes })
}

/// CSS specificity packed as (id<<20)|(class<<10)|type — enough headroom.
fn specificity(compounds: &[Compound]) -> u32 {
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    for comp in compounds {
        if comp.id.is_some() {
            a += 1;
        }
        b += comp.classes.len() as u32;
        if comp.tag.is_some() {
            c += 1;
        }
    }
    (a << 20) | (b << 10) | c
}

fn parse_decls(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for decl in body.split(';') {
        let mut it = decl.splitn(2, ':');
        let (p, v) = match (it.next(), it.next()) {
            (Some(p), Some(v)) => (p.trim(), v.trim()),
            _ => continue,
        };
        if !p.is_empty() && !v.is_empty() {
            out.push((p.to_ascii_lowercase(), v.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    fn info(tag: &str, id: Option<&str>, classes: &[&str]) -> ElemInfo {
        ElemInfo {
            tag: tag.to_string(),
            id: id.map(|s| s.to_string()),
            classes: classes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn parses_and_matches_type_class_id() {
        let ss = parse("p { color: red } .lead { font-weight: bold } #main { color: blue }");
        assert!(!ss.matched(&info("p", None, &[]), &[], &[], 1000.0).is_empty());
        assert!(!ss.matched(&info("div", None, &["lead"]), &[], &[], 1000.0).is_empty());
        assert!(!ss.matched(&info("div", Some("main"), &[]), &[], &[], 1000.0).is_empty());
        assert!(ss.matched(&info("span", None, &[]), &[], &[], 1000.0).is_empty());
    }

    #[test]
    fn descendant_and_child_combinators() {
        let ss = parse("nav a { color: green } ul > li { color: teal }");
        let nav = info("nav", None, &[]);
        let div = info("div", None, &[]);
        let ul = info("ul", None, &[]);
        // nav a: matches an <a> with <nav> anywhere above
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone()], &[], 1000.0).is_empty());
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone(), div.clone()], &[], 1000.0).is_empty());
        assert!(ss.matched(&info("a", None, &[]), &[div.clone()], &[], 1000.0).is_empty());
        // ul > li: <li> whose IMMEDIATE parent is <ul>
        assert!(!ss.matched(&info("li", None, &[]), &[ul.clone()], &[], 1000.0).is_empty());
        assert!(ss.matched(&info("li", None, &[]), &[ul.clone(), div.clone()], &[], 1000.0).is_empty());
    }

    #[test]
    fn specificity_ranks_id_over_class_over_type() {
        let ss = parse("p { color: a } p.x { color: b } #p { color: c }");
        let e = info("p", Some("p"), &["x"]);
        let mut m = ss.matched(&e, &[], &[], 1000.0);
        m.sort_by_key(|(spec, order, _)| (*spec, *order));
        // ascending: type(p) < class(p.x) < id(#p)
        assert_eq!(m.len(), 3);
        assert!(m[0].0 < m[1].0 && m[1].0 < m[2].0);
    }

    #[test]
    fn comments_at_rules_and_bad_selectors_are_tolerated() {
        let ss = parse(
            "/* c */ @media screen { p { color: x } } \
             a:hover { color: y } input[type=text] { color: z } \
             h1, h2 { color: ok }",
        );
        // :hover + [attr] selectors dropped (unsupported); the @media block is
        // now DESCENDED (not dropped), but `screen` alone always matches so its
        // `p` rule is fine either way …
        assert!(ss.matched(&info("a", None, &[]), &[], &[], 1000.0).is_empty());
        assert!(ss.matched(&info("input", None, &[]), &[], &[], 1000.0).is_empty());
        // … and the plain "h1, h2" list still parsed.
        assert!(!ss.matched(&info("h2", None, &[]), &[], &[], 1000.0).is_empty());
    }

    #[test]
    fn extracts_stylesheet_link_hrefs() {
        let dom = dom::parse(
            "<head><link rel=stylesheet href=/a.css>\
             <link rel=\"icon\" href=/x.ico>\
             <link rel=\"stylesheet\" href='/b.css'></head>",
        );
        assert_eq!(stylesheet_links(&dom), alloc::vec!["/a.css".to_string(), "/b.css".to_string()]);
    }

    #[test]
    fn external_css_cascades_before_inline_style() {
        // external (red) parsed first, inline <style> (blue) after → blue wins.
        let dom = dom::parse("<html><head><style>p{color:blue}</style></head><body><p>x</p></body></html>");
        let ss = collect_all(&dom, "p { color: red }");
        let mut m = ss.matched(&info("p", None, &[]), &[], &[], 1000.0);
        m.sort_by_key(|(spec, order, _)| (*spec, *order));
        assert_eq!(m.len(), 2, "both external + inline rules match");
        assert!(m[0].1 < m[1].1, "external rule has earlier document order");
    }

    #[test]
    fn collects_style_blocks_from_dom() {
        let dom = dom::parse("<html><head><style>p{color:red}</style></head>\
            <body><style>.x{color:blue}</style><p>hi</p></body></html>");
        let ss = collect(&dom);
        assert!(!ss.matched(&info("p", None, &[]), &[], &[], 1000.0).is_empty());
        assert!(!ss.matched(&info("span", None, &["x"]), &[], &[], 1000.0).is_empty());
    }

    #[test]
    fn media_min_width_applies_only_above_breakpoint() {
        // A `.col` base rule always applies; the `@media (min-width:768px)`
        // override applies only at wide viewports.
        let ss = parse(
            ".col { color: red } @media (min-width: 768px) { .col { color: green } }",
        );
        let e = info("div", None, &["col"]);
        // Wide (1000 ≥ 768): both rules present.
        assert_eq!(ss.matched(&e, &[], &[], 1000.0).len(), 2, "media rule applies wide");
        // Narrow (500 < 768): only the base rule.
        assert_eq!(ss.matched(&e, &[], &[], 500.0).len(), 1, "media rule dropped narrow");
        // An un-evaluable feature never matches (rule dropped both ways).
        let ss2 = parse("@media (prefers-color-scheme: dark) { .col { color: blue } }");
        assert!(ss2.matched(&e, &[], &[], 1000.0).is_empty(), "unknown feature never applies");
    }
}
