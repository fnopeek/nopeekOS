# `tools/wasm/beak-engine/src/js/compile.rs` @ 5e0102684

## L1-10 · `use alloc::string::String;`

```
//! AST → Befehlsliste.
//!
//! **Der Uebersetzer sagt NEIN, wo er noch nicht kann** (`Unsupported`), und
//! der Rufer faehrt das Programm dann ganz mit dem Baumlaeufer. Das ist die
//! wichtigste Eigenschaft dieses Umbaus: es gibt zwei Maschinen, aber nie fuer
//! DASSELBE Programm. Eine Mischung waere ein zweiter Semantikpfad, und die
//! laufen erfahrungsgemaess still auseinander.
//!
//! Die Absage-Namen sind Schluessel, keine Saetze: `test262` zaehlt sie, und
//! die Rangliste sagt, was als naechstes uebersetzbar werden muss.
```

## L20 · `struct Loop {`

```
/// Wohin `break`/`continue` springen. Ein Eintrag je offener Schleife.
```

## L22 · `breaks: Vec<usize>,`

```
/// Stellen, die auf das Ende der Schleife gepatcht werden.
```

## L24 · `continues: Vec<usize>,`

```
/// Stellen, die auf den Fortsetzungspunkt gepatcht werden.
```

## L26-31 · `depth: usize,`

```
/// Wieviele Umgebungen beim Betreten offen waren.
///
/// Ein `break` aus einem Block heraus springt an dessen `PopEnv` VORBEI.
/// Ohne diese Zahl bleibt die Umgebung offen, der naechste `PopEnv` nimmt
/// die falsche, und eine Bindung aus dem Block ist danach draussen noch
/// zu sehen — sechs annexB-Tests, die genau das pruefen.
```

## L33-34 · `labels: Vec<String>,`

```
/// Der Name, unter dem `break lbl` / `continue lbl` diesen Ausgang
/// findet. `None` heisst: nur ueber das unbenannte `break` erreichbar.
```

## L36-42 · `iters: usize,`

```
/// Wieviele `for…of`/`for…in` beim Betreten offen waren.
///
/// Dieselbe Buchhaltung wie `depth` fuer die Umgebungen, und aus demselben
/// Grund: ein `break lbl`/`continue lbl` springt an den `IterClose` der
/// INNEREN Schleife vorbei. Dann bleibt deren Iterator im Rahmen liegen —
/// ungeschlossen, und der naechste `IterNext` der aeusseren Schleife
/// findet den falschen. Ein test262-Fall hat genau das gesagt.
```

## L44-47 · `brk_only: bool,`

```
/// Ein `switch` ist BRECHBAR, aber nicht fortsetzbar: `break` gehoert ihm,
/// `continue` der Schleife darunter. Ohne diese Unterscheidung liefe ein
/// `continue` in einem `switch` innerhalb einer Schleife an den Anfang des
/// `switch` — und das ist eine Endlosschleife, kein Fehler, den man sieht.
```

## L54 · `depth: usize,`

```
/// Wieviele `PushEnv` gerade offen sind.
```

## L56 · `iters: usize,`

```
/// Wieviele Schleifeniteratoren gerade offen sind.
```

## L58-63 · `in_gen: bool,`

```
/// Uebersetzen wir gerade den Rumpf eines GENERATORS? Nur dort ist ein
/// `yield` ein `Op::Yield`; in einem Pfeil INNERHALB eines Generators
/// liest unser Parser `yield` ebenfalls als Yield-Ausdruck, und dessen
/// Rumpf ist ein eigener Chunk mit `in_gen == false` — der sagt hier nein
/// und faellt auf den Baumlaeufer zurueck, statt einen `Op::Yield` in
/// einen Rahmen zu legen, der ihn nicht anhalten kann.
```

## L65-67 · `in_async: bool,`

```
/// Uebersetzen wir gerade den Rumpf einer ASYNC-Funktion? Dieselbe
/// Ueberlegung wie bei `in_gen`: ein `await` im Rumpf eines gewoehnlichen
/// Pfeils darin ist ein eigener Chunk, der hier nein sagt.
```

## L69-76 · `chains: Vec<Vec<usize>>,`

```
/// Offene Ausgaenge einer Optional-Kette (`a?.b.c`).
///
/// Der Kurzschluss gehoert der GANZEN Kette, nicht dem einen Glied:
/// `a?.b.c` gibt `undefined`, wenn `a` fehlt — es fasst `.c` gar nicht
/// erst an. `Expr::Chain` macht die Klammer auf, jedes `?.` darin traegt
/// hier seinen Sprung ein, und beim Zumachen zeigen alle auf dasselbe
/// Ende. Jeder Sprung raeumt VORHER seinen eigenen Stapel ab, damit das
/// Ende nicht wissen muss, wieviel darunter lag.
```

