# `tools/wasm/beak-engine/examples/wsreal.rs` @ 5e0102684

## L1-10 · `use beak_engine::js::websocket::{Event, Socket};`

```
// Der WebSocket-Code gegen einen ECHTEN Server — host-seitig.
//
// Die Rahmen rechnet DIESE Datei, also derselbe Code, den beak faehrt; die
// TLS-Leitung liegt draussen (`tools/wsdrive.py`), weil der Motor kein `std`
// kennt und keinen Strom aufmachen kann. Zeilenprotokoll auf stdin:
//
//     RX <hex>     Bytes von der Leitung hereingeben
//     SEND <text>  einen Textrahmen hinauslegen
//
// Geantwortet wird mit `EV <ereignis>`, `TX <hex>` und `OK`.
```

## L24-25 · `let mut sock = Socket::new(1, &url, &origin, true, [0x5a; 16]).expect("URL zerlegen");`

```
// Fester Zufall: der Lauf soll wiederholbar sein. Fuer die Maske ist das
// hier richtig und auf dem Geraet falsch — §5.3 verlangt dort echten.
```

