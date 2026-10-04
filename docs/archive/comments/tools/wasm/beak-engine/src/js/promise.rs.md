# `tools/wasm/beak-engine/src/js/promise.rs` @ 5e0102684

## L1-14 · `use alloc::rc::Rc;`

```
//! `Promise` und die Microtask-Schlange.
//!
//! **Ohne Aufhaengen der Maschine.** Ein Versprechen braucht keine
//! Unterbrechung des Auswerters — nur eine Schlange, die zwischen den Aufgaben
//! abgearbeitet wird. Das ist der Grund, warum es VOR Generatoren und
//! `await` kommt: die brauchen einen anhaltbaren Auswerter, ein
//! `.then()`-Gespann nicht.
//!
//! **Kein Abschluss, sondern gebundene Argumente.** `NativeFn` ist ein
//! Funktionszeiger und bekommt das Funktionsobjekt nicht zu sehen — ein
//! `resolve`, das „sein" Versprechen kennt, ginge damit nicht. Also traegt es
//! den Zustand als GEBUNDENES erstes Argument (`ObjKind::Bound`), und das ist
//! zugleich der Ort fuer die Sperre „schon erledigt": `resolve` und `reject`
//! teilen sich einen Kasten, und wer zuerst kommt, schliesst ihn.
```

## L24 · `const CAP_PROMISE: &str = "\0!cap.p";`

```
/// Der Kasten, den `resolve` und `reject` sich teilen.
```

## L27 · `const AGG_LEFT: &str = "\0!agg.left";`

```
/// Zaehlerkasten fuer `all`/`allSettled`/`any`.
```

## L36-43 · `pub struct Reaction {`

```
/// Ein angehaengter Behandler: die Funktion (oder keine — dann wird
/// durchgereicht) und das abgeleitete Versprechen, das er erledigt.
///
/// `cap` ist der FREMDE Fall: `then` darf sein Ergebnis ueber
/// `constructor[Symbol.species]` bauen lassen, und dann ist das Ergebnis kein
/// `ObjKind::Promise`, sondern irgendein Objekt mit einem eigenen
/// `resolve`/`reject`-Paar. `derived` bleibt trotzdem gesetzt (als
/// Platzhalter), damit der Rest des Codes unveraendert bleibt.
```

## L54-56 · `pub handled: bool,`

```
/// Hat jemals jemand `then` daran gehaengt? `PerformPromiseThen` setzt es,
/// egal ob ein Ablehnungsbehandler dabei war — das abgeleitete
/// Versprechen uebernimmt die Verantwortung.
```

## L60 · `pub enum Job {`

```
/// Eine Microtask.
```

## L62 · `React { r: Reaction, arg: Value, rejected: bool },`

```
/// Einen Behandler auf einen erledigten Zustand loslassen.
```

## L64 · `Adopt { thenable: Value, then: Value, target: Gc },`

```
/// Ein fremdes Thenable uebernehmen: `then.call(thenable, res, rej)`.
```

## L77 · `pub fn settle(i: &mut Interp, p: &Gc, v: Value, rejected: bool) {`

```
/// Erledigen. Nur EINMAL — wer schon erledigt ist, aendert sich nicht mehr.
```

## L90-93 · `if rejected && !handled { i.pending_rejections.push(p.clone()); }`

```
// Eine Ablehnung, an der nichts haengt, ist die stillste Art, eine Seite
// scheitern zu lassen: kein Fehler, keine Meldung, nur etwas, das nie
// passiert. Sie wird gemerkt und am Ende der Schlange gemeldet — bis
// dahin darf noch jemand ein `catch` anhaengen, und das ist der Normalfall.
```

## L97 · `pub fn resolve_promise(i: &mut Interp, p: &Gc, v: Value) {`

```
/// `ResolvePromise`: ein Thenable wird UEBERNOMMEN, alles andere erfuellt.
```

## L105-106 · `let then = match i.get(&v, "then") {`

```
// `then` LESEN kann werfen (ein Getter) — dann ist das der Grund
// fuer die Ablehnung, nicht ein spaeterer Aufruf.
```

## L120-121 · `pub fn resolving_functions(i: &mut Interp, p: &Gc) -> (Value, Value) {`

```
/// Das Paar `(resolve, reject)` fuer ein Versprechen. Beide teilen sich einen
/// Kasten; der erste Aufruf schliesst ihn fuer den anderen mit.
```

## L146 · `pub fn perform_then(i: &mut Interp, p: &Gc, on_ok: Value, on_err: Value) -> Gc {`

```
/// `PerformPromiseThen` — haengt an und gibt das abgeleitete Versprechen.
```

