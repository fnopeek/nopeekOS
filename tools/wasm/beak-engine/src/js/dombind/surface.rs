//! Platform names pages feature-test for: the list interfaces, `window.event`,
//! and small attributes of the window, document and nodes.
//!
//! Each was measured as a read that found nothing on GitHub or DuckDuckGo
//! (`--features miss-census`) and checked against Chromium. A missing name is
//! not neutral: `x instanceof NodeList` throws, and `"PointerEvent" in window`
//! picks a slower or broken path.

use super::*;

/// The prototype of a global interface, if it exists.
fn proto_of(i: &Interp, name: &str) -> Option<Gc> {
    let c = i.realm.global.borrow().get_own(name).and_then(|p| p.value.clone())?;
    let Value::Obj(c) = c else { return None };
    let p = c.borrow().get_own("prototype").and_then(|p| p.value.clone())?;
    match p { Value::Obj(p) => Some(p), _ => None }
}

/// A list of nodes as `NodeList` (static, from `querySelectorAll` and
/// `childNodes`).
///
/// It stays an array underneath, as it always was here; only the prototype
/// changes, so `instanceof NodeList` is true and the array methods pages
/// already use keep working.
pub(super) fn node_list(i: &mut Interp, ids: Vec<u32>) -> Value {
    let vals: Vec<Value> = ids.into_iter().map(|id| wrap(i, id)).collect();
    let arr = i.new_array(vals);
    if let (Value::Obj(o), Some(p)) = (&arr, proto_of(i, "NodeList")) { o.borrow_mut().proto = Some(p) }
    arr
}

/// The same as `HTMLCollection` (`children`, `getElementsBy*`, `forms`,
/// `form.elements`, `select.options`), with the named properties: an
/// element's `id`, or the `name` of the elements that may carry one, reads
/// it from the collection (`form.elements.q`, `document.forms.login`).
pub(super) fn html_collection(i: &mut Interp, ids: Vec<u32>) -> Value {
    let named: Vec<(Rc<str>, u32)> = match &i.doc {
        Some(d) => ids.iter().flat_map(|&id| {
            let n = &d.nodes[id as usize];
            let mut v = Vec::new();
            if let Some(x) = n.attr("id").filter(|x| !x.is_empty()) { v.push((x.clone(), id)) }
            if is_named_by_name(&n.tag) {
                if let Some(x) = n.attr("name").filter(|x| !x.is_empty()) { v.push((x.clone(), id)) }
            }
            v
        }).collect(),
        None => Vec::new(),
    };
    let vals: Vec<Value> = ids.into_iter().map(|id| wrap(i, id)).collect();
    let arr = i.new_array(vals);
    let Value::Obj(o) = &arr else { return arr };
    if let Some(p) = proto_of(i, "HTMLCollection") { o.borrow_mut().proto = Some(p) }
    for (k, id) in named {
        // An index wins over a name, and the first element with a name wins.
        if array_index(&k).is_some() || o.borrow().get_own(&k).is_some() { continue }
        let v = wrap(i, id);
        o.borrow_mut().define(&k, Prop { value: Some(v), get: None, set: None,
            writable: false, enumerable: false, configurable: true });
    }
    arr
}

/// The elements whose `name` attribute names them in an `HTMLCollection`
/// (DOM §4.2.10.2: the HTML namespace and `name` on these).
fn is_named_by_name(tag: &str) -> bool {
    matches!(tag, "a" | "area" | "embed" | "form" | "frame" | "frameset" | "iframe" | "img"
        | "object" | "button" | "fieldset" | "input" | "output" | "select" | "textarea"
        | "meta" | "map")
}

/// The slot holding the event being dispatched, read by `window.event`.
pub(super) const CUR_EVENT: &str = "\0!window.event";

/// Set `window.event` for the duration of a listener call, returning what
/// was there before. HTML §8.1.8.2 "current event".
pub(super) fn enter_event(i: &mut Interp, ev: &Value) -> Value {
    let g = i.realm.global.clone();
    let prev = g.borrow().get_own(CUR_EVENT).and_then(|p| p.value.clone()).unwrap_or(Value::Undefined);
    g.borrow_mut().define(CUR_EVENT, Prop { value: Some(ev.clone()), get: None, set: None,
        writable: true, enumerable: false, configurable: true });
    prev
}

pub(super) fn leave_event(i: &mut Interp, prev: Value) {
    let g = i.realm.global.clone();
    g.borrow_mut().define(CUR_EVENT, Prop { value: Some(prev), get: None, set: None,
        writable: true, enumerable: false, configurable: true });
}

