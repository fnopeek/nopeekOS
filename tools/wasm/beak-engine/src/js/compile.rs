//! AST -> op list.
//!
//! The compiler declines what it cannot handle (`Unsupported`), and the
//! caller then runs the whole program in the tree walker. There are two
//! machines, but never for the same program; mixing them would be a second
//! semantic path that silently drifts apart.
//!
//! Decline names are keys, not sentences, so the test262 runner can count
//! them.

use alloc::string::String;
use alloc::rc::Rc;
use alloc::vec::Vec;

use super::ast::*;
use super::code::*;
use super::value::Value;

/// Where `break`/`continue` jump. One entry per open loop.
struct Loop {
    /// Sites patched to the loop end.
    breaks: Vec<usize>,
    /// Sites patched to the continue point.
    continues: Vec<usize>,
    /// Number of environments open on entry.
    ///
    /// A `break` out of a block jumps past its `PopEnv`; without this count
    /// the environment stays open and block bindings leak outside.
    depth: usize,
    /// Labels under which `break lbl` / `continue lbl` find this exit. Empty
    /// means reachable only by an unlabeled `break`.
    labels: Vec<String>,
    /// Number of `for…of`/`for…in` iterators open on entry.
    ///
    /// Same bookkeeping as `depth`: a `break lbl`/`continue lbl` jumps past
    /// the inner loop's `IterClose`, and its iterator would otherwise stay in
    /// the frame, unclosed, for the outer loop's `IterNext` to find.
    iters: usize,
    /// A `switch` (or labeled non-loop) is breakable but not continuable:
    /// `break` belongs to it, `continue` to the loop below. Otherwise a
    /// `continue` inside a `switch` would loop back to the `switch` forever.
    brk_only: bool,
}

pub struct Compiler {
    pub chunk: Chunk,
    loops: Vec<Loop>,
    /// Number of currently open `PushEnv`s.
    depth: usize,
    /// Number of currently open loop iterators.
    iters: usize,
    /// Compiling a generator body. Only there is `yield` an `Op::Yield`; an
    /// arrow inside a generator is its own chunk with `in_gen == false`,
    /// which declines rather than emitting a `Yield` into a frame that cannot
    /// suspend.
    in_gen: bool,
    /// Compiling an async function body. Same reasoning as `in_gen` for
    /// `await` in a nested plain arrow.
    in_async: bool,
    /// Open exits of optional chains (`a?.b.c`).
    ///
    /// Short-circuiting applies to the whole chain: `a?.b.c` is `undefined`
    /// when `a` is nullish and never touches `.c`. `Expr::Chain` opens an
    /// entry, each `?.` registers its jump here, and on close all point to
    /// the same end. Each jump pops its own stack first, so the end need not
    /// know how much lay below.
    chains: Vec<Vec<usize>>,
    /// Labels for the next loop.
    ///
    /// In the AST `outer: for (…)` is a label around a loop, but only the
    /// loop knows where `continue outer` goes. The label stores its names
    /// here and the loop takes them when it is created.
    pending_labels: Vec<String>,
    /// Number of currently pending `finally` blocks.
    ///
    /// A `yield` inside is declined, for the same reason as `return`:
    /// `gen.return()` there must still run the finalizer, but finalizers are
    /// copied inline, so there is no place for a saved completion to resume.
    fin: usize,
}

