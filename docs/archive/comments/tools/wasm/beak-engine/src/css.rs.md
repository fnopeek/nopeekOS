# `tools/wasm/beak-engine/src/css.rs` @ 5e0102684

## L1-17 · `use alloc::borrow::Cow;`

```
//! css.rs — author stylesheet parsing (css-syntax-3 subset) + selector match.
//!
//! Slice-0.2 gave us the UA sheet (as data) + inline `style="…"`. This adds the
//! middle cascade layer: author rules from `<style>` blocks. The pipeline is
//! now the real thing —
//!
//! `​``text
//!   inherited(parent) → UA sheet → author <style> (specificity) → inline
//! `​``
//!
//! Selectors: type / `.class` / `#id` / `*`, compounds (`div.a#b`), descendant
//! (space) + child (`>`) combinators, comma lists. Right-to-left matching with
//! an ancestor stack + `(id, class, type)` specificity. Unsupported bits
//! (pseudo-classes, `[attr]`, `+`/`~` siblings, `@media` bodies) are dropped,
//! not mis-applied — forward-compatible like a browser (docs/spec/CONFORMANCE.md).
//! External `<link>` stylesheets need a sub-resource fetch → later; the parser
//! + cascade here are exactly what that will reuse.
```

## L28-29 · `#[derive(Clone)]`

```
/// The identity a selector matches against: tag + id + classes + all attributes
/// (names lowercased) for `[attr]` selectors.
```

## L32-36 · `pub el: &'a Element,`

```
/// The live element. Borrowed, not snapshotted: a selector has to be able
/// to look at an element's CHILDREN (`:empty`, `:has()`), and a copy of the
/// tag/id/class triple never can. This is also the shape `querySelector`
/// needs — matching against a live tree, not against copies of it — so
/// there is one matcher for the cascade and for scripting, not two.
```

## L38-42 · `pub state: ElemState,`

```
/// What only the runtime knows. `:checked`/`:disabled` read it today;
/// `:hover`/`:focus` are the same mechanism and stay `false` until there is
/// an event loop to flip them. Keeping them HERE rather than as scattered
/// "never matches" special cases is what makes that a one-line change
/// later instead of a hunt.
```

## L46 · `#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]`

```
/// Runtime state a selector can ask about (CSS Selectors 4 §4, §11).
```

## L57-60 · `ElemInfo::with_state(`

```
// What the DOCUMENT says. `:hover`/`:focus` have no document form and
// stay false until an event loop sets them; live `checked` after a
// click belongs to the form state and comes through `with_state`.
// Not covered: a `<fieldset disabled>` disabling its descendants.
```

## L71-73 · `pub fn of_hovered(el: &'a Element, hovered: &[u32]) -> ElemInfo<'a> {`

```
/// `of()` plus the pointer state: `hovered` is the ascending `seq` list of
/// the elements the pointer is inside (`Layout::hover_at`). A binary search
/// over a list that is empty on all but one path through the page.
```

## L86-90 · `pub fn with_state(el: &'a Element, state: ElemState) -> ElemInfo<'a> {`

```
/// Free — everything derived from the element now lives ON the element,
/// computed once by `Element::index_attrs` at parse time. This used to
/// split `class`, hash the Bloom bits and scan the attribute list four
/// times on EVERY construction, and it is constructed ~30 000× per layout:
/// 12.5 % of the layout, measured by doubling the work.
```

## L95 · `pub fn classes(&self) -> &'a [String] {`

```
/// `class`, split at parse time.
```

## L100 · `pub fn bloom(&self) -> &'a Bloom {`

```
/// This element's own Bloom bits (id + classes) — see `ancestor_bloom`.
```

## L114-116 · `#[cfg(test)]`

```
/// `:empty`: no element children and no non-whitespace text (Selectors 4
/// §14.3 — white-space-only text nodes do not disqualify, which is what
/// browsers do and what the `<td></td>` / `<p>\n</p>` idioms rely on).
```

## L129-134 · `fn is_link(&self) -> bool {`

```
/// `:any-link` — an `<a>`/`<area>`/`<link>` that carries an `href`
/// (Selectors 4 §8.1). A bare `<a name=…>` anchor is not a link.
///
/// Read from `attrs` on demand rather than indexed onto `Element`: only a
/// selector that asks reaches here, while anything cached on the element
/// is paid ~30 000× per layout.
```

## L144 · `Adjacent,`

```
/// `A + B` — B's immediately preceding element sibling matches A.
```

## L146 · `General,`

```
/// `A ~ B` — some preceding element sibling of B matches A.
```

## L150-163 · `pub type Bloom = [u64; 4];`

```
/// A 256-bit Bloom filter over the id/class names on an element's ANCESTOR
/// chain, so a descendant selector that cannot possibly match is rejected
/// without walking the chain at all.
///
/// Right-to-left matching is cheap for the subject (the index already
/// narrowed it) and expensive above it: `Comb::Descendant` walks every
/// ancestor before giving up, and giving up is the common case — measured on
/// Main_Page, ~14 600 interpreter instructions per candidate, which made
/// `matched()` 61 % of a whole layout.
///
/// Only ids and classes go in, never tags: a tag is barely selective, and
/// leaving it out keeps the filter sparse. Only the CONJUNCTIVE parts of a
/// compound count — `:is()`/`:not()` alternatives may match without their
/// name appearing, so including them would produce false NEGATIVES.
```

## L166 · `pub fn bloom_of(id: Option<&str>, classes: &[String]) -> Bloom {`

```
/// One element's filter bits. Called once per element at parse time.
```

## L184 · `for bit in [(h >> 3) & 255, (h >> 33) & 255] {`

```
// Two bits per name, taken from separate stretches of the hash.
```

## L190 · `#[inline]`

```
/// Every bit the selector needs must be present in the chain's filter.
```

## L199-200 · `pub fn ancestor_bloom(ancestors: &[ElemInfo]) -> Bloom {`

```
/// The filter for one ancestor chain (root → … → parent). Built once per
/// `matched_filtered` call and shared by every candidate.
```

## L212-215 · `#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]`

```
/// Which generated-content pseudo-element (if any) a selector targets. Only
/// `::before`/`::after` (single- or double-colon) are recognised; every other
/// pseudo-element (`::first-line`, `::placeholder`, …) is unsupported and
/// drops the whole selector at parse time, same as an unknown pseudo-class.
```

## L224 · `#[derive(Clone, Copy)]`

```
/// An `[attr]` attribute selector with its match operator.
```

## L227 · `Exists,  // [a]`

```
// [a]
```

## L228 · `Eq,      // [a=v]`

```
// [a=v]
```

## L229 · `Includes, // [a~=v] — whitespace-separated word list contains v`

```
// [a~=v] — whitespace-separated word list contains v
```

## L230 · `Dash,    // [a|=v] — v or v-…`

```
// [a|=v] — v or v-…
```

## L231 · `Prefix,  // [a^=v]`

```
// [a^=v]
```

## L232 · `Suffix,  // [a$=v]`

```
// [a$=v]
```

## L233 · `Substr,  // [a*=v]`

```
// [a*=v]
```

## L261-262 · `#[derive(Clone, Copy)]`

```
/// A structural pseudo-class, evaluated against the element's 1-based index
/// among its element siblings and the total sibling count `(index, count)`.
```

## L268 · `NthChild(i32, i32),     // matches index == a*n + b for some n ≥ 0`

```
// matches index == a*n + b for some n ≥ 0
```

## L269 · `NthLastChild(i32, i32), // same, counted from the end`

```
// same, counted from the end
```

## L270 · `FirstOfType,`

```
/// The same five, counted only among siblings with the subject's tag.
```

## L278-282 · `#[derive(Clone, Copy)]`

```
/// Where the subject sits among its element siblings: the 1-based index and
/// the count, and the same pair counted only among siblings that share its tag
/// (`:*-of-type`). The of-type half needs the FULL sibling list, not just the
/// preceding one — reachable only since the matcher borrows live elements and
/// can look at the parent's children.
```

## L287 · `of_type: &'a OfType<'a>,`

```
/// The of-type pair, counted on first ask — see `OfType`.
```

## L289-290 · `parent: Option<&'a Element>,`

```
/// The subject's parent, for looking at siblings that come AFTER it —
/// `:has(+ x)`, `:has(~ x)`. `None` when the caller supplied no ancestors.
```

## L294-306 · `struct OfType<'a> {`

```
/// `:*-of-type`'s two counters, computed on the FIRST selector that asks and
/// remembered for the rest of the element.
///
/// They cost two tag-comparing walks over the sibling list, and they used to
/// run eagerly at the top of every `Selector::matches` — i.e. once per
/// CANDIDATE selector, not once per element. Measured on a Tailwind-built page
/// (692 KB CSS, 61 candidate selectors per element) that was **57.5 % of the
/// whole layout**, spent filling in a pair that no selector on the page reads:
/// `:*-of-type` is rare, and the four cheap `:*-child` counters next to it are
/// free (`prev_siblings.len()`, `sib_count`).
///
/// So: one per element, borrowed by every candidate's `SibCtx`, evaluated only
/// if a `Structural::*OfType` actually reaches for it.
```

