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
        // Ein Stellvertreter traegt die Marke seines ZIELS: `[object Array]`
        // fuer einen Stellvertreter auf ein Feld.
        if let Value::Obj(o) = &this {
            if super::proxy::parts(o).is_some() {
                let t = super::proxy::target(i, o)?;
                let f = i.get(&Value::Obj(i.realm.object_proto.clone()), "toString")?;
                return i.call(&f, Value::Obj(t), &[]);
            }
        }
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
                    ObjKind::BigWrap(_) => "BigInt",
                    ObjKind::NumWrap(_) => "Number",
                    ObjKind::BoolWrap(_) => "Boolean",
                    ObjKind::Arguments => "Arguments",
                    ObjKind::Regex(_) => "RegExp",
                    ObjKind::Promise(_) => "Promise",
                    ObjKind::Date(_) => "Date",
                    // Oben abgefangen; hier nur, damit der Uebersetzer die
                    // Vollstaendigkeit prueft statt sie zu erlauben.
                    ObjKind::Proxy(_) => "Object",
                    ObjKind::Buffer(_) => "ArrayBuffer",
                    // Eine Sicht traegt ihren Namen ueber `Symbol.toStringTag`
                    // auf `%TypedArray%.prototype`; hier kommt sie nur an,
                    // wenn den jemand geloescht hat.
                    ObjKind::TypedArray(_) | ObjKind::DataView(_) => "Object",
                    // Ein Generator traegt seinen Namen ueber
                    // `Symbol.toStringTag` und kommt hier nur an, wenn den
                    // jemand geloescht hat — dann ist er ein gewoehnliches
                    // Objekt, genau wie in jedem echten Motor.
                    ObjKind::Generator(_) => "Object",
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
        let e = o.borrow().is_enumerable(&k);
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
    def(fp, "toString", |i, t, _| {
        if !i.is_callable(&t) { return i.type_err("Function.prototype.toString on a non-function"); }
        Ok(Value::str("function () { [native code] }"))
    }, 0, fp);

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
    // `Error.isError` fragt die ART, nicht die Prototypenkette — ein
    // `Object.create(Error.prototype)` ist KEIN Fehler.
    if let Some(Value::Obj(ec)) = global.borrow().get_own("Error").and_then(|p| p.value.clone()) {
        def(&ec, "isError", |_, _, a| {
            Ok(Value::Bool(matches!(a.first(), Some(Value::Obj(o))
                if matches!(o.borrow().kind, ObjKind::Error))))
        }, 1, fp);
    }
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
        i.set(&e, "errors", errs, true)?;
        Ok(e)
    });
    err_ctor!("URIError", |i, _, a| make_error(i, "URIError", a));

    // ── Array.prototype ──────────────────────────────────────────────────
    array_proto.borrow_mut().define("length", Prop {
        value: Some(Value::Num(0.0)), get: None, set: None,
        writable: true, enumerable: false, configurable: false });
    // **`ToObject(this)` ZUERST** (ES §23.1.3.x, Schritt 1 in jeder dieser
    // Funktionen). Solange ein Schreiben auf ein Primitiv still verpuffte,
    // fiel das Fehlen nicht auf: `[].pop.call(true)` schrieb ins Leere und
    // gab dasselbe zurueck. Mit der Wurf-Fahne wird daraus ein TypeError,
    // und der Test sagt endlich, was schon immer falsch war.
    def(&array_proto, "push", |i, this, a| {
        let this = Value::Obj(i.to_object(&this)?);
        let mut n = array_len(i, &this)?;
        for v in a { i.tick()?; i.set(&this, &num_to_string(n), v.clone(), true)?; n += 1.0; }
        i.set(&this, "length", Value::Num(n), true)?;
        Ok(Value::Num(n))
    }, 1, fp);
    def(&array_proto, "pop", |i, this, _| {
        let this = Value::Obj(i.to_object(&this)?);
        let n = array_len(i, &this)?;
        if n <= 0.0 { i.set(&this, "length", Value::Num(0.0), true)?; return Ok(Value::Undefined); }
        let k = num_to_string(n - 1.0);
        let v = i.get(&this, &k)?;
        if let Value::Obj(o) = &this { o.borrow_mut().remove(&k); }
        i.set(&this, "length", Value::Num(n - 1.0), true)?;
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
        // LOECHER zaehlen nicht mit. Unsere Felder haben zwar keine, aber
        // `new Array(10)` legt auch keine Indizes an — und genau daran haengt,
        // ob der Aufruf ohne Startwert wirft.
        let (mut acc, mut k) = match a.get(1) {
            Some(v) => (v.clone(), 0usize),
            None => {
                let mut at = None;
                for j in 0..n {
                    i.tick()?;
                    if has_index(i, &this, j) { at = Some(j); break; }
                }
                let Some(j) = at else {
                    return i.type_err("reduce of empty array with no initial value");
                };
                (i.get(&this, &num_to_string(j as f64))?, j + 1)
            }
        };
        while k < n {
            i.tick()?;
            if !has_index(i, &this, k) { k += 1; continue; }
            let v = i.get(&this, &num_to_string(k as f64))?;
            acc = i.call(&f, Value::Undefined, &[acc, v, Value::Num(k as f64), this.clone()])?;
            k += 1;
        }
        Ok(acc)
    }, 1, fp);
    def(&array_proto, "reduceRight", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function"); }
        let n = array_len(i, &this)? as usize;
        let (mut acc, mut k) = match a.get(1) {
            Some(v) => (v.clone(), n),
            None => {
                let mut at = None;
                for j in (0..n).rev() {
                    i.tick()?;
                    if has_index(i, &this, j) { at = Some(j); break; }
                }
                let Some(j) = at else {
                    return i.type_err("reduceRight of empty array with no initial value");
                };
                (i.get(&this, &num_to_string(j as f64))?, j)
            }
        };
        while k > 0 {
            i.tick()?;
            k -= 1;
            if !has_index(i, &this, k) { continue; }
            let v = i.get(&this, &num_to_string(k as f64))?;
            acc = i.call(&f, Value::Undefined, &[acc, v, Value::Num(k as f64), this.clone()])?;
        }
        Ok(acc)
    }, 1, fp);
    // Ohne Sprachumgebung ist `toLocaleString` die Verkettung der einzelnen
    // `toLocaleString` — das ist keine Vereinfachung, das ist der Algorithmus.
    def(&array_proto, "toLocaleString", |i, this, _| {
        let n = array_len(i, &this)? as usize;
        let mut out = String::new();
        for k in 0..n {
            i.tick()?;
            if k > 0 { out.push(','); }
            let v = i.get(&this, &num_to_string(k as f64))?;
            if matches!(v, Value::Undefined | Value::Null) { continue; }
            let f = i.get(&v, "toLocaleString")?;
            if !i.is_callable(&f) { return i.type_err("toLocaleString is not a function"); }
            let r = i.call(&f, v, &[])?;
            out.push_str(&i.to_string(&r)?);
        }
        Ok(Value::string(out))
    }, 0, fp);
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
    // **`reverse` TAUSCHT, es baut nicht neu** (ES §23.1.3.26). Der
    // Unterschied ist beobachtbar und hat drei Familien gekostet: `rebuild`
    // schrieb `length` und alle Indizes, also warf ein eingefrorenes `[1]`
    // (die Spezifikation tauscht dort NICHTS) und eine typisierte Sicht
    // verlor ihre nicht-numerischen Eigenschaften. Getauscht wird ueber
    // `Set(…, true)` — auf einem eingefrorenen Feld mit zwei Eintraegen
    // wirft das, und genau so haelt es node.
    def(&array_proto, "reverse", |i, this, _| {
        let this = Value::Obj(i.to_object(&this)?);
        let len = array_len(i, &this)? as i64;
        let middle = len / 2;
        for lower in 0..middle {
            i.tick()?;
            let upper = len - lower - 1;
            let (lo, up) = (num_to_string(lower as f64), num_to_string(upper as f64));
            let lo_has = matches!(&this, Value::Obj(o) if i.has_property(o, &lo));
            let up_has = matches!(&this, Value::Obj(o) if i.has_property(o, &up));
            let lo_v = if lo_has { i.get(&this, &lo)? } else { Value::Undefined };
            let up_v = if up_has { i.get(&this, &up)? } else { Value::Undefined };
            match (lo_has, up_has) {
                (true, true) => { i.set(&this, &lo, up_v, true)?; i.set(&this, &up, lo_v, true)?; }
                (false, true) => { i.set(&this, &lo, up_v, true)?; i.delete_or_throw(&this, &up)?; }
                (true, false) => { i.delete_or_throw(&this, &lo)?; i.set(&this, &up, lo_v, true)?; }
                (false, false) => {}
            }
        }
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
        let this = Value::Obj(i.to_object(&this)?);
        let mut items = i.elems(&this)?;
        if items.len() < 2 { return Ok(this); }
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
        // Zurueckgeschrieben wird ELEMENTWEISE, ohne `length` anzufassen —
        // sonst verliert eine typisierte Sicht ihre Laenge (die ist dort ein
        // Getter) und ihre nicht-numerischen Eigenschaften.
        for (k, v) in items.into_iter().enumerate() {
            i.tick()?;
            i.set(&this, &num_to_string(k as f64), v, true)?;
        }
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
        let s = this_string(i, &this)?;
        let n = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?) as usize;
        Ok(match s.chars().nth(n) { Some(c) => { let mut t = String::new(); t.push(c); Value::string(t) }
                                    None => Value::str("") })
    }, 1, fp);
    def(&string_proto, "charCodeAt", |i, this, a| {
        let s = this_string(i, &this)?;
        let n = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?) as usize;
        Ok(match s.chars().nth(n) { Some(c) => Value::Num(c as u32 as f64), None => Value::Num(f64::NAN) })
    }, 1, fp);
    def(&string_proto, "indexOf", |i, this, a| {
        let s = this_string(i, &this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Num(match s.find(&*t) {
            Some(b) => s[..b].chars().count() as f64, None => -1.0 }))
    }, 1, fp);
    def(&string_proto, "includes", |i, this, a| {
        let s = this_string(i, &this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(s.contains(&*t)))
    }, 1, fp);
    def(&string_proto, "split", |i, this, a| {
        let s = this_string(i, &this)?;
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
        let s = this_string(i, &this)?;
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
        let s = this_string(i, &this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(s.starts_with(&*t)))
    }, 1, fp);
    def(&string_proto, "endsWith", |i, this, a| {
        let s = this_string(i, &this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(s.ends_with(&*t)))
    }, 1, fp);
    def(&string_proto, "slice", |i, this, a| {
        let s = this_string(i, &this)?;
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
        let s = this_string(i, &this)?;
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
        let s = this_string(i, &this)?;
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
        let s = this_string(i, &t)?;
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Num(match s.rfind(&*n) { Some(b) => s[..b].chars().count() as f64, None => -1.0 }))
    }, 1, fp);
    def(&string_proto, "at", |i, t, a| {
        let s = this_string(i, &t)?;
        let len = s.chars().count() as i64;
        let mut k = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?) as i64;
        if k < 0 { k += len; }
        Ok(match s.chars().nth(k.max(0) as usize) {
            Some(c) if k >= 0 && k < len => { let mut x = String::new(); x.push(c); Value::string(x) }
            _ => Value::Undefined })
    }, 1, fp);
    def(&string_proto, "trimStart", |i, t, _| {
        let s = this_string(i, &t)?; Ok(Value::str(s.trim_start()))
    }, 0, fp);
    def(&string_proto, "trimEnd", |i, t, _| {
        let s = this_string(i, &t)?; Ok(Value::str(s.trim_end()))
    }, 0, fp);
    for (alias, orig) in [("trimLeft", "trimStart"), ("trimRight", "trimEnd")] {
        let v = string_proto.borrow().get_own(orig).and_then(|p| p.value.clone());
        if let Some(v) = v { string_proto.borrow_mut().define(alias, Prop::builtin(v)); }
    }
    def(&string_proto, "padStart", |i, t, a| pad(i, t, a, true), 2, fp);
    def(&string_proto, "padEnd", |i, t, a| pad(i, t, a, false), 2, fp);
    def(&string_proto, "concat", |i, t, a| {
        let mut s = this_string(i, &t)?.to_string();
        for v in a { s.push_str(&i.to_string(v)?); }
        Ok(Value::string(s))
    }, 1, fp);
    def(&string_proto, "toUpperCase", |i, this, _| {
        let s = this_string(i, &this)?; Ok(Value::string(s.to_uppercase()))
    }, 0, fp);
    def(&string_proto, "toLowerCase", |i, this, _| {
        let s = this_string(i, &this)?; Ok(Value::string(s.to_lowercase()))
    }, 0, fp);
    def(&string_proto, "trim", |i, this, _| {
        let s = this_string(i, &this)?; Ok(Value::str(s.trim()))
    }, 0, fp);
    // Ohne Sprachumgebung IST die landessprachliche Umwandlung die
    // gewoehnliche. Eine erfundene tuerkische Sonderregel waere schlechter
    // als keine.
    def(&string_proto, "toLocaleUpperCase", |i, this, _| {
        let s = this_string(i, &this)?; Ok(Value::string(s.to_uppercase()))
    }, 0, fp);
    def(&string_proto, "toLocaleLowerCase", |i, this, _| {
        let s = this_string(i, &this)?; Ok(Value::string(s.to_lowercase()))
    }, 0, fp);
    // Unsere Zeichenketten sind UTF-8 — eine einzelne Ersatzhaelfte kann
    // darin gar nicht stehen. Also ist jede wohlgeformt, und `toWellFormed`
    // gibt sie unveraendert zurueck. Benannt statt verschwiegen: eine Seite,
    // die eine kaputte UTF-16-Folge baut, bekommt hier `true` statt `false`.
    // `normalize` OHNE die Unicode-Zerlegungstabellen: sie prueft die Form
    // und gibt die Zeichenkette unveraendert zurueck. Fuer bereits
    // normalisierten Text — praktisch jeden Text im Netz — ist das die
    // richtige Antwort; fuer zerlegten ist es die falsche. Benannt statt
    // verschwiegen, und trotzdem besser als "normalize is not a function",
    // woran eine Seite ganz stirbt.
    def(&string_proto, "normalize", |i, this, a| {
        let s = this_string(i, &this)?;
        match a.first() {
            None | Some(Value::Undefined) => {}
            Some(v) => {
                let f = i.to_string(v)?;
                if !matches!(&*f, "NFC" | "NFD" | "NFKC" | "NFKD") {
                    return i.range_err("normalize: form must be one of NFC, NFD, NFKC, NFKD");
                }
            }
        }
        Ok(Value::Str(s))
    }, 0, fp);
    def(&string_proto, "isWellFormed", |i, this, _| {
        let _ = this_string(i, &this)?; Ok(Value::Bool(true))
    }, 0, fp);
    def(&string_proto, "toWellFormed", |i, this, _| {
        let s = this_string(i, &this)?; Ok(Value::Str(s))
    }, 0, fp);
    // Die dreizehn annexB-Auszeichner. Sie stehen in der Spezifikation, weil
    // alter Code sie ruft — und der Anfuehrungszeichen-Ersatz gehoert dazu.
    macro_rules! html_wrap {
        ($($m:literal => $tag:literal, $attr:literal, $len:literal),* $(,)?) => { $(
            def(&string_proto, $m, |i, t, a| {
                let s = this_string(i, &t)?;
                let (tag, attr) = ($tag, $attr);
                let mut open = String::from("<");
                open.push_str(tag);
                if !attr.is_empty() {
                    let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                    open.push(' ');
                    open.push_str(attr);
                    open.push_str("=\"");
                    open.push_str(&v.replace('"', "&quot;"));
                    open.push('"');
                }
                open.push('>');
                Ok(Value::string(alloc::format!("{open}{s}</{tag}>")))
            }, $len, fp);
        )* };
    }
    html_wrap! {
        "anchor" => "a", "name", 1,
        "link" => "a", "href", 1,
        "fontcolor" => "font", "color", 1,
        "fontsize" => "font", "size", 1,
        "big" => "big", "", 0,
        "blink" => "blink", "", 0,
        "bold" => "b", "", 0,
        "fixed" => "tt", "", 0,
        "italics" => "i", "", 0,
        "small" => "small", "", 0,
        "strike" => "strike", "", 0,
        "sub" => "sub", "", 0,
        "sup" => "sup", "", 0,
    }

    // ── Number / Boolean prototypes ──────────────────────────────────────
    def(&number_proto, "valueOf", |i, this, _| {
        if let Value::Obj(o) = &this {
            if let ObjKind::NumWrap(n) = &o.borrow().kind { return Ok(Value::Num(*n)); }
        }
        if matches!(this, Value::Num(_)) { return Ok(this); }
        i.type_err("Number.prototype.valueOf on a non-number")
    }, 0, fp);
    def(&number_proto, "toString", |i, this, a| {
        let n = this_number(i, &this)?;
        let radix = match a.first() {
            None | Some(Value::Undefined) => 10.0,
            Some(v) => to_integer(i.to_number(v)?),
        };
        if !(2.0..=36.0).contains(&radix) { return i.range_err("toString: radix out of range"); }
        Ok(Value::string(if radix == 10.0 { num_to_string(n) }
                         else { num_to_radix(n, radix as u32) }))
    }, 1, fp);
    def(&boolean_proto, "valueOf", |i, this, _| {
        if let Value::Obj(o) = &this {
            if let ObjKind::BoolWrap(b) = &o.borrow().kind { return Ok(Value::Bool(*b)); }
        }
        if matches!(this, Value::Bool(_)) { return Ok(this); }
        i.type_err("Boolean.prototype.valueOf on a non-boolean")
    }, 0, fp);
    def(&boolean_proto, "toString", |i, this, _| {
        let b = match &this {
            Value::Bool(b) => *b,
            Value::Obj(o) => match &o.borrow().kind {
                ObjKind::BoolWrap(b) => *b,
                _ => return i.type_err("Boolean.prototype.toString on a non-boolean"),
            },
            _ => return i.type_err("Boolean.prototype.toString on a non-boolean"),
        };
        Ok(Value::str(if b { "true" } else { "false" }))
    }, 0, fp);

    // `toFixed` ist die einzige Zahlenformatierung, die echter Code wirklich
    // ruft — und sie rundet KAUFMAENNISCH auf die Stelle, nicht ueber
    // `format!("{:.n}")`, das zur geraden Ziffer rundet. `(1.005).toFixed(2)`
    // ist in JS "1.00" (weil 1.005 als f64 knapp darunter liegt), und wer
    // hier eine eigene Rundung erfindet, weicht genau dort ab.
    def(&number_proto, "toFixed", |i, t, a| {
        let n = this_number(i, &t)?;
        let d = to_integer(i.to_number(a.first().unwrap_or(&Value::Num(0.0)))?);
        if !(0.0..=100.0).contains(&d) { return i.range_err("toFixed: digits out of range"); }
        if !n.is_finite() || libm::fabs(n) >= 1e21 { return Ok(Value::string(num_to_string(n))); }
        Ok(Value::string(fixed(n, d as u32)))
    }, 1, fp);
    def(&number_proto, "toPrecision", |i, t, a| {
        let n = this_number(i, &t)?;
        let Some(v) = a.first().filter(|v| !matches!(v, Value::Undefined)) else {
            return Ok(Value::string(num_to_string(n)));
        };
        let p = to_integer(i.to_number(v)?);
        if !(1.0..=100.0).contains(&p) { return i.range_err("toPrecision: out of range"); }
        if !n.is_finite() { return Ok(Value::string(num_to_string(n))); }
        if n == 0.0 { return Ok(Value::string(fixed(0.0, p as u32 - 1))); }
        // Die Spezifikation waehlt zwischen fester und Exponentialform nach
        // DEM Exponenten, nicht nach Geschmack: unter -6 oder ab p Stellen
        // wird exponentiell geschrieben, sonst fest.
        let e = libm::floor(libm::log10(libm::fabs(n))) as i32;
        if e < -6 || e >= p as i32 {
            let mant = n / libm::pow(10.0, e as f64);
            let sign = if e < 0 { '-' } else { '+' };
            return Ok(Value::string(alloc::format!("{}e{}{}",
                fixed(mant, p as u32 - 1), sign, e.abs())));
        }
        Ok(Value::string(fixed(n, (p as i32 - 1 - e).max(0) as u32)))
    }, 1, fp);
    def(&number_proto, "toExponential", |i, t, a| {
        let n = this_number(i, &t)?;
        let arg = a.first().cloned().unwrap_or(Value::Undefined);
        // Die Umwandlung des Arguments laeuft VOR der Endlichkeitspruefung —
        // sie ist beobachtbar (ES 21.1.3.2).
        let dv = i.to_number(&arg)?;
        if !n.is_finite() { return Ok(Value::string(num_to_string(n))); }
        let auto = matches!(arg, Value::Undefined);
        let d = to_integer(dv);
        if !auto && !(0.0..=100.0).contains(&d) { return i.range_err("toExponential: out of range"); }
        if n == 0.0 {
            let f = if auto { 0 } else { d as u32 };
            let sign = if n.is_sign_negative() { "-" } else { "" };
            return Ok(Value::string(alloc::format!("{sign}{}e+0", fixed(0.0, f))));
        }
        let a2 = libm::fabs(n);
        let mut e = libm::floor(libm::log10(a2)) as i32;
        let f = if auto {
            // Ohne Stellenangabe: so viele, wie die kuerzeste Darstellung
            // braucht. `num_to_string` liefert genau die.
            let s = num_to_string(a2);
            let digits: usize = s.chars().filter(|c| c.is_ascii_digit()).count();
            (digits.saturating_sub(1)).min(100) as u32
        } else { d as u32 };
        let mut mant = a2 / libm::pow(10.0, e as f64);
        // Das Runden der Mantisse kann sie auf 10 heben — dann traegt der
        // Exponent die Stelle.
        let r = libm::pow(10.0, f as f64);
        if libm::floor(mant * r + 0.5) >= 10.0 * r { mant /= 10.0; e += 1; }
        let sign = if e < 0 { '-' } else { '+' };
        let body = alloc::format!("{}e{}{}", fixed(mant, f), sign, e.abs());
        Ok(Value::string(if n < 0.0 { alloc::format!("-{body}") } else { body }))
    }, 1, fp);
    // Ohne Landeseinstellungen: dieselbe Ausgabe wie `toString`. Eine
    // erfundene Tausendertrennung waere schlimmer — sie saehe aus wie eine
    // Lokalisierung und waere die falsche.
    def(&number_proto, "toLocaleString", |i, t, _| {
        let n = this_number(i, &t)?; Ok(Value::string(num_to_string(n)))
    }, 0, fp);

    // ── Konstruktoren + globale Funktionen ───────────────────────────────
    // ── `Object.prototype.__proto__` und die vier annexB-Helfer ─────────
    //
    // `__proto__` ist ein ZUGRIFF auf `Object.prototype`, keine Eigenschaft
    // je Objekt — sonst waere `({}).__proto__` ein eigener Schluessel und
    // taeuchte in `Object.keys` auf.
    {
        let get = native(Some(function_proto.clone()), |i, t, _| {
            let o = i.to_object(&t)?;
            let p = o.borrow().proto.clone();
            Ok(match p { Some(x) => Value::Obj(x), None => Value::Null })
        }, "get __proto__", 0, false);
        let set = native(Some(function_proto.clone()), |i, t, a| {
            if matches!(t, Value::Undefined | Value::Null) {
                return i.type_err("cannot set __proto__ of undefined or null");
            }
            let Value::Obj(o) = &t else { return Ok(Value::Undefined) };
            let np = match a.first() {
                Some(Value::Obj(p)) => Some(p.clone()),
                Some(Value::Null) => None,
                // Ein Primitiv ist KEIN Fehler, es wird still uebergangen.
                _ => return Ok(Value::Undefined),
            };
            let mut cur = np.clone();
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
            o.borrow_mut().proto = np;
            Ok(Value::Undefined)
        }, "set __proto__", 1, false);
        object_proto.borrow_mut().define("__proto__", Prop {
            value: None, get: Some(Value::Obj(get)), set: Some(Value::Obj(set)),
            writable: false, enumerable: false, configurable: true });
    }
    def(&object_proto, "__defineGetter__", |i, t, a| {
        let o = i.to_object(&t)?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("getter is not a function"); }
        let k = i.to_prop_key(a.first().unwrap_or(&Value::Undefined))?;
        let keep = o.borrow().get_own(&k).and_then(|p| p.set.clone());
        o.borrow_mut().define(&k, Prop { value: None, get: Some(f), set: keep,
            writable: false, enumerable: true, configurable: true });
        Ok(Value::Undefined)
    }, 2, fp);
    def(&object_proto, "__defineSetter__", |i, t, a| {
        let o = i.to_object(&t)?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("setter is not a function"); }
        let k = i.to_prop_key(a.first().unwrap_or(&Value::Undefined))?;
        let keep = o.borrow().get_own(&k).and_then(|p| p.get.clone());
        o.borrow_mut().define(&k, Prop { value: None, get: keep, set: Some(f),
            writable: false, enumerable: true, configurable: true });
        Ok(Value::Undefined)
    }, 2, fp);
    def(&object_proto, "__lookupGetter__", |i, t, a| lookup_accessor(i, t, a, false), 1, fp);
    def(&object_proto, "__lookupSetter__", |i, t, a| lookup_accessor(i, t, a, true), 1, fp);
    def(&object_proto, "toLocaleString", |i, t, _| {
        let f = i.get(&t, "toString")?;
        if !i.is_callable(&f) { return i.type_err("toString is not a function"); }
        i.call(&f, t, &[])
    }, 0, fp);

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
        let all = i.own_keys_of(&o)?;
        let mut keys = Vec::new();
        for k in all {
            if is_sym_key(&k) { continue; }
            if enum_own(i, &o, &k)? { keys.push(Value::Str(k)); }
        }
        Ok(i.new_array(keys))
    }, 1, fp);
    def(&object_ctor, "getOwnPropertyNames", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let keys: Vec<Value> = i.own_keys_of(&o)?.into_iter().map(Value::Str).collect();
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
        let p = i.proto_of(&o)?;
        Ok(match p { Some(x) => Value::Obj(x), None => Value::Null })
    }, 1, fp);
    def(&object_ctor, "create", |i, _, a| {
        let proto = match a.first() {
            Some(Value::Obj(o)) => Some(o.clone()),
            Some(Value::Null) => None,
            _ => return i.type_err("Object.create needs an object or null"),
        };
        let g = new_obj(proto);
        // Das ZWEITE Argument ist dieselbe Tabelle wie bei
        // `defineProperties` — dieselbe Funktion, nicht dieselbe Idee.
        if let Some(p) = a.get(1) {
            if !matches!(p, Value::Undefined) { i.define_props_from(&g, p)?; }
        }
        Ok(Value::Obj(g))
    }, 2, fp);
    def(&object_ctor, "defineProperty", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else { return i.type_err("Object.defineProperty on a non-object") };
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let d = a.get(2).cloned().unwrap_or(Value::Undefined);
        let Value::Obj(_) = &d else { return i.type_err("property descriptor must be an object") };
        let p = i.to_prop_desc(&d)?;
        let o = o.clone();
        i.define_own(&o, &k, p)?;
        Ok(a[0].clone())
    }, 3, fp);
    def(&object_ctor, "defineProperties", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return i.type_err("Object.defineProperties on a non-object");
        };
        let o = o.clone();
        let props = a.get(1).cloned().unwrap_or(Value::Undefined);
        i.define_props_from(&o, &props)?;
        Ok(Value::Obj(o))
    }, 2, fp);
    def(&object_ctor, "getOwnPropertyDescriptors", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let out = new_obj(Some(i.realm.object_proto.clone()));
        for k in i.own_keys_of(&o)? {
            let Some(p) = i.get_own_desc(&o, &k)? else { continue };
            let d = i.from_prop_desc(&p);
            out.borrow_mut().set_prop(k, Prop::data(d));
        }
        Ok(Value::Obj(out))
    }, 1, fp);
    def(&object_ctor, "getOwnPropertyDescriptor", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let Some(p) = i.get_own_desc(&o, &k)? else { return Ok(Value::Undefined) };
        Ok(i.from_prop_desc(&p))
    }, 2, fp);
    def(&object_ctor, "assign", |i, _, a| {
        let target = a.first().cloned().unwrap_or(Value::Undefined);
        // `ToObject(target)` steht VOR allem anderen — auf `undefined` wirft
        // es, statt still nichts zu tun.
        let target = Value::Obj(i.to_object(&target)?);
        for src in a.get(1..).unwrap_or(&[]) {
            let Value::Obj(o) = src else { continue };
            // Zeichenketten UND Symbole: `assign` kopiert jede eigene
            // aufzaehlbare Eigenschaft, und ein Symbol ist eine.
            let o = o.clone();
            let mut keys = i.own_keys_of(&o)?;
            if super::proxy::parts(&o).is_none() { keys.extend(o.borrow().own_sym_keys()); }
            for k in keys {
                if !enum_own(i, &o, &k)? { continue; }
                let v = i.get(src, &k)?;
                i.set(&target, &k, v, true)?;
            }
        }
        Ok(target)
    }, 2, fp);
    def(&object_ctor, "values", |i, _, a| {
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        let o = i.to_object(&src)?;
        let keys = i.own_keys_of(&o)?;
        let mut out = Vec::new();
        for k in keys {
            if is_sym_key(&k) || !enum_own(i, &o, &k)? { continue; }
            out.push(i.get(&src, &k)?);
        }
        Ok(i.new_array(out))
    }, 1, fp);
    def(&object_ctor, "entries", |i, _, a| {
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        let o = i.to_object(&src)?;
        let keys = i.own_keys_of(&o)?;
        let mut out = Vec::new();
        for k in keys {
            if is_sym_key(&k) || !enum_own(i, &o, &k)? { continue; }
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
        let first = a.first().cloned().unwrap_or(Value::Undefined);
        if matches!(first, Value::Undefined | Value::Null) {
            return i.type_err("Object.setPrototypeOf on undefined or null");
        }
        // Das zweite Argument wird auch dann geprueft, wenn das erste ein
        // Primitiv ist — die Reihenfolge steht in der Spezifikation.
        if !matches!(a.get(1), Some(Value::Obj(_)) | Some(Value::Null)) {
            return i.type_err("Object.setPrototypeOf: prototype must be an object or null");
        }
        let Some(Value::Obj(o)) = a.first() else { return Ok(first) };
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
    def(&object_ctor, "seal", |_, _, a| {
        if let Some(Value::Obj(o)) = a.first() {
            o.borrow_mut().extensible = false;
            let keys = o.borrow().own_keys();
            for k in keys {
                let existing = o.borrow().get_own(&k).cloned();
                if let Some(mut p) = existing {
                    // Versiegeln nimmt nur die KONFIGURIERBARKEIT — der Wert
                    // bleibt schreibbar. Das ist der ganze Unterschied zu
                    // `freeze`, und er wird geprueft.
                    p.configurable = false;
                    o.borrow_mut().set_prop(k, p);
                }
            }
        }
        Ok(a.first().cloned().unwrap_or(Value::Undefined))
    }, 1, fp);
    def(&object_ctor, "isSealed", |_, _, a| {
        let Some(Value::Obj(o)) = a.first() else { return Ok(Value::Bool(true)) };
        if o.borrow().extensible { return Ok(Value::Bool(false)) }
        let keys = o.borrow().own_keys();
        for k in keys {
            if o.borrow().get_own(&k).map(|p| p.configurable).unwrap_or(false) {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    }, 1, fp);
    def(&object_ctor, "isFrozen", |_, _, a| {
        let Some(Value::Obj(o)) = a.first() else { return Ok(Value::Bool(true)) };
        if o.borrow().extensible { return Ok(Value::Bool(false)) }
        let keys = o.borrow().own_keys();
        for k in keys {
            let Some(p) = o.borrow().get_own(&k).cloned() else { continue };
            if p.configurable || (!p.is_accessor() && p.writable) {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    }, 1, fp);
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
    def(&object_ctor, "hasOwn", |i, _, a| {
        let o = i.to_object(a.first().unwrap_or(&Value::Undefined))?;
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let has = o.borrow().has_own(&k);
        Ok(Value::Bool(has))
    }, 2, fp);
    def(&object_ctor, "groupBy", |i, _, a| {
        let out = new_obj(None);
        group_into(i, a, &out, false)?;
        Ok(Value::Obj(out))
    }, 2, fp);

    global.borrow_mut().define("Object", Prop::builtin(Value::Obj(object_ctor)));

    let array_ctor = native(Some(function_proto.clone()), |i, _, a| {
        if a.len() == 1 {
            if let Value::Num(n) = &a[0] {
                let arr = i.new_array(Vec::new());
                i.set(&arr, "length", Value::Num(*n), true)?;
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
        let prim = match a.first() {
            None => Value::str(""),
            // `String(sym)` ist die AUSNAHME: sie darf, wo `"" + sym` wirft.
            // Genau so steht es in der Spezifikation, und es ist der einzige
            // Weg, ein Symbol absichtlich zu Text zu machen.
            Some(Value::Sym(sd)) => Value::Str(Interp::sym_to_display(sd)),
            Some(v) => Value::Str(i.to_string(v)?),
        };
        // **`new String(x)` ist ein OBJEKT, `String(x)` ist Text.** Bis 0.98.0
        // gab beak beide Male das Primitiv, und `typeof new String()` sagte
        // "string". Aufgefallen ist es erst, als `this` im lockeren Modus
        // richtig eingepackt wurde: der Test verglich das eingepackte `this`
        // mit dem NICHT eingepackten `new String()` — vorher waren beide
        // Primitive und die zwei Fehler hoben sich auf.
        if i.native_new { return Ok(Value::Obj(i.to_object(&prim)?)); }
        Ok(prim)
    }, "String", 1, true);
    string_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(string_proto.clone())));
    string_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(string_ctor.clone())));
    def(&string_ctor, "fromCharCode", |i, _, a| {
        let mut s = String::new();
        for v in a { let n = i.to_number(v)? as u32; if let Some(c) = char::from_u32(n) { s.push(c); } }
        Ok(Value::string(s))
    }, 1, fp);
    def(&string_ctor, "fromCodePoint", |i, _, a| {
        let mut s = String::new();
        for v in a {
            let n = i.to_number(v)?;
            if libm::trunc(n) != n || !(0.0..=0x10FFFF as f64).contains(&n) {
                return i.range_err("invalid code point");
            }
            match char::from_u32(n as u32) {
                Some(c) => s.push(c),
                // Eine einzelne Ersatzhaelfte ist ein gueltiger Codepunkt
                // fuer diese Funktion, aber kein `char`. Sie geht als
                // Ersatzzeichen durch — unsere Zeichenketten sind UTF-8.
                None => s.push('\u{FFFD}'),
            }
        }
        Ok(Value::string(s))
    }, 1, fp);
    // `String.raw` liest die ROHEN Teile eines Vorlagenobjekts. Sie ist auch
    // ohne markierte Vorlagen erreichbar — mit einem selbstgebauten Objekt,
    // und genau so prueft test262 sie.
    def(&string_ctor, "raw", |i, _, a| {
        let t = a.first().cloned().unwrap_or(Value::Undefined);
        let raw = i.get(&t, "raw")?;
        let len = i.get(&raw, "length")?;
        let n = i.to_number(&len)?;
        let n = if n.is_nan() || n <= 0.0 { 0 } else { n as usize };
        let mut out = String::new();
        for k in 0..n {
            i.tick()?;
            let seg = i.get(&raw, &num_to_string(k as f64))?;
            out.push_str(&i.to_string(&seg)?);
            if k + 1 < n {
                if let Some(sub) = a.get(k + 1) { out.push_str(&i.to_string(sub)?); }
            }
        }
        Ok(Value::string(out))
    }, 1, fp);
    global.borrow_mut().define("String", Prop::builtin(Value::Obj(string_ctor)));

    let number_ctor = native(Some(function_proto.clone()), |i, _, a| {
        let prim = Value::Num(match a.first() { None => 0.0, Some(v) => i.to_number(v)? });
        if i.native_new { return Ok(Value::Obj(i.to_object(&prim)?)); }
        Ok(prim)
    }, "Number", 1, true);
    number_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(number_proto.clone())));
    number_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(number_ctor.clone())));
    for (n, v) in [("MAX_SAFE_INTEGER", 9007199254740991.0), ("MIN_SAFE_INTEGER", -9007199254740991.0),
                   ("MAX_VALUE", f64::MAX), ("MIN_VALUE", 5e-324), ("EPSILON", f64::EPSILON),
                   ("POSITIVE_INFINITY", f64::INFINITY), ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
                   ("NaN", f64::NAN)] {
        number_ctor.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    // Die vier Praedikate sind KEINE Umwandlung: `Number.isNaN("NaN")` ist
    // falsch, `isNaN("NaN")` wahr. Genau darin liegt ihr Zweck.
    def(&number_ctor, "isNaN", |_, _, a| {
        Ok(Value::Bool(matches!(a.first(), Some(Value::Num(n)) if n.is_nan())))
    }, 1, fp);
    def(&number_ctor, "isFinite", |_, _, a| {
        Ok(Value::Bool(matches!(a.first(), Some(Value::Num(n)) if n.is_finite())))
    }, 1, fp);
    def(&number_ctor, "isInteger", |_, _, a| {
        Ok(Value::Bool(matches!(a.first(), Some(Value::Num(n)) if n.is_finite() && libm::trunc(*n) == *n)))
    }, 1, fp);
    def(&number_ctor, "isSafeInteger", |_, _, a| {
        Ok(Value::Bool(matches!(a.first(), Some(Value::Num(n))
            if n.is_finite() && libm::trunc(*n) == *n && libm::fabs(*n) <= 9007199254740991.0)))
    }, 1, fp);
    global.borrow_mut().define("Number", Prop::builtin(Value::Obj(number_ctor.clone())));

    // ── BigInt ───────────────────────────────────────────────────────────
    let bigint_proto = new_obj(Some(object_proto.clone()));
    let bigint_ctor = native(Some(function_proto.clone()), |i, _, a| {
        if i.native_new { return i.type_err("BigInt is not a constructor"); }
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        let p = i.to_primitive(&v, false)?;
        Ok(Value::BigInt(Rc::new(match &p {
            Value::BigInt(b) => (**b).clone(),
            Value::Bool(b) => super::bigint::Big::from_u64(if *b { 1 } else { 0 }),
            Value::Str(t) => match super::bigint::Big::parse(t) {
                Some(b) => b,
                None => return Err(i.throw_kind("SyntaxError", "cannot convert string to a BigInt")),
            },
            Value::Num(n) => match super::bigint::Big::from_f64(*n) {
                Some(b) => b,
                None => return i.range_err("the number is not a safe integer"),
            },
            _ => return i.type_err("cannot convert value to a BigInt"),
        })))
    }, "BigInt", 1, false);
    bigint_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(bigint_proto.clone())));
    bigint_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(bigint_ctor.clone())));
    bigint_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("BigInt")));
    def(&bigint_proto, "toString", |i, t, a| {
        let b = this_bigint(i, &t)?;
        let radix = match a.first() {
            None | Some(Value::Undefined) => 10.0,
            Some(v) => to_integer(i.to_number(v)?),
        };
        if !(2.0..=36.0).contains(&radix) { return i.range_err("toString: radix out of range"); }
        Ok(Value::string(b.to_string_radix(radix as u32)))
    }, 0, fp);
    def(&bigint_proto, "toLocaleString", |i, t, _| {
        let b = this_bigint(i, &t)?;
        Ok(Value::string(b.to_string_radix(10)))
    }, 0, fp);
    def(&bigint_proto, "valueOf", |i, t, _| {
        let b = this_bigint(i, &t)?;
        Ok(Value::BigInt(Rc::new(b)))
    }, 0, fp);
    def(&bigint_ctor, "asIntN", |i, _, a| as_n(i, a, true), 2, fp);
    def(&bigint_ctor, "asUintN", |i, _, a| as_n(i, a, false), 2, fp);
    global.borrow_mut().define("BigInt", Prop::builtin(Value::Obj(bigint_ctor)));

    let bool_ctor = native(Some(function_proto.clone()), |i, _, a| {
        let prim = Value::Bool(a.first().map(|v| v.truthy()).unwrap_or(false));
        if i.native_new { return Ok(Value::Obj(i.to_object(&prim)?)); }
        Ok(prim)
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
    // Eine Stelle je Funktion mit EINEM Argument — die Liste ist die
    // Spezifikation, nicht die Umsetzung.
    macro_rules! m1 {
        ($($n:literal => $f:expr),* $(,)?) => { $(
            def(&math, $n, |i, _, a| {
                let x = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
                let f: fn(f64) -> f64 = $f;
                Ok(Value::Num(f(x)))
            }, 1, fp);
        )* };
    }
    m1! {
        "round" => |x| if x.is_nan() || x.is_infinite() { x } else { libm::floor(x + 0.5) },
        "sign" => |x| if x.is_nan() { f64::NAN } else if x > 0.0 { 1.0 }
                      else if x < 0.0 { -1.0 } else { x },
        "log" => libm::log, "log2" => libm::log2, "log10" => libm::log10,
        "log1p" => libm::log1p, "exp" => libm::exp, "expm1" => libm::expm1,
        "sin" => libm::sin, "cos" => libm::cos, "tan" => libm::tan,
        "asin" => libm::asin, "acos" => libm::acos, "atan" => libm::atan,
        "sinh" => libm::sinh, "cosh" => libm::cosh, "tanh" => libm::tanh,
        "asinh" => libm::asinh, "acosh" => libm::acosh, "atanh" => libm::atanh,
        "cbrt" => libm::cbrt,
        "fround" => |x| x as f32 as f64,
        "f16round" => f16round,
        "clz32" => |x| to_uint32(x).leading_zeros() as f64,
    }
    def(&math, "atan2", |i, _, a| {
        let y = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
        let x = i.to_number(a.get(1).unwrap_or(&Value::Undefined))?;
        Ok(Value::Num(libm::atan2(y, x)))
    }, 2, fp);
    def(&math, "hypot", |i, _, a| {
        let mut sum = 0.0;
        for v in a { let n = i.to_number(v)?; sum += n * n; }
        Ok(Value::Num(libm::sqrt(sum)))
    }, 2, fp);
    def(&math, "imul", |i, _, a| {
        let x = to_int32(i.to_number(a.first().unwrap_or(&Value::Undefined))?);
        let y = to_int32(i.to_number(a.get(1).unwrap_or(&Value::Undefined))?);
        Ok(Value::Num(x.wrapping_mul(y) as f64))
    }, 2, fp);
    def(&math, "random", |i, _, _| Ok(Value::Num(i.next_random())), 0, fp);
    for (n, v) in [("LOG2E", core::f64::consts::LOG2_E), ("LOG10E", core::f64::consts::LOG10_E),
                   ("SQRT1_2", core::f64::consts::FRAC_1_SQRT_2)] {
        math.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    math.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("Math")));
    global.borrow_mut().define("Math", Prop::builtin(Value::Obj(math)));

    // ── Globale Werte + Funktionen ───────────────────────────────────────
    global.borrow_mut().define("undefined", Prop::frozen(Value::Undefined));
    global.borrow_mut().define("NaN", Prop::frozen(Value::Num(f64::NAN)));
    global.borrow_mut().define("Infinity", Prop::frozen(Value::Num(f64::INFINITY)));
    global.borrow_mut().define("globalThis", Prop::builtin(Value::Obj(global.clone())));
    def(&global, "isNaN", |i, _, a| Ok(Value::Bool(i.to_number(a.first().unwrap_or(&Value::Undefined))?.is_nan())), 1, fp);
    def(&global, "isFinite", |i, _, a| Ok(Value::Bool(i.to_number(a.first().unwrap_or(&Value::Undefined))?.is_finite())), 1, fp);
    // `eval` als globale Funktion ist die INDIREKTE Form: sie laeuft im
    // globalen Bereich. Die direkte erkennt der Aufruf selbst (siehe
    // `Interp::is_eval_fn`).
    def(&global, "eval", |i, _, a| {
        let c = a.first().cloned().unwrap_or(Value::Undefined);
        i.perform_eval(&c, None)
    }, 1, fp);
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
    // `Number.parseInt === parseInt` steht so in der Spezifikation — also
    // DASSELBE Objekt weiterreichen und nicht ein zweites bauen.
    for n in ["parseInt", "parseFloat"] {
        let v = global.borrow().get_own(n).and_then(|p| p.value.clone());
        if let Some(v) = v { number_ctor.borrow_mut().define(n, Prop::builtin(v)); }
    }

    // ── Die URI-Funktionen ───────────────────────────────────────────────
    //
    // Nicht Zierde: `encodeURIComponent is not defined` ist auf beiden
    // Wikipedias die ZWEITE Wand, gleich hinter `document.cookie`
    // (`wallcheck WCPAGE=*`). Vier Funktionen mit einer gemeinsamen Tabelle;
    // der Unterschied zwischen ihnen ist genau, welche Zeichen roh
    // durchgehen (ES 19.2.6).
    def(&global, "encodeURIComponent", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        uri_encode(i, &s, "-_.!~*'()")
    }, 1, fp);
    def(&global, "encodeURI", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        uri_encode(i, &s, "-_.!~*'();/?:@&=+$,#")
    }, 1, fp);
    def(&global, "decodeURIComponent", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        uri_decode(i, &s, "")
    }, 1, fp);
    // `decodeURI` laesst die reservierten Zeichen KODIERT stehen — sonst
    // aenderte das Dekodieren die Struktur der Adresse, und ein `%2F` wuerde
    // zu einem Pfadtrenner, der vorher keiner war.
    def(&global, "decodeURI", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        uri_decode(i, &s, ";/?:@&=+$,#")
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
    // Vier Sammlungen, EIN Rumpf — als Makro und nicht als Abschluss, weil
    // die Methoden ihren eigenen Namen brauchen: `Map.prototype.has.call(
    // new Set())` muss werfen, und ein Funktionszeiger sieht keine
    // eingefangene Variable.
    macro_rules! collection {
        ($name:literal, $is_map:literal, $ctor:expr) => {{
        let proto = new_obj(Some(object_proto.clone()));
        let d = |o: &Gc, n: &str, f: NativeFn, l: usize| {
            let g = native(Some(function_proto.clone()), f, n, l, false);
            o.borrow_mut().define(n, Prop::builtin(Value::Obj(g)));
        };
        d(&proto, "has", |i, t, a| {
            this_coll(i, &t, $name)?;
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("@{k}");
            Ok(Value::Bool(matches!(&t, Value::Obj(o) if o.borrow().has_own(&key))))
        }, 1);
        d(&proto, "get", |i, t, a| {
            this_coll(i, &t, $name)?;
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("@{k}");
            Ok(match i.get(&t, &key)? { Value::Undefined => Value::Undefined, v => v })
        }, 1);
        d(&proto, "set", |i, t, a| {
            this_coll(i, &t, $name)?;
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
            this_coll(i, &t, $name)?;
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Value::Obj(o) = &t {
                o.borrow_mut().define(&alloc::format!("@{k}"), Prop {
                    value: Some(Value::Str(k)), get: None, set: None,
                    writable: true, enumerable: false, configurable: true });
            }
            Ok(t)
        }, 1);
        d(&proto, "delete", |i, t, a| {
            this_coll(i, &t, $name)?;
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let key = alloc::format!("@{k}");
            Ok(Value::Bool(matches!(&t, Value::Obj(o) if o.borrow_mut().remove(&key))))
        }, 1);
        d(&proto, "clear", |i, t, _| {
            this_coll(i, &t, $name)?;
            if let Value::Obj(o) = &t {
                let ks = o.borrow().own_keys();
                for k in ks { if k.starts_with('@') { o.borrow_mut().remove(&k); } }
            }
            Ok(Value::Undefined)
        }, 0);
        d(&proto, "forEach", |i, t, a| {
            this_coll(i, &t, $name)?;
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
        d(&proto, "keys", |i, t, _| {
            this_coll(i, &t, $name)?; let v = coll_view(i, &t, 0)?; i.array_iter(v, 0) }, 0);
        d(&proto, "values", |i, t, _| {
            this_coll(i, &t, $name)?; let v = coll_view(i, &t, 1)?; i.array_iter(v, 0) }, 0);
        d(&proto, "entries", |i, t, _| {
            this_coll(i, &t, $name)?; let v = coll_view(i, &t, 2)?; i.array_iter(v, 0) }, 0);
        {
            let key = if $is_map { "entries" } else { "values" };
            let f = proto.borrow().get_own(key).and_then(|p| p.value.clone());
            if let Some(f) = f { proto.borrow_mut().define(SYM_ITERATOR, Prop::builtin(f)); }
        }
        proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str($name)));

        let size = native(Some(function_proto.clone()), |i, t, _| {
            this_coll(i, &t, $name)?;
            let Value::Obj(o) = &t else { return Ok(Value::Num(0.0)) };
            let n = o.borrow().own_keys().iter().filter(|k| k.starts_with('@')).count();
            Ok(Value::Num(n as f64))
        }, "size", 0, false);
        proto.borrow_mut().define("size", Prop {
            value: None, get: Some(Value::Obj(size)), set: None,
            writable: false, enumerable: false, configurable: true });

        let ctor = native(Some(function_proto.clone()), $ctor, $name, 0, true);
        ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
        proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
        global.borrow_mut().define($name, Prop::builtin(Value::Obj(ctor)));
        }};
    }
    collection!("Map", true, (|i: &mut Interp, _: Value, a: &[Value]| coll_new(i, "Map", true, a)) as NativeFn);
    collection!("Set", false, (|i: &mut Interp, _: Value, a: &[Value]| coll_new(i, "Set", false, a)) as NativeFn);
    collection!("WeakMap", true, (|i: &mut Interp, _: Value, a: &[Value]| coll_new(i, "WeakMap", true, a)) as NativeFn);
    collection!("WeakSet", false, (|i: &mut Interp, _: Value, a: &[Value]| coll_new(i, "WeakSet", false, a)) as NativeFn);

    // ── Die sieben Mengenoperationen (ES 2025) ───────────────────────────
    //
    // Sie lesen das Argument ueber `size`/`has`/`keys` — es muss KEIN Set
    // sein, nur mengenaehnlich. Genau das prueft test262, und genau das
    // brauchen Seiten, die eine eigene Menge herumreichen.
    {
        let set_proto = match global.borrow().get_own("Set").and_then(|p| p.value.clone()) {
            Some(v) => match v { Value::Obj(c) => c.borrow().get_own("prototype")
                .and_then(|p| p.value.clone()), _ => None },
            None => None,
        };
        if let Some(Value::Obj(sp)) = set_proto {
            def(&sp, "union", |i, t, a| set_op(i, t, a, SetOp::Union), 1, fp);
            def(&sp, "intersection", |i, t, a| set_op(i, t, a, SetOp::Intersection), 1, fp);
            def(&sp, "difference", |i, t, a| set_op(i, t, a, SetOp::Difference), 1, fp);
            def(&sp, "symmetricDifference", |i, t, a| set_op(i, t, a, SetOp::Symmetric), 1, fp);
            def(&sp, "isSubsetOf", |i, t, a| set_op(i, t, a, SetOp::Subset), 1, fp);
            def(&sp, "isSupersetOf", |i, t, a| set_op(i, t, a, SetOp::Superset), 1, fp);
            def(&sp, "isDisjointFrom", |i, t, a| set_op(i, t, a, SetOp::Disjoint), 1, fp);
        }
        let map_ctor = global.borrow().get_own("Map").and_then(|p| p.value.clone());
        if let Some(Value::Obj(mc)) = map_ctor {
            def(&mc, "groupBy", |i, _, a| {
                let out = coll_new(i, "Map", true, &[])?;
                let Value::Obj(o) = &out else { return Ok(out) };
                group_into(i, a, &o.clone(), true)?;
                Ok(out)
            }, 2, fp);
        }
    }

    // ── Symbol ───────────────────────────────────────────────────────────
    //
    // Ein Symbol ist ein PRIMITIV (`Value::Sym`), kein Objekt. Sein
    // Eigenschaftsname liegt als NUL-praefigierte Zeichenkette in derselben
    // Tabelle wie jeder andere — die Begruendung steht bei `PropName`.
    // `Symbol` IST ein Konstruktor — `new Symbol()` wirft im Rumpf, nicht
    // davor. Der Unterschied ist ueber `isConstructor` beobachtbar.
    let symbol_ctor = native(Some(function_proto.clone()), |i, _, a| {
        if i.native_new { return i.type_err("Symbol is not a constructor"); }
        let desc = match a.first() {
            None | Some(Value::Undefined) => None,
            Some(v) => Some(i.to_string(v)?),
        };
        Ok(i.new_symbol(desc))
    }, "Symbol", 0, true);
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
    {
        let g = native(Some(function_proto.clone()), |i, t, _| {
            let sd = match &t {
                Value::Sym(sd) => sd.clone(),
                Value::Obj(o) => match &o.borrow().kind {
                    ObjKind::SymWrap(sd) => sd.clone(),
                    _ => return i.type_err("not a symbol"),
                },
                _ => return i.type_err("not a symbol"),
            };
            Ok(match &sd.desc { Some(d) => Value::Str(d.clone()), None => Value::Undefined })
        }, "get description", 0, false);
        symbol_proto.borrow_mut().define("description", Prop {
            value: None, get: Some(Value::Obj(g)), set: None,
            writable: false, enumerable: false, configurable: true });
    }
    global.borrow_mut().define("Symbol", Prop::builtin(Value::Obj(symbol_ctor)));

    // ── `Reflect` ────────────────────────────────────────────────────────
    //
    // Kein Konstruktor und keine Funktion, sondern ein Namensraum: jede
    // Methode ist die nackte Spec-Operation, die die entsprechende Syntax
    // sonst versteckt. Deshalb steht hier fast nichts eigenes — jede Zeile
    // ruft dieselbe Hilfe, die auch `o.k`, `o.k = v`, `k in o`, `delete o.k`
    // und `new f(…)` benutzen. Der Unterschied ist allein die Antwort: wo die
    // Syntax wirft oder schweigt, gibt `Reflect` ein `true`/`false` zurueck.
    let reflect = new_obj(Some(object_proto.clone()));
    def(&reflect, "get", |i, _, a| {
        let t = a.first().cloned().unwrap_or(Value::Undefined);
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        i.get(&t, &k)
    }, 2, fp);
    def(&reflect, "set", |i, _, a| {
        let t = a.first().cloned().unwrap_or(Value::Undefined);
        if !matches!(t, Value::Obj(_)) { return i.type_err("Reflect.set on a non-object"); }
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let v = a.get(2).cloned().unwrap_or(Value::Undefined);
        i.set(&t, &k, v, true)?;
        Ok(Value::Bool(true))
    }, 3, fp);
    def(&reflect, "has", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else { return i.type_err("Reflect.has on a non-object") };
        let o = o.clone();
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let r = i.has_prop(&o, &k)?;
        Ok(Value::Bool(r))
    }, 2, fp);
    def(&reflect, "deleteProperty", |i, _, a| {
        let t = a.first().cloned().unwrap_or(Value::Undefined);
        if !matches!(t, Value::Obj(_)) { return i.type_err("Reflect.deleteProperty on a non-object") }
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(i.delete_key(&t, &k)?))
    }, 2, fp);
    def(&reflect, "ownKeys", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else { return i.type_err("Reflect.ownKeys on a non-object") };
        let o = o.clone();
        let all = i.own_keys_of(&o)?;
        let is_px = super::proxy::parts(&o).is_some();
        let mut out: Vec<Value> = all.iter().filter(|k| !is_sym_key(k))
            .map(|k| Value::Str(k.clone())).collect();
        // Erst die Zeichenketten, dann die Symbole — die Reihenfolge steht in
        // der Spec und wird geprueft.
        if is_px {
            out.extend(all.iter().filter(|k| is_sym_key(k))
                .map(|k| Value::Sym(Rc::new(sym_from_key(k)))));
        } else {
            out.extend(o.borrow().own_sym_keys().into_iter()
                .map(|k| Value::Sym(Rc::new(sym_from_key(&k)))));
        }
        Ok(i.new_array(out))
    }, 1, fp);
    def(&reflect, "getPrototypeOf", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return i.type_err("Reflect.getPrototypeOf on a non-object");
        };
        let p = o.borrow().proto.clone();
        Ok(match p { Some(x) => Value::Obj(x), None => Value::Null })
    }, 1, fp);
    def(&reflect, "setPrototypeOf", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return i.type_err("Reflect.setPrototypeOf on a non-object");
        };
        o.borrow_mut().proto = match a.get(1) {
            Some(Value::Obj(p)) => Some(p.clone()),
            _ => None,
        };
        Ok(Value::Bool(true))
    }, 2, fp);
    def(&reflect, "defineProperty", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return i.type_err("Reflect.defineProperty on a non-object");
        };
        let o = o.clone();
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let d = a.get(2).cloned().unwrap_or(Value::Undefined);
        let p = i.to_prop_desc(&d)?;
        // Der Unterschied zu `Object.defineProperty`: hier ist ein
        // Fehlschlag ein `false`, kein Wurf.
        if !o.borrow().extensible && !o.borrow().has_own(&k) {
            return Ok(Value::Bool(false));
        }
        o.borrow_mut().set_prop(k, p);
        Ok(Value::Bool(true))
    }, 3, fp);
    def(&reflect, "getOwnPropertyDescriptor", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return i.type_err("Reflect.getOwnPropertyDescriptor on a non-object");
        };
        let o = o.clone();
        let k = i.to_prop_key(a.get(1).unwrap_or(&Value::Undefined))?;
        let Some(p) = o.borrow().get_own(&k).cloned() else { return Ok(Value::Undefined) };
        Ok(i.from_prop_desc(&p))
    }, 2, fp);
    def(&reflect, "isExtensible", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return i.type_err("Reflect.isExtensible on a non-object");
        };
        let e = o.borrow().extensible;
        Ok(Value::Bool(e))
    }, 1, fp);
    def(&reflect, "preventExtensions", |i, _, a| {
        let Some(Value::Obj(o)) = a.first() else {
            return i.type_err("Reflect.preventExtensions on a non-object");
        };
        o.borrow_mut().extensible = false;
        Ok(Value::Bool(true))
    }, 1, fp);
    def(&reflect, "apply", |i, _, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("Reflect.apply target is not a function") }
        let this = a.get(1).cloned().unwrap_or(Value::Undefined);
        let list = a.get(2).cloned().unwrap_or(Value::Undefined);
        let args = i.elems(&list)?;
        i.call(&f, this, &args)
    }, 3, fp);
    def(&reflect, "construct", |i, _, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_constructor(&f) { return i.type_err("Reflect.construct target is not a constructor"); }
        // FEHLT das dritte Argument, ist das Ziel selbst das Neuziel; steht
        // dort `undefined`, ist es eines und wirft. Der Unterschied ist
        // beobachtbar, und `isConstructor` aus dem test262-Vorspann baut
        // genau darauf.
        if let Some(nt) = a.get(2) {
            if !i.is_constructor(nt) { return i.type_err("Reflect.construct newTarget is not a constructor"); }
        }
        let list = a.get(1).cloned().unwrap_or(Value::Undefined);
        let args = if matches!(list, Value::Undefined) { Vec::new() } else { i.elems(&list)? };
        i.construct(&f, &args)
    }, 2, fp);
    reflect.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("Reflect")));
    global.borrow_mut().define("Reflect", Prop::builtin(Value::Obj(reflect)));

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

    // Der Generatorvertrag haengt darunter — deshalb ist ein Generator selbst
    // iterierbar, ohne dass `generator.rs` ein `Symbol.iterator` setzt.
    let (generator_proto, generator_func_proto) =
        super::generator::install(&iterator_proto, &function_proto);

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
            slot_set(&t, IT_TARGET, Value::Undefined);
            return Ok(i.iter_result(Value::Undefined, true));
        }
        slot_set(&t, IT_INDEX, Value::Num(idx as f64 + 1.0));
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
                slot_set(&t, IT_TARGET, Value::Undefined);
                Ok(i.iter_result(Value::Undefined, true))
            }
            Some(c) => {
                slot_set(&t, IT_INDEX, Value::Num((off + c.len_utf8()) as f64));
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

    // `matchMedia` beantwortet die Frage mit DEM Medienzustand, den die
    // Engine wirklich fuer die Darstellung benutzt (`css::media_matches`) —
    // nicht mit einem festen `false`. Eine Seite, die ihr Layout danach
    // waehlt, bekommt so dieselbe Antwort wie der Kaskadenlauf, und Layout
    // und Skript koennen nicht auseinanderlaufen.
    //
    // Ohne `set_viewport`/`set_media` gibt es die Funktion GAR NICHT: die
    // Medienlage gehoert dem Wirt, und geraten waere sie eine Messung, die
    // keine ist.
    def(&global, "matchMedia", |i, _, a| {
        let q = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let Some((w, dark)) = i.media else {
            return i.type_err("matchMedia needs a viewport (host did not submit one)");
        };
        let m = crate::css::Media::new(w as f32, dark);
        let hit = crate::css::media_matches(&q, m);
        let g = new_obj(Some(i.realm.object_proto.clone()));
        {
            let mut o = g.borrow_mut();
            o.define("matches", Prop::data(Value::Bool(hit)));
            o.define("media", Prop::data(Value::Str(q)));
        }
        // Die Lage aendert sich in beak waehrend eines Laufs nicht — ein
        // angemeldeter Behandler wuerde also nie gerufen. Ihn anzunehmen und
        // zu verwerfen ist trotzdem richtig: die Seite verlaesst sich darauf,
        // dass die Anmeldung nicht wirft.
        for n in ["addListener", "removeListener", "addEventListener", "removeEventListener"] {
            let f = native(Some(i.realm.function_proto.clone()),
                           |_, _, _| Ok(Value::Undefined), n, 2, false);
            g.borrow_mut().define(n, Prop::builtin(Value::Obj(f)));
        }
        Ok(Value::Obj(g))
    }, 1, fp);

    // ── Storage ──────────────────────────────────────────────────────────
    //
    // Im Speicher, nicht auf der Platte. Ein Skript, das `localStorage`
    // ABFRAGT (und das tun sie, als Vertraeglichkeitspruefung), bekommt eine
    // Antwort; was es hineinlegt, ueberlebt die Seite nicht. Das ist eine
    // benannte Luecke — beak muesste es an npkFS haengen, und dann waere die
    // Frage, wem der Speicher gehoert.
    //
    // **Ein Prototyp, zwei Behaelter.** `Storage` ist im Zensus 1179 Aufrufe
    // wert, und die Zeile, die fehlte, war nicht `getItem` — die gab es —
    // sondern der NAME: `x instanceof Storage` und
    // `Storage.prototype.getItem.call(…)` scheitern an einer flachen Huelle,
    // deren Methoden auf ihr selbst sitzen.
    let make_storage_proto = |object_proto: &Gc, function_proto: &Gc| -> Gc {
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
        let len = native(Some(function_proto.clone()), |_, t, _| {
            let Value::Obj(o) = &t else { return Ok(Value::Num(0.0)) };
            let n = o.borrow().own_keys().into_iter().filter(|k| k.starts_with("__s_")).count();
            Ok(Value::Num(n as f64))
        }, "length", 0, false);
        st.borrow_mut().define("length", Prop { value: None, get: Some(Value::Obj(len)), set: None,
            writable: false, enumerable: false, configurable: true });
        st
    };
    let storage_proto = make_storage_proto(&object_proto, &function_proto);
    let storage_ctor = native(Some(function_proto.clone()),
        |i, _, _| i.type_err("Illegal constructor"), "Storage", 0, true);
    storage_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(storage_proto.clone())));
    storage_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(storage_ctor.clone())));
    global.borrow_mut().define("Storage", Prop::builtin(Value::Obj(storage_ctor)));
    let ls = new_obj(Some(storage_proto.clone()));
    let ss = new_obj(Some(storage_proto));
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
    // `atob`/`btoa` — 10 426 Aufrufe im Zensus, die zweitgroesste Einzelluecke
    // nach `addEventListener`. Beide arbeiten auf LATIN-1, nicht auf UTF-8:
    // `btoa("ä")` wirft im Browser, weil `ä` ausserhalb von 0..255 liegt.
    // Das ist kein Detail — wer hier UTF-8 kodiert, gibt fuer jedes Umlaut
    // eine andere Zeichenkette zurueck als jeder Browser.
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    def(&global, "btoa", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut bytes = Vec::with_capacity(s.len());
        for c in s.chars() {
            if (c as u32) > 255 { return i.type_err("btoa: character out of Latin-1 range"); }
            bytes.push(c as u8);
        }
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for ch in bytes.chunks(3) {
            let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(B64[(n >> 18) as usize & 63] as char);
            out.push(B64[(n >> 12) as usize & 63] as char);
            out.push(if ch.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if ch.len() > 2 { B64[n as usize & 63] as char } else { '=' });
        }
        Ok(Value::string(out))
    }, 1, fp);
    def(&global, "atob", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut acc: u32 = 0;
        let mut bits = 0u32;
        let mut out = String::new();
        for c in s.chars() {
            if c.is_ascii_whitespace() || c == '=' { continue }
            let v = match c {
                'A'..='Z' => c as u32 - 'A' as u32,
                'a'..='z' => c as u32 - 'a' as u32 + 26,
                '0'..='9' => c as u32 - '0' as u32 + 52,
                '+' => 62, '/' => 63,
                _ => return i.type_err("atob: not base64"),
            };
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                // Ein Byte wird ein ZEICHEN, nicht ein UTF-8-Byte: `atob`
                // gibt eine Latin-1-Zeichenkette zurueck.
                out.push(((acc >> bits) & 0xff) as u8 as char);
            }
        }
        Ok(Value::string(out))
    }, 1, fp);
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
    // ── Nachzuegler ──────────────────────────────────────────────────────
    //
    // Kein Thema, sondern eine LISTE: Eingebaute, die schlicht fehlten.
    // Gefunden mit einer Probe, die `typeof` ueber alles laufen laesst, was
    // die Spec nennt — nicht geraten, ausgezaehlt.
    def(&array_proto, "at", |i, this, a| {
        let n = array_len(i, &this)? as i64;
        let mut k = to_integer(i.to_number(a.first().unwrap_or(&Value::Undefined))?) as i64;
        if k < 0 { k += n; }
        if k < 0 || k >= n { return Ok(Value::Undefined) }
        i.get(&this, &num_to_string(k as f64))
    }, 1, fp);
    def(&array_proto, "fill", |i, this, a| {
        let this = Value::Obj(i.to_object(&this)?);
        let n = array_len(i, &this)? as i64;
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        let (s, e) = range_args(i, a.get(1), a.get(2), n)?;
        for k in s..e {
            i.tick()?;
            i.set(&this, &num_to_string(k as f64), v.clone(), true)?;
        }
        Ok(this)
    }, 1, fp);
    def(&array_proto, "copyWithin", |i, this, a| {
        let this = Value::Obj(i.to_object(&this)?);
        let n = array_len(i, &this)? as i64;
        let mut t = to_integer(i.to_number(a.first().unwrap_or(&Value::Undefined))?) as i64;
        if t < 0 { t += n; }
        let t = t.clamp(0, n);
        let (s, e) = range_args(i, a.get(1), a.get(2), n)?;
        // Ueber eine KOPIE, sonst ueberschreibt der Lauf seine eigene Quelle,
        // wenn sich Ziel und Bereich ueberlappen.
        let mut buf = Vec::new();
        for k in s..e {
            i.tick()?;
            buf.push(i.get(&this, &num_to_string(k as f64))?);
        }
        for (j, v) in buf.into_iter().enumerate() {
            let at = t + j as i64;
            if at >= n { break }
            i.set(&this, &num_to_string(at as f64), v, true)?;
        }
        Ok(this)
    }, 2, fp);
    def(&array_proto, "flat", |i, this, a| {
        let d = match a.first() {
            None | Some(Value::Undefined) => 1.0,
            Some(v) => to_integer(i.to_number(v)?),
        };
        let mut out = Vec::new();
        flatten(i, &this, d, &mut out)?;
        Ok(i.new_array(out))
    }, 0, fp);
    def(&array_proto, "flatMap", |i, this, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function") }
        let t = a.get(1).cloned().unwrap_or(Value::Undefined);
        let n = array_len(i, &this)? as usize;
        let mut out = Vec::new();
        for k in 0..n {
            i.tick()?;
            let key = num_to_string(k as f64);
            let v = i.get(&this, &key)?;
            let r = i.call(&f, t.clone(), &[v, Value::Num(k as f64), this.clone()])?;
            // Nur EINE Ebene, und nur echte Felder: was kein Feld ist, wird
            // als WERT genommen. `[1,2].flatMap(x => x)` gab sonst `[]` —
            // `flatten` las die `length` einer Zahl.
            let is_arr = matches!(&r, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::Array));
            if is_arr { flatten(i, &r, 0.0, &mut out)?; } else { out.push(r); }
        }
        Ok(i.new_array(out))
    }, 1, fp);
    def(&array_proto, "findLast", |i, this, a| { find_last(i, this, a, false) }, 1, fp);
    def(&array_proto, "findLastIndex", |i, this, a| { find_last(i, this, a, true) }, 1, fp);
    def(&array_proto, "toReversed", |i, this, _| {
        let mut items = i.elems(&this)?;
        items.reverse();
        Ok(i.new_array(items))
    }, 0, fp);
    def(&array_proto, "toSorted", |i, this, a| {
        // Ueber dieselbe `sort` wie in-place — eine zweite Sortierordnung
        // waere genau die Sorte Unterschied, die niemand bemerkt, bis sie
        // zaehlt.
        let items = i.elems(&this)?;
        let arr = i.new_array(items);
        let ap = i.realm.array_proto.clone();
        let sort = i.get(&Value::Obj(ap), "sort")?;
        i.call(&sort, arr.clone(), a)?;
        Ok(arr)
    }, 1, fp);
    def(&array_proto, "with", |i, this, a| {
        let mut items = i.elems(&this)?;
        let n = items.len() as i64;
        let mut k = to_integer(i.to_number(a.first().unwrap_or(&Value::Undefined))?) as i64;
        if k < 0 { k += n; }
        if k < 0 || k >= n { return i.range_err("index out of range") }
        items[k as usize] = a.get(1).cloned().unwrap_or(Value::Undefined);
        Ok(i.new_array(items))
    }, 2, fp);
    def(&array_proto, "toSpliced", |i, this, a| {
        let items = i.elems(&this)?;
        let n = items.len() as i64;
        let mut s = to_integer(i.to_number(a.first().unwrap_or(&Value::Undefined))?) as i64;
        if s < 0 { s += n; }
        let s = s.clamp(0, n);
        let del = match a.get(1) {
            None => n - s,
            Some(v) => (to_integer(i.to_number(v)?) as i64).clamp(0, n - s),
        };
        let mut out: Vec<Value> = items[..s as usize].to_vec();
        out.extend(a.iter().skip(2).cloned());
        out.extend(items[(s + del) as usize..].iter().cloned());
        Ok(i.new_array(out))
    }, 2, fp);
    array_proto.borrow_mut().define(SYM_UNSCOPABLES, Prop {
        value: Some({
            let u = new_obj(None);
            for n in ["at", "copyWithin", "entries", "fill", "find", "findIndex", "findLast",
                      "findLastIndex", "flat", "flatMap", "includes", "keys", "toReversed",
                      "toSorted", "toSpliced", "values"] {
                u.borrow_mut().define(n, Prop::data(Value::Bool(true)));
            }
            Value::Obj(u)
        }),
        get: None, set: None, writable: false, enumerable: false, configurable: true });

    def(&string_proto, "at", |i, this, a| {
        let s = this_string(i, &this)?;
        let ch: Vec<char> = s.chars().collect();
        let n = ch.len() as i64;
        let mut k = to_integer(i.to_number(a.first().unwrap_or(&Value::Undefined))?) as i64;
        if k < 0 { k += n; }
        if k < 0 || k >= n { return Ok(Value::Undefined) }
        let mut t = String::new();
        t.push(ch[k as usize]);
        Ok(Value::string(t))
    }, 1, fp);
    def(&string_proto, "codePointAt", |i, this, a| {
        let s = this_string(i, &this)?;
        let k = to_integer(i.to_number(a.first().unwrap_or(&Value::Undefined))?);
        if k < 0.0 { return Ok(Value::Undefined) }
        // Nach UTF-16-Einheiten gezaehlt, wie die Spec — unsere Texte sind
        // aber `str`. Die Umrechnung ist dieselbe wie in `charCodeAt`.
        let units: Vec<u16> = s.encode_utf16().collect();
        let k = k as usize;
        if k >= units.len() { return Ok(Value::Undefined) }
        let c = units[k];
        if (0xD800..0xDC00).contains(&c) && k + 1 < units.len() {
            let lo = units[k + 1];
            if (0xDC00..0xE000).contains(&lo) {
                let cp = 0x10000 + ((c as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                return Ok(Value::Num(cp as f64));
            }
        }
        Ok(Value::Num(c as f64))
    }, 1, fp);
    def(&string_proto, "substr", |i, this, a| {
        let s = this_string(i, &this)?;
        let ch: Vec<char> = s.chars().collect();
        let n = ch.len() as i64;
        let mut st = to_integer(i.to_number(a.first().unwrap_or(&Value::Undefined))?) as i64;
        if st < 0 { st = (n + st).max(0); }
        let st = st.min(n);
        let len = match a.get(1) {
            None | Some(Value::Undefined) => n - st,
            Some(v) => (to_integer(i.to_number(v)?) as i64).clamp(0, n - st),
        };
        Ok(Value::string(ch[st as usize..(st + len) as usize].iter().collect()))
    }, 2, fp);
    def(&string_proto, "localeCompare", |i, this, a| {
        let s = this_string(i, &this)?;
        let t = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        // Ohne Gebietsschema: der Vergleich ist der gewoehnliche. Das ist
        // erlaubt und ehrlicher als eine erfundene Sortierordnung.
        Ok(Value::Num(if *s < *t { -1.0 } else if *s > *t { 1.0 } else { 0.0 }))
    }, 1, fp);

    def(&global, "escape", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut out = String::new();
        for c in s.encode_utf16() {
            match char::from_u32(c as u32) {
                Some(ch) if ch.is_ascii_alphanumeric() || "@*_+-./".contains(ch) => out.push(ch),
                _ if c < 256 => out.push_str(&alloc::format!("%{c:02X}")),
                _ => out.push_str(&alloc::format!("%u{c:04X}")),
            }
        }
        Ok(Value::string(out))
    }, 1, fp);
    def(&global, "unescape", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let b: Vec<char> = s.chars().collect();
        let mut units: Vec<u16> = Vec::new();
        let mut k = 0;
        while k < b.len() {
            if b[k] == '%' && k + 5 < b.len() && b[k + 1] == 'u' {
                let h: String = b[k + 2..k + 6].iter().collect();
                if let Ok(v) = u16::from_str_radix(&h, 16) { units.push(v); k += 6; continue }
            }
            if b[k] == '%' && k + 2 < b.len() {
                let h: String = b[k + 1..k + 3].iter().collect();
                if let Ok(v) = u8::from_str_radix(&h, 16) { units.push(v as u16); k += 3; continue }
            }
            let mut buf = [0u16; 2];
            units.extend_from_slice(b[k].encode_utf16(&mut buf));
            k += 1;
        }
        Ok(Value::string(String::from_utf16_lossy(&units)))
    }, 1, fp);

    // ── ArrayBuffer, %TypedArray% und DataView ───────────────────────────
    //
    // **Der Puffer ist der Speicher, die Sicht nur eine Sicht darauf.** Zwei
    // Sichten auf denselben Puffer sehen einander; das ist der Sinn der
    // Familie und der Grund, warum die Bytes im `ArrayBuffer` liegen und
    // nicht in der Sicht.
    //
    // `%TypedArray%` selbst ist nicht rufbar und hat keinen Namen im globalen
    // Objekt — es ist der gemeinsame Vorfahr, an dem alle Methoden haengen.
    // Die neun Konstruktoren erben von ihm, ihre Prototypen von seinem.
    let mut ta_protos: HashMap<&'static str, Gc> = HashMap::new();
    let ab_proto = new_obj(Some(object_proto.clone()));
    let ab_ctor = native(Some(function_proto.clone()), |i, _, a| {
        let n = match a.first() {
            None | Some(Value::Undefined) => 0.0,
            Some(v) => i.to_number(v)?,
        };
        if n < 0.0 || n != to_integer(n) || n > MAX_BUFFER_BYTES as f64 {
            return i.range_err("invalid ArrayBuffer length");
        }
        Ok(i.new_buffer(n as usize))
    }, "ArrayBuffer", 1, true);
    ab_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(ab_proto.clone())));
    ab_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ab_ctor.clone())));
    def(&ab_ctor, "isView", |_, _, a| {
        Ok(Value::Bool(matches!(a.first(), Some(Value::Obj(o))
            if matches!(o.borrow().kind, ObjKind::TypedArray(_) | ObjKind::DataView(_)))))
    }, 1, fp);
    let ab_len = native(Some(function_proto.clone()), |i, t, _| {
        let Some(b) = buf_of(&t) else { return i.type_err("not an ArrayBuffer") };
        Ok(Value::Num(if b.detached.get() { 0.0 } else { b.bytes.borrow().len() as f64 }))
    }, "get byteLength", 0, false);
    ab_proto.borrow_mut().define("byteLength", Prop {
        value: None, get: Some(Value::Obj(ab_len)), set: None,
        writable: false, enumerable: false, configurable: true });
    def(&ab_proto, "slice", |i, t, a| {
        let Some(b) = buf_of(&t) else { return i.type_err("not an ArrayBuffer") };
        let n = b.bytes.borrow().len() as i64;
        let (s, e) = range_args(i, a.first(), a.get(1), n)?;
        let part: Vec<u8> = b.bytes.borrow()[s as usize..e as usize].to_vec();
        let out = i.new_buffer(part.len());
        if let Value::Obj(o) = &out {
            if let ObjKind::Buffer(nb) = &o.borrow().kind { *nb.bytes.borrow_mut() = part; }
        }
        Ok(out)
    }, 2, fp);
    ab_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("ArrayBuffer")));
    global.borrow_mut().define("ArrayBuffer", Prop::builtin(Value::Obj(ab_ctor.clone())));

    let ta_proto = new_obj(Some(object_proto.clone()));
    let ta_ctor = native(Some(function_proto.clone()), |i, _, _| {
        i.type_err("Abstract class TypedArray not directly constructable")
    }, "TypedArray", 0, true);
    ta_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(ta_proto.clone())));
    ta_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ta_ctor.clone())));
    // Die vier Auskuenfte sind LESER auf dem gemeinsamen Prototyp, keine
    // Eigenschaften der Sicht — sonst muesste jede Sicht sie mitschleppen und
    // ein abgetrennter Puffer koennte sie nicht mehr aendern.
    for (name, which) in [("length", 0u8), ("byteLength", 1), ("byteOffset", 2)] {
        let g = native(Some(function_proto.clone()), match which {
            0 => |i: &mut Interp, t: Value, _: &[Value]| {
                let Some(x) = this_ta(i, &t) else { return i.type_err("not a TypedArray") };
                Ok(Value::Num(x.live_len() as f64))
            },
            1 => |i: &mut Interp, t: Value, _: &[Value]| {
                let Some(x) = this_ta(i, &t) else { return i.type_err("not a TypedArray") };
                Ok(Value::Num((x.live_len() * x.kind.size()) as f64))
            },
            _ => |i: &mut Interp, t: Value, _: &[Value]| {
                let Some(x) = this_ta(i, &t) else { return i.type_err("not a TypedArray") };
                Ok(Value::Num(if x.live_len() == 0 && x.len > 0 { 0.0 } else { x.offset as f64 }))
            },
        }, "get", 0, false);
        ta_proto.borrow_mut().define(name, Prop {
            value: None, get: Some(Value::Obj(g)), set: None,
            writable: false, enumerable: false, configurable: true });
    }
    let ta_buf = native(Some(function_proto.clone()), |i, t, _| {
        let Some(x) = this_ta(i, &t) else { return i.type_err("not a TypedArray") };
        Ok(Value::Obj(x.buf.clone()))
    }, "get buffer", 0, false);
    ta_proto.borrow_mut().define("buffer", Prop {
        value: None, get: Some(Value::Obj(ta_buf)), set: None,
        writable: false, enumerable: false, configurable: true });
    let ta_tag = native(Some(function_proto.clone()), |i, t, _| {
        Ok(match this_ta(i, &t) { Some(x) => Value::str(x.kind.name()), None => Value::Undefined })
    }, "get [Symbol.toStringTag]", 0, false);
    ta_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop {
        value: None, get: Some(Value::Obj(ta_tag)), set: None,
        writable: false, enumerable: false, configurable: true });
    def(&ta_proto, "set", |i, t, a| {
        let Some(x) = this_ta(i, &t) else { return i.type_err("not a TypedArray") };
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        let off = match a.get(1) { None => 0.0, Some(v) => i.to_number(v)? };
        if off < 0.0 { return i.range_err("offset is negative") }
        let items = i.elems(&src)?;
        if off as usize + items.len() > x.live_len() {
            return i.range_err("source is too large");
        }
        for (k, v) in items.into_iter().enumerate() {
            if x.kind.is_big() {
                let b = i.to_bigint(&v)?;
                ta_write_big(&x, off as usize + k, &b);
            } else {
                let n = i.to_number(&v)?;
                ta_write(&x, off as usize + k, n);
            }
        }
        Ok(Value::Undefined)
    }, 1, fp);
    def(&ta_proto, "subarray", |i, t, a| {
        let Some(x) = this_ta(i, &t) else { return i.type_err("not a TypedArray") };
        let n = x.live_len() as i64;
        let (s, e) = range_args(i, a.first(), a.get(1), n)?;
        // Eine Untersicht teilt den PUFFER — sie kopiert nicht.
        Ok(i.new_view(x.kind, x.buf.clone(), x.offset + s as usize * x.kind.size(),
                      (e - s) as usize))
    }, 2, fp);
    def(&ta_proto, "slice", |i, t, a| {
        let Some(x) = this_ta(i, &t) else { return i.type_err("not a TypedArray") };
        let n = x.live_len() as i64;
        let (s, e) = range_args(i, a.first(), a.get(1), n)?;
        // `slice` KOPIERT, `subarray` nicht — der einzige Unterschied, und er
        // ist der ganze Grund, dass es beide gibt.
        let out = i.new_typed(x.kind, (e - s) as usize);
        if let Value::Obj(o) = out.clone() {
            if let Some(y) = ta_of(&o) {
                for k in 0..(e - s) as usize {
                    if x.kind.is_big() { ta_copy_big_at(&y, k, &x, s as usize + k); }
                    else { ta_write(&y, k, ta_get(&x, s as usize + k)); }
                }
            }
        }
        Ok(out)
    }, 2, fp);
    // Eine Sicht ist iterierbar ueber DIESELBE Funktion wie ein Feld — sie
    // laeuft ueber `length` und Indizes, und beides beantwortet die Sicht.
    if let Some(v) = array_proto.borrow().get_own("values").and_then(|p| p.value.clone()) {
        ta_proto.borrow_mut().define(SYM_ITERATOR, Prop::builtin(v));
    }
    // Und die gewoehnlichen Feldmethoden gelten auch hier, solange sie nur
    // lesen und rechnen. Was ein FELD zurueckgibt statt einer Sicht (`map`,
    // `filter`), bleibt vorerst weg — lieber gar nicht als mit falschem Typ.
    //
    // Sie werden NICHT weitergereicht, sondern umhuellt: `%TypedArray%.
    // prototype.reduceRight.call(undefined)` muss werfen, und das taete die
    // Feldfassung nicht. Der Mantel prueft die Sicht und ruft dann dieselbe
    // Funktion — keine zweite Semantik, nur die fehlende Vorpruefung.
    macro_rules! ta_borrow {
        ($($m:literal),* $(,)?) => { $(
            {
                let orig = array_proto.borrow().get_own($m).and_then(|p| p.value.clone());
                if let Some(orig) = orig {
                    let len = match &orig {
                        Value::Obj(o) => o.borrow().get_own("length")
                            .and_then(|p| p.value.clone())
                            .map(|v| if let Value::Num(n) = v { n as usize } else { 0 })
                            .unwrap_or(0),
                        _ => 0,
                    };
                    let w = native(Some(function_proto.clone()),
                        |i, t, a| ta_forward(i, t, a, $m), $m, len, false);
                    ta_proto.borrow_mut().define($m, Prop::builtin(Value::Obj(w)));
                }
            }
        )* };
    }
    ta_borrow!["forEach", "indexOf", "lastIndexOf", "includes", "join", "reduce",
               "reduceRight", "some", "every", "find", "findIndex", "findLast",
               "findLastIndex", "at", "fill", "reverse", "sort", "entries",
               "keys", "values", "copyWithin", "toLocaleString"];
    {
        let v = ta_proto.borrow().get_own("values").and_then(|p| p.value.clone());
        if let Some(v) = v { ta_proto.borrow_mut().define(SYM_ITERATOR, Prop::builtin(v)); }
    }

    // Die neun Konstruktoren. Ihr Rumpf ist derselbe; nur die Elementart
    // unterscheidet sie, und die steht im Zeiger.
    for (kind, f) in [
        (ElemKind::I8,  (|i: &mut Interp, _: Value, a: &[Value]| ta_new(i, ElemKind::I8, a)) as NativeFn),
        (ElemKind::U8,  |i, _, a| ta_new(i, ElemKind::U8, a)),
        (ElemKind::U8C, |i, _, a| ta_new(i, ElemKind::U8C, a)),
        (ElemKind::I16, |i, _, a| ta_new(i, ElemKind::I16, a)),
        (ElemKind::U16, |i, _, a| ta_new(i, ElemKind::U16, a)),
        (ElemKind::I32, |i, _, a| ta_new(i, ElemKind::I32, a)),
        (ElemKind::U32, |i, _, a| ta_new(i, ElemKind::U32, a)),
        (ElemKind::F32, |i, _, a| ta_new(i, ElemKind::F32, a)),
        (ElemKind::F64, |i, _, a| ta_new(i, ElemKind::F64, a)),
        (ElemKind::I64, |i, _, a| ta_new(i, ElemKind::I64, a)),
        (ElemKind::U64, |i, _, a| ta_new(i, ElemKind::U64, a)),
    ] {
        let proto = new_obj(Some(ta_proto.clone()));
        let ctor = native(Some(ta_ctor.clone()), f, kind.name(), 3, true);
        ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
        ctor.borrow_mut().define("BYTES_PER_ELEMENT",
            Prop::frozen(Value::Num(kind.size() as f64)));
        proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
        proto.borrow_mut().define("BYTES_PER_ELEMENT",
            Prop::frozen(Value::Num(kind.size() as f64)));
        global.borrow_mut().define(kind.name(), Prop::builtin(Value::Obj(ctor)));
        ta_protos.insert(kind.name(), proto);
    }

    let dv_proto = new_obj(Some(object_proto.clone()));
    let dv_ctor = native(Some(function_proto.clone()), |i, _, a| {
        let Some(Value::Obj(b)) = a.first() else { return i.type_err("DataView needs a buffer") };
        let Some(bd) = buf_of(&Value::Obj(b.clone())) else {
            return i.type_err("DataView needs a buffer");
        };
        let have = bd.bytes.borrow().len();
        let off = match a.get(1) { None => 0.0, Some(v) => i.to_number(v)? };
        if off < 0.0 || off as usize > have { return i.range_err("offset out of range") }
        let len = match a.get(2) {
            None | Some(Value::Undefined) => have - off as usize,
            Some(v) => {
                let n = i.to_number(v)?;
                if n < 0.0 || n > MAX_BUFFER_BYTES as f64 {
                    return i.range_err("invalid DataView length");
                }
                n as usize
            }
        };
        if off as usize + len > have { return i.range_err("length out of range") }
        Ok(Value::Obj(new_kind(Some(i.realm.dataview_proto.clone()),
            ObjKind::DataView(Rc::new(DvData { buf: b.clone(), offset: off as usize, len })))))
    }, "DataView", 1, true);
    dv_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(dv_proto.clone())));
    dv_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(dv_ctor.clone())));
    dv_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("DataView")));
    for (name, which) in [("byteLength", 0u8), ("byteOffset", 1)] {
        let g = native(Some(function_proto.clone()), match which {
            0 => |i: &mut Interp, t: Value, _: &[Value]| {
                let Some(d) = dv_of(&t) else { return i.type_err("not a DataView") };
                Ok(Value::Num(d.len as f64))
            },
            _ => |i: &mut Interp, t: Value, _: &[Value]| {
                let Some(d) = dv_of(&t) else { return i.type_err("not a DataView") };
                Ok(Value::Num(d.offset as f64))
            },
        }, "get", 0, false);
        dv_proto.borrow_mut().define(name, Prop {
            value: None, get: Some(Value::Obj(g)), set: None,
            writable: false, enumerable: false, configurable: true });
    }
    let dv_buf = native(Some(function_proto.clone()), |i, t, _| {
        let Some(d) = dv_of(&t) else { return i.type_err("not a DataView") };
        Ok(Value::Obj(d.buf.clone()))
    }, "get buffer", 0, false);
    dv_proto.borrow_mut().define("buffer", Prop {
        value: None, get: Some(Value::Obj(dv_buf)), set: None,
        writable: false, enumerable: false, configurable: true });
    // Je Art ein Leser und ein Schreiber. **Die Bytefolge ist hier eine
    // ANGABE, nicht die der Maschine** — das ist der ganze Unterschied zur
    // getypten Sicht, und deshalb steht `little` in jedem Aufruf.
    for (name, kind) in [("Int8", ElemKind::I8), ("Uint8", ElemKind::U8),
                         ("Int16", ElemKind::I16), ("Uint16", ElemKind::U16),
                         ("Int32", ElemKind::I32), ("Uint32", ElemKind::U32),
                         ("Float32", ElemKind::F32), ("Float64", ElemKind::F64),
                         ("BigInt64", ElemKind::I64), ("BigUint64", ElemKind::U64)] {
        let gname = alloc::format!("get{name}");
        let sname = alloc::format!("set{name}");
        let k = kind;
        let g = native(Some(function_proto.clone()), match k {
            ElemKind::I8 => |i: &mut Interp, t: Value, a: &[Value]| dv_get(i, t, a, ElemKind::I8),
            ElemKind::U8 => |i, t, a| dv_get(i, t, a, ElemKind::U8),
            ElemKind::I16 => |i, t, a| dv_get(i, t, a, ElemKind::I16),
            ElemKind::U16 => |i, t, a| dv_get(i, t, a, ElemKind::U16),
            ElemKind::I32 => |i, t, a| dv_get(i, t, a, ElemKind::I32),
            ElemKind::U32 => |i, t, a| dv_get(i, t, a, ElemKind::U32),
            ElemKind::F32 => |i, t, a| dv_get(i, t, a, ElemKind::F32),
            ElemKind::I64 => |i, t, a| dv_get(i, t, a, ElemKind::I64),
            ElemKind::U64 => |i, t, a| dv_get(i, t, a, ElemKind::U64),
            _ => |i, t, a| dv_get(i, t, a, ElemKind::F64),
        }, &gname, 1, false);
        dv_proto.borrow_mut().define(&gname, Prop::builtin(Value::Obj(g)));
        let sf = native(Some(function_proto.clone()), match k {
            ElemKind::I8 => |i: &mut Interp, t: Value, a: &[Value]| dv_set(i, t, a, ElemKind::I8),
            ElemKind::U8 => |i, t, a| dv_set(i, t, a, ElemKind::U8),
            ElemKind::I16 => |i, t, a| dv_set(i, t, a, ElemKind::I16),
            ElemKind::U16 => |i, t, a| dv_set(i, t, a, ElemKind::U16),
            ElemKind::I32 => |i, t, a| dv_set(i, t, a, ElemKind::I32),
            ElemKind::U32 => |i, t, a| dv_set(i, t, a, ElemKind::U32),
            ElemKind::F32 => |i, t, a| dv_set(i, t, a, ElemKind::F32),
            ElemKind::I64 => |i, t, a| dv_set(i, t, a, ElemKind::I64),
            ElemKind::U64 => |i, t, a| dv_set(i, t, a, ElemKind::U64),
            _ => |i, t, a| dv_set(i, t, a, ElemKind::F64),
        }, &sname, 2, false);
        dv_proto.borrow_mut().define(&sname, Prop::builtin(Value::Obj(sf)));
    }
    global.borrow_mut().define("DataView", Prop::builtin(Value::Obj(dv_ctor)));


    // Platzhalter — `dombind::install` ersetzt sie sofort. Sie stehen hier,
    // weil ein Realm ohne sie nicht baubar waere und `install` den fertigen
    // Realm braucht, um die Prototypen daranzuhaengen.
    let ph = || new_obj(Some(object_proto.clone()));
    let _ = &ta_protos;
    Realm { global, global_env, object_proto: object_proto.clone(), function_proto, array_proto,
            string_proto, number_proto, boolean_proto, error_proto, error_ctors,
            node_proto: ph(), element_proto: ph(), text_proto: ph(), document_proto: ph(),
            event_proto: ph(), token_list_proto: ph(), style_proto: ph(), comment_proto: ph(),
            regexp_proto: ph(), symbol_proto, iterator_proto,
            generator_proto, generator_func_proto, array_iter_proto,
            string_iter_proto, promise_proto: ph(), date_proto: ph(), bigint_proto,
            iter_helper_proto: ph(), iter_wrap_proto: ph(), eval_fn: None,
            html_element_proto: ph(), svg_element_proto: ph(), fragment_proto: ph(),
            tag_protos: HashMap::new(), url_proto: ph(), url_params_proto: ph(),
            ta_protos, typed_proto: ta_proto, buffer_proto: ab_proto,
            dataview_proto: dv_proto }
}

