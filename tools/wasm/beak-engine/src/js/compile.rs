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
            Stmt::Try { .. } => Err(Unsupported("try")),
            Stmt::ForIn { .. } => Err(Unsupported("for-in")),
            Stmt::ForOf { .. } => Err(Unsupported("for-of")),
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
            self.chunk.emit(Op::DeclVar { name: n, mutable: d.kind != VarKind::Const });
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
            Expr::Unary { op: UnaryOp::Delete, .. } => Err(Unsupported("delete")),
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
            Expr::Assign { .. } => Err(Unsupported("compound-assign")),
            Expr::Update { .. } => Err(Unsupported("update")),
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
                let n = self.plain_args(args)?;
                self.chunk.emit(Op::Call(n));
                Ok(())
            }
            Expr::Call { optional: true, .. } => Err(Unsupported("optional-call")),
            Expr::New { callee, args } => {
                self.expr(callee)?;
                let n = self.plain_args(args)?;
                self.chunk.emit(Op::New(n));
                Ok(())
            }
            Expr::Array(items) => {
                let mut n = 0u16;
                for it in items {
                    match it {
                        None => return Err(Unsupported("array-hole")),
                        Some(Expr::Spread(_)) => return Err(Unsupported("spread")),
                        Some(x) => {
                            self.expr(x)?;
                            n += 1;
                        }
                    }
                }
                self.chunk.emit(Op::MakeArray(n));
                Ok(())
            }
            Expr::Func(f) => {
                if f.is_generator || f.is_async {
                    return Err(Unsupported("generator-or-async"));
                }
                let i = self.chunk.func(f.clone());
                self.chunk.emit(Op::Closure(i));
                Ok(())
            }
            Expr::Object(_) => Err(Unsupported("object-literal")),
            Expr::Template { .. } | Expr::TaggedTemplate { .. } => Err(Unsupported("template")),
            Expr::Regex { .. } => Err(Unsupported("regex-literal")),
            Expr::Class(_) => Err(Unsupported("class-expr")),
            Expr::BigInt(_) => Err(Unsupported("bigint")),
            Expr::Super => Err(Unsupported("super")),
            Expr::Spread(_) => Err(Unsupported("spread")),
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
