# CONFORMANCE.md — beak standards tracker ("Strichliste")

Living coverage tracker for the native browser engine (`beak-engine`). The
web is a set of precisely-specified, separately-testable standards; this file
records **what we implement and how well we conform** — measured against the
official test suites, not self-graded.

> Principle (same as [[feedback-test-on-hw]] / messen-nicht-raten): the
> standard is the single source of truth, and conformance is an *oracle-graded
> number*, not an opinion. Build standard-first, track the number, never drift.

## The oracles

| Area | Canonical test suite | How we run it |
|------|----------------------|---------------|
| **CSS** | **Web Platform Tests** (WPT) CSS reftests + the CSS2.1 suite | render test → render reference → **pixel-compare** (our host renderer does this NOW, no browser needed) |
| **HTML parsing** | WPT `html/` + html5lib-tests (tokenizer + tree-construction JSON) | feed input → compare our DOM to the expected tree (data-driven, host) |
| **DOM / CSSOM** | WPT `dom/`, `cssom/` (testharness.js) | needs our JS engine to drive → arrives with Stage 1+ |
| **JavaScript** | **test262** (ECMAScript) | pure `.js` + asserts, no browser → run once the JS engine exists |
| Milestones | Acid1 / Acid2 / Acid3 | famous whole-page reftests — nice north-star checkpoints |

Reftests + html5lib-tests + test262 are all **data files we run natively** on
the dev box (§10). testharness.js-based tests need the JS engine first.

### Current number (measured 2026-07-31, beak 0.1.65)

```
3683 pass / 1855 fail / 248 inconclusive   (of 5786 vendored reftests)
= 66.5 % of the conclusive 5538
```

The 0.1.65 round moved six tests out of "inconclusive" — implementing `:root`
made those references paint at all, so they are now honestly counted (five of
them as failures). Against 0.1.64 there was **not one PASS → FAIL**.

Per suite (pass / total of that suite, inconclusive included in the total):

| Suite | Pass | Total | % |
|---|---|---|---|
| css-fonts | 40 | 43 | 93.0 |
| css-color | 221 | 282 | 78.4 |
| css-position | 28 | 36 | 77.8 |
| **CSS2** (2.1 suite) | 2580 | 3351 | 77.0 |
| html-forms | 15 | 21 | 71.4 |
| css-text | 241 | 381 | 63.3 |
| css-display | 44 | 88 | 50.0 |
| css-cascade | 13 | 32 | 40.6 |
| css-backgrounds | 59 | 144 | 41.0 |
| css-values | 33 | 93 | 35.5 |
| css-grid | 256 | 749 | 34.2 |
| css-sizing | 37 | 109 | 33.9 |
| **css-flexbox** | 110 | 428 | 25.7 |
| css-align | 6 | 29 | 20.7 |

Run it: `cargo test --release --manifest-path tools/wasm/beak-engine/Cargo.toml
--test wpt -- --nocapture` (~5 min). Redirect to a log and wait on it rather
than watching a raw pipe — a piped run has been SIGTERM'd mid-way before.
`WPT_FILTER=<substr>` narrows a run to iterate on one feature; `WPT_DUMP=<dir>`
writes `<test>-test.bmp` + `<test>-ref.bmp` so a failing reftest can be *looked
at* instead of guessed at.

"Inconclusive" means the reference itself rendered blank, so the comparison
says nothing about us either way. Those are excluded from the pass rate
rather than counted as wins.

**Reading the number honestly:** it can go *down* for a good reason. Fixing a
half-masked bug flipped nine tests green→red once — they had only been green
because two bugs cancelled out (we weren't painting `html{background:red}`,
so there was no red to fail on). Never revert on the number alone; look at
each case.

---

# Gap map (measured 2026-07-31, beak 0.1.65)

Two independent axes, because they disagree and each catches what the other
misses:

- **Real-web axis** — `tests/gap.rs`, DOM-weighted: cascade the *real*
  stylesheets of a *real* page over its *real* DOM and count, per property, on
  how many elements it wins. Declaration counts in a stylesheet lie; element
  counts don't.
- **Oracle axis** — WPT: how many reftests a missing feature costs us.

