# `tools/wasm/beak-engine/src/js/parser.rs` @ 5e0102684

## L1-17 · `use alloc::boxed::Box;`

```
//! Rekursiver Abstieg mit Vorrangkletterung fuer die Ausdruecke.
//!
//! Drei Entscheidungen, die den Rest erklaeren:
//!
//! 1. **Ein Token Vorausschau, mehr nicht.** Wo die Grammatik mehr verlangt —
//!    `(a, b) => c` gegen `(a, b)` — wird NICHT vorausgeschaut, sondern erst
//!    als Ausdruck gelesen und beim `=>` in ein Muster umgebogen
//!    (`expr_to_pattern`). Das ist die Deckgrammatik, die die Spezifikation
//!    selbst beschreibt, und sie kostet keine Ruecksetzpunkte.
//! 2. **`regex_ok` folgt aus dem VORIGEN Token.** Der Lexer kann `/` nicht
//!    allein einordnen; die Tabelle unten sagt, wann ein Operand erwartet wird.
//!    Nach `}` wird Regex angenommen — das ist bei einem Blockende richtig und
//!    bei einem Objektliteral falsch, und die erste Lage kommt in echtem Code
//!    um Groessenordnungen haeufiger vor.
//! 3. **Fruehfehler sind noch nicht vollstaendig.** Was gebaut ist, steht in
//!    `strict_*`; was fehlt, faellt im test262-Lauf als „erwartete einen
//!    Parse-Fehler" auf und ist damit gezaehlt statt vergessen.
```

## L34-35 · `fn regex_allowed_after(t: &Tok) -> bool {`

```
/// Erwartet die Grammatik hier einen Operanden? Dann ist `/` der Anfang eines
/// regulaeren Ausdrucks, sonst eine Division.
```

## L39-41 · `Tok::Template { has_sub, .. } => *has_sub,`

```
// Nach einem Template-Stueck MIT folgender Einsetzung kommt ein
// Ausdruck, also darf dort ein Regex stehen: `${/^a/.test(x)}`.
// Nach dem letzten Stueck ist `/` eine Division.
```

## L43-53 · `Tok::Keyword(k) if !k.is_reserved() => false,`

```
// **Ein kontextuelles Schluesselwort ist meistens ein Bezeichner** —
// `var target = 8; target / 2` ist eine Division, keine Regex. Hier
// stand vorher „jedes Schluesselwort ausser fuenf erlaubt einen
// Regex", und damit brach jedes Skript, das eine Variable `get`,
// `set`, `of`, `as`, `from`, `async`, `target`, `meta` oder `let`
// teilt. Gefunden an d3: `tickIntervals[target / …]` — ab da las der
// Lexer den Rest der Zeile als Regex und die ganze Bibliothek fiel aus.
//
// `of` steht mit hier, obwohl `for (x of …)` ein Ausdruck folgt: ein
// Regex ist nicht iterierbar, `for (x of /re/)` ist also kein Code,
// den jemand schreibt.
```

## L64-65 · `cur_start: usize,`

```
/// Stand des Lexers VOR `cur` — fuer die Faelle, in denen dasselbe Zeichen
/// neu gelesen werden muss (Template-Fortsetzung nach `}`).
```

## L74-75 · `just_paren: bool,`

```
/// War der zuletzt gelesene Operand geklammert? `(a && b) ?? c` ist
/// erlaubt, `a && b ?? c` nicht — und im Baum sieht beides gleich aus.
```

## L77-79 · `in_class_field: bool,`

```
/// Stehen wir in einem Feld-Initialisierer einer Klasse? Dort ist
/// `arguments` verboten. Pfeile erben das (sie haben kein eigenes
/// `arguments`), gewoehnliche Funktionen setzen es zurueck.
```

## L81-83 · `pend_simple: bool,`

```
/// Was `params()` zuletzt gelesen hat — `block_body_with_prologue` holt es
/// sich, weil die Regel „use strict neben nicht-einfachen Parametern" erst
/// beim Direktiven-Vorspann entscheidbar ist.
```

## L86 · `num_legacy_octal: bool,`

```
/// War `cur` eine Zahl in Alt-Oktal-Form?
```

## L88-92 · `paren_hdr: Vec<bool>,`

