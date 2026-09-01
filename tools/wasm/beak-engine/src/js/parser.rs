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

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;

use super::ast::*;
use super::lexer::{Kw, Lexer, P, Tok, Token};

#[derive(Debug)]
pub struct ParseError {
    pub msg: String,
    pub at: usize,
}

/// Erwartet die Grammatik hier einen Operanden? Dann ist `/` der Anfang eines
/// regulaeren Ausdrucks, sonst eine Division.
fn regex_allowed_after(t: &Tok) -> bool {
    match t {
        Tok::Ident(_) | Tok::Num(_) | Tok::BigInt(_) | Tok::Str(_) | Tok::Regex(..) => false,
        // Nach einem Template-Stueck MIT folgender Einsetzung kommt ein
        // Ausdruck, also darf dort ein Regex stehen: `${/^a/.test(x)}`.
        // Nach dem letzten Stueck ist `/` eine Division.
        Tok::Template { has_sub, .. } => *has_sub,
        Tok::Keyword(k) => !matches!(k, Kw::This | Kw::Super | Kw::True | Kw::False | Kw::Null),
        Tok::Punct(p) => !matches!(p, P::RParen | P::RBracket | P::Inc | P::Dec),
        Tok::Eof => true,
    }
}

pub struct Parser<'a> {
    lx: Lexer<'a>,
    cur: Token,
    /// Stand des Lexers VOR `cur` — fuer die Faelle, in denen dasselbe Zeichen
    /// neu gelesen werden muss (Template-Fortsetzung nach `}`).
    cur_start: usize,
    strict: bool,
    module: bool,
    in_func: bool,
    in_gen: bool,
    in_async: bool,
    in_loop: u32,
    in_switch: u32,
    /// War der zuletzt gelesene Operand geklammert? `(a && b) ?? c` ist
    /// erlaubt, `a && b ?? c` nicht — und im Baum sieht beides gleich aus.
    just_paren: bool,
}

type R<T> = Result<T, ParseError>;

impl<'a> Parser<'a> {
    pub fn new(src: &'a str, module: bool) -> R<Self> {
        let mut lx = Lexer::new(src);
        let cur = lx.next(true).map_err(|e| ParseError { msg: e.msg.to_string(), at: e.at })?;
        Ok(Parser {
            lx, cur_start: 0, cur, strict: module, module,
            in_func: false, in_gen: false, in_async: module, in_loop: 0, in_switch: 0,
            just_paren: false,
        })
    }

    fn err<T>(&self, msg: &str) -> R<T> {
        Err(ParseError { msg: msg.to_string(), at: self.cur.start })
    }

    fn bump(&mut self) -> R<()> {
        let ok = regex_allowed_after(&self.cur.tok);
        self.cur_start = self.lx.pos;
        self.cur = self.lx.next(ok).map_err(|e| ParseError { msg: e.msg.to_string(), at: e.at })?;
        Ok(())
    }

    fn is_p(&self, p: P) -> bool { self.cur.tok == Tok::Punct(p) }
    fn is_kw(&self, k: Kw) -> bool { self.cur.tok == Tok::Keyword(k) }
    fn eat_p(&mut self, p: P) -> R<bool> {
        if self.is_p(p) { self.bump()?; Ok(true) } else { Ok(false) }
    }
    fn eat_kw(&mut self, k: Kw) -> R<bool> {
        if self.is_kw(k) { self.bump()?; Ok(true) } else { Ok(false) }
    }
    fn expect_p(&mut self, p: P) -> R<()> {
        if self.eat_p(p)? { Ok(()) } else { self.err("unexpected token") }
    }

    /// Der Name eines Bezeichners an dieser Stelle, mit den kontextabhaengigen
    /// Schluesselwoertern als gueltigen Namen. `yield` und `await` haengen am
    /// Kontext: in einem Generator bzw. einer async-Funktion sind sie reserviert.
    fn ident_name(&mut self) -> R<String> {
        let name = match &self.cur.tok {
            Tok::Ident(s) => s.clone(),
            Tok::Keyword(k) if !k.is_reserved() => {
                if self.strict && matches!(k, Kw::Let | Kw::Static) {
                    return self.err("reserved word in strict mode");
                }
                k.as_str().to_string()
            }
            Tok::Keyword(Kw::Yield) if !self.in_gen && !self.strict => "yield".to_string(),
            Tok::Keyword(Kw::Await) if !self.in_async && !self.module => "await".to_string(),
            _ => return self.err("expected identifier"),
        };
        self.bump()?;
        Ok(name)
    }

    /// Ein Modulname: Bezeichner ODER Zeichenkette. `export { "a b" as c }`
    /// ist ES2022 und der einzige Ort, an dem ein Name Leerzeichen tragen darf
    /// — WebAssembly-Module exportieren solche Namen.
    fn module_export_name(&mut self) -> R<String> {
        if let Tok::Str(s) = self.cur.tok.clone() { self.bump()?; return Ok(s); }
        self.property_name()
    }

    /// Ein Eigenschaftsname nach `.` — dort sind ALLE reservierten Woerter
    /// erlaubt (`a.class`, `a.if`). Seit ES5, und echter Code nutzt es.
    fn property_name(&mut self) -> R<String> {
        let name = match &self.cur.tok {
            Tok::Ident(s) => s.clone(),
            Tok::Keyword(k) => k.as_str().to_string(),
            _ => return self.err("expected property name"),
        };
        self.bump()?;
        Ok(name)
    }

    /// Semikolon — oder die automatische Einfuegung. Sie greift vor `}`, am
    /// Ende der Datei und wenn vor dem naechsten Token eine Zeile begann.
    fn semicolon(&mut self) -> R<()> {
        if self.eat_p(P::Semi)? { return Ok(()); }
        if self.is_p(P::RBrace) || self.cur.tok == Tok::Eof || self.cur.newline_before {
            return Ok(());
        }
        self.err("expected semicolon")
    }

    // ── Programm ─────────────────────────────────────────────────────────
    pub fn parse_program(&mut self) -> R<Program> {
        let body = self.directive_prologue_and_body(true)?;
        if self.cur.tok != Tok::Eof { return self.err("unexpected token after program"); }
        Ok(Program { body, module: self.module })
    }

    /// Der Direktiven-Vorspann: fuehrende Zeichenkettenausdruecke, unter denen
    /// `"use strict"` den Rest der Einheit umschaltet. Er muss VOR dem
    /// Weiterlesen ausgewertet werden — der strenge Modus aendert, was
    /// ueberhaupt noch parst.
    fn directive_prologue_and_body(&mut self, top: bool) -> R<Vec<Stmt>> {
        let mut out = Vec::new();
        loop {
            if self.cur.tok == Tok::Eof || self.is_p(P::RBrace) { break; }
            let is_str = matches!(self.cur.tok, Tok::Str(_));
            let raw_start = self.cur.start;
            let raw_end = self.cur.end;
            let st = self.statement()?;
            if is_str {
                if let Stmt::Expr(Expr::Str(_)) = &st {
                    // Auf den ROHTEXT geprueft, nicht auf den entschluesselten:
                    // `"use strict"` ist KEINE Direktive (ES §11.2.1).
                    let raw = &self.lx_src()[raw_start..raw_end];
                    if raw == "\"use strict\"" || raw == "'use strict'" { self.strict = true; }
                    out.push(st);
                    continue;
                }
            }
            out.push(st);
            break;
        }
        while self.cur.tok != Tok::Eof && !self.is_p(P::RBrace) {
            let st = self.statement()?;
            out.push(st);
        }
        let _ = top;
        Ok(out)
    }

    fn lx_src(&self) -> &'a str { self.lx.src_text() }

