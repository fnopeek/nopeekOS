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

use alloc::string::String;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Eof,
    Ident(String),
    /// Ein reserviertes Wort. Getrennt von `Ident`, weil `class` und `x` an
    /// derselben Stelle voellig Verschiedenes bedeuten — und zusammengelegt
    /// haette jede Pruefung einen Stringvergleich statt eines Sprungs.
    Keyword(Kw),
    Num(f64),
    BigInt(String),
    Str(String),
    /// Rohtext + Flags. Der Inhalt wird NICHT geprueft: das ist die Aufgabe
    /// der RegExp-Maschine, und ein Parser, der es doch tut, lehnt Muster ab,
    /// die er nur nicht kennt.
    Regex(String, String),
    /// Ein Stueck Template: der entschluesselte Text, der Rohtext, und ob nach
    /// ihm eine Einsetzung `${` folgt (sonst endet das Literal hier).
    ///
    /// Das Feld hiess `tail` und das war eine Falle: in ESTree bedeutet `tail`
    /// GENAU DAS GEGENTEIL (das letzte Stueck). Der Name hat drei Skripte des
    /// Zielkorpus gekostet — nach `${` ist `/` ein Regex, und die Pruefung
    /// unten hatte die Bedingung falsch herum gelesen.
    Template { cooked: Option<String>, raw: String, has_sub: bool },
    Punct(P),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kw {
    Await, Break, Case, Catch, Class, Const, Continue, Debugger, Default, Delete,
    Do, Else, Enum, Export, Extends, False, Finally, For, Function, If, Import,
    In, Instanceof, New, Null, Return, Super, Switch, This, Throw, True, Try,
    Typeof, Var, Void, While, With, Yield,
    // Kontextabhaengig: nur an bestimmten Stellen reserviert. Sie kommen hier
    // als Keyword an und der Parser darf sie als Bezeichner zurueckbiegen.
    Let, Static, Async, Get, Set, Of, As, From, Target, Meta,
}

impl Kw {
    /// Ist das Wort ueberall reserviert? `let`/`static`/`async`/`of` sind es
    /// NICHT — `var of = 1` ist gueltiges JavaScript, und ein Parser, der das
    /// ablehnt, scheitert an echtem Code, nicht an schlechtem.
    pub fn is_reserved(self) -> bool {
        !matches!(self, Kw::Let | Kw::Static | Kw::Async | Kw::Get | Kw::Set
            | Kw::Of | Kw::As | Kw::From | Kw::Target | Kw::Meta)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Kw::Await=>"await", Kw::Break=>"break", Kw::Case=>"case", Kw::Catch=>"catch",
            Kw::Class=>"class", Kw::Const=>"const", Kw::Continue=>"continue",
            Kw::Debugger=>"debugger", Kw::Default=>"default", Kw::Delete=>"delete",
            Kw::Do=>"do", Kw::Else=>"else", Kw::Enum=>"enum", Kw::Export=>"export",
            Kw::Extends=>"extends", Kw::False=>"false", Kw::Finally=>"finally",
            Kw::For=>"for", Kw::Function=>"function", Kw::If=>"if", Kw::Import=>"import",
            Kw::In=>"in", Kw::Instanceof=>"instanceof", Kw::New=>"new", Kw::Null=>"null",
            Kw::Return=>"return", Kw::Super=>"super", Kw::Switch=>"switch", Kw::This=>"this",
            Kw::Throw=>"throw", Kw::True=>"true", Kw::Try=>"try", Kw::Typeof=>"typeof",
            Kw::Var=>"var", Kw::Void=>"void", Kw::While=>"while", Kw::With=>"with",
            Kw::Yield=>"yield", Kw::Let=>"let", Kw::Static=>"static", Kw::Async=>"async",
            Kw::Get=>"get", Kw::Set=>"set", Kw::Of=>"of", Kw::As=>"as", Kw::From=>"from",
            Kw::Target=>"target", Kw::Meta=>"meta",
        }
    }
}

