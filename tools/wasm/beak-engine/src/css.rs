//! css.rs — author stylesheet parsing (css-syntax-3 subset) and selector matching.
//!
//! Cascade order:
//!
//! ```text
//!   inherited(parent) → UA sheet → author <style> (specificity) → inline
//! ```
//!
//! Selectors are matched right to left against an ancestor stack, with
//! `(id, class, type)` specificity. Unsupported selectors and at-rules are
//! dropped rather than mis-applied, as a browser does
//! (docs/spec/CONFORMANCE.md). External `<link>` sheets are fetched by the
//! shell and handed in as text.

use alloc::borrow::Cow;
use alloc::rc::Rc;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use crate::dom::{Dom, Element, Node};

/// The identity a selector matches against: tag + id + classes + all attributes
/// (names lowercased) for `[attr]` selectors.
#[derive(Clone)]
pub struct ElemInfo<'a> {
    /// The live element, borrowed rather than snapshotted: `:empty` and
    /// `:has()` need its children. The same matcher serves the cascade and
    /// `querySelector`.
    pub el: &'a Element,
    /// State only the runtime knows (`:checked`, `:disabled`, `:hover`), kept
    /// in one place rather than as scattered special cases.
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
        // Document state only. `:hover` has no document form; a live
        // `checked` after a click comes from the form state via `with_state`.
        // Not covered: `<fieldset disabled>` disabling its descendants.
        ElemInfo::with_state(
            el,
            ElemState {
                checked: el.checked_attr,
                disabled: el.disabled_attr,
                ..ElemState::default()
            },
        )
    }

    /// `of()` plus the pointer state: `hovered` is the ascending `seq` list of
    /// the elements the pointer is inside (`Layout::hover_at`). A binary search
    /// over a list that is empty on all but one path through the page.
    pub fn of_hovered(el: &'a Element, hovered: &[u32]) -> ElemInfo<'a> {
        ElemInfo::with_state(
            el,
            ElemState {
                checked: el.checked_attr,
                disabled: el.disabled_attr,
                hover: !hovered.is_empty() && hovered.binary_search(&el.seq).is_ok(),
                ..ElemState::default()
            },
        )
    }

    /// Free: everything derived from the element (classes, Bloom bits) is
    /// computed once by `Element::index_attrs` at parse time, because this is
    /// constructed for every element on every layout.
    pub fn with_state(el: &'a Element, state: ElemState) -> ElemInfo<'a> {
        ElemInfo { el, state }
    }

    /// `class`, split at parse time.
    pub fn classes(&self) -> &'a [String] {
        &self.el.classes
    }

    /// This element's own Bloom bits (id + classes) — see `ancestor_bloom`.
    pub fn bloom(&self) -> &'a Bloom {
        &self.el.bloom
    }

    pub fn tag(&self) -> &str {
        &self.el.tag
    }
    pub fn id(&self) -> Option<&'a str> {
        self.el.id.as_deref()
    }
    pub fn seq(&self) -> u32 {
        self.el.seq
    }
    #[cfg(test)]
    pub fn clone_for_test(&self) -> ElemInfo<'a> {
        ElemInfo::with_state(self.el, self.state)
    }

    /// `:empty`: no element children and no non-whitespace text (Selectors 4
    /// §14.3). White-space-only text does not disqualify, which the
    /// `<td></td>` / `<p>\n</p>` idioms rely on.
    pub fn is_empty_element(&self) -> bool {
        self.el.children.iter().all(|n| match n {
            Node::Text(t) => t.trim().is_empty(),
            Node::Element(_) => false,
        })
    }

    /// `:any-link` — an `<a>`/`<area>`/`<link>` that carries an `href`
    /// (Selectors 4 §8.1). A bare `<a name=…>` anchor is not a link.
    ///
    /// Read from `attrs` on demand rather than indexed onto `Element`: only a
    /// selector that asks pays for it, while anything cached on the element
    /// is paid for every element on every layout.
    fn is_link(&self) -> bool {
        matches!(self.el.tag.as_str(), "a" | "area" | "link") && self.el.attr("href").is_some()
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

/// A 256-bit Bloom filter over the id/class names on an element's ancestor
/// chain, so a descendant selector that cannot possibly match is rejected
/// without walking the chain.
///
/// Right-to-left matching is cheap for the subject (the index narrowed it)
/// and expensive above it: `Comb::Descendant` walks every ancestor before
/// giving up, and giving up is the common case.
///
/// Only ids and classes go in, never tags: a tag is barely selective, and
/// leaving it out keeps the filter sparse. Only the conjunctive parts of a
/// compound count — `:is()`/`:not()` alternatives may match without their
/// name appearing, so including them would produce false negatives.
pub type Bloom = [u64; 4];

/// One element's filter bits. Called once per element at parse time.
pub fn bloom_of(id: Option<&str>, classes: &[String]) -> Bloom {
    let mut f = [0u64; 4];
    if let Some(id) = id {
        bloom_add(&mut f, id);
    }
    for c in classes {
        bloom_add(&mut f, c);
    }
    f
}

fn bloom_add(f: &mut Bloom, s: &str) {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Two bits per name, taken from separate stretches of the hash.
    for bit in [(h >> 3) & 255, (h >> 33) & 255] {
        f[(bit >> 6) as usize] |= 1u64 << (bit & 63);
    }
}

/// Every bit the selector needs must be present in the chain's filter.
#[inline]
fn bloom_covers(need: &Bloom, have: &Bloom) -> bool {
    (need[0] & !have[0]) == 0
        && (need[1] & !have[1]) == 0
        && (need[2] & !have[2]) == 0
        && (need[3] & !have[3]) == 0
}

/// The filter for one ancestor chain (root → … → parent). Built once per
/// `matched_filtered` call and shared by every candidate.
pub fn ancestor_bloom(ancestors: &[ElemInfo]) -> Bloom {
    let mut f = [0u64; 4];
    for a in ancestors {
        f[0] |= a.el.bloom[0];
        f[1] |= a.el.bloom[1];
        f[2] |= a.el.bloom[2];
        f[3] |= a.el.bloom[3];
    }
    f
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
/// (`:*-of-type`). The of-type half needs the full sibling list, not just the
/// preceding one, which is why the matcher borrows live elements.
#[derive(Clone, Copy)]
struct SibCtx<'a> {
    idx: u32,
    count: u32,
    /// The of-type pair, counted on first ask — see `OfType`.
    of_type: &'a OfType<'a>,
    /// The subject's parent, for siblings that come after it — `:has(+ x)`,
    /// `:has(~ x)`. `None` when the caller supplied no ancestors.
    parent: Option<&'a Element>,
    /// The elements above the subject, root first, for `:dir()`.
    chain: &'a [ElemInfo<'a>],
}

/// The direction `:dir()` sees: the nearest `dir` attribute on the element or
/// an ancestor, `ltr` by default. Not implemented: `dir=auto` (treated as
/// `ltr`) and the direction of `<bdi>`/text content.
fn is_rtl(e: &ElemInfo, chain: &[ElemInfo]) -> bool {
    for el in core::iter::once(e.el).chain(chain.iter().rev().map(|a| a.el)) {
        match el.attr("dir") {
            Some(v) if v.trim().eq_ignore_ascii_case("rtl") => return true,
            Some(v) if v.trim().eq_ignore_ascii_case("ltr") => return false,
            _ => {}
        }
    }
    false
}

/// `:*-of-type`'s two counters, computed when the first selector asks and
/// remembered for the rest of the element.
///
/// They cost two tag-comparing walks over the sibling list, so they must not
/// run once per candidate selector: `:*-of-type` is rare, while the four
/// `:*-child` counters next to it are free (`prev_siblings.len()`,
/// `sib_count`). One per element, borrowed by every candidate's `SibCtx`.
struct OfType<'a> {
    tag: &'a str,
    prev: &'a [ElemInfo<'a>],
    parent: Option<&'a Element>,
    memo: core::cell::Cell<Option<(u32, u32)>>,
}

impl<'a> OfType<'a> {
    fn new(tag: &'a str, prev: &'a [ElemInfo<'a>], parent: Option<&'a Element>) -> OfType<'a> {
        OfType { tag, prev, parent, memo: core::cell::Cell::new(None) }
    }

    /// `(idx, count)`, 1-based, among siblings sharing the subject's tag.
    fn get(&self) -> (u32, u32) {
        if let Some(v) = self.memo.get() {
            return v;
        }
        let idx = self.prev.iter().filter(|p| p.tag() == self.tag).count() as u32 + 1;
        let count = self
            .parent
            .map(|p| {
                p.children
                    .iter()
                    .filter(|n| matches!(n, Node::Element(e) if e.tag == self.tag))
                    .count() as u32
            })
            // No parent on the path (the root, or a caller that did not supply
            // one): the subject is the only element of its type we can see.
            .unwrap_or(idx);
        self.memo.set(Some((idx, count)));
        (idx, count)
    }
}

impl Structural {
    fn matches(&self, ctx: Option<SibCtx>) -> bool {
        let Some(SibCtx { idx, count, of_type, .. }) = ctx else { return false }; // no sibling context → can't evaluate
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
            Structural::FirstOfType => of_type.get().0 == 1,
            Structural::LastOfType => {
                let (i, n) = of_type.get();
                i == n
            }
            Structural::OnlyOfType => of_type.get().1 == 1,
            Structural::NthOfType(a, b) => nth(a, b, of_type.get().0),
            Structural::NthLastOfType(a, b) => {
                let (i, n) = of_type.get();
                n >= i && nth(a, b, n - i + 1)
            }
        }
    }
}

/// Match a compound against an ancestor, with sibling context if it needs
/// one (`tr:nth-of-type(odd) > td`, the striped-table idiom). The sibling
/// lookup walks the grandparent's children, so it runs only when the
/// compound carries a structural pseudo-class.
fn matches_anc(comp: &Compound, ancestors: &[ElemInfo], i: usize) -> bool {
    if comp.structural.is_empty() && comp.dir.is_none() {
        return comp.matches(&ancestors[i], None);
    }
    let Some((prev, count)) = anc_siblings(ancestors, i) else {
        return false;
    };
    let parent = ancestors.get(i.wrapping_sub(1)).map(|p| p.el);
    let of_type = OfType::new(ancestors[i].tag(), &prev, parent);
    comp.matches(&ancestors[i], Some(SibCtx {
        idx: prev.len() as u32 + 1,
        count,
        of_type: &of_type,
        parent,
        chain: &ancestors[..i],
    }))
}

/// The preceding element siblings of the ancestor at `i`, and the total
/// sibling count.
fn anc_siblings<'a>(
    ancestors: &[ElemInfo<'a>],
    i: usize,
) -> Option<(alloc::vec::Vec<ElemInfo<'a>>, u32)> {
    if i == 0 {
        return None;
    }
    let el = ancestors[i].el;
    let parent = ancestors[i - 1].el;
    let kids: alloc::vec::Vec<&'a Element> = parent
        .children
        .iter()
        .filter_map(|n| match n { Node::Element(e) => Some(e), _ => None })
        .collect();
    let pos = kids.iter().position(|k| core::ptr::eq(*k, el))?;
    let prev = kids[..pos]
        .iter()
        .map(|e| ElemInfo { el: e, state: ElemState::default() })
        .collect();
    Some((prev, kids.len() as u32))
}