## L319 · `fn get(&self) -> (u32, u32) {`

```
/// `(idx, count)`, 1-based, among siblings sharing the subject's tag.
```

## L333-334 · `.unwrap_or(idx);`

```
// No parent on the path (the root, or a caller that did not supply
// one): the subject is the only element of its type we can see.
```

## L343 · `let Some(SibCtx { idx, count, of_type, .. }) = ctx else { return false }; // no sibling context → can't evaluate`

```
// no sibling context → can't evaluate
```

## L344 · `let nth = |a: i32, b: i32, i: u32| {`

```
// i == a*n + b for some integer n ≥ 0 (handles a ≤ 0 too).
```

## L375-385 · `fn matches_anc(comp: &Compound, ancestors: &[ElemInfo], i: usize) -> bool {`

```
/// Die vorangehenden Element-Geschwister eines VORFAHREN und ihre Gesamtzahl.
///
/// Bis 0.62.0 bekam ein Vorfahre gar keinen Geschwisterkontext — und damit
/// scheiterte jedes `:nth-*` auf ihm. `tr:nth-of-type(odd)` allein traf,
/// `tr:nth-of-type(odd) > td` nicht, und das ist die Form, in der JEDE
/// gestreifte Tabelle im Web geschrieben ist.
///
/// Kostet einen Durchlauf durch die Kinder des Grosselternteils, wird deshalb
/// nur geholt, wenn der Verbund ueberhaupt ein strukturelles Pseudo traegt.
/// Einen Verbund gegen einen VORFAHREN pruefen — mit Geschwisterkontext,
/// falls er einen braucht. Die Zwischenwerte leben hier, nicht beim Aufrufer.
```

## L425-429 · `struct HasArg {`

```
/// One alternative inside `:has(…)`: a combinator and the compound it applies
/// to, relative to the subject. Measured against the CSS four real pages load,
/// **223 of 243** `:has()` arguments are exactly this shape (178 descendant,
/// 20 `+`, 18 `>`, 7 `~`); anything more complex still drops its selector, the
/// same as before, so nothing regresses.
```

## L436-439 · `fn matches(&self, e: &ElemInfo, ctx: Option<SibCtx>) -> bool {`

```
/// Does anything in the scope this combinator opens match? `ctx` supplies
/// the subject's parent, needed only for the sibling combinators — so a
/// descendant/child `:has()` works even on an ancestor compound, where
/// there is no sibling context.
```

## L477-488 · `#[derive(Default)]`

```
/// The elements a sheet's `:hover` rules could possibly react to — the same
/// idea as Blink's invalidation sets, in the smallest form that pays.
///
/// It holds the names on the compound that CARRIES the `:hover`, not on the
/// selector's subject: in `nav:hover a` the pointer has to be inside the
/// `<nav>`, and the `<a>` restyles because of it. Since an ancestor's box
/// encloses its descendant's, hit-testing only the carriers finds exactly the
/// elements whose state can change.
///
/// Measured on Wikipedia's Main_Page: 8327 element boxes, of which a handful
/// carry a hover rule. Collecting all of them made 98.7 % of pointer movement
/// walk a list that could never answer anything but "no".
```

## L494-497 · `any: bool,`

```
/// A `:hover` compound that names nothing (`*:hover`, `[data-x]:hover`, or
/// a `:is(…)` whose alternatives carry the names) can match anything, so
/// no filtering is possible. Rare, and being wrong here would freeze the
/// page under the pointer — so it degrades to "collect everything".
```

## L506-508 · `pub fn may_match(&self, el: &Element) -> bool {`

```
/// Could a `:hover` rule react to the pointer being inside this element?
/// Called once per element box per layout, so it answers "no" as early as
/// it can — an empty set is one bool.
```

## L524-533 · `fn add(&mut self, c: &Compound) {`

```
/// Record a compound that tests `:hover`, under its MOST SELECTIVE name.
///
/// Only one name, the way the sheet's own rule index picks one: every name
/// on the compound has to match for the selector to, so the narrowest of
/// them is enough — and `may_match` ORs what it is given. Recording all of
/// them made `li.gallerybox:hover` claim every `<li>` on the page and
/// `div.gallerytextwrapper:hover` every `<div>`. Harmless for hit-testing,
/// where a false yes only costs a rectangle, but fatal for deciding
/// whether a pointer move can be answered by repainting: one gallery rule
/// dragged a whole Wikipedia article onto the slow path.
```

## L542 · `self.any = true;`

```
// Nothing to filter on — the safe answer is "anything".
```

