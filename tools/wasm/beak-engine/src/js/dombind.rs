//! Das DOM fuer JavaScript.
//!
//! **Wem gehoert der Baum.** `beak_engine::dom` haelt einen BESITZENDEN Baum
//! (`Element` haelt seine Kinder), und darin kann JavaScript keine Referenz
//! halten: ein Handle muesste einen Knoten ueberleben, den ein Nachbar
//! besitzt. Also wird der Baum in eine **Arena** geflacht — ein `Vec` von
//! Knoten, und ein Handle ist ein Index.
//!
//! Das ist eine ZWEITE Darstellung desselben Dokuments, und das ist eine
//! Schuld, keine Loesung: solange beaks Layout den alten Baum liest und JS die
//! Arena schreibt, wirkt eine DOM-Aenderung NICHT auf das Bild. Die Arena ist
//! aber genau die Form, zu der der alte Baum vereinheitlicht werden muss —
//! Indizes statt Besitz — und dieser Schritt macht sie erst messbar.
//!
//! **Knotenidentitaet ist beobachtbar.** `document.body === document.body`
//! muss wahr sein, also wird das JS-Huellobjekt je Knoten EINMAL gebaut und
//! behalten.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;

use super::interp::*;
use super::value::*;

/// Elemente ohne Schlusstag.
fn is_void(tag: &str) -> bool {
    matches!(tag, "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input"
        | "link" | "meta" | "source" | "track" | "wbr")
}

fn push_escaped(out: &mut String, s: &str, in_attr: bool) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if in_attr => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

pub const ELEMENT_NODE: f64 = 1.0;
pub const TEXT_NODE: f64 = 3.0;
pub const DOCUMENT_NODE: f64 = 9.0;

pub struct DomNode {
    pub kind: f64,
    pub tag: Rc<str>,
    pub attrs: Vec<(Rc<str>, Rc<str>)>,
    pub text: Rc<str>,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    /// Einmal gebaut, dann behalten — sonst waere `el === el` falsch.
    pub js: Option<Gc>,
    /// Angemeldete Behandler. Noch wird nichts zugestellt; sie hier zu HALTEN
    /// kostet nichts und ist die Stelle, an der die Zustellung ansetzt.
    pub listeners: Vec<(Rc<str>, Value)>,
}

impl DomNode {
    fn new(kind: f64, tag: &str) -> DomNode {
        DomNode { kind, tag: Rc::from(tag), attrs: Vec::new(), text: Rc::from(""),
                  parent: None, children: Vec::new(), js: None, listeners: Vec::new() }
    }
    pub fn attr(&self, k: &str) -> Option<&Rc<str>> {
        self.attrs.iter().find(|(n, _)| &**n == k).map(|(_, v)| v)
    }
    pub fn set_attr(&mut self, k: &str, v: &str) {
        match self.attrs.iter_mut().find(|(n, _)| &**n == k) {
            Some(slot) => slot.1 = Rc::from(v),
            None => self.attrs.push((Rc::from(k), Rc::from(v))),
        }
    }
}

pub struct Doc {
    pub nodes: Vec<DomNode>,
    pub doc: u32,
    pub html: Option<u32>,
    pub body: Option<u32>,
    pub head: Option<u32>,
}

impl Doc {
    pub fn empty() -> Doc {
        let mut nodes = Vec::new();
        nodes.push(DomNode::new(DOCUMENT_NODE, "#document"));
        Doc { nodes, doc: 0, html: None, body: None, head: None }
    }

    /// Aus beaks geparstem Baum. Der `seq` des Originals wird NICHT
    /// uebernommen — die Arena vergibt eigene Indizes, und die Zuordnung
    /// zurueck ist die Aufgabe des Schritts, der beide vereinheitlicht.
    pub fn from_dom(src: &crate::dom::Dom) -> Doc {
        let mut d = Doc::empty();
        let doc = d.doc;
        d.add_children(&src.root, doc);
        d.html = d.find_tag(d.doc, "html");
        d.body = d.find_tag(d.doc, "body");
        d.head = d.find_tag(d.doc, "head");
        d
    }

    fn add_children(&mut self, e: &crate::dom::Element, parent: u32) {
        for c in &e.children {
            match c {
                crate::dom::Node::Text(t) => {
                    let mut n = DomNode::new(TEXT_NODE, "#text");
                    n.text = Rc::from(t.as_str());
                    let id = self.push(n, parent);
                    let _ = id;
                }
                crate::dom::Node::Element(el) => {
                    let mut n = DomNode::new(ELEMENT_NODE, &el.tag);
                    for (k, v) in &el.attrs { n.attrs.push((Rc::from(k.as_str()), Rc::from(v.as_str()))); }
                    let id = self.push(n, parent);
                    self.add_children(el, id);
                }
            }
        }
    }

