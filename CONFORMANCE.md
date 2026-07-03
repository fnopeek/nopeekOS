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
| Tokenizer / parser | css-syntax-3 | 🟡 | `<style>` block parser (`css.rs`: rule list + selector list + declaration blocks, `/* */` comments, `@…{}`/`@…;` at-rules skipped) + inline `style="…"` + **external `<link rel=stylesheet>`** (shell fetches, cascades before `<style>`). Reader-mode toggle (UA-only) as a fallback. Not a full token stream |
| Selectors (type/class/id/desc/…) | selectors-4 | 🟡 | type / `.class` / `#id` / `*`, compounds (`div.a#b`), **descendant** (space) + **child** (`>`) combinators, comma lists. No pseudo-classes/`[attr]`/`+`~` siblings (those selectors are dropped, not mis-applied) |
| Cascade, specificity, inheritance | css-cascade | 🟡 | full order UA → author (**`(id,class,type)` specificity** + doc-order tie-break) → inline, plus inheritance. No `!important`/`@media`/external sheets |
| **UA default stylesheet** | HTML rendering §15 | ✅ | real UA sheet as data (`style.rs::ua_rule`): `display`, em-relative `font-size`, weight/italic/mono, `color` role, margins, list indent — no longer hardcoded in layout |
| `getComputedStyle` (CSSOM) | cssom-1 | ❌ | needs DOM + JS |
| `getComputedStyle` (CSSOM) | cssom-1 | ❌ | needs DOM + JS |

## CSS — layout

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Block flow (vertical stacking) | CSS2.1 §9 | ✅ | block formatting context (`layout.rs`): stack + adjacent-sibling **margin collapse**; anonymous inline runs flushed at block boundaries |
| Inline flow / line boxes | CSS2.1 §9.4.2 | ✅ | line boxes with **mixed-style runs** (size/colour/weight/italic) sharing a baseline; greedy wrap; `<a>`/`<b>`/`<i>`/`<code>` flow inline; `<br>` breaks. No bidi/UAX-14 yet |
| Box model (margin/border/padding) | css-box-3 | 🟡 | full block box model: `width`/`min-width`/`max-width`, `margin` (4-side + **`auto` centering**, §10.3.3 + §10.4 min/max redo), `padding` (4-side), `box-sizing`, vertical margin collapse → **centered `max-width` containers work**. No per-side borders/`border-radius` |
| Text wrapping / `white-space` | css-text-3 | 🟡 | `normal` collapse+wrap and `pre` (honor newlines, no wrap); no `nowrap`/`pre-wrap` distinction |
| Tables (`table`/`tr`/`td`/`th`) | css-tables-3 | 🟡 | `layout.rs::layout_table`: rows stack, cells in auto-width columns (content-preferred, clamped to fit, wrap allowed), `th` bold, `<caption>`, row separators. No colspan/rowspan/border-collapse/`table-layout:fixed` |
| Flexbox | css-flexbox-1 | 🟡 | `layout.rs::layout_flex` (single line): row/column direction, `flex-grow`/`-shrink`/`-basis` + `flex` shorthand, `gap`, `justify-content` (all 6), `align-items`/`align-self`, `order`. No wrap/reverse/`margin:auto`/baseline |
| Grid | css-grid-2 | 🟡 | `layout.rs::layout_grid`: `grid-template-columns` (px/%/`fr`/`auto`/`repeat()`), row-major auto-placement, `grid-column: span N` / `A / B`, `gap`; auto row heights. No explicit line placement / `grid-template-rows`/`-areas` / dense flow / item alignment |
| Positioning (rel/abs/fixed/sticky) | css-position-3 | 🟡 | `relative` (in-flow paint offset by top/left/right/bottom) + `absolute`/`fixed` (out of flow, positioned vs nearest `position!=static` ancestor's box / page; `top`/`left`/`right`). No `bottom`/`z-index`/true fixed-or-sticky scroll behaviour |
| Values & units (px/em/%/rem/…) | css-values-4 | 🟡 | `px`/`em`/`rem`/`%` lengths, `auto`, `fr`, hex/named colours — parsed across box model / flex / grid. No `calc()`/`vw`/`vh`/`ch` |

## CSS — paint

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Color / text color | css-color-4 | 🟡 | `color:` parsed (`#rgb`/`#rrggbb` + common named colours) via inline styles; theme roles otherwise. No `rgb()`/`hsl()` yet |
| Backgrounds / borders | css-backgrounds-3 | 🟡 | `background`/`background-color` fill (behind content) + uniform `border` (4 edges) on block boxes; hex/named colours. No gradients/images/`border-radius`/per-side |
| Font size / weight / family | css-fonts-4 | 🟡 | em-relative `font-size` cascade; **weight** (synthetic bold smear) + **italic** (faux shear) — single Inter face, no real bold/italic/mono font loaded yet |
| Glyph rasterisation + AA | — | ✅ | fontdue + coverage blend (infrastructure) |
| Transforms / opacity / filters | css-transforms/… | ❌ | §9 frontier |

## Images

| Feature | Status | Notes |
|---------|--------|-------|
| PNG decode | ✅ | `image.rs` (8-bit RGB/RGBA, non-interlaced, miniz_oxide inflate) — wired into `<img>` |
| JPEG decode | ❌ | common web case → next; JPEG `<img>` currently shows a placeholder |
| `<img>` layout + paint | 🟡 | block-level image box (size from `width`/`height`/intrinsic, scaled to fit, aspect kept), decoded PNG blit (nearest-neighbour + alpha) or a labelled placeholder. Shell fetches the bytes (≤16/page, 24 MB decode budget). No inline images / `srcset` / `object-fit` |

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
