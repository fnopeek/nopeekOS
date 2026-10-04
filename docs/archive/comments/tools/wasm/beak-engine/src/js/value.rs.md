# `tools/wasm/beak-engine/src/js/value.rs` @ 5e0102684

## L1-12 · `use alloc::rc::Rc;`

```
//! Werte und das Objektmodell.
//!
//! **Eigenschaftsbeschreibungen von Anfang an.** Ein Objektmodell, das nur
//! Werte kennt und `writable`/`enumerable`/`configurable` spaeter nachruestet,
//! muss jede Zeile noch einmal anfassen — und test262 fragt danach in fast
//! jedem zweiten Test. Also gleich richtig.
//!
//! **Zaehlende Freigabe, kein Sammler.** `Rc` heisst: Zyklen bleiben liegen.
//! Fuer einen Browser, der Skripte je Seite laufen laesst und die Instanz beim
//! Verlassen wegwirft, ist das tragbar; fuer eine lange laufende Anwendung
//! nicht. Das ist eine bewusste Schuld, keine Nachlaessigkeit — und die Stelle,
//! an der ein Sammler ansetzen wuerde, ist genau `Gc`.
```

## L30 · `BigInt(Rc<crate::js::bigint::Big>),`

```
/// Eine ganze Zahl ohne Groessengrenze. Ein PRIMITIV, kein Objekt.
```

## L49-50 · `let kind = &o.borrow().kind;`

```
// Eine GEBUNDENE Funktion ist eine, und ein Stellvertreter ist
// eine, wenn sein Ziel eine ist. Beides fehlte hier.
```

## L80 · `pub fn strict_eq(&self, other: &Value) -> bool {`

```
/// `===`. Der einzige Haken ist NaN (nie gleich) und `-0 === 0` (gleich).
```

## L87-89 · `(Value::Sym(a), Value::Sym(b)) => a.key == b.key,`

```
// Zwei Symbole sind dasselbe, wenn ihr SCHLUESSEL derselbe ist —
// und der ist je Symbol einmalig. Damit ist die Identitaet nicht
// an den `Rc` gebunden, und `Symbol.for` darf ihn frisch bauen.
```

## L97-98 · `pub fn same_value(&self, other: &Value) -> bool {`

```
/// `Object.is`-Gleichheit: wie `===`, aber NaN ist sich selbst gleich und
/// `-0` ist nicht `0`. `assert._isSameValue` baut genau das nach.
```

## L108-121 · `pub type PropName = Rc<str>;`

```
/// Ein Eigenschaftsname — Zeichenkette ODER Symbol, in EINER Tabelle.
///
/// Ein Symbol traegt seinen Schluessel als Zeichenkette mit fuehrendem
/// NUL-Byte. Das ist kein Trick, um Arbeit zu sparen, sondern die Entscheidung
/// gegen eine zweite Eigenschaftstabelle: mit einem eigenen Schluesseltyp
/// waere jede Suche `HashMap<PropName, _>` und muesste fuer jedes `o.foo`
/// erst einen `PropName` bauen — eine Allokation auf dem heissesten Pfad der
/// Maschine. So bleibt `get_own(&str)` unveraendert und kostenlos.
///
/// Der Preis, ehrlich benannt: eine Seite, die `obj["\0#7"]` schreibt, trifft
/// den Namensraum der Symbole. Kein Zeichen ist von aussen unerreichbar —
/// `String.fromCharCode(0)` gibt es. Ein fuehrendes NUL ist aber die Form,
/// die in echtem Code nicht vorkommt, und die Pruefung kostet EIN Byte
/// (`is_sym_key`), nicht einen Praefixvergleich.
```

## L124-125 · `pub struct SymData {`

```
/// Ein Symbol. `key` ist der Eigenschaftsname, unter dem es in Objekten liegt,
/// und zugleich seine Identitaet ([[Value::strict_eq]]).
```

## L129 · `pub registered: Option<Rc<str>>,`

```
/// Gesetzt bei `Symbol.for` — `Symbol.keyFor` gibt genau das zurueck.
```

## L133 · `#[inline]`

```
/// Gehoert dieser Eigenschaftsname einem Symbol?
```

## L137-160 · `pub fn is_private_key(k: &str) -> bool { k.as_bytes().starts_with(b"\0~") }`

