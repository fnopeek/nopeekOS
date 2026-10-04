# `tools/wasm/beak-engine/src/js/eval.rs` @ 5e0102684

## L1 · `use alloc::rc::Rc;`

```
//! Anweisungen und Ausdruecke auswerten.
```

## L13-15 · `pub fn exec(&mut self, st: &Stmt, env: &Rc<RefCell<Env>>) -> C<Option<Value>> {`

```
// ── Anweisungen ──────────────────────────────────────────────────────
/// Liefert den Abschlusswert, wo es einen gibt (der Wert eines Programms
/// ist der letzte Ausdruckswert — `eval` und die Konsole leben davon).
```

## L21-22 · `if self.steps & 0xFFFF == 0 { self.check_deadline()?; }`

```
// Und die Uhr. Sie stand nur in `tick`, und dort kommt reines JS nie
// vorbei — siehe `Interp::check_deadline`.
```

## L28 · `Stmt::Func(_) => Ok(None), // beim Hochziehen erledigt`

```
// beim Hochziehen erledigt
```

## L51-53 · `self.pending_labels.push(label.clone());`

```
// Den Namen fuer die Schleife darunter ablegen — sie holt ihn
// beim Betreten ab (`Interp::pending_labels`). War der Rumpf
// keine Schleife, holt ihn niemand, und er gehoert wieder weg.
```

## L58-59 · `Err(Abrupt::Break(Some(l))) if l == *label => Ok(None),`

```
// Ein `break lbl` endet GENAU hier; ein `continue lbl`
// gehoert der Schleife darunter und wird dort gefangen.
```

## L98-105 · `Stmt::With { obj, body } => {`

```
// `with (o) { … }` — eine Umgebung, deren Namen aus einem OBJEKT
// kommen. Im strengen Modus verboten; das hat schon der Parser
// abgelehnt, hier steht nur der lockere Fall.
//
// Vue braucht es: sein Vorlagen-Uebersetzer erzeugt
// `with (_ctx) { … }`, und ohne das rendert eine Vue-Seite nichts
// — sie meldet den Fehler in ihrem eigenen Behandler und laesst
// den Kasten leer.
```

## L111-114 · `self.hints_ok = false;`

```
// **Die Wegweiser gehen aus, fuer die ganze Sitzung.** Eine
// Objektumgebung kann Bindungen bekommen und verlieren,
// waehrend eine Befehlsstelle schon gelaufen ist — dieselbe
// Ueberlegung wie beim direkten `eval` (`Interp::hints_ok`).
```

## L118-119 · `Stmt::Import(_) | Stmt::ExportAll { .. } => Ok(None),`

```
// `import` und die reinen Weiterreichungen tun zur LAUFZEIT
// nichts — sie sind beim Verknuepfen erledigt (`modules.rs`).
```

## L121-124 · `Stmt::ExportNamed { decl, .. } => match decl {`

```
// Aber `export function f(){}` ist eine DEKLARATION mit einem
// `export` davor. Sie zu ueberspringen hiess: `f` gibt es im
// eigenen Modul nicht — und das fiel niemandem auf, weil auch
// der Import still war.
```

## L135-136 · `let own = match &**d {`

```
// Eine benannte Deklaration hinter `export default` fuehrt
// AUCH ihren eigenen Namen ein.
```

## L170-177 · `self.bind_pattern(&dec.id, v, env, d.kind != VarKind::Var)?;`

```
// **`var` LEGT hier nichts mehr an — das Hochziehen hat das
// getan.** Die Zeile fuehrt nur noch eine Zuweisung aus (ES
// §VariableStatement: PutValue, nicht InitializeBinding), und der
// Unterschied ist sichtbar, sobald die Bindung nicht in der Kette
// steht, sondern auf dem globalen Objekt: `init_binding` legte
// daneben eine zweite an, und `window.X` blieb `undefined`.
// `let`/`const` initialisieren dagegen wirklich — ihre Bindung
// steht in der Kette und wartet in der TDZ.
```

## L185-188 · `let mine = core::mem::take(&mut self.pending_labels);`

```
// Eigene Umgebung fuer den Kopf: `for (let i=0;;)` bindet `i` je
// Durchlauf neu, damit eine Closure im Rumpf den Wert DIESES Durchlaufs
// festhaelt. Ohne das teilen sich alle Closures dieselbe Zelle — der
// Klassiker, an dem `for (var i…)` scheitert.
```

## L239-245 · `pub fn for_head_bind(&mut self, left: &ForHead, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {`

```
/// Den Kopf einer `for..of`/`for..in`-Schleife an einen Wert binden.
///
/// Drei Faelle in einer Funktion, und beide Maschinen rufen sie: ein
/// `let`/`const` legt seinen Namen je Durchlauf NEU an (deshalb
/// `declare_pattern`), ein `var` findet den hochgezogenen weiter aussen,
/// und ein blosses Ziel (`for (x of …)`, `for ([a,b] of …)`) ist eine
/// ZUWEISUNG und legt gar nichts an.
```

## L263 · `let keys = self.for_in_keys(&obj)?;`

```
// Dieselbe Hilfe wie die Befehlsmaschine — siehe `Interp::for_in_keys`.
```

## L280-288 · `fn exec_for_of(&mut self, left: &ForHead, right: &Expr, body: &Stmt,`

```
/// `for..of` — SCHRITTWEISE, nicht erst einsammeln.
///
/// Der Unterschied ist nicht Tempo, sondern Machbarkeit: ein Iterator
/// ohne Ende (ein Generator, ein Strom) ist voellig gewoehnlich, und ein
/// Rumpf, der im ersten Durchlauf `break` sagt, muss damit umgehen. Wer
/// vorher einsammelt, haengt an genau dieser Stelle.
///
/// Und jedes vorzeitige Verlassen ruft `return()` auf dem Iterator —
/// sonst bleiben fremde `finally`-Bloecke liegen.
```

