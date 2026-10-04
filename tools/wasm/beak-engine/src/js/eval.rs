//! Tree-walking evaluation of statements and expressions.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::ast::*;
use super::interp::*;
use super::value::*;

impl Interp {
    // ── Statements ───────────────────────────────────────────────────────
    /// Returns the completion value where there is one (a program's value
    /// is its last expression value; `eval` and the console rely on it).
    pub fn exec(&mut self, st: &Stmt, env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(self.throw_kind("RangeError", "step budget exhausted"));
        }
        // Also check the deadline here: pure JS never passes through `tick`.
        // See `Interp::check_deadline`.
        if self.steps & 0xFFFF == 0 { self.check_deadline()?; }
        match st {
            Stmt::Expr(e) => Ok(Some(self.eval(e, env)?)),
            Stmt::Empty | Stmt::Debugger => Ok(None),
            Stmt::VarDecl(d) => { self.exec_var(d, env)?; Ok(None) }
            Stmt::Func(_) => Ok(None), // handled by hoisting
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
                // Leave the label for the loop below, which picks it up on
                // entry (`Interp::pending_labels`). If the body was not a
                // loop nobody picks it up, so it is removed again.
                self.pending_labels.push(label.clone());
                let r = self.exec(body, env);
                self.pending_labels.retain(|x| x != label);
                match r {
                    // A `break lbl` ends exactly here; a `continue lbl`
                    // belongs to the loop below and is caught there.
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
            // `with (o) { … }`: an environment whose names come from an
            // object. Forbidden in strict mode (rejected by the parser), so
            // only the sloppy case reaches here. Vue's template compiler
            // emits `with (_ctx) { … }`.
            Stmt::With { obj, body } => {
                let v = self.eval(obj, env)?;
                let o = self.to_object(&v)?;
                let inner = Env::new(Some(env.clone()), false);
                inner.borrow_mut().with_obj = Some(o);
                // Lookup hints are switched off for the whole session: an
                // object environment can gain and lose bindings after an
                // instruction has already run. Same reasoning as for direct
                // `eval` (`Interp::hints_ok`).
                self.hints_ok = false;
                self.exec(body, &inner)
            }
            // `import` and pure re-exports do nothing at run time; they are
            // resolved at link time (`modules.rs`).
            Stmt::Import(_) | Stmt::ExportAll { .. } => Ok(None),
            // `export function f(){}` is a declaration with `export` in
            // front and must still be evaluated, or `f` would not exist in
            // its own module.
            Stmt::ExportNamed { decl, .. } => match decl {
                Some(d) => self.exec(d, env),
                None => Ok(None),
            },
            Stmt::ExportDefault(d) => {
                let v = match &**d {
                    ExportDefault::Expr(e) => self.eval(e, env)?,
                    ExportDefault::Func(f) => self.make_closure(f.clone(), env, None),
                    ExportDefault::Class(c) => self.eval_class(c, env)?,
                };
                // A named declaration after `export default` also introduces
                // its own name.
                let own = match &**d {
                    ExportDefault::Func(f) => f.name.clone(),
                    ExportDefault::Class(c) => c.name.clone(),
                    _ => None,
                };
                if let Some(n) = own { self.init_binding(&n, v.clone(), env); }
                self.init_binding(super::modules::DEFAULT_LOCAL, v, env);
                Ok(None)
            }
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
            // `var` creates nothing here; hoisting already did. The
            // statement only performs an assignment (ES VariableStatement:
            // PutValue, not InitializeBinding). This matters when the binding
            // lives on the global object: initialising would create a second
            // binding beside it and `window.X` would stay `undefined`.
            // `let`/`const` do initialise: their binding is in the chain,
            // waiting in the TDZ.
            self.bind_pattern(&dec.id, v, env, d.kind != VarKind::Var)?;
        }
        Ok(())
    }

