# `tools/wasm/beak-engine/src/dom.rs` @ 5e0102684

## L1-11 · `use alloc::string::{String, ToString};`

```
//! dom.rs — tolerant HTML tree construction (WHATWG §13 subset).
//!
//! Slice-0 emitted a flat `Vec<Block>`; that threw away the structure both
//! inline flow and the CSS cascade need. This builds a real node tree —
//! elements with attributes + children, and text nodes — with the tolerant
//! recovery real pages rely on (implied `</p>`/`</li>`, unmatched end tags
//! ignored, raw-text `<script>`/`<style>`). Not the full state machine yet;
//! it grows toward it against html5lib-tests (docs/spec/CONFORMANCE.md).
//!
//! The tree is owned (no `Rc`/arena) — enough for a render-only pass. Live
//! mutation (Stage 2, JS-driven) will move to an arena; the shape stays.
```

## L17 · `pub enum Node {`

```
/// A DOM node: an element subtree or a run of text.
```

## L23 · `pub struct Element {`

```
/// An element: lowercased tag, attributes, child nodes.
```

## L28-32 · `pub seq: u32,`

```
/// Document-order index, assigned at parse time. A stable identity for one
/// element across re-layouts of the SAME document — form controls key their
/// live state (typed value, checked, focus) on it. Tree position can't serve
/// as the key: layout skips `display:none` subtrees, so any counter kept
/// during layout would drift from one kept during a plain DOM walk.
```

## L34-42 · `pub classes: Vec<String>,`

```
/// Derived from `attrs` ONCE, by `index_attrs` at parse time. There is no
/// scripting, and the two things that DO edit the tree afterwards —
/// `picture::resolve` folding a `<source>` into an `<img>`, and
/// `Engine::toggle_details` flipping `open` — touch neither class, id nor
/// tag, so none of this goes stale. `ElemInfo` used to re-derive all of it
/// on every construction, ~30 000× per layout.
/// Measured by doubling the work: **12.5 % of a whole layout**, spread so
/// thin across `flow_children` and the cascade that no single function
/// looked expensive.
```

## L46-47 · `pub checked_attr: bool,`

```
/// What the SOURCE said. Live form state (a box the reader ticked) is a
/// different thing and travels in `ElemState`.
```

## L53-56 · `pub fn bare(tag: String, seq: u32) -> Element { Element::new(tag, seq) }`

```
/// Ein Element von aussen bauen — der Weg zurueck aus der JS-Arena
/// (`js::dombind::Doc::to_dom`). Die abgeleiteten Felder sind LEER; wer
/// das benutzt, muss `index_attrs` rufen, sonst sieht der Selektor-
/// Vergleich weder Klassen noch id.
```

## L73-74 · `pub fn index_attrs(&mut self) {`

```
/// Fill the derived fields. MUST run after `attrs` is final — for foreign
/// content that is after the SVG name fixups, not before.
```

## L90 · `pub fn attr(&self, name: &str) -> Option<&str> {`

```
/// Value of `name` (case-insensitive, already lowercased at parse time).
```

## L96-97 · `pub struct Dom {`

```
/// The parsed document. `root` is a synthetic container holding the top-level
/// nodes (`<html>`/`<body>`/…); layout starts from `<body>` if present.
```

## L103 · `pub fn body(&self) -> &Element {`

```
/// The `<body>` element to lay out, or the root if the page has none.
```

## L108-112 · `pub fn root_element(&self) -> &Element {`

```
/// The document's root element (`<html>`), or the synthetic container when
/// the page has none. Its style has to be resolved even though it is never
/// painted: `html { font-size: … }` is what every `rem` resolves against,
/// and the `62.5%` "1rem = 10px" idiom is common enough that skipping it
/// scales a whole page.
```

## L118 · `fn find_tag<'a>(el: &'a Element, tag: &str) -> Option<&'a Element> {`

```
/// First descendant element with tag `tag` (depth-first).
```

## L133 · `const VOID: &[&str] = &[`

```
// Elements that never have children / an end tag.
```