/// Compile a function body.
///
/// Unlike a program there is no completion value (a function without
/// `return` yields `undefined`), and a final `Ret` does exactly that.
/// Parameters and `this` already live in the environment built by
/// `Interp::call_env`; the body starts at its first statement.
pub fn function(f: &Func) -> CompileResult<Chunk> {
    // An async generator suspends on both `yield` and `await`, so both flags
    // may be set. The protocol around it lives in `generator.rs`.
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

/// Compile a whole program. `Err` means the tree walker runs it.
pub fn program(prog: &Program) -> CompileResult<Chunk> {
    let mut c = Compiler { chunk: Chunk::new(), loops: Vec::new(), depth: 0, iters: 0,
                           in_gen: false, in_async: false, fin: 0, chains: Vec::new(), pending_labels: Vec::new() };
    // Hoisting stays in `Interp::hoist`: it works on the environment, so it
    // is shared by both machines. Only the body is compiled here.
    for st in &prog.body {
        c.stmt(st)?;
    }
    c.chunk.emit(Op::Ret);
    Ok(c.chunk)
}

/// The callee's name if it is a dotted chain of identifiers, up to three
/// links: `Foo`, `Intl.PluralRules`, `window.Intl.ListFormat`. Anything else
/// (`t[n]`, `(0, o.X)`) has no name.
pub(crate) fn dotted_name(e: &Expr) -> Option<alloc::string::String> {
    match e {
        Expr::Ident(n) => Some(n.clone()),
        Expr::Member { obj, prop: p, optional: false } => match &**p {
            MemberProp::Ident(p) => {
                let head = dotted_name(obj)?;
                // Three links are enough for an error message.
                if head.matches('.').count() >= 2 { return None }
                Some(alloc::format!("{head}.{p}"))
            }
            _ => None,
        },
        _ => None,
    }
}

impl Compiler {
    // ── Statements ───────────────────────────────────────────────────────
    /// Like `stmt`, but without a completion value: a function's value is its
    /// `return`, so `SetCompletion` per statement would be wasted work.
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
            // A program's value is its last expression value.
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
                // Own environment so `for (let i …)` does not bind into the
                // enclosing block. A fresh binding per iteration is not
                // implemented, so a body that captures it is declined below.
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
                // A `switch` does not catch `continue`; it belongs to the
                // loop below, through the `switch`'s environment.
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
            // Function declarations are handled by hoisting.
            Stmt::Func(_) => Ok(()),
            // A label before a loop belongs to the loop (only it has a
            // continue point); before anything else it is an exit that only
            // `break lbl` reaches.
            Stmt::Labeled { label, body } => {
                // A chain of labels belongs entirely to the loop below:
                // `a: b: for (…)` carries both, and `continue a` is valid.
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
                // Otherwise the label itself is the exit, reachable only by
                // `break lbl`.
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
                // `continue` needs a continue point, which only a loop has.
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
                if *is_await {
                    // `for await` suspends mid-loop. The parser allows it only
                    // in async context, but a nested arrow is its own chunk
                    // and must decline, as for `await` itself.
                    if !self.in_async { return Err(Unsupported("for-await-outside-async")) }
                    return self.for_await(left, right, body);
                }
                self.for_of(left, right, body)
            }
            Stmt::With { .. } => Err(Unsupported("with")),
            // A class declaration: build and bind to its name. Hoisting
            // already created the binding in its TDZ; it is initialized here.
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

    /// What a block binds before its first statement runs, with the same
    /// cases and order as `Interp::hoist`. `var` is not included: it rises to
    /// the function boundary and is already hoisted.
    fn block_decls(&mut self, body: &[Stmt]) -> CompileResult<u32> {
        self.block_decls_of(body.iter())
    }

    /// The same over any sequence. A `switch` hoists across all cases
    /// together: they share one environment, and a function declared in the
    /// third case is visible in the first.
    fn block_decls_of<'a>(&mut self, body: impl Iterator<Item = &'a Stmt>) -> CompileResult<u32> {
        let mut out = Vec::new();
        for st in body {
            match st {
                Stmt::Func(f) => {
                    if let Some(n) = &f.name {
                        let name = self.chunk.name(n);
                        let func = self.chunk.func(f.clone());
                        out.push(BlockDecl::Func { name, func });
                    }
                }
                Stmt::VarDecl(d) if d.kind != VarKind::Var => {
                    // A pattern puts all its names in the TDZ, via the same
                    // `names_of` as `Interp::hoist`.
                    let mut names = Vec::new();
                    for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                    for n in names {
                        let name = self.chunk.name(&n);
                        out.push(BlockDecl::Tdz { name, mutable: d.kind != VarKind::Const });
                    }
                }
                // A class is in the TDZ like a `let`, as in `Interp::hoist`.
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
    /// The finalizer is copied, not called: once for the normal path, once
    /// for the throw path. A subroutine would need a return address on the
    /// stack (the JVM's `jsr`/`ret` problem); two copies of a usually short
    /// block are simpler.
    ///
    /// Not implemented, so declined: `return`/`break`/`continue` out of a
    /// `try` with `finally`. That needs a saved completion that resumes after
    /// the finalizer, and an abrupt finalizer must override it.
    fn try_stmt(&mut self, block: &[Stmt], handler: &Option<CatchClause>,
                finalizer: &Option<Vec<Stmt>>) -> CompileResult<()> {
        if finalizer.is_some() && (Self::jumps_out(block)
            || handler.as_ref().is_some_and(|h| Self::jumps_out(&h.body))) {
            return Err(Unsupported("finally-with-jump"));
        }
        // A `yield` under a pending finalizer is declined like a `return`;
        // see `Compiler::fin`.
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

        // The catch path. The thrown value is on top on arrival.
        let catch_at = self.chunk.here();
        let mut catch_guard = None;
        if let Some(h) = handler {
            self.depth = depth0;
            // With a finalizer the `catch` block needs its own handler: if it
            // throws, the finalizer must still run.
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

        // The throw path without `catch`: finalizer, then rethrow.
        let rethrow_at = self.chunk.here();
        self.depth = depth0;
        if let Some(f) = finalizer {
            self.finalizer(f)?;
        }
        self.chunk.emit(Op::Rethrow);

        // The normal path (also taken after a caught throw).
        self.chunk.patch(to_end);
        self.chunk.patch(after_catch);
        self.depth = depth0;
        if let Some(f) = finalizer {
            self.finalizer(f)?;
        }
        // Only now are the handler's targets known.
        match &mut self.chunk.ops[start] {
            Op::TryStart { catch, finally } => {
                if handler.is_some() {
                    *catch = catch_at;
                    // A caught throw goes through the catch path, which ends
                    // in the normal finalizer.
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

    /// Does anything jump out of this body (`return`, `break`, `continue`)?
    /// A `break` inside a nested loop does not leave the `try`.
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
    /// Iterates lazily through `Op::IterAll`/`Op::IterNext`, using the same
    /// iterator helpers as the tree walker.
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
        // One environment per iteration, so a closure in the body captures
        // this iteration's value, not the last one.
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
        // Two different exits: an early exit closes the iterator
        // (`return()`); an exhausted iterator must not be closed.
        for at in l.breaks { self.chunk.patch(at); }
        self.chunk.emit(Op::IterClose);
        let to_end = self.chunk.emit_jump(Op::Jump);
        self.chunk.patch(done);
        self.chunk.emit(Op::IterDrop);
        self.chunk.patch(to_end);
        self.iters -= 1;
        Ok(())
    }

    /// `for await (x of y)` (ES 14.7.5.7, `iteratorKind: async`).
    ///
    /// Same shape as `for_of` with a suspension point in the middle.
    /// `Op::IterNext` calls `next()` and reads the result in one step; here
    /// it is split into call, `Op::Await`, inspect. The await uses the normal
    /// suspension mechanism.
    fn for_await(&mut self, left: &ForHead, right: &Expr, body: &Stmt) -> CompileResult<()> {
        self.expr(right)?;
        self.chunk.emit(Op::IterAllAsync);
        self.iters += 1;
        let depth0 = self.depth;
        let top = self.chunk.here();
        self.chunk.emit(Op::IterNextAsyncCall);
        self.chunk.emit(Op::Await);
        let done = self.chunk.emit_jump(Op::IterStepAsync);
        let lbl = self.take_label();
        self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                               depth: depth0, brk_only: false, labels: lbl, iters: self.iters });
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
        // Two exits as in `for…of`: early exit closes, exhaustion does not.
        for at in l.breaks { self.chunk.patch(at); }
        self.chunk.emit(Op::IterClose);
        let to_end = self.chunk.emit_jump(Op::Jump);
        self.chunk.patch(done);
        self.chunk.emit(Op::IterDrop);
        self.chunk.patch(to_end);
        self.iters -= 1;
        Ok(())
    }

    /// `yield* x` (ES 15.5.5): delegation to an inner iterator.
    ///
    /// A loop of ops, not a single op: at its suspension point the machine
    /// must know how it was resumed (value, throw or `return`) to forward
    /// exactly that to the inner iterator. That is what `Vm::Resume` carries.
    ///
    /// ```text
    ///   <x>
    ///   DelegateStart            ; get inner iterator, push `undefined`
    /// top:
    ///   DelegateCall(giveup)     ; next / throw / return, per resumption
    ///   [Await]                  ; async generator only
    ///   DelegateStep(end)        ; done -> end, else push value
    ///   Yield | YieldDelegate    ; hand out and suspend
    ///   Jump top
    /// giveup:                    ; inner iterator has no `return`
    ///   Ret                      ; the outer generator gives up
    /// end:
    /// ```
    fn yield_delegate(&mut self, arg: &Expr) -> CompileResult<()> {
        self.expr(arg)?;
        self.chunk.emit(Op::DelegateStart(self.in_async));
        self.iters += 1;
        let top = self.chunk.here();
        let giveup = self.chunk.emit_jump(Op::DelegateCall);
        if self.in_async { self.chunk.emit(Op::Await); }
        let is_async = self.in_async;
        let end = self.chunk.emit(Op::DelegateStep { end: u32::MAX, is_async });
        // The async generator gets the marker too, just not raw: by it
        // `Vm::at_delegate` knows that `throw()`/`return()` are forwarded
        // here instead of unwinding the outer body.
        self.chunk.emit(Op::YieldDelegate(!is_async));
        self.chunk.emit(Op::Jump(top));
        // The inner iterator has no `return`: the value of `gen.return(v)` is
        // already on the stack and the outer body is done.
        self.chunk.patch(giveup);
        self.chunk.emit(Op::Ret);
        self.chunk.patch(end);
        self.iters -= 1;
        Ok(())
    }

    /// `for (k in obj)`.
    ///
    /// Same shape as `for_of`, but the key list is eager
    /// (`Interp::for_in_keys`) and has no `return()`, so both exits share one
    /// `IterDrop`.
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
        // One environment per iteration, as in `for…of`.
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
    /// * One environment for all cases, with the bindings of all case bodies
    ///   hoisted together.
    /// * Fall-through: only the entry point is searched; from there all case
    ///   bodies run in sequence until a `break`.
    /// * Tests are evaluated in order until one matches, and no further.
    ///   `default` is taken only if none matched, wherever it stands.
    ///
    /// The discriminant stays on the stack during the test chain and is
    /// popped in a small gate before any case body runs; otherwise every
    /// `break`, `continue` and outward jump would have to pop it.
    fn switch(&mut self, disc: &Expr, cases: &[SwitchCase]) -> CompileResult<()> {
        self.expr(disc)?;
        let b = self.block_decls_of(cases.iter().flat_map(|c| c.body.iter()))?;
        self.chunk.emit(Op::PushEnv(b));
        self.depth += 1;
        let lbl = self.take_label();
        self.loops.push(Loop { breaks: Vec::new(), continues: Vec::new(),
                               depth: self.depth, brk_only: true, labels: lbl, iters: self.iters });

        // The test chain. Each match jumps to its gate.
        let mut hits = Vec::new();
        for (k, c) in cases.iter().enumerate() {
            let Some(t) = &c.test else { continue };
            self.chunk.emit(Op::Dup);
            self.expr(t)?;
            self.chunk.emit(Op::Bin(BinOp::EqEqEq));
            hits.push((self.chunk.emit_jump(Op::JumpTrue), k));
        }
        // No match: pop the value and go to `default` (or the end).
        self.chunk.emit(Op::Pop);
        let to_default = self.chunk.emit_jump(Op::Jump);

        // The gates: pop the value, then enter the body.
        let mut gates = Vec::new();
        for (at, k) in hits {
            self.chunk.patch(at);
            self.chunk.emit(Op::Pop);
            gates.push((self.chunk.emit_jump(Op::Jump), k));
        }

        // The bodies in sequence; fall-through follows naturally.
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
                // A pattern. The parser rejects `var {a};`, so the
                // initializer is always present. Hoisting created the
                // bindings.
                let Some(e) = &dec.init else {
                    return Err(Unsupported("destructuring-no-init"));
                };
                self.expr(e)?;
                let p = self.chunk.pat(dec.id.clone());
                self.chunk.emit(Op::BindPat { pat: p, mode: BindMode::Init });
                continue;
            };
            // `var x;` without initializer leaves an existing binding alone,
            // otherwise `var f; function f(){}` would erase the hoisted
            // function.
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
            // `var f = function(){}` names the function after the variable.
            if dec.init.as_ref().is_some_and(|e| e.is_anon_fn_def()) {
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

    // ── Expressions ──────────────────────────────────────────────────────
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
            // `typeof x` must not throw on an unbound name, hence its own op.
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
                // `delete x` on anything else is `true`, as in the tree
                // walker; the operand is not evaluated there either.
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
                // `#x in obj`: the left side is a name, not a value.
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
                // The left value only served the short circuit; here the
                // right one counts.
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
                    // `q = function(){}` names the function, as `var q = …`.
                    if right.is_anon_fn_def() { self.chunk.emit(Op::NameFunc(i)); }
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
                // A pattern target: `[a,b] = x`, `({a} = x)`. The value of the
                // assignment is the right side, so a copy stays on the stack.
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
            // The bracket around an optional chain: all short circuits inside
            // end here, not at the individual link.
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
            // `a?.b(…)` and `a?.b?.(…)`: the receiver is `a` and must be kept;
            // the generic callee path would call with `undefined` as `this`.
            Expr::Call { callee, args, optional }
                if matches!(&**callee, Expr::Member { optional: true, .. }) => {
                if self.chains.is_empty() { return Err(Unsupported("optional-outside-chain")) }
                let Expr::Member { obj, prop, .. } = &**callee else { unreachable!() };
                self.expr(obj)?;
                // First check `a`: the `?.` before the name.
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
                // Then the `?.` before the parentheses, if present: receiver
                // and callee are on the stack, the short circuit pops both.
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
                // The receiver is decided here: `o.f()` calls with `o` as
                // `this`, `f()` with undefined. The callee's name is passed
                // along for the "x is not a function" message only.
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
                            // A literal key is known at compile time, so
                            // `o[8362]()` (a minified module registry) can be
                            // named. A truly computed key stays unnamed rather
                            // than costing extra ops on every call.
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
            // `f?.()`: callee and receiver are on the stack, so the short
            // circuit pops two values.
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
                // As for calls, the name is only for the error message.
                let named = match dotted_name(callee) {
                    Some(n) => self.chunk.name(&n),
                    None => u32::MAX,
                };
                self.expr(callee)?;
                if Self::args_have_spread(args) {
                    self.args_as_array(args)?;
                    self.chunk.emit(Op::NewSpread);
                } else {
                    let n = self.plain_args(args)?;
                    self.chunk.emit(Op::New { argc: n, name: named });
                }
                Ok(())
            }
            Expr::Array(items) => {
                let mut mask = Vec::with_capacity(items.len());
                let mut n = 0u16;
                let mut any_spread = false;
                for it in items {
                    match it {
                        // A hole is not `undefined`, but the tree walker
                        // makes the same approximation; keep them identical.
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
                            // Same rule as `Interp::set_literal_proto`.
                            let ist_proto = !p.computed && !p.shorthand
                                && matches!(&p.key, super::ast::PropKey::Ident(n)
                                                  | super::ast::PropKey::Str(n) if n == "__proto__");
                            if ist_proto {
                                self.chunk.emit(Op::SetLiteralProto);
                                continue;
                            }
                            let named = e.is_anon_fn_def();
                            match k {
                                Some(n) => {
                                    if named { self.chunk.emit(Op::NameFunc(n)); }
                                    self.chunk.emit(Op::DefineProp(n));
                                }
                                None => { self.chunk.emit(Op::DefinePropComputed { named }); }
                            }
                        }
                        ObjPropValue::Method(f) => {
                            let k = self.prop_key(&p.key, p.computed)?;
                            let fi = self.chunk.func(f.clone());
                            self.chunk.emit(Op::Closure(fi));
                            match k {
                                Some(n) => {
                                    self.chunk.emit(Op::NameFunc(n));
                                    self.chunk.emit(Op::DefineProp(n));
                                }
                                None => { self.chunk.emit(Op::DefinePropComputed { named: true }); }
                            }
                        }
                        ObjPropValue::Get(f) | ObjPropValue::Set(f) => {
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
                let i = self.chunk.func(f.clone());
                self.chunk.emit(Op::Closure(i));
                Ok(())
            }
            // `tag`a${x}b``: same stack shape as a call (callee, receiver,
            // arguments), with the template object as argument 0.
            Expr::TaggedTemplate { tag, quasis, exprs } => {
                let mut named = u32::MAX;
                match &**tag {
                    Expr::Member { obj, prop, optional: false } => match &**prop {
                        MemberProp::Ident(_) | MemberProp::Private(_) => {
                            named = self.member_name(prop);
                            self.expr(obj)?;
                            self.chunk.emit(Op::Dup);
                            self.chunk.emit(Op::GetProp(named));
                            self.chunk.emit(Op::Swap);
                        }
                        MemberProp::Computed(k) => {
                            self.expr(obj)?;
                            self.chunk.emit(Op::Dup);
                            self.expr(k)?;
                            self.chunk.emit(Op::GetIndex);
                            self.chunk.emit(Op::Swap);
                        }
                    },
                    // `super.tag`x`` and `a?.tag`x`` have their own receiver
                    // rules; decline (the tree walker handles both) rather
                    // than call with `undefined`.
                    Expr::Super | Expr::Member { optional: true, .. } => {
                        return Err(Unsupported("tagged-template-callee"));
                    }
                    other => {
                        if let Expr::Ident(n) = other { named = self.chunk.name(n); }
                        self.expr(other)?;
                        let k = self.chunk.konst(Value::Undefined);
                        self.chunk.emit(Op::Const(k));
                    }
                }
                let t = self.chunk.template(quasis.clone());
                self.chunk.emit(Op::TemplateObject(t));
                for x in exprs { self.expr(x)?; }
                let argc = (exprs.len() + 1) as u16;
                self.chunk.emit(Op::Call { argc, name: named });
                Ok(())
            }
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

            // The parser rejects `...x` outside arrays and argument lists;
            // here it is the bare inner expression, as in the tree walker.
            Expr::Spread(inner) => self.expr(inner),
            // Suspend. The value goes out through `next()`; `Vm::send` puts
            // the argument of `next(v)` in the same stack slot, making it the
            // value of this expression.
            Expr::Yield { arg, delegate } => {
                if !self.in_gen { return Err(Unsupported("yield-outside-generator")) }
                if self.fin > 0 { return Err(Unsupported("yield-in-finally")) }
                if *delegate {
                    let Some(e) = arg else {
                        return Err(Unsupported("yield-delegate-without-operand"));
                    };
                    return self.yield_delegate(e);
                }
                match arg {
                    Some(e) => self.expr(e)?,
                    None => {
                        let k = self.chunk.konst(Value::Undefined);
                        self.chunk.emit(Op::Const(k));
                    }
                }
                // In an async generator the value is awaited first: `yield x`
                // is `AsyncGeneratorYield(? Await(x))` (ES 15.5.5), so
                // `yield Promise.resolve(1)` yields `1`. Emitted here so that
                // `Op::Yield` keeps a single meaning.
                if self.in_async { self.chunk.emit(Op::Await); }
                self.chunk.emit(Op::Yield);
                Ok(())
            }
            // Await. Unlike `yield`, allowed under `finally`: an awaiting
            // function is resumed only with a value or a throw (`send` and
            // `unwind`), never with a `return()` that must run a finalizer.
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

    /// Does the argument list contain `...x`? Then all arguments are built
    /// into one array, since the count is only known at run time.
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

    /// A static property name becomes a name index; a computed one leaves its
    /// key on the stack and returns `None`.
    fn prop_key(&mut self, k: &PropKey, computed: bool) -> CompileResult<Option<u32>> {
        if computed {
            let PropKey::Computed(e) = k else { return Err(Unsupported("prop-key")) };
            self.expr(e)?;
            // Convert immediately: `ToPropertyKey` side effects must happen
            // before the value is evaluated.
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

    /// `x++` / `--o.p`: the target expression is evaluated only once.
    fn update(&mut self, op: UpdateOp, arg: &Expr, prefix: bool) -> CompileResult<()> {
        let up = op == UpdateOp::Inc;
        match arg {
            Expr::Ident(n) => {
                let i = self.chunk.name(n);
                self.chunk.emit(Op::LoadVar(i));
                // Convert before stepping: `x = "3"; x++` gives 4, not "31".
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
                            // Move the old value below the object: it is the
                            // result, the object is needed by the store.
                            self.chunk.emit(Op::Dup);
                            self.chunk.emit(Op::Rot3);
                        }
                        self.chunk.emit(Op::Step(up));
                        self.chunk.emit(Op::SetProp(i));
                        if !prefix { self.chunk.emit(Op::Pop); }
                        Ok(())
                    }
                    // `o[k]++`: object and key evaluated once; the old value
                    // is the result and moves below both.
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

    /// `a += b`, `a ||= b` etc.; the left expression is evaluated once.
    fn compound(&mut self, op: AssignOp, target: &Expr, right: &Expr) -> CompileResult<()> {
        // Short-circuiting forms evaluate the right side only when needed:
        // `a ||= b` must not touch `b` if `a` is truthy.
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
                // `o.x ||= v` and `o[k] ||= v`. Object and key are evaluated
                // once and lie below the read value; if nothing is written
                // they must be popped, hence two exits.
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
                    // Short circuit: the read value is the result; pop the
                    // object (and key) below it.
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
                // `o[k] += v`: evaluate object and key once, then duplicate;
                // `o[i++] += 1` must not increment `i` twice.
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

    /// Close all loop iterators opened since `n`. The target loop's own
    /// iterator stays: `continue` keeps using it, and on `break` the loop's
    /// epilogue closes it.
    fn unwind_iters(&mut self, n: usize) {
        for _ in n..self.iters {
            self.chunk.emit(Op::IterClose);
        }
    }

    /// Close all environments opened since `depth`; a jump out of a block
    /// would otherwise leave them open.
    fn unwind_to(&mut self, depth: usize) {
        for _ in depth..self.depth {
            self.chunk.emit(Op::PopEnv);
        }
    }

    /// The key of a member access, where it is known at compile time.
    ///
    /// A private field is just a different key text (`value::private_key`,
    /// NUL-prefixed): excluded from `own_keys`, so invisible to `Object.keys`
    /// and `JSON.stringify`, otherwise an ordinary property. That is why
    /// `Private` shares the `Ident` branch everywhere.
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

    /// Take the pending labels; each loop takes them exactly once.
    fn take_label(&mut self) -> Vec<String> {
        core::mem::take(&mut self.pending_labels)
    }

    /// The short circuit of a `?.`: if the top is nullish, pop `depth`
    /// values, push `undefined` and jump to the end of the chain.
    ///
    /// Popping happens here, not at the end, because only here is it known
    /// how much lies below the tested value (nothing for `a?.b`, the receiver
    /// for `o.f?.()`).
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

    /// Does a function in this body capture anything?
    ///
    /// Used to decline `for (let i …)` only when it matters: the spec gives
    /// each iteration a fresh binding, which is only observable through a
    /// closure in the body.
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

/// Visit every expression of a statement. Deliberately coarse: the only
/// caller asks whether a function occurs anywhere.
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
        // Other statements are not relevant to the caller.
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

/// The key of a decline. The caller counts them (see `Interp::run_program`).
pub fn unsupported_name(u: &Unsupported) -> &'static str {
    u.0
}

/// Keeps `Chunk::funcs` from counting as dead code while calls go through
/// `Interp::call`.
pub fn funcs_of(c: &Chunk) -> &[Rc<Func>] {
    &c.funcs
}
