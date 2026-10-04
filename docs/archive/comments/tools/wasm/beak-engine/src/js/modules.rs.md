# `tools/wasm/beak-engine/src/js/modules.rs` @ 5e0102684

## L1-21 · `use alloc::rc::Rc;`

```
//! ES-Module: Graph, Verknuepfung, Auswertung.
//!
//! **Was hier NICHT ist: das Holen.** Die Engine hat kein Netz und kein
//! Dateisystem; sie sagt nur, WELCHE Adressen sie noch braucht
//! (`Program::requests`), und der Wirt legt den Quelltext dazu (`add_module`).
//! Das ist dieselbe Trennung wie bei den Stilblaettern und Bildern.
//!
//! **Drei Schritte, und die Reihenfolge ist der ganze Punkt.**
//!
//! 1. HOLEN, bis der Graph geschlossen ist — Sache des Wirts.
//! 2. VERKNUEPFEN (`link`): jedes Modul bekommt seine Umgebung, seine
//!    Funktionsdeklarationen stehen darin schon, und jeder `import` wird ein
//!    VERWEIS auf die Bindung im Herkunftsmodul.
//! 3. AUSWERTEN (`evaluate`): Tiefe zuerst, jedes Modul genau einmal.
//!
//! Schritt 2 muss vor JEDER Auswertung fertig sein, und zwar fuer den ganzen
//! Graphen. Der Grund sind Zyklen: `main.js` und `oldpage.js` der
//! Fritzbox-Oberflaeche importieren einander. Wer im Kreis zuerst laeuft,
//! greift auf Namen des anderen zu, bevor dessen Rumpf lief — und findet sie,
//! weil die Funktionsdeklarationen beim Verknuepfen schon stehen und der
//! Verweis lebendig ist. Eine Kopie an dieser Stelle saehe `undefined`, still.
```

