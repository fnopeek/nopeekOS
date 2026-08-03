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

### Current number (measured 2026-08-02, beak 0.1.72)

```
3869 pass / 1718 fail / 199 inconclusive   (of 5786 vendored reftests)
= 69.2 % of the conclusive 5587
```

Session arc: 3682 (0.1.64) → 3683 → 3688 → 3723 → 3745 → 3746 → 3863 → **3869**. The
inconclusive count fell 254 → 199 over that span: references that used to
render blank — because `:root` was dropped, or because an `inline-block` had no
box — now paint, so 55 more tests actually measure something.

Per suite (pass / total of that suite, inconclusive included in the total):

| Suite | Pass | Total | % | vs 0.1.69 |
|---|---|---|---|---|
| css-fonts | 40 | 43 | 93.0 | — |
| css-color | 221 | 282 | 78.4 | — |
| css-position | 28 | 36 | 77.8 | — |
| **CSS2** (2.1 suite) | 2605 | 3351 | 77.7 | +4 |
| html-forms | 15 | 21 | 71.4 | — |
| css-text | 240 | 381 | 63.0 | — |
| **css-flexbox** | 235 | 428 | 54.9 | **+86** |
| css-display | 44 | 88 | 50.0 | — |
| **css-align** | 13 | 29 | 44.8 | **+6** |
| css-cascade | 14 | 32 | 43.8 | — |
| css-backgrounds | 60 | 144 | 41.7 | — |
| **css-grid** | 283 | 749 | 37.8 | **+27** |
| css-values | 34 | 93 | 36.6 | — |
| css-sizing | 37 | 109 | 33.9 | — |

**css-position 25.0 → 77.8 %** is bucket-B item 12 landing: rows, row groups
and captions are boxes of their own now, so `position: relative` and a
background work on them. It had dropped to 25 % at 0.1.67 for the honest
reason — 19 of those tests had been green only because neither side of the
reftest painted anything.

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

# Gap map (measured 2026-07-31, beak 0.1.67)

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

1521 elements, 112 distinct properties applied, **5300 implemented / 604
unimplemented** property applications — down from 4340 / 1564 at 0.1.64, so
buckets A and B closed **61 % of the gap**. What is left, by elements affected,
split by whether ignoring it is actually wrong:

| Property | Elems | Dominant value | Ignoring it is… |
|---|---:|---|---|
| `mask-*` (+`-webkit-`) | 28 each | `url(…)` + `center`/`no-repeat` | **wrong** — this is Vector's whole icon system |
| `text-overflow` | 24 | `ellipsis` | **wrong** — truncated labels run on |
| `unicode-bidi` | 21 | `isolate` | **wrong**, but blocked on bidi generally |
| `background-image` / `-position` / `-repeat` / `-size` | 19 each | `url(…)` | **wrong** — 16 real icons unpainted |
| `box-shadow` | 10 | `0 2px 6px -1px rgba(…)` | cosmetic |
| `transform` | 2 | `translateY(-50%)` | **wrong** where used for centring |
| `vertical-align` (inline only) | 74 | 46× `middle`, 10× `text-bottom` | **partly** — cells align since 0.1.65, inline boxes still sit on the baseline |
| `user-select` (+`-moz-`/`-webkit-`) | 112 | `none` | harmless — no selection yet anyway |
| `transition-property`/`-duration` | 68 | — | harmless — ignoring = jump straight to the final state |
| `cursor` | 30 | 22× `pointer` | cosmetic — the *compositor* owns the cursor, not us |
| `scroll-margin-top`, `overflow-anchor`, `touch-action`, `-*-appearance`, `list-style-image:none`, `font-variant:normal`, `text-indent:0` | 1–15 | — | harmless |

**After A + B the top of this list is almost all noise.** That is the point of
the damage column: without it, `user-select` at 112 elements looks like the
biggest remaining item, and it is worth nothing. The two that still cost real
pixels are `mask-image` and `background-image` — the same missing capability
(fetch a sub-resource named from inside CSS), which is why they are one entry
in bucket B.

**Caveat:** one page, one skin. `mask-*` at 28 is *the* icon mechanism and
matters more than its count suggests. Re-run on a second site (a shop, a docs
page, GitHub) before treating this ranking as general.

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