/// `this.length` als Zahl. Eigene Funktion, weil `i.to_number(&i.get(...))`
/// zwei gleichzeitige Ausleihen waeren — und das Aufteilen an jeder Stelle
/// haette den Code nur laenger gemacht.
/// Ein Array in-place durch eine neue Elementfolge ersetzen. Die Grundlage
/// fuer alles, was die Laenge aendert (`shift`, `splice`, `sort`, `reverse`).
/// Ein internes FACH schreiben.
///
/// Diese Namen (` !target`, ` !index`) sind keine Eigenschaften des Objekts —
/// die Objektdarstellung hat nur keinen anderen Ort dafuer. Sie gehen deshalb
/// an der Eigenschaftsmaschinerie vorbei: `Set` mit Wurf-Fahne wuerde am
/// eingefrorenen ` !target` scheitern, und das waere ein Fehler ueber etwas,
/// das ein Skript gar nicht sehen kann.
fn slot_set(t: &Value, key: &str, v: Value) {
    if let Value::Obj(o) = t { o.borrow_mut().define(key, Prop::data(v)); }
}

fn rebuild(i: &mut Interp, this: &Value, items: Vec<Value>) -> C<()> {
    let Value::Obj(o) = this else { return Ok(()) };
    o.borrow_mut().clear_indices();
    let n = items.len();
    for (k, v) in items.into_iter().enumerate() {
        o.borrow_mut().set_prop(Rc::from(num_to_string(k as f64).as_str()), Prop::data(v));
    }
    i.set(this, "length", Value::Num(n as f64), true)
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
    if let Value::Obj(o) = &out {
        o.borrow_mut().define(COLL_KIND, Prop::frozen(Value::str(name)));
    }
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
    let s = this_string(i, &t)?;
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

/// `start`/`end` einer Bereichsangabe, negativ vom Ende gezaehlt.
fn range_args(i: &mut Interp, s: Option<&Value>, e: Option<&Value>, n: i64) -> C<(i64, i64)> {
    let conv = |i: &mut Interp, v: Option<&Value>, d: i64| -> C<i64> {
        Ok(match v {
            None | Some(Value::Undefined) => d,
            Some(x) => {
                let r = to_integer(i.to_number(x)?) as i64;
                if r < 0 { (n + r).max(0) } else { r.min(n) }
            }
        })
    };
    let a = conv(i, s, 0)?;
    let b = conv(i, e, n)?;
    Ok((a, b.max(a)))
}

/// `flat`: Felder bis zur Tiefe `d` ausschuetten. Nur ECHTE Felder werden
/// aufgeloest — ein feldaehnliches Objekt bleibt ein Wert.
fn flatten(i: &mut Interp, v: &Value, d: f64, out: &mut Vec<Value>) -> C<()> {
    let n = array_len(i, v)? as usize;
    for k in 0..n {
        i.tick()?;
        let key = num_to_string(k as f64);
        let has = matches!(v, Value::Obj(o) if o.borrow().has_own(&key));
        if !has { continue }
        let x = i.get(v, &key)?;
        let is_arr = matches!(&x, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::Array));
        if d > 0.0 && is_arr {
            flatten(i, &x, d - 1.0, out)?;
        } else {
            out.push(x);
        }
    }
    Ok(())
}

