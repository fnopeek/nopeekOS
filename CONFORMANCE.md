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

### Current number (measured 2026-08-04, beak 0.3.6)

```
3967 pass / 1639 fail / 180 inconclusive   (of 5786 vendored reftests)
= 70.8 % of the conclusive 5606
```

Session arc: 3682 (0.1.64) → 3683 → 3688 → 3723 → 3745 → 3746 → 3863 → 3869 →
3870 (0.3.0) → 3880 (0.3.2) → 3926 (0.3.3) → 3959 (0.3.4) → 3963 (0.3.5) → **3967** (0.3.6). The inconclusive count fell 254 → 180 over that
span: references that used to render blank — because `:root` was dropped,
because an `inline-block` had no box, or because an inline box painted no
background — now paint, so 73 more tests actually measure something.

**0.3.2 in detail: +29 gained, −24 lost, and every loss is a named hole.**
`border-style` with no explicit width (6), `block-in-inline` splitting (3),
`grid-lanes`/masonry which we deliberately do not build (4), bidi (1),
`word-spacing` (1), the inline frame in intrinsic sizing (1,
`slice-nowrap-intrinsic-size`), `width` on `<body>` being ignored so a
percentage resolves against the viewport (1, `calc-text-indent-1`), and 5 that
were INCONCLUSIVE — their reference was blank and now renders, so they finally
measure something and say we are wrong. That is the pattern the oracle keeps
producing: a correct feature makes the *reference* work, and the honest score
goes down before it goes up.

**0.3.3 in detail: +50 gained, −4 lost** — `border-width` and `border-style`
became independent halves (bucket item 19). The four losses: two are
`border-image`, which we do not implement and whose references now paint a real
border; one is `border-right-width-medium` drifting 0.48 % → 0.54 % past the
threshold it was already sitting on; one is a scrollable-baseline test.
**And the real page did not move by one pixel** — de.wikipedia/Stansstad
renders byte-identically before and after, because MediaWiki always writes the
`border` shorthand. The change is oracle-only on that page.

**0.3.4: +33 gained, 0 lost** — `content: attr(X)` (item 20). One function,
one family: 42 of the 51 remaining `CSS2/content-*` failures were `attr()`,
and they all sat at 0.63–0.70 % diff, i.e. one missing string away from green.
Picking it came out of a census of the 1680 remaining failures by family
rather than by suite — the biggest was `CSS2/content` at 51, and reading what
those values actually asked for is what identified the single lever.

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

# Gap map (real-web axis measured 2026-08-04, beak 0.3.4; oracle families below re-counted the same day)

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

`gap.rs` scrapes the implemented set out of `style.rs::apply_one` at run time.
It used to be a hand-maintained list and **went stale twice**, most recently
claiming `background-image`/`mask-image` were missing months after they
shipped — which put a phantom item at the top of this ranking. If the scrape
ever finds under 100 arms it asserts rather than under-reporting.

## Real-web axis — de.wikipedia.org/wiki/Stansstad

1521 elements, 111 distinct properties applied, **5601 implemented / 301
unimplemented** property applications. The arc: 4340 / 1564 at 0.1.64 → 5300 /
604 at 0.1.67 → **5601 / 301** now. What is left, by elements affected, split by
whether ignoring it is actually wrong:

| Property | Elems | Dominant value | Ignoring it is… |
|---|---:|---|---|
| `user-select` (+`-moz-`/`-webkit-`) | 112 | `none` | harmless — no selection yet anyway |
| `transition-property`/`-duration` (+`transition`) | 71 | — | harmless — ignoring = jump straight to the final state |
| `cursor` | 30 | 22× `pointer` | cosmetic — the *compositor* owns the cursor, not us |
| `text-overflow` | 24 | `ellipsis` | **wrong** — truncated labels run on |
| `unicode-bidi` | 21 | `isolate` | **wrong**, but blocked on bidi generally |
| `scroll-margin-top` | 15 | `75px` | harmless — no smooth scrolling to anchor |
| `box-shadow` | 10 | `0 2px 6px -1px rgba(…)` | cosmetic |
| `transform` | 2 | `translateY(-50%)` | **wrong** where used for centring |
| `overflow-x`, `overflow-anchor`, `touch-action`, `-*-appearance`, `list-style-image:none`, `font-variant:normal` | 1–2 | — | harmless |

