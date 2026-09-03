//! Das globale Objekt und die eingebauten Prototypen.
//!
//! Das MINDESTMASS ist nicht geraten: es ist das, was test262s eigener
//! Vorspann (`assert.js` + `sta.js`) verlangt, nachgelesen statt vermutet —
//! `Object.prototype.toString.call`, `Function.prototype.call`, `String()`,
//! `Error` mit `name`/`message`, `Array.prototype` fuer `compareArray`.
//! Laeuft der Vorspann nicht, besteht kein einziger Test, egal wie gut der
//! Rest ist.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use hashbrown::HashMap;

use super::interp::*;
use super::value::*;

fn def(o: &Gc, name: &str, f: NativeFn, len: usize, proto: &Gc) {
    let g = native(Some(proto.clone()), f, name, len, false);
    o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
}

/// Wie `def`, aber unter einem SYMBOL. Der Anzeigename ist ein anderer als der
/// Schluessel — `f.name` ist `"[Symbol.iterator]"`, nicht das NUL-Byte.
fn def_sym(o: &Gc, key: &str, show: &str, f: NativeFn, len: usize, proto: &Gc) {
    let g = native(Some(proto.clone()), f, show, len, false);
    o.borrow_mut().define(key, Prop::builtin(Value::Obj(g)));
}

