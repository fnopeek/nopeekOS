# `tools/wasm/beak-engine/src/js/vm.rs` @ 5e0102684

## L1-40 · `use alloc::rc::Rc;`

```
//! Die Befehlsmaschine — eine Schleife statt eines Rust-Stapels.
//!
//! **Was hier anders ist als im Baumlaeufer, und nur DAS:** der Zustand einer
//! laufenden Auswertung liegt in Feldern (`stack`, `frames`), nicht in
//! Rust-Aufrufrahmen. Wegspeichern und weiterlaufen lassen ist damit
//! moeglich — das ist die ganze Begruendung des Umbaus, und alles, was
//! Generatoren und `async`/`await` brauchen.
//!
//! **Was hier NICHT anders ist: die Bedeutung.** Jeder Befehl ruft dieselbe
//! Hilfe wie der Baumlaeufer — `binary`, `unary_val`, `vm_load`, `vm_store`,
//! `get`, `set`, `call`, `construct`, `make_closure`. Wo diese Datei rechnet,
//! statt zu rufen, waere eine zweite Semantik, und die laeuft still
//! auseinander.
//!
//! **Stufe 4 und ihre Entwurfsfrage.** Sie lautete: ein Generator, der von
//! einem EINGEBAUTEN gerufen wird (`[...gen]`, `Array.from(gen)`), sitzt unter
//! einem Rust-Rahmen — dort kann er nicht anhalten. Die Antwort ist, die
//! Voraussetzung der Frage zu streichen:
//!
//! **Ein Generator ist keine Rahmen in fremder Maschine, er ist eine EIGENE.**
//! Jedes Generatorobjekt haelt seine `Vm` mit genau einem Wurzelrahmen. Ein
//! `yield` muss deshalb nie ueber einen Rust-Rahmen zurueck — zwischen dem
//! `yield` und dem `Vm::resume`, das darauf wartet, liegt keiner:
//!
//!     Array.from                (Rust)
//!       Interp::call(gen.next)  (Rust)
//!         next                  (Rust)
//!           Vm::resume          ← die EIGENE Maschine des Generators
//!             Frame(rumpf) ip=17 … Op::Yield → zurueck mit „angehalten"
//!
//! Ein `next()` kostet damit EINEN Rust-Rahmen, nicht einen je `yield`. Und
//! weil das so ist, ist es voellig gleichgueltig, wer ruft: der Baumlaeufer,
//! ein Eingebautes, `Op::Call` einer anderen Maschine oder ein zweiter
//! Generator. Niemand muss etwas dazulernen, es gibt keinen eifrigen
//! Rueckfall und keine Falle mit unendlichen Generatoren.
//!
//! Ein `yield` steht immer im Rumpf des Generators SELBST (in einer inneren
//! Funktion waere es deren eigenes), und ein Aufruf von dort muss
//! zurueckkehren, bevor es weitergeht — beim `Op::Yield` ist der Wurzelrahmen
//! also der einzige. Deshalb reicht ein Wurzelrahmen je Maschine.
```

## L50-51 · `struct Frame {`

```
/// Ein Aufrufrahmen. Heute gibt es genau einen (das Programm); die Form steht
/// schon, weil sie der Punkt der Uebung ist.
```

## L55-56 · `envs: Vec<Rc<RefCell<Env>>>,`

```
/// Die Umgebung, in der dieser Rahmen laeuft. `PushEnv`/`PopEnv` schieben
/// hier, nicht auf dem Rust-Stapel.
```

## L58-59 · `base: usize,`

```
/// Der Stapelstand beim Betreten — beim Verlassen wird darauf zurueck-
/// geschnitten, damit ein `Ret` mitten im Ausdruck nichts liegenlaesst.
```

## L61 · `handlers: Vec<Handler>,`

```
/// Offene `try`-Behandler, innerster zuletzt.
```

## L63-64 · `is_program: bool,`

```
/// Ist das der Rahmen eines PROGRAMMS? Dessen Wert ist sein letzter
/// Ausdruckswert, der einer Funktion ihr `return`.
```

## L66-72 · `root: bool,`

```
/// Der UNTERSTE Rahmen dieser Maschine. Ein Wurf sucht darueber hinaus
/// keinen Behandler mehr, ein `Ret` beendet den Lauf, und die Aufruftiefe
/// wird nicht mitgezaehlt — dieser Rahmen gehoert nicht dem Rufer.
///
/// Getrennt von `is_program`, weil ein Generatorrumpf zwar Wurzel ist,
/// aber KEINEN Abschlusswert hat: ein `{ x = 1; }` in ihm setzt
/// `SetCompletion`, und das duerfte sein `return` nicht ueberschreiben.
```

## L74-80 · `iters: Vec<Iter>,`

```
/// Offene Laufzustaende von `for…of` und `for…in`. Sie stehen HIER und
/// nicht auf dem Wertestapel: ein `break` oder ein Wurf mitten in der
/// Schleife muesste sie sonst einzeln wegraeumen, und das ist genau die
/// Buchhaltung, an der solche Maschinen scheitern.
///
/// Beide in EINER Liste, weil die ganze Aufraeumerei — Behandlertiefe,
/// `Ret`, `unwind` — sie dann nur einmal kennen muss.
```

## L84 · `enum Iter {`

```
/// Was eine laufende Schleife festhaelt.
```

## L86-87 · `Obj(Value),`

```
/// Ein echter Iterator (`for…of`). Sein `return()` gehoert bei jedem
/// vorzeitigen Verlassen gerufen.
```

## L89-92 · `Async { it: Value, is_async: bool, done: bool },`

```
/// Ein Iterator eines `for await`. `async` sagt, ob er wirklich einer ist
/// (dann ist das abzuwartende Ding das ganze Ergebnisobjekt) oder ob wir
/// einen synchronen umhuellen (dann ist es nur sein `value`, und `done`
/// steht schon hier — ES 27.1.4.4, AsyncFromSyncIteratorContinuation).
```

