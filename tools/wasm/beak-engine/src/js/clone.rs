//! `structuredClone` (HTML §2.7.3, StructuredSerialize + Deserialize in one
//! pass) for the value kinds of the language core.
//!
//! Cycles and shared references keep their shape: every object is cloned once
//! and looked up by address afterwards. Functions, symbols, promises, proxies,
//! weak collections and DOM objects are not serializable and throw a
//! `DataCloneError`. Not implemented: the transfer list.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::interp::{C, Interp, Realm};
use super::value::*;

pub fn install(realm: &mut Realm) {
    let f = native(Some(realm.function_proto.clone()), |i, _, a| {
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        let mut memo = BTreeMap::new();
        clone(i, &v, &mut memo)
    }, "structuredClone", 1, false);
    realm.global.borrow_mut().define("structuredClone", Prop::builtin(Value::Obj(f)));
}

type Memo = BTreeMap<usize, Value>;

/// A `DOMException` named `DataCloneError` when the host has one, else an
/// `Error` with that name: page code checks `e.name`.
fn data_clone_error<T>(i: &mut Interp, what: &str) -> C<T> {
    let msg = alloc::format!("{what} could not be cloned");
    let g = Value::Obj(i.realm.global.clone());
    let ctor = i.get(&g, "DOMException")?;
    if i.is_constructor(&ctor) {
        let e = i.construct(&ctor, &[Value::str(&msg), Value::str("DataCloneError")])?;
        return Err(super::interp::Abrupt::Throw(e));
    }
    let e = i.throw_kind("Error", &msg);
    if let super::interp::Abrupt::Throw(Value::Obj(o)) = &e {
        o.borrow_mut().define("name", Prop::builtin(Value::str("DataCloneError")));
    }
    Err(e)
}

enum Shape {
    Plain,
    Array,
    Error,
    Wrap(ObjKind),
    Date(f64),
    Regex(Rc<str>),
    Buffer(Vec<u8>),
    Typed(Rc<TaData>),
    View(Rc<DvData>),
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
    No(&'static str),
}

fn shape(i: &Interp, o: &Gc) -> Shape {
    let b = o.borrow();
    let kind = b.get_own(COLL_KIND).and_then(|p| p.value.clone());
    match &b.kind {
        ObjKind::Plain | ObjKind::Arguments => {
            // A DOM wrapper is a platform object, not data.
            let mut p = b.proto.clone();
            while let Some(x) = p {
                if Rc::ptr_eq(&x, &i.realm.event_target_proto) { return Shape::No("a platform object") }
                p = x.borrow().proto.clone();
            }
            Shape::Plain
        }
        ObjKind::Array => Shape::Array,
        ObjKind::Error => Shape::Error,
        ObjKind::BoolWrap(v) => Shape::Wrap(ObjKind::BoolWrap(*v)),
        ObjKind::NumWrap(v) => Shape::Wrap(ObjKind::NumWrap(*v)),
        ObjKind::StrWrap(v) => Shape::Wrap(ObjKind::StrWrap(v.clone())),
        ObjKind::BigWrap(v) => Shape::Wrap(ObjKind::BigWrap(v.clone())),
        ObjKind::Date(t) => Shape::Date(t.get()),
        ObjKind::Regex(r) => Shape::Regex(Rc::from(r.source.as_str())),
        ObjKind::Buffer(d) => {
            if d.detached.get() { return Shape::No("a detached ArrayBuffer") }
            Shape::Buffer(d.bytes.borrow().clone())
        }
        ObjKind::TypedArray(t) => Shape::Typed(t.clone()),
        ObjKind::DataView(d) => Shape::View(d.clone()),
        ObjKind::Collection(c) => match kind {
            Some(Value::Str(k)) if &*k == "Map" => Shape::Map(c.borrow().pairs()),
            Some(Value::Str(k)) if &*k == "Set" =>
                Shape::Set(c.borrow().pairs().into_iter().map(|(k, _)| k).collect()),
            _ => Shape::No("a weak collection"),
        },
        ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. } => Shape::No("a function"),
        ObjKind::SymWrap(_) => Shape::No("a symbol"),
        ObjKind::Promise(_) => Shape::No("a promise"),
        ObjKind::Proxy(_) => Shape::No("a proxy"),
        ObjKind::Generator(_) => Shape::No("a generator"),
        ObjKind::ModuleNs(_) | ObjKind::Dataset(_) => Shape::No("a platform object"),
    }
}

