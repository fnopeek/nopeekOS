# `tools/wasm/beak-engine/examples/e2e.rs` @ 5e0102684

## L1-4 · `fn main() {`

```
// Der Beweis: ein Skript veraendert den Baum, und das BILD aendert sich.
//
// Ohne diesen Lauf ist die DOM-Bindung nur eine Behauptung — die Arena und
// beaks Baum waren bis eben zwei getrennte Welten.
```

## L18-19 · `let count = |e: &Engine| {`

```
// Nicht am Debug-Text gemessen, sondern an den Zeichenbefehlen selbst:
// wie viele Rechtecke gemalt werden, und welcher Text im Bild steht.
```

## L32 · `let dom = beak_engine::dom::parse(html);`

```
// Skript laufen lassen — auf DEMSELBEN Dokument.
```