/// One alternative inside `:has(…)`: a combinator and the compound it applies
/// to, relative to the subject. Covers the common shapes (descendant, `>`,
/// `+`, `~` followed by one compound); anything more complex drops its
/// selector.
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

/// The elements a sheet's `:hover` rules could possibly react to — the same
/// idea as Blink's invalidation sets, in its smallest form.
///
/// It holds the names on the compound that carries the `:hover`, not on the
/// selector's subject: in `nav:hover a` the pointer has to be inside the
/// `<nav>`, and the `<a>` restyles because of it. Since an ancestor's box
/// encloses its descendant's, hit-testing only the carriers finds exactly the
/// elements whose state can change. Most elements on a page carry no hover
/// rule, so pointer movement should not walk them.
#[derive(Default)]
pub struct HoverSet {
    ids: BTreeSet<String>,
    classes: BTreeSet<String>,
    tags: BTreeSet<String>,
    /// A `:hover` compound that names nothing (`*:hover`, `[data-x]:hover`, or
    /// a `:is(…)` whose alternatives carry the names) can match anything, so
    /// no filtering is possible. Being wrong here would freeze the page under
    /// the pointer, so it degrades to "collect everything".
    any: bool,
}

impl HoverSet {
    pub fn is_empty(&self) -> bool {
        !self.any && self.ids.is_empty() && self.classes.is_empty() && self.tags.is_empty()
    }

    /// Could a `:hover` rule react to the pointer being inside this element?
    /// Called once per element box per layout, so it answers "no" as early as
    /// it can — an empty set is one bool.
    pub fn may_match(&self, el: &Element) -> bool {
        if self.is_empty() {
            return false;
        }
        if self.any || self.tags.contains(&el.tag) {
            return true;
        }
        if let Some(id) = &el.id {
            if self.ids.contains(id) {
                return true;
            }
        }
        !self.classes.is_empty() && el.classes.iter().any(|c| self.classes.contains(c))
    }

    /// Record a compound that tests `:hover`, under its most selective name.
    ///
    /// Only one name, the way the sheet's rule index picks one: every name on
    /// the compound has to match, so the narrowest is enough, and `may_match`
    /// ORs what it is given. Recording all of them would make
    /// `li.gallerybox:hover` claim every `<li>` on the page — harmless for
    /// hit-testing, but it would push every pointer move onto the slow
    /// (layout) path instead of a repaint.
    fn add(&mut self, c: &Compound) {
        if let Some(id) = &c.id {
            self.ids.insert(id.clone());
        } else if let Some(cl) = c.classes.first() {
            self.classes.insert(cl.clone());
        } else if let Some(t) = &c.tag {
            self.tags.insert(t.clone());
        } else {
            // Nothing to filter on — the safe answer is "anything".
            self.any = true;
        }
    }
}

/// Which link pseudo-class a compound asks for.
///
/// No browsing history is kept, so `:visited` matches nothing. That is
/// deliberate: a history-aware `:visited` is the classic history-leak side
/// channel, which is why browsers restrict it to a few colour properties and
/// lie to `getComputedStyle`. It makes `:link` and `:any-link` the same test.
#[derive(Clone, Copy, PartialEq, Debug)]
enum LinkSel {
    Any,
    Unvisited,
    Visited,
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
    /// compounds; the compound matches only if each group has at least one
    /// matching alternative. Contributes its most specific argument's
    /// specificity. Only compound alternatives are supported (no combinators
    /// inside).
    is_groups: Vec<Vec<Compound>>,
    /// `:where(…)` groups: match like `:is()` but contribute zero specificity.
    where_groups: Vec<Vec<Compound>>,
    structural: Vec<Structural>,
    /// `:root` — the document's root element. In an HTML document that is
    /// always `<html>`, so this is a tag test that keeps pseudo-class
    /// specificity.
    root: bool,
    /// `:empty` — no children other than white-space-only text (Selectors 4
    /// §14.3).
    empty: bool,
    /// State pseudo-classes, each `Some(want)` when the selector asks for it.
    /// They read `ElemInfo::state`.
    checked: Option<bool>,
    disabled: Option<bool>,
    /// `:hover`. Reads `ElemState::hover`, which the shell sets from the
    /// pointer position — so a rule only wins while the pointer is inside the
    /// element's box, and every element the pointer is inside is hovered, not
    /// just the innermost (`div:hover .child` is why).
    hover: Option<bool>,
    /// `:link` / `:visited` / `:any-link`, the usual way a page states its
    /// link colour.
    link: Option<LinkSel>,
    /// One entry per `:has()` on this compound; the inner list is its
    /// comma-separated alternatives, so an entry matches if any of them does
    /// and several `:has()` all have to hold.
    has: Vec<Vec<HasArg>>,
    /// `::before`/`::after` on this compound (only valid on the last compound
    /// of a selector — checked in `parse_selector`).
    pseudo: PseudoElem,
    /// `:dir(rtl)` is `Some(true)`, `:dir(ltr)` `Some(false)`.
    dir: Option<bool>,
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
        if !self.classes.iter().all(|c| e.classes().iter().any(|x| x == c)) {
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
        if self.hover.is_some_and(|w| w != e.state.hover) {
            return false;
        }
        if let Some(want) = self.dir {
            let chain = ctx.as_ref().map_or(&[][..], |c| c.chain);
            if is_rtl(e, chain) != want {
                return false;
            }
        }
        if let Some(l) = self.link {
            let ok = match l {
                // No history → nothing is visited, so `:link` degenerates to
                // `:any-link` and `:visited` never matches. See `LinkSel`.
                LinkSel::Any | LinkSel::Unvisited => e.is_link(),
                LinkSel::Visited => false,
            };
            if !ok {
                return false;
            }
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
        // :has() last: it is the only test that walks a subtree, and the cheap
        // tests above have already ruled out most elements — `.foo:has(.bar)`
        // descends only into elements that are actually `.foo`.
        self.has.iter().all(|group| group.iter().any(|a| a.matches(e, ctx)))
    }

    /// Does this compound (or anything nested in it) test `:hover`? Nested
    /// counts: `:is(a:hover, b)` and `:not(:hover)` both change what the
    /// pointer does, and missing one would leave the page frozen under it.
    fn wants_hover(&self) -> bool {
        self.hover.is_some()
            || self.not.iter().any(Compound::wants_hover)
            || self.is_groups.iter().flatten().any(Compound::wants_hover)
            || self.where_groups.iter().flatten().any(Compound::wants_hover)
            || self.has.iter().flatten().any(|h| h.compound.wants_hover())
    }

    /// Tests `:checked` — the one control state a selector can actually read.
    /// `:focus`/`:focus-within`/`:active` are in `never_matches`, so a control
    /// gaining or losing the keyboard cannot restyle anything through the
    /// cascade; only a checkbox or radio can.
    fn wants_checked(&self) -> bool {
        self.checked.is_some()
            || self.not.iter().any(Compound::wants_checked)
            || self.is_groups.iter().flatten().any(Compound::wants_checked)
            || self.where_groups.iter().flatten().any(Compound::wants_checked)
            || self.has.iter().flatten().any(|h| h.compound.wants_checked())
    }
}

/// A complex selector: compounds left→right, with the combinator that precedes
/// each compound after the first (`combs[k]` sits left of `compounds[k+1]`).
pub struct Selector {
    compounds: Vec<Compound>,
    combs: Vec<Comb>,
    spec: u32,
    /// Names that must appear somewhere on the subject's ancestor chain —
    /// see `Bloom`. Empty for a selector with no ancestor part, which then
    /// always passes the pre-test.
    anc_bloom: Bloom,
    /// `::before`/`::after` this selector targets (`None` = a normal
    /// selector, matching the real element).
    pseudo: PseudoElem,
}

impl Selector {
    /// Record every compound of this selector that tests `:hover`. An ancestor
    /// compound counts as much as the subject's — `nav:hover a` restyles the
    /// link, and the pointer is inside the `<nav>`.
    fn collect_hover(&self, out: &mut HoverSet) {
        for c in self.compounds.iter().filter(|c| c.wants_hover()) {
            out.add(c);
        }
    }

    /// Record every compound that tests `:checked`, wherever it sits in the
    /// selector — an ancestor carrier (`:checked ~ .menu`) restyles something
    /// the control does not contain, so the one set has to cover both.
    fn collect_checked(&self, out: &mut HoverSet) {
        for c in self.compounds.iter().filter(|c| c.wants_checked()) {
            out.add(c);
        }
    }

    /// Record every `:hover` carrier whose rule restyles a sibling — anything
    /// with `+` or `~` to the right of the carrier (`li:hover + li`). A
    /// repaint that walks the carrier's subtree cannot see that sibling, so
    /// those carriers take the slow path.
    fn collect_hover_sideways(&self, out: &mut HoverSet) {
        for (i, c) in self.compounds.iter().enumerate() {
            if c.wants_hover()
                && self.combs[i.min(self.combs.len())..]
                    .iter()
                    .any(|k| matches!(k, Comb::Adjacent | Comb::General))
            {
                out.add(c);
            }
        }
    }