    // ── Anweisungen ──────────────────────────────────────────────────────
    fn statement(&mut self) -> R<Stmt> {
        match &self.cur.tok {
            Tok::Punct(P::LBrace) => {
                self.bump()?;
                let mut body = Vec::new();
                while !self.is_p(P::RBrace) {
                    if self.cur.tok == Tok::Eof { return self.err("unterminated block"); }
                    body.push(self.statement()?);
                }
                self.bump()?;
                Ok(Stmt::Block(body))
            }
            Tok::Punct(P::Semi) => { self.bump()?; Ok(Stmt::Empty) }
            Tok::Keyword(k) => {
                let k = *k;
                match k {
                    Kw::Var | Kw::Const => self.var_statement(),
                    // `let` ist nur dann eine Deklaration, wenn ein Bezeichner,
                    // `[` oder `{` folgt — sonst ist es ein Bezeichner
                    // (`let = 1`, `let.a`, `let(x)`).
                    Kw::Let if self.let_is_decl()? => self.var_statement(),
                    Kw::Function => { let f = self.function(false, true)?; Ok(Stmt::Func(Box::new(f))) }
                    Kw::Async if self.async_function_ahead()? => {
                        self.bump()?;
                        let f = self.function(true, true)?;
                        Ok(Stmt::Func(Box::new(f)))
                    }
                    Kw::Class => { let c = self.class(true)?; Ok(Stmt::Class(Box::new(c))) }
                    Kw::If => self.if_statement(),
                    Kw::For => self.for_statement(),
                    Kw::While => {
                        self.bump()?; self.expect_p(P::LParen)?;
                        let test = self.expression()?;
                        self.expect_p(P::RParen)?;
                        self.in_loop += 1;
                        let body = self.statement()?;
                        self.in_loop -= 1;
                        Ok(Stmt::While { test, body: Box::new(body) })
                    }
                    Kw::Do => {
                        self.bump()?;
                        self.in_loop += 1;
                        let body = self.statement()?;
                        self.in_loop -= 1;
                        if !self.eat_kw(Kw::While)? { return self.err("expected while"); }
                        self.expect_p(P::LParen)?;
                        let test = self.expression()?;
                        self.expect_p(P::RParen)?;
                        // Nach `do {} while ()` darf das Semikolon immer fehlen.
                        let _ = self.eat_p(P::Semi)?;
                        Ok(Stmt::DoWhile { body: Box::new(body), test })
                    }
                    Kw::Return => {
                        if !self.in_func { return self.err("return outside function"); }
                        self.bump()?;
                        let arg = if self.is_p(P::Semi) || self.is_p(P::RBrace)
                            || self.cur.tok == Tok::Eof || self.cur.newline_before {
                            None
                        } else { Some(self.expression()?) };
                        self.semicolon()?;
                        Ok(Stmt::Return(arg))
                    }
                    Kw::Break | Kw::Continue => {
                        self.bump()?;
                        let label = if !self.cur.newline_before && !self.is_p(P::Semi)
                            && !self.is_p(P::RBrace) && self.cur.tok != Tok::Eof
                            && matches!(self.cur.tok, Tok::Ident(_) | Tok::Keyword(_)) {
                            Some(self.ident_name()?)
                        } else { None };
                        self.semicolon()?;
                        if label.is_none() {
                            if k == Kw::Continue && self.in_loop == 0 {
                                return self.err("continue outside loop");
                            }
                            if k == Kw::Break && self.in_loop == 0 && self.in_switch == 0 {
                                return self.err("break outside loop or switch");
                            }
                        }
                        Ok(if k == Kw::Break { Stmt::Break(label) } else { Stmt::Continue(label) })
                    }
                    Kw::Throw => {
                        self.bump()?;
                        if self.cur.newline_before { return self.err("newline after throw"); }
                        let e = self.expression()?;
                        self.semicolon()?;
                        Ok(Stmt::Throw(e))
                    }
                    Kw::Try => self.try_statement(),
                    Kw::Switch => self.switch_statement(),
                    Kw::Debugger => { self.bump()?; self.semicolon()?; Ok(Stmt::Debugger) }
                    Kw::With => {
                        if self.strict { return self.err("with in strict mode"); }
                        self.bump()?; self.expect_p(P::LParen)?;
                        let obj = self.expression()?;
                        self.expect_p(P::RParen)?;
                        let body = self.statement()?;
                        Ok(Stmt::With { obj, body: Box::new(body) })
                    }
                    Kw::Import if self.module && !self.import_is_expr()? => self.import_statement(),
                    Kw::Export if self.module => self.export_statement(),
                    _ => self.expression_statement(),
                }
            }
            _ => self.expression_statement(),
        }
    }

    /// Folgt auf `let` etwas, das es zur Deklaration macht?
    fn let_is_decl(&mut self) -> R<bool> {
        let save = (self.lx.pos, self.cur.clone(), self.cur_start);
        self.bump()?;
        let yes = matches!(self.cur.tok, Tok::Ident(_))
            || self.is_p(P::LBracket) || self.is_p(P::LBrace)
            || matches!(&self.cur.tok, Tok::Keyword(k) if !k.is_reserved()
                || matches!(k, Kw::Yield | Kw::Await));
        self.restore(save);
        Ok(yes)
    }

    /// `async function` — aber nur ohne Zeilenumbruch dazwischen.
    fn async_function_ahead(&mut self) -> R<bool> {
        let save = (self.lx.pos, self.cur.clone(), self.cur_start);
        self.bump()?;
        let yes = self.is_kw(Kw::Function) && !self.cur.newline_before;
        self.restore(save);
        Ok(yes)
    }

    fn import_is_expr(&mut self) -> R<bool> {
        let save = (self.lx.pos, self.cur.clone(), self.cur_start);
        self.bump()?;
        let yes = self.is_p(P::LParen) || self.is_p(P::Dot);
        self.restore(save);
        Ok(yes)
    }

    /// Zuruecksetzen. Nur fuer die drei Stellen oben, an denen ein Token
    /// Vorausschau nicht reicht — nicht als allgemeines Ruecksetzen: davon
    /// leben Parser, die man nicht mehr versteht.
    fn restore(&mut self, save: (usize, Token, usize)) {
        self.lx.pos = save.0;
        self.cur = save.1;
        self.cur_start = save.2;
    }

    fn var_statement(&mut self) -> R<Stmt> {
        let d = self.var_decl()?;
        self.semicolon()?;
        Ok(Stmt::VarDecl(d))
    }

    fn var_decl(&mut self) -> R<VarDecl> {
        let kind = match &self.cur.tok {
            Tok::Keyword(Kw::Var) => VarKind::Var,
            Tok::Keyword(Kw::Let) => VarKind::Let,
            Tok::Keyword(Kw::Const) => VarKind::Const,
            _ => return self.err("expected var/let/const"),
        };
        self.bump()?;
        let mut decls = Vec::new();
        loop {
            let id = self.binding_pattern()?;
            let init = if self.eat_p(P::Eq)? { Some(self.assign_expr()?) } else { None };
            // `const x;` hat keinen Wert, den es festhalten koennte.
            if init.is_none() && kind == VarKind::Const && !matches!(id, Pat::Ident(_)) {
                return self.err("destructuring declaration without initializer");
            }
            if init.is_none() && !matches!(id, Pat::Ident(_)) {
                return self.err("destructuring declaration without initializer");
            }
            decls.push(Declarator { id, init });
            if !self.eat_p(P::Comma)? { break; }
        }
        Ok(VarDecl { kind, decls })
    }

    fn binding_pattern(&mut self) -> R<Pat> {
        if self.is_p(P::LBracket) || self.is_p(P::LBrace) {
            let e = self.primary()?;
            return self.expr_to_pattern(e, true);
        }
        Ok(Pat::Ident(self.ident_name()?))
    }

    fn if_statement(&mut self) -> R<Stmt> {
        self.bump()?; self.expect_p(P::LParen)?;
        let test = self.expression()?;
        self.expect_p(P::RParen)?;
        let cons = self.statement()?;
        let alt = if self.eat_kw(Kw::Else)? { Some(Box::new(self.statement()?)) } else { None };
        Ok(Stmt::If { test, cons: Box::new(cons), alt })
    }

