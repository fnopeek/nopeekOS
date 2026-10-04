# `tools/wasm/beak-engine/src/js/code.rs` @ 5e0102684

## L1-22 · `use alloc::rc::Rc;`

```
//! Der Befehlssatz, in den ein Programm uebersetzt wird — und die Einheit, in
//! der er liegt (`Chunk`).
//!
//! **Warum es das gibt.** Ein Baumlaeufer benutzt den RUST-Stapel als
//! Zustandsspeicher, und ein Rust-Stapel laesst sich nicht anhalten. Damit
//! sind Generatoren, `async`/`await` und alles, was spaeter einmal mitten in
//! einem Ausdruck stehenbleiben soll, nicht bloss ungebaut, sondern
//! unbaubar — man kaeme an den Zustand nicht heran. Die Befehlsliste dreht
//! das um: der Zustand ist ein Feld, das man wegspeichern und weiterlaufen
//! lassen kann.
//!
//! Der Kopf von `interp.rs` hat diesen Umbau vorgesehen und eine Bedingung
//! daran geknuepft: „der test262-Lauf ist danach das Netz, mit dem eine
//! Umstellung auf Bytecode ueberhaupt erst verantwortbar ist." Das Netz gibt
//! es (52,77 %, Fehlerkarte nach Familien, 45 s je Lauf), also wird sie
//! eingeloest.
//!
//! **Was hier NICHT passiert: eine zweite Semantik.** Jeder Befehl unten ruft
//! dieselben Hilfen wie der Baumlaeufer (`Interp::binary`, `Interp::call`,
//! `Value::truthy`, `Env`). Die Maschine tauscht den VERTEILER, nicht die
//! Bedeutung — und solange ein Programm entweder ganz uebersetzt oder ganz
//! vom Baumlaeufer gefahren wird, kann auch keine Mischung entstehen.
```

## L31-33 · `#[derive(Debug, Clone)]`

```
/// Ein Befehl. Sprungziele sind absolute Indizes in `Chunk::ops` — relative
/// waeren beim Zurueckflicken (`patch`) eine zweite Rechnung, und die erste
/// ist schon fehleranfaellig genug.
```

## L36 · `Const(u32),`

```
/// `constants[i]` auf den Stapel.
```

## L38 · `LoadVar(u32),`

```
/// Den Wert des Namens `names[i]` auf den Stapel.
```

## L40-41 · `StoreVar(u32),`

```
/// Oben zuweisen an `names[i]`; der Wert BLEIBT auf dem Stapel (eine
/// Zuweisung ist ein Ausdruck).
```

## L43-49 · `DeclVar { name: u32, mutable: bool, lexical: bool },`

```
/// `names[i]` binden. Nimmt den Wert vom Stapel.
///
/// `lexical` unterscheidet die beiden Faelle, und der Unterschied ist
/// beobachtbar: ein `let`/`const` gehoert GENAU HIER hin, ein `var` in die
/// naechste Funktionsumgebung — die das Hochziehen schon angelegt hat, und
/// die kann weiter oben liegen. `for (const x = 1; …)` hat den Fehler
/// gezeigt: die Zuweisung fand das aeussere `x`.
```

## L51-53 · `NameFunc(u32),`

```
/// Einer eben gebauten Funktion den Namen der Variablen geben, an die sie
/// gerade gebunden wird (`var f = function(){}` -> `f.name === "f"`).
/// Sichtbar in Stapelspuren und in `f.name`; sechs Tests.
```

## L55-57 · `ToKey,`

```
/// Oben in einen Eigenschaftsschluessel wandeln — VOR der Auswertung des
/// Wertes, weil `ToPropertyKey` Nebenwirkungen haben kann und die Spec
/// ihre Reihenfolge festlegt.
```

## L59 · `This,`

```
/// `this`.
```

## L63 · `Swap,`

```
/// Oben verwerfen und darunter liegenden Wert behalten (`a, b` → a weg).
```

## L66-67 · `ToNumeric,`

