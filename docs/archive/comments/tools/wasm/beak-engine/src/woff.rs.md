# `tools/wasm/beak-engine/src/woff.rs` @ 5e0102684

## L1-18 · `use alloc::vec;`

```
//! WOFF 1.0 → sfnt. Der Vorgaenger von WOFF2, und er ist NICHT ausgestorben.
//!
//! Hier stand ein `return false` mit der Begruendung, die Fassung sei
//! „praktisch ausgestorben". Gemessen: arcade.ch liefert alle vier Gesichter
//! seiner Hausschrift als WOFF1, und ohne sie faellt die ganze Seite auf die
//! eingebaute Schrift zurueck — also stimmt darunter keine einzige Breite und
//! keine einzige Hoehe mehr ([[feedback_a_comment_that_names_its_condition_expires]]).
//! Jeder Baukasten, der noch `.woff` neben `.woff2` ausliefert, trifft uns
//! genauso, sobald `@font-face` beide Formate anbietet und wir das zweite
//! nicht koennen.
//!
//! **Warum das kurz ist.** WOFF2 muss `glyf`/`loca` zurueckbauen und bringt
//! Brotli mit; WOFF1 tut nichts dergleichen. Es ist das sfnt selbst, Tabelle
//! fuer Tabelle mit zlib gepackt (RFC 1950) — und `miniz_oxide` liegt fuer PNG
//! ohnehin im Baum. Es bleibt: Kopf lesen, Verzeichnis lesen, entpacken,
//! sfnt wieder zusammensetzen.
//!
//! W3C WOFF File Format 1.0, §3 (header) und §4 (table directory).
```

## L23-24 · `const MAX_SFNT: usize = 32 * 1024 * 1024;`

```
/// Derselbe Deckel wie in `woff2` — eine Schrift, die entpackt groesser ist,
/// wird abgelehnt, statt den Speicher zu fuellen.
```

## L27 · `const HDR: usize = 44;`

```
/// Kopf (44 B) + je 20 B Verzeichniseintrag.
```

## L42-43 · `pub fn to_sfnt(d: &[u8]) -> Option<Vec<u8>> {`

```
/// `wOFF` → sfnt, oder `None`, wenn der Container nicht haelt, was sein Kopf
/// sagt.
```

## L54-56 · `let mut tables: Vec<(u32, Vec<u8>)> = Vec::new();`

```
// `totalSfntSize` ist eine ANGABE der Datei, keine Messung. Sie wird
// benutzt, um EINMAL zu reservieren, und danach nicht mehr geglaubt: die
// Groesse, die zaehlt, ist die aufaddierte der Tabellen.
```

## L74-76 · `let raw = if comp >= orig {`

```
// §4: `compLength == origLength` heisst UNGEPACKT. Alles andere ist
// zlib. Ein Entpacker auf rohe Tabellenbytes losgelassen scheitert
// sonst an `head`, das fast immer unkomprimiert daliegt.
```

## L91-92 · `tables.sort_by_key(|(t, _)| *t);`

```
// Das sfnt-Verzeichnis ist nach Marke SORTIERT — die Spezifikation
// verlangt das auch von der WOFF-Datei, aber eine Datei ist keine Zusage.
```

## L100-101 · `let mut sel = 0u16;`

```
// searchRange / entrySelector / rangeShift — die drei Suchhilfen aus dem
// sfnt-Kopf. fontdue liest sie nicht, ein anderer Leser schon.
```

## L126-130 · `fn checksum(d: &[u8]) -> u32 {`

```
/// sfnt-Pruefsumme: die u32 der Tabelle aufaddiert, mit Nullen aufgefuellt.
///
/// Neu gerechnet und nicht aus der WOFF-Datei uebernommen — dort steht die
/// des ORIGINALS, und wenn die Datei sich irrt, faellt der Fehler sonst
/// einem Leser vor die Fuesse, der die Summe prueft.
```

