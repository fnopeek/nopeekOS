# `tools/wasm/beak-engine/src/select.rs` @ 5e0102684

## L1-27 · `use alloc::string::String;`

```
//! Text auf der Seite markieren — und damit kopieren und finden.
//!
//! **Warum das fehlte und warum es zaehlt.** Nach „Links anklicken" ist
//! Markieren und Kopieren die meistbenutzte Handlung in einem Browser
//! ueberhaupt, und beak konnte sie nicht: `grep` ueber die Shell zaehlte
//! null Treffer (`docs/plan/BROWSER_TABS.md` §B4). Die Adresszeile kann es
//! seit 0.150.0 — die Seite ist ein Bild auf einer Leinwand, und ein Bild
//! markiert man nicht.
//!
//! **Die Vorarbeit war schon da.** Die Anzeigeliste ist keine Pixelwand: sie
//! ist eine Folge von `DrawOp::Text`, jeder mit Ort, Groesse, Schriftwahl und
//! seinem Text. Damit ist „welcher Buchstabe liegt unter diesem Punkt" eine
//! Rechnung und keine Suche — und dieselbe Rechnung rueckwaerts gibt die
//! Rechtecke zum Hervorheben.
//!
//! Die Masse gehoeren dem MOTOR, nicht dem Layout: eine Textbreite haengt am
//! Gesicht, und die Gesichter haelt `Engine`. Deshalb steht die
//! Schnittstelle dort (`Engine::text_pos_at` und Nachbarn) und hier nur die
//! Rechnung.
//!
//! **Reihenfolge = Anzeigeliste.** Ein Bereich ist ein Paar aus Orten, und
//! „von … bis" braucht eine Ordnung. Genommen wird die der Anzeigeliste, und
//! die ist fuer Text die Dokumentreihenfolge — nicht, weil das immer stimmt
//! (ein `position:absolute`-Kasten wird spaeter gemalt und steht vielleicht
//! weiter oben), sondern weil es die einzige Ordnung ist, die es umsonst
//! gibt. Was dabei herauskommt, ist die Reihenfolge, in der die Seite
//! GEZEICHNET wird; fuer gewoehnlichen Fliesstext ist das die gelesene.
```

## L35-41 · `#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]`

```
/// Ein Ort im Text der Seite: der wievielte Zeichenbefehl, und das wievielte
/// Byte darin.
///
/// `Ord` vergleicht erst den Befehl, dann das Byte — genau die Ordnung, die
/// „von hier bis dort" braucht. Ein Bereich wird deshalb nie verkehrt herum
/// gehalten, sondern beim Gebrauch sortiert: wer nach links zieht, meint
/// dasselbe wie wer nach rechts zieht.
```

## L48 · `struct Run<'a> {`

```
/// Ein Zeichenbefehl, aufbereitet: sein Kasten und was zum Messen noetig ist.
```

## L76 · `fn width(fonts: &Fonts, r: &Run, s: &str) -> f32 {`

```
/// Breite eines Stuecks in diesem Lauf.
```

## L87-93 · `pub fn text_pos_at(fonts: &Fonts, lay: &Layout, x: i32, y: i32) -> Option<TextPos> {`

```
/// Der Ort im Text unter `(x, y)` in Dokumentkoordinaten.
///
/// Trifft der Punkt keinen Lauf, wird der NAECHSTE genommen — erst senkrecht
/// (welche Zeile), dann waagerecht (welches Ende). Das ist kein
/// Entgegenkommen, sondern die Bedingung dafuer, dass Ziehen funktioniert:
/// wer ueber den rechten Rand hinauszieht, meint „bis zum Zeilenende", und
/// wer in den Rand zwischen zwei Absaetzen faehrt, meint einen von beiden.
```

## L98-99 · `let vdist = |r: &Run| -> i32 {`

```
// Abstand einer Zeile zum Zeiger, senkrecht. 0 heisst: der Punkt liegt
// darin.
```

## L105 · `let line: Vec<&Run> = rs.iter().filter(|r| vdist(r) == best).collect();`

```
// Alle Laeufe DIESER Zeile, in Anzeigereihenfolge.
```

## L108-109 · `let mut chosen = line[0];`

```
// Waagerecht: der Lauf, der den Punkt enthaelt; sonst der letzte, der
// davor beginnt; sonst der erste.
```

## L119-123 · `fn byte_at(fonts: &Fonts, r: &Run, x: i32) -> u32 {`