    fn for_statement(&mut self) -> R<Stmt> {
        self.bump()?;
        let is_await = self.eat_kw(Kw::Await)?;
        self.expect_p(P::LParen)?;

        // Leerer Kopf: `for (;;)`
        if self.is_p(P::Semi) {
            self.bump()?;
            return self.for_classic(None, is_await);
        }

        let is_decl = self.is_kw(Kw::Var) || self.is_kw(Kw::Const)
            || (self.is_kw(Kw::Let) && self.let_is_decl()?);

        if is_decl {
            let kind = match &self.cur.tok {
                Tok::Keyword(Kw::Var) => VarKind::Var,
                Tok::Keyword(Kw::Let) => VarKind::Let,
                _ => VarKind::Const,
            };
            self.bump()?;
            let id = self.binding_pattern()?;
            if self.is_kw(Kw::In) || self.is_kw(Kw::Of) {
                let of = self.is_kw(Kw::Of);
                self.bump()?;
                let right = if of { self.assign_expr()? } else { self.expression()? };
                self.expect_p(P::RParen)?;
                self.in_loop += 1;
                let body = self.statement()?;
                self.in_loop -= 1;
                let head = ForHead::VarDecl(VarDecl { kind, decls: vec![Declarator { id, init: None }] });
                return Ok(if of {
                    Stmt::ForOf { left: Box::new(head), right, body: Box::new(body), is_await }
                } else {
                    Stmt::ForIn { left: Box::new(head), right, body: Box::new(body) }
                });
            }
            // Gewoehnliche Deklaration im Kopf — der Rest der Liste folgt.
            let init = if self.eat_p(P::Eq)? { Some(self.assign_expr()?) } else { None };
            let mut decls = vec![Declarator { id, init }];
            while self.eat_p(P::Comma)? {
                let id = self.binding_pattern()?;
                let init = if self.eat_p(P::Eq)? { Some(self.assign_expr()?) } else { None };
                decls.push(Declarator { id, init });
            }
            self.expect_p(P::Semi)?;
            return self.for_classic(Some(ForInit::VarDecl(VarDecl { kind, decls })), is_await);
        }

        // Ausdruckskopf. `in` muss hier ausgeschlossen bleiben, sonst frisst
        // der Vergleichsoperator das `in` von `for (x in y)`.
        let e = self.expression_no_in()?;
        if self.is_kw(Kw::In) || self.is_kw(Kw::Of) {
            let of = self.is_kw(Kw::Of);
            self.bump()?;
            let right = if of { self.assign_expr()? } else { self.expression()? };
            self.expect_p(P::RParen)?;
            self.in_loop += 1;
            let body = self.statement()?;
            self.in_loop -= 1;
            let head = ForHead::Pattern(self.expr_to_pattern(e, false)?);
            return Ok(if of {
                Stmt::ForOf { left: Box::new(head), right, body: Box::new(body), is_await }
            } else {
                Stmt::ForIn { left: Box::new(head), right, body: Box::new(body) }
            });
        }
        self.expect_p(P::Semi)?;
        self.for_classic(Some(ForInit::Expr(e)), is_await)
    }

    fn for_classic(&mut self, init: Option<ForInit>, is_await: bool) -> R<Stmt> {
        if is_await { return self.err("for await requires of"); }
        let test = if self.is_p(P::Semi) { None } else { Some(self.expression()?) };
        self.expect_p(P::Semi)?;
        let update = if self.is_p(P::RParen) { None } else { Some(self.expression()?) };
        self.expect_p(P::RParen)?;
        self.in_loop += 1;
        let body = self.statement()?;
        self.in_loop -= 1;
        Ok(Stmt::For { init: init.map(Box::new), test, update, body: Box::new(body) })
    }

    fn try_statement(&mut self) -> R<Stmt> {
        self.bump()?;
        let block = self.block_body()?;
        let mut handler = None;
        if self.eat_kw(Kw::Catch)? {
            // `catch {}` ohne Bindung ist ES2019.
            let param = if self.eat_p(P::LParen)? {
                let p = self.binding_pattern()?;
                self.expect_p(P::RParen)?;
                Some(p)
            } else { None };
            handler = Some(CatchClause { param, body: self.block_body()? });
        }
        let finalizer = if self.eat_kw(Kw::Finally)? { Some(self.block_body()?) } else { None };
        if handler.is_none() && finalizer.is_none() {
            return self.err("try without catch or finally");
        }
        Ok(Stmt::Try { block, handler, finalizer })
    }

    fn block_body(&mut self) -> R<Vec<Stmt>> {
        self.expect_p(P::LBrace)?;
        let mut out = Vec::new();
        while !self.is_p(P::RBrace) {
            if self.cur.tok == Tok::Eof { return self.err("unterminated block"); }
            out.push(self.statement()?);
        }
        self.bump()?;
        Ok(out)
    }

    fn switch_statement(&mut self) -> R<Stmt> {
        self.bump()?; self.expect_p(P::LParen)?;
        let disc = self.expression()?;
        self.expect_p(P::RParen)?;
        self.expect_p(P::LBrace)?;
        self.in_switch += 1;
        let mut cases = Vec::new();
        let mut seen_default = false;
        while !self.is_p(P::RBrace) {
            if self.cur.tok == Tok::Eof { self.in_switch -= 1; return self.err("unterminated switch"); }
            let test = if self.eat_kw(Kw::Case)? {
                Some(self.expression()?)
            } else if self.eat_kw(Kw::Default)? {
                if seen_default { self.in_switch -= 1; return self.err("duplicate default"); }
                seen_default = true;
                None
            } else { self.in_switch -= 1; return self.err("expected case or default"); };
            self.expect_p(P::Colon)?;
            let mut body = Vec::new();
            while !self.is_p(P::RBrace) && !self.is_kw(Kw::Case) && !self.is_kw(Kw::Default) {
                if self.cur.tok == Tok::Eof { self.in_switch -= 1; return self.err("unterminated switch"); }
                body.push(self.statement()?);
            }
            cases.push(SwitchCase { test, body });
        }
        self.in_switch -= 1;
        self.bump()?;
        Ok(Stmt::Switch { disc, cases })
    }

    fn expression_statement(&mut self) -> R<Stmt> {
        // Ein Label ist ein Bezeichner mit `:` dahinter, und das sieht man
        // erst nach dem Bezeichner.
        if matches!(self.cur.tok, Tok::Ident(_)) {
            let save = (self.lx.pos, self.cur.clone(), self.cur_start);
            let name = self.ident_name()?;
            if self.eat_p(P::Colon)? {
                let body = self.statement()?;
                return Ok(Stmt::Labeled { label: name, body: Box::new(body) });
            }
            self.restore(save);
        }
        let e = self.expression()?;
        self.semicolon()?;
        Ok(Stmt::Expr(e))
    }

    // ── Module ───────────────────────────────────────────────────────────
    fn import_statement(&mut self) -> R<Stmt> {
        self.bump()?;
        let mut specifiers = Vec::new();
        if let Tok::Str(s) = self.cur.tok.clone() {
            self.bump()?; self.semicolon()?;
            return Ok(Stmt::Import(Import { specifiers, source: s }));
        }
        if matches!(self.cur.tok, Tok::Ident(_)) || matches!(&self.cur.tok, Tok::Keyword(k) if !k.is_reserved()) {
            specifiers.push(ImportSpec::Default(self.ident_name()?));
            let _ = self.eat_p(P::Comma)?;
        }
        if self.eat_p(P::Star)? {
            if !self.eat_kw(Kw::As)? { return self.err("expected as"); }
            specifiers.push(ImportSpec::Namespace(self.ident_name()?));
        } else if self.eat_p(P::LBrace)? {
            while !self.is_p(P::RBrace) {
                let imported = self.module_export_name()?;
                let local = if self.eat_kw(Kw::As)? { self.ident_name()? } else { imported.clone() };
                specifiers.push(ImportSpec::Named { imported, local });
                if !self.eat_p(P::Comma)? { break; }
            }
            self.expect_p(P::RBrace)?;
        }
        if !self.eat_kw(Kw::From)? { return self.err("expected from"); }
        let source = match self.cur.tok.clone() {
            Tok::Str(s) => { self.bump()?; s }
            _ => return self.err("expected module source"),
        };
        self.semicolon()?;
        Ok(Stmt::Import(Import { specifiers, source }))
    }