## L78-83 · `pending_labels: Vec<String>,`

```
/// Der Name, den die naechste Schleife bekommt.
///
/// `outer: for (…)` ist im Baum eine Marke UM eine Schleife, in der
/// Maschine aber gehoert der Name der SCHLEIFE — nur sie weiss, wohin ein
/// `continue outer` springt. Also legt die Marke ihn hier ab und die
/// Schleife nimmt ihn beim Anlegen mit.
```

## L85-92 · `fin: usize,`

```
/// Wieviele `finally` gerade anhaengig sind.
///
/// Ein `yield` darunter ist ABGELEHNT, und zwar aus demselben Grund wie
/// `return` darunter: `gen.return()` an so einer Stelle muss den
/// Finalisierer noch fahren, und der Finalisierer wird hier KOPIERT statt
/// angesprungen — es gibt keine Stelle, an die ein zwischengespeicherter
/// Abschluss zurueckkaeme. Halb gebaut waere schlimmer als abgelehnt; im
/// Korpus kostet es 60 Dateien.
```

## L96-101 · `pub fn function(f: &Func) -> CompileResult<Chunk> {`

```
/// Einen FUNKTIONSRUMPF uebersetzen.
///
/// Unterschied zum Programm: kein Abschlusswert (eine Funktion ohne `return`
/// gibt `undefined`), und am Ende steht ein `Ret`, das genau das tut.
/// Parameter und `this` liegen schon in der Umgebung, die `Interp::call_env`
/// gebaut hat — der Rumpf faengt beim ersten Statement an.
```

## L103-107 · `let mut c = Compiler { chunk: Chunk::new(), loops: Vec::new(), depth: 0, iters: 0,`

```
// Ein async-Generator ist BEIDES auf einmal: er haelt an `yield` UND an
// `await` an, und `next()` gibt ein Versprechen zurueck. `in_gen` und
// `in_async` stehen deshalb beide — die Maschine kennt beide Anhaltegruende
// laengst (`Step::Yield`, `Step::Await`), der Vertrag darum herum steht in
// `generator.rs`.
```

## L119 · `pub fn program(prog: &Program) -> CompileResult<Chunk> {`

```
/// Ein ganzes Programm uebersetzen. `Err` heisst: der Baumlaeufer macht es.
```

## L123-125 · `for st in &prog.body {`

```
// Hochziehen bleibt beim Baumlaeufer (`Interp::hoist`) — es arbeitet auf
// der Umgebung, nicht auf dem Code, und ist damit fuer beide Maschinen
// dasselbe. Hier nur der Rumpf.
```

## L133-140 · `pub(crate) fn dotted_name(e: &Expr) -> Option<alloc::string::String> {`

```
/// Der Name eines Gerufenen, wenn er eine Punktkette aus Bezeichnern ist:
/// `Foo`, `Intl.PluralRules`, `window.Intl.ListFormat`.
///
/// **Die drei Stufen sind kein Luxus.** Ein Buendel schreibt
/// `new window.Intl.ListFormat(…)`, und mit nur zwei Stufen blieb die
/// Meldung namenlos — genau in dem Fall, fuer den sie da ist.
/// Alles andere (`t[n]`, `(0, o.X)`) hat keinen Namen, und dann ist
/// `u32::MAX` die ehrliche Antwort.
```

## L147 · `if head.matches('.').count() >= 2 { return None }`

```
// Drei Glieder reichen; laenger sagt eine Meldung nichts mehr.
```

## L158-161 · `fn stmt_no_completion(&mut self, st: &Stmt) -> CompileResult<()> {`

```
// ── Anweisungen ──────────────────────────────────────────────────────
/// Wie `stmt`, aber ohne Abschlusswert. In einer FUNKTION gibt es keinen —
/// ihr Wert ist ihr `return`, und ein `SetCompletion` je Anweisung waere
/// Arbeit fuer nichts.
```

## L176 · `Stmt::Expr(e) => {`

```
// Der Wert eines Programms ist sein letzter Ausdruckswert.
```

## L249-252 · `let empty = self.chunk.block(Vec::new());`

```
// Eine eigene Umgebung, damit `for (let i …)` seinen Zaehler
// nicht in den umgebenden Block schreibt. Dass jeder Umlauf
// eine FRISCHE Bindung bekaeme (die Schliessungsfalle), kann
// diese Fassung noch nicht — deshalb sagt sie unten nein.
```

## L315-317 · `let Some(k) = self.loops.iter().rposition(|l| !l.brk_only) else {`

