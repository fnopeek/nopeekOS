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

use alloc::rc::Rc;
use alloc::vec::Vec;

use super::ast::*;
use super::code::*;
use super::value::Value;

/// Wohin `break`/`continue` springen. Ein Eintrag je offener Schleife.
struct Loop {
    /// Stellen, die auf das Ende der Schleife gepatcht werden.
    breaks: Vec<usize>,
    /// Stellen, die auf den Fortsetzungspunkt gepatcht werden.
    continues: Vec<usize>,
    /// Wieviele Umgebungen beim Betreten offen waren.
    ///
    /// Ein `break` aus einem Block heraus springt an dessen `PopEnv` VORBEI.
    /// Ohne diese Zahl bleibt die Umgebung offen, der naechste `PopEnv` nimmt
    /// die falsche, und eine Bindung aus dem Block ist danach draussen noch
    /// zu sehen — sechs annexB-Tests, die genau das pruefen.
    depth: usize,
}

pub struct Compiler {
    pub chunk: Chunk,
    loops: Vec<Loop>,
    /// Wieviele `PushEnv` gerade offen sind.
    depth: usize,
}

/// Einen FUNKTIONSRUMPF uebersetzen.
///
/// Unterschied zum Programm: kein Abschlusswert (eine Funktion ohne `return`
/// gibt `undefined`), und am Ende steht ein `Ret`, das genau das tut.
/// Parameter und `this` liegen schon in der Umgebung, die `Interp::call_env`
/// gebaut hat — der Rumpf faengt beim ersten Statement an.
pub fn function(f: &Func) -> CompileResult<Chunk> {
    if f.is_generator || f.is_async {
        return Err(Unsupported("generator-or-async"));
    }
    let mut c = Compiler { chunk: Chunk::new(), loops: Vec::new(), depth: 0 };
    for st in &f.body {
        c.stmt_no_completion(st)?;
    }
    let k = c.chunk.konst(Value::Undefined);
    c.chunk.emit(Op::Const(k));
    c.chunk.emit(Op::Ret);
    Ok(c.chunk)
}

/// Ein ganzes Programm uebersetzen. `Err` heisst: der Baumlaeufer macht es.
pub fn program(prog: &Program) -> CompileResult<Chunk> {
    let mut c = Compiler { chunk: Chunk::new(), loops: Vec::new(), depth: 0 };
    // Hochziehen bleibt beim Baumlaeufer (`Interp::hoist`) — es arbeitet auf
    // der Umgebung, nicht auf dem Code, und ist damit fuer beide Maschinen
    // dasselbe. Hier nur der Rumpf.
    for st in &prog.body {
        c.stmt(st)?;
    }
    c.chunk.emit(Op::Ret);
    Ok(c.chunk)
}

impl Compiler {
    // ── Anweisungen ──────────────────────────────────────────────────────
    /// Wie `stmt`, aber ohne Abschlusswert. In einer FUNKTION gibt es keinen —
    /// ihr Wert ist ihr `return`, und ein `SetCompletion` je Anweisung waere
    /// Arbeit fuer nichts.
    fn stmt_no_completion(&mut self, st: &Stmt) -> CompileResult<()> {
        match st {
            Stmt::Expr(e) => {
                self.expr(e)?;
                self.chunk.emit(Op::Pop);
                Ok(())
            }
            other => self.stmt(other),
        }
    }