```
/// `ToNumeric` — wie `Un(Plus)`, aber eine grosse Zahl bleibt gross.
/// `x++` auf einem BigInt darf nicht in `+x` laufen, das wirft.
```

## L69 · `Step(bool),`

```
/// Eins dazu oder eins weg, im TYP des Wertes (`true` = dazu).
```

## L72-73 · `TypeofVar(u32),`

```
/// `typeof x` auf einem NAMEN — muss ohne ReferenceError auskommen, wenn
/// es den Namen nicht gibt, und ist deshalb kein `LoadVar` + `Un`.
```

## L76 · `JumpTrue(u32),`

```
/// Springt, wenn oben TRUTHY ist; nimmt den Wert immer vom Stapel.
```

## L78 · `JumpFalse(u32),`

```
/// Springt, wenn oben falsy ist; nimmt den Wert IMMER vom Stapel.
```

## L80-81 · `JumpFalseKeep(u32),`

```
/// Fuer `&&`/`||`/`??`: springt bei falsy/truthy/nullish und LAESST den
/// Wert liegen — das ist der Wert des Ausdrucks.
```

## L85 · `GetProp(u32),`

```
/// `obj[names[i]]` — Stapel: obj → wert.
```

## L87 · `GetIndex,`

```
/// `obj[key]` — Stapel: obj, key → wert.
```

## L89 · `SetProp(u32),`

```
/// Stapel: obj, wert → wert (die Zuweisung ist ein Ausdruck).
```

## L91 · `SetIndex,`

```
/// Stapel: obj, key, wert → wert.
```

## L93-97 · `Call { argc: u16, name: u32 },`

```
/// Stapel: callee, this, arg0..argN → ergebnis.
///
/// `name` ist der NAME des Gerufenen, nur fuer die Fehlermeldung —
/// `u32::MAX` heisst „keiner". „value is not a function" sagt nicht, WAS
/// fehlt, und genau das ist im Zielkorpus der haeufigste Fehlschlag.
```

## L99-103 · `New { argc: u16, name: u32 },`

```
/// Stapel: callee, arg0..argN → ergebnis.
/// Der Name des Gerufenen faehrt MIT — nicht fuer das Bauen, sondern fuer
/// den Fehlschlag: „Intl.PluralRules is not a constructor" sagt, was
/// fehlt, „undefined is not a constructor" nicht. `u32::MAX` heisst
/// namenlos (ein Ausdruck statt eines Namens).
```

## L105-107 · `MakeArray(u16),`

```
/// Feld aus den obersten `n` Werten. `true` an Stelle `k` heisst: dort
/// stand eine LUECKE (`[1, , 3]`), und die ist nicht dasselbe wie
/// `undefined` — `in` findet sie nicht.
```

## L109 · `NewObject,`

```
/// Ein leerer Gegenstand mit `Object.prototype`.
```

## L111 · `DefineProp(u32),`

```
/// Stapel: obj, wert → obj. Eine Dateneigenschaft unter `names[i]`.
```

## L113-114 · `SetLiteralProto,`

```
/// `{ __proto__: v }` — setzt den PROTOTYP des Objekts auf dem Stapel,
/// statt eine Eigenschaft anzulegen.
```

## L116 · `DefinePropComputed,`

```
/// Stapel: obj, schluessel, wert → obj.
```

## L118-119 · `DefineAccessor { name: u32, get: bool },`

```
/// Stapel: obj, funktion → obj. `get` unterscheidet Leser von Schreiber;
/// beide muessen sich auf DERSELBEN Eigenschaft treffen koennen.
```

## L122-123 · `SpreadInto,`

```
/// Stapel: obj, quelle → obj. `{...src}` kopiert die aufzaehlbaren
/// EIGENEN Eigenschaften.
```

## L125-126 · `Dup2,`

```
/// Die obersten ZWEI verdoppeln — fuer `o[k]++`, wo Objekt und Schluessel
/// nur EINMAL ausgewertet werden duerfen.
```

## L128-129 · `Rot3,`

```
/// `[a, b, c]` → `[c, a, b]`. Fuer `o.p++`, wo der ALTE Wert das Ergebnis
/// ist und trotzdem unter dem Objekt hindurch nach unten muss.
```