    /// Right-to-left match: the last compound must match `subject`, then earlier
    /// compounds must match ancestors per their combinators. `ancestors` is
    /// root→…→parent order. Descendant matching is nearest-first, without
    /// backtracking.
    fn matches(&self, subject: &ElemInfo, ancestors: &[ElemInfo], prev_siblings: &[ElemInfo], sib_count: u32, anc_bloom: &Bloom, of_type: &OfType) -> bool {
        // O(1) rejection before the ancestor walk: if a name this selector
        // requires above the subject is absent from the whole chain, no amount
        // of walking will find it. False positives are fine (the real match
        // runs anyway); false negatives are not, which is why only conjunctive
        // ids/classes went into the filter.
        if !bloom_covers(&self.anc_bloom, anc_bloom) {
            return false;
        }
        // The subject's structural pseudo-classes evaluate against its 1-based
        // sibling index (preceding count + 1) and the total sibling count —
        // and, for `:*-of-type`, against the same pair restricted to its tag.
        // The of-type pair is shared across every candidate selector of this
        // element and counted only if one asks (see `OfType`).
        let subj_ctx = Some(SibCtx {
            idx: prev_siblings.len() as u32 + 1,
            count: sib_count,
            of_type,
            parent: ancestors.last().map(|p| p.el),
            chain: ancestors,
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
                    if anc < 0 || !matches_anc(comp, ancestors, anc as usize) {
                        return false;
                    }
                    anc -= 1;
                    at_subject = false;
                }
                Comb::Descendant => {
                    let mut a = anc;
                    let mut found = false;
                    while a >= 0 {
                        if matches_anc(comp, ancestors, a as usize) {
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

/// What the page is being rendered into — everything `@media` can ask about.
/// `Copy`, so it threads through the cascade cheaply.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Media {
    pub width: f32,
    /// The viewport height; `None` when the caller does not know it, which
    /// makes every height feature unknown (and so not matching).
    pub height: Option<f32>,
    /// The user's colour-scheme preference; the shell resolves it from the
    /// compositor palette, so it is the system theme.
    pub dark: bool,
}

impl Media {
    pub fn new(width: f32, dark: bool) -> Media {
        Media { width, height: None, dark }
    }

    pub fn with_height(self, h: f32) -> Media {
        Media { height: Some(h), ..self }
    }
}

/// Everything selector matching evaluates conditional rules against: the
/// media state and the query containers around the element, nearest last.
#[derive(Clone, Copy, Debug)]
pub struct MatchEnv<'a> {
    pub media: Media,
    pub containers: &'a [crate::media::CqBox],
}

/// The viewport a cascade runs in: a bare width (no height, no containers)
/// or a full [`MatchEnv`]. The colour scheme always comes from the theme.
pub trait Viewport {
    fn env(&self, dark: bool) -> MatchEnv<'_>;
}

impl Viewport for f32 {
    fn env(&self, dark: bool) -> MatchEnv<'_> {
        MatchEnv { media: Media::new(*self, dark), containers: &[] }
    }
}

impl Viewport for MatchEnv<'_> {
    fn env(&self, dark: bool) -> MatchEnv<'_> {
        MatchEnv { media: Media { dark, ..self.media }, containers: self.containers }
    }
}


/// What a declaration of this property can move.
///
/// A `:hover` rule that only recolours something cannot move a box, and
/// answering it must not cost a layout. Blink carries the same idea as an
/// invalidation class per property.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// Can change geometry — a layout has to run.
    Layout,
    /// Can only change what a box looks like, never where anything sits.
    Paint,
    /// Not implemented, so applying it changes nothing: `apply_one` has no arm
    /// for it, and custom properties have already been substituted by
    /// `vars.rs`. A rule made only of these is free to gain or lose.
    Nothing,
}

// Every property name the cascade can resolve, turned into a number once,
// when the stylesheet is parsed, so `apply_one` dispatches through a jump
// table instead of a chain of string comparisons.
//
// The macro is the single source of truth: the enum and `prop_key` come from
// one list, so a name can never point at a variant that does not exist. The
// match in `apply_one` is exhaustive, so a new variant fails to compile until
// it is handled.
macro_rules! css_props {
    ($($var:ident = $name:literal @ $class:ident),* $(,)?) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        #[repr(u16)]
        pub enum Prop {
            /// Anything we do not implement. Custom properties land here too —
            /// `vars.rs` substitutes `var()` textually before this point, so a
            /// surviving `--name` declaration has nothing left to say.
            Unknown,
            $($var),*
        }

        /// Resolve a declaration's property name. An exact string match, so an
        /// unknown name can never be mistaken for a supported one.
        pub fn prop_key(name: &str) -> Prop {
            match name {
                $($name => Prop::$var,)*
                _ => Prop::Unknown,
            }
        }

        /// Number of variants incl. `Unknown` — the width of any per-property
        /// table.
        pub const PROP_N: usize = 1 + [$($name),*].len();

        /// The canonical name, for diagnostics.
        pub fn prop_name(p: Prop) -> &'static str {
            match p {
                Prop::Unknown => "(unknown)",
                $(Prop::$var => $name,)*
            }
        }

        /// What a declaration of this property can move. The class is written
        /// next to the name in the one list, so a new property cannot be added
        /// without stating it.
        pub fn prop_class(p: Prop) -> Class {
            match p {
                Prop::Unknown => Class::Nothing,
                $(Prop::$var => Class::$class,)*
            }
        }
    };
}

css_props! {
    All = "all" @ Layout,
    Display = "display" @ Layout,
    TableLayout = "table-layout" @ Layout,
    BorderCollapse = "border-collapse" @ Layout,
    EmptyCells = "empty-cells" @ Layout,
    VerticalAlign = "vertical-align" @ Layout,
    Overflow = "overflow" @ Layout,
    OverflowX = "overflow-x" @ Layout,
    OverflowY = "overflow-y" @ Layout,
    TextOverflow = "text-overflow" @ Paint,
    Filter = "filter" @ Paint,
    WebkitFilter = "-webkit-filter" @ Paint,
    // Opera's prefix, and the only one Bootstrap still ships for `object-fit`.
    ObjectFit = "object-fit" @ Paint,
    OObjectFit = "-o-object-fit" @ Paint,
    OverflowWrap = "overflow-wrap" @ Layout,
    WordWrap = "word-wrap" @ Layout,
    WordBreak = "word-break" @ Layout,
    Outline = "outline" @ Paint,
    OutlineWidth = "outline-width" @ Paint,
    OutlineStyle = "outline-style" @ Paint,
    OutlineColor = "outline-color" @ Paint,
    OutlineOffset = "outline-offset" @ Paint,
    AccentColor = "accent-color" @ Paint,
    BorderRadius = "border-radius" @ Paint,
    Transform = "transform" @ Layout,
    BoxShadow = "box-shadow" @ Paint,
    BorderTopLeftRadius = "border-top-left-radius" @ Paint,
    BorderTopRightRadius = "border-top-right-radius" @ Paint,
    BorderBottomRightRadius = "border-bottom-right-radius" @ Paint,
    BorderBottomLeftRadius = "border-bottom-left-radius" @ Paint,
    CaptionSide = "caption-side" @ Layout,
    BorderSpacing = "border-spacing" @ Layout,
    Color = "color" @ Paint,
    FontWeight = "font-weight" @ Layout,
    FontStyle = "font-style" @ Layout,
    FontSize = "font-size" @ Layout,
    LineHeight = "line-height" @ Layout,
    Font = "font" @ Layout,
    TextDecoration = "text-decoration" @ Paint,
    TextDecorationLine = "text-decoration-line" @ Paint,
    TextTransform = "text-transform" @ Layout,
    TextAlignLast = "text-align-last" @ Layout,
    TextIndent = "text-indent" @ Layout,
    LetterSpacing = "letter-spacing" @ Layout,
    WordSpacing = "word-spacing" @ Layout,
    Direction = "direction" @ Layout,
    TextAlign = "text-align" @ Layout,
    ListStyleType = "list-style-type" @ Layout,
    ListStyle = "list-style" @ Layout,
    CounterReset = "counter-reset" @ Layout,
    CounterIncrement = "counter-increment" @ Layout,
    WhiteSpace = "white-space" @ Layout,
    Opacity = "opacity" @ Paint,
    Visibility = "visibility" @ Layout,
    FontFamily = "font-family" @ Layout,
    Width = "width" @ Layout,
    MinWidth = "min-width" @ Layout,
    InlineSize = "inline-size" @ Layout,
    BlockSize = "block-size" @ Layout,
    MinInlineSize = "min-inline-size" @ Layout,
    MinBlockSize = "min-block-size" @ Layout,
    MaxInlineSize = "max-inline-size" @ Layout,
    MaxBlockSize = "max-block-size" @ Layout,
    Inset = "inset" @ Layout,
    InsetInline = "inset-inline" @ Layout,
    InsetBlock = "inset-block" @ Layout,
    InsetInlineStart = "inset-inline-start" @ Layout,
    InsetInlineEnd = "inset-inline-end" @ Layout,
    InsetBlockStart = "inset-block-start" @ Layout,
    InsetBlockEnd = "inset-block-end" @ Layout,
    TextDecorationColor = "text-decoration-color" @ Paint,
    MaxWidth = "max-width" @ Layout,
    Height = "height" @ Layout,
    MinHeight = "min-height" @ Layout,
    MaxHeight = "max-height" @ Layout,
    BoxSizing = "box-sizing" @ Layout,
    AspectRatio = "aspect-ratio" @ Layout,
    Appearance = "appearance" @ Layout,
    WebkitAppearance = "-webkit-appearance" @ Layout,
    MozAppearance = "-moz-appearance" @ Layout,
    Contain = "contain" @ Layout,
    ContainIntrinsicSize = "contain-intrinsic-size" @ Layout,
    Margin = "margin" @ Layout,
    MarginTop = "margin-top" @ Layout,
    MarginBlockStart = "margin-block-start" @ Layout,
    MarginBottom = "margin-bottom" @ Layout,
    MarginBlockEnd = "margin-block-end" @ Layout,
    MarginLeft = "margin-left" @ Layout,
    MarginInlineStart = "margin-inline-start" @ Layout,
    MarginRight = "margin-right" @ Layout,
    MarginInlineEnd = "margin-inline-end" @ Layout,
    MarginInline = "margin-inline" @ Layout,
    MarginBlock = "margin-block" @ Layout,
    Padding = "padding" @ Layout,
    PaddingTop = "padding-top" @ Layout,
    PaddingBlockStart = "padding-block-start" @ Layout,
    PaddingRight = "padding-right" @ Layout,
    PaddingInlineEnd = "padding-inline-end" @ Layout,
    PaddingBottom = "padding-bottom" @ Layout,
    PaddingBlockEnd = "padding-block-end" @ Layout,
    PaddingLeft = "padding-left" @ Layout,
    PaddingInlineStart = "padding-inline-start" @ Layout,
    PaddingInline = "padding-inline" @ Layout,
    PaddingBlock = "padding-block" @ Layout,
    BackgroundColor = "background-color" @ Paint,
    Background = "background" @ Paint,
    BackgroundImage = "background-image" @ Paint,
    BackgroundRepeat = "background-repeat" @ Paint,
    BackgroundPosition = "background-position" @ Paint,
    BackgroundSize = "background-size" @ Paint,
    BackgroundClip = "background-clip" @ Paint,
    BackgroundOrigin = "background-origin" @ Paint,
    WebkitBackgroundClip = "-webkit-background-clip" @ Paint,
    Mask = "mask" @ Paint,
    WebkitMask = "-webkit-mask" @ Paint,
    MaskImage = "mask-image" @ Paint,
    WebkitMaskImage = "-webkit-mask-image" @ Paint,
    MaskRepeat = "mask-repeat" @ Paint,
    WebkitMaskRepeat = "-webkit-mask-repeat" @ Paint,
    MaskPosition = "mask-position" @ Paint,
    WebkitMaskPosition = "-webkit-mask-position" @ Paint,
    MaskSize = "mask-size" @ Paint,
    WebkitMaskSize = "-webkit-mask-size" @ Paint,
    Border = "border" @ Layout,
    BorderTop = "border-top" @ Layout,
    BorderRight = "border-right" @ Layout,
    BorderBottom = "border-bottom" @ Layout,
    BorderLeft = "border-left" @ Layout,
    BorderWidth = "border-width" @ Layout,
    BorderColor = "border-color" @ Paint,
    BorderStyle = "border-style" @ Layout,
    BorderTopWidth = "border-top-width" @ Layout,
    BorderRightWidth = "border-right-width" @ Layout,
    BorderBottomWidth = "border-bottom-width" @ Layout,
    BorderLeftWidth = "border-left-width" @ Layout,
    BorderTopColor = "border-top-color" @ Paint,
    BorderRightColor = "border-right-color" @ Paint,
    BorderBottomColor = "border-bottom-color" @ Paint,
    BorderLeftColor = "border-left-color" @ Paint,
    BorderTopStyle = "border-top-style" @ Layout,
    BorderRightStyle = "border-right-style" @ Layout,
    BorderBottomStyle = "border-bottom-style" @ Layout,
    BorderLeftStyle = "border-left-style" @ Layout,
    Position = "position" @ Layout,
    Float = "float" @ Layout,
    Clear = "clear" @ Layout,
    Clip = "clip" @ Layout,
    Top = "top" @ Layout,
    Right = "right" @ Layout,
    Bottom = "bottom" @ Layout,
    Left = "left" @ Layout,
    ZIndex = "z-index" @ Layout,
    FlexDirection = "flex-direction" @ Layout,
    FlexWrap = "flex-wrap" @ Layout,
    FlexFlow = "flex-flow" @ Layout,
    JustifyContent = "justify-content" @ Layout,
    AlignItems = "align-items" @ Layout,
    AlignContent = "align-content" @ Layout,
    AlignSelf = "align-self" @ Layout,
    Gap = "gap" @ Layout,
    GridGap = "grid-gap" @ Layout,
    ColumnGap = "column-gap" @ Layout,
    RowGap = "row-gap" @ Layout,
    GridColumnGap = "grid-column-gap" @ Layout,
    GridRowGap = "grid-row-gap" @ Layout,
    GridAutoColumns = "grid-auto-columns" @ Layout,
    FlexGrow = "flex-grow" @ Layout,
    FlexShrink = "flex-shrink" @ Layout,
    FlexBasis = "flex-basis" @ Layout,
    Order = "order" @ Layout,
    Flex = "flex" @ Layout,
    GridTemplateColumns = "grid-template-columns" @ Layout,
    GridTemplateRows = "grid-template-rows" @ Layout,
    GridAutoRows = "grid-auto-rows" @ Layout,
    GridTemplateAreas = "grid-template-areas" @ Layout,
    Grid = "grid" @ Layout,
    GridTemplate = "grid-template" @ Layout,
    GridColumn = "grid-column" @ Layout,
    GridRow = "grid-row" @ Layout,
    GridColumnStart = "grid-column-start" @ Layout,
    GridRowStart = "grid-row-start" @ Layout,
    GridArea = "grid-area" @ Layout,
    JustifyItems = "justify-items" @ Layout,
    JustifySelf = "justify-self" @ Layout,
    PlaceItems = "place-items" @ Layout,
    PlaceSelf = "place-self" @ Layout,
    PlaceContent = "place-content" @ Layout,
    ContainerType = "container-type" @ Layout,
    ContainerName = "container-name" @ Layout,
    Container = "container" @ Layout,
    Content = "content" @ Layout,
}

