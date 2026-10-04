# `tools/wasm/beak-engine/examples/boxprobe.rs` @ 5e0102684

## L1-9 · `fn main() {`

```
//! Die KAESTEN einer Datei nennen — Etikett, Ort, Groesse.
//!
//! `pagedump` zeigt, was gemalt wird; ein Kasten ohne Farbe malt nichts und
//! ist trotzdem der, um den es geht (Raender, `min-height`, Zusammenfall).
//! Das ist die Seite, die man gegen `getBoundingClientRect` eines echten
//! Browsers halten kann.
//!
//!   cargo run --release --example boxprobe <datei.html>
//!   W=800 BOX=div.p cargo run --release --example boxprobe <datei.html>
```