    fn export_statement(&mut self) -> R<Stmt> {
        self.bump()?;
        if self.eat_kw(Kw::Default)? {
            let d = if self.is_kw(Kw::Function) {
                ExportDefault::Func(Box::new(self.function(false, false)?))
            } else if self.is_kw(Kw::Async) && self.async_function_ahead()? {
                self.bump()?;
                ExportDefault::Func(Box::new(self.function(true, false)?))
            } else if self.is_kw(Kw::Class) {
                ExportDefault::Class(Box::new(self.class(false)?))
            } else {
                let e = self.assign_expr()?; self.semicolon()?;
                ExportDefault::Expr(e)
            };
            return Ok(Stmt::ExportDefault(Box::new(d)));
        }
        if self.eat_p(P::Star)? {
            let alias = if self.eat_kw(Kw::As)? { Some(self.module_export_name()?) } else { None };
            if !self.eat_kw(Kw::From)? { return self.err("expected from"); }
            let source = match self.cur.tok.clone() {
                Tok::Str(s) => { self.bump()?; s }
                _ => return self.err("expected module source"),
            };
            self.semicolon()?;
            return Ok(Stmt::ExportAll { source, alias });
        }
        if self.eat_p(P::LBrace)? {
            let mut specifiers = Vec::new();
            while !self.is_p(P::RBrace) {
                let local = self.module_export_name()?;
                let exported = if self.eat_kw(Kw::As)? { self.module_export_name()? } else { local.clone() };
                specifiers.push(ExportSpec { local, exported });
                if !self.eat_p(P::Comma)? { break; }
            }
            self.expect_p(P::RBrace)?;
            let source = if self.eat_kw(Kw::From)? {
                match self.cur.tok.clone() {
                    Tok::Str(s) => { self.bump()?; Some(s) }
                    _ => return self.err("expected module source"),
                }
            } else { None };
            self.semicolon()?;
            return Ok(Stmt::ExportNamed { decl: None, specifiers, source });
        }
        let decl = self.statement()?;
        Ok(Stmt::ExportNamed { decl: Some(Box::new(decl)), specifiers: Vec::new(), source: None })
    }

    // ── Funktionen und Klassen ───────────────────────────────────────────
    fn function(&mut self, is_async: bool, need_name: bool) -> R<Func> {
        self.bump()?; // `function`
        let is_generator = self.eat_p(P::Star)?;
        let name = if self.is_p(P::LParen) {
            if need_name { return self.err("function statement requires a name"); }
            None
        } else { Some(self.ident_name()?) };

        let (og, oa, of) = (self.in_gen, self.in_async, self.in_func);
        self.in_gen = is_generator; self.in_async = is_async; self.in_func = true;
        let params = self.params()?;
        let outer_strict = self.strict;
        let body = self.block_body_with_prologue()?;
        self.strict = outer_strict;
        self.in_gen = og; self.in_async = oa; self.in_func = of;

        Ok(Func { name, params, body, is_async, is_generator, is_arrow: false, expr_body: false })
    }

    fn block_body_with_prologue(&mut self) -> R<Vec<Stmt>> {
        self.expect_p(P::LBrace)?;
        let out = self.directive_prologue_and_body(false)?;
        self.expect_p(P::RBrace)?;
        Ok(out)
    }

    fn params(&mut self) -> R<Vec<Pat>> {
        self.expect_p(P::LParen)?;
        let mut out = Vec::new();
        while !self.is_p(P::RParen) {
            if self.eat_p(P::Ellipsis)? {
                out.push(Pat::Rest(Box::new(self.binding_pattern()?)));
                break;
            }
            let p = self.binding_pattern()?;
            let p = if self.eat_p(P::Eq)? {
                Pat::Assign { left: Box::new(p), right: Box::new(self.assign_expr()?) }
            } else { p };
            out.push(p);
            if !self.eat_p(P::Comma)? { break; }
        }
        self.expect_p(P::RParen)?;
        Ok(out)
    }

    fn class(&mut self, need_name: bool) -> R<Class> {
        self.bump()?; // `class`
        // Ein Klassenkoerper ist IMMER streng, auch in einer lockeren Datei.
        let outer_strict = self.strict;
        self.strict = true;
        let name = if self.is_p(P::LBrace) || self.is_kw(Kw::Extends) {
            if need_name { None } else { None }
        } else { Some(self.ident_name()?) };
        let super_class = if self.eat_kw(Kw::Extends)? { Some(self.lhs_expr()?) } else { None };
        self.expect_p(P::LBrace)?;
        let mut body = Vec::new();
        while !self.is_p(P::RBrace) {
            if self.cur.tok == Tok::Eof { self.strict = outer_strict; return self.err("unterminated class"); }
            if self.eat_p(P::Semi)? { continue; }
            body.push(self.class_member()?);
        }
        self.bump()?;
        self.strict = outer_strict;
        Ok(Class { name, super_class, body })
    }

    fn class_member(&mut self) -> R<ClassMember> {
        let mut is_static = false;
        if self.is_kw(Kw::Static) {
            let save = (self.lx.pos, self.cur.clone(), self.cur_start);
            self.bump()?;
            // `static` allein als Feldname (`static = 1`, `static;`) ist erlaubt.
            if self.is_p(P::Eq) || self.is_p(P::Semi) || self.is_p(P::RBrace) || self.is_p(P::LParen) {
                self.restore(save);
            } else if self.is_p(P::LBrace) {
                // Statischer Initialisierungsblock (ES2022).
                let body = self.block_body()?;
                return Ok(ClassMember::StaticBlock(body));
            } else { is_static = true; }
        }
        self.method_or_field(is_static)
    }

    fn method_or_field(&mut self, is_static: bool) -> R<ClassMember> {
        let mut is_async = false;
        let mut is_generator = false;
        let mut kind = MethodKind::Method;

        if self.is_kw(Kw::Async) {
            let save = (self.lx.pos, self.cur.clone(), self.cur_start);
            self.bump()?;
            if self.is_p(P::LParen) || self.is_p(P::Eq) || self.is_p(P::Semi)
                || self.is_p(P::RBrace) || self.cur.newline_before {
                self.restore(save);
            } else { is_async = true; }
        }
        if self.eat_p(P::Star)? { is_generator = true; }
        if (self.is_kw(Kw::Get) || self.is_kw(Kw::Set)) && !is_generator {
            let want = if self.is_kw(Kw::Get) { MethodKind::Get } else { MethodKind::Set };
            let save = (self.lx.pos, self.cur.clone(), self.cur_start);
            self.bump()?;
            if self.is_p(P::LParen) || self.is_p(P::Eq) || self.is_p(P::Semi) || self.is_p(P::RBrace) {
                self.restore(save);
            } else { kind = want; }
        }

        let (key, computed) = self.property_key()?;
        if self.is_p(P::LParen) {
            if let PropKey::Ident(n) = &key {
                if n == "constructor" && !is_static && kind == MethodKind::Method { kind = MethodKind::Constructor; }
            }
            let (og, oa, of) = (self.in_gen, self.in_async, self.in_func);
            self.in_gen = is_generator; self.in_async = is_async; self.in_func = true;
            let params = self.params()?;
            let body = self.block_body_with_prologue()?;
            self.in_gen = og; self.in_async = oa; self.in_func = of;
            let func = Func { name: None, params, body, is_async, is_generator, is_arrow: false, expr_body: false };
            return Ok(ClassMember::Method { key, func: Box::new(func), kind, is_static, computed });
        }
        // Feld.
        let value = if self.eat_p(P::Eq)? { Some(self.assign_expr()?) } else { None };
        self.semicolon()?;
        Ok(ClassMember::Field { key, value, is_static, computed })
    }