pub fn make_realm() -> Realm {
    let object_proto = new_obj(None);
    let function_proto = new_kind(Some(object_proto.clone()), ObjKind::Plain);
    let array_proto = new_kind(Some(object_proto.clone()), ObjKind::Array);
    let string_proto = new_obj(Some(object_proto.clone()));
    let number_proto = new_obj(Some(object_proto.clone()));
    let boolean_proto = new_obj(Some(object_proto.clone()));
    let error_proto = new_obj(Some(object_proto.clone()));
    let symbol_proto = new_obj(Some(object_proto.clone()));
    let iterator_proto = new_obj(Some(object_proto.clone()));
    let array_iter_proto = new_obj(Some(iterator_proto.clone()));
    let string_iter_proto = new_obj(Some(iterator_proto.clone()));
    let global = new_obj(Some(object_proto.clone()));
    let global_env = Env::new(None, true);
    global_env.borrow_mut().this_val = Some(Value::Obj(global.clone()));

    let fp = &function_proto;

    // ── Object.prototype ─────────────────────────────────────────────────
    def(&object_proto, "toString", |i, this, _| {
        Ok(Value::string(match &this {
            Value::Undefined => "[object Undefined]".to_string(),
            Value::Null => "[object Null]".to_string(),
            Value::Obj(o) => {
                // `Symbol.toStringTag` gewinnt vor der eingebauten Art —
                // aber nur, wenn es eine Zeichenkette ist.
                if let Ok(Value::Str(t)) = i.get(&this, SYM_TO_STRING_TAG) {
                    return Ok(Value::string(alloc::format!("[object {t}]")));
                }
                let tag = match &o.borrow().kind {
                    ObjKind::Array => "Array",
                    ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. } => "Function",
                    ObjKind::Error => "Error",
                    ObjKind::StrWrap(_) => "String",
                    ObjKind::SymWrap(_) => "Symbol",
                    ObjKind::NumWrap(_) => "Number",
                    ObjKind::BoolWrap(_) => "Boolean",
                    ObjKind::Arguments => "Arguments",
                    ObjKind::Regex(_) => "RegExp",
                    ObjKind::Promise(_) => "Promise",
                    ObjKind::Plain => "Object",
                };
                alloc::format!("[object {tag}]")
            }
            Value::Sym(_) => "[object Symbol]".to_string(),
            _ => "[object Object]".to_string()
        }))
    }, 0, fp);
    def(&object_proto, "valueOf", |i, this, _| {
        let o = i.to_object(&this)?; Ok(Value::Obj(o))
    }, 0, fp);
    def(&object_proto, "hasOwnProperty", |i, this, a| {
        let k = i.to_prop_key(a.first().unwrap_or(&Value::Undefined))?;
        let o = i.to_object(&this)?;
        let has = o.borrow().has_own(&k);
        Ok(Value::Bool(has))
    }, 1, fp);
    def(&object_proto, "isPrototypeOf", |_, this, a| {
        let (Value::Obj(p), Some(Value::Obj(v))) = (&this, a.first()) else { return Ok(Value::Bool(false)) };
        let mut cur = v.borrow().proto.clone();
        while let Some(c) = cur {
            if Rc::ptr_eq(&c, p) { return Ok(Value::Bool(true)); }
            let n = c.borrow().proto.clone(); cur = n;
        }
        Ok(Value::Bool(false))
    }, 1, fp);
    def(&object_proto, "propertyIsEnumerable", |i, this, a| {
        let k = i.to_prop_key(a.first().unwrap_or(&Value::Undefined))?;
        let o = i.to_object(&this)?;
        let e = o.borrow().get_own(&k).map(|p| p.enumerable).unwrap_or(false);
        Ok(Value::Bool(e))
    }, 1, fp);

    // ── Function.prototype ───────────────────────────────────────────────
    def(fp, "call", |i, this, a| {
        let t = a.first().cloned().unwrap_or(Value::Undefined);
        i.call(&this, t, a.get(1..).unwrap_or(&[]))
    }, 1, fp);
    def(fp, "apply", |i, this, a| {
        let t = a.first().cloned().unwrap_or(Value::Undefined);
        let args = match a.get(1) {
            None | Some(Value::Undefined) | Some(Value::Null) => Vec::new(),
            // `apply` liest ARRAY-AEHNLICH, nicht ueber den Iterator —
            // `f.apply(null, {length:2, 0:'a', 1:'b'})` muss gehen.
            Some(v) => i.elems(v)?,
        };
        i.call(&this, t, &args)
    }, 2, fp);
    def(fp, "bind", |i, this, a| {
        let Value::Obj(t) = &this else { return i.type_err("bind on a non-function") };
        let bt = a.first().cloned().unwrap_or(Value::Undefined);
        let rest: Vec<Value> = a.get(1..).unwrap_or(&[]).to_vec();
        let g = new_kind(Some(i.realm.function_proto.clone()),
            ObjKind::Bound { target: t.clone(), this_val: bt, args: rest });
        Ok(Value::Obj(g))
    }, 1, fp);
    def(fp, "toString", |_, _, _| Ok(Value::str("function () { [native code] }")), 0, fp);

    // ── Error ────────────────────────────────────────────────────────────
    error_proto.borrow_mut().define("name", Prop::builtin(Value::str("Error")));
    error_proto.borrow_mut().define("message", Prop::builtin(Value::str("")));
    def(&error_proto, "toString", |i, this, _| {
        let n = i.get(&this, "name")?;
        let m = i.get(&this, "message")?;
        let ns = i.to_string(&n)?; let ms = i.to_string(&m)?;
        Ok(Value::string(if ms.is_empty() { ns.to_string() }
                         else { alloc::format!("{ns}: {ms}") }))
    }, 0, fp);

    let mut error_ctors: HashMap<&'static str, Gc> = HashMap::new();
    error_ctors.insert("Error", error_proto.clone());

    // Ein Konstruktor je Fehlerart. Er baut sein Objekt selbst — deshalb
    // `ctor: true` und `this` unbenutzt.
    macro_rules! err_ctor {
        ($name:literal, $f:expr) => {{
            let proto = if $name == "Error" { error_proto.clone() }
                        else { new_obj(Some(error_proto.clone())) };
            if $name != "Error" {
                proto.borrow_mut().define("name", Prop::builtin(Value::str($name)));
                proto.borrow_mut().define("message", Prop::builtin(Value::str("")));
            }
            let c = native(Some(function_proto.clone()), $f, $name, 1, true);
            c.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
            proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(c.clone())));
            global.borrow_mut().define($name, Prop::builtin(Value::Obj(c)));
            error_ctors.insert($name, proto);
        }};
    }
    err_ctor!("Error", |i, _, a| make_error(i, "Error", a));
    err_ctor!("TypeError", |i, _, a| make_error(i, "TypeError", a));
    err_ctor!("RangeError", |i, _, a| make_error(i, "RangeError", a));
    err_ctor!("SyntaxError", |i, _, a| make_error(i, "SyntaxError", a));
    err_ctor!("ReferenceError", |i, _, a| make_error(i, "ReferenceError", a));
    err_ctor!("EvalError", |i, _, a| make_error(i, "EvalError", a));
    // `AggregateError` nimmt die Fehlerliste ZUERST, die Meldung danach —
    // als einziger Fehlerkonstruktor. `Promise.any` braucht ihn.
    err_ctor!("AggregateError", |i, _, a| {
        let e = make_error(i, "AggregateError", a.get(1..).unwrap_or(&[]))?;
        let errs = match a.first() {
            None | Some(Value::Undefined) => i.new_array(alloc::vec::Vec::new()),
            Some(v) => { let items = i.iterate(v)?; i.new_array(items) }
        };
        i.set(&e, "errors", errs)?;
        Ok(e)
    });
    err_ctor!("URIError", |i, _, a| make_error(i, "URIError", a));

    // ── Array.prototype ──────────────────────────────────────────────────
    array_proto.borrow_mut().define("length", Prop {
        value: Some(Value::Num(0.0)), get: None, set: None,
        writable: true, enumerable: false, configurable: false });
    def(&array_proto, "push", |i, this, a| {
        let mut n = array_len(i, &this)?;
        for v in a { i.tick()?; i.set(&this, &num_to_string(n), v.clone())?; n += 1.0; }
        i.set(&this, "length", Value::Num(n))?;
        Ok(Value::Num(n))
    }, 1, fp);
    def(&array_proto, "pop", |i, this, _| {
        let n = array_len(i, &this)?;
        if n <= 0.0 { i.set(&this, "length", Value::Num(0.0))?; return Ok(Value::Undefined); }
        let k = num_to_string(n - 1.0);
        let v = i.get(&this, &k)?;
        if let Value::Obj(o) = &this { o.borrow_mut().remove(&k); }
        i.set(&this, "length", Value::Num(n - 1.0))?;
        Ok(v)
    }, 0, fp);
    def(&array_proto, "join", |i, this, a| {
        let sep = match a.first() {
            None | Some(Value::Undefined) => Rc::from(","),
            Some(v) => i.to_string(v)?,
        };
        let n = array_len(i, &this)? as usize;
        let mut s = String::new();
        for k in 0..n {
            i.tick()?;
            if k > 0 { s.push_str(&sep); }
            let v = i.get(&this, &num_to_string(k as f64))?;
            if !matches!(v, Value::Undefined | Value::Null) { s.push_str(&i.to_string(&v)?); }
        }
        Ok(Value::string(s))
    }, 1, fp);
    def(&array_proto, "toString", |i, this, _| {
        let j = i.get(&this, "join")?;
        if i.is_callable(&j) { i.call(&j, this, &[]) } else { Ok(Value::str("[object Array]")) }
    }, 0, fp);
    def(&array_proto, "indexOf", |i, this, a| {
        let target = a.first().cloned().unwrap_or(Value::Undefined);
        let n = array_len(i, &this)? as usize;
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if v.strict_eq(&target) { return Ok(Value::Num(k as f64)); }
        }
        Ok(Value::Num(-1.0))
    }, 1, fp);
    def(&array_proto, "slice", |i, this, a| {
        let n = array_len(i, &this)? as i64;
        let idx = |v: Option<&Value>, d: i64, i: &mut Interp| -> C<i64> {
            Ok(match v { None | Some(Value::Undefined) => d,
                Some(x) => { let r = to_integer(i.to_number(x)?) as i64;
                             if r < 0 { (n + r).max(0) } else { r.min(n) } } })
        };
        let s = idx(a.first(), 0, i)?;
        let e = idx(a.get(1), n, i)?;
        let mut out = Vec::new();
        for k in s..e { i.tick()?; out.push(i.get(&this, &num_to_string(k as f64))?); }
        Ok(i.new_array(out))
    }, 2, fp);
    def(&array_proto, "forEach", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        let t = a.get(1).cloned().unwrap_or(Value::Undefined);
        for k in 0..n {
            i.tick()?;
            let key = num_to_string(k as f64);
            if let Value::Obj(o) = &this { if !i.has_property(o, &key) { continue; } }
            let v = i.get(&this, &key)?;
            i.call(&f, t.clone(), &[v, Value::Num(k as f64), this.clone()])?;
        }
        Ok(Value::Undefined)
    }, 1, fp);
    def(&array_proto, "map", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        let t = a.get(1).cloned().unwrap_or(Value::Undefined);
        // Die Kapazitaet NIE aus einer gastkontrollierten Laenge: `new
        // Array(2**32-1).map(f)` hat hier 96 GiB angefordert, und eine
        // gescheiterte Allokation ist ein ABBRUCH, den `catch_unwind` nicht
        // faengt — sie hat den ganzen Lauf mitgenommen. Wachsen lassen kostet
        // nichts, was die Schrittgrenze nicht ohnehin vorher stoppt.
        let mut out = Vec::with_capacity(n.min(1 << 16));
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            out.push(i.call(&f, t.clone(), &[v, Value::Num(k as f64), this.clone()])?);
        }
        Ok(i.new_array(out))
    }, 1, fp);
    def(&array_proto, "filter", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        let t = a.get(1).cloned().unwrap_or(Value::Undefined);
        let mut out = Vec::new();
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if i.call(&f, t.clone(), &[v.clone(), Value::Num(k as f64), this.clone()])?.truthy() {
                out.push(v);
            }
        }
        Ok(i.new_array(out))
    }, 1, fp);
    // Der Rest der Array-Werkzeugkiste. Gemessen als naechste Wand: `some`
    // allein hat 19 Skripte des Zielkorpus gestoppt.
    def(&array_proto, "some", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        let t = a.get(1).cloned().unwrap_or(Value::Undefined);
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if i.call(&f, t.clone(), &[v, Value::Num(k as f64), this.clone()])?.truthy() {
                return Ok(Value::Bool(true));
            }
        }
        Ok(Value::Bool(false))
    }, 1, fp);
    def(&array_proto, "every", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        let t = a.get(1).cloned().unwrap_or(Value::Undefined);
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if !i.call(&f, t.clone(), &[v, Value::Num(k as f64), this.clone()])?.truthy() {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    }, 1, fp);
    def(&array_proto, "find", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if i.call(&f, Value::Undefined, &[v.clone(), Value::Num(k as f64), this.clone()])?.truthy() {
                return Ok(v);
            }
        }
        Ok(Value::Undefined)
    }, 1, fp);
    def(&array_proto, "findIndex", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if i.call(&f, Value::Undefined, &[v, Value::Num(k as f64), this.clone()])?.truthy() {
                return Ok(Value::Num(k as f64));
            }
        }
        Ok(Value::Num(-1.0))
    }, 1, fp);
    def(&array_proto, "reduce", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        let (mut acc, mut k) = match a.get(1) {
            Some(v) => (v.clone(), 0usize),
            None => {
                if n == 0 { return i.type_err("reduce of empty array with no initial value"); }
                (i.get(&this, "0")?, 1usize)
            }
        };
        while k < n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            acc = i.call(&f, Value::Undefined, &[acc, v, Value::Num(k as f64), this.clone()])?;
            k += 1;
        }
        Ok(acc)
    }, 1, fp);
    def(&array_proto, "includes", |i, this, a| {
        let target = a.first().cloned().unwrap_or(Value::Undefined);
        let n = array_len(i, &this)? as usize;
        for k in 0..n {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if v.same_value(&target) || v.strict_eq(&target) { return Ok(Value::Bool(true)); }
        }
        Ok(Value::Bool(false))
    }, 1, fp);
    def(&array_proto, "lastIndexOf", |i, this, a| {
        let target = a.first().cloned().unwrap_or(Value::Undefined);
        let n = array_len(i, &this)? as usize;
        for k in (0..n).rev() {
            i.tick()?;
            let v = i.get(&this, &num_to_string(k as f64))?;
            if v.strict_eq(&target) { return Ok(Value::Num(k as f64)); }
        }
        Ok(Value::Num(-1.0))
    }, 1, fp);
    def(&array_proto, "shift", |i, this, _| {
        let items = i.elems(&this)?;
        if items.is_empty() { return Ok(Value::Undefined); }
        let first = items[0].clone();
        rebuild(i, &this, items[1..].to_vec())?;
        Ok(first)
    }, 0, fp);
    def(&array_proto, "unshift", |i, this, a| {
        let mut items = a.to_vec();
        items.extend(i.elems(&this)?);
        let n = items.len();
        rebuild(i, &this, items)?;
        Ok(Value::Num(n as f64))
    }, 1, fp);
    def(&array_proto, "reverse", |i, this, _| {
        let mut items = i.elems(&this)?;
        items.reverse();
        rebuild(i, &this, items)?;
        Ok(this)
    }, 0, fp);
    def(&array_proto, "splice", |i, this, a| {
        let items = i.elems(&this)?;
        let n = items.len() as i64;
        let start = match a.first() { None => 0,
            Some(v) => { let r = to_integer(i.to_number(v)?) as i64;
                         if r < 0 { (n + r).max(0) } else { r.min(n) } } };
        let del = match a.get(1) { None => n - start,
            Some(v) => (to_integer(i.to_number(v)?) as i64).clamp(0, n - start) };
        let removed: Vec<Value> = items[start as usize..(start + del) as usize].to_vec();
        let mut out: Vec<Value> = items[..start as usize].to_vec();
        out.extend(a.get(2..).unwrap_or(&[]).iter().cloned());
        out.extend(items[(start + del) as usize..].iter().cloned());
        rebuild(i, &this, out)?;
        Ok(i.new_array(removed))
    }, 2, fp);
    def(&array_proto, "sort", |i, this, a| {
        let mut items = i.elems(&this)?;
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        // Einfuegesortierung: sie ruft den Vergleicher genauso oft wie noetig
        // und braucht keinen Ausleih-Trick, um waehrend des Sortierens in die
        // Maschine zurueckzurufen — ein `sort_by` mit `?` im Vergleicher geht
        // in Rust nicht ohne Verrenkung.
        for k in 1..items.len() {
            let mut j = k;
            while j > 0 {
                i.tick()?;
                let (x, y) = (items[j - 1].clone(), items[j].clone());
                let swap = if i.is_callable(&f) {
                    let r = i.call(&f, Value::Undefined, &[x.clone(), y.clone()])?;
                    i.to_number(&r)? > 0.0
                } else {
                    i.to_string(&x)? > i.to_string(&y)?
                };
                if !swap { break; }
                items.swap(j - 1, j);
                j -= 1;
            }
        }
        rebuild(i, &this, items)?;
        Ok(this)
    }, 1, fp);
    def(&array_proto, "concat", |i, this, a| {
        let mut out = i.elems(&this)?;
        for v in a {
            if matches!(v, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::Array)) {
                out.extend(i.elems(v)?);
            } else { out.push(v.clone()); }
        }
        Ok(i.new_array(out))
    }, 1, fp);

    // ── String.prototype ─────────────────────────────────────────────────
    let this_str = |i: &mut Interp, this: &Value| -> C<Rc<str>> {
        match this {
            Value::Str(s) => Ok(s.clone()),
            Value::Obj(o) => {
                if let ObjKind::StrWrap(s) = &o.borrow().kind { return Ok(s.clone()); }
                i.to_string(this)
            }
            _ => i.to_string(this),
        }
    };
    let _ = this_str;
    def(&string_proto, "toString", |i, this, _| {
        if let Value::Obj(o) = &this {
            if let ObjKind::StrWrap(s) = &o.borrow().kind { return Ok(Value::Str(s.clone())); }
        }
        if matches!(this, Value::Str(_)) { return Ok(this); }
        i.type_err("String.prototype.toString on a non-string")
    }, 0, fp);
    def(&string_proto, "valueOf", |i, this, _| {
        if let Value::Obj(o) = &this {
            if let ObjKind::StrWrap(s) = &o.borrow().kind { return Ok(Value::Str(s.clone())); }
        }
        if matches!(this, Value::Str(_)) { return Ok(this); }
        i.type_err("String.prototype.valueOf on a non-string")
    }, 0, fp);
    def(&string_proto, "charAt", |i, this, a| {
        let s = i.to_string(&this)?;
        let n = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?) as usize;
        Ok(match s.chars().nth(n) { Some(c) => { let mut t = String::new(); t.push(c); Value::string(t) }
                                    None => Value::str("") })
    }, 1, fp);
    def(&string_proto, "charCodeAt", |i, this, a| {
        let s = i.to_string(&this)?;
        let n = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?) as usize;
        Ok(match s.chars().nth(n) { Some(c) => Value::Num(c as u32 as f64), None => Value::Num(f64::NAN) })
    }, 1, fp);
    def(&string_proto, "indexOf", |i, this, a| {
        let s = i.to_string(&this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Num(match s.find(&*t) {
            Some(b) => s[..b].chars().count() as f64, None => -1.0 }))
    }, 1, fp);
    def(&string_proto, "includes", |i, this, a| {
        let s = i.to_string(&this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(s.contains(&*t)))
    }, 1, fp);
    def(&string_proto, "split", |i, this, a| {
        let s = i.to_string(&this)?;
        let parts: Vec<Value> = match a.first() {
            None | Some(Value::Undefined) => vec![Value::Str(s)],
            Some(v) => {
                let sep = i.to_string(v)?;
                if sep.is_empty() {
                    s.chars().map(|c| { let mut t = String::new(); t.push(c); Value::string(t) }).collect()
                } else { s.split(&*sep).map(Value::str).collect() }
            }
        };
        Ok(i.new_array(parts))
    }, 2, fp);
    def(&string_proto, "substring", |i, this, a| {
        let s = i.to_string(&this)?;
        let n = s.chars().count() as i64;
        let g = |v: Option<&Value>, d: i64, i: &mut Interp| -> C<i64> {
            Ok(match v { None | Some(Value::Undefined) => d,
                Some(x) => to_integer(i.to_number(x)?).max(0.0).min(n as f64) as i64 })
        };
        let (mut a0, mut b0) = (g(a.first(), 0, i)?, g(a.get(1), n, i)?);
        if a0 > b0 { core::mem::swap(&mut a0, &mut b0); }
        Ok(Value::string(s.chars().skip(a0 as usize).take((b0 - a0) as usize).collect()))
    }, 2, fp);
    def(&string_proto, "startsWith", |i, this, a| {
        let s = i.to_string(&this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(s.starts_with(&*t)))
    }, 1, fp);
    def(&string_proto, "endsWith", |i, this, a| {
        let s = i.to_string(&this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(s.ends_with(&*t)))
    }, 1, fp);
    def(&string_proto, "slice", |i, this, a| {
        let s = i.to_string(&this)?;
        let n = s.chars().count() as i64;
        let g = |v: Option<&Value>, d: i64, i: &mut Interp| -> C<i64> {
            Ok(match v { None | Some(Value::Undefined) => d,
                Some(x) => { let r = to_integer(i.to_number(x)?) as i64;
                             if r < 0 { (n + r).max(0) } else { r.min(n) } } })
        };
        let (a0, b0) = (g(a.first(), 0, i)?, g(a.get(1), n, i)?);
        if a0 >= b0 { return Ok(Value::str("")); }
        Ok(Value::string(s.chars().skip(a0 as usize).take((b0 - a0) as usize).collect()))
    }, 2, fp);
    def(&string_proto, "repeat", |i, this, a| {
        let s = i.to_string(&this)?;
        let n = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?);
        if n < 0.0 || !n.is_finite() { return i.range_err("invalid count value"); }
        // Ein Deckel, weil `"x".repeat(2**31)` sonst den Speicher frisst und
        // eine gescheiterte Allokation ein ABBRUCH ist, kein Fehler.
        if (n as usize).saturating_mul(s.len()) > (1 << 24) {
            return i.range_err("resulting string too large");
        }
        let mut out = String::new();
        for _ in 0..(n as usize) { i.tick()?; out.push_str(&s); }
        Ok(Value::string(out))
    }, 1, fp);
    def(&string_proto, "replace", |i, this, a| {
        // Ohne RegExp: nur der Fall "Zeichenkette durch Zeichenkette, einmal".
        // Ein Muster als erstes Argument wirft, statt still nichts zu tun.
        let s = i.to_string(&this)?;
        let pat = match a.first() {
            Some(Value::Str(p)) => p.clone(),
            _ => return i.type_err("String.prototype.replace needs a string pattern"),
        };
        let rep = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        Ok(Value::string(match s.find(&*pat) {
            Some(k) => { let mut o = String::from(&s[..k]); o.push_str(&rep);
                         o.push_str(&s[k + pat.len()..]); o }
            None => s.to_string(),
        }))
    }, 2, fp);
    def(&string_proto, "lastIndexOf", |i, t, a| {
        let s = i.to_string(&t)?;
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Num(match s.rfind(&*n) { Some(b) => s[..b].chars().count() as f64, None => -1.0 }))
    }, 1, fp);
    def(&string_proto, "at", |i, t, a| {
        let s = i.to_string(&t)?;
        let len = s.chars().count() as i64;
        let mut k = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?) as i64;
        if k < 0 { k += len; }
        Ok(match s.chars().nth(k.max(0) as usize) {
            Some(c) if k >= 0 && k < len => { let mut x = String::new(); x.push(c); Value::string(x) }
            _ => Value::Undefined })
    }, 1, fp);
    def(&string_proto, "trimStart", |i, t, _| {
        let s = i.to_string(&t)?; Ok(Value::str(s.trim_start()))
    }, 0, fp);
    def(&string_proto, "trimEnd", |i, t, _| {
        let s = i.to_string(&t)?; Ok(Value::str(s.trim_end()))
    }, 0, fp);
    def(&string_proto, "padStart", |i, t, a| pad(i, t, a, true), 2, fp);
    def(&string_proto, "padEnd", |i, t, a| pad(i, t, a, false), 2, fp);
    def(&string_proto, "concat", |i, t, a| {
        let mut s = i.to_string(&t)?.to_string();
        for v in a { s.push_str(&i.to_string(v)?); }
        Ok(Value::string(s))
    }, 1, fp);
    def(&string_proto, "toUpperCase", |i, this, _| {
        let s = i.to_string(&this)?; Ok(Value::string(s.to_uppercase()))
    }, 0, fp);
    def(&string_proto, "toLowerCase", |i, this, _| {
        let s = i.to_string(&this)?; Ok(Value::string(s.to_lowercase()))
    }, 0, fp);
    def(&string_proto, "trim", |i, this, _| {
        let s = i.to_string(&this)?; Ok(Value::str(s.trim()))
    }, 0, fp);

    // ── Number / Boolean prototypes ──────────────────────────────────────
    def(&number_proto, "valueOf", |i, this, _| {
        if let Value::Obj(o) = &this {
            if let ObjKind::NumWrap(n) = &o.borrow().kind { return Ok(Value::Num(*n)); }
        }
        if matches!(this, Value::Num(_)) { return Ok(this); }
        i.type_err("Number.prototype.valueOf on a non-number")
    }, 0, fp);
    def(&number_proto, "toString", |i, this, _| {
        let v = if let Value::Obj(o) = &this {
            match &o.borrow().kind { ObjKind::NumWrap(n) => Value::Num(*n), _ => this.clone() }
        } else { this.clone() };
        let n = i.to_number(&v)?;
        Ok(Value::string(num_to_string(n)))
    }, 1, fp);
    def(&boolean_proto, "valueOf", |i, this, _| {
        if let Value::Obj(o) = &this {
            if let ObjKind::BoolWrap(b) = &o.borrow().kind { return Ok(Value::Bool(*b)); }
        }
        if matches!(this, Value::Bool(_)) { return Ok(this); }
        i.type_err("Boolean.prototype.valueOf on a non-boolean")
    }, 0, fp);
    def(&boolean_proto, "toString", |i, this, _| {
        let b = i.to_string(&this)?; Ok(Value::Str(b))
    }, 0, fp);

    // ── Konstruktoren + globale Funktionen ───────────────────────────────
    let object_ctor = native(Some(function_proto.clone()), |i, _, a| {
        match a.first() {
            None | Some(Value::Undefined) | Some(Value::Null) =>
                Ok(Value::Obj(new_obj(Some(i.realm.object_proto.clone())))),
            Some(v) => Ok(Value::Obj(i.to_object(v)?)),
        }
    }, "Object", 1, true);
    object_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(object_proto.clone())));
    object_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(object_ctor.clone())));
    def(&object_ctor, "keys", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let keys: Vec<Value> = o.borrow().own_keys().into_iter()
            .filter(|k| o.borrow().get_own(k).map(|p| p.enumerable).unwrap_or(false))
            .map(Value::Str).collect();
        Ok(i.new_array(keys))
    }, 1, fp);
    def(&object_ctor, "getOwnPropertyNames", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let keys: Vec<Value> = o.borrow().own_keys().into_iter().map(Value::Str).collect();
        Ok(i.new_array(keys))
    }, 1, fp);
    def(&object_ctor, "getOwnPropertySymbols", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let keys = o.borrow().own_sym_keys();
        // Aus dem Schluessel zurueck auf das Symbol: er traegt Beschreibung
        // und Registrierung, und er IST die Identitaet — ein hier gebautes
        // `SymData` ist damit `===` zum urspruenglichen.
        let out: Vec<Value> = keys.into_iter()
            .map(|k| Value::Sym(Rc::new(sym_from_key(&k)))).collect();
        Ok(i.new_array(out))
    }, 1, fp);
    def(&object_ctor, "getPrototypeOf", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let p = o.borrow().proto.clone();
        Ok(match p { Some(x) => Value::Obj(x), None => Value::Null })
    }, 1, fp);
    def(&object_ctor, "create", |i, _, a| {
        let proto = match a.first() {
            Some(Value::Obj(o)) => Some(o.clone()),
            Some(Value::Null) => None,
            _ => return i.type_err("Object.create needs an object or null"),
        };
        Ok(Value::Obj(new_obj(proto)))
    }, 2, fp);
    def(&object_ctor, "defineProperty", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else { return i.type_err("Object.defineProperty on a non-object") };
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let d = a.get(2).cloned().unwrap_or(Value::Undefined);
        let Value::Obj(_) = &d else { return i.type_err("property descriptor must be an object") };
        let get = i.get(&d, "get")?;
        let set = i.get(&d, "set")?;
        let has = |i: &mut Interp, n: &str| -> bool {
            matches!(&d, Value::Obj(dd) if dd.borrow().has_own(n)) && { let _ = i; true }
        };
        let p = Prop {
            value: if has(i, "value") { Some(i.get(&d, "value")?) } else { None },
            get: if matches!(get, Value::Undefined) { None } else { Some(get) },
            set: if matches!(set, Value::Undefined) { None } else { Some(set) },
            writable: i.get(&d, "writable")?.truthy(),
            enumerable: i.get(&d, "enumerable")?.truthy(),
            configurable: i.get(&d, "configurable")?.truthy(),
        };
        o.borrow_mut().set_prop(k, p);
        Ok(a[0].clone())
    }, 3, fp);
    def(&object_ctor, "getOwnPropertyDescriptor", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let Some(p) = o.borrow().get_own(&k).cloned() else { return Ok(Value::Undefined) };
        let d = new_obj(Some(i.realm.object_proto.clone()));
        {
            let mut b = d.borrow_mut();
            if p.is_accessor() {
                b.define("get", Prop::data(p.get.clone().unwrap_or(Value::Undefined)));
                b.define("set", Prop::data(p.set.clone().unwrap_or(Value::Undefined)));
            } else {
                b.define("value", Prop::data(p.value.clone().unwrap_or(Value::Undefined)));
                b.define("writable", Prop::data(Value::Bool(p.writable)));
            }
            b.define("enumerable", Prop::data(Value::Bool(p.enumerable)));
            b.define("configurable", Prop::data(Value::Bool(p.configurable)));
        }
        Ok(Value::Obj(d))
    }, 2, fp);
    def(&object_ctor, "assign", |i, _, a| {
        let target = a.first().cloned().unwrap_or(Value::Undefined);
        for src in a.get(1..).unwrap_or(&[]) {
            let Value::Obj(o) = src else { continue };
            // Zeichenketten UND Symbole: `assign` kopiert jede eigene
            // aufzaehlbare Eigenschaft, und ein Symbol ist eine.
            let mut keys = o.borrow().own_keys();
            keys.extend(o.borrow().own_sym_keys());
            for k in keys {
                let enumerable = o.borrow().get_own(&k).map(|p| p.enumerable).unwrap_or(false);
                if !enumerable { continue; }
                let v = i.get(src, &k)?;
                i.set(&target, &k, v)?;
            }
        }
        Ok(target)
    }, 2, fp);
    def(&object_ctor, "values", |i, _, a| {
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        let o = i.to_object(&src)?;
        let keys = o.borrow().own_keys();
        let mut out = Vec::new();
        for k in keys {
            if !o.borrow().get_own(&k).map(|p| p.enumerable).unwrap_or(false) { continue; }
            out.push(i.get(&src, &k)?);
        }
        Ok(i.new_array(out))
    }, 1, fp);
    def(&object_ctor, "entries", |i, _, a| {
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        let o = i.to_object(&src)?;
        let keys = o.borrow().own_keys();
        let mut out = Vec::new();
        for k in keys {
            if !o.borrow().get_own(&k).map(|p| p.enumerable).unwrap_or(false) { continue; }
            let v = i.get(&src, &k)?;
            out.push(i.new_array(alloc::vec![Value::Str(k), v]));
        }
        Ok(i.new_array(out))
    }, 1, fp);
    def(&object_ctor, "fromEntries", |i, _, a| {
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        let items = i.iterate(&src)?;
        let g = new_obj(Some(i.realm.object_proto.clone()));
        for it in items {
            let k = i.get(&it, "0")?;
            let v = i.get(&it, "1")?;
            let ks = i.to_string(&k)?;
            g.borrow_mut().set_prop(ks, Prop::data(v));
        }
        Ok(Value::Obj(g))
    }, 1, fp);
    def(&object_ctor, "setPrototypeOf", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return Ok(a.first().cloned().unwrap_or(Value::Undefined));
        };
        let new_proto = match a.get(1) { Some(Value::Obj(p)) => Some(p.clone()), _ => None };
        // Ein ZYKLUS ist zu verweigern, nicht zu bauen (ES 10.4.7.1). Ohne
        // diese Pruefung legt `Object.setPrototypeOf(Object.prototype, {})`
        // die Maschine still lahm: jeder Eigenschaftszugriff laeuft danach im
        // Kreis, in nativem Code, an der Schrittgrenze vorbei.
        let mut cur = new_proto.clone();
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > crate::js::interp::MAX_PROTO_CHAIN || Rc::ptr_eq(&c, o) {
                return i.type_err("cyclic prototype chain");
            }
            let n = c.borrow().proto.clone();
            cur = n;
        }
        if !o.borrow().extensible {
            return i.type_err("cannot set prototype of a non-extensible object");
        }
        o.borrow_mut().proto = new_proto;
        Ok(a[0].clone())
    }, 2, fp);
    def(&object_ctor, "is", |_, _, a| {
        let x = a.first().cloned().unwrap_or(Value::Undefined);
        let y = a.get(1).cloned().unwrap_or(Value::Undefined);
        Ok(Value::Bool(x.same_value(&y)))
    }, 2, fp);
    def(&object_ctor, "freeze", |_, _, a| {
        if let Some(Value::Obj(o)) = a.first() {
            o.borrow_mut().extensible = false;
            let keys = o.borrow().own_keys();
            for k in keys {
                // Die Ausleihe MUSS vor dem Schreiben enden. Als `if let`
                // geschrieben lebt die Leihgabe bis zum Ende des Rumpfes und
                // das `borrow_mut` darin paniked — 94 Abstuerze im Lauf.
                let existing = o.borrow().get_own(&k).cloned();
                if let Some(mut p) = existing {
                    p.writable = false; p.configurable = false;
                    o.borrow_mut().set_prop(k, p);
                }
            }
        }
        Ok(a.first().cloned().unwrap_or(Value::Undefined))
    }, 1, fp);
    def(&object_ctor, "preventExtensions", |_, _, a| {
        if let Some(Value::Obj(o)) = a.first() { o.borrow_mut().extensible = false; }
        Ok(a.first().cloned().unwrap_or(Value::Undefined))
    }, 1, fp);
    def(&object_ctor, "isExtensible", |_, _, a| {
        Ok(Value::Bool(matches!(a.first(), Some(Value::Obj(o)) if o.borrow().extensible)))
    }, 1, fp);
    global.borrow_mut().define("Object", Prop::builtin(Value::Obj(object_ctor)));

    let array_ctor = native(Some(function_proto.clone()), |i, _, a| {
        if a.len() == 1 {
            if let Value::Num(n) = &a[0] {
                let arr = i.new_array(Vec::new());
                i.set(&arr, "length", Value::Num(*n))?;
                return Ok(arr);
            }
        }
        Ok(i.new_array(a.to_vec()))
    }, "Array", 1, true);
    array_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(array_proto.clone())));
    array_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(array_ctor.clone())));
    def(&array_ctor, "isArray", |_, _, a| {
        Ok(Value::Bool(matches!(a.first(), Some(Value::Obj(o)) if matches!(o.borrow().kind, ObjKind::Array))))
    }, 1, fp);
    def(&array_ctor, "of", |i, _, a| Ok(i.new_array(a.to_vec())), 0, fp);
    // `from` nimmt BEIDES: den Iterator, wenn es einen gibt, sonst
    // `length`+Indizes. Das ist keine Nachsicht, das steht so in der
    // Spezifikation — und ein `NodeList` ohne `Symbol.iterator` haengt genau
    // daran.
    def(&array_ctor, "from", |i, _, a| {
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if !matches!(f, Value::Undefined) && !i.is_callable(&f) {
            return i.type_err("Array.from: mapper is not a function");
        }
        let m = if matches!(src, Value::Undefined | Value::Null) { Value::Undefined }
                else { i.get(&src, SYM_ITERATOR)? };
        let items = if i.is_callable(&m) { i.iterate(&src)? } else { i.elems(&src)? };
        let mut out = Vec::with_capacity(items.len());
        for (n, v) in items.into_iter().enumerate() {
            out.push(if i.is_callable(&f) {
                i.call(&f, Value::Undefined, &[v, Value::Num(n as f64)])?
            } else { v });
        }
        Ok(i.new_array(out))
    }, 1, fp);
    global.borrow_mut().define("Array", Prop::builtin(Value::Obj(array_ctor)));

    let string_ctor = native(Some(function_proto.clone()), |i, _, a| {
        Ok(match a.first() {
            None => Value::str(""),
            // `String(sym)` ist die AUSNAHME: sie darf, wo `"" + sym` wirft.
            // Genau so steht es in der Spezifikation, und es ist der einzige
            // Weg, ein Symbol absichtlich zu Text zu machen.
            Some(Value::Sym(sd)) => Value::Str(Interp::sym_to_display(sd)),
            Some(v) => Value::Str(i.to_string(v)?),
        })
    }, "String", 1, true);
    string_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(string_proto.clone())));
    string_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(string_ctor.clone())));
    def(&string_ctor, "fromCharCode", |i, _, a| {
        let mut s = String::new();
        for v in a { let n = i.to_number(v)? as u32; if let Some(c) = char::from_u32(n) { s.push(c); } }
        Ok(Value::string(s))
    }, 1, fp);
    global.borrow_mut().define("String", Prop::builtin(Value::Obj(string_ctor)));

    let number_ctor = native(Some(function_proto.clone()), |i, _, a| {
        Ok(Value::Num(match a.first() { None => 0.0, Some(v) => i.to_number(v)? }))
    }, "Number", 1, true);
    number_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(number_proto.clone())));
    number_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(number_ctor.clone())));
    for (n, v) in [("MAX_SAFE_INTEGER", 9007199254740991.0), ("MIN_SAFE_INTEGER", -9007199254740991.0),
                   ("MAX_VALUE", f64::MAX), ("MIN_VALUE", 5e-324), ("EPSILON", f64::EPSILON),
                   ("POSITIVE_INFINITY", f64::INFINITY), ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
                   ("NaN", f64::NAN)] {
        number_ctor.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    global.borrow_mut().define("Number", Prop::builtin(Value::Obj(number_ctor)));

    let bool_ctor = native(Some(function_proto.clone()), |_, _, a| {
        Ok(Value::Bool(a.first().map(|v| v.truthy()).unwrap_or(false)))
    }, "Boolean", 1, true);
    bool_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(boolean_proto.clone())));
    boolean_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(bool_ctor.clone())));
    global.borrow_mut().define("Boolean", Prop::builtin(Value::Obj(bool_ctor)));

    // ── Math ─────────────────────────────────────────────────────────────
    let math = new_obj(Some(object_proto.clone()));
    for (n, v) in [("PI", core::f64::consts::PI), ("E", core::f64::consts::E),
                   ("LN2", core::f64::consts::LN_2), ("LN10", core::f64::consts::LN_10),
                   ("SQRT2", core::f64::consts::SQRT_2)] {
        math.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    def(&math, "abs", |i, _, a| Ok(Value::Num(libm::fabs(i.to_number(a.first().unwrap_or(&Value::Undefined))?))), 1, fp);
    def(&math, "floor", |i, _, a| Ok(Value::Num(libm::floor(i.to_number(a.first().unwrap_or(&Value::Undefined))?))), 1, fp);
    def(&math, "ceil", |i, _, a| Ok(Value::Num(libm::ceil(i.to_number(a.first().unwrap_or(&Value::Undefined))?))), 1, fp);
    def(&math, "trunc", |i, _, a| Ok(Value::Num(libm::trunc(i.to_number(a.first().unwrap_or(&Value::Undefined))?))), 1, fp);
    def(&math, "sqrt", |i, _, a| Ok(Value::Num(libm::sqrt(i.to_number(a.first().unwrap_or(&Value::Undefined))?))), 1, fp);
    def(&math, "pow", |i, _, a| {
        let x = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
        let y = i.to_number(a.get(1).unwrap_or(&Value::Undefined))?;
        Ok(Value::Num(if y == 0.0 { 1.0 } else { libm::pow(x, y) }))
    }, 2, fp);
    def(&math, "max", |i, _, a| {
        let mut m = f64::NEG_INFINITY;
        for v in a { let n = i.to_number(v)?; if n.is_nan() { return Ok(Value::Num(f64::NAN)); } if n > m { m = n; } }
        Ok(Value::Num(m))
    }, 2, fp);
    def(&math, "min", |i, _, a| {
        let mut m = f64::INFINITY;
        for v in a { let n = i.to_number(v)?; if n.is_nan() { return Ok(Value::Num(f64::NAN)); } if n < m { m = n; } }
        Ok(Value::Num(m))
    }, 2, fp);
    math.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("Math")));
    global.borrow_mut().define("Math", Prop::builtin(Value::Obj(math)));

    // ── Globale Werte + Funktionen ───────────────────────────────────────
    global.borrow_mut().define("undefined", Prop::frozen(Value::Undefined));
    global.borrow_mut().define("NaN", Prop::frozen(Value::Num(f64::NAN)));
    global.borrow_mut().define("Infinity", Prop::frozen(Value::Num(f64::INFINITY)));
    global.borrow_mut().define("globalThis", Prop::builtin(Value::Obj(global.clone())));
    def(&global, "isNaN", |i, _, a| Ok(Value::Bool(i.to_number(a.first().unwrap_or(&Value::Undefined))?.is_nan())), 1, fp);
    def(&global, "isFinite", |i, _, a| Ok(Value::Bool(i.to_number(a.first().unwrap_or(&Value::Undefined))?.is_finite())), 1, fp);
    def(&global, "parseInt", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let t = s.trim();
        // `to_digit` paniked bei einer Basis ueber 36 — in `core`, also ohne
        // Netz. Ausserhalb von 2..=36 ist das Ergebnis NaN (ES 19.2.5).
        let radix = match a.get(1) { None | Some(Value::Undefined) => 10,
            Some(v) => {
                let r = to_integer(i.to_number(v)?);
                if r == 0.0 { 10 }
                else if !(2.0..=36.0).contains(&r) { return Ok(Value::Num(f64::NAN)); }
                else { r as u32 }
            } };
        let (neg, body) = match t.strip_prefix('-') { Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(t)) };
        let body = if radix == 16 { body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")).unwrap_or(body) } else { body };
        let end = body.char_indices().find(|(_, c)| c.to_digit(radix).is_none()).map(|(i, _)| i).unwrap_or(body.len());
        if end == 0 { return Ok(Value::Num(f64::NAN)); }
        let mut v = 0f64;
        for c in body[..end].chars() { v = v * radix as f64 + c.to_digit(radix).unwrap() as f64; }
        Ok(Value::Num(if neg { -v } else { v }))
    }, 2, fp);
    def(&global, "parseFloat", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let t = s.trim();
        let end = t.char_indices()
            .take_while(|(k, c)| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+'
                || *c == 'e' || *c == 'E' || (*k == 0 && *c == '-'))
            .map(|(k, c)| k + c.len_utf8()).last().unwrap_or(0);
        Ok(Value::Num(t[..end].parse::<f64>().unwrap_or(f64::NAN)))
    }, 1, fp);

    // ── Map und Set ──────────────────────────────────────────────────────
    //
    // Auf einem gewoehnlichen Objekt aufgesetzt: die Eintraege liegen unter
    // einem Praefix, das kein Skript sieht. Der Preis ist ehrlich zu nennen —
    // ein `Map`-Schluessel ist damit seine ZEICHENKETTE, nicht seine
    // Identitaet. `m.set({}, 1); m.set({}, 2)` hat bei uns EINEN Eintrag, in
    // einem Browser zwei. Fuer Konfigurationskarten (und genau dafuer
    // benutzen die Zielseiten es) stimmt es; fuer Objektschluessel nicht.
    // `native` nimmt einen Funktionszeiger, keinen Abschluss — deshalb kommt
    // der Konstruktor von aussen herein statt hier eingefangen zu werden.
    let collection = |name: &'static str, is_map: bool, ctor_fn: NativeFn, object_proto: &Gc,
                      function_proto: &Gc, global: &Gc| {
        let proto = new_obj(Some(object_proto.clone()));
        let d = |o: &Gc, n: &str, f: NativeFn, l: usize| {
            let g = native(Some(function_proto.clone()), f, n, l, false);
            o.borrow_mut().define(n, Prop::builtin(Value::Obj(g)));
        };
        d(&proto, "has", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("@{k}");
            Ok(Value::Bool(matches!(&t, Value::Obj(o) if o.borrow().has_own(&key))))
        }, 1);
        d(&proto, "get", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("@{k}");
            Ok(match i.get(&t, &key)? { Value::Undefined => Value::Undefined, v => v })
        }, 1);
        d(&proto, "set", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let v = a.get(1).cloned().unwrap_or(Value::Undefined);
            if let Value::Obj(o) = &t {
                o.borrow_mut().define(&alloc::format!("@{k}"), Prop {
                    value: Some(v), get: None, set: None,
                    writable: true, enumerable: false, configurable: true });
            }
            Ok(t)
        }, 2);
        d(&proto, "add", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Value::Obj(o) = &t {
                o.borrow_mut().define(&alloc::format!("@{k}"), Prop {
                    value: Some(Value::Str(k)), get: None, set: None,
                    writable: true, enumerable: false, configurable: true });
            }
            Ok(t)
        }, 1);
        d(&proto, "delete", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("@{k}");
            Ok(Value::Bool(matches!(&t, Value::Obj(o) if o.borrow_mut().remove(&key))))
        }, 1);
        d(&proto, "clear", |_, t, _| {
            if let Value::Obj(o) = &t {
                let ks = o.borrow().own_keys();
                for k in ks { if k.starts_with('@') { o.borrow_mut().remove(&k); } }
            }
            Ok(Value::Undefined)
        }, 0);
        d(&proto, "forEach", |i, t, a| {
            let f = a.first().cloned().unwrap_or(Value::Undefined);
            if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
            let Value::Obj(o) = &t else { return Ok(Value::Undefined) };
            let ks: Vec<Rc<str>> = o.borrow().own_keys().into_iter()
                .filter(|k| k.starts_with('@')).collect();
            for k in ks {
                i.tick()?;
                let v = i.get(&t, &k)?;
                i.call(&f, Value::Undefined, &[v, Value::str(&k[1..]), t.clone()])?;
            }
            Ok(Value::Undefined)
        }, 1);
        // Die drei Sichten. Eine Umsetzung fuer Map UND Set: bei einem Set
        // IST der Wert der Schluessel, `entries` liefert dort also `[v, v]` —
        // und genau das schreibt die Spezifikation vor.
        //
        // Der Iterator laeuft ueber eine MOMENTAUFNAHME. Ein echter
        // Map-Iterator sieht spaetere Eintraege noch; unsere Karte ist
        // ohnehin die zeichenkettenbasierte Naeherung von oben, und eine
        // Aufnahme ist die ehrlichere Naeherung als ein Index, der bei jedem
        // `next` neu ueber die Schluesselliste laeuft.
        d(&proto, "keys", |i, t, _| { let v = coll_view(i, &t, 0)?; i.array_iter(v, 0) }, 0);
        d(&proto, "values", |i, t, _| { let v = coll_view(i, &t, 1)?; i.array_iter(v, 0) }, 0);
        d(&proto, "entries", |i, t, _| { let v = coll_view(i, &t, 2)?; i.array_iter(v, 0) }, 0);
        {
            let key = if is_map { "entries" } else { "values" };
            let f = proto.borrow().get_own(key).and_then(|p| p.value.clone());
            if let Some(f) = f { proto.borrow_mut().define(SYM_ITERATOR, Prop::builtin(f)); }
        }
        proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str(name)));

        let size = native(Some(function_proto.clone()), |_, t, _| {
            let Value::Obj(o) = &t else { return Ok(Value::Num(0.0)) };
            let n = o.borrow().own_keys().iter().filter(|k| k.starts_with('@')).count();
            Ok(Value::Num(n as f64))
        }, "size", 0, false);
        proto.borrow_mut().define("size", Prop {
            value: None, get: Some(Value::Obj(size)), set: None,
            writable: false, enumerable: false, configurable: true });

        let ctor = native(Some(function_proto.clone()), ctor_fn, name, 0, true);
        ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
        proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
        global.borrow_mut().define(name, Prop::builtin(Value::Obj(ctor)));
    };
    collection("Map", true, |i, _, a| coll_new(i, "Map", true, a), &object_proto, &function_proto, &global);
    collection("Set", false, |i, _, a| coll_new(i, "Set", false, a), &object_proto, &function_proto, &global);
    collection("WeakMap", true, |i, _, a| coll_new(i, "WeakMap", true, a), &object_proto, &function_proto, &global);
    collection("WeakSet", false, |i, _, a| coll_new(i, "WeakSet", false, a), &object_proto, &function_proto, &global);

    // ── Symbol ───────────────────────────────────────────────────────────
    //
    // Ein Symbol ist ein PRIMITIV (`Value::Sym`), kein Objekt. Sein
    // Eigenschaftsname liegt als NUL-praefigierte Zeichenkette in derselben
    // Tabelle wie jeder andere — die Begruendung steht bei `PropName`.
    let symbol_ctor = native(Some(function_proto.clone()), |i, _, a| {
        let desc = match a.first() {
            None | Some(Value::Undefined) => None,
            Some(v) => Some(i.to_string(v)?),
        };
        Ok(i.new_symbol(desc))
    }, "Symbol", 0, false);          // `new Symbol()` wirft — kein Konstruktor
    symbol_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(symbol_proto.clone())));
    symbol_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(symbol_ctor.clone())));
    for (name, key) in WELL_KNOWN {
        let v = Value::Sym(Rc::new(SymData {
            desc: Some(Rc::from(alloc::format!("Symbol.{name}").as_str())),
            key: Rc::from(*key), registered: None }));
        symbol_ctor.borrow_mut().define(name, Prop::frozen(v));
    }
    // `Symbol.for` teilt sich EINE Registrierung ueber alle Aufrufe. Der
    // Schluessel wird aus dem Text abgeleitet und nicht durchgezaehlt —
    // damit ist `Symbol.for("x") === Symbol.for("x")` schon durch die
    // Gleichheit auf `key` wahr, ohne dass die Tabelle befragt werden muss.
    def(&symbol_ctor, "for", |i, _, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        if let Some(v) = i.sym_registry.get(&k) { return Ok(v.clone()); }
        let v = Value::Sym(Rc::new(SymData {
            desc: Some(k.clone()),
            key: Rc::from(alloc::format!("\0*{k}").as_str()),
            registered: Some(k.clone()) }));
        i.sym_registry.insert(k, v.clone());
        Ok(v)
    }, 1, fp);
    def(&symbol_ctor, "keyFor", |i, _, a| {
        match a.first() {
            Some(Value::Sym(sd)) => Ok(match &sd.registered {
                Some(k) => Value::Str(k.clone()), None => Value::Undefined }),
            _ => i.type_err("Symbol.keyFor requires a symbol"),
        }
    }, 1, fp);
    global.borrow_mut().define("Symbol", Prop::builtin(Value::Obj(symbol_ctor)));

    // `this` ist entweder das Primitiv oder seine Huelle — beides muss gehen,
    // weil `sym.toString()` das Primitiv durchreicht, `Object(sym).toString()`
    // aber die Huelle.
    fn this_sym(i: &mut Interp, t: &Value) -> C<Rc<SymData>> {
        match t {
            Value::Sym(sd) => Ok(sd.clone()),
            Value::Obj(o) => match &o.borrow().kind {
                ObjKind::SymWrap(sd) => Ok(sd.clone()),
                _ => i.type_err("not a Symbol"),
            },
            _ => i.type_err("not a Symbol"),
        }
    }
    def(&symbol_proto, "toString", |i, t, _| {
        let sd = this_sym(i, &t)?;
        Ok(Value::Str(Interp::sym_to_display(&sd)))
    }, 0, fp);
    def(&symbol_proto, "valueOf", |i, t, _| {
        let sd = this_sym(i, &t)?;
        Ok(Value::Sym(sd))
    }, 0, fp);
    let desc_get = native(Some(function_proto.clone()), |i, t, _| {
        let sd = this_sym(i, &t)?;
        Ok(match &sd.desc { Some(d) => Value::Str(d.clone()), None => Value::Undefined })
    }, "get description", 0, false);
    symbol_proto.borrow_mut().define("description", Prop {
        value: None, get: Some(Value::Obj(desc_get)), set: None,
        writable: false, enumerable: false, configurable: true });
    symbol_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("Symbol")));
    // `"" + sym` wirft (siehe `to_string`); ohne diese Sperre wuerde
    // `ToPrimitive` erst `valueOf` finden und das Symbol still weiterreichen,
    // statt an der Umwandlung zu scheitern, wo der Fehler hingehoert.
    let sym_prim = native(Some(function_proto.clone()), |i, t, _| {
        let sd = this_sym(i, &t)?;
        Ok(Value::Sym(sd))
    }, "[Symbol.toPrimitive]", 1, false);
    symbol_proto.borrow_mut().define(SYM_TO_PRIMITIVE, Prop::frozen(Value::Obj(sym_prim)));

    // ── Der Iteratorvertrag ──────────────────────────────────────────────
    //
    // `%IteratorPrototype%` traegt nur EINES: sich selbst zurueckzugeben.
    // Genau daran haengt, dass `for (x of arr.entries())` geht — der
    // Iterator muss selbst iterierbar sein.
    let self_iter = native(Some(function_proto.clone()), |_, t, _| Ok(t),
                           "[Symbol.iterator]", 0, false);
    iterator_proto.borrow_mut().define(SYM_ITERATOR, Prop::builtin(Value::Obj(self_iter)));

    // Der Zustand eines eingebauten Iterators liegt als NUL-praefigierte
    // Eigenschaft auf ihm selbst. Kein Skript sieht sie (sie faellt aus
    // `own_keys`), und `native` nimmt ohnehin keinen Abschluss.
    def(&array_iter_proto, "next", |i, t, _| {
        let target = i.get(&t, IT_TARGET)?;
        if matches!(target, Value::Undefined) { return Ok(i.iter_result(Value::Undefined, true)); }
        let idx = i.get(&t, IT_INDEX)?;
        let idx = i.to_number(&idx)? as usize;
        let len = i.get(&target, "length")?;
        let len = i.to_number(&len)? as usize;
        if idx >= len {
            i.set(&t, IT_TARGET, Value::Undefined)?;
            return Ok(i.iter_result(Value::Undefined, true));
        }
        i.set(&t, IT_INDEX, Value::Num(idx as f64 + 1.0))?;
        let kind = i.get(&t, IT_KIND)?;
        let kind = i.to_number(&kind)?;
        let out = if kind == 1.0 {
            Value::Num(idx as f64)
        } else {
            let v = i.get(&target, &num_to_string(idx as f64))?;
            if kind == 2.0 { i.new_array(vec![Value::Num(idx as f64), v]) } else { v }
        };
        Ok(i.iter_result(out, false))
    }, 0, fp);
    array_iter_proto.borrow_mut().define(SYM_TO_STRING_TAG,
        Prop::frozen(Value::str("Array Iterator")));

    // Zeichen fuer Zeichen — nach CODEPOINT, nicht nach Byte. Unsere Texte
    // sind Rust-`str`, ein `char` ist also genau ein Codepunkt; das trifft
    // die Spezifikation auch fuer alles ausserhalb der BMP.
    def(&string_iter_proto, "next", |i, t, _| {
        let s = i.get(&t, IT_TARGET)?;
        let Value::Str(s) = s else { return Ok(i.iter_result(Value::Undefined, true)) };
        let off = i.get(&t, IT_INDEX)?;
        let off = i.to_number(&off)? as usize;
        match s[off..].chars().next() {
            None => {
                i.set(&t, IT_TARGET, Value::Undefined)?;
                Ok(i.iter_result(Value::Undefined, true))
            }
            Some(c) => {
                i.set(&t, IT_INDEX, Value::Num((off + c.len_utf8()) as f64))?;
                let mut b = String::new(); b.push(c);
                Ok(i.iter_result(Value::string(b), false))
            }
        }
    }, 0, fp);
    string_iter_proto.borrow_mut().define(SYM_TO_STRING_TAG,
        Prop::frozen(Value::str("String Iterator")));

    def_sym(&string_proto, SYM_ITERATOR, "[Symbol.iterator]", |i, t, _| {
        let s = i.to_string(&t)?;
        let g = new_obj(Some(i.realm.string_iter_proto.clone()));
        g.borrow_mut().define(IT_TARGET, Prop::frozen(Value::Str(s)));
        g.borrow_mut().define(IT_INDEX, Prop::data(Value::Num(0.0)));
        Ok(Value::Obj(g))
    }, 0, fp);

    def(&array_proto, "values", |i, t, _| i.array_iter(t, 0), 0, fp);
    def(&array_proto, "keys",   |i, t, _| i.array_iter(t, 1), 0, fp);
    def(&array_proto, "entries",|i, t, _| i.array_iter(t, 2), 0, fp);
    // `[Symbol.iterator]` IST `values` — dieselbe Funktion, nicht eine zweite
    // mit gleichem Inhalt: `arr[Symbol.iterator] === arr.values` ist wahr.
    let av = array_proto.borrow().get_own("values").and_then(|p| p.value.clone());
    if let Some(v) = av { array_proto.borrow_mut().define(SYM_ITERATOR, Prop::builtin(v)); }

    // ── Zeitgeber ────────────────────────────────────────────────────────
    //
    // Angemeldet, nicht ausgefuehrt. Sofort zu rufen waere falsch (eine
    // Abfrageschleife wuerde endlos rekursieren), und gar nicht zu haben ist
    // ein Fehler, der das Skript beendet. Die Warteschlange ist die Stelle,
    // an der beaks Ereignisschleife spaeter ansetzt — dieselbe Form wie bei
    // `addEventListener`.
    for n in ["setTimeout", "setInterval", "requestAnimationFrame", "requestIdleCallback"] {
        let g = native(Some(function_proto.clone()), |i, _, a| {
            let f = a.first().cloned().unwrap_or(Value::Undefined);
            if i.is_callable(&f) { i.timers.push(f); }
            Ok(Value::Num(i.timers.len() as f64))
        }, n, 2, false);
        global.borrow_mut().define(n, Prop::builtin(Value::Obj(g)));
    }
    for n in ["clearTimeout", "clearInterval", "cancelAnimationFrame", "cancelIdleCallback"] {
        let g = native(Some(function_proto.clone()), |_, _, _| Ok(Value::Undefined), n, 1, false);
        global.borrow_mut().define(n, Prop::builtin(Value::Obj(g)));
    }

    // `queueMicrotask` steht bewusst NICHT in der Liste darueber: es ist kein
    // Zeitgeber, sondern haengt an DERSELBEN Schlange wie `.then`. Genau
    // dafuer benutzen Seiten es — „nach dem laufenden Skript, aber vor dem
    // naechsten Zeitgeber".
    def(&global, "queueMicrotask", |i, _, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("queueMicrotask needs a function"); }
        let p = super::promise::new_promise(i);
        super::promise::settle(i, &p, Value::Undefined, false);
        super::promise::perform_then(i, &p, f, Value::Undefined);
        Ok(Value::Undefined)
    }, 1, fp);

    // ── Storage ──────────────────────────────────────────────────────────
    //
    // Im Speicher, nicht auf der Platte. Ein Skript, das `localStorage`
    // ABFRAGT (und das tun sie, als Vertraeglichkeitspruefung), bekommt eine
    // Antwort; was es hineinlegt, ueberlebt die Seite nicht. Das ist eine
    // benannte Luecke — beak muesste es an npkFS haengen, und dann waere die
    // Frage, wem der Speicher gehoert.
    let make_storage = |object_proto: &Gc, function_proto: &Gc| -> Gc {
        let st = new_obj(Some(object_proto.clone()));
        let d = |o: &Gc, n: &str, f: NativeFn, l: usize| {
            let g = native(Some(function_proto.clone()), f, n, l, false);
            o.borrow_mut().define(n, Prop::builtin(Value::Obj(g)));
        };
        d(&st, "getItem", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("__s_{k}");
            Ok(match i.get(&t, &key)? { Value::Undefined => Value::Null, v => v })
        }, 1);
        d(&st, "setItem", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("__s_{k}");
            if let Value::Obj(o) = &t {
                o.borrow_mut().define(&key, Prop {
                    value: Some(Value::Str(v)), get: None, set: None,
                    writable: true, enumerable: false, configurable: true });
            }
            Ok(Value::Undefined)
        }, 2);
        d(&st, "removeItem", |i, t, a| {
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("__s_{k}");
            if let Value::Obj(o) = &t { o.borrow_mut().remove(&key); }
            Ok(Value::Undefined)
        }, 1);
        d(&st, "clear", |_, t, _| {
            if let Value::Obj(o) = &t {
                let keys = o.borrow().own_keys();
                for k in keys { if k.starts_with("__s_") { o.borrow_mut().remove(&k); } }
            }
            Ok(Value::Undefined)
        }, 0);
        d(&st, "key", |i, t, a| {
            let n = i.to_number(a.first().unwrap_or(&Value::Undefined))? as usize;
            let Value::Obj(o) = &t else { return Ok(Value::Null) };
            let keys: Vec<Rc<str>> = o.borrow().own_keys().into_iter()
                .filter(|k| k.starts_with("__s_")).collect();
            Ok(match keys.get(n) { Some(k) => Value::str(&k[4..]), None => Value::Null })
        }, 1);
        st
    };
    let ls = make_storage(&object_proto, &function_proto);
    let ss = make_storage(&object_proto, &function_proto);
    global.borrow_mut().define("localStorage", Prop::builtin(Value::Obj(ls)));
    global.borrow_mut().define("sessionStorage", Prop::builtin(Value::Obj(ss)));

    // ── Function ─────────────────────────────────────────────────────────
    // 9261 Tests scheiterten allein an `Function is not defined` — mehr als
    // an jeder anderen einzelnen Ursache. Die meisten greifen nur auf
    // `Function.prototype`; `new Function(args, body)` kostet nochmal zehn
    // Zeilen und ist der Weg, auf dem test262 dynamisch erzeugten Code prueft.
    let function_ctor = native(Some(function_proto.clone()), |i, _, a| {
        let mut params = String::new();
        for (k, v) in a.iter().take(a.len().saturating_sub(1)).enumerate() {
            if k > 0 { params.push(','); }
            params.push_str(&i.to_string(v)?);
        }
        let body = match a.last() { Some(v) => i.to_string(v)?, None => Rc::from("") };
        let src = alloc::format!("(function anonymous({params}\n) {{\n{body}\n}})");
        let prog = match super::parse(&src, false) {
            Ok(p) => p,
            Err(e) => return Err(i.throw_kind("SyntaxError", &e.msg)),
        };
        // Genau EIN Ausdruck, und er ist der Funktionsausdruck oben.
        match prog.body.first() {
            Some(super::ast::Stmt::Expr(e)) => {
                let env = i.realm.global_env.clone();
                i.eval(e, &env)
            }
            _ => Err(i.throw_kind("SyntaxError", "Function body did not compile")),
        }
    }, "Function", 1, true);
    function_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(function_proto.clone())));
    function_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(function_ctor.clone())));
    global.borrow_mut().define("Function", Prop::builtin(Value::Obj(function_ctor)));

    // ── Die Wirtsumgebung ────────────────────────────────────────────────
    //
    // Gemessen, nicht geraten (`examples/wallcheck.rs`): auf Wikipedia stirbt
    // das ERSTE Skript an `performance`, und weil es `mw` haette setzen sollen,
    // sterben die 107 danach an `mw`. EIN fehlender Global kostet eine ganze
    // Seite. Deshalb kommen diese hier zuerst und nicht `Symbol` (2 Skripte).
    //
    // Was sie liefern, ist die FORM, nicht der Inhalt: beak muss Uhr, Adresse
    // und Kennung noch einreichen. Ein Stumpf, der die richtige Form hat, ist
    // trotzdem das, worauf ein Skript prueft.
    global.borrow_mut().define("window", Prop::builtin(Value::Obj(global.clone())));
    global.borrow_mut().define("self", Prop::builtin(Value::Obj(global.clone())));
    global.borrow_mut().define("top", Prop::builtin(Value::Obj(global.clone())));
    global.borrow_mut().define("parent", Prop::builtin(Value::Obj(global.clone())));

    let perf = new_obj(Some(object_proto.clone()));
    // Eine Uhr, die nur steigt. beak reicht die echte nach; bis dahin ist
    // Monotonie das Einzige, worauf sich ein Skript wirklich verlaesst.
    def(&perf, "now", |i, _, _| { i.fake_now += 1.0; Ok(Value::Num(i.fake_now)) }, 0, fp);
    for m in ["mark", "measure", "clearMarks", "clearMeasures"] {
        let g = native(Some(function_proto.clone()), |_, _, _| Ok(Value::Undefined), m, 1, false);
        perf.borrow_mut().define(m, Prop::builtin(Value::Obj(g)));
    }
    for m in ["getEntriesByName", "getEntriesByType", "getEntries"] {
        let g = native(Some(function_proto.clone()), |i, _, _| Ok(i.new_array(Vec::new())), m, 1, false);
        perf.borrow_mut().define(m, Prop::builtin(Value::Obj(g)));
    }
    perf.borrow_mut().define("timeOrigin", Prop::builtin(Value::Num(0.0)));
    global.borrow_mut().define("performance", Prop::builtin(Value::Obj(perf)));

    let console = new_obj(Some(object_proto.clone()));
    // Nicht mehr still. `beak-engine` hat keine Serienleitung, aber der Wirt
    // hat eine: die Zeilen werden gesammelt, und beak holt sie ab. Eine
    // Seite, deren eigene Diagnose ins Leere geht, kann man aus der Ferne
    // nicht befragen — und genau das ist die Lage am Geraet.
    for (m, tag) in [("log", ""), ("info", ""), ("debug", ""), ("dir", ""),
                     ("group", ""), ("groupEnd", ""), ("warn", "warn: "),
                     ("error", "error: "), ("trace", "trace: ")] {
        let f: fn(&mut Interp, Value, &[Value]) -> C<Value> = match tag {
            "warn: " => |i, _, a| { let l = console_join(i, a, "warn: "); i.console_push(l); Ok(Value::Undefined) },
            "error: " => |i, _, a| { let l = console_join(i, a, "error: "); i.console_push(l); Ok(Value::Undefined) },
            "trace: " => |i, _, a| { let l = console_join(i, a, "trace: "); i.console_push(l); Ok(Value::Undefined) },
            _ => |i, _, a| { let l = console_join(i, a, ""); i.console_push(l); Ok(Value::Undefined) },
        };
        let g = native(Some(function_proto.clone()), f, m, 0, false);
        console.borrow_mut().define(m, Prop::builtin(Value::Obj(g)));
    }
    global.borrow_mut().define("console", Prop::builtin(Value::Obj(console)));

    let nav = new_obj(Some(object_proto.clone()));
    nav.borrow_mut().define("userAgent", Prop::builtin(Value::str("Mozilla/5.0 (nopeekOS) beak")));
    nav.borrow_mut().define("language", Prop::builtin(Value::str("de")));
    nav.borrow_mut().define("onLine", Prop::builtin(Value::Bool(true)));
    global.borrow_mut().define("navigator", Prop::builtin(Value::Obj(nav)));

    let loc = new_obj(Some(object_proto.clone()));
    for (k, v) in [("href", "about:blank"), ("protocol", "about:"), ("host", ""),
                   ("hostname", ""), ("pathname", "blank"), ("search", ""), ("hash", "")] {
        loc.borrow_mut().define(k, Prop::builtin(Value::str(v)));
    }
    global.borrow_mut().define("location", Prop::builtin(Value::Obj(loc)));

    // ── Date, klein aber vorhanden ───────────────────────────────────────
    let date_proto = new_obj(Some(object_proto.clone()));
    def(&date_proto, "getTime", |i, this, _| i.get(&this, "__t"), 0, fp);
    def(&date_proto, "valueOf", |i, this, _| i.get(&this, "__t"), 0, fp);
    def(&date_proto, "toString", |_, _, _| Ok(Value::str("Thu Jan 01 1970 00:00:00 GMT+0000")), 0, fp);
    let date_ctor = native(Some(function_proto.clone()), |i, _, a| {
        let t = match a.first() { Some(v) => i.to_number(v)?, None => 0.0 };
        let proto = i.get(&Value::Obj(i.realm.global.clone()), "Date")?;
        let p = match i.get(&proto, "prototype")? { Value::Obj(o) => Some(o), _ => None };
        let d = new_obj(p);
        d.borrow_mut().define("__t", Prop { value: Some(Value::Num(t)), get: None, set: None,
            writable: true, enumerable: false, configurable: false });
        Ok(Value::Obj(d))
    }, "Date", 7, true);
    date_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(date_proto.clone())));
    date_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(date_ctor.clone())));
    def(&date_ctor, "now", |i, _, _| { i.fake_now += 1.0; Ok(Value::Num(i.fake_now)) }, 0, fp);
    global.borrow_mut().define("Date", Prop::builtin(Value::Obj(date_ctor)));

    // Platzhalter — `dombind::install` ersetzt sie sofort. Sie stehen hier,
    // weil ein Realm ohne sie nicht baubar waere und `install` den fertigen
    // Realm braucht, um die Prototypen daranzuhaengen.
    let ph = || new_obj(Some(object_proto.clone()));
    Realm { global, global_env, object_proto: object_proto.clone(), function_proto, array_proto,
            string_proto, number_proto, boolean_proto, error_proto, error_ctors,
            node_proto: ph(), element_proto: ph(), text_proto: ph(), document_proto: ph(),
            regexp_proto: ph(), symbol_proto, iterator_proto, array_iter_proto,
            string_iter_proto, promise_proto: ph() }
}