## L151 · `pub fn perform_then_cap(i: &mut Interp, p: &Gc, on_ok: Value, on_err: Value,`

```
/// Wie `perform_then`, aber mit einem FREMDEN Erledigungspaar.
```

## L181-186 · `pub fn run_jobs(i: &mut Interp) -> usize {`

```
/// Die Schlange leeren.
///
/// Mit Deckel: eine Kette, die sich selbst nachlegt (`p.then(f)` in `f`), ist
/// ein voellig gewoehnliches Muster und wuerde sonst nie enden. Der Deckel ist
/// dieselbe Entscheidung wie bei `run_timers` — EINMAL durchlaufen ist zu
/// wenig (eine Kette braucht ihre Sprossen), unbegrenzt zu viel.
```

## L189-199 · `loop {`

```
// **Der Kontrollpunkt der Beobachter.** Er gehoert hierher und nicht in
// `run_timers`: `run_jobs` laeuft nach JEDEM Einstiegspunkt — nach einem
// Skript, nach einem Ereignis, nach jedem Zeitgeber. Ein Beobachter, der
// erst beim naechsten Zeitgeber benachrichtigt wuerde, bekaeme auf einer
// Seite ohne Zeitgeber nie etwas zu sehen.
//
// In einer Schleife, weil ein Rueckruf den Baum aendern darf: das ist
// das uebliche Muster (eine Bibliothek erweckt frisch eingehaengtes
// Markup und haengt dabei selbst etwas ein). Begrenzt wird sie nicht
// hier, sondern von Schritt- und Zeitdeckel — die gelten auch fuer eine
// Endlosschleife, die aus lauter gewoehnlichen Runden besteht.
```

## L201-205 · `let zugestellt = super::dombind::deliver_mutations(i)`

```
// **Zuerst zustellen, dann die Schlange fahren.** Die Meldung wird
// angemeldet, wenn die Aenderung passiert — also waehrend das Skript
// lief und damit VOR jedem `.then`, das danach kam. Andersherum
// gerufen liefe der Beobachter als Letzter, und eine Seite, die im
// `.then` das Ergebnis erwartet, saehe nichts.
```

## L217-218 · `fn report_rejections(i: &mut Interp) {`

```
/// Was am Ende der Schlange noch unbehandelt abgelehnt ist, wird GEMELDET —
/// als `unhandledrejection` am Fenster und auf der Konsole.
```

## L227-228 · `let prevented = super::dombind::dispatch_rejection(i, reason.clone(), Value::Obj(p.clone()))`

```
// Der Behandler darf noch antworten — `preventDefault` unterdrueckt
// die Meldung, genau wie im Browser.
```

## L250-251 · `let finish = |i: &mut Interp, r: &Reaction, v: Value, rejected: bool| {`

```
// Ein FREMDES Erledigungspaar bekommt den Ausgang als Aufruf,
// nicht als Zustandswechsel — es gehoert nicht uns.
```

## L284-291 · `pub fn bind1(i: &mut Interp, f: NativeFn, arg: Value) -> Value {`

```
/// Eine native Funktion mit EINEM festgebundenen ersten Argument.
///
/// Der Ersatz fuer einen Abschluss: `NativeFn` ist ein Funktionszeiger und
/// sieht sein eigenes Funktionsobjekt nicht, `ObjKind::Bound` stellt die
/// gebundenen Argumente aber vorn an.
/// Ein natives mit einem GEBUNDENEN ersten Argument — der einzige Weg, einem
/// Funktionszeiger Zustand mitzugeben. `generator.rs` braucht ihn fuer die
/// zwei Behandler, mit denen ein `await` wieder anlaeuft.
```

## L298-299 · `fn finally_step(i: &mut Interp, a: &[Value], rejected: bool) -> C<Value> {`

```
/// Der Behandler von `finally`: rufen, auf sein Ergebnis warten, DANN den
/// urspruenglichen Ausgang unveraendert weitergeben.
```

## L314-315 · `pub fn to_promise(i: &mut Interp, v: &Value) -> Gc {`

```
/// Ein Wert als Versprechen: ist er schon eines, bleibt er es.
/// `PromiseResolve`: ein Versprechen bleibt es, alles andere wird eins.
```

## L335-336 · `if let Err(Abrupt::Throw(e)) = i.call(&ex, Value::Undefined, &[res, rej]) {`

```
// Der Ausfuehrer laeuft SOFORT, nicht als Microtask — und wirft er,
// wird das Versprechen abgelehnt statt der Fehler weiterzureichen.
```

## L357-366 · `let c = match species_of(i, &t)? {`

