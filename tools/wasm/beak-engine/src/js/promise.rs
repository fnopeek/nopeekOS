//! `Promise` and the microtask queue.
//!
//! Promises need no suspendable evaluator, only a queue drained between
//! tasks; generators and `await` are what need suspension.
//!
//! `NativeFn` is a plain function pointer and cannot see its own function
//! object, so `resolve`/`reject` carry their state as a bound first argument
//! (`ObjKind::Bound`). Both share one box; whichever runs first closes it.

use alloc::rc::Rc;
use alloc::vec::Vec;
use alloc::vec;
use core::cell::RefCell;

use super::interp::*;
use super::value::*;

/// The box shared by `resolve` and `reject`.
const CAP_PROMISE: &str = "\0!cap.p";
const CAP_DONE: &str = "\0!cap.done";
/// Counter box for `all`/`allSettled`/`any`.
const AGG_LEFT: &str = "\0!agg.left";
const AGG_VALUES: &str = "\0!agg.values";
const AGG_CAP: &str = "\0!agg.cap";
const AGG_INDEX: &str = "\0!agg.i";
const AGG_MODE: &str = "\0!agg.mode";

pub enum PState { Pending, Fulfilled(Value), Rejected(Value) }

/// An attached handler: the function (or none, meaning pass-through) and the
/// derived promise it settles.
///
/// `cap` is the foreign case: `then` may build its result via
/// `constructor[Symbol.species]`, and then the result is an arbitrary object
/// with its own `resolve`/`reject` pair. `derived` stays set as a placeholder.
pub struct Reaction {
    pub handler: Option<Value>,
    pub derived: Gc,
    pub cap: Option<(Value, Value)>,
}

pub struct PData {
    pub state: PState,
    pub on_ok: Vec<Reaction>,
    pub on_err: Vec<Reaction>,
    /// Has anyone ever attached `then`? Set by `PerformPromiseThen` whether or
    /// not a rejection handler was given; the derived promise takes over
    /// responsibility.
    pub handled: bool,
}

/// A microtask.
pub enum Job {
    /// Run a handler against a settled state.
    React { r: Reaction, arg: Value, rejected: bool },
    /// Adopt a foreign thenable: `then.call(thenable, res, rej)`.
    Adopt { thenable: Value, then: Value, target: Gc },
}

fn pdata(o: &Gc) -> Option<Rc<RefCell<PData>>> {
    match &o.borrow().kind { ObjKind::Promise(d) => Some(d.clone()), _ => None }
}

pub fn new_promise(i: &Interp) -> Gc {
    new_kind(Some(i.realm.promise_proto.clone()), ObjKind::Promise(Rc::new(RefCell::new(
        PData { state: PState::Pending, on_ok: Vec::new(), on_err: Vec::new(), handled: false }))))
}

/// Settle. Only once; a settled promise never changes again.
pub fn settle(i: &mut Interp, p: &Gc, v: Value, rejected: bool) {
    let Some(d) = pdata(p) else { return };
    let (ok, err) = {
        let mut b = d.borrow_mut();
        if !matches!(b.state, PState::Pending) { return; }
        b.state = if rejected { PState::Rejected(v.clone()) } else { PState::Fulfilled(v.clone()) };
        (core::mem::take(&mut b.on_ok), core::mem::take(&mut b.on_err))
    };
    let handled = d.borrow().handled;
    for r in if rejected { err } else { ok } {
        i.jobs.push_back(Job::React { r, arg: v.clone(), rejected });
    }
    // An unhandled rejection fails silently. Remember it and report it when the
    // queue is drained; until then a `catch` may still be attached, which is the
    // normal case.
    if rejected && !handled { i.pending_rejections.push(p.clone()); }
}