## L548-554 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// Which link pseudo-class a compound asks for.
///
/// We keep no browsing history, so `:visited` matches NOTHING. That is not a
/// gap to close later: a history-aware `:visited` is the classic history-leak
/// side channel, which is why browsers restrict it to a handful of colour
/// properties and lie to `getComputedStyle`. Never-visited is the honest and
/// safe answer, and it makes `:link` and `:any-link` the same test.
```

## L562-563 · `struct Compound {`

```
/// A compound selector: optional type + id + classes + `[attr]` + `:not(…)` +
/// structural pseudo-classes, all of which must hold.
```

## L570-574 · `is_groups: Vec<Vec<Compound>>,`

```
/// `:is(…)`/`:matches(…)` groups: each group is a list of alternative
/// compounds; the compound matches only if EACH group has at least one
/// matching alternative. Contributes its most specific argument's
/// specificity (like a class group). Only compound alternatives are
/// supported (no combinators inside) — enough for MediaWiki/Bootstrap.
```

## L576 · `where_groups: Vec<Vec<Compound>>,`

```
/// `:where(…)` groups: match like `:is()` but contribute ZERO specificity.
```

## L579-581 · `root: bool,`

```
/// `:root` — the document's root element. In an HTML document that is
/// always `<html>`, so this is a tag test that keeps pseudo-class
/// specificity.
```

## L583-585 · `empty: bool,`

```
/// `:empty` — no children other than white-space-only text (Selectors 4
/// §14.3). Only expressible since the matcher borrows the live element:
/// a snapshot of tag/id/class can never answer "what is inside".
```

## L587-589 · `checked: Option<bool>,`

```
/// State pseudo-classes, each `Some(want)` when the selector asks for it.
/// They read `ElemInfo::state` — the same field `:hover`/`:focus` will use
/// once there is an event loop, which is why they live together.
```

## L592-595 · `hover: Option<bool>,`

```
/// `:hover`. Reads `ElemState::hover`, which the shell sets from the
/// pointer position — so a rule only wins while the pointer is inside the
/// element's box, and every element the pointer is inside is hovered, not
/// just the innermost (`div:hover .child` is why).
```

## L597-599 · `link: Option<LinkSel>,`

```
/// `:link` / `:visited` / `:any-link`. Before these existed the whole
/// selector was dropped, so `a:link{color:…}` — how a page states its link
/// colour — silently lost to the UA default.
```

## L601-603 · `has: Vec<Vec<HasArg>>,`

```
/// One entry per `:has()` on this compound; the inner list is its
/// comma-separated alternatives, so an entry matches if ANY of them does
/// and several `:has()` all have to hold.
```

## L605-606 · `pseudo: PseudoElem,`

```
/// `::before`/`::after` on this compound (only valid on the LAST compound
/// of a selector — checked in `parse_selector`).
```

## L611-613 · `fn matches(&self, e: &ElemInfo, ctx: Option<SibCtx>) -> bool {`

```
/// `ctx = Some((index, count))` provides the sibling position for structural
/// pseudo-classes; `None` (an ancestor with no known position) makes any
/// structural pseudo fail (the selector is dropped rather than mis-applied).
```

## L651-652 · `LinkSel::Any | LinkSel::Unvisited => e.is_link(),`

```
// No history → nothing is visited, so `:link` degenerates to
// `:any-link` and `:visited` never matches. See `LinkSel`.
```

## L660 · `if self.not.iter().any(|n| n.matches(e, ctx)) {`

```
// :not(x) — none of the negated compounds may match.
```

## L664-666 · `if !self.is_groups.iter().chain(self.where_groups.iter()).all(|group| group.iter().any(|alt| alt.matches(e, ctx))) {`

```
// :is(…)/:where(…) — every group must have at least one matching
// alternative (an empty group, e.g. all-unsupported args, matches
// nothing → the compound never matches).
```

## L670-673 · `self.has.iter().all(|group| group.iter().any(|a| a.matches(e, ctx)))`

```
// :has() LAST. It is the only test that walks a subtree, and every
// cheap test above has already ruled out the elements it would walk
// for nothing — `.foo:has(.bar)` descends only into elements that are
// actually `.foo`.
```

## L677-679 · `fn wants_hover(&self) -> bool {`

```
/// Does this compound (or anything nested in it) test `:hover`? Nested
/// counts: `:is(a:hover, b)` and `:not(:hover)` both change what the
/// pointer does, and missing one would leave the page frozen under it.
```

## L688-691 · `fn wants_checked(&self) -> bool {`

```
/// Tests `:checked` — the one control state a selector can actually read.
/// `:focus`/`:focus-within`/`:active` are in `never_matches`, so a control
/// gaining or losing the keyboard cannot restyle anything through the
/// cascade; only a checkbox or radio can.
```

## L701-702 · `pub struct Selector {`

```
/// A complex selector: compounds left→right, with the combinator that precedes
/// each compound after the first (`combs[k]` sits left of `compounds[k+1]`).
```

## L707-709 · `anc_bloom: Bloom,`

```
/// Names that MUST appear somewhere on the subject's ancestor chain —
/// see `Bloom`. Empty for a selector with no ancestor part, which then
/// always passes the pre-test.
```

## L711-712 · `pseudo: PseudoElem,`

```
/// `::before`/`::after` this selector targets (`None` = a normal
/// selector, matching the real element).
```

## L717-719 · `fn collect_hover(&self, out: &mut HoverSet) {`

```
/// Record every compound of this selector that tests `:hover`. An ancestor
/// compound counts as much as the subject's — `nav:hover a` restyles the
/// link, and the pointer is inside the `<nav>`.
```

## L726-734 · `fn collect_checked(&self, out: &mut HoverSet) {`

```
/// Record every `:hover` carrier whose rule restyles a SIBLING — anything
/// with `+` or `~` to the right of the carrier.
///
/// `li:hover + li` is a real idiom, and what it restyles is not inside the
/// element the pointer is in. A repaint that walks the carrier's subtree
/// cannot see it, so those carriers take the slow path.
/// Record every compound that tests `:checked`, wherever it sits in the
/// selector — an ancestor carrier (`:checked ~ .menu`) restyles something
/// the control does not contain, so the ONE set has to cover both.
```

## L753-756 · `fn matches(&self, subject: &ElemInfo, ancestors: &[ElemInfo], prev_siblings: &[ElemInfo], sib_count: u32, anc_bloom: &Bl`

```
/// Right-to-left match: the last compound must match `subject`, then earlier
/// compounds must match ancestors per their combinators. `ancestors` is
/// root→…→parent order. Descendant matching is nearest-first (no backtrack —
/// enough for content selectors; noted as a shortcut).
```

## L758-762 · `if !bloom_covers(&self.anc_bloom, anc_bloom) {`

```
// O(1) rejection before the ancestor walk: if a name this selector
// requires above the subject is absent from the whole chain, no amount
// of walking will find it. False positives are fine (the real match
// runs anyway); false negatives are not, which is why only conjunctive
// ids/classes went into the filter.
```

## L766-771 · `let subj_ctx = Some(SibCtx {`

```
// The subject's structural pseudo-classes evaluate against its 1-based
// sibling index (preceding count + 1) and the total sibling count —
// and, for `:*-of-type`, against the same pair restricted to its tag.
// Those two are the expensive half and are shared across every
// candidate selector of this element, counted only if one asks (see
// `OfType`); the `:*-child` pair below is free.
```

## L782 · `let mut anc = ancestors.len() as isize - 1; // immediate parent`

```
// immediate parent
```

## L783 · `let mut sib = prev_siblings.len() as isize - 1; // immediately preceding sibling`

```
// immediately preceding sibling
```

## L784-787 · `let mut at_subject = true;`

```
// Sibling combinators (`+`/`~`) only resolve while we're still matching at
// the subject's own level — once an ancestor combinator moves the context
// up, we no longer have that ancestor's siblings, so drop rather than
// mis-apply (covers the common `A + B`, `A ~ B`, `.x .a + .b` cases).
```

## L848-852 · `#[derive(Clone, Copy)]`

```
/// A parsed `@media` condition. We evaluate only the width features that
/// Bootstrap and WordPress breakpoints rely on (`min-width`/`max-width`, in px)
/// plus the `screen`/`all` media types; a query naming any other media type or
/// feature (orientation, prefers-*, print, …) is marked `understood = false`
/// and never matches, so we never mis-apply its rules.
```

## L857 · `scheme_dark: Option<bool>,`

```
/// `prefers-color-scheme` — `Some(true)` wants dark, `Some(false)` light.
```

## L860-865 · `negated: bool,`

```
/// A leading `not`, which negates the WHOLE query (Media Queries 4 §3.1),
/// not one feature of it. `@media not screen and (max-width: 480px)` is
/// how a mobile-first page states its desktop rules — dropping it leaves
/// a 1400px window rendering the phone layout, which is what Google's
/// consent page did: buttons at full window width, both the phone and the
/// desktop set of them on screen at once.
```

## L871-873 · `if !self.understood {`

```
// A query we did not understand never matches — not even negated.
// `not <something we cannot evaluate>` is not "true", it is unknown,
// and unknown has to fail closed in both directions.
```

## L884-886 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// What the page is being rendered INTO — everything `@media` can ask about.
/// One value instead of a widening list of parameters, and it is `Copy`, so it
/// threads through the cascade the way `viewport_w` used to.
```

## L890-891 · `pub dark: bool,`

```
/// The user's colour-scheme preference. On this system that IS the page
/// theme: the shell resolves it from the compositor palette.
```

## L901-902 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// One `selectors { declarations }` rule; `order` is document position (for
/// same-specificity tie-breaking, last wins). `media` is the `@media`
```

## L904-921 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Every property name the cascade can resolve, turned into a number ONCE —
/// when the stylesheet is parsed.
///
/// A real page applies ~150 000 declarations per layout, and finding the right
/// branch by comparing the name as a string cost ~10 % of the whole layout
/// (measured 2026-08-14): a chain of ~120 comparisons, repeated for a mapping
/// that never changes. With sequential discriminants the same dispatch is a
/// jump table.
///
/// The macro is the single source of truth: the enum and `prop_key` come from
/// one list, so a name can never point at a variant that does not exist. The
/// match in `apply_one` is exhaustive, so a NEW variant fails to compile until
/// somebody handles it.
/// What a declaration of this property can move.
///
/// The point is the pointer: a `:hover` rule that only recolours something
/// cannot move a single box, and answering it must not cost a layout. Blink
/// carries the same idea as an invalidation class per property.
```

## L924 · `Layout,`

```
/// Can change geometry — a layout has to run.
```

## L926 · `Paint,`

```
/// Can only change what a box looks like, never where anything sits.
```

## L928-931 · `Nothing,`

```
/// We do not implement it, so applying it changes NOTHING. `apply_one`
/// has no arm for it and a custom property has already been substituted
/// away by `vars.rs`. This is not a guess: an unknown declaration is a
/// no-op, so a rule made only of them is free to gain or lose.
```

## L940-942 · `Unknown,`

```
/// Anything we do not implement. Custom properties land here too —
/// `vars.rs` substitutes `var()` textually before this point, so a
/// surviving `--name` declaration has nothing left to say.
```

## L947-949 · `pub fn prop_key(name: &str) -> Prop {`

```
/// Resolve a declaration's property name. An EXACT string match, so an
/// unknown name can never be mistaken for a supported one — which is
/// why this is not a hash.
```

## L957-960 · `pub const PROP_N: usize = 1 + [$($name),*].len();`

```
/// Number of variants incl. `Unknown` — the width of any per-property
/// table. A per-property census is then three lines: an
/// `[AtomicU64; PROP_N]`, one `fetch_add` at the top of `apply_one`,
/// and a dump keyed by `prop_name`.
```

## L963 · `pub fn prop_name(p: Prop) -> &'static str {`

```
/// The canonical name, for diagnostics.
```

## L971-973 · `pub fn prop_class(p: Prop) -> Class {`

```
/// What a declaration of this property can move. The class is written
/// next to the name in the one list, so a NEW property cannot be added
/// without saying which it is.
```

## L996 · `ObjectFit = "object-fit" @ Paint,`

```
// Opera's prefix, and the only one Bootstrap still ships for `object-fit`.
```

## L1175 · `pub struct Rule {`

```
/// condition list it sits inside (comma = OR), or `None` when unconditional.
```

## L1178-1180 · `decls: Vec<(Prop, String)>,`

```
/// Normal declarations, `!important` already stripped at PARSE time.
/// The cascade runs two passes over every matched rule, so leaving the
/// suffix on meant re-scanning every value's tail twice per element.
```

## L1182-1187 · `customs: Vec<(String, String)>,`

```
/// Custom Properties (`--name: wert`) mit ihrem NAMEN.
///
/// Sie koennen nicht in `decls` stehen: dort ist der Name schon zu einem
/// `Prop` geworden, und fuer `--irgendwas` ist das `Prop::Unknown` — der
/// Name waere weg. Sie brauchen ihn aber, denn sie werden je Element
/// kaskadiert und vererbt, wie jede andere geerbte Eigenschaft auch.
```

## L1190-1191 · `decls_imp: Vec<(Prop, String)>,`

```
/// The `!important` ones, same shape. Usually empty, which is the point:
/// pass 2 then has nothing to walk.
```

## L1195-1197 · `layer: u16,`

```
/// Cascade-layer rank (css-cascade-5 §6.4.4). `UNLAYERED` for a rule that
/// sits outside every `@layer`; otherwise the layer's position in
/// declaration order, so a LATER layer wins a normal declaration.
```

## L1201-1203 · `pub const UNLAYERED: u16 = u16::MAX;`

```
/// Rank of a rule that is in no layer. Normal declarations from unlayered
/// rules beat every layered one, so it sorts highest; the `!important` pass
/// reverses the axis (`imp_rank`), which drops it to the bottom there.
```

## L1206-1207 · `#[inline]`

```
/// Layer priority for the `!important` pass, where the whole layer axis
/// reverses: the FIRST-declared layer wins, and unlayered loses to all.
```

## L1213-1221 · `#[derive(Default)]`

```
/// The layers a stylesheet declared — css-cascade-5 §6.4.
///
/// A layer's position is fixed by its FIRST declaration, whether that came
/// from `@layer a, b;` (order only) or `@layer a { … }` (order + rules), and
/// nesting makes a dotted path. Ranks cannot be handed out while parsing:
/// `@layer a; @layer b; @layer a.c;` has to sort `a.c` INSIDE `a`, which
/// moves `b` after every rule tagged `b` already exists. So this is a
/// push-only arena of ids, and `ranks()` turns ids into positions once the
/// sheet is fully read.
```

## L1228-1229 · `fn id(&mut self, name: &str) -> u16 {`

```
/// Stable id for `name`, registering it — and any missing ancestor, since
/// `@layer a.b` alone still creates `a` — on first sight.
```

## L1241-1242 · `fn anon(&mut self, parent: &str) -> (u16, String) {`

```
/// A fresh anonymous layer (`@layer { … }`), which no later rule can name.
/// The NUL keeps the generated name out of the author's namespace.
```

## L1253-1255 · `fn ranks(&self) -> Vec<u16> {`

```
/// `id -> rank`, in tree order. A layer's sort key is the chain of ids of
/// its own prefixes: `a.c` keys as `[id(a), id(a.c)]`, which puts it after
/// `a` and before any later sibling of `a`, at any nesting depth.
```

## L1281-1291 · `pub type Matched<'a> = (u16, u32, u32, &'a [(Prop, String)], &'a [(Prop, String)],`

```
/// One rule that matched: `(layer rank, specificity, document order, normal
/// declarations, `!important` declarations)`. The caller sorts ascending and
/// applies the normal pass first, then the important one — and because the
/// layer axis reverses between the two passes, the important pass re-sorts on
/// `imp_rank` rather than reusing pass 1's order.
/// Eine getroffene Regel: (Ebene, Spezifitaet, Reihenfolge, normal,
/// `!important`, Custom Properties normal, Custom Properties `!important`).
///
/// Die Custom Properties fahren MIT und nicht in einem zweiten Lauf: das
/// Treffen selbst ist der teuerste Teil der Kaskade (~95 % der Layoutzeit auf
/// einer echten Seite), und zweimal zu treffen waere zweimal zu bezahlen.
```

## L1295-1313 · `pub type Registered = alloc::vec::Vec<(String, String)>;`

```
/// Der Anfangswert einer mit `@property` angemeldeten Custom Property.
///
/// **Warum das kein Randfall ist.** Tailwind v4 legt seine Schatten als
/// Liste von Platzhaltern an:
///
///     box-shadow: var(--tw-inset-shadow), … , var(--tw-shadow)
///
/// und meldet jeden Platzhalter mit `@property --tw-shadow{initial-value:
/// 0 0 #0000}` an. Die Ersatzdeklarationen daneben stehen in einem
/// `@supports`, das nur in aelteren Browsern gilt. Ohne `@property` hat also
/// KEINE dieser Variablen einen Wert, das `var()` bleibt stehen, die
/// Deklaration ist ungueltig — und auf einer Tailwind-Seite hat nichts einen
/// Schatten.
///
/// **Vereinfachung, benannt:** `inherits: false` wird nicht beachtet. Die
/// Anfangswerte werden auf der Wurzel gesetzt und vererben sich damit nach
/// unten. Fuer die Platzhalter, um die es geht, ist das dasselbe; fuer eine
/// Eigenschaft, die ein Vorfahre setzt und ein Nachfahre NICHT erben soll,
/// waere es zu grosszuegig.
```

## L1316-1326 · `#[derive(Clone, Debug)]`

```
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
/// Eine Schrift, die die SEITE mitbringt (`@font-face`).
```

## L1329 · `pub family: u32,`

```
/// Streuwert des Familiennamens (`style::hash_name` auf klein).
```

## L1331-1332 · `pub src: Vec<String>,`

```
/// Die Quellen in der Reihenfolge der Seite. Die erste, die beak lesen
/// kann, gewinnt — genau wie im Browser.
```

## L1334 · `pub weight: u16,`

```
/// 100..900. Ein Bereich (`400 700`) wird auf seinen Anfang gelegt.
```

## L1341-1342 · `pub faces: Vec<FontFace>,`

```
/// Was die Seite an Schriften mitbringt. NUR die Angaben — die Bytes holt
/// der Wirt (siehe `Engine::take_pending_fonts`).
```

## L1344 · `pub registered: Registered,`

```
/// Anfangswerte aus `@property` — siehe [`Registered`].
```

## L1346 · `normal: Index,`

```
/// Selectors targeting real elements.
```

## L1348-1351 · `pseudo: Index,`

```
/// Selectors ending in `::before`/`::after`, kept apart because the
/// cascade runs THREE times per element (the element, then each
/// generated box). Sharing one index made each pass walk the other
/// two passes' candidates only to reject them.
```

## L1353-1355 · `pub hover_set: HoverSet,`

```
/// Which elements a `:hover` rule here could possibly react to. On a page
/// without hover rules it is empty and pointer movement costs exactly
/// nothing; on a page with them it is a small fraction of the document.
```

## L1357-1363 · `pub hover_layout_set: HoverSet,`

```
/// The subset of `hover_set` whose rules declare at least one property that
/// can MOVE something. A pointer entering an element outside this set can
/// only change how that element looks — which is the whole point: a repaint
/// instead of a layout.
///
/// Split per RULE, not per page: one `:hover{display:none}` somewhere must
/// not make every recolouring hover on the page expensive.
```

## L1365-1368 · `pub hover_sideways_set: HoverSet,`

```
/// Carriers whose rules restyle a SIBLING (`li:hover + li`). Geometry may
/// well be untouched, so this is not about the layout claim — it is about
/// what a repaint can FIND: it walks the carrier's own subtree, and a
/// sibling is not in it.
```

## L1370-1373 · `pub checked_set: HoverSet,`

```
/// Every `:checked` carrier, by name. A control whose CHECKED state changes
/// and that may match one of these has to be laid out: `:checked` can move
/// boxes (the checkbox hack is `input:checked ~ .menu{display:block}`), and
/// repainting the control alone would leave that menu shut.
```

## L1375-1381 · `urls: BTreeMap<u64, String>,`

```
/// Every `url(…)` appearing anywhere in the sheet, keyed by `url_key`.
///
/// `ComputedStyle` is `Copy`, so it cannot carry the URL itself — it
/// stores the key and looks the string up here. Collecting them from the
/// source text rather than threading an interner through the cascade
/// keeps `apply_one` a pure function: a URL that wins the cascade is by
/// definition present in the text it was parsed from.
```

## L1385-1386 · `pub fn url_key(url: &str) -> u64 {`

```
/// Stable 64-bit key for a `url()` value (FNV-1a). Case-sensitive on purpose:
/// a `data:` payload's base64 is case-significant.
```

## L1396-1404 · `fn url_at(text: &str, from: usize) -> Option<(Cow<'_, str>, usize)> {`

```
/// The next `url(…)` in `text` at or after `from`, as `(url, index after it)`.
///
/// One scanner for both callers, because the ways to get this wrong are the
/// same in each: a QUOTED url may legally contain `)`, a url is rarely the
/// whole value (`background: red url(x) no-repeat`), and a quoted one may
/// carry BACKSLASH-ESCAPED quotes — which is exactly how an inline SVG
/// `data:` URI is written (`url("data:image/svg+xml,<svg xmlns=\"…\">")`).
/// Stopping at the first inner quote truncates the payload into something that
/// still looks like a URL and silently decodes to nothing.
```

## L1427-1431 · `fn unescape(s: &str) -> Cow<'_, str> {`

```
/// Undo CSS string escaping in a URL. This used to leave hex escapes alone
/// on the grounds that decoding one would corrupt more than it fixed; now
/// that there is a spec-correct decoder it must use the SAME one the
/// declaration parser does, or the `url()` table and the declaration that
/// names it disagree about the key and the image never resolves.
```

## L1436 · `pub fn url_value(v: &str) -> Option<Cow<'_, str>> {`

```
/// The first `url(…)` in a declaration value, unquoted and unescaped.
```

## L1442-1446 · `fn collect_urls(css: &str, out: &mut BTreeMap<u64, String>) {`

```
/// Record every `url(…)` in a block of CSS text into `out`.
///
/// Runs over the raw text, not the parsed rules, because `url()` can appear in
/// any property (`background-image`, `mask-image`, `list-style-image`, …) and
/// we want the table complete before the cascade picks a winner.
```

## L1457-1464 · `type Cand = (u32, u32, Bloom);`

```
/// Candidate `(rule, selector)` pairs bucketed by the most selective simple
/// selector of a selector's rightmost compound. A selector lives in exactly
/// one bucket, so collecting several buckets cannot produce duplicates.
/// A bucket entry: which rule + selector, and what that selector needs to
/// find on the ancestor chain. Carrying the requirement HERE rather than
/// looking it up through `rules[ri].selectors[si]` is what lets `candidates`
/// reject before collecting: measured on Main_Page, the tag buckets offer
/// 577 828 candidates per layout and 82 % of them cannot match.
```

## L1472-1473 · `universal: Vec<Cand>,`

```
/// Rightmost compound names no tag, id or class (`*`, `[attr]`, a bare
/// `:not(...)`) — must be tried for every element.
```

## L1480 · `if let Some(id) = &last.id {`

```
// Most selective first: an id narrows far more than a tag.
```

## L1535 · `pub fn url(&self, key: u64) -> Option<&str> {`

```
/// The `url()` string behind a key held by a `ComputedStyle`.
```

## L1540-1541 · `pub fn add_urls(&mut self, text: &str) {`

```
/// Record `url()`s from text outside the sheet itself (inline `style`
/// attributes), so the cascade can resolve a key that came from there.
```

## L1546 · `fn build_index(&mut self) {`

```
/// Build the selector index. Called once after parsing.
```

## L1567-1571 · `pub fn matched<'a>(`

```
/// All declaration blocks that match `subject` itself (given its
/// `ancestors`) — i.e. selectors with no trailing `::before`/`::after`.
/// Each is tagged with the winning selector's specificity + document
/// order; caller sorts ascending and applies in order (later overrides
/// earlier).
```

## L1583-1584 · `pub fn matched_pseudo<'a>(`

```
/// Same as `matched`, but for `subject`'s `::before`/`::after` generated
/// box — only selectors ending in that pseudo-element are considered.
```

## L1606-1610 · `let chain = ancestor_bloom(ancestors);`

```
// Only selectors whose rightmost compound could match this element are
// worth testing — that is what the index buys. Everything else is
// unchanged: same tests, same specificity, same result.
// Measured on a real page: ~97 candidates per element, so an empty Vec
// reallocates about seven times per call, 9 000 times per layout.
```

## L1612 · `let of_type = OfType::new(subject.tag(), prev_siblings, ancestors.last().map(|p| p.el));`

```
// One per element, shared by every candidate below.
```

## L1618-1623 · `let mut out: Vec<Matched<'a>> = Vec::with_capacity(16);`

```
// One rule contributes one entry, at the highest specificity among its
// matching selectors. That grouping used to be bought with a sort over
// every candidate; it is cheaper to dedupe the HITS, because there are
// ~73 candidates per element and ~2 of them match. `rule.order` is
// unique per rule and identifies it. Output order is not a contract —
// every caller sorts by (layer, spec, order) before reading.
```

## L1631-1632 · `if let Some(conds) = &rule.media {`

```
// Skip rules inside an `@media` block whose condition doesn't hold
// at this viewport width.
```

## L1658 · `pub fn collect(dom: &Dom, media: Media) -> Stylesheet {`

```
/// Gather + parse every `<style>` block in the document into one stylesheet.
```

## L1663-1672 · `pub fn style_text(dom: &Dom) -> String {`

```
/// Author stylesheet = already-fetched external `<link>` CSS (document order:
/// `<head>` first) followed by inline `<style>` blocks. The shell fetches the
/// linked files (the engine is host-free) and hands their bytes in as `external`.
/// Der Text aller `<style>`-Bloecke des Dokuments, in Baumreihenfolge.
///
/// **Das ist alles, was die Kaskade am BAUM haengt** — das und die `url()` in
/// `style`-Attributen, die `add_inline_urls` nachtraegt. Deshalb steht diese
/// Funktion oeffentlich hier: wer das gesammelte Blatt zwischenspeichert,
/// muss auf DIESEN Inhalt schluesseln und nicht auf „irgendetwas am Baum hat
/// sich bewegt". Ein `classList.toggle` aendert kein Stilblatt.
```

## L1679-1682 · `pub fn add_inline_urls(dom: &Dom, sheet: &mut Stylesheet) {`

```
/// Die `url()` aus den `style`-Attributen des Baums in die Tabelle eines
/// Blattes nachtragen. Ueber `BTreeMap::or_insert_with` mehrfach anwendbar:
/// ein zwischengespeichertes Blatt bekommt damit die Adressen des AKTUELLEN
/// Baums, ohne neu geparst zu werden.
```

## L1696-1710 · `let mut sheet = parse(&css);`

```
// Custom Properties werden NICHT mehr hier ersetzt.
//
// Bis 0.59.0 lief davor ein Textlauf ueber das ganze Blatt, mit einer
// globalen Karte: ein Wert je Name fuer das ganze Dokument. Das traegt
// das Muster, fuer das es gebaut war (`:root` setzt eine Palette), und
// bricht bei dem, das jedes Rahmenwerk benutzt — die Basisklasse liest
// die Variable, jede Variante setzt sie neu. Eine Regel, die ein Element
// NICHT trifft, entschied dessen Wert.
//
// Eine Custom Property ist eine GEERBTE Eigenschaft. Sie gehoert in die
// Kaskade, je Element, und dort steht sie jetzt: `Rule` haelt sie mit
// Namen, `Matched` traegt sie mit, `style::resolve_in` kaskadiert und
// vererbt sie, und `vars::expand` setzt sie beim Anwenden eines Wertes
// ein. Damit fallen auch die Heuristiken weg, mit denen der Textlauf
// raten musste, welcher Block „unbedingt" ist.
```

## L1712-1713 · `gather_inline_urls(&dom.root, &mut sheet);`

```
// An inline `style="background-image:url(…)"` never passes through the
// sheet text, so its URL would have no entry to resolve against.
```

## L1718 · `fn gather_inline_urls(el: &Element, sheet: &mut Stylesheet) {`

```
/// Add every `url()` found in an inline `style` attribute to the sheet's table.
```

## L1732-1746 · `pub fn import_urls(css: &str) -> Vec<String> {`

```
/// The targets of a stylesheet's `@import` rules, in source order — the shell's
/// fetch list for the round after the linked sheets.
///
/// `parse` SKIPS `@import` (it cannot fetch), so without this a sheet that is
/// nothing but imports styles nothing at all. That is not a corner case: it is
/// how a hand-written site splits its CSS into modules, and
/// `sandbox.nopeek.ch` loads its entire design through fifteen of them behind
/// one `<link>` — the page rendered completely unstyled.
///
/// Only at the top level: an `@import` inside a block is invalid, and stepping
/// over blocks is also what keeps a `content: "@import x"` string out of the
/// list. The prelude between the URL and the `;` (a `layer()`, a `supports()`,
/// a media query) is stepped over rather than honoured — an import with a
/// media query is applied unconditionally for now, which is named in
/// CONFORMANCE rather than silently approximated.
```

## L1756 · `q @ (b'"' | b'\'') => {`

```
// A string can hold braces and semicolons; skip it whole.
```

## L1777 · `fn import_target(prelude: &str) -> Option<String> {`

```
/// The URL out of one `@import` prelude: `"x"`, `'x'`, `url(x)`, `url("x")`.
```

## L1785 · `_ => rest.split(')').next()?.trim(),`

```
// Unquoted `url(x)` — ends at the closing paren.
```

## L1792-1793 · `pub fn stylesheet_links(dom: &Dom) -> Vec<String> {`

```
/// Hrefs of every `<link rel="stylesheet">` in the document, for the shell to
/// fetch as sub-resources.
```

## L1839-1841 · `pub fn parse(css: &str) -> Stylesheet {`

```
/// Parse a stylesheet body into rules (css-syntax-3 subset). Descends INTO
/// `@media` blocks (their rules apply conditionally on the viewport); other
/// at-rules (`@keyframes`/`@font-face`/`@supports`/`@import`) are skipped.
```

## L1843-1845 · `let css = if css.contains("<![CDATA[") {`

```
// XHTML `<style>` bodies wrap the CSS in a `<![CDATA[ … ]]>` marker (the
// CSS2.1 reftest suite does this pervasively). It's raw text to us, so strip
// the markers before parsing — real CSS never contains them.
```

## L1858 · `if !layers.names.is_empty() {`

```
// Ids became positions only now that every layer is known.
```

## L1874-1877 · `let moves = r`

```
// A rule made only of properties we do not implement declares nothing
// at all — `apply_one` has no arm for them — so it can gain or lose
// without moving a pixel. MediaWiki's `:hover{cursor:pointer}` is
// exactly that, and it must not drag the page into a layout.
```

## L1908-1910 · `fn parse_into(`

```
/// Scan `css[start..end]` for rules, tagging each with the given `media`
/// context, and recurse into nested `@media` blocks. `order` is threaded so
/// document order is preserved across (and into) media blocks.
```

## L1933 · `let mut j = i + 1;`

```
// Read the at-keyword to tell `@media` (descend) from the rest (skip).
```

## L1952-1953 · `let mut k = j;`

```
// Descend into `@supports` when the condition holds; else skip
// the block (its rules never apply). Keeps any enclosing @media.
```

## L1968-1970 · `let mut k = j;`

```
// `@font-face { font-family: X; src: url(...) format(...) }`
// Nur EINGESAMMELT: die Datei holt der Wirt, die Engine hat
// kein Netz.
```

## L1978-1979 · `let mut k = j;`

```
// `@property --name { … initial-value: V … }` — der Wert, den
// die Eigenschaft hat, bevor irgendetwas sie setzt.
```

## L1999-2003 · `let mut k = j;`

```
// `@layer a, b;` declares order only; `@layer a { … }` and the
// anonymous `@layer { … }` also carry rules. Skipping the block
// — which is what an unknown at-rule gets — drops the whole
// sheet on a page that wraps its CSS in layers, which is what
// the current generation of CSS frameworks does.
```

## L2035-2038 · `while i < end && bytes[i] != b'{' && bytes[i] != b'}' {`

```
// `p \{ … \}` is a selector containing two escaped braces, not a rule:
// skipping the escape is what makes the block opener the NEXT real
// `{`, which swallows the rule after it — exactly as the spec says an
// unmatched selector should.
```

## L2045-2050 · `if bytes[i] == b'}' {`

```
// A `}` where a selector was expected is a stray close — usually the end
// of a nested style rule (`.a { .b { … } }`, which we flatten rather than
// support) or plain malformed CSS. css-syntax-3 error recovery: consume it
// and keep scanning. Aborting here (the old `break`) dropped the ENTIRE
// rest of a large sheet — Wikipedia's grid layout sits 174 KB past one
// such nested block, so a single `}` silently killed the whole page.
```

## L2056 · `i += 1; // '{'`

```
// '{'
```

## L2058-2061 · `let mut depth = 1i32;`

```
// Scan to the MATCHING `}`, tracking `{}` depth (and skipping string
// literals so a `{`/`}` inside `content:"…"` doesn't miscount). Without
// depth tracking a nested rule's inner `}` ended the parent early and
// leaked its real closing `}` to the top level, desyncing the parser.
```

## L2091 · `i += 1; // '}'`

```
// '}'
```

## L2115-2117 · `let lid = if layer.is_empty() { UNLAYERED } else { layers.id(layer) };`

```
// The layer id is stable; `parse` turns it into a rank once the
// whole sheet is read (a nested layer declared late moves its
// siblings, so a rank handed out here would go stale).
```

## L2126 · `pub(crate) fn matching_brace(bytes: &[u8], open: usize, end: usize) -> usize {`

```
/// Index of the `}` that closes the `{` at/after `open` (or `end` if unbalanced).
```

## L2131 · `if bytes[i] == b'\\' {`

```
// An escaped brace is a character in a name, not a block boundary.
```

## L2151-2154 · `pub fn supports_cond(cond: &str) -> bool {`

```
/// Evaluate an `@supports` condition. Handles `not`, top-level `and`/`or`, and
/// `(prop: value)` leaves. A colour-property leaf is supported iff the value
/// parses as a colour; other feature leaves are assumed supported (render-what-
/// the-author-intended bias — we implement most box/flex/grid properties).
```

## L2169 · `let inner = c.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(c).trim();`

```
// Unwrap one layer of grouping parens.
```

## L2186-2190 · `if crate::vars::has_var(val) { return true }`

```
// **Ein `var()` ist in JEDER Eigenschaft gueltig** (css-variables-1 §3):
// eingesetzt wird beim Gebrauch, also kann beim Parsen nichts daran
// scheitern. Der Farbleser darunter sagt zu `var(--test, red)` nein — und
// DDG fragt GENAU das, bevor es seine 1,1 MB Blaetter an
// `css-vars-ponyfill` uebergibt.
```

## L2193-2194 · `return crate::color::parse_color_val(val).is_some();`

```
// `transparent` IS a supported colour — ask the value parser, not the
// one that folds transparency into "no value".
```

## L2200 · `fn strip_ci_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {`

```
/// Case-insensitive prefix strip.
```

## L2202-2203 · `if s.get(..prefix.len()).is_some_and(|h| h.eq_ignore_ascii_case(prefix)) {`

```
// `get`, not a range index: a decoded escape can put a multi-byte
// character at the front, and `prefix.len()` may then fall inside it.
```

## L2211-2212 · `fn split_top<'a>(s: &'a str, sep: &str) -> Option<Vec<&'a str>> {`

```
/// Split `s` on `sep` at paren-depth 0. `None` if `sep` never occurs at top
/// level (so the caller can treat `s` as a single term).
```

## L2239-2242 · `fn parse_media_query(prelude: &str) -> Vec<MediaCond> {`

```
/// Parse a `@media` prelude (comma = OR) into conditions. Only `min-width`/
/// `max-width` (px) plus the `screen`/`all`/`only` media types are evaluated;
/// any other media type or feature marks that branch not-understood so it never
/// matches (we never mis-apply a rule we cannot evaluate).
```

## L2249 · `if let Some(rest) = ql.trim_start().strip_prefix("not ") {`

```
// `not` leads the query and covers all of it.
```

## L2263-2265 · `match feat {`

```
// A width feature whose value we can't parse must NOT leave
// the bound `None` (that matches every viewport) — mark the
// whole query not-understood so it fails closed instead.
```

## L2275-2277 · `"prefers-color-scheme" => match val {`

```
// `no-preference` was dropped from the spec and never
// matches; anything else is a value we don't know, so
// the query fails closed like any other.
```

## L2299-2303 · `pub fn media_matches(prelude: &str, m: Media) -> bool {`

```
/// Does an `@media` prelude hold at this viewport width? Shared with the
/// custom-property pre-pass, which must gate on exactly the same condition the
/// cascade uses — otherwise a variable from a non-matching block leaks.
/// Does a media query text apply to `m`? Used by `@media` preludes and by
/// `<source media=…>` in a `<picture>`.
```

## L2308 · `fn parse_px(v: &str) -> Option<f32> {`

```
/// A media-feature `<length>` — px only (Bootstrap/WP breakpoints are all px).
```

## L2311-2316 · `const INITIAL_FONT_PX: f32 = 16.0;`

```
// **`em` und `rem` in einer Medienabfrage sind 16 px** — beide beziehen
// sich auf den ANFANGSWERT von `font-size`, nicht auf das Wurzelelement
// (media-queries-4 §1.3). Das ist keine Feinheit: Tailwind v4 schreibt
// JEDEN Haltepunkt so (`@media (min-width:64rem)`), und ohne diese vier
// Zeilen ist auf einer Tailwind-Seite jede responsive Klasse tot — sie
// fiel einfach nie an.
```

## L2326-2330 · `fn parse_media_px(v: &str) -> Option<f32> {`

```
/// A media-feature length: a plain `<px>` or a `calc()` of `±<px>` terms.
/// MediaWiki (and others) express breakpoints as `calc(640px - 1px)`; without
/// `calc()` support the value fails to parse, `max_width` stays `None`, and the
/// `@media (max-width: …)` block then matches EVERY viewport — leaking mobile
/// rules (e.g. `.wikitable{float:none}`) onto the desktop layout.
```

## L2334-2335 · `let mut acc = 0.0f32;`

```
// Sum of whitespace-separated `±<px>` terms (CSS requires spaces around
// the `+`/`-` operators, so tokenising on whitespace is sufficient).
```

## L2366-2368 · `out.push(' ');`

```
// A comment is a token separator (css-syntax-3), NOT nothing —
// replace it with a space so `12px/* */solid` doesn't glue into
// `12pxsolid` (and `hsl(120/* */75%…)` tokenises correctly).
```

## L2378-2379 · `fn qualify(parent: &str, name: &str) -> String {`

```
/// Full name of a layer named `name` inside the layer `parent` (empty at the
/// top level). `@layer a { @layer b { … } }` is the layer `a.b`.
```

## L2418-2420 · `fn split_top_level_commas(text: &str) -> Vec<&str> {`

```
/// Split a comma-separated selector list on TOP-LEVEL commas only — commas
/// inside `[…]` or `:is(…)`/`:where(…)`/`:not(…)` parentheses do not separate.
/// A naive `split(',')` would tear `:is(div,table,ul)` apart.
```

## L2440-2443 · `fn parse_compound_list(arg: &str) -> Vec<Compound> {`

```
/// Parse an `:is()`/`:where()` argument (a forgiving selector list) into its
/// alternative compounds. Per css-selectors §4 forgiving parsing, an
/// unsupported alternative (e.g. `:hover`, or one with a combinator we don't
/// model) is dropped rather than invalidating the whole list.
```

## L2478-2480 · `let last = compounds.len() - 1;`

```
// `::before`/`::after` may only sit on the LAST compound (the subject) —
// `a:before b` has no meaning, so treat it as an unsupported selector
// rather than mis-applying it to `b`.
```

## L2487-2490 · `let mut anc_bloom: Bloom = [0u64; 4];`

```
// Walking right to left, every compound past the FIRST descendant/child
// combinator is guaranteed to sit on the ancestor chain. Sibling
// combinators stay at the subject's own level, so a compound reached
// only through them is not an ancestor and must not be required.
```

## L2510-2515 · `fn tokenize_selector(text: &str) -> Vec<String> {`

```
/// Split into compound tokens + `>` tokens on whitespace / `>` boundaries.
///
/// An escape is copied through whole, terminating whitespace included: the
/// space in `.c\06C ass` belongs to the escape, so it is NOT a descendant
/// combinator, and `\>` is a character in a name rather than a child
/// combinator. Decoding happens later, in `parse_compound`.
```

## L2519 · `let mut depth = 0i32; // inside [...] or :not(...) — don't split on combinators there`

```
// inside [...] or :not(...) — don't split on combinators there
```

## L2585 · `if b[0] == b'*' {`

```
// Leading type selector or universal `*`.
```

## L2623-2624 · `let mut d = 0i32;`

```
// Balanced close paren, so a nested `:is(:nth-child(2n))`
// doesn't terminate on the inner `)`.
```

## L2648-2649 · `("before", None) => {`

```
// `:before`/`::before` (legacy single-colon CSS2 syntax is
// still valid per css-pseudo-4 §2.1) — generated content.
```

## L2662-2663 · `_ if dbl => return None,`

```
// Any other `::pseudo-element` (first-line, placeholder,
// selection, …) is unsupported → drop rather than mis-apply.
```

## L2666-2672 · `let a = a.trim();`

```
// A state a static render never enters makes the
// negation trivially true: drop the clause and KEEP the
// rule. That is what carries the "visually hidden"
// idiom — `.skip-link:not(:focus){clip:rect(1px,1px,1px,1px)}`
// is how a page hides a link until it is tabbed to, and
// dropping the whole rule leaves the link on the page
// for everyone to read.
```

## L2678-2679 · `("is" | "matches", Some(a)) => c.is_groups.push(parse_compound_list(&a)),`

```
// `:is()`/`:where()` (and the legacy `:matches()` alias) —
// forgiving compound-alternative lists.
```

## L2681-2683 · `("has", Some(a)) => c.has.push(parse_has_list(&a)?),`

```
// `:has(<relative-selector-list>)`. An alternative we
// cannot express drops the whole selector — which is what
// an unknown pseudo-class did before, so nothing regresses.
```

## L2717 · `_ => return None, // :hover/:checked/… — unsupported → drop the selector`

```
// :hover/:checked/… — unsupported → drop the selector
```

## L2726-2728 · `fn never_matches(sel: &str) -> bool {`

```
/// A pseudo-class naming an interaction state this engine never enters, so a
/// selector demanding it can never match. Only meaningful inside `:not()`,
/// where it makes the negation always true.
```

## L2732-2733 · `":focus" | ":focus-visible" | ":focus-within" | ":active" | ":target"`

```
// `:hover` is NOT here any more — it is a real state now, so
// `:not(:hover)` has to be evaluated rather than assumed true.
```

## L2738 · `fn parse_attr(inner: &str) -> Option<AttrSel> {`

```
/// Parse the inside of an `[attr…]` selector (`attr`, `attr=v`, `attr~=v`, …).
```

## L2755 · `if let Some(s) = val.strip_suffix(" i").or_else(|| val.strip_suffix(" s")) {`

```
// Drop a trailing case-sensitivity flag (`[a="x" i]`).
```

## L2774 · `fn parse_nth(s: &str) -> Option<(i32, i32)> {`

```
/// Parse an `An+B` micro-syntax (`2n+1`, `odd`, `even`, `3`, `-n+3`).
```

## L2796-2798 · `fn parse_has_list(arg: &str) -> Option<Vec<HasArg>> {`

```
/// `:has()`'s argument: comma-separated relative selectors, each an optional
/// leading combinator plus ONE compound. Returns `None` — dropping the whole
/// selector — for anything else, e.g. `:has(> a > span)`.
```

## L2810-2811 · `if rest.is_empty() || rest.contains([' ', '>', '+', '~', '\t']) {`

```
// One compound only: any inner combinator (a space included) is out of
// scope. 20 of 243 real arguments; they keep failing as they did.
```

## L2820-2822 · `fn compound_spec(comp: &Compound) -> (u32, u32, u32) {`

```
/// One compound's specificity as `(id, class, type)` counts. Recurses through
/// `:not()` (its argument's specificity) and `:is()` (its MOST specific
/// argument's); `:where()` adds nothing (css-selectors §16/§4).
```

## L2825 · `let mut b = (comp.classes.len()`

```
// classes, `[attr]` and pseudo-classes all count at the class level.
```

## L2835 · `let mut c = comp.tag.is_some() as u32 + (comp.pseudo != PseudoElem::None) as u32;`

```
// tag + a pseudo-element each count like a type selector (css-cascade §5.8.3).
```

## L2850-2851 · `for group in &comp.has {`

```
// `:has()` contributes its most specific argument, as `:is()` does
// (Selectors 4 §17).
```

## L2862 · `fn specificity(compounds: &[Compound]) -> u32 {`

```
/// CSS specificity packed as (id<<20)|(class<<10)|type — enough headroom.
```

## L2874-2879 · `pub fn split_decls(body: &str) -> Vec<&str> {`

```
/// Split a declaration block on its top-level `;`.
///
/// NOT `str::split(';')`: a semicolon inside a string or a `url()` belongs to
/// the value. `url("data:image/svg+xml;utf8,<svg …>")` is the form icon
/// systems ship, and cutting it at `;utf8` leaves a declaration that still
/// parses — it just points at nothing.
```

## L2887-2889 · `b'\\' => i += 1,`

```
// Escaped anything — inside a string it protects a quote, outside
// it makes `;` or `:` part of a name. Either way the next byte is
// not a delimiter.
```

## L2910-2912 · `let db = decl.as_bytes();`

```
// The first UNESCAPED colon. `background\\: red` escapes it, so the
// whole thing is one property name with no value — an invalid
// declaration, which is what the test of that idiom checks.
```

## L2930-2940 · `let val = if v.contains('"') || v.contains('\'') || v.contains("url(") {`

```
// A property name and a keyword value are ident tokens, so an
// escape in either is part of the NAME: `bac\kground: g\reen` is
// `background: green` (css-syntax-3 §4.3.7).
//
// A value carrying a string or a `url()` is left alone, because
// there the backslash is doing the opposite job: it PROTECTS a
// quote from ending the token. Decoding it here would hand
// `url("data:…<svg xmlns=\"…\">")` on to a parser that then stops
// at the first inner quote — which is the spelling MediaWiki
// ships, and it decodes to a blank image rather than an error.
// Those tokens are unescaped where they are consumed instead.
```

## L2944-2948 · `match unescape_value(v) {`

```
// An escape that decodes to whitespace or a control character
// leaves an ident that CONTAINS that character — `red\9` is
// not the keyword `red` — so nothing can ever match it and the
// declaration is invalid. Decoding it and moving on would let
// the trim in `apply_one` turn it back into the keyword.
```

## L2964-2971 · `fn escape_at(s: &str, i: usize) -> (Option<char>, usize) {`

```
/// One `\…` escape starting at `i` (which must be the backslash): the code
/// point it stands for, and the index just past it — css-syntax-3 §4.3.7.
///
/// The hex form takes up to SIX digits and then swallows one following
/// whitespace, which is the only way to write `\6C` before a letter that is
/// itself a hex digit. Anything else escapes the next character literally,
/// and that is the form that matters on real pages: it is how a class name
/// gets to contain `:` or `/`.
```

## L2976 · `return (Some('\u{FFFD}'), j);`

```
// A trailing backslash is U+FFFD, not a parse error (§4.3.7).
```

## L2980-2981 · `if b[j] == b'\n' {`

```
// `\<newline>` is a line continuation inside a string: it stands for
// nothing at all.
```

## L3000 · `let ch = if cp == 0 { None } else { char::from_u32(cp) };`

```
// NUL, a surrogate, and anything past the last code point are U+FFFD.
```

## L3005-3006 · `fn css_unescape(s: &str) -> Cow<'_, str> {`

```
/// Resolve every escape in `s`. Borrows unchanged when there is none, which is
/// every declaration on almost every page.
```

## L3029-3030 · `fn unescape_value(s: &str) -> Option<String> {`

```
/// Unescape an unquoted value or a property name, or `None` when an escape
/// decoded to whitespace or a control character — see the call site.
```

## L3056-3061 · `fn ident_at(s: &str, mut i: usize) -> (String, usize) {`

```
/// Consume a CSS identifier at `i`, honouring escapes: the unescaped name and
/// the index after it.
///
/// The escape is what makes punctuation part of a NAME rather than a
/// delimiter — `.md\:flex` is one class, not a class followed by a pseudo —
/// which is how every utility-CSS framework spells a variant.
```

## L3087-3089 · `#[test]`

```
/// **Die Frage, die DuckDuckGo stellt, bevor es 1,1 MB Blaetter an einen
/// Polyfill uebergibt.** `var()` ist in JEDER Eigenschaft gueltig; wer
/// hier nein sagt, laedt `css-vars-ponyfill` und rechnet 120 s statt 1,5.
```

## L3095 · `assert!(super::supports_decl("color", "red"));`

```
// Kein `var()` daneben: der Farbleser entscheidet weiter.
```

## L3098 · `assert!(!super::supports_decl("color", "notvar(--x)"));`

```
// `notvar(` ist keine Ersetzung.
```

## L3103-3104 · `#[test]`

```
/// `@import` is how a hand-written site splits its CSS, and the whole of
/// `sandbox.nopeek.ch` hangs behind fifteen of them.
```

## L3129-3130 · `#[test]`

```
/// A brace or a semicolon inside a string must not end the scan, and an
/// `@import` inside a block is invalid and ignored.
```

## L3139-3142 · `fn info(tag: &str, id: Option<&str>, classes: &[&str]) -> ElemInfo<'static> {`

```
/// A standalone element to match against. `ElemInfo` borrows a live node
/// now, so the tree has to outlive the `ElemInfo` — leaked here so the
/// assertions can keep passing `info(...)` inline. A unit test process
/// exits before that matters.
```

## L3196 · `assert!(hit("section:has(img)", "a"));`

```
// Descendant — the default, and 178 of 243 real uses.
```

## L3199 · `assert!(!hit("section:has(> img)", "a"));`

```
// Child: <img> is a grandchild, so `> img` must NOT match.
```

## L3202 · `assert!(hit("section:has(+ section)", "a"));`

```
// Forward siblings need the parent, which the context now carries.
```

## L3207 · `assert!(hit("section:has(img, blockquote)", "a"));`

```
// A comma list is an OR.
```

## L3209-3210 · `assert!(!hit("section:has(> p > img)", "a"));`

```
// An argument we cannot express drops the selector rather than
// mis-applying it — no rule, so no match.
```

## L3235 · `assert!(hit("input:enabled", 0));`

```
// `:enabled` is the negation, not "no disabled selector at all".
```

## L3242-3244 · `let html = "<div><p>a</p><span>s</span><p>b</p><span>t</span><p>c</p></div>";`

```
// `:nth-last-of-type` and `:only-of-type` need siblings that come AFTER
// the subject, which is readable off the parent's children only because
// the matcher borrows live elements.
```

## L3269 · `assert!(hit("p:first-of-type", 0));`

```
// <p> elements are at positions 0, 2, 4 → of-type indices 1, 2, 3.
```

## L3276-3277 · `assert!(hit("span:nth-of-type(2)", 3));`

```
// `:first-child` is NOT the same question: the second <span> is the
// fourth child but only the second of its type.
```

## L3284-3286 · `let ss = parse("td:empty { color: red }");`

```
// `:empty` needs to see INSIDE the element — impossible while the
// matcher took a snapshot of tag/id/class, which is why it used to
// drop its whole selector.
```

## L3321 · `assert!(!ss.matched(&info("a", None, &[]), &[nav.clone()], &[], 0, Media::new(1000.0, false)).is_empty());`

```
// nav a: matches an <a> with <nav> anywhere above
```

## L3325 · `assert!(!ss.matched(&info("li", None, &[]), &[ul.clone()], &[], 0, Media::new(1000.0, false)).is_empty());`

```
// ul > li: <li> whose IMMEDIATE parent is <ul>
```

## L3336 · `assert_eq!(m.len(), 3);`

```
// ascending: type(p) < class(p.x) < id(#p)
```

## L3342-3344 · `#[test]`

```
/// A rule inside `@layer` used to be dropped with the block, which on a
/// sheet that puts EVERYTHING in layers — what the current generation of
/// CSS frameworks emits — is the whole page unstyled.
```

## L3351-3352 · `assert_eq!(m.last().unwrap().3[0].1, "b");`

```
// `@layer a, b` fixed the order, so `b` wins even though `a`'s rule is
// written last — layer beats document order.
```

## L3362-3363 · `m.sort_by_key(|(layer, spec, order, _, _, _, _)| (imp_rank(*layer), *spec, *order));`

```
// The `!important` pass runs the layer axis the other way round, so the
// same unlayered rule is the WEAKEST there (css-cascade-5 §6.4.4).
```

## L3368-3369 · `#[test]`

```
/// `@layer a; @layer b; @layer a.c;` has to sort `a.c` INSIDE `a` — after
/// `b` already exists — which is why ranks are assigned once at the end.
```

## L3386-3388 · `assert!(ss.matched(&info("a", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());`

```
// :hover + [attr] selectors dropped (unsupported); the @media block is
// now DESCENDED (not dropped), but `screen` alone always matches so its
// `p` rule is fine either way …
```

## L3391 · `assert!(!ss.matched(&info("h2", None, &[]), &[], &[], 0, Media::new(1000.0, false)).is_empty());`

```
// … and the plain "h1, h2" list still parsed.
```

## L3400 · `assert_eq!(m.len(), 2);`

```
// `:root` and `html` both match; `:root.night` does not (no class).
```

## L3402 · `let root_spec = m.iter().find(|(_, _, _, d, _, _, _)| d[0].1 == "a").unwrap().1;`

```
// `:root` has class-level specificity, `html` only type-level.
```

## L3406 · `assert!(ss.matched(&info("body", None, &[]), &[], &[], 0, Media::new(1000.0, false)).len() == 0);`

```
// Not the root element → no match.
```

## L3408 · `let night = info("html", None, &["night"]);`

```
// With the class present, the qualified rule matches too.
```

## L3425 · `let dom = dom::parse("<html><head><style>p{color:blue}</style></head><body><p>x</p></body></html>");`

```
// external (red) parsed first, inline <style> (blue) after → blue wins.
```

## L3445-3446 · `let ss = parse(`

```
// A `.col` base rule always applies; the `@media (min-width:768px)`
// override applies only at wide viewports.
```

## L3451 · `assert_eq!(ss.matched(&e, &[], &[], 0, Media::new(1000.0, false)).len(), 2, "media rule applies wide");`

```
// Wide (1000 ≥ 768): both rules present.
```

## L3453 · `assert_eq!(ss.matched(&e, &[], &[], 0, Media::new(500.0, false)).len(), 1, "media rule dropped narrow");`

```
// Narrow (500 < 768): only the base rule.
```

## L3455-3457 · `let ssn = parse("@media not screen and (max-width: 480px) { .w { width: auto } }");`

```
// An un-evaluable feature never matches (rule dropped both ways).
// `not` negates the whole query — how a mobile-first page states its
// desktop rules. Without it a wide window renders the phone layout.
```

## L3471-3480 · `fn src_is_readable(url: &str, tail: &str) -> bool {`

```
/// Kann beak diese `@font-face`-Quelle ueberhaupt lesen?
///
/// Zuerst zaehlt das ausdrueckliche `format(...)` — es ist die Zusage der
/// Seite und genauer als jede Endung. Fehlt es, entscheidet die Endung; und
/// sagt auch die nichts, wird die Quelle GENOMMEN: eine nackte Adresse ist
/// die Einladung, es zu versuchen.
///
/// Gelesen werden WOFF2 (`woff2.rs`), WOFF1 (`woff.rs`) und rohes sfnt.
/// `embedded-opentype` und `svg` sind Formate, die es nur fuer Browser gab,
/// die es nicht mehr gibt.
```

## L3489 · `let path = &url[..url.find(['?', '#']).unwrap_or(url.len())];`

```
// Endung — ohne Abfrage und Fragment (`x.woff?v=2`, `x.eot?#iefix`).
```

## L3495-3496 · `fn parse_font_face(body: &str) -> Option<FontFace> {`

```
/// Ein `@font-face`-Block. `None`, wenn Familie oder Quelle fehlen — ohne
/// beides ist er keine Schrift, sondern ein Kommentar.
```

## L3507-3518 · `let mut rest = v.as_str();`

```
// `url(a) format("woff2"), url(b)` — die Reihenfolge ist die
// Rangfolge der Seite, also bleibt sie erhalten. Was NICHT
// bleibt: Quellen in einem Format, das wir nicht lesen koennen.
//
// css-fonts-4 §4.3 sagt „die erste UNTERSTUETZTE", nicht „die
// erste". Der Unterschied ist keine Feinheit: das kugelsichere
// `@font-face` jeder Icon-Schrift der 2010er beginnt mit
// `url(x.eot) format('embedded-opentype')` fuer den IE, und wer
// davon die erste nimmt, holt genau die eine Datei, die er nicht
// lesen kann — und faellt fuer die ganze Familie auf die
// eingebaute Schrift zurueck. Genau so verschwand das
// Symbolgesicht von flexslider auf arcade.ch.
```

## L3525-3526 · `let tail = &rest[..rest.find(',').unwrap_or(rest.len())];`

```
// Der Rest bis zum naechsten Komma gehoert zu DIESER Quelle:
// dort steht ihr `format(...)`, wenn sie eins hat.
```

