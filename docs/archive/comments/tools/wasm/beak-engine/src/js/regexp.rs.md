# `tools/wasm/beak-engine/src/js/regexp.rs` @ 5e0102684

## L1-17 · `use alloc::boxed::Box;`

```
//! Regulaere Ausdruecke: Muster-Parser und Rueckverfolgung.
//!
//! **Rueckverfolgung, nicht Automat.** Ein DFA waere schneller und koennte
//! nicht katastrophal werden — aber er kann keine Rueckwaertsverweise und
//! keine Umschau, und die Spezifikation ist selbst in Rueckverfolgung
//! formuliert (ES 22.2.2). Ein Motor, der `(a+)+b` in Sekunden statt in
//! Jahrtausenden beantwortet, aber `\1` nicht kennt, waere fuer eine echte
//! Seite der schlechtere Tausch.
//!
//! **Deshalb ein Schrittdeckel, von Anfang an.** Katastrophales
//! Backtracking ist dieselbe Falle wie die vier nativen Schleifen, die in
//! dieser Sitzung ohne Deckel gelaufen sind — nur dass hier die FREMDE SEITE
//! das Muster stellt. `(a+)+$` auf dreissig `a` sind ohne Deckel 2^30 Wege.
//!
//! **Zeichen, nicht UTF-16-Einheiten.** JS zaehlt in UTF-16; hier wird in
//! `char` gezaehlt. Fuer alles ausserhalb der Basisebene (Emoji) weichen die
//! Indizes ab. Bewusst und benannt, statt still falsch.
```

## L59 · `let mut out = String::new();`

```
// Die Reihenfolge ist festgelegt: d g i m s u v y.
```

## L134-135 · `let save = self.i;`

```
// `{` ist nur dann ein Zaehler, wenn es auch einer ist —
// sonst ein gewoehnliches Zeichen (`a{b}` ist gueltig).
```

## L189 · `let mut name = String::new();`

```
// Benannte Gruppe `(?<name>…)`.
```

## L232 · `if self.at() == Some('-') && self.c.get(self.i + 1).copied() != Some(']') && self.c.get(self.i + 1).is_some() {`

```
// Ein `-` gefolgt von etwas anderem als `]` macht einen Bereich.
```

## L249-250 · `fn class_escape(&mut self) -> Result<Result<char, ClassItem>, &'static str> {`

```
/// In einer Klasse: entweder ein Zeichen (`Ok`) oder eine ganze Gruppe
/// wie `\d` (`Err`, was hier kein Fehler ist sondern der andere Fall).
```

## L312 · `if !self.eat('<') { return Ok(Node::Char('k')); }`

```
// Benannter Rueckverweis `\k<name>`.
```

## L328-332 · `return Err("unicode property escapes are not supported");`

```
// Unicode-Eigenschaften: die Tabelle dafuer kostet Zehntausende
// Zeichen und keine Seite des Zielkorpus benutzt sie. Als
// FEHLER melden, nicht still als Zeichen lesen — ein Muster,
// das etwas anderes tut als es sagt, ist schlimmer als eins,
// das gar nicht laeuft.
```

## L352 · `pub struct Match {`

```
/// Ein Treffer: Zeichenspannen je Gruppe, 0 = das Ganze.
```

## L365-369 · `const MAX_STEPS: u32 = 400_000;`

```
/// Wie viele Rueckverfolgungsschritte ein Treffer kosten darf.
///
/// Kein Zierrat: `(a+)+$` auf dreissig `a` sind 2^30 Wege, und das MUSTER
/// stellt die fremde Seite. Reisst der Deckel, gilt „kein Treffer" — falsch,
/// aber begrenzt falsch, und ein haengender Browser waere schlimmer.
```

## L371-382 · `const STEPS_PER_CHAR: u32 = 64;`

```
/// Was jedes weitere Zeichen der Eingabe dem Deckel zulegt.
///
/// **Der Deckel galt bis 0.174.0 je STARTSTELLE, und damit gar nicht.** Ein
/// Muster ohne Anker wird an jeder Stelle neu versucht; bei 300 000 Zeichen
/// Stilblatt waren das 300 000 × 400 000 Schritte, also kein Deckel, sondern
/// ein haengender Browser. Gefunden an DuckDuckGos Ergebnisseite: sie laedt
/// `css-vars-ponyfill` nach und laesst es ihre eigenen Blaetter mit
/// verschachtelten Mustern zerlegen.
///
/// Jetzt gilt er fuer den GANZEN Lauf, waechst aber mit der Eingabe: eine
/// ehrliche Suche kostet ungefaehr einen Schritt je Zeichen, und ein Muster,
/// das viel mehr braucht, sucht nicht mehr, sondern zaehlt Wege ab.
```

## L452-454 · `let save = st.caps.clone();`

```
// Die Erfassungen der gescheiterten Alternative muessen weg,
// sonst traegt der Treffer Spuren eines Weges, den er nicht
// gegangen ist.
```