**🔑 On this page the real-web axis is essentially CLOSED.** Of the 301
remaining applications, roughly 230 are in the "harmless" rows, and the only
ones that still cost pixels are `text-overflow: ellipsis` (24), `box-shadow`
(10) and two `transform: translateY(-50%)`. Everything that used to head this
list — `mask-*`, `background-image` and their placement properties, 47
applications of the Vector icon system — shipped in 0.3.0 + 0.3.2.

**That changes what this axis is for.** One page no longer discriminates, so
the next real-web measurement has to be a DIFFERENT site (a shop, a docs page,
GitHub) rather than a re-run of this one. Until then the oracle axis is the
one carrying information.

## Oracle axis — where the WPT failures sit

Failing-test families, largest blocks first (`FAIL` only, inconclusive
excluded):

Counted by FAMILY NAME (the test name with its trailing number stripped), not
by suite — the biggest suite always looks like the biggest problem, and the
family view is what surfaced the `attr()` lever
([[feedback-census-by-family-not-suite]]).

Re-counted at 0.3.4 over the 1647 remaining failures, largest first:
**`CSS2/bidi` 22 · `CSS2/margin-collapse` 21 · `css-grid/positioned-grid-items`
20 · `CSS2/content` 18 (rest: 3 `counters()` styles, `open-quote`, 4× `\A` as a
forced break, 5× `::before` on `html`/`head`) · `css-grid/orthogonal-positioned-
grid-items` 17 · `css-sizing/contain-intrinsic-size` 15 ·
`css-grid/column-align-items` 15 · `CSS2/abspos-containing-block-initial` 15**.
The whole abspos clan (`abspos*`, `top-*`, `bottom-*`, `left-*`, `right-*`)
is **102** tests and the largest coherent block left, but its diffs run
8–97 %, so it is real layout work rather than a near-miss family.

The older per-suite table, kept for the shape of it:

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
aren't among them): `box-shadow`,
`text-shadow`, `letter-spacing`, `word-spacing`, `hyphens`, `cursor`, `outline*`, `transform`, `transition`,
`animation`, `aspect-ratio`, `object-fit`, `filter`, `quotes`,
`appearance`, `resize`, `writing-mode`.

**Selectors that drop the whole rule** (`css.rs` returns `None` → the rule is
discarded rather than mis-applied): every pseudo-class outside
`:not()`/`:is()`/`:where()`/`:first-child`/`:last-child`/`:only-child`/
`:nth-child()`/`:nth-last-child()`. Inside `:not()` an interaction state we never enter (`:hover`/`:focus`/`:active`/`:target`/`:visited`) is treated as never matching, so the negation holds and the rule survives — that is what makes the visually-hidden idiom work. Otherwise dropped: **`:root`**, `:checked`,
`:hover`, `:focus`, `:link`/`:visited`, `:first-of-type`/`:nth-of-type()`,
`:empty`, `:disabled`, `:has()`. Pseudo-*elements* other than
`::before`/`::after` also drop the rule.

**Value syntax not understood:** `env()`,
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
(only the first is used), `background-clip`/`-origin`, `background-attachment`.
Backgrounds on INLINE boxes landed in 0.3.2 — that was the hole that blocked
this page's icons entirely, not the image machinery.

**At-rules skipped:** `@font-face`, `@keyframes`, `@import`, `@layer`,
`@container`, `@page`. (`@media` width features + `prefers-color-scheme` and
`@supports` conditions *are* evaluated.)

**Layout:** `rowspan` (colspan works), sticky positioning (parsed, behaves as
relative), bidi reordering, UAX-14 line breaking, `display: contents`,
multi-column, writing modes.

**Percentage padding resolves against the FONT SIZE, not the containing
block.** `pad_left` and its siblings are a resolved `f32` on `ComputedStyle`,
computed in the cascade where the containing block's width is not known yet,
and `parse_length` falls back to the em basis — so `padding-left: 50%` comes
out 8px at a 16px font instead of half the parent's width. Block and inline
boxes alike. The fix is to make the four paddings `Len` (as `margin_left`/
`margin_right` already are) and resolve them at layout time. Found while
checking `c547-indent-001`, whose reference reproduces `text-indent: 50%` with
`padding-left: 50%` on a span. Pre-existing, not a 0.3.2 regression.

