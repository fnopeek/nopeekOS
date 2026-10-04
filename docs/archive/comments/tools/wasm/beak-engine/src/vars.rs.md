# `tools/wasm/beak-engine/src/vars.rs` @ 5e0102684

## L1-31 · `use alloc::string::{String, ToString};`

```
//! vars.rs — CSS Custom Properties (`--name`) und `var()`.
//!
//! **Sie werden in der KASKADE aufgeloest, je Element** — `style::resolve_in`
//! sammelt sie aus den Regeln, die dieses Element wirklich treffen, erbt sie
//! vom Elternteil und setzt sie beim Anwenden eines Wertes ein. Hier steht
//! nur noch, was eine Karte IST und wie aus einem Wert ein fertiger wird.
//!
//! Bis 0.59.0 lief davor ein Textlauf ueber das ganze Blatt, mit einer
//! globalen Karte: ein Wert je Name fuer das ganze Dokument. Das traegt genau
//! ein Muster — `:root` setzt eine Palette, alles liest daraus — und bricht
//! bei dem, das jedes moderne Rahmenwerk benutzt: die Basisklasse liest die
//! Variable, jede Variante setzt sie neu.
//!
//!     .btn         { --bs-btn-bg: transparent; background: var(--bs-btn-bg) }
//!     .btn-primary { --bs-btn-bg: #0d6efd }
//!     .btn-link    { --bs-btn-bg: transparent }   <- steht ZULETZT im Blatt
//!
//! `.btn-link` trifft einen `<button class="btn btn-primary">` nie und gewann
//! trotzdem die globale Karte: **jeder Bootstrap-Knopf war durchsichtig.**
//! Dasselbe traf Hinweise, Tabellenstreifen und `list-group .active`.
//!
//! Mit dem Textlauf sind auch seine Heuristiken weg — er musste RATEN, welcher
//! Block „unbedingt" gilt, und tat das an der Selektor-Zeichenkette. Ein
//! Kommentar davor (Bootstraps Kopfzeile, mit Versionsnummer und URLs) reichte,
//! um `:root` fuer bedingt zu halten; dann gewann `[data-bs-theme=dark]`, und
//! die Seite war dunkel, ohne dass irgendwo ein `data-bs-theme` stand. Die
//! Kaskade muss nichts raten: sie TRIFFT.
//!
//! Gemessen hat der Umbau nichts gekostet — auf drei eingefrorenen Seiten
//! 56,6/45,5/56,2 ms vorher gegen 57,5/44,5/47,2 ms nachher. Der Textlauf ueber
//! ein 368-KB-Blatt war eben auch nicht gratis.
```

## L75 · `fn is_var_at(b: &[u8], i: usize) -> bool {`

```
// ── low-level helpers ───────────────────────────────────────────────────────
```

## L77-81 · `fn is_var_at(b: &[u8], i: usize) -> bool {`

