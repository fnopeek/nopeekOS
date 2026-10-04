# `tools/wasm/beak-engine/examples/gcstime.rs` @ 5e0102684

## L1-9 · `fn main() {`

```
// Was kostet `getComputedStyle` — und was kostet es, wenn das Skript vorher
// den Baum angefasst hat?
//
// Seit 0.74.0 rechnet die Kaskade auf dem LEBENDEN Baum. Der wird aus der
// JS-Arena gebaut und zwischengespeichert; jede Aenderung macht den Speicher
// ungueltig. Diese Probe sagt, was beides wirklich kostet — „sollte schnell
// genug sein" ist keine Zahl ([[feedback_remeasure_before_claiming_a_delta]]).
//
//   PAGE=<pfad ohne .html> N=200 cargo run --release --example gcstime
```

## L36-38 · `let mut run = |sess: &mut beak_engine::js::Session, body: &str| -> f64 {`

```
// Die Schleifenzahl wird EINGESETZT, nicht ersetzt. `replace("N", …)` traf
// auch das N in `tagName`; die Probe mass danach etwas anderes, als sie
// behauptete, und das sah aus wie ein Fehler in der Engine.
```

## L47-48 · `let warm = run(&mut sess, "getComputedStyle(e).color;");`

```
// Ohne Aenderung dazwischen: der Baum steht, der Zwischenspeicher greift,
// gemessen wird die Kaskade auf einer Vorfahrenkette.
```

## L50 · `let cold = run(&mut sess, "e.setAttribute('class', 'x' + i); getComputedStyle(e).color;");`

```
// Mit einer Aenderung davor: jede erzwingt einen Neubau des Baums.
```

## L56-57 · `let d = sess.interp.doc.as_ref().expect("doc");`

```
// Der Neubau allein, damit die Zahl darueber nachpruefbar ist statt nur
// plausibel.
```