fn find_last(i: &mut Interp, this: Value, a: &[Value], want_index: bool) -> C<Value> {
    let f = a.first().cloned().unwrap_or(Value::Undefined);
    if !i.is_callable(&f) { return i.type_err("callback is not a function") }
    let t = a.get(1).cloned().unwrap_or(Value::Undefined);
    let n = array_len(i, &this)? as i64;
    let mut k = n - 1;
    while k >= 0 {
        i.tick()?;
        let key = num_to_string(k as f64);
        let v = i.get(&this, &key)?;
        let r = i.call(&f, t.clone(), &[v.clone(), Value::Num(k as f64), this.clone()])?;
        if r.truthy() {
            return Ok(if want_index { Value::Num(k as f64) } else { v });
        }
        k -= 1;
    }
    Ok(if want_index { Value::Num(-1.0) } else { Value::Undefined })
}

// ── Hilfen fuer Puffer und Sichten ───────────────────────────────────────

fn buf_of(v: &Value) -> Option<Rc<BufData>> {
    let Value::Obj(o) = v else { return None };
    match &o.borrow().kind { ObjKind::Buffer(b) => Some(b.clone()), _ => None }
}

fn dv_of(v: &Value) -> Option<Rc<DvData>> {
    let Value::Obj(o) = v else { return None };
    match &o.borrow().kind { ObjKind::DataView(d) => Some(d.clone()), _ => None }
}