```
// Ein `switch` faengt kein `continue` — das gehoert der
// Schleife darunter, und der Weg dorthin fuehrt durch die
// Umgebung des `switch` hindurch.
```

## L343-344 · `Stmt::Func(_) => Ok(()),`

```
// Funktionsdeklarationen erledigt das Hochziehen, genau wie beim
// Baumlaeufer — hier ist nichts zu tun.
```

## L346-348 · `Stmt::Labeled { label, body } => {`

```
// Eine Marke vor einer SCHLEIFE gehoert der Schleife (nur sie hat
// einen Fortsetzungspunkt); vor allem anderen ist sie selbst ein
// Ausgang, den nur ein `break lbl` trifft.
```

## L350-353 · `let mut labels = alloc::vec![label.clone()];`

```
// **Eine KETTE von Marken gehoert ganz der Schleife darunter.**
// `a: b: for (…)` traegt beide, und `continue a` ist gueltig.
// Der erste Entwurf gab nur die innerste weiter und lehnte
// `continue a` ab — node sagte prompt etwas anderes.
```

## L367-368 · `self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),`

```
// Sonst ist die Marke selbst der Ausgang — nur ein `break lbl`
// trifft sie, ein `continue` braucht einen Fortsetzungspunkt.
```

## L387-388 · `let Some(k) = self.loops.iter().rposition(`

```
// Ein `continue` braucht einen Fortsetzungspunkt, den hat nur
// eine Schleife — eine Marke vor einem Block ist keiner.
```

## L403-406 · `if !self.in_async { return Err(Unsupported("for-await-outside-async")) }`

```
// `for await` haelt MITTEN in der Schleife an — der
// Parser laesst es nur im async-Kontext zu, aber ein
// Pfeil darin ist ein eigener Chunk und muss hier nein
// sagen, genau wie bei `await` selbst.
```

## L413-415 · `Stmt::Class(c) => {`

```
// Eine Klassen-DEKLARATION: bauen und an ihren Namen binden. Die
// Bindung selbst hat das Hochziehen schon angelegt (auf „nicht
// bereit", die zeitliche Totzone) — hier wird sie fertig.
```

## L433-436 · `fn block_decls(&mut self, body: &[Stmt]) -> CompileResult<u32> {`

```
/// Was ein Block bindet, BEVOR seine erste Zeile laeuft — dieselben zwei
/// Faelle und dieselbe Reihenfolge wie `Interp::hoist`. `var` steht nicht
/// dabei: das steigt bis zur Funktionsgrenze und ist beim Programmstart
/// schon erledigt.
```

## L441-444 · `fn block_decls_of<'a>(&mut self, body: impl Iterator<Item = &'a Stmt>) -> CompileResult<u32> {`

```
/// Dasselbe ueber eine beliebige Folge — ein `switch` zieht ueber ALLE
/// Faelle zusammen hoch, so wie es der Baumlaeufer tut: sie teilen sich
/// EINE Umgebung, und eine Funktionsdeklaration im dritten Fall ist im
/// ersten schon sichtbar.
```

## L457-459 · `let mut names = Vec::new();`

```
// Auch ein Muster steht mit ALLEN seinen Namen in der
// Totzone — dieselbe Liste wie `Interp::hoist`, dieselbe
// Funktion (`names_of`).
```

## L467-468 · `Stmt::Class(c) => {`

```
// Eine Klasse steht wie ein `let` in der Totzone — dieselben
// zwei Schleifen wie `Interp::hoist`.
```

## L481-494 · `fn try_stmt(&mut self, block: &[Stmt], handler: &Option<CatchClause>,`

```
/// `try` / `catch` / `finally`.
///
/// **Der Finalisierer wird KOPIERT, nicht angesprungen** — einmal fuer den
/// normalen Weg, einmal fuer den Wurf. Ein Unterprogramm waere kuerzer und
/// braeuchte eine Ruecksprungadresse auf dem Stapel; das ist die Stelle,
/// an der solche Maschinen historisch falsch werden (das alte `jsr`/`ret`
/// der JVM ist genau daran gestorben). Zwei Kopien eines meist kurzen
/// Blocks sind der ehrlichere Handel.
///
/// **Nicht gebaut und deshalb abgelehnt:** ein `return`/`break`/`continue`
/// AUS einem `try` mit `finally` heraus. Das muss den Abschluss
/// zwischenspeichern, den Finalisierer fahren und ihn danach fortsetzen —
/// und wenn der Finalisierer selbst abbricht, gewinnt ER. Halb gebaut
/// waere das schlimmer als gar nicht.
```