**`width` on `<body>` is ignored**, so every percentage on the page resolves
against the viewport rather than the body's box. Costs `calc-text-indent-1`
(whose reference bakes the same value in as literal px) and is one reason a
`%`-sized reference and its test can disagree by a constant.

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

**Paint (closed in 0.3.2):** an inline box now paints its background, image,
mask and border, one rectangle per line box it appears on. What is still open
there is `block-in-inline` splitting: a `display: block` child does NOT break
its inline ancestor into the anonymous boxes CSS 2.1 §9.2.1.1 asks for, so the
inline box paints one fragment straight through the block instead of stopping
above it and resuming below (`block-in-inline-003`, `-nested-002`,
`split-inner-inline-2`).

**Note for the next census: `gap.rs` counts cascade wins and does NOT know
about `display:none` ancestors** — it read de.wikipedia/Stansstad as "28 mask +
19 background elements" when 44 elements win a CSS image and most of the icons
sit in collapsed menus. `DCSSIMG=<html> DCSS=<css>` in `tests/diag.rs` reports
per element what stops it painting; run that before sizing an item from
`gap.rs` alone.

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
    `floats-wrap` 22. (`border-*-width` stood here at 44; item 19 took it.)
15. **Grid** — 467 fails but 84 are experimental `grid-lanes`; the real
    reserve is auto-placement (`row/column-auto`, 64) and abspos-in-grid (48).
16. Bidi reordering — costs ~13 tests *actively* and blocks `unicode-bidi`.

18. ✅ **Inline boxes carry a box (0.3.2).** Bucket-B item 9's other half: an
    inline box painted no background, image, mask or border, because
    `paint_box_decoration` runs off a block box's resolved geometry and an
    inline box has none — its fragments live in the line boxes. It now takes
    one rectangle per line box it appears on, and its horizontal margin,
    border and padding advance the inline flow.

    **Why it mattered more than the count suggests:** on de.wikipedia/Stansstad
    15 of the 44 elements that win a CSS image are `a.external`, an inline box.
    So the whole CSS-image feature shipped in 0.3.0 painted **nothing** on that
    page — the icon lives in `padding-right: 1em` that was not reserved, under
    a background that was not painted. Verified by rendering the article twice,
    once with the icon SVG on disk and once without, and diffing the two
    bitmaps: 11 distinct 11×11 boxes appear, and nothing else moves.

    The geometry: a fragment's rectangle is the box's OWN font ascent + descent
    (CSS 2.1 §10.6.1 — the content area, not the line box) grown by its padding
    and border, so vertical padding spills over the neighbouring lines instead
    of pushing them apart. Only the first and last fragment carry the left and
    right edge (`box-decoration-break: slice`). Fragments are painted before
    anything else on their line and sorted by box index — allocation order is
    tree order, which puts an ancestor's background under its descendant's.
    An icon-only `<span>` with no text keeps its line box alive, since padding
    on an inline box is exactly what CSS 2.1 §9.4.2 says does that.

    **Two things this exposed that had been invisible:**
    - A pending space before an inline box was swallowed by the box. It has to
      advance the pen OUTSIDE the box, or the background starts a space too
      early — and the space belongs to the text around the box, not to it.
    - Adjacent same-style text runs are merged into one `DrawOp`, which is only
      valid if the second starts exactly where the first ended. A box edge now
      moves the pen in between, so the merge is gated on the pen not having
      moved (`run_end`). Without that gate the second run was drawn at the
      first one's pen and the gap vanished — 14 `border-*-width-0NN` tests.

    **`text-indent` came with it**, because 4 losses were the same story in
    reverse: the tests indent a block's first line and their references
    reproduce it with `margin-left` on an inline box. Once the reference
    painted, the missing property was the visible half.

    Still open in this corner, all measured: `block-in-inline` splitting (3),
    `border-style` without an explicit width (5, see the paint table),
    `text-indent` in intrinsic widths, and `word-spacing`.