/// One `selectors { declarations }` rule; `order` is document position (for
/// same-specificity tie-breaking, last wins). `media` is the `@media`
/// condition list it sits inside (comma = OR), or `None` when unconditional.
pub struct Rule {
    selectors: Vec<Selector>,
    /// Normal declarations, `!important` already stripped at parse time so the
    /// two cascade passes do not re-scan every value's tail.
    decls: Vec<(Prop, String)>,
    /// Custom properties (`--name: value`) with their name. They cannot live
    /// in `decls`, where `--anything` becomes `Prop::Unknown` and loses its
    /// name; they are cascaded and inherited per element like any inherited
    /// property.
    customs: Vec<(String, String)>,
    customs_imp: Vec<(String, String)>,
    /// The `!important` ones, same shape. Usually empty, which is the point:
    /// pass 2 then has nothing to walk.
    decls_imp: Vec<(Prop, String)>,
    order: u32,
    media: Option<Rc<crate::media::MediaChain>>,
    container: Option<Rc<crate::media::ContainerChain>>,
    /// Cascade-layer rank (css-cascade-5 §6.4.4). `UNLAYERED` for a rule that
    /// sits outside every `@layer`; otherwise the layer's position in
    /// declaration order, so a later layer wins a normal declaration.
    layer: u16,
}

/// Rank of a rule that is in no layer. Normal declarations from unlayered
/// rules beat every layered one, so it sorts highest; the `!important` pass
/// reverses the axis (`imp_rank`), which drops it to the bottom there.
pub const UNLAYERED: u16 = u16::MAX;

/// Layer priority for the `!important` pass, where the whole layer axis
/// reverses: the first-declared layer wins, and unlayered loses to all.
#[inline]
pub fn imp_rank(layer: u16) -> u16 {
    UNLAYERED - layer
}

/// The layers a stylesheet declared — css-cascade-5 §6.4.
///
/// A layer's position is fixed by its first declaration, whether that came
/// from `@layer a, b;` (order only) or `@layer a { … }` (order + rules), and
/// nesting makes a dotted path. Ranks cannot be handed out while parsing:
/// `@layer a; @layer b; @layer a.c;` has to sort `a.c` inside `a`, which
/// moves `b` after every rule tagged `b` already exists. So this is a
/// push-only arena of ids, and `ranks()` turns ids into positions once the
/// sheet is fully read.
#[derive(Default)]
struct Layers {
    names: Vec<String>,
}

impl Layers {
    /// Stable id for `name`, registering it — and any missing ancestor, since
    /// `@layer a.b` alone still creates `a` — on first sight.
    fn id(&mut self, name: &str) -> u16 {
        if let Some(cut) = name.rfind('.') {
            self.id(&name[..cut]);
        }
        if let Some(i) = self.names.iter().position(|n| n == name) {
            return i as u16;
        }
        self.names.push(String::from(name));
        (self.names.len() - 1) as u16
    }

    /// A fresh anonymous layer (`@layer { … }`), which no later rule can name.
    /// The NUL keeps the generated name out of the author's namespace.
    fn anon(&mut self, parent: &str) -> (u16, String) {
        let n = self.names.len();
        let name = if parent.is_empty() {
            format!("\u{0}{n}")
        } else {
            format!("{parent}.\u{0}{n}")
        };
        (self.id(&name), name)
    }

    /// `id -> rank`, in tree order. A layer's sort key is the chain of ids of
    /// its own prefixes: `a.c` keys as `[id(a), id(a.c)]`, which puts it after
    /// `a` and before any later sibling of `a`, at any nesting depth.
    fn ranks(&self) -> Vec<u16> {
        let key = |name: &str| -> Vec<u16> {
            let mut k = Vec::new();
            let mut at = 0;
            loop {
                let cut = name[at..].find('.').map(|c| at + c);
                let prefix = &name[..cut.unwrap_or(name.len())];
                k.push(self.names.iter().position(|n| n == prefix).unwrap_or(0) as u16);
                match cut {
                    Some(c) => at = c + 1,
                    None => break,
                }
            }
            k
        };
        let mut ord: Vec<u16> = (0..self.names.len() as u16).collect();
        ord.sort_by_cached_key(|&i| key(&self.names[i as usize]));
        let mut rank = vec![0u16; self.names.len()];
        for (pos, &id) in ord.iter().enumerate() {
            rank[id as usize] = pos as u16;
        }
        rank
    }
}

/// One rule that matched: `(layer rank, specificity, document order, normal
/// declarations, `!important` declarations, custom properties, `!important`
/// custom properties)`. The caller sorts ascending and applies the normal
/// pass first, then the important one — and because the layer axis reverses
/// between the two passes, the important pass re-sorts on `imp_rank` rather
/// than reusing pass 1's order. Custom properties ride along so matching,
/// the expensive part of the cascade, runs only once.
pub type Matched<'a> = (u16, u32, u32, &'a [(Prop, String)], &'a [(Prop, String)],
                        &'a [(String, String)], &'a [(String, String)]);

/// Initial values of custom properties registered with `@property`.
///
/// Frameworks such as Tailwind v4 compose values from placeholders
/// (`box-shadow: var(--tw-inset-shadow), …, var(--tw-shadow)`) and give each
/// placeholder its value only through `@property --x { initial-value: … }`.
/// Without it the `var()` stays unresolved and the declaration is invalid.
///
/// Simplification: `inherits: false` is ignored. Initial values are set on
/// the root and inherit downwards, which is too generous for a property an
/// ancestor sets and a descendant should not inherit.
pub type Registered = alloc::vec::Vec<(String, String)>;

/// A font the page brings (`@font-face`).
#[derive(Clone, Debug)]
pub struct FontFace {
    /// Hash of the lowercased family name (`style::hash_name`).
    pub family: u32,
    /// Sources in page order. The first one beak can read wins, as in a
    /// browser.
    pub src: Vec<String>,
    /// 100..900. A range (`400 700`) is reduced to its start.
    pub weight: u16,
    pub italic: bool,
}