/// `ResolvePromise`: a thenable is adopted, anything else fulfils.
pub fn resolve_promise(i: &mut Interp, p: &Gc, v: Value) {
    if let Value::Obj(o) = &v {
        if Rc::ptr_eq(o, p) {
            let e = i.throw_kind("TypeError", "a promise cannot resolve with itself");
            if let Abrupt::Throw(ev) = e { settle(i, p, ev, true); }
            return;
        }
        // Reading `then` may throw (a getter); that error is the rejection reason.
        let then = match i.get(&v, "then") {
            Ok(t) => t,
            Err(Abrupt::Throw(ev)) => { settle(i, p, ev, true); return }
            Err(_) => return,
        };
        if i.is_callable(&then) {
            i.jobs.push_back(Job::Adopt { thenable: v.clone(), then, target: p.clone() });
            return;
        }
    }
    settle(i, p, v, false);
}

/// The `(resolve, reject)` pair for a promise. Both share one box; the first
/// call closes it for the other.
pub fn resolving_functions(i: &mut Interp, p: &Gc) -> (Value, Value) {
    let cap = new_obj(None);
    {
        let mut b = cap.borrow_mut();
        b.define(CAP_PROMISE, Prop::frozen(Value::Obj(p.clone())));
        b.define(CAP_DONE, Prop::data(Value::Bool(false)));
    }
    let c = Value::Obj(cap);
    let res = bind1(i, |i, _, a| { cap_settle(i, a, false); Ok(Value::Undefined) }, c.clone());
    let rej = bind1(i, |i, _, a| { cap_settle(i, a, true); Ok(Value::Undefined) }, c);
    (res, rej)
}

fn cap_settle(i: &mut Interp, a: &[Value], rejected: bool) {
    let Some(Value::Obj(cap)) = a.first() else { return };
    if cap.borrow().get_own(CAP_DONE).and_then(|p| p.value.clone())
          .map(|v| v.truthy()).unwrap_or(false) { return; }
    cap.borrow_mut().define(CAP_DONE, Prop::data(Value::Bool(true)));
    let Some(Value::Obj(p)) = cap.borrow().get_own(CAP_PROMISE).and_then(|p| p.value.clone())
        else { return };
    let v = a.get(1).cloned().unwrap_or(Value::Undefined);
    if rejected { settle(i, &p, v, true) } else { resolve_promise(i, &p, v) }
}

/// `PerformPromiseThen`: attach and return the derived promise.
pub fn perform_then(i: &mut Interp, p: &Gc, on_ok: Value, on_err: Value) -> Gc {
    perform_then_cap(i, p, on_ok, on_err, None)
}

/// Like `perform_then`, but with a foreign resolving pair.
pub fn perform_then_cap(i: &mut Interp, p: &Gc, on_ok: Value, on_err: Value,
                        cap: Option<(Value, Value)>) -> Gc {
    if let Some(d) = pdata(p) { d.borrow_mut().handled = true; }
    let derived = new_promise(i);
    let ok = if i.is_callable(&on_ok) { Some(on_ok) } else { None };
    let err = if i.is_callable(&on_err) { Some(on_err) } else { None };
    let Some(d) = pdata(p) else { return derived };
    let queued = {
        let mut b = d.borrow_mut();
        match &b.state {
            PState::Pending => {
                b.on_ok.push(Reaction { handler: ok, derived: derived.clone(), cap: cap.clone() });
                b.on_err.push(Reaction { handler: err, derived: derived.clone(), cap: cap.clone() });
                None
            }
            PState::Fulfilled(v) => Some((
                Reaction { handler: ok, derived: derived.clone(), cap: cap.clone() },
                v.clone(), false)),
            PState::Rejected(v) => Some((
                Reaction { handler: err, derived: derived.clone(), cap: cap.clone() },
                v.clone(), true)),
        }
    };
    if let Some((r, arg, rejected)) = queued {
        i.jobs.push_back(Job::React { r, arg, rejected });
    }
    derived
}