## L138-151 · `const RAWTEXT: &[&str] = &["script", "style", "noscript"];`

```
// Raw-text elements: everything up to the matching close is literal text.
//
// **`noscript` steht hier, seit beak Skripte fahren kann.** Bei
// eingeschaltetem Skripting liest der Parser den Inhalt eines `<noscript>`
// laut HTML §13.2.5 als ROHTEXT — er baut daraus gar keine Elemente. Solange
// beak keine Skripte hatte, war das Gegenteil richtig, und der alte
// Kommentar in `style.rs` sagte das auch so.
//
// Was daran haengt, ist keine Feinheit: Googles Ergebnisseite legt in ihr
// `<noscript>` ein `<style>table,div,span,p{display:none}</style>` UND ein
// `<meta http-equiv="refresh" url=/httpservice/retry/enablejs>`. Als Markup
// gelesen versteckt das jede Tabelle, jeden Kasten und jeden Absatz der
// Seite und navigiert dann weg — bei einem Browser, der sehr wohl Skripte
// fährt. [[feedback_the_named_gap_may_not_be_the_gap]]
```

## L153 · `const BLOCK_STARTERS: &[&str] = &[`

```
// Block-level starters that imply a `</p>` when a `<p>` is still open.
```

## L160-161 · `pub fn parse(html: &str) -> Dom {`

```
/// Tolerant HTML → DOM tree. Recovers from the malformation real pages ship;
/// see the module doc for the subset covered.
```

## L163 · `let mut stack: Vec<Element> = Vec::with_capacity(16);`

```
// Open-element stack; index 0 is the synthetic root and is never popped.
```

## L172 · `let mut saw_svg = false;`

```
// Set once an <svg> start tag is seen; gates the foreign-content check.
```

## L179 · `if bytes[i + 1] == b'!' || bytes[i + 1] == b'?' {`

```
// Comment / doctype / CDATA-ish: `<!...>` (and `<?...>`).
```

## L194-196 · `let name = if saw_svg && stack.iter().any(|e| e.tag == "svg") {`

```
// The end tag is lowercased like every other, so inside
// foreign content it has to be adjusted the same way or it
// never matches the start tag it closes.
```

## L206 · `apply_implied_end_tags(&mut stack, &name);`

```
// Start tag. Apply the implied-end-tag recovery rules first.
```

## L212-214 · `if name == "svg" {`

```
// Foreign content keeps its case (see `svg::adjust_tag_name`). The
// stack walk only runs on a document that has an <svg> in it at
// all, so the common page pays nothing.
```

## L242-243 · `let ch = html[i..].chars().next().unwrap();`

```
// Text: decode entities as we accumulate; whitespace stays raw and
// is collapsed at layout (per `white-space: normal`).
```

## L257 · `while stack.len() > 1 {`

```
// Close any elements left open (malformed / truncated document).
```

## L268-277 · `fn imply_details_summary(root: &mut Element, seq: &mut u32) {`

```
/// "If there is no child summary element, the user agent should provide its own
/// legend" (HTML §4.11.1). Without one a closed `<details>` renders as NOTHING
/// — no triangle, no label, no box to click — and its contents are unreachable
/// rather than merely folded away. That is a worse page than showing
/// everything, so the control is not optional.
///
/// A browser puts this in the shadow tree; we insert it into the DOM, which
/// costs the `<details>`' own children one position in `:nth-child`. Measured
/// before choosing: 0 of 533 `<details>` in the page corpus and 4 of 7 in WPT
/// lack a summary, and none of those is selected by index.
```

## L297-303 · `fn imply_html_body(root: &mut Element) {`

```
/// HTML's tree construction inserts `<html>` and `<body>` even when the source
/// omits the tags (HTML Standard §13.2.6), and both are ordinary elements the
/// cascade can target. Without them a document that writes neither — routine in
/// hand-written pages and test suites — has nothing for `html { … }` /
/// `body { … }` to match, so those rules silently do nothing. `seq` is left at
/// 0 on the implied elements: they carry no form state, and every parsed
/// element keeps the index it was given.
```