/// A parsed author stylesheet.
///
/// Rules are also indexed by the most selective simple selector in each
/// selector's rightmost compound, because that one must match the subject for
/// the whole selector to have a chance. Without the index every element is
/// tested against every rule, which dominates layout on a real page.
pub struct Stylesheet {
    rules: Vec<Rule>,
    /// Some `@media` rule tests a height-dependent feature, so a height-only
    /// resize can change the cascade.
    pub media_reads_height: bool,
    /// Some rule sits in an `@container` block.
    pub has_container_rules: bool,
    /// The page's fonts. Descriptors only — the host fetches the bytes (see
    /// `Engine::take_pending_fonts`).
    pub faces: Vec<FontFace>,
    /// Initial values from `@property` — see [`Registered`].
    pub registered: Registered,
    /// Selectors targeting real elements.
    normal: Index,
    /// Selectors ending in `::before`/`::after`, kept apart because the
    /// cascade runs three times per element (the element, then each
    /// generated box), and a shared index would make each pass walk the
    /// others' candidates.
    pseudo: Index,
    /// Which elements a `:hover` rule here could possibly react to. On a page
    /// without hover rules it is empty and pointer movement costs exactly
    /// nothing; on a page with them it is a small fraction of the document.
    pub hover_set: HoverSet,
    /// The subset of `hover_set` whose rules declare at least one property that
    /// can move something. A pointer entering an element outside this set can
    /// only change how that element looks, so a repaint suffices.
    ///
    /// Split per rule, not per page: one `:hover{display:none}` somewhere must
    /// not make every recolouring hover on the page expensive.
    pub hover_layout_set: HoverSet,
    /// Carriers whose rules restyle a sibling (`li:hover + li`). Not about
    /// geometry but about what a repaint can find: it walks the carrier's own
    /// subtree, and a sibling is not in it.
    pub hover_sideways_set: HoverSet,
    /// Every `:checked` carrier, by name. A control whose checked state changes
    /// and that may match one of these has to be laid out: `:checked` can move
    /// boxes (the checkbox hack is `input:checked ~ .menu{display:block}`), and
    /// repainting the control alone would leave that menu shut.
    pub checked_set: HoverSet,
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
/// One scanner for both callers. A quoted url may legally contain `)`, a url
/// is rarely the whole value (`background: red url(x) no-repeat`), and a
/// quoted one may carry backslash-escaped quotes, as inline SVG `data:` URIs
/// do (`url("data:image/svg+xml,<svg xmlns=\"…\">")`). Stopping at the first
/// inner quote would truncate the payload.
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

/// Undo CSS string escaping in a URL, with the same decoder the declaration
/// parser uses — otherwise the `url()` table and the declaration naming it
/// disagree about the key and the image never resolves.
fn unescape(s: &str) -> Cow<'_, str> {
    css_unescape(s)
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
/// the table must be complete before the cascade picks a winner.
fn collect_urls(css: &str, out: &mut BTreeMap<u64, String>) {
    let mut i = 0usize;
    while let Some((u, next)) = url_at(css, i) {
        if !u.is_empty() {
            out.entry(url_key(&u)).or_insert_with(|| u.into_owned());
        }
        i = next;
    }
}

/// A bucket entry: rule index, selector index, and what that selector needs
/// to find on the ancestor chain. Carrying the requirement here rather than
/// looking it up through `rules[ri].selectors[si]` lets `candidates` reject
/// before collecting; most tag-bucket candidates cannot match.
type Cand = (u32, u32, Bloom);

/// Candidate `(rule, selector)` pairs bucketed by the most selective simple
/// selector of a selector's rightmost compound. A selector lives in exactly
/// one bucket, so collecting several buckets cannot produce duplicates.
#[derive(Default)]
struct Index {
    by_id: BTreeMap<String, Vec<Cand>>,
    by_class: BTreeMap<String, Vec<Cand>>,
    by_tag: BTreeMap<String, Vec<Cand>>,
    /// Rightmost compound names no tag, id or class (`*`, `[attr]`, a bare
    /// `:not(...)`) — must be tried for every element.
    universal: Vec<Cand>,
}

impl Index {
    fn insert(&mut self, key: (u32, u32), last: &Compound, need: Bloom) {
        let key = (key.0, key.1, need);
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

    fn candidates(&self, subject: &ElemInfo, chain: &Bloom, out: &mut Vec<(u32, u32)>) {
        let mut take = |v: &Vec<Cand>| {
            for &(ri, si, need) in v {
                if bloom_covers(&need, chain) {
                    out.push((ri, si));
                }
            }
        };
        if let Some(id) = subject.id() {
            if let Some(v) = self.by_id.get(id) {
                take(v);
            }
        }
        for c in subject.classes() {
            if let Some(v) = self.by_class.get(c.as_str()) {
                take(v);
            }
        }
        if let Some(v) = self.by_tag.get(subject.tag()) {
            take(v);
        }
        take(&self.universal);
    }
}

impl Stylesheet {
    pub fn empty() -> Stylesheet {
        Stylesheet {
            faces: Vec::new(),
            rules: Vec::new(),
            media_reads_height: false,
            has_container_rules: false,
            registered: Registered::new(),
            normal: Index::default(),
            pseudo: Index::default(),
            hover_set: HoverSet::default(),
            hover_layout_set: HoverSet::default(),
            hover_sideways_set: HoverSet::default(),
            checked_set: HoverSet::default(),
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
                    self.normal.insert(key, last, sel.anc_bloom);
                } else {
                    self.pseudo.insert(key, last, sel.anc_bloom);
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
    ) -> Vec<Matched<'a>> {
        let env = MatchEnv { media, containers: &[] };
        self.matched_filtered(subject, ancestors, prev_siblings, sib_count, env, PseudoElem::None)
    }

    /// Like [`Self::matched`], with query containers for `@container` rules.
    pub fn matched_env<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        env: MatchEnv,
    ) -> Vec<Matched<'a>> {
        self.matched_filtered(subject, ancestors, prev_siblings, sib_count, env, PseudoElem::None)
    }

    /// Same as `matched`, but for `subject`'s `::before`/`::after` generated
    /// box — only selectors ending in that pseudo-element are considered.
    pub fn matched_pseudo<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        env: MatchEnv,
        pseudo: PseudoElem,
    ) -> Vec<Matched<'a>> {
        self.matched_filtered(subject, ancestors, prev_siblings, sib_count, env, pseudo)
    }

    fn matched_filtered<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
        prev_siblings: &[ElemInfo],
        sib_count: u32,
        env: MatchEnv,
        want: PseudoElem,
    ) -> Vec<Matched<'a>> {
        // Only selectors whose rightmost compound could match this element are
        // worth testing — that is what the index buys. Pre-sized because an
        // element typically has dozens of candidates.
        let chain = ancestor_bloom(ancestors);
        // One per element, shared by every candidate below.
        let of_type = OfType::new(subject.tag(), prev_siblings, ancestors.last().map(|p| p.el));
        let mut cands: Vec<(u32, u32)> = Vec::with_capacity(32);
        let index = if want == PseudoElem::None { &self.normal } else { &self.pseudo };
        index.candidates(subject, &chain, &mut cands);

        // One rule contributes one entry, at the highest specificity among its
        // matching selectors. Deduplicating the hits is cheaper than sorting
        // every candidate, because few candidates match. `rule.order` is
        // unique per rule and identifies it. Output order is not a contract —
        // every caller sorts by (layer, spec, order) before reading.
        let mut out: Vec<Matched<'a>> = Vec::with_capacity(16);
        for &(ri, si) in &cands {
            let rule = &self.rules[ri as usize];
            let sel = &rule.selectors[si as usize];
            if sel.pseudo != want {
                continue;
            }
            // Skip rules inside an `@media` block whose condition doesn't hold.
            if let Some(chain) = &rule.media {
                if !chain.matches(&env.media) {
                    continue;
                }
            }
            if let Some(chain) = &rule.container {
                if !chain.matches(env.containers) {
                    continue;
                }
            }
            if !sel.matches(subject, ancestors, prev_siblings, sib_count, &chain, &of_type) {
                continue;
            }
            match out.iter_mut().find(|e| e.2 == rule.order) {
                Some(e) => e.1 = e.1.max(sel.spec),
                None => out.push((
                    rule.layer,
                    sel.spec,
                    rule.order,
                    rule.decls.as_slice(),
                    rule.decls_imp.as_slice(),
                    rule.customs.as_slice(),
                    rule.customs_imp.as_slice(),
                )),
            }
        }
        out
    }
}

/// Gather + parse every `<style>` block in the document into one stylesheet.
pub fn collect(dom: &Dom, media: Media) -> Stylesheet {
    collect_all(dom, "", media)
}

/// The text of every `<style>` block in the document, in tree order.
///
/// Together with the `url()`s in `style` attributes (see `add_inline_urls`),
/// this is all the cascade depends on in the tree. A cache of the collected
/// sheet must key on this text, not on "something in the tree changed": a
/// `classList.toggle` changes no stylesheet.
pub fn style_text(dom: &Dom) -> String {
    let mut out = String::new();
    gather_style_text(&dom.root, &mut out);
    out
}

/// Add the `url()`s of the tree's `style` attributes to a sheet's table.
/// Idempotent (`BTreeMap::or_insert_with`), so a cached sheet picks up the
/// URLs of the current tree without being re-parsed.
pub fn add_inline_urls(dom: &Dom, sheet: &mut Stylesheet) {
    gather_inline_urls(&dom.root, sheet);
}