fn this_ta(_i: &mut Interp, v: &Value) -> Option<Rc<TaData>> {
    let Value::Obj(o) = v else { return None };
    ta_of(o)
}

/// Ein Element aus einer Sicht — ausserhalb ist es `NaN`, nicht ein Fehler.
fn ta_get(t: &Rc<TaData>, k: usize) -> f64 {
    if k >= t.live_len() { return f64::NAN }
    let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return f64::NAN };
    t.kind.read(&b.bytes.borrow(), t.offset + k * t.kind.size())
}

/// Ein Element einer 64-Bit-Sicht schreiben.
fn ta_write_big(t: &Rc<TaData>, k: usize, v: &super::bigint::Big) {
    if k >= t.live_len() { return }
    let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return };
    let at = t.offset + k * t.kind.size();
    t.kind.write_big(&mut b.bytes.borrow_mut(), at, v);
}

/// Ein Element von einer 64-Bit-Sicht in eine andere.
fn ta_copy_big(dst: &Rc<TaData>, k: usize, src: &Rc<TaData>) { ta_copy_big_at(dst, k, src, k) }

fn ta_copy_big_at(dst: &Rc<TaData>, dk: usize, src: &Rc<TaData>, sk: usize) {
    if sk >= src.live_len() { return }
    let v = {
        let ObjKind::Buffer(sb) = &src.buf.borrow().kind else { return };
        let at = src.offset + sk * src.kind.size();
        src.kind.read_v(&sb.bytes.borrow(), at)
    };
    if let Value::BigInt(b) = v { ta_write_big(dst, dk, &b); }
}