    fn property_key(&mut self) -> R<(PropKey, bool)> {
        match self.cur.tok.clone() {
            Tok::Punct(P::LBracket) => {
                self.bump()?;
                let e = self.assign_expr()?;
                self.expect_p(P::RBracket)?;
                Ok((PropKey::Computed(Box::new(e)), true))
            }
            Tok::Punct(P::Hash) => {
                self.bump()?;
                Ok((PropKey::Private(self.property_name()?), false))
            }
            Tok::Str(s) => { self.bump()?; Ok((PropKey::Str(s), false)) }
            Tok::Num(n) => { self.bump()?; Ok((PropKey::Num(n), false)) }
            Tok::BigInt(s) => { self.bump()?; Ok((PropKey::Str(s), false)) }
            _ => Ok((PropKey::Ident(self.property_name()?), false)),
        }
    }

    // ── Ausdruecke ───────────────────────────────────────────────────────
    fn expression(&mut self) -> R<Expr> {
        let first = self.assign_expr()?;
        if !self.is_p(P::Comma) { return Ok(first); }
        let mut list = vec![first];
        while self.eat_p(P::Comma)? { list.push(self.assign_expr()?); }
        Ok(Expr::Seq(list))
    }

    /// Wie `expression`, aber `in` gilt nicht als Operator — nur fuer den
    /// `for`-Kopf.
    fn expression_no_in(&mut self) -> R<Expr> {
        let first = self.assign_expr_impl(true)?;
        if !self.is_p(P::Comma) { return Ok(first); }
        let mut list = vec![first];
        while self.eat_p(P::Comma)? { list.push(self.assign_expr_impl(true)?); }
        Ok(Expr::Seq(list))
    }

    fn assign_expr(&mut self) -> R<Expr> { self.assign_expr_impl(false) }

    fn assign_expr_impl(&mut self, no_in: bool) -> R<Expr> {
        if self.in_gen && self.is_kw(Kw::Yield) { return self.yield_expr(); }

        // Pfeil mit einem einzelnen Bezeichner: `x => …`, `async x => …`.
        if let Some(f) = self.try_simple_arrow()? { return Ok(f); }

        let left = self.conditional(no_in)?;
        let op = match &self.cur.tok {
            Tok::Punct(P::Eq) => AssignOp::Assign,
            Tok::Punct(P::PlusEq) => AssignOp::Add,
            Tok::Punct(P::MinusEq) => AssignOp::Sub,
            Tok::Punct(P::StarEq) => AssignOp::Mul,
            Tok::Punct(P::SlashEq) => AssignOp::Div,
            Tok::Punct(P::PercentEq) => AssignOp::Mod,
            Tok::Punct(P::StarStarEq) => AssignOp::Exp,
            Tok::Punct(P::ShlEq) => AssignOp::Shl,
            Tok::Punct(P::ShrEq) => AssignOp::Shr,
            Tok::Punct(P::UShrEq) => AssignOp::UShr,
            Tok::Punct(P::AmpEq) => AssignOp::BitAnd,
            Tok::Punct(P::PipeEq) => AssignOp::BitOr,
            Tok::Punct(P::CaretEq) => AssignOp::BitXor,
            Tok::Punct(P::AmpAmpEq) => AssignOp::And,
            Tok::Punct(P::PipePipeEq) => AssignOp::Or,
            Tok::Punct(P::QuestionQuestionEq) => AssignOp::Nullish,
            _ => return Ok(left),
        };
        self.bump()?;
        // Nur bei `=` darf links ein Muster stehen; `[a] += 1` gibt es nicht.
        let pat = if op == AssignOp::Assign {
            self.expr_to_pattern(left, false)?
        } else {
            match &left {
                Expr::Ident(_) | Expr::Member { .. } => Pat::Expr(Box::new(left)),
                // Annex B B.3.5: im lockeren Modus ist ein Aufruf ein
                // gueltiges Zuweisungsziel (es wirft dann zur Laufzeit).
                Expr::Call { .. } if !self.strict => Pat::Expr(Box::new(left)),
                _ => return self.err("invalid assignment target"),
            }
        };
        let right = self.assign_expr_impl(no_in)?;
        Ok(Expr::Assign { op, left: Box::new(pat), right: Box::new(right) })
    }

    fn try_simple_arrow(&mut self) -> R<Option<Expr>> {
        let is_ident = matches!(self.cur.tok, Tok::Ident(_))
            || matches!(&self.cur.tok, Tok::Keyword(k) if !k.is_reserved());
        if !is_ident { return Ok(None); }
        let save = (self.lx.pos, self.cur.clone(), self.cur_start);

        // `async x => …` und `async (…) => …`. Die geklammerte Form ist die
        // haeufigere in echtem Code und war der zweite Grund, aus dem der erste
        // Lauf gueltige Dateien ablehnte.
        if self.is_kw(Kw::Async) {
            self.bump()?;
            if !self.cur.newline_before {
                if matches!(self.cur.tok, Tok::Ident(_)) {
                    let p = Pat::Ident(self.ident_name()?);
                    if self.is_p(P::Arrow) {
                        self.bump()?;
                        return Ok(Some(self.arrow_body(vec![p], true)?));
                    }
                } else if self.is_p(P::LParen) {
                    // Als Argumentliste lesen und beim `=>` umbiegen — dieselbe
                    // Deckgrammatik wie beim gewoehnlichen Pfeil. Ohne `=>` ist
                    // es ein Aufruf `async(…)`, und der Ruecksetzpunkt traegt.
                    if let Ok(args) = self.arguments() {
                        if self.is_p(P::Arrow) && !self.cur.newline_before {
                            self.bump()?;
                            let mut params = Vec::with_capacity(args.len());
                            for a in args {
                                let (e, spread) = match a {
                                    Arg::Expr(e) => (e, false),
                                    Arg::Spread(e) => (e, true),
                                };
                                let p = self.expr_to_pattern(e, true)?;
                                params.push(if spread { Pat::Rest(Box::new(p)) } else { p });
                            }
                            return Ok(Some(self.arrow_body(params, true)?));
                        }
                    }
                }
            }
            self.restore(save);
            return Ok(None);
        }
        let name = self.ident_name()?;
        if self.is_p(P::Arrow) && !self.cur.newline_before {
            self.bump()?;
            return Ok(Some(self.arrow_body(vec![Pat::Ident(name)], false)?));
        }
        self.restore(save);
        Ok(None)
    }

    fn arrow_body(&mut self, params: Vec<Pat>, is_async: bool) -> R<Expr> {
        let (og, oa, of) = (self.in_gen, self.in_async, self.in_func);
        // Ein Pfeil hat kein eigenes `yield`-Verhalten; `in_gen` bleibt aussen
        // stehen, weil `yield` im Pfeilkoerper den umgebenden Generator meint.
        self.in_async = is_async; self.in_func = true;
        let (body, expr_body) = if self.is_p(P::LBrace) {
            let outer_strict = self.strict;
            let b = self.block_body_with_prologue()?;
            self.strict = outer_strict;
            (b, false)
        } else {
            let e = self.assign_expr()?;
            (vec![Stmt::Return(Some(e))], true)
        };
        self.in_gen = og; self.in_async = oa; self.in_func = of;
        Ok(Expr::Func(Box::new(Func {
            name: None, params, body, is_async, is_generator: false, is_arrow: true, expr_body,
        })))
    }

    fn yield_expr(&mut self) -> R<Expr> {
        self.bump()?;
        let delegate = self.eat_p(P::Star)?;
        // Nach `yield*` MUSS ein Ausdruck folgen — auch ueber eine Zeile
        // hinweg. Die Semikolon-Einfuegung greift nur beim nackten `yield`.
        let arg = if (!delegate && self.cur.newline_before) || self.is_p(P::RParen) || self.is_p(P::RBracket)
            || self.is_p(P::RBrace) || self.is_p(P::Comma) || self.is_p(P::Semi)
            || self.is_p(P::Colon) || self.cur.tok == Tok::Eof {
            None
        } else { Some(Box::new(self.assign_expr()?)) };
        if delegate && arg.is_none() { return self.err("yield* requires an argument"); }
        Ok(Expr::Yield { arg, delegate })
    }