## L94-96 · `Keys(Vec<Value>),`

```
/// Die Schluesselliste eines `for…in`, RUECKWAERTS — dann ist `pop` der
/// naechste Schritt und es braucht keinen Index daneben. Es gibt hier
/// nichts zu schliessen: die Liste steht schon fest.
```

## L100-102 · `struct Handler {`

```
/// Ein offener `try`. Die drei Tiefen sind der Punkt: ein Wurf kann mitten in
/// einem Ausdruck passieren, und dann liegen halbe Werte auf dem Stapel,
/// offene Blockumgebungen im Rahmen und angefangene Iterationen daneben.
```

## L111 · `pub enum Step {`

```
/// Wie ein Lauf geendet hat.
```

## L113 · `Done(Value),`

```
/// Der Wurzelrahmen ist zurueck.
```

## L115-121 · `Yield(Value, bool),`

```
/// `yield` — die Maschine steht und laesst sich wieder aufnehmen.
///
/// Das `bool` heisst ROH: der Wert IST schon das `{value, done}`-Objekt
/// und darf nicht noch einmal eingepackt werden. Genau das verlangt
/// `yield*` im gewoehnlichen Generator — es reicht das Ergebnisobjekt des
/// INNEREN Iterators unveraendert durch (ES 15.5.5, `GeneratorYield`),
/// und Tests pruefen die Identitaet.
```

## L123-124 · `Await(Value),`

```
/// `await` — dasselbe Anhalten, nur wartet hier ein Versprechen darauf,
/// sie wieder anzuwerfen, statt eines `next()`.
```

## L128 · `enum Flow {`

```
/// Wie ein einzelner Befehl ausgegangen ist.
```

## L130 · `Go,`

```
/// Weiter zum naechsten.
```

