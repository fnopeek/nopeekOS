# `tools/wasm/beak-engine/tests/fixtures.rs` @ 5e0102684

## L1-13 · `fn selector(class: &str) -> String {`

```
//! Jede Klasse in einer Vorlage muss im Blatt auch eine Regel HABEN.
//!
//! Der Grund ist eine Stunde Fehlersuche: die Tailwind-Vorlage benutzte
//! `left-2`, das vendorierte Blatt kennt aber nur `top-*`/`bottom-*`/`inset-*`
//! — Tailwind v4 gibt nur aus, was seine Quelle wirklich benutzt, und diese
//! Quelle war eine andere. Der Kasten stand deshalb bei x=0, und das sah
//! haargenau nach einem Fehler in unserem `position:absolute` aus. Er war
//! keiner: das Blatt sagte nichts, also tat die Maschine nichts.
//!
//! Eine Vorlage, die eine Klasse ohne Regel zeigt, ist ein Orakel, das luegt —
//! und zwar in die teure Richtung: sie meldet einen Fehler, den es nicht gibt.
//! Diese Probe haelt Vorlage und Blatt zusammen, damit das nicht ein zweites
//! Mal eine Runde kostet.
```

## L15-16 · `fn selector(class: &str) -> String {`

```
/// Selektor-Text einer Klasse, so wie ein Blatt ihn schreibt: `:` und `/` und
/// `.` werden in Tailwind mit Backslash geschuetzt.
```

## L28-29 · `fn defined(css: &str, sel: &str) -> bool {`

```
/// Steht `sel` im Blatt — und endet dort auch, statt der Anfang eines
/// laengeren Namens zu sein (`.p-1` darf nicht auf `.p-10` passen)?
```

## L36-37 · `let before = css[..at].chars().next_back().unwrap_or(' ');`

```
// Ein Selektor faengt nicht mitten in einem Namen an: das Zeichen davor
// darf kein Namenszeichen sein (sonst passt `.p-1` auf `.grid-cols-1`).
```

## L65-66 · `let own = html.split("<style>").nth(1).and_then(|s| s.split("</style>").next()).unwrap_or("");`

```
// Der eigene <style>-Block der Vorlage zaehlt mit: er definiert den Rahmen
// um jeden Block, und der gehoert nicht ins fremde Blatt.
```