    pub fn push(&mut self, n: DomNode, parent: u32) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(n);
        self.nodes[id as usize].parent = Some(parent);
        self.nodes[parent as usize].children.push(id);
        id
    }

    /// Frei stehender Knoten, noch ohne Elternteil (`createElement`).
    pub fn create(&mut self, kind: f64, tag: &str) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(DomNode::new(kind, tag));
        id
    }

    fn find_tag(&self, from: u32, tag: &str) -> Option<u32> {
        for &c in &self.nodes[from as usize].children {
            if &*self.nodes[c as usize].tag == tag { return Some(c); }
            if let Some(f) = self.find_tag(c, tag) { return Some(f); }
        }
        None
    }

    /// Aus dem alten Elternteil aushaengen. Muss VOR jedem Einhaengen laufen,
    /// sonst steht ein Knoten in zwei Kinderlisten und der Baum ist keiner mehr.
    pub fn detach(&mut self, id: u32) {
        if let Some(p) = self.nodes[id as usize].parent {
            self.nodes[p as usize].children.retain(|&c| c != id);
        }
        self.nodes[id as usize].parent = None;
    }

    pub fn append(&mut self, parent: u32, child: u32) {
        self.detach(child);
        self.nodes[child as usize].parent = Some(parent);
        self.nodes[parent as usize].children.push(child);
    }

    pub fn insert_before(&mut self, parent: u32, child: u32, before: Option<u32>) {
        self.detach(child);
        self.nodes[child as usize].parent = Some(parent);
        let at = match before {
            Some(b) => self.nodes[parent as usize].children.iter().position(|&c| c == b)
                        .unwrap_or(self.nodes[parent as usize].children.len()),
            None => self.nodes[parent as usize].children.len(),
        };
        self.nodes[parent as usize].children.insert(at, child);
    }

    /// Der Text eines Teilbaums, aneinandergehaengt.
    pub fn text_of(&self, id: u32) -> String {
        let n = &self.nodes[id as usize];
        if n.kind == TEXT_NODE { return n.text.to_string(); }
        let mut s = String::new();
        for &c in &n.children { s.push_str(&self.text_of(c)); }
        s
    }

    pub fn classes(&self, id: u32) -> Vec<Rc<str>> {
        match self.nodes[id as usize].attr("class") {
            Some(c) => c.split_ascii_whitespace().map(Rc::from).collect(),
            None => Vec::new(),
        }
    }

    /// Alle Elemente in Dokumentreihenfolge ab `from`.
    pub fn descendants(&self, from: u32, out: &mut Vec<u32>) {
        for &c in &self.nodes[from as usize].children {
            if self.nodes[c as usize].kind == ELEMENT_NODE { out.push(c); }
            self.descendants(c, out);
        }
    }
}

impl Doc {
    /// Ein HTML-Bruchstueck parsen und unter `parent` einhaengen.
    ///
    /// Das ist der Weg von `innerHTML =` und `insertAdjacentHTML`. Es geht
    /// ueber beaks eigenen Parser — ein zweiter, laxerer waere eine zweite
    /// Wahrheit darueber, was das Web bedeutet.
    pub fn parse_into(&mut self, parent: u32, html: &str, at: Option<usize>) -> Vec<u32> {
        let frag = crate::dom::parse(html);
        let mut made = Vec::new();
        for c in &frag.root.children {
            if let Some(id) = self.from_src_node(c) { made.push(id); }
        }
        let idx = at.unwrap_or(self.nodes[parent as usize].children.len());
        for (k, id) in made.iter().enumerate() {
            self.nodes[*id as usize].parent = Some(parent);
            let pos = (idx + k).min(self.nodes[parent as usize].children.len());
            self.nodes[parent as usize].children.insert(pos, *id);
        }
        made
    }

    /// Einen geparsten Teilbaum in die Arena legen, noch ohne Elternteil.
    fn from_src_node(&mut self, n: &crate::dom::Node) -> Option<u32> {
        match n {
            crate::dom::Node::Text(t) => {
                let id = self.create(TEXT_NODE, "#text");
                self.nodes[id as usize].text = Rc::from(t.as_str());
                Some(id)
            }
            crate::dom::Node::Element(el) => {
                let id = self.create(ELEMENT_NODE, &el.tag);
                for (k, v) in &el.attrs {
                    self.nodes[id as usize].attrs.push((Rc::from(k.as_str()), Rc::from(v.as_str())));
                }
                for c in &el.children {
                    if let Some(cid) = self.from_src_node(c) {
                        self.nodes[cid as usize].parent = Some(id);
                        self.nodes[id as usize].children.push(cid);
                    }
                }
                Some(id)
            }
        }
    }

    /// Ein Knoten als HTML. Fuer `innerHTML`/`outerHTML` — und die
    /// Maskierung ist Pflicht, nicht Kosmetik: ein `<` im Text, das
    /// unmaskiert herauskommt, macht aus Inhalt Auszeichnung.
    pub fn serialize(&self, id: u32, inner_only: bool) -> String {
        let n = &self.nodes[id as usize];
        let mut s = String::new();
        if n.kind == TEXT_NODE { push_escaped(&mut s, &n.text, false); return s; }
        if !inner_only && n.kind == ELEMENT_NODE {
            s.push('<'); s.push_str(&n.tag);
            for (k, v) in &n.attrs {
                s.push(' '); s.push_str(k); s.push_str("=\"");
                push_escaped(&mut s, v, true);
                s.push('"');
            }
            s.push('>');
        }
        for &c in &n.children { s.push_str(&self.serialize(c, false)); }
        if !inner_only && n.kind == ELEMENT_NODE && !is_void(&n.tag) {
            s.push_str("</"); s.push_str(&n.tag); s.push('>');
        }
        s
    }