## L137-143 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// Womit eine angehaltene Maschine wieder angeworfen wurde.
///
/// **Der dritte Weg hinein.** `send` allein reicht fuer ein gewoehnliches
/// `yield`: ein `next(v)` legt `v` an die Anhaltestelle, ein `throw(e)`
/// wickelt sofort ab, ein `return(v)` gibt die Maschine auf. `yield*` braucht
/// aber alle drei ALS WERT — es muss sie an den inneren Iterator
/// weiterreichen, statt selbst darauf zu reagieren.
```

## L150 · `completion: Value,`

```
/// Der Abschlusswert des Programms (sein letzter Ausdruckswert).
```

## L152 · `resume: Resume,`

```
/// Womit zuletzt wieder angeworfen wurde — nur `yield*` liest es.
```

## L154-157 · `deleg: Resume,`

```
/// Dasselbe, festgehalten ueber ein `await` hinweg: in einem
/// async-Generator liegt zwischen dem Aufruf am inneren Iterator und der
/// Auswertung seines Ergebnisses ein Anhaltepunkt, und der wirft mit
/// `send` wieder an — das setzt `resume` auf `Normal` zurueck.
```

## L167-169 · `pub fn run(&mut self, i: &mut Interp, chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> C<Value> {`

```
/// Ein uebersetztes Programm fahren. `env` ist die Umgebung, in die der
/// Rufer schon hochgezogen hat — das Hochziehen bleibt beim Baumlaeufer,
/// weil es auf der UMGEBUNG arbeitet und fuer beide Maschinen dasselbe ist.
```

## L176-178 · `Step::Yield(..) => Err(i.throw_kind("TypeError", "yield outside a generator")),`

```
// Ein Programmrumpf wird mit `in_gen == false` uebersetzt; ein
// `Op::Yield` kann darin nicht stehen. Kein `panic!`: ein Absturz
// der Maschine ist in einem Kernel keine Fehlermeldung.
```

## L184-196 · `pub fn run_function(i: &mut Interp, chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> C<Value> {`

```
/// Einen FUNKTIONSRUMPF auf der Maschine fahren, wenn der Ruf NICHT von
/// ihr kommt.
///
/// `Op::Call` in der Maschine legt einen Rahmen an und laeuft weiter —
/// aber jeder Aufruf, der von aussen kommt (aus einem eingebauten
/// Rueckruf, aus der Microtask-Schlange, aus einem Ereignisbehandler, aus
/// einem Generator), ging ueber `Interp::run_js_body` und damit auf den
/// BAUMLAEUFER. Und weil dessen Aufrufe wieder dort landen, blieb alles
/// darunter beim Baumlaeufer: die Fritzbox-Anmeldung fuhr 320 721
/// Schritte, davon 4 286 auf der Maschine.
///
/// Es ist derselbe Chunk, den `Op::Call` benutzt haette — kein zweiter
/// Semantikpfad, nur derselbe von einem anderen Rufer aus erreicht.
```

## L209-213 · `pub fn for_generator(chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> Vm {`

```
/// Eine Maschine fuer einen GENERATOR- oder ASYNC-RUMPF: ein einziger
/// Wurzelrahmen,
/// noch nichts gelaufen. Die Umgebung hat `Interp::call_env` gebaut —
/// Parameter und `this` stehen beim AUFRUF fest, der Rumpf laeuft erst
/// beim ersten `next()`. Genau diese Reihenfolge verlangt die Spec.
```

## L222-223 · `pub fn send(&mut self, v: Value) {`

```
/// Den Wert von `next(v)` an die Stelle legen, an der `Op::Yield` seinen
/// abgegeben hat — er ist der Wert des `yield`-Ausdrucks.
```

## L229-231 · `pub fn send_throw(&mut self, v: Value) {`

```
/// Wieder anwerfen mit einem WURF — aber ohne abzuwickeln. Nur sinnvoll,
/// wenn die Maschine an einem `yield*` steht (`at_delegate`): dort ist der
/// Wurf ein WERT, der an den inneren Iterator geht.
```

## L237-239 · `pub fn send_return(&mut self, v: Value) {`

```
/// Wieder anwerfen mit einem `return`. Dasselbe: an einem `yield*` bekommt
/// der innere Iterator sein `return()` zu sehen, bevor der aeussere
/// Generator aufgibt.
```

## L245 · `fn delegate_iter(&self) -> Option<Value> {`

```
/// Der innere Iterator des laufenden `yield*`.
```

## L253-259 · `fn do_return(&mut self, i: &mut Interp) -> C<Flow> {`

```
/// Den obersten Rahmen verlassen und seinen Wert zurueckgeben.
///
/// Herausgeloest aus `Op::Ret`, weil `yield*` denselben Ausgang braucht:
/// **ein `return()` am Generator, das den inneren Iterator erschoepft
/// hat, IST ein `return` aus dem aeusseren Rumpf** — samt dem Schliessen
/// jeder offenen `for…of`-Iteration darin. Zwei Fassungen davon waeren
/// zwei Semantiken fuer denselben Ausgang.
```

## L267-269 · `for it in f.iters.iter().rev() {`

```
// Ein `return` aus einer `for…of`-Schleife heraus muss ihren Iterator
// SCHLIESSEN — sonst faehrt ein Generator seinen eigenen
// `finally`-Block nie. Von innen nach aussen.
```

## L276-278 · `let c = core::mem::replace(&mut self.completion, Value::Undefined);`

```
// Der Wert eines PROGRAMMS ist sein letzter Ausdruckswert,
// nicht das, was am Ende auf dem Stapel liegt. Ein Funktions-
// oder Generatorrumpf hat keinen.
```

## L292-295 · `pub fn at_delegate(&self) -> bool {`

```
/// Steht die angehaltene Maschine an einem `yield*`?
///
/// `drive` zaehlt `ip` VOR dem Befehl hoch, also steht der Anhaltebefehl
/// bei `ip - 1`.
```

## L303-305 · `pub fn inject_throw(&mut self, i: &mut Interp, v: Value) -> bool {`

```
/// `gen.throw(v)`: den Wurf an der Anhaltestelle einwerfen. `false`
/// heisst, dass ihn hier keiner faengt — dann ist der Generator fertig
/// und der Wurf geht an den Rufer.
```

## L310-315 · `pub fn close(&mut self, i: &mut Interp) {`

```
/// `gen.return(v)`: die Maschine aufgeben. Offene `for…of`-Iterationen
/// werden GESCHLOSSEN (von innen nach aussen), nicht bloss vergessen —
/// sonst faehrt ein fremder Generator seinen `finally`-Block nie.
///
/// Ein anhaengiger `finally` KANN es hier nicht geben: ein `yield` unter
/// einem solchen ist schon beim Uebersetzen abgelehnt (`Compiler::fin`).
```

## L326-331 · `pub fn roots(&self, objs: &mut Vec<super::value::Gc>,`

```
/// Alles, was diese Maschine festhaelt — fuer den Abbau eines Realms.
///
/// Ein angehaltener Generator ist der einzige Ort, an dem Umgebungen und
/// halbfertige Werte leben, ohne in einer Eigenschaft oder Bindung zu
/// stehen. `Interp::teardown` faende sie sonst nicht, und ein Rc-Ring
/// darin kaeme nie auf null.
```

## L346-347 · `pub fn drive(&mut self, i: &mut Interp) -> C<Step> {`

```
/// Die Schleife. Sie laeuft, bis der Wurzelrahmen zurueck ist oder ein
/// `yield` sie anhaelt — und beim naechsten Aufruf genau dort weiter.
```

## L349-353 · `let mut held: Option<(usize, Rc<Chunk>)> = None;`

```
// **Der Chunk wird GEHALTEN, nicht je Befehl neu geliehen.** Ein
// `Rc::clone` mit dem Freigeben danach sind zwei Zaehleroperationen —
// je BEFEHL, auf dem heissesten Pfad des Motors. Gewechselt wird er
// nur, wenn sich der Rahmen aendert, und das prueft ein
// Zeigervergleich.
```

## L373-382 · `let counts = match &chunk.ops[ip] {`

```
// Der Deckel gegen `while(true)` — aber mit derselben KOERNUNG wie
// im Baumlaeufer, sonst ist er ein anderer Deckel.
//
// Der zaehlt einen Schritt je ANWEISUNG. Je Befehl zu zaehlen
// waere feiner und damit strenger: derselbe Test, der dort
// durchlief, brach hier ab (`encodeURI` mit seiner langen
// Zeichentabelle). Gezaehlt wird deshalb, was eine Schleife
// wirklich vorantreibt — ein Rueckwaertssprung, ein Aufruf, eine
// Anweisungsgrenze. Das ist dieselbe Groessenordnung und bleibt
// eine echte Abbruchgarantie.
```

## L388-389 · `| Op::YieldDelegate(_) | Op::DelegateCall(_) => true,`

```
// Ein `yield*` ruft je Umlauf am inneren Iterator — das treibt
// die Schleife voran und gehoert unter denselben Deckel.
```

## L399-403 · `if i.steps & 0xFFFF == 0 { i.check_deadline()?; }`

```
// Und die Uhr — dieselbe Koernung wie im Baumlaeufer, sonst
// ist es eine andere Uhr. Siehe `Interp::check_deadline`:
// sie stand nur in `tick`, und der zaehlt nur eingebaute
// Schleifen. Genau die Rechnung, die Minuten dauert, kam
// deshalb nie an ihr vorbei.
```

## L411-413 · `Err(Abrupt::Throw(v)) => {`

```
// Ein Wurf sucht sich seinen Behandler. Findet er keinen, geht
// er an den Rufer — dann faehrt ihn der Rust-Stapel hoch, und
// das ist richtig, solange Aufrufe noch so laufen.
```

## L425-431 · `let env = self.frames.last().unwrap().envs.last().unwrap().clone();`

```
// **Hier und nicht je Arm.** Ein Versuch, sie erst beim Gebrauch zu
// holen, sparte ein `Rc`-Zaehlerpaar je Befehl — und war falsch:
// einige Befehle AENDERN die Umgebungskette, bevor sie sie benutzen,
// und bekamen dann die neue statt der alten
// (`cannot access 'dialog' before initialization`). Wer das noch
// einmal angeht, muss je Arm nachweisen, dass er vor jeder Aenderung
// liest — nicht es annehmen.
```

## L453-454 · `i.bind_here(n, v, &env);`

```
// GENAU HIER — `init_binding` liefe die Kette hoch und
// schriebe eine gleichnamige Bindung weiter aussen.
```

## L457-464 · `i.vm_store(n, v, &env)?;`

```
// Ein `var` ist hier laengst hochgezogen; die Zeile
// ZUWEIST nur noch (ES §VariableStatement: PutValue).
// Dieselbe Regel wie im Baumlaeufer, und sie muss hier
// ein zweites Mal stehen, weil die zweite Maschine ihren
// eigenen Weg hat
// ([[feedback_the_second_engine_only_runs_where_the_first_one_called]]):
// `init_binding` legte eine zweite Bindung neben die auf
// dem globalen Objekt, und `window.X` blieb undefined.
```

## L491-492 · `Op::Swap => {`

```
// `a b` → `b a`: der Aufruf braucht `callee` unter `this`, und der
// Uebersetzer legt sie in der anderen Reihenfolge ab.
```

## L550-557 · `if let Some(ix) = int_index(&key) {`

```
// **Ein ganzzahliger Index geht ohne Haufen.** `a[i]` baute
// bisher aus `i` eine Zeichenkette AUF DEM HAUFEN (`Rc<str>`),
// gab sie an `get`, und `ta_read`/`array_index` lasen die Zahl
// sofort wieder heraus. Der Schluessel ist derselbe — er wird
// nur in einen Puffer auf dem Stapel geschrieben statt
// alloziert und wieder freigegeben. Auf einer Seite, die
// rechnet (SHA-256, Bildbearbeitung, ein Parser), ist das der
// haeufigste Befehl ueberhaupt.
```

## L563-567 · `let k = i.to_prop_key(&key)?;`

```
// `to_prop_key`, NICHT `to_string`: ein Symbol ist ein
// Schluessel und keine Zeichenkette, und es zu einer zu
// machen hat `Symbol.iterator` & Co. ins Leere zeigen
// lassen — 35 Tests, gefunden vom Diff gegen den
// Baumlaeufer.
```

## L599-601 · `if n == Some("eval") && i.is_eval_fn(&callee) {`

```
// Ein DIREKTER `eval`-Aufruf ist kein gewoehnlicher: er sieht
// den Bereich des Rufers. Erkannt wird er am Namen UND an der
// Sache — eine eigene Funktion namens `eval` ist keiner.
```

## L603-604 · `i.hints_ok = false;`

```
// Ab jetzt kann eine Bindung weiter INNEN entstehen, als
// ein Wegweiser zeigt. Siehe `Interp::hints_ok`.
```

## L649 · `Op::BindPat { pat, mode } => {`

```
// Dieselben Hilfen wie der Baumlaeufer — siehe `Op::BindPat`.
```

## L663 · `Op::Class(c) => {`

```
// Dieselbe Funktion, die der Baumlaeufer ruft — siehe `Op::Class`.
```

## L679-680 · `i.name_function(&val, &key);`

```
// `{ m(){} }` und `{ a: function(){} }` bekommen den
// Schluessel als Namen — dieselbe Regel wie `var f = …`.
```

## L849-855 · `Err(e) => {`

```
// **Wirft `next()` SELBST, wird nicht geschlossen.** Der
// Iterator ist dann in unbekanntem Zustand, und `return()`
// darauf waere spec-widrig — anders als bei einem Wurf aus
// dem RUMPF, der ihn sehr wohl schliessen muss. Deshalb
// faellt er hier aus der Liste, bevor der Wurf geht: sonst
// holt ihn der naechste Behandler oder das Verlassen des
// Rahmens nach.
```

## L865 · `keys.reverse();`

```
// Rueckwaerts, damit `pop` der naechste Schritt ist.
```

## L886-890 · `Some(Iter::Async { it, .. }) => i.iter_close(&it),`

```
// **Benannt halb:** die Spezifikation WARTET das Ergebnis
// von `return()` an einem async-Iterator ab
// (AsyncIteratorClose). Wir rufen es und gehen weiter —
// der Unterschied ist sichtbar, wenn ein `break` einen
// Aufraeumer startet, auf den danach jemand zaehlt.
```

## L912-913 · `let r = match i.call(&f, it.clone(), &[]) {`

```
// Wie beim synchronen `IterNext`: wirft `next()` SELBST, wird
// nicht geschlossen — der Iterator faellt vorher aus der Liste.
```

## L953-955 · `for d in &chunk.blocks[*b as usize] {`

```
// Erst binden, dann laufen: `let` steht von Blockanfang an in
// der Totzone, eine Funktionsdeklaration ist von Blockanfang
// an fertig. Genau die zwei Schleifen aus `Interp::hoist`.
```

## L975-983 · `Op::DelegateStart(is_async) => {`

```
// Der Rahmen bleibt stehen, wo er steht — `ip` zeigt schon auf den
// naechsten Befehl, und `Vm::send` legt den Wert von `next(v)` an
// die Stelle, an der dieser hier seinen abgegeben hat.
// ── yield* ───────────────────────────────────────────────────
//
// Drei Befehle, weil zwischen dem Anstossen des inneren Iterators
// und dem Auswerten seines Ergebnisses in einem async-Generator
// ein `await` liegt — und ein Anhaltepunkt laesst sich nicht in
// einen Befehl hineinfalten. Derselbe Schnitt wie bei `for await`.
```

## L993-995 · `self.resume = Resume::Normal;`

```
// Der erste „erhaltene" Wert ist `undefined` (ES 15.5.5,
// Schritt 5) — und die Art ist `Normal`, egal womit der
// aeussere Generator gerade lief.
```

## L1015-1017 · `Resume::Throw => {`

```
// **Ein Iterator ohne `throw` bekommt sein `return`**
// (ES 15.5.5 6.b.iii) und der Wurf wird zum TypeError:
// der innere darf nicht halb offen zurueckbleiben.
```

## L1023-1024 · `Resume::Return => { self.push(received); self.jump(*giveup); Ok(Flow::Go) }`

```
// Ohne `return` gibt der aeussere Generator direkt auf,
// mit dem Wert, den `gen.return(v)` mitgebracht hat.
```

## L1030-1032 · `let r = match i.call(&m, it, &[received]) {`

```
// Wirft der Aufruf SELBST, bleibt der innere Iterator liegen —
// dieselbe Regel wie im gewoehnlichen `for…of`: sein Zustand
// ist dann unbekannt, und `return()` darauf waere spec-widrig.
```

## L1037-1042 · `let wrapped_sync = matches!(self.frames.last().unwrap().iters.last(),`

```
// **Ein umgehuellter SYNCHRONER Iterator gibt sein Ergebnis
// schon fertig** — abzuwarten ist dort nur der WERT, und sein
// `done` steht jetzt fest (ES 27.1.4.4,
// AsyncFromSyncIteratorContinuation). Dieselbe Aufteilung wie
// bei `for await`; ohne sie kam aus `yield* [Promise…]` das
// Versprechen selbst heraus.
```

## L1062-1063 · `if let Some(Iter::Async { is_async: false, done, .. }) =`

```
// Der umgehuellte synchrone Fall: `done` steht schon am
// Eintrag, und `r` IST der abgewartete Wert.
```

## L1086-1089 · `if self.deleg == Resume::Return {`

```
// **Womit angeworfen wurde, entscheidet den AUSGANG.** Ein
// erschoepfter innerer Iterator nach einem `gen.return(v)`
// beendet den aeusseren Rumpf; nach einem `next()` ist der
// Wert bloss das Ergebnis des `yield*`-Ausdrucks.
```

## L1095-1097 · `let v = i.get(&r, "value")?;`

```
// Ein async-Generator gibt den WERT heraus und laesst ihn
// einpacken; ein gewoehnlicher reicht das Ergebnisobjekt
// unveraendert durch (`Op::YieldDelegate`).
```

## L1108-1112 · `Op::YieldDelegate(raw) => {`

```
// Der Anhaltepunkt eines `yield*` im GEWOEHNLICHEN Generator: der
// Wert ist schon das Ergebnisobjekt des inneren Iterators und geht
// unveraendert hinaus. Er ist ausserdem die Marke, an der
// `at_delegate` erkennt, dass ein `throw()` oder `return()` hier
// NICHT abwickeln darf.
```

## L1125-1130 · `fn unwind(&mut self, i: &mut Interp, v: Value) -> bool {`

```
/// Den innersten Behandler suchen, der den Wurf nimmt. `false` heisst:
/// keiner da, der Wurf verlaesst diese Maschine.
///
/// Zurueckgeschnitten wird auf die Tiefen, die der Behandler sich gemerkt
/// hat — Wertestapel, Umgebungen UND offene Iterationen. Wer eins davon
/// vergisst, bekommt einen `catch`-Block, der auf fremdem Zustand steht.
```

## L1132-1136 · `let h = loop {`

```
// **Ueber Rahmengrenzen hinweg.** Ein `throw` tief in einer Funktion
// sucht sein `try` beim RUFER, wenn es dort keins gibt — genau das
// macht ein Aufrufstapel aus. Ohne diese Schleife blieb ein Wurf im
// eigenen Rahmen haengen und kam als „UNCAUGHT" heraus, obwohl zwei
// Ebenen darueber ein `catch` stand.
```

## L1143-1145 · `if f.root {`

```
// Kein Behandler in diesem Rahmen: ihn verlassen und weitersuchen.
// Das PROGRAMM ist die Grenze — darueber gibt es nur den Rufer in
// Rust, und dorthin geht der Wurf als `Err`.
```

## L1150-1156 · `for it in f.iters.iter().rev() {`

```
// **Auch beim Verlassen eines Rahmens gehoeren offene `for…of`
// geschlossen** — von innen nach aussen, genau wie im `Ret`. Ohne
// diese Schleife faehrt ein fremder Generator seinen
// `finally`-Block nie, wenn der Wurf aus dem Schleifenrumpf durch
// die Funktion nach draussen geht. Gefunden vom Diff, als
// `for ([x.attr] of it)` uebersetzbar wurde und ein werfender
// Schreiber im KOPF dasselbe ausloeste.
```

## L1163-1165 · `loop {`

```
// Ein Wurf aus einer `for…of`-Schleife heraus muss ihren Iterator
// SCHLIESSEN, nicht nur vergessen — `return()` ist der Weg, auf dem
// ein Generator seinen `finally`-Block noch faehrt.
```

## L1181-1192 · `fn invoke(&mut self, i: &mut Interp, callee: Value, this: Value, args: Vec<Value>,`

```
/// Einen Aufruf ausfuehren.
///
/// **Der Punkt der ganzen Stufe:** ist der Gerufene eine JS-Funktion,
/// deren Rumpf sich uebersetzen laesst, bekommt er einen RAHMEN — der
/// Rust-Stapel waechst nicht mit. Alles andere (eingebaute Funktionen,
/// gebundene, Generatoren) geht weiter durch `Interp::call`, und das ist
/// richtig so: die haben keinen Rumpf aus Befehlen.
///
/// Die Tiefe wird trotzdem gezaehlt. Ein Rahmenstapel kann nicht
/// ueberlaufen, aber eine endlose JS-Rekursion soll denselben
/// `RangeError` geben wie vorher — sonst haengt sie, bis der Speicher
/// ausgeht.
```

## L1203-1205 · `if !i.is_callable(&callee) {`

```
// Den NAMEN nennen, nicht nur das Ereignis — dieselbe Hilfe wie im
// Baumlaeufer. Ohne diese Zeile verlor jedes Skript, das auf die
// Maschine wanderte, still seine Fehlerdiagnose.
```

## L1214-1219 · `if d.node.is_generator || d.node.is_async {`

```
// **Ein Generator und eine async-Funktion bekommen hier keinen
// Rahmen.** Ihr Aufruf baut ein Objekt bzw. ein Versprechen, und der
// Rumpf laeuft auf einer EIGENEN Maschine — beides tut `Interp::call`
// an einer Stelle, fuer beide Maschinen dieselbe. Die Pruefung steht
// vor `func_chunk`, weil dessen Chunk hier sonst als gewoehnlicher
// Funktionsrumpf losliefe.
```

## L1262-1265 · `fn pop(&mut self) -> Value {`

```
/// Der Uebersetzer erzeugt nur ausgeglichenen Code; ein leerer Stapel hier
/// waere ein Fehler IM UEBERSETZER, kein Programmfehler. `Undefined` statt
/// `panic!`, weil ein Absturz der Maschine in einem Kernel keine
/// Fehlermeldung ist, sondern ein Halt.
```

## L1280-1285 · `#[inline]`

```
/// Ein Feldindex als Zahl, wenn der Schluessel einer ist.
///
/// Die Grenze ist die von `array_index`: `0 <= n < 2^32-1` und ganzzahlig.
/// Genau in diesem Bereich ist die Zeichenkette einer Zahl in JS die schlichte
/// Dezimaldarstellung, also derselbe Schluessel, den `to_string` gebaut haette.
/// `-0` faellt mit hinein und wird zu `"0"` — was JS auch tut.
```

## L1294-1295 · `struct IdxBuf {`

```
/// Zehn Ziffern auf dem STAPEL — die groesste Zahl in diesem Bereich hat
/// zehn (`4294967294`).
```

## L1316 · `unsafe { core::str::from_utf8_unchecked(&self.b[self.at..]) }`

```
// SAFETY: nur ASCII-Ziffern geschrieben, also gueltiges UTF-8.
```

## L1325-1336 · `static CALLS: AtomicU32 = AtomicU32::new(0);`

```
/// **Die Uhr des Wirts muss BEIDE Maschinen erreichen.**
///
/// Bis 0.117.0 stand sie nur in `Interp::tick`, und `tick` ruft nur, wer
/// in einem EINGEBAUTEN schleift (`Array.prototype.*`, `JSON`, die
/// Iterator-Hilfen). Eine reine JS-Schleife lief an ihr vorbei: kein
/// Herzschlag, und das Zeitbudget war fuer genau den Fall unwirksam, fuer
/// den es gedacht ist. Am Geraet waren das vier Minuten Stillstand ohne
/// eine Zeile im Log.
///
/// Der Test schleift deshalb OHNE eingebauten Aufruf. Mit einem darin
/// haette er auch vorher bestanden — und genau das ist die Falle, die die
/// Luecke so lange offen gehalten hat.
```

## L1340 · `false // sofort abgelaufen: der Lauf muss hier enden`

```
// sofort abgelaufen: der Lauf muss hier enden
```

## L1354-1357 · `fn class_self(novm: bool) -> alloc::string::String {`

```
/// **Eine benannte Klasse steht in ihrem EIGENEN Rumpf** (ES 15.7.14) —
/// und zwar in beiden Maschinen. DuckDuckGos Intl-Polyfill baut seine
/// `Locale` als `class aa { … new aa(…) … }` und starb sonst mit
/// `aa is not defined`; der Name darf dabei NICHT nach aussen dringen.
```

## L1391-1399 · `fn zweimal(src: &str) -> (alloc::string::String, alloc::string::String) {`

```
/// **Ein Wegweiser darf die BEDEUTUNG nicht aendern.**
///
/// `Chunk::hints` merkt sich, in welcher Tiefe ein Name beim letzten Mal
/// stand. Entstuende danach eine Bindung WEITER INNEN, zeigte er daran
/// vorbei — und das waere kein Absturz, sondern ein falscher Wert.
///
/// Der Test faehrt dieselben Programme ZWEIMAL, einmal mit Wegweisern und
/// einmal ohne, und vergleicht. Ein Test, der nur „laeuft durch" prueft,
/// saehe genau den Fehler nicht, um den es geht.
```

## L1426-1427 · `"var o=[];var x='a';for(var k=0;k<3;k++){let x='b'+k;o.push(x);}o.push(x);o.join(',')",`

```
// Verschattung in einem Block, in einer Schleife: dieselbe
// Befehlsstelle, viele Durchlaeufe.
```

## L1429 · `"var x='aussen';function f(){return x}function g(){var x='innen';return f()+'|'+x}g()",`

```
// Ein Abschluss liest nach aussen, waehrend innen gleich heisst.
```

## L1431 · `"function f(){ try { return y } catch(e) { return 'TDZ:'+e.name } finally { } } var r=f(); let y=1; r",`

```
// Zeitliche Totzone: der Name STEHT hier, ist aber noch nichts.
```

## L1433 · `"const c=1; try { c=2; return } catch(e) { e.name }",`

```
// `const` beschreiben — der Fehler muss derselbe bleiben.
```

## L1435 · `"var x='aussen';function f(){function inner(){return x}var a=inner();eval(\"var x='innen'\");return a+'|'+inner()}f()",`

```
// Ein direktes `eval`, das eine Bindung WEITER INNEN anlegt.
```

## L1437 · `"var a=1;function f(){var b=2;return function(){var c=3;return function(){return a+b+c}}}f()()()",`

```
// Tief geschachtelt, damit der Weg wirklich mehrere Spruenge hat.
```

## L1439 · `"var n='g';function f(){var n='f';{let n='b';return n+f2()}}function f2(){return n}f()",`

```
// Derselbe Name auf mehreren Ebenen, gelesen von innen nach aussen.
```

## L1448-1449 · `fn beide(src: &str) -> (alloc::string::String, alloc::string::String) {`

```
/// Dasselbe Programm auf BEIDEN Maschinen — die Befehlsmaschine und der
/// Baumlaeufer muessen Zeichen fuer Zeichen dasselbe sagen.
```

## L1473-1480 · `#[test]`

```
/// **`yield*` — die Delegation, und sie hat NIE funktioniert.**
///
/// Bis 0.169 sagte der Uebersetzer bei jedem `yield*` ab
/// (`yield-delegate`), und der Baumlaeufer dahinter warf „generators are
/// not supported" — auch im gewoehnlichen Generator. Der Grund, warum es
/// kein Anbau war: an der Anhaltestelle muss die Maschine WISSEN, womit
/// sie wieder angeworfen wurde (Wert · Wurf · `return`), um es an den
/// inneren Iterator weiterzureichen.
```

## L1484 · `("function* i(){ yield 1; yield 2; return 'fin' }\`

```
// Der Rueckgabewert des INNEREN ist der Wert des `yield*`.
```

## L1488 · `("function* g(){ yield* [1,2]; yield* 'ab'; yield* new Map([['k',1]]).keys() }\`

```
// Ueber ein Feld, eine Zeichenkette, einen eingebauten Iterator.
```

## L1491 · `("function* e(){ while(true){ const g = yield '?'; if(g==='stop') return 'E' } }\`

```
// `next(v)` erreicht das INNERE `yield`.
```

## L1495 · `("function* c(){ try{ yield 'A' }catch(e){ yield 'f:'+e.message } yield 'B' }\`

```
// `throw()` geht an den inneren Iterator, der ihn fangen darf.
```

## L1501 · `("var m={[Symbol.iterator](){return{next:()=>({value:'x',done:false}),\`

```
// `return()` laesst den inneren aufraeumen, BEVOR der aeussere aufgibt.
```

## L1506 · `("var m={[Symbol.iterator](){var n=0;return{next:()=>n++?{value:9,done:true}\`

```
// **Das Ergebnisobjekt des INNEREN geht unveraendert hinaus.**
```

## L1511-1512 · `("var m={[Symbol.iterator](){return{next:()=>({value:1,done:false}),\`

```
// Ein Iterator ohne `throw` bekommt sein `return` und dann einen
// TypeError (ES 15.5.5, 6.b.iii).
```

## L1538 · `#[test]`

```
/// `yield*` im ASYNC-Generator: derselbe Weg, nur wartet er zwischendurch.
```

## L1545-1546 · `("async function* g(){ yield* [Promise.resolve('p'),'q'] }\`

```
// Ein async-Generator delegiert an einen SYNCHRONEN Iterator: die
// Werte darin werden abgewartet (AsyncFromSyncIteratorContinuation).
```

## L1549 · `("async function* c(){ try{ yield 'A' }catch(e){ yield 'f:'+e.message } }\`

```
// `throw()` erreicht auch hier den inneren Generator.
```

## L1561-1572 · `fn async_out(src: &str) -> alloc::string::String {`

```
/// **Async-Generatoren und `for await`.**
///
/// Beide Anhaltegruende aus DERSELBEN Maschine: `yield` haelt fuer ein
/// `next()` an, `await` fuer die Microtask-Schlange. Bis 0.167 sagte der
/// Uebersetzer bei jedem `async function*` ab — und zwar fuer den GANZEN
/// umgebenden Chunk, weshalb 2518 Programme komplett auf den Baumlaeufer
/// fielen, der kein `yield` kann.
///
/// Gefahren wird ueber `jsrun`s Weg: Programm laufen lassen, dann die
/// Schlange leeren, dann das Ergebnis ablesen. Ohne das zweite steht in
/// `out` nichts — ein async-Generator liefert seinen ersten Wert
/// fruehestens im naechsten Microtask.
```

## L1597-1598 · `("async function* g(){ yield 1; yield Promise.resolve(2); await null; yield 3 }              (async()=>{ for await (cons`

```
// `yield` im async-Generator WARTET seinen Wert ab (ES 15.5.5):
// `yield Promise.resolve(2)` gibt 2 heraus, nicht das Versprechen.
```

## L1600-1601 · `("async function* g(){ yield 'a'; yield 'b'; yield 'c' }              (async()=>{ const h=g(); const r=await Promise.all`

```
// Drei `next()` auf einmal stellen sich an, statt die Maschine
// dreimal anzuwerfen (ES 27.6.3.6).
```

## L1603 · `("async function* g(){ yield 1 }              (async()=>{ const h=g(); await h.next(); const e=await h.next();          `

```
// Ein fertiger Generator beantwortet jede weitere Anfrage.
```

## L1605 · `("async function* g(){ yield 1; throw new Error('drin') }              (async()=>{ const h=g(); await h.next();         `

```
// Ein Wurf im Rumpf LEHNT AB, er wirft nicht.
```

## L1608-1609 · `("(async()=>{ for await (const x of [Promise.resolve('p'),'q']) out.push(x) })()", "p|q"),`

```
// `for await` ueber ein gewoehnliches Feld huellt den synchronen
// Iterator ein — und wartet dabei jeden WERT ab.
```

## L1611 · `("const it={[Symbol.asyncIterator](){let n=0;return{                next:()=>Promise.resolve({value:n++,done:n>9}),     `

```
// `break` schliesst den async-Iterator.
```

## L1614 · `("async function* g(){}              (async()=>{ const h=g();              out.push(Object.prototype.toString.call(h)); `

```
// Der Tag und die Selbst-Iterierbarkeit stehen am Prototyp.
```

## L1623-1626 · `#[test]`

```
/// **Der Uebersetzer darf an einem async-Generator nicht mehr absagen** —
/// und zwar auch dann nicht, wenn er nur NEBEN dem Code steht. Vor 0.167
/// liess ein `async function*` irgendwo im Programm den ganzen Chunk
/// ablehnen, und damit fiel auch der Code daneben auf den Baumlaeufer.
```

## L1640-1645 · `#[test]`

```
/// **Getaggte Templates, auf beiden Maschinen.**
///
/// Sie waren bis 0.166 in KEINER von beiden gebaut — der Uebersetzer sagte
/// `tagged-template` ab, und der Baumlaeufer dahinter warf. Damit starb
/// jede Seite mit lit-html, styled-components oder graphql-tag an der
/// ersten Zeile ihrer Bibliothek.
```

## L1649 · `(r"function t(s,...v){return s.raw.join('|')+'#'+s.join('|')+'#'+v.join(',')+'#'+s.length}ta${1}b\t${2}c",`

```
// Gekochte und ROHE Stuecke, dazu die Einsetzungen.
```

## L1652 · `(r"String.rawx\ny${5}z", r"x\ny5z"),`

```
// `String.raw` ist die eingebaute Marke und lebt genau davon.
```

## L1654-1656 · `("var a=[];for(var k=0;k<3;k++){a.push((function(s){return s})same)} String(a[0]===a[1] && a[1]===a[2])", "true"),`

```
// **Dieselbe Stelle gibt bei jeder Auswertung DENSELBEN
// Gegenstand** (ES 13.2.8.4) — lit-html schluesselt seinen
// Zwischenspeicher damit.
```

## L1658 · `("var f=function(s){return s};String(fsame !== fsame)", "true"),`

```
// Zwei verschiedene Stellen mit demselben Text sind es NICHT.
```

## L1660 · `("function t(s){return [Object.isFrozen(s),Object.isFrozen(s.raw),Object.keys(s).join(',')].join('|')}tq${1}r", "true|`

```
// Eingefroren, und `raw` ist nicht aufzaehlbar.
```

## L1662 · `("var o={n:7,t(s){return this.n}};String(o.tq)", "7"),`

```
// Der Empfaenger gehoert zum Aufruf: `o.t`x`` ruft mit `o`.
```

## L1664-1665 · `(r"function t(s){return String(s[0])+'/'+s.raw[0]}t\xg", r"undefined/\xg"),`

```
// Eine ungueltige Flucht macht `cooked` undefined und laesst `raw`
// stehen — NUR mit Marke.
```

## L1668 · `(r"a\xgb", "SyntaxError: invalid escape sequence in template"),`

```
// OHNE Marke ist dieselbe Flucht ein Fruehfehler.
```

## L1671 · `(r"a\0b.length.toString()", "3"),`

```
// `\0` ohne Ziffer dahinter bleibt erlaubt.
```

## L1681-1699 · `#[test]`

```
/// **Ein freigegebener Funktionsknoten darf seinen Rumpf nicht vererben.**
///
/// `func_chunks` merkt sich den uebersetzten Rumpf unter der ADRESSE des
/// AST-Knotens. Eine Adresse ist aber nur solange eine Identitaet, wie
/// sie belegt ist: gibt das erste `<script>` seinen Baum frei, kann eine
/// Funktion des zweiten genau dort liegen — und bekommt dann den Rumpf
/// der ersten. Am Geraet heisst das: die Seite ruft ihre eigene Funktion,
/// und es laeuft der Code einer fremden Bibliothek.
///
/// Gefunden an Alpine.js: nach `alpine.js` scheiterte JEDER Aufruf einer
/// eigenen Funktion mit `mt is not defined` — einem Namen aus Alpines
/// Innerem. Auf dem Baumlaeufer lief derselbe Code sauber, weil der
/// diesen Zwischenspeicher nicht hat.
///
/// **Geprueft wird die Invariante, nicht ein Lauf.** Zwei Programme
/// hintereinander zu fahren und auf eine Kollision zu HOFFEN ist ein
/// Test, der beruhigt: ob der Allokator dieselbe Zelle zurueckgibt, ist
/// Zufall. Hier steht die Bedingung selbst: eine Adresse, unter der ein
/// Rumpf gemerkt ist, darf nicht neu vergeben werden.
```

## L1713 · `let _ = i.func_chunk(&f);`

```
// Uebersetzen und merken — ab jetzt haengt an dieser Adresse ein Rumpf.
```

## L1716-1717 · `for k in 0..64 {`

```
// Der Allokator gibt eine gerade freigegebene Zelle bevorzugt sofort
// wieder aus. Genau das ist die Falle.
```

## L1726-1727 · `#[test]`

```
/// Ein direktes `eval` MUSS die Wegweiser abschalten — das ist der
/// einzige Weg, auf dem eine Bindung nachtraeglich weiter innen entsteht.
```

## L1737-1745 · `#[test]`

```
/// **Ein Primitiv bekommt keine Huelle fuers Lesen — aber alles muss
/// weiterhin dasselbe antworten.**
///
/// `Interp::get` fing bis 0.120.0 mit `to_object(base)` an, und das legt
/// bei einer Zeichenkette JEDES ZEICHEN als eigene Eigenschaft an. Fuer
/// `s.indexOf(...)` wurden so bei 89 KB erst 89 000 Eintraege gebaut,
/// deren Antwort auf dem PROTOTYP liegt. Jetzt faengt die Kette beim
/// Prototyp an — und genau das prueft diese Liste: dass dabei nichts
/// verlorengeht, was ein Primitiv trotzdem koennen muss.
```

## L1755-1756 · `("String(\"abc\".hasOwnProperty(\"0\"))", "true"),`

```
// Eigene Eigenschaften hat ein Primitiv trotzdem: der Weg dorthin
// geht ueber `to_object`, und der bleibt.
```

## L1761 · `("Object.keys(new String(\"ab\")).join(\",\")", "0,1"),`

```
// Ein echtes String-OBJEKT bleibt, wie es war.
```

## L1764 · `("(function(){Object.prototype.zz=\"Z\";var v=\"ab\".zz;delete Object.prototype.zz;return v})()", "Z"),`

```
// Die Kette muss weiterlaufen, nicht beim Prototyp enden.
```

## L1766 · `("(255).toString(16)", "ff"),`

```
// Und die anderen Primitive.
```

