//! `Proxy` and `Proxy.revocable`.
//!
//! A proxy is an object of its own kind (`ObjKind::Proxy`) that every
//! object-model operation consults first. The hooks therefore live where
//! the operations are (`Interp::get`, `set`, `has_property`, `delete_key`,
//! `own_keys_of`, `get_own_desc`, `define_own`, `proto_of`/`set_proto_of`,
//! `call`, `construct`); this file builds the constructor and the shared
//! trap lookup.
//!
//! Not implemented: the post-trap invariant checks against the target
//! (ES 10.5). Trap results are trusted as returned.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::interp::*;
use super::value::*;

/// Target and handler of a proxy, or `None` once revoked.
pub type ProxyCell = Rc<core::cell::RefCell<Option<(Gc, Gc)>>>;

pub fn parts(o: &Gc) -> Option<ProxyCell> {
    match &o.borrow().kind { ObjKind::Proxy(c) => Some(c.clone()), _ => None }
}

/// Is this value a proxy?
pub fn is_proxy(v: &Value) -> bool {
    matches!(v, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::Proxy(_)))
}

/// Fetch target and trap. `Ok(None)` means no trap: the operation goes to
/// the target unchanged.
pub fn trap(i: &mut Interp, o: &Gc, name: &str) -> C<Option<(Value, Value, Value)>> {
    let Some(cell) = parts(o) else { return Ok(None) };
    let Some((t, h)) = cell.borrow().clone() else {
        return i.type_err("cannot perform this operation on a revoked proxy");
    };
    let hv = Value::Obj(h);
    let f = i.get(&hv, name)?;
    if matches!(f, Value::Undefined | Value::Null) { return Ok(None); }
    if !i.is_callable(&f) { return i.type_err("proxy trap is not a function"); }
    // The handler is the trap's receiver (`Call(trap, handler, args)`,
    // ES 10.5.x); class-based handlers read their own fields through `this`.
    Ok(Some((f, hv, Value::Obj(t))))
}

/// The proxy's target, for operations without a trap.
pub fn target(i: &mut Interp, o: &Gc) -> C<Gc> {
    let Some(cell) = parts(o) else { return i.type_err("not a proxy") };
    match cell.borrow().clone() {
        Some((t, _)) => Ok(t),
        None => i.type_err("cannot perform this operation on a revoked proxy"),
    }
}

/// A property key back as a JS value: a trap sees the key as script would
/// have written it, so a symbol stays a symbol.
pub fn key_value(key: &str) -> Value {
    if is_sym_key(key) {
        Value::Sym(Rc::new(sym_from_key(&PropName::from(key))))
    } else {
        Value::str(key)
    }
}

fn make(i: &mut Interp, a: &[Value]) -> C<(Gc, ProxyCell)> {
    let (Some(Value::Obj(t)), Some(Value::Obj(h))) = (a.first(), a.get(1)) else {
        return i.type_err("Proxy: target and handler must be objects");
    };
    let cell: ProxyCell = Rc::new(core::cell::RefCell::new(Some((t.clone(), h.clone()))));
    // The proxy's own prototype is never walked (every access goes through
    // the traps); `is_callable` looks at the target so `new Proxy(f, {})`
    // stays callable.
    let g = new_kind(None, ObjKind::Proxy(cell.clone()));
    Ok((g, cell))
}

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let ctor = native(Some(fp.clone()), |i, _, a| {
        if !i.native_new { return i.type_err("Proxy requires new"); }
        let (g, _) = make(i, a)?;
        Ok(Value::Obj(g))
    }, "Proxy", 2, true);

    let rev = native(Some(fp.clone()), |i, _, a| {
        let (g, cell) = make(i, a)?;
        let o = new_obj(Some(i.realm.object_proto.clone()));
        o.borrow_mut().define("proxy", Prop::data(Value::Obj(g)));
        // The revoke function finds its proxy through a NUL-prefixed
        // property, since a native fn pointer cannot capture.
        let f = native(Some(i.realm.function_proto.clone()), |i, t, _| {
            let Value::Obj(o) = &t else { return Ok(Value::Undefined) };
            let p = o.borrow().get_own(REVOKE_TARGET).and_then(|p| p.value.clone());
            let _ = i;
            if let Some(Value::Obj(px)) = p {
                if let Some(c) = parts(&px) { *c.borrow_mut() = None; }
            }
            Ok(Value::Undefined)
        }, "", 0, false);
        f.borrow_mut().define(REVOKE_TARGET, Prop {
            value: o.borrow().get_own("proxy").and_then(|p| p.value.clone()),
            get: None, set: None, writable: false, enumerable: false, configurable: false });
        // `revoke` is called with itself as `this`, hence the binding.
        let bound = new_kind(Some(i.realm.function_proto.clone()), ObjKind::Bound {
            target: f.clone(), this_val: Value::Obj(f), args: Vec::new() });
        o.borrow_mut().define("revoke", Prop::data(Value::Obj(bound)));
        Ok(Value::Obj(o))
    }, "revocable", 2, false);
    ctor.borrow_mut().define("revocable", Prop::builtin(Value::Obj(rev)));

    realm.global.borrow_mut().define("Proxy", Prop::builtin(Value::Obj(ctor)));

    // The remaining Reflect functions go through the same object-model
    // operations and need nothing here.
    let _ = String::new();
}

/// Where `revoke` finds its proxy.
pub const REVOKE_TARGET: &str = "\0!revoke";
