# `tools/wasm/i2c_hid/harness/src/main.rs` @ 5e0102684

## L1-5 · `use aml_core::{Ec, Machine, Namespace, Value};`

```
//! Pruefstand: eine echte DSDT laden und berichten, was an HID-over-I2C
//! darin steht. Kein Geraet noetig — genau der Gang, den das Modul am
//! Notebook fahren wird.
//!
//!     cargo run -p i2c_hid_harness -- <DSDT.aml>
```

## L20-21 · `struct NoEc {`

```
/// Der Pruefstand hat keinen EC; jede Lesung meldet 0 und jede Notiz geht
/// auf die Ausgabe, damit der Gang sichtbar ist.
```

## L56-58 · `let devs = aml_core::devices_with_ids(&ns);`

```
// Jedes Geraet mit einer Kennung — die Gegenprobe, wenn die Suche
// nichts findet: steht das Geraet ueberhaupt im Namespace, und wie
// heisst es dort?
```

## L68-70 · `let argv: Vec<String> = std::env::args().collect();`

```
// `-e <pfad>`: ein beliebiges Objekt auswerten. Ein Name gibt seinen
// Inhalt, eine Methode wird ausgefuehrt — damit laesst sich eine Frage
// an die Firmware stellen, statt ihre Antwort zu erraten.
```

