# `tools/wasm/beak-engine/examples/cmpcheck.rs` @ 5e0102684

## L1-14 · `fn main() {`

```
// Je Bootstrap-Komponente eine Zeile: wie hoch, wieviele Zeichenbefehle?
//
// Der Sinn der Komponentenvorlage steht und faellt damit, dass ein Befund
// EINEN Block nennt. Ein Diff ueber die ganze Seite sagt „4,2 % anders" und
// hilft niemandem; diese Tabelle sagt „c-modal ist 14 000 px hoch", und dann
// weiss man, wo man hinschaut.
//
//   cargo run --release --example cmpcheck            (Breite 1902, wie am Geraet)
//   W=1000 cargo run --release --example cmpcheck
//   CMP=c-modal cargo run --release --example cmpcheck   nur einen Block
//
// Jeder Block wird EINZELN ausgelegt, mit demselben Kopf wie die ganze Seite.
// Das isoliert: was in einem Block schiefgeht, kann den naechsten nicht mehr
// verschieben, und die Hoehe ist die des Blocks und nicht die seiner Nachbarn.
```

## L16-19 · `let tw = std::env::var("FIX").is_ok_and(|f| f == "tailwind");`

```
// Zwei Vorlagen, dasselbe Werkzeug: `FIX=tailwind` prueft die
// Utility-Familien, ohne `FIX` die Bootstrap-Komponenten. Ein zweites
// Rahmenwerk belastet andere Ecken — und was BEIDE richtig malen, ist
// wahrscheinlich wirklich richtig.
```

## L28-29 · `let own = between(page, "<style>", "</style>").unwrap_or_default();`

```
// Der eigene <style>-Block der Vorlage gehoert dazu — er zeichnet den
// Rahmen um jeden Block, und ohne ihn misst man eine andere Seite.
```

## L63 · `fn sections(page: &str) -> Vec<(String, String)> {`

```
/// Jeden `<section id="…">…</section>`-Block als (id, html).
```

