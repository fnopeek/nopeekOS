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

use alloc::string::String;
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
    /// Der Name, unter dem `break lbl` / `continue lbl` diesen Ausgang
    /// findet. `None` heisst: nur ueber das unbenannte `break` erreichbar.
    labels: Vec<String>,
    /// Wieviele `for…of`/`for…in` beim Betreten offen waren.
    ///
    /// Dieselbe Buchhaltung wie `depth` fuer die Umgebungen, und aus demselben
    /// Grund: ein `break lbl`/`continue lbl` springt an den `IterClose` der
    /// INNEREN Schleife vorbei. Dann bleibt deren Iterator im Rahmen liegen —
    /// ungeschlossen, und der naechste `IterNext` der aeusseren Schleife
    /// findet den falschen. Ein test262-Fall hat genau das gesagt.
    iters: usize,
    /// Ein `switch` ist BRECHBAR, aber nicht fortsetzbar: `break` gehoert ihm,
    /// `continue` der Schleife darunter. Ohne diese Unterscheidung liefe ein
    /// `continue` in einem `switch` innerhalb einer Schleife an den Anfang des
    /// `switch` — und das ist eine Endlosschleife, kein Fehler, den man sieht.
    brk_only: bool,
}

pub struct Compiler {
    pub chunk: Chunk,
    loops: Vec<Loop>,
    /// Wieviele `PushEnv` gerade offen sind.
    depth: usize,
    /// Wieviele Schleifeniteratoren gerade offen sind.
    iters: usize,
    /// Uebersetzen wir gerade den Rumpf eines GENERATORS? Nur dort ist ein
    /// `yield` ein `Op::Yield`; in einem Pfeil INNERHALB eines Generators
    /// liest unser Parser `yield` ebenfalls als Yield-Ausdruck, und dessen
    /// Rumpf ist ein eigener Chunk mit `in_gen == false` — der sagt hier nein
    /// und faellt auf den Baumlaeufer zurueck, statt einen `Op::Yield` in
    /// einen Rahmen zu legen, der ihn nicht anhalten kann.
    in_gen: bool,
    /// Uebersetzen wir gerade den Rumpf einer ASYNC-Funktion? Dieselbe
    /// Ueberlegung wie bei `in_gen`: ein `await` im Rumpf eines gewoehnlichen
    /// Pfeils darin ist ein eigener Chunk, der hier nein sagt.
    in_async: bool,
    /// Offene Ausgaenge einer Optional-Kette (`a?.b.c`).
    ///
    /// Der Kurzschluss gehoert der GANZEN Kette, nicht dem einen Glied:
    /// `a?.b.c` gibt `undefined`, wenn `a` fehlt — es fasst `.c` gar nicht
    /// erst an. `Expr::Chain` macht die Klammer auf, jedes `?.` darin traegt
    /// hier seinen Sprung ein, und beim Zumachen zeigen alle auf dasselbe
    /// Ende. Jeder Sprung raeumt VORHER seinen eigenen Stapel ab, damit das
    /// Ende nicht wissen muss, wieviel darunter lag.
    chains: Vec<Vec<usize>>,
    /// Der Name, den die naechste Schleife bekommt.
    ///
    /// `outer: for (…)` ist im Baum eine Marke UM eine Schleife, in der
    /// Maschine aber gehoert der Name der SCHLEIFE — nur sie weiss, wohin ein
    /// `continue outer` springt. Also legt die Marke ihn hier ab und die
    /// Schleife nimmt ihn beim Anlegen mit.
    pending_labels: Vec<String>,
    /// Wieviele `finally` gerade anhaengig sind.
    ///
    /// Ein `yield` darunter ist ABGELEHNT, und zwar aus demselben Grund wie
    /// `return` darunter: `gen.return()` an so einer Stelle muss den
    /// Finalisierer noch fahren, und der Finalisierer wird hier KOPIERT statt
    /// angesprungen — es gibt keine Stelle, an die ein zwischengespeicherter
    /// Abschluss zurueckkaeme. Halb gebaut waere schlimmer als abgelehnt; im
    /// Korpus kostet es 60 Dateien.
    fin: usize,
}