```
/// Fuer jede OFFENE Klammer: schliesst sie den KOPF einer Anweisung
/// (`if`, `for`, `while`, `with`)? Danach faengt eine Anweisung an, und
/// ein `/` dort ist ein Regex — waehrend dasselbe `)` am Ende eines
/// Ausdrucks eine Division einleitet. Das ist die eine Stelle, an der
/// `regex_allowed_after` mit dem Token allein nicht auskommt.
```

## L94 · `prev_hdr: bool,`

```
/// War das zuletzt verbrauchte Token eines dieser vier Woerter?
```

## L98-100 · `struct Save {`

```
/// Ein Ruecksetzpunkt. Traegt die Klammerbuchfuehrung mit — eine Vorausschau,
/// die eine Argumentliste durchliest, legt sonst Klammern ab, die niemand
/// mehr abraeumt.
```

## L130-137 · `let hdr = match &self.cur.tok {`

```
// `yield` und `await` sind nur DORT Schluesselwoerter, wo sie
// reserviert sind; sonst sind sie Bezeichner, und dann ist `/` eine
// Division. Das weiss nur der Parser — `regex_allowed_after` sieht
// bloss das Token.
// Klammerbuchfuehrung: beim `(` merken, WORAUF es folgte, und beim
// passenden `)` wieder herausholen. `for (const [k, v] of m) /re/.test(k)`
// ist echter Code — er stand in DuckDuckGos Hauptbuendel und liess den
// ganzen Chunk als SyntaxError ausfallen.
```

## L154-155 · `self.num_legacy_octal = self.lx.legacy_octal;`

```
// Gehoert zum GERADE GELESENEN Token, nicht zum Lexer — der naechste
// `bump` setzt es zurueck.
```

## L172-174 · `fn ident_name(&mut self) -> R<String> {`

```
/// Der Name eines Bezeichners an dieser Stelle, mit den kontextabhaengigen
/// Schluesselwoertern als gueltigen Namen. `yield` und `await` haengen am
/// Kontext: in einem Generator bzw. einer async-Funktion sind sie reserviert.
```

## L188-190 · `if name == "await" && (self.in_async || self.module) {`

```
// Am NAMEN geprueft, nicht am Token: `\u0061wait` kommt als Bezeichner
// herein (eine Flucht macht aus einem Schluesselwort keines mehr), und
// eine Pruefung auf `Tok::Keyword` sieht davon nichts. 294 Tests.
```

## L201-203 · `fn module_export_name(&mut self) -> R<String> {`

```
/// Ein Modulname: Bezeichner ODER Zeichenkette. `export { "a b" as c }`
/// ist ES2022 und der einzige Ort, an dem ein Name Leerzeichen tragen darf
/// — WebAssembly-Module exportieren solche Namen.
```

## L209-210 · `fn property_name(&mut self) -> R<String> {`

```
/// Ein Eigenschaftsname nach `.` — dort sind ALLE reservierten Woerter
/// erlaubt (`a.class`, `a.if`). Seit ES5, und echter Code nutzt es.
```

## L221-222 · `fn semicolon(&mut self) -> R<()> {`

```
/// Semikolon — oder die automatische Einfuegung. Sie greift vor `}`, am
/// Ende der Datei und wenn vor dem naechsten Token eine Zeile begann.
```

## L231 · `pub fn parse_program(&mut self) -> R<Program> {`

```
// ── Programm ─────────────────────────────────────────────────────────
```

## L235 · `Ok(Program { body, module: self.module, strict: self.strict || self.module })`

```
// Ein Modul ist immer streng, ein Skript nur mit Direktive.
```

## L239-242 · `fn directive_prologue_and_body(&mut self, top: bool) -> R<Vec<Stmt>> {`

```
/// Der Direktiven-Vorspann: fuehrende Zeichenkettenausdruecke, unter denen
/// `"use strict"` den Rest der Einheit umschaltet. Er muss VOR dem
/// Weiterlesen ausgewertet werden — der strenge Modus aendert, was
/// ueberhaupt noch parst.
```

## L253-254 · `let raw = &self.lx_src()[raw_start..raw_end];`

```
// Auf den ROHTEXT geprueft, nicht auf den entschluesselten:
// `"use strict"` ist KEINE Direktive (ES §11.2.1).
```

