# `tools/wasm/beak-engine/src/raster.rs` @ 5e0102684

## L1-7 · `use alloc::vec::Vec;`

```
//! Rasteriser — draws a `Layout`'s display list into a BGRA pixel buffer.
//!
//! Glyphs come from fontdue (Inter, the same font the compositor uses); the
//! layout is ours. `paint` renders the visible slice at a scroll offset, so
//! the buffer stays viewport-sized regardless of document length. Pure — the
//! same code paints on nopeekOS (into a `Widget::Canvas`) and on the desktop
//! adapter (into a window framebuffer), see docs/spec/BROWSER.md §10.
```

## L20 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// What a pointer move costs — the answer `Engine::set_hover` gives.
```

## L23 · `Unchanged,`

```
/// The pointer is inside exactly the same elements as before.
```

## L25-27 · `Changed { paint_only: bool },`

```
/// The state changed. `paint_only` means no rule that can gain or lose
/// here declares anything that moves a box, so the geometry of the whole
/// page is unchanged and only the look of those elements differs.
```

## L32 · `pub fn is_changed(self) -> bool {`

```
/// Did anything change at all?
```

## L39-40 · `fonts: core::cell::RefCell<Fonts>,`

```
/// Die Schriften. `RefCell`, weil eine Seite ihre eigenen erst NACH dem
/// ersten Auslegen mitbringt und `layout` nur `&self` hat.
```

## L42-45 · `pending_fonts: core::cell::RefCell<alloc::vec::Vec<(alloc::string::String, u32, u16, bool)>>,`

```
/// Schriften, die die Seite verlangt und die noch fehlen: `(Adresse,
/// Familie, Gewicht, kursiv)`. Der Wirt holt sie und meldet sich mit
/// `add_font` zurueck — dieselbe Runde wie beim Modulgraphen und den
/// nachgeladenen Stilblaettern.
```

## L47-54 · `img_miss: core::cell::RefCell<Option<alloc::string::String>>,`

```
/// Die Adresse, unter der das MALEN zuletzt vergeblich nach Pixeln suchte.
///
/// **Der Platzhalter ist stumm**, und damit sieht „nie angefragt" genauso
/// aus wie „geholt, dekodiert, und beim Malen unter einem anderen
/// Schluessel gesucht". Auf DuckDuckGos Trefferliste stand im Log
/// „4 von 4 dekodiert" und auf dem Schirm blieben die Kaestchen leer —
/// die Frage, mit welcher Zeichenkette gesucht wurde, konnte niemand
/// beantworten ([[feedback_the_fast_path_must_say_it_ran]]).
```

## L56-59 · `inline_fonts: core::cell::RefCell<alloc::vec::Vec<(alloc::string::String, u32, u16, bool)>>,`

```
/// Gesichter, deren Bytes SCHON dastehen — ein `@font-face` mit
/// `data:`-Adresse. Sie warten hier, weil `note_font_faces` unter einem
/// gehaltenen `self.sheet` laeuft und `add_font` es leert; `load_inline_fonts`
/// nimmt sie, wenn nichts mehr entliehen ist.
```

## L61 · `asked_fonts: core::cell::RefCell<alloc::vec::Vec<alloc::string::String>>,`

```
/// Adressen, die schon angefragt wurden — sonst fragt jedes Auslegen neu.
```

## L63-65 · `glyphs: RefCell<HashMap<(u32, u32, u32), (Metrics, Vec<u8>)>>,`

```
/// Rasterised-glyph cache keyed by (char, size-bits, face-id). fontdue's
/// rasterise is not free; without this every glyph is re-rasterised every
/// frame, which makes scrolling lag. Bounded by the glyph set the page uses.
```

## L67 · `theme: Theme,`

```
/// Page colours (theme-resolved by the shell; dark until then).
```

## L69-72 · `images: RefCell<crate::image::ImageMap>,`

```
/// Decoded page images keyed by `<img src>`. Fetched ones are handed in by
/// the shell each nav; a `data:` src carries its own bytes and is decoded
/// during layout — the same two clocks `css_images` runs on, which is why
/// this is a `RefCell` and not a plain map.
```

## L74-78 · `css_images: RefCell<HashMap<u64, alloc::rc::Rc<crate::image::Image>>>,`

```
/// Decoded CSS images (`background-image`/`mask-image`) keyed by
/// `css::url_key`. Separate from `images` so a page's `<img src="x">` and
/// a stylesheet's `url(x)` cannot collide, and because these are resolved
/// on a different clock: `data:` URIs decode during layout, fetched ones
/// arrive later from the shell.
```

## L80-81 · `css_img_budget: core::cell::Cell<usize>,`

```
/// Decoded-BGRA budget for CSS images, separate from `img_budget` so a
/// page full of icons cannot starve its `<img>`s (or the reverse).
```

## L83 · `img_budget: core::cell::Cell<usize>,`

```
/// Remaining decoded-BGRA budget for the current page (streaming decode).
```

## L85 · `#[allow(clippy::type_complexity)]`

```
/// Auswahl und Fundstellen — siehe `set_marks`.
```

## L89-99 · `img_cache: RefCell<Vec<(alloc::string::String, alloc::rc::Rc<crate::image::Image>)>>,`

```
/// Decoded `<img>` pixels kept ACROSS navigations, keyed by the RESOLVED
/// url and oldest-first.
///
/// Keyed by url and NOT by the `src` attribute, which is what `images` uses
/// — `/logo.png` is a different picture on a different host, and a cache
/// that confused the two would show one site's image on another's page.
///
/// `Rc` is what makes it cheap: an image the next page uses again costs
/// its pixels ONCE, shared between this cache and the page map. Only
/// pictures no live page holds are paid for twice, and `IMG_CACHE_BUDGET`
/// bounds those.
```

## L101 · `img_cache_bytes: core::cell::Cell<usize>,`

```
/// Bytes of BGRA currently in `img_cache`.
```

## L103-105 · `css_cache: RefCell<Vec<(alloc::string::String, alloc::rc::Rc<crate::image::Image>)>>,`

```
/// The same cache for `background-image`/`mask-image` layers. Separate
/// from `img_cache` for the reason `css_images` is separate from `images`:
/// a page's `<img src=x>` and a sheet's `url(x)` must not collide.
```

## L108-113 · `viewport_h: core::cell::Cell<u32>,`

```
/// Viewport height (px) — the initial containing block's height, which
/// `top`/`bottom`/`height` percentages on root-level absolutely positioned
/// boxes resolve against (CSS 2.1 §10.1). Device state like `theme`, not
/// page content, so it lives here rather than in every layout signature.
/// `Cell` for the same reason `glyphs` is a `RefCell`: the shell holds the
/// engine by shared reference across a frame.
```

## L115-117 · `inspect: core::cell::Cell<bool>,`

```
/// When set, `layout` records an `InspectBox` per element box (the dev
/// tool). Off by default so the label-formatting cost is only paid while the
/// user is inspecting; the shell toggles it and re-lays-out.
```

## L119-121 · `hover_prev: RefCell<Vec<u32>>,`

```
/// The pointer state the LAST layout was made with. `repaint_hover` needs
/// both: the style a box is painted with now, and the one it should be
/// painted with next.
```

## L123-125 · `hover: RefCell<Vec<u32>>,`

```
/// `seq`s of the elements the pointer is inside, ascending — what `:hover`
/// reads. Device state like `theme`, not page content, so it lives here
/// rather than in every layout signature.
```

## L127-129 · `clock: core::cell::Cell<Option<fn() -> u64>>,`

```
/// A tick source lent by the shell, so a layout can report what each phase
/// cost on the machine that is actually slow. `None` on the host, where
/// `tests/diag.rs` times the phases from outside.
```

## L131-132 · `caret_on: core::cell::Cell<bool>,`

```
/// Ist der Schreibzeiger in DIESEM Bild sichtbar? Der Wirt kippt es im
/// Takt; das Layout bleibt dabei stehen.
```

## L134-141 · `last_forms: core::cell::RefCell<crate::forms::FormState>,`

```
/// Der Steuerelement-Zustand des letzten Auslegens.
///
/// **Fuer das Neuauslegen auf Verlangen.** Der Haken des Wirts ist ein
/// `fn`-Zeiger und faengt nichts ein; die getippten Werte liegen aber in
/// einer Variablen der Bildschleife. Mit `FormState::default()`
/// auszulegen waere falsch — ein Feld mit Text ist breiter als ein leeres,
/// und die Kaesten gingen an die Seite zurueck. Also merkt sich die
/// Engine, womit sie zuletzt gerufen wurde.
```

## L143 · `repaint_bail: core::cell::Cell<&'static str>,`

```
/// Why the last pointer repaint gave up — see `Engine::repaint_bail`.
```

## L145-166 · `dom: RefCell<Vec<(u64, crate::dom::Dom)>>,`

```
/// The last parsed DOCUMENT with the fingerprint of the inputs that built
/// it. Parsing a real page is ~170 ms on the device, and a page is laid out
/// several times over its life from unchanged bytes — an image landing, a
/// form key, the pointer entering a link. Every one of those re-parsed the
/// whole HTML for nothing.
///
/// The width and the palette are part of the identity because
/// `picture::resolve` BAKES the winning `srcset` candidate into the tree:
/// the same bytes at a different width are a different document.
/// Parsed documents, most-recently-used FIRST — **index 0 is the current
/// page** and every reader below takes that one.
///
/// More than one slot because going back is the most common navigation
/// there is, and the DOM the reader is returning to was thrown away by the
/// page in between. Measured on the device: revisiting an article whose
/// bytes had not changed cost 110 ms parse + 710 ms cascade a second time,
/// purely because one other page had been visited.
///
/// Keyed by content (plus width and theme), so it can only ever hand back
/// a document identical to the one that would have been parsed. A page
/// that answers differently every request — a live front page — misses by
/// construction, and that is correct rather than unfortunate.
```

## L168-173 · `scripted: RefCell<Option<crate::dom::Dom>>,`

```
/// Ein vom SKRIPT veraenderter Baum. Ist er gesetzt, wird nicht geparst
/// und nicht zwischengespeichert — er IST das Dokument.
///
/// So herum, weil der Zwischenspeicher auf dem HTML-Fingerabdruck sitzt:
/// derselbe Quelltext, aber ein anderer Baum, und der Abdruck wuesste
/// nichts davon.
```

## L175-177 · `scripted_gen: core::cell::Cell<u64>,`

```
/// Zaehlt hoch, sobald ein Skript den Baum veraendert hat. Geht in den
/// SCHLUESSEL des Stilblatts ein — ein Skript kann ein `<style>`
/// hinzufuegen, und dann waere das zwischengespeicherte Blatt falsch.
```

## L179-185 · `hit_all: core::cell::Cell<bool>,`

```
/// Fuer JEDES Element einen Treffer-Kasten aufzeichnen, nicht nur fuer die
/// mit `:hover`-Regeln.
///
/// beak schaltet das ein, sobald eine Seite Ereignisbehandler angemeldet
/// hat: ohne einen Kasten je Element gibt es keinen Weg vom Klickpunkt zum
/// Knoten. Aus, solange keiner da ist — die Liste ist ein `push` je
/// Element, und dieser Pfad wurde einmal gemessen und gekuerzt.
```

## L187-192 · `sheet: RefCell<Vec<(u64, crate::css::Stylesheet)>>,`

```
/// The last parsed stylesheet with the fingerprint of the inputs that built
/// it. Parsing a real page's CSS is a third of a layout, and a page is laid
/// out several times over its life (images landing, a form key, a resize)
/// from unchanged bytes — so the parse is repeated for nothing.
/// Collected stylesheets, same shape and same rule as `dom`: index 0 is
/// the current page's.
```

## L194-196 · `docs_parsed: core::cell::Cell<u64>,`

```
/// How often a document was really parsed, and a sheet really collected.
/// The point of the slots above is that these stop counting on a revisit,
/// and a time is too noisy to assert on.
```

## L201-203 · `fn fingerprint(b: &[u8]) -> u64 {`

```
/// Cheap content fingerprint (FNV-1a over 8-byte words). Identity by pointer
/// would be wrong here: the shell parses into one static buffer, so a different
/// document can land at the same address with the same length.
```

## L224-229 · `fn cache_put(`

```
/// Insert into a cross-navigation cache, oldest-first, evicting from the front
/// until the budget holds.
///
/// An evicted entry's pixels stay alive as long as a page still references them
/// (`Rc`) — eviction bounds the CACHE, not the page. Both caches share this one
/// routine and one budget, so they cannot drift apart.
```

## L239 · `return; // one picture may not evict the whole cache for itself`

```
// one picture may not evict the whole cache for itself
```

## L260-265 · `const DOC_SLOTS: usize = 3;`

```
/// How many parsed documents (and stylesheets) to keep.
///
/// Three, not more: a `Dom` plus its `Stylesheet` for a real article is
/// megabytes, and this shares a 128 MB heap with a 24 MB page image budget and
/// an 8 MB image cache. Three covers what it is for — the page you are on, the
/// one you came from, and the one before that.
```

## L268-283 · `fn current_dom<'a>(`

```
/// Move the entry keyed `key` to the front and say whether it was there.
///
/// Front means current: every reader takes index 0, so a hit has to be
/// promoted and not merely found.
/// Der Baum, aus dem das LETZTE Layout gebaut wurde.
///
/// **Der Grund, warum das eine eigene Funktion ist:** `layout_forms` umgeht den
/// Parse-Zwischenspeicher, sobald ein Skript einen Baum zurueckgeschrieben hat
/// (`let dom_hit = scripted || promote(…)`). Damit bleibt `self.dom` auf jeder
/// Seite mit Skripten LEER — und beide Schnellwege, die ihn lasen, gaben
/// stillschweigend auf. Der Hover-Weg (gemessen 0,16 ms gegen 24 ms Layout)
/// und der Steuerelement-Weg liefen auf keiner echten Seite.
///
/// Gemerkt hat es niemand, weil der eine Weg nichts sagte und der andere
/// seinen Grund nur im Fehlerfall meldete
/// ([[feedback-the-fast-path-must-say-it-ran]]).
```

## L307-308 · `pub fn new() -> Engine {`

```
/// Parse the embedded font faces. Cheap enough to build once and reuse
/// across page loads (the shell keeps one `Engine`).
```

## L327-328 · `viewport_h: core::cell::Cell::new(600),`

```
// 600 keeps the historical behaviour of the reftest canvas for any
// caller that never sets it.
```

## L347 · `pub fn set_viewport_h(&self, h: u32) {`

```
/// Tell the engine how tall the viewport is (see `viewport_h`).
```

## L354-355 · `pub fn set_inspect(&self, on: bool) {`

```
/// Enable/disable the inspect dev tool. When on, the next `layout` records
/// an element box per node into `Layout::inspect`; the shell re-lays-out.
```

## L360-363 · `pub fn set_hover(&self, seqs: Vec<u32>) -> HoverChange {`

```
/// Tell the engine which elements the pointer is inside — `Layout::hover_at`
/// produces the list from the previous layout. Says what the change COSTS:
/// a pointer that stayed in the same elements must cost nothing, and one
/// that entered something which only recolours must not cost a layout.
```

## L369-370 · `let moved: Vec<u32> = cur`

```
// Only the elements that GAINED or LOST the state can restyle; one that
// is in both lists is unaffected by the move.
```

## L382-387 · `fn hover_is_paint_only(&self, moved: &[u32]) -> bool {`

```
/// Can the elements whose pointer state just changed only be REPAINTED?
///
/// True when no `:hover` rule that could gain or lose on any of them
/// declares a property that moves something. Conservative in the direction
/// that matters: an unknown answer is "no", which costs a layout we might
/// not have needed — never a stale page.
```

## L391 · `if sheet.hover_layout_set.is_empty() {`

```
// Nothing on the page styles geometry on hover — no lookup needed.
```

## L413-425 · `pub fn repaint_controls(&self, lay: &mut Layout, state: &crate::forms::FormState) -> bool {`

```
/// Answer a paint-only pointer change by patching the display list, with
/// no parse, no cascade over the page and no box arithmetic.
///
/// Only call it after `set_hover` said `paint_only`. `false` means the
/// patch could not be made with certainty and the caller must lay out —
/// the layout it was given is then untouched, because every change is
/// applied only once all of them are known to be possible.
/// Ein Steuerelement neu malen statt die Seite auszulegen. `false` heisst:
/// es geht nicht, legt aus.
///
/// Der Anrufer ist jedes Ereignis, das nur den Zustand eines Kastens
/// aendert — Tastendruck im Feld, Fokus, Fokusverlust, Haekchen. Auf
/// Wikipedia kostete jedes davon 280 ms.
```

## L438-439 · `let may_restyle = |seq: u32| {`

```
// Nur eine `:checked`-Regel kann durch die Kaskade etwas anderes
// umstylen; `:focus` und Verwandte matchen bei uns nie.
```

## L453 · `find(&dom.root, seq).map_or(true, |el| sheet.checked_set.may_match(el))`

```
// Kein Element gefunden heisst: nicht entscheidbar, also auslegen.
```

## L475-478 · `pub fn repaint_bail(&self) -> &'static str {`

```
/// Why the last `repaint_hover` handed the page to a layout. `""` when the
/// last one succeeded. Worth saying ONCE per page on the device: a browser
/// that quietly lays out on every pointer move looks like the feature was
/// never built ([[feedback-log-the-exception-not-the-rule]]).
```

## L503-504 · `if !sheet.hover_sideways_set.is_empty() {`

```
// A rule that restyles a sibling of the element the pointer is in is
// out of reach for a subtree walk.
```

## L525-527 · `const SUBTREE_CAP: usize = 128;`

```
// Bounded on purpose: repainting an element at a time only beats a
// layout while the subtree is small. `nav:hover` over a menu of a
// few dozen items is worth it; the same rule on `<body>` is not.
```

## L537-538 · `if kids_off.len() != kids_on.len() {`

```
// The two descents visit the same tree in the same order, so a
// length mismatch would mean one of them stopped early.
```

## L557-558 · `pub fn revert_hover(&self) {`

```
/// Put the pointer state back to what the last layout was made with —
/// for a change that could be neither repainted nor afforded.
```

## L564-565 · `pub fn page_has_hover(&self) -> bool {`

```
/// Does the current page style anything on `:hover`? False means pointer
/// movement can be ignored outright — no hit-test, no layout.
```

## L570-581 · `pub fn toggle_details(&self, seq: u32) -> bool {`

```
/// Open/close the `<details>` owning the `<summary>` at `seq`
/// (`Layout::hit_toggle` names it). Returns whether anything changed — the
/// caller re-lays-out only then.
///
/// This EDITS the cached document: the `open` content attribute is what
/// the state actually is, so `details[open] > summary` in the page's own
/// stylesheet gets the right answer for free — rustdoc and MDN both style
/// the open state that way. Keeping the state beside the DOM instead would
/// have meant two truths and one of them invisible to the cascade.
///
/// It lives as long as the parsed document does: navigating away and back
/// re-parses and starts closed again, which is what a browser does too.
```

## L585 · `fn parent_of(el: &mut crate::dom::Element, seq: u32) -> Option<&mut crate::dom::Element> {`

```
/// The element whose child subtree holds `seq`.
```

## L618-619 · `pub fn set_theme(&mut self, theme: Theme) {`

```
/// Set the page colours (the shell resolves these from the compositor
/// palette so the page follows light/dark).
```

## L624-626 · `pub fn theme(&self) -> Theme {`

```
/// Die Farben, mit denen die Engine kaskadiert. `getComputedStyle` muss
/// dieselben nehmen, sonst antwortet es ueber eine andere Seite als die
/// gemalte.
```

## L631-638 · `pub fn image_miss(&self) -> Option<(alloc::string::String, usize)> {`

```
/// Start a fresh page's image set: clear the previous decode + reset the
/// per-page budget. The shell then fetches + `add_image`s each `<img>` ONE
/// AT A TIME (streaming) so the compressed bytes never pile up — decode the
/// image, keep only its pixels, reuse the same fetch scratch for the next.
/// `images_begin` clears the PAGE map, never the cross-navigation cache —
/// that is the whole point of the cache surviving a navigation.
/// Wonach das letzte Malen vergeblich suchte, und wie viele Bilder der
/// Speicher haelt. Der Wirt fragt danach, wenn ein Kasten leer bleibt.
```

## L647-651 · `self.glyphs.get_mut().clear();`

```
// Drop the previous page's rasterised glyphs. The cache is keyed by
// (char, size, face) and never evicts, so without this it grows across
// every navigation (and every distinct font size) until the heap OOMs.
// Bounding it to one page's working set costs only a lazy re-rasterise
// of the visible glyphs on the first paint after a nav.
```

## L655-659 · `pub fn add_image(&mut self, src: &str, bytes: &[u8]) -> Result<(), crate::image::Reject> {`

```
/// Decode ONE image and store it under `src`. The compressed `bytes` are
/// borrowed (dropped by the caller right after) — only the decoded pixels
/// are retained. A rejection names ITSELF (`Reject`) rather than being one
/// `false` for two opposite failures; the box keeps its placeholder either
/// way, but only the caller can say which one to put in the log.
```

## L664-666 · `pub fn add_image_cached(&mut self, src: &str, url: &str, bytes: &[u8])`

```
/// As [`Self::add_image`], and also keep the pixels under `url` for the
/// next navigation. The shell resolves the url; the engine never sees a
/// base to resolve against.
```

## L676-681 · `pub fn adopt_cached(&mut self, pairs: &[(alloc::string::String, alloc::string::String)])`

```
/// Serve `pairs` of `(src, url)` from the cross-navigation cache.
///
/// Returns the `src`s that were served — the shell drops those from its
/// fetch queue. Called BEFORE the first layout, which is where it pays
/// twice: no request, no decode, and the box is definite on the first
/// layout instead of being guessed and moving the page later.
```

## L691-692 · `let n = img.bgra.len();`

```
// Charged against the page budget like any other image: the
// pixels are live for this page whether or not they are shared.
```

## L705-708 · `fn cache_put(&mut self, url: &str, img: alloc::rc::Rc<crate::image::Image>) {`

```
/// Insert into the cross-navigation cache, oldest-first, evicting from the
/// front until the budget holds. An evicted entry's pixels stay alive as
/// long as a page still references them (`Rc`) — eviction bounds the
/// CACHE, not the page.
```

## L714-720 · `pub fn set_scripted_dom(&self, dom: Option<crate::dom::Dom>) {`

```
/// How many documents this engine has actually parsed, and how many
/// stylesheets it has actually collected, since it was created.
/// Den vom Skript veraenderten Baum einreichen. Ab jetzt legt das Layout
/// IHN aus, nicht mehr das geparste HTML.
///
/// `None` nimmt das zurueck — bei einer Navigation muss das passieren,
/// sonst zeigt die naechste Seite den Baum der vorigen.
```

## L728-742 · `pub fn image_srcs_now(&self, html: &str, width: u32) -> alloc::vec::Vec<alloc::string::String> {`

```
/// Die Bildadressen aus dem Baum, den das LAYOUT benutzt.
///
/// **Die Bildsammlung las das urspruengliche HTML** — auf einer Seite, die
/// ihren Inhalt per Skript baut, steht dort nichts. DuckDuckGos
/// Ergebnisseite ist eine Huelle, die React fuellt: die Karte im
/// Wissenskasten und die Seitensymbole der Treffer kamen deshalb nie auch
/// nur zur ANFRAGE, und im Geraetelog stand keine einzige Zeile zu ihrem
/// Wirt
/// ([[feedback_the_second_engine_only_runs_where_the_first_one_called]]).
///
/// Der Skriptbaum wird so gelesen, wie das Layout ihn liest — ohne
/// `picture::resolve`, denn das laeuft auf ihm auch dort nicht (eine
/// eigene, benannte Luecke: ein per Skript eingehaengtes `<picture>`
/// waehlt seinen Kandidaten nicht). Ohne Skriptbaum ist es Zeichen fuer
/// Zeichen die alte Antwort.
```

## L755-759 · `fn note_font_faces(&self, sheet: &crate::css::Stylesheet) {`

```
/// Welche `@font-face`-Schriften die Seite verlangt und noch nicht hat.
///
/// Nur die ERSTE Quelle je Gesicht: die Liste ist die Rangfolge der Seite,
/// und beak liest WOFF2 und rohes sfnt — die erste Angabe ist praktisch
/// immer WOFF2.
```

## L765-769 · `let list = if url.starts_with("data:") || url.starts_with("DATA:") {`

```
// **Ein `data:`-Gesicht braucht kein Netz** — seine Bytes stehen im
// Blatt. Dem Wirt gegeben hiesse, `data:application` als
// RECHNERnamen aufzuloesen: die Anfrage scheitert, das Gesicht
// fehlt, und ein Stueck der Adresse liegt beim Aufloeser. Bei
// CSS-Bildern loest die Engine sie seit je selbst auf.
```

## L779-780 · `pub fn take_pending_fonts(&self) -> alloc::vec::Vec<(alloc::string::String, u32, u16, bool)> {`

```
/// Was der Wirt holen soll. Leert die Liste — jede Adresse wird einmal
/// angefragt.
```

## L785-790 · `pub fn load_inline_fonts(&self) -> bool {`

```
/// Die `data:`-Gesichter dekodieren und aufnehmen. Liefert true, wenn
/// eines dazukam — dann muss neu ausgelegt werden, wie bei einer geholten
/// Schrift auch.
///
/// Der Wirt ruft das, WEIL hier nichts mehr entliehen ist: `add_font`
/// leert die Blatt- und Baumzwischenspeicher.
```

## L803-811 · `pub fn loaded_faces(&self) -> usize {`

```
/// Eine geholte Schrift aufnehmen. `bytes` darf WOFF2 oder rohes sfnt
/// sein; alles andere wird abgelehnt, statt als kaputte Schrift zu enden.
///
/// Liefert false, wenn die Bytes nicht lesbar waren — der Wirt meldet das.
/// Wieviele der sechs eingebauten Gesichter bisher geparst wurden.
///
/// Sie werden FAUL geladen (siehe `fonts::LazyFace`), und „faul" ist ohne
/// diese Zahl eine Behauptung: eine Seite, die doch alle sechs anfasst,
/// spart nichts, und man saehe es nicht.
```

## L827 · `self.sheet.borrow_mut().clear();`

```
// Alles, was mit der alten Schrift gemessen wurde, ist ueberholt.
```

## L835-838 · `pub fn text_pos_at(&self, lay: &Layout, x: i32, y: i32) -> Option<crate::select::TextPos> {`

```
// ── Text auf der Seite markieren ────────────────────────────────────
//
// Die Rechnung steht in `select.rs`; hier ist sie nur an die Schriften
// angeschlossen, denn ohne Gesicht gibt es keine Textbreite.
```

## L840 · `pub fn text_pos_at(&self, lay: &Layout, x: i32, y: i32) -> Option<crate::select::TextPos> {`

```
/// Der Ort im Text unter einem Punkt in Dokumentkoordinaten.
```

## L845 · `pub fn selection_rects(&self, lay: &Layout, a: crate::select::TextPos,`

```
/// Die Rechtecke, die einen Bereich hervorheben.
```

## L851 · `pub fn selected_text(&self, lay: &Layout, a: crate::select::TextPos,`

```
/// Was in diesem Bereich steht.
```

## L857 · `pub fn find_all(&self, lay: &Layout, needle: &str)`

```
/// Alle Fundstellen von `needle` auf der Seite.
```

## L863-868 · `pub fn set_marks(&self, sel: Option<(crate::select::TextPos, crate::select::TextPos)>,`

```
/// Was hervorgehoben wird, wenn als naechstes gemalt wird.
///
/// Der erste Bereich ist die AUSWAHL, die weiteren sind Fundstellen einer
/// Suche — beide werden gleich gemalt, nur in verschiedenen Farben, und
/// beide sind Zustand des Wirts, nicht des Layouts: ein Neuauslegen darf
/// eine Markierung nicht loeschen.
```

## L874-875 · `#[cfg(test)]`

```
/// Die Schriften, geliehen — nur fuer die Proben in `select.rs`, die
/// dieselbe Messung fahren muessen wie der Motor.
```

## L881-885 · `pub fn scripted_gen(&self) -> u64 { self.scripted_gen.get() }`

```
/// Wie oft der Baum seit dem Start durch Skripte ersetzt wurde.
///
/// Der Wirt braucht die Zahl, um sein FORMULARMODELL nachzuziehen: eine
/// Seite, die ihre Maske erst per Skript baut, hat sonst Steuerelemente
/// im Bild, die es fuer `submit` gar nicht gibt.
```

## L888-889 · `pub fn with_scripted<R>(&self, f: impl FnOnce(&crate::dom::Dom) -> R) -> Option<R> {`

```
/// Etwas auf dem lebenden Baum ausrechnen — ohne ihn herauszugeben, denn
/// er gehoert dem Motor.
```

## L894-902 · `pub fn title(&self) -> Option<alloc::string::String> {`

```
/// Der `<title>` des Dokuments, aus dem zuletzt ausgelegt wurde.
///
/// **Aus dem BAUM, den der Motor ohnehin haelt** — nicht aus einem
/// zweiten Parse: `current_dom` gibt den Baum des letzten Layouts, und
/// das ist der geskriptete, sobald es einen gibt. Damit sieht ein
/// Tabstreifen auch ein `document.title = …`.
///
/// Der Baum steht erst NACH dem Auslegen. Wer beim Ankommen des Dokuments
/// fragt, bekommt den Titel der VORIGEN Seite.
```

## L909 · `pub fn set_hit_all(&self, on: bool) { self.hit_all.set(on); }`

```
/// Treffer-Kaesten fuer alle Elemente aufzeichnen (siehe `hit_all`).
```

## L916 · `pub fn img_cache_stats(&self) -> (usize, usize) {`

```
/// Entries and bytes BOTH cross-navigation caches hold, for the trace.
```

## L922-924 · `fn store_image(&self, src: &str, bytes: &[u8]) -> Result<(), crate::image::Reject> {`

```
/// The one place `<img>` pixels enter the store, shared by the shell's
/// fetched bytes and by a `data:` src decoded during layout, so the budget
/// is honoured on both paths.
```

## L938-941 · `fn resolve_data_uri_images(&self, dom: &crate::dom::Dom) {`

```
/// Decode every `data:` `<img src>` in the document. Such a src carries its
/// own bytes — there is nothing to fetch, and the pixels must exist BEFORE
/// layout because the intrinsic size decides the box. Mirrors
/// `resolve_css_images`, which does the same for `url(data:…)`.
```

## L952-954 · `let _ = eng.store_image(src, &bytes);`

```
// Eine Absage hier hat keinen Weg ins Log —
// die Bytes stehen im Dokument, es gibt keinen
// Wirt, der sie geholt haette und sie melden koennte.
```

## L967-969 · `pub fn set_images(&mut self, pairs: &[(alloc::string::String, Vec<u8>)]) {`

```
/// Decode + store a whole batch at once (holds all compressed bytes) — kept
/// for tests / non-streaming callers; the shell uses `images_begin` +
/// `add_image` to avoid hoarding.
```

## L977-978 · `pub fn layout(&self, html: &str, width: u32) -> Layout {`

```
/// Parse + lay out a document at `width`. Scroll-independent. Collects the
/// page's `<style>` blocks into the author stylesheet used by the cascade.
```

## L983-993 · `pub fn last_forms(&self) -> crate::forms::FormState {`

```
/// Like `layout`, but also applies `external_css` — the concatenated bytes
/// of the page's `<link rel=stylesheet>` files, which the shell fetches
/// (the engine is host-free) and passes in. External CSS cascades before
/// inline `<style>` (document/head order).
/// Lend the engine a monotonic tick source, so `Layout::phase` reports
/// what parse, cascade and layout each cost. Purely optional — the engine
/// stays free of host functions either way.
/// Der Steuerelement-Zustand des letzten Auslegens, als Kopie.
///
/// Eine KOPIE und keine Entleihung: der Rufer legt damit sofort neu aus,
/// und `layout_forms` schreibt dabei in dasselbe Feld zurueck.
```

## L998 · `pub fn set_caret_on(&self, on: bool) { self.caret_on.set(on); }`

```
/// Den Schreibzeiger fuer das naechste Bild an- oder ausknipsen.
```

## L1009-1011 · `pub fn layout_forms(`

```
/// Like `layout_ext`, but paints the page's form controls with the user's
/// live state (typed text, checked boxes, focus + caret). The shell keeps
/// one `FormState` per page and re-lays out when it changes.
```

## L1029-1031 · `crate::picture::resolve(&mut dom, crate::css::Media::new(width as f32, self.theme.is_dark()));`

```
// `<picture>`/`srcset` is folded into the `<img>` before anything
// reads a `src` — layout, the fetch list and the draw op then all
// see the one URL that actually won.
```

## L1041-1046 · `let t_parse = now();`

```
// The cascade also reads the document's own `<style>` blocks and the
// viewport width (media queries), so both are part of the identity.
// The theme is part of the identity too: `prefers-color-scheme` decides
// which rules apply, and `resolve_vars` BAKES the winning custom
// properties into the text it hands on — so a light and a dark sheet
// are different documents, not the same one read differently.
```

## L1049-1061 · `let style_text = crate::css::style_text(dom);`

```
// The viewport HEIGHT is part of the identity too, since `resolve_vars`
// bakes custom properties down and one may hold a `vh` length. Without
// it a purely vertical window resize would keep the stale sheet.
//
// **Der Skriptbaum steht mit seinem INHALT im Schluessel, nicht mit
// einem Zaehler.** Bis 0.184.0 ging `scripted_gen` hier ein, und das
// heisst: jede einzelne DOM-Aenderung — ein `classList.toggle` —
// machte das ganze Blatt ungueltig und `parse` lief ueber 1,06 MB
// neu (gemessen auf DDGs Ergebnisseite: 15,8 ms je Auslegen). Am
// Baum haengt die Kaskade aber nur an zwei Dingen: dem Text der
// `<style>`-Bloecke und den `url()` in `style`-Attributen. Das erste
// steht jetzt im Schluessel, das zweite wird bei einem Treffer
// nachgetragen.
```

## L1077-1079 · `crate::css::add_inline_urls(dom, &mut self.sheet.borrow_mut()[0].1);`

```
// Ein `style="background-image:url(…)"`, das ein Skript eben
// gesetzt hat, steht in keinem geparsten Blatt — es muss in die
// Tabelle, sonst hat der Befehl beim Malen keine Adresse.
```

## L1094-1099 · `fn resolve_inline_svgs(&self, dom: &crate::dom::Dom, lay: &Layout) {`

```
/// Rasterise the inline `<svg>`s a layout painted, under their store keys.
///
/// This runs AFTER layout on purpose: `currentColor` is the element's
/// computed `color` and the box is CSS's, so neither is known before the
/// cascade. The box is definite from the markup either way, so nothing has
/// to be laid out twice.
```

## L1123 · `continue;`

```
// An <svg> subtree holds no HTML; nothing below it to visit.
```

## L1132-1138 · `fn resolve_css_images(&self, sheet: &crate::css::Stylesheet, lay: &mut Layout) {`

```
/// Turn the CSS image keys a layout needs back into URLs.
///
/// A `data:` URI carries its own bytes, so the engine decodes it here and
/// the shell never hears about it; everything else is reported in
/// `css_image_srcs` for the shell to fetch and hand back via
/// `add_css_image`. Already-decoded keys are skipped, so this stays cheap
/// across the several layouts one page runs through.
```

## L1168-1169 · `pub fn add_css_image(&self, key: u64, bytes: &[u8]) -> Result<(), crate::image::Reject> {`

```
/// Store a CSS image the shell fetched (see `Layout::css_image_srcs`).
/// Costs a repaint, never a re-layout: a background cannot move a box.
```

## L1174-1180 · `pub fn add_css_image_cached(&self, key: u64, url: &str, bytes: &[u8])`

```
/// As [`Self::add_css_image`], and keep the pixels under `url` for the next
/// navigation.
///
/// `url_key` is a hash of the `url()` text as the sheet wrote it, so it is
/// unique only WITHIN one document — two sites both saying `url(/bg.png)`
/// share a key. Across navigations the RESOLVED url is the only honest
/// identity, exactly as for `<img>`.
```

## L1191-1192 · `pub fn adopt_css_cached(&self, key: u64, url: &str) -> bool {`

```
/// Serve one background layer from the cross-navigation cache. True on a
/// hit — the shell then does not queue it for fetching.
```

## L1207-1208 · `pub fn css_images_begin(&self) {`

```
/// Drop the previous page's CSS images (called on navigation). The
/// cross-navigation cache is untouched, same as `images_begin`.
```

## L1214-1215 · `pub fn layout_ua(&self, html: &str, width: u32) -> Layout {`

```
/// Lay out with the UA sheet ONLY — no author `<style>`/`<link>` CSS
/// (reader mode; docs/spec/BROWSER.md §9.7 "never worse than clean content").
```

## L1220 · `pub fn layout_ua_forms(&self, html: &str, width: u32, forms: &crate::forms::FormState) -> Layout {`

```
/// Reader mode with live form state (see `layout_forms`).
```

## L1235 · `&[],`

```
// Reader mode drops the page's sheet, so nothing can style `:hover`.
```

## L1241-1256 · `pub fn paint_band(`

```
/// Paint the slice `[scroll_y, scroll_y + h)` into `out` (must be
/// `w * h * 4` BGRA bytes).
/// Paint only the viewport rows `y0..y1` — the same picture [`Self::paint`]
/// would put there, without touching the rest of the buffer.
///
/// This is what makes a scroll cheap. Scrolling does not change the page;
/// it moves it. The pixels that merely moved are shifted with one
/// `copy_within`, and only the newly exposed band is drawn — against ~60-80
/// ms for a full 1902x1000 repaint on the device, which is what every
/// scroll used to cost.
///
/// No clipping had to be added anywhere for this, and that is the whole
/// trick: every drawing primitive already clips against `(0, 0, w, h)` of
/// the buffer it is handed. A band is just a narrower buffer — hand over
/// those rows' slice, say `h = y1 - y0`, and move the scroll offset down by
/// `y0` so document coordinates still land where they belong.
```

## L1276-1284 · `fn paint_marks(&self, layout: &Layout, wi: i32, hi: i32, scroll_y: i32, out: &mut [u8]) {`

```
/// Auswahl und Fundstellen, NACH allem anderen und halbdurchsichtig.
///
/// Ein Browser malt die Auswahl deckend HINTER den Text und dreht dessen
/// Farbe um. Das ginge hier auch — aber es hiesse, den Anstrich in zwei
/// Durchgaenge zu teilen und jedem Textbefehl anzusehen, ob er markiert
/// ist. Ein durchscheinender Schleier darueber liest sich auf hellem wie
/// dunklem Grund und kostet einen Durchgang ueber ein paar Rechtecke.
/// Die ehrlichere Naeherung, und sie steht hier statt in einem Kommentar
/// weiter unten.
```

## L1297-1298 · `let hit = Rgba { c: self.theme.link, a: 70 };`

```
// Fundstellen zuerst, damit die Auswahl darueber liegt: wer waehrend
// einer Suche etwas markiert, soll seine Markierung sehen.
```

## L1306 · `fill(out, wi, hi, 0, 0, wi, hi, layout.bg.into());`

```
// Canvas = the propagated body background (falls back to theme bg).
```

## L1313-1316 · `DrawOp::Caret { x, y, w: rw, h: rh, color } => {`

```
// **Der Schreibzeiger blinkt, also wird er hier ausgelassen
// statt weggelassen.** Ein Takt darf kein Neuauslegen kosten
// — am Geraet sind das 10-40 ms, zweimal je Sekunde. So
// bleibt das Layout stehen und nur der Anstrich wechselt.
```

## L1329-1330 · `let keep = (*x - *dx + *spread, *y - *dy + *spread - scroll_y,`

```
// Der Kasten, der ausgespart bleibt: das Schattenrechteck
// zurueckgerechnet auf den Rahmenkasten.
```

## L1338 · `continue; // fully off-screen line → skip`

```
// fully off-screen line → skip
```

## L1340-1341 · `let cv = clip.map(|(cx, cy, cw, ch)| (cx, cy - scroll_y, cw, ch));`

```
// Der Ausschnitt steht in DOKUMENTkoordinaten; hier wird
// gerollt, also faehrt er dieselbe Strecke mit.
```

## L1350-1353 · `match self.images.borrow().get(src) {`

```
// Look the pixels up at PAINT time, so an image that
// arrives after layout needs only a repaint. A miss (not
// fetched yet, or an undecodable format) draws the
// placeholder that layout used to emit as separate ops.
```

## L1376-1379 · `if let Some(img) = self.css_images.borrow().get(key) {`

```
// A missing background draws NOTHING — unlike `<img>`,
// there is no placeholder for one: the box is styled and
// sized either way, so an absent decoration must simply be
// absent rather than a grey frame over the content.
```

## L1389-1392 · `#[allow(clippy::too_many_arguments)]`

```
/// The box an `<img>` shows while its pixels are missing: a thin frame
/// plus the alt text. Lives here rather than in layout so that an image
/// arriving later swaps the placeholder for the picture without the
/// display list changing at all.
```

## L1415-1416 · `#[allow(clippy::too_many_arguments)]`

```
/// Draw a run at `(x, y=run-top)` in the face selected by `bold`/`italic`/
/// `mono` (see `Fonts::pick`) — real weight/slant/monospace, no synthesis.
```

## L1430-1432 · `family: u32,`

```
// Streuwert der `font-family` — dieselbe Zahl, mit der das Layout
// gemessen hat. Ohne sie malte der Rasterer die eingebaute Schrift
// unter die Breiten einer Seitenschrift.
```

## L1434-1436 · `sp: (f32, f32),`

```
// `(letter-spacing, word-spacing)` — the SAME pair layout measured the
// run with. Advancing the pen by anything else puts the glyphs somewhere
// the line box did not reserve.
```

## L1439-1441 · `clip: Option<(i32, i32, i32, i32)>,`

```
// Der Ausschnitt in ANSICHTskoordinaten, `None` heisst die ganze
// Leinwand. Er klemmt dieselben vier Grenzen wie der Rand des
// Puffers, also kostet er kein Pixel mehr.
```

## L1455 · `let mut cache = self.glyphs.borrow_mut();`

```
// One borrow for the whole run instead of three per character.
```

## L1457-1461 · `let ligated = sp.0 == 0.0 && font.ligatures().is_some();`

```
// **Dieselbe Regel wie beim Messen, sonst landen die Glyphen neben
// dem Kasten, den die Zeile reserviert hat** — Ligaturen ausser bei
// `letter-spacing`, das sie laut css-text-3 §8.2 aufbricht.
// `shape` gibt je Glyphe die Byte-Spanne der Quelle, und daraus kommt
// die Laufweite, die das Layout gerechnet hat.
```

## L1468-1469 · `(g as u32 | 0x8000_0000, extra)`

```
// Der Glyphenspeicher unterscheidet Zeichen von Index am
// hohen Bit: sonst kollidierte Glyphe 65 mit `A`.
```

## L1477-1480 · `if unit & 0x8000_0000 == 0`

```
// Dasselbe wie beim MESSEN: ein Formatierungszeichen hat keine
// Glyphe und keine Laufweite. Wer es hier malen liesse, schoebe
// den Stift um genau das weiter, was die Messung nicht gerechnet
// hat ([[feedback_intrinsic_shared_path]]).
```

## L1495-1496 · `let (cx0, cx1) = (gx0.max(klx), (gx0 + m.width as i32).min(klr));`

```
// Clip the glyph box against the buffer once; the inner loop then
// walks a row by offset and never re-tests a bound.
```

## L1506-1508 · `let a = mul255(cov[row + gx], color.a);`

```
// Glyph coverage times the colour's own alpha — a
// translucent text colour dims the whole run, it does not
// sharpen its edges.
```

## L1525-1534 · `fn fill(out: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, c: Rgba) {`

```
/// Fill a rect by building ONE row and copying it, rather than storing four
/// bytes per pixel.
///
/// This is the hottest loop in the app. A frame clears the canvas and then
/// paints roughly another viewport of backgrounds on top, so about 3.7 M pixels
/// are written per scroll step — and under the wasmi interpreter every one of
/// those byte stores is an interpreted instruction with its own bounds check.
/// `copy_within` compiles to `memory.copy`, a single instruction the host
/// executes as a native memmove, so an N-pixel row costs log2(N) copies to
/// build plus one copy per further row instead of 4·N·rows stores.
```

## L1543-1547 · `if !c.is_opaque() {`

```
// A translucent fill has to READ each destination pixel, so none of the
// row-copy trick below applies — every pixel is its own blend. Kept behind
// this branch rather than folded into the loop so the opaque case, which is
// the overwhelming majority and the hottest loop in the app, still costs
// one `memory.copy` per row.
```

## L1561 · `out[first] = c.2; // B`

```
// B
```

## L1562 · `out[first + 1] = c.1; // G`

```
// G
```

## L1563 · `out[first + 2] = c.0; // R`

```
// R
```

## L1564 · `out[first + 3] = 255; // A`

```
// A
```

## L1577-1580 · `fn round_insets(row_y: f32, rh: f32, r: [f32; 4]) -> (f32, f32) {`

```
/// How far a rounded rect's left and right edges move inwards on the row whose
/// top is `row_y` (rect-local), in fractional pixels. Radii are `[tl, tr, br,
/// bl]` (CSS corner order) and are treated as circular — CSS allows an ellipse
/// per corner, we take one radius.
```

## L1583 · `let cy = row_y + 0.5; // sample the row's centre`

```
// sample the row's centre
```

## L1603-1605 · `fn fill_span(out: &mut [u8], w: i32, h: i32, y: i32, xl: f32, xr: f32, c: Rgba) {`

```
/// Fill one row's horizontal span with fractional ends: the interior is a solid
/// run, the two boundary pixels get partial coverage. That antialiasing is what
/// keeps a 2px corner from looking like a chopped pixel.
```

## L1611 · `let (i0, i1) = ((l as i32 + 1).max(0), ((rr as i32) - 1).min(w));`

```
// Solid interior first, then the two fractional edges over it.
```

## L1619-1620 · `let a = (cov.min(1.0) * c.a as f32) as u8;`

```
// Two coverages multiply: how much of the pixel the shape covers,
// and how opaque the colour itself is.
```

## L1625 · `if rr - l <= 1.0 {`

```
// A span narrower than one pixel covers a single pixel partially.
```

## L1634-1639 · `fn grad_tile_size(area: (i32, i32), size: BgSize) -> (i32, i32) {`

```
/// Die Kachelgroesse eines Verlaufs.
///
/// Ein Verlauf hat KEINE eigene Groesse (css-images-3 §4.3): sein Vorgabemass
/// ist die Positionierflaeche selbst. Damit fallen `auto`, `cover` und
/// `contain` alle auf die Flaeche zurueck, und nur eine ausdrueckliche
/// Groesse macht daraus eine Kachel.
```

## L1652-1658 · `#[allow(clippy::too_many_arguments)]`

```
/// Einen Farbverlauf ueber die Positionierflaeche `x,y,gw,gh` malen —
/// gekachelt nach `size`/`pos`/`repeat`, beschnitten auf `cl` und auf die
/// Eckenradien `r`.
///
/// Die Kachelung ist dieselbe wie bei einem Bild und aus demselben Grund:
/// `background-image` ist EINE Eigenschaft, und ein Verlauf steht darin an
/// derselben Stelle wie ein `url()`.
```

## L1681-1682 · `let span = |origin: i32, box_lo: i32, box_hi: i32, tile: i32, rep: bool| -> (i32, i32) {`

```
// Wie in `blit_bg`: wie viele Kacheln zurueck und vor, bis der Malbereich
// verlassen ist. Eine nicht wiederholte Achse hat genau eine.
```

## L1700-1706 · `#[allow(clippy::too_many_arguments)]`

```
/// EINE Kachel des Verlaufs.
///
/// Drei Wege, und der Grund ist die Groesse: ein Seitenhintergrund ist
/// 1902x1000 = 1,9 Mio Pixel, und jedes einzeln zu rechnen kostet mehr als
/// alles andere im Bild zusammen. Ein SENKRECHTER Verlauf hat je Zeile genau
/// eine Farbe — eine Zeile ist ein `memory.copy`. Nur der schraege und der
/// radiale laufen wirklich Pixel fuer Pixel.
```

## L1720 · `let x0 = x.max(cl.0).max(0);`

```
// Sichtbarer Bereich: Kachel ∩ Malbereich ∩ Bild.
```

## L1731-1733 · `let span = |py: i32| -> (i32, i32) {`

```
// Die Rundung gehoert dem MALBEREICH, nicht der Kachel: mit einem Rahmen
// sind das zwei verschiedene Rechtecke, und die Ecke, die der Verlauf
// nicht ueberlaufen darf, ist die des Malbereichs.
```

## L1745-1747 · `let (rx, ry) = if g.circle {`

```
// Mitte, `farthest-corner`. Eine Ellipse behaelt das Seitenverhaeltnis
// des Kastens und geht durch die Ecke — das Wurzel-Zwei-Fache der
// halben Seiten. Ein Kreis hat EINEN Radius: den Abstand zur Ecke.
```

## L1769-1770 · `let rad = g.angle_for(fw, fh) * core::f32::consts::PI / 180.0;`

```
// CSS zaehlt den Winkel im Uhrzeigersinn ab „nach oben"; die Achse zeigt
// damit nach `(sin, -cos)` in Bildkoordinaten (y waechst nach unten).
```

## L1776 · `let t_at = |px: f32, py: f32| 0.5 + ((px - cx) * sa - (py - cy) * ca) * inv;`

```
// `t` an einem Pixel: die Projektion auf die Achse, auf 0..1 normiert.
```

## L1780 · `for py in y0..y1 {`

```
// Senkrecht: eine Farbe je Zeile.
```

## L1795-1797 · `let mut row: Vec<Rgba> = Vec::with_capacity((x1 - x0) as usize);`

```
// Waagrecht: jede Zeile ist dieselbe Farbfolge. Einmal rechnen, dann
// nur noch schreiben — das nimmt der heissesten Schleife im Bild die
// Winkelrechnung und die Stoppsuche je Pixel.
```

## L1828-1834 · `#[allow(clippy::too_many_arguments)]`

```
/// Fill a rounded rect, or — when `ring > 0` — only a border of that thickness
/// along its inside edge. Radii are in px, `[tl, tr, br, bl]`.
///
/// A solid fill only walks rows inside the corner bands; everything between
/// them is ONE `fill` call. So a page-tall background with a 2px radius still
/// costs one `memory.copy` per row instead of a per-pixel loop over millions
/// of pixels.
```

## L1845-1853 · `let cap = fw.max(fh);`

```
// Erst jeden Radius auf die Kastenseite deckeln, DANN die Paare summieren.
//
// Sonst laeuft die Summe ueber: Tailwind schreibt seine Pille als
// `border-radius: 3.40282e38px` — und das ist f32::MAX, also ist
// `r[0] + r[1]` unendlich, `extent / sum` wird 0, und der Faktor unten
// setzt ALLE Radien auf null. Die Pille kam als Rechteck heraus.
// Deckeln aendert die Form nicht: ein Radius groesser als die Seite ist
// ohnehin nicht darstellbar, und die Verhaeltnisse zwischen den Ecken
// bleiben, weil danach immer noch EIN gemeinsamer Faktor wirkt.
```

## L1856-1858 · `let mut scale = 1.0f32;`

```
// A radius may not exceed half the box, and CSS scales ALL of them by one
// factor when any pair overflows its side (css-backgrounds-3 §5.5) —
// clamping each corner on its own would change the shape.
```

## L1884-1886 · `let inner = [`

```
// Ring: the hole's radii shrink with the border but never go negative — a
// border thicker than the radius leaves a square inner corner, as browsers
// do. Rows above and below the hole are border across their whole span.
```

## L1907 · `fn ceil_f(v: f32) -> i32 {`

```
/// `ceil` as an i32 — `core` has no `f32::ceil` in `no_std`.
```

## L1912-1914 · `#[inline]`

```
/// Two 0..255 coverages multiplied, rounded — `255 * x == x`, so an opaque
/// colour leaves a coverage untouched and the antialiasing is bit-identical to
/// what it was before alpha existed.
```

## L1920-1922 · `#[inline]`

```
/// Blend `c` at `a`/255 coverage over the pixel starting at byte `i`. Takes the
/// offset rather than (x, y) so the caller can walk a row by adding 4 instead of
/// recomputing `y * w + x` for every pixel it touches.
```

## L1924-1949 · `#[allow(clippy::too_many_arguments)]`

```
/// Ein weichgezeichneter Schlagschatten.
///
/// **Die Deckung eines gaussisch weichgezeichneten Rechtecks ist trennbar:**
///
///     a(x,y) = A * S(x; links, rechts) * S(y; oben, unten)
///     S(t; a, b) = Phi((t-a)/sigma) - Phi((t-b)/sigma)
///
/// Das ist keine Naeherung, sondern exakt — die Faltung eines Rechtecks mit
/// einem trennbaren Kern zerfaellt in zwei eindimensionale. Damit kostet ein
/// Pixel EINE Multiplikation statt einer Faltung, und das Waagrechte wird
/// einmal je Schatten gerechnet statt einmal je Zeile.
///
/// `sigma = blur / 2`, wie CSS Backgrounds 3 §7.1.1 es vorschreibt: der
/// Radius spannt zwei Standardabweichungen.
///
/// **Die Ecken folgen dem `border-radius`** — seit 0.187.0. Der Kommentar
/// hier sagte vorher, der Unterschied liege bei 6 px Radius unter der
/// Sichtbarkeitsschwelle. Das stimmt fuer eine KARTE und nicht fuer eine
/// KAPSEL: DuckDuckGos Suchfeld ist 40 px hoch mit 24 px Radius, und sein
/// Ring ist ein Schatten. Eckig gemalt war es der ganze Unterschied zum
/// Browserbild ([[feedback_a_comment_that_names_its_condition_expires]]).
///
/// **Der trennbare Weg bleibt, wo er stimmt.** Ohne Radius ist die Deckung
/// exakt `fx * fy`, und daran wird nichts gerechnet. Nur INNERHALB der vier
/// Eckquadrate tritt die Abstandsfunktion an ihre Stelle — dort, und nur
/// dort, war das Rechteck falsch.
```

## L1957 · `let pad = libm::ceilf(sigma * 3.0) as i32;`

```
// Jenseits von drei Sigma ist die Deckung unter einem halben Prozent.
```

## L1964 · `let mut fx: alloc::vec::Vec<f32> = alloc::vec::Vec::with_capacity((x1 - x0) as usize);`

```
// Das waagrechte Profil einmal, nicht je Zeile.
```

## L1969-1970 · `let lim = (w.min(h) as f32) * 0.5;`

```
// Die Radien auf den Kasten klemmen: mehr als die halbe Kante gibt es
// nicht (CSS Backgrounds 3 §5.5).
```

## L1975-1976 · `let corner = |px: f32, py: f32| -> Option<(f32, f32, f32)> {`

```
// Mittelpunkt und Radius der Ecke, in deren Quadrat dieser Punkt liegt —
// sonst `None`, und dann gilt der trennbare Weg unveraendert.
```

## L1998-2007 · `if inside_y && px >= keep.0 && px < keep.0 + keep.2 {`

```
// Ein AEUSSERER Schatten wird nicht in den RAHMENkasten gemalt
// (CSS Backgrounds 3 §7.1.1). Unter einem deckenden Kasten faellt
// das nicht auf; unter einem durchsichtigen ist es der Ring, den
// ein Browser dort auch zeigt.
//
// Ausgespart wird der RAHMENkasten, nicht das Schattenrechteck.
// Dieselben sind die beiden nur ohne Versatz und ohne Spread —
// Tailwinds `shadow-lg` hat beides (`0 10px 15px -3px`), und die
// falsche Aussparung schnitt einen weissen Zapfen genau in den
// Streifen unter dem Kasten, wo der Schatten am dunkelsten ist.
```

## L2011-2013 · `let cov = match if round { corner(px as f32 + 0.5, py as f32 + 0.5) } else { None } {`

```
// **Im Eckquadrat entscheidet der ABSTAND, nicht das Produkt.**
// Auf den geraden Kanten geben beide dasselbe (eine Kante weit
// weg traegt den Faktor eins), also stossen sie stetig aneinander.
```

## L2032-2034 · `fn span(t: f32, a: f32, b: f32, sigma: f32) -> f32 {`

```
/// Wieviel einer gaussisch verschmierten Kante liegt bei `t` noch im Band
/// `[a, b]`? `Phi` ueber die Fehlerfunktion — `libm` hat sie, also braucht es
/// hier keine Naeherung, die man spaeter erklaeren muss.
```

## L2041-2046 · `fn stroke_check(out: &mut [u8], w: i32, h: i32, x: i32, y: i32, cw: i32, ch: i32, color: Rgba) {`

```
/// Der Haken eines Kaestchens: zwei Striche im Kasten `w`x`h`.
///
/// Die drei Punkte sind die Verhaeltnisse, die jeder Browser malt — kurzer
/// Strich nach unten rechts, langer nach oben rechts. Die Kantenglaettung
/// kommt aus dem ABSTAND zum Strich, nicht aus einer Ueberabtastung: bei
/// 13 px ist der Haken zwei Striche breit, und jede Stufe waere sichtbar.
```

## L2050 · `let pts = [(0.22 * fw, 0.52 * ch as f32), (0.42 * fw, 0.73 * fh), (0.78 * fw, 0.28 * fh)];`

```
// Die Ecken des Hakens, in Anteilen des Kastens.
```

## L2052 · `let hw = (fw.min(fh) * 0.085).max(0.9); // halbe Strichbreite`

```
// halbe Strichbreite
```

## L2053 · `let (x0, y0) = ((x.max(0)), (y.max(0)));`

```
// Nur die Zeilen und Spalten anfassen, die der Haken ueberhaupt trifft.
```

## L2071-2072 · `let cov = (hw + 0.5 - d).clamp(0.0, 1.0);`

```
// Ein Pixel, dessen Mitte genau auf dem Rand liegt, ist halb
// gedeckt — daher das halbe Pixel Zugabe.
```

## L2092 · `out[i] = ((c.2 as u32 * a + out[i] as u32 * ia) / 255) as u8; // B`

```
// B
```

## L2093 · `out[i + 1] = ((c.1 as u32 * a + out[i + 1] as u32 * ia) / 255) as u8; // G`

```
// G
```

## L2094 · `out[i + 2] = ((c.0 as u32 * a + out[i + 2] as u32 * ia) / 255) as u8; // R`

```
// R
```

## L2098-2101 · `fn bg_tile_size(area: (i32, i32), img: (u32, u32), size: BgSize) -> (i32, i32) {`

```
/// Nearest-neighbour scale a decoded `img` (BGRA) into a `dw`×`dh` box at
/// (dx, dy), alpha-blending over `out`. Clipped to the buffer.
/// Resolve `background-size` against the positioning area (css-backgrounds-3
/// §3.9). `auto` on one axis keeps the intrinsic aspect ratio.
```

## L2137-2139 · `#[allow(clippy::too_many_arguments)]`

```
/// Paint one `background-image`/`mask-image` layer into the box `dx,dy,dw,dh`.
/// Everything is clipped to that box; repeating axes tile outward from the
/// positioned origin.
```

## L2149-2150 · `clip: (i32, i32, i32, i32),`

```
// The painting area (`background-clip`). `d*` above is the POSITIONING
// area, which is what the tile grid is anchored to and measured against.
```

## L2165-2166 · `let span = |origin: i32, box_lo: i32, box_hi: i32, tile: i32, rep: bool| -> (i32, i32) {`

```
// Tile range: how many steps back from the origin before leaving the box,
// and how many forward. A non-repeating axis is the single tile.
```

## L2177-2178 · `let (cx0, cx1) = (clip.0.max(0), (clip.0 + clip.2).min(w));`

```
// Clip to the painting area AND to the surface in one rect, so the inner
// loop never tests bounds per pixel.
```

## L2196-2198 · `let cols: Vec<usize> = (x0..x1)`

```
// Source column per destination column, resolved once per tile
// rather than a multiply+divide per pixel (the interpreter charges
// ~150× for a per-pixel loop — see the wasmi hot-loop note).
```

## L2208-2210 · `let a = match tint {`

```
// A translucent tint multiplies into the mask's own
// alpha, so a 50 %-opaque tint through a solid stencil is
// half-covered rather than solid.
```

## L2216-2217 · `let src = match (tint, filter) {`

```
// A mask takes only the alpha and paints the tint
// through it; a background image paints its own pixels.
```

## L2219 · `(Some(c), _) => [c.c.2, c.c.1, c.c.0],`

```
// A mask's tint was already filtered at layout.
```

## L2247-2254 · `fn object_rect(fit: ObjectFit, dx: i32, dy: i32, dw: i32, dh: i32, iw: i32, ih: i32) -> (i32, i32, i32, i32) {`

```
/// Where a replaced element's pixels land inside the content box the layout
/// gave it — `object-fit` (css-images-3 §5.5). Returns the rectangle the
/// picture is DRAWN into, which for `cover`/`none` may be larger than the box
/// (the caller clips) and for `contain`/`scale-down` smaller (it letterboxes).
///
/// `object-position` is not implemented; the picture is centred, which is that
/// property's initial value (`50% 50%`) and what every use of `object-fit` on
/// the two vendored sheets asks for.
```

## L2258 · `ObjectFit::Fill => return (dx, dy, dw, dh),`

```
// The initial value: stretch to the box on both axes, aspect ignored.
```

## L2263-2264 · `ObjectFit::ScaleDown => (bw / sw).min(bh / sh).min(1.0),`

```
// `scale-down` is `none` and `contain`, whichever comes out smaller —
// an image that already fits keeps its own size instead of growing.
```

## L2271-2272 · `fn filt(layout: &Layout, idx: u16) -> Option<ColorFilter> {`

```
/// An image op's `filter` index resolved against the layout's side table.
/// 0 is "none", so the unfiltered path never touches the table at all.
```

## L2283-2285 · `let (ox, oy, ow, oh) = object_rect(fit, dx, dy, dw, dh, iw, ih);`

```
// Two rectangles once `object-fit` is not `fill`: the picture is scaled
// into `p*` and painted only where that meets the box, so `cover` crops
// instead of overflowing and `contain` leaves the rest of the box alone.
```

## L2294-2295 · `let cols: Vec<usize> = (x0..x1).map(|px| ((px - ox) * iw / ow).clamp(0, iw - 1) as usize * 4).collect();`

```
// The source column for each destination column, resolved once for the
// whole blit instead of a multiply, divide and clamp per pixel.
```

## L2303-2305 · `let px = match filter {`

```
// `filter` recolours the SOURCE pixel. Read through a local copy
// only when there is one, so the unfiltered blit keeps indexing
// straight into the decoded buffer.
```

## L2330-2331 · `const RED_10: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fi`

```
/// A decodable image in plain bytes — SVG is one of the formats
/// `image::decode` accepts, so a test needs no binary fixture.
```

## L2336-2339 · `let eng = Engine::new();`

```
// The claim behind the cheap scroll: painting rows y0..y1 into a slice
// produces the same bytes a whole frame would have in those rows.
// Deliberately with boxes and text that STRADDLE the band edges — an
// op crossing a boundary is where a missing clip would show.
```

## L2354 · `let mut banded = alloc::vec![0u8; px];`

```
// Bands chosen to cut through content, not between elements.
```

## L2361 · `let before = banded.clone();`

```
// And a band outside the viewport is a no-op rather than a panic.
```

## L2375 · `eng.layout(a, 800); // back`

```
// back
```

## L2376 · `eng.layout(b, 800); // forward`

```
// forward
```

## L2383-2384 · `let eng = Engine::new();`

```
// DOC_SLOTS = 3. A fourth page pushes the least recently used out —
// and the two still held stay free.
```

## L2402 · `eng.images_begin(); // a navigation: the page map goes, the cache stays`

```
// a navigation: the page map goes, the cache stays
```

## L2404-2405 · `let miss = eng.adopt_cached(&[(`

```
// The SAME src string on ANOTHER host is another picture. Serving it
// from the cache would put one site's image on another site's page.
```

## L2412 · `let hit = eng.adopt_cached(&[(`

```
// The same URL under a different src is the same picture.
```

## L2424 · `assert!(eng.add_css_image_cached(7, "https://a.example/bg.png", RED_10).is_ok());`

```
// `url_key` 7 is whatever site A's sheet hashed `url(/bg.png)` to.
```

## L2426 · `eng.css_images_begin(); // a navigation`

```
// a navigation
```

## L2428-2429 · `assert!(!eng.adopt_css_cached(7, "https://b.example/bg.png"),`

```
// Another site writing the SAME `url(/bg.png)` hashes to the same key
// and is a different picture. The key alone would have served it.
```

## L2432 · `assert!(eng.adopt_css_cached(99, "https://a.example/bg.png"),`

```
// The same picture under another key (a sheet that wrote it absolute).
```

## L2439-2442 · `let mut eng = Engine::new();`

```
// The point of adopting BEFORE the first layout: with the pixels
// already here the box is not guessed, so the arriving image never
// moves the page — which is the full re-layout that cost 1110-1710 ms
// on the device.
```

## L2453 · `let mut cold = Engine::new();`

```
// Without the cache the same markup has to guess.
```

## L2459-2461 · `const MASK_LEFT_HALF: &str = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22\`

```
/// An inline SVG `data:` URI, quote-safe: the SVG's own attribute quotes
/// are percent-encoded, so the URI survives being nested inside a CSS
/// string inside an HTML attribute (which is how real pages ship them).
```

## L2477 · `const PAD: u32 = 8; // the UA body { margin } — where page content starts`

```
// the UA `body { margin }` — where page content starts
```

## L2479-2480 · `fn pixel_at(html: &str, x: u32, y: u32) -> (u8, u8, u8) {`

```
/// Paint one page and read a pixel back as (r, g, b). `x`/`y` are relative
/// to the document's top-left content corner, i.e. past the page padding.
```

## L2485-2486 · `fn page(html: &str, cw: u32, ch: u32) -> impl Fn(u32, u32) -> (u8, u8, u8) {`

```
/// `pixel_at` over a content box of a given size — inline content needs a
/// line's worth of width. Returns a reader so one paint answers many probes.
```

## L2500-2502 · `const STRIPES_4X1: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAIAAAB2Xpia\`

```
/// A 4x1 PNG — red, green, blue, yellow — as a `data:` URI, so a test can
/// say where the picture landed without a fetch. Four columns and one row
/// make the aspect ratio (4:1) unmistakable against a square box.
```

## L2506-2512 · `#[test]`

```
/// **Ein Ausschnitt, der nicht mitwandert, schneidet an der ALTEN Stelle.**
/// Der Inhalt eines atomaren Inline wird bei 0,0 ausgelegt, dort
/// beschnitten und danach an seine Zeilenstelle verschoben. Blieb der
/// Ausschnitt stehen, wurde der Lauf an seiner neuen Stelle gegen ein
/// Rechteck an der alten geschnitten — uebrig blieb, wo sich beide um ein
/// Pixel ueberlappten, eine senkrechte Linie von einem Pixel. Auf
/// DuckDuckGos Trefferliste standen die quer durch die Seite.
```

## L2524 · `let mut ink = 0;`

```
// Rote Tinte RECHTS von x=200 — dort steht der Kasten wirklich.
```

## L2535-2539 · `#[test]`

```
/// **Die Kette von der Ankunft bis zum Pixel**, mit einer
/// protokollrelativen Adresse — genau der Fall von DuckDuckGos
/// Trefferliste (`//external-content.duckduckgo.com/ip3/x.ico`). Der Log
/// sagte „4 von 4 dekodiert", und die Kaestchen blieben leer; diese Zeile
/// fragt, ob zwischen ABLEGEN und MALEN derselbe Schluessel steht.
```

## L2546 · `eng.images_begin();`

```
// Wie der Wirt es tut: erst die Runde beginnen, dann das Bild ablegen.
```

## L2555 · `let i = ((2 * w + 2) * 4) as usize;`

```
// Das Testbild ist rot/gruen/blau/gelb — der erste Streifen ist ROT.
```

## L2562-2564 · `#[test]`

```
/// Dieselbe Kette in der REIHENFOLGE DES GERAETS: erst auslegen, dann
/// kommt das Bild an, dann wird neu gemalt. Genau das tut der Wirt, und
/// genau da blieben DuckDuckGos Kaestchen leer.
```

## L2574 · `let lay = eng.layout(html, w);          // ausgelegt, Bild noch nicht da`

```
// ausgelegt, Bild noch nicht da
```

## L2578 · `eng.paint(&lay, w, h, 0, &mut buf);     // nur neu MALEN, nicht neu auslegen`

```
// nur neu MALEN, nicht neu auslegen
```

## L2591-2592 · `#[test]`

```
/// `object-fit` decides how a replaced element's pixels fill the box the
/// layout gave it — the box itself is 20x20 in every one of these.
```

## L2595-2596 · `let html = stripes("fill");`

```
// `fill` is the initial value: stretched to the box, aspect ignored,
// so the four source columns become four 5px stripes.
```

## L2603 · `let html = stripes("contain");`

```
// `contain`: scaled down to 20x5 and centred — letterboxed above/below.
```

## L2610-2611 · `let html = stripes("cover");`

```
// `cover`: scaled UP to 80x20 and cropped to the box — only the two
// middle stripes survive, and nothing spills outside the box.
```

## L2619-2620 · `for fit in ["none", "scale-down"] {`

```
// `none`: the intrinsic 4x1, centred. `scale-down` is the smaller of
// that and `contain`, which here is the same 4x1.
```

## L2630-2632 · `#[test]`

```
/// Bootstrap ships `object-fit` only behind Opera's prefix in its utility
/// classes, so the unprefixed name alone would leave `.object-fit-cover`
/// doing nothing on a real page.
```

## L2643-2645 · `#[test]`

```
/// A mask paints the element's own background-colour through the image's
/// alpha — it does NOT paint the image. This SVG is opaque on its left half
/// only, so the box must be red on the left and untouched on the right.
```

## L2648 · `let html = alloc::format!(`

```
// A `data:` URI needs no fetch: the engine decodes it during layout.
```

## L2657-2659 · `#[test]`

```
/// An `outline` is drawn OUTSIDE the border box and takes no space — that
/// is the whole reason the property exists separately from `border`: a
/// focus ring has to appear without moving the page under the reader.
```

## L2666 · `assert_eq!(pixel_at(boxed, 10, 10), (0, 0, 255));`

```
// The box itself is untouched, and so is everything after it.
```

## L2670 · `assert_eq!(pixel_at(ringed, 10, 25), (255, 255, 255), "no ring below at rest…");`

```
// The ring sits in the two pixels OUTSIDE the border box.
```

## L2676 · `let after = |css: &str| {`

```
// It takes no space: a following box sits at exactly the same y.
```

## L2688-2693 · `#[test]`

```
/// `:hover` all the way to a pixel: lay out once to learn the geometry, ask
/// the layout what the pointer is inside, hand that back to the engine, lay
/// out again — which is exactly the loop the shell runs on `MouseMove`.
///
/// A cascade test would pass on a rule that resolves and never reaches the
/// screen; only the pixel says the feature works.
```

## L2711 · `let hovered = rest.hover_at((PAD + 10) as i32, (PAD + 10) as i32);`

```
// The box is at the content origin; probe its middle in document space.
```

## L2719 · `assert!(eng.set_hover(alloc::vec![]).is_changed());`

```
// Leaving the element takes the colour away again.
```

## L2724-2730 · `#[test]`

```
/// The whole point: a pointer change that only recolours is answered by
/// PATCHING the display list, and the result has to be indistinguishable
/// from having laid the page out again.
///
/// Measured on Wikipedia's Main_Page: 0.16 ms against 24 ms, and 55 of the
/// 64 hover targets on the page take this path — the rest fall back, which
/// is what every guard in `repaint_hover` exists to do.
```

## L2751 · `assert!(dump_ops(&full).contains("Rgb(255, 0, 0), a: 255"), "the link is red now");`

```
// …and it really did something.
```

## L2755-2762 · `#[test]`

```
/// A background that only exists under the pointer has nothing to replace
/// — it has to be INSERTED, and where it goes is the box's own insertion
/// point. The finished display list no longer holds an index for that, so
/// each hit rect carries the op that sits there, by content.
///
/// The rect it is painted at is not the hit rect: an inline box's
/// background covers its font's ascent + descent, not the line box, and
/// getting that wrong makes the patch one pixel taller than the layout.
```

## L2766-2767 · `"<p>text <a href=\"/x\">link</a> more</p>",`

```
// inline: painted at the font's box, and split across lines draws
// only the outer borders
```

## L2769 · `"<a href=\"/x\" style=\"display:block;width:60px;height:20px\">link</a>",`

```
// block: painted at the border box
```

## L2772-2773 · `let html = alloc::format!(`

```
// Only paint-class properties: a `border-left` would ADVANCE the
// inline flow, which is a layout and `set_hover` says so.
```

## L2790-2791 · `#[test]`

```
/// A hover rule that can MOVE something is not a repaint, and the engine
/// has to say so before anyone tries.
```

## L2804-2805 · `assert_eq!(probe("a:hover{cursor:pointer;color:#f00}"), HoverChange::Changed { paint_only: true });`

```
// A property we do not implement cannot move anything — and MediaWiki
// writes `cursor:pointer` into a third of its hover rules.
```

## L2807 · `assert_eq!(probe("a:hover{cursor:pointer;display:block}"), HoverChange::Changed { paint_only: false });`

```
// …but one layout property in the same rule is enough.
```

## L2811-2813 · `#[test]`

```
/// The guards. Each of these is a real page shape that a colour patch
/// cannot reproduce, and each has to end in "lay it out" rather than in a
/// wrong picture.
```

## L2827-2828 · `if ok {`

```
// Whatever it decided, it must never leave a page that disagrees
// with what a layout would have produced.
```

## L2834-2835 · `assert!(gives_up(`

```
// A pseudo-element with TEXT of its own: `content` is not part of what
// the element says, so its run cannot be identified.
```

## L2840 · `assert!(gives_up(`

```
// A rule that reaches sideways restyles something outside the subtree.
```

## L2847-2851 · `#[test]`

```
/// Ein aeusserer Schatten wird aus dem RAHMENkasten ausgespart, nicht aus
/// seinem eigenen Rechteck. Bei `0 10px 15px -3px` sind das zwei
/// verschiedene Rechtecke, und die falsche Aussparung liess genau den
/// Streifen unter dem Kasten weiss — dort, wo der Schatten am
/// dunkelsten ist.
```

## L2857 · `assert_eq!(p(50, 40), (255, 255, 255), "im Kasten kein Schatten");`

```
// Der Kasten selbst bleibt weiss.
```

## L2859 · `let under = p(50, 62).0;`

```
// Direkt darunter steht der dunkelste Teil.
```

## L2862 · `assert!(p(50, 75).0 > under, "der Schatten laeuft aus");`

```
// Und er wird nach unten hin heller.
```

## L2866-2868 · `#[test]`

```
/// `filter` recolours the element AND its whole subtree — the box's own
/// background, the text inside it, and an image's pixels, which are only
/// looked up at paint time and so travel as an index instead of a colour.
```

## L2871 · `let inv = "<div style='width:20px;height:20px;background:#ffff00;filter:invert(100%)'></div>";`

```
// `invert(100%)` on yellow is blue — the css-color reftest's case.
```

## L2875-2876 · `let nested = "<div style='filter:invert(100%)'>\`

```
// …and it reaches a descendant's background, which a page cannot
// cancel from inside the subtree.
```

## L2881 · `let html = alloc::format!(`

```
// An image's pixels: the 4x1 stripes, first column red → cyan.
```

## L2888 · `let twice = "<div style='width:20px;height:20px;background:#ffff00;\`

```
// A chain composes into ONE transform: inverting twice is identity.
```

## L2893-2894 · `let gray = "<div style='width:20px;height:20px;background:#ff0000;filter:grayscale(100)'></div>";`

```
// `grayscale(100)` — Bootstrap writes the amount as a bare number and
// means 1. Luma of pure red is 0.213 → 54.
```

## L2898-2899 · `let blur = "<div style='width:20px;height:20px;background:#ffff00;\`

```
// `blur` cannot be a matrix, so the whole declaration is dropped
// rather than half-applied — the box keeps its own colour.
```

## L2905-2908 · `#[test]`

```
/// `display: contents` generates no box: no border, and the children take
/// the place the box would have had. Inline-level content joins the line
/// its parent is building — which is the half a transparent block cannot
/// do, and the half every one of these tests turns on.
```

## L2916 · `let (a, b) = (dump_ops(&eng.layout(unboxed, 400)), dump_ops(&eng.layout(plain, 400)));`

```
// One line, laid out at the same y as if the span were not there.
```

## L2921-2922 · `let blocks = "<div>P<div style='display:contents'><div>A</div><div>S</div></div></div>";`

```
// A block-level child still gets a block: the wrapper is transparent,
// not a licence to flatten a paragraph into the line above it.
```

## L2929-2937 · `#[test]`

```
/// **Die `visually-hidden`-Technik des ganzen Webs**: ein Kasten von 1x1
/// mit `overflow:hidden` und einem langen Text darin. Ein Textbefehl wurde
/// vorher GANZ behalten, sobald er den Ausschnitt irgendwo beruehrte — und
/// bei 1x1 beruehrt er ihn immer. Auf DuckDuckGos Kopfzeile stand
/// „Search Settings" damit lesbar quer ueber dem Zahnrad.
///
/// Geprueft wird in PIXELN, nicht an einer Zahl: der Fehler war einer des
/// MALENS, der Kasten war die ganze Zeit richtig 1x1
/// ([[feedback_paint_test_not_parse_test]]).
```

## L2949 · `let mut ink = 0;`

```
// Ab x=8 ist der 1x1-Kasten vorbei; dahinter darf keine Tinte liegen.
```

## L2960-2964 · `#[test]`

```
/// **Die Bildsammlung las das urspruengliche HTML.** Auf einer Seite, die
/// ihren Inhalt per Skript baut, steht dort nichts: DuckDuckGos
/// Ergebnisseite ist eine Huelle, die React fuellt, und ihre Karte wie ihre
/// Seitensymbole kamen nie auch nur zur ANFRAGE — im Geraetelog stand
/// keine einzige Zeile zu `external-content.duckduckgo.com`.
```

## L2969 · `assert!(eng.image_srcs_now(huelle, 800).is_empty());`

```
// Ohne Skriptbaum: Zeichen fuer Zeichen die alte Antwort.
```

## L2983-2984 · `#[test]`

```
/// `text-overflow: ellipsis` — Bootstrap's `.text-truncate` idiom. The box
/// keeps the width it was given; only what is painted inside it changes.
```

## L2992-2993 · `assert!(run.contains("\u{2026}\""), "the run ends in an ellipsis: {run}");`

```
// Die Zeile traegt seit dem Text-Ausschnitt ein `clip=` am Ende; der
// Text steht davor, und geprueft wird der Text.
```

## L2997-2998 · `let clipped = truncate.replace("text-overflow:ellipsis", "text-overflow:clip");`

```
// `clip` is the initial value, and the same box under it keeps the
// whole run — the difference is the property, not the overflow.
```

## L3004-3010 · `#[test]`

```
/// A hover rule reaches a pseudo-element, and MediaWiki underlines the
/// article tabs with exactly that: an absolutely positioned `::after` that
/// is 2 px tall, transparent at rest and coloured under the pointer.
///
/// Its box is generated during layout and paints NOTHING at rest, so there
/// is no op to replace and none inside it to insert ahead of. What it can
/// name is its predecessor — everything its element painted comes first.
```

## L3013-3015 · `let html = "<style>a{display:flex;position:relative;color:#00e;width:80px;height:24px}\`

```
// The real shape: a tab link is itself a flex box (that is how Vector
// centres its label), and the underline hangs off it as an absolutely
// positioned `::after`.
```

## L3033-3034 · `#[test]`

```
/// A pseudo-element's box is for repainting, not for hit-testing: it must
/// not widen where the pointer counts as being inside the element.
```

## L3044-3045 · `let b = lay.hover_boxes.iter().find(|b| b.pseudo == crate::css::PseudoElem::None).copied().expect("own box");`

```
// The `::after` sits 40 px below the link. A point inside IT is not
// inside the link.
```

## L3051-3055 · `#[test]`

```
/// Everything a layout draws must survive being written down and read back
/// — the comparison the repaint tests lean on.
/// **Das Gate fuer den Schnellweg: neu gemalt muss BYTEGLEICH sein zu neu
/// ausgelegt.** Alles andere waere ein zweiter Renderpfad, der langsam
/// auseinanderlaeuft ([[feedback-byte-identical-render-gate]]).
```

## L3067-3068 · `state.focus = Some(seq);`

```
// Fokus + getippter Wert + Haken — alles, was ein Klick und eine Taste
// auslesen.
```

## L3079-3089 · `#[test]`

```
/// **Derselbe Vergleich, aber mit einem Kasten, der einen HINTERGRUND
/// hat.** Der wird UNTER den schon gemalten Inhalt geschoben, also vor
/// jedes Feld darin — und die notierte Befehlsspanne des Feldes blieb
/// dabei stehen. Der Schnellweg ersetzte danach fremde Befehle: getippter
/// Text lag unter dem alten Kasten (unsichtbar), und der Text daneben
/// rutschte in der Malreihenfolge nach hinten. Am Geraet sah das aus wie
/// „das Feld zeigt erst beim Verlassen etwas an und blendet dabei den
/// Text daneben weg".
///
/// Ein Verlauf am Vorfahren macht es schlimmer (ein Befehl mehr je
/// Kasten), deshalb steht er hier mit drin.
```

## L3105-3106 · `#[test]`

```
/// Und wenn eine `:checked`-Regel Kaesten bewegen kann, gibt der Schnellweg
/// auf, statt ein Menue zu, das sich haette oeffnen muessen.
```

## L3160-3161 · `#[test]`

```
/// Without a mask the same box is a plain filled rect — the guard that the
/// mask path is what changed, not background painting in general.
```

## L3169-3170 · `#[test]`

```
/// `no-repeat` must leave the rest of the box alone, and the tile must sit
/// where `background-position` puts it.
```

## L3184-3188 · `#[test]`

```
/// The spelling MediaWiki actually ships: a double-quoted `url()` whose
/// payload carries BACKSLASH-ESCAPED quotes. Stopping at the first inner
/// quote truncates the URI into something that still parses as a URL and
/// then silently decodes to nothing — so this is a paint test, not a
/// parse test.
```

## L3202-3204 · `#[test]`

```
/// An inline box has no block geometry — only the fragments it leaves in
/// line boxes. It still paints a background over them, and its horizontal
/// padding is part of that background AND advances the text after it.
```

## L3212-3214 · `#[test]`

```
/// The `a.external` shape: the icon lives in the padding the box reserves
/// past its text, so nothing shows unless BOTH the padding advances the
/// flow and the fragment paints its background image.
```

## L3229-3231 · `#[test]`

```
/// A box that only wraps an icon has no text at all. Its padding still
/// keeps the line box alive (CSS 2.1 §9.4.2) — otherwise the whole
/// `.vector-icon` pattern paints nothing.
```

## L3238-3240 · `#[test]`

```
/// Broken over two lines, an inline box leaves one rectangle per line —
/// and only the first carries its left border (`box-decoration-break:
/// slice`, the default).
```

## L3254-3256 · `#[test]`

```
/// Clicking a `<summary>` opens its section and closing it puts the page
/// back. The state is the `open` CONTENT attribute, so the page's own
/// `details[open]` rules see it — rustdoc and MDN both style that state.
```

## L3277 · `assert!(!eng.toggle_details(u32::MAX), "no such element");`

```
// A seq that is not a summary changes nothing — an unknown one too.
```

