# `tools/wasm/beak-engine/examples/hitchk.rs` @ 5e0102684

## L1-9 · `fn main() {`

```
//! Findet der KLICKPUNKT den Knopf? — der Schritt, den `selftest.rs` auslaesst.
//!
//! `selftest.rs` baut die Zustellkette aus dem BAUM (`ancestors`). beak baut
//! sie aus dem LAYOUT (`lay.element_chain(x, y)`). Damit prueft der
//! host-seitige Lauf alles ausser genau dem Schritt, an dem ein Geraetelauf
//! scheitern kann — und ein Geraetelauf hat es getan: vier `control-activate`,
//! keine einzige Behandler-Zeile.
//!
//! Hier wird die Kette so gebaut wie in beak: aus den gemalten Kaesten.
```

## L39-41 · `let mut all_ok = true;`

```
// Jeder Knopf ueber SEINEN Steuerkasten — dieselbe Quelle, aus der beaks
// `hit_control` den Klick nimmt. Trifft `element_chain` dort nichts, dann
// kommt der Klick nie bei der Seite an.
```

