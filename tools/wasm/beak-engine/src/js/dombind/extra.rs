//! Smaller DOM interfaces: iteration on the list types, constructible
//! `DocumentFragment` and `CSSStyleSheet`, shadow roots, `ElementInternals`,
//! the popover API, `<dialog>`, `checkVisibility`, `PerformanceObserver`.
//!
//! Not rendered: shadow trees (the host's light DOM is laid out instead),
//! `adoptedStyleSheets` (stored and readable, not in the cascade), the top
//! layer. The states are real and exposed (`DomNode::ui`, `:popover-open`,
//! `:modal`), so the cascade can pick them up.

use super::*;

fn hide(v: Value) -> Prop {
    Prop { value: Some(v), get: None, set: None, writable: true, enumerable: false, configurable: true }
}

fn hslot(_i: &mut Interp, t: &Value, k: &str) -> Value {
    match t { Value::Obj(o) => o.borrow().get_own(k).and_then(|p| p.value.clone()), _ => None }
        .unwrap_or(Value::Undefined)
}

/// The prototype for an object a constructor builds: the receiver's (a
/// subclass via `super()` or `Reflect.construct`), else the global
/// interface's.
fn ctor_proto(i: &mut Interp, this: &Value, name: &str) -> Gc {
    if let Value::Obj(t) = this {
        if let Some(p) = t.borrow().proto.clone() { return p }
    }
    let c = i.realm.global.borrow().get_own(name).and_then(|p| p.value.clone());
    let p = match c { Some(Value::Obj(c)) => c.borrow().get_own("prototype").and_then(|p| p.value.clone()), _ => None };
    match p { Some(Value::Obj(p)) => p, _ => i.realm.object_proto.clone() }
}

/// An array iterator over these values: the shape every list type uses for
/// `entries`/`keys`/`values`/`@@iterator` (WebIDL §3.7.9).
fn iter_of(i: &mut Interp, items: Vec<Value>) -> C<Value> {
    let arr = i.new_array(items);
    let it = i.get(&arr, SYM_ITERATOR)?;
    i.call(&it, arr, &[])
}

const SS_RULES: &str = "\0!ss.rules";
const SS_DISABLED: &str = "\0!ss.disabled";
const ADOPTED: &str = "\0!adopted";
const INTERNALS: &str = "\0!internals";
const IN_EL: &str = "\0!int.el";
const IN_VALUE: &str = "\0!int.value";
const IN_VALIDITY: &str = "\0!int.validity";
const IN_MESSAGE: &str = "\0!int.message";
const DLG_RETURN: &str = "\0!dlg.return";

/// The top-level rules of a stylesheet text, each as its own text. Enough
/// for `cssRules[i].cssText` and `insertRule`/`deleteRule` bookkeeping.
fn split_rules(css: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if let Some(q) = quote {
            if c == '\\' { if let Some(n) = chars.next() { cur.push(n); } }
            else if c == q { quote = None; }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '/' if chars.peek() == Some(&'*') => {
                cur.pop();
                chars.next();
                let mut prev = ' ';
                for x in chars.by_ref() { if prev == '*' && x == '/' { break } prev = x; }
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth <= 0 {
                    depth = 0;
                    let t = cur.trim();
                    if !t.is_empty() { out.push(String::from(t)); }
                    cur.clear();
                }
            }
            ';' if depth == 0 => {
                let t = cur.trim();
                if t.len() > 1 { out.push(String::from(t)); }
                cur.clear();
            }
            _ => {}
        }
    }
    out
}

fn sheet_rules(i: &mut Interp, t: &Value) -> Vec<String> {
    let v = hslot(i, t, SS_RULES);
    let Value::Obj(o) = v else { return Vec::new() };
    let n = o.borrow().own_keys().len();
    let mut out = Vec::new();
    for k in 0..n {
        if let Some(Value::Str(s)) = o.borrow().get_own(&alloc::format!("{k}")).and_then(|p| p.value.clone()) {
            out.push(s.to_string());
        }
    }
    out
}

fn set_sheet_rules(i: &mut Interp, t: &Value, rules: Vec<String>) {
    let arr = i.new_array(rules.into_iter().map(Value::string).collect());
    if let Value::Obj(o) = t { o.borrow_mut().define(SS_RULES, hide(arr)); }
}

fn is_sheet(v: &Value) -> bool {
    matches!(v, Value::Obj(o) if o.borrow().get_own(SS_RULES).is_some())
}

/// Elements that may host a shadow root (DOM §4.9 attachShadow step 2).
fn can_host(tag: &str) -> bool {
    crate::dom::valid_custom_element_name(tag) || matches!(tag, "article" | "aside" | "blockquote"
        | "body" | "div" | "footer" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "main"
        | "nav" | "p" | "section" | "span")
}