## L483-485 · `let save = st.caps.clone();`

```
// Rueckschau: jede Startstelle davor probieren, die genau hier
// endet. Naiv, aber richtig — und Rueckschau ist selten genug,
// dass die Naivitaet nicht auffaellt.
```

## L527-529 · `if p == pos { return if done + 1 >= min { k(st, p) } else { None }; }`

```
// Ein Durchgang, der NICHTS verbraucht hat, wuerde ewig laufen —
// `(a?)*` ist gueltiges JavaScript. Abbrechen, sobald die
// Mindestzahl erreicht ist.
```

## L548-560 · `fn anchored(n: &Node) -> bool {`

```
/// Sucht ab `start`. `sticky` erzwingt einen Treffer GENAU dort.
/// Faengt JEDER Weg durch das Muster mit `^` an?
///
/// **Dann gibt es genau eine Startstelle, und das ist keine Feinheit.**
/// Ohne diese Frage probiert `exec` ein verankertes Muster an jeder
/// Stelle der Eingabe — jede davon scheitert sofort, aber sie kostet.
/// Ein handgeschriebener Parser ruft sein `^…` einmal je Wortmarke auf
/// dem RESTTEXT auf, und aus linear wird quadratisch: bei DuckDuckGos
/// 300-KB-Blatt und dem `css-vars-ponyfill` davor lief der Lauf in
/// 25 Minuten nicht zu Ende.
///
/// Mit `m` gilt die Verankerung je ZEILE, dann stimmt die Abkuerzung
/// nicht mehr — deshalb steht die Flagge in der Bedingung.
```

## L576-577 · `let mut st = St { s, f: self.flags, caps: alloc::vec![None; self.group_count + 1],`

```
// Die Fanggruppen EINMAL, nicht je Startstelle: eine Allokation je
// Zeichen der Eingabe ist auf einem Stilblatt teurer als das Suchen.
```

## L594 · `use super::interp::{C, Interp, Realm};`

```
// ── Die JS-Seite ────────────────────────────────────────────────────────────
```

## L605 · `pub fn make(i: &mut Interp, pattern: &str, flags: &str) -> C<Value> {`

```
/// Ein RegExp-Objekt aus Muster und Flaggen.
```

## L615-616 · `o.define("lastIndex", Prop { value: Some(Value::Num(0.0)), get: None, set: None,`

```
// `lastIndex` ist SCHREIBBAR und gehoert dem Objekt, nicht dem
// Prototyp — daran haengt, dass `g`-Suchen weiterlaufen.
```

## L619-623 · `}`

```
// `source`, `flags` und die acht Flaggen stehen NICHT hier: sie sind
// Leser auf `RegExp.prototype` (ES 22.2.6). Am Ausdruck selbst waeren
// sie eigene Eigenschaften — `Object.defineProperty(re, "flags", …)`
// schluege dann fehl statt zu greifen, und `Object.keys(re)` faende
// neun Namen statt keinen.
```

## L628-629 · `fn match_result(i: &mut Interp, re: &Regex, chars: &[char], m: &Match, input: &str) -> Value {`

```
/// Ein Treffer als JS-Array — mit `index`, `input` und `groups` daran, so wie
/// `exec` es liefert.
```

## L659 · `fn do_exec(i: &mut Interp, this: &Value, s: &str) -> C<Value> {`

```
/// `exec` mit der `lastIndex`-Buchhaltung, die `g`/`y` verlangen.
```

## L689 · `fn expand(rep: &str, chars: &[char], m: &Match) -> String {`