## L275 · `fn statement(&mut self) -> R<Stmt> {`

```
// ── Anweisungen ──────────────────────────────────────────────────────
```

## L294-296 · `Kw::Let if self.let_is_decl()? => self.var_statement(),`

```
// `let` ist nur dann eine Deklaration, wenn ein Bezeichner,
// `[` oder `{` folgt — sonst ist es ein Bezeichner
// (`let = 1`, `let.a`, `let(x)`).
```

## L325 · `let _ = self.eat_p(P::Semi)?;`

```
// Nach `do {} while ()` darf das Semikolon immer fehlen.
```

## L384 · `fn let_is_decl(&mut self) -> R<bool> {`

```
/// Folgt auf `let` etwas, das es zur Deklaration macht?
```

## L396 · `fn async_function_ahead(&mut self) -> R<bool> {`

```
/// `async function` — aber nur ohne Zeilenumbruch dazwischen.
```

## L413-415 · `fn mark(&self) -> Save {`

```
/// Zuruecksetzen. Nur fuer die drei Stellen oben, an denen ein Token
/// Vorausschau nicht reicht — nicht als allgemeines Ruecksetzen: davon
/// leben Parser, die man nicht mehr versteht.
```

## L447 · `if init.is_none() && kind == VarKind::Const && !matches!(id, Pat::Ident(_)) {`

```
// `const x;` hat keinen Wert, den es festhalten koennte.
```

## L482 · `if self.is_p(P::Semi) {`

```
// Leerer Kopf: `for (;;)`
```

## L514 · `let init = if self.eat_p(P::Eq)? { Some(self.assign_expr()?) } else { None };`

```
// Gewoehnliche Deklaration im Kopf — der Rest der Liste folgt.
```

## L526-527 · `let e = self.expression_no_in()?;`

```
// Ausdruckskopf. `in` muss hier ausgeschlossen bleiben, sonst frisst
// der Vergleichsoperator das `in` von `for (x in y)`.
```

## L565 · `let param = if self.eat_p(P::LParen)? {`

```
// `catch {}` ohne Bindung ist ES2019.
```

## L619-620 · `let all: Vec<Stmt> = cases.iter().flat_map(|c| c.body.iter().cloned()).collect();`

```
// Der ganze switch-Koerper ist EIN Bereich, ueber alle Faelle hinweg:
// `case 1: let x; case 2: let x;` ist ein Fehler.
```

## L627-628 · `if matches!(self.cur.tok, Tok::Ident(_)) {`

```
// Ein Label ist ein Bezeichner mit `:` dahinter, und das sieht man
// erst nach dem Bezeichner.
```

## L643 · `fn import_statement(&mut self) -> R<Stmt> {`

```
// ── Module ───────────────────────────────────────────────────────────
```

## L724 · `fn function(&mut self, is_async: bool, need_name: bool) -> R<Func> {`

```
// ── Funktionen und Klassen ───────────────────────────────────────────
```

## L726 · `self.bump()?; // function`

```
// `function`
```

## L747-751 · `fn block_body_with_prologue(&mut self) -> R<(Vec<Stmt>, bool)> {`

```
/// Der Rumpf UND seine Strenge. Bis 0.98.0 gab er nur den Rumpf, und die
/// Strenge blieb im Parser stehen — zwei Aufrufer haben sie danach nicht
/// einmal zurueckgesetzt (`({ m(){ "use strict"; } })` faerbte alles
/// dahinter mit). Wer sie zurueckgibt, zwingt jeden Aufrufer, sich zu
/// entscheiden.
```

## L753-756 · `let simple = core::mem::replace(&mut self.pend_simple, true);`

```
// Die Parameter, die gerade gelesen wurden — der Vorspann braucht sie:
// `"use strict"` neben einer nicht-einfachen Parameterliste ist ein
// Fruehfehler, und dieselbe Direktive macht doppelte Namen nachtraeglich
// unzulaessig. Beides ist erst HIER entscheidbar.
```

## L777-787 · `fn check_scope(&self, stmts: &[Stmt]) -> R<()> {`

```
/// Prueft EINE Anweisungsliste auf doppelte Deklarationen.
///
/// Bewusst nur der gleiche Bereich, ohne die Hochwanderung von `var` durch
/// verschachtelte Bloecke: `{ let x; { var x; } }` faellt hier NICHT auf.
/// Das ist eine Teilmenge der Regel und keine falsche — was fehlt, steht
/// im test262-Lauf als „faelschlich angenommen" und ist damit gezaehlt
/// statt vergessen.
///
/// Die Richtung ist unsymmetrisch, und das ist die Regel selbst:
/// `var x; var x;` ist erlaubt, `let x; let x;` nicht, und `var x; let x;`
/// scheitert an der lexikalischen Seite.
```

## L789 · `let mut seen: Vec<(String, bool)> = Vec::new();`

```
// (Name, ist_lexikalisch)
```

## L802 · `let mut cur = st;`

```
// Ein Label davor aendert nichts an der Deklaration dahinter.
```

## L822-823 · `fn bound_names(p: &Pat, out: &mut Vec<String>) {`

```
/// Sammelt die gebundenen Namen eines Musters. Grundlage fuer „doppelter
/// Parameter" und spaeter fuer „doppelte lexikalische Deklaration".
```

## L838-844 · `fn finish_params(&mut self, params: &[Pat], unique: bool) -> R<()> {`

```
/// Nach dem Lesen einer Parameterliste: Einfachheit und Namen festhalten,
/// und die Doppelung pruefen, wo sie schon jetzt entscheidbar ist.
///
/// `unique` = die Stelle verlangt Eindeutigkeit unabhaengig vom Modus
/// (Methoden, Pfeile, Setter). Sonst gilt sie im strengen Modus und bei
/// jeder nicht-einfachen Liste — und ausserdem noch einmal spaeter, wenn
/// eine `"use strict"`-Direktive im Koerper den Modus umlegt.
```

## L869-871 · `if self.is_p(P::Eq) { return self.err("rest parameter cannot have a default"); }`

```
// Ein Rest-Parameter nimmt keinen Vorgabewert und duldet
// nichts hinter sich — auch kein Schlusskomma. Der alte Code
// brach hier einfach ab und liess beides durchgehen.
```

## L890 · `self.bump()?; // class`

```
// `class`
```

## L891 · `let outer_strict = self.strict;`

```
// Ein Klassenkoerper ist IMMER streng, auch in einer lockeren Datei.
```

## L915 · `if self.is_p(P::Eq) || self.is_p(P::Semi) || self.is_p(P::RBrace) || self.is_p(P::LParen) {`

```
// `static` allein als Feldname (`static = 1`, `static;`) ist erlaubt.
```

## L919-920 · `let ocf = core::mem::replace(&mut self.in_class_field, true);`

```
// Statischer Initialisierungsblock (ES2022) — auch dort gibt
// es kein `arguments`.
```

## L971 · `let value = if self.eat_p(P::Eq)? {`

```
// Feld.
```

## L1001 · `fn expression(&mut self) -> R<Expr> {`

```
// ── Ausdruecke ───────────────────────────────────────────────────────
```

## L1010-1011 · `fn expression_no_in(&mut self) -> R<Expr> {`

```
/// Wie `expression`, aber `in` gilt nicht als Operator — nur fuer den
/// `for`-Kopf.
```

## L1025 · `if let Some(f) = self.try_simple_arrow()? { return Ok(f); }`

```
// Pfeil mit einem einzelnen Bezeichner: `x => …`, `async x => …`.
```

## L1049 · `let pat = if op == AssignOp::Assign {`

```
// Nur bei `=` darf links ein Muster stehen; `[a] += 1` gibt es nicht.
```

## L1055-1056 · `Expr::Call { .. } if !self.strict => Pat::Expr(Box::new(left)),`

```
// Annex B B.3.5: im lockeren Modus ist ein Aufruf ein
// gueltiges Zuweisungsziel (es wirft dann zur Laufzeit).
```

## L1071-1073 · `if self.is_kw(Kw::Async) {`

```
// `async x => …` und `async (…) => …`. Die geklammerte Form ist die
// haeufigere in echtem Code und war der zweite Grund, aus dem der erste
// Lauf gueltige Dateien ablehnte.
```

## L1084-1086 · `if let Ok(args) = self.arguments() {`

```
// Als Argumentliste lesen und beim `=>` umbiegen — dieselbe
// Deckgrammatik wie beim gewoehnlichen Pfeil. Ohne `=>` ist
// es ein Aufruf `async(…)`, und der Ruecksetzpunkt traegt.
```

## L1117 · `self.finish_params(&params, true)?;`

```
// Ein Pfeil duldet doppelte Parameter NIE — auch im lockeren Modus.
```

## L1120-1121 · `self.in_async = is_async; self.in_func = true;`

```
// Ein Pfeil hat kein eigenes `yield`-Verhalten; `in_gen` bleibt aussen
// stehen, weil `yield` im Pfeilkoerper den umgebenden Generator meint.
```

## L1129 · `let e = self.assign_expr()?;`

```
// Ein Ausdruckskoerper hat keinen Vorspann — er erbt schlicht.
```

## L1143-1144 · `let arg = if (!delegate && self.cur.newline_before) || self.is_p(P::RParen) || self.is_p(P::RBracket)`

```
// Nach `yield*` MUSS ein Ausdruck folgen — auch ueber eine Zeile
// hinweg. Die Semikolon-Einfuegung greift nur beim nackten `yield`.
```

## L1157 · `let cons = self.assign_expr()?;`

```
// Die beiden Zweige sind AssignmentExpression — `in` gilt darin wieder.
```

## L1164-1165 · `fn binary(&mut self, min_prec: u8, no_in: bool) -> R<Expr> {`

```
/// Vorrangkletterung. Die Tabelle ist die aus der Spezifikation; `**` ist
/// rechtsassoziativ, alles andere links.
```

## L1199 · `if let Some(LogicalOp::Nullish) = logical {`

```
// `??` darf sich nicht ungeklammert mit `&&`/`||` mischen.
```

## L1241-1242 · `if op == UnaryOp::Delete && self.strict && matches!(arg, Expr::Ident(_)) {`

```
// `delete x` auf einen blossen Bezeichner ist im strengen Modus
// ein Fruehfehler.
```

## L1246-1249 · `if op == UnaryOp::Delete && deletes_private(&arg) {`

```
// `delete a.#x` ebenso — und ohne Modus-Frage, denn ein privater
// Name kann nur in einem Klassenkoerper stehen, und der ist immer
// streng. Klammern heben es nicht auf: der Baum hat sie nicht mehr,
// also greift die Pruefung ohnehin durch.
```

## L1253 · `if self.is_p(P::StarStar) { return self.err("unparenthesized unary before **"); }`

```
// `-a ** b` ist mehrdeutig und deshalb verboten.
```

## L1333 · `self.bump()?; // new`

```
// `new`
```

## L1340-1345 · `let direct_import = self.is_kw(Kw::Import);`

```
// `new import(…)` gibt es nicht — ImportCall ist eine CallExpression
// und kein Konstruktorziel. Zwei Nachbarn, die es SEHR WOHL gibt und
// die beide an einer zu breiten Fassung gescheitert sind:
// `new import.meta.Foo()` (MetaProperty) und `new (import(x))` (die
// Klammer macht daraus einen gewoehnlichen Operanden). Deshalb wird
// hier die DIREKTE Form gemerkt, bevor gelesen wird.
```

## L1351-1352 · `let mut callee = callee;`

```
// Die Glieder VOR den Argumenten gehoeren noch zum Konstruktor:
// `new a.b.C()` ruft `a.b.C`.
```

## L1392-1394 · `if !self.is_p(P::RBrace) { return self.err("expected } in template"); }`

```
// Nach dem Ausdruck steht `}` — aber als Fortsetzung des Templates,
// nicht als Satzzeichen. Der Lexer muss ab DORT weiterlesen, also
// wird an der Stelle des `}` neu angesetzt.
```

