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
//! not mis-applied — forward-compatible like a browser (docs/spec/CONFORMANCE.md).
//! External `<link>` stylesheets need a sub-resource fetch → later; the parser
//! + cascade here are exactly what that will reuse.

use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::dom::{Dom, Element, Node};

/// The identity a selector matches against: tag + id + classes + all attributes
/// (names lowercased) for `[attr]` selectors.
#[derive(Clone)]
pub struct ElemInfo<'a> {
    /// The live element. Borrowed, not snapshotted: a selector has to be able
    /// to look at an element's CHILDREN (`:empty`, `:has()`), and a copy of the
    /// tag/id/class triple never can. This is also the shape `querySelector`
    /// needs — matching against a live tree, not against copies of it — so
    /// there is one matcher for the cascade and for scripting, not two.
    pub el: &'a Element,
    /// `class` split once. Selector matching runs EVERY rule against this
    /// element, so re-splitting per rule would dominate the cascade; the slices
    /// borrow, so nothing is copied.
    classes: Vec<&'a str>,
    /// What only the runtime knows. `:checked`/`:disabled` read it today;
    /// `:hover`/`:focus` are the same mechanism and stay `false` until there is
    /// an event loop to flip them. Keeping them HERE rather than as scattered
    /// "never matches" special cases is what makes that a one-line change
    /// later instead of a hunt.
    pub state: ElemState,
}

/// Runtime state a selector can ask about (CSS Selectors 4 §4, §11).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ElemState {
    pub checked: bool,
    pub disabled: bool,
    pub focus: bool,
    pub hover: bool,
}

