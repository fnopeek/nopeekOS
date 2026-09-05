//! Die Iterator-Hilfen (ES 2025) — `Iterator`, `Iterator.from` und die zwoelf
//! Methoden auf `%IteratorPrototype%`.
//!
//! **Faul, wo die Spezifikation faul ist.** `map`, `filter`, `take`, `drop`
//! und `flatMap` geben ein Hilfsobjekt zurueck, das erst beim `next()`
//! rechnet — eine eifrige Fassung wuerde an einer unendlichen Quelle haengen,
//! und genau dafuer gibt es `take`.
//!
//! Eine native Funktion nimmt keinen Abschluss, also liegt der Zustand am
//! Objekt: NUL-praefigierte Schluessel, unsichtbar fuer jedes Skript — genau
//! wie bei den Feld-Iteratoren (`IT_TARGET` & Co.).

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;

use super::interp::*;
use super::value::*;

/// Die Quelle des Hilfsobjekts (der darunterliegende Iterator).
const H_SRC: &str = "\0!hsrc";
/// Der Rueckruf (`map`/`filter`/`flatMap`) bzw. die Zahl (`take`/`drop`).
const H_FN: &str = "\0!hfn";
/// Wieviele noch (`take`/`drop`).
const H_N: &str = "\0!hn";
/// Der laufende Zaehler, den der Rueckruf als zweites Argument bekommt.
const H_I: &str = "\0!hi";
/// Welche Hilfe: 0 map, 1 filter, 2 take, 3 drop, 4 flatMap.
const H_KIND: &str = "\0!hkind";
/// Beim `flatMap`: der gerade offene innere Iterator.
const H_INNER: &str = "\0!hinner";
/// Ist das Hilfsobjekt schon fertig? Danach gibt `next` nur noch `done`.
const H_DONE: &str = "\0!hdone";

fn hidden(v: Value) -> Prop {
    Prop { value: Some(v), get: None, set: None,
           writable: true, enumerable: false, configurable: false }
}

fn iter_result(i: &mut Interp, value: Value, done: bool) -> Value {
    let o = new_obj(Some(i.realm.object_proto.clone()));
    o.borrow_mut().define("value", Prop::data(value));
    o.borrow_mut().define("done", Prop::data(Value::Bool(done)));
    Value::Obj(o)
}

fn make_helper(i: &mut Interp, src: Value, kind: u8, f: Value, n: f64) -> Value {
    let g = new_obj(Some(i.realm.iter_helper_proto.clone()));
    {
        let mut o = g.borrow_mut();
        o.define(H_SRC, hidden(src));
        o.define(H_KIND, hidden(Value::Num(kind as f64)));
        o.define(H_FN, hidden(f));
        o.define(H_N, hidden(Value::Num(n)));
        o.define(H_I, hidden(Value::Num(0.0)));
        o.define(H_DONE, hidden(Value::Bool(false)));
    }
    Value::Obj(g)
}

fn slot(i: &mut Interp, t: &Value, k: &str) -> C<Value> {
    match t {
        Value::Obj(o) => match o.borrow().get_own(k).and_then(|p| p.value.clone()) {
            Some(v) => Ok(v),
            None => i.type_err("not an Iterator Helper"),
        },
        _ => i.type_err("not an Iterator Helper"),
    }
}

fn put(t: &Value, k: &str, v: Value) {
    if let Value::Obj(o) = t { o.borrow_mut().define(k, hidden(v)); }
}

