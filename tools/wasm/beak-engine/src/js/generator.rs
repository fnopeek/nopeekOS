//! Generators: the suspension the bytecode machine exists for.
//!
//! A generator is its own machine, not a frame inside another (see the head
//! of `vm.rs`). The state is a `Vm`, and the three methods are three ways to
//! resume it.
//!
//! A generator object is built in exactly one place (`make`, called from
//! `Interp::call_inner`), also when the call came from the bytecode machine:
//! it routes generator calls through `Interp::call` instead of pushing a frame.

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use core::cell::{Cell, RefCell};

use super::interp::{Interp, C};
use super::value::{Gc, ObjKind, Prop, Value, new_kind};
use super::vm::{Step, Vm};

/// A generator's lifecycle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Built, no instruction run yet. `next(v)` discards `v`; there is no
    /// `yield` yet to receive it.
    Start,
    /// Suspended at a `yield`.
    Suspended,
    /// Currently running. A `next()` from inside is an error, not a restart.
    Running,
    Done,
}

/// What a generator object holds.
///
/// `Cell`/`RefCell` and the machine as an `Option`, so that no borrow is held
/// while it runs: a generator calling its own `next()` would otherwise hit a
/// `borrow_mut` and halt the kernel. Take it out, run, put it back.
pub struct GenState {
    vm: RefCell<Option<Vm>>,
    status: Cell<Status>,
    /// The promise an async function settles at the end. `None` for a plain
    /// generator and for an async generator, which has one promise per request
    /// (`queue`) instead.
    ///
    /// All three share this state because a waiting async function is a
    /// suspended body; the only difference is who resumes it: `next()` or the
    /// microtask queue.
    promise: Option<Gc>,
    /// Pending requests of an async generator, in arrival order.
    ///
    /// `agen.next()` returns a promise immediately, even while the body waits at
    /// an `await`; three `next()` calls in a row must queue rather than resume
    /// the machine three times (ES 27.6.3.6). Plain generators do not need it.
    queue: RefCell<VecDeque<Req>>,
}