pub(super) fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let g = realm.global.clone();

    // ── NodeList, HTMLCollection ─────────────────────────────────────────
    //
    // Both chain to `Array.prototype` here, which a browser's do not: the
    // lists have always been arrays in beak, and pages that call `.map` on
    // one would break if that changed now. What a page tests for —
    // `instanceof`, `item`, `forEach`, iteration — is right.
    let ap = realm.array_proto.clone();
    let nl = new_obj(Some(ap.clone()));
    let nl_ctor = native(Some(fp.clone()), |i, _, _| i.type_err("Illegal constructor"), "NodeList", 0, true);
    let hc = new_obj(Some(ap));
    let hc_ctor = native(Some(fp.clone()), |i, _, _| i.type_err("Illegal constructor"), "HTMLCollection", 0, true);
    for (p, c, name) in [(&nl, &nl_ctor, "NodeList"), (&hc, &hc_ctor, "HTMLCollection")] {
        c.borrow_mut().define("prototype", Prop::frozen(Value::Obj(p.clone())));
        p.borrow_mut().define("constructor", Prop::builtin(Value::Obj(c.clone())));
        p.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str(name)));
        meth(p, "item", |i, t, a| {
            let n = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
            // `u32`, not `u64`: forge has no f64-to-u64 conversion, and no list is
            // that long.
            let k = if n.is_finite() && n >= 0.0 && n < 4294967296.0 { n as u32 } else { return Ok(Value::Null) };
            let v = i.get(&t, &alloc::format!("{k}"))?;
            Ok(if matches!(v, Value::Undefined) { Value::Null } else { v })
        }, 1, &fp);
        g.borrow_mut().define(name, Prop::builtin(Value::Obj(c.clone())));
    }
    meth(&hc, "namedItem", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let n = match i.get(&t, "length")? { Value::Num(n) => n as usize, _ => 0 };
        for x in 0..n {
            let el = i.get(&t, &alloc::format!("{x}"))?;
            let Ok(id) = node_of(i, &el) else { continue };
            let hit = i.doc.as_ref().is_some_and(|d| {
                let nd = &d.nodes[id as usize];
                nd.attr("id").is_some_and(|v| **v == *k)
                    || (is_named_by_name(&nd.tag) && nd.attr("name").is_some_and(|v| **v == *k))
            });
            if hit { return Ok(el) }
        }
        Ok(Value::Null)
    }, 1, &fp);

    // ── window ───────────────────────────────────────────────────────────
    //
    // `event` is [Replaceable]: a page's own `var event` or assignment
    // replaces it with a data property.
    let ev_get = native(Some(fp.clone()), |i, _, _| {
        Ok(i.realm.global.borrow().get_own(CUR_EVENT).and_then(|p| p.value.clone()).unwrap_or(Value::Undefined))
    }, "get event", 0, false);
    let ev_set = native(Some(fp.clone()), |i, _, a| {
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        i.realm.global.borrow_mut().define("event", Prop::data(v));
        Ok(Value::Undefined)
    }, "set event", 1, false);
    g.borrow_mut().define("event", Prop { value: None, get: Some(Value::Obj(ev_get)),
        set: Some(Value::Obj(ev_set)), writable: false, enumerable: false, configurable: true });
    // A top-level document: no frame element, no child frames. `length` is
    // [Replaceable] like `event`, so a page's global `var length` works.
    getter(&g, "frameElement", |_, _, _| Ok(Value::Null), &fp);
    accessor(&g, "length", |_, _, _| Ok(Value::Num(0.0)), |i, _, a| {
        let v = a.first().cloned().unwrap_or(Value::Undefined);
        i.realm.global.borrow_mut().define("length", Prop::data(v));
        Ok(Value::Undefined)
    }, &fp);

    // ── document and nodes ───────────────────────────────────────────────
    let dp = realm.document_proto.clone();
    // Never a prerender: beak only runs what is shown.
    getter(&dp, "prerendering", |_, _, _| Ok(Value::Bool(false)), &fp);
    // `baseURI` (DOM §4.4): the document's base URL, the same for every node.
    getter(&realm.node_proto, "baseURI", |i, _, _| {
        Ok(match doc_base(i) { Some(p) => Value::string(p.href()), None => Value::string(i.loc_href.clone()) })
    }, &fp);
    // The prefixed alias of `matches`, still used by older polyfills.
    let m = realm.element_proto.borrow().get_own("matches").and_then(|p| p.value.clone());
    if let Some(m) = m { realm.element_proto.borrow_mut().define("webkitMatchesSelector", Prop::builtin(m)); }
}
