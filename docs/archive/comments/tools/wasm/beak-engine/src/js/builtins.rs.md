# `tools/wasm/beak-engine/src/js/builtins.rs` @ 5e0102684

## L1-8 · `use alloc::rc::Rc;`

```
//! Das globale Objekt und die eingebauten Prototypen.
//!
//! Das MINDESTMASS ist nicht geraten: es ist das, was test262s eigener
//! Vorspann (`assert.js` + `sta.js`) verlangt, nachgelesen statt vermutet —
//! `Object.prototype.toString.call`, `Function.prototype.call`, `String()`,
//! `Error` mit `name`/`message`, `Array.prototype` fuer `compareArray`.
//! Laeuft der Vorspann nicht, besteht kein einziger Test, egal wie gut der
//! Rest ist.
```

## L25-26 · `fn def_sym(o: &Gc, key: &str, show: &str, f: NativeFn, len: usize, proto: &Gc) {`

```
/// Wie `def`, aber unter einem SYMBOL. Der Anzeigename ist ein anderer als der
/// Schluessel — `f.name` ist `"[Symbol.iterator]"`, nicht das NUL-Byte.
```

## L32-34 · `fn loc_parts(i: &Interp) -> super::url::Parts {`

```
/// Die Adresse des Dokuments, zerlegt. **Bei jedem Zugriff frisch** — das
/// kostet eine Zerlegung und spart die zweite Wahrheit; `location` wird
/// gelesen, nicht in einer Schleife gezaehlt.
```

## L41-47 · `fn loc_go(i: &mut Interp, url: String, replace: bool) -> C<Value> {`

```
/// Eine Navigation ANMELDEN. Die Engine faehrt sie nicht — sie hat kein Netz.
///
/// **Zwei Schemata kommen hier nicht durch, und beide mit Grund:**
/// `javascript:` waere ein zweiter Weg, Code einzuspeisen, an der
/// Skript-Zustellung vorbei; `data:` erbte die Herkunft der Seite, die es
/// oeffnet, und waere damit eine fremde Seite unter unserem Namen. Beide
/// sind in echten Browsern fuer die Navigation der obersten Ebene gesperrt.
```

## L51-53 · `i.console_push(alloc::format!("warn: Navigation auf {low:.32} abgelehnt"));`

```
// Kein Wurf: der Browser lehnt still ab. Eine Ausnahme hier wuerde
// das Skript beenden, das sie ausloest — und das ist mehr Schaden,
// als die Absage anrichtet ([[feedback_a_host_call_that_throws_ends_the_script]]).
```

## L61-63 · `fn loc_navigate(i: &mut Interp, arg: Option<&Value>, replace: bool, _reload: bool) -> C<Value> {`

```
/// `assign(u)` / `replace(u)`: aufloesen gegen die AKTUELLE Adresse, dann
/// anmelden. Ohne Argument ist es `undefined` — und das ist eine Adresse,
/// die es nicht gibt, also passiert nichts.
```

## L72-74 · `pub(crate) fn loc_put_forwards(i: &mut Interp, arg: Option<&Value>) -> C<Value> {`

```
/// `[PutForwards=href]`: `window.location = u` und `document.location = u`
/// sind eine Navigation, keine Zuweisung. Oeffentlich, weil `set_location`
/// den Setzer am Dokument baut.
```

## L79-83 · `macro_rules! loc_part {`

```
/// Ein Teil von `location`: lesen aus `loc_href`, schreiben heisst navigieren.
///
/// Ein Makro und keine Funktion, weil `native` einen reinen Funktionszeiger
/// nimmt: ein Getter, der ein uebergebenes Stueck Verhalten FAENGT, ginge
/// nicht durch. So steht jedes Paar als eigener, fangfreier Rumpf da.
```

## L119 · `def(&object_proto, "toString", |i, this, _| {`

```
// ── Object.prototype ─────────────────────────────────────────────────
```

## L121-122 · `if let Value::Obj(o) = &this {`

```
// Ein Stellvertreter traegt die Marke seines ZIELS: `[object Array]`
// fuer einen Stellvertreter auf ein Feld.
```

## L134-135 · `if let Ok(Value::Str(t)) = i.get(&this, SYM_TO_STRING_TAG) {`

```
// `Symbol.toStringTag` gewinnt vor der eingebauten Art —
// aber nur, wenn es eine Zeichenkette ist.
```

## L149-150 · `ObjKind::Dataset(_) => "DOMStringMap",`

```
// `Object.prototype.toString.call(el.dataset)` sagt in
// jedem Browser `[object DOMStringMap]`.
```

## L155-156 · `ObjKind::Proxy(_) => "Object",`

```
// Oben abgefangen; hier nur, damit der Uebersetzer die
// Vollstaendigkeit prueft statt sie zu erlauben.
```

## L158-159 · `ObjKind::ModuleNs(_) => "Module",`

```
// Ein Namensraum traegt sein Etikett als Eigenschaft und
// ist oben schon beantwortet.
```

## L162-164 · `ObjKind::TypedArray(_) | ObjKind::DataView(_) => "Object",`

```
// Eine Sicht traegt ihren Namen ueber `Symbol.toStringTag`
// auf `%TypedArray%.prototype`; hier kommt sie nur an,
// wenn den jemand geloescht hat.
```

## L166-169 · `ObjKind::Generator(_) => "Object",`

```
// Ein Generator traegt seinen Namen ueber
// `Symbol.toStringTag` und kommt hier nur an, wenn den
// jemand geloescht hat — dann ist er ein gewoehnliches
// Objekt, genau wie in jedem echten Motor.
```

## L171-172 · `ObjKind::Collection(_) => "Object",`

```
// Wie der Generator: `Map` und `Set` tragen ihren Namen
// ueber `Symbol.toStringTag` auf ihrem Prototyp.
```

## L178-189 · `v => {`

```
// **Ein Primitiv wird EINGEPACKT, bevor die Marke gelesen wird**
// (§20.1.3.6 Schritt 3) — vorher fielen `true`, `1`, `"s"` und
// `10n` alle auf `[object Object]`. Das ist die Zeile, mit der
// jede Bibliothek ihre Typen unterscheidet: Bootstrap lehnte
// deshalb sein eigenes `backdrop: true` als „object" ab.
//
// Eingepackt wird trotzdem NICHT wirklich. Eine Huelle waere hier
// reine Verschwendung, und bei einer Zeichenkette eine teure:
// sie legt eine Eigenschaft je Zeichen an
// ([[feedback_the_wrapper_built_the_whole_string]]). Beobachtbar
// ist an ihr ohnehin nur der Weg zu `Symbol.toStringTag` — und
// den geht `get` auf dem Primitiv genauso.
```

## L211-215 · `Ok(Value::Bool(i.get_own_desc(&o, &k)?.is_some()))`

```
// **Durch den Stellvertreter, nicht an ihm vorbei.** `has_own` liest
// die eigene Tabelle — die eines Proxys ist LEER, und damit war
// `hasOwnProperty.call(proxy, k)` immer `false`. Vue fragt seinen
// reaktiven Zustand genau so ab: `hasOwn(data, key)`, und die Antwort
// entschied, ob `this.n` in einer Komponente den Wert findet.
```

## L233 · `def(fp, "call", |i, this, a| {`

```
// ── Function.prototype ───────────────────────────────────────────────
```

## L242-243 · `Some(v) => i.elems(v)?,`

```
// `apply` liest ARRAY-AEHNLICH, nicht ueber den Iterator —
// `f.apply(null, {length:2, 0:'a', 1:'b'})` muss gehen.
```

## L261 · `error_proto.borrow_mut().define("name", Prop::builtin(Value::str("Error")));`

```
// ── Error ────────────────────────────────────────────────────────────
```

## L275-276 · `macro_rules! err_ctor {`

```
// Ein Konstruktor je Fehlerart. Er baut sein Objekt selbst — deshalb
// `ctor: true` und `this` unbenutzt.
```

## L293-294 · `if let Some(Value::Obj(ec)) = global.borrow().get_own("Error").and_then(|p| p.value.clone()) {`

```
// `Error.isError` fragt die ART, nicht die Prototypenkette — ein
// `Object.create(Error.prototype)` ist KEIN Fehler.
```

## L306-307 · `err_ctor!("AggregateError", |i, _, a| {`

```
// `AggregateError` nimmt die Fehlerliste ZUERST, die Meldung danach —
// als einziger Fehlerkonstruktor. `Promise.any` braucht ihn.
```

## L319 · `array_proto.borrow_mut().define("length", Prop {`

```
// ── Array.prototype ──────────────────────────────────────────────────
```

## L323-327 · `def(&array_proto, "push", |i, this, a| {`

```
// **`ToObject(this)` ZUERST** (ES §23.1.3.x, Schritt 1 in jeder dieser
// Funktionen). Solange ein Schreiben auf ein Primitiv still verpuffte,
// fiel das Fehlen nicht auf: `[].pop.call(true)` schrieb ins Leere und
// gab dasselbe zurueck. Mit der Wurf-Fahne wird daraus ein TypeError,
// und der Test sagt endlich, was schon immer falsch war.
```

