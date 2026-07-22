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

### Current number (measured 2026-07-22, beak 0.1.44)

```
3626 pass / 1898 fail / 262 inconclusive   (of 5786 vendored reftests)
```

Run it: `cargo test --release --manifest-path tools/wasm/beak-engine/Cargo.toml
--test wpt -- --nocapture` (~5 min). Redirect to a log and wait on it rather
than watching a raw pipe — a piped run has been SIGTERM'd mid-way before.

"Inconclusive" means the reference itself rendered blank, so the comparison
says nothing about us either way. Those are excluded from the pass rate
rather than counted as wins.

**Reading the number honestly:** it can go *down* for a good reason. Fixing a
half-masked bug flipped nine tests green→red once — they had only been green
because two bugs cancelled out (we weren't painting `html{background:red}`,
so there was no red to fail on). Never revert on the number alone; look at
each case.

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
| Cascade, specificity, inheritance | css-cascade | 🟡 | full order UA → author (**`(id,class,type)` specificity** + doc-order tie-break) → inline, plus inheritance, plus a second **`!important`** pass on top (css-cascade-4 §6.3). External sheets and `@media` both cascade |
| `!important` | css-cascade-4 §6.3 | ✅ | two-pass author cascade: an `!important` decl wins its property regardless of specificity, order or inline |
| Custom properties / `var()` | css-variables-1 | 🟡 | `--x` collection + `var()` substitution with fallbacks; `@media`-aware, so dark-mode/mobile blocks don't leak into `:root` |
| **UA default stylesheet** | HTML rendering §15 | ✅ | real UA sheet as data (`style.rs::ua_rule`): `display`, em-relative `font-size`, weight/italic/mono, `color` role, margins, list indent — no longer hardcoded in layout |
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
| Positioning (rel/abs/fixed/sticky) | css-position-3 | 🟡 | `relative` (in-flow paint offset) + `absolute`/`fixed` (out of flow, positioned vs nearest `position!=static` ancestor's box / page). `top`/`left`/`right`/**`bottom`** (§10.6.4, needs the viewport height); `top`/`bottom` percentages resolve against the containing block's **height** (§9.3.2). No `z-index` / true fixed-or-sticky scroll behaviour |
| Values & units (px/em/%/rem/…) | css-values-4 | 🟡 | `values.rs`: `px`/`em`/`rem`/`%` lengths, `auto`, `fr`, plus **`calc()`** with `+ - * /` and nesting (one code path for a bare `16px`, a `50%` and a full `calc(100% - 3rem)`). `rem` resolves against the root element, not the parent. No `vw`/`vh`/`ch` |

## CSS — paint

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Color / text color | css-color-4 | ✅ | `color.rs`: `#rgb`/`#rrggbb`/`#rgba`, the named-colour table, `rgb()`/`hsl()`/`hwb()`/`lab()`/`lch()`/`oklab()`/`oklch()`/`color()`, alpha and modern slash syntax |
| Backgrounds / borders | css-backgrounds-3 | 🟡 | `background`/`background-color` fill (behind content) + uniform `border` (4 edges) on block boxes; hex/named colours. No gradients/images/`border-radius`/per-side |
| Font size / weight / family | css-fonts-4 | 🟡 | em-relative `font-size` cascade with correct compounding; **six real subsetted faces** (Inter regular/bold/italic/bold-italic + mono/mono-bold, `fonts.rs`) — synthetic bold/italic retired. No `@font-face` / webfonts / family fallback lists |
| Glyph rasterisation + AA | — | ✅ | fontdue + coverage blend (infrastructure) |
| Transforms / opacity / filters | css-transforms/… | ❌ | §9 frontier |

## Images

| Feature | Status | Notes |
|---------|--------|-------|
| PNG decode | ✅ | `image.rs` (8-bit RGB/RGBA, non-interlaced, miniz_oxide inflate) — wired into `<img>` |
| JPEG decode | ❌ | common web case → next; JPEG `<img>` currently shows a placeholder |
| `<img>` layout + paint | 🟡 | block-level image box (size from `width`/`height`/intrinsic, scaled to fit, aspect kept), decoded PNG blit (nearest-neighbour + alpha) or a labelled placeholder. Shell fetches the bytes (≤16/page, 24 MB decode budget). No inline images / `srcset` / `object-fit` |

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