fn clone(i: &mut Interp, v: &Value, memo: &mut Memo) -> C<Value> {
    let o = match v {
        Value::Obj(o) => o.clone(),
        Value::Sym(_) => return data_clone_error(i, "a symbol"),
        other => return Ok(other.clone()),
    };
    let key = Rc::as_ptr(&o) as *const () as usize;
    if let Some(c) = memo.get(&key) { return Ok(c.clone()) }
    i.tick()?;
    let out = match shape(i, &o) {
        Shape::No(what) => return data_clone_error(i, what),
        Shape::Plain => {
            let c = Value::Obj(new_obj(Some(i.realm.object_proto.clone())));
            memo.insert(key, c.clone());
            copy_props(i, &o, &c, memo)?;
            return Ok(c);
        }
        Shape::Array => {
            let c = i.new_array(Vec::new());
            memo.insert(key, c.clone());
            let len = i.get(v, "length")?;
            i.set(&c, "length", len, true)?;
            copy_props(i, &o, &c, memo)?;
            return Ok(c);
        }
        Shape::Error => {
            // The name picks the prototype; only the standard names survive.
            let name = i.get(v, "name")?;
            let name = i.to_string(&name)?;
            let proto = i.realm.error_ctors.iter()
                .find(|(n, _)| **n == &*name).map(|(_, c)| c.clone())
                .unwrap_or_else(|| i.realm.error_proto.clone());
            let e = new_kind(Some(proto), ObjKind::Error);
            let c = Value::Obj(e.clone());
            memo.insert(key, c.clone());
            for k in ["message", "stack", "cause"] {
                let own = i.get_own_desc(&o, k)?;
                if let Some(p) = own {
                    let val = match p.value { Some(x) => x, None => i.get(v, k)? };
                    let val = if k == "cause" { clone(i, &val, memo)? }
                              else { Value::Str(i.to_string(&val)?) };
                    e.borrow_mut().define(k, Prop::builtin(val));
                }
            }
            return Ok(c);
        }
        Shape::Wrap(kind) => {
            let proto = o.borrow().proto.clone();
            Value::Obj(new_kind(proto, kind))
        }
        Shape::Date(t) => {
            Value::Obj(new_kind(Some(i.realm.date_proto.clone()),
                ObjKind::Date(Rc::new(core::cell::Cell::new(t)))))
        }
        Shape::Regex(src) => {
            let flags = i.get(v, "flags")?;
            let flags = i.to_string(&flags)?;
            super::regexp::make(i, &src, &flags)?
        }
        Shape::Buffer(bytes) => {
            let c = i.new_buffer(0);
            if let Value::Obj(g) = &c {
                if let ObjKind::Buffer(d) = &g.borrow().kind { *d.bytes.borrow_mut() = bytes; }
            }
            c
        }
        Shape::Typed(t) => {
            let buf = clone(i, &Value::Obj(t.buf.clone()), memo)?;
            let Value::Obj(b) = buf else { return data_clone_error(i, "a typed array") };
            i.new_view(t.kind, b, t.offset, t.len)
        }
        Shape::View(d) => {
            let buf = clone(i, &Value::Obj(d.buf.clone()), memo)?;
            let Value::Obj(b) = buf else { return data_clone_error(i, "a DataView") };
            Value::Obj(new_kind(Some(i.realm.dataview_proto.clone()),
                ObjKind::DataView(Rc::new(DvData { buf: b, offset: d.offset, len: d.len }))))
        }
        Shape::Map(pairs) => {
            let (c, data) = new_collection(i, "Map")?;
            memo.insert(key, c.clone());
            for (k, val) in pairs {
                let k = clone(i, &k, memo)?;
                let val = clone(i, &val, memo)?;
                data.borrow_mut().set(k, val);
            }
            return Ok(c);
        }
        Shape::Set(keys) => {
            let (c, data) = new_collection(i, "Set")?;
            memo.insert(key, c.clone());
            for k in keys {
                let k = clone(i, &k, memo)?;
                data.borrow_mut().set(k.clone(), k);
            }
            return Ok(c);
        }
    };
    memo.insert(key, out.clone());
    Ok(out)
}

/// An empty `Map` or `Set` built by its constructor, and its storage.
fn new_collection(i: &mut Interp, name: &str) -> C<(Value, Rc<RefCell<CollData>>)> {
    let g = Value::Obj(i.realm.global.clone());
    let ctor = i.get(&g, name)?;
    let c = i.construct(&ctor, &[])?;
    let data = match &c {
        Value::Obj(o) => match &o.borrow().kind { ObjKind::Collection(d) => Some(d.clone()), _ => None },
        _ => None,
    };
    match data { Some(d) => Ok((c, d)), None => data_clone_error(i, name) }
}

/// Own enumerable string-keyed properties, read with `[[Get]]`, as the spec's
/// object serialization does.
fn copy_props(i: &mut Interp, src: &Gc, dst: &Value, memo: &mut Memo) -> C<()> {
    let sv = Value::Obj(src.clone());
    for k in i.own_keys_of(src)? {
        if !i.get_own_desc(src, &k)?.is_some_and(|p| p.enumerable) { continue }
        let val = i.get(&sv, &k)?;
        let val = clone(i, &val, memo)?;
        i.set(dst, &k, val, true)?;
    }
    Ok(())
}