/// Author stylesheet = already-fetched external `<link>` CSS (document order:
/// `<head>` first) followed by inline `<style>` blocks. The shell fetches the
/// linked files (the engine is host-free) and hands their bytes in as `external`.
pub fn collect_all(dom: &Dom, external: &str, _media: Media) -> Stylesheet {
    let mut css = String::from(external);
    css.push('\n');
    gather_style_text(&dom.root, &mut css);
    if css.trim().is_empty() {
        let mut sheet = Stylesheet::empty();
        gather_inline_urls(&dom.root, &mut sheet);
        return sheet;
    }
    // Custom properties are not substituted here: they are inherited
    // properties and cascade per element (`Rule::customs`, `Matched`,
    // `style::resolve_in`), with `vars::expand` substituting on apply.
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

/// The targets of a stylesheet's `@import` rules, in source order — the shell's
/// fetch list for the round after the linked sheets.
///
/// `parse` skips `@import` (it cannot fetch), so without this a sheet made of
/// nothing but imports would style nothing.
///
/// Only at the top level: an `@import` inside a block is invalid, and stepping
/// over blocks also keeps a `content: "@import x"` string out of the list.
/// The prelude after the URL (`layer()`, `supports()`, a media query) is
/// stepped over rather than honoured. Not implemented: an import's media
/// query; the import applies unconditionally.
pub fn import_urls(css: &str) -> Vec<String> {
    let css = strip_comments(css);
    let b = css.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut depth) = (0usize, 0i32);
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            // A string can hold braces and semicolons; skip it whole.
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'@' if depth == 0 && css[i..].len() >= 7 && css[i..i + 7].eq_ignore_ascii_case("@import") => {
                let end = css[i..].find(';').map_or(css.len(), |e| i + e);
                if let Some(u) = import_target(&css[i + 7..end]) {
                    out.push(u);
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The URL out of one `@import` prelude: `"x"`, `'x'`, `url(x)`, `url("x")`.
fn import_target(prelude: &str) -> Option<String> {
    let t = prelude.trim_start();
    let rest = if t.len() >= 4 && t[..4].eq_ignore_ascii_case("url(") { &t[4..] } else { t };
    let rest = rest.trim_start();
    let mut c = rest.chars();
    let url = match c.next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next()?,
        // Unquoted `url(x)` — ends at the closing paren.
        _ => rest.split(')').next()?.trim(),
    };
    let url = url.trim();
    (!url.is_empty()).then(|| url.to_string())
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

/// Parse a stylesheet body into rules (css-syntax-3 subset). Descends into
/// `@media`, `@supports` and `@layer` blocks; collects `@font-face` and
/// `@property`; other at-rules (`@keyframes`, `@import`, …) are skipped.
pub fn parse(css: &str) -> Stylesheet {
    // XHTML `<style>` bodies wrap the CSS in a `<![CDATA[ … ]]>` marker (the
    // CSS2.1 reftest suite does this). It is raw text to us, so strip the
    // markers before parsing — real CSS never contains them.
    let css = if css.contains("<![CDATA[") {
        css.replace("<![CDATA[", " ").replace("]]>", " ")
    } else {
        String::from(css)
    };
    let css = strip_comments(&css);
    let mut rules = Vec::new();
    let mut order = 0u32;
    let mut layers = Layers::default();
    let mut registered: Registered = Registered::new();
    let mut faces: Vec<FontFace> = Vec::new();
    parse_into(&css, 0, css.len(), Conds::default(), &mut rules, &mut order, "", &mut layers, &mut registered, &mut faces);
    // Ids became positions only now that every layer is known.
    if !layers.names.is_empty() {
        let rank = layers.ranks();
        for r in &mut rules {
            if r.layer != UNLAYERED {
                r.layer = rank[r.layer as usize];
            }
        }
    }
    let mut urls = BTreeMap::new();
    collect_urls(&css, &mut urls);
    let mut hover_set = HoverSet::default();
    let mut hover_layout_set = HoverSet::default();
    let mut hover_sideways_set = HoverSet::default();
    let mut checked_set = HoverSet::default();
    for r in &rules {
        // A rule made only of properties we do not implement declares nothing
        // at all — `apply_one` has no arm for them — so it can gain or lose
        // without moving a pixel (e.g. `:hover{cursor:pointer}`), and must not
        // force a layout.
        let moves = r
            .decls
            .iter()
            .chain(r.decls_imp.iter())
            .any(|(p, _)| prop_class(*p) == Class::Layout);
        for sel in &r.selectors {
            sel.collect_hover(&mut hover_set);
            if moves {
                sel.collect_hover(&mut hover_layout_set);
            }
            sel.collect_hover_sideways(&mut hover_sideways_set);
            sel.collect_checked(&mut checked_set);
        }
    }
    let media_reads_height = rules.iter().any(|r| {
        r.media.as_ref().is_some_and(|m| m.any_feature(&crate::media::reads_height))
    });
    let has_container_rules = rules.iter().any(|r| r.container.is_some());
    let mut sheet = Stylesheet {
        faces,
        media_reads_height,
        has_container_rules,
        rules,
        registered,
        normal: Index::default(),
        pseudo: Index::default(),
        hover_set,
        hover_layout_set,
        hover_sideways_set,
        checked_set,
        urls,
    };
    sheet.build_index();
    sheet
}

/// Scan `css[start..end]` for rules, tagging each with the given `media`
/// context, and recurse into nested `@media` blocks. `order` is threaded so
/// document order is preserved across (and into) media blocks.
/// The `@media` and `@container` blocks a rule sits inside.
#[derive(Clone, Default)]
struct Conds {
    media: Option<Rc<crate::media::MediaChain>>,
    container: Option<Rc<crate::media::ContainerChain>>,
}

fn parse_into(
    css: &str,
    start: usize,
    end: usize,
    media: Conds,
    rules: &mut Vec<Rule>,
    order: &mut u32,
    layer: &str,
    layers: &mut Layers,
    registered: &mut Registered,
    faces: &mut Vec<FontFace>,
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
                let chain = crate::media::MediaChain {
                    list: crate::media::MediaList::parse(&css[j..k]),
                    parent: media.media.clone(),
                };
                let inner = Conds { media: Some(Rc::new(chain)), container: media.container.clone() };
                let close = matching_brace(bytes, k, end);
                parse_into(css, k + 1, close, inner, rules, order, layer, layers, registered, faces);
                i = (close + 1).min(end);
            } else if css[i + 1..j].eq_ignore_ascii_case("container") {
                let mut k = j;
                while k < end && bytes[k] != b'{' && bytes[k] != b';' {
                    k += 1;
                }
                if k >= end || bytes[k] == b';' {
                    i = (k + 1).min(end);
                    continue;
                }
                let chain = crate::media::ContainerChain::parse(&css[j..k], media.container.clone());
                let inner = Conds { media: media.media.clone(), container: Some(Rc::new(chain)) };
                let close = matching_brace(bytes, k, end);
                parse_into(css, k + 1, close, inner, rules, order, layer, layers, registered, faces);
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
                    parse_into(css, k + 1, close, media.clone(), rules, order, layer, layers, registered, faces);
                }
                i = (close + 1).min(end);
            } else if css[i + 1..j].eq_ignore_ascii_case("font-face") {
                // `@font-face { font-family: X; src: url(...) format(...) }`
                // Only collected: the host fetches the file, the engine has no
                // network.
                let mut k = j;
                while k < end && bytes[k] != b'{' && bytes[k] != b';' { k += 1; }
                if k >= end || bytes[k] == b';' { i = (k + 1).min(end); continue }
                let close = matching_brace(bytes, k, end);
                if let Some(f) = parse_font_face(&css[k + 1..close]) { faces.push(f); }
                i = (close + 1).min(end);
            } else if css[i + 1..j].eq_ignore_ascii_case("property") {
                // `@property --name { … initial-value: V … }` — the value the
                // property has before anything sets it.
                let mut k = j;
                while k < end && bytes[k] != b'{' && bytes[k] != b';' {
                    k += 1;
                }
                if k >= end || bytes[k] == b';' {
                    i = (k + 1).min(end);
                    continue;
                }
                let close = matching_brace(bytes, k, end);
                let name = css[j..k].trim();
                if name.starts_with("--") {
                    for (p, v) in parse_decls(&css[k + 1..close]) {
                        if p.eq_ignore_ascii_case("initial-value") {
                            registered.push((String::from(name), v));
                        }
                    }
                }
                i = (close + 1).min(end);
            } else if css[i + 1..j].eq_ignore_ascii_case("layer") {
                // `@layer a, b;` declares order only; `@layer a { … }` and the
                // anonymous `@layer { … }` also carry rules. Skipping the block
                // like an unknown at-rule would drop the whole sheet of a page
                // that wraps its CSS in layers.
                let mut k = j;
                while k < end && bytes[k] != b'{' && bytes[k] != b';' {
                    k += 1;
                }
                let names = css[j..k.min(end)].trim();
                if k >= end || bytes[k] == b';' {
                    for n in names.split(',') {
                        let n = n.trim();
                        if !n.is_empty() {
                            layers.id(&qualify(layer, n));
                        }
                    }
                    i = (k + 1).min(end);
                    continue;
                }
                let (id, full) = if names.is_empty() {
                    layers.anon(layer)
                } else {
                    let full = qualify(layer, names);
                    (layers.id(&full), full)
                };
                let close = matching_brace(bytes, k, end);
                parse_into(css, k + 1, close, media.clone(), rules, order, &full, layers, registered, faces);
                let _ = id;
                i = (close + 1).min(end);
            } else {
                i = skip_at_rule(bytes, i).min(end);
            }
            continue;
        }
        let sel_start = i;
        // `p \{ … \}` is a selector containing two escaped braces, not a rule:
        // skipping the escape makes the block opener the next real `{`, which
        // swallows the rule after it — as the spec says an unmatched selector
        // should.
        while i < end && bytes[i] != b'{' && bytes[i] != b'}' {
            i += if bytes[i] == b'\\' { 2 } else { 1 };
        }
        if i >= end {
            break;
        }
        // A `}` where a selector was expected is a stray close — usually the end
        // of a nested style rule (`.a { .b { … } }`, which we flatten rather than
        // support) or malformed CSS. css-syntax-3 error recovery: consume it and
        // keep scanning instead of dropping the rest of the sheet.
        if bytes[i] == b'}' {
            i += 1;
            continue;
        }
        let sel_text = &css[sel_start..i];
        i += 1; // '{'
        let body_start = i;
        // Scan to the matching `}`, tracking `{}` depth (and skipping string
        // literals so a `{`/`}` inside `content:"…"` doesn't miscount), so a
        // nested rule's inner `}` does not end the parent early.
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
        let parsed = parse_decls(body);
        let mut decls = Vec::with_capacity(parsed.len());
        let mut decls_imp = Vec::new();
        let mut customs = Vec::new();
        let mut customs_imp = Vec::new();
        for (name, val) in parsed {
            let t = val.trim_end();
            let n = t.len();
            let imp = n >= 10 && t.is_char_boundary(n - 10)
                && t[n - 10..].eq_ignore_ascii_case("!important");
            let val = if imp { t[..n - 10].trim_end().into() } else { val };
            if name.starts_with("--") && name.len() > 2 {
                if imp { customs_imp.push((name, val)) } else { customs.push((name, val)) }
                continue;
            }
            let key = prop_key(&name);
            if imp { decls_imp.push((key, val)) } else { decls.push((key, val)) }
        }
        if !selectors.is_empty()
            && !(decls.is_empty() && decls_imp.is_empty()
                 && customs.is_empty() && customs_imp.is_empty()) {
            // The layer id is stable; `parse` turns it into a rank once the
            // whole sheet is read (a nested layer declared late moves its
            // siblings, so a rank handed out here would go stale).
            let lid = if layer.is_empty() { UNLAYERED } else { layers.id(layer) };
            rules.push(Rule { selectors, decls, decls_imp, customs, customs_imp,
                              order: *order, media: media.media.clone(),
                              container: media.container.clone(), layer: lid });
            *order += 1;
        }
    }
}