/// A pending request to an async generator.
struct Req {
    kind: ReqKind,
    value: Value,
    /// The promise `next()`/`throw()`/`return()` already returned, settled here.
    promise: Gc,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReqKind { Next, Throw, Return }

impl GenState {
    /// What the suspended machine holds; see `Vm::roots`.
    pub fn roots(&self, objs: &mut alloc::vec::Vec<Gc>,
                 envs: &mut alloc::vec::Vec<Rc<RefCell<super::interp::Env>>>) {
        if let Some(vm) = self.vm.borrow().as_ref() {
            vm.roots(objs, envs);
        }
        // The queue holds a promise and a value per request. Skipping it here would
        // leave an Rc cycle once a callback on the promise holds the generator.
        for r in self.queue.borrow().iter() {
            objs.push(r.promise.clone());
            if let Value::Obj(o) = &r.value { objs.push(o.clone()); }
        }
    }
}

/// A call to a generator function: builds the object and runs nothing.
///
/// `None` means the compiler cannot handle the body; the old path (the tree
/// walker reporting "generators are not supported") stays instead of
/// building half a generator.
///
/// Spec order: first the environment with parameters, `this` and
/// `arguments` (`call_env`), then hoisting, then `Get(f, "prototype")` for
/// the object's prototype.
pub fn make(i: &mut Interp, func: &Gc, d: &Rc<super::value::FuncData>,
            this_val: Value, args: &[Value]) -> C<Option<Value>> {
    let Some(chunk) = i.func_chunk(&d.node) else { return Ok(None) };
    let env = i.call_env(d, this_val, args)?;
    i.hoist_body(&d.node.body, &env)?;
    let proto = match i.get(&Value::Obj(func.clone()), "prototype")? {
        Value::Obj(p) => p,
        _ => i.realm.generator_proto.clone(),
    };
    let st = GenState {
        vm: RefCell::new(Some(Vm::for_generator(chunk, &env))),
        status: Cell::new(Status::Start),
        promise: None,
        queue: RefCell::new(VecDeque::new()),
    };
    Ok(Some(Value::Obj(new_kind(Some(proto), ObjKind::Generator(Rc::new(st))))))
}

// ── async/await ──────────────────────────────────────────────────────────
//
// The same suspended body, resumed by the microtask queue instead of
// `next()`. The state hangs on an object no script ever sees; it only
// carries the generator to the two handlers via `promise::bind1` (a
// `NativeFn` is a pointer and has no closure).

/// A call to an async function: returns a promise and runs the body up to
/// the first `await` synchronously, as the spec requires.
///
/// An error while binding parameters also becomes a rejection: an async
/// function never throws, it rejects.
pub fn make_async(i: &mut Interp, d: &Rc<super::value::FuncData>,
                  this_val: Value, args: &[Value]) -> C<Option<Value>> {
    let Some(chunk) = i.func_chunk(&d.node) else {
        // An uncompilable body still returns a promise. The tree walker runs it (and
        // fails with a TypeError at an `await`), but the call contract stays the
        // same: an async function never throws and never returns a bare value.
        let outer = super::promise::new_promise(i);
        match i.run_js_body(d, this_val, args) {
            Ok(v) => super::promise::resolve_promise(i, &outer, v),
            Err(super::interp::Abrupt::Throw(e)) => super::promise::settle(i, &outer, e, true),
            Err(e) => return Err(e),
        }
        return Ok(Some(Value::Obj(outer)));
    };
    let outer = super::promise::new_promise(i);
    let prepared = i.call_env(d, this_val, args)
        .and_then(|env| { i.hoist_body(&d.node.body, &env)?; Ok(env) });
    let env = match prepared {
        Ok(e) => e,
        Err(super::interp::Abrupt::Throw(e)) => {
            super::promise::settle(i, &outer, e, true);
            return Ok(Some(Value::Obj(outer)));
        }
        Err(e) => return Err(e),
    };
    let st = Rc::new(GenState {
        vm: RefCell::new(Some(Vm::for_generator(chunk, &env))),
        status: Cell::new(Status::Start),
        promise: Some(outer.clone()),
        queue: RefCell::new(VecDeque::new()),
    });
    let holder = Value::Obj(new_kind(None, ObjKind::Generator(st.clone())));
    pump(i, &st, Seed::Start, holder);
    Ok(Some(Value::Obj(outer)))
}

/// What the machine is resumed with.
enum Seed {
    /// First run; there is no `await` yet to take a value.
    Start,
    Value(Value),
    Throw(Value),
    /// A `return(v)` at a `yield*`: passed as a value to the inner iterator
    /// instead of abandoning the outer generator.
    Delegate(Value),
    /// A `return(v)` at a plain `yield` of an async generator: `v` is awaited
    /// first, then returned through the pending finalizers (ES 27.6.3.8).
    Return(Value),
}

/// Run an async function's machine until it waits or finishes, then settle
/// its promise.
fn pump(i: &mut Interp, st: &Rc<GenState>, seed: Seed, holder: Value) {
    let Some(outer) = st.promise.clone() else { return };
    let Some(mut vm) = st.vm.borrow_mut().take() else { return };
    st.status.set(Status::Running);
    let r = match seed {
        Seed::Start => vm.drive(i),
        Seed::Value(v) => { vm.send(v); vm.drive(i) }
        // If nobody in the body catches the throw, the function is done and its
        // promise rejected.
        Seed::Throw(e) => {
            if vm.at_delegate() { vm.send_throw(e); vm.drive(i) }
            else if vm.inject_throw(i, e.clone()) { vm.drive(i) }
            else { Err(super::interp::Abrupt::Throw(e)) }
        }
        Seed::Delegate(v) => { vm.send_return(v); vm.drive(i) }
        Seed::Return(v) => { vm.send(v); vm.drive(i) }
    };
    match r {
        Ok(Step::Await(v)) => {
            st.status.set(Status::Suspended);
            *st.vm.borrow_mut() = Some(vm);
            let p = super::promise::to_promise(i, &v);
            let ok = super::promise::bind1(i, |i, _, a| { wake(i, a, false); Ok(Value::Undefined) },
                                           holder.clone());
            let er = super::promise::bind1(i, |i, _, a| { wake(i, a, true); Ok(Value::Undefined) },
                                           holder);
            super::promise::perform_then(i, &p, ok, er);
        }
        Ok(Step::Done(v)) => {
            st.status.set(Status::Done);
            super::promise::resolve_promise(i, &outer, v);
        }
        // Cannot happen: async generators are served by `serve`, so no `yield`
        // reaches this path.
        Ok(Step::Yield(..)) => {
            st.status.set(Status::Done);
            vm.close(i);
        }
        Err(super::interp::Abrupt::Throw(e)) => {
            st.status.set(Status::Done);
            vm.close(i);
            super::promise::settle(i, &outer, e, true);
        }
        Err(_) => {
            st.status.set(Status::Done);
            vm.close(i);
        }
    }
}

/// The handler `await` attaches to the promise: `a[0]` is the bound carrier,
/// `a[1]` the resolution.
fn wake(i: &mut Interp, a: &[Value], rejected: bool) {
    let Some(holder) = a.first().cloned() else { return };
    let v = a.get(1).cloned().unwrap_or(Value::Undefined);
    let st = match &holder {
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::Generator(g) => g.clone(),
            _ => return,
        },
        _ => return,
    };
    pump(i, &st, if rejected { Seed::Throw(v) } else { Seed::Value(v) }, holder);
}

