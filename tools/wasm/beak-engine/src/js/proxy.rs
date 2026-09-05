//! `Proxy` und `Reflect`s Gegenstueck dazu.
//!
//! Ein Stellvertreter ist ein Objekt mit einer eigenen Art
//! (`ObjKind::Proxy`); jede Grundoperation des Objektmodells fragt ihn
//! zuerst. Die Haken sitzen deshalb nicht hier, sondern dort, wo die
//! Operation ohnehin steht — `Interp::get`, `set`, `has_property`,
//! `delete_key`, `own_keys_of`, `get_own_desc`, `define_own`,
//! `proto_of`/`set_proto_of`, `call` und `construct`. Diese Datei baut den
//! Konstruktor und die gemeinsame Hilfe, die eine Falle holt.
//!
//! **Benannt statt verschwiegen: die INVARIANTEN sind nicht geprueft.** Die
//! Spezifikation verlangt nach jedem Fallenaufruf einen Abgleich mit dem
//! Ziel (eine nicht konfigurierbare Eigenschaft darf nicht verschwinden, ein
//! nicht erweiterbares Ziel keine neuen Schluessel melden, …). Wir rufen die
//! Falle und glauben ihr. Das ist fuer eine Seite folgenlos — sie belaegt
//! sich selbst —, aber es ist eine Luecke gegen die Spezifikation.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::interp::*;
use super::value::*;

/// Ziel und Behandler eines Stellvertreters, oder `None` nach dem Widerruf.
pub type ProxyCell = Rc<core::cell::RefCell<Option<(Gc, Gc)>>>;

pub fn parts(o: &Gc) -> Option<ProxyCell> {
    match &o.borrow().kind { ObjKind::Proxy(c) => Some(c.clone()), _ => None }
}

/// Ist dieser Wert ein Stellvertreter?
pub fn is_proxy(v: &Value) -> bool {
    matches!(v, Value::Obj(o) if matches!(o.borrow().kind, ObjKind::Proxy(_)))
}

/// Ziel und Falle holen. `Ok(None)` heisst: keine Falle, die Operation geht
/// unveraendert ans Ziel.
pub fn trap(i: &mut Interp, o: &Gc, name: &str) -> C<Option<(Value, Value)>> {
    let Some(cell) = parts(o) else { return Ok(None) };
    let Some((t, h)) = cell.borrow().clone() else {
        return i.type_err("cannot perform this operation on a revoked proxy");
    };
    let hv = Value::Obj(h);
    let f = i.get(&hv, name)?;
    if matches!(f, Value::Undefined | Value::Null) { return Ok(None); }
    if !i.is_callable(&f) { return i.type_err("proxy trap is not a function"); }
    Ok(Some((f, Value::Obj(t))))
}

/// Das Ziel eines Stellvertreters — fuer die Faelle ohne Falle.
pub fn target(i: &mut Interp, o: &Gc) -> C<Gc> {
    let Some(cell) = parts(o) else { return i.type_err("not a proxy") };
    match cell.borrow().clone() {
        Some((t, _)) => Ok(t),
        None => i.type_err("cannot perform this operation on a revoked proxy"),
    }
}

/// Einen Eigenschaftsnamen zurueck in einen JS-Wert — eine Falle bekommt den
/// Schluessel, wie ein Skript ihn geschrieben haette, also ein Symbol als
/// Symbol.
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
    // Der Prototyp des Stellvertreters wird nie gelaufen — jeder Zugriff geht
    // ueber die Fallen —, aber `new Proxy(f, {})` muss aufrufbar bleiben, und
    // dafuer schaut `is_callable` auf das ZIEL.
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
        // Der Widerruf haengt am Stellvertreter selbst: die Funktion findet
        // ihn ueber ein NUL-praefigiertes Feld, weil ein Zeiger keinen
        // Abschluss nimmt.
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
        // `revoke` ruft sich selbst als `this` — dafuer wird es gebunden.
        let bound = new_kind(Some(i.realm.function_proto.clone()), ObjKind::Bound {
            target: f.clone(), this_val: Value::Obj(f), args: Vec::new() });
        o.borrow_mut().define("revoke", Prop::data(Value::Obj(bound)));
        Ok(Value::Obj(o))
    }, "revocable", 2, false);
    ctor.borrow_mut().define("revocable", Prop::builtin(Value::Obj(rev)));

    realm.global.borrow_mut().define("Proxy", Prop::builtin(Value::Obj(ctor)));

    // `Reflect.ownKeys` und die uebrigen Reflect-Funktionen laufen ueber
    // dieselben Grundoperationen wie der Rest — sie brauchen hier nichts.
    let _ = String::new();
}

/// Wo `revoke` seinen Stellvertreter findet.
pub const REVOKE_TARGET: &str = "\0!revoke";
