# `tools/wasm/beak-engine/tests/apigap.rs` @ 5e0102684

## L1-15 · `use std::collections::BTreeMap;`

```
//! Was von den WIRKLICH gerufenen DOM-Schnittstellen fehlt — nach Aufrufzahl.
//!
//! Die Rangfolge kommt nicht aus dem Bauch, sondern aus einer Chromium-Messung
//! auf denselben zwoelf Zielseiten: `tools/jsscope/out/apicensus.json` haelt
//! je Seite fest, welche DOM-Schnittstelle wie oft gerufen wurde, getrennt
//! nach Laden und Bedienen. Dieser Test setzt jeden Eintrag gegen das, was
//! die Engine hat, und gibt die Luecke geordnet aus.
//!
//! Warum das noetig war: der erste Anlauf dieser Runde baute nach der
//! test262-Fehlerkarte. Die nannte Generatoren als groesste Luecke — im
//! echten Korpus stirbt daran KEIN einziges Skript. Der Zensus nannte
//! stattdessen `addEventListener` (33 360 Aufrufe) und `atob` (10 426).
//!
//!   APICENSUS=<tools>/jsscope/out/apicensus.json \
//!     cargo test --test apigap -- --nocapture
```

## L28-29 · `let mut agg: BTreeMap<String, u64> = BTreeMap::new();`

```
// Kein JSON-Crate in den Abhaengigkeiten und keins noetig: gebraucht
// werden nur die `"Iface.member": zahl`-Paare, und die stehen flach da.
```

## L31-32 · `let parts: Vec<&str> = raw.split('"').collect();`

```
// Nach `split('"')` stehen die Zeichenketten auf den UNGERADEN Plaetzen,
// und was danach kommt, auf dem naechsten geraden.
```

## L47 · `let mut src = String::from(PROBE);`

```
// Die Probe laeuft IN der Engine: nur sie weiss, was sie hat.
```