/// Einen FUNKTIONSRUMPF uebersetzen.
///
/// Unterschied zum Programm: kein Abschlusswert (eine Funktion ohne `return`
/// gibt `undefined`), und am Ende steht ein `Ret`, das genau das tut.
/// Parameter und `this` liegen schon in der Umgebung, die `Interp::call_env`
/// gebaut hat — der Rumpf faengt beim ersten Statement an.
pub fn function(f: &Func) -> CompileResult<Chunk> {
    // Ein async-Generator ist BEIDES auf einmal: er haelt an `yield` UND an
    // `await` an, und `next()` gibt ein Versprechen zurueck. Das ist eine
    // eigene Runde, nicht die Summe der beiden — deshalb hier nein.
    if f.is_async && f.is_generator {
        return Err(Unsupported("async-generator"));
    }
    let mut c = Compiler { chunk: Chunk::new(), loops: Vec::new(), depth: 0, iters: 0,
                           in_gen: f.is_generator, in_async: f.is_async, fin: 0, chains: Vec::new(), pending_labels: Vec::new() };
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
    let mut c = Compiler { chunk: Chunk::new(), loops: Vec::new(), depth: 0, iters: 0,
                           in_gen: false, in_async: false, fin: 0, chains: Vec::new(), pending_labels: Vec::new() };
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
                let lbl = self.take_label();
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                                       depth: self.depth, brk_only: false, labels: lbl, iters: self.iters });
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
                let lbl = self.take_label();
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                                       depth: self.depth, brk_only: false, labels: lbl, iters: self.iters });
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
                let lbl = self.take_label();
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                                       depth: self.depth, brk_only: false, labels: lbl, iters: self.iters });
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
                let it = self.loops.last().unwrap().iters;
                self.unwind_iters(it);
                self.unwind_to(d);
                let at = self.chunk.emit_jump(Op::Jump);
                self.loops.last_mut().unwrap().breaks.push(at);
                Ok(())
            }
            Stmt::Continue(None) => {
                // Ein `switch` faengt kein `continue` — das gehoert der
                // Schleife darunter, und der Weg dorthin fuehrt durch die
                // Umgebung des `switch` hindurch.
                let Some(k) = self.loops.iter().rposition(|l| !l.brk_only) else {
                    return Err(Unsupported("continue-outside-loop"));
                };
                self.unwind_iters(self.loops[k].iters);
                self.unwind_to(self.loops[k].depth);
                let at = self.chunk.emit_jump(Op::Jump);
                self.loops[k].continues.push(at);
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
            // Eine Marke vor einer SCHLEIFE gehoert der Schleife (nur sie hat
            // einen Fortsetzungspunkt); vor allem anderen ist sie selbst ein
            // Ausgang, den nur ein `break lbl` trifft.
            Stmt::Labeled { label, body } => {
                // **Eine KETTE von Marken gehoert ganz der Schleife darunter.**
                // `a: b: for (…)` traegt beide, und `continue a` ist gueltig.
                // Der erste Entwurf gab nur die innerste weiter und lehnte
                // `continue a` ab — node sagte prompt etwas anderes.
                let mut labels = alloc::vec![label.clone()];
                let mut inner: &Stmt = body;
                while let Stmt::Labeled { label: l2, body: b2 } = inner {
                    labels.push(l2.clone());
                    inner = b2;
                }
                if matches!(inner, Stmt::While { .. } | Stmt::DoWhile { .. }
                            | Stmt::For { .. } | Stmt::ForIn { .. } | Stmt::ForOf { .. }) {
                    self.pending_labels = labels;
                    let r = self.stmt(inner);
                    self.pending_labels.clear();
                    return r;
                }
                // Sonst ist die Marke selbst der Ausgang — nur ein `break lbl`
                // trifft sie, ein `continue` braucht einen Fortsetzungspunkt.
                self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                                       depth: self.depth, brk_only: true, labels, iters: self.iters });
                let r = self.stmt(inner);
                let l = self.loops.pop().unwrap();
                r?;
                for at in l.breaks { self.chunk.patch(at); }
                Ok(())
            }
            Stmt::Break(Some(l)) => {
                let Some(k) = self.loops.iter().rposition(|x| x.labels.iter().any(|n| n == l))
                else { return Err(Unsupported("break-unknown-label")) };
                self.unwind_iters(self.loops[k].iters);
                self.unwind_to(self.loops[k].depth);
                let at = self.chunk.emit_jump(Op::Jump);
                self.loops[k].breaks.push(at);
                Ok(())
            }
            Stmt::Continue(Some(l)) => {
                // Ein `continue` braucht einen Fortsetzungspunkt, den hat nur
                // eine Schleife — eine Marke vor einem Block ist keiner.
                let Some(k) = self.loops.iter().rposition(
                    |x| !x.brk_only && x.labels.iter().any(|n| n == l))
                else { return Err(Unsupported("continue-unknown-label")) };
                self.unwind_iters(self.loops[k].iters);
                self.unwind_to(self.loops[k].depth);
                let at = self.chunk.emit_jump(Op::Jump);
                self.loops[k].continues.push(at);
                Ok(())
            }
            Stmt::Switch { disc, cases } => self.switch(disc, cases),
            Stmt::Try { block, handler, finalizer } => self.try_stmt(block, handler, finalizer),
            Stmt::ForIn { left, right, body } => self.for_in(left, right, body),
            Stmt::ForOf { left, right, body, is_await } => {
                if *is_await { return Err(Unsupported("for-await")) }
                self.for_of(left, right, body)
            }
            Stmt::With { .. } => Err(Unsupported("with")),
            // Eine Klassen-DEKLARATION: bauen und an ihren Namen binden. Die
            // Bindung selbst hat das Hochziehen schon angelegt (auf „nicht
            // bereit", die zeitliche Totzone) — hier wird sie fertig.
            Stmt::Class(c) => {
                let k = self.chunk.class(c.clone());
                self.chunk.emit(Op::Class(k));
                match &c.name {
                    Some(n) => {
                        let name = self.chunk.name(n);
                        self.chunk.emit(Op::DeclVar { name, mutable: true, lexical: true });
                    }
                    None => { self.chunk.emit(Op::Pop); }
                }
                Ok(())
            }
            Stmt::Import(_) | Stmt::ExportNamed { .. } | Stmt::ExportDefault(_)
            | Stmt::ExportAll { .. } => Err(Unsupported("module")),
        }
    }

    /// Was ein Block bindet, BEVOR seine erste Zeile laeuft — dieselben zwei
    /// Faelle und dieselbe Reihenfolge wie `Interp::hoist`. `var` steht nicht
    /// dabei: das steigt bis zur Funktionsgrenze und ist beim Programmstart
    /// schon erledigt.
    fn block_decls(&mut self, body: &[Stmt]) -> CompileResult<u32> {
        self.block_decls_of(body.iter())
    }

    /// Dasselbe ueber eine beliebige Folge — ein `switch` zieht ueber ALLE
    /// Faelle zusammen hoch, so wie es der Baumlaeufer tut: sie teilen sich
    /// EINE Umgebung, und eine Funktionsdeklaration im dritten Fall ist im
    /// ersten schon sichtbar.
    fn block_decls_of<'a>(&mut self, body: impl Iterator<Item = &'a Stmt>) -> CompileResult<u32> {
        let mut out = Vec::new();
        for st in body {
            match st {
                Stmt::Func(f) => {
                    if f.is_async && f.is_generator {
                        return Err(Unsupported("async-generator"));
                    }
                    if let Some(n) = &f.name {
                        let name = self.chunk.name(n);
                        let func = self.chunk.func(f.clone());
                        out.push(BlockDecl::Func { name, func });
                    }
                }
                Stmt::VarDecl(d) if d.kind != VarKind::Var => {
                    // Auch ein Muster steht mit ALLEN seinen Namen in der
                    // Totzone — dieselbe Liste wie `Interp::hoist`, dieselbe
                    // Funktion (`names_of`).
                    let mut names = Vec::new();
                    for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                    for n in names {
                        let name = self.chunk.name(&n);
                        out.push(BlockDecl::Tdz { name, mutable: d.kind != VarKind::Const });
                    }
                }
                // Eine Klasse steht wie ein `let` in der Totzone — dieselben
                // zwei Schleifen wie `Interp::hoist`.
                Stmt::Class(c) => {
                    if let Some(n) = &c.name {
                        let name = self.chunk.name(n);
                        out.push(BlockDecl::Tdz { name, mutable: true });
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
        // Ein `yield` unter einem anhaengigen Finalisierer ist derselbe Fall
        // wie ein `return` darunter — siehe `Compiler::fin`.
        if finalizer.is_some() { self.fin += 1; }
        let r = self.try_inner(block, handler, finalizer);
        if finalizer.is_some() { self.fin -= 1; }
        r
    }

    fn try_inner(&mut self, block: &[Stmt], handler: &Option<CatchClause>,
                 finalizer: &Option<Vec<Stmt>>) -> CompileResult<()> {
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
                Some(p) => {
                    let k = self.chunk.pat(p.clone());
                    self.chunk.emit(Op::BindPat { pat: k, mode: BindMode::Declare });
                }
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
        self.expr(right)?;
        self.chunk.emit(Op::IterAll);
        self.iters += 1;
        let depth0 = self.depth;
        let top = self.chunk.here();
        let done = self.chunk.emit_jump(Op::IterNext);
        let lbl = self.take_label();
        self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                               depth: depth0, brk_only: false, labels: lbl, iters: self.iters });
        // Je Umlauf eine eigene Umgebung: eine Schliessung im Rumpf soll den
        // Wert DIESES Umlaufs festhalten, nicht den letzten.
        let empty = self.chunk.block(Vec::new());
        self.chunk.emit(Op::PushEnv(empty));
        self.depth += 1;
        let h = self.chunk.head(left.clone());
        self.chunk.emit(Op::BindHead(h));
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
        self.iters -= 1;
        Ok(())
    }

    /// `for (k in obj)`.
    ///
    /// Dieselbe Form wie `for_of` — nur ist die Schluesselliste EIFRIG
    /// (`Interp::for_in_keys`, dieselbe Hilfe wie im Baumlaeufer), und es gibt
    /// nichts zu schliessen: eine fertige Liste hat kein `return()`. Deshalb
    /// nehmen beide Ausgaenge denselben `IterDrop`.
    fn for_in(&mut self, left: &ForHead, right: &Expr, body: &Stmt) -> CompileResult<()> {
        self.expr(right)?;
        self.chunk.emit(Op::ForInAll);
        self.iters += 1;
        let depth0 = self.depth;
        let top = self.chunk.here();
        let done = self.chunk.emit_jump(Op::ForInNext);
        let lbl = self.take_label();
        self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                               depth: depth0, brk_only: false, labels: lbl, iters: self.iters });
        // Je Umlauf eine eigene Umgebung — wie bei `for…of`, und aus
        // demselben Grund: eine Schliessung im Rumpf haelt den Schluessel
        // DIESES Umlaufs fest.
        let empty = self.chunk.block(Vec::new());
        self.chunk.emit(Op::PushEnv(empty));
        self.depth += 1;
        let h = self.chunk.head(left.clone());
        self.chunk.emit(Op::BindHead(h));
        self.stmt(body)?;
        self.chunk.emit(Op::PopEnv);
        self.depth -= 1;
        let l = self.loops.pop().unwrap();
        for at in l.continues { self.patch_to(at, top); }
        self.chunk.emit(Op::Jump(top));
        for at in l.breaks { self.chunk.patch(at); }
        self.chunk.patch(done);
        self.chunk.emit(Op::IterDrop);
        self.iters -= 1;
        Ok(())
    }

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
    fn switch(&mut self, disc: &Expr, cases: &[SwitchCase]) -> CompileResult<()> {
        self.expr(disc)?;
        let b = self.block_decls_of(cases.iter().flat_map(|c| c.body.iter()))?;
        self.chunk.emit(Op::PushEnv(b));
        self.depth += 1;
        let lbl = self.take_label();
        self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                               depth: self.depth, brk_only: true, labels: lbl, iters: self.iters });

        // Die Bedingungskette. Jeder Treffer springt in seine Weiche.
        let mut hits = Vec::new();
        for (k, c) in cases.iter().enumerate() {
            let Some(t) = &c.test else { continue };
            self.chunk.emit(Op::Dup);
            self.expr(t)?;
            self.chunk.emit(Op::Bin(BinOp::EqEqEq));
            hits.push((self.chunk.emit_jump(Op::JumpTrue), k));
        }
        // Keine passte: den Wert weg und zu `default` (oder ans Ende).
        self.chunk.emit(Op::Pop);
        let to_default = self.chunk.emit_jump(Op::Jump);

        // Die Weichen: Wert herunternehmen, dann in den Rumpf.
        let mut gates = Vec::new();
        for (at, k) in hits {
            self.chunk.patch(at);
            self.chunk.emit(Op::Pop);
            gates.push((self.chunk.emit_jump(Op::Jump), k));
        }

        // Die Rumpfe, hintereinander — das Durchfallen ergibt sich von selbst.
        let mut starts: Vec<u32> = Vec::new();
        for c in cases {
            starts.push(self.chunk.here());
            for st in &c.body { self.stmt(st)?; }
        }
        let end = self.chunk.here();
        for (at, k) in gates {
            self.patch_to(at, starts[k]);
        }
        let dflt = cases.iter().position(|c| c.test.is_none());
        self.patch_to(to_default, match dflt {
            Some(k) => starts[k],
            None => end,
        });

        let l = self.loops.pop().unwrap();
        for at in l.breaks { self.chunk.patch(at); }
        self.chunk.emit(Op::PopEnv);
        self.depth -= 1;
        Ok(())
    }

    fn var_decl(&mut self, d: &VarDecl) -> CompileResult<()> {
        for dec in &d.decls {
            let Pat::Ident(name) = &dec.id else {
                // Ein MUSTER. Ohne Initialisierer gibt es das nicht (der
                // Parser laesst `var {a};` nicht durch), also steht der Wert
                // hier immer. Die Bindungen hat das Hochziehen schon angelegt.
                let Some(e) = &dec.init else {
                    return Err(Unsupported("destructuring-no-init"));
                };
                self.expr(e)?;
                let p = self.chunk.pat(dec.id.clone());
                self.chunk.emit(Op::BindPat { pat: p, mode: BindMode::Init });
                continue;
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
                        MemberProp::Ident(_) | MemberProp::Private(_) => {
                            let i = self.member_name(prop);
                            self.chunk.emit(Op::DeleteProp(i));
                        }
                        MemberProp::Computed(k) => {
                            self.expr(k)?;
                            self.chunk.emit(Op::DeleteIndex);
                        }
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
                // `#x in obj`: die linke Seite ist ein NAME, kein Wert.
                if *op == BinOp::In {
                    if let Expr::Ident(n) = &**left {
                        if let Some(name) = n.strip_prefix('#') {
                            self.expr(right)?;
                            let k = self.chunk.name(name);
                            self.chunk.emit(Op::PrivateIn(k));
                            return Ok(());
                        }
                    }
                }
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
                    // `q = function(){}` gibt der Funktion den Namen der
                    // Variablen — dieselbe Regel wie bei `var q = …`.
                    self.chunk.emit(Op::NameFunc(i));
                    self.chunk.emit(Op::StoreVar(i));
                    Ok(())
                }
                Pat::Expr(inner) => match &**inner {
                    Expr::Member { obj, prop, optional: false } => match &**prop {
                        MemberProp::Ident(_) | MemberProp::Private(_) => {
                            let i = self.member_name(prop);
                            self.expr(obj)?;
                            self.expr(right)?;
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
                    },
                    _ => Err(Unsupported("assign-target")),
                },
                // Ein Muster als Ziel: `[a,b] = x`, `({a} = x)`. Der WERT der
                // Zuweisung ist die rechte Seite, nicht das Gebundene —
                // deshalb bleibt eine Kopie liegen.
                p => {
                    self.expr(right)?;
                    self.chunk.emit(Op::Dup);
                    let k = self.chunk.pat(p.clone());
                    self.chunk.emit(Op::BindPat { pat: k, mode: BindMode::Assign });
                    Ok(())
                }
            },
            Expr::Assign { op, left, right } => {
                let Pat::Expr(target) = &**left else {
                    return Err(Unsupported("destructuring-assign"));
                };
                self.compound(*op, target, right)
            }
            Expr::Member { obj, prop, optional: false } if matches!(**obj, Expr::Super) => {
                match &**prop {
                    MemberProp::Ident(n) => {
                        let i = self.chunk.name(n);
                        self.chunk.emit(Op::SuperGet(i));
                        Ok(())
                    }
                    _ => Err(Unsupported("super-computed")),
                }
            }
            Expr::Member { obj, prop, optional: false } => {
                self.expr(obj)?;
                match &**prop {
                    MemberProp::Ident(_) | MemberProp::Private(_) => {
                        let i = self.member_name(prop);
                        self.chunk.emit(Op::GetProp(i));
                        Ok(())
                    }
                    MemberProp::Computed(k) => {
                        self.expr(k)?;
                        self.chunk.emit(Op::GetIndex);
                        Ok(())
                    }
                }
            }
            // Die Klammer um eine Optional-Kette: alle Kurzschluesse darin
            // enden HIER, nicht am einzelnen Glied.
            Expr::Chain(inner) => {
                self.chains.push(Vec::new());
                let r = self.expr(inner);
                let exits = self.chains.pop().unwrap();
                r?;
                for at in exits { self.chunk.patch(at); }
                Ok(())
            }
            Expr::Member { obj, prop, optional: true } => {
                if self.chains.is_empty() { return Err(Unsupported("optional-outside-chain")) }
                self.expr(obj)?;
                self.short_circuit(1)?;
                match &**prop {
                    MemberProp::Ident(_) | MemberProp::Private(_) => {
                        let i = self.member_name(prop);
                        self.chunk.emit(Op::GetProp(i));
                    }
                    MemberProp::Computed(k) => {
                        self.expr(k)?;
                        self.chunk.emit(Op::GetIndex);
                    }
                }
                Ok(())
            }
            Expr::Call { callee, args, optional: false } if matches!(**callee, Expr::Super) => {
                if Self::args_have_spread(args) { return Err(Unsupported("super-spread")) }
                let n = self.plain_args(args)?;
                self.chunk.emit(Op::SuperCall(n));
                Ok(())
            }
            Expr::Call { callee, args, optional: false }
                if matches!(&**callee, Expr::Member { obj, optional: false, .. }
                            if matches!(**obj, Expr::Super)) => {
                let Expr::Member { prop, .. } = &**callee else { unreachable!() };
                let MemberProp::Ident(n) = &**prop else {
                    return Err(Unsupported("super-computed"));
                };
                let i = self.chunk.name(n);
                self.chunk.emit(Op::SuperCallee(i));
                if Self::args_have_spread(args) {
                    self.args_as_array(args)?;
                    self.chunk.emit(Op::CallSpread(i));
                } else {
                    let a = self.plain_args(args)?;
                    self.chunk.emit(Op::Call { argc: a, name: i });
                }
                Ok(())
            }
            // `a?.b(…)` und `a?.b?.(…)`: der Empfaenger ist `a`, und er darf
            // NICHT verlorengehen.
            //
            // Ohne diesen Zweig fiel BEIDES in den Fall „irgendein Ausdruck
            // als Gerufener" und rief mit `undefined` als `this`.
            // `o?.m?.forEach(f)` warf dann „Map method on the wrong
            // receiver" — und zwar nur auf der Befehlsmaschine, der
            // Baumlaeufer war die ganze Zeit richtig. test262 hat es nicht
            // gesehen; gefunden hat es die Fritzbox-Oberflaeche.
            Expr::Call { callee, args, optional }
                if matches!(&**callee, Expr::Member { optional: true, .. }) => {
                if self.chains.is_empty() { return Err(Unsupported("optional-outside-chain")) }
                let Expr::Member { obj, prop, .. } = &**callee else { unreachable!() };
                self.expr(obj)?;
                // Erst `a` pruefen — das ist das `?.` VOR dem Namen.
                self.short_circuit(1)?;
                self.chunk.emit(Op::Dup);
                let mut named = u32::MAX;
                match &**prop {
                    MemberProp::Computed(k) => {
                        self.expr(k)?;
                        self.chunk.emit(Op::GetIndex);
                    }
                    _ => {
                        named = self.member_name(prop);
                        self.chunk.emit(Op::GetProp(named));
                    }
                }
                // Und dann das `?.` VOR der Klammer, wenn eins dasteht: hier
                // liegen Empfaenger UND Gerufener, der Kurzschluss raeumt
                // beide ab.
                if *optional { self.short_circuit(2)?; named = u32::MAX; }
                self.chunk.emit(Op::Swap);
                if Self::args_have_spread(args) {
                    self.args_as_array(args)?;
                    self.chunk.emit(Op::CallSpread(named));
                } else {
                    let n = self.plain_args(args)?;
                    self.chunk.emit(Op::Call { argc: n, name: named });
                }
                Ok(())
            }
            Expr::Call { callee, args, optional: false } => {
                // Der Empfaenger gehoert zum Aufruf: `o.f()` ruft mit `o` als
                // `this`, `f()` mit undefined. Beides wird HIER entschieden,
                // damit die Maschine unten nur noch abarbeitet.
                // Der Name des Gerufenen wird MITGEGEBEN — nicht fuer den
                // Aufruf, sondern fuer den Fehlschlag: „o is not a function"
                // sagt, was fehlt, „value is not a function" nicht.
                let mut named = u32::MAX;
                match &**callee {
                    Expr::Member { obj, prop, optional: false } => match &**prop {
                        MemberProp::Ident(_) | MemberProp::Private(_) => {
                            let i = self.member_name(prop);
                            named = i;
                            self.expr(obj)?;
                            self.chunk.emit(Op::Dup);
                            self.chunk.emit(Op::GetProp(i));
                            self.chunk.emit(Op::Swap);
                        }
                        MemberProp::Computed(k) => {
                            // Ein LITERALER Schluessel ist zur Uebersetzungszeit
                            // bekannt — `o[8362]()` kann seine 8362 nennen, und
                            // genau so sieht ein minifiziertes Modulregister
                            // aus. Ein wirklich berechneter Schluessel bleibt
                            // namenlos; ihn mitzufuehren kostete zwei Befehle
                            // an jedem Aufruf, und das ist der falsche Handel.
                            match k {
                                Expr::Num(n) => {
                                    let t = super::value::num_to_string(*n);
                                    named = self.chunk.name(&t);
                                }
                                Expr::Str(t) => { named = self.chunk.name(t.as_str()); }
                                _ => {}
                            }
                            self.expr(obj)?;
                            self.chunk.emit(Op::Dup);
                            self.expr(k)?;
                            self.chunk.emit(Op::GetIndex);
                            self.chunk.emit(Op::Swap);
                        }
                    },
                    _ => {
                        if let Expr::Ident(n) = &**callee { named = self.chunk.name(n); }
                        self.expr(callee)?;
                        let k = self.chunk.konst(Value::Undefined);
                        self.chunk.emit(Op::Const(k));
                    }
                }
                if Self::args_have_spread(args) {
                    self.args_as_array(args)?;
                    self.chunk.emit(Op::CallSpread(named));
                } else {
                    let n = self.plain_args(args)?;
                    self.chunk.emit(Op::Call { argc: n, name: named });
                }
                Ok(())
            }
            // `f?.()` — hier liegen callee UND Empfaenger, also raeumt der
            // Kurzschluss zwei Werte ab.
            Expr::Call { callee, args, optional: true } => {
                if self.chains.is_empty() { return Err(Unsupported("optional-outside-chain")) }
                match &**callee {
                    Expr::Member { obj, prop, optional: false } => {
                        let i = self.member_name(prop);
                        self.expr(obj)?;
                        self.chunk.emit(Op::Dup);
                        match &**prop {
                            MemberProp::Computed(k) => {
                                self.expr(k)?;
                                self.chunk.emit(Op::GetIndex);
                            }
                            _ => { self.chunk.emit(Op::GetProp(i)); }
                        }
                        self.short_circuit(2)?;
                        self.chunk.emit(Op::Swap);
                    }
                    _ => {
                        self.expr(callee)?;
                        self.short_circuit(1)?;
                        let k = self.chunk.konst(Value::Undefined);
                        self.chunk.emit(Op::Const(k));
                    }
                }
                if Self::args_have_spread(args) {
                    self.args_as_array(args)?;
                    self.chunk.emit(Op::CallSpread(u32::MAX));
                } else {
                    let a = self.plain_args(args)?;
                    self.chunk.emit(Op::Call { argc: a, name: u32::MAX });
                }
                Ok(())
            }
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
                            if f.is_async && f.is_generator {
                                return Err(Unsupported("async-generator"));
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
                            if f.is_async && f.is_generator {
                                return Err(Unsupported("async-generator"));
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
                if f.is_async && f.is_generator {
                    return Err(Unsupported("async-generator"));
                }
                let i = self.chunk.func(f.clone());
                self.chunk.emit(Op::Closure(i));
                Ok(())
            }
            Expr::TaggedTemplate { .. } => Err(Unsupported("tagged-template")),
            Expr::Class(c) => {
                let k = self.chunk.class(c.clone());
                self.chunk.emit(Op::Class(k));
                Ok(())
            }
            Expr::BigInt(t) => {
                let Some(b) = super::bigint::Big::parse(t) else {
                    return Err(Unsupported("bigint-literal"));
                };
                let k = self.chunk.konst(Value::BigInt(alloc::rc::Rc::new(b)));
                self.chunk.emit(Op::Const(k));
                Ok(())
            }
            Expr::Super => Err(Unsupported("super")),

            // Ein `...x` ausserhalb von Feld und Argumentliste hat der Parser
            // schon abgelehnt; hier ist es der nackte Innenausdruck, genau wie
            // im Baumlaeufer.
            Expr::Spread(inner) => self.expr(inner),
            // **Anhalten.** Der Wert geht an `next()` heraus; was `next(v)`
            // hereingibt, legt `Vm::send` an dieselbe Stelle des Stapels und
            // ist damit der Wert dieses Ausdrucks.
            Expr::Yield { arg, delegate } => {
                if !self.in_gen { return Err(Unsupported("yield-outside-generator")) }
                if *delegate { return Err(Unsupported("yield-delegate")) }
                if self.fin > 0 { return Err(Unsupported("yield-in-finally")) }
                match arg {
                    Some(e) => self.expr(e)?,
                    None => {
                        let k = self.chunk.konst(Value::Undefined);
                        self.chunk.emit(Op::Const(k));
                    }
                }
                self.chunk.emit(Op::Yield);
                Ok(())
            }
            // **Warten.** Kein `fin`-Verbot wie beim `yield`: eine wartende
            // Funktion wird nur mit einem WERT oder einem WURF wieder
            // angeworfen, und fuer beides gibt es den Weg schon (`send` und
            // `unwind`). Ein `gen.return()`, das einen Finalisierer noch
            // fahren muesste, gibt es hier nicht.
            Expr::Await(inner) => {
                if !self.in_async { return Err(Unsupported("await-outside-async")) }
                self.expr(inner)?;
                self.chunk.emit(Op::Await);
                Ok(())
            }
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
        let up = op == UpdateOp::Inc;
        match arg {
            Expr::Ident(n) => {
                let i = self.chunk.name(n);
                self.chunk.emit(Op::LoadVar(i));
                // `to_number` VOR dem Rechnen: `x = "3"; x++` gibt 4, nicht
                // "31". `Op::Un(Plus)` ist genau diese Umwandlung.
                self.chunk.emit(Op::ToNumeric);
                if !prefix { self.chunk.emit(Op::Dup); }
                self.chunk.emit(Op::Step(up));
                self.chunk.emit(Op::StoreVar(i));
                if !prefix { self.chunk.emit(Op::Pop); }
                Ok(())
            }
            Expr::Member { obj, prop, optional: false } => {
                self.expr(obj)?;
                match &**prop {
                    MemberProp::Ident(_) | MemberProp::Private(_) => {
                        let i = self.member_name(prop);
                        self.chunk.emit(Op::Dup);
                        self.chunk.emit(Op::GetProp(i));
                        self.chunk.emit(Op::ToNumeric);
                        if !prefix {
                            // Den alten Wert unter das Objekt schieben: er ist
                            // das Ergebnis, das Objekt braucht der Schreiber.
                            self.chunk.emit(Op::Dup);
                            self.chunk.emit(Op::Rot3);
                        }
                        self.chunk.emit(Op::Step(up));
                        self.chunk.emit(Op::SetProp(i));
                        if !prefix { self.chunk.emit(Op::Pop); }
                        Ok(())
                    }
                    // `o[k]++` — dieselbe Regel: Objekt und Schluessel nur
                    // EINMAL. Der alte Wert ist das Ergebnis und muss unter
                    // beiden hindurch nach unten.
                    MemberProp::Computed(k) => {
                        self.expr(k)?;
                        self.chunk.emit(Op::ToKey);
                        self.chunk.emit(Op::Dup2);
                        self.chunk.emit(Op::GetIndex);
                        self.chunk.emit(Op::ToNumeric);
                        if !prefix {
                            self.chunk.emit(Op::Dup);
                            self.chunk.emit(Op::Rot4);
                        }
                        self.chunk.emit(Op::Step(up));
                        self.chunk.emit(Op::SetIndex);
                        if !prefix { self.chunk.emit(Op::Pop); }
                        Ok(())
                    }
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
            let jump = |c: &mut Compiler| match op {
                AssignOp::And => c.chunk.emit_jump(Op::JumpFalseKeep),
                AssignOp::Or => c.chunk.emit_jump(Op::JumpTrueKeep),
                _ => c.chunk.emit_jump(Op::JumpNullishKeep),
            };
            match target {
                Expr::Ident(n) => {
                    let i = self.chunk.name(n);
                    self.chunk.emit(Op::LoadVar(i));
                    let at = jump(self);
                    self.chunk.emit(Op::Pop);
                    self.expr(right)?;
                    self.chunk.emit(Op::StoreVar(i));
                    self.chunk.patch(at);
                    return Ok(());
                }
                // `o.x ||= v` und `o[k] ||= v`. Objekt und Schluessel werden
                // EINMAL ausgewertet und liegen unter dem gelesenen Wert; wird
                // nicht geschrieben, muessen sie wieder weg — deshalb der
                // Umweg ueber zwei Ausgaenge statt eines Sprungs.
                Expr::Member { obj, prop, optional: false } => {
                    let computed = matches!(&**prop, MemberProp::Computed(_));
                    let i = self.member_name(prop);
                    self.expr(obj)?;
                    if let MemberProp::Computed(k) = &**prop {
                        self.expr(k)?;
                        self.chunk.emit(Op::ToKey);
                        self.chunk.emit(Op::Dup2);
                        self.chunk.emit(Op::GetIndex);
                    } else {
                        self.chunk.emit(Op::Dup);
                        self.chunk.emit(Op::GetProp(i));
                    }
                    let keep = jump(self);
                    self.chunk.emit(Op::Pop);
                    self.expr(right)?;
                    if computed { self.chunk.emit(Op::SetIndex); }
                    else { self.chunk.emit(Op::SetProp(i)); }
                    let done = self.chunk.emit_jump(Op::Jump);
                    // Der Kurzschluss: der gelesene Wert ist das Ergebnis,
                    // Objekt (und Schluessel) darunter gehoeren weggeraeumt.
                    self.chunk.patch(keep);
                    self.chunk.emit(Op::Swap);
                    self.chunk.emit(Op::Pop);
                    if computed {
                        self.chunk.emit(Op::Swap);
                        self.chunk.emit(Op::Pop);
                    }
                    self.chunk.patch(done);
                    return Ok(());
                }
                _ => return Err(Unsupported("logical-assign-target")),
            }
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
                MemberProp::Ident(_) | MemberProp::Private(_) => {
                    let i = self.member_name(prop);
                    self.expr(obj)?;
                    self.chunk.emit(Op::Dup);
                    self.chunk.emit(Op::GetProp(i));
                    self.expr(right)?;
                    self.chunk.emit(Op::Bin(bop));
                    self.chunk.emit(Op::SetProp(i));
                    Ok(())
                }
                // `o[k] += v`: Objekt und Schluessel EINMAL auswerten, dann
                // verdoppeln — `o[i++] += 1` darf `i` nicht zweimal zaehlen.
                MemberProp::Computed(k) => {
                    self.expr(obj)?;
                    self.expr(k)?;
                    self.chunk.emit(Op::ToKey);
                    self.chunk.emit(Op::Dup2);
                    self.chunk.emit(Op::GetIndex);
                    self.expr(right)?;
                    self.chunk.emit(Op::Bin(bop));
                    self.chunk.emit(Op::SetIndex);
                    Ok(())
                }
            },
            _ => Err(Unsupported("compound-target")),
        }
    }

    /// Alle Umgebungen schliessen, die zwischen HIER und `depth` offen sind.
    /// Ein Sprung aus einem Block heraus laesst sie sonst stehen.
    /// Alle Schleifeniteratoren schliessen, die zwischen HIER und `n` offen
    /// sind. Der Iterator der ZIELschleife bleibt: bei `continue` laeuft sie
    /// weiter, bei `break` schliesst ihn ihr eigener Nachspann.
    fn unwind_iters(&mut self, n: usize) {
        for _ in n..self.iters {
            self.chunk.emit(Op::IterClose);
        }
    }

    fn unwind_to(&mut self, depth: usize) {
        for _ in depth..self.depth {
            self.chunk.emit(Op::PopEnv);
        }
    }

    /// Der Schluessel eines Elementzugriffs, wo er zur Uebersetzungszeit
    /// feststeht.
    ///
    /// **Ein privates Feld ist dabei nichts Besonderes** — nur ein anderer
    /// Schluesseltext (`value::private_key`, NUL davor). Es faellt damit aus
    /// `own_keys` heraus und ist fuer `Object.keys` und `JSON.stringify`
    /// unsichtbar, verhaelt sich sonst aber wie jede Eigenschaft. Deshalb
    /// steht `Private` ueberall in DEMSELBEN Zweig wie `Ident`: ein eigener
    /// Weg waere eine zweite Semantik fuer denselben Zugriff.
    fn member_name(&mut self, p: &MemberProp) -> u32 {
        match p {
            MemberProp::Ident(n) => self.chunk.name(n),
            MemberProp::Private(n) => {
                let k = super::value::private_key(n);
                self.chunk.name(&k)
            }
            MemberProp::Computed(_) => u32::MAX,
        }
    }

    /// Der Kurzschluss eines `?.`: ist der Wert oben nullish, raeumt er
    /// `depth` Werte ab, legt `undefined` hin und springt ans Ende der Kette.
    ///
    /// Aufgeraeumt wird HIER und nicht am Ende, weil nur hier feststeht,
    /// wieviel unter dem geprueften Wert liegt — bei `a?.b` ist es nichts,
    /// bei `o.f?.()` liegt der Empfaenger darunter.
    /// Den vorgemerkten Namen abholen — jede Schleife genau einmal.
    fn take_label(&mut self) -> Vec<String> {
        core::mem::take(&mut self.pending_labels)
    }

    fn short_circuit(&mut self, depth: usize) -> CompileResult<()> {
        let go_on = self.chunk.emit_jump(Op::JumpNullishKeep);
        for _ in 0..depth { self.chunk.emit(Op::Pop); }
        let k = self.chunk.konst(Value::Undefined);
        self.chunk.emit(Op::Const(k));
        let out = self.chunk.emit_jump(Op::Jump);
        self.chains.last_mut().unwrap().push(out);
        self.chunk.patch(go_on);
        Ok(())
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