/// `this.length` als Zahl. Eigene Funktion, weil `i.to_number(&i.get(...))`
/// zwei gleichzeitige Ausleihen waeren — und das Aufteilen an jeder Stelle
/// haette den Code nur laenger gemacht.
/// Ein Array in-place durch eine neue Elementfolge ersetzen. Die Grundlage
/// fuer alles, was die Laenge aendert (`shift`, `splice`, `sort`, `reverse`).
fn rebuild(i: &mut Interp, this: &Value, items: Vec<Value>) -> C<()> {
    let Value::Obj(o) = this else { return Ok(()) };
    o.borrow_mut().clear_indices();
    let n = items.len();
    for (k, v) in items.into_iter().enumerate() {
        o.borrow_mut().set_prop(Rc::from(num_to_string(k as f64).as_str()), Prop::data(v));
    }
    i.set(this, "length", Value::Num(n as f64))
}

/// Der gemeinsame Rumpf der vier Sammlungs-Konstruktoren.
/// Die Eintraege einer Map/eines Sets als Array. `kind`: 0 Schluessel,
/// 1 Werte, 2 Paare.
fn coll_view(i: &mut Interp, t: &Value, kind: u8) -> C<Value> {
    let Value::Obj(o) = t else { return i.type_err("not a collection") };
    let ks: Vec<Rc<str>> = o.borrow().own_keys().into_iter()
        .filter(|k| k.starts_with('@')).collect();
    let mut out = Vec::with_capacity(ks.len());
    for k in ks {
        i.tick()?;
        let key = Value::str(&k[1..]);
        if kind == 0 { out.push(key); continue; }
        let v = i.get(t, &k)?;
        out.push(if kind == 1 { v } else { i.new_array(vec![key, v]) });
    }
    Ok(i.new_array(out))
}

