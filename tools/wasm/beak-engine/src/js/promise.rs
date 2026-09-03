//! `Promise` und die Microtask-Schlange.
//!
//! **Ohne Aufhaengen der Maschine.** Ein Versprechen braucht keine
//! Unterbrechung des Auswerters — nur eine Schlange, die zwischen den Aufgaben
//! abgearbeitet wird. Das ist der Grund, warum es VOR Generatoren und
//! `await` kommt: die brauchen einen anhaltbaren Auswerter, ein
//! `.then()`-Gespann nicht.
//!
//! **Kein Abschluss, sondern gebundene Argumente.** `NativeFn` ist ein
//! Funktionszeiger und bekommt das Funktionsobjekt nicht zu sehen — ein
//! `resolve`, das „sein" Versprechen kennt, ginge damit nicht. Also traegt es
//! den Zustand als GEBUNDENES erstes Argument (`ObjKind::Bound`), und das ist
//! zugleich der Ort fuer die Sperre „schon erledigt": `resolve` und `reject`
//! teilen sich einen Kasten, und wer zuerst kommt, schliesst ihn.

use alloc::rc::Rc;
use alloc::vec::Vec;
use alloc::vec;
use core::cell::RefCell;

use super::interp::*;
use super::value::*;

/// Der Kasten, den `resolve` und `reject` sich teilen.
const CAP_PROMISE: &str = "\0!cap.p";
const CAP_DONE: &str = "\0!cap.done";
/// Zaehlerkasten fuer `all`/`allSettled`/`any`.
const AGG_LEFT: &str = "\0!agg.left";
const AGG_VALUES: &str = "\0!agg.values";
const AGG_CAP: &str = "\0!agg.cap";
const AGG_INDEX: &str = "\0!agg.i";
const AGG_MODE: &str = "\0!agg.mode";

pub enum PState { Pending, Fulfilled(Value), Rejected(Value) }

/// Ein angehaengter Behandler: die Funktion (oder keine — dann wird
/// durchgereicht) und das abgeleitete Versprechen, das er erledigt.
pub struct Reaction { pub handler: Option<Value>, pub derived: Gc }

pub struct PData {
    pub state: PState,
    pub on_ok: Vec<Reaction>,
    pub on_err: Vec<Reaction>,
}

/// Eine Microtask.
pub enum Job {
    /// Einen Behandler auf einen erledigten Zustand loslassen.
    React { r: Reaction, arg: Value, rejected: bool },
    /// Ein fremdes Thenable uebernehmen: `then.call(thenable, res, rej)`.
    Adopt { thenable: Value, then: Value, target: Gc },
}

fn pdata(o: &Gc) -> Option<Rc<RefCell<PData>>> {
    match &o.borrow().kind { ObjKind::Promise(d) => Some(d.clone()), _ => None }
}

pub fn new_promise(i: &Interp) -> Gc {
    new_kind(Some(i.realm.promise_proto.clone()), ObjKind::Promise(Rc::new(RefCell::new(
        PData { state: PState::Pending, on_ok: Vec::new(), on_err: Vec::new() }))))
}

/// Erledigen. Nur EINMAL — wer schon erledigt ist, aendert sich nicht mehr.
pub fn settle(i: &mut Interp, p: &Gc, v: Value, rejected: bool) {
    let Some(d) = pdata(p) else { return };
    let (ok, err) = {
        let mut b = d.borrow_mut();
        if !matches!(b.state, PState::Pending) { return; }
        b.state = if rejected { PState::Rejected(v.clone()) } else { PState::Fulfilled(v.clone()) };
        (core::mem::take(&mut b.on_ok), core::mem::take(&mut b.on_err))
    };
    for r in if rejected { err } else { ok } {
        i.jobs.push_back(Job::React { r, arg: v.clone(), rejected });
    }
}