## L371-372 · `if let Value::Obj(o) = &this { if !i.has_property(o, &key) { continue } }`

```
// Ein LOCH ist kein `undefined`: `new Array(3).indexOf(undefined)`
// ist -1, nicht 0 (ES 23.1.3.17 Schritt 9a).
```

## L388-390 · `let mut out = Vec::new();`

```
// **Loecher bleiben Loecher.** `slice` kopiert nur, was DA ist
// (ES 23.1.3.28 Schritt 9b); ein aufgefuelltes Loch waere ein Wert,
// den niemand geschrieben hat.
```

## L421-425 · `let mut out = Vec::with_capacity(n.min(1 << 16));`

```
// Die Kapazitaet NIE aus einer gastkontrollierten Laenge: `new
// Array(2**32-1).map(f)` hat hier 96 GiB angefordert, und eine
// gescheiterte Allokation ist ein ABBRUCH, den `catch_unwind` nicht
// faengt — sie hat den ganzen Lauf mitgenommen. Wachsen lassen kostet
// nichts, was die Schrittgrenze nicht ohnehin vorher stoppt.
```

## L430-433 · `if let Value::Obj(o) = &this { if !i.has_property(o, &key) { continue } }`

```
// Ein Loch wird UEBERSPRUNGEN und bleibt im Ergebnis eines
// (ES 23.1.3.20 Schritt 6c). d3 baut seine Farbtabellen als
// `new Array(3).concat(…).map(colors)` — jedes durchgereichte
// Loch kommt dort als `undefined.length` an.
```

## L458-459 · `def(&array_proto, "some", |i, this, a| {`

```
// Der Rest der Array-Werkzeugkiste. Gemessen als naechste Wand: `some`
// allein hat 19 Skripte des Zielkorpus gestoppt.
```

## L522-524 · `let (mut acc, mut k) = match a.get(1) {`

```
// LOECHER zaehlen nicht mit. Unsere Felder haben zwar keine, aber
// `new Array(10)` legt auch keine Indizes an — und genau daran haengt,
// ob der Aufruf ohne Startwert wirft.
```

## L575-576 · `def(&array_proto, "toLocaleString", |i, this, _| {`

```
// Ohne Sprachumgebung ist `toLocaleString` die Verkettung der einzelnen
// `toLocaleString` — das ist keine Vereinfachung, das ist der Algorithmus.
```

## L606-607 · `let last = match a.get(1) {`

```
// Ohne Stelle ist es das letzte Element; eine negative zaehlt vom
// Ende, und was davor liegt, gibt es nicht (ES 23.1.3.20).
```

## L640-646 · `def(&array_proto, "reverse", |i, this, _| {`

```
// **`reverse` TAUSCHT, es baut nicht neu** (ES §23.1.3.26). Der
// Unterschied ist beobachtbar und hat drei Familien gekostet: `rebuild`
// schrieb `length` und alle Indizes, also warf ein eingefrorenes `[1]`
// (die Spezifikation tauscht dort NICHTS) und eine typisierte Sicht
// verlor ihre nicht-numerischen Eigenschaften. Getauscht wird ueber
// `Set(…, true)` — auf einem eingefrorenen Feld mit zwei Eintraegen
// wirft das, und genau so haelt es node.
```

## L688-691 · `for k in 1..items.len() {`

```
// Einfuegesortierung: sie ruft den Vergleicher genauso oft wie noetig
// und braucht keinen Ausleih-Trick, um waehrend des Sortierens in die
// Maschine zurueckzurufen — ein `sort_by` mit `?` im Vergleicher geht
// in Rust nicht ohne Verrenkung.
```

## L708-710 · `for (k, v) in items.into_iter().enumerate() {`

```
// Zurueckgeschrieben wird ELEMENTWEISE, ohne `length` anzufassen —
// sonst verliert eine typisierte Sicht ihre Laenge (die ist dort ein
// Getter) und ihre nicht-numerischen Eigenschaften.
```

## L718-721 · `let mut out: Vec<(usize, Value)> = Vec::new();`

```
// **Loecher bleiben Loecher** (ES 23.1.3.1, `CreateDataProperty` nur
// bei `HasProperty`). `new Array(3).concat("a","b")` hat die Laenge 5
// und drei LOECHER — genau so baut d3 seine Farbtabellen, und das
// folgende `.map(colors)` darf sie nicht sehen.
```

## L749 · `let this_str = |i: &mut Interp, this: &Value| -> C<Rc<str>> {`

```
// ── String.prototype ─────────────────────────────────────────────────
```

## L786 · `fn clamp_chars(n: f64, len: usize) -> usize {`

```
/// Eine Zeichenposition, geklemmt auf `[0, len]`.
```

## L790-791 · `fn byte_of(s: &str, n: usize) -> usize {`

```
/// Der BYTE-Versatz der `n`-ten Zeichenstelle. beak rechnet in Zeichen,
/// Rust schneidet in Bytes — dazwischen gehoert genau eine Umrechnung.
```

## L795 · `fn str_start(i: &mut Interp, s: &str, v: Option<&Value>) -> C<usize> {`

```
/// Der Byte-Versatz, ab dem eine Zeichenkettensuche anfangen soll.
```

## L804-805 · `fn arr_from(i: &mut Interp, v: Option<&Value>, n: usize, dflt: f64) -> C<usize> {`

```
/// Der Startindex einer Feldsuche: eine negative Stelle zaehlt vom Ende
/// (ES 23.1.3.17), und was davor liegt, ist die Null.
```

## L817-824 · `def(&string_proto, "indexOf", |i, this, a| {`

```
// **Die Startstelle ist kein Beiwerk — sie ist die Schleife.**
// `while ((i = s.indexOf(x, i + 1)) >= 0)` ist die Art, wie jeder
// Zeichenketten-Laeufer im Web ein zweites Vorkommen sucht. Wer den
// zweiten Parameter verwirft, gibt IMMER das erste zurueck, und die
// Schleife laeuft fuer immer. Gefunden 2026-09-13 an DuckDuckGos
// Ergebnisseite: `balanced-match` sucht so seine Klammerpaare, haengte
// 27,6 Millionen Eintraege in ein Array und riss beak mit einer
// Allokator-Panik ins Aus.
```

## L891-892 · `if (n as usize).saturating_mul(s.len()) > (1 << 24) {`

```
// Ein Deckel, weil `"x".repeat(2**31)` sonst den Speicher frisst und
// eine gescheiterte Allokation ein ABBRUCH ist, kein Fehler.
```

## L901-902 · `let s = this_string(i, &this)?;`

```
// Ohne RegExp: nur der Fall "Zeichenkette durch Zeichenkette, einmal".
// Ein Muster als erstes Argument wirft, statt still nichts zu tun.
```

## L918-920 · `let len = s.chars().count();`

```
// Ohne Stelle — und bei `NaN` — gilt das ENDE (ES 22.1.3.10): der
// Treffer darf ueber die Stelle hinausragen, nur ANFANGEN muss er
// davor.
```

## L966-968 · `def(&string_proto, "toLocaleUpperCase", |i, this, _| {`

```
// Ohne Sprachumgebung IST die landessprachliche Umwandlung die
// gewoehnliche. Eine erfundene tuerkische Sonderregel waere schlechter
// als keine.
```

## L975-984 · `def(&string_proto, "normalize", |i, this, a| {`

```
// Unsere Zeichenketten sind UTF-8 — eine einzelne Ersatzhaelfte kann
// darin gar nicht stehen. Also ist jede wohlgeformt, und `toWellFormed`
// gibt sie unveraendert zurueck. Benannt statt verschwiegen: eine Seite,
// die eine kaputte UTF-16-Folge baut, bekommt hier `true` statt `false`.
// `normalize` OHNE die Unicode-Zerlegungstabellen: sie prueft die Form
// und gibt die Zeichenkette unveraendert zurueck. Fuer bereits
// normalisierten Text — praktisch jeden Text im Netz — ist das die
// richtige Antwort; fuer zerlegten ist es die falsche. Benannt statt
// verschwiegen, und trotzdem besser als "normalize is not a function",
// woran eine Seite ganz stirbt.
```

## L1004-1005 · `macro_rules! html_wrap {`

```
// Die dreizehn annexB-Auszeichner. Sie stehen in der Spezifikation, weil
// alter Code sie ruft — und der Anfuehrungszeichen-Ersatz gehoert dazu.
```

## L1042 · `def(&number_proto, "valueOf", |i, this, _| {`

```
// ── Number / Boolean prototypes ──────────────────────────────────────
```

## L1079-1083 · `def(&number_proto, "toFixed", |i, t, a| {`

```
// `toFixed` ist die einzige Zahlenformatierung, die echter Code wirklich
// ruft — und sie rundet KAUFMAENNISCH auf die Stelle, nicht ueber
// `format!("{:.n}")`, das zur geraden Ziffer rundet. `(1.005).toFixed(2)`
// ist in JS "1.00" (weil 1.005 als f64 knapp darunter liegt), und wer
// hier eine eigene Rundung erfindet, weicht genau dort ab.
```

## L1100-1102 · `let e = libm::floor(libm::log10(libm::fabs(n))) as i32;`

```
// Die Spezifikation waehlt zwischen fester und Exponentialform nach
// DEM Exponenten, nicht nach Geschmack: unter -6 oder ab p Stellen
// wird exponentiell geschrieben, sonst fest.
```

## L1115-1116 · `let dv = i.to_number(&arg)?;`

```
// Die Umwandlung des Arguments laeuft VOR der Endlichkeitspruefung —
// sie ist beobachtbar (ES 21.1.3.2).
```

## L1130-1131 · `let s = num_to_string(a2);`

```
// Ohne Stellenangabe: so viele, wie die kuerzeste Darstellung
// braucht. `num_to_string` liefert genau die.
```

## L1137-1138 · `let r = libm::pow(10.0, f as f64);`

```
// Das Runden der Mantisse kann sie auf 10 heben — dann traegt der
// Exponent die Stelle.
```

## L1145-1147 · `def(&number_proto, "toLocaleString", |i, t, _| {`

```
// Ohne Landeseinstellungen: dieselbe Ausgabe wie `toString`. Eine
// erfundene Tausendertrennung waere schlimmer — sie saehe aus wie eine
// Lokalisierung und waere die falsche.
```

## L1152-1157 · `{`

```
// ── Konstruktoren + globale Funktionen ───────────────────────────────
// ── `Object.prototype.__proto__` und die vier annexB-Helfer ─────────
//
// `__proto__` ist ein ZUGRIFF auf `Object.prototype`, keine Eigenschaft
// je Objekt — sonst waere `({}).__proto__` ein eigener Schluessel und
// taeuchte in `Object.keys` auf.
```

## L1172 · `_ => return Ok(Value::Undefined),`

```
// Ein Primitiv ist KEIN Fehler, es wird still uebergangen.
```

## L1250-1252 · `let out: Vec<Value> = keys.into_iter()`

```
// Aus dem Schluessel zurueck auf das Symbol: er traegt Beschreibung
// und Registrierung, und er IST die Identitaet — ein hier gebautes
// `SymData` ist damit `===` zum urspruenglichen.
```

## L1269-1270 · `if let Some(p) = a.get(1) {`

```
// Das ZWEITE Argument ist dieselbe Tabelle wie bei
// `defineProperties` — dieselbe Funktion, nicht dieselbe Idee.
```

## L1283-1284 · `i.define_or_throw(&o, &k, p)?;`

```
// `Object.defineProperty` WIRFT, wo `Reflect.defineProperty` `false`
// gibt (ES §20.1.2.4 -> DefinePropertyOrThrow).
```

## L1315-1316 · `let target = Value::Obj(i.to_object(&target)?);`

```
// `ToObject(target)` steht VOR allem anderen — auf `undefined` wirft
// es, statt still nichts zu tun.
```

## L1320-1321 · `let o = o.clone();`

```
// Zeichenketten UND Symbole: `assign` kopiert jede eigene
// aufzaehlbare Eigenschaft, und ein Symbol ist eine.
```

## L1373-1374 · `if !matches!(a.get(1), Some(Value::Obj(_)) | Some(Value::Null)) {`

```
// Das zweite Argument wird auch dann geprueft, wenn das erste ein
// Primitiv ist — die Reihenfolge steht in der Spezifikation.
```

## L1380-1383 · `let mut cur = new_proto.clone();`

```
// Ein ZYKLUS ist zu verweigern, nicht zu bauen (ES 10.4.7.1). Ohne
// diese Pruefung legt `Object.setPrototypeOf(Object.prototype, {})`
// die Maschine still lahm: jeder Eigenschaftszugriff laeuft danach im
// Kreis, in nativem Code, an der Schrittgrenze vorbei.
```

## L1412-1414 · `p.configurable = false;`

```
// Versiegeln nimmt nur die KONFIGURIERBARKEIT — der Wert
// bleibt schreibbar. Das ist der ganze Unterschied zu
// `freeze`, und er wird geprueft.
```

## L1450-1452 · `let existing = o.borrow().get_own(&k).cloned();`

```
// Die Ausleihe MUSS vor dem Schreiben enden. Als `if let`
// geschrieben lebt die Leihgabe bis zum Ende des Rumpfes und
// das `borrow_mut` darin paniked — 94 Abstuerze im Lauf.
```

## L1499-1502 · `def(&array_ctor, "from", |i, _, a| {`

```
// `from` nimmt BEIDES: den Iterator, wenn es einen gibt, sonst
// `length`+Indizes. Das ist keine Nachsicht, das steht so in der
// Spezifikation — und ein `NodeList` ohne `Symbol.iterator` haengt genau
// daran.
```

## L1525-1527 · `Some(Value::Sym(sd)) => Value::Str(Interp::sym_to_display(sd)),`

```
// `String(sym)` ist die AUSNAHME: sie darf, wo `"" + sym` wirft.
// Genau so steht es in der Spezifikation, und es ist der einzige
// Weg, ein Symbol absichtlich zu Text zu machen.
```

## L1531-1536 · `if i.native_new { return Ok(Value::Obj(i.to_object(&prim)?)); }`

```
// **`new String(x)` ist ein OBJEKT, `String(x)` ist Text.** Bis 0.98.0
// gab beak beide Male das Primitiv, und `typeof new String()` sagte
// "string". Aufgefallen ist es erst, als `this` im lockeren Modus
// richtig eingepackt wurde: der Test verglich das eingepackte `this`
// mit dem NICHT eingepackten `new String()` — vorher waren beide
// Primitive und die zwei Fehler hoben sich auf.
```

## L1556-1558 · `None => s.push('\u{FFFD}'),`

```
// Eine einzelne Ersatzhaelfte ist ein gueltiger Codepunkt
// fuer diese Funktion, aber kein `char`. Sie geht als
// Ersatzzeichen durch — unsere Zeichenketten sind UTF-8.
```

## L1564-1566 · `def(&string_ctor, "raw", |i, _, a| {`

```
// `String.raw` liest die ROHEN Teile eines Vorlagenobjekts. Sie ist auch
// ohne markierte Vorlagen erreichbar — mit einem selbstgebauten Objekt,
// und genau so prueft test262 sie.
```

## L1599-1600 · `def(&number_ctor, "isNaN", |_, _, a| {`

```
// Die vier Praedikate sind KEINE Umwandlung: `Number.isNaN("NaN")` ist
// falsch, `isNaN("NaN")` wahr. Genau darin liegt ihr Zweck.
```

## L1616 · `let bigint_proto = new_obj(Some(object_proto.clone()));`

```
// ── BigInt ───────────────────────────────────────────────────────────
```

## L1669 · `let math = new_obj(Some(object_proto.clone()));`

```
// ── Math ─────────────────────────────────────────────────────────────
```

## L1696-1697 · `macro_rules! m1 {`

```
// Eine Stelle je Funktion mit EINEM Argument — die Liste ist die
// Spezifikation, nicht die Umsetzung.
```

## L1745 · `global.borrow_mut().define("undefined", Prop::frozen(Value::Undefined));`

```
// ── Globale Werte + Funktionen ───────────────────────────────────────
```

## L1752-1754 · `def(&global, "eval", |i, _, a| {`

```
// `eval` als globale Funktion ist die INDIREKTE Form: sie laeuft im
// globalen Bereich. Die direkte erkennt der Aufruf selbst (siehe
// `Interp::is_eval_fn`).
```

## L1762-1763 · `let radix = match a.get(1) { None | Some(Value::Undefined) => 10,`

```
// `to_digit` paniked bei einer Basis ueber 36 — in `core`, also ohne
// Netz. Ausserhalb von 2..=36 ist das Ergebnis NaN (ES 19.2.5).
```

## L1789-1790 · `for n in ["parseInt", "parseFloat"] {`

```
// `Number.parseInt === parseInt` steht so in der Spezifikation — also
// DASSELBE Objekt weiterreichen und nicht ein zweites bauen.
```

## L1796-1802 · `def(&global, "encodeURIComponent", |i, _, a| {`

```
// ── Die URI-Funktionen ───────────────────────────────────────────────
//
// Nicht Zierde: `encodeURIComponent is not defined` ist auf beiden
// Wikipedias die ZWEITE Wand, gleich hinter `document.cookie`
// (`wallcheck WCPAGE=*`). Vier Funktionen mit einer gemeinsamen Tabelle;
// der Unterschied zwischen ihnen ist genau, welche Zeichen roh
// durchgehen (ES 19.2.6).
```

## L1815-1817 · `def(&global, "decodeURI", |i, _, a| {`

```
// `decodeURI` laesst die reservierten Zeichen KODIERT stehen — sonst
// aenderte das Dekodieren die Struktur der Adresse, und ein `%2F` wuerde
// zu einem Pfadtrenner, der vorher keiner war.
```

## L1823-1836 · `macro_rules! collection {`

```
// ── Map und Set ──────────────────────────────────────────────────────
//
// Auf einem gewoehnlichen Objekt aufgesetzt: die Eintraege liegen unter
// einem Praefix, das kein Skript sieht. Der Preis ist ehrlich zu nennen —
// ein `Map`-Schluessel ist damit seine ZEICHENKETTE, nicht seine
// Identitaet. `m.set({}, 1); m.set({}, 2)` hat bei uns EINEN Eintrag, in
// einem Browser zwei. Fuer Konfigurationskarten (und genau dafuer
// benutzen die Zielseiten es) stimmt es; fuer Objektschluessel nicht.
// `native` nimmt einen Funktionszeiger, keinen Abschluss — deshalb kommt
// der Konstruktor von aussen herein statt hier eingefangen zu werden.
// Vier Sammlungen, EIN Rumpf — als Makro und nicht als Abschluss, weil
// die Methoden ihren eigenen Namen brauchen: `Map.prototype.has.call(
// new Set())` muss werfen, und ein Funktionszeiger sieht keine
// eingefangene Variable.
```

## L1884-1887 · `let mut n = 0usize;`

```
// Ueber den INDEX, nicht ueber eine Kopie: ein Behandler darf
// waehrend des Laufs einfuegen, und die Spezifikation sagt, dass
// das Neue noch besucht wird. Deshalb bleibt ein geloeschter
// Platz auch als Luecke stehen, statt die Liste zu verschieben.
```

## L1898-1905 · `d(&proto, "keys", |i, t, _| {`

```
// Die drei Sichten. Eine Umsetzung fuer Map UND Set: bei einem Set
// IST der Wert der Schluessel, `entries` liefert dort also `[v, v]` —
// und genau das schreibt die Spezifikation vor.
//
// Der Iterator laeuft ueber eine MOMENTAUFNAHME. Ein echter
// Map-Iterator sieht spaeter Eingefuegtes noch; das ist der eine
// Punkt, an dem diese Umsetzung noch von der Spezifikation abweicht,
// und er ist bewusst getrennt von der Frage der Schluesselidentitaet.
```

## L1939-1943 · `{`

```
// ── Die sieben Mengenoperationen (ES 2025) ───────────────────────────
//
// Sie lesen das Argument ueber `size`/`has`/`keys` — es muss KEIN Set
// sein, nur mengenaehnlich. Genau das prueft test262, und genau das
// brauchen Seiten, die eine eigene Menge herumreichen.
```

## L1970-1976 · `let symbol_ctor = native(Some(function_proto.clone()), |i, _, a| {`

```
// ── Symbol ───────────────────────────────────────────────────────────
//
// Ein Symbol ist ein PRIMITIV (`Value::Sym`), kein Objekt. Sein
// Eigenschaftsname liegt als NUL-praefigierte Zeichenkette in derselben
// Tabelle wie jeder andere — die Begruendung steht bei `PropName`.
// `Symbol` IST ein Konstruktor — `new Symbol()` wirft im Rumpf, nicht
// davor. Der Unterschied ist ueber `isConstructor` beobachtbar.
```

## L1993-1996 · `def(&symbol_ctor, "for", |i, _, a| {`

```
// `Symbol.for` teilt sich EINE Registrierung ueber alle Aufrufe. Der
// Schluessel wird aus dem Text abgeleitet und nicht durchgezaehlt —
// damit ist `Symbol.for("x") === Symbol.for("x")` schon durch die
// Gleichheit auf `key` wahr, ohne dass die Tabelle befragt werden muss.
```

## L2032-2039 · `let reflect = new_obj(Some(object_proto.clone()));`

```
// ── `Reflect` ────────────────────────────────────────────────────────
//
// Kein Konstruktor und keine Funktion, sondern ein Namensraum: jede
// Methode ist die nackte Spec-Operation, die die entsprechende Syntax
// sonst versteckt. Deshalb steht hier fast nichts eigenes — jede Zeile
// ruft dieselbe Hilfe, die auch `o.k`, `o.k = v`, `k in o`, `delete o.k`
// und `new f(…)` benutzen. Der Unterschied ist allein die Antwort: wo die
// Syntax wirft oder schweigt, gibt `Reflect` ein `true`/`false` zurueck.
```

## L2074-2075 · `if is_px {`

```
// Erst die Zeichenketten, dann die Symbole — die Reihenfolge steht in
// der Spec und wird geprueft.
```

## L2110-2112 · `Ok(Value::Bool(i.define_own(&o, &k, p)?))`

```
// Der Unterschied zu `Object.defineProperty`: hier ist ein
// Fehlschlag ein `false`, kein Wurf. Die PRUEFUNG ist dieselbe —
// sie steht in `define_own` und nicht zweimal daneben.
```

## L2149-2152 · `let nt = match a.get(2) {`

```
// FEHLT das dritte Argument, ist das Ziel selbst das Neuziel; steht
// dort `undefined`, ist es eines und wirft. Der Unterschied ist
// beobachtbar, und `isConstructor` aus dem test262-Vorspann baut
// genau darauf.
```

## L2170-2172 · `fn this_sym(i: &mut Interp, t: &Value) -> C<Rc<SymData>> {`

```
// `this` ist entweder das Primitiv oder seine Huelle — beides muss gehen,
// weil `sym.toString()` das Primitiv durchreicht, `Object(sym).toString()`
// aber die Huelle.
```

## L2199-2201 · `let sym_prim = native(Some(function_proto.clone()), |i, t, _| {`

```
// `"" + sym` wirft (siehe `to_string`); ohne diese Sperre wuerde
// `ToPrimitive` erst `valueOf` finden und das Symbol still weiterreichen,
// statt an der Umwandlung zu scheitern, wo der Fehler hingehoert.
```

## L2208-2212 · `let self_iter = native(Some(function_proto.clone()), |_, t, _| Ok(t),`

```
// ── Der Iteratorvertrag ──────────────────────────────────────────────
//
// `%IteratorPrototype%` traegt nur EINES: sich selbst zurueckzugeben.
// Genau daran haengt, dass `for (x of arr.entries())` geht — der
// Iterator muss selbst iterierbar sein.
```

## L2217-2218 · `let (generator_proto, generator_func_proto) =`

```
// Der Generatorvertrag haengt darunter — deshalb ist ein Generator selbst
// iterierbar, ohne dass `generator.rs` ein `Symbol.iterator` setzt.
```

## L2221-2223 · `let (async_iterator_proto, async_gen_proto, async_gen_func_proto) =`

```
// Dasselbe fuer den ASYNC-Vertrag. `%AsyncIteratorPrototype%` haengt NICHT
// unter `%IteratorPrototype%` — es ist eine eigene Wurzel mit nur einem
// Eintrag, `[Symbol.asyncIterator]() { return this }`.
```

## L2227-2229 · `def(&array_iter_proto, "next", |i, t, _| {`

```
// Der Zustand eines eingebauten Iterators liegt als NUL-praefigierte
// Eigenschaft auf ihm selbst. Kein Skript sieht sie (sie faellt aus
// `own_keys`), und `native` nimmt ohnehin keinen Abschluss.
```

## L2255-2257 · `def(&string_iter_proto, "next", |i, t, _| {`

```
// Zeichen fuer Zeichen — nach CODEPOINT, nicht nach Byte. Unsere Texte
// sind Rust-`str`, ein `char` ist also genau ein Codepunkt; das trifft
// die Spezifikation auch fuer alles ausserhalb der BMP.
```

## L2289-2290 · `let av = array_proto.borrow().get_own("values").and_then(|p| p.value.clone());`

```
// `[Symbol.iterator]` IST `values` — dieselbe Funktion, nicht eine zweite
// mit gleichem Inhalt: `arr[Symbol.iterator] === arr.values` ist wahr.
```

## L2294-2303 · `fn idle_deadline(i: &mut Interp) -> Value {`

```
// ── Zeitgeber ────────────────────────────────────────────────────────
//
// Angemeldet, nicht ausgefuehrt. Sofort zu rufen waere falsch (eine
// Abfrageschleife wuerde endlos rekursieren), und gar nicht zu haben ist
// ein Fehler, der das Skript beendet. Die Warteschlange ist die Stelle,
// an der beaks Ereignisschleife spaeter ansetzt — dieselbe Form wie bei
// `addEventListener`.
// Die Frist, die `requestIdleCallback` seinem Rueckruf mitgibt. Der
// uebliche Rumpf ist `while (d.timeRemaining() > 0) …` — ohne das Objekt
// ist das ein TypeError mitten in der Schleife einer Bibliothek.
```

## L2313-2315 · `fn arm(i: &mut Interp, a: &[Value], repeat: bool) -> Result<Value, super::interp::Abrupt> {`

```
// `setTimeout(f, ms, …args)` und `setInterval` — mit der Verzoegerung,
// mit den Argumenten dahinter und mit einer Kennung, die `clearTimeout`
// auch wirklich findet.
```

## L2320-2321 · `let ms = if ms.is_nan() || ms < 0.0 { 0.0 } else { ms };`

```
// Was keine Zahl ist, ist null (HTML §8.6). Ein negatives Wartestueck
// gibt es nicht.
```

## L2336-2340 · `fn arm_frame(i: &mut Interp, a: &[Value], idle: bool) -> Result<Value, super::interp::Abrupt> {`

```
// `requestAnimationFrame` und `requestIdleCallback` haengen am BILD, nicht
// an einer Wartezeit: sie sind sofort faellig. Ihr Rueckruf bekommt das,
// was die Spezifikation ihm zusagt — einen Zeitstempel bzw. eine Frist —,
// denn eine Animationsschleife rechnet mit `ts - last`, und ohne den
// Stempel kommt `NaN` heraus.
```

## L2370-2373 · `def(&global, "queueMicrotask", |i, _, a| {`

```
// `queueMicrotask` steht bewusst NICHT in der Liste darueber: es ist kein
// Zeitgeber, sondern haengt an DERSELBEN Schlange wie `.then`. Genau
// dafuer benutzen Seiten es — „nach dem laufenden Skript, aber vor dem
// naechsten Zeitgeber".
```

## L2383-2391 · `def(&global, "matchMedia", |i, _, a| {`

```
// `matchMedia` beantwortet die Frage mit DEM Medienzustand, den die
// Engine wirklich fuer die Darstellung benutzt (`css::media_matches`) —
// nicht mit einem festen `false`. Eine Seite, die ihr Layout danach
// waehlt, bekommt so dieselbe Antwort wie der Kaskadenlauf, und Layout
// und Skript koennen nicht auseinanderlaufen.
//
// Ohne `set_viewport`/`set_media` gibt es die Funktion GAR NICHT: die
// Medienlage gehoert dem Wirt, und geraten waere sie eine Messung, die
// keine ist.
```

## L2405-2408 · `for n in ["addListener", "removeListener", "addEventListener", "removeEventListener"] {`

```
// Die Lage aendert sich in beak waehrend eines Laufs nicht — ein
// angemeldeter Behandler wuerde also nie gerufen. Ihn anzunehmen und
// zu verwerfen ist trotzdem richtig: die Seite verlaesst sich darauf,
// dass die Anmeldung nicht wirft.
```

## L2417-2429 · `let make_storage_proto = |object_proto: &Gc, function_proto: &Gc| -> Gc {`

```
// ── Storage ──────────────────────────────────────────────────────────
//
// Im Speicher, nicht auf der Platte. Ein Skript, das `localStorage`
// ABFRAGT (und das tun sie, als Vertraeglichkeitspruefung), bekommt eine
// Antwort; was es hineinlegt, ueberlebt die Seite nicht. Das ist eine
// benannte Luecke — beak muesste es an npkFS haengen, und dann waere die
// Frage, wem der Speicher gehoert.
//
// **Ein Prototyp, zwei Behaelter.** `Storage` ist im Zensus 1179 Aufrufe
// wert, und die Zeile, die fehlte, war nicht `getItem` — die gab es —
// sondern der NAME: `x instanceof Storage` und
// `Storage.prototype.getItem.call(…)` scheitern an einer flachen Huelle,
// deren Methoden auf ihr selbst sitzen.
```

## L2492-2496 · `let function_ctor = native(Some(function_proto.clone()), |i, _, a| {`

```
// ── Function ─────────────────────────────────────────────────────────
// 9261 Tests scheiterten allein an `Function is not defined` — mehr als
// an jeder anderen einzelnen Ursache. Die meisten greifen nur auf
// `Function.prototype`; `new Function(args, body)` kostet nochmal zehn
// Zeilen und ist der Weg, auf dem test262 dynamisch erzeugten Code prueft.
```

## L2509 · `match prog.body.first() {`

```
// Genau EIN Ausdruck, und er ist der Funktionsausdruck oben.
```

## L2522-2536 · `const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";`

```
// ── Die Wirtsumgebung ────────────────────────────────────────────────
//
// Gemessen, nicht geraten (`examples/wallcheck.rs`): auf Wikipedia stirbt
// das ERSTE Skript an `performance`, und weil es `mw` haette setzen sollen,
// sterben die 107 danach an `mw`. EIN fehlender Global kostet eine ganze
// Seite. Deshalb kommen diese hier zuerst und nicht `Symbol` (2 Skripte).
//
// Was sie liefern, ist die FORM, nicht der Inhalt: beak muss Uhr, Adresse
// und Kennung noch einreichen. Ein Stumpf, der die richtige Form hat, ist
// trotzdem das, worauf ein Skript prueft.
// `atob`/`btoa` — 10 426 Aufrufe im Zensus, die zweitgroesste Einzelluecke
// nach `addEventListener`. Beide arbeiten auf LATIN-1, nicht auf UTF-8:
// `btoa("ä")` wirft im Browser, weil `ä` ausserhalb von 0..255 liegt.
// Das ist kein Detail — wer hier UTF-8 kodiert, gibt fuer jedes Umlaut
// eine andere Zeichenkette zurueck als jeder Browser.
```

## L2574-2575 · `out.push(((acc >> bits) & 0xff) as u8 as char);`

```
// Ein Byte wird ein ZEICHEN, nicht ein UTF-8-Byte: `atob`
// gibt eine Latin-1-Zeichenkette zurueck.
```

## L2587-2588 · `def(&perf, "now", |i, _, _| Ok(Value::Num(i.now_ms())), 0, fp);`

```
// Eine Uhr, die nur steigt. beak reicht die echte nach; bis dahin ist
// Monotonie das Einzige, worauf sich ein Skript wirklich verlaesst.
```

## L2603-2606 · `for (m, tag) in [("log", ""), ("info", ""), ("debug", ""), ("dir", ""),`

```
// Nicht mehr still. `beak-engine` hat keine Serienleitung, aber der Wirt
// hat eine: die Zeilen werden gesammelt, und beak holt sie ab. Eine
// Seite, deren eigene Diagnose ins Leere geht, kann man aus der Ferne
// nicht befragen — und genau das ist die Lage am Geraet.
```

## L2622-2629 · `if super::random::available() {`

```
// ── crypto ───────────────────────────────────────────────────────────
//
// **Es erscheint nur, wenn der Wirt wirklich eine Quelle eingereicht
// hat.** Eine Seite prueft `if (window.crypto)` und richtet sich danach;
// ein `crypto`, das schwachen Zufall liefert, beantwortet diese Frage
// falsch — und aus `getRandomValues` baut Seitencode Sitzungsmarken.
// Lieber die Luecke, die man sieht, als die Zusage, die nicht haelt
// (siehe `js::random`).
```

## L2640-2643 · `if matches!(t.kind, ElemKind::F32 | ElemKind::F64) {`

```
// Fliesskomma-Sichten sind ausgeschlossen (WebCrypto 10.1.1):
// zufaellige Bitmuster sind dort teilweise NaN, und eine Seite,
// die daraus einen Schluessel baut, verliert Entropie ohne es zu
// merken.
```

## L2652 · `if n > 65_536 {`

```
// Derselbe Deckel wie in der Spezifikation und im Kernel.
```

## L2671-2672 · `_ => false,`

```
// Der Puffer ist unter der Sicht geschrumpft. Kein
// Schreiben, kein Halt.
```

## L2680-2681 · `Ok(arg)`

```
// Die Sicht SELBST kommt zurueck, nicht eine Kopie — Seitencode
// schreibt `const a = crypto.getRandomValues(new Uint8Array(16))`.
```

## L2684-2686 · `def(&crypto, "randomUUID", |i, _, _| {`

```
// `randomUUID` aus derselben Quelle. Version 4, Variante 1, so wie
// RFC 9562 es verlangt — die sechs festen Bits werden gesetzt, nicht
// gewuerfelt.
```

## L2707-2715 · `if super::test262::enabled() {`

```
// ── $262 ─────────────────────────────────────────────────────────────
//
// **Das Wirtsobjekt des Konformanzlaeufers, und es erscheint nur, wenn
// der Wirt es bestellt hat** (`js::test262::enable`) — genau wie `crypto`
// nur erscheint, wenn es eine echte Zufallsquelle gibt. Eine SEITE darf
// `$262` nie sehen: `evalScript` waere ein zweiter Weg, Code an der
// Skript-Zustellung vorbei laufen zu lassen, und `detachArrayBuffer` zieht
// fremden Sichten den Speicher weg. Was fehlt und warum, steht im Kopf von
// `js/test262.rs`.
```

## L2719-2722 · `def(&h, "detachArrayBuffer", |i, _, a| {`

```
// Den Puffer abtrennen. Der Motor kennt den Zustand laengst
// (`BufData::detached`, mit einem Kommentar, der genau diesen Haken
// nennt) — es fehlte nur, wer ihn setzt. Die Bytes fallen MIT weg:
// ein abgetrennter Puffer haelt keinen Speicher mehr fest.
```

## L2734-2738 · `def(&h, "evalScript", |i, _, a| {`

```
// `evalScript` ist SKRIPT-Code im globalen Bereich, nicht `eval`:
// `var` und Funktionsdeklarationen werden zu Eigenschaften des
// globalen Objekts, `let`/`const` landen im globalen lexikalischen
// Bereich und ueberleben das Skript. Genau das tut `run_program` —
// es ist derselbe Weg, den der Laeufer fuer den Test selbst faehrt.
```

## L2751-2754 · `def(&h, "gc", |_, _, _| Ok(Value::Undefined), 0, fp);`

```
// **Ein ehrliches Nichts.** Die Engine zaehlt Referenzen, sie sammelt
// nicht; `gc()` hat nichts zu tun. test262 verlangt allein, dass der
// Aufruf nicht wirft — ein Test, der danach eine Freigabe PRUEFT,
// prueft `FinalizationRegistry`, und das steht nicht an.
```

## L2763-2772 · `for (k, v) in [("appName", "Netscape"), ("appCodeName", "Mozilla"),`

```
// ── Die Felder, die JEDE Seite liest ──────────────────────────────────
//
// **`appName`, `appCodeName`, `product` und `productSub` sind
// KONSTANTEN der Spezifikation** (HTML 8.9.1.1), keine Auskunft ueber
// uns: sie MUESSEN „Netscape", „Mozilla", „Gecko" und „20030107"
// lauten, in jedem Browser. Sie wegzulassen ist kein Stueck Ehrlichkeit,
// sondern eine Luecke — Seitencode liest sie und faellt auf `undefined`
// in einen Zweig, den niemand getestet hat. Was uns wirklich benennt,
// ist `userAgent`, und der sagt weiterhin, was wir sind
// ([[feedback_no_ua_impersonation]]).
```

## L2775-2776 · `("appVersion", "5.0 (nopeekOS)"),`

```
// Die Spezifikation verlangt, dass `appVersion` mit
// „5.0 (" beginnt; dahinter steht, wer wir sind.
```

## L2779-2780 · `("vendor", ""), ("vendorSub", "")] {`

```
// Chrome sagt hier „Google Inc.", Firefox die leere
// Zeichenkette. Wir sind keins von beiden.
```

## L2784-2785 · `nav.borrow_mut().define("cookieEnabled", Prop::builtin(Value::Bool(true)));`

```
// Kekse nimmt beak an (`cookies.rs`) — die Antwort ist wahr, nicht
// hoeflich.
```

## L2787-2789 · `nav.borrow_mut().define("webdriver", Prop::builtin(Value::Bool(false)));`

```
// **`webdriver` ist `false`, und das ist die WAHRHEIT**: beak wird nicht
// ferngesteuert. Ein fehlendes Feld liest sich fuer eine Seite wie
// „weiss nicht", und das ist schlechter als eine richtige Antwort.
```

## L2792-2794 · `{`

```
// `languages` folgt `language` — eine Liste mit einem Eintrag, kein
// erfundener Zweitwunsch. Ein Leser, damit sie beim Aendern von
// `language` mitgeht.
```

## L2809-2823 · `let loc = new_obj(Some(object_proto.clone()));`

```
// ── location ─────────────────────────────────────────────────────────
//
// **Ein Objekt, das navigiert.** Vorher standen hier sieben Datenfelder;
// `location.replace(u)` war damit ein `TypeError` und `location.href = u`
// schrieb still eine Eigenschaft um und ging nirgendwohin. Eine Seite,
// die sich per Skript weiterschickt — jede Anmeldung, jede Weiterleitung
// nach einem POST, Googles Sperrseite — kam so nie an.
//
// Alle Teile sind Zugriffsfunktionen ueber `interp.loc_href`, damit es
// die Adresse nur EINMAL gibt: `location.pathname = "/x"` muss `href`
// mitziehen, und zwei Kopien laufen beim ersten Setzer auseinander.
//
// **Die Engine navigiert nicht.** Sie legt die Absicht in `interp.nav`;
// der Wirt holt sie ab. Dasselbe Muster wie `history_ops` und
// `pending_fetches`.
```

## L2855-2856 · `{`

```
// `origin` ist NUR lesbar — das ist keine Bequemlichkeit, sondern die
// Spezifikation: eine Seite kann ihre Herkunft nicht umschreiben.
```

## L2865-2867 · `def(&loc, "reload", |i, _, _| {`

```
// `reload()` nimmt kein Argument: dieselbe Adresse, neuer Abruf. Der
// Verlauf waechst dabei NICHT — sonst kaeme man mit „zurueck" nie
// heraus.
```

## L2873-2876 · `def(&loc, "toString", |i, _, _| Ok(Value::str(&i.loc_href)), 0, fp);`

```
// `String(location)` ist die ADRESSE, nicht `[object Object]`
// (HTML §7.2.4). `"" + location` und `location == "…"` sind
// verbreitete Idiome; ohne das vergleicht eine Seite gegen einen Text,
// den sie nie geschrieben hat.
```

## L2879-2882 · `{`

```
// `window.location = "…"` navigiert, es ERSETZT das Objekt nicht
// (HTML §7.2.4, [PutForwards=href]). Als Datenfeld haette eine Seite
// hier still ihr `location` gegen eine Zeichenkette getauscht und waere
// danach an jedem `location.href` gestorben.
```

## L2888-2890 · `{`

```
// `isSecureContext` folgt dem Schema der Adresse — gelesen, nicht
// behauptet. Seiten schalten daran Merkmale frei, die ohne sicheren
// Kanal nicht laufen duerfen.
```

## L2906-2917 · `let hist = new_obj(Some(object_proto.clone()));`

```
// ── history ──────────────────────────────────────────────────────────
//
// **Die Engine navigiert nicht.** Sie hat keinen Verlauf und soll keinen
// erfinden; `state` gehoert dem Dokument, `length` und das Springen
// gehoeren dem Wirt. Also: lesen aus dem, was der Wirt eingereicht hat,
// schreiben in eine Liste, die er abholt — dasselbe Muster wie bei den
// Keksen (`take_cookie_sets`).
//
// Rangfolge am Zielkorpus gemessen, nicht geraten: `replaceState` 42,
// `pushState` 29, `state` 21, `scrollRestoration` 12, `back` 11, `go` 5.
// Auf der Fritz!Box ist es `history.state?.sid` — verschleiert, aber es
// ist `state`, und ohne `history` stirbt ihr erstes Skript sofort.
```

## L2921-2922 · `let g_state = native(Some(function_proto.clone()),`

```
// `state` und `length` sind Leser: sie muessen den Wert von JETZT
// liefern, nicht den vom Aufbau der Umgebung.
```

## L2931-2934 · `h.define("scrollRestoration", Prop::builtin(Value::str("manual")));`

```
// `scrollRestoration` ist ein reiner Merkposten: wir stellen keine
// Bildlaufstellung wieder her, also ist "manual" die ehrliche
// Antwort. Schreibbar, damit eine Seite es setzen kann, ohne zu
// sterben — der Wert wird gelesen, nicht befolgt.
```

## L2954-2955 · `i.history_ops.push(super::interp::HistoryOp::Go(n));`

```
// `go(0)` laedt neu. Das ist etwas anderes als „nichts tun", und der
// Wirt darf es unterscheiden — also geht es genauso raus.
```

## L2970-2975 · `def(&array_proto, "at", |i, this, a| {`

```
// ── Date, klein aber vorhanden ───────────────────────────────────────
// ── Nachzuegler ──────────────────────────────────────────────────────
//
// Kein Thema, sondern eine LISTE: Eingebaute, die schlicht fehlten.
// Gefunden mit einer Probe, die `typeof` ueber alles laufen laesst, was
// die Spec nennt — nicht geraten, ausgezaehlt.
```

## L3001-3002 · `let mut buf = Vec::new();`

```
// Ueber eine KOPIE, sonst ueberschreibt der Lauf seine eigene Quelle,
// wenn sich Ziel und Bereich ueberlappen.
```

## L3035-3037 · `let is_arr = matches!(&r, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::Array));`

```
// Nur EINE Ebene, und nur echte Felder: was kein Feld ist, wird
// als WERT genommen. `[1,2].flatMap(x => x)` gab sonst `[]` —
// `flatten` las die `length` einer Zahl.
```

## L3051-3053 · `let items = i.elems(&this)?;`

```
// Ueber dieselbe `sort` wie in-place — eine zweite Sortierordnung
// waere genau die Sorte Unterschied, die niemand bemerkt, bis sie
// zaehlt.
```

## L3112-3113 · `let units: Vec<u16> = s.encode_utf16().collect();`

```
// Nach UTF-16-Einheiten gezaehlt, wie die Spec — unsere Texte sind
// aber `str`. Die Umrechnung ist dieselbe wie in `charCodeAt`.
```

## L3143-3144 · `Ok(Value::Num(if *s < *t { -1.0 } else if *s > *t { 1.0 } else { 0.0 }))`

```
// Ohne Gebietsschema: der Vergleich ist der gewoehnliche. Das ist
// erlaubt und ehrlicher als eine erfundene Sortierordnung.
```

## L3181-3190 · `let mut ta_protos: HashMap<&'static str, Gc> = HashMap::new();`

```
// ── ArrayBuffer, %TypedArray% und DataView ───────────────────────────
//
// **Der Puffer ist der Speicher, die Sicht nur eine Sicht darauf.** Zwei
// Sichten auf denselben Puffer sehen einander; das ist der Sinn der
// Familie und der Grund, warum die Bytes im `ArrayBuffer` liegen und
// nicht in der Sicht.
//
// `%TypedArray%` selbst ist nicht rufbar und hat keinen Namen im globalen
// Objekt — es ist der gemeinsame Vorfahr, an dem alle Methoden haengen.
// Die neun Konstruktoren erben von ihm, ihre Prototypen von seinem.
```

## L3236-3238 · `for (name, which) in [("length", 0u8), ("byteLength", 1), ("byteOffset", 2)] {`

```
// Die vier Auskuenfte sind LESER auf dem gemeinsamen Prototyp, keine
// Eigenschaften der Sicht — sonst muesste jede Sicht sie mitschleppen und
// ein abgetrennter Puffer koennte sie nicht mehr aendern.
```

## L3295 · `Ok(i.new_view(x.kind, x.buf.clone(), x.offset + s as usize * x.kind.size(),`

```
// Eine Untersicht teilt den PUFFER — sie kopiert nicht.
```

## L3303-3304 · `let out = i.new_typed(x.kind, (e - s) as usize);`

```
// `slice` KOPIERT, `subarray` nicht — der einzige Unterschied, und er
// ist der ganze Grund, dass es beide gibt.
```

## L3316-3317 · `if let Some(v) = array_proto.borrow().get_own("values").and_then(|p| p.value.clone()) {`

```
// Eine Sicht ist iterierbar ueber DIESELBE Funktion wie ein Feld — sie
// laeuft ueber `length` und Indizes, und beides beantwortet die Sicht.
```

## L3321-3328 · `macro_rules! ta_borrow {`

```
// Und die gewoehnlichen Feldmethoden gelten auch hier, solange sie nur
// lesen und rechnen. Was ein FELD zurueckgibt statt einer Sicht (`map`,
// `filter`), bleibt vorerst weg — lieber gar nicht als mit falschem Typ.
//
// Sie werden NICHT weitergereicht, sondern umhuellt: `%TypedArray%.
// prototype.reduceRight.call(undefined)` muss werfen, und das taete die
// Feldfassung nicht. Der Mantel prueft die Sicht und ruft dann dieselbe
// Funktion — keine zweite Semantik, nur die fehlende Vorpruefung.
```

## L3357-3358 · `for (kind, f) in [`

```
// Die neun Konstruktoren. Ihr Rumpf ist derselbe; nur die Elementart
// unterscheidet sie, und die steht im Zeiger.
```

## L3432-3434 · `for (name, kind) in [("Int8", ElemKind::I8), ("Uint8", ElemKind::U8),`

```
// Je Art ein Leser und ein Schreiber. **Die Bytefolge ist hier eine
// ANGABE, nicht die der Maschine** — das ist der ganze Unterschied zur
// getypten Sicht, und deshalb steht `little` in jedem Aufruf.
```

## L3473-3475 · `let ph = || new_obj(Some(object_proto.clone()));`

```
// Platzhalter — `dombind::install` ersetzt sie sofort. Sie stehen hier,
// weil ein Realm ohne sie nicht baubar waere und `install` den fertigen
// Realm braucht, um die Prototypen daranzuhaengen.
```

## L3486-3487 · `websocket_proto: new_obj(Some(object_proto.clone())),`

```
// Wird von `websocket::install` ersetzt, sobald es die Schnittstelle
// gibt; ohne echten Zufall bleibt es dieser leere Platzhalter.
```

## L3507-3518 · `fn slot_set(t: &Value, key: &str, v: Value) {`

```
/// `this.length` als Zahl. Eigene Funktion, weil `i.to_number(&i.get(...))`
/// zwei gleichzeitige Ausleihen waeren — und das Aufteilen an jeder Stelle
/// haette den Code nur laenger gemacht.
/// Ein Array in-place durch eine neue Elementfolge ersetzen. Die Grundlage
/// fuer alles, was die Laenge aendert (`shift`, `splice`, `sort`, `reverse`).
/// Ein internes FACH schreiben.
///
/// Diese Namen (` !target`, ` !index`) sind keine Eigenschaften des Objekts —
/// die Objektdarstellung hat nur keinen anderen Ort dafuer. Sie gehen deshalb
/// an der Eigenschaftsmaschinerie vorbei: `Set` mit Wurf-Fahne wuerde am
/// eingefrorenen ` !target` scheitern, und das waere ein Fehler ueber etwas,
/// das ein Skript gar nicht sehen kann.
```

## L3533-3539 · `fn hist_url(i: &mut Interp, v: Option<&Value>) -> C<String> {`

```
/// Das dritte Argument von `pushState`/`replaceState`: eine Adresse, oder
/// nichts. `null`/`undefined` heisst „dieselbe Adresse behalten" und kommt
/// als leerer Text heraus — der Wirt unterscheidet das.
///
/// **Aufgeloest wird hier NICHT.** Die Engine kennt die Adresse des
/// Dokuments nicht besser als der Wirt, und eine halb aufgeloeste Adresse
/// waere schlimmer als die rohe: sie saehe richtig aus.
```

## L3547-3549 · `fn coll_view(i: &mut Interp, t: &Value, kind: u8) -> C<Value> {`

```
/// Der gemeinsame Rumpf der vier Sammlungs-Konstruktoren.
/// Die Eintraege einer Map/eines Sets als Array. `kind`: 0 Schluessel,
/// 1 Werte, 2 Paare.
```

## L3593 · `fn range_args(i: &mut Interp, s: Option<&Value>, e: Option<&Value>, n: i64) -> C<(i64, i64)> {`

```
/// `start`/`end` einer Bereichsangabe, negativ vom Ende gezaehlt.
```

## L3609-3610 · `fn flatten(i: &mut Interp, v: &Value, d: f64, out: &mut Vec<Value>) -> C<()> {`

```
/// `flat`: Felder bis zur Tiefe `d` ausschuetten. Nur ECHTE Felder werden
/// aufgeloest — ein feldaehnliches Objekt bleibt ein Wert.
```

## L3648 · `fn buf_of(v: &Value) -> Option<Rc<BufData>> {`

```
// ── Hilfen fuer Puffer und Sichten ───────────────────────────────────────
```

## L3665 · `fn ta_get(t: &Rc<TaData>, k: usize) -> f64 {`

```
/// Ein Element aus einer Sicht — ausserhalb ist es `NaN`, nicht ein Fehler.
```

## L3672 · `fn ta_write_big(t: &Rc<TaData>, k: usize, v: &super::bigint::Big) {`

```
/// Ein Element einer 64-Bit-Sicht schreiben.
```

## L3680 · `fn ta_copy_big(dst: &Rc<TaData>, k: usize, src: &Rc<TaData>) { ta_copy_big_at(dst, k, src, k) }`

```
/// Ein Element von einer 64-Bit-Sicht in eine andere.
```

## L3700-3706 · `fn ta_new(i: &mut Interp, kind: ElemKind, a: &[Value]) -> C<Value> {`

```
/// Der gemeinsame Rumpf aller neun Konstruktoren. Vier Formen, und sie sind
/// nicht dasselbe:
///
/// * `new TA(n)` — ein frischer Puffer fuer n Elemente
/// * `new TA(sicht)` — KOPIE, Element fuer Element umgerechnet
/// * `new TA(puffer, versatz, laenge)` — eine SICHT, kein neuer Speicher
/// * `new TA(iterierbares|feldaehnliches)` — Kopie der Werte
```

## L3761 · `let items = match i.get_iterator(&src) {`

```
// Iterierbar geht vor feldaehnlich — genau wie in der Spec.
```

## L3793-3794 · `let Some(d) = dv_of(&t) else { return i.type_err("not a DataView") };`

```
// Bei `getFloat*` steht die Bytefolge an Stelle 1, bei den Ganzzahlen
// ebenso — nur `set*` schiebt sie um eins nach hinten.
```

## L3814 · `let big = if kind.is_big() { Some(i.to_bigint(a.get(1).unwrap_or(&Value::Undefined))?) } else { None };`

```
// Die Umwandlung laeuft VOR der Bereichspruefung — sie ist beobachtbar.
```

## L3847-3851 · `fn console_join(i: &mut Interp, args: &[Value], prefix: &str) -> String {`

```
/// Die Argumente eines `console`-Aufrufs zu einer Zeile machen.
///
/// Ein `toString`, das selbst wirft, darf den Aufruf nicht zum Ausnahmefall
/// machen: eine Ausgabe, die das Programm anhaelt, ist schlimmer als eine
/// unvollstaendige.
```

## L3856-3857 · `if let Value::Sym(sd) = a { out.push_str(&Interp::sym_to_display(sd)); continue; }`

```
// Ein Symbol wirft bei `ToString` — auf der Konsole waere das der
// falsche Ort dafuer: die Zeile soll berichten, nicht abbrechen.
```

## L3867-3872 · `fn fixed(n: f64, d: u32) -> String {`

```
/// `toFixed`, mit der Rundung, die JS vorschreibt: auf den BETRAG, und bei
/// genau der Haelfte zur groesseren Zahl — also von der Null WEG.
///
/// Rusts `{:.n}` rundet zur geraden Ziffer, und `(-2.5).toFixed(0)` ist
/// damit `-2` statt `-3`. Der Unterschied faellt nur im Vergleich mit einem
/// echten Motor auf; gefunden hat ihn genau der.
```

## L3877-3879 · `let scaled = libm::floor(x * p + 0.5);`

```
// `x * p + 0.5` und dann abrunden: der Gleitkommafehler von `x * p`
// gehoert dazu. `(1.005).toFixed(2)` ist "1.00", WEIL 1.005 als f64
// knapp darunter liegt — wer das wegrechnet, weicht von jedem Browser ab.
```

## L3898-3902 · `fn uri_encode(i: &mut Interp, s: &str, keep: &str) -> C<Value> {`

```
/// Der gemeinsame Rumpf von `encodeURI` und `encodeURIComponent`.
///
/// `keep` sind die Sonderzeichen, die roh durchgehen; Buchstaben und Ziffern
/// gehen immer durch. Kodiert wird UTF-8, Byte fuer Byte — genau so steht es
/// in der Spezifikation, und genau so erwartet es jeder Server.
```

## L3908-3910 · `if (0xD800..0xE000).contains(&(c as u32)) { return Err(i.throw_kind("URIError", "URI malformed")) }`

```
// Eine einzelne Haelfte eines Ersatzpaares ist kein Zeichen und laesst
// sich nicht als UTF-8 schreiben. Der Lexer laesst sie nicht entstehen,
// aber `String.fromCharCode` schon.
```

## L3921-3925 · `fn uri_decode(i: &mut Interp, s: &str, reserved: &str) -> C<Value> {`

```
/// Der gemeinsame Rumpf von `decodeURI` und `decodeURIComponent`.
///
/// `reserved` sind die Zeichen, deren Kodierung STEHEN bleibt. Ein `%` ohne
/// zwei Hexziffern dahinter ist ein URIError — nicht ein stilles `%`: eine
/// halb dekodierte Adresse sieht aus wie eine ganze.
```

## L3952-3954 · `fn lookup_accessor(i: &mut Interp, t: Value, a: &[Value], want_set: bool) -> C<Value> {`

```
/// `__lookupGetter__` / `__lookupSetter__`: die KETTE hoch, bis eine eigene
/// Eigenschaft dieses Namens da ist — und nur wenn die ein Zugriff ist, gibt
/// es etwas zurueck.
```

## L3974-3979 · `fn group_into(i: &mut Interp, a: &[Value], out: &Gc, as_map: bool) -> C<()> {`

```
/// Der gemeinsame Rumpf von `Object.groupBy` und `Map.groupBy`.
///
/// Der Unterschied ist allein der SCHLUESSEL: dort ein Eigenschaftsname (also
/// immer eine Zeichenkette oder ein Symbol), hier ein Karteneintrag mit
/// eigener Identitaet — `Map.groupBy` darf nach einem OBJEKT gruppieren, und
/// genau das ist der Grund, warum es die Variante ueberhaupt gibt.
```

## L4010-4012 · `#[derive(Clone, Copy, PartialEq)]`

```
/// Welche der sieben Mengenoperationen. Eine Umsetzung fuer alle, weil sie
/// sich nur darin unterscheiden, was mit einem Schluessel passiert, der auf
/// beiden Seiten (oder nur auf einer) steht.
```

## L4016 · `fn set_keys(t: &Value) -> Vec<Value> {`

```
/// Die eigenen Schluessel der Menge, als WERTE.
```

## L4031-4033 · `let sz = i.get(&other, "size")?;`

```
// Das MENGENPROTOKOLL: `size` als Zahl, `has` und `keys` als Funktionen.
// Die Reihenfolge der Pruefungen steht in der Spezifikation und ist
// beobachtbar.
```

## L4043 · `let theirs: Vec<Value> = if matches!(op, SetOp::Union | SetOp::Intersection`

```
// Nur holen, wer sie braucht — `isSubsetOf` fragt sonst umsonst.
```

## L4123-4126 · `fn f16round(x: f64) -> f64 {`

```
/// Auf die naechste `binary16`-Zahl runden. Rust hat `f16` in `core` nicht
/// stabil, also von Hand: Vorzeichen, 5 Bit Exponent, 10 Bit Mantisse — mit
/// den beiden Randfaellen, an denen so eine Funktion sonst falsch wird
/// (subnormal und Ueberlauf nach Unendlich).
```

## L4132 · `if a < 1.0 / 33554432.0 { return if neg { -0.0 } else { 0.0 }; }`

```
// 2^-24 ist die kleinste subnormale binary16; darunter bleibt die Null.
```

## L4138 · `if libm::fabs(a / step - (libm::floor(a / step) + 0.5)) < 1e-9 {`

```
// Zur GERADEN runden, wo es genau in der Mitte liegt.
```

## L4147-4148 · `fn ta_forward(i: &mut Interp, t: Value, a: &[Value], name: &str) -> C<Value> {`

```
/// Eine geborgte Feldmethode auf einer Sicht: erst pruefen, dass `this`
/// wirklich eine ist, dann DIESELBE Funktion rufen.
```

## L4161-4163 · `fn this_number(i: &mut Interp, t: &Value) -> C<f64> {`

```
/// `thisNumberValue` — eine Zahl oder ihre Huelle, alles andere wirft. Das
/// ist KEINE Umwandlung: `Number.prototype.toFixed.call("1")` ist ein
/// TypeError, kein "1.0".
```

## L4175-4176 · `fn has_index(i: &mut Interp, this: &Value, k: usize) -> bool {`

```
/// Gibt es den Index ueberhaupt? Fuer eine Sicht immer, fuer ein Feld nur,
/// wenn die Eigenschaft (oder eine geerbte) da ist.
```

## L4189-4191 · `pub fn this_string(i: &mut Interp, t: &Value) -> C<Rc<str>> {`

```
/// `RequireObjectCoercible` + `ToString` — der Kopf jeder
/// String.prototype-Methode. `String.prototype.trim.call(null)` ist ein
/// TypeError, nicht `"null"`.
```

## L4199-4202 · `fn this_coll(i: &mut Interp, t: &Value, name: &str) -> C<Rc<RefCell<CollData>>> {`

```
/// Ist `this` GENAU diese Sammlung? Der Vermerk `COLL_KIND` steht fuer das
/// interne Feld, das die Spezifikation verlangt — ohne ihn liefe
/// `Map.prototype.has.call({})` still durch und `…call(null)` gaebe `false`
/// statt zu werfen.
```

## L4214-4216 · `fn norm_key(v: Value) -> Value {`

```
/// `-0` als Schluessel wird zu `+0` — das verlangt die Spezifikation
/// ausdruecklich (`Map.prototype.set`, Schritt 6), und zwar fuer den
/// GESPEICHERTEN Wert, nicht nur fuer den Vergleich.
```

## L4221-4222 · `fn coll_of(t: &Value) -> Option<Rc<RefCell<CollData>>> {`

```
/// Der Speicher einer Sammlung, ohne Pruefung auf die Art — fuer die
/// Mengenoperationen, die schon wissen, dass sie ein `Set` in der Hand haben.
```

## L4229-4230 · `fn enum_own(i: &mut Interp, o: &Gc, k: &str) -> C<bool> {`

```
/// Ist die eigene Eigenschaft aufzaehlbar? Fuer einen Stellvertreter fragt
/// das seine `getOwnPropertyDescriptor`-Falle, nicht die Tabelle darunter.
```

## L4238 · `fn this_bigint(i: &mut Interp, t: &Value) -> C<super::bigint::Big> {`

```
/// `thisBigIntValue` — eine grosse Zahl oder ihre Huelle.
```

## L4250-4251 · `fn as_n(i: &mut Interp, a: &[Value], signed: bool) -> C<Value> {`

```
/// `BigInt.asIntN` / `asUintN`: auf `bits` Stellen zuschneiden, mit oder ohne
/// Vorzeichen.
```