/// Create the shadow root of `host` (DOM §4.9 "attach a shadow root").
pub(super) fn attach_shadow_root(d: &mut Doc, host: u32, mode: &str, delegates: bool) -> u32 {
    let root = d.create(ELEMENT_NODE, SHADOW_TAG);
    d.nodes[root as usize].attrs.push((Rc::from("mode"), Rc::from(mode)));
    if delegates { d.nodes[root as usize].attrs.push((Rc::from("delegatesfocus"), Rc::from(""))); }
    d.nodes[root as usize].shadow = Some(host);
    d.nodes[host as usize].shadow = Some(root);
    root
}

fn shadow_of(i: &Interp, id: u32) -> Option<u32> {
    let d = i.doc.as_ref()?;
    let n = &d.nodes[id as usize];
    if &*n.tag == SHADOW_TAG { return None }
    n.shadow
}

fn popover_kind(i: &Interp, id: u32) -> Option<String> {
    let v = i.doc.as_ref()?.nodes[id as usize].attr("popover")?.to_ascii_lowercase();
    Some(match v.as_str() { "" | "auto" => String::from("auto"), "hint" => String::from("hint"),
                            _ => String::from("manual") })
}

/// A `ToggleEvent` (HTML §6.12.1), fired synchronously for `beforetoggle`.
fn toggle_event(i: &mut Interp, kind: &str, old: &str, new: &str, cancelable: bool) -> Gc {
    let proto = i.realm.event_proto.clone();
    let ev = build_event(i, proto, kind, true);
    set_ev(&ev, EV_CANCELABLE, Value::Bool(cancelable));
    ev.borrow_mut().define("oldState", Prop::data(Value::str(old)));
    ev.borrow_mut().define("newState", Prop::data(Value::str(new)));
    ev
}

/// Fire `toggle` as a task, as HTML §6.12.2 queues it.
fn queue_toggle(i: &mut Interp, id: u32, old: &str, new: &str) -> C<()> {
    let el = wrap(i, id);
    let rec = i.new_array(alloc::vec![el, Value::str(old), Value::str(new)]);
    let f = super::super::promise::bind1(i, |i, _, a| {
        let rec = a.first().cloned().unwrap_or(Value::Undefined);
        let el = i.get(&rec, "0")?;
        let old = i.get(&rec, "1")?;
        let new = i.get(&rec, "2")?;
        let (old, new) = (i.to_string(&old)?, i.to_string(&new)?);
        let Ok(id) = node_of(i, &el) else { return Ok(Value::Undefined) };
        let ev = toggle_event(i, "toggle", &old, &new, false);
        deliver(i, &ev, "toggle", &[id])?;
        Ok(Value::Undefined)
    }, rec);
    queue_task(i, f)
}

/// Run `f` as a task: through the page's own timer queue, at delay zero.
fn queue_task(i: &mut Interp, f: Value) -> C<()> {
    let st = i.get(&Value::Obj(i.realm.global.clone()), "setTimeout")?;
    i.call(&st, Value::Undefined, &[f, Value::Num(0.0)])?;
    Ok(())
}

fn set_ui(i: &mut Interp, id: u32, bit: u8, on: bool) {
    if let Some(d) = &mut i.doc {
        let n = &mut d.nodes[id as usize];
        if on { n.ui |= bit } else { n.ui &= !bit }
        d.touch();
    }
}

fn ui_has(i: &Interp, id: u32, bit: u8) -> bool {
    i.doc.as_ref().is_some_and(|d| d.nodes[id as usize].ui & bit != 0)
}

/// HTML §6.12.2 "show popover".
fn show_popover(i: &mut Interp, id: u32) -> C<()> {
    if popover_kind(i, id).is_none() {
        return Err(dom_exc(i, "NotSupportedError", "the element has no popover attribute"));
    }
    if ui_has(i, id, UI_POPOVER_OPEN) { return Ok(()) }
    if !i.doc.as_ref().is_some_and(|d| d.connected(id)) {
        return Err(dom_exc(i, "InvalidStateError", "the popover is not connected"));
    }
    let ev = toggle_event(i, "beforetoggle", "closed", "open", true);
    if deliver(i, &ev, "beforetoggle", &[id])? { return Ok(()) }
    set_ui(i, id, UI_POPOVER_OPEN, true);
    queue_toggle(i, id, "closed", "open")
}

/// HTML §6.12.2 "hide popover".
fn hide_popover(i: &mut Interp, id: u32) -> C<()> {
    if popover_kind(i, id).is_none() {
        return Err(dom_exc(i, "NotSupportedError", "the element has no popover attribute"));
    }
    if !ui_has(i, id, UI_POPOVER_OPEN) { return Ok(()) }
    let ev = toggle_event(i, "beforetoggle", "open", "closed", false);
    deliver(i, &ev, "beforetoggle", &[id])?;
    set_ui(i, id, UI_POPOVER_OPEN, false);
    queue_toggle(i, id, "open", "closed")
}