```
/// `true` if bytes at `i` spell `var(` (case-insensitive on `var`).
///
/// Das Zeichen DAVOR gehoert zur Frage: ein Funktionsname ist ein ganzer
/// Bezeichner, also ist `notvar(--x)` die Funktion `notvar` und kein `var()`.
/// Ohne die Schranke setzte `expand` dort mitten im Namen ein.
```

## L94 · `fn contains_var(b: &[u8]) -> bool {`

```
/// Cheap scan: does the text contain a `var(` anywhere?
```

## L109 · `fn skip_comment(b: &[u8], i: usize) -> usize {`

```
/// Index one past a `/* … */` comment that starts at `i`.
```

## L116 · `if k + 1 < n {`

```
// Advance past the closing `*/` (or to end if unterminated).
```

## L124 · `fn skip_string(b: &[u8], i: usize) -> usize {`

```
/// Index one past a `"…"` or `'…'` string that starts at `i` (with `\` escapes).
```

## L142 · `fn is_name(c: u8) -> bool {`

```
/// CSS ident byte (ASCII alnum, `-`, `_`, or any non-ASCII / UTF-8 byte).
```

## L151-167 · `pub type VarMap = alloc::vec::Vec<(alloc::rc::Rc<str>, alloc::rc::Rc<str>)>;`

```
// ── Je Element, nicht je Dokument ───────────────────────────────────────────
//
// Frueher lief hier ein Textlauf ueber das ganze Blatt: EINE Karte, ein Wert
// je Name fuer das ganze Dokument. Das traegt das Muster, fuer das es gebaut
// war (`:root` setzt eine Palette), und bricht bei dem, das jedes moderne
// Rahmenwerk benutzt — die Basisklasse liest die Variable, jede Variante
// setzt sie neu:
//
//     .btn         { --bs-btn-bg: transparent; background: var(--bs-btn-bg) }
//     .btn-primary { --bs-btn-bg: #0d6efd }
//     .btn-link    { --bs-btn-bg: transparent }   <- steht ZULETZT im Blatt
//
// `.btn-link` trifft einen `<button class="btn btn-primary">` nie und gewann
// trotzdem: jeder Bootstrap-Knopf war durchsichtig. Eine Custom Property ist
// eine GEERBTE Eigenschaft — sie gehoert in die Kaskade, je Element. Dort
// steht sie jetzt (`style::resolve_in`); hier bleibt nur, was eine Karte ist
// und wie ein Wert daraus entsteht.
```

## L169-173 · `pub type VarMap = alloc::vec::Vec<(alloc::rc::Rc<str>, alloc::rc::Rc<str>)>;`

```
/// Die Custom Properties, die auf einem Element gelten.
///
/// Eine flache Liste und keine Karte: ein Element traegt selten mehr als ein
/// paar Dutzend, und ein linearer Vergleich ueber kurze Namen ist billiger
/// als das Hashen, das eine Karte je Zugriff kostet.
```

## L176-177 · `pub fn has_var(v: &str) -> bool { contains_var(v.as_bytes()) }`

```
/// Steht ein `var()` in diesem Wert? Ein Bytescan, damit der Normalfall —
/// die allermeisten Deklarationen haben keins — nichts kostet.
```

## L182-188 · `if v.trim() == "initial" { return None }`

```
// `--x: initial` macht die Eigenschaft GARANTIERT UNGUELTIG (CSS Variables
// 1 §3.1) — sie gilt als nicht gesetzt, und ein `var(--x, rueckfall)`
// nimmt den Rueckfall. Das ist kein Randfall: Bootstrap schaltet damit
// seine Tabellenfarben um
// (`--bs-table-color-type: initial`, dann `var(--bs-table-color-type,
// var(--bs-table-color))`), und als Zeichenkette gelesen faerbte es jede
// Zelle mit dem Wort „initial".
```

## L193-194 · `pub fn var_set(map: &mut VarMap, name: &str, value: &str) {`

```
/// Setzen oder ersetzen. Ersetzen statt Anhaengen, damit die Liste nicht mit
/// jeder ueberschriebenen Deklaration waechst.
```

## L202-208 · `pub fn expand(value: &str, map: &VarMap, skip: Option<&str>) -> String {`

```
/// `var()` in einem Wert ersetzen, gegen die Karte DIESES Elements.
///
/// `skip` ist der Name, dessen eigener Wert gerade ausgerechnet wird: er darf
/// sich nicht selbst einsetzen. `--x: var(--x, 1rem)` ist die Schreibweise,
/// mit der eine Seite „nimm den geerbten Wert, sonst 1rem" sagt (Wikipedia
/// tut das); wuerde er sich selbst finden, bliebe ein `var()` stehen und die
/// Deklaration waere ungueltig.
```

## L223-225 · `const MAX_PASSES: usize = 16;`

```
/// Deckel gegen Ringe: `--a: var(--b); --b: var(--a)` hoert von selbst nicht
/// auf. Was danach noch ein `var()` traegt, ist ungueltig — und das ist die
/// richtige Antwort, nicht ein erfundener Wert.
```

## L252-254 · `(None, None) => { out.push_str(&input[i..end]); i = end; continue }`

```
// Kein Wert und kein Rueckfall: das `var()` bleibt stehen,
// der Wertparser scheitert daran, und die Deklaration
// faellt weg — CSS Variables 1 §3.
```

## L269 · `#[cfg(test)]`

```
// ── tests ───────────────────────────────────────────────────────────────────
```

## L280-285 · `fn painted_text(html: &str, css: &str) -> Option<(u8, u8, u8)> {`

```
/// Die Farbe, die eine Seite auf ihr erstes Textstueck malt.
///
/// Der kuerzeste ehrliche Weg, einen KASKADIERTEN Wert zu pruefen: er geht
/// durch Parser, Treffer, Kaskade, Vererbung und Einsetzung — also durch
/// alles, was hier zu pruefen ist. Eine Probe direkt auf `expand` sagt
/// ueber die Kaskade nichts.
```

## L295-297 · `fn fills(html: &str, css: &str) -> alloc::vec::Vec<(u8, u8, u8)> {`

```
/// Alle Fuellfarben einer Seite. Eine Liste und kein „die erste": welche
/// Fuellung die Leinwand ist und welche das Element, haengt am Aufbau der
/// Seite — und die Probe soll den WERT pruefen, nicht die Malreihenfolge.
```

## L308-312 · `#[test]`

```
/// Eine Custom Property wird erst beim GEBRAUCH eingesetzt, nicht beim
/// Setzen (css-variables-1 §3). Sonst friert die Regel, die zuerst kommt,
/// den Stand der Kaskade ein — und eine Regel dahinter, die eine benutzte
/// Variable erst setzt, kommt zu spaet. Genau so schreibt Tailwind seine
/// Ringe: die Breite steht vor der Farbe.
```

## L322 · `#[test]`

```
// ── Die Kaskade: WER entscheidet den Wert ───────────────────────────────
```

## L324-329 · `#[test]`

```
/// **Der Fehler, wegen dem die Aufloesung in die Kaskade gezogen wurde.**
///
/// Bis 0.59.0 lief ein Textlauf ueber das ganze Blatt mit einer globalen
/// Karte. `.c` trifft das Element nicht und gewann trotzdem — auf einer
/// echten Seite hiess das: `.btn-link{--bs-btn-bg:transparent}` steht
/// zuletzt im Blatt, und JEDER Bootstrap-Knopf war durchsichtig.
```

## L336-337 · `#[test]`

```
/// Und die Umkehrung: die eigene Regel des Elements gewinnt gegen eine
/// gleichnamige, die woanders steht.
```

## L347-348 · `#[test]`

```
/// Eine Custom Property wird VERERBT — der Kern der Sache, und der Grund,
/// warum sie nicht bloss je Element gilt.
```

## L356-357 · `#[test]`

```
/// Und ein Nachfahre darf sie ueberschreiben, ohne den Vorfahren zu
/// beruehren.
```

## L370-373 · `#[test]`

```
/// Ein Kommentar vor einer Regel gehoert nicht in ihren Selektor —
/// und seit die Kaskade wirklich TRIFFT, kann er es auch nicht mehr.
/// Gemessen an Bootstrap 5.3.3: die Kopfzeile trug Versionsnummer und
/// URLs, und der Dunkelblock gewann die ganze helle Palette.
```

## L383-384 · `#[test]`

```
/// Und wenn das Attribut DA ist, gewinnt der Dunkelblock — sonst waere
/// die Regel darueber bloss ein „nie".
```

## L392-394 · `#[test]`

```
/// MediaWiki liefert eine Definition je Benutzereinstellung und die Seite
/// traegt genau eine davon. Die andere darf nicht gewinnen — frueher
/// brauchte es dafuer eine Heuristik, heute reicht das Treffen.
```

## L402 · `#[test]`

```
/// Spezifitaet schlaegt Reihenfolge, wie bei jeder anderen Eigenschaft.
```

## L409 · `#[test]`

```
/// Ein `@media`, das nicht gilt, liefert auch keine Variablen.
```

## L416 · `#[test]`

```
// ── Die Einsetzung: WAS aus einem Wert wird ─────────────────────────────
```

## L433-435 · `#[test]`

```
/// Kein Wert und kein Rueckfall: das `var()` bleibt stehen, der
/// Wertparser scheitert daran, und die Deklaration faellt weg. Genau das
/// verlangt CSS Variables 1 §3 — ein leerer Wert waere etwas anderes.
```

## L479-482 · `#[test]`

```
/// **Wikipedias Schreibweise.** `--fs: var(--fs, 1rem)` heisst „nimm den
/// geerbten Wert, sonst 1rem". Duerfte sie sich selbst finden, bliebe ein
/// `var()` stehen und die Deklaration waere ungueltig — die Suchleiste
/// verlor daran einmal ihre Lupe.
```

## L488 · `#[test]`

```
/// Ein Ring aus zwei Namen dreht sich nicht ewig.
```

## L502 · `#[test]`

```
/// Die Form, in der Bootstrap seine Palette weiterreicht.
```