## L131-132 · `Rot4,`

```
/// `[a, b, c, d]` → `[d, a, b, c]`. Dasselbe fuer `o[k]++`: dort liegen
/// Objekt UND Schluessel darueber.
```

## L134 · `Regex { body: u32, flags: u32 },`

```
/// Ein regulaerer Ausdruck aus `names[body]` und `names[flags]`.
```

## L136-138 · `TemplateObject(u32),`

```
/// Der Vorlagen-Gegenstand eines getaggten Templates (ES 13.2.8.4).
/// Gebaut wird er EINMAL je Befehlsstelle — `Interp::template_object`
/// haelt ihn unter der Adresse dieses Eintrags fest.
```

## L140-142 · `IterAllAsync,`

```
/// `for await (… of x)`: den ASYNCHRONEN Iterator holen (ES 7.4.2 mit
/// `hint: async`). Hat `x` kein `Symbol.asyncIterator`, wird sein
/// gewoehnlicher genommen und als synchron vermerkt.
```

## L144-147 · `IterNextAsyncCall,`

```
/// `next()` rufen und das legen, worauf gewartet werden muss: bei einem
/// echten async-Iterator das ganze Ergebnis, bei einem umgehuellten
/// synchronen nur dessen `value` (sein `done` steht schon fest und wird
/// am Iterator vermerkt).
```

## L149-150 · `IterStepAsync(u32),`

```
/// Nach dem `Op::Await`: das abgewartete Ergebnis auswerten und
/// entweder den Wert legen oder ans Schleifenende springen.
```

## L152 · `Concat(u16),`

```
/// Die obersten `n` Werte zu einer Zeichenkette verketten (Vorlage).
```

## L154 · `DeleteProp(u32),`

```
/// `delete obj[names[i]]` bzw. `delete obj[key]`.
```

## L156-157 · `PrivateIn(u32),`

