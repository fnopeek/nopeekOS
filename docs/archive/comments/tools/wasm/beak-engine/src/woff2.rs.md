# `tools/wasm/beak-engine/src/woff2.rs` @ 5e0102684

## L1-21 · `use alloc::vec;`

```
//! WOFF2 → sfnt (TrueType). Der Container, den moderne Seiten fuer ihre
//! Schriften benutzen.
//!
//! **Warum das sein muss.** `@font-face` steht praktisch auf jeder modernen
//! Seite, und die Datei dahinter ist fast immer WOFF2. Ohne diesen Schritt
//! faellt jeder Text auf die eingebaute Schrift zurueck — mit zwei Folgen,
//! die beide nach einem Layoutfehler aussehen und keiner sind: eine
//! Symbolschrift malt ihren Ligaturtext (`eye` statt eines Auges), und JEDE
//! Textbreite weicht ab, also stimmt darunter keine einzige Hoehe mehr.
//!
//! **Was hier NICHT gebaut wurde: Brotli.** Der Entpacker ist
//! `brotli-decompressor` mit `default-features = false` — dieselbe
//! Entscheidung wie bei WebP (`super::webp`): die Kiste ist ohne `std`
//! baubar, und RFC 7932 samt seinem 122-KB-Woerterbuch nachzubauen waere
//! mehr Arbeit als der ganze Rest dieser Datei.
//!
//! **Umfang.** Der `glyf`/`loca`-Rueckbau ist vollstaendig (das ist der
//! eigentliche Gewinn von WOFF2 und in echten Schriften immer aktiv), ebenso
//! die `hmtx`-Rueckwandlung. Schriftsammlungen (`ttcf`) und die
//! Metadatenbloecke werden uebergangen — beide kommen als `@font-face` nicht
//! vor.
```

## L26-28 · `const KNOWN_TAGS: [&[u8; 4]; 63] = [`

```
/// Die 63 Tabellennamen, die WOFF2 als Index statt als Marke schreibt.
/// **Reihenfolge ist Vertrag** — ein einziger Eintrag zu viel verschiebt
/// alles danach, und der Datenstrom laeuft danach aus dem Tritt.
```

## L40-42 · `const MAX_SFNT: usize = 32 * 1024 * 1024;`

```
/// Deckel: eine Schrift, die entpackt groesser ist, wird abgelehnt statt den
/// Speicher zu fuellen. 32 MB ist weit ueber jeder echten Schrift (die
/// groesste hier: 285 KB).
```

## L65 · `fn base128(&mut self) -> Option<u32> {`

```
/// `UIntBase128` — 1 bis 5 Bytes, sieben Bit je Byte, hohes Bit = weiter.
```

## L70 · `if i == 0 && b == 0x80 { return None }`

```
// Fuehrende Null ist verboten, und mehr als 32 Bit auch.
```

## L78 · `fn u255(&mut self) -> Option<u16> {`

```
/// `255UInt16` — der Kurzcode fuer kleine Zahlen.
```

## L91 · `stored: usize,`

```
/// Laenge im Datenstrom (transformiert, wenn transformiert).
```

## L97-102 · `#[derive(Clone, Copy)]`

```
/// Eine WOFF2-Datei in eine sfnt-Datei verwandeln, die fontdue lesen kann.
/// Wo ein Rueckbau stehengeblieben ist.
///
/// Ein Entpacker, der nur `None` liefert, kostet auf einer 90-KB-Schrift eine
/// Stunde. `step` sagt die STELLE, `glyph` die Glyphe — beides wird im
/// Vorbeigehen gesetzt, nicht nachtraeglich rekonstruiert.
```

## L140 · `let transformed = if &tag == b"glyf" || &tag == b"loca" { tv != 3 } else { tv != 0 };`

```
// Bei `glyf`/`loca` ist Fassung 3 die Null-Umwandlung, sonst Fassung 0.
```

## L154 · `let mut raw: Vec<&[u8]> = Vec::with_capacity(dir.len());`

```
// Die Tabellen in Verzeichnisreihenfolge aus dem Strom schneiden.
```

## L162-163 · `let mut out: Vec<(([u8; 4]), Vec<u8>)> = Vec::with_capacity(dir.len());`

```
// Rueckbau. `glyf` und `loca` gehoeren zusammen: der Rueckbau des einen
// erzeugt das andere.
```

## L167 · `if &e.tag == b"loca" { continue }          // faellt mit glyf an`

```
// faellt mit `glyf` an
```

## L182 · `if e.transformed { return None }           // unbekannte Umwandlung`

```
// unbekannte Umwandlung
```

## L200-201 · `fn build_sfnt(flavor: u32, mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {`

```
/// Die sfnt-Datei zusammensetzen: Kopf, Tabellenverzeichnis (nach Marke
/// sortiert, so will es das Format), dann die Daten auf 4 Byte ausgerichtet.
```

