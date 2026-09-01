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
use core::cell::RefCell;
use hashbrown::HashMap;

use super::interp::*;
use super::value::*;

fn def(o: &Gc, name: &str, f: NativeFn, len: usize, proto: &Gc) {
    let g = native(Some(proto.clone()), f, leak(name), len, false);
    o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
}

/// Namen der eingebauten Funktionen sind `&'static str`. Statt jeden einzeln
/// als Konstante zu fuehren, wird hier eine kleine Tabelle benutzt — sie ist
/// vollstaendig, weil sie neben den Definitionen steht.
fn leak(s: &str) -> &'static str {
    const NAMES: &[&str] = &[
        "toString", "valueOf", "hasOwnProperty", "isPrototypeOf", "propertyIsEnumerable",
        "call", "apply", "bind", "defineProperty", "getOwnPropertyDescriptor",
        "getOwnPropertyNames", "keys", "values", "entries", "create", "getPrototypeOf",
        "setPrototypeOf", "freeze", "isFrozen", "assign", "is", "preventExtensions",
        "isExtensible", "push", "pop", "slice", "indexOf", "join", "map", "filter",
        "forEach", "concat", "isArray", "charAt", "charCodeAt", "substring", "toUpperCase",
        "toLowerCase", "trim", "split", "replace", "String", "Number", "Boolean", "Object",
        "Array", "Error", "TypeError", "RangeError", "SyntaxError", "ReferenceError",
        "EvalError", "URIError", "Function", "print", "abs", "floor", "ceil", "round",
        "max", "min", "pow", "sqrt", "isNaN", "isFinite", "parseInt", "parseFloat",
        "fromCharCode", "toFixed", "includes", "startsWith", "endsWith", "reverse",
        "lastIndexOf", "defineProperties", "seal", "isSealed", "sign", "trunc",
    ];
    NAMES.iter().find(|n| **n == s).copied().unwrap_or("anonymous")
}

pub fn make_realm() -> Realm {
    let object_proto = new_obj(None);
    let function_proto = new_kind(Some(object_proto.clone()), ObjKind::Plain);
    let array_proto = new_kind(Some(object_proto.clone()), ObjKind::Array);
    let string_proto = new_obj(Some(object_proto.clone()));
    let number_proto = new_obj(Some(object_proto.clone()));
    let boolean_proto = new_obj(Some(object_proto.clone()));
    let error_proto = new_obj(Some(object_proto.clone()));
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
                let tag = match &o.borrow().kind {
                    ObjKind::Array => "Array",
                    ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. } => "Function",
                    ObjKind::Error => "Error",
                    ObjKind::StrWrap(_) => "String",
                    ObjKind::NumWrap(_) => "Number",
                    ObjKind::BoolWrap(_) => "Boolean",
                    ObjKind::Arguments => "Arguments",
                    ObjKind::Plain => "Object",
                };
                alloc::format!("[object {tag}]")
            }
            _ => { let _ = i; "[object Object]".to_string() }
        }))
    }, 0, fp);
    def(&object_proto, "valueOf", |i, this, _| {
        let o = i.to_object(&this)?; Ok(Value::Obj(o))
    }, 0, fp);
    def(&object_proto, "hasOwnProperty", |i, this, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
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
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
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
            Some(v) => i.iterate(v)?,
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
            let c = native(Some(function_proto.clone()), $f, leak($name), 1, true);
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
    def(&array_proto, "concat", |i, this, a| {
        let mut out = i.iterate(&this)?;
        for v in a {
            if matches!(v, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::Array)) {
                out.extend(i.iterate(v)?);
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
        let k = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
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
        let k = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
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
    global.borrow_mut().define("Array", Prop::builtin(Value::Obj(array_ctor)));

    let string_ctor = native(Some(function_proto.clone()), |i, _, a| {
        Ok(match a.first() { None => Value::str(""), Some(v) => Value::Str(i.to_string(v)?) })
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

    Realm { global, global_env, object_proto, function_proto, array_proto,
            string_proto, number_proto, boolean_proto, error_proto, error_ctors }
}

/// `this.length` als Zahl. Eigene Funktion, weil `i.to_number(&i.get(...))`
/// zwei gleichzeitige Ausleihen waeren — und das Aufteilen an jeder Stelle
/// haette den Code nur laenger gemacht.
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