    fn conditional(&mut self, no_in: bool) -> R<Expr> {
        let test = self.binary(0, no_in)?;
        if !self.eat_p(P::Question)? { return Ok(test); }
        // Die beiden Zweige sind AssignmentExpression — `in` gilt darin wieder.
        let cons = self.assign_expr()?;
        self.expect_p(P::Colon)?;
        let alt = self.assign_expr_impl(no_in)?;
        Ok(Expr::Cond { test: Box::new(test), cons: Box::new(cons), alt: Box::new(alt) })
    }

    /// Vorrangkletterung. Die Tabelle ist die aus der Spezifikation; `**` ist
    /// rechtsassoziativ, alles andere links.
    fn binary(&mut self, min_prec: u8, no_in: bool) -> R<Expr> {
        let mut left = self.unary()?;
        let mut left_paren = self.just_paren;
        loop {
            let (op, prec, logical) = match &self.cur.tok {
                Tok::Punct(P::PipePipe) => (None, 1, Some(LogicalOp::Or)),
                Tok::Punct(P::QuestionQuestion) => (None, 1, Some(LogicalOp::Nullish)),
                Tok::Punct(P::AmpAmp) => (None, 2, Some(LogicalOp::And)),
                Tok::Punct(P::Pipe) => (Some(BinOp::BitOr), 3, None),
                Tok::Punct(P::Caret) => (Some(BinOp::BitXor), 4, None),
                Tok::Punct(P::Amp) => (Some(BinOp::BitAnd), 5, None),
                Tok::Punct(P::EqEq) => (Some(BinOp::EqEq), 6, None),
                Tok::Punct(P::NotEq) => (Some(BinOp::NotEq), 6, None),
                Tok::Punct(P::EqEqEq) => (Some(BinOp::EqEqEq), 6, None),
                Tok::Punct(P::NotEqEq) => (Some(BinOp::NotEqEq), 6, None),
                Tok::Punct(P::Lt) => (Some(BinOp::Lt), 7, None),
                Tok::Punct(P::Gt) => (Some(BinOp::Gt), 7, None),
                Tok::Punct(P::LtEq) => (Some(BinOp::LtEq), 7, None),
                Tok::Punct(P::GtEq) => (Some(BinOp::GtEq), 7, None),
                Tok::Keyword(Kw::Instanceof) => (Some(BinOp::Instanceof), 7, None),
                Tok::Keyword(Kw::In) if !no_in => (Some(BinOp::In), 7, None),
                Tok::Punct(P::Shl) => (Some(BinOp::Shl), 8, None),
                Tok::Punct(P::Shr) => (Some(BinOp::Shr), 8, None),
                Tok::Punct(P::UShr) => (Some(BinOp::UShr), 8, None),
                Tok::Punct(P::Plus) => (Some(BinOp::Add), 9, None),
                Tok::Punct(P::Minus) => (Some(BinOp::Sub), 9, None),
                Tok::Punct(P::Star) => (Some(BinOp::Mul), 10, None),
                Tok::Punct(P::Slash) => (Some(BinOp::Div), 10, None),
                Tok::Punct(P::Percent) => (Some(BinOp::Mod), 10, None),
                Tok::Punct(P::StarStar) => (Some(BinOp::Exp), 11, None),
                _ => break,
            };
            if prec < min_prec { break; }
            // `??` darf sich nicht ungeklammert mit `&&`/`||` mischen.
            if let Some(LogicalOp::Nullish) = logical {
                if !left_paren
                    && matches!(&left, Expr::Logical { op: LogicalOp::And | LogicalOp::Or, .. }) {
                    return self.err("?? cannot be mixed with && or ||");
                }
            }
            let exp = op == Some(BinOp::Exp);
            self.bump()?;
            self.just_paren = false;
            let right = self.binary(if exp { prec } else { prec + 1 }, no_in)?;
            let right_paren = self.just_paren;
            left_paren = false;
            if let Some(lg) = logical {
                if lg != LogicalOp::Nullish && !right_paren {
                    if let Expr::Logical { op: LogicalOp::Nullish, .. } = &right {
                        return self.err("?? cannot be mixed with && or ||");
                    }
                }
                left = Expr::Logical { op: lg, left: Box::new(left), right: Box::new(right) };
            } else {
                left = Expr::Binary { op: op.unwrap(), left: Box::new(left), right: Box::new(right) };
            }
        }
        Ok(left)
    }

    fn unary(&mut self) -> R<Expr> {
        self.just_paren = false;
        let op = match &self.cur.tok {
            Tok::Punct(P::Minus) => Some(UnaryOp::Minus),
            Tok::Punct(P::Plus) => Some(UnaryOp::Plus),
            Tok::Punct(P::Bang) => Some(UnaryOp::Bang),
            Tok::Punct(P::Tilde) => Some(UnaryOp::Tilde),
            Tok::Keyword(Kw::Typeof) => Some(UnaryOp::Typeof),
            Tok::Keyword(Kw::Void) => Some(UnaryOp::Void),
            Tok::Keyword(Kw::Delete) => Some(UnaryOp::Delete),
            _ => None,
        };
        if let Some(op) = op {
            self.bump()?;
            let arg = self.unary()?;
            // `delete x` auf einen blossen Bezeichner ist im strengen Modus
            // ein Fruehfehler.
            if op == UnaryOp::Delete && self.strict && matches!(arg, Expr::Ident(_)) {
                return self.err("delete of an unqualified identifier in strict mode");
            }
            // `-a ** b` ist mehrdeutig und deshalb verboten.
            if self.is_p(P::StarStar) { return self.err("unparenthesized unary before **"); }
            return Ok(Expr::Unary { op, arg: Box::new(arg) });
        }
        if self.is_p(P::Inc) || self.is_p(P::Dec) {
            let op = if self.is_p(P::Inc) { UpdateOp::Inc } else { UpdateOp::Dec };
            self.bump()?;
            let arg = self.unary()?;
            self.check_simple_target(&arg)?;
            return Ok(Expr::Update { op, arg: Box::new(arg), prefix: true });
        }
        if self.is_kw(Kw::Await) && (self.in_async || self.module) {
            self.bump()?;
            let arg = self.unary()?;
            return Ok(Expr::Await(Box::new(arg)));
        }
        let e = self.postfix()?;
        Ok(e)
    }

    fn postfix(&mut self) -> R<Expr> {
        let e = self.lhs_expr()?;
        if (self.is_p(P::Inc) || self.is_p(P::Dec)) && !self.cur.newline_before {
            let op = if self.is_p(P::Inc) { UpdateOp::Inc } else { UpdateOp::Dec };
            self.check_simple_target(&e)?;
            self.bump()?;
            return Ok(Expr::Update { op, arg: Box::new(e), prefix: false });
        }
        Ok(e)
    }

    fn check_simple_target(&self, e: &Expr) -> R<()> {
        match e {
            Expr::Ident(_) | Expr::Member { .. } => Ok(()),
            Expr::Call { .. } if !self.strict => Ok(()),
            _ => Err(ParseError { msg: "invalid update target".to_string(), at: self.cur.start }),
        }
    }

