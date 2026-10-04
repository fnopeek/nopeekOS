# `tools/wasm/beak-engine/examples/laytime.rs` @ 5e0102684

## L2-9 · `fn main() {`

```
// Was kostet ein Layout auf einer ECHTEN Seite?
//
// Die Custom Properties laufen seit 0.59.0 durch die Kaskade statt durch
// einen Textlauf davor — je Element eine Karte statt einmal je Blatt. Diese
// Probe sagt, was das wirklich kostet; „sollte schnell genug sein" ist keine
// Zahl ([[feedback_remeasure_before_claiming_a_delta]]).
//
//   PAGE=<pfad ohne .html> W=1902 N=5 cargo run --release --example laytime
```

## L29-31 · `let mut state = beak_engine::forms::FormState::default();`

```
// Und was kostet dasselbe, wenn sich nur EIN Steuerelement geaendert hat?
// Das ist die Zahl, die zaehlt: bis 0.71.0 ging jeder Tastendruck in einem
// Feld den vollen Weg oben.
```

