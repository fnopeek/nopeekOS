# `tools/wasm/beak-engine/src/js/lexer.rs` @ 5e0102684

## L1-17 · `use alloc::string::String;`

```
//! Der Tokenizer.
//!
//! Auf Abruf, nicht im Voraus — und das ist keine Stilfrage: ob `/` eine
//! Division oder der Anfang eines regulaeren Ausdrucks ist, kann der Lexer
//! nicht allein entscheiden (`a /b/ g` gegen `return /b/g`). Nur der Parser
//! weiss, ob an dieser Stelle ein Operand oder ein Operator erwartet wird, also
//! sagt er es beim Holen (`next(regex_ok)`). Ein Lexer, der vorher durchlaeuft,
//! muesste diese Frage raten.
//!
//! Zwei weitere Dinge, die hier und nicht im Parser sitzen:
//!
//! - **`newline_before`** an jedem Token. Die automatische Semikolon-Einfuegung
//!   haengt daran, und der Parser kann den Zeilenumbruch nicht mehr sehen,
//!   wenn die Leerzeichen erst weg sind.
//! - **Template-Fortsetzung.** `` `a${x}b` `` ist EIN Literal mit einem Loch;
//!   nach dem `}` muss weiter im Template gelesen werden, was nur geht, wenn
//!   der Parser es anfordert (`next_template_part`).
```

## L25-27 · `Keyword(Kw),`

```
/// Ein reserviertes Wort. Getrennt von `Ident`, weil `class` und `x` an
/// derselben Stelle voellig Verschiedenes bedeuten — und zusammengelegt
/// haette jede Pruefung einen Stringvergleich statt eines Sprungs.
```

## L32-34 · `Regex(String, String),`

```
/// Rohtext + Flags. Der Inhalt wird NICHT geprueft: das ist die Aufgabe
/// der RegExp-Maschine, und ein Parser, der es doch tut, lehnt Muster ab,
/// die er nur nicht kennt.
```

## L36-42 · `Template { cooked: Option<String>, raw: String, has_sub: bool },`