fn coll_new(i: &mut Interp, name: &str, is_map: bool, a: &[Value]) -> C<Value> {
    let pv = i.get(&Value::Obj(i.realm.global.clone()), name)?;
    let proto = match i.get(&pv, "prototype")? { Value::Obj(p) => Some(p), _ => None };
    let out = Value::Obj(new_obj(proto));
    if let Some(src) = a.first() {
        if !matches!(src, Value::Undefined | Value::Null) {
            for it in i.iterate(src)? {
                let (k, v) = if is_map { (i.get(&it, "0")?, i.get(&it, "1")?) } else { (it.clone(), it) };
                let ks = i.to_string(&k)?;
                if let Value::Obj(o) = &out {
                    o.borrow_mut().define(&alloc::format!("@{ks}"), Prop {
                        value: Some(v), get: None, set: None,
                        writable: true, enumerable: false, configurable: true });
                }
            }
        }
    }
    Ok(out)
}

fn pad(i: &mut Interp, t: Value, a: &[Value], start: bool) -> C<Value> {
    let s = i.to_string(&t)?;
    let want = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?) as usize;
    let fill = match a.get(1) { None | Some(Value::Undefined) => Rc::from(" "), Some(v) => i.to_string(v)? };
    let have = s.chars().count();
    if want <= have || fill.is_empty() { return Ok(Value::Str(s)); }
    if want > (1 << 22) { return i.range_err("padding too large"); }
    let mut p = String::new();
    while p.chars().count() < want - have { p.push_str(&fill); }
    let p: String = p.chars().take(want - have).collect();
    Ok(Value::string(if start { p + &s } else { s.to_string() + &p }))
}

