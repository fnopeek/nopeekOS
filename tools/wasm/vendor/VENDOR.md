# Vendorter H.264-Dekoder

`rusty_h264-decoder` + `rusty_h264-common` 0.16.0, BSD-2-Clause,
© Mata Network — <https://github.com/remade-with-rust/rusty_h264>.

Hier im Baum statt aus der Registry, aus zwei Gruenden. Der erste ist ein
Fehler, der zweite ist der wichtigere.

## 1. Der no_std-Arm baut upstream nicht

Genau EINE Zeile, in `rusty_h264-decoder/src/mb16.rs`:

    pub(crate) mod edcstat {
    +   use alloc::vec::Vec;

Ohne `std` bringt der Prelude `Vec` nicht mit, und dieses verschachtelte
`mod` hat keinen eigenen Import; mit `std` faellt es niemandem auf. Es ist
Diagnosecode — das `eprintln!` darin ist im no_std-Arm ohnehin ein No-op.
**Das ist der einzige Unterschied zum veroeffentlichten Stand.** Gemeldet
gehoert er trotzdem.

## 2. Ein Dekoder ist die Flaeche, die wir besitzen wollen

Ein Mediendekoder ist die klassischste Angriffsflaeche, die es gibt. Dass
er `#![forbid(unsafe_code)]` traegt und gefuzzt panikfrei ist, war der
Grund, ihn zu waehlen; dass er dann bei einem `cargo update` unbemerkt
wechselt, waere der Grund, es zu bereuen. Im Baum ist jede Aenderung ein
Diff, den jemand liest.

## Wie er gebaut wird

`--no-default-features`, dazu `libm`. Damit bleibt der **skalare** Arm:

- **`asm` MUSS aus bleiben.** Es sind portable Rust-SIMD-Kerne, und forge
  kennt `simd128` nicht. Eine Funktion, die forge nicht uebersetzt, bekommt
  den Trap-Stumpf — das Modul stuerzt beim ersten Aufruf ab, es wird nicht
  langsamer. Gemessen ist der Arm ohnehin nur 1,14x auf normalem Material.
- **`std` und `global-alloc` bleiben aus**: tune bringt seine Halde selbst
  mit (`nopeek_widgets::heap`).

Nicht mitkopiert: `tests/`, `examples/`, `benches/` und die zugehoerigen
Dev-Abhaengigkeiten (eine davon ist ein Fenstersystem). Die Abschnitte sind
aus den `Cargo.toml` entfernt; sonst ist alles unveraendert.

## Beim Aktualisieren

1. Neue Fassung entpacken, `src/` und `Cargo.toml` ersetzen.
2. Pruefen, ob der Patch aus (1) noch noetig ist.
3. Beispiel-/Test-Abschnitte wieder entfernen.
4. `python3 tools/forge-gate.py` auf das gebaute tune.wasm — eine neue
   Fassung kann einen Opcode mitbringen, den forge nicht kennt.
5. `<tools>/mediabench/src/pipeline_probe.rs` gegen ffmpeg: die Pixel
   muessen byte-gleich bleiben.
