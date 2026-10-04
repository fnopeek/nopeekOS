# `tools/wasm/beak-engine/examples/jsrun.rs` @ 5e0102684

## L1-11 · `fn host_random(out: &mut [u8]) -> bool {`

```
//! Eine JS-Datei laufen lassen und sagen, was sie auf die Konsole geschrieben
//! hat.
//!
//! `js::run` sammelt die Konsole ein und wirft sie weg — fuer eine Probe ist
//! genau die Konsole aber das Ergebnis. `CAP=1` setzt die Schrittgrenze, damit
//! eine Endlosschleife als `RangeError` endet statt als haengender Lauf.
//!
//!   cargo run --release --example jsrun -- probe.js
/// Zufall fuer die HOST-Werkzeuge — aus `/dev/urandom`, nicht aus einer
/// Bequemlichkeit. Ohne sie gaebe es hier kein `crypto`, und dann misst das
/// Werkzeug eine andere Plattform als das Geraet.
```

## L24-25 · `let module = std::env::var("MODULE").is_ok();`

```
// `MODULE=1` parst als Modul. beak versucht am Geraet BEIDES — die Datei
// sagt nicht, was sie ist —, also muss die Probe das auch koennen.
```

## L41-43 · `if std::env::var("NOVM").is_ok() { i.vm_off = true; }`

```
// `NOVM=1` faehrt dieselbe Datei ohne die Befehlsmaschine. Der Diff der
// beiden Ausgaben ist die einzige Art, zu pruefen, dass die zwei
// Maschinen dieselbe Bedeutung haben — und nicht nur dieselbe Zahl.
```

## L45-46 · `if let Ok(h) = std::env::var("HTML") {`

```
// `HTML=<datei|text>` haengt ein Dokument an. Ohne das gibt es `document`
// GAR NICHT — das ist Absicht der Engine und keine Luecke des Werkzeugs.
```

## L51-53 · `let media = beak_engine::css::Media::new(1024.0, std::env::var("DARK").is_ok());`

```
// Denselben Kaskadenkontext einreichen, den beak einreicht — sonst
// antwortet `getComputedStyle` hier anders als am Geraet, und die
// Probe prueft eine Maschine, die es so nicht gibt.
```

## L70-71 · `i.set_media(1024.0, 768.0, std::env::var("DARK").is_ok());`

```
// Ein Fenster dazu, sonst gibt es `matchMedia` nicht. `DARK=1` dreht
// das Farbschema.
```

## L76-77 · `for _ in 0..64 { if i.run_timers() == 0 { break } }`

```
// Zeitgeber UND Microtasks nachlaufen lassen — eine Probe, die auf
// `setTimeout` endet, haette sonst kein Ergebnis.
```

