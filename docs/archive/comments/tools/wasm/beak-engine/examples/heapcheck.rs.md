# `tools/wasm/beak-engine/examples/heapcheck.rs` @ 5e0102684

## L1-14 · `use std::alloc::{GlobalAlloc, Layout, System};`

```
//! Wieviel Halde kostet was — GEHALTEN und als SPITZE, getrennt.
//!
//! Ausgeloest von einem Absturz am Geraet: beak gab beim Beenden 79 MB zurueck,
//! davon 59 MB gewachsen, und `beakbench` zeigte, dass das Schriftrastern
//! allein von 11 auf 89 MiB treibt. Was `beakbench` NICHT trennen kann, ist
//! „braucht der Browser das dauerhaft" von „das war eine Spitze beim Parsen,
//! und talc gibt Seiten nie zurueck" — die Antwort entscheidet, ob hier etwas
//! zu holen ist.
//!
//!     cargo run --release --example heapcheck
//!
//! Der Allokator zaehlt Bytes, nicht Seiten: eine Wasm-Halde ist immer
//! groesser als die Summe der lebenden Allokationen (Verschnitt, Bins), aber
//! das VERHAELTNIS von gehalten zu Spitze ist dasselbe.
```

## L24 · `unsafe impl GlobalAlloc for Counting {`

```
// SAFETY: reicht jede Anfrage an `System` durch und zaehlt nur mit.
```

## L70 · `let f = beak_engine::fonts::Fonts::new();`

```
// Die sechs eingebauten Gesichter — der Posten, um den es geht.
```

## L75 · `println!();`

```
// Je Gesicht einzeln — ueber `add_web`, das denselben Parser fuehrt.
```

## L90-92 · `for p in std::env::args().skip(1) {`

```
// **Und jetzt die Frage, die zaehlt: wieviele Gesichter fasst eine ECHTE
// Seite an?** Faul zu laden hilft nur, wenn eine Seite nicht ohnehin alle
// sechs beruehrt.
```

## L100-101 · `let sheet = beak_engine::css::collect_all(&dom, &css, beak_engine::css::Media::new(1902.0, false));`

```
// Mit dem externen Blatt, sonst misst man eine Seite ohne ihr CSS —
// und die fasst weniger Gesichter an als die echte.
```

## L108-110 · `println!("  {:<24} {} von 6 Gesichtern   gehalten {:>6.1} MB   SPITZE {:>6.1} MB   ({} px hoch)",`

```
// **Die SPITZE bestimmt die Halde, nicht das Gehaltene.** Ein
// Wasm-Heap waechst auf den groessten gleichzeitigen Stand und gibt
// nie zurueck; was danach frei wird, senkt ihn nicht mehr.
```