// ── async generators ─────────────────────────────────────────────────────
//
// Not the sum of both but their interleaving. An async generator suspends
// for two reasons: at `await` (the microtask queue resumes it) and at `yield`
// (`next()` resumes it). Both come from the same `Vm` and `Step`; the
// contract around them is:
//
//   * `next()` returns a promise immediately, even mid-`await`, so requests
//     queue (`Req`).
//   * `yield x` is `AsyncGeneratorYield(? Await(x))` (ES 15.5.5 / 27.6.3.8),
//     the value is awaited first. The compiler emits an `Op::Await` before
//     every `Op::Yield` in an async generator, so the machine stays simple.

/// A call to an async generator function: builds the object and runs
/// nothing, like a plain generator but with an empty request queue.
pub fn make_async_gen(i: &mut Interp, func: &Gc, d: &Rc<super::value::FuncData>,
                      this_val: Value, args: &[Value]) -> C<Option<Value>> {
    let Some(chunk) = i.func_chunk(&d.node) else { return Ok(None) };
    let env = i.call_env(d, this_val, args)?;
    i.hoist_body(&d.node.body, &env)?;
    let proto = match i.get(&Value::Obj(func.clone()), "prototype")? {
        Value::Obj(p) => p,
        _ => i.realm.async_gen_proto.clone(),
    };
    let st = GenState {
        vm: RefCell::new(Some(Vm::for_generator(chunk, &env))),
        status: Cell::new(Status::Start),
        promise: None,
        queue: RefCell::new(VecDeque::new()),
    };
    Ok(Some(Value::Obj(new_kind(Some(proto), ObjKind::Generator(Rc::new(st))))))
}

/// Queue a request and return the promise that will settle it
/// (ES 27.6.3.6 AsyncGeneratorEnqueue).
///
/// The result is always a promise, also on error: `next()` on something
/// that is not an async generator rejects instead of throwing, so the caller
/// gets a rejection it can handle.
fn enqueue(i: &mut Interp, t: &Value, kind: ReqKind, v: Value) -> Value {
    let p = super::promise::new_promise(i);
    let st = match t {
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::Generator(g) => Some(g.clone()),
            _ => None,
        },
        _ => None,
    };
    let Some(st) = st else {
        let e = match i.type_err::<Value>("not an async generator") {
            Err(super::interp::Abrupt::Throw(e)) => e,
            _ => Value::Undefined,
        };
        super::promise::settle(i, &p, e, true);
        return Value::Obj(p);
    };
    st.queue.borrow_mut().push_back(Req { kind, value: v, promise: p.clone() });
    // If the machine is running (or waiting at an `await`), it picks up the
    // request by itself once it is done there.
    if st.status.get() != Status::Running {
        let holder = t.clone();
        serve(i, &st, holder, None);
    }
    Value::Obj(p)
}