    fn stmt(&mut self, st: &Stmt) -> CompileResult<()> {
        match st {
            Stmt::Empty | Stmt::Debugger => Ok(()),
            // Der Wert eines Programms ist sein letzter Ausdruckswert.
            Stmt::Expr(e) => {
                self.expr(e)?;
                self.chunk.emit(Op::SetCompletion);
                Ok(())
            }
            Stmt::Block(body) => {
                let b = self.block_decls(body)?;
                self.chunk.emit(Op::PushEnv(b));
                self.depth += 1;
                for s in body {
                    self.stmt(s)?;
                }
                self.chunk.emit(Op::PopEnv);
                self.depth -= 1;
                Ok(())
            }
            Stmt::VarDecl(d) => self.var_decl(d),
            Stmt::If { test, cons, alt } => {
                self.expr(test)?;
                let to_else = self.chunk.emit_jump(Op::JumpFalse);
                self.stmt(cons)?;
                match alt {
                    None => self.chunk.patch(to_else),
                    Some(a) => {
                        let to_end = self.chunk.emit_jump(Op::Jump);
                        self.chunk.patch(to_else);
                        self.stmt(a)?;
                        self.chunk.patch(to_end);
                    }
                }
                Ok(())
            }
            Stmt::While { test, body } => {
                let top = self.chunk.here();
                self.expr(test)?;
                let out = self.chunk.emit_jump(Op::JumpFalse);
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), depth: self.depth });
                self.stmt(body)?;
                let l = self.loops.pop().unwrap();
                for at in l.continues {
                    self.patch_to(at, top);
                }
                self.chunk.emit(Op::Jump(top));
                self.chunk.patch(out);
                for at in l.breaks {
                    self.chunk.patch(at);
                }
                Ok(())
            }
            Stmt::DoWhile { body, test } => {
                let top = self.chunk.here();
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), depth: self.depth });
                self.stmt(body)?;
                let l = self.loops.pop().unwrap();
                let cond = self.chunk.here();
                for at in l.continues {
                    self.patch_to(at, cond);
                }
                self.expr(test)?;
                let out = self.chunk.emit_jump(Op::JumpFalse);
                self.chunk.emit(Op::Jump(top));
                self.chunk.patch(out);
                for at in l.breaks {
                    self.chunk.patch(at);
                }
                Ok(())
            }
            Stmt::For { init, test, update, body } => {
                // Eine eigene Umgebung, damit `for (let i …)` seinen Zaehler
                // nicht in den umgebenden Block schreibt. Dass jeder Umlauf
                // eine FRISCHE Bindung bekaeme (die Schliessungsfalle), kann
                // diese Fassung noch nicht — deshalb sagt sie unten nein.
                let empty = self.chunk.block(Vec::new());
                self.chunk.emit(Op::PushEnv(empty));
                self.depth += 1;
                match init {
                    None => {}
                    Some(f) => match &**f {
                        ForInit::Expr(e) => {
                            self.expr(e)?;
                            self.chunk.emit(Op::Pop);
                        }
                        ForInit::VarDecl(d) => {
                            if d.kind != VarKind::Var && self.captures(body) {
                                return Err(Unsupported("for-let-capture"));
                            }
                            self.var_decl(d)?;
                        }
                    },
                }
                let top = self.chunk.here();
                let out = match test {
                    None => None,
                    Some(t) => {
                        self.expr(t)?;
                        Some(self.chunk.emit_jump(Op::JumpFalse))
                    }
                };
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), depth: self.depth });
                self.stmt(body)?;
                let l = self.loops.pop().unwrap();
                let cont = self.chunk.here();
                for at in l.continues {
                    self.patch_to(at, cont);
                }
                if let Some(u) = update {
                    self.expr(u)?;
                    self.chunk.emit(Op::Pop);
                }
                self.chunk.emit(Op::Jump(top));
                if let Some(o) = out {
                    self.chunk.patch(o);
                }
                for at in l.breaks {
                    self.chunk.patch(at);
                }
                self.chunk.emit(Op::PopEnv);
                self.depth -= 1;
                Ok(())
            }
            Stmt::Break(None) => {
                let Some(d) = self.loops.last().map(|l| l.depth) else {
                    return Err(Unsupported("break-outside-loop"));
                };
                self.unwind_to(d);
                let at = self.chunk.emit_jump(Op::Jump);
                self.loops.last_mut().unwrap().breaks.push(at);
                Ok(())
            }
            Stmt::Continue(None) => {
                let Some(d) = self.loops.last().map(|l| l.depth) else {
                    return Err(Unsupported("continue-outside-loop"));
                };
                self.unwind_to(d);
                let at = self.chunk.emit_jump(Op::Jump);
                self.loops.last_mut().unwrap().continues.push(at);
                Ok(())
            }
            Stmt::Return(e) => {
                match e {
                    None => {
                        let k = self.chunk.konst(Value::Undefined);
                        self.chunk.emit(Op::Const(k));
                    }
                    Some(x) => self.expr(x)?,
                }
                self.chunk.emit(Op::Ret);
                Ok(())
            }
            Stmt::Throw(e) => {
                self.expr(e)?;
                self.chunk.emit(Op::Throw);
                Ok(())
            }
            // Funktionsdeklarationen erledigt das Hochziehen, genau wie beim
            // Baumlaeufer — hier ist nichts zu tun.
            Stmt::Func(_) => Ok(()),
            Stmt::Labeled { .. } => Err(Unsupported("label")),
            Stmt::Break(Some(_)) | Stmt::Continue(Some(_)) => Err(Unsupported("labeled-jump")),
            Stmt::Switch { .. } => Err(Unsupported("switch")),
            Stmt::Try { block, handler, finalizer } => self.try_stmt(block, handler, finalizer),
            Stmt::ForIn { .. } => Err(Unsupported("for-in")),
            Stmt::ForOf { left, right, body, is_await } => {
                if *is_await { return Err(Unsupported("for-await")) }
                self.for_of(left, right, body)
            }
            Stmt::With { .. } => Err(Unsupported("with")),
            Stmt::Class(_) => Err(Unsupported("class")),
            Stmt::Import(_) | Stmt::ExportNamed { .. } | Stmt::ExportDefault(_)
            | Stmt::ExportAll { .. } => Err(Unsupported("module")),
        }
    }

    /// Was ein Block bindet, BEVOR seine erste Zeile laeuft — dieselben zwei
    /// Faelle und dieselbe Reihenfolge wie `Interp::hoist`. `var` steht nicht
    /// dabei: das steigt bis zur Funktionsgrenze und ist beim Programmstart
    /// schon erledigt.
    fn block_decls(&mut self, body: &[Stmt]) -> CompileResult<u32> {
        let mut out = Vec::new();
        for st in body {
            match st {
                Stmt::Func(f) => {
                    if f.is_generator || f.is_async {
                        return Err(Unsupported("generator-or-async"));
                    }
                    if let Some(n) = &f.name {
                        let name = self.chunk.name(n);
                        let func = self.chunk.func(f.clone());
                        out.push(BlockDecl::Func { name, func });
                    }
                }
                Stmt::VarDecl(d) if d.kind != VarKind::Var => {
                    for dec in &d.decls {
                        let Pat::Ident(n) = &dec.id else {
                            return Err(Unsupported("destructuring-decl"));
                        };
                        let name = self.chunk.name(n);
                        out.push(BlockDecl::Tdz { name, mutable: d.kind != VarKind::Const });
                    }
                }
                _ => {}
            }
        }
        Ok(self.chunk.block(out))
    }

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
    fn try_stmt(&mut self, block: &[Stmt], handler: &Option<CatchClause>,
                finalizer: &Option<Vec<Stmt>>) -> CompileResult<()> {
        if finalizer.is_some() && (Self::jumps_out(block)
            || handler.as_ref().is_some_and(|h| Self::jumps_out(&h.body))) {
            return Err(Unsupported("finally-with-jump"));
        }
        let start = self.chunk.emit(Op::TryStart { catch: u32::MAX, finally: u32::MAX });
        let depth0 = self.depth;
        let b = self.block_decls(block)?;
        self.chunk.emit(Op::PushEnv(b));
        self.depth += 1;
        for st in block { self.stmt(st)?; }
        self.chunk.emit(Op::PopEnv);
        self.depth -= 1;
        self.chunk.emit(Op::TryEnd);
        let to_end = self.chunk.emit_jump(Op::Jump);

        // Der Fangpfad. Der geworfene Wert liegt oben, wenn wir hier ankommen.
        let catch_at = self.chunk.here();
        let mut catch_guard = None;
        if let Some(h) = handler {
            self.depth = depth0;
            // **Der `catch`-Block braucht seinen EIGENEN Behandler**, wenn es
            // einen Finalisierer gibt: wirft er selbst, muss der Finalisierer
            // trotzdem laufen. Ohne diese Zeile verschwand `finally` still,
            // sobald `catch` warf — zwei test262-Faelle, und im Alltag genau
            // das Muster „aufraeumen und weiterwerfen".
            if finalizer.is_some() {
                catch_guard = Some(self.chunk.emit(
                    Op::TryStart { catch: u32::MAX, finally: u32::MAX }));
            }
            let hb = self.block_decls(&h.body)?;
            self.chunk.emit(Op::PushEnv(hb));
            self.depth += 1;
            match &h.param {
                None => { self.chunk.emit(Op::Pop); }
                Some(Pat::Ident(n)) => {
                    let i = self.chunk.name(n);
                    self.chunk.emit(Op::BindCatch(i));
                }
                Some(_) => return Err(Unsupported("catch-destructuring")),
            }
            for st in &h.body { self.stmt(st)?; }
            self.chunk.emit(Op::PopEnv);
            self.depth -= 1;
            if catch_guard.is_some() { self.chunk.emit(Op::TryEnd); }
        }
        let after_catch = self.chunk.emit_jump(Op::Jump);

        // Der Wurfpfad OHNE `catch`: Finalisierer, dann weiterwerfen.
        let rethrow_at = self.chunk.here();
        self.depth = depth0;
        if let Some(f) = finalizer {
            self.finalizer(f)?;
        }
        self.chunk.emit(Op::Rethrow);

        // Der normale Weg (und der Weg nach einem gefangenen Wurf).
        self.chunk.patch(to_end);
        self.chunk.patch(after_catch);
        self.depth = depth0;
        if let Some(f) = finalizer {
            self.finalizer(f)?;
        }
        // Erst jetzt steht fest, wohin der Behandler zeigt.
        match &mut self.chunk.ops[start] {
            Op::TryStart { catch, finally } => {
                if handler.is_some() {
                    *catch = catch_at;
                    // Ein gefangener Wurf laeuft ueber den Fangpfad, und der
                    // endet im normalen Finalisierer.
                    *finally = u32::MAX;
                } else {
                    *catch = u32::MAX;
                    *finally = rethrow_at;
                }
            }
            _ => unreachable!(),
        }
        if let Some(at) = catch_guard {
            match &mut self.chunk.ops[at] {
                Op::TryStart { finally, .. } => *finally = rethrow_at,
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn finalizer(&mut self, body: &[Stmt]) -> CompileResult<()> {
        let b = self.block_decls(body)?;
        self.chunk.emit(Op::PushEnv(b));
        self.depth += 1;
        for st in body { self.stmt(st)?; }
        self.chunk.emit(Op::PopEnv);
        self.depth -= 1;
        Ok(())
    }

    /// Springt aus diesem Rumpf etwas HERAUS? `return`, `break`, `continue`.
    /// Ein `break` innerhalb einer eigenen Schleife zaehlt nicht — es
    /// verlaesst den `try` nicht.
    fn jumps_out(body: &[Stmt]) -> bool {
        fn walk(st: &Stmt, in_loop: bool, hit: &mut bool) {
            match st {
                Stmt::Return(_) => *hit = true,
                Stmt::Break(_) | Stmt::Continue(_) if !in_loop => *hit = true,
                Stmt::Block(b) => b.iter().for_each(|s| walk(s, in_loop, hit)),
                Stmt::If { cons, alt, .. } => {
                    walk(cons, in_loop, hit);
                    if let Some(a) = alt { walk(a, in_loop, hit) }
                }
                Stmt::While { body, .. } | Stmt::DoWhile { body, .. }
                | Stmt::For { body, .. } | Stmt::ForIn { body, .. }
                | Stmt::ForOf { body, .. } => walk(body, true, hit),
                Stmt::Labeled { body, .. } => walk(body, in_loop, hit),
                Stmt::Try { block, handler, finalizer } => {
                    block.iter().for_each(|s| walk(s, in_loop, hit));
                    if let Some(h) = handler { h.body.iter().for_each(|s| walk(s, in_loop, hit)) }
                    if let Some(f) = finalizer { f.iter().for_each(|s| walk(s, in_loop, hit)) }
                }
                Stmt::Switch { cases, .. } =>
                    cases.iter().flat_map(|c| c.body.iter()).for_each(|s| walk(s, true, hit)),
                _ => {}
            }
        }
        let mut hit = false;
        body.iter().for_each(|s| walk(s, false, &mut hit));
        hit
    }

    /// `for (x of e) body`.
    ///
    /// Die Werte werden EAGER geholt (`Interp::iterate`), genau wie im
    /// Baumlaeufer. Ein Iterator, der erst beim Ziehen rechnet, braucht eine
    /// Maschine, die anhalten kann — das ist Stufe 4, und bis dahin waeren
    /// zwei verschiedene Iterationssemantiken das schlechtere Geschaeft.
    fn for_of(&mut self, left: &ForHead, right: &Expr, body: &Stmt) -> CompileResult<()> {
        let name = match left {
            ForHead::VarDecl(d) => match d.decls.first().map(|x| &x.id) {
                Some(Pat::Ident(n)) => n.clone(),
                _ => return Err(Unsupported("for-of-pattern")),
            },
            ForHead::Pattern(Pat::Ident(n)) => n.clone(),
            _ => return Err(Unsupported("for-of-target")),
        };
        let lexical = matches!(left, ForHead::VarDecl(d) if d.kind != VarKind::Var);
        self.expr(right)?;
        self.chunk.emit(Op::IterAll);
        let depth0 = self.depth;
        let top = self.chunk.here();
        let done = self.chunk.emit_jump(Op::IterNext);
        self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(), depth: depth0 });
        // Je Umlauf eine eigene Umgebung: eine Schliessung im Rumpf soll den
        // Wert DIESES Umlaufs festhalten, nicht den letzten.
        let empty = self.chunk.block(Vec::new());
        self.chunk.emit(Op::PushEnv(empty));
        self.depth += 1;
        let n = self.chunk.name(&name);
        self.chunk.emit(Op::DeclVar { name: n, mutable: true, lexical });
        self.stmt(body)?;
        self.chunk.emit(Op::PopEnv);
        self.depth -= 1;
        let l = self.loops.pop().unwrap();
        for at in l.continues { self.patch_to(at, top); }
        self.chunk.emit(Op::Jump(top));
        // Zwei Ausgaenge, und sie sind NICHT dasselbe: wer vorzeitig geht,
        // schliesst den Iterator (`return()`); wer ihn leergelesen hat, darf
        // das nicht mehr.
        for at in l.breaks { self.chunk.patch(at); }
        self.chunk.emit(Op::IterClose);
        let to_end = self.chunk.emit_jump(Op::Jump);
        self.chunk.patch(done);
        self.chunk.emit(Op::IterDrop);
        self.chunk.patch(to_end);
        Ok(())
    }

    fn var_decl(&mut self, d: &VarDecl) -> CompileResult<()> {
        for dec in &d.decls {
            let Pat::Ident(name) = &dec.id else {
                return Err(Unsupported("destructuring-decl"));
            };
            // `var x;` OHNE Initialisierer laesst eine vorhandene Bindung in
            // Ruhe — sonst loescht `var f; function f(){}` die Funktion, die
            // das Hochziehen gerade gebunden hat. Ein Test, und er hat recht.
            if d.kind == VarKind::Var && dec.init.is_none() {
                continue;
            }
            match &dec.init {
                Some(e) => self.expr(e)?,
                None => {
                    let k = self.chunk.konst(Value::Undefined);
                    self.chunk.emit(Op::Const(k));
                }
            }
            let n = self.chunk.name(name);
            // `var f = function(){}` gibt der Funktion den Namen der Variablen.
            if dec.init.is_some() {
                self.chunk.emit(Op::NameFunc(n));
            }
            self.chunk.emit(Op::DeclVar {
                name: n,
                mutable: d.kind != VarKind::Const,
                lexical: d.kind != VarKind::Var,
            });
        }
        Ok(())
    }

    // ── Ausdruecke ───────────────────────────────────────────────────────
    fn expr(&mut self, e: &Expr) -> CompileResult<()> {
        match e {
            Expr::Num(n) => {
                let k = self.chunk.konst(Value::Num(*n));
                self.chunk.emit(Op::Const(k));
                Ok(())
            }
            Expr::Str(s) => {
                let k = self.chunk.konst(Value::str(s));
                self.chunk.emit(Op::Const(k));
                Ok(())
            }
            Expr::Bool(b) => {
                let k = self.chunk.konst(Value::Bool(*b));
                self.chunk.emit(Op::Const(k));
                Ok(())
            }
            Expr::Null => {
                let k = self.chunk.konst(Value::Null);
                self.chunk.emit(Op::Const(k));
                Ok(())
            }
            Expr::This => {
                self.chunk.emit(Op::This);
                Ok(())
            }
            Expr::Ident(n) => {
                let i = self.chunk.name(n);
                self.chunk.emit(Op::LoadVar(i));
                Ok(())
            }
            // `typeof x` darf auf einem unbekannten Namen NICHT werfen — das
            // ist der Grund, warum es einen eigenen Befehl hat und nicht
            // `LoadVar` + `Un` ist.
            Expr::Unary { op: UnaryOp::Typeof, arg } => {
                if let Expr::Ident(n) = &**arg {
                    let i = self.chunk.name(n);
                    self.chunk.emit(Op::TypeofVar(i));
                } else {
                    self.expr(arg)?;
                    self.chunk.emit(Op::Un(UnaryOp::Typeof));
                }
                Ok(())
            }
            Expr::Unary { op: UnaryOp::Delete, arg } => match &**arg {
                Expr::Member { obj, prop, optional: false } => {
                    self.expr(obj)?;
                    match &**prop {
                        MemberProp::Ident(n) => {
                            let i = self.chunk.name(n);
                            self.chunk.emit(Op::DeleteProp(i));
                        }
                        MemberProp::Computed(k) => {
                            self.expr(k)?;
                            self.chunk.emit(Op::DeleteIndex);
                        }
                        MemberProp::Private(_) => return Err(Unsupported("private-field")),
                    }
                    Ok(())
                }
                // `delete x` auf allem anderen ist `true` — genau wie im
                // Baumlaeufer. Der Ausdruck wird trotzdem NICHT ausgewertet,
                // auch dort nicht.
                _ => {
                    let k = self.chunk.konst(Value::Bool(true));
                    self.chunk.emit(Op::Const(k));
                    Ok(())
                }
            },
            Expr::Unary { op, arg } => {
                self.expr(arg)?;
                self.chunk.emit(Op::Un(*op));
                Ok(())
            }
            Expr::Binary { op, left, right } => {
                self.expr(left)?;
                self.expr(right)?;
                self.chunk.emit(Op::Bin(*op));
                Ok(())
            }
            Expr::Logical { op, left, right } => {
                self.expr(left)?;
                let at = match op {
                    LogicalOp::And => self.chunk.emit_jump(Op::JumpFalseKeep),
                    LogicalOp::Or => self.chunk.emit_jump(Op::JumpTrueKeep),
                    LogicalOp::Nullish => self.chunk.emit_jump(Op::JumpNullishKeep),
                };
                // Der linke Wert war nur der Kurzschlusswert; wenn wir hier
                // sind, gilt der rechte.
                self.chunk.emit(Op::Pop);
                self.expr(right)?;
                self.chunk.patch(at);
                Ok(())
            }
            Expr::Cond { test, cons, alt } => {
                self.expr(test)?;
                let to_alt = self.chunk.emit_jump(Op::JumpFalse);
                self.expr(cons)?;
                let to_end = self.chunk.emit_jump(Op::Jump);
                self.chunk.patch(to_alt);
                self.expr(alt)?;
                self.chunk.patch(to_end);
                Ok(())
            }
            Expr::Seq(list) => {
                for (i, x) in list.iter().enumerate() {
                    self.expr(x)?;
                    if i + 1 < list.len() {
                        self.chunk.emit(Op::Pop);
                    }
                }
                Ok(())
            }
            Expr::Assign { op: AssignOp::Assign, left, right } => match &**left {
                Pat::Ident(n) => {
                    self.expr(right)?;
                    let i = self.chunk.name(n);
                    self.chunk.emit(Op::StoreVar(i));
                    Ok(())
                }
                Pat::Expr(inner) => match &**inner {
                    Expr::Member { obj, prop, optional: false } => match &**prop {
                        MemberProp::Ident(name) => {
                            self.expr(obj)?;
                            self.expr(right)?;
                            let i = self.chunk.name(name);
                            self.chunk.emit(Op::SetProp(i));
                            Ok(())
                        }
                        MemberProp::Computed(k) => {
                            self.expr(obj)?;
                            self.expr(k)?;
                            self.expr(right)?;
                            self.chunk.emit(Op::SetIndex);
                            Ok(())
                        }
                        MemberProp::Private(_) => Err(Unsupported("private-field")),
                    },
                    _ => Err(Unsupported("assign-target")),
                },
                _ => Err(Unsupported("destructuring-assign")),
            },
            Expr::Assign { op, left, right } => {
                let Pat::Expr(target) = &**left else {
                    return Err(Unsupported("destructuring-assign"));
                };
                self.compound(*op, target, right)
            }
            Expr::Member { obj, prop, optional: false } => {
                self.expr(obj)?;
                match &**prop {
                    MemberProp::Ident(name) => {
                        let i = self.chunk.name(name);
                        self.chunk.emit(Op::GetProp(i));
                        Ok(())
                    }
                    MemberProp::Computed(k) => {
                        self.expr(k)?;
                        self.chunk.emit(Op::GetIndex);
                        Ok(())
                    }
                    MemberProp::Private(_) => Err(Unsupported("private-field")),
                }
            }
            Expr::Member { optional: true, .. } | Expr::Chain(_) => Err(Unsupported("optional-chain")),
            Expr::Call { callee, args, optional: false } => {
                // Der Empfaenger gehoert zum Aufruf: `o.f()` ruft mit `o` als
                // `this`, `f()` mit undefined. Beides wird HIER entschieden,
                // damit die Maschine unten nur noch abarbeitet.
                match &**callee {
                    Expr::Member { obj, prop, optional: false } => match &**prop {
                        MemberProp::Ident(name) => {
                            self.expr(obj)?;
                            self.chunk.emit(Op::Dup);
                            let i = self.chunk.name(name);
                            self.chunk.emit(Op::GetProp(i));
                            self.chunk.emit(Op::Swap);
                        }
                        MemberProp::Computed(k) => {
                            self.expr(obj)?;
                            self.chunk.emit(Op::Dup);
                            self.expr(k)?;
                            self.chunk.emit(Op::GetIndex);
                            self.chunk.emit(Op::Swap);
                        }
                        MemberProp::Private(_) => return Err(Unsupported("private-field")),
                    },
                    _ => {
                        self.expr(callee)?;
                        let k = self.chunk.konst(Value::Undefined);
                        self.chunk.emit(Op::Const(k));
                    }
                }
                if Self::args_have_spread(args) {
                    self.args_as_array(args)?;
                    self.chunk.emit(Op::CallSpread);
                } else {
                    let n = self.plain_args(args)?;
                    self.chunk.emit(Op::Call(n));
                }
                Ok(())
            }
            Expr::Call { optional: true, .. } => Err(Unsupported("optional-call")),
            Expr::New { callee, args } => {
                self.expr(callee)?;
                if Self::args_have_spread(args) {
                    self.args_as_array(args)?;
                    self.chunk.emit(Op::NewSpread);
                } else {
                    let n = self.plain_args(args)?;
                    self.chunk.emit(Op::New(n));
                }
                Ok(())
            }
            Expr::Array(items) => {
                let mut mask = Vec::with_capacity(items.len());
                let mut n = 0u16;
                let mut any_spread = false;
                for it in items {
                    match it {
                        // Eine LUECKE ist nicht `undefined` — aber der
                        // Baumlaeufer macht daraus ebenfalls `undefined`
                        // (`Expr::Array`, `None => Value::Undefined`), und
                        // dieselbe Naeherung ist besser als eine zweite.
                        None => {
                            let k = self.chunk.konst(Value::Undefined);
                            self.chunk.emit(Op::Const(k));
                            mask.push(false);
                        }
                        Some(Expr::Spread(inner)) => {
                            self.expr(inner)?;
                            mask.push(true);
                            any_spread = true;
                        }
                        Some(x) => {
                            self.expr(x)?;
                            mask.push(false);
                        }
                    }
                    n += 1;
                }
                if any_spread {
                    let m = self.chunk.spread_mask(mask);
                    self.chunk.emit(Op::MakeArraySpread { n, spread: m });
                } else {
                    self.chunk.emit(Op::MakeArray(n));
                }
                Ok(())
            }
            Expr::Object(props) => {
                self.chunk.emit(Op::NewObject);
                for p in props {
                    match &p.value {
                        ObjPropValue::Spread(e) => {
                            self.expr(e)?;
                            self.chunk.emit(Op::SpreadInto);
                        }
                        ObjPropValue::Init(e) => {
                            let k = self.prop_key(&p.key, p.computed)?;
                            self.expr(e)?;
                            match k {
                                Some(n) => { self.chunk.emit(Op::DefineProp(n)); }
                                None => { self.chunk.emit(Op::DefinePropComputed); }
                            }
                        }
                        ObjPropValue::Method(f) => {
                            if f.is_generator || f.is_async {
                                return Err(Unsupported("generator-or-async"));
                            }
                            let k = self.prop_key(&p.key, p.computed)?;
                            let fi = self.chunk.func(f.clone());
                            self.chunk.emit(Op::Closure(fi));
                            match k {
                                Some(n) => { self.chunk.emit(Op::DefineProp(n)); }
                                None => { self.chunk.emit(Op::DefinePropComputed); }
                            }
                        }
                        ObjPropValue::Get(f) | ObjPropValue::Set(f) => {
                            if f.is_generator || f.is_async {
                                return Err(Unsupported("generator-or-async"));
                            }
                            let get = matches!(p.value, ObjPropValue::Get(_));
                            let k = self.prop_key(&p.key, p.computed)?;
                            let fi = self.chunk.func(f.clone());
                            self.chunk.emit(Op::Closure(fi));
                            match k {
                                Some(name) => { self.chunk.emit(Op::DefineAccessor { name, get }); }
                                None => { self.chunk.emit(Op::DefineAccessorComputed { get }); }
                            }
                        }
                    }
                }
                Ok(())
            }
            Expr::Template { quasis, exprs } => {
                let mut n = 0u16;
                for (idx, q) in quasis.iter().enumerate() {
                    let k = self.chunk.konst(Value::str(q.cooked.as_deref().unwrap_or("")));
                    self.chunk.emit(Op::Const(k));
                    n += 1;
                    if let Some(x) = exprs.get(idx) {
                        self.expr(x)?;
                        n += 1;
                    }
                }
                self.chunk.emit(Op::Concat(n));
                Ok(())
            }
            Expr::Regex { body, flags } => {
                let b = self.chunk.name(body);
                let f = self.chunk.name(flags);
                self.chunk.emit(Op::Regex { body: b, flags: f });
                Ok(())
            }
            Expr::Update { op, arg, prefix } => self.update(*op, arg, *prefix),
            Expr::Func(f) => {
                if f.is_generator || f.is_async {
                    return Err(Unsupported("generator-or-async"));
                }
                let i = self.chunk.func(f.clone());
                self.chunk.emit(Op::Closure(i));
                Ok(())
            }
            Expr::TaggedTemplate { .. } => Err(Unsupported("tagged-template")),
            Expr::Class(_) => Err(Unsupported("class-expr")),
            Expr::BigInt(_) => Err(Unsupported("bigint")),
            Expr::Super => Err(Unsupported("super")),

            // Ein `...x` ausserhalb von Feld und Argumentliste hat der Parser
            // schon abgelehnt; hier ist es der nackte Innenausdruck, genau wie
            // im Baumlaeufer.
            Expr::Spread(inner) => self.expr(inner),
            Expr::Yield { .. } => Err(Unsupported("yield")),
            Expr::Await(_) => Err(Unsupported("await")),
            Expr::MetaProp { .. } => Err(Unsupported("meta-prop")),
            Expr::ImportCall(_) => Err(Unsupported("import-call")),
        }
    }

    fn plain_args(&mut self, args: &[Arg]) -> CompileResult<u16> {
        let mut n = 0u16;
        for a in args {
            match a {
                Arg::Spread(_) => return Err(Unsupported("spread-arg")),
                Arg::Expr(e) => {
                    self.expr(e)?;
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// Hat diese Argumentliste ein `...x`? Dann werden ALLE Argumente in ein
    /// Feld gebaut und der Aufruf nimmt dieses — sonst muesste der Befehl eine
    /// Zahl tragen, die erst zur Laufzeit feststeht.
    fn args_have_spread(args: &[Arg]) -> bool {
        args.iter().any(|a| matches!(a, Arg::Spread(_)))
    }

    fn args_as_array(&mut self, args: &[Arg]) -> CompileResult<()> {
        let mut mask = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Arg::Expr(e) => { self.expr(e)?; mask.push(false); }
                Arg::Spread(e) => { self.expr(e)?; mask.push(true); }
            }
        }
        let n = args.len() as u16;
        let m = self.chunk.spread_mask(mask);
        self.chunk.emit(Op::MakeArraySpread { n, spread: m });
        Ok(())
    }

    /// Ein statischer Eigenschaftsname wird zum Namensindex; ein berechneter
    /// laesst seinen Schluessel auf dem Stapel und gibt `None`.
    fn prop_key(&mut self, k: &PropKey, computed: bool) -> CompileResult<Option<u32>> {
        if computed {
            let PropKey::Computed(e) = k else { return Err(Unsupported("prop-key")) };
            self.expr(e)?;
            // SOFORT umwandeln: `ToPropertyKey` darf Nebenwirkungen haben, und
            // die Spec legt fest, dass sie VOR der Auswertung des Wertes
            // passieren.
            self.chunk.emit(Op::ToKey);
            return Ok(None);
        }
        Ok(Some(match k {
            PropKey::Ident(n) => self.chunk.name(n),
            PropKey::Str(s) => self.chunk.name(s),
            PropKey::Num(n) => {
                let s = crate::js::value::num_to_string(*n);
                self.chunk.name(&s)
            }
            PropKey::Computed(e) => { self.expr(e)?; self.chunk.emit(Op::ToKey); return Ok(None) }
            PropKey::Private(_) => return Err(Unsupported("private-field")),
        }))
    }

    /// `x++` / `--o.p` — der Zielausdruck darf nur EINMAL ausgewertet werden.
    fn update(&mut self, op: UpdateOp, arg: &Expr, prefix: bool) -> CompileResult<()> {
        let d = if op == UpdateOp::Inc { 1.0 } else { -1.0 };
        match arg {
            Expr::Ident(n) => {
                let i = self.chunk.name(n);
                self.chunk.emit(Op::LoadVar(i));
                // `to_number` VOR dem Rechnen: `x = "3"; x++` gibt 4, nicht
                // "31". `Op::Un(Plus)` ist genau diese Umwandlung.
                self.chunk.emit(Op::Un(UnaryOp::Plus));
                if !prefix { self.chunk.emit(Op::Dup); }
                let k = self.chunk.konst(Value::Num(d));
                self.chunk.emit(Op::Const(k));
                self.chunk.emit(Op::Bin(BinOp::Add));
                self.chunk.emit(Op::StoreVar(i));
                if !prefix { self.chunk.emit(Op::Pop); }
                Ok(())
            }
            Expr::Member { obj, prop, optional: false } => {
                self.expr(obj)?;
                match &**prop {
                    MemberProp::Ident(name) => {
                        let i = self.chunk.name(name);
                        self.chunk.emit(Op::Dup);
                        self.chunk.emit(Op::GetProp(i));
                        self.chunk.emit(Op::Un(UnaryOp::Plus));
                        if !prefix {
                            // Den alten Wert unter das Objekt schieben: er ist
                            // das Ergebnis, das Objekt braucht der Schreiber.
                            self.chunk.emit(Op::Dup);
                            self.chunk.emit(Op::Rot3);
                        }
                        let k = self.chunk.konst(Value::Num(d));
                        self.chunk.emit(Op::Const(k));
                        self.chunk.emit(Op::Bin(BinOp::Add));
                        self.chunk.emit(Op::SetProp(i));
                        if !prefix { self.chunk.emit(Op::Pop); }
                        Ok(())
                    }
                    MemberProp::Computed(_) => Err(Unsupported("update-computed")),
                    MemberProp::Private(_) => Err(Unsupported("private-field")),
                }
            }
            _ => Err(Unsupported("update-target")),
        }
    }

    /// `a += b`, `a ||= b`, und `a = b` als Sonderfall — der linke Ausdruck
    /// wird EINMAL ausgewertet.
    fn compound(&mut self, op: AssignOp, target: &Expr, right: &Expr) -> CompileResult<()> {
        // Die kurzschliessenden Formen werten die Rechte NUR aus, wenn sie
        // gebraucht wird: `a ||= b` darf `b` nicht anfassen, wenn `a` wahr ist.
        if matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish) {
            let Expr::Ident(n) = target else { return Err(Unsupported("logical-assign-member")) };
            let i = self.chunk.name(n);
            self.chunk.emit(Op::LoadVar(i));
            let at = match op {
                AssignOp::And => self.chunk.emit_jump(Op::JumpFalseKeep),
                AssignOp::Or => self.chunk.emit_jump(Op::JumpTrueKeep),
                _ => self.chunk.emit_jump(Op::JumpNullishKeep),
            };
            self.chunk.emit(Op::Pop);
            self.expr(right)?;
            self.chunk.emit(Op::StoreVar(i));
            self.chunk.patch(at);
            return Ok(());
        }
        let bop = match op {
            AssignOp::Add => BinOp::Add, AssignOp::Sub => BinOp::Sub,
            AssignOp::Mul => BinOp::Mul, AssignOp::Div => BinOp::Div,
            AssignOp::Mod => BinOp::Mod, AssignOp::Exp => BinOp::Exp,
            AssignOp::Shl => BinOp::Shl, AssignOp::Shr => BinOp::Shr,
            AssignOp::UShr => BinOp::UShr, AssignOp::BitAnd => BinOp::BitAnd,
            AssignOp::BitOr => BinOp::BitOr, AssignOp::BitXor => BinOp::BitXor,
            AssignOp::Assign => return Err(Unsupported("plain-assign-here")),
            _ => return Err(Unsupported("assign-op")),
        };
        match target {
            Expr::Ident(n) => {
                let i = self.chunk.name(n);
                self.chunk.emit(Op::LoadVar(i));
                self.expr(right)?;
                self.chunk.emit(Op::Bin(bop));
                self.chunk.emit(Op::StoreVar(i));
                Ok(())
            }
            Expr::Member { obj, prop, optional: false } => match &**prop {
                MemberProp::Ident(name) => {
                    let i = self.chunk.name(name);
                    self.expr(obj)?;
                    self.chunk.emit(Op::Dup);
                    self.chunk.emit(Op::GetProp(i));
                    self.expr(right)?;
                    self.chunk.emit(Op::Bin(bop));
                    self.chunk.emit(Op::SetProp(i));
                    Ok(())
                }
                _ => Err(Unsupported("compound-computed")),
            },
            _ => Err(Unsupported("compound-target")),
        }
    }

    /// Alle Umgebungen schliessen, die zwischen HIER und `depth` offen sind.
    /// Ein Sprung aus einem Block heraus laesst sie sonst stehen.
    fn unwind_to(&mut self, depth: usize) {
        for _ in depth..self.depth {
            self.chunk.emit(Op::PopEnv);
        }
    }

    fn patch_to(&mut self, at: usize, target: u32) {
        match &mut self.chunk.ops[at] {
            Op::Jump(t) => *t = target,
            other => panic!("patch_to auf {other:?}"),
        }
    }

    /// Faengt in diesem Rumpf eine Funktion etwas ein?
    ///
    /// Nur dafuer da, `for (let i …)` abzulehnen, wenn es darauf ankommt: die
    /// Spec gibt jedem Umlauf eine FRISCHE Bindung, und wer das nicht baut,
    /// liefert einer Schliessung den Endwert. Ohne Schliessung im Rumpf ist der
    /// Unterschied nicht beobachtbar — dann darf die Maschine mitfahren.
    fn captures(&self, body: &Stmt) -> bool {
        let mut found = false;
        walk_stmt(body, &mut |e| {
            if matches!(e, Expr::Func(_) | Expr::Class(_)) {
                found = true;
            }
        });
        found
    }
}

/// Jeden Ausdruck eines Statements besuchen. Bewusst grob: der einzige Rufer
/// fragt „kommt hier IRGENDWO eine Funktion vor", und dafuer reicht es.
fn walk_stmt(st: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match st {
        Stmt::Expr(x) | Stmt::Throw(x) => walk_expr(x, f),
        Stmt::Return(Some(x)) => walk_expr(x, f),
        Stmt::Block(b) => b.iter().for_each(|s| walk_stmt(s, f)),
        Stmt::If { test, cons, alt } => {
            walk_expr(test, f);
            walk_stmt(cons, f);
            if let Some(a) = alt { walk_stmt(a, f) }
        }
        Stmt::While { test, body } => { walk_expr(test, f); walk_stmt(body, f) }
        Stmt::DoWhile { body, test } => { walk_stmt(body, f); walk_expr(test, f) }
        Stmt::For { init, test, update, body } => {
            if let Some(i) = init {
                match &**i {
                    ForInit::Expr(x) => walk_expr(x, f),
                    ForInit::VarDecl(d) => {
                        for x in d.decls.iter().filter_map(|x| x.init.as_ref()) { walk_expr(x, f) }
                    }
                }
            }
            if let Some(t) = test { walk_expr(t, f) }
            if let Some(u) = update { walk_expr(u, f) }
            walk_stmt(body, f);
        }
        Stmt::VarDecl(d) => {
            for x in d.decls.iter().filter_map(|x| x.init.as_ref()) { walk_expr(x, f) }
        }
        Stmt::Labeled { body, .. } => walk_stmt(body, f),
        // Alles Uebrige lehnt der Uebersetzer ohnehin ab; ein `true` waere
        // hier nur vorsichtiger, nicht richtiger.
        _ => {}
    }
}

fn walk_expr(x: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(x);
    match x {
        Expr::Unary { arg, .. } | Expr::Update { arg, .. } | Expr::Spread(arg)
        | Expr::Chain(arg) | Expr::Await(arg) => walk_expr(arg, f),
        Expr::Binary { left, right, .. } | Expr::Logical { left, right, .. } => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        Expr::Assign { right, .. } => walk_expr(right, f),
        Expr::Cond { test, cons, alt } => {
            walk_expr(test, f);
            walk_expr(cons, f);
            walk_expr(alt, f);
        }
        Expr::Call { callee, args, .. } | Expr::New { callee, args } => {
            walk_expr(callee, f);
            for a in args {
                match a {
                    Arg::Expr(e) | Arg::Spread(e) => walk_expr(e, f),
                }
            }
        }
        Expr::Member { obj, prop, .. } => {
            walk_expr(obj, f);
            if let MemberProp::Computed(k) = &**prop { walk_expr(k, f) }
        }
        Expr::Seq(list) => list.iter().for_each(|e| walk_expr(e, f)),
        Expr::Array(items) => items.iter().flatten().for_each(|e| walk_expr(e, f)),
        _ => {}
    }
}

/// Die Namen aller Absagen dieses Laufs — der Uebersetzer zaehlt sie nicht
/// selbst, der Rufer tut es (siehe `Interp::run_program`).
pub fn unsupported_name(u: &Unsupported) -> &'static str {
    u.0
}

/// Nur damit `Vec<Rc<Func>>` im `Chunk` nicht als toter Code gilt, solange die
/// Maschine Aufrufe noch ueber `Interp::call` faehrt.
pub fn funcs_of(c: &Chunk) -> &[Rc<Func>] {
    &c.funcs
}