**Properties not parsed at all** (`style.rs::apply_one` has ~135 arms; these
aren't among them): `box-shadow`, `text-indent`,
`text-shadow`, `letter-spacing`, `word-spacing`, `hyphens`, `cursor`, `outline*`, `transform`, `transition`,
`animation`, `aspect-ratio`, `object-fit`, `filter`, `quotes`,
`appearance`, `resize`, `writing-mode`.

**Selectors that drop the whole rule** (`css.rs` returns `None` → the rule is
discarded rather than mis-applied): every pseudo-class outside
`:not()`/`:is()`/`:where()`/`:first-child`/`:last-child`/`:only-child`/
`:nth-child()`/`:nth-last-child()`. That includes **`:root`**, `:checked`,
`:hover`, `:focus`, `:link`/`:visited`, `:first-of-type`/`:nth-of-type()`,
`:empty`, `:disabled`, `:has()`. Pseudo-*elements* other than
`::before`/`::after` also drop the rule.

**Value syntax not understood:** `attr()`, `env()`,
`linear-gradient()`/`radial-gradient()`/`conic-gradient()`, `image-set()`.
A gradient is *recognised* in the `background` shorthand (so it resets the
colour, as the spec says) but never painted.

**CSS images (0.3.0):** `background-image` and `mask-image` are painted, with
`-repeat`/`-position`/`-size` and the `background`/`mask` shorthands. A mask
stencils the element's `background-color` through the image's alpha, which is
how icon systems (MediaWiki Vector, Codex) draw a recolourable icon. `data:`
URIs are decoded by the engine during layout; anything else is reported in
`Layout::css_image_srcs` for the shell to fetch, and arrives as a REPAINT — a
background can never move a box. Still missing: multiple layers per element
(only the first is used), `background-clip`/`-origin`, `background-attachment`,
and a background on an INLINE box (see Paint, below) — which on a real page is
the bigger of the two gaps.

**At-rules skipped:** `@font-face`, `@keyframes`, `@import`, `@layer`,
`@container`, `@page`. (`@media` width features + `prefers-color-scheme` and
`@supports` conditions *are* evaluated.)

**Layout:** `rowspan` (colspan works), sticky positioning (parsed, behaves as
relative), bidi reordering, UAX-14 line breaking, `display: contents`,
multi-column, writing modes.

**Paint order:** the display list is flat and painted in emission order, with
two escapes: an explicit `z-index` range, and — since 0.1.71 — a float layer.
Still flat inside those, so Appendix E's step 6 is only approximated: a
`z-index: 0` positioned box does NOT rise above a float or above in-flow
content it follows in the document. Making it would need `z-index: auto` boxes
to keep participating in the parent's ordering while positioned boxes hoist,
which a single flat list with disjoint ranges cannot express — a real stacking
tree is the fix, and it is a rewrite of `reorder_by_z`.

**Tables:** a row with no cells is dropped in `collect_table_rows`, so it
contributes no height — an empty `<tr>` used as a spacer collapses away.
Costs `border-collapse-empty-row`, whose reference models those spacers as
thicker collapsed borders (it only started rendering right once `tr:not(
:last-child) td` began matching, see 11b).

**Paint:** an inline box paints no background and no border — only its text.
`paint_box_decoration` runs off a block box's resolved geometry, and an inline
box has none of its own (its fragments live in the line boxes). Costs
`display-008/009` outright, and on the real web it is every highlighted or
badged `<span>`.

**Measured on de.wikipedia/Stansstad (0.3.0, 1521 elements):** 44 elements win
a CSS image. **15 of them are `a.external`** — the external-link arrow, an
INLINE box, so this hole is what blocks `background-image` on that page
entirely, not the image machinery. Most of the rest are `.vector-icon` masks
inside `display:none` subtrees (the collapsed menus), which are correctly not
painted. Four icons are actually visible; one is a `data:` URI and paints, three
come from `load.php` and need the fetch. **Note for the next census: `gap.rs`
counts cascade wins and does NOT know about `display:none` ancestors — it read
this page as "28 mask + 19 background elements".**

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

**B — real visual damage, medium effort — 6-8 ✅ DONE in 0.1.66**

6. ✅ `border-radius` (307 elements) — the shorthand with `/`, the four corner
   longhands, percentages, and the §5.5 all-corners scaling when a pair
   overflows a side. New `DrawOp::RoundRect`: a solid fill walks only its
   corner rows (the straight middle stays ONE `fill`, so a page-tall background
   costs what it always did), and `ring > 0` strokes a uniform border as one
   shape so the border follows the curve. Horizontal edges are antialiased —
   without that a 2px corner is a chopped pixel. Every `border-radius` WPT test
   moved measurably closer (`box-shadow-border-radius-001` 18.75 → 14.78 %) and
   two `background-clip` ones flipped green. A **non-uniform** border still
   falls back to four square edges.
7. ✅ `overflow: hidden`/`clip` — reuses the `clip: rect()` machinery.
   `auto`/`scroll` deliberately do NOT clip: with no in-page scroll container,
   clipping there would hide content the user is meant to reach. Two spec rules
   came with it, both caught by WPT: an out-of-flow descendant is clipped only
   by an ancestor in its **containing-block** chain (§11.1.1 — a `static` box
   cannot clip an abspos child, and nothing but the viewport clips a `fixed`
   one), and `overflow` on `html`/`body` **propagates to the viewport**, so
   those two never clip themselves. Skipped when a descendant recorded a
   z-index range in the span — `clip_ops` rebuilds the tail, and a scrambled
   display list is far worse than an unclipped overflow.
8. ✅ `word-break` / `overflow-wrap: break-word` / `word-wrap` (220 elements) —
   split a word at the last character that fits. **Never inside a grapheme
   cluster**: `line-breaking-014` broke the flag of Wales apart, so the break
   point snaps back over ZWJ sequences, variation selectors, skin-tone
   modifiers, keycaps, combining marks, regional-indicator pairs and tag
   sequences. Intrinsic widths are deliberately unchanged (that is
   `overflow-wrap` semantics; true `break-all` would also shrink min-content).
9. `background-image: url()` + `linear-gradient()` + `mask-image` — 19 + 28
   elements, and the entire Vector icon set. Needs sub-resource fetch driven
   *from CSS* (the shell only scans `<img>` and `<link>` today), which is the
   real cost, not the painting. **The biggest remaining real-web item.**

   **Bonus from 7:** `overflow != visible` establishes a block formatting
   context (CSS2.1 §9.4.1) — the predicate had a comment saying so and a TODO
   because the style didn't track overflow. It does now, and that one line took
   `floats-bfc-002`, `floats-wrap-bfc-008` and two flexbox tests with it.

10. ✅ **`display: inline-block`** — was mapped onto plain `Display::Inline`,
    so it had no box at all: no background, no border, no width/height.
    **239 failing tests used it, 165 of them in the REFERENCE** — which is why
    the flexbox suite looked so much worse than it was. Now `Display::
    InlineBlock`: laid out at the origin with the full block box model
    (shrink-to-fit for `auto`, the same §10.3.9 formula floats use) into a
    detached display list, then translated once the line box knows where it
    sits. Blockified when it is a float, out of flow, or a flex/grid item
    (css-display-3 §2.7). It aligns on **its last line box's baseline**
    (§10.8.1) — bottom margin edge only when it has no line box or clips its
    overflow; the six `inline-block-baseline-*` tests measure exactly that.
11b. ✅ **Table cells cascade from their row.** `cell_style` resolved a cell
    with the TABLE's style as its parent, an empty sibling list, and nothing
    between table and cell on the ancestor path — so `tbody tr td` matched
    nothing, `td:first-child` matched everything, and a `tr { color }` never
    reached its cells. A cell's style is now settled once, while the row is
    collected, with the row on the path and its siblings known; `Row` carries
    it, so measurement and layout read the same value instead of re-resolving
    it four times. `push_row_path` puts the row and its group on the path for
    both — a cell's descendants must see the same ancestor chain either way,
    or the widths drift ([[feedback-intrinsic-shared-path]]).

    **WPT barely notices this (+2/−1 net) — the real web is the point.** What
    it did expose is that the *references* of three table tests started
    rendering correctly, which is where two of the three changes below came
    from. Worth remembering when a fidelity fix looks oracle-neutral: check
    whether it moved the reference side.

11c. ✅ **Collapsed borders: rows and row groups take part.** `collapsed_edge`
    compared the cell with the neighbouring cell or the table and nothing
    else, and `border-style: hidden` was folded into `width: 0` — identical to
    `none`. `BorderSide` now carries `hidden`, which wins over every other
    candidate at a grid line (CSS2.1 §17.6.2 rule 1), and a cell's four edges
    resolve against its row and its row group as well. `tr`/`tbody`/`thead`/
    `tfoot { border-style: hidden }` therefore suppresses its cells' borders:
    `border-conflict-style-101/104/105/106`. `102/103` need `<col>`/
    `<colgroup>`, which we do not build at all. The rest of the priority chain
    (style rank, then element rank) still only kicks in at equal width.

11. ✅ **Sibling context for flex items, grid items and inline children.**
    `layout_flex`, `layout_grid` and `collect_inline` resolved child styles with
    `self.styled(ce, st, &[], 0)` — empty preceding-siblings, sibling count 0 —
    so **every child looked like `:nth-child(1)`**. The whole Opera flexbox
    family paints its items in four colours via `:nth-child(n)` and got four
    yellow boxes. **+33 tests, not one regression**, and on the real web this is
    `tr:nth-child(even)` zebra striping and every `:first-child` margin reset.
    Still missing the same way: table cells (`cell_style`), the `<caption>`
    scan, the table-role probe, and `intrinsic_walk` — the last one matters
    because measurement and layout must agree.
12. ✅ **`position: relative` + `background` on table rows and row groups** —
    **+24 tests, the whole `position-relative-table-*` family.**
    `collect_table_rows` returned `Vec<Vec<Cell>>`, throwing the `<tr>` away, so
    a row could be neither styled nor painted. It now yields `Row { el, group,
    cells }`, each carrying its resolved style (with sibling context, so
    `tr:nth-child(even)` stripes), and `lay_table_rows` closes a box around
    every row and around each run of rows sharing a group: the background goes
    in BEHIND the cells, then `position: relative` shifts the whole range —
    cells, text and links together. A positioned row or group is also the
    containing block its cells' abspos descendants resolve against, which is
    what the `-absolute-child` half of the family measures. Cells and
    `<caption>` got the same treatment (a caption is now laid out through
    `layout_box`, so it takes a width, a background and an offset), and a row
    or group set to `display: none` is dropped in `collect_table_rows` — the
    one place both measurement and layout go through.

    **The 2 losses are honest and of the same kind as the wins:**
    `display-008/009` reproduce a row-group background with an *inline* box
    carrying a `background-color`, and we do not paint backgrounds on inline
    boxes. Both sides used to paint nothing; now the test side is right and the
    reference is the one with the hole.

17. ✅ **Floats paint above the in-flow boxes around them, and the containing
    block is the padding box.** Two small positioning fixes from one real page
    (+6, no losses).

    The float one came from a device report: on de.wikipedia, `div.mw-heading`'s
    `border-bottom` rule was drawn straight across the floated infobox. Root:
    the display list is painted in emission order, and the heading comes later
    in the document — but CSS2.1 Appendix E puts non-positioned floats (step 4)
    ABOVE in-flow block boxes (step 3). Floats now record their op range with a
    paint layer, reusing the `z-index` reorder machinery with `(z, layer)` as
    the sort key. **`z-index: auto` must stay untracked**: a tracked range is
    our stand-in for a stacking context, and tracking a `position: relative`
    wrapper made it swallow its children's ranges so their z-indexes stopped
    ordering against each other at all (−10 before that was understood).

    **And the float layer has to work INSIDE a tracked range** (0.1.72): the
    first cut only recorded floats at nesting depth 0, which is WPT-neutral and
    fixed nothing on the real page — MediaWiki wraps the whole article in one
    positioned, z-indexed container, so every float on it sits at depth 1.
    `split_float_ranges` now cuts the enclosing range around each float: the
    pieces keep the parent's `(z, layer)`, the float becomes `(parent z, float
    layer)`, and `reorder_by_z` still sees one disjoint ascending list. Verified
    by rendering the real article and reading the pixel at the crossing:
    `(162,169,177)` (the rule) → `(248,249,250)` (the infobox behind it).

    The containing block a positioned box establishes is its **padding** box
    (§10.1); we were handing out the content edge horizontally and the border
    edge vertically. `padding_cb` now backs out to the padding edges, and
    `definite_cb_height` subtracts the border under `box-sizing: border-box`.

**C — the big structural blocks (oracle-heavy)**

13. ✅ **Flexbox core — 34.8 → 54.9 %, and grid 34.2 → 37.5 % with it (+118).**
    Two things, and the smaller-looking one was the giant:

    **§9.7 resolving flexible lengths** is now the real freeze/unfreeze loop.
    `resolve_flex_line` used to distribute once and clamp afterwards, which
    silently threw away the space a clamped item gave back instead of handing
    it to the items that could still take it; it also measured free space from
    the items' content boxes only, so every item's margin, padding and border
    was counted as free space on top. (+3 on its own.)

    **The flex and grid containers left their own border out of their box.**
    `box_left`/`box_w` subtracted only the padding, and the content origin and
    the bottom edge ignored the border entirely — so a `display: flex` box with
    `width: 40em; border: 1px` came out 640px wide instead of 642 and every
    item sat a pixel off. The whole Opera flexbox family builds exactly that
    box. **+83 flex, +25 grid.** Watch the direction of the fix: `off_left`
    from `resolve_block_h` ALREADY carries `margin + padding + border`, so the
    content origin needed no change — only the border BOX did. Adding the
    border to `content_x` as well double-counted it and cost 11 flex and 24
    grid tests, which is how the mistake was caught.

    Still missing in flex: reverse directions, baseline alignment, and the
    `align-content` cases beyond the 6 that came along.
14. **CSS2 long tail** — `margin-collapse` 28, `abspos-containing-block` 25,
    `floats-wrap` 22, `border-*-width` 44 (`thin`/`medium`/`thick` keywords
    are a suspiciously cheap 44).
15. **Grid** — 467 fails but 84 are experimental `grid-lanes`; the real
    reserve is auto-placement (`row/column-auto`, 64) and abspos-in-grid (48).
16. Bidi reordering — costs ~13 tests *actively* and blocks `unicode-bidi`.

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
| Inline flow / line boxes | CSS2.1 §9.4.2 | ✅ | line boxes with **mixed-style runs** (size/colour/weight/italic) sharing a baseline; greedy wrap; `<a>`/`<b>`/`<i>`/`<code>` flow inline; `<br>` breaks. Atomic inline boxes: `<img>`, form controls and **`display: inline-block`** (a full block box laid out at the origin, then translated into its line; aligned on its own last line box's baseline). No bidi/UAX-14 yet |
| Box model (margin/border/padding) | css-box-3 | 🟡 | full block box model: `width`/`min-width`/`max-width`, `margin` (4-side + **`auto` centering**, §10.3.3 + §10.4 min/max redo), `padding` (4-side), **per-side borders** (width/style/colour), `box-sizing`, logical `margin-inline`/`-block` + `padding-inline`/`-block`, vertical margin collapse → **centered `max-width` containers work**. No `border-radius` |
| Text wrapping / `white-space` | css-text-3 | 🟡 | `normal` collapse+wrap and `pre` (each source line its own line box, trailing spaces hang, §8); `<br>` forces a break even under max-content. **`word-break`/`overflow-wrap`/`word-wrap: break-word`** split an over-long word at the last character that fits, never inside a grapheme cluster (ZWJ sequences, variation selectors, skin tones, keycaps, combining marks, flags, tag sequences). `pre-wrap`/`pre-line`/`nowrap` are **not** distinguished from `pre`/`normal`. No `hyphens`, no UAX-14 line breaking, no bidi reordering |
| Tables (`table`/`tr`/`td`/`th`) | css-tables-3 | 🟡 | `layout.rs`: §17.2.1 anonymous-box fixup, auto **and** `table-layout: fixed` column algorithms, `colspan` (spanning cells distribute only the shortfall), **both border models** — `border-collapse` (winner-takes-the-edge, half the collapsed line per cell, incl. in column widths) and separated with `border-spacing` + `empty-cells` — the `border`/`cellpadding`/`cellspacing` presentation attributes, table border box + `auto` horizontal centring, `<caption>`. **`caption-side`** and per-cell **`vertical-align`** (`top`/`middle`/`bottom`; `baseline` degrades to `top` — no cross-cell baseline alignment). No `rowspan`, `display:inline-table` is block-level |
| Flexbox | css-flexbox-1 | 🟡 | `layout.rs::layout_flex`: row/column, **multi-line wrap**, `flex-grow`/`-shrink`/`-basis` + `flex` shorthand, `gap`, `justify-content` (all 6), `align-items`/`align-self`, `order`, per-item `margin:auto`, automatic content minimum size. No reverse directions, no baseline alignment, no `align-content`, no writing modes. **Weakest suite at 25.7 %** — see the gap map |
| Grid | css-grid-2 | 🟡 | `layout.rs::layout_grid`: `grid-template-columns`/`-rows` (px/%/`fr`/`auto`/`repeat()`/`minmax()`≈), `grid-template-areas`, `grid-auto-rows`, row-major auto-placement, `grid-column`/`-row` (`span N`, `A / B`), `grid-area`, `gap`, `justify-items`/`justify-self`/`place-*`. No dense flow, no abspos-in-grid, no orthogonal/RTL flows |
| Positioning (rel/abs/fixed/sticky) | css-position-3 | 🟡 | `relative` (in-flow paint offset) + `absolute`/`fixed` (out of flow, positioned vs nearest `position!=static` ancestor's box / page). `top`/`left`/`right`/**`bottom`** (§10.6.4); `top`/`bottom` percentages resolve against the containing block's **height** (§9.3.2). **`z-index`** via recorded display-list ranges stable-sorted at the end of layout (§9.9) — the ranges must stay disjoint, a leaking throwaway measurement corrupts the whole list. `sticky` parses but lays out without offsets; `fixed` scrolls with the page |
| Values & units (px/em/%/rem/…) | css-values-4 | 🟡 | `values.rs`: `px`/`em`/`rem`/`ex`/`ch`/`%`/`vw`/`vh`/`vmin`/`vmax`/`pt`/`pc`/`cm`/`mm`/`in`/`Q`, `auto`, `fr`, plus **`calc()`** with `+ - * /` and nesting (one code path for a bare `16px`, a `50%` and a full `calc(100% - 3rem)`). plus **`min()`/`max()`** (variadic) and **`clamp()`**, nestable in any combination with `calc()`. `rem` resolves against the root element, not the parent. No `attr()`/`env()` — those drop the declaration |

## CSS — paint

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Color / text color | css-color-4 | ✅ | `color.rs`: `#rgb`/`#rrggbb`/`#rgba`, the named-colour table, `rgb()`/`hsl()`/`hwb()`/`lab()`/`lch()`/`oklab()`/`oklch()`/`color()`, alpha and modern slash syntax |
| Backgrounds / borders | css-backgrounds-3 | 🟡 | `background`/`background-color` fill (inserted behind content at a recorded op index) + **per-side** `border` (width/style/colour, incl. the shorthands and `border-collapse` edge resolution) + **`border-radius`** (shorthand with `/`, four corner longhands, percentages, §5.5 corner scaling; antialiased fill, and a uniform border stroked as one rounded ring — a non-uniform one keeps square corners). No `background-image`/gradients, no `background-clip`/`-repeat`/`-position`/`-size`, no `box-shadow`, no `border-image`. 42.4 % of the suite |
| Font size / weight / family | css-fonts-4 | 🟡 | em-relative `font-size` cascade with correct compounding; **six real subsetted faces** (Inter regular/bold/italic/bold-italic + mono/mono-bold, `fonts.rs`) — synthetic bold/italic retired. No `@font-face` / webfonts / family fallback lists |
| Text decoration | css-text-decor-3 | 🟡 | `text-decoration`/`-line`: underline / line-through / overline as rects in the run's colour, at metric-free approximations of the font's decoration positions. UA rules: `:any-link` (href-gated), `<u>`/`<ins>`, `<s>`/`<del>`/`<strike>`. Inherited rather than propagated (§1.2) — same pixels for every construct we have. No `-color`/`-style`/`-thickness`, no `text-underline-offset` |
| Glyph rasterisation + AA | — | ✅ | fontdue + coverage blend, warm glyph cache; `fill` builds one row and `copy_within`s it (a per-pixel loop cost ~10× under wasmi) |
| Transforms / filters / shadows | css-transforms/… | ❌ | §9 frontier — `transform`, `filter`, `text-shadow`, `box-shadow` all unparsed |
| `overflow` | css-overflow-3 | 🟡 | `hidden`/`clip` on both axes drop what the box's content painted outside its padding box, and establish a block formatting context (CSS2.1 §9.4.1). Honours the containing-block rule for out-of-flow descendants (§11.1.1) and viewport propagation from `html`/`body` (§3.3). `auto`/`scroll` do **not** clip — there is no in-page scroll container to reach the hidden part with. No scrollbars, and the clip is skipped inside a z-index stacking range |
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