```
/// Der Schluessel, unter dem ein privates Feld `#name` liegt.
///
/// Das NUL davor ist der ganze Trick: solche Schluessel fallen aus
/// `own_keys` heraus, und damit ist ein privates Feld fuer `Object.keys`,
/// `for..in`, `JSON.stringify` und die Streuung unsichtbar — ohne einen
/// zweiten Speicher neben der Eigenschaftstabelle.
///
/// **Was das NICHT leistet:** zwei Klassen, die beide ein `#p` auf DEMSELBEN
/// Objekt anlegen, teilen es sich. Echte Motoren schluesseln nach Klasse.
/// Der Fall verlangt, dass eine Klasse ein fremdes Objekt als `this`
/// bekommt; die Vereinfachung ist benannt und nicht still.
/// Liegt hier ein privates Feld?
///
/// Der Schluessel eines Symbols faengt ebenfalls mit NUL an — die beiden zu
/// trennen ist Pflicht, sonst gibt `Object.getOwnPropertySymbols` das private
/// Feld als Symbol heraus, und damit ist es nicht mehr privat. Genau das ist
/// beim ersten Lauf passiert.
///
/// **Und das zweite Zeichen ist deshalb `~` und nicht `#`:** ein
/// gewoehnliches Symbol liegt schon unter `\0#<n>:<beschreibung>`
/// (`Interp::new_symbol`). Der erste Entwurf nahm `#`, und damit verschwanden
/// die echten Symbole aus `getOwnPropertySymbols` — ein Feld fuer Neues, das
/// das Alte umbringt. Die Marker sind: `@` wohlbekannt, `*` registriert,
/// `#` gewoehnlich, `~` privat.
```

## L167-169 · `pub const PRIVATE_PREFIX: &str = "\0~";`

```
/// Das Vorzeichen eines privaten Feldes im Schluesseltext. Ein Skript kann
/// es nicht erzeugen — NUL steht in keinem Bezeichner —, also ist „faengt
/// damit an" ein sicheres Erkennungsmerkmal fuer die Markenpruefung.
```

## L172 · `pub fn private_name(key: &str) -> &str {`

```
/// Der Name ohne Vorzeichen, fuer die Fehlermeldung.
```

## L177-187 · `pub fn sym_from_key(k: &PropName) -> SymData {`

```
/// Aus einem Symbolschluessel das Symbol zurueckgewinnen.
///
/// Nicht Bequemlichkeit, sondern Notwendigkeit:
/// `Object.getOwnPropertySymbols` gibt SYMBOLE zurueck, und in der Tabelle
/// steht nur der Schluessel. Also traegt der Schluessel alles, was ein Symbol
/// ausmacht — Beschreibung und Registrierung — und ist trotzdem einmalig.
///
///   `\0@iterator`  wohlbekannt  → `Symbol.iterator`
///   `\0*name`      registriert  → `Symbol.for("name")`
///   `\0#7`         anonym       → `Symbol()`
///   `\0#7:text`    beschrieben  → `Symbol("text")`
```

## L201-203 · `pub const SYM_ITERATOR: &str = "\0@iterator";`

```
/// Die wohlbekannten Symbole. Ihr Schluessel ist eine Konstante, damit
/// eingebauter Code `self.get(v, SYM_ITERATOR)` schreiben kann, ohne das
/// Symbolobjekt erst zu suchen.
```

## L220-222 · `pub const IT_TARGET: &str = "\0!target";`

```
/// Der Zustand eines eingebauten Iterators. Auch NUL-praefigiert, also aus
/// `own_keys` heraus und fuer jedes Skript unsichtbar — `native` nimmt keinen
/// Abschluss, der Zustand muss also irgendwo am Objekt liegen.
```

## L224-227 · `pub const COLL_KIND: &str = "\0!coll";`

```
/// Der Ersatz fuer das interne Feld `[[SetData]]`/`[[MapData]]`: welche
/// Sammlung das hier IST. NUL-praefigiert, also fuer jedes Skript unsichtbar
/// — ohne den Vermerk waere eine selbstgebaute Menge von einer echten nicht
/// zu unterscheiden, und `Set.prototype.union.call({…})` liefe still durch.
```

## L232 · `pub const IT_KIND: &str = "\0!kind";`

```
/// 0 = Werte, 1 = Schluessel, 2 = Paare.
```

## L235 · `pub const WELL_KNOWN: &[(&str, &str)] = &[`

```
/// Die Liste, aus der `Symbol.iterator` & Co. am globalen Objekt entstehen.
```

## L264-275 · `#[derive(Clone, Default)]`

```
/// Ein PARTIELLER Beschreiber — was `Object.defineProperty` bekommt.
///
/// **Nicht dasselbe wie `Prop`, und das ist der ganze Punkt.** `Prop` ist
/// eine fertig abgelegte Eigenschaft, dort ist `writable` ein `bool`. Ein
/// Beschreiber dagegen hat FEHLENDE Felder, und „fehlt" heisst „lass, wie es
/// ist" — nicht „false". Bis 0.99.0 fielen beide auf `Prop` zusammen, und
/// damit war `defineProperty(o,'x',{value:1})` auf einer schreibbaren
/// Eigenschaft ein stilles `writable = false`.
///
/// `Some(Value::Undefined)` heisst „steht da und ist `undefined`" und ist
/// etwas anderes als `None` — `{get: undefined}` macht eine
/// Zugriffseigenschaft ohne Leser.
```

## L289 · `pub fn is_generic(&self) -> bool { !self.is_accessor() && !self.is_data() }`

```
/// Weder das eine noch das andere — `{enumerable: true}` allein.
```

## L294 · `pub fn from_prop(p: &Prop) -> Desc {`

```
/// Der Beschreiber einer BESTEHENDEN Eigenschaft: vollstaendig besetzt.
```

## L307-309 · `pub fn into_new_prop(self) -> Prop {`

```
/// Was daraus wird, wenn die Eigenschaft NEU angelegt wird: jedes
/// fehlende Feld bekommt seinen Vorgabewert, und der ist `false`/
/// `undefined` — nur HIER, nicht beim Aendern.
```

## L314-317 · `get: self.get,`

```
// **`undefined` bleibt stehen.** `{set: undefined}` ist eine
// ZUGRIFFSeigenschaft ohne Schreiber — filtert man das auf `None`,
// sieht sie hinterher aus wie eine Datenbeschreibung, und ein
// erneutes Umdefinieren wird faelschlich abgelehnt.
```

## L331-333 · `pub fn builtin(v: Value) -> Prop {`

```
/// Was eingebaute Eigenschaften tragen: schreibbar und konfigurierbar,
/// aber NICHT aufzaehlbar (ES 17). Ein `for..in` ueber ein frisches Objekt
/// darf `toString` nicht sehen.
```

## L337-341 · `pub fn tag(v: Value) -> Prop {`

```
/// Was ein `@@toStringTag` (und `Symbol.prototype[@@toPrimitive]`) traegt:
/// nicht schreibbar, nicht aufzaehlbar, aber **konfigurierbar** (ES 17).
/// `Prop::frozen` war dafuer der falsche Konstruktor — er sperrt auch das
/// Umdefinieren, und das faellt erst auf, wenn `defineProperty` wirklich
/// prueft.
```

## L357 · `pub ctor: bool,`

```
/// Darf mit `new` gerufen werden (Konstruktor)?
```

## L364-365 · `pub this_val: Option<Value>,`

```
/// Gebundenes `this` — Pfeile erben es, gewoehnliche Funktionen bekommen
/// es beim Aufruf.
```

## L368-374 · `pub class: Option<Rc<crate::js::ast::Class>>,`

```
/// Die Klasse, deren KONSTRUKTOR das hier ist — und nur dann gesetzt,
/// wenn sie Instanzfelder hat.
///
/// Ein Instanzfeld gehoert weder auf den Prototyp (es ist je Instanz)
/// noch in den Rumpf (es steht dort nicht). Es gehoert an den Aufruf, und
/// der Aufruf braucht dafuer die Liste — hier liegt sie. `env` daneben ist
/// schon der richtige Bereich fuer die Initialisierer.
```

## L378-383 · `pub struct BufData {`

```
/// Der Bytespeicher hinter jedem TypedArray und jeder DataView.
///
/// **Ein `ArrayBuffer` ist der Speicher, ein TypedArray nur eine SICHT
/// darauf.** Zwei Sichten auf denselben Puffer sehen einander — das ist der
/// Punkt der ganzen Familie, und deshalb liegt der Speicher hier und nicht
/// in der Sicht.
```

## L386-387 · `pub detached: core::cell::Cell<bool>,`

```
/// Abgetrennt. Danach hat der Puffer die Laenge 0 und jede Sicht darauf
/// ist leer; test262 loest das ueber `$262.detachArrayBuffer` aus.
```

## L391 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Die Elementart einer Sicht.
```

## L404-405 · `pub fn is_big(self) -> bool { matches!(self, ElemKind::I64 | ElemKind::U64) }`

```
/// Traegt diese Art GROSSE Zahlen? Dann ist ein Element ein `BigInt`, und
/// jeder Schreiber muss `ToBigInt` statt `ToNumber` rufen.
```

## L418 · `pub fn read_v(self, b: &[u8], at: usize) -> Value {`

```
/// Ein Element als JS-Wert — bei den 64-Bit-Arten eine grosse Zahl.
```

## L431 · `pub fn write_big(self, b: &mut [u8], at: usize, v: &crate::js::bigint::Big) {`

```
/// Eine grosse Zahl schreiben — abgeschnitten auf 64 Bit.
```

## L436-438 · `pub fn read(self, b: &[u8], at: usize) -> f64 {`

```
/// Ein Element lesen. Immer LITTLE ENDIAN — das ist, was jede Plattform
/// tut, auf der dieser Code laeuft, und die Spec laesst der Sicht (anders
/// als der `DataView`) keine Wahl.
```

## L454-455 · `ElemKind::I64 => g(8) as i64 as f64,`

```
// Ueber `read_v` zu holen; hier nur, damit der Uebersetzer die
// Vollstaendigkeit prueft.
```

## L460-462 · `pub fn write(self, b: &mut [u8], at: usize, v: f64) {`

```
/// Ein Element schreiben. Die Umwandlung ist die der Spec: NaN und
/// Unendlich werden bei den Ganzzahlen zu 0, der Rest wird modulo
/// abgeschnitten — ausser bei `Uint8Clamped`, das KLEMMT und rundet.
```

## L471-472 · `let f = libm::floor(v);`

```
// Zur naechsten GERADEN bei .5 — die Spec sagt das
// ausdruecklich, und `round` taete es nicht.
```

## L491-494 · `pub fn to_uint_wrap(v: f64, bits: usize) -> u64 {`

```
/// `ToIntegerOrInfinity` + modulo 2^bits — der gemeinsame Rumpf von
/// `ToInt8`/`ToUint8`/`ToInt16`/… Ein eigener Name, weil die Regel
/// (NaN und Unendlich zu 0, dann abschneiden, dann modulo) an neun Stellen
/// dieselbe ist.
```

## L496-504 · `let full = to_uint32(v) as u64;`

```
// **Ueber `to_uint32`, nicht ueber einen eigenen f64→u64-Cast.** Der
// uebersetzt zu `i64.trunc_sat_f64_u`, und forge kann diesen Befehl
// nicht — ein Modul mit ihm bliebe AM GERAET beim ersten Aufruf der
// Stelle stehen, nicht beim Laden. Das forge-Tor hat ihn gemeldet, bevor
// irgendetwas signiert war; ohne das Tor waere er in einer Freigabe
// gelandet und erst beim Benutzen aufgefallen.
//
// Die Rechnung ist dieselbe: `ToUint32` schneidet ab und rechnet modulo
// 2^32, und alles darunter ist ein Ausschnitt davon.
```

## L509 · `pub struct TaData {`

```
/// Eine SICHT auf einen Puffer.
```

## L513 · `pub offset: usize,`

```
/// Byteversatz im Puffer.
```

## L515 · `pub len: usize,`

```
/// Anzahl ELEMENTE, nicht Bytes.
```

## L519-520 · `pub struct DvData {`

```
/// Die ungetypte Sicht: jeder Zugriff nennt seine Art und seine Bytefolge
/// selbst. Deshalb steht hier keine `ElemKind`.
```

## L528-529 · `pub fn live_len(&self) -> usize {`

```
/// Wieviele Elemente die Sicht WIRKLICH hat — ein abgetrennter Puffer
/// macht sie leer, ohne dass jemand die Sicht anfasst.
```

## L544 · `Bound { target: Gc, this_val: Value, args: Vec<Value> },`

```
/// Gebundene Funktion aus `Function.prototype.bind`.
```

## L555-562 · `ModuleNs(Rc<str>),`

```
/// Der Namensraum eines Moduls, mit dessen Adresse.
///
/// **Ein exotisches Objekt, kein gewoehnliches** (ES2024 §10.4.6): seine
/// Eigenschaften sind LEBENDE Bindungen, keine Werte. `export let canvas`
/// plus ein `setCanvas()` heisst, dass `ns.canvas` NACH dem Setzer den
/// neuen Wert zeigen muss — eine Momentaufnahme zeigt fuer immer `null`.
/// Die Eigenschaftstabelle bleibt trotzdem gefuellt: sie ist es, die
/// `Object.keys(ns)` und `for..in` beantwortet.
```

## L564-570 · `Dataset(u32),`

```
/// `el.dataset` — die Zahl ist der Knoten, dem es gehoert.
///
/// **Eine eigene Art, weil das SCHREIBEN durchgreifen muss.** Bis 0.172 war
/// `dataset` eine Momentaufnahme: `el.dataset.theme = 'light'` legte eine
/// gewoehnliche Eigenschaft an und liess das Attribut stehen. Auf
/// `sandbox.nopeek.ch` ist genau das der Theme-Schalter — das Blatt waehlt
/// mit `[data-theme="light"]`, und die Seite blieb dunkel.
```

## L572 · `Buffer(Rc<BufData>),`

```
/// Der Speicher (`ArrayBuffer`) und die zwei Sichten darauf.
```

## L576-578 · `Proxy(crate::js::proxy::ProxyCell),`

```
/// Ein Stellvertreter: Ziel und Behandler, oder `None` nach dem
/// Widerruf. Siehe `proxy.rs` — die Fallen sitzen in den
/// Grundoperationen, nicht hier.
```

## L580-581 · `Date(Rc<core::cell::Cell<f64>>),`

```
/// Der Zeitwert eines `Date`. Als eigene Art und nicht als Eigenschaft,
/// damit er nicht in `Object.getOwnPropertyNames` auftaucht.
```

## L583-585 · `Generator(Rc<crate::js::generator::GenState>),`

```
/// Ein angehaltener Generator: seine EIGENE Maschine, samt Zustand.
/// Siehe `generator.rs` — und den Kopf von `vm.rs` fuer den Grund, warum
/// es eine eigene ist und kein Rahmen in einer fremden.
```

## L587 · `Collection(Rc<RefCell<CollData>>),`

```
/// Der Inhalt einer `Map`/`Set`/`WeakMap`/`WeakSet`.
```

## L591-601 · `#[derive(Clone, PartialEq, Eq, Hash)]`

```
/// Ein Sammlungsschluessel, so wie SameValueZero ihn vergleicht.
///
/// **Warum eine eigene Darstellung.** Bis 0.103.0 lag ein Eintrag als
/// Eigenschaft `@<Zeichenkette des Schluessels>` im Objekt. Das hat drei
/// Dinge falsch gemacht, und alle drei sind an echtem Code aufgefallen: zwei
/// verschiedene Objekte mit gleichem `toString` waren EIN Eintrag, `1` und
/// `"1"` waren derselbe Schluessel, und schon das Nachschlagen RIEF das
/// `toString` des Schluessels. core-js legt seinen internen Zustand in einer
/// `WeakMap` ab, deren Schluessel Funktionen sind — der Aufruf ging in deren
/// `Function.prototype.toString`, das core-js im selben Modul gerade ersetzt
/// hatte, und von dort im Kreis, bis der Aufrufdeckel griff.
```

## L607-609 · `Num(u64),`

```
/// Die Bits der Zahl — mit `-0` auf `+0` gelegt und JEDEM NaN auf
/// dasselbe Muster. Das ist der einzige Unterschied zwischen
/// SameValueZero und `Object.is`.
```

## L614-616 · `Obj(usize),`

```
/// Die ADRESSE, nicht der Inhalt: zwei gleich aussehende Objekte sind
/// zwei Schluessel. Sie bleibt gueltig, weil `entries` den Schluessel
/// selbst festhaelt — die Sammlung haelt ihn also am Leben.
```

## L638-643 · `#[derive(Default)]`

```
/// Die Eintraege einer Sammlung.
///
/// **Weich, nicht schwach.** `WeakMap` haelt hier genauso fest wie `Map`. Ein
/// echtes schwaches Halten braucht einen Sammler, und den gibt es nicht
/// (siehe Modulkopf) — die Alternative waere ein Schluessel, der still
/// verschwindet, und das waere schlimmer als einer, der zu lange lebt.
```

## L646-649 · `pub entries: Vec<Option<(Value, Value)>>,`

```
/// Die Eintraege in EINFUEGEREIHENFOLGE. Geloescht heisst `None` statt
/// entfernt: `forEach` und die Iteratoren laufen ueber Indizes, und ein
/// Zusammenschieben wuerde sie mitten im Lauf verschieben — genau der
/// Fall, den die Spezifikation ausdruecklich regelt.
```

## L651-652 · `pub index: HashMap<CollKey, usize>,`

```
/// Schluessel -> Index. Ohne ihn waere jedes `get` ein Durchlauf, und
/// core-js fragt seinen Zustand bei JEDEM Zugriff.
```

## L662-663 · `pub fn set(&mut self, k: Value, v: Value) {`

```
/// Setzen. Ein vorhandener Schluessel behaelt seinen PLATZ — die
/// Reihenfolge richtet sich nach dem ersten Einfuegen.
```

## L680-681 · `pub fn pairs(&self) -> Vec<(Value, Value)> {`

```
/// Die lebenden Paare als Liste — die Momentaufnahme, aus der `keys`,
/// `values` und `entries` ihren Iterator bauen.
```

## L687-693 · `#[cfg(feature = "heap-census")]`

```
/// Wieviele `Object` gerade LEBEN.
///
/// **Die Zahl, ohne die „erreichbar" nichts aussagt.** Ein Markierungsgang
/// von den Wurzeln zaehlt, was zu FINDEN ist; erst der Vergleich mit dem
/// Gesamtbestand sagt, wieviel davon in einem Ring liegt, den `Rc` nie
/// aufloest. Nur mit `--features heap-census`, damit das ausgelieferte Modul
/// keinen Zaehler je Objekt traegt.
```

## L712-715 · `order: Vec<PropName>,`

```
/// Einfuegereihenfolge. JS gibt Eigenschaften in einer FESTGELEGTEN
/// Reihenfolge zurueck (ganzzahlige Schluessel aufsteigend zuerst, dann
/// der Rest in Einfuegereihenfolge) — eine reine Hashtabelle kann das
/// nicht, also steht die Reihenfolge daneben.
```

## L734-741 · `pub fn is_enumerable(&self, k: &str) -> bool {`

```
/// Ist dieser eigene Schluessel aufzaehlbar?
///
/// **Eigene Funktion, weil die Antwort nicht immer in der Tabelle steht.**
/// Die Indizes einer Sicht (`TypedArray`) sind aufzaehlbar, obwohl es zu
/// ihnen keinen Eintrag gibt — sie entstehen aus der Laenge. Acht Stellen
/// haben das frueher mit `get_own(k).map(|p| p.enumerable)` gefragt und
/// bekamen fuer eine Sicht achtmal `false`: `Object.keys`, `for..in`,
/// `JSON.stringify` und die Streuung sahen ein leeres Objekt.
```

## L755-762 · `pub fn clear_props(&mut self) {`

```
/// Alle Index-Eigenschaften in EINEM Durchgang entfernen.
///
/// `remove` je Schluessel laeuft die Reihenfolgeliste jedes Mal ab — bei
/// n Schluesseln also O(n²), und das an der Schrittgrenze vorbei. Genau
/// daran ist der Lauf von 20 auf ueber 60 Sekunden gestiegen, nachdem
/// `shift`/`splice`/`sort` dazukamen.
/// Alles wegnehmen. Nur fuer den Abbau eines Realms — siehe
/// `Interp::teardown`, dort steht, warum es das braucht.
```

## L780-789 · `pub fn raw_keys(&self) -> Vec<PropName> { self.order.clone() }`

```
/// Eigene ZEICHENKETTEN-Schluessel in der Reihenfolge, die die
/// Spezifikation vorschreibt: ganzzahlige Indizes aufsteigend, danach
/// alles andere in Einfuegereihenfolge.
///
/// Symbole sind hier NICHT dabei — jeder Aufrufer (`Object.keys`,
/// `for..in`, `JSON.stringify`, `getOwnPropertyNames`) will genau das.
/// Wer Symbole braucht, nimmt `own_sym_keys`.
/// JEDER Schluessel, auch die NUL-praefigierten. `own_keys` laesst die
/// absichtlich weg — wer aber ein Objekt UEBERNIMMT (`super()` auf einen
/// eingebauten Konstruktor), braucht auch die inneren Vermerke.
```

## L793-796 · `if let ObjKind::TypedArray(t) = &self.kind {`

```
// **Eine Sicht traegt ihre Indizes nicht in der Tabelle.** Sie
// entstehen aus der Laenge, und ohne diesen Zweig faende
// `Object.keys(ta)` nichts — auch `for..in` und `JSON.stringify`
// nicht.
```

## L822 · `pub fn own_sym_keys(&self) -> Vec<PropName> {`

```
/// Eigene SYMBOL-Schluessel, in Einfuegereihenfolge.
```

## L830-832 · `pub fn array_index(k: &str) -> Option<u32> {`

```
/// Ist `k` ein Array-Index? Die Regel ist eng: eine kanonische Dezimalzahl
/// ohne fuehrende Null, kleiner als 2^32-1. `"01"` und `"1.0"` sind KEINE
/// Indizes, und daran haengt die Reihenfolge der Schluessel.
```

## L840-844 · `pub fn num_to_string(n: f64) -> String {`

```
/// Zahl zu Zeichenkette, nach den Regeln von JS.
///
/// Nicht `format!("{}")`: Rust schreibt `1` als `1` (gut), aber auch `1e21`
/// als `1000000000000000000000` und `f64::INFINITY` als `inf`. JS hat eigene
/// Regeln, und Zahlen werden staendig zu Text.
```

## L850-862 · `if libm::fabs(n) < 9007199254740992.0 && libm::trunc(n) == n {`

```
// **Schnellweg: eine ganze Zahl.** Ein Index, eine Laenge, ein Zaehler —
// das ist der ueberwiegende Fall, und die Antwort ist die Ziffernfolge.
//
// Der Weg darunter zahlt dafuer `format!("{:e}")`: einen Haldenpuffer,
// Grisu, ein Filtern ueber die Ziffern und einen zweiten Puffer. Auf
// einer Google-Suchseite waren das **48 % der Laufzeit** — nicht weil
// die Seite mehr rechnet als in einem Browser, sondern weil jede Zahl,
// die zu einem Eigenschaftsschluessel wird, diesen Weg nahm.
//
// Bis 2^53 ist eine ganze Zahl in `f64` exakt, und unter 1e21 schreibt
// JS sie schlicht aus (ES 6.1.6.1.20, Fall `1 <= pt <= 21`) — die beiden
// Bedingungen decken sich also, und der lange Weg bleibt fuer alles
// andere zustaendig.
```

## L875-878 · `let neg = n < 0.0;`

```
// ES 6.1.6.1.20 waehlt die Form nach der KOMMASTELLE, nicht nach der
// Groesse: `s * 10^(n-k)` mit der kuerzesten Ziffernfolge `s` (k Ziffern).
// Rusts `{:e}` liefert genau diese kuerzeste Folge — sie selbst zu
// rechnen hiesse Grisu nachzubauen.
```

## L887 · `let pt = exp + 1;                    // wo das Komma steht`

```
// wo das Komma steht
```

## L913-919 · `pub fn num_to_radix(v: f64, radix: u32) -> String {`

```
/// `Number.prototype.toString(radix)` fuer eine Basis ausser 10.
///
/// Der Weg ist der von V8 (`DoubleToRadixCString`) und nicht ein eigener:
/// die Spezifikation laesst die Nachkommastellen ausdruecklich offen
/// („implementation-approximated"), und zwei Motoren, die hier verschieden
/// runden, geben verschiedene Farbwerte aus. Byte-gleich zu node
/// gegengeprueft.
```

## L931-932 · `let mut delta = 0.5 * (libm::nextafter(value, f64::INFINITY) - value);`

```
// Der Abstand zur naechsten darstellbaren Zahl ist die Abbruchgrenze:
// weiter zu rechnen hiesse Ziffern zu drucken, die im `f64` nicht stehen.
```

## L944-945 · `loop {`

```
// Aufrunden mit Uebertrag — und laeuft der bis vor die erste
// Stelle, waechst die Ganzzahl.
```

## L960-961 · `let mut int_digits: Vec<u8> = Vec::new();`

```
// Ueber 2^53 traegt der `f64` die unteren Stellen nicht mehr — dort
// stehen Nullen, und das ist ehrlicher als erfundene Ziffern.
```

## L984-985 · `pub fn string_to_num(s: &str) -> f64 {`

```
/// Zeichenkette zu Zahl (`Number("…")`). Leerraum ringsum, `0x`/`0o`/`0b`,
/// `Infinity`, und leer = 0.
```

## L1005-1006 · `pub fn to_int32(n: f64) -> i32 {`

```
/// `ToInt32` — die Umwandlung hinter den Bitoperatoren. Modulo 2^32 mit
/// Vorzeichen; NaN und Unendlich werden 0.
```

## L1015-1021 · `pub fn f64_to_usize(v: f64) -> usize {`

```
/// `f64` zu einer kleinen Ganzzahl, OHNE `i64.trunc_sat_f64_u`.
///
/// **forge kann diesen Befehl nicht**, und ein Modul mit ihm bleibt am Geraet
/// beim ERSTEN Aufruf der Stelle stehen — nicht beim Laden, wo es auffiele.
/// Der Umweg ueber `u32` ist fuer alles, was hier gezaehlt wird (Monate,
/// Ziffern, Schiebeweiten, Indizes), derselbe Wert und uebersetzt.
/// `python3 tools/forge-gate.py` ist der Vorablauf, der das prueft.
```

## L1028 · `pub fn to_integer(n: f64) -> f64 {`

```
/// `ToInteger`: abschneiden, NaN zu 0.
```

## L1038-1047 · `#[cfg(feature = "heap-census")]`

```
/// Jedes Objekt, das je gebaut wurde — als SCHWACHER Verweis.
///
/// **Ohne ein Verzeichnis kann niemand kehren.** Ein Sammler markiert von den
/// Wurzeln aus und raeumt dann alles weg, was er nicht gefunden hat — und
/// „alles" muss man aufzaehlen koennen. `Rc` kann das nicht; diese Liste
/// schon. Sie haelt nichts fest (`Weak`), kostet also einen Zeiger je Objekt
/// und ein Feld in der Liste.
///
/// Nur mit `--features heap-census`: das ausgelieferte Modul traegt sie
/// nicht, solange es keinen Sammler gibt, der sie benutzt.
```

## L1052 · `#[cfg(feature = "heap-census")]`

```
/// Ab welcher Laenge das Verzeichnis das naechste Mal aufgeraeumt wird.
```

## L1061-1065 · `if all.len() >= NEXT_COMPACT {`

```
// **Verdoppelnd aufraeumen, nicht nach einem Verhaeltnis.** Ein Gang
// ueber die Liste ist O(n); ihn immer dann zu laufen, wenn sie
// doppelt so lang ist wie der Bestand, laeuft ihn bei einer Seite,
// die staendig Muell macht, fast bei jedem Objekt — und aus O(n) wird
// O(n²). Die Marke verdoppelt sich stattdessen nach jedem Gang.
```

## L1075 · `pub fn native(proto: Option<Gc>, f: NativeFn, name: &str, length: usize, ctor: bool) -> Gc {`

```
/// Damit `Box<dyn …>` nicht noetig ist, wenn ein natives Objekt gebaut wird.
```

## L1092-1095 · `#[test]`

```
/// **`Number::toString` hat eigene Regeln, und der Schnellweg muss sie
/// treffen.** Die Tabelle steht gegen echtes JS, nicht gegen die alte
/// Fassung: eine Probe, die nur „wie vorher" prueft, zementiert einen
/// Fehler, statt ihn zu finden.
```

## L1101-1102 · `(9007199254740991.0, "9007199254740991"),`

```
// Genau an der Grenze des Schnellwegs: 2^53-1 geht darueber,
// 2^53 faellt auf den langen Weg — beide muessen gleich lauten.
```

## L1105 · `(1e20, "100000000000000000000"),`

```
// Und darueber, wo JS immer noch ausschreibt (pt <= 21).
```

## L1107 · `(1e21, "1e+21"),`

```
// Ab 1e21 exponentiell.
```

