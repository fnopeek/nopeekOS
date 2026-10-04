# `tools/wasm/beak-engine/src/js/expr.rs` @ 5e0102684

## L1 · `use alloc::boxed::Box;`

```
//! Ausdruecke auswerten.
```

## L63-65 · `if *op == BinOp::In {`

```
// `#x in obj` fragt nach der MARKE, nicht nach einer
// Eigenschaft — und die linke Seite ist kein Wert, den man
// auswerten koennte.
```

## L110-112 · `let name = super::compile::dotted_name(callee);`

```
// Derselbe Name wie in der Befehlsmaschine — beide Maschinen
// muessen dieselbe Meldung geben, sonst sagt ein Wechsel des
// Weges etwas anderes ueber denselben Fehler.
```

## L118-121 · `Expr::MetaProp { meta, prop } => {`

```
// `import.meta` liegt als Bindung in der Modulumgebung; die Kette
// gibt die richtige, auch aus einem Rueckruf heraus. Ausserhalb
// eines Moduls gibt es sie nicht — dann `undefined`, wie
// `new.target`.
```

## L132-136 · `Expr::TaggedTemplate { tag, quasis, exprs } => {`

```
// `tag`a${x}b`` (ES 13.2.8.6): die Marke bekommt den
// Vorlagen-Gegenstand als erstes Argument und danach die
// Einsetzungen. **Der Empfaenger gehoert dazu** — `o.tag`x``
// ruft mit `o` als `this`, genau wie ein gewoehnlicher Aufruf;
// `String.raw` ist die eingebaute Marke, die das ausnutzt.
```

## L169-177 · `pub(crate) fn with_has_pub(&mut self, o: &Gc, n: &str) -> C<bool> { self.with_has(o, n) }`

```
/// Wie `load_ident`, sagt aber MIT, in welcher Tiefe der Name stand —
/// `None` heisst „nicht in der Kette, das globale Objekt hat geantwortet".
/// Nur der Wegweiser braucht das; siehe `Chunk::hints`.
/// Hat die Objektumgebung eines `with` diesen Namen?
///
/// Nicht bloss `HasProperty`: `Symbol.unscopables` kann einen Namen
/// AUSBLENDEN, obwohl er da ist. Genau dafuer gibt es die Tabelle —
/// `with([]) { keys }` darf nicht `Array.prototype.keys` finden, sonst
/// bricht Code, der aelter ist als die Methode.
```

## L191-196 · `let mut cur = env.clone();`

```
// **EIN Durchgang durch die Kette.** Bis 0.117.0 liefen hier
// nacheinander `env_lookup`, `env_deref` und zwei `vars.get`: derselbe
// Name bis zu VIERMAL gehasht und verglichen, und `env_deref` baute
// dafuer jedes Mal ein `Rc<str>` auf dem Haufen — fuer eine
// Import-Kette, die fast nie betreten wird. `LoadVar` ist 24 % aller
// Befehle.
```

## L200-201 · `let wo = cur.borrow().with_obj.clone();`

```
// **Die Objektumgebung eines `with` zuerst.** Ein `None` ist eine
// Nullpruefung; nur wo wirklich ein `with` steht, kostet es etwas.
```

## L205-206 · `return Ok((self.get(&Value::Obj(o), n)?, None));`

```
// KEINE Tiefe zurueck: ein Wegweiser darf auf eine
// Bindung, die aus einem Objekt kommt, nicht zeigen.
```

## L215-216 · `Hit::Import(e, n2) => return Ok((self.load_import(e, n2, n)?, None)),`

```
// Ein importierter Name steht nicht HIER, sondern im Modul,
// aus dem er kommt — und er wird bei JEDEM Lesen dort geholt.
```

## L222-223 · `let g = self.realm.global.clone();`

```
// Nicht in der Kette: das globale Objekt fragen, sonst ReferenceError.
// Der Unterschied zu `undefined` ist der ganze Sinn der Sache.
```

## L229-231 · `fn load_import(&mut self, e: Rc<RefCell<Env>>, n2: Rc<str>, n: &str) -> C<Value> {`

```
/// Einem `import` bis zur echten Bindung folgen — der SELTENE Weg.
/// Hier darf ein `Rc<str>` entstehen; auf dem gewoehnlichen entsteht
/// keins mehr.
```

## L248-254 · `pub fn super_call(&mut self, args: &[Value], env: &Rc<RefCell<Env>>) -> C<Value> {`

```
/// Der Prototyp, auf dem `super` sucht: der des Heimatobjekts.
/// `super(...)` — den Elternkonstruktor auf DIESEM `this` fahren.
///
/// Kein eigener Empfaenger, kein eigenes Objekt: das gibt es schon,
/// `construct` hat es angelegt, bevor der Koerper lief. Eigene Funktion,
/// weil die Befehlsmaschine sie ruft — und weil die Instanzfelder daran
/// haengen.
```

## L262-266 · `let native_parent = matches!(&ctor, Value::Obj(o)`

```
// Ein EINGEBAUTER Elternkonstruktor baut sein eigenes Objekt und kann
// `this` gar nicht fuellen — `super()` in `class E extends Error {}`
// hat die Meldung bisher weggeworfen. Also wird das Gebaute
// UEBERNOMMEN: Art und eigene Eigenschaften wandern hinueber, der
// Prototyp der abgeleiteten Klasse bleibt.
```

## L270-274 · `let built = self.construct_on(&ctor, this_val.clone(), args)?;`

```
// Der eingebaute Konstruktor bekommt das frisch angelegte `this`
// als EMPFAENGER. Er baut trotzdem sein eigenes Objekt, aber ohne
// diese Zeile wuesste er nicht, welche Klasse gerade gebaut wird
// — und `HTMLElement` braucht genau das, um die angemeldete
// Marke aus der Prototypkette zu lesen.
```

## L284-289 · `super::dombind::readopt(self, src, dst);`

```
// **Und der Verweis zurueck.** Ein eingebauter Konstruktor
// kann sein Objekt anderswo eingetragen haben — `HTMLElement`
// haengt es an den DOM-Knoten. Wird die Kopie hier nicht
// nachgezogen, zeigen Baum und Instanz auf ZWEI Objekte: die
// Komponente traegt ihre Felder, der Knoten kennt sie nicht,
// und `document.querySelector` liefert die falsche Haelfte.
```

## L294-299 · `if let Value::Obj(o) = &r {`

```
// **Gibt der Elternkonstruktor ein OBJEKT zurueck, ist DAS `this`.**
// `class Base { constructor(o) { return o } }` ist das Muster
// hinter jeder Klasse, die eine bestehende Instanz zurueckreicht.
// Bis 0.99.0 warf `super()` die Antwort weg und arbeitete auf dem
// frisch gebauten Objekt weiter — sichtbar wurde es erst, als die
// Markenpruefung ein privates Feld auf dem falschen Objekt suchte.
```

## L310-314 · `fn finish_super(&mut self, env: &Rc<RefCell<Env>>, this_val: Value) -> C<Value> {`

```
/// **Jetzt erst die eigenen Instanzfelder.** Die Spec legt sie nach dem
/// Elternkonstruktor an, und der Unterschied ist sichtbar: ein
/// Initialisierer darf ein Feld der Elternklasse lesen. Welche Klasse
/// „eigene" ist, sagt das Heimatobjekt — sein `constructor` ist der
/// Konstruktor, in dem wir stehen.
```

## L329-330 · `pub fn super_get(&mut self, key: &str, env: &Rc<RefCell<Env>>) -> C<(Value, Value)> {`

```
/// `super.k` — der Wert kommt von OBEN, `this` bleibt unten. Eigene
/// Funktion, weil beide Maschinen sie rufen.
```

## L346-348 · `fn super_lookup(&mut self, key: &str, env: &Rc<RefCell<Env>>) -> C<(Value, Value)> {`

```
/// `super.k` aufloesen: der Wert kommt von OBEN, `this` bleibt unten.
/// Genau diese Trennung ist der Sinn von `super` — der gefundene Wert
/// wird gleich mit dem eigenen Empfaenger gerufen.
```

## L356-363 · `pub fn member_key2(&mut self, p: &MemberProp, env: &Rc<RefCell<Env>>) -> C<Rc<str>> {`

```
/// Der Schluessel eines Elementzugriffs.
///
/// **Es gab davon zwei**, eine hier und eine in `eval.rs` — Wort fuer Wort
/// dieselbe, bis ich das private Feld auf einen NUL-Schluessel umstellte
/// und nur eine davon anfasste. Danach schrieb `this.#p = v` unter `#p`
/// und `this.#p` las unter `\0#p`: das Feld war zugleich sichtbar und
/// leer. Eine Kopie ist kein Duplikat, sie ist eine zweite Semantik, die
/// auf ihren Tag wartet.
```

## L385-390 · `if matches!(callee, Expr::Super) {`

```
// `a.b()` bindet `this` an `a` — deshalb wird der Empfaenger hier
// getrennt geholt und nicht ueber `eval(callee)`, das ihn verlieren
// wuerde.
// `super(...)` ruft den Konstruktor der Elternklasse auf DIESEM `this`.
// Kein eigener Empfaenger, kein eigenes Objekt: das Objekt gibt es
// schon, `construct` hat es angelegt, bevor der Koerper lief.
```

## L417-420 · `Expr::Ident(n) => match self.with_target(n, env)? {`

```
// **`with (o) { m() }` ruft `m` MIT `o` als Empfaenger**
// (ES 9.1.1.2.4, WithBaseObject). Ohne das sieht eine Methode,
// die aus dem `with`-Objekt kommt, `undefined` als `this` — und
// genau so ruft Vues uebersetzter Code seine Hilfen.
```

## L429-430 · `if matches!(callee, Expr::Ident(n) if n == "eval") && self.is_eval_fn(&f) {`

```
// Der DIREKTE `eval`-Aufruf: am Namen UND an der Sache erkannt. Beide
// Maschinen tun hier dasselbe, siehe `Op::Call`.
```

## L437-438 · `return Err(self.not_a_function(match callee {`

```
// Den Namen nennen, nicht nur das Ereignis — dieselbe Hilfe wie
// die Befehlsmaschine, siehe `Interp::not_a_function`.
```

## L447-448 · `pub fn construct_named(&mut self, f: &Value, args: &[Value], name: Option<&str>) -> C<Value> {`

```
/// Bauen und den NAMEN des Gerufenen mitgeben — er steht nur in der
/// Fehlermeldung, und dort entscheidet er den Fall.
```

## L460-472 · `pub fn construct_target(&mut self, f: &Value, args: &[Value], nt: &Value) -> C<Value> {`

```
/// `new` mit einem eigenen NEUZIEL — der Fall von
/// `Reflect.construct(ziel, args, neuziel)`.
///
/// **Das dritte Argument ist keine Feinheit, es ist die Vererbung.**
/// Jede von Babel oder SWC uebersetzte `class X extends Y` ruft im
/// Konstruktor `Reflect.construct(Y, args, X)` — nur so bekommt das
/// frische Objekt `X.prototype` statt `Y.prototype`. Wurde das Neuziel
/// verworfen, landete die Instanz an der OBERklasse: eine React-
/// Komponente hatte `setState`, aber kein `render`, und die ganze Seite
/// rendert daraufhin nichts — ohne eine einzige Fehlerzeile, weil formal
/// nichts schiefging. Die Uebersetzer pruefen vorher mit
/// `Reflect.construct(Boolean, [], function(){})`, ob es den Weg gibt;
/// beak sagte ja und tat es dann nicht.
```

## L477-482 · `pub fn construct_on(&mut self, f: &Value, recv: Value, args: &[Value]) -> C<Value> {`

```
/// Wie `construct`, aber mit einem Empfaenger fuer den EINGEBAUTEN Fall.
///
/// Nur `super()` reicht einen: das frisch angelegte `this` traegt schon
/// den Prototyp der abgeleiteten Klasse, und daran haengt die einzige
/// Auskunft darueber, WAS gerade gebaut wird. Ein gewoehnliches `new`
/// gibt `undefined` weiter, wie bisher.
```

## L487-488 · `fn construct_full(&mut self, f: &Value, recv: Value, args: &[Value], nt: Option<&Value>)`

```
/// Der eine Weg, auf dem gebaut wird. `nt` ist das Neuziel, wenn es von
/// dem aufgerufenen Konstruktor abweicht (`Reflect.construct`).
```

## L492 · `if super::proxy::parts(fo).is_some() {`

```
// Die `construct`-Falle.
```

## L508 · `if let ObjKind::Native(n) = &fo.borrow().kind {`

```
// Ein nativer Konstruktor baut sein Objekt selbst; ein Pfeil ist keiner.
```

## L519-520 · `let proto_from = nt.unwrap_or(f).clone();`

```
// Der Prototyp kommt vom NEUZIEL, nicht vom gerufenen Konstruktor.
// Ohne Neuziel sind beide dasselbe.
```

## L528-529 · `Ok(match r { Value::Obj(_) => r, _ => Value::Obj(obj) })`

```
// Gibt der Konstruktor ein OBJEKT zurueck, gewinnt es; alles andere
// wird verworfen und das frische Objekt gewinnt.
```

## L533-535 · `pub fn define_accessor(&mut self, g: &Gc, key: Rc<str>, f: Value, is_get: bool) {`

```
/// Einen Leser oder Schreiber auf eine Eigenschaft legen — die beiden
/// muessen sich auf DERSELBEN treffen, sonst verdeckt der zweite den
/// ersten. Eigene Funktion, weil die Befehlsmaschine sie ruft.
```

## L545 · `pub fn spread_into(&mut self, g: &Gc, src: &Value) -> C<()> {`

```
/// `{...src}` — die aufzaehlbaren EIGENEN Eigenschaften kopieren.
```

## L548-549 · `for k in self.own_keys_of(o)? {`

```
// Schluessel UND Aufzaehlbarkeit durch den Stellvertreter —
// `{...proxy}` war sonst `{}`.
```

## L570-573 · `if !p.computed && !p.shorthand && &*key == "__proto__" {`

```
// `__proto__: v` — nur ALS SCHLUESSEL geschrieben, nicht
// berechnet und nicht abgekuerzt. `{[k]: v}` mit
// `k = "__proto__"` und `{__proto__}` sind gewoehnliche
// Eigenschaften, und die Spezifikation unterscheidet das.
```

## L591-592 · `let show = alloc::format!("{} {key}", if is_get { "get" } else { "set" });`

```
// Ein Leser heisst `"get x"`, kein blosses `"x"` — sonst
// waeren Leser und Schreiber ununterscheidbar.
```

## L603-604 · `let (parent_proto, parent_ctor) = match &c.super_class {`

```
// Zwei Ketten, nicht eine: die Instanzen haengen unter
// `Eltern.prototype`, der KONSTRUKTOR unter der Elternklasse selbst.
```

## L609-614 · `Value::Null => (None, None),`

```
// **`class X extends null` ist erlaubt** (ES 15.7.14
// Schritt 8.d.i): der Prototyp der Instanzen hat KEINEN
// Elter, der Konstruktor haengt an `%Function.prototype%`.
// Vorher lief das in `get(null, "prototype")` und meldete
// `cannot read 'prototype' of null` — eine Meldung, die
// nicht sagt, WO.
```

## L616-618 · `_ if !self.is_constructor(&sv) => {`

```
// Und was kein Konstruktor ist, sagt das mit dem NAMEN
// der Klasse. Ein Laufzeitfehler ohne Stelle kostet eine
// Stunde ([[feedback_a_runtime_error_without_a_position_costs_an_hour]]).
```

## L641-649 · `let cenv = match &c.name {`

```
// **Eine benannte Klasse steht in ihrem EIGENEN Rumpf** (ES 15.7.14):
// `class aa { … new aa(…) … }` sieht sich selbst, und zwar auch dann,
// wenn der aeussere Name nie vergeben wird oder spaeter umgehaengt
// wird. Derselbe Bereich wie bei einem benannten Funktionsausdruck
// (`func_value` daneben) — nur fehlte er hier, und der Intl-Polyfill
// von DuckDuckGo starb daran mit `aa is not defined`.
//
// Die Oberklasse wird noch im AEUSSEREN Bereich ausgewertet: dort ist
// die Bindung laut Spezifikation noch nicht angelegt.
```

## L657-658 · `let ctor_node = c.body.iter().find_map(|m| match m {`

```
// Der Konstruktor IST die Klasse. Fehlt er, wird ein leerer erzeugt —
// sonst haette die Klasse keinen aufrufbaren Koerper.
```

## L665-669 · `None if c.super_class.is_some() => Rc::new(Func {`

```
// Ein FEHLENDER Konstruktor ist nicht dasselbe wie ein leerer:
// in einer abgeleiteten Klasse reicht er seine Argumente an die
// Elternklasse durch (`constructor(...a) { super(...a) }`). Ohne
// das liefe `class B extends A {}` nie durch A's Konstruktor und
// eine Instanz haette keins ihrer Felder.
```

## L688-695 · `{`

```
// Hat die Klasse Instanzfelder, traegt ihr Konstruktor sie mit —
// `call_env` bzw. der `super()`-Weg legen sie dann an. Ohne Felder
// bleibt der Zeiger leer, damit ein gewoehnlicher Aufruf nichts
// nachzuschlagen hat.
// **Jeder** Klassenkonstruktor traegt seine Klasse, nicht nur die mit
// Feldern: der Zeiger sagt jetzt auch „das hier ist ein Konstruktor",
// und daran haengt, dass ein Rumpf ohne `return` sein `this` liefert.
// Ohne Felder ist `init_fields` ohnehin ein Leerlauf.
```

## L719-720 · `if let Some(n) = &c.name {`

```
// Jetzt, nicht spaeter: ein STATISCHES Feld wird noch in dieser
// Funktion ausgewertet und darf die Klasse schon sehen.
```

## L725-729 · `if let (Some(p), Value::Obj(co)) = (&parent_ctor, &ctor) {`

```
// **Statische Vererbung.** Ohne sie findet `B.create()` das
// `static create` der Elternklasse nicht — und `Object.getPrototypeOf(B)`
// ist `Function.prototype` statt `A`. Fiel auf, als `class` auf die
// Befehlsmaschine kam und die Probe gegen node lief; die Luecke war
// vorher in BEIDEN Maschinen, weil beide dieselbe Funktion rufen.
```

## L757-759 · `_ => { target.borrow_mut().set_prop(k, Prop::builtin(v)); }`

```
// Methoden einer Klasse sind NICHT aufzaehlbar — anders
// als die eines Objektliterals. Ein `for..in` ueber eine
// Instanz darf sie nicht sehen.
```

## L768-769 · `_ => {}`

```
// Felder je Instanz stehen im Konstruktor, nicht hier —
// siehe `Interp::init_fields`.
```

## L777-779 · `if op == UnaryOp::Typeof {`

```
// `typeof x` auf einen UNBEKANNTEN Namen wirft nicht — das ist der
// klassische Weg, ein globales Objekt zu pruefen, und der Vorspann
// benutzt ihn (`typeof JSON !== "undefined"`).
```

## L782-784 · `if self.with_target(n, env)?.is_none() && env_lookup(env, n).is_none() {`

```
// Ein `with`-Objekt traegt den Namen genauso wie eine
// Bindung — sonst meldet `typeof a` im `with` „undefined"
// fuer etwas, das direkt daneben gelesen werden kann.
```

## L799-800 · `if super::interp::env_strict(env) {`

```
// Strenger Code laesst ein gescheitertes `delete`
// nicht als `false` durchgehen (ES §13.5.1.2).
```

## L808-810 · `Expr::Ident(n) => match self.with_target(n, env)? {`

```
// `delete x` auf einen blossen Namen ist sonst `true` und
// tut nichts — INNERHALB eines `with` loescht es aber die
// Eigenschaft am Objekt (ES 13.5.1.2 Schritt 5).
```

## L825-843 · `pub fn func_value(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>) -> Value {`

```
/// Der WERTteil eines Praefixoperators — alles ausser `delete` und dem
/// `typeof` auf einem unbekannten Namen, die beide den Ausdruck selbst
/// brauchen und nicht nur sein Ergebnis.
///
/// Eigene Funktion, weil die Befehlsmaschine (`js::vm`) sie ruft. Zwei
/// Umsetzungen desselben Operators nebeneinander laufen auseinander, und
/// die schwaechere gewinnt dann immer.
/// Der Wert eines Funktions-AUSDRUCKS.
///
/// Eine benannte Funktions-EXPRESSION sieht ihren eigenen Namen in ihrem
/// Rumpf — `(function e(){ … e … })`. Das ist der Weg, auf dem
/// minifizierter Code rekursiert, und ohne ihn stirbt er an einem
/// einbuchstabigen `ReferenceError`. Eine DEKLARATION bekommt diese
/// Bindung NICHT: dort steht der Name schon aussen, und eine innere wuerde
/// eine Neuzuweisung verdecken.
///
/// Eigene Funktion, weil die Befehlsmaschine sie ruft. Sie dort
/// nachzubauen kostete beim ersten Versuch sechs Tests — genau die
/// Rekursion ueber den eigenen Namen.
```

## L858-859 · `pub fn delete_key(&mut self, base: &Value, key: &str) -> C<bool> {`

```
/// `delete obj[key]` — `false` nur, wenn die Eigenschaft da ist und sich
/// nicht entfernen laesst. Fehlt sie ganz, ist die Antwort `true`.
```

## L887-889 · `pub fn delete_or_throw(&mut self, base: &Value, key: &str) -> C<()> {`

```
/// `DeletePropertyOrThrow` (ES §7.3.9) — was die Eingebauten benutzen.
/// Der `delete`-OPERATOR gibt `false` zurueck (ausser im strengen Modus);
/// eine eingebaute Funktion darf das nie ignorieren.
```

## L902-903 · `UnaryOp::Plus => Value::Num(self.to_number(&v)?),`

```
// `+x` ist die EINZIGE Stelle, an der eine grosse Zahl wirft statt
// sich umzuwandeln — es gaebe sonst einen stillen Weg nach `f64`.
```

## L917-918 · `pub fn typeof_ident(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Value> {`

```
/// `typeof <name>` — wirft NICHT, wenn es den Namen nicht gibt. Der
/// klassische Weg, ein globales Objekt zu pruefen.
```

## L920 · `if self.with_target(n, env)?.is_some() {`

```
// Ein `with`-Objekt kann den Namen tragen, und dann ist er DA.
```

## L935-936 · `pub fn vm_load(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Value> {`

```
/// Einen Namen lesen (`x`) bzw. zuweisen (`x = v`) — dieselben Funktionen,
/// die der Baumlaeufer benutzt, nur oeffentlich fuer die Maschine.
```

## L941-947 · `pub fn vm_load_at(&mut self, n: &str, env: &Rc<RefCell<Env>>,`

```
/// `LoadVar` mit dem Weg von letztem Mal — siehe `Chunk::hints`.
///
/// Trifft der Hinweis, kostet ein Zugriff EINEN Tabellenblick statt vier.
/// Trifft er nicht (oder ist keiner da), laeuft der volle Weg und der
/// Hinweis lernt dabei. Falsch werden kann er nur, wenn nachtraeglich
/// eine Bindung WEITER INNEN entsteht — und das kann allein ein direktes
/// `eval`, das die Wegweiser deshalb abschaltet.
```

## L952-960 · `let mut cur: *const RefCell<Env> = Rc::as_ptr(env);`

```
// **Der Weg wird gelesen, nicht geliehen.** Ein `Rc::clone` je
// Sprung waeren drei Zaehlerpaare fuer einen Zeiger, den niemand
// behaelt — im Profil sind das die 2,3 % in `Cell<isize>::get`.
//
// SAFETY: Die Kette haengt an `env`, und `env` lebt fuer die
// Dauer dieses Aufrufs; jede Umgebung haelt ihren Elter als
// `Rc`, also lebt die ganze Kette mit. Gelesen wird nur, und
// nichts davon verlaesst diese Schleife: zwischen Betreten und
// Verlassen laeuft kein fremder Code, der sie umhaengen koennte.
```

## L967-969 · `if reached {`

```
// NUR ein Treffer zaehlt. „Steht hier, ist aber tot" und
// „kommt aus einem Modul" gehen den vollen Weg, damit die
// Fehlermeldung und die Import-Kette dieselben bleiben.
```

## L971 · `let b = unsafe { (*cur).borrow() };`

```
// SAFETY: `cur` ist der Zeiger aus derselben Kette, siehe oben.
```

## L986-991 · `pub fn vm_store(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {`

```
/// **Direkt, nicht ueber einen gebauten AST-Knoten.** Bis 0.117.0 stand
/// hier `self.store(&Expr::Ident(String::from(n)), v, env)` — das
/// allozierte je ZUWEISUNG eine Zeichenkette auf dem Haufen und baute
/// einen Ausdrucksknoten, den `store` in der naechsten Zeile wieder
/// auseinandernahm. `StoreVar` sind 6 % aller Befehle: in einem
/// Anmeldelauf der Fritzbox 140 Millionen Allokationen fuer nichts.
```

## L996-1001 · `pub fn vm_store_at(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>,`

```
/// `StoreVar` mit dem Weg von letztem Mal — dieselbe Ueberlegung wie in
/// `vm_load_at`, und derselbe Vorbehalt: ein direktes `eval` schaltet ihn ab.
///
/// `StoreVar` sind 6,2 % aller Befehle, und der volle Weg fragt bis zu
/// VIER Tabellen: `env_lookup` je Ebene, dann die Import-Tabelle, dann
/// `get`, dann `get_mut`.
```

## L1006-1007 · `let mut cur: *const RefCell<Env> = Rc::as_ptr(env);`

```
// SAFETY: wie in `vm_load_at` — die Kette haengt an `env`, sie
// wird nur gelesen, und zwischendrin laeuft kein fremder Code.
```

## L1015-1018 · `let mut b = unsafe { (*cur).borrow_mut() };`

```
// SAFETY: `cur` ist der Zeiger aus derselben Kette, siehe oben.
// `borrow_mut` ist hier zulaessig, weil auf diesem Weg keine
// andere Leihe offen ist: die Schleife hat ihre wieder
// fallengelassen, und der Rufer haelt nur den `Rc`.
```

## L1020-1023 · `if b.imports.is_none() {`

```
// **Nur wo es GAR KEINE Import-Tabelle gibt.** Sonst muesste
// hier entschieden werden, ob der Name importiert ist — und
// auf einen Import zu schreiben ist ein Fehler, kein
// Schreiben. Das ist der volle Weg.
```

## L1052-1053 · `Expr::Ident(n) => self.assign_ident(n, v, env),`

```
// EINE Fassung, in `eval.rs`. Zwei nebeneinander sind zwei
// Semantiken, die auf ihren ersten Unterschied warten.
```

## L1075-1076 · `if matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish) {`

```
// Die kurzschliessenden Formen werten die Rechte NUR aus, wenn sie
// gebraucht wird — `a ||= b` darf `b` nicht anfassen, wenn `a` wahr ist.
```

## L1105-1106 · `fn num_done(&mut self, op: BinOp, a: f64, b: f64) -> C<Value> {`

```
/// Das Ende jedes Zahlenarmes — hier steht der Operator fest und beide
/// Seiten sind umgewandelt.
```

## L1110 · `None => self.type_err("not a numeric operator"),`

```
// `in`/`instanceof` haben eigene Arme und kommen hier nicht an.
```

## L1117-1127 · `if let (Value::Num(a), Value::Num(b)) = (&l, &r) {`

```
// **Schnellweg: beide Seiten sind schon Zahlen.** Dann ist jede
// Umwandlung die Identitaet — `ToPrimitive` gibt die Zahl zurueck,
// `ToNumeric` auch, und keine davon kann etwas beobachten: kein
// `valueOf`, kein `Symbol.toPrimitive`, kein Wurf, keine Reihenfolge.
// Der lange Weg rechnet DASSELBE, nur durch vier Ergebnisrahmen
// hindurch — und `C<Value>` ist 40 Byte, die je Rahmen ueber den
// Stapel wandern.
//
// Gemessen an der Fritzbox-Anmeldung (SHA-256 von Hand in JS):
// `to_numeric` + `to_number` + `to_primitive_hint` sind 10,2 % der
// Laufzeit und die `?`-Weiterreichung noch einmal 12,1 %.
```

## L1133-1136 · `let lp = self.to_primitive_hint(&l, "default")?;`

```
// `+` ist der einzige Operator, der auch Text meint. Beide
// Seiten werden ZUERST primitiv gemacht, DANN entschieden —
// die Reihenfolge ist sichtbar, wenn `valueOf` Nebenwirkungen
// hat.
```

## L1155-1161 · `let lp = self.to_numeric(&l)?;`

```
// Beide Seiten ZUERST primitiv machen, dann entscheiden: zwei
// grosse Zahlen rechnen gross, zwei kleine klein, gemischt
// wirft. Die Reihenfolge ist sichtbar, wenn `valueOf`
// Nebenwirkungen hat.
// `ToNumeric(lhs)` GANZ, dann erst `ToNumeric(rhs)`. Die
// Reihenfolge ist beobachtbar: gibt `lhs.valueOf` ein Symbol
// zurueck, darf `rhs.valueOf` gar nicht mehr laufen.
```

## L1183-1184 · `match self.big_cmp(&lp, &rp)? {`

```
// Gross gegen klein DARF verglichen werden — nur gerechnet
// werden darf damit nicht.
```

## L1199-1200 · `if matches!(op, UShr) {`

```
// `>>>` gibt es fuer grosse Zahlen NICHT: es setzt eine
// feste Breite voraus, und die hat der Typ nicht.
```

## L1239-1241 · `let hi = self.get(&r, SYM_HAS_INSTANCE)?;`

```
// `Symbol.hasInstance` ueberstimmt die Prototypkette — das ist
// der Weg, auf dem eine Klasse selbst entscheidet, was sie als
// ihre Instanz gelten laesst.
```

## L1262 · `fn loose_eq(&mut self, l: &Value, r: &Value) -> C<bool> {`

```
/// `==`. Die Regel, die niemand mag, aber echter Code benutzt sie.
```

## L1270-1271 · `(BigInt(a), Num(n)) | (Num(n), BigInt(a)) =>`

```
// `1n == 1` ist WAHR, `1n === 1` falsch. Der Vergleich laeuft
// ueber den mathematischen Wert, nicht ueber eine Umwandlung.
```

## L1278-1279 · `(Sym(_), Num(_) | Str(_) | Bool(_)) | (Num(_) | Str(_) | Bool(_), Sym(_)) => false,`

```
// Ein Symbol ist nur sich selbst gleich — `==` wandelt es NICHT
// um. Gegen ein Objekt entscheidet erst dessen `ToPrimitive`.
```

## L1292 · `fn big_arith(&mut self, op: BinOp, a: &Rc<crate::js::bigint::Big>,`

```
/// Die vier Rechenoperatoren auf zwei grossen Zahlen.
```

## L1314-1315 · `fn big_cmp(&mut self, l: &Value, r: &Value) -> C<Option<core::cmp::Ordering>> {`

```
/// Ein Vergleich, bei dem mindestens eine Seite gross ist. `None` heisst
/// „unvergleichbar" (NaN auf der anderen Seite).
```

## L1325 · `if let (Some(a), Some(b)) = (to_big(l), to_big(r)) { return Ok(Some(a.cmp(&b))); }`

```
// Zwei grosse, oder eine grosse gegen einen Text: exakt vergleichen.
```

## L1327-1329 · `let (a, b) = match (l, r) {`

```
// Gross gegen Zahl: ueber `f64`. Das ist auf sehr grossen Werten
// ungenau — benannt statt verschwiegen, und der Fall kommt in echtem
// Code nicht vor.
```

## L1342-1352 · `fn num_bin(op: BinOp, a: f64, b: f64) -> Option<Value> {`

```
/// Der Zahlenkern der zweistelligen Operatoren: beide Seiten sind schon
/// gewoehnliche Zahlen, es ist nichts mehr umzuwandeln.
///
/// **Er steht hier EINMAL und wird von beiden Seiten gerufen** — vom
/// Schnellweg oben in `binary` und vom Ende jedes langsamen Armes, wenn der
/// seine Umwandlungen hinter sich hat. Abgeschrieben liefe die Bedeutung
/// zwischen den zwei Stellen still auseinander: `%` hat vier Sonderfaelle,
/// `**` drei, und ein Vergleich mit NaN ist immer falsch, auch `>=`.
///
/// `None` gibt es nur fuer `in` und `instanceof` — die haben mit zwei Zahlen
/// nichts zu tun und eigene Arme.
```

## L1373 · `EqEq | EqEqEq => Value::Bool(a == b),`

```
// Zwei Zahlen: `==` ist `===`, und beide sind der Vergleich selbst.
```

## L1380-1381 · `fn powf(a: f64, b: f64) -> f64 {`

```
/// `**`. `libm` ist schon Abhaengigkeit der Engine (CSS Color 4), also wird
/// hier nichts Neues hereingezogen.
```