19. ✅ **`border-width` and `border-style` are independent halves (0.3.3).**
    +50 / −4, the biggest single-property jump the tracker has recorded.

    We folded the style into the width — `border-style` only ever set the width
    to 0 for `none`/`hidden`, and nothing else. So `border-style: solid` with no
    explicit `border-width` produced **no border at all** instead of `medium`,
    and `border-width: 5px` with no style produced one that should not exist.
    Both directions are in the corpus, and both are what the whole
    `border-*-width` family (44) and the `c55*-brdr-*`/`c55*-ibrdr-*` families
    turn on.

    `BorderSide` now keeps `spec_width` (the specified value, initial `medium`)
    and `styled` beside `width`, and `width` stays the USED value — which is why
    the change is contained: layout and paint read `width` in ~50 places and
    none of them had to know. Every setter goes through `sync()`.

    Three spec details came with it, each worth a test on its own:
    - **A negative `border-width` is invalid, not zero.** The declaration is
      dropped and the side keeps what it had, so `border-top-width: -1pt` after
      a style leaves `medium` standing. `border-top-width-012` and its 13
      siblings test exactly that; we were clamping to 0.
    - **`border-color`'s initial value is `currentColor`**, and it has to be
      resolved AFTER the whole cascade — `border-style: solid; color: green`
      and `color: green; border-style: solid` must agree. `None` on the side
      means "currentColor, unresolved" and `finish_borders` closes it at the
      end of `resolve` (and of `resolve_pseudo`). That also makes
      `border-color: inherit` land right: the parent's value is the same
      keyword, so it resolves against the child's own colour, which is what
      `border-color-012` asserts.
    - **A shorthand resets every longhand it names**, so `parse_border_shorthand`
      starts from `BorderSide::default()` rather than from what came before.

    **The regression check that mattered was not the oracle:** rendering
    de.wikipedia/Stansstad before and after gave a **byte-identical** bitmap.
    A change to the border model touches every box on every page, and 5786
    reftests averaged into one number cannot say that
    ([[feedback-byte-identical-render-gate]]).

20. ✅ **`content: attr(X)` (0.3.4).** +33, **zero losses** — the cleanest
    entry in this file.

    How it was picked matters more than what it was: a census of the 1680
    remaining failures **by family name** rather than by suite put
    `CSS2/content` on top at 51, and reading what those `content` values
    actually asked for showed **42 of the 51 were `attr()`** — one function.
    All of them sat at 0.63–0.70 % diff, one missing string away from the
    0.5 % threshold. Suite-level numbers would never have surfaced that:
    CSS2 is the biggest suite by far, so its share of the failures looks
    unremarkable until it is broken down.

    `attr()` takes exactly ONE argument. The type/fallback arguments are
    css-values-5 and would change what the value MEANS, so they invalidate the
    declaration rather than being ignored. A missing attribute is the empty
    string, not a dropped value (CSS2.1 §12.2) — the box is still generated.

    The 9 `attr()` tests still failing are not about `attr()`: five put the
    `::before` on `html`/`head` (layout starts at `<body>`), and
    `content-048`'s 99.89 % is the `<body bgcolor>` presentational hint, which
    we do not honour. The rest of the family is 3 `counters()` styles
    (`lower-greek`, `armenian`, `georgian`), `open-quote`, and 4 tests where a
    `\A` in generated content has to force a line break.