```
/// Ein Stueck Template: der entschluesselte Text, der Rohtext, und ob nach
/// ihm eine Einsetzung `${` folgt (sonst endet das Literal hier).
///
/// Das Feld hiess `tail` und das war eine Falle: in ESTree bedeutet `tail`
/// GENAU DAS GEGENTEIL (das letzte Stueck). Der Name hat drei Skripte des
/// Zielkorpus gekostet — nach `${` ist `/` ein Regex, und die Pruefung
/// unten hatte die Bedingung falsch herum gelesen.
```

## L53-54 · `Let, Static, Async, Get, Set, Of, As, From, Target, Meta,`

```
// Kontextabhaengig: nur an bestimmten Stellen reserviert. Sie kommen hier
// als Keyword an und der Parser darf sie als Bezeichner zurueckbiegen.
```

## L59-61 · `pub fn is_reserved(self) -> bool {`

```
/// Ist das Wort ueberall reserviert? `let`/`static`/`async`/`of` sind es
/// NICHT — `var of = 1` ist gueltiges JavaScript, und ein Parser, der das
/// ablehnt, scheitert an echtem Code, nicht an schlechtem.
```

## L115 · `Hash,`

```
/// `#name` — der private Name in einer Klasse.
```

## L124-125 · `pub newline_before: bool,`

```
/// Stand vor diesem Token ein Zeilenumbruch? Die Semikolon-Einfuegung
/// haengt allein daran.
```

## L138 · `text: &'a str,`

```
/// Der Quelltext als `str`, fuer Ausschnitte mit Mehrbyte-Zeichen.
```

## L140-142 · `pub legacy_octal: bool,`

```
/// War die zuletzt gelesene Zahl ein Alt-Oktal (`0755`) oder eine
/// Nicht-Oktal-Ziffernfolge mit fuehrender Null (`089`)? Im strengen Modus
/// beides ein Fruehfehler — und ob der gilt, weiss nur der Parser.
```

## L146-153 · `fn id_start(c: char) -> bool {`

```
/// Ist `c` ein Zeichen, mit dem ein Bezeichner anfangen darf?
///
/// Nicht die volle Unicode-Tabelle: ASCII exakt, und ab 0x80 wird alles
/// zugelassen. Das ist bewusst zu grosszuegig statt zu streng — ein Parser,
/// der einen gueltigen Bezeichner ablehnt, verliert die ganze Datei, waehrend
/// ein zu weit gefasster Bezeichner nur ein Programm annimmt, das ohnehin
/// niemand ausliefert. Die Tabelle (ID_Start/ID_Continue) kostet ~40 KB und
/// wandert erst herein, wenn eine gemessene Seite sie braucht.
```

## L156-159 · `!matches!(c, '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{00A0}' | '\u{1680}'`

```
// Ab 0x80 alles ZULASSEN, ausser dem, was nachweislich Leerraum oder
// Zeilentrenner ist. Ohne diese Ausnahme wird U+2028 zum Bezeichner und
// `1\u{2028}2` liest sich als eine Zahl mit Buchstaben dahinter — genau
// so ist es aufgefallen (test262 `line-terminators/between-tokens-ls`).
```

## L170-171 · `if text.as_bytes().starts_with(b"#!") {`

```
// `#!/usr/bin/env node` — nur in der allerersten Zeile, sonst ist `#`
// der private Name einer Klasse.
```

## L187-188 · `pub fn src_text(&self) -> &'a str { self.text }`

```
/// Der Quelltext. Die Direktivenpruefung (`"use strict"`) muss den ROHEN
/// Ausschnitt sehen: `"use\u0020strict"` ist keine Direktive.
```

## L191 · `fn char_at(&self, i: usize) -> (char, usize) {`

```
/// Zeichen ab `pos`, mit seiner UTF-8-Laenge.
```

## L193-194 · `if i >= self.text.len() || !self.text.is_char_boundary(i) { return ('\0', 1); }`

```
// `self.text[i..]` PANIKT auf einer Nicht-Zeichengrenze. Ein Lexer
// darf an keiner Eingabe platzen — im Zweifel ein Byte weiter.
```

## L202-208 · `fn skip_trivia(&mut self) -> Result<bool, LexError> {`

```
/// Leerraum und Kommentare ueberspringen; meldet, ob dabei eine neue Zeile
/// begann.
///
/// Enthaelt die beiden Altlasten aus Annex B, und sie sind keine Kuriositaet:
/// `<!--` und `-->` sind 375 der 445 Dateien, die dieser Parser im ersten
/// test262-Lauf faelschlich ablehnte. Ein Browser, der sie nicht kennt,
/// verliert bei jedem alten Skript-Block die ganze Datei.
```

## L221 · `b'<' if self.at(self.pos + 1) == b'!' && self.at(self.pos + 2) == b'-'`

```
// `<!--` ist ein Zeilenkommentar (Annex B B.1.1).
```

## L227-228 · `b'-' if (nl || self.pos == 0) && self.at(self.pos + 1) == b'-'`

```
// `-->` ebenso, aber NUR am Zeilenanfang: sonst waere `a-->b`
// kein Dekrement mehr, und das ist gueltiges JavaScript.
```

## L240-241 · `if matches!(self.src[self.pos], b'\n' | b'\r') { nl = true; }`

```
// Ein Zeilenumbruch IM Blockkommentar zaehlt fuer die
// Semikolon-Einfuegung — `return /*\n*/ x` gibt undefined.
```

## L243-245 · `else if self.src[self.pos] == 0xE2 && self.at(self.pos + 1) == 0x80`

```
// U+2028/U+2029 sind ebenfalls Zeilenumbrueche, und
// auch IM Blockkommentar zaehlen sie fuer die
// Semikolon-Einfuegung.
```

## L257-258 · `'\u{2028}' | '\u{2029}' => { nl = true; self.pos += n; }`

```
// U+2028/U+2029 sind Zeilenumbrueche, \u{FEFF} und die
// Unicode-Leerzeichen sind Leerraum.
```

## L303-305 · `let mut had_escape = false;`

```
// Ein Bezeichner darf `\u{...}`-Fluchten enthalten, und sie sind fuer
// die Bedeutung gleichwertig: `if` IST `if`. Das ist der Grund,
// warum hier zusammengebaut und erst danach nach Keywords gefragt wird.
```

## L325-328 · `Some(k) if !had_escape => Ok(Tok::Keyword(k)),`

```
// Ein Schluesselwort, das ueber eine Flucht geschrieben wurde, ist
// KEIN Schluesselwort mehr (Early Error) — aber es ist auch kein
// gueltiger Bezeichner. Wir geben es als Bezeichner zurueck; die
// Regel gehoert in die spaetere Fruehfehlerpruefung, nicht hierher.
```

## L348-350 · `return Ok(char::from_u32(v).unwrap_or('\u{FFFD}'));`

```
// Ein einzelnes Surrogat ist ein gueltiges JS-Zeichen, aber kein
// gueltiges `char`. Als Ersatzzeichen fuehren, statt die Datei zu
// verlieren.
```

## L382 · `fn escape(&mut self) -> Result<Option<char>, LexError> {`

```
/// Eine Flucht nach `\`. `None` = Zeilenfortsetzung (traegt nichts bei).
```

## L402-403 · `b'0'..=b'7' => {`

```
// Legacy-Oktal (`\101`). Im strengen Modus ein Fruehfehler; die
// Pruefung gehoert dorthin, nicht in den Lexer.
```

## L423 · `pub fn template_part(&mut self) -> Result<Tok, LexError> {`

```
/// Ein Stueck Template ab der aktuellen Stelle (nach `` ` `` oder `}`).
```

## L445-454 · `let nx = self.at(self.pos);`

```
// Ein getaggtes Template darf ungueltige Fluchten enthalten;
// dann ist `cooked` undefined und nur `raw` gilt (ES2018).
// Deshalb wird hier NICHT abgebrochen.
//
// **In einem Template sind die alten Zahlfluchten verboten**
// (ES 12.9.6, TemplateCharacter): `\1`–`\9` gar nicht, und
// `\0` nur, solange keine Ziffer folgt. `escape()` nimmt sie
// an, weil sie in einer gewoehnlichen Zeichenkette im lockeren
// Modus erlaubt sind — die Stelle, die den Unterschied kennt,
// ist diese hier.
```

## L460-465 · `let after_slash = self.pos;`

```
// Die Stelle der FLUCHTKENNUNG merken. Nach einem Fehler steht
// `self.pos` irgendwo mitten in der halbgelesenen Folge —
// `+= 1` von dort aus frass bei `` `\u0` `` das schliessende
// Akzentzeichen und machte aus einer ungueltigen Flucht ein
// „unterminated template". Aufgesetzt wird direkt hinter der
// Kennung; der Rest ist gewoehnlicher Text.
```

## L480-482 · `fn digits(&mut self, radix: u32) -> Result<usize, LexError> {`

```
/// Ziffern zur Basis `radix` lesen, mit den Regeln fuer den Trenner:
/// `_` muss ZWISCHEN zwei Ziffern stehen. `1_0` ja, `1_`/`_1`/`1__0` nein.
/// Liefert die Anzahl gelesener Ziffern.
```

## L490 · `if !any || prev_sep { return Err(LexError { msg: "misplaced numeric separator", at: self.pos }); }`

```
// Kein Trenner am Anfang, keiner doppelt, keiner am Ende.
```

## L528-530 · `let lead_zero = self.src[self.pos] == b'0';`

```
// Dezimal, inklusive Legacy-Oktal (`0755`) und der Nicht-Oktal-Form
// (`089`) — beides im strengen Modus ein Fruehfehler, aber kein
// Lexfehler. Beide duerfen keinen Trenner tragen, deshalb erst pruefen.
```

## L554 · `self.digits(10)?;`

```
// `_` gilt auch hier: `1e1_0` ist gueltig (ES2021).
```

## L559 · `if self.legacy_octal || (lead_zero && self.pos > start + 1) {`

```
// `01n` gibt es nicht — ein BigInt hat keine fuehrende Null.
```

## L566-568 · `let (c, _) = self.char_at(self.pos);`

```
// Eine Ziffer direkt hinter einer Zahl ist ein Fehler (`3in`), sonst
// liest der Parser `3` und `in` und baut daraus etwas Sinnvolles, das
// im Quelltext nicht stand.
```

## L588-590 · `b'\\' => {`

```
// Ein `\\` schuetzt EIN ZEICHEN, nicht ein Byte: `/\\ä/` hat
// hinter dem Schraegstrich zwei Bytes, und `+= 2` landete
// mitten darin.
```

## L624-625 · `let (p, n): (P, usize) = if s.len() >= 4 && &s[..4] == b">>>=" { (UShrEq, 4) }`

```
// Vier Zeichen zuerst, dann drei, dann zwei — sonst wird `>>>=` als
// `>>>` und `=` gelesen.
```

## L650-651 · `else if two(b'?', b'.') && !self.at(self.pos + 2).is_ascii_digit() { (QuestionDot, 2) }`

```
// `?.` NUR wenn keine Ziffer folgt: `a?.5:b` ist der Bedingungs-
// operator mit `.5`, nicht optionales Verketten.
```

## L669-672 · `b'#' => {`

```
// `# x` gibt es nicht: zwischen dem Zeichen und dem Namen
// darf nichts stehen (ES 12.6.1). Die Pruefung MUSS hier
// sitzen — eine Zeile spaeter hat `skip_trivia` den
// Leerraum schon geschluckt und der Unterschied ist weg.
```

## L691-692 · `fn parse_f64(s: &str) -> f64 {`

```
/// Dezimalzahl nach f64. `core` hat `str::parse::<f64>()`, und das ist die
/// korrekt gerundete Umwandlung — kein Grund, eine eigene zu schreiben.
```

## L695-696 · `if s.len() > 1 && s.starts_with('0') && s.bytes().all(|b| (b'0'..=b'7').contains(&b)) {`

```
// Legacy-Oktal (`0755`) faellt hier herein: fuehrende Null + nur Ziffern
// 0-7 wird als Oktal gelesen, alles andere als Dezimal (`089` = 89).
```

## L706-707 · `#[cfg(test)]`

```
/// Alle Token einer Quelle, ohne Parser-Rueckmeldung (nur fuer Tests: der
/// Parser holt selbst, weil nur er `regex_ok` kennt).
```

