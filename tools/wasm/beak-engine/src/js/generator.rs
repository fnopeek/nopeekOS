//! Generatoren — das Anhalten, wegen dem die Befehlsmaschine ueberhaupt
//! gebaut wurde.
//!
//! **Die Antwort auf die Entwurfsfrage von Stufe 4** steht im Kopf von
//! `vm.rs` und heisst: ein Generator ist eine EIGENE Maschine, kein Rahmen in
//! fremder. Deshalb steht hier so wenig — der Zustand ist eine `Vm`, und die
//! drei Methoden sind drei Arten, sie wieder anzuwerfen.
//!
//! **Keine zweite Semantik.** Gebaut wird ein Generatorobjekt an genau EINER
//! Stelle (`make`, gerufen aus `Interp::call_inner`), und zwar auch dann,
//! wenn der Aufruf aus der Befehlsmaschine kam: die schickt einen Generator
//! bewusst ueber `Interp::call`, statt ihm einen Rahmen zu geben.

use alloc::rc::Rc;
use core::cell::{Cell, RefCell};

use super::interp::{Interp, C};
use super::value::{Gc, ObjKind, Prop, Value, new_kind};
use super::vm::{Step, Vm};

/// Der Lebenslauf eines Generators.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Gebaut, aber noch keinen Befehl gefahren. Ein `next(v)` wirft `v` weg —
    /// es gibt noch kein `yield`, das ihn entgegennehmen koennte.
    Start,
    /// Steht auf einem `yield`.
    Suspended,
    /// Laeuft gerade. Ein `next()` von innen ist ein Fehler, kein Neustart.
    Running,
    Done,
}

/// Was ein Generatorobjekt festhaelt.
///
/// `Cell`/`RefCell` und die Maschine als `Option`, damit waehrend des Laufens
/// KEINE Ausleihe offen steht: ein Generator, der sich selbst `next()` ruft,
/// kaeme sonst an einem `borrow_mut` vorbei und liesse den Kernel anhalten.
/// Herausnehmen, fahren, zurueckstellen.
pub struct GenState {
    vm: RefCell<Option<Vm>>,
    status: Cell<Status>,
    /// Das Versprechen, das eine ASYNC-Funktion am Ende erledigt. `None` bei
    /// einem gewoehnlichen Generator.
    ///
    /// Dass beide sich denselben Zustand teilen, ist kein Sparen: eine
    /// wartende async-Funktion IST ein angehaltener Rumpf, und der Bauplan
    /// sagte es so — „derselbe Mechanismus mit einem Promise davor". Der
    /// Unterschied ist allein, WER sie wieder anwirft: ein `next()` oder die
    /// Microtask-Schlange.
    promise: Option<Gc>,
}

impl GenState {
    /// Was die angehaltene Maschine festhaelt — siehe `Vm::roots`.
    pub fn roots(&self, objs: &mut alloc::vec::Vec<Gc>,
                 envs: &mut alloc::vec::Vec<Rc<RefCell<super::interp::Env>>>) {
        if let Some(vm) = self.vm.borrow().as_ref() {
            vm.roots(objs, envs);
        }
    }
}

/// Ein Aufruf einer Generatorfunktion: er baut das Objekt und faehrt NICHTS.
///
/// `None` heisst, dass der Uebersetzer den Rumpf nicht kann — dann bleibt es
/// beim alten Weg (der Baumlaeufer laeuft in sein „generators are not
/// supported"), statt hier einen halben Generator zu bauen.
///
/// Die Reihenfolge ist die der Spec: erst die Umgebung mit Parametern,
/// `this` und `arguments` (`call_env`), dann das Hochziehen, dann
/// `Get(f, "prototype")` fuer den Prototyp des Objekts.
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
    };
    Ok(Some(Value::Obj(new_kind(Some(proto), ObjKind::Generator(Rc::new(st))))))
}

// ── async/await ──────────────────────────────────────────────────────────
//
// Derselbe angehaltene Rumpf, nur wirft ihn die Microtask-Schlange wieder an
// statt eines `next()`. Der Zustand haengt an einem Objekt, das kein Skript je
// sieht — es ist bloss der Traeger, ueber den `promise::bind1` den beiden
// Behandlern ihren Generator mitgibt (ein `NativeFn` ist ein Zeiger und
// bekommt keinen Abschluss).

/// Ein Aufruf einer async-Funktion: er gibt ein VERSPRECHEN zurueck und
/// laeuft den Rumpf bis zum ersten `await` sofort — synchron, wie die Spec es
/// verlangt.
///
/// Auch ein Fehler beim Binden der Parameter wird zur ABLEHNUNG, nicht zu
/// einem Wurf: eine async-Funktion wirft nie, sie lehnt ab.
pub fn make_async(i: &mut Interp, d: &Rc<super::value::FuncData>,
                  this_val: Value, args: &[Value]) -> C<Option<Value>> {
    let Some(chunk) = i.func_chunk(&d.node) else {
        // **Auch ein unuebersetzbarer Rumpf gibt ein Versprechen zurueck.**
        // Der Baumlaeufer faehrt ihn (und laeuft an einem `await` in seinen
        // TypeError), aber der Aufrufvertrag bleibt derselbe: eine
        // async-Funktion wirft nie und gibt nie einen nackten Wert. Zwei
        // Aufrufvertraege fuer dasselbe Schluesselwort waeren genau die
        // zweite Semantik, die dieser Umbau vermeidet.
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
    });
    let holder = Value::Obj(new_kind(None, ObjKind::Generator(st.clone())));
    pump(i, &st, Seed::Start, holder);
    Ok(Some(Value::Obj(outer)))
}