## L501-502 · `if finalizer.is_some() { self.fin += 1; }`

```
// Ein `yield` unter einem anhaengigen Finalisierer ist derselbe Fall
// wie ein `return` darunter — siehe `Compiler::fin`.
```

## L522 · `let catch_at = self.chunk.here();`

```
// Der Fangpfad. Der geworfene Wert liegt oben, wenn wir hier ankommen.
```

## L527-531 · `if finalizer.is_some() {`

```
// **Der `catch`-Block braucht seinen EIGENEN Behandler**, wenn es
// einen Finalisierer gibt: wirft er selbst, muss der Finalisierer
// trotzdem laufen. Ohne diese Zeile verschwand `finally` still,
// sobald `catch` warf — zwei test262-Faelle, und im Alltag genau
// das Muster „aufraeumen und weiterwerfen".
```

## L557 · `let rethrow_at = self.chunk.here();`

```
// Der Wurfpfad OHNE `catch`: Finalisierer, dann weiterwerfen.
```

## L565 · `self.chunk.patch(to_end);`

```
// Der normale Weg (und der Weg nach einem gefangenen Wurf).
```

## L572 · `match &mut self.chunk.ops[start] {`

```
// Erst jetzt steht fest, wohin der Behandler zeigt.
```

## L577-578 · `} else {`

```
// Ein gefangener Wurf laeuft ueber den Fangpfad, und der
// endet im normalen Finalisierer.
```

## L606-608 · `fn jumps_out(body: &[Stmt]) -> bool {`

```
/// Springt aus diesem Rumpf etwas HERAUS? `return`, `break`, `continue`.
/// Ein `break` innerhalb einer eigenen Schleife zaehlt nicht — es
/// verlaesst den `try` nicht.
```

## L638-643 · `fn for_of(&mut self, left: &ForHead, right: &Expr, body: &Stmt) -> CompileResult<()> {`

```
/// `for (x of e) body`.
///
/// Die Werte werden EAGER geholt (`Interp::iterate`), genau wie im
/// Baumlaeufer. Ein Iterator, der erst beim Ziehen rechnet, braucht eine
/// Maschine, die anhalten kann — das ist Stufe 4, und bis dahin waeren
/// zwei verschiedene Iterationssemantiken das schlechtere Geschaeft.
```

## L654-655 · `let empty = self.chunk.block(Vec::new());`

```
// Je Umlauf eine eigene Umgebung: eine Schliessung im Rumpf soll den
// Wert DIESES Umlaufs festhalten, nicht den letzten.
```

## L667-669 · `for at in l.breaks { self.chunk.patch(at); }`

```
// Zwei Ausgaenge, und sie sind NICHT dasselbe: wer vorzeitig geht,
// schliesst den Iterator (`return()`); wer ihn leergelesen hat, darf
// das nicht mehr.
```

## L680-688 · `fn for_await(&mut self, left: &ForHead, right: &Expr, body: &Stmt) -> CompileResult<()> {`

```
/// `for await (x of y)` (ES 14.7.5.7, `iteratorKind: async`).
///
/// **Dieselbe Form wie `for_of`, mit einem Anhaltepunkt mittendrin.** Das
/// ist der ganze Unterschied und zugleich der Grund fuer die drei eigenen
/// Befehle: `Op::IterNext` ruft `next()` und liest sein Ergebnis in EINEM
/// Schritt, hier muss dazwischen gewartet werden. Also aufgeteilt —
/// rufen, `Op::Await`, auswerten. Das Warten laeuft ueber denselben
/// Anhaltemechanismus wie jedes andere `await`; die Maschine braucht
/// dafuer nichts Neues.
```

## L712-713 · `for at in l.breaks { self.chunk.patch(at); }`

```
// Zwei Ausgaenge, wie beim synchronen `for…of`: wer vorzeitig geht,
// schliesst den Iterator; wer ihn leergelesen hat, darf das nicht.
```

## L724-744 · `fn yield_delegate(&mut self, arg: &Expr) -> CompileResult<()> {`

```
/// `yield* x` (ES 15.5.5) — die Delegation an einen inneren Iterator.
///
/// **Eine Schleife in Befehlen, kein einzelner Befehl**, und das ist der
/// Grund, warum es sie bis 0.169 gar nicht gab: an ihrem Anhaltepunkt muss
/// die Maschine WISSEN, womit sie wieder angeworfen wurde — mit einem
/// Wert, einem Wurf oder einem `return` —, um genau das an den inneren
/// Iterator weiterzureichen. `Vm::send` liefert nur einen Wert,
/// `inject_throw` wickelt sofort ab. Der dritte Weg hinein ist
/// `Vm::Resume`, und er wird hier gelesen.
///
///     <x>
///     DelegateStart            ; inneren Iterator holen, `undefined` legen
///   top:
///     DelegateCall(giveup)     ; next / throw / return, je nach Anwurf
///     [Await]                  ; nur im async-Generator
///     DelegateStep(end)        ; fertig -> ans Ende, sonst Wert legen
///     Yield | YieldDelegate    ; hinausgeben und anhalten
///     Jump top
///   giveup:                    ; innerer Iterator hat kein `return`
///     Ret                      ; der AEUSSERE Generator gibt auf
///   end:
```

