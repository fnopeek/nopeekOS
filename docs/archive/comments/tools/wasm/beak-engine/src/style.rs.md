# `tools/wasm/beak-engine/src/style.rs` @ 5e0102684

## L1-14 · `use alloc::string::{String, ToString};`

```
//! style.rs — computed style + the UA default stylesheet, as data.
//!
//! docs/spec/CONFORMANCE.md's rule: be *standard-shaped* from the start. Slice-0 baked
//! per-tag pixel sizes into the layout code; this replaces that with a real
//! cascade seam:
//!
//! `​``text
//!   inherited(parent) → UA sheet(tag) → inline style="…"  → ComputedStyle
//! `​``
//!
//! Author `<style>`/linked CSS (selectors, specificity) slot in *between* the
//! UA sheet and inline styles later — the pipeline shape is already correct.
//! Colours resolve against the active `Theme` so pages follow light/dark like
//! the rest of the UI (until pages set their own `color`, which we honor).
```

## L25 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `display` — only the values our layout implements so far.
```

## L31-37 · `Contents,`

```
/// `display: contents` — the element generates NO box at all; its children
/// lay out as if they were the parent's (css-display-3 §3.1). Kept as its
/// own value rather than resolved away in the cascade because which box
/// the CHILDREN need decides how the parent's flow takes them: all-inline
/// children join the line box being built, a block-level one gets a
/// transparent block. `resolve` has already stripped every property that
/// only describes a box, so neither choice can paint or move anything.
```

## L39-41 · `InlineBlock,`

```
/// `display: inline-block` — a block box inside, an atomic inline box
/// outside: it takes part in a line box like an image, but lays its own
/// content out with the full block box model.
```

## L43-46 · `InlineFlex,`

```
/// `display: inline-flex` — innen ein Flex-Container, aussen ein atomarer
/// Inline-Kasten. Bis 0.62.0 wurde es wie `flex` behandelt, also
/// block-artig: eine `.btn-group` legte sich damit ueber die ganze Breite
/// statt sich auf ihre Knoepfe zu schrumpfen.
```

## L49-50 · `Table,`

```
/// `<table>` — establishes the (simplified) table formatting context in
/// `layout.rs`; its `tr`/`td`/`th` descendants are laid by that walker.
```

## L52 · `Flex,`

```
/// `display: flex` — flex formatting context (single-line) in `layout.rs`.
```

## L54 · `Grid,`

```
/// `display: grid` — grid formatting context (explicit columns + auto rows).
```

## L56-59 · `TableCaption,`

```
/// `display: table-caption` — a `<caption>` box by any other name. Sized to
/// the finished table rather than sizing it, so it must be recognised or a
/// long caption widens the table it describes (MediaWiki's image thumbs are
/// exactly this: `figure{display:table}` + `figcaption{display:table-caption}`).
```

## L61 · `TableRow,`

```
/// `display: table-row` — a row inside a (CSS) table. Laid by `layout_table`.
```

## L63 · `TableRowGroup,`

```
/// `display: table-row-group` — a plain row group (`<tbody>`).
```

## L65-66 · `TableHeaderGroup,`

```
/// `display: table-header-group` (`<thead>`) — its rows sort before every
/// other row group regardless of source order (CSS2.1 §17.2.1 / HTML §15).
```

## L68-69 · `TableFooterGroup,`

```
/// `display: table-footer-group` (`<tfoot>`) — its rows sort after every
/// other row group regardless of source order.
```

## L71-72 · `TableCell,`

```
/// `display: table-cell` — a cell inside a (CSS) table. Outside a table
/// context it degrades to a block box.
```

## L74-79 · `TableColumn,`

```
/// `display: table-column`/`table-column-group` (CSS2.1 §17.2.1): these
/// generate no box of their own (they only carry column properties, which
/// this engine's simplified table layout doesn't apply per-column). Kept
/// distinct from `Other`-ish content so `layout.rs` can tell a real column
/// marker (never rendered, regardless of what tag carries the value) apart
/// from arbitrary stray content (which anonymous-box-wraps instead).
```

