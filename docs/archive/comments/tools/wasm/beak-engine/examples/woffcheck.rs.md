# `tools/wasm/beak-engine/examples/woffcheck.rs` @ 5e0102684

## L1-9 · `fn main() {`

```
//! Eine WOFF-Datei entpacken und gegen eine Referenz-TTF stellen.
//!
//!   cargo run --release --example woffcheck -- x.woff2 [x.ttf]
//!   cargo run --release --example woffcheck -- x.woff
//!
//! BEIDE Container, weil beide im Netz stehen: `wOF2` geht durch `woff2`,
//! `wOFF` durch `woff`. Verglichen werden die TABELLEN, nicht die Datei:
//! Reihenfolge, Ausrichtung und Pruefsummen darf ein Entpacker anders
//! schreiben, der INHALT nicht.
```

## L31 · `match fontdue::Font::from_bytes(got.as_slice(), fontdue::FontSettings::default()) {`

```
// Laedt fontdue es?
```

## L61-63 · `let (Ok(a), Ok(b)) = (`

```
// **Der eigentliche Beweis: dieselben PIXEL.** Ein `glyf`, das anders
// kodiert ist, darf denselben Umriss haben — und genau das muss geprueft
// werden, nicht die Bytezahl.
```