/// Drain the queue.
///
/// Capped: a chain that re-enqueues itself (`p.then(f)` inside `f`) is a
/// common pattern and would otherwise never end. Running once is too little
/// (a chain needs its steps), unbounded is too much; same choice as
/// `run_timers`.
pub fn run_jobs(i: &mut Interp) -> usize {
    // Only with no script on the stack; the outermost checkpoint drains.
    if i.script_depth > 0 { return 0 }
    let mut n = 0;
    // Observer checkpoint. It belongs here, not in `run_timers`: `run_jobs` runs
    // after every entry point (script, event, timer), so a page without timers
    // still gets its observer callbacks.
    //
    // Looped because a callback may mutate the tree again. The loop is bounded by
    // the step and time caps, not here.
    loop {
        // Deliver mutation records before running the queue: they were queued while
        // the script ran, i.e. before any `.then` registered afterwards.
        let zugestellt = super::dombind::deliver_mutations(i)
            | super::dombind::deliver_box_observers(i);
        i.script_depth += 1;
        let gefahren = run_queue(i);
        i.script_depth -= 1;
        n += gefahren;
        if !zugestellt && gefahren == 0 { break }
        if i.tick().is_err() { break }
    }
    report_rejections(i);
    n
}

/// Rejections still unhandled at the end of the queue are reported as
/// `unhandledrejection` on the window and on the console.
fn report_rejections(i: &mut Interp) {
    if i.pending_rejections.is_empty() { return }
    let list = core::mem::take(&mut i.pending_rejections);
    for p in list {
        let Some(d) = pdata(&p) else { continue };
        // The borrow must end here: the handler below may touch the promise.
        let reason = {
            let b = d.borrow();
            if b.handled { continue }
            match &b.state { PState::Rejected(v) => v.clone(), _ => continue }
        };
        // The handler may still answer: `preventDefault` suppresses the report, as in
        // browsers.
        let prevented = super::dombind::dispatch_rejection(i, reason.clone(), Value::Obj(p.clone()))
            .unwrap_or(false);
        if prevented { continue }
        let text = i.get(&reason, "message").ok()
            .and_then(|m| i.to_string(&m).ok())
            .filter(|m| !m.is_empty())
            .or_else(|| i.to_string(&reason).ok())
            .map(|s| alloc::string::String::from(&*s))
            .unwrap_or_else(|| "?".into());
        i.console_push(alloc::format!("Unhandled promise rejection: {text}"));
    }
}

fn run_queue(i: &mut Interp) -> usize {
    let mut n = 0;
    while let Some(j) = i.jobs.pop_front() {
        n += 1;
        if n > MAX_JOBS { break; }
        if i.tick().is_err() { break; }
        match j {
            Job::React { r, arg, rejected } => {
                // A foreign resolving pair gets the outcome as a call, not as a state change.
                let finish = |i: &mut Interp, r: &Reaction, v: Value, rejected: bool| {
                    match &r.cap {
                        Some((res, rej)) => {
                            let f = if rejected { rej.clone() } else { res.clone() };
                            let _ = i.call(&f, Value::Undefined, &[v]);
                        }
                        None => {
                            if rejected { settle(i, &r.derived, v, true) }
                            else { resolve_promise(i, &r.derived, v) }
                        }
                    }
                };
                match r.handler.clone() {
                    None => finish(i, &r, arg, rejected),
                    Some(h) => match i.call(&h, Value::Undefined, &[arg]) {
                        Ok(v) => finish(i, &r, v, false),
                        Err(Abrupt::Throw(e)) => finish(i, &r, e, true),
                        Err(_) => {}
                    },
                }
            }
            Job::Adopt { thenable, then, target } => {
                let (res, rej) = resolving_functions(i, &target);
                if let Err(Abrupt::Throw(e)) = i.call(&then, thenable, &[res, rej]) {
                    settle(i, &target, e, true);
                }
            }
        }
    }
    n
}