/// `ResolvePromise`: ein Thenable wird UEBERNOMMEN, alles andere erfuellt.
pub fn resolve_promise(i: &mut Interp, p: &Gc, v: Value) {
    if let Value::Obj(o) = &v {
        if Rc::ptr_eq(o, p) {
            let e = i.throw_kind("TypeError", "a promise cannot resolve with itself");
            if let Abrupt::Throw(ev) = e { settle(i, p, ev, true); }
            return;
        }
        // `then` LESEN kann werfen (ein Getter) — dann ist das der Grund
        // fuer die Ablehnung, nicht ein spaeterer Aufruf.
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

/// Das Paar `(resolve, reject)` fuer ein Versprechen. Beide teilen sich einen
/// Kasten; der erste Aufruf schliesst ihn fuer den anderen mit.
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

/// `PerformPromiseThen` — haengt an und gibt das abgeleitete Versprechen.
pub fn perform_then(i: &mut Interp, p: &Gc, on_ok: Value, on_err: Value) -> Gc {
    let derived = new_promise(i);
    let ok = if i.is_callable(&on_ok) { Some(on_ok) } else { None };
    let err = if i.is_callable(&on_err) { Some(on_err) } else { None };
    let Some(d) = pdata(p) else { return derived };
    let queued = {
        let mut b = d.borrow_mut();
        match &b.state {
            PState::Pending => {
                b.on_ok.push(Reaction { handler: ok, derived: derived.clone() });
                b.on_err.push(Reaction { handler: err, derived: derived.clone() });
                None
            }
            PState::Fulfilled(v) => Some((Reaction { handler: ok, derived: derived.clone() },
                                          v.clone(), false)),
            PState::Rejected(v) => Some((Reaction { handler: err, derived: derived.clone() },
                                         v.clone(), true)),
        }
    };
    if let Some((r, arg, rejected)) = queued {
        i.jobs.push_back(Job::React { r, arg, rejected });
    }
    derived
}

/// Die Schlange leeren.
///
/// Mit Deckel: eine Kette, die sich selbst nachlegt (`p.then(f)` in `f`), ist
/// ein voellig gewoehnliches Muster und wuerde sonst nie enden. Der Deckel ist
/// dieselbe Entscheidung wie bei `run_timers` — EINMAL durchlaufen ist zu
/// wenig (eine Kette braucht ihre Sprossen), unbegrenzt zu viel.
pub fn run_jobs(i: &mut Interp) -> usize {
    let mut n = 0;
    while let Some(j) = i.jobs.pop_front() {
        n += 1;
        if n > MAX_JOBS { break; }
        if i.tick().is_err() { break; }
        match j {
            Job::React { r, arg, rejected } => {
                match r.handler {
                    None => {
                        if rejected { settle(i, &r.derived, arg, true) }
                        else { resolve_promise(i, &r.derived, arg) }
                    }
                    Some(h) => match i.call(&h, Value::Undefined, &[arg]) {
                        Ok(v) => resolve_promise(i, &r.derived, v),
                        Err(Abrupt::Throw(e)) => settle(i, &r.derived, e, true),
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

/// Eine native Funktion mit EINEM festgebundenen ersten Argument.
///
/// Der Ersatz fuer einen Abschluss: `NativeFn` ist ein Funktionszeiger und
/// sieht sein eigenes Funktionsobjekt nicht, `ObjKind::Bound` stellt die
/// gebundenen Argumente aber vorn an.
fn bind1(i: &mut Interp, f: NativeFn, arg: Value) -> Value {
    let target = native(Some(i.realm.function_proto.clone()), f, "", 1, false);
    Value::Obj(new_kind(Some(i.realm.function_proto.clone()), ObjKind::Bound {
        target, this_val: Value::Undefined, args: vec![arg] }))
}

/// Der Behandler von `finally`: rufen, auf sein Ergebnis warten, DANN den
/// urspruenglichen Ausgang unveraendert weitergeben.
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

/// Ein Wert als Versprechen: ist er schon eines, bleibt er es.
fn to_promise(i: &mut Interp, v: &Value) -> Gc {
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
        // Der Ausfuehrer laeuft SOFORT, nicht als Microtask — und wirft er,
        // wird das Versprechen abgelehnt statt der Fehler weiterzureichen.
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
        let der = perform_then(i, &p.clone(), ok, err);
        Ok(Value::Obj(der))
    }, 2, &fp);
    d(&proto, "catch", |i, t, a| {
        let f = i.get(&t, "then")?;
        let h = a.first().cloned().unwrap_or(Value::Undefined);
        i.call(&f, t.clone(), &[Value::Undefined, h])
    }, 1, &fp);
    // `finally` reicht Wert UND Fehler unveraendert weiter — es sieht sie nur.
    //
    // Und es WARTET auf das, was der Behandler zurueckgibt: `.finally(() =>
    // aufraeumen())` mit einem Versprechen darin muss das Ergebnis
    // zurueckhalten, bis das Aufraeumen fertig ist. Deshalb der Umweg ueber
    // ein eigenes Versprechen statt den Wert direkt zurueckzugeben — der
    // kostet genau die Sprosse, die die Spezifikation dort vorsieht.
    d(&proto, "finally", |i, t, a| {
        let h = a.first().cloned().unwrap_or(Value::Undefined);
        let f = i.get(&t, "then")?;
        if !i.is_callable(&h) { return i.call(&f, t.clone(), &[h.clone(), h]); }
        let pass = bind1(i, |i, _, a| finally_step(i, a, false), h.clone());
        let fail = bind1(i, |i, _, a| finally_step(i, a, true), h);
        i.call(&f, t.clone(), &[pass, fail])
    }, 1, &fp);

    d(&ctor, "resolve", |i, _, a| {
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        Ok(Value::Obj(to_promise(i, &v)))
    }, 1, &fp);
    d(&ctor, "reject", |i, _, a| {
        let p = new_promise(i);
        settle(i, &p, a.first().cloned().unwrap_or(Value::Undefined), true);
        Ok(Value::Obj(p))
    }, 1, &fp);

    // `all` = 0, `allSettled` = 1, `any` = 2. Eine Umsetzung fuer drei: sie
    // unterscheiden sich nur darin, was ein einzelnes Ergebnis mit dem
    // Zaehler macht.
    d(&ctor, "all", |i, _, a| aggregate(i, a, 0), 1, &fp);
    d(&ctor, "allSettled", |i, _, a| aggregate(i, a, 1), 1, &fp);
    d(&ctor, "any", |i, _, a| aggregate(i, a, 2), 1, &fp);
    d(&ctor, "race", |i, _, a| {
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

/// Ein einzelnes Ergebnis auf den Zaehler buchen.
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

    // `all` faellt beim ersten Fehler; `any` beim ersten Erfolg. Beide sind
    // damit sofort fertig — der Zaehler zaehlt nur die andere Richtung.
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
    let _ = i.set(&vals, &num_to_string(idx), entry);

    // Der Zaehler liegt auf dem GEMEINSAMEN Vorfahren der Schlitze, nicht auf
    // dem Schlitz — sonst zaehlte jeder fuer sich.
    let Some(agg) = so.borrow().proto.clone() else { return };
    let left = agg.borrow().get_own(AGG_LEFT).and_then(|p| p.value.clone())
        .and_then(|v| match v { Value::Num(n) => Some(n), _ => None }).unwrap_or(0.0) - 1.0;
    agg.borrow_mut().define(AGG_LEFT, Prop::data(Value::Num(left)));
    if left > 0.0 { return; }
    if mode == 2 {
        let e = i.throw_kind("AggregateError", "all promises were rejected");
        if let Abrupt::Throw(ev) = e {
            let _ = i.set(&ev, "errors", vals);
            settle(i, &cap, ev, true);
        }
    } else {
        settle(i, &cap, vals, false);
    }
}

/// Deckel fuer `run_jobs` — dieselbe Begruendung wie die Schrittgrenze.
pub const MAX_JOBS: usize = 100_000;

