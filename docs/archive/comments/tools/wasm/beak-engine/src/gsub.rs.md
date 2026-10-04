# `tools/wasm/beak-engine/src/gsub.rs` @ 5e0102684

## L1-20 · `use alloc::vec::Vec;`

```
//! GSUB ligature substitution — the step between "which characters" and
//! "which glyphs".
//!
//! **Why it exists.** An icon font does not map its symbol to a codepoint; it
//! maps the WORD to one glyph, as a ligature. `<i class="fos-icon">home</i>`
//! carries four characters that are each an empty glyph, and the picture is
//! the ligature of all four. Without this, that element measured 1 px where a
//! browser gives it 24.
//!
//! **Why fontdue does not do it.** `FontSettings::load_substitutions` only
//! makes the ligature GLYPHS rasterisable by index — fontdue has no shaper and
//! says so ("singular characters do not have enough context to be
//! substituted"). The substitution itself is ours.
//!
//! Only ligatures (GSUB lookup type 4), and only from the features that are ON
//! by default: `liga`, `clig`, `rlig`. `dlig` (discretionary) and `hlig`
//! (historical) are opt-in through `font-variant-ligatures`, and applying them
//! unasked would change body text nobody asked to change. Contextual lookups
//! (types 5–8) are not applied — an icon font does not use them, and half a
//! shaper is worse than none.
```

## L24-28 · `#[derive(Default)]`

```
/// The ligature substitutions of ONE face, keyed by the first component glyph.
///
/// Sorted by that first glyph so a lookup is a binary search: the common case
/// is a run whose glyphs are not the start of any ligature, and that case has
/// to cost almost nothing.
```

## L32-40 · `avg_char: f32,`

```
/// Die mittlere und die groesste Zeichenbreite der Schrift, in Em.
///
/// **Sie stehen hier, weil hier schon die echten Tabellen gelesen
/// werden** — `fontdue` wertet weder `OS/2` noch `hhea` aus, und ein
/// zweiter Leser fuer dieselben Bytes waere eine zweite Wahrheit ueber
/// dieselbe Schrift. Gebraucht werden sie fuer die EIGENBREITE eines
/// `<input size=n>` und eines `<textarea cols=n>`: die rechnet jeder
/// Browser aus der mittleren Zeichenbreite, nicht aus der Breite der
/// Null — gemessen ueber fuenf Stuetzstellen von `size=1` bis `size=40`.
```

## L46 · `rest: Vec<u16>,`

```
/// The components AFTER the first one.
```

## L56-60 · `pub fn apply(&self, glyphs: &[u16], at: usize) -> Option<(u16, usize)> {`

```
/// The longest ligature that starts at `glyphs[at]`, as
/// `(ligature glyph, how many glyphs it consumes)`.
///
/// Longest-first: a font with both `ffi` and `ff` must take `ffi` where it
/// applies, or `ffi` never fires at all.
```

## L80-83 · `pub fn avg_char(&self) -> f32 { self.avg_char }`

```
/// Read the ligature substitutions out of a font's GSUB table. A font with
/// none — every subsetted face we ship — yields an empty table, and an
/// empty table is what makes the fast path in `measure` legal.
/// Die mittlere Zeichenbreite in Em (`OS/2.xAvgCharWidth`), oder 0.
```

## L85 · `pub fn max_char(&self) -> f32 { self.max_char }`

```
/// Die groesste Zeichenbreite in Em (`hhea.advanceWidthMax`), oder 0.
```

## L91-93 · `let upem = face.units_per_em() as f32;`

```
// **Zuerst die zwei Breiten** — sie haengen nicht am GSUB, und eine
// Schrift ohne Ligaturen (jede, die wir mitliefern) verlaesst die
// Funktion gleich darunter.
```

## L97-101 · `let be16 = |t: &[u8], at: usize| -> Option<u16> {`

```
// `OS/2` Feld 2 (Offset 2, i16) und `hhea` Feld `advanceWidthMax`
// (Offset 34, u16). `ttf-parser` 0.21 gibt beide nicht als
// Methode heraus, die Tabellen selbst aber schon — gelesen wird
// mit Laengenpruefung, eine kurze Tabelle gibt 0 und der Rufer
// faellt auf die Breite der Null zurueck.
```

## L111-116 · `let bb = face.global_bounding_box();`

```
// **Die groesste Zeichenbreite ist die des UMRISSKASTENS der
// ganzen Schrift**, nicht `hhea.advanceWidthMax`. Mit dem Vorschub
// blieb ein konstanter Versatz von 37 px ueber ALLE
// Stuetzstellen — und ein Fehler, der sich mit der Groesse nicht
// aendert, sitzt im konstanten Glied. Blink nimmt an dieser Stelle
// `xMax - xMin` aus `head`, und damit geht die Rechnung auf.
```

## L123 · `let mut wanted: Vec<u16> = Vec::new();`

```
// Which lookups belong to a default-on ligature feature.
```

## L135-138 · `let all = wanted.is_empty();`

```
// A font whose feature list names none of the three still gets its
// ligature lookups read. Icon fonts are routinely built with a bare
// GSUB and no feature record at all, and refusing them here would be
// a standards-correct answer to the wrong question.
```

## L148 · `for (set_index, set) in ls.ligature_sets.into_iter().enumerate() {`

```
// The coverage index of the FIRST component picks the set.
```

## L154 · `continue; // a one-component "ligature" substitutes nothing`

```
// a one-component "ligature" substitutes nothing
```

## L173-174 · `fn coverage_glyph(cov: &ttf_parser::opentype_layout::Coverage, i: u16) -> Option<u16> {`

```
/// The glyph at coverage index `i` — the inverse of `Coverage::get`, which the
/// table does not offer because a ligature set is addressed BY that index.
```

## L179-180 · `Coverage::Format2 { records } => {`

```
// In a range record `value` IS the coverage index of `start`
// (OpenType: startCoverageIndex).
```

## L213-214 · `#[test]`

```
/// `ffi` and `ff` both start at `f`; the LONGER one has to win, or `ffi`
/// can never fire.
```

## L222 · `assert_eq!(t.apply(&[5, 1, 1], 1), Some((100, 2)));`

```
// …and it applies at any offset, not just the start.
```

## L233-234 · `#[test]`

```
/// A run that ends mid-ligature keeps its glyphs: `f` at the very end of a
/// string is an `f`, not half an `ff`.
```