## L33 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Wo ein Modul im Ablauf steht.
```

## L36 · `New,`

```
/// Geholt und geparst, sonst nichts.
```

## L38 · `Linked,`

```
/// Umgebung steht, Verweise sind gelegt.
```

## L40-41 · `Running,`

```
/// Der Rumpf laeuft GERADE. Trifft der Graph hier wieder ein, ist das ein
/// Zyklus — und der ist erlaubt, nicht etwa ein Fehler.
```

## L51-52 · `pub exports: HashMap<Rc<str>, (Rc<str>, Rc<str>)>,`

```
/// Der ausgefuehrte Name -> (Modul, lokaler Name dort). Ein
/// Weiterreichen (`export { x } from …`) zeigt direkt auf die Quelle.
```

## L54-55 · `pub star: Vec<Rc<str>>,`

```
/// `export * from …` — beim Nachschlagen durchsucht, nicht kopiert: das
/// Ziel kann selbst noch nicht verknuepft sein.
```

## L57-59 · `pub resolved: HashMap<Rc<str>, Rc<str>>,`

```
/// Rohe Angabe -> aufgeloeste Adresse. **Der Wirt fuellt sie.** Die
/// Engine kann `"./config.js"` nicht aufloesen: dazu gehoert die Adresse
/// des Dokuments, und die gehoert dem Wirt (siehe `set_location`).
```

## L62 · `pub ns: Option<Value>,`

```
/// Das Namensraumobjekt, sobald jemand `import * as x` verlangt hat.
```

## L66-72 · `pub const META_LOCAL: &str = "*meta*";`

```
/// Der lokale Name, unter dem `import.meta` in der Modulumgebung liegt.
///
/// **Als Bindung, nicht als Feld am `Interp`.** `import.meta` ist LEXIKALISCH
/// — eine Funktion, die es liest, gehoert zu dem Modul, in dem sie STEHT,
/// nicht zu dem, das sie gerade ruft. Ein „aktuelles Modul" am Interpreter
/// waere beim ersten Rueckruf falsch, und zwar still. Die Umgebungskette hat
/// die Antwort schon.
```

## L75-76 · `pub const DEFAULT_LOCAL: &str = "*default*";`

```
/// Der lokale Name, unter dem `export default` seinen Wert ablegt. Kein
/// gueltiger Bezeichner, also kann ihn kein Skript treffen.
```

## L79-80 · `pub fn requests(prog: &Program) -> Vec<String> {`

```
/// Die Adressen, die dieses Programm noch braucht — in Quelltextreihenfolge,
/// ohne Doppelte.
```

## L96-98 · `pub fn add_module(&mut self, url: &str, prog: Rc<Program>) {`

```
/// Ein geholtes und geparstes Modul eintragen. Die Adresse ist der
/// Schluessel und muss schon aufgeloest sein — die Engine kennt keine
/// relativen Pfade.
```

## L104-106 · `e.strict = true;`

```
// Ein Modul ist IMMER streng, und sein `this` ist `undefined` —
// nicht das Fenster. Skripte, die beides verwechseln, verhalten
// sich sonst je nach Ladeweg anders.
```

## L126-128 · `pub fn map_module_dep(&mut self, url: &str, spec: &str, target: &str) {`

```
/// Wohin eine Angabe aus diesem Modul zeigt. Der Wirt sagt es, EINMAL je
/// Paar — ohne die Zuordnung sucht der Lader spaeter unter `"./x.js"`
/// und findet nichts.
```

## L135-136 · `fn dep_url(&self, url: &str, spec: &str) -> Rc<str> {`

```
/// Eine Angabe aus diesem Modul, aufgeloest. Ohne Zuordnung bleibt sie
/// wie sie ist — dann waren die Angaben schon absolut.
```

## L144 · `pub fn module_deps(&self, url: &str) -> Vec<Rc<str>> {`

```
/// Die AUFGELOESTEN Adressen, die dieses Modul braucht.
```

## L149-150 · `pub fn module_requests(&self, url: &str) -> Vec<String> {`

```
/// Welche Adressen dieses eingetragene Modul noch braucht, AUFGELOEST
/// gegen seine eigene — der Wirt fragt das nach jeder Holrunde.
```

## L158-166 · `pub fn link_module(&mut self, url: &str) -> C<()> {`

```
/// Den Graphen ab `url` verknuepfen. Ohne Auswertung.
///
/// **Zwei Durchlaeufe, und das ist keine Bequemlichkeit.** Erst bekommt
/// JEDES Modul seine Deklarationen und seine Ausfuhrtabelle, dann erst
/// wird ein einziger Verweis gelegt. Bei einem Zyklus ist das der
/// Unterschied zwischen „laeuft" und „`hallo` wird von a.js nicht
/// ausgefuehrt": wer zuerst verknuepft, fragt sonst eine Tabelle ab, die
/// es noch nicht gibt. Die Spezifikation macht es genauso
/// (`InnerModuleLinking` sammelt vor `InitializeEnvironment`).
```

## L193 · `fn collect(&mut self, url: &str, out: &mut Vec<Rc<str>>) -> C<()> {`

```
/// Alle erreichbaren Module, Tiefe zuerst, ohne Doppelte.
```

## L204 · `fn build_exports(&mut self, m: &Rc<RefCell<Module>>) -> C<()> {`

```
/// Die Ausfuhrtabelle des Moduls aus seinem Quelltext.
```

## L212-213 · `let from: Rc<str> = match source {`

```
// `export { a as b } from "m"` zeigt DIREKT auf `m` —
// nicht ueber eine lokale Bindung, die es nicht gibt.
```

## L232-233 · `Some(a) => { exports.insert(Rc::from(a.as_str()),`

```
// `export * as ns from "m"` ist ein EINZELNER Name, kein
// Durchreichen — er braucht das Namensraumobjekt.
```

## L247 · `fn bind_imports(&mut self, m: &Rc<RefCell<Module>>) -> C<()> {`

```
/// Jeden `import` als Verweis in die Umgebung des Moduls legen.
```

## L260-261 · `let ns = self.namespace(&from)?;`

```
// Ein Namensraum ist ein WERT, kein Verweis: das
// Objekt selbst wechselt nie, nur seine Felder.
```

## L272 · `fn alias(&mut self, env: &Rc<RefCell<Env>>, local: &str, from: &str, name: &str) -> C<()> {`

```
/// Einen Namen im Zielmodul aufloesen und den Verweis eintragen.
```

## L281-282 · `return self.ref_err(&alloc::format!(`

```
// KEIN stiller Ausfall: ein Name, den das Zielmodul nicht
// ausfuehrt, ist ein Fehler, und er nennt beide Seiten.
```

## L292-293 · `fn resolve_export(&mut self, url: &str, name: &str, seen: &mut Vec<String>)`

```
/// Wo liegt der ausgefuehrte Name wirklich? Folgt Weiterreichungen und
/// `export *`.
```

## L307 · `if name != "default" {`

```
// `export *` reicht alles ausser `default` weiter.
```

## L316-322 · `fn namespace(&mut self, url: &str) -> C<Value> {`

```
/// Das Namensraumobjekt eines Moduls: eine Momentaufnahme seiner Ausfuhr.
///
/// **Eine Aufnahme, kein lebendes Objekt.** Die Spezifikation will exotic
/// getter, die bei jedem Zugriff nachsehen; das ist hier bewusst nicht
/// gebaut, weil kein gemessener Aufruf im Zielkorpus darauf angewiesen
/// ist. Was es kostet, steht damit fest: ein Namensraum, der VOR der
/// Auswertung des Zielmoduls gelesen wird, zeigt nur die Funktionen.
```

## L354-357 · `pub fn ns_live(&mut self, url: &str, name: &str) -> Option<Value> {`

```
/// Der AKTUELLE Wert eines Exports — die lebende Bindung hinter einer
/// Namensraum-Eigenschaft. `None`, wenn der Name kein Export ist (dann
/// antwortet die gewoehnliche Eigenschaftstabelle, etwa fuer
/// `Symbol.toStringTag`).
```

## L367 · `pub fn eval_module(&mut self, url: &str) -> C<()> {`

```
/// Den Graphen ab `url` auswerten — Tiefe zuerst, jedes Modul einmal.
```

## L372-374 · `let state = m.borrow().state;`

```
// Den Zustand HERAUSKOPIEREN. Die Ausleihe eines `match`-Gegenstands
// lebt bis zum Ende des ganzen `match`, und `link_module` schreibt in
// dasselbe Modul.
```

## L377-378 · `ModState::Running | ModState::Done => return Ok(()),`

```
// `Running` heisst: wir stehen IM Zyklus. Zurueckkehren, nicht
// ein zweites Mal fahren.
```

## L392-395 · `let r = (|| -> C<()> {`

```
// Der Rumpf. Die Deklarationen stehen seit dem Verknuepfen, also NUR
// ausfuehren — ein zweites Hochziehen wuerde eine `let`-Bindung
// zurueck auf „nicht bereit" stellen, die ein Zyklus schon gefuellt
// hat.
```

## L403-405 · `if r.is_err() && self.module_fail.is_none() {`

```
// Den ERSTEN Werfer festhalten, nicht den letzten: der aeussere
// Aufrufer reicht denselben Fehler nach oben durch, und dessen Name
// wuerde den echten ueberschreiben.
```

## L409-411 · `if r.is_ok() { self.refresh_namespace(&m); }`

```
// Ein Namensraum, der VOR dem Rumpf gebaut wurde, hat die spaeter
// zugewiesenen Werte nicht. Hier nachziehen — das ist der billige
// Ersatz fuer die exotic getter der Spezifikation.
```

## L433 · `pub fn decl_names(d: &Stmt) -> Vec<String> {`

```
/// Die Namen, die eine Deklaration einfuehrt — hinter einem `export`.
```

## L445-446 · `pub fn unexport(st: &Stmt) -> Option<&Stmt> {`

```
/// Steckt hinter diesem `export` eine Deklaration? Dann ist SIE die
/// Anweisung, die laufen und hochgezogen werden muss.
```

## L454-455 · `pub fn describe(i: &mut Interp, e: Abrupt) -> String {`

```
/// Der Ausgang einer Auswertung als Fehlertext — der Wirt hat keinen Zugriff
/// auf `Abrupt`.
```