/// Womit die Maschine wieder anlaeuft.
enum Seed {
    /// Zum ersten Mal — es gibt noch kein `await`, das einen Wert naehme.
    Start,
    Value(Value),
    Throw(Value),
}

/// Die Maschine einer async-Funktion fahren, bis sie wartet oder fertig ist —
/// und danach ihr Versprechen erledigen.
fn pump(i: &mut Interp, st: &Rc<GenState>, seed: Seed, holder: Value) {
    let Some(outer) = st.promise.clone() else { return };
    let Some(mut vm) = st.vm.borrow_mut().take() else { return };
    st.status.set(Status::Running);
    let r = match seed {
        Seed::Start => vm.drive(i),
        Seed::Value(v) => { vm.send(v); vm.drive(i) }
        // Faengt den Wurf im Rumpf niemand, ist die Funktion damit fertig —
        // und ihr Versprechen abgelehnt.
        Seed::Throw(e) => {
            if vm.inject_throw(i, e.clone()) { vm.drive(i) }
            else { Err(super::interp::Abrupt::Throw(e)) }
        }
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
        // Kann nicht vorkommen: ein async-Generator ist beim Uebersetzen
        // abgelehnt, ein `yield` steht also in keinem async-Rumpf.
        Ok(Step::Yield(_)) => {
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

/// Der Behandler, den `await` am Versprechen anhaengt: `a[0]` ist der
/// gebundene Traeger, `a[1]` die Aufloesung.
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

/// Der Zustand hinter `this` — oder ein TypeError, wenn `this` keiner ist.
/// Die Ausleihe endet HIER, vor allem, was danach laeuft.
fn state(i: &mut Interp, t: &Value) -> C<Rc<GenState>> {
    if let Value::Obj(o) = t {
        if let ObjKind::Generator(g) = &o.borrow().kind {
            return Ok(g.clone());
        }
    }
    i.type_err("not a generator")
}

/// Was `drive` ergeben hat, in ein `{value, done}` umsetzen — und den Zustand
/// dabei richtig stellen. Ein Wurf beendet den Generator endgueltig.
fn finish(i: &mut Interp, st: &Rc<GenState>, mut vm: Vm, r: C<Step>) -> C<Value> {
    match r {
        Ok(Step::Yield(v)) => {
            st.status.set(Status::Suspended);
            *st.vm.borrow_mut() = Some(vm);
            Ok(i.iter_result(v, false))
        }
        Ok(Step::Done(v)) => {
            st.status.set(Status::Done);
            Ok(i.iter_result(v, true))
        }
        // Kann nicht vorkommen: ein `await` steht nur im Rumpf einer
        // async-Funktion, und ein async-Generator ist beim Uebersetzen
        // abgelehnt. Steht hier, weil ein `_ =>` den Fall verschweigen wuerde.
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

/// Die Maschine herausnehmen, wenn der Zustand es erlaubt.
///
/// `Ok(None)` heisst „fertig, nichts mehr zu tun"; ein laufender Generator
/// gibt einen TypeError, weil ihn wieder anzuwerfen seinen Stapel
/// verdoppeln wuerde.
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
    // Beim ALLERERSTEN `next` gibt es kein `yield`, das den Wert nehmen
    // koennte — er faellt weg. Genau das sagt die Spec.
    if started { vm.send(v); }
    st.status.set(Status::Running);
    let r = vm.drive(i);
    finish(i, &st, vm, r)
}

/// `gen.throw(v)`: den Wurf an der Anhaltestelle einwerfen. Faengt ihn dort
/// niemand, ist der Generator fertig und der Wurf gehoert dem Rufer — auch
/// dann, wenn noch gar nichts gelaufen war.
pub fn throw(i: &mut Interp, t: &Value, v: Value) -> C<Value> {
    let st = state(i, t)?;
    let Some(mut vm) = take(i, &st)? else { return Err(super::interp::Abrupt::Throw(v)) };
    st.status.set(Status::Running);
    if !vm.inject_throw(i, v.clone()) {
        st.status.set(Status::Done);
        vm.close(i);
        return Err(super::interp::Abrupt::Throw(v));
    }
    let r = vm.drive(i);
    finish(i, &st, vm, r)
}

/// `gen.return(v)`: aufgeben. Offene `for…of`-Iterationen werden geschlossen
/// (`Vm::close`); ein anhaengiger `finally` kann es nicht geben, weil ein
/// `yield` darunter schon beim Uebersetzen abgelehnt wird.
pub fn ret(i: &mut Interp, t: &Value, v: Value) -> C<Value> {
    let st = state(i, t)?;
    let Some(mut vm) = take(i, &st)? else { return Ok(i.iter_result(v, true)) };
    st.status.set(Status::Done);
    vm.close(i);
    Ok(i.iter_result(v, true))
}

/// Die zwei Prototypen des Generatorvertrags.
///
/// `%GeneratorPrototype%` haengt unter `%IteratorPrototype%` — daher kommt
/// `[Symbol.iterator]() { return this }`, und genau daran haengt, dass
/// `for (x of gen())` und `[...gen()]` gehen, ohne dass hier etwas dafuer
/// steht.
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