fn ta_write(t: &Rc<TaData>, k: usize, v: f64) {
    if k >= t.live_len() { return }
    let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return };
    let at = t.offset + k * t.kind.size();
    t.kind.write(&mut b.bytes.borrow_mut(), at, v);
}

/// Der gemeinsame Rumpf aller neun Konstruktoren. Vier Formen, und sie sind
/// nicht dasselbe:
///
/// * `new TA(n)` — ein frischer Puffer fuer n Elemente
/// * `new TA(sicht)` — KOPIE, Element fuer Element umgerechnet
/// * `new TA(puffer, versatz, laenge)` — eine SICHT, kein neuer Speicher
/// * `new TA(iterierbares|feldaehnliches)` — Kopie der Werte
fn ta_new(i: &mut Interp, kind: ElemKind, a: &[Value]) -> C<Value> {
    match a.first() {
        None | Some(Value::Undefined) => Ok(i.new_typed(kind, 0)),
        Some(Value::Obj(o)) if matches!(o.borrow().kind, ObjKind::Buffer(_)) => {
            let b = o.clone();
            let have = match &b.borrow().kind {
                ObjKind::Buffer(bd) => bd.bytes.borrow().len(),
                _ => 0,
            };
            let off = match a.get(1) { None | Some(Value::Undefined) => 0.0,
                                       Some(v) => i.to_number(v)? };
            if off < 0.0 || off as usize > have || (off as usize) % kind.size() != 0 {
                return i.range_err("start offset is outside the buffer");
            }
            let off = off as usize;
            let len = match a.get(2) {
                None | Some(Value::Undefined) => {
                    if (have - off) % kind.size() != 0 {
                        return i.range_err("buffer length is not a multiple of the element size");
                    }
                    (have - off) / kind.size()
                }
                Some(v) => {
                    let n = i.to_number(v)?;
                    if n < 0.0 || n > MAX_BUFFER_BYTES as f64 {
                        return i.range_err("invalid typed array length");
                    }
                    n as usize
                }
            };
            if off + len * kind.size() > have {
                return i.range_err("length is outside the buffer");
            }
            Ok(i.new_view(kind, b, off, len))
        }
        Some(Value::Obj(o)) if ta_of(o).is_some() => {
            let src = ta_of(o).unwrap();
            if src.kind.is_big() != kind.is_big() {
                return i.type_err("cannot mix BigInt and number typed arrays");
            }
            let n = src.live_len();
            let out = i.new_typed(kind, n);
            if let Value::Obj(g) = &out {
                if let Some(d) = ta_of(g) {
                    for k in 0..n {
                        if kind.is_big() { ta_copy_big(&d, k, &src); }
                        else { ta_write(&d, k, ta_get(&src, k)); }
                    }
                }
            }
            Ok(out)
        }
        Some(Value::Obj(_)) => {
            let src = a[0].clone();
            // Iterierbar geht vor feldaehnlich — genau wie in der Spec.
            let items = match i.get_iterator(&src) {
                Ok(_) => i.iterate(&src)?,
                Err(_) => i.elems(&src)?,
            };
            let out = i.new_typed(kind, items.len());
            if let Value::Obj(g) = &out {
                if let Some(d) = ta_of(g) {
                    for (k, v) in items.into_iter().enumerate() {
                        if kind.is_big() {
                            let b = i.to_bigint(&v)?;
                            ta_write_big(&d, k, &b);
                        } else {
                            let n = i.to_number(&v)?;
                            ta_write(&d, k, n);
                        }
                    }
                }
            }
            Ok(out)
        }
        Some(v) => {
            let n = i.to_number(v)?;
            if n < 0.0 || n != to_integer(n) || n * kind.size() as f64 > MAX_BUFFER_BYTES as f64 {
                return i.range_err("invalid typed array length");
            }
            Ok(i.new_typed(kind, n as usize))
        }
    }
}