/// A native function with one bound first argument.
///
/// Stands in for a closure: `NativeFn` is a function pointer and cannot see
/// its own function object, but `ObjKind::Bound` prepends bound arguments.
/// `generator.rs` uses it for the two handlers that resume an `await`.
pub fn bind1(i: &mut Interp, f: NativeFn, arg: Value) -> Value {
    let target = native(Some(i.realm.function_proto.clone()), f, "", 1, false);
    Value::Obj(new_kind(Some(i.realm.function_proto.clone()), ObjKind::Bound {
        target, this_val: Value::Undefined, args: vec![arg] }))
}

/// The `finally` handler: call, wait for its result, then pass on the
/// original outcome unchanged.
fn finally_step(i: &mut Interp, a: &[Value], rejected: bool) -> C<Value> {
    let h = a.first().cloned().unwrap_or(Value::Undefined);
    let outcome = a.get(1).cloned().unwrap_or(Value::Undefined);
    let r = i.call(&h, Value::Undefined, &[])?;
    let waited = to_promise(i, &r);
    let thunk = bind1(i, if rejected {
        |_: &mut Interp, _: Value, a: &[Value]| Err(Abrupt::Throw(
            a.first().cloned().unwrap_or(Value::Undefined)))
    } else {
        |_: &mut Interp, _: Value, a: &[Value]| Ok(a.first().cloned().unwrap_or(Value::Undefined))
    }, outcome);
    Ok(Value::Obj(perform_then(i, &waited, thunk, Value::Undefined)))
}