/// Settle the front request and take the next, until the queue is empty or
/// the machine waits (ES 27.6.3.5 AsyncGeneratorResumeNext).
///
/// `seed` is set when returning from an `await`; the machine then continues
/// instead of starting a new request.
fn serve(i: &mut Interp, st: &Rc<GenState>, holder: Value, mut seed: Option<Seed>) {
    loop {
        // A finished generator answers everything in the queue without touching the
        // machine.
        if st.status.get() == Status::Done {
            let Some(r) = st.queue.borrow_mut().pop_front() else { return };
            match r.kind {
                ReqKind::Throw => super::promise::settle(i, &r.promise, r.value, true),
                ReqKind::Return => {
                    let v = i.iter_result(r.value, true);
                    super::promise::resolve_promise(i, &r.promise, v);
                }
                ReqKind::Next => {
                    let v = i.iter_result(Value::Undefined, true);
                    super::promise::resolve_promise(i, &r.promise, v);
                }
            }
            continue;
        }
        // No `seed` means a new request begins.
        let cur = match seed.take() {
            Some(s) => s,
            None => {
                let (kind, value) = {
                    let q = st.queue.borrow();
                    let Some(r) = q.front() else { return };
                    (r.kind, r.value.clone())
                };
                match kind {
                    ReqKind::Next => {
                        // On the very first `next` there is no `yield` to take the value; same rule
                        // as in a plain generator.
                        if st.status.get() == Status::Start { Seed::Start } else { Seed::Value(value) }
                    }
                    ReqKind::Throw => Seed::Throw(value),
                    // At a `yield*` both go to the inner iterator instead of unwinding or
                    // abandoning the outer one.
                    ReqKind::Return if st.vm.borrow().as_ref()
                        .is_some_and(|vm| vm.at_delegate()) => Seed::Delegate(value),
                    ReqKind::Return if st.status.get() == Status::Suspended => Seed::Return(value),
                    // Not started: nothing to run. Not implemented: the spec awaits the
                    // value first (AsyncGeneratorAwaitReturn).
                    ReqKind::Return => {
                        if let Some(mut vm) = st.vm.borrow_mut().take() { vm.close(i); }
                        st.status.set(Status::Done);
                        continue;
                    }
                }
            }
        };
        let Some(mut vm) = st.vm.borrow_mut().take() else {
            st.status.set(Status::Done);
            continue;
        };
        st.status.set(Status::Running);
        let r = match cur {
            Seed::Start => vm.drive(i),
            Seed::Value(v) if core::mem::take(&mut vm.ret_await) => {
                if vm.inject_return(i, v.clone()) { vm.drive(i) }
                else { vm.close(i); Ok(Step::Done(v)) }
            }
            Seed::Value(v) => { vm.send(v); vm.drive(i) }
            Seed::Return(v) => { vm.ret_await = true; Ok(Step::Await(v)) }
            Seed::Throw(e) => {
                vm.ret_await = false;
                if vm.at_delegate() { vm.send_throw(e); vm.drive(i) }
                else if vm.inject_throw(i, e.clone()) { vm.drive(i) }
                else { Err(super::interp::Abrupt::Throw(e)) }
            }
            Seed::Delegate(v) => { vm.send_return(v); vm.drive(i) }
        };
        match r {
            // An `await` ends the round but not the request. The front request stays;
            // the machine stays `Running` so a concurrent `next()` queues instead of
            // resuming it.
            Ok(Step::Await(v)) => {
                *st.vm.borrow_mut() = Some(vm);
                let p = super::promise::to_promise(i, &v);
                let ok = super::promise::bind1(i, |i, _, a| { wake_gen(i, a, false); Ok(Value::Undefined) },
                                               holder.clone());
                let er = super::promise::bind1(i, |i, _, a| { wake_gen(i, a, true); Ok(Value::Undefined) },
                                               holder);
                super::promise::perform_then(i, &p, ok, er);
                return;
            }
            Ok(Step::Yield(v, _)) => {
                st.status.set(Status::Suspended);
                *st.vm.borrow_mut() = Some(vm);
                // An async generator always yields the value, also for `yield*`:
                // `AsyncGeneratorYield(? IteratorValue(…))`.
                let out = i.iter_result(v, false);
                if let Some(r) = st.queue.borrow_mut().pop_front() {
                    super::promise::resolve_promise(i, &r.promise, out);
                }
            }
            Ok(Step::Done(v)) => {
                st.status.set(Status::Done);
                let out = i.iter_result(v, true);
                if let Some(r) = st.queue.borrow_mut().pop_front() {
                    super::promise::resolve_promise(i, &r.promise, out);
                }
            }
            Err(super::interp::Abrupt::Throw(e)) => {
                st.status.set(Status::Done);
                vm.close(i);
                if let Some(r) = st.queue.borrow_mut().pop_front() {
                    super::promise::settle(i, &r.promise, e, true);
                }
            }
            Err(_) => {
                st.status.set(Status::Done);
                vm.close(i);
                return;
            }
        }
    }
}

/// The handler an `await` inside an async generator attaches.
fn wake_gen(i: &mut Interp, a: &[Value], rejected: bool) {
    let Some(holder) = a.first().cloned() else { return };
    let v = a.get(1).cloned().unwrap_or(Value::Undefined);
    let st = match &holder {
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::Generator(g) => g.clone(),
            _ => return,
        },
        _ => return,
    };
    let seed = if rejected { Seed::Throw(v) } else { Seed::Value(v) };
    serve(i, &st, holder, Some(seed));
}

