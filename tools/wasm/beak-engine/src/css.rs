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

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::dom::{Dom, Element, Node};

/// The identity a selector matches against: tag + id + classes + all attributes
/// (names lowercased) for `[attr]` selectors.
#[derive(Clone)]
pub struct ElemInfo {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<(String, String)>,
}

impl ElemInfo {
    pub fn of(el: &Element) -> ElemInfo {
        let id = el.attr("id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let classes = el
            .attr("class")
            .map(|c| c.split_whitespace().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let attrs = el.attrs.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
        ElemInfo { tag: el.tag.clone(), id, classes, attrs }
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

/// Which generated-content pseudo-element (if any) a selector targets. Only
/// `::before`/`::after` (single- or double-colon) are recognised; every other
/// pseudo-element (`::first-line`, `::placeholder`, …) is unsupported and
/// drops the whole selector at parse time, same as an unknown pseudo-class.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PseudoElem {
    #[default]
    None,
    Before,
    After,
}

/// An `[attr]` attribute selector with its match operator.
#[derive(Clone, Copy)]
enum AttrOp {
    Exists,  // [a]
    Eq,      // [a=v]
    Includes, // [a~=v] — whitespace-separated word list contains v
    Dash,    // [a|=v] — v or v-…
    Prefix,  // [a^=v]
    Suffix,  // [a$=v]
    Substr,  // [a*=v]
}

struct AttrSel {
    name: String,
    op: AttrOp,
    val: String,
}

impl AttrSel {
    fn matches(&self, e: &ElemInfo) -> bool {
        e.attrs.iter().any(|(k, v)| {
            if *k != self.name {
                return false;
            }
            match self.op {
                AttrOp::Exists => true,
                AttrOp::Eq => v == &self.val,
                AttrOp::Includes => v.split_whitespace().any(|w| w == self.val),
                AttrOp::Dash => v == &self.val || v.starts_with(&alloc::format!("{}-", self.val)),
                AttrOp::Prefix => !self.val.is_empty() && v.starts_with(&self.val),
                AttrOp::Suffix => !self.val.is_empty() && v.ends_with(&self.val),
                AttrOp::Substr => !self.val.is_empty() && v.contains(&self.val),
            }
        })
    }
}

/// A structural pseudo-class, evaluated against the element's 1-based index
/// among its element siblings and the total sibling count `(index, count)`.
#[derive(Clone, Copy)]
enum Structural {
    FirstChild,
    LastChild,
    OnlyChild,
    NthChild(i32, i32),     // matches index == a*n + b for some n ≥ 0
    NthLastChild(i32, i32), // same, counted from the end
}

impl Structural {
    fn matches(&self, ctx: Option<(u32, u32)>) -> bool {
        let Some((idx, count)) = ctx else { return false }; // no sibling context → can't evaluate
        // i == a*n + b for some integer n ≥ 0 (handles a ≤ 0 too).
        let nth = |a: i32, b: i32, i: u32| {
            let i = i as i32;
            if a == 0 {
                i == b
            } else {
                let d = i - b;
                d % a == 0 && d / a >= 0
            }
        };
        match *self {
            Structural::FirstChild => idx == 1,
            Structural::LastChild => idx == count,
            Structural::OnlyChild => count == 1,
            Structural::NthChild(a, b) => nth(a, b, idx),
            Structural::NthLastChild(a, b) => count >= idx && nth(a, b, count - idx + 1),
        }
    }
}

/// A compound selector: optional type + id + classes + `[attr]` + `:not(…)` +
/// structural pseudo-classes, all of which must hold.
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<AttrSel>,
    not: Vec<Compound>,
    structural: Vec<Structural>,
    /// `::before`/`::after` on this compound (only valid on the LAST compound
    /// of a selector — checked in `parse_selector`).
    pseudo: PseudoElem,
}

impl Compound {
    /// `ctx = Some((index, count))` provides the sibling position for structural
    /// pseudo-classes; `None` (an ancestor with no known position) makes any
    /// structural pseudo fail (the selector is dropped rather than mis-applied).
    fn matches(&self, e: &ElemInfo, ctx: Option<(u32, u32)>) -> bool {
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
        if !self.classes.iter().all(|c| e.classes.iter().any(|x| x == c)) {
            return false;
        }
        if !self.attrs.iter().all(|a| a.matches(e)) {
            return false;
        }
        if self.structural.iter().any(|s| !s.matches(ctx)) {
            return false;
        }
        // :not(x) — none of the negated compounds may match.
        !self.not.iter().any(|n| n.matches(e, ctx))
    }
}

/// A complex selector: compounds left→right, with the combinator that precedes
/// each compound after the first (`combs[k]` sits left of `compounds[k+1]`).
pub struct Selector {
    compounds: Vec<Compound>,
    combs: Vec<Comb>,
    spec: u32,
    /// `::before`/`::after` this selector targets (`None` = a normal
    /// selector, matching the real element).
    pseudo: PseudoElem,
}

impl Selector {
    /// Right-to-left match: the last compound must match `subject`, then earlier
    /// compounds must match ancestors per their combinators. `ancestors` is
    /// root→…→parent order. Descendant matching is nearest-first (no backtrack —
    /// enough for content selectors; noted as a shortcut).
    fn matches(&self, subject: &ElemInfo, ancestors: &[ElemInfo], prev_siblings: &[ElemInfo], sib_count: u32) -> bool {
        // The subject's structural pseudo-classes evaluate against its 1-based
        // sibling index (preceding count + 1) and the total sibling count.
        let subj_ctx = Some((prev_siblings.len() as u32 + 1, sib_count));
        let last = self.compounds.len() - 1;
        if !self.compounds[last].matches(subject, subj_ctx) {
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
                    if anc < 0 || !comp.matches(&ancestors[anc as usize], None) {
                        return false;
                    }
                    anc -= 1;
                    at_subject = false;
                }
                Comb::Descendant => {
                    let mut a = anc;
                    let mut found = false;
                    while a >= 0 {
                        if comp.matches(&ancestors[a as usize], None) {
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
                    if !at_subject || sib < 0 || !comp.matches(&prev_siblings[sib as usize], None) {
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
                        if comp.matches(&prev_siblings[a as usize], None) {
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
///
/// Rules are also indexed by the most selective simple selector in each
/// selector's RIGHTMOST compound, because that one must match the subject for
/// the whole selector to have a chance. Without the index every element is
/// tested against every rule, which on a real page is the dominant cost of a
/// page load: measured on an English Wikipedia article (183 KB HTML, 230 KB
/// CSS) laying out took 159 ms with the site's stylesheet and 6.8 ms with an
/// empty one — i.e. ~95 % of layout was selector matching. Under the WASM
/// interpreter on the device that was 13 of the 14.6 seconds to first paint.
pub struct Stylesheet {
    rules: Vec<Rule>,
    /// Candidate `(rule, selector)` pairs per bucket. A selector lives in
    /// exactly one bucket, so collecting several buckets cannot produce
    /// duplicates.
    by_id: BTreeMap<String, Vec<(u32, u32)>>,
    by_class: BTreeMap<String, Vec<(u32, u32)>>,
    by_tag: BTreeMap<String, Vec<(u32, u32)>>,
    /// Selectors whose rightmost compound names no tag, id or class (`*`,
    /// `[attr]`, a bare `:not(...)`) — these must be tried for every element.
    universal: Vec<(u32, u32)>,
}

impl Stylesheet {
    pub fn empty() -> Stylesheet {
        Stylesheet {
            rules: Vec::new(),
            by_id: BTreeMap::new(),
            by_class: BTreeMap::new(),
            by_tag: BTreeMap::new(),
            universal: Vec::new(),
        }
    }

    /// Build the selector index. Called once after parsing.
    fn build_index(&mut self) {
        for (ri, rule) in self.rules.iter().enumerate() {
            for (si, sel) in rule.selectors.iter().enumerate() {
                let key = (ri as u32, si as u32);
                let last = match sel.compounds.last() {
                    Some(c) => c,
                    None => continue,
                };
                // Most selective first: an id narrows far more than a tag.
                if let Some(id) = &last.id {
                    self.by_id.entry(id.clone()).or_default().push(key);
                } else if let Some(cls) = last.classes.first() {
                    self.by_class.entry(cls.clone()).or_default().push(key);
                } else if let Some(tag) = &last.tag {
                    self.by_tag.entry(tag.clone()).or_default().push(key);
                } else {
                    self.universal.push(key);
                }
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// All declaration blocks that match `subject` itself (given its
    /// `ancestors`) — i.e. selectors with no trailing `::before`/`::after`.
    /// Each is tagged with the winning selector's specificity + document
    /// order; caller sorts ascending and applies in order (later overrides
    /// earlier).
    pub fn matched<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        viewport_w: f32,
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        self.matched_filtered(subject, ancestors, prev_siblings, sib_count, viewport_w, PseudoElem::None)
    }

    /// Same as `matched`, but for `subject`'s `::before`/`::after` generated
    /// box — only selectors ending in that pseudo-element are considered.
    pub fn matched_pseudo<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        viewport_w: f32,
        pseudo: PseudoElem,
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        self.matched_filtered(subject, ancestors, prev_siblings, sib_count, viewport_w, pseudo)
    }

    fn matched_filtered<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        viewport_w: f32,
        want: PseudoElem,
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        // Only selectors whose rightmost compound could match this element are
        // worth testing — that is what the index buys. Everything else is
        // unchanged: same tests, same specificity, same result.
        let mut cands: Vec<(u32, u32)> = Vec::new();
        if let Some(id) = &subject.id {
            if let Some(v) = self.by_id.get(id.as_str()) {
                cands.extend_from_slice(v);
            }
        }
        for c in &subject.classes {
            if let Some(v) = self.by_class.get(c.as_str()) {
                cands.extend_from_slice(v);
            }
        }
        if let Some(v) = self.by_tag.get(subject.tag.as_str()) {
            cands.extend_from_slice(v);
        }
        cands.extend_from_slice(&self.universal);
        // Group by rule so one rule contributes one entry, at the highest
        // specificity among its matching selectors (as before).
        cands.sort_unstable();

        let mut out = Vec::new();
        let mut i = 0;
        while i < cands.len() {
            let ri = cands[i].0;
            let rule = &self.rules[ri as usize];
            // Skip rules inside an `@media` block whose condition doesn't hold
            // at this viewport width.
            let media_ok = match &rule.media {
                Some(conds) => conds.iter().any(|c| c.matches(viewport_w)),
                None => true,
            };
            let mut best: Option<u32> = None;
            while i < cands.len() && cands[i].0 == ri {
                if media_ok {
                    let sel = &rule.selectors[cands[i].1 as usize];
                    if sel.pseudo == want && sel.matches(subject, ancestors, prev_siblings, sib_count) {
                        best = Some(best.map_or(sel.spec, |b| b.max(sel.spec)));
                    }
                }
                i += 1;
            }
            if let Some(spec) = best {
                out.push((spec, rule.order, rule.decls.as_slice()));
            }
        }
        out
    }
}

/// Gather + parse every `<style>` block in the document into one stylesheet.
pub fn collect(dom: &Dom, viewport_w: f32) -> Stylesheet {
    collect_all(dom, "", viewport_w)
}

/// Author stylesheet = already-fetched external `<link>` CSS (document order:
/// `<head>` first) followed by inline `<style>` blocks. The shell fetches the
/// linked files (the engine is host-free) and hands their bytes in as `external`.
pub fn collect_all(dom: &Dom, external: &str, viewport_w: f32) -> Stylesheet {
    let mut css = String::from(external);
    css.push('\n');
    gather_style_text(&dom.root, &mut css);
    if css.trim().is_empty() {
        return Stylesheet::empty();
    }
    // Expand CSS custom properties (`var(--x)`) as a pre-pass so the parser +
    // cascade never see variables — modern sites (Bootstrap's `--bs-*`) lean
    // on them heavily.
    let css = crate::vars::resolve_vars(&css, viewport_w);
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
    let mut sheet = Stylesheet {
        rules,
        by_id: BTreeMap::new(),
        by_class: BTreeMap::new(),
        by_tag: BTreeMap::new(),
        universal: Vec::new(),
    };
    sheet.build_index();
    sheet
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
        if i >= end {
            break;
        }
        // A `}` where a selector was expected is a stray close — usually the end
        // of a nested style rule (`.a { .b { … } }`, which we flatten rather than
        // support) or plain malformed CSS. css-syntax-3 error recovery: consume it
        // and keep scanning. Aborting here (the old `break`) dropped the ENTIRE
        // rest of a large sheet — Wikipedia's grid layout sits 174 KB past one
        // such nested block, so a single `}` silently killed the whole page.
        if bytes[i] == b'}' {
            i += 1;
            continue;
        }
        let sel_text = &css[sel_start..i];
        i += 1; // '{'
        let body_start = i;
        // Scan to the MATCHING `}`, tracking `{}` depth (and skipping string
        // literals so a `{`/`}` inside `content:"…"` doesn't miscount). Without
        // depth tracking a nested rule's inner `}` ended the parent early and
        // leaked its real closing `}` to the top level, desyncing the parser.
        let mut depth = 1i32;
        let mut quote = 0u8;
        while i < end {
            let c = bytes[i];
            if quote != 0 {
                if c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == quote {
                    quote = 0;
                }
            } else {
                match c {
                    b'"' | b'\'' => quote = c,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
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
pub(crate) fn matching_brace(bytes: &[u8], open: usize, end: usize) -> usize {
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

/// Does an `@media` prelude hold at this viewport width? Shared with the
/// custom-property pre-pass, which must gate on exactly the same condition the
/// cascade uses — otherwise a variable from a non-matching block leaks.
pub(crate) fn media_matches(prelude: &str, viewport_w: f32) -> bool {
    parse_media_query(prelude).iter().any(|c| c.matches(viewport_w))
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
    // `::before`/`::after` may only sit on the LAST compound (the subject) —
    // `a:before b` has no meaning, so treat it as an unsupported selector
    // rather than mis-applying it to `b`.
    let last = compounds.len() - 1;
    if compounds.iter().enumerate().any(|(i, c)| i != last && c.pseudo != PseudoElem::None) {
        return None;
    }
    let pseudo = compounds[last].pseudo;
    let spec = specificity(&compounds);
    Some(Selector { compounds, combs, spec, pseudo })
}

/// Split into compound tokens + `>` tokens on whitespace / `>` boundaries.
fn tokenize_selector(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32; // inside [...] or :not(...) — don't split on combinators there
    for ch in text.chars() {
        match ch {
            '[' | '(' => {
                depth += 1;
                cur.push(ch);
            }
            ']' | ')' => {
                depth = (depth - 1).max(0);
                cur.push(ch);
            }
            _ if depth > 0 => cur.push(ch),
            _ if ch.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(core::mem::take(&mut cur));
                }
            }
            '>' | '+' | '~' => {
                if !cur.is_empty() {
                    out.push(core::mem::take(&mut cur));
                }
                out.push(ch.to_string());
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_compound(tok: &str) -> Option<Compound> {
    if tok.is_empty() {
        return None;
    }
    let b = tok.as_bytes();
    let mut c = Compound {
        tag: None,
        id: None,
        classes: Vec::new(),
        attrs: Vec::new(),
        not: Vec::new(),
        structural: Vec::new(),
        pseudo: PseudoElem::None,
    };
    let mut i = 0;
    // Leading type selector or universal `*`.
    if b[0] == b'*' {
        i = 1;
    } else if b[0].is_ascii_alphabetic() {
        let s = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
            i += 1;
        }
        c.tag = Some(tok[s..i].to_ascii_lowercase());
    }
    while i < b.len() {
        match b[i] {
            b'.' | b'#' => {
                let marker = b[i];
                i += 1;
                let s = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'_') {
                    i += 1;
                }
                if i == s {
                    return None;
                }
                if marker == b'.' {
                    c.classes.push(tok[s..i].to_string());
                } else {
                    c.id = Some(tok[s..i].to_string());
                }
            }
            b'[' => {
                let end = tok[i..].find(']')? + i;
                c.attrs.push(parse_attr(&tok[i + 1..end])?);
                i = end + 1;
            }
            b':' => {
                let dbl = i + 1 < b.len() && b[i + 1] == b':';
                i += if dbl { 2 } else { 1 };
                let s = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
                    i += 1;
                }
                let name = tok[s..i].to_ascii_lowercase();
                let arg = if i < b.len() && b[i] == b'(' {
                    let end = tok[i..].find(')')? + i;
                    let a = tok[i + 1..end].to_string();
                    i = end + 1;
                    Some(a)
                } else {
                    None
                };
                match (name.as_str(), arg) {
                    // `:before`/`::before` (legacy single-colon CSS2 syntax is
                    // still valid per css-pseudo-4 §2.1) — generated content.
                    ("before", None) => {
                        if c.pseudo != PseudoElem::None {
                            return None;
                        }
                        c.pseudo = PseudoElem::Before;
                    }
                    ("after", None) => {
                        if c.pseudo != PseudoElem::None {
                            return None;
                        }
                        c.pseudo = PseudoElem::After;
                    }
                    // Any other `::pseudo-element` (first-line, placeholder,
                    // selection, …) is unsupported → drop rather than mis-apply.
                    _ if dbl => return None,
                    ("not", Some(a)) => c.not.push(parse_compound(a.trim())?),
                    ("first-child", None) => c.structural.push(Structural::FirstChild),
                    ("last-child", None) => c.structural.push(Structural::LastChild),
                    ("only-child", None) => c.structural.push(Structural::OnlyChild),
                    ("nth-child", Some(a)) => {
                        let (x, y) = parse_nth(&a)?;
                        c.structural.push(Structural::NthChild(x, y));
                    }
                    ("nth-last-child", Some(a)) => {
                        let (x, y) = parse_nth(&a)?;
                        c.structural.push(Structural::NthLastChild(x, y));
                    }
                    _ => return None, // :hover/:checked/:root/… — unsupported → drop the selector
                }
            }
            _ => return None,
        }
    }
    Some(c)
}

/// Parse the inside of an `[attr…]` selector (`attr`, `attr=v`, `attr~=v`, …).
fn parse_attr(inner: &str) -> Option<AttrSel> {
    let inner = inner.trim();
    for (pat, op) in [
        ("~=", AttrOp::Includes),
        ("|=", AttrOp::Dash),
        ("^=", AttrOp::Prefix),
        ("$=", AttrOp::Suffix),
        ("*=", AttrOp::Substr),
        ("=", AttrOp::Eq),
    ] {
        if let Some(p) = inner.find(pat) {
            let name = inner[..p].trim().to_ascii_lowercase();
            if name.is_empty() {
                return None;
            }
            let mut val = inner[p + pat.len()..].trim();
            // Drop a trailing case-sensitivity flag (`[a="x" i]`).
            if let Some(s) = val.strip_suffix(" i").or_else(|| val.strip_suffix(" s")) {
                val = s.trim();
            }
            if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            {
                val = &val[1..val.len() - 1];
            }
            return Some(AttrSel { name, op, val: val.to_string() });
        }
    }
    let name = inner.to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some(AttrSel { name, op: AttrOp::Exists, val: String::new() })
}

/// Parse an `An+B` micro-syntax (`2n+1`, `odd`, `even`, `3`, `-n+3`).
fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "even" => return Some((2, 0)),
        "odd" => return Some((2, 1)),
        _ => {}
    }
    if let Some(n) = s.find('n') {
        let a = match s[..n].trim() {
            "" | "+" => 1,
            "-" => -1,
            x => x.parse::<i32>().ok()?,
        };
        let rest = s[n + 1..].replace(' ', "");
        let b = if rest.is_empty() { 0 } else { rest.parse::<i32>().ok()? };
        Some((a, b))
    } else {
        Some((0, s.parse::<i32>().ok()?))
    }
}

/// CSS specificity packed as (id<<20)|(class<<10)|type — enough headroom.
fn specificity(compounds: &[Compound]) -> u32 {
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    let mut acc = |comp: &Compound| {
        if comp.id.is_some() {
            a += 1;
        }
        // classes, `[attr]` and pseudo-classes all count at the class level.
        b += (comp.classes.len() + comp.attrs.len() + comp.structural.len()) as u32;
        if comp.tag.is_some() {
            c += 1;
        }
        // A pseudo-element contributes like a type selector (css-cascade §5.8.3).
        if comp.pseudo != PseudoElem::None {
            c += 1;
        }
    };
    for comp in compounds {
        acc(comp);
        // `:not(x)` contributes its argument's specificity (css-selectors §16).
        for n in &comp.not {
            acc(n);
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
            attrs: Vec::new(),
        }
    }

    #[test]
    fn parses_and_matches_type_class_id() {
        let ss = parse("p { color: red } .lead { font-weight: bold } #main { color: blue }");
        assert!(!ss.matched(&info("p", None, &[]), &[], &[], 0, 1000.0).is_empty());
        assert!(!ss.matched(&info("div", None, &["lead"]), &[], &[], 0, 1000.0).is_empty());
        assert!(!ss.matched(&info("div", Some("main"), &[]), &[], &[], 0, 1000.0).is_empty());
        assert!(ss.matched(&info("span", None, &[]), &[], &[], 0, 1000.0).is_empty());
    }

    #[test]
    fn descendant_and_child_combinators() {
        let ss = parse("nav a { color: green } ul > li { color: teal }");
        let nav = info("nav", None, &[]);
        let div = info("div", None, &[]);
        let ul = info("ul", None, &[]);
        // nav a: matches an <a> with <nav> anywhere above
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone()], &[], 0, 1000.0).is_empty());
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone(), div.clone()], &[], 0, 1000.0).is_empty());
        assert!(ss.matched(&info("a", None, &[]), &[div.clone()], &[], 0, 1000.0).is_empty());
        // ul > li: <li> whose IMMEDIATE parent is <ul>
        assert!(!ss.matched(&info("li", None, &[]), &[ul.clone()], &[], 0, 1000.0).is_empty());
        assert!(ss.matched(&info("li", None, &[]), &[ul.clone(), div.clone()], &[], 0, 1000.0).is_empty());
    }

    #[test]
    fn specificity_ranks_id_over_class_over_type() {
        let ss = parse("p { color: a } p.x { color: b } #p { color: c }");
        let e = info("p", Some("p"), &["x"]);
        let mut m = ss.matched(&e, &[], &[], 0, 1000.0);
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
        assert!(ss.matched(&info("a", None, &[]), &[], &[], 0, 1000.0).is_empty());
        assert!(ss.matched(&info("input", None, &[]), &[], &[], 0, 1000.0).is_empty());
        // … and the plain "h1, h2" list still parsed.
        assert!(!ss.matched(&info("h2", None, &[]), &[], &[], 0, 1000.0).is_empty());
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
        let ss = collect_all(&dom, "p { color: red }", 800.0);
        let mut m = ss.matched(&info("p", None, &[]), &[], &[], 0, 1000.0);
        m.sort_by_key(|(spec, order, _)| (*spec, *order));
        assert_eq!(m.len(), 2, "both external + inline rules match");
        assert!(m[0].1 < m[1].1, "external rule has earlier document order");
    }

    #[test]
    fn collects_style_blocks_from_dom() {
        let dom = dom::parse("<html><head><style>p{color:red}</style></head>\
            <body><style>.x{color:blue}</style><p>hi</p></body></html>");
        let ss = collect(&dom, 800.0);
        assert!(!ss.matched(&info("p", None, &[]), &[], &[], 0, 1000.0).is_empty());
        assert!(!ss.matched(&info("span", None, &["x"]), &[], &[], 0, 1000.0).is_empty());
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
        assert_eq!(ss.matched(&e, &[], &[], 0, 1000.0).len(), 2, "media rule applies wide");
        // Narrow (500 < 768): only the base rule.
        assert_eq!(ss.matched(&e, &[], &[], 0, 500.0).len(), 1, "media rule dropped narrow");
        // An un-evaluable feature never matches (rule dropped both ways).
        let ss2 = parse("@media (prefers-color-scheme: dark) { .col { color: blue } }");
        assert!(ss2.matched(&e, &[], &[], 0, 1000.0).is_empty(), "unknown feature never applies");
    }
}
