//! The bytecode VM: a loop instead of the Rust call stack.
//!
//! The state of a running evaluation lives in fields (`stack`, `frames`),
//! not in Rust frames, so it can be suspended and resumed; that is what
//! generators and `async`/`await` need.
//!
//! The semantics are not reimplemented here. Every op calls the same helper
//! as the tree-walker (`binary`, `unary_val`, `vm_load`, `vm_store`, `get`,
//! `set`, `call`, `construct`, `make_closure`); computing locally would be a
//! second semantics that silently drifts.
//!
//! Each generator object owns its own `Vm` with a single root frame, so a
//! `yield` never has to unwind through a Rust frame, even when the generator
//! is driven by a builtin such as `Array.from`:
//!
//! ```text
//! Array.from                (Rust)
//!   Interp::call(gen.next)  (Rust)
//!     next                  (Rust)
//!       Vm::resume          <- the generator's own VM
//!         Frame(body) ip=17 ... Op::Yield -> returns "suspended"
//! ```
//!
//! One `next()` costs one Rust frame regardless of the caller. A `yield`
//! always belongs to the generator body itself, and any call made from there
//! returns before it continues, so at `Op::Yield` the root frame is the only
//! one; one root frame per VM is enough.

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::code::{Chunk, Op, FIN_JUMP, FIN_NORMAL, FIN_RETURN, FIN_THROW};
use super::interp::{Abrupt, Env, Interp, C};
use super::value::Value;

/// A call frame.
struct Frame {
    chunk: Rc<Chunk>,
    ip: usize,
    /// Environment chain of this frame; `PushEnv`/`PopEnv` operate here.
    envs: Vec<Rc<RefCell<Env>>>,
    /// Stack height on entry; truncated back to on exit so a `Ret` in the
    /// middle of an expression leaves nothing behind.
    base: usize,
    /// Open `try` handlers, innermost last.
    handlers: Vec<Handler>,
    /// Program frame: its value is the completion value, a function's is its
    /// `return`.
    is_program: bool,
    /// Bottom frame of this VM. A throw looks for no handler beyond it, a
    /// `Ret` ends the run, and it does not count toward call depth.
    ///
    /// Separate from `is_program`: a generator body is a root but has no
    /// completion value, so `SetCompletion` in it must not override `return`.
    root: bool,
    /// Open `for…of` / `for…in` iteration state. Kept here rather than on the
    /// value stack so `break` or a throw need not clean it up item by item;
    /// one list so handler depth, `Ret` and `unwind` handle both kinds alike.
    iters: Vec<Iter>,
}

/// State held by a running loop.
enum Iter {
    /// A `for…of` iterator; its `return()` must be called on every early exit.
    Obj(Value),
    /// A `for await` iterator. If `is_async`, the whole result object is
    /// awaited; for a wrapped sync iterator only its `value` is, and `done` is
    /// stored here (ES 27.1.4.4, AsyncFromSyncIteratorContinuation).
    Async { it: Value, is_async: bool, done: bool },
    /// A `for…in` key list, reversed so `pop` yields the next key. Nothing to
    /// close.
    Keys(Vec<Value>),
}

/// An open `try`. The three depths restore the value stack, block
/// environments and open iterations when a throw lands mid-expression.
struct Handler {
    catch_ip: Option<u32>,
    finally_ip: Option<u32>,
    stack: usize,
    envs: usize,
    iters: usize,
}

/// How a run ended.
pub enum Step {
    /// The root frame returned.
    Done(Value),
    /// `yield`: suspended, resumable.
    ///
    /// The `bool` means raw: the value already is the `{value, done}` object
    /// and must not be wrapped again. `yield*` in a sync generator passes the
    /// inner iterator's result object through unchanged (ES 15.5.5,
    /// GeneratorYield), and its identity is observable.
    Yield(Value, bool),
    /// `await`: suspended until a promise settles instead of a `next()`.
    Await(Value),
}

/// Outcome of a single op.
enum Flow {
    /// Continue with the next op.
    Go,
    Done(Value),
    Yield(Value, bool),
    Await(Value),
}

/// How a suspended VM was resumed.
///
/// A plain `yield` only needs `send`; `throw` unwinds and `return` closes.
/// `yield*` needs all three as values, to forward them to the inner iterator
/// instead of acting on them itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Resume { Normal, Throw, Return }

pub struct Vm {
    stack: Vec<Value>,
    frames: Vec<Frame>,
    /// Completion value of the program.
    completion: Value,
    /// How the VM was last resumed; read only by `yield*`.
    resume: Resume,
    /// `resume` preserved across an `await`: in an async generator there is
    /// a suspension between calling the inner iterator and evaluating its
    /// result, and resuming from it via `send` resets `resume` to `Normal`.
    deleg: Resume,
    /// An async generator's `return(v)` is awaiting `v`; the settlement is a
    /// return completion, not the value of the `yield`.
    pub ret_await: bool,
}

impl Vm {
    pub fn new() -> Vm {
        Vm { stack: Vec::new(), frames: Vec::new(), completion: Value::Undefined,
             resume: Resume::Normal, deleg: Resume::Normal, ret_await: false }
    }