    /// Alle Kinder eines Knotens loesen (fuer `innerHTML =`).
    pub fn clear_children(&mut self, id: u32) {
        let old: Vec<u32> = self.nodes[id as usize].children.clone();
        for c in old { self.nodes[c as usize].parent = None; }
        self.nodes[id as usize].children.clear();
    }

    /// Einen Teilbaum kopieren (`cloneNode`).
    pub fn clone_node(&mut self, id: u32, deep: bool) -> u32 {
        let (kind, tag, attrs, text, kids) = {
            let n = &self.nodes[id as usize];
            (n.kind, n.tag.clone(), n.attrs.clone(), n.text.clone(), n.children.clone())
        };
        let new = self.create(kind, &tag);
        self.nodes[new as usize].attrs = attrs;
        self.nodes[new as usize].text = text;
        if deep {
            for c in kids {
                let cc = self.clone_node(c, true);
                self.nodes[cc as usize].parent = Some(new);
                self.nodes[new as usize].children.push(cc);
            }
        }
        new
    }

    /// Zurueck in beaks Baum — der Schritt, der eine DOM-Aenderung UEBERHAUPT
    /// ERST sichtbar macht.
    ///
    /// Solange nur die Arena veraendert wird, bleibt das Bild stehen: Layout,
    /// Kaskade und Formulare lesen `dom::Dom`. Statt das Layout auf die Arena
    /// umzubauen — was jede gemessene Zahl dieser Engine aufs Spiel setzen
    /// wuerde — wird hier zurueckgeschrieben. Der Preis ist ein voller
    /// Neuaufbau des Baums je Skriptlauf, nicht je Aenderung; gegen ein
    /// Layout von 150 ms faellt das nicht auf.
    ///
    /// `seq` wird NEU vergeben, in Dokumentreihenfolge — genau wie der Parser
    /// es tut. Damit stimmt die Identitaet, an der die Formularzustaende
    /// haengen, fuer alles, was der Skriptlauf nicht angefasst hat.
    pub fn to_dom(&self) -> crate::dom::Dom {
        let mut seq = 0u32;
        let mut root = crate::dom::Element::bare("#root".into(), seq);
        for &c in &self.nodes[self.doc as usize].children.clone() {
            if let Some(n) = self.to_node(c, &mut seq) { root.children.push(n); }
        }
        root.index_attrs();
        crate::dom::Dom { root }
    }

    fn to_node(&self, id: u32, seq: &mut u32) -> Option<crate::dom::Node> {
        let n = &self.nodes[id as usize];
        if n.kind == TEXT_NODE {
            return Some(crate::dom::Node::Text(n.text.to_string()));
        }
        if n.kind != ELEMENT_NODE { return None; }
        *seq += 1;
        let mut e = crate::dom::Element::bare(n.tag.to_string(), *seq);
        for (k, v) in &n.attrs { e.attrs.push((k.to_string(), v.to_string())); }
        // MUSS nach den Attributen laufen — sonst sind Klassen, id und der
        // Bloom-Filter leer und kein Selektor trifft mehr.
        e.index_attrs();
        for &c in &n.children {
            if let Some(x) = self.to_node(c, seq) { e.children.push(x); }
        }
        Some(crate::dom::Node::Element(e))
    }
}

/// Ein Skript der Seite: entweder sein Text oder die Adresse, unter der er
/// steht.
pub enum ScriptRef {
    Inline(String),
    External(String),
}

/// ALLE Skripte der Seite, in Dokumentreihenfolge — eingebettete wie externe.
///
/// Die Reihenfolge ist die des Quelltextes, und beak fuehrt sie auch so aus.
/// Das ist die Bedeutung von `defer` und NICHT die eines blockierenden
/// `<script>` mitten im Koerper: ein Browser wuerde ein klassisches Skript
/// ausfuehren, BEVOR er weiterparst, und `document.write` haengt davon ab.
/// beak hat das Dokument schon fertig, wenn es hier ankommt — also verhaelt
/// sich alles wie `defer`. Bewusst, und es deckt alles ausser `document.write`.
pub fn page_scripts(d: &Doc) -> Vec<ScriptRef> {
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    let mut out = Vec::new();
    for id in all {
        let n = &d.nodes[id as usize];
        if &*n.tag != "script" { continue; }
        if !script_type_is_js(n) { continue; }
        match n.attr("src") {
            Some(src) if !src.trim().is_empty() => out.push(ScriptRef::External(src.to_string())),
            _ => {
                let text = d.text_of(id);
                if !text.trim().is_empty() { out.push(ScriptRef::Inline(text)); }
            }
        }
    }
    out
}

/// Ein `type`, das nicht JavaScript meint (`application/json`,
/// `text/template`), ist Nutzlast und kein Programm.
fn script_type_is_js(n: &DomNode) -> bool {
    match n.attr("type") {
        None => true,
        Some(t) => {
            let t = t.to_ascii_lowercase();
            t.is_empty() || t.contains("javascript") || t == "module" || t == "text/ecmascript"
        }
    }
}

/// Die Inhalte aller `<script>`-Elemente OHNE `src`, in Dokumentreihenfolge.
///
/// Nur die eingebetteten: ein `src` muesste geholt werden, und die Reihenfolge
/// zwischen geholten und eingebetteten Skripten ist eine eigene Frage
/// (`defer`, `async`, und was ein `document.write` dazwischen anrichtet).
/// Bewusst der kleinere, ehrliche Anfang.
pub fn inline_scripts(d: &Doc) -> Vec<String> {
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    let mut out = Vec::new();
    for id in all {
        let n = &d.nodes[id as usize];
        if &*n.tag != "script" || n.attr("src").is_some() { continue; }
        if !script_type_is_js(n) { continue; }
        let text = d.text_of(id);
        if !text.trim().is_empty() { out.push(text); }
    }
    out
}

