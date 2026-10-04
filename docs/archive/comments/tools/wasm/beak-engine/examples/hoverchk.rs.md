# `tools/wasm/beak-engine/examples/hoverchk.rs` @ 5e0102684

## L1-3 · `fn main() {`

```
// Trennen die beiden Fragen wirklich? `hit_all` darf die TREFFER-Kaesten
// mehren, ohne dass ein Element dadurch `:hover`-faehig wird — sonst gilt
// jede Mausbewegung als Stilwechsel und kostet ein volles Layout.
```

## L8 · `let html = r##"<html><head><style>`

```
// EIN Element mit :hover-Regel, eins ohne.
```

## L18 · `let (x, y) = (10, 30);`

```
// Punkt im ZWEITEN div (ohne :hover-Regel).
```

