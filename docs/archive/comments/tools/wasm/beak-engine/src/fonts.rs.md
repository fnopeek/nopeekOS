# `tools/wasm/beak-engine/src/fonts.rs` @ 5e0102684

## L1-9 · `use alloc::vec::Vec;`

```
//! fonts.rs — the browser's font faces + per-run face selection.
//!
//! Six embedded faces: Inter Regular/Bold/Italic/BoldItalic for body text
//! (matching the compositor's Inter) and Noto Sans Mono Regular/Bold for
//! `<code>`/`<pre>`/`font-family:monospace`. Layout MEASURES and the raster
//! DRAWS through the same `pick(bold, italic, mono)`, so glyph advances agree
//! with the glyphs actually painted. This replaces the earlier single-face
//! approach that faked weight (a 1px horizontal smear) and slant (a shear) —
//! real faces render correctly and, for monospace, at the right advance width.
```

## L16-26 · `#[derive(Clone, Copy)]`

```
/// A face as the text path needs it: the outlines AND the ligature table that
/// belongs to them.
///
/// `Copy` and `Deref<Target = Font>` on purpose — every existing
/// `font.metrics(…)` and `measure(font, …)` call site keeps working unchanged,
/// and the ligatures ride along to the two places that need them (measuring
/// and painting) instead of being plumbed through thirty signatures.
///
/// **Measuring and painting MUST take the same table.** A run shaped in the
/// painter but not the measurer lands in a box that was never reserved for it
/// ([[feedback_intrinsic_shared_path]]).
```

## L41-43 · `pub fn ligatures(&self) -> Option<&'a Ligatures> {`

```
/// The ligature table, or `None` when this face has none — which is every
/// face we ship (they are subsetted) and most web fonts. The `None` is what
/// lets `measure` keep its allocation-free per-character loop.
```

## L48-54 · `pub fn char_widths(&self, size: f32) -> Option<(f32, f32)> {`

```
/// Die mittlere und die groesste Zeichenbreite bei `size`, aus den echten
/// Tabellen der Schrift (`OS/2.xAvgCharWidth`, `hhea.advanceWidthMax`).
///
/// **Gebraucht fuer die EIGENBREITE eines `<input size=n>` und eines
/// `<textarea cols=n>`** — jeder Browser rechnet sie aus der mittleren
/// Zeichenbreite, nicht aus der Breite der Null. Eine Schrift ohne die
/// Tabellen gibt `None`, und der Rufer nimmt weiter die Null.
```

## L60-62 · `pub fn shape(&self, s: &str) -> Vec<(u16, usize, usize)> {`

```
/// The glyphs this text becomes, each with the BYTE RANGE of the source it
/// covers. The byte ranges are what keeps line breaking working: it walks
/// offsets into the original string, not glyph counts.
```

## L90-92 · `pub struct WebFace {`

```
/// The loaded set of faces. Built once (in `raster::Engine::new`) and shared
/// by reference into layout + paint.
/// Ein Gesicht, das die SEITE mitgebracht hat (`@font-face`).
```

## L94-95 · `pub family: u32,`

```
/// Streuwert des Familiennamens — dieselbe Zahl, die `ComputedStyle`
/// traegt (`style::family_hash`).
```

## L97 · `pub weight: u16,`

```
/// 100..900. Gewaehlt wird das naechstliegende, nicht das gleiche.
```

## L101-102 · `pub lig: Ligatures,`

```
/// Gelesen, wenn die Schrift ankommt: eine Symbolschrift bringt ihre
/// Ligaturen mit, und genau die sind ihr Bild.
```

## L106-122 · `struct LazyFace {`

```
/// Ein eingebautes Gesicht, das erst beim ERSTEN Gebrauch geparst wird.
///
/// **fontdue hat keinen faulen Weg:** `Font::from_bytes` legt den Umriss
/// JEDER cmap-erreichbaren Glyphe an — das sagt `assets/subset.sh` selbst,
/// und deshalb wurde dort schon von 19 516 auf 14 524 Glyphen gekuerzt.
/// Gemessen kostet das **6,6 bis 7,0 MB Halde je Gesicht** (host-seitig,
/// `examples/heapcheck.rs`), bei sechs Gesichtern 40 MB — die beak beim START
/// bezahlte, obwohl eine gewoehnliche Seite zwei davon anfasst.
///
/// Am Geraet war das der groesste Posten ueberhaupt: `beakbench` misst die
/// Halde bei `instantiate` mit 11 MiB und nach dem Schriftrastern mit 89 MiB;
/// das Layout selbst legt nichts mehr drauf. Und weil talc Seiten nie
/// zurueckgibt, wurde diese Spitze zum Dauerbedarf.
///
/// `OnceCell` und kein `RefCell`: gelesen wird ueber `&self` (siehe `pick`),
/// und ein Gesicht wird genau einmal gebaut. Einen Faden gibt es nicht —
/// `Fonts` liegt einzeln in `Engine`.
```