/// HTML §4.11.4 "close the dialog".
fn close_dialog(i: &mut Interp, id: u32, result: Option<Value>) -> C<()> {
    let open = i.doc.as_ref().is_some_and(|d| d.nodes[id as usize].attr("open").is_some());
    if !open { return Ok(()) }
    if let Some(d) = &mut i.doc { d.remove_attr_at(id, "open"); }
    set_ui(i, id, UI_MODAL, false);
    let el = wrap(i, id);
    if let (Some(r), Value::Obj(o)) = (result, &el) { o.borrow_mut().define(DLG_RETURN, hide(r)); }
    let f = super::super::promise::bind1(i, |i, _, a| {
        let el = a.first().cloned().unwrap_or(Value::Undefined);
        fire_simple(i, &el, "close", false)?;
        Ok(Value::Undefined)
    }, el);
    queue_task(i, f)
}

/// Is the element rendered? Answered from the boxes of the last layout,
/// and from `hidden`/`display:none` up the tree when there is none yet.
fn rendered(i: &mut Interp, t: &Value, id: u32) -> bool {
    if !i.doc.as_ref().is_some_and(|d| d.connected(id)) { return false }
    ensure_box(i, t);
    if i.geometry.is_some() { return elem_rect(i, t).is_some() }
    let Some(d) = &i.doc else { return false };
    let mut cur = Some(id);
    while let Some(x) = cur {
        let n = &d.nodes[x as usize];
        if n.attr("hidden").is_some() { return false }
        if n.attr("style").is_some_and(|s| style_decls(s).iter()
            .any(|(k, v)| k == "display" && v.trim().eq_ignore_ascii_case("none"))) { return false }
        cur = n.parent;
    }
    true
}

/// The internals' element, if it is form-associated.
fn internals_target(i: &mut Interp, t: &Value) -> C<u32> {
    let el = hslot(i, t, IN_EL);
    node_of(i, &el)
}