```
// **`SpeciesConstructor`** (ES §27.2.5.4 Schritt 3). Wer
// `p.constructor[Symbol.species]` setzt, bestimmt, WAS `then`
// zurueckgibt — auch wenn das gar kein Versprechen ist.
//
// Das ist nicht Feinschliff: core-js prueft mit genau diesem
// Ausdruck, ob die eingebaute `Promise` taugt. Fiel die Pruefung
// durch, ERSETZTE es sie durch seine eigene — und die kennt in der
// Fassung, die die Fritzbox ausliefert, kein `allSettled`. Die
// Komponenten warten in `connectedCallback` darauf und blieben fuer
// immer leer.
```

## L380-386 · `d(&proto, "finally", |i, t, a| {`

```
// `finally` reicht Wert UND Fehler unveraendert weiter — es sieht sie nur.
//
// Und es WARTET auf das, was der Behandler zurueckgibt: `.finally(() =>
// aufraeumen())` mit einem Versprechen darin muss das Ergebnis
// zurueckhalten, bis das Aufraeumen fertig ist. Deshalb der Umweg ueber
// ein eigenes Versprechen statt den Wert direkt zurueckzugeben — der
// kostet genau die Sprosse, die die Spezifikation dort vorsieht.
```

## L408-410 · `d(&ctor, "all", |i, t, a| { agg_this(i, &t)?; aggregate(i, a, 0) }, 1, &fp);`

```
// `all` = 0, `allSettled` = 1, `any` = 2. Eine Umsetzung fuer drei: sie
// unterscheiden sich nur darin, was ein einzelnes Ergebnis mit dem
// Zaehler macht.
```

## L430-431 · `d(&ctor, "withResolvers", |i, t, _| {`

```
// `withResolvers` gibt genau die drei Stuecke heraus, die der
// Konstruktor sonst im Ausfuehrer versteckt.
```

## L433-435 · `if !i.is_constructor(&t) { return i.type_err("withResolvers on a non-constructor"); }`

```
// Sie bauen ueber `NewPromiseCapability(this)` — also muss `this` ein
// Konstruktor sein, auch wenn wir immer unser eigenes Versprechen
// liefern.
```

## L445-446 · `d(&ctor, "try", |i, t, a| {`

```
// `Promise.try` faengt einen SYNCHRONEN Wurf ein und macht ihn zur
// Ablehnung — das ist ihr ganzer Zweck.
```

## L501 · `fn agg_step(i: &mut Interp, a: &[Value], rejected: bool) {`

```
/// Ein einzelnes Ergebnis auf den Zaehler buchen.
```

## L513-514 · `if (mode == 0 && rejected) || (mode == 2 && !rejected) {`

```
// `all` faellt beim ersten Fehler; `any` beim ersten Erfolg. Beide sind
// damit sofort fertig — der Zaehler zaehlt nur die andere Richtung.
```

## L535-536 · `let Some(agg) = so.borrow().proto.clone() else { return };`

```
// Der Zaehler liegt auf dem GEMEINSAMEN Vorfahren der Schlitze, nicht auf
// dem Schlitz — sonst zaehlte jeder fuer sich.
```

## L553 · `pub const MAX_JOBS: usize = 100_000;`

```
/// Deckel fuer `run_jobs` — dieselbe Begruendung wie die Schrittgrenze.
```

## L557-559 · `fn agg_this(i: &mut Interp, t: &Value) -> C<()> {`

```
/// Die Sammelstatiken bauen ihr Ergebnis ueber `NewPromiseCapability(this)`
/// — auf einem Nicht-Konstruktor werfen sie, bevor sie den Iterator
/// anfassen.
```

## L564-565 · `fn species_of(i: &mut Interp, t: &Value) -> C<Option<Value>> {`

```
/// `SpeciesConstructor(p, %Promise%)` — aber nur, wenn es NICHT die eigene
/// ist. `None` heisst „der gewoehnliche Weg".
```

## L572-573 · `let own = i.realm.global.borrow().get_own("Promise").and_then(|p| p.value.clone());`

```
// Die eigene `Promise` (oder ihr eigenes `species`, das auf sie zeigt)
// nimmt den kurzen Weg — sonst kostete JEDES `then` einen Konstruktoraufruf.
```

## L582-587 · `fn new_capability(i: &mut Interp, c: &Value) -> C<(Value, Value, Value)> {`

```
/// `NewPromiseCapability(C)` — `new C(executor)` und die zwei Funktionen, die
/// der Ausfuehrer bekommen hat.
///
/// Der Ausfuehrer ist ein NATIVER Sammler: er schreibt die beiden Argumente
/// in einen Kasten, den wir danach auslesen. Ein fremder Konstruktor darf sie
/// aufheben, sofort rufen oder wegwerfen — alle drei Faelle stehen so.
```