/// `PromiseResolve`: a promise stays one, anything else becomes one.
pub fn to_promise(i: &mut Interp, v: &Value) -> Gc {
    if let Value::Obj(o) = v {
        if pdata(o).is_some() { return o.clone(); }
    }
    let p = new_promise(i);
    resolve_promise(i, &p, v.clone());
    p
}

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let proto = new_obj(Some(realm.object_proto.clone()));
    realm.promise_proto = proto.clone();

    let ctor = native(Some(fp.clone()), |i, _, a| {
        let ex = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&ex) { return i.type_err("Promise resolver is not a function"); }
        let p = new_promise(i);
        let (res, rej) = resolving_functions(i, &p);
        // The executor runs synchronously, not as a microtask; if it throws, the
        // promise is rejected instead of propagating the error.
        if let Err(Abrupt::Throw(e)) = i.call(&ex, Value::Undefined, &[res, rej]) {
            settle(i, &p, e, true);
        }
        Ok(Value::Obj(p))
    }, "Promise", 1, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
    proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("Promise")));

    let d = |o: &Gc, n: &str, f: NativeFn, l: usize, fp: &Gc| {
        let g = native(Some(fp.clone()), f, n, l, false);
        o.borrow_mut().define(n, Prop::builtin(Value::Obj(g)));
    };

    d(&proto, "then", |i, t, a| {
        let Value::Obj(p) = &t else { return i.type_err("then on a non-promise") };
        if pdata(p).is_none() { return i.type_err("then on a non-promise"); }
        let ok = a.first().cloned().unwrap_or(Value::Undefined);
        let err = a.get(1).cloned().unwrap_or(Value::Undefined);
        let p = p.clone();
        // `SpeciesConstructor` (ES §27.2.5.4 step 3): whoever sets
        // `p.constructor[Symbol.species]` decides what `then` returns, even if it is
        // not a promise. Polyfills such as core-js test exactly this expression and
        // replace the built-in `Promise` if it fails.
        let c = match species_of(i, &t)? {
            Some(c) => c,
            None => { let der = perform_then(i, &p, ok, err); return Ok(Value::Obj(der)) }
        };
        let (obj, res, rej) = new_capability(i, &c)?;
        perform_then_cap(i, &p, ok, err, Some((res, rej)));
        Ok(obj)
    }, 2, &fp);
    d(&proto, "catch", |i, t, a| {
        let f = i.get(&t, "then")?;
        let h = a.first().cloned().unwrap_or(Value::Undefined);
        i.call(&f, t.clone(), &[Value::Undefined, h])
    }, 1, &fp);
    // `finally` passes value and error through unchanged; it only observes them.
    // It also waits for what the handler returns, hence the extra promise, which
    // costs exactly the tick the spec prescribes.
    d(&proto, "finally", |i, t, a| {
        let h = a.first().cloned().unwrap_or(Value::Undefined);
        let f = i.get(&t, "then")?;
        if !i.is_callable(&h) { return i.call(&f, t.clone(), &[h.clone(), h]); }
        let pass = bind1(i, |i, _, a| finally_step(i, a, false), h.clone());
        let fail = bind1(i, |i, _, a| finally_step(i, a, true), h);
        i.call(&f, t.clone(), &[pass, fail])
    }, 1, &fp);

    d(&ctor, "resolve", |i, t, a| {
        if !matches!(t, Value::Obj(_)) { return i.type_err("Promise.resolve on a non-object"); }
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        Ok(Value::Obj(to_promise(i, &v)))
    }, 1, &fp);
    d(&ctor, "reject", |i, t, a| {
        if !i.is_constructor(&t) { return i.type_err("Promise.reject on a non-constructor"); }
        let p = new_promise(i);
        settle(i, &p, a.first().cloned().unwrap_or(Value::Undefined), true);
        Ok(Value::Obj(p))
    }, 1, &fp);

    // `all` = 0, `allSettled` = 1, `any` = 2. One implementation; they differ only
    // in what a single result does to the counter.
    d(&ctor, "all", |i, t, a| { agg_this(i, &t)?; aggregate(i, a, 0) }, 1, &fp);
    d(&ctor, "allSettled", |i, t, a| { agg_this(i, &t)?; aggregate(i, a, 1) }, 1, &fp);
    d(&ctor, "any", |i, t, a| { agg_this(i, &t)?; aggregate(i, a, 2) }, 1, &fp);
    d(&ctor, "race", |i, t, a| {
        agg_this(i, &t)?;
        let p = new_promise(i);
        let (res, rej) = resolving_functions(i, &p);
        let items = match i.iterate(a.first().unwrap_or(&Value::Undefined)) {
            Ok(v) => v,
            Err(Abrupt::Throw(e)) => { settle(i, &p, e, true); return Ok(Value::Obj(p)) }
            Err(e) => return Err(e),
        };
        for it in items {
            let ip = to_promise(i, &it);
            perform_then(i, &ip, res.clone(), rej.clone());
        }
        Ok(Value::Obj(p))
    }, 1, &fp);

    // `withResolvers` exposes the three pieces the constructor hides in the
    // executor.
    d(&ctor, "withResolvers", |i, t, _| {
        // Built via `NewPromiseCapability(this)`, so `this` must be a constructor even
        // though we always return our own promise.
        if !i.is_constructor(&t) { return i.type_err("withResolvers on a non-constructor"); }
        let p = new_promise(i);
        let (res, rej) = resolving_functions(i, &p);
        let o = new_obj(Some(i.realm.object_proto.clone()));
        o.borrow_mut().define("promise", Prop::data(Value::Obj(p)));
        o.borrow_mut().define("resolve", Prop::data(res));
        o.borrow_mut().define("reject", Prop::data(rej));
        Ok(Value::Obj(o))
    }, 0, &fp);
    // `Promise.try` turns a synchronous throw into a rejection.
    d(&ctor, "try", |i, t, a| {
        if !i.is_constructor(&t) { return i.type_err("Promise.try on a non-constructor"); }
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("Promise.try: argument is not a function"); }
        let p = new_promise(i);
        let rest: Vec<Value> = a.iter().skip(1).cloned().collect();
        match i.call(&f, Value::Undefined, &rest) {
            Ok(v) => resolve_promise(i, &p, v),
            Err(Abrupt::Throw(e)) => settle(i, &p, e, true),
            Err(e) => return Err(e),
        }
        Ok(Value::Obj(p))
    }, 1, &fp);

    realm.global.borrow_mut().define("Promise", Prop::builtin(Value::Obj(ctor)));
}