```
/// `#x in obj` — die Markenpruefung. Der Name steht in der Namenstabelle,
/// das Objekt auf dem Stapel.
```

## L160-161 · `MakeArraySpread { n: u16, spread: u32 },`

```
/// Ein Feld aus den obersten `n` EINTRAEGEN bauen, von denen jeder
/// entweder ein Wert oder ein zu spreizender ist (`spread[k]`).
```

## L163-164 · `CallSpread(u32),`

```
/// Wie `Call`/`New`, aber die Argumente stehen als FELD auf dem Stapel —
/// so kann `f(...xs)` dieselbe Aufrufhilfe benutzen.
```

## L167 · `SuperGet(u32),`

```
/// `super.k` lesen — Stapel: → wert. `Interp::super_get`.
```

## L169-171 · `SuperCallee(u32),`

```
/// `super.k` als Gerufenes — Stapel: → wert, this. Der Empfaenger ist das
/// EIGENE `this`, der Wert kommt von oben; genau diese Trennung ist der
/// Sinn von `super`.
```

## L173-175 · `SuperCall(u16),`

```
/// `super(...)` — Stapel: arg0..argN → undefined. `Interp::super_call`
/// macht alles: Elternkonstruktor suchen, auf DIESEM `this` fahren, und
/// danach die eigenen Instanzfelder anlegen.
```

## L177-178 · `JumpNullishTo(u32),`

```
/// Springt, wenn oben `null`/`undefined` ist, und laesst den Wert liegen.
/// Das Gegenstueck zu `JumpNullishKeep`, fuer die Optional-Kette.
```

## L180 · `Closure(u32),`

```
/// Aus `names[i]` eine Funktion bauen — der Index zeigt in `funcs`.
```

## L182-188 · `BindPat { pat: u32, mode: BindMode },`

```
/// Ein MUSTER binden — Stapel: wert → (nichts). Der Index zeigt in `pats`.
///
/// Wie `Op::Class` rechnet der Befehl nichts, er ruft `bind_pattern` bzw.
/// `declare_pattern` — dieselben Hilfen wie der Baumlaeufer. Ein Muster
/// ist eine Bauvorschrift mit Voreinstellungen, Restsammlern, geschachtelten
/// Mustern und Zielen, die gar keine Bindungen sind (`[a.b] = x`); ein
/// Nachbau davon waere eine zweite Zuweisungssemantik.
```

## L190-191 · `BindHead(u32),`

```
/// Den Kopf einer `for..of`/`for..in`-Schleife binden — Stapel: wert →
/// (nichts). `Interp::for_head_bind` kennt die drei Faelle.
```

## L193-206 · `Class(u32),`

```
/// Eine Klasse bauen — der Index zeigt in `classes`.
///
/// **Der Befehl rechnet nichts, er RUFT `Interp::eval_class`** — dieselbe
/// Funktion, die der Baumlaeufer ruft. Eine Klasse ist kein Ausdruck mit
/// Unterausdruecken, den man in Befehle zerlegen wollte: sie ist eine
/// Bauvorschrift mit einem Dutzend Sonderregeln (fehlender Konstruktor,
/// abgeleiteter Durchreicher, Methoden NICHT aufzaehlbar, Leser und
/// Schreiber auf DERSELBEN Eigenschaft). Sie ein zweites Mal zu schreiben
/// waere die teuerste Sorte zweiter Semantik.
///
/// Ihre Unterausdruecke — `extends`, berechnete Schluessel, statische
/// Felder — laufen dadurch im Baumlaeufer, und zwar in BEIDEN Faellen.
/// Das ist kein Bruch der Regel „ganz oder gar nicht", sondern ihre
/// strengste Lesart: fuer einen Klassenrumpf gibt es genau EINEN Weg.
```

## L209-210 · `Rethrow,`

```
/// Den Wert oben werfen — der Rueckweg aus einem `finally`, das nicht
/// gefangen hat.
```

## L212-218 · `TryStart { catch: u32, finally: u32 },`

```
/// Einen Behandler aufmachen. `catch`/`finally` sind Sprungziele,
/// `u32::MAX` heisst „gibt es nicht".
///
/// Der Behandler merkt sich AUCH die Stapel- und Umgebungstiefe: ein Wurf
/// mitten in einem Ausdruck laesst halbe Werte liegen, und ohne das
/// Zurueckschneiden faende der `catch`-Block einen Stapel vor, den niemand
/// gebaut hat.
```

## L220 · `TryEnd,`

```
/// Den obersten Behandler wieder zumachen.
```

## L222 · `BindCatch(u32),`

```
/// Die geworfene Sache in `names[i]` binden — der Kopf eines `catch`.
```

## L224-230 · `IterAll,`

```
/// Den Iterator des Wertes oben holen und im Rahmen ablegen.
///
/// FAUL, ueber `get_iterator`/`iter_next`/`iter_close` — dieselben Hilfen
/// wie im Baumlaeufer. Die Werte vorher einzusammeln waere kuerzer und
/// falsch: ein Rumpf, der die Quelle veraendert, muss das sehen, und ein
/// vorzeitiger Ausstieg muss `return()` rufen. Fuenf Tests haben genau
/// das gesagt.
```

## L232 · `IterNext(u32),`

```
/// Den naechsten Wert auf den Stapel; ist der Iterator zu Ende, springen.
```

## L234-241 · `ForInAll,`

```
/// Die Schluessel eines `for…in` holen und im Rahmen ablegen — Stapel:
/// obj → (nichts).
///
/// EIFRIG, ueber `Interp::for_in_keys`, dieselbe Hilfe wie im
/// Baumlaeufer. Anders als bei `for…of` ist das richtig: die Liste wird
/// vorher gebaut, damit eine Aenderung am Objekt die Schleife nicht ins
/// Rutschen bringt. `null`/`undefined` geben eine leere Liste, also
/// null Umlaeufe statt eines Fehlers.
```

## L243 · `ForInNext(u32),`

```
/// Den naechsten Schluessel auf den Stapel; ist die Liste leer, springen.
```

## L245 · `IterDrop,`

```
/// Den Iterator vergessen — er ist zu Ende, `return()` waere falsch.
```

## L247-248 · `IterClose,`

```
/// Den Iterator SCHLIESSEN (`return()`) und vergessen — der Weg fuer
/// `break` und fuer jeden Abbruch.
```

## L250 · `Ret,`

```
/// Aus dem Rahmen zurueck; oben liegt der Wert.
```

## L252-260 · `Yield,`

```
/// **Anhalten.** Oben liegt der Wert, den `next()` zurueckgibt; der Rahmen
/// bleibt stehen, wo er steht.
///
/// Beim Wiederaufnehmen legt `Vm::send` den Wert von `next(v)` an genau
/// dieselbe Stelle des Stapels — und der ist der Wert des
/// `yield`-Ausdrucks. Mehr ist ein `yield` nicht: der halbe Ausdruck
/// darunter (`a + (yield 1)` hat `a` liegen) steht im Wertestapel des
/// Rahmens und ueberlebt das Anhalten, weil er ein FELD ist und kein
/// Rust-Stapel. Das ist die ganze Begruendung des Umbaus, eingeloest.
```

## L262-264 · `YieldDelegate(bool),`

```
/// Der Anhaltepunkt eines `yield*` im gewoehnlichen Generator: gibt das
/// Ergebnisobjekt des INNEREN Iterators unveraendert heraus und ist die
/// Marke, an der `throw()`/`return()` weiterreichen statt abzuwickeln.
```

## L266-267 · `DelegateStart(bool),`

```
/// `yield* x`: den inneren Iterator holen (synchron oder asynchron, je
/// nach Art des Generators) und den ersten „erhaltenen" Wert legen.
```

## L269-272 · `DelegateCall(u32),`

```
/// Den inneren Iterator anstossen — mit `next`, `throw` oder `return`, je
/// nachdem, womit der aeussere Generator wieder angeworfen wurde. Der
/// Sprung geht ans Schleifenende, wenn der innere Iterator kein `return`
/// hat und der aeussere aufgeben soll.
```

## L274-275 · `DelegateStep { end: u32, is_async: bool },`

```
/// Das (ggf. abgewartete) Ergebnis auswerten: fertig → ans Ziel springen,
/// sonst den Wert fuers `yield` legen.
```

## L277-283 · `Await,`

```
/// **Warten.** Oben liegt das Erwartete; die Maschine haelt an, und was
/// sie wieder anwirft, ist die Aufloesung des Versprechens.
///
/// Derselbe Mechanismus wie `Yield` — nur legt sich hier ein Versprechen
/// davor, und das Wiederaufnehmen kommt aus der Microtask-Schlange statt
/// von einem `next()`. Genau das meinte der Bauplan mit „`async`/`await`
/// ist derselbe Mechanismus mit einem Promise davor".
```

## L285-288 · `PushEnv(u32),`

```
/// Eine Umgebung fuer einen Block oeffnen — mit den Bindungen, die dort
/// HOCHGEZOGEN gehoeren (`blocks[i]`). Ohne sie steht `let` erst ab seiner
/// Zeile, statt von Blockanfang an in der zeitlichen Totzone, und eine
/// Funktionsdeklaration im Block gaebe es vor ihrer Zeile gar nicht.
```

## L291-292 · `SetCompletion,`

```
/// Das Ergebnis des Programms merken (der Wert eines Programms ist sein
/// letzter Ausdruckswert — `eval` und die Konsole leben davon).
```

## L296 · `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`

```
/// Wie ein Muster gebunden wird.
```

## L299-300 · `Init,`

```
/// Eine Deklaration: die Bindung gibt es schon (das Hochziehen hat sie
/// angelegt), hier wird sie fertig.
```

## L302 · `Assign,`

```
/// Eine Zuweisung an bestehende Ziele — auch an Eigenschaften.
```

## L304 · `Declare,`

```
/// Der Kopf eines `catch`: die Namen entstehen GENAU HIER.
```

## L308-313 · `#[derive(Debug, Clone)]`