fn dv_get(i: &mut Interp, t: Value, a: &[Value], kind: ElemKind) -> C<Value> {
    // Bei `getFloat*` steht die Bytefolge an Stelle 1, bei den Ganzzahlen
    // ebenso — nur `set*` schiebt sie um eins nach hinten.
    let Some(d) = dv_of(&t) else { return i.type_err("not a DataView") };
    let off = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
    if off < 0.0 || !off.is_finite() { return i.range_err("offset is out of bounds") }
    let off = off as usize;
    if off + kind.size() > d.len { return i.range_err("offset is out of bounds") }
    let little = matches!(kind, ElemKind::I8 | ElemKind::U8)
        || a.get(1).map(|v| v.truthy()).unwrap_or(false);
    let ObjKind::Buffer(b) = &d.buf.borrow().kind else { return i.type_err("detached") };
    let n = kind.size();
    let mut bytes: Vec<u8> = b.bytes.borrow()[d.offset + off..d.offset + off + n].to_vec();
    if !little { bytes.reverse(); }
    Ok(kind.read_v(&bytes, 0))
}

fn dv_set(i: &mut Interp, t: Value, a: &[Value], kind: ElemKind) -> C<Value> {
    let Some(d) = dv_of(&t) else { return i.type_err("not a DataView") };
    let off = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
    if off < 0.0 || !off.is_finite() { return i.range_err("offset is out of bounds") }
    let off = off as usize;
    // Die Umwandlung laeuft VOR der Bereichspruefung — sie ist beobachtbar.
    let big = if kind.is_big() { Some(i.to_bigint(a.get(1).unwrap_or(&Value::Undefined))?) } else { None };
    let v = match &big { Some(_) => 0.0, None => i.to_number(a.get(1).unwrap_or(&Value::Undefined))? };
    if off + kind.size() > d.len { return i.range_err("offset is out of bounds") }
    let little = matches!(kind, ElemKind::I8 | ElemKind::U8)
        || a.get(2).map(|x| x.truthy()).unwrap_or(false);
    let n = kind.size();
    let mut bytes = alloc::vec![0u8; n];
    match &big { Some(b) => kind.write_big(&mut bytes, 0, b), None => kind.write(&mut bytes, 0, v) }
    if !little { bytes.reverse(); }
    let ObjKind::Buffer(b) = &d.buf.borrow().kind else { return i.type_err("detached") };
    b.bytes.borrow_mut()[d.offset + off..d.offset + off + n].copy_from_slice(&bytes);
    Ok(Value::Undefined)
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

/// `toFixed`, mit der Rundung, die JS vorschreibt: auf den BETRAG, und bei
/// genau der Haelfte zur groesseren Zahl — also von der Null WEG.
///
/// Rusts `{:.n}` rundet zur geraden Ziffer, und `(-2.5).toFixed(0)` ist
/// damit `-2` statt `-3`. Der Unterschied faellt nur im Vergleich mit einem
/// echten Motor auf; gefunden hat ihn genau der.
fn fixed(n: f64, d: u32) -> String {
    let neg = n < 0.0 || (n == 0.0 && n.is_sign_negative() && d == 0 && n != 0.0);
    let x = libm::fabs(n);
    let p = libm::pow(10.0, d as f64);
    // `x * p + 0.5` und dann abrunden: der Gleitkommafehler von `x * p`
    // gehoert dazu. `(1.005).toFixed(2)` ist "1.00", WEIL 1.005 als f64
    // knapp darunter liegt — wer das wegrechnet, weicht von jedem Browser ab.
    let scaled = libm::floor(x * p + 0.5);
    let digits = num_to_string(scaled);
    let mut out = String::new();
    if neg && scaled != 0.0 { out.push('-'); }
    if d == 0 { out.push_str(&digits); return out }
    let d = d as usize;
    if digits.len() <= d {
        out.push_str("0.");
        for _ in 0..(d - digits.len()) { out.push('0'); }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..digits.len() - d]);
        out.push('.');
        out.push_str(&digits[digits.len() - d..]);
    }
    out
}