## L1407-1409 · `if self.strict && self.num_legacy_octal {`

```
// `010` und `089` sind im strengen Modus Fruehfehler. Der Lexer
// merkt sich nur, DASS es die Form war — ob sie zaehlt, weiss
// erst der Parser, weil eine Direktive den Modus umlegt.
```

## L1421-1425 · `if quasis.iter().any(|q| q.cooked.is_none()) {`

```
// **Nur ein GETAGGTES Template darf ungueltige Fluchten
// enthalten** (ES 12.9.6). Ohne Marke ist jede davon ein
// Fruehfehler — der Lexer merkt sie sich als `cooked: None`,
// weil er zur Lesezeit noch nicht weiss, ob eine Marke
// davorsteht; die Entscheidung faellt hier.
```

## L1433-1436 · `if self.in_class_field && n == "arguments" {`

```
// Ein Feld-Initialisierer laeuft ohne eigenes `arguments`, also
// ist der Name dort ein Fruehfehler. Ein PFEIL darin erbt das
// (auch er hat keins); eine gewoehnliche Funktion setzt es
// zurueck, deshalb steht das Sichern in `function`, nicht hier.
```

## L1443 · `self.bump()?;`

```
// `#x in obj` — die Pruefung auf ein privates Feld.
```

## L1468-1470 · `if self.in_class_field && self.is_p(P::LParen) {`

```
// `super()` ruft den Basiskonstruktor — in einem
// Feld-Initialisierer oder statischen Block gibt es keinen
// zu rufen. `super.x` bleibt dort erlaubt.
```

## L1494-1495 · `if args.is_empty() { return self.err("import() requires an argument"); }`

```
// `import()` ist keine gewoehnliche Funktion: kein Spread,
// mindestens ein und hoechstens zwei Argumente (ES 13.3.10).
```

## L1512-1513 · `fn paren_or_arrow(&mut self) -> R<Expr> {`

```
/// `(` — entweder eine Klammer, eine Sequenz, oder die Parameterliste
/// eines Pfeils. Welches, sagt erst das Zeichen NACH dem `)`.
```

## L1516 · `if self.is_p(P::RParen) {`

```
// `()` kann nur ein Pfeil sein.
```

## L1532 · `if self.is_p(P::RParen) { break; } // erlaubtes Schlusskomma`

```
// erlaubtes Schlusskomma
```

## L1563 · `let mut kind = 0u8; // 0 normal, 1 get, 2 set`

```
// 0 normal, 1 get, 2 set
```

## L1589-1593 · `let ocf = core::mem::replace(&mut self.in_class_field, false);`

```
// Auch ein Getter im Objektliteral hat sein EIGENES
// `arguments` — steht das Literal in einem Feld-Initialisierer,
// gilt die Sperre darin nicht mehr. Genau daran ist
// `context={…,get(){…arguments…}}` aus dem Discourse-Bundle
// gescheitert.
```

## L1626-1628 · `let name = match &key {`

```
// Kurzform — und `{a = 1}` ist NUR als Muster gueltig. Hier als
// Zuweisung aufgehoben, `expr_to_pattern` biegt es um; ein
// Objektliteral mit dieser Form scheitert dort.
```

## L1648-1651 · `fn expr_to_pattern(&mut self, e: Expr, binding: bool) -> R<Pat> {`

```
// ── Deckgrammatik: Ausdruck zu Muster ────────────────────────────────
/// `binding` = die Stelle verlangt eine BINDUNG (Deklaration, Parameter).
/// Dort sind nur Bezeichner und Muster erlaubt; in einer Zuweisung darf
/// auch `a.b` oder `a[0]` links stehen.
```

## L1673-1675 · `if matches!(*inner, Expr::Assign { .. }) {`

```
// `[...a = 1]` gibt es nicht, und `[...a,]` auch
// nicht — ein Schlusskomma HINTER dem Rest ist ein
// eigenes (leeres) Element und faellt oben durch.
```

## L1697-1698 · `let p = self.expr_to_pattern(inner, binding)?;`

```
// In einer BINDUNG muss der Rest ein blosser Name
// sein: `{...{a}} = x` ist keine Bindung.
```

## L1720 · `pub fn parse(src: &str, module: bool) -> Result<Program, ParseError> {`

```
/// Ein Programm parsen. `module` waehlt die Grammatik, nicht nur einen Schalter.
```

## L1726-1731 · `fn deletes_private(e: &Expr) -> bool {`

```
/// Loescht `delete e` einen privaten Namen?
///
/// Nur das AEUSSERSTE Glied zaehlt: `delete a.#x` ist verboten, `delete
/// a.#x.y` erlaubt — dort faellt `y`, nicht `#x`. Die erste Fassung rekursierte
/// in den Empfaenger und lehnte damit `delete this.#b.data[k]` ab, das auf
/// srf.ch und im Discourse-Forum ausgeliefert wird. Gegen V8 geprueft.
```