21. ✅ **Four device-reported defects, 0.3.5.** A round driven entirely by the
    second axis — Florian running the real page on the device and naming what
    looked wrong. Three of the four were invisible to WPT.

    - **`prefers-color-scheme` was never evaluated.** `parse_media_query` knew
      `min-width`/`max-width` and marked every other feature not-understood, so
      the query failed closed and the block was dropped. Now a `Media { width,
      dark }` threads through the cascade in place of the bare width, and
      `dark` comes from `Theme::is_dark()` (Rec. 601 luma on the page
      background — the shell resolves the theme from the compositor palette, so
      the page theme IS the user's preference). **The parsed sheet is cached,
      and `resolve_vars` bakes the winning custom properties into the text it
      hands on, so the theme had to enter the cache key too** — otherwise
      switching theme reuses the other scheme's sheet.
    - **Form-control chrome followed the DEVICE, not the page.** The reported
      symptom: a black search box on Wikipedia's white page. `paint_control`
      mixed its face from `theme.bg`/`theme.text`, and Wikipedia paints itself
      light whatever the desktop is set to (its dark mode is opt-in, every
      block gated on `html.skin-theme-clientpref-os` — which is why fixing the
      media query alone changes nothing there, exactly as in Firefox). The
      chrome now follows the surface it sits on, read off the control's own
      inherited text colour: light text means a dark surface behind it. That
      keeps a bare page on the device theme and a self-painting page on its
      own colours, which `color-scheme` alone would not (real pages almost
      never declare it).
    - **A grayscale JPEG was thrown away.** We ask zune-jpeg for RGB output and
      a YCbCr source obliges, but a single-component image comes back one byte
      per pixel regardless — and `get_output_colorspace()` echoes the REQUEST
      rather than reporting that. The 3-channel assumption failed its length
      guard and the image became a blank figure. Channel count is now measured
      from the buffer. Wikipedia serves its scanned aerial photographs this way.
    - **`display: table-caption` did not exist**, so MediaWiki's image thumbs
      (`figure{display:table}` + `figcaption{display:table-caption}`) put the
      caption into the anonymous cell beside the image and sized the column to
      the caption text: a 250 px photo in a **551 px** box. Two halves — the
      display value and its `TableRole::Skip`, and then `partition_cells` had
      to let a proper table child END the run of stray siblings instead of
      being transparent to it, because an anonymous cell is a contiguous slice
      and an open run swallows whatever follows. The thumb is 252 px now.
      **+8 `caption-side-applies-to`; −4 `*-applies-to-015`, which are the
      table-caption row of the applies-to matrix and only became measurable
      because the box now exists.**

22. ✅ **`border-color: transparent`, and `white-space: nowrap` (0.3.6).**
    Both from the same device round as item 21.

    **`transparent` is a VALUE on a border, not an absence** (+5, no losses).
    `parse_color("transparent")` returns `None`, and `border-color` treated
    that as "nothing parsed" and kept whatever colour was already there — so a
    page that hides a button's frame with `border-color: transparent` got the
    frame anyway. On de.wikipedia that is every icon button: the hamburger menu
    rendered as an empty 105×34 rectangle. `BorderSide` now carries
    `see_through` as a third state beside "a colour" and "unset =
    currentColor", and `finish_borders` leaves it alone. It also recovered
    `background-rounded-image-clip-001`, one of item 19's four losses.

    **`white-space: nowrap` was folded onto `normal`** (+3, −4). It is now its
    own inherited flag: spaces still collapse, but they are not break
    opportunities, and min-content becomes the whole line rather than the
    widest word (otherwise a shrink-to-fit box is sized to one word and the
    run hangs out of it). **The 4 losses are the reference side**:
    `table-anonymous-objects-081..084` put `white-space: nowrap` on `<body>`,
    so the TEST now renders one line per cell — correctly — while the
    reference, a real `<table>` without it, still wraps. That exposes a
    pre-existing gap: our auto table columns come out too narrow for monospace
    content. Worth its own step.

23. ✅ **An `<img>` has a box too, and a caption sits on the table (0.3.7).**
    WPT-neutral — zero status changes — and both visible on every MediaWiki
    image thumb. This is what the side-by-side against a real browser is for.

    - **A replaced element painted no background and no border.** `<img>` was
      an atomic item in the inline flow that only blitted pixels, so
      `figure … .mw-file-element { border: 1px solid }` — the frame MediaWiki
      puts on every thumbnail — did nothing. The item now carries its box, the
      line reserves the frame's width, and `emit_line` paints background and
      border under the pixels. Same shape as item 18's inline boxes; the only
      difference is the vertical extent, which for an image is the image and
      not a font's ascent + descent.
    - **`layout_table` handed its captions the table's MARGIN-box x.**
      `layout_table_body` takes the horizontal margins off the grid;
      `layout_captions` did not, so a floated table with `margin-left: 1.4em`
      put its caption a margin's width to the left of the rows above it. The
      thumb caption was visibly offset from its own picture.
    - 🔧 **The inspect dev tool was lying about `inline-block` content.** An
      `inline-block` is laid out at the ORIGIN and translated into its line
      afterwards; the ops, links and controls move with it, the inspect boxes
      did not. Every box inside one was reported at the page's top-left corner
      — a gallery thumbnail as `div.thumb @(0,2)`, which reads as a layout bug
      that is not there. They travel with the box now.

24. ✅ **A `::before`/`::after` can be a BOX (0.3.8).** WPT break-even
    (+2/−2) and it puts Wikipedia's logo on screen — the CSS-icon idiom,
    `content: ""` plus a size plus a `background-image`, which no `<img>` is
    involved in. dewiki's logo gadget hides `.mw-logo-icon` outright and draws
    the globe with `.mw-logo::before` and the tagline with
    `.mw-logo-container::after`; 18 rules on that one page use the pattern.

    `resolve_pseudo` used to refuse any generated element with a `display` of
    its own or an explicit `width`/`height` — CONFORMANCE's forward-compatible
    rule, since layout could only place a text run. It now builds an
    `AtomicBox` (background, border, optional text) that every path can place:
    an inline run takes it like an `inline-block`, a block container feeds it
    into its anonymous inline box, a flex container reserves it at the start
    of the main axis.

    **Three guards, each one a test that caught it:**
    - **Out-of-flow generated boxes produce nothing.** MediaWiki underlines the
      active tab with `a::after { position: absolute; bottom: 0; height: 2px }`.
      Placed in flow, that draws a line straight *through* the tab's text —
      which is what the first cut did to "Artikel" and "Lesen".
    - **The display list is closed.** `display: none` and the table-internal
      roles generate no box at all: `before-content-display-012` puts
      `content: "FAIL"` on a `display: table-column-group` and asserts nothing
      appears. The old bail happened to cover these; the new predicate names
      them.
    - **Only the LEADING box in a flex container.** A trailing one has to sit
      directly behind the last item, and reserving it off the main axis puts it
      at the container's far edge instead (`flexbox_generated` measures that
      gap). Waits until generated content is a real flex item.

    Still open here: `display: contents` on a generated element (we have no
    such display at all) and the trailing flex box.