```
/// Was beim Betreten eines Blocks gebunden wird, bevor die erste Zeile laeuft.
///
/// Die Liste entsteht beim Uebersetzen aus denselben zwei Schleifen wie
/// `Interp::hoist` — dieselbe Reihenfolge, dieselben Faelle. Sie hier
/// nachzubauen statt den AST mitzuschleppen kostet zwei Zahlen je Bindung
/// statt einer Kopie des ganzen Rumpfes.
```

## L316-317 · `Tdz { name: u32, mutable: bool },`

```
/// `let`/`const`/`class`: gebunden, aber NICHT bereit — die zeitliche
/// Totzone. Ohne sie ist `let` nur ein `var` mit anderem Namen.
```

## L319-320 · `Func { name: u32, func: u32 },`

```
/// Eine Funktionsdeklaration: sofort fertig, damit sie vor ihrer Zeile
/// aufrufbar ist.
```

## L324-325 · `pub const HINT_NONE: u16 = u16::MAX;`

```
/// „Noch kein Hinweis." Kein `Option`, weil das je Befehlsstelle ein Byte
/// mehr waere und die Stelle heiss ist.
```

## L328 · `pub struct Chunk {`

```
/// Uebersetzter Code samt allem, worauf seine Befehle zeigen.
```

## L331-340 · `pub hints: Vec<core::cell::Cell<u16>>,`

