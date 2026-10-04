# `tools/wasm/beak-engine/examples/predict2.rs` @ 5e0102684

## L1-10 · `fn main() {`

```
// Was zeigt der Geraetetest MIT externen Skripten? Aendert sich der Baum?
//
// `wallcheck` fragt "laufen die Skripte durch", dieses hier fragt "tun sie
// etwas SICHTBARES" — und das ist die Frage, die der Anwender stellt.
//
// Die Zuordnung `<script src>` -> abgelegte Datei kommt aus `measure.json`:
//
//   python3 -c "import json;[print(f\"{p[\'name\']}\\t{s[\'url\']}\\t{s[\'file\']}\")
//     for p in json.load(open(\'out/measure.json\')) for s in (p.get(\'scripts\') or [])]" > map.tsv
//   SCRIPTMAP=map.tsv JSSCOPE=<…>/tools/jsscope cargo run --release --example predict2
```

## L14 · `let tsv = std::fs::read_to_string(std::env::var("SCRIPTMAP").unwrap()).unwrap();`

```
// Zuordnung Seite -> (URL, Datei), von `measure.json` abgeleitet.
```

