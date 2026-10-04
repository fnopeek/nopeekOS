# `tools/wasm/beak-engine/examples/pagerender.rs` @ 5e0102684

## L1-6 · `fn main() {`

```
// Eine eingefrorene Seite rendern und als BMP ablegen — zum ANSEHEN.
//
// Die Komponentenvorlage prueft, was ich prüfen wollte. Diese hier prüft, was
// ein Betreiber wirklich ausliefert — eingefroren, also zweimal gleich.
//
//   PAGE=<pfad ohne .html> W=1902 OUT=x.bmp cargo run --release --example pagerender
```

## L10 · `let css = match std::env::var("CSSFILE") {`

```
// `CSSFILE=` fuer eine Vorlage, deren Blatt nicht neben ihr liegt.
```

## L30 · `fn to_bmp(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {`

```
/// BGRA nach BMP. Von unten nach oben, wie das Format es will.
```