pub(super) fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let g = realm.global.clone();
    let proto_of = |name: &str| -> Option<Gc> {
        let c = g.borrow().get_own(name).and_then(|p| p.value.clone())?;
        let Value::Obj(c) = c else { return None };
        let p = c.borrow().get_own("prototype").and_then(|p| p.value.clone())?;
        match p { Value::Obj(p) => Some(p), _ => None }
    };

    // ── Iteration on the list types ──────────────────────────────────────
    if let Some(tl) = proto_of("DOMTokenList") {
        for (name, part) in [("values", 0u8), ("keys", 1), ("entries", 2)] {
            let f: NativeFn = match part {
                0 => |i, t, _| token_iter(i, &t, 0),
                1 => |i, t, _| token_iter(i, &t, 1),
                _ => |i, t, _| token_iter(i, &t, 2),
            };
            meth(&tl, name, f, 0, &fp);
        }
        meth(&tl, SYM_ITERATOR, |i, t, _| token_iter(i, &t, 0), 0, &fp);
        // `classList` has no supported tokens (DOM §7.1).
        meth(&tl, "supports", |i, _, _| i.type_err("DOMTokenList has no supported tokens"), 1, &fp);
    }
    if let Some(nnm) = proto_of("NamedNodeMap") {
        meth(&nnm, SYM_ITERATOR, |i, t, _| {
            let n = match i.get(&t, "length")? { Value::Num(n) => n as usize, _ => 0 };
            let mut v = Vec::new();
            for k in 0..n { v.push(i.get(&t, &alloc::format!("{k}"))?); }
            iter_of(i, v)
        }, 0, &fp);
    }

    // ── Document ─────────────────────────────────────────────────────────
    let dp = realm.document_proto.clone();
    meth(&dp, "getElementsByName", |i, t, a| {
        let id = node_of(i, &t)?;
        let name = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut all = Vec::new();
        if let Some(d) = &i.doc { d.descendants(id, &mut all); }
        let hits: Vec<u32> = match &i.doc {
            Some(d) => all.into_iter().filter(|&x| d.nodes[x as usize].attr("name")
                .is_some_and(|v| **v == *name)).collect(),
            None => Vec::new(),
        };
        Ok(nodes_array(i, hits))
    }, 1, &fp);
    for target in [dp.clone()].into_iter().chain(proto_of("ShadowRoot")) {
        accessor(&target, "adoptedStyleSheets",
            |i, t, _| {
                let v = hslot(i, &t, ADOPTED);
                if matches!(v, Value::Obj(_)) { return Ok(v) }
                let arr = i.new_array(Vec::new());
                if let Value::Obj(o) = &t { o.borrow_mut().define(ADOPTED, hide(arr.clone())); }
                Ok(arr)
            },
            |i, t, a| {
                let v = a.first().cloned().unwrap_or(Value::Undefined);
                let items = i.iterate(&v)?;
                if !items.iter().all(is_sheet) {
                    return i.type_err("adoptedStyleSheets: every entry must be a constructed CSSStyleSheet");
                }
                let arr = i.new_array(items);
                if let Value::Obj(o) = &t { o.borrow_mut().define(ADOPTED, hide(arr)); }
                Ok(Value::Undefined)
            }, &fp);
    }

    // ── DocumentFragment, constructible (DOM §4.7) ───────────────────────
    let frag = realm.fragment_proto.clone();
    let fc = native(Some(fp.clone()), |i, this, _| {
        if !i.native_new { return i.type_err("Constructor DocumentFragment requires 'new'") }
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, "#fragment");
        let v = wrap(i, id);
        if let (Value::Obj(o), Value::Obj(t)) = (&v, &this) {
            if let Some(p) = t.borrow().proto.clone() { o.borrow_mut().proto = Some(p); }
        }
        Ok(v)
    }, "DocumentFragment", 0, true);
    fc.borrow_mut().define("prototype", Prop::frozen(Value::Obj(frag.clone())));
    frag.borrow_mut().define("constructor", Prop::builtin(Value::Obj(fc.clone())));
    g.borrow_mut().define("DocumentFragment", Prop::builtin(Value::Obj(fc)));

    // ── CSSStyleSheet, constructible (CSSOM §6.1.2) ──────────────────────
    let ssp = new_obj(Some(realm.object_proto.clone()));
    let ssc = native(Some(fp.clone()), |i, this, a| {
        if !i.native_new { return i.type_err("Constructor CSSStyleSheet requires 'new'") }
        let proto = ctor_proto(i, &this, "CSSStyleSheet");
        let o = new_obj(Some(proto));
        let v = Value::Obj(o.clone());
        set_sheet_rules(i, &v, Vec::new());
        let disabled = match a.first() { Some(opt @ Value::Obj(_)) => i.get(opt, "disabled")?.truthy(), _ => false };
        o.borrow_mut().define(SS_DISABLED, hide(Value::Bool(disabled)));
        Ok(v)
    }, "CSSStyleSheet", 0, true);
    ssc.borrow_mut().define("prototype", Prop::frozen(Value::Obj(ssp.clone())));
    ssp.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ssc.clone())));
    ssp.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("CSSStyleSheet")));
    g.borrow_mut().define("CSSStyleSheet", Prop::builtin(Value::Obj(ssc)));
    meth(&ssp, "replaceSync", |i, t, a| {
        let text = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        // `@import` is not allowed in a constructed sheet and is dropped.
        let rules = split_rules(&text).into_iter()
            .filter(|r| !r.trim_start().to_ascii_lowercase().starts_with("@import")).collect();
        set_sheet_rules(i, &t, rules);
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&ssp, "replace", |i, t, a| {
        let text = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let rules = split_rules(&text).into_iter()
            .filter(|r| !r.trim_start().to_ascii_lowercase().starts_with("@import")).collect();
        set_sheet_rules(i, &t, rules);
        let p = super::super::promise::new_promise(i);
        super::super::promise::resolve_promise(i, &p, t.clone());
        Ok(Value::Obj(p))
    }, 1, &fp);
    for name in ["cssRules", "rules"] {
        getter(&ssp, name, |i, t, _| {
            let rules = sheet_rules(i, &t);
            let mut out = Vec::new();
            for r in rules {
                let o = new_obj(Some(i.realm.object_proto.clone()));
                o.borrow_mut().define("cssText", Prop::data(Value::string(r)));
                out.push(Value::Obj(o));
            }
            Ok(i.new_array(out))
        }, &fp);
    }
    meth(&ssp, "insertRule", |i, t, a| {
        let rule = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut rules = sheet_rules(i, &t);
        let at = match a.get(1) { Some(v) => i.to_number(v)? as usize, None => 0 };
        if at > rules.len() { return Err(dom_exc(i, "IndexSizeError", "insertRule: index out of range")) }
        let parsed = split_rules(&rule);
        if parsed.len() != 1 { return Err(dom_exc(i, "SyntaxError", "insertRule: not exactly one rule")) }
        rules.insert(at, parsed.into_iter().next().unwrap_or_default());
        set_sheet_rules(i, &t, rules);
        Ok(Value::Num(at as f64))
    }, 1, &fp);
    meth(&ssp, "deleteRule", |i, t, a| {
        let mut rules = sheet_rules(i, &t);
        let at = i.to_number(a.first().unwrap_or(&Value::Undefined))? as usize;
        if at >= rules.len() { return Err(dom_exc(i, "IndexSizeError", "deleteRule: index out of range")) }
        rules.remove(at);
        set_sheet_rules(i, &t, rules);
        Ok(Value::Undefined)
    }, 1, &fp);
    accessor(&ssp, "disabled",
        |i, t, _| Ok(Value::Bool(hslot(i, &t, SS_DISABLED).truthy())),
        |_, t, a| {
            let on = a.first().is_some_and(|v| v.truthy());
            if let Value::Obj(o) = &t { o.borrow_mut().define(SS_DISABLED, hide(Value::Bool(on))); }
            Ok(Value::Undefined)
        }, &fp);
    for name in ["href", "ownerNode", "ownerRule", "parentStyleSheet", "title"] {
        getter(&ssp, name, |_, _, _| Ok(Value::Null), &fp);
    }
    getter(&ssp, "type", |_, _, _| Ok(Value::str("text/css")), &fp);

    // ── Shadow roots (DOM §4.8) ──────────────────────────────────────────
    let ep = realm.element_proto.clone();
    meth(&ep, "attachShadow", |i, t, a| {
        let id = node_of(i, &t)?;
        let opt = a.first().cloned().unwrap_or(Value::Undefined);
        let mode = match &opt { Value::Obj(_) => i.get(&opt, "mode")?, _ => Value::Undefined };
        let mode = i.to_string(&mode)?;
        if &*mode != "open" && &*mode != "closed" {
            return i.type_err("attachShadow: mode must be 'open' or 'closed'");
        }
        let delegates = match &opt { Value::Obj(_) => i.get(&opt, "delegatesFocus")?.truthy(), _ => false };
        let tag = i.doc.as_ref().map(|d| d.nodes[id as usize].tag.clone()).unwrap_or_else(|| Rc::from(""));
        if !can_host(&tag) {
            return Err(dom_exc(i, "NotSupportedError", &alloc::format!("<{tag}> cannot host a shadow root")));
        }
        if shadow_of(i, id).is_some() {
            return Err(dom_exc(i, "NotSupportedError", "the element already hosts a shadow root"));
        }
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let root = attach_shadow_root(d, id, &mode, delegates);
        Ok(wrap(i, root))
    }, 1, &fp);
    getter(&ep, "shadowRoot", |i, t, _| {
        let id = node_of(i, &t)?;
        let Some(root) = shadow_of(i, id) else { return Ok(Value::Null) };
        let open = i.doc.as_ref().is_some_and(|d| d.nodes[root as usize].attr("mode").is_some_and(|m| &**m == "open"));
        Ok(if open { wrap(i, root) } else { Value::Null })
    }, &fp);
    if let Some(sr) = proto_of("ShadowRoot") {
        getter(&sr, "mode", |i, t, _| {
            with_node!(i, t, |n| Ok(n.attr("mode").map(|m| Value::Str(m.clone())).unwrap_or(Value::str("open"))))
        }, &fp);
        getter(&sr, "host", |i, t, _| {
            let id = node_of(i, &t)?;
            let h = i.doc.as_ref().and_then(|d| d.nodes[id as usize].shadow);
            Ok(match h { Some(h) => wrap(i, h), None => Value::Null })
        }, &fp);
        getter(&sr, "delegatesFocus", |i, t, _| with_node!(i, t, |n| Ok(Value::Bool(n.attr("delegatesfocus").is_some()))), &fp);
        getter(&sr, "slotAssignment", |_, _, _| Ok(Value::str("named")), &fp);
        getter(&sr, "clonable", |_, _, _| Ok(Value::Bool(false)), &fp);
        getter(&sr, "serializable", |_, _, _| Ok(Value::Bool(false)), &fp);
        getter(&sr, "activeElement", |_, _, _| Ok(Value::Null), &fp);
        accessor(&sr, "innerHTML",
            |i, t, _| {
                let id = node_of(i, &t)?;
                Ok(Value::string(i.doc.as_ref().map(|d| d.serialize(id, true)).unwrap_or_default()))
            },
            |i, t, a| {
                let id = node_of(i, &t)?;
                let html = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                if let Some(d) = &mut i.doc {
                    d.clear_children(id);
                    d.parse_into(id, &html, None);
                }
                fire_connected(i, id)?;
                Ok(Value::Undefined)
            }, &fp);
    }

    // ── ElementInternals (HTML §4.13.7) ──────────────────────────────────
    let ip = new_obj(Some(realm.object_proto.clone()));
    let ic = native(Some(fp.clone()), |i, _, _| i.type_err("Illegal constructor"), "ElementInternals", 0, true);
    ic.borrow_mut().define("prototype", Prop::frozen(Value::Obj(ip.clone())));
    ip.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ic.clone())));
    ip.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("ElementInternals")));
    g.borrow_mut().define("ElementInternals", Prop::builtin(Value::Obj(ic)));
    let hep = realm.html_element_proto.clone();
    meth(&hep, "attachInternals", |i, t, _| {
        let id = node_of(i, &t)?;
        let tag = match &i.doc { Some(d) => d.nodes[id as usize].tag.clone(), None => Rc::from("") };
        if !crate::dom::valid_custom_element_name(&tag) || i.custom.by_name(&tag).is_none() {
            return Err(dom_exc(i, "NotSupportedError", "attachInternals: not a custom element"));
        }
        if matches!(hslot(i, &t, INTERNALS), Value::Obj(_)) {
            return Err(dom_exc(i, "NotSupportedError", "attachInternals: already attached"));
        }
        let proto = match i.get(&Value::Obj(i.realm.global.clone()), "ElementInternals")? {
            c @ Value::Obj(_) => match i.get(&c, "prototype")? { Value::Obj(p) => p, _ => i.realm.object_proto.clone() },
            _ => i.realm.object_proto.clone(),
        };
        let o = new_obj(Some(proto));
        o.borrow_mut().define(IN_EL, hide(t.clone()));
        o.borrow_mut().define(IN_VALUE, hide(Value::Null));
        o.borrow_mut().define(IN_VALIDITY, hide(Value::Undefined));
        o.borrow_mut().define(IN_MESSAGE, hide(Value::str("")));
        // `states` is a set view on the element's custom states.
        let set = new_obj(Some(i.realm.object_proto.clone()));
        set.borrow_mut().define(IN_EL, hide(t.clone()));
        install_state_set(i, &set);
        o.borrow_mut().define("states", Prop::builtin(Value::Obj(set)));
        if let Value::Obj(e) = &t { e.borrow_mut().define(INTERNALS, hide(Value::Obj(o.clone()))); }
        Ok(Value::Obj(o))
    }, 0, &fp);
    getter(&ip, "shadowRoot", |i, t, _| {
        let id = internals_target(i, &t)?;
        Ok(match shadow_of(i, id) { Some(r) => wrap(i, r), None => Value::Null })
    }, &fp);
    getter(&ip, "form", |i, t, _| {
        let id = internals_target(i, &t)?;
        Ok(match owning_form(i, id) { Some(f) => wrap(i, f), None => Value::Null })
    }, &fp);
    getter(&ip, "labels", |i, t, _| {
        let id = internals_target(i, &t)?;
        let l = labels_of(i, id);
        Ok(nodes_array(i, l))
    }, &fp);
    meth(&ip, "setFormValue", |_, t, a| {
        let v = a.first().cloned().unwrap_or(Value::Null);
        if let Value::Obj(o) = &t { o.borrow_mut().define(IN_VALUE, hide(v)); }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&ip, "setValidity", |i, t, a| {
        let flags = a.first().cloned().unwrap_or(Value::Undefined);
        let msg = match a.get(1) { Some(v) => i.to_string(v)?, None => Rc::from("") };
        let mut invalid = false;
        if let Value::Obj(f) = &flags {
            let keys = f.borrow().own_keys();
            for k in keys { if i.get(&flags, &k)?.truthy() { invalid = true; } }
        }
        if invalid && msg.is_empty() {
            return i.type_err("setValidity: a message is required when a flag is set");
        }
        if let Value::Obj(o) = &t {
            o.borrow_mut().define(IN_VALIDITY, hide(if invalid { flags } else { Value::Undefined }));
            o.borrow_mut().define(IN_MESSAGE, hide(Value::Str(if invalid { msg } else { Rc::from("") })));
        }
        Ok(Value::Undefined)
    }, 2, &fp);
    getter(&ip, "validity", |i, t, _| {
        let flags = hslot(i, &t, IN_VALIDITY);
        let o = new_obj(Some(i.realm.object_proto.clone()));
        let mut any = false;
        for k in ["valueMissing", "typeMismatch", "patternMismatch", "tooLong", "tooShort",
                  "rangeUnderflow", "rangeOverflow", "stepMismatch", "badInput", "customError"] {
            let on = matches!(flags, Value::Obj(_)) && i.get(&flags, k)?.truthy();
            any |= on;
            o.borrow_mut().define(k, Prop::data(Value::Bool(on)));
        }
        o.borrow_mut().define("valid", Prop::data(Value::Bool(!any)));
        Ok(Value::Obj(o))
    }, &fp);
    getter(&ip, "validationMessage", |i, t, _| Ok(hslot(i, &t, IN_MESSAGE)), &fp);
    getter(&ip, "willValidate", |_, _, _| Ok(Value::Bool(true)), &fp);
    for name in ["checkValidity", "reportValidity"] {
        meth(&ip, name, |i, t, _| {
            let valid = !matches!(hslot(i, &t, IN_VALIDITY), Value::Obj(_));
            if !valid {
                let el = hslot(i, &t, IN_EL);
                fire_simple(i, &el, "invalid", true)?;
            }
            Ok(Value::Bool(valid))
        }, 0, &fp);
    }

    // ── Popover (HTML §6.12) ─────────────────────────────────────────────
    accessor(&hep, "popover",
        |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(match popover_kind(i, id) { Some(k) => Value::string(k), None => Value::Null })
        },
        |i, t, a| {
            let id = node_of(i, &t)?;
            match a.first() {
                None | Some(Value::Null) => { if let Some(d) = &mut i.doc { d.remove_attr_at(id, "popover"); } }
                Some(v) => {
                    let s = i.to_string(v)?;
                    if let Some(d) = &mut i.doc { d.set_attr_at(id, "popover", &s); }
                }
            }
            Ok(Value::Undefined)
        }, &fp);
    meth(&hep, "showPopover", |i, t, _| { let id = node_of(i, &t)?; show_popover(i, id)?; Ok(Value::Undefined) }, 0, &fp);
    meth(&hep, "hidePopover", |i, t, _| { let id = node_of(i, &t)?; hide_popover(i, id)?; Ok(Value::Undefined) }, 0, &fp);
    meth(&hep, "togglePopover", |i, t, a| {
        let id = node_of(i, &t)?;
        let force = match a.first() {
            Some(Value::Bool(b)) => Some(*b),
            Some(o @ Value::Obj(_)) => match i.get(o, "force")? { Value::Undefined => None, v => Some(v.truthy()) },
            Some(Value::Undefined) | None => None,
            Some(v) => Some(v.truthy()),
        };
        let open = ui_has(i, id, UI_POPOVER_OPEN);
        let want = force.unwrap_or(!open);
        if want && !open { show_popover(i, id)?; }
        if !want && open { hide_popover(i, id)?; }
        Ok(Value::Bool(ui_has(i, id, UI_POPOVER_OPEN)))
    }, 0, &fp);

    // ── checkVisibility (CSSOM View §6) ──────────────────────────────────
    meth(&ep, "checkVisibility", |i, t, a| {
        let id = node_of(i, &t)?;
        if !rendered(i, &t, id) { return Ok(Value::Bool(false)) }
        let opt = a.first().cloned().unwrap_or(Value::Undefined);
        let want = |i: &mut Interp, k: &str| -> C<bool> {
            Ok(matches!(opt, Value::Obj(_)) && i.get(&opt, k)?.truthy())
        };
        if want(i, "checkOpacity")? || want(i, "opacityProperty")? {
            let gcs = i.get(&Value::Obj(i.realm.global.clone()), "getComputedStyle")?;
            let st = i.call(&gcs, Value::Undefined, &[t.clone()])?;
            let op = i.get(&st, "opacity")?;
            if i.to_string(&op)?.trim() == "0" { return Ok(Value::Bool(false)) }
        }
        if want(i, "checkVisibilityCSS")? || want(i, "visibilityProperty")? {
            let gcs = i.get(&Value::Obj(i.realm.global.clone()), "getComputedStyle")?;
            let st = i.call(&gcs, Value::Undefined, &[t.clone()])?;
            let v = i.get(&st, "visibility")?;
            let v = i.to_string(&v)?;
            if &*v == "hidden" || &*v == "collapse" { return Ok(Value::Bool(false)) }
        }
        Ok(Value::Bool(true))
    }, 0, &fp);

    // ── <dialog> (HTML §4.11.4) and <details> `open` ─────────────────────
    for tag in ["dialog", "details"] {
        if let Some(p) = realm.tag_protos.get(tag) {
            if p.borrow().get_own("open").is_none() { bool_attr_prop!(p, fp, "open", "open"); }
        }
    }
    if let Some(dlg) = realm.tag_protos.get("dialog").cloned() {
        meth(&dlg, "show", |i, t, _| {
            let id = node_of(i, &t)?;
            let open = i.doc.as_ref().is_some_and(|d| d.nodes[id as usize].attr("open").is_some());
            if open {
                if ui_has(i, id, UI_MODAL) {
                    return Err(dom_exc(i, "InvalidStateError", "the dialog is already open as modal"));
                }
                return Ok(Value::Undefined);
            }
            if let Some(d) = &mut i.doc { d.set_attr_at(id, "open", ""); }
            Ok(Value::Undefined)
        }, 0, &fp);
        meth(&dlg, "showModal", |i, t, _| {
            let id = node_of(i, &t)?;
            let open = i.doc.as_ref().is_some_and(|d| d.nodes[id as usize].attr("open").is_some());
            if open {
                if ui_has(i, id, UI_MODAL) { return Ok(Value::Undefined) }
                return Err(dom_exc(i, "InvalidStateError", "the dialog is already open"));
            }
            if !i.doc.as_ref().is_some_and(|d| d.connected(id)) {
                return Err(dom_exc(i, "InvalidStateError", "the dialog is not connected"));
            }
            if let Some(d) = &mut i.doc { d.set_attr_at(id, "open", ""); }
            set_ui(i, id, UI_MODAL, true);
            Ok(Value::Undefined)
        }, 0, &fp);
        meth(&dlg, "close", |i, t, a| {
            let id = node_of(i, &t)?;
            let r = match a.first() { Some(Value::Undefined) | None => None, Some(v) => Some(Value::Str(i.to_string(v)?)) };
            close_dialog(i, id, r)?;
            Ok(Value::Undefined)
        }, 0, &fp);
        meth(&dlg, "requestClose", |i, t, a| {
            let id = node_of(i, &t)?;
            let open = i.doc.as_ref().is_some_and(|d| d.nodes[id as usize].attr("open").is_some());
            if !open { return Ok(Value::Undefined) }
            if fire_simple(i, &t, "cancel", true)? { return Ok(Value::Undefined) }
            let r = match a.first() { Some(Value::Undefined) | None => None, Some(v) => Some(Value::Str(i.to_string(v)?)) };
            close_dialog(i, id, r)?;
            Ok(Value::Undefined)
        }, 0, &fp);
        accessor(&dlg, "returnValue",
            |i, t, _| Ok(match hslot(i, &t, DLG_RETURN) { Value::Undefined => Value::str(""), v => v }),
            |i, t, a| {
                let v = Value::Str(i.to_string(a.first().unwrap_or(&Value::Undefined))?);
                if let Value::Obj(o) = &t { o.borrow_mut().define(DLG_RETURN, hide(v)); }
                Ok(Value::Undefined)
            }, &fp);
    }

    // ── PerformanceObserver (Performance Timeline §3) ────────────────────
    //
    // beak records no performance entries, so the supported types are none
    // and an observer never fires; that is the true answer, not a stub.
    let pop = new_obj(Some(realm.object_proto.clone()));
    let poc = native(Some(fp.clone()), |i, this, a| {
        if !i.native_new { return i.type_err("Constructor PerformanceObserver requires 'new'") }
        let cb = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&cb) { return i.type_err("PerformanceObserver: the callback is not a function") }
        let proto = ctor_proto(i, &this, "PerformanceObserver");
        Ok(Value::Obj(new_obj(Some(proto))))
    }, "PerformanceObserver", 1, true);
    poc.borrow_mut().define("prototype", Prop::frozen(Value::Obj(pop.clone())));
    pop.borrow_mut().define("constructor", Prop::builtin(Value::Obj(poc.clone())));
    pop.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("PerformanceObserver")));
    meth(&pop, "observe", |i, _, a| {
        let opt = a.first().cloned().unwrap_or(Value::Undefined);
        let has_type = matches!(opt, Value::Obj(_)) && !matches!(i.get(&opt, "type")?, Value::Undefined);
        let has_list = matches!(opt, Value::Obj(_)) && !matches!(i.get(&opt, "entryTypes")?, Value::Undefined);
        if has_type == has_list {
            return i.type_err("PerformanceObserver.observe: give either type or entryTypes");
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&pop, "disconnect", |_, _, _| Ok(Value::Undefined), 0, &fp);
    meth(&pop, "takeRecords", |i, _, _| Ok(i.new_array(Vec::new())), 0, &fp);
    getter(&poc, "supportedEntryTypes", |i, _, _| Ok(i.new_array(Vec::new())), &fp);
    g.borrow_mut().define("PerformanceObserver", Prop::builtin(Value::Obj(poc)));
}

/// `classList` iteration: 0 values, 1 keys, 2 entries.
fn token_iter(i: &mut Interp, t: &Value, part: u8) -> C<Value> {
    let id = node_of(i, t)?;
    let cs = i.doc.as_ref().map(|d| d.classes(id)).unwrap_or_default();
    let mut v = Vec::new();
    for (k, c) in cs.into_iter().enumerate() {
        v.push(match part {
            0 => Value::Str(c),
            1 => Value::Num(k as f64),
            _ => i.new_array(alloc::vec![Value::Num(k as f64), Value::Str(c)]),
        });
    }
    iter_of(i, v)
}

/// `CustomStateSet` (HTML §4.13.7.5) on the element's `DomNode::states`.
fn install_state_set(i: &mut Interp, set: &Gc) {
    let fp = i.realm.function_proto.clone();
    fn el_of(i: &mut Interp, t: &Value) -> C<u32> {
        let el = hslot(i, t, IN_EL);
        node_of(i, &el)
    }
    fn states(i: &Interp, id: u32) -> Vec<Rc<str>> {
        i.doc.as_ref().map(|d| d.nodes[id as usize].states.clone()).unwrap_or_default()
    }
    meth(set, "add", |i, t, a| {
        let id = el_of(i, &t)?;
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc {
            if !d.nodes[id as usize].states.contains(&s) { d.nodes[id as usize].states.push(s); d.touch(); }
        }
        Ok(t)
    }, 1, &fp);
    meth(set, "delete", |i, t, a| {
        let id = el_of(i, &t)?;
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let Some(d) = &mut i.doc else { return Ok(Value::Bool(false)) };
        let had = d.nodes[id as usize].states.contains(&s);
        d.nodes[id as usize].states.retain(|x| *x != s);
        if had { d.touch(); }
        Ok(Value::Bool(had))
    }, 1, &fp);
    meth(set, "has", |i, t, a| {
        let id = el_of(i, &t)?;
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(states(i, id).contains(&s)))
    }, 1, &fp);
    meth(set, "clear", |i, t, _| {
        let id = el_of(i, &t)?;
        if let Some(d) = &mut i.doc { d.nodes[id as usize].states.clear(); d.touch(); }
        Ok(Value::Undefined)
    }, 0, &fp);
    getter(set, "size", |i, t, _| {
        let id = el_of(i, &t)?;
        Ok(Value::Num(states(i, id).len() as f64))
    }, &fp);
    for name in ["values", "keys"] {
        meth(set, name, |i, t, _| {
            let id = el_of(i, &t)?;
            let v = states(i, id).into_iter().map(Value::Str).collect();
            iter_of(i, v)
        }, 0, &fp);
    }
    meth(set, SYM_ITERATOR, |i, t, _| {
        let id = el_of(i, &t)?;
        let v = states(i, id).into_iter().map(Value::Str).collect();
        iter_of(i, v)
    }, 0, &fp);
    meth(set, "forEach", |i, t, a| {
        let id = el_of(i, &t)?;
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        for s in states(i, id) {
            i.call(&f, Value::Undefined, &[Value::Str(s.clone()), Value::Str(s), t.clone()])?;
        }
        Ok(Value::Undefined)
    }, 1, &fp);
}