fn aggregate(i: &mut Interp, a: &[Value], mode: u8) -> C<Value> {
    let p = new_promise(i);
    let items = match i.iterate(a.first().unwrap_or(&Value::Undefined)) {
        Ok(v) => v,
        Err(Abrupt::Throw(e)) => { settle(i, &p, e, true); return Ok(Value::Obj(p)) }
        Err(e) => return Err(e),
    };
    let n = items.len();
    let values = i.new_array(vec![Value::Undefined; n]);
    let agg = new_obj(None);
    {
        let mut b = agg.borrow_mut();
        b.define(AGG_LEFT, Prop::data(Value::Num(n as f64)));
        b.define(AGG_VALUES, Prop::data(values.clone()));
        b.define(AGG_CAP, Prop::frozen(Value::Obj(p.clone())));
        b.define(AGG_MODE, Prop::frozen(Value::Num(mode as f64)));
    }
    if n == 0 {
        if mode == 2 {
            let e = i.throw_kind("AggregateError", "all promises were rejected");
            if let Abrupt::Throw(ev) = e { settle(i, &p, ev, true); }
        } else { settle(i, &p, values, false); }
        return Ok(Value::Obj(p));
    }
    for (k, it) in items.into_iter().enumerate() {
        let ip = to_promise(i, &it);
        let slot = new_obj(None);
        slot.borrow_mut().define(AGG_INDEX, Prop::frozen(Value::Num(k as f64)));
        slot.borrow_mut().proto = Some(agg.clone());
        let sv = Value::Obj(slot);
        let ok = bind1(i, |i, _, a| { agg_step(i, a, false); Ok(Value::Undefined) }, sv.clone());
        let err = bind1(i, |i, _, a| { agg_step(i, a, true); Ok(Value::Undefined) }, sv);
        perform_then(i, &ip, ok, err);
    }
    Ok(Value::Obj(p))
}

/// Book a single result onto the counter.
fn agg_step(i: &mut Interp, a: &[Value], rejected: bool) {
    let Some(Value::Obj(so)) = a.first().cloned() else { return };
    let slot = Value::Obj(so.clone());
    let v = a.get(1).cloned().unwrap_or(Value::Undefined);
    let Ok(mode) = i.get(&slot, AGG_MODE) else { return };
    let mode = i.to_number(&mode).unwrap_or(0.0) as u8;
    let Ok(Value::Obj(cap)) = i.get(&slot, AGG_CAP) else { return };
    let Ok(vals) = i.get(&slot, AGG_VALUES) else { return };
    let Ok(idx) = i.get(&slot, AGG_INDEX) else { return };
    let idx = i.to_number(&idx).unwrap_or(0.0);

    // `all` settles on the first rejection, `any` on the first fulfilment; the
    // counter only counts the other direction.
    if (mode == 0 && rejected) || (mode == 2 && !rejected) {
        settle(i, &cap, v, mode == 0);
        return;
    }
    let entry = if mode == 1 {
        let o = new_obj(Some(i.realm.object_proto.clone()));
        {
            let mut b = o.borrow_mut();
            if rejected {
                b.define("status", Prop::data(Value::str("rejected")));
                b.define("reason", Prop::data(v));
            } else {
                b.define("status", Prop::data(Value::str("fulfilled")));
                b.define("value", Prop::data(v));
            }
        }
        Value::Obj(o)
    } else { v };
    let _ = i.set(&vals, &num_to_string(idx), entry, false);

    // The counter lives on the shared prototype of the slots, not on the slot;
    // otherwise each would count alone.
    let Some(agg) = so.borrow().proto.clone() else { return };
    let left = agg.borrow().get_own(AGG_LEFT).and_then(|p| p.value.clone())
        .and_then(|v| match v { Value::Num(n) => Some(n), _ => None }).unwrap_or(0.0) - 1.0;
    agg.borrow_mut().define(AGG_LEFT, Prop::data(Value::Num(left)));
    if left > 0.0 { return; }
    if mode == 2 {
        let e = i.throw_kind("AggregateError", "all promises were rejected");
        if let Abrupt::Throw(ev) = e {
            let _ = i.set(&ev, "errors", vals, false);
            settle(i, &cap, ev, true);
        }
    } else {
        settle(i, &cap, vals, false);
    }
}

