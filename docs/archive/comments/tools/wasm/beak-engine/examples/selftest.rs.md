# `tools/wasm/beak-engine/examples/selftest.rs` @ 5e0102684

## L1-3 · `use beak_engine::js::dombind::Doc;`

```
// Die eingebaute Pruefseite host-seitig durchspielen — dieselbe Datei, die
// beak ausliefert. Was hier NEIN sagt, sagt auf dem Geraet auch NEIN; der
// Unterschied waere ein Befund fuer sich.
```

## L6-8 · `fn host_random(out: &mut [u8]) -> bool {`

```
/// Zufall fuer die HOST-Werkzeuge — aus `/dev/urandom`, nicht aus einer
/// Bequemlichkeit. Ohne sie gaebe es hier kein `crypto`, und dann misst das
/// Werkzeug eine andere Plattform als das Geraet.
```

## L18-21 · `static ENG: beak_engine::Engine = {`

```
/// Die Engine, aus der das erzwungene Neuauslegen kommt — dieselbe, die
/// danach die Klickketten legt. Ein `fn`-Zeiger faengt nichts ein, also
/// muss sie hier stehen; am Geraet ist sie aus demselben Grund eine
/// Globale (`beak/src/lib.rs`).
```

## L32-34 · `fn host_relayout(ip: &mut beak_engine::js::interp::Interp) {`

```
/// **Neu auslegen auf Verlangen**, host-seitig — derselbe Weg wie
/// `host_relayout` im Wirt. Ohne ihn waere die Zeile `fresh` hier rot und am
/// Geraet gruen ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L56-58 · `let scripts: Vec<(String, u32)> = page_scripts(&doc)`

```
// MIT dem Knoten: er ist `document.currentScript` und die Einfuegestelle
// von `document.write`. Ohne ihn liefe die Probe auf einer anderen
// Plattform als das Geraet ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L67-68 · `ENG.with(|eng| eng.set_hit_all(true));`

```
// Der Haken, wie ihn der Wirt setzt. Die Pruefseite haengt ein Element
// ein und misst es im selben Schritt — ohne ihn meldet es 0.
```

## L70-72 · `if std::env::var("NORELAYOUT").is_err() { sess.interp.relayout = Some(host_relayout); }`

```
// `NORELAYOUT=1` nimmt den Haken heraus. Nicht Zierde: damit laesst sich
// pruefen, dass die Zeile `fresh` ueberhaupt noch beissen KANN — ohne
// Haken sagt sie „0/0 statt 240".
```

## L75-77 · `sess.interp.set_location("beak:selftest");`

```
// Dieselbe Adresse wie am Geraet (`selftest::URL`). Ohne sie stuende hier
// `about:blank` und dort `beak:selftest` — und die eine Sache, die diese
// Seite kann, ist Wirt und Geraet VERGLEICHBAR zu machen.
```

## L79-84 · `sess.interp.epoch_ms = std::time::SystemTime::now()`

```
// Und die Uhr. Am Geraet setzt `beak/src/lib.rs` sie aus
// `npk_unix_time()`; ohne dieselbe Zeile hier stuende `Date.now()`
// host-seitig bei 1970, und die Pruefzeile waere auf dem Rechner
// DAUERHAFT rot — ein Warnlicht, das immer leuchtet, liest niemand mehr.
// Beidseitig gesetzt prueft sie, was sie soll: kommt die Uhr des Wirts
// in der Engine an?
```

## L89-96 · `let theme = beak_engine::Theme {`

```
// Der Kaskadenkontext — GENAU wie beak ihn einreicht (`beak/src/lib.rs`).
//
// Er fehlte hier, und dadurch lief `getComputedStyle` host-seitig auf dem
// Ausweichpfad (nur Inline-Stil) statt auf dem echten. Die Zeile
// `CSSStyleDeclaration benannt` war deshalb host GRUEN und am Geraet ROT:
// ein Testpfad, der nicht der echte ist, ist kein Test
// ([[feedback_the_test_path_must_be_the_real_path]]) — und diesmal hat er
// die Luecke nicht nur verpasst, er hat sie ZUGEDECKT.
```

## L128-136 · `println!("\n── Klicks (Kette aus dem LAYOUT, wie in beak) ──");`

```
// Die Klicks, die auf dem Geraet der Finger macht — und ZWAR UEBER DAS
// LAYOUT, nicht ueber den Baum.
//
// Die erste Fassung baute die Kette mit `ancestors()` aus dem Baum. Damit
// pruefte sie alles ausser dem einen Schritt, an dem es am Geraet
// scheiterte: beak nimmt die Kette aus `lay.element_chain(x, y)`, und ein
// `<button>` stand dort nicht drin. Host gruen, Geraet stumm — ein
// Testpfad, der nicht der echte ist, ist kein Test
// ([[feedback_verify_the_call_path]]).
```

## L143-147 · `sess.interp.set_geometry(beak_engine::js::interp::Geometry {`

```
// Die Kaesten einreichen — GENAU wie beak es je Bild tut. Ohne das
// antwortet `getBoundingClientRect` hier mit Nullen, und die Zeile `geom`
// waere host-seitig rot und am Geraet gruen: derselbe Fehler wie bei der
// Klickkette und beim Kaskadenkontext, dritte Auspraegung
// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L170-174 · `match beak_engine::js::dombind::dispatch_at(&mut sess.interp, "click", &nodes,`

```
// **Mit dem ORT, wie der Wirt** — er schickt seit 0.186.0
// `dispatch_at`. Ohne die Koordinaten waere `e.clientX` hier
// `undefined` und am Geraet eine Zahl, und die Zeile `mausev`
// stuende host rot und Geraet gruen
// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L182-183 · `for _ in 0..8 { if sess.interp.run_timers() == 0 { break } }`

```
// Mehrere Runden: die Promise-Kette endet in einem `setTimeout`, das
// erst faellig wird, nachdem die Kette durch ist.
```