fn array_len(i: &mut Interp, this: &Value) -> C<f64> {
    let l = i.get(this, "length")?;
    i.to_number(&l)
}

fn make_error(i: &mut Interp, kind: &'static str, a: &[Value]) -> C<Value> {
    let proto = i.realm.error_ctors.get(kind).cloned().unwrap_or_else(|| i.realm.error_proto.clone());
    let e = new_kind(Some(proto), ObjKind::Error);
    if let Some(m) = a.first() {
        if !matches!(m, Value::Undefined) {
            let s = i.to_string(m)?;
            e.borrow_mut().define("message", Prop::builtin(Value::Str(s)));
        }
    }
    Ok(Value::Obj(e))
}


/// Die Argumente eines `console`-Aufrufs zu einer Zeile machen.
///
/// Ein `toString`, das selbst wirft, darf den Aufruf nicht zum Ausnahmefall
/// machen: eine Ausgabe, die das Programm anhaelt, ist schlimmer als eine
/// unvollstaendige.
fn console_join(i: &mut Interp, args: &[Value], prefix: &str) -> String {
    let mut out = String::from(prefix);
    for (n, a) in args.iter().enumerate() {
        if n > 0 { out.push(' '); }
        // Ein Symbol wirft bei `ToString` — auf der Konsole waere das der
        // falsche Ort dafuer: die Zeile soll berichten, nicht abbrechen.
        if let Value::Sym(sd) = a { out.push_str(&Interp::sym_to_display(sd)); continue; }
        match i.to_string(a) {
            Ok(s) => out.push_str(&s),
            Err(_) => out.push_str("<nicht darstellbar>"),
        }
    }
    out
}