```
/// Das Byte, an dem der Punkt in DIESEM Lauf liegt.
///
/// Gerundet auf die naechste Zeichengrenze: wer auf die linke Haelfte eines
/// Buchstabens zeigt, meint davor. So macht es jeder Editor, und ohne das
/// laesst sich das erste Zeichen einer Zeile nicht mitnehmen.
```

## L138 · `pub fn selection_rects(fonts: &Fonts, lay: &Layout, a: TextPos, b: TextPos)`

```
/// Die Rechtecke, die den Bereich hervorheben — Dokumentkoordinaten.
```

## L159-166 · `pub fn selected_text(lay: &Layout, a: TextPos, b: TextPos) -> String {`

```
/// Der ausgewaehlte Text.
///
/// Zwischen zwei Laeufen steht ein Zeilenumbruch, wenn sie auf
/// verschiedenen Zeilen liegen, sonst nichts — ein Absatz kommt aus mehreren
/// Laeufen (fett, kursiv, ein Link mittendrin), und dazwischen gehoert kein
/// Trenner. Ein Leerzeichen zu setzen waere falsch: die Laeufe tragen ihre
/// Leerzeichen selbst, und `wortA` + `wortB` sind im Original vielleicht
/// `wortAwortB`.
```

## L185-192 · `pub fn find_all(lay: &Layout, needle: &str) -> Vec<(TextPos, TextPos)> {`

```
/// Alle Vorkommen von `needle` im Text der Seite, ohne Ruecksicht auf Gross-
/// und Kleinschreibung — als Bereiche, die `selection_rects` hervorheben kann.
///
/// **Je Lauf, nicht ueber die ganze Seite.** Ein Wort, das eine Zeile
/// ueberschreitet oder mitten im Wort fett wird, faellt damit durch. Das ist
/// die ehrliche Grenze dieser Fassung, und sie ist benannt statt versteckt:
/// laufuebergreifend zu suchen hiesse, den Text zusammenzusetzen und die
/// Rueckabbildung auf Laeufe zu fuehren — machbar, aber eine andere Groesse.
```

## L198-201 · `let low = r.text.to_lowercase();`

```
// Kleinschreiben kann die BYTELAENGE aendern (ẞ → ß ist gleich lang,
// aber İ → i̇ nicht), und dann zeigen die Fundstellen ins Leere.
// Deshalb nur dann ueber die kleingeschriebene Fassung suchen, wenn
// sie dieselbe Laenge hat — sonst zeichengenau vergleichen.
```

## L231-232 · `#[test]`

```
/// Der Punkt trifft das Zeichen, auf das er zeigt — und die Rueckreise
/// gibt denselben Text her.
```

## L236 · `let runs = super::runs(&l);`

```
// Ganz links im Absatz: vor dem H.
```

## L242 · `let b = eng.text_pos_at(&l, r.x + 10_000, r.y + 4).expect("Ende");`

```
// Weit rechts: hinter dem letzten.
```

## L248-249 · `#[test]`

```
/// Rueckwaerts gezogen ist derselbe Bereich. Ohne das gibt jede Auswahl
/// von rechts nach links nichts her.
```

## L260-261 · `#[test]`

```
/// Ueber zwei Absaetze hinweg kommt ein Zeilenumbruch dazwischen — aber
/// NICHT zwischen zwei Laeufen derselben Zeile.
```

## L271 · `let (eng2, l2) = lay("<body><p>a<b>b</b>c</p></body>");`

```
// Fett mitten im Satz: ein Absatz, mehrere Laeufe, KEIN Umbruch.
```

## L281 · `#[test]`

```
/// Die Hervorhebung liegt AUF dem Text, nicht daneben.
```

## L287 · `let b = TextPos { op: r.idx, off: 5 };   // "Hallo"`

```
// "Hallo"
```

## L294 · `let all = eng.selection_rects(&l, a, TextPos { op: r.idx, off: r.text.len() as u32 });`

```
// "Hallo" ist kuerzer als "Hallo Welt".
```

## L299 · `#[test]`

```
/// Suchen ist gross-/kleinschreibungsblind und findet jedes Vorkommen.
```

## L312 · `#[test]`

```
/// Ein Umlaut ist zwei Bytes — eine Auswahl darf nie mitten hinein.
```

## L317-318 · `let w = super::width(&eng_fonts(&eng), r, r.text) as i32;`

```
// Jeden Pixel des Laufs abtasten: jeder Offset muss eine
// Zeichengrenze sein, sonst schneidet `selected_text` in ein Zeichen.
```