    fn lhs_expr(&mut self) -> R<Expr> {
        let mut e = if self.is_kw(Kw::New) { self.new_expr()? } else { self.primary()? };
        let mut saw_optional = false;
        loop {
            if self.eat_p(P::Dot)? {
                let prop = if self.eat_p(P::Hash)? {
                    MemberProp::Private(self.property_name()?)
                } else { MemberProp::Ident(self.property_name()?) };
                e = Expr::Member { obj: Box::new(e), prop: Box::new(prop), optional: false };
            } else if self.is_p(P::QuestionDot) {
                self.bump()?;
                saw_optional = true;
                if self.is_p(P::LParen) {
                    let args = self.arguments()?;
                    e = Expr::Call { callee: Box::new(e), args, optional: true };
                } else if self.eat_p(P::LBracket)? {
                    let idx = self.expression()?;
                    self.expect_p(P::RBracket)?;
                    e = Expr::Member { obj: Box::new(e), prop: Box::new(MemberProp::Computed(idx)), optional: true };
                } else if self.eat_p(P::Hash)? {
                    e = Expr::Member { obj: Box::new(e), prop: Box::new(MemberProp::Private(self.property_name()?)), optional: true };
                } else {
                    e = Expr::Member { obj: Box::new(e), prop: Box::new(MemberProp::Ident(self.property_name()?)), optional: true };
                }
            } else if self.eat_p(P::LBracket)? {
                let idx = self.expression()?;
                self.expect_p(P::RBracket)?;
                e = Expr::Member { obj: Box::new(e), prop: Box::new(MemberProp::Computed(idx)), optional: false };
            } else if self.is_p(P::LParen) {
                let args = self.arguments()?;
                e = Expr::Call { callee: Box::new(e), args, optional: false };
            } else if matches!(self.cur.tok, Tok::Template { .. }) {
                if saw_optional { return self.err("tagged template in optional chain"); }
                let (quasis, exprs) = self.template_parts()?;
                e = Expr::TaggedTemplate { tag: Box::new(e), quasis, exprs };
            } else { break; }
        }
        Ok(if saw_optional { Expr::Chain(Box::new(e)) } else { e })
    }

    fn new_expr(&mut self) -> R<Expr> {
        self.bump()?; // `new`
        if self.is_p(P::Dot) {
            self.bump()?;
            let p = self.property_name()?;
            if p != "target" { return self.err("expected new.target"); }
            return Ok(Expr::MetaProp { meta: "new".to_string(), prop: "target".to_string() });
        }
        let callee = if self.is_kw(Kw::New) { self.new_expr()? } else { self.primary()? };
        // Die Glieder VOR den Argumenten gehoeren noch zum Konstruktor:
        // `new a.b.C()` ruft `a.b.C`.
        let mut callee = callee;
        loop {
            if self.eat_p(P::Dot)? {
                let prop = MemberProp::Ident(self.property_name()?);
                callee = Expr::Member { obj: Box::new(callee), prop: Box::new(prop), optional: false };
            } else if self.eat_p(P::LBracket)? {
                let idx = self.expression()?;
                self.expect_p(P::RBracket)?;
                callee = Expr::Member { obj: Box::new(callee), prop: Box::new(MemberProp::Computed(idx)), optional: false };
            } else { break; }
        }
        let args = if self.is_p(P::LParen) { self.arguments()? } else { Vec::new() };
        Ok(Expr::New { callee: Box::new(callee), args })
    }

    fn arguments(&mut self) -> R<Vec<Arg>> {
        self.expect_p(P::LParen)?;
        let mut out = Vec::new();
        while !self.is_p(P::RParen) {
            if self.eat_p(P::Ellipsis)? { out.push(Arg::Spread(self.assign_expr()?)); }
            else { out.push(Arg::Expr(self.assign_expr()?)); }
            if !self.eat_p(P::Comma)? { break; }
        }
        self.expect_p(P::RParen)?;
        Ok(out)
    }

    fn template_parts(&mut self) -> R<(Vec<TemplateElement>, Vec<Expr>)> {
        let mut quasis = Vec::new();
        let mut exprs = Vec::new();
        loop {
            let (cooked, raw, has_sub) = match self.cur.tok.clone() {
                Tok::Template { cooked, raw, has_sub } => (cooked, raw, has_sub),
                _ => return self.err("expected template"),
            };
            quasis.push(TemplateElement { cooked, raw });
            if !has_sub { self.bump()?; break; }
            self.bump()?;
            exprs.push(self.expression()?);
            // Nach dem Ausdruck steht `}` — aber als Fortsetzung des Templates,
            // nicht als Satzzeichen. Der Lexer muss ab DORT weiterlesen, also
            // wird an der Stelle des `}` neu angesetzt.
            if !self.is_p(P::RBrace) { return self.err("expected } in template"); }
            self.lx.pos = self.cur.start + 1;
            let t = self.lx.template_part().map_err(|e| ParseError { msg: e.msg.to_string(), at: e.at })?;
            let end = self.lx.pos;
            self.cur = Token { tok: t, start: end, end, newline_before: false };
        }
        Ok((quasis, exprs))
    }

    fn primary(&mut self) -> R<Expr> {
        match self.cur.tok.clone() {
            Tok::Num(n) => { self.bump()?; Ok(Expr::Num(n)) }
            Tok::BigInt(s) => { self.bump()?; Ok(Expr::BigInt(s)) }
            Tok::Str(s) => { self.bump()?; Ok(Expr::Str(s)) }
            Tok::Regex(b, f) => { self.bump()?; Ok(Expr::Regex { body: b, flags: f }) }
            Tok::Template { .. } => {
                let (quasis, exprs) = self.template_parts()?;
                Ok(Expr::Template { quasis, exprs })
            }
            Tok::Ident(_) => Ok(Expr::Ident(self.ident_name()?)),
            Tok::Punct(P::Hash) => {
                // `#x in obj` — die Pruefung auf ein privates Feld.
                self.bump()?;
                let name = self.property_name()?;
                Ok(Expr::Ident(alloc::format!("#{name}")))
            }
            Tok::Punct(P::LParen) => self.paren_or_arrow(),
            Tok::Punct(P::LBracket) => {
                self.bump()?;
                let mut out = Vec::new();
                while !self.is_p(P::RBracket) {
                    if self.is_p(P::Comma) { self.bump()?; out.push(None); continue; }
                    let e = if self.eat_p(P::Ellipsis)? {
                        Expr::Spread(Box::new(self.assign_expr()?))
                    } else { self.assign_expr()? };
                    out.push(Some(e));
                    if !self.eat_p(P::Comma)? { break; }
                }
                self.expect_p(P::RBracket)?;
                Ok(Expr::Array(out))
            }
            Tok::Punct(P::LBrace) => self.object_literal(),
            Tok::Keyword(k) => match k {
                Kw::This => { self.bump()?; Ok(Expr::This) }
                Kw::Super => { self.bump()?; Ok(Expr::Super) }
                Kw::True => { self.bump()?; Ok(Expr::Bool(true)) }
                Kw::False => { self.bump()?; Ok(Expr::Bool(false)) }
                Kw::Null => { self.bump()?; Ok(Expr::Null) }
                Kw::Function => Ok(Expr::Func(Box::new(self.function(false, false)?))),
                Kw::Class => Ok(Expr::Class(Box::new(self.class(false)?))),
                Kw::Async if self.async_function_ahead()? => {
                    self.bump()?;
                    Ok(Expr::Func(Box::new(self.function(true, false)?)))
                }
                Kw::New => self.new_expr(),
                Kw::Import => {
                    self.bump()?;
                    if self.eat_p(P::Dot)? {
                        let p = self.property_name()?;
                        if p != "meta" { return self.err("expected import.meta"); }
                        return Ok(Expr::MetaProp { meta: "import".to_string(), prop: "meta".to_string() });
                    }
                    Ok(Expr::ImportCall(self.arguments()?))
                }
                _ if !k.is_reserved() => Ok(Expr::Ident(self.ident_name()?)),
                Kw::Yield if !self.in_gen && !self.strict => Ok(Expr::Ident(self.ident_name()?)),
                Kw::Await if !self.in_async && !self.module => Ok(Expr::Ident(self.ident_name()?)),
                _ => self.err("unexpected keyword"),
            },
            _ => self.err("unexpected token"),
        }
    }

