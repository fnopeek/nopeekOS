# `tools/wasm/beak-engine/src/js/date.rs` @ 5e0102684

## L1-11 · `use alloc::rc::Rc;`

```
//! `Date` — die Zeitrechnung aus ES 21.4, nicht der Stumpf davor.
//!
//! **Eine Zeitzone: UTC.** Es gibt keine Zonendatenbank im Bild und keinen
//! Weg, an die des Wirts zu kommen; „lokal" IST hier UTC, und
//! `getTimezoneOffset()` sagt ehrlich 0. Das ist eine benannte
//! Vereinfachung, keine Luecke im Verborgenen: alle `getX`/`setX` fallen
//! damit mit ihren `getUTCX`/`setUTCX` zusammen.
//!
//! Der Zeitwert liegt in `ObjKind::Date` und nicht als Eigenschaft am
//! Objekt — die Vorfassung trug ihn als `__t`, und der stand damit in
//! `Object.getOwnPropertyNames(d)`.
```

## L25 · `const MAX_TIME: f64 = 8.64e15;`

```
/// Der aeusserste darstellbare Zeitwert (ES 21.4.1.1).
```

## L31 · `const CUM: [f64; 13] = [0.0, 31.0, 59.0, 90.0, 120.0, 151.0,`

```
/// Kumulierte Tage vor jedem Monat, im Gemeinjahr.
```

## L50-51 · `let mut y = libm::floor(t / (MS_DAY * 365.2425)) + 1970.0;`

```
// Schaetzen und in hoechstens zwei Schritten korrigieren — die Schaetzung
// ueber die mittlere Jahreslaenge liegt nie weiter daneben.
```

## L107-108 · `fn this_time(i: &mut Interp, t: &Value) -> C<f64> {`

