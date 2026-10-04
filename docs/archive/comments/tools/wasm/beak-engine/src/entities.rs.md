# `tools/wasm/beak-engine/src/entities.rs` @ 5e0102684

## L1-17 · `pub const MAX_NAME: usize = 31;`

```
//! Die benannten Zeichenverweise von HTML (WHATWG §13.5) — ALLE.
//!
//! **Erzeugt, nicht getippt**, aus `https://html.spec.whatwg.org/entities.json`.
//! Die Liste ist abgeschlossen und normiert; eine handverlesene Auswahl waere
//! eine Schaetzung darueber, was eine Seite schreiben darf
//! ([[feedback_invented_limits]]). Vorher kannte beak fuenfzehn Namen, und
//! jeder andere stand als Text auf der Seite: DuckDuckGos Vorlage fuer
//! „Searches related to" schreibt `&ZeroWidthSpace;`, und das erschien zehnmal
//! woertlich in der Randspalte.
//!
//! Zwei Tabellen, weil die Spezifikation zwei Faelle kennt:
//! * `NAMED` — mit `;`, 2125 Eintraege. Der Normalfall.
//! * `LEGACY` — die 106 Namen, die AUCH ohne `;` gelten (`&copy 2026`).
//!   Nach Laenge absteigend sortiert: es gilt die LAENGSTE Uebereinstimmung.
//!
//! Ein Ersatz kann ZWEI Codepunkte haben (93 Namen, z. B. `&NotEqualTilde;`
//! = `\u{2242}\u{338}`), deshalb steht rechts eine Zeichenkette und kein `char`.
```

## L19-20 · `pub const MAX_NAME: usize = 31;`

```
/// Der laengste Name in `NAMED` (31 Zeichen) — die Abbruchgrenze des
/// Lesers. Ohne sie liest er bei einem `&` ohne `;` beliebig weit.
```

## L23 · `static NAMED: &[(&str, &str)] = &[`

```
/// Nach Namen sortiert; `lookup` sucht binaer.
```

## L559-560 · `static LEGACY: &[(&str, &str)] = &[`

```
/// Die Altlast: gueltig auch OHNE `;`. Nach Laenge absteigend, damit die
/// erste Uebereinstimmung die laengste ist.
```

## L591-592 · `pub fn lookup(name: &str) -> Option<&'static str> {`

```
/// Der Ersatz fuer `&name;`. Gross- und Kleinschreibung zaehlt: `&Beta;` und
/// `&beta;` sind zwei verschiedene Zeichen.
```

## L597-598 · `pub fn longest_legacy(s: &str) -> Option<(&'static str, usize)> {`

```
/// Die laengste Altlast-Uebereinstimmung am Anfang von `s` (ohne `&`).
/// Liefert `(Ersatz, wieviele Bytes der Name lang war)`.
```

## L609 · `assert_eq!(super::lookup("ZeroWidthSpace"), Some("\u{200b}"));`

```
// Die eine, an der DuckDuckGos Randspalte haengt.
```

## L613 · `assert_eq!(super::lookup("NotEqualTilde"), Some("\u{2242}\u{338}"));`

```
// Zwei Codepunkte auf der rechten Seite.
```

## L615 · `assert_ne!(super::lookup("Beta"), super::lookup("beta"));`

```
// Gross/klein sind zwei Zeichen, kein Schreibfehler.
```

## L622 · `assert_eq!(super::longest_legacy("not;"), Some(("\u{ac}", 3)));`

```
// `&notin` waere `\u{ac}in`, wenn `not` zuerst traefe.
```

