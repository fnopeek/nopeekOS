# `tools/wasm/beak-engine/src/js/json.rs` @ 5e0102684

## L1-12 · `use alloc::format;`

```
//! `JSON.parse` und `JSON.stringify`.
//!
//! Kein Zusatz, sondern Grundausstattung: der Selbsttest fiel darueber, und
//! auf echten Seiten steht die Konfiguration einer Komponente fast immer als
//! JSON in einem `<script type="application/json">` oder in einem
//! `data-`-Attribut. Ohne `JSON` bricht der Startpfad solcher Seiten in der
//! ersten Zeile ab.
//!
//! Eigener Erzeuger und eigener Leser statt „irgendwie ueber die Sprache":
//! JSON ist NICHT die JS-Literalsyntax (keine einfachen Anfuehrungszeichen,
//! kein Komma am Ende, keine Bezeichner als Schluessel), und der Leser haette
//! sonst mehr angenommen als er darf.
```

## L22-24 · `const MAX_JSON_DEPTH: usize = 200;`

```
/// Wie tief `stringify` und `parse` gehen duerfen. Beide laufen rekursiv auf
/// dem WIRTS-Stapel, und ein zu tiefes Dokument ist im Kernel kein Fehler,
/// sondern ein Absturz — dieselbe Ueberlegung wie bei `MAX_DEPTH`.
```

## L38 · `fn stringify(i: &mut Interp, _t: Value, args: &[Value]) -> C<Value> {`

```
// ── stringify ────────────────────────────────────────────────────────────
```

## L42-46 · `let space = match args.get(2) {`

```
// Der dritte Parameter ist der Einzug. Eine Zahl heisst „so viele
// Leerzeichen", ein Text heisst „dieser Text" — beide gedeckelt auf 10,
// wie die Spezifikation es vorschreibt.
// Ein Number- oder String-OBJEKT zaehlt wie sein Wert (ES §25.5.2.1, 5).
// Seit `new Number(5)` wirklich ein Objekt ist, kommt der Fall auch vor.
```

## L64-65 · `false => Ok(Value::Undefined),`

```
// `undefined`, eine Funktion — die haben keine Entsprechung, und die
// Antwort ist `undefined`, nicht der Text "undefined".
```

## L70-72 · `fn write_value(i: &mut Interp, v: &Value, indent: &str, cur: &str, seen: &mut Vec<Gc>,`

```
/// Schreibt `v` nach `out`. `false` heisst „hat keine Entsprechung" — der
/// Aufrufer entscheidet dann, ob das ein `null` (im Array) oder ein
/// weggelassenes Feld (im Objekt) wird.
```

## L79-80 · `let v = match v {`

```
// `toJSON` gewinnt ueber alles andere — daran haengt, dass ein Datum als
// Zeichenkette herauskommt statt als leeres Objekt.
```

## L88-90 · `let v = match &v {`

```
// Die HUELLE eines Primitivs zaehlt wie das Primitiv: `JSON.stringify(
// Object(0n))` muss werfen, `Object(1)` wird zu `1`. Ohne diesen Schritt
// faellt eine Huelle in den Objektzweig und wird `{}`.
```

## L102-103 · `if n.is_finite() { out.push_str(&num_to_string(*n)); } else { out.push_str("null"); }`

```
// NaN und Unendlich sind kein JSON. `null` ist die vorgeschriebene
// Ersatzform, nicht ein Fehler.
```

## L108-109 · `Value::BigInt(_) => Err(i.throw_kind("TypeError", "Do not know how to serialize a BigInt")),`

```
// JSON kennt keine grossen Zahlen, und stillschweigend zu kuerzen
// waere Datenverlust — die Spezifikation schreibt hier einen Fehler vor.
```

## L111-112 · `Value::Undefined | Value::Sym(_) => Ok(false),`

```
// Wie `undefined`: faellt aus dem Objekt heraus, ist im Array `null`.
// Ein Fehler waere falsch — JSON kennt Symbole schlicht nicht.
```

## L116-117 · `if seen.iter().any(|s| Rc::ptr_eq(s, o)) {`

```
// Ein Zyklus ist der eine Fall, in dem `stringify` werfen MUSS.
// Ohne die Pruefung laeuft er, bis der Stapel reisst.
```

## L149-150 · `if !write_value(i, &e, indent, &inner, seen, out, depth + 1)? {`

```
// Im Array wird eine Luecke zu `null` — weglassen wuerde die
// Laenge aendern, und die traegt hier Bedeutung.
```

## L157-158 · `let keys = i.own_keys_of(o)?;`

```
// Schluessel und Aufzaehlbarkeit durch einen Stellvertreter hindurch —
// `JSON.stringify(new Proxy({a:1},{}))` war sonst `{}`.
```

## L198 · `struct P<'a> { b: &'a [u8], p: usize }`

```
// ── parse ────────────────────────────────────────────────────────────────
```

## L202-206 · `pub(crate) fn parse_value(i: &mut Interp, v: &Value) -> C<Value> {`

```
/// `JSON.parse` fuer einen Wert, den der Rufer schon HAT.
///
/// `Response.json()` geht hierueber. Ein eigener Leser dort waere eine
/// zweite Semantik — und die faellt zuerst bei etwas Kleinem auseinander,
/// etwa was ein nacktes `NaN` im Text bedeutet.
```

## L224-226 · `match args.get(1) {`

```
// Der Wiederhersteller laeuft ueber das FERTIGE Ergebnis, von innen nach
// aussen. Er darf Werte ersetzen und weglassen; das ist der Grund, warum
// er nicht schon beim Lesen greifen kann.
```

## L337 · `p.p += 1; // "`

```
// "
```

## L346-347 · `0..=0x1f => return Err(i.throw_kind("SyntaxError",`

```
// Ein rohes Steuerzeichen ist in JSON verboten — anders als in
// einem JS-Literal. Wer das durchlaesst, nimmt mehr an als er darf.
```

## L361-363 · `let ch = if (0xd800..0xdc00).contains(&cp)`

```
// Ein hohes Ersatzzeichen sucht sein Gegenstueck; ein
// einzelnes bleibt als Ersatzzeichen stehen, statt den
// Lauf abzubrechen — genau so macht es ein Browser.
```

## L377-378 · `c if c < 0x80 => s.push(c as char),`

```
// Mehrbytefolgen wandern unveraendert durch; die Quelle war ein
// gueltiger Rust-`str`, also ist jede davon gueltiges UTF-8.
```

