# `tools/wasm/beak-engine/src/js/iterhelp.rs` @ 5e0102684

## L1-11 · `use alloc::rc::Rc;`

```
//! Die Iterator-Hilfen (ES 2025) — `Iterator`, `Iterator.from` und die zwoelf
//! Methoden auf `%IteratorPrototype%`.
//!
//! **Faul, wo die Spezifikation faul ist.** `map`, `filter`, `take`, `drop`
//! und `flatMap` geben ein Hilfsobjekt zurueck, das erst beim `next()`
//! rechnet — eine eifrige Fassung wuerde an einer unendlichen Quelle haengen,
//! und genau dafuer gibt es `take`.
//!
//! Eine native Funktion nimmt keinen Abschluss, also liegt der Zustand am
//! Objekt: NUL-praefigierte Schluessel, unsichtbar fuer jedes Skript — genau
//! wie bei den Feld-Iteratoren (`IT_TARGET` & Co.).
```

## L21 · `const H_SRC: &str = "\0!hsrc";`

```
/// Die Quelle des Hilfsobjekts (der darunterliegende Iterator).
```

## L23 · `const H_FN: &str = "\0!hfn";`

```
/// Der Rueckruf (`map`/`filter`/`flatMap`) bzw. die Zahl (`take`/`drop`).
```

## L25 · `const H_N: &str = "\0!hn";`

```
/// Wieviele noch (`take`/`drop`).
```

## L27 · `const H_I: &str = "\0!hi";`

```
/// Der laufende Zaehler, den der Rueckruf als zweites Argument bekommt.
```

## L29 · `const H_KIND: &str = "\0!hkind";`

```
/// Welche Hilfe: 0 map, 1 filter, 2 take, 3 drop, 4 flatMap.
```

## L31 · `const H_INNER: &str = "\0!hinner";`

```
/// Beim `flatMap`: der gerade offene innere Iterator.
```

## L33 · `const H_DONE: &str = "\0!hdone";`

```
/// Ist das Hilfsobjekt schon fertig? Danach gibt `next` nur noch `done`.
```

## L76-77 · `fn helper_next(i: &mut Interp, t: Value, _a: &[Value]) -> C<Value> {`

```
/// Ein Schritt des Hilfsobjekts. Die fünf Formen unterscheiden sich nur
/// darin, was sie mit dem Wert von unten machen.
```

## L86 · `if kind == 4 {`

```
// `flatMap` liest erst den offenen inneren Iterator leer.
```

## L98 · `let nv = slot(i, &t, H_N)?;`

```
// `drop`: die ersten n wegwerfen, EINMAL.
```

## L145-146 · `if matches!(r, Value::Str(_)) {`

```
// Eine Zeichenkette ist hier ausdruecklich NICHT iterierbar:
// `flatMap` soll nicht in ihre Zeichen zerfallen.
```

## L162-163 · `fn call_or_close(i: &mut Interp, t: &Value, f: &Value, args: &[Value], src: &Value) -> C<Value> {`

```
/// Wirft der Rueckruf, wird die Quelle geschlossen — sonst bliebe ein
/// `finally` im fremden Generator liegen.
```

## L171-172 · `fn this_iter(i: &mut Interp, t: &Value) -> C<Value> {`

```
/// `GetIteratorDirect` — die Hilfen nehmen den Empfaenger, wie er ist, und
/// fragen NICHT nach `Symbol.iterator`.
```

## L184-185 · `fn need_count(i: &mut Interp, a: &[Value]) -> C<f64> {`

```
/// `ToIntegerOrInfinity` mit der Ablehnung, die die Hilfen verlangen: NaN
/// und negative Zahlen sind ein RangeError, nicht stillschweigend 0.
```

## L204-205 · `let hproto = new_obj(Some(iproto.clone()));`

```
// %IteratorHelperPrototype% — erbt von %IteratorPrototype%, damit ein
// Hilfsobjekt selbst wieder `map`/`filter`/… kann.
```

## L210 · `let src = slot(i, &t, H_SRC)?;`

```
// Aufgeben heisst: die Quelle schliessen und fertig melden.
```

## L220 · `def(&iproto, "map", |i, t, a| {`

```
// ── Die fuenf faulen Hilfen ──────────────────────────────────────────
```

## L242 · `def(&iproto, "toArray", |i, t, _| {`

```
// ── Die sieben, die bis zum Ende laufen ──────────────────────────────
```

## L281-282 · `macro_rules! short {`

```
// `some`, `every` und `find` unterscheiden sich nur im Abbruch — aber
// jede braucht ihren eigenen Zeiger, also ein Makro.
```

## L308-311 · `let ctor = native(Some(fp.clone()), |i, _, _| {`

```
// ── Der Konstruktor ──────────────────────────────────────────────────
//
// Abstrakt: `new Iterator()` wirft, `Iterator()` auch. Er ist nur da,
// damit `Iterator.prototype` und `Iterator.from` eine Heimat haben.
```

## L323-324 · `let wproto = new_obj(Some(iproto.clone()));`

```
// %WrapForValidIteratorPrototype% — was `Iterator.from` um einen fremden
// Iterator legt, damit die Hilfen darauf laufen.
```

## L341-342 · `let it = if matches!(v, Value::Str(_)) { i.get_iterator(&v)? }`

```
// Eine Zeichenkette wird ueber ihren `Symbol.iterator` genommen,
// alles andere direkt, wenn es schon ein Iterator ist.
```

## L351 · `if let Value::Obj(o) = &it {`

```
// Haengt es schon an `%IteratorPrototype%`, braucht es keinen Mantel.
```

## L367-368 · `def(&ctor, "concat", |i, _, a| {`

```
// `Iterator.concat` reiht mehrere hintereinander. Eifrig eingesammelt —
// benannt statt verschwiegen: an einer unendlichen Quelle haengt sie.
```

