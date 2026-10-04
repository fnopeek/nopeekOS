# `tools/wasm/beak-engine/src/js/generator.rs` @ 5e0102684

## L1-12 · `use alloc::collections::VecDeque;`

```
//! Generatoren — das Anhalten, wegen dem die Befehlsmaschine ueberhaupt
//! gebaut wurde.
//!
//! **Die Antwort auf die Entwurfsfrage von Stufe 4** steht im Kopf von
//! `vm.rs` und heisst: ein Generator ist eine EIGENE Maschine, kein Rahmen in
//! fremder. Deshalb steht hier so wenig — der Zustand ist eine `Vm`, und die
//! drei Methoden sind drei Arten, sie wieder anzuwerfen.
//!
//! **Keine zweite Semantik.** Gebaut wird ein Generatorobjekt an genau EINER
//! Stelle (`make`, gerufen aus `Interp::call_inner`), und zwar auch dann,
//! wenn der Aufruf aus der Befehlsmaschine kam: die schickt einen Generator
//! bewusst ueber `Interp::call`, statt ihm einen Rahmen zu geben.
```

## L22 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// Der Lebenslauf eines Generators.
```

## L25-26 · `Start,`

```
/// Gebaut, aber noch keinen Befehl gefahren. Ein `next(v)` wirft `v` weg —
/// es gibt noch kein `yield`, das ihn entgegennehmen koennte.
```

## L28 · `Suspended,`

```
/// Steht auf einem `yield`.
```

## L30 · `Running,`

```
/// Laeuft gerade. Ein `next()` von innen ist ein Fehler, kein Neustart.
```

## L35-40 · `pub struct GenState {`

```
/// Was ein Generatorobjekt festhaelt.
///
/// `Cell`/`RefCell` und die Maschine als `Option`, damit waehrend des Laufens
/// KEINE Ausleihe offen steht: ein Generator, der sich selbst `next()` ruft,
/// kaeme sonst an einem `borrow_mut` vorbei und liesse den Kernel anhalten.
/// Herausnehmen, fahren, zurueckstellen.
```

## L44-52 · `promise: Option<Gc>,`

```
/// Das Versprechen, das eine ASYNC-Funktion am Ende erledigt. `None` bei
/// einem gewoehnlichen Generator UND bei einem async-Generator — der hat
/// nicht EIN Versprechen, sondern eins je Anfrage (`queue`).
///
/// Dass alle drei sich denselben Zustand teilen, ist kein Sparen: eine
/// wartende async-Funktion IST ein angehaltener Rumpf, und der Bauplan
/// sagte es so — „derselbe Mechanismus mit einem Promise davor". Der
/// Unterschied ist allein, WER sie wieder anwirft: ein `next()` oder die
/// Microtask-Schlange.
```

## L54-63 · `queue: RefCell<VecDeque<Req>>,`

```
/// Die offenen Anfragen eines ASYNC-Generators, in der Reihenfolge, in der
/// sie kamen.
///
/// **Ein async-Generator braucht sie, ein gewoehnlicher nicht** — und das
/// ist der ganze Unterschied zwischen beiden. `agen.next()` gibt SOFORT
/// ein Versprechen zurueck, auch wenn der Rumpf noch an einem `await`
/// haengt; drei `next()` hintereinander duerfen die Maschine nicht
/// dreimal anwerfen, sondern muessen sich anstellen (ES 27.6.3.6). Ohne
/// die Schlange waere der zweite Aufruf entweder ein Fehler („already
/// running") oder ein zweiter Stapel auf demselben Rumpf.
```

## L67 · `struct Req {`

```
/// Eine offene Anfrage an einen async-Generator.
```

## L71-72 · `promise: Gc,`

```
/// Das Versprechen, das `next()`/`throw()`/`return()` schon
/// zurueckgegeben hat und das hier erledigt wird.
```

## L80 · `pub fn roots(&self, objs: &mut alloc::vec::Vec<Gc>,`

```
/// Was die angehaltene Maschine festhaelt — siehe `Vm::roots`.
```

## L86-88 · `for r in self.queue.borrow().iter() {`

```
// Die Schlange haelt je Anfrage ein Versprechen und einen Wert fest.
// Sie hier auszulassen liesse einen Rc-Ring stehen, sobald ein
// Rueckruf auf das Versprechen den Generator selbst festhaelt.
```

## L96-104 · `pub fn make(i: &mut Interp, func: &Gc, d: &Rc<super::value::FuncData>,`

```
/// Ein Aufruf einer Generatorfunktion: er baut das Objekt und faehrt NICHTS.
///
/// `None` heisst, dass der Uebersetzer den Rumpf nicht kann — dann bleibt es
/// beim alten Weg (der Baumlaeufer laeuft in sein „generators are not
/// supported"), statt hier einen halben Generator zu bauen.
///
/// Die Reihenfolge ist die der Spec: erst die Umgebung mit Parametern,
/// `this` und `arguments` (`call_env`), dann das Hochziehen, dann
/// `Get(f, "prototype")` fuer den Prototyp des Objekts.
```

## L123-129 · `pub fn make_async(i: &mut Interp, d: &Rc<super::value::FuncData>,`

```
// ── async/await ──────────────────────────────────────────────────────────
//
// Derselbe angehaltene Rumpf, nur wirft ihn die Microtask-Schlange wieder an
// statt eines `next()`. Der Zustand haengt an einem Objekt, das kein Skript je
// sieht — es ist bloss der Traeger, ueber den `promise::bind1` den beiden
// Behandlern ihren Generator mitgibt (ein `NativeFn` ist ein Zeiger und
// bekommt keinen Abschluss).
```

## L131-136 · `pub fn make_async(i: &mut Interp, d: &Rc<super::value::FuncData>,`

```
/// Ein Aufruf einer async-Funktion: er gibt ein VERSPRECHEN zurueck und
/// laeuft den Rumpf bis zum ersten `await` sofort — synchron, wie die Spec es
/// verlangt.
///
/// Auch ein Fehler beim Binden der Parameter wird zur ABLEHNUNG, nicht zu
/// einem Wurf: eine async-Funktion wirft nie, sie lehnt ab.
```

## L140-145 · `let outer = super::promise::new_promise(i);`

```
// **Auch ein unuebersetzbarer Rumpf gibt ein Versprechen zurueck.**
// Der Baumlaeufer faehrt ihn (und laeuft an einem `await` in seinen
// TypeError), aber der Aufrufvertrag bleibt derselbe: eine
// async-Funktion wirft nie und gibt nie einen nackten Wert. Zwei
// Aufrufvertraege fuer dasselbe Schluesselwort waeren genau die
// zweite Semantik, die dieser Umbau vermeidet.
```

## L176 · `enum Seed {`

```
/// Womit die Maschine wieder anlaeuft.
```

## L178 · `Start,`

```
/// Zum ersten Mal — es gibt noch kein `await`, das einen Wert naehme.
```

## L182-183 · `Delegate(Value),`

```
/// Ein `return(v)` an einem `yield*`: es geht als WERT an den inneren
/// Iterator, statt den aeusseren Generator aufzugeben.
```

## L187-188 · `fn pump(i: &mut Interp, st: &Rc<GenState>, seed: Seed, holder: Value) {`

```
/// Die Maschine einer async-Funktion fahren, bis sie wartet oder fertig ist —
/// und danach ihr Versprechen erledigen.
```

## L196-197 · `Seed::Throw(e) => {`

```
// Faengt den Wurf im Rumpf niemand, ist die Funktion damit fertig —
// und ihr Versprechen abgelehnt.
```

## L220-221 · `Ok(Step::Yield(..)) => {`

```
// Kann nicht vorkommen: ein async-Generator ist beim Uebersetzen
// abgelehnt, ein `yield` steht also in keinem async-Rumpf.
```

## L238-239 · `fn wake(i: &mut Interp, a: &[Value], rejected: bool) {`

```
/// Der Behandler, den `await` am Versprechen anhaengt: `a[0]` ist der
/// gebundene Traeger, `a[1]` die Aufloesung.
```

## L253-267 · `pub fn make_async_gen(i: &mut Interp, func: &Gc, d: &Rc<super::value::FuncData>,`

```
// ── async-Generatoren ────────────────────────────────────────────────────
//
// **Sie sind nicht die Summe der beiden, sondern ihre Verschraenkung.** Ein
// async-Generator haelt an ZWEI Gruenden an: an `await` (die Microtask-
// Schlange wirft ihn wieder an) und an `yield` (ein `next()` tut es). Beide
// Gruende kommen aus derselben `Vm`, und `Step` kennt sie laengst — was
// fehlte, war der Vertrag darum herum:
//
//   * `next()` gibt SOFORT ein Versprechen zurueck, auch mitten im `await`.
//     Also eine Schlange, keine Antwort (`Req`).
//   * `yield x` ist `AsyncGeneratorYield(? Await(x))` (ES 15.5.5 / 27.6.3.8) —
//     der Wert wird ERST abgewartet. Das steht nicht hier, sondern im
//     Uebersetzer: er legt in einem async-Generator vor jedes `Op::Yield` ein
//     `Op::Await`. So bleibt die Maschine dumm und es gibt keine zweite
//     Semantik fuer `yield`.
```

## L269-270 · `pub fn make_async_gen(i: &mut Interp, func: &Gc, d: &Rc<super::value::FuncData>,`

```
/// Ein Aufruf einer async-Generatorfunktion: er baut das Objekt und faehrt
/// NICHTS — wie beim gewoehnlichen Generator, nur mit leerer Anfrage-Schlange.
```

## L289-295 · `fn enqueue(i: &mut Interp, t: &Value, kind: ReqKind, v: Value) -> Value {`

```
/// Eine Anfrage anstellen und das Versprechen zurueckgeben, das sie erledigen
/// wird (ES 27.6.3.6 AsyncGeneratorEnqueue).
///
/// **Der Rueckgabewert ist IMMER ein Versprechen, auch im Fehlerfall** — ein
/// `next()` auf etwas, das kein async-Generator ist, lehnt ab und wirft
/// nicht. Ein Wurf hier wuerde das rufende Skript beenden, statt eine
/// Ablehnung zuzustellen, die es behandeln kann.
```

## L314-315 · `if st.status.get() != Status::Running {`

```
// Laeuft die Maschine gerade (oder wartet sie an einem `await`), nimmt sie
// die Anfrage von selbst auf, sobald sie dort fertig ist.
```

## L323-327 · `fn serve(i: &mut Interp, st: &Rc<GenState>, holder: Value, mut seed: Option<Seed>) {`

```
/// Die vorderste Anfrage erledigen und die naechste nehmen, bis die Schlange
/// leer ist oder die Maschine wartet (ES 27.6.3.5 AsyncGeneratorResumeNext).
///
/// `seed` ist gesetzt, wenn wir aus einem `await` zurueckkommen — dann laeuft
/// die Maschine weiter, statt eine neue Anfrage zu beginnen.
```

## L330-331 · `if st.status.get() == Status::Done {`

```
// Ein fertiger Generator beantwortet alles aus der Schlange, ohne die
// Maschine anzufassen.
```

## L347 · `let cur = match seed.take() {`

```
// Kein `seed` heisst: eine NEUE Anfrage beginnt.
```

## L358-360 · `if st.status.get() == Status::Start { Seed::Start } else { Seed::Value(value) }`

```
// Beim allerersten `next` gibt es kein `yield`, das
// den Wert naehme — dieselbe Regel wie im
// gewoehnlichen Generator.
```

## L364-365 · `ReqKind::Return if st.vm.borrow().as_ref()`

```
// An einem `yield*` geht auch hier beides an den INNEREN
// Iterator, statt den aeusseren abzuwickeln bzw. aufzugeben.
```

## L368-373 · `ReqKind::Return => {`

```
// **`return` fuehrt den Rumpf nicht zu Ende.** Ein
// `finally` mit `yield` darin ist beim Uebersetzen
// abgelehnt, es kann also keinen geben, der noch laufen
// muesste; offene `for…of`-Iterationen schliesst
// `Vm::close`. Benannt fehlt: die Spezifikation WARTET den
// Wert vorher ab (AsyncGeneratorAwaitReturn), wir nicht.
```

## L398-400 · `Ok(Step::Await(v)) => {`

```
// **Ein `await` beendet die Runde, aber nicht die Anfrage.** Die
// vorderste bleibt stehen; die Maschine bleibt „Running", damit
// ein `next()` daneben sich anstellt statt sie anzuwerfen.
```

## L414-415 · `let out = i.iter_result(v, false);`

```
// Ein async-Generator gibt immer den WERT heraus, auch beim
// `yield*`: `AsyncGeneratorYield(? IteratorValue(…))`.
```

## L444 · `fn wake_gen(i: &mut Interp, a: &[Value], rejected: bool) {`

```
/// Der Behandler, den ein `await` IM async-Generator anhaengt.
```

## L459-463 · `pub fn install_async(f_proto: &Gc) -> (Gc, Gc, Gc) {`

```
/// `%AsyncIteratorPrototype%`, `%AsyncGeneratorPrototype%` und
/// `%AsyncGeneratorFunction.prototype%`.
///
/// Der erste traegt nur `[Symbol.asyncIterator]() { return this }` — daran
/// haengt, dass `for await (x of agen())` ueberhaupt einen Iterator findet.
```

## L493-494 · `fn state(i: &mut Interp, t: &Value) -> C<Rc<GenState>> {`

```
/// Der Zustand hinter `this` — oder ein TypeError, wenn `this` keiner ist.
/// Die Ausleihe endet HIER, vor allem, was danach laeuft.
```

## L504-505 · `fn finish(i: &mut Interp, st: &Rc<GenState>, mut vm: Vm, r: C<Step>) -> C<Value> {`

```
/// Was `drive` ergeben hat, in ein `{value, done}` umsetzen — und den Zustand
/// dabei richtig stellen. Ein Wurf beendet den Generator endgueltig.
```

## L511-514 · `Ok(if raw { v } else { i.iter_result(v, false) })`

```
// **ROH heisst: schon ein Ergebnisobjekt.** `yield*` reicht das
// des INNEREN Iterators unveraendert durch (ES 15.5.5,
// `GeneratorYield`) — es noch einmal einzupacken gaebe
// `{value: {value: 1, done: false}, done: false}`.
```

## L521-523 · `Ok(Step::Await(_)) => {`

```
// Kann nicht vorkommen: ein `await` steht nur im Rumpf einer
// async-Funktion, und ein async-Generator ist beim Uebersetzen
// abgelehnt. Steht hier, weil ein `_ =>` den Fall verschweigen wuerde.
```

## L537-541 · `fn take(i: &mut Interp, st: &Rc<GenState>) -> C<Option<Vm>> {`

```
/// Die Maschine herausnehmen, wenn der Zustand es erlaubt.
///
/// `Ok(None)` heisst „fertig, nichts mehr zu tun"; ein laufender Generator
/// gibt einen TypeError, weil ihn wieder anzuwerfen seinen Stapel
/// verdoppeln wuerde.
```

## L558-559 · `if started { vm.send(v); }`

```
// Beim ALLERERSTEN `next` gibt es kein `yield`, das den Wert nehmen
// koennte — er faellt weg. Genau das sagt die Spec.
```

## L566-568 · `pub fn throw(i: &mut Interp, t: &Value, v: Value) -> C<Value> {`

```
/// `gen.throw(v)`: den Wurf an der Anhaltestelle einwerfen. Faengt ihn dort
/// niemand, ist der Generator fertig und der Wurf gehoert dem Rufer — auch
/// dann, wenn noch gar nichts gelaufen war.
```

## L573-575 · `if vm.at_delegate() {`

```
// **An einem `yield*` wickelt ein Wurf NICHT ab.** Dort ist er ein Wert,
// der an den inneren Iterator weitergereicht wird (ES 15.5.5, 6.b) — hat
// der kein `throw`, entscheidet das die Maschine, nicht diese Stelle.
```

## L590-592 · `pub fn ret(i: &mut Interp, t: &Value, v: Value) -> C<Value> {`

```
/// `gen.return(v)`: aufgeben. Offene `for…of`-Iterationen werden geschlossen
/// (`Vm::close`); ein anhaengiger `finally` kann es nicht geben, weil ein
/// `yield` darunter schon beim Uebersetzen abgelehnt wird.
```

## L596-599 · `if vm.at_delegate() {`

```
// **An einem `yield*` bekommt der INNERE Iterator sein `return` zuerst.**
// Er darf seinen eigenen Aufraeumer fahren, und er darf die Aufgabe sogar
// abfangen (indem sein `return()` `done: false` gibt) — dann laeuft der
// aeussere Generator weiter, und `finish` sieht ein gewoehnliches `yield`.
```

## L611-616 · `pub fn install(i_proto: &Gc, f_proto: &Gc) -> (Gc, Gc) {`

```
/// Die zwei Prototypen des Generatorvertrags.
///
/// `%GeneratorPrototype%` haengt unter `%IteratorPrototype%` — daher kommt
/// `[Symbol.iterator]() { return this }`, und genau daran haengt, dass
/// `for (x of gen())` und `[...gen()]` gehen, ohne dass hier etwas dafuer
/// steht.
```