fn keyword(s: &str) -> Option<Kw> {
    Some(match s {
        "await"=>Kw::Await, "break"=>Kw::Break, "case"=>Kw::Case, "catch"=>Kw::Catch,
        "class"=>Kw::Class, "const"=>Kw::Const, "continue"=>Kw::Continue,
        "debugger"=>Kw::Debugger, "default"=>Kw::Default, "delete"=>Kw::Delete,
        "do"=>Kw::Do, "else"=>Kw::Else, "enum"=>Kw::Enum, "export"=>Kw::Export,
        "extends"=>Kw::Extends, "false"=>Kw::False, "finally"=>Kw::Finally,
        "for"=>Kw::For, "function"=>Kw::Function, "if"=>Kw::If, "import"=>Kw::Import,
        "in"=>Kw::In, "instanceof"=>Kw::Instanceof, "new"=>Kw::New, "null"=>Kw::Null,
        "return"=>Kw::Return, "super"=>Kw::Super, "switch"=>Kw::Switch, "this"=>Kw::This,
        "throw"=>Kw::Throw, "true"=>Kw::True, "try"=>Kw::Try, "typeof"=>Kw::Typeof,
        "var"=>Kw::Var, "void"=>Kw::Void, "while"=>Kw::While, "with"=>Kw::With,
        "yield"=>Kw::Yield, "let"=>Kw::Let, "static"=>Kw::Static, "async"=>Kw::Async,
        "get"=>Kw::Get, "set"=>Kw::Set, "of"=>Kw::Of, "as"=>Kw::As, "from"=>Kw::From,
        "target"=>Kw::Target, "meta"=>Kw::Meta,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P {
    LBrace, RBrace, LParen, RParen, LBracket, RBracket,
    Semi, Comma, Dot, Ellipsis, Colon, Question, QuestionDot, QuestionQuestion,
    Arrow, Inc, Dec,
    Plus, Minus, Star, StarStar, Slash, Percent,
    Lt, Gt, LtEq, GtEq, EqEq, NotEq, EqEqEq, NotEqEq,
    Shl, Shr, UShr, Amp, Pipe, Caret, Bang, Tilde, AmpAmp, PipePipe,
    Eq, PlusEq, MinusEq, StarEq, SlashEq, PercentEq, StarStarEq,
    ShlEq, ShrEq, UShrEq, AmpEq, PipeEq, CaretEq,
    AmpAmpEq, PipePipeEq, QuestionQuestionEq,
    /// `#name` — der private Name in einer Klasse.
    Hash,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
    /// Stand vor diesem Token ein Zeilenumbruch? Die Semikolon-Einfuegung
    /// haengt allein daran.
    pub newline_before: bool,
}

#[derive(Debug)]
pub struct LexError {
    pub msg: &'static str,
    pub at: usize,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pub pos: usize,
    /// Der Quelltext als `str`, fuer Ausschnitte mit Mehrbyte-Zeichen.
    text: &'a str,
    /// War die zuletzt gelesene Zahl ein Alt-Oktal (`0755`) oder eine
    /// Nicht-Oktal-Ziffernfolge mit fuehrender Null (`089`)? Im strengen Modus
    /// beides ein Fruehfehler — und ob der gilt, weiss nur der Parser.
    pub legacy_octal: bool,
}

/// Ist `c` ein Zeichen, mit dem ein Bezeichner anfangen darf?
///
/// Nicht die volle Unicode-Tabelle: ASCII exakt, und ab 0x80 wird alles
/// zugelassen. Das ist bewusst zu grosszuegig statt zu streng — ein Parser,
/// der einen gueltigen Bezeichner ablehnt, verliert die ganze Datei, waehrend
/// ein zu weit gefasster Bezeichner nur ein Programm annimmt, das ohnehin
/// niemand ausliefert. Die Tabelle (ID_Start/ID_Continue) kostet ~40 KB und
/// wandert erst herein, wenn eine gemessene Seite sie braucht.
fn id_start(c: char) -> bool {
    if c.is_ascii() { return c.is_ascii_alphabetic() || c == '$' || c == '_'; }
    // Ab 0x80 alles ZULASSEN, ausser dem, was nachweislich Leerraum oder
    // Zeilentrenner ist. Ohne diese Ausnahme wird U+2028 zum Bezeichner und
    // `1\u{2028}2` liest sich als eine Zahl mit Buchstaben dahinter — genau
    // so ist es aufgefallen (test262 `line-terminators/between-tokens-ls`).
    !matches!(c, '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{00A0}' | '\u{1680}'
        | '\u{2000}'..='\u{200B}' | '\u{202F}' | '\u{205F}' | '\u{3000}')
}
fn id_part(c: char) -> bool {
    id_start(c) || c.is_ascii_digit() || c == '\u{200C}' || c == '\u{200D}'
}

impl<'a> Lexer<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut pos = 0;
        // `#!/usr/bin/env node` — nur in der allerersten Zeile, sonst ist `#`
        // der private Name einer Klasse.
        if text.as_bytes().starts_with(b"#!") {
            pos = 2;
            let b = text.as_bytes();
            while pos < b.len() {
                if matches!(b[pos], b'\n' | b'\r') { break; }
                if b[pos] == 0xE2 && pos + 2 < b.len() && b[pos + 1] == 0x80
                    && matches!(b[pos + 2], 0xA8 | 0xA9) { break; }
                pos += 1;
            }
        }
        Lexer { src: text.as_bytes(), pos, text, legacy_octal: false }
    }

    fn at(&self, i: usize) -> u8 { if i < self.src.len() { self.src[i] } else { 0 } }

    /// Der Quelltext. Die Direktivenpruefung (`"use strict"`) muss den ROHEN
    /// Ausschnitt sehen: `"use\u0020strict"` ist keine Direktive.
    pub fn src_text(&self) -> &'a str { self.text }

    /// Zeichen ab `pos`, mit seiner UTF-8-Laenge.
    fn char_at(&self, i: usize) -> (char, usize) {
        match self.text[i..].chars().next() {
            Some(c) => (c, c.len_utf8()),
            None => ('\0', 1),
        }
    }

    /// Leerraum und Kommentare ueberspringen; meldet, ob dabei eine neue Zeile
    /// begann.
    ///
    /// Enthaelt die beiden Altlasten aus Annex B, und sie sind keine Kuriositaet:
    /// `<!--` und `-->` sind 375 der 445 Dateien, die dieser Parser im ersten
    /// test262-Lauf faelschlich ablehnte. Ein Browser, der sie nicht kennt,
    /// verliert bei jedem alten Skript-Block die ganze Datei.
    fn skip_trivia(&mut self) -> Result<bool, LexError> {
        let mut nl = false;
        loop {
            if self.pos >= self.src.len() { return Ok(nl); }
            let b = self.src[self.pos];
            match b {
                b' ' | b'\t' | 0x0B | 0x0C => { self.pos += 1; }
                b'\n' | b'\r' => { nl = true; self.pos += 1; }
                b'/' if self.at(self.pos + 1) == b'/' => {
                    while self.pos < self.src.len()
                        && !matches!(self.src[self.pos], b'\n' | b'\r') { self.pos += 1; }
                }
                // `<!--` ist ein Zeilenkommentar (Annex B B.1.1).
                b'<' if self.at(self.pos + 1) == b'!' && self.at(self.pos + 2) == b'-'
                    && self.at(self.pos + 3) == b'-' => {
                    while self.pos < self.src.len()
                        && !matches!(self.src[self.pos], b'\n' | b'\r') { self.pos += 1; }
                }
                // `-->` ebenso, aber NUR am Zeilenanfang: sonst waere `a-->b`
                // kein Dekrement mehr, und das ist gueltiges JavaScript.
                b'-' if (nl || self.pos == 0) && self.at(self.pos + 1) == b'-'
                    && self.at(self.pos + 2) == b'>' => {
                    while self.pos < self.src.len()
                        && !matches!(self.src[self.pos], b'\n' | b'\r') { self.pos += 1; }
                }
                b'/' if self.at(self.pos + 1) == b'*' => {
                    self.pos += 2;
                    loop {
                        if self.pos >= self.src.len() {
                            return Err(LexError { msg: "unterminated comment", at: self.pos });
                        }
                        // Ein Zeilenumbruch IM Blockkommentar zaehlt fuer die
                        // Semikolon-Einfuegung — `return /*\n*/ x` gibt undefined.
                        if matches!(self.src[self.pos], b'\n' | b'\r') { nl = true; }
                        // U+2028/U+2029 sind ebenfalls Zeilenumbrueche, und
                        // auch IM Blockkommentar zaehlen sie fuer die
                        // Semikolon-Einfuegung.
                        else if self.src[self.pos] == 0xE2 && self.at(self.pos + 1) == 0x80
                            && matches!(self.at(self.pos + 2), 0xA8 | 0xA9) { nl = true; }
                        if self.src[self.pos] == b'*' && self.at(self.pos + 1) == b'/' {
                            self.pos += 2; break;
                        }
                        self.pos += 1;
                    }
                }
                _ if b >= 0x80 => {
                    let (c, n) = self.char_at(self.pos);
                    match c {
                        // U+2028/U+2029 sind Zeilenumbrueche, \u{FEFF} und die
                        // Unicode-Leerzeichen sind Leerraum.
                        '\u{2028}' | '\u{2029}' => { nl = true; self.pos += n; }
                        '\u{FEFF}' | '\u{00A0}' | '\u{1680}'
                        | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => {
                            self.pos += n;
                        }
                        _ => return Ok(nl),
                    }
                }
                _ => return Ok(nl),
            }
        }
    }

    pub fn next(&mut self, regex_ok: bool) -> Result<Token, LexError> {
        let newline_before = self.skip_trivia()?;
        let start = self.pos;
        if self.pos >= self.src.len() {
            return Ok(Token { tok: Tok::Eof, start, end: start, newline_before });
        }
        let tok = self.scan(regex_ok)?;
        Ok(Token { tok, start, end: self.pos, newline_before })
    }

    fn scan(&mut self, regex_ok: bool) -> Result<Tok, LexError> {
        let b = self.src[self.pos];
        match b {
            b'"' | b'\'' => self.string(b),
            b'`' => { self.pos += 1; self.template_part() }
            b'0'..=b'9' => self.number(),
            b'.' if self.at(self.pos + 1).is_ascii_digit() => self.number(),
            b'/' if regex_ok => self.regex(),
            _ => {
                let (c, n) = self.char_at(self.pos);
                if id_start(c) || (c == '\\' && self.at(self.pos + 1) == b'u') {
                    return self.ident();
                }
                let _ = n;
                self.punct()
            }
        }
    }

    fn ident(&mut self) -> Result<Tok, LexError> {
        let mut s = String::new();
        // Ein Bezeichner darf `\u{...}`-Fluchten enthalten, und sie sind fuer
        // die Bedeutung gleichwertig: `if` IST `if`. Das ist der Grund,
        // warum hier zusammengebaut und erst danach nach Keywords gefragt wird.
        let mut had_escape = false;
        loop {
            if self.pos >= self.src.len() { break; }
            if self.src[self.pos] == b'\\' && self.at(self.pos + 1) == b'u' {
                self.pos += 2;
                let c = self.unicode_escape()?;
                if s.is_empty() && !id_start(c) { return Err(LexError { msg: "bad identifier escape", at: self.pos }); }
                if !s.is_empty() && !id_part(c) { return Err(LexError { msg: "bad identifier escape", at: self.pos }); }
                s.push(c);
                had_escape = true;
                continue;
            }
            let (c, n) = self.char_at(self.pos);
            if s.is_empty() { if !id_start(c) { break; } } else if !id_part(c) { break; }
            s.push(c);
            self.pos += n;
        }
        if s.is_empty() { return Err(LexError { msg: "expected identifier", at: self.pos }); }
        match keyword(&s) {
            // Ein Schluesselwort, das ueber eine Flucht geschrieben wurde, ist
            // KEIN Schluesselwort mehr (Early Error) — aber es ist auch kein
            // gueltiger Bezeichner. Wir geben es als Bezeichner zurueck; die
            // Regel gehoert in die spaetere Fruehfehlerpruefung, nicht hierher.
            Some(k) if !had_escape => Ok(Tok::Keyword(k)),
            _ => Ok(Tok::Ident(s)),
        }
    }

    fn unicode_escape(&mut self) -> Result<char, LexError> {
        if self.at(self.pos) == b'{' {
            self.pos += 1;
            let mut v: u32 = 0;
            let mut any = false;
            while self.pos < self.src.len() && self.src[self.pos] != b'}' {
                let d = (self.src[self.pos] as char).to_digit(16)
                    .ok_or(LexError { msg: "bad unicode escape", at: self.pos })?;
                v = v.saturating_mul(16).saturating_add(d);
                if v > 0x10FFFF { return Err(LexError { msg: "unicode escape out of range", at: self.pos }); }
                self.pos += 1; any = true;
            }
            if !any || self.at(self.pos) != b'}' { return Err(LexError { msg: "bad unicode escape", at: self.pos }); }
            self.pos += 1;
            // Ein einzelnes Surrogat ist ein gueltiges JS-Zeichen, aber kein
            // gueltiges `char`. Als Ersatzzeichen fuehren, statt die Datei zu
            // verlieren.
            return Ok(char::from_u32(v).unwrap_or('\u{FFFD}'));
        }
        let mut v: u32 = 0;
        for _ in 0..4 {
            let d = (self.at(self.pos) as char).to_digit(16)
                .ok_or(LexError { msg: "bad unicode escape", at: self.pos })?;
            v = v * 16 + d;
            self.pos += 1;
        }
        Ok(char::from_u32(v).unwrap_or('\u{FFFD}'))
    }

    fn string(&mut self, quote: u8) -> Result<Tok, LexError> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(LexError { msg: "unterminated string", at: self.pos });
            }
            let b = self.src[self.pos];
            if b == quote { self.pos += 1; return Ok(Tok::Str(s)); }
            if matches!(b, b'\n' | b'\r') {
                return Err(LexError { msg: "newline in string", at: self.pos });
            }
            if b == b'\\' { self.pos += 1; if let Some(c) = self.escape()? { s.push(c); } continue; }
            let (c, n) = self.char_at(self.pos);
            s.push(c);
            self.pos += n;
        }
    }

    /// Eine Flucht nach `\`. `None` = Zeilenfortsetzung (traegt nichts bei).
    fn escape(&mut self) -> Result<Option<char>, LexError> {
        if self.pos >= self.src.len() { return Err(LexError { msg: "unterminated escape", at: self.pos }); }
        let b = self.src[self.pos];
        self.pos += 1;
        Ok(Some(match b {
            b'n' => '\n', b't' => '\t', b'r' => '\r', b'b' => '\u{8}',
            b'f' => '\u{C}', b'v' => '\u{B}', b'0' if !self.at(self.pos).is_ascii_digit() => '\0',
            b'x' => {
                let mut v = 0u32;
                for _ in 0..2 {
                    let d = (self.at(self.pos) as char).to_digit(16)
                        .ok_or(LexError { msg: "bad hex escape", at: self.pos })?;
                    v = v * 16 + d; self.pos += 1;
                }
                char::from_u32(v).unwrap_or('\u{FFFD}')
            }
            b'u' => self.unicode_escape()?,
            b'\r' => { if self.at(self.pos) == b'\n' { self.pos += 1; } return Ok(None); }
            b'\n' => return Ok(None),
            // Legacy-Oktal (`\101`). Im strengen Modus ein Fruehfehler; die
            // Pruefung gehoert dorthin, nicht in den Lexer.
            b'0'..=b'7' => {
                let mut v = (b - b'0') as u32;
                let mut n = 1;
                while n < 3 && matches!(self.at(self.pos), b'0'..=b'7') {
                    let next = v * 8 + (self.src[self.pos] - b'0') as u32;
                    if next > 255 { break; }
                    v = next; self.pos += 1; n += 1;
                }
                char::from_u32(v).unwrap_or('\u{FFFD}')
            }
            _ => {
                self.pos -= 1;
                let (c, n) = self.char_at(self.pos);
                self.pos += n;
                c
            }
        }))
    }

    /// Ein Stueck Template ab der aktuellen Stelle (nach `` ` `` oder `}`).
    pub fn template_part(&mut self) -> Result<Tok, LexError> {
        let raw_start = self.pos;
        let mut cooked = String::new();
        let mut bad = false;
        loop {
            if self.pos >= self.src.len() {
                return Err(LexError { msg: "unterminated template", at: self.pos });
            }
            let b = self.src[self.pos];
            if b == b'`' {
                let raw = self.text[raw_start..self.pos].into();
                self.pos += 1;
                return Ok(Tok::Template { cooked: if bad { None } else { Some(cooked) }, raw, has_sub: false });
            }
            if b == b'$' && self.at(self.pos + 1) == b'{' {
                let raw = self.text[raw_start..self.pos].into();
                self.pos += 2;
                return Ok(Tok::Template { cooked: if bad { None } else { Some(cooked) }, raw, has_sub: true });
            }
            if b == b'\\' {
                self.pos += 1;
                // Ein getaggtes Template darf ungueltige Fluchten enthalten;
                // dann ist `cooked` undefined und nur `raw` gilt (ES2018).
                // Deshalb wird hier NICHT abgebrochen.
                match self.escape() {
                    Ok(Some(c)) => cooked.push(c),
                    Ok(None) => {}
                    Err(_) => { bad = true; self.pos += 1; }
                }
                continue;
            }
            let (c, n) = self.char_at(self.pos);
            cooked.push(c);
            self.pos += n;
        }
    }

    /// Ziffern zur Basis `radix` lesen, mit den Regeln fuer den Trenner:
    /// `_` muss ZWISCHEN zwei Ziffern stehen. `1_0` ja, `1_`/`_1`/`1__0` nein.
    /// Liefert die Anzahl gelesener Ziffern.
    fn digits(&mut self, radix: u32) -> Result<usize, LexError> {
        let mut n = 0;
        let mut prev_sep = false;
        let mut any = false;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == b'_' {
                // Kein Trenner am Anfang, keiner doppelt, keiner am Ende.
                if !any || prev_sep { return Err(LexError { msg: "misplaced numeric separator", at: self.pos }); }
                prev_sep = true; self.pos += 1; continue;
            }
            if (c as char).to_digit(radix).is_none() { break; }
            prev_sep = false; any = true; n += 1; self.pos += 1;
        }
        if prev_sep { return Err(LexError { msg: "trailing numeric separator", at: self.pos }); }
        Ok(n)
    }

    fn number(&mut self) -> Result<Tok, LexError> {
        let start = self.pos;
        self.legacy_octal = false;
        let mut is_int_radix = false;
        if self.src[self.pos] == b'0' && self.pos + 1 < self.src.len() {
            let k = self.at(self.pos + 1) | 0x20;
            if matches!(k, b'x' | b'o' | b'b') {
                is_int_radix = true;
                let radix = match k { b'x' => 16, b'o' => 8, _ => 2 };
                self.pos += 2;
                let ds = self.pos;
                if self.digits(radix)? == 0 {
                    return Err(LexError { msg: "missing digits", at: self.pos });
                }
                let mut v = 0f64;
                for &c in &self.src[ds..self.pos] {
                    if c == b'_' { continue; }
                    v = v * radix as f64 + (c as char).to_digit(radix).unwrap_or(0) as f64;
                }
                if self.at(self.pos) == b'n' {
                    self.pos += 1;
                    return Ok(Tok::BigInt(self.text[start..self.pos - 1].into()));
                }
                return Ok(Tok::Num(v));
            }
        }
        let _ = is_int_radix;
        // Dezimal, inklusive Legacy-Oktal (`0755`) und der Nicht-Oktal-Form
        // (`089`) — beides im strengen Modus ein Fruehfehler, aber kein
        // Lexfehler. Beide duerfen keinen Trenner tragen, deshalb erst pruefen.
        let lead_zero = self.src[self.pos] == b'0';
        if lead_zero && self.at(self.pos + 1).is_ascii_digit() {
            self.legacy_octal = true;
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() { self.pos += 1; }
            if self.at(self.pos) == b'_' {
                return Err(LexError { msg: "separator in legacy octal", at: self.pos });
            }
        } else {
            self.digits(10)?;
        }
        let mut is_float = false;
        if self.at(self.pos) == b'.' {
            if self.legacy_octal { return Err(LexError { msg: "legacy octal with a fraction", at: self.pos }); }
            is_float = true;
            self.pos += 1;
            self.digits(10)?;
        }
        if self.at(self.pos) | 0x20 == b'e' {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.at(self.pos), b'+' | b'-') { self.pos += 1; }
            if self.at(self.pos).is_ascii_digit() {
                is_float = true;
                // `_` gilt auch hier: `1e1_0` ist gueltig (ES2021).
                self.digits(10)?;
            } else { self.pos = save; }
        }
        if !is_float && self.at(self.pos) == b'n' {
            // `01n` gibt es nicht — ein BigInt hat keine fuehrende Null.
            if self.legacy_octal || (lead_zero && self.pos > start + 1) {
                return Err(LexError { msg: "legacy octal bigint", at: self.pos });
            }
            self.pos += 1;
            return Ok(Tok::BigInt(self.text[start..self.pos - 1].replace('_', "")));
        }
        // Eine Ziffer direkt hinter einer Zahl ist ein Fehler (`3in`), sonst
        // liest der Parser `3` und `in` und baut daraus etwas Sinnvolles, das
        // im Quelltext nicht stand.
        let (c, _) = self.char_at(self.pos);
        if id_start(c) || c.is_ascii_digit() {
            return Err(LexError { msg: "identifier after number", at: self.pos });
        }
        let txt = self.text[start..self.pos].replace('_', "");
        let v = parse_f64(&txt);
        Ok(Tok::Num(v))
    }

    fn regex(&mut self) -> Result<Tok, LexError> {
        let start = self.pos;
        self.pos += 1;
        let mut in_class = false;
        loop {
            if self.pos >= self.src.len() {
                return Err(LexError { msg: "unterminated regex", at: self.pos });
            }
            let b = self.src[self.pos];
            match b {
                b'\\' => { self.pos += 2; continue; }
                b'[' => in_class = true,
                b']' => in_class = false,
                b'/' if !in_class => break,
                b'\n' | b'\r' => return Err(LexError { msg: "newline in regex", at: self.pos }),
                _ => {}
            }
            let (_, n) = self.char_at(self.pos);
            self.pos += n;
        }
        let body = self.text[start + 1..self.pos].into();
        self.pos += 1;
        let fstart = self.pos;
        loop {
            let (c, n) = self.char_at(self.pos);
            if self.pos >= self.src.len() || !id_part(c) { break; }
            self.pos += n;
        }
        Ok(Tok::Regex(body, self.text[fstart..self.pos].into()))
    }

    fn punct(&mut self) -> Result<Tok, LexError> {
        use P::*;
        let s = &self.src[self.pos..];
        let three = |a: u8, b: u8, c: u8| s.len() >= 3 && s[0] == a && s[1] == b && s[2] == c;
        let two = |a: u8, b: u8| s.len() >= 2 && s[0] == a && s[1] == b;
        // Vier Zeichen zuerst, dann drei, dann zwei — sonst wird `>>>=` als
        // `>>>` und `=` gelesen.
        let (p, n): (P, usize) = if s.len() >= 4 && &s[..4] == b">>>=" { (UShrEq, 4) }
            else if three(b'.', b'.', b'.') { (Ellipsis, 3) }
            else if three(b'=', b'=', b'=') { (EqEqEq, 3) }
            else if three(b'!', b'=', b'=') { (NotEqEq, 3) }
            else if three(b'*', b'*', b'=') { (StarStarEq, 3) }
            else if three(b'<', b'<', b'=') { (ShlEq, 3) }
            else if three(b'>', b'>', b'=') { (ShrEq, 3) }
            else if three(b'>', b'>', b'>') { (UShr, 3) }
            else if three(b'&', b'&', b'=') { (AmpAmpEq, 3) }
            else if three(b'|', b'|', b'=') { (PipePipeEq, 3) }
            else if three(b'?', b'?', b'=') { (QuestionQuestionEq, 3) }
            else if two(b'=', b'>') { (Arrow, 2) }
            else if two(b'+', b'+') { (Inc, 2) }
            else if two(b'-', b'-') { (Dec, 2) }
            else if two(b'*', b'*') { (StarStar, 2) }
            else if two(b'=', b'=') { (EqEq, 2) }
            else if two(b'!', b'=') { (NotEq, 2) }
            else if two(b'<', b'=') { (LtEq, 2) }
            else if two(b'>', b'=') { (GtEq, 2) }
            else if two(b'<', b'<') { (Shl, 2) }
            else if two(b'>', b'>') { (Shr, 2) }
            else if two(b'&', b'&') { (AmpAmp, 2) }
            else if two(b'|', b'|') { (PipePipe, 2) }
            else if two(b'?', b'?') { (QuestionQuestion, 2) }
            // `?.` NUR wenn keine Ziffer folgt: `a?.5:b` ist der Bedingungs-
            // operator mit `.5`, nicht optionales Verketten.
            else if two(b'?', b'.') && !self.at(self.pos + 2).is_ascii_digit() { (QuestionDot, 2) }
            else if two(b'+', b'=') { (PlusEq, 2) }
            else if two(b'-', b'=') { (MinusEq, 2) }
            else if two(b'*', b'=') { (StarEq, 2) }
            else if two(b'/', b'=') { (SlashEq, 2) }
            else if two(b'%', b'=') { (PercentEq, 2) }
            else if two(b'&', b'=') { (AmpEq, 2) }
            else if two(b'|', b'=') { (PipeEq, 2) }
            else if two(b'^', b'=') { (CaretEq, 2) }
            else {
                let one = match s[0] {
                    b'{' => LBrace, b'}' => RBrace, b'(' => LParen, b')' => RParen,
                    b'[' => LBracket, b']' => RBracket, b';' => Semi, b',' => Comma,
                    b'.' => Dot, b':' => Colon, b'?' => Question, b'+' => Plus,
                    b'-' => Minus, b'*' => Star, b'/' => Slash, b'%' => Percent,
                    b'<' => Lt, b'>' => Gt, b'&' => Amp, b'|' => Pipe, b'^' => Caret,
                    b'!' => Bang, b'~' => Tilde, b'=' => Eq,
                    // `# x` gibt es nicht: zwischen dem Zeichen und dem Namen
                    // darf nichts stehen (ES 12.6.1). Die Pruefung MUSS hier
                    // sitzen — eine Zeile spaeter hat `skip_trivia` den
                    // Leerraum schon geschluckt und der Unterschied ist weg.
                    b'#' => {
                        let nxt = self.at(self.pos + 1);
                        let ok = nxt == b'\\' || {
                            let (c, _) = self.char_at(self.pos + 1);
                            nxt != 0 && id_start(c)
                        };
                        if !ok { return Err(LexError { msg: "space between # and name", at: self.pos }); }
                        Hash
                    }
                    _ => return Err(LexError { msg: "unexpected character", at: self.pos }),
                };
                (one, 1)
            };
        self.pos += n;
        Ok(Tok::Punct(p))
    }
}

/// Dezimalzahl nach f64. `core` hat `str::parse::<f64>()`, und das ist die
/// korrekt gerundete Umwandlung — kein Grund, eine eigene zu schreiben.
fn parse_f64(s: &str) -> f64 {
    if let Ok(v) = s.parse::<f64>() { return v; }
    // Legacy-Oktal (`0755`) faellt hier herein: fuehrende Null + nur Ziffern
    // 0-7 wird als Oktal gelesen, alles andere als Dezimal (`089` = 89).
    if s.len() > 1 && s.starts_with('0') && s.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        let mut v = 0f64;
        for b in s.bytes() { v = v * 8.0 + (b - b'0') as f64; }
        return v;
    }
    let cleaned: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    cleaned.parse::<f64>().unwrap_or(f64::NAN)
}

/// Alle Token einer Quelle, ohne Parser-Rueckmeldung (nur fuer Tests: der
/// Parser holt selbst, weil nur er `regex_ok` kennt).
#[cfg(test)]
pub fn tokenize_all(src: &str) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        let t = lx.next(true)?;
        let end = t.tok == Tok::Eof;
        out.push(t);
        if end { return Ok(out); }
    }
}