/// Ein Schritt des Hilfsobjekts. Die fünf Formen unterscheiden sich nur
/// darin, was sie mit dem Wert von unten machen.
fn helper_next(i: &mut Interp, t: Value, _a: &[Value]) -> C<Value> {
    if slot(i, &t, H_DONE)?.truthy() { return Ok(iter_result(i, Value::Undefined, true)); }
    let src = slot(i, &t, H_SRC)?;
    let kv = slot(i, &t, H_KIND)?;
    let kind = i.to_number(&kv)? as u8;
    let f = slot(i, &t, H_FN)?;
    loop {
        i.tick()?;
        // `flatMap` liest erst den offenen inneren Iterator leer.
        if kind == 4 {
            let inner = slot(i, &t, H_INNER).unwrap_or(Value::Undefined);
            if !matches!(inner, Value::Undefined) {
                match i.iter_next(&inner) {
                    Ok(Some(v)) => return Ok(iter_result(i, v, false)),
                    Ok(None) => put(&t, H_INNER, Value::Undefined),
                    Err(e) => { put(&t, H_DONE, Value::Bool(true)); return Err(e) }
                }
            }
        }
        if kind == 3 {
            // `drop`: die ersten n wegwerfen, EINMAL.
            let nv = slot(i, &t, H_N)?;
            let n = i.to_number(&nv)?;
            if n > 0.0 {
                for _ in 0..f64_to_usize(n) {
                    i.tick()?;
                    if i.iter_next(&src)?.is_none() {
                        put(&t, H_DONE, Value::Bool(true));
                        return Ok(iter_result(i, Value::Undefined, true));
                    }
                }
                put(&t, H_N, Value::Num(0.0));
            }
        }
        if kind == 2 {
            let nv = slot(i, &t, H_N)?;
            let n = i.to_number(&nv)?;
            if n <= 0.0 {
                put(&t, H_DONE, Value::Bool(true));
                i.iter_close(&src);
                return Ok(iter_result(i, Value::Undefined, true));
            }
            put(&t, H_N, Value::Num(n - 1.0));
        }
        let nx = match i.iter_next(&src) {
            Ok(v) => v,
            Err(e) => { put(&t, H_DONE, Value::Bool(true)); return Err(e) }
        };
        let Some(v) = nx else {
            put(&t, H_DONE, Value::Bool(true));
            return Ok(iter_result(i, Value::Undefined, true));
        };
        let iv = slot(i, &t, H_I)?;
        let idx = i.to_number(&iv)?;
        put(&t, H_I, Value::Num(idx + 1.0));
        match kind {
            0 => {
                let r = call_or_close(i, &t, &f, &[v, Value::Num(idx)], &src)?;
                return Ok(iter_result(i, r, false));
            }
            1 => {
                let r = call_or_close(i, &t, &f, &[v.clone(), Value::Num(idx)], &src)?;
                if r.truthy() { return Ok(iter_result(i, v, false)); }
            }
            2 | 3 => return Ok(iter_result(i, v, false)),
            _ => {
                let r = call_or_close(i, &t, &f, &[v, Value::Num(idx)], &src)?;
                // Eine Zeichenkette ist hier ausdruecklich NICHT iterierbar:
                // `flatMap` soll nicht in ihre Zeichen zerfallen.
                if matches!(r, Value::Str(_)) {
                    put(&t, H_DONE, Value::Bool(true));
                    i.iter_close(&src);
                    return i.type_err("flatMap: the mapped value is not iterable");
                }
                let inner = match i.get_iterator(&r) {
                    Ok(x) => x,
                    Err(e) => { put(&t, H_DONE, Value::Bool(true)); i.iter_close(&src); return Err(e) }
                };
                put(&t, H_INNER, inner);
            }
        }
    }
}

/// Wirft der Rueckruf, wird die Quelle geschlossen — sonst bliebe ein
/// `finally` im fremden Generator liegen.
fn call_or_close(i: &mut Interp, t: &Value, f: &Value, args: &[Value], src: &Value) -> C<Value> {
    match i.call(f, Value::Undefined, args) {
        Ok(v) => Ok(v),
        Err(e) => { put(t, H_DONE, Value::Bool(true)); i.iter_close(src); Err(e) }
    }
}

/// `GetIteratorDirect` — die Hilfen nehmen den Empfaenger, wie er ist, und
/// fragen NICHT nach `Symbol.iterator`.
fn this_iter(i: &mut Interp, t: &Value) -> C<Value> {
    if !matches!(t, Value::Obj(_)) { return i.type_err("Iterator method on a non-object"); }
    Ok(t.clone())
}

fn need_fn(i: &mut Interp, a: &[Value]) -> C<Value> {
    let f = a.first().cloned().unwrap_or(Value::Undefined);
    if !i.is_callable(&f) { return i.type_err("Iterator helper: argument is not a function"); }
    Ok(f)
}