// ── Selektoren ──────────────────────────────────────────────────────────────
//
// Eine EIGENE, kleine Auswertung — nicht die aus `css.rs`. Die passt auf
// beaks `Element` und nicht auf die Arena, und sie hierher zu ziehen waere der
// zweite Umbau in einem Schritt. Abgedeckt ist, was echter Code fast immer
// benutzt: `tag`, `#id`, `.class`, `[attr]`, `[attr=wert]`, beliebig
// kombiniert, dazu Nachfahren (Leerzeichen), Kind (`>`) und Listen (`,`).
// Was fehlt, faellt als NICHT GETROFFEN auf, nicht als falscher Treffer.

fn matches_simple(d: &Doc, id: u32, sel: &str) -> bool {
    let n = &d.nodes[id as usize];
    if n.kind != ELEMENT_NODE { return false; }
    let mut rest = sel.trim();
    if rest == "*" { return true; }
    // Fuehrender Typselektor.
    let tag_end = rest.find(['.', '#', '[']).unwrap_or(rest.len());
    if tag_end > 0 {
        if !n.tag.eq_ignore_ascii_case(&rest[..tag_end]) { return false; }
        rest = &rest[tag_end..];
    }
    while !rest.is_empty() {
        let c = rest.as_bytes()[0];
        let end = rest[1..].find(['.', '#', '[']).map(|i| i + 1).unwrap_or(rest.len());
        let part = &rest[1..end];
        match c {
            b'#' => if n.attr("id").map(|v| &**v) != Some(part) { return false; },
            b'.' => if !d.classes(id).iter().any(|k| &**k == part) { return false; },
            b'[' => {
                let inner = part.trim_end_matches(']');
                let (k, v) = match inner.split_once('=') {
                    Some((k, v)) => (k, Some(v.trim_matches(['"', '\'']))),
                    None => (inner, None),
                };
                match (n.attr(k), v) {
                    (None, _) => return false,
                    (Some(av), Some(want)) if &**av != want => return false,
                    _ => {}
                }
            }
            _ => return false,
        }
        rest = &rest[end..];
    }
    true
}

/// Ein zusammengesetzter Selektor, von rechts nach links geprueft — so herum,
/// weil der rechte Teil den Kandidaten schon festlegt.
fn matches_compound(d: &Doc, id: u32, sel: &str) -> bool {
    let mut parts: Vec<(&str, char)> = Vec::new();
    let mut comb = ' ';
    for tok in sel.split_whitespace() {
        if tok == ">" { comb = '>'; continue; }
        if let Some(t) = tok.strip_prefix('>') {
            parts.push((t, '>'));
            comb = ' ';
            continue;
        }
        parts.push((tok, comb));
        comb = ' ';
    }
    if parts.is_empty() { return false; }
    let (last, _) = parts.pop().unwrap();
    if !matches_simple(d, id, last) { return false; }
    let mut cur = d.nodes[id as usize].parent;
    while let Some((p, c)) = parts.pop() {
        let mut found = None;
        while let Some(x) = cur {
            if matches_simple(d, x, p) { found = Some(x); break; }
            if c == '>' { return false; }
            cur = d.nodes[x as usize].parent;
        }
        match found { Some(x) => cur = d.nodes[x as usize].parent, None => return false }
    }
    true
}

pub fn selector_match(d: &Doc, id: u32, sel: &str) -> bool {
    sel.split(',').any(|s| { let s = s.trim(); !s.is_empty() && matches_compound(d, id, s) })
}

pub fn query(d: &Doc, from: u32, sel: &str, all: bool) -> Vec<u32> {
    let mut cands = Vec::new();
    d.descendants(from, &mut cands);
    let mut out = Vec::new();
    for c in cands {
        if selector_match(d, c, sel) {
            out.push(c);
            if !all { break; }
        }
    }
    out
}

// ── Die JS-Seite ────────────────────────────────────────────────────────────

use super::interp::C;

/// Der Index im Huellobjekt. Nicht aufzaehlbar und nicht konfigurierbar — ein
/// Skript, das ueber die Eigenschaften eines Elements laeuft, darf ihn nicht
/// sehen.
const SLOT: &str = "__node";

pub fn node_of(i: &mut Interp, v: &Value) -> C<u32> {
    match i.get(v, SLOT)? {
        Value::Num(n) if n >= 0.0 => Ok(n as u32),
        _ => i.type_err("not a DOM node"),
    }
}

