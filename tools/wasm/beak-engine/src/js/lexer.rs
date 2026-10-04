//! The tokenizer.
//!
//! Tokens are produced on demand, not up front: whether `/` is a division or
//! starts a regular expression (`a /b/ g` vs `return /b/g`) depends on whether
//! the parser expects an operand, so the parser says so on each fetch
//! (`next(regex_ok)`).
//!
//! Two more things live here rather than in the parser:
//!
//! - `newline_before` on every token, which automatic semicolon insertion
//!   needs and which is lost once whitespace is skipped.
//! - Template continuation: `` `a${x}b` `` is one literal with a hole; after
//!   the `}` the parser asks to continue reading the template
//!   (`next_template_part`).

use alloc::string::String;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Eof,
    Ident(String),
    /// A reserved word, kept apart from `Ident` so checks are an enum match
    /// rather than a string comparison.
    Keyword(Kw),
    Num(f64),
    BigInt(String),
    Str(String),
    /// Body text and flags. The body is not validated here; that is the
    /// RegExp engine's job.
    Regex(String, String),
    /// A template chunk: cooked text, raw text, and whether a substitution
    /// `${` follows (otherwise the literal ends here). Note: the opposite of
    /// ESTree's `tail`.
    Template { cooked: Option<String>, raw: String, has_sub: bool },
    Punct(P),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kw {
    Await, Break, Case, Catch, Class, Const, Continue, Debugger, Default, Delete,
    Do, Else, Enum, Export, Extends, False, Finally, For, Function, If, Import,
    In, Instanceof, New, Null, Return, Super, Switch, This, Throw, True, Try,
    Typeof, Var, Void, While, With, Yield,
    // Contextual: reserved only in some positions. They arrive as keywords
    // and the parser may turn them back into identifiers.
    Let, Static, Async, Get, Set, Of, As, From, Target, Meta,
}

impl Kw {
    /// Whether the word is reserved everywhere. Contextual words such as
    /// `let`/`static`/`async`/`of` are not: `var of = 1` is valid.
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
    /// `#name`: a private name in a class.
    Hash,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
    /// A line terminator preceded this token; drives semicolon insertion.
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
    /// The source as `str`, for slices containing multi-byte characters.
    text: &'a str,
    /// The last number was a legacy octal (`0755`) or a non-octal decimal
    /// with a leading zero (`089`). Both are early errors in strict mode,
    /// which only the parser knows.
    pub legacy_octal: bool,
}

/// Whether `c` may start an identifier.
///
/// Not the full Unicode ID_Start table: ASCII is exact, and above 0x80
/// everything is accepted. Deliberately too permissive: rejecting a valid
/// identifier loses the whole file, accepting too much only admits programs
/// nobody ships.
fn id_start(c: char) -> bool {
    if c.is_ascii() { return c.is_ascii_alphabetic() || c == '$' || c == '_'; }
    // Above 0x80 accept everything except whitespace and line terminators;
    // otherwise `1\u{2028}2` would read as a number followed by letters.
    !matches!(c, '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{00A0}' | '\u{1680}'
        | '\u{2000}'..='\u{200B}' | '\u{202F}' | '\u{205F}' | '\u{3000}')
}
fn id_part(c: char) -> bool {
    id_start(c) || c.is_ascii_digit() || c == '\u{200C}' || c == '\u{200D}'
}