/// Cap for `run_jobs`; same rationale as the step limit.
pub const MAX_JOBS: usize = 100_000;


/// The aggregate statics build their result via `NewPromiseCapability(this)`;
/// on a non-constructor they throw before touching the iterator.
fn agg_this(i: &mut Interp, t: &Value) -> C<()> {
    if i.is_constructor(t) { Ok(()) } else { i.type_err("Promise static on a non-constructor") }
}

/// `SpeciesConstructor(p, %Promise%)`, but only when it is not our own.
/// `None` means the ordinary path.
fn species_of(i: &mut Interp, t: &Value) -> C<Option<Value>> {
    let ctor = i.get(t, "constructor")?;
    if matches!(ctor, Value::Undefined) { return Ok(None) }
    if !matches!(ctor, Value::Obj(_)) { return i.type_err("constructor is not an object") }
    let sp = i.get(&ctor, SYM_SPECIES)?;
    if matches!(sp, Value::Undefined | Value::Null) { return Ok(None) }
    // Our own `Promise` (or a `species` pointing to it) takes the short path;
    // otherwise every `then` would cost a constructor call.
    let own = i.realm.global.borrow().get_own("Promise").and_then(|p| p.value.clone());
    if matches!((&sp, &own), (Value::Obj(a), Some(Value::Obj(b))) if Rc::ptr_eq(a, b)) {
        return Ok(None);
    }
    if !i.is_callable(&sp) { return i.type_err("species is not a constructor") }
    Ok(Some(sp))
}

/// `NewPromiseCapability(C)`: `new C(executor)` and the two functions the
/// executor received.
///
/// The executor is a native collector that writes both arguments into a box
/// read afterwards. A foreign constructor may keep, call or discard them.
fn new_capability(i: &mut Interp, c: &Value) -> C<(Value, Value, Value)> {
    let box_ = new_obj(None);
    let ex = bind1(i, |i, _, a| {
        let Some(Value::Obj(b)) = a.first() else { return Ok(Value::Undefined) };
        b.borrow_mut().define(CAP_RES, Prop::data(a.get(1).cloned().unwrap_or(Value::Undefined)));
        b.borrow_mut().define(CAP_REJ, Prop::data(a.get(2).cloned().unwrap_or(Value::Undefined)));
        let _ = i;
        Ok(Value::Undefined)
    }, Value::Obj(box_.clone()));
    let obj = i.construct(c, &[ex])?;
    let res = box_.borrow().get_own(CAP_RES).and_then(|p| p.value.clone()).unwrap_or(Value::Undefined);
    let rej = box_.borrow().get_own(CAP_REJ).and_then(|p| p.value.clone()).unwrap_or(Value::Undefined);
    Ok((obj, res, rej))
}

const CAP_RES: &str = "\0!cap.res";
const CAP_REJ: &str = "\0!cap.rej";

#[cfg(test)]
mod tests {
    /// An `unhandledrejection` handler may use the promise it is told about.
    #[test]
    fn rejection_handler_may_touch_the_promise() {
        let dom = crate::dom::parse("<html><body></body></html>");
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::super::dombind::Doc::from_dom(&dom));
        let src = "addEventListener('unhandledrejection', function (e) { \
                   e.preventDefault(); e.promise.catch(function () {}); \
                   e.promise.then(null, function (r) { console.log('late ' + r) }); \
                   console.log('seen ' + e.reason) }); \
                   Promise.reject('x');";
        let prog = super::super::parse(src, false).expect("parses");
        assert!(i.run_program(&prog).is_ok());
        super::run_jobs(&mut i);
        assert_eq!(i.take_console(), ["seen x", "late x"]);
    }
}