/// Der gemeinsame Rumpf von `encodeURI` und `encodeURIComponent`.
///
/// `keep` sind die Sonderzeichen, die roh durchgehen; Buchstaben und Ziffern
/// gehen immer durch. Kodiert wird UTF-8, Byte fuer Byte — genau so steht es
/// in der Spezifikation, und genau so erwartet es jeder Server.
fn uri_encode(i: &mut Interp, s: &str, keep: &str) -> C<Value> {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || keep.contains(c) { out.push(c); continue }
        // Eine einzelne Haelfte eines Ersatzpaares ist kein Zeichen und laesst
        // sich nicht als UTF-8 schreiben. Der Lexer laesst sie nicht entstehen,
        // aber `String.fromCharCode` schon.
        if (0xD800..0xE000).contains(&(c as u32)) { return Err(i.throw_kind("URIError", "URI malformed")) }
        for b in c.encode_utf8(&mut buf).as_bytes() {
            out.push('%');
            out.push(char::from_digit((b >> 4) as u32, 16).unwrap().to_ascii_uppercase());
            out.push(char::from_digit((b & 15) as u32, 16).unwrap().to_ascii_uppercase());
        }
    }
    Ok(Value::str(&out))
}

/// Der gemeinsame Rumpf von `decodeURI` und `decodeURIComponent`.
///
/// `reserved` sind die Zeichen, deren Kodierung STEHEN bleibt. Ein `%` ohne
/// zwei Hexziffern dahinter ist ein URIError — nicht ein stilles `%`: eine
/// halb dekodierte Adresse sieht aus wie eine ganze.
fn uri_decode(i: &mut Interp, s: &str, reserved: &str) -> C<Value> {
    let b = s.as_bytes();
    let mut bytes: Vec<u8> = Vec::with_capacity(b.len());
    let mut k = 0;
    while k < b.len() {
        if b[k] != b'%' { bytes.push(b[k]); k += 1; continue }
        if k + 2 >= b.len() { return Err(i.throw_kind("URIError", "URI malformed")) }
        let hi = (b[k + 1] as char).to_digit(16);
        let lo = (b[k + 2] as char).to_digit(16);
        let (Some(hi), Some(lo)) = (hi, lo) else {
            return Err(i.throw_kind("URIError", "URI malformed"))
        };
        let v = (hi * 16 + lo) as u8;
        if v < 0x80 && reserved.contains(v as char) {
            bytes.extend_from_slice(&b[k..k + 3]);
        } else {
            bytes.push(v);
        }
        k += 3;
    }
    match String::from_utf8(bytes) {
        Ok(t) => Ok(Value::str(&t)),
        Err(_) => Err(i.throw_kind("URIError", "URI malformed")),
    }
}