impl<'a> Lexer<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut pos = 0;
        // Hashbang `#!...`: only on the very first line; elsewhere `#` starts
        // a private name.
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

    /// The source text. The directive check needs the raw slice:
    /// `"use\u0020strict"` is not a directive.
    pub fn src_text(&self) -> &'a str { self.text }

    /// The character at `i`, with its UTF-8 length.
    fn char_at(&self, i: usize) -> (char, usize) {
        // `self.text[i..]` panics off a char boundary; the lexer must not
        // panic on any input, so advance one byte instead.
        if i >= self.text.len() || !self.text.is_char_boundary(i) { return ('\0', 1); }
        match self.text[i..].chars().next() {
            Some(c) => (c, c.len_utf8()),
            None => ('\0', 1),
        }
    }

    /// Skip whitespace and comments; returns whether a line terminator was
    /// crossed.
    ///
    /// Includes the Annex B HTML-like comments `<!--` and `-->`, which old
    /// script blocks still contain.
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
                // `<!--` is a single-line comment (Annex B B.1.1).
                b'<' if self.at(self.pos + 1) == b'!' && self.at(self.pos + 2) == b'-'
                    && self.at(self.pos + 3) == b'-' => {
                    while self.pos < self.src.len()
                        && !matches!(self.src[self.pos], b'\n' | b'\r') { self.pos += 1; }
                }
                // So is `-->`, but only at the start of a line; otherwise
                // `a-->b` would stop being a decrement.
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
                        // A line break inside a block comment counts for
                        // semicolon insertion: `return /*\n*/ x` returns undefined.
                        if matches!(self.src[self.pos], b'\n' | b'\r') { nl = true; }
                        // U+2028/U+2029 are line terminators too.
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
                        // U+2028/U+2029 are line terminators; U+FEFF and the
                        // Unicode spaces are whitespace.
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
        // Identifiers may contain `\u{...}` escapes, so the name is assembled
        // first and only then looked up as a keyword.
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
            // A keyword spelled with an escape is neither a keyword nor a
            // valid identifier (early error). It is returned as an identifier;
            // the early-error check enforces the rule.
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
            // A lone surrogate is valid in JS but not a Rust `char`; map it to
            // U+FFFD instead of failing.
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

    /// An escape after `\`. `None` means a line continuation (contributes nothing).
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
            // Legacy octal escape (`\101`). An early error in strict mode,
            // checked by the parser.
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

    /// One template chunk from the current position (after `` ` `` or `}`).
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
                // A tagged template may contain invalid escapes; then `cooked`
                // is undefined and only `raw` applies (ES2018), so no error here.
                //
                // Legacy numeric escapes are invalid in templates (ES 12.9.6,
                // TemplateCharacter): `\1`-`\9` never, `\0` only when no digit
                // follows. `escape()` accepts them since sloppy strings allow them.
                let nx = self.at(self.pos);
                if matches!(nx, b'1'..=b'9')
                    || (nx == b'0' && (self.at(self.pos + 1) as char).is_ascii_digit()) {
                    bad = true;
                }
                // On error resume right after the escape letter, not from
                // wherever `escape()` stopped; otherwise `` `\u0` `` would
                // consume the closing backtick.
                let after_slash = self.pos;
                match self.escape() {
                    Ok(Some(c)) => cooked.push(c),
                    Ok(None) => {}
                    Err(_) => { bad = true; self.pos = after_slash + 1; }
                }
                continue;
            }
            let (c, n) = self.char_at(self.pos);
            cooked.push(c);
            self.pos += n;
        }
    }

    /// Read digits in base `radix`. A `_` separator must sit between two
    /// digits: `1_0` yes, `1_`/`_1`/`1__0` no. Returns the digit count.
    fn digits(&mut self, radix: u32) -> Result<usize, LexError> {
        let mut n = 0;
        let mut prev_sep = false;
        let mut any = false;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == b'_' {
                // No separator at the start, doubled, or at the end.
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
        // Decimal, including legacy octal (`0755`) and the non-octal form
        // (`089`): strict-mode early errors, not lex errors. Neither may
        // contain a separator.
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
                // Separators are allowed in the exponent too: `1e1_0` (ES2021).
                self.digits(10)?;
            } else { self.pos = save; }
        }
        if !is_float && self.at(self.pos) == b'n' {
            // A BigInt literal has no leading zero: `01n` is invalid.
            if self.legacy_octal || (lead_zero && self.pos > start + 1) {
                return Err(LexError { msg: "legacy octal bigint", at: self.pos });
            }
            self.pos += 1;
            return Ok(Tok::BigInt(self.text[start..self.pos - 1].replace('_', "")));
        }
        // An identifier start or digit directly after a number is an error
        // (`3in`); otherwise the parser would read `3` and `in`.
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
                // A backslash escapes one character, not one byte.
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.src.len() {
                        let (_, n) = self.char_at(self.pos);
                        self.pos += n;
                    }
                    continue;
                }
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
        // Longest match first: four, three, then two characters, else `>>>=`
        // would read as `>>>` and `=`.
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
            // `?.` only when no digit follows: `a?.5:b` is a conditional with
            // `.5`, not optional chaining.
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
                    // No whitespace between `#` and the name (ES 12.6.1). Must
                    // be checked here; the next fetch has already skipped it.
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

/// Decimal literal to f64 via `str::parse::<f64>()`, which rounds correctly.
fn parse_f64(s: &str) -> f64 {
    if let Ok(v) = s.parse::<f64>() { return v; }
    // Legacy octal (`0755`) ends up here: a leading zero with only digits 0-7
    // reads as octal, anything else as decimal (`089` = 89).
    if s.len() > 1 && s.starts_with('0') && s.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        let mut v = 0f64;
        for b in s.bytes() { v = v * 8.0 + (b - b'0') as f64; }
        return v;
    }
    let cleaned: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    cleaned.parse::<f64>().unwrap_or(f64::NAN)
}

/// All tokens of a source without parser feedback. Tests only: the parser
/// fetches itself, since only it knows `regex_ok`.
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