## L136-140 · `let settings = FontSettings { load_substitutions: false, ..FontSettings::default() };`

```
// `load_substitutions` only feeds fontdue's glyph-INDEX API — it
// makes the ligature glyphs rasterisable, it does not substitute
// (fontdue has no shaper; `gsub.rs` does that part). The six
// embedded faces are subsetted and carry no GSUB at all, so
// leaving it on here would only outline dead glyphs.
```

## L156-157 · `web: Vec<WebFace>,`

```
/// Die Gesichter der Seite. Werden EINGEHAENGT, nicht ersetzt: die
/// eingebauten bleiben die Ersatzkette.
```

## L186-187 · `pub fn loaded_faces(&self) -> usize {`

```
/// Wieviele der sechs eingebauten Gesichter wirklich geparst wurden.
/// Gehoert ins Log: „faul" ist sonst eine Behauptung.
```

## L194-198 · `pub fn add_web(&mut self, family: u32, weight: u16, italic: bool, bytes: &[u8]) -> bool {`

```
/// The face for a run's style. Monospace ships no italic face (Noto Sans
/// Mono has none), so `mono + italic` renders upright mono — rare
/// (`<code><i>`), and better than mixing a proportional italic into code.
/// Eine Schrift der Seite aufnehmen. Liefert false, wenn die Bytes keine
/// lesbare Schrift sind — der Rufer meldet das, statt es zu verschlucken.
```

## L200-203 · `let settings = FontSettings { load_substitutions: true, ..FontSettings::default() };`

```
// **HIER ist `load_substitutions` noetig**, anders als bei den
// eingebauten Gesichtern: eine Seitenschrift bringt ihr GSUB mit, und
// ohne diese Zeile gaebe es die Ligaturglyphe zwar im Baum, aber
// keinen Umriss zum Rastern.
```

## L217 · `pub fn has_web(&self, family: u32) -> bool {`

```
/// Hat die Seite diese Familie mitgebracht?
```

## L222-226 · `fn web_pick(&self, family: u32, bold: bool, italic: bool) -> Option<Face<'_>> {`

```
/// Das beste Gesicht dieser Familie fuer Gewicht und Neigung.
///
/// **Naechstliegend, nicht gleich.** Eine Seite laedt oft nur Regular und
/// Bold und verlangt trotzdem 600 — dann ist Bold die Antwort, nicht die
/// eingebaute Schrift.
```

## L232-234 · `let cost = (w.weight as i32 - want as i32).abs()`

```
// Die Neigung wiegt schwerer als das Gewicht: ein kursives
// Gesicht durch ein aufrechtes zu ersetzen sieht falscher aus als
// ein Strich zu duenn.
```

## L254-256 · `pub fn regular(&self) -> Face<'_> {`

```
/// The regular body face — for size-agnostic estimates (line-box height
/// around floats, intrinsic auto-sizing) where a single reference face is
/// fine and keeps behaviour identical to the old single-font path.
```

## L261-266 · `pub fn face_key(bold: bool, italic: bool, mono: bool, family: u32) -> u32 {`

```
/// A stable id per face — mixed into the raster's glyph-cache key so two
/// faces never collide on the same `(char, size)`.
/// Ein stabiler Schluessel je Gesicht — geht in den Glyphenspeicher, damit
/// zwei Gesichter sich bei `(Zeichen, Groesse)` nicht ins Gehege kommen.
/// Die Familie MUSS mit hinein: sonst malte die zweite Seitenschrift die
/// Glyphen der ersten.
```

## L288-291 · `#[test]`

```
/// **Die eingebauten Gesichter haben KEINE Ligaturen** — sie sind
/// gekuerzt (`assets/subset.sh`). Das ist keine Nebenbemerkung: daran
/// haengt, dass `measure` seinen alten, allokationsfreien Weg nimmt,
/// und diese Behauptung soll gemessen sein statt geglaubt.
```

## L301-303 · `#[test]`

```
/// Ohne Ligaturen ist `shape` die Identitaet: eine Glyphe je Zeichen, und
/// die Byte-Spannen decken die Zeichenkette luecken- und ueberlappungsfrei
/// ab. Der Zeilenumbruch laeuft auf diesen Spannen.
```