## L313 · `fn is_metadata(n: &Node) -> bool {`

```
// Metadata stays a child of <html>, as the parser would place it in <head>.
```

## L336 · `pub fn title(dom: &Dom) -> Option<String> {`

```
/// The document `<title>` text, cleaned, if present.
```

## L349 · `fn flush_text(stack: &mut [Element], text: &mut String) {`

```
// ── tree-builder helpers ───────────────────────────────────────────────────
```

## L355-357 · `let t = core::mem::take(text);`

```
// Keep whitespace-only text too: between inline elements it is a
// significant space (`</a> <a>` ≠ `</a><a>`); the inline collapser turns
// it into a single space, and in a block context it collapses to nothing.
```

## L366 · `fn close_tag(stack: &mut Vec<Element>, name: &str) {`

```
/// Close the nearest matching open element (tolerant: unmatched → ignored).
```

## L377-378 · `fn apply_implied_end_tags(stack: &mut Vec<Element>, starting: &str) {`

```
/// HTML's optional-end-tag recovery for the cases content pages hit: a new
/// block closes an open `<p>`; a new `<li>`/`<dt>`/`<dd>` closes its sibling.
```

## L394 · `fn is_tagish(b: u8) -> bool {`

```
// ── tag / attribute lexing ─────────────────────────────────────────────────
```

## L400-401 · `fn read_tag(bytes: &[u8], start: usize) -> (String, usize) {`

```
/// Read a tag body between `<` and the matching `>`, honoring quotes so a `>`
/// inside an attribute value doesn't end the tag. Returns (body, index-after-`>`).
```

## L403 · `let mut i = start + 1; // skip '<'`

```
// skip '<'
```

## L423 · `fn skip_bogus(bytes: &[u8], start: usize) -> usize {`

```
/// Skip a `<!-- comment -->`, `<!doctype …>`, or `<?…?>` bogus construct.
```

## L426 · `let mut i = start + 4;`

```
// to "-->"
```

## L436 · `let mut i = start + 1;`

```
// to the next '>'
```

## L447 · `fn tag_name(raw: &str) -> String {`

```
/// Lowercased tag name after an optional leading `/`.
```

## L461-463 · `fn parse_attrs(raw: &str, out: &mut Vec<(String, String)>) {`

```
/// Parse `name="v"` / `name='v'` / `name=v` / bare `name` attribute pairs from
/// a tag body. Attribute names are lowercased; values keep their case and are
/// entity-decoded.
```

## L466 · `let mut i = 0;`

```
// Skip the tag name.
```

## L503 · `i += 1; // closing quote`

```
// closing quote
```

## L519-520 · `fn read_rawtext(html: &str, start: usize, name: &str) -> (String, usize) {`

```
/// Read raw-text-element content up to `</name>` (case-insensitive). Returns
/// (decoded-content, index-just-after-the-close-tag).
```

## L533 · `let mut j = i + 2 + close.len();`

```
// advance past "</name ... >"
```

## L546 · `fn decode_all(s: &str) -> String {`

```
// ── entities + whitespace ──────────────────────────────────────────────────
```

## L548 · `fn decode_all(s: &str) -> String {`

```
/// Decode every entity in `s` (used for attribute values / title text).
```

## L566-567 · `fn decode_entity(s: &str) -> (String, usize) { decode_entity_at(s, false) }`

```
/// Decode one entity starting at `&…`. Returns (text, bytes-consumed). On no
/// match, consumes just the `&` and returns it literally.
```

## L570-585 · `fn decode_entity_at(s: &str, in_attr: bool) -> (String, usize) {`