## L754-758 · `self.chunk.emit(Op::YieldDelegate(!is_async));`

```
// **Auch der async-Generator bekommt die MARKE**, nur ohne ROH: an
// ihr erkennt `Vm::at_delegate`, dass ein `throw()`/`return()` hier
// weiterzureichen ist statt abzuwickeln. Mit einem gewoehnlichen
// `Op::Yield` sah die Stelle aus wie jedes andere `yield`, und ein
// `agen.throw(e)` wickelte den aeusseren Rumpf ab.
```

## L761-762 · `self.chunk.patch(giveup);`

```
// Der innere Iterator hat kein `return`: der Wert von `gen.return(v)`
// liegt schon auf dem Stapel, und der aeussere Rumpf ist damit fertig.
```

## L770-775 · `fn for_in(&mut self, left: &ForHead, right: &Expr, body: &Stmt) -> CompileResult<()> {`

```
/// `for (k in obj)`.
///
/// Dieselbe Form wie `for_of` — nur ist die Schluesselliste EIFRIG
/// (`Interp::for_in_keys`, dieselbe Hilfe wie im Baumlaeufer), und es gibt
/// nichts zu schliessen: eine fertige Liste hat kein `return()`. Deshalb
/// nehmen beide Ausgaenge denselben `IterDrop`.
```

## L786-788 · `let empty = self.chunk.block(Vec::new());`

```
// Je Umlauf eine eigene Umgebung — wie bei `for…of`, und aus
// demselben Grund: eine Schliessung im Rumpf haelt den Schluessel
// DIESES Umlaufs fest.
```

## L807-825 · `fn switch(&mut self, disc: &Expr, cases: &[SwitchCase]) -> CompileResult<()> {`

```
/// `switch`.
///
/// Drei Dinge machen ihn aus, und alle drei stehen im Code:
///
/// * **EINE Umgebung fuer alle Faelle**, mit den Bindungen aller
///   Fallrumpfe zusammen hochgezogen — eine Funktionsdeklaration im
///   dritten Fall ist im ersten schon da.
/// * **Durchfallen ist die Regel.** Gesucht wird nur der EINSTIEG; ab dort
///   laufen alle Faelle hintereinander weg, bis ein `break` kommt.
/// * **Die Bedingungen werden der Reihe nach ausgewertet, bis eine
///   passt** — und nicht weiter. `default` kommt erst dran, wenn keine
///   passte, egal wo er steht. Genau das tut der Baumlaeufer auch; eine
///   zweite Auswertungsreihenfolge waere hier der teuerste Unterschied.
///
/// Der Wert des `switch` liegt waehrend der Bedingungskette auf dem
/// Stapel und wird in einer kleinen Weiche wieder heruntergenommen, BEVOR
/// ein Fallrumpf laeuft. Ihn dort liegen zu lassen waere kuerzer und
/// falsch: jeder `break`, jedes `continue` und jeder Sprung nach draussen
/// muesste ihn einzeln wegraeumen.
```

## L835 · `let mut hits = Vec::new();`

```
// Die Bedingungskette. Jeder Treffer springt in seine Weiche.
```

## L844 · `self.chunk.emit(Op::Pop);`

```
// Keine passte: den Wert weg und zu `default` (oder ans Ende).
```

## L848 · `let mut gates = Vec::new();`

```
// Die Weichen: Wert herunternehmen, dann in den Rumpf.
```

## L856 · `let mut starts: Vec<u32> = Vec::new();`

```
// Die Rumpfe, hintereinander — das Durchfallen ergibt sich von selbst.
```

## L882-884 · `let Some(e) = &dec.init else {`

```
// Ein MUSTER. Ohne Initialisierer gibt es das nicht (der
// Parser laesst `var {a};` nicht durch), also steht der Wert
// hier immer. Die Bindungen hat das Hochziehen schon angelegt.
```

## L893-895 · `if d.kind == VarKind::Var && dec.init.is_none() {`