    /// `(` — entweder eine Klammer, eine Sequenz, oder die Parameterliste
    /// eines Pfeils. Welches, sagt erst das Zeichen NACH dem `)`.
    fn paren_or_arrow(&mut self) -> R<Expr> {
        self.bump()?;
        // `()` kann nur ein Pfeil sein.
        if self.is_p(P::RParen) {
            self.bump()?;
            if !self.is_p(P::Arrow) { return self.err("empty parenthesized expression"); }
            self.bump()?;
            return self.arrow_body(Vec::new(), false);
        }
        let mut items: Vec<Expr> = Vec::new();
        let mut rest: Option<Pat> = None;
        loop {
            if self.eat_p(P::Ellipsis)? {
                rest = Some(Pat::Rest(Box::new(self.binding_pattern()?)));
                break;
            }
            items.push(self.assign_expr()?);
            if !self.eat_p(P::Comma)? { break; }
            if self.is_p(P::RParen) { break; } // erlaubtes Schlusskomma
        }
        self.expect_p(P::RParen)?;

        if self.is_p(P::Arrow) && !self.cur.newline_before {
            self.bump()?;
            let mut params = Vec::with_capacity(items.len());
            for e in items { params.push(self.expr_to_pattern(e, true)?); }
            if let Some(r) = rest { params.push(r); }
            return self.arrow_body(params, false);
        }
        if rest.is_some() { return self.err("rest parameter outside arrow function"); }
        self.just_paren = true;
        Ok(if items.len() == 1 { items.pop().unwrap() } else { Expr::Seq(items) })
    }

    fn object_literal(&mut self) -> R<Expr> {
        self.bump()?;
        let mut props = Vec::new();
        while !self.is_p(P::RBrace) {
            if self.eat_p(P::Ellipsis)? {
                let e = self.assign_expr()?;
                props.push(ObjProp {
                    key: PropKey::Ident(String::new()),
                    value: ObjPropValue::Spread(e), computed: false, shorthand: false,
                });
                if !self.eat_p(P::Comma)? { break; }
                continue;
            }
            let mut is_async = false;
            let mut is_generator = false;
            let mut kind = 0u8; // 0 normal, 1 get, 2 set

            if self.is_kw(Kw::Async) {
                let save = (self.lx.pos, self.cur.clone(), self.cur_start);
                self.bump()?;
                if self.is_p(P::Colon) || self.is_p(P::LParen) || self.is_p(P::Comma)
                    || self.is_p(P::RBrace) || self.is_p(P::Eq) || self.cur.newline_before {
                    self.restore(save);
                } else { is_async = true; }
            }
            if self.eat_p(P::Star)? { is_generator = true; }
            if (self.is_kw(Kw::Get) || self.is_kw(Kw::Set)) && !is_generator && !is_async {
                let want = if self.is_kw(Kw::Get) { 1 } else { 2 };
                let save = (self.lx.pos, self.cur.clone(), self.cur_start);
                self.bump()?;
                if self.is_p(P::Colon) || self.is_p(P::LParen) || self.is_p(P::Comma)
                    || self.is_p(P::RBrace) || self.is_p(P::Eq) {
                    self.restore(save);
                } else { kind = want; }
            }

            let (key, computed) = self.property_key()?;

            if kind != 0 {
                let (og, oa, of) = (self.in_gen, self.in_async, self.in_func);
                self.in_gen = false; self.in_async = false; self.in_func = true;
                let params = self.params()?;
                let body = self.block_body_with_prologue()?;
                self.in_gen = og; self.in_async = oa; self.in_func = of;
                let f = Box::new(Func { name: None, params, body, is_async: false,
                    is_generator: false, is_arrow: false, expr_body: false });
                props.push(ObjProp {
                    key, computed, shorthand: false,
                    value: if kind == 1 { ObjPropValue::Get(f) } else { ObjPropValue::Set(f) },
                });
            } else if self.is_p(P::LParen) {
                let (og, oa, of) = (self.in_gen, self.in_async, self.in_func);
                self.in_gen = is_generator; self.in_async = is_async; self.in_func = true;
                let params = self.params()?;
                let body = self.block_body_with_prologue()?;
                self.in_gen = og; self.in_async = oa; self.in_func = of;
                props.push(ObjProp {
                    key, computed, shorthand: false,
                    value: ObjPropValue::Method(Box::new(Func { name: None, params, body,
                        is_async, is_generator, is_arrow: false, expr_body: false })),
                });
            } else if self.eat_p(P::Colon)? {
                let v = self.assign_expr()?;
                props.push(ObjProp { key, value: ObjPropValue::Init(v), computed, shorthand: false });
            } else {
                // Kurzform — und `{a = 1}` ist NUR als Muster gueltig. Hier als
                // Zuweisung aufgehoben, `expr_to_pattern` biegt es um; ein
                // Objektliteral mit dieser Form scheitert dort.
                let name = match &key {
                    PropKey::Ident(n) => n.clone(),
                    _ => return self.err("invalid shorthand property"),
                };
                let v = if self.eat_p(P::Eq)? {
                    Expr::Assign {
                        op: AssignOp::Assign,
                        left: Box::new(Pat::Ident(name.clone())),
                        right: Box::new(self.assign_expr()?),
                    }
                } else { Expr::Ident(name) };
                props.push(ObjProp { key, value: ObjPropValue::Init(v), computed: false, shorthand: true });
            }
            if !self.eat_p(P::Comma)? { break; }
        }
        self.expect_p(P::RBrace)?;
        Ok(Expr::Object(props))
    }

    // ── Deckgrammatik: Ausdruck zu Muster ────────────────────────────────
    /// `binding` = die Stelle verlangt eine BINDUNG (Deklaration, Parameter).
    /// Dort sind nur Bezeichner und Muster erlaubt; in einer Zuweisung darf
    /// auch `a.b` oder `a[0]` links stehen.
    fn expr_to_pattern(&mut self, e: Expr, binding: bool) -> R<Pat> {
        Ok(match e {
            Expr::Ident(n) => {
                if self.strict && (n == "eval" || n == "arguments") && binding {
                    return self.err("cannot bind eval or arguments in strict mode");
                }
                Pat::Ident(n)
            }
            Expr::Member { .. } if !binding => Pat::Expr(Box::new(e)),
            Expr::Call { .. } if !binding && !self.strict => Pat::Expr(Box::new(e)),
            Expr::Assign { op: AssignOp::Assign, left, right } => {
                Pat::Assign { left, right }
            }
            Expr::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                let n = items.len();
                for (i, it) in items.into_iter().enumerate() {
                    out.push(match it {
                        None => None,
                        Some(Expr::Spread(inner)) => {
                            if i + 1 != n { return self.err("rest element must be last"); }
                            Some(Pat::Rest(Box::new(self.expr_to_pattern(*inner, binding)?)))
                        }
                        Some(x) => Some(self.expr_to_pattern(x, binding)?),
                    });
                }
                Pat::Array(out)
            }
            Expr::Object(props) => {
                let mut out = Vec::new();
                let mut rest = None;
                let n = props.len();
                for (i, p) in props.into_iter().enumerate() {
                    match p.value {
                        ObjPropValue::Spread(inner) => {
                            if i + 1 != n { return self.err("rest property must be last"); }
                            rest = Some(Box::new(self.expr_to_pattern(inner, binding)?));
                        }
                        ObjPropValue::Init(v) => {
                            let value = self.expr_to_pattern(v, binding)?;
                            out.push(ObjPatProp { key: p.key, value, computed: p.computed, shorthand: p.shorthand });
                        }
                        _ => return self.err("invalid destructuring target"),
                    }
                }
                Pat::Object { props: out, rest }
            }
            Expr::Spread(inner) => Pat::Rest(Box::new(self.expr_to_pattern(*inner, binding)?)),
            _ => return self.err("invalid assignment target"),
        })
    }
}

/// Ein Programm parsen. `module` waehlt die Grammatik, nicht nur einen Schalter.
pub fn parse(src: &str, module: bool) -> Result<Program, ParseError> {
    Parser::new(src, module)?.parse_program()
}