/// Index of the `}` that closes the `{` at/after `open` (or `end` if unbalanced).
pub(crate) fn matching_brace(bytes: &[u8], open: usize, end: usize) -> usize {
    let mut depth = 0i32;
    let mut i = open;
    while i < end {
        // An escaped brace is a character in a name, not a block boundary.
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
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
/// parses as a colour; other feature leaves are assumed supported (bias
/// towards rendering what the author intended).
pub fn supports_cond(cond: &str) -> bool {
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

pub fn supports_decl(prop: &str, val: &str) -> bool {
    let p = prop.to_ascii_lowercase();
    // A `var()` is valid in every property (css-variables-1 §3): it is
    // substituted at use, so nothing about it can fail at parse time. The
    // colour parser below would reject `var(--test, red)`, a common feature
    // probe that otherwise triggers a CSS-variables polyfill.
    if crate::vars::has_var(val) { return true }
    if p == "color" || p == "background" || p == "fill" || p == "stroke" || p.ends_with("-color") {
        // `transparent` IS a supported colour — ask the value parser, not the
        // one that folds transparency into "no value".
        return crate::color::parse_color_val(val).is_some();
    }
    !val.is_empty()
}

/// Case-insensitive prefix strip.
fn strip_ci_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    // `get`, not a range index: a decoded escape can put a multi-byte
    // character at the front, and `prefix.len()` may then fall inside it.
    if s.get(..prefix.len()).is_some_and(|h| h.eq_ignore_ascii_case(prefix)) {
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

/// Does a media query text apply to `m`? Used by `@media` preludes, by
/// `<source media=…>` in a `<picture>` and by `matchMedia`.
pub fn media_matches(prelude: &str, m: Media) -> bool {
    crate::media::MediaList::parse(prelude).matches(&m)
}

/// A media-feature `<length>` in px; `em`/`rem` count as 16px.
fn parse_px(v: &str) -> Option<f32> {
    let v = v.trim();
    // `em` and `rem` in a media query are relative to the initial value of
    // `font-size`, not the root element (media-queries-4 §1.3). Frameworks
    // write breakpoints this way (`@media (min-width:64rem)`).
    const INITIAL_FONT_PX: f32 = 16.0;
    for u in ["rem", "em"] {
        if let Some(n) = v.strip_suffix(u) {
            return n.trim().parse::<f32>().ok().map(|f| f * INITIAL_FONT_PX);
        }
    }
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
}

/// A media-feature length: a plain `<px>` or a `calc()` of `±<px>` terms
/// (e.g. `calc(640px - 1px)`). A value that fails to parse must not leave the
/// bound unset, or the `@media (max-width: …)` block would match every
/// viewport.
pub(crate) fn parse_media_px(v: &str) -> Option<f32> {
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
            // A comment is a token separator (css-syntax-3), not nothing —
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

/// Full name of a layer named `name` inside the layer `parent` (empty at the
/// top level). `@layer a { @layer b { … } }` is the layer `a.b`.
fn qualify(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        String::from(name)
    } else {
        format!("{parent}.{name}")
    }
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

/// Split a comma-separated selector list on top-level commas only — commas
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
/// unsupported alternative (one with a combinator we don't model, …) is
/// dropped rather than invalidating the whole list.
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
    // `::before`/`::after` may only sit on the last compound (the subject) —
    // `a:before b` has no meaning, so treat it as an unsupported selector
    // rather than mis-applying it to `b`.
    let last = compounds.len() - 1;
    if compounds.iter().enumerate().any(|(i, c)| i != last && c.pseudo != PseudoElem::None) {
        return None;
    }
    let pseudo = compounds[last].pseudo;
    let spec = specificity(&compounds);
    // Walking right to left, every compound past the first descendant/child
    // combinator is guaranteed to sit on the ancestor chain. Sibling
    // combinators stay at the subject's own level, so a compound reached
    // only through them is not an ancestor and must not be required.
    let mut anc_bloom: Bloom = [0u64; 4];
    let mut on_chain = false;
    for ci in (0..combs.len()).rev() {
        if matches!(combs[ci], Comb::Descendant | Comb::Child) {
            on_chain = true;
        }
        if on_chain {
            let c = &compounds[ci];
            if let Some(id) = &c.id {
                bloom_add(&mut anc_bloom, id);
            }
            for cl in &c.classes {
                bloom_add(&mut anc_bloom, cl);
            }
        }
    }
    Some(Selector { compounds, combs, spec, pseudo, anc_bloom })
}

/// Split into compound tokens + `>` tokens on whitespace / `>` boundaries.
///
/// An escape is copied through whole, terminating whitespace included: the
/// space in `.c\06C ass` belongs to the escape, so it is not a descendant
/// combinator, and `\>` is a character in a name rather than a child
/// combinator. Decoding happens later, in `parse_compound`.
fn tokenize_selector(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32; // inside [...] or :not(...) — don't split on combinators there
    let b = text.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'\\' {
            let (_, next) = escape_at(text, i);
            cur.push_str(&text[i..next]);
            i = next;
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        i += ch.len_utf8();
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
        hover: None,
        link: None,
        has: Vec::new(),
        pseudo: PseudoElem::None,
        dir: None,
    };
    let mut i = 0;
    // Leading type selector or universal `*`.
    if b[0] == b'*' {
        i = 1;
    } else if b[0].is_ascii_alphabetic() || b[0] == b'\\' {
        let (name, next) = ident_at(tok, i);
        i = next;
        c.tag = Some(name.to_ascii_lowercase());
    }
    while i < b.len() {
        match b[i] {
            b'.' | b'#' => {
                let marker = b[i];
                i += 1;
                let (name, next) = ident_at(tok, i);
                if next == i {
                    return None;
                }
                i = next;
                if marker == b'.' {
                    c.classes.push(name);
                } else {
                    c.id = Some(name);
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
                        // negation trivially true: drop the clause and keep the
                        // rule. That carries the "visually hidden until focused"
                        // idiom — `.skip-link:not(:focus){clip:rect(…)}` —
                        // which would otherwise leave the link visible.
                        let a = a.trim();
                        if !never_matches(a) {
                            c.not.push(parse_compound(a)?);
                        }
                    }
                    // `:is()`/`:where()` (and the legacy `:matches()` alias) —
                    // forgiving compound-alternative lists.
                    ("is" | "matches", Some(a)) => c.is_groups.push(parse_compound_list(&a)),
                    // `:has(<relative-selector-list>)`. An alternative we
                    // cannot express drops the whole selector.
                    ("has", Some(a)) => c.has.push(parse_has_list(&a)?),
                    ("where", Some(a)) => c.where_groups.push(parse_compound_list(&a)),
                    ("root", None) => c.root = true,
                    // Every element counts as defined. Not implemented: a
                    // custom element before its definition is not `:defined`.
                    ("defined", None) => {}
                    ("dir", Some(a)) => match a.trim().to_ascii_lowercase().as_str() {
                        "rtl" => c.dir = Some(true),
                        "ltr" => c.dir = Some(false),
                        _ => return None,
                    },
                    // No element is ever shown as a popover.
                    ("popover-open", None) => c.not.push(parse_compound("*")?),
                    ("empty", None) => c.empty = true,
                    ("checked", None) => c.checked = Some(true),
                    ("disabled", None) => c.disabled = Some(true),
                    ("enabled", None) => c.disabled = Some(false),
                    ("hover", None) => c.hover = Some(true),
                    ("any-link", None) => c.link = Some(LinkSel::Any),
                    ("link", None) => c.link = Some(LinkSel::Unvisited),
                    ("visited", None) => c.link = Some(LinkSel::Visited),
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
        // `:hover` is not here: it is a real state, so `:not(:hover)` has to
        // be evaluated rather than assumed true.
        ":focus" | ":focus-visible" | ":focus-within" | ":active" | ":target" | ":popover-open"
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
/// leading combinator plus one compound. Returns `None` — dropping the whole
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
        // scope.
        if rest.is_empty() || rest.contains([' ', '>', '+', '~', '\t']) {
            return None;
        }
        out.push(HasArg { comb, compound: parse_compound(rest)? });
    }
    (!out.is_empty()).then_some(out)
}

/// One compound's specificity as `(id, class, type)` counts. Recurses through
/// `:not()` (its argument's specificity) and `:is()` (its most specific
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
        + comp.disabled.is_some() as usize
        + comp.hover.is_some() as usize
        + comp.link.is_some() as usize) as u32;
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
/// Not `str::split(';')`: a semicolon inside a string or a `url()` belongs to
/// the value. `url("data:image/svg+xml;utf8,<svg …>")` is a common form for
/// icons, and cutting it at `;utf8` leaves a declaration that still parses
/// but points at nothing.
pub fn split_decls(body: &str) -> Vec<&str> {
    let b = body.as_bytes();
    let (mut out, mut start) = (Vec::new(), 0usize);
    let (mut depth, mut quote) = (0i32, 0u8);
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            // Escaped anything — inside a string it protects a quote, outside
            // it makes `;` or `:` part of a name. Either way the next byte is
            // not a delimiter.
            b'\\' => i += 1,
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
        // The first unescaped colon. `background\\: red` escapes it, so the
        // whole thing is one property name with no value — an invalid
        // declaration, which is what the test of that idiom checks.
        let db = decl.as_bytes();
        let mut k = 0usize;
        let cut = loop {
            if k >= db.len() {
                break None;
            }
            match db[k] {
                b'\\' => k += 2,
                b':' => break Some(k),
                _ => k += 1,
            }
        };
        let (p, v) = match cut {
            Some(k) => (decl[..k].trim(), decl[k + 1..].trim()),
            None => continue,
        };
        if !p.is_empty() && !v.is_empty() {
            // A property name and a keyword value are ident tokens, so an
            // escape in either is part of the name: `bac\kground: g\reen` is
            // `background: green` (css-syntax-3 §4.3.7).
            //
            // A value carrying a string or a `url()` is left alone, because
            // there the backslash protects a quote from ending the token.
            // Decoding it here would hand `url("data:…<svg xmlns=\"…\">")` to
            // a parser that stops at the first inner quote. Those tokens are
            // unescaped where they are consumed instead.
            let val = if v.contains('"') || v.contains('\'') || v.contains("url(") {
                String::from(v)
            } else {
                // An escape that decodes to whitespace or a control character
                // leaves an ident that contains that character — `red\9` is
                // not the keyword `red` — so nothing can ever match it and the
                // declaration is invalid. Decoding it and moving on would let
                // the trim in `apply_one` turn it back into the keyword.
                match unescape_value(v) {
                    Some(x) => x,
                    None => continue,
                }
            };
            // Custom property names are case-sensitive (css-variables-1 §2).
            let name = match unescape_value(p) {
                Some(x) if x.starts_with("--") => x,
                Some(x) => x.to_ascii_lowercase(),
                None => continue,
            };
            out.push((name, val));
        }
    }
    out
}

/// One `\…` escape starting at `i` (which must be the backslash): the code
/// point it stands for, and the index just past it — css-syntax-3 §4.3.7.
///
/// The hex form takes up to six digits and then swallows one following
/// whitespace, which is the only way to write `\6C` before a letter that is
/// itself a hex digit. Anything else escapes the next character literally,
/// which is how a class name gets to contain `:` or `/`.
fn escape_at(s: &str, i: usize) -> (Option<char>, usize) {
    let b = s.as_bytes();
    let mut j = i + 1;
    if j >= b.len() {
        // A trailing backslash is U+FFFD, not a parse error (§4.3.7).
        return (Some('\u{FFFD}'), j);
    }
    if !b[j].is_ascii_hexdigit() {
        // `\<newline>` is a line continuation inside a string: it stands for
        // nothing at all.
        if b[j] == b'\n' {
            return (None, j + 1);
        }
        let ch = s[j..].chars().next().unwrap_or('\u{FFFD}');
        return (Some(ch), j + ch.len_utf8());
    }
    let start = j;
    while j < b.len() && j - start < 6 && b[j].is_ascii_hexdigit() {
        j += 1;
    }
    let cp = u32::from_str_radix(&s[start..j], 16).unwrap_or(0);
    if j < b.len() {
        match b[j] {
            b'\r' if b.get(j + 1) == Some(&b'\n') => j += 2,
            b' ' | b'\t' | b'\n' | b'\r' | 0x0c => j += 1,
            _ => {}
        }
    }
    // NUL, a surrogate, and anything past the last code point are U+FFFD.
    let ch = if cp == 0 { None } else { char::from_u32(cp) };
    (Some(ch.unwrap_or('\u{FFFD}')), j)
}

/// Resolve every escape in `s`. Borrows unchanged when there is none, which is
/// every declaration on almost every page.
fn css_unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] == b'\\' {
            let (ch, next) = escape_at(s, i);
            if let Some(ch) = ch {
                out.push(ch);
            }
            i = next;
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Cow::Owned(out)
}

/// Unescape an unquoted value or a property name, or `None` when an escape
/// decoded to whitespace or a control character — see the call site.
fn unescape_value(s: &str) -> Option<String> {
    if !s.contains('\\') {
        return Some(String::from(s));
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] == b'\\' {
            let (ch, next) = escape_at(s, i);
            if let Some(ch) = ch {
                if ch.is_whitespace() || ch.is_control() {
                    return None;
                }
                out.push(ch);
            }
            i = next;
        } else {
            let ch = s[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Some(out)
}

/// Consume a CSS identifier at `i`, honouring escapes: the unescaped name and
/// the index after it.
///
/// The escape is what makes punctuation part of a name rather than a
/// delimiter — `.md\:flex` is one class, not a class followed by a pseudo —
/// which is how every utility-CSS framework spells a variant.
fn ident_at(s: &str, mut i: usize) -> (String, usize) {
    let b = s.as_bytes();
    let mut out = String::new();
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            let (ch, next) = escape_at(s, i);
            if let Some(ch) = ch {
                out.push(ch);
            }
            i = next;
        } else if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80 {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    (out, i)
}

#[cfg(test)]
mod tests {

    /// `var()` is valid in every property, so `@supports` / `CSS.supports`
    /// must say yes to it.
    #[test]
    fn a_var_is_valid_in_every_property() {
        assert!(super::supports_decl("color", "var(--test, red)"));
        assert!(super::supports_decl("background-color", "var(--x)"));
        assert!(super::supports_cond("(color: var(--test, red))"));
        // No `var()`: the colour parser still decides.
        assert!(super::supports_decl("color", "red"));
        assert!(!super::supports_decl("color", "totalerquatsch"));
        // `notvar(` is not a substitution.
        assert!(!super::supports_decl("color", "notvar(--x)"));
    }


    /// `@import` is how a hand-written site splits its CSS.
    #[test]
    fn import_urls_reads_every_spelling() {
        let css = r#"
            /* @import "commented-out.css"; */
            @charset "utf-8";
            @import 'base.css';
            @import "layout.css";
            @import url(components.css);
            @import url("panels/network.css");
            @import url('panels/threats.css') screen;
            @import "print.css" print;
            @layer a, b;
            .x { content: "@import fake.css"; }
            @media (min-width: 1px) { .y { color: red } }
        "#;
        assert_eq!(
            super::import_urls(css),
            alloc::vec![
                "base.css", "layout.css", "components.css",
                "panels/network.css", "panels/threats.css", "print.css",
            ]
        );
    }

    /// A brace or a semicolon inside a string must not end the scan, and an
    /// `@import` inside a block is invalid and ignored.
    #[test]
    fn import_urls_steps_over_strings_and_blocks() {
        let css = r#".a::before { content: "};@import 'no.css';" } @import "yes.css";"#;
        assert_eq!(super::import_urls(css), alloc::vec!["yes.css"]);
    }
    use super::*;
    use crate::dom;

    /// A standalone element to match against. `ElemInfo` borrows a live node,
    /// so the tree has to outlive it — leaked here so the assertions can pass
    /// `info(...)` inline.
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
        // Descendant — the default.
        assert!(hit("section:has(img)", "a"));
        assert!(!hit("section:has(img)", "b"));
        // Child: <img> is a grandchild, so `> img` must not match.
        assert!(!hit("section:has(> img)", "a"));
        assert!(hit("section:has(> p)", "a"));
        // Forward siblings need the parent, which the context carries.
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
        // `:nth-last-of-type` and `:only-of-type` need siblings that come after
        // the subject, read off the parent's children.
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
        // `:first-child` is not the same question: the second <span> is the
        // fourth child but only the second of its type.
        assert!(hit("span:nth-of-type(2)", 3));
        assert!(!hit("span:nth-child(2)", 3));
    }

    #[test]
    fn empty_matches_only_a_childless_element() {
        // `:empty` needs to see inside the element.
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
        // ul > li: <li> whose immediate parent is <ul>
        assert!(!ss.matched(&info("li", None, &[]), &[ul.clone()], &[], 0, Media::new(1000.0, false)).is_empty());
        assert!(ss.matched(&info("li", None, &[]), &[ul.clone(), div.clone()], &[], 0, Media::new(1000.0, false)).is_empty());
    }

    #[test]
    fn specificity_ranks_id_over_class_over_type() {
        let ss = parse("p { color: a } p.x { color: b } #p { color: c }");
        let e = info("p", Some("p"), &["x"]);
        let mut m = ss.matched(&e, &[], &[], 0, Media::new(1000.0, false));
        m.sort_by_key(|(layer, spec, order, _, _, _, _)| (*layer, *spec, *order));
        // ascending: type(p) < class(p.x) < id(#p)
        assert_eq!(m.len(), 3);
        assert!(m[0].1 < m[1].1 && m[1].1 < m[2].1);
    }

    #[test]
    /// A sheet that puts everything in `@layer` blocks must still apply its
    /// rules.
    #[test]
    fn layered_rules_survive_and_order_by_layer_not_source() {
        let ss = parse("@layer a, b; @layer b { p { color: b } } @layer a { p { color: a } }");
        let mut m = ss.matched(&info("p", None, &[]), &[], &[], 0, Media::new(800.0, false));
        assert_eq!(m.len(), 2, "both layered rules kept");
        m.sort_by_key(|(layer, spec, order, _, _, _, _)| (*layer, *spec, *order));
        // `@layer a, b` fixed the order, so `b` wins even though `a`'s rule is
        // written last — layer beats document order.
        assert_eq!(m.last().unwrap().3[0].1, "b");
    }

    #[test]
    fn unlayered_beats_every_layer_and_important_reverses_it() {
        let ss = parse("@layer a { p { color: a } } p { color: plain }");
        let mut m = ss.matched(&info("p", None, &[]), &[], &[], 0, Media::new(800.0, false));
        m.sort_by_key(|(layer, spec, order, _, _, _, _)| (*layer, *spec, *order));
        assert_eq!(m.last().unwrap().3[0].1, "plain", "unlayered wins a normal decl");
        // The `!important` pass runs the layer axis the other way round, so the
        // same unlayered rule is the weakest there (css-cascade-5 §6.4.4).
        m.sort_by_key(|(layer, spec, order, _, _, _, _)| (imp_rank(*layer), *spec, *order));
        assert_eq!(m.first().unwrap().3[0].1, "plain");
    }

    /// `@layer a; @layer b; @layer a.c;` has to sort `a.c` inside `a` — after
    /// `b` already exists — which is why ranks are assigned once at the end.
    #[test]
    fn a_nested_layer_sorts_inside_its_parent_not_at_the_end() {
        let ss = parse("@layer a; @layer b; @layer a.c { p { color: ac } } @layer b { p { color: b } }");
        let mut m = ss.matched(&info("p", None, &[]), &[], &[], 0, Media::new(800.0, false));
        m.sort_by_key(|(layer, spec, order, _, _, _, _)| (*layer, *spec, *order));
        assert_eq!(m.first().unwrap().3[0].1, "ac", "a.c sits inside a, before b");
        assert_eq!(m.last().unwrap().3[0].1, "b");
    }

    #[test]
    fn comments_at_rules_and_bad_selectors_are_tolerated() {
        let ss = parse(
            "/* c */ @media screen { p { color: x } } \
             a:hover { color: y } input[type=text] { color: z } \
             h1, h2 { color: ok }",
        );
        // :hover + [attr] selectors don't match these subjects; the @media block
        // is descended, and `screen` alone always matches, so its `p` rule is
        // fine either way …
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
        let root_spec = m.iter().find(|(_, _, _, d, _, _, _)| d[0].1 == "a").unwrap().1;
        let tag_spec = m.iter().find(|(_, _, _, d, _, _, _)| d[0].1 == "c").unwrap().1;
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
        m.sort_by_key(|(layer, spec, order, _, _, _, _)| (*layer, *spec, *order));
        assert_eq!(m.len(), 2, "both external + inline rules match");
        assert!(m[0].2 < m[1].2, "external rule has earlier document order");
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
        // desktop rules.
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

/// Can this `@font-face` source be read at all?
///
/// An explicit `format(...)` wins — it is the page's promise and more precise
/// than any extension. Without it the extension decides, and without that the
/// source is taken: a bare URL is an invitation to try.
///
/// Readable: WOFF2 (`woff2.rs`), WOFF1 (`woff.rs`) and raw sfnt.
/// `embedded-opentype` and `svg` existed only for browsers that are gone.
fn src_is_readable(url: &str, tail: &str) -> bool {
    let low = tail.to_ascii_lowercase();
    if let Some(i) = low.find("format(") {
        let f = &low[i + 7..];
        let f = &f[..f.find(')').unwrap_or(f.len())];
        let f = f.trim().trim_matches(['"', '\'']).trim();
        return matches!(f, "woff2" | "woff" | "truetype" | "opentype" | "");
    }
    // Extension, without query and fragment (`x.woff?v=2`, `x.eot?#iefix`).
    let path = &url[..url.find(['?', '#']).unwrap_or(url.len())];
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    !matches!(&*ext, "eot" | "svg" | "svgz")
}

/// One `@font-face` block. `None` if family or source is missing — without
/// both it is not a font.
fn parse_font_face(body: &str) -> Option<FontFace> {
    let mut family = 0u32;
    let mut src: Vec<String> = Vec::new();
    let mut weight = 400u16;
    let mut italic = false;
    for (p, v) in parse_decls(body) {
        if p.eq_ignore_ascii_case("font-family") {
            let n = v.trim().trim_matches(['"', '\'']).trim().to_ascii_lowercase();
            if !n.is_empty() { family = crate::style::hash_name(&n); }
        } else if p.eq_ignore_ascii_case("src") {
            // `url(a) format("woff2"), url(b)` — page order is the page's
            // ranking, so it is kept, minus sources in a format we cannot read.
            // css-fonts-4 §4.3 says "the first supported", not "the first": the
            // classic bulletproof `@font-face` starts with
            // `url(x.eot) format('embedded-opentype')` for IE, and taking that
            // one would drop the whole family to the built-in font.
            let mut rest = v.as_str();
            while let Some(i) = rest.find("url(") {
                rest = &rest[i + 4..];
                let Some(j) = rest.find(')') else { break };
                let u = rest[..j].trim().trim_matches(['"', '\'']).trim();
                rest = &rest[j + 1..];
                // The rest up to the next comma belongs to this source: its
                // `format(...)` sits there, if it has one.
                let tail = &rest[..rest.find(',').unwrap_or(rest.len())];
                if !u.is_empty() && src_is_readable(u, tail) { src.push(String::from(u)); }
            }
        } else if p.eq_ignore_ascii_case("font-weight") {
            let first = v.split_whitespace().next().unwrap_or("");
            weight = match first.to_ascii_lowercase().as_str() {
                "normal" => 400, "bold" => 700,
                n => n.parse().unwrap_or(400),
            };
        } else if p.eq_ignore_ascii_case("font-style") {
            italic = v.trim().to_ascii_lowercase().starts_with("italic")
                  || v.trim().to_ascii_lowercase().starts_with("oblique");
        }
    }
    if family == 0 || src.is_empty() { return None }
    Some(FontFace { family, src, weight: weight.clamp(1, 1000), italic })
}