```
// `var x;` OHNE Initialisierer laesst eine vorhandene Bindung in
// Ruhe — sonst loescht `var f; function f(){}` die Funktion, die
// das Hochziehen gerade gebunden hat. Ein Test, und er hat recht.
```

## L907 · `if dec.init.is_some() {`

```
// `var f = function(){}` gibt der Funktion den Namen der Variablen.
```

## L920 · `fn expr(&mut self, e: &Expr) -> CompileResult<()> {`

```
// ── Ausdruecke ───────────────────────────────────────────────────────
```

## L952-954 · `Expr::Unary { op: UnaryOp::Typeof, arg } => {`

```
// `typeof x` darf auf einem unbekannten Namen NICHT werfen — das
// ist der Grund, warum es einen eigenen Befehl hat und nicht
// `LoadVar` + `Un` ist.
```

## L980-982 · `_ => {`

```
// `delete x` auf allem anderen ist `true` — genau wie im
// Baumlaeufer. Der Ausdruck wird trotzdem NICHT ausgewertet,
// auch dort nicht.
```

## L995 · `if *op == BinOp::In {`

```
// `#x in obj`: die linke Seite ist ein NAME, kein Wert.
```

## L1018-1019 · `self.chunk.emit(Op::Pop);`

```
// Der linke Wert war nur der Kurzschlusswert; wenn wir hier
// sind, gilt der rechte.
```

## L1048-1049 · `self.chunk.emit(Op::NameFunc(i));`

```
// `q = function(){}` gibt der Funktion den Namen der
// Variablen — dieselbe Regel wie bei `var q = …`.
```

## L1073-1075 · `p => {`

```
// Ein Muster als Ziel: `[a,b] = x`, `({a} = x)`. Der WERT der
// Zuweisung ist die rechte Seite, nicht das Gebundene —
// deshalb bleibt eine Kopie liegen.
```

## L1115-1116 · `Expr::Chain(inner) => {`

```
// Die Klammer um eine Optional-Kette: alle Kurzschluesse darin
// enden HIER, nicht am einzelnen Glied.
```

## L1165-1173 · `Expr::Call { callee, args, optional }`

```
// `a?.b(…)` und `a?.b?.(…)`: der Empfaenger ist `a`, und er darf
// NICHT verlorengehen.
//
// Ohne diesen Zweig fiel BEIDES in den Fall „irgendein Ausdruck
// als Gerufener" und rief mit `undefined` als `this`.
// `o?.m?.forEach(f)` warf dann „Map method on the wrong
// receiver" — und zwar nur auf der Befehlsmaschine, der
// Baumlaeufer war die ganze Zeit richtig. test262 hat es nicht
// gesehen; gefunden hat es die Fritzbox-Oberflaeche.
```

## L1179 · `self.short_circuit(1)?;`

```
// Erst `a` pruefen — das ist das `?.` VOR dem Namen.
```

## L1193-1195 · `if *optional { self.short_circuit(2)?; named = u32::MAX; }`

```
// Und dann das `?.` VOR der Klammer, wenn eins dasteht: hier
// liegen Empfaenger UND Gerufener, der Kurzschluss raeumt
// beide ab.
```

## L1208-1213 · `let mut named = u32::MAX;`

```
// Der Empfaenger gehoert zum Aufruf: `o.f()` ruft mit `o` als
// `this`, `f()` mit undefined. Beides wird HIER entschieden,
// damit die Maschine unten nur noch abarbeitet.
// Der Name des Gerufenen wird MITGEGEBEN — nicht fuer den
// Aufruf, sondern fuer den Fehlschlag: „o is not a function"
// sagt, was fehlt, „value is not a function" nicht.
```

## L1226-1231 · `match k {`

```
// Ein LITERALER Schluessel ist zur Uebersetzungszeit
// bekannt — `o[8362]()` kann seine 8362 nennen, und
// genau so sieht ein minifiziertes Modulregister
// aus. Ein wirklich berechneter Schluessel bleibt
// namenlos; ihn mitzufuehren kostete zwei Befehle
// an jedem Aufruf, und das ist der falsche Handel.
```

## L1263-1264 · `Expr::Call { callee, args, optional: true } => {`

```
// `f?.()` — hier liegen callee UND Empfaenger, also raeumt der
// Kurzschluss zwei Werte ab.
```

## L1299 · `let named = match dotted_name(callee) {`

```
// Wie beim Aufruf: der Name ist fuer die MELDUNG da.
```

## L1320-1323 · `None => {`

```
// Eine LUECKE ist nicht `undefined` — aber der
// Baumlaeufer macht daraus ebenfalls `undefined`
// (`Expr::Array`, `None => Value::Undefined`), und
// dieselbe Naeherung ist besser als eine zweite.
```

## L1360-1362 · `let ist_proto = !p.computed && !p.shorthand`

```
// Siehe `Interp::set_literal_proto` — dieselbe
// Regel, damit die beiden Maschinen nicht
// auseinanderlaufen.
```

## L1424-1426 · `Expr::TaggedTemplate { tag, quasis, exprs } => {`

```
// `tag`a${x}b`` — dieselbe Stapelform wie ein gewoehnlicher
// Aufruf (erst der Gerufene, dann der Empfaenger, dann die
// Argumente), nur dass Argument 0 der Vorlagen-Gegenstand ist.
```

## L1446-1448 · `Expr::Super | Expr::Member { optional: true, .. } => {`

```
// `super.tag`x`` und `a?.tag`x`` haben je eigene Regeln
// fuer den Empfaenger; benannt absagen ist ehrlicher, als
// sie mit `undefined` zu rufen. Der Baumlaeufer kann beide.
```

## L1481-1483 · `Expr::Spread(inner) => self.expr(inner),`

```
// Ein `...x` ausserhalb von Feld und Argumentliste hat der Parser
// schon abgelehnt; hier ist es der nackte Innenausdruck, genau wie
// im Baumlaeufer.
```

## L1485-1487 · `Expr::Yield { arg, delegate } => {`

```
// **Anhalten.** Der Wert geht an `next()` heraus; was `next(v)`
// hereingibt, legt `Vm::send` an dieselbe Stelle des Stapels und
// ist damit der Wert dieses Ausdrucks.
```

## L1504-1509 · `if self.in_async { self.chunk.emit(Op::Await); }`

```
// **In einem async-Generator wird der Wert ERST abgewartet.**
// `yield x` ist dort `AsyncGeneratorYield(? Await(x))`
// (ES 15.5.5) — `yield Promise.resolve(1)` gibt also `1`
// heraus, nicht das Versprechen. Die Regel steht hier und
// nicht in `generator.rs`, damit `Op::Yield` EINE Bedeutung
// behaelt und die Maschine dumm bleibt.
```

## L1514-1518 · `Expr::Await(inner) => {`

```
// **Warten.** Kein `fin`-Verbot wie beim `yield`: eine wartende
// Funktion wird nur mit einem WERT oder einem WURF wieder
// angeworfen, und fuer beides gibt es den Weg schon (`send` und
// `unwind`). Ein `gen.return()`, das einen Finalisierer noch
// fahren muesste, gibt es hier nicht.
```

## L1544-1546 · `fn args_have_spread(args: &[Arg]) -> bool {`

```
/// Hat diese Argumentliste ein `...x`? Dann werden ALLE Argumente in ein
/// Feld gebaut und der Aufruf nimmt dieses — sonst muesste der Befehl eine
/// Zahl tragen, die erst zur Laufzeit feststeht.
```

## L1565-1566 · `fn prop_key(&mut self, k: &PropKey, computed: bool) -> CompileResult<Option<u32>> {`

```
/// Ein statischer Eigenschaftsname wird zum Namensindex; ein berechneter
/// laesst seinen Schluessel auf dem Stapel und gibt `None`.
```

## L1571-1573 · `self.chunk.emit(Op::ToKey);`

```
// SOFORT umwandeln: `ToPropertyKey` darf Nebenwirkungen haben, und
// die Spec legt fest, dass sie VOR der Auswertung des Wertes
// passieren.
```

## L1589 · `fn update(&mut self, op: UpdateOp, arg: &Expr, prefix: bool) -> CompileResult<()> {`

```
/// `x++` / `--o.p` — der Zielausdruck darf nur EINMAL ausgewertet werden.
```

## L1596-1597 · `self.chunk.emit(Op::ToNumeric);`

```
// `to_number` VOR dem Rechnen: `x = "3"; x++` gibt 4, nicht
// "31". `Op::Un(Plus)` ist genau diese Umwandlung.
```

## L1614-1615 · `self.chunk.emit(Op::Dup);`

```
// Den alten Wert unter das Objekt schieben: er ist
// das Ergebnis, das Objekt braucht der Schreiber.
```

## L1624-1626 · `MemberProp::Computed(k) => {`

```
// `o[k]++` — dieselbe Regel: Objekt und Schluessel nur
// EINMAL. Der alte Wert ist das Ergebnis und muss unter
// beiden hindurch nach unten.
```

## L1648-1649 · `fn compound(&mut self, op: AssignOp, target: &Expr, right: &Expr) -> CompileResult<()> {`

```
/// `a += b`, `a ||= b`, und `a = b` als Sonderfall — der linke Ausdruck
/// wird EINMAL ausgewertet.
```

## L1651-1652 · `if matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish) {`

```
// Die kurzschliessenden Formen werten die Rechte NUR aus, wenn sie
// gebraucht wird: `a ||= b` darf `b` nicht anfassen, wenn `a` wahr ist.
```

## L1670-1673 · `Expr::Member { obj, prop, optional: false } => {`

```
// `o.x ||= v` und `o[k] ||= v`. Objekt und Schluessel werden
// EINMAL ausgewertet und liegen unter dem gelesenen Wert; wird
// nicht geschrieben, muessen sie wieder weg — deshalb der
// Umweg ueber zwei Ausgaenge statt eines Sprungs.
```

## L1693-1694 · `self.chunk.patch(keep);`

```
// Der Kurzschluss: der gelesene Wert ist das Ergebnis,
// Objekt (und Schluessel) darunter gehoeren weggeraeumt.
```

## L1738-1739 · `MemberProp::Computed(k) => {`

```
// `o[k] += v`: Objekt und Schluessel EINMAL auswerten, dann
// verdoppeln — `o[i++] += 1` darf `i` nicht zweimal zaehlen.
```

## L1756-1760 · `fn unwind_iters(&mut self, n: usize) {`

```
/// Alle Umgebungen schliessen, die zwischen HIER und `depth` offen sind.
/// Ein Sprung aus einem Block heraus laesst sie sonst stehen.
/// Alle Schleifeniteratoren schliessen, die zwischen HIER und `n` offen
/// sind. Der Iterator der ZIELschleife bleibt: bei `continue` laeuft sie
/// weiter, bei `break` schliesst ihn ihr eigener Nachspann.
```

## L1773-1781 · `fn member_name(&mut self, p: &MemberProp) -> u32 {`

```
/// Der Schluessel eines Elementzugriffs, wo er zur Uebersetzungszeit
/// feststeht.
///
/// **Ein privates Feld ist dabei nichts Besonderes** — nur ein anderer
/// Schluesseltext (`value::private_key`, NUL davor). Es faellt damit aus
/// `own_keys` heraus und ist fuer `Object.keys` und `JSON.stringify`
/// unsichtbar, verhaelt sich sonst aber wie jede Eigenschaft. Deshalb
/// steht `Private` ueberall in DEMSELBEN Zweig wie `Ident`: ein eigener
/// Weg waere eine zweite Semantik fuer denselben Zugriff.
```

## L1793-1799 · `fn take_label(&mut self) -> Vec<String> {`

```
/// Der Kurzschluss eines `?.`: ist der Wert oben nullish, raeumt er
/// `depth` Werte ab, legt `undefined` hin und springt ans Ende der Kette.
///
/// Aufgeraeumt wird HIER und nicht am Ende, weil nur hier feststeht,
/// wieviel unter dem geprueften Wert liegt — bei `a?.b` ist es nichts,
/// bei `o.f?.()` liegt der Empfaenger darunter.
/// Den vorgemerkten Namen abholen — jede Schleife genau einmal.
```

## L1822-1827 · `fn captures(&self, body: &Stmt) -> bool {`

```
/// Faengt in diesem Rumpf eine Funktion etwas ein?
///
/// Nur dafuer da, `for (let i …)` abzulehnen, wenn es darauf ankommt: die
/// Spec gibt jedem Umlauf eine FRISCHE Bindung, und wer das nicht baut,
/// liefert einer Schliessung den Endwert. Ohne Schliessung im Rumpf ist der
/// Unterschied nicht beobachtbar — dann darf die Maschine mitfahren.
```

## L1839-1840 · `fn walk_stmt(st: &Stmt, f: &mut dyn FnMut(&Expr)) {`

```
/// Jeden Ausdruck eines Statements besuchen. Bewusst grob: der einzige Rufer
/// fragt „kommt hier IRGENDWO eine Funktion vor", und dafuer reicht es.
```

## L1870-1871 · `_ => {}`

```
// Alles Uebrige lehnt der Uebersetzer ohnehin ab; ein `true` waere
// hier nur vorsichtiger, nicht richtiger.
```

## L1909-1910 · `pub fn unsupported_name(u: &Unsupported) -> &'static str {`

```
/// Die Namen aller Absagen dieses Laufs — der Uebersetzer zaehlt sie nicht
/// selbst, der Rufer tut es (siehe `Interp::run_program`).
```

## L1915-1916 · `pub fn funcs_of(c: &Chunk) -> &[Rc<Func>] {`

```
/// Nur damit `Vec<Rc<Func>>` im `Chunk` nicht als toter Code gilt, solange die
/// Maschine Aufrufe noch ueber `Interp::call` faehrt.
```