## L321-322 · `let mut start = None;`

```
// Erst den passenden Fall suchen, dann AB DORT alles laufen lassen —
// das Durchfallen ist die Regel, nicht die Ausnahme.
```

## L358-359 · `match self.exec_block(f, &fenv) {`

```
// Ein Abbruch im `finally` UEBERSCHREIBT den aus dem Rumpf — auch
// einen geworfenen Fehler. Das ist die Regel und die Falle.
```

## L368-369 · `pub fn hoist_block(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {`

```
/// `let`/`const`/`class` eines Blocks anlegen (Totzone) und
/// Funktionsdeklarationen binden.
```

## L400-416 · `pub fn name_function(&mut self, v: &Value, name: &str) {`

```
/// Eine eben angelegte Bindung auf `const` stellen. Getrennt von
/// `init_binding`, weil der Baumlaeufer die Veraenderlichkeit schon beim
/// Hochziehen setzt und die Maschine erst beim Ausfuehren dort ankommt.
/// Eine Bindung anlegen, die es GIBT, aber noch nicht bereit ist — die
/// zeitliche Totzone von `let`/`const`/`class`. Dieselbe Zeile, die
/// `hoist` schreibt; hier oeffentlich, weil die Befehlsmaschine sie beim
/// Betreten eines Blocks braucht.
/// Eine fertige Bindung GENAU HIER anlegen — ohne die Kette hochzugehen.
///
/// Der Unterschied zu `init_binding` ist der ganze Punkt: das sucht erst
/// nach einer vorhandenen Bindung und schreibt DIE. Fuer eine
/// Funktionsdeklaration in einem Block ist das falsch, und zwar
/// beobachtbar: `let f = 1; { function f(){} }` darf das aeussere `f`
/// nicht anfassen (annexB B.3.3 nimmt die var-Bindung genau dann zurueck,
/// wenn sie einen frueheren Fehler ausloeste). Sieben Tests.
/// Einer eben gebauten anonymen Funktion den Namen geben, unter dem sie
/// gerade gebunden wird. Sichtbar in Stapelspuren und in `f.name`.
```

## L454-461 · `pub fn declare_pattern(&mut self, p: &Pat, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {`

```
// ── Muster binden ────────────────────────────────────────────────────
/// Die Namen eines Musters HIER anlegen und dann binden.
///
/// Der Unterschied zu `bind_pattern(…, true)` ist der Ort: `init_binding`
/// laeuft die Umgebungskette HOCH und faende eine gleichnamige Bindung
/// weiter aussen. Der Kopf eines `catch` und der einer `for`-Schleife
/// legen ihre Namen aber GENAU HIER an, je Durchlauf neu. Eigene
/// Funktion, weil beide Maschinen sie brauchen.
```

## L485-486 · `if let Pat::Ident(n) = &**left {`

```
// `var [a = () => {}] = []` nennt den Pfeil `a` — dieselbe
// Regel wie `var f = function(){}`, nur eine Ebene tiefer.
```

## L538 · `self.assign_to_expr(e, v, env)`

```
// `[a.b] = x` — Ziel ist eine Eigenschaft, keine Bindung.
```

## L544-548 · `pub fn assign_ident(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {`

```
/// `PutValue` auf einen Bezeichner (ES §6.2.5.6).
///
/// **Die einzige Fassung.** Bis 0.98.0 stand dieselbe Regel ein zweites
/// Mal in `expr.rs::store`, und die zweite hat beim strengen Modus sofort
/// anders entschieden — [[feedback-a-copy-is-a-second-semantics-waiting]].
```

## L553-556 · `pub fn with_target(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Option<Gc>> {`

```
/// Das `with`-Objekt, das diesen Namen traegt — oder `None`.
///
/// Die Kette wird nur bis zur ersten gewoehnlichen Bindung desselben
/// Namens abgelaufen: eine INNERE `var` verschattet ein aeusseres `with`.
```

## L572-573 · `pub fn assign_ident_depth(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>)`

```
/// Wie `assign_ident`, sagt aber MIT, in welcher Tiefe der Name stand.
/// Nur der Wegweiser braucht das; siehe `Chunk::hints`.
```

## L576-578 · `if let Some(o) = self.with_target(n, env)? {`

```
// Ein `with` kann den Namen tragen, und dann wird in das OBJEKT
// geschrieben. Die Kette wird dafuer einmal abgelaufen — nur, wenn
// ueberhaupt eine Objektumgebung darin steht.
```

## L584-586 · `if e.borrow().imports.as_ref().is_some_and(|m| m.contains_key(n)) {`

```
// Auf einen importierten Namen zu schreiben ist ein Fehler, kein
// Schreiben ins Herkunftsmodul: die Bindung dort gehoert dem
// Modul, das sie ausfuehrt.
```

## L600-603 · `let g = self.realm.global.clone();`

```
// Nicht in der Bindungskette — aber das GLOBALE OBJEKT ist auch ein
// Bindungsort: `Object = 12` schreibt dort und ist auch im strengen
// Modus erlaubt. Unaufloesbar ist ein Name erst, wenn ihn auch das
// globale Objekt nicht kennt.
```

## L609-610 · `super::interp::strict_site!(self, 6);`

```
// Jetzt ist er wirklich unbekannt: im lockeren Modus wird eine globale
// Eigenschaft daraus, im strengen wirft es.
```

## L641-642 · `pub fn names_of(p: &Pat, out: &mut Vec<String>) {`

```
/// Namen eines Musters — dieselbe Liste wie beim Hochziehen, hier fuer die
/// Totzone eines Blocks.
```