/// `%AsyncIteratorPrototype%`, `%AsyncGeneratorPrototype%` and
/// `%AsyncGeneratorFunction.prototype%`.
///
/// The first carries only `[Symbol.asyncIterator]() { return this }`, which
/// is what lets `for await (x of agen())` find an iterator.
pub fn install_async(f_proto: &Gc) -> (Gc, Gc, Gc) {
    let async_iter = super::value::new_obj(None);
    let proto = super::value::new_obj(Some(async_iter.clone()));
    let fn_proto = super::value::new_obj(Some(f_proto.clone()));
    let def = |o: &Gc, key: &str, show: &str, f: super::value::NativeFn| {
        let g = super::value::native(Some(f_proto.clone()), f, show, 1, false);
        o.borrow_mut().define(key, Prop::builtin(Value::Obj(g)));
    };
    def(&async_iter, super::value::SYM_ASYNC_ITERATOR, "[Symbol.asyncIterator]",
        |_, t, _| Ok(t));
    def(&proto, "next", "next",
        |i, t, a| Ok(enqueue(i, &t, ReqKind::Next, a.first().cloned().unwrap_or(Value::Undefined))));
    def(&proto, "return", "return",
        |i, t, a| Ok(enqueue(i, &t, ReqKind::Return, a.first().cloned().unwrap_or(Value::Undefined))));
    def(&proto, "throw", "throw",
        |i, t, a| Ok(enqueue(i, &t, ReqKind::Throw, a.first().cloned().unwrap_or(Value::Undefined))));
    proto.borrow_mut().define(super::value::SYM_TO_STRING_TAG,
        Prop::frozen(Value::str("AsyncGenerator")));
    fn_proto.borrow_mut().define("prototype", Prop {
        value: Some(Value::Obj(proto.clone())), get: None, set: None,
        writable: false, enumerable: false, configurable: true });
    fn_proto.borrow_mut().define(super::value::SYM_TO_STRING_TAG,
        Prop::frozen(Value::str("AsyncGeneratorFunction")));
    proto.borrow_mut().define("constructor", Prop {
        value: Some(Value::Obj(fn_proto.clone())), get: None, set: None,
        writable: false, enumerable: false, configurable: true });
    (async_iter, proto, fn_proto)
}

/// The state behind `this`, or a TypeError if there is none. The borrow ends
/// here, before anything that runs afterwards.
fn state(i: &mut Interp, t: &Value) -> C<Rc<GenState>> {
    if let Value::Obj(o) = t {
        if let ObjKind::Generator(g) = &o.borrow().kind {
            return Ok(g.clone());
        }
    }
    i.type_err("not a generator")
}

/// Turn the result of `drive` into `{value, done}` and update the state. A
/// throw finishes the generator for good.
fn finish(i: &mut Interp, st: &Rc<GenState>, mut vm: Vm, r: C<Step>) -> C<Value> {
    match r {
        Ok(Step::Yield(v, raw)) => {
            st.status.set(Status::Suspended);
            *st.vm.borrow_mut() = Some(vm);
            // Raw means already a result object: `yield*` passes the inner iterator's
            // result through unchanged (ES 15.5.5, `GeneratorYield`). Wrapping it again
            // would give `{value: {value: 1, done: false}, done: false}`.
            Ok(if raw { v } else { i.iter_result(v, false) })
        }
        Ok(Step::Done(v)) => {
            st.status.set(Status::Done);
            Ok(i.iter_result(v, true))
        }
        // Cannot happen: `await` only appears in async bodies, which never reach
        // this path. Listed so that a `_ =>` does not hide the case.
        Ok(Step::Await(_)) => {
            st.status.set(Status::Done);
            vm.close(i);
            i.type_err("await in a plain generator")
        }
        Err(e) => {
            st.status.set(Status::Done);
            vm.close(i);
            Err(e)
        }
    }
}

/// Take the machine out if the state allows it.
///
/// `Ok(None)` means done, nothing left to do; a running generator gives a
/// TypeError because resuming it would double its stack.
fn take(i: &mut Interp, st: &Rc<GenState>) -> C<Option<Vm>> {
    match st.status.get() {
        Status::Running => i.type_err("generator is already running"),
        Status::Done => Ok(None),
        Status::Start | Status::Suspended => {
            let vm = st.vm.borrow_mut().take();
            if vm.is_none() { st.status.set(Status::Done); }
            Ok(vm)
        }
    }
}