25. ✅ **`min-width` on an out-of-flow box, and SVG gradient fills (0.3.9).**
    WPT-neutral again — zero status changes — both from looking at the render.

    - **`layout_abs` never clamped its width.** The height went through
      `min-height`/`max-height`, the width did not, though CSS2.1 §10.4 applies
      to out-of-flow boxes like any other. It shows on a shrink-to-fit box with
      no content: MediaWiki's search magnifier is an empty absolutely
      positioned `<span>` sized only by `min-width`, and it came out **one
      pixel** wide.
    - **`fill="url(#gradient)"` fell back to BLACK.** `parse_color` does not
      know `url()`, so the fill kept the inherited default — every SVG built on
      gradients rendered as a black blob, which is what Wikipedia's globe was.
      Gradients are now scanned out of the raw document (the parser skips
      `<defs>`) and painted as **one flat colour: the mean of their stops.**
      That is a stand-in, not an implementation — a real gradient needs
      per-pixel interpolation in the rasteriser — but the mean is never
      catastrophically wrong, and at icon size the difference is small. The
      globe reads correctly now.

26. ✅ **`:not(:focus)` keeps its rule (0.3.10).** WPT-neutral, and it takes
    "Zum Inhalt springen" off the top of every Wikipedia page.

    A pseudo-class we do not support inside `:not()` made `parse_compound`
    fail, which dropped the WHOLE rule. But a state a static render never
    enters — `:hover`, `:focus`, `:focus-visible`, `:focus-within`, `:active`,
    `:target`, `:visited` — makes the negation **trivially true**, so the
    clause can be dropped and the rule kept. That is exactly what carries the
    visually-hidden idiom:

    ```css
    .mw-jump-link:not(:focus) { position: absolute; clip: rect(1px,1px,1px,1px);
                                width: 1px; height: 1px; overflow: hidden }
    ```

    Every skip link and every screen-reader-only label on the web is built this
    way. Dropping the rule leaves them sitting in the page as ordinary text —
    which is what a device report called out first, before any of the subtler
    layout problems.