/// Das Huellobjekt eines Knotens — einmal gebaut, dann behalten.
pub fn wrap(i: &mut Interp, id: u32) -> Value {
    if let Some(doc) = &i.doc {
        if let Some(js) = doc.nodes.get(id as usize).and_then(|n| n.js.clone()) {
            return Value::Obj(js);
        }
    }
    let kind = i.doc.as_ref().map(|d| d.nodes[id as usize].kind).unwrap_or(ELEMENT_NODE);
    let proto = match kind {
        DOCUMENT_NODE => i.realm.document_proto.clone(),
        TEXT_NODE => i.realm.text_proto.clone(),
        _ => i.realm.element_proto.clone(),
    };
    let g = new_obj(Some(proto));
    g.borrow_mut().define(SLOT, Prop {
        value: Some(Value::Num(id as f64)), get: None, set: None,
        writable: false, enumerable: false, configurable: false });
    if let Some(doc) = &mut i.doc { doc.nodes[id as usize].js = Some(g.clone()); }
    Value::Obj(g)
}

fn nodes_array(i: &mut Interp, ids: Vec<u32>) -> Value {
    let vals: Vec<Value> = ids.into_iter().map(|id| wrap(i, id)).collect();
    i.new_array(vals)
}

/// Lesender Zugriff auf einen Knoten, ohne die Ausleihe ueber einen Aufruf
/// hinweg zu halten — jede Abfrage kopiert, was sie braucht.
macro_rules! with_node {
    ($i:expr, $this:expr, |$n:ident| $body:expr) => {{
        let id = node_of($i, &$this)?;
        let Some(d) = &$i.doc else { return $i.type_err("no document") };
        let $n = &d.nodes[id as usize];
        $body
    }};
}

fn getter(o: &Gc, name: &str, f: NativeFn, fp: &Gc) {
    let g = native(Some(fp.clone()), f, name, 0, false);
    o.borrow_mut().define(name, Prop {
        value: None, get: Some(Value::Obj(g)), set: None,
        writable: false, enumerable: true, configurable: true });
}

fn accessor(o: &Gc, name: &str, get: NativeFn, set: NativeFn, fp: &Gc) {
    let g = native(Some(fp.clone()), get, name, 0, false);
    let s = native(Some(fp.clone()), set, name, 1, false);
    o.borrow_mut().define(name, Prop {
        value: None, get: Some(Value::Obj(g)), set: Some(Value::Obj(s)),
        writable: false, enumerable: true, configurable: true });
}

fn meth(o: &Gc, name: &str, f: NativeFn, len: usize, fp: &Gc) {
    let g = native(Some(fp.clone()), f, name, len, false);
    o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
}