```
/// Dasselbe, aber mit der Auskunft, ob wir in einem ATTRIBUT stehen — und das
/// ist keine Feinheit, sondern der Unterschied zwischen einem funktionierenden
/// und einem zerstoerten Link.
///
/// **Die Tabelle ist jetzt die ganze** (`entities::NAMED`, 2125 Namen).
/// Vorher standen fuenfzehn hier, und jeder andere Name landete als TEXT auf
/// der Seite: DuckDuckGos Vorlage schreibt `&ZeroWidthSpace;`, und das stand
/// zehnmal woertlich in der Randspalte.
///
/// Zwei Regeln der Spezifikation (WHATWG §13.2.5.72 f.), beide hier:
///
/// * **Ohne `;` gilt nur die Altlast** — die 106 Namen, die HTML weiter ohne
///   Semikolon zulaesst, und davon die LAENGSTE Uebereinstimmung.
/// * **In einem Attribut wird eine Altlast NICHT ersetzt**, wenn danach `=`
///   oder ein alphanumerisches Zeichen steht. Genau dafuer gibt es die Regel:
///   `?a&copy=1` ist ein Abfrageteil und kein Copyright-Zeichen.
```

## L590-591 · `if let Some(num) = rest.strip_prefix('#') {`

```
// Numerisch: `&#123;` / `&#x7b;`. Der Leser darf hier NICHT ueber die
// Ziffern hinauslaufen, sonst frisst ein `&#` ohne `;` den halben Text.
```

## L601-602 · `let semi = usize::from(digits[n..].starts_with(';'));`

```
// Das `;` gehoert dazu, wenn es da ist; fehlt es, ist der Verweis
// laut Spezifikation trotzdem gueltig (mit Parse-Fehler).
```

## L610-611 · `let name_len = rest.char_indices()`

```
// Benannt. Der Name laeuft bis zum ersten Zeichen, das keiner sein kann —
// laenger als der laengste der Tabelle braucht niemand zu lesen.
```

## L622 · `let mut lit = String::from("&");`

```
// Kein bekannter Name: woertlich stehen lassen, wie bisher.
```

## L629-630 · `if name_len > 0 {`

```
// Kein Semikolon — nur die Altlast, und im Attribut nur, wenn danach
// weder `=` noch ein alphanumerisches Zeichen kommt.
```

## L644 · `pub fn collapse_ws(s: &str) -> String {`

```
/// Collapse runs of whitespace to a single space, trimming the ends.
```

## L689-691 · `#[test]`

```
/// **Die Tabelle war fuenfzehn Namen gross, das Web hat 2125.** Alles
/// andere stand woertlich auf der Seite — DuckDuckGos Randspalte zeigte
/// zehnmal `&ZeroWidthSpace;` als Text.
```

## L699-701 · `#[test]`

```
/// Ohne `;` gilt nur die Altlast — und in einem ATTRIBUT auch die nicht,
/// wenn `=` oder ein Buchstabe folgt. Sonst zerlegt `?a&copy=1` sich
/// selbst (WHATWG §13.2.5.73).
```

## L707 · `assert_eq!(a.attr("title"), Some("\u{a9} 2026"));`

```
// Kein `=` dahinter: dort wird ersetzt, im Attribut wie im Text.
```

## L712 · `#[test]`

```
/// Der numerische Leser darf nicht ueber seine Ziffern hinauslaufen.
```

## L716-717 · `assert_eq!(text_of(dom.body()), "ABC D &# E");`

```
// `&#67` ohne `;` ist ein Parse-Fehler und wird TROTZDEM ersetzt
// (WHATWG §13.2.5.80) — deshalb klebt das C am B.
```

## L728 · `let p = match &body.children[1] {`

```
// The <p> keeps its inline structure: text, an <a> element, text.
```

## L761 · `assert_eq!(text_of(&as_el(&body.children[1])), "keep");`

```
// The '<' inside the script did not break out into new elements.
```

## L767-769 · `let dom = parse(`

```
// SVG is case-sensitive; the HTML tokenizer is not. Without the spec's
// adjustment an inline icon arrives with `viewbox` and has no
// coordinate system at all.
```

## L779-780 · `assert_eq!(child_tags(as_el(&svg.children[0])), ["path"]);`

```
// The end tag is lowercased too — if it were not adjusted, </clipPath>
// would not close its start tag and the tree would nest wrongly.
```

## L786 · `let div = as_el(&dom.body().children[1]);`

```
// Outside foreign content nothing changes.
```