## L84-86 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `text-align` — how a block container distributes its line boxes'
/// leftover inline space. `Start`/`End` are the writing-mode-relative pair;
/// this engine is LTR-only, so `Start == Left` and `End == Right` at use time.
```

## L94-96 · `Justify,`

```
/// Stretch every line but the last to fill the line box. Not implemented
/// (our line segments merge adjacent same-style words, so there are no
/// per-word boxes left to expand) — laid out as `Start`.
```

## L100-103 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// CSS `line-height`. A unitless number inherits AS a number (each descendant
/// resolves it against its own font-size); a length/percentage inherits as the
/// already-computed px. Keeping the two apart is what makes `body{line-height:
/// 1.5}` scale a nested heading instead of squashing it.
```

## L106 · `Normal,`

```
/// `normal` — use the face's own line metrics.
```

## L113-114 · `pub fn px(self, font_px: f32) -> Option<f32> {`

```
/// The used line-height in px for a box at `font_px`, or `None` for
/// `normal` (the caller falls back to font metrics).
```

## L124 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `text-transform` — a rendering-time case mapping of the text content.
```

## L133-134 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `list-style-type` — the marker a `display:list-item` box generates.
/// Inherited, so setting it on the `<ul>`/`<ol>` reaches every `<li>`.
```

## L147 · `LowerGreek,`

```
/// The 24 Greek letters, final sigma excluded (css-counter-styles-3).
```

## L149 · `Armenian,`

```
/// Additive, 1..=9999.
```

## L151 · `Georgian,`

```
/// Additive, 1..=19999.
```

## L153 · `DisclosureClosed,`

```
/// A `<summary>`'s disclosure triangle, pointing right (closed).
```

## L155 · `DisclosureOpen,`

```
/// The same, pointing down (open).
```

## L160 · `pub fn is_bullet(self) -> bool {`

```
/// A glyph marker (bullet) rather than a counter string.
```

## L164-166 · `pub fn is_disclosure(self) -> bool {`

```
/// A disclosure triangle. Drawn as a shape, not as a glyph: the subsetted
/// Inter faces carry no U+25B8/U+25BE (`assets/subset.sh` keeps
/// U+0000-2E7F, and Inter simply has no small triangles in it).
```

## L172 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `table-layout` — how a table computes its column widths.
```

## L175 · `Auto,`

```
/// Column widths derived from content (the default, content-based sizing).
```

## L177-178 · `Fixed,`

```
/// CSS2 §17.5.2.1 fixed layout: widths come from the table/`<col>`/first-row
/// cell `width`s; content does not widen columns.
```

## L182 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// A `grid-template-columns` track size.
```

## L185 · `Auto,      // size to column content (max-content)`

```
// size to column content (max-content)
```

## L186 · `Fixed(f32), // px`

```
// px
```

## L188 · `Fr(f32), // fraction of leftover space`

```
// fraction of leftover space
```

## L191 · `pub const MAX_GRID_COLS: usize = 16;`

```
/// Max explicit grid columns we track (content grids rarely exceed this).
```

## L194 · `pub const GRID_AREAS_MAX: usize = 12;`

```
/// Max named grid areas per container (page shells rarely exceed this).
```

## L197-198 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// A `grid-template-areas` region: the area name (FNV-1a hash) and its half-open
/// cell rectangle `[c0,c1) × [r0,r1)` in the template grid.
```

## L212-213 · `pub fn area_hash(name: &str) -> u32 {`

```
/// FNV-1a hash of a grid-area name (0 = "none"). Both `grid-template-areas` and
/// `grid-area` values pass through `apply_one`'s lowercasing, so the hashes match.
```

## L227-232 · `pub fn counter_hash(name: &str) -> u32 {`

```
/// Hash a CSS counter name. CSS identifiers are technically case-sensitive, but
/// no page relies on two counters differing only by case, and the two parsers
/// that must agree — this one and the `content: counter(…)` side — reach the
/// name through different paths (`apply_one` already ASCII-lowercases its value,
/// `content` does not), so case-folding here keeps them consistent. Reuses the
/// grid-area FNV so `0` is never a valid hash.
```

## L237-239 · `fn parse_counter_ops(v: &str, out: &mut [(u32, i32); COUNTER_OPS_MAX], n: &mut u8, default: i32) {`

```
/// Parse a `counter-reset`/`counter-increment` value — a list of `<name>
/// [<integer>]?` pairs — into `out`/`n`. `default` is the value when a name has
/// no explicit integer (0 for reset, 1 for increment). `none` clears the list.
```

## L248 · `if name == "none" {`

```
// A stray keyword (`none`) among names is not a counter; skip it.
```

## L252 · `let val = match it.peek().and_then(|s| s.parse::<i32>().ok()) {`

```
// An optional integer follows the name; otherwise the default applies.
```

## L267 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// `justify-content` — main-axis distribution of leftover space.
```

## L278-282 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// `overflow-x` / `overflow-y`. Kept as the keyword rather than a pair of
/// booleans because two different questions are asked of it and they do not
/// agree: whether the box CLIPS its paint (`hidden`/`clip`), and whether it is
/// a scroll container (`hidden`/`scroll`/`auto` — `clip` is not one, which is
/// exactly what stops it zeroing a flex item's automatic minimum size).
```

## L293-295 · `pub fn clips(self) -> bool {`

```
/// Paint outside the padding box is cut away. `auto`/`scroll` deliberately
/// do NOT: without in-page scroll containers, clipping there would hide
/// content the user is meant to be able to reach.
```

## L300 · `pub fn scrolls(self) -> bool {`

```
/// A scroll container (css-overflow-3 §3.3).
```

## L306-310 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// `object-fit` (css-images-3 §5.5) — how a replaced element's own pixels are
/// scaled into the content box the layout gave it. Purely a paint decision:
/// the box keeps the size `width`/`height` resolved either way, only the
/// picture inside it moves. `Fill` is the initial value and is the stretch
/// every `<img>` got before this existed.
```

## L320-323 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Which of a box's three nested rectangles a background is measured against:
/// `background-clip` says where it is PAINTED, `background-origin` where the
/// image is positioned from. Defaults differ — clip is the border box, origin
/// the padding box — so one enum with two fields, not one shared setting.
```

## L332 · `pub fn shrink(self, st: &ComputedStyle, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {`

```
/// Shrink a BORDER box to this edge. Returns `(x, y, w, h)`.
```

## L354-357 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// `align-content` — how a multi-line flex container packs its LINES (and a
/// grid its row tracks) in whatever cross space is left over. The six
/// distributions are `Justify`'s; `stretch` belongs to this property alone and
/// is its initial value, which is why it is not folded into that enum.
```

## L369 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// `align-items` / `align-self` — cross-axis placement.
```

## L378 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// `flex-basis` — an item's main-size seed before grow/shrink.
```

## L381 · `Auto, // use the content's intrinsic main size`

```
// use the content's intrinsic main size
```

## L386-388 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// One edge of a box's border: its used width (px) and colour. A side paints
/// only when `width > 0` AND `color` is set. The four sides are independent
/// (`border-top`/`-right`/… may differ).
```

## L391-392 · `pub width: f32,`

```
/// The USED width — what layout and paint read. It is the specified width
/// only while a style is in effect, and 0 otherwise.
```

## L394-396 · `pub color: Option<Rgba>,`

```
/// `None` means `currentColor` — the initial value, and still unresolved.
/// `finish_borders` turns it into the element's own `color` once the whole
/// cascade has run, so a later `color` declaration still reaches it.
```

## L398-401 · `pub hidden: bool,`

```
/// `border-style: hidden`. Paints exactly like `none` on its own box, but
/// in a collapsed table it is not the same thing: `hidden` SUPPRESSES the
/// grid line it meets, beating every other border there (CSS2.1 §17.6.2
/// rule 1), while `none` is merely the weakest candidate.
```

## L403-405 · `pub spec_width: f32,`

```
/// The specified `border-width`, kept apart from the used one. The two
/// halves arrive in either order and neither implies the other: a width
/// with no style paints nothing, a style with no width is `medium`.
```

## L407 · `pub styled: bool,`

```
/// A `border-style` other than `none`/`hidden` is in effect.
```

## L409-411 · `pub see_through: bool,`

```
/// `border-color: transparent` — a VALUE, not an absence. The side keeps
/// its width and paints nothing, which differs from both a colour and from
/// leaving the property unset (that means `currentColor`).
```

## L413-417 · `pub specified: bool,`

```
/// A width or style declaration reached this side. `border: none` is a
/// DECLARATION, and it computes to the same used width as a side nobody
/// touched — only this bit tells them apart. It matters where the UA
/// supplies a frame of its own: a form control's, which the page then
/// suppresses (`paint_control`).
```

## L421-423 · `pub const EX_PER_EM: f32 = 0.55;`

```
/// Inter's x-height and "0" advance as a fraction of the em, measured at
/// size 100 (55.0 and 63.09). `parse_length` has no font to ask — it sees only
/// `Units` — so the metrics come here as constants instead of being guessed.
```

## L427 · `const BORDER_MEDIUM: f32 = 3.0;`

```
/// `border-width`'s initial value, `medium`.
```

## L437-439 · `fn sync(&mut self) {`

```
/// Recompute the used width after either half changed. `border-style`'s
/// initial value is `none`, and that forces the used width to 0 whatever
/// `border-width` says (CSS2.1 §8.5.3).
```

## L448-453 · `fn set_color(&mut self, tok: &str, theme: &Theme) -> bool {`

```
/// Apply one `border-color` token, reporting whether it was one. A page
/// that hides a button's frame writes `border-color: transparent`; treating
/// that as "no colour parsed" drops the declaration and leaves the frame
/// standing — which is how Wikipedia's icon buttons came out as empty
/// rectangles. `rgba(0,0,0,0)` says the same thing and must land here too:
/// it is how DuckDuckGo reserves the hover frame around every result.
```

## L464-467 · `Some(ColorVal::CurrentColor) => {`

```
// A side with no colour already MEANS `currentcolor` — that is
// its initial value, and `finish_borders` fills it in from the
// element's own `color` after the whole cascade has run. So the
// deferral this keyword needs is the state the field starts in.
```

## L477-478 · `fn copy_width(&mut self, from: &BorderSide) {`

```
/// Take only the WIDTH half from another side — `border-top-width:
/// inherit` must not drag the style or colour along with it.
```

## L485-486 · `fn copy_style(&mut self, from: &BorderSide) {`

```
/// Take only the STYLE half. `styled` decides the used width, so `sync`
/// has to run after it.
```

## L494-495 · `fn copy_color(&mut self, from: &BorderSide) {`

```
/// Take only the COLOUR half, `transparent` included — that is carried by
/// `see_through`, not by the colour being absent.
```

## L501-502 · `fn set_style(&mut self, tok: &str) {`

```
/// Apply one `border-style` token. An unknown one is invalid and leaves the
/// side alone.
```

## L520-522 · `fn finish_borders(s: &mut ComputedStyle) {`

```
/// Resolve every side's `currentColor` against the element's final `color`.
/// Runs after the whole cascade, because `border-style: solid; color: green`
/// and `color: green; border-style: solid` have to mean the same thing.
```

## L532-534 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// A CSS length keyword/value for the box model. `Auto` means "auto" (for
/// width/margins) or "none" (for max-width). `%` is relative to the containing
/// block's content width, resolved at layout time.
```

## L540-541 · `Calc { pct: f32, px: f32 },`

```
/// `calc()` in affine form `pct% of basis + px` — calc is linear in the
/// percentage basis, so any mix of `%`/px/em resolves to (pct, px).
```

## L543-546 · `Intrinsic(Intrinsic),`

```
/// `min-content` / `max-content` / `fit-content`: a size the CONTENT
/// decides, not the containing block. Which of the three it is only
/// matters on the inline axis; on the block axis all three are the
/// content height.
```

## L550 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Which intrinsic size a `Len::Intrinsic` asks for (css-sizing-3 §5).
```

## L553 · `Min,`

```
/// `min-content` — as narrow as the content allows.
```

## L555 · `Max,`

```
/// `max-content` — as wide as the content wants, no wrapping.
```

## L557 · `Fit,`

```
/// `fit-content` — max-content, clamped into the available space.
```

## L562-567 · `pub fn px(self, cb: f32) -> Option<f32> {`

```
/// Resolve to px against a containing-block width; `Auto` → `None`.
///
/// An intrinsic keyword is `None` here too: the containing block cannot
/// answer it, only the content can. Every caller that can measure content
/// asks `intrinsic()` first; the rest treat it as `auto`, which is the
/// right fallback because both mean "not a length the parent dictates".
```

## L577 · `pub fn intrinsic(self) -> Option<Intrinsic> {`

```
/// The intrinsic keyword, if this is one.
```

## L585-587 · `pub fn is_auto(self) -> bool {`

```
/// `auto` in the sense that matters to sizing: no length from the parent.
/// An intrinsic keyword is NOT auto for stretching — that is the whole
/// difference — so callers that stretch must ask `intrinsic()` too.
```

## L593-598 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// One axis of `background-position` / `mask-position`.
///
/// A percentage aligns the same fraction of the image with that fraction of
/// the positioning area (css-backgrounds-3 §3.6), so it cannot be resolved
/// until the image's size is known — which is at paint time, since an image
/// may arrive after layout.
```

## L602 · `Pct(f32),`

```
/// Fraction 0..1: `offset = (area - image) * f`.
```

## L606 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// `background-size` / `mask-size`.
```

## L609 · `Auto,`

```
/// Intrinsic size (css-backgrounds-3 §3.9).
```

## L613-614 · `Fixed(Option<Len>, Option<Len>),`

```
/// Explicit per axis; `None` on an axis means `auto` (keep the aspect
/// ratio against the other axis). Percentages are of the positioning area.
```

## L618 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// Ein Farbstopp eines Verlaufs.
```

## L622-629 · `pub pos: f32,`

```
/// Lage auf der Achse. `px` sagt, in welcher Einheit: als Anteil 0..1,
/// oder absolut in px. `NaN` heisst „nicht angegeben" — die Luecken
/// werden gleichmaessig gefuellt, wie es die Spezifikation vorschreibt.
///
/// Eine px-Lage KANN hier nicht in einen Anteil umgerechnet werden: sie
/// misst gegen die Verlaufsachse, und deren Laenge kennt erst der Kasten.
/// 14 von 86 gesetzten Stopps im Messkorpus sind px — sie als „nicht
/// angegeben" zu behandeln hiesse, sie still gleichmaessig zu verteilen.
```

## L632-634 · `pub cur: bool,`

```
/// `currentcolor`. Sie steht beim Parsen noch nicht fest — die Farbe des
/// Elements wird erst aufgeloest, wenn der Kasten gemalt wird. Kostet
/// nichts: das Byte liegt in der Auffuellung neben `px`.
```

## L641-652 · `pub const MAX_STOPS: usize = 6;`

```
/// Wie viele Farbstopps ein Verlauf tragen darf.
///
/// **Ausgezaehlt, nicht geraten**: ueber 23 echte Stilblaetter (Fritzbox +
/// der Messkorpus) haben 109 Verlaeufe zwei Stopps, 31 drei, 7 vier, zwei
/// sechs und zwei zehn — dazwischen liegt nichts. Sechs deckt also genau so
/// viel wie acht, und die beiden Ausreisser sind Tailwind-MASKEN, die wir
/// ohnehin nicht malen.
///
/// Der Platz ist der Grund fuer die Sparsamkeit: der Verlauf liegt in
/// `BgLayer`, `BgLayer` zweimal in `ComputedStyle` (Hintergrund und Maske),
/// und `ComputedStyle` ist die heisseste Struktur des Motors. Gemessen:
/// 1312 B ohne Verlaeufe, 1472 B mit sechs Stopps, 1520 B mit acht.
```

## L655 · `pub const CORNER_NONE: u8 = 0;`

```
/// `Gradient::corner`: keine Ecke, oder eine der vier.
```

## L662-669 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// Ein Farbverlauf als Hintergrund.
///
/// **Warum er IM Stil liegt und nicht in einer Tabelle daneben.** Ein
/// `url()`-Hintergrund traegt nur einen Schluessel, weil die Bytes anderswo
/// liegen; ein Verlauf hat keine Bytes. Ein Index in eine Blatt-Tabelle ginge
/// nicht: `var()` wird beim GEBRAUCH aufgeloest, der Text am Blatt ist also
/// ein anderer als der, den die Kaskade sieht — und genau so schreibt die
/// Fritzbox ihren Kopf (`linear-gradient(90deg, var(--blue-100), …)`).
```

## L673 · `pub n: u8,`

```
/// Anzahl gueltiger Stopps. 0 heisst: kein Verlauf.
```

## L676-678 · `pub circle: bool,`

```
/// Nur radial: `circle` statt der Vorgabe `ellipse`. Ein Kreis hat EINEN
/// Radius, eine Ellipse zwei — auf einem breiten Kasten sind das zwei
/// deutlich verschiedene Bilder. Das Byte liegt in der Auffuellung.
```

## L680-682 · `pub corner: u8,`

```
/// `to <ecke>`, wenn eine angegeben war — `CORNER_NONE` sonst. Der
/// Winkel dazu steht erst am Kasten fest; `angle` traegt bis dahin die
/// 45-Grad-Naeherung.
```

## L684-685 · `pub angle: f32,`

```
/// Grad im Uhrzeigersinn von „nach oben" — die Zaehlweise von CSS.
/// `to bottom` ist 180, `to right` ist 90.
```

## L703-708 · `pub fn angle_for(&self, w: f32, h: f32) -> f32 {`

```
/// Der Winkel der Achse an einem `w` x `h` grossen Kasten.
///
/// Fuer alles ausser einer Ecke ist das schlicht `angle`. Fuer eine Ecke
/// liegt die Achse so, dass ihre Senkrechte durch die Mitte die beiden
/// Nachbarecken trifft (css-images-3 §3.4.1) — das haengt am
/// Seitenverhaeltnis, nicht am Schluesselwort.
```

## L722-726 · `pub fn with_current(&self, color: Rgba) -> Gradient {`

```
/// `currentcolor`-Stopps mit der Farbe des Elements fuellen.
///
/// Erst hier, nicht beim Parsen: css-color-4 §6.2 loest `currentcolor`
/// zum Gebrauchswert auf, und beim Parsen der Deklaration kann `color`
/// noch gar nicht feststehen.
```

## L738-742 · `pub fn resolved(&self, line: f32) -> Gradient {`

```
/// Derselbe Verlauf, aber mit Stopps als reine Anteile 0..1 — px gegen
/// die Achsenlaenge `line` gerechnet, offene Lagen gefuellt.
///
/// Das geschieht beim MALEN, nicht beim Parsen: `line` haengt am Kasten,
/// und derselbe Stil malt zwei verschieden breite Kaesten.
```

## L756-760 · `pub fn at(&self, t: f32) -> Rgba {`

```
/// Die Farbe an der Stelle `t` der Achse (0..1 ausserhalb erlaubt).
///
/// Zwischen zwei Stopps wird MIT VORMULTIPLIZIERTEM Alpha gemischt —
/// `transparent` ist `rgba(0,0,0,0)`, und ein naiver Mittelwert zoege
/// einen Verlauf nach Schwarz statt ihn ausblenden zu lassen.
```

## L786 · `let f = if span > 1e-6 { (t - a.pos) / span } else { 1.0 };`

```
// Zwei Stopps auf derselben Lage sind eine harte Kante.
```

## L788-789 · `if a.color.a == 255 && b.color.a == 255 {`

```
// Sind beide Stopps deckend, ist das Vormultiplizieren ein Umweg mit
// drei Divisionen — und dieser Zweig laeuft je PIXEL.
```

## L802 · `fn mix_premul(a: Rgba, b: Rgba, f: f32) -> Rgba {`

```
/// Zwei Farben mischen, vormultipliziert (css-images-3 §3.4.2).
```

## L819-823 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// A `background-image` or `mask-image` layer.
///
/// `image` is a `url_key` into the stylesheet's URL table, not the string:
/// `ComputedStyle` is `Copy` and must stay that way (it is copied per element
/// and memoised), so it cannot hold an allocation.
```

## L827-828 · `pub gradient: Gradient,`

```
/// Ein Farbverlauf statt eines Bildes. `n == 0` heisst „keiner" — ein
/// `Option` waere hier nur ein Byte Verpackung mehr.
```

## L830 · `pub repeat: (bool, bool),`

```
/// (repeat-x, repeat-y).
```

## L846 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `position`.
```

## L856 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `float`.
```

## L864 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `clear`.
```

## L873-877 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// CSS 2.1 `clip` (§11.1.2). Applies only to absolutely-positioned boxes; the
/// four offsets are resolved px from the element's border-box top-left corner
/// (`top`/`bottom` from the top edge, `left`/`right` from the left edge). `None`
/// on a side = `auto` = that border edge. `Inherit` is a transient value that
/// `resolve` collapses to the parent's computed `clip`.
```

## L890-893 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// CSS `z-index` (CSS2.1 §9.9.1): `auto` or an integer stack level, valid only
/// on a positioned box (`position != static`). `Inherit` is a transient value
/// that `resolve` collapses to the parent's computed `z-index`, same pattern
/// as `Clip::Inherit`.
```

## L901-909 · `#[derive(Clone, Copy, Debug, PartialEq)]`

```
/// One `box-shadow` layer, outer only.
///
/// Real pages use two very different things under one name: a soft drop shadow
/// (`0 2px 8px rgba(...)`) and a **hairline rule** (`0 1px #c8ccd1`), which is a
/// zero-blur shadow standing in for a border the author did not want in the box
/// model. The second is a plain rectangle and is what shows up as a missing
/// separator; the first needs a blur kernel and looks fine while absent. So only
/// `blur == 0` is painted, and a blurred shadow keeps being skipped rather than
/// drawn as a hard slab.
```

## L916-918 · `pub color: Option<Rgba>,`

```
/// `None` = `currentColor`, resolved at PAINT time. Resolving it here would
/// take whatever `color` happened to be cascaded so far — and `box-shadow`
/// is routinely written before `color` in the same block.
```

## L923-927 · `pub fn paints(&self) -> bool {`

```
/// Whether we have a paint for this layer at all. The single source for it:
/// `paintable_shadow` picks the layer with it and `shadow_ops` draws the
/// layer with it, so neither can drift into painting what the other skipped.
/// Eine SCHARFE Schicht — auf echten Seiten meist ein Haarstrich, den der
/// Autor nicht im Kastenmodell haben wollte.
```

## L933-934 · `#[derive(Clone, Copy)]`

```
/// The subset of computed properties the renderer consumes. Split by CSS
/// inheritance: font/colour/`white-space` inherit; box/`display` do not.
```

## L937 · `pub font_px: f32,`

```
// — inherited —
```

## L939-942 · `pub em_base: f32,`

```
/// The *parent's* font-size — the base for resolving this element's own
/// `font-size` in `em`/`%`/`inherit` (CSS: font-size em/% is parent-
/// relative, NOT relative to the value a UA/earlier rule already set).
/// Recomputed per element in `inherit_reset`; never compounds.
```

## L944 · `pub rem_base: f32,`

```
/// The root element's computed `font-size` — the basis for `rem`.
```

## L946-949 · `pub vw: f32,`

```
/// The viewport, for `vw`/`vh`/`vmin`/`vmax`. Document-global like
/// `rem_base`, and carried the same way: seeded on the initial style and
/// copied down by `inherit_reset`, so every `s.units()` has it without
/// threading two more arguments through the cascade.
```

## L955-961 · `pub family: u32,`

```
/// Die ERSTE Familie aus `font-family`, als Streuwert.
///
/// **Nicht der Name, nur seine Zahl.** Ein `ComputedStyle` wird millionenfach
/// kopiert; eine Zeichenkette darin waere eine Allokation je Element. Der
/// Streuwert reicht: `Fonts` haelt die geladenen `@font-face`-Gesichter unter
/// derselben Zahl. 0 heisst „keine benannte Familie" (die eingebauten
/// Gesichter entscheiden dann wie bisher ueber `bold`/`italic`/`mono`).
```

## L963 · `pub pre: bool, // white-space: pre (no collapse, honor newlines)`

```
// white-space: pre (no collapse, honor newlines)
```

## L964-968 · `pub nowrap: bool,`

```
/// `white-space: nowrap` — whitespace still collapses, but the line never
/// breaks at one. Inherited. Real pages use it to keep a label, a
/// coordinate pair or a table header on one line; wrapping it anyway makes
/// the box a line taller and, under `position: absolute`, overlaps
/// whatever it was placed above.
```

## L970-972 · `pub hidden: bool,`

```
/// `visibility: hidden`/`collapse` — the box still lays out and still takes
/// its space, but paints nothing. Inherited, so a descendant can set
/// `visible` and reappear inside a hidden ancestor (CSS2.1 §11.2).
```

## L974-978 · `pub transparent: bool,`

```
/// The box (and its whole subtree) is fully transparent — `opacity: 0`.
/// Unlike `visibility` this cannot be undone further down: opacity groups
/// the subtree, so a descendant with `opacity: 1` is still invisible. It
/// stays HIT-TESTABLE, which is exactly what a checkbox-hack click overlay
/// (`position:absolute; width:100%; height:100%; opacity:0`) needs.
```

## L980-984 · `pub opacity: f32,`

```
/// `opacity`, 0..1. Below 1 it scales the alpha of every op the element
/// and its subtree emit (see `Ctx::apply_filter`) — an approximation of
/// the group compositing the spec asks for: two OVERLAPPING descendants
/// show through each other instead of being flattened first. Exact group
/// opacity needs an offscreen buffer per stacking context.
```

## L986-998 · `pub inline_fade: f32,`

```
/// Alpha, mit der der TEXT und der Kastenschmuck dieses Elements
/// vormultipliziert werden — aufgesammelt ueber die INLINE-Vorfahren.
///
/// Ein Block bekommt seinen `opacity`/`filter` nachtraeglich ueber seinen
/// Befehlsbereich gelegt (`Ctx::apply_filter`). Ein Inline-Kasten hat
/// keinen solchen Bereich: seine Laeufe werden erst beim Schliessen der
/// Zeile ausgegeben, lange nachdem das Element vorbei ist. Also traegt er
/// seine Deckung hier mit, und seine Nachfahren multiplizieren ihre dazu.
///
/// Auf allem, was einen eigenen Bereich HAT (Block, Inline-Block, Float,
/// ausser Fluss), steht sie wieder auf 1 — sonst wuerde dieselbe Deckung
/// zweimal angewandt. Der Preis dafuer ist benannt: ein BLOCK innerhalb
/// eines halbdurchsichtigen Inline-Kastens verliert dessen Deckung.
```

## L1000-1002 · `pub opacity_zero: bool,`

```
/// This element's OWN `opacity: 0`, before it is folded into `transparent`.
/// Kept apart so a later declaration in the same cascade can undo an
/// earlier one, while an ANCESTOR's transparency still can't be undone.
```

## L1006-1010 · `pub center_blocks: bool,`

```
/// `<center>` and `<div align=center>` centre BLOCK-level children too,
/// not just inline content — the behaviour browsers spell `text-align:
/// -moz-center`. Plain CSS `text-align: center` must NOT do this, which
/// is why it needs its own inherited flag rather than riding on the
/// alignment value. The `<center><table>` idiom depends on it entirely.
```

## L1014-1017 · `pub rtl: bool,`

```
/// `direction: rtl` — the inline base direction. This engine does no bidi
/// reordering (no RTL faces are embedded); what it does honour is the part
/// that governs layout of LTR content inside an RTL container: `start`/
/// `end` text alignment flip, so an unstyled RTL block right-aligns.
```

## L1020-1021 · `pub text_align_last: Option<TextAlign>,`

```
/// `text-align-last` — alignment of a block's LAST line. `None` = `auto`,
/// i.e. defer to `text-align`.
```

## L1023-1025 · `pub text_indent: Len,`

```
/// `text-indent` — how far the block's FIRST line box starts in from its
/// content edge. Inherited; a percentage resolves against the containing
/// block's width. Negative values hang the first line out to the left.
```

## L1027-1029 · `pub letter_spacing: f32,`

```
/// `letter-spacing` in px — extra advance after EVERY character of a run,
/// the last one included, which is what browsers measure an inline box as.
/// Inherited. `normal` is 0.
```

## L1031 · `pub word_spacing: f32,`

```
/// `word-spacing` in px — extra advance on every word separator. Inherited.
```

## L1033 · `pub display: Display,`

```
// — not inherited —
```

## L1035 · `pub width: Len,`

```
// — box model (block) —
```

## L1038 · `pub max_width: Len, // Auto = no maximum`

```
// Auto = no maximum
```

## L1041 · `pub max_height: Len, // Auto = no maximum`

```
// Auto = no maximum
```

## L1044-1047 · `pub margin_top_auto: bool,`

```
/// `margin-top: auto` / `margin-bottom: auto`. In normal flow a vertical
/// `auto` is zero (CSS2.1 §10.6.3), so the number above is enough there —
/// but a flex item's auto margin EATS the free space on its axis
/// (css-flexbox-1 §8.1), which a zero cannot express.
```

## L1052-1058 · `pub pct_pad: [f32; 4],`

```
/// A PERCENTAGE on `padding` (top, right, bottom, left) and on the vertical
/// `margin`s, kept unresolved until the containing block's width is known.
/// Both axes resolve against the WIDTH (CSS 2.1 §8.1, §8.3) — that is not a
/// typo in the spec: it is what makes `padding-top: 56.25%` an aspect ratio
/// rather than a height, which is how every responsive video embed on the
/// web reserves its box. `0.0` means "no percentage here"; a literal `0%`
/// resolves to the same zero either way, so it needs no separate marker.
```

## L1065 · `pub box_border: bool, // box-sizing: border-box`

```
// box-sizing: border-box
```

## L1066-1069 · `pub aspect_ratio: Option<f32>,`

```
/// `aspect-ratio` as width÷height. A box with one axis definite and the
/// other `auto` derives the auto one from this (css-sizing-4 §4). The ratio
/// governs the box `box-sizing` names — the content box by default, the
/// border box under `border-box`.
```

## L1071-1077 · `pub vh_seen: u8,`

```
/// Which of this element's winning declarations carried a viewport-HEIGHT
/// unit (`vh`/`vmin`/`vmax`), as a bitmask: bit 0 = a property that sizes
/// or positions the box outright, bit 1 = `max-height`, bit 2 =
/// `min-height`. The caps are kept apart because a cap that never BINDS
/// changes no geometry at all — Wikipedia's menus are all
/// `max-height: 75vh` and never reach it — so layout can flag those only
/// when they actually clamp. Per-element, so deliberately NOT inherited.
```

## L1079-1080 · `pub appearance_none: bool,`

```
/// `appearance: none` — the page opts this form control OUT of the UA
/// widget look (css-ui-4 §4) and draws the whole thing itself.
```

## L1082-1086 · `pub bfc_root: bool,`

```
/// `display: flow-root` — a block box whose ONLY difference from `block`
/// is that it establishes a block formatting context. A flag rather than a
/// `Display` variant precisely because that is the whole difference:
/// everything that matches on `display` would otherwise need an arm that
/// says "same as block".
```

## L1088 · `pub contain_size: bool, // contain: size/strict — content contributes no size`

```
// `contain: size`/`strict` — content contributes no size
```

## L1089 · `pub contain_intrinsic: Option<(f32, f32)>, // contain-intrinsic-size (w, h) px`

```
// `contain-intrinsic-size` (w, h) px
```

## L1090 · `pub bg: Option<Rgba>, // background-color (None = transparent)`

```
// background-color (None = transparent)
```

## L1091-1097 · `pub bg_set: bool,`

```
/// Hat die SEITE einen Hintergrund gesetzt — auch `transparent`?
///
/// `bg: None` heisst „durchsichtig" und sagt nicht, wer das entschieden
/// hat. Fuer ein Steuerelement ist das der Unterschied zwischen „male
/// deine UA-Flaeche" und „die Seite malt selbst": Bootstrap gibt
/// `.btn-outline-*` ein `--bs-btn-bg: transparent`, und ohne diese Fahne
/// bekam ein Umriss-Knopf trotzdem die graue Flaeche des Themas.
```

## L1099 · `pub bg_layer: BgLayer,`

```
/// `background-image` + its placement properties.
```

## L1101-1103 · `pub bg_clip: BoxEdge,`

```
/// `background-clip` — which box the background (colour AND image) is
/// painted inside. A page uses it to keep a background off a translucent
/// or dashed border, which is exactly where the difference shows.
```

## L1105-1107 · `pub bg_origin: BoxEdge,`

```
/// `background-origin` — which box `background-position` and a percentage
/// `background-size` resolve against. The padding box by default, so a
/// bordered box centres its image inside the border, not across it.
```

## L1109-1112 · `pub mask_layer: BgLayer,`

```
/// `mask-image` + its placement. A mask does not paint the image: it
/// stencils the element's own `background-color` through the image's alpha
/// — which is how icon systems (MediaWiki's Vector, Codex) draw a
/// recolourable icon from one SVG.
```

## L1118-1119 · `pub outline: BorderSide,`

```
/// `outline` (css-ui-4 §3). A `BorderSide` because it has the same three
/// parts — but it never enters the box model: no layout code may read it.
```

## L1122-1130 · `pub outline_set: bool,`

```
/// Hat die SEITE ueber den Umriss etwas gesagt?
///
/// **Der Unterschied zwischen „kein Umriss" und „ich will keinen" ist der
/// ganze Punkt.** Ein Browser malt den Fokusring als `outline` und laesst
/// die Seite ihn mit `outline: none` abschalten. beak kennt `:focus` in
/// der Kaskade nicht, kann die UA-Regel also nicht dort hinschreiben —
/// aber es kann merken, ob die Seite die Eigenschaft ueberhaupt in die
/// Hand genommen hat. Hat sie das, gilt ihr Wort; hat sie es nicht, malt
/// beak seinen eigenen Ring.
```

## L1132-1139 · `pub accent: Option<Rgba>,`

```
/// `accent-color` (css-ui-4 §5.1) — die Farbe, mit der ein Kaestchen, ein
/// Radioknopf, ein Schieber und ein Fortschrittsbalken gemalt werden.
///
/// **VERERBT**, und `None` heisst `auto`: dann malt das Thema. Eine Seite,
/// die ihre Kaestchen in ihrer eigenen Akzentfarbe will, schreibt genau
/// diese eine Zeile — `sandbox.nopeek.ch` dreimal
/// (`.network-checkbox input { accent-color: var(--accent) }`), und ohne
/// sie bekam sie unsere Themenfarbe statt ihrer.
```

## L1141 · `pub position: Position,`

```
// — positioning —
```

## L1149 · `pub is_rule: bool, // <hr> — painted as a divider`

```
// <hr> — painted as a divider
```

## L1150 · `pub is_break: bool, // <br> — forced line break in inline flow`

```
// <br> — forced line break in inline flow
```

## L1151-1156 · `pub is_summary: bool,`

```
/// This element is a `<details>`'s disclosure control — its first
/// `<summary>` child. Element identity, not a box property, so it is NOT
/// settable from CSS: a page that writes `summary { list-style: none }`
/// (most of them do) removes the triangle, and the box must stay clickable
/// anyway. `record_inspect` turns this into the toggle rect the shell
/// hit-tests.
```

## L1158-1159 · `pub valign: VAlign,`

```
/// `vertical-align` — not inherited. On a table cell it aligns the content
/// box in the row; on an inline-level box it shifts the box on the line.
```

## L1161-1164 · `pub deco: u8,`

```
/// `text-decoration-line` as `DECO_*` bits. CSS propagates a decoration to
/// in-flow descendants rather than inheriting it (css-text-decor-3 §1.2);
/// we inherit, which paints the same pixels for every construct we have —
/// the difference only shows where a descendant tries to *cancel* one.
```

## L1166-1167 · `pub deco_color: Option<Rgba>,`

```
/// `text-decoration-color`. `None` = `currentColor`, which is the initial
/// value and what the line used to be painted in unconditionally.
```

## L1169 · `pub flex_row: bool, // flex-direction: row (true) vs column (false)`

```
// — flex container —
```

## L1170 · `pub flex_row: bool, // flex-direction: row (true) vs column (false)`

```
// flex-direction: row (true) vs column (false)
```

## L1172 · `pub flex_balance: bool, // flex-wrap: balance (css-flexbox-2 line balancing)`

```
// flex-wrap: balance (css-flexbox-2 line balancing)
```

## L1175-1176 · `pub align_content: ContentAlign,`

```
/// `align-content` — line packing on the cross axis. Container-level, so a
/// flex ITEM never reads it.
```

## L1178 · `pub flex_grow: f32,`

```
// — flex item —
```

## L1184 · `pub grid_ncols: u8,`

```
// — grid container —
```

## L1190-1193 · `pub grid_auto_cols: GridTrack,`

```
/// `grid-auto-columns` — the size of an IMPLICIT column, one past the last
/// `grid-template-columns` track. Without it every implicit column fell back
/// to the `auto` default and a `grid-auto-columns: 100px` row came out
/// content-sized.
```

## L1195-1197 · `pub grid_col_gap: Len,`

```
/// `column-gap` / `row-gap`. A `Len`, because a percentage gap resolves
/// against the container's own content box on that axis — and dropping the
/// percentage silently made `gap: 10%` a gap of nothing.
```

## L1201 · `pub grid_areas: [GridArea; GRID_AREAS_MAX],`

```
// `grid-template-areas` — named regions (container), 0-count = none.
```

## L1204-1206 · `pub grid_col_fill: u8,`

```
// `repeat(auto-fill/auto-fit, …)` in the columns: 0 = none, else the stored
// one-copy pattern spans `grid_col_fill_start .. +len` and is expanded to fill
// the container width at layout time.
```

## L1210 · `pub grid_col_span: u16,`

```
// — grid item —
```

## L1212 · `pub grid_col_start: i16, // 0 = auto placement`

```
// 0 = auto placement
```

## L1213 · `pub grid_row_start: i16, // 0 = auto placement`

```
// 0 = auto placement
```

## L1215 · `pub grid_area: u32, // grid-area: <name> (FNV-1a hash, 0 = none)`

```
// `grid-area: <name>` (FNV-1a hash, 0 = none)
```

## L1217 · `pub float: FloatKind,`

```
// — float —
```

## L1220 · `pub clip: Clip,`

```
// — clip (abs-positioned only) —
```

## L1222 · `pub table_layout: TableLayout,`

```
// — table —
```

## L1224-1227 · `pub border_spacing: (f32, f32),`

```
/// `border-spacing` (horizontal, vertical) in px — the gap the separated
/// border model leaves between adjacent cell borders, and between the
/// table's own padding edge and the outermost cells (CSS2.1 §17.6.1).
/// Inherited, so it reaches cells from the table without a walk.
```

## L1229-1230 · `pub border_collapse: bool,`

```
/// `border-collapse: collapse` — cell borders merge with their neighbours'
/// and with the table's, and `border-spacing` no longer applies.
```

## L1232-1238 · `pub overflow_x: Overflow,`

```
/// `overflow-x`/`overflow-y` are `hidden`/`clip` — the box paints nothing
/// of its content past its padding box on that axis. `auto`/`scroll`
/// deliberately do NOT set these: without in-page scroll containers,
/// clipping there would hide content the user is meant to be able to reach.
/// Two axes, because a page that scrolls a panel vertically and forbids
/// horizontal overflow writes exactly `overflow-x: hidden; overflow-y:
/// auto`, and a single flag can only get one of those two right.
```

## L1241-1244 · `pub ellipsis: bool,`

```
/// `text-overflow: ellipsis` — a line the box clips on the inline axis
/// ends in `…` instead of being cut mid-glyph. Not inherited (css-ui-4
/// §5.2); it is read on the box that does the clipping, and applies to
/// every text run inside it.
```

## L1246-1247 · `pub object_fit: ObjectFit,`

```
/// `object-fit` — how a replaced element's pixels fill the content box.
/// Not inherited.
```

## L1249-1255 · `pub bg_cc: bool,`

```
/// `background-color` was declared as `currentcolor` (or as a relative
/// colour over it). Kept as a FLAG beside the resolved colour, not folded
/// into it, because css-color-4 §6.2 resolves the keyword at used-value
/// time: a child that writes `background-color: inherit` inherits the
/// unresolved value and resolves it against ITS OWN `color`. Every other
/// colour field already defers by having `None` mean `currentcolor` —
/// `bg`'s `None` means transparent, so this one needs the flag.
```

## L1257-1260 · `pub filter: Option<crate::color::ColorFilter>,`

```
/// `filter`, as the one colour transform the whole chain composes to.
/// Not inherited, but it does apply to the element's whole SUBTREE — which
/// layout does by walking the op range the box produced, not by passing it
/// down the cascade (a descendant must not be able to cancel it).
```

## L1262-1264 · `pub break_word: bool,`

```
/// `overflow-wrap`/`word-wrap: break-word` or `word-break: break-all`/
/// `break-word` — a word longer than its line may be split mid-word rather
/// than overflowing the box. Inherited, like both source properties.
```

## L1266-1268 · `pub radius: [Len; 4],`

```
/// `border-radius`, `[tl, tr, br, bl]` (CSS corner order). Circular — CSS
/// allows an ellipse per corner (`r1 / r2`), we keep the horizontal radius.
/// Percentages resolve against the border-box width at paint time.
```

## L1270-1271 · `pub shadow: Option<BoxShadow>,`

```
/// `box-shadow`: the first OUTER, zero-blur layer of the list — see
/// `paintable_shadow` for why the first *paintable* one and not the first.
```

## L1273-1275 · `pub shadow_soft: Option<BoxShadow>,`

```
/// Die erste WEICHE aeussere Schicht. Getrennt vom scharfen Platz, weil
/// beide gleichzeitig sichtbar sein koennen — und der weiche liegt
/// hinter dem scharfen.
```

## L1277-1287 · `pub shadow_inset: Option<BoxShadow>,`

```
/// Die erste INNERE Schicht ohne Weichzeichnung.
///
/// Sieht nach einer Randerscheinung aus und ist die Art, wie Bootstrap
/// 5.3 seine Tabellen streift:
///
///     box-shadow: inset 0 0 0 9999px var(--bs-table-bg-type)
///
/// Ein innerer Schatten mit 9999 px Ausdehnung IST eine Fuellung ueber
/// dem Hintergrund — und weil er ueber dem Hintergrund liegt, faerbt er
/// die Zelle, ohne deren eigenes `background-color` zu ersetzen. Genau
/// dafuer haben sie ihn gewaehlt.
```

## L1289-1293 · `pub translate: Option<(Len, Len)>,`

```
/// `transform: translate(...)` as a paint-time offset, in px and in
/// PERCENT of the box's own size (`Len::Pct`) — the `translate(-50%,-50%)`
/// centring idiom needs the latter. Only translation: rotation and scale
/// would need a transformed raster path, and every other transform value
/// leaves this `None` rather than being approximated.
```

## L1295-1297 · `pub caption_bottom: bool,`

```
/// `caption-side: bottom` — the caption renders below the table grid
/// instead of above it. Inherited (CSS2.1 §17.4.1), so it can be set on
/// either the `<table>` or the `<caption>`.
```

## L1299-1300 · `pub empty_cells_hide: bool,`

```
/// `empty-cells: hide` — a cell with no in-flow content paints neither
/// border nor background in the separated model (CSS2.1 §17.6.1.1).
```

## L1302-1306 · `pub attr_cell_border: Option<f32>,`

```
/// `<table border>` / `<table cellpadding>`: HTML presentational hints that
/// style the table's CELLS, not the table (HTML §15.3.8). The cells are
/// several levels down (`tr`, row groups), so they ride down as inherited
/// state instead of needing an ancestor-attribute lookup. `None` = the
/// attribute is absent.
```

## L1309-1311 · `pub counter_reset: [(u32, i32); COUNTER_OPS_MAX],`

```
// — CSS counters (css-lists-3 §4) — `(name_hash, value)` pairs. Names are
// case-folded FNV-1a hashes (like `grid_area`) so `ComputedStyle` stays
// `Copy` — no `Vec`. Not inherited. `_n` is how many of the slots are used.
```

## L1318-1320 · `pub const COUNTER_OPS_MAX: usize = 4;`

```
/// Max named counters one `counter-reset`/`counter-increment` declaration can
/// carry. Real pages list one or two; extras are dropped (keeps the array
/// `Copy`, no heap).
```

## L1324-1325 · `pub fn overflow_clip(&self) -> bool {`

```
/// Clips on BOTH axes — what a baseline or a formatting-context decision
/// asks about, as opposed to which edge a paint is cut against.
```

## L1330 · `pub fn border_x(&self) -> f32 {`

```
/// Total horizontal border (left + right) contribution to the box.
```

## L1334 · `pub fn border_y(&self) -> f32 {`

```
/// Total vertical border (top + bottom) contribution to the box.
```

## L1339 · `pub fn root(theme: &Theme) -> ComputedStyle {`

```
/// The initial style for the document root, seeded from the theme.
```

## L1345-1347 · `vw: 800.0,`

```
// Overwritten by `layout()` with the real viewport. The default is
// the reftest canvas, so a bare `ComputedStyle::root()` in a unit
// test still resolves `vw`/`vh` to something meaningful.
```

## L1488-1491 · `fn parse_radius_shorthand(v: &str, u: Units) -> Option<[Len; 4]> {`

```
/// `border-radius`: 1-4 lengths in the usual corner shorthand order, with an
/// optional `/ <1-4 lengths>` vertical set that we drop (we draw circular
/// corners). All-or-nothing: one unparseable component leaves the property
/// alone rather than applying a half-read shape.
```

## L1514-1515 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// `vertical-align`. Lengths and percentages are not represented — they fall
/// back to `Baseline` rather than being mis-placed.
```

## L1542 · `pub const DECO_UNDERLINE: u8 = 1;`

```
/// `ComputedStyle::deco` bits (`text-decoration-line`).
```

## L1547-1549 · `fn parse_deco(v: &str) -> u8 {`

```
/// `text-decoration` / `text-decoration-line`: keep the line keywords, ignore
/// the colour and style components of the shorthand (we draw a solid line in
/// the text's own colour).
```

## L1563-1568 · `#[derive(Clone, Copy, Debug)]`

```
/// The bases a length may need that are not the containing block: `em` (the
/// element's own font-size, or its inherited one while `font-size` itself is
/// being resolved), `rem` (the ROOT element's computed font-size) and the
/// viewport for `vw`/`vh`/`vmin`/`vmax`. `em` and `rem` differ the moment a
/// document sets `html { font-size: … }` — the `62.5%` "1rem = 10px" idiom is
/// everywhere, and treating `rem` as `em` scales such a page by 1.6x.
```

## L1573 · `pub vw: f32,`

```
/// Viewport width in px — the basis for `vw`, and half of `vmin`/`vmax`.
```

## L1575 · `pub vh: f32,`

```
/// Viewport height in px — the basis for `vh`, and the other half.
```

## L1579-1583 · `fn inherit_reset(parent: &ComputedStyle) -> ComputedStyle {`

```
/// The starting point for any freshly-resolved style: the inherited slice
/// copied from `parent`, non-inherited properties reset to their CSS initial
/// value. Shared by `resolve()` (a real element) and `resolve_pseudo()` (a
/// `::before`/`::after` generated box, which inherits from its originating
/// element the same way a child would).
```

## L1588 · `rem_base: parent.rem_base,`

```
// `rem` is root-relative: inherited untouched, never reset per element.
```

## L1590 · `vw: parent.vw,`

```
// Document-global, same as `rem_base`.
```

## L1631 · `display: Display::Inline, // CSS initial display is inline`

```
// CSS initial `display` is inline
```

## L1667 · `outline: BorderSide::default(),`

```
// Not inherited: an outline belongs to the element that asked for it.
```

## L1722 · `counter_reset: [(0, 0); COUNTER_OPS_MAX],`

```
// Counters are not inherited: reset to empty on every element.
```

## L1730-1735 · `pub fn anon_inherit(parent: &ComputedStyle, display: Display) -> ComputedStyle {`

```
/// The style for an anonymous box (CSS2.1 §17.2.1): inherited properties
/// (color/font/…) come from `parent` exactly as for a real child, every
/// non-inherited property is the CSS initial value (`inherit_reset`), and
/// `display` is set to whatever box the layout algorithm needs to generate
/// (`Table`/`TableRow`/`TableCell`/…) — an anonymous box has no source
/// element, so nothing else can set it.
```

## L1742-1745 · `pub fn resolve(`

```
/// Resolve an element's computed style by the cascade: inherit from `parent`,
/// apply the UA rule for its tag, then matching author `<style>` rules (by
/// specificity + order), then any inline `style="…"` (highest). `ancestors` is
/// the root→…→parent chain, for descendant/child selector matching.
```

## L1761-1767 · `#[allow(clippy::too_many_arguments)]`

```
/// Wie [`resolve`], aber mit den Custom Properties des ELTERNTEILS — und es
/// gibt die eigenen zurueck.
///
/// `own` bleibt `None`, wenn das Element keine einzige Custom Property setzt.
/// Das ist der Normalfall und der Grund fuer die Form: Bootstraps `:root`
/// traegt ueber 200 Namen, und die je Element zu KOPIEREN waere teurer als
/// die ganze Kaskade. Wer nichts setzt, teilt die Karte des Elternteils.
```

## L1781-1783 · `let el = subject.el;`

```
// The SUBJECT arrives as an `ElemInfo`, not a bare `Element`: it carries the
// pointer state, and a caller that built it by hand would silently cascade
// `:hover` as false. The compiler now asks every call site for it.
```

## L1787-1789 · `if el.tag == "a" && el.attr("href").is_some() {`

```
// `:any-link { text-decoration: underline }` (HTML rendering §15.3.9). It
// needs the `href`, which `ua_rule` doesn't see — a bare `<a name=…>`
// anchor is not a link and is not underlined.
```

## L1793-1795 · `if el.tag == "dialog" && el.attr("open").is_none() {`

```
// A `<dialog>` without `open` is not rendered (HTML §4.11.4). This IS an
// ordinary UA-sheet rule — it sits before the author cascade, so a page
// that shows its own dialog with CSS still can.
```

## L1799-1807 · `if el.tag == "summary"`

```
// `<details>`/`<summary>` (HTML §4.11.1). The FIRST `<summary>` child is
// the disclosure control: it renders whatever the state, and carries the
// marker. A `<summary>` anywhere else is a plain block, which is why this
// cannot live in `ua_rule` — that only sees the tag.
//
// The marker is set here, before the author cascade, so `summary {
// list-style: none }` (which most pages write) removes it. `is_summary` is
// NOT: the box has to stay clickable even with the triangle gone, or the
// reader can never open the section.
```

## L1816-1819 · `s.pad_left = 16.0;`

```
// Room for the marker. A list marker is painted OUTSIDE the content
// edge (there is no `list-style-position` yet), and a `<summary>` has
// no `<ul>` around it to have paid for that space — the triangle would
// land in the margin, and at the left edge of the page outside it.
```

## L1823-1829 · `if matches!(`

```
// A button-like control is `box-sizing: border-box` in the UA sheet (HTML
// rendering §15.5.1) — unlike a text field, which stays content-box. It is
// what pages build on: Google puts a `height:30px` button inside a
// `height:30px` bordered wrapper and expects it to fit exactly. Read as
// content-box the button came out 8px taller than its own frame and hung
// out the bottom. `ua_rule` can't do this — it only sees the tag, and
// `<input>` is a button or a text field depending on its `type`.
```

## L1836-1839 · `s.text_align = crate::style::TextAlign::Center;`

```
// …and `text-align: center` (HTML rendering §15.5.1). It is the button
// sheet's own rule, and with the children laid out it is what actually
// centres the label — the painter used to do it by hand, which no
// child box could inherit.
```

## L1843-1844 · `match el.attr("dir") {`

```
// HTML's `dir` attribute is a presentational hint for `direction`: it sits
// between the UA sheet and the author cascade, so author CSS still wins.
```

## L1850-1854 · `if el.tag == "table" {`

```
// `<table>`'s presentational attributes (HTML §15.3.8). `cellspacing` is
// the one the reftest corpus leans on: it writes `cellspacing="0"`, and
// without this the UA's 2px default silently applies where the page asked
// for none. `border`/`cellpadding` style the cells, so they ride down as
// inherited state (see `attr_cell_border`).
```

## L1874-1878 · `if let Some(c) = el.attr("bgcolor").and_then(|v| parse_color(v.trim(), theme)) {`

```
// `bgcolor` is a presentational hint for `background-color` (HTML §15.3.3),
// the same family as `<table border>`/`cellpadding` above, and it sits
// between the UA sheet and the author cascade so author CSS still wins.
// Old table-built pages carry their whole colour scheme in it — Hacker
// News' orange masthead is a `bgcolor` on a `<td>`.
```

## L1883-1897 · `if matches!(`

```
// `width`/`height` as presentational hints (HTML Rendering §15.3.5-6).
// Table-built pages do their centring with spacer cells — Google's home
// page is `<td width="25%">&nbsp;</td>` either side of the search box —
// and an ignored attribute collapses the spacer to nothing, which slams
// the content against the left edge. The value is a "dimension": a bare
// number is pixels, a trailing `%` a percentage.
//
// **`<img>` stand hier NICHT, mit der Begruendung, `img_box` lese die
// Attribute ohnehin — und das war der Fehler.** `img_box` liest sie erst,
// wenn der Kasten schon gelegt wird; bis dahin sagt die Kaskade `auto`,
// und jeder, der VORHER fragt, bekommt die falsche Antwort: ein
// `<img width=30 height=30>` in einer streckenden Flexzeile kam 30x60
// heraus (Chromium 30x30), weil seine Quergroesse als `auto` galt. Doppelt
// angewandt wird nichts — `img_box` nimmt `css(st.width)` ZUERST und faellt
// nur ohne sie auf das Attribut zurueck, und das ist derselbe Wert.
```

## L1910-1919 · `if matches!(`

```
// `align` is a presentational hint for `text-align` (HTML Rendering
// §15.3.3). It is inherited, so a cell's `align="center"` centres
// everything inside it — which is the other half of how table-built pages
// centre: `<td width="25%">` spacers place the cell, `align="center"`
// places the content INSIDE it. With only the first, Google's search box
// sat at the left edge of a correctly-centred cell.
//
// `<table align>` is deliberately absent: there it means float/auto
// margins, not text alignment, and treating it as this would centre a
// table's text instead of the table.
```

## L1928-1929 · `s.center_blocks = el.tag == "div";`

```
// `<div align=center>` is the other spelling of `<center>` and
// gets the same block-centring; a cell's `align` does not.
```

## L1939-1942 · `let inline = el.attr("style");`

```
// Author cascade WITH `!important` (CSS Cascade 4 §6.3): two passes. Normal
// declarations first (UA < author-normal < inline-normal), then `!important`
// on top (author-important < inline-important) — so an `!important` decl
// wins its property regardless of specificity/order.
```

## L1947-1954 · `if ancestors.is_empty() {`

```
// Pass 1 — normal <style> declarations, low→high layer/specificity.
// `revert-layer` needs to know which layer a declaration came from, so
// it is resolved before anything is applied — and only when the sheet
// actually contains one, which no page but a cascade reftest does.
// Die mit `@property` angemeldeten Anfangswerte, EINMAL an der
// Wurzel: sie stehen unter allem, was eine Regel setzt, und vererben
// sich von dort nach unten. Ohne sie bleibt bei Tailwind jedes
// `var(--tw-…)` unaufgeloest und die Deklaration faellt weg.
```

## L1961-1966 · `for (_, _, _, _, _, customs, _) in &matched {`

```
// ── Custom Properties zuerst ──────────────────────────────────
//
// Vor jeder anderen Deklaration, denn jede andere darf sie lesen. Und
// in derselben Kaskadenordnung: Ebene, Spezifitaet, Reihenfolge —
// eine Regel, die dieses Element nicht trifft, steht hier gar nicht
// erst in der Liste. Genau das war der Fehler des Textlaufs.
```

## L2008-2010 · `matched.sort_by_key(|(layer, spec, order, _, _, _, _)| (crate::css::imp_rank(*layer), *spec, *order));`

```
// Pass 2 — `!important`, where the layer axis reverses (css-cascade-5
// §6.4.4): the FIRST layer wins, and an unlayered important loses to
// every layered one. Specificity and order keep their direction.
```

## L2033-2034 · `if matches!(s.clip, Clip::Inherit) {`

```
// `clip: inherit` takes the parent's computed value (clip is not inherited
// by default, so this is resolved here rather than in the initial slice).
```

## L2038 · `if matches!(s.z_index, ZIndex::Inherit) {`

```
// `z-index: inherit`, same pattern (z-index is not inherited by default).
```

## L2042-2051 · `if !s.is_summary`

```
// A CLOSED `<details>` renders only its disclosure control; the rest is
// skipped (HTML §4.11.1). This runs AFTER the author cascade on purpose:
// a browser hides the skipped contents through the shadow tree, where no
// author rule can reach them, so `details:not([open]) > div { display:
// block }` must not reveal them either.
//
// Element children only. A bare text node directly inside `<details>` is
// not covered — measured at 0 of 446 in the corpus (see
// `docs/plan/HTML_GAP_2026_08.md`), and covering it would mean a guard at
// each of the eleven places layout turns a `Node::Text` into a run.
```

## L2059-2062 · `if el.tag == "html" || el.tag == "body" {`

```
// `overflow` on the root element — and on `<body>` while the root keeps
// `visible` — propagates to the VIEWPORT, and the element's own used value
// becomes `visible` (css-overflow-3 §3.3). So the box itself neither clips
// nor establishes a formatting context.
```

## L2067-2075 · `let out_of_flow =`

```
// A float or an out-of-flow box is blockified (css-display-3 §2.7): it
// never joins a line box, so `inline`/`inline-block` there is just a block.
// `inline` matters for generated content — a page underlines its active tab
// with `a::after { position: absolute; … }` and states no display at all,
// relying on exactly this rule to give it a box.
// The internal table displays blockify too, and that is the whole of what
// `top`/`left` "do not apply" to them means: out of flow, the box is no
// longer a row or a cell, so the offsets it was given are an ordinary
// absolutely positioned block's.
```

## L2095-2097 · `if matches!(`

```
// `margin` does not apply to a box with an internal table display
// (CSS2.1 §8.3): the border model and `border-spacing` decide the distance
// between rows and cells, and a margin there has nothing to move.
```

## L2116-2124 · `let is_cell = matches!(s.display, Display::TableCell) || el.tag == "td" || el.tag == "th";`

```
// `vertical-align` applies to inline-level boxes and table cells only
// (CSS2.1 §10.8.1). An out-of-flow or block-level box is never aligned in
// a line box, and leaving the value on it would ride down into the text
// runs the box creates and shift its whole content — which is exactly what
// `vertical-align-sub-001` catches (two absolutely positioned spans that
// must coincide).
// A `<td>`/`<th>` carries `display: block` from the UA sheet — the table
// machinery recognises cells by tag/role, not by display — so the tag has
// to be part of the test.
```

## L2133-2134 · `s.transparent |= s.opacity_zero;`

```
// Opacity groups the subtree: a transparent ancestor wins over anything
// this element declares, but within this element the cascade decides.
```

## L2136-2138 · `let own_alpha = s.opacity * s.filter.map_or(1.0, |f| f.a);`

```
// Die Deckung, die dieses Element seinen Laeufen mitgibt — siehe
// `inline_fade`. Nur ein reiner Inline-Kasten sammelt; alles mit einem
// eigenen Befehlsbereich faengt wieder bei 1 an.
```

## L2148-2150 · `if s.bg_cc {`

```
// `currentcolor` is a used-value keyword: it reads the `color` this element
// ended up with, whatever order the declarations came in and whether the
// value arrived through `inherit`.
```

## L2154-2162 · `if s.float != FloatKind::None || matches!(s.position, Position::Absolute | Position::Fixed) {`

```
// **Was schwebt oder absolut steht, ist block-artig** (css-display-3 §2.7).
// Ohne die Regel blieb ein `display: inline-flex` mit `float: right` ein
// ATOMARER INLINE: er stand auf der Zeile statt zu fliessen. Auf DDGs
// Wissenskasten war das der „Directions"-Knopf, der links vor dem Titel
// klebte, statt rechts neben ihm zu stehen.
//
// `getComputedStyle` rechnete dieselbe Regel schon — aber NUR fuer
// Flexkinder und nur fuer die Antwort, nicht fuers Layout. Jetzt eine
// Funktion fuer beide ([[feedback_a_copy_is_a_second_semantics_waiting]]).
```

## L2171-2175 · `pub fn blockify(d: Display) -> Display {`

```
/// Die block-artige Entsprechung eines `display` (css-display-3 §2.7).
///
/// Gilt fuer alles, was aus dem Fluss faellt — schwebend, absolut, fest — und
/// fuer ein Flex- oder Rasterkind. `list-item`, `block`, `flex` und `grid`
/// bleiben, wie sie sind; `none` und `contents` erzeugen gar keinen Kasten.
```

## L2186-2198 · `fn unbox_contents(tag: &str, s: &mut ComputedStyle) {`

```
/// `display: contents` — the element generates no box (css-display-3 §3.1).
///
/// Everything that only describes a box has nothing left to describe, and
/// `inherit_reset` is exactly that split already: what it copies from its
/// argument is the INHERITED half of the style, what it writes literally is
/// the initial value of the non-inherited half. Applying it to the element's
/// own computed style therefore keeps `color`/`font`/`text-align` — which the
/// children must still inherit through it — and drops the margins, padding,
/// borders, background, size, `position`, `float` and `overflow` in one go,
/// with no list of properties to keep in step with the struct.
///
/// A replaced or void element has no children to put in its place, so there
/// `display: contents` computes to `none` instead (css-display-4 §3.3).
```

## L2211-2212 · `let (link, rule, brk, summ) = (s.is_link, s.is_rule, s.is_break, s.is_summary);`

```
// Element identity and the counter mechanism are not box properties: a
// counter is defined on the element, and `<a>` is still a link.
```

## L2228-2237 · `#[allow(clippy::too_many_arguments)]`

```
/// Resolve `el`'s `::before`/`::after` generated box: the winning `content`
/// declaration (by the same specificity/order cascade as any other property)
/// plus the pseudo-element's own computed style. Returns `None` when there is
/// no matching rule, `content` is `none`/`normal`/unparseable (`attr()`,
/// `counter()`, `open-quote`, `url()`, … are out of scope — docs/spec/CONFORMANCE.md's
/// forward-compatible rule: produce nothing rather than mis-render), or the
/// pseudo box itself computes to `display: none`.
///
/// `own` is `el`'s OWN already-resolved computed style — the pseudo box
/// inherits from it exactly as a real child element would.
```

## L2258-2262 · `let mut content_vals: Vec<&str> = Vec::new();`

```
// The `content` declarations in cascade order (later overrides earlier). An
// INVALID one is dropped at parse time (CSS Syntax 3 §4), so the winner is
// the LAST one that parses — not simply the last one. The template may
// reference counters (`counter()`/`counters()`), resolved later against the
// layout-time counter stack; a plain string is a single `Text` piece.
```

## L2283-2296 · `if matches!(s.display, Display::Inline | Display::InlineBlock)`

```
// Layout only knows how to place the pseudo box as an anonymous INLINE
// text run (see `layout.rs`'s `pseudo()`); `display: none` produces no
// box. Any other display (`block`, `list-item`, …) is a box shape we
// don't lay out here — docs/spec/CONFORMANCE.md's forward-compatible rule: produce
// nothing rather than render it wrong. An explicit `width`/`height` is
// the same story a level down: generated content is emitted as a plain
// text run, so a sized spacer (the common `content: "…"; display:
// inline-block; width: N%` idiom some reftest references use as an
// indent trick) would flow as unsized text instead of reserving that
// width — visibly wrong, so skip it too.
// Same blockification as a real element (css-display-3 §2.7) — a generated
// box that is floated or out of flow never joins a line box. This is what
// gives `a::after { position: absolute }` a box when the page states no
// display at all.
```

## L2303-2305 · `let own_alpha = s.opacity * s.filter.map_or(1.0, |f| f.a);`

```
// Die Deckung, die dieses Element seinen Laeufen mitgibt — siehe
// `inline_fade`. Nur ein reiner Inline-Kasten sammelt; alles mit einem
// eigenen Befehlsbereich faengt wieder bei 1 an.
```

## L2315-2317 · `if s.bg_cc {`

```
// `currentcolor` is a used-value keyword: it reads the `color` this element
// ended up with, whatever order the declarations came in and whether the
// value arrived through `inherit`.
```

## L2326-2334 · `pub fn is_generated_box(&self) -> bool {`

```
/// Does this generated element produce a BOX we lay out as a rectangle —
/// the CSS-icon idiom, `content: ""` plus a size plus a `background-image`?
///
/// Deliberately a closed list. `display: none` produces nothing, and the
/// table-internal roles have no content box of their own, so generated
/// content in them renders nothing at all — `before-content-display-012`
/// puts `content: "FAIL"` on a `display: table-column-group` and asserts
/// that nothing appears. Anything not listed here and not `inline` keeps
/// the old forward-compatible answer: produce nothing rather than guess.
```

## L2343-2346 · `#[derive(Clone, Debug, PartialEq)]`

```
/// One component of a resolved `content` value on a `::before`/`::after` box.
/// A plain string is a single `Text`; `counter()`/`counters()` stay symbolic
/// because their value depends on layout-time counter state, resolved in
/// `layout.rs`.
```

## L2350 · `Counter { name: u32, style: ListStyle },`

```
/// `counter(name, style)` — the innermost in-scope value of `name`.
```

## L2352-2353 · `Counters { name: u32, sep: String, style: ListStyle },`

```
/// `counters(name, sep, style)` — every in-scope value of `name`, joined
/// by `sep` (outermost first).
```

## L2355-2357 · `Attr(String),`

```
/// `attr(name)` — the originating element's attribute as a string, or the
/// empty string when it has no such attribute (CSS2.1 §12.2). Lowercased,
/// which is how the HTML parser stores attribute names.
```

## L2361-2367 · `pub fn parse_content_template(v: &str) -> Option<Vec<ContentPiece>> {`

```
/// Parse a CSS `content` value into its component pieces: concatenated
/// `<string>` tokens (`"a" 'b'`), `counter()`/`counters()` and `attr()`, in
/// order. Any OTHER component (`open-quote`/`close-quote`, `url()`, an unknown
/// identifier) is out of scope: rather than mis-render, the WHOLE value
/// produces no content — the caller then generates nothing, per
/// docs/spec/CONFORMANCE.md's forward-compatible rule. `none`/`normal` also produce
/// nothing (no box).
```

## L2384 · `let mut ident = String::new();`

```
// An identifier or function token: read up to whitespace or '('.
```

## L2395 · `chars.next(); // consume '('`

```
// consume '('
```

## L2400-2404 · `if lname == "counter" {`

```
// A wrong argument count or an unrecognised `<counter-style>` makes
// the WHOLE `content` value invalid — it is dropped so an earlier
// valid declaration wins (CSS Syntax 3 §4), and an unimplemented but
// syntactically-valid style falls into the same "produce nothing"
// bucket per docs/spec/CONFORMANCE.md's forward-compatible rule.
```

## L2406 · `if args.len() > 2 {`

```
// `counter(name)` | `counter(name, <style>)`
```

## L2416 · `if !(2..=3).contains(&args.len()) {`

```
// `counters(name, <sep>)` | `counters(name, <sep>, <style>)`
```

## L2430 · `chars.next(); // consume '('`

```
// consume '('
```

## L2432-2435 · `let args = split_top_commas(&inside);`

```
// CSS2.1 `attr(X)` takes exactly one argument — an attribute NAME,
// not a string. The type/fallback arguments are css-values-5 and
// would change what the value means, so they invalidate it here
// rather than being ignored.
```

## L2445-2446 · `return None;`

```
// `open-quote`, `url(...)`, or an unknown identifier — unsupported, so
// the whole value contributes nothing.
```

## L2452-2457 · `fn parse_string_token(chars: &mut core::iter::Peekable<core::str::Chars>) -> Option<String> {`

```
/// Parse ONE CSS `<string>` token, with `chars` positioned on the opening
/// quote. Consumes through the closing quote. Handles css-syntax-3 §4.3.7
/// escapes: `\` + 1-6 hex digits (+ one optional trailing whitespace) is a code
/// point (`\A` = U+000A, the "forced line break" idiom); `\` + an actual
/// newline is a line continuation (no output); `\` + any other char is that
/// literal char. Returns `None` on an unterminated string.
```

## L2459 · `let q = chars.next()?; // opening quote`

```
// opening quote
```

## L2463 · `None => return None, // unterminated → invalid value`

```
// unterminated → invalid value
```

## L2468 · `chars.next(); // escaped newline: line continuation, no output`

```
// escaped newline: line continuation, no output
```

## L2482 · `chars.next(); // one trailing whitespace terminates the escape`

```
// one trailing whitespace terminates the escape
```

## L2499-2501 · `fn read_until_close(chars: &mut core::iter::Peekable<core::str::Chars>) -> Option<String> {`

```
/// Read the raw text inside a `(` … `)` (the `(` already consumed), balancing
/// nested parens and skipping over quoted strings so a `,`/`)` inside a string
/// doesn't terminate. Returns `None` if unterminated.
```

## L2538-2539 · `fn split_top_commas(s: &str) -> Vec<String> {`

```
/// Split a function-argument list on top-level commas (commas inside quotes or
/// nested parens don't split). Each arg is returned trimmed.
```

## L2583-2584 · `fn unquote_string(s: &str) -> Option<String> {`

```
/// Unwrap a single quoted `<string>` argument (the `counters()` separator) to
/// its literal text. `None` if it isn't a proper quoted string.
```

## L2593-2595 · `fn ua_rule(tag: &str, parent: &ComputedStyle, theme: &Theme, s: &mut ComputedStyle) {`

```
/// The UA default stylesheet (HTML rendering §15), expressed as code over the
/// computed style. `em` sizes are relative to the *parent* font (per CSS), so
/// nested headings/lists scale naturally. Kept close to a browser's defaults.
```

## L2599 · `"head" | "title" | "meta" | "link" | "script" | "style" | "template"`

```
// Non-rendered subtrees.
```

## L2604-2609 · `"body" => {`

```
// `<noscript>` gehoert wieder dazu, und der alte Kommentar hier sagte
// selbst, unter welcher Bedingung: „a browser hides it only while
// scripting is ENABLED … which is literally beak's case." Beaks Fall
// ist es seit Stage 1 nicht mehr — beak faehrt Skripte. Der Parser
// liest den Inhalt jetzt als Rohtext (`dom.rs::RAWTEXT`), diese Zeile
// ist der zweite Riegel.
```

## L2618 · `"html" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav"`

```
// Block containers.
```

## L2624-2628 · `"center" => {`

```
// `<center>` is `display: block; text-align: center` (HTML rendering
// §15.3.2). Left as the initial `inline` it swallows whatever it wraps
// into a line box — and a `<table>` inside it collapses to running text.
// Hacker News wraps its ENTIRE page in one, so the whole site rendered
// as a single paragraph.
```

## L2635-2640 · `"table" => {`

```
// Tables. `<table>` gets the table formatting context; cells are block
// containers for their own content (`th` also bold). `tr`/`tbody`/… are
// walked by `layout_table`, so their display is only a fallback.
// No default margin: the HTML UA sheet gives `<table>` none (only
// `border-spacing`), and inventing one shifts everything after a table
// by half an em relative to what every reftest reference assumes.
```

## L2643-2644 · `s.border_spacing = (2.0, 2.0);`

```
// The HTML UA sheet's `border-spacing: 2px` (HTML §15.3.8). It is
// inherited, so every cell sees it without walking back up.
```

## L2650-2651 · `let p = s.attr_cell_padding.unwrap_or(1.0);`

```
// `padding: 1px` from the UA sheet, overridden by `<table
// cellpadding>` when that attribute is present.
```

## L2658 · `if s.attr_cell_border.is_some_and(|b| b > 0.0) {`

```
// `<table border>` gives every cell a 1px inset border.
```

## L2671-2673 · `"caption" => {`

```
// §15.3.8 gibt `<caption>` nur `text-align: center`. Fett und ein
// Abstand darunter waren unsere Zutat — und eine Tabellenueberschrift,
// die fett ist, wo die Seite sie mager erwartet, faellt auf.
```

## L2684-2685 · `"h1" => heading(s, theme, em, 2.00, 0.67),`

```
// Headings — Groesse und Rand aus HTML §15.3.6, nicht nach Augenmass.
// Der Rand ist in em der UEBERSCHRIFT, oben wie unten gleich.
```

## L2693 · `"ul" | "ol" | "menu" => {`

```
// Lists.
```

## L2696-2698 · `s.pad_left = 40.0;`

```
// `padding-inline-start: 40px` (HTML §15.3.6), nicht 26 nach
// Augenmass: jede Liste einer ungestalteten Seite stand 14 px zu
// weit links, und ein Reftest backt die Zahl als Pixel ein.
```

## L2703-2707 · `if parent.display == Display::ListItem {`

```
// Eine Liste IN einer Liste hat keinen Aussenrand. Die
// Spezifikation sagt „irgendein Listen-Vorfahr"; wir sehen den
// Elter, und der ist bei der Schachtelung, die vorkommt, das
// `<li>`. `li > div > ul` faellt durch — und faellt auf, sobald es
// jemand misst.
```

## L2722-2723 · `s.margin_left = Len::Px(40.0);`

```
// `margin-inline-start: 40px` — ein RAND, keine Polsterung: ein
// Hintergrund auf `<dd>` faengt links bei 40 px an, nicht bei 0.
```

## L2727-2732 · `"blockquote" | "figure" => {`

```
// `margin-block: 1em; margin-inline: 40px` (HTML §15.3.3) — ein RAND
// aussen, keine Polsterung innen. Der Unterschied ist sichtbar,
// sobald das Zitat einen Hintergrund oder Rahmen traegt. Und keine
// eigene Farbe: die Spezifikation faerbt `<blockquote>` nicht, und ein
// grauer Kasten auf einer Seite, die ihn schwarz erwartet, ist unser
// Geschmack im Blatt eines fremden Autors.
```

## L2755-2756 · `"address" => {`

```
// `font-style: italic` (HTML §15.3.3). Die einzige Vorgabe, die
// `<address>` von einem `<div>` unterscheidet.
```

## L2762-2765 · `"fieldset" => {`

```
// §15.3.11: 2 px Aussenrand, ein 2 px `groove`-Rahmen und eine
// Polsterung, die oben und unten verschieden ist. Ohne den Rahmen war
// ein `<fieldset>` von einem `<div>` nicht zu unterscheiden — und das
// Feld, das es umschliesst, ist genau der Zweck des Elements.
```

## L2781-2786 · `"legend" => {`

```
// `<legend>` ist kein gewoehnlicher Block: es schrumpft auf seinen
// Text und sitzt auf dem oberen Rahmen seines `<fieldset>`. Die
// Spezifikation beschreibt das als eigenen Kasten; `fit-content` holt
// die BREITE davon ein (96 statt 1854 px), die Lage auf dem Rahmen
// noch nicht — die kostet ein Loch im Rahmen und einen Kasten, der aus
// dem Fluss faellt, und das ist eine eigene Arbeit.
```

## L2795 · `"a" => {`

```
// Inline styling.
```

## L2805-2807 · `"small" => s.font_px = em / 1.2,`

```
// `smaller` und `larger` sind EINE Stufe der Schriftskala, also
// /1,2 und ×1,2 (css-fonts-4 §3.3) — nicht 0,85 und 1,15 nach
// Augenmass. Chromium rechnet aus 16 px genau 13,3333 und 19,2.
```

## L2812-2813 · `"sup" => {`

```
// Superscript / subscript: `font-size: smaller`, von der Grundlinie
// gehoben bzw. gesenkt (HTML §15.3.4).
```

## L2822 · `_ => {}`

```
// span / label / abbr / time / u / s / … → plain inline.
```

## L2836-2842 · `fn split_important(v: &str) -> (&str, bool) {`

```
/// Parse and apply a `style="a: b; c: d"` declaration list. This is real CSS
/// declaration syntax (css-syntax-3), just without selectors — the same parser
/// a `<style>` rule body will use. Unknown properties are ignored (forward
/// compatible, like a browser).
/// Split a declaration value into (value, is_important). `!important` is a
/// trailing flag (css-syntax-3): optional whitespace, then `!important`
/// (case-insensitive).
```

## L2853-2860 · `fn set_var(own: &mut Option<crate::vars::VarMap>, inherited: &crate::vars::VarMap,`

```
/// Apply the `style="…"` declarations whose importance matches `important`, so
/// callers run the two cascade passes. css-syntax-3 syntax, unknown props skipped.
/// Eine Custom Property setzen — und dabei ihren eigenen Wert einsetzen.
///
/// `skip` ist ihr eigener Name: `--x: var(--x, 1rem)` heisst „nimm den
/// geerbten Wert, sonst 1rem" (so schreibt es Wikipedia). Wuerde sie sich
/// selbst finden, bliebe ein `var()` stehen und die Deklaration waere
/// ungueltig.
```

## L2863 · `if own.is_none() { *own = Some(inherited.clone()); }`

```
// Erst hier kopieren: wer nichts setzt, teilt die Karte des Elternteils.
```

## L2866-2877 · `crate::vars::var_set(map, name, val);`

```
// ROH ablegen, nicht ersetzen. Der Wert einer Custom Property wird erst
// eingesetzt, wenn ihn jemand BENUTZT (css-variables-1 §3) — und dann gegen
// die fertige Karte. Wer hier schon ersetzt, friert den Stand der Kaskade
// von diesem Augenblick ein: Tailwind schreibt
//
//     .ring-4        { --tw-ring-shadow: … var(--tw-ring-color,currentcolor) }
//     .ring-blue-500 { --tw-ring-color:  var(--color-blue-500) }
//
// und die Farbregel steht HINTER der Breitenregel. Eingesetzt wurde
// deshalb der Ausweichwert `currentcolor`, und `ring-4 ring-blue-500`
// malte einen Ring in der Textfarbe. Ringe im Schleifchen fangen die
// Paesse in `expand` ab, nicht mehr diese Stelle.
```

## L2881 · `fn inline_customs(decls: &str, important: bool) -> alloc::vec::Vec<(String, String)> {`

```
/// Die Custom Properties eines `style`-Attributs.
```

## L2895 · `fn apply_var_decl(prop: Prop, v: &str, theme: &Theme, parent: &ComputedStyle,`

```
/// Eine gewoehnliche Deklaration anwenden, `var()` vorher ersetzt.
```

## L2950-2951 · `impl ComputedStyle {`

```
/// Apply a single `prop: val` declaration. Shared by inline styles now and by
/// author `<style>` rules later.
```

## L2953 · `pub fn units(&self) -> Units {`

```
/// The `em`/`rem` bases for parsing this element's declarations.
```

## L2959 · `pub const VH_DIRECT: u8 = 1;`

```
/// `vh_seen` bits — see `ComputedStyle::vh_seen`.
```

## L2964-2966 · `fn has_viewport_h_unit(v: &str) -> bool {`

```
/// Does this declaration value carry a length relative to the viewport
/// HEIGHT? `vw` alone does not: a width-only dependency is already covered by
/// the layout width being part of the cache key.
```

## L2971 · `let starts = i > 0 && (b[i - 1].is_ascii_digit() || b[i - 1] == b'.');`

```
// A unit only ever follows a digit or a '.', never a letter.
```

## L2986-2987 · `fn is_revert_layer(v: &str) -> bool {`

```
/// The `revert-layer` keyword (css-cascade-5 §7.4): roll this property back to
/// what it would be if the declaring layer had never said anything about it.
```

## L2993-3000 · `fn resolve_revert_layers(matched: &[crate::css::Matched], important: bool) -> Vec<(Prop, u16)> {`

```
/// Which `(property, layer)` pairs a winning `revert-layer` removes from the
/// cascade. `matched` is in ascending cascade order, so a property's winner is
/// its LAST declaration; if that one says `revert-layer`, its whole layer goes
/// out for that property and the next-lower layer's declaration takes over —
/// which may itself be a `revert-layer`, hence the loop.
///
/// Unlayered rules revert against the UA sheet, which is already applied by
/// the time this runs, so taking them out leaves exactly the right value.
```

## L3007-3008 · `if dead.contains(&(*p, *layer)) || dead.contains(&(Prop::All, *layer)) {`

```
// `all: revert-layer` reverts the layer for EVERY property, so
// a dead `All` takes the whole layer with it.
```

## L3022-3023 · `let mut changed = false;`

```
// Each round kills at least one (property, layer) pair or stops, and
// there are finitely many, so this terminates.
```

## L3037-3039 · `fn apply_decl(prop: Prop, v: &str, theme: &Theme, parent: Option<&ComputedStyle>, s: &mut ComputedStyle) {`

```
/// One declaration, with the CSS-wide keywords taken first. `parent` is what
/// `inherit` reads; a caller with no parent to hand (the UA sheet, which never
/// writes one) passes `None` and gets the old behaviour.
```

## L3049-3053 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// A CSS-wide keyword (css-cascade-5 §7). `revert` is deliberately absent:
/// it rolls back to the UA ORIGIN, which would mean snapshotting every
/// element's style after `ua_rule` and before the author cascade — a copy of
/// a large `Copy` struct per element, for a keyword six tests in the whole
/// corpus write. It keeps its previous per-property handling.
```

## L3061-3064 · `pub fn wide_keyword(v: &str) -> Option<Wide> {`

```
/// Is this declaration value a CSS-wide keyword? These apply to EVERY
/// property, so before this they were handled property by property — a
/// handful had an arm, and on the rest `border-bottom-color: inherit` simply
/// failed to parse and left the previous declaration standing.
```

## L3067 · `if !(5..=7).contains(&v.len()) {`

```
// Cheap gate: the three keywords are 5-7 bytes and start with i/u.
```

## L3082-3090 · `pub fn apply_wide(prop: Prop, kw: Wide, parent: &ComputedStyle, theme: &Theme, s: &mut ComputedStyle) -> bool {`

```
/// Apply a CSS-wide keyword to one property.
///
/// `inherit` takes the parent's computed value. `unset` takes whichever of
/// inherit/initial the property's inheritance says — which `inherit_reset`
/// already encodes exactly: what it copies from the parent is inherited, what
/// it leaves at the default is not. `initial` reads from a fresh root style.
///
/// Returns false for a property with no arm here, so the caller can fall
/// through to the old per-property handling rather than silently dropping it.
```

## L3105-3109 · `Prop::All => {`

```
// `all` is every property at once, minus `direction`/`unicode-bidi`,
// which css-cascade-5 §3.2 excludes by name. Everything else in the
// struct that is NOT a property has to survive: the document-global
// bases, the presentational-attribute hints, and the element-identity
// flags the UA sheet set.
```

## L3127-3128 · `s.em_base = s.font_px;`

```
// Later declarations in the same block measure `em` against the
// size `all` just installed.
```

## L3146-3147 · `Prop::Border => {`

```
// Border: the shorthands copy whole sides, the longhands one field of
// one side — the same split the parser makes.
```

## L3270-3271 · `Prop::FontSize => {`

```
// `font-size` moves the em basis every later declaration measures
// against, so it has to travel with the size.
```

## L3309-3313 · `let t = val.trim();`

```
// CSS keywords are case-insensitive, so this used to lowercase every value
// it was handed. On a real article that is ~152 000 declarations per
// layout of which ~500 actually carry an uppercase letter: 151 500 heap
// allocations to produce a copy identical to the input. Borrow instead,
// and only allocate for the 0.3 % that need it.
```

## L3322-3324 · `if has_viewport_h_unit(&v) {`

```
// Only declarations that actually reach an element pass here, so this
// counts MATCHED rules rather than occurrences in the stylesheet text —
// Wikipedia's three stray `vh`s never match anything.
```

## L3332-3335 · `let u = s.units();`

```
// Font-relative bases for this element, taken once: `apply_one` handles a
// single declaration, so `font-size` (which uses its own inherited base)
// is the only property that could move them, and it does so for the NEXT
// call — matching the cascade's declaration order.
```

## L3338-3339 · `Prop::All => {}`

```
// `all` only ever carries a CSS-wide keyword, and `apply_wide` has
// already taken those. Anything else is an invalid declaration.
```

## L3348-3349 · `"flow-root" => {`

```
// A block box that establishes a BFC — the explicit spelling
// of the `overflow: hidden` clearfix, without the clipping.
```

## L3379-3384 · `Prop::Overflow => {`

```
// `word-wrap` is the legacy alias of `overflow-wrap`; `word-break:
// break-word` is a deprecated spelling with the same effect. All three
// land on one flag — we break at a character, not by script rules, so
// `break-all` is not distinguished from `break-word`.
// Two values are `x y`; a single one applies to both. Only a box that
// clips on BOTH axes is clipped here (see `overflow_clip`).
```

## L3394-3397 · `Prop::TextOverflow => s.ellipsis = v.split_whitespace().next_back().is_some_and(|t| t != "clip"),`

```
// `text-overflow` takes two values in css-ui-4 (line-start, line-end);
// only the END one is ever anything but `clip` on a real page, and the
// one-value form sets exactly that. A `<string>` custom ellipsis is
// parsed as "not `clip`" rather than rendered literally.
```

## L3399-3403 · `Prop::Filter | Prop::WebkitFilter => {`

```
// A chain of only colour functions composes to one matrix; anything
// else (`blur`, `drop-shadow`, `url()`) leaves the property alone, so
// the page gets its own pixels rather than a guess at a blur.
// The identity is kept as `None` — it is `filter: none`, and carrying
// it would put every op of the subtree through a no-op transform.
```

## L3410-3411 · `s.object_fit = match v.trim() {`

```
// `object-fit` also accepts `scale-down` paired with `none`; the
// pair is what `scale-down` already means, so the keyword decides.
```

## L3445-3446 · `if let Some(n) = parse_len_opt(css_tokens(&v).first().copied().unwrap_or(""), u) {`

```
// One corner takes `h v`; we keep the horizontal radius.
// Klammernbewusst: eine Ecke kann `calc(…) calc(…)` tragen.
```

## L3462 · `Prop::BorderSpacing => {`

```
// One length applies to both axes; two give horizontal then vertical.
```

## L3482-3484 · `let base = s.em_base;`

```
// em/%/inherit/relative keywords resolve against the PARENT font
// (em_base), NOT the running value — so nothing compounds and a
// later cascade winner (incl. `inherit`) is exact, not multiplied.
```

## L3497-3500 · `_ if unitless_nonzero(&v) => None,`

```
// Ohne Einheit ist es keine Laenge und damit eine ungueltige
// Deklaration — die faellt WEG und laesst den geerbten Wert
// stehen, statt als 700-px-Schrift durchzukommen
// ([[feedback_unknown_unit_invalidates_the_declaration]]).
```

## L3508-3510 · `Prop::LineHeight => {`

```
// `line-height: normal | <number> | <length> | <percentage>`. A bare
// number stays a number (inherits as a ratio); everything else computes
// to px against THIS element's font-size, per CSS 2.1 §10.8.1.
```

## L3523-3528 · `match parse_len_opt(t, u) {`

```
// **`calc()` ohne Einheit ist eine ZAHL, keine Laenge.**
// Tailwind v4 schreibt JEDE Zeilenhoehe so — `.text-xs` bringt
// `line-height: calc(1 / .75)` mit. Als Laenge gelesen sind
// das 1,3 PIXEL: die Zeilenkaesten fallen auf null zusammen,
// aufeinanderfolgende Absaetze werden uebereinandergedruckt,
// und der Text landet mit negativem y ueber dem Seitenrand.
```

## L3542-3543 · `if v != "inherit" && v != "unset" {`

```
// The shorthand resets the line to `none` when it names no line
// keyword, so colour/style-only values legitimately clear it.
```

## L3548-3550 · `Prop::LetterSpacing | Prop::WordSpacing => {`

```
// `normal` is zero extra advance; anything else is a length. A
// percentage is only legal on `word-spacing` and resolves against the
// space's own advance, which is not known here — dropped, not guessed.
```

## L3553-3555 · `let px = if t == "normal" {`

```
// A percentage resolves against the element's own font-size
// (css-text-4 §8.1/§8.2) — not against a containing block, which is
// what `parse_length` would do with it.
```

## L3575 · `Some(ColorVal::CurrentColor) => None,`

```
// `None` on this field is `currentColor` — its initial value.
```

## L3589-3590 · `Prop::TextAlignLast => {`

```
// Applies to the last line of a block (and so to a block with only
// one). `auto` defers to `text-align`.
```

## L3621-3622 · `"match-parent" | "inherit" | "unset" => s.text_align,`

```
// `match-parent` on a LTR root computes to `left`; `inherit`/
// `unset` are already the inherited value we started from.
```

## L3632-3633 · `Prop::ListStyle => {`

```
// `list-style: <type> || <position> || <image>` in any order; we only
// consume the type keyword. `none` legitimately means "no marker".
```

## L3642-3644 · `Prop::CounterReset => parse_counter_ops(&v, &mut s.counter_reset, &mut s.counter_reset_n, 0),`

```
// CSS counters (css-lists-3 §4). `content: counter(…)` reads these at
// layout time; the values themselves are resolved in `layout.rs`, which
// maintains the scoped counter stack.
```

## L3666-3669 · `_ => {}`

```
// `inherit`/`unset`/garbage: an invalid or non-recomputable value
// drops (CSS Syntax 3 §4), keeping whatever the cascade already
// set — for `inherit` specifically, that's already the parent's
// value, since `pre` is copied from `parent` before this runs.
```

## L3672-3674 · `Prop::Opacity => {`

```
// `collapse` differs from `hidden` only on table rows/columns (where it
// removes the track); everywhere else the spec says treat it as
// `hidden`, and we have no row-removal to do.
```

## L3676-3678 · `let t = v.trim();`

```
// Also `50%` — css-color-4 allows a percentage everywhere a
// <alpha-value> is taken, and Tailwind's `opacity-*` emits plain
// numbers while hand-written CSS often does not.
```

## L3698 · `Prop::Width | Prop::InlineSize => set_size(&mut s.width, &v, u),`

```
// — box model —
```

## L3706-3707 · `Prop::AspectRatio => {`

```
// `<number>` or `<ratio>` (`16 / 9`); `auto` and a degenerate ratio
// both mean "no preferred ratio".
```

## L3724-3727 · `Prop::Appearance | Prop::WebkitAppearance | Prop::MozAppearance => s.appearance_none = v == "none",`

```
// css-ui-4 §4. Only `none` concerns us: it says "do not draw the UA
// widget", and the page then supplies the whole look. Every other
// value (`auto`, `button`, `textfield`, …) keeps our chrome. The
// prefixed spellings still carry the real web's styled controls.
```

## L3731 · `let mut it = css_tokens(&v).into_iter().filter_map(|t| parse_length(t, u));`

```
// definite length(s): one → both axes, two → (width, height).
```

## L3753-3754 · `Prop::MarginInline => {`

```
// Logical two-value shorthands, LTR/horizontal-tb: `margin-inline` is
// (left, right), `margin-block` is (top, bottom); one value sets both.
```

## L3768-3769 · `(s.pad_top, s.pct_pad[0]) = (0.0, 0.0);`

```
// The shorthand resets every side, so an invalid one lands on zero
// rather than keeping what the side happened to hold.
```

## L3794-3797 · `Prop::BackgroundColor => {`

```
// — background + border —
// `background-color` is a single property; `background` is a shorthand
// that resets every longhand it covers — including the image — and is
// applied as a unit or not at all.
```

## L3803-3804 · `s.bg = match cv {`

```
// Handles space-separated function colours like
// `rgb(0% 50% 0%)` / `hsl(120 100% 25%)`.
```

## L3817-3819 · `s.bg = match cv {`

```
// The whole value is one colour — the overwhelmingly common
// case, and the only one where a function colour's internal
// spaces must not be read as separate tokens.
```

## L3844-3848 · `if image.is_some() || gradient.is_some() || v == "none" {`

```
// Einen Wert, den wir gar nicht lesen koennen, VERWIRFT css-syntax-3
// — der vorige bleibt stehen. `-webkit-gradient(…)` ist so einer,
// und Blaetter schreiben ihn als Vorspann vor die
// Standardschreibweise. Ihn als „kein Bild" zu nehmen loeschte
// genau den Verlauf, der eine Zeile spaeter kommt.
```

## L3854-3856 · `Prop::BackgroundClip | Prop::WebkitBackgroundClip => {`

```
// `background-clip: text` stencils the background through the glyphs —
// a different mechanism, not a smaller box — so it is left alone rather
// than approximated by one of the three rectangles.
```

## L3882-3883 · `Prop::Mask | Prop::WebkitMask => {`

```
// `mask` is still shipped prefixed by the icon systems that use it, and
// the two spellings are the same property to us.
```

## L3885-3887 · `if let Some((_, layer, ..)) = parse_bg_shorthand(val, &v, u, theme, &mut false) {`

```
// A mask takes no colour of its own — it stencils the element's
// `background-color` — so the shorthand's colour, `currentcolor`
// included, has nothing to land on here.
```

## L3915-3917 · `Prop::Outline => { s.outline = parse_border_shorthand(&v, u, theme); s.outline_set = true; }`

```
// `outline` reuses the border shorthand grammar (width || style ||
// colour) — css-ui-4 §3.5 defines it that way, minus `outline-style:
// auto`, which is the UA's own focus ring and not a value we can draw.
```

## L3925-3926 · `Prop::OutlineStyle => {`

```
// `auto` is a UA-defined ring; treat it as `solid` so a page asking for
// a focus ring gets one instead of nothing.
```

## L3936-3937 · `if v.trim() != "invert" {`

```
// `invert` has no equivalent here (we do not read back pixels);
// currentColor is the honest approximation and stays visible.
```

## L3947-3950 · `Prop::AccentColor => {`

```
// `auto` ist der Anfangswert und heisst „das Thema entscheidet".
// Jede andere Farbe gilt; eine unlesbare laesst den Vorgaenger stehen
// (dieselbe Regel wie ueberall — ein gescheiterter Parse verwirft die
// Deklaration, er loescht nicht).
```

## L4011 · `Prop::Position => {`

```
// — positioning —
```

## L4046 · `let norm = inner.replace(',', " ");`

```
// Four <length>|auto components, comma- or space-separated.
```

## L4069-4071 · `Prop::Inset => {`

```
// The logical inset properties. This engine lays out horizontal-tb
// only, so `inline` is the left/right axis and `block` top/bottom; the
// `start`/`end` mapping follows `direction`, which we do honour.
```

## L4100-4101 · `None => s.z_index,`

```
// Invalid <integer> → declaration ignored (keeps whatever
// the cascade already had, per CSS error handling).
```

## L4107 · `Prop::FlexDirection => s.flex_row = !v.starts_with("column"),`

```
// — flex —
```

## L4132 · `Prop::Gap | Prop::GridGap => {`

```
// `gap` shorthand is `<row-gap> <column-gap>`; the longhands set one axis.
```

## L4134-4136 · `let t = css_tokens(&v);`

```
// Klammernbewusst, wie `padding` und `margin` — sonst zerfaellt
// `gap: calc(.25rem * 3)` in drei Wortstuecke und der Abstand ist
// null. Tailwind schreibt jedes `gap-*` so.
```

## L4178 · `Prop::GridTemplateColumns => {`

```
// — grid —
```

## L4195-4196 · `Prop::Grid | Prop::GridTemplate => {`

```
// `grid` / `grid-template` shorthand: `<rows> / <cols>` (areas/flow forms
// are not supported — they fall through to the row/column split).
```

## L4198 · `if v.contains('"') || v.contains('\'') {`

```
// The shorthand may carry `grid-template-areas` strings.
```

## L4227-4228 · `let name = v.trim();`

```
// `grid-area: <name>` (custom ident) → named placement. The numeric
// `row / col / …` form is left to grid-row/grid-column longhands.
```

## L4247 · `Prop::PlaceContent => {`

```
// `place-content: <align-content> <justify-content>`; one value sets both.
```

## L4267-4271 · `fn parse_saturating_i32(v: &str) -> Option<i32> {`

```
/// A CSS `<integer>`, saturating to the 32-bit signed range instead of
/// rejecting out-of-range literals as invalid (CSS Values & Units — Range
/// Checking: values outside the supported range are clamped, not dropped).
/// Accepts an optional leading `+`/`-` and ASCII digits only; `None` for
/// anything else (empty, non-digit, signs-only).
```

## L4289-4293 · `fn parse_len_opt(v: &str, u: Units) -> Option<Len> {`

```
/// A `<length>`/`auto`/`%` value for the box model.
/// Fallible length parse. `None` = the value is invalid, so the caller must
/// KEEP the previously-cascaded value — an invalid declaration is dropped, it
/// does not reset the property to its default (CSS Syntax 3 §4). `auto` is a
/// valid keyword and returns `Some(Len::Auto)`.
```

## L4314-4318 · `fn is_math_fn(v: &str) -> bool {`

```
/// Does this value start with a CSS math function? `values.rs` evaluates all
/// four; this is only the gate that sends them there. `min`/`max`/`clamp` were
/// missing from it, so `width: max(20px, 10px)` fell through to a plain length
/// parse, failed, and became `auto` — while the same expression inside a custom
/// property resolved fine, because `vars.rs` calls the resolver directly.
```

## L4320-4322 · `["calc(", "min(", "max(", "clamp("]`

```
// `get` rather than a range index: a value may now begin with a multi-byte
// character — an escape that decoded to U+FFFD is one — and slicing to a
// byte length would land inside it and panic.
```

## L4332 · `fn first_layer(v: &str) -> &str {`

```
// ── background-image / mask-image ───────────────────────────────────────────
```

## L4334-4335 · `fn first_layer(v: &str) -> &str {`

```
/// The first layer of a comma-separated `<bg-layer>` list. Splitting has to be
/// paren-aware: a `data:` URI is full of commas.
```

## L4340-4342 · `fn next_layer(v: &str) -> (&str, &str) {`

```
/// One layer off the front of a comma-separated list, plus what is left. `""`
/// as the rest means the list is done — a trailing comma yields one empty
/// final layer, which every caller rejects on its own terms.
```

## L4359-4360 · `fn parse_bg_image(val: &str) -> Option<u64> {`

```
/// `background-image`/`mask-image` → a URL key. Values we cannot paint
/// (gradients, `none`, `element()`) leave the layer imageless.
```

## L4365-4369 · `fn split_top(s: &str) -> alloc::vec::Vec<&str> {`

```
/// Die Argumente einer Funktion auf OBERSTER Ebene trennen.
///
/// Ein Komma in `rgba(0,0,0,.5)` trennt keine Argumente — ohne diese
/// Klammerzaehlung zerfaellt jeder Verlauf mit einer `rgba()`-Farbe in
/// Bruchstuecke.
```

## L4386 · `fn fn_body<'a>(v: &'a str, name: &str) -> Option<&'a str> {`

```
/// Der Rumpf einer Funktion, wenn der Text mit ihrem Namen beginnt.
```

## L4402-4403 · `fn parse_angle(v: &str) -> Option<f32> {`

```
/// Ein Winkel in Grad. `0deg` zeigt nach OBEN und dreht im Uhrzeigersinn —
/// das ist die Zaehlweise von CSS und nicht die der Mathematik.
```

## L4418-4426 · `fn side_angle(v: &str) -> Option<(f32, u8)> {`

```
/// `to right`, `to bottom left`, … als Winkel — und, bei einer ECKE, als
/// Eckenschluessel.
///
/// Eine Ecke ist keine feste Zahl: css-images-3 §3.4.1 verlangt, dass die
/// Achse so liegt, dass die Senkrechte durch die Mitte die beiden
/// NACHBARecken trifft. Auf einem 800x60-Kasten sind das 85,7 Grad und nicht
/// 45 — der Unterschied zwischen „fast waagrecht" und „diagonal". Der Winkel
/// haengt also am Kasten und wird erst beim Malen gerechnet; hier faellt nur
/// der Schluessel an, plus die 45-Grad-Naeherung als Rueckfalltyp.
```

## L4450-4452 · `fn split_interpolation(a0: &str) -> (&str, bool) {`

```
/// Das ` in <farbraum> [<hue>]` vom ersten Parameter abtrennen.
///
/// Zurueck kommt der Rest (oft leer) und ob eine Angabe da war.
```

## L4456 · `let _ = r;`

```
// Nur die Angabe, keine Richtung: `linear-gradient(in oklab, …)`.
```

## L4477-4478 · `fn gradient_token(layer: &str) -> Option<&str> {`

```
/// Den Verlaufsaufruf aus einer Schicht herausschneiden — er kann hinter
/// einer Farbe stehen. Zurueck kommt der Text ab dem Funktionsnamen.
```

## L4481 · `let b = layer.as_bytes();`

```
// Zurueck bis zum Anfang des Bezeichners (`repeating-radial` …).
```

## L4490-4495 · `pub fn parse_gradient(val: &str, theme: &Theme) -> Gradient {`

```
/// Einen Verlauf aus `background-image` lesen.
///
/// Gebaut wird, was echte Seiten schreiben — ausgezaehlt ueber 23 Blaetter:
/// `linear-gradient` (172 Vorkommen), `radial-gradient` (36), je auch in der
/// `repeating-`Form. `conic-gradient` (11) fehlt noch und wird ehrlich
/// abgelehnt statt als linearer gemalt.
```

## L4497-4499 · `let Some(v) = gradient_token(first_layer(val)) else { return Gradient::NONE };`

```
// Der Verlauf steht nicht zwingend am Anfang: `background: #eee
// linear-gradient(…)` ist eine Kurzform, und die Farbe kommt zuerst.
// Ohne diese Suche verlor die Deklaration ihren Verlauf still.
```

## L4515 · `let mut angle = if kind == GradKind::Linear { 180.0 } else { 0.0 };`

```
// Erstes Argument: Richtung oder Form — oder schon der erste Farbstopp.
```

## L4520-4524 · `let (a0, interp) = split_interpolation(args[0]);`

```
// `in oklab` / `in hsl longer hue` sagt, in WELCHEM Raum gemischt wird.
// Wir mischen in sRGB und lassen die Angabe fallen — ein etwas anderer
// Mittelweg zwischen denselben Farben. Sie mitzulesen ist trotzdem
// Pflicht: Tailwind v4 schreibt JEDEN Verlauf so, und ohne diesen
// Zweig faellt der erste Parameter durch und der Verlauf ganz weg.
```

## L4532-4534 · `circle = a0.starts_with("circle");`

```
// Von der Form wird nur `circle` gelesen — die Mitte und die
// Groessenwoerter (`closest-side` …) fallen auf die Vorgabe
// „Mitte, farthest-corner" zurueck, und das ist der haeufigste Fall.
```

## L4540-4541 · `for a in &args[first..] {`

```
// Ein Stopp kann ZWEI Positionen tragen (`red 0% 40%`) — das ist die
// Kurzform fuer zwei Stopps derselben Farbe.
```

## L4546-4549 · `let color = match parse_color_val(cs, theme) {`

```
// Ein Farbstopp, den wir nicht lesen koennen, laesst den GANZEN
// Verlauf fallen. Ihn zu ueberspringen waere schlimmer: die
// uebrigen Stopps ruecken auf und malen ein anderes Bild, das
// aussieht, als haetten wir es gekonnt.
```

## L4571-4572 · `g`

```
// Die Luecken bleiben offen: `fill_positions` laeuft in `resolved()`,
// wenn die Achsenlaenge feststeht und px-Lagen Anteile geworden sind.
```

## L4576 · `fn split_ws_top(s: &str) -> alloc::vec::Vec<&str> {`

```
/// Wie `split_top`, aber an Leerzeichen — fuer `red 0% 40%`.
```

## L4597-4600 · `fn pct_or_len(v: &str) -> Option<(f32, bool)> {`

```
/// Eine Stopp-Position als Anteil 0..1. Laengen ohne Bezug (px) koennen hier
/// noch nicht aufgeloest werden — die Kastenbreite steht erst im Layout fest.
/// Eine Stopp-Lage: Anteil (`50%`) oder absolut (`40px`). Der Rueckgabewert
/// sagt mit, welches von beidem — umrechnen kann erst der Kasten.
```

## L4609-4611 · `if t == "0" {`

```
// Eine nackte Null IST eine Laenge — `#000 0 10px` steht so in echten
// Blaettern. Ohne diesen Zweig faellt sie auf „nicht angegeben" zurueck
// und wird still verteilt.
```

## L4618-4619 · `fn fill_positions(g: &mut Gradient) {`

```
/// Die Luecken zwischen gesetzten Positionen gleichmaessig fuellen — so
/// schreibt es die Spezifikation vor (css-images-3 §3.4.3).
```

## L4636 · `for i in 1..n {`

```
// Eine Position darf nie kleiner sein als die davor.
```

## L4677-4679 · `fn parse_pos_component(v: &str, u: Units) -> Option<(Option<bool>, BgPos)> {`

```
/// One `background-position` component: a keyword, a length or a percentage.
/// `Some((axis, pos))` where `axis` is `Some(false)` for horizontal-only
/// keywords, `Some(true)` for vertical-only, `None` when it fits either.
```

## L4696-4698 · `fn parse_bg_pos(v: &str, u: Units) -> Option<(BgPos, BgPos)> {`

```
/// `background-position` (css-backgrounds-3 §3.6), one- and two-value forms.
/// A keyword binds to its own axis regardless of order, so `center right`
/// means x=right, y=center.
```

## L4713 · `if ax == Some(true) || bx == Some(false) {`

```
// Reject a pair that names the same axis twice (`left right`).
```

## L4751-4758 · `fn parse_bg_shorthand(`

```
/// The `background`/`mask` shorthand, parsed as a unit: `(colour, layer)`.
///
/// `None` means the value is INVALID and the whole declaration must be dropped
/// (css-syntax-3 §4) — `background: "red"` and `background:\0020red` are the
/// reftests that insist on it. That distinction is the whole reason this
/// returns a result instead of mutating: a shorthand resets every longhand it
/// covers, so treating an unparseable value as "no colour named" would clear a
/// perfectly good background instead of leaving it alone.
```

## L4764-4765 · `cc: &mut bool,`

```
// `cc` is set when the colour in the shorthand was `currentcolor`:
// resolving it needs the element's own `color`, which this walk cannot see.
```

## L4770-4772 · `let (mut origin, mut clip) = (None, None);`

```
// A `<box>` in the shorthand sets ORIGIN; a second one sets clip. With only
// one, both take it (css-backgrounds-3 §3.10) — which is why they are
// tracked as one `Option` that fills twice.
```

## L4776-4778 · `let spaced = first_layer(v).replace('/', " / ");`

```
// Position and size are written `<position> / <size>`; everything else is
// order-free. Collect the unclaimed tokens and split them on the slash.
// The slash need not be spaced (`center/contain`), so give it room first.
```

## L4785-4786 · `color = match cv {`

```
// Inside the shorthand's token walk there is no `ComputedStyle` to
// read `color` from; the caller applies the flag.
```

## L4795-4799 · `|| tok.contains("-gradient(")`

```
// Ein Verlauf ist hier schon als `layer.gradient` gelesen; dieser
// Zweig ueberspringt nur sein TOKEN, damit es nicht als Farbe oder
// Platzangabe missverstanden wird. Er behaelt den Reset
// (`background: <gradient>` HAT keine Farbe) statt
// dropping the declaration and leaving a stale one in place.
```

## L4802 · `} else {`

```
// attachment / the image — not the layer's placement
```

## L4826 · `fn size_non_negative(l: &Len) -> bool {`

```
/// `width`/`height`/`min`/`max` reject negative used lengths as invalid.
```

## L4834-4839 · `fn parse_dimension_attr(v: &str) -> Option<Len> {`

```
/// Assign a size property only if the value is valid AND non-negative, else
/// keep the prior value (invalid declaration dropped).
/// Parse an HTML *dimension* attribute value: a bare number is pixels, a
/// trailing `%` is a percentage (HTML §2.4.4.4). Deliberately NOT the CSS
/// length parser — `width="200"` carries no unit and CSS would reject it,
/// which is precisely how these attributes came to be ignored.
```

## L4846-4847 · `let n: f32 = num.parse().ok()?;`

```
// Trailing junk is not a number: HTML's own rule is to stop at the first
// non-digit, but a strict parse keeps a typo from becoming a silent 0.
```

## L4861 · `fn set_max(slot: &mut Len, v: &str, u: Units) {`

```
/// `max-width`/`max-height`: `none` = no maximum (Auto); else a non-negative size.
```

## L4870-4872 · `fn parse_calc_affine(v: &str, u: Units) -> Option<Len> {`

```
/// Resolve a `calc()` to affine `(pct, px)` form via the full values resolver:
/// evaluate with a %-basis of 0 (→ the px part) and 100 (→ px + pct), so any
/// `%`/px/em/vw mix collapses to `pct% of basis + px`.
```

## L4890-4891 · `fn is_font_size_token(t: &str) -> bool {`

```
/// Whether a token can start a `font-size` — the shorthand's anchor: everything
/// before it is style/variant/weight, everything after it is the family.
```

## L4904-4908 · `let num_end = head.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(head.len());`

```
// **Eine blosse Zahl ist KEINE Schriftgroesse.** `font: 700 13px/1.6 x`
// fing sonst bei der `700` an: das Gewicht wurde zur Groesse, `13px/1.6`
// zur Familie. Gemessen an der eigenen Komponentenvorlage — eine Zeile
// Text wurde 285 statt 21 px hoch, und die Schreibweise steht auf halben
// Web. Eine Laenge braucht eine Einheit; einzige Ausnahme ist die 0.
```

## L4914-4917 · `fn apply_font_shorthand(v: &str, theme: &Theme, s: &mut ComputedStyle) {`

```
/// `font: [<style> || <variant> || <weight>] <size>[/<line-height>] <family>`
/// (CSS 2.1 §15.8). Resetting every sub-property it does not mention is the
/// whole point of the shorthand, so unspecified style/weight/line-height go
/// back to their initial values rather than keeping what the cascade had.
```

## L4923-4925 · `if matches!(t, "inherit" | "unset" | "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar") {`

```
// `inherit` restores the parent's font; `em_base` IS the parent's size.
// System-font keywords have no user-configurable faces here, so they take
// the UA body font.
```

## L4933 · `return; // no size → not a valid font shorthand, change nothing`

```
// no size → not a valid font shorthand, change nothing
```

## L4944 · `Err(_) => return, // an unknown keyword invalidates the whole value`

```
// an unknown keyword invalidates the whole value
```

## L4965-4969 · `fn is_unitless_calc(v: &str) -> bool {`

```
/// Ist das ein `calc()`, in dem keine Einheit und kein Prozent vorkommt?
///
/// Dann ist sein Ergebnis eine ZAHL. Der Unterschied entscheidet bei
/// `line-height` zwischen einem Verhaeltnis und einer Laenge — und zwischen
/// einer lesbaren Seite und uebereinandergedrucktem Text.
```

## L4978-4980 · `let b = t.as_bytes();`

```
// Ein Einheitenzeichen steht IMMER direkt hinter einer Ziffer oder einem
// Punkt. Ein blosser Buchstabe ist dagegen ein Funktionsname (`min`,
// `max`, `var` sind vorher schon ersetzt).
```

## L4990-5001 · `pub fn serialize_computed(s: &ComputedStyle) -> String {`

```
/// Einen gerechneten Stil als Deklarationstext ausgeben — die Antwort von
/// `getComputedStyle`.
///
/// **Was hier NICHT drin ist, und warum.** Ein Browser gibt fuer `width` den
/// BENUTZTEN Wert in px zurueck, und der entsteht erst im Layout. Diese
/// Funktion laeuft VOR dem Layout, also steht hier der angegebene Wert
/// (`auto`, `50%`). Das ist eine Teilantwort und als solche benannt — eine
/// erfundene Pixelzahl waere schlimmer, weil sie aussaehe wie eine Messung
/// ([[feedback_invented_fallback_hides_the_fault]]).
///
/// Die Liste deckt, was Seiten wirklich lesen. Was nicht daraufsteht,
/// beantwortet die leere Zeichenkette — wie jede nicht gesetzte Eigenschaft.
```

## L5038-5039 · `put("margin-top", &if s.margin_top_auto { "auto".into() } else { px(s.margin_top) });`

```
// Oben/unten sind bereits aufgeloest, links/rechts koennen `auto` sein
// (das ist die Zentrierung) — deshalb zwei verschiedene Formen.
```

## L5055-5065 · `put("position", match s.position {`

```
// **Einunddreissig von dreiundvierzig gefragten Eigenschaften kamen leer
// zurueck**, gemessen auf DuckDuckGos Ergebnisseite. Das ist keine
// Kosmetik: eine Positionierungsbibliothek fragt `position`, bevor sie
// rechnet, ein Rollbeobachter `overflow`, ein Flexhelfer `flex-grow`. Wer
// "" bekommt, nimmt den falschen Zweig — und die Seite legt sich SELBST
// falsch aus, ohne dass im Layout ein Fehler steckt.
//
// Geantwortet wird aus DEMSELBEN Feld, aus dem das Layout rechnet. Wo das
// Feld weniger weiss als CSS (aus `flex-direction: row-reverse` ist nur
// `flex_row` uebrig), steht hier, was das Layout TUT — zwei Wahrheiten
// waeren schlimmer als eine grobe.
```

## L5091-5092 · `if s.overflow_x == s.overflow_y { put("overflow", ovf(s.overflow_x)); }`

```
// Die Kurzform gibt es nur, wenn beide Achsen dasselbe sagen — so
// serialisiert ein Browser sie auch.
```

## L5124-5126 · `put("white-space", if s.nowrap { "nowrap" } else { "normal" });`

```
// `white-space` fuehrt beak als EINE Frage — bricht die Zeile um oder
// nicht. `pre` und `pre-wrap` unterscheidet das Feld nicht; hier steht,
// was das Layout tut.
```

## L5133 · `if s.grid_col_gap == s.grid_row_gap { put("gap", &len(s.grid_row_gap)); }`

```
// Die Kurzform `gap`, wenn beide Achsen dasselbe sagen — wie `overflow`.
```

## L5135 · `let r = &s.radius;`

```
// Vier Ecken; gleich grosse schreibt ein Browser als EINEN Wert.
```

## L5143 · `fn trim_f32(v: f32) -> String {`

```
/// `1.5` statt `1.5000`, `2` statt `2.0` — so schreibt ein Browser es auch.
```

## L5150 · `fn split_sides(v: &str) -> [&str; 2] {`

```
/// `<a> [<b>]` — a two-sided logical shorthand. One value applies to both.
```

## L5152 · `let t = css_tokens(v);`

```
// Klammernbewusst, aus demselben Grund wie `four_values`.
```

## L5158-5159 · `fn parse_list_style(v: &str) -> Option<ListStyle> {`

```
/// A `list-style-type` keyword, or `None` for anything we don't render as a
/// marker (`inside`/`outside`/`url(…)`/an unknown counter style).
```

## L5181-5183 · `fn set_margin_tb(v: &str, u: Units, px: &mut f32, auto: &mut bool, pct: &mut f32) {`

```
/// Top/bottom margin: `auto` computes to 0 for block boxes.
/// A top/bottom margin: the used length in normal flow, plus whether the
/// author wrote `auto`. Both are needed — see `ComputedStyle::margin_top_auto`.
```

## L5186 · `let (p, q) = if *auto { (0.0, 0.0) } else { length_parts(v, u).unwrap_or((0.0, 0.0)) };`

```
// A margin may be negative, so no sign filter here — unlike padding.
```

## L5192 · `fn margin_lr(v: &str, u: Units) -> Len {`

```
/// Left/right margin keeps `auto` (drives centering / slack).
```

## L5197-5204 · `fn parse_translate(v: &str, u: Units) -> Option<(Len, Len)> {`

```
/// A padding length. Negative is invalid (padding ≥ 0) → keeps `prior`.
/// `transform` → a translation, or `None` for anything else.
///
/// `translate(x[,y])` / `translateX(x)` / `translateY(y)` only. A rotation or a
/// scale is deliberately dropped rather than approximated: half a transform
/// moves a box to a place neither the author nor the untransformed layout
/// intended. Percentages resolve against the BOX's own size, not the containing
/// block, so they are kept as `Len::Pct` until paint.
```

## L5231-5254 · `type ShadowSet = (Option<BoxShadow>, Option<BoxShadow>, Option<BoxShadow>);`

```
/// One `box-shadow` layer: `[inset]? [<color>]? <dx> <dy> [<blur>] [<spread>]
/// [<color>]?` → `(inset, layer)`. `None` means the layer is INVALID, which is
/// not the same as "we don't paint it": `inset` is perfectly valid CSS we
/// simply have no inner-shadow paint for, so it comes back as
/// `Some((true, …))` and lets the declaration REPLACE whatever stood before.
/// Returning `None` for it left the previous shadow painted instead.
/// Lengths keep their order; the colour may sit at either end (CSS Backgrounds 3
/// §7.1). An omitted colour stays `None` = `currentColor`, resolved at paint.
/// The one layer of a `box-shadow` list we can actually paint, or `None` for a
/// list where no layer is paintable. The outer `Option` is VALIDITY: `None`
/// means some layer failed to parse, so the whole declaration is dropped and
/// the box keeps the shadow it had (CSS Syntax 3 §9 — a bad value invalidates
/// the declaration, not just the layer).
///
/// Which layer: the FIRST paintable one, not the first one. Layers paint
/// front-to-back, so the first paintable layer is also the topmost one we would
/// draw. Taking layer 1 unconditionally cost DuckDuckGo its searchbox ring —
/// `0 10px 20px …, 0 2px 6px …, 0 0 0 1px rgba(0,0,0,.08)` puts the only sharp
/// layer LAST, and the two blurred ones ahead of it are skipped at paint time.
///
/// Measured across duckduckgo.com and two Wikipedia articles: 7 declarations
/// hide their ring behind a blurred layer this way, and **no** declaration has
/// more than one paintable layer — so a list of layers would cost every
/// `ComputedStyle` copy several more words for a case real pages do not write.
```

## L5264-5268 · `let invisible = sh.color.is_some_and(|c| c.a == 0);`

```
// Alpha 0 malt nichts — so eine Schicht darf ihren Platz nicht
// belegen. Tailwind v4 fuellt jede unbenutzte Schicht seiner Liste mit
// `0 0 #0000`, und die steht VOR der echten: sie ist unscharf-frei,
// also galt sie als der scharfe Anteil, und der Ring dahinter fiel
// heraus. Ein `ring-4` malte damit nichts.
```

## L5271 · `} else if inset {`

```
// Gueltig geparst, nur ohne Wirkung — die Deklaration bleibt.
```

## L5277-5283 · `if sh.paints() {`

```
// Zwei Plaetze, weil echte Seiten zwei verschiedene Dinge unter
// demselben Namen schreiben und BEIDE sichtbar sind: DDG legt
// einen 1-px-Ring hinter zwei weiche Schichten, Bootstrap malt
// Karten mit einer einzigen weichen. Eine volle Liste waere
// gemessen unnoetig — keine der geprueften Deklarationen hat mehr
// als einen scharfen UND einen weichen Anteil —, und sie kostete
// jede `ComputedStyle`-Kopie mehrere Woerter.
```

## L5307-5309 · `let len = if is_math_fn(tok) {`

```
// `calc(4px + var(--x))` ist nach der Var-Ersetzung eine Rechnung, und
// `parse_length` kennt nur Einheiten. Ohne das ist die ganze Schicht
// ungueltig — Tailwinds Ringbreite ist IMMER eine Rechnung.
```

## L5329-5337 · `if matches!(parse_color_val(tok, &Theme::DARK), Some(ColorVal::Transparent)) {`

```
// Eine DURCHSICHTIGE Farbe ist eine gueltige Schicht, keine kaputte.
//
// `parse_color` gibt fuer `#0000` nichts zurueck, und der Zweig
// darunter erklaerte damit die ganze Deklaration fuer ungueltig.
// Tailwind v4 baut seine Schatten aus einer Liste von Platzhaltern
// (`box-shadow: var(--tw-inset-shadow), … , var(--tw-shadow)`), und
// die unbenutzten sind genau `0 0 #0000` — ein einziger davon
// loeschte den echten Schatten gleich mit. Auf einer Tailwind-Seite
// hatte damit NICHTS einen Schatten.
```

## L5342-5346 · `if matches!(parse_color_val(tok, &Theme::DARK), Some(ColorVal::CurrentColor)) {`

```
// `currentcolor` ist die Vorgabe dieser Eigenschaft (css-backgrounds-3
// §7.1) und steht hier als `None` — genau wie eine weggelassene Farbe.
// Ausgeschrieben wurde sie trotzdem abgelehnt, und das ist der Fall,
// den Tailwind schreibt: `var(--tw-ring-color, currentcolor)`. Ohne
// eine gesetzte Ringfarbe malte `ring-4` deshalb nichts.
```

## L5350-5352 · `return None;`

```
// An unknown token invalidates the layer rather than being ignored —
// otherwise a value we cannot read paints something the author never
// asked for.
```

## L5370-5379 · `fn length_parts(v: &str, u: Units) -> Option<(f32, f32)> {`

```
/// A length split into `(constant px, percent)`, so a value whose basis is not
/// known yet can be carried whole: `50%` → `(0, 50)`, `calc(10% + 5px)` →
/// `(5, 10)`, `5px` → `(5, 0)`. `None` means it is not a length at all and the
/// declaration is dropped (the side keeps what it had).
///
/// The `calc` case is measured, not parsed: every CSS math function on lengths
/// is LINEAR in its percentage, so evaluating it against a basis of 0 and of
/// 100 gives the constant and the coefficient without a second expression
/// walker. `calc(50% - 0px)` against a reference that writes plain `50%` is
/// exactly the pair `grid-calc-margin` compares.
```

## L5398-5403 · `fn set_pad(v: &str, u: Units, px: &mut f32, pct: &mut f32) {`

```
/// One padding side. A plain negative length is INVALID and keeps what the side
/// had; a value that carries a percentage is valid whatever its sign, because
/// its used value is only known once the basis is — `calc(100% - 21.5rem)` is
/// how Tailwind pads the end of a scrolling row, and it is negative only until
/// the containing block is measured. That one is clamped to zero at USE time
/// (css-values-4 §10), which is `resolve_pct_box`'s `.max(0.0)`.
```

## L5413-5416 · `fn border_width_kw(tok: &str, u: Units) -> Option<f32> {`

```
/// A border-width keyword/length → px. `thin`/`medium`/`thick` = 1/3/5px.
/// A NEGATIVE length is invalid, not zero: the declaration is dropped and the
/// side keeps the width it had (`border-top-width-012` and its siblings turn
/// on exactly that difference).
```

## L5427 · `fn set_side_width(side: &mut BorderSide, v: &str, u: Units) {`

```
/// Assign one side's border width (keeps the prior value on an invalid one).
```

## L5434-5436 · `fn parse_border_shorthand(v: &str, u: Units, theme: &Theme) -> BorderSide {`

```
/// Parse a `border`/`border-<side>` shorthand (`<width> || <style> || <color>`,
/// any order) into a side. `none`/`hidden` → no border. A specified border with
/// no explicit colour uses `currentColor` (the element's `color`).
```

## L5438-5439 · `let mut side = BorderSide::default();`

```
// A shorthand resets every side it names, so this starts from the initial
// value rather than from whatever came before.
```

## L5459-5461 · `side.specified = true;`

```
// The shorthand resets the whole side whatever it names, so writing it at
// all is taking control of the frame — `border: red` suppresses one just as
// `border: none` does.
```

## L5466 · `fn four_sides<'a>(toks: &[&'a str]) -> Option<[&'a str; 4]> {`

```
/// Expand 1–4 CSS box-side tokens into [top, right, bottom, left].
```

## L5477 · `fn four_values(v: &str) -> (&str, &str, &str, &str) {`

```
/// Expand a 1–4 token box shorthand into (top, right, bottom, left).
```

## L5479-5484 · `let p: alloc::vec::Vec<&str> = css_tokens(v);`

```
// **Klammernbewusst zerlegen.** `split_whitespace` machte aus
// `padding: calc(2 * 10px)` drei Seiten (`calc(2`, `*`, `10px)`), und
// uebrig blieb Unsinn — die Deklaration fiel weg, und der Kasten hatte
// GAR KEINE Polsterung. Tailwind schreibt jede Abstandsklasse so
// (`.p-4{padding:calc(var(--spacing) * 4)}`), also war auf einer
// Tailwind-Seite jedes `p-*`, `m-*`, `gap-*` wirkungslos.
```

## L5495-5498 · `pub struct TrackList {`

```
/// A parsed track list: `n` tracks in `tracks`, plus—if the source held a
/// `repeat(auto-fill|auto-fit, …)`—the one-copy pattern's span (`fill_start` ..
/// `+fill_len`) and `fill` kind (1 = auto-fill, 2 = auto-fit) so layout can
/// expand it to the container width.
```

## L5507-5509 · `fn parse_grid_tracks(v: &str, u: Units) -> TrackList {`

```
/// Parse a `grid-template-*` value, expanding `repeat(n, …)` and recording any
/// `repeat(auto-fill|auto-fit, …)`. `[line-name]` tokens are skipped.
/// Truncates at `MAX_GRID_COLS`.
```

## L5519 · `continue; // line name — no track`

```
// line name — no track
```

## L5532 · `fill = auto;`

```
// Store one copy; layout repeats it to fill the width.
```

## L5570-5573 · `let max_part = inner.split(',').nth(1).unwrap_or(inner).trim();`

```
// minmax(min, max): size by the MAX (min=0 lets it shrink to fit). A
// bare max length must become a Fixed CAP — not unbounded `Auto`
// max-content, which blows a `minmax(0,59.25rem)` content column up to
// the whole article's unwrapped width.
```

## L5577 · `match parse_length(t, u) {`

```
// A fixed length: px/em/rem/pt/cm/vw/… against the element's own units.
```

## L5585 · `fn split_top_level(v: &str) -> alloc::vec::Vec<alloc::string::String> {`

```
/// Split on whitespace, but keep `repeat(…)` / `minmax(…)` (parens) intact.
```

## L5614-5618 · `fn set_grid_areas(s: &mut ComputedStyle, v: &str) {`

```
/// Split a value at the top-level `/` (respecting `repeat(…)`/`minmax(…)`
/// parens), returning `(before, after)`. `None` if there is no top-level slash.
/// Parse `grid-template-areas` strings into the container's named-area map. Each
/// quoted string is a row; whitespace-separated tokens are cell names (`.` =
/// empty). An area's rectangle is the bounding box of its cells.
```

## L5662-5663 · `if s.grid_ncols == 0 {`

```
// With no explicit `grid-template-columns`, the template width defines the
// (auto-sized) column tracks.
```

## L5686 · `fn parse_line(t: &str) -> Option<i16> {`

```
/// A single grid-line spec → its integer index (`0`/`None` for `auto`/named).
```

## L5695-5696 · `fn parse_line_placement(v: &str) -> (i16, u16) {`

```
/// `grid-column`/`grid-row` → `(start_line, span)`. `start_line == 0` means
/// auto-placement. Handles `span N`, `A / B`, `A / span N`, `span N / B`, `A`.
```

## L5705 · `let start = parse_line(b).map(|bl| bl - sp as i16).unwrap_or(0);`

```
// `span N / B` → end at B, start = B − N.
```

## L5726-5729 · `fn parse_justify(v: &str) -> Justify {`

```
/// `justify-content` / the second half of `place-content`. `<overflow-position>`
/// (`safe`/`unsafe`) only says what to do when the content does not fit, which
/// never changes where it sits when it does — so the keyword is skipped and the
/// position behind it decides.
```

## L5741-5742 · `fn parse_content_align(v: &str) -> ContentAlign {`

```
/// `align-content` — the six distributions plus `stretch`, which is the
/// initial value and the one no other alignment property has.
```

## L5753-5754 · `_ => ContentAlign::Start,`

```
// `baseline` on a line-packing property falls back to `start`
// (css-align-3 §4.3), which is also where every unknown value lands.
```

## L5760-5764 · `let v = v.trim();`

```
// `<baseline-position>` is two words (`first baseline` / `last baseline`)
// and `<overflow-position>` prefixes one (`safe center`). Dropping the
// qualifier leaves the position that decides where the box goes; keeping
// the whole string made the value UNKNOWN, and an unknown `align-items`
// fell back to `stretch` — which sizes the item instead of aligning it.
```

## L5793-5794 · `fn apply_flex_shorthand(v: &str, s: &mut ComputedStyle) {`

```
/// `flex` shorthand: keywords (`none`/`auto`/`initial`) or `grow [shrink] [basis]`.
/// A bare number `flex:1` = `1 1 0` (per spec `<n> 1 0%`).
```

## L5840-5847 · `fn unitless_nonzero(v: &str) -> bool {`

```
/// Parse a CSS `<length>` to px. Supports `px`, `em`/`rem` (relative to
/// `em_base`), and bare numbers (treated as px).
/// Eine Zahl ohne Einheit und ungleich null — in CSS keine Laenge.
///
/// Nur `font-size` fragt das heute. `parse_length` selbst laesst so etwas
/// weiterhin durch, und das ist eine benannte Luecke, keine Absicht: sie zu
/// schliessen beruehrt jede Laengeneigenschaft auf einmal und gehoert
/// gemessen, nicht nebenbei erledigt.
```

## L5858-5859 · `if let Some(n) = v.strip_suffix("rem") {`

```
// Font-relative first so "rem" is matched before the "em" suffix eats it.
// `rem` is ROOT-relative (not em_base) — else nested rem compounds wrongly.
```

## L5866-5871 · `if let Some(n) = v.strip_suffix("ex") {`

```
// `ex`/`ch`. Missing here they fell through as INVALID, which is not
// "ignore the unit" but "ignore the declaration": `outline-width: 0ex`
// then left the shorthand's `medium` in place and drew a ring the page had
// just switched off. The factors are MEASURED off our own font rather than
// both guessed at 0.5 — a `ch` is the "0" advance, and at 0.5 a
// `width: 20ch` column came out 26 % too narrow.
```

## L5879 · `return n.trim().parse::<f32>().ok().map(|f| f * u.em / 100.0);`

```
// No containing measure here → treat % of em (rough; refined later).
```

## L5882-5883 · `const VP: &[(&str, fn(&Units) -> f32)] = &[`

```
// Viewport-percentage units (CSS Values 3 §5.1.2). BEFORE the absolute
// table: `vmin` ends in `in`, so the inch arm would eat it otherwise.
```

## L5895 · `const ABS: &[(&str, f32)] = &[`

```
// Absolute units → CSS reference pixels (1in = 96px, CSS Values 3 §5.2).
```

## L5913-5916 · `fn parse_color(v: &str, _theme: &Theme) -> Option<Rgba> {`

```
/// Parse a CSS `<color>`. Delegates to the full `color` module — hex
/// (#rgb/#rgba/#rrggbb/#rrggbbaa), rgb()/rgba()/hsl()/hsla(), and all 148 CSS
/// named colours. `None` keeps the inherited value (`currentcolor`/`inherit`/
/// `transparent`/unparseable), preserving the caller's contract.
```

## L5921-5924 · `fn parse_color_val(v: &str, _theme: &Theme) -> Option<ColorVal> {`

```
/// As [`parse_color`], but keeps "fully transparent" apart from "no value".
/// Use it wherever the property HAS a paint-nothing state (a border side, a
/// background); `parse_color` alone silently turns `rgba(0,0,0,0)` into the
/// inherited colour.
```

## L5929-5933 · `fn css_tokens(v: &str) -> alloc::vec::Vec<&str> {`

```
/// Split a CSS value on top-level whitespace, keeping parenthesised groups
/// (`rgb(0% 50% 0%)`, `calc(1px + 2px)`) intact as single tokens. Needed
/// because CSS function values contain internal spaces that a naive
/// `split_whitespace` would shred. Values here are ASCII, so byte slicing is
/// safe on the whitespace/paren boundaries.
```

## L5974-5975 · `fn subject(dom: &dom::Dom) -> css::ElemInfo<'_> {`

```
/// `resolve` takes the SUBJECT as an `ElemInfo` (it carries the pointer
/// state). A test that is not about `:hover` states the resting one.
```

## L5980-5983 · `fn apply_one(name: &str, val: &str, theme: &Theme, s: &mut ComputedStyle) {`

```
/// `apply_one` takes a resolved `Prop` since 0.24.1; a test states the
/// property the way a stylesheet does. Going through `prop_key` also means
/// a test naming a property that does not exist fails loudly (`Unknown`)
/// instead of quietly asserting on an untouched style.
```

## L5990-5997 · `#[test]`

```
/// Das UA-Blatt gegen HTML §15.3 — die Zahlen, nicht das Aussehen.
///
/// Gemessen an Chromium (`getComputedStyle`, 16 px Grundschrift), weil
/// eine Vorgabe, die „vernuenftig aussieht", trotzdem falsch ist: ein
/// Reftest backt sie als Literal-Pixel ein, und eine echte Seite ist
/// gegen sie gestaltet. `tools/fixtures/ua.html` faehrt dieselben
/// Elemente durch `<tools>/gallery/run.py`; dieser Test ist die billige
/// Fassung davon, die bei jedem `cargo test` mitlaeuft.
```

## L6009 · `for (tag, fs, m) in [("h1", 32.0, 21.44), ("h2", 24.0, 19.92), ("h3", 18.72, 18.72),`

```
// (Tag, Schriftgroesse, Rand oben = Rand unten) — §15.3.6.
```

## L6030 · `assert_eq!((px(s.margin_left), px(s.margin_right), s.pad_left), (40.0, 40.0, 0.0), "{tag}");`

```
// Ein RAND, keine Polsterung: ein Hintergrund faengt bei 40 px an.
```

## L6040 · `for (html, fs) in [("<p>x<small>y</small></p>", 16.0 / 1.2),`

```
// `smaller`/`larger` sind eine Stufe der Skala: /1,2 und ×1,2.
```

## L6068-6071 · `#[test]`

```
/// A CSS-wide keyword applies to EVERY property. Before this it was
/// handled property by property, so `border-bottom-color: inherit` did not
/// parse — and a failed parse leaves the PREVIOUS declaration standing,
/// which is how a rule that says "red, then inherit" painted red.
```

## L6082-6084 · `let mut parent = ComputedStyle::root(&theme);`

```
// `inherit` on a NON-inherited property: the parent's value, not the
// initial one. Declared after `red`, so it also proves the earlier
// declaration is overridden rather than left in place.
```

## L6093 · `let mut p2 = ComputedStyle::root(&theme);`

```
// `initial` on an inherited property drops the inherited value.
```

## L6100-6101 · `let sheet3 = css::parse("div { letter-spacing: unset; width: unset }");`

```
// `unset` follows the property's own inheritance: inherited for
// `letter-spacing`, initial for `width`.
```

## L6107 · `let mut p4 = ComputedStyle::root(&theme);`

```
// A longhand takes only its own half of the side.
```

## L6129-6132 · `#[test]`

```
/// `:link` / `:any-link` are how a page states its link colour. Before they
/// parsed, the whole selector was dropped and the page silently kept the UA
/// colour — and it loses to a bare `a` rule only because `a:link` is one
/// class-level step more specific, which is exactly what the drop cost us.
```

## L6147-6148 · `assert_eq!(color(link, "a:link{color:red} a{color:lime}"), red);`

```
// Specificity: `a:link` (0,1,1) beats a bare `a` (0,0,1) whatever the
// order — the reason dropping the selector was not merely a no-op.
```

## L6151 · `let anchor = "<body><a name=\"top\">x</a></body>";`

```
// An anchor with no href is not a link (Selectors 4 §8.1).
```

## L6155 · `assert_eq!(color(link, "a{color:lime} a:visited{color:red}"), lime);`

```
// We keep no history, so nothing is ever `:visited` — see `LinkSel`.
```

## L6159-6161 · `#[test]`

```
/// The four viewport-percentage units, everywhere a length is read.
/// `vmin` is the one that needs care: it ends in `in`, so the inch arm of
/// the absolute table eats it unless the viewport arms come first.
```

## L6179-6180 · `assert_eq!(st("width:calc(50vw - 20px)").width, Len::Px(480.0));`

```
// A viewport unit is a length like any other: it composes with `calc()`
// and it is a valid `font-size`, where it must NOT be read as an `em`.
```

## L6183 · `assert_eq!(st("width:1in").width, Len::Px(96.0));`

```
// Nothing above may disturb the inch/mm arms that follow it.
```

## L6187-6192 · `#[test]`

```
/// The CSS math functions have to reach the BOX MODEL, not just custom
/// properties. `values.rs` evaluated all four from the start, but
/// `parse_len_opt` only routed `calc(`, so `width: max(20px, 10px)` failed
/// its length parse and fell back to `auto`; and the padding parse called
/// `parse_length` directly, so `padding: calc(…)` was dropped entirely.
/// `length_parts` owns both now, and carries the percentage with it.
```

## L6205 · `assert_eq!(st("width:max(calc(1rem + 4px),10px)").width, Len::Px(20.0));`

```
// Nested, and mixed with the units a real page writes.
```

## L6209-6210 · `assert_eq!(st("padding-left:8px;padding-left:calc(0px - 4px)").pad_left, 8.0);`

```
// A negative padding is invalid and must leave the side alone, exactly
// as a plain negative length does.
```

## L6214-6217 · `#[test]`

```
/// `border-width` and `border-style` are independent halves and neither
/// implies the other: a width alone paints nothing, a style alone is
/// `medium`, and the colour defaults to `currentColor` however late in the
/// declaration block the `color` arrives.
```

## L6232-6233 · `assert_eq!(side("border-top-style:solid;border-top-width:-1pt").width, 3.0);`

```
// An invalid width leaves the specified one alone — it does not fall
// back to 0, which is what `border-top-width-012` checks.
```

## L6238-6239 · `let t = side("border-top:1px solid #f00;border-top-color:transparent");`

```
// `transparent` is a VALUE: the width stays, nothing paints, and it is
// not the same as leaving the colour unset (that means currentColor).
```

## L6244 · `assert_eq!(`

```
// …and a real colour after it wins back.
```

## L6269 · `let a = resolve(&subject(&dom), &root, &theme, &sheet, &[], &[], 0, 1000.0);`

```
// 1st <p>: author sets red+bold, inline overrides colour to green.
```

## L6273 · `let p2 = match &dom.body().children[1] {`

```
// 2nd <p>: author red+bold, no inline.
```

## L6287 · `let dom = dom::parse("<body><p id=\"x\" class=\"b\">x</p></body>");`

```
// !important on a low-specificity class beats a higher-specificity #id.
```

## L6292 · `let dom2 = dom::parse("<body><p class=\"b\" style=\"color:#ff0000\">x</p></body>");`

```
// author !important beats a normal inline style.
```

## L6297 · `let dom3 = dom::parse("<body><p class=\"b\">x</p></body>");`

```
// a later normal declaration must NOT override an earlier !important.
```

## L6311-6312 · `fn find<'a>(el: &'a Element, tag: &str) -> Option<&'a Element> {`

```
// Find by tag, not by position: the parser implies <html>/<body>
// around the fragment (HTML Standard §13.2.6).
```

## L6332-6334 · `#[test]`

```
/// The value arrives at `apply_one` lowercased for keyword matching — a
/// `data:` URI must NOT be taken from that copy, or its base64 payload is
/// silently corrupted.
```

## L6349 · `#[test]`

```
/// A `data:` URI is full of commas; the layer split must not cut it.
```

## L6370-6371 · `#[test]`

```
/// A keyword binds to its own axis whatever the order — `center right`
/// means x=right, y=center (css-backgrounds-3 §3.6).
```

## L6380 · `let before = st.bg_layer.pos;`

```
// Two horizontal keywords are not a position at all → declaration dropped.
```

## L6399-6400 · `#[test]`

```
/// The form the icon systems ship: one shorthand carrying url, position,
/// size and repeat, with an unspaced slash.
```

## L6412-6413 · `#[test]`

```
/// Ein Verlauf ist kein `url()` — er darf keinen Bildschluessel setzen,
/// sonst suchte der Rasterer Bytes, die es nie geben wird.
```

## L6433 · `assert_eq!(parse_gradient("linear-gradient(red, blue)", &theme).angle, 180.0);`

```
// Vorgaberichtung ist „nach unten", nicht „nach rechts".
```

## L6438-6439 · `#[test]`

```
/// Offene Lagen werden gleichmaessig verteilt — aber ERST beim Malen,
/// weil eine px-Lage bis dahin keine Zahl auf der Achse ist.
```

## L6451 · `assert_eq!(g.resolved(80.0).stops()[1].pos, 0.5);`

```
// Derselbe Stil an einem anderen Kasten: eine andere Lage.
```

## L6462 · `assert_eq!(g.at(-3.0), Rgba::opaque(Rgb(0, 0, 0)));`

```
// Ausserhalb der Achse wird der Randstopp gehalten.
```

## L6467-6468 · `#[test]`

```
/// `transparent` ist `rgba(0,0,0,0)`. Ohne vormultipliziertes Mischen
/// liefe der Verlauf durch Schwarz statt einfach auszublenden.
```

## L6488-6490 · `#[test]`

```
/// `conic-gradient` wird ehrlich abgelehnt statt als linearer gemalt.
/// Tailwind v4 schreibt jeden Verlauf mit Mischraum. Ohne den Zweig
/// dafuer fiel der erste Parameter durch und der Verlauf ganz weg.
```

## L6506-6507 · `#[test]`

```
/// Die Ecke ist kein fester Winkel: die Achse steht senkrecht auf der
/// Verbindung der beiden NACHBARecken, haengt also am Kasten.
```

## L6513 · `assert!((g.angle_for(800.0, 60.0) - 85.7).abs() < 0.2, "{}", g.angle_for(800.0, 60.0));`

```
// Ein breiter flacher Kasten: fast waagrecht.
```

## L6515 · `assert!((g.angle_for(60.0, 200.0) - 16.7).abs() < 0.2, "{}", g.angle_for(60.0, 200.0));`

```
// Ein hoher schmaler: fast senkrecht.
```

## L6517 · `assert!((g.angle_for(100.0, 100.0) - 45.0).abs() < 0.01);`

```
// Quadratisch: die 45 Grad, die die Naeherung immer nahm.
```

## L6519 · `let g = parse_gradient("linear-gradient(to right, red, blue)", &theme);`

```
// Eine Seite bleibt eine Seite.
```

## L6525-6526 · `#[test]`

```
/// `background: <farbe> <verlauf>` — die Kurzform stellt die Farbe voran,
/// und der Verlauf ging dabei still verloren.
```

## L6536-6537 · `#[test]`

```
/// Ein Vorspann in fremder Schreibweise darf den Standardwert nicht
/// loeschen — und in der anderen Reihenfolge auch nicht.
```

## L6558-6562 · `pub fn family_hash(list: &str) -> u32 {`

```
/// Der Streuwert der ERSTEN Familie einer `font-family`-Liste.
///
/// Nur die erste: eine Ersatzkette wie `"Foo", Arial, sans-serif` sagt „nimm
/// Foo, wenn du es hast". Hat beak `Foo` nicht, faellt es ohnehin auf seine
/// eingebaute Schrift zurueck — und die IST der Rest der Kette.
```

## L6567 · `let low = first.to_ascii_lowercase();`

```
// Gattungsnamen sind keine Familien, sie sind die Ersatzkette selbst.
```

## L6577 · `pub fn hash_name(low: &str) -> u32 {`

```
/// FNV-1a ueber den kleingeschriebenen Namen. 0 bleibt fuer „keine" frei.
```

## L6589-6594 · `#[test]`

```
/// Der Preis der Verlaeufe, festgenagelt statt geschaetzt.
///
/// `ComputedStyle` wird je Element kopiert und gemerkt; waechst sie
/// unbemerkt weiter, zahlt das jede Seite. Faellt dieser Test, ist das
/// keine Regression — es ist die Frage, ob das neue Feld seinen Platz
/// wert ist.
```

## L6599-6609 · `assert_eq!(core::mem::size_of::<super::ComputedStyle>(), 1504);`

```
// 1472 -> 1496: sechs `f32` fuer die Prozentanteile von `padding` und
// den senkrechten `margin`s. Der Platz ist es wert — ohne sie fielen
// ALLE VIER Polsterungen in Prozent auf null, und `padding-top: 56.25%`
// ist die Art, wie das Web ein 16:9-Kaestchen reserviert.
//
// 1496 -> 1504: `outline_set`. EIN bool, und es kostet acht Bytes,
// weil es hinter dem letzten `f32` keine Luecke mehr gibt. Der Platz
// ist es wert: ohne ihn kann beak „die Seite will keinen Fokusring"
// nicht von „die Seite hat nichts gesagt" unterscheiden, und dann
// faerbt der Fokus den Rahmen der Seite um — auf DuckDuckGos
// Suchfeld sah das aus wie ein Fehler und war einer.
```