pub fn next(i: &mut Interp, t: &Value, v: Value) -> C<Value> {
    let st = state(i, t)?;
    let started = st.status.get() == Status::Suspended;
    let Some(mut vm) = take(i, &st)? else { return Ok(i.iter_result(Value::Undefined, true)) };
    // On the very first `next` there is no `yield` to take the value; the spec
    // discards it.
    if started { vm.send(v); }
    st.status.set(Status::Running);
    let r = vm.drive(i);
    finish(i, &st, vm, r)
}

/// `gen.throw(v)`: inject the throw at the suspension point. If nothing
/// catches it there, the generator is done and the throw goes to the caller,
/// also when nothing has run yet.
pub fn throw(i: &mut Interp, t: &Value, v: Value) -> C<Value> {
    let st = state(i, t)?;
    let Some(mut vm) = take(i, &st)? else { return Err(super::interp::Abrupt::Throw(v)) };
    st.status.set(Status::Running);
    // At a `yield*` a throw does not unwind: it is passed to the inner iterator
    // (ES 15.5.5, 6.b). If that has no `throw`, the machine decides, not this
    // code.
    if vm.at_delegate() {
        vm.send_throw(v);
        let r = vm.drive(i);
        return finish(i, &st, vm, r);
    }
    if !vm.inject_throw(i, v.clone()) {
        st.status.set(Status::Done);
        vm.close(i);
        return Err(super::interp::Abrupt::Throw(v));
    }
    let r = vm.drive(i);
    finish(i, &st, vm, r)
}

/// `gen.return(v)`: a return completion at the suspension point. Pending
/// finalizers run (and may yield again); without any the machine is closed
/// (`Vm::close`), which also closes open `for…of` iterations.
pub fn ret(i: &mut Interp, t: &Value, v: Value) -> C<Value> {
    let st = state(i, t)?;
    let started = st.status.get() == Status::Suspended;
    let Some(mut vm) = take(i, &st)? else { return Ok(i.iter_result(v, true)) };
    // At a `yield*` the inner iterator gets its `return` first. It may run its
    // own cleanup and even refuse (its `return()` gives `done: false`); then the
    // outer generator continues and `finish` sees an ordinary `yield`.
    if vm.at_delegate() {
        st.status.set(Status::Running);
        vm.send_return(v);
        let r = vm.drive(i);
        return finish(i, &st, vm, r);
    }
    if started && vm.inject_return(i, v.clone()) {
        st.status.set(Status::Running);
        let r = vm.drive(i);
        return finish(i, &st, vm, r);
    }
    st.status.set(Status::Done);
    vm.close(i);
    Ok(i.iter_result(v, true))
}

/// The two prototypes of the generator protocol.
///
/// `%GeneratorPrototype%` sits under `%IteratorPrototype%`, which supplies
/// `[Symbol.iterator]() { return this }`; that is what makes `for (x of gen())`
/// and `[...gen()]` work.
pub fn install(i_proto: &Gc, f_proto: &Gc) -> (Gc, Gc) {
    let proto = super::value::new_obj(Some(i_proto.clone()));
    let fn_proto = super::value::new_obj(Some(f_proto.clone()));
    let def = |o: &Gc, name: &str, f: super::value::NativeFn| {
        let g = super::value::native(Some(f_proto.clone()), f, name, 1, false);
        o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
    };
    def(&proto, "next", |i, t, a| next(i, &t, a.first().cloned().unwrap_or(Value::Undefined)));
    def(&proto, "return", |i, t, a| ret(i, &t, a.first().cloned().unwrap_or(Value::Undefined)));
    def(&proto, "throw", |i, t, a| throw(i, &t, a.first().cloned().unwrap_or(Value::Undefined)));
    proto.borrow_mut().define(super::value::SYM_TO_STRING_TAG,
        Prop::frozen(Value::str("Generator")));
    fn_proto.borrow_mut().define("prototype", Prop {
        value: Some(Value::Obj(proto.clone())), get: None, set: None,
        writable: false, enumerable: false, configurable: true });
    fn_proto.borrow_mut().define(super::value::SYM_TO_STRING_TAG,
        Prop::frozen(Value::str("GeneratorFunction")));
    proto.borrow_mut().define("constructor", Prop {
        value: Some(Value::Obj(fn_proto.clone())), get: None, set: None,
        writable: false, enumerable: false, configurable: true });
    (proto, fn_proto)
}