```
/// **Der Weg von letztem Mal, je Befehlsstelle.** Gemessen an der
/// Fritzbox-Anmeldung: 24,4 % aller Befehle sind `LoadVar`, und 85,4 %
/// davon finden ihren Namen DREI Umgebungen weiter oben. Der volle Weg
/// fragt dafuer vier Tabellen — drei davon nur, um „nicht hier" zu
/// hoeren.
///
/// Der Hinweis ist die Tiefe, in der der Name beim letzten Mal stand.
/// Sie ist LEXIKALISCH festgelegt: dieselbe Befehlsstelle sieht dieselbe
/// Kettenform. Trifft er trotzdem nicht, laeuft der volle Weg — er ist
/// ein Hinweis, keine Auskunft.
```

## L343 · `pub names: Vec<Rc<str>>,`

```
/// Namen (Bezeichner und Eigenschaften), einmal abgelegt statt je Befehl.
```

## L346-348 · `pub templates: Vec<Vec<super::ast::TemplateElement>>,`

```
/// Die Stuecke jedes getaggten Templates. Sie liegen HIER und nicht im
/// Baum, weil ein `Chunk` den Baum ueberlebt — und die Adresse dieses
/// Eintrags ist der Schluessel, unter dem der Gegenstand gemerkt wird.
```

## L354 · `pub blocks_spread: Vec<Vec<bool>>,`

```
/// Je `MakeArraySpread` eine Maske: welcher Eintrag war ein `...x`?
```

## L367-368 · `self.hints.push(core::cell::Cell::new(HINT_NONE));`

```
// Genau hier, damit die beiden Listen nicht auseinanderlaufen
// koennen — `ops` waechst nirgends sonst.
```

## L373 · `pub fn emit_jump(&mut self, make: fn(u32) -> Op) -> usize {`

```
/// Einen noch unbekannten Sprung eintragen und seine Stelle zurueckgeben.
```

## L378 · `pub fn patch(&mut self, at: usize) {`

```
/// Das Ziel eines vorgemerkten Sprungs auf HIER setzen.
```

## L395-396 · `pub fn name(&mut self, s: &str) -> u32 {`

```
/// Namen werden dedupliziert: eine Schleife, die `i` zwanzigmal liest,
/// legt ihn einmal ab.
```

## L445-449 · `#[derive(Debug)]`

```
/// Was der Uebersetzer noch nicht kann. **Kein Fehler, eine Absage** — der
/// Rufer faehrt das Programm dann GANZ mit dem Baumlaeufer, nie halb.
///
/// Der Text ist der Name der Form, nicht ein Satz: er wird gezaehlt, und eine
/// Zaehlung braucht einen Schluessel, keine Prosa.
```

## L455 · `impl core::fmt::Display for Unsupported {`

```
/// Damit ein Zaehler ueber viele Laeufe etwas sagt.
```