/// `__lookupGetter__` / `__lookupSetter__`: die KETTE hoch, bis eine eigene
/// Eigenschaft dieses Namens da ist — und nur wenn die ein Zugriff ist, gibt
/// es etwas zurueck.
fn lookup_accessor(i: &mut Interp, t: Value, a: &[Value], want_set: bool) -> C<Value> {
    let o = i.to_object(&t)?;
    let k = i.to_prop_key(a.first().unwrap_or(&Value::Undefined))?;
    let mut cur = Some(o);
    let mut hops = 0;
    while let Some(c) = cur {
        hops += 1;
        if hops > crate::js::interp::MAX_PROTO_CHAIN { break; }
        let found = c.borrow().get_own(&k).cloned();
        if let Some(p) = found {
            let f = if want_set { p.set.clone() } else { p.get.clone() };
            return Ok(f.unwrap_or(Value::Undefined));
        }
        let n = c.borrow().proto.clone();
        cur = n;
    }
    Ok(Value::Undefined)
}

/// Der gemeinsame Rumpf von `Object.groupBy` und `Map.groupBy`. Der
/// Unterschied ist allein der SCHLUESSEL: dort eine Eigenschaft, hier ein
/// Karteneintrag — und unsere Karte ist ohnehin zeichenkettenbasiert.
fn group_into(i: &mut Interp, a: &[Value], out: &Gc, as_map: bool) -> C<()> {
    let f = a.get(1).cloned().unwrap_or(Value::Undefined);
    if !i.is_callable(&f) { i.type_err::<()>("callback is not a function")?; }
    let items = i.iterate(a.first().unwrap_or(&Value::Undefined))?;
    for (n, v) in items.into_iter().enumerate() {
        i.tick()?;
        let kv = i.call(&f, Value::Undefined, &[v.clone(), Value::Num(n as f64)])?;
        let k = if as_map { alloc::format!("@{}", i.to_string(&kv)?) }
                else { i.to_prop_key(&kv)?.to_string() };
        let have = out.borrow().get_own(&k).and_then(|p| p.value.clone());
        match have {
            Some(arr) => {
                let lv = i.get(&arr, "length")?;
                let len = i.to_number(&lv)?;
                i.set(&arr, &num_to_string(len), v, true)?;
            }
            None => {
                let arr = i.new_array(vec![v]);
                out.borrow_mut().define(&k, Prop::data(arr));
            }
        }
    }
    Ok(())
}

/// Welche der sieben Mengenoperationen. Eine Umsetzung fuer alle, weil sie
/// sich nur darin unterscheiden, was mit einem Schluessel passiert, der auf
/// beiden Seiten (oder nur auf einer) steht.
#[derive(Clone, Copy, PartialEq)]
enum SetOp { Union, Intersection, Difference, Symmetric, Subset, Superset, Disjoint }

/// Die eigenen Eintraege als Schluesselliste (ohne das `@`).
fn set_keys(t: &Value) -> Vec<Rc<str>> {
    let Value::Obj(o) = t else { return Vec::new() };
    o.borrow().own_keys().into_iter().filter(|k| k.starts_with('@')).collect()
}

fn set_op(i: &mut Interp, t: Value, a: &[Value], op: SetOp) -> C<Value> {
    let is_set = matches!(&t, Value::Obj(o)
        if matches!(o.borrow().get_own(COLL_KIND).and_then(|p| p.value.clone()),
                    Some(Value::Str(k)) if &*k == "Set"));
    if !is_set { return i.type_err("Set method on a non-Set receiver"); }
    let other = a.first().cloned().unwrap_or(Value::Undefined);
    if matches!(other, Value::Undefined | Value::Null) {
        return i.type_err("argument is not set-like");
    }
    // Das MENGENPROTOKOLL: `size` als Zahl, `has` und `keys` als Funktionen.
    // Die Reihenfolge der Pruefungen steht in der Spezifikation und ist
    // beobachtbar.
    let sz = i.get(&other, "size")?;
    let szn = i.to_number(&sz)?;
    if szn.is_nan() { return i.type_err("set-like object has no numeric size"); }
    let has = i.get(&other, "has")?;
    if !i.is_callable(&has) { return i.type_err("set-like object has no callable has"); }
    let keys_fn = i.get(&other, "keys")?;
    if !i.is_callable(&keys_fn) { return i.type_err("set-like object has no callable keys"); }

    let mine = set_keys(&t);
    // Nur holen, wer sie braucht — `isSubsetOf` fragt sonst umsonst.
    let theirs: Vec<Rc<str>> = if matches!(op, SetOp::Union | SetOp::Intersection
        | SetOp::Symmetric | SetOp::Superset) {
        let it = i.call(&keys_fn, other.clone(), &[])?;
        let vs = i.iterate(&it)?;
        let mut out = Vec::with_capacity(vs.len());
        for v in vs { out.push(i.to_string(&v)?); }
        out
    } else { Vec::new() };

    let contains = |i: &mut Interp, k: &str| -> C<bool> {
        let r = i.call(&has, other.clone(), &[Value::str(k)])?;
        Ok(r.truthy())
    };

    match op {
        SetOp::Subset => {
            if (mine.len() as f64) > szn { return Ok(Value::Bool(false)); }
            for k in &mine {
                i.tick()?;
                if !contains(i, &k[1..])? { return Ok(Value::Bool(false)); }
            }
            Ok(Value::Bool(true))
        }
        SetOp::Superset => {
            if (mine.len() as f64) < szn { return Ok(Value::Bool(false)); }
            let Value::Obj(o) = &t else { return Ok(Value::Bool(false)) };
            for k in &theirs {
                i.tick()?;
                if !o.borrow().has_own(&alloc::format!("@{k}")) { return Ok(Value::Bool(false)); }
            }
            Ok(Value::Bool(true))
        }
        SetOp::Disjoint => {
            for k in &mine {
                i.tick()?;
                if contains(i, &k[1..])? { return Ok(Value::Bool(false)); }
            }
            Ok(Value::Bool(true))
        }
        _ => {
            let out = coll_new(i, "Set", false, &[])?;
            let put = |out: &Value, k: &str| {
                if let Value::Obj(o) = out {
                    o.borrow_mut().define(&alloc::format!("@{k}"), Prop {
                        value: Some(Value::str(k)), get: None, set: None,
                        writable: true, enumerable: false, configurable: true });
                }
            };
            match op {
                SetOp::Union => {
                    for k in &mine { put(&out, &k[1..]); }
                    for k in &theirs { put(&out, k); }
                }
                SetOp::Intersection => {
                    for k in &mine {
                        i.tick()?;
                        if contains(i, &k[1..])? { put(&out, &k[1..]); }
                    }
                }
                SetOp::Difference => {
                    for k in &mine {
                        i.tick()?;
                        if !contains(i, &k[1..])? { put(&out, &k[1..]); }
                    }
                }
                _ => {
                    let Value::Obj(mo) = &t else { return Ok(out) };
                    for k in &mine {
                        i.tick()?;
                        if !contains(i, &k[1..])? { put(&out, &k[1..]); }
                    }
                    for k in &theirs {
                        i.tick()?;
                        if !mo.borrow().has_own(&alloc::format!("@{k}")) { put(&out, k); }
                    }
                }
            }
            Ok(out)
        }
    }
}

/// Auf die naechste `binary16`-Zahl runden. Rust hat `f16` in `core` nicht
/// stabil, also von Hand: Vorzeichen, 5 Bit Exponent, 10 Bit Mantisse — mit
/// den beiden Randfaellen, an denen so eine Funktion sonst falsch wird
/// (subnormal und Ueberlauf nach Unendlich).
fn f16round(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 || x.is_infinite() { return x; }
    let neg = x < 0.0;
    let a = libm::fabs(x);
    if a >= 65520.0 { return if neg { f64::NEG_INFINITY } else { f64::INFINITY }; }
    // 2^-24 ist die kleinste subnormale binary16; darunter bleibt die Null.
    if a < 1.0 / 33554432.0 { return if neg { -0.0 } else { 0.0 }; }
    let e = libm::floor(libm::log2(a));
    let e = if e < -14.0 { -14.0 } else { e };
    let step = libm::pow(2.0, e - 10.0);
    let mut r = libm::floor(a / step + 0.5) * step;
    // Zur GERADEN runden, wo es genau in der Mitte liegt.
    if libm::fabs(a / step - (libm::floor(a / step) + 0.5)) < 1e-9 {
        let lo = libm::floor(a / step);
        if (lo as i64) % 2 == 0 { r = lo * step; }
    }
    if r >= 65520.0 { return if neg { f64::NEG_INFINITY } else { f64::INFINITY }; }
    if neg { -r } else { r }
}

/// Eine geborgte Feldmethode auf einer Sicht: erst pruefen, dass `this`
/// wirklich eine ist, dann DIESELBE Funktion rufen.
fn ta_forward(i: &mut Interp, t: Value, a: &[Value], name: &str) -> C<Value> {
    if !matches!(&t, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::TypedArray(_))) {
        return i.type_err("TypedArray method on a non-TypedArray receiver");
    }
    let f = {
        let ap = i.realm.array_proto.clone();
        let v = ap.borrow().get_own(name).and_then(|p| p.value.clone());
        match v { Some(v) => v, None => return i.type_err("missing Array method") }
    };
    i.call(&f, t, a)
}

/// `thisNumberValue` — eine Zahl oder ihre Huelle, alles andere wirft. Das
/// ist KEINE Umwandlung: `Number.prototype.toFixed.call("1")` ist ein
/// TypeError, kein "1.0".
fn this_number(i: &mut Interp, t: &Value) -> C<f64> {
    match t {
        Value::Num(n) => Ok(*n),
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::NumWrap(n) => Ok(*n),
            _ => i.type_err("not a number"),
        },
        _ => i.type_err("not a number"),
    }
}

/// Gibt es den Index ueberhaupt? Fuer eine Sicht immer, fuer ein Feld nur,
/// wenn die Eigenschaft (oder eine geerbte) da ist.
fn has_index(i: &mut Interp, this: &Value, k: usize) -> bool {
    match this {
        Value::Obj(o) => {
            if matches!(o.borrow().kind, ObjKind::TypedArray(_)) { return true; }
            let key = num_to_string(k as f64);
            i.has_property(&o.clone(), &key)
        }
        Value::Str(s) => k < s.chars().count(),
        _ => false,
    }
}

/// `RequireObjectCoercible` + `ToString` — der Kopf jeder
/// String.prototype-Methode. `String.prototype.trim.call(null)` ist ein
/// TypeError, nicht `"null"`.
pub fn this_string(i: &mut Interp, t: &Value) -> C<Rc<str>> {
    if matches!(t, Value::Undefined | Value::Null) {
        return i.type_err("String.prototype method called on null or undefined");
    }
    i.to_string(t)
}

/// Ist `this` GENAU diese Sammlung? Der Vermerk `COLL_KIND` steht fuer das
/// interne Feld, das die Spezifikation verlangt — ohne ihn liefe
/// `Map.prototype.has.call({})` still durch und `…call(null)` gaebe `false`
/// statt zu werfen.
fn this_coll(i: &mut Interp, t: &Value, name: &str) -> C<()> {
    let ok = matches!(t, Value::Obj(o)
        if matches!(o.borrow().get_own(COLL_KIND).and_then(|p| p.value.clone()),
                    Some(Value::Str(k)) if &*k == name));
    if ok { Ok(()) } else { i.type_err(&alloc::format!("{name} method on the wrong receiver")) }
}

/// Ist die eigene Eigenschaft aufzaehlbar? Fuer einen Stellvertreter fragt
/// das seine `getOwnPropertyDescriptor`-Falle, nicht die Tabelle darunter.
fn enum_own(i: &mut Interp, o: &Gc, k: &str) -> C<bool> {
    if super::proxy::parts(o).is_some() {
        return Ok(matches!(i.get_own_desc(o, k)?, Some(p) if p.enumerable));
    }
    Ok(o.borrow().is_enumerable(k))
}

/// `thisBigIntValue` — eine grosse Zahl oder ihre Huelle.
fn this_bigint(i: &mut Interp, t: &Value) -> C<super::bigint::Big> {
    match t {
        Value::BigInt(b) => Ok((**b).clone()),
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::BigWrap(b) => Ok((**b).clone()),
            _ => i.type_err("not a BigInt"),
        },
        _ => i.type_err("not a BigInt"),
    }
}

/// `BigInt.asIntN` / `asUintN`: auf `bits` Stellen zuschneiden, mit oder ohne
/// Vorzeichen.
fn as_n(i: &mut Interp, a: &[Value], signed: bool) -> C<Value> {
    let bits = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
    let bits = to_integer(bits);
    if bits < 0.0 || bits > 4294967295.0 { return i.range_err("asIntN: bits out of range"); }
    let v = a.get(1).cloned().unwrap_or(Value::Undefined);
    let p = i.to_primitive(&v, false)?;
    let Value::BigInt(b) = &p else { return i.type_err("asIntN: value is not a BigInt") };
    Ok(Value::BigInt(Rc::new(b.as_n(f64_to_usize(bits) as u64, signed))))
}