Reproduce (both ~5 min, page + sheets must be re-fetched, they aren't vendored):

```
curl --http2 -A "beak/0.1.64 (nopeekOS)" -o wiki.html https://de.wikipedia.org/wiki/Stansstad
# then each <link rel=stylesheet href=/w/load.php…>, concatenated into wiki.css
GAPHTML=wiki.html GAPCSS=wiki.css cargo test --release --test gap -- --nocapture
```

`gap.rs`'s `IMPLEMENTED` list is kept in sync by hand with the `match prop`
arms in `style.rs::apply_one`; extract the truth with
`awk '/fn apply_one/,/^}/' src/style.rs | grep -oE '^\s{8}"[a-z-]+"'`.

## Real-web axis — de.wikipedia.org/wiki/Stansstad

1521 elements, 112 distinct properties applied, **4707 implemented / 1197
unimplemented** property applications (was 4340 / 1564 at 0.1.64 — the five
bucket-A features below closed 367 of them). The unimplemented ones, by
elements affected, split by whether ignoring them is actually wrong:

| Property | Elems | Dominant value | Ignoring it is… |
|---|---:|---|---|
| `border-radius` | 307 | `2px` | **wrong** — every pill/button/search box is a hard rectangle |
| `word-wrap` / `word-break` / `overflow-wrap` | 165 / 52 / 3 | `break-word` | **wrong** — long tokens overflow their box instead of breaking |
| `vertical-align` (inline only) | 74 | 46× `middle`, 10× `text-bottom` | **partly wrong** — cells align since 0.1.65, inline boxes still sit on the baseline |
| `overflow` | 58 | 47× `hidden` | **wrong** — we paint what should be clipped |
| `user-select` (+`-moz-`/`-webkit-`) | 112 | `none` | harmless — no selection yet anyway |
| `transition-property`/`-duration` | 68 | — | harmless — ignoring = jump straight to the final state |
| `cursor` | 30 | 22× `pointer` | cosmetic — the *compositor* owns the cursor, not us |
| `mask-*` (+`-webkit-`) | 28 each | `url(…)` + `center`/`no-repeat` | **wrong** — this is Vector's whole icon system |
| `text-overflow` | 24 | `ellipsis` | **wrong** — truncated labels run on |
| `unicode-bidi` | 21 | `isolate` | **wrong**, but blocked on bidi generally |
| `background-image` / `-position` / `-repeat` / `-size` | 19 each | `url(…)` | **wrong** — 16 real icons unpainted |
| `scroll-margin-top`, `overflow-anchor`, `touch-action`, `-*-appearance`, `list-style-image:none`, `font-variant:normal`, `text-indent:0` | 1–15 | — | harmless |
| `box-shadow` | 10 | `0 2px 6px -1px rgba(…)` | cosmetic |
| `transform` | 2 | `translateY(-50%)` | **wrong** where used for centering |

**Caveat:** one page, one skin. `border-radius` at 307 is inflated by Vector's
`2px`-on-everything; `mask-*` at 28 is *the* icon mechanism and matters more
than its count suggests. Re-run on a second site (a shop, a docs page, GitHub)
before treating this ranking as general.

## Oracle axis — where the WPT failures sit

Failing-test families, largest blocks first (`FAIL` only, inconclusive
excluded):

| Suite | Fails | Biggest families |
|---|---:|---|
| CSS2 | 673 | `margin-collapse` 28 · `abspos-containing-block` 25 · `margin-bottom-applies-to` 24 · `floats-wrap` 22 · `border-*-width` 44 · `block-in-inline` 13 · `caption-side` 10 · `vertical-align-*` 6 |
| css-grid | 467 | `grid-lanes` 84 (masonry, experimental — **skip**) · `row/column-auto` 64 · `positioned-grid`+`grid-abspos` 48 · `row-fill`/`column-fill` 37 · `column-align`/`row-justify` 35 |
| css-flexbox | 301 | `flex-0/1/N-…` 86 (the flex-basis/grow/shrink core) · `flexbox-writing-mode` 14 · `align-items`/`align-content` 15 · `flexbox-break` 8 |
| css-text | 114 | `white-space` 17 · `ws-break-spaces` 12 · `word-break` 12 · `text-wrap` 11 · `hyphens-*` 25 |
| css-color | 61 | mostly `color-mix`/relative colors |
| css-backgrounds | 60 | `background-clip` 15 · `box-shadow` 11 · `clip-text` 10 · `border-image` 3 |
| css-values | 43 | `ch-unit` 8 · `calc-offsets` 4 · `attr()` 6 · `vh-*` 6 |
| css-display | 43 | `run-in` 35 (**skip** — nothing on the web uses it) · `display-contents` 5 |
| css-sizing | 41 | `contain-intrinsic-size` 15 · `box-sizing` 6 |
| css-cascade | 18 | `@layer`, `revert`, `all` |
| css-align | 14 | `self-align`, `place-content` |

## Known holes, by kind

**Properties not parsed at all** (`style.rs::apply_one` has 115 arms; these
aren't among them): `background-image`/`-repeat`/`-position`/`-size`,
`border-radius`, `box-shadow`, `text-decoration*`, `text-indent`,
`text-shadow`, `letter-spacing`, `word-spacing`, `word-break`/`overflow-wrap`/
`hyphens`, `overflow*`, `cursor`, `outline*`, `transform`, `transition`,
`animation`, `aspect-ratio`, `object-fit`, `filter`, `mask-*`, `quotes`,
`appearance`, `resize`, `writing-mode`.

**Selectors that drop the whole rule** (`css.rs` returns `None` → the rule is
discarded rather than mis-applied): every pseudo-class outside
`:not()`/`:is()`/`:where()`/`:first-child`/`:last-child`/`:only-child`/
`:nth-child()`/`:nth-last-child()`. That includes **`:root`**, `:checked`,
`:hover`, `:focus`, `:link`/`:visited`, `:first-of-type`/`:nth-of-type()`,
`:empty`, `:disabled`, `:has()`. Pseudo-*elements* other than
`::before`/`::after` also drop the rule.

**Value syntax not understood:** `attr()`, `env()`,
`linear-gradient()`/`radial-gradient()`/`conic-gradient()`, `image-set()`,
`url()` in any property except as a bare marker.

**At-rules skipped:** `@font-face`, `@keyframes`, `@import`, `@layer`,
`@container`, `@page`. (`@media` width features + `prefers-color-scheme` and
`@supports` conditions *are* evaluated.)

**Layout:** `rowspan` (colspan works), sticky positioning (parsed, behaves as
relative), bidi reordering, UAX-14 line breaking, `display: contents`,
multi-column, writing modes.

## Priority buckets

Ranked by (real-web damage × oracle reserve) ÷ effort. Not a schedule — a
menu to pick from.

**A — cheap, self-contained, low regression risk — ✅ DONE in 0.1.65**

1. ✅ `min()` / `max()` / `clamp()` in `values.rs`, beside the existing
   `calc()` parser. Previously the whole declaration was dropped.
2. ✅ `:root` — was dropping the entire rule. Custom properties always
   survived (`vars.rs` collects those separately), real declarations did not.
   Matches `<html>`, keeps pseudo-class specificity, buckets into the `html`
   tag index. **This is what made six WPT references paint at all.**
3. ✅ `text-decoration` / `-line` (underline / line-through / overline), plus
   the UA rules: `:any-link` underlines (href-gated, so `<a name>` does not),
   `<u>`/`<ins>` underline, `<s>`/`<del>`/`<strike>` strike through.
4. ✅ `caption-side: bottom`. WPT-neutral — the `caption-side-applies-to-*`
   tests check that it does **nothing** on non-captions — but correct, and
   visible on any table with a bottom caption.
5. 🟡 `vertical-align`: parsed as a full `VAlign` enum and honoured **on table
   cells** (`top`/`middle`/`bottom`, sliding the cell's emitted ops by the row
   slack). **Inline-level boxes still ignore it** — doing that right needs the
   parent's font metrics (`middle` is "half the parent's x-height") and
   line-box feedback for `top`/`bottom`, which touches the hottest code path
   in the engine. Own step.
   ⚠️ **Lesson from this one:** parsing a property that was previously ignored
   silently applies it in contexts where it must not. `vertical-align` on an
   absolutely positioned or block-level box has no effect (CSS2.1 §10.8.1);
   without that guard `vertical-align-sub-001` (two abspos spans that must
   coincide) went 2.01 % → 3.46 %. **When adding a property, add its
   applies-to rule in the same commit.**

**B — real visual damage, medium effort**

6. `border-radius` — 307 elements here, and it gates `background-clip` (15
   WPT) + `box-shadow-radius` tests. Needs rounded-rect fill and a clip path
   in `raster.rs`; the rest of the engine only needs to carry 4 radii.
7. `overflow: hidden` clipping — 58 elements, 22 WPT near-misses; the
   `Layout` already carries rects, so this is a raster-side clip stack.
8. `word-break` / `overflow-wrap: break-word` — 220 elements combined, 12 WPT.
   A break opportunity inside a word when the line can't otherwise fit.
9. `background-image: url()` + `linear-gradient()` + `mask-image` — 19 + 28
   elements, and the entire Vector icon set. Needs sub-resource fetch driven
   *from CSS* (the shell only scans `<img>` and `<link>` today), which is the
   real cost, not the painting.

**C — the big structural blocks (oracle-heavy)**

10. **Flexbox core** — 25.7 % is our worst suite; 86 of the fails are the
    `flex-0/1/N` basis/grow/shrink family, i.e. one algorithm (css-flexbox-1
    §9.7 resolving flexible lengths) rather than 86 separate bugs. Highest
    oracle yield per unit of work anywhere in the list.
11. **CSS2 long tail** — `margin-collapse` 28, `abspos-containing-block` 25,
    `floats-wrap` 22, `border-*-width` 44 (`thin`/`medium`/`thick` keywords
    are a suspiciously cheap 44).
12. **Grid** — 467 fails but 84 are experimental `grid-lanes`; the real
    reserve is auto-placement (`row/column-auto`, 64) and abspos-in-grid (48).
13. Bidi reordering — costs ~13 tests *actively* and blocks `unicode-bidi`.

**Explicitly not worth doing:** `run-in` (35 WPT, dead on the real web),
`grid-lanes`/masonry (84, experimental), `cursor` (the compositor owns the
cursor), `user-select`/`touch-action`/`overflow-anchor`/`scroll-margin`
(harmless to ignore until we have selection and smooth scrolling).

---

## Legend

`❌` none · `🟡` partial · `✅` solid · `%` = share of the mapped suite passing
(filled in once we wire that suite into the host harness).

---

## HTML

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Tokenizer (tags, text, attrs) | WHATWG HTML §13.2 | 🟡 | tolerant subset; not the full state machine |
| Comments `<!-- -->` | §13.2 | ✅ | incl. inner markup / multiline |
| Character references (entities) | §13.2 | 🟡 | named common set + numeric `&#…;` / `&#x…;` |
| Tree construction (real DOM) | §13.2.6 | 🟡 | owned node tree (`dom.rs`): elements+attrs+text, tolerant recovery — implied `</p>`/`</li>`/`</dt>`/`</td>`, unmatched end tags ignored, raw-text `<script>`/`<style>`. Not the full insertion-mode state machine |
| `<script>`/`<style>` raw-text | §13.2 | 🟡 | content skipped (not yet executed/applied) |

## CSS — parsing & cascade

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Tokenizer / parser | css-syntax-3 | 🟡 | `<style>` block parser (`css.rs`: rule list + selector list + declaration blocks, `/* */` comments) + inline `style="…"` + **external `<link rel=stylesheet>`** (shell fetches, cascades before `<style>`). `@media` (width features + `prefers-color-scheme`) and `@supports` are evaluated and descended into; `@font-face`/`@keyframes`/`@import`/`@layer`/`@container`/`@page` are skipped. Reader-mode toggle (UA-only) as a fallback. Not a full token stream |
| Selectors (type/class/id/desc/…) | selectors-4 | 🟡 | type / `.class` / `#id` / `*`, compounds (`div.a#b`), `[attr]`, all four combinators (descendant / `>` / `+` / `~`), comma lists, `:not()` / `:is()` / `:where()` / `:matches()` with correct specificity, structural `:first-child`/`:last-child`/`:only-child`/`:nth-child()`/`:nth-last-child()`, **`:root`**, and the `::before`/`::after` pseudo-elements. **Every other pseudo-class drops the whole rule** (not mis-applied) — incl. `:checked`, `:hover`, `:focus`, `:*-of-type`, `:empty`, `:has()`. Selectors are bucketed by the most selective simple selector of their rightmost compound (`Index`), so an element only tests its own candidates |
| Cascade, specificity, inheritance | css-cascade | 🟡 | full order UA → author (**`(id,class,type)` specificity** + doc-order tie-break) → inline, plus inheritance, plus a second **`!important`** pass on top (css-cascade-4 §6.3). External sheets and `@media` both cascade |
| `!important` | css-cascade-4 §6.3 | ✅ | two-pass author cascade: an `!important` decl wins its property regardless of specificity, order or inline |
| Custom properties / `var()` | css-variables-1 | 🟡 | `--x` collection + `var()` substitution with fallbacks; `@media`-aware *and* root-class-aware (`vars.rs::root_selector_excluded`), so a `html.theme-night{…}` or `html.…-clientpref-2{…}` definition only applies when the document actually carries that class — both leaks cost us a whole-page regression once |
| **UA default stylesheet** | HTML rendering §15 | ✅ | real UA sheet as data (`style.rs::ua_rule`): `display`, em-relative `font-size`, weight/italic/mono, `color` role, margins, list indent — no longer hardcoded in layout |
| `getComputedStyle` (CSSOM) | cssom-1 | ❌ | needs DOM + JS |

## CSS — layout

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Block flow (vertical stacking) | CSS2.1 §9 | ✅ | block formatting context (`layout.rs`): stack + adjacent-sibling **margin collapse**; anonymous inline runs flushed at block boundaries |
| Inline flow / line boxes | CSS2.1 §9.4.2 | ✅ | line boxes with **mixed-style runs** (size/colour/weight/italic) sharing a baseline; greedy wrap; `<a>`/`<b>`/`<i>`/`<code>` flow inline; `<br>` breaks. No bidi/UAX-14 yet |
| Box model (margin/border/padding) | css-box-3 | 🟡 | full block box model: `width`/`min-width`/`max-width`, `margin` (4-side + **`auto` centering**, §10.3.3 + §10.4 min/max redo), `padding` (4-side), **per-side borders** (width/style/colour), `box-sizing`, logical `margin-inline`/`-block` + `padding-inline`/`-block`, vertical margin collapse → **centered `max-width` containers work**. No `border-radius` |
| Text wrapping / `white-space` | css-text-3 | 🟡 | `normal` collapse+wrap and `pre` (each source line its own line box, trailing spaces hang, §8); `<br>` forces a break even under max-content. `pre-wrap`/`pre-line`/`nowrap` are **not** distinguished from `pre`/`normal`. No `word-break`/`overflow-wrap`/`hyphens`, no UAX-14 line breaking, no bidi reordering |
| Tables (`table`/`tr`/`td`/`th`) | css-tables-3 | 🟡 | `layout.rs`: §17.2.1 anonymous-box fixup, auto **and** `table-layout: fixed` column algorithms, `colspan` (spanning cells distribute only the shortfall), **both border models** — `border-collapse` (winner-takes-the-edge, half the collapsed line per cell, incl. in column widths) and separated with `border-spacing` + `empty-cells` — the `border`/`cellpadding`/`cellspacing` presentation attributes, table border box + `auto` horizontal centring, `<caption>`. **`caption-side`** and per-cell **`vertical-align`** (`top`/`middle`/`bottom`; `baseline` degrades to `top` — no cross-cell baseline alignment). No `rowspan`, `display:inline-table` is block-level |
| Flexbox | css-flexbox-1 | 🟡 | `layout.rs::layout_flex`: row/column, **multi-line wrap**, `flex-grow`/`-shrink`/`-basis` + `flex` shorthand, `gap`, `justify-content` (all 6), `align-items`/`align-self`, `order`, per-item `margin:auto`, automatic content minimum size. No reverse directions, no baseline alignment, no `align-content`, no writing modes. **Weakest suite at 25.7 %** — see the gap map |
| Grid | css-grid-2 | 🟡 | `layout.rs::layout_grid`: `grid-template-columns`/`-rows` (px/%/`fr`/`auto`/`repeat()`/`minmax()`≈), `grid-template-areas`, `grid-auto-rows`, row-major auto-placement, `grid-column`/`-row` (`span N`, `A / B`), `grid-area`, `gap`, `justify-items`/`justify-self`/`place-*`. No dense flow, no abspos-in-grid, no orthogonal/RTL flows |
| Positioning (rel/abs/fixed/sticky) | css-position-3 | 🟡 | `relative` (in-flow paint offset) + `absolute`/`fixed` (out of flow, positioned vs nearest `position!=static` ancestor's box / page). `top`/`left`/`right`/**`bottom`** (§10.6.4); `top`/`bottom` percentages resolve against the containing block's **height** (§9.3.2). **`z-index`** via recorded display-list ranges stable-sorted at the end of layout (§9.9) — the ranges must stay disjoint, a leaking throwaway measurement corrupts the whole list. `sticky` parses but lays out without offsets; `fixed` scrolls with the page |
| Values & units (px/em/%/rem/…) | css-values-4 | 🟡 | `values.rs`: `px`/`em`/`rem`/`ex`/`ch`/`%`/`vw`/`vh`/`vmin`/`vmax`/`pt`/`pc`/`cm`/`mm`/`in`/`Q`, `auto`, `fr`, plus **`calc()`** with `+ - * /` and nesting (one code path for a bare `16px`, a `50%` and a full `calc(100% - 3rem)`). plus **`min()`/`max()`** (variadic) and **`clamp()`**, nestable in any combination with `calc()`. `rem` resolves against the root element, not the parent. No `attr()`/`env()` — those drop the declaration |

## CSS — paint

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Color / text color | css-color-4 | ✅ | `color.rs`: `#rgb`/`#rrggbb`/`#rgba`, the named-colour table, `rgb()`/`hsl()`/`hwb()`/`lab()`/`lch()`/`oklab()`/`oklch()`/`color()`, alpha and modern slash syntax |
| Backgrounds / borders | css-backgrounds-3 | 🟡 | `background`/`background-color` fill (inserted behind content at a recorded op index) + **per-side** `border` (width/style/colour, incl. the shorthands and `border-collapse` edge resolution). No `background-image`/gradients, no `background-clip`/`-repeat`/`-position`/`-size`, no `border-radius`, no `box-shadow`, no `border-image`. 40.3 % of the suite |
| Font size / weight / family | css-fonts-4 | 🟡 | em-relative `font-size` cascade with correct compounding; **six real subsetted faces** (Inter regular/bold/italic/bold-italic + mono/mono-bold, `fonts.rs`) — synthetic bold/italic retired. No `@font-face` / webfonts / family fallback lists |
| Text decoration | css-text-decor-3 | 🟡 | `text-decoration`/`-line`: underline / line-through / overline as rects in the run's colour, at metric-free approximations of the font's decoration positions. UA rules: `:any-link` (href-gated), `<u>`/`<ins>`, `<s>`/`<del>`/`<strike>`. Inherited rather than propagated (§1.2) — same pixels for every construct we have. No `-color`/`-style`/`-thickness`, no `text-underline-offset` |
| Glyph rasterisation + AA | — | ✅ | fontdue + coverage blend, warm glyph cache; `fill` builds one row and `copy_within`s it (a per-pixel loop cost ~10× under wasmi) |
| Transforms / filters / shadows | css-transforms/… | ❌ | §9 frontier — `transform`, `filter`, `text-shadow`, `box-shadow` all unparsed |
| `opacity` / `visibility` | css-color-4 / css-display | 🟡 | `visibility: hidden/collapse` inherits and suppresses bg/border/text/image/marker (and removes the element as a click target); `opacity: 0` groups its subtree (a descendant can't take it back) but **stays hit-testable** — that is the point of the checkbox-hack overlay. No fractional opacity compositing |

## Images

| Feature | Status | Notes |
|---------|--------|-------|
| PNG decode | ✅ | `image.rs` (8-bit RGB/RGBA, non-interlaced, miniz_oxide inflate) — wired into `<img>` |
| JPEG decode | ✅ | baseline + progressive via the no_std `zune-jpeg` decoder |
| `<img>` layout + paint | 🟡 | **atomic inline** image box in the line flow (image-in-a-link is clickable) as well as block-level; size from `width`/`height` **attribute or CSS**, else intrinsic, scaled to fit with aspect kept; decoded PNG/JPEG/SVG blit (nearest-neighbour + alpha) or a labelled placeholder. Shell fetches the bytes streaming (fetch→decode→drop). No `srcset`/`object-fit`. **An image whose box has to be guessed forces a full relayout when it arrives** — CSS sizes on `<img>` are worth honouring for that alone |

## SVG

Not yet oracle-graded — the WPT `svg/` reftests are not vendored, so these
rows are self-assessed and marked as such (see "How this file is maintained").

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Document + viewport | SVG 1.1 §7 | 🟡 | `svg.rs`: `width`/`height`/`viewBox` with uniform scaling; rendered as an inline replaced box, and via `<img src=*.svg>` |
| Shapes | SVG 1.1 §9 | 🟡 | `rect`/`circle`/`ellipse`/`line`/`polyline`/`polygon` |
| Paths | SVG 1.1 §8 | 🟡 | `M L H V C S Q T Z` (absolute + relative), flattened to polygons; no arcs (`A`) |
| Fill + stroke | SVG 1.1 §11 | 🟡 | solid fills, even-odd/nonzero winding, stroke width + colour. No gradients, patterns, dash arrays, line joins/caps |
| `<defs>` / `<use>` / groups | SVG 1.1 §5 | ❌ | not resolved |
| Transforms | SVG 1.1 §7.6 | ❌ | `transform=` ignored |
| Text | SVG 1.1 §10 | ❌ | `<text>` not rendered |

## Forms

Measured: **16 / 21** vendored WPT reftests (`tests/wpt/html-forms`, from
`html/rendering/widgets`). Only the *rendering* of controls is measurable this
way — WPT tests submission behaviour through `testharness.js`, which needs JS,
so `submit` is covered by unit tests against the real markup of live search
boxes until Stage 1.

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Control rendering (`input`/`button`/`select`/`textarea`) | HTML §4.10 + rendering §15.5 | 🟡 | atomic inline boxes wherever they land (in-flow, inline, flex/grid items, table cells); value/placeholder/label text, focus ring + caret, checkbox/radio mark, select chevron, multi-line textarea; author `background-color`/`width`/`height` honoured. No `appearance`, no native date/time/range widgets |
| Button content layout | HTML §button-layout | 🟡 | label text centred in the box; the button's *children* are not laid out (an icon + markup inside a `<button>` collapses to its text) → the 3 `centering-00x` reftests fail on box size |
| Text editing in a field | HTML §4.10.5 | 🟡 | insert/Backspace/Delete/arrows/Home/End + caret, per-control state; no selection, no clipboard, no IME, ASCII only |
| Form submission (GET) | HTML §4.10.21/22 | ✅ | successful-control rules (named + enabled, only the activated button, checked boxes/radios), `application/x-www-form-urlencoded`, implicit submission via the default button, action query replaced |
| Form submission (POST) | HTML §4.10.21 | ❌ | needs a request body — `npk_http_request` is GET-only |
| Validation / `required` / `pattern` | HTML §4.10.20 | ❌ | reports through JS APIs → Stage 2 |
| `<datalist>` / `<fieldset>` / `<label>` binding | HTML §4.10 | ❌ | labels render as plain text; clicking one does not focus its control |

## DOM + JavaScript

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Live DOM tree + mutation | DOM | ❌ | Stage 2 |
| Event loop / timers | HTML | ❌ | Stage 2 |
| `fetch` / XHR | fetch | 🟡 | host `npk_http_request` exists; not exposed to JS |
| ECMAScript language (test262) | ES2020 | ❌ | Stage 1 — own engine, grow as a subset ([[project-native-browser-beak]] §7-D1) |

---

## How this file is maintained

1. **Standard-first:** implement a feature per its spec, not by eyeballing a
   page. When in doubt, the spec + a reftest decide — never a guess.
2. **Every increment updates a row here** (status and, once wired, the `%`).
3. **Wire the suites into the host harness** as we go: CSS reftests +
   html5lib-tests first (render/parse + compare, no JS needed), test262 when
   the JS engine lands. Then the `%` columns become real, measured numbers.
4. No silent drift: a non-standard shortcut gets a row + a note saying so.

Related: `BROWSER.md` (architecture, §8 test262, §10 host-testability).