/// `ToIntegerOrInfinity` mit der Ablehnung, die die Hilfen verlangen: NaN
/// und negative Zahlen sind ein RangeError, nicht stillschweigend 0.
fn need_count(i: &mut Interp, a: &[Value]) -> C<f64> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let n = i.to_number(&v)?;
    if n.is_nan() { return i.range_err("Iterator helper: count must be a number"); }
    let n = to_integer(n);
    if n < 0.0 { return i.range_err("Iterator helper: count must not be negative"); }
    Ok(n)
}

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let iproto = realm.iterator_proto.clone();

    let def = |o: &Gc, name: &str, f: NativeFn, len: usize| {
        let g = native(Some(fp.clone()), f, name, len, false);
        o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
    };

    // %IteratorHelperPrototype% — erbt von %IteratorPrototype%, damit ein
    // Hilfsobjekt selbst wieder `map`/`filter`/… kann.
    let hproto = new_obj(Some(iproto.clone()));
    realm.iter_helper_proto = hproto.clone();
    def(&hproto, "next", |i, t, a| helper_next(i, t, a), 0);
    def(&hproto, "return", |i, t, _| {
        // Aufgeben heisst: die Quelle schliessen und fertig melden.
        let src = slot(i, &t, H_SRC)?;
        put(&t, H_DONE, Value::Bool(true));
        i.iter_close(&src);
        Ok(iter_result(i, Value::Undefined, true))
    }, 0);
    hproto.borrow_mut().define(SYM_TO_STRING_TAG, Prop {
        value: Some(Value::str("Iterator Helper")), get: None, set: None,
        writable: false, enumerable: false, configurable: true });

    // ── Die fuenf faulen Hilfen ──────────────────────────────────────────
    def(&iproto, "map", |i, t, a| {
        let s = this_iter(i, &t)?; let f = need_fn(i, a)?;
        Ok(make_helper(i, s, 0, f, 0.0))
    }, 1);
    def(&iproto, "filter", |i, t, a| {
        let s = this_iter(i, &t)?; let f = need_fn(i, a)?;
        Ok(make_helper(i, s, 1, f, 0.0))
    }, 1);
    def(&iproto, "take", |i, t, a| {
        let s = this_iter(i, &t)?; let n = need_count(i, a)?;
        Ok(make_helper(i, s, 2, Value::Undefined, n))
    }, 1);
    def(&iproto, "drop", |i, t, a| {
        let s = this_iter(i, &t)?; let n = need_count(i, a)?;
        Ok(make_helper(i, s, 3, Value::Undefined, n))
    }, 1);
    def(&iproto, "flatMap", |i, t, a| {
        let s = this_iter(i, &t)?; let f = need_fn(i, a)?;
        Ok(make_helper(i, s, 4, f, 0.0))
    }, 1);

    // ── Die sieben, die bis zum Ende laufen ──────────────────────────────
    def(&iproto, "toArray", |i, t, _| {
        let s = this_iter(i, &t)?;
        let mut out = Vec::new();
        while let Some(v) = i.iter_next(&s)? { i.tick()?; out.push(v); }
        Ok(i.new_array(out))
    }, 0);
    def(&iproto, "forEach", |i, t, a| {
        let s = this_iter(i, &t)?; let f = need_fn(i, a)?;
        let mut k = 0.0;
        while let Some(v) = i.iter_next(&s)? {
            i.tick()?;
            if let Err(e) = i.call(&f, Value::Undefined, &[v, Value::Num(k)]) {
                i.iter_close(&s); return Err(e);
            }
            k += 1.0;
        }
        Ok(Value::Undefined)
    }, 1);
    def(&iproto, "reduce", |i, t, a| {
        let s = this_iter(i, &t)?; let f = need_fn(i, a)?;
        let mut k = 0.0;
        let mut acc = match a.get(1) {
            Some(v) => v.clone(),
            None => match i.iter_next(&s)? {
                Some(v) => { k = 1.0; v }
                None => return i.type_err("reduce of empty iterator with no initial value"),
            },
        };
        while let Some(v) = i.iter_next(&s)? {
            i.tick()?;
            acc = match i.call(&f, Value::Undefined, &[acc, v, Value::Num(k)]) {
                Ok(x) => x,
                Err(e) => { i.iter_close(&s); return Err(e) }
            };
            k += 1.0;
        }
        Ok(acc)
    }, 1);
    // `some`, `every` und `find` unterscheiden sich nur im Abbruch — aber
    // jede braucht ihren eigenen Zeiger, also ein Makro.
    macro_rules! short {
        ($($n:literal => $mode:literal),* $(,)?) => { $(
            def(&iproto, $n, |i, t, a| {
                let s = this_iter(i, &t)?; let f = need_fn(i, a)?;
                let mode: u8 = $mode;
                let mut k = 0.0;
                while let Some(v) = i.iter_next(&s)? {
                    i.tick()?;
                    let r = match i.call(&f, Value::Undefined, &[v.clone(), Value::Num(k)]) {
                        Ok(x) => x,
                        Err(e) => { i.iter_close(&s); return Err(e) }
                    };
                    k += 1.0;
                    match mode {
                        0 => if r.truthy() { i.iter_close(&s); return Ok(Value::Bool(true)) },
                        1 => if !r.truthy() { i.iter_close(&s); return Ok(Value::Bool(false)) },
                        _ => if r.truthy() { i.iter_close(&s); return Ok(v) },
                    }
                }
                Ok(match mode { 0 => Value::Bool(false), 1 => Value::Bool(true), _ => Value::Undefined })
            }, 1);
        )* };
    }
    short! { "some" => 0u8, "every" => 1u8, "find" => 2u8 }

    // ── Der Konstruktor ──────────────────────────────────────────────────
    //
    // Abstrakt: `new Iterator()` wirft, `Iterator()` auch. Er ist nur da,
    // damit `Iterator.prototype` und `Iterator.from` eine Heimat haben.
    let ctor = native(Some(fp.clone()), |i, _, _| {
        i.type_err("Iterator is an abstract class")
    }, "Iterator", 0, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(iproto.clone())));
    iproto.borrow_mut().define("constructor", Prop {
        value: Some(Value::Obj(ctor.clone())), get: None, set: None,
        writable: true, enumerable: false, configurable: true });
    iproto.borrow_mut().define(SYM_TO_STRING_TAG, Prop {
        value: Some(Value::str("Iterator")), get: None, set: None,
        writable: true, enumerable: false, configurable: true });

    // %WrapForValidIteratorPrototype% — was `Iterator.from` um einen fremden
    // Iterator legt, damit die Hilfen darauf laufen.
    let wproto = new_obj(Some(iproto.clone()));
    realm.iter_wrap_proto = wproto.clone();
    def(&wproto, "next", |i, t, _| {
        let s = slot(i, &t, H_SRC)?;
        let f = i.get(&s, "next")?;
        i.call(&f, s, &[])
    }, 0);
    def(&wproto, "return", |i, t, _| {
        let s = slot(i, &t, H_SRC)?;
        let f = i.get(&s, "return")?;
        if !i.is_callable(&f) { return Ok(iter_result(i, Value::Undefined, true)); }
        i.call(&f, s, &[])
    }, 0);

    def(&ctor, "from", |i, _, a| {
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        // Eine Zeichenkette wird ueber ihren `Symbol.iterator` genommen,
        // alles andere direkt, wenn es schon ein Iterator ist.
        let it = if matches!(v, Value::Str(_)) { i.get_iterator(&v)? }
                 else {
                     let m = if matches!(v, Value::Undefined | Value::Null) { Value::Undefined }
                             else { i.get(&v, SYM_ITERATOR)? };
                     if i.is_callable(&m) { i.get_iterator(&v)? }
                     else if matches!(v, Value::Obj(_)) { v.clone() }
                     else { return i.type_err("Iterator.from: not an iterator") }
                 };
        // Haengt es schon an `%IteratorPrototype%`, braucht es keinen Mantel.
        if let Value::Obj(o) = &it {
            let mut cur = o.borrow().proto.clone();
            let mut hops = 0;
            while let Some(c) = cur {
                hops += 1;
                if hops > MAX_PROTO_CHAIN { break }
                if Rc::ptr_eq(&c, &i.realm.iterator_proto) { return Ok(it.clone()); }
                let n = c.borrow().proto.clone();
                cur = n;
            }
        }
        let g = new_obj(Some(i.realm.iter_wrap_proto.clone()));
        g.borrow_mut().define(H_SRC, hidden(it));
        Ok(Value::Obj(g))
    }, 1);
    // `Iterator.concat` reiht mehrere hintereinander. Eifrig eingesammelt —
    // benannt statt verschwiegen: an einer unendlichen Quelle haengt sie.
    def(&ctor, "concat", |i, _, a| {
        let mut out = Vec::new();
        for v in a {
            if !matches!(v, Value::Obj(_)) { return i.type_err("Iterator.concat: not an object"); }
            let it = i.get_iterator(v)?;
            while let Some(x) = i.iter_next(&it)? { i.tick()?; out.push(x); }
        }
        let arr = i.new_array(out);
        i.array_iter(arr, 0)
    }, 0);

    realm.global.borrow_mut().define("Iterator", Prop::builtin(Value::Obj(ctor)));
    let _ = vec![0u8; 0];
}