/// Baut `Node`/`Element`/`Document`-Prototypen und das globale `document`.
pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let node_proto = new_obj(Some(realm.object_proto.clone()));
    let element_proto = new_obj(Some(node_proto.clone()));
    let text_proto = new_obj(Some(node_proto.clone()));
    let document_proto = new_obj(Some(node_proto.clone()));

    // ── Node ─────────────────────────────────────────────────────────────
    getter(&node_proto, "nodeType", |i, t, _| with_node!(i, t, |n| Ok(Value::Num(n.kind))), &fp);
    getter(&node_proto, "nodeName", |i, t, _| with_node!(i, t, |n| {
        let s = n.tag.to_uppercase();
        Ok(Value::string(if n.kind == ELEMENT_NODE { s } else { n.tag.to_string() }))
    }), &fp);
    getter(&node_proto, "parentNode", |i, t, _| {
        let id = node_of(i, &t)?;
        let p = i.doc.as_ref().and_then(|d| d.nodes[id as usize].parent);
        Ok(match p { Some(x) => wrap(i, x), None => Value::Null })
    }, &fp);
    getter(&node_proto, "parentElement", |i, t, _| {
        let id = node_of(i, &t)?;
        let p = i.doc.as_ref().and_then(|d| d.nodes[id as usize].parent)
            .filter(|&x| i.doc.as_ref().is_some_and(|d| d.nodes[x as usize].kind == ELEMENT_NODE));
        Ok(match p { Some(x) => wrap(i, x), None => Value::Null })
    }, &fp);
    getter(&node_proto, "childNodes", |i, t, _| {
        let id = node_of(i, &t)?;
        let cs = i.doc.as_ref().map(|d| d.nodes[id as usize].children.clone()).unwrap_or_default();
        Ok(nodes_array(i, cs))
    }, &fp);
    getter(&node_proto, "firstChild", |i, t, _| {
        let id = node_of(i, &t)?;
        let c = i.doc.as_ref().and_then(|d| d.nodes[id as usize].children.first().copied());
        Ok(match c { Some(x) => wrap(i, x), None => Value::Null })
    }, &fp);
    getter(&node_proto, "lastChild", |i, t, _| {
        let id = node_of(i, &t)?;
        let c = i.doc.as_ref().and_then(|d| d.nodes[id as usize].children.last().copied());
        Ok(match c { Some(x) => wrap(i, x), None => Value::Null })
    }, &fp);
    getter(&node_proto, "nextSibling", |i, t, _| Ok(sibling(i, &t, 1)?), &fp);
    getter(&node_proto, "previousSibling", |i, t, _| Ok(sibling(i, &t, -1)?), &fp);
    accessor(&node_proto, "textContent",
        |i, t, _| { let id = node_of(i, &t)?;
                    let s = i.doc.as_ref().map(|d| d.text_of(id)).unwrap_or_default();
                    Ok(Value::string(s)) },
        |i, t, a| {
            let id = node_of(i, &t)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let Some(d) = &mut i.doc else { return Ok(Value::Undefined) };
            // Alle Kinder weg, ein Textknoten hin. Die alten Knoten bleiben in
            // der Arena liegen — sie sind nur nicht mehr verhaengt. Freigeben
            // hiesse Indizes verschieben, und ein Handle darf nie verrutschen.
            let old: Vec<u32> = d.nodes[id as usize].children.clone();
            for c in old { d.nodes[c as usize].parent = None; }
            d.nodes[id as usize].children.clear();
            if !s.is_empty() {
                let tid = d.create(TEXT_NODE, "#text");
                d.nodes[tid as usize].text = s;
                d.append(id, tid);
            }
            Ok(Value::Undefined)
        }, &fp);
    meth(&node_proto, "appendChild", |i, t, a| {
        let p = node_of(i, &t)?;
        let c = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc { d.append(p, c); }
        Ok(a[0].clone())
    }, 1, &fp);
    meth(&node_proto, "insertBefore", |i, t, a| {
        let p = node_of(i, &t)?;
        let c = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let b = match a.get(1) { Some(Value::Obj(_)) => Some(node_of(i, &a[1])?), _ => None };
        if let Some(d) = &mut i.doc { d.insert_before(p, c, b); }
        Ok(a[0].clone())
    }, 2, &fp);
    meth(&node_proto, "removeChild", |i, t, a| {
        let _ = node_of(i, &t)?;
        let c = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc { d.detach(c); }
        Ok(a[0].clone())
    }, 1, &fp);
    meth(&node_proto, "contains", |i, t, a| {
        let p = node_of(i, &t)?;
        let Ok(c) = node_of(i, a.first().unwrap_or(&Value::Undefined)) else { return Ok(Value::Bool(false)) };
        let Some(d) = &i.doc else { return Ok(Value::Bool(false)) };
        let mut cur = Some(c);
        while let Some(x) = cur {
            if x == p { return Ok(Value::Bool(true)); }
            cur = d.nodes[x as usize].parent;
        }
        Ok(Value::Bool(false))
    }, 1, &fp);
    // Anmelden, aber noch nicht zustellen. Ein `addEventListener`, das WIRFT,
    // beendet das Skript — eins, das die Anmeldung nur aufbewahrt, laesst es
    // weiterlaufen. Die Zustellung setzt genau hier an.
    meth(&node_proto, "addEventListener", |i, t, a| {
        let id = node_of(i, &t)?;
        let ev = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if let Some(d) = &mut i.doc { d.nodes[id as usize].listeners.push((ev, f)); }
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&node_proto, "removeEventListener", |i, t, a| {
        let id = node_of(i, &t)?;
        let ev = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc {
            d.nodes[id as usize].listeners.retain(|(e, _)| *e != ev);
        }
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&node_proto, "dispatchEvent", |_, _, _| Ok(Value::Bool(true)), 1, &fp);

    // ── Element ──────────────────────────────────────────────────────────
    getter(&element_proto, "tagName", |i, t, _| with_node!(i, t, |n| Ok(Value::string(n.tag.to_uppercase()))), &fp);
    getter(&element_proto, "children", |i, t, _| {
        let id = node_of(i, &t)?;
        let cs: Vec<u32> = i.doc.as_ref().map(|d| d.nodes[id as usize].children.iter()
            .copied().filter(|&c| d.nodes[c as usize].kind == ELEMENT_NODE).collect()).unwrap_or_default();
        Ok(nodes_array(i, cs))
    }, &fp);
    getter(&element_proto, "firstElementChild", |i, t, _| {
        let id = node_of(i, &t)?;
        let c = i.doc.as_ref().and_then(|d| d.nodes[id as usize].children.iter()
            .copied().find(|&c| d.nodes[c as usize].kind == ELEMENT_NODE));
        Ok(match c { Some(x) => wrap(i, x), None => Value::Null })
    }, &fp);
    accessor(&element_proto, "id",
        |i, t, _| with_node!(i, t, |n| Ok(match n.attr("id") { Some(v) => Value::Str(v.clone()), None => Value::str("") })),
        |i, t, a| { let id = node_of(i, &t)?; let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                    if let Some(d) = &mut i.doc { d.nodes[id as usize].set_attr("id", &v); }
                    Ok(Value::Undefined) }, &fp);
    accessor(&element_proto, "className",
        |i, t, _| with_node!(i, t, |n| Ok(match n.attr("class") { Some(v) => Value::Str(v.clone()), None => Value::str("") })),
        |i, t, a| { let id = node_of(i, &t)?; let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                    if let Some(d) = &mut i.doc { d.nodes[id as usize].set_attr("class", &v); }
                    Ok(Value::Undefined) }, &fp);
    meth(&element_proto, "getAttribute", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        with_node!(i, t, |n| Ok(match n.attr(&k) { Some(v) => Value::Str(v.clone()), None => Value::Null }))
    }, 1, &fp);
    meth(&element_proto, "hasAttribute", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        with_node!(i, t, |n| Ok(Value::Bool(n.attr(&k).is_some())))
    }, 1, &fp);
    meth(&element_proto, "setAttribute", |i, t, a| {
        let id = node_of(i, &t)?;
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc { d.nodes[id as usize].set_attr(&k, &v); }
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&element_proto, "removeAttribute", |i, t, a| {
        let id = node_of(i, &t)?;
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc { d.nodes[id as usize].attrs.retain(|(n, _)| *n != k); }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&element_proto, "matches", |i, t, a| {
        let id = node_of(i, &t)?;
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(i.doc.as_ref().is_some_and(|d| selector_match(d, id, &s))))
    }, 1, &fp);
    meth(&element_proto, "remove", |i, t, _| {
        let id = node_of(i, &t)?;
        if let Some(d) = &mut i.doc { d.detach(id); }
        Ok(Value::Undefined)
    }, 0, &fp);
    accessor(&element_proto, "innerHTML",
        |i, t, _| { let id = node_of(i, &t)?;
                    let s = i.doc.as_ref().map(|d| d.serialize(id, true)).unwrap_or_default();
                    Ok(Value::string(s)) },
        |i, t, a| {
            let id = node_of(i, &t)?;
            let html = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let Some(d) = &mut i.doc else { return Ok(Value::Undefined) };
            d.clear_children(id);
            d.parse_into(id, &html, None);
            Ok(Value::Undefined)
        }, &fp);
    getter(&element_proto, "outerHTML", |i, t, _| {
        let id = node_of(i, &t)?;
        let s = i.doc.as_ref().map(|d| d.serialize(id, false)).unwrap_or_default();
        Ok(Value::string(s))
    }, &fp);
    meth(&element_proto, "insertAdjacentHTML", |i, t, a| {
        let id = node_of(i, &t)?;
        let pos = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_lowercase();
        let html = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        let Some(d) = &mut i.doc else { return Ok(Value::Undefined) };
        // Die vier Stellen der Spezifikation: vor/nach dem Element selbst,
        // und ganz vorn/hinten in ihm.
        let (parent, at) = match pos.as_str() {
            "afterbegin" => (id, Some(0)),
            "beforeend" => (id, None),
            "beforebegin" | "afterend" => {
                let Some(p) = d.nodes[id as usize].parent else { return Ok(Value::Undefined) };
                let k = d.nodes[p as usize].children.iter().position(|&c| c == id).unwrap_or(0);
                (p, Some(if pos == "beforebegin" { k } else { k + 1 }))
            }
            _ => return i.type_err("invalid insertAdjacentHTML position"),
        };
        d.parse_into(parent, &html, at);
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&node_proto, "cloneNode", |i, t, a| {
        let id = node_of(i, &t)?;
        let deep = a.first().map(|v| v.truthy()).unwrap_or(false);
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let new = d.clone_node(id, deep);
        Ok(wrap(i, new))
    }, 1, &fp);
    meth(&node_proto, "hasChildNodes", |i, t, _| {
        let id = node_of(i, &t)?;
        Ok(Value::Bool(i.doc.as_ref().is_some_and(|d| !d.nodes[id as usize].children.is_empty())))
    }, 0, &fp);
    // Geometrie gibt es zum Skriptzeitpunkt NICHT: beak legt erst NACH den
    // Skripten aus. Ein Nullrechteck ist damit keine Luege ueber die Groesse,
    // sondern die Wahrheit ueber den Zeitpunkt — und ein fehlendes
    // `getBoundingClientRect` beendet das Skript, was schlimmer ist.
    // Sobald Ereignisse zugestellt werden, laeuft ein Skript NACH dem Layout
    // und dann gehoert hier die echte Zahl her.
    meth(&element_proto, "getBoundingClientRect", |i, _, _| {
        let g = new_obj(Some(i.realm.object_proto.clone()));
        for k in ["x", "y", "top", "left", "right", "bottom", "width", "height"] {
            g.borrow_mut().define(k, Prop::data(Value::Num(0.0)));
        }
        Ok(Value::Obj(g))
    }, 0, &fp);
    for k in ["offsetWidth", "offsetHeight", "clientWidth", "clientHeight",
              "scrollWidth", "scrollHeight", "scrollTop", "scrollLeft", "offsetTop", "offsetLeft"] {
        getter(&element_proto, k, |_, _, _| Ok(Value::Num(0.0)), &fp);
    }
    getter(&element_proto, "classList", |i, t, _| {
        // Frisch je Zugriff — `el.classList === el.classList` ist damit
        // falsch, waehrend ein Browser dasselbe Objekt liefert. Gemerkt, weil
        // es eines Tages auffaellt; die Liste selbst arbeitet auf dem Element.
        let id = node_of(i, &t)?;
        let g = new_obj(Some(i.realm.object_proto.clone()));
        g.borrow_mut().define(SLOT, Prop { value: Some(Value::Num(id as f64)), get: None,
            set: None, writable: false, enumerable: false, configurable: false });
        let fp2 = i.realm.function_proto.clone();
        meth(&g, "contains", |i, t, a| {
            let id = node_of(i, &t)?;
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            Ok(Value::Bool(i.doc.as_ref().is_some_and(|d| d.classes(id).iter().any(|c| *c == k))))
        }, 1, &fp2);
        meth(&g, "add", |i, t, a| {
            let id = node_of(i, &t)?;
            for v in a {
                let k = i.to_string(v)?;
                let Some(d) = &mut i.doc else { break };
                let mut cs = d.classes(id);
                if !cs.iter().any(|c| *c == k) { cs.push(k); }
                let joined = cs.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
                d.nodes[id as usize].set_attr("class", &joined);
            }
            Ok(Value::Undefined)
        }, 1, &fp2);
        meth(&g, "remove", |i, t, a| {
            let id = node_of(i, &t)?;
            for v in a {
                let k = i.to_string(v)?;
                let Some(d) = &mut i.doc else { break };
                let cs: Vec<Rc<str>> = d.classes(id).into_iter().filter(|c| *c != k).collect();
                let joined = cs.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
                d.nodes[id as usize].set_attr("class", &joined);
            }
            Ok(Value::Undefined)
        }, 1, &fp2);
        meth(&g, "toggle", |i, t, a| {
            let id = node_of(i, &t)?;
            let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let has = i.doc.as_ref().is_some_and(|d| d.classes(id).iter().any(|c| *c == k));
            let Some(d) = &mut i.doc else { return Ok(Value::Bool(false)) };
            let mut cs: Vec<Rc<str>> = d.classes(id).into_iter().filter(|c| *c != k).collect();
            if !has { cs.push(k); }
            let joined = cs.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
            d.nodes[id as usize].set_attr("class", &joined);
            Ok(Value::Bool(!has))
        }, 1, &fp2);
        Ok(Value::Obj(g))
    }, &fp);
    // `style` ist ein leeres Objekt: Schreibzugriffe laufen ins Leere, statt
    // zu werfen. Ein Stumpf, der die Seite weiterlaufen laesst — und der
    // NICHTS vortaeuscht, weil er auch nichts zurueckliest.
    getter(&element_proto, "style", |i, _, _| {
        Ok(Value::Obj(new_obj(Some(i.realm.object_proto.clone()))))
    }, &fp);

    // querySelector & Co. auf Element wie auf Document.
    for target in [&element_proto, &document_proto] {
        meth(target, "querySelector", |i, t, a| {
            let id = node_of(i, &t)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let found = i.doc.as_ref().map(|d| query(d, id, &s, false)).unwrap_or_default();
            Ok(match found.first() { Some(&x) => wrap(i, x), None => Value::Null })
        }, 1, &fp);
        meth(target, "querySelectorAll", |i, t, a| {
            let id = node_of(i, &t)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let found = i.doc.as_ref().map(|d| query(d, id, &s, true)).unwrap_or_default();
            Ok(nodes_array(i, found))
        }, 1, &fp);
        meth(target, "getElementsByTagName", |i, t, a| {
            let id = node_of(i, &t)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let mut all = Vec::new();
            if let Some(d) = &i.doc { d.descendants(id, &mut all); }
            let found: Vec<u32> = match &i.doc {
                Some(d) => all.into_iter().filter(|&x| &*s == "*" || d.nodes[x as usize].tag.eq_ignore_ascii_case(&s)).collect(),
                None => Vec::new(),
            };
            Ok(nodes_array(i, found))
        }, 1, &fp);
        meth(target, "getElementsByClassName", |i, t, a| {
            let id = node_of(i, &t)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let mut all = Vec::new();
            if let Some(d) = &i.doc { d.descendants(id, &mut all); }
            let found: Vec<u32> = match &i.doc {
                Some(d) => all.into_iter().filter(|&x| d.classes(x).iter().any(|c| **c == *s)).collect(),
                None => Vec::new(),
            };
            Ok(nodes_array(i, found))
        }, 1, &fp);
    }

    // ── Document ─────────────────────────────────────────────────────────
    getter(&document_proto, "documentElement", |i, _, _| {
        match i.doc.as_ref().and_then(|d| d.html) { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    getter(&document_proto, "body", |i, _, _| {
        match i.doc.as_ref().and_then(|d| d.body) { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    getter(&document_proto, "head", |i, _, _| {
        match i.doc.as_ref().and_then(|d| d.head) { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    getter(&document_proto, "readyState", |_, _, _| Ok(Value::str("complete")), &fp);
    meth(&document_proto, "getElementById", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let found = match &i.doc {
            Some(d) => { let mut all = Vec::new(); d.descendants(d.doc, &mut all);
                         all.into_iter().find(|&x| d.nodes[x as usize].attr("id").map(|v| &**v) == Some(&*s)) }
            None => None,
        };
        Ok(match found { Some(x) => wrap(i, x), None => Value::Null })
    }, 1, &fp);
    meth(&document_proto, "createElement", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let lower = s.to_lowercase();
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, &lower);
        Ok(wrap(i, id))
    }, 1, &fp);
    meth(&document_proto, "createTextNode", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(TEXT_NODE, "#text");
        d.nodes[id as usize].text = s;
        Ok(wrap(i, id))
    }, 1, &fp);
    meth(&document_proto, "createDocumentFragment", |i, _, _| {
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, "#fragment");
        Ok(wrap(i, id))
    }, 0, &fp);

    realm.node_proto = node_proto;
    realm.element_proto = element_proto;
    realm.text_proto = text_proto;
    realm.document_proto = document_proto;
}

fn sibling(i: &mut Interp, this: &Value, dir: i32) -> C<Value> {
    let id = node_of(i, this)?;
    let Some(d) = &i.doc else { return Ok(Value::Null) };
    let Some(p) = d.nodes[id as usize].parent else { return Ok(Value::Null) };
    let cs = &d.nodes[p as usize].children;
    let Some(pos) = cs.iter().position(|&c| c == id) else { return Ok(Value::Null) };
    let next = if dir > 0 { pos.checked_add(1) } else { pos.checked_sub(1) };
    let target = next.and_then(|k| cs.get(k).copied());
    Ok(match target { Some(x) => wrap(i, x), None => Value::Null })
}