    /// Run a compiled program. The caller has already hoisted into `env`;
    /// hoisting works on the environment and is shared by both engines.
    pub fn run(&mut self, i: &mut Interp, chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> C<Value> {
        self.frames.push(Frame { chunk, ip: 0, envs: alloc::vec![env.clone()], base: 0,
                                 is_program: true, root: true,
                                 handlers: Vec::new(), iters: Vec::new() });
        match self.drive(i)? {
            Step::Done(v) => Ok(v),
            // A program body is compiled with `in_gen == false`, so no
            // `Op::Yield` can occur. No `panic!`: a panic halts the system.
            Step::Yield(..) => Err(i.throw_kind("TypeError", "yield outside a generator")),
            Step::Await(_) => Err(i.throw_kind("TypeError", "await outside an async function")),
        }
    }

    /// Run a function body on the VM when the call comes from outside it
    /// (builtin callback, microtask, event handler).
    ///
    /// Without this, such calls would go to the tree-walker and everything
    /// beneath them would stay there. It is the same chunk `Op::Call` would
    /// use, reached from a different caller.
    pub fn run_function(i: &mut Interp, chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> C<Value> {
        let mut vm = Vm::new();
        vm.frames.push(Frame { chunk, ip: 0, envs: alloc::vec![env.clone()], base: 0,
                               is_program: false, root: true,
                               handlers: Vec::new(), iters: Vec::new() });
        match vm.drive(i)? {
            Step::Done(v) => Ok(v),
            Step::Yield(..) => Err(i.throw_kind("TypeError", "yield outside a generator")),
            Step::Await(_) => Err(i.throw_kind("TypeError", "await outside an async function")),
        }
    }

    /// A VM for a generator or async body: one root frame, nothing run yet.
    /// `Interp::call_env` built the environment, so parameters and `this` are
    /// bound at call time while the body runs only on the first `next()`, as
    /// the spec requires.
    pub fn for_generator(chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> Vm {
        let mut vm = Vm::new();
        vm.frames.push(Frame { chunk, ip: 0, envs: alloc::vec![env.clone()], base: 0,
                               is_program: false, root: true,
                               handlers: Vec::new(), iters: Vec::new() });
        vm
    }

    /// Push the value of `next(v)` where `Op::Yield` left off; it becomes the
    /// value of the `yield` expression.
    pub fn send(&mut self, v: Value) {
        self.resume = Resume::Normal;
        self.stack.push(v);
    }

    /// Resume with a throw without unwinding. Only meaningful at a `yield*`
    /// (`at_delegate`), where the throw is a value forwarded to the inner
    /// iterator.
    pub fn send_throw(&mut self, v: Value) {
        self.resume = Resume::Throw;
        self.stack.push(v);
    }

    /// Resume with a `return`. At a `yield*` the inner iterator sees its
    /// `return()` before the outer generator finishes.
    pub fn send_return(&mut self, v: Value) {
        self.resume = Resume::Return;
        self.stack.push(v);
    }

    /// The inner iterator of the running `yield*`.
    fn delegate_iter(&self) -> Option<Value> {
        match self.frames.last()?.iters.last()? {
            Iter::Obj(v) | Iter::Async { it: v, .. } => Some(v.clone()),
            _ => None,
        }
    }

    /// Leave the top frame and return its value.
    ///
    /// Shared by `Op::Ret` and `yield*`: a `return()` that exhausted the inner
    /// iterator is a `return` from the outer body, including closing every
    /// open `for…of` in it.
    fn do_return(&mut self, i: &mut Interp) -> C<Flow> {
        let v = if self.stack.len() > self.frames.last().unwrap().base {
            self.pop()
        } else {
            Value::Undefined
        };
        let f = self.frames.pop().unwrap();
        // Returning out of a `for…of` must close its iterator (innermost
        // first), or a generator never runs its `finally`.
        for it in f.iters.iter().rev() {
            match it { Iter::Obj(v) | Iter::Async { it: v, .. } => i.iter_close(v), _ => {} }
        }
        self.stack.truncate(f.base);
        if f.root {
            if f.is_program {
                // A program's value is its completion value, not what is left
                // on the stack. Function and generator bodies have none.
                let c = core::mem::replace(&mut self.completion, Value::Undefined);
                return Ok(Flow::Done(if matches!(c, Value::Undefined) { v } else { c }));
            }
            return Ok(Flow::Done(v));
        }
        i.depth -= 1;
        if self.frames.is_empty() {
            return Ok(Flow::Done(v));
        }
        self.push(v);
        Ok(Flow::Go)
    }

    /// Whether the suspended VM is at a `yield*`.
    ///
    /// `drive` increments `ip` before executing, so the suspending op is at
    /// `ip - 1`.
    pub fn at_delegate(&self) -> bool {
        let Some(f) = self.frames.last() else { return false };
        f.ip.checked_sub(1)
            .and_then(|k| f.chunk.ops.get(k))
            .is_some_and(|op| matches!(op, Op::YieldDelegate(_)))
    }

    /// `gen.throw(v)`: throw at the suspension point. `false` means nothing
    /// here catches it; the generator is done and the throw goes to the caller.
    pub fn inject_throw(&mut self, i: &mut Interp, v: Value) -> bool {
        self.unwind(i, v)
    }

    /// Abandon the VM. Open `for…of` iterations are closed (innermost first)
    /// so inner generators run their `finally`. Pending finalizers of this
    /// VM are the caller's business (`inject_return`).
    pub fn close(&mut self, i: &mut Interp) {
        while let Some(f) = self.frames.pop() {
            for it in f.iters.iter().rev() {
                match it { Iter::Obj(v) | Iter::Async { it: v, .. } => i.iter_close(v), _ => {} }
            }
            if !f.root { i.depth -= 1; }
        }
        self.stack.clear();
    }

    /// Everything this VM holds, for realm teardown.
    ///
    /// A suspended generator is the only place environments and intermediate
    /// values live outside any property or binding; without this,
    /// `Interp::teardown` would miss them and an Rc cycle there would leak.
    pub fn roots(&self, objs: &mut Vec<super::value::Gc>,
                 envs: &mut Vec<Rc<RefCell<Env>>>) {
        for v in &self.stack {
            if let Value::Obj(o) = v { objs.push(o.clone()); }
        }
        if let Value::Obj(o) = &self.completion { objs.push(o.clone()); }
        for f in &self.frames {
            envs.extend(f.envs.iter().cloned());
            for it in &f.iters {
                if let Iter::Obj(Value::Obj(o)) | Iter::Async { it: Value::Obj(o), .. } = it { objs.push(o.clone()); }
            }
        }
    }

    /// The dispatch loop. Runs until the root frame returns or a `yield` /
    /// `await` suspends it; the next call continues from there.
    pub fn drive(&mut self, i: &mut Interp) -> C<Step> {
        // Hold the chunk instead of cloning the `Rc` per op (two refcount
        // operations on the hottest path); switch only when the frame changes,
        // checked by pointer comparison.
        let mut held: Option<(usize, Rc<Chunk>)> = None;
        loop {
            let flen = self.frames.len();
            let (ip, need) = {
                let f = self.frames.last().unwrap();
                let need = match &held {
                    Some((n, c)) => *n != flen || !Rc::ptr_eq(c, &f.chunk),
                    None => true,
                };
                (f.ip, need)
            };
            if need {
                held = Some((flen, self.frames.last().unwrap().chunk.clone()));
            }
            let chunk: &Chunk = &held.as_ref().unwrap().1;
            if ip >= chunk.ops.len() {
                return Ok(Step::Done(core::mem::replace(&mut self.completion, Value::Undefined)));
            }
            self.frames.last_mut().unwrap().ip += 1;
            // Step budget against `while(true)`, with the same granularity as
            // the tree-walker, which counts per statement. Counting per op
            // would be a stricter limit; instead count what drives a loop
            // forward: backward jumps, calls and statement boundaries.
            let counts = match &chunk.ops[ip] {
                Op::Jump(t) => (*t as usize) <= ip,
                Op::Call { .. } | Op::New { .. } | Op::CallSpread(_) | Op::NewSpread
                | Op::SetCompletion | Op::DeclVar { .. } | Op::Ret
                | Op::Yield | Op::Await | Op::ForInNext(_) | Op::SuperCall(_) | Op::SuperCallSpread
                // `yield*` calls the inner iterator once per round.
                | Op::YieldDelegate(_) | Op::DelegateCall(_) => true,
                _ => false,
            };
            i.vm_ops += 1;
            if counts {
                i.steps += 1;
                if i.steps > i.max_steps {
                    return Err(i.throw_kind("RangeError", "step budget exhausted"));
                }
                // The host deadline, checked at the same granularity as the
                // tree-walker (`Interp::check_deadline`); `tick` alone only
                // covers loops inside builtins.
                if i.steps & 0xFFFF == 0 { i.check_deadline()?; }
            }
            match self.step(i, chunk, ip) {
                Ok(Flow::Done(v)) => return Ok(Step::Done(v)),
                Ok(Flow::Yield(v, raw)) => return Ok(Step::Yield(v, raw)),
                Ok(Flow::Await(v)) => return Ok(Step::Await(v)),
                Ok(Flow::Go) => {}
                // A throw looks for a handler; if none, it goes to the Rust
                // caller.
                Err(Abrupt::Throw(v)) => {
                    if !self.unwind(i, v.clone()) {
                        return Err(Abrupt::Throw(v));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn step(&mut self, i: &mut Interp, chunk: &Chunk, ip: usize) -> C<Flow> {
        // Fetched once before dispatch, not lazily per arm: some ops change
        // the environment chain before using it and must see the old one.
        let env = self.frames.last().unwrap().envs.last().unwrap().clone();
        match &chunk.ops[ip] {
            Op::Const(k) => self.push(chunk.constants[*k as usize].clone()),
            Op::LoadVar(n) => {
                let v = match chunk.hints.get(ip) {
                    Some(h) => i.vm_load_at(&chunk.names[*n as usize], &env, h)?,
                    None => i.vm_load(&chunk.names[*n as usize], &env)?,
                };
                self.push(v);
            }
            Op::StoreVar(n) => {
                let v = self.top();
                match chunk.hints.get(ip) {
                    Some(h) => i.vm_store_at(&chunk.names[*n as usize], v, &env, h)?,
                    None => i.vm_store(&chunk.names[*n as usize], v, &env)?,
                }
            }
            Op::DeclVar { name, mutable, lexical } => {
                let v = self.pop();
                let n = &chunk.names[*name as usize];
                if *lexical {
                    // Bind in this exact env; `init_binding` would walk up and
                    // write a same-named outer binding.
                    i.bind_here(n, v, &env);
                } else {
                    // A `var` is already hoisted; this only assigns
                    // (ES VariableStatement: PutValue). `init_binding` would
                    // create a second binding beside the one on the global
                    // object.
                    i.vm_store(n, v, &env)?;
                }
                if !*mutable {
                    i.make_const(n, &env);
                }
            }
            Op::NameFunc(n) => {
                let v = self.top();
                i.name_function(&v, &chunk.names[*n as usize]);
            }
            Op::ToKey => {
                let v = self.pop();
                let k = i.to_prop_key(&v)?;
                self.push(Value::Str(k));
            }
            Op::This => {
                let v = super::interp::this_observed(i, &env);
                self.push(v);
            }
            Op::Pop => {
                self.pop();
            }
            Op::Dup => {
                let v = self.top();
                self.push(v);
            }
            // `a b` -> `b a`: a call needs `callee` below `this`, and the
            // compiler pushes them the other way round.
            Op::Swap => {
                let n = self.stack.len();
                self.stack.swap(n - 1, n - 2);
            }
            Op::Un(op) => {
                let v = self.pop();
                let r = i.unary_val(*op, v)?;
                self.push(r);
            }
            Op::ToNumeric => { let v = self.pop(); let r = i.to_numeric(&v)?; self.push(r); }
            Op::Step(up) => { let v = self.pop(); let r = i.step_numeric(&v, *up)?; self.push(r); }
            Op::TypeofVar(n) => {
                let v = i.typeof_ident(&chunk.names[*n as usize], &env)?;
                self.push(v);
            }
            Op::Bin(op) => {
                let r = self.pop();
                let l = self.pop();
                let v = i.binary(*op, l, r)?;
                self.push(v);
            }
            Op::Jump(t) => self.jump(*t),
            Op::JumpFalse(t) => {
                let v = self.pop();
                if !v.truthy() {
                    self.jump(*t);
                }
            }
            Op::JumpTrue(t) => {
                let v = self.pop();
                if v.truthy() {
                    self.jump(*t);
                }
            }
            Op::JumpFalseKeep(t) => {
                if !self.top().truthy() {
                    self.jump(*t);
                }
            }
            Op::JumpTrueKeep(t) => {
                if self.top().truthy() {
                    self.jump(*t);
                }
            }
            Op::JumpNullishKeep(t) => {
                if !matches!(self.top(), Value::Undefined | Value::Null) {
                    self.jump(*t);
                }
            }
            Op::GetProp(n) => {
                let obj = self.pop();
                if matches!(obj, Value::Undefined | Value::Null) {
                    return i.type_err(&alloc::format!("cannot read '{}' of {}{}",
                        chunk.names[*n as usize],
                        if matches!(obj, Value::Null) { "null" } else { "undefined" },
                        source_name(chunk, ip)));
                }
                let v = i.get(&obj, &chunk.names[*n as usize])?;
                self.push(v);
            }
            Op::GetIndex => {
                let key = self.pop();
                let obj = self.pop();
                // An integer index is formatted into a stack buffer instead of
                // a heap `Rc<str>`; the key is identical. This is among the
                // hottest ops in compute-heavy code.
                if let Some(ix) = int_index(&key) {
                    let b = IdxBuf::new(ix);
                    let v = i.get(&obj, b.as_str())?;
                    self.push(v);
                } else {
                    // `to_prop_key`, not `to_string`: a symbol is a key, not a
                    // string.
                    let k = i.to_prop_key(&key)?;
                    let v = i.get(&obj, &k)?;
                    self.push(v);
                }
            }
            Op::SetProp(n) => {
                let val = self.pop();
                let obj = self.pop();
                let throw = super::interp::env_strict(&env);
                i.set(&obj, &chunk.names[*n as usize], val.clone(), throw)?;
                self.push(val);
            }
            Op::SetIndex => {
                let val = self.pop();
                let key = self.pop();
                let obj = self.pop();
                let throw = super::interp::env_strict(&env);
                if let Some(ix) = int_index(&key) {
                    let b = IdxBuf::new(ix);
                    i.set(&obj, b.as_str(), val.clone(), throw)?;
                } else {
                    let k = i.to_prop_key(&key)?;
                    i.set(&obj, &k, val.clone(), throw)?;
                }
                self.push(val);
            }
            Op::Call { argc, name } => {
                let args = self.take(*argc as usize);
                let this = self.pop();
                let callee = self.pop();
                let n = chunk.names.get(*name as usize).map(|s| &**s);
                // A direct `eval` sees the caller's scope. Recognized by name
                // and by identity: a user function named `eval` is not one.
                if n == Some("eval") && i.is_eval_fn(&callee) {
                    // From now on a binding may appear further in than a
                    // scope hint points. See `Interp::hints_ok`.
                    i.hints_ok = false;
                    let env = self.frames.last().unwrap().envs.last().unwrap().clone();
                    let c = args.first().cloned().unwrap_or(Value::Undefined);
                    let v = i.perform_eval(&c, Some(env))?;
                    self.push(v);
                    return Ok(Flow::Go);
                }
                self.invoke(i, callee, this, args, n)?;
            }
            Op::New { argc, name } => {
                let args = self.take(*argc as usize);
                let callee = self.pop();
                let n = chunk.names.get(*name as usize).map(|s| &**s);
                let v = i.construct_named(&callee, &args, n)?;
                self.push(v);
            }
            Op::MakeArray(n) => {
                let items = self.take(*n as usize);
                let v = i.new_array(items);
                self.push(v);
            }
            Op::SuperGet(n) => {
                let (v, _) = i.super_get(&chunk.names[*n as usize], &env)?;
                self.push(v);
            }
            Op::SuperCallee(n) => {
                let (v, t) = i.super_get(&chunk.names[*n as usize], &env)?;
                self.push(v);
                self.push(t);
            }
            Op::ImportCall => {
                let _options = self.pop();
                let spec = self.pop();
                let v = i.dynamic_import(&spec, &env)?;
                self.push(v);
            }
            Op::ImportMeta => {
                let n = super::modules::META_LOCAL;
                let v = super::interp::env_lookup(&env, n)
                    .and_then(|e| e.borrow().vars.get(n).map(|b| b.value.clone()))
                    .unwrap_or(Value::Undefined);
                self.push(v);
            }
            Op::SuperCall(argc) => {
                let args = self.take(*argc as usize);
                let v = i.super_call(&args, &env)?;
                self.push(v);
            }
            Op::SuperCallSpread => {
                let args = self.pop();
                let a = i.iterate(&args)?;
                let v = i.super_call(&a, &env)?;
                self.push(v);
            }
            Op::JumpNullishTo(t) => {
                if matches!(self.top(), Value::Undefined | Value::Null) {
                    self.jump(*t);
                }
            }
            Op::Closure(f) => {
                let v = i.func_value(chunk.funcs[*f as usize].clone(), &env);
                self.push(v);
            }
            Op::Method { f, under } => {
                let home = match &self.stack[self.stack.len() - 1 - *under as usize] {
                    Value::Obj(o) => Some(o.clone()),
                    _ => None,
                };
                let v = i.make_method(chunk.funcs[*f as usize].clone(), &env, None, home);
                self.push(v);
            }
            // Same helpers as the tree-walker; see `Op::BindPat`.
            Op::BindPat { pat, mode } => {
                let v = self.pop();
                let p = &chunk.pats[*pat as usize];
                match mode {
                    super::code::BindMode::Init => i.bind_pattern(p, v, &env, true)?,
                    super::code::BindMode::Assign => i.bind_pattern(p, v, &env, false)?,
                    super::code::BindMode::Declare => i.declare_pattern(p, v, &env)?,
                }
            }
            Op::BindHead(h) => {
                let v = self.pop();
                i.for_head_bind(&chunk.heads[*h as usize], v, &env)?;
            }
            // Same function the tree-walker calls; see `Op::Class`.
            Op::Class(c) => {
                let v = i.eval_class(&chunk.classes[*c as usize], &env)?;
                self.push(v);
            }
            Op::NewObject => {
                let g = super::value::new_obj(Some(i.realm.object_proto.clone()));
                self.push(Value::Obj(g));
            }
            Op::SetLiteralProto => {
                let val = self.pop();
                if let Value::Obj(g) = self.top() { let g = g.clone(); i.set_literal_proto(&g, &val); }
            }
            Op::DefineProp(n) => {
                let val = self.pop();
                let key: Rc<str> = chunk.names[*n as usize].clone();
                if let Value::Obj(g) = self.top() {
                    g.borrow_mut().set_prop(key, super::value::Prop::data(val));
                }
            }
            Op::DefinePropComputed { named } => {
                let val = self.pop();
                let key = self.pop();
                let k = i.to_prop_key(&key)?;
                if *named { i.name_function(&val, &k); }
                if let Value::Obj(g) = self.top() {
                    g.borrow_mut().set_prop(k, super::value::Prop::data(val));
                }
            }
            Op::DefineAccessor { name, get } => {
                let f = self.pop();
                let key: Rc<str> = chunk.names[*name as usize].clone();
                let show = alloc::format!("{} {key}", if *get { "get" } else { "set" });
                i.name_function(&f, &show);
                if let Value::Obj(g) = self.top() {
                    i.define_accessor(&g, key, f, *get);
                }
            }
            Op::DefineAccessorComputed { get } => {
                let f = self.pop();
                let key = self.pop();
                let k = i.to_prop_key(&key)?;
                let show = alloc::format!("{} {k}", if *get { "get" } else { "set" });
                i.name_function(&f, &show);
                if let Value::Obj(g) = self.top() {
                    i.define_accessor(&g, k, f, *get);
                }
            }
            Op::SpreadInto => {
                let src = self.pop();
                if let Value::Obj(g) = self.top() {
                    i.spread_into(&g, &src)?;
                }
            }
            Op::Rot3 => {
                let n = self.stack.len();
                let c = self.stack.remove(n - 1);
                self.stack.insert(n - 3, c);
            }
            Op::Rot4 => {
                let n = self.stack.len();
                let c = self.stack.remove(n - 1);
                self.stack.insert(n - 4, c);
            }
            Op::Dup2 => {
                let n = self.stack.len();
                let (a, b) = (self.stack[n - 2].clone(), self.stack[n - 1].clone());
                self.push(a);
                self.push(b);
            }
            Op::Regex { body, flags } => {
                let v = super::regexp::make(i, &chunk.names[*body as usize],
                                            &chunk.names[*flags as usize])?;
                self.push(v);
            }
            Op::TemplateObject(t) => {
                let v = i.template_object(&chunk.templates[*t as usize]);
                self.push(v);
            }
            Op::Concat(n) => {
                let parts = self.take(*n as usize);
                let mut out = alloc::string::String::new();
                for p in &parts {
                    out.push_str(&i.to_string(p)?);
                }
                self.push(Value::string(out));
            }
            Op::PrivateIn(n) => {
                let obj = self.pop();
                let v = i.private_in(&chunk.names[*n as usize], &obj)?;
                self.push(v);
            }
            Op::DeleteProp(n) => {
                let obj = self.pop();
                let key = &chunk.names[*n as usize];
                let v = i.delete_key(&obj, key)?;
                if !v {
                    super::interp::strict_site!(i, 7);
                    if super::interp::env_strict(&env) {
                        return i.type_err(&alloc::format!("cannot delete property '{key}'"));
                    }
                }
                self.push(Value::Bool(v));
            }
            Op::DeleteIndex => {
                let key = self.pop();
                let obj = self.pop();
                let k = i.to_prop_key(&key)?;
                let v = i.delete_key(&obj, &k)?;
                if !v {
                    super::interp::strict_site!(i, 7);
                    if super::interp::env_strict(&env) {
                        return i.type_err(&alloc::format!("cannot delete property '{k}'"));
                    }
                }
                self.push(Value::Bool(v));
            }
            Op::MakeArraySpread { n, spread } => {
                let items = self.take(*n as usize);
                let mask = &chunk.blocks_spread[*spread as usize];
                let mut out = Vec::new();
                for (k, v) in items.into_iter().enumerate() {
                    if mask.get(k).copied().unwrap_or(false) {
                        out.extend(i.iterate(&v)?);
                    } else {
                        out.push(v);
                    }
                }
                let a = i.new_array(out);
                self.push(a);
            }
            Op::CallSpread(name) => {
                let args = self.pop();
                let this = self.pop();
                let callee = self.pop();
                let a = i.iterate(&args)?;
                let n = chunk.names.get(*name as usize).map(|s| &**s);
                self.invoke(i, callee, this, a, n)?;
            }
            Op::NewSpread => {
                let args = self.pop();
                let callee = self.pop();
                let a = i.iterate(&args)?;
                let v = i.construct(&callee, &a)?;
                self.push(v);
            }
            Op::Throw | Op::Rethrow => {
                let v = self.pop();
                return Err(Abrupt::Throw(v));
            }
            Op::TryStart { catch, finally } => {
                let (s, e, it) = {
                    let f = self.frames.last().unwrap();
                    (self.stack.len(), f.envs.len(), f.iters.len())
                };
                self.frames.last_mut().unwrap().handlers.push(Handler {
                    catch_ip: (*catch != u32::MAX).then_some(*catch),
                    finally_ip: (*finally != u32::MAX).then_some(*finally),
                    stack: s, envs: e, iters: it,
                });
            }
            Op::TryEnd => {
                self.frames.last_mut().unwrap().handlers.pop();
            }
            Op::BindCatch(n) => {
                let v = self.pop();
                i.bind_here(&chunk.names[*n as usize], v, &env);
            }
            Op::IterAll => {
                let v = self.pop();
                let it = i.get_iterator(&v)?;
                self.frames.last_mut().unwrap().iters.push(Iter::Obj(it));
            }
            Op::IterNext(done) => {
                let it = match self.frames.last().unwrap().iters.last() {
                    Some(Iter::Obj(v)) => v.clone(),
                    _ => return Err(i.throw_kind("TypeError", "no iterator here")),
                };
                match i.iter_next(&it) {
                    Ok(Some(v)) => self.push(v),
                    Ok(None) => {
                        self.jump(*done);
                    }
                    // If `next()` itself throws, the iterator is not closed
                    // (unlike a throw from the loop body): its state is unknown.
                    // Drop it from the list first so no handler or frame exit
                    // closes it later.
                    Err(e) => {
                        self.frames.last_mut().unwrap().iters.pop();
                        return Err(e);
                    }
                }
            }
            Op::ForInAll => {
                let v = self.pop();
                let mut keys = i.for_in_keys(&v)?;
                // Reversed so `pop` yields the next key.
                keys.reverse();
                let vals = keys.into_iter().map(Value::Str).collect();
                self.frames.last_mut().unwrap().iters.push(Iter::Keys(vals));
            }
            Op::ForInNext(done) => {
                let next = match self.frames.last_mut().unwrap().iters.last_mut() {
                    Some(Iter::Keys(ks)) => ks.pop(),
                    _ => return Err(i.throw_kind("TypeError", "no key list here")),
                };
                match next {
                    Some(k) => self.push(k),
                    None => self.jump(*done),
                }
            }
            Op::IterDrop => {
                self.frames.last_mut().unwrap().iters.pop();
            }
            Op::IterClose => {
                match self.frames.last_mut().unwrap().iters.pop() {
                    Some(Iter::Obj(it)) => i.iter_close(&it),
                    // Not implemented: AsyncIteratorClose awaits the result of
                    // `return()`; we call it and continue without awaiting.
                    Some(Iter::Async { it, .. }) => i.iter_close(&it),
                    _ => {}
                }
            }
            Op::IterAllAsync => {
                let v = self.pop();
                let (it, is_async) = i.get_async_iterator(&v)?;
                self.frames.last_mut().unwrap().iters.push(
                    Iter::Async { it, is_async, done: false });
            }
            Op::IterNextAsyncCall => {
                let (it, is_async) = match self.frames.last().unwrap().iters.last() {
                    Some(Iter::Async { it, is_async, .. }) => (it.clone(), *is_async),
                    _ => return Err(i.throw_kind("TypeError", "no async iterator here")),
                };
                i.tick()?;
                let f = i.get(&it, "next")?;
                if !i.is_callable(&f) {
                    self.frames.last_mut().unwrap().iters.pop();
                    return Err(i.throw_kind("TypeError", "iterator has no next method"));
                }
                // As in `IterNext`: if `next()` itself throws, drop the iterator
                // without closing it.
                let r = match i.call(&f, it.clone(), &[]) {
                    Ok(r) => r,
                    Err(e) => { self.frames.last_mut().unwrap().iters.pop(); return Err(e) }
                };
                if is_async {
                    self.push(r);
                } else {
                    if !matches!(r, Value::Obj(_)) {
                        self.frames.last_mut().unwrap().iters.pop();
                        return Err(i.throw_kind("TypeError", "iterator result is not an object"));
                    }
                    let d = i.get(&r, "done")?.truthy();
                    let v = i.get(&r, "value")?;
                    if let Some(Iter::Async { done, .. }) =
                        self.frames.last_mut().unwrap().iters.last_mut() { *done = d; }
                    self.push(v);
                }
            }
            Op::IterStepAsync(end) => {
                let awaited = self.pop();
                let (is_async, stashed) = match self.frames.last().unwrap().iters.last() {
                    Some(Iter::Async { is_async, done, .. }) => (*is_async, *done),
                    _ => return Err(i.throw_kind("TypeError", "no async iterator here")),
                };
                if is_async {
                    if !matches!(awaited, Value::Obj(_)) {
                        self.frames.last_mut().unwrap().iters.pop();
                        return Err(i.throw_kind("TypeError", "iterator result is not an object"));
                    }
                    if i.get(&awaited, "done")?.truthy() { self.jump(*end); }
                    else { let v = i.get(&awaited, "value")?; self.push(v); }
                } else if stashed {
                    self.jump(*end);
                } else {
                    self.push(awaited);
                }
            }
            Op::PushEnv(b) => {
                let child = Env::new(Some(env.clone()), false);
                // Bind before running: `let` is in the TDZ from block start, a
                // function declaration is initialized from block start. Mirrors
                // `Interp::hoist`.
                for d in &chunk.blocks[*b as usize] {
                    match d {
                        super::code::BlockDecl::Tdz { name, mutable } =>
                            i.declare_tdz(&chunk.names[*name as usize], *mutable, &child),
                        super::code::BlockDecl::Func { name, func } => {
                            let v = i.make_closure(chunk.funcs[*func as usize].clone(), &child, None);
                            i.bind_here(&chunk.names[*name as usize], v, &child);
                        }
                    }
                }
                self.frames.last_mut().unwrap().envs.push(child);
            }
            Op::PopEnv => {
                self.frames.last_mut().unwrap().envs.pop();
            }
            Op::CopyEnv => {
                let copy = {
                    let old = env.borrow();
                    let e = Env::new(old.parent.clone(), false);
                    {
                        let mut n = e.borrow_mut();
                        n.strict = old.strict;
                        for (k, b) in old.vars.iter() {
                            n.vars.insert(k.clone(), super::interp::Binding {
                                value: b.value.clone(), mutable: b.mutable,
                                initialized: b.initialized });
                        }
                    }
                    e
                };
                if let Some(top) = self.frames.last_mut().unwrap().envs.last_mut() { *top = copy; }
            }
            Op::SetCompletion => {
                self.completion = self.pop();
            }
            Op::Ret => return self.ret(i),
            Op::EndFinally(t) => {
                let kind = match self.pop() { Value::Num(n) => n as u32, _ => FIN_NORMAL };
                match kind {
                    FIN_NORMAL => { self.pop(); }
                    FIN_THROW => {
                        let v = self.pop();
                        return Err(Abrupt::Throw(v));
                    }
                    FIN_RETURN => return self.ret(i),
                    k => {
                        self.pop();
                        let to = chunk.finally_tables[*t as usize][(k - FIN_JUMP) as usize];
                        self.jump(to);
                    }
                }
            }
            // ── yield* ───────────────────────────────────────────────────
            //
            // Split into several ops because in an async generator an `await`
            // sits between calling the inner iterator and evaluating its
            // result, and a suspension cannot be folded into one op. Same cut
            // as `for await`.
            Op::DelegateStart(is_async) => {
                let v = self.pop();
                let it = if *is_async {
                    let (it, is_async) = i.get_async_iterator(&v)?;
                    Iter::Async { it, is_async, done: false }
                } else {
                    Iter::Obj(i.get_iterator(&v)?)
                };
                self.frames.last_mut().unwrap().iters.push(it);
                // The first received value is `undefined` with completion type
                // normal (ES 15.5.5 step 5), whatever resumed the outer one.
                self.resume = Resume::Normal;
                self.push(Value::Undefined);
            }
            Op::DelegateCall(giveup) => {
                let received = self.pop();
                let kind = self.resume;
                self.deleg = kind;
                let Some(it) = self.delegate_iter() else {
                    return Err(i.throw_kind("TypeError", "no delegate here"));
                };
                let name = match kind {
                    Resume::Normal => "next",
                    Resume::Throw => "throw",
                    Resume::Return => "return",
                };
                let m = i.get(&it, name)?;
                if !i.is_callable(&m) {
                    self.frames.last_mut().unwrap().iters.pop();
                    return match kind {
                        // An iterator without `throw` is closed and the throw
                        // becomes a TypeError (ES 15.5.5 6.b.iii).
                        Resume::Throw => {
                            i.iter_close(&it);
                            Err(i.throw_kind("TypeError",
                                "the delegated iterator has no throw method"))
                        }
                        // Without `return`, the outer generator returns the
                        // value passed to `gen.return(v)`.
                        Resume::Return => { self.push(received); self.jump(*giveup); Ok(Flow::Go) }
                        Resume::Normal => Err(i.throw_kind("TypeError",
                                "the delegated value is not an iterator")),
                    };
                }
                // If the call itself throws, drop the inner iterator without
                // closing it, as in `for…of`.
                let r = match i.call(&m, it, &[received]) {
                    Ok(r) => r,
                    Err(e) => { self.frames.last_mut().unwrap().iters.pop(); return Err(e) }
                };
                // For a wrapped sync iterator only the value is awaited and
                // `done` is known now (ES 27.1.4.4,
                // AsyncFromSyncIteratorContinuation), as in `for await`.
                let wrapped_sync = matches!(self.frames.last().unwrap().iters.last(),
                                            Some(Iter::Async { is_async: false, .. }));
                if wrapped_sync {
                    if !matches!(r, Value::Obj(_)) {
                        self.frames.last_mut().unwrap().iters.pop();
                        return Err(i.throw_kind("TypeError",
                            "the delegated iterator result is not an object"));
                    }
                    let d = i.get(&r, "done")?.truthy();
                    let v = i.get(&r, "value")?;
                    if let Some(Iter::Async { done, .. }) =
                        self.frames.last_mut().unwrap().iters.last_mut() { *done = d; }
                    self.push(v);
                } else {
                    self.push(r);
                }
            }
            Op::DelegateStep { end, is_async } => {
                let r = self.pop();
                // Wrapped sync case: `done` is on the entry and `r` is the
                // awaited value.
                if let Some(Iter::Async { is_async: false, done, .. }) =
                    self.frames.last().unwrap().iters.last() {
                    let fertig = *done;
                    if fertig {
                        self.frames.last_mut().unwrap().iters.pop();
                        self.push(r);
                        if self.deleg == Resume::Return { return self.ret(i); }
                        self.jump(*end);
                    } else {
                        self.push(r);
                    }
                    return Ok(Flow::Go);
                }
                if !matches!(r, Value::Obj(_)) {
                    self.frames.last_mut().unwrap().iters.pop();
                    return Err(i.throw_kind("TypeError",
                        "the delegated iterator result is not an object"));
                }
                if i.get(&r, "done")?.truthy() {
                    self.frames.last_mut().unwrap().iters.pop();
                    let v = i.get(&r, "value")?;
                    self.push(v);
                    // The resume kind decides the exit: after `gen.return(v)` an
                    // exhausted inner iterator returns from the outer body;
                    // after `next()` the value is the result of `yield*`.
                    if self.deleg == Resume::Return {
                        return self.ret(i);
                    }
                    self.jump(*end);
                } else if *is_async {
                    // An async generator yields the value to be wrapped; a sync
                    // one passes the result object through (`Op::YieldDelegate`).
                    let v = i.get(&r, "value")?;
                    self.push(v);
                } else {
                    self.push(r);
                }
            }
            // The frame stays put; `ip` already points at the next op, and
            // `Vm::send` pushes the value of `next(v)` in place of this one's.
            Op::Yield => {
                let v = self.pop();
                return Ok(Flow::Yield(v, false));
            }
            // Suspension point of a `yield*`. In a sync generator the value is
            // the inner result object, passed through unchanged. It is also the
            // marker `at_delegate` uses to forward `throw()` / `return()`
            // instead of unwinding.
            Op::YieldDelegate(raw) => {
                let v = self.pop();
                return Ok(Flow::Yield(v, *raw));
            }
            Op::Await => {
                let v = self.pop();
                return Ok(Flow::Await(v));
            }
        }
        Ok(Flow::Go)
    }

    /// Find the innermost handler for a throw. `false` means none; the throw
    /// leaves this VM.
    ///
    /// Value stack, environments and open iterations are all truncated to
    /// the depths the handler recorded.
    fn unwind(&mut self, i: &mut Interp, v: Value) -> bool {
        // Search across frames: a throw without a local `try` goes to the
        // caller's.
        let h = loop {
            let Some(f) = self.frames.last_mut() else { return false };
            if let Some(h) = f.handlers.pop() {
                f.envs.truncate(h.envs);
                break h;
            }
            // No handler in this frame: leave it and keep searching. The root
            // frame is the boundary; beyond it the throw goes to Rust as `Err`.
            if f.root {
                return false;
            }
            let f = self.frames.pop().unwrap();
            // Close open `for…of` iterators of the frame being left, innermost
            // first, as in `Ret`.
            for it in f.iters.iter().rev() {
                match it { Iter::Obj(v) | Iter::Async { it: v, .. } => i.iter_close(v), _ => {} }
            }
            self.stack.truncate(f.base);
            i.depth -= 1;
        };
        // A throw out of a `for…of` must close its iterator so a generator
        // still runs its `finally`.
        loop {
            let it = {
                let f = self.frames.last_mut().unwrap();
                if f.iters.len() <= h.iters { break }
                f.iters.pop().unwrap()
            };
            match &it { Iter::Obj(v) | Iter::Async { it: v, .. } => i.iter_close(v), _ => {} }
        }
        let (t, fin) = match (h.catch_ip, h.finally_ip) {
            (Some(c), _) => (c, false),
            (None, Some(f)) => (f, true),
            (None, None) => return false,
        };
        self.frames.last_mut().unwrap().ip = t as usize;
        self.stack.truncate(h.stack);
        self.stack.push(v);
        if fin { self.stack.push(Value::Num(FIN_THROW as f64)); }
        true
    }

    /// `return` from the top frame: through every pending finalizer first,
    /// innermost first. The value is on top.
    fn ret(&mut self, i: &mut Interp) -> C<Flow> {
        let has_fin = self.frames.last().unwrap().handlers.iter().any(|h| h.finally_ip.is_some());
        if !has_fin { return self.do_return(i); }
        let base = self.frames.last().unwrap().base;
        let v = if self.stack.len() > base { self.pop() } else { Value::Undefined };
        self.enter_return(i, v);
        Ok(Flow::Go)
    }

    /// Enter the innermost finalizer of the top frame with a return
    /// completion. Handlers without one are dropped on the way; `false`
    /// means there was none.
    fn enter_return(&mut self, i: &mut Interp, v: Value) -> bool {
        let h = loop {
            let f = self.frames.last_mut().unwrap();
            match f.handlers.pop() {
                None => return false,
                Some(h) if h.finally_ip.is_some() => break h,
                Some(_) => {}
            }
        };
        self.frames.last_mut().unwrap().envs.truncate(h.envs);
        loop {
            let it = {
                let f = self.frames.last_mut().unwrap();
                if f.iters.len() <= h.iters { break }
                f.iters.pop().unwrap()
            };
            match &it { Iter::Obj(v) | Iter::Async { it: v, .. } => i.iter_close(v), _ => {} }
        }
        self.stack.truncate(h.stack);
        self.stack.push(v);
        self.stack.push(Value::Num(FIN_RETURN as f64));
        self.frames.last_mut().unwrap().ip = h.finally_ip.unwrap() as usize;
        true
    }

    /// `gen.return(v)` at a plain `yield`: run the pending finalizers.
    /// `false` means there are none; the caller then closes the machine.
    pub fn inject_return(&mut self, i: &mut Interp, v: Value) -> bool {
        if self.frames.is_empty() { return false }
        self.enter_return(i, v)
    }

    /// Perform a call.
    ///
    /// A JS function with a compilable body gets a new frame, so the Rust
    /// stack does not grow. Everything else (builtins, bound functions,
    /// generators) goes through `Interp::call`.
    ///
    /// Depth is still counted so unbounded recursion throws a `RangeError`
    /// instead of exhausting memory.
    fn invoke(&mut self, i: &mut Interp, callee: Value, this: Value, args: Vec<Value>,
              name: Option<&str>) -> C<()> {
        let d = match &callee {
            Value::Obj(o) => match &o.borrow().kind {
                super::value::ObjKind::Function(d) => Some(d.clone()),
                _ => None,
            },
            _ => None,
        };
        let Some(d) = d else {
            // Name the callee in the error, using the tree-walker's helper.
            if !i.is_callable(&callee) {
                return Err(i.not_a_function(name, Some(&this)));
            }
            i.vm_calls_native += 1;
            let v = i.call(&callee, this, &args)?;
            self.push(v);
            return Ok(());
        };
        // Generators and async functions get no frame here: the call creates
        // an object or promise and the body runs on its own VM, which
        // `Interp::call` sets up. Checked before `func_chunk`, which would
        // otherwise run the body as a plain function.
        if d.node.is_generator || d.node.is_async {
            i.vm_calls_slow += 1;
            let v = i.call(&callee, this, &args)?;
            self.push(v);
            return Ok(());
        }
        let Some(chunk) = i.func_chunk(&d.node) else {
            i.vm_calls_slow += 1;
            let v = i.call(&callee, this, &args)?;
            self.push(v);
            return Ok(());
        };
        i.vm_calls += 1;
        i.depth += 1;
        if i.depth > i.max_depth {
            i.depth -= 1;
            return Err(i.throw_kind("RangeError", "Maximum call stack size exceeded"));
        }
        let env = match i.call_env(&d, this, &args) {
            Ok(e) => e,
            Err(e) => { i.depth -= 1; return Err(e) }
        };
        if let Err(e) = i.hoist_body(&d.node.body, &env) {
            i.depth -= 1;
            return Err(e);
        }
        let base = self.stack.len();
        self.frames.push(Frame {
            chunk, ip: 0, envs: alloc::vec![env], base,
            is_program: false, root: false, handlers: Vec::new(), iters: Vec::new(),
        });
        Ok(())
    }

    fn jump(&mut self, t: u32) {
        self.frames.last_mut().unwrap().ip = t as usize;
    }

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    /// The compiler emits balanced code, so an empty stack here is a compiler
    /// bug. Returns `Undefined` rather than panicking, since a panic halts the
    /// system.
    fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or(Value::Undefined)
    }

    fn top(&mut self) -> Value {
        self.stack.last().cloned().unwrap_or(Value::Undefined)
    }

    fn take(&mut self, n: usize) -> Vec<Value> {
        let at = self.stack.len().saturating_sub(n);
        self.stack.split_off(at)
    }
}

/// The key as an array index, if it is one.
///
/// Same range as `array_index`: integral, `0 <= n < 2^32-1`. In that range the
/// JS string of a number is its plain decimal form, so the key matches what
/// `to_string` would produce. `-0` maps to `"0"`, as in JS.
#[inline]
fn int_index(v: &Value) -> Option<u32> {
    match v {
        Value::Num(n) if *n >= 0.0 && *n < 4294967295.0 && libm::floor(*n) == *n => Some(*n as u32),
        _ => None,
    }
}

/// Decimal digits on the stack; the largest index (`4294967294`) has ten.
struct IdxBuf {
    b: [u8; 10],
    at: usize,
}

impl IdxBuf {
    #[inline]
    fn new(mut v: u32) -> IdxBuf {
        let mut b = [0u8; 10];
        let mut at = 10;
        loop {
            at -= 1;
            b[at] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 { break }
        }
        IdxBuf { b, at }
    }
    #[inline]
    fn as_str(&self) -> &str {
        // SAFETY: only ASCII digits are written, which is valid UTF-8.
        unsafe { core::str::from_utf8_unchecked(&self.b[self.at..]) }
    }
}

/// What produced the value an op at `ip` consumes, as ` (a.b)` for an error
/// message; empty if the previous ops are not a plain name or member chain.
///
/// Minified code has no lines, and the AST carries no positions, so naming
/// the operand is what makes `cannot read 'x' of undefined` findable.
fn source_name(chunk: &Chunk, ip: usize) -> alloc::string::String {
    let mut parts: Vec<&str> = Vec::new();
    let mut j = ip;
    // A method call duplicates the receiver before reading the method.
    if j > 0 && matches!(chunk.ops[j - 1], Op::Dup) { j -= 1 }
    while j > 0 && parts.len() < 3 {
        j -= 1;
        match &chunk.ops[j] {
            Op::GetProp(m) => parts.push(&chunk.names[*m as usize]),
            Op::LoadVar(m) => { parts.push(&chunk.names[*m as usize]); break }
            Op::This => { parts.push("this"); break }
            _ => { if parts.is_empty() { return alloc::string::String::new() } break }
        }
    }
    if parts.is_empty() { return alloc::string::String::new() }
    parts.reverse();
    alloc::format!(" ({})", parts.join("."))
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};

    /// The host deadline must reach both engines.
    ///
    /// The loop deliberately calls no builtin: `Interp::tick` only runs
    /// inside builtins, so a builtin in the loop would hide a missing check.
    static CALLS: AtomicU32 = AtomicU32::new(0);
    fn clock() -> bool {
        CALLS.fetch_add(1, Ordering::Relaxed);
        false // expired immediately: the run must stop
    }

    fn run(novm: bool) -> (bool, u32) {
        CALLS.store(0, Ordering::Relaxed);
        let mut i = super::Interp::new();
        i.vm_off = novm;
        i.deadline = Some(clock);
        let src = "var x = 0; for (var k = 0; k < 400000; k++) { x = x + 1; }";
        let prog = crate::js::parse(src, false).expect("parst");
        let err = i.run_program(&prog).is_err();
        (err, CALLS.load(Ordering::Relaxed))
    }

    /// A named class expression is bound in its own body (ES 15.7.14), on
    /// both engines, and the name does not leak outside.
    fn class_self(novm: bool) -> alloc::string::String {
        let mut i = super::Interp::new();
        i.vm_off = novm;
        let src = "var C = class Inner { \
                   constructor(){ this.ok = this instanceof Inner } \
                   self(){ return new Inner() } \
                   static who(){ return typeof Inner } }; \
                   console.log(C.who(), new C().ok, new C().self().ok, typeof Inner);";
        let prog = crate::js::parse(src, false).expect("parst");
        assert!(i.run_program(&prog).is_ok(), "der Lauf warf");
        i.console.join("|")
    }

    #[test]
    fn eine_benannte_klasse_sieht_sich_selbst() {
        assert_eq!(class_self(false), "function true true undefined");
        assert_eq!(class_self(true), "function true true undefined");
    }

    #[test]
    fn die_uhr_erreicht_die_befehlsmaschine() {
        let (abgebrochen, gefragt) = run(false);
        assert!(gefragt > 0, "die Uhr wurde nie gefragt");
        assert!(abgebrochen, "der Lauf lief ueber die abgelaufene Uhr hinaus");
    }

    #[test]
    fn die_uhr_erreicht_den_baumlaeufer() {
        let (abgebrochen, gefragt) = run(true);
        assert!(gefragt > 0, "die Uhr wurde nie gefragt");
        assert!(abgebrochen, "der Lauf lief ueber die abgelaufene Uhr hinaus");
    }

    /// Scope hints must not change semantics.
    ///
    /// `Chunk::hints` caches the depth at which a name was last found; a
    /// binding created further in later would make it yield a wrong value.
    /// Runs each program with and without hints and compares the results.
    fn zweimal(src: &str) -> (alloc::string::String, alloc::string::String) {
        let lauf = |hints: bool| {
            let mut i = super::Interp::new();
            i.hints_ok = hints;
            let prog = match crate::js::parse(src, false) {
                Ok(p) => p,
                Err(e) => return alloc::format!("SyntaxError @{}", e.at),
            };
            match i.run_program(&prog) {
                Ok(v) => i.to_string(&v).map(|s| s.to_string())
                          .unwrap_or_else(|_| alloc::string::String::from("?")),
                Err(super::Abrupt::Throw(v)) => {
                    let m = i.get(&v, "message").ok()
                        .and_then(|m| i.to_string(&m).ok())
                        .unwrap_or_else(|| alloc::rc::Rc::from("?"));
                    alloc::format!("THROW {m}")
                }
                Err(_) => alloc::string::String::from("ABRUPT"),
            }
        };
        (lauf(true), lauf(false))
    }

    #[test]
    fn wegweiser_aendert_die_bedeutung_nicht() {
        let faelle: &[&str] = &[
            // Shadowing in a block inside a loop: one op site, many passes.
            "var o=[];var x='a';for(var k=0;k<3;k++){let x='b'+k;o.push(x);}o.push(x);o.join(',')",
            // A closure reads outward while an inner binding has the same name.
            "var x='aussen';function f(){return x}function g(){var x='innen';return f()+'|'+x}g()",
            // TDZ: the name exists but is not initialized yet.
            "function f(){ try { return y } catch(e) { return 'TDZ:'+e.name } finally { } } var r=f(); let y=1; r",
            // Assigning to `const` must throw the same error.
            "const c=1; try { c=2; return } catch(e) { e.name }",
            // A direct `eval` that creates a binding further in.
            "var x='aussen';function f(){function inner(){return x}var a=inner();eval(\"var x='innen'\");return a+'|'+inner()}f()",
            // Deep nesting, so the lookup takes several hops.
            "var a=1;function f(){var b=2;return function(){var c=3;return function(){return a+b+c}}}f()()()",
            // The same name on several levels, read inside out.
            "var n='g';function f(){var n='f';{let n='b';return n+f2()}}function f2(){return n}f()",
        ];
        for (k, src) in faelle.iter().enumerate() {
            let (mit, ohne) = zweimal(src);
            assert_eq!(mit, ohne, "Fall {k} laeuft mit und ohne Wegweiser auseinander: {src}");
        }
    }

    /// Runs a program on both engines; their output must match exactly.
    fn beide(src: &str) -> (alloc::string::String, alloc::string::String) {
        let lauf = |vm: bool| {
            let mut i = super::Interp::new();
            i.vm_off = !vm;
            let prog = match crate::js::parse(src, false) {
                Ok(p) => p,
                Err(e) => return alloc::format!("SyntaxError: {}", e.msg),
            };
            match i.run_program(&prog) {
                Ok(v) => i.to_string(&v).map(|s| s.to_string())
                          .unwrap_or_else(|_| alloc::string::String::from("?")),
                Err(super::Abrupt::Throw(v)) => {
                    let m = i.get(&v, "message").ok()
                        .and_then(|m| i.to_string(&m).ok())
                        .unwrap_or_else(|| alloc::rc::Rc::from("?"));
                    alloc::format!("THROW {m}")
                }
                Err(_) => alloc::string::String::from("ABRUPT"),
            }
        };
        (lauf(true), lauf(false))
    }

    /// `yield*` forwards all three resume kinds (value, throw, `return`) to
    /// the inner iterator.
    #[test]
    fn yield_star_reicht_alle_drei_anwuerfe_weiter() {
        let faelle: &[(&str, &str)] = &[
            // The inner generator's return value is the value of `yield*`.
            ("function* i(){ yield 1; yield 2; return 'fin' }\
              function* o(){ out.push(yield* i()) }\
              [...o()].forEach(x=>out.unshift(x))", "2|1|fin"),
            // Over an array, a string and a builtin iterator.
            ("function* g(){ yield* [1,2]; yield* 'ab'; yield* new Map([['k',1]]).keys() }\
              out.push(...g())", "1|2|a|b|k"),
            // `next(v)` reaches the inner `yield`.
            ("function* e(){ while(true){ const g = yield '?'; if(g==='stop') return 'E' } }\
              function* w(){ out.push('R:'+(yield* e())) }\
              var h=w(); h.next(); h.next('a'); h.next('stop')", "R:E"),
            // `throw()` goes to the inner iterator, which may catch it.
            ("function* c(){ try{ yield 'A' }catch(e){ yield 'f:'+e.message } yield 'B' }\
              function* o(){ yield* c(); yield 'z' }\
              var h=o(); out.push(h.next().value);\
              out.push(h.throw(new Error('bam')).value);\
              out.push(h.next().value); out.push(h.next().value)", "A|f:bam|B|z"),
            // `return()` lets the inner one clean up before the outer finishes.
            ("var m={[Symbol.iterator](){return{next:()=>({value:'x',done:false}),\
              return:(v)=>{out.push('zu');return{value:v,done:true}}}}};\
              function* o(){ yield* m; out.push('nie') }\
              var h=o(); h.next(); out.push(h.return('ok').value)", "zu|ok"),
            // The inner result object is passed through unchanged.
            ("var m={[Symbol.iterator](){var n=0;return{next:()=>n++?{value:9,done:true}\
              :{value:7,done:false,mine:true}}}};\
              function* o(){ yield* m }\
              out.push(o().next().mine)", "true"),
            // An iterator without `throw` is closed, then a TypeError
            // (ES 15.5.5, 6.b.iii).
            ("var m={[Symbol.iterator](){return{next:()=>({value:1,done:false}),\
              return:()=>{out.push('zu');return{done:true}}}}};\
              function* o(){ yield* m }\
              var h=o(); h.next();\
              try{ h.throw(new Error('x')) }catch(e){ out.push(e.constructor.name) }",
             "zu|TypeError"),
        ];
        for (k, (src, want)) in faelle.iter().enumerate() {
            let full = alloc::format!("var out=[];{src};out.join('|')");
            let mut i = super::Interp::new();
            let prog = crate::js::parse(&full, false).expect("parst");
            let got = match i.run_program(&prog) {
                Ok(v) => i.to_string(&v).map(|s| s.to_string())
                          .unwrap_or_else(|_| alloc::string::String::from("?")),
                Err(super::Abrupt::Throw(v)) => {
                    let m = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok())
                             .unwrap_or_else(|| alloc::rc::Rc::from("?"));
                    alloc::format!("THROW {m}")
                }
                Err(_) => alloc::string::String::from("ABRUPT"),
            };
            assert_eq!(&got, want, "Fall {k}: {src}");
        }
    }

    /// `yield*` in an async generator: same path, with awaits in between.
    #[test]
    fn yield_star_im_async_generator() {
        let faelle: &[(&str, &str)] = &[
            ("async function* i(){ yield 1; await null; yield 2; return 'fin' }\
              async function* o(){ out.push('r='+(yield* i())) }\
              (async()=>{ for await (const x of o()) out.unshift(x) })()", "2|1|r=fin"),
            // Delegating to a sync iterator awaits its values
            // (AsyncFromSyncIteratorContinuation).
            ("async function* g(){ yield* [Promise.resolve('p'),'q'] }\
              (async()=>{ for await (const x of g()) out.push(String(x)) })()", "p|q"),
            // `throw()` reaches the inner generator here too.
            ("async function* c(){ try{ yield 'A' }catch(e){ yield 'f:'+e.message } }\
              async function* o(){ yield* c(); yield 'z' }\
              (async()=>{ const h=o(); out.push((await h.next()).value);\
              out.push((await h.throw(new Error('bam'))).value);\
              out.push((await h.next()).value) })()", "A|f:bam|z"),
        ];
        for (k, (src, want)) in faelle.iter().enumerate() {
            assert_eq!(&async_out(src), want, "Fall {k}: {src}");
        }
    }

    /// Async generators and `for await`: `yield` suspends for `next()`,
    /// `await` for the microtask queue, both in the same VM.
    ///
    /// Runs the program, drains the job queue, then reads `out`; an async
    /// generator delivers its first value no earlier than the next microtask.
    fn async_out(src: &str) -> alloc::string::String {
        let mut i = super::Interp::new();
        let full = alloc::format!("var out=[];{src};out.join('|')");
        let prog = match crate::js::parse(&full, false) {
            Ok(p) => p,
            Err(e) => return alloc::format!("SyntaxError: {}", e.msg),
        };
        if let Err(super::Abrupt::Throw(v)) = i.run_program(&prog) {
            let m = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok())
                     .unwrap_or_else(|| alloc::rc::Rc::from("?"));
            return alloc::format!("THROW {m}");
        }
        crate::js::promise::run_jobs(&mut i);
        let read = crate::js::parse("out.join('|')", false).expect("parst");
        match i.run_program(&read) {
            Ok(v) => i.to_string(&v).map(|s| s.to_string())
                      .unwrap_or_else(|_| alloc::string::String::from("?")),
            Err(_) => alloc::string::String::from("ABRUPT"),
        }
    }

    #[test]
    fn async_generatoren_halten_an_yield_und_an_await() {
        let faelle: &[(&str, &str)] = &[
            // `yield` in an async generator awaits its operand (ES 15.5.5):
            // `yield Promise.resolve(2)` yields 2, not the promise.
            ("async function* g(){ yield 1; yield Promise.resolve(2); await null; yield 3 }              (async()=>{ for await (const x of g()) out.push(x) })()", "1|2|3"),
            // Three concurrent `next()` calls queue up instead of resuming the
            // VM three times (ES 27.6.3.6).
            ("async function* g(){ yield 'a'; yield 'b'; yield 'c' }              (async()=>{ const h=g(); const r=await Promise.all([h.next(),h.next(),h.next()]);              r.forEach(x=>out.push(x.value)) })()", "a|b|c"),
            // A finished generator answers every further request.
            ("async function* g(){ yield 1 }              (async()=>{ const h=g(); await h.next(); const e=await h.next();              out.push(e.done); out.push(String(e.value)) })()", "true|undefined"),
            // A throw in the body rejects; it does not throw synchronously.
            ("async function* g(){ yield 1; throw new Error('drin') }              (async()=>{ const h=g(); await h.next();              try { await h.next() } catch(e) { out.push('abgelehnt:'+e.message) } })()",
             "abgelehnt:drin"),
            // `for await` over an array wraps the sync iterator and awaits each
            // value.
            ("(async()=>{ for await (const x of [Promise.resolve('p'),'q']) out.push(x) })()", "p|q"),
            // `break` closes the async iterator.
            ("const it={[Symbol.asyncIterator](){let n=0;return{                next:()=>Promise.resolve({value:n++,done:n>9}),                return:()=>{out.push('zu');return Promise.resolve({done:true})}}}};              (async()=>{ for await (const x of it){ out.push(x); if(x===1) break } })()",
             "0|1|zu"),
            // The toStringTag and self-iteration live on the prototype.
            ("async function* g(){}              (async()=>{ const h=g();              out.push(Object.prototype.toString.call(h));              out.push(h[Symbol.asyncIterator]()===h) })()",
             "[object AsyncGenerator]|true"),
        ];
        for (k, (src, want)) in faelle.iter().enumerate() {
            assert_eq!(&async_out(src), want, "Fall {k}: {src}");
        }
    }

    /// The compiler must accept async generators, including when they only
    /// appear beside other code in the same chunk.
    #[test]
    fn ein_async_generator_laesst_den_chunk_nicht_absagen() {
        for src in ["async function* g(){ yield 1 } 1 + 1",
                    "var o = { async *m(){ yield 1 } }; 1 + 1",
                    "var f = async function*(){ yield 1 }; 1 + 1",
                    "class C { async *m(){ yield 1 } } 1 + 1",
                    "async function h(){ for await (const x of []) {} } 1 + 1"] {
            let prog = crate::js::parse(src, false).expect("parst");
            assert!(super::super::compile::program(&prog).is_ok(),
                "der Uebersetzer sagt noch ab: {src}");
        }
    }

    /// Tagged templates behave the same on both engines.
    #[test]
    fn getaggte_templates_sagen_auf_beiden_maschinen_dasselbe() {
        let faelle: &[(&str, &str)] = &[
            // Cooked and raw strings plus substitutions.
            (r"function t(s,...v){return s.raw.join('|')+'#'+s.join('|')+'#'+v.join(',')+'#'+s.length}t`a${1}b\t${2}c`",
             "a|b\\t|c#a|b\t|c#1,2#3"),
            // `String.raw` is the builtin tag.
            (r"String.raw`x\ny${5}z`", r"x\ny5z"),
            // The same site yields the same template object on every
            // evaluation (ES 13.2.8.4); libraries key caches by it.
            ("var a=[];for(var k=0;k<3;k++){a.push((function(s){return s})`same`)} String(a[0]===a[1] && a[1]===a[2])", "true"),
            // Two different sites with the same text do not.
            ("var f=function(s){return s};String(f`same` !== f`same`)", "true"),
            // Frozen, and `raw` is not enumerable.
            ("function t(s){return [Object.isFrozen(s),Object.isFrozen(s.raw),Object.keys(s).join(',')].join('|')}t`q${1}r`", "true|true|0,1"),
            // The receiver belongs to the call: `o.t`x`` calls with `o`.
            ("var o={n:7,t(s){return this.n}};String(o.t`q`)", "7"),
            // An invalid escape makes the cooked string undefined and keeps
            // `raw`, only when tagged.
            (r"function t(s){return String(s[0])+'/'+s.raw[0]}t`\xg`", r"undefined/\xg"),
            (r"function t(s){return String(s[0])}t`\1`", "undefined"),
            // Untagged, the same escape is an early error.
            (r"`a\xgb`", "SyntaxError: invalid escape sequence in template"),
            (r"`a\1b`", "SyntaxError: invalid escape sequence in template"),
            // `\0` not followed by a digit is allowed.
            (r"`a\0b`.length.toString()", "3"),
        ];
        for (k, (src, want)) in faelle.iter().enumerate() {
            let (vm, walker) = beide(src);
            assert_eq!(vm, walker, "Fall {k}: die zwei Maschinen sagen Verschiedenes: {src}");
            assert_eq!(&vm, want, "Fall {k}: {src}");
        }
    }

    /// A freed function node must not pass its compiled body on.
    ///
    /// `func_chunks` caches compiled bodies by AST node address. An address is
    /// only an identity while it is allocated: a function in a later script
    /// could land at a freed address and run the earlier function's body.
    ///
    /// Tests the invariant directly (a cached address is never reused)
    /// rather than hoping for an allocator collision.
    #[test]
    fn eine_gemerkte_adresse_wird_nicht_neu_vergeben() {
        use crate::js::ast::Func;
        use alloc::rc::Rc;
        fn leer(name: &str) -> Rc<Func> {
            Rc::new(Func { name: Some(alloc::string::String::from(name)),
                           params: alloc::vec::Vec::new(), body: alloc::vec::Vec::new(),
                           is_async: false, is_generator: false, is_arrow: false,
                           expr_body: false, strict: false })
        }
        let mut i = super::Interp::new();
        let f = leer("erste");
        let adresse = Rc::as_ptr(&f) as usize;
        // Compile and cache; a body is now keyed by this address.
        let _ = i.func_chunk(&f);
        drop(f);
        // Allocators tend to hand out a just-freed cell immediately.
        for k in 0..64 {
            let g = leer("zweite");
            assert_ne!(Rc::as_ptr(&g) as usize, adresse,
                "Versuch {k}: eine neue Funktion liegt auf der Adresse, unter der \
                 noch ein fremder Rumpf gemerkt ist — sie bekaeme ihn beim Aufruf");
        }
    }

    /// A direct `eval` must disable scope hints: it is the only way a binding
    /// can appear further in after the fact.
    #[test]
    fn direktes_eval_schaltet_die_wegweiser_ab() {
        let mut i = super::Interp::new();
        assert!(i.hints_ok, "am Anfang sind sie an");
        let prog = crate::js::parse("var q = 1; eval(\"q = 2\"); q", false).expect("parst");
        let _ = i.run_program(&prog);
        assert!(!i.hints_ok, "nach einem direkten eval muessen sie aus sein");
    }

    /// Property reads on primitives start at the prototype without creating a
    /// wrapper object (which for a string would materialize every character);
    /// results must be the same as with a wrapper.
    #[test]
    fn primitive_antworten_ohne_huelle_dasselbe() {
        let faelle: &[(&str, &str)] = &[
            ("\"abc\".length", "3"),
            ("\"abc\"[1]", "b"),
            ("String(\"abc\"[9])", "undefined"),
            ("\"abcb\".indexOf(\"b\")", "1"),
            ("\"abc\".toUpperCase()", "ABC"),
            ("\"abc\".charAt(2)", "c"),
            // Own properties of a primitive still go through `to_object`.
            ("String(\"abc\".hasOwnProperty(\"0\"))", "true"),
            ("String(\"abc\".hasOwnProperty(\"length\"))", "true"),
            ("Object.keys(\"ab\").join(\",\")", "0,1"),
            ("(function(){var o=[];for(var k in \"ab\")o.push(k);return o.join(\",\")})()", "0,1"),
            // A real String object is unaffected.
            ("Object.keys(new String(\"ab\")).join(\",\")", "0,1"),
            ("new String(\"ab\")[1]", "b"),
            // The lookup continues up the chain past the prototype.
            ("(function(){Object.prototype.zz=\"Z\";var v=\"ab\".zz;delete Object.prototype.zz;return v})()", "Z"),
            // The other primitives.
            ("(255).toString(16)", "ff"),
            ("(1.5).toFixed(2)", "1.50"),
            ("true.toString()", "true"),
            ("typeof Symbol(\"x\").description", "string"),
        ];
        for (src, want) in faelle {
            let mut i = super::Interp::new();
            let prog = crate::js::parse(src, false).expect("parst");
            let got = match i.run_program(&prog) {
                Ok(v) => i.to_string(&v).map(|s| s.to_string())
                          .unwrap_or_else(|_| alloc::string::String::from("?")),
                Err(_) => alloc::string::String::from("WURF"),
            };
            assert_eq!(&got, want, "fuer {src}");
        }
    }

    /// The completion value of `src` as a string, on both engines; they must agree.
    fn on_both(src: &str) -> alloc::string::String {
        let mut out = alloc::vec::Vec::new();
        for novm in [false, true] {
            let mut i = super::Interp::new();
            i.vm_off = novm;
            let prog = crate::js::parse(src, false).expect("parses");
            let got = match i.run_program(&prog) {
                Ok(v) => i.to_string(&v).map(|s| s.to_string())
                          .unwrap_or_else(|_| alloc::string::String::from("?")),
                Err(_) => alloc::string::String::from("THROW"),
            };
            out.push(got);
        }
        assert_eq!(out[0], out[1], "engines disagree on {src}");
        out.pop().unwrap()
    }

    /// An object returned by the parent constructor becomes `this` of the
    /// derived constructor, also when its body has no `return`.
    #[test]
    fn super_result_replaces_this() {
        assert_eq!(on_both("function B(){ return {x:1} } \
                            class D extends B { constructor(){ super(); this.y = 2 } } \
                            JSON.stringify(new D())"), r#"{"x":1,"y":2}"#);
        // `super()` calls the constructor's prototype, not `parent.prototype.constructor`.
        assert_eq!(on_both("class A { constructor(){ this.a = 1 } } class D extends A {} \
                            Object.setPrototypeOf(D, function O(){ this.o = 1 }); \
                            JSON.stringify(new D())"), r#"{"o":1}"#);
    }

    /// A getter reached through `super` runs with the current `this`, not
    /// the parent prototype; object literal methods have a home object too.
    #[test]
    fn super_property_reads_keep_the_receiver() {
        assert_eq!(on_both("class A { constructor(){ this.v = 7 } get g(){ return this.v } } \
                            class B extends A { get g(){ return super.g + 1 } get h(){ const e = super.g; return e } } \
                            const b = new B(); [b.g, b.h].join()"), "8,7");
        assert_eq!(on_both("const o = { __proto__: { get q(){ return this.w }, m(){ return 'p' + this.w } }, \
                            w: 5, get q2(){ return super.q }, m(){ return super.m() + '!' }, ['k'](){ return super.m() } }; \
                            [o.q2, o.m(), o.k()].join()"), "5,p5!,p5");
        assert_eq!(on_both("Reflect.get({ get x(){ return this.y } }, 'x', { y: 42 })"), "42");
    }

    /// `Reflect.construct` with a builtin takes the prototype from the new target.
    #[test]
    fn reflect_construct_native_uses_new_target() {
        assert_eq!(on_both("function S(){} \
                            var e = Reflect.construct(Error, ['m'], S), a = Reflect.construct(Array, [2], S); \
                            [e instanceof S, e.message, a instanceof S, a.length].join()"),
                   "true,m,true,2");
    }

    /// Only an anonymous function definition takes the binding's name.
    #[test]
    fn names_come_from_syntax_not_values() {
        assert_eq!(on_both("var p = {name: ''}; p.name = 'ok'; var q; q = {name: ''}; q.name = 'k'; \
                            var o = {z: p}; var f = (0, function(){}); var g = (function(){}); \
                            var h; h = () => 0; var y; [y] = [function(){}]; \
                            var m = {a: function(){}, ['c' + 1]: function(){}, n(){}}; \
                            [p.name, q.name, JSON.stringify(f.name), g.name, h.name, \
                             JSON.stringify(y.name), m.a.name, m.c1.name, m.n.name].join()"),
                   r#"ok,k,"",g,h,"",a,c1,n"#);
    }

    /// `ToIndex` truncates; only a negative or too large value is a RangeError.
    #[test]
    fn typed_array_lengths_use_to_index() {
        assert_eq!(on_both("[new Uint8Array(2.5).length, new Float64Array(1.9).length, \
                            new ArrayBuffer(3.7).byteLength, new DataView(new ArrayBuffer(4), 1.5).byteLength, \
                            new DataView(new ArrayBuffer(4)).getUint8(0.9)].join()"), "2,1,3,3,0");
        assert_eq!(on_both("try { new Uint8Array(-1); 'no' } catch (e) { e.name }"), "RangeError");
    }

    #[test]
    fn error_cause() {
        assert_eq!(on_both("var e = new Error('m', {cause: 5}); \
                            [e.cause, 'cause' in new Error('m'), 'cause' in new Error('m', {}), \
                             new TypeError('t', {cause: 0}).cause, e.propertyIsEnumerable('cause')].join()"),
                   "5,false,false,0,false");
    }

    /// `\p{..}` matches from the Unicode tables and rejects what the spec rejects.
    #[test]
    fn regexp_property_escapes() {
        assert_eq!(on_both(r#"[/^\p{Lu}+$/u.test("AÄΩ"), /\p{Lu}/u.test("a"), /^\p{sc=Grek}+$/u.test("αβ"),
                            /^\p{scx=Grek}$/u.test("͂"), /^[\p{N}\p{P}]+$/u.test("1,2."),
                            /^\P{L}$/u.test("1"), /^\p{Any}$/u.test("\u{10FFFF}"),
                            /\p{L}/.test("p{L}")].join()"#),
                   "true,false,true,true,true,true,true,true");
        assert_eq!(on_both(r#"["\\p{ascii}", "\\p{Script}", "\\p{Latin}", "\\p{ASCII=Y}", "[\\p{L}-z]"]
                            .map(function (b) { try { new RegExp(b, "u"); return "ok" } catch (e) { return e.name } })
                            .join()"#),
                   "SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError");
    }

    /// Class set notation under `v`: difference, intersection, nested classes, strings.
    #[test]
    fn regexp_v_flag_sets() {
        assert_eq!(on_both(r#"[/^[\d--[3-5]]+$/v.test("0129"), /^[\d--[3-5]]$/v.test("4"),
                            /^[\p{L}&&\p{ASCII}]+$/v.test("aZ"), /^[\p{L}&&\p{ASCII}]$/v.test("é"),
                            /^[\q{abc|d}x]+$/v.test("abcxd"), /x/v.flags, /x/v.unicode].join()"#),
                   "true,false,true,false,true,v,false");
        assert_eq!(on_both(r#"["[a-z--[aeiou]]", "[^\\q{ab}]", "[a&&&b]"]
                            .map(function (b) { try { new RegExp(b, "v"); return "ok" } catch (e) { return e.name } })
                            .join()"#),
                   "SyntaxError,SyntaxError,SyntaxError");
    }

    /// A quantifier over one character does not recurse per iteration.
    #[test]
    fn long_single_char_repeat() {
        assert_eq!(on_both(r#"var s = "a".repeat(300000) + "b"; [/^a+b$/.test(s), /^[^x]*$/.test(s),
                            /a*?b/.exec("aab")[0]].join()"#), "true,true,aab");
    }

    #[test]
    fn structured_clone_keeps_shape() {
        assert_eq!(on_both(r#"var o = {a: [1, {b: 2}], d: new Date(5), m: new Map([[1, 2]]), s: new Set(["x"]),
                            t: new Uint8Array([1, 2]), e: new RangeError("r")}; o.me = o;
                            var c = structuredClone(o);
                            [c !== o, c.me === c, c.a[1].b, c.d.getTime(), c.m.get(1), c.s.has("x"),
                             c.t.join("-"), c.e instanceof RangeError, c.e.message].join()"#),
                   "true,true,2,5,2,true,1-2,true,r");
        assert_eq!(on_both("try { structuredClone({f: function(){}}); 'no' } catch (e) { e.name }"),
                   "DataCloneError");
    }

    #[test]
    fn intl_plural_relative_segmenter() {
        assert_eq!(on_both(r#"var o = new Intl.PluralRules("en", {type: "ordinal"});
                            [new Intl.PluralRules("de").select(1), o.select(2), o.select(13), o.select(23),
                             new Intl.RelativeTimeFormat("en").format(-3, "day"),
                             new Intl.RelativeTimeFormat("de", {numeric: "auto"}).format(1, "day"),
                             new Intl.RelativeTimeFormat("de").format(2, "years")].join()"#),
                   "one,two,other,few,3 days ago,morgen,in 2 Jahren");
        assert_eq!(on_both(r#"var g = new Intl.Segmenter("en"), w = new Intl.Segmenter("en", {granularity: "word"});
                            [Array.from(g.segment("é🇩🇪!"), function (x) { return x.index }).join(" "),
                             Array.from(w.segment("It's 3.14, ok")).filter(function (x) { return x.isWordLike })
                                  .map(function (x) { return x.segment }).join("|")].join()"#),
                   "0 2 4,It's|3.14|ok");
    }

    /// Runs on both engines, requires agreement and that the VM declined no
    /// function body.
    fn compiled_on_both(src: &str) -> alloc::string::String {
        let mut out = alloc::vec::Vec::new();
        for novm in [false, true] {
            let mut i = super::Interp::new();
            i.vm_off = novm;
            let prog = crate::js::parse(src, false).expect("parses");
            let got = match i.run_program(&prog) {
                Ok(v) => i.to_string(&v).map(|s| s.to_string())
                          .unwrap_or_else(|_| alloc::string::String::from("?")),
                Err(super::Abrupt::Throw(v)) => {
                    let m = i.to_string(&v).map(|s| s.to_string()).unwrap_or_default();
                    alloc::format!("THROW {m}")
                }
                Err(_) => alloc::string::String::from("ABRUPT"),
            };
            if !novm {
                assert!(i.func_declines.is_empty() && i.vm_declined == 0,
                        "declined {:?} in {src}", i.func_declines);
            }
            out.push(got);
        }
        assert_eq!(out[0], out[1], "engines disagree on {src}");
        out.pop().unwrap()
    }

    #[test]
    fn finally_runs_on_return_break_continue() {
        let cases: &[(&str, &str)] = &[
            ("function f(){ var l=[]; function g(){ try { l.push(1); return 'r' } finally { l.push(2) } } \
              return g()+l.join() } f()", "r1,2"),
            // Nested: innermost finalizer first, the value survives both.
            ("function f(){ var l=[]; function g(){ try { try { return 'v' } finally { l.push('a') } } \
              finally { l.push('b') } } return g()+l.join() } f()", "va,b"),
            // An abrupt finalizer overrides the pending completion.
            ("function f(){ try { return 1 } finally { return 2 } } f()", "2"),
            ("function f(){ try { throw 1 } finally { return 3 } } f()", "3"),
            ("function f(){ try { return 1 } finally { throw 'x' } } try { f() } catch(e) { 'c'+e }", "cx"),
            ("function f(){ for(;;){ try { return 1 } finally { break } } return 'b' } f()", "b"),
            ("function f(){ var n=0; for(var i=0;i<3;i++){ try { continue } finally { n+=10 } } return n } f()", "30"),
            // Labeled jumps across two finalizers, inner `for…of` closed first.
            ("function f(){ var l=[]; var it={[Symbol.iterator](){ return { next(){ return {value:1,done:false} }, \
              return(){ l.push('close'); return {} } } }}; \
              outer: for(var k=0;k<2;k++){ try { for (var x of it) { try { continue outer } finally { l.push('in') } } } \
              finally { l.push('out') } } return l.join() } f()", "in,close,out,in,close,out"),
            ("function f(){ var l=[]; a: { try { break a } finally { l.push('f') } l.push('no') } return l.join() } f()", "f"),
            // `catch` with a jump and a finalizer.
            ("function f(){ var l=[]; for(;;){ try { throw 1 } catch(e) { l.push('c'); break } finally { l.push('f') } } \
              return l.join() } f()", "c,f"),
            ("function f(){ try { throw 1 } catch(e) { return 'c' } finally { } } f()", "c"),
            // The return value is evaluated before the finalizer runs.
            ("function f(){ var x=1; try { return x } finally { x=2 } } f()", "1"),
            // A throw from the finalizer of a return goes to the outer catch.
            ("function f(){ try { try { return 1 } finally { throw 'e' } } catch(e) { return 'caught '+e } } f()", "caught e"),
            // A break out of a try without finally leaves no handler behind.
            ("function t(){ for(;;){ try { break } catch(e){ return 'stale' } } throw 'x' } \
              try { t() } catch(e) { 'ok '+e }", "ok x"),
            // Program level.
            ("var l=[]; for(var i=0;i<2;i++){ try { if(i) break; continue } finally { l.push(i) } } l.join()", "0,1"),
        ];
        for (src, want) in cases {
            assert_eq!(&compiled_on_both(src), want, "{src}");
        }
    }

    #[test]
    fn generator_finally_with_yield_and_return() {
        let cases: &[(&str, &str)] = &[
            // `gen.return()` runs the finalizer, which may yield again.
            ("function* g(){ try { yield 1 } finally { yield 2 } } var it=g(); \
              var a=it.next(), b=it.return(5), c=it.next(), d=it.next(); \
              [a.value,a.done,b.value,b.done,c.value,c.done,d.done].join()",
             "1,false,2,false,5,true,true"),
            ("var l=[]; function* g(){ try { try { yield 1 } finally { l.push('a') } } finally { l.push('b') } } \
              var it=g(); it.next(); var r=it.return(7); l.join()+'|'+r.value+r.done", "a,b|7true"),
            // A return in the finalizer overrides `gen.return(v)`.
            ("function* g(){ try { yield 1 } finally { return 9 } } var it=g(); it.next(); \
              var r=it.return(5); r.value+''+r.done", "9true"),
            // `return` in the body with a yield in the finalizer.
            ("function* g(){ try { return 'r' } finally { yield 'f' } } var it=g(); \
              var a=it.next(), b=it.next(); a.value+a.done+b.value+b.done", "ffalsertrue"),
            // `gen.return()` inside `catch`.
            ("var l=[]; function* g(){ try { throw 1 } catch(e) { yield 'c' } finally { l.push('f') } } \
              var it=g(); it.next(); it.return(0); l.join()", "f"),
            // `gen.throw()` at a yield inside the finalizer.
            ("function* g(){ try { yield 1 } finally { yield 2 } } var it=g(); it.next(); it.return(0); \
              try { it.throw('t') } catch(e) { 'got '+e }", "got t"),
        ];
        for (src, want) in cases {
            let mut i = super::Interp::new();
            let prog = crate::js::parse(src, false).expect("parses");
            let v = i.run_program(&prog).ok().expect("runs");
            assert!(i.func_declines.is_empty(), "declined {:?} in {src}", i.func_declines);
            assert_eq!(&*i.to_string(&v).ok().unwrap(), *want, "{src}");
        }
    }

    #[test]
    fn async_finally_with_await_and_jumps() {
        let cases: &[(&str, &str)] = &[
            ("async function f(){ try { await null; return 'r' } finally { await null; out.push('f') } } \
              f().then(v=>out.push(v))", "f|r"),
            ("async function f(){ for(let i=0;i<3;i++){ try { await null; if(i==1) continue; out.push(i) } \
              finally { out.push('f'+i) } } } f()", "0|f0|f1|2|f2"),
            ("async function f(){ try { await Promise.reject('e') } catch(e) { return 'c'+e } finally { out.push('f') } } \
              f().then(v=>out.push(v))", "f|ce"),
            // An async generator's `return()` awaits its value, then runs the finalizer.
            ("async function* g(){ try { yield 1 } finally { out.push('fin') } } \
              (async()=>{ const h=g(); await h.next(); const r=await h.return(Promise.resolve(4)); \
              out.push(r.value+''+r.done) })()", "fin|4true"),
            ("async function* g(){ try { yield 1 } finally { yield 2 } } \
              (async()=>{ const h=g(); await h.next(); const a=await h.return(5); const b=await h.next(); \
              out.push(a.value+''+a.done, b.value+''+b.done) })()", "2false|5true"),
        ];
        for (src, want) in cases {
            assert_eq!(&async_out(src), want, "{src}");
        }
    }

    #[test]
    fn super_call_with_spread() {
        assert_eq!(compiled_on_both("class A { constructor(...a){ this.s=a.join() } } \
                                     class B extends A { constructor(...a){ super(0, ...a, 9) } } new B(1,2).s"),
                   "0,1,2,9");
        assert_eq!(compiled_on_both("class B extends Array { constructor(){ super(...[1,2,3]) } } \
                                     var b=new B(); b.length+''+(b instanceof B)"), "3true");
    }

    #[test]
    fn for_let_gives_each_iteration_its_binding() {
        let cases: &[(&str, &str)] = &[
            ("var fs=[]; for (let i=0;i<3;i++) fs.push(()=>i); fs.map(f=>f()).join()", "0,1,2"),
            ("var fs=[]; for (let i=0;i<3;i++) { fs.push({get: ()=>i}); } fs.map(o=>o.get()).join()", "0,1,2"),
            ("var fs=[]; for (let i=0;i<4;i++) { if (i%2) continue; try { fs.push(()=>i) } catch(e){} } \
              fs.map(f=>f()).join()", "0,2"),
            // A closure may change its own iteration's copy; the next one starts from it.
            ("var fs=[]; for (let i=0;i<6;i++) { fs.push(()=>i); fs[fs.length-1]; if (i==1) { i=3 } } \
              fs.length+':'+fs.map(f=>f()).join()", "4:0,3,4,5"),
            ("function f(){ var fs=[]; for (let i=0, j=10; i<2; i++, j--) fs.push(()=>i+'/'+j); \
              return fs.map(f=>f()).join() } f()", "0/10,1/9"),
        ];
        for (src, want) in cases {
            assert_eq!(&compiled_on_both(src), want, "{src}");
        }
    }
}