```
/// `$1`, `$&`, `$\`` und `$'` in einer Ersetzung auffuellen.
```

## L719-724 · `fn record(i: &mut Interp, chars: &[char], m: &Match) {`

```
/// Alle Treffer einer Suche — die Grundlage von `match`, `replace` und
/// `split`. Ein LEERER Treffer muss die Stelle weiterschieben, sonst laeuft
/// die Schleife ewig (`"abc".replace(/x*/g, "-")`).
/// Den Treffer fuer die annexB-Statiken festhalten. Eine Stelle, damit
/// `exec`, `match`, `replace` und `split` nicht drei verschiedene Wahrheiten
/// hinterlassen.
```

## L742-744 · `fn all_matches_global(re: &Regex, chars: &[char]) -> Vec<Match> {`

```
/// Wie `all_matches`, aber ohne Ruecksicht auf die `g`-Flagge — `matchAll`
/// hat sie schon geprueft, und ein aus einer Zeichenkette gebautes Muster
/// traegt sie nicht.
```

## L787-791 · `macro_rules! flag_get {`

```
// ── Die Leser (ES 22.2.6) ────────────────────────────────────────────
//
// Auf `RegExp.prototype` selbst geben sie `undefined` (bzw. `"(?:)"`)
// statt zu werfen — die Ausnahme steht so in der Spezifikation, weil der
// Prototyp selbst kein Ausdruck ist.
```

## L817-818 · `"hasIndices" => |_| false,`

```
// Weder `d` noch `v` sind gebaut; sie sind trotzdem da und sagen
// ehrlich `false`, statt zu fehlen.
```

## L834-836 · `let g = native(Some(fp.clone()), |i, t, _| {`

```
// `flags` ist KEIN eigener Zustand, sondern die Zusammenfassung der
// acht Leser — und liest sie einzeln, damit ein ueberschriebener
// Leser durchschlaegt. Genau das prueft test262.
```

## L853-855 · `def(&proto, "compile", |i, t, a| {`

```
// annexB: `compile` baut den Ausdruck IM SELBEN Objekt neu. Moeglich,
// weil die Art des Objekts veraenderlich ist — der Ausdruck liegt in
// `ObjKind::Regex`, nicht in einer eingefrorenen Eigenschaft.
```

## L858-860 · `let own_proto = matches!(&t, Value::Obj(o)`

```
// Nur ein Ausdruck, den `%RegExp%` SELBST gebaut hat, darf neu
// uebersetzt werden (`[[LegacyFeaturesEnabled]]`). Eine Unterklasse
// erkennt man am Prototyp.
```

## L918 · `def(&ctor, "escape", |i, _, a| {`

```
// ── `RegExp.escape` (ES 2025) ────────────────────────────────────────
```

## L925-927 · `if k == 0 && c.is_ascii_alphanumeric() {`

```
// Die ERSTE Stelle wird auch dann geschuetzt, wenn sie harmlos
// aussieht: sonst waere `escape("ab")` in `\1ab` einlesbar als
// Rueckverweis.
```

## L940-942 · `c if ",-=<>#&!%:;@~'\"".contains(c) || c.is_whitespace()`

```
// Die Liste steht in der Spezifikation (`otherPunctuators`),
// dazu Leerraum und Zeilenenden. Bis 0xFF als `\xHH`, darueber
// als `\uXXXX` — nicht umgekehrt.
```

## L956-960 · `{`

```
// ── Die annexB-Statiken ──────────────────────────────────────────────
//
// Sie liegen am KONSTRUKTOR, nicht am Ausdruck, und lesen den letzten
// erfolgreichen Treffer aus `Interp::last_match`. Ein Leser auf einem
// anderen `this` wirft — genau das prueft test262.
```

## L982 · `let set = native(Some(fp.clone()), |i, t, a| {`

```
// `input` ist als einziges auch SCHREIBBAR.
```

## L1006 · `let sp = realm.string_proto.clone();`

```
// ── Die String-Methoden, die ein Muster nehmen ───────────────────────
```

## L1025-1028 · `def(&sp, "matchAll", |i, t, a| {`

```
// `matchAll` sammelt EIFRIG ein und gibt einen Feld-Iterator darueber.
// Ein echter Motor laeuft faul und sieht Aenderungen an `lastIndex`
// waehrenddessen; benannt statt verschwiegen — die Schleife
// `for (const m of s.matchAll(re))` sieht keinen Unterschied.
```

## L1034-1035 · `if !matches!(arg, Value::Undefined | Value::Null) {`

```
// Ein musteraehnliches Objekt zaehlt auch: `IsRegExp` fragt
// `Symbol.match`, und dann MUSS `flags` da sein.
```

## L1081 · `let parts: Vec<Value> = match &sep {`

```
// Zeichenkettenteilung — wie bisher.
```

## L1103-1104 · `for c in m.caps.iter().skip(1) {`

```
// Erfasste Gruppen landen MIT in der Liste — das ist die Regel,
// an der `"a1b".split(/(\d)/)` haengt.
```

## L1117 · `fn as_regex(i: &mut Interp, v: Option<&Value>) -> C<Value> {`

```
/// Ein Argument als RegExp — eine Zeichenkette wird zu einem Muster.
```

## L1126 · `fn escape_literal(s: &str) -> String {`

```
/// Eine Zeichenkette, die als Muster genau sich selbst treffen soll.
```

## L1142 · `if compiled(&pat).is_none() {`

```
// Zeichenkette als Muster: der einfache Fall, ohne Motor.
```

## L1176 · `let mut args: Vec<Value> = m.caps.iter().map(|c| match c {`

```
// Der Ersetzer bekommt Treffer, Gruppen, Stelle und Text.
```

## L1198 · `#[derive(Clone, Copy)]`

```
/// Welches Stueck des letzten Treffers eine annexB-Statik liest.
```

## L1202-1204 · `fn legacy_this(i: &mut Interp, t: &Value) -> C<()> {`

```
/// Die Statiken gehoeren dem KONSTRUKTOR. Auf einem anderen `this` werfen sie
/// — sonst waere `RegExp.__lookupGetter__("$1").call({})` ein stiller Leser
/// auf fremdem Zustand.
```

## L1226-1227 · `fn is_regexp_proto(i: &Interp, t: &Value) -> bool {`

```
/// Ist das GENAU `RegExp.prototype`? Die Leser geben dort `undefined` statt
/// zu werfen.
```