    fn exec_for(&mut self, init: &Option<alloc::boxed::Box<ForInit>>, test: &Option<Expr>,
                update: &Option<Expr>, body: &Stmt, env: &Rc<RefCell<Env>>) -> C<Option<Value>> {
        // Own environment for the head: `for (let i=0;;)` rebinds `i` per
        // iteration so a closure in the body captures that iteration's
        // value (CreatePerIterationEnvironment).
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

    /// Binds the head of a `for..of`/`for..in` loop to a value.
    ///
    /// Three cases, used by both engines: `let`/`const` creates its name
    /// anew per iteration (hence `declare_pattern`), `var` finds the hoisted
    /// binding further out, and a bare target (`for (x of …)`,
    /// `for ([a,b] of …)`) is an assignment and creates nothing.
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
        // Same helper as the bytecode VM; see `Interp::for_in_keys`.
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

    /// `for..of`, stepping the iterator rather than collecting first.
    ///
    /// Collecting first would hang on an endless iterator (a generator, a
    /// stream) even when the body breaks in the first iteration. Every early
    /// exit calls `return()` on the iterator so its `finally` blocks run.
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
        // Find the matching case, then run everything from there on;
        // fall-through is the rule.
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
            // An abrupt completion in `finally` overrides the one from the
            // body, including a thrown error.
            match self.exec_block(f, &fenv) {
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        result
    }

    /// Creates a block's `let`/`const`/`class` bindings (TDZ) and binds its
    /// function declarations.
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

    /// Gives a freshly created anonymous function the name it is being bound
    /// to. Visible in stack traces and `f.name`.
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

    // ── Binding patterns ─────────────────────────────────────────────────
    /// Creates a pattern's names in this environment, then binds them.
    ///
    /// Unlike `bind_pattern(…, true)`, which goes through `init_binding` and
    /// walks up the chain to a same-named outer binding, the head of a
    /// `catch` or a `for` loop creates its names exactly here, anew per
    /// iteration. Used by both engines.
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
                    // `var [a = () => {}] = []` names the arrow `a`, the same
                    // rule as `var f = function(){}` one level deeper.
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
                        let enumerable = o.borrow().is_enumerable(&k);
                        if !enumerable { continue; }
                        let val = self.get(&v, &k)?;
                        out.borrow_mut().set_prop(k, Prop::data(val));
                    }
                    self.bind_pattern(r, Value::Obj(out), env, declare)?;
                }
                Ok(())
            }
            Pat::Expr(e) => {
                // `[a.b] = x`: the target is a property, not a binding.
                self.assign_to_expr(e, v, env)
            }
        }
    }

    /// `PutValue` on an identifier (ES 6.2.5.6). The single implementation
    /// of this rule; keep it that way.
    pub fn assign_ident(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        self.assign_ident_depth(n, v, env).map(|_| ())
    }

    /// The `with` object that carries this name, or `None`.
    ///
    /// The chain is walked only up to the first ordinary binding of the same
    /// name: an inner `var` shadows an outer `with`.
    pub fn with_target(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Option<Gc>> {
        let mut cur = env.clone();
        loop {
            let (wo, hat, up) = {
                let b = cur.borrow();
                (b.with_obj.clone(), b.vars.contains_key(n), b.parent.clone())
            };
            if let Some(o) = wo {
                if self.with_has_pub(&o, n)? { return Ok(Some(o)) }
            }
            if hat { return Ok(None) }
            match up { Some(p) => cur = p, None => return Ok(None) }
        }
    }

    /// Like `assign_ident`, but also reports the depth where the name was
    /// found. Only the lookup hint needs this; see `Chunk::hints`.
    pub fn assign_ident_depth(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>)
        -> C<Option<usize>> {
        // A `with` may carry the name, in which case the write goes to the
        // object. The chain is walked for this only if it contains an object
        // environment at all.
        if let Some(o) = self.with_target(n, env)? {
            let strict = super::interp::env_strict(env);
            return self.set(&Value::Obj(o), n, v, strict).map(|_| None);
        }
        if let Some((e, depth)) = env_lookup_depth(env, n) {
            // Writing to an imported name is an error, not a write into the
            // source module: that binding belongs to the module that runs it.
            if e.borrow().imports.as_ref().is_some_and(|m| m.contains_key(n)) {
                return self.type_err(&alloc::format!("assignment to import '{n}'"));
            }
            let (mutable, init) = {
                let b = e.borrow();
                let bd = b.vars.get(n).unwrap();
                (bd.mutable, bd.initialized)
            };
            if !init { return self.ref_err(&alloc::format!("cannot access '{n}' before initialization")); }
            if !mutable { return self.type_err("assignment to constant variable"); }
            e.borrow_mut().vars.get_mut(n).unwrap().value = v;
            return Ok(Some(depth));
        }
        // Not in the binding chain, but the global object is a binding
        // location too: `Object = 12` writes there, even in strict mode. A
        // name is unresolvable only if the global object lacks it as well.
        let g = self.realm.global.clone();
        if self.has_property(&g, n) {
            let strict = super::interp::env_strict(env);
            return self.set(&Value::Obj(g), n, v, strict).map(|_| None);
        }
        // Truly unknown: sloppy mode creates a global property, strict mode
        // throws.
        super::interp::strict_site!(self, 6);
        if super::interp::env_strict(env) {
            return self.ref_err(&alloc::format!("{n} is not defined"));
        }
        self.set(&Value::Obj(g), n, v, false).map(|_| None)
    }

    fn assign_to_expr(&mut self, e: &Expr, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        match e {
            Expr::Ident(n) => self.assign_ident(n, v, env),
            Expr::Member { obj, prop, .. } => {
                let base = self.eval(obj, env)?;
                let key = self.member_key2(prop, env)?;
                let throw = super::interp::env_strict(env);
                self.set(&base, &key, v, throw)
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

/// Names of a pattern; the same list as for hoisting, used here for a
/// block's TDZ.
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
