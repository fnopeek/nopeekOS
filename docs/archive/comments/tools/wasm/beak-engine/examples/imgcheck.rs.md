# `tools/wasm/beak-engine/examples/imgcheck.rs` @ 5e0102684

## L1-12 · `fn kind(b: &[u8]) -> &'static str {`

```
//! Warum ist dieses Bild nicht angekommen? — der Dekoder mit GRUND.
//!
//! `[beak] image dropped: undecodable or over budget` nennt zwei Fehler in
//! einem Satz, und nur einer davon ist je der richtige
//! ([[feedback_a_denial_and_a_timeout_are_two_failures]]). Hier laeuft
//! derselbe Weg host-seitig, und die Absage sagt, WELCHER.
//!
//!   cargo run --release --example imgcheck -- bild.jpg [...]
//!   cargo run --release --example imgcheck -- seite.html   # nur die Liste
//!
//! Eine `.html` wird nicht dekodiert, sondern AUFGEZAEHLT — mit `image_srcs`,
//! also der Liste, die beak selbst holt, nicht einer zweiten aus einem grep.
```

