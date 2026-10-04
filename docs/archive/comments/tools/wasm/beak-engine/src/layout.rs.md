# `tools/wasm/beak-engine/src/layout.rs` @ 5e0102684

## L1-15 · `use alloc::collections::BTreeMap;`

```
//! layout.rs — hand-rolled block + inline flow over the styled DOM.
//!
//! Walks the DOM (dom.rs) resolving each element's `ComputedStyle` (style.rs)
//! and turns it into a positioned **display list** (`DrawOp`s) + link hit-rects
//! + a total height. Two formatting contexts, per CSS2.1 §9:
//!
//! * **Block** — block-level children stack vertically; adjacent vertical
//!   margins collapse (the common case).
//! * **Inline** — runs of text + inline elements (`<a>`, `<b>`, `<code>`, …)
//!   flow into **line boxes**: greedy word-wrap to the content width, mixed
//!   sizes/colours/weights on one line sharing a baseline. This is what puts
//!   nav links *inline* with their text instead of each on its own line.
//!
//! Scroll-independent: computed once per (content, width); `raster::paint`
//! draws the visible slice at any offset. Flex/Grid/floats/position come next.
```

## L37-51 · `struct PendingCbH<'a> {`

```
/// The deferred recipe for a positioned box's containing-block HEIGHT.
///
/// §10.1 makes a positioned box the containing block for its absolutely
/// positioned descendants, and that block's height is a USED height — known
/// only once the box has been laid out. Computing it eagerly means a full
/// speculative layout of the whole box, and almost nothing ever reads the
/// answer: only a descendant with `bottom`, or a percentage `top`/`height`,
/// resolves against it. On a large article 266 boxes paid for that and it was
/// over half of the layout.
///
/// So the recipe is kept instead, and run on the first read — in the context
/// the eager measurement would have seen, which is what the saved `path_len`,
/// `cb`, `cb_h` and `floats` restore. Everything here is either `Copy` or, in
/// the case of `floats`, empty in the common case (an empty `Vec` clone does
/// not allocate).
```

## L58 · `border_y: i32,`

```
/// Border box → padding box, subtracted from the measured height.
```

## L64-65 · `resolved: Option<i32>,`

```
/// The answer, once someone has asked. `cb.3` caches it too, but only for
/// as long as that particular `cb` value lives.
```

## L69-74 · `type PosCb = (i32, i32, i32, Option<i32>, Option<u32>);`

```
/// The positioned containing block: `(x, y, width, height)` plus, when the
/// height is not yet known, the index of the recipe that computes it.
/// Deliberately still `Copy` and still a tuple-ish value — the extra slot
/// makes the compiler visit every site that installs or restores a containing
/// block, which is the point: a pending recipe that outlives its `cb` would
/// hand some unrelated descendant the wrong height.
```

## L77-81 · `#[derive(Clone, Copy)]`

```
/// Where everything a layout RECORDS stood before a speculative run — see
/// `Ctx::spec_mark`. One list, deliberately: a recorded vector that is not
/// rolled back leaks trial-run entries into the real page, and that has now
/// happened twice (`stack_ops`/`floats`, then `hover_boxes`). A new side table
/// is added here and is then rolled back by every speculative site at once.
```

## L96 · `#[derive(Clone, Copy)]`

```
/// A speculative flex-item placement additionally moves the containing block.
```

## L103-107 · `const MEAS_FLEX_COL: u8 = 1;`

```
/// Sites that ask for a speculative height. A distinct key per site in the
/// `measured` memo, because the same element asked about by two of them is two
/// different questions with two different derived styles. Only the column axis
/// still measures speculatively — a flex ROW lays its items out for real and
/// keeps the result (see `flex_row`).
```

## L110-117 · `#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]`

```
/// Identity of one speculative height measurement, for the `measured` memo.
///
/// `site` separates the call sites, because the same element can be measured
/// by two of them with different styles — a `position: relative` flex item is
/// measured once as a flex item (with `flex_item_style` applied) and once as
/// its own containing block. The style itself is NOT hashed: at every site it
/// is derived from the element's resolved style (which `styled` already keys
/// by identity) plus `arg`, so the pair identifies it exactly.
```

## L125 · `arg: u32,`

```
/// The site's own style-deriving argument (flex main size), as bits.
```

## L127-129 · `cb_h: u64,`

```
/// Everything ambient that the measurement can read: the containing
/// block's height for percentages, and whether floats are in play (they
/// make the answer depend on where the box sits).
```

## L135-136 · `#[derive(Clone, Copy)]`

```
/// An active float's exclusion rectangle (document space) within a block
/// formatting context. Line boxes and later content avoid these.
```

## L143 · `is_left: bool, // float:left (true) vs float:right (false)`

```
// float:left (true) vs float:right (false)
```

## L146-148 · `fn establishes_bfc(st: &ComputedStyle) -> bool {`

```
/// Whether a block-level box establishes a new block formatting context, so its
/// border box must not overlap floats (CSS2.1 §9.4.1): the formatting-context
/// displays (flex/grid/table) and a box that clips its overflow.
```

## L157-160 · `#[derive(Clone, Copy, Default)]`

```
/// A set of adjoining vertical margins (CSS2.1 §8.3.1). Collapsing margins do
/// not simply take the maximum: the used value is the largest positive margin
/// plus the most negative one (negatives are deducted from the positive max, or
/// from zero when every adjoining margin is negative).
```

## L173 · `fn add(&mut self, m: f32) {`

```
/// Fold one more adjoining margin into the set.
```

## L183 · `fn merge(&mut self, o: Collapse) {`

```
/// Fold another whole set of adjoining margins in.
```

## L192 · `fn value(self) -> f32 {`

```
/// The single used margin the collapsed set resolves to.
```

## L196-204 · `fn px(self) -> i32 {`

```
/// Derselbe Wert in ganzen Pixeln, GERUNDET.
///
/// `as i32` schneidet ab, und ein Rand wird selten ganzzahlig: `h5` hat
/// nach der Spezifikation 22,1776 px. Abgeschnitten verliert jeder Kasten
/// bis zu einem Pixel, und weil der naechste auf der Unterkante des
/// vorigen aufsetzt, addiert sich das die Seite hinunter — auf einer
/// nackten Vorlage waren es 8 px bis zum letzten `<div>`. Chromium rechnet
/// in 1/64 px und rundet erst beim Malen; runden ist die naechste
/// Naeherung, die eine ganzzahlige Auslegung erlaubt.
```

## L210 · `struct Flow {`

```
/// Result of flowing a run of block/inline children with margin collapsing.
```

## L212 · `bottom: i32,`

```
/// Y of the bottom edge of the last committed (non-collapsing) content.
```

## L214 · `open: Collapse,`

```
/// Trailing adjoining margin left open at the bottom (not yet committed).
```

## L216 · `first_top: i32,`

```
/// Border-box top of the first committed content (valid iff `committed`).
```

## L218 · `committed: bool,`

```
/// Whether any content was committed (vs. everything collapsing through).
```

## L220-228 · `open_sealed: bool,`

```
/// Darf der offene Schlussrand noch mit dem Unterrand des Elters
/// verschmelzen?
///
/// **Nein, wenn er von einem geraeumten Element kommt, dessen eigene
/// Raender aneinanderstossen** (CSS 2.1 §8.3.1, letzter Absatz): dessen
/// Rand verschmilzt zwar mit denen der FOLGEGESCHWISTER, aber das
/// Ergebnis nicht mehr mit dem Unterrand des Elters. Genau daran haengt,
/// dass ein Kasten aus `float` + leerem `clear` + `margin-top` seine
/// Hoehe bekommt statt null.
```

## L232 · `struct BoxOut {`

```
/// Result of laying one block-level box in normal flow.
```

## L234 · `bottom: i32,`

```
/// Border-box bottom (== `top_y` when the box collapses through).
```

## L236 · `top_y: i32,`

```
/// Border-box top actually used.
```

## L238-239 · `open: Collapse,`

```
/// Adjoining margin the box exposes to the next sibling / its parent: its
/// bottom margin, or — when it collapses through — its whole collapsed set.
```

## L241-242 · `through: bool,`

```
/// The box has no content, border, padding or height: its top and bottom
/// margins are adjoining and it occupies no vertical space.
```

## L244-248 · `box_x: i32,`

```
/// The box's OWN used border-box left edge and width. Not the containing
/// block's — `max-width`, `margin: 0 auto`, an explicit `width` or plain
/// margins all make the two differ, and the inspect tool reported the
/// parent's numbers for years because they coincide on a plain
/// `width: auto` block.
```

## L253 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// The role a child element plays inside a table box (CSS2.1 §17.2.1).
```

## L258 · `HeaderGroup,`

```
/// `table-header-group` (`<thead>`) — rows sort before every other group.
```

## L260 · `FooterGroup,`

```
/// `table-footer-group` (`<tfoot>`) — rows sort after every other group.
```

## L263-264 · `Skip,`

```
/// `caption`/`col`/`colgroup` — recognised table structure that generates
/// no box of its own; never wrapped, never breaks a stray-content run.
```

## L266-269 · `Other,`

```
/// Anything else: text or an element that isn't a table part. A run of
/// consecutive `Other`/`Cell`/text nodes gets wrapped into an anonymous
/// box (a row, if found directly in a table/row-group; a cell, if found
/// directly in a row) rather than being silently dropped.
```

## L273-276 · `#[derive(Clone, Copy)]`

```
/// A table cell box: a real `<td>`/`<th>`/`display:table-cell` element, or an
/// anonymous cell wrapping a run of sibling nodes that CSS2.1 §17.2.1 requires
/// boxing (stray text/inline content found directly inside a table row, or a
/// stray element — including a lone cell — found directly inside a table).
```

## L283-286 · `struct StyledCell<'a> {`

```
/// A table cell together with the style it resolved to. The style is settled
/// once, while the row is collected — that is the only place where both the
/// cell's position among its siblings (`td:first-child`) and its real parent
/// chain (`tbody tr td`, and inheritance from the row) are known.
```

## L292-295 · `struct Row<'a> {`

```
/// One row of a table's grid, with the boxes it belongs to. Keeping the `<tr>`
/// and its row group here (rather than returning bare cell lists) is what lets
/// a row be styled at all: its own background, and `position: relative`, which
/// moves the whole row — cells included — after it is laid out.
```

## L297-299 · `el: Option<(&'a Element, ComputedStyle)>,`

```
/// The `<tr>`/`display:table-row` element and its resolved style. Absent
/// for an anonymous row wrapping stray content — no element, so no
/// selector can reach it and it paints nothing of its own.
```

## L301-302 · `group: Option<(&'a Element, ComputedStyle)>,`

```
/// The `<tbody>`/`<thead>`/`<tfoot>` this row came from. Consecutive rows
/// carrying the same group form that group's box.
```

## L307-309 · `#[derive(Clone, Copy)]`

```
/// Where a table row / row group box began in the output. Everything emitted
/// from here on belongs to it, which is what lets its background go BEHIND its
/// cells and `position: relative` move the whole thing afterwards.
```

## L313-315 · `enum TableSeg<'a> {`

```
/// One segment of a `flow_children` node list: either a single node laid out
/// normally, or a maximal run of stray table-part siblings (CSS2 §17.2.1)
/// laid out together as one anonymous `table` box. See `segment_table_runs`.
```

## L321-324 · `fn table_content_width(st: &ComputedStyle, avail: f32) -> f32 {`

```
/// A table's content-box width available to its columns: the used `width`
/// (resolved against `avail`) minus the table's own padding+border under
/// `box-sizing: border-box`; `width: auto` falls back to the full available
/// width so an auto-width fixed table still fills its container.
```

## L333 · `fn band_of(floats: &[FloatRect], top: i32, bot: i32, cl: i32, cr: i32) -> (i32, i32) {`

```
/// Narrow the x-range `[cl, cr]` by floats overlapping the band `[top, bot)`.
```

## L348-360 · `fn svg_key(el: &Element) -> alloc::string::String {`

```
/// Resolve a block box's horizontal geometry within a containing block of
/// content width `avail`: CSS2.1 §10.3.3 (used width + margins) plus the
/// §10.4 min/max-width redo. Returns (content-width, content-left-offset =
/// margin-left + padding-left).
/// A replaced element whose content we do not lay out — an `<iframe>`'s
/// document, a `<video>`'s frames, a `<canvas>`'s bitmap, an `<object>`'s
/// plugin. What it has is a BOX, and CSS2.1 §10.3.2 / §10.6.2 give a replaced
/// element with no intrinsic size **300 × 150**; HTML maps the presentational
/// `width`/`height` attributes onto it, which is how a video embed states its
/// size. Returns the intrinsic content size, or `None` for anything else.
///
/// `<img>` is deliberately not here: it has real intrinsic dimensions once its
/// pixels land, and its own path (`img_box`) tracks whether the box was guessed.
```

## L362-364 · `fn svg_key(el: &Element) -> alloc::string::String {`

```
/// The image-store key of an inline `<svg>`. `seq` is the document-order index
/// the parser assigns, so the key is stable across re-layouts of the same
/// document and cannot collide with a page's own `src` (no URL has this shape).
```

## L369-371 · `fn svg_alt(el: &Element, is_svg: bool) -> alloc::string::String {`

```
/// What to show if the raster fails. An icon's accessible name is its
/// `aria-label`/`<title>`, which is also what a page gives a control whose
/// only content is that icon.
```

## L385-389 · `if el.tag == "object" {`

```
// `<object>` is the exception: when its resource cannot be obtained it
// represents its FALLBACK content and is not replaced at all (HTML §4.8.7).
// We never load a plugin, so a fallback is exactly what a browser shows —
// `flexbox_object` measures precisely that. Without a fallback it is still
// an empty replaced box. `<param>` is metadata, not content.
```

## L408-409 · `let pad = st.pad_left + st.pad_right + st.border_x();`

```
// Horizontal padding + border both sit between the content box and the
// margin edge (border-box `width` includes them; content-box adds them).
```

## L430-438 · `fn used_border_box(st: &ComputedStyle, x: i32, avail: i32) -> (i32, i32) {`

```
/// Der EIGENE Randkasten eines Blocks in einem verfuegbaren Streifen.
///
/// `layout_flex` und `layout_grid` rechnen ihn intern genau so aus — hier steht
/// er noch einmal fuer den AUFZEICHNENDEN Pfad. Ohne ihn meldete jeder Flex-
/// und Rasterkasten die Breite seines Streifens statt seiner selbst: ein
/// `display:flex; width:400px` stand mit 1902 px in `getBoundingClientRect`,
/// obwohl es 400 malt. Genau derselbe Fehler war im Flusspfad schon einmal
/// gefixt (MediaWikis `.mw-page-container`) — die BFC-Abzweigung daneben hat
/// ihn behalten.
```

## L446-452 · `fn px_of(v: f32) -> i32 {`

```
/// Eine CSS-Laenge in ganzen Pixeln, GERUNDET.
///
/// beak legt in ganzen Zahlen aus, CSS rechnet in Bruechen — jede Umrechnung
/// ist eine Entscheidung, und sie muss ueberall dieselbe sein. `as i32`
/// schneidet ab: `padding: 1.1in` wurde 105, `margin: 1.1in` (gerundet) 106,
/// und ein Reftest, der beides gegeneinander stellt, scheitert an der
/// Umrechnung statt an der Regel (`CSS2/floats-019`).
```

## L457-458 · `fn translate_op_list(ops: &mut [DrawOp], dx: i32, dy: i32) {`

```
/// Move a detached op list (an `inline-block`'s, laid out at the origin) to
/// where its line box put it.
```

## L471-477 · `DrawOp::Text { x, y, clip, .. } => {`

```
// **Der Ausschnitt eines Textlaufs wandert MIT**, aus demselben
// Grund wie der des Hintergrunds darunter: er steht in
// Dokumentkoordinaten. Blieb er stehen, wurde der Lauf an seiner
// NEUEN Stelle gegen ein Rechteck an der ALTEN geschnitten — und
// uebrig blieb, wo sich beide um ein Pixel ueberlappten, eine
// senkrechte Linie von einem Pixel Breite. Auf DuckDuckGos
// Trefferliste standen die quer durch die Seite.
```

## L486-488 · `DrawOp::BgImage { x, y, clip, .. } | DrawOp::Gradient { x, y, clip, .. } => {`

```
// Der Malbereich steht in Dokumentkoordinaten wie der Kasten
// selbst — bleibt er stehen, schneidet er den Hintergrund an der
// alten Stelle ab.
```

## L499-507 · `fn shadow_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, out: &mut Vec<DrawOp>) {`

```
/// The box's `box-shadow`, painted behind its background. Only the zero-blur
/// case — which on real pages is a hairline separator, not a drop shadow.
/// MediaWiki draws the rule under the article tabs with
/// `box-shadow: 0 1px #c8ccd1`, and without this the page simply lacks it.
///
/// A free function, not a method, because `repaint_hover` has to produce the
/// very same ops from the very same style — a second copy of the rule there
/// would drift, and one that merely FORGOT the shadow silently left the tab
/// underline behind while recolouring the text above it.
```

## L509-511 · `let base = radii_px(st, w);`

```
// **Die Schattenform ist der Rahmenkasten, um den Spread GEWACHSEN** —
// und mit ihm die Ecken (CSS Backgrounds 3 §7.1.1). Ein Schatten mit
// Spread unter einer Pille ist also runder als sie, nicht gleich rund.
```

## L517-519 · `if let Some(sh) = st.shadow_soft {`

```
// Der WEICHE zuerst: er liegt hinter dem scharfen. Das ist die Form, in
// der Bootstrap seine Schatten schreibt (`0 .5rem 1rem rgba(0,0,0,.15)`),
// und bis 0.61.0 fiel sie ganz weg — nur `blur == 0` wurde gemalt.
```

## L543-549 · `let color = sh.color.unwrap_or(st.color);`

```
// An OUTER shadow is not painted inside the border box (CSS Backgrounds
// 3 §7.1.1) — the box is cut out of it. Without that the shadow is a
// full-size copy of the box, and since these boxes are usually
// transparent it floods the whole row instead of leaving the 1px strip
// the author wanted. Subtracting one rect from another gives at most
// four pieces: a band above, a band below, and the left/right slivers
// of the rows in between.
```

## L551-559 · `let r_sharp = grown(sh.spread);`

```
// **Ein `0 0 0 Npx` auf einem runden Kasten ist ein RING, kein Rahmen aus
// vier Rechtecken.** So schreibt das halbe Web seine Umrandungen — DDGs
// Suchfeld hat gar keinen `border`, sein sichtbarer Strich ist der dritte
// Schatten seiner Liste (`0 0 0 1px rgba(0,0,0,.08)`). Als vier Rechtecke
// gemalt bekam die Kapsel eckige Ecken, und das war der ganze Unterschied
// zwischen „sieht aus wie ein Browser" und „sieht aus wie ein Kasten".
//
// Nur ohne Versatz: mit `dx`/`dy` ist die Differenz der beiden Kaesten
// kein Ring mehr, und dafuer bleibt der Weg darunter.
```

## L582-587 · `fn inset_shadow_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, out: &mut Vec<DrawOp>) {`

```
/// Der INNERE Schatten, gemalt ueber den Hintergrund und unter den Rahmen.
///
/// Ohne Weichzeichnung ist er ein Rechteck mit einem Loch: der Kasten minus
/// dem, was der Schatten freilaesst. Bootstrap streift damit seine Tabellen
/// (`inset 0 0 0 9999px`) — bei so einer Ausdehnung ist das Loch leer und der
/// Schatten fuellt die ganze Zelle.
```

## L591 · `let (hx, hy) = (x + sh.dx as i32 + sh.spread as i32, y + sh.dy as i32 + sh.spread as i32);`

```
// Das Loch: der Kasten, verschoben und nach innen geschrumpft.
```

## L615-624 · `fn clip_ops(ops: &mut Vec<DrawOp>, start: usize, cl: i32, ct: i32, cr: i32, cb: i32) -> Vec<Option<usize>> {`

```
/// Clip the display-list ops in `ops[start..]` to the document-space rectangle
/// `[cl, ct) .. [cr, cb)`. Filled rects are intersected (pixel-exact); text and
/// images are kept whole if their box overlaps the rect, dropped otherwise (a
/// flat display list can't clip glyph runs mid-way). An empty rect removes the
/// whole range — the CSS 2.1 `clip` case where nothing of the box is painted.
/// Returns, for every op that was at `start + i`, where it ended up — `None`
/// if the clip dropped it. Side tables that point into the display list (the
/// z-index ranges, the float ranges, a control's own span) have to be
/// rewritten with it: this function REBUILDS the tail, so every index past
/// `start` moves.
```

## L639-640 · `DrawOp::Caret { x, y, w, h, color } => {`

```
// Der Zeiger wird wie ein Rechteck geschnitten — er sitzt IM
// Steuerelement, also darf er dessen Ausschnitt nicht verlassen.
```

## L657-659 · `DrawOp::Shadow { x, y, w, h, blur, color, dx, dy, spread, r } => {`

```
// Ein weicher Schatten wird nur ganz oder gar nicht behalten:
// ihn zuzuschneiden hiesse, seine Deckung neu zu rechnen, und die
// entsteht erst beim Malen.
```

## L665-667 · `DrawOp::RoundRect { x, y, w, h, color, .. } => {`

```
// A rounded box that the clip fully contains keeps its corners;
// one the clip cuts degrades to a square rect, which is wrong at
// the corners but never paints outside the clip.
```

## L679-682 · `DrawOp::Check { x, y, w, h, .. } => {`

```
// Ganz oder gar nicht, wie der weiche Schatten: die Deckung des
// Hakens entsteht beim Malen, und ein halber Haken waere ein
// anderes Zeichen. Ein Kaestchen ist 13 px gross — ein Abschnitt,
// der es zerschneidet, verdeckt es ohnehin fast ganz.
```

## L688-691 · `DrawOp::Image { x, y, w, h, .. } | DrawOp::BgImage { x, y, w, h, .. } => {`

```
// Kept whole when it overlaps, like `Image`: the layer's origin
// is its box, so shrinking the rect would MOVE the background
// rather than crop it. Over-paints only when a clip cuts through a
// box that has one.
```

## L697-699 · `DrawOp::Gradient { x, y, w, h, clip, repeat, pos, size, r, g } => {`

```
// Ein Verlauf traegt seinen Malbereich selbst: der Schnitt geht
// in `clip`, waehrend `x,y,w,h` (die Verlaufsachse) stehen
// bleibt — sonst wanderten die Farbstopps mit dem Schnitt.
```

## L721-723 · `let (nl, nt) = (cl, ct);`

```
// Der neue Ausschnitt wird mit dem alten GESCHNITTEN:
// zwei ineinander liegende `overflow:hidden` begrenzen
// beide, und der innere gewinnt nur, wo er enger ist.
```

## L746-751 · `fn remap_clip(map: &[Option<usize>], start: usize, s: usize, e: usize) -> Option<(usize, usize)> {`

```
/// Rewrite one `[s, e)` span of the display list through a `clip_ops` map.
/// `None` when nothing of it survived — the span is gone and its entry with it.
/// A span that the clip TORE (some ops kept, some dropped) still yields the
/// range that encloses what is left: a stacking range only has to cover its
/// subtree, and covering a dropped neighbour's slot is impossible here because
/// the clip never reorders.
```

## L754 · `return Some((s, e)); // entirely ahead of the clip — untouched`

```
// entirely ahead of the clip — untouched
```

## L760 · `_ if s < start => Some((s, start)),`

```
// Nothing left after the clip edge; keep only the part before it.
```

## L766-775 · `fn effective_filter(st: &ComputedStyle) -> Option<crate::color::ColorFilter> {`

```
/// The colour transform an element actually paints with: its `filter`, then
/// its `opacity`. Both end up in the same matrix — `ColorFilter` already has
/// an alpha factor, because `filter: opacity()` needs one — so opacity costs
/// no second pass over the ops.
///
/// It is an APPROXIMATION of what the spec asks for. Real `opacity` composites
/// the element and its subtree as one group: two overlapping descendants are
/// flattened first, then faded together. Scaling each op's alpha instead lets
/// them show through each other. Getting that exactly right needs an offscreen
/// buffer per stacking context; this is the version that costs nothing.
```

## L787-788 · `fn faded(c: Rgba, k: f32) -> Rgba {`

```
/// Eine Farbe mit der aufgesammelten Inline-Deckung vormultiplizieren.
/// `k == 1.0` (der Normalfall) laesst sie unangetastet — auch in den Bits.
```

## L796-803 · `fn fade_style(st: &ComputedStyle) -> ComputedStyle {`

```
/// Derselbe Stil, mit der Inline-Deckung schon in den Farben. Fuer den
/// SCHMUCK eines Inline-Kastens — Hintergrund, Rahmen, Umriss —, der wie sein
/// Text keinen eigenen Befehlsbereich hat.
///
/// Bewusst nur die Farben, nicht die Bilder: ein Hintergrundbild wird ueber
/// seinen Schluessel erst beim Malen aufgeloest, und ein halbdurchsichtiges
/// Bild braucht einen Filterindex am Befehl. Das ist eine eigene Runde; hier
/// stuende sonst eine Halbheit.
```

## L819 · `fn filter_key(table: &mut Vec<crate::color::ColorFilter>, f: crate::color::ColorFilter) -> u16 {`

```
/// Intern one `filter` transform, deduped, and return its 1-based index.
```

## L828-833 · `fn translate_offset(st: &ComputedStyle, box_w: i32, box_h: i32) -> (i32, i32) {`

```
/// `position:relative` paint offset (dx, dy): `left`/`top` win over `right`/
/// `bottom`; `%` resolves against the containing block's content width.
/// `transform: translate(...)` as whole pixels. Percentages are of the box's
/// OWN border box (CSS Transforms 1 §8) — which is what makes
/// `translate(-50%, -50%)` centre a box on the point it is positioned at, and
/// why this cannot reuse `rel_offset`'s containing-block basis.
```

## L840 · `Len::Auto | Len::Intrinsic(_) => 0,`

```
// `translate` takes no intrinsic keyword; `auto` there is zero.
```

## L853-858 · `let vert = |l: Len| l.px(cb_h.unwrap_or(0.0));`

```
// `top`/`bottom` are of the containing block's HEIGHT (CSS 2.1 §9.3.2), not
// its width. Both axes read `cb_w` here, so `top: 100%` on a 100px-tall box
// in an 800px-wide page moved it 800px down — off the bottom of everything.
// A containing block with no definite height leaves the percentage
// unresolvable and every engine takes it as zero.
// [[feedback_a_percentage_needs_its_own_axis]]
```

## L867-868 · `fn solve_h(width: Len, ml: Len, mr: Len, avail: f32, pad: f32, border_box: bool) -> (f32, f32) {`

```
/// Solve used content-width + left margin for one width value. Auto width fills
/// (auto margins → 0); a definite width lets auto margins center / take slack.
```

## L880 · `(None, None) => (rest / 2.0).max(0.0), // margin:0 auto → center`

```
// margin:0 auto → center
```

## L889 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// 8-bit RGB. The rasteriser converts to the buffer's BGRA at blit time.
```

## L893-897 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// 8-bit RGB plus the alpha a page asked for. `Rgb` stays the opaque value the
/// theme, the image decoders and the compositor speak; alpha lives only on the
/// path a `<color>` travels — cascade → display list → rasteriser — because
/// that is the only path where a page can ask for it and the backdrop it must
/// composite over is known (at paint time, not before).
```

## L908-909 · `pub const fn is_opaque(&self) -> bool {`

```
/// The fast paths (`memory.copy` fills, direct stores) are only valid for
/// an opaque colour; everything else has to read the destination back.
```

## L916-918 · `pub fn over(self, dst: Rgb) -> Rgb {`

```
/// This colour composited over an opaque one. The canvas is the only place
/// that must flatten early: it IS the ground, so there is nothing left to
/// blend against at paint time.
```

## L929-932 · `#[cfg(test)]`

```
/// Unit tests state colours as opaque `Rgb` literals; comparing the two
/// directly keeps those assertions about the CHANNELS rather than restating the
/// wrapper on every line. Deliberately test-only — production code that means
/// "opaque and this colour" should say so.
```

## L946-948 · `#[derive(Clone, Copy)]`

```
/// Resolved page colours for the active theme. The shell fills these from the
/// compositor palette (npk_theme_token) so the page follows light/dark like
/// the rest of the UI; `DARK` is the fallback before the query.
```

## L968-970 · `pub fn is_dark(&self) -> bool {`

```
/// Is this a dark palette? Answers `prefers-color-scheme` — the page theme
/// IS the user's colour-scheme preference here, since the shell resolves it
/// from the compositor palette. Rec. 601 luma on the page background.
```

## L977 · `#[derive(Clone)]`

```
/// One paint instruction, positioned in document space (pre-scroll).
```

## L980-982 · `Text {`

```
/// A run of already-wrapped, same-style text; `y` is the run's top.
/// `sp` is `(letter-spacing, word-spacing)` in px — the run measures and
/// paints at the same advance only because both read this one value.
```

## L991-992 · `family: u32,`

```
/// Streuwert der `font-family` — ohne ihn malte der Rasterer eine
/// andere Schrift als das Layout gemessen hat.
```

## L996-1009 · `clip: Option<(i32, i32, i32, i32)>,`

```
/// Der Ausschnitt, in dem dieser Lauf malen darf — in Dokument-
/// koordinaten, `None` heisst unbeschnitten.
///
/// **Ein Textbefehl wurde vorher GANZ behalten, sobald er den
/// Ausschnitt irgendwo beruehrte.** Das ist bei einem grossen Kasten
/// harmlos und bei einem kleinen das Gegenteil: die
/// `visually-hidden`-Technik des ganzen Webs ist ein Kasten von 1x1
/// mit `overflow:hidden` und einem langen Text darin, und der stand
/// damit LESBAR ueber dem, was daneben liegt — auf DuckDuckGos
/// Kopfzeile „Search Settings" quer ueber dem Zahnrad.
///
/// Der Rasterer klemmt jede Glyphe ohnehin gegen die Leinwand; der
/// Ausschnitt sind dieselben vier Zeilen mit anderen Grenzen, also
/// kostet er kein Pixel mehr.
```

## L1012 · `Rect { x: i32, y: i32, w: i32, h: i32, color: Rgba },`

```
/// A filled rectangle (divider, list bullet).
```

## L1014-1033 · `Shadow { x: i32, y: i32, w: i32, h: i32, blur: f32, color: Rgba, dx: i32, dy: i32,`

```
/// Ein WEICHER Schlagschatten: der Kasten, unter dem er liegt, plus der
/// Weichzeichnungsradius. Der Maler rechnet die Deckung selbst aus.
///
/// Eigener Befehl und kein Haufen `Rect`: ein weicher Schatten hat an
/// jedem Pixel eine andere Deckung, und die entsteht erst beim Malen.
/// Ein scharfer (`blur == 0`) bleibt, was er war — vier Rechtecke, weil
/// er auf echten Seiten meist ein Haarstrich statt eines Schattens ist.
/// `x,y,w,h` is the shadow's own rect — the border box moved by
/// `dx,dy` and grown by `spread`. The three CSS numbers ride along
/// because the painter needs the BORDER BOX back: an outer shadow is not
/// painted inside it (css-backgrounds-3 §7.1.1), and that cut-out is a
/// different rectangle as soon as there is an offset or a spread. Keeping
/// them (rather than the border box itself) is what makes the op survive
/// a translation untouched.
/// Ein aeusserer Kastenschatten. `r` sind die Eckradien der SCHATTENform
/// — der Radius des Rahmenkastens, um den Spread gewachsen (CSS
/// Backgrounds 3 §7.1.1). Ohne sie malt ein Schatten unter einer Pille
/// eckige Ecken, und genau daran sah DuckDuckGos Suchfeld aus wie ein
/// Kasten statt wie eine Kapsel: der sichtbare Ring dort ist ein
/// `box-shadow`, kein Rahmen.
```

## L1036-1039 · `RoundRect { x: i32, y: i32, w: i32, h: i32, r: [f32; 4], color: Rgba, ring: f32 },`

```
/// A `border-radius` box. `r` is `[tl, tr, br, bl]` in px; `ring` is 0 for
/// a solid fill, or the border thickness to stroke along the inside edge.
/// Kept apart from `Rect` so the plain case stays one `memory.copy` per
/// row — the rounded one has to walk its corner rows.
```

## L1041-1046 · `Caret { x: i32, y: i32, w: i32, h: i32, color: Rgba },`

```
/// Der Schreibzeiger in einem Textfeld — ein eigener Befehl, damit er
/// BLINKEN kann.
///
/// Als gewoehnliches Rechteck kostete jeder Takt ein Neuauslegen (am
/// Geraet 10-40 ms, zweimal je Sekunde); als eigene Art laesst der
/// Rasterer ihn einfach aus, und der Wirt malt nur seinen Streifen neu.
```

## L1048-1056 · `Check { x: i32, y: i32, w: i32, h: i32, color: Rgba },`

```
/// Der Haken eines angekreuzten Kaestchens: zwei Striche im Kasten
/// `x,y,w,h`.
///
/// Eigener Befehl aus demselben Grund wie `Shadow` — seine Deckung
/// entsteht erst beim Malen. Ein Haken aus Rechtecken ist eine Treppe,
/// und bei 13 px sieht man jede Stufe. Vorher stand hier ein gefuelltes
/// QUADRAT, und das ist nicht bloss haesslich: ein Haken und ein Punkt
/// sind die zwei Zeichen, an denen man ein Kaestchen von einem
/// Radioknopf unterscheidet.
```

## L1058-1069 · `Image { x: i32, y: i32, w: i32, h: i32, src: String, alt: String, fit: ObjectFit, filter: u16 },`

```
/// A decoded image, scaled to `w`×`h` at blit time.
/// An `<img>` box. Carries the `src` KEY, not the decoded pixels: the
/// rasteriser looks the image up when it paints, and draws a placeholder
/// on a miss. That way an image arriving after layout costs a repaint
/// instead of a full re-layout — which on a real article is the
/// difference between ~15 ms and ~145 ms, per image batch.
/// `fit` is `object-fit`: the box is `w`×`h` either way, the picture
/// inside it is placed by the rasteriser, which is the only place the
/// intrinsic size is known (the pixels are looked up at paint time).
/// `filter` is a 1-based index into `Layout::filters`, 0 for none — the
/// pixels only exist at paint time, so the transform has to travel with
/// the op rather than being applied to a colour here.
```

## L1071-1077 · `BgImage {`

```
/// A `background-image` or `mask-image` layer over the box `x,y,w,h` (the
/// background positioning area). Carries the `url_key`, not the pixels —
/// same reason as `Image`: an asset arriving late costs a repaint, never a
/// re-layout, because a background never affects geometry.
///
/// `tint: Some(c)` is the mask case — the image's alpha stencils colour
/// `c` instead of its own pixels being drawn.
```

## L1083-1086 · `clip: (i32, i32, i32, i32),`

```
/// The painting area (`background-clip`) as `(x, y, w, h)`. `x..h` above
/// are the POSITIONING area (`background-origin`); the two are the same
/// rectangle only when neither property is set and the box has no
/// border.
```

## L1093-1095 · `filter: u16,`

```
/// Same 1-based index as `Image::filter`. A MASK never uses it: it
/// paints `tint` through the image's alpha, so the transform lands on
/// that colour at layout time instead.
```

## L1098-1102 · `Gradient {`

```
/// Ein Farbverlauf als Hintergrund.
///
/// Eigener Befehl und keine Kette aus Rechtecken: ein Verlauf hat an jedem
/// Pixel eine andere Farbe, und tausend 1-px-Streifen je Kasten waeren
/// eine Anzeigeliste, die niemand mehr lesen kann.
```

## L1106-1107 · `r: [f32; 4],`

```
/// Eckenradien der Kastenform, damit ein Verlauf unter einer runden
/// Ecke nicht darueber hinauslaeuft.
```

## L1109-1113 · `repeat: (bool, bool),`

```
/// Dieselben drei wie bei `BgImage`, und aus demselben Grund: ein
/// Verlauf IST ein Hintergrundbild. Tailwinds Punkt- und Gittermuster
/// sind ein 10x10 grosser Verlauf, der sich kachelt — ohne
/// `background-size` waere es eine einzige Kachel ueber die ganze
/// Seite, und das Muster verschwaende.
```

## L1121-1126 · `enum Kid<'a> {`

```
/// Ein Flex-Kind: ein Element, oder ein ANONYMER Kasten um einen nackten
/// Textlauf (css-flexbox-1 §4).
///
/// Der anonyme traegt seinen fertigen Kasten mit sich — er hat kein Element,
/// also auch keinen Weg durch `layout_box`, und `AtomicBox` ist genau die
/// Form, die das Layout dafuer schon hat (ein `::before` ist derselbe Fall).
```

## L1138 · `fn op_bottom(op: &DrawOp) -> i32 {`

```
/// The bottom edge a draw op reaches, for sizing the scrollable page.
```

## L1141-1142 · `DrawOp::Text { y, size, .. } => y + ceil_i32(*size),`

```
// A text run's `y` is its top; `size` over-estimates the descent
// slightly, which is the safe direction for a scroll extent.
```

## L1151-1152 · `DrawOp::Shadow { y, h, .. } => y + h,`

```
// Der weiche Rand reicht ueber den Kasten hinaus, aber er ist
// durchsichtig und soll die Seite nicht laenger machen.
```

## L1157-1159 · `fn radii_px(st: &ComputedStyle, w: i32) -> [f32; 4] {`

```
/// `border-radius` resolved to px against the border-box width. Percentages
/// resolve per-axis in CSS; we draw circular corners, so the width is the one
/// basis.
```

## L1166-1167 · `fn uniform_border(st: &ComputedStyle) -> Option<(f32, Rgba)> {`

```
/// The border's `(width, colour)` when all four sides carry the same visible
/// one, else `None`.
```

## L1182-1186 · `fn bg_ops(st: &ComputedStyle, bg: Option<u64>, mask: Option<u64>, x: i32, y: i32, w: i32, h: i32, out: &mut Vec<DrawOp>)`

```
/// One box's background layer, bottom-up: colour (or a mask stencilling it),
/// then the image. Shared by block boxes, which insert it UNDER content they
/// have already emitted, and by inline-box fragments, which push it ahead of
/// their line's text. The keys are resolved by the caller — only it knows
/// where to register the image the layout still needs.
```

## L1188-1192 · `let (cx, cy, cw, ch) = st.bg_clip.shrink(st, x, y, w, h);`

```
// Three rectangles, two of them used here: `background-clip` says where the
// paint may land, `background-origin` where the image is anchored and what
// a percentage size resolves against. They default DIFFERENTLY — border box
// and padding box — so a bordered box with a centred image centres it
// inside the border while its colour still runs under it.
```

## L1214-1216 · `let r = radii_px(st, w);`

```
// A corner radius is measured on the border box; clipping the
// background inwards pulls the curve in with it by the same amount
// (css-backgrounds-3 §5.3), never below zero.
```

## L1231 · `(None, _) => {}`

```
// A mask with no colour to stencil paints nothing at all.
```

## L1249-1251 · `if st.bg_layer.gradient.is_some() && cw > 0 && ch > 0 {`

```
// Ein Verlauf ist dieselbe Schicht wie ein `url()` — `background-image`
// ist eines von beidem, nie beides. Er wird ueber die Positionierflaeche
// gespannt und am Malbereich beschnitten, wie das Bild auch.
```

## L1253-1254 · `let r = radii_px(st, w);`

```
// Wie bei der Farbe: der Radius wird am Rahmenkasten gemessen, und
// ein nach innen gezogener Malbereich zieht die Rundung mit.
```

## L1277-1280 · `fn border_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, sides: (bool, bool), out: &mut Vec<DrawOp>) {`

```
/// One box's four border edges. `sides` says whether the box's left and right
/// edges belong to THIS rectangle — a fragment of an inline box that continues
/// on from the previous line, or breaks onto the next one, carries neither
/// (the `box-decoration-break: slice` default).
```

## L1282-1285 · `let r = radii_px(st, w);`

```
// A rounded border can only be stroked as one shape, so it needs all four
// sides to agree; anything else falls through to the four independent
// edges below (square corners, visibly wrong only once the radius is
// larger than the border).
```

## L1293 · `let side = |out: &mut Vec<DrawOp>, s: &BorderSide, rect: (i32, i32, i32, i32)| {`

```
// Each side paints independently on the border-box edge.
```

## L1319-1322 · `fn outline_ops(st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, sides: (bool, bool), out: &mut Vec<DrawOp>) {`

```
/// The `outline` ring (css-ui-4 §3). Unlike a border it takes NO space — it is
/// drawn outside the border box, offset outwards by `outline-offset`, and the
/// layout never sees it. That is the whole reason it exists: a focus ring has
/// to be able to appear without moving the page under the reader.
```

## L1329 · `let (rx, ry) = (x - off - ow, y - off - ow);`

```
// Grow the border box by the offset, then lay the ring OUTSIDE that.
```

## L1337-1338 · `let grow = (off + ow) as f32;`

```
// A rounded box's outline follows its curve, widened by the ring's own
// distance from the box (css-ui-4 §3.4).
```

## L1351-1352 · `if sides.0 {`

```
// An inline box that continues onto the next line carries no side edge,
// exactly as its border does.
```

## L1361 · `pub struct LinkRect {`

```
/// A clickable link's document-space rectangle.
```

## L1370-1373 · `pub struct InspectBox {`

```
/// A laid-out element's document-space box plus a human label — the data behind
/// beak's "inspect" dev tool. Recorded only when inspection is enabled (see
/// `Ctx::inspect`); the shell hit-tests these and shows the deepest box under
/// the cursor so a mis-placed element can be named on the device.
```

## L1379-1380 · `pub depth: u16,`

```
/// Tree depth (ancestor count) — the deepest box containing a point is the
/// most specific element there.
```

## L1382-1383 · `pub label: String,`

```
/// `tag#id.class  W×H  display:… float:… position:…` — enough to find the
/// element in the page and see the geometry/box properties that went wrong.
```

## L1387-1389 · `pub struct ControlRect {`

```
/// An interactive form control's document-space rectangle. The shell hit-tests
/// these to give a control focus / activate it; `seq` identifies the element
/// across re-layouts (`dom::Element::seq`).
```

## L1397-1401 · `at: usize,`

```
/// Where this control's ops sit in `Layout::ops`, and the box they were
/// painted from. A control's own state — focus, checked, the typed value,
/// the caret — changes far more often than the page does, and repainting
/// that range beats laying the document out again by three orders of
/// magnitude ([[project-beak-pointer-and-repaint]]).
```

## L1409-1411 · `pub width: u32,`

```
/// The viewport width this was laid out at. A layout that does not say so
/// cannot be re-read later — `repaint_hover` has to resolve a style the
/// same way the layout did, and `vw`/media queries are part of that.
```

## L1415 · `pub height: u32,`

```
/// Total document height (px). May exceed the viewport → scroll.
```

## L1417-1423 · `pub viewport_h_used: bool,`

```
/// Did this layout actually depend on the viewport HEIGHT? When false, a
/// purely vertical resize cannot move a single box, so the shell may reuse
/// this layout and just re-clip — the difference between a repaint and a
/// full re-layout (~6.4 s on device for a big article).
///
/// Sound in the direction that matters: it over-reports (value-equality on
/// the containing block, any matched `vh` rule), never under-reports.
```

## L1425-1432 · `pub phase: [u64; 3],`

```
/// What the three pipeline phases cost, in whatever unit the caller's
/// clock counts (see `Engine::set_clock`). Zero when no clock is set.
///
/// The device reports parse+cascade+layout as ONE number, which is exactly
/// the number we cannot act on: a host profile says the box layout
/// dominates, but the host is not an interpreter and the phases do not
/// scale alike under one. Splitting it needs a clock, and the engine has
/// no host functions by design — so the caller lends it one.
```

## L1434-1435 · `pub bg: Rgb,`

```
/// Canvas background — the `<body>` background propagated to the whole
/// viewport (CSS backgrounds §3.11.2), else the theme background.
```

## L1437-1445 · `pub guessed_image_srcs: Vec<String>,`

```
/// `src`s whose `<img>` box was GUESSED (no pixels yet, and no
/// `width`/`height` pair to size it definitely).
///
/// The shell uses this to decide what an arriving image costs: a `src`
/// that is NOT in here has a definite box, so its pixels only need a
/// REPAINT; one that is in here can still move the page when it decodes,
/// which warrants a re-layout. On a real article that is the difference
/// between ~15 ms and ~145 ms — and under the device's WASM interpreter,
/// between a page that scrolls while it loads and one that freezes.
```

## L1447-1450 · `pub css_image_keys: Vec<u64>,`

```
/// `url_key`s of the CSS images (`background-image`/`mask-image`) this
/// layout actually needs — i.e. the ones that won the cascade on a box we
/// painted, not every `url()` in the stylesheet. The engine turns these
/// back into URLs (via the sheet's table) for the shell to fetch.
```

## L1452-1454 · `pub css_image_srcs: Vec<(u64, String)>,`

```
/// The subset of `css_image_keys` the shell still has to fetch, as
/// (key, URL) — `data:` URIs are resolved by the engine itself and never
/// appear here. Filled in by `Engine::layout_ext`, which holds the sheet.
```

## L1456-1463 · `pub inline_svgs: Vec<(u32, Rgb, u32, u32)>,`

```
/// Inline `<svg>` elements this layout painted, as (seq, colour, w, h).
///
/// An inline SVG is a replaced element with no `src`, and it cannot be
/// rasterised before the cascade runs: `currentColor` — what practically
/// every icon set paints with — IS the element's computed `color`, and the
/// box is decided by CSS, not by the SVG's own attributes. So layout states
/// what it needs and `Engine::resolve_inline_svgs` renders it afterwards,
/// the same split `css_image_srcs` already uses.
```

## L1465 · `pub inspect: Vec<InspectBox>,`

```
/// Element boxes for the inspect dev tool (empty unless inspection was on).
```

## L1467-1468 · `pub hover_boxes: Vec<HoverBox>,`

```
/// Element boxes for pointer hit-testing (empty unless the sheet has
/// `:hover` rules).
```

## L1470-1474 · `pub filters: Vec<crate::color::ColorFilter>,`

```
/// The `filter` colour transforms this layout used, referenced by the
/// 1-based index an image op carries. A side table rather than a field on
/// the op: a `ColorFilter` is 52 bytes and `filter` is rare, so carrying
/// one per op would roughly double the display list on every page that has
/// no filter at all.
```

## L1478-1489 · `#[derive(Clone, Copy)]`

```
/// An element's box, for deciding what the pointer is inside.
///
/// Deliberately not `InspectBox`: that one carries a formatted label, and this
/// list exists on every page with a hover rule, not only while a developer is
/// inspecting.
/// Der Kasten EINES Elements, so wie ein Skript ihn erfragt.
///
/// Eigene Form statt `HoverBox` durchzureichen: die traegt Anker, Pseudo-Art
/// und Nachmal-Fahnen mit, von denen hier nichts gebraucht wird — und sie
/// deckt Steuerelemente nicht ab. Ein `<button>` wird von `paint_control`
/// gemalt und steht in `controls`; genau auf solche Kaesten fragen Seiten
/// aber am haeufigsten.
```

## L1497-1498 · `pub bx: i16,`

```
/// Rahmenbreiten, waagerecht und senkrecht summiert — `clientWidth` ist
/// der Rahmenkasten OHNE sie.
```

## L1501-1505 · `pub px: i16,`

```
/// Polsterung, ebenso summiert. `ResizeObserver` meldet den INHALTSkasten,
/// und der ist der Rahmenkasten ohne beides — ohne diese zwei Zahlen waere
/// die gemeldete Groesse um die Polsterung zu gross, und ein Diagramm, das
/// sein Zeichenfeld daraus baut, waere in einem gepolsterten Kasten jedes
/// Mal zu breit.
```

## L1508-1511 · `pub positioned: bool,`

```
/// `position` ist nicht `static`. Das ist die ganze Frage, die
/// `offsetParent` stellt (CSSOM View §5): der naechste positionierte
/// Vorfahr. Ohne diese Ecke muesste die Bindung fuer JEDEN Vorfahren die
/// Kaskade neu aufloesen — dieselbe Antwort, hundertmal teurer.
```

## L1522-1533 · `pub anchor: Option<OpKey>,`

```
/// Where this element's box decoration BELONGS in the display list, named
/// by the op that sits there rather than by an index.
///
/// A background that only exists while the pointer is inside has nothing
/// to replace — it has to be inserted, and a box inserts its decoration
/// ahead of everything it paints. An index would be the obvious way to say
/// where that is and the wrong one: the list is still inserted into,
/// clipped and reordered by z after this is recorded, and every one of
/// those moves it. Content does not move.
///
/// `None` when the box painted nothing at all — then there is no "ahead of"
/// to speak of, and a repaint hands the page to a layout.
```

## L1535-1537 · `pub paint: (i32, i32, i32, i32),`

```
/// Where this fragment's own decoration is painted, which is NOT the hit
/// rect: an inline box's background covers its font's ascent + descent plus
/// padding, not the line box (CSS 2.1 §10.6.1).
```

## L1539-1540 · `pub sides: (bool, bool),`

```
/// Which of the left/right borders this fragment draws — a box broken
/// across lines draws them only on its outer ends.
```

## L1542 · `pub shadow: bool,`

```
/// A block box paints its `box-shadow`; an inline fragment does not.
```

## L1544-1547 · `pub pseudo: crate::css::PseudoElem,`

```
/// Which pseudo-element this box belongs to. `None` is the element itself;
/// a `::before`/`::after` gets its own box because a hover rule reaches it
/// — MediaWiki underlines the article tabs with `a:hover::after`, and a
/// repaint that had no rectangle for it could only give up.
```

## L1549-1551 · `pub anchor_after: bool,`

```
/// The anchor names the op the decoration goes AFTER, not before it. An
/// absolutely positioned pseudo is appended at the end of what its
/// originating element painted, so what it can name is its predecessor.
```

## L1553-1555 · `pub has_text: bool,`

```
/// Does this box paint text of its own? A pseudo's `content` string is not
/// part of what the element SAYS, so a colour change on one cannot be
/// repainted from the display list alone.
```

## L1557-1560 · `pub hoverable: bool,`

```
/// Does this box take part in `:hover`? False for one recorded only so a
/// `<summary>` can be clicked — the pointer being inside it is not a
/// cascade event, and reporting it would repaint on every page that has a
/// `<details>` and no hover rule at all.
```

## L1562-1567 · `pub bx: i16,`

```
/// Rahmenbreiten, waagerecht und senkrecht SUMMIERT.
///
/// Nur dafuer da, dass `clientWidth`/`clientHeight` den Polsterkasten
/// nennen koennen statt des Rahmenkastens. Zwei Zahlen statt vier, weil
/// `clientLeft`/`clientTop` im Aufrufzensus gar nicht vorkommen — und eine
/// Zahl, die niemand liest, ist Ballast auf einem heissen Pfad.
```

## L1570-1571 · `pub px: i16,`

```
/// Polsterung, waagerecht und senkrecht summiert — fuer den
/// INHALTSkasten, den ein `ResizeObserver` meldet.
```

## L1574 · `pub positioned: bool,`

```
/// `position` ist nicht `static` — die Frage, die `offsetParent` stellt.
```

## L1576-1583 · `pub toggle: bool,`

```
/// Clicking this box opens/closes its `<details>`.
///
/// It rides in `hover_boxes` rather than in a list of its own because this
/// list is ALREADY carried through everything a hit rect has to survive:
/// the rollback mark, the relative-offset shift, and the drain into an
/// `AtomicBox`. A fourth parallel list would have to repeat all three, and
/// the one time that was done by hand it shipped with a missing shift
/// (0.25.0, see `shift_since`).
```

## L1587-1589 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Enough of an op to find it again: kind, position, and the two numbers that
/// tell same-shaped ops apart. Deliberately small and `Copy` — one of these
/// hangs off every hit rect on the page.
```

## L1599 · `fn op_key(op: &DrawOp) -> OpKey {`

```
/// The key of an op, for `HoverBox::anchor`.
```

## L1617-1621 · `pub fn control_spans(&self) -> Vec<(u32, usize, usize)> {`

```
/// Wo die Befehle eines Steuerelements liegen — `(seq, at, len)`.
///
/// Nur fuer Proben: `repaint_controls` ersetzt genau diese Spanne, und
/// wenn sie nicht stimmt, frisst die Ersetzung den Nachbarn. Von aussen
/// war das bisher nicht nachzusehen.
```

## L1626-1644 · `pub fn caret_rect(&self) -> Option<(i32, i32, i32, i32)> {`

```
/// Does any box this layout painted for one of `srcs` reach into the
/// vertical band `[top, bottom)` of the document?
///
/// The shell asks this before repainting for an arriving `<img>`. A
/// repaint is the WHOLE viewport — 1902x1000x4 = 7,6 MB of fill, ~50 ms
/// on the device — and an image that landed below the fold cannot change
/// a single visible pixel. Painting for it is all of the cost and none of
/// the picture. Nothing is lost: scrolling marks the page dirty anyway,
/// so the image is drawn the moment it can be seen.
///
/// Answered from the display list rather than from a side table, because
/// the display list is where an image's PLACED box lives — `img_box` only
/// measures, and the y a repaint cares about is decided when the box is
/// flowed. One pass per arriving batch, not per image.
/// Der Kasten des Schreibzeigers in DOKUMENTkoordinaten, wenn die Seite
/// gerade einen malt.
///
/// Der Wirt braucht ihn, um im Takt nur DIESEN Streifen neu zu malen —
/// ein blinkender Zeiger, der die ganze Seite kostet, blinkt nicht lange.
```

## L1661-1665 · `pub fn css_images_in_band(&self, keys: &[u64], top: i32, bottom: i32) -> bool {`

```
/// As [`Self::images_in_band`], for `background-image`/`mask-image` layers.
///
/// Tested against the op's CLIP rectangle, not its positioning area: the
/// clip is what actually gets painted, and with `background-origin` or a
/// border the two are different rectangles.
```

## L1675-1683 · `pub fn element_rects(&self) -> Vec<ElemRect> {`

```
/// The deepest (most specific) inspect box containing a document-space
/// point, for the inspect dev tool. Ties break toward the one recorded
/// later (painted on top).
/// Die Kaesten aller Elemente, die einer gemalt hat — Elementkaesten UND
/// Steuerelemente, in einer Liste.
///
/// Ein Kasten kann mehrfach vorkommen: ein Inline-Kasten hat ein Fragment
/// je Zeile. Das ist gewollt — `getClientRects` nennt sie einzeln,
/// `getBoundingClientRect` ihre Vereinigung.
```

## L1690-1694 · `for b in &self.hover_boxes {`

```
// **Der Kasten eines Steuerelements ist das Steuerelement.** Ein
// blockweiter Knopf bekommt vom Blockpfad AUSSERDEM einen Kasten in
// voller Spaltenbreite aufgezeichnet — und der stand vorher zuerst in
// der Liste. `getBoundingClientRect` gab dann 620 px statt der 296,
// die gemalt werden, und `margin:auto` sah aus, als wirke es nicht.
```

## L1713-1715 · `pub fn hit_toggle(&self, x: i32, y: i32) -> Option<u32> {`

```
/// The `<summary>` at a document-space point, by element `seq` — the
/// disclosure control the shell toggles. Innermost wins, so a `<details>`
/// nested inside another one's summary opens the inner section.
```

## L1724 · `pub fn hit_test(&self, x: i32, y: i32) -> Option<&str> {`

```
/// Link href at a document-space point (caller adds scroll to screen y).
```

## L1726 · `self.links`

```
// Reverse so a link painted later (on top) wins an overlap.
```

## L1734-1743 · `pub fn element_chain(&self, x: i32, y: i32) -> Vec<u32> {`

```
/// Every element the pointer is inside, at a document-space point —
/// ascending `seq`, which is document order.
///
/// It is a LIST, not the innermost element: CSS hovers an element and all
/// its ancestors, which is what `nav:hover a` and every dropdown menu on
/// the web relies on. Containment does that for free — an ancestor's box
/// encloses its descendant's — without keeping a parent pointer per box.
/// Die `seq`-Kette unter dem Punkt — ALLE Elemente, nicht nur die
/// `:hover`-faehigen. Der Weg vom Klickpunkt zum Knoten fuer die
/// Ereigniszustellung; braucht `Engine::set_hit_all`.
```

## L1752-1768 · `v.extend(self.controls.iter()`

```
// Ein Steuerelement hat KEINEN `hover_box`: es ist ein atomarer
// Inline-Kasten, seine Kinder laufen nie durchs Layout, und damit
// kommt es nie an `record_inspect` vorbei. Ohne diese Zeile endet die
// Kette beim ELTERNTEIL — der Behandler eines `<button>` feuert nie,
// sein `onclick`-Attribut auch nicht, und `e.target` ist der falsche
// Knoten.
//
// Am Geraet sah das aus wie „der Klick kommt gar nicht an": vier
// `control-activate` und keine einzige Zeile von der Seite. Der
// host-seitige Selftest hatte es nicht gefunden, weil er die Kette
// aus dem BAUM baute statt aus dem Layout — `examples/hitchk.rs`
// schliesst genau diese Luecke.
//
// Bewusst hier und nicht in `record_inspect`: „darf der Zeiger diesen
// Kasten treffen" ist nicht „reagiert dieses Element auf `:hover`".
// Die zwei Fragen zusammenzulegen hat schon einmal sechs volle
// Layouts je Mausbewegung gekostet ([[feedback_hitting_is_not_hovering]]).
```

## L1781-1783 · `.filter(|b| b.pseudo == crate::css::PseudoElem::None && b.hoverable)`

```
// A pseudo-element's box is recorded for repainting, not for
// hit-testing — extending the pointer's reach would change which
// element it is inside, which is a different question.
```

## L1793-1795 · `pub fn hit_control(&self, x: i32, y: i32) -> Option<&ControlRect> {`

```
/// Form control at a document-space point. Checked BEFORE `hit_test` by the
/// shell: a control nested in a link (a search button inside an `<a>`) must
/// take the click itself.
```

## L1804 · `const MIDDLE_HALF_X: f32 = crate::style::BASE_FONT_PX * 0.25;`

```
// ── small font helpers (no_std: no f32::ceil) ──────────────────────────────
```

## L1806-1810 · `const MIDDLE_HALF_X: f32 = crate::style::BASE_FONT_PX * 0.25;`

```
/// Half the x-height that `vertical-align: middle` measures against (CSS2.1
/// §10.8.1 says the parent's, which is not threaded this far down). The line
/// SIZING and the PLACEMENT must use the identical value — with two different
/// approximations a middle-aligned box is sized into one line and painted
/// against another, and lands outside its own line box.
```

## L1817-1820 · `fn is_css_space(c: char) -> bool {`

```
/// The characters CSS collapses (css-text-3 §4.1.1: the "white space"
/// characters are space, tab and the newlines). Rust's `char::is_whitespace`
/// is the Unicode `White_Space` property, which also covers U+00A0 and U+3000 —
/// and both of those exist precisely so they do NOT collapse or offer a break.
```

## L1825-1829 · `fn is_hangable_space(c: char) -> bool {`

```
/// A character that HANGS at the end of a line: it is painted, but it does not
/// count towards the line's width (css-text-3 §4.1.3 phase II removes a
/// trailing sequence of collapsible spaces AND other space separators). These
/// are the space separators that do not collapse — U+00A0 is deliberately not
/// among them, since a no-break space is content.
```

## L1836-1837 · `fn is_zero_width_format(c: char) -> bool {`

```
/// A zero-width formatting character: it is not a typographic character unit,
/// so no letter-spacing is added after it (css-text-3 §8.2).
```

## L1844 · `pub fn is_zero_width_format_pub(c: char) -> bool { is_zero_width_format(c) }`

```
/// `is_zero_width_format` fuer den Rasterer — dieselbe Liste, nicht eine zweite.
```

## L1847 · `pub(crate) fn char_spacing(c: char, sp: (f32, f32)) -> f32 {`

```
/// The extra advance `sp` adds after `c`.
```

## L1852-1853 · `let ws = matches!(c, ' ' | '\u{00A0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}');`

```
// Word-spacing lands on the word separators css-text-3 §8.1 names —
// notably NOT U+3000 IDEOGRAPHIC SPACE.
```

## L1858-1863 · `pub fn measure_sp_pub(font: Face, s: &str, size: f32, sp: (f32, f32)) -> f32 {`

```
/// `measure_sp` fuer `select.rs` — dieselbe Rechnung, nicht eine zweite.
///
/// Die Textauswahl misst Praefixe, um vom Pixel aufs Byte zu kommen; ein
/// eigener Messweg dort waere garantiert um Bruchteile daneben, und genau
/// diese Bruchteile sind der Unterschied zwischen „das Zeichen unter dem
/// Zeiger" und dem daneben ([[feedback_intrinsic_shared_path]]).
```

## L1868 · `pub fn line_gap_pub(font: Face, size: f32) -> f32 { line_gap(font, size) }`

```
/// `line_gap` fuer `select.rs` — aus demselben Grund.
```

## L1872-1881 · `if font.ligatures().is_none() {`

```
// **Ein Formatierungszeichen hat KEINE Laufweite.** `is_zero_width_format`
// sagt seit je, welche das sind, wurde aber nur fuer `letter-spacing`
// gefragt — die Schrift wurde trotzdem nach einer Glyphe gefragt, und fuer
// ein Zeichen ohne Glyphe gibt sie die Breite von `.notdef` zurueck.
// Gemessen: zwanzig U+200C kamen auf 184 px statt 0, und zwanzig U+200B
// auf 1 px (je ein Bruchteil, der sich aufaddierte). DuckDuckGos Vorlage
// fuer „Searches related to" haengt ein `&ZeroWidthSpace;` hinter jeden
// Eintrag, und dieses eine Pixel brach den Text auf zwei Zeilen.
// The fast path is the one that runs: every embedded face is subsetted and
// carries no GSUB, so a page in the body font never allocates here.
```

## L1891-1894 · `fn measure_sp(font: Face, s: &str, size: f32, sp: (f32, f32)) -> f32 {`

```
/// `measure` plus `(letter-spacing, word-spacing)`. Letter-spacing lands after
/// EVERY character including the last — that is what an inline box measures as
/// in every engine, and the reftests are written against it. Word-spacing lands
/// on the word separator itself (css-text-3 §8.1: U+0020 and U+00A0).
```

## L1899-1903 · `if sp.0 != 0.0 || font.ligatures().is_none() {`

```
// **Letter-spacing suppresses ligatures**, and that is the spec, not a
// shortcut: `letter-spacing` separates typographic character units, and a
// ligature spanning several of them must be broken to make room
// (css-text-3 §8.2). Word-spacing does not — it lands on a separator that
// no ligature crosses.
```

## L1915-1917 · `adv + s[*at..*at + *n].chars().map(|c| char_spacing(c, sp)).sum::<f32>()`

```
// A ligature is one unit; the spacing of the run it covers is the
// separator spacing of its characters, which for a ligature is
// always zero (no ligature spans a space).
```

## L1922-1925 · `fn fit_prefix(font: Face, s: &str, size: f32, avail: f32, sp: (f32, f32)) -> usize {`

```
/// Byte length of the longest prefix of `s` that fits in `avail` px, snapped
/// back to a legal break. Returns 0 when not even the first cluster fits — the
/// caller decides whether to try a fresh line or force one through (never
/// returning 0 forever is the caller's job, not this function's).
```

## L1930-1932 · `for (g, at, _) in font.shape(s) {`

```
// The byte ranges are why `shape` reports them: this walks OFFSETS
// into the original string, and a ligature is indivisible — a break
// inside one would ask for half a glyph.
```

## L1955-1960 · `fn joins_back(c: char) -> bool {`

```
/// Does `c` bind to the character BEFORE it? Zero-width joiner sequences,
/// variation selectors, skin-tone modifiers, keycaps, combining marks,
/// regional-indicator pairs and tag sequences (the Wales/Scotland/England
/// flags) are all one user-perceived character, and
/// `word-break: break-all` may still not split one (css-text-3 §5.1 breaks
/// between grapheme clusters, not code points).
```

## L1970 · `fn cluster_boundary(s: &str, mut n: usize) -> usize {`

```
/// The largest legal break offset at or before `n`.
```

## L1983-1984 · `fn first_cluster(s: &str) -> usize {`

```
/// End of the first grapheme cluster in `s` — what a line that cannot fit even
/// one cluster is forced to take, so the loop always makes progress.
```

## L2000-2001 · `fn space_width(font: Face, size: f32, sp: (f32, f32)) -> f32 {`

```
/// The advance of the space BETWEEN two words. `sp` is the run's
/// `(letter-spacing, word-spacing)`: both apply to a word separator.
```

## L2012-2018 · `fn run_metrics(font: Face, size: f32, lh: f32) -> (f32, f32) {`

```
/// One inline run's contribution to its line box: `(ascent above the shared
/// baseline, box height)`. With `line-height: normal` these are the face's own
/// metrics — unchanged from before line-height existed. An explicit
/// line-height distributes its difference from the content height as
/// half-leading above and below the baseline (CSS 2.1 §10.8.1), so a value
/// under the content height legitimately yields a negative half and lets
/// consecutive lines overlap.
```

## L2029 · `#[derive(Clone, Copy)]`

```
// ── CSS counters (css-lists-3 §4) ───────────────────────────────────────────
```

## L2031-2034 · `#[derive(Clone, Copy)]`

```
/// One counter instance on the scope stack: its value, plus the tree DEPTH
/// (`path.len()`) of the element whose `counter-reset` created it — used to tell
/// an ancestor's counter (nest a new instance) from a sibling's (overwrite the
/// existing one).
```

## L2042-2048 · `#[derive(Default)]`

```
/// The scoped counter state threaded through the tree walk. A stack of
/// instances (innermost last); `counter()` reads the innermost value of a name,
/// `counters()` walks every in-scope instance of it (outermost first). Scope is
/// bounded by truncating the stack back to a saved length when a subtree's child
/// list ends (see `flow_children`/`collect_inline`), which implements the
/// "descendants + following siblings" scope of css-lists-3 §4.4 closely enough
/// for the content web.
```

## L2055-2059 · `fn enter(&mut self, st: &ComputedStyle, depth: usize) {`

```
/// Apply an element's `counter-reset` then `counter-increment` (spec order),
/// given its tree depth. Reset nests a new instance when the innermost
/// same-name counter belongs to an ancestor (shallower depth), else it
/// overwrites that instance's value (self/sibling). Increment auto-creates a
/// counter at 0 first if none is in scope.
```

## L2064 · `self.stack[i].value = value;`

```
// self or a sibling at the same level → overwrite in place.
```

## L2079 · `fn value(&self, name: u32) -> i32 {`

```
/// The innermost in-scope value of `name` (0 if none), for `counter()`.
```

## L2084 · `fn values(&self, name: u32) -> Vec<i32> {`

```
/// Every in-scope value of `name`, outermost first, for `counters()`.
```

## L2090 · `struct Ctx<'a> {`

```
// ── entry point + block/inline tree walk ───────────────────────────────────
```

## L2092-2095 · `struct Ctx<'a> {`

```
/// Per-layout mutable context: the shared inputs (font / theme / author sheet)
/// plus the accumulating display list and the live ancestor `path` (for
/// selector matching). Bundling these keeps the recursive walkers from carrying
/// a dozen arguments each.
```

## L2101-2103 · `hit_all: bool,`

```
/// Fuer JEDES Element einen Treffer-Kasten aufzeichnen. Ohne das gibt es
/// keinen Weg vom Klickpunkt zum Knoten — und nur Elemente mit einer
/// `:hover`-Regel haetten einen.
```

## L2105-2112 · `guessed: core::cell::RefCell<Vec<String>>,`

```
/// `src`s whose `<img>` box had to be GUESSED — no decoded pixels and no
/// `width`/`height` pair. Only for these does a later decode move the
/// page, so only their arrival justifies a re-layout.
///
/// A plain bool here was wrong: one image that never arrives (a 403, an
/// undecodable format) kept it true forever, so every later batch forced
/// a full re-layout even when all of ITS images had definite boxes. On a
/// real article that was 5.7 s of frozen UI per batch.
```

## L2114 · `inline_svgs: core::cell::RefCell<Vec<(u32, Rgb, u32, u32)>>,`

```
/// Inline `<svg>` render requests — see `Layout::inline_svgs`.
```

## L2116-2120 · `css_images: core::cell::RefCell<Vec<u64>>,`

```
/// `url_key`s of the CSS images this layout referenced. Deliberately a
/// SET (deduped on insert), not an append-only log: a throwaway
/// measurement layout paints boxes too, and its entries must be
/// indistinguishable from the real pass's rather than something the
/// measure helpers have to remember to roll back.
```

## L2125 · `filters: Vec<crate::color::ColorFilter>,`

```
/// `filter` transforms, deduped; an image op holds a 1-based index here.
```

## L2127-2128 · `forms: &'a FormState,`

```
/// Live form-control state (typed values, checked boxes, focus) — read
/// only; the shell owns it and re-lays out when it changes.
```

## L2130 · `path: Vec<ElemInfo<'a>>, // root → … → current parent`

```
// root → … → current parent
```

## L2131-2136 · `cb: PosCb,`

```
/// Positioned containing block (x, y, width, height) for
/// `position:absolute` descendants — the nearest ancestor with
/// `position != static`, else page. The height is `None` unless the
/// establishing box has an explicit one: abspos children are laid out
/// during the parent's child walk, so a content-derived height isn't
/// known yet. `top`/`bottom` percentages need it (CSS 2.1 §9.3.2).
```

## L2138-2139 · `cb_pend: Vec<PendingCbH<'a>>,`

```
/// Deferred containing-block heights, indexed by `cb.4`. A stack: it is
/// truncated when the box that pushed its recipe goes out of scope.
```

## L2141 · `viewport_w: f32,`

```
/// Viewport width (px) — the layout width — for `@media` evaluation.
```

## L2143-2144 · `viewport_h: f32,`

```
/// The viewport height this layout was built for — the initial containing
/// block's height, and the basis every `vh` resolved against.
```

## L2146-2147 · `floats: Vec<FloatRect>,`

```
/// Active floats in the current block formatting context — line boxes and
/// later blocks flow around them. Saved/restored when entering a new BFC.
```

## L2149-2157 · `stack_ops: Vec<(i32, i32, usize, usize)>,`

```
/// Recorded `(z_index, op_start, op_end)` / `(z_index, link_start,
/// link_end)` ranges for the **outermost** positioned boxes with an
/// explicit (non-`auto`) `z-index` — one contiguous slice of `ops`/`links`
/// per box (CSS2.1 §9.9). `layout()` stable-sorts by `z_index` at the end
/// so negative levels paint behind, positive ones in front, and everything
/// else (`auto`/untracked) keeps its in-order position.
/// `(z-index, paint layer, op_start, op_end)`. The layer separates the
/// sub-orders CSS2.1 Appendix E puts INSIDE one z-index: in-flow block
/// boxes paint below floats, and floats below positioned boxes.
```

## L2160-2163 · `float_ops: Vec<(usize, usize)>,`

```
/// Op / link ranges emitted by non-positioned floats. Kept apart from
/// `stack_ops` only because they carry no `z`: both lists are concatenated
/// at the end and `z_order` nests them, so a float inside a positioned box
/// is simply that box's child.
```

## L2166-2169 · `abs_count: u32,`

```
/// How many out-of-flow boxes have been laid out so far, split by whether
/// they escape a positioned ancestor. `overflow: hidden` compares these
/// across its content to see whether anything inside it left its clip's
/// jurisdiction (CSS2.1 §11.1.1) — see `clip_overflow`.
```

## L2172-2177 · `clear_floor: Option<(i32, Collapse, i32)>,`

```
/// Die Raeumung, die der Rufer gerade angewandt hat, fuer den EINEN
/// Kasten, der als naechstes ausgelegt wird: `(Anker davor, offener Rand
/// davor, geraeumte Oberkante)`. Nur der zweite Durchgang in
/// `flow_block_impl` braucht sie — eine Raeumung SETZT die Oberkante, also
/// muss ein spaeter gefundener Rand in die hypothetische Lage, nicht
/// obendrauf. Wird beim Eintritt genommen, damit kein Kind sie sieht.
```

## L2179-2184 · `cb_h: Option<f32>,`

```
/// The containing block's CONTENT height, when it is definite — what a
/// percentage `height`/`min-`/`max-height` resolves against (CSS2.1 §10.5).
/// `None` means the containing block's height depends on its content, and
/// then a percentage computes to `auto`. That fallback is the whole reason
/// this is an `Option`: `html { height: 100% }` is an everyday idiom, and
/// guessing a height for it truncates pages.
```

## L2186-2188 · `last_baseline: Option<i32>,`

```
/// Document y of the last line box's baseline emitted so far. An
/// `inline-block` aligns on the baseline of ITS last line box (CSS2.1
/// §10.8.1), which is only known once its content has been laid out.
```

## L2190-2195 · `stack_depth: u32,`

```
/// Depth of currently-open *tracked* (recorded) stacking ranges. Only a
/// box at depth 0 gets recorded — a z-indexed box nested inside another
/// already-tracked one paints as part of its ancestor's range instead
/// (full nested stacking contexts, e.g. an explicit `z-index` inside
/// another explicit `z-index`, are out of scope — sibling ordering is
/// the common case these reftests need).
```

## L2197-2199 · `abs_over_open_line: bool,`

```
/// Set for the duration of ONE `layout_abs` call: an out-of-flow box was
/// reached while a line box was still open. It has to sort above that
/// line's ops even though it was emitted first — see `LAYER_POSITIONED`.
```

## L2201 · `float_depth: u32,`

```
/// Nesting guard for float ranges, independent of `stack_depth`.
```

## L2203-2208 · `marker_ord: i32,`

```
/// The list counter for the `display:list-item` box about to be laid out.
/// `flow_children` owns one counter per child run and stamps it here right
/// before descending, so the marker code reads the right ordinal without
/// threading it through every box-layout signature. A nested list can't
/// clobber it: the inner list is laid out from inside the outer item's
/// children, i.e. after its marker was already emitted.
```

## L2210 · `counters: Counters,`

```
/// CSS counter state (`counter-reset`/`-increment`, read by `counter()`).
```

## L2212 · `inspect: bool,`

```
/// When set, element boxes are recorded into `inspects` for the dev tool.
```

## L2215-2217 · `hover: &'a [u32],`

```
/// `seq`s of the elements the pointer is currently inside, ascending.
/// Usually EMPTY, which is why every `ElemInfo` can afford to consult it:
/// the check is one `is_empty()` on a path walked ~30 000× per layout.
```

## L2219-2221 · `hover_boxes: Vec<HoverBox>,`

```
/// Element boxes the shell hit-tests on pointer movement. Only collected
/// when the sheet has `:hover` rules at all — a page without them must not
/// pay for a list nobody reads.
```

## L2223-2226 · `vh_used: core::cell::Cell<bool>,`

```
/// Did anything in this layout actually consume the viewport HEIGHT — a
/// `vh`/`vmin`/`vmax` length that won the cascade, or a box resolved
/// against the initial containing block? `Cell` because the style walk
/// runs behind `&self`.
```

## L2228-2230 · `vp_height_box: core::cell::Cell<bool>,`

```
/// Set while laying out a box whose definite height came from the viewport
/// (a `%` against the ICB). Read by `flow_children` right after the box,
/// which knows whether anything follows it to be pushed around.
```

## L2232-2235 · `intrinsic: BTreeMap<u32, (f32, f32)>,`

```
/// Memoised `intrinsic_width` results, keyed by element `seq`. Measuring a
/// subtree now cascades every descendant, and the same element is asked
/// repeatedly (a table sizes its columns over several passes) — without
/// this the cascade work would multiply.
```

## L2237-2239 · `measuring_cb_h: core::cell::Cell<bool>,`

```
/// Set while `measure_box_height` is resolving a positioned box's own
/// containing-block height. That measurement re-enters the same box, which
/// would ask for the same height again — one level is all the answer needs.
```

## L2241-2248 · `measured: core::cell::RefCell<BTreeMap<MeasureKey, i32>>,`

```
/// Memoised `measure_box_height` results — see `measured_h`. A speculative
/// measurement lays a whole subtree out and throws the result away, and
/// nested ones repeat: measuring a flex item that is itself a flex
/// container re-measures its items, and the enclosing box is measured
/// again for every level above it. On a real article that made the same
/// element's box run through layout 34 times on average and 256 times at
/// worst — powers of two, the signature of a doubling per nesting level.
/// This collapses that back to once per distinct question.
```

## L2250-2258 · `pseudos: core::cell::RefCell<BTreeMap<u64, Option<(Vec<crate::style::ContentPiece>, ComputedStyle)>>>,`

```
/// Memoised `style::resolve_pseudo` results — the SAME cascade work as
/// `styles`, for the `::before`/`::after` box, and it had no cache at all.
/// Measured under the interpreter it was 51 % of a whole layout: 62 340
/// calls for 2 316 elements, almost all of them searching the entire sheet
/// only to answer "this element generates nothing".
///
/// Only the CASCADE result is cached. The content template is rendered
/// fresh on every hit, because `content: counter(x)` depends on the counter
/// state at that point in the walk, not on the element.
```

## L2260-2268 · `segs: core::cell::RefCell<BTreeMap<u64, Vec<(u32, u32, bool)>>>,`

```
/// Memoised `segment_table_runs` results. The measure walk
/// (`intrinsic_walk`) and the layout walk (`flow_children`) segment the
/// SAME child lists independently, and each classification cascades the
/// child to read its `display` — measured, 82 % of the calls repeat a list
/// already segmented, and the classification is 25 % of a whole layout.
///
/// Keyed by the node slice's identity AND the ancestor chain, because the
/// cascade that decides a role reads the chain: the same `<div>` can be a
/// table row in one context and not in another.
```

## L2270-2274 · `styles: core::cell::RefCell<BTreeMap<u64, ComputedStyle>>,`

```
/// Memoised `style::resolve` results, keyed by a hash of everything the
/// cascade reads (see `style_key`) — so this is a pure cache, not a policy.
/// A real article cascades the SAME element about twelve times: every
/// throwaway measurement re-walks its subtree, and selector matching is
/// ~90 % of layout, so that multiplier is most of the cost of a page.
```

## L2276-2283 · `varmaps: core::cell::RefCell<BTreeMap<u32, alloc::rc::Rc<crate::vars::VarMap>>>,`

```
/// Die Custom Properties je Element, nach `seq`.
///
/// Sie stehen NICHT in `ComputedStyle`: der ist `Copy` und wird je
/// Element kopiert; eine `Rc` darin haette die ganze Layoutschicht
/// umgeworfen. Also laufen sie daneben — und weil eine Custom Property
/// eine geerbte Eigenschaft ist, braucht jedes Element den Eintrag seines
/// Elternteils. Wer selbst keine setzt, TEILT dessen Karte (`Rc`), sonst
/// koestete Bootstraps 200-Namen-Palette je Element eine Kopie.
```

## L2287-2295 · `fn style_key(el: &Element, parent: &ComputedStyle, ancestors: &[ElemInfo], prev: &[ElemInfo], sib_count: u32) -> u64 {`

```
/// Hash the inputs `style::resolve` actually depends on. Elements are
/// identified by `seq` rather than by content, which makes the whole ancestor
/// chain and sibling list a handful of integer mixes.
///
/// `parent` is not hashed in full — only the inherited values a cascade can
/// read back (font size, colour, weight, direction). The chain of ancestor
/// `seq`s already determines which element the parent IS; the fingerprint is
/// there for the call sites that hand a cell the table's style rather than the
/// row's, so those cannot collide with each other.
```

## L2321-2324 · `fn info(&self, el: &'a Element) -> ElemInfo<'a> {`

```
/// An `ElemInfo` that knows whether the pointer is inside this element.
/// Every construction inside the layout goes through here — a bare
/// `ElemInfo::of` would silently report "not hovered" and the page would
/// stay frozen under the pointer for exactly the elements it forgot.
```

## L2329-2341 · `fn contents_is_inline(&mut self, el: &'a Element, st: &ComputedStyle) -> bool {`

```
/// Does a `display: contents` element hold nothing but inline-level
/// content? Then its children belong in the line box the parent is already
/// building, and putting them anywhere else splits a line the reference
/// keeps whole (`P<fieldset style=display:contents>A…` is one word).
///
/// If ANY child is block-level the parent's flow has to break for it
/// regardless, and a transparent block — which is what `resolve` has
/// already made of this style, zero margins and all — lands the same
/// pixels while keeping the block/anonymous-block split intact.
///
/// Costs one style resolve per child, paid only for `display: contents`.
/// `styled` memoises on the same key the real walk uses, so the walk that
/// follows reads them back out of the map.
```

## L2354-2355 · `Display::Contents => self.contents_is_inline(ce, &cs),`

```
// Nested unboxing — `details, summary { display: contents }`
// is one element's contents inside another's.
```

## L2367-2373 · `fn note_vh(&self, s: &ComputedStyle) {`

```
/// `style::resolve` through the memo. Every cascade inside the layout goes
/// through here so a re-measured subtree costs a map lookup, not a full
/// selector match against the page's stylesheet.
/// Record a viewport-HEIGHT dependency that is unconditional. A `vh` cap
/// (`max-`/`min-height`) is NOT one: it only moves geometry when it
/// actually clamps, which `clamp_vh` decides once the content height is
/// known.
```

## L2380-2381 · `fn note_vh_clamp(&self, st: &ComputedStyle, before: i32, after: i32) {`

```
/// A `vh`-derived `max-height`/`min-height` that actually changed the used
/// height IS a viewport-height dependency; one that never binds is not.
```

## L2392 · `fn vars_of(&self, seq: u32) -> alloc::rc::Rc<crate::vars::VarMap> {`

```
/// Die Custom Properties, die ein Kind von `seq` erbt.
```

## L2410-2412 · `self.varmaps.borrow_mut().insert(el.seq, match own {`

```
// Wer selbst nichts setzt, teilt die Karte des Elternteils — dieselbe
// `Rc`, kein Kopieren. Der Eintrag muss trotzdem da sein, sonst faende
// ein Kind nichts und die Vererbung risse an dieser Stelle ab.
```

## L2422-2431 · `fn should_track_stack(&self, st: &ComputedStyle) -> bool {`

```
/// Whether `st` should open a new tracked stacking range right now: it is
/// positioned, has an explicit `z-index`, and isn't already nested inside
/// another tracked range.
///
/// **Every positioned box, at every depth.** Appendix E paints them in step
/// 8, after all the in-flow content of steps 3–7, and their own `z-index`
/// orders them against their SIBLINGS — which is why the ranges have to
/// nest (`z_order`). The older rule tracked only an explicit `z-index` at
/// the top level, because a flat list made a `position: relative` wrapper
/// swallow its children; nesting removes that reason.
```

## L2436-2438 · `fn stack_key(st: &ComputedStyle) -> (i32, i32) {`

```
/// Where a positioned box sorts: its own `z-index` (`auto` counts as 0, and
/// both land in Appendix E step 8 together), in the positioned layer so it
/// paints over the in-flow content of the same stacking context.
```

## L2446-2457 · `fn resolve_pct_box(st: &ComputedStyle, cb_w: f32) -> Option<ComputedStyle> {`

```
/// Resolve percentage `padding` and vertical `margin` against the containing
/// block's **width**, once, at the entry to laying the box out — the same
/// shape as `resolve_pct_heights`, so everything downstream keeps reading
/// plain pixels. Both axes take the width (CSS 2.1 §8.1, §8.3): a
/// percentage top padding is a fraction of the INLINE size, which is what
/// makes `padding-top: 56.25%` reserve a 16:9 box.
///
/// Before this the vertical ones fell to zero and the horizontal ones too —
/// `pad_*` is a resolved `f32`, and the cascade that fills it cannot see a
/// containing block. Only the `margin`s on the inline axis were `Len` and
/// so survived to layout, which is why `margin-left: 50%` worked and
/// `padding-left: 50%` did not.
```

## L2463-2465 · `let at = |p: f32| p / 100.0 * cb_w;`

```
// The stored px is the CONSTANT half of the value (`calc(10% + 5px)`
// keeps its 5px), so the percentage is added to it, not put in its
// place.
```

## L2482-2490 · `fn resolve_pct_heights(&self, st: &ComputedStyle) -> Option<ComputedStyle> {`

```
/// Resolve percentage `height`/`min-`/`max-height` against the containing
/// block ONCE, at the entry to laying the box out. Everything downstream
/// then matches on `Len::Px` exactly as before — which is the point: the
/// two earlier attempts at percentage heights each taught one code path to
/// resolve them and measured WORSE, because the other paths still read the
/// same box as `auto` and the two answers disagreed.
///
/// Returns `None` when nothing needs resolving, so the common case does not
/// copy a 1 kB `ComputedStyle`.
```

## L2497-2504 · `if cbh == Some(self.viewport_h as f32) {`

```
// A percentage height resolving against the viewport does NOT by itself
// move anything: `html, body { height: 100% }` is on nearly every site
// and only fixes those boxes' own bottom edge, with nothing after them.
// What moves content is a box like that having FOLLOWING content — so
// the box is only marked here, and `flow_children` raises the flag if
// something actually comes after it. Compared by value, not identity:
// an ancestor that happens to be exactly one viewport tall
// over-reports, which only costs us today's re-layout.
```

## L2508-2510 · `let one = |l: Len, auto: Len| match l {`

```
// §10.5: against an indefinite containing block a percentage behaves as
// `auto`. `min-height` is the exception the spec spells out — it falls
// back to 0, which is its initial value anyway.
```

## L2525-2526 · `fn record_stack_entry(&mut self, z: i32, layer: i32, op_start: usize, op_end: usize, link_start: usize, link_end: usize)`

```
/// Record one box's emitted `ops[op_start..op_end]` / `links[link_start..
/// link_end]` as its own stacking-order unit. Empty ranges are skipped.
```

## L2536-2538 · `fn record_inspect(&mut self, el: &Element, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, op0: usize) {`

```
/// Record `el`'s box `(x, y, w, h)` for the inspect dev tool. No-op unless
/// inspection is enabled, so the label formatting cost is only paid when the
/// user is actually inspecting.
```

## L2540-2547 · `let hoverable = self.sheet.hover_set.may_match(el);`

```
// Same call site, own switch: the pointer needs these boxes on any page
// with a `:hover` rule, whether or not anyone is inspecting — and a
// `<summary>` needs one whether or not the page hovers anything.
// ZWEI Fragen, und sie zusammenzulegen war ein Fehler mit Messwert:
// „darf der Zeiger diesen Kasten TREFFEN" ist nicht „reagiert dieses
// Element auf `:hover`". Mit `hit_all` bekam jedes Element
// `hoverable = true`, also galt jede Mausbewegung als Stilwechsel —
// auf Wikipedia sechs volle Layouts a 130 ms fuer nichts.
```

## L2610 · `pub(crate) fn display_name(d: Display) -> &'static str {`

```
/// A short name for a `Display` value, for the inspect label.
```

## L2626-2636 · `const LAYER_FLOAT: i32 = 1;`

```
/// Stable-reorder `items` so the tracked `(z_index, start, end)` ranges sort
/// by `z_index` (negative before, positive after), while every byte NOT
/// covered by a range — and any range at `z_index == 0` — keeps its original
/// relative position (a plain stable sort with untracked spans implicitly
/// keyed `0`). Ranges must be non-overlapping (guaranteed by `stack_depth`
/// gating at collection time).
/// Paint layers WITHIN one z-index (CSS2.1 Appendix E, steps 3 and 4): in-flow
/// block boxes, then non-positioned floats. Untracked spans of the display list
/// are in-flow content and take layer 0, which is what lifts a float above the
/// block backgrounds and borders emitted after it.
/// Non-positioned floats: above the in-flow block boxes around them.
```

## L2638-2651 · `const LAYER_POSITIONED: i32 = 2;`

```
/// An out-of-flow box that was emitted while a line box was still open.
///
/// Appendix E paints positioned boxes in step 8, after the in-flow inline
/// content of step 7 — but the display list is built in visit order, and a line
/// is not written until it BREAKS. So an abspos box reached mid-line lands in
/// the list ahead of text that precedes it in the document, and paints under it.
///
/// Flushing the line instead would be wrong: `foo<div style=position:absolute>
/// </div>bar` is one line, and breaking it early moves `bar`. So the box is
/// lifted over exactly that line and nothing else. Lifting positioned boxes
/// wholesale was measured twice and is worse both times: out-of-flow only gives
/// +25/-21 (`border-005` — an absolute box FIRST, a `position: relative` box
/// after it, both step 8, so document order must decide and lifting one of them
/// hands it to the loser), and every positioned box gives +16/-46.
```

## L2663-2677 · `fn z_order(len: usize, ranges: &[(i32, i32, usize, usize)]) -> Vec<usize> {`

```
/// The old display-list index of every op, in painting order — the whole of
/// the z-ordering, expressed once so `reorder_by_z` and `z_permutation` cannot
/// drift apart.
///
/// **The ranges NEST**, and that is the point of this shape. A stacking context
/// is a tree: `z-index` orders a box against its SIBLINGS inside its parent's
/// context, not against the whole page. A flat list cannot say that — the two
/// earlier attempts at painting positioned boxes in Appendix E order both had
/// a `position: relative` parent swallow its children's ranges, and both
/// measured worse (+21/−30 and +21/−57) for exactly that reason.
///
/// Ranges over one array are properly nested by construction: each is a
/// subtree's span, and two subtrees are either disjoint or contained. One that
/// straddles a sibling is dropped rather than trusted — a scrambled display
/// list is far worse than one box in the wrong layer.
```

## L2679-2680 · `let mut sorted: Vec<(i32, i32, usize, usize)> =`

```
// Parents first: by start ascending, then by end DESCENDING, so a range
// that contains another is seen before it.
```

## L2685 · `let mut nodes: Vec<(i32, i32, usize, usize, Vec<usize>)> = Vec::with_capacity(sorted.len());`

```
// `nodes[i] = (z, layer, start, end, kids)`; `roots` are the outermost.
```

## L2688 · `let mut open: Vec<usize> = Vec::new(); // stack of enclosing node indices`

```
// stack of enclosing node indices
```

## L2699 · `continue; // straddles its parent's end — not a subtree, drop it`

```
// straddles its parent's end — not a subtree, drop it
```

## L2711-2714 · `fn level(`

```
// One level: the untracked gaps between the children (in-flow content, key
// `(0, 0)`) and the children themselves, stable-sorted by `(z, layer)`.
// Stable is what keeps document order among everything that ties — which
// is the rule for two positioned boxes that both land in step 8.
```

## L2746-2749 · `fn z_permutation(len: usize, ranges: &[(i32, i32, usize, usize)]) -> Vec<usize> {`

```
/// Wohin `reorder_by_z` jeden Befehl legt: `perm[alt] == neu`.
///
/// Dieselbe Blockbildung wie dort, nur mit Indizes statt Werten — damit
/// koennen Nebentabellen, die auf Befehle zeigen, mitgezogen werden.
```

## L2761-2765 · `fn remap_span(perm: &[usize], at: usize, len: usize) -> Option<usize> {`

```
/// Die neue Startstelle einer Spanne — oder `None`, wenn die Umsortierung sie
/// ZERRISSEN hat.
///
/// Ein zerrissener Bereich ist nicht am Stueck ersetzbar; der Schnellweg muss
/// ihn dann ablehnen, statt fremde Befehle zu ueberschreiben.
```

## L2779 · `pub fn layout(`

```
/// Lay a document out into a scroll-independent display list.
```

## L2793-2794 · `let mut initial = ComputedStyle::root(theme);`

```
// The root element is never painted, but `html { … }` still cascades into
// the document — and its `font-size` is the basis for every `rem`.
```

## L2796-2797 · `initial.vw = width as f32;`

```
// Seed the viewport before the first cascade: `vw`/`vh` on `html` itself
// have to resolve, and every descendant inherits these two down.
```

## L2822-2827 · `cb: (0, 0, width as i32, Some(viewport_h as i32), None),`

```
// Initial containing block: the viewport, anchored at the CANVAS
// origin (CSS2.1 §10.1) — not at the page's content box. `left: 100px`
// on a box with no positioned ancestor means 100px from the window
// edge, whatever inset the page content sits at. Its height is
// definite, which is what makes `top:0; bottom:0` on a root-level
// abspos box stretch to the window rather than collapse.
```

## L2862-2863 · `ctx.varmaps.borrow_mut().insert(html_el.seq, alloc::rc::Rc::new(root_vars));`

```
// Die Wurzelpalette. `:root{--bs-…}` ist die Karte, aus der alles andere
// liest — ohne diesen Eintrag erbt niemand etwas.
```

## L2866-2867 · `let body = dom.body();`

```
// Resolve <body> for the canvas-background rule below; layout reaches it
// as an ordinary child of the root.
```

## L2871-2872 · `let body_inherited = ctx.vars_of(html_el.seq);`

```
// Auch der Rumpf erbt die Wurzelpalette — er wird hier fuer die
// Leinwandfarbe aufgeloest, also ausserhalb des Baumlaufs.
```

## L2882-2894 · `let mut y;`

```
// The ROOT ELEMENT IS A BOX. It used to be skipped — layout started at
// `<body>`'s children, inside a hardcoded 20px page inset — so `html
// { position: absolute }`, its border, its width and `<body>`'s own margin
// all meant nothing. Laying it out like any other block is what makes the
// whole `abspos-containing-block-initial` family measurable, and it is
// where the page inset now comes from: `<body>`'s UA margin.
// NOTE: 0.3.13 resolved a percentage `height` on the root against the
// viewport here — the ICB's height IS definite, so it looked right. It was
// measured OUT again in 0.3.14: it fixed none of the two tests it was
// added for, cost `abspos-containing-block-006`, and truncated every page
// that writes the everyday `html { height: 100% }` to one viewport, which
// stopped scrolling dead. Percentage heights belong with general
// percentage-height support, not as a special case for the root.
```

## L2897-2899 · `y = 0;`

```
// `html { display: none }` — the root generates no box, so the document
// renders nothing at all (`root-box-003`). Only the canvas keeps its
// propagated background.
```

## L2902 · `ctx.path.push(ctx.info(body));`

```
// A document with no `<html>` at all: the synthetic container is both.
```

## L2908-2909 · `ctx.layout_abs(html_el, &root, cx, 0);`

```
// An out-of-flow root resolves against the ICB like any other
// out-of-flow box — it just has no in-flow position to fall back to.
```

## L2917 · `let float_bottom = ctx.floats.iter().map(|f| f.bottom).max().unwrap_or(0);`

```
// A float can extend below the last in-flow line — grow the page to contain it.
```

## L2920-2924 · `let painted_bottom = ctx.ops.iter().map(op_bottom).max().unwrap_or(0);`

```
// The page's scrollable height is how far the PAINTED content reaches, not
// where the root box ends. `html { height: 100% }` is an everyday idiom and
// it makes the root box exactly one viewport tall — everything below it
// still scrolls in every browser. Taking the root's border-box bottom alone
// truncated such a page to the window and killed scrolling outright.
```

## L2928-2935 · `let canvas_bg = root.bg.or(body_style.bg).map_or(theme.bg, |c| c.over(theme.bg));`

```
// The body's background propagates to the whole canvas (a bare `<body
// background>` fills the viewport, not just the body box).
// Canvas background (CSS 2.1 §14.2): the ROOT element's background is
// propagated to the canvas; `<body>`'s is used only when the root's is
// transparent. Honouring `html { color }` without this paints white text
// on a white canvas for every "this page should be green" reftest.
// The canvas is the ground: a translucent body background has nothing
// under it but the theme, so it is flattened here rather than at paint.
```

## L2937-2939 · `#[cfg(feature = "diag-boxes")]`

```
// z-index stacking order (CSS2.1 §9.9 / Appendix E): reorder the flat,
// tree-order display list so negative-z ranges paint first (behind) and
// positive-z ranges paint last (in front) of everything else.
```

## L2945-2951 · `let float_range = |v: &Vec<(usize, usize)>| -> Vec<(i32, i32, usize, usize)> {`

```
// Floats are ordinary nodes in the stacking tree now: a float inside a
// positioned box is simply its child, and `(0, LAYER_FLOAT)` sorts it after
// that box's own in-flow content (Appendix E step 4 after step 3).
// `split_float_ranges` used to CUT the enclosing range around each float,
// which the flat list needed and the tree actively breaks: the cut pieces
// become SIBLINGS, so the float sorted ahead of the very background it sits
// on and a red container painted over its own green children.
```

## L2959-2961 · `let perm = z_permutation(ctx.ops.len(), &op_ranges);`

```
// Die Umsortierung nach z verschiebt ganze Bloecke — und damit auch die
// Spanne, die ein Steuerelement fuer sich notiert hat. Erst die Abbildung
// alt -> neu, dann die Befehle UND die Spannen damit umschreiben.
```

## L2989-2990 · `fn float_band(&self, top: i32, bot: i32, cl: i32, cr: i32) -> (i32, i32) {`

```
/// Narrow an x-range `[cl, cr]` by any active floats overlapping the
/// vertical band `[top, bot)`. Returns the (left, right) available there.
```

## L2995-2998 · `fn avoid_floats_bfc(`

```
/// Position a block that establishes a new BFC so its border box does not
/// overlap active floats (CSS2.1 §9.5): shift it into the widest available
/// band at its top, dropping below any float a definite width can't fit
/// beside. Returns the adjusted (margin-box left, available width, top).
```

## L3001-3002 · `el: Option<&'a Element>,`

```
// `None` for an ANONYMOUS box: there is no element to lay out twice,
// so it keeps the first-row placement.
```

## L3014 · `let frame = st.pad_left + st.pad_right + st.border_x();`

```
// Outer (margin-box) width the box demands.
```

## L3017-3022 · `Len::Auto | Len::Intrinsic(_) => Some(ceil_i32(ml + mr + frame)),`

```
// `auto` shrinks into the band — but the MARGINS and the frame do
// not shrink with it. If they alone do not fit, the border box
// would sit inside the float, which §9.5 forbids a BFC root, so
// the box goes below instead. Treating auto as "always fits" put
// a `margin-left` wide enough to clear the float straight on top
// of it.
```

## L3046-3052 · `for _ in 0..8 {`

```
// The whole BORDER BOX has to clear the floats, not just its first row
// (CSS2.1 §9.5): a float whose top is BELOW this box's top still
// overlaps it, and the box has no way to narrow partway down. Its
// height is only known by laying it out, so the candidate position is
// measured and the box dropped past whatever cuts into it. Bounded,
// because each retry starts below one more float bottom and a page can
// stack a lot of them.
```

## L3072 · `fn clear_below(&self, clear: ClearKind, y: i32) -> i32 {`

```
/// The y at or below which floats on the cleared side(s) no longer intrude.
```

## L3089-3094 · `fn place_float(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) {`

```
/// Place a `float:left|right` box (CSS2.1 §9.5.1). Computes the float's
/// margin-box width (shrink-to-fit for `auto`), finds the highest position
/// where that margin box fits beside earlier floats on either side (dropping
/// below the ones it can't fit beside), lays the box out isolated in its own
/// BFC, and records its margin box as an exclusion rect. Does not advance
/// normal flow. `x`/`w` are the BFC content box; `y` the static flow top.
```

## L3097-3105 · `let ml = st.margin_left.px(w as f32).unwrap_or(0.0);`

```
// **Ein Rand darf negativ sein, auch an einem Float** (CSS 2.1 §9.5 —
// §8.3 nimmt Floats von nichts aus). Hier stand `.max(0.0)`, und weil
// `layout_box` unten denselben Rand UNGEKUERZT wieder abzieht, wuchs
// der Kasten um genau den Betrag: Bootstraps `.form-check-input`
// (`float:left; margin-left:-1.5em` in einem `padding-left:1.5em`) kam
// 60 statt 20 px breit heraus, und mit einem eigenen
// Formatierungskontext daneben rutschte der ELTER um dieselben 40 px.
// Das ist die Bauweise jeder Checkbox und jedes Radioknopfes in
// Bootstrap.
```

## L3109-3110 · `let content_w = match st.width {`

```
// Content width: shrink-to-fit for `auto` (min(max(min-content, avail),
// preferred)); a definite width is used directly (may overflow the CB).
```

## L3127-3131 · `let fw = ceil_i32(content_w + pad_border + ml + mr);`

```
// Margin-box outer width (never the whole CB for a shrink-to-fit float,
// but a definite width may exceed the CB). Negative margins can pull it
// below zero — `layout_box` and `record_inspect` need that true value,
// while the float BAND keeps the old floor of 1px so that a float never
// reserves nothing at all.
```

## L3133-3136 · `let band_w = fw.max(0);`

```
// Ein Randkasten, den negative Raender auf null oder darunter ziehen,
// belegt NICHTS — Chromium laesst den naechsten eigenen
// Formatierungskontext daneben bei x = 0 stehen, nicht einen Pixel
// weiter rechts.
```

## L3138-3143 · `let pct = |l: Len| matches!(l, Len::Pct(_) | Len::Calc { .. });`

```
// **A percentage width would resolve a SECOND time below.** `layout_box`
// is handed `fw` — the float's OWN margin-box width — as its containing
// block, which is the contract for a shrink-to-fit float and a trap for
// `width: 50%`: it came out half of half. A 300px container gave a 75px
// float where every browser gives 150, and the two-column `float:left;
// width:50%` idiom is as old as CSS. Same for the two bounds.
```

## L3163-3168 · `let mut fy = self.clear_below(st.clear, y).max(y);`

```
// Float margins never collapse: the margin box top is the static flow
// position `y`. `clear` applies to floats as well (CSS2.1 §9.5.2), so
// first drop below every earlier float on the cleared side — without
// it Wikipedia's `clear:right` article thumbnails wedge in beside the
// infobox instead of below it, squeezing the text to a few characters
// per line. Then drop further until the margin box actually fits.
```

## L3193 · `let mbox_left = if is_left { bl } else { (br - band_w).max(bl) };`

```
// Margin-box left edge: left floats pack left, right floats pack right.
```

## L3195 · `let border_top = fy + st.margin_top as i32;`

```
// The border box sits below the margin box top by `margin-top`.
```

## L3198 · `let saved = core::mem::take(&mut self.floats);`

```
// The float's own contents establish a new BFC — isolate its inner floats.
```

## L3200-3201 · `let op0 = self.ops.len();`

```
// `layout_box` re-adds margin-left + padding from `mbox_left`; passing the
// margin-box width lets an `auto`-width child fill the shrink-to-fit box.
```

## L3203-3209 · `let ctl_st;`

```
// Dieselbe Vorabaufloesung wie im Blockweg: ein Steuerelement nimmt in
// `layout_box_inner` die uebergebene Breite als GEGEBEN und legt seine
// eigenen Raender NICHT wieder drauf. Der Float-Vertrag ist aber
// „Randkasten hier, Rand legt `layout_box` an" — also den Rand hier
// anlegen und den RANDkasten uebergeben. Ohne das sass Bootstraps
// `.form-check-input` (`float:left; margin-left:-1.5em`) auf der
// Polsterkante statt am linken Rand, und die Beschriftung daneben.
```

## L3233-3239 · `fn layout_children(&mut self, nodes: &'a [Node], parent: &ComputedStyle, owner: Option<&Element>, x: i32, w: i32, y0: i3`

```
/// Lay `nodes` as an independent block formatting context (a table cell,
/// grid item, the page root, …). Returns the y below the last child, with
/// the last in-flow block's bottom margin committed — margins do not
/// collapse out of an established BFC. `owner` is the element these nodes
/// belong to, for `::before`/`::after` generated content — `None` for an
/// anonymous box (CSS2.1 §17.2.1 table objects): an anonymous box has no
/// source element, so it cannot be selected and cannot generate one.
```

## L3245-3249 · `fn flow_children(`

```
/// Block formatting: lay `nodes` as a vertical stack, grouping consecutive
/// inline-level content into line boxes and collapsing adjoining vertical
/// margins (CSS2.1 §8.3.1). `anchor_y` is the collapse edge at entry (the
/// bottom of the previous content); `incoming` is any margin already open
/// there (e.g. a parent's top margin collapsing into its first child).
```

## L3260 · `let mut anchor = anchor_y; // bottom of last committed content`

```
// bottom of last committed content
```

## L3261 · `let mut open = incoming; // adjoining margin not yet committed`

```
// adjoining margin not yet committed
```

## L3265-3267 · `let counter_base = self.counters.stack.len();`

```
// Counter scope: any counter a child of this run resets lives until this
// child list ends (its descendants + following siblings). Truncate the
// stack back to here on the way out (css-lists-3 §4.4 scope boundary).
```

## L3270-3271 · `let mut vp_pending = false;`

```
// A viewport-derived-height child is waiting to see whether anything
// follows it in this flow.
```

## L3273-3276 · `if let Some(owner) = owner {`

```
// `owner::before` — an anonymous inline box carrying its `content`
// string, inserted ahead of `owner`'s real children (CSS2.1 §12.1).
// An anonymous `owner` (a table object with no source element) can't
// be selected, so it can't generate one.
```

## L3284-3285 · `let mut siblings: Vec<ElemInfo> = Vec::new();`

```
// Preceding element siblings (document order) for `+`/`~` combinators,
// and the total element-sibling count for `:nth-child`/`:last-child`.
```

## L3288 · `let mut list_ord: i32 = owner`

```
// `<ol start="n">` seeds the list counter; the first item lands on `n`.
```

## L3296-3300 · `let segs = self.segment_table_runs(nodes, parent);`

```
// A run of `table-row`/`-row-group`/`-header-group`/`-footer-group`/
// `-cell` siblings found here (not already inside table/row layout)
// has no `table` ancestor: CSS2.1 §17.2.1 wraps the whole run in one
// anonymous `table` box rather than laying each part out as an
// ordinary block.
```

## L3335-3337 · `self.counters.enter(&st, self.path.len());`

```
// Apply this element's counter-reset/-increment before laying it
// out, so its `::before`/`::after` and descendants see the updated
// values. Depth = its ancestor count (it is not yet on `path`).
```

## L3340 · `list_ord = el`

```
// `<li value="n">` restarts the counter at n (HTML §4.4.8).
```

## L3347-3352 · `if st.display == Display::Contents {`

```
// `display: contents` generates no box: the children go where this
// element's box would have been. Inline-level content joins the
// line box already open here; anything else takes the block path
// below, where the style `resolve` stripped makes the box it
// builds transparent — zero margins, no border, no background,
// `width: auto` — so it neither paints nor moves its children.
```

## L3354-3359 · `if !st.pre && self.contents_is_inline(el, &st) {`

```
// `white-space: pre` is a whole-BOX path here (`layout_pre`
// owns the element's text, newlines and all), so an unboxed
// element would never reach it and its source line breaks
// would collapse. The transparent block is where that path
// still runs — and it is what a bare `pre` block renders as
// anyway, so this loses nothing the inline route would give.
```

## L3368-3371 · `if el.tag == "img" || el.tag == "svg" {`

```
// `<img>` is an atomic inline box: add it to the current inline run
// (a lone `<img>` flows as one item → its own line; an `<img>` in an
// `<a>`/`<span>` flows with the text). Nested imgs are handled in
// `collect_inline`; this catches direct children of any display.
```

## L3373-3378 · `if matches!(st.position, Position::Absolute | Position::Fixed) {`

```
// Out of flow FIRST, exactly as the control branch below does.
// This branch matches on the TAG, so the blockification in
// `styled` does not route an abspos image past it the way it
// does every other replaced element — it landed on the line and
// grew the page by its own height. Found via Wikipedia's 1×1
// autologin pixel; a 40×40 overlay image cost 40px.
```

## L3397-3400 · `if st.display == Display::Inline && replaced_intrinsic(el).is_some() {`

```
// Every other replaced element is an atomic inline box too, and one
// that lays out through the block model — same as an `inline-block`,
// which is what `inline_block_box` builds. (A floated or out-of-flow
// one was blockified in `styled`, so it never reaches here.)
```

## L3407-3410 · `if let Some(kind) = crate::forms::kind_of(el) {`

```
// Form controls are atomic inline boxes too — and their children
// (a `<button>`'s label, a `<select>`'s options) never lay out as
// page content. Same treatment in `collect_inline`, since most
// controls sit inside inline context.
```

## L3415-3419 · `if matches!(st.position, Position::Absolute | Position::Fixed) {`

```
// An absolutely-positioned control is out of flow, like any
// other abspos box — the checkbox-hack toggle overlay
// (`position:absolute; width:100%; height:100%; opacity:0`)
// must NOT advance the line, or its full-size box inflates
// the container by the whole page height.
```

## L3426-3447 · `let centred = matches!(st.margin_left, Len::Auto)`

```
// A control the page made BLOCK-LEVEL falls through to the
// block path below, which paints it without a line box.
//
// An atomic inline sits on the baseline, so its parent comes out
// the control's height PLUS the descender — 2px on a 32px field.
// That is what doubles the bottom rule of a search box whose
// wrapper is pulled onto the group's border with `margin: -1px`:
// ours drew the field's edge 2px above the group's, where every
// browser has them coincide.
//
// Gated on a definite width, because this path takes the
// caller's width: `display:block` on a control means full width
// only when the page also asked for it, which the `display:block;
// width:100%` idiom (Codex, Bootstrap) always does. A bare
// block-level control keeps its intrinsic width as an inline.
//
// Die AUSNAHME ist `margin: auto`. Ein Kasten, der links und
// rechts `auto` sagt, will mittig stehen — und das kann nur
// ein Block, kein Inline (an einem Inline rechnet `auto` zu
// null). Die Fritzbox setzt ihren Anmeldeknopf genau so:
// `display:block; margin:auto; min-width:18.5rem`, und bei uns
// klebte er 162 px zu weit links.
```

## L3452-3460 · `if !block_level && st.float == FloatKind::None {`

```
// **Ein GEFLOTETES Steuerelement ist aus dem Fluss**, genau wie
// das absolut positionierte zwei Zweige weiter oben — es
// gehoert in den Float-Zweig unten, nicht auf die Zeile. Ohne
// diese Bedingung verschluckte der Steuerelement-Zweig den
// Float, und zwar nur bei AUTOMATISCHER Breite: mit einer
// erklaerten fiel es durch `block_level` hindurch und floss.
// Deshalb sah es wie ein Breitenfehler aus. Auf DDGs
// Wissenskasten klebte so der „Directions"-Knopf links vor dem
// Titel, statt rechts neben ihm zu stehen.
```

## L3469-3470 · `if matches!(st.position, Position::Absolute | Position::Fixed) {`

```
// `position:absolute`/`fixed` are out of flow → laid at a
// containing-block-relative position, not advancing the flow.
```

## L3479-3480 · `if st.float != FloatKind::None {`

```
// `float:left|right` — out of normal flow, placed at the current
// flow edge; following inline + blocks flow around it.
```

## L3482-3498 · `let positioned = st.position != Position::Static;`

```
// The float's margin-box top is its STATIC position, which is
// below the margin still open from the preceding block — a
// float doesn't collapse with it, but it doesn't ignore it
// either. `open` stays untouched: the float is out of flow, so
// the next in-flow block still collapses through it.
// A float paints ABOVE the in-flow block boxes around it
// (Appendix E steps 3/4). Recording its range is what stops a
// later sibling's border — MediaWiki's `div.mw-heading` rule,
// say — from being drawn across it. `stack_depth` bounds the
// NESTING (a float inside a float is covered by the outer one),
// not whether we record at all: the enclosing z-index range, if
// any, gets cut around this one at the end.
// A float that is ALSO positioned is a POSITIONED box — Appendix
// E step 8, not the float step 4. `float:left;
// position:relative` beside an absolutely positioned sibling
// must win on document order; recorded as a float it lost to
// one that precedes it (`anonymous-boxes-001`).
```

## L3532-3533 · `if !inline.is_empty() {`

```
// Block-level, in normal flow. Flush pending inline content first —
// a line box separates margins, so the open margin commits here.
```

## L3545-3551 · `let mut had_clearance = false;`

```
// `clear` introduces clearance, dropping the block below the floats
// and separating margins. §9.5.2 measures against the box's
// HYPOTHETICAL position — where its border top edge would sit with
// `clear: none`, so with its own top margin already collapsed in —
// and then clearance SETS that edge: the margin is consumed, not
// added on top of it. Clearing against the bare anchor instead put
// every cleared box one whole top margin too low.
```

## L3563-3573 · `if !committed {`

```
// Clearance stops the top margin collapsing through, so the
// container's border box stays where the flow put it — the
// cleared box adds HEIGHT below, it does not drag the whole
// container down. **Dieselbe Regel, die acht Zeilen
// weiter unten fuer ein `::after` mit `clear` schon
// steht** — sie fehlte hier, und damit war der
// gewoehnliche Clearfix
// (`<div style="clear:both"></div>` als letztes Kind)
// wirkungslos: der Kasten bekam die Hoehe des geraeumten
// Kindes statt der des Floats, weil sein eigener Rand
// mit heruntergezogen wurde.
```

## L3578-3580 · `anchor = cleared - own;`

```
// `flow_block_impl` re-adds the top margin to the anchor it
// is handed, so hand it the one that lands the border edge
// exactly on `cleared`.
```

## L3589-3591 · `let track = self.should_track_stack(&st);`

```
// An explicit `z-index` on a positioned (relative/sticky) box
// opens a tracked stacking range (CSS2.1 §9.9), same as abspos —
// unless already nested inside another tracked range.
```

## L3596-3603 · `let out = if establishes_bfc(&st) || crate::forms::kind_of(el).is_some() {`

```
// A block that establishes a new BFC (flex/grid/table) keeps its
// border box clear of active floats (CSS2.1 §9.5) and does not
// collapse its margins with its children; its top margin still
// collapses with the preceding flow, its bottom margin stays open.
// A form control is atomic: it takes the box-making path so
// `layout_box` paints it as a CONTROL. Without this a block-level
// control fell into `flow_block_impl` and was laid out as an
// ordinary block — CSS border, no face, no value, no placeholder.
```

## L3609-3618 · `let mut cst = st;`

```
// **Ein blockweites Steuerelement ist ein ersetzter Blockkasten**
// (CSS 2.1 §10.3.4): seine eigene Breite und seine eigenen
// Raender entscheiden. `layout_box_inner` nimmt die uebergebene
// Breite fuer ein Steuerelement als GEGEBEN — das ist der
// Vertrag der Flex-, Raster- und Zellenwege, wo der Rufer den
// Kasten schon aufgeloest hat. HIER ist `bw` die Breite des
// Umgebungskastens, und ohne diesen Schritt malte
// `display:block; width:100px; margin-left:50px` ueber die
// ganze Zeile und auf x = 0. Beide `auto`-Raender bleiben
// stehen: die mittige Lage loest `layout_box_inner` selbst auf.
```

## L3644-3650 · `let _ = op0;`

```
// **Hier NICHT aufzeichnen.** `layout_box` tut es selbst, und
// zwar mit dem eigenen Randkasten des Elements. Zwei Eintraege
// zu derselben `seq` sind kein Fehler (ein Inline-Kasten hat
// ein Fragment je Zeile), aber `getBoundingClientRect` gibt
// ihre VEREINIGUNG — und die aus 256 und 1902 ist 1902. Genau
// so las sich jedes `overflow:hidden` mit fester Breite: ein
// Tailwind-`w-64` meldete die Fensterbreite.
```

## L3661-3666 · `self.record_inspect(el, &st, o.box_x, o.top_y, o.box_w, o.bottom - o.top_y, op0);`

```
// The box's OWN border box. Reporting the containing
// block's `x`/`w` here made every device report about a
// centred or max-width container wrong: MediaWiki's
// `.mw-page-container` (max-width 99.75rem, margin 0 auto)
// paints 1596 px wide at x=162 and was reported as
// 1920 wide at x=0.
```

## L3674-3677 · `if self.vp_height_box.replace(vp_mark) && !out.through {`

```
// This child's height came from the viewport. That only MOVES
// anything if content follows it in this flow — `html, body
// { height: 100% }` has nothing after it, a mid-page `height: 50vh`
// banner has everything after it.
```

## L3681-3682 · `self.vh_used.set(true);`

```
// Something committed after such a box: its bottom edge, and so
// this content's position, tracks the viewport height.
```

## L3686 · `if st.position == Position::Relative {`

```
// `position:relative` stays in flow but its paint shifts by top/left.
```

## L3693-3694 · `let (tdx, tdy) = translate_offset(&st, out.box_w, out.bottom - out.top_y);`

```
// `transform: translate(...)` — the same paint-time shift, but its
// percentages are of the BOX, not the containing block.
```

## L3705 · `open = out.open;`

```
// Nothing committed: the box's margins stay adjoining.
```

## L3707-3708 · `if had_clearance { open_sealed = true; }`

```
// Aber wenn DIESER Kasten geraeumt wurde, endet die
// Verschmelzung am Elter (§8.3.1).
```

## L3717-3719 · `open_sealed = false;`

```
// Ein festgeschriebener Kasten faengt einen neuen
// Schlussrand an — das Siegel gilt nur fuer den, der von
// dem geraeumten Element kam.
```

## L3723-3729 · `if let Some(clear) = owner.and_then(|o| self.pseudo_clear(o, parent, PseudoElem::After)) {`

```
// A generated `::after` carrying `clear` is BLOCK-level: the open line
// closes before it and it takes clearance like any other block. This is
// the clearfix idiom — `.cw::after { content: ""; display: block;
// clear: both }` — how a very large part of the real web makes a
// container contain its floats. The box is zero-sized by definition, so
// `pseudo_box` below drops it; what matters is that the content edge
// follows it down past the floats. On a line box `clear` means nothing.
```

## L3743-3745 · `if !committed {`

```
// Clearance stops the top margin collapsing through, so the
// container's border box stays where the flow put it — the cleared
// box adds HEIGHT below, it does not drag the whole container down.
```

## L3756-3758 · `if let Some(owner) = owner {`

```
// `owner::after` — appended behind the real children, before the
// final line-box flush so it shares a line with trailing inline
// content (or starts its own, if the last child was block-level).
```

## L3776 · `self.counters.stack.truncate(counter_base);`

```
// Leave the counter scope this child list opened.
```

## L3781-3785 · `fn pseudo(&self, owner: &Element, own: &ComputedStyle, kind: PseudoElem) -> Option<(String, ComputedStyle)> {`

```
/// `owner`'s `::before`/`::after` generated box, if `owner`'s own cascade
/// (already resolved as `own`) has a matching rule with a supported
/// `content` string. `self.path`'s last entry is always `owner` itself at
/// every call site (the uniform `path.push(ElemInfo::of(el))` before any
/// box-laying call), so its ancestors are everything before that.
```

## L3788-3790 · `(ps.display == Display::Inline).then_some((text, ps))`

```
// Only a plain inline generated element is a text run. A box-shaped one
// is `pseudo_box`'s job, and anything else (`display: none`, the
// table-internal roles) produces nothing at all.
```

## L3794-3796 · `fn pseudo_clear(&self, owner: &Element, own: &ComputedStyle, kind: PseudoElem) -> Option<ClearKind> {`

```
/// The `clear` on `owner`'s generated box, when it has one that takes part
/// in the flow. `None` for a text-only pseudo (`clear` needs a block box),
/// an out-of-flow one (it clears nothing) or no generated box at all.
```

## L3807-3808 · `let key = style_key(owner, own, &self.path[..anc], &[], 0) ^ ((kind as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));`

```
// Same inputs the cascade reads, plus which pseudo-element is asked
// about — `prev`/`sib_count` are constant here, so they add nothing.
```

## L3821-3830 · `fn place_abs_pseudos(&mut self, el: &Element, st: &ComputedStyle, bx: i32, by: i32, bw: i32, bh: i32) {`

```
/// Place an out-of-flow `::before`/`::after` now that its originating box's
/// geometry is known. Its containing block is that box's PADDING box, so
/// this can only run once the box is finished — which is why it hangs off
/// the end of the block and flex paths rather than the child walk. Only for
/// a POSITIONED owner: for a static one the containing block is some
/// ancestor, and this box is not it.
///
/// This is how a page underlines its active tab —
/// `a::after { position: absolute; bottom: 0; left: 0; width: 100%;
/// height: 2px }` — and how MediaWiki hangs the magnify icon off a thumb.
```

## L3867-3869 · `let x = match (ps.left.px(aw), ps.right.px(aw)) {`

```
// `left` wins over `right`; with neither the box sits at the
// containing block's start edge (§10.3.7 with a static position we
// do not track for generated content).
```

## L3880-3884 · `if self.sheet.hover_set.may_match(el) {`

```
// A pointer rule can reach this box (`a:hover::after` is how
// MediaWiki underlines the article tabs), and repainting it needs
// a rectangle. It goes in AFTER whatever the element has painted
// so far — which, when the box paints nothing at rest, is the only
// thing there is to name it by.
```

## L3931-3941 · `fn kid_intrinsic(&mut self, kid: &Kid<'a>, s: &ComputedStyle) -> (f32, f32) {`

```
/// The finished rectangle of a `::before`/`::after` that carries a box of
/// its own — the CSS-icon idiom, `content: ""` plus a size plus a
/// `background-image`. Every layout path can place one of these: an inline
/// run puts it on a line like an `inline-block`, a flex container reserves
/// it at the start (or end) of its main axis.
///
/// `width`/`height` come from the style when definite; otherwise the text
/// decides, as for any shrink-to-fit box. Percentages resolve against
/// `avail_w`.
/// Die eigenen Breiten eines Flex-Kindes. Ein anonymer Kasten hat sie
/// schon — er wurde beim Sammeln gemessen.
```

## L3949 · `fn place_atomic(&mut self, b: &AtomicBox, x: i32, y: i32) {`

```
/// Einen fertigen Kasten an seinen Platz legen.
```

## L3956-3961 · `fn anon_text_box(&mut self, text: &str, st: &ComputedStyle, avail_w: i32) -> Option<AtomicBox> {`

```
/// Der anonyme Kasten um einen nackten Textlauf in einem Flex-Container.
///
/// Einzeilig, wie der Kasten eines `::before` auch: die Breite ist die
/// gemessene Textbreite, die Hoehe eine Zeile. Ein anonymer Kasten, der
/// UMBRICHT, braeuchte die volle Inline-Maschinerie und damit ein
/// Element, das es hier nicht gibt — benannt statt still.
```

## L3964-3969 · `if t.is_empty() || t.chars().all(is_zero_width_format) {`

```
// **Ein Lauf aus lauter Formatierungszeichen ist KEIN Inhalt.** Er
// erzeugt keinen Kasten — und erst recht keinen von einem Pixel, das
// dem Nachbarn fehlt. DuckDuckGos Vorlage haengt ein
// `&ZeroWidthSpace;` hinter jeden Eintrag von „Searches related to";
// daraus wurde ein eigenes Flex-Element, dessen eine Pixel den Text
// daneben auf zwei Zeilen brach.
```

## L3977-3987 · `let lead = 0.0f32;`

```
// Der Text sitzt an der Oberkante des Kastens, ohne eigenen
// Durchschuss.
//
// Benannt, weil es nicht ganz stimmt: ein Nachbar-Element setzt seine
// erste Zeile mit halbem Durchschuss, und in einer Schrift, deren
// Zeilenabstand groesser ist als ihre Groesse, steht der anonyme Lauf
// dadurch bis zu zwei Pixel hoeher. Ein fester Ausgleich waere
// geraten: er stimmte in einer Schrift und waere in der naechsten
// wieder daneben. Richtig ist, die erste Zeile durch dieselbe
// Inline-Maschinerie zu legen wie ein Element — und die braucht ein
// Element, das es hier nicht gibt.
```

## L4020-4025 · `if matches!(ps.position, Position::Absolute | Position::Fixed) {`

```
// An out-of-flow generated box needs a containing block and offsets we
// do not resolve for pseudo-elements yet. Placing it IN the flow puts
// it somewhere it never belongs — MediaWiki underlines the active tab
// with `a::after { position: absolute; bottom: 0; height: 2px }`, and
// in-flow that draws a line straight through the tab's text. Produce
// nothing rather than render it wrong.
```

## L4084-4086 · `fn render_content(&self, owner: &Element, template: &[ContentPiece]) -> String {`

```
/// Resolve a `content` template to its final text, reading any
/// `counter()`/`counters()` against the current counter scope and any
/// `attr()` off `owner` — the element the pseudo-element hangs on.
```

## L4092-4093 · `ContentPiece::Attr(name) => out.push_str(owner.attr(name).unwrap_or("")),`

```
// A missing attribute is the empty string, not a dropped value
// (CSS2.1 §12.2) — an empty `::before` box is still generated.
```

## L4100 · `if vals.is_empty() {`

```
// An out-of-scope counter is treated as a single 0.
```

## L4117-4121 · `fn layout_block(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {`

```
/// Lay one block-level box with the CSS block box model: resolve the
/// horizontal box (margins incl. `auto`-centering, width, min/max-width,
/// padding) within the containing block's content width `w`, add vertical
/// padding, then lay the content. This is what makes `max-width` + `margin:
/// 0 auto` **centered containers** work.
```

## L4123-4125 · `self.flow_block_impl(el, st, x, w, y0, Collapse::default(), true).bottom`

```
// `isolated`: `y0` is the border-box top (the caller — a float, cell,
// flex item, abs box — already positioned it and owns its margins), so
// no parent/sibling margin collapsing applies to this box's own edges.
```

## L4129-4135 · `fn flow_block_impl(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, base_y: i32, incoming: Collapse, isol`

```
/// Lay one block-level box with the CSS block box model and margin
/// collapsing. In flow (`isolated == false`), `base_y` is the collapse edge
/// and `incoming` the open adjoining margin: the box's top margin collapses
/// with them (and, if it has no top border/padding, with its first child);
/// its bottom margin collapses with its last child (auto height) and is
/// left open for the next sibling. When `isolated`, `base_y` is the
/// border-box top and margins are committed, not propagated.
```

## L4137-4138 · `let st_in = st;`

```
// Der Stil, wie er hereinkam — der Durchfall-Fall unten faehrt diesen
// Kasten ein zweites Mal, und zwar genau so, wie der Rufer ihn wollte.
```

## L4140 · `let clear_floor = self.clear_floor.take();`

```
// Gehoert diesem Kasten allein — genommen, bevor ein Kind sie sieht.
```

## L4147-4149 · `if let (Some((iw, _)), Len::Auto) = (replaced_intrinsic(el), st.width) {`

```
// §10.3.4: a replaced element with `width: auto` takes its INTRINSIC
// width. It does not fill its container the way a block box does, so
// `resolve_block_h`'s auto-solve is the wrong answer for it.
```

## L4154-4162 · `if let Len::Intrinsic(k) = st.width {`

```
// An intrinsic keyword on an IN-FLOW block's `width` (`min-content`,
// `max-content`, `fit-content`). The first attempt at this was
// MEASURED at +12/−12 and reverted: `intrinsic_width` walked every
// child as block content, so on a grid, flex or table box it answered
// about the wrong formatting context — and `width: fit-content` on a
// `display:grid` wrapper is how several grid REFERENCES frame
// themselves. Now that the measurement dispatches on `display`
// (`intrinsic_flex`/`intrinsic_grid`/`intrinsic_table`), the keyword
// is honoured here too.
```

## L4182-4184 · `let spec0 = self.spec_mark();`

```
// Alles, was dieser Kasten aufzeichnet, faengt hier an. Zehn
// `len()`-Abfragen, keine Allokation — es kostet nichts, den Punkt
// immer zu kennen.
```

## L4192-4193 · `let mut top = incoming;`

```
// Top margin. In flow it collapses with the incoming margin; a box with
// no top border/padding also collapses it with its first child.
```

## L4199-4200 · `let pad_v = pt + pb + bt + bb;`

```
// Explicit `height`/`min`/`max-height` (definite lengths only; `%` needs
// a definite CB height we don't track). Border-box subtracts pad+border.
```

## L4209-4216 · `let retry_state = (collapse_top`

```
// **Kann dieser Kasten einen zweiten Durchgang brauchen?** Nur, wenn
// sein Oberrand mit denen der Kinder zusammenfaellt UND etwas ihn
// daran hindert, selbst durchzufallen — dann bleiben durchgefallene
// Kinderraender an SEINEM Oberrand haengen (unten, nach dem Fluss).
// Die Frage haengt allein am Stil, also wird sie hier gestellt: bei
// fast jedem Kasten ist die Antwort nein und der Schnappschuss
// entfaellt. Zaehler und Listenzahl gehoeren dazu — ein zweiter
// Durchgang darf `counter-increment` nicht doppelt anwenden.
```

## L4225 · `let prov_top_y = if isolated { base_y } else { base_y + top.px() };`

```
// Provisional border-box top (exact unless the first child grows `top`).
```

## L4228 · `let (child_anchor, child_incoming) = if collapse_top {`

```
// Where children start, and what open margin flows into them.
```

## L4235-4241 · `if st.is_rule {`

```
// `<hr>` renders a rule at the content top.
//
// ZWEI Pixel hoch, nicht drei: §15.3.3 gibt der Linie `height: 0` und
// einen 1-px-`inset`-Rahmen, also liegen Ober- und Unterkante direkt
// aneinander. Wir hatten einen 3-px-Kasten mit der Linie in der Mitte
// — jede ungestaltete Seite war unter jedem `<hr>` um 1 px verschoben,
// und die Linie selbst stand einen Pixel zu tief.
```

## L4249-4251 · `if st.display == Display::ListItem && st.list_style != ListStyle::None && !st.hidden && !st.transparent {`

```
// The `display:list-item` marker box, outside the content edge.
// `list-style-type:none` generates none at all — Wikipedia's nav/TOC
// lists rely on that, and a bullet there is pure noise.
```

## L4255-4258 · `let n = ((st.font_px * 0.30) as i32).clamp(3, 7);`

```
// A triangle out of rows of `Rect`, not a glyph: the subsetted
// Inter faces carry no U+25B8/U+25BE, so a text marker would
// paint nothing on exactly the pages that need it. `n` steps
// give a 2n-1 wide, n tall triangle.
```

## L4260-4261 · `let (cx, cy) = (content_x - 12 + n / 2, top + (st.font_px * 0.55) as i32);`

```
// Both orientations centre on the same point, so the marker
// does not jump sideways when the section is opened.
```

## L4272 · `(x0 + i, y0 + i, 2 * (n - i) - 1, 1)`

```
// Pointing down: rows narrowing towards the tip.
```

## L4275 · `(x0 + i, y0 + i, 1, 2 * (n - i) - 1)`

```
// Pointing right: columns shortening towards the tip.
```

## L4281-4289 · `let s = ((st.font_px * 0.33) as i32).clamp(4, 9);`

```
// Die FORM ist der Wert dieser Eigenschaft: `disc` ist eine
// gefuellte Scheibe, `circle` ein Ring, `square` ein Quadrat.
// Alle drei als Quadrat zu malen macht sie ununterscheidbar —
// und eine verschachtelte Liste, die ihre Ebenen genau darueber
// auseinanderhaelt, sieht dann auf jeder Ebene gleich aus.
//
// Die Groesse folgt der Schrift (Browser nehmen rund ein
// Drittel der Schriftgroesse), damit der Punkt in einer kleinen
// Liste nicht klobig und in einer grossen nicht verloren wirkt.
```

## L4297 · `ListStyle::Circle => {`

```
// `circle` ist hohl — ein Ring von einem Pixel.
```

## L4310-4311 · `let label = marker_label(st.list_style, self.marker_ord);`

```
// A counter marker is right-aligned against the content edge,
// like every browser's `::marker` box.
```

## L4330-4332 · `let prev_cb = self.cb;`

```
// A positioned block becomes the containing block for `absolute`
// descendants — its PADDING box (§10.1). `prov_top_y` is the border-box
// top, so the padding edge is one border down.
```

## L4338-4348 · `if cb.3.is_none() && !self.measuring_cb_h.get() {`

```
// §10.1: the containing block for an absolutely positioned
// descendant is this box's PADDING box — a USED height, definite
// once laid out even when `height` is `auto`. That is a different
// question from `cb_h` below, where §10.5 rightly leaves an auto
// height indefinite for IN-FLOW children.
//
// Treating both as indefinite made `top: 50%` on an abspos child
// unresolvable, so it fell back to its static position. With the
// `top:50%` + `translate(-50%)` centring idiom that puts the box a
// full box-height too low — which is where Wikipedia's search
// magnifier ended up, half outside its `overflow:hidden` field.
```

## L4367-4370 · `let prev_cb_h = self.cb_h;`

```
// This box's own content height is what a percentage height on a CHILD
// resolves against — and only when it is definite. An `auto` height
// depends on those very children, so it stays indefinite and their
// percentages fall back to `auto` (§10.5).
```

## L4373-4375 · `let float0 = self.floats.len();`

```
// Floats already active here belong to an enclosing formatting context;
// anything added below is this box's own (§10.6.7, resolved after the
// children).
```

## L4378-4381 · `Flow { bottom: child_anchor, open: Collapse::default(), first_top: child_anchor, committed: false, open_sealed: false }`

```
// A replaced element's children are not page content — an
// `<iframe>`'s fallback text, a `<video>`'s `<source>` list, a
// `<canvas>`'s alternative. Nothing commits; the box is the whole
// of it, and its height comes from the intrinsic size below.
```

## L4392-4393 · `self.cb_pend.truncate(prev_pend);`

```
// The recipes pushed inside this box can no longer be reached: every
// `cb` that referred to one has been restored past it.
```

## L4396-4413 · `if let Some((ctrs, ord)) = retry_state {`

```
// **Faellt der Rand ALLER Kinder durch, gehoert er an den OBERRAND
// dieses Kastens** (CSS 2.1 §8.3.1). Legt sich kein Kind fest, stossen
// seine Raender oben wie unten an die des Elters; hat der Elter aber
// eine Hoehe (hier: `min-height`), erreichen sie dessen UNTERrand nicht
// mehr — also bleibt nur der obere, und der Kasten rueckt nach unten.
//
// `margin-collapse-min-height-001.xht`: `min-height: 2em`, drei leere
// Kinder, das letzte mit `margin-bottom: 5em`. Chromium setzt den Elter
// auf y = 5em; wir setzten ihn auf 1em und liessen die 5em ganz fallen.
//
// Der Rand ist erst BEKANNT, wenn die Kinder gelaufen sind, und er
// bestimmt, wo sie stehen — ein Kind, das durchfaellt, kann einen Float
// enthalten, und der wird gemalt. Deshalb ein zweiter Durchgang mit dem
// gefundenen Rand als Eingang statt einer Verschiebung der Befehle
// hinterher: eine Nebentabelle aus Befehlsindizes mitzuziehen ist die
// Sorte Buchhaltung, die spaeter auseinanderlaeuft. Ein DRITTER
// Durchgang kann nicht kommen — `Collapse::merge` ist idempotent, also
// findet der zweite genau den Rand wieder, mit dem er gestartet ist.
```

## L4422-4427 · `let base2 = match clear_floor {`

```
// §9.5.2: eine Raeumung SETZT die Oberkante, sie addiert
// nicht. Der eben gefundene Rand gehoert deshalb in die
// HYPOTHETISCHE Lage — wo der Kasten ohne `clear` staende
// —, und die Raeumung nimmt davon das Maximum. Ohne das
// rutschte ein geraeumter Kasten um genau diesen Rand unter
// seinen Float (`CSS2/margin-collapse-157`).
```

## L4441-4442 · `let border_top_y = if collapse_top && flow.committed { flow.first_top } else { prov_top_y };`

```
// Resolve the border-box top: when the top margin collapsed through, the
// box's border box sits at the first committed child's border-box top.
```

## L4446-4452 · `let auto_height = !matches!(st.height, Len::Px(_));`

```
// §10.6.7: a box that establishes a block formatting context and has an
// auto height grows to contain its own floats. A box that does NOT
// establish one never does — its floats escape to the enclosing
// context, which is exactly why a bare `<div>` around a float measures
// zero and an `overflow:hidden` wrapper must not. `isolated` marks the
// BFC roots the caller positions itself (float, cell, flex item,
// inline-block, abspos, root); `establishes_bfc` the in-flow ones.
```

## L4460-4463 · `if !flow.committed || st.contain_size {`

```
// Size containment takes this branch even with content in it: under
// `contain: size` the content contributes NO size (css-contain-2 §3.1),
// so the box is measured exactly as if it were empty — the content
// still paints, it just overflows.
```

## L4465 · `let mut ch = 0;`

```
// No in-flow content. Its explicit box height, if any.
```

## L4474-4475 · `ch = ih as i32;`

```
// §10.6.2: `height: auto` on a replaced element is its
// intrinsic height, not the zero its (unrendered) content says.
```

## L4478-4480 · `if let (Some(fb), false) = (float_bottom, st.contain_size) {`

```
// A container whose ONLY content is a float: no line box ever
// committed, so the float alone decides the height. A contained
// box takes nothing from its floats either.
```

## L4490-4493 · `if collapse_top && bb == 0 && pb == 0 && ch == 0 && !flow.committed {`

```
// A box with no content, border, padding or height collapses through:
// its top and bottom margins are adjoining.
// Collapsing through needs the box to be genuinely empty; a
// contained box holds content and must not let margins meet.
```

## L4507-4509 · `let collapse_bottom = !isolated && bb == 0 && pb == 0 && auto_height && !flow.open_sealed;`

```
// Box with committed content. The last child's trailing margin
// (`flow.open`) collapses with this box's bottom margin only when the
// box has auto height and no bottom border/padding separating them.
```

## L4512 · `if let Some(fb) = float_bottom {`

```
// A float reaching past the last line box extends the content edge.
```

## L4518-4541 · `let mn = px_h(st.min_height).unwrap_or(0);`

```
// **`min-height` ueber dem Inhalt VERSCHLUCKT den Schlussrand.**
// Er entkommt nicht (die Fusszeile darunter rueckt nicht weg) —
// und er wird auch nicht mitgerechnet.
//
// Das Zweite stand hier falsch: `ch` bekam `flow.open` dazu, also
// wuchs der Kasten um den Rand, den er gerade eingesperrt hatte.
// In `margin-collapse-min-height-001` sind das 550 px, und der
// gruene Kasten lief statt 100 px hoch aus dem Bild.
//
// Gemessen an Chromium ueber fuenf Faelle, und die Grenze ist
// genau diese Bedingung:
//
//     min-h  Kind  Rand    Elter  Fusszeile      der Rand
//      100    30   550      100   direkt danach  verschluckt
//      100   200   550      200   +550           entkommt
//      100    30    20      100   direkt danach  verschluckt
//        0    30   550       30   +550           entkommt
//      100   120   550      120   +550           entkommt
//
// Er verschwindet also GENAU dann, wenn `min-height` die Hoehe
// ueber den Inhalt hebt — dann liegt zwischen Inhaltsunterkante
// und Rahmenunterkante Platz, die beiden Raender stossen nicht
// mehr aneinander, und ein nicht aneinanderstossender Rand in
// diesem Zwischenraum wird aufgesogen.
```

## L4558-4559 · `ch = (flow.bottom + flow.open.px() - content_top).max(0);`

```
// Bottom border/padding or a definite height: commit the trailing
// child margin into the content box.
```

## L4582-4597 · `fn svg_box(&self, el: &Element, st: &ComputedStyle) -> (i32, i32) {`

```
/// The decoded image (if any) + natural box size for an `<img>`: from the
/// `width`/`height` attributes, else the decoded intrinsic size, else a
/// fallback. Not clamped to the line width — `flow` fits it when placing.
/// Size an `<img>` box.
///
/// Also records, via `guessed`, whether this box had to guess:
/// with both `width` and `height` given the geometry is definite and the
/// pixels arriving later change nothing, so the shell can repaint instead
/// of re-laying-out. Without them the box depends on the decoded size, and
/// a later decode really does move the page.
/// The box of an inline `<svg>`, and the render request that fills it.
///
/// Unlike an `<img>` this never has to be guessed: the intrinsic size is in
/// the markup (`width`/`height`, else the `viewBox`, else CSS's 300×150
/// default for a replaced element with no intrinsic size), so the box is
/// definite on the FIRST layout and arriving pixels only need a repaint.
```

## L4608-4611 · `fn svg_size(&self, el: &Element, st: &ComputedStyle) -> (i32, i32) {`

```
/// The same box WITHOUT registering a raster — what a measurement needs.
/// `svg_box` enqueues the element for rasterising, and an intrinsic pass
/// that called it would queue an icon that is never painted (and queue the
/// painted one twice).
```

## L4627 · `let (w, h) = match (aw, ah) {`

```
// One given side keeps the intrinsic ratio, as for any replaced element.
```

## L4641-4647 · `let css = |l: Len| match l {`

```
// A definite CSS length beats the presentational attribute (HTML
// §15.3). Only px counts: a percentage needs the containing block,
// which a replaced element's own box measurement does not have here, so
// it stays as indefinite as `auto`. Wikipedia sizes its footer wordmark
// this way (`style="width:7.5em;height:1.125em"`) — without this the
// box is not just the wrong size, it also counts as GUESSED, and every
// guess costs a full re-layout the moment the pixels land.
```

## L4655-4657 · `let mut g = self.guessed.borrow_mut();`

```
// Throwaway measurements size the same `<img>` several times,
// so without this the list holds one entry per measurement pass
// and the shell rescans them all on every image that lands.
```

## L4673-4675 · `fn control_box(&mut self, el: &'a Element, st: &ComputedStyle, kind: ControlKind, avail: f32) -> CtlBox {`

```
/// Measure a form control and capture what it displays right now (the
/// user's typed value, else the authored default). Controls are atomic
/// inline boxes — they never wrap, and their children never lay out.
```

## L4677-4679 · `let pct = |l: Len| matches!(l, Len::Pct(_) | Len::Calc { .. });`

```
// `&mut` only so a percentage height can ask for the containing
// block's height, which may still be a deferred recipe. The common
// case never asks — resolving would lay a whole box out.
```

## L4689-4695 · `let (avg_w, extra_w) = match font.char_widths(size) {`

```
// **Die Eigenbreite rechnet mit der MITTLEREN Zeichenbreite**, nicht
// mit der Breite der Null, und ein `<input>` legt einmal den
// Unterschied zur BREITESTEN drauf — Platz, damit ein getipptes breites
// Zeichen das Feld nicht sofort rollen laesst. Chromium tut genau das;
// nachgemessen ueber fuenf Stuetzstellen (`size` 1, 5, 10, 20, 40), und
// erst die dritte sagt, ob die Gerade stimmt: zwei Punkte passen auf
// jede. Ohne das war ein nacktes Feld 217 statt 256 px breit.
```

## L4700-4704 · `let line = st.line_height.px(size).filter(|v| *v > 0.0).unwrap_or_else(|| line_gap(font, size));`

```
// Die Zeilenhoehe, die die SEITE gesetzt hat, sonst die der Schrift.
// Ohne das war jedes Feld so hoch wie sein Schriftbild: Bootstrap gibt
// `.form-control` `line-height: 1.5`, und ein Feld, das 24 statt 14 px
// Zeile bekommt, ist am Ende 38 statt 28 px hoch — der Unterschied
// zwischen „sieht komisch aus" und „sieht aus wie im Browser".
```

## L4710-4711 · `let (mut text, ghost) = match kind {`

```
// What the box shows: typed text (bulleted for a password), the
// placeholder when empty, or a button/select label.
```

## L4731-4742 · `let ua_min = match kind {`

```
// Intrinsic size, then let a definite CSS width/height win (real pages
// size their search fields in CSS, not with `size=`).
// **Eine Polsterung, zwei Benutzer.** Die Breite wurde mit
// `CTL_PAD_X + 4` gerechnet, gemalt wurde mit `max(CSS, CTL_PAD_X)` —
// und sobald eine Seite ihre Knoepfe selbst polstert (Bootstrap gibt
// `.btn` 16 px), war der Kasten zu schmal fuer seine eigene
// Beschriftung. `Gross` wurde als `ross` gemalt, weil der Maler die zu
// lange Zeichenkette vorne abschnitt. Beide Seiten lesen jetzt
// dieselben zwei Zahlen ([[feedback_intrinsic_shared_path]]).
//
// Die UA-Untergrenze bleibt: ein Knopf ohne eigene Polsterung soll
// nicht am Text kleben, und `+ 4` ist, was er dafuer immer hatte.
```

## L4745-4746 · `ControlKind::Select => 0,`

```
// Der Pfeilstreifen traegt den Abstand nach rechts, und links
// sitzt die Beschriftung am Rahmen — gegen Chromium gemessen.
```

## L4749-4753 · `ControlKind::Text | ControlKind::Password | ControlKind::TextArea => 2,`

```
// **Ein TEXTfeld polstert waagrecht zwei Pixel, ein Knopf sechs.**
// Beide ueber einen Kamm zu scheren machte jedes nackte Feld acht
// Pixel zu breit — und weil ein Rahmenwerk seinen Feldern immer
// eigene Polsterung gibt, sagte darueber weder Bootstrap noch
// Tailwind etwas.
```

## L4759-4764 · `let box_like = matches!(kind, ControlKind::Checkbox | ControlKind::Radio);`

```
// Senkrecht dasselbe: `.form-control` bringt `padding: .375rem .75rem`
// mit, und ohne sie steht ein Feld 6 px zu flach in seiner Zeile.
// Ein Kaestchen und ein Radioknopf haben KEINE Polsterung — die
// UA-Untergrenze ist fuer Felder und Knoepfe da, damit der Text nicht
// am Rahmen klebt, und hier gibt es keinen Text. Mit ihr kam ein
// `height: 200px` grosses Kaestchen 206 px hoch heraus.
```

## L4766-4769 · `let ua_pad_y = match kind {`

```
// Ein `<select>` hat KEINE senkrechte UA-Polsterung — seine zwei
// zusaetzlichen Pixel stecken im Widget selbst (unten in der
// Kastenrechnung). Mit einer Untergrenze hier waeren sie doppelt da,
// sobald die Seite selbst polstert: 34 statt 36.
```

## L4772 · `ControlKind::Select => 0,`

```
// Sein Abstand steckt im Widget, nicht in der Polsterung.
```

## L4774 · `ControlKind::TextArea => 2,`

```
// Ein `<textarea>` polstert zwei Pixel, ein Feld einen.
```

## L4781-4783 · `let border = ctl_border(st, kind);`

```
// The frame is part of the box, and it is the page's when the page
// styled it — a control with `border: none` is exactly as tall as its
// content, and a `border: 2px` one two pixels taller per side.
```

## L4786-4790 · `let def_ch = vert_len(st.height, cbh).map(|hh| {`

```
// HTML §button-layout: a `<button>` is not a label — its children are
// page content. Laid out here as their own block formatting context,
// then centred in the content box by `paint_control`.
// A definite CSS height gives the contents a definite content box to
// align in — `align-items: center` on a `display:flex` button needs it.
```

## L4797-4801 · `let _ = size;`

```
// **13 px, fest.** Das Zeichen waechst NICHT mit der Schrift —
// Chromium skaliert es mit dem Zoom, nicht mit `font-size`,
// und eine Seite, die ihre Kaestchen gross will, schreibt eine
// Breite hin (`width: 24px`). Die alte Formel `font-size *
// 0.9` gab bei 16 px ein 14er Kaestchen.
```

## L4806-4810 · `let cols = el.attr("cols").and_then(|c| c.trim().parse::<f32>().ok()).unwrap_or(20.0);`

```
// **Die Vorgaben stehen in HTML §4.10.11 und lauten 20 und 2**,
// nicht 30 und 3 — ein nacktes `<textarea>` kam damit 90 px zu
// breit und 20 px zu hoch heraus. Dazu der Streifen fuer die
// Rollleiste: er ist da, auch wenn nichts zu rollen ist, und
// geht in die Eigenbreite ein (Chromium ebenso).
```

## L4815-4818 · `rows as i32 * ceil_i32(line) + pad_t + pad_b + by,`

```
// **Jede Zeile wird fuer sich ganzzahlig.** `rows * line`
// erst am Ende abzuschneiden verlor bei vier Zeilen drei
// Pixel: eine Zeilenhoehe von 19,36 ist im Kasten 20, und
// zwar viermal.
```

## L4825-4829 · `ceil_i32(cols * avg_w + extra_w) + pad_l + pad_r + bx,`

```
// Aufgerundet, nicht abgeschnitten: die Vorgabe `size=20`
// fiel sonst genau einen Pixel zu schmal aus. Was bleibt,
// ist ein Pixel bei sehr kleinem `size` — Chromium rundet
// dort in einer eigenen Gleitkommakette, und die
// nachzubauen hiesse Rundung anzupassen statt ein Modell.
```

## L4837-4840 · `ceil_i32(line) + if st.appearance_none { 0 } else { 2 }`

```
// Ein `<select>` haelt ueber und unter seiner Beschriftung je
// einen Pixel frei — Teil des Widgets, INNEN, also auch dann
// da, wenn die Seite selbst polstert. `appearance: none` nimmt
// ihn mit dem Rest des Widgets weg.
```

## L4850-4851 · `text.clear();`

```
// The contents ARE the label — painting `text` on top of them would
// write the button's own text twice.
```

## L4857 · `w = if st.box_border { cw as i32 } else { cw as i32 + pad_l + pad_r + bx };`

```
// A CSS width is a content width unless `box-sizing: border-box`.
```

## L4860-4864 · `if let Some(chh) = vert_len(st.height, cbh) {`

```
// A percentage height resolves against the containing block's HEIGHT
// (§10.5), never `avail` (its width) — the checkbox-hack overlay is
// `width:100%; height:100%`, and measuring its height off the width
// made it as tall as its container is wide. An indefinite CB height
// leaves the percentage unresolvable, so the intrinsic height stands.
```

## L4874-4876 · `if let Some(mn) = vert_len(st.min_height, cbh) {`

```
// `min-height` on a control is how real pages give a search field its
// height (Codex: `min-height: 32px`). Without it the control keeps its
// intrinsic line height and sits short inside its own flex row.
```

## L4878-4881 · `h = h.max(if st.box_border { mn as i32 } else { mn as i32 + pad_t + pad_b + by });`

```
// Ohne `border-box` ist `min-height` eine INHALTShoehe, und
// darauf kommt die WIRKLICHE Polsterung — nicht die
// UA-Untergrenze. Mit ihr kam ein `min-height: 44px` hohes Feld
// 48 statt 58 px heraus, sobald die Seite selbst polsterte.
```

## L4888 · `let caret = if focused && kind.is_text() {`

```
// Caret: the shell keeps a byte offset; painting counts characters.
```

## L4894-4898 · `let said_w = st.width.px(avail).is_some();`

```
// Die Untergrenze von 8 px ist dafuer da, dass ein Steuerelement OHNE
// eigene Groesse nicht verschwindet. Wo die Seite eine Groesse nennt —
// auch die Null — ist sie die Antwort: ein `height: 0` Feld in einer
// Flex-Spalte der Hoehe 0 soll null sein, nicht acht
// ([[feedback_invented_limits]]).
```

## L4913-4914 · `(st.outline.styled && st.outline.width > 0.0).then(|| (`

```
// Die Seite hat es in die Hand genommen: ihr Wort gilt, und
// `outline: none` heisst NICHTS malen.
```

## L4925-4927 · `no_face: st.appearance_none || (st.bg_set && st.bg.is_none()),`

```
// Eine Seite, die dem Steuerelement einen Hintergrund gibt — auch
// `transparent` —, malt seine Flaeche selbst. So macht es jeder
// Browser, und Bootstraps `.btn-outline-*` verlaesst sich darauf.
```

## L4942-4953 · `fn control_content(`

```
/// A `<button>`'s children, laid out as their own block formatting context
/// at the origin (HTML §button-layout). `None` for everything else — an
/// `<input>` is void, and a text-only button is exactly its label, which
/// the one-op path already draws.
///
/// This is what made the three `centering-00x` reftests fail, and NOT on
/// the side the name suggests: our render of the TEST was right all along,
/// and it was the REFERENCE — which frames its expectation as a
/// `display: table-cell` inside a `<button>` — that came out shrink-wrapped
/// to the word inside it. The same gap eats every icon button on the web:
/// `<button><svg …/> Speichern</button>` lost its icon and shrank to the
/// text.
```

## L4963-4969 · `let takes_children = el.tag == "button"`

```
// A `<button>` takes its children; a checkbox or radio is void, but a
// page that took the widget away with `appearance: none` builds its own
// out of `::before`/`::after` — that is how every custom checkbox on
// the web is drawn, and it is what the two `no-centering` reftests ask
// for. A text field, a `<select>` and a `<textarea>` are left alone:
// their contents are a shadow tree in every engine, and a pseudo does
// not reach into it.
```

## L4976-4979 · `let pushed = self.path.last().map(|p| p.seq()) != Some(el.seq);`

```
// The element has to be on the path before anything under it is styled
// — `button > div` selects through it, and `pseudo_content` reads the
// path as the pseudo's ancestor chain. Two of the call sites push it
// already; pushing it twice would make `>` skip a level.
```

## L4992-4994 · `let content_w = match st.width.px(avail) {`

```
// The content width is the box's own, resolved the way an
// `inline-block` resolves it: a definite CSS width wins, else
// shrink-to-fit between the content's min and its preferred width.
```

## L4999-5004 · `let (mut pref, mut min) = match st.display {`

```
// Measured in the SAME formatting context the contents will be
// laid out in — a flex row's items sit side by side, so its
// max-content width is their SUM, not the widest of them.
// Measured as block content, `<button class=flex><svg/><span>Mit
// Text</span></button>` came out one item wide and broke its
// own label onto two lines.
```

## L5010-5011 · `for p in [before, after].into_iter().flatten() {`

```
// A generated box starts its own line, so it is the WIDEST
// contribution, not one added to the others.
```

## L5022-5031 · `let (s0, sl0, f0, fl0) = (self.stack_ops.len(), self.stack_links.len(),`

```
// **Die Stapelbereiche gehoeren dazu, und ihr Fehlen war sichtbar.**
// Was hier gleich ausgelegt wird, wird danach wieder HERAUSGEZOGEN —
// ein Bereich, den ein Kind dabei notiert, zeigt hinterher auf
// Befehle, die es nie gemalt hat, naemlich auf die naechsten. Auf
// Wikipedia sortierte so ein Bereich aus dem SUCHknopf die Flaeche
// eines spaeteren Knopfes HINTER dessen eigene Beschriftung: der
// Knopf neben „Appearance" war eine graue Kiste ohne Text, und jeder
// Klick darauf kostete ein volles Auslegen, weil seine Spanne
// zerrissen war. `spec_rollback` nennt genau diese Regel — sie galt
// hier nur nicht.
```

## L5036-5041 · `let mut inner = *st;`

```
// The contents lay out in the formatting context the BUTTON declares.
// Tailwind writes `flex`/`inline-grid` on nearly every icon button, and
// laying those children out as blocks stacks an icon above its label
// instead of beside it. The style handed down has the chrome zeroed:
// `paint_control` owns the face, the frame and the padding, and a
// second copy here would paint the box twice and inset it twice.
```

## L5077-5078 · `self.links.truncate(l0);`

```
// A button's content model is phrasing content: no control, no link,
// nothing that owns a hit rect of its own inside it.
```

## L5083-5085 · `self.stack_ops.truncate(s0);`

```
// Und die Bereiche, die in `ops` zeigen — siehe oben. Die innere
// Stapelordnung eines Knopfes geht damit verloren; sie war vorher
// nicht etwa da, sondern wurde auf FREMDE Befehle angewandt.
```

## L5093-5095 · `fn pseudo_intrinsic(&self, el: &Element, st: &ComputedStyle, kind: PseudoElem) -> Option<f32> {`

```
/// The preferred width of `el`'s `::before`/`::after` generated box, when
/// it makes one that takes part in the flow. `intrinsic_width_nodes` only
/// sees real nodes, and a control's contents may be nothing else.
```

## L5101-5105 · `let margins = ps.margin_left.px(0.0).unwrap_or(0.0) + ps.margin_right.px(0.0).unwrap_or(0.0);`

```
// Der RANDkasten zaehlt, nicht der Rahmenkasten: DDGs Lupe traegt
// ihre 4 px Abstand als `margin-right`, und ohne die kam der Kasten
// um genau diese 4 px zu schmal heraus. Ein Prozentrand loest sich
// gegen eine Breite auf, die es hier noch nicht gibt — er zaehlt
// deshalb null, wie in `child_outer` auch.
```

## L5116-5118 · `fn layout_abs(&mut self, el: &'a Element, st: &ComputedStyle, static_x: i32, static_y: i32) {`

```
/// Lay a `position:absolute`/`fixed` box, out of flow, at a position derived
/// from the containing block (`self.cb`) + `top`/`right`/`bottom`/`left`.
/// The element is `el`, already pushed onto `self.path` by the caller.
```

## L5120-5122 · `let over_line = core::mem::take(&mut self.abs_over_open_line);`

```
// Read FIRST. Resolving the containing block below can lay this very
// box out speculatively and roll it back, and that pass would otherwise
// consume the flag and leave the real pass with nothing.
```

## L5129-5130 · `let cbh = self.cb_height();`

```
// The containing block's height may still be a deferred recipe — ask
// for it before anything reads it (§10.1).
```

## L5133-5137 · `if cbh == Some(self.viewport_h as i32)`

```
// An out-of-flow box against the INITIAL containing block moves with
// the viewport height only if it actually reads that height: `bottom`
// anchors it to the far edge, and a percentage `top`/`height` scales
// with it. A box placed by `top`/`left` alone does not care how tall
// the viewport is.
```

## L5145-5147 · `let prev_cb_h = self.cb_h;`

```
// An out-of-flow box resolves its percentage height against the
// containing block it is positioned in, not against whatever in-flow
// ancestor happens to be open (§10.5 + §10.1).
```

## L5157-5162 · `let frame = st.margin_left.px(avail).unwrap_or(0.0)`

```
// Shrink-to-fit (§10.3.7). `intrinsic_width` returns a CONTENT
// width, but what goes to `layout_box` is read as a containing
// block and has margin/padding/border taken off it AGAIN — so
// the box lost its own frame twice and its content overflowed
// by exactly that much. Floats and inline-blocks hand over the
// MARGIN-box width for this reason; this path did not.
```

## L5171-5176 · `let width = clamp_len(width, st.min_width, st.max_width, st.box_border, st.pad_left + st.pad_right + st.border_x());`

```
// `min-width`/`max-width` apply to an out-of-flow box like any other
// (CSS2.1 §10.4) — the height path already went through them, the width
// did not. A shrink-to-fit box with no content is the case that shows
// it: MediaWiki's search magnifier is an empty absolutely positioned
// span sized only by `min-width`, and without the clamp it came out
// ONE pixel wide.
```

## L5178-5179 · `let px = if let Some(l) = left {`

```
// Horizontal: an offset pins to the CB edge; with both `left`/`right`
// auto the box keeps its **static position** (CSS2.1 §10.3.7).
```

## L5187-5191 · `let top = vert_len(st.top, cbh);`

```
// Vertical offsets resolve against the CB **height**, never its width
// (§9.3.2) — getting that wrong stretches every percentage-positioned
// layout by the CB's aspect ratio. An indefinite CB height leaves a
// percentage unresolvable here (the parent's content height doesn't
// exist yet while its children are laid out), so it behaves as `auto`.
```

## L5195-5197 · `let mut st_owned = *st;`

```
// §10.6.4: `top` + `bottom` with `height:auto` stretches the box to the
// gap between them. Over-constrained (all three given) → `bottom` is
// the one that gets ignored, which is what the `top` arm below does.
```

## L5199-5203 · `let mut overridden = false;`

```
// NOTE: a percentage `height` here would also resolve against `cbh`
// (§10.5), but doing it in the abspos path ALONE measured worse: an
// in-flow `height:100%` parent still collapses to auto, and the
// mismatch between the two paths breaks more than it fixes. It belongs
// with general percentage-height support, not here.
```

## L5211-5221 · `let mt = st.margin_top;`

```
// `top` pins to the CB top; `bottom` pins the box's bottom edge, which
// needs its own height — only known once it is laid out. So lay it out
// at the static position and slide the finished box (and everything it
// emitted) into place. With neither offset the static position is the
// answer already (§10.6.4).
// `layout_box` is handed the BORDER-box top, so the vertical margins
// belong here: §10.6.4 puts the box at `top + margin-top` below the
// containing block's edge, and the static position is where its MARGIN
// box would have sat. Leaving them out placed an absolutely positioned
// box its own `margin-top` too high — visible the moment a page uses
// `margin` instead of `top` to nudge an overlay.
```

## L5229 · `let w_i = width.max(1.0) as i32;`

```
// layout_box → layout_block re-establishes the CB for its own children.
```

## L5233-5239 · `let _ = over_line;`

```
// An explicit `z-index` on this positioned box opens a tracked
// stacking range for it (CSS2.1 §9.9) — unless it's already nested
// inside another tracked range, which absorbs it instead. A box
// reached mid-line opens one too, to climb over that line.
// An out-of-flow box is positioned by definition, so `over_line` — the
// old stand-in for "this one at least must be lifted over its line" —
// has nothing left to add.
```

## L5245-5249 · `let pct = |l: Len| matches!(l, Len::Pct(_) | Len::Calc { .. });`

```
// The same second resolution `place_float` guards against: `layout_box`
// reads `w_i` as the containing-block width, and `w_i` IS this box's
// own used width — so a `width: 18%` box came out 18 % of 18 %. The
// reference of `floats-wrap-bfc-outside-001` is exactly that box, which
// is why fixing only the float side made a correct test fail.
```

## L5269-5278 · `if el.tag == "img" || el.tag == "svg" {`

```
// A replaced element out of flow still has to be PAINTED. `layout_box`
// gives it a rectangle — borders, background, the space it occupies —
// but the picture itself is only ever emitted by the inline path, so
// routing an abspos `<img>` here left an empty box behind. The render
// gate is what said so: `tailwind` lost seven draw ops at unchanged
// height the moment images stopped riding on the line.
//
// The size comes from `img_box`, not from `w_i`: a positioned replaced
// element with `width: auto` takes its INTRINSIC width (§10.3.7), while
// `w_i` is what a non-replaced block would have stretched to.
```

## L5282-5284 · `if iw > 0 && ih > 0 {`

```
// No `hidden`/`transparent` guard: the inline path emits the op
// either way and lets paint decide, and dropping it here made the
// op counts disagree between the two paths for the same picture.
```

## L5303-5306 · `let bottom = if let Some(target) = shift_to_bottom {`

```
// Bottom-anchored: now that the used height is known, translate the box
// (all ops/links/controls it just emitted) so its bottom edge lands on
// the offset. Indices stay valid — nothing is inserted or removed, so
// any stacking range recorded inside is untouched.
```

## L5317-5319 · `if let Clip::Rect { top, right, bottom: cbot, left } = st.clip {`

```
// `clip: rect(...)` (CSS 2.1 §11.1.2) — clip this box (and its
// descendants, all emitted into `ops[start..]`) to a rectangle whose
// offsets are measured from the border-box top-left corner.
```

## L5321 · `let (cw, off_left) = resolve_block_h(st, w_i as f32);`

```
// Border box, mirroring `layout_block`'s geometry.
```

## L5324 · `let bl = px as i32 + ml as i32; // border-box left`

```
// border-box left
```

## L5325 · `let bt = py as i32; // border-box top (== the y0 layout_block used)`

```
// border-box top (== the y0 layout_block used)
```

## L5327 · `let bb = bottom; // border-box bottom (layout_box return)`

```
// border-box bottom (layout_box return)
```

## L5328 · `let cl = bl + left.map(|v| v as i32).unwrap_or(0);`

```
// Clip edges (auto = the corresponding border edge).
```

## L5335-5338 · `let (tdx, tdy) = translate_offset(st, w_i, bottom - py as i32);`

```
// `transform: translate(...)`, same paint-time shift as in flow. This is
// where the `top:50%` + `translate(-50%)` centring idiom lands, so an
// out-of-flow box that skipped it sat half a box too low — far enough
// to be clipped away by an `overflow:hidden` parent.
```

## L5347 · `let dy = bottom - box_bottom;`

```
// The out-of-flow box, at its final (post-bottom-shift) position.
```

## L5353-5369 · `fn clip_overflow(&mut self, st: &ComputedStyle, marks: (usize, u32, u32), box_left: i32, box_top: i32, box_w: i32, box_h`

```
/// Insert the block's `background-color` behind its content (at `bg_idx`)
/// and stroke its `border` on the border-box edges.
/// Insert a box's `background-color` behind the content it already emitted
/// (at `bg_idx`). Split out from `paint_box_decoration` because a table can
/// paint its background — an opaque infobox must not let the article text
/// it floats over show through — while its BORDER still can't be drawn from
/// here: the table box has no resolved border box yet, and guessing one
/// puts the stroke in the wrong place (measured: 5 reftests).
/// `overflow: hidden` — drop whatever the box's content painted outside its
/// padding box. `marks` is `(first op index, abs_count, fixed_count)` taken
/// BEFORE the content was laid out. Call BEFORE `paint_box_decoration`, so
/// the box's own background and border are not clipped by it.
///
/// Skipped when a descendant recorded a z-index stacking range inside the
/// span: `clip_ops` rebuilds the tail and can drop ops, which would leave
/// those ranges pointing at the wrong slots — and a scrambled display list
/// is a far worse defect than an unclipped overflow.
```

## L5375-5380 · `if self.fixed_count > fixed0 || (st.position == Position::Static && self.abs_count > abs0) {`

```
// An out-of-flow descendant is clipped only by an ancestor in its
// CONTAINING-BLOCK chain (CSS2.1 §11.1.1). A `position: static` box is
// not the containing block of an absolutely positioned descendant, and
// nothing but the viewport is for a fixed one — so a box that let one
// escape cannot clip its span at all. The display list is flat, so the
// escapee's ops can't be excluded individually.
```

## L5384-5385 · `let (cl, cr) = if st.overflow_x.clips() {`

```
// An axis that does not clip is given the whole plane, so one call
// covers `overflow-x: hidden; overflow-y: auto` without a second path.
```

## L5403-5409 · `self.stack_ops.retain_mut(|(_, _, s, e)| match remap_clip(&map, start, *s, *e) {`

```
// Every side table that points into the display list moves with it.
// This used to be a BAIL — "a descendant recorded a stacking range in
// here, so do not clip at all" — which was tolerable while only an
// explicit `z-index` opened one. Now every positioned box does, and the
// bail meant an `overflow: hidden` box with any positioned child
// stopped clipping: 467 draw ops escaped their boxes on one vendored
// page.
```

## L5424-5425 · `for c in &mut self.controls {`

```
// A control's span must stay EXACT — `repaint_controls` overwrites it
// in place — so a torn or dropped one is marked unusable instead.
```

## L5445-5455 · `fn ellipsize(&mut self, start: usize, cr: i32) {`

```
/// `text-overflow: ellipsis` — a text run that would cross the box's right
/// clip edge is cut back and ends in `…` instead (css-ui-4 §5.2).
///
/// Done here, on the finished display list, rather than during line
/// breaking: the property does not change layout at all — the line is
/// measured, broken and positioned as if it were `clip`, and only what
/// gets PAINTED differs. Doing it any earlier would move the box.
///
/// `clip_ops` keeps a text run whole when it merely overlaps the clip
/// (glyphs are not clipped per pixel), so without this a `.text-truncate`
/// box does not just lack the `…` — its text runs on out of the box.
```

## L5465-5467 · `if avail <= 0.0 {`

```
// No room for even the ellipsis: the run is past the edge entirely
// and `clip_ops` will drop it, so leave it be rather than emitting
// a lone `…` at a position the line never reserved.
```

## L5477-5489 · `fn apply_filter(&mut self, st: &ComputedStyle, start: usize) {`

```
/// `filter` — recolour everything the box painted, itself and its subtree.
///
/// The property applies to the whole subtree and cannot be cancelled from
/// inside it, which in a FLAT display list is exactly the op range the box
/// produced. Called after `paint_box_decoration`, so the box's own
/// background and border — spliced in at the head of that range — are in it.
///
/// Colours are transformed here rather than at paint, because here they are
/// known. An image's pixels are not: it travels as a key and is looked up
/// when it is drawn, so those ops get an index into `filters` instead.
/// Applying the matrix twice IS the composition of two filters, which is
/// what a filtered box inside a filtered box means — the image index is
/// composed by hand for the same reason.
```

## L5508-5509 · `DrawOp::Gradient { g, .. } => {`

```
// Die Farben eines Verlaufs stehen schon hier fest, also
// wird jeder Stopp gefiltert — kein Eintrag in `filters`.
```

## L5515-5516 · `DrawOp::BgImage { tint: Some(c), .. } => *c = f.apply(*c),`

```
// A mask paints `tint` THROUGH the image's alpha, so the
// filter belongs on that colour, not on the stencil's pixels.
```

## L5527-5529 · `fn filter_index(&mut self, st: &ComputedStyle) -> u16 {`

```
/// Register a `filter` and hand back the 1-based index an image op carries.
/// An `<img>` with a filter of its OWN needs this before its op exists:
/// the op is emitted from the line box, which has no `Ctx` to ask.
```

## L5555-5563 · `fn insert_ops_at(&mut self, at: usize, ops: Vec<DrawOp>) {`

```
/// Splice ops into the display list at `at`, keeping every recorded
/// stacking/float range consistent.
///
/// `insert` shifts every later op up by `n` slots — any already-recorded
/// range overlapping or after `at` (a descendant's tracked z-index range,
/// recorded before its ancestor's background gets painted in) must shift
/// too. Half-open `[s, e)`: a range that already ends at-or-before `at` is
/// untouched (`e > at`, strict — `e == at` means the insertion lands right
/// after the range, not inside it).
```

## L5572-5577 · `for c in &mut self.controls {`

```
// Die Steuerelemente sind die DRITTE Tabelle mit Befehlsindizes, und
// sie wurde hier jahrelang vergessen. Der Hintergrund eines Kastens
// wird UNTER seinen schon gemalten Inhalt geschoben — also vor jedes
// Feld darin. Blieb `at` stehen, ersetzte der Schnellweg beim naechsten
// Tastendruck fremde Befehle, und das Feld malte sich unter den alten
// Kasten: getippter Text unsichtbar, der Text daneben verschoben.
```

## L5601 · `fn bg_key(&self, image: Option<u64>) -> Option<u64> {`

```
/// Note that this layout needs a CSS image, and hand back its key.
```

## L5611-5612 · `fn paint_edge(&mut self, s: &BorderSide, x: i32, y: i32, w: i32, h: i32) {`

```
/// Paint one border edge rect (the collapsed model draws grid lines
/// individually rather than a box's four sides together).
```

## L5622-5624 · `if w <= 0 || h <= 0 || st.hidden || st.transparent {`

```
// `visibility:hidden` suppresses this box's own background and border.
// Bailing before the `bg_idx` insert is what keeps the recorded
// stacking ranges intact — nothing is inserted, so nothing shifts.
```

## L5639-5648 · `let mut border: Vec<DrawOp> = Vec::new();`

```
// CSS 2.1 Appendix E paints a box as shadow, background, border — and
// ALL of it before any descendant. All three splice in at `bg_idx`, so
// whatever is inserted last ends up underneath: border, background,
// shadow, in that order.
//
// The border used to be APPENDED instead, which put it on top of the
// box's own descendants. That is invisible while a child stays inside
// its parent's content box — and wrong the moment one does not, which
// is exactly what a negative margin is for: the child's border landed
// UNDER the parent's instead of over it.
```

## L5652-5654 · `let mut inset = Vec::new();`

```
// Der INNERE Schatten liegt ueber dem Hintergrund; er wird also vor
// ihm eingefuegt, damit er nach der Verschiebung durch `insert_bg`
// darueber steht.
```

## L5662-5669 · `fn insert_shadow(&mut self, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, bg_idx: usize) {`

```
/// Paint the box's `box-shadow` behind its background. Only the zero-blur
/// case — which on real pages is a hairline separator, not a drop shadow.
/// MediaWiki draws the rule under the article tabs with
/// `box-shadow: 0 1px #c8ccd1`, and without this the page simply lacks it.
///
/// Inserted at `bg_idx` BEFORE the background, so it ends up underneath;
/// `insert_bg` then shifts the recorded stacking ranges for its own ops the
/// same way, and both insertions are accounted for.
```

## L5676-5682 · `fn layout_table(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> (i32, i32, i32) {`

```
/// Simplified table layout. Two column models: `table-layout: auto` sizes
/// columns from cell content (readable infoboxes + data tables); `table-
/// layout: fixed` (CSS2 §17.5.2.1) takes column widths from the table/
/// `<col>`/first-row cell `width`s and distributes the rest, painting each
/// cell's own box (background/border/padding). Rows/cells are recognised by
/// HTML tag (`tr`/`td`/`th`/`thead`…) or `display: table-*`; anonymous
/// boxes fill any missing row/row-group/cell wrapper (CSS2 §17.2.1).
```

## L5684-5690 · `let cbw = w as f32;`

```
// <caption> renders as a block on the table's top or bottom edge
// (CSS2.1 §17.4.1), per its own `caption-side` — aligned with the
// TABLE box, so the table's horizontal margins have to come off first.
// `layout_table_body` applies them to the grid; without the same shift
// here a floated table with a left margin puts its caption a margin's
// width further left than the rows above it, which is exactly what
// MediaWiki's image thumbs (`margin-left: 1.4em`) show.
```

## L5695-5698 · `let (mut top_x, mut top_w) = (cx, cw);`

```
// Eine OBERE Ueberschrift steht vor dem Gitter und ist doch so breit
// wie es — also einmal die Spalten ausrechnen, bevor irgendetwas
// gemalt wird. Das kostet ein zusaetzliches Auszaehlen der Zeilen, und
// nur fuer Tabellen, die ueberhaupt eine obere Ueberschrift haben.
```

## L5715-5716 · `fn has_top_caption(&mut self, el: &'a Element, st: &ComputedStyle) -> bool {`

```
/// Hat die Tabelle eine Ueberschrift AN IHREM OBEREN Rand? Nur dann lohnt
/// der Vorablauf ueber die Spalten.
```

## L5735-5741 · `fn layout_captions(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32, bottom: bool) -> i32 {`

```
/// Lay out the caption children whose `caption-side` puts them on the
/// requested edge, stacked at `y0`. Returns the y below them. A caption is
/// recognised by `display: table-caption` as well as by the `<caption>`
/// tag — MediaWiki's image thumbs are a `figure{display:table}` with a
/// `figcaption{display:table-caption}`, and reading only the tag turns the
/// caption into stray content that widens the table instead of wrapping to
/// it.
```

## L5756-5760 · `self.path.push(self.info(e));`

```
// A caption is a block-level box of its own, not a bare run of
// children: it takes a width/height, a background and a border,
// and `position: relative` moves it like any other box (the
// caller of `layout_box` normally applies that — here that
// caller is us).
```

## L5778-5789 · `fn layout_table_body(&mut self, nodes: &'a [Node], st: &ComputedStyle, x: i32, w: i32, y0: i32) -> (i32, i32, i32) {`

```
/// The table's row grid (everything but `<caption>`): shared by a real
/// `<table>` (`el.children`, above) and an anonymous table synthesized in
/// `flow_children` around a stray run of table-part siblings that has no
/// `table`/`inline-table` ancestor (CSS2 §17.2.1) — an anonymous table
/// can't have a `<caption>` child (nothing selects an anonymous box), so
/// only the row-collection step is shared.
///
/// Gibt `(Unterkante, linke Kante, Breite)` des TABELLENKASTENS zurueck —
/// nicht des Streifens. Eine Tabelle mit `width: auto` schrumpft auf ihren
/// Inhalt (§17.5.2), und wer nur die Unterkante zurueckgibt, laesst jeden
/// Rufer raten: `getBoundingClientRect` meldete jahrelang die Streifen-
/// breite (1886 statt 157 px auf einer nackten Seite).
```

## L5795-5799 · `let bg_idx = self.ops.len();`

```
// A table with no rows is still a BOX. `width`/`height` on it are
// content-box dimensions like anywhere else, so a bordered empty
// table paints a frame of exactly that size — dropping out here
// painted nothing at all, which is what an empty `<table>` with a
// background looked like.
```

## L5816-5820 · `let (colw, table_w, off) = self.table_columns(&rows, ncols, st, w);`

```
// `fixed` tables paint each cell's own box (backgrounds/borders are the
// point of the spec). The `auto` model leaves cell decoration to the
// block boxes inside each cell — painting per-cell backgrounds/borders
// there would (without collapsed-border resolution) draw borders that
// should be hidden and swatches the reference omits.
```

## L5824-5827 · `let (btl, btt) = (px_of(st.border_left.width), px_of(st.border_top.width));`

```
// The table's own border box: border, then padding, then the row grid.
// Getting the border edge in here is what lets the table paint its own
// decoration at all — laying the grid at `x + pad_left` (no border
// offset) put every stroke a border-width off.
```

## L5838-5842 · `let frame_y = if collapse { 0.0 } else { st.pad_top + st.pad_bottom + st.border_y() };`

```
// `height` on a table is a MINIMUM for its box, never a maximum
// (CSS2.1 §17.5.3) — rows keep the height their content needs, and a
// table shorter than its `height` grows to it. The rows themselves are
// not stretched into the extra space; that is the "distribute over
// rows" part of §17.5.3 and needs per-row percentage heights first.
```

## L5852-5856 · `if collapse {`

```
// A table box paints its own background and border like any other box;
// only per-cell decoration is left to the boxes inside (see above).
// Without this a floated infobox is transparent and the article text it
// overlaps shows straight through it. Its used width comes from the
// columns it actually produced, not from the space it was offered.
```

## L5869-5874 · `fn finish_table_part(&mut self, cs: &ComputedStyle, x: i32, y: i32, w: i32, h: i32, part: SpecMark, cb_w: f32, cb_h: Opt`

```
/// Close a table row or row-group box around everything emitted since
/// `part`: its background goes behind that range, and `position: relative`
/// then moves box and content together. Rows and row groups take a
/// background but never a border — the separated model ignores border
/// properties on them (CSS2.1 §17.6.1), and the collapsed model resolves
/// every grid line at the cells.
```

## L5882-5887 · `let (z, layer) = Self::stack_key(cs);`

```
// A positioned table part is a positioned BOX (Appendix E step 8)
// like any other. Table parts are painted on their own path, so
// they were the one family of positioned boxes that never reached
// `record_stack_entry` — and an absolutely positioned sibling that
// PRECEDES one in the document then painted over it, where document
// order among two step-8 boxes says the later one wins.
```

## L5893-5900 · `fn table_columns(&mut self, rows: &[Row<'a>], ncols: usize, st: &ComputedStyle, w: i32)`

```
/// Die Spaltenbreiten, die gebrauchte Breite der Tabelle und ihr Versatz
/// im Streifen.
///
/// Steht fuer sich, weil die Antwort ZWEIMAL gebraucht wird: einmal beim
/// Auslegen und einmal vorher, weil eine `<caption>` so breit ist wie die
/// TABELLE (§17.4.1) und nicht wie der Streifen, in dem sie steht — und
/// wie breit die Tabelle ist, weiss erst das Gitter. Sie haengt allein an
/// den Zeilen und am Stil, nicht an `y`.
```

## L5904-5910 · `let (ml_len, mr_len) = (st.margin_left, st.margin_right);`

```
// The columns share the table's CONTENT box, so the space they may use
// is what is left of `w` after the table's own border and padding —
// otherwise the grid overflows the border box by exactly that much.
// Horizontal margins apply to a table box like any other block-level
// box, but the enclosing BFC branch only carries the vertical ones —
// flex and grid pick these up inside `resolve_block_h`, which a table
// can't use (its `width:auto` shrink-wraps instead of filling).
```

## L5914-5917 · `let frame = if st.border_collapse { 0 } else { (st.pad_left + st.pad_right + st.border_x()) as i32 };`

```
// In the collapsed model the table has neither padding nor a border box
// of its own (CSS2.1 §17.6.2) — the outermost cell borders ARE the
// table's frame, so the grid starts flush at the table's edge and the
// cells draw every grid line, outer ones included.
```

## L5919-5921 · `let (sx, _) = spacing_of(st);`

```
// Separated border model: `border-spacing` runs between every pair of
// columns AND once along each outer edge, so `ncols + 1` gaps come out
// of the content box before the columns share what is left.
```

## L5930-5932 · `let table_w = colw.iter().sum::<i32>() + gaps + frame;`

```
// A table's used width is known only once its columns are, so `auto`
// margins can only be resolved here (CSS2.1 §10.3.3 over §17.5.2): both
// auto centres the table, one auto pushes it to the other edge.
```

## L5938-5942 · `_ if st.center_blocks => ml + slack / 2,`

```
// Inside `<center>` a table is centred even with zero margins —
// that is what `-moz-center` does, and it is the whole reason the
// `<center><table>` idiom worked. Google's home page centres its
// search box that way, so without it a correctly sized table still
// sits hard against the left edge.
```

## L5949-5954 · `fn auto_columns(&mut self, rows: &[Row<'a>], ncols: usize, st: &ComputedStyle, w: i32) -> Vec<i32> {`

```
/// Auto table sizing (CSS2 §17.5.2.2, approximated): each column takes the
/// widest cell's *border-box* preferred width (content + that cell's
/// padding/border, or its explicit `width`). The table shrink-wraps to that,
/// shrinking columns proportionally (never below their minimum) only when
/// they overflow the available width; an explicit table `width` wider than
/// the content spreads the slack across columns.
```

## L5956-5959 · `let pct_basis = table_content_width(st, w as f32);`

```
// A cell percentage is a fraction of the TABLE, not of whatever space
// the table was offered — resolving it against the available width
// makes `width="25%"` mean a quarter of the viewport in a narrower
// table.
```

## L5963-5964 · `let mut sized = vec![false; ncols];`

```
// Columns a cell pinned with an explicit `width`. Slack belongs to
// the OTHERS — see the distribution below.
```

## L5966-5968 · `for pass_span in [false, true] {`

```
// Single-column cells define their column outright; cells spanning
// several only have to fit ACROSS them, so they run in a second pass
// once the single-span widths are known.
```

## L5994 · `#[cfg(feature = "diag-boxes")]`

```
// Dev: which column made a table too wide, and which cell drove it.
```

## L6026-6045 · `let min_total: f32 = minw.iter().sum();`

```
// **Erst jeder Spalte ihr Minimum, dann der Rest nach SPIELRAUM.**
//
// Vorher stand hier `(cap * pref[c] / total).max(minw[c])`, und
// das `.max` legte die Differenz OBEN DRAUF, statt sie der Spalte
// wegzunehmen, die noch schrumpfen kann. Eine Spalte, die nicht
// unter ihr Minimum kann — ein Bild —, machte den Tisch damit
// breiter als den Platz, den er hat.
//
// Gemessen auf Wikipedias „Today's featured picture": Bildspalte
// 404, Textspalte mit einer Vorzugsbreite von ~3040 in einem
// 1296er Kasten. Die anteilige Rechnung gab dem Bild 152, das
// `.max` hob es auf 404 — und der Tisch kam auf **1548 statt
// 1296**. Der Text lief 252 px aus seinem Kasten heraus, und weil
// die Vorlage der Seite taeglich wechselt (Bild oben ODER
// daneben), sah es aus, als passiere es „manchmal".
//
// Chromium verteilt in derselben Lage 404 | 892 — genau das, was
// unten herauskommt: Minimum sichern, den Rest im Verhaeltnis von
// `pref - minw`. Passen nicht einmal die Minima, laeuft der Tisch
// ueber; das ist dann unvermeidbar und tut jeder Browser.
```

## L6048 · `colw = minw.clone();`

```
// Nicht einmal die Minima passen — dann laeuft der Tisch ueber.
```

## L6063-6083 · `let slack = content_w - total;`

```
// No `total > 0` guard: a table whose columns all measure zero
// (every cell empty) still has to reach its specified `width` —
// otherwise `table { width: 100px }` around empty cells collapses
// to its border alone, which is what a `display: table` root with
// an empty `<body>` does.
//
// The slack goes to the columns that did NOT ask for a width
// (CSS2.1 §17.5.2.2). Spreading it over all of them widened the
// sized ones past what they asked for: `25% | auto | 25%` came out
// 41% | 18% | 41%, so the middle cell — the one holding the
// content — ended up the narrowest of the three. Only when every
// column is pinned does the slack spread across all of them,
// because then there is nowhere else for it to go.
//
// **Verteilt wird im VERHAELTNIS der Inhaltsbreiten**, nicht zu
// gleichen Teilen. Nachgemessen an Chromium: eine zweispaltige
// Tabelle mit `width:100%`, Koepfe „Eng" und „Mit Rahmen", kommt
// dort auf 518 | 1359 — genau `content_w * pref[c] / total`. Zu
// gleichen Teilen ergab 908 | 971, und damit steht die schmale
// Spalte fast so breit wie die, die den Text traegt. Das ist die
// Aufteilung, die jede `width:100%`-Tabelle des Webs betrifft.
```

## L6092-6093 · `let extra = slack / free.len() as f32;`

```
// Alle freien Spalten messen null: dann gibt es kein
// Verhaeltnis, und zu gleichen Teilen ist die einzige Antwort.
```

## L6108-6111 · `fn fixed_columns(&self, rows: &[Row], ncols: usize, st: &ComputedStyle, w: i32) -> Vec<i32> {`

```
/// `table-layout: fixed` column sizing (CSS2 §17.5.2.1): column widths come
/// from the first row's cell `width`s (each a *border-box* width), and the
/// rest of the table's used width is split equally across the remaining
/// columns; content never widens a column.
```

## L6114 · `let mut fixed: Vec<Option<f32>> = vec![None; ncols];`

```
// Per-column border-box width; None = "auto" (share the leftover).
```

## L6120-6121 · `if c >= ncols || cell_span(&sc.cell) > 1 {`

```
// A first-row cell that spans columns doesn't pin any single
// one of them (CSS2 §17.5.2.1 reads widths per column).
```

## L6135-6138 · `let content_w = if st.width == Len::Auto && auto_count == 0 {`

```
// A table is a shrink-to-fit box: with `width: auto` and every column
// pinned by the first row, the table is exactly those columns wide. It
// does NOT fill its container the way a block does — which is what put
// a 200px cell's border across the whole page.
```

## L6145-6146 · `let auto_w = if auto_count > 0 { (leftover / auto_count as f32).max(0.0) } else { 0.0 };`

```
// Remaining table space is divided equally between auto columns; if every
// column is sized, the slack is spread over all of them instead.
```

## L6158-6161 · `fn push_row_path(&mut self, row: &Row<'a>) -> usize {`

```
/// Put a row's own ancestors on the path for the duration of the work its
/// cells do, and return the depth to truncate back to. Measurement and
/// layout BOTH go through this — a cell's descendants must resolve against
/// the same ancestor chain in either, or the widths drift apart.
```

## L6173-6175 · `fn lay_table_rows(&mut self, rows: &[Row<'a>], ncols: usize, colw: &[i32], st: &ComputedStyle, x: i32, y0: i32) -> i32 {`

```
/// Lay a table's rows given resolved (border-box) column widths. Cells sit
/// side by side; each cell box stretches to the row's tallest cell and paints
/// its own background/border, with content placed inside its padding.
```

## L6177-6178 · `let (sx, sy) = spacing_of(st);`

```
// The gaps around the outside are added by the caller, which owns the
// table's padding edge; these are the ones BETWEEN cells and rows.
```

## L6182-6184 · `let grid_w = colw.iter().sum::<i32>() + sx * ncols.saturating_sub(1) as i32;`

```
// A row (and a row group) spans every column plus the spacing between
// them, but not the outer spacing the caller owns — that is the box
// its background covers and the containing block its cells see.
```

## L6186-6187 · `let mut prev_row: Vec<(ComputedStyle, i32, i32)> = Vec::new();`

```
// The previous row's (style, x, width), so a cell can resolve the grid
// line it shares with the cell above it in the collapsed model.
```

## L6189-6190 · `let mut prev_row_el: Option<(u32, ComputedStyle)> = None;`

```
// The row above, so its bottom border joins the conflict at the line
// the two rows share (only the lower cell paints that line).
```

## L6195-6197 · `let mut group: Option<(&'a Element, ComputedStyle, SpecMark, i32)> = None;`

```
// The open row group: its style, where its box and ops start, and the
// bottom of its last row so far. Rows of one group are contiguous, so
// the group closes when a row with a different one comes along.
```

## L6211-6212 · `let row_depth = self.push_row_path(row);`

```
// Pass 1: place the cells + measure the tallest one. Their styles
// were settled when the row was collected.
```

## L6214 · `let mut cells: Vec<(ComputedStyle, i32, i32, i32, i32)> = Vec::new(); // (style, cell_x, cell_w, content_x, content_w)`

```
// (style, cell_x, cell_w, content_x, content_w)
```

## L6221 · `let end = (c + cell_span(&sc.cell)).min(ncols);`

```
// A spanning cell is as wide as all the columns it covers.
```

## L6223-6224 · `let cw = colw[c..end].iter().sum::<i32>() + sx * (end - c).saturating_sub(1) as i32;`

```
// A spanning cell swallows the gaps between the columns it
// covers along with the columns themselves.
```

## L6233 · `let mut row_h = 0i32;`

```
// Row height = the tallest cell border-box (content, or explicit height).
```

## L6256-6259 · `let row_part = self.part_start();`

```
// Pass 2: emit content + paint each cell's border-box at row height.
// A positioned row (or, failing that, row group) is the containing
// block its cells' absolutely positioned descendants resolve
// against — the row's height is known now, the group's is not yet.
```

## L6266-6267 · `self.cb = (x, top, grid_w, None, None);`

```
// A row GROUP's height is not known yet, and unlike a
// positioned block there is no box to measure for it.
```

## L6282-6286 · `let cell_cb_h = self.cb_h;`

```
// A cell IS a definite containing block for its children by the
// time it is painted: the row height is resolved. Without this
// a `height: 100%` child of a `height: 100px` cell measured
// nothing at all and painted NOTHING — the commonest way a page
// fills a table cell with a coloured block.
```

## L6305-6310 · `let slack = (row_h - box_hs[c]).max(0);`

```
// `vertical-align` in the row (CSS2.1 §17.5.3). The content was
// laid out at the cell's top; middle/bottom just slide the ops
// it produced down by the leftover of the row height. `baseline`
// (the initial value) would align the cells' first-line
// baselines — we treat it as `top`, which is what it degrades
// to for equal-size single-line cells.
```

## L6321-6324 · `let above = prev_row.iter().find(|(_, px, pw)| *px < cell_x + cell_w && px + pw > *cell_x);`

```
// Each grid line is drawn exactly once, by the cell above/
// left of it, as the winner of the two borders that meet
// there. The outer lines resolve against the table's own
// border, which is why the table paints none itself.
```

## L6329-6334 · `for outer in [row.el.map(|(_, s)| s), row.group.map(|(_, s)| s)].into_iter().flatten() {`

```
// A cell is not the only box at its grid lines: its row and
// its row group meet them too (§17.6.2), which is how a
// `tr`/`tbody { border-style: hidden }` suppresses the
// borders of the cells inside it. The row's own left/right
// edges only exist at the ends of the row, and a group's
// top/bottom only where the group starts and ends.
```

## L6344-6351 · `let mut inset = Vec::new();`

```
// Auch im zusammengefassten Modell malt eine Zelle ihre
// Schatten. Hier stand nur `insert_bg`, und damit fielen
// sie weg — was genau die gestreifte Tabelle traf:
// Bootstrap streift mit `box-shadow: inset`, und sein
// Reboot setzt `border-collapse: collapse` auf JEDE
// Tabelle. Beide Wege muessen dasselbe malen, sonst
// haengt das Aussehen einer Zelle daran, welches
// Randmodell die Seite gewaehlt hat.
```

## L6357-6358 · `let half = |w: f32| (w / 2.0) as i32;`

```
// A collapsed border straddles the grid line: half of it
// falls in each of the two cells that meet there.
```

## L6379-6381 · `let (mut sdx, mut sdy) = (0, 0);`

```
// A relative cell takes its own box with it, so this has to run
// after the decoration was inserted — unlike the `vertical-align`
// slide above, which moves the content inside a cell that stays.
```

## L6394-6398 · `if let Cell::Real(e) = row.cells[c].cell {`

```
// **Der Kasten der ZELLE.** Die Tabellenteile laufen auf einem
// eigenen Pfad, der nie an `record_inspect` vorbeikam:
// `getBoundingClientRect` gab an jedem `td`, `th`, `tr`,
// `thead`, `tbody` und `tfoot` GAR NICHTS zurueck — 37 der 38
// fehlenden Kaesten der Bootstrap-Galerie waren das.
```

## L6418-6419 · `y - if rows.is_empty() { 0 } else { sy }`

```
// The trailing gap belongs BETWEEN rows, not after the last one — the
// caller adds the outer one.
```

## L6423-6424 · `fn measure_children_height(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {`

```
/// Lay an element's children to measure their flowed height without emitting
/// any draw ops (used to size table rows before painting cell boxes).
```

## L6434-6435 · `fn measure_cell_height(&mut self, cell: &Cell<'a>, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {`

```
/// Same as `measure_children_height`, for a table cell that may be an
/// anonymous box (no owning element to push on `self.path`).
```

## L6448-6449 · `fn table_role(&self, e: &Element, parent: &ComputedStyle, prev: &[ElemInfo], sib_count: u32) -> TableRole {`

```
/// Classify a table child by tag, else by its computed `display` (CSS
/// tables). Only elements are passed in.
```

## L6473-6477 · `fn collect_table_rows(&mut self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<Row<'a>> {`

```
/// Collect a table's rows (CSS2 §17.2.1 anonymous table objects), in final
/// render order: any `table-header-group` rows first, then every other row
/// (plain `<tr>`/`table-row`, `table-row-group`, and any stray content
/// coalesced into anonymous rows) in document order, then any
/// `table-footer-group` rows last — regardless of their source order.
```

## L6485-6488 · `let visible = |s: &Option<(&Element, ComputedStyle)>| s.is_none_or(|(_, st)| st.display != Display::None);`

```
// A row (or a whole row group) set to `display: none` generates no box:
// it takes no height and no column width. Dropping it here rather than
// at paint time is what keeps measurement and layout agreeing — both
// sides of the table go through this one function.
```

## L6494-6500 · `fn collect_rows_into(`

```
/// Walk `nodes` (a table's or row-group's children), bucketing each row by
/// group kind. A child that is a `table-row`/`-row-group`/`-header-group`/
/// `-footer-group` is a proper table child and recurses/becomes a row
/// directly; any other maximal run of consecutive siblings (stray cells,
/// stray text, stray elements — anything that isn't a proper table child)
/// is wrapped in ONE anonymous row (whitespace-only text neither starts
/// nor breaks a run, and is dropped if it's all a run ever contained).
```

## L6512-6515 · `let mut siblings: Vec<ElemInfo> = Vec::new();`

```
// Rows and row groups cascade like any other element: `:nth-child` on
// a `<tr>` is zebra striping, and every element child counts towards it
// — `<caption>`/`<col>` included, since they are DOM siblings even
// though they generate no row.
```

## L6534-6537 · `Some(TableRole::Row) => {`

```
// A row's cells cascade from the row, with the row on
// the path — `tbody tr td` and `td:first-child` both
// need that, and it has to hold here because this is
// where a cell's style is settled for good.
```

## L6563-6564 · `}`

```
// `<caption>`/`<col>`/`<colgroup>` generate no box and are
// fully transparent to the stray-content run around them.
```

## L6567-6568 · `let has_content = match n {`

```
// A stray cell, stray non-table element, or non-whitespace
// text: not a proper table child, so it joins the run.
```

## L6590-6598 · `fn partition_cells(&self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<StyledCell<'a>> {`

```
/// Partition a row's children into cells (CSS2 §17.2.1): a proper
/// `table-cell` child stays its own (real) cell; any other maximal run of
/// consecutive siblings (stray text, stray non-cell elements) is wrapped in
/// ONE anonymous cell. Shared by a real `<tr>`'s children and an anonymous
/// row's coalesced node run.
/// `parent` is the ROW's style: a cell inherits from its row, and an
/// anonymous cell takes the §17.2.1 anonymous-box style from it. The
/// caller must have the row on `self.path` — the cells' selectors are
/// resolved here, and measurement and layout have to agree on that path.
```

## L6623-6629 · `Some(TableRole::Skip) => {`

```
// A caption/`<col>` is a PROPER table child, so it ends the run
// of consecutive stray siblings rather than sitting inside it
// (CSS2.1 §17.2.1 wraps consecutive non-table children only).
// The anonymous cell is a contiguous slice, so leaving the run
// open here would swallow the caption's text and size the
// column to it — which is how a MediaWiki image thumb came out
// as wide as its caption instead of as wide as its image.
```

## L6661-6675 · `fn intrinsic_width(&mut self, el: &'a Element, st: &ComputedStyle) -> (f32, f32) {`

```
/// (max-content, min-content) width of a box's contents: max-content = the
/// widest line when nothing wraps, min-content = the widest unbreakable
/// word. `st` is the box's OWN resolved style — using a fixed reference
/// size here regressed nested anonymous tables (CSS2.1 §17.2.1): a cell
/// whose font differs would get a column sized at the wrong scale.
///
/// Two things the flat "concatenate every descendant's text" shortcut got
/// wrong, both of which real pages lean on:
///
/// * Out-of-flow (`absolute`/`fixed`) and `display:none` descendants
///   contribute nothing (css-sizing-3 §4). A CSS-only dropdown hangs its
///   panel off the button as an abspos child; counting it made the button
///   as wide as all 62 language names laid end to end (~4150 px).
/// * A block-level child starts its own line, so a block container's
///   max-content is its WIDEST child, not the sum of every descendant.
```

## L6680-6685 · `let out = if st.contain_size {`

```
// Size containment: the content contributes NOTHING to the box's own
// size (css-contain-2 §3.1), so both intrinsic widths come from
// `contain-intrinsic-size` — or are zero when it says nothing. This
// has to sit ahead of every content-measuring branch below, including
// the replaced one: the point of the property is that the box sizes
// as if it held a single child of exactly that size.
```

## L6690-6692 · `let iw = self.svg_size(el, st).0 as f32;`

```
// `replaced_intrinsic` reads the width ATTRIBUTE; an `<svg>` also
// has a `viewBox` and a CSS width, and `svg_size` is the one place
// that resolves all three the way the paint does.
```

## L6696-6712 · `let iw = self.img_box(el, st).0 as f32;`

```
// **Ein `<img>` stand in KEINEM der Zweige** — `replaced_intrinsic`
// fuehrt es nicht, und ein Bild hat keine Kinder. Also fiel es
// durch bis zum Textzweig und meldete NULL, bei beiden Breiten.
//
// Wo das zuschlaegt: die Eigenbreite ist die Untergrenze, unter die
// ein Flex-Element nicht schrumpft (css-flexbox-1 §4.5). Mit null
// schrumpft jedes Bild in einer engen Flexzeile auf null und wird
// von `img_box` auf EINEN Pixel geklemmt — ein 90 px hoher Strich
// von einem Pixel Breite, und daneben die Favicons als 1x16. Auf
// DuckDuckGos Ergebnisseite traf es JEDES Bild.
//
// Gemessen wird durch `img_box`, also durch genau die Funktion, die
// den Kasten danach auch legt ([[feedback_intrinsic_shared_path]]);
// `intrinsic_walk` tut es fuer ein Bild INNERHALB eines Behaelters
// seit je so, nur der Weg auf das Bild SELBST kannte die Regel
// nicht ([[feedback_the_rule_may_already_be_written_eight_lines_below]]).
// Ein Bild bricht nicht um: Min- und Max-Inhaltsbreite sind gleich.
```

## L6717-6718 · `} else if let Some(kind) = crate::forms::kind_of(el) {`

```
// A control has no text children to measure — without this it sizes to
// 0 as a flex/grid item and disappears.
```

## L6723-6728 · `let mut mst = *st;`

```
// Measure it with ITS OWN style, the way it will be painted. A
// root style here read the label at the root font size and lost
// every declared size and the page's frame, so a shrink-to-fit
// wrapper reserved 9px more than the control paints — Google's
// search button sat in a box wider than itself.
// [[feedback-intrinsic-shared-path]]
```

## L6730-6731 · `for len in [&mut mst.width, &mut mst.min_width, &mut mst.max_width] {`

```
// Percentages have no basis while measuring, so they behave as
// `auto` (css-sizing-3 §4.1) rather than resolving against 0.
```

## L6737-6759 · `let bw = self.control_box(el, &mst, kind, 0.0).w as f32;`

```
// **Diese Funktion gibt eine INHALTSbreite** — jeder Aufrufer
// legt Polsterung und Rahmen selbst wieder drauf
// (`child_outer`, `flex_metrics`, `flex_column`). `control_box`
// liefert den fertigen RANDkasten, und ihn hier ungekuerzt
// zurueckzugeben zaehlte den Rahmen zweimal: eine
// `inline-flex`-Knopfgruppe mit drei Bootstrap-Knoepfen kam
// 285 statt 205 px breit heraus, je Knopf genau seine eigenen
// 26 px Polsterung und Rahmen zu viel.
// **Diese Funktion gibt eine INHALTSbreite** — jeder Aufrufer
// legt Polsterung und Rahmen selbst wieder drauf
// (`child_outer`, `flex_metrics`, `flex_column`). `control_box`
// liefert den fertigen RANDkasten; ihn ungekuerzt
// zurueckzugeben zaehlte den Rahmen zweimal, und eine
// `inline-flex`-Knopfgruppe mit drei Bootstrap-Knoepfen kam
// 285 statt 205 px breit heraus.
//
// Abgezogen wird GENAU der Rahmen, den der Aufrufer wieder
// addiert — der aus dem Stil, nicht der wirksame aus
// `control_box`. Die UA-Mindestpolsterung eines Knopfes ohne
// eigene Polsterung kennt der Aufrufer nicht; sie hier
// mitabzuziehen machte jedes Wikipedia-Steuerelement 12 px zu
// schmal (163 Kaesten schlechter, 54 besser — gemessen, bevor
// es stehen blieb).
```

## L6766-6770 · `let push = self.path.last().map(|p| p.seq()) != Some(el.seq);`

```
// `el`'s children cascade with `el` as their parent, so it has to
// be on the ancestor path — unless a caller (the abspos path) put
// it there already. Without this their descendant selectors match
// against `el`'s parent and resolve the wrong `display`, which is
// exactly what the anonymous-table-object reftests measure.
```

## L6777-6780 · `Display::Flex | Display::InlineFlex => self.intrinsic_flex(el, st),`

```
// Ein Flex- oder Rasterkasten ist KEIN Blockcontainer: seine
// Kinder liegen nach einer anderen Regel nebeneinander, und
// sie als Blockinhalt zu messen beantwortet den falschen
// Formatierungskontext.
```

## L6788-6799 · `let ps = self.pseudo_intrinsic(el, st, PseudoElem::Before).unwrap_or(0.0)`

```
// **`::before` und `::after` stehen AUF der Zeile, also zaehlen
// sie mit.** Das Layout malt sie seit je (`pseudo_box`), die
// Messung daneben kannte sie nur am Steuerelement — und damit
// liefen die beiden Wege auseinander
// ([[feedback_intrinsic_shared_path]]). DuckDuckGos „Searches
// related to" haengt seine Lupe als `::before` an: der Kasten kam
// um deren 20 px zu schmal heraus, und der Text brach auf zwei
// Zeilen.
//
// Bei max-content addieren sie sich zum Inhalt; bei min-content
// konkurrieren sie, denn zwischen Pseudo und erstem Wort darf die
// Zeile brechen — dieselbe Regel wie fuer einen atomaren Inline.
```

## L6804-6809 · `let out = (ceil_i32(out.0) as f32, ceil_i32(out.1) as f32);`

```
// Whole pixels, rounded UP. A max-content width is a REQUIREMENT — the
// width at which the content does not wrap — so a consumer that turns
// 678.4 into a used width of 678 loses the last word to a second line.
// Floats and inline-blocks learned this and ceil themselves; flex items
// and shrink-to-fit out-of-flow boxes truncated, which is why a root
// `display:flex` wrapped text its `display:block` reference did not.
```

## L6815-6817 · `fn intrinsic_width_nodes(&mut self, nodes: &'a [Node], st: &ComputedStyle) -> (f32, f32) {`

```
/// `intrinsic_width` over a bare node slice — an anonymous cell has no
/// owning element to gather text from. `st` is the style the slice's
/// content inherits from.
```

## L6826-6830 · `fn intrinsic_table(&mut self, nodes: &'a [Node], st: &ComputedStyle) -> (f32, f32) {`

```
/// A table's own (max-content, min-content) width: each column takes its
/// widest cell, and the table is the sum of its columns. Deliberately the
/// same decomposition `auto_columns` lays out with — `collect_table_rows`
/// owns the CSS2.1 §17.2.1 anonymous-object fixup, so measuring through it
/// keeps the measurement and the layout from drifting apart.
```

## L6864-6867 · `fn intrinsic_walk(&mut self, nodes: &'a [Node], st: &ComputedStyle, run: &mut Run, pref: &mut f32, min: &mut f32) {`

```
/// Walk `nodes` as one block container's contents, accumulating inline
/// content into `run` and folding each block-level child's own measurement
/// into `pref`/`min`. `st` is the parent style the children cascade from;
/// `self.path` must already end at their parent.
```

## L6870-6878 · `let mut siblings: Vec<ElemInfo> = Vec::new();`

```
// The measure walk resolves styles the same way the LAYOUT walk does —
// with the preceding siblings and the sibling count. Passing `&[]`/`0`
// made every sibling-combinator rule (`+`, `~`) invisible to width
// measurement while layout applied it, so the two disagreed about the
// same box. Codex hides an icon-only button's label with
// `.cdx-button--icon-only span + span { position: absolute }`: layout
// took it out of flow, the measurement still counted its text, and
// Wikipedia's hamburger came out ~80px too wide — pushing the logo and
// the search box right across the whole header.
```

## L6881-6882 · `for seg in self.segment_table_runs(nodes, st) {`

```
// A stray run of table parts (rows/cells with no table ancestor) is one
// anonymous table box, measured as such — not as loose siblings.
```

## L6906-6908 · `#[allow(clippy::too_many_arguments)]`

```
/// One node of a block container's content walk (see `intrinsic_walk`).
/// `horiz` says whether this container's children sit side by side, so a
/// finished box adds to the running width instead of competing with it.
```

## L6918-6921 · `if el.tag == "br" {`

```
// A forced break ends the line even at max-content, so the text on
// either side of it never adds up. Wikipedia's infoboxes label their
// cells across two or three `<br>` lines; measuring those as one line
// made the label column ~2x too wide and squeezed the article text.
```

## L6927 · `if cs.display == Display::None || matches!(cs.position, Position::Absolute | Position::Fixed) {`

```
// Not rendered, or out of flow → contributes no intrinsic width.
```

## L6931 · `if cs.display == Display::Inline`

```
// An inline box's text joins the line its parent is building.
```

## L6944-6945 · `let (p, m) = if el.tag == "img" || el.tag == "svg" {`

```
// Everything else is a box of its own: an atomic inline (image, form
// control) or a block-level child. Either way it ends the current line.
```

## L6954-6963 · `let atomic_inline = matches!(cs.display,`

```
// An atomic inline sits ON the current line — it does not end it.
// `inline-block`, an image, a form control: all of them are
// inline-LEVEL, so their widths add to the line's the same way a word
// does. Treating them as block-level children (which is what falling
// through to `pref.max(p)` below does) measures a container of two
// inline-blocks as ONE of them wide, and they then have no room beside
// each other and stack — Google's header bar is exactly this shape.
// (Reaching here with `display:inline` means an image, a form control
// or another replaced box — the plain-inline branch above already
// returned. A FLOATED box leaves the line, so it is not one of these.)
```

## L6966-6977 · `if cs.float != FloatKind::None || (atomic_inline && cs.float == FloatKind::None) {`

```
// **Ein Float steht NEBEN der Zeile, nicht darunter** — also zaehlt er
// bei max-content zu ihr dazu, genau wie ein atomarer Inline. Vorher
// wurde er dagegen GEMAXT, und damit fiel seine Breite aus der
// Eigenbreite heraus: DuckDuckGos Kopfleiste ist ein
// schrumpfender Kasten mit einem langen Text und einem
// `float: right`-Knopf daneben, und der Kasten kam 32 px — die Breite
// des Knopfes — zu schmal heraus. Dann passte der Float nicht mehr
// und rutschte eine Zeile tiefer, unter den Text.
//
// Dass geflossene GESCHWISTER sich aufsummieren, bleibt damit richtig:
// `run.atomic` summiert. Und bei MIN-content bekommt jeder Float seine
// eigene Zeile, also konkurrieren sie dort — das tut `atomic_min`.
```

## L6993-6996 · `fn child_outer(&mut self, el: &'a Element, cs: &ComputedStyle) -> (f32, f32) {`

```
/// Was ein KIND zur schrumpfenden Breite seines Elternteils beitraegt:
/// sein MARGIN-Kasten. Herausgezogen, weil Block-, Flex- und Rasterkinder
/// dieselbe Rechnung brauchen — und eine zweite Fassung waere eine zweite
/// Semantik.
```

## L6999-7000 · `let margins = cs.margin_left.px(0.0).unwrap_or(0.0) + cs.margin_right.px(0.0).unwrap_or(0.0);`

```
// A percentage margin resolves against a width that does not exist
// yet, so it is indefinite here and contributes nothing.
```

## L7003-7004 · `match cs.width {`

```
// A definite `width` fixes the child's outer width, so that — not what
// its content would prefer — is what it contributes (css-sizing-3 §4).
```

## L7015-7017 · `fn flow_kids(&mut self, el: &'a Element, st: &ComputedStyle)`

```
/// Die in-flow-KINDER eines Kastens mit ihren Stilen — so gesammelt, wie
/// `layout_flex` und `layout_grid` es tun (Reihenfolge, Geschwisterzahl,
/// `display:none` und ausser Fluss fallen weg).
```

## L7034-7039 · `fn intrinsic_flex(&mut self, el: &'a Element, st: &ComputedStyle) -> (f32, f32) {`

```
/// Die inneren Breiten eines FLEX-Kastens. Seine Kinder sind ITEMS, kein
/// Blockinhalt: in einer Zeile stehen sie nebeneinander, also ist die
/// max-content-Breite ihre SUMME; in einer Spalte stapeln sie, also die
/// breiteste. Ohne diese Unterscheidung antwortet die Messung ueber den
/// falschen Formatierungskontext — und genau daran ist der erste Versuch
/// gescheitert, `width: fit-content` an einem Block zu ehren.
```

## L7043-7050 · `let mut anon = 0usize;`

```
// css-flexbox-1 §4: a bare text run between the children is an
// ANONYMOUS flex item. `flow_kids` reports elements only, so it counted
// for nothing here — the same gap `layout_flex` already closed on the
// LAYOUT side, and the two then disagreed about the same box:
// `<button class=flex><svg/>Speichern</button>` measured one icon wide
// and painted an `S`. `anon_text_box` lays such a run on ONE line, so
// its min-content is its max-content — keeping the two in step matters
// more than the wrap it does not do ([[feedback_intrinsic_shared_path]]).
```

## L7073-7074 · `if st.flex_wrap { min = min.max(m) } else { min += m }`

```
// Umbrechend darf jede Zeile fuer sich schmal werden; ohne
// Umbruch muessen alle Items nebeneinander passen.
```

## L7090-7096 · `fn intrinsic_grid(&mut self, el: &'a Element, st: &ComputedStyle) -> (f32, f32) {`

```
/// Die inneren Breiten eines RASTER-Kastens: die Summe seiner Spalten.
///
/// Genau, solange jede Spur eine feste Groesse hat — und das ist die Form,
/// in der die WPT-Referenzen ein Raster rahmen. Fuer `auto`/`fr` steht
/// hier der groesste Beitrag EINES Kindes je Spur; die Zuordnung Kind →
/// Spalte braeuchte die ganze Platzierung, und die laeuft erst im Layout.
/// Benannt statt verschwiegen.
```

## L7101-7104 · `let (mut col_p, mut col_m) = (alloc::vec![0.0f32; ncols], alloc::vec![0.0f32; ncols]);`

```
// Die Kinder den SPALTEN zuordnen — zeilenweise, wie die Platzierung
// im Layout, und mit `grid-column-start`, wo eines steht. Ohne diese
// Zuordnung waere die max-content-Breite „Spurenzahl mal breitestes
// Kind" und damit fuer jedes ungleiche Raster zu gross.
```

## L7117-7118 · `if span != 1 { continue }`

```
// Ein Kind ueber mehrere Spuren sagt ueber eine einzelne nichts —
// dieselbe Vereinfachung wie in der Spaltenrechnung des Layouts.
```

## L7128-7129 · `GridTrack::Pct(_) => {}`

```
// Ein Prozentsatz hat hier keine Bezugsgroesse (css-sizing-3
// §4.1) und traegt darum nichts bei.
```

## L7138 · `fn intrinsic_width_cell(&mut self, cell: &Cell<'a>, st: &ComputedStyle) -> (f32, f32) {`

```
/// `intrinsic_width`, dispatching on whether the cell is real or anonymous.
```

## L7146-7151 · `fn cell_pref_min(&mut self, cell: &Cell<'a>, cs: &ComputedStyle, avail: Option<f32>, collapse: bool) -> (f32, f32) {`

```
/// A cell's (max-content, min-content) BORDER-BOX width, honouring an
/// explicit `width` on the cell itself (CSS2.1 §17.5.2.2). `avail` is the
/// basis a percentage width resolves against, or `None` while the table's
/// own width is still being measured — a percentage is indefinite then and
/// contributes nothing. Shared by `auto_columns` (layout) and
/// `intrinsic_table` (measurement) so the two cannot drift apart.
```

## L7171-7176 · `fn segment_table_runs(&self, nodes: &'a [Node], parent: &ComputedStyle) -> Vec<TableSeg<'a>> {`

```
/// Split `nodes` into pass-through single nodes and maximal runs of
/// `table-row`/`-row-group`/`-header-group`/`-footer-group`/`-cell`
/// siblings (whitespace-only text between them doesn't break a run). A run
/// found here has no `table`/`inline-table` ancestor — `flow_children`
/// wraps it in one anonymous `table` box (CSS2 §17.2.1) instead of laying
/// each part out as an ordinary block.
```

## L7181-7187 · `let key = {`

```
// The sibling context every `table_role` here must see. Built once:
// `elems` is every element child in order, `before[i]` how many of them
// precede node `i`. Passing `&[], 0` instead (as this used to) is not
// just wrong for `:nth-child` — it also gives the cascade cache a
// second, incompatible key for the same element, and those repeat
// misses were 90 % of all repeat misses on a real page.
// Identity of this question: which node list, in which ancestor chain.
```

## L7262-7282 · `fn layout_item(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {`

```
/// Dispatch a block-level box to the right formatting context.
/// Every box-making path funnels through here — a flex item, a grid item, a
/// table cell's own box, an out-of-flow box, a float. A POSITIONED one gets
/// its stacking range recorded here rather than at each of those five call
/// sites; a caller that records one too (the flow loop, `layout_abs`,
/// `place_float`) produces a range that either equals this one or contains
/// it, and an identical nested pair orders exactly as the single range
/// does. A `position: sticky` flex item was the case that named this: it
/// reached no other recording site at all and painted under its sibling.
/// Ein Flex- oder Rasterkind auslegen.
///
/// **Ein Item hat seinen EIGENEN Formatierungskontext** (css-flexbox-1 §4,
/// css-grid-2 §6), und das ist keine Feinheit: ein Float im einen Item
/// reicht nicht in das daneben, und ein `clear` im zweiten sieht den Float
/// des ersten nicht.
///
/// Der Behaelter isoliert schon (`establishes_bfc` ist fuer `flex`/`grid`
/// wahr) — aber nur nach AUSSEN. Zwischen den Geschwistern lief die Liste
/// weiter, und auf Wikipedias Hauptseite raeumte deshalb der Float der
/// LINKEN Spalte den Clearfix der RECHTEN: „In the news" wurde 342 px zu
/// hoch (566x696 statt 531x351 in Chromium) und schob alles darunter weg.
```

## L7284-7289 · `let saved = core::mem::take(&mut self.floats);`

```
// Dieselbe Frage steht noch an zwei Stellen offen und ist dort NICHT
// gemessen: ein absolut gesetzter Kasten (`layout_abs`) und eine
// Tabellenzelle bekommen die Float-Liste ihres Rufers ebenfalls zu
// sehen. Beide legen laut Spezifikation auch einen eigenen Kontext an;
// wer das anfasst, misst es erst — hier steht die Zahl, die es
// rechtfertigt, nur fuer Flex und Raster.
```

## L7308-7312 · `if let Some(kind) = crate::forms::kind_of(el) {`

```
// A form control is atomic wherever it lands. `flow_children` and
// `collect_inline` catch the in-flow cases (so a field flows with the
// text beside it); this catches every other box-making path — flex and
// grid items, table cells, absolutely positioned controls. Real search
// boxes sit in `display:flex` rows, so missing this rendered NOTHING.
```

## L7318-7326 · `let mut dx = 0;`

```
// Here the CALLER already resolved this box: `w` is the flex item's
// main size / the grid column / the table cell, and a definite
// height is the stretched cross size. Painting the control's own
// intrinsic size instead would overlap the next item and ignore the
// stretch every grid/flex item gets by default.
// `width: auto` plus zwei `auto`-Raender: ein blockweiter Kasten
// mit EIGENER Breite, mittig gesetzt. CSS 2.1 §10.3.4 behandelt
// ein Steuerelement dabei wie einen ersetzten Kasten — die Breite
// kommt aus ihm selbst, der Rest wird gleichmaessig verteilt.
```

## L7336-7347 · `}`

```
// **Die Hoehe wird hier NICHT mehr angefasst.** Sie stand
// zuletzt als `ctl.h = st.height` da, „und sie zaehlt, wie sie
// dasteht" — nur zaehlt eine `Len::Px` an einem Steuerelement
// ohne `box-sizing: border-box` als INHALTShoehe, und
// `control_box` hat genau das schon aufgeloest. Der zweite
// Durchgang legte Polsterung und Rahmen also wieder ab: ein
// Flex-Item bekommt von `flex_item_style` die gestreckte Hoehe
// minus seinem Rahmenwerk eingetragen, und daraus wurde hier
// die ganze Hoehe. Ein `<input>` mit `padding: 6px 12px;
// border: 1px` kam in einer Flex-Zeile 20 statt 34 px hoch
// heraus — die Quer-Achse desselben Fehlers, den 0.166 auf der
// Hauptachse geschlossen hat.
```

## L7353-7359 · `if el.tag == "img" || el.tag == "svg" {`

```
// A replaced element reached through a BOX-making path: a flex or grid
// item, a table cell's own box. `flow_children` puts an `<img>`/`<svg>`
// on the line it is building — there is no line here, so nothing put
// the picture anywhere and a flex row of icons painted NOTHING at all.
// The measurement was right the whole time (the neighbours sat the
// correct distance apart, around a hole), which is exactly why it read
// as an image-decoding problem and not as a missing branch.
```

## L7380-7381 · `let boxed = Self::resolve_pct_box(st, w as f32);`

```
// The block path resolves percentage heights itself (it is entered
// directly from the flow loop too); the other three come through here.
```

## L7386-7388 · `let f0 = self.ops.len();`

```
// `flow_block_impl` applies its own `filter` over its own op range —
// doing it again here would compose the transform with itself, and an
// inversion applied twice is no inversion at all.
```

## L7401-7403 · `let (rx, rw) = used_border_box(st, x, w);`

```
// Auch hier der EIGENE Randkasten: ein Flex-Item bekommt seine
// Inhaltsbreite hereingereicht, und die aufzuzeichnen hiesse,
// `getBoundingClientRect` um die Polsterung zu belügen.
```

## L7409-7418 · `let (rx, rw) = match table_box {`

```
// **Auch hier den Kasten aufzeichnen.** Bisher taten das nur der
// Flusspfad, Floats und absolut gesetzte Kaesten — alles, was ueber
// `layout_box` kommt (Flex- und Rasterkinder, Tabellenzellen), hatte
// gar keinen. `getBoundingClientRect` gab dort NULL zurueck, und eine
// Null sieht aus wie eine Messung.
// **Der Kasten einer Tabelle ist die Tabelle, nicht ihr Streifen.**
// Hier stand `(x, w)` — die Breite, die ANGEBOTEN wurde. Eine Tabelle
// mit `width: auto` schrumpft auf ihren Inhalt, malt auch so, und
// meldete sich trotzdem 1886 px breit, wo sie 157 malt. Derselbe
// Fehler wie bei den Flexkaesten in 0.145.0, nur eine Zeile weiter.
```

## L7428-7436 · `fn layout_grid(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {`

```
/// Grid layout (css-grid-2 subset). Handles the container box model (width/
/// margins/padding/background/border/explicit height), explicit
/// `grid-template-columns`/`-rows` (px/%/fr/auto/`repeat`), `grid-auto-rows`,
/// the `grid`/`grid-template` `<rows> / <cols>` shorthand, row-major
/// auto-placement with explicit line placement (`grid-column`/`grid-row`,
/// start line + span), separate `row-gap`/`column-gap`, and item alignment
/// (`justify-items`/`align-items`/`justify-self`/`align-self`, incl. the
/// default `stretch`). Not yet: named lines/areas, `repeat(auto-fill)`,
/// dense packing, subgrid, or `align-content`/`justify-content`.
```

## L7438 · `if st.grid_ncols == 0 && st.grid_nrows == 0 {`

```
// No template at all → a grid degenerates to a block box.
```

## L7443-7445 · `let (cw, off_left) = resolve_block_h(st, w as f32);`

```
// Container horizontal box (mirrors `layout_block`) — border included,
// same as the flex container: `width` is the CONTENT box, the border
// box is that plus padding and border.
```

## L7466 · `let px_h = |len: Len| content_height_of(st, len).map(|v| v as i32);`

```
// Explicit / min / max height clamp the content-box height.
```

## L7485-7486 · `fn grid_content(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {`

```
/// Lay a grid container's items inside its content box `(x, w, y0)`, returning
/// the content-box height. `self.cb` is already set for the container.
```

## L7488-7489 · `let mut tracks: Vec<GridTrack> = st.grid_tracks[..st.grid_ncols as usize].to_vec();`

```
// Column tracks, expanding any `repeat(auto-fill/auto-fit, …)` to fill the
// container width.
```

## L7520 · `let count = (((avail - fixed + g) / per_rep) as i64).max(1) as usize;`

```
// `as i64` truncates toward zero (= floor for the positive ratio).
```

## L7536-7537 · `tracks.push(st.grid_auto_cols);`

```
// Rows-only grid → one IMPLICIT column, which `grid-auto-columns`
// sizes exactly as `grid-auto-rows` sizes an implicit row.
```

## L7542 · `let mut items: Vec<(&Element, ComputedStyle)> = Vec::new();`

```
// Grid items = in-flow child elements; abspos children are out of flow.
```

## L7558-7561 · `abs_items.push((ce, cs));`

```
// Deferred: a positioned child that names a grid line has
// that GRID AREA as its containing block (css-grid §9), and
// no track has a size yet. One that names none keeps the
// container's padding box.
```

## L7569-7570 · `let col_gap = st.grid_col_gap.px(w as f32).unwrap_or(0.0);`

```
// A percentage gap resolves against the container's own content box
// on that axis; an indefinite height makes a percentage row-gap zero.
```

## L7577 · `let def_h: Option<f32> = content_height_of(st, st.height);`

```
// Definite container content height (for %/fr row resolution).
```

## L7580-7581 · `let resolve_col = |line: i16| -> usize {`

```
// — placement — (col, colspan, row, rowspan) per item, row-major flow
// honouring explicit `grid-column`/`grid-row` start lines + spans.
```

## L7591-7592 · `let resolve_row = |line: i16| -> usize { (line.max(1) as usize) - 1 };`

```
// Row line → 0-based row index; the row count is not yet known, so
// negative lines (relative to the last row) fall back to the first row.
```

## L7608-7609 · `if s.grid_area != 0 {`

```
// Named `grid-area` placement (from the container's grid-template-areas)
// takes priority over line/auto placement.
```

## L7684-7685 · `let avail = w as f32;`

```
// — column sizing — fixed/% direct, `auto` = max-content of single-span
// items, `fr` splits the leftover.
```

## L7729-7730 · `let cell_width = |c: usize, span: usize| -> f32 {`

```
// Per-item content-box horizontal placement (needed before measuring the
// natural height at that width).
```

## L7738 · `let mut hpos: Vec<(f32, f32)> = Vec::with_capacity(items.len()); // (ix, iw)`

```
// (ix, iw)
```

## L7771 · `let nrows = occ.len().max(st.grid_nrows as usize);`

```
// — row sizing —
```

## L7811 · `if let Some(dh) = def_h {`

```
// `fr` rows share the container's leftover definite height.
```

## L7839 · `for (i, (el_i, s)) in items.iter().enumerate() {`

```
// — final item placement with cross-axis alignment / stretch —
```

## L7856-7865 · `let bottom = self.layout_item(el_i, &s2, ix as i32, (iw as i32).max(1), cell_y);`

```
// NOTE: css-grid-2 §6.6 says a grid item's percentage height
// resolves against its GRID AREA, and the row tracks are sized by
// now, so it could be answered right here — `self.cb_h =
// Some(cell_h)` guarded on the spanned rows being definite. It
// MEASURES WORSE: 4052 against 4056 for keeping the container's
// content height. Guarding on definite tracks changed nothing, so
// the difference is not the circularity — something downstream
// (the `align-self: stretch` branch above already gives an
// auto-height item the row's height) compensates for the coarser
// answer. Parked with the number rather than taken on faith.
```

## L7879-7885 · `let prev_cb = self.cb;`

```
// — positioned children, now that every track has a size —
//
// css-grid §9: an absolutely positioned child whose `grid-row`/
// `grid-column` names lines is contained by that GRID AREA; on an axis
// where it names none, the containing block stays the grid container's
// padding box. The two axes are decided separately, which is why this
// is not one `if`.
```

## L7922-7929 · `fn cb_height(&mut self) -> Option<i32> {`

```
/// Lay a box just to measure its natural height, discarding the emitted ops
/// (used for grid auto-row sizing before the real placement pass).
/// The positioned containing block's height, running the deferred recipe
/// (`PendingCbH`) on first read and caching the answer into `cb.3`.
///
/// Everything that resolves a percentage against the containing block's
/// height goes through here rather than reading `cb.3` — that is what
/// makes the deferral invisible.
```

## L7939-7940 · `if self.measuring_cb_h.get() {`

```
// The same guard the eager version had: measuring the box re-enters it
// and asks for its own height again. One level is all the answer needs.
```

## L7945-7948 · `let p = &self.cb_pend[idx];`

```
// Restore the context the measurement would have run in eagerly: it
// happens now from somewhere inside the box's own subtree, where the
// ancestor path is longer and the containing block, its height and the
// active floats have all moved on.
```

## L7963-7964 · `let used = Some((h - border_y).max(0));`

```
// `measure_box_height` returns the BORDER-box height; the containing
// block is the PADDING box, so the two borders come off.
```

## L7971-7972 · `fn measured_h(&mut self, site: u8, arg: f32, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y: i32) -> i32 {`

```
/// `measure_box_height` through the `measured` memo. `site` and `arg`
/// identify which question is being asked — see `MeasureKey`.
```

## L7982-7984 · `cb_def_h: match (self.cb.3, self.cb.4) {`

```
// Not resolved on purpose: a pending containing block is
// identified by its recipe index, so two different ones cannot
// share a key without either being measured.
```

## L8000-8002 · `fn flex_mark(&self) -> FlexMark {`

```
/// Everything one speculative flex-item placement can move — exactly the
/// state `measure_box_height` rolls back, so keeping a placement instead of
/// discarding it is the only difference between the two.
```

## L8012 · `fn spec_mark(&self) -> SpecMark {`

```
/// Where every recorded vector stands right now.
```

## L8028-8039 · `fn spec_rollback(&mut self, m: &SpecMark) {`

```
/// Drop everything a discarded layout recorded.
///
/// Stacking ranges index into `ops`/`links`, so a speculative run has to
/// drop the ones it recorded too — otherwise they survive pointing into a
/// vector that was truncated behind them, and `reorder_by_z` (which needs
/// disjoint ascending ranges) slices the real display list at the wrong
/// offsets. Floats live on past the box that placed them, so a leak puts
/// exclusion rects into the real layout: the next float finds a BFC that
/// looks full and drops below phantom neighbours. And `hover_boxes` /
/// `inspects` are hit-test geometry — a trial run records them at trial
/// COORDINATES, so the pointer lights up an element the page never painted
/// there.
```

## L8064-8070 · `fn layout_flex(&mut self, el: &'a Element, st: &ComputedStyle, x: i32, w: i32, y0: i32) -> i32 {`

```
/// Flex layout (css-flexbox-1 subset): row/column direction, the container's
/// own box model (width/margins/padding/background/border/explicit height,
/// establishing the containing block), `flex-grow`/`-shrink`/`-basis` with
/// the automatic content minimum size, per-item margins (incl. `margin:auto`
/// on the main axis), `gap`, `justify-content`, `align-items`/`align-self`
/// (start/center/end/stretch), and `flex-wrap` (multi-line). Not yet:
/// reverse directions, `align-content`, baseline alignment.
```

## L8072-8075 · `let mut items: Vec<(Kid<'a>, ComputedStyle)> = Vec::new();`

```
// Ein nackter Textlauf zwischen den Kindern ist laut css-flexbox-1 §4
// ein ANONYMER Flex-Kasten — er verschwand hier bisher spurlos, weil
// die Schleife unten nur Elemente aufsammelte. `<div class="flex">Label
// <span>x</span></div>` verlor sein „Label".
```

## L8077-8079 · `let mut items: Vec<(Kid<'a>, ComputedStyle)> = Vec::new();`

```
// Flex items = in-flow child elements; abspos children are out of flow.
// Structural selectors count EVERY element sibling, so the position is
// tracked independently of which children become items.
```

## L8085-8086 · `if t.trim().is_empty() {`

```
// Nur Laeufe mit sichtbarem Inhalt: reiner Leerraum zwischen
// zwei Kaesten erzeugt keinen Kasten (§4).
```

## L8099 · `if matches!(cs.display, Display::Inline | Display::InlineBlock | Display::InlineFlex) {`

```
// A flex item is blockified (css-display-3 §2.7).
```

## L8115-8125 · `let lead_box = self.pseudo_box(el, st, PseudoElem::Before, w);`

```
// A `::before`/`::after` with a box of its own is a flex item like any
// other child (CSS Display 3 §2.2). It is a fixed rectangle and never a
// flexible length, so instead of threading a second item KIND through
// the whole §9.7 machinery it is reserved off the main axis here and
// the real items share what is left. Exact for the idiom this serves —
// a `content: ""` box with a definite width.
// Only the LEADING one: a trailing box would have to sit right behind
// the last item, and reserving it off the axis puts it at the
// container's far edge instead (`flexbox_generated` measures exactly
// that gap). The icon-before-content idiom this serves needs the lead;
// the tail waits until generated content is a real flex item.
```

## L8128 · `if items.is_empty() && lead_box.is_none() && tail_box.is_none() {`

```
// Empty flex box: fall back to block so its own box decoration still paints.
```

## L8132 · `items.sort_by_key(|(_, s)| s.order); // stable → equal order keeps DOM order`

```
// stable → equal order keeps DOM order
```

## L8134-8137 · `let (cw, off_left) = resolve_block_h(st, w as f32);`

```
// Container horizontal box (mirrors `layout_block`/`layout_grid`). The
// border counts: a flex container with `width: 40em; border: 1px` has a
// 642px border box like any other block, and leaving it out made every
// bordered flex container two pixels narrow AND shifted its content.
```

## L8147-8148 · `let row = st.flex_row;`

```
// Along the main axis in a row. `content_x`/`content_w` shrink around
// the generated boxes so the real items never overlap them.
```

## L8162 · `let def_h: Option<f32> = content_height_of(st, st.height);`

```
// Definite container content height (for cross-stretch / main-axis flex).
```

## L8164-8173 · `let min_h: Option<f32> = content_height_of(st, st.min_height);`

```
// **`min-height` bestimmt die Quer-Groesse genauso.** Ein Behaelter mit
// `min-height: 100vh` und `align-items: center` hat 993 px Platz, in
// denen er mittig setzen kann — bisher sah das Layout nur `height`,
// fand nichts, und der Kasten klebte oben. Dasselbe Loch liess
// `min-height: 100%` an einem Kind gegen NICHTS rechnen: zwei
// WPT-Tests und die Fritzbox-Anmeldung, ein Fehler.
//
// Es ist ein BODEN, kein Ersatz: ist der natuerliche Inhalt hoeher,
// gilt der. Deshalb reicht ein Durchgang — der Boden steht vorher
// fest, und die Zeile nimmt das Maximum.
```

## L8175-8178 · `let max_h: Option<f32> = content_height_of(st, st.max_height);`

```
// Und `max-height` ist derselbe Satz von der anderen Seite: eine
// DECKE. Ohne sie lief ein Kind auf 9999 px, obwohl der Behaelter auf
// 100 gedeckelt war — der Deckel griff erst ganz am Ende an der
// Kastenhoehe, nicht an dem, was die Zeile ihren Kindern gibt.
```

## L8190-8193 · `self.cb_h = prev_cb_h;`

```
// Place the generated boxes now that the line's height is known: on the
// main axis in a row (centred on the cross axis, which is what
// `align-items: center` does for the icon idiom), stacked before and
// after the content in a column.
```

## L8216 · `let px_h = |len: Len| content_height_of(st, len).map(|v| v as i32);`

```
// Explicit / min / max height clamp the content-box height.
```

## L8235-8237 · `fn flex_row(`

```
/// Row flex (main axis = horizontal, cross axis = vertical). `def_cross` is
/// the container's definite content height (cross size) if any. Returns the
/// content-box height consumed by all lines.
```

## L8246-8247 · `min_cross: Option<f32>,`

```
// Der Boden aus `min-height` — bestimmt wie `def_cross`, aber er
// ERSETZT die natuerliche Groesse nicht, er hebt sie nur an.
```

## L8249 · `max_cross: Option<f32>,`

```
// Und die Decke aus `max-height`.
```

## L8253-8256 · `let main_gap = st.grid_col_gap.px(avail).unwrap_or(0.0);`

```
// Row flex: the MAIN axis is horizontal, so the gap between items is
// `column-gap` and the gap between wrapped lines is `row-gap`. Reading
// one value for both made `gap: 10px 20px` put the row gap between the
// items instead of between the lines.
```

## L8260-8262 · `let m = self.flex_metrics(items, avail, true, def_cross.or(min_cross));`

```
// — per-item metrics (content-box main size = width) —
// Die Prozentbasis der Kinder ist die BENUTZTE Quer-Groesse — und die
// steht auch fest, wenn sie aus `min-height` kommt.
```

## L8265 · `let lines = flex_break_lines(&m, avail, main_gap, st.flex_wrap, st.flex_balance);`

```
// — line breaking (flex-wrap) —
```

## L8268-8274 · `let n_lines = lines.len();`

```
// `align-content` packs the LINES in whatever cross space the container
// has left over, so every line's cross size must be known before the
// first line is placed. A multi-line container therefore lays its lines
// out once to measure them, throws that placement away, and does it
// again where they really go. Single-line containers keep the one-pass
// path: align-content has no effect on them (css-flexbox-1 §8.4), and
// neither does the packing default `start`.
```

## L8290-8293 · `let free = cross - nat.iter().sum::<i32>() as f32 - gaps;`

```
// NOT clamped at zero: the default overflow behaviour is `unsafe`
// (css-align-3 §5.3), so `center` on lines that do not fit spills
// equally out of both ends rather than piling up at the start.
// Only the distributions have a spec'd fallback, below.
```

## L8297-8299 · `ContentAlign::Stretch if free > 0.0 => {`

```
// Every line grows by an equal share. The share is taken
// CUMULATIVELY so the integer rounding cannot drift: the last
// line still ends exactly on the container's content edge.
```

## L8313-8314 · `offset_cross = (free / 2.0) as i32;`

```
// Both fall back to `center` when the lines overflow
// (css-align-3 §5.4), not to `start`.
```

## L8325-8326 · `ContentAlign::Start | ContentAlign::Stretch | ContentAlign::Between => {}`

```
// `stretch` cannot shrink a line and `space-between` piles up
// at the start — both are `flex-start` once the space is gone.
```

## L8347-8351 · `#[allow(clippy::too_many_arguments)]`

```
/// Lay one flex line out at `cross_y` and return the cross size it used.
/// `forced_cross` is the size the line was GIVEN (a single line filling a
/// definite container, or a share handed out by `align-content: stretch`);
/// `None` means it sizes to its tallest item, which is also what the
/// measuring pass asks for.
```

## L8364 · `min_cross: Option<f32>,`

```
// `min-height` des Behaelters, wenn diese Zeile die einzige ist.
```

## L8366 · `max_cross: Option<f32>,`

```
// dito `max-height`.
```

## L8374 · `let size = resolve_flex_line(li, avail, gaps_total);`

```
// Resolve flexible lengths within this line's available main size.
```

## L8377-8378 · `let used: f32 = li`

```
// Leftover main space → justify-content, unless main-axis auto margins
// absorb it.
```

## L8402 · `let mut item_x = alloc::vec![0.0f32; ln];`

```
// Main-axis positions (border-box left) per item.
```

## L8413-8422 · `let mut h_nat = alloc::vec![0i32; ln];`

```
// Natural cross size (height) at the resolved width, to size the
// line — laid out AT the spot the item will most likely keep, and
// the ops KEPT instead of discarded.
//
// Measuring a flex item already lays its whole subtree out; the old
// code threw that away and then laid the identical thing again, so
// every nesting level doubled the work (2^5 on MediaWiki's header).
// Counted on real pages, 93–96 % of items end up at exactly
// `(item_x[k], cross_y)` with their natural height, so the second
// pass was almost always a byte-for-byte repeat of the first.
```

## L8428-8433 · `let box_main = (size[k] + li[k].main_pad).max(0.0) as i32;`

```
// **Kein Mindestpixel.** Ein Flexkasten darf leer sein, und die
// Eigenbreite daneben rechnet auch mit 0 — eine 1 hier laesst die
// beiden Wege auseinanderlaufen. Auf DDGs „Searches related to"
// war das ein `&ZeroWidthSpace;` als anonymes Element: es nahm
// dem Text neben sich genau ein Pixel weg, und der brach damit
// auf zwei Zeilen ([[feedback_intrinsic_shared_path]]).
```

## L8443 · `Kid::Anon(b) => {`

```
// Ein anonymer Kasten ist schon fertig — er wird nur gelegt.
```

## L8450-8455 · `self.floats.truncate(mark.spec.floats);`

```
// Keep the ops, but NOT the ambient state a discarded
// measurement used to drop: a flex item is its own block
// formatting context, so its floats must not reach its
// siblings, and the containing block it installed is gone with
// it. Leaving those standing made every later item on the line
// flow around phantom exclusions.
```

## L8461-8469 · `let nat_line = (0..ln)`

```
// Line cross size: a single unwrapped line fills a definite container
// height; otherwise it's the tallest item margin box.
//
// The line is sized from each item's HYPOTHETICAL cross size
// (Flexbox §9.4 step 7) — the natural one clamped by its own
// `min-`/`max-height`. Using the raw natural height left the line
// short of any item held open by a `min-height`, and that item then
// hung out past the container: Wikipedia's search button is
// `min-height: 32px` inside a 30px-tall line.
```

## L8477-8479 · `let line_cross = clamp_cross(`

```
// `height` SETZT die Zeile (ein zu kleiner Wert laesst die Inhalte
// ueberlaufen), `min-height` HEBT sie nur an — die Reihenfolge, die
// css-sizing-3 §5 fuer die Klammerung vorschreibt.
```

## L8486-8490 · `let mut first_redo = ln;`

```
// Now that the line's cross size is known, work out where each item
// really goes, and find the FIRST one the speculative pass got
// wrong. A forced height counts as wrong even when the number
// matches: it changes the derived style, so the subtree below can
// resolve differently.
```

## L8497-8500 · `let cross_m_auto = li[k].cm_lead_auto || li[k].cm_trail_auto;`

```
// An `auto` cross margin eats the line's free cross space, and
// that overrides both `align-self` and the stretch (css-flexbox-1
// §8.1 / §9.4 step 11) — `mt-auto` on a row item means bottom, no
// matter what the container aligns to.
```

## L8516 · `_ => cross_y + li[k].cm_lead as i32, // start / stretch-with-def-size / baseline`

```
// start / stretch-with-def-size / baseline
```

## L8526-8528 · `if first_redo < ln {`

```
// `ops` is one sequential list, so redoing item k means dropping
// everything emitted from k onward — hence the first mismatch, not
// each one. Worst case this is exactly the two passes it replaced.
```

## L8536-8537 · `if let Kid::Anon(b) = kid { self.place_atomic(b, item_x[k] as i32, y); }`

```
// Der anonyme Kasten aendert sich durch die zweite Runde
// nicht — er hat keine Kinder, die anders fielen.
```

## L8542-8546 · `let box_main = (size[k] + li[k].main_pad).max(0.0) as i32;`

```
// `layout_box` takes the box the caller resolved — the
// item's BORDER box. `size[k]` is its content size, so the
// item's own padding and border have to go back on, or a
// control (which paints exactly this width) loses them and
// clips its label.
```

## L8556-8557 · `fn flex_column(`

```
/// Column flex (main axis = vertical, cross axis = horizontal). `def_cross`
/// is the container's definite content height (main size) if any.
```

## L8568 · `let avail = w as f32; // cross-axis available (width)`

```
// cross-axis available (width)
```

## L8569-8570 · `let gap = st.grid_row_gap.px(def_cross.unwrap_or(0.0)).unwrap_or(0.0);`

```
// Column flex: the main axis is vertical, so `row-gap` separates the
// items, resolved against the container's own definite height.
```

## L8573-8574 · `let mut cross_w = alloc::vec![0.0f32; n];`

```
// Cross-axis (horizontal) width + position, plus main-axis (vertical)
// margins, per item. Cross axis is never flexed (grow/shrink are main).
```

## L8583-8587 · `let pad_h = s.pad_left + s.pad_right + s.border_x();`

```
// Polsterung UND Rahmen: `flex_item_style` legt beide wieder auf
// eine Inhaltsbreite drauf, also muss der Weg hierher beide
// abziehen. Nur die Polsterung abzuziehen machte jedes gerahmte
// Item um seinen Rahmen zu breit — dieselbe Zwillingsrechnung, die
// `flex_metrics` daneben schon richtig hat.
```

## L8594-8597 · `let ml_auto = matches!(s.margin_left, Len::Auto);`

```
// Cross axis of a column = horizontal. An `auto` margin there takes
// the free width and cancels the stretch (css-flexbox-1 §9.4 step
// 11) — without that, `mx-auto` on a stretched item has nothing
// left to centre and reads as ignored.
```

## L8602-8608 · `to_content((avail - ml - mr).max(1.0)).max(1.0)`

```
// **Gestreckt wird der AUSSENkasten** (css-flexbox-1 §9.4
// Schritt 11): der Randkasten des Items fuellt die Querachse,
// seine Inhaltsbreite ist um Polsterung und Rahmen kleiner.
// Ohne `to_content` bekam JEDES Kind eines gepolsterten
// Flex-Items die Breite des Elternrandkastens — auf der
// Bootstrap-Galerie 32 px zu viel an jedem Kartenrumpf, jedem
// Dialogrumpf und jedem Listeneintrag.
```

## L8624-8626 · `let bw = wd + pad_h;`

```
// Ausgerichtet wird der RANDkasten, nicht der Inhalt: `wd` ist
// eine Inhaltsbreite, also gehoert die Polsterung fuer jede
// Rechnung mit freiem Platz wieder drauf.
```

## L8642 · `_ => x + ml as i32, // start / stretch`

```
// start / stretch
```

## L8656 · `let gaps_total = gap * (n as f32 - 1.0).max(0.0);`

```
// Total intrinsic main size (heights + vertical margins + gaps).
```

## L8659-8671 · `let main_size: Option<Vec<f32>> = def_cross.map(|avail| {`

```
// **Die Hauptachse einer Spalte ist die HOEHE, und sie flext.**
//
// Bisher tat sie das nicht: diese Funktion mass die natuerlichen Hoehen
// und verteilte den Rest nur ueber `justify-content` und Auto-Raender.
// `flex-grow` hatte auf der Hauptachse einer Spalte KEINE Wirkung — und
// `display:flex; flex-direction:column` mit einem `flex:1`-Kind ist das
// haeufigste App-Layout des Webs. Auf sandbox.nopeek.ch bekam die
// Inhaltsflaeche dadurch ihre Inhaltshoehe statt der Fensterhoehe, und
// alles darunter sass 224 px zu hoch.
//
// Nur bei DEFINITER Hauptgroesse: ohne sie waechst der Behaelter selbst
// mit dem Inhalt, es gibt keinen freien Platz, und `grow` ist per
// Spezifikation wirkungslos.
```

## L8679-8680 · `let base = match s.flex_basis {`

```
// Die Grundgroesse: `flex-basis`, sonst eine definite
// `height`, sonst die gemessene Inhaltshoehe.
```

## L8687-8692 · `let floor = match vert_len(s.min_height, Some(avail as i32)) {`

```
// **Die automatische Mindestgroesse** (css-flexbox-1 §4.5):
// `min-height: auto` an einem Flex-Item ist seine
// INHALTSgroesse, nicht null — deshalb schrumpft ein
// `<select>` in einer 0 px hohen Spalte nicht weg. Nur
// solange der Inhalt sichtbar ueberlaeuft: ein Rollkasten
// ist dafuer gemacht, geklemmt zu werden, und hat keine.
```

## L8695-8698 · `None if !s.overflow_y.scrolls() => {`

```
// Das MINIMUM aus Inhalts- und angegebener Groesse,
// wie `flex_metrics` es fuer die Zeile schon rechnet:
// wer eine Hoehe nennt, die kleiner ist als sein
// Inhalt, hat sie so gemeint.
```

## L8726-8727 · `let outer = |i: usize| -> f32 {`

```
// Was die Zeilen unten wirklich belegen — geflext, wo es eine
// Hauptgroesse gibt, sonst wie gemessen.
```

## L8736 · `let free = def_cross.map(|c| c - intrinsic).unwrap_or(0.0).max(0.0);`

```
// A definite container height gives free main space → justify-content.
```

## L8738-8741 · `let n_auto: usize = (0..n).map(|i| ma_lead[i] as usize + ma_trail[i] as usize).sum();`

```
// Main-axis auto margins take the free space FIRST; `justify-content`
// only ever sees what they leave (css-flexbox-1 §8.1). This is what
// makes `mt-auto` on the last child of a fixed-height column pin it to
// the bottom — the card-footer pattern.
```

## L8760-8761 · `let s2 = flex_item_style(s, main_size.as_ref().map(|v| v[i]), Some(cross_w[i]), false);`

```
// Die Hauptgroesse wird ERZWUNGEN, wo der Behaelter eine hat;
// die Quergroesse ist die gestreckte Breite von oben.
```

## L8768-8770 · `b.max(y as i32 + outer(i) as i32)`

```
// Ein Kasten mit erzwungener Hauptgroesse belegt genau sie,
// auch wenn sein Inhalt kuerzer ist — sonst wandert alles
// darunter nach oben.
```

## L8790-8795 · `fn flex_metrics(&mut self, items: &[(Kid<'a>, ComputedStyle)], avail: f32, row: bool,`

```
/// Per-item flex metrics on the main axis (row: width; column: width used as
/// cross). `row` selects which margins/paddings are the main vs cross axis.
///
/// `def_cross` ist die DEFINITE Quer-Groesse des Behaelters, oder `None`,
/// wenn sie sich erst aus dem Inhalt ergibt. Sie ist die einzige zulaessige
/// Grundlage fuer ein Prozent auf der Quer-Achse — siehe unten.
```

## L8800-8802 · `let (main_pad, cross_pad) = if row {`

```
// Padding AND border on each axis: every consumer below adds
// `main_pad` to a CONTENT size to get a border box, so leaving the
// border out makes each of them short by it.
```

## L8808 · `let (m_lead_len, m_trail_len) = if row {`

```
// Main-axis leading/trailing margins (row: left/right; column: top/bottom).
```

## L8814-8817 · `let h_lead_auto = matches!(s.margin_left, Len::Auto);`

```
// An `auto` margin is free space on ITS axis, so which of the four
// counts as main and which as cross flips with the direction. The
// vertical pair carries its keyword beside the number, because in
// normal flow it is used as zero.
```

## L8832 · `let (cm_lead, cm_trail) = if row {`

```
// Cross-axis margins.
```

## L8838 · `let (main_size, min_size, max_size) = if row {`

```
// The item's main-axis size property (row: width; column: height).
```

## L8842 · `(s.width, s.min_width, s.max_width) // column cross axis is width`

```
// column cross axis is width
```

## L8846-8859 · `let (pref, minc) = self.kid_intrinsic(el, s);`

```
// `intrinsic_width` reports the element's CONTENT width — its own
// padding and border are added by whoever lays it out, and that
// holds for a CONTROL too: `control_box` hands back the finished
// border box, and `intrinsic_width` already takes the CSS frame
// back off (0.145.0, the `inline-flex` button group).
//
// **Hier stand dieselbe Subtraktion ein zweites Mal.** Sie war
// richtig, solange `intrinsic_width` den Randkasten ungekuerzt
// durchreichte (0.66.0); seit 0.145.0 zieht sie denselben Betrag
// ein zweites Mal ab, und `resolve_flex_line` legt ihn nur EINmal
// wieder drauf. Jedes Steuerelement in einem Flex-Container kam so
// um genau seine Polsterung plus Rahmen zu schmal heraus — ein
// Bootstrap-Knopf 48 statt 74 px, jede `.input-group`, jede
// `.modal-footer`, jede `.navbar`-Suchzeile.
```

## L8866-8869 · `let scrolls = if row { s.overflow_x.scrolls() } else { s.overflow_y.scrolls() };`

```
// Automatic minimum size = min(content min, specified suggestion) —
// but only while the item's overflow is `visible` on the main axis.
// A scroll container has no automatic minimum (css-flexbox-1 §4.5):
// its content is meant to be clipped, so it may shrink to nothing.
```

## L8877 · `let cross_auto = if row {`

```
// Cross-axis stretch is possible only when the cross size is auto.
```

## L8883-8893 · `let (min_cross, max_cross) = if row {`

```
// **Die Quer-Achse hat eine EIGENE Grundlage.** Hier stand `avail`
// — und das ist die HAUPT-Achse, bei einer Zeile also die Breite.
// `min-height: 100%` an einem Flex-Element wurde damit zu 100 %
// der BREITE des Behaelters: ein 328 px breites Eingabefeld war
// 330 px hoch statt 40.
//
// Richtig ist die definite Quer-Groesse des Behaelters, und wenn
// es keine gibt, gilt das Prozent als nicht aufloesbar: `min-`
// faellt auf 0, `max-` auf „keins" (CSS 2.1 §10.7 — genau die
// Regel, die `resolve_pct_heights` fuer den Flussfall schon
// anwendet).
```

## L8928-8935 · `fn shift_ops(&mut self, m: &SpecMark, dx: i32, dy: i32) {`

```
/// Shift everything recorded since `m` by `(dx, dy)` — used to place a
/// flex item on the cross axis, and to offset a `position:relative` box
/// after it is laid in flow.
///
/// It takes the whole mark rather than a few explicit indices so that a
/// side table added later cannot be forgotten here: hit rects that do not
/// follow their painted box put the pointer where the box used to be, and
/// `hover_boxes` shipped in 0.25.0 with exactly that defect.
```

## L8949-8950 · `DrawOp::BgImage { x, y, clip, .. } | DrawOp::Gradient { x, y, clip, .. } => {`

```
// Wie in `translate_op_list`: `clip` ist absolut, nicht
// relativ zum Kasten.
```

## L8963-8965 · `for c in &mut self.controls[m.controls..] {`

```
// Hit rects must follow their painted box (relative offsets, flex
// cross-alignment) or the click — or the pointer — lands where the box
// used to be.
```

## L8973-8975 · `b.paint.0 += dx;`

```
// The anchor names an op inside this very range, so it moves with
// it — a shifted box pointing at an unshifted anchor would look up
// an op that no longer exists.
```

## L8990-8992 · `struct FlexItem {`

```
/// Resolved main-axis metrics for one flex item (all content-box px). Margins/
/// paddings are split into the main axis (`m_*`, `main_pad`) and cross axis
/// (`cm_*`, `cross_pad`) by the caller's direction.
```

## L9004 · `hypo: f32, // base clamped to [floor, ceil]`

```
// base clamped to [floor, ceil]
```

## L9011 · `cross_auto: bool, // cross size is auto → eligible for stretch`

```
// cross size is `auto` → eligible for stretch
```

## L9016-9017 · `fn flex_pack_lines(m: &[FlexItem], cap: f32, gap: f32) -> Vec<(usize, usize)> {`

```
/// Greedily fill lines until the next item's outer main size would overflow
/// `cap` (at least one item per line).
```

## L9040-9043 · `fn flex_break_lines(m: &[FlexItem], avail: f32, gap: f32, wrap: bool, balance: bool) -> Vec<(usize, usize)> {`

```
/// Partition items into flex lines. Without `wrap`, one line holds everything.
/// With `wrap`, greedily pack to `avail`. With `balance` (css-flexbox-2), pack
/// into the fewest lines, then shrink the line capacity to the smallest value
/// that still fits that many lines — evening the items across the lines.
```

## L9069-9071 · `fn resolve_flex_line(li: &[FlexItem], avail: f32, gaps_total: f32) -> Vec<f32> {`

```
/// Resolve flexible lengths for one line: grow into positive free space (by
/// `flex-grow`) or shrink out of negative free space (by `flex-shrink × base`),
/// each clamped to `[floor, ceil]`. Returns the used content main size per item.
```

## L9074-9076 · `let fixed: f32 = li.iter().map(|it| it.m_lead + it.m_trail + it.main_pad).sum::<f32>() + gaps_total;`

```
// Margins, padding and borders do not flex: take them out once, and let the
// content boxes share what is left. Leaving them in handed the line every
// item's padding as extra free space.
```

## L9081 · `let hypo_sum: f32 = li.iter().map(|it| it.hypo).sum();`

```
// §9.7.1 — grow or shrink is decided once, from the hypothetical sizes.
```

## L9086-9088 · `let mut target: Vec<f32> = Vec::with_capacity(n);`

```
// §9.7.2 — freeze the inflexible ones straight away: no flex factor, or a
// base size already past the hypothetical size in the direction we would
// move it (min/max already had the last word there).
```

## L9097-9098 · `let free_of = |target: &[f32], frozen: &[bool]| -> f32 {`

```
// §9.7.3 — free space counts frozen items at their target and the rest at
// their flex base size.
```

## L9104-9107 · `for _ in 0..n {`

```
// §9.7.4 — the loop. Every round freezes at least one item, so `n` rounds
// always finish. This is the part a single pass cannot do: space clamped
// away from one item has to come back round to the items that can still
// take it, which is exactly what the `flex-0/1/N-*` family measures.
```

## L9114-9115 · `if fsum < 1.0 {`

```
// Flex factors totalling less than one only claim that fraction of the
// ORIGINAL free space; the remainder stays with the container.
```

## L9122-9123 · `let scaled = |i: usize| if growing { li[i].grow } else { li[i].shrink * li[i].base };`

```
// Growing shares out by flex-grow; shrinking by flex-shrink weighted
// with the base size, so a big item gives up more than a small one.
```

## L9137-9138 · `for i in 0..n {`

```
// §9.7.4e — freeze whoever was pulled past a limit; their violation is
// what the next round redistributes. No violation → everyone is done.
```

## L9149 · `fn clamp_cross(v: f32, min: f32, max: f32) -> f32 {`

```
/// Clamp a stretched cross size to the item's cross min/max.
```

## L9154-9157 · `fn flex_item_style(s: &ComputedStyle, main: Option<f32>, forced_cross: Option<f32>, row: bool) -> ComputedStyle {`

```
/// Build a flex item's style for layout: force its main-axis size (`main`,
/// content-box) and, when stretching, its cross-axis size (`forced_cross`,
/// border-box). Item margins are zeroed — the flex code positions the item by
/// hand — while keeping the item's own box-sizing for padding conversion.
```

## L9171-9178 · `let inner_v = s.pad_top + s.pad_bottom + s.border_y();`

```
// `c` is the stretched BORDER-box cross size. `Len::Px` means a
// border-box value under `box-sizing:border-box` and a content-box
// one otherwise (see `content_height_of`), so the content-box case
// has to give back padding AND border. Leaving the border out made
// a bordered flex item exactly `border_y()` taller than the line it
// was stretched into — the same box-model twin that bucket item 31
// removed from `layout_flex` and `layout_grid`; this was the third
// copy.
```

## L9183-9193 · `if let Some(m) = main {`

```
// Column: main axis is vertical (HEIGHT), cross is horizontal (width).
//
// Es stand anders hier — der Zweig setzte `width` aus `main` und liess
// die Hoehe unberuehrt, und der Kommentar darueber sagte trotzdem
// „main axis is vertical". Der Aufrufer reichte folgerichtig die
// QUER-Groesse als `main` durch, und `flex-grow` hatte auf der
// Hauptachse einer Spalte nie eine Wirkung.
// `main: None` heisst „nicht erzwingen" — das braucht die MESSUNG, die
// ja gerade die natuerliche Hoehe sucht. Ohne diesen Fall reichte sie
// die Querbreite als Hauptgroesse durch und mass jedes Item so hoch,
// wie es breit ist.
```

## L9198-9203 · `if let Some(c) = forced_cross {`

```
// **Achtung, zwei Bedeutungen.** In der ZEILE ist `forced_cross` eine
// RANDkastenhoehe (die gestreckte Zeilenhoehe); in der SPALTE ist es
// die INHALTSbreite, die `flex_column` oben ausgerechnet hat. Sie
// gleich zu behandeln zog jedem gepolsterten Item seine Polsterung von
// der Breite ab — `flex-aspect-ratio-content-box-padding` misst genau
// das.
```

## L9211 · `fn layout_pre(`

```
/// `white-space: pre` — honor newlines and runs of spaces; no word-wrap.
```

## L9223 · `let raw = raw.strip_prefix('\n').unwrap_or(&raw);`

```
// Browsers strip a single leading newline right after <pre>.
```

## L9225-9227 · `let used_lh = st.line_height.px(st.font_px).unwrap_or(0.0);`

```
// `line-height` governs a preformatted block's line advance exactly as it
// does an inline formatting context's; the baseline sits at the content
// ascent plus half the leading.
```

## L9256-9259 · `#[derive(Default)]`

```
/// The line being measured: its text, plus the horizontal frame that rides on
/// it. The frame is what inline boxes on the line reserve (margin + border +
/// padding) — unbreakable width, so it adds to the min- and max-content
/// measurement alike.
```

## L9264-9268 · `atomic: f32,`

```
/// Sum of the outer widths of the atomic inline boxes on this line —
/// `inline-block`, images, form controls. They sit ON the line next to the
/// text, so at max-content they add to it. Measuring them as block-level
/// children instead took the WIDEST of them, which is why a shrink-to-fit
/// box around two inline-blocks came out one-child wide and stacked them.
```

## L9270-9271 · `atomic_min: f32,`

```
/// The widest min-content among those boxes. A line CAN break between two
/// of them, so at MIN-content they compete rather than add.
```

## L9275-9279 · `fn inline_frame(st: &ComputedStyle, cb: f32) -> f32 {`

```
/// The horizontal space one inline box adds to its line. `flow` advances the
/// pen by exactly this (as `InlineBox::lead + trail`), so the measurement has
/// to count it too or every shrink-to-fit box around a padded `<span>` comes
/// out too narrow. `cb` is 0 while measuring: a percentage margin has no basis
/// yet, so it contributes nothing.
```

## L9288-9292 · `fn flush_run(fonts: &crate::fonts::Fonts, st: &ComputedStyle, run: &mut Run, pref: &mut f32, min: &mut f32, horiz: bool)`

```
/// Measure the inline text collected so far as one line and fold it into a
/// box's running (max-content, min-content), then clear it. `white-space:
/// normal` collapses every whitespace run — including the newlines and
/// indentation between sibling tags in pretty-printed markup — to one space
/// first, or source formatting would count as visible width.
```

## L9301-9303 · `if st.pre {`

```
// `white-space: pre` keeps the source line breaks, so each source line is
// its own line box and the widest one wins — collapsing them into one
// would measure a whole code block as a single enormous line.
```

## L9308-9309 · `widest = widest.max(measure_sp(font, line.trim_end_matches(is_hangable_space), st.font_px, sp));`

```
// Trailing spaces hang past the line box, so they never widen it
// (css-text-3 §8). Leading ones DO count under `pre`.
```

## L9326-9328 · `let font = fonts.pick(st.bold, st.italic, st.mono, st.family);`

```
// The run's OWN font, not `regular()`: monospace advances wider than the
// proportional face, so measuring mono content with it under-sizes every
// auto table column that holds code.
```

## L9338-9340 · `let m = if st.nowrap {`

```
// `white-space: nowrap` has no break opportunities, so min-content is the
// whole line — not its widest word. Without this a shrink-to-fit box around
// a nowrap run is sized to one word and the run hangs out of it.
```

## L9351-9352 · `words.max(atomic_min)`

```
// The line may break either side of an atomic inline, so its own
// min-content competes with the widest word rather than adding to it.
```

## L9355-9357 · `if horiz {`

```
// Inside a row (or any inline-axis container) a run of stray inline content
// is one anonymous cell sitting BESIDE its siblings, so it adds to the
// row's width instead of competing with it (CSS2.1 §17.2.1).
```

## L9376-9378 · `fn collapse_whitespace(s: &str) -> String {`

```
/// Collapse every run of whitespace to a single space (CSS2.1 `white-space:
/// normal`), so measuring concatenated multi-node text (`intrinsic_width`)
/// doesn't count source-formatting newlines/indentation as visible width.
```

## L9397-9407 · `fn inline_block_box(&mut self, el: &'a Element, st: &ComputedStyle, avail_w: i32) -> Option<AtomicBox> {`

```
/// Collect an inline element's subtree into the current inline run
/// (recursing through nested inline elements, carrying each one's style +
/// link href). `el` is already on `self.path` when this is called.
/// Lay an `inline-block` out at the origin and capture everything it
/// painted, so the line box can place a finished rectangle. Width is
/// shrink-to-fit for `auto` (CSS2.1 §10.3.9, the same formula floats use).
///
/// The box establishes its own block formatting context, so the parent's
/// floats must not reach into it — and its own must not leak out
/// ([[feedback-speculative-layout-state]]: every throwaway context has to
/// put back what it took).
```

## L9432-9436 · `let outer_w = ceil_i32(content_w + pad_border + ml + mr).max(0);`

```
// **Ein leerer atomarer Inline ist NULL breit, nicht eins.** Der
// Mindestpixel hier war der Grund, warum DDGs Pillen auf zwei Zeilen
// brachen: ihre Vorlage haengt ein `&ZeroWidthSpace;` hinter den Text,
// das wird zu einem eigenen Kasten, und dessen eine Pixel nahm der
// Text daneben genau am Umbruch fehlte. Chromium misst dort 0.
```

## L9439-9446 · `let pct = |l: Len| matches!(l, Len::Pct(_) | Len::Calc { .. });`

```
// **Ein Prozent loeste sich ein ZWEITES Mal auf** — derselbe Fall, den
// `place_float` schon kennt und benennt: `layout_box` bekommt unten
// `outer_w`, den EIGENEN Randkasten dieses Elements, als
// Umgebungsbreite. Fuer `width: auto` ist das der Vertrag; fuer
// `width: 50%` ist es eine Falle — die Haelfte der Haelfte. Bootstraps
// `.placeholder.col-6` kam 469 statt 939 px heraus, und `col-*` auf
// einem `inline-block` ist ein Alltagsmuster. Gleiches fuer die beiden
// Grenzen.
```

## L9467-9471 · `let (s0, sl0, f0, fl0) = (self.stack_ops.len(), self.stack_links.len(),`

```
// Dieselbe Regel wie beim Knopfinhalt: die Befehle wandern gleich aus
// `self.ops` heraus, also duerfen keine Bereiche zurueckbleiben, die
// in sie zeigen. Ein `inline-block` mit einem positionierten Kind
// liess sonst einen Bereich stehen, der die naechsten Befehle der
// SEITE umsortierte.
```

## L9477-9478 · `let border_bottom = self.layout_box(el, st, 0, outer_w, st.margin_top as i32);`

```
// `layout_box` re-adds margin-left + padding, so it gets the MARGIN-box
// width — the same contract `place_float` uses.
```

## L9495-9497 · `let baseline = match inner_baseline {`

```
// The box aligns on its LAST line box's baseline; with no in-flow line
// box, or when it clips its overflow, it aligns on its bottom margin
// edge instead (CSS2.1 §10.8.1).
```

## L9505-9508 · `fn inline_box_of(&self, el: &Element, st: &ComputedStyle, cb_w: i32) -> Option<InlineBox> {`

```
/// The inline box an inline-level child needs, if any: one that paints
/// something of its own or reserves horizontal space. An `<img>`, a form
/// control and an `inline-block` are atomic — each already lays out and
/// paints its own box — and a `<br>` has none at all.
```

## L9514-9515 · `let (ml, mr) = (st.margin_left.px(cb).unwrap_or(0.0), st.margin_right.px(cb).unwrap_or(0.0));`

```
// `lead + trail` is `inline_frame` split at the content — the intrinsic
// measurement counts the same total, keep the two in step.
```

## L9527-9534 · `let hoverable = self.sheet.hover_set.may_match(el);`

```
// A box that paints nothing and reserves no space normally has no
// reason to exist — but a hover rule needs its RECTANGLE even when it
// is invisible at rest, and a bare `<a href>` is exactly that box. Miss
// this and the pointer finds every element except the ones it aims at.
// Mit `hit_all` braucht JEDES Inline-Element seinen Kasten, nicht nur
// die hoverbaren: `getBoundingClientRect` fragt danach, und ein
// `<label>` oder `<strong>` ohne Kasten antwortet mit NULL — eine
// Zahl, die aussieht wie eine Messung.
```

## L9553-9558 · `fn image_deco(&self, st: &ComputedStyle) -> Option<InlineBox> {`

```
/// The box decoration an `<img>` paints around its pixels. A replaced
/// element is atomic in the inline flow but it still has a box: MediaWiki
/// frames every thumbnail with `border: 1px solid` on the `<img>` itself,
/// and without this the picture sits in its figure with no frame at all.
/// Reuses `InlineBox` for the values; only the vertical extent differs —
/// an image's content box is the image, not a font's ascent + descent.
```

## L9571-9572 · `hover_seq: None,`

```
// An image's own box; a hover rule on it is caught by the element's
// block-level record, not here.
```

## L9589-9591 · `if matches!(st.display, Display::InlineBlock | Display::InlineFlex) {`

```
// `display: inline-block` — lay the whole box out now (block model,
// shrink-to-fit width) into its own display list, and hand the line
// box a finished rectangle. Position comes later, in `emit_line`.
```

## L9598-9600 · `if el.tag == "img" || el.tag == "svg" {`

```
// An `<img>` inside inline content (e.g. `<a><img></a>` — Wikipedia's
// thumbnails) is an atomic inline box; carry the enclosing link so it
// stays clickable.
```

## L9609-9610 · `if replaced_intrinsic(el).is_some() {`

```
// …and every other replaced element, laid out through the block model
// and handed to the line as a finished rectangle.
```

## L9617-9620 · `if let Some(kind) = crate::forms::kind_of(el).filter(|_| st.display != Display::Contents) {`

```
// A `<button>` under `display: contents` is unboxed like any other
// element — its label becomes ordinary inline content of the parent,
// and the UA widget it would otherwise draw is exactly the box the
// property says must not exist.
```

## L9630-9631 · `let counter_base = self.counters.stack.len();`

```
// `el` was already counter-entered by the caller; bound the counters its
// own descendants reset to this subtree (mirrors `flow_children`).
```

## L9633-9634 · `if let Some(b) = self.pseudo_box(el, st, PseudoElem::Before, bw) {`

```
// `el::before` — same anonymous-inline-box treatment as the block
// path (`flow_children`), just feeding this inline run instead.
```

## L9652-9659 · `if matches!(cs.position, Position::Absolute | Position::Fixed) {`

```
// `position:absolute`/`fixed` leaves the inline flow the same
// way it leaves the block flow — `flow_children` has had this
// branch all along and this one did not, so an out-of-flow box
// that happened to be INLINE-level stayed on the line and grew
// the page with it. Wikipedia's 1×1 autologin pixel is exactly
// that shape, and a 40×40 abspos `<img>` added its full height.
// Ahead of the float test because `float` computes to `none` on
// a positioned box (css-display-3 §2.7).
```

## L9668-9669 · `if cs.float != FloatKind::None {`

```
// A floated inline element leaves the inline flow and is placed
// as a float; surrounding text wraps around it.
```

## L9696 · `#[derive(Clone, Copy, PartialEq)]`

```
// ── inline formatting context ──────────────────────────────────────────────
```

## L9698-9699 · `#[derive(Clone, Copy, PartialEq)]`

```
/// The visual attributes a text run needs to be measured + painted. Two runs
/// merge into one `DrawOp` only if these match (fewer ops, same pixels).
```

## L9702-9703 · `hidden: bool,`

```
/// `visibility:hidden` on the run's own style: it still measures and still
/// takes its place on the line, it just isn't painted.
```

## L9705 · `transparent: bool,`

```
/// `opacity:0` on the run: painted as nothing, but still a click target.
```

## L9712 · `family: u32,`

```
/// Streuwert der `font-family` — siehe `ComputedStyle::family`.
```

## L9715 · `deco: u8,`

```
/// `text-decoration-line` bits (`style::DECO_*`).
```

## L9717 · `deco_color: Option<Rgba>,`

```
/// `text-decoration-color`; `None` = `currentColor`.
```

## L9719 · `break_word: bool,`

```
/// `overflow-wrap`/`word-break` allow splitting this run mid-word.
```

## L9721-9722 · `nowrap: bool,`

```
/// `white-space: nowrap` — this run's spaces are not break opportunities,
/// so the line grows past its box rather than wrapping.
```

## L9724 · `lh: f32,`

```
/// Used `line-height` in px, or 0 for `normal` (use the face's metrics).
```

## L9726-9728 · `sp: (f32, f32),`

```
/// `(letter-spacing, word-spacing)` in px. Every width in this file that
/// belongs to a run goes through `measure_sp`, so a run measures and paints
/// at the same advance — the two must not drift apart.
```

## L9732-9733 · `struct AtomicBox {`

```
/// An inline-block's finished display list, laid out at the origin and
/// translated into place once the line box knows where it sits.
```

## L9738-9742 · `inspects: Vec<InspectBox>,`

```
/// Hit rects recorded while laying this box out at the origin. They move
/// with it — without that every box inside an `inline-block` is reported at
/// the page's top-left corner, which reads as a layout bug that is not
/// there. It was one for `:hover`: the pointer lit up links it was nowhere
/// near, and the real link answered to nothing.
```

## L9745 · `w: i32,`

```
/// Margin-box size — what the line reserves.
```

## L9748 · `baseline: i32,`

```
/// Distance from the margin-box top to the baseline the line aligns on.
```

## L9750-9753 · `valign: crate::style::VAlign,`

```
/// How the box sits on the line (CSS2.1 §10.8.1). Ignoring this put every
/// atomic inline on the baseline, so a row of `inline-block`s of differing
/// heights came out as a STAIRCASE — MediaWiki galleries, icon rows and
/// badges all set `vertical-align: top` for exactly that reason.
```

## L9758-9762 · `struct InlineBox {`

```
/// An inline-level box that decorates itself or reserves horizontal space —
/// `<a class="external">` with its arrow icon, a badged `<span>`. Unlike a
/// block box it has no geometry of its own: it takes as many rectangles as it
/// has line boxes. Vertical padding and borders paint but never change the
/// line's height (CSS 2.1 §10.6.1); horizontal ones advance the flow.
```

## L9765-9767 · `hover_seq: Option<u32>,`

```
/// `seq` of the element this box came from, when its rectangle is worth
/// keeping: a `:hover` rule could react to it, or die Seite faehrt
/// Skripte und fragt nach Geometrie (`hit_all`).
```

## L9769-9775 · `hoverable: bool,`

```
/// Kann eine `:hover`-Regel dieses Element wirklich treffen?
///
/// **Getrennt von `hover_seq`, und das ist keine Feinheit.** Mit `hit_all`
/// bekommt JEDES Inline-Element einen Kasten — waeren die alle „hoverbar",
/// gaelte jede Mausbewegung als Stilwechsel, und das kostete auf Wikipedia
/// sechs volle Layouts fuer nichts. Dieselbe Falle, die `record_inspect`
/// im Kommentar nennt.
```

## L9777-9778 · `bg: Option<u64>,`

```
/// Image keys, already registered with the layout that needs them — `flow`
/// paints without a `Ctx` to ask.
```

## L9781-9782 · `lead: f32,`

```
/// Resolved px the box adds before its content (`margin + border +
/// padding`) and after it.
```

## L9789 · `enum Item {`

```
/// One inline item: a word, an atomic `<img>`, a form control, or a `<br>`.
```

## L9792-9796 · `BoxStart { bx: usize, space_before: bool },`

```
/// An inline box opens / closes around the items between them. Both index
/// `Inline::boxes`; they nest, so a box always closes the innermost open one.
/// The opening marker carries any collapsed space that precedes the box —
/// that space belongs to the text around it, so it advances the pen OUTSIDE
/// the box's background.
```

## L9799-9802 · `Strut(RunStyle),`

```
/// An inline box that generated no content of its own. It still contributes
/// its leading to any line box it lands in (CSS 2.1 §10.8) — `<span
/// style="line-height:5"></span>X` is a tall line — but it never makes a
/// line non-empty, so a line holding nothing else is still not generated.
```

## L9806-9809 · `Atomic { box_: RefCell<Option<AtomicBox>>, space_before: bool },`

```
/// `display: inline-block` — laid out already, waiting for its position.
/// The finished display list is MOVED out when the line box places it;
/// `flow` only has a shared borrow of the item list (`Placed::Control`
/// borrows from it), hence the cell. Each `Inline` is flowed exactly once.
```

## L9814-9830 · `const CTL_UNUSABLE: usize = usize::MAX;`

```
// Form-control chrome metrics (px).
//
// **Ausgerechnet, nicht geschaetzt.** `tools/fixtures/controls.html` stellt
// jedes Steuerelement viermal hin — nackt, nur gepolstert, nur gerahmt,
// beides — und aus den vier Hoehen faellt jede dieser Zahlen einzeln heraus.
// Vorher stand hier `PAD_Y = 3` und ein 1-px-Rahmen fuer alles; damit war ein
// nacktes Feld 28 statt 26 px hoch, mit eigener Polsterung 34 statt 36, und
// beide Fehler zeigten in verschiedene Richtungen — die Sorte, die sich in
// einem Rahmenwerk gegenseitig zudeckt.
/// „Dieses Steuerelement ist nicht mehr auffindbar." Gesetzt, wenn seine
/// Befehlsspanne beim Umbau zerriss — `repaint_controls` ueberschreibt die
/// Spanne an Ort und Stelle, eine geratene waere fremder Inhalt.
///
/// **Ein Wachwert muss ueberall angehalten werden, wo gerechnet wird.** Drei
/// Stellen zaehlten ungeprueft darauf weiter (`c.at + c.len`, zweimal
/// `c.at += …`); im Prueflauf ist das ein Ueberlauf-Panik, im ausgelieferten
/// Bild laeuft es still um null herum und zeigt auf einen echten Befehl.
```

## L9835 · `const CTL_ARROW: i32 = 20;`

```
/// Der Streifen, den ein `<select>` fuer seinen Pfeil frei haelt.
```

## L9837-9838 · `const CTL_SCROLLBAR: i32 = 16;`

```
/// Was ein `<textarea>` fuer seine Rollleiste reserviert — sie ist da, auch
/// wenn nichts zu rollen ist, und geht in die Eigenbreite ein.
```

## L9841 · `#[derive(Clone)]`

```
/// A measured form control, ready to place on a line and paint.
```

## L9848 · `text: String,`

```
/// Displayed text: value, placeholder, or button/select label.
```

## L9850 · `ghost: bool,`

```
/// `text` is a placeholder → paint it muted.
```

## L9852-9853 · `placeholder: String,`

```
/// The element's `placeholder`, kept for a repaint: emptying a field has to
/// bring it back, and the repaint has no element to ask.
```

## L9856-9859 · `disabled: bool,`

```
/// `disabled` — und das ist eine ANZEIGE, nicht bloss ein Zustand. Ein
/// gesperrter Knopf, der aussieht wie ein bedienbarer, ist eine falsche
/// Auskunft: der Benutzer klickt und nichts passiert. Jeder Browser
/// blasst ihn ab; beak malte ihn bis hierher unveraendert.
```

## L9862-9871 · `focus_ring: Option<(i32, Option<Rgba>, i32)>,`

```
/// Der Fokusring, wenn dieses Steuerelement die Tastatur hat: Breite,
/// Farbe (`None` = die des Themas) und Abstand vom Rahmenkasten.
///
/// **Ein Browser malt hier eine `outline`, keinen umgefaerbten Rahmen.**
/// beak faerbte bis 0.175.0 den Rahmen der SEITE blau um — auf
/// DuckDuckGos rundem Suchfeld sah das aus wie ein Fehler, und es war
/// einer: die Seite hatte ihre Farbe gesagt, und wir haben sie
/// ueberschrieben. Ein Umriss liegt AUSSERHALB des Kastens und nimmt
/// nichts weg. Hat die Seite selbst etwas ueber `outline` gesagt, gilt
/// ihr Wort — auch das Nein.
```

## L9873 · `caret: Option<usize>,`

```
/// Caret position in characters, when this control has keyboard focus.
```

## L9875 · `bg: Option<Rgba>,`

```
/// The control's own `background-color`, if the page styled it.
```

## L9877-9881 · `accent: Option<Rgba>,`

```
/// `accent-color` (css-ui-4 §5.1), `None` = `auto` (das Thema entscheidet).
///
/// **Am STEUERELEMENT, nicht am Textlauf.** Der erste Versuch legte sie an
/// `RunStyle` — den Stil eines Textlaufs, von dem eine Seite tausende hat,
/// und von denen keiner ein Kaestchen malt.
```

## L9883-9885 · `no_face: bool,`

```
/// The page paints this control's FACE itself — either it said
/// `appearance: none`, or it gave the control a background of its own
/// (`transparent` included). Only the face; the widget still shows.
```

## L9887-9892 · `appearance_none: bool,`

```
/// `appearance: none` (css-ui-4 §4) — the page opted out of the WIDGET, not
/// just its face. No UA frame, no tick, no dot, no chevron: what remains is
/// an ordinary box the page styles itself, which is how every custom
/// checkbox on the web is built. The heuristic above must NOT reach this
/// far — a page that merely writes `background: transparent` on a checkbox
/// still wants the tick.
```

## L9894-9896 · `bg_img: Option<(u64, BgLayer)>,`

```
/// The page's own `background-image` (resolved key + placement). A control
/// that opted out of the UA look carries its icon this way — DDG's search
/// button is a bare box with a magnifier here and nothing else.
```

## L9898-9901 · `content: Option<CtlContent>,`

```
/// A `<button>`'s laid-out CONTENTS (HTML §button-layout). A button is not
/// a label: its children are page content, and an icon + markup inside one
/// is the commonest button on the web. Present only when the element has
/// element children — a text-only button stays the cheap one-op label.
```

## L9903-9907 · `pad_l: i32,`

```
/// Leading text inset. Controls are atomic — we paint them with our own
/// metrics — but a page that reserves room for an icon does it with
/// `padding-left`, and ignoring that puts the text on top of the icon
/// (Wikipedia's search field asks for 36px to clear its magnifier). CSS
/// only ever WIDENS the inset; it cannot squeeze the text below `CTL_PAD_X`.
```

## L9909-9911 · `pad_r: i32,`

```
/// Die rechte Polsterung — dieselbe Zahl, mit der die Breite gerechnet
/// wurde. Der Maler nahm frueher `CTL_PAD_X`, und die Differenz zur
/// gemessenen Breite schnitt die Beschriftung ab.
```

## L9913-9914 · `pad_t: i32,`

```
/// Senkrecht dasselbe Paar — nur der Inhaltskasten braucht sie, um mittig
/// zu stehen.
```

## L9917 · `border: [CtlSide; 4],`

```
/// The frame, in paint order top/right/bottom/left.
```

## L9919-9922 · `radius: [f32; 4],`

```
/// `border-radius` in px, top-left clockwise. A control is painted with our
/// own metrics, so the page's radius has to be CARRIED here — it is not a
/// detail: every button on a Bootstrap or Tailwind page is rounded, and
/// square corners are the first thing that reads as „not a browser".
```

## L9927-9930 · `#[derive(Clone)]`

```
/// A control's laid-out contents, at the origin, ready to be translated into
/// the control's content box. Only the draw ops travel: a button's content
/// model is phrasing content, so there is no nested control to carry, and a
/// link inside one is not valid HTML either.
```

## L9936-9939 · `centred: bool,`

```
/// Button layout centres its contents VERTICALLY in the content box. A
/// checkbox or radio that opted out of the widget does not — it is an
/// ordinary box, and its generated content starts at the top-left corner.
/// That distinction is the whole of `input-{checkbox,radio}-no-centering`.
```

## L9943-9945 · `#[derive(Clone, Copy)]`

```
/// One edge of a control's frame. The UA gives every control a 1px one; a page
/// that writes any `border` longhand or shorthand owns all four instead —
/// including `border: none`, which is a declaration and not an absence.
```

## L9949 · `color: Option<Rgba>,`

```
/// `None` = paint in the UA's frame colour (the page named none).
```

## L9951 · `transparent: bool,`

```
/// `border-color: transparent` — keeps the width, paints nothing.
```

## L9955-9958 · `fn ctl_border(st: &ComputedStyle, kind: ControlKind) -> [CtlSide; 4] {`

```
/// A control's frame: the author's four sides once the page touched any of
/// them, else the UA's 1px. Google wraps its search button in a bordered
/// `<span>` and writes `border: none` on the `<input>`; painting our own frame
/// anyway put a second rectangle 1px down and right of the first.
```

## L9962-9971 · `let ua_w = if st.appearance_none {`

```
// The UA frame IS part of the widget: `appearance: none` takes it with the
// rest of it. Without this a custom checkbox came out inside a 1px box the
// page never asked for, on top of the border it drew itself.
//
// **Und er ist nicht fuer jedes Steuerelement gleich breit** (HTML §15.5,
// „Form controls"): ein Feld, ein `<textarea>` und ein Knopf tragen 2 px
// je Seite, ein `<select>` eines. Ein Kaestchen und ein Radioknopf tragen
// GAR keinen — ihr Rahmen ist Teil des gemalten Zeichens und liegt INNEN;
// ihn zum Kasten zu addieren machte ein `width: 24px` grosses Kaestchen
// 26 px breit.
```

## L9977-9979 · `ControlKind::Select | ControlKind::TextArea => 1,`

```
// Ein `<select>` und ein `<textarea>` tragen EINEN Pixel je Seite,
// ein Feld und ein Knopf zwei — aus den vier Hoehen der Vorlage
// einzeln herausgerechnet, nicht ueber einen Kamm geschoren.
```

## L9985-9986 · `w: if owned { px_of(s.width) } else { ua_w },`

```
// An unstyled side still takes the author's `border-color` — the UA
// frame is a real border, so colouring it is all a page needs to do.
```

## L9993 · `fn default_value(el: &Element, kind: ControlKind) -> String {`

```
/// The control's authored default value (what it shows before the user edits).
```

## L10017 · `fn select_label(el: &Element, value: &str) -> String {`

```
/// Label of the currently selected `<option>` (falls back to the raw value).
```

## L10038-10041 · `if el.tag == "input" && el.attr("value").is_some() {`

```
// HTML §4.10.5.1.20: on an `<input>`, `value=""` is an explicit EMPTY
// label — only a MISSING attribute gets the UA default. Pages put their
// own icon on the button by CSS and rely on it staying empty; DDG's search
// button is a magnifier that way, and "Absenden" painted straight over it.
```

## L10051-10052 · `fn mix(a: Rgb, b: Rgb, t: u32) -> Rgb {`

```
/// Blend `t`/255 of `b` into `a` — control faces/borders are derived from the
/// page theme so they read correctly on light and dark backgrounds alike.
```

## L10058-10064 · `fn dimmed(t: &Theme) -> Theme {`

```
/// Ein abgeblendetes Thema: alles, was Farbe traegt, zur Flaeche hin gemischt.
///
/// **Ein Regler, nicht acht Sonderfaelle.** `disabled` blasst Rahmen,
/// Beschriftung, Haken und Punkt gemeinsam ab; wer das an jeder Malstelle
/// einzeln entscheidet, laesst eine davon kraeftig stehen und merkt es erst
/// auf einer echten Seite. Chromium malt seine gesperrten Steuerelemente mit
/// 30 % Deckung ueber der Flaeche — das ist dieser Wert.
```

## L10077-10082 · `fn surface_palette(theme: &Theme, text: Rgb) -> Theme {`

```
/// Paint one control's chrome + text at (x, top) and record its hit rect.
/// The UA palette to draw a form control's chrome from, given the colour its
/// text inherited. `theme` is used as-is when the two agree, so a page that
/// says nothing keeps following the device; only a page that paints against
/// the theme gets a flipped palette. Approximates CSS Color Adjust's
/// `color-scheme`, which real pages almost never declare.
```

## L10102 · `fn luma(c: Rgb) -> u32 {`

```
/// Rec. 601 luma, the same measure `Theme::is_dark` uses.
```

## L10117-10118 · `let rect = |ops: &Vec<DrawOp>, ctl: &CtlBox| ControlRect {`

```
// Jede Rueckkehr aus dieser Funktion meldet ihren Bereich — sonst zeigt
// ein Eintrag auf Befehle, die ein anderes Element gemalt hat.
```

## L10123-10124 · `if ctl.style.hidden {`

```
// A `visibility:hidden` control paints nothing and takes no clicks — it is
// not registered, so it can't sit as an invisible target over the page.
```

## L10129-10130 · `if ctl.style.transparent {`

```
// `opacity:0`: paint nothing, but keep the hit rect — this is the
// checkbox-hack overlay that a CSS-only dropdown is toggled with.
```

## L10136-10141 · `let theme = &surface_palette(theme, ctl.style.color.c);`

```
// A control's chrome follows the SURFACE IT SITS ON, not the device theme.
// Wikipedia paints itself light whatever the desktop is set to (its dark
// mode is opt-in, gated on a class), so a face mixed from a dark theme is a
// black box on a white page. The signal that is actually to hand is the
// control's own inherited text colour: light text means a dark surface
// behind it, and dark text a light one.
```

## L10144-10145 · `let ink: Rgba = if ctl.disabled {`

```
// Die Beschriftung traegt die Farbe des ELEMENTS, nicht die des Themas —
// sie geht am Regler oben vorbei und muss einzeln mit.
```

## L10149-10156 · `let border = Rgba::opaque(mix(theme.bg, theme.text, 150));`

```
// **Der Rahmen ist das, woran man ein Steuerelement erkennt.** Er kam aus
// `theme.rule` (der Linienfarbe einer Tabelle, #dee2e6) und war damit auf
// Weiss fast unsichtbar — jedes Feld und jeder Knopf sah aus wie ein
// hellgrauer Fleck. Ein Browser malt hier `ButtonBorder`, ein sattes
// #767676, und der Wert steht nicht als Konstante da, weil beak ein
// dunkles Thema hat: 150/255 zwischen Flaeche und Textfarbe ergibt auf
// Weiss #7c7c7c und auf Dunkel dasselbe Mittelgrau von der anderen Seite
// ([[feedback_dark_mode_is_two_things]]).
```

## L10159-10163 · `let ring: Option<(f32, Rgba)> = {`

```
// Eine gerundete Ecke kann nicht aus vier Rechtecken bestehen. Solange alle
// vier Seiten dieselbe Breite und Farbe haben — bei Knoepfen und Feldern
// immer —, ist der Rahmen EIN Ring; sonst bleibt es beim eckigen Rahmen,
// und das ist die ehrlichere Naeherung als eine Ecke, die nur auf einer
// Seite rund waere.
```

## L10177-10179 · `let focus_op = |ops: &mut Vec<DrawOp>| {`

```
// **Der Fokus liegt AUSSERHALB.** Ein Browser zeichnet hier eine
// `outline`: sie nimmt dem Kasten nichts weg und faerbt nichts um. Was
// die Seite selbst ueber `outline` gesagt hat, gilt — auch ihr Nein.
```

## L10199 · `let face_op = |ops: &mut Vec<DrawOp>, color: Rgba| {`

```
// Die Flaeche — gerundet, wenn die Seite es sagt.
```

## L10207-10214 · `let face: Option<Rgba> = match ctl.bg {`

```
// A page that styles its own button (`background-color`) wins; otherwise
// the UA face is derived from the theme so it reads on light and dark.
// `appearance: none` (css-ui-4 §4) removes the question: the page opted
// out of the UA widget, so there is NO default face — only what the page
// paints itself. Our chrome otherwise filled in a box over a control the
// page wanted bare, and `surface_palette` guessed that shade from the
// control's own text colour, so a white icon glyph turned it black on a
// white page.
```

## L10219-10223 · `ControlKind::Submit | ControlKind::Reset | ControlKind::Button | ControlKind::File`

```
// Ein Knopf hat eine erhabene Flaeche (`ButtonFace`, #efefef);
// ein Feld und ein Kaestchen sind WEISS (`Field`), nicht
// hellgrau. Der alte Wert mischte auch in ein Textfeld einen
// Grauschleier, und eine Maske aus zwanzig Feldern sah dadurch
// aus wie eine gesperrte.
```

## L10230-10231 · `let bg_img = |ops: &mut Vec<DrawOp>| {`

```
// The page's own background image sits on the face, under the frame and
// any text — that is where an icon-only button keeps its icon.
```

## L10245-10249 · `ControlKind::Radio if !ctl.appearance_none => {`

```
// Ein Radioknopf ist RUND, ein Kaestchen eckig. Das ist keine
// Geschmacksfrage: die Form IST die Bedeutung — rund heisst „eine aus
// dieser Gruppe", eckig heisst „unabhaengig an oder aus". Beide als
// Quadrat zu malen nimmt dem Benutzer die Auskunft, ob seine Wahl die
// anderen ausschliesst.
```

## L10252-10255 · `if let Some(f) = face {`

```
// Angekreuzt bleibt die Flaeche HELL — der Ring und der Punkt
// darin nehmen die Farbe an. Nebeneinander gestellt malt
// Chromium genau das (Ring, weisser Zwischenraum, Punkt), und
// nicht die gefuellte Scheibe, die man dabei vor Augen hat.
```

## L10261-10263 · `let akzent = ctl.accent.unwrap_or_else(|| Rgba::from(theme.link));`

```
// **Der Ring war grau, auch wenn der Knopf gewaehlt war.** Das
// war der eigentliche Fehler: der Unterschied zwischen „gewaehlt"
// und „nicht gewaehlt" lag allein am Punkt in der Mitte.
```

## L10268-10270 · `let i = (w / 4).max(3);`

```
// Chromium malt in einen 13-px-Knopf einen Punkt von 6 px —
// etwas mehr als ein Viertel Einzug, mit sichtbarer heller
// Luft zum Ring.
```

## L10281-10284 · `if ctl.checked && ctl.bg.is_none() && !ctl.no_face {`

```
// Angekreuzt: das Kaestchen wird die Farbe, und darauf steht ein
// HAKEN. Vorher stand hier ein gefuelltes Quadrat auf heller
// Flaeche — dasselbe Zeichen wie beim Radioknopf, nur eckig, und
// damit war die Form nicht mehr die Auskunft.
```

## L10286-10289 · `face_op(ops, ctl.accent.unwrap_or_else(|| theme.link.into()));`

```
// **`accent-color` schlaegt das Thema** (css-ui-4 §5.1). Ohne
// sie bekam eine Seite, die ihre Kaestchen in ihrer eigenen
// Akzentfarbe will, unsere — `sandbox.nopeek.ch` schreibt
// genau das, dreimal.
```

## L10299-10300 · `if ctl.checked {`

```
// Die Seite hat die Flaeche selbst gesetzt — dann bleibt der
// Haken die Vordergrundfarbe, nicht die des Themas.
```

## L10312-10316 · `if let Some(c) = &ctl.content {`

```
// A button's laid-out contents sit in its content box, centred
// VERTICALLY (HTML §button-layout). Horizontally they are not
// centred as a box — `text-align: center` from the UA sheet is
// what centres the text inside them, which is why a 100px block
// child stays at the left edge with its own text in the middle.
```

## L10330-10331 · `let inner_w = (w - ctl.pad_l - CTL_PAD_X - 2).max(1) as f32;`

```
// Multi-line: honour hard newlines and wrap on width, top-
// aligned, clipped to the rows that fit in the box.
```

## L10356-10360 · `let inner = (w - ctl.pad_l - ctl.pad_r).max(0) as f32;`

```
// Clip an over-long value to the box. WHICH END is dropped is
// not a detail: a field being typed into must keep its tail,
// where the caret is — but a LABEL must keep its head, because
// it is a name, and a name clipped at the front is a different
// word. Google's consent buttons read "lle ablehnen".
```

## L10367-10368 · `let tx = match ctl.kind {`

```
// A button's label is centred in its box (HTML §button-layout);
// a field's value is not.
```

## L10392 · `let cx = x + w - CTL_PAD_X - CTL_ARROW / 2;`

```
// A downward chevron, drawn as a stack of narrowing bars.
```

## L10420-10421 · `focus_op(ops);`

```
// Zuletzt, also OBEN: der Ring liegt ueber allem, was das Steuerelement
// selbst gemalt hat.
```

## L10426-10433 · `fn stroke_frame(ops: &mut Vec<DrawOp>, x: i32, y: i32, w: i32, h: i32, sides: &[CtlSide; 4], ua: Rgba) {`

```
/// Paint a control's frame: each side its own width and colour, `ua` standing
/// in wherever the page named none. A side the page suppressed (`border: none`,
/// `border-color: transparent`) paints nothing at all.
///
/// Focus is the one thing the page cannot take away: a control with no frame
/// left still gets a 1px ring while it has the keyboard, because that ring is
/// an OUTLINE — it says where typing goes, and a page hiding its border never
/// meant to hide that.
```

## L10438-10439 · `for (side, rect) in [`

```
// Widths are clamped to the box so a frame thicker than its control still
// reads as a frame instead of painting past the far edge.
```

## L10461-10462 · `fn wrap_lines(font: Face, text: &str, size: f32, max_w: f32, max_rows: usize) -> Vec<String> {`

```
/// Break `text` into at most `max_rows` lines that fit `max_w`, splitting on
/// hard newlines first and then greedily on words (a `<textarea>`'s content).
```

## L10492-10496 · `fn clip_text_head(font: Face, text: &str, size: f32, max_w: f32) -> String {`

```
/// Trim `text` from the LEFT until it fits `max_w` (the caret sits at the end
/// of a field the user is typing into, so the tail is what matters).
/// Trim from the END until it fits — for a label, which is read from the
/// front. The counterpart of `clip_text_tail`, which trims from the front for
/// a field whose caret is at the back.
```

## L10529-10531 · `struct Inline {`

```
/// Accumulates inline content, then flows it into line boxes. Whitespace
/// collapses per `white-space: normal`: a run of spaces (within a text node or
/// across inline-element boundaries) becomes at most one inter-word space.
```

## L10534-10535 · `boxes: Vec<InlineBox>,`

```
/// Every inline box opened in this run, in tree order — so painting them
/// by index paints an ancestor's background under its descendant's.
```

## L10548 · `fn text(&mut self, raw: &str, st: &ComputedStyle, href: Option<&str>) {`

```
/// Add collapsed text from one text node under style `st`.
```

## L10550-10555 · `let rs = RunStyle { hidden: st.hidden, transparent: st.transparent, size: st.font_px, family: st.family,`

```
// Die Deckung der Inline-Vorfahren steckt HIER in der Farbe: ein
// Inline-Kasten hat keinen Befehlsbereich, ueber den sie spaeter
// gelegt werden koennte (siehe `ComputedStyle::inline_fade`). Zwei
// Laeufe verschmelzen nur bei gleicher `RunStyle` — die verschiedene
// Alpha trennt sie also von selbst, ohne dass der Verschmelzer davon
// wissen muss.
```

## L10585-10586 · `#[allow(clippy::too_many_arguments)]`

```
/// Add an atomic `<img>` (decoded or a placeholder) to the inline run,
/// carrying the enclosing link so an image-in-a-link stays clickable.
```

## L10594 · `fn atomic(&mut self, box_: AtomicBox) {`

```
/// Add a laid-out `inline-block` to the inline run.
```

## L10601 · `fn control(&mut self, ctl: CtlBox) {`

```
/// Add an atomic form control to the inline run.
```

## L10608-10610 · `fn open_box(&mut self, b: InlineBox) -> usize {`

```
/// Open an inline box around the items that follow. Deliberately leaves
/// `pending_space` alone: a space before `<a>` belongs to the word inside
/// it, not to the box.
```

## L10653-10655 · `#[allow(clippy::too_many_arguments)]`

```
/// Flow the accumulated items into line boxes starting at `y0`; append the
/// resulting `DrawOp`s + `LinkRect`s. Returns the y below the last line.
/// `theme` supplies placeholder colours for undecodable images.
```

## L10668-10669 · `indent: f32,`

```
// `text-indent` in px: only the FIRST line box starts in from the
// content edge — every later one resets the pen to the float band.
```

## L10679-10681 · `let face = |s: &RunStyle| fonts.pick(s.bold, s.italic, s.mono, s.family);`

```
// Each word/segment measures with its own face (a monospace run advances
// differently from proportional Inter), so glyph positions match what
// the raster later paints via the same `Fonts::pick`.
```

## L10685-10687 · `let strut_h = if strut > 0.0 { strut } else { line_gap(fonts.regular(), BASE_FONT_PX) };`

```
// Each line's usable [left, right] narrows around floats at its y-band.
// The strut: an empty line box is as tall as the block's own
// line-height, and that is also the band height float avoidance probes.
```

## L10693-10695 · `let mut line_below = 0.0f32;`

```
// How far the deepest item on the line reaches BELOW the baseline. Only
// needed to size a line around a `vertical-align: middle` box; text
// carries its descent inside its own line-box height already.
```

## L10699-10700 · `let mut open: Vec<OpenFrag> = Vec::new();`

```
// Inline boxes currently spanning the pen, innermost last, and the
// fragments they have finished on the line being built.
```

## L10703-10707 · `let mut run_end = f32::NAN;`

```
// Where the last text run left the pen. Two runs merge into one op only
// if the second starts exactly where the first ended — an inline box's
// edge (its space, margin, border, padding) moves the pen in between,
// and merging across that would draw the second run at the first one's
// pen and lose the gap.
```

## L10715-10718 · `pen += space_width(`

```
// The box's own face, not `regular()`: a box inherits
// the font of the run whose space this is, so it is the
// closest thing to hand — and a monospace space is much
// wider than a proportional one.
```

## L10730-10731 · `let x1 = (pen + b.trail - b.margin_right) as i32;`

```
// The border box ends where the content does plus the right
// padding and border; the margin stays outside it.
```

## L10764-10765 · `if !style.nowrap && !line.is_empty() && pen + sw + ww > right {`

```
// `white-space: nowrap`: the space before this word is not a
// break opportunity, so the line overflows instead.
```

## L10778-10783 · `if style.break_word && pen + lead + ww > right {`

```
// `overflow-wrap: break-word` — the word is wider than a
// whole line, so wrapping it whole would just overflow the
// box. Split it across lines at the last character that
// fits. A line that can't take even one character is
// already as narrow as it will get (a float band), so force
// one character through rather than spin.
```

## L10860-10867 · `let half_x = MIDDLE_HALF_X;`

```
// How far this box reaches above and below the baseline
// decides how tall the line box has to be. A `middle` box
// straddles the baseline, so half of it hangs ABOVE — the
// line has to grow for that half or the box paints outside
// its own line, which is what pushed MediaWiki's gallery
// thumbnails up out of their frames. `top`/`bottom` are
// measured against the line box itself and so contribute
// only their height.
```

## L10883-10885 · `let (fl, fr) = deco.as_ref().map_or((0.0, 0.0), |d| (d.lead, d.trail));`

```
// The frame an `<img>` paints around itself is part of the
// space it takes on the line — measure with it, or the
// border overlaps whatever comes next.
```

## L10887 · `let (mut bw, mut bh) = (*iw as f32, *ih as f32);`

```
// Fit the image to the content width, keeping aspect.
```

## L10940-10942 · `line.push(Placed::Control { x: sx, ctl });`

```
// The control's box sits ON the text baseline like an
// inline-block, minus its bottom padding so a field and the
// label beside it look aligned.
```

## L10960-10961 · `enum Placed<'a> {`

```
/// One item placed on the current line: a same-style text run, an image, or a
/// form control (borrowed from the inline run — it is only measured once).
```

## L10969 · `struct OpenFrag {`

```
/// An inline box spanning the pen: its fragment on the current line is open.
```

## L10972-10973 · `x0: Option<i32>,`

```
/// Border-box left edge, or `None` on a line the box only continues onto —
/// there the fragment starts at whatever content the line starts with.
```

## L10975 · `left: bool,`

```
/// This fragment carries the box's left border + padding.
```

## L10979-10981 · `struct Frag {`

```
/// One inline box's rectangle on one line box. A box that spans three lines
/// leaves three of these, and only the first/last carry its left/right edge
/// (the `box-decoration-break: slice` default).
```

## L10990-10997 · `fn line_exists(line: &[Placed<'_>], frags: &[Frag]) -> bool {`

```
/// The line is about to be emitted: close every open inline-box fragment at
/// the current pen. The boxes stay open — their next fragment begins on the
/// next line, no longer carrying the left edge and starting wherever that
/// line's content does.
/// Does this line box exist at all? It does if it holds content — or if an
/// inline box on it reserves horizontal space: margins, borders and padding on
/// an inline box keep an otherwise empty line alive (CSS 2.1 §9.4.2), which is
/// how an icon-only `<span>` gets a box to paint its background into.
```

## L11010 · `struct Seg {`

```
/// One same-style segment placed on the current line.
```

## L11018-11020 · `fn transform_word(w: String, tt: TextTransform) -> String {`

```
/// `text-transform` applied to one whitespace-delimited word. `capitalize`
/// uppercases the first letter of each word, which is exactly what a word here
/// is — the caller has already split on whitespace.
```

## L11042 · `fn marker_label(ls: ListStyle, n: i32) -> String {`

```
/// The marker string for a counter-style `list-style-type` at ordinal `n`.
```

## L11057-11060 · `fn format_counter(style: ListStyle, n: i32) -> String {`

```
/// A `counter()`/`counters()` value formatted in `style` — the bare number, no
/// trailing separator (unlike a list `marker_label`, which appends `.`).
/// `disc`/`circle`/`square` render their glyph (as browsers do); `none` is
/// empty. Unknown/`decimal` fall through to plain decimal.
```

## L11070 · `ListStyle::DecimalLeadingZero => alloc::format!("{n:02}"),`

```
// Pad 1..9 to two digits (`01`); everything else is plain decimal.
```

## L11075-11076 · `ListStyle::DisclosureClosed => "▸".into(),`

```
// As `counter()` text these are characters, not the drawn triangle a
// `<summary>` marker gets — css-counter-styles-3 names exactly these.
```

## L11084-11085 · `fn greek_counter(n: i32) -> String {`

```
/// `lower-greek`: bijective base-24 over α..ω with FINAL SIGMA left out
/// (css-counter-styles-3 §6.1) — 24 letters, not the 25 the block holds.
```

## L11105-11107 · `fn additive_counter(n: i32, table: &[(i32, char)]) -> String {`

```
/// An additive counter style: the largest weight that fits, repeatedly. Used
/// by `armenian` and `georgian`, which have no positional notation at all.
/// Out of range falls back to decimal, as CSS requires of an exhausted style.
```

## L11147-11148 · `fn alpha_counter(n: i32, upper: bool) -> String {`

```
/// Bijective base-26: 1→a, 26→z, 27→aa. Out-of-range ordinals fall back to the
/// decimal representation, as CSS requires of an exhausted counter style.
```

## L11165-11166 · `fn roman_counter(n: i32, upper: bool) -> String {`

```
/// Additive Roman numerals (CSS `lower-roman`/`upper-roman`). Only 1..=3999 is
/// representable; anything else falls back to decimal.
```

## L11187-11190 · `fn align_dx(align: TextAlign, rtl: bool, pen: f32, right: f32) -> i32 {`

```
/// `text-align`'s inline shift for one finished line: `pen` is the x just past
/// the last placed item, `right` the line box's right edge. LTR only, so
/// `start`/`left`/`justify` never shift. A line that overflows its box (a long
/// unbreakable word) has no slack to distribute and stays put.
```

## L11196 · `let to_right = match align {`

```
// `start`/`end` are direction-relative; `left`/`right` never are.
```

## L11207-11209 · `#[allow(clippy::too_many_arguments)]`

```
/// Emit one completed line at a shared baseline; return the next line's top y.
/// Text runs sit on the baseline (`top + ascent == baseline`); images are
/// bottom-aligned to the baseline. Images in a link get a `LinkRect` too.
```

## L11211-11214 · `fn push_decorations(style: &RunStyle, x: i32, w: i32, baseline: i32, ops: &mut Vec<DrawOp>) {`

```
/// Underline / line-through / overline for one text run, in the run's own
/// colour. Positions are metric-free approximations of the font's decoration
/// metrics: below the baseline, at roughly half the x-height, and at the cap
/// top. Emitted BEFORE the glyphs so a thick line never eats a descender.
```

## L11236-11249 · `pub fn resolve_out_of_band(`

```
/// Resolve ONE element's computed style outside a layout, with a given pointer
/// state — by descending from the root exactly the way `layout` does, through
/// the same `style::resolve`.
///
/// `subtree` also resolves the element's descendants, up to that many. A hover
/// rule does not only restyle what the pointer is IN — `nav:hover a`, every
/// dropdown on the web, styles a DESCENDANT from a state that lives on the
/// ancestor. Resolving only the carrier left those runs painted in the resting
/// colour with nothing to say that anything had been missed.
///
/// The descent is the price of not keeping a computed style per element alive
/// between layouts: it is one `resolve` per ancestor plus one per preceding
/// sibling at each level, against ~8300 for a page. It is not a second copy of
/// the cascade; the function it calls is the one layout calls.
```

## L11278 · `return None; // bigger than the fast path is worth`

```
// bigger than the fast path is worth
```

## L11284-11290 · `#[derive(Clone, Copy)]`

```
/// One element's computed style, plus the pseudo-elements that hang off it.
///
/// `::before`/`::after` are here because a hover rule reaches them —
/// MediaWiki underlines the article tabs with `a:hover::after{background}`,
/// so a pass that looked only at real elements saw a colour change on the
/// text and quietly missed the line under it. Their boxes are generated
/// during layout and never recorded, so a changed one gives up.
```

## L11298 · `pub fn pseudos_differ(a: &StyleProbe, b: &StyleProbe) -> bool {`

```
/// Do these two probes paint their pseudo-elements differently?
```

## L11308 · `#[allow(clippy::too_many_arguments)]`

```
/// Resolve one element's style and its two pseudo-elements.
```

## L11333 · `fn find_seq(el: &Element, seq: u32) -> Option<&Element> {`

```
/// The element with this `seq`, or `None`.
```

## L11348 · `pub fn find_seq_pub(dom: &Dom, seq: u32) -> Option<&Element> {`

```
/// The element with this `seq`, for callers outside this module.
```

## L11353-11354 · `pub fn subtree_text(el: &Element, out: &mut String) {`

```
/// Everything an element's subtree says, whitespace collapsed — see
/// `HoverRepaint::text`.
```

## L11371-11373 · `#[allow(clippy::too_many_arguments)]`

```
/// Resolve every descendant of `el`, appending to `out`. False once more than
/// `budget` of them exist — a subtree that large is not worth repainting one
/// element at a time, and laying out is the honest answer.
```

## L11414-11416 · `#[allow(clippy::too_many_arguments)]`

```
/// Walk into the child whose subtree holds `seq`. Seqs are handed out in
/// document order, so a child's subtree is `[child.seq, next_sibling.seq)` —
/// which is what `bound` carries for the last child.
```

## L11449-11452 · `if r.is_none() {`

```
// On success the chain STAYS — the caller resolves the element's
// descendants next, and they need their real ancestors. Popping it
// here left `.tabs li:hover a` unable to match anything, so a rule
// that styles a descendant quietly did nothing.
```

## L11463-11491 · `pub fn repaint_controls(`

```
/// Repaint one element in a finished display list, for a pointer change that
/// cannot move anything (`css::Class::Paint`).
///
/// The point is what it does NOT do: no parse, no cascade over the page, no
/// box arithmetic. A pointer entering a link on Wikipedia's Main_Page changes
/// 1 op of 723 and adds 1 more — measured — and used to cost a full layout,
/// 25 ms on the dev box and ~1950 ms on the device, for 0.06 % of the viewport.
///
/// Correctness is CHECKED, not argued. Everything this touches is regenerated
/// through the very functions layout used (`bg_ops`, `border_ops`,
/// `push_decorations`), and the old state is regenerated too and has to be
/// found in the list exactly where it is replaced. Anything ambiguous returns
/// `false` and the caller lays out, which is what it did before.
/// Ein Steuerelement neu malen, ohne die Seite neu auszulegen.
///
/// **Warum das die groesste einzelne Zahl in beak ist:** jeder Tastendruck in
/// einem Feld war bisher ein `bump_content_gen("form-key")`, also ein volles
/// Auslegen des Dokuments. Auf Wikipedia sind das 280 ms — je Zeichen. Was
/// sich dabei wirklich aendert, ist der Malbereich EINES Kastens.
///
/// Erlaubt ist das, weil `:focus`, `:focus-within` und `:active` bei uns in
/// `never_matches` stehen: Tastaturbesitz kann durch die Kaskade gar nichts
/// umstylen. `:checked` kann es sehr wohl — der Kaestchen-Trick ist
/// `input:checked ~ .menu{display:block}` —, also entscheidet dort
/// `may_restyle`, und im Zweifel wird ausgelegt.
///
/// Die Geometrie wird NICHT neu gerechnet: der Kasten behaelt seine Masse. Ein
/// Wert, der breiter ist als sein Feld, wird beschnitten (wie vorher) statt
/// mitzuwandern — das ist die benannte Grenze dieses Weges.
```

## L11499-11500 · `let mut plan: Vec<(usize, Vec<DrawOp>, CtlBox)> = Vec::new();`

```
// Erst planen, dann anwenden — ein Lauf, der auf halbem Weg aufgibt, liesse
// eine halb neu gemalte Seite stehen ([[repaint_hover]] macht es genauso).
```

## L11506 · `let (text, ghost, raw) = match state.value_set(old.seq) {`

```
// Der angezeigte Text: nur wer getippt hat, aendert ihn.
```

## L11514-11515 · `ControlKind::Select => return Err("select label comes from the tree"),`

```
// Ein `<select>` zeigt die Beschriftung seiner gewaehlten
// Option, und die steht im Baum, nicht im Zustand. Auslegen.
```

## L11540-11550 · `let mut was = Vec::new();`

```
// **Die Spanne beweisen, bevor sie ersetzt wird.**
//
// `at`/`len` zeigen auf Befehle, die WOANDERS entstanden sind, und
// zwischen dem Notieren und hier liegen drei Stellen, die die Liste
// umbauen: ein eingeschobener Hintergrund, ein angehaengter
// Inline-Kasten, die Umsortierung nach z. Jede hat die Spanne schon
// einmal verschoben, ohne es zu sagen — und eine falsche Spanne
// ersetzt fremde Befehle, was am Geraet aussieht wie „der Text daneben
// verschwindet". Also nachrechnen: was da steht, MUSS das sein, was
// der alte Zustand gemalt haette. Kostet `len` Vergleiche (~6) und
// macht aus einem stillen Schaden ein ehrliches Auslegen.
```

## L11554-11561 · `if c.len == 0 {`

```
// Eine LEERE Spanne sagt nicht, wo neue Befehle hingehoeren. Ein Feld
// ohne Rahmen, ohne eigene Flaeche und ohne Text malt nichts — die
// Fritzbox baut ihres genau so —, und `at` liegt dann auf einer
// Stelle, an der zwei Kaesten aneinandergrenzen. Ob die neuen Befehle
// VOR oder HINTER den Hintergrund des naechsten gehoeren, steht im
// Baum und nicht im Index. Also einmal auslegen: danach hat das Feld
// einen Cursor, die Spanne ist nicht mehr leer, und jeder weitere
// Tastendruck geht wieder den Schnellweg.
```

## L11579-11580 · `plan.sort_by_key(|(i, ..)| core::cmp::Reverse(lay.controls[*i].at));`

```
// Von hinten nach vorn: eine Ersetzung verschiebt nur, was DAHINTER liegt,
// und die Eintraege davor bleiben gueltig.
```

## L11604-11606 · `let mut edits: Vec<Edit> = Vec::new();`

```
// Plan every edit first and apply nothing until all of them are known to
// be possible: a pass that patched as it went left a half-repainted page
// behind whenever it gave up in the middle.
```

## L11614-11623 · `edits.sort_by_key(|e| e.at);`

```
// Nothing to do is a RESULT, not a failure — and the common one. Most of a
// page sits inside something a `:hover` rule COULD match without any rule
// actually applying, and `border-color` on a side with no width is a style
// that changes and paints nothing. Each of those pointer moves used to cost
// a full layout that produced a byte-identical display list.
//
// What must not pass silently is a change this pass did not account for —
// and that is decided per element in `plan_one`, by comparing the ops the
// two styles PRODUCE rather than the fields they differ in.
// Two elements laying claim to the same op cannot both be right.
```

## L11625-11626 · `if edits.windows(2).any(|w| w[0].at + w[0].len > w[1].at || w[0].at == w[1].at) {`

```
// Two elements laying claim to the same slot cannot both be right, and two
// insertions at the same index have no order between them.
```

## L11636 · `struct Edit {`

```
/// One planned replacement: `ops[at .. at+len]` becomes `ops`.
```

## L11643-11644 · `pub struct HoverRepaint {`

```
/// One element the pointer entered or left, with everything needed to repaint
/// it: where it painted, and how it and its subtree are styled before and after.
```

## L11646-11647 · `pub boxes: Vec<HoverBox>,`

```
/// The element's own fragments — one per line for an inline box — each
/// with the anchor that says where its decoration belongs.
```

## L11649-11652 · `pub pairs: Vec<(StyleProbe, StyleProbe)>,`

```
/// The element itself, then every descendant, before and after. The
/// unchanged ones are here too: a run painted in a colour some OTHER
/// element also uses cannot be told apart, and this pass has to know that
/// rather than recolour the wrong text.
```

## L11654-11662 · `pub text: String,`

```
/// Everything this element's subtree says, whitespace collapsed.
///
/// A rectangle is not proof of ownership: an element's border box can
/// enclose text that belongs to something else entirely — a table cell's
/// box contains the footnote marker of a link that is not inside it — and
/// two links on a page share a colour. Requiring the run to be part of what
/// this element actually SAYS is what tells them apart. A run that is not
/// found is left alone, and if that leaves nothing to do the page is laid
/// out instead.
```

## L11666 · `fn in_boxes(boxes: &[HoverBox], x: i32, y: i32) -> bool {`

```
/// Is `(x, y)` inside any of the element's fragments?
```

## L11671-11675 · `fn deco_ops(st: &ComputedStyle, b: &HoverBox) -> Option<Vec<DrawOp>> {`

```
/// The box decoration this style paints at `rect`, in display-list order.
///
/// A background IMAGE is deliberately not resolved: its key belongs to the
/// layout, and a repaint that guessed one would paint the wrong picture. A
/// style that wants one gives up instead.
```

## L11694-11695 · `fn box_differs(a: &ComputedStyle, b: &ComputedStyle) -> bool {`

```
/// Do these two styles paint the element's own BOX differently? Text aside,
/// this is everything a box draws for itself.
```

## L11712 · `fn op_eq(a: &DrawOp, b: &DrawOp) -> bool {`

```
/// Two ops the rasteriser would draw identically.
```

## L11734-11736 · `fn find_key_once(ops: &[DrawOp], key: OpKey) -> Option<usize> {`

```
/// Where the op with this key sits, if it sits there exactly once. The key
/// names an op by content, so it survives every insertion, clip and z-reorder
/// that happened after it was recorded.
```

## L11750 · `fn find_once(ops: &[DrawOp], want: &[DrawOp]) -> Option<usize> {`

```
/// Where `want` sits in `ops`, if it sits there exactly once.
```

## L11759 · `return None; // two boxes look alike — patch neither`

```
// two boxes look alike — patch neither
```

## L11767-11768 · `fn says(subtree: &str, run: &str) -> bool {`

```
/// Is this run part of what the element says? Whitespace is collapsed on both
/// sides because a run is already wrapped and the source is not.
```

## L11784-11785 · `struct Sub {`

```
/// One text substitution: runs painted in `off` become `on`, and their
/// decorations are re-emitted.
```

## L11792-11794 · `fn plan_one(`

```
/// `Err` = cannot be done with certainty, lay out instead. The reason is
/// carried out so a page that keeps taking the slow path can say WHY once,
/// rather than looking like the feature simply does not work.
```

## L11801-11802 · `if g.pairs[1..].iter().any(|(a, b)| pseudos_differ(a, b)) {`

```
// A DESCENDANT's pseudo-element has no recorded rect — only the carrier's
// do — so one that repaints is out of reach.
```

## L11809-11810 · `for b in &g.boxes {`

```
// ── every box this element paints: its own, and its pseudo-elements ───
// Each rect is a border box, which a paint-only change cannot have moved.
```

## L11826-11827 · `if b.has_text && (off.color != on.color || off.deco != on.deco) {`

```
// A pseudo's `content` string is not part of what the element SAYS, so
// its own run cannot be identified the way the element's runs are.
```

## L11835 · `continue; // this box looks the same in both states`

```
// this box looks the same in both states
```

## L11838-11840 · `let Some(at) = find_once(&lay.ops, &was) else {`

```
// There is something to replace, and it has to be found where it is
// replaced. The two lists may differ in length: a border that gains
// a side, a background that goes away entirely.
```

## L11846-11849 · `let Some(key) = b.anchor else {`

```
// Nothing to replace — a background that only exists under the
// pointer. A box puts its decoration in AHEAD of everything it
// paints; an absolutely positioned pseudo goes in AFTER everything
// its element painted. The anchor says by what, and which side.
```

## L11859-11860 · `for (kind, off, on) in [`

```
// A pseudo-element that repaints but has no rectangle of its own: the
// inline and flex paths generate one without recording it.
```

## L11878-11879 · `if g.pairs[1..].iter().any(|(a, b)| box_differs(&a.own, &b.own)) {`

```
// A DESCENDANT that repaints its own box is out of reach: its rect was
// never recorded, only the carrier's.
```

## L11883-11884 · `if g.pairs.iter().any(|(a, b)| {`

```
// Becoming invisible does not recolour anything — it takes whole ops out
// of the list, text included, and puts them back later.
```

## L11892 · `let mut subs: Vec<Sub> = Vec::new();`

```
// ── the text inside it ────────────────────────────────────────────────
```

## L11898-11899 · `if subs.iter().any(|s| s.off.color == off.color && (s.on.color != on.color || s.on.deco != on.deco)) {`

```
// Two elements in this subtree painted in the same colour that must
// now become different ones — a run cannot be assigned to either.
```

## L11905-11906 · `if g.pairs.iter().map(|(a, b)| (&a.own, &b.own)).any(|(off, on)| {`

```
// A run painted in a colour that some UNCHANGED element also uses would be
// recoloured by mistake.
```

## L11923-11932 · `let face = |op: &DrawOp| -> Option<(i32, Rgba, u32, bool, bool, bool)> {`

```
// Recolouring can MERGE two runs. The line builder joins neighbouring
// segments that share a face, so two runs the page painted apart —
// `46° 58′ 50″ N, 8° 20′ 20″ O` split across three links — become ONE op
// the moment they agree on a colour, with a single underline across the
// whole thing instead of three. A patch cannot produce that.
//
// The test is deliberately blunt: give up whenever a repainted run ends up
// looking like the run it TOUCHES. Whether they really merge also depends
// on the `href` behind them, which the display list no longer carries — so
// the only honest answer from here is "maybe", and maybe means lay out.
```

## L11959-11962 · `let (x, y, size) = (*x, *y, *size);`

```
// Everything `push_decorations` was given, recovered from the op it was
// emitted next to — the run's own width and baseline, measured with the
// same face at the same size AND the same spacing. No second copy of
// the rule.
```

## L12005 · `let Some(start) = i.checked_sub(was.len()) else {`

```
// A run's decorations sit immediately before it.
```

## L12015-12020 · `if touched == 0 {`

```
// A colour changed and not one run carried it. That is the ordinary case
// for a container whose text belongs to a link with a colour of its own —
// the link keeps its colour, so there is genuinely nothing to repaint, and
// a full layout produces a byte-identical list. What must not be mistaken
// for it is a run this pass SKIPPED: one that carries the colour but could
// not be shown to be part of what the element says.
```

## L12037-12038 · `fn placed_x(p: &Placed<'_>) -> i32 {`

```
/// Where a placed item starts on its line — the left edge of a fragment that
/// only continues onto this line, since the box's own left edge is a line above.
```

## L12046-12049 · `fn paint_frag(`

```
/// Paint one fragment of an inline box. The rectangle is the box's own content
/// area — its font's ascent + descent, NOT the line box (CSS 2.1 §10.6.1) —
/// grown by its padding and border. Vertical padding therefore spills over the
/// neighbouring lines instead of pushing them apart, which is what CSS asks for.
```

## L12071-12076 · `fn frag_rect(`

```
/// The rectangle one fragment of an inline box decorates — NOT its line box.
///
/// One source, because the pointer repaint has to regenerate exactly what
/// `paint_frag` produced. Handing it the hit rect instead made every inline
/// background one pixel too tall, which is the difference between a patch that
/// matches a layout and one that does not.
```

## L12113-12117 · `let mut pending_anchor: Vec<(usize, usize)> = Vec::new();`

```
// An inline box's decoration goes in where the fragment begins — but that
// op does not exist yet when the hit rect is recorded, so the index is
// parked and turned into a content key once the line is done. Within this
// function `ops` is only ever APPENDED to, so an index taken here still
// means the same slot at the end of it.
```

## L12119-12121 · `if !frags.is_empty() {`

```
// Inline-box decoration goes down before anything on the line, so text sits
// on its own background. Sorted by box index — allocation order is tree
// order, which puts an ancestor's background under its descendant's.
```

## L12128-12130 · `if let Some(seq) = b.hover_seq {`

```
// A box spanning three lines leaves three fragments and therefore
// three hit rects — which is right: the pointer is inside the box
// wherever any of its fragments is.
```

## L12134-12137 · `let (_, fy, _, fh) = frag_rect(fonts, b, x0, x1, baseline);`

```
// Die EIGENE Hoehe des Inline-Kastens, nicht die der
// Zeile: eine Zeile mit einem hohen Steuerelement darin
// machte sonst jedes `<label>` daneben genauso hoch, und
// `getBoundingClientRect` meldete 68 statt 21.
```

## L12170-12171 · `match seg.style.valign {`

```
// vertical-align: raise a superscript, drop a subscript off the
// shared baseline (the run is already at its reduced sup/sub size).
```

## L12177-12178 · `if let (Some(h), false) = (&seg.href, seg.style.hidden) {`

```
// A hidden run is not a click target either — otherwise a
// collapsed dropdown leaves invisible links over the article.
```

## L12205-12210 · `use crate::style::VAlign;`

```
// CSS2.1 §10.8.1. `baseline` puts the box's own baseline on the
// line's — with the approximation `baseline == h` that is its
// bottom margin edge, which is what a block-ish inline-block
// does. `top`/`bottom` measure against the LINE BOX instead,
// and that is the case real pages lean on: without it a row of
// differently tall `inline-block`s descends like a staircase.
```

## L12215-12218 · `VAlign::Middle => baseline - MIDDLE_HALF_X as i32 - box_.h / 2,`

```
// Approximate: the box's midpoint against the baseline
// raised by half an x-height, taken as a fraction of the
// line's ascent (the parent's font metrics are not threaded
// this far down).
```

## L12246-12247 · `let base = ops.len();`

```
// Die Befehle des Kastens landen HINTER den bisherigen — die
// Indizes seiner Steuerelemente zaehlen aber ab null.
```

## L12264 · `let top = baseline - h; // image bottom sits on the baseline`

```
// image bottom sits on the baseline
```

## L12268-12269 · `if !hidden && !transparent {`

```
// Emitted whether or not the pixels have arrived — the
// rasteriser draws the placeholder when the lookup misses.
```

## L12271-12273 · `if let Some(d) = &deco {`

```
// The image's own box, under its pixels: a replaced element
// is atomic in the flow but still paints a background and a
// border (MediaWiki frames every thumbnail this way).
```

## L12298-12300 · `#[test]`

```
/// Every weight in an additive table must be positive: a zero would make
/// `while v >= w` spin forever, and a counter style is reachable from any
/// page's CSS.
```

## L12306 · `for t in [&ARMENIAN[..], &GEORGIAN[..]] {`

```
// Strictly descending, or the greedy loop emits the wrong glyph.
```

## L12316 · `assert_eq!(greek_counter(25), "\u{3b1}\u{3b1}");`

```
// 25 wraps to a two-letter form — 24 letters, final sigma excluded.
```

## L12319 · `assert_eq!(additive_counter(1988, &ARMENIAN), "\u{54c}\u{54b}\u{541}\u{538}");`

```
// 1988 = 1000 + 900 + 80 + 8
```

## L12322 · `assert_eq!(additive_counter(0, &ARMENIAN), "0");`

```
// Out of range falls back to decimal rather than looping or truncating.
```

## L12349-12356 · `#[test]`

```
/// **Der Clearfix.** Ein Kasten, dessen letztes Kind `clear` traegt, muss
/// so hoch werden wie sein Float — die Raeumung ist Platz IM Kasten, kein
/// Schub AUF ihn.
///
/// Vorher kam der Kasten mit der Hoehe des geraeumten Kindes heraus (also
/// 0 bei einem leeren), weil sein eigener Rand mit heruntergezogen wurde.
/// Das ist das Muster, mit dem ein sehr grosser Teil des echten Webs seine
/// Floats einschliesst.
```

## L12368 · `assert_eq!(hoehe("<div style=\"clear:both\"></div>"), (0, 50),`

```
// Der klassische Clearfix: leeres `clear`-Kind.
```

## L12371 · `assert_eq!(hoehe("<div style=\"clear:both;height:10px\"></div>"), (0, 60));`

```
// Mit eigener Hoehe kommt sie unten dazu.
```

## L12373-12374 · `assert_eq!(hoehe("<div style=\"height:5px\"></div>"), (0, 5));`

```
// Ohne `clear` schliesst ein Kasten seinen Float NICHT ein (§10.6.3) —
// die Gegenprobe, damit der Riegel nicht zu weit greift.
```

## L12376-12378 · `assert_eq!(hoehe("<div style=\"clear:both\"></div><div style=\"margin-top:99px\"></div>"),`

```
// Und der Rand eines geraeumten, durchkollabierenden Kindes
// verschmilzt nicht mit dem Unterrand des Elters (§8.3.1):
// 50 px Raeumung + 99 px Rand des Folgegeschwisters.
```

## L12383-12389 · `#[test]`

```
/// Only the elements a `:hover` rule can actually react to get a box —
/// the invalidation set. On Wikipedia's Main_Page that is the difference
/// between 8327 boxes and a handful, and it is what makes 98.7 % of
/// pointer movement cost nothing at all.
///
/// The carrier of the `:hover` is what must be hit-testable, NOT the
/// selector's subject: in `nav:hover a` the pointer is inside the `<nav>`.
```

## L12402 · `let only_a = page("a:hover{background:#f00}");`

```
// `a:hover` — the link carries it, the `<nav>` around it does not.
```

## L12406-12407 · `let only_nav = page("nav:hover a{color:#0f0}");`

```
// `nav:hover a` — now the NAV is the carrier, and it is the one the
// pointer has to be found inside.
```

## L12412-12413 · `assert!(page("*:hover{color:#f00}").len() > 2);`

```
// A compound that names nothing can match anything → collect everything
// rather than freeze the page under the pointer.
```

## L12417-12422 · `#[test]`

```
/// The pointer hovers the element it is inside AND every ancestor that
/// contains it — `nav:hover a` (every dropdown on the web) styles a
/// descendant from a state that lives on the parent.
///
/// This is the geometry half; that the restyle reaches a PIXEL is
/// `raster::tests::hover_repaints_the_element_under_the_pointer`.
```

## L12425 · `let html = "<body><style>nav:hover{background:#eee} a:hover{background:#f00}</style>\`

```
// Both elements carry a hover rule, so both are hit-testable.
```

## L12436 · `assert!(l.hover_at(-5, -5).is_empty());`

```
// A point outside every box hovers nothing.
```

## L12440-12448 · `#[test]`

```
/// CSS 2.1 Appendix E: a box paints its background AND its border before
/// any descendant. The background already did; the border was appended
/// after the content, so it landed on top of the box's own children.
///
/// Invisible while a child stays inside its parent's content box — and
/// wrong the moment one does not, which is what a negative margin is FOR.
/// The CSS2.1 suite tests exactly that idiom: pull a child left by the
/// parent's border width so its own border covers it, and check no red
/// shows. 62 of those went from fail to pass.
```

## L12451 · `let l = lay(`

```
// The child's black border is pulled onto the parent's red one.
```

## L12469 · `let l = lay(`

```
// And the background still goes under the border of the SAME box.
```

## L12481-12490 · `#[test]`

```
/// A hit rect has to sit where the box is PAINTED, on every path that
/// moves a box after laying it out.
///
/// An `inline-block` is laid out at the ORIGIN and translated onto its
/// line; a `position:relative` box is laid in flow and then offset; a
/// `vertical-align`ed table cell slides its content down. Every one of
/// those moved the ops and left the hit rects behind — so the pointer lit
/// up elements it was nowhere near (Wikipedia's whole sister-project row
/// answered to the top-left corner) and the link actually under the
/// pointer answered to nothing.
```

## L12503 · `check(`

```
// Laid out at the origin, then translated onto the line box.
```

## L12508 · `check(`

```
// Laid out in flow, then offset by `position:relative`.
```

## L12514 · `check(`

```
// A flex item is measured, discarded, then laid out for real.
```

## L12521-12525 · `#[test]`

```
/// A discarded trial layout must not leave its hit rects behind. It
/// records them at TRIAL coordinates, so a leak points the pointer at a
/// rectangle the page never painted — and records the same element once
/// per trial, which is how Wikipedia's Main_Page came to carry 5131 hit
/// rects for 656 real ones.
```

## L12540 · `#[test]`

```
/// A page with no `:hover` rule must not pay for a list nobody reads.
```

## L12549-12553 · `let l = lay_inspect(`

```
// The device debugging tool has to agree with the pixels. It used to
// report the CONTAINING BLOCK's x/width, which coincides with the box
// only for a plain `width: auto` block — so every report about a
// centred or max-width container (MediaWiki's `.mw-page-container`)
// carried the viewport's numbers instead of the box's.
```

## L12560 · `let red = rects(&l).into_iter().find(|(.., c)| *c == Rgb(0xff, 0, 0)).unwrap();`

```
// … and it matches what is actually painted.
```

## L12577 · `assert_eq!(inner_h("height:200px"), 100);`

```
// Definite parent → half of its CONTENT height.
```

## L12579 · `assert_eq!(inner_h("height:220px;padding:10px;box-sizing:border-box"), 100);`

```
// `box-sizing: border-box` — the content box is what a % measures.
```

## L12581-12583 · `assert_eq!(inner_h("background:#eee"), 0);`

```
// Indefinite parent → the percentage behaves as `auto` (CSS2.1 §10.5),
// which for an empty box is zero. Guessing a height here is what
// truncated pages the two earlier attempts at this.
```

## L12589-12592 · `let body: String = (0..60)`

```
// The 0.3.13 regression, nailed down: `html { height: 100% }` makes the
// root box exactly one viewport tall, and the page still has to scroll.
// `Layout::height` is the painted extent, not the root box's bottom —
// that fix (0.3.14) is what made general percentage heights safe to add.
```

## L12605-12608 · `let tops = |va: &str| {`

```
// Every atomic inline used to sit on the baseline, so a row of
// `inline-block`s of differing heights descended like a staircase —
// MediaWiki galleries, icon rows and badges all set `vertical-align`
// for exactly this.
```

## L12626-12628 · `let ((ry, _), (by, _)) = tops("middle");`

```
// A `middle` box straddles the baseline, so the line has to grow around
// it — otherwise it paints above its own line (the gallery thumbnails
// hung out of their frames).
```

## L12636-12639 · `let l = lay(`

```
// Floats sit side by side, so a shrink-to-fit box around a row of them
// is as wide as their SUM. Taking the widest sized Wikipedia's
// `float: right` footer <ul> to one icon, and its floated <li> children
// then stacked vertically instead of sitting in a row.
```

## L12653-12661 · `#[test]`

```
/// **Ein Float steht NEBEN der Zeile, also zaehlt seine Breite dazu.**
/// Vorher wurde er gegen die Zeile GEMAXT, und der schrumpfende Kasten kam
/// genau um die Float-Breite zu schmal heraus — dann passte der Float
/// nicht mehr hinein und rutschte eine Zeile tiefer. Auf DuckDuckGos
/// Kopfleiste war das der Hamburger-Knopf unter „Protection. Privacy."
///
/// Geprueft wird die BREITE des schrumpfenden Kastens: 200 + 32. Sie ist
/// die Ursache; wo der Float dann landet, ist die Folge. Chromium misst
/// auf derselben Vorlage dieselben 232.
```

## L12680-12686 · `#[test]`

```
/// **Was schwebt, ist block-artig** (css-display-3 §2.7), und ein
/// geflotetes STEUERELEMENT ist aus dem Fluss wie jedes andere. Beides
/// fehlte: ein `display:inline-flex` mit `float:right` blieb ein atomarer
/// Inline auf der Zeile, und der Steuerelement-Zweig in `flow_children`
/// verschluckte den Float — aber nur bei automatischer Breite, weshalb es
/// wie ein Breitenfehler aussah. Auf DDGs Wissenskasten klebte so der
/// „Directions"-Knopf links vor dem Titel.
```

## L12700-12703 · `#[test]`

```
/// **Ein Formatierungszeichen hat keine Laufweite, und `::before` zaehlt
/// mit.** Beides traf DDGs „Searches related to": die Vorlage haengt ein
/// `&ZeroWidthSpace;` hinter jeden Eintrag und eine Lupe davor, und der
/// Kasten kam um beides zu schmal heraus — der Text brach auf zwei Zeilen.
```

## L12706 · `let zwnj: String = core::iter::repeat('\u{200c}').take(20).collect();`

```
// Zwanzig U+200C: null breit, nicht zwanzig Glyphen.
```

## L12710-12712 · `let g = rects(&l).into_iter().find(|(.., c)| *c == Rgb(0, 0xff, 0));`

```
// Vorher waren das 184 px — je ein `.notdef` aus der Schrift. Das eine
// Pixel, das bleibt, ist der Boden des gemalten Kastens, nicht die
// Laufweite; er steht hier als Zahl statt als Behauptung.
```

## L12717-12718 · `let l2 = lay(`

```
// Und das `::before` steht AUF der Zeile, also zaehlt seine Breite
// samt Rand in die Eigenbreite des schrumpfenden Kastens.
```

## L12742 · `assert_eq!(lines(""), 4);`

```
// Four lines in a 40px box: without clipping all four still paint.
```

## L12744 · `assert!(lines("overflow:hidden") < 4);`

```
// `hidden` keeps only what fits.
```

## L12746-12747 · `assert_eq!(lines("overflow:auto"), 4);`

```
// `auto`/`scroll` deliberately do not clip — we have no scroll
// container, so clipping would hide reachable content.
```

## L12761 · `let (lines, _) = run("");`

```
// Without it the word stays one run that overflows its 80px box.
```

## L12764 · `let (lines, _) = run("overflow-wrap:break-word");`

```
// With it the word is split across several lines …
```

## L12767 · `assert!(run("word-wrap:break-word").0 > 1);`

```
// … and the legacy spellings mean the same thing.
```

## L12770 · `let l = lay(&format!("<body><div style=\"width:80px;overflow-wrap:break-word\">{long}</div></body>"), 300);`

```
// Every piece must fit the box — that is the whole point.
```

## L12775-12776 · `let emoji = "\u{1F468}\u{200D}\u{1F4BB}\u{1F469}\u{200D}\u{1F467}";`

```
// A grapheme cluster is never split, however narrow the box: an emoji
// ZWJ sequence must stay whole (css-text-3 §5.1, `line-breaking-014`).
```

## L12784-12795 · `#[test]`

```
/// **Ein `0 0 0 Npx`-Schatten auf einem runden Kasten ist ein RING.**
///
/// So schreibt das halbe Web seine Umrandungen, und DuckDuckGos Suchfeld
/// hat gar keinen `border` — sein sichtbarer Strich ist der dritte
/// Schatten seiner Liste. Als vier Rechtecke gemalt bekam die Kapsel
/// eckige Ecken; gemessen gegen Chromium wanderte beaks Kante bei jedem
/// `y` auf derselben Spalte, waehrend Chromiums sich nach aussen bog.
/// **Der Schreibzeiger ist ein eigener Befehl, damit er blinken kann.**
///
/// Als gewoehnliches Rechteck haette jeder Takt ein Neuauslegen gekostet
/// (am Geraet 10-40 ms, zweimal je Sekunde). So bleibt das Layout stehen,
/// und der Rasterer laesst ihn in der dunklen Haelfte einfach aus.
```

## L12811-12812 · `let (w, h) = (400u32, 60u32);`

```
// Dieselben Kaesten, zwei Anstriche: hell und dunkel muessen sich
// GENAU an dieser Stelle unterscheiden und sonst nirgends.
```

## L12842 · `let (r, thick, w, h) = ring("border-radius:20px;box-shadow:0 0 0 1px #000")`

```
// Der Fall der echten Seite: Kapsel, kein Rahmen, Ring als Schatten.
```

## L12846-12847 · `assert_eq!((w, h), (202, 42));`

```
// Der Kasten des Schattens ist um den Spread GEWACHSEN, und die Ecken
// mit ihm (CSS Backgrounds 3 §7.1.1): 20 + 1.
```

## L12850 · `assert!(ring("box-shadow:0 0 0 1px #000").is_none());`

```
// Ohne Radius bleibt es beim alten Weg — vier Rechtecke, kein Ring.
```

## L12852 · `assert!(ring("border-radius:20px;box-shadow:2px 0 0 1px #000").is_none());`

```
// Mit Versatz ist die Differenz kein Ring mehr.
```

## L12856-12859 · `#[test]`

```
/// Der WEICHE Schatten folgt den Ecken ebenfalls — gemessen in PIXELN,
/// nicht an Befehlen ([[feedback_paint_test_not_parse_test]]): ein Punkt
/// weit ausserhalb der Eckrundung muss frei bleiben, die Mitte derselben
/// Kante gedeckt sein.
```

## L12862-12870 · `let l = lay("<body style=\"margin:0\"><div style=\"width:120px;height:60px;\`

```
// **Mit SPREAD, und das ist keine Zierde.** Ohne ihn liegt die ganze
// Eckrundung INNERHALB des Rahmenkastens, und der wird aus einem
// aeusseren Schatten ohnehin ausgespart (CSS Backgrounds 3 §7.1.1) —
// eckig und rund sehen dort gleich aus, und der Test waere gruen,
// ohne etwas zu pruefen. Erst der Spread schiebt die Ecke nach
// draussen, wo sie zu sehen ist.
//
// Ein LEERER Kasten (ein Buchstabe traefe die Messpunkte) und ein
// WEISSER Schatten (der Vorgabegrund dieser Probe ist dunkel).
```

## L12878-12883 · `let frei = at(10, 10);`

```
// Rahmenkasten 40,40 120x60; Schattenform um 8 gewachsen, also
// 32,32 136x76 mit Radius 38 — Mittelpunkt der oberen linken Ecke
// wieder (70,70). (38,38) liegt 7 px AUSSERHALB dieses Kreises und
// zugleich AUSSERHALB des ausgesparten Rahmenkastens: genau dort
// malte der eckige Schatten voll durch. (100,36) ist die Mitte der
// Oberkante im Schatten, (10,10) der freie Grund.
```

## L12899 · `let l = lay("<body><div style=\"width:100px;height:40px;background:#f00;border-radius:8px\">x</div></body>", 400);`

```
// Shorthand, resolved against the border-box width.
```

## L12902 · `let l = lay("<body><div style=\"width:100px;height:40px;background:#f00;border-radius:1px 2px 3px 4px\">x</div></body>",`

```
// Percentages resolve; four values map to the CSS corner order.
```

## L12905 · `let l = lay("<body><div style=\"width:100px;height:40px;border:3px solid #000;border-radius:8px\">x</div></body>", 400);`

```
// A uniform border becomes one stroked ring …
```

## L12908 · `let l = lay("<body><div style=\"width:100px;height:40px;border:3px solid #000;border-top-width:9px;border-radius:8px\">x`

```
// … a mismatched one falls back to the four square edges.
```

## L12911 · `let l = lay("<body><div style=\"width:100px;height:40px;background:#f00\">x</div></body>", 400);`

```
// No radius → the plain (fast) rect op, unchanged.
```

## L12918 · `let y_of = |va: &str| {`

```
// One tall cell sets the row height; the short cell's text moves.
```

## L12933 · `assert_eq!(y_of("baseline"), top);`

```
// The initial value degrades to `top` for us (no cross-cell baselines).
```

## L12948-12949 · `assert_eq!(box_of("width:100px;height:60px"), (100, 60));`

```
// Empty cells used to collapse the table onto its border: the columns
// measure zero, so nothing ever claimed the specified width.
```

## L12951 · `let (_, h) = box_of("width:100px;height:1px");`

```
// `height` is a MINIMUM (CSS2.1 §17.5.3) — taller content wins.
```

## L12954 · `assert_eq!(box_of("width:100px;min-height:60px"), (100, 60));`

```
// `min-height` does the same job.
```

## L12956 · `assert_eq!(`

```
// `box-sizing: border-box` counts the border in, not on top.
```

## L12966-12969 · `let outer = |inner: &str| {`

```
// An out-of-flow box with `width: auto` shrink-wraps. Its own frame was
// subtracted twice (once here, once by the block path that reads the
// handed-over width as a containing block), and a child's margins never
// reached the measurement at all.
```

## L12980 · `assert_eq!(outer("border:10px solid #f00;width:200px;height:60px"), 240);`

```
// 200 content + 20 child border + 20 own border.
```

## L12982 · `assert_eq!(outer("border:10px solid #f00;width:200px;height:60px;margin:0 50px"), 340);`

```
// … + 50px margins on each side.
```

## L12986-12990 · `#[test]`

```
/// Inline-blocks sit side by side ON a line, so a shrink-to-fit container
/// has to be wide enough for their SUM. Measuring them as block-level
/// children takes the widest instead, and they then have no room beside
/// each other and stack — which is how Google's `float:right` header bar
/// came out one word wide with "Gmail" and "Bilder" on separate lines.
```

## L13006 · `let (w, h) = bar("float:right;");`

```
// Two 60px children beside each other: at least 120 wide, one row tall.
```

## L13015-13019 · `#[test]`

```
/// `<td width="25%">` is a presentational hint, not CSS — it carries no
/// unit, so the CSS length parser rejects it. Table-built pages centre
/// with exactly this (Google's home page puts the search box between two
/// 25% spacer cells); ignoring it collapses the spacer and slams the
/// content to the left edge.
```

## L13037 · `assert_eq!(left_edge("width=\"200\" style=\"width:100px\""), 100);`

```
// Author CSS still wins over the hint.
```

## L13041-13044 · `#[test]`

```
/// The other half of table-built centring: `<td width="25%">` spacers put
/// the CELL in the middle, `align="center"` puts the content in the middle
/// of the cell. With only the first, Google's search box sat at the left
/// edge of a correctly-placed cell.
```

## L13063 · `assert_eq!(left_edge("align=\"center\" style=\"text-align:left\""), 0);`

```
// Author CSS still wins over the hint.
```

## L13067-13071 · `#[test]`

```
/// A table wider than its content hands the slack to the columns that did
/// NOT ask for a width. Spreading it over all of them widened the sized
/// ones past what they asked for: `25% | auto | 25%` came out 41/18/41,
/// so the middle column — the one with the content in it — ended up the
/// narrowest of the three.
```

## L13083-13084 · `900,`

```
// Deliberately WIDER than the table: a cell percentage is a
// fraction of the table, not of the space it was offered.
```

## L13089 · `assert_eq!(middle(""), (200, 400));`

```
// The auto column takes ALL of it: 800 - 200 - 200 = 400.
```

## L13091 · `assert_eq!(middle("width=\"50%\""), (200, 400));`

```
// Spelling the same thing out explicitly must agree.
```

## L13095-13098 · `#[test]`

```
/// `<center>` centres block-level children, not only inline content —
/// browsers spell it `text-align: -moz-center`, and the `<center><table>`
/// idiom is built on it. Plain CSS `text-align: center` must NOT do this,
/// or every centred paragraph would drag its block children along.
```

## L13125-13129 · `let lines = |display: &str| {`

```
// A max-content width is the width at which the content does NOT wrap.
// Truncating a fractional one to whole pixels loses the last word —
// and the shrink-to-fit paths disagreed about it: a float ceiled, a
// flex item truncated, so the same text wrapped in one and not in the
// other.
```

## L13158 · `let l = table("#a{background:#f00}");`

```
// A row background spans every column, not just one cell.
```

## L13162 · `let text_a = texts(&l).into_iter().find(|(.., t)| *t == "A").unwrap();`

```
// … and it sits BEHIND its cells: the text is emitted after the fill.
```

## L13170 · `let base = table("#b{background:#f00}");`

```
// `position:relative` moves the whole row — background and cells.
```

## L13180 · `let l = table("tbody{background:#f00}");`

```
// A row group is a box too, spanning all of its rows.
```

## L13184 · `let l = table("tr:nth-child(2){background:#f00}");`

```
// Rows have sibling context, so `:nth-child` can stripe them.
```

## L13193-13197 · `let l = lay(`

```
// MediaWiki's shape: a right-floated infobox, then headings whose
// `border-bottom` rule runs the full content width. The rule is a later
// in-flow block box, so it must paint UNDER the float (CSS2.1 Appendix
// E: in-flow blocks, then floats) — otherwise it is drawn straight
// across the table.
```

## L13209 · `let over = |z: &str| {`

```
// A raised z-index still wins over the float layer …
```

## L13224 · `assert!(!over("-1"), "z-index:-1 paints below a float");`

```
// … and a negative one still loses to it.
```

## L13226-13229 · `let l = lay(`

```
// The float layer has to work INSIDE a tracked z-index range too:
// MediaWiki wraps a whole article in one positioned container, which is
// why the first attempt (float ranges only at depth 0) fixed nothing on
// the real page. The enclosing range gets cut around the float instead.
```

## L13248 · `let l = lay(`

```
// Wikipedia's shape: floated TABLE (not a div), then a heading with a rule.
```

## L13280-13281 · `assert_eq!(reds(&table("tbody tr td{background:#f00}")).len(), 2, "tbody tr td must match");`

```
// The row and its group are on the ancestor path, so a descendant
// selector through them matches at all.
```

## L13283 · `let first = reds(&table("td:first-child{background:#f00}"));`

```
// A cell knows its position among its siblings.
```

## L13288-13289 · `let colour_of = |css: &str| {`

```
// Inherited properties reach the cell through the row, not from the
// table two levels up.
```

## L13321 · `let (cap, cell) = cap_vs_cell("table{caption-side:bottom}");`

```
// Inherited, so setting it on the table reaches the caption.
```

## L13330-13332 · `let boxes = |css: &str| {`

```
// DuckDuckGo reserves the hover frame around every search result with
// `border: 1px solid rgba(0,0,0,0)`. Painting the carrier colour put a
// black box around each result.
```

## L13347 · `assert_eq!(h_opaque, h_clear, "transparent border still occupies space");`

```
// It is a border, not an absence: the box is still the same size.
```

## L13354 · `assert_eq!(rects(&lay("<body><a href=\"/x\">hi</a></body>", 400)), 1);`

```
// A real link gets a decoration rect …
```

## L13356 · `assert_eq!(rects(&lay("<body><a name=\"x\">hi</a></body>", 400)), 0);`

```
// … a bare named anchor is not a link, so it does not.
```

## L13358 · `let off = lay("<body><style>a{text-decoration:none}</style><a href=\"/x\">hi</a></body>", 400);`

```
// … and author CSS can take it away.
```

## L13361 · `let strike = lay("<body><span style=\"text-decoration:line-through\">hi</span></body>", 400);`

```
// `line-through` sits above the baseline, `underline` below it.
```

## L13387 · `#[test]`

```
// ── <details>/<summary> (HTML §4.11.1) ─────────────────────────────────
```

## L13389-13391 · `#[test]`

```
/// The whole point: a CLOSED `<details>` shows its summary and nothing
/// else. Measured on MDN, 117 of 119 sections are closed — rendered open
/// they turn the page into one endless scroll.
```

## L13409-13410 · `#[test]`

```
/// Author CSS must not be able to reveal the skipped contents: a browser
/// hides them through the shadow tree, where no page rule reaches.
```

## L13422-13423 · `#[test]`

```
/// Only the FIRST `<summary>` is the control; a second one is ordinary
/// content and is skipped with the rest.
```

## L13436-13438 · `#[test]`

```
/// No `<summary>` child means the UA provides the legend. Without one the
/// element renders as nothing at all and its contents become unreachable
/// — worse than showing everything.
```

## L13448-13449 · `#[test]`

```
/// A `<summary>` outside a `<details>` is a plain block: no marker, and
/// nothing to click.
```

## L13457-13458 · `#[test]`

```
/// The marker is drawn, points the right way, and stays clickable when the
/// page removes it — most pages write `summary { list-style: none }`.
```

## L13463-13464 · `let cols = |l: &Layout| {`

```
// A right-pointing triangle is columns (w == 1), a down-pointing one
// is rows (h == 1).
```

## L13487-13488 · `#[test]`

```
/// The toggle rect covers the summary, and hit-testing finds it there and
/// not over the rest of the page.
```

## L13497-13502 · `#[test]`

```
/// `display: contents` does NOT reparent: a `<summary>` under an unboxed
/// `<div>` is still not the `<details>`' control, and the whole `<div>` is
/// skipped with everything in it (css-display-3, and the wpt reftest
/// `display-contents-details-001`). The mirror risk is the one that would
/// hurt: if the ancestor chain dropped unboxed elements, every grandchild
/// of a closed `<details>` would look like a child and vanish.
```

## L13515-13516 · `let o = lay(`

```
// The mirror: an OPEN details must still show what is nested under an
// unboxed child.
```

## L13526-13529 · `#[test]`

```
/// A grandchild is not a child: only the `<details>`' own element children
/// are skipped, and they take their subtrees with them because they are
/// `display:none`. If the ancestor chain were ever flattened this test
/// would keep the page from silently losing everything one level down.
```

## L13541-13544 · `#[test]`

```
/// A `<dialog>` without `open` is not rendered (HTML §4.11.4). Left
/// visible, a modal's content lands in the middle of the flow — and the
/// page's own `dialog[open]` rules would never have hidden it, because a
/// browser does that in the UA sheet.
```

## L13556-13557 · `let forced = lay(`

```
// Unlike a closed `<details>`, this one IS an ordinary UA rule: a page
// that shows its dialog with CSS still can.
```

## L13566-13577 · `#[test]`

```
/// **`<noscript>` ist inert, weil beak Skripte faehrt.**
///
/// Dieser Test stand einmal umgekehrt da, und er hatte recht: solange
/// beak kein JavaScript hatte, war der Inhalt eines `<noscript>`
/// gewoehnliches Markup und gehoerte gerendert (HTML §15.3.1). Seit
/// Stage 1 stimmt die Voraussetzung nicht mehr.
///
/// Was daran haengt, zeigt Googles Ergebnisseite: sie legt in ihr
/// `<noscript>` ein `<style>table,div,span,p{display:none}</style>` und
/// ein `<meta http-equiv="refresh">` auf ihre „bitte aktiviere
/// JavaScript"-Seite. Als Markup gelesen versteckt das jede Tabelle,
/// jeden Kasten und jeden Absatz — und navigiert dann weg.
```

## L13585-13586 · `let l2 = lay(`

```
// Ein `<img>` darin darf NICHT geholt werden — sonst laedt die Seite
// Bilder, die nur fuer den skriptlosen Fall gedacht sind.
```

## L13596 · `let l3 = lay("<body><noscript><style>p{display:none}</style></noscript>danach</body>", 800);`

```
// Der Rohtext darf auch nicht als TEXT auf der Seite landen.
```

## L13602 · `let l4 = lay("<body><script>var x = 1</script><style>p{}</style>after</body>", 800);`

```
// `<script>`/`<style>` bleiben wie bisher versteckt.
```

## L13621-13622 · `let l = lay(`

```
// "Hello <a>Wikipedia</a> and <a>Phosphor</a> here" must lay onto ONE
// line (wide viewport) — the whole point of inline flow.
```

## L13631 · `assert_eq!(l.links.len(), 2);`

```
// Both links are clickable and to the right of the leading text.
```

## L13651-13653 · `#[test]`

```
/// `attr()` in generated content reads the originating element's
/// attribute. A missing one is the EMPTY STRING, not a dropped declaration
/// — the box is still generated, so the brackets around it still show.
```

## L13662-13663 · `assert!(text("attr(DATA-X)", "<p data-x=\"MiXeD\">y</p>").contains("MiXeD"));`

```
// The attribute NAME is case-insensitive (the parser lowercases it);
// the VALUE keeps its case.
```

## L13665-13666 · `assert!(!text("attr(data-x px)", "<p data-x=\"5\">y</p>").contains('5'));`

```
// A type/fallback argument is css-values-5 — out of scope, so the whole
// declaration is dropped rather than half-applied.
```

## L13670-13671 · `#[test]`

```
/// `text-indent` moves the FIRST line box only — every later line starts at
/// the content edge again.
```

## L13721-13722 · `let l = lay(`

```
// A type rule colours all <p>; a descendant rule colours links inside
// .box only; specificity: #id beats the type rule on the same element.
```

## L13742 · `assert_eq!(color_of("blue"), Some(Rgba::opaque(Rgb(0, 0, 255)))); // #id wins over p`

```
// #id wins over p
```

## L13743 · `assert_eq!(color_of("green"), Some(Rgba::opaque(Rgb(0, 255, 0)))); // .box a matched`

```
// .box a matched
```

## L13771-13772 · `let base = lay("<body><p>a</p><p>b</p></body>", 800);`

```
// The relative <p> is nudged right+down; the following <p> keeps the
// normal-flow position (relative reserves its original space).
```

## L13781-13786 · `#[test]`

```
/// **Ein Prozent an einem `inline-block` loeste sich ZWEIMAL auf.**
/// `inline_block_box` reicht `layout_box` den EIGENEN Randkasten als
/// Umgebungsbreite — der Vertrag fuer `width: auto` und eine Falle fuer
/// `width: 50%`: die Haelfte der Haelfte. `place_float` kennt und benennt
/// denselben Fall seit 0.138.0; der Inline-Weg war der letzte, der ihn
/// noch hatte. Bootstraps `.placeholder.col-6` kam 469 statt 939 px.
```

## L13798-13803 · `#[test]`

```
/// **Ein negativer Rand verschiebt einen Float, er verbreitert ihn nicht.**
/// `place_float` klemmte `margin-left` mit `.max(0.0)` ab, `layout_box`
/// zog ihn unten ungekuerzt wieder ab — der Kasten wuchs um genau den
/// Betrag (60 statt 20 px). Floats sind von CSS 2.1 §8.3 nicht
/// ausgenommen, und `float:left; margin-left:-1.5em` ist die Bauweise
/// jeder Bootstrap-Checkbox.
```

## L13818-13819 · `let l = lay(`

```
// The absolute badge is positioned at cb.left+left / cb.top+top; the
// sibling <p> flows as if the badge weren't there.
```

## L13828-13829 · `assert_eq!(badge.0, 8 + 30, "abs left = cb.left + left");`

```
// cb = the relative div's content box, which starts at the body's UA
// margin (8px) — the page has no gutter of its own.
```

## L13832 · `let without = lay("<body><div style=\"position:relative\"><p>flow</p></div></body>", 800);`

```
// out of flow: the sibling <p> lands where it would with no badge at all.
```

## L13839-13842 · `let l = lay(`

```
// `.container { max-width:400px; margin:0 auto; padding:20px }` on an
// 800px viewport (body content width 784): the box is capped to 400 and
// centered → left margin (784-400)/2 = 192, +body margin(8) +pad_left(20)
// → x≈220.
```

## L13856-13860 · `let long: String = (0..60).map(|i| alloc::format!("<p>Zeile {i} mit etwas Text</p>")).collect();`

```
// `Layout::height` is the scrollable extent, and the shell scrolls by
// it — so a root box SHORTER than its content must not shorten the
// page. `html { height: 100% }` is an everyday idiom; taking the root's
// border-box bottom for the page height truncated such a page to one
// viewport and stopped scrolling outright (0.3.13 → fixed in 0.3.14).
```

## L13870-13875 · `#[test]`

```
/// Flexbox §9.4 step 7: a line is as tall as its items' HYPOTHETICAL cross
/// sizes — the natural size clamped by the item's own `min-`/`max-height`.
/// Sizing the line from the raw natural height left it short of any item
/// held open by a `min-height`, and that item then hung out below the
/// container. This is Wikipedia's search bar: a `min-height: 32px` button
/// beside a shorter field, and the button's border sat 2px past the group's.
```

## L13900-13904 · `#[test]`

```
/// A bordered flex item stretched to the line must end up exactly as tall
/// as the line — not `border_y()` taller. `flex_item_style` handed the
/// stretched BORDER-box size back as a content height with only the padding
/// removed, which is the same box-model twin bucket item 31 removed from
/// two other places.
```

## L13919-13923 · `#[test]`

```
/// An OUTER `box-shadow` is cut out of its own border box (CSS Backgrounds 3
/// §7.1.1), so `0 1px <color>` leaves exactly a 1px strip below the box.
/// Real pages use that as a hairline separator far more often than as a drop
/// shadow — MediaWiki rules off its article tabs with it, and painting the
/// shadow as an unclipped copy floods the whole row instead.
```

## L13930-13931 · `let r = shadow_rects("height:20px;box-shadow:0 1px rgb(1,2,3)");`

```
// A rule under the box: one strip, its height the y-offset, and it sits
// BELOW the border box rather than over it.
```

## L13936 · `assert_eq!(shadow_rects("height:20px;box-shadow:0 0 0 3px rgb(1,2,3)").len(), 4);`

```
// A spread with no offset rings the box on all four sides.
```

## L13938 · `assert!(shadow_rects("height:20px;box-shadow:0 0 rgb(1,2,3)").is_empty());`

```
// Fully covered by its own box → nothing to paint.
```

## L13940-13941 · `assert!(shadow_rects("height:20px;box-shadow:0 2px 8px rgb(1,2,3)").is_empty());`

```
// A BLURRED shadow is skipped rather than drawn as a hard slab, and an
// inner one is a different paint entirely.
```

## L13943-13946 · `let ins = shadow_rects("height:20px;box-shadow:inset 0 1px rgb(1,2,3)");`

```
// Ein INNERER Schatten malt seit 0.62.0 — und zwar INNEN: die Fuellung
// minus dem Loch, das er freilaesst. `inset 0 1px` verschiebt das Loch
// um eins nach unten, uebrig bleibt ein Streifen oben IM Kasten.
// Bootstrap streift damit seine Tabellen.
```

## L13950-13951 · `let l = lay("<body><div style=\"height:20px;box-shadow:0 1px;color:rgb(1,2,3)\">x</div></body>", 400);`

```
// `currentColor` is the LAST colour, not whatever was cascaded when the
// shadow was parsed — same rule the border sides follow.
```

## L13956-13962 · `#[test]`

```
/// A `box-shadow` list is painted from the first layer we HAVE a paint for,
/// not from layer one. DuckDuckGo's searchbox ring is the third layer of
/// `0 10px 20px …, 0 2px 6px …, 0 0 0 1px rgba(0,0,0,.08)`; taking layer one
/// picked a blurred shadow, which paint then skipped, so the box lost its
/// outline entirely. Measured over duckduckgo.com and two Wikipedia
/// articles: 7 declarations hide their only sharp layer behind a blurred
/// one, and none has two paintable layers.
```

## L13964-13970 · `#[test]`

```
/// Ein nackter Textlauf in einem Flex-Container ist ein ANONYMER Kasten
/// (css-flexbox-1 §4) — und verschwand bis 0.66.0 spurlos.
///
/// `<div class="flex">Label<span>x</span></div>` verlor sein „Label".
/// Das ist die schlimmste Sorte Layoutfehler: er LOESCHT Text, statt ihn
/// falsch zu setzen, und auf dem Schirm fehlt nur etwas, von dem niemand
/// weiss, dass es da sein sollte.
```

## L13980 · `assert!(texts[1].0 > texts[0].0, "das zweite Kind steht rechts: {texts:?}");`

```
// Nebeneinander, nicht untereinander — und auf derselben Zeile.
```

## L13985-13995 · `#[test]`

```
/// **Eine Spalte, die nicht schrumpfen kann, macht den Tisch nicht
/// breiter** (CSS2.1 §17.5.2.2).
///
/// Beim Verteilen bekam jede Spalte ihren Anteil und danach `.max(minw)` —
/// und was das `.max` dazulegte, wurde niemandem weggenommen. Eine
/// Bildspalte (Minimum = ihre Breite) machte den Tisch damit breiter als
/// den Platz, den er hat: auf Wikipedias „Today's featured picture" 1548
/// statt 1296, und der Text lief 252 px aus seinem Kasten.
///
/// An Chromium gemessen: 404 | 892 in einem 1296er Kasten — Minimum
/// sichern, den Rest im Verhaeltnis von `pref - minw`.
```

## L14011 · `assert!(w("table") >= 590, "…und fuellt ihn: {}", w("table"));`

```
// Und er faellt auch nicht in sich zusammen — `width:100%` gilt.
```

## L14015-14024 · `#[test]`

```
/// **Ein Flex-Item hat seinen eigenen Formatierungskontext** — ein Float
/// darin reicht nicht zum Nachbarn (css-flexbox-1 §4).
///
/// Der Behaelter isolierte schon nach aussen; zwischen den GESCHWISTERN
/// lief die Float-Liste weiter. Auf Wikipedias Hauptseite raeumte deshalb
/// der Float der linken Spalte den Clearfix der rechten: „In the news"
/// mass 566x696 statt 531x351 und schob alles darunter weg.
///
/// An Chromium gemessen (`<tools>/gallery/run.py`): der zweite Kasten ist
/// 20 px hoch, nicht 400.
```

## L14044-14056 · `#[test]`

```
/// **Ein Kasten, dessen Befehle wieder herausgezogen werden, darf keinen
/// Stapelbereich zuruecklassen.**
///
/// Der Inhalt eines `<button>` wird ausgelegt und dann aus `ops` gedraint;
/// notiert ein Kind dabei einen Bereich, zeigt der hinterher auf die
/// NAECHSTEN Befehle der Seite. Auf Wikipedia sortierte ein Bereich aus
/// dem Suchknopf die Flaeche eines spaeteren Knopfes hinter dessen eigene
/// Beschriftung — der Knopf neben „Appearance" war eine graue Kiste ohne
/// Text, und jeder Klick darauf kostete ein volles Auslegen, weil seine
/// Spanne zerrissen war.
///
/// Geprueft wird die REIHENFOLGE, nicht die Lage: die Flaeche eines
/// Knopfes gehoert vor seine Beschriftung.
```

## L14064-14065 · `let first_text = l.ops.iter().position(|o| matches!(o, DrawOp::Text { .. }));`

```
// Der erste Befehl der Seite ist die Flaeche des ERSTEN Knopfes —
// vorher stand sie ganz am Ende, hinter allem anderen.
```

## L14074-14075 · `#[test]`

```
/// Dasselbe fuer einen atomaren Inline-Kasten: auch seine Befehle wandern
/// aus `ops` heraus, und auch er liess Bereiche stehen.
```

## L14086-14088 · `assert!(pos("ikone") < pos("zwei") && pos("zwei") < pos("danach"),`

```
// Ohne die Ruecknahme wanderte „ikone" ans ENDE der Seite: der
// Bereich, den das positionierte Kind im Inline-Block notiert hatte,
// zeigte hinterher auf dessen eigene, wieder eingesetzte Befehle.
```

## L14094-14095 · `#[test]`

```
/// Reiner Leerraum zwischen zwei Kaesten erzeugt KEINEN Kasten (§4) —
/// sonst bekaeme jede eingerueckte Quelle unsichtbare Flex-Kinder.
```

## L14103-14105 · `#[test]`

```
/// css-flexbox-1 §8.1: eine `auto`-Marge frisst den freien Platz auf IHRER
/// Achse. In der Spalte ist das die Hauptachse — `margin-top:auto` auf dem
/// letzten Kind heftet es an den Boden (das Karten-Fussmuster).
```

## L14121-14122 · `#[test]`

```
/// Auf der Querachse ueberstimmt sie `align-items` — und den Stretch.
/// `mt-auto` in einer ZEILE heisst unten, `my-auto` heisst Mitte.
```

## L14139-14141 · `#[test]`

```
/// In der Spalte ist links/rechts die QUERachse: `mx-auto` zentriert, und
/// dazu muss der Stretch weichen — sonst ist der Kasten schon so breit wie
/// die Zeile und es bleibt nichts zu verteilen.
```

## L14155-14156 · `#[test]`

```
/// `opacity` unter 1 verblasst den Kasten UND seinen Teilbaum. Gemessen
/// wird die Alpha der Befehle, nicht die Farbe — die bleibt.
```

## L14177-14179 · `#[test]`

```
/// Eine Schatten-Schicht mit Alpha 0 malt nichts — und darf deshalb den
/// einen scharfen Platz nicht belegen. Tailwind stellt genau so eine als
/// Platzhalter VOR den echten Ring.
```

## L14191-14192 · `#[test]`

```
/// `currentcolor` ist die Vorgabe von `box-shadow` — ausgeschrieben muss
/// sie dasselbe heissen wie weggelassen, sonst ist die Schicht ungueltig.
```

## L14204-14207 · `#[test]`

```
/// Ein Steuerelement wird mit UNSEREN Massen gemalt, also muss die Seite
/// ihren `border-radius` mitgeben koennen. Ohne ihn hatte JEDER Knopf auf
/// einer Bootstrap- oder Tailwind-Seite scharfe Ecken — das erste, was
/// nach „kein Browser" aussieht.
```

## L14221 · `let sq = lay(`

```
// Und ohne Radius bleibt es beim Rechteck — kein Ring, wo keiner hin soll.
```

## L14229-14233 · `#[test]`

```
/// Ein WEICHER Schatten wird gemalt — und zwar weich.
///
/// Bis 0.61.0 fiel er ganz weg (nur `blur == 0` wurde gemalt), und das
/// betraf jede Bootstrap-Karte, jeden Dialog und jedes Menue: sie lagen
/// flach auf der Seite statt darueber.
```

## L14239-14240 · `let mut buf = alloc::vec![0u8; 400 * 120 * 4];`

```
// Und er ist wirklich weich: die Deckung faellt nach aussen ab. Ohne
// die Pruefung koennte er ein harter Klotz sein und der Test gruen.
```

## L14249-14258 · `#[test]`

```
/// **Eine CSS-Laenge wird ueberall auf dieselbe Art ganzzahlig: gerundet.**
///
/// `CSS2/floats-019` stellt `padding-top: 1.1in` gegen `margin: 1.1in`
/// — 105,6 px, zweimal dieselbe Zahl. Wer den Rand rundet und die
/// Polsterung abschneidet, bekommt 106 gegen 105, und der Test scheitert
/// an der Umrechnung statt an der Regel.
///
/// Gerundet statt abgeschnitten, weil sich Abschneiden ADDIERT: jeder
/// Kasten setzt auf der Unterkante des vorigen auf, und auf einer nackten
/// Seite mit sechs Ueberschriften waren das 8 px bis zum letzten `<div>`.
```

## L14266-14267 · `let a = y_of("<div style='padding-top:1.1in'></div>");`

```
// 1.1in = 105.6 px. Als Polsterung des Elters und als eigener Rand
// muss dieselbe Zahl herauskommen.
```

## L14274-14280 · `#[test]`

```
/// **Der Kasten einer Tabelle ist die Tabelle, nicht ihr Streifen** — und
/// die `<caption>` ist so breit wie die Tabelle, nicht wie der Streifen.
///
/// Eine Tabelle mit `width: auto` schrumpft auf ihren Inhalt (§17.5.2) und
/// MALT auch so; gemeldet wurde trotzdem die angebotene Breite. Auf einer
/// nackten Seite waren das 1886 statt 157 px, und eine Ueberschrift stand
/// ueber der ganzen Fensterbreite statt ueber ihrer Tabelle.
```

## L14294 · `let l = lay_inspect(`

```
// Die Gegenprobe: eine Tabelle mit `width: 100%` FUELLT den Streifen.
```

## L14301-14313 · `#[test]`

```
/// **Fallen die Raender aller Kinder durch, gehoeren sie an den OBERRAND
/// des Elters** — und eine Raeumung SETZT die Oberkante, statt den Rand
/// obendrauf zu legen.
///
/// An Chromium gemessen (`getBoundingClientRect`), sieben Faelle:
///
///     leeres Kind 50px Unterrand, Elter frei      Elter y=50 h=0
///     dasselbe mit min-height:20px                Elter y=50 h=20
///     dasselbe mit height:20px                    Elter y=50 h=20
///     zwei leere Kinder, letztes 50px             Elter y=50 h=20
///     Kind enthaelt nur einen Float               Elter y=50 h=20, Float y=50
///     Kind mit Hoehe 30 (faellt NICHT durch)      Elter y=10 h=30
///     Elter mit Rahmen oben (kein Zusammenfall)   Elter y=10 h=21
```

## L14318-14319 · `let boxes = |css: &str, body: &str| -> Vec<(String, i32, i32)> {`

```
// Ein Etikett -> (Oberkante, Hoehe). Die Liste wird mitgegeben, weil
// ein Fehlschlag ohne sie nur „kein Kasten" sagt.
```

## L14336-14337 · `let v = boxes(".p{margin-top:10px;min-height:20px}.c{margin-bottom:50px}.f{float:left}",`

```
// Ein Kind, das durchfaellt, kann trotzdem etwas malen: sein Float
// wandert mit. Genau daran haengt `margin-collapse-min-height-001.xht`.
```

## L14342-14343 · `let v = boxes(".p{margin-top:10px;min-height:20px}.c{margin-bottom:50px;height:30px}", kind);`

```
// Die Gegenprobe: ein Kind mit Hoehe faellt nicht durch, sein
// Unterrand entkommt nach unten und der Elter bleibt oben stehen.
```

## L14346-14347 · `let v = boxes(".p{margin-top:10px;min-height:20px;border-top:1px solid #000}.c{margin-bottom:50px}", kind);`

```
// Und ohne Zusammenfall am Oberrand (Rahmen dazwischen) bleibt alles,
// wie es war: der Rand des Kindes wird verschluckt.
```

## L14352-14359 · `#[test]`

```
/// Eine Raeumung SETZT die Oberkante des Kastens; ein Rand, der erst beim
/// Auslegen der Kinder gefunden wird, geht in die HYPOTHETISCHE Lage ein
/// und wird von der Raeumung geschluckt, sobald der Float tiefer reicht.
///
/// `CSS2/margin-collapse-157` prueft genau das mit sechs Quadraten, die
/// gleich aussehen muessen: der leere Kasten mit `margin: 1em` darin darf
/// den geraeumten Kasten NICHT unter den Float schieben. Chromium setzt
/// alle drei Formen auf dieselbe Oberkante.
```

## L14379-14388 · `#[test]`

```
/// Eine Schattenliste hat DREI Plaetze — den ersten SCHARFEN, den ersten
/// WEICHEN und den ersten INNEREN Anteil —, und eine Schicht, die nichts
/// malt, belegt keinen davon.
///
/// Der Test hiess bis hierher „malt die erste MALBARE Schicht, nicht die
/// erste" und beschrieb damit den Stand vor 0.61.0: weiche Schichten
/// fielen weg, `inset` gab es nicht. **Er stand seit 0.61.0 ohne `#[test]`
/// da** — die Zeile wurde beim Einfuegen des Nachbartests darueber
/// verbraucht —, und genau deshalb ist niemandem aufgefallen, dass seine
/// Behauptung im selben Commit falsch wurde.
```

## L14391 · `let shadow = |css: &str| -> (Vec<(i32, i32, i32, i32, Rgb)>, usize) {`

```
// Alle Rechtecke einer Farbe, dazu die Zahl der weichen Schatten.
```

## L14397-14401 · `let (r, soft) = shadow(`

```
// Der Kasten: `body` hat 8 px Rand, das `div` ist 384x20 bei (8,8).
// Die DDG-Form: zwei weiche Schichten, dann der 1-px-Ring, der zu
// sehen ist. Vier Seiten, weil eine reine Ausdehnung den Kasten
// umrandet — und **ein** weicher Schatten, nicht zwei: die zweite
// weiche Schicht findet ihren Platz besetzt.
```

## L14408-14410 · `let (r, _) = shadow("height:20px;box-shadow:inset 0 0 0 2px rgb(9,9,9),0 1px rgb(1,2,3)");`

```
// Ein `inset` belegt den scharfen Platz NICHT — der Streifen darunter
// gehoert der zweiten Schicht —, und gemalt wird er trotzdem: vier
// Seiten nach innen, in seiner eigenen Farbe.
```

## L14420-14422 · `let (r, soft) = shadow("height:20px;box-shadow:0 2px 8px rgb(1,2,3),inset 0 1px rgb(1,2,3)");`

```
// Ohne scharfen Anteil bleibt der Kasten ohne Ring: was malt, ist der
// weiche Schatten und der innere Streifen IM Kasten (y = 8), nicht
// darunter.
```

## L14427-14429 · `let (r, _) = shadow("height:20px;box-shadow:0 1px rgb(1,2,3);box-shadow:inset 0 1px rgb(1,2,3)");`

```
// `inset` ist gueltiges CSS — die zweite Deklaration ERSETZT die erste,
// statt als schlechter Wert zu verfallen. Der Streifen wandert damit
// von unter dem Kasten (y = 28) in ihn hinein (y = 8).
```

## L14433-14434 · `let (r, _) = shadow("height:20px;box-shadow:0 1px rgb(1,2,3);box-shadow:0 1px wobble(3)");`

```
// Eine Schicht, die wir nicht LESEN koennen, verwirft dagegen die ganze
// Deklaration — der Kasten behaelt den Schatten, den er schon hatte.
```

## L14440-14445 · `#[test]`

```
/// A control the page made block-level is a BLOCK box, not an atomic inline.
/// An atomic inline sits on the baseline, so its parent came out the
/// control's height plus the descender — which drew a second rule 2px under
/// a search field whose wrapper is pulled onto the group border with
/// `margin: -1px`. It must still paint as a CONTROL (face, value,
/// placeholder), not as an ordinary block with a CSS border.
```

## L14461-14462 · `let inl = lay(`

```
// The inline default keeps its line box — this changes only what the
// page explicitly asked to be block-level.
```

## L14472-14478 · `#[test]`

```
/// `transform: translate(...)` shifts the paint, and its percentages are of
/// the BOX — that is what makes `translate(-50%,-50%)` centre. Together with
/// `top: 50%` against a positioned ancestor of AUTO height (§10.1: the
/// containing block is its used padding box, definite once laid out) this is
/// the icon-centring idiom every component library uses. Taking the
/// containing block from the SPECIFIED height left `top:50%` unresolvable,
/// so the box fell back to its static position — a full box-height too low.
```

## L14489-14490 · `assert_eq!(icon.1, 8 + 6, "centred, not at its static position");`

```
// Parent's padding box is 32 tall from y=8 → 50 % is 24, less half the
// icon = y+6. Falling back to the static position would give y+32.
```

## L14493-14494 · `let l2 = lay(`

```
// A pixel translate on an in-flow box shifts the paint without moving
// anything else, and `translate(x,y)` takes both axes.
```

## L14502 · `let l3 = lay(`

```
// Anything that is not a translation is dropped rather than guessed at.
```

## L14512-14520 · `#[test]`

```
/// The width MEASUREMENT must resolve styles with the same sibling context
/// the layout walk uses, or a sibling-combinator rule is applied by one and
/// ignored by the other — and the two then disagree about the same box.
///
/// Every component library hides an icon-only button's label with the
/// visually-hidden idiom on `span + span`. Measuring without the siblings
/// left the label in flow for sizing purposes, so the button came out as
/// wide as its hidden text: Wikipedia's hamburger was ~80px too wide and
/// shoved the logo and the search field right across the whole header.
```

## L14536-14541 · `#[test]`

```
/// The presentational half of the old web: `<center>` is a BLOCK
/// (HTML rendering §15.3.2) and `bgcolor` is a background hint (§15.3.3).
/// Left as the initial `inline`, `<center>` swallows what it wraps into a
/// line box — and a `<table>` inside it collapses into running text.
/// Hacker News wraps its whole page in one and paints its masthead with
/// `bgcolor`, so it rendered as a single grey paragraph.
```

## L14555 · `let bg = |html: &str| {`

```
// `bgcolor` paints, and author CSS still outranks it.
```

## L14565-14573 · `#[test]`

```
/// A line box is not written until it BREAKS, so an out-of-flow box reached
/// mid-line lands in the display list ahead of text that precedes it in the
/// document — and paints under it. CSS 2.1 Appendix E puts positioned boxes
/// in step 8, after that inline content in step 7.
///
/// The box is lifted over exactly that one line. Flushing the line instead
/// would break `foo<div style=position:absolute></div>bar` onto two lines,
/// and lifting positioned boxes wholesale is worse: out-of-flow-only
/// measured +25/−21 against the reftests, every positioned box +16/−46.
```

## L14579-14580 · `let ops = order(`

```
// The green box covers the red one exactly; only paint order decides
// whether any red is left, and the green one comes LATER in the source.
```

## L14591-14594 · `let ops = order(`

```
// …and it is lifted over the line only, not over a box that FOLLOWS it.
// Both of these are step 8, so document order decides and the blue one
// wins — this is the shape (`CSS2/border-005`) that a blanket hoist got
// wrong.
```

## L14609-14611 · `let box_of = |html: &str| {`

```
// CSS2.1 §10.3.2 + §10.6.2. We never load a frame, a video or a canvas
// bitmap — but the BOX is still there, and on the real web that box is
// every video embed and every embedded map.
```

## L14623 · `assert_eq!(`

```
// The presentational attributes size it — how a video embed states 16:9.
```

## L14628-14629 · `assert_eq!(`

```
// CSS wins over the attribute, and `height: auto` still falls back to
// the intrinsic 150 rather than to the zero its content would give.
```

## L14638-14639 · `let l = lay("<body><object>fallback text</object></body>", 800);`

```
// HTML §4.8.7: when the resource cannot be obtained — and ours never
// can — the element represents its fallback content instead.
```

## L14642 · `let l = lay("<body><object style=\"background:#ff0000\"></object></body>", 800);`

```
// With nothing to fall back to it stays an empty replaced box.
```

## L14650-14653 · `let l = lay("<body><img src=\"/x.png\" alt=\"Foto\" width=\"200\" height=\"100\"></body>", 800);`

```
// With both dimensions given, the box is definite: layout emits ONE
// image op carrying the src, at the authored size, and does NOT need
// the pixels. Drawing the placeholder is the rasteriser's job now, so
// the arriving image is a repaint rather than a re-layout.
```

## L14666-14671 · `let l = lay(`

```
// Die Eigenbreite eines Bildes ist die Untergrenze, unter die ein
// Flex-Element nicht schrumpft (css-flexbox-1 §4.5). `intrinsic_width`
// meldete fuer ein `<img>` NULL — es stand in keinem Zweig —, also
// schrumpfte jedes Bild in einer engen Zeile bis auf den einen Pixel,
// auf den `img_box` klemmt. Auf DuckDuckGos Ergebnisseite war das der
// 90 px hohe Strich von einem Pixel Breite und jedes Favicon daneben.
```

## L14688-14690 · `let l = lay(`

```
// Messung und Auslegung teilen sich `img_box`, also meldet die
// Eigenbreite genau den Kasten, der danach gelegt wird — auch fuer ein
// Bild, dessen Pixel noch nicht da sind ([[feedback_intrinsic_shared_path]]).
```

## L14707-14710 · `let l = lay(`

```
// `width`/`height` am `<img>` sind Praesentationshinweise (HTML
// Rendering §15.3.5-6), also GEHOEREN sie in die Kaskade. Solange sie
// nur `img_box` kannte, sah jeder, der vorher fragt, `auto`: in einer
// streckenden Flexzeile wurde aus 30x30 ein 30x60 (Chromium: 30x30).
```

## L14726-14727 · `let l = lay("<body><img src=\"/x.png\" alt=\"Foto\"></body>", 800);`

```
// No width/height and no decoded pixels → the box is a guess, so a
// later decode really does move the page and the shell must re-lay-out.
```

## L14734-14735 · `let l = lay(`

```
// A repaint is the whole viewport, so the shell asks before paying for
// one. Two images, one on screen and one far below a 500 px fold.
```

## L14744-14745 · `assert!(l.images_in_band(&["/low.png"], 1800, 2800), "scrolled to → repaint");`

```
// Scrolled down to it, the same image is worth a repaint — which is why
// skipping one loses nothing: scrolling marks the page dirty anyway.
```

## L14747 · `assert!(l.images_in_band(&["/low.png", "/top.png"], 0, 500), "one visible is enough");`

```
// A batch repaints if ANY of its images is visible.
```

## L14749 · `assert!(!l.images_in_band(&["/nowhere.png"], 0, 100_000), "not painted → not visible");`

```
// A src this layout never placed cannot be visible.
```

## L14769 · `fn lay_forms(html: &str, w: u32, st: &FormState) -> Layout {`

```
/// Lay out with live form state (what the shell does while the user types).
```

## L14778-14779 · `let l = lay(`

```
// A search form: label text, field and button share one line, and each
// control is clickable at its painted rect.
```

## L14791 · `let t = texts(&l);`

```
// The label sits on that same line, to the left of the field.
```

## L14795 · `let hit = l.hit_control(field.x + 4, field.y + 4).expect("hit");`

```
// Hit-test: a point inside the field finds the field, not the button.
```

## L14804-14808 · `let l = lay(`

```
// Real search boxes sit in a `display:flex` row, which reaches children
// through `layout_box` — NOT the in-flow walk. And table/grid sizing
// lays boxes out speculatively to measure them, discarding the ops; the
// control hit rects must be discarded with them or every control is
// recorded several times (at stale positions → clicks miss).
```

## L14819-14820 · `assert_eq!(field.w, 300, "CSS width sizes the flex item's control");`

```
// As a flex item the control fills the box flex resolved for it, so it
// sits flush beside its neighbour instead of overlapping it.
```

## L14823-14824 · `assert_eq!(l.controls[2].y, l.controls[3].y);`

```
// Grid items stretch to their column (`justify-items: stretch`), share a
// row, and the table's control lands below both.
```

## L14831-14837 · `#[test]`

```
/// **`accent-color` schlaegt das Thema** (css-ui-4 §5.1).
///
/// Eine Seite, die ihre Kaestchen in ihrer eigenen Akzentfarbe will,
/// schreibt genau eine Zeile — `sandbox.nopeek.ch` dreimal
/// (`.network-checkbox input { accent-color: var(--accent) }`). Ohne die
/// Eigenschaft bekam sie unsere Themenfarbe, und Florian sah es am Geraet:
/// „die checkboxen … sehen anders aus".
```

## L14846 · `let eigen = first_rect(":root{--a:#e11d48} input{accent-color:var(--a)}",`

```
// Eine eigene Farbe gilt — auch aus einer CSS-Variablen.
```

## L14853-14856 · `let l = lay("<body><style>input{accent-color:#e11d48}</style>\`

```
// Ein Radioknopf ist RUND — seine Flaeche, sein Ring und sein Punkt
// sind `RoundRect`, und `rects` sammelt nur `Rect`. Erst diese
// Unterscheidung macht den Test zu einem Test: die erste Fassung sah
// eine leere Liste und haette jede Farbe durchgehen lassen.
```

## L14866 · `let thema = first_rect("input{accent-color:auto}", "<input type=checkbox checked>");`

```
// `auto` ist der Anfangswert und laesst das Thema entscheiden.
```

## L14871-14877 · `#[test]`

```
/// **Die UA-Masse eines Steuerelements, gegen Chromium ausgerechnet.**
///
/// Sie standen bis 0.168 als eine Zahl fuer alle da (`PAD_Y = 3`, ein
/// 1-px-Rahmen), und `tools/fixtures/controls.html` hat jede einzeln
/// herausgerechnet: dieselben vier Faelle je Steuerelement — nackt, nur
/// gepolstert, nur gerahmt, beides — ergeben ein Gleichungssystem, das
/// genau eine Loesung hat. Was hier steht, ist diese Loesung.
```

## L14880 · `let h = |tag: &str, css: &str| -> i32 {`

```
// Ein Kasten je Fall, in der Reihenfolge der Vorlage.
```

## L14885-14887 · `let f = |css: &str| h("<input style=\"@\">", css);`

```
// Ein Feld: UA-Polsterung 1 px, UA-Rahmen 2 px je Seite. Die
// Zeilenhoehe ist dieselbe in allen vier Faellen, also ist die
// DIFFERENZ die Aussage — sie haengt nicht an der Schrift.
```

## L14894 · `let cb = |css: &str| h("<input type=checkbox style=\"@\">", css);`

```
// Ein Kaestchen ist 13 px und waechst NICHT mit der Schrift.
```

## L14899 · `let ta = |a: &str| -> i32 {`

```
// Ein `<textarea>` nimmt `rows` — Vorgabe 2, nicht 3 (HTML §4.10.11).
```

## L14908-14913 · `#[test]`

```
/// **Ein Steuerelement im Flex behaelt seine Polsterung.** `flex_metrics`
/// zog Polsterung + Rahmen ein ZWEITES Mal ab (`intrinsic_width` tut es
/// seit 0.145.0 selbst), und `resolve_flex_line` legte sie nur einmal
/// wieder drauf. Gemessen wird gegen denselben Knopf AUSSERHALB eines
/// Flex-Containers: derselbe Text, dieselbe Polsterung, also dieselbe
/// Breite. Chromium sagt zu beiden 74 px; wir sagten 48 im Flex.
```

## L14924-14929 · `#[test]`

```
/// **Ein blockweites Steuerelement ist ein ersetzter Blockkasten**
/// (CSS 2.1 §10.3.4): seine eigene Breite und sein eigener Rand
/// entscheiden. `layout_box_inner` nimmt die uebergebene Breite als
/// GEGEBEN — der Vertrag der Flex-/Raster-/Zellenwege — und `flow_children`
/// uebergab die Breite des UMGEBUNGSkastens: 1000 px breit auf x = 0
/// statt 100 px auf x = 58.
```

## L14941-14946 · `#[test]`

```
/// **Ein geflotetes Steuerelement legt seinen Rand an.** `place_float`
/// uebergibt den RANDkasten und verlaesst sich darauf, dass `layout_box`
/// den Rand anlegt — fuer ein Steuerelement tut es das nicht. Bootstraps
/// `.form-check-input` (`float:left; margin-left:-1.5em` in einem
/// `padding-left:1.5em`) sass deshalb auf der Polsterkante: jede Checkbox,
/// jeder Radioknopf, jeder Schalter.
```

## L14959-14962 · `#[test]`

```
/// A form control's chrome follows the surface it sits on. The engine runs
/// on a DARK theme here, so a page that says nothing keeps dark controls —
/// but a page that paints itself light (Wikipedia does, whatever the
/// desktop is set to) must not get a black box on its white background.
```

## L14965 · `let face = |css: &str| {`

```
// The control's face is the first rect painted for it.
```

## L14979 · `let l = lay(html, 800);`

```
// Empty + unfocused → the placeholder, no caret.
```

## L14985 · `let mut st = FormState::default();`

```
// Typed + focused → the value, plus a 1px caret rect.
```

## L14993-14997 · `assert_eq!(rects(&l2).len(), plain_rects + 4, "der Ring, vier Kanten");`

```
// Der Fokus bringt zweierlei mit: den Caret und den Ring, den ein
// Browser als `outline` AUSSERHALB des Kastens malt (vier Kanten).
// Bis 0.175.0 faerbte beak stattdessen den Rahmen der Seite um — das
// sah auf einem Feld, das seine Farbe selbst gesagt hat, falsch aus,
// und es war falsch.
```

## L14999-15000 · `assert_eq!(l2.ops.iter().filter(|o| matches!(o, DrawOp::Caret { .. })).count(), 1,`

```
// Der Zeiger ist seit 0.188.0 ein eigener Befehl — er zaehlt bei den
// Rechtecken nicht mehr mit, steht aber da.
```

## L15007-15008 · `let l = lay(`

```
// A hidden field takes no space; a <button>'s label paints inside the
// button, and a <select>'s options never leak into page text.
```

## L15023-15026 · `#[test]`

```
/// Google wraps its search button in a bordered `<span>` and writes
/// `border: none` on the `<input>`. Painting our own frame regardless put a
/// second rectangle a pixel down and right of the wrapper's — the "shadow"
/// on both home-page buttons.
```

## L15034-15035 · `assert_eq!(rects(&plain).len(), rects(&bare).len() + 4, "border:none keeps no frame");`

```
// The UA frame is four 1px rects around the face; `border: none` is a
// declaration, not an absence, and removes all four.
```

## L15038 · `let styled = lay(`

```
// The page's own widths and colours, per side.
```

## L15049-15050 · `let clear = lay(`

```
// `border-color: transparent` keeps the width and paints nothing — the
// idiom for reserving a frame's space without showing it.
```

## L15059-15062 · `#[test]`

```
/// A control measured with a ROOT style read its label at the root font
/// size and lost every declared size, so a shrink-to-fit wrapper reserved
/// more width than the control paints — the button sat in a box wider than
/// itself, with a strip of the wrapper showing on the right.
```

## L15077-15080 · `#[test]`

```
/// A button-like control is border-box in the UA sheet (HTML rendering
/// §15.5.1); a text field is not. Read as content-box, Google's
/// `height:30px` button came out 8px taller than the `height:30px` wrapper
/// it was built to fit and hung out the bottom.
```

## L15091 · `assert!(`

```
// An explicit `box-sizing` from the page still wins over the UA sheet.
```

## L15101-15102 · `#[test]`

```
/// Focus is the one part of the frame a page cannot take away: the ring
/// says where typing goes, and `border: none` was never meant to hide that.
```

## L15111-15113 · `assert_eq!(rects(&focused).len(), rects(&l).len() + 4, "der Ring, vier Rechtecke");`

```
// Vier Rechtecke fuer den Ring. Der Zeiger zaehlt seit 0.188.0 NICHT
// mehr mit: er ist ein eigener Befehl, damit er blinken kann, ohne
// dass ein Takt ein Neuauslegen kostet.
```

## L15119-15125 · `#[test]`

```
/// **Sagt die Seite `outline: none`, gibt es keinen Ring — und ihr
/// Rahmen behaelt seine Farbe.**
///
/// So macht es jeder Browser: der Fokus ist eine `outline`, und eine
/// Seite darf sie abschalten. beak faerbte bis 0.175.0 stattdessen den
/// RAHMEN der Seite um; auf DuckDuckGos Suchfeld, das seine Farbe selbst
/// nennt, wurde daraus ein blauer Kasten, den niemand bestellt hatte.
```

## L15135-15136 · `assert_eq!(rects(&focused).len(), rects(&l).len(), "kein Ring");`

```
// Kein Ring — und der Zeiger ist seit 0.188.0 kein Rechteck mehr,
// also kommt bei den Rechtecken GAR nichts dazu.
```

## L15140 · `let red = rects(&focused).iter()`

```
// Und der Rahmen ist noch der der Seite.
```

## L15146-15157 · `#[test]`

```
/// **Das Zeichen ist ein HAKEN, kein Quadrat.** Bis 0.149.0 stand hier
/// „the tick is one filled rect" und der Test hatte recht — gemalt wurde
/// ein gefuelltes Quadrat, also dasselbe Zeichen wie beim Radioknopf,
/// nur eckig. Chromium daneben gestellt zeigte den Unterschied. Der Test
/// prueft jetzt, was die Form BEDEUTET, statt wie viele Rechtecke sie
/// kostet: ohne Haken kein `Check`, mit Haken genau einer.
/// **`min-height` ueber dem Inhalt verschluckt den Schlussrand.**
///
/// Die fuenf Faelle sind die, mit denen die Regel an Chromium
/// charakterisiert wurde (siehe den Kommentar an der Stelle): der Rand
/// verschwindet GENAU dann, wenn `min-height` die Hoehe ueber den Inhalt
/// hebt — sonst entkommt er wie immer und schiebt, was darunter steht.
```

## L15162 · `for (mh, chh, mb, want_h, want_f) in [`

```
// (min-height, Kindhoehe, Rand) -> (Elterhoehe, Fusszeilen-Oberkante)
```

## L15202-15203 · `let l = lay(`

```
// Wikipedia's pattern: <a><img></a> among text. The image must flow on
// the same line as the surrounding words AND be a clickable link.
```

## L15227-15228 · `let l = lay(`

```
// Two flex:1 items in a row → side by side, splitting the width, NOT
// stacked. Without flex they'd be one-below-the-other.
```

## L15254-15255 · `let l = lay(`

```
// A flex container honours its own width/height/background (it establishes
// the box, not just a passthrough for its items).
```

## L15272-15273 · `let l = lay(`

```
// Default align-items:stretch → an auto-height item fills the container's
// definite cross size (100px), not just its one-line content height.
```

## L15288-15289 · `let l = lay(`

```
// Column direction stacks items vertically; their (non-collapsing) margins
// separate them.
```

## L15304-15305 · `let plain = lay("<body><div style=\"width:400px;text-align:right\">abcde</div></body>", 800);`

```
// `letter-spacing` lands after every character, so a right-aligned run
// of five characters starts 5 × the spacing further left.
```

## L15323-15324 · `let one = lay("<body><div style=\"width:400px\">a\u{00A0}b</div></body>", 800);`

```
// `&nbsp;` exists so a line does NOT break there. It is also not
// collapsible, so four of them are four characters wide, not one space.
```

## L15342-15343 · `let l = lay(`

```
// Two 120px items in a 200px wrap container = two 40px lines, packed
// into 200px of cross space: 120px is left over, half of it above.
```

## L15359-15360 · `let l = lay(`

```
// The initial value: no leftover cross space survives, so the second
// line starts halfway down a 200px container.
```

## L15376 · `let l = lay(`

```
// Two 120px items in a 200px wrap container → the 2nd wraps below the 1st.
```

## L15391-15392 · `let l = lay(`

```
// 3 columns, 4 items → items 1-3 on row 1 (distinct x, same y), item 4
// wraps to row 2 under item 1.
```

## L15410 · `let l = lay(`

```
// col 2 spans both tracks → its content box starts at col 0 (full width).
```

## L15425 · `let l = lay(`

```
// 2×2 grid, fixed 50px rows, `gap: 40px 20px` (row-gap 40, column-gap 20).
```

## L15442-15443 · `let l = lay(`

```
// `grid-column: 3` puts the item in the third track (200px offset); a
// later `grid-row: 2` item drops to the second row.
```

## L15462 · `let l = lay(`

```
// `repeat(auto-fill, 100px)` in a 300px grid → exactly 3 columns.
```

## L15482-15483 · `let l = lay(`

```
// An auto-height item defaults to `align-self: stretch` → its background
// fills the 80px explicit row.
```

## L15496 · `let l = lay(`

```
// `grid: <rows> / <columns>` sets both track lists.
```

## L15512-15514 · `let discs: alloc::vec::Vec<f32> = l.ops.iter().filter_map(|o| match o {`

```
// `disc` ist eine SCHEIBE, kein Quadrat — die Form IST der Wert der
// Eigenschaft, sonst sind `disc`, `circle` und `square` auf dem Schirm
// dasselbe Zeichen und eine verschachtelte Liste verliert ihre Ebenen.
```

## L15521 · `assert!(texts(&l).iter().all(|(x, _, _)| *x > 8));`

```
// list text is indented past the plain content edge (the body margin)
```

## L15525-15528 · `#[test]`

```
/// Ein `opacity` auf einem INLINE-Element verblasst seinen Text und seinen
/// Schmuck. Ein Inline-Kasten bekommt keinen eigenen Befehlsbereich, ueber
/// den es nachtraeglich gelegt werden koennte — die Deckung faehrt deshalb
/// im Stil mit, und Vorfahren multiplizieren sich auf.
```

## L15543 · `let bg = l.ops.iter().find_map(|o| match o {`

```
// Und der Hintergrund des Inline-Kastens genauso.
```

## L15551-15552 · `#[test]`

```
/// Und sie multipliziert sich mit der des BLOCKS darueber, statt sie zu
/// ersetzen: der Block legt seine ueber den ganzen Befehlsbereich.
```

## L15570-15573 · `fn cell_span(cell: &Cell) -> usize {`

```
/// The definite **padding-box** height of a positioned box — what `top`/`bottom`
/// percentages on its absolutely-positioned descendants resolve against
/// (CSS 2.1 §9.3.2). Only an explicit `height` counts: abspos children are laid
/// out during the parent's child walk, before its content height exists.
```

## L15575-15577 · `fn cell_span(cell: &Cell) -> usize {`

```
/// `colspan` (HTML §4.9.11): how many columns a cell occupies. `0` means "to
/// the end of the row group" in old HTML and was dropped from the spec, so it
/// folds to 1 like any other unparseable value.
```

## L15585-15587 · `fn row_columns(row: &[StyledCell]) -> (Vec<usize>, usize) {`

```
/// The start column of every cell in `row`, and how many columns the row
/// occupies in total. Without `rowspan` a row's cells simply pack left to
/// right, each taking `colspan` slots.
```

## L15598-15602 · `fn spread_span(track: &mut [f32], c: usize, span: usize, want: f32) {`

```
/// Widen `track[c .. c+span]` just enough that it totals `want`, sharing the
/// shortfall equally. CSS2 §17.5.2.2 leaves the distribution up to the UA; a
/// spanning cell must never dictate a single column's width, which is what
/// made a `<td colspan="2" style="width:290px">` infobox header blow column 0
/// up to the width meant for the whole table.
```

## L15618-15621 · `fn cell_borders(cs: &ComputedStyle, collapse: bool) -> (f32, f32, f32, f32) {`

```
/// A cell's used border widths (left, right, top, bottom). In the collapsed
/// model a border is shared with the neighbouring cell and sits centred on the
/// grid line, so only HALF of it lies inside this cell (CSS2.1 §17.6.2) — that
/// half is what the column widths and the content box have to account for.
```

## L15631-15634 · `fn collapsed_edge(a: &BorderSide, b: &BorderSide) -> BorderSide {`

```
/// The border that wins a collapsed grid line (CSS2.1 §17.6.2.1). Width-first,
/// which is the part that decides real tables; the full style-then-element
/// priority chain only matters when the widths tie, and a tie already draws the
/// same line at the same size.
```

## L15636-15637 · `if a.hidden || b.hidden {`

```
// Rule 1 first: one `hidden` among the boxes meeting at a grid line
// suppresses that line outright, however wide the others are.
```

## L15648-15650 · `fn spacing_of(st: &ComputedStyle) -> (i32, i32) {`

```
/// A table's used `border-spacing` in px. The collapsed border model merges
/// adjacent borders instead of spacing them, so it ignores the property
/// entirely (CSS2.1 §17.6.1).
```

## L15659-15662 · `fn clamp_len(outer: f32, min_w: Len, max_w: Len, box_border: bool, frame: f32) -> f32 {`

```
/// Clamp an outer (border-box) width to `min-width`/`max-width`, which are
/// content-box lengths unless `box-sizing: border-box` is in effect. Only
/// definite px limits apply — a percentage limit needs a containing block that
/// intrinsic sizing does not have yet.
```

## L15675-15676 · `fn side_by_side(st: &ComputedStyle) -> bool {`

```
/// Whether a box lays its children out along the inline axis, so their
/// max-contents add up rather than the widest one winning.
```

## L15686-15690 · `fn intrinsic_size(k: Intrinsic, max_c: f32, min_c: f32, avail: f32) -> f32 {`

```
/// The used size an intrinsic keyword asks for, given the box's own
/// (max-content, min-content) pair and the room it has — css-sizing-3 §5.
///
/// `fit-content` is the shrink-to-fit formula CSS2.1 already used for floats;
/// naming it as a keyword only lets the author ask for it explicitly.
```

## L15699-15702 · `fn vert_len(len: Len, cbh: Option<i32>) -> Option<f32> {`

```
/// Resolve a vertical length against a containing-block height (CSS2.1 §9.3.2 /
/// §10.5). A percentage needs a definite CB height; an indefinite one (the
/// parent's content height doesn't exist yet while its children lay out) leaves
/// it unresolvable, which behaves as `auto`.
```

## L15705-15706 · `Len::Auto | Len::Intrinsic(_) => None,`

```
// An intrinsic keyword on the block axis is the CONTENT height, which
// no containing block can supply — unresolvable here, like `auto`.
```

## L15715-15723 · `fn with_aspect_height(st: &ComputedStyle, cw: f32) -> Option<ComputedStyle> {`

```
/// The CONTENT-box height a definite `height`/`min-`/`max-height` asks for.
/// Under `box-sizing: border-box` the used height spans padding AND border;
/// flex and grid each subtracted only the padding, so every bordered container
/// with a definite height came out two border-widths too tall — and a root
/// `display:flex` stretched between `top`/`bottom` overshot the viewport.
/// Resolve `aspect-ratio` into a used height, once the box's content width is
/// known. Only the width→height direction: that is the one pages use (a card,
/// a video embed, an image placeholder holding its shape while it loads), and
/// the other needs a definite height that a block box in flow does not have.
```

## L15730-15733 · `let frame = if s.box_border { s.pad_left + s.pad_right + s.border_x() } else { 0.0 };`

```
// The ratio governs whichever box `box-sizing` names, so under
// `border-box` the width it divides is the border-box width — and the
// height it yields is one too, which is exactly what `content_height_of`
// then takes the frame back off.
```

## L15750-15751 · `Len::Px(h) if st.box_border => Some((h as i32 - st.border_y() as i32).max(0)),`

```
// `box-sizing:border-box` → the used height already spans padding AND
// border, so the padding box is that minus the border.
```

## L15758-15762 · `fn padding_cb(st: &ComputedStyle, content_x: i32, content_top: i32, content_w: i32) -> PosCb {`

```
/// The containing block a positioned box establishes for its absolutely
/// positioned descendants: its **padding** box, not its content box (CSS2.1
/// §10.1). Given the box's content origin and width, back out to the padding
/// edges — `top: 0` sits just inside the border, and `left: 0` at the padding
/// edge, so a padded container does not push its abspos children inwards.
```