27. ✅ **Out-of-flow generated boxes land where they belong (0.3.11).**
    WPT-neutral, zero status changes — and the active tab gets its underline.

    Item 24 skipped an absolutely positioned `::before`/`::after` outright,
    because placing it IN the flow drew a line straight through the tab's text.
    It is now resolved against its originating box's **padding box** the way any
    out-of-flow child is. That has to happen once the box is FINISHED — its
    height is the containing block — so it hangs off the end of the block and
    flex paths rather than the child walk, and only for a positioned owner (a
    static one is not the containing block).

    **Two things had to be true before it worked**, and both were spec rules we
    only half had:
    - **Blockification (css-display-3 §2.7) applies to `inline` too**, not just
      `inline-block`. A floated or out-of-flow box never joins a line box.
    - **…and it applies to generated boxes.** `resolve_pseudo` built its style
      without that step. The tab underline states no `display` at all —
      `a::after { position: absolute; bottom: 0; width: 100%; height: 2px }` —
      and relies entirely on being blockified to have a box.

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
| Inline flow / line boxes | CSS2.1 §9.4.2 | ✅ | line boxes with **mixed-style runs** (size/colour/weight/italic) sharing a baseline; greedy wrap; `<a>`/`<b>`/`<i>`/`<code>` flow inline; `<br>` breaks. Atomic inline boxes: `<img>`, form controls and **`display: inline-block`** (a full block box laid out at the origin, then translated into its line; aligned on its own last line box's baseline). **Non-atomic inline boxes carry a box too** (0.3.2): horizontal margin/border/padding advance the flow and each line box gets a decoration rectangle, sliced so only the first/last fragment carries the left/right edge. A `display: block` child does not split its inline ancestor into anonymous boxes yet. No bidi/UAX-14 yet |
| Box model (margin/border/padding) | css-box-3 | 🟡 | full block box model: `width`/`min-width`/`max-width`, `margin` (4-side + **`auto` centering**, §10.3.3 + §10.4 min/max redo), `padding` (4-side), **per-side borders** (width/style/colour), `box-sizing`, logical `margin-inline`/`-block` + `padding-inline`/`-block`, vertical margin collapse → **centered `max-width` containers work**. No `border-radius` |
| Generated content (`::before`/`::after`) | CSS2.1 §12 | 🟡 | `content` as concatenated `<string>` tokens (with css-syntax-3 §4.3.7 escapes), **`counter()`/`counters()`** against a real counter scope (`counter-reset`/`counter-increment`, nesting by tree depth) and **`attr(X)`** — the originating element's attribute, empty string when absent. Any other component (`open-quote`/`close-quote`, `url()`, an unknown identifier) makes the WHOLE value produce nothing rather than render half of it. A generated element is a text run when it is `inline`, and **a real box** (background, border, size) for `block`/`inline-block`/`list-item`/`flex`/`grid`/`table` — the CSS-icon idiom. `display: none`, `display: contents` and the table-internal roles produce nothing, as does an out-of-flow one (no containing block resolved for pseudo-elements yet). In a flex container only the LEADING box is placed. `html::before`/`head::before` never render because layout starts at `<body>` |
| `text-indent` | css-text-3 §7 | 🟡 | the block's FIRST line box starts in from the content edge (lengths, percentages of the containing block, negative hanging values). Inherited. **Not counted in intrinsic widths** — a shrink-to-fit box around indented text comes out that much too narrow |
| Text wrapping / `white-space` | css-text-3 | 🟡 | `normal` collapse+wrap, `pre` (each source line its own line box, trailing spaces hang, §8) and **`nowrap`** (spaces collapse but are not break opportunities; min-content is the whole line); `<br>` forces a break even under max-content. **`word-break`/`overflow-wrap`/`word-wrap: break-word`** split an over-long word at the last character that fits, never inside a grapheme cluster (ZWJ sequences, variation selectors, skin tones, keycaps, combining marks, flags, tag sequences). `pre-wrap`/`pre-line` are **not** distinguished from `pre`. No `hyphens`, no UAX-14 line breaking, no bidi reordering |
| Tables (`table`/`tr`/`td`/`th`) | css-tables-3 | 🟡 | `layout.rs`: §17.2.1 anonymous-box fixup, auto **and** `table-layout: fixed` column algorithms, `colspan` (spanning cells distribute only the shortfall), **both border models** — `border-collapse` (winner-takes-the-edge, half the collapsed line per cell, incl. in column widths) and separated with `border-spacing` + `empty-cells` — the `border`/`cellpadding`/`cellspacing` presentation attributes, table border box + `auto` horizontal centring, `<caption>`. **`caption-side`** and per-cell **`vertical-align`** (`top`/`middle`/`bottom`; `baseline` degrades to `top` — no cross-cell baseline alignment). No `rowspan`, `display:inline-table` is block-level |
| Flexbox | css-flexbox-1 | 🟡 | `layout.rs::layout_flex`: row/column, **multi-line wrap**, `flex-grow`/`-shrink`/`-basis` + `flex` shorthand, `gap`, `justify-content` (all 6), `align-items`/`align-self`, `order`, per-item `margin:auto`, automatic content minimum size. No reverse directions, no baseline alignment, no `align-content`, no writing modes. **Weakest suite at 25.7 %** — see the gap map |
| Grid | css-grid-2 | 🟡 | `layout.rs::layout_grid`: `grid-template-columns`/`-rows` (px/%/`fr`/`auto`/`repeat()`/`minmax()`≈), `grid-template-areas`, `grid-auto-rows`, row-major auto-placement, `grid-column`/`-row` (`span N`, `A / B`), `grid-area`, `gap`, `justify-items`/`justify-self`/`place-*`. No dense flow, no abspos-in-grid, no orthogonal/RTL flows |
| Positioning (rel/abs/fixed/sticky) | css-position-3 | 🟡 | `relative` (in-flow paint offset) + `absolute`/`fixed` (out of flow, positioned vs nearest `position!=static` ancestor's box / page). `top`/`left`/`right`/**`bottom`** (§10.6.4); `top`/`bottom` percentages resolve against the containing block's **height** (§9.3.2). **`z-index`** via recorded display-list ranges stable-sorted at the end of layout (§9.9) — the ranges must stay disjoint, a leaking throwaway measurement corrupts the whole list. `sticky` parses but lays out without offsets; `fixed` scrolls with the page |
| Values & units (px/em/%/rem/…) | css-values-4 | 🟡 | `values.rs`: `px`/`em`/`rem`/`ex`/`ch`/`%`/`vw`/`vh`/`vmin`/`vmax`/`pt`/`pc`/`cm`/`mm`/`in`/`Q`, `auto`, `fr`, plus **`calc()`** with `+ - * /` and nesting (one code path for a bare `16px`, a `50%` and a full `calc(100% - 3rem)`). plus **`min()`/`max()`** (variadic) and **`clamp()`**, nestable in any combination with `calc()`. `rem` resolves against the root element, not the parent. `attr()` works in `content` (see Generated content); as a LENGTH (css-values-5) it still drops the declaration, as does `env()` |

## CSS — paint

| Feature | Spec | Status | Notes |
|---------|------|--------|-------|
| Color / text color | css-color-4 | ✅ | `color.rs`: `#rgb`/`#rrggbb`/`#rgba`, the named-colour table, `rgb()`/`hsl()`/`hwb()`/`lab()`/`lch()`/`oklab()`/`oklch()`/`color()`, alpha and modern slash syntax |
| Backgrounds / borders | css-backgrounds-3 | 🟡 | `background`/`background-color` fill (inserted behind content at a recorded op index) + **per-side** `border` (width/style/colour, incl. the shorthands and `border-collapse` edge resolution) + **`border-radius`** (shorthand with `/`, four corner longhands, percentages, §5.5 corner scaling; antialiased fill, and a uniform border stroked as one rounded ring — a non-uniform one keeps square corners) + **`background-image`/`mask-image`** with `-repeat`/`-position`/`-size`, on block AND inline boxes. No gradients, no `background-clip`/`-origin`/`-attachment`, one layer per element, no `box-shadow`, no `border-image`. **`border-width` and `border-style` are independent** (0.3.3): `BorderSide` keeps the specified width beside the used one, so a width with no style paints nothing and a style with no width is `medium`, in either declaration order. A negative width is invalid and keeps the previous value rather than becoming 0. `border-color` defaults to `currentColor`, resolved after the whole cascade so `border-style: solid; color: green` and the reverse agree |
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