```
/// Den Zeitwert eines `Date` holen. Alles andere wirft — `Date.prototype.
/// getTime.call({})` ist ein TypeError, keine 0.
```

## L137-138 · `fn year_str(y: f64) -> String {`

```
/// Das Jahr, wie `toString` es schreibt: vierstellig, mit Vorzeichen wenn
/// negativ.
```

## L176-178 · `pub fn parse_date(s: &str) -> f64 {`

```
/// `Date.parse`. Zwei Formate, und beide muessen sein: das ISO-Format der
/// Spezifikation und die eigene Ausgabe von `toString`/`toUTCString` — die
/// Spezifikation verlangt ausdruecklich, dass der Rueckweg klappt.
```

## L192 · `let (year, neg, mut k) = if b.first() == Some(&b'+') || b.first() == Some(&b'-') {`

```
// Jahr: vierstellig, oder mit Vorzeichen sechsstellig.
```

## L200 · `if neg && year == 0.0 { return None; }`

```
// `-000000` ist ausdruecklich ungueltig.
```

## L217-218 · `let mut off = 0.0;`

```
// Ohne Zeitteil ist ein reines Datum UTC; MIT Zeitteil und ohne Zone
// waere es ortszeitlich — und die IST hier UTC.
```

## L235 · `let frac = &s[start..e.min(start + 3)];`

```
// Nur die ersten drei Stellen zaehlen, der Rest faellt weg.
```

## L269-270 · `fn parse_legacy(s: &str) -> Option<f64> {`

```
/// Die eigene Ausgabe wieder einlesen: `Www Mmm DD YYYY HH:MM:SS GMT+0000 …`
/// und `Www, DD Mmm YYYY HH:MM:SS GMT`.
```

## L315-317 · `fn set_fields(i: &mut Interp, t: Value, a: &[Value], first: usize, count: usize) -> C<Value> {`

```
/// Der gemeinsame Rumpf aller `setX`: die sieben Felder holen, die
/// genannten ersetzen, wieder zusammensetzen. `first` ist das erste Feld,
/// das das Argument ersetzt (0 = Jahr … 6 = Millisekunden).
```

## L320-321 · `let mut args = Vec::with_capacity(count);`

```
// Die Argumente werden IMMER umgewandelt, auch wenn der Zeitwert NaN ist
// — ihre Nebenwirkungen sind beobachtbar.
```

## L329-330 · `let base = if cur.is_nan() { if first == 0 { 0.0 } else { return set_time(i, &t, f64::NAN).map(Value::Num) } } else { cu`

```
// `setFullYear` auf einem ungueltigen Datum faengt bei der Epoche an;
// jedes andere `setX` bleibt ungueltig.
```

## L341-357 · `fn intl_def(o: &Gc, name: &str, f: NativeFn, len: usize, proto: &Gc) {`

```
/// `Intl` — so viel davon, wie eine Seite braucht, um nicht zu STERBEN.
///
/// **Es gibt keine Gebietsdatenbank in beak**, und diese Funktion tut nicht so.
/// Was sie liefert, ist wahr: `resolvedOptions()` nennt die Zeitzone, die die
/// Maschine wirklich fuehrt (UTC — `npk_unix_time` gibt keine andere), den
/// gregorianischen Kalender und lateinische Ziffern. `format` reicht an die
/// `toLocale*`-Methoden von `Date` weiter, die es schon gibt.
///
/// Gebaut, weil die Ausfallart ohne es toedlich ist. Der Zensus zaehlt ueber
/// zwoelf Zielseiten NULL `Intl`-Aufrufe; sandbox.nopeek.ch hat genau einen —
/// `Intl.DateTimeFormat().resolvedOptions().timeZone`, in `init()`, eine
/// Zeile ueber `initEventListeners()`. Ein `ReferenceError` dort kostete
/// jeden Knopf der Seite.
///
/// Was FEHLT und hier nicht vorgetaeuscht wird: Zahlengruppierung nach
/// Gebiet, Waehrungen, Pluralregeln, Kollation, relative Zeiten. Steht in
/// CONFORMANCE.
```

## L368-369 · `let mk = |realm: &mut Realm, tag: &'static str, fmt: NativeFn| -> Gc {`

```
// Ein gemeinsamer Prototyp fuer beide Formatierer: `resolvedOptions` sagt,
// was die Maschine wirklich kann, `format` reicht weiter.
```

## L382 · `b.define("timeZone", Prop::data(Value::str("UTC")));`

```
// Die Wahrheit ueber beak: die Uhr laeuft in UTC.
```

## L404-405 · `let dtf_p = dtf_proto.clone();`

```
// Beide sind mit UND ohne `new` aufrufbar (ES2024 §11.1.2) — eine Seite
// schreibt `Intl.DateTimeFormat()` genauso oft wie `new`.
```

## L454-457 · `let ctor = native(Some(fp.clone()), |i, _, a| {`

```
// ── Der Konstruktor ──────────────────────────────────────────────────
//
// Drei Formen, und die dritte ist die einzige, die rechnet: kein
// Argument = jetzt, ein Argument = Zahl oder Text, ab zwei = Felder.
```

## L459 · `if !i.native_new {`

```
// `Date()` OHNE `new` gibt Text, nicht ein Objekt.
```

## L467-468 · `if let Value::Obj(o) = &a[0] {`

```
// Ein `Date` als Argument gibt seinen Zeitwert direkt weiter,
// ohne den Umweg ueber den Text.
```

## L484-485 · `if !f[0].is_nan() {`

```
// Zwei Ziffern heissen 19xx — die annexB-Regel, und sie gilt
// auch hier, nicht nur in `setYear`.
```

## L499-500 · `let t = i.now_ms();`

```
// Eine steigende Uhr auf dem Zeitstempel des Wirts: zwei Aufrufe
// duerfen nicht denselben Wert geben, und der Wert muss heute sein.
```

## L523-526 · `macro_rules! getter {`

```
// ── Die Leser ────────────────────────────────────────────────────────
//
// Weil die Ortszeit UTC ist, ist jedes `getX` sein eigenes `getUTCX` —
// dieselbe Funktion, kein zweiter Rumpf.
```

## L546 · `"getYear" => |t| year_from_time(t) - 1900.0,`

```
// annexB: das Jahr minus 1900, mit allen Folgen.
```

## L556 · `macro_rules! setter {`

```
// ── Die Schreiber ────────────────────────────────────────────────────
```

## L576 · `def(&proto, "setYear", |i, t, a| {`

```
// annexB: zweistellige Jahre heissen 19xx.
```

## L589 · `macro_rules! stringer {`

```
// ── Die Texte ────────────────────────────────────────────────────────
```

## L604-605 · `"toLocaleString" => full_string,`

```
// Ohne Landeseinstellungen sind die drei ihre gewoehnlichen
// Geschwister. Eine erfundene Ortsschreibweise waere die falsche.
```

## L615-616 · `def(&proto, "toJSON", |i, t, _| {`

```
// `toJSON` ist GENERISCH: es fragt `toISOString` am Objekt, nicht die
// eigene Rechnung. Ein Ersatz dort schlaegt durch.
```

## L626 · `{`

```
// annexB: dieselbe Funktion wie `toUTCString`, nicht eine zweite.
```

## L631-632 · `{`

```
// Der Wunsch „default" wird hier zu Text — daran haengt, dass
// `date + ""` das Datum schreibt statt die Millisekunden.
```