## L246-247 · `fn reconstruct_hmtx(t: &[u8], hhea: &[u8], head: &[u8], _glyf: Option<&[u8]>) -> Option<Vec<u8>> {`

```
/// `hmtx`-Umwandlung 1: die linken Seitenlager stehen nicht in der Datei,
/// weil sie gleich `xMin` der Glyphe sind.
```

## L253-255 · `if flags & 0x03 == 0 { return Some(t[1..].to_vec()) }`

```
// Bit 0: lsb fehlt, Bit 1: leftSideBearing der Nicht-Metrik-Glyphen fehlt.
// Ohne `glyf` koennen wir sie nicht ausrechnen — dann lieber absagen als
// Nullen erfinden.
```

## L264 · `fn reconstruct_glyf(t: &[u8], tr: &mut Trace) -> Option<(Vec<u8>, Vec<u8>)> {`

```
/// Der `glyf`-Rueckbau (WOFF2 §5.1). Liefert `(glyf, loca)`.
```

## L286-287 · `let bitmap_len = (num_glyphs + 7) / 8;`

```
// Der bbox-Strom beginnt mit einer Bitmaske: ein Bit je Glyphe, „hat eine
// eigene Umgrenzung".
```

## L291-292 · `let overlap = if option_flags & 1 != 0 {`

```
// Die Fahne fuer ueberlappende Konturen ist eine spaetere Zutat und steht
// GANZ hinten, nicht in einem der sieben Stroeme.
```

## L306 · `offsets.push(start as u32);`

```
// Leere Glyphe: kein Eintrag, nur derselbe Versatz noch einmal.
```

## L312-313 · `tr.step = "zusammengesetzte Glyphe";`

```
// Zusammengesetzt: die Rohform steht schon im Verbundstrom, sie
// muss nur abgegrenzt werden.
```

## L322 · `if f & 0x0001 != 0 { composite.take(4)?; } else { composite.take(2)?; }`

```
// ARG_1_AND_2_ARE_WORDS
```

## L324 · `if f & 0x0008 != 0 { composite.take(2)?; }        // WE_HAVE_A_SCALE`

```
// WE_HAVE_A_SCALE
```

## L325 · `else if f & 0x0040 != 0 { composite.take(4)?; }   // X_AND_Y_SCALE`

```
// X_AND_Y_SCALE
```

## L326 · `else if f & 0x0080 != 0 { composite.take(8)?; }   // TWO_BY_TWO`

```
// TWO_BY_TWO
```

## L327 · `if f & 0x0100 != 0 { have_instr = true; }         // WE_HAVE_INSTRUCTIONS`

```
// WE_HAVE_INSTRUCTIONS
```

## L328 · `if f & 0x0020 == 0 { break }                      // MORE_COMPONENTS`

```
// MORE_COMPONENTS
```

## L366-367 · `while glyf.len() % 2 != 0 { glyf.push(0); }`

```
// Jede Glyphe endet auf einer geraden Adresse — `loca` im kurzen
// Format kann nur gerade Versaetze ausdruecken.
```

## L394-395 · `fn triplets(flags: &mut Reader, glyph: &mut Reader, n: usize)`

```
/// Die Punkte einer einfachen Glyphe aus der Dreiergruppen-Kodierung
/// (WOFF2 §5.2). Liefert (Auf-der-Kurve-Fahnen, x, y) in ABSOLUTEN Koordinaten.
```

## L443-450 · `fn emit_points(out: &mut Vec<u8>, on: &[bool], xs: &[i16], ys: &[i16], overlap: bool) {`

```
/// Die Punkte als TrueType schreiben — in der KOMPAKTEN Form.
///
/// Der erste Entwurf schrieb jede Fahne einzeln und jede Differenz als 16
/// Bit. Derselbe Umriss, aber ein Drittel groesser — und genau daran ist die
/// Symbolschrift gescheitert: ihr `loca` steht im KURZEN Format, das nur
/// gerade Versaetze bis 128 KB ausdruecken kann, und die aufgeblaehte Tabelle
/// lief darueber. Eine Abkuerzung im Format ist eben keine Abkuerzung
/// ([[feedback_a_workaround_is_the_wrong_answer_to_a_missing_capability]]).
```

## L456 · `const X_SAME: u8 = 0x10;   // bei X_SHORT: Vorzeichen, sonst „unveraendert"`

```
// bei X_SHORT: Vorzeichen, sonst „unveraendert"
```

## L471 · `if i == 0 && overlap { f |= OVERLAP; }`

```
// OVERLAP_SIMPLE gehoert laut Spezifikation auf den ERSTEN Punkt.
```

## L493-494 · `let mut i = 0;`

```
// Gleiche Fahnen zusammenfassen. Der Zaehler ist ein Byte, also hoechstens
// 255 Wiederholungen je Lauf.
```

## L512 · `use brotli_decompressor::{Allocator, SliceWrapper, SliceWrapperMut};`

```
// ── Brotli ──────────────────────────────────────────────────────────────
```

