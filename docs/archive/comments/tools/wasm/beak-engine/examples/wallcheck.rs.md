# `tools/wasm/beak-engine/examples/wallcheck.rs` @ 5e0102684

## L1-11 · `fn main() {`

```
// Woran stirbt echter, ausgelieferter Code ZUERST?
//
// Nicht test262, sondern die Skripte, die Chromium beim Laden der zwoelf
// Zielseiten geparst hat. Ohne DOM — das ist erwartet und genau der Punkt:
// die Frage ist, ob die SPRACHE die Wand ist oder die Wirtsumgebung.
//
// **Je SEITE eine Umgebung, nicht je Datei.** Die erste Fassung fuhr jedes
// Skript einzeln und meldete 98 mal `mw is not defined` — aber `mw` wird von
// einem SCHWESTERSKRIPT derselben Seite gesetzt. Ein Browser teilt die
// Umgebung, also muss diese Messung es auch tun, sonst misst sie die
// Isolation und nicht die Engine.
```

## L21-22 · `fs_.sort_by_key(|p| {`

```
// In der Reihenfolge, in der `measure.mjs` sie abgelegt hat — das
// ist die Reihenfolge, in der der Browser sie geparst hat.
```

## L37-38 · `let mut sess = beak_engine::js::Session::new(2_000_000);`

```
// Eine Umgebung fuer die ganze Seite. Ein Absturz in Skript 3 darf die
// Umgebung nicht mitnehmen, also faengt jeder Lauf fuer sich.
```

## L40-42 · `let html_path = std::path::Path::new(&root).parent().unwrap()`

```
// Das ECHTE HTML der Seite dazu — `measure.mjs` hat es neben den
// Skripten abgelegt. Ohne Dokument misst dieser Lauf nur die Sprache;
// mit ihm misst er, was ein Skript im Browser vorfindet.
```

## L48-50 · `sess.interp.set_media(1280.0, 800.0, false);`

```
// Fenster + Farbschema wie im Browser, sonst faellt jede Seite
// schon an `innerWidth`/`matchMedia` aus und der Lauf misst das
// Werkzeug statt die Engine.
```

## L71-73 · `let want = std::env::var("WCPAGE").unwrap_or_default();`

```
// `WCPAGE=<seite>` (oder `*`) nennt Datei UND Grund. Der Histogramm-Balken
// sagt WAS die Wand ist, nicht WO — und beim Vergleich zweier
// Staende ist genau das die Frage.
```

