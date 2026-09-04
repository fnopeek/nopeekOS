//! Anweisungen und Ausdruecke auswerten.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::ast::*;
use super::interp::*;
use super::value::*;

impl Interp {
    // ── Anweisungen ──────────────────────────────────────────────────────
    /// Liefert den Abschlusswert, wo es einen gibt (der Wert eines Programms
    /// ist der letzte Ausdruckswert — `eval` und die Konsole leben davon).
    pub fn exec(&mut self, st: &Stmt, env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(self.throw_kind("RangeError", "step budget exhausted"));
        }
        match st {
            Stmt::Expr(e) => Ok(Some(self.eval(e, env)?)),
            Stmt::Empty | Stmt::Debugger => Ok(None),
            Stmt::VarDecl(d) => { self.exec_var(d, env)?; Ok(None) }
            Stmt::Func(_) => Ok(None), // beim Hochziehen erledigt
            Stmt::Class(c) => {
                let v = self.eval_class(c, env)?;
                if let Some(n) = &c.name { self.init_binding(n, v, env); }
                Ok(None)
            }
            Stmt::Block(body) => {
                let inner = Env::new(Some(env.clone()), false);
                self.exec_block(body, &inner)
            }
            Stmt::If { test, cons, alt } => {
                if self.eval(test, env)?.truthy() { self.exec(cons, env) }
                else if let Some(a) = alt { self.exec(a, env) }
                else { Ok(None) }
            }
            Stmt::Return(e) => {
                let v = match e { Some(x) => self.eval(x, env)?, None => Value::Undefined };
                Err(Abrupt::Return(v))
            }
            Stmt::Throw(e) => { let v = self.eval(e, env)?; Err(Abrupt::Throw(v)) }
            Stmt::Break(l) => Err(Abrupt::Break(l.clone())),
            Stmt::Continue(l) => Err(Abrupt::Continue(l.clone())),
            Stmt::Labeled { label, body } => {
                // Den Namen fuer die Schleife darunter ablegen — sie holt ihn
                // beim Betreten ab (`Interp::pending_labels`). War der Rumpf
                // keine Schleife, holt ihn niemand, und er gehoert wieder weg.
                self.pending_labels.push(label.clone());
                let r = self.exec(body, env);
                self.pending_labels.retain(|x| x != label);
                match r {
                    // Ein `break lbl` endet GENAU hier; ein `continue lbl`
                    // gehoert der Schleife darunter und wird dort gefangen.
                    Err(Abrupt::Break(Some(l))) if l == *label => Ok(None),
                    other => other,
                }
            }
            Stmt::While { test, body } => {
                let mine = core::mem::take(&mut self.pending_labels);
                while self.eval(test, env)?.truthy() {
                    match self.exec(body, env) {
                        Err(Abrupt::Break(None)) => break,
                        Err(Abrupt::Break(Some(l))) if mine.iter().any(|x| *x == l) => break,
                        Err(Abrupt::Continue(None)) => continue,
                        Err(Abrupt::Continue(Some(l))) if mine.iter().any(|x| *x == l) => continue,
                        Err(e) => return Err(e),
                        Ok(_) => {}
                    }
                }
                Ok(None)
            }
            Stmt::DoWhile { body, test } => {
                let mine = core::mem::take(&mut self.pending_labels);
                loop {
                    match self.exec(body, env) {
                        Err(Abrupt::Break(None)) => break,
                        Err(Abrupt::Break(Some(l))) if mine.iter().any(|x| *x == l) => break,
                        Err(Abrupt::Continue(None)) => {}
                        Err(Abrupt::Continue(Some(l))) if mine.iter().any(|x| *x == l) => {}
                        Err(e) => return Err(e),
                        Ok(_) => {}
                    }
                    if !self.eval(test, env)?.truthy() { break; }
                }
                Ok(None)
            }
            Stmt::For { init, test, update, body } => self.exec_for(init, test, update, body, env),
            Stmt::ForIn { left, right, body } => self.exec_for_in(left, right, body, env),
            Stmt::ForOf { left, right, body, .. } => self.exec_for_of(left, right, body, env),
            Stmt::Switch { disc, cases } => self.exec_switch(disc, cases, env),
            Stmt::Try { block, handler, finalizer } => self.exec_try(block, handler, finalizer, env),
            Stmt::With { .. } => self.type_err("with is not supported"),
            Stmt::Import(_) | Stmt::ExportNamed { .. } | Stmt::ExportDefault(_)
            | Stmt::ExportAll { .. } => Ok(None),
        }
    }

    fn exec_block(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        self.hoist_block(body, env)?;
        let mut last = None;
        for st in body { if let Some(v) = self.exec(st, env)? { last = Some(v); } }
        Ok(last)
    }

    fn exec_var(&mut self, d: &VarDecl, env: &Rc<RefCell<Env>>) -> C<()> {
        for dec in &d.decls {
            let v = match &dec.init {
                Some(e) => {
                    let val = self.eval(e, env)?;
                    if let Pat::Ident(n) = &dec.id {
                        let n = n.clone();
                        self.name_function(&val, &n);
                    }
                    val
                }
                None => Value::Undefined,
            };
            if d.kind == VarKind::Var && dec.init.is_none() { continue; }
            self.bind_pattern(&dec.id, v, env, true)?;
        }
        Ok(())
    }

    fn exec_for(&mut self, init: &Option<alloc::boxed::Box<ForInit>>, test: &Option<Expr>,
                update: &Option<Expr>, body: &Stmt, env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        // Eigene Umgebung fuer den Kopf: `for (let i=0;;)` bindet `i` je
        // Durchlauf neu, damit eine Closure im Rumpf den Wert DIESES Durchlaufs
        // festhaelt. Ohne das teilen sich alle Closures dieselbe Zelle — der
        // Klassiker, an dem `for (var i…)` scheitert.
        let mine = core::mem::take(&mut self.pending_labels);
        let head = Env::new(Some(env.clone()), false);
        let mut per_iter: Vec<Rc<str>> = Vec::new();
        if let Some(i) = init {
            match &**i {
                ForInit::VarDecl(d) => {
                    if d.kind != VarKind::Var {
                        for dec in &d.decls {
                            let mut n = Vec::new();
                            super::eval::names_of(&dec.id, &mut n);
                            for x in n { per_iter.push(Rc::from(x.as_str())); }
                        }
                    }
                    self.hoist_block(&[Stmt::VarDecl(d.clone())], &head)?;
                    self.exec_var(d, &head)?;
                }
                ForInit::Expr(e) => { self.eval(e, &head)?; }
            }
        }
        loop {
            if let Some(t) = test { if !self.eval(t, &head)?.truthy() { break; } }
            let iter_env = if per_iter.is_empty() { head.clone() } else {
                let e = Env::new(Some(head.clone()), false);
                for n in &per_iter {
                    let v = head.borrow().vars.get(n).map(|b| b.value.clone()).unwrap_or(Value::Undefined);
                    e.borrow_mut().vars.insert(n.clone(),
                        Binding { value: v, mutable: true, initialized: true });
                }
                e
            };
            let r = self.exec(body, &iter_env);
            if !per_iter.is_empty() {
                for n in &per_iter {
                    let v = iter_env.borrow().vars.get(n).map(|b| b.value.clone());
                    if let (Some(v), Some(b)) = (v, head.borrow_mut().vars.get_mut(n)) { b.value = v; }
                }
            }
            match r {
                Err(Abrupt::Break(None)) => break,
                Err(Abrupt::Break(Some(l))) if mine.iter().any(|x| *x == l) => break,
                Err(Abrupt::Continue(None)) => {}
                Err(Abrupt::Continue(Some(l))) if mine.iter().any(|x| *x == l) => {}
                Err(e) => return Err(e),
                Ok(_) => {}
            }
            if let Some(u) = update { self.eval(u, &head)?; }
        }
        Ok(None)
    }

    /// Den Kopf einer `for..of`/`for..in`-Schleife an einen Wert binden.
    ///
    /// Drei Faelle in einer Funktion, und beide Maschinen rufen sie: ein
    /// `let`/`const` legt seinen Namen je Durchlauf NEU an (deshalb
    /// `declare_pattern`), ein `var` findet den hochgezogenen weiter aussen,
    /// und ein blosses Ziel (`for (x of …)`, `for ([a,b] of …)`) ist eine
    /// ZUWEISUNG und legt gar nichts an.
    pub fn for_head_bind(&mut self, left: &ForHead, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        match left {
            ForHead::VarDecl(d) => {
                let id = &d.decls[0].id;
                if d.kind != VarKind::Var {
                    return self.declare_pattern(id, v, env);
                }
                self.bind_pattern(id, v, env, true)
            }
            ForHead::Pattern(p) => self.bind_pattern(p, v, env, false),
        }
    }

    fn exec_for_in(&mut self, left: &ForHead, right: &Expr, body: &Stmt,
                   env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        let mine = core::mem::take(&mut self.pending_labels);
        let obj = self.eval(right, env)?;
        // Dieselbe Hilfe wie die Befehlsmaschine — siehe `Interp::for_in_keys`.
        let keys = self.for_in_keys(&obj)?;
        for k in keys {
            let inner = Env::new(Some(env.clone()), false);
            self.for_head_bind(left, Value::Str(k), &inner)?;
            match self.exec(body, &inner) {
                Err(Abrupt::Break(None)) => break,
                Err(Abrupt::Break(Some(l))) if mine.iter().any(|x| *x == l) => break,
                Err(Abrupt::Continue(None)) => continue,
                Err(Abrupt::Continue(Some(l))) if mine.iter().any(|x| *x == l) => continue,
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        Ok(None)
    }

    /// `for..of` — SCHRITTWEISE, nicht erst einsammeln.
    ///
    /// Der Unterschied ist nicht Tempo, sondern Machbarkeit: ein Iterator
    /// ohne Ende (ein Generator, ein Strom) ist voellig gewoehnlich, und ein
    /// Rumpf, der im ersten Durchlauf `break` sagt, muss damit umgehen. Wer
    /// vorher einsammelt, haengt an genau dieser Stelle.
    ///
    /// Und jedes vorzeitige Verlassen ruft `return()` auf dem Iterator —
    /// sonst bleiben fremde `finally`-Bloecke liegen.
    fn exec_for_of(&mut self, left: &ForHead, right: &Expr, body: &Stmt,
                   env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        let mine = core::mem::take(&mut self.pending_labels);
        let src = self.eval(right, env)?;
        let it = self.get_iterator(&src)?;
        loop {
            let Some(v) = self.iter_next(&it)? else { break };
            let inner = Env::new(Some(env.clone()), false);
            if let Err(e) = self.for_head_bind(left, v, &inner) {
                self.iter_close(&it);
                return Err(e);
            }
            match self.exec(body, &inner) {
                Err(Abrupt::Break(None)) => { self.iter_close(&it); break }
                Err(Abrupt::Break(Some(l))) if mine.iter().any(|x| *x == l) => {
                    self.iter_close(&it); break
                }
                Err(Abrupt::Continue(None)) => continue,
                Err(Abrupt::Continue(Some(l))) if mine.iter().any(|x| *x == l) => continue,
                Err(e) => { self.iter_close(&it); return Err(e) }
                Ok(_) => {}
            }
        }
        Ok(None)
    }

    fn exec_switch(&mut self, disc: &Expr, cases: &[SwitchCase],
                   env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        let d = self.eval(disc, env)?;
        let inner = Env::new(Some(env.clone()), false);
        let all: Vec<Stmt> = cases.iter().flat_map(|c| c.body.iter().cloned()).collect();
        self.hoist_block(&all, &inner)?;
        // Erst den passenden Fall suchen, dann AB DORT alles laufen lassen —
        // das Durchfallen ist die Regel, nicht die Ausnahme.
        let mut start = None;
        for (i, c) in cases.iter().enumerate() {
            if let Some(t) = &c.test {
                let tv = self.eval(t, &inner)?;
                if d.strict_eq(&tv) { start = Some(i); break; }
            }
        }
        if start.is_none() { start = cases.iter().position(|c| c.test.is_none()); }
        let Some(s) = start else { return Ok(None) };
        for c in &cases[s..] {
            for st in &c.body {
                match self.exec(st, &inner) {
                    Err(Abrupt::Break(None)) => return Ok(None),
                    Err(e) => return Err(e),
                    Ok(_) => {}
                }
            }
        }
        Ok(None)
    }

    fn exec_try(&mut self, block: &[Stmt], handler: &Option<CatchClause>,
                finalizer: &Option<Vec<Stmt>>, env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        let inner = Env::new(Some(env.clone()), false);
        let mut result = self.exec_block(block, &inner);
        if let (Err(Abrupt::Throw(exc)), Some(h)) = (&result, handler) {
            let exc = exc.clone();
            let cenv = Env::new(Some(env.clone()), false);
            if let Some(p) = &h.param {
                self.declare_pattern(p, exc, &cenv)?;
            }
            result = self.exec_block(&h.body, &cenv);
        }
        if let Some(f) = finalizer {
            let fenv = Env::new(Some(env.clone()), false);
            // Ein Abbruch im `finally` UEBERSCHREIBT den aus dem Rumpf — auch
            // einen geworfenen Fehler. Das ist die Regel und die Falle.
            match self.exec_block(f, &fenv) {
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        result
    }

    /// `let`/`const`/`class` eines Blocks anlegen (Totzone) und
    /// Funktionsdeklarationen binden.
    pub fn hoist_block(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {
        for st in body {
            match st {
                Stmt::VarDecl(d) if d.kind != VarKind::Var => {
                    for dec in &d.decls {
                        let mut n = Vec::new();
                        names_of(&dec.id, &mut n);
                        for x in n {
                            env.borrow_mut().vars.insert(Rc::from(x.as_str()), Binding {
                                value: Value::Undefined,
                                mutable: d.kind != VarKind::Const,
                                initialized: false });
                        }
                    }
                }
                Stmt::Class(c) => if let Some(n) = &c.name {
                    env.borrow_mut().vars.insert(Rc::from(n.as_str()),
                        Binding { value: Value::Undefined, mutable: true, initialized: false });
                },
                Stmt::Func(f) => if let Some(n) = &f.name {
                    let v = self.make_closure(f.clone(), env, None);
                    env.borrow_mut().vars.insert(Rc::from(n.as_str()),
                        Binding { value: v, mutable: true, initialized: true });
                },
                _ => {}
            }
        }
        Ok(())
    }

    /// Eine eben angelegte Bindung auf `const` stellen. Getrennt von
    /// `init_binding`, weil der Baumlaeufer die Veraenderlichkeit schon beim
    /// Hochziehen setzt und die Maschine erst beim Ausfuehren dort ankommt.
    /// Eine Bindung anlegen, die es GIBT, aber noch nicht bereit ist — die
    /// zeitliche Totzone von `let`/`const`/`class`. Dieselbe Zeile, die
    /// `hoist` schreibt; hier oeffentlich, weil die Befehlsmaschine sie beim
    /// Betreten eines Blocks braucht.
    /// Eine fertige Bindung GENAU HIER anlegen — ohne die Kette hochzugehen.
    ///
    /// Der Unterschied zu `init_binding` ist der ganze Punkt: das sucht erst
    /// nach einer vorhandenen Bindung und schreibt DIE. Fuer eine
    /// Funktionsdeklaration in einem Block ist das falsch, und zwar
    /// beobachtbar: `let f = 1; { function f(){} }` darf das aeussere `f`
    /// nicht anfassen (annexB B.3.3 nimmt die var-Bindung genau dann zurueck,
    /// wenn sie einen frueheren Fehler ausloeste). Sieben Tests.
    /// Einer eben gebauten anonymen Funktion den Namen geben, unter dem sie
    /// gerade gebunden wird. Sichtbar in Stapelspuren und in `f.name`.
    pub fn name_function(&mut self, v: &Value, name: &str) {
        let Value::Obj(o) = v else { return };
        let empty = matches!(o.borrow().get_own("name").and_then(|p| p.value.clone()),
            Some(Value::Str(s)) if s.is_empty());
        if !empty { return }
        o.borrow_mut().define("name", Prop {
            value: Some(Value::str(name)), get: None, set: None,
            writable: false, enumerable: false, configurable: true });
    }

    pub fn bind_here(&mut self, name: &str, v: Value, env: &Rc<RefCell<Env>>) {
        env.borrow_mut().vars.insert(Rc::from(name),
            Binding { value: v, mutable: true, initialized: true });
    }

    pub fn declare_tdz(&mut self, name: &str, mutable: bool, env: &Rc<RefCell<Env>>) {
        env.borrow_mut().vars.insert(Rc::from(name), Binding {
            value: Value::Undefined, mutable, initialized: false,
        });
    }

    pub fn make_const(&mut self, name: &str, env: &Rc<RefCell<Env>>) {
        if let Some(b) = env.borrow_mut().vars.get_mut(name) {
            b.mutable = false;
        }
    }

    pub fn init_binding(&mut self, name: &str, v: Value, env: &Rc<RefCell<Env>>) {
        if let Some(e) = env_lookup(env, name) {
            if let Some(b) = e.borrow_mut().vars.get_mut(name) {
                b.value = v; b.initialized = true; return;
            }
        }
        env.borrow_mut().vars.insert(Rc::from(name),
            Binding { value: v, mutable: true, initialized: true });
    }

    // ── Muster binden ────────────────────────────────────────────────────
    /// Die Namen eines Musters HIER anlegen und dann binden.
    ///
    /// Der Unterschied zu `bind_pattern(…, true)` ist der Ort: `init_binding`
    /// laeuft die Umgebungskette HOCH und faende eine gleichnamige Bindung
    /// weiter aussen. Der Kopf eines `catch` und der einer `for`-Schleife
    /// legen ihre Namen aber GENAU HIER an, je Durchlauf neu. Eigene
    /// Funktion, weil beide Maschinen sie brauchen.
    pub fn declare_pattern(&mut self, p: &Pat, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        let mut names = Vec::new();
        names_of(p, &mut names);
        for n in names {
            env.borrow_mut().vars.insert(Rc::from(n.as_str()),
                Binding { value: Value::Undefined, mutable: true, initialized: false });
        }
        self.bind_pattern(p, v, env, true)
    }

    pub fn bind_pattern(&mut self, p: &Pat, v: Value, env: &Rc<RefCell<Env>>, declare: bool) -> C<()> {
        match p {
            Pat::Ident(n) => {
                if declare { self.init_binding(n, v, env); }
                else {
                    self.name_function(&v, n);
                    self.assign_ident(n, v, env)?;
                }
                Ok(())
            }
            Pat::Assign { left, right } => {
                let v = if matches!(v, Value::Undefined) {
                    let d = self.eval(right, env)?;
                    // `var [a = () => {}] = []` nennt den Pfeil `a` — dieselbe
                    // Regel wie `var f = function(){}`, nur eine Ebene tiefer.
                    if let Pat::Ident(n) = &**left {
                        let n = n.clone();
                        self.name_function(&d, &n);
                    }
                    d
                } else { v };
                self.bind_pattern(left, v, env, declare)
            }
            Pat::Rest(inner) => self.bind_pattern(inner, v, env, declare),
            Pat::Array(items) => {
                let vals = self.iterate(&v)?;
                for (i, it) in items.iter().enumerate() {
                    let Some(pat) = it else { continue };
                    if let Pat::Rest(inner) = pat {
                        let rest: Vec<Value> = vals.iter().skip(i).cloned().collect();
                        let arr = self.new_array(rest);
                        self.bind_pattern(inner, arr, env, declare)?;
                        break;
                    }
                    let val = vals.get(i).cloned().unwrap_or(Value::Undefined);
                    self.bind_pattern(pat, val, env, declare)?;
                }
                Ok(())
            }
            Pat::Object { props, rest } => {
                if matches!(v, Value::Undefined | Value::Null) {
                    return self.type_err("cannot destructure undefined or null");
                }
                let mut taken: Vec<Rc<str>> = Vec::new();
                for pr in props {
                    let key = self.prop_key(&pr.key, env)?;
                    taken.push(key.clone());
                    let val = self.get(&v, &key)?;
                    self.bind_pattern(&pr.value, val, env, declare)?;
                }
                if let Some(r) = rest {
                    let o = self.to_object(&v)?;
                    let keys = o.borrow().own_keys();
                    let out = new_obj(Some(self.realm.object_proto.clone()));
                    for k in keys {
                        if taken.contains(&k) { continue; }
                        let enumerable = o.borrow().get_own(&k).map(|p| p.enumerable).unwrap_or(false);
                        if !enumerable { continue; }
                        let val = self.get(&v, &k)?;
                        out.borrow_mut().set_prop(k, Prop::data(val));
                    }
                    self.bind_pattern(r, Value::Obj(out), env, declare)?;
                }
                Ok(())
            }
            Pat::Expr(e) => {
                // `[a.b] = x` — Ziel ist eine Eigenschaft, keine Bindung.
                self.assign_to_expr(e, v, env)
            }
        }
    }

    fn assign_ident(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        if let Some(e) = env_lookup(env, n) {
            let (mutable, init) = {
                let b = e.borrow();
                let bd = b.vars.get(n).unwrap();
                (bd.mutable, bd.initialized)
            };
            if !init { return self.ref_err(&alloc::format!("cannot access '{n}' before initialization")); }
            if !mutable { return self.type_err("assignment to constant variable"); }
            e.borrow_mut().vars.get_mut(n).unwrap().value = v;
            return Ok(());
        }
        // Unbekannter Name: im lockeren Modus wird eine globale Eigenschaft
        // daraus. Der strenge Modus wuerde werfen — noch offen.
        let g = Value::Obj(self.realm.global.clone());
        self.set(&g, n, v)
    }

    fn assign_to_expr(&mut self, e: &Expr, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        match e {
            Expr::Ident(n) => self.assign_ident(n, v, env),
            Expr::Member { obj, prop, .. } => {
                let base = self.eval(obj, env)?;
                let key = self.member_key2(prop, env)?;
                self.set(&base, &key, v)
            }
            _ => self.ref_err("invalid assignment target"),
        }
    }

    pub fn prop_key(&mut self, k: &PropKey, env: &Rc<RefCell<Env>>) -> C<Rc<str>> {
        Ok(match k {
            PropKey::Ident(n) | PropKey::Str(n) => Rc::from(n.as_str()),
            PropKey::Private(n) => private_key(n),
            PropKey::Num(n) => Rc::from(num_to_string(*n).as_str()),
            PropKey::Computed(e) => { let v = self.eval(e, env)?; self.to_prop_key(&v)? }
        })
    }
}

/// Namen eines Musters — dieselbe Liste wie beim Hochziehen, hier fuer die
/// Totzone eines Blocks.
pub fn names_of(p: &Pat, out: &mut Vec<String>) {
    match p {
        Pat::Ident(n) => out.push(n.clone()),
        Pat::Array(items) => for it in items.iter().flatten() { names_of(it, out) },
        Pat::Object { props, rest } => {
            for pr in props { names_of(&pr.value, out); }
            if let Some(r) = rest { names_of(r, out); }
        }
        Pat::Assign { left, .. } => names_of(left, out),
        Pat::Rest(inner) => names_of(inner, out),
        Pat::Expr(_) => {}
    }
}