impl<'a> ElemInfo<'a> {
    pub fn of(el: &'a Element) -> ElemInfo<'a> {
        // What the DOCUMENT says. `:hover`/`:focus` have no document form and
        // stay false until an event loop sets them; live `checked` after a
        // click belongs to the form state and comes through `with_state`.
        // Not covered: a `<fieldset disabled>` disabling its descendants.
        ElemInfo::with_state(
            el,
            ElemState {
                checked: el.attr("checked").is_some(),
                disabled: el.attr("disabled").is_some(),
                ..ElemState::default()
            },
        )
    }

    pub fn with_state(el: &'a Element, state: ElemState) -> ElemInfo<'a> {
        let classes = el.attr("class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
        ElemInfo { el, classes, state }
    }

    pub fn tag(&self) -> &str {
        &self.el.tag
    }
    pub fn id(&self) -> Option<&str> {
        self.el.attr("id").map(str::trim).filter(|s| !s.is_empty())
    }
    pub fn seq(&self) -> u32 {
        self.el.seq
    }
    /// `:empty`: no element children and no non-whitespace text (Selectors 4
    /// §14.3 — white-space-only text nodes do not disqualify, which is what
    /// browsers do and what the `<td></td>` / `<p>\n</p>` idioms rely on).
    #[cfg(test)]
    pub fn clone_for_test(&self) -> ElemInfo<'a> {
        ElemInfo::with_state(self.el, self.state)
    }

    pub fn is_empty_element(&self) -> bool {
        self.el.children.iter().all(|n| match n {
            Node::Text(t) => t.trim().is_empty(),
            Node::Element(_) => false,
        })
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
        e.el.attrs.iter().any(|(k, v)| {
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
    /// The same five, counted only among siblings with the subject's tag.
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthOfType(i32, i32),
    NthLastOfType(i32, i32),
}

/// Where the subject sits among its element siblings: the 1-based index and
/// the count, and the same pair counted only among siblings that share its tag
/// (`:*-of-type`). The of-type half needs the FULL sibling list, not just the
/// preceding one — reachable only since the matcher borrows live elements and
/// can look at the parent's children.
#[derive(Clone, Copy)]
struct SibCtx<'a> {
    idx: u32,
    count: u32,
    idx_of_type: u32,
    count_of_type: u32,
    /// The subject's parent, for looking at siblings that come AFTER it —
    /// `:has(+ x)`, `:has(~ x)`. `None` when the caller supplied no ancestors.
    parent: Option<&'a Element>,
}

impl Structural {
    fn matches(&self, ctx: Option<SibCtx>) -> bool {
        let Some(SibCtx { idx, count, idx_of_type, count_of_type, .. }) = ctx else { return false }; // no sibling context → can't evaluate
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
            Structural::FirstOfType => idx_of_type == 1,
            Structural::LastOfType => idx_of_type == count_of_type,
            Structural::OnlyOfType => count_of_type == 1,
            Structural::NthOfType(a, b) => nth(a, b, idx_of_type),
            Structural::NthLastOfType(a, b) => {
                count_of_type >= idx_of_type && nth(a, b, count_of_type - idx_of_type + 1)
            }
        }
    }
}

/// One alternative inside `:has(…)`: a combinator and the compound it applies
/// to, relative to the subject. Measured against the CSS four real pages load,
/// **223 of 243** `:has()` arguments are exactly this shape (178 descendant,
/// 20 `+`, 18 `>`, 7 `~`); anything more complex still drops its selector, the
/// same as before, so nothing regresses.
struct HasArg {
    comb: Comb,
    compound: Compound,
}

impl HasArg {
    /// Does anything in the scope this combinator opens match? `ctx` supplies
    /// the subject's parent, needed only for the sibling combinators — so a
    /// descendant/child `:has()` works even on an ancestor compound, where
    /// there is no sibling context.
    fn matches(&self, e: &ElemInfo, ctx: Option<SibCtx>) -> bool {
        let hit = |cand: &Element| self.compound.matches(&ElemInfo::of(cand), None);
        match self.comb {
            Comb::Descendant => {
                fn any(nodes: &[Node], f: &dyn Fn(&Element) -> bool) -> bool {
                    nodes.iter().any(|n| match n {
                        Node::Element(c) => f(c) || any(&c.children, f),
                        Node::Text(_) => false,
                    })
                }
                any(&e.el.children, &hit)
            }
            Comb::Child => e.el.children.iter().any(|n| match n {
                Node::Element(c) => hit(c),
                Node::Text(_) => false,
            }),
            Comb::Adjacent | Comb::General => {
                let Some(SibCtx { parent: Some(p), .. }) = ctx else { return false };
                let after = p
                    .children
                    .iter()
                    .filter_map(|n| match n {
                        Node::Element(c) => Some(c),
                        Node::Text(_) => None,
                    })
                    .skip_while(|c| c.seq != e.seq())
                    .skip(1);
                if self.comb == Comb::Adjacent {
                    after.into_iter().take(1).any(|c| hit(c))
                } else {
                    after.into_iter().any(|c| hit(c))
                }
            }
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
    /// `:is(…)`/`:matches(…)` groups: each group is a list of alternative
    /// compounds; the compound matches only if EACH group has at least one
    /// matching alternative. Contributes its most specific argument's
    /// specificity (like a class group). Only compound alternatives are
    /// supported (no combinators inside) — enough for MediaWiki/Bootstrap.
    is_groups: Vec<Vec<Compound>>,
    /// `:where(…)` groups: match like `:is()` but contribute ZERO specificity.
    where_groups: Vec<Vec<Compound>>,
    structural: Vec<Structural>,
    /// `:root` — the document's root element. In an HTML document that is
    /// always `<html>`, so this is a tag test that keeps pseudo-class
    /// specificity.
    root: bool,
    /// `:empty` — no children other than white-space-only text (Selectors 4
    /// §14.3). Only expressible since the matcher borrows the live element:
    /// a snapshot of tag/id/class can never answer "what is inside".
    empty: bool,
    /// State pseudo-classes, each `Some(want)` when the selector asks for it.
    /// They read `ElemInfo::state` — the same field `:hover`/`:focus` will use
    /// once there is an event loop, which is why they live together.
    checked: Option<bool>,
    disabled: Option<bool>,
    /// One entry per `:has()` on this compound; the inner list is its
    /// comma-separated alternatives, so an entry matches if ANY of them does
    /// and several `:has()` all have to hold.
    has: Vec<Vec<HasArg>>,
    /// `::before`/`::after` on this compound (only valid on the LAST compound
    /// of a selector — checked in `parse_selector`).
    pseudo: PseudoElem,
}

impl Compound {
    /// `ctx = Some((index, count))` provides the sibling position for structural
    /// pseudo-classes; `None` (an ancestor with no known position) makes any
    /// structural pseudo fail (the selector is dropped rather than mis-applied).
    fn matches(&self, e: &ElemInfo, ctx: Option<SibCtx>) -> bool {
        if let Some(t) = &self.tag {
            if t != e.tag() {
                return false;
            }
        }
        if let Some(id) = &self.id {
            if e.id() != Some(id.as_str()) {
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
        if self.root && e.tag() != "html" {
            return false;
        }
        if self.empty && !e.is_empty_element() {
            return false;
        }
        if self.checked.is_some_and(|w| w != e.state.checked) {
            return false;
        }
        if self.disabled.is_some_and(|w| w != e.state.disabled) {
            return false;
        }
        // :not(x) — none of the negated compounds may match.
        if self.not.iter().any(|n| n.matches(e, ctx)) {
            return false;
        }
        // :is(…)/:where(…) — every group must have at least one matching
        // alternative (an empty group, e.g. all-unsupported args, matches
        // nothing → the compound never matches).
        if !self.is_groups.iter().chain(self.where_groups.iter()).all(|group| group.iter().any(|alt| alt.matches(e, ctx))) {
            return false;
        }
        // :has() LAST. It is the only test that walks a subtree, and every
        // cheap test above has already ruled out the elements it would walk
        // for nothing — `.foo:has(.bar)` descends only into elements that are
        // actually `.foo`.
        self.has.iter().all(|group| group.iter().any(|a| a.matches(e, ctx)))
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
        // sibling index (preceding count + 1) and the total sibling count —
        // and, for `:*-of-type`, against the same pair restricted to its tag.
        // The of-type COUNT needs siblings that come after the subject too, so
        // it is read off the parent's children.
        let tag = subject.tag();
        let idx_of_type = prev_siblings.iter().filter(|p| p.tag() == tag).count() as u32 + 1;
        let count_of_type = ancestors
            .last()
            .map(|p| {
                p.el.children
                    .iter()
                    .filter(|n| matches!(n, Node::Element(e) if e.tag == tag))
                    .count() as u32
            })
            // No parent on the path (the root, or a caller that did not supply
            // one): the subject is the only element of its type we can see.
            .unwrap_or(idx_of_type);
        let subj_ctx = Some(SibCtx {
            idx: prev_siblings.len() as u32 + 1,
            count: sib_count,
            idx_of_type,
            count_of_type,
            parent: ancestors.last().map(|p| p.el),
        });
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
    /// `prefers-color-scheme` — `Some(true)` wants dark, `Some(false)` light.
    scheme_dark: Option<bool>,
    understood: bool,
    /// A leading `not`, which negates the WHOLE query (Media Queries 4 §3.1),
    /// not one feature of it. `@media not screen and (max-width: 480px)` is
    /// how a mobile-first page states its desktop rules — dropping it leaves
    /// a 1400px window rendering the phone layout, which is what Google's
    /// consent page did: buttons at full window width, both the phone and the
    /// desktop set of them on screen at once.
    negated: bool,
}

impl MediaCond {
    fn matches(&self, m: Media) -> bool {
        // A query we did not understand never matches — not even negated.
        // `not <something we cannot evaluate>` is not "true", it is unknown,
        // and unknown has to fail closed in both directions.
        if !self.understood {
            return false;
        }
        let hit = self.min_width.is_none_or(|v| m.width >= v)
            && self.max_width.is_none_or(|v| m.width <= v)
            && self.scheme_dark.is_none_or(|want| want == m.dark);
        hit != self.negated
    }
}

/// What the page is being rendered INTO — everything `@media` can ask about.
/// One value instead of a widening list of parameters, and it is `Copy`, so it
/// threads through the cascade the way `viewport_w` used to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Media {
    pub width: f32,
    /// The user's colour-scheme preference. On this system that IS the page
    /// theme: the shell resolves it from the compositor palette.
    pub dark: bool,
}

impl Media {
    pub fn new(width: f32, dark: bool) -> Media {
        Media { width, dark }
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
    /// Selectors targeting real elements.
    normal: Index,
    /// Selectors ending in `::before`/`::after`, kept apart because the
    /// cascade runs THREE times per element (the element, then each
    /// generated box). Sharing one index made each pass walk the other
    /// two passes' candidates only to reject them.
    pseudo: Index,
    /// Every `url(…)` appearing anywhere in the sheet, keyed by `url_key`.
    ///
    /// `ComputedStyle` is `Copy`, so it cannot carry the URL itself — it
    /// stores the key and looks the string up here. Collecting them from the
    /// source text rather than threading an interner through the cascade
    /// keeps `apply_one` a pure function: a URL that wins the cascade is by
    /// definition present in the text it was parsed from.
    urls: BTreeMap<u64, String>,
}

/// Stable 64-bit key for a `url()` value (FNV-1a). Case-sensitive on purpose:
/// a `data:` payload's base64 is case-significant.
pub fn url_key(url: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in url.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The next `url(…)` in `text` at or after `from`, as `(url, index after it)`.
///
/// One scanner for both callers, because the ways to get this wrong are the
/// same in each: a QUOTED url may legally contain `)`, a url is rarely the
/// whole value (`background: red url(x) no-repeat`), and a quoted one may
/// carry BACKSLASH-ESCAPED quotes — which is exactly how an inline SVG
/// `data:` URI is written (`url("data:image/svg+xml,<svg xmlns=\"…\">")`).
/// Stopping at the first inner quote truncates the payload into something that
/// still looks like a URL and silently decodes to nothing.
fn url_at(text: &str, from: usize) -> Option<(Cow<'_, str>, usize)> {
    let p = text[from..].find("url(")? + from;
    let open = p + 4;
    let b = text.as_bytes();
    let (start, end, after) = match b.get(open) {
        Some(&q) if q == b'"' || q == b'\'' => {
            let s = open + 1;
            let mut i = s;
            while i < b.len() && b[i] != q {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            let e = i.min(b.len());
            (s, e, (e + 1).min(text.len()))
        }
        _ => {
            let e = text[open..].find(')').map(|e| open + e).unwrap_or(text.len());
            (open, e, (e + 1).min(text.len()))
        }
    };
    Some((unescape(text.get(start..end)?.trim()), after.max(open)))
}

/// Undo CSS string escaping (`\"` → `"`). Hex escapes (`\41`) are left alone:
/// they do not occur in the URLs real pages ship, and guessing at one would
/// corrupt more than it fixes.
fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some(n) if n.is_ascii_hexdigit() => {
                    out.push('\\');
                    out.push(n);
                }
                Some(n) => out.push(n),
                None => {}
            },
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// The first `url(…)` in a declaration value, unquoted and unescaped.
pub fn url_value(v: &str) -> Option<Cow<'_, str>> {
    let (u, _) = url_at(v, 0)?;
    (!u.is_empty()).then_some(u)
}

/// Record every `url(…)` in a block of CSS text into `out`.
///
/// Runs over the raw text, not the parsed rules, because `url()` can appear in
/// any property (`background-image`, `mask-image`, `list-style-image`, …) and
/// we want the table complete before the cascade picks a winner.
fn collect_urls(css: &str, out: &mut BTreeMap<u64, String>) {
    let mut i = 0usize;
    while let Some((u, next)) = url_at(css, i) {
        if !u.is_empty() {
            out.entry(url_key(&u)).or_insert_with(|| u.into_owned());
        }
        i = next;
    }
}

/// Candidate `(rule, selector)` pairs bucketed by the most selective simple
/// selector of a selector's rightmost compound. A selector lives in exactly
/// one bucket, so collecting several buckets cannot produce duplicates.
#[derive(Default)]
struct Index {
    by_id: BTreeMap<String, Vec<(u32, u32)>>,
    by_class: BTreeMap<String, Vec<(u32, u32)>>,
    by_tag: BTreeMap<String, Vec<(u32, u32)>>,
    /// Rightmost compound names no tag, id or class (`*`, `[attr]`, a bare
    /// `:not(...)`) — must be tried for every element.
    universal: Vec<(u32, u32)>,
}

impl Index {
    fn insert(&mut self, key: (u32, u32), last: &Compound) {
        // Most selective first: an id narrows far more than a tag.
        if let Some(id) = &last.id {
            self.by_id.entry(id.clone()).or_default().push(key);
        } else if let Some(cls) = last.classes.first() {
            self.by_class.entry(cls.clone()).or_default().push(key);
        } else if let Some(tag) = &last.tag {
            self.by_tag.entry(tag.clone()).or_default().push(key);
        } else if last.root {
            self.by_tag.entry("html".into()).or_default().push(key);
        } else {
            self.universal.push(key);
        }
    }

    fn candidates(&self, subject: &ElemInfo, out: &mut Vec<(u32, u32)>) {
        if let Some(id) = subject.id() {
            if let Some(v) = self.by_id.get(id) {
                out.extend_from_slice(v);
            }
        }
        for c in &subject.classes {
            if let Some(v) = self.by_class.get(*c) {
                out.extend_from_slice(v);
            }
        }
        if let Some(v) = self.by_tag.get(subject.tag()) {
            out.extend_from_slice(v);
        }
        out.extend_from_slice(&self.universal);
    }
}

impl Stylesheet {
    pub fn empty() -> Stylesheet {
        Stylesheet {
            rules: Vec::new(),
            normal: Index::default(),
            pseudo: Index::default(),
            urls: BTreeMap::new(),
        }
    }

    /// The `url()` string behind a key held by a `ComputedStyle`.
    pub fn url(&self, key: u64) -> Option<&str> {
        self.urls.get(&key).map(|s| s.as_str())
    }

    /// Record `url()`s from text outside the sheet itself (inline `style`
    /// attributes), so the cascade can resolve a key that came from there.
    pub fn add_urls(&mut self, text: &str) {
        collect_urls(text, &mut self.urls);
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
                if sel.pseudo == PseudoElem::None {
                    self.normal.insert(key, last);
                } else {
                    self.pseudo.insert(key, last);
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
        media: Media,
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        self.matched_filtered(subject, ancestors, prev_siblings, sib_count, media, PseudoElem::None)
    }

    /// Same as `matched`, but for `subject`'s `::before`/`::after` generated
    /// box — only selectors ending in that pseudo-element are considered.
    pub fn matched_pseudo<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        media: Media,
        pseudo: PseudoElem,
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        self.matched_filtered(subject, ancestors, prev_siblings, sib_count, media, pseudo)
    }

    fn matched_filtered<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        media: Media,
        want: PseudoElem,
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        // Only selectors whose rightmost compound could match this element are
        // worth testing — that is what the index buys. Everything else is
        // unchanged: same tests, same specificity, same result.
        let mut cands: Vec<(u32, u32)> = Vec::new();
        let index = if want == PseudoElem::None { &self.normal } else { &self.pseudo };
        index.candidates(subject, &mut cands);
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
                Some(conds) => conds.iter().any(|c| c.matches(media)),
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
pub fn collect(dom: &Dom, media: Media) -> Stylesheet {
    collect_all(dom, "", media)
}

/// Author stylesheet = already-fetched external `<link>` CSS (document order:
/// `<head>` first) followed by inline `<style>` blocks. The shell fetches the
/// linked files (the engine is host-free) and hands their bytes in as `external`.
pub fn collect_all(dom: &Dom, external: &str, media: Media) -> Stylesheet {
    let mut css = String::from(external);
    css.push('\n');
    gather_style_text(&dom.root, &mut css);
    if css.trim().is_empty() {
        let mut sheet = Stylesheet::empty();
        gather_inline_urls(&dom.root, &mut sheet);
        return sheet;
    }
    // Expand CSS custom properties (`var(--x)`) as a pre-pass so the parser +
    // cascade never see variables — modern sites (Bootstrap's `--bs-*`) lean
    // on them heavily.
    // The document root's classes decide which of a site's per-preference
    // custom-property blocks actually applies (MediaWiki ships one per user
    // setting on `html.…-clientpref-N`).
    let root = dom.root_element();
    let root_class_attr = root.attr("class").unwrap_or("");
    let root_classes: Vec<&str> = root_class_attr.split_whitespace().collect();
    let css = crate::vars::resolve_vars(&css, media, &root_classes);
    let mut sheet = parse(&css);
    // An inline `style="background-image:url(…)"` never passes through the
    // sheet text, so its URL would have no entry to resolve against.
    gather_inline_urls(&dom.root, &mut sheet);
    sheet
}

/// Add every `url()` found in an inline `style` attribute to the sheet's table.
fn gather_inline_urls(el: &Element, sheet: &mut Stylesheet) {
    if let Some(s) = el.attr("style") {
        if s.contains("url(") {
            sheet.add_urls(s);
        }
    }
    for c in &el.children {
        if let Node::Element(e) = c {
            gather_inline_urls(e, sheet);
        }
    }
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
    let mut urls = BTreeMap::new();
    collect_urls(&css, &mut urls);
    let mut sheet = Stylesheet { rules, normal: Index::default(), pseudo: Index::default(), urls };
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
        // `transparent` IS a supported colour — ask the value parser, not the
        // one that folds transparency into "no value".
        return crate::color::parse_color_val(val).is_some();
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
            let mut cond = MediaCond { min_width: None, max_width: None, scheme_dark: None, understood: true, negated: false };
            let mut ql = q.to_ascii_lowercase();
            // `not` leads the query and covers all of it.
            if let Some(rest) = ql.trim_start().strip_prefix("not ") {
                cond.negated = true;
                ql = rest.to_string();
            }
            for part in ql.split("and") {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                if let Some(inner) = p.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
                    let mut kv = inner.splitn(2, ':');
                    let feat = kv.next().unwrap_or("").trim();
                    let val = kv.next().unwrap_or("").trim();
                    // A width feature whose value we can't parse must NOT leave
                    // the bound `None` (that matches every viewport) — mark the
                    // whole query not-understood so it fails closed instead.
                    match feat {
                        "min-width" => match parse_media_px(val) {
                            Some(px) => cond.min_width = Some(px),
                            None => cond.understood = false,
                        },
                        "max-width" => match parse_media_px(val) {
                            Some(px) => cond.max_width = Some(px),
                            None => cond.understood = false,
                        },
                        // `no-preference` was dropped from the spec and never
                        // matches; anything else is a value we don't know, so
                        // the query fails closed like any other.
                        "prefers-color-scheme" => match val {
                            "dark" => cond.scheme_dark = Some(true),
                            "light" => cond.scheme_dark = Some(false),
                            _ => cond.understood = false,
                        },
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
/// Does a media query text apply to `m`? Used by `@media` preludes and by
/// `<source media=…>` in a `<picture>`.
pub fn media_matches(prelude: &str, m: Media) -> bool {
    parse_media_query(prelude).iter().any(|c| c.matches(m))
}

/// A media-feature `<length>` — px only (Bootstrap/WP breakpoints are all px).
fn parse_px(v: &str) -> Option<f32> {
    let v = v.trim();
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
}

/// A media-feature length: a plain `<px>` or a `calc()` of `±<px>` terms.
/// MediaWiki (and others) express breakpoints as `calc(640px - 1px)`; without
/// `calc()` support the value fails to parse, `max_width` stays `None`, and the
/// `@media (max-width: …)` block then matches EVERY viewport — leaking mobile
/// rules (e.g. `.wikitable{float:none}`) onto the desktop layout.
fn parse_media_px(v: &str) -> Option<f32> {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix("calc(").and_then(|s| s.strip_suffix(')')) {
        // Sum of whitespace-separated `±<px>` terms (CSS requires spaces around
        // the `+`/`-` operators, so tokenising on whitespace is sufficient).
        let mut acc = 0.0f32;
        let mut sign = 1.0f32;
        let mut have = false;
        for tok in inner.split_whitespace() {
            match tok {
                "+" => sign = 1.0,
                "-" => sign = -1.0,
                _ => {
                    acc += sign * parse_px(tok)?;
                    sign = 1.0;
                    have = true;
                }
            }
        }
        return have.then_some(acc);
    }
    parse_px(v)
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
    split_top_level_commas(text).into_iter().filter_map(|s| parse_selector(s.trim())).collect()
}

/// Split a comma-separated selector list on TOP-LEVEL commas only — commas
/// inside `[…]` or `:is(…)`/`:where(…)`/`:not(…)` parentheses do not separate.
/// A naive `split(',')` would tear `:is(div,table,ul)` apart.
fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            ',' if depth == 0 => {
                out.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(text[start..].trim());
    out
}

/// Parse an `:is()`/`:where()` argument (a forgiving selector list) into its
/// alternative compounds. Per css-selectors §4 forgiving parsing, an
/// unsupported alternative (e.g. `:hover`, or one with a combinator we don't
/// model) is dropped rather than invalidating the whole list.
fn parse_compound_list(arg: &str) -> Vec<Compound> {
    split_top_level_commas(arg).into_iter().filter_map(|s| parse_compound(s.trim())).collect()
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
        is_groups: Vec::new(),
        where_groups: Vec::new(),
        structural: Vec::new(),
        root: false,
        empty: false,
        checked: None,
        disabled: None,
        has: Vec::new(),
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
                    // Balanced close paren, so a nested `:is(:nth-child(2n))`
                    // doesn't terminate on the inner `)`.
                    let mut d = 0i32;
                    let mut end = None;
                    for (k, &ch) in b[i..].iter().enumerate() {
                        match ch {
                            b'(' => d += 1,
                            b')' => {
                                d -= 1;
                                if d == 0 {
                                    end = Some(i + k);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let end = end?;
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
                    ("not", Some(a)) => {
                        // A state a static render never enters makes the
                        // negation trivially true: drop the clause and KEEP the
                        // rule. That is what carries the "visually hidden"
                        // idiom — `.skip-link:not(:focus){clip:rect(1px,1px,1px,1px)}`
                        // is how a page hides a link until it is tabbed to, and
                        // dropping the whole rule leaves the link on the page
                        // for everyone to read.
                        let a = a.trim();
                        if !never_matches(a) {
                            c.not.push(parse_compound(a)?);
                        }
                    }
                    // `:is()`/`:where()` (and the legacy `:matches()` alias) —
                    // forgiving compound-alternative lists.
                    ("is" | "matches", Some(a)) => c.is_groups.push(parse_compound_list(&a)),
                    // `:has(<relative-selector-list>)`. An alternative we
                    // cannot express drops the whole selector — which is what
                    // an unknown pseudo-class did before, so nothing regresses.
                    ("has", Some(a)) => c.has.push(parse_has_list(&a)?),
                    ("where", Some(a)) => c.where_groups.push(parse_compound_list(&a)),
                    ("root", None) => c.root = true,
                    ("empty", None) => c.empty = true,
                    ("checked", None) => c.checked = Some(true),
                    ("disabled", None) => c.disabled = Some(true),
                    ("enabled", None) => c.disabled = Some(false),
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
                    ("first-of-type", None) => c.structural.push(Structural::FirstOfType),
                    ("last-of-type", None) => c.structural.push(Structural::LastOfType),
                    ("only-of-type", None) => c.structural.push(Structural::OnlyOfType),
                    ("nth-of-type", Some(a)) => {
                        let (x, y) = parse_nth(&a)?;
                        c.structural.push(Structural::NthOfType(x, y));
                    }
                    ("nth-last-of-type", Some(a)) => {
                        let (x, y) = parse_nth(&a)?;
                        c.structural.push(Structural::NthLastOfType(x, y));
                    }
                    _ => return None, // :hover/:checked/… — unsupported → drop the selector
                }
            }
            _ => return None,
        }
    }
    Some(c)
}

/// A pseudo-class naming an interaction state this engine never enters, so a
/// selector demanding it can never match. Only meaningful inside `:not()`,
/// where it makes the negation always true.
fn never_matches(sel: &str) -> bool {
    matches!(
        sel.trim().to_ascii_lowercase().as_str(),
        ":hover" | ":focus" | ":focus-visible" | ":focus-within" | ":active" | ":target" | ":visited"
    )
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

/// `:has()`'s argument: comma-separated relative selectors, each an optional
/// leading combinator plus ONE compound. Returns `None` — dropping the whole
/// selector — for anything else, e.g. `:has(> a > span)`.
fn parse_has_list(arg: &str) -> Option<Vec<HasArg>> {
    let mut out = Vec::new();
    for part in split_top_level_commas(arg) {
        let part = part.trim();
        let (comb, rest) = match part.as_bytes().first() {
            Some(b'>') => (Comb::Child, &part[1..]),
            Some(b'+') => (Comb::Adjacent, &part[1..]),
            Some(b'~') => (Comb::General, &part[1..]),
            _ => (Comb::Descendant, part),
        };
        let rest = rest.trim();
        // One compound only: any inner combinator (a space included) is out of
        // scope. 20 of 243 real arguments; they keep failing as they did.
        if rest.is_empty() || rest.contains([' ', '>', '+', '~', '\t']) {
            return None;
        }
        out.push(HasArg { comb, compound: parse_compound(rest)? });
    }
    (!out.is_empty()).then_some(out)
}

/// One compound's specificity as `(id, class, type)` counts. Recurses through
/// `:not()` (its argument's specificity) and `:is()` (its MOST specific
/// argument's); `:where()` adds nothing (css-selectors §16/§4).
fn compound_spec(comp: &Compound) -> (u32, u32, u32) {
    let mut a = comp.id.is_some() as u32;
    // classes, `[attr]` and pseudo-classes all count at the class level.
    let mut b = (comp.classes.len()
        + comp.attrs.len()
        + comp.structural.len()
        + comp.root as usize
        + comp.empty as usize
        + comp.checked.is_some() as usize
        + comp.disabled.is_some() as usize) as u32;
    // tag + a pseudo-element each count like a type selector (css-cascade §5.8.3).
    let mut c = comp.tag.is_some() as u32 + (comp.pseudo != PseudoElem::None) as u32;
    for n in &comp.not {
        let (na, nb, nc) = compound_spec(n);
        a += na;
        b += nb;
        c += nc;
    }
    for group in &comp.is_groups {
        if let Some((ma, mb, mc)) = group.iter().map(compound_spec).max() {
            a += ma;
            b += mb;
            c += mc;
        }
    }
    // `:has()` contributes its most specific argument, as `:is()` does
    // (Selectors 4 §17).
    for group in &comp.has {
        if let Some((ma, mb, mc)) = group.iter().map(|h| compound_spec(&h.compound)).max() {
            a += ma;
            b += mb;
            c += mc;
        }
    }
    (a, b, c)
}

/// CSS specificity packed as (id<<20)|(class<<10)|type — enough headroom.
fn specificity(compounds: &[Compound]) -> u32 {
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    for comp in compounds {
        let (ca, cb, cc) = compound_spec(comp);
        a += ca;
        b += cb;
        c += cc;
    }
    (a << 20) | (b << 10) | c
}

/// Split a declaration block on its top-level `;`.
///
/// NOT `str::split(';')`: a semicolon inside a string or a `url()` belongs to
/// the value. `url("data:image/svg+xml;utf8,<svg …>")` is the form icon
/// systems ship, and cutting it at `;utf8` leaves a declaration that still
/// parses — it just points at nothing.
pub fn split_decls(body: &str) -> Vec<&str> {
    let b = body.as_bytes();
    let (mut out, mut start) = (Vec::new(), 0usize);
    let (mut depth, mut quote) = (0i32, 0u8);
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'\\' if quote != 0 => i += 1, // escaped char inside a string
            q @ (b'"' | b'\'') if quote == 0 => quote = q,
            q if quote != 0 && q == quote => quote = 0,
            b'(' if quote == 0 => depth += 1,
            b')' if quote == 0 => depth = (depth - 1).max(0),
            b';' if quote == 0 && depth == 0 => {
                out.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&body[start.min(body.len())..]);
    out
}

fn parse_decls(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for decl in split_decls(body) {
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

    /// A standalone element to match against. `ElemInfo` borrows a live node
    /// now, so the tree has to outlive the `ElemInfo` — leaked here so the
    /// assertions can keep passing `info(...)` inline. A unit test process
    /// exits before that matters.
    fn info(tag: &str, id: Option<&str>, classes: &[&str]) -> ElemInfo<'static> {
        let mut h = alloc::format!("<{tag}");
        if let Some(i) = id {
            h += &alloc::format!(" id=\"{i}\"");
        }
        if !classes.is_empty() {
            h += &alloc::format!(" class=\"{}\"", classes.join(" "));
        }
        h += &alloc::format!("></{tag}>");
        let dom: &'static dom::Dom = alloc::boxed::Box::leak(alloc::boxed::Box::new(dom::parse(&h)));
        fn find<'x>(el: &'x dom::Element, tag: &str) -> Option<&'x dom::Element> {
            if el.tag == tag {
                return Some(el);
            }
            el.children.iter().find_map(|n| match n {
                dom::Node::Element(e) => find(e, tag),
                _ => None,
            })
        }
        ElemInfo::of(find(&dom.root, tag).expect("element"))
    }

    #[test]
    fn has_looks_into_the_subtree_and_forward_at_siblings() {
        let dom = dom::parse(
            "<div id=root><section id=a><p><img></p></section>\
             <section id=b><p>text</p></section>\
             <section id=c></c></section><span id=d></span></div>",
        );
        fn find<'x>(el: &'x dom::Element, id: &str) -> Option<&'x dom::Element> {
            if el.attr("id") == Some(id) { return Some(el); }
            el.children.iter().find_map(|n| match n {
                dom::Node::Element(e) => find(e, id),
                _ => None,
            })
        }
        let root = find(&dom.root, "root").unwrap();
        let kids: Vec<&dom::Element> = root.children.iter().filter_map(|n| match n {
            dom::Node::Element(e) => Some(e),
            _ => None,
        }).collect();
        let hit = |sel: &str, id: &str| {
            let ss = parse(&alloc::format!("{sel} {{ color: red }}"));
            let i = kids.iter().position(|e| e.attr("id") == Some(id)).unwrap();
            let prev: Vec<ElemInfo> = kids[..i].iter().map(|e| ElemInfo::of(e)).collect();
            !ss.matched(
                &ElemInfo::of(kids[i]),
                &[ElemInfo::of(root)],
                &prev,
                kids.len() as u32,
                Media::new(1000.0, false),
            ).is_empty()
        };
        // Descendant — the default, and 178 of 243 real uses.
        assert!(hit("section:has(img)", "a"));
        assert!(!hit("section:has(img)", "b"));
        // Child: <img> is a grandchild, so `> img` must NOT match.
        assert!(!hit("section:has(> img)", "a"));
        assert!(hit("section:has(> p)", "a"));
        // Forward siblings need the parent, which the context now carries.
        assert!(hit("section:has(+ section)", "a"));
        assert!(!hit("section:has(+ section)", "c"));
        assert!(hit("section:has(~ span)", "a"));
        assert!(!hit("span:has(~ section)", "d"));
        // A comma list is an OR.
        assert!(hit("section:has(img, blockquote)", "a"));
        // An argument we cannot express drops the selector rather than
        // mis-applying it — no rule, so no match.
        assert!(!hit("section:has(> p > img)", "a"));
    }

    #[test]
    fn state_pseudo_classes_read_the_element_state() {
        let dom = dom::parse(
            "<form><input type=checkbox checked><input type=checkbox><input disabled></form>",
        );
        fn inputs<'x>(el: &'x dom::Element, out: &mut Vec<&'x dom::Element>) {
            if el.tag == "input" { out.push(el); }
            for n in &el.children {
                if let dom::Node::Element(e) = n { inputs(e, out); }
            }
        }
        let mut v = Vec::new();
        inputs(&dom.root, &mut v);
        let hit = |sel: &str, i: usize| {
            let ss = parse(&alloc::format!("{sel} {{ color: red }}"));
            !ss.matched(&ElemInfo::of(v[i]), &[], &[], 3, Media::new(1000.0, false)).is_empty()
        };
        assert!(hit("input:checked", 0));
        assert!(!hit("input:checked", 1));
        assert!(hit("input:disabled", 2));
        assert!(!hit("input:disabled", 0));
        // `:enabled` is the negation, not "no disabled selector at all".
        assert!(hit("input:enabled", 0));
        assert!(!hit("input:enabled", 2));
    }

    #[test]
    fn of_type_counts_only_siblings_with_the_same_tag() {
        // `:nth-last-of-type` and `:only-of-type` need siblings that come AFTER
        // the subject, which is readable off the parent's children only because
        // the matcher borrows live elements.
        let html = "<div><p>a</p><span>s</span><p>b</p><span>t</span><p>c</p></div>";
        let dom = dom::parse(html);
        fn kids<'x>(el: &'x dom::Element) -> Vec<&'x dom::Element> {
            el.children.iter().filter_map(|n| match n {
                dom::Node::Element(e) => Some(e),
                _ => None,
            }).collect()
        }
        fn find_div<'x>(el: &'x dom::Element) -> Option<&'x dom::Element> {
            if el.tag == "div" { return Some(el); }
            el.children.iter().find_map(|n| match n {
                dom::Node::Element(e) => find_div(e),
                _ => None,
            })
        }
        let div = find_div(&dom.root).expect("div");
        let parent = ElemInfo::of(div);
        let ks = kids(div);
        let hit = |sel: &str, i: usize| {
            let ss = parse(&alloc::format!("{sel} {{ color: red }}"));
            let prev: Vec<ElemInfo> = ks[..i].iter().map(|e| ElemInfo::of(e)).collect();
            let subj = ElemInfo::of(ks[i]);
            !ss.matched(&subj, &[parent.clone_for_test()], &prev, ks.len() as u32, Media::new(1000.0, false)).is_empty()
        };
        // <p> elements are at positions 0, 2, 4 → of-type indices 1, 2, 3.
        assert!(hit("p:first-of-type", 0));
        assert!(!hit("p:first-of-type", 2));
        assert!(hit("p:nth-of-type(2)", 2));
        assert!(hit("p:last-of-type", 4));
        assert!(hit("p:nth-last-of-type(1)", 4));
        assert!(!hit("p:only-of-type", 0));
        // `:first-child` is NOT the same question: the second <span> is the
        // fourth child but only the second of its type.
        assert!(hit("span:nth-of-type(2)", 3));
        assert!(!hit("span:nth-child(2)", 3));
    }

    #[test]
    fn empty_matches_only_a_childless_element() {
        // `:empty` needs to see INSIDE the element — impossible while the
        // matcher took a snapshot of tag/id/class, which is why it used to
        // drop its whole selector.
        let ss = parse("td:empty { color: red }");
        let hit = |html: &str| {
            let dom = dom::parse(html);
            fn find<'x>(el: &'x dom::Element, tag: &str) -> Option<&'x dom::Element> {
                if el.tag == tag { return Some(el); }
                el.children.iter().find_map(|n| match n {
                    dom::Node::Element(e) => find(e, tag),
                    _ => None,
                })
            }
            let el = ElemInfo::of(find(&dom.root, "td").expect("td"));
            !ss.matched(&el, &[], &[], 1, Media::new(1000.0, false)).is_empty()
        };
        assert!(hit("<table><tr><td></td></tr></table>"), "no children at all");
        assert!(hit("<table><tr><td>\n  </td></tr></table>"), "whitespace-only text does not count");
        assert!(!hit("<table><tr><td>x</td></tr></table>"), "text disqualifies");
        assert!(!hit("<table><tr><td><span></span></td></tr></table>"), "an element child disqualifies");
    }

    #[test]
    fn parses_and_matches_type_class_id() {
        let ss = parse("p { color: red } .lead { font-weight: bold } #main { color: blue }");
        assert!(!ss.matched(&info("p", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(!ss.matched(&info("div", None, &["lead"]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(!ss.matched(&info("div", Some("main"), &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(ss.matched(&info("span", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
    }

    #[test]
    fn descendant_and_child_combinators() {
        let ss = parse("nav a { color: green } ul > li { color: teal }");
        let nav = info("nav", None, &[]);
        let div = info("div", None, &[]);
        let ul = info("ul", None, &[]);
        // nav a: matches an <a> with <nav> anywhere above
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone()], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone(), div.clone()], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(ss.matched(&info("a", None, &[]), &[div.clone()], &[], 0, Media::new(1000.0, false)).is_empty());
        // ul > li: <li> whose IMMEDIATE parent is <ul>
        assert!(!ss.matched(&info("li", None, &[]), &[ul.clone()], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(ss.matched(&info("li", None, &[]), &[ul.clone(), div.clone()], &[], 0, Media::new(1000.0, false)).is_empty());
    }

    #[test]
    fn specificity_ranks_id_over_class_over_type() {
        let ss = parse("p { color: a } p.x { color: b } #p { color: c }");
        let e = info("p", Some("p"), &["x"]);
        let mut m = ss.matched(&e, &[], &[], 0, Media::new(1000.0, false));
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
        assert!(ss.matched(&info("a", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(ss.matched(&info("input", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
        // … and the plain "h1, h2" list still parsed.
        assert!(!ss.matched(&info("h2", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
    }

    #[test]
    fn root_pseudo_matches_the_html_element() {
        let ss = parse(":root { color: a } :root.night { color: b } html { color: c }");
        let html = info("html", None, &[]);
        let m = ss.matched(&html, &[], &[], 0, Media::new(1000.0, false));
        // `:root` and `html` both match; `:root.night` does not (no class).
        assert_eq!(m.len(), 2);
        // `:root` has class-level specificity, `html` only type-level.
        let root_spec = m.iter().find(|(_, _, d)| d[0].1 == "a").unwrap().0;
        let tag_spec = m.iter().find(|(_, _, d)| d[0].1 == "c").unwrap().0;
        assert!(root_spec > tag_spec);
        // Not the root element → no match.
        assert!(ss.matched(&info("body", None, &[]), &[], &[], 0, Media::new(1000.0, false)).len() == 0);
        // With the class present, the qualified rule matches too.
        let night = info("html", None, &["night"]);
        assert_eq!(ss.matched(&night, &[], &[], 0, Media::new(1000.0, false)).len(), 3);
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
        let ss = collect_all(&dom, "p { color: red }", Media::new(800.0, false));
        let mut m = ss.matched(&info("p", None, &[]), &[], &[], 0, Media::new(1000.0, false));
        m.sort_by_key(|(spec, order, _)| (*spec, *order));
        assert_eq!(m.len(), 2, "both external + inline rules match");
        assert!(m[0].1 < m[1].1, "external rule has earlier document order");
    }

    #[test]
    fn collects_style_blocks_from_dom() {
        let dom = dom::parse("<html><head><style>p{color:red}</style></head>\
            <body><style>.x{color:blue}</style><p>hi</p></body></html>");
        let ss = collect(&dom, Media::new(800.0, false));
        assert!(!ss.matched(&info("p", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(!ss.matched(&info("span", None, &["x"]), &[], &[], 0, Media::new(1000.0, false)).is_empty());
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
        assert_eq!(ss.matched(&e, &[], &[], 0, Media::new(1000.0, false)).len(), 2, "media rule applies wide");
        // Narrow (500 < 768): only the base rule.
        assert_eq!(ss.matched(&e, &[], &[], 0, Media::new(500.0, false)).len(), 1, "media rule dropped narrow");
        // An un-evaluable feature never matches (rule dropped both ways).
        // `not` negates the whole query — how a mobile-first page states its
        // desktop rules. Without it a wide window renders the phone layout.
        let ssn = parse("@media not screen and (max-width: 480px) { .w { width: auto } }");
        assert!(!ssn.rules.is_empty());
        let wide = Media::new(1400.0, false);
        let phone = Media::new(400.0, false);
        let hit = |m: Media| !ssn.matched(&info("div", None, &["w"]), &[], &[], 1, m).is_empty();
        assert!(hit(wide), "1400px is NOT max-width:480 -> the negated query holds");
        assert!(!hit(phone), "400px IS max-width:480 -> the negated query does not");

        let ss2 = parse("@media (prefers-color-scheme: dark) { .col { color: blue } }");
        assert!(ss2.matched(&e, &[], &[], 0, Media::new(1000.0, false)).is_empty(), "unknown feature never applies");
    }
}
